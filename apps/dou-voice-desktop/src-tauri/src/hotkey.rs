use tauri::{App, AppHandle, Manager, Wry};

#[cfg(windows)]
use crate::app_state::WINDOWS_HOTKEY_POLL_INTERVAL;
use crate::app_state::{DesktopState, DEFAULT_HOTKEY_LABEL, HOTKEY_PRESS_DEBOUNCE};
use crate::diagnostics::emit_voice_debug;
use crate::settings::current_hotkey;
use crate::voice::{
    abort_hotkey_recording, finish_hotkey_recording, hotkey_recording_active,
    note_hotkey_ignored_while_busy, start_hotkey_recording, voice_input_busy,
};

/// 注册全局 press-to-talk 热键。
///
/// 产品按键形态是按下开始录音、弹起停止识别输入。Windows 使用平台轮询，
/// 因此保存设置后下一轮轮询立即生效。
#[cfg(windows)]
pub(crate) fn setup_global_shortcut(app: &mut App<Wry>) -> tauri::Result<()> {
    spawn_windows_modifier_hotkey_listener(app.handle().clone());
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn setup_global_shortcut(app: &mut App<Wry>) -> tauri::Result<()> {
    crate::macos_hotkey::spawn_macos_hotkey_listener(app.handle().clone())
        .map_err(|error| tauri::Error::Anyhow(std::io::Error::other(error).into()))
}

/// 暂停全局热键监听，用于设置页捕获新热键。
#[tauri::command]
pub(crate) fn begin_hotkey_capture(app: AppHandle<Wry>) -> Result<(), String> {
    set_hotkey_capture_active(&app, true)?;
    if matches!(
        mark_hotkey_released(&app),
        HotkeyReleaseDecision::FinishRecording
    ) {
        spawn_hotkey_release(app);
    }
    Ok(())
}

/// 恢复全局热键监听。
#[tauri::command]
pub(crate) fn end_hotkey_capture(app: AppHandle<Wry>) -> Result<(), String> {
    set_hotkey_capture_active(&app, false)
}

/// 校验并规范化设置页捕获到的热键候选值。
#[tauri::command]
pub(crate) fn normalize_hotkey_candidate(shortcut: String) -> Result<String, String> {
    normalize_global_shortcut(&shortcut)
}

/// 保存设置后更新系统全局热键注册。
#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn update_global_shortcut(
    _app: &AppHandle<Wry>,
    _previous: &str,
    _next: &str,
) -> Result<(), String> {
    Ok(())
}

#[cfg(any(windows, target_os = "macos"))]
fn normalize_global_shortcut(shortcut: &str) -> Result<String, String> {
    dou_voice_platform::hotkey::normalize_hotkey_for_current_platform(shortcut)
        .ok_or_else(|| format!("unsupported hotkey on this platform: {shortcut}"))
}

/// 保存设置后更新系统全局热键注册。
#[cfg(all(not(windows), not(target_os = "macos")))]
pub(crate) fn update_global_shortcut(
    app: &AppHandle<Wry>,
    previous: &str,
    next: &str,
) -> Result<(), String> {
    let previous = normalize_global_shortcut(previous)?;
    let next = normalize_global_shortcut(next)?;
    if previous == next {
        return Ok(());
    }

    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let manager = app.global_shortcut();
    if manager.is_registered(previous.as_str()) {
        manager
            .unregister(previous.as_str())
            .map_err(|error| format!("Could not unregister hotkey `{previous}`: {error}"))?;
    }

    if let Err(error) = manager.register(next.as_str()) {
        let _ = manager.register(previous.as_str());
        return Err(format!("Could not register hotkey `{next}`: {error}"));
    }
    Ok(())
}

/// 注册全局 press-to-talk 热键。
///
/// Linux 使用 Tauri global-shortcut 插件注册当前设置里的组合。
/// modifier-only 组合需要平台专用轮询实现，因此不会在该路径注册。
#[cfg(all(not(windows), not(target_os = "macos")))]
pub(crate) fn setup_global_shortcut(app: &mut App<Wry>) -> tauri::Result<()> {
    use tauri_plugin_global_shortcut::ShortcutState;

    // 当前产品形态是 press-to-talk：Pressed 开始录音，Released 停止并识别输入。
    app.handle().plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(|app, _shortcut, event| match event.state {
                ShortcutState::Pressed => trigger_hotkey_pressed(app),
                ShortcutState::Released => trigger_hotkey_released(app),
            })
            .build(),
    )?;
    register_current_global_shortcut(app.handle())?;
    Ok(())
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn register_current_global_shortcut(app: &AppHandle<Wry>) -> tauri::Result<()> {
    let shortcut = current_hotkey_or_default(app);
    register_global_shortcut(app, &shortcut)
        .map_err(|error| tauri::Error::Anyhow(std::io::Error::other(error).into()))
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn register_global_shortcut(app: &AppHandle<Wry>, shortcut: &str) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let shortcut = normalize_global_shortcut(shortcut)?;
    app.global_shortcut()
        .register(shortcut.as_str())
        .map_err(|error| format!("Could not register hotkey `{shortcut}`: {error}"))
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn normalize_global_shortcut(shortcut: &str) -> Result<String, String> {
    dou_voice_platform::hotkey::normalize_hotkey_for_current_platform(shortcut)
        .ok_or_else(|| format!("unsupported hotkey on this platform: {shortcut}"))
}

#[cfg(windows)]
fn spawn_windows_modifier_hotkey_listener(app: AppHandle<Wry>) {
    std::thread::spawn(move || {
        let mut pressed = false;
        let mut hotkey = current_hotkey_or_default(&app);
        loop {
            if hotkey_capture_active(&app) {
                if pressed {
                    pressed = false;
                    trigger_hotkey_released(&app);
                }
                std::thread::sleep(WINDOWS_HOTKEY_POLL_INTERVAL);
                continue;
            }

            let next_hotkey = current_hotkey_or_default(&app);
            if next_hotkey != hotkey {
                if pressed {
                    pressed = false;
                    trigger_hotkey_released(&app);
                }
                hotkey = next_hotkey;
            }

            let next_pressed = dou_voice_platform::hotkey::hotkey_pressed(&hotkey);
            match (pressed, next_pressed) {
                (false, true) => {
                    pressed = true;
                    trigger_hotkey_pressed(&app);
                }
                (true, false) => {
                    pressed = false;
                    trigger_hotkey_released(&app);
                }
                _ => {}
            }
            std::thread::sleep(WINDOWS_HOTKEY_POLL_INTERVAL);
        }
    });
}

pub(crate) fn current_hotkey_or_default(app: &AppHandle<Wry>) -> String {
    current_hotkey(app).unwrap_or_else(|_| DEFAULT_HOTKEY_LABEL.to_string())
}

fn set_hotkey_capture_active(app: &AppHandle<Wry>, active: bool) -> Result<(), String> {
    let state = app.state::<DesktopState>();
    let mut hotkey = state
        .hotkey
        .lock()
        .map_err(|_| "Internal hotkey state is corrupted (mutex poisoned)".to_string())?;
    hotkey.capture_active = active;
    Ok(())
}

#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn hotkey_capture_active(app: &AppHandle<Wry>) -> bool {
    let state = app.state::<DesktopState>();
    state
        .hotkey
        .lock()
        .map(|hotkey| hotkey.capture_active)
        .unwrap_or(false)
}

pub(crate) fn trigger_hotkey_pressed(app: &AppHandle<Wry>) {
    let press_generation = match mark_hotkey_pressed(app) {
        HotkeyPressDecision::Accepted(generation) => generation,
        HotkeyPressDecision::Debounced => {
            emit_voice_debug(
                app,
                "hotkey_debounced",
                "Ignored duplicate hotkey press within debounce window.",
                None,
                None,
                None,
            );
            return;
        }
        HotkeyPressDecision::AlreadyPressed
        | HotkeyPressDecision::SuppressedUntilRelease
        | HotkeyPressDecision::Capturing => return,
    };

    if voice_input_busy(app) {
        suppress_hotkey_until_release(app, press_generation);
        note_hotkey_ignored_while_busy(app);
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match start_hotkey_recording(app.clone()).await {
            Ok(()) => {
                if hotkey_released_after_start(&app, press_generation) {
                    spawn_hotkey_release(app);
                }
            }
            Err(error) => {
                if error.starts_with("Voice input is already running") {
                    suppress_hotkey_until_release(&app, press_generation);
                    note_hotkey_ignored_while_busy(&app);
                } else {
                    let _ = mark_hotkey_released(&app);
                    emit_voice_debug(
                        &app,
                        "hotkey_start_failed",
                        format!("Could not start hotkey recording: {error}"),
                        None,
                        None,
                        None,
                    );
                    eprintln!("Could not start hotkey recording: {error}");
                }
            }
        }
    });
}

pub(crate) fn trigger_hotkey_released(app: &AppHandle<Wry>) {
    match mark_hotkey_released(app) {
        HotkeyReleaseDecision::FinishRecording => spawn_hotkey_release(app.clone()),
        HotkeyReleaseDecision::Ignore => {}
    }
}

/// 手动停止当前热键录音，用于托盘/主窗口逃生路径。
///
/// 这不是时长限制；它只在用户显式点击停止时触发，避免平台层丢失 release 事件后
/// 录音状态无法恢复。
#[tauri::command]
pub(crate) fn stop_hotkey_recording(app: AppHandle<Wry>) -> Result<(), String> {
    if hotkey_recording_active(&app) || hotkey_press_active(&app) {
        force_mark_hotkey_released(&app);
        spawn_hotkey_release(app);
        return Ok(());
    }

    Err("No active hotkey recording to stop".to_string())
}

pub(crate) fn stop_hotkey_recording_if_active(app: AppHandle<Wry>) {
    if hotkey_recording_active(&app) || hotkey_press_active(&app) {
        force_mark_hotkey_released(&app);
        spawn_hotkey_release(app);
    }
}

pub(crate) fn abort_hotkey_recording_if_active(app: &AppHandle<Wry>, message: &str) {
    force_mark_hotkey_released(app);
    let _ = abort_hotkey_recording(app, message);
}

fn spawn_hotkey_release(app: AppHandle<Wry>) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = finish_hotkey_recording(app.clone()).await {
            if error.starts_with("No active recording to finish") {
                return;
            }
            emit_voice_debug(
                &app,
                "hotkey_finish_failed",
                format!("Could not finish hotkey recording: {error}"),
                None,
                None,
                None,
            );
            eprintln!("Could not finish hotkey recording: {error}");
        }
    });
}

enum HotkeyPressDecision {
    Accepted(u64),
    AlreadyPressed,
    Debounced,
    SuppressedUntilRelease,
    Capturing,
}

enum HotkeyReleaseDecision {
    FinishRecording,
    Ignore,
}

fn mark_hotkey_pressed(app: &AppHandle<Wry>) -> HotkeyPressDecision {
    let state = app.state::<DesktopState>();
    let Ok(mut hotkey) = state.hotkey.lock() else {
        return HotkeyPressDecision::SuppressedUntilRelease;
    };
    if hotkey.capture_active {
        return HotkeyPressDecision::Capturing;
    }
    if hotkey.pressed {
        return HotkeyPressDecision::AlreadyPressed;
    }
    if hotkey.suppressed_until_release {
        return HotkeyPressDecision::SuppressedUntilRelease;
    }

    let now = std::time::Instant::now();
    if hotkey
        .last_press_at
        .is_some_and(|last_press| now.duration_since(last_press) < HOTKEY_PRESS_DEBOUNCE)
    {
        hotkey.suppressed_until_release = true;
        return HotkeyPressDecision::Debounced;
    }

    hotkey.last_press_at = Some(now);
    hotkey.press_generation = hotkey.press_generation.wrapping_add(1);
    hotkey.pressed = true;
    HotkeyPressDecision::Accepted(hotkey.press_generation)
}

fn mark_hotkey_released(app: &AppHandle<Wry>) -> HotkeyReleaseDecision {
    let state = app.state::<DesktopState>();
    let Ok(mut hotkey) = state.hotkey.lock() else {
        return HotkeyReleaseDecision::Ignore;
    };
    if hotkey.suppressed_until_release {
        hotkey.suppressed_until_release = false;
        return HotkeyReleaseDecision::Ignore;
    }
    if !hotkey.pressed {
        return HotkeyReleaseDecision::Ignore;
    }
    hotkey.pressed = false;
    HotkeyReleaseDecision::FinishRecording
}

fn force_mark_hotkey_released(app: &AppHandle<Wry>) {
    let state = app.state::<DesktopState>();
    let Ok(mut hotkey) = state.hotkey.lock() else {
        return;
    };
    hotkey.pressed = false;
    hotkey.suppressed_until_release = false;
}

fn hotkey_press_active(app: &AppHandle<Wry>) -> bool {
    let state = app.state::<DesktopState>();
    state
        .hotkey
        .lock()
        .map(|hotkey| hotkey.pressed)
        .unwrap_or(false)
}

fn hotkey_released_after_start(app: &AppHandle<Wry>, generation: u64) -> bool {
    let state = app.state::<DesktopState>();
    let Ok(hotkey) = state.hotkey.lock() else {
        return false;
    };
    hotkey.press_generation == generation && !hotkey.pressed && !hotkey.suppressed_until_release
}

fn suppress_hotkey_until_release(app: &AppHandle<Wry>, generation: u64) {
    let state = app.state::<DesktopState>();
    let Ok(mut hotkey) = state.hotkey.lock() else {
        return;
    };
    if hotkey.press_generation != generation {
        return;
    }
    hotkey.pressed = false;
    hotkey.suppressed_until_release = true;
}
