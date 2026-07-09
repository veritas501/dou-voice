//! 文本输入适配层。
//!
//! 设计目标：语音输入完成后，把识别文本送进当前焦点窗口。输入链路分为两层：
//!
//! 1. **enigo 主路径**（默认）：调用 `enigo.text(text)`。Windows 上等价于
//!    `SendInput(KEYEVENTF_UNICODE)`，跨平台；macOS 走 CGEvent，Linux 走 XTest/libei。
//! 2. **剪贴板 fallback**：备份原剪贴板 → 写入文本 → 模拟 Ctrl+V（用扫描码
//!    `Key::Other(0x56)` 绕过键盘布局）→ 还原原剪贴板。
//!
//! 主路径前会做残留修饰键清理（等待热键修饰键释放 + 补发 key-up + 发 ESC 清菜单激活
//! 态），避免 hotkey 残留导致第一个字符被解释成菜单快捷键。

#[cfg(windows)]
mod windows_input;

#[cfg(not(windows))]
mod unsupported;

#[cfg(not(windows))]
use unsupported as platform;
#[cfg(windows)]
use windows_input as platform;

/// 文本输入实际使用的方法。
///
/// - `Direct`：`enigo.text()` 主路径，无快捷键、无剪贴板副作用
/// - `Clipboard`：剪贴板 + 模拟 Ctrl+V fallback；用户无需手动粘贴
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextInputMethod {
    Direct,
    Clipboard,
}

/// 文本输入结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextInputOutcome {
    pub method: TextInputMethod,
    /// 前序方法失败时的错误信息（用于诊断）。
    pub prior_error: Option<String>,
}

/// 把识别文本输入到当前焦点窗口。
///
/// 默认走 enigo 主路径；失败时回退到剪贴板 + 模拟 Ctrl+V。两种方法都会自动完成输入，
/// 不需要用户手动粘贴。
pub fn type_text(text: &str) -> Result<TextInputOutcome, String> {
    platform::type_text(text)
}

/// 仅将文本写入剪贴板，不发送任何快捷键。
///
/// 供"Clipboard paste"设置项使用：始终只写剪贴板，调用方应告知用户文本已就绪、需要
/// 手动粘贴。
pub fn copy_text_to_clipboard(text: &str) -> Result<TextInputOutcome, String> {
    platform::copy_text_to_clipboard(text)
}
