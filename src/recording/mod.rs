//! The recording pipeline: start a RoomComposite egress, follow it through the
//! webhooks LiveKit sends back, hand the file to the delivery queue, and delete
//! it when retention says to.
//!
//! Split by the stage of that pipeline a piece belongs to, because the stages
//! fail independently and are read independently. The file this grew out of
//! opened with a doc comment describing the fixed values at the top of it,
//! which by the end described about one part in sixty of what the file held.
//!
//! - [`contract`] — the strings and numbers shared with LiveKit and the
//!   Egress template. `docs/recording-contract.md` explains each one.
//! - [`provider`] — the state machine, and the two traits the rest of the
//!   pipeline is written against: `RecordingProvider` for the Egress calls,
//!   `DeliveryProvider` for where the media goes afterwards.
//! - [`store`] — the rows: starting a recording, moving it, reading it back,
//!   and the webhook-event ledger that makes a redelivered webhook idempotent.
//! - [`sweeper`] — stopping, and everything that runs on a timer because a
//!   provider call or a webhook did not arrive.
//! - [`webhook`] — reading LiveKit's `egress_ended` payload, and the audit
//!   line every stage writes.
//! - [`queue`] — what is owed: which recordings are waiting to be delivered,
//!   who claimed one, and when a failed attempt comes back.
//! - [`transfer`] — one delivery: the upload, the share, and undoing the parts
//!   of it that landed when a later part did not.
//! - [`deletion`] — removing the media, and the tombstone that outlives it.
//! - [`summary`] — what an account sees of its own recordings, and what
//!   retention has scheduled.
//! - [`replay`] — the non-media record of the session the template renders.

pub mod contract;
pub mod deletion;
pub mod provider;
pub mod queue;
pub mod replay;
pub mod store;
pub mod summary;
pub mod sweeper;
pub mod transfer;
pub mod webhook;

// The pipeline is one namespace to everything outside it. The split is how this
// code is read and changed, not a new API: `crate::recording::stop_recording`
// keeps meaning what it did when there was one file.
pub use contract::*;
pub use deletion::*;
pub use provider::*;
pub use queue::*;
pub use replay::*;
pub use store::*;
pub use summary::*;
pub use sweeper::*;
pub use transfer::*;
pub use webhook::*;
