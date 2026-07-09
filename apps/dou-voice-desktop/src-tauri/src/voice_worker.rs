use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use dou_voice_core::{
    start_input_streaming, transcribe_pcm_stream_with_events, AsrClientConfig, AsrEvent, AuthParams,
};
use tauri::Emitter;
use tauri::{AppHandle, Manager, Wry};

use crate::app_state::{
    AsrDebugLogState, RecordingWorker, StreamingRecognitionResult, OVERLAY_LABEL,
};
use crate::asr_options::streaming_transcribe_options;
use crate::diagnostics::{emit_asr_debug_event, emit_voice_debug};

/// 创建并启动流式录音 worker。
///
/// CPAL stream 需要留在创建线程内，因此主线程只保存停止信号和 ASR 结果接收端。
/// 音频分片通过无界 channel 送入 ASR future，连接未完成时自然形成待发送缓冲。
pub(crate) fn spawn_streaming_recording_worker(
    app: AppHandle<Wry>,
    auth: AuthParams,
    input_device: Option<String>,
) -> Result<RecordingWorker, String> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (result_tx, result_rx) = mpsc::channel::<Result<StreamingRecognitionResult, String>>();
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
    let (pcm_tx, pcm_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let (audio_tx, audio_rx) = mpsc::channel::<Vec<u8>>();
    let (asr_event_tx, mut asr_event_rx) = tokio::sync::mpsc::unbounded_channel::<AsrEvent>();
    let pcm_bytes = Arc::new(AtomicUsize::new(0));
    let chunk_count = Arc::new(AtomicUsize::new(0));
    let accepting_audio = Arc::new(AtomicBool::new(true));
    let result_pcm_bytes = Arc::clone(&pcm_bytes);
    let audio_pcm_bytes = Arc::clone(&pcm_bytes);
    let audio_chunk_count = Arc::clone(&chunk_count);
    let callback_accepting_audio = Arc::clone(&accepting_audio);

    emit_voice_debug(
        &app,
        "stream_worker",
        "Streaming worker created.",
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

    let asr_app = app.clone();
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

    // CPAL stream 不能安全放进 Tauri 全局 state；录音流固定留在工作线程内。
    let audio_app = app.clone();
    let audio_callback_app = audio_app.clone();
    std::thread::spawn(move || {
        let mut last_level_emit = Instant::now() - Duration::from_millis(80);
        match start_input_streaming(input_device.as_deref(), move |pcm| {
            if !callback_accepting_audio.load(Ordering::Relaxed) {
                return;
            }
            let levels = mic_levels_from_pcm(&pcm);
            if last_level_emit.elapsed() >= Duration::from_millis(80) {
                last_level_emit = Instant::now();
                emit_mic_level(&audio_callback_app, levels.clone());
            }
            let chunk_bytes = pcm.len();
            let chunks = audio_chunk_count.fetch_add(1, Ordering::Relaxed) + 1;
            let total_bytes =
                audio_pcm_bytes.fetch_add(chunk_bytes, Ordering::Relaxed) + chunk_bytes;
            if chunks <= 5 || chunks % 10 == 0 {
                let (peak, rms) = audio_level_summary(&levels);
                emit_voice_debug(
                    &audio_callback_app,
                    "audio_chunk",
                    format!("Queued PCM chunk: {chunk_bytes} bytes; peak={peak:.4}, rms={rms:.4}."),
                    Some(chunks),
                    Some(total_bytes),
                    None,
                );
            }
            if audio_tx.send(pcm).is_err() {
                emit_voice_debug(
                    &audio_callback_app,
                    "audio_channel_closed",
                    "Local PCM queue closed before audio stream stopped.",
                    Some(chunks),
                    Some(total_bytes),
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
                forward_audio_until_stopped(&audio_app, audio_rx, pcm_tx, stop_rx, accepting_audio);
                recording.stop();
                emit_voice_debug(
                    &audio_app,
                    "audio_stream_stopped",
                    "Input audio stream stopped; PCM channel closed.",
                    Some(chunk_count.load(Ordering::Relaxed)),
                    Some(pcm_bytes.load(Ordering::Relaxed)),
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
                let _ = ready_tx.send(Err(message.clone()));
            }
        }
    });

    ready_rx
        .recv()
        .map_err(|_| "recording worker failed to start".to_string())??;
    Ok(RecordingWorker { stop_tx, result_rx })
}

fn emit_mic_level(app: &AppHandle<Wry>, levels: Vec<f32>) {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = window.emit("mic-level", levels);
        return;
    }
    let _ = app.emit("mic-level", levels);
}

fn forward_audio_until_stopped(
    app: &AppHandle<Wry>,
    audio_rx: mpsc::Receiver<Vec<u8>>,
    pcm_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    stop_rx: mpsc::Receiver<()>,
    accepting_audio: Arc<AtomicBool>,
) {
    loop {
        match audio_rx.recv_timeout(Duration::from_millis(20)) {
            Ok(pcm) => {
                if pcm_tx.send(pcm).is_err() {
                    accepting_audio.store(false, Ordering::Relaxed);
                    emit_voice_debug(
                        app,
                        "audio_channel_closed",
                        "ASR PCM channel closed before stop signal.",
                        None,
                        None,
                        None,
                    );
                    return;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        match stop_rx.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => {
                accepting_audio.store(false, Ordering::Relaxed);
                break;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    accepting_audio.store(false, Ordering::Relaxed);
    while let Ok(pcm) = audio_rx.try_recv() {
        if pcm_tx.send(pcm).is_err() {
            break;
        }
    }
    drop(pcm_tx);
    emit_voice_debug(
        app,
        "pcm_channel_closed",
        "PCM channel closed; ASR input can finish.",
        None,
        None,
        None,
    );
}

fn audio_level_summary(levels: &[f32]) -> (f32, f32) {
    if levels.is_empty() {
        return (0.0, 0.0);
    }
    let peak = levels.iter().copied().fold(0.0_f32, f32::max);
    let rms = (levels.iter().map(|level| level * level).sum::<f32>() / levels.len() as f32).sqrt();
    (peak, rms)
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
