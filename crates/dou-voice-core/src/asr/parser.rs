use prost::Message;
use serde::Deserialize;

use super::config::{
    VOICEGENIE_ASR_ENDED, VOICEGENIE_ASR_RESPONSE, VOICEGENIE_NAMESPACE, VOICEGENIE_SESSION_FAILED,
    VOICEGENIE_TASK_FAILED,
};
use super::event::AsrEvent;
use super::protocol::pbws;
use crate::CoreResult;

const AUTH_ERROR_CODE: i64 = 709_599_054;
const AUTH_ERROR_KEYWORDS: &[&str] = &[
    "cookie",
    "auth",
    "login",
    "session",
    "unauthorized",
    "expired",
];

pub(crate) fn parse_binary_server_event(data: &[u8]) -> CoreResult<Option<AsrEvent>> {
    if !data
        .windows(VOICEGENIE_NAMESPACE.len())
        .any(|window| window == VOICEGENIE_NAMESPACE.as_bytes())
        && !data
            .windows(VOICEGENIE_ASR_RESPONSE.len())
            .any(|window| window == VOICEGENIE_ASR_RESPONSE.as_bytes())
    {
        return Ok(None);
    }

    let Some(envelope) = parse_voicegenie_envelope(data)? else {
        return Ok(None);
    };
    voicegenie_envelope_to_event(envelope)
}

pub(crate) fn parse_voicegenie_envelope(data: &[u8]) -> CoreResult<Option<VoiceGenieEnvelope>> {
    let Ok(response) = pbws::WebSocketResponse::decode(data) else {
        return Ok(None);
    };
    let envelope = VoiceGenieEnvelope {
        task_id: non_empty_string(response.task_id),
        message_id: non_empty_string(response.message_id),
        namespace: non_empty_string(response.namespace),
        event: non_empty_string(response.event),
        status_code: (response.status_code != 0).then_some(response.status_code),
        status_text: non_empty_string(response.status_text),
        payload: response.payload,
        data: (!response.data.is_empty()).then_some(response.data),
        seq_id: response.seq_id,
    };

    if envelope.namespace.as_deref() != Some(VOICEGENIE_NAMESPACE) {
        return Ok(None);
    }
    Ok(Some(envelope))
}

fn non_empty_string(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn voicegenie_envelope_to_event(envelope: VoiceGenieEnvelope) -> CoreResult<Option<AsrEvent>> {
    if let Some(status) = envelope.status_text.as_deref() {
        if status != "OK" {
            return Ok(Some(AsrEvent::Error(status.to_string())));
        }
    }

    match envelope.event.as_deref() {
        Some(VOICEGENIE_ASR_RESPONSE) => {
            let Some(payload) = envelope.payload else {
                return Ok(None);
            };
            parse_voicegenie_payload(payload.as_bytes())
        }
        Some(VOICEGENIE_ASR_ENDED) => Ok(Some(AsrEvent::Finished)),
        Some(VOICEGENIE_SESSION_FAILED) | Some(VOICEGENIE_TASK_FAILED) => {
            Ok(Some(AsrEvent::Error(envelope.status_text.unwrap_or_else(
                || "VoiceGenie session failed".to_string(),
            ))))
        }
        _ => Ok(None),
    }
}

fn parse_voicegenie_payload(payload: &[u8]) -> CoreResult<Option<AsrEvent>> {
    if payload.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Ok(None);
    }
    let payload = serde_json::from_slice::<VoiceGeniePayload>(payload)?;
    let Some(result) = payload
        .results
        .as_ref()
        .and_then(|results| results.first())
        .filter(|result| result.text.as_deref().is_some_and(|text| !text.is_empty()))
    else {
        return Ok(None);
    };

    let text = result.text.clone().unwrap_or_default();
    let is_final = result
        .is_interim
        .map(|is_interim| !is_interim)
        .or(result.is_final)
        .or(payload.is_final)
        .unwrap_or(false);
    if is_final {
        Ok(Some(AsrEvent::Final(text)))
    } else {
        Ok(Some(AsrEvent::Partial(text)))
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct VoiceGenieEnvelope {
    pub(crate) task_id: Option<String>,
    pub(crate) message_id: Option<String>,
    pub(crate) namespace: Option<String>,
    pub(crate) event: Option<String>,
    pub(crate) status_code: Option<i32>,
    pub(crate) status_text: Option<String>,
    pub(crate) payload: Option<String>,
    pub(crate) data: Option<Vec<u8>>,
    pub(crate) seq_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct VoiceGeniePayload {
    results: Option<Vec<VoiceGenieResult>>,
    #[serde(rename = "isFinal", alias = "is_final")]
    is_final: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct VoiceGenieResult {
    text: Option<String>,
    is_interim: Option<bool>,
    #[serde(rename = "isFinal", alias = "is_final")]
    is_final: Option<bool>,
}

pub(crate) fn parse_server_event(message: &str) -> CoreResult<Option<AsrEvent>> {
    let message = message.trim();
    if !message.starts_with('{') {
        return Ok(None);
    }

    let message = serde_json::from_str::<ServerMessage>(message)?;
    let code = message.code.unwrap_or(0);
    let server_message = message.message.unwrap_or_default();

    if code != 0 {
        if code == AUTH_ERROR_CODE || is_auth_error_message(&server_message) {
            return Ok(Some(AsrEvent::AuthExpired));
        }
        return Ok(Some(AsrEvent::Error(server_message)));
    }

    match message.event.as_deref() {
        Some("result") => Ok(message
            .result
            .and_then(|result| result.text)
            .filter(|text| !text.is_empty())
            .map(AsrEvent::Partial)),
        Some("finish") => Ok(Some(AsrEvent::Finished)),
        _ => Ok(None),
    }
}

fn is_auth_error_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    AUTH_ERROR_KEYWORDS
        .iter()
        .any(|keyword| message.contains(keyword))
}

#[derive(Debug, Deserialize)]
struct ServerMessage {
    event: Option<String>,
    result: Option<ServerResult>,
    code: Option<i64>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ServerResult {
    #[serde(rename = "Text", alias = "text")]
    text: Option<String>,
}
