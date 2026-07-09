/// ASR WebSocket 生命周期和识别结果事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsrEvent {
    /// WebSocket 已连接。
    Opened,
    /// 实时 PCM 输入已经结束。
    InputEnded {
        /// 已发送到 ASR WebSocket 的 binary frame 数量。
        chunks: usize,
        /// 已发送的实时 PCM 字节数。
        bytes: usize,
    },
    /// 尾部静音已经发送完成。该事件仅保留给兼容旧诊断输出。
    TailSilenceSent {
        /// 已发送的静音分片数量。
        chunks: usize,
        /// 已发送的静音字节数。
        bytes: usize,
        /// 静音覆盖的目标时长。
        duration_ms: u64,
    },
    /// 实时输入结束后正在等待服务端最终事件。
    WaitingForServer,
    /// 服务端在兜底等待时间内没有继续返回事件。
    ReceiveTimeout {
        /// 超时时已经收到的事件数量。
        events: usize,
        /// 本次等待使用的超时时间。
        timeout_ms: u64,
    },
    /// 服务端关闭了 WebSocket。
    SocketClosed,
    /// 服务端返回的中间识别结果。
    Partial(String),
    /// 服务端返回的最终识别结果。
    Final(String),
    /// 服务端确认本轮识别结束。
    Finished,
    /// Cookie 或设备标识失效，需要重新登录。
    AuthExpired,
    /// 服务端返回的非认证类错误。
    Error(String),
}

/// 从 ASR 事件中合成最终转写文本。
///
/// VoiceGenie 会在正常结束时返回 final 文本；该文本优先级最高。只有没有 final 时，
/// 才使用 partial 合成兜底，避免超时或协议异常时完全丢失识别内容。
pub fn transcript_text_from_events(events: &[AsrEvent]) -> Option<String> {
    if let Some(final_text) = events.iter().rev().find_map(|event| match event {
        AsrEvent::Final(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        _ => None,
    }) {
        return Some(final_text);
    }

    let mut transcript = TranscriptAccumulator::default();
    for event in events {
        if let AsrEvent::Partial(text) = event {
            transcript.update(text);
        }
    }
    transcript.finish()
}

#[derive(Debug, Default)]
struct TranscriptAccumulator {
    segments: Vec<String>,
    current: Option<String>,
}

impl TranscriptAccumulator {
    fn update(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }

        let Some(current) = self.current.as_ref() else {
            self.current = Some(text.to_string());
            return;
        };

        if is_same_transcript_segment(current, text) {
            self.current = Some(text.to_string());
            return;
        }

        self.commit_current();
        self.current = Some(text.to_string());
    }

    fn finish(mut self) -> Option<String> {
        self.commit_current();
        if self.segments.is_empty() {
            return None;
        }
        Some(join_transcript_segments(&self.segments))
    }

    fn commit_current(&mut self) {
        let Some(text) = self.current.take() else {
            return;
        };
        if text.is_empty() || self.segments.last() == Some(&text) {
            return;
        }
        self.segments.push(text);
    }
}

fn is_same_transcript_segment(previous: &str, next: &str) -> bool {
    if previous == next || previous.starts_with(next) || next.starts_with(previous) {
        return true;
    }

    let shared = common_prefix_chars(previous, next);
    if shared == 0 {
        return false;
    }

    let shorter = previous.chars().count().min(next.chars().count());
    shared >= 3 || shared * 2 >= shorter
}

fn common_prefix_chars(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

fn join_transcript_segments(segments: &[String]) -> String {
    let mut output = String::new();
    for segment in segments {
        if segment.is_empty() {
            continue;
        }
        if !output.is_empty() && needs_segment_separator(&output, segment) {
            output.push(' ');
        }
        output.push_str(segment);
    }
    output
}

fn needs_segment_separator(previous: &str, next: &str) -> bool {
    let Some(last) = previous.chars().last() else {
        return false;
    };
    let Some(first) = next.chars().next() else {
        return false;
    };
    if last.is_whitespace() || first.is_whitespace() || is_leading_punctuation(first) {
        return false;
    }
    if is_cjk(last) && is_cjk(first) {
        return false;
    }
    !is_trailing_punctuation(last)
}

fn is_cjk(ch: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&ch)
        || ('\u{3400}'..='\u{4DBF}').contains(&ch)
        || ('\u{F900}'..='\u{FAFF}').contains(&ch)
}

fn is_leading_punctuation(ch: char) -> bool {
    matches!(
        ch,
        ',' | '.'
            | ';'
            | ':'
            | '!'
            | '?'
            | ')'
            | ']'
            | '}'
            | '，'
            | '。'
            | '；'
            | '：'
            | '！'
            | '？'
            | '）'
            | '】'
            | '」'
            | '』'
    )
}

fn is_trailing_punctuation(ch: char) -> bool {
    matches!(
        ch,
        ',' | '.'
            | ';'
            | ':'
            | '!'
            | '?'
            | '('
            | '['
            | '{'
            | '，'
            | '。'
            | '；'
            | '：'
            | '！'
            | '？'
            | '（'
            | '【'
            | '「'
            | '『'
    )
}
