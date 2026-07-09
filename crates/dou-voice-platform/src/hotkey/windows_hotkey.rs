use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_0, VK_1, VK_2, VK_3, VK_4, VK_5, VK_6, VK_7, VK_8, VK_9, VK_A, VK_B,
    VK_BACK, VK_C, VK_CONTROL, VK_D, VK_DELETE, VK_DOWN, VK_E, VK_END, VK_ESCAPE, VK_F, VK_F1,
    VK_F10, VK_F11, VK_F12, VK_F13, VK_F14, VK_F15, VK_F16, VK_F17, VK_F18, VK_F19, VK_F2, VK_F20,
    VK_F21, VK_F22, VK_F23, VK_F24, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_G, VK_H,
    VK_HOME, VK_I, VK_INSERT, VK_J, VK_K, VK_L, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN,
    VK_M, VK_MENU, VK_N, VK_NEXT, VK_O, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6,
    VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_P, VK_PRIOR, VK_Q, VK_R,
    VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_S, VK_SHIFT, VK_SPACE, VK_T,
    VK_TAB, VK_U, VK_UP, VK_V, VK_W, VK_X, VK_Y, VK_Z,
};

use super::{parse_hotkey, HotkeyKey, HotkeySpec};

pub fn hotkey_pressed(shortcut: &str) -> bool {
    parse_hotkey(shortcut).is_some_and(spec_pressed)
}

fn spec_pressed(spec: HotkeySpec) -> bool {
    modifiers_match(spec)
        && match spec.key {
            Some(key) => key_pressed(vkey_for_key(key)),
            None => true,
        }
}

fn modifiers_match(spec: HotkeySpec) -> bool {
    spec.ctrl == ctrl_pressed()
        && spec.alt == alt_pressed()
        && spec.shift == shift_pressed()
        && spec.win == win_pressed()
}

fn win_pressed() -> bool {
    key_pressed(VK_LWIN) || key_pressed(VK_RWIN)
}

fn alt_pressed() -> bool {
    key_pressed(VK_MENU) || key_pressed(VK_LMENU) || key_pressed(VK_RMENU)
}

fn ctrl_pressed() -> bool {
    key_pressed(VK_CONTROL) || key_pressed(VK_LCONTROL) || key_pressed(VK_RCONTROL)
}

fn shift_pressed() -> bool {
    key_pressed(VK_SHIFT) || key_pressed(VK_LSHIFT) || key_pressed(VK_RSHIFT)
}

fn vkey_for_key(key: HotkeyKey) -> VIRTUAL_KEY {
    match key {
        HotkeyKey::Backquote => VK_OEM_3,
        HotkeyKey::Backslash => VK_OEM_5,
        HotkeyKey::Backspace => VK_BACK,
        HotkeyKey::BracketLeft => VK_OEM_4,
        HotkeyKey::BracketRight => VK_OEM_6,
        HotkeyKey::Comma => VK_OEM_COMMA,
        HotkeyKey::Delete => VK_DELETE,
        HotkeyKey::End => VK_END,
        HotkeyKey::Equal => VK_OEM_PLUS,
        HotkeyKey::Space => VK_SPACE,
        HotkeyKey::Enter => VK_RETURN,
        HotkeyKey::Home => VK_HOME,
        HotkeyKey::Insert => VK_INSERT,
        HotkeyKey::Minus => VK_OEM_MINUS,
        HotkeyKey::PageDown => VK_NEXT,
        HotkeyKey::PageUp => VK_PRIOR,
        HotkeyKey::Period => VK_OEM_PERIOD,
        HotkeyKey::Quote => VK_OEM_7,
        HotkeyKey::Semicolon => VK_OEM_1,
        HotkeyKey::Slash => VK_OEM_2,
        HotkeyKey::Tab => VK_TAB,
        HotkeyKey::Escape => VK_ESCAPE,
        HotkeyKey::ArrowDown => VK_DOWN,
        HotkeyKey::ArrowLeft => VK_LEFT,
        HotkeyKey::ArrowRight => VK_RIGHT,
        HotkeyKey::ArrowUp => VK_UP,
        HotkeyKey::Letter('A') => VK_A,
        HotkeyKey::Letter('B') => VK_B,
        HotkeyKey::Letter('C') => VK_C,
        HotkeyKey::Letter('D') => VK_D,
        HotkeyKey::Letter('E') => VK_E,
        HotkeyKey::Letter('F') => VK_F,
        HotkeyKey::Letter('G') => VK_G,
        HotkeyKey::Letter('H') => VK_H,
        HotkeyKey::Letter('I') => VK_I,
        HotkeyKey::Letter('J') => VK_J,
        HotkeyKey::Letter('K') => VK_K,
        HotkeyKey::Letter('L') => VK_L,
        HotkeyKey::Letter('M') => VK_M,
        HotkeyKey::Letter('N') => VK_N,
        HotkeyKey::Letter('O') => VK_O,
        HotkeyKey::Letter('P') => VK_P,
        HotkeyKey::Letter('Q') => VK_Q,
        HotkeyKey::Letter('R') => VK_R,
        HotkeyKey::Letter('S') => VK_S,
        HotkeyKey::Letter('T') => VK_T,
        HotkeyKey::Letter('U') => VK_U,
        HotkeyKey::Letter('V') => VK_V,
        HotkeyKey::Letter('W') => VK_W,
        HotkeyKey::Letter('X') => VK_X,
        HotkeyKey::Letter('Y') => VK_Y,
        HotkeyKey::Letter('Z') => VK_Z,
        HotkeyKey::Letter(_) => unreachable!("unsupported letter key"),
        HotkeyKey::Digit('0') => VK_0,
        HotkeyKey::Digit('1') => VK_1,
        HotkeyKey::Digit('2') => VK_2,
        HotkeyKey::Digit('3') => VK_3,
        HotkeyKey::Digit('4') => VK_4,
        HotkeyKey::Digit('5') => VK_5,
        HotkeyKey::Digit('6') => VK_6,
        HotkeyKey::Digit('7') => VK_7,
        HotkeyKey::Digit('8') => VK_8,
        HotkeyKey::Digit('9') => VK_9,
        HotkeyKey::Digit(_) => unreachable!("unsupported digit key"),
        HotkeyKey::Function(1) => VK_F1,
        HotkeyKey::Function(2) => VK_F2,
        HotkeyKey::Function(3) => VK_F3,
        HotkeyKey::Function(4) => VK_F4,
        HotkeyKey::Function(5) => VK_F5,
        HotkeyKey::Function(6) => VK_F6,
        HotkeyKey::Function(7) => VK_F7,
        HotkeyKey::Function(8) => VK_F8,
        HotkeyKey::Function(9) => VK_F9,
        HotkeyKey::Function(10) => VK_F10,
        HotkeyKey::Function(11) => VK_F11,
        HotkeyKey::Function(12) => VK_F12,
        HotkeyKey::Function(13) => VK_F13,
        HotkeyKey::Function(14) => VK_F14,
        HotkeyKey::Function(15) => VK_F15,
        HotkeyKey::Function(16) => VK_F16,
        HotkeyKey::Function(17) => VK_F17,
        HotkeyKey::Function(18) => VK_F18,
        HotkeyKey::Function(19) => VK_F19,
        HotkeyKey::Function(20) => VK_F20,
        HotkeyKey::Function(21) => VK_F21,
        HotkeyKey::Function(22) => VK_F22,
        HotkeyKey::Function(23) => VK_F23,
        HotkeyKey::Function(24) => VK_F24,
        HotkeyKey::Function(_) => unreachable!("unsupported function key"),
    }
}

fn key_pressed(vkey: VIRTUAL_KEY) -> bool {
    unsafe {
        // SAFETY: GetAsyncKeyState accepts virtual-key codes. We only pass Win32 VK_* constants.
        GetAsyncKeyState(i32::from(vkey.0)) < 0
    }
}
