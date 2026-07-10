use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use dou_voice_core::{
    start_input_streaming, transcribe_pcm_stream_with_events, AsrClientConfig, AsrEvent, AuthParams,
};
use tauri::Emitter;
use tauri::{AppHandle, Manager, Wry};

use crate::app_state::{
    AsrDebugLogState, PcmSender, RecordingInputStop, RecordingWorker, StreamingRecognitionResult,
    OVERLAY_LABEL,
};
use crate::asr_options::streaming_transcribe_options;
use crate::diagnostics::{emit_asr_debug_event, emit_voice_debug};
use crate::microphone_worker::{attach_prewarmed_microphone, prewarmed_microphone_running};

/// 已创建的识别会话。音频来源由调用者接入，连接完成前 PCM 会保留在本地队列中。
pub(crate) struct StreamingRecognitionWorker {
    pub(crate) result_rx: mpsc::Receiver<Result<StreamingRecognitionResult, String>>,
    pub(crate) pcm_tx: PcmSender,
}

/// 创建流式识别会话，但不创建麦克风流。
///
/// 常规模式和“本地麦克风常开”模式共用这个部分：前者临时创建 CPAL 流，后者把
/// 常开流在热键按下时接入。这样两种模式的 ASR 协议、诊断和关闭语义保持一致。
pub(crate) fn spawn_streaming_recognition_worker(
    app: AppHandle<Wry>,
    auth: AuthParams,
) -> StreamingRecognitionWorker {
    let (result_tx, result_rx) = mpsc::channel::<Result<StreamingRecognitionResult, String>>();
    let (pcm_tx, pcm_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let (asr_event_tx, mut asr_event_rx) = tokio::sync::mpsc::unbounded_channel::<AsrEvent>();
    let pcm_bytes = Arc::new(AtomicUsize::new(0));
    let chunk_count = Arc::new(AtomicUsize::new(0));

    emit_voice_debug(
        &app,
        "stream_worker",
        "Streaming recognition worker created.",
        None,
        None,
        None,
    );

    let event_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut debug_state = AsrDebugLogState::default();
        while let Some(event) = asr_event_rx.recv().await {
            emit_asr_debug_event(&event_app, &event, &mut debug_state);
        }
    });

    let forward_app = app.clone();
    let forward_pcm_bytes = Arc::clone(&pcm_bytes);
    let forward_chunk_count = Arc::clone(&chunk_count);
    tauri::async_runtime::spawn(async move {
        while let Some(pcm) = audio_rx.recv().await {
            let chunk_bytes = pcm.len();
            let chunks = forward_chunk_count.fetch_add(1, Ordering::Relaxed) + 1;
            let total_bytes =
                forward_pcm_bytes.fetch_add(chunk_bytes, Ordering::Relaxed) + chunk_bytes;
            if chunks <= 5 || chunks % 10 == 0 {
                emit_voice_debug(
                    &forward_app,
                    "audio_chunk",
                    format!("Queued PCM chunk: {chunk_bytes} bytes."),
                    Some(chunks),
                    Some(total_bytes),
                    None,
                );
            }
            if pcm_tx.send(pcm).is_err() {
                emit_voice_debug(
                    &forward_app,
                    "audio_channel_closed",
                    "ASR PCM channel closed before audio input stopped.",
                    Some(chunks),
                    Some(total_bytes),
                    None,
                );
                return;
            }
        }
        emit_voice_debug(
            &forward_app,
            "pcm_channel_closed",
            "PCM channel closed; ASR input can finish.",
            Some(forward_chunk_count.load(Ordering::Relaxed)),
            Some(forward_pcm_bytes.load(Ordering::Relaxed)),
            None,
        );
    });

    let asr_app = app;
    let result_pcm_bytes = Arc::clone(&pcm_bytes);
    tauri::async_runtime::spawn(async move {
        emit_voice_debug(
            &asr_app,
            "asr_connecting",
            "Connecting ASR WebSocket.",
            None,
            None,
            None,
        );
        let result = transcribe_pcm_stream_with_events(
            &AsrClientConfig::default(),
            &auth,
            pcm_rx,
            &streaming_transcribe_options(),
            asr_event_tx,
        )
        .await;
        let result = match result {
            Ok(events) => {
                let pcm_bytes = result_pcm_bytes.load(Ordering::Relaxed);
                emit_voice_debug(
                    &asr_app,
                    "asr_done",
                    format!("ASR completed with {} events.", events.len()),
                    None,
                    Some(pcm_bytes),
                    None,
                );
                Ok(StreamingRecognitionResult { events, pcm_bytes })
            }
            Err(error) => {
                emit_voice_debug(
                    &asr_app,
                    "asr_error",
                    format!("ASR failed: {error}"),
                    None,
                    Some(result_pcm_bytes.load(Ordering::Relaxed)),
                    None,
                );
                Err(format!("recognition failed: {error}"))
            }
        };
        let _ = result_tx.send(result);
    });

    StreamingRecognitionWorker {
        result_rx,
        pcm_tx: audio_tx,
    }
}

/// 创建并启动按键触发的临时录音 worker。
///
/// CPAL stream 需要留在创建线程内，因此主线程只保存停止信号和结果接收端。
pub(crate) fn spawn_streaming_recording_worker(
    app: AppHandle<Wry>,
    auth: AuthParams,
    input_device: Option<String>,
) -> Result<RecordingWorker, String> {
    let StreamingRecognitionWorker { result_rx, pcm_tx } =
        spawn_streaming_recognition_worker(app.clone(), auth);
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
    let accepting_audio = Arc::new(AtomicBool::new(true));
    let callback_accepting_audio = Arc::clone(&accepting_audio);
    let audio_app = app.clone();
    let audio_callback_app = audio_app.clone();

    std::thread::spawn(move || {
        let mut last_level_emit = Instant::now() - Duration::from_millis(80);
        match start_input_streaming(input_device.as_deref(), move |pcm| {
            if !callback_accepting_audio.load(Ordering::Relaxed) {
                return;
            }
            if last_level_emit.elapsed() >= Duration::from_millis(80) {
                last_level_emit = Instant::now();
                emit_mic_level(&audio_callback_app, mic_levels_from_pcm(&pcm));
            }
            if pcm_tx.send(pcm).is_err() {
                callback_accepting_audio.store(false, Ordering::Relaxed);
                emit_voice_debug(
                    &audio_callback_app,
                    "audio_channel_closed",
                    "Local PCM queue closed before audio stream stopped.",
                    None,
                    None,
                    None,
                );
            }
        }) {
            Ok(recording) => {
                emit_voice_debug(
                    &audio_app,
                    "audio_stream_ready",
                    "Input audio stream started.",
                    None,
                    None,
                    None,
                );
                let _ = ready_tx.send(Ok(()));
                let _ = stop_rx.recv();
                accepting_audio.store(false, Ordering::Relaxed);
                recording.stop();
                emit_voice_debug(
                    &audio_app,
                    "audio_stream_stopped",
                    "Input audio stream stopped; PCM channel closed.",
                    None,
                    None,
                    None,
                );
            }
            Err(error) => {
                let message = error.to_string();
                emit_voice_debug(
                    &audio_app,
                    "audio_stream_error",
                    format!("Input audio stream failed: {message}"),
                    None,
                    None,
                    None,
                );
                let _ = ready_tx.send(Err(message));
            }
        }
    });

    ready_rx
        .recv()
        .map_err(|_| "recording worker failed to start".to_string())??;
    Ok(RecordingWorker {
        input_stop: RecordingInputStop::OnDemand { stop_tx },
        result_rx,
    })
}

/// 按当前本地麦克风模式创建热键录音来源。
pub(crate) fn spawn_hotkey_recording_worker(
    app: AppHandle<Wry>,
    auth: AuthParams,
    input_device: Option<String>,
    microphone_always_on: bool,
) -> Result<RecordingWorker, String> {
    if microphone_always_on && prewarmed_microphone_running(&app)? {
        let StreamingRecognitionWorker { result_rx, pcm_tx } =
            spawn_streaming_recognition_worker(app.clone(), auth);
        let input_stop = attach_prewarmed_microphone(&app, pcm_tx)?
            .ok_or_else(|| "prewarmed microphone is not running".to_string())?;
        emit_voice_debug(
            &app,
            "prewarmed_mic_attached",
            "Attached local microphone stream to this ASR session.",
            None,
            None,
            None,
        );
        return Ok(RecordingWorker {
            input_stop,
            result_rx,
        });
    }

    if microphone_always_on {
        emit_voice_debug(
            &app,
            "prewarmed_mic_unavailable",
            "Local microphone stream is unavailable; starting an on-demand stream.",
            None,
            None,
            None,
        );
    }
    spawn_streaming_recording_worker(app, auth, input_device)
}

fn emit_mic_level(app: &AppHandle<Wry>, levels: Vec<f32>) {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = window.emit("mic-level", levels);
        return;
    }
    let _ = app.emit("mic-level", levels);
}

fn mic_levels_from_pcm(pcm: &[u8]) -> Vec<f32> {
    const BARS: usize = 9;
    let samples = pcm
        .chunks_exact(2)
        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32768.0)
        .collect::<Vec<_>>();
    if samples.is_empty() {
        return vec![0.0; BARS];
    }

    let bucket_size = (samples.len() / BARS).max(1);
    (0..BARS)
        .map(|index| {
            let start = index * bucket_size;
            let end = if index == BARS - 1 {
                samples.len()
            } else {
                ((index + 1) * bucket_size).min(samples.len())
            };
            if start >= end {
                return 0.0;
            }
            let sum = samples[start..end]
                .iter()
                .map(|sample| sample * sample)
                .sum::<f32>();
            (sum / (end - start) as f32).sqrt().clamp(0.0, 1.0)
        })
        .collect()
}
