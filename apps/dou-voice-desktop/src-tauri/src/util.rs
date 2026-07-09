/// 生成一次 localStorage 捕获请求的轻量 ID。
pub(crate) fn uuid_like_request_id() -> String {
    format!("{}-{}", std::process::id(), unix_time_ms())
}

/// 返回当前 Unix 时间戳，单位毫秒。
pub(crate) fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

/// 限制调试日志中的文本长度，避免 Activity 被长文本撑爆。
pub(crate) fn text_preview(text: &str) -> String {
    const MAX_CHARS: usize = 160;
    let mut preview = text.chars().take(MAX_CHARS).collect::<String>();
    if text.chars().count() > MAX_CHARS {
        preview.push_str("...");
    }
    preview
}
