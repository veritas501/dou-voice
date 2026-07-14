//! 非 Windows 平台的前台窗口查询 stub。
//!
//! 当前返回 `None`，由上层回退到 Tauri 的光标位置 / current_monitor。
//! macOS/Linux 后续可分别接入 NSWorkspace 与 X11/Wayland 焦点查询。

/// 非 Windows：暂无前台窗口几何。
pub fn foreground_window_center() -> Option<(i32, i32)> {
    None
}
