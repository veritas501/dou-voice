use futures_util::{SinkExt, StreamExt};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    oneshot,
};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use uuid::Uuid;

use crate::{AuthParams, CoreError, CoreResult, PcmChunk};
mod config;
mod error_map;
mod event;
mod options;
mod parser;
mod protocol;
mod receive;
mod send;
mod session;

pub use config::AsrClientConfig;
#[cfg(test)]
use config::{encode_query_value, VOICEGENIE_END_ASR};
use config::{AsrWireProtocol, VOICEGENIE_FINISH_SESSION};
use error_map::{asr_connection_error, asr_ws_error};
pub use event::{transcript_text_from_events, AsrEvent};
pub use options::PcmTranscribeOptions;
#[cfg(test)]
use parser::{parse_binary_server_event, parse_server_event, parse_voicegenie_envelope};
#[cfg(test)]
use protocol::{encode_voicegenie_client_event, encode_voicegenie_task_request};
use receive::{emit_observer_event, receive_asr_events};
use send::{send_legacy_pcm_once, send_pcm_chunks, send_pcm_stream};
#[cfg(test)]
use session::voicegenie_asr_session_payload;
use session::{finish_audio_input, send_voicegenie_control_event, start_asr_session_if_needed};

/// ASR 客户端抽象。
///
/// 当前生产路径使用 `transcribe_pcm_bytes` 的一次性实现；该 trait 保留给后续流式状态机接入。
pub trait AsrClient {
    /// 连接 ASR 服务。
    fn connect(&mut self, auth: &AuthParams) -> CoreResult<()>;

    /// 发送一段 PCM 音频。
    fn send_audio(&mut self, chunk: PcmChunk) -> CoreResult<()>;

    /// 通知服务端当前音频输入结束。
    fn finish(&mut self) -> CoreResult<()>;

    /// 断开连接并释放资源。
    fn disconnect(&mut self) -> CoreResult<()>;
}

/// 将完整的 16kHz mono s16le PCM 音频发送到豆包 ASR 并收集事件。
///
/// 这是当前桌面端使用的核心链路。VoiceGenie 会按 Web 端协议先建立 ASR
/// session，再发送音频并用 `EndASR` 显式结束；旧 raw PCM fallback 只发送单个
/// binary message，不做分片传输。
pub async fn transcribe_pcm_bytes(
    config: &AsrClientConfig,
    auth: &AuthParams,
    pcm: &[u8],
    options: &PcmTranscribeOptions,
) -> CoreResult<Vec<AsrEvent>> {
    install_rustls_crypto_provider();
    auth.validate()?;
    validate_pcm_options(options)?;

    let web_tab_id = Uuid::new_v4().to_string();
    let url = config.build_url(auth, &web_tab_id)?;
    let wire_protocol = config.wire_protocol();
    let mut request = url
        .into_client_request()
        .map_err(|error| asr_connection_error("Build ASR WebSocket request", error))?;
    request.headers_mut().insert(
        "Cookie",
        HeaderValue::from_str(&auth.cookie_header())
            .map_err(|error| asr_connection_error("Invalid ASR Cookie header", error))?,
    );
    request.headers_mut().insert(
        "Origin",
        HeaderValue::from_str(&config.origin)
            .map_err(|error| asr_connection_error("Invalid ASR Origin header", error))?,
    );
    request.headers_mut().insert(
        "User-Agent",
        HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
        ),
    );
    request.headers_mut().insert(
        "Accept-Language",
        HeaderValue::from_static("zh-CN,zh;q=0.9"),
    );

    let (socket, _) = connect_async(request)
        .await
        .map_err(|error| asr_ws_error("Connect ASR WebSocket", &error))?;
    let (mut writer, mut reader) = socket.split();

    let mut events = vec![AsrEvent::Opened];
    let task_id =
        start_asr_session_if_needed(&mut writer, &mut reader, auth, wire_protocol, options).await?;
    let (receive_done_tx, receive_done_rx) = oneshot::channel::<()>();
    let send_result = async {
        match wire_protocol {
            AsrWireProtocol::LegacyRawPcm => {
                send_legacy_pcm_once(&mut writer, pcm, options).await?;
            }
            AsrWireProtocol::VoiceGenie => {
                let mut sequence = 1;
                send_pcm_chunks(
                    &mut writer,
                    pcm,
                    options,
                    wire_protocol,
                    &task_id,
                    &mut sequence,
                )
                .await?;
                finish_audio_input(&mut writer, wire_protocol, &task_id).await?;
                let _ = receive_done_rx.await;
                let _ = send_voicegenie_control_event(
                    &mut writer,
                    VOICEGENIE_FINISH_SESSION,
                    None,
                    Some(&task_id),
                )
                .await;
            }
        }
        let _ = writer.close().await;
        CoreResult::Ok(())
    };
    let receive_result = async {
        let result = receive_asr_events(&mut reader, options, None, false, None).await;
        let _ = receive_done_tx.send(());
        result
    };

    let (send_result, receive_result) = tokio::join!(send_result, receive_result);
    send_result?;
    events.extend(receive_result?);

    Ok(events)
}

/// 从实时 PCM 分片流发送到豆包 ASR 并收集事件。
///
/// 该路径用于桌面端 press-to-talk：麦克风回调持续推送 16kHz mono s16le PCM，
/// WebSocket 连接完成后立即按回调节奏上传。调用方关闭 `pcm_rx` 后，本函数按当前
/// 协议发送结束信号并保持 WebSocket 打开，等待服务端返回最终事件或触发超时兜底。
pub async fn transcribe_pcm_stream(
    config: &AsrClientConfig,
    auth: &AuthParams,
    pcm_rx: UnboundedReceiver<Vec<u8>>,
    options: &PcmTranscribeOptions,
) -> CoreResult<Vec<AsrEvent>> {
    transcribe_pcm_stream_inner(config, auth, pcm_rx, options, None).await
}

/// 从实时 PCM 分片流发送到豆包 ASR，并把服务端事件同步发送给观察者。
///
/// 该函数用于桌面端调试和实时状态展示。`event_tx` 只承载 ASR 生命周期事件；
/// 认证参数仍只用于 WebSocket 握手，不会通过事件暴露。
pub async fn transcribe_pcm_stream_with_events(
    config: &AsrClientConfig,
    auth: &AuthParams,
    pcm_rx: UnboundedReceiver<Vec<u8>>,
    options: &PcmTranscribeOptions,
    event_tx: UnboundedSender<AsrEvent>,
) -> CoreResult<Vec<AsrEvent>> {
    transcribe_pcm_stream_inner(config, auth, pcm_rx, options, Some(event_tx)).await
}

async fn transcribe_pcm_stream_inner(
    config: &AsrClientConfig,
    auth: &AuthParams,
    mut pcm_rx: UnboundedReceiver<Vec<u8>>,
    options: &PcmTranscribeOptions,
    event_tx: Option<UnboundedSender<AsrEvent>>,
) -> CoreResult<Vec<AsrEvent>> {
    install_rustls_crypto_provider();
    auth.validate()?;
    validate_pcm_options(options)?;

    let web_tab_id = Uuid::new_v4().to_string();
    let url = config.build_url(auth, &web_tab_id)?;
    let wire_protocol = config.wire_protocol();
    if wire_protocol == AsrWireProtocol::LegacyRawPcm {
        return Err(CoreError::AsrConnection(
            "Legacy raw-PCM ASR endpoint does not support streaming input; use the VoiceGenie endpoint".to_string(),
        ));
    }
    let mut request = url
        .into_client_request()
        .map_err(|error| asr_connection_error("Build ASR WebSocket request", error))?;
    request.headers_mut().insert(
        "Cookie",
        HeaderValue::from_str(&auth.cookie_header())
            .map_err(|error| asr_connection_error("Invalid ASR Cookie header", error))?,
    );
    request.headers_mut().insert(
        "Origin",
        HeaderValue::from_str(&config.origin)
            .map_err(|error| asr_connection_error("Invalid ASR Origin header", error))?,
    );
    request.headers_mut().insert(
        "User-Agent",
        HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
        ),
    );
    request.headers_mut().insert(
        "Accept-Language",
        HeaderValue::from_static("zh-CN,zh;q=0.9"),
    );

    let (socket, _) = connect_async(request)
        .await
        .map_err(|error| asr_ws_error("Connect ASR WebSocket", &error))?;
    let (mut writer, mut reader) = socket.split();

    let mut events = vec![AsrEvent::Opened];
    if let Some(event_tx) = event_tx.as_ref() {
        let _ = event_tx.send(AsrEvent::Opened);
    }
    let task_id =
        start_asr_session_if_needed(&mut writer, &mut reader, auth, wire_protocol, options).await?;
    let (receive_done_tx, receive_done_rx) = oneshot::channel::<()>();
    let (input_done_tx, input_done_rx) = oneshot::channel::<()>();
    let send_result = async {
        let mut sequence = 1;
        let stream_stats = send_pcm_stream(
            &mut writer,
            &mut pcm_rx,
            options,
            wire_protocol,
            &task_id,
            &mut sequence,
        )
        .await?;
        emit_observer_event(
            event_tx.as_ref(),
            AsrEvent::InputEnded {
                chunks: stream_stats.chunks,
                bytes: stream_stats.bytes,
            },
        );
        finish_audio_input(&mut writer, wire_protocol, &task_id).await?;
        emit_observer_event(event_tx.as_ref(), AsrEvent::WaitingForServer);
        let _ = input_done_tx.send(());
        // 输入结束后不要立即 close WebSocket；服务端仍可能返回最后一次修正。
        let _ = receive_done_rx.await;
        if wire_protocol == AsrWireProtocol::VoiceGenie {
            let _ = send_voicegenie_control_event(
                &mut writer,
                VOICEGENIE_FINISH_SESSION,
                None,
                Some(&task_id),
            )
            .await;
        }
        let _ = writer.close().await;
        CoreResult::Ok(())
    };
    let receive_result = async {
        let result = receive_asr_events(
            &mut reader,
            options,
            event_tx.as_ref(),
            true,
            Some(input_done_rx),
        )
        .await;
        let _ = receive_done_tx.send(());
        result
    };

    let (send_result, receive_result) = tokio::join!(send_result, receive_result);
    send_result?;
    events.extend(receive_result?);

    Ok(events)
}

/// 显式安装 rustls crypto provider。
///
/// workspace 中可能同时启用多个 rustls feature 组合，显式安装可以避免运行时无法
/// 自动选择 provider 的 panic。
fn install_rustls_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// 检查发送参数是否满足 PCM 分片约束。
fn validate_pcm_options(options: &PcmTranscribeOptions) -> CoreResult<()> {
    if options.chunk_bytes == 0 {
        return Err(CoreError::AudioUnavailable(
            "chunk_bytes must be greater than zero".to_string(),
        ));
    }
    if options.chunk_bytes % 2 != 0 {
        return Err(CoreError::AudioUnavailable(
            "chunk_bytes must align to i16 samples".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
