use super::FeedbackSound;

/// macOS/Linux 提示音实现。
///
/// 使用和 Windows 同一套内置 wav 资源，通过 rodio 默认输出设备异步播放。失败时静默
/// 忽略，避免音频设备不可用影响主录音/输入链路。
pub fn play_sound(sound: FeedbackSound) {
    let data = sound_data(sound);
    std::thread::spawn(move || {
        let cursor = std::io::Cursor::new(data);
        let Ok(stream) = rodio::OutputStreamBuilder::open_default_stream() else {
            return;
        };
        let Ok(sink) = rodio::play(stream.mixer(), cursor) else {
            return;
        };
        sink.sleep_until_end();
    });
}

fn sound_data(sound: FeedbackSound) -> &'static [u8] {
    match sound {
        FeedbackSound::Start => include_bytes!("../../assets/sounds/voice-start.wav"),
        FeedbackSound::Stop => include_bytes!("../../assets/sounds/voice-stop.wav"),
        FeedbackSound::Complete => include_bytes!("../../assets/sounds/voice-complete.wav"),
        FeedbackSound::Error => include_bytes!("../../assets/sounds/voice-error.wav"),
    }
}
