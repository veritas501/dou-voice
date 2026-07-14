//! Windows 文本输入实现：enigo 主路径 + 剪贴板 fallback。
//!
//! 调用顺序：
//! 1. enigo 主路径：`enigo.text(text)` —— 在 Windows 上等价于 `SendInput` Unicode，
//!    跨平台。调用前先做残留修饰键清理（等待热键修饰键释放、补发 key-up、发 ESC 清
//!    菜单激活态）。
//! 2. 剪贴板 fallback：备份原剪贴板 → 写入文本 → 发 Ctrl+V（用扫描码绕键盘布局）
//!    → 还原原剪贴板。
//!
//! 设计要点：
//! - 主路径只发 Unicode 事件（enigo 的 `queue_char`），无虚拟键、无快捷键
//! - 剪贴板 fallback 用 `Key::Other(0x56)` 扫描码，避免俄语/AZERTY/DVORAK 键盘下
//!   Ctrl+V 错位
//! - 剪贴板 fallback 自动还原原内容，不破坏用户剪贴板

use std::mem::size_of;
use std::ptr;
use std::thread::sleep;
use std::time::{Duration, Instant};

use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LMENU,
    VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
};

use super::{TextInputMethod, TextInputOutcome};

// ---------------------------------------------------------------------------
// 常量：修饰键释放、SendInput 分片、剪贴板重试
// ---------------------------------------------------------------------------

/// 等待 press-to-talk 热键修饰键自然释放的最长时间。
///
/// 旧值 120ms 在 Chromium 系浏览器上偶发不够；提到 250ms 后，松开热键到
/// 浏览器消息队列稳定的时间足够覆盖大多数场景。
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_millis(250);
const MODIFIER_RELEASE_POLL: Duration = Duration::from_millis(10);

/// 修饰键释放后再补一段静默时间，降低目标窗口消息队列内残留 key-down 被后续
/// Unicode 事件配对成快捷键的概率。
const POST_MODIFIER_RELEASE_DELAY: Duration = Duration::from_millis(40);

/// ESC key-up 发出后给目标窗口一点时间清掉菜单激活态。
const POST_ESC_RELEASE_DELAY: Duration = Duration::from_millis(20);

const CLIPBOARD_OPEN_ATTEMPTS: usize = 8;
const CLIPBOARD_OPEN_RETRY_DELAY: Duration = Duration::from_millis(20);

/// 写入剪贴板后等待系统刷新，再发 Ctrl+V。
const POST_CLIPBOARD_WRITE_DELAY: Duration = Duration::from_millis(60);

/// 发出 Ctrl+V 后等待目标窗口完成粘贴，再还原剪贴板。
const POST_PASTE_DELAY: Duration = Duration::from_millis(50);

/// 被视为"残留修饰键"的虚拟键集合。
///
/// `release_stuck_modifiers` 会为其中仍处于按下态的键补发 key-up。注意：这只清
/// 键状态，不能撤销目标窗口消息队列里已经入队的 key-down——后者由 ESC key-up
/// 和等待时间共同缓解。
const STUCK_MODIFIERS: &[VIRTUAL_KEY] = &[
    VK_SHIFT,
    VK_LSHIFT,
    VK_RSHIFT,
    VK_CONTROL,
    VK_LCONTROL,
    VK_RCONTROL,
    VK_MENU,
    VK_LMENU,
    VK_RMENU,
    VK_LWIN,
    VK_RWIN,
];

// ---------------------------------------------------------------------------
// 公共入口
// ---------------------------------------------------------------------------

/// 把文本输入到当前焦点窗口。
///
/// 默认走 enigo 主路径（`enigo.text`，等价 SendInput Unicode）；失败时回退到剪贴板 +
/// 模拟 Ctrl+V。两条路径都会自动完成输入，无需用户手动粘贴。
pub fn type_text(text: &str) -> Result<TextInputOutcome, String> {
    if text.is_empty() {
        return Ok(TextInputOutcome {
            method: TextInputMethod::Direct,
            prior_error: None,
        });
    }

    // 第 1 层：enigo 主路径。
    match enigo_type_text(text) {
        Ok(()) => Ok(TextInputOutcome {
            method: TextInputMethod::Direct,
            prior_error: None,
        }),
        Err(direct_error) => {
            // 第 2 层：剪贴板 + Ctrl+V fallback。
            paste_via_clipboard(text)?;
            Ok(TextInputOutcome {
                method: TextInputMethod::Clipboard,
                prior_error: Some(direct_error),
            })
        }
    }
}

/// 仅写入剪贴板，不发送任何快捷键。
///
/// 供用户显式选择"剪贴板"模式时使用：文本写入剪贴板后由 UI 提示用户手动粘贴。
/// 注意：此函数不还原原剪贴板内容（因为用户要事后手动粘贴，还原会覆盖新内容）。
pub fn copy_text_to_clipboard(text: &str) -> Result<TextInputOutcome, String> {
    if text.is_empty() {
        return Ok(TextInputOutcome {
            method: TextInputMethod::Clipboard,
            prior_error: None,
        });
    }
    set_clipboard_text(text)?;
    Ok(TextInputOutcome {
        method: TextInputMethod::Clipboard,
        prior_error: None,
    })
}

// ---------------------------------------------------------------------------
// 第 1 层：enigo 主路径
// ---------------------------------------------------------------------------

/// 用 enigo 把文本输入到当前焦点窗口。
///
/// 关键点：
/// - 注入前等待修饰键自然释放，并补发 key-up 清掉残留状态
/// - 注入前补发一个 ESC key-up，清掉浏览器/菜单的激活态
/// - enigo 的 `text()` 内部对每个 char 发 `KEYEVENTF_UNICODE` 的 key down/up
fn enigo_type_text(text: &str) -> Result<(), String> {
    wait_for_modifiers_to_release();
    release_stuck_modifiers()?;
    release_esc_to_clear_menu()?;
    sleep(POST_MODIFIER_RELEASE_DELAY);

    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| format!("enigo init failed: {e}"))?;
    enigo
        .text(text)
        .map_err(|e| format!("enigo.text failed: {e}"))
}

/// 等待 press-to-talk 热键修饰键自然释放。
fn wait_for_modifiers_to_release() {
    let started = Instant::now();
    while any_modifier_pressed() && started.elapsed() < MODIFIER_RELEASE_TIMEOUT {
        sleep(MODIFIER_RELEASE_POLL);
    }
}

/// 如果修饰键仍被 Windows 认为处于按下态，补发 key-up。
///
/// 用 `GetKeyState` 而非 `GetAsyncKeyState`：前者反映当前线程消息队列的同步态，
/// 后者反映硬件异步态。对于"目标窗口消息队列里是否还有未消化的 key-down"这个问题，
/// `GetKeyState` 更贴近。
fn release_stuck_modifiers() -> Result<(), String> {
    let input_size = i32::try_from(size_of::<INPUT>())
        .map_err(|_| "Windows INPUT size does not fit i32".to_string())?;
    let inputs = STUCK_MODIFIERS
        .iter()
        .copied()
        .filter(|vkey| key_is_pressed_sync(*vkey))
        .map(virtual_key_up_input)
        .collect::<Vec<_>>();
    if inputs.is_empty() {
        return Ok(());
    }
    send_input_events(&inputs, input_size, "modifier release")?;
    Ok(())
}

/// 补发一个 ESC key-up。
///
/// 浏览器、Office 等应用在 Alt 残留时可能激活菜单栏；ESC key-up 能让它们退出菜单态，
/// 避免后续 Unicode 事件的第一个字符被解释成菜单快捷键。只发 key-up，不发 key-down，
/// 因为这里并不想真正触发 ESC 命令，只想清掉"菜单已激活"的状态标志。
fn release_esc_to_clear_menu() -> Result<(), String> {
    let input_size = i32::try_from(size_of::<INPUT>())
        .map_err(|_| "Windows INPUT size does not fit i32".to_string())?;
    let esc_up = virtual_key_up_input(VK_ESCAPE);
    send_input_events(&[esc_up], input_size, "esc release")?;
    sleep(POST_ESC_RELEASE_DELAY);
    Ok(())
}

fn any_modifier_pressed() -> bool {
    STUCK_MODIFIERS.iter().copied().any(key_is_pressed_async)
}

fn key_is_pressed_async(vkey: VIRTUAL_KEY) -> bool {
    unsafe {
        // SAFETY: GetAsyncKeyState accepts any virtual-key code.
        GetAsyncKeyState(i32::from(vkey.0)) < 0
    }
}

fn key_is_pressed_sync(vkey: VIRTUAL_KEY) -> bool {
    unsafe {
        // SAFETY: GetKeyState accepts any virtual-key code. 返回值的最高位为 1 表示
        // 当前线程消息队列认为该键处于按下态。
        (GetKeyState(i32::from(vkey.0)) as u16) & 0x8000 != 0
    }
}

fn send_input_events(inputs: &[INPUT], input_size: i32, context: &str) -> Result<(), String> {
    let expected = u32::try_from(inputs.len())
        .map_err(|_| format!("too many {context} events in one SendInput call"))?;
    let sent = unsafe {
        // SAFETY: `inputs` is a valid slice of initialized INPUT values and `input_size`
        // is exactly the size of INPUT required by the Win32 SendInput contract.
        SendInput(inputs, input_size)
    };
    if sent != expected {
        return Err(format!(
            "SendInput sent {sent}/{expected} {context} events; the target window may reject injected input or run at a higher integrity level"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 第 2 层：剪贴板 + Ctrl+V fallback
// ---------------------------------------------------------------------------

/// 剪贴板 fallback：备份 → 写入 → 发 Ctrl+V → 还原。
///
/// Ctrl+V 通过新建 enigo 实例发送；`Key::Other(0x56)` 是 VK_V 的扫描码，
/// 能绕过非 QWERTY 键盘布局。
fn paste_via_clipboard(text: &str) -> Result<(), String> {
    // 备份原剪贴板文本内容（仅备份 CF_UNICODETEXT；其他格式如图片会丢失）。
    let original = read_clipboard_text().unwrap_or_default();

    set_clipboard_text(text)?;
    sleep(POST_CLIPBOARD_WRITE_DELAY);

    // 用 enigo 发 Ctrl+V。`Key::Other(0x56)` 是 VK_V 扫描码，避免俄语/AZERTY 键盘
    // 下用 `Key::Unicode('v')` 导致的 Ctrl+V 错位。
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("enigo init failed for paste: {e}"))?;
    send_ctrl_v(&mut enigo)?;

    sleep(POST_PASTE_DELAY);

    // 还原原剪贴板内容。失败不阻断主流程（粘贴已成功）。
    if !original.is_empty() {
        let _ = set_clipboard_text(&original);
    } else {
        // 原内容为空时清空剪贴板，避免残留本次文本。
        let _ = clear_clipboard();
    }

    Ok(())
}

/// 发送 Ctrl+V，用扫描码绕过键盘布局。
fn send_ctrl_v(enigo: &mut Enigo) -> Result<(), String> {
    enigo
        .key(Key::Control, Direction::Press)
        .map_err(|e| format!("Could not press Ctrl for paste fallback: {e}"))?;
    enigo
        .key(Key::Other(0x56), Direction::Click)
        .map_err(|e| format!("Could not press V for paste fallback: {e}"))?;

    sleep(Duration::from_millis(100));

    enigo
        .key(Key::Control, Direction::Release)
        .map_err(|e| format!("Could not release Ctrl after paste fallback: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 剪贴板读写（Win32 API）
// ---------------------------------------------------------------------------

fn set_clipboard_text(text: &str) -> Result<(), String> {
    let _clipboard = open_clipboard()?;
    let data = clipboard_utf16(text);
    let bytes = data
        .len()
        .checked_mul(size_of::<u16>())
        .ok_or_else(|| "clipboard text is too large".to_string())?;
    let handle = unsafe {
        // SAFETY: GlobalAlloc is called with a byte count computed from a Vec length.
        GlobalAlloc(GMEM_MOVEABLE, bytes)
    }
    .map_err(|e| format!("GlobalAlloc failed: {e}"))?;

    let locked = unsafe {
        // SAFETY: `handle` was returned by GlobalAlloc and is valid until freed or transferred.
        GlobalLock(handle)
    };
    if locked.is_null() {
        free_global(handle);
        return Err("Could not lock clipboard memory for writing text".to_string());
    }

    unsafe {
        // SAFETY: `locked` points to a movable global memory block of `bytes` bytes, and
        // `data` contains exactly `bytes` bytes of UTF-16 clipboard data including NUL.
        ptr::copy_nonoverlapping(data.as_ptr(), locked.cast::<u16>(), data.len());
        let _ = GlobalUnlock(handle);
    }

    let emptied = unsafe {
        // SAFETY: Clipboard is open for the current thread while `_clipboard` is alive.
        EmptyClipboard()
    };
    if emptied.is_err() {
        free_global(handle);
        return Err("Could not empty the clipboard before writing text".to_string());
    }

    // SetClipboardData 成功后所有权转移给剪贴板，不再需要 free。
    let transferred = unsafe {
        // SAFETY: Clipboard is open and `handle` contains CF_UNICODETEXT-compatible data.
        SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(handle.0)))
    };
    if transferred.is_err() {
        free_global(handle);
        return Err("Could not set clipboard text (another app may own the clipboard)".to_string());
    }

    Ok(())
}

/// 读取当前剪贴板文本（CF_UNICODETEXT）。失败返回 None。
fn read_clipboard_text() -> Option<String> {
    let _clipboard = open_clipboard().ok()?;
    let handle = unsafe {
        // SAFETY: Clipboard is open while `_clipboard` is alive. GetClipboardData returns
        // a handle owned by the clipboard; we only read it via GlobalLock.
        GetClipboardData(u32::from(CF_UNICODETEXT.0))
    }
    .ok()?;
    if handle.is_invalid() {
        return None;
    }

    let locked = unsafe { GlobalLock(HGLOBAL(handle.0)) };
    if locked.is_null() {
        return None;
    }

    // 计算 UTF-16 NUL 结尾字符串长度。
    let ptr_u16: *const u16 = locked.cast();
    let mut len = 0usize;
    unsafe {
        // SAFETY: CF_UNICODETEXT guarantees a NUL-terminated UTF-16 sequence.
        while *ptr_u16.add(len) != 0 {
            len += 1;
        }
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr_u16, len) };
    let text = String::from_utf16_lossy(slice);
    unsafe {
        let _ = GlobalUnlock(HGLOBAL(handle.0));
    }
    Some(text)
}

/// 清空剪贴板（用于 fallback 还原阶段，当原内容为空时）。
fn clear_clipboard() -> Result<(), String> {
    let _clipboard = open_clipboard()?;
    unsafe {
        // SAFETY: Clipboard is open while `_clipboard` is alive.
        EmptyClipboard()
    }
    .map_err(|e| format!("Could not empty the clipboard: {e}"))?;
    Ok(())
}

/// 释放全局内存句柄，忽略错误（仅用于失败路径的清理）。
fn free_global(handle: HGLOBAL) {
    unsafe {
        let _ = windows::Win32::Foundation::GlobalFree(Some(handle));
    }
}

fn clipboard_utf16(text: &str) -> Vec<u16> {
    let mut data = text.encode_utf16().collect::<Vec<_>>();
    data.push(0);
    data
}

fn open_clipboard() -> Result<ClipboardGuard, String> {
    for _ in 0..CLIPBOARD_OPEN_ATTEMPTS {
        let opened = unsafe {
            // SAFETY: Passing a null owner HWND is allowed; the current task owns the open clipboard.
            OpenClipboard(None)
        };
        if opened.is_ok() {
            return Ok(ClipboardGuard);
        }
        sleep(CLIPBOARD_OPEN_RETRY_DELAY);
    }
    Err("Could not open the clipboard after retries (another app may be holding it)".to_string())
}

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: ClipboardGuard is only constructed after OpenClipboard succeeds.
            let _ = CloseClipboard();
        }
    }
}

// ---------------------------------------------------------------------------
// INPUT 构造工具（修饰键清理用）
// ---------------------------------------------------------------------------

fn virtual_key_up_input(vkey: VIRTUAL_KEY) -> INPUT {
    virtual_key_input(vkey, true)
}

fn virtual_key_input(vkey: VIRTUAL_KEY, key_up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vkey,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_utf16_appends_null_terminator() {
        let data = clipboard_utf16("ASR 老");

        assert_eq!(data.last().copied(), Some(0));
        assert_eq!(
            &data[..data.len() - 1],
            &"ASR 老".encode_utf16().collect::<Vec<_>>()
        );
    }

    #[test]
    fn stuck_modifiers_covers_all_modifier_variants() {
        // 确保左右 Shift/Ctrl/Alt/Win 都在列表里。
        let vk_set: std::collections::HashSet<u16> =
            STUCK_MODIFIERS.iter().map(|vk| vk.0).collect();
        for expected in [
            VK_SHIFT.0,
            VK_LSHIFT.0,
            VK_RSHIFT.0,
            VK_CONTROL.0,
            VK_LCONTROL.0,
            VK_RCONTROL.0,
            VK_MENU.0,
            VK_LMENU.0,
            VK_RMENU.0,
            VK_LWIN.0,
            VK_RWIN.0,
        ] {
            assert!(
                vk_set.contains(&expected),
                "missing VK {expected:#x} in STUCK_MODIFIERS"
            );
        }
    }
}
