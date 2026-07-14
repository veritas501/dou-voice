//! 系统输出音量 duck / restore 适配层。
//!
//! 语音输入期间压低默认播放设备主音量，避免背景音乐干扰 ASR；结束后恢复到进入前的电平。
//! - Windows：Core Audio `IAudioEndpointVolume`
//! - macOS：`osascript` 读写 `output volume`（0–100，相对 factor）
//! - Linux：`wpctl` / `pactl` 读写默认 sink 音量（相对 factor）
//!
//! 错误文案保持英文、尽量直白（缺工具 / 无设备 / 解析失败 / 写回失败），不 panic。

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod process;

#[cfg(windows)]
mod windows_volume;

#[cfg(target_os = "macos")]
mod macos_volume;

#[cfg(target_os = "linux")]
mod linux_volume;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod unsupported_volume;

#[cfg(target_os = "linux")]
use linux_volume as platform;
#[cfg(target_os = "macos")]
use macos_volume as platform;
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
use unsupported_volume as platform;
#[cfg(windows)]
use windows_volume as platform;

/// 默认压低到当前主音量的比例（0.0–1.0）。
///
/// 这是相对乘数：当前 80% → duck 后约 8%，不是绝对设为 10%。
pub const DEFAULT_DUCK_FACTOR: f32 = 0.1;

/// 当前平台是否支持系统输出音量 duck。
pub fn is_supported() -> bool {
    cfg!(any(windows, target_os = "macos", target_os = "linux"))
}

/// 压低系统默认播放设备主音量。
///
/// - `factor` 为相对当前电平的比例，会被夹到 `[0.0, 1.0]`。
/// - 若已经处于 duck 状态，返回 `Ok(false)` 且不改变音量。
/// - 成功压低返回 `Ok(true)`。
/// - 平台工具缺失或 API 失败时返回错误；调用方应记录诊断并继续语音流程。
/// - 实现保证不 panic：失败只走 `Err(String)`。
pub fn duck_output_volume(factor: f32) -> Result<bool, String> {
    if !factor.is_finite() {
        return Err(format!(
            "Invalid volume duck factor `{factor}` (must be a finite number between 0 and 1)"
        ));
    }
    platform::duck_output_volume(factor.clamp(0.0, 1.0))
}

/// 恢复进入 duck 前保存的主音量。
///
/// - 若当前没有待恢复的 duck 状态，返回 `Ok(false)`。
/// - 成功恢复返回 `Ok(true)`。
/// - 实现保证不 panic：失败只走 `Err(String)`。
pub fn restore_output_volume() -> Result<bool, String> {
    platform::restore_output_volume()
}
