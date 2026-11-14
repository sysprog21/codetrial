//! Starting an interview's Live session: the first open's bound, the
//! credential check that mirrors it, and the startup probe that waits for the
//! model's first output without consuming it, so the room loop still delivers
//! the greeting and accounts for everything before it.

use std::io;
use std::time::Duration;

use super::{
    ApiFailure, ApiSurface, CredentialFailure, FIRST_OPEN_LIMIT, GEMINI_EVENT_QUEUE, GeminiEvent,
    GeminiKeys, GeminiLiveSession, LIVE_WEBSOCKET_ENDPOINT, LiveSession, credential_failure,
    credentials, live_session_with_keys_at,
};

/// Bounds `open` by `limit`, the way `FIRST_OPEN_LIMIT` bounds a first open.
pub(crate) async fn first_open_within<T>(
    limit: Duration,
    open: impl Future<Output = Result<T, Box<dyn std::error::Error + Send + Sync>>>,
) -> Result<T, Box<dyn std::error::Error + Send + Sync>> {
    tokio::time::timeout(limit, open).await.unwrap_or_else(|_| {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("Gemini did not open within {limit:?}"),
        )
        .into())
    })
}

/// Checks configured Live credentials, moving past keys Gemini rejects while
/// backups remain.
///
/// For `check-gemini`, which should pass whenever an interview could start.
/// It makes at most one attempt per configured key under its diagnostic budget,
/// all inside `FIRST_OPEN_LIMIT`.
/// Marking a rejected key unavailable is not a bound on its own: a quota
/// cooldown lasts a minute, and slow rejections from enough keys outlast it,
/// putting the first key back in rotation. Other failures return at once: a
/// check reports them instead of waiting them out.
pub async fn check_live_session(
    keys: &GeminiKeys,
    boot: &dyn LiveSession,
) -> Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>> {
    check_live_session_at(LIVE_WEBSOCKET_ENDPOINT, keys, boot).await
}

pub(crate) async fn check_live_session_at(
    endpoint: &str,
    keys: &GeminiKeys,
    boot: &dyn LiveSession,
) -> Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>> {
    let attempts = keys.count().min(crate::livekit::GEMINI_RESTART_LIMIT + 1);
    first_open_within(FIRST_OPEN_LIMIT, async {
        for _ in 1..attempts {
            match live_session_with_keys_at(endpoint, keys, boot, None).await {
                Err(error) if credential_failure(error.as_ref()).is_some() => {}
                result => return result,
            }
        }
        live_session_with_keys_at(endpoint, keys, boot, None).await
    })
    .await
}

impl GeminiLiveSession {
    /// What Gemini said about this socket's credential, if it said anything.
    fn reported_failure(&self) -> Option<CredentialFailure> {
        *self
            .failure
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub(crate) fn record_failure(&self, keys: &GeminiKeys) {
        if let Some(failure) = self.reported_failure()
            && let Some(key) = &self.credential
        {
            keys.failed(key, failure, ApiSurface::Live);
        }
    }

    pub(crate) fn startup_error(
        &self,
        error: Box<dyn std::error::Error + Send + Sync>,
    ) -> Box<dyn std::error::Error + Send + Sync> {
        match self.reported_failure() {
            Some(failure) => ApiFailure {
                status: 0,
                credential: Some(failure),
                detail: Some("Gemini refused startup".to_string()),
            }
            .into(),
            None => error,
        }
    }

    pub(crate) fn audio_write_queued(&self) -> bool {
        self.queued_audio
    }

    /// End can win before the room loop replays the bounded startup prefix.
    /// Take its speech once, with copies of its turn boundaries and tool calls,
    /// without polling the reader or consuming output, usage or their receive
    /// times. Empty tool batches still disarm the runtime's boundary guards.
    pub(crate) fn take_buffered_startup_speech(&mut self) -> Vec<GeminiEvent> {
        let mut speech = Vec::new();
        self.pending_events.retain_mut(|(_, event)| match event {
            GeminiEvent::InputTranscript(text) => {
                speech.push(GeminiEvent::InputTranscript(std::mem::take(text)));
                false
            }
            GeminiEvent::TurnComplete | GeminiEvent::Interrupted | GeminiEvent::ToolCall(_) => {
                speech.push(event.clone());
                true
            }
            _ => true,
        });
        speech
    }

    /// A failed or cancelled input write may hide an already received answer.
    /// Inspect received events with their order, timestamps and queue bound.
    /// Only receive times strictly before the original deadline qualify; the
    /// deadline itself is expired. Never wait or request another generation.
    pub(crate) fn buffered_startup_output(
        &mut self,
        deadline: std::time::Instant,
    ) -> Result<Option<std::time::Instant>, Box<dyn std::error::Error + Send + Sync>> {
        // Snapshot before draining: publication guarantees every preceding
        // event is already queued. Reading it afterwards could miss that
        // prefix, including candidate speech that End must preserve before
        // replay.
        let observed = self
            .first_output
            .get()
            .copied()
            .filter(|at| *at < deadline && !self.gone());
        if let Some((at, _)) = self
            .pending_events
            .iter()
            .find(|(at, event)| *at < deadline && model_output(event))
        {
            return Ok(Some(*at));
        }
        while let Ok(event) = self.events.try_recv() {
            if let Some(at) = self.buffer_startup_event(event)?
                && at < deadline
            {
                return Ok(Some(at));
            }
        }
        Ok(observed)
    }

    pub(super) fn buffer_startup_event(
        &mut self,
        (received_at, event): (std::time::Instant, GeminiEvent),
    ) -> Result<Option<std::time::Instant>, Box<dyn std::error::Error + Send + Sync>> {
        let ready = model_output(&event);
        if self.pending_events.len() == GEMINI_EVENT_QUEUE {
            return Err(io::Error::other("Gemini startup event limit reached").into());
        }
        self.pending_events.push_back((received_at, event));

        // Taken off the channel here, so the room loop's later replay from
        // pending_events is not counted a second time.
        self.pressure.consumed();
        Ok(ready.then_some(received_at))
    }

    /// The startup gate observes output without consuming the greeting. The
    /// room loop must still deliver it and account for earlier usage frames.
    /// Return its receive time, not the time a delayed wait dequeues it.
    /// Until that loop starts, this wait must also ping: the reader's idle
    /// watchdog can be shorter than the configured reply timeout.
    pub(crate) async fn wait_for_model_output(
        &mut self,
        keepalive: &mut tokio::time::Interval,
    ) -> Result<std::time::Instant, Box<dyn std::error::Error + Send + Sync>> {
        loop {
            let event = tokio::select! {
                event = self.events.recv() => event,
                _ = keepalive.tick() => {
                    self.keep_alive().await?;
                    continue;
                }
            };
            let Some((received_at, event)) = event else {
                break;
            };
            if let Some(at) = self.buffer_startup_event((received_at, event))? {
                return Ok(at);
            }
        }
        Err(ApiFailure {
            status: 0,
            credential: self.reported_failure(),
            detail: Some("Gemini ended before producing startup output".to_string()),
        }
        .into())
    }
}

pub(super) fn model_output(event: &GeminiEvent) -> bool {
    match event {
        GeminiEvent::Audio { bytes, .. } => !bytes.is_empty(),
        GeminiEvent::Text(text) | GeminiEvent::OutputTranscript(text) => !text.trim().is_empty(),
        GeminiEvent::ToolCall(calls) => !calls.is_empty(),
        _ => false,
    }
}

/// Machine-owned reasons only; silence and an internal provider error do not
/// identify quota or billing. The browser uses these on interviewer_state.
pub(crate) fn startup_failure_reason(error: &(dyn std::error::Error + 'static)) -> &'static str {
    if credentials::exhausted_by_billing(error) {
        return "billing";
    }
    if credentials::exhausted_until(error).is_some() {
        return "quota";
    }
    match credential_failure(error) {
        Some(CredentialFailure::Quota) => "quota",
        Some(CredentialFailure::Billing) => "billing",
        Some(CredentialFailure::Invalid) => "credentials",
        Some(CredentialFailure::Refused) => "refused",
        None => "unavailable",
    }
}
