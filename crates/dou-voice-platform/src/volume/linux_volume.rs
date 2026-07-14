//! Linux 系统输出音量 duck：通过桌面音频工具读写默认 sink 音量。
//!
//! 语义与 Windows 一致：目标 = 当前音量 × factor（相对比例）。
//! 兼容链：
//! 1. PipeWire：`wpctl get/set-volume @DEFAULT_AUDIO_SINK@`
//! 2. PulseAudio：`pactl get/set-sink-volume @DEFAULT_SINK@`
//!
//! 外部命令期间不持锁；工具缺失或解析失败时返回带后端明细的错误。

use std::sync::Mutex;

use super::process::run_command;

/// 进入 duck 前的主音量 scalar（0.0–1.0）；`None` 表示当前未 duck。
static DUCKED_VOLUME: Mutex<Option<f32>> = Mutex::new(None);

/// 压低默认输出音量到 `current * factor`。
pub fn duck_output_volume(factor: f32) -> Result<bool, String> {
    if is_already_ducked()? {
        return Ok(false);
    }

    let current = read_output_volume_scalar()?;
    let target = (current * factor).clamp(0.0, 1.0);
    if (current - target).abs() > f32::EPSILON {
        set_output_volume_scalar(target).map_err(|error| {
            format!(
                "Could not lower Linux output volume from {:.0}% to {:.0}%: {error}",
                current * 100.0,
                target * 100.0
            )
        })?;
    }

    match commit_ducked_volume(current, || set_output_volume_scalar(current)) {
        Ok(true) => Ok(true),
        Ok(false) => Ok(false),
        Err(error) => Err(format!(
            "Lowered Linux output volume to {:.0}% but failed to remember the previous level ({:.0}%): {error}",
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

    let previous = previous.clamp(0.0, 1.0);
    if let Err(error) = set_output_volume_scalar(previous) {
        if let Err(store_error) = store_ducked_volume(previous) {
            return Err(format!(
                "Could not restore Linux output volume to {:.0}%: {error}; also lost the saved level: {store_error}",
                previous * 100.0
            ));
        }
        return Err(format!(
            "Could not restore Linux output volume to {:.0}%: {error}",
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

fn read_output_volume_scalar() -> Result<f32, String> {
    let wpctl_error = match read_wpctl_volume() {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    let pactl_error = match read_pactl_volume() {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    Err(format!(
        "Could not read default output volume. wpctl: {wpctl_error}; pactl: {pactl_error}. Install pipewire (wpctl) or pulseaudio-utils (pactl)."
    ))
}

fn set_output_volume_scalar(level: f32) -> Result<(), String> {
    let level = level.clamp(0.0, 1.0);
    let wpctl_error = match set_wpctl_volume(level) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    let pactl_error = match set_pactl_volume(level) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    Err(format!(
        "Could not set default output volume to {:.0}%. wpctl: {wpctl_error}; pactl: {pactl_error}. Install pipewire (wpctl) or pulseaudio-utils (pactl).",
        level * 100.0
    ))
}

/// `wpctl get-volume @DEFAULT_AUDIO_SINK@` → `Volume: 0.80` / `Volume: 0.80 [MUTED]`
fn read_wpctl_volume() -> Result<f32, String> {
    let text = run_command("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])?;
    parse_wpctl_volume(&text).ok_or_else(|| format!("could not parse wpctl volume from `{text}`"))
}

fn set_wpctl_volume(level: f32) -> Result<(), String> {
    // wpctl 接受 0.0–1.0 浮点。
    let arg = format!("{level:.4}");
    run_command("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", &arg]).map(|_| ())
}

/// `pactl get-sink-volume @DEFAULT_SINK@` 文本里取第一个 `NN%`。
fn read_pactl_volume() -> Result<f32, String> {
    let text = run_command("pactl", &["get-sink-volume", "@DEFAULT_SINK@"])?;
    parse_pactl_volume_percent(&text)
        .map(|percent| (percent as f32 / 100.0).clamp(0.0, 1.0))
        .ok_or_else(|| format!("could not parse pactl volume from `{text}`"))
}

fn set_pactl_volume(level: f32) -> Result<(), String> {
    let percent = (level.clamp(0.0, 1.0) * 100.0).round().clamp(0.0, 100.0) as u32;
    let arg = format!("{percent}%");
    run_command("pactl", &["set-sink-volume", "@DEFAULT_SINK@", &arg]).map(|_| ())
}

fn parse_wpctl_volume(text: &str) -> Option<f32> {
    // "Volume: 0.80" / "Volume: 0.80 [MUTED]"
    let trimmed = text.trim();
    let value_part = trimmed
        .strip_prefix("Volume:")
        .map(str::trim)
        .unwrap_or(trimmed);
    let token = value_part.split_whitespace().next()?;
    let value: f32 = token.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    Some(value.clamp(0.0, 1.0))
}

fn parse_pactl_volume_percent(text: &str) -> Option<u32> {
    // "... 52428 /  80% / -5.81 dB ..."
    for token in text.split_whitespace() {
        let Some(raw) = token.strip_suffix('%') else {
            continue;
        };
        if let Ok(value) = raw.parse::<f32>() {
            if value.is_finite() {
                return Some(value.round().clamp(0.0, 100.0) as u32);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{parse_pactl_volume_percent, parse_wpctl_volume};

    #[test]
    fn parses_wpctl_volume() {
        assert!((parse_wpctl_volume("Volume: 0.80\n").unwrap() - 0.80).abs() < f32::EPSILON);
        assert!(
            (parse_wpctl_volume("Volume: 0.40 [MUTED]\n").unwrap() - 0.40).abs() < f32::EPSILON
        );
    }

    #[test]
    fn parses_pactl_volume() {
        let sample =
            "Volume: front-left: 52428 /  80% / -5.81 dB,   front-right: 52428 /  80% / -5.81 dB";
        assert_eq!(parse_pactl_volume_percent(sample), Some(80));
    }
}
