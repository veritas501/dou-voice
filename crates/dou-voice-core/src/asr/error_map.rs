//! ASR 错误分类：把底层 WebSocket / IO 错误转成更直白的英文用户提示。
//!
//! 分类优先级：
//! 1. HTTP 握手状态（401/403 → 认证，5xx → 服务端）
//! 2. IO / DNS / timeout 关键词
//! 3. TLS / 协议错误
//! 4. 兜底保留原始错误文本

use std::fmt::Display;
use std::io::ErrorKind;

use tokio_tungstenite::tungstenite::Error as WsError;

use crate::CoreError;

/// 把任意可显示错误包装为 ASR 连接错误，并尽量归类。
pub(crate) fn asr_connection_error(context: &str, error: impl Display) -> CoreError {
    classify_message(context, &error.to_string())
}

/// 把 tungstenite WebSocket 错误包装为 ASR 连接错误。
pub(crate) fn asr_ws_error(context: &str, error: &WsError) -> CoreError {
    match error {
        WsError::Http(response) => {
            let status = response.status();
            let code = status.as_u16();
            let reason = status.canonical_reason().unwrap_or("unknown");
            return match code {
                401 | 403 => CoreError::AuthExpired,
                404 => CoreError::AsrConnection(format!(
                    "{context}: ASR endpoint not found (HTTP {code} {reason}). The Doubao WebSocket URL may have changed."
                )),
                429 => CoreError::AsrConnection(format!(
                    "{context}: ASR rate limited (HTTP {code}). Wait a moment and try again."
                )),
                500..=599 => CoreError::AsrConnection(format!(
                    "{context}: ASR server error (HTTP {code} {reason}). Try again later."
                )),
                _ => CoreError::AsrConnection(format!(
                    "{context}: ASR handshake rejected (HTTP {code} {reason})"
                )),
            };
        }
        WsError::Io(io_error) => {
            return classify_io(context, io_error);
        }
        WsError::Tls(tls_error) => {
            return CoreError::AsrConnection(format!(
                "{context}: TLS handshake failed ({tls_error}). Check system time, proxy, or network interception."
            ));
        }
        WsError::Url(url_error) => {
            return CoreError::AsrConnection(format!(
                "{context}: invalid ASR WebSocket URL ({url_error})"
            ));
        }
        WsError::Protocol(protocol_error) => {
            return CoreError::AsrConnection(format!(
                "{context}: WebSocket protocol error ({protocol_error})"
            ));
        }
        WsError::ConnectionClosed => {
            return CoreError::AsrConnection(format!(
                "{context}: ASR WebSocket closed by the server"
            ));
        }
        WsError::AlreadyClosed => {
            return CoreError::AsrConnection(format!(
                "{context}: ASR WebSocket was already closed"
            ));
        }
        _ => {}
    }

    classify_message(context, &error.to_string())
}

fn classify_io(context: &str, error: &std::io::Error) -> CoreError {
    match error.kind() {
        ErrorKind::TimedOut => CoreError::AsrConnection(format!(
            "{context}: network timed out while contacting ASR. Check your internet connection."
        )),
        ErrorKind::ConnectionRefused => CoreError::AsrConnection(format!(
            "{context}: connection refused by ASR host. Check network/firewall settings."
        )),
        ErrorKind::ConnectionReset => CoreError::AsrConnection(format!(
            "{context}: connection reset while talking to ASR"
        )),
        ErrorKind::NotConnected => CoreError::AsrConnection(format!(
            "{context}: socket is not connected to ASR"
        )),
        ErrorKind::BrokenPipe => CoreError::AsrConnection(format!(
            "{context}: connection broken while sending audio to ASR"
        )),
        ErrorKind::UnexpectedEof => CoreError::AsrConnection(format!(
            "{context}: ASR closed the connection unexpectedly"
        )),
        _ => {
            // DNS / host lookup often surfaces as custom or uncategorized IO errors.
            classify_message(context, &error.to_string())
        }
    }
}

fn classify_message(context: &str, raw: &str) -> CoreError {
    let lower = raw.to_ascii_lowercase();

    // Explicit auth/session failures map to AuthExpired so UI can prompt re-login.
    if contains_any(
        &lower,
        &[
            "401",
            "403",
            "unauthorized",
            "forbidden",
            "cookie",
            "login required",
            "auth expired",
            "session expired",
        ],
    ) {
        return CoreError::AuthExpired;
    }

    if contains_any(
        &lower,
        &[
            "failed to lookup address",
            "name or service not known",
            "no such host",
            "nodename nor servname",
            "dns error",
            "getaddrinfo",
            "temporary failure in name resolution",
        ],
    ) {
        return CoreError::AsrConnection(format!(
            "{context}: DNS lookup failed for the ASR host. Check DNS/network settings. ({raw})"
        ));
    }

    if contains_any(
        &lower,
        &[
            "timed out",
            "timeout",
            "deadline has elapsed",
            "operation timed out",
        ],
    ) {
        return CoreError::AsrConnection(format!(
            "{context}: timed out waiting for ASR. Check network latency or try again. ({raw})"
        ));
    }

    if contains_any(
        &lower,
        &[
            "connection refused",
            "network is unreachable",
            "no route to host",
            "host is down",
            "software caused connection abort",
        ],
    ) {
        return CoreError::AsrConnection(format!(
            "{context}: cannot reach ASR host. Check internet/firewall/proxy. ({raw})"
        ));
    }

    if contains_any(&lower, &["certificate", "tls", "ssl", "handshake failure"]) {
        return CoreError::AsrConnection(format!(
            "{context}: TLS/certificate error while connecting to ASR. ({raw})"
        ));
    }

    if contains_any(
        &lower,
        &[
            "protocol error",
            "invalid opcode",
            "unexpected continuation",
            "mask",
            "payload too large",
            "httparse",
        ],
    ) {
        return CoreError::AsrConnection(format!(
            "{context}: ASR protocol/parse error. ({raw})"
        ));
    }

    CoreError::AsrConnection(format!("{context}: {raw}"))
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::{asr_connection_error, classify_message};
    use crate::CoreError;

    #[test]
    fn classifies_dns_failures() {
        let error = asr_connection_error(
            "Connect ASR",
            "failed to lookup address information: Name or service not known",
        );
        let text = error.to_string();
        assert!(text.contains("DNS lookup failed"), "{text}");
    }

    #[test]
    fn classifies_timeouts() {
        let error = classify_message("Wait ASR result", "operation timed out");
        let text = error.to_string();
        assert!(text.contains("timed out"), "{text}");
    }

    #[test]
    fn classifies_auth_keywords_as_expired() {
        let error = classify_message("ASR handshake", "HTTP error: 401 Unauthorized");
        assert!(matches!(error, CoreError::AuthExpired), "{error}");
    }

    #[test]
    fn keeps_context_in_fallback() {
        let error = asr_connection_error("Send PCM frame", "something obscure");
        let text = error.to_string();
        assert!(text.contains("Send PCM frame"), "{text}");
        assert!(text.contains("something obscure"), "{text}");
    }
}
