use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio::time::{sleep, sleep_until, timeout, Instant};
use tokio_tungstenite::tungstenite::Message;

use super::error_map::asr_ws_error;
use super::event::AsrEvent;
use super::options::PcmTranscribeOptions;
use super::parser::{parse_binary_server_event, parse_server_event};
use crate::{CoreError, CoreResult};

pub(crate) async fn receive_asr_events<S>(
    socket: &mut S,
    options: &PcmTranscribeOptions,
    event_tx: Option<&UnboundedSender<AsrEvent>>,
    allow_timeout_after_events: bool,
    input_done_rx: Option<oneshot::Receiver<()>>,
) -> CoreResult<Vec<AsrEvent>>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let mut events = Vec::new();
    let mut input_done_rx = input_done_rx;
    let mut post_input_deadline = None;
    loop {
        let receive_step = receive_next_message(
            socket,
            options,
            event_tx,
            allow_timeout_after_events,
            &events,
            &mut input_done_rx,
            &mut post_input_deadline,
        )
        .await?;

        let next_message = match receive_step {
            ReceiveStep::Message(next_message) => next_message,
            ReceiveStep::Continue => continue,
            ReceiveStep::Stop => break,
        };

        let Some(message) = next_message else {
            emit_observer_event(event_tx, AsrEvent::SocketClosed);
            break;
        };

        let message = message.map_err(|error| asr_ws_error("Receive ASR WebSocket message", &error))?;
        match message {
            Message::Text(text) => {
                if let Some(event) = parse_server_event(&text)? {
                    let is_finished = matches!(event, AsrEvent::Finished);
                    if let Some(event_tx) = event_tx {
                        let _ = event_tx.send(event.clone());
                    }
                    events.push(event);
                    if is_finished {
                        break;
                    }
                }
            }
            Message::Binary(data) => {
                let parsed_event = if let Some(event) = parse_binary_server_event(&data)? {
                    Some(event)
                } else {
                    let text = String::from_utf8_lossy(&data);
                    parse_server_event(&text)?
                };
                if let Some(event) = parsed_event {
                    let is_finished = matches!(event, AsrEvent::Finished);
                    if let Some(event_tx) = event_tx {
                        let _ = event_tx.send(event.clone());
                    }
                    events.push(event);
                    if is_finished {
                        break;
                    }
                }
            }
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
    Ok(events)
}

type WsMessageResult = Option<Result<Message, tokio_tungstenite::tungstenite::Error>>;

enum ReceiveStep {
    Message(WsMessageResult),
    Continue,
    Stop,
}

async fn receive_next_message<S>(
    socket: &mut S,
    options: &PcmTranscribeOptions,
    event_tx: Option<&UnboundedSender<AsrEvent>>,
    allow_timeout_after_events: bool,
    events: &[AsrEvent],
    input_done_rx: &mut Option<oneshot::Receiver<()>>,
    post_input_deadline: &mut Option<Instant>,
) -> CoreResult<ReceiveStep>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    if let Some(deadline) = *post_input_deadline {
        let post_input_timeout = sleep_until(deadline);
        tokio::pin!(post_input_timeout);
        return Ok(tokio::select! {
            _ = &mut post_input_timeout => {
                handle_receive_timeout(event_tx, allow_timeout_after_events, events, options.post_input_receive_timeout_ms)?;
                ReceiveStep::Stop
            }
            next_message = socket.next() => ReceiveStep::Message(next_message),
        });
    }

    if let Some(mut input_done) = input_done_rx.take() {
        let receive_timeout = sleep(Duration::from_millis(options.receive_timeout_ms));
        tokio::pin!(receive_timeout);
        return Ok(tokio::select! {
            _ = &mut input_done => {
                *post_input_deadline = Some(
                    Instant::now()
                        + Duration::from_millis(options.post_input_receive_timeout_ms),
                );
                ReceiveStep::Continue
            }
            _ = &mut receive_timeout => {
                *input_done_rx = Some(input_done);
                handle_receive_timeout(event_tx, allow_timeout_after_events, events, options.receive_timeout_ms)?;
                ReceiveStep::Stop
            }
            next_message = socket.next() => {
                *input_done_rx = Some(input_done);
                ReceiveStep::Message(next_message)
            }
        });
    }

    match timeout(
        Duration::from_millis(options.receive_timeout_ms),
        socket.next(),
    )
    .await
    {
        Ok(next_message) => Ok(ReceiveStep::Message(next_message)),
        Err(_) => {
            handle_receive_timeout(
                event_tx,
                allow_timeout_after_events,
                events,
                options.receive_timeout_ms,
            )?;
            Ok(ReceiveStep::Stop)
        }
    }
}

fn handle_receive_timeout(
    event_tx: Option<&UnboundedSender<AsrEvent>>,
    allow_timeout_after_events: bool,
    events: &[AsrEvent],
    timeout_ms: u64,
) -> CoreResult<()> {
    if allow_timeout_after_events && !events.is_empty() {
        emit_observer_event(
            event_tx,
            AsrEvent::ReceiveTimeout {
                events: events.len(),
                timeout_ms,
            },
        );
        return Ok(());
    }

    Err(if events.is_empty() {
        CoreError::AsrConnection(format!(
            "Timed out after {timeout_ms}ms waiting for the first ASR result. Check network or auth."
        ))
    } else {
        CoreError::AsrConnection(format!(
            "Timed out after {timeout_ms}ms waiting for ASR finish event ({} events received)",
            events.len()
        ))
    })
}

pub(crate) fn emit_observer_event(event_tx: Option<&UnboundedSender<AsrEvent>>, event: AsrEvent) {
    if let Some(event_tx) = event_tx {
        let _ = event_tx.send(event);
    }
}
