use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use dou_voice_core::start_input_streaming;
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::app_state::{
    DesktopState, MicrophoneControl, PcmSender, PrewarmedMicrophone, RecordingInputStop,
    OVERLAY_LABEL,
};
use crate::diagnostics::emit_voice_debug;

struct ActiveDestination {
    generation: u64,
    pcm_tx: PcmSender,
}

type Destination = Arc<Mutex<Option<ActiveDestination>>>;

/// 启动常开的本地麦克风。
///
/// 音频流始终由专属线程持有。空闲时 callback 找不到 destination，会立即丢弃 PCM；
/// 因此开启该模式不会让音频上传，也不会在内存中累计录音。
pub(crate) fn spawn_prewarmed_microphone(
    app: AppHandle<Wry>,
    input_device: Option<String>,
) -> Result<PrewarmedMicrophone, String> {
    let (control_tx, control_rx) = mpsc::channel::<MicrophoneControl>();
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
    let destination: Destination = Arc::new(Mutex::new(None));
    let thread_destination = Arc::clone(&destination);
    let callback_control_tx = control_tx.clone();
    let thread_app = app.clone();
    let thread_input_device = input_device.clone();

    let join_handle = std::thread::spawn(move || {
        let mut last_level_emit = Instant::now() - Duration::from_millis(80);
        let callback_destination = Arc::clone(&thread_destination);
        let callback_app = thread_app.clone();
        match start_input_streaming(thread_input_device.as_deref(), move |pcm| {
            let active = callback_destination.lock().ok().and_then(|destination| {
                destination
                    .as_ref()
                    .map(|target| (target.generation, target.pcm_tx.clone()))
            });
            let Some((generation, pcm_tx)) = active else {
                return;
            };

            if last_level_emit.elapsed() >= Duration::from_millis(80) {
                last_level_emit = Instant::now();
                emit_mic_level(&callback_app, mic_levels_from_pcm(&pcm));
            }

            if pcm_tx.send(pcm).is_err() {
                // ASR 已提前结束时，仅移除对应 generation，不能误断开下一次热键。
                let _ = callback_control_tx.send(MicrophoneControl::Detach { generation });
            }
        }) {
            Ok(recording) => {
                emit_voice_debug(
                    &thread_app,
                    "prewarmed_mic_ready",
                    "Local microphone stream is running; idle audio is discarded.",
                    None,
                    None,
                    None,
                );
                let _ = ready_tx.send(Ok(()));
                run_control_loop(control_rx, thread_destination);
                recording.stop();
                emit_voice_debug(
                    &thread_app,
                    "prewarmed_mic_stopped",
                    "Local microphone stream stopped.",
                    None,
                    None,
                    None,
                );
            }
            Err(error) => {
                let message = error.to_string();
                emit_voice_debug(
                    &thread_app,
                    "prewarmed_mic_error",
                    format!("Local microphone stream failed: {message}"),
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
        .map_err(|_| "prewarmed microphone worker failed to start".to_string())??;
    Ok(PrewarmedMicrophone {
        control_tx,
        input_device,
        join_handle,
    })
}

impl PrewarmedMicrophone {
    /// 把当前热键的 ASR sender 接入本地流，返回该次录音的精确断开句柄。
    pub(crate) fn attach(&self, pcm_tx: PcmSender) -> Result<RecordingInputStop, String> {
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.control_tx
            .send(MicrophoneControl::Attach {
                pcm_tx,
                response_tx,
            })
            .map_err(|_| "prewarmed microphone worker is not running".to_string())?;
        let generation = response_rx
            .recv()
            .map_err(|_| "prewarmed microphone did not confirm attachment".to_string())??;
        Ok(RecordingInputStop::Prewarmed {
            control_tx: self.control_tx.clone(),
            generation,
        })
    }

    /// 关闭常开本地流并等待其释放设备。
    pub(crate) fn stop(self) {
        let _ = self.control_tx.send(MicrophoneControl::Stop);
        let _ = self.join_handle.join();
    }
}

/// 让运行时本地麦克风与保存后的设置一致。
///
/// 设备变更时必须先释放旧 stream，才能可靠地切换到新设备。若新设备启动失败，尽力
/// 恢复旧设备，且不让应用启动或设置页面因此崩溃。
pub(crate) fn reconcile_prewarmed_microphone(
    app: &AppHandle<Wry>,
    enabled: bool,
    input_device: Option<String>,
) -> Result<(), String> {
    let existing = {
        let state = app.state::<DesktopState>();
        let mut microphone = state
            .prewarmed_microphone
            .lock()
            .map_err(|_| "prewarmed microphone state poisoned".to_string())?;
        if enabled
            && microphone
                .as_ref()
                .is_some_and(|worker| worker.input_device == input_device)
        {
            return Ok(());
        }
        microphone.take()
    };

    let previous_device = existing.as_ref().map(|worker| worker.input_device.clone());
    if let Some(worker) = existing {
        worker.stop();
    }

    if !enabled {
        return Ok(());
    }

    match spawn_prewarmed_microphone(app.clone(), input_device) {
        Ok(worker) => {
            let state = app.state::<DesktopState>();
            let mut microphone = state
                .prewarmed_microphone
                .lock()
                .map_err(|_| "prewarmed microphone state poisoned".to_string())?;
            *microphone = Some(worker);
            Ok(())
        }
        Err(error) => {
            if let Some(previous_device) = previous_device {
                if let Ok(worker) = spawn_prewarmed_microphone(app.clone(), previous_device) {
                    let state = app.state::<DesktopState>();
                    if let Ok(mut microphone) = state.prewarmed_microphone.lock() {
                        *microphone = Some(worker);
                    };
                }
            }
            Err(error)
        }
    }
}

/// 把常开本地流接入当前 ASR 会话。返回 None 表示此时没有正在运行的本地流。
pub(crate) fn attach_prewarmed_microphone(
    app: &AppHandle<Wry>,
    pcm_tx: PcmSender,
) -> Result<Option<RecordingInputStop>, String> {
    let state = app.state::<DesktopState>();
    let microphone = state
        .prewarmed_microphone
        .lock()
        .map_err(|_| "prewarmed microphone state poisoned".to_string())?;
    microphone
        .as_ref()
        .map(|worker| worker.attach(pcm_tx))
        .transpose()
}

/// 返回常开本地流是否已成功占用当前输入设备。
pub(crate) fn prewarmed_microphone_running(app: &AppHandle<Wry>) -> Result<bool, String> {
    let state = app.state::<DesktopState>();
    state
        .prewarmed_microphone
        .lock()
        .map_err(|_| "prewarmed microphone state poisoned".to_string())
        .map(|microphone| microphone.is_some())
}

fn run_control_loop(control_rx: mpsc::Receiver<MicrophoneControl>, destination: Destination) {
    let mut next_generation = 0_u64;
    while let Ok(command) = control_rx.recv() {
        match command {
            MicrophoneControl::Attach {
                pcm_tx,
                response_tx,
            } => {
                let response = destination
                    .lock()
                    .map_err(|_| "prewarmed microphone destination state poisoned".to_string())
                    .and_then(|mut destination| {
                        if destination.is_some() {
                            return Err(
                                "prewarmed microphone already has an active recording".to_string()
                            );
                        }
                        next_generation = next_generation.wrapping_add(1);
                        *destination = Some(ActiveDestination {
                            generation: next_generation,
                            pcm_tx,
                        });
                        Ok(next_generation)
                    });
                let _ = response_tx.send(response);
            }
            MicrophoneControl::Detach { generation } => {
                detach_generation(&destination, generation);
            }
            MicrophoneControl::Stop => {
                if let Ok(mut destination) = destination.lock() {
                    *destination = None;
                }
                return;
            }
        }
    }
}

fn detach_generation(destination: &Destination, generation: u64) {
    if let Ok(mut destination) = destination.lock() {
        if destination
            .as_ref()
            .is_some_and(|target| target.generation == generation)
        {
            *destination = None;
        }
    }
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
