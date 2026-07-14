//! 未实现系统音量 duck 的平台 stub。
//!
//! 当前返回成功但 no-op，避免阻塞语音输入主路径。

/// 不修改系统音量。
pub fn duck_output_volume(_factor: f32) -> Result<bool, String> {
    Ok(false)
}

/// 无待恢复状态。
pub fn restore_output_volume() -> Result<bool, String> {
    Ok(false)
}
