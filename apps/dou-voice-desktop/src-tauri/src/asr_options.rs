use dou_voice_core::PcmTranscribeOptions;

/// 流式识别使用更接近实时麦克风回调的尾部静音节奏。
pub(crate) fn streaming_transcribe_options() -> PcmTranscribeOptions {
    PcmTranscribeOptions {
        chunk_bytes: 5_120,
        chunk_delay_ms: 160,
        tail_silence_ms: 800,
        receive_timeout_ms: 10_000,
        post_input_receive_timeout_ms: 2_000,
    }
}
