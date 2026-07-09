use std::time::Duration;

use futures_util::SinkExt;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message;

use super::config::AsrWireProtocol;
use super::options::PcmTranscribeOptions;
use super::protocol::encode_client_audio_frame;
use crate::{CoreError, CoreResult};

/// 按接近实时录音的节奏发送 PCM 分片。
pub(crate) async fn send_pcm_chunks<S>(
    socket: &mut S,
    pcm: &[u8],
    options: &PcmTranscribeOptions,
    wire_protocol: AsrWireProtocol,
    task_request_id: &str,
    sequence: &mut u64,
) -> CoreResult<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Display,
{
    for chunk in pcm.chunks(options.chunk_bytes) {
        let mut stats = SendStats::default();
        send_binary_frame(
            socket,
            chunk.to_vec(),
            &mut stats,
            wire_protocol,
            task_request_id,
            sequence,
        )
        .await?;
        if options.chunk_delay_ms > 0 {
            sleep(Duration::from_millis(options.chunk_delay_ms)).await;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SendStats {
    pub(crate) chunks: usize,
    pub(crate) bytes: usize,
}

/// 旧 raw PCM 端点不支持分片传输，只能发送单个 binary message。
pub(crate) async fn send_legacy_pcm_once<S>(
    socket: &mut S,
    pcm: &[u8],
    options: &PcmTranscribeOptions,
) -> CoreResult<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Display,
{
    if pcm.len() % 2 != 0 {
        return Err(CoreError::AudioUnavailable(
            "pcm bytes must align to i16 samples".to_string(),
        ));
    }

    let target_len = pcm.len() + legacy_tail_silence_bytes(options);
    let mut message = Vec::with_capacity(target_len);
    message.extend_from_slice(pcm);
    message.resize(target_len, 0);
    socket
        .send(Message::Binary(message.into()))
        .await
        .map_err(|error| CoreError::AsrConnection(error.to_string()))
}

fn legacy_tail_silence_bytes(options: &PcmTranscribeOptions) -> usize {
    if options.tail_silence_ms == 0 {
        return 0;
    }
    let bytes_per_ms = 16_000 * 2 / 1_000;
    (options.tail_silence_ms as usize) * bytes_per_ms
}

/// 按麦克风回调节奏发送实时 PCM 分片。
pub(crate) async fn send_pcm_stream<S>(
    socket: &mut S,
    pcm_rx: &mut UnboundedReceiver<Vec<u8>>,
    options: &PcmTranscribeOptions,
    wire_protocol: AsrWireProtocol,
    task_request_id: &str,
    sequence: &mut u64,
) -> CoreResult<SendStats>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Display,
{
    let mut stats = SendStats {
        chunks: 0,
        bytes: 0,
    };
    let mut pending = Vec::with_capacity(options.chunk_bytes);
    while let Some(chunk) = pcm_rx.recv().await {
        if chunk.is_empty() {
            continue;
        }
        if chunk.len() % 2 != 0 {
            return Err(CoreError::AudioUnavailable(
                "streamed pcm chunk must align to i16 samples".to_string(),
            ));
        }
        pending.extend_from_slice(&chunk);
        while pending.len() >= options.chunk_bytes {
            let frame = pending.drain(..options.chunk_bytes).collect::<Vec<_>>();
            send_binary_frame(
                socket,
                frame,
                &mut stats,
                wire_protocol,
                task_request_id,
                sequence,
            )
            .await?;
        }
    }
    if !pending.is_empty() {
        send_binary_frame(
            socket,
            pending,
            &mut stats,
            wire_protocol,
            task_request_id,
            sequence,
        )
        .await?;
    }
    Ok(stats)
}

async fn send_binary_frame<S>(
    socket: &mut S,
    frame: Vec<u8>,
    stats: &mut SendStats,
    wire_protocol: AsrWireProtocol,
    task_request_id: &str,
    sequence: &mut u64,
) -> CoreResult<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Display,
{
    if frame.len() % 2 != 0 {
        return Err(CoreError::AudioUnavailable(
            "streamed pcm frame must align to i16 samples".to_string(),
        ));
    }
    if frame.is_empty() {
        return Ok(());
    }
    stats.chunks += 1;
    stats.bytes += frame.len();
    let message = encode_client_audio_frame(frame, wire_protocol, task_request_id, *sequence);
    *sequence += 1;
    socket
        .send(Message::Binary(message.into()))
        .await
        .map_err(|error| CoreError::AsrConnection(error.to_string()))?;
    Ok(())
}
