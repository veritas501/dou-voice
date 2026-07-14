//! macOS 系统输出音量 duck：通过 `osascript` 读写默认输出音量。
//!
//! 语义与 Windows 一致：目标 = 当前音量 × factor（相对比例，不是绝对 10%）。
//! AppleScript 的 `output volume` 是 0–100 整数；量化到 0 可接受。
//!
//! 外部命令期间不持锁，避免卡住其它路径；若写音量成功但无法记录恢复值，会立刻回滚。

use std::sync::Mutex;

use super::process::run_command;

/// 进入 duck 前的主音量 scalar（0.0–1.0）；`None` 表示当前未 duck。
static DUCKED_VOLUME: Mutex<Option<f32>> = Mutex::new(None);

/// 压低默认输出音量到 `current * factor`。
pub fn duck_output_volume(factor: f32) -> Result<bool, String> {
    if is_already_ducked()? {
        return Ok(false);
    }

    let current_percent = read_output_volume_percent()?;
    let current = (current_percent as f32 / 100.0).clamp(0.0, 1.0);
    let target_percent = ((current_percent as f32) * factor)
        .round()
        .clamp(0.0, 100.0) as u32;

    if target_percent != current_percent {
        set_output_volume_percent(target_percent).map_err(|error| {
            format!(
                "Could not lower macOS output volume from {current_percent}% to {target_percent}%: {error}"
            )
        })?;
    }

    match commit_ducked_volume(current, || set_output_volume_percent(current_percent)) {
        Ok(true) => Ok(true),
        Ok(false) => Ok(false),
        Err(error) => Err(format!(
            "Lowered macOS output volume to {target_percent}% but failed to remember the previous level ({current_percent}%): {error}"
        )),
    }
}

/// 写回进入 duck 前保存的主音量。
pub fn restore_output_volume() -> Result<bool, String> {
    let previous = take_ducked_volume()?;
    let Some(previous) = previous else {
        return Ok(false);
    };

    let previous_percent =
        (previous.clamp(0.0, 1.0) * 100.0).round().clamp(0.0, 100.0) as u32;
    if let Err(error) = set_output_volume_percent(previous_percent) {
        // 恢复失败：把原值塞回，下次 finish/quit 还能再试。
        if let Err(store_error) = store_ducked_volume(previous) {
            return Err(format!(
                "Could not restore macOS output volume to {previous_percent}%: {error}; also lost the saved level: {store_error}"
            ));
        }
        return Err(format!(
            "Could not restore macOS output volume to {previous_percent}%: {error}"
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

/// 读取系统输出音量（0–100）。
fn read_output_volume_percent() -> Result<u32, String> {
    let text = run_command(
        "osascript",
        &["-e", "output volume of (get volume settings)"],
    )
    .map_err(|error| format!("Could not read macOS output volume: {error}"))?;
    parse_percent(&text).ok_or_else(|| {
        format!("Could not parse macOS output volume (expected 0-100 integer, got `{text}`)")
    })
}

/// 设置系统输出音量（0–100）。
fn set_output_volume_percent(percent: u32) -> Result<(), String> {
    let percent = percent.min(100);
    let script = format!("set volume output volume {percent}");
    run_command("osascript", &["-e", &script]).map(|_| ())
}

fn parse_percent(text: &str) -> Option<u32> {
    let value: f32 = text.trim().parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    Some(value.round().clamp(0.0, 100.0) as u32)
}

#[cfg(test)]
mod tests {
    use super::parse_percent;

    #[test]
    fn parses_integer_volume() {
        assert_eq!(parse_percent("80"), Some(80));
        assert_eq!(parse_percent(" 0\n"), Some(0));
        assert_eq!(parse_percent("100"), Some(100));
    }
}
