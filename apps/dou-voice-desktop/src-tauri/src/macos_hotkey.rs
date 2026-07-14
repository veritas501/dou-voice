use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, CFRunLoopSource};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventTapCallBack, CGEventTapLocation,
    CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Wry};

use crate::hotkey::{current_hotkey_or_default, trigger_hotkey_pressed, trigger_hotkey_released};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeHotkeyCaptureEvent {
    hotkey: String,
    is_key_down: bool,
}

struct MacHotkeyState {
    app: AppHandle<Wry>,
    pressed: bool,
    capture_last_hotkey: Option<String>,
}

pub(crate) fn spawn_macos_hotkey_listener(app: AppHandle<Wry>) -> Result<(), String> {
    if !accessibility_trusted() {
        return Err("macOS accessibility permission is required for hotkey capture".to_string());
    }

    let state = Arc::new(Mutex::new(MacHotkeyState {
        app,
        pressed: false,
        capture_last_hotkey: None,
    }));
    let running = Arc::new(AtomicBool::new(true));
    let (init_tx, init_rx) = std::sync::mpsc::channel::<Result<(), String>>();

    thread::spawn(move || {
        run_event_tap(state, running, init_tx);
    });

    init_rx
        .recv()
        .map_err(|_| "macOS hotkey listener terminated during startup".to_string())?
}

fn run_event_tap(
    state: Arc<Mutex<MacHotkeyState>>,
    running: Arc<AtomicBool>,
    init_tx: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let event_mask: CGEventMask = (1 << CGEventType::KeyDown.0)
        | (1 << CGEventType::KeyUp.0)
        | (1 << CGEventType::FlagsChanged.0)
        | (1 << CGEventType::TapDisabledByTimeout.0)
        | (1 << CGEventType::TapDisabledByUserInput.0);
    let state_ptr = Arc::into_raw(Arc::clone(&state)) as *mut c_void;
    let callback: CGEventTapCallBack = Some(event_tap_callback);

    let tap: Option<CFRetained<CFMachPort>> = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::SessionEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            event_mask,
            callback,
            state_ptr,
        )
    };
    let Some(tap) = tap else {
        unsafe {
            let _ = Arc::from_raw(state_ptr as *const Mutex<MacHotkeyState>);
        }
        let _ = init_tx.send(Err(
            "Could not create macOS event tap for hotkey listener (Accessibility permission may be missing)".to_string()
        ));
        return;
    };

    let source: Option<CFRetained<CFRunLoopSource>> =
        CFMachPort::new_run_loop_source(None, Some(&tap), 0);
    let Some(source) = source else {
        unsafe {
            CFMachPort::invalidate(&tap);
            let _ = Arc::from_raw(state_ptr as *const Mutex<MacHotkeyState>);
        }
        let _ = init_tx.send(Err(
            "Could not create macOS hotkey run loop source".to_string()
        ));
        return;
    };

    let Some(run_loop) = CFRunLoop::current() else {
        unsafe {
            CFMachPort::invalidate(&tap);
            let _ = Arc::from_raw(state_ptr as *const Mutex<MacHotkeyState>);
        }
        let _ = init_tx.send(Err("Could not get the current macOS hotkey run loop".to_string()));
        return;
    };

    run_loop.add_source(Some(&source), unsafe {
        objc2_core_foundation::kCFRunLoopCommonModes
    });
    CGEvent::tap_enable(&tap, true);
    let _ = init_tx.send(Ok(()));

    while running.load(Ordering::SeqCst) {
        CFRunLoop::run_in_mode(
            unsafe { objc2_core_foundation::kCFRunLoopDefaultMode },
            0.1,
            true,
        );
        if !CGEvent::tap_is_enabled(&tap) {
            CGEvent::tap_enable(&tap, true);
        }
    }

    run_loop.remove_source(Some(&source), unsafe {
        objc2_core_foundation::kCFRunLoopCommonModes
    });
    CGEvent::tap_enable(&tap, false);
    unsafe {
        CFMachPort::invalidate(&tap);
        let _ = Arc::from_raw(state_ptr as *const Mutex<MacHotkeyState>);
    }
}

unsafe extern "C-unwind" fn event_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    let state = &*(user_info as *const Mutex<MacHotkeyState>);
    let cg_event = event.as_ref();
    let flags = CGEvent::flags(Some(cg_event));

    if matches!(
        event_type,
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
    ) {
        return event.as_ptr();
    }

    let key = match event_type {
        CGEventType::KeyDown | CGEventType::KeyUp => key_from_event(cg_event),
        CGEventType::FlagsChanged => None,
        _ => None,
    };
    let combo = hotkey_from_event(flags, key.as_deref());

    if let Ok(mut state) = state.lock() {
        if crate::hotkey::hotkey_capture_active(&state.app) {
            handle_capture_event(&mut state, event_type, combo);
            return event.as_ptr();
        }

        handle_runtime_event(&mut state, event_type, combo);
    }

    event.as_ptr()
}

fn handle_runtime_event(
    state: &mut MacHotkeyState,
    event_type: CGEventType,
    combo: Option<String>,
) {
    let target = current_hotkey_or_default(&state.app);
    match event_type {
        CGEventType::KeyDown => {
            if combo.as_deref() == Some(target.as_str()) && !state.pressed {
                state.pressed = true;
                trigger_hotkey_pressed(&state.app);
            }
        }
        CGEventType::KeyUp => {
            if state.pressed {
                state.pressed = false;
                trigger_hotkey_released(&state.app);
            }
        }
        CGEventType::FlagsChanged => {
            if combo.as_deref() == Some(target.as_str()) {
                if !state.pressed {
                    state.pressed = true;
                    trigger_hotkey_pressed(&state.app);
                }
            } else if state.pressed {
                state.pressed = false;
                trigger_hotkey_released(&state.app);
            }
        }
        _ => {}
    }
}

fn handle_capture_event(
    state: &mut MacHotkeyState,
    event_type: CGEventType,
    combo: Option<String>,
) {
    match event_type {
        CGEventType::KeyDown => {
            let Some(hotkey) = combo.filter(|value| hotkey_part_count(value) >= 2) else {
                return;
            };
            state.capture_last_hotkey = Some(hotkey.clone());
            emit_capture_event(&state.app, hotkey, true);
        }
        CGEventType::FlagsChanged => {
            let next_hotkey = combo.filter(|value| hotkey_part_count(value) >= 2);
            let next_count = next_hotkey
                .as_deref()
                .map(hotkey_part_count)
                .unwrap_or_default();
            let previous_count = state
                .capture_last_hotkey
                .as_deref()
                .map(hotkey_part_count)
                .unwrap_or_default();

            if next_count >= 2 && next_count >= previous_count {
                let Some(hotkey) = next_hotkey else {
                    return;
                };
                state.capture_last_hotkey = Some(hotkey.clone());
                emit_capture_event(&state.app, hotkey, true);
            } else if let Some(hotkey) = state.capture_last_hotkey.take() {
                emit_capture_event(&state.app, hotkey, false);
            }
        }
        CGEventType::KeyUp => {
            if let Some(hotkey) = state.capture_last_hotkey.take() {
                emit_capture_event(&state.app, hotkey, false);
            }
        }
        _ => {}
    }
}

fn emit_capture_event(app: &AppHandle<Wry>, hotkey: String, is_key_down: bool) {
    let _ = app.emit(
        "native-hotkey-capture",
        NativeHotkeyCaptureEvent {
            hotkey,
            is_key_down,
        },
    );
}

fn hotkey_from_event(flags: CGEventFlags, key: Option<&str>) -> Option<String> {
    let mut parts = Vec::new();
    if flags.contains(CGEventFlags::MaskControl) {
        parts.push("Ctrl".to_string());
    }
    if flags.contains(CGEventFlags::MaskAlternate) {
        parts.push("Alt".to_string());
    }
    if flags.contains(CGEventFlags::MaskShift) {
        parts.push("Shift".to_string());
    }
    if flags.contains(CGEventFlags::MaskCommand) {
        parts.push("Command".to_string());
    }
    if let Some(key) = key {
        parts.push(key.to_string());
    }
    (!parts.is_empty()).then(|| parts.join("+"))
}

fn hotkey_part_count(hotkey: &str) -> usize {
    hotkey.split('+').filter(|part| !part.is_empty()).count()
}

fn key_from_event(event: &CGEvent) -> Option<String> {
    let keycode = CGEvent::integer_value_field(Some(event), CGEventField::KeyboardEventKeycode);
    keycode_to_hotkey_label(keycode as u16).map(str::to_string)
}

fn keycode_to_hotkey_label(keycode: u16) -> Option<&'static str> {
    Some(match keycode {
        0x00 => "A",
        0x0B => "B",
        0x08 => "C",
        0x02 => "D",
        0x0E => "E",
        0x03 => "F",
        0x05 => "G",
        0x04 => "H",
        0x22 => "I",
        0x26 => "J",
        0x28 => "K",
        0x25 => "L",
        0x2E => "M",
        0x2D => "N",
        0x1F => "O",
        0x23 => "P",
        0x0C => "Q",
        0x0F => "R",
        0x01 => "S",
        0x11 => "T",
        0x20 => "U",
        0x09 => "V",
        0x0D => "W",
        0x07 => "X",
        0x10 => "Y",
        0x06 => "Z",
        0x1D => "0",
        0x12 => "1",
        0x13 => "2",
        0x14 => "3",
        0x15 => "4",
        0x17 => "5",
        0x16 => "6",
        0x1A => "7",
        0x1C => "8",
        0x19 => "9",
        0x31 => "Space",
        0x24 => "Enter",
        0x30 => "Tab",
        0x35 => "Escape",
        0x33 => "Backspace",
        0x75 => "Delete",
        0x72 => "Insert",
        0x73 => "Home",
        0x77 => "End",
        0x74 => "PageUp",
        0x79 => "PageDown",
        0x7B => "ArrowLeft",
        0x7C => "ArrowRight",
        0x7E => "ArrowUp",
        0x7D => "ArrowDown",
        0x1B => "Minus",
        0x18 => "Equal",
        0x21 => "BracketLeft",
        0x1E => "BracketRight",
        0x2A => "Backslash",
        0x29 => "Semicolon",
        0x27 => "Quote",
        0x2B => "Comma",
        0x2F => "Period",
        0x2C => "Slash",
        0x32 => "Backquote",
        0x7A => "F1",
        0x78 => "F2",
        0x63 => "F3",
        0x76 => "F4",
        0x60 => "F5",
        0x61 => "F6",
        0x62 => "F7",
        0x64 => "F8",
        0x65 => "F9",
        0x6D => "F10",
        0x67 => "F11",
        0x6F => "F12",
        0x69 => "F13",
        0x6B => "F14",
        0x71 => "F15",
        0x6A => "F16",
        0x40 => "F17",
        0x4F => "F18",
        0x50 => "F19",
        0x5A => "F20",
        _ => return None,
    })
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}
