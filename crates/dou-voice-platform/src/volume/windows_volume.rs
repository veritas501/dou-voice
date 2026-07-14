//! Windows 系统输出音量 duck：读写默认播放设备主音量。
//!
//! 使用 Core Audio：
//! `IMMDeviceEnumerator` → 默认 `eRender` 设备 → `IAudioEndpointVolume`
//! `Get/SetMasterVolumeLevelScalar`。
//!
//! 进程内只允许一层 duck：开始时保存当前 scalar，结束时写回。
//! COM 调用失败返回直白英文错误；不 panic。

use std::sync::Mutex;

use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, S_FALSE};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

/// 进入 duck 前的主音量 scalar；`None` 表示当前未 duck。
static DUCKED_VOLUME: Mutex<Option<f32>> = Mutex::new(None);

/// 压低默认播放设备主音量到 `current * factor`。
pub fn duck_output_volume(factor: f32) -> Result<bool, String> {
    if is_already_ducked()? {
        return Ok(false);
    }

    ensure_com()?;
    let volume = default_render_volume()?;
    let current = read_master_volume(&volume)?;
    let target = (current * factor).clamp(0.0, 1.0);

    // 已经静音或目标与当前几乎相同则不必改写，但仍记录原值以便对称 restore。
    if (current - target).abs() > f32::EPSILON {
        set_master_volume(&volume, target).map_err(|error| {
            format!(
                "Could not lower Windows output volume from {:.0}% to {:.0}%: {error}",
                current * 100.0,
                target * 100.0
            )
        })?;
    }

    match commit_ducked_volume(current, || set_master_volume(&volume, current)) {
        Ok(true) => Ok(true),
        Ok(false) => Ok(false),
        Err(error) => Err(format!(
            "Lowered Windows output volume to {:.0}% but failed to remember the previous level ({:.0}%): {error}",
            target * 100.0,
            current * 100.0
        )),
    }
}

/// 写回进入 duck 前保存的主音量。
pub fn restore_output_volume() -> Result<bool, String> {
    let previous = take_ducked_volume()?;
    let Some(previous) = previous else {
        return Ok(false);
    };

    ensure_com()?;
    let volume = default_render_volume()?;
    let previous = previous.clamp(0.0, 1.0);
    if let Err(error) = set_master_volume(&volume, previous) {
        // 恢复失败时把原值塞回状态，避免永久丢失用户音量。
        if let Err(store_error) = store_ducked_volume(previous) {
            return Err(format!(
                "Could not restore Windows output volume to {:.0}%: {error}; also lost the saved level: {store_error}",
                previous * 100.0
            ));
        }
        return Err(format!(
            "Could not restore Windows output volume to {:.0}%: {error}",
            previous * 100.0
        ));
    }
    Ok(true)
}

fn is_already_ducked() -> Result<bool, String> {
    let ducked = DUCKED_VOLUME
        .lock()
        .map_err(|_| "internal volume state is corrupted (mutex poisoned)".to_string())?;
    Ok(ducked.is_some())
}

fn take_ducked_volume() -> Result<Option<f32>, String> {
    let mut ducked = DUCKED_VOLUME
        .lock()
        .map_err(|_| "internal volume state is corrupted (mutex poisoned)".to_string())?;
    Ok(ducked.take())
}

fn store_ducked_volume(value: f32) -> Result<(), String> {
    let mut ducked = DUCKED_VOLUME
        .lock()
        .map_err(|_| "internal volume state is corrupted (mutex poisoned)".to_string())?;
    *ducked = Some(value);
    Ok(())
}

/// 记录 duck 前音量。
///
/// - `Ok(true)`：成功记录
/// - `Ok(false)`：竞态下已有 duck，已 rollback 本次改动
/// - `Err`：状态损坏；已尽量 rollback
fn commit_ducked_volume(
    current: f32,
    rollback: impl FnOnce() -> Result<(), String>,
) -> Result<bool, String> {
    match DUCKED_VOLUME.lock() {
        Ok(mut ducked) => {
            if ducked.is_some() {
                // 极端竞态：别人已 duck。回滚我们刚写入的音量。
                let _ = rollback();
                return Ok(false);
            }
            *ducked = Some(current);
            Ok(true)
        }
        Err(_) => {
            let _ = rollback();
            Err("internal volume state is corrupted (mutex poisoned)".to_string())
        }
    }
}

fn read_master_volume(volume: &IAudioEndpointVolume) -> Result<f32, String> {
    // SAFETY: volume 来自 Activate，接口有效；失败以 HRESULT 返回。
    let value = unsafe {
        volume
            .GetMasterVolumeLevelScalar()
            .map_err(|error| format!("GetMasterVolumeLevelScalar failed: {error}"))?
    };
    if !value.is_finite() {
        return Err(format!(
            "GetMasterVolumeLevelScalar returned non-finite value: {value}"
        ));
    }
    Ok(value.clamp(0.0, 1.0))
}

fn set_master_volume(volume: &IAudioEndpointVolume, level: f32) -> Result<(), String> {
    let level = level.clamp(0.0, 1.0);
    // SAFETY: volume 来自 Activate；event context 传 null 表示无事件上下文。
    unsafe {
        volume
            .SetMasterVolumeLevelScalar(level, std::ptr::null())
            .map_err(|error| format!("SetMasterVolumeLevelScalar({level:.3}) failed: {error}"))
    }
}

/// 确保当前线程 COM 可用。已初始化或 apartment 不同时继续，不 CoUninitialize。
fn ensure_com() -> Result<(), String> {
    // SAFETY: CoInitializeEx 对重复调用返回 S_FALSE / RPC_E_CHANGED_MODE，均可继续用 COM。
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_ok() || hr == S_FALSE || hr == RPC_E_CHANGED_MODE {
        return Ok(());
    }
    Err(format!(
        "Could not initialize COM for volume control (HRESULT {hr:?})"
    ))
}

/// 获取默认播放设备的 `IAudioEndpointVolume`。
fn default_render_volume() -> Result<IAudioEndpointVolume, String> {
    // SAFETY: CoCreateInstance / Activate 使用系统 MMDevice API；返回的 COM 接口由 rust
    // windows crate 管理引用计数。
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|error| {
                format!("Could not create Windows audio device enumerator: {error}")
            })?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|error| {
                format!(
                    "No default playback device found (check speakers/headphones are enabled): {error}"
                )
            })?;
        device
            .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            .map_err(|error| {
                format!("Could not open volume control for the default playback device: {error}")
            })
    }
}
