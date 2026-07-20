#[cfg(windows)]
mod windows_hotkey;
#[cfg(windows)]
mod windows_hotkey_hook;

#[cfg(not(windows))]
mod unsupported_hotkey;

#[cfg(not(windows))]
use unsupported_hotkey as platform;
#[cfg(windows)]
use windows_hotkey as platform;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HotkeySpec {
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
    pub(crate) win: bool,
    pub(crate) key: Option<HotkeyKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HotkeyKey {
    Backquote,
    Backslash,
    Backspace,
    BracketLeft,
    BracketRight,
    Comma,
    Delete,
    End,
    Equal,
    Space,
    Enter,
    Home,
    Insert,
    Minus,
    PageDown,
    PageUp,
    Period,
    Quote,
    Semicolon,
    Slash,
    Tab,
    Escape,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Letter(char),
    Digit(char),
    Function(u8),
}

/// 判断当前是否按住 Windows press-to-talk 热键。
///
/// 部分组合是 modifier-only，Tauri/global-hotkey 的字符串注册路径不支持，因此由平台层
/// 提供安全封装给桌面端轮询使用。未知组合返回 false。
pub fn hotkey_pressed(shortcut: &str) -> bool {
    platform::hotkey_pressed(shortcut)
}

/// 启动 Windows 低级键盘钩子，用于吞掉 press-to-talk 主键（防前台透传）。
///
/// 非 Windows 平台为 no-op。钩子安装失败时返回错误；调用方可选择仅记录日志并继续
/// 使用轮询热键（fail-open）。
pub fn start_hotkey_key_swallow() -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_hotkey_hook::start_hotkey_key_swallow()
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

/// 停止 Windows 低级键盘钩子。非 Windows 为 no-op。
pub fn stop_hotkey_key_swallow() {
    #[cfg(windows)]
    {
        windows_hotkey_hook::stop_hotkey_key_swallow();
    }
}

/// 更新需要吞主键的热键。modifier-only 或非法字符串会清空目标（不吞任何键）。
pub fn set_swallowed_hotkey(shortcut: Option<&str>) {
    #[cfg(windows)]
    {
        windows_hotkey_hook::set_swallowed_hotkey(shortcut);
    }
    #[cfg(not(windows))]
    {
        let _ = shortcut;
    }
}

/// 临时开关吞键（设置页捕获热键时应关闭）。非 Windows 为 no-op。
pub fn set_hotkey_swallow_enabled(enabled: bool) {
    #[cfg(windows)]
    {
        windows_hotkey_hook::set_hotkey_swallow_enabled(enabled);
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
    }
}

/// 判断热键字符串是否属于当前支持的格式。
///
/// 支持至少一个修饰键加一个普通键，或两个及以上 modifier-only 组合。
pub fn is_supported_hotkey(shortcut: &str) -> bool {
    parse_hotkey(shortcut).is_some()
}

/// 判断热键字符串是否属于当前平台实际可用的格式。
///
/// Windows 使用平台轮询，macOS 使用原生事件监听，二者允许两个及以上修饰键组成的
/// press-to-talk 热键。Linux 当前走 Tauri global-shortcut 插件，只接受至少一个修饰键
/// 加一个主键的组合。
pub fn is_supported_hotkey_for_current_platform(shortcut: &str) -> bool {
    normalize_hotkey_for_current_platform(shortcut).is_some()
}

/// 将设置页热键字符串归一化为当前平台注册路径可用的表示。
pub fn normalize_hotkey_for_current_platform(shortcut: &str) -> Option<String> {
    let spec = parse_hotkey(shortcut)?;
    if cfg!(all(not(windows), not(target_os = "macos"))) && spec.key.is_none() {
        return None;
    }
    Some(format_hotkey_spec_for_current_platform(spec))
}

pub(crate) fn parse_hotkey(shortcut: &str) -> Option<HotkeySpec> {
    let mut spec = HotkeySpec {
        ctrl: false,
        alt: false,
        shift: false,
        win: false,
        key: None,
    };

    for raw in shortcut.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            return None;
        }
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => set_modifier(&mut spec.ctrl)?,
            "alt" | "option" => set_modifier(&mut spec.alt)?,
            "shift" => set_modifier(&mut spec.shift)?,
            "win" | "super" | "meta" | "cmd" | "command" => set_modifier(&mut spec.win)?,
            _ => {
                if spec.key.is_some() {
                    return None;
                }
                spec.key = parse_key(token);
                spec.key?;
            }
        }
    }

    let modifier_count = usize::from(spec.ctrl)
        + usize::from(spec.alt)
        + usize::from(spec.shift)
        + usize::from(spec.win);
    match (modifier_count, spec.key) {
        (0, _) => None,
        (1, None) => None,
        (_, Some(_)) | (2.., None) => Some(spec),
    }
}

fn format_hotkey_spec_for_current_platform(spec: HotkeySpec) -> String {
    let mut parts = Vec::new();
    if spec.ctrl {
        parts.push("Ctrl".to_string());
    }
    if spec.alt {
        parts.push("Alt".to_string());
    }
    if spec.shift {
        parts.push("Shift".to_string());
    }
    if spec.win {
        parts.push(meta_modifier_label().to_string());
    }
    if let Some(key) = spec.key {
        parts.push(format_hotkey_key(key));
    }
    parts.join("+")
}

#[cfg(target_os = "macos")]
fn meta_modifier_label() -> &'static str {
    "Command"
}

#[cfg(windows)]
fn meta_modifier_label() -> &'static str {
    "Win"
}

#[cfg(all(not(target_os = "macos"), not(windows)))]
fn meta_modifier_label() -> &'static str {
    "Super"
}

fn format_hotkey_key(key: HotkeyKey) -> String {
    match key {
        HotkeyKey::Backquote => "Backquote".to_string(),
        HotkeyKey::Backslash => "Backslash".to_string(),
        HotkeyKey::Backspace => "Backspace".to_string(),
        HotkeyKey::BracketLeft => "BracketLeft".to_string(),
        HotkeyKey::BracketRight => "BracketRight".to_string(),
        HotkeyKey::Comma => "Comma".to_string(),
        HotkeyKey::Delete => "Delete".to_string(),
        HotkeyKey::End => "End".to_string(),
        HotkeyKey::Equal => "Equal".to_string(),
        HotkeyKey::Space => "Space".to_string(),
        HotkeyKey::Enter => "Enter".to_string(),
        HotkeyKey::Home => "Home".to_string(),
        HotkeyKey::Insert => "Insert".to_string(),
        HotkeyKey::Minus => "Minus".to_string(),
        HotkeyKey::PageDown => "PageDown".to_string(),
        HotkeyKey::PageUp => "PageUp".to_string(),
        HotkeyKey::Period => "Period".to_string(),
        HotkeyKey::Quote => "Quote".to_string(),
        HotkeyKey::Semicolon => "Semicolon".to_string(),
        HotkeyKey::Slash => "Slash".to_string(),
        HotkeyKey::Tab => "Tab".to_string(),
        HotkeyKey::Escape => "Escape".to_string(),
        HotkeyKey::ArrowDown => "ArrowDown".to_string(),
        HotkeyKey::ArrowLeft => "ArrowLeft".to_string(),
        HotkeyKey::ArrowRight => "ArrowRight".to_string(),
        HotkeyKey::ArrowUp => "ArrowUp".to_string(),
        HotkeyKey::Letter(ch) | HotkeyKey::Digit(ch) => ch.to_string(),
        HotkeyKey::Function(number) => format!("F{number}"),
    }
}

fn set_modifier(enabled: &mut bool) -> Option<()> {
    if *enabled {
        return None;
    }
    *enabled = true;
    Some(())
}

fn parse_key(token: &str) -> Option<HotkeyKey> {
    let normalized = token.trim();
    if normalized.len() == 1 {
        let ch = normalized.chars().next()?;
        if ch.is_ascii_alphabetic() {
            return Some(HotkeyKey::Letter(ch.to_ascii_uppercase()));
        }
        if ch.is_ascii_digit() {
            return Some(HotkeyKey::Digit(ch));
        }
    }

    match normalized.to_ascii_lowercase().as_str() {
        "`" | "backquote" => Some(HotkeyKey::Backquote),
        "\\" | "backslash" => Some(HotkeyKey::Backslash),
        "backspace" => Some(HotkeyKey::Backspace),
        "[" | "bracketleft" => Some(HotkeyKey::BracketLeft),
        "]" | "bracketright" => Some(HotkeyKey::BracketRight),
        "," | "comma" => Some(HotkeyKey::Comma),
        "delete" => Some(HotkeyKey::Delete),
        "end" => Some(HotkeyKey::End),
        "=" | "equal" => Some(HotkeyKey::Equal),
        "space" => Some(HotkeyKey::Space),
        "enter" | "return" => Some(HotkeyKey::Enter),
        "home" => Some(HotkeyKey::Home),
        "insert" => Some(HotkeyKey::Insert),
        "-" | "minus" => Some(HotkeyKey::Minus),
        "pagedown" | "page_down" => Some(HotkeyKey::PageDown),
        "pageup" | "page_up" => Some(HotkeyKey::PageUp),
        "." | "period" => Some(HotkeyKey::Period),
        "'" | "quote" => Some(HotkeyKey::Quote),
        ";" | "semicolon" => Some(HotkeyKey::Semicolon),
        "/" | "slash" => Some(HotkeyKey::Slash),
        "tab" => Some(HotkeyKey::Tab),
        "esc" | "escape" => Some(HotkeyKey::Escape),
        "arrowdown" | "down" => Some(HotkeyKey::ArrowDown),
        "arrowleft" | "left" => Some(HotkeyKey::ArrowLeft),
        "arrowright" | "right" => Some(HotkeyKey::ArrowRight),
        "arrowup" | "up" => Some(HotkeyKey::ArrowUp),
        key if key.len() >= 2 && key.starts_with('f') => {
            let number = key[1..].parse::<u8>().ok()?;
            if (1..=24).contains(&number) {
                Some(HotkeyKey::Function(number))
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_supported_hotkey, is_supported_hotkey_for_current_platform,
        normalize_hotkey_for_current_platform, parse_hotkey, HotkeyKey,
    };

    #[test]
    fn accepts_modifier_only_combo_with_two_modifiers() {
        let spec = parse_hotkey("Win+Alt").expect("hotkey should parse");
        assert!(spec.win);
        assert!(spec.alt);
        assert_eq!(spec.key, None);
    }

    #[test]
    fn accepts_modifier_and_main_key_combo() {
        let spec = parse_hotkey("Ctrl+Alt+Space").expect("hotkey should parse");
        assert!(spec.ctrl);
        assert!(spec.alt);
        assert_eq!(spec.key, Some(HotkeyKey::Space));
    }

    #[test]
    fn rejects_single_modifier_or_single_key() {
        assert!(!is_supported_hotkey("Ctrl"));
        assert!(!is_supported_hotkey("Space"));
    }

    #[test]
    fn rejects_duplicate_modifiers() {
        assert!(!is_supported_hotkey("Ctrl+Ctrl+Space"));
        assert!(!is_supported_hotkey("Win+Meta"));
    }

    #[test]
    fn accepts_whitespace_and_case_variants() {
        let spec = parse_hotkey(" control + option + f12 ").expect("hotkey should parse");
        assert!(spec.ctrl);
        assert!(spec.alt);
        assert_eq!(spec.key, Some(HotkeyKey::Function(12)));
    }

    #[cfg(windows)]
    #[test]
    fn current_platform_accepts_modifier_only_combo_on_windows() {
        assert!(is_supported_hotkey_for_current_platform("Win+Alt"));
    }

    #[cfg(all(not(windows), not(target_os = "macos")))]
    #[test]
    fn current_platform_rejects_modifier_only_combo_without_native_support() {
        assert!(!is_supported_hotkey_for_current_platform("Win+Alt"));
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn current_platform_accepts_modifier_only_combo_with_native_support() {
        assert!(is_supported_hotkey_for_current_platform("Win+Alt"));
    }

    #[test]
    fn normalizes_supported_hotkey_label() {
        assert_eq!(
            normalize_hotkey_for_current_platform(" control + option + f12 ").as_deref(),
            Some("Ctrl+Alt+F12")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn normalizes_meta_modifier_for_macos() {
        assert_eq!(
            normalize_hotkey_for_current_platform("Win+Q").as_deref(),
            Some("Command+Q")
        );
    }

    #[cfg(windows)]
    #[test]
    fn normalizes_meta_modifier_for_windows() {
        assert_eq!(
            normalize_hotkey_for_current_platform("Command+Q").as_deref(),
            Some("Win+Q")
        );
    }

    #[cfg(all(not(target_os = "macos"), not(windows)))]
    #[test]
    fn normalizes_meta_modifier_for_linux_global_shortcut() {
        assert_eq!(
            normalize_hotkey_for_current_platform("Win+Q").as_deref(),
            Some("Super+Q")
        );
    }
}
