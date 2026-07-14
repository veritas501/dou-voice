//! Windows 前台窗口几何查询。
//!
//! 使用 `GetForegroundWindow` + `GetWindowRect` 取外框中心。
//! 跳过无效、不可见、最小化窗口，避免把 overlay 钉在主屏或托盘附近。

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, IsIconic, IsWindowVisible,
};

/// 返回前台窗口外框中心点（物理像素）。
pub fn foreground_window_center() -> Option<(i32, i32)> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() || hwnd == HWND::default() {
        return None;
    }
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return None;
    }
    if unsafe { IsIconic(hwnd) }.as_bool() {
        return None;
    }

    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;

    let width = rect.right.saturating_sub(rect.left);
    let height = rect.bottom.saturating_sub(rect.top);
    // 零面积窗口（隐藏壳、某些 shell 占位）没有有效几何，放弃。
    if width <= 0 || height <= 0 {
        return None;
    }

    let center = POINT {
        x: rect.left + width / 2,
        y: rect.top + height / 2,
    };
    Some((center.x, center.y))
}
