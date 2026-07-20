//! Windows 低级键盘钩子：在完整匹配 press-to-talk 热键时吞掉主键，阻止前台窗口收到字符。
//!
//! 安全边界（刻意收窄，避免误伤其他程序）：
//! - 只吞配置热键的 **主键** key-down / key-up，从不吞 Ctrl/Alt/Shift/Win
//! - 修饰键必须与配置 **精确一致**（未配置的修饰键必须未按下）
//! - modifier-only 热键无主键，不做任何吞键
//! - 跳过注入事件（`LLKHF_INJECTED`），避免影响 enigo / 剪贴板粘贴
//! - 钩子安装失败时 fail-open：轮询热键仍可用，仅失去吞键能力
//! - 设置页捕获热键期间通过 `set_hotkey_swallow_enabled(false)` 关闭

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT,
    LLKHF_INJECTED, MSG, PM_REMOVE, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};

use super::windows_hotkey::{is_modifier_vkey, modifiers_match, vkey_for_key};
use super::{parse_hotkey, HotkeySpec};

/// 当前要吞键的热键规格；`None` 表示不吞。
static TARGET_SPEC: Mutex<Option<HotkeySpec>> = Mutex::new(None);

/// 捕获热键或其它原因需要临时关闭吞键。
static SWALLOW_ENABLED: AtomicBool = AtomicBool::new(true);

/// 钩子是否已安装。
static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

/// 已吞下 key-down、等待匹配 key-up 的主键 VK（0 表示无）。
///
/// 双重用途：
/// 1. key-up 对称吞键（用户先松修饰键再松主键时前台也不该收到 Z）
/// 2. 作为「物理仍按住」合成态，供 `hotkey_pressed` 轮询（见 `is_swallowed_main_key_held`）
static SWALLOWED_MAIN_VK: AtomicU16 = AtomicU16::new(0);

/// 钩子线程的 Win32 线程 id，用于 PostThreadMessage(WM_QUIT) 干净退出。
static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);

static HOOK_HANDLE: Mutex<Option<isize>> = Mutex::new(None);
static HOOK_JOIN: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

/// 启动全局低级键盘钩子（幂等）。应在 Windows 桌面端启动时调用一次。
pub fn start_hotkey_key_swallow() -> Result<(), String> {
    if HOOK_INSTALLED.load(Ordering::SeqCst) {
        return Ok(());
    }

    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

    let handle = thread::Builder::new()
        .name("dou-voice-hotkey-llhook".into())
        .spawn(move || run_hook_thread(ready_tx))
        .map_err(|error| format!("Could not spawn hotkey swallow thread: {error}"))?;

    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(())) => {
            if let Ok(mut join) = HOOK_JOIN.lock() {
                *join = Some(handle);
            }
            Ok(())
        }
        Ok(Err(error)) => {
            let _ = handle.join();
            Err(error)
        }
        Err(_) => {
            // 超时：尝试通知线程退出，不阻塞启动。
            let tid = HOOK_THREAD_ID.load(Ordering::SeqCst);
            if tid != 0 {
                unsafe {
                    let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
                }
            }
            let _ = handle.join();
            Err("Hotkey swallow hook timed out during startup".to_string())
        }
    }
}

/// 停止钩子并等待线程退出（幂等）。
pub fn stop_hotkey_key_swallow() {
    let tid = HOOK_THREAD_ID.swap(0, Ordering::SeqCst);
    if tid != 0 {
        unsafe {
            let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }

    if let Ok(mut join) = HOOK_JOIN.lock() {
        if let Some(handle) = join.take() {
            let _ = handle.join();
        }
    }

    HOOK_INSTALLED.store(false, Ordering::SeqCst);
    SWALLOWED_MAIN_VK.store(0, Ordering::SeqCst);
}

/// 更新需要吞主键的热键字符串；解析失败或 modifier-only 时清空目标（不吞）。
pub fn set_swallowed_hotkey(shortcut: Option<&str>) {
    let next = shortcut
        .and_then(parse_hotkey)
        .filter(|spec| spec.key.is_some());
    if let Ok(mut guard) = TARGET_SPEC.lock() {
        *guard = next;
    }
    // 热键变更时清掉待匹配的 key-up，避免跨配置误吞。
    SWALLOWED_MAIN_VK.store(0, Ordering::SeqCst);
}

/// 设置页捕获热键时关闭吞键，恢复后重新打开。
pub fn set_hotkey_swallow_enabled(enabled: bool) {
    SWALLOW_ENABLED.store(enabled, Ordering::SeqCst);
    if !enabled {
        SWALLOWED_MAIN_VK.store(0, Ordering::SeqCst);
    }
}

/// 钩子是否正在按住已吞掉的主键。
///
/// `WH_KEYBOARD_LL` 吞键后，系统通常**不会**再更新该键的 async key state，
/// 因此 `GetAsyncKeyState` 会读成未按下。轮询热键必须合并此状态，否则
/// press-to-talk 永远触发不了。
pub fn is_swallowed_main_key_held(vkey: u16) -> bool {
    let pending = SWALLOWED_MAIN_VK.load(Ordering::Relaxed);
    pending != 0 && pending == vkey
}

fn run_hook_thread(ready_tx: std::sync::mpsc::Sender<Result<(), String>>) {
    let thread_id = unsafe { GetCurrentThreadId() };
    HOOK_THREAD_ID.store(thread_id, Ordering::SeqCst);

    let install_result = install_hook();
    let _ = ready_tx.send(install_result.clone());
    if install_result.is_err() {
        HOOK_THREAD_ID.store(0, Ordering::SeqCst);
        return;
    }

    HOOK_INSTALLED.store(true, Ordering::SeqCst);

    // 低级钩子回调在安装它的线程上执行，必须泵消息。
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    uninstall_hook();
    HOOK_INSTALLED.store(false, Ordering::SeqCst);
    HOOK_THREAD_ID.store(0, Ordering::SeqCst);
    SWALLOWED_MAIN_VK.store(0, Ordering::SeqCst);
}

fn install_hook() -> Result<(), String> {
    unsafe {
        // SAFETY: 传 null 模块名取当前进程 HMODULE；LL hook 需要进程模块句柄。
        let module = GetModuleHandleW(windows::core::PCWSTR::null())
            .map_err(|error| format!("Could not get module handle for keyboard hook: {error}"))?;
        let hook = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(low_level_keyboard_proc),
            // HMODULE 与 HINSTANCE 在 Win32 上同布局。
            Some(HINSTANCE(module.0)),
            0,
        )
        .map_err(|error| format!("Could not install low-level keyboard hook: {error}"))?;

        if let Ok(mut guard) = HOOK_HANDLE.lock() {
            *guard = Some(hook.0 as isize);
        }
        Ok(())
    }
}

fn uninstall_hook() {
    let handle = HOOK_HANDLE
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
        .map(|raw| HHOOK(raw as *mut core::ffi::c_void));
    if let Some(hook) = handle {
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
        }
    }

    // 抽干可能残留的退出消息。
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {}
    }
}

/// 低级键盘钩子回调：匹配配置热键主键时吞掉，否则原样传递。
///
/// # Safety
///
/// 由系统在钩子线程上调用；`lparam` 指向有效的 `KBDLLHOOKSTRUCT`。
unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        if let Some(decision) = swallow_decision(wparam, lparam) {
            if decision {
                // 非零返回值阻止事件继续派发到前台窗口。
                return LRESULT(1);
            }
        }
    }

    let hook = HOOK_HANDLE
        .lock()
        .ok()
        .and_then(|guard| (*guard).map(|raw| HHOOK(raw as *mut core::ffi::c_void)));
    // hook 句柄可为 None：系统仍会沿钩子链传递。
    CallNextHookEx(hook, code, wparam, lparam)
}

/// 判断是否应吞掉本次键盘事件。
///
/// 返回 `Some(true)` 吞掉，`Some(false)`/`None` 放行。
unsafe fn swallow_decision(wparam: WPARAM, lparam: LPARAM) -> Option<bool> {
    if !SWALLOW_ENABLED.load(Ordering::Relaxed) {
        return Some(false);
    }

    let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
    if info.flags.contains(LLKHF_INJECTED) {
        return Some(false);
    }

    let vk = info.vkCode;
    if is_modifier_vkey(vk) {
        return Some(false);
    }

    let is_key_down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
    let is_key_up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
    if !is_key_down && !is_key_up {
        return Some(false);
    }

    // 已吞下的主键：其 key-up 必须继续吞，避免前台收到不对称事件。
    let pending = u32::from(SWALLOWED_MAIN_VK.load(Ordering::Relaxed));
    if is_key_up && pending != 0 && pending == vk {
        SWALLOWED_MAIN_VK.store(0, Ordering::Relaxed);
        return Some(true);
    }

    let spec = TARGET_SPEC.lock().ok().and_then(|guard| *guard)?;
    let Some(main_key) = spec.key else {
        return Some(false);
    };
    let main_vk = vkey_for_key(main_key);
    if !vkeys_equal(main_vk, vk) {
        return Some(false);
    }

    // 主键事件：仅在修饰键与配置完全一致时吞。
    // key-down 时记录，保证后续 key-up 即使修饰键已松也能吞掉。
    if !modifiers_match(spec) {
        return Some(false);
    }

    if is_key_down {
        SWALLOWED_MAIN_VK.store(vk as u16, Ordering::Relaxed);
        return Some(true);
    }

    if is_key_up {
        SWALLOWED_MAIN_VK.store(0, Ordering::Relaxed);
        return Some(true);
    }

    Some(false)
}

fn vkeys_equal(expected: VIRTUAL_KEY, actual: u32) -> bool {
    u32::from(expected.0) == actual
}

#[cfg(test)]
mod tests {
    use super::super::{parse_hotkey, HotkeyKey};

    #[test]
    fn only_hotkeys_with_main_key_are_swallow_candidates() {
        let with_key = parse_hotkey("Ctrl+Alt+Z").expect("parse");
        assert!(with_key.key.is_some());
        assert_eq!(with_key.key, Some(HotkeyKey::Letter('Z')));

        let modifier_only = parse_hotkey("Ctrl+Alt").expect("parse");
        assert!(modifier_only.key.is_none());
    }
}
