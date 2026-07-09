#[cfg(windows)]
mod windows_feedback;

#[cfg(not(windows))]
mod unsupported_feedback;

#[cfg(not(windows))]
use unsupported_feedback as platform;
#[cfg(windows)]
use windows_feedback as platform;

/// 语音输入生命周期中的系统反馈音事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackSound {
    /// 开始录音。
    Start,
    /// 停止录音并进入识别。
    Stop,
    /// 文本输入完成。
    Complete,
    /// 录音、识别或输入失败。
    Error,
}

/// 播放平台系统提示音。
///
/// 当前 Windows 播放内嵌短 WAV；其他平台暂时 no-op，
/// 后续适配时再接入各自原生提示音或可配置音效。
pub fn play_sound(sound: FeedbackSound) {
    platform::play_sound(sound);
}
