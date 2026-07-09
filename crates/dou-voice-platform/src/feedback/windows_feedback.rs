use windows::Win32::Media::Audio::SND_FLAGS;
use windows::Win32::Media::Audio::{PlaySoundA, SND_MEMORY, SND_NODEFAULT};

use super::FeedbackSound;

const START_WAV: &[u8] = include_bytes!("../../assets/sounds/voice-start.wav");
const COMPLETE_WAV: &[u8] = include_bytes!("../../assets/sounds/voice-complete.wav");
const ERROR_WAV: &[u8] = include_bytes!("../../assets/sounds/voice-error.wav");

/// 使用内嵌短 WAV 表达语音输入状态。
///
/// 不回退到系统提示音，避免 Windows 默认提示音和自定义音效混用。
pub fn play_sound(sound: FeedbackSound) {
    match sound {
        FeedbackSound::Start => play_embedded_wav(START_WAV),
        FeedbackSound::Stop => {}
        FeedbackSound::Complete => play_embedded_wav(COMPLETE_WAV),
        FeedbackSound::Error => play_embedded_wav(ERROR_WAV),
    };
}

/// 后台播放内嵌 WAV，避免录音状态切换被提示音时长阻塞。
fn play_embedded_wav(wav: &'static [u8]) {
    let _handle = std::thread::spawn(move || {
        let _ = play_wav_bytes(wav);
    });
}

/// 调用 Win32 PlaySound 播放内存中的 WAV 数据。
fn play_wav_bytes(wav: &'static [u8]) -> bool {
    unsafe {
        // SAFETY: The pointer points to static WAV bytes included in the binary.
        PlaySoundA(
            windows::core::PCSTR(wav.as_ptr()),
            None,
            SND_FLAGS(SND_MEMORY.0 | SND_NODEFAULT.0),
        )
        .as_bool()
    }
}
