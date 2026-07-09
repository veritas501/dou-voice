#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod asr_options;
mod auth_window;
mod build_info;
mod diagnostics;
mod hotkey;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos_hotkey;
mod overlay;
mod settings;
mod tray;
mod util;
mod voice;
mod voice_worker;
mod window;

use app_state::{DesktopState, LoginCaptureState};
use auth_window::{export_auth, open_login_window};
use build_info::get_app_build_info;
use diagnostics::{check_auth_status, export_diagnostics};
use hotkey::{
    begin_hotkey_capture, end_hotkey_capture, normalize_hotkey_candidate, setup_global_shortcut,
};
use overlay::setup_overlay;
use settings::{
    get_available_input_devices, get_default_auth_path, get_settings, initialize_default_auth_path,
    initialize_settings, save_settings,
};
use tray::setup_tray;
use voice::{get_voice_status, record_once_and_type};
use window::should_hide_on_close;

use tauri::WindowEvent;

#[tauri::command]
fn get_app_icon_data_url() -> String {
    use base64::Engine;

    let encoded =
        base64::engine::general_purpose::STANDARD.encode(include_bytes!("../icons/icon.ico"));
    format!("data:image/x-icon;base64,{encoded}")
}

/// Tauri 桌面应用入口。
fn main() {
    let builder = tauri::Builder::default()
        .manage(LoginCaptureState::default())
        .manage(DesktopState::default())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            crate::window::show_main_window(app);
        }));

    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());

    builder
        .setup(|app| {
            initialize_default_auth_path(app.handle())?;
            initialize_settings(app.handle())?;
            setup_overlay(app)?;
            setup_tray(app)?;
            setup_global_shortcut(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if should_hide_on_close(window.label()) {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            open_login_window,
            export_auth,
            export_diagnostics,
            get_default_auth_path,
            get_app_build_info,
            get_app_icon_data_url,
            get_available_input_devices,
            get_settings,
            get_voice_status,
            record_once_and_type,
            save_settings,
            check_auth_status,
            begin_hotkey_capture,
            end_hotkey_capture,
            normalize_hotkey_candidate
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
