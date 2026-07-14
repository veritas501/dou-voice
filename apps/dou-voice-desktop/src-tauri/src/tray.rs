use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Wry};

use crate::app_state::{VoiceStatus, TRAY_ID, TRAY_QUIT_ID, TRAY_SHOW_ID};

/// 更新托盘 tooltip 和菜单项状态。
pub(crate) fn update_tray_status(app: &AppHandle<Wry>, status: &VoiceStatus) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(tray_tooltip(status)));
        let _ = tray.set_icon(Some(tray_icon_for_phase(app, &status.phase)));
    }
}

/// 生成托盘悬停文案。
fn tray_tooltip(status: &VoiceStatus) -> String {
    format!("Dou Voice - {}", tray_phase_label(&status.phase))
}

/// 将内部 phase 映射为托盘展示文案。
fn tray_phase_label(phase: &str) -> &'static str {
    match phase {
        "idle" => "Ready",
        "starting" => "Starting",
        "recording" => "Recording",
        "stopping" => "Stopping",
        "loading_auth" => "Loading Auth",
        "recognizing" => "Recognizing",
        "typing" => "Typing",
        "error" => "Error",
        _ => "Unknown",
    }
}

/// 生成状态托盘图标。
fn tray_icon_for_phase(app: &AppHandle<Wry>, phase: &str) -> Image<'static> {
    app.default_window_icon()
        .map(|icon| tray_icon_with_status_dot(icon.clone().to_owned(), phase))
        .unwrap_or_else(|| fallback_tray_icon_for_phase(phase))
}

fn tray_icon_with_status_dot(icon: Image<'static>, phase: &str) -> Image<'static> {
    let width = icon.width();
    let height = icon.height();
    if phase == "idle" || width == 0 || height == 0 {
        return icon;
    }

    let [red, green, blue] = tray_phase_color(phase);
    let mut rgba = icon.rgba().to_vec();
    let radius = (width.min(height) as i32 * 7 / 25).clamp(4, 10);
    let white_ring = (radius / 3).max(2);
    let shadow_ring = 1;
    let margin = (radius / 3).max(1);
    let center_x = width as i32 - margin - radius;
    let center_y = margin + radius;
    let white_radius = radius + white_ring;
    let shadow_radius = white_radius + shadow_ring;

    for y in 0..height as i32 {
        for x in 0..width as i32 {
            let dx = x - center_x;
            let dy = y - center_y;
            let distance = dx * dx + dy * dy;
            let pixel = if distance <= radius * radius {
                Some([red, green, blue, 255])
            } else if distance <= white_radius * white_radius {
                Some([255, 255, 255, 255])
            } else if distance <= shadow_radius * shadow_radius {
                Some([17, 24, 39, 220])
            } else {
                None
            };

            if let Some(pixel) = pixel {
                let offset = ((y as u32 * width + x as u32) * 4) as usize;
                rgba[offset..offset + 4].copy_from_slice(&pixel);
            }
        }
    }

    Image::new_owned(rgba, width, height)
}

fn fallback_tray_icon_for_phase(phase: &str) -> Image<'static> {
    const SIZE: u32 = 32;
    let [red, green, blue] = tray_phase_color(phase);
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    let center = (SIZE as i32 - 1) / 2;
    let outer_radius = 15 * 15;
    let inner_radius = if phase == "idle" { 13 * 13 } else { 11 * 11 };

    for y in 0..SIZE as i32 {
        for x in 0..SIZE as i32 {
            let dx = x - center;
            let dy = y - center;
            let distance = dx * dx + dy * dy;
            let pixel = if distance > outer_radius {
                [0, 0, 0, 0]
            } else if distance > inner_radius {
                [28, 32, 39, 255]
            } else if phase == "idle" {
                [248, 250, 252, 255]
            } else {
                [red, green, blue, 255]
            };
            rgba.extend_from_slice(&pixel);
        }
    }

    Image::new_owned(rgba, SIZE, SIZE)
}

/// 将语音输入状态映射为托盘图标主色。
fn tray_phase_color(phase: &str) -> [u8; 3] {
    match phase {
        "idle" => [34, 197, 94],
        "error" => [239, 68, 68],
        "recording" => [14, 165, 233],
        "typing" => [168, 85, 247],
        "recognizing" | "loading_auth" | "starting" | "stopping" => [245, 158, 11],
        _ => [148, 163, 184],
    }
}

/// 创建托盘菜单。
pub(crate) fn setup_tray(app: &mut App<Wry>) -> tauri::Result<()> {
    let show_window = MenuItemBuilder::with_id(TRAY_SHOW_ID, "Show Window").build(app)?;
    let quit = MenuItemBuilder::with_id(TRAY_QUIT_ID, "Quit").build(app)?;
    let menu = MenuBuilder::new(app)
        .items(&[&show_window, &quit])
        .build()?;

    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("Dou Voice - Ready")
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } = event
            {
                crate::window::show_main_window(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            TRAY_SHOW_ID => crate::window::show_main_window(app),
            TRAY_QUIT_ID => {
                // 退出前尽量恢复系统音量，避免异常路径留下低音量。
                if let Err(error) = dou_voice_platform::volume::restore_output_volume() {
                    eprintln!("Could not restore system volume on quit: {error}");
                }
                app.exit(0);
            }
            _ => {}
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }
    tray.build(app)?;
    update_tray_status(app.handle(), &VoiceStatus::idle());
    Ok(())
}
