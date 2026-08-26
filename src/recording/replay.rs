use rusqlite::OptionalExtension;
use serde_json::Value;

use crate::accounts::Accounts;

use super::*;

/// The replay, which is the interview without the video.
///
/// Six producers, one envelope, one version. A replay is what the candidate was
/// looking at while the recording ran: what was said, what was typed, what the
/// tests said, where the clock was, what Jim was doing, and what the recording
/// itself was doing. None of it is media.
pub const REPLAY_VERSION: u64 = 1;

/// One event, at its largest. An editor snapshot of a full screen of code is a
/// few kilobytes; sixty-four is room for a pathological one and a refusal for
/// anything that is not an event at all.
pub const MAX_REPLAY_EVENT_BYTES: usize = 64 * 1024;

/// Per interview, whichever comes first. Five thousand events is one every half
/// second for forty minutes, and eight megabytes is more replay than any
/// interview produces; both exist so that a stuck producer costs a bounded
/// amount rather than the disk.
pub const MAX_REPLAY_EVENTS: i64 = 5_000;
pub const MAX_REPLAY_BYTES: i64 = 8 * 1024 * 1024;

/// The longest a single string inside a payload may be, in UTF-8 bytes of the
/// value itself, before JSON escaping.
///
/// Bytes, not characters: sixteen thousand four-byte characters are sixty-four
/// kilobytes, which is the whole event budget spent on one field. Escaping can
/// still double a string of quotes on the way into JSON, and the payload limit
/// is what bounds that, because it measures the serialized form.
///
/// Not a size limit so much as a shape limit: a transcript line is a sentence
/// and an editor snapshot is code, and anything arriving as one enormous string
/// is something other than what the producer is for. Over it, the string is cut
/// and the event kept, because the interview is worth more than the tail of one
/// oversized value. That is a visible alteration and the contract says so.
pub const MAX_REPLAY_STRING: usize = 16 * 1024;

/// What a replay event can be about.
///
/// A closed list, because the replay page renders each one differently and an
/// unknown kind is a producer nobody wrote a renderer for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayKind {
    Transcript,
    Editor,
    Tests,
    Stage,
    Avatar,
    Lifecycle,
}

impl ReplayKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transcript => "transcript",
            Self::Editor => "editor",
            Self::Tests => "tests",
            Self::Stage => "stage",
            Self::Avatar => "avatar",
            Self::Lifecycle => "lifecycle",
        }
    }

    pub const ALL: [Self; 6] = [
        Self::Transcript,
        Self::Editor,
        Self::Tests,
        Self::Stage,
        Self::Avatar,
        Self::Lifecycle,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    /// Whether the newest event of this kind is the whole story.
    ///
    /// An editor snapshot replaces the last one and a transcript line does not,
    /// which is what lets a late join be one snapshot plus the events after it
    /// rather than every keystroke since the interview began.
    pub fn is_snapshot(self) -> bool {
        matches!(
            self,
            Self::Editor | Self::Stage | Self::Avatar | Self::Lifecycle
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayEvent {
    pub kind: ReplayKind,
    /// The browser's clock, in milliseconds. Kept because the replay is played
    /// back against it, and never trusted for ordering: `seq` is what orders.
    pub at: i64,
    pub payload: Value,
}

/// Why an event was refused. Each one is a different thing for the producer to
/// have done wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayRejection {
    /// Not this envelope, or not this version of it.
    Envelope,
    /// A kind nothing renders.
    Kind,
    /// Larger than one event may be.
    Oversize,
}

impl ReplayRejection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Envelope => "replay_envelope_invalid",
            Self::Kind => "replay_kind_unknown",
            Self::Oversize => "replay_event_too_large",
        }
    }
}

/// Reads one event off the wire, redacting as it goes.
///
/// Redaction happens here rather than at the database, because this is the last
/// place the value is still a candidate's and the first place it is this
/// server's. A payload that reaches storage unredacted is one nothing later can
/// un-store.
pub fn parse_replay_event(value: &Value) -> Result<ReplayEvent, ReplayRejection> {
    if value.get("v").and_then(Value::as_u64) != Some(REPLAY_VERSION) {
        return Err(ReplayRejection::Envelope);
    }
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .ok_or(ReplayRejection::Envelope)?;
    let kind = ReplayKind::parse(kind).ok_or(ReplayRejection::Kind)?;
    let at = value
        .get("at")
        .and_then(Value::as_i64)
        .filter(|at| *at >= 0)
        .ok_or(ReplayRejection::Envelope)?;
    let payload = value
        .get("payload")
        .filter(|payload| payload.is_object())
        .ok_or(ReplayRejection::Envelope)?;

    // Three steps, in this order, and the order is the whole rule.
    //
    // Trimming first, because cutting an overlong string is a transformation
    // the contract promises: a code snapshot at the limit is a prefix, and the
    // event is kept. Measuring next, because that is the size of what a
    // producer is allowed to send. Stripping secret keys last and measuring
    // again, because removal is not something a producer may rely on to get
    // under the limit: a megabyte arriving under a key that happens to be
    // redacted is a megabyte.
    let payload = trim_replay_payload(payload);
    if payload_bytes(&payload) > MAX_REPLAY_EVENT_BYTES {
        return Err(ReplayRejection::Oversize);
    }
    let payload = strip_secret_keys(&payload);
    if payload_bytes(&payload) > MAX_REPLAY_EVENT_BYTES {
        return Err(ReplayRejection::Oversize);
    }
    Ok(ReplayEvent { kind, at, payload })
}

/// Keys whose value is never worth keeping, whatever a producer thinks.
///
/// Matched on a lowercased key with `-` and `_` removed, containing one of
/// these rather than equal to it: the browser writes `apiKey`, `accessToken`,
/// `x-api-key` and `Authorization`, and a list of exact names would let every
/// one of them through.
///
/// The ceiling, because it is a real one: this is a key check plus a media
/// check, and it does not read values. A bearer token in the middle of a
/// transcript line, or a signed URL in a test result, survives it. The
/// producers are first-party and none of them handle credentials; a
/// value-scanning rule would cost false positives on ordinary code and prose
/// for a case none of them can reach.
///
/// `session` is deliberately absent: it would take `sessionId` and
/// `sessionName` with it, and the session cookie is `HttpOnly` and unreachable
/// from any producer.
const REDACTED_KEYS: [&str; 10] = [
    "token",
    "secret",
    "password",
    "credential",
    "authorization",
    "apikey",
    "privatekey",
    "bearer",
    "jwt",
    "cookie",
];

/// Everything a replay event must not carry, removed rather than refused.
///
/// Removed, because a producer that accidentally included a token should still
/// deliver the transcript line it was carrying; refusing the whole event would
/// lose the interview to protect it.
pub fn redact_replay_payload(payload: &Value) -> Value {
    strip_secret_keys(&trim_replay_payload(payload))
}

/// Media out, overlong strings cut. Everything here is a transformation of a
/// value the producer is allowed to send.
fn trim_replay_payload(payload: &Value) -> Value {
    match payload {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), trim_replay_payload(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(trim_replay_payload).collect()),
        Value::String(text) => Value::String(redact_replay_string(text)),
        other => other.clone(),
    }
}

/// Keys whose value is never kept, at any depth.
fn strip_secret_keys(payload: &Value) -> Value {
    match payload {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .filter(|(key, _)| {
                    let key: String = key
                        .to_ascii_lowercase()
                        .chars()
                        .filter(|character| *character != '-' && *character != '_')
                        .collect();
                    !REDACTED_KEYS.iter().any(|marker| key.contains(marker))
                })
                .map(|(key, value)| (key.clone(), strip_secret_keys(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(strip_secret_keys).collect()),
        other => other.clone(),
    }
}

/// The serialized size of a payload, which is what both limits are in terms of.
///
/// `usize::MAX` when it will not serialize, so a value that cannot be stored is
/// refused as oversize rather than accepted and then found unstorable.
pub fn payload_bytes(payload: &Value) -> usize {
    serde_json::to_vec(payload).map_or(usize::MAX, |bytes| bytes.len())
}

fn redact_replay_string(text: &str) -> String {
    // A `data:` URL is how an image reaches JSON, and the one thing a replay
    // must never carry is media: the video is the provider's, delivered under a
    // permission that expires, and a frame smuggled into an event outlives it.
    let lowered = text.trim_start().to_ascii_lowercase();
    if lowered.starts_with("data:") || lowered.starts_with("blob:") {
        return "[media removed]".to_string();
    }
    if text.len() <= MAX_REPLAY_STRING {
        return text.to_string();
    }

    // Cut on a character boundary, not a byte one. The limit is in bytes
    // because that is what storage costs, and slicing a UTF-8 string at an
    // arbitrary byte panics.
    let mut end = MAX_REPLAY_STRING;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// How many events one request may carry, and how large that request may be.
///
/// The browser batches: a transcript line, an editor snapshot and a timer tick
/// inside one second are three events and one round trip. Both bounds exist so
/// that a batch is a batch rather than a way around the per-event limit.
pub const MAX_REPLAY_BATCH: usize = 32;
pub const MAX_REPLAY_BATCH_BYTES: usize = 256 * 1024;

/// What one ingest request did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ingest {
    /// A batch with nothing in it. Not an error the database can see, and not
    /// something to answer with a range of sequence numbers nobody was given.
    Empty,
    /// The sequence numbers allocated, in order.
    Stored { first: i64, last: i64 },
    /// Nothing was stored, and nothing more will be. The recording is not
    /// failed: a replay that stopped growing is still a recording worth
    /// keeping.
    QuotaExceeded,
    /// No such interview, or not this account's.
    NoInterview,
}

/// Appends a batch, allocating each sequence number as it goes.
///
/// Allocation is inside the insert, not around it. `SELECT MAX(seq)` and then
/// `INSERT` is two statements with a gap in the middle, and two writers in that
/// gap both pick the same number: one of them loses to the primary key and its
/// event is gone. `INSERT ... SELECT` from the same table is one statement, and
/// SQLite serializes writers, so the number is chosen and taken together.
///
/// The quota is checked once, before the batch. Checking per event would let a
/// batch straddle the limit and store half of itself, and half a batch is a
/// replay with a hole in it.
/// Whether this account's replay of this interview is still open, in either
/// direction.
///
/// Withdrawn consent closes it. A browser that buffered events before the
/// withdrawal will try to flush them afterwards, and consent that stops the
/// video while the replay keeps growing is not withdrawal. One predicate for
/// reads and writes both, because a replay nobody may add to is not one to keep
/// handing out either.
fn replay_open(
    connection: &rusqlite::Connection,
    interview_id: &str,
    account_id: i64,
) -> rusqlite::Result<bool> {
    let open: i64 = connection.query_row(
        "
        SELECT COUNT(*) FROM interviews
        WHERE id = ?1 AND account_id = ?2 AND consent_withdrawn_at IS NULL
        ",
        (interview_id, account_id),
        |row| row.get(0),
    )?;
    Ok(open > 0)
}

pub fn append_replay_events(
    accounts: &Accounts,
    interview_id: &str,
    account_id: i64,
    events: &[ReplayEvent],
    now: i64,
) -> rusqlite::Result<Ingest> {
    if events.is_empty() {
        return Ok(Ingest::Empty);
    }
    accounts.with(|connection| {
        // Read first, and only then take the write lock. `BEGIN IMMEDIATE`
        // takes SQLite's database-wide writer lock, so checking ownership
        // inside it would let any signed-in caller serialize every other writer
        // by posting batches for interview ids they guessed. The check inside
        // the transaction below is still the authority; this one only keeps a
        // stranger out of the lock queue.
        if !replay_open(connection, interview_id, account_id)? {
            return Ok(Ingest::NoInterview);
        }

        // One IMMEDIATE transaction around the whole batch, so a batch is all
        // of its events or none of them, and so the sequence it allocates is
        // contiguous. The mutex around this connection serializes this process;
        // IMMEDIATE is what a second `codetrial web` on the same database has
        // to wait on, and it takes the write lock before the quota read rather
        // than upgrading afterwards, which is where a deferred reader would
        // find its snapshot stale.
        let transaction = rusqlite::Transaction::new_unchecked(
            connection,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        if !replay_open(&transaction, interview_id, account_id)? {
            return Ok(Ingest::NoInterview);
        }

        let (count, bytes): (i64, i64) = transaction.query_row(
            "
        SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM replay_events WHERE interview_id = ?1
        ",
            [interview_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let incoming: i64 = events
            .iter()
            .map(|event| i64::try_from(payload_bytes(&event.payload)).unwrap_or(i64::MAX))
            .sum();
        if count + events.len() as i64 > MAX_REPLAY_EVENTS || bytes + incoming > MAX_REPLAY_BYTES {
            // The flag is on the recording, because that is what a reviewer
            // opens: a replay that is missing its tail should say so where the
            // replay is read, not only in a log nobody reads.
            //
            // An interview whose recording never started has no row to flag,
            // and this updates nothing. The refusal still stands: the producer
            // is told the ceiling was reached, and there is no replay to review
            // without a recording to review it beside.
            transaction.execute(
                "UPDATE recordings SET quota_exceeded = 1 WHERE interview_id = ?1",
                [interview_id],
            )?;
            transaction.commit()?;
            return Ok(Ingest::QuotaExceeded);
        }

        let mut first = None;
        let mut last = 0;
        for event in events {
            let payload = event.payload.to_string();
            let bytes = payload.len() as i64;

            // Allocated inside the insert and read back out of it. A
            // SELECT-then-INSERT has a gap two writers land in, and a MAX(seq)
            // read after the insert can return the other writer's row rather
            // than this one's.
            let seq: i64 = transaction.query_row(
                "
        INSERT INTO replay_events (interview_id, seq, kind, at, payload, bytes, received_at)
        SELECT ?1,
               COALESCE((SELECT MAX(seq) FROM replay_events WHERE interview_id = ?1), -1) + 1,
               ?2, ?3, ?4, ?5, ?6
        RETURNING seq
        ",
                (
                    interview_id,
                    event.kind.as_str(),
                    event.at,
                    &payload,
                    bytes,
                    now,
                ),
                |row| row.get(0),
            )?;
            first.get_or_insert(seq);
            last = seq;
        }
        transaction.commit()?;

        // `first` is set on the first pass, because an empty batch returned
        // above.
        Ok(Ingest::Stored {
            first: first.unwrap_or(last),
            last,
        })
    })
}

fn row_to_replay_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, ReplayEvent)> {
    let kind: String = row.get(1)?;
    let payload: String = row.get(3)?;
    Ok((
        row.get::<_, i64>(0)?,
        ReplayEvent {
            // A row the table's own `CHECK` allows and this binary does not is
            // a producer from a later deploy. It comes back as `Lifecycle`
            // rather than taking the read down.
            kind: ReplayKind::parse(&kind).unwrap_or(ReplayKind::Lifecycle),
            at: row.get(2)?,
            payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
        },
    ))
}

/// What a late join needs before it starts reading events.
///
/// Not a stored thing and not a cadence. The four replaceable kinds already are
/// the snapshot: the newest of each is the whole state of that kind, so a
/// snapshot is the replay with the superseded frames dropped, computed at the
/// read. A periodic snapshot written to a second table would be a copy that can
/// disagree with the events it was made from, and nothing needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The last event this snapshot accounts for. Read events after it and
    /// concatenate: that is the whole reconstruction rule.
    pub seq: i64,
    pub events: Vec<(i64, ReplayEvent)>,
    /// The replay stopped growing at a ceiling, so its tail is missing. The
    /// person reading it should be told, not left to wonder.
    pub quota_exceeded: bool,
}

/// A snapshot, or the reason there is not one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotView {
    Ready(Snapshot),
    /// Past the retention deadline. The media is going or gone, and the replay
    /// is not served past the life of the thing it describes.
    Expired,
    /// The recording has been deleted.
    Deleted,
    /// No such interview, not this account's, or consent withdrawn.
    NoInterview,
}

/// The replay of one interview, minus what has been superseded.
///
/// Read inside one transaction, so the ceiling and the rows under it are the
/// same moment: a writer landing between two statements would otherwise put an
/// event in the snapshot that the caller is about to ask for again.
pub fn replay_snapshot(
    accounts: &Accounts,
    interview_id: &str,
    account_id: i64,
    now: i64,
) -> rusqlite::Result<SnapshotView> {
    replay_view(accounts, interview_id, account_id, -1, now)
}

/// Everything after `seq`, under the same guards.
///
/// What a reader that already holds a snapshot asks for next. Superseded frames
/// are not dropped here: a caller carrying on from a snapshot is replaying, and
/// an editor frame it never saw is not one to skip.
pub fn replay_tail(
    accounts: &Accounts,
    interview_id: &str,
    account_id: i64,
    after: i64,
    now: i64,
) -> rusqlite::Result<SnapshotView> {
    replay_view(accounts, interview_id, account_id, after.max(0), now)
}

fn replay_view(
    accounts: &Accounts,
    interview_id: &str,
    account_id: i64,
    after: i64,
    now: i64,
) -> rusqlite::Result<SnapshotView> {
    accounts.with(|connection| {
        let transaction = connection.unchecked_transaction()?;
        if !replay_open(&transaction, interview_id, account_id)? {
            return Ok(SnapshotView::NoInterview);
        }

        // No recording row is not a refusal: an interview whose recording never
        // started still has a replay, and it is this account's to read.
        let recording: Option<(String, Option<i64>, i64)> = transaction
            .query_row(
                "
        SELECT state, expires_at, quota_exceeded FROM recordings
        WHERE interview_id = ?1 AND account_id = ?2
        ",
                (interview_id, account_id),
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let mut quota_exceeded = false;
        if let Some((state, expires_at, quota)) = recording {
            quota_exceeded = quota != 0;
            if RecordingState::parse(&state) == Some(RecordingState::Deleted) {
                return Ok(SnapshotView::Deleted);
            }

            // Expiry is read here and written by the retention sweeper. A
            // deadline that has passed is a refusal whether or not the sweeper
            // has run yet, because the link outliving the file is the failure
            // this check exists for.
            if expires_at.is_some_and(|expires_at| expires_at <= now) {
                return Ok(SnapshotView::Expired);
            }
        }

        let seq: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(seq), -1) FROM replay_events WHERE interview_id = ?1",
            [interview_id],
            |row| row.get(0),
        )?;
        let superseded = ReplayKind::ALL
            .iter()
            .filter(|kind| kind.is_snapshot())
            .map(|kind| format!("'{}'", kind.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        let query = if after < 0 {
            format!(
                "
        SELECT seq, kind, at, payload FROM replay_events
        WHERE interview_id = ?1 AND seq <= ?2 AND seq > ?3
          AND (kind NOT IN ({superseded})
               OR seq = (SELECT MAX(seq) FROM replay_events newer
                         WHERE newer.interview_id = ?1
                           AND newer.kind = replay_events.kind
                           AND newer.seq <= ?2))
        ORDER BY seq
        "
            )
        } else {
            "
        SELECT seq, kind, at, payload FROM replay_events
        WHERE interview_id = ?1 AND seq <= ?2 AND seq > ?3
        ORDER BY seq
        "
            .to_string()
        };
        let mut statement = transaction.prepare(&query)?;
        let events = statement
            .query_map((interview_id, seq, after), row_to_replay_event)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        transaction.commit()?;
        Ok(SnapshotView::Ready(Snapshot {
            seq,
            events,
            quota_exceeded,
        }))
    })
}

/// Every event of an interview, in the order they were allocated.
pub fn replay_events(
    accounts: &Accounts,
    interview_id: &str,
    account_id: i64,
    after: i64,
) -> rusqlite::Result<Vec<(i64, ReplayEvent)>> {
    accounts.with(|connection| {
        // Scoped in the query rather than by the caller. The read routes are a
        // later task, and an id from a URL is the only thing they will have.
        let mut statement = connection.prepare(
            "
        SELECT seq, kind, at, payload FROM replay_events
        WHERE interview_id = ?1 AND seq > ?2
          AND interview_id IN (SELECT id FROM interviews WHERE account_id = ?3)
        ORDER BY seq
        ",
        )?;
        let rows = statement
            .query_map((interview_id, after, account_id), row_to_replay_event)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}
