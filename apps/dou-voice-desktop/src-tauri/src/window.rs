use tauri::{AppHandle, Manager, Wry};

use crate::app_state::{LOGIN_LABEL, MAIN_LABEL, OVERLAY_LABEL};

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
