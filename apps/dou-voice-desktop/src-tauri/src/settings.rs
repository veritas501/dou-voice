use std::fs;
use std::path::PathBuf;

use tauri::{AppHandle, Manager, Wry};

use crate::app_state::{
    AppSettings, AudioInputDeviceResult, DesktopState, SettingsSnapshot, AUTH_FILE_NAME,
    DEFAULT_HOTKEY_LABEL, INPUT_METHOD_CLIPBOARD, INPUT_METHOD_DIRECT, SETTINGS_FILE_NAME,
};
use crate::diagnostics::{auth_status_result, emit_voice_debug};
use crate::hotkey::update_global_shortcut;
use crate::microphone_worker::reconcile_prewarmed_microphone;
use crate::overlay::hide_overlay;
use crate::util::unix_time_ms;

/// 返回桌面端默认 auth.json 路径。
///
/// Windows 下由 Tauri app config 目录决定，通常位于 `%APPDATA%` 下。
#[tauri::command]
pub(crate) fn get_default_auth_path(app: AppHandle<Wry>) -> Result<String, String> {
    Ok(default_auth_path(&app)?.display().to_string())
}

/// 读取设置页快照。
#[tauri::command]
pub(crate) fn get_settings(app: AppHandle<Wry>) -> Result<SettingsSnapshot, String> {
    let state = app.state::<DesktopState>();
    let settings = state
        .settings
        .lock()
        .map_err(|_| "settings state poisoned".to_string())?
        .clone();
    let user_settings_exists = state
        .user_settings_exists
        .lock()
        .map_err(|_| "user settings state poisoned".to_string())
        .map(|exists| *exists)?;

    Ok(SettingsSnapshot {
        settings,
        auth: auth_status_result(&app)?,
        onboarding_required: !user_settings_exists,
    })
}

/// 列出当前可用的录音输入设备。
#[tauri::command]
pub(crate) fn get_available_input_devices() -> Result<Vec<AudioInputDeviceResult>, String> {
    let mut devices = vec![AudioInputDeviceResult {
        id: "default".to_string(),
        name: "Default".to_string(),
        is_default: true,
    }];

    devices.extend(
        dou_voice_core::list_input_devices()
            .map_err(|error| format!("failed to list input devices: {error}"))?
            .into_iter()
            .map(|device| AudioInputDeviceResult {
                id: device.name.clone(),
                name: device.name,
                is_default: device.is_default,
            }),
    );

    Ok(devices)
}

/// 保存设置页当前支持的选项。
#[tauri::command]
pub(crate) fn save_settings(
    app: AppHandle<Wry>,
    settings: AppSettings,
) -> Result<SettingsSnapshot, String> {
    let settings = sanitize_settings(settings);
    let previous_settings = {
        let state = app.state::<DesktopState>();
        let latest = state
            .settings
            .lock()
            .map_err(|_| "settings state poisoned".to_string())?;
        latest.clone()
    };
    let hotkey_changed = previous_settings.hotkey != settings.hotkey;
    let microphone_changed = previous_settings.microphone_always_on
        != settings.microphone_always_on
        || (settings.microphone_always_on
            && previous_settings.selected_input_device != settings.selected_input_device);
    if microphone_changed && voice_input_is_busy(&app)? {
        return Err(
            "cannot change the local microphone mode while voice input is active".to_string(),
        );
    }
    if hotkey_changed {
        update_global_shortcut(&app, &previous_settings.hotkey, &settings.hotkey)?;
    }

    if microphone_changed {
        if let Err(error) = reconcile_prewarmed_microphone(
            &app,
            settings.microphone_always_on,
            settings.selected_input_device.clone(),
        ) {
            if hotkey_changed {
                let _ = update_global_shortcut(&app, &settings.hotkey, &previous_settings.hotkey);
            }
            return Err(format!("failed to update local microphone mode: {error}"));
        }
    }

    if let Err(error) = save_settings_file(&app, &settings) {
        if hotkey_changed {
            let _ = update_global_shortcut(&app, &settings.hotkey, &previous_settings.hotkey);
        }
        if microphone_changed {
            let _ = reconcile_prewarmed_microphone(
                &app,
                previous_settings.microphone_always_on,
                previous_settings.selected_input_device.clone(),
            );
        }
        return Err(error);
    }

    {
        let state = app.state::<DesktopState>();
        let mut latest = state
            .settings
            .lock()
            .map_err(|_| "settings state poisoned".to_string())?;
        *latest = settings.clone();
        let mut user_settings_exists = state
            .user_settings_exists
            .lock()
            .map_err(|_| "user settings state poisoned".to_string())?;
        *user_settings_exists = true;
    }
    if !settings.overlay_enabled {
        hide_overlay(&app);
    }

    Ok(SettingsSnapshot {
        settings,
        auth: auth_status_result(&app)?,
        onboarding_required: false,
    })
}

/// 解析桌面端默认 auth.json 路径。
pub(crate) fn default_auth_path(app: &AppHandle<Wry>) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(AUTH_FILE_NAME))
        .map_err(|error| format!("failed to resolve app config directory: {error}"))
}

/// 解析桌面端设置文件路径。
fn settings_path(app: &AppHandle<Wry>) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(SETTINGS_FILE_NAME))
        .map_err(|error| format!("failed to resolve app config directory: {error}"))
}

/// 生成诊断文件输出路径。
pub(crate) fn diagnostics_output_path(app: &AppHandle<Wry>) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| {
            dir.join("diagnostics")
                .join(format!("diagnostics-{}.json", unix_time_ms()))
        })
        .map_err(|error| format!("failed to resolve diagnostics directory: {error}"))
}

/// 启动时把默认 auth 路径写入运行态。
pub(crate) fn initialize_default_auth_path(app: &AppHandle<Wry>) -> Result<(), String> {
    let path = default_auth_path(app)?;
    let state = app.state::<DesktopState>();
    let mut auth_path = state
        .auth_path
        .lock()
        .map_err(|_| "desktop auth path state poisoned".to_string())?;
    *auth_path = path;
    Ok(())
}

/// 启动时加载设置文件；损坏或缺失时回退默认值。
pub(crate) fn initialize_settings(app: &AppHandle<Wry>) -> Result<(), String> {
    let loaded = load_settings_file(app)?;
    let state = app.state::<DesktopState>();
    let mut latest = state
        .settings
        .lock()
        .map_err(|_| "settings state poisoned".to_string())?;
    *latest = loaded.settings;
    let mut user_settings_exists = state
        .user_settings_exists
        .lock()
        .map_err(|_| "user settings state poisoned".to_string())?;
    *user_settings_exists = loaded.exists;
    Ok(())
}

/// 启动时恢复用户选择的本地麦克风常开模式。
///
/// 设备被其他程序占用或权限未授予时，应用仍应能正常进入设置页，所以只记录诊断，
/// 后续热键会自动回退到按需打开麦克风。
pub(crate) fn initialize_prewarmed_microphone(app: &AppHandle<Wry>) {
    let state = app.state::<DesktopState>();
    let settings = state.settings.lock().map(|settings| settings.clone());
    let Ok(settings) = settings else {
        return;
    };
    if !settings.microphone_always_on {
        return;
    }
    if let Err(error) = reconcile_prewarmed_microphone(
        app,
        settings.microphone_always_on,
        settings.selected_input_device,
    ) {
        emit_voice_debug(
            app,
            "prewarmed_mic_unavailable",
            format!("Failed to restore local microphone stream: {error}"),
            None,
            None,
            None,
        );
    }
}

struct LoadedSettings {
    settings: AppSettings,
    exists: bool,
}

fn load_settings_file(app: &AppHandle<Wry>) -> Result<LoadedSettings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(LoadedSettings {
            settings: AppSettings::default(),
            exists: false,
        });
    }

    let Ok(data) = fs::read_to_string(&path) else {
        return Ok(LoadedSettings {
            settings: AppSettings::default(),
            exists: false,
        });
    };
    let Ok(settings) = serde_json::from_str::<AppSettings>(&data) else {
        return Ok(LoadedSettings {
            settings: AppSettings::default(),
            exists: false,
        });
    };
    Ok(LoadedSettings {
        settings: sanitize_settings(settings),
        exists: true,
    })
}

pub(crate) fn save_settings_file(
    app: &AppHandle<Wry>,
    settings: &AppSettings,
) -> Result<(), String> {
    let path = settings_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create settings directory: {error}"))?;
    }

    let data = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("failed to serialize settings: {error}"))?;
    fs::write(&path, data).map_err(|error| format!("failed to write settings: {error}"))
}

pub(crate) fn sanitize_settings(mut settings: AppSettings) -> AppSettings {
    settings.hotkey = settings.hotkey.trim().to_string();
    settings.hotkey =
        dou_voice_platform::hotkey::normalize_hotkey_for_current_platform(&settings.hotkey)
            .unwrap_or_else(|| DEFAULT_HOTKEY_LABEL.to_string());
    if !dou_voice_platform::hotkey::is_supported_hotkey_for_current_platform(&settings.hotkey) {
        settings.hotkey = DEFAULT_HOTKEY_LABEL.to_string();
    }
    if !matches!(
        settings.input_method.as_str(),
        INPUT_METHOD_DIRECT | INPUT_METHOD_CLIPBOARD
    ) {
        settings.input_method = INPUT_METHOD_DIRECT.to_string();
    }
    settings.selected_input_device = settings
        .selected_input_device
        .as_deref()
        .map(str::trim)
        .filter(|device| !device.is_empty() && *device != "default")
        .map(str::to_string);
    settings
}

/// 读取当前热键 label。
pub(crate) fn current_hotkey(app: &AppHandle<Wry>) -> Result<String, String> {
    let state = app.state::<DesktopState>();
    state
        .settings
        .lock()
        .map_err(|_| "settings state poisoned".to_string())
        .map(|settings| settings.hotkey.clone())
}

/// 读取当前输入方式。
pub(crate) fn current_input_method(app: &AppHandle<Wry>) -> Result<String, String> {
    let state = app.state::<DesktopState>();
    state
        .settings
        .lock()
        .map_err(|_| "settings state poisoned".to_string())
        .map(|settings| settings.input_method.clone())
}

/// 读取当前录音输入设备。None 表示系统默认设备。
pub(crate) fn current_input_device(app: &AppHandle<Wry>) -> Result<Option<String>, String> {
    let state = app.state::<DesktopState>();
    state
        .settings
        .lock()
        .map_err(|_| "settings state poisoned".to_string())
        .map(|settings| settings.selected_input_device.clone())
}

/// 读取当前认证文件路径。
pub(crate) fn current_auth_path(app: &AppHandle<Wry>) -> Result<PathBuf, String> {
    let state = app.state::<DesktopState>();
    state
        .auth_path
        .lock()
        .map_err(|_| "desktop auth path state poisoned".to_string())
        .map(|path| path.clone())
}

/// 读取当前音效开关。
pub(crate) fn sound_enabled(app: &AppHandle<Wry>) -> Result<bool, String> {
    let state = app.state::<DesktopState>();
    state
        .settings
        .lock()
        .map_err(|_| "settings state poisoned".to_string())
        .map(|settings| settings.sound_enabled)
}

/// 读取当前 overlay 开关。
pub(crate) fn overlay_enabled(app: &AppHandle<Wry>) -> Result<bool, String> {
    let state = app.state::<DesktopState>();
    state
        .settings
        .lock()
        .map_err(|_| "settings state poisoned".to_string())
        .map(|settings| settings.overlay_enabled)
}

/// 读取本地麦克风常开开关。
pub(crate) fn microphone_always_on(app: &AppHandle<Wry>) -> Result<bool, String> {
    let state = app.state::<DesktopState>();
    state
        .settings
        .lock()
        .map_err(|_| "settings state poisoned".to_string())
        .map(|settings| settings.microphone_always_on)
}

fn voice_input_is_busy(app: &AppHandle<Wry>) -> Result<bool, String> {
    let state = app.state::<DesktopState>();
    state
        .voice_busy
        .lock()
        .map_err(|_| "voice busy state poisoned".to_string())
        .map(|busy| *busy)
}
