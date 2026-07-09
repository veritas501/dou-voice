#![recursion_limit = "256"]

//! 跨平台豆包 ASR 客户端核心库。
//!
//! 该 crate 只放可跨平台复用的能力：豆包 ASR WebSocket、认证参数存储、音频采集
//! 与转换、核心状态机和错误模型。桌面窗口、托盘、全局热键、输入模拟等系统能力
//! 应留在平台层或 Tauri shell 中，避免把 OS 细节泄漏到核心链路。

pub mod asr;
pub mod audio;
pub mod auth;
pub mod error;
pub mod state;

pub use asr::{
    transcribe_pcm_bytes, transcribe_pcm_stream, transcribe_pcm_stream_with_events,
    transcript_text_from_events, AsrClient, AsrClientConfig, AsrEvent, PcmTranscribeOptions,
};
pub use audio::{
    list_input_devices, record_default_input, record_input, start_default_input_recording,
    start_default_input_streaming, start_input_recording, start_input_streaming, ActiveRecording,
    ActiveStreamRecording, AudioCapture, AudioFormat, InputDeviceInfo, PcmChunk,
};
pub use auth::{AuthParams, AuthParamsStore};
pub use error::{CoreError, CoreResult};
pub use state::{TranscriptionCommand, TranscriptionState, TranscriptionStateMachine};
