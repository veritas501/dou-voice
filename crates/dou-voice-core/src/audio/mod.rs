use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::{CoreError, CoreResult};

/// 音频格式描述。
///
/// 核心 ASR 链路当前统一使用 16kHz mono s16le PCM，平台采集层需要在进入 ASR 前
/// 转换到这个格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    /// 采样率，单位 Hz。
    pub sample_rate_hz: u32,
    /// 声道数。
    pub channels: u16,
}

impl AudioFormat {
    pub const PCM_16K_MONO: Self = Self {
        sample_rate_hz: 16_000,
        channels: 1,
    };
}

/// 一段已解码的 PCM 样本。
///
/// 当前流式接口尚未接入生产路径，但保留该结构用于后续状态机和实时发送。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmChunk {
    /// 样本格式。
    pub format: AudioFormat,
    /// i16 PCM 样本。
    pub samples: Vec<i16>,
}

/// 可供录音使用的输入设备信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDeviceInfo {
    /// 设备名称。桌面端会用该名称持久化用户选择。
    pub name: String,
    /// 是否是系统当前默认输入设备。
    pub is_default: bool,
}

/// 平台音频采集抽象。
///
/// 最终产品应通过该边界接入持续录音状态机；当前 MVP 使用下面的 CPAL helper。
pub trait AudioCapture {
    /// 开始采集并通过回调交付 PCM 分片。
    fn start(&mut self, on_chunk: Box<dyn FnMut(PcmChunk) + Send>) -> CoreResult<()>;

    /// 停止采集并释放设备资源。
    fn stop(&mut self) -> CoreResult<()>;
}

/// 正在进行的默认输入设备录音。
///
/// 注意：CPAL 的 `Stream` 在 Windows 上不是跨线程 `Send`。桌面端如果需要跨事件
/// 回调控制录音，必须把该对象固定在创建它的工作线程内。
pub struct ActiveRecording {
    stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    source_sample_rate: u32,
    source_channels: usize,
}

/// 正在进行的实时 PCM 分片录音。
///
/// 与 `ActiveRecording` 不同，该类型不保存整段音频，而是在 CPAL 回调中持续输出
/// 已转换好的 16kHz mono s16le PCM 分片。
pub struct ActiveStreamRecording {
    stream: cpal::Stream,
}

impl ActiveStreamRecording {
    /// 停止实时录音并释放输入设备。
    pub fn stop(self) {
        drop(self.stream);
    }
}

impl ActiveRecording {
    /// 停止录音并返回 16kHz mono s16le PCM 字节。
    pub fn stop(self) -> CoreResult<Vec<u8>> {
        let Self {
            stream,
            samples,
            source_sample_rate,
            source_channels,
        } = self;
        drop(stream);

        let recorded = samples
            .lock()
            .map_err(|_| {
                CoreError::AudioUnavailable("Internal microphone buffer state is corrupted (mutex poisoned)".to_string())
            })?
            .clone();
        if recorded.is_empty() {
            return Err(CoreError::AudioUnavailable(
                "input device returned no samples".to_string(),
            ));
        }

        Ok(convert_to_16k_mono_s16le(
            &recorded,
            source_sample_rate,
            source_channels,
        ))
    }
}

/// 从系统默认输入设备录制固定时长音频。
///
/// 这是托盘菜单测试入口使用的便捷函数。press-to-talk 场景应使用
/// `start_default_input_recording`。
pub fn record_default_input(duration: Duration) -> CoreResult<Vec<u8>> {
    record_input(duration, None)
}

/// 从指定输入设备录制固定时长音频。
///
/// `device_name` 为 `None` 时使用系统默认输入设备；指定名称找不到时回退系统默认设备，
/// 避免设备拔插后直接中断录音链路。
pub fn record_input(duration: Duration, device_name: Option<&str>) -> CoreResult<Vec<u8>> {
    let recording = start_input_recording(device_name)?;
    std::thread::sleep(duration);
    recording.stop()
}

/// 列出系统输入设备。
pub fn list_input_devices() -> CoreResult<Vec<InputDeviceInfo>> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let devices = host
        .input_devices()
        .map_err(|error| CoreError::AudioUnavailable(error.to_string()))?
        .map(|device| {
            let name = device.name().unwrap_or_else(|_| "Unknown".to_string());
            InputDeviceInfo {
                is_default: Some(name.clone()) == default_name,
                name,
            }
        })
        .collect::<Vec<_>>();
    Ok(devices)
}

/// 开始从系统默认输入设备录音。
///
/// 返回的 `ActiveRecording` 持有 CPAL stream，调用方负责在同一线程或受控 worker
/// 中调用 `stop`。
pub fn start_default_input_recording() -> CoreResult<ActiveRecording> {
    start_input_recording(None)
}

/// 开始从指定输入设备录音。
pub fn start_input_recording(device_name: Option<&str>) -> CoreResult<ActiveRecording> {
    let host = cpal::default_host();
    let device = resolve_input_device(&host, device_name)?;
    let supported_config = device
        .default_input_config()
        .map_err(|error| CoreError::AudioUnavailable(error.to_string()))?;
    let sample_format = supported_config.sample_format();
    let config: cpal::StreamConfig = supported_config.into();
    let source_sample_rate = config.sample_rate.0;
    let source_channels = usize::from(config.channels);
    if source_channels == 0 {
        return Err(CoreError::AudioUnavailable(
            "input device reports zero channels".to_string(),
        ));
    }

    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let stream_samples = Arc::clone(&samples);
    let err_fn = |error| eprintln!("Microphone input stream error: {error}");

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| append_f32_samples(data, &stream_samples),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _| append_i16_samples(data, &stream_samples),
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _| append_u16_samples(data, &stream_samples),
            err_fn,
            None,
        ),
        other => {
            return Err(CoreError::AudioUnavailable(format!(
                "unsupported input sample format: {other:?}"
            )));
        }
    }
    .map_err(|error| CoreError::AudioUnavailable(error.to_string()))?;

    stream
        .play()
        .map_err(|error| CoreError::AudioUnavailable(error.to_string()))?;
    Ok(ActiveRecording {
        stream,
        samples,
        source_sample_rate,
        source_channels,
    })
}

/// 开始从系统默认输入设备实时产出 16kHz mono s16le PCM 分片。
///
/// `on_pcm` 会在 CPAL 音频回调线程中被调用，调用方应避免阻塞；典型用法是把分片
/// 立即发送到无界 channel，由 ASR 任务异步上传。
pub fn start_default_input_streaming(
    on_pcm: impl FnMut(Vec<u8>) + Send + 'static,
) -> CoreResult<ActiveStreamRecording> {
    start_input_streaming(None, on_pcm)
}

/// 开始从指定输入设备实时产出 16kHz mono s16le PCM 分片。
pub fn start_input_streaming(
    device_name: Option<&str>,
    on_pcm: impl FnMut(Vec<u8>) + Send + 'static,
) -> CoreResult<ActiveStreamRecording> {
    let host = cpal::default_host();
    let device = resolve_input_device(&host, device_name)?;
    let supported_config = device
        .default_input_config()
        .map_err(|error| CoreError::AudioUnavailable(error.to_string()))?;
    let sample_format = supported_config.sample_format();
    let config: cpal::StreamConfig = supported_config.into();
    let source_sample_rate = config.sample_rate.0;
    let source_channels = usize::from(config.channels);
    if source_channels == 0 {
        return Err(CoreError::AudioUnavailable(
            "input device reports zero channels".to_string(),
        ));
    }

    let on_pcm = Arc::new(Mutex::new(on_pcm));
    let err_fn = |error| eprintln!("Microphone input stream error: {error}");

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let on_pcm = Arc::clone(&on_pcm);
            device.build_input_stream(
                &config,
                move |data: &[f32], _| {
                    emit_stream_pcm(
                        convert_to_16k_mono_s16le(data, source_sample_rate, source_channels),
                        &on_pcm,
                    )
                },
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let on_pcm = Arc::clone(&on_pcm);
            device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    let samples = data
                        .iter()
                        .map(|sample| f32::from(*sample) / 32768.0)
                        .collect::<Vec<_>>();
                    emit_stream_pcm(
                        convert_to_16k_mono_s16le(&samples, source_sample_rate, source_channels),
                        &on_pcm,
                    )
                },
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let on_pcm = Arc::clone(&on_pcm);
            device.build_input_stream(
                &config,
                move |data: &[u16], _| {
                    let samples = data
                        .iter()
                        .map(|sample| (f32::from(*sample) - 32768.0) / 32768.0)
                        .collect::<Vec<_>>();
                    emit_stream_pcm(
                        convert_to_16k_mono_s16le(&samples, source_sample_rate, source_channels),
                        &on_pcm,
                    )
                },
                err_fn,
                None,
            )
        }
        other => {
            return Err(CoreError::AudioUnavailable(format!(
                "unsupported input sample format: {other:?}"
            )));
        }
    }
    .map_err(|error| CoreError::AudioUnavailable(error.to_string()))?;

    stream
        .play()
        .map_err(|error| CoreError::AudioUnavailable(error.to_string()))?;
    Ok(ActiveStreamRecording { stream })
}

fn resolve_input_device(host: &cpal::Host, device_name: Option<&str>) -> CoreResult<cpal::Device> {
    let selected = device_name.map(str::trim).filter(|name| !name.is_empty());
    if let Some(selected) = selected {
        if let Ok(mut devices) = host.input_devices() {
            if let Some(device) =
                devices.find(|device| device.name().map(|name| name == selected).unwrap_or(false))
            {
                return Ok(device);
            }
        }
    }

    host.default_input_device()
        .ok_or_else(|| CoreError::AudioUnavailable("no default input device".to_string()))
}

/// 收集 f32 输入样本，并限制在正常音频幅度范围内。
fn append_f32_samples(data: &[f32], samples: &Arc<Mutex<Vec<f32>>>) {
    if let Ok(mut samples) = samples.lock() {
        samples.extend(data.iter().map(|sample| sample.clamp(-1.0, 1.0)));
    }
}

/// 将 i16 输入样本归一化到 f32，方便统一重采样。
fn append_i16_samples(data: &[i16], samples: &Arc<Mutex<Vec<f32>>>) {
    if let Ok(mut samples) = samples.lock() {
        samples.extend(data.iter().map(|sample| f32::from(*sample) / 32768.0));
    }
}

/// 将 unsigned PCM 输入转为中心在 0 的 f32 样本。
fn append_u16_samples(data: &[u16], samples: &Arc<Mutex<Vec<f32>>>) {
    if let Ok(mut samples) = samples.lock() {
        samples.extend(
            data.iter()
                .map(|sample| (f32::from(*sample) - 32768.0) / 32768.0),
        );
    }
}

/// 将实时 PCM 分片交给调用方。
fn emit_stream_pcm(pcm: Vec<u8>, on_pcm: &Arc<Mutex<impl FnMut(Vec<u8>) + Send + 'static>>) {
    if pcm.is_empty() {
        return;
    }
    if let Ok(mut on_pcm) = on_pcm.lock() {
        on_pcm(pcm);
    }
}

/// 下混、重采样并编码为豆包 ASR 期望的 PCM 字节。
fn convert_to_16k_mono_s16le(samples: &[f32], source_sample_rate: u32, channels: usize) -> Vec<u8> {
    let mono = downmix_to_mono(samples, channels);
    let resampled = resample_linear(
        &mono,
        source_sample_rate,
        AudioFormat::PCM_16K_MONO.sample_rate_hz,
    );
    f32_to_s16le_bytes(&resampled)
}

/// 简单平均下混到 mono。
fn downmix_to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    samples
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// 线性重采样。
///
/// MVP 选择该实现是为了减少依赖和复杂度；后续如需更高音质可替换为专用 resampler。
fn resample_linear(samples: &[f32], source_sample_rate: u32, target_sample_rate: u32) -> Vec<f32> {
    if samples.is_empty() || source_sample_rate == target_sample_rate {
        return samples.to_vec();
    }

    let ratio = source_sample_rate as f64 / target_sample_rate as f64;
    let output_len = (samples.len() as f64 / ratio).round().max(1.0) as usize;
    let mut output = Vec::with_capacity(output_len);
    for index in 0..output_len {
        let position = index as f64 * ratio;
        let left = position.floor() as usize;
        let right = (left + 1).min(samples.len() - 1);
        let fraction = (position - left as f64) as f32;
        let sample = samples[left] * (1.0 - fraction) + samples[right] * fraction;
        output.push(sample);
    }
    output
}

/// 将归一化 f32 样本写为小端 i16 PCM。
fn f32_to_s16le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        let sample = sample.clamp(-1.0, 1.0);
        let scaled = if sample < 0.0 {
            sample * 32768.0
        } else {
            sample * 32767.0
        };
        bytes.extend_from_slice(&(scaled as i16).to_le_bytes());
    }
    bytes
}
