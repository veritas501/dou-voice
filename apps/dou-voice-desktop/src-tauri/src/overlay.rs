#[cfg(target_os = "macos")]
use tauri::WebviewUrl;
use tauri::{App, AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow, Wry};
#[cfg(not(target_os = "macos"))]
use tauri::{WebviewUrl, WebviewWindowBuilder};
#[cfg(target_os = "macos")]
use tauri_nspanel::{tauri_panel, CollectionBehavior, PanelBuilder, PanelLevel, StyleMask};

use crate::app_state::{
    DesktopState, VoiceStatus, OVERLAY_BOTTOM_MARGIN_PX, OVERLAY_HEIGHT, OVERLAY_HIDE_DELAY,
    OVERLAY_LABEL, OVERLAY_WIDTH,
};
use crate::settings::overlay_enabled;

const OVERLAY_HIDE_ANIMATION_DELAY: std::time::Duration = std::time::Duration::from_millis(720);

#[cfg(target_os = "macos")]
tauri_panel! {
    panel!(DouVoiceOverlayPanel {
        config: {
            can_become_key_window: false,
            is_floating_panel: true
        }
    })
}

/// 根据当前语音状态更新 overlay 显隐。
pub(crate) fn update_overlay_status(app: &AppHandle<Wry>) {
    let Some(window) = app.get_webview_window(OVERLAY_LABEL) else {
        return;
    };

    if !overlay_enabled(app).unwrap_or(true) {
        let _ = window.hide();
        return;
    }

    let status = {
        let state = app.state::<DesktopState>();
        state.voice_status.lock().map(|status| status.clone()).ok()
    };
    let Some(status) = status else {
        return;
    };

    if overlay_should_show(&status) {
        position_overlay_window(&window);
        let _ = window.show();
        if overlay_should_auto_hide(&status.phase) {
            schedule_overlay_hide(app.clone(), status.phase);
        }
    } else {
        let _ = window.hide();
    }
}

/// 立即隐藏 overlay，不根据当前语音状态重新显示。
pub(crate) fn hide_overlay(app: &AppHandle<Wry>) {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = window.hide();
    }
}

fn overlay_should_show(status: &VoiceStatus) -> bool {
    status.phase != "idle"
        || status
            .last_text
            .as_deref()
            .is_some_and(|text| !text.is_empty())
}

fn overlay_should_auto_hide(phase: &str) -> bool {
    matches!(phase, "idle" | "error")
}

fn position_overlay_window(window: &WebviewWindow<Wry>) {
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };
    let work_area = monitor.work_area();
    let Ok(size) = window.outer_size() else {
        return;
    };

    let work_width = work_area.size.width as i32;
    let work_height = work_area.size.height as i32;
    let window_width = size.width as i32;
    let window_height = size.height as i32;
    let centered_x = work_area.position.x + ((work_width - window_width) / 2).max(0);
    let bottom_y =
        work_area.position.y + (work_height - window_height - OVERLAY_BOTTOM_MARGIN_PX).max(0);
    let _ = window.set_position(PhysicalPosition::new(centered_x, bottom_y));
}

fn schedule_overlay_hide(app: AppHandle<Wry>, phase: String) {
    std::thread::spawn(move || {
        std::thread::sleep(OVERLAY_HIDE_DELAY);
        let should_hide = {
            let state = app.state::<DesktopState>();
            state
                .voice_status
                .lock()
                .map(|status| status.phase == phase)
                .unwrap_or(false)
        };
        if should_hide {
            if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
                let _ = window.emit("overlay-hide-request", ());
                std::thread::sleep(OVERLAY_HIDE_ANIMATION_DELAY);
                let _ = window.hide();
            }
        }
    });
}

/// 创建 overlay 窗口。
///
/// Windows 已验证为 non-activating；Linux 先使用 Tauri 透明置顶窗口作为基础能力，
/// macOS 使用 NSPanel 保持非激活浮层行为。
#[cfg(not(target_os = "macos"))]
pub(crate) fn setup_overlay(app: &mut App<Wry>) -> tauri::Result<()> {
    let window =
        WebviewWindowBuilder::new(&*app, OVERLAY_LABEL, WebviewUrl::App("overlay.html".into()))
            .title("Dou Voice Status")
            .inner_size(OVERLAY_WIDTH, OVERLAY_HEIGHT)
            .resizable(false)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .transparent(true)
            .shadow(false)
            .focused(false)
            .focusable(false)
            .visible(false)
            .build()?;
    let _ = window.set_focusable(false);
    position_overlay_window(&window);
    Ok(())
}

/// 创建 macOS overlay NSPanel。
///
/// PanelBuilder 会先创建 Tauri webview window，再转为 NSPanel；窗口仍会注册在 Tauri 中，
/// 后续可继续通过 `get_webview_window(OVERLAY_LABEL)` 定位、显示和隐藏。
#[cfg(target_os = "macos")]
pub(crate) fn setup_overlay(app: &mut App<Wry>) -> tauri::Result<()> {
    let panel = PanelBuilder::<_, DouVoiceOverlayPanel>::new(app.handle(), OVERLAY_LABEL)
        .url(WebviewUrl::App("overlay.html".into()))
        .title("Dou Voice Status")
        .level(PanelLevel::Status)
        .size(tauri::Size::Logical(tauri::LogicalSize {
            width: OVERLAY_WIDTH,
            height: OVERLAY_HEIGHT,
        }))
        .has_shadow(false)
        .transparent(true)
        .no_activate(true)
        .corner_radius(0.0)
        .style_mask(StyleMask::empty().borderless().nonactivating_panel())
        .with_window(|window| {
            window
                .decorations(false)
                .transparent(true)
                .focusable(false)
                .visible(false)
        })
        .collection_behavior(
            CollectionBehavior::new()
                .can_join_all_spaces()
                .full_screen_auxiliary(),
        )
        .build()?;

    panel.hide();
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        position_overlay_window(&window);
    }
    Ok(())
}
