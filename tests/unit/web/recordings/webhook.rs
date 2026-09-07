//! The `tests` module of `src/web/recordings/webhook.rs`, which declares this
//! file by path.
//! Everything here reaches into `src/web/recordings/webhook.rs` through
//! `super`, so it is a unit
//! test and not an integration test: private items are in scope.

use super::*;
use crate::recording::{Failure, RecordingState};

/// Which provider word means which failure, pinned one arm at a time.
///
/// Every arm here reaches the candidate: `Failure` carries the recovery the
/// status route prints, so mapping an aborted job to the allowance failure
/// tells somebody to wait for a quota that was never the problem. Deleting
/// any one arm sends its status to the catch-all instead, where the row is
/// left active and the sweeper fails it a minute later for the wrong
/// reason, and nothing was checking that.
#[test]
fn a_provider_status_maps_to_its_own_failure() {
    let object = "codetrial/rec-1.mp4";
    let complete = serde_json::json!({
        "egressInfo": { "fileResults": [{ "filename": object, "size": "1024" }] }
    });
    let empty = serde_json::json!({ "egressInfo": { "fileResults": [] } });

    assert_eq!(
        egress_outcome("EGRESS_COMPLETE", &complete, object),
        Some((RecordingState::Transferring, None)),
        "a completion carrying the expected object is the only way to transfer"
    );
    for (status, failure) in [
        ("EGRESS_COMPLETE", Failure::PartialOutput),
        ("EGRESS_FAILED", Failure::Egress),
        ("EGRESS_ABORTED", Failure::Aborted),
        ("EGRESS_LIMIT_REACHED", Failure::LimitReached),
    ] {
        assert_eq!(
            egress_outcome(status, &empty, object),
            Some((RecordingState::Failed, Some(failure))),
            "{status}"
        );
    }

    // Still running, and saying so must not mark the row stopped: the orphan
    // sweep would stop watching a job that has not finished.
    for status in ["EGRESS_ACTIVE", "EGRESS_STARTING", "EGRESS_ENDING", ""] {
        assert_eq!(egress_outcome(status, &empty, object), None, "{status}");
    }
}
