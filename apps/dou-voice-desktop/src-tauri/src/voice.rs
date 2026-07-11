use std::time::Duration;

use dou_voice_core::{
    record_input, transcribe_pcm_bytes, transcript_text_from_events, AsrClientConfig,
    AuthParamsStore, PcmTranscribeOptions,
};
use dou_voice_platform::feedback::FeedbackSound;
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::app_state::{
    DesktopState, StreamingRecognitionResult, VoiceInputResult, VoiceStatus,
    DEFAULT_RECORD_SECONDS, INPUT_METHOD_CLIPBOARD,
};
use crate::diagnostics::emit_voice_debug;
use crate::overlay::update_overlay_status;
use crate::settings::{
    current_auth_path, current_input_device, current_input_method, microphone_always_on,
    sound_enabled,
};
use crate::tray::update_tray_status;
use crate::util::text_preview;
use crate::voice_worker::spawn_hotkey_recording_worker;

/// 读取当前语音输入状态，供主窗口首次加载时渲染。
#[tauri::command]
pub(crate) fn get_voice_status(
    state: tauri::State<'_, DesktopState>,
) -> Result<VoiceStatus, String> {
    state
        .voice_status
        .lock()
        .map_err(|_| "voice status state poisoned".to_string())
        .map(|status| status.clone())
}

/// 固定时长录音测试入口。
///
/// 托盘菜单和主窗口按钮仍保留该路径，便于验证麦克风、ASR 和直接输入是否可用。
#[tauri::command]
pub(crate) async fn record_once_and_type(
    app: AppHandle<Wry>,
    seconds: Option<u64>,
) -> Result<VoiceInputResult, String> {
    run_voice_input(app, seconds.unwrap_or(DEFAULT_RECORD_SECONDS)).await
}

/// 执行一次固定时长语音输入。
///
/// 该路径用于测试按钮和托盘菜单；真正产品形态的热键路径使用 start/finish 分离流程。
pub(crate) async fn run_voice_input(
    app: AppHandle<Wry>,
    seconds: u64,
) -> Result<VoiceInputResult, String> {
    if seconds == 0 {
        return Err("record seconds must be greater than zero".to_string());
    }
    begin_voice_input(&app)?;
    let result = run_voice_input_body(&app, seconds).await;
    finish_voice_input(&app, &result);
    result
}

async fn run_voice_input_body(
    app: &AppHandle<Wry>,
    seconds: u64,
) -> Result<VoiceInputResult, String> {
    set_voice_status(app, "recording", format!("Recording for {seconds}s."), None);
    let input_device = current_input_device(app)?;
    let pcm = tauri::async_runtime::spawn_blocking(move || {
        record_input(Duration::from_secs(seconds), input_device.as_deref())
    })
    .await
    .map_err(|error| format!("recording task failed: {error}"))?
    .map_err(|error| error.to_string())?;

    recognize_and_type_pcm(app, pcm).await
}

pub(crate) async fn start_hotkey_recording(app: AppHandle<Wry>) -> Result<(), String> {
    emit_voice_debug(
        &app,
        "hotkey_pressed",
        "Hotkey pressed; preparing streaming recognition.",
        None,
        None,
        None,
    );
    begin_voice_input(&app)?;
    let result = start_hotkey_recording_body(&app);
    if let Err(error) = &result {
        finish_voice_input(&app, &Err(error.clone()));
    }
    result
}

fn start_hotkey_recording_body(app: &AppHandle<Wry>) -> Result<(), String> {
    let auth_path = current_auth_path(app)?;
    set_voice_status(
        app,
        "loading_auth",
        format!("Loading auth: {}", auth_path.display()),
        None,
    );
    let auth = AuthParamsStore::new(&auth_path)
        .load()
        .map_err(|error| format!("failed to load auth: {error}"))?;
    emit_voice_debug(
        app,
        "auth_loaded",
        format!("Auth loaded from {}", auth_path.display()),
        None,
        None,
        None,
    );

    set_voice_status(app, "starting", "Starting recording.".to_string(), None);
    let input_device = current_input_device(app)?;
    let worker =
        spawn_hotkey_recording_worker(app.clone(), auth, input_device, microphone_always_on(app)?)?;
    {
        let state = app.state::<DesktopState>();
        let mut active = state
            .active_recording
            .lock()
            .map_err(|_| "active recording state poisoned".to_string())?;
        *active = Some(worker);
    }
    set_voice_status(
        app,
        "recording",
        "Recording while hotkey is held.".to_string(),
        None,
    );
    Ok(())
}

pub(crate) async fn finish_hotkey_recording(
    app: AppHandle<Wry>,
) -> Result<VoiceInputResult, String> {
    emit_voice_debug(
        &app,
        "hotkey_released",
        "Hotkey released; stopping audio stream.",
        None,
        None,
        None,
    );
    let result = finish_hotkey_recording_body(&app).await;
    finish_voice_input(&app, &result);
    result
}

async fn finish_hotkey_recording_body(app: &AppHandle<Wry>) -> Result<VoiceInputResult, String> {
    let recording = {
        let state = app.state::<DesktopState>();
        let mut active = state
            .active_recording
            .lock()
            .map_err(|_| "active recording state poisoned".to_string())?;
        active.take()
    };
    let Some(recording) = recording else {
        return Err("no active recording".to_string());
    };

    let live_text = current_voice_text(app);
    set_voice_status(
        app,
        "stopping",
        "Stopping recording.".to_string(),
        live_text.clone(),
    );
    set_voice_status(
        app,
        "recognizing",
        "Waiting for ASR result.".to_string(),
        live_text,
    );
    let result = tauri::async_runtime::spawn_blocking(move || {
        recording.stop_input();
        recording
            .result_rx
            .recv()
            .map_err(|_| "recording worker returned no result".to_string())?
    })
    .await
    .map_err(|error| format!("stop recording task failed: {error}"))?
    .map_err(|error| error.to_string())?;

    type_recognition_result(app, result).await
}

async fn type_recognition_result(
    app: &AppHandle<Wry>,
    result: StreamingRecognitionResult,
) -> Result<VoiceInputResult, String> {
    let Some(final_text) = transcript_text_from_events(&result.events) else {
        emit_empty_text_debug(app, result.events.len(), result.pcm_bytes);
        return Ok(VoiceInputResult {
            final_text: String::new(),
            pcm_bytes: result.pcm_bytes,
        });
    };
    emit_voice_debug(
        app,
        "final_text",
        format!(
            "Selected final text: {} chars from {} ASR events.",
            final_text.chars().count(),
            result.events.len()
        ),
        None,
        Some(result.pcm_bytes),
        Some(text_preview(&final_text)),
    );

    set_voice_status(
        app,
        "typing",
        "Typing into the focused window.".to_string(),
        Some(final_text.clone()),
    );
    type_text_to_focused_window(app, &final_text)?;

    Ok(VoiceInputResult {
        final_text,
        pcm_bytes: result.pcm_bytes,
    })
}

async fn recognize_and_type_pcm(
    app: &AppHandle<Wry>,
    pcm: Vec<u8>,
) -> Result<VoiceInputResult, String> {
    let auth_path = current_auth_path(app)?;
    set_voice_status(
        app,
        "loading_auth",
        format!("Loading auth: {}", auth_path.display()),
        None,
    );
    let auth = AuthParamsStore::new(&auth_path)
        .load()
        .map_err(|error| format!("failed to load auth: {error}"))?;

    set_voice_status(app, "recognizing", "Recognizing.".to_string(), None);
    let events = transcribe_pcm_bytes(
        &AsrClientConfig::default(),
        &auth,
        &pcm,
        &PcmTranscribeOptions::default(),
    )
    .await
    .map_err(|error| format!("recognition failed: {error}"))?;
    let Some(final_text) = transcript_text_from_events(&events) else {
        emit_empty_text_debug(app, events.len(), pcm.len());
        return Ok(VoiceInputResult {
            final_text: String::new(),
            pcm_bytes: pcm.len(),
        });
    };

    set_voice_status(
        app,
        "typing",
        "Typing into the focused window.".to_string(),
        Some(final_text.clone()),
    );
    type_text_to_focused_window(app, &final_text)?;

    Ok(VoiceInputResult {
        final_text,
        pcm_bytes: pcm.len(),
    })
}

fn type_text_to_focused_window(app: &AppHandle<Wry>, text: &str) -> Result<(), String> {
    let input_method = current_input_method(app)?;
    let outcome = if input_method == INPUT_METHOD_CLIPBOARD {
        // 用户显式选择"剪贴板"模式：只写剪贴板，提示手动粘贴。
        dou_voice_platform::input::copy_text_to_clipboard(text)
            .map_err(|error| format!("failed to copy text to clipboard: {error}"))?
    } else {
        // 默认"直接"模式：分层 fallback，最后一层是剪贴板。
        dou_voice_platform::input::type_text(text)
            .map_err(|error| format!("failed to type text: {error}"))?
    };

    match outcome.method {
        dou_voice_platform::input::TextInputMethod::Direct => emit_voice_debug(
            app,
            "input_method",
            "Text input completed via enigo (SendInput Unicode).".to_string(),
            None,
            None,
            None,
        ),
        dou_voice_platform::input::TextInputMethod::Clipboard => {
            if input_method == INPUT_METHOD_CLIPBOARD {
                // 用户显式选择"剪贴板"模式：文本仅写入剪贴板，需手动粘贴。
                emit_voice_debug(
                    app,
                    "input_method",
                    "Text copied to clipboard; user must paste manually.".to_string(),
                    None,
                    None,
                    None,
                )
            } else {
                // 自动 fallback 到剪贴板 + Ctrl+V：已自动粘贴，但前序 enigo 失败。
                emit_voice_debug(
                    app,
                    "input_fallback",
                    format!(
                        "Text pasted via clipboard + Ctrl+V fallback. prior_error={}",
                        outcome.prior_error.as_deref().unwrap_or("unknown")
                    ),
                    None,
                    None,
                    None,
                )
            }
        }
    }
    Ok(())
}

pub(crate) fn voice_input_busy(app: &AppHandle<Wry>) -> bool {
    let state = app.state::<DesktopState>();
    state.voice_busy.lock().map(|busy| *busy).unwrap_or(false)
}

pub(crate) fn note_hotkey_ignored_while_busy(app: &AppHandle<Wry>) {
    let (phase, last_text) = {
        let state = app.state::<DesktopState>();
        state
            .voice_status
            .lock()
            .map(|status| (status.phase.clone(), status.last_text.clone()))
            .unwrap_or_else(|_| ("starting".to_string(), None))
    };
    let message = match phase.as_str() {
        "recording" => "Already recording. Release hotkey to transcribe.",
        "stopping" | "recognizing" => "Still recognizing previous speech. Please wait.",
        "typing" => "Typing previous result. Please wait.",
        _ => "Voice input is already running. Please wait.",
    };

    emit_voice_debug(app, "hotkey_ignored", message.to_string(), None, None, None);
    set_voice_status(app, phase, message.to_string(), last_text);
}

fn begin_voice_input(app: &AppHandle<Wry>) -> Result<(), String> {
    let state = app.state::<DesktopState>();
    {
        let mut busy = state
            .voice_busy
            .lock()
            .map_err(|_| "voice busy state poisoned".to_string())?;
        if *busy {
            return Err("voice input is already running".to_string());
        }
        *busy = true;
    }
    set_voice_status(app, "starting", "Preparing voice input.".to_string(), None);
    Ok(())
}

fn finish_voice_input(app: &AppHandle<Wry>, result: &Result<VoiceInputResult, String>) {
    {
        let state = app.state::<DesktopState>();
        if let Ok(mut busy) = state.voice_busy.lock() {
            *busy = false;
        };
    }
    match result {
        Ok(result) if result.final_text.trim().is_empty() => {
            set_voice_status(app, "idle", "No speech detected.".to_string(), None)
        }
        Ok(result) => set_voice_status(
            app,
            "idle",
            "Input completed.".to_string(),
            Some(result.final_text.clone()),
        ),
        Err(error) => set_voice_status(app, "error", error.clone(), None),
    }
}

fn emit_empty_text_debug(app: &AppHandle<Wry>, event_count: usize, pcm_bytes: usize) {
    emit_voice_debug(
        app,
        "empty_text",
        format!("ASR completed with {event_count} events but returned no text."),
        None,
        Some(pcm_bytes),
        None,
    );
}

fn set_voice_status(
    app: &AppHandle<Wry>,
    phase: impl Into<String>,
    message: String,
    last_text: Option<String>,
) {
    let phase = phase.into();
    let status = VoiceStatus {
        phase,
        message,
        last_text,
    };

    let previous_phase = {
        let state = app.state::<DesktopState>();
        let result = if let Ok(mut latest) = state.voice_status.lock() {
            let previous_phase = latest.phase.clone();
            *latest = status.clone();
            previous_phase
        } else {
            String::new()
        };
        result
    };

    update_tray_status(app, &status);
    if sound_enabled(app).unwrap_or(true) {
        if let Some(sound) = feedback_sound_for_transition(&previous_phase, &status.phase) {
            dou_voice_platform::feedback::play_sound(sound);
        }
    }

    let _ = app.emit("voice-status", status);
    update_overlay_status(app);
}

fn current_voice_text(app: &AppHandle<Wry>) -> Option<String> {
    let state = app.state::<DesktopState>();
    state
        .voice_status
        .lock()
        .ok()
        .and_then(|status| status.last_text.clone())
        .filter(|text| !text.is_empty())
}

/// ASR partial/final 到达时刷新实时展示文本。
pub(crate) fn update_live_asr_text(app: &AppHandle<Wry>, text: &str) {
    if text.is_empty() {
        return;
    }

    let phase = {
        let state = app.state::<DesktopState>();
        state
            .voice_status
            .lock()
            .map(|status| status.phase.clone())
            .unwrap_or_else(|_| "recognizing".to_string())
    };
    if !matches!(phase.as_str(), "recording" | "stopping" | "recognizing") {
        return;
    }

    let message = match phase.as_str() {
        "recording" => "Listening.".to_string(),
        "stopping" => "Stopping recording.".to_string(),
        _ => "Recognizing.".to_string(),
    };
    set_voice_status(app, phase, message, Some(text.to_string()));
}

fn feedback_sound_for_transition(previous: &str, next: &str) -> Option<FeedbackSound> {
    if previous == next {
        return None;
    }
    if next == "error" {
        return Some(FeedbackSound::Error);
    }
    if next == "recording" {
        return Some(FeedbackSound::Start);
    }
    if previous == "recording" && matches!(next, "stopping" | "loading_auth" | "recognizing") {
        return Some(FeedbackSound::Stop);
    }
    if next == "idle" && previous != "idle" {
        return Some(FeedbackSound::Complete);
    }
    None
}
