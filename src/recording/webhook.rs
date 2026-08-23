use serde_json::{Value, json};

/// One structured line on stderr, which is the only audit trail this pipeline
/// has.
///
/// JSON because it is read by whatever collects logs rather than by a person
/// scrolling, and ids only because a deletion log that named an address would
/// outlive the deletion it recorded. `state` and `reason` are enumerated
/// values, never provider text.
/// Returns the line it wrote, so a test can assert on the thing that was
/// emitted rather than on a second copy of this function.
pub fn audit(event: &str, recording_id: &str, fields: &[(&str, &str)]) -> String {
    // Serialized, not escaped by hand. A provider error can carry a newline,
    // and hand-rolled escaping that covers quotes and backslashes lets that
    // newline end the line: everything after it reads as a second log entry
    // that nobody wrote.
    let mut line = serde_json::Map::new();
    line.insert("event".to_string(), json!(event));
    line.insert("recording_id".to_string(), json!(recording_id));
    for (key, value) in fields {
        // Bounded, because the value can be a provider's error body and a log
        // line is not a place to put one.
        let value: String = value.chars().take(AUDIT_FIELD_LIMIT).collect();
        line.insert((*key).to_string(), json!(value));
    }
    let line = Value::Object(line).to_string();
    eprintln!("{line}");
    line
}

/// Characters, not bytes, so a truncation cannot split one.
///
/// It bounds the values only. Event names and field keys are string literals in
/// this crate, and the recording id is 22 characters by construction; nothing
/// caller-supplied reaches either.
const AUDIT_FIELD_LIMIT: usize = 400;

/// Where a recording can fail, and what it means.
///
/// One value per failure a person would act on differently. The provider's own
/// message never reaches the row: it goes to the audit line, and the row
/// carries the code, so `error` stays something a status route can show and a
/// query can group by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The provider refused to start, three retries apart.
    Start,
    /// The recording ended and produced nothing worth moving.
    PartialOutput,
    /// Nothing said what happened to it, and the row went stale.
    LostWebhook,
    /// The provider gave up on its own.
    Egress,
    /// The provider abandoned the job.
    Aborted,
    /// The project ran out of the allowance this job needed.
    LimitReached,
    /// The media exists and could not be delivered. Not terminal: the bytes are
    /// still in the staging bucket, so the recording stays in `transferring`
    /// and another attempt is the recovery.
    Drive,
    /// The media outlived the attempt to delete it.
    Cleanup,
    /// The media was deleted and the row could not be updated to say so.
    Tombstone,
    /// Consent was taken back.
    ConsentWithdrawn,
    /// An operator turned recording off while this was running.
    KillSwitch,
}

impl Failure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "provider_start_failed",
            Self::PartialOutput => "partial_output",
            Self::LostWebhook => "abandoned",
            Self::Egress => "egress_failed",
            Self::Aborted => "egress_aborted",
            Self::LimitReached => "egress_limit_reached",
            Self::Drive => "drive_failed",
            Self::Cleanup => "cleanup_failed",
            Self::Tombstone => "tombstone_failed",
            Self::ConsentWithdrawn => "consent_withdrawn",
            Self::KillSwitch => "kill_switch",
        }
    }

    /// What a person does about it, in the words the status route uses.
    ///
    /// Written down because "failed" is not an instruction. A candidate whose
    /// recording failed before it produced anything can start another one; a
    /// candidate whose file exists and could not be delivered cannot, and
    /// telling them to retry would be telling them to be recorded twice.
    pub fn recovery(self) -> &'static str {
        match self {
            Self::Start | Self::LostWebhook | Self::Egress | Self::Aborted => "start_again",

            // Not the candidate's to fix, and starting again would hit the same
            // ceiling.
            Self::LimitReached => "wait_for_operator",
            Self::PartialOutput => "start_again",

            // The bytes are in the staging bucket. Retrying the delivery is the
            // pipeline's job, and it is not a second interview.
            Self::Drive => "retry_delivery",
            // Neither of these is anything a candidate or the pipeline can do.
            Self::Cleanup => "delete_by_hand",
            Self::Tombstone => "clear_the_row_by_hand",
            Self::ConsentWithdrawn => "none",
            Self::KillSwitch => "wait_for_operator",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::Start,
            Self::PartialOutput,
            Self::LostWebhook,
            Self::Egress,
            Self::Aborted,
            Self::LimitReached,
            Self::Drive,
            Self::Cleanup,
            Self::Tombstone,
            Self::ConsentWithdrawn,
            Self::KillSwitch,
        ]
        .into_iter()
        .find(|failure| failure.as_str() == value)
    }
}

/// Whether an `egress_ended` carried a file worth moving.
///
/// `EGRESS_COMPLETE` with no file result, or a file of no bytes, is the shape
/// a truncated recording arrives in: the provider finished, and there is
/// nothing to deliver. Treating it as a success sent an empty object through
/// the transfer and shared a zero-byte video with a candidate.
pub fn completed_with_output(event: &Value, expected_object: &str) -> bool {
    event
        .pointer("/egressInfo/fileResults")
        .and_then(Value::as_array)
        .is_some_and(|files| {
            files.iter().any(|file| {
                // The object this recording is going to transfer, not any file
                // the job happened to write. A manifest or a segment playlist
                // is a file with a name and a size, and delivering one of those
                // is delivering the wrong thing.
                file.get("filename")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name == expected_object)
                    && file.get("size").is_some_and(byte_count_is_positive)
            })
        })
}

/// `int64` crosses protojson as a string, so a quoted count is the shape this
/// pipeline receives. A number is accepted too: it is a serializer setting on
/// somebody else's service, and rejecting a size that is plainly present would
/// throw away a recording over a configuration nobody here controls.
fn byte_count_is_positive(size: &Value) -> bool {
    size.as_str()
        .and_then(|size| size.parse::<u64>().ok())
        .or_else(|| size.as_u64())
        .is_some_and(|size| size > 0)
}

// The delivery queue: the work between a recording that has a file and one that
// has been handed to the person it belongs to.
//
// One row per recording that still owes a delivery, and none for one that does
// not. Success deletes the row; giving up deletes it too, because
// `recordings.state` and `recordings.error` are where an outcome lives and a
// queue that kept its own history would be a second answer that can disagree.
