//! The setup exchange that opens a Live session: one deadline covers sending
//! the setup message and waiting for the provider to complete it.

use super::{
    ApiFailure, failure_from_reason, is_setup_complete_text, redact_api_keys, server_api_failure,
};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use std::io;
use std::time::Duration;
use tokio_tungstenite::tungstenite::protocol::Message;

/// A timed-out send may have left a partial frame in the sink. The opener
/// drops the transport on any error; it must not reuse it or wait for a close.
pub(super) async fn complete_live_setup<S>(
    stream: &mut S,
    setup: Message,
    api_keys: &[String],
    limit: Duration,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    S: Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let deadline = tokio::time::Instant::now() + limit;
    tokio::time::timeout_at(deadline, stream.send(setup))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Gemini setup send timed out"))??;
    tokio::time::timeout_at(deadline, wait_for_setup_complete(stream, api_keys))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Gemini setup response timed out"))?
}

pub(super) async fn wait_for_setup_complete<S>(
    stream: &mut S,
    api_keys: &[String],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    S: Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    while let Some(message) = stream.next().await {
        let message = message?;
        let text = match &message {
            Message::Text(text) => Some(text.as_str()),
            Message::Binary(bytes) => std::str::from_utf8(bytes).ok(),
            _ => None,
        };
        if let Some(error) = text.and_then(server_api_failure) {
            return Err(error.into());
        }
        match message {
            Message::Text(text) if is_setup_complete_text(text.as_str()) => return Ok(()),
            Message::Binary(bytes) if is_setup_complete_text(std::str::from_utf8(&bytes)?) => {
                return Ok(());
            }
            Message::Close(frame) => {
                let detail = match &frame {
                    Some(frame) => format!(
                        "Gemini closed before setupComplete: code={} reason={}",
                        u16::from(frame.code),
                        redact_api_keys(&frame.reason, api_keys)
                    ),
                    None => "Gemini closed before setupComplete without a reason".to_string(),
                };
                if let Some(failure) = frame
                    .as_ref()
                    .and_then(|frame| failure_from_reason(&frame.reason))
                {
                    return Err(ApiFailure {
                        status: 0,
                        credential: Some(failure),
                        detail: Some(detail),
                    }
                    .into());
                }
                return Err(io::Error::new(io::ErrorKind::ConnectionAborted, detail).into());
            }
            _ => {}
        }
    }

    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "Gemini WebSocket ended before setupComplete",
    )
    .into())
}
