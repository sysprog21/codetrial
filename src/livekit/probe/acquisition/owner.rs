//! Controlled owner-await gates, distinct from physical transport pressure.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use super::Capture;
use crate::agent::ThinkingHold;
use crate::livekit::session::GeminiEventContext;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::livekit) enum Stage {
    Acknowledgement,
    BoardSettlement,
    BoardResend,
    ToolSend,
}

#[derive(Clone, Copy)]
pub(in crate::livekit) enum Transition {
    Hold(bool),
    Discard(bool),
}

pub(in crate::livekit) struct Gate {
    scope: Arc<Capture>,
    socket: u64,
    stage: Stage,
    transition: Option<Transition>,
    entered: Notify,
    release: Notify,
}

impl Gate {
    pub(in crate::livekit) fn new(
        scope: &Arc<Capture>,
        socket: u64,
        stage: Stage,
        transition: Option<Transition>,
    ) -> Result<Arc<Self>, &'static str> {
        if socket == 0 {
            return Err("owner gate requires an assigned socket");
        }
        if transition.is_some() && stage != Stage::ToolSend {
            return Err("owner transition requires the scoped tool-send boundary");
        }
        Ok(Arc::new(Self {
            scope: Arc::clone(scope),
            socket,
            stage,
            transition,
            entered: Notify::new(),
            release: Notify::new(),
        }))
    }

    pub(in crate::livekit) async fn entered(&self) -> Result<(), tokio::time::error::Elapsed> {
        tokio::time::timeout(Duration::from_secs(2), self.entered.notified()).await
    }

    pub(in crate::livekit) fn release(&self) {
        self.release.notify_one();
    }
}

static ACTIVE: Mutex<Option<Arc<Gate>>> = Mutex::new(None);

pub(in crate::livekit) struct Installed(Arc<Gate>);
impl Drop for Installed {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
        if active.as_ref().is_some_and(|now| Arc::ptr_eq(now, &self.0)) {
            *active = None;
        }
    }
}

pub(in crate::livekit) fn install(gate: &Arc<Gate>) -> Result<Installed, &'static str> {
    let mut active = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
    if active.is_some() {
        return Err("an owner gate is already installed");
    }
    *active = Some(Arc::clone(gate));
    Ok(Installed(Arc::clone(gate)))
}

pub(in crate::livekit) async fn before(stage: Stage, context: &mut GeminiEventContext<'_>) {
    let gate = ACTIVE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(Arc::clone);
    let Some(gate) = gate else { return };
    let matches = {
        let identity = context.activity.latency.acquisition_identity();
        gate.stage == stage
            && gate.scope.room == identity.room
            && gate.scope.session == identity.session
            && gate.socket == identity.socket
    };
    if !matches {
        return;
    }
    {
        let mut active = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
        if !active.as_ref().is_some_and(|now| Arc::ptr_eq(now, &gate)) {
            return;
        }
        *active = None;
    }
    gate.entered.notify_one();
    gate.release.notified().await;
    match gate.transition {
        Some(Transition::Hold(held)) => {
            context.state.thinking_hold = if held {
                ThinkingHold::Held {
                    since: Instant::now(),
                }
            } else {
                ThinkingHold::Off
            };
        }
        Some(Transition::Discard(discarded)) => {
            context.activity.socket.discarding_output = discarded;
        }
        None => {}
    }
}
