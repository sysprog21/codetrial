//! How a Live session ends and how a test looks at it: retiring the transport
//! while the room keeps the session's metadata, joining the reader, reporting
//! the event channel's pressure, and the read-only views fixtures use.

use super::GeminiLiveSession;
use super::pressure::{Pressure, SocketPressure};
#[cfg(test)]
use super::{ApiFailure, TokenUsage, credentials};
#[cfg(test)]
use futures_util::stream::SplitSink;
use std::sync::{Arc, Mutex};
#[cfg(test)]
use tokio::net::TcpStream;
use tokio::sync::mpsc::channel;
#[cfg(test)]
use tokio_tungstenite::tungstenite::Message;
#[cfg(test)]
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// A rotation that ran dry on quota alone with its first key back at
/// `retry_at`, and a bare provider failure with `status` (0 is a connect or
/// setup failure), for logical-time retry fixtures that cannot wait out a real
/// cooldown or open a real socket.
#[cfg(test)]
pub(crate) fn quota_exhausted_for_test(
    retry_at: std::time::Instant,
) -> Box<dyn std::error::Error + Send + Sync> {
    credentials::exhausted_for_test(retry_at).into()
}

#[cfg(test)]
pub(crate) fn provider_failure_for_test(status: u16) -> Box<dyn std::error::Error + Send + Sync> {
    ApiFailure {
        status,
        credential: None,
        detail: None,
    }
    .into()
}

/// Read-only ownership and receipt observation for local lifecycle fixtures.
#[cfg(test)]
pub(crate) struct LiveReaderInspection {
    task: tokio::task::AbortHandle,
    usage: Arc<Mutex<std::collections::VecDeque<TokenUsage>>>,
}

#[cfg(test)]
impl LiveReaderInspection {
    pub(crate) fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Ends the reader the way a server hang-up does from the loop's side:
    /// its event stream closes and the room loop takes its Close path.
    pub(crate) fn abort(&self) {
        self.task.abort();
    }

    pub(crate) fn pending_usage_count(&self) -> usize {
        self.usage
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }
}

impl GeminiLiveSession {
    /// Leaves a fail-fast session in the room while its owner retires the
    /// transport. Metadata and unconsumed usage stay with the returned socket.
    pub(crate) fn retire(&mut self) -> Self {
        let (sender, events) = channel(1);
        drop(sender);
        std::mem::replace(
            self,
            Self {
                writer: None,
                reader: None,
                events,
                pressure: Pressure::already_reported(),
                event_received_at: None,
                opened_at: std::time::Instant::now(),
                resumption: Arc::new(Mutex::new(None)),
                usage: Arc::new(Mutex::new(std::collections::VecDeque::new())),
                checkpoint_at: Arc::new(Mutex::new(None)),
                busy: Arc::new(Mutex::new(None)),
                credential: None,
                failure: Arc::new(Mutex::new(None)),
                dead: true,
                last_ping: tokio::time::Instant::now(),
                input_cause: None,
            },
        )
    }

    /// Can be retried after cancellation: the handle stays owned until the
    /// join finishes, so Drop can still abort it at every suspension point.
    pub(crate) async fn stop_reader(&mut self) {
        if let Some(reader) = &mut self.reader {
            reader.abort();
            let _ = reader.await;
        }
        self.reader = None;
    }

    /// Makes this socket look `age` old, which a test cannot wait for.
    #[cfg(test)]
    pub(crate) fn age_for_test(&mut self, age: std::time::Duration) {
        self.opened_at = std::time::Instant::now() - age;
    }

    #[cfg(test)]
    pub(crate) fn inspect_reader_for_test(&self) -> Option<LiveReaderInspection> {
        self.reader.as_ref().map(|reader| LiveReaderInspection {
            task: reader.abort_handle(),
            usage: Arc::clone(&self.usage),
        })
    }

    #[cfg(test)]
    pub(crate) fn writer_for_test(
        &mut self,
    ) -> &mut SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message> {
        self.writer
            .as_mut()
            .expect("the test owns a connected writer")
    }

    /// The channel pressure of this socket, once: a socket let go reports it
    /// at its replacement or at the end of the interview, whichever comes
    /// first, and a retired placeholder never does.
    pub(crate) fn take_pressure(&mut self) -> Option<SocketPressure> {
        self.pressure.take_report()
    }

    #[cfg(test)]
    pub(crate) fn pressure_for_test(&self) -> SocketPressure {
        self.pressure.peek()
    }
}
