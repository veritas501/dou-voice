//! 前台焦点窗口位置查询。
//!
//! overlay 需要显示在“用户正在操作的显示器”上，而不是 overlay 自身所在屏或主屏。
//! 最可靠的信号是系统前台窗口中心；拿不到时由上层再回退到光标位置。

#[cfg(windows)]
mod windows_focus;

#[cfg(not(windows))]
mod unsupported_focus;

#[cfg(not(windows))]
use unsupported_focus as platform;
#[cfg(windows)]
use windows_focus as platform;

/// 当前前台窗口客户区/外框的中心点（屏幕物理像素坐标）。
///
/// - 前台窗口无效、不可见或最小化时返回 `None`。
/// - 非 Windows 当前返回 `None`，上层应改用光标位置。
pub fn foreground_window_center() -> Option<(i32, i32)> {
    platform::foreground_window_center()
}
