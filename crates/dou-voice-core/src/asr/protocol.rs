use prost::Message;

use super::config::{
    AsrWireProtocol, VOICEGENIE_APP_KEY, VOICEGENIE_NAMESPACE, VOICEGENIE_TASK_REQUEST,
};

pub(crate) mod pbws {
    include!(concat!(env!("OUT_DIR"), "/pbws.rs"));
}

pub(crate) fn encode_client_audio_frame(
    pcm: Vec<u8>,
    wire_protocol: AsrWireProtocol,
    task_id: &str,
    sequence: u64,
) -> Vec<u8> {
    match wire_protocol {
        AsrWireProtocol::LegacyRawPcm => pcm,
        AsrWireProtocol::VoiceGenie => encode_voicegenie_task_request(pcm, task_id, sequence),
    }
}

pub(crate) fn encode_voicegenie_task_request(
    pcm: Vec<u8>,
    task_id: &str,
    sequence: u64,
) -> Vec<u8> {
    encode_voicegenie_client_event(
        VOICEGENIE_TASK_REQUEST,
        None,
        Some(pcm),
        Some(task_id),
        Some(sequence),
    )
}

pub(crate) fn encode_voicegenie_client_event(
    event: &str,
    payload: Option<&str>,
    data: Option<Vec<u8>>,
    task_id: Option<&str>,
    sequence: Option<u64>,
) -> Vec<u8> {
    let request = pbws::WebSocketRequest {
        token: String::new(),
        appkey: VOICEGENIE_APP_KEY.to_string(),
        namespace: VOICEGENIE_NAMESPACE.to_string(),
        version: String::new(),
        event: event.to_string(),
        payload: Some(payload.unwrap_or("{}").to_string()),
        data: data.unwrap_or_default(),
        task_id: task_id.unwrap_or_default().to_string(),
        seq_id: sequence,
    };
    let mut output = Vec::with_capacity(request.encoded_len());
    // Encoding into a growable Vec should not fail; avoid panicking in the ASR path.
    if let Err(error) = request.encode(&mut output) {
        // Soft fallback: retry once with a fresh buffer. Still never panic here.
        output.clear();
        if request.encode(&mut output).is_err() {
            eprintln!("VoiceGenie protobuf encode failed: {error}");
            return Vec::new();
        }
    }
    output
}
