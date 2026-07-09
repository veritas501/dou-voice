use super::{
    encode_query_value, parse_server_event,
    protocol::pbws::{WebSocketRequest, WebSocketResponse},
    AsrClientConfig, AsrEvent, PcmTranscribeOptions,
};
use crate::AuthParams;
use prost::Message;
use std::collections::BTreeMap;

fn encode_voicegenie_response(response: WebSocketResponse) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(response.encoded_len());
    response
        .encode(&mut encoded)
        .expect("encoding VoiceGenie response to Vec cannot fail");
    encoded
}

#[test]
fn default_endpoint_points_to_doubao_asr() {
    let config = AsrClientConfig::default();

    assert!(config.endpoint.contains("/api/v2/sami/voicegenie"));
}

#[test]
fn builds_voicegenie_wss_url_with_required_params() {
    let config = AsrClientConfig::default();
    let auth = AuthParams {
        cookies: BTreeMap::from([("sessionid".to_string(), "abc".to_string())]),
        device_id: "device 1".to_string(),
        web_id: "web".to_string(),
        captured_at_unix_ms: 1,
    };

    let url = config.build_url(&auth, "tab").expect("build url");

    assert!(url.starts_with("wss://frontier-audio-web-ws.doubao.com/api/v2/sami/voicegenie?"));
    assert!(url.contains("api_app_key=GOqQpfo1fO7slHv8"));
    assert!(url.contains("namespace=VoiceGenie"));
    assert!(url.contains("version_code=20800"));
    assert!(url.contains("pc_version=3.26.0"));
    assert!(url.contains("device_id=device%201"));
    assert!(url.contains("web_id=web"));
    assert!(url.contains("tea_uuid=web"));
    assert!(url.contains("web_platform=browser"));
    assert!(url.contains("web_tab_id=tab"));
    assert!(!url.contains("format=pcm"));
}

#[test]
fn builds_legacy_wss_url_with_required_params() {
    let config = AsrClientConfig {
        endpoint: "wss://ws-samantha.doubao.com/samantha/audio/asr".to_string(),
        origin: "https://www.doubao.com".to_string(),
    };
    let auth = AuthParams {
        cookies: BTreeMap::from([("sessionid".to_string(), "abc".to_string())]),
        device_id: "device 1".to_string(),
        web_id: "web".to_string(),
        captured_at_unix_ms: 1,
    };

    let url = config.build_url(&auth, "tab").expect("build url");

    assert!(url.starts_with("wss://ws-samantha.doubao.com/samantha/audio/asr?"));
    assert!(url.contains("pc_version=3.12.3"));
    assert!(url.contains("format=pcm"));
}

#[test]
fn encodes_query_values() {
    assert_eq!(encode_query_value("a b+c"), "a%20b%2Bc");
}

#[test]
fn parses_result_event() {
    let event =
        parse_server_event(r#"{"event":"result","result":{"Text":"hello"},"code":0,"message":""}"#)
            .expect("parse event");

    assert_eq!(event, Some(AsrEvent::Partial("hello".to_string())));
}

#[test]
fn parses_lowercase_text_result_event() {
    let event =
        parse_server_event(r#"{"event":"result","result":{"text":"hello"},"code":0,"message":""}"#)
            .expect("parse event");

    assert_eq!(event, Some(AsrEvent::Partial("hello".to_string())));
}

#[test]
fn ignores_non_json_server_text() {
    let event = parse_server_event("\nnot-json").expect("ignore text");

    assert_eq!(event, None);
}

#[test]
fn ignores_non_voicegenie_binary_message() {
    let event = super::parse_binary_server_event(b"\nnot-json").expect("ignore binary");

    assert_eq!(event, None);
}

#[test]
fn encodes_voicegenie_task_request_envelope() {
    let pcm = vec![0_u8, 1, 2, 3];
    let encoded = super::encode_voicegenie_task_request(pcm.clone(), "task-id", 7);

    let request = WebSocketRequest::decode(encoded.as_slice()).expect("decode request");

    assert_eq!(request.appkey, "GOqQpfo1fO7slHv8");
    assert_eq!(request.namespace, "VoiceGenie");
    assert_eq!(request.event, "TaskRequest");
    assert_eq!(request.payload.as_deref(), Some("{}"));
    assert_eq!(request.data, pcm);
    assert_eq!(request.task_id, "task-id");
    assert_eq!(request.seq_id, Some(7));
}

#[test]
fn encodes_voicegenie_control_envelope() {
    let encoded = super::encode_voicegenie_client_event(
        super::VOICEGENIE_END_ASR,
        Some(r#"{"reason":"client_done"}"#),
        None,
        Some("task-id"),
        None,
    );

    let request = WebSocketRequest::decode(encoded.as_slice()).expect("decode request");

    assert_eq!(request.event, "EndASR");
    assert_eq!(
        request.payload.as_deref(),
        Some(r#"{"reason":"client_done"}"#)
    );
    assert_eq!(request.task_id, "task-id");
    assert!(request.data.is_empty());
}

#[test]
fn parses_voicegenie_task_started_envelope() {
    let data = vec![9_u8, 8, 7];
    let encoded = encode_voicegenie_response(WebSocketResponse {
        task_id: "task-id".to_string(),
        message_id: "response-id".to_string(),
        namespace: "VoiceGenie".to_string(),
        event: "TaskStarted".to_string(),
        status_code: 20_000_000,
        status_text: "OK".to_string(),
        payload: Some(r#"{"state":"started"}"#.to_string()),
        data: data.clone(),
        seq_id: Some(9),
    });

    let envelope = super::parse_voicegenie_envelope(&encoded)
        .expect("parse envelope")
        .expect("envelope");

    assert_eq!(envelope.task_id.as_deref(), Some("task-id"));
    assert_eq!(envelope.message_id.as_deref(), Some("response-id"));
    assert_eq!(envelope.namespace.as_deref(), Some("VoiceGenie"));
    assert_eq!(envelope.event.as_deref(), Some("TaskStarted"));
    assert_eq!(envelope.status_code, Some(20_000_000));
    assert_eq!(envelope.status_text.as_deref(), Some("OK"));
    assert_eq!(envelope.payload.as_deref(), Some(r#"{"state":"started"}"#));
    assert_eq!(envelope.data.as_deref(), Some(data.as_slice()));
    assert_eq!(envelope.seq_id, Some(9));
}

#[test]
fn parses_voicegenie_asr_response_envelope() {
    let encoded = encode_voicegenie_response(WebSocketResponse {
        task_id: "request-id".to_string(),
        message_id: "response-id".to_string(),
        namespace: "VoiceGenie".to_string(),
        event: "ASRResponse".to_string(),
        status_code: 20_000_000,
        status_text: "OK".to_string(),
        payload: Some(r#"{"results":[{"text":"Hello hello","is_interim":true}]}"#.to_string()),
        data: Vec::new(),
        seq_id: Some(10),
    });

    let event = super::parse_binary_server_event(&encoded).expect("parse envelope");

    assert_eq!(event, Some(AsrEvent::Partial("Hello hello".to_string())));
}

#[test]
fn parses_voicegenie_final_asr_response_envelope() {
    let encoded = encode_voicegenie_response(WebSocketResponse {
        task_id: "task-id".to_string(),
        message_id: "response-id".to_string(),
        namespace: "VoiceGenie".to_string(),
        event: "ASRResponse".to_string(),
        status_code: 20_000_000,
        status_text: "OK".to_string(),
        payload: Some(r#"{"results":[{"text":"final text","is_interim":false}]}"#.to_string()),
        data: Vec::new(),
        seq_id: Some(11),
    });

    let event = super::parse_binary_server_event(&encoded).expect("parse envelope");

    assert_eq!(event, Some(AsrEvent::Final("final text".to_string())));
}

#[test]
fn builds_voicegenie_asr_session_payload_with_web_asr_config() {
    let auth = AuthParams {
        cookies: BTreeMap::from([("sessionid".to_string(), "abc".to_string())]),
        device_id: "device-id".to_string(),
        web_id: "web-id".to_string(),
        captured_at_unix_ms: 1,
    };

    let payload = super::voicegenie_asr_session_payload(&auth);

    assert!(payload.contains(r#""request_type":3"#));
    assert!(payload.contains(r#""format":"pcm""#));
    assert!(payload.contains(r#""sample_rate":16000"#));
    assert!(payload.contains(r#""stream_model":"bigasr-acllm-release-streaming-grpc-4""#));
    assert!(payload.contains(r#""end_smooth_silence_proportion":0.9"#));
    assert!(payload.contains(r#""did":"device-id""#));
}

#[test]
fn parses_finish_event() {
    let event = parse_server_event(r#"{"event":"finish","result":null,"code":0,"message":""}"#)
        .expect("parse event");

    assert_eq!(event, Some(AsrEvent::Finished));
}

#[test]
fn detects_auth_error_event() {
    let event = parse_server_event(
        r#"{"event":"error","result":null,"code":709599054,"message":"Invalid cookie"}"#,
    )
    .expect("parse event");

    assert_eq!(event, Some(AsrEvent::AuthExpired));
}

#[test]
fn transcript_accumulates_when_partial_resets_to_new_segment() {
    let events = vec![
        AsrEvent::Opened,
        AsrEvent::Partial("我也把".to_string()),
        AsrEvent::Partial("我也把".to_string()),
        AsrEvent::Partial("PC?".to_string()),
        AsrEvent::Partial("PCM.".to_string()),
        AsrEvent::Partial("PCM string.".to_string()),
        AsrEvent::Partial("PCM string with event.".to_string()),
    ];

    let text = super::transcript_text_from_events(&events);

    assert_eq!(text, Some("我也把 PCM string with event.".to_string()));
}

#[test]
fn transcript_keeps_corrections_inside_same_segment() {
    let events = vec![
        AsrEvent::Partial("你好介绍".to_string()),
        AsrEvent::Partial("你好，介绍".to_string()),
        AsrEvent::Partial("你好，介绍一下你自己。".to_string()),
    ];

    let text = super::transcript_text_from_events(&events);

    assert_eq!(text, Some("你好，介绍一下你自己。".to_string()));
}

#[test]
fn transcript_prefers_final_when_partial_only_differs_by_punctuation() {
    let events = vec![
        AsrEvent::Partial("好的继续".to_string()),
        AsrEvent::Partial("好的继续推荐".to_string()),
        AsrEvent::Partial("好的继续推进下一步".to_string()),
        AsrEvent::Final("好的，继续推进下一步。".to_string()),
    ];

    let text = super::transcript_text_from_events(&events);

    assert_eq!(text, Some("好的，继续推进下一步。".to_string()));
}

#[test]
fn transcript_prefers_final_when_partial_prefix_was_wrong() {
    let events = vec![
        AsrEvent::Partial("织实一下这个 bug".to_string()),
        AsrEvent::Partial("Fix 一下这个 bug".to_string()),
        AsrEvent::Final("Fix 一下这个 bug。".to_string()),
    ];

    let text = super::transcript_text_from_events(&events);

    assert_eq!(text, Some("Fix 一下这个 bug。".to_string()));
}

#[test]
fn transcript_prefers_final_over_joined_partial_segments() {
    let events = vec![
        AsrEvent::Partial("老的那套 ASR".to_string()),
        AsrEvent::Partial("是不支持分片传输的".to_string()),
        AsrEvent::Final("老的那套 ASR 是不支持分片传输的。".to_string()),
    ];

    let text = super::transcript_text_from_events(&events);

    assert_eq!(text, Some("老的那套 ASR 是不支持分片传输的。".to_string()));
}

#[test]
fn rejects_odd_chunk_size() {
    let options = PcmTranscribeOptions {
        chunk_bytes: 3,
        ..PcmTranscribeOptions::default()
    };

    assert!(super::validate_pcm_options(&options).is_err());
}
