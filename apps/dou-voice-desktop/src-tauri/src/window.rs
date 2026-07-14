use serde::Serialize;
use tauri::{AppHandle, LogicalSize, Manager, Size, Wry};

use crate::app_state::{LOGIN_LABEL, MAIN_LABEL, OVERLAY_LABEL};

/// 主窗口默认逻辑宽度（与 `tauri.conf.json` 对齐）。
pub(crate) const MAIN_WINDOW_WIDTH: f64 = 900.0;
/// 主窗口最小逻辑高度，避免设置项较少时窗口塌缩得过矮。
pub(crate) const MAIN_WINDOW_MIN_HEIGHT: f64 = 360.0;
/// 主窗口相对工作区高度的最大占比，避免贴满屏幕。
pub(crate) const MAIN_WINDOW_MAX_WORK_AREA_RATIO: f64 = 0.92;
/// 宽高变化小于该逻辑像素时跳过 resize，抑制测量抖动。
const FIT_SIZE_EPSILON: f64 = 1.0;

/// 前端 `fit_main_window` 调用后的实际窗口尺寸（逻辑像素）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FitMainWindowResult {
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) clamped: bool,
}

/// 显示并聚焦主窗口。
pub(crate) fn show_main_window(app: &AppHandle<Wry>) {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// 判断窗口关闭时是否应隐藏到托盘。
///
/// 主窗口和登录窗口都隐藏而不销毁，可避免 Windows WebView 关闭时出现多余资源释放噪声。
pub(crate) fn should_hide_on_close(label: &str) -> bool {
    matches!(label, MAIN_LABEL | LOGIN_LABEL | OVERLAY_LABEL)
}

/// 读取主窗口所在显示器的工作区物理尺寸（宽、高）。
fn main_work_area_physical_size(window: &tauri::WebviewWindow<Wry>) -> Option<(u32, u32)> {
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())?;
    let work_area = monitor.work_area();
    Some((work_area.size.width, work_area.size.height))
}

/// 按前端测量的内容高度调整主窗口，并夹到当前显示器工作区内。
///
/// 前端负责测量“自然内容高度”；Rust 负责 DPI / 工作区边界和真正的 `set_size`。
/// 高度触顶时返回 `clamped=true`，前端可退回局部滚动（例如 Diagnostics 日志）。
#[tauri::command]
pub(crate) fn fit_main_window(
    app: AppHandle<Wry>,
    content_width: Option<f64>,
    content_height: f64,
) -> Result<FitMainWindowResult, String> {
    let window = app
        .get_webview_window(MAIN_LABEL)
        .ok_or_else(|| "Main window is not available.".to_string())?;

    if !content_height.is_finite() || content_height <= 0.0 {
        return Err("Content height must be a positive finite number.".to_string());
    }

    let requested_width = content_width
        .filter(|width| width.is_finite() && *width > 0.0)
        .unwrap_or(MAIN_WINDOW_WIDTH)
        .clamp(320.0, MAIN_WINDOW_WIDTH);

    let scale_factor = window.scale_factor().unwrap_or(1.0).max(0.1);
    let work_area_size = main_work_area_physical_size(&window);

    let max_width = work_area_size
        .map(|(width, _)| (f64::from(width) / scale_factor).floor())
        .filter(|width| width.is_finite() && *width > 0.0)
        .unwrap_or(MAIN_WINDOW_WIDTH);
    let max_height = work_area_size
        .map(|(_, height)| {
            ((f64::from(height) / scale_factor) * MAIN_WINDOW_MAX_WORK_AREA_RATIO).floor()
        })
        .filter(|height| height.is_finite() && *height > 0.0)
        .unwrap_or(content_height.max(MAIN_WINDOW_MIN_HEIGHT));

    let width = requested_width.min(max_width).max(320.0);
    let height = content_height
        .max(MAIN_WINDOW_MIN_HEIGHT)
        .min(max_height.max(MAIN_WINDOW_MIN_HEIGHT));
    let clamped =
        content_height > height + FIT_SIZE_EPSILON || requested_width > width + FIT_SIZE_EPSILON;

    // 尺寸几乎不变时跳过 set_size，避免 WebView 重布局抖动。
    if let Ok(current) = window.inner_size() {
        let current_width = f64::from(current.width) / scale_factor;
        let current_height = f64::from(current.height) / scale_factor;
        if (current_width - width).abs() < FIT_SIZE_EPSILON
            && (current_height - height).abs() < FIT_SIZE_EPSILON
        {
            return Ok(FitMainWindowResult {
                width: current_width,
                height: current_height,
                clamped,
            });
        }
    }

    window
        .set_size(Size::Logical(LogicalSize::new(width, height)))
        .map_err(|error| format!("Failed to resize main window: {error}"))?;

    Ok(FitMainWindowResult {
        width,
        height,
        clamped,
    })
}
