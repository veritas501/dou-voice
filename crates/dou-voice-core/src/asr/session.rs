use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use super::config::{
    AsrWireProtocol, VOICEGENIE_END_ASR, VOICEGENIE_SESSION_FAILED, VOICEGENIE_SESSION_STARTED,
    VOICEGENIE_START_SESSION, VOICEGENIE_START_TASK, VOICEGENIE_TASK_FAILED,
    VOICEGENIE_TASK_STARTED,
};
use super::options::PcmTranscribeOptions;
use super::parser::{parse_voicegenie_envelope, VoiceGenieEnvelope};
use super::protocol::encode_voicegenie_client_event;
use crate::{AuthParams, CoreError, CoreResult};

pub(crate) async fn start_asr_session_if_needed<S, R>(
    writer: &mut S,
    reader: &mut R,
    auth: &AuthParams,
    wire_protocol: AsrWireProtocol,
    options: &PcmTranscribeOptions,
) -> CoreResult<String>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Display,
    R: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    if wire_protocol != AsrWireProtocol::VoiceGenie {
        return Ok(Uuid::new_v4().to_string());
    }

    send_voicegenie_control_event(writer, VOICEGENIE_START_TASK, None, None).await?;
    let task_started =
        wait_for_voicegenie_event(reader, VOICEGENIE_TASK_STARTED, options.receive_timeout_ms)
            .await?;
    let task_id = task_started.task_id.ok_or_else(|| {
        CoreError::AsrConnection("VoiceGenie TaskStarted missing task_id".to_string())
    })?;

    let session_payload = voicegenie_asr_session_payload(auth);
    send_voicegenie_control_event(
        writer,
        VOICEGENIE_START_SESSION,
        Some(&session_payload),
        Some(&task_id),
    )
    .await?;
    wait_for_voicegenie_event(
        reader,
        VOICEGENIE_SESSION_STARTED,
        options.receive_timeout_ms,
    )
    .await?;

    Ok(task_id)
}

pub(crate) async fn finish_audio_input<S>(
    socket: &mut S,
    wire_protocol: AsrWireProtocol,
    task_id: &str,
) -> CoreResult<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Display,
{
    match wire_protocol {
        AsrWireProtocol::LegacyRawPcm => Ok(()),
        AsrWireProtocol::VoiceGenie => {
            send_voicegenie_control_event(socket, VOICEGENIE_END_ASR, None, Some(task_id)).await
        }
    }
}

pub(crate) async fn send_voicegenie_control_event<S>(
    socket: &mut S,
    event: &str,
    payload: Option<&str>,
    task_id: Option<&str>,
) -> CoreResult<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Display,
{
    let message = encode_voicegenie_client_event(event, payload, None, task_id, None);
    socket
        .send(Message::Binary(message.into()))
        .await
        .map_err(|error| CoreError::AsrConnection(error.to_string()))
}

async fn wait_for_voicegenie_event<S>(
    socket: &mut S,
    expected_event: &str,
    timeout_ms: u64,
) -> CoreResult<VoiceGenieEnvelope>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let wait = async {
        loop {
            let Some(message) = socket.next().await else {
                return Err(CoreError::AsrConnection(
                    "VoiceGenie WebSocket closed during session setup".to_string(),
                ));
            };
            let message = message.map_err(|error| CoreError::AsrConnection(error.to_string()))?;
            let Message::Binary(data) = message else {
                continue;
            };
            let Some(envelope) = parse_voicegenie_envelope(&data)? else {
                continue;
            };
            if matches!(
                envelope.event.as_deref(),
                Some(VOICEGENIE_SESSION_FAILED) | Some(VOICEGENIE_TASK_FAILED)
            ) {
                return Err(CoreError::AsrConnection(
                    envelope.status_text.unwrap_or_else(|| {
                        envelope
                            .event
                            .clone()
                            .unwrap_or_else(|| "VoiceGenie session failed".to_string())
                    }),
                ));
            }
            if envelope.event.as_deref() == Some(expected_event) {
                return Ok(envelope);
            }
        }
    };

    timeout(Duration::from_millis(timeout_ms), wait)
        .await
        .map_err(|_| {
            CoreError::AsrConnection(format!("timed out waiting for VoiceGenie {expected_event}"))
        })?
}

pub(crate) fn voicegenie_asr_session_payload(auth: &AuthParams) -> String {
    let asr_extra = json!({
        "use_bigasr_audio_emb": true,
        "enable_shot_audio_without_vad": true,
        "enable_asr_twopass": true,
        "minor_model": "bigasr-acllm-release-grpc-mlg-0",
        "nonstream_model": "bigasr-acllm-release-grpc-main",
        "asr_post_process_task_type": "chinese_switch_english_june_asr_post_process_model",
        "send_mq": false,
        "new_arch": true,
        "no_peak_skip_decoder": false,
        "nonstream_asr_timeout_ms": 8000,
        "bigasr_config": {
            "max_vad_accumulate_duration_ms": 15000,
            "regex_replace_key": "bigasr",
            "vad_config": { "model": "v2", "voice_max_seconds": 5 },
            "enable_context_hotword": true
        },
        "force_to_speech_ms": 10000,
        "enable_regex_sdk": true,
        "ilme_weight": 0.1,
        "reset_voice_max_seconds": 20,
        "correction_map_key": "ocean",
        "enable_text_format": true,
        "enable_reset_params": true,
        "recv_bigasr_head_timeout": 10000,
        "begin_smooth_window_ms": 500,
        "stream_model": "bigasr-acllm-release-streaming-grpc-4",
        "del_uw_words": true,
        "hotword_extractor_config": {
            "douyin_extractor": { "version": -1 },
            "qishui_extractor": { "version": -1 },
            "toutiao_extractor": { "version": -1 }
        },
        "local_message_id": Uuid::new_v4().to_string(),
        "asr_context": {},
        "botid_config": {
            "7512709850661421097": {
                "model": "bigasr-acllm-release-grpc-multilingual",
                "extra": {
                    "asr_text_post_process_type": "stream_post_process",
                    "minor_model": "bigasr-acllm-release-grpc-mlg-0",
                    "enable_asr_twopass": false
                }
            }
        },
        "lm_weight": 0.2,
        "asr_post_process_third_party_config_key": "flow",
        "begin_smooth_voice_proportion": 0.5,
        "enable_function_call": false,
        "vad_namespace": "VAD_V3",
        "asr_intervention_word": "76",
        "enable_trim_punctuation": true,
        "regex_replace_key": "ocean",
        "use_bigasr_itn": true,
        "sa_session_config": {
            "session_config": {
                "show_language": true,
                "show_utterances": true,
                "additional_params": {
                    "req": { "workflow": "audio_in,resample,vad,fe,decode,itn,ddc,punc" },
                    "params": { "max_indefinite_utterance": 1 }
                },
                "hotwords": "",
                "nbest": 1
            }
        },
        "no_repeat_ngram_size": 6,
        "use_bigasr_punc": true,
        "extra_peaks_at_final": 3,
        "force_finish": false,
        "recv_req_timeout": 12000,
        "enable_context_hotword": false,
        "enable_vad_timeout_break": false,
        "dumpRate": 0,
        "voice_max_seconds": 25,
        "end_smooth_silence_proportion": 0.9,
        "check_audio_tag": 0,
        "end_smooth_window_ms": 800,
        "enable_text_post_process": true,
        "asr_text_post_process_type": "last_post_process"
    });
    let asr = json!({
        "extra": asr_extra,
        "model": "bigasr-acllm-release-grpc-main",
        "adaptation_phrase_set_id": "422bed22-a3ef-48ff-b901-97b15cc30238",
        "enable_vad": true,
        "enable_punctuation": true,
        "lang": "zh",
        "audio_info": { "channel": 1, "format": "pcm", "sample_rate": 16000 },
        "enable_disfluency": true,
        "enable_itn": true,
        "hot_word_version": 3,
        "audio_src": 1
    });
    let extra = json!({
        "did": auth.device_id.as_str(),
        "sub_conv_source_message": "",
        "dump_rate": 1,
        "enable_im_poi_report": true,
        "disable_markdown_filter": true,
        "enable_asr_audio_context": false,
        "enable_section_report": true,
        "app_version": "20800",
        "sub_conv_firstmet_type": "",
        "enable_image_asr_ctx": true,
        "enable_card_simple_report": true,
        "enable_text_simple_report": true,
        "enable_rt_poi_report": false,
        "send_mq": true,
        "first_party_app_audio_asr_info": {
            "bot_id": "7234781073513644036",
            "message_id": ""
        },
        "is_server_error_retry": false,
        "os": std::env::consts::OS,
        "enable_modify_pairs_im_report": true,
        "enable_box_input_asr": false
    });

    json!({
        "business": 1,
        "enable_audio_input": true,
        "query_mode": 2,
        "interrupt_type": 0,
        "request_type": 3,
        "chat": {
            "bot_id": "7234781073513644036",
            "message_id": "",
            "conversation_id": "",
            "is_conf_fetched": false,
            "is_dora_onboarding": false,
            "new_conversation": false,
            "question_id": ""
        },
        "asr": asr,
        "tts": {
            "audio_config": null,
            "extra": null
        },
        "extra": extra
    })
    .to_string()
}
