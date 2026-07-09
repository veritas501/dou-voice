use std::fs;

use dou_voice_core::{AsrClientConfig, AsrEvent, AuthParamsStore};
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::app_state::{
    AsrDebugLogState, AuthStatusResult, DesktopState, DiagnosticsAsrSummary,
    DiagnosticsAuthSummary, DiagnosticsSnapshot, ExportDiagnosticsResult,
    PcmTranscribeOptionsSummary, VoiceDebugEvent, MAX_DIAGNOSTIC_EVENTS,
};
use crate::asr_options::streaming_transcribe_options;
use crate::build_info::app_build_info;
use crate::settings::{current_auth_path, diagnostics_output_path};
use crate::util::{text_preview, unix_time_ms};

/// 检查当前 auth 文件是否可用。
#[tauri::command]
pub(crate) fn check_auth_status(app: AppHandle<Wry>) -> Result<AuthStatusResult, String> {
    auth_status_result(&app)
}

/// 导出脱敏诊断快照到本地文件。
#[tauri::command]
pub(crate) fn export_diagnostics(app: AppHandle<Wry>) -> Result<ExportDiagnosticsResult, String> {
    let snapshot = build_diagnostics_snapshot(&app)?;
    let output_path = diagnostics_output_path(&app)?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create diagnostics directory: {error}"))?;
    }

    let event_count = snapshot.events.len();
    let data = serde_json::to_vec_pretty(&snapshot)
        .map_err(|error| format!("failed to serialize diagnostics: {error}"))?;
    fs::write(&output_path, data)
        .map_err(|error| format!("failed to write diagnostics: {error}"))?;

    Ok(ExportDiagnosticsResult {
        output_path: output_path.display().to_string(),
        event_count,
    })
}

pub(crate) fn auth_status_result(app: &AppHandle<Wry>) -> Result<AuthStatusResult, String> {
    let path = current_auth_path(app)?;
    let exists = path.exists();
    if !exists {
        return Ok(AuthStatusResult {
            path: path.display().to_string(),
            exists,
            load_ok: false,
            cookie_count: None,
            device_id_present: None,
            web_id_present: None,
            captured_at_unix_ms: None,
            error: Some("auth file does not exist".to_string()),
        });
    }

    match AuthParamsStore::new(&path).load() {
        Ok(auth) => Ok(AuthStatusResult {
            path: path.display().to_string(),
            exists,
            load_ok: true,
            cookie_count: Some(auth.cookies.len()),
            device_id_present: Some(!auth.device_id.is_empty()),
            web_id_present: Some(!auth.web_id.is_empty()),
            captured_at_unix_ms: Some(auth.captured_at_unix_ms),
            error: None,
        }),
        Err(error) => Ok(AuthStatusResult {
            path: path.display().to_string(),
            exists,
            load_ok: false,
            cookie_count: None,
            device_id_present: None,
            web_id_present: None,
            captured_at_unix_ms: None,
            error: Some(error.to_string()),
        }),
    }
}

/// 构造脱敏诊断快照。
fn build_diagnostics_snapshot(app: &AppHandle<Wry>) -> Result<DiagnosticsSnapshot, String> {
    let state = app.state::<DesktopState>();
    let voice_status = state
        .voice_status
        .lock()
        .map_err(|_| "voice status state poisoned".to_string())?
        .clone();
    let events = state
        .diagnostic_events
        .lock()
        .map_err(|_| "diagnostic event state poisoned".to_string())?
        .iter()
        .cloned()
        .collect::<Vec<_>>();

    Ok(DiagnosticsSnapshot {
        generated_at_unix_ms: unix_time_ms(),
        app_version: env!("CARGO_PKG_VERSION"),
        build: app_build_info(),
        platform: std::env::consts::OS,
        auth: diagnostics_auth_summary(app)?,
        asr: diagnostics_asr_summary(),
        voice_status,
        events,
    })
}

/// 读取认证文件摘要，避免泄漏敏感值。
fn diagnostics_auth_summary(app: &AppHandle<Wry>) -> Result<DiagnosticsAuthSummary, String> {
    Ok(DiagnosticsAuthSummary::from(auth_status_result(app)?))
}

fn diagnostics_asr_summary() -> DiagnosticsAsrSummary {
    let config = AsrClientConfig::default();
    DiagnosticsAsrSummary {
        endpoint: config.endpoint,
        origin: config.origin,
        streaming_options: PcmTranscribeOptionsSummary::from(streaming_transcribe_options()),
    }
}

/// 广播语音输入调试事件到前端 Activity。
pub(crate) fn emit_voice_debug(
    app: &AppHandle<Wry>,
    stage: impl Into<String>,
    message: impl Into<String>,
    chunk_count: Option<usize>,
    pcm_bytes: Option<usize>,
    text: Option<String>,
) {
    let event = VoiceDebugEvent {
        timestamp_unix_ms: unix_time_ms(),
        stage: stage.into(),
        message: message.into(),
        chunk_count,
        pcm_bytes,
        text,
    };
    append_diagnostic_event(app, event.clone());
    let _ = app.emit("voice-debug", event);
}

fn append_diagnostic_event(app: &AppHandle<Wry>, event: VoiceDebugEvent) {
    let state = app.state::<DesktopState>();
    if let Ok(mut events) = state.diagnostic_events.lock() {
        if events.len() == MAX_DIAGNOSTIC_EVENTS {
            events.pop_front();
        }
        events.push_back(event);
    };
}

pub(crate) fn emit_asr_debug_event(
    app: &AppHandle<Wry>,
    event: &AsrEvent,
    state: &mut AsrDebugLogState,
) {
    match event {
        AsrEvent::Opened => {
            emit_voice_debug(app, "asr_opened", "ASR WebSocket opened.", None, None, None)
        }
        AsrEvent::InputEnded { chunks, bytes } => emit_voice_debug(
            app,
            "asr_input_ended",
            format!("PCM input channel ended; ASR frames={chunks}."),
            Some(*chunks),
            Some(*bytes),
            None,
        ),
        AsrEvent::TailSilenceSent {
            chunks,
            bytes,
            duration_ms,
        } => emit_voice_debug(
            app,
            "asr_tail_silence",
            format!("Tail silence sent for {duration_ms} ms."),
            Some(*chunks),
            Some(*bytes),
            None,
        ),
        AsrEvent::WaitingForServer => emit_voice_debug(
            app,
            "asr_waiting",
            "Waiting for ASR final response.",
            None,
            None,
            None,
        ),
        AsrEvent::ReceiveTimeout { events, timeout_ms } => emit_voice_debug(
            app,
            "asr_receive_timeout",
            format!(
                "No ASR event received for {timeout_ms} ms; using current text. events={events}"
            ),
            None,
            None,
            None,
        ),
        AsrEvent::SocketClosed => emit_voice_debug(
            app,
            "asr_socket_closed",
            "ASR WebSocket closed by peer.",
            None,
            None,
            None,
        ),
        AsrEvent::Partial(text) => {
            emit_partial_debug_event(app, state, text);
            crate::voice::update_live_asr_text(app, text);
        }
        AsrEvent::Final(text) => {
            emit_voice_debug(
                app,
                "asr_final",
                format!("ASR final: {} chars.", text.chars().count()),
                None,
                None,
                Some(text_preview(text)),
            );
            crate::voice::update_live_asr_text(app, text);
        }
        AsrEvent::Finished => emit_voice_debug(
            app,
            "asr_finish",
            "ASR finish event received.",
            None,
            None,
            None,
        ),
        AsrEvent::AuthExpired => emit_voice_debug(
            app,
            "asr_auth_expired",
            "ASR reported expired auth.",
            None,
            None,
            None,
        ),
        AsrEvent::Error(message) => emit_voice_debug(
            app,
            "asr_server_error",
            format!("ASR server error: {message}"),
            None,
            None,
            None,
        ),
    }
}

/// 只在 partial 文本变化时输出完整内容，重复事件按低频摘要输出。
fn emit_partial_debug_event(app: &AppHandle<Wry>, state: &mut AsrDebugLogState, text: &str) {
    if state.last_partial_text.as_deref() == Some(text) {
        state.repeated_partial_count += 1;
        if state.repeated_partial_count == 10 || state.repeated_partial_count % 50 == 0 {
            emit_voice_debug(
                app,
                "asr_partial_repeat",
                format!(
                    "ASR partial repeated {} times.",
                    state.repeated_partial_count
                ),
                None,
                None,
                Some(text_preview(text)),
            );
        }
        return;
    }

    state.last_partial_text = Some(text.to_string());
    state.repeated_partial_count = 0;
    emit_voice_debug(
        app,
        "asr_partial",
        format!("ASR partial changed: {} chars.", text.chars().count()),
        None,
        None,
        Some(text_preview(text)),
    );
}
