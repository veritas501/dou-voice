use std::thread::sleep;
use std::time::Duration;

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

use super::{TextInputMethod, TextInputOutcome};

const POST_CLIPBOARD_WRITE_DELAY: Duration = Duration::from_millis(60);
const POST_PASTE_DELAY: Duration = Duration::from_millis(50);

/// macOS/Linux 文本输入实现。
///
/// 与 Windows 保持同样的两层策略：优先 `enigo.text()` 直接输入，失败后回退到
/// 剪贴板 + 粘贴快捷键。更细的 Linux Wayland 原生工具链（wtype/dotool/ydotool）
/// 后续可按桌面环境继续补充。
pub fn type_text(text: &str) -> Result<TextInputOutcome, String> {
    if text.is_empty() {
        return Ok(TextInputOutcome {
            method: TextInputMethod::Direct,
            prior_error: None,
        });
    }

    match enigo_type_text(text) {
        Ok(()) => Ok(TextInputOutcome {
            method: TextInputMethod::Direct,
            prior_error: None,
        }),
        Err(direct_error) => {
            paste_via_clipboard(text)?;
            Ok(TextInputOutcome {
                method: TextInputMethod::Clipboard,
                prior_error: Some(direct_error),
            })
        }
    }
}

pub fn copy_text_to_clipboard(text: &str) -> Result<TextInputOutcome, String> {
    let mut clipboard =
        Clipboard::new().map_err(|error| format!("failed to open clipboard: {error}"))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|error| format!("failed to set clipboard text: {error}"))?;
    Ok(TextInputOutcome {
        method: TextInputMethod::Clipboard,
        prior_error: None,
    })
}

fn enigo_type_text(text: &str) -> Result<(), String> {
    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|error| format!("enigo init failed: {error}"))?;
    enigo
        .text(text)
        .map_err(|error| format!("enigo.text failed: {error}"))
}

fn paste_via_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard =
        Clipboard::new().map_err(|error| format!("failed to open clipboard: {error}"))?;
    let original = clipboard.get_text().ok();
    clipboard
        .set_text(text.to_string())
        .map_err(|error| format!("failed to set clipboard text: {error}"))?;

    sleep(POST_CLIPBOARD_WRITE_DELAY);

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|error| format!("enigo init failed for paste: {error}"))?;
    send_paste_shortcut(&mut enigo)?;

    sleep(POST_PASTE_DELAY);

    if let Some(original) = original {
        let _ = clipboard.set_text(original);
    }

    Ok(())
}

fn send_paste_shortcut(enigo: &mut Enigo) -> Result<(), String> {
    let modifier = paste_modifier_key();
    let paste_key = paste_key();
    enigo
        .key(modifier, Direction::Press)
        .map_err(|error| format!("failed to press paste modifier: {error}"))?;
    enigo
        .key(paste_key, Direction::Click)
        .map_err(|error| format!("failed to click paste key: {error}"))?;
    enigo
        .key(modifier, Direction::Release)
        .map_err(|error| format!("failed to release paste modifier: {error}"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn paste_modifier_key() -> Key {
    Key::Meta
}

#[cfg(not(target_os = "macos"))]
fn paste_modifier_key() -> Key {
    Key::Control
}

#[cfg(target_os = "macos")]
fn paste_key() -> Key {
    Key::Other(9)
}

#[cfg(not(target_os = "macos"))]
fn paste_key() -> Key {
    Key::Unicode('v')
}
