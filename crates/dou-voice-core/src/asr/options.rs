/// 一次性 PCM 转写参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmTranscribeOptions {
    /// 每次发送到 WebSocket 的字节数，必须按 i16 样本对齐。
    pub chunk_bytes: usize,
    /// 分片之间的发送间隔，用于模拟实时录音输入。
    pub chunk_delay_ms: u64,
    /// 音频结束后追加的静音时长。
    ///
    /// VoiceGenie 使用 `EndASR` 显式结束，不依赖该值；旧 raw PCM 一次性路径会把静音
    /// 合并到同一个 binary message 末尾，避免旧协议分片发送。
    pub tail_silence_ms: u64,
    /// 接收服务端事件的超时时间。
    pub receive_timeout_ms: u64,
    /// 实时输入结束后继续等待最终结果的兜底时间。
    pub post_input_receive_timeout_ms: u64,
}

impl Default for PcmTranscribeOptions {
    fn default() -> Self {
        Self {
            chunk_bytes: 5_120,
            chunk_delay_ms: 160,
            tail_silence_ms: 800,
            receive_timeout_ms: 10_000,
            post_input_receive_timeout_ms: 2_000,
        }
    }
}
