//! The recording pipeline's fixed values.
//!
//! Every string and number here is part of a contract with something outside
//! this process: a LiveKit Twirp method, a route LiveKit Cloud is configured
//! to call back, a path the Egress browser fetches, an object layout the
//! transfer step walks. `docs/recording-contract.md` explains each one; this
//! is where the value itself lives, so that changing it is one edit rather
//! than a search for every place it was spelled out.

/// Where LiveKit Cloud delivers webhooks. Configured in the LiveKit project,
/// so changing it is a deployment change and not only a code change.
pub const WEBHOOK_ROUTE: &str = "/api/recording/webhook";

/// The RoomComposite custom template, served out of `web/` like every other
/// static asset. Egress fetches it over the public internet and appends its
/// own `url`, `token` and `layout` query parameters.
pub const TEMPLATE_PATH: &str = "/recording/index.html";

/// Twirp service and methods. LiveKit routes these at
/// `{https base}/twirp/{service}/{method}`, the same shape
/// `src/livekit.rs` already uses for `livekit.RoomService`.
pub const EGRESS_SERVICE: &str = "livekit.Egress";
pub const EGRESS_START_METHOD: &str = "StartRoomCompositeEgress";
pub const EGRESS_STOP_METHOD: &str = "StopEgress";

/// The staged object for one recording. One function rather than a format
/// string repeated at the upload, the delete and the acceptance check, because
/// those three disagreeing is a leaked object nobody looks for.
pub fn gcs_object_path(prefix: &str, recording_id: &str) -> String {
    format!("{}/{recording_id}.mp4", prefix.trim_end_matches('/'))
}

/// The encode. Written out rather than taken from an `EncodingOptionsPreset`
/// because no preset carries this bitrate: `H264_720P_30` is LiveKit's 3 Mbps
/// profile, and the ceiling this product agreed to is 2 Mbps.
pub const OUTPUT_WIDTH: u32 = 1280;
pub const OUTPUT_HEIGHT: u32 = 720;
pub const OUTPUT_FRAMERATE: u32 = 30;
/// Kilobits per second, which is the unit `EncodingOptions` uses.
pub const OUTPUT_VIDEO_BITRATE: u32 = 2000;
pub const OUTPUT_AUDIO_BITRATE: u32 = 128;
pub const OUTPUT_FILE_TYPE: &str = "MP4";
pub const OUTPUT_VIDEO_CODEC: &str = "H264_MAIN";
/// Not the `OPUS` default: an MP4 `EncodedFileOutput` needs AAC.
pub const OUTPUT_AUDIO_CODEC: &str = "AAC";

/// The version of the recording disclosure a candidate agreed to.
///
/// A date rather than a counter, because what is being versioned is a piece of
/// prose: `web/interview.html` shows this text, `POST /api/interviews` records
/// which text was shown, and the two are checked against each other. A page
/// left open across a deploy that changed the wording is refused rather than
/// recorded as having consented to words it never displayed.
pub const CONSENT_VERSION: &str = "2026-08-21";

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::accounts::{Accounts, blocking};
use crate::config::RecordingConfig;

/// Where a recording is, out of the eight places it can be.
///
/// The transition table in `docs/recording-contract.md` is the authority and
/// [`RecordingState::may_become`] is its only implementation. Any transition
/// absent from both is a bug, and the point of writing it down as a function is
/// that a wrong one fails a test rather than reaching the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    /// Written before the provider is called, so a process that dies during
    /// that call leaves evidence rather than nothing.
    Starting,
    Recording,
    /// A stop has been asked for; the provider has not said it is done.
    Finalizing,
    /// The provider is done and the file is being moved to its Shared Drive.
    Transferring,
    Ready,
    Failed,
    /// The media outlived the attempt to delete it. Terminal for the pipeline
    /// and an alert for a person.
    CleanupFailed,
    Deleted,
}

impl RecordingState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Recording => "recording",
            Self::Finalizing => "finalizing",
            Self::Transferring => "transferring",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::CleanupFailed => "cleanup_failed",
            Self::Deleted => "deleted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::Starting,
            Self::Recording,
            Self::Finalizing,
            Self::Transferring,
            Self::Ready,
            Self::Failed,
            Self::CleanupFailed,
            Self::Deleted,
        ]
        .into_iter()
        .find(|state| state.as_str() == value)
    }

    /// Whether the provider should still be recording.
    ///
    /// Not the same question as [`is_active`](Self::is_active), and the
    /// difference is `finalizing`: the pipeline still owes that recording work,
    /// and the provider has already been asked to stop. A job attached to a
    /// `finalizing` row is one whose stop request arrived before it had an id
    /// to name, and it has to be stopped rather than left running.
    pub fn provider_should_run(self) -> bool {
        matches!(self, Self::Starting | Self::Recording)
    }

    /// Whether the pipeline still owes this recording work.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Recording | Self::Finalizing | Self::Transferring
        )
    }

    /// The transition table, as a function.
    ///
    /// Written as "what may follow" rather than "what may precede" because
    /// every caller has a current state and a proposed one, and because a
    /// self-transition has to be allowed: a webhook that arrives twice, or a
    /// stop asked for twice, must be a no-op rather than a refusal.
    pub fn may_become(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        match self {
            Self::Starting => matches!(next, Self::Recording | Self::Finalizing | Self::Failed),

            // Straight to `transferring` as well, because the provider can
            // finish without being asked: a room that ended, a duration limit.
            // The webhook then says the file is ready while this side still
            // says `recording`, and refusing that transition threw the file
            // away and failed the recording fifteen minutes later.
            Self::Recording => {
                matches!(next, Self::Finalizing | Self::Transferring | Self::Failed)
            }

            // Straight to `failed` as well as forward: a recording that ends
            // with no file is finished and has nothing to transfer.
            Self::Finalizing => matches!(next, Self::Transferring | Self::Failed),
            Self::Transferring => matches!(next, Self::Ready | Self::Failed),

            // Both ends of the pipeline reach deletion, because a failed
            // recording can still have left bytes in the staging bucket.
            Self::Ready | Self::Failed => {
                matches!(next, Self::Deleted | Self::CleanupFailed)
            }
            Self::CleanupFailed => matches!(next, Self::Deleted),
            Self::Deleted => false,
        }
    }
}

/// What a start request needs, assembled once so the adapter cannot invent any
/// of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartEgress {
    pub room_name: String,
    pub template_base_url: String,
    pub bucket: String,
    pub filepath: String,
    pub service_account_json: String,
    pub bitrate: u32,
}

impl StartEgress {
    /// The `StartRoomCompositeEgress` body, in the casing the provider uses on
    /// the way in. `tests/fixtures/recording/start-egress-request.json` is the
    /// same shape and is what pins it.
    pub fn body(&self) -> Value {
        json!({
            "room_name": self.room_name,
            "custom_base_url": format!("{}{TEMPLATE_PATH}", self.template_base_url.trim_end_matches('/')),
            "audio_only": false,
            "video_only": false,
            "advanced": {
                "width": OUTPUT_WIDTH,
                "height": OUTPUT_HEIGHT,
                "framerate": OUTPUT_FRAMERATE,
                "video_codec": OUTPUT_VIDEO_CODEC,
                "video_bitrate": self.bitrate,
                "audio_codec": OUTPUT_AUDIO_CODEC,
                "audio_bitrate": OUTPUT_AUDIO_BITRATE,
            },
            "file_outputs": [{
                "file_type": OUTPUT_FILE_TYPE,
                "filepath": self.filepath,
                "disable_manifest": true,
                "gcp": {
                    "credentials": self.service_account_json,
                    "bucket": self.bucket,
                },
            }],
        })
    }
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The provider, as the pipeline needs it.
///
/// A trait rather than a direct call because every failure this pipeline has to
/// survive is the provider's: a start that refuses, a stop that never arrives,
/// a webhook that goes missing. None of those can be produced against LiveKit
/// on demand, and a test that cannot produce them is a test of the happy path.
///
/// Boxed futures rather than `async fn`, because the whole point is `dyn`.
pub trait RecordingProvider: Send + Sync + 'static {
    /// The provider's egress id, on success.
    fn start<'a>(&'a self, request: &'a StartEgress) -> BoxFuture<'a, Result<String, String>>;
    /// The room comes along because a recording belongs to one LiveKit
    /// project, and an egress id alone does not say which. `StopEgress` itself
    /// does not want it; the adapter does, to know whose credentials to sign
    /// with.
    fn stop<'a>(
        &'a self,
        egress_id: &'a str,
        room_name: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;
    /// An Egress job already running for this room, if there is one.
    ///
    /// Asked before every retry. A start that succeeded and whose id this side
    /// failed to write down leaves a job nothing knows about, and starting
    /// again would make a second one: two bills, two files, and neither
    /// stoppable by a candidate withdrawing consent.
    fn active_for_room<'a>(
        &'a self,
        room_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, String>>;
}

/// Epoch seconds, injected.
///
/// The retry schedule is minutes long and the stale-row sweeper is
/// quarter-hours long, and a test that waited for either would be a test nobody
/// runs.
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        crate::current_epoch_seconds() as i64
    }
}

/// LiveKit Cloud, over the same Twirp shape `src/livekit.rs` already uses for
/// `livekit.RoomService`.
///
/// It holds the pool rather than one project's credentials, because rooms are
/// routed by provider id and a recording belongs to whichever project holds its
/// room. Signing every Egress call with the primary project's key would work
/// for a single-provider deployment and record nothing at all for the others.
pub struct LiveKitEgress {
    pub pool: crate::config::ProviderPool,
    pub room_prefix: String,
    /// The configured override, which pins one project deliberately.
    pub livekit: Option<crate::config::RecordingLivekit>,
}

impl LiveKitEgress {
    /// The credentials for the project that holds this room.
    ///
    /// The override replaces them only when it names that same project. It
    /// exists so recording can use a key scoped to `roomRecord` rather than the
    /// one that mints candidate tokens; it is not a redirect, and using it for
    /// a room on another project would sign calls with credentials that cannot
    /// see the room.
    fn credentials(&self, room_name: &str) -> Result<(String, String, String), String> {
        let provider = self
            .pool
            .for_room(room_name, &self.room_prefix)
            .ok_or_else(|| "no LiveKit project is configured for that room".to_string())?;
        if let Some(livekit) = &self.livekit
            && crate::config::same_livekit_project(&livekit.url, &provider.url)
        {
            return Ok((
                livekit.url.clone(),
                livekit.api_key.clone(),
                livekit.api_secret.clone(),
            ));
        }
        Ok((
            provider.url.clone(),
            provider.api_key.clone(),
            provider.api_secret.clone(),
        ))
    }

    async fn call(&self, method: &str, room_name: &str, body: Value) -> Result<Value, String> {
        let (url, api_key, api_secret) = self.credentials(room_name)?;
        let token = crate::token::livekit_egress_token(
            &api_key,
            &api_secret,
            room_name,
            crate::current_epoch_seconds(),
        )
        .map_err(|error| format!("could not sign the egress credential: {error}"))?;
        let base = url
            .trim_end_matches('/')
            .replacen("wss://", "https://", 1)
            .replacen("ws://", "http://", 1);
        let response = crate::http_client()
            .post(format!("{base}/twirp/{EGRESS_SERVICE}/{method}"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            // The URL is never quoted back: it can carry a query string, and
            // this message reaches stderr.
            .map_err(|error| {
                format!(
                    "LiveKit {method} could not be reached: {}",
                    error
                        .status()
                        .map_or_else(|| "no response".to_string(), |status| status.to_string())
                )
            })?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("LiveKit {method} failed: {status} {text}"));
        }
        serde_json::from_str(&text)
            .map_err(|error| format!("LiveKit {method} returned invalid JSON: {error}"))
    }
}

/// `ListEgress`, kept out of the fixed method list because it is not part of
/// the start/stop contract. It is how this side recovers from having lost an id
/// it was given.
const EGRESS_LIST_METHOD: &str = "ListEgress";

impl RecordingProvider for LiveKitEgress {
    fn start<'a>(&'a self, request: &'a StartEgress) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let response = self
                .call(EGRESS_START_METHOD, &request.room_name, request.body())
                .await?;

            // snake_case on the way back, unlike the webhook. The contract
            // document is where that asymmetry is explained.
            response
                .get("egress_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .ok_or_else(|| "LiveKit start returned no egress_id".to_string())
        })
    }

    fn stop<'a>(
        &'a self,
        egress_id: &'a str,
        room_name: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.call(
                EGRESS_STOP_METHOD,
                room_name,
                json!({ "egress_id": egress_id }),
            )
            .await
            .map(|_| ())
        })
    }

    fn active_for_room<'a>(
        &'a self,
        room_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(async move {
            let response = self
                .call(
                    EGRESS_LIST_METHOD,
                    room_name,
                    json!({ "room_name": room_name, "active": true }),
                )
                .await?;
            Ok(response
                .get("items")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("egress_id"))
                .and_then(Value::as_str)
                .map(str::to_string))
        })
    }
}

/// Everything the pipeline needs that is not the database.
///
/// One struct because the three travel together and a handler that has any of
/// them needs all of them.
#[derive(Clone)]
pub struct Recorder {
    pub provider: Arc<dyn RecordingProvider>,
    pub clock: Arc<dyn Clock>,
    pub config: RecordingConfig,
}

/// A recording, as the routes and the sweeper read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recording {
    pub id: String,
    pub account_id: i64,
    pub interview_id: String,
    pub room_name: Option<String>,
    pub egress_id: Option<String>,
    pub state: RecordingState,
    pub error: Option<String>,
    pub retries: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

const SELECT_RECORDING: &str = "
        SELECT id, account_id, interview_id, room_name, egress_id, state, error, retries,
               created_at, updated_at
        FROM recordings
";

fn row_to_recording(row: &rusqlite::Row<'_>) -> rusqlite::Result<Recording> {
    let state: String = row.get(5)?;
    Ok(Recording {
        id: row.get(0)?,
        account_id: row.get(1)?,
        interview_id: row.get(2)?,
        room_name: row.get(3)?,
        egress_id: row.get(4)?,

        // An unreadable state is not a state. It comes back as `Failed` rather
        // than panicking, because a row written by a newer binary must not take
        // the sweeper down with it.
        state: RecordingState::parse(&state).unwrap_or(RecordingState::Failed),
        error: row.get(6)?,
        retries: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

/// Why a start was refused. Each one is a different sentence to the candidate
/// and a different thing for an operator to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartRefusal {
    /// No such interview, not this account's, or consent has been withdrawn.
    NoConsent,
    /// The interview never claimed a room, so there is nothing to record.
    NoRoom,
    KillSwitch,
    /// This account already has a recording the pipeline owes work on.
    AlreadyActive,
    Database,
}

/// The row that says a recording exists, written before the provider is called.
///
/// The `bool` is whether this call inserted the row, and it is the whole answer
/// to "may I call the provider". Two concurrent starts both see a `starting`
/// row with no egress id, and if both treated that as work to do there would be
/// two Egress jobs. Only the caller that inserted it starts; a row whose
/// creator never got that far is the sweeper's.
///
/// Every rule the insert has to obey is inside the insert. Consent, ownership
/// and a claimed room are its `WHERE`; one-per-interview and
/// one-active-per-account are unique indexes. Reading them first and inserting
/// second is serialized by this process's one connection and by nothing across
/// two, and the reads that remain exist only to say *why* an insert that landed
/// nowhere landed nowhere.
pub fn begin_recording(
    accounts: &Accounts,
    clock: &dyn Clock,
    interview_id: &str,
    account_id: i64,
    recording_id: &str,
    recipient_email: &str,
    kill_switch: bool,
) -> Result<(Recording, bool), StartRefusal> {
    let now = clock.now();
    accounts
        .with(|connection| {
            // The switch is read before anything else, including before an
            // existing recording is handed back. An operator who throws it
            // means every start, not only the ones that have not happened yet.
            if kill_switch {
                return Ok(Err(StartRefusal::KillSwitch));
            }

            // Consent, ownership and a claimed room are the insert's own
            // `WHERE`, so nothing can change between checking them and writing.
            let inserted = connection.execute(
                "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at
        )
        SELECT ?1, ?2, ?3, interviews.room_name, ?3, ?4, 'starting', ?5, ?5
        FROM interviews
        WHERE interviews.id = ?3
          AND interviews.account_id = ?2
          AND interviews.consent_withdrawn_at IS NULL
          AND interviews.room_name IS NOT NULL
        ",
                (
                    recording_id,
                    account_id,
                    interview_id,
                    recipient_email,
                    now,
                ),
            );
            if inserted.as_ref().is_ok_and(|rows| *rows > 0) {
                let recording = connection.query_row(
                    &format!("{SELECT_RECORDING} WHERE id = ?1"),
                    [recording_id],
                    row_to_recording,
                )?;
                return Ok(Ok((recording, true)));
            }

            // From here on, everything is about explaining a refusal. An
            // existing recording for this interview is the common one, and it
            // is not a refusal at all: a repeated start finds the first
            // caller's row rather than making a second Egress job.
            let existing = connection
                .query_row(
                    &format!("{SELECT_RECORDING} WHERE interview_id = ?1"),
                    [interview_id],
                    row_to_recording,
                )
                .map(Some)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    error => Err(error),
                })?;
            if let Some(existing) = existing {
                return Ok(if existing.account_id == account_id {
                    Ok((existing, false))
                } else {
                    Err(StartRefusal::NoConsent)
                });
            }

            let interview: Option<(Option<String>, Option<i64>)> = connection
                .query_row(
                    "SELECT room_name, consent_withdrawn_at FROM interviews WHERE id = ?1 AND account_id = ?2",
                    (interview_id, account_id),
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map(Some)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    error => Err(error),
                })?;
            let Some((room_name, withdrawn)) = interview else {
                return Ok(Err(StartRefusal::NoConsent));
            };
            if withdrawn.is_some() {
                return Ok(Err(StartRefusal::NoConsent));
            }
            if room_name.is_none() {
                return Ok(Err(StartRefusal::NoRoom));
            }

            let active: i64 = connection.query_row(
                "
        SELECT COUNT(*) FROM recordings
        WHERE account_id = ?1
          AND state IN ('starting', 'recording', 'finalizing', 'transferring')
        ",
                [account_id],
                |row| row.get(0),
            )?;
            if active > 0 {
                return Ok(Err(StartRefusal::AlreadyActive));
            }

            // The insert obeyed every rule this function knows and still did
            // not land, so whatever stopped it is a database problem rather
            // than a refusal.
            Err(inserted.err().unwrap_or(rusqlite::Error::QueryReturnedNoRows))
        })
        .unwrap_or_else(|error| {
            eprintln!("could not begin a recording: {error}");
            Err(StartRefusal::Database)
        })
}

/// Records the provider's answer.
///
/// The id is written whatever the state, and the state moves only from
/// `starting`. Guarding the id on the state as well was a leak: a stop that
/// landed first left the row in `finalizing`, the id was discarded, and the
/// Egress job it named could no longer be stopped by anything.
///
/// It is never overwritten. A slow first attempt and a sweeper retry can both
/// come back with a job, and the second to arrive must not replace the first:
/// that would leave a job nothing can name. `false` means somebody else's id is
/// already there, and the caller has a job of its own to stop.
pub fn record_egress_id(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
    egress_id: &str,
) -> rusqlite::Result<bool> {
    let now = clock.now();
    accounts.with(|connection| {
        let changed = connection.execute(
            "
        UPDATE recordings
        SET egress_id = ?2,
            state = CASE WHEN state = 'starting' THEN 'recording' ELSE state END,
            updated_at = ?3
        WHERE id = ?1 AND egress_id IS NULL
        ",
            (recording_id, egress_id, now),
        )?;
        if changed > 0 {
            return Ok(true);
        }

        // Already ours is not a loss. A retry that adopted the same job, or an
        // answer that arrived twice, should not send the caller off to stop it.
        let current: Option<String> = connection.query_row(
            "SELECT egress_id FROM recordings WHERE id = ?1",
            [recording_id],
            |row| row.get(0),
        )?;
        Ok(current.as_deref() == Some(egress_id))
    })
}

/// Moves a recording, refusing any move the table does not have.
///
/// Returns the state the row is in afterwards and whether this call is what put
/// it there. The state is not always the one that was asked for: a duplicate
/// webhook asks for a transition that has already happened, and the answer to
/// that is the current state rather than an error.
///
/// The `bool` is what makes a concurrent stop safe. Two callers can both find a
/// recording active, and only the one that moved it should talk to the
/// provider; the other has nothing left to do.
pub fn transition(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
    next: RecordingState,
    error: Option<&str>,
) -> rusqlite::Result<Option<(RecordingState, bool)>> {
    let now = clock.now();
    accounts.with(|connection| {
        let current: Option<String> = connection
            .query_row(
                "SELECT state FROM recordings WHERE id = ?1",
                [recording_id],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            })?;
        let Some(current) = current.as_deref().and_then(RecordingState::parse) else {
            return Ok(None);
        };

        // Already there. The transition is allowed, and nothing moved, which is
        // the difference a duplicate stop rides on: the second caller has
        // nothing to tell the provider.
        if current == next {
            return Ok(Some((current, false)));
        }
        if !current.may_become(next) {
            return Ok(Some((current, false)));
        }

        // Guarded on the state it was read in, so two callers racing on one
        // recording cannot both come away believing they moved it.
        let changed = connection.execute(
            "
        UPDATE recordings
        SET state = ?2, error = COALESCE(?3, error), updated_at = ?4
        WHERE id = ?1 AND state = ?5
        ",
            (recording_id, next.as_str(), error, now, current.as_str()),
        )?;
        if changed > 0 {
            return Ok(Some((next, true)));
        }

        // Somebody else moved it first, so the state to report is theirs and
        // not the one that was asked for. Reporting `next` here told `/end`
        // that a recording was `finalizing` when it had already failed.
        let actual: String = connection.query_row(
            "SELECT state FROM recordings WHERE id = ?1",
            [recording_id],
            |row| row.get(0),
        )?;
        Ok(RecordingState::parse(&actual).map(|state| (state, false)))
    })
}

pub fn recording_by_id(
    accounts: &Accounts,
    recording_id: &str,
) -> rusqlite::Result<Option<Recording>> {
    accounts.with(|connection| {
        connection
            .query_row(
                &format!("{SELECT_RECORDING} WHERE id = ?1"),
                [recording_id],
                row_to_recording,
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            })
    })
}

pub fn recording_for_interview(
    accounts: &Accounts,
    interview_id: &str,
    account_id: i64,
) -> rusqlite::Result<Option<Recording>> {
    accounts.with(|connection| {
        connection
            .query_row(
                &format!("{SELECT_RECORDING} WHERE interview_id = ?1 AND account_id = ?2"),
                (interview_id, account_id),
                row_to_recording,
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            })
    })
}

pub fn recording_for_room(
    accounts: &Accounts,
    room_name: &str,
) -> rusqlite::Result<Option<Recording>> {
    accounts.with(|connection| {
        connection
            .query_row(
                &format!("{SELECT_RECORDING} WHERE room_name = ?1"),
                [room_name],
                row_to_recording,
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            })
    })
}

pub fn recording_for_egress(
    accounts: &Accounts,
    egress_id: &str,
) -> rusqlite::Result<Option<Recording>> {
    accounts.with(|connection| {
        connection
            .query_row(
                &format!("{SELECT_RECORDING} WHERE egress_id = ?1"),
                [egress_id],
                row_to_recording,
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            })
    })
}

/// Rows the pipeline still owes work on that nothing has touched for a while.
///
/// "Touched", not "created": a recording that has been running for forty
/// minutes is a recording, and one whose row has not moved for fifteen is a
/// process that died holding it.
pub fn stale_active(
    accounts: &Accounts,
    older_than: Option<i64>,
) -> rusqlite::Result<Vec<Recording>> {
    accounts.with(|connection| {
        // `None` means every active row, which is what the kill switch wants.
        // Written as a null-checked comparison rather than as a sentinel
        // timestamp, because `updated_at <= i64::MAX` reads as a bug even when
        // it is not one.
        let mut statement = connection.prepare(&format!(
            "{SELECT_RECORDING}
        WHERE state IN ('starting', 'recording', 'finalizing', 'transferring')
          AND (?1 IS NULL OR updated_at <= ?1)
        "
        ))?;
        let rows = statement
            .query_map([older_than], row_to_recording)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Whether this webhook still needs dealing with.
///
/// `false` means it has already been applied. LiveKit retries anything it did
/// not get a 2xx for, and a retry is a fresh, validly signed message with a new
/// `createdAt` and the same `id`, so the id is the only thing that can tell
/// them apart.
///
/// An event that was claimed and never applied comes back `true`. Deleting the
/// claim on failure was the first answer and it had a hole: if the delete
/// itself failed, or the process died between the two, the id stayed and the
/// redelivery did nothing. A null `applied_at` is an attempt that did not
/// finish, and there is no second write to lose.
pub fn claim_webhook_event(
    accounts: &Accounts,
    event_id: &str,
    received_at: i64,
) -> rusqlite::Result<bool> {
    accounts.with(|connection| {
        connection.execute(
            "
        INSERT INTO recording_events (event_id, received_at) VALUES (?1, ?2)
        ON CONFLICT(event_id) DO NOTHING
        ",
            (event_id, received_at),
        )?;
        let applied: Option<i64> = connection.query_row(
            "SELECT applied_at FROM recording_events WHERE event_id = ?1",
            [event_id],
            |row| row.get(0),
        )?;
        Ok(applied.is_none())
    })
}

/// Marks an event dealt with, so its retries are no-ops.
pub fn mark_webhook_applied(
    accounts: &Accounts,
    event_id: &str,
    applied_at: i64,
) -> rusqlite::Result<()> {
    accounts.with(|connection| {
        connection.execute(
            "UPDATE recording_events SET applied_at = ?2 WHERE event_id = ?1",
            (event_id, applied_at),
        )?;
        Ok(())
    })
}

/// How long the ledger remembers. LiveKit gives up retrying long before this,
/// and a table that only grows is a table that eventually matters.
pub const EVENT_RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;

pub fn prune_webhook_events(accounts: &Accounts, before: i64) -> rusqlite::Result<usize> {
    accounts.with(|connection| {
        connection.execute(
            "DELETE FROM recording_events WHERE received_at < ?1",
            [before],
        )
    })
}

/// Moves the row and then stops the provider, in that order.
///
/// The order matters and is not obvious. Calling the provider first meant two
/// callers racing on one recording, `/end` and a `room_finished` webhook say,
/// both sent a stop, and a process that died between a successful stop and the
/// write left a row that stayed active forever. Moving first makes the row the
/// record of intent, and only the caller that moved it talks to the provider.
///
/// A failed stop leaves the row where it was moved to and the job still
/// running, which the sweeper finds once the row goes stale. That is the cost
/// of this order and it is the smaller one: the alternative loses the intent.
/// The `bool` is whether this call moved the row, and it is reported even when
/// the provider then refuses. The move happened; a caller that only audited on
/// `Ok` lost the record of a state change that is now in the database.
pub async fn stop_recording(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    recording: &Recording,
    next: RecordingState,
    reason: Option<&str>,
) -> Result<(RecordingState, bool), StopFailure> {
    let (state, mine) = {
        let accounts = accounts.clone();
        let clock = recorder.clock.clone();
        let recording_id = recording.id.clone();
        let reason = reason.map(str::to_string);
        blocking(move || {
            transition(
                &accounts,
                clock.as_ref(),
                &recording_id,
                next,
                reason.as_deref(),
            )
        })
        .await
        .map_err(|error| StopFailure {
            state: recording.state,
            moved: false,
            error: format!("could not record the stop: {error}"),
        })?
        .ok_or_else(|| StopFailure {
            state: recording.state,
            moved: false,
            error: "the recording went away while it was being stopped".to_string(),
        })?
    };
    if !mine {
        // Somebody else moved it, and whatever they did includes telling the
        // provider. There is nothing left here, and the caller has to be told
        // that rather than counting a move it did not make.
        return Ok((state, false));
    }
    if let Some(egress_id) = &recording.egress_id {
        if let Err(error) = recorder
            .provider
            .stop(
                egress_id,
                recording.room_name.as_deref().unwrap_or_default(),
            )
            .await
        {
            // The row moved and the provider did not agree. Both go back, so
            // the caller can audit the change it made and the sweeper can keep
            // asking about the job.
            return Err(StopFailure {
                state,
                moved: true,
                error,
            });
        }

        // Written only after the provider agreed. A row with an egress id and
        // no `stopped_at` is a job that still needs stopping, which is the only
        // thing that can say so once the state is `failed` and terminal.
        let accounts = accounts.clone();
        let clock = recorder.clock.clone();
        let id = recording.id.clone();
        if let Err(error) = blocking(move || mark_stopped(&accounts, clock.as_ref(), &id)).await {
            eprintln!(
                "recording {} was stopped and the row does not know: {error}",
                recording.id
            );
        }
    }
    Ok((state, true))
}

/// Stops a job whose recording stopped being active while the provider was
/// answering.
///
/// Both paths that hand a fresh egress id to a row need this: the start route
/// and the sweeper's retry. A candidate can withdraw consent, a room can
/// finish, a tab can close, all while `StartRoomCompositeEgress` is still in
/// flight, and the row that comes back is terminal with a live job attached.
pub async fn settle_new_egress(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    recording_id: &str,
    egress_id: &str,
) {
    let settled = {
        let accounts = accounts.clone();
        let recording_id = recording_id.to_string();
        blocking(move || recording_by_id(&accounts, &recording_id)).await
    };
    let Ok(Some(settled)) = settled else { return };
    if settled.state.provider_should_run() {
        return;
    }
    match recorder
        .provider
        .stop(egress_id, settled.room_name.as_deref().unwrap_or_default())
        .await
    {
        Ok(()) => {
            let accounts = accounts.clone();
            let clock = recorder.clock.clone();
            let id = settled.id.clone();
            let _ = blocking(move || mark_stopped(&accounts, clock.as_ref(), &id)).await;
        }
        Err(error) => eprintln!(
            "recording {} was stopped before it started and its job is still running: {error}",
            settled.id
        ),
    }
}

/// A stop whose row moved and whose provider did not agree.
///
/// Both halves matter to the caller: the state change is real and has to be
/// audited, and the job is still running and has to be swept for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopFailure {
    pub state: RecordingState,
    pub moved: bool,
    pub error: String,
}

impl std::fmt::Display for StopFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.error)
    }
}

/// Writes down that the provider is no longer running this job.
pub fn mark_stopped(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
) -> rusqlite::Result<()> {
    let now = clock.now();
    accounts.with(|connection| {
        connection.execute(
            "UPDATE recordings SET stopped_at = ?2 WHERE id = ?1 AND stopped_at IS NULL",
            (recording_id, now),
        )?;
        Ok(())
    })
}

/// Recordings whose provider job should not be running and is not known to have
/// stopped.
///
/// Two cases need it. A candidate withdraws consent, the row moves to `failed`,
/// and the stop fails: `failed` is terminal, so no state will ever say the job
/// is still going, and the missing `stopped_at` is what does. And a stop that
/// arrived before the recording had an egress id leaves a `finalizing` row that
/// was never able to name the job it wanted stopped.
///
/// `room_name IS NOT NULL` because the adapter resolves credentials from the
/// room: a tombstoned row has given its room name up, and asking to stop a job
/// without one fails every minute forever. A tombstone also means the media is
/// already gone, so there is nothing left to stop.
pub fn unstopped_jobs(accounts: &Accounts) -> rusqlite::Result<Vec<Recording>> {
    accounts.with(|connection| {
        let mut statement = connection.prepare(&format!(
            "{SELECT_RECORDING}
        WHERE egress_id IS NOT NULL
          AND stopped_at IS NULL
          AND room_name IS NOT NULL
          AND state NOT IN ('starting', 'recording')
        "
        ))?;
        let rows = statement
            .query_map([], row_to_recording)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Takes the next retry, so that only one sweeper makes it.
///
/// Guarded on the `updated_at` the caller read, which is what stops two
/// processes from both reconciling and both starting. Touching `updated_at`
/// also restarts the staleness clock, because an attempt is now in flight.
pub fn claim_retry(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
    seen_updated_at: i64,
) -> rusqlite::Result<bool> {
    let now = clock.now();
    accounts.with(|connection| {
        let changed = connection.execute(
            "
        UPDATE recordings SET updated_at = ?3
        WHERE id = ?1
          AND updated_at = ?2
          AND state = 'starting'
          AND egress_id IS NULL
        ",
            (recording_id, seen_updated_at, now),
        )?;
        Ok(changed > 0)
    })
}

/// One minute, five, then fifteen.
///
/// A provider that refused once is usually busy; a provider that has refused
/// three times over sixteen minutes is not going to answer for this interview,
/// which will be over by then.
pub const RETRY_BACKOFF_SECONDS: [i64; 3] = [60, 300, 900];

/// How long to wait before retry number `retries + 1`, or `None` when there is
/// no retry left to make.
///
/// `retries` counts retries, and the first attempt is not one. That is why the
/// route that makes the first attempt does not increment it when the provider
/// refuses: doing so made the first retry five minutes out rather than one, and
/// the schedule this names is one minute, then five, then fifteen.
pub fn retry_delay(retries: i64) -> Option<i64> {
    usize::try_from(retries)
        .ok()
        .and_then(|index| RETRY_BACKOFF_SECONDS.get(index))
        .copied()
}

/// How long a row may sit untouched before the pipeline decides nobody owns it.
///
/// Untouched, not unfinished: a recording that has been running for forty
/// minutes is a recording. One whose row has not moved for fifteen is a process
/// that died holding it.
pub const STALE_SECONDS: i64 = 900;

/// The slack on top of the interview's own maximum, before a `recording` row is
/// treated as abandoned.
///
/// `recording` is the one state with no periodic signal. LiveKit sends
/// `egress_updated` on a status change and not as a heartbeat, so a healthy
/// forty-minute interview touches its row once at the start and not again;
/// applying the fifteen-minute rule to it failed every recording that outlived
/// its own first quarter of an hour. What a recording cannot legitimately do is
/// outlive `CODETRIAL_RECORDING_MAX_MINUTES`, so that is the bound, plus enough
/// for the provider to finish writing and say so.
pub const RECORDING_STALE_SLACK_SECONDS: i64 = 300;

/// How long this state may sit untouched.
pub fn stale_after(state: RecordingState, max_minutes: u32) -> i64 {
    match state {
        RecordingState::Recording => i64::from(max_minutes) * 60 + RECORDING_STALE_SLACK_SECONDS,
        _ => STALE_SECONDS,
    }
}

pub fn bump_retry(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
) -> rusqlite::Result<()> {
    let now = clock.now();
    accounts.with(|connection| {
        connection.execute(
            "UPDATE recordings SET retries = retries + 1, updated_at = ?2 WHERE id = ?1",
            (recording_id, now),
        )?;
        Ok(())
    })
}

/// What one sweep did, so the caller can say so rather than sweeping silently.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepOutcome {
    pub retried: usize,
    pub failed: usize,
}

/// Resolves recordings nobody is looking after.
///
/// Run at startup, because a process that died mid-interview leaves rows that
/// no request will ever touch again, and on an interval, because the retry
/// schedule is the only thing that turns a provider's bad minute into a
/// recording rather than a failure.
///
/// The kill switch is handled here too. It refuses new starts at
/// [`begin_recording`], and an operator who throws it while interviews are
/// running means the running ones as well.
pub async fn sweep_recordings(accounts: &Arc<Accounts>, recorder: &Recorder) -> SweepOutcome {
    let now = recorder.clock.now();
    let kill_switch = recorder.config.kill_switch;
    let cutoff = if kill_switch { None } else { Some(now - 60) };
    let stale = {
        let accounts = accounts.clone();
        match blocking(move || stale_active(&accounts, cutoff)).await {
            Ok(stale) => stale,
            Err(error) => {
                eprintln!("WARNING: recording sweep could not read its work: {error}");
                return SweepOutcome::default();
            }
        }
    };

    // Bounded, because a ledger that only grows is a table that eventually
    // matters. LiveKit gives up retrying long before this.
    {
        let accounts = accounts.clone();
        let before = now - EVENT_RETENTION_SECONDS;
        if let Err(error) = blocking(move || prune_webhook_events(&accounts, before)).await {
            eprintln!("WARNING: could not prune the webhook ledger: {error}");
        }
    }

    let mut outcome = SweepOutcome::default();
    for recording in stale {
        let age = now - recording.updated_at;
        if kill_switch {
            if moved_by_this_call(
                &recording.id,
                stop_recording(
                    accounts,
                    recorder,
                    &recording,
                    RecordingState::Failed,
                    Some(Failure::KillSwitch.as_str()),
                )
                .await,
            ) {
                // Inside, like the other sweep paths. Two sweepers reading the
                // same row both reported a kill, and one of them killed
                // nothing.
                audit(
                    "recording_killed",
                    &recording.id,
                    &[
                        ("reason", Failure::KillSwitch.as_str()),
                        ("recovery", Failure::KillSwitch.recovery()),
                    ],
                );
                outcome.failed += 1;
            }
            continue;
        }

        // A start that never got an egress id is the one case worth another
        // provider call: everything else is waiting on the provider to speak,
        // and asking again would not make it.
        let retry = recording.state == RecordingState::Starting
            && recording.egress_id.is_none()
            && retry_delay(recording.retries).is_some_and(|delay| age >= delay);
        if retry {
            if retry_start(accounts, recorder, &recording).await {
                outcome.retried += 1;
            }
            continue;
        }

        // Out of retries and still with nothing to show for them. This is its
        // own ending rather than the stale one: the provider refused every
        // time, which is a different thing from nobody having said anything,
        // and `Failure::Start` is the word for it.
        if recording.state == RecordingState::Starting
            && recording.egress_id.is_none()
            && retry_delay(recording.retries).is_none()
        {
            if moved_by_this_call(
                &recording.id,
                stop_recording(
                    accounts,
                    recorder,
                    &recording,
                    RecordingState::Failed,
                    Some(Failure::Start.as_str()),
                )
                .await,
            ) {
                // After the transition. Two sweepers can read the same row, and
                // an audit line written first says a recording failed on the
                // strength of a move this sweep did not make.
                audit(
                    "recording_failed",
                    &recording.id,
                    &[
                        ("reason", Failure::Start.as_str()),
                        ("recovery", Failure::Start.recovery()),
                    ],
                );
                outcome.failed += 1;
            }
            continue;
        }
        if age < stale_after(recording.state, recorder.config.max_minutes) {
            continue;
        }

        // A delivery that already failed keeps its own reason. Overwriting it
        // with `abandoned` would change the recovery from "retry the delivery"
        // to "record again", for a recording whose media may still exist.
        let already = recording
            .error
            .as_deref()
            .and_then(Failure::parse)
            .filter(|failure| *failure == Failure::Drive);
        let reason = already.unwrap_or(Failure::LostWebhook);
        if moved_by_this_call(
            &recording.id,
            stop_recording(
                accounts,
                recorder,
                &recording,
                RecordingState::Failed,
                // `None` where the row already says why: `transition`
                // coalesces, and a reason passed here would replace it.
                already.is_none().then_some(reason.as_str()),
            )
            .await,
        ) {
            // After the transition, and only when this sweep made it. Two
            // sweepers reading the same row both reported an abandonment, and
            // one of them had moved nothing.
            audit(
                "recording_abandoned",
                &recording.id,
                &[
                    ("state", recording.state.as_str()),
                    ("reason", reason.as_str()),
                    ("recovery", reason.recovery()),
                    ("age_seconds", &age.to_string()),
                ],
            );
            outcome.failed += 1;
        }
    }

    // Jobs this side has finished with that the provider is still running. The
    // stop that was supposed to end them failed after the row had already
    // moved, and `failed` is terminal, so nothing else will ever ask again.
    let orphans = {
        let accounts = accounts.clone();
        blocking(move || unstopped_jobs(&accounts))
            .await
            .unwrap_or_default()
    };
    for orphan in orphans {
        let Some(egress_id) = &orphan.egress_id else {
            continue;
        };
        match recorder
            .provider
            .stop(egress_id, orphan.room_name.as_deref().unwrap_or_default())
            .await
        {
            Ok(()) => {
                let accounts = accounts.clone();
                let clock = recorder.clock.clone();
                let id = orphan.id.clone();
                let _ = blocking(move || mark_stopped(&accounts, clock.as_ref(), &id)).await;
            }
            Err(error) => {
                audit(
                    "recording_still_running",
                    &orphan.id,
                    &[("error", &error), ("action", "stop_by_hand")],
                );
            }
        }
    }
    outcome
}

async fn retry_start(accounts: &Arc<Accounts>, recorder: &Recorder, recording: &Recording) -> bool {
    // Taken before anything is asked of the provider. Two sweepers can read the
    // same stale row, both find no job for the room, and both start one; the
    // unique index guards row creation and says nothing about who owns a retry.
    let claimed = {
        let accounts = accounts.clone();
        let clock = recorder.clock.clone();
        let id = recording.id.clone();
        let seen = recording.updated_at;
        blocking(move || claim_retry(&accounts, clock.as_ref(), &id, seen)).await
    };
    if !claimed.unwrap_or(false) {
        return false;
    }

    let room_name = recording.room_name.clone().unwrap_or_default();

    // Asked before starting, not after failing. A start that succeeded and
    // whose id this side never wrote down leaves a job nothing knows about;
    // starting again would make a second one, and neither would be stoppable by
    // a candidate withdrawing consent.
    match recorder.provider.active_for_room(&room_name).await {
        Ok(Some(egress_id)) => {
            eprintln!(
                "recording {} already had an Egress job; adopting it rather than starting a second",
                recording.id
            );
            let stored = {
                let accounts = accounts.clone();
                let clock = recorder.clock.clone();
                let id = recording.id.clone();
                let egress_id = egress_id.clone();
                blocking(move || record_egress_id(&accounts, clock.as_ref(), &id, &egress_id))
                    .await
                    .unwrap_or(false)
            };
            if stored {
                settle_new_egress(accounts, recorder, &recording.id, &egress_id).await;
            }
            return stored;
        }
        Ok(None) => {}
        Err(error) => {
            // Not knowing is not permission to start a second one.
            eprintln!(
                "recording {} could not be reconciled with the provider: {error}",
                recording.id
            );
            let accounts = accounts.clone();
            let clock = recorder.clock.clone();
            let id = recording.id.clone();
            let _ = blocking(move || bump_retry(&accounts, clock.as_ref(), &id)).await;
            return false;
        }
    }

    let request = StartEgress {
        room_name: room_name.clone(),
        template_base_url: recorder.config.template_base_url.clone(),
        bucket: recorder.config.gcs_bucket.clone(),
        filepath: gcs_object_path(&recorder.config.gcs_prefix, &recording.id),
        service_account_json: recorder.config.service_account_json.clone(),
        bitrate: recorder.config.bitrate,
    };
    let clock = recorder.clock.clone();
    match recorder.provider.start(&request).await {
        Ok(egress_id) => {
            let stored = {
                let accounts = accounts.clone();
                let id = recording.id.clone();
                let egress_id = egress_id.clone();
                blocking(move || record_egress_id(&accounts, clock.as_ref(), &id, &egress_id))
                    .await
                    .unwrap_or(false)
            };
            if stored {
                // The row can have moved while the provider was answering, and
                // a retry has the same window the first attempt does.
                settle_new_egress(accounts, recorder, &recording.id, &egress_id).await;
            } else {
                // Either the row already names another job, or the write
                // failed. Either way this attempt owns a job the row does not,
                // so it stops the one it just made rather than leaving two.
                eprintln!(
                    "recording {} started a job its row does not name; stopping it",
                    recording.id
                );
                let _ = recorder
                    .provider
                    .stop(&egress_id, &room_name)
                    .await
                    .inspect_err(|error| {
                        eprintln!("recording {} left a job running: {error}", recording.id)
                    });
            }
            stored
        }
        Err(error) => {
            eprintln!(
                "recording {} could not start on retry: {error}",
                recording.id
            );
            let accounts = accounts.clone();
            let id = recording.id.clone();
            let _ = blocking(move || bump_retry(&accounts, clock.as_ref(), &id)).await;
            false
        }
    }
}

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

/// Where a recording's media goes once the provider is done with it.
///
/// A trait for the same reason [`RecordingProvider`] is one: every failure this
/// stage has to survive belongs to somebody else's service, and a transfer that
/// half-succeeds cannot be produced against Google Drive on demand.
pub trait DeliveryProvider: Send + Sync + 'static {
    /// The Drive file id, given the staged object.
    fn transfer<'a>(
        &'a self,
        gcs_object: &'a str,
        filename: &'a str,
        recording_id: &'a str,
    ) -> BoxFuture<'a, Result<String, String>>;
    /// The permission id, given the file and who may read it.
    fn share<'a>(
        &'a self,
        drive_file_id: &'a str,
        recipient_email: &'a str,
        expires_at: i64,
    ) -> BoxFuture<'a, Result<String, String>>;
    /// Revoke, delete the Drive file, delete the staged object, in that order.
    ///
    /// The permission carries the file it is on, and that file is not
    /// necessarily the one being deleted. A delivery that reused an earlier
    /// attempt's file and then created its own permission has to revoke that
    /// permission without deleting a file somebody else is still using, and
    /// Drive cannot revoke a permission without knowing which file it is on.
    fn revoke_and_delete<'a>(
        &'a self,
        drive_file_id: Option<&'a str>,
        permission: Option<(&'a str, &'a str)>,
        gcs_object: Option<&'a str>,
    ) -> BoxFuture<'a, Result<(), String>>;
}

/// Moves the file to its Shared Drive and shares it with the person it belongs
/// to.
///
/// Only from `transferring`, and the recording stays there when it fails. The
/// bytes are still in the staging bucket, so a delivery failure is retryable by
/// definition and moving to `failed` would advertise a recovery no transition
/// could reach. `error` records `drive_failed` and `retries` counts the
/// attempts; the schedule that makes them belongs to the transfer queue.
///
/// The Drive ids are written as they are earned, one statement each. A transfer
/// that succeeded and a share that failed must leave the file id behind, or the
/// file is in the Shared Drive and nothing knows where.
pub async fn deliver_recording(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    delivery: &dyn DeliveryProvider,
    recording: &Recording,
    recipient_email: &str,
    expires_at: i64,
) -> Result<RecordingState, Failure> {
    let gcs_object = gcs_object_path(&recorder.config.gcs_prefix, &recording.id);
    let filename = format!("{}.mp4", recording.id);
    let fail = |step: &str, error: &str| {
        audit(
            "recording_delivery_failed",
            &recording.id,
            &[
                ("step", step),
                ("error", error),
                ("reason", Failure::Drive.as_str()),
                ("recovery", Failure::Drive.recovery()),
            ],
        );
        Failure::Drive
    };

    // The staged object is written down before anything remote happens. It is
    // derivable from the recording id, but the deletion path reads it from the
    // row, and a delivery that ended anywhere except success used to leave that
    // column empty: the bytes stayed in the bucket with nothing naming them.
    let recorded = {
        let accounts = accounts.clone();
        let id = recording.id.clone();
        let gcs_object = gcs_object.clone();
        blocking(move || set_gcs_object(&accounts, &id, &gcs_object)).await
    };

    // `Err` is not `Ok(false)`. A database failure is not a confirmed state
    // race, and treating it as one sends the cleanup path after media that is
    // still wanted.
    if recorded.is_err() {
        let failure = fail(
            "record_object",
            "the staged object could not be written down",
        );
        record_delivery_failure(accounts, recorder, &recording.id).await;
        return Err(failure);
    }
    if !recorded.unwrap_or(false) {
        // Nothing remote happens until this is written down, and it is written
        // only for a recording that is still transferring. Creating media whose
        // cleanup handle was never recorded, or creating it at all for a
        // recording that already ended, are the two failures this ordering
        // exists to prevent.
        let failure = fail(
            "record_object",
            "the staged object could not be written to a transferring recording",
        );
        record_delivery_failure(accounts, recorder, &recording.id).await;
        return Err(failure);
    }

    // An earlier attempt may have uploaded and then failed to share. Uploading
    // again would leave that file in the Shared Drive with nothing naming it.
    let existing = {
        let accounts = accounts.clone();
        let id = recording.id.clone();
        blocking(move || delivery_handles(&accounts, &id)).await
    };
    let Ok((existing, existing_permission, _)) = existing else {
        // Not knowing whether a file exists is not permission to make another
        // one. A read failure read as "no file" is how the no-orphan guarantee
        // stops holding during exactly the trouble it is for.
        let failure = fail("read_handles", "the delivery handles could not be read");
        record_delivery_failure(accounts, recorder, &recording.id).await;
        return Err(failure);
    };
    let uploaded = existing.is_none();
    let drive_file_id = match existing {
        Some(drive_file_id) => drive_file_id,
        None => match delivery
            .transfer(&gcs_object, &filename, &recording.id)
            .await
        {
            Ok(id) => id,
            Err(error) => {
                let failure = fail("transfer", &error);
                record_delivery_failure(accounts, recorder, &recording.id).await;
                return Err(failure);
            }
        },
    };

    // Written before the share, and its failure stops the delivery. A file in
    // the Shared Drive that no row names is a file nothing can find and nothing
    // can delete, and going on to share it would hand somebody a link to it.
    //
    // Only when this call uploaded. Reusing an id the row already holds and
    // then writing it again would fail the `IS NULL` guard and send this
    // delivery off to delete the file it was reusing.
    let stored = if uploaded {
        let accounts = accounts.clone();
        let id = recording.id.clone();
        let drive_file_id = drive_file_id.clone();
        blocking(move || set_drive_file(&accounts, &id, &drive_file_id)).await
    } else {
        Ok(true)
    };
    if stored.is_err() {
        // A write that failed is not a state race. The file stays where it is,
        // because the row may still want it and the next attempt reuses the id
        // once this can be written down.
        let failure = fail("record_file", "the Drive file id could not be written down");
        record_delivery_failure(accounts, recorder, &recording.id).await;
        return Err(failure);
    }
    if !stored.unwrap_or(false) {
        // The row left `transferring` while the upload ran, which a withdrawal
        // or a kill switch does. The file this call created belongs to nobody,
        // so it goes rather than sitting in the Shared Drive with nothing
        // naming it.
        let failure = fail(
            "record_file",
            "the Drive file id could not be written to a transferring recording",
        );

        // No `record_delivery_failure` here. That write is guarded on
        // `transferring`, which is the state the row that beat this one is in,
        // so recording a failure would put this delivery's error and retry
        // count on somebody else's delivery.
        clean_up_after_losing(
            accounts,
            delivery,
            &recording.id,
            "record_file",
            (uploaded.then_some(drive_file_id.as_str()), None),
            &gcs_object,
        )
        .await;
        return Err(failure);
    }

    // A permission this recording already has is not shared again. Asking Drive
    // to grant the same person the same access twice is either a duplicate
    // grant or an error, and neither is what a retry wanted.
    let shared = existing_permission.is_none();
    let permission_id = match existing_permission {
        Some(permission_id) => permission_id,
        None => match delivery
            .share(&drive_file_id, recipient_email, expires_at)
            .await
        {
            Ok(id) => id,
            Err(error) => {
                let failure = fail("share", &error);
                record_delivery_failure(accounts, recorder, &recording.id).await;
                return Err(failure);
            }
        },
    };

    // The permission id first, on its own, for the same reason the file id was:
    // a permission nothing names is one nothing can revoke.
    let stored = if shared {
        let accounts = accounts.clone();
        let id = recording.id.clone();
        let permission_id = permission_id.clone();
        blocking(move || set_drive_permission(&accounts, &id, &permission_id)).await
    } else {
        Ok(true)
    };
    if stored.is_err() {
        let failure = fail(
            "record_permission",
            "the permission id could not be written down",
        );
        record_delivery_failure(accounts, recorder, &recording.id).await;
        return Err(failure);
    }
    if !stored.unwrap_or(false) {
        let failure = fail(
            "record_permission",
            "the permission id could not be written to a transferring recording",
        );
        clean_up_after_losing(
            accounts,
            delivery,
            &recording.id,
            "record_permission",
            (
                uploaded.then_some(drive_file_id.as_str()),
                shared.then_some((drive_file_id.as_str(), permission_id.as_str())),
            ),
            &gcs_object,
        )
        .await;
        return Err(failure);
    }

    let finished = {
        let accounts = accounts.clone();
        let clock = recorder.clock.clone();
        let id = recording.id.clone();
        blocking(move || finish_delivery(&accounts, clock.as_ref(), &id, expires_at)).await
    };

    // `Ok(false)` is a row that was no longer `transferring`, which means
    // somebody else moved it: a withdrawal, a kill switch. Reporting `Ready`
    // then would announce a delivery the row does not have.
    if finished.is_err() {
        let failure = fail("finish", "the delivery could not be written down");
        record_delivery_failure(accounts, recorder, &recording.id).await;
        return Err(failure);
    }
    if !finished.unwrap_or(false) {
        // A lost race, not a failure of this delivery: somebody else moved the
        // row and their state is the right one, so recording a failure over it
        // would overwrite a newer answer with an older complaint. What this
        // delivery made goes with it, because nothing is going to come back for
        // a file belonging to a recording that ended.
        audit(
            "recording_delivery_lost",
            &recording.id,
            &[("step", "finish")],
        );
        clean_up_after_losing(
            accounts,
            delivery,
            &recording.id,
            "finish",
            (
                uploaded.then_some(drive_file_id.as_str()),
                shared.then_some((drive_file_id.as_str(), permission_id.as_str())),
            ),
            &gcs_object,
        )
        .await;
        return Err(Failure::Drive);
    }
    audit("recording_ready", &recording.id, &[]);
    Ok(RecordingState::Ready)
}

/// Revokes, deletes, and tombstones, in that order.
///
/// A failure anywhere is `cleanup_failed`, which is terminal for the pipeline
/// and an alert for a person: the media outlived the attempt to delete it, and
/// nothing automatic is going to fix that.
pub async fn delete_recording(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    delivery: &dyn DeliveryProvider,
    recording: &Recording,
    deleted_by: &str,
) -> Result<RecordingState, String> {
    let stored = {
        let accounts = accounts.clone();
        let id = recording.id.clone();
        blocking(move || delivery_handles(&accounts, &id)).await
    };
    let Ok((drive_file_id, drive_permission_id, gcs_object)) = stored else {
        return Err("could not read the delivery handles".to_string());
    };

    // The permission is named with the file it is on, which for a deletion is
    // the same file. The pair exists for the delivery paths, where it is not.
    let permission = drive_file_id.as_deref().zip(drive_permission_id.as_deref());
    if let Err(error) = delivery
        .revoke_and_delete(drive_file_id.as_deref(), permission, gcs_object.as_deref())
        .await
    {
        // The transition first, and the line only if this call made it. A
        // concurrent deletion can win, and an audit record telling an operator
        // that cleanup failed for a row that is `deleted` sends them looking
        // for a file that is not there.
        let moved = {
            let accounts = accounts.clone();
            let clock = recorder.clock.clone();
            let id = recording.id.clone();
            blocking(move || {
                transition(
                    &accounts,
                    clock.as_ref(),
                    &id,
                    RecordingState::CleanupFailed,
                    Some(Failure::Cleanup.as_str()),
                )
            })
            .await
        };
        if let Ok(Some((_, true))) = moved {
            audit(
                "recording_cleanup_failed",
                &recording.id,
                &[
                    ("step", "revoke_and_delete"),
                    ("error", &error),
                    ("reason", Failure::Cleanup.as_str()),
                    ("action", Failure::Cleanup.recovery()),
                ],
            );
        }
        return Err(error);
    }

    let tombstoned = {
        let accounts = accounts.clone();
        let clock = recorder.clock.clone();
        let id = recording.id.clone();
        let deleted_by = deleted_by.to_string();
        blocking(move || tombstone(&accounts, clock.as_ref(), &id, &deleted_by)).await
    };

    // A zero-row update is somebody else's deletion, not this one's. Reporting
    // `recording_deleted` for it would put two deletions in the log for one
    // file.
    if let Ok(false) = tombstoned {
        return Ok(RecordingState::Deleted);
    }
    if let Err(error) = tombstoned {
        // The media is gone and the row still says otherwise, holding the
        // recipient address and handles for files that no longer exist. That is
        // a person's problem, and the row has to say so rather than sitting in
        // `ready` describing a recording nobody can watch.
        let moved = {
            let accounts = accounts.clone();
            let clock = recorder.clock.clone();
            let id = recording.id.clone();
            blocking(move || {
                transition(
                    &accounts,
                    clock.as_ref(),
                    &id,
                    RecordingState::CleanupFailed,
                    Some(Failure::Tombstone.as_str()),
                )
            })
            .await
        };
        if let Ok(Some((_, true))) = moved {
            audit(
                "recording_cleanup_failed",
                &recording.id,
                &[
                    ("step", "tombstone"),
                    ("error", &error.to_string()),
                    ("reason", Failure::Tombstone.as_str()),
                    ("action", Failure::Tombstone.recovery()),
                    ("media", "already_deleted"),
                ],
            );
        }
        return Err(format!("the deletion could not be written down: {error}"));
    }
    audit("recording_deleted", &recording.id, &[("by", deleted_by)]);
    Ok(RecordingState::Deleted)
}

/// Whether this call is the one that moved the row.
///
/// A provider that refused the stop does not undo the move: the row is in its
/// new state either way, and a caller that audited only on success lost the
/// record of a change that is now in the database. The refusal is the
/// sweeper's, and the missing `stopped_at` is what brings it back.
fn moved_by_this_call(
    recording_id: &str,
    outcome: Result<(RecordingState, bool), StopFailure>,
) -> bool {
    match outcome {
        Ok((_, moved)) => moved,
        Err(failure) => {
            report_stop_failure(recording_id, None, &failure);
            failure.moved
        }
    }
}

/// One place that decides which of the two stop failures this is.
///
/// `moved` is the difference. With it the transition happened and the provider
/// refused; without it the transition never happened and the provider was never
/// called, and reporting that as a refusal sends an operator to the wrong
/// service.
pub fn report_stop_failure(recording_id: &str, step: Option<&str>, failure: &StopFailure) {
    let event = if failure.moved {
        "recording_stop_refused"
    } else {
        "recording_stop_not_recorded"
    };
    match step {
        Some(step) => audit(
            event,
            recording_id,
            &[("step", step), ("error", &failure.error)],
        ),
        None => audit(event, recording_id, &[("error", &failure.error)]),
    };
}

/// What a delivery that lost its row may delete.
///
/// Two questions, not one, and they have different answers. A Drive file this
/// invocation uploaded and could not record is nobody's and always goes. The
/// staged object belongs to the recording, and goes only when the recording is
/// over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CleanupScope {
    /// What this call created. `false` only when the row could not be read,
    /// because not knowing is not permission to delete.
    mine: bool,
    /// The staged object, which another delivery may still be reading from and
    /// which retention owns.
    object: bool,
}

async fn cleanup_scope(accounts: &Arc<Accounts>, recording_id: &str) -> CleanupScope {
    let accounts = accounts.clone();
    let recording_id = recording_id.to_string();
    let Ok(Some(row)) = blocking(move || recording_by_id(&accounts, &recording_id)).await else {
        return CleanupScope {
            mine: false,
            object: false,
        };
    };
    if row.error.as_deref().and_then(Failure::parse) == Some(Failure::Drive) {
        // A delivery the queue will come back to. Its staged object stays; the
        // file this invocation uploaded and never recorded does not belong to
        // that retry and would be orphaned by keeping it.
        return CleanupScope {
            mine: true,
            object: false,
        };
    }
    match row.state {
        RecordingState::Transferring | RecordingState::Ready => CleanupScope {
            mine: true,
            object: false,
        },
        RecordingState::Failed | RecordingState::CleanupFailed | RecordingState::Deleted => {
            CleanupScope {
                mine: true,
                object: true,
            }
        }
        _ => CleanupScope {
            mine: false,
            object: false,
        },
    }
}

/// The cleanup one lost delivery is allowed to perform.
///
/// `mine` is what this invocation created: a file it uploaded, and a permission
/// it granted paired with the file that permission is on. A file it reused
/// belongs to an earlier attempt and the staged object belongs to the
/// recording, so deleting either because this call lost a race takes media from
/// somebody who still wants it.
async fn clean_up_after_losing(
    accounts: &Arc<Accounts>,
    delivery: &dyn DeliveryProvider,
    recording_id: &str,
    step: &str,
    mine: (Option<&str>, Option<(&str, &str)>),
    gcs_object: &str,
) {
    let scope = cleanup_scope(accounts, recording_id).await;
    let (drive_file_id, permission) = if scope.mine { mine } else { (None, None) };
    let object = scope.object.then_some(gcs_object);
    if drive_file_id.is_none() && permission.is_none() && object.is_none() {
        return;
    }
    report_cleanup(
        recording_id,
        step,
        delivery
            .revoke_and_delete(drive_file_id, permission, object)
            .await,
    );
}

/// A cleanup that itself failed, said out loud.
///
/// These run on paths already handling a failure, and swallowing them meant a
/// withdrawal that won mid-delivery could leave a Drive file and staged bytes
/// behind with nothing recorded and nobody told.
fn report_cleanup(recording_id: &str, step: &str, outcome: Result<(), String>) {
    if let Err(error) = outcome {
        audit(
            "recording_cleanup_failed",
            recording_id,
            &[
                ("step", step),
                ("error", &error),
                ("reason", Failure::Cleanup.as_str()),
                ("action", Failure::Cleanup.recovery()),
            ],
        );
    }
}

/// A delivery attempt that did not work, without moving the recording.
///
/// The row stays in `transferring` because the bytes are still in the staging
/// bucket and another attempt is the recovery. Moving it to `failed` advertised
/// `retry_delivery` from a state no transition could leave.
async fn record_delivery_failure(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    recording_id: &str,
) {
    let accounts = accounts.clone();
    let clock = recorder.clock.clone();
    let recording_id = recording_id.to_string();
    let _ = blocking(move || {
        accounts.with(|connection| {
            connection.execute(
                "
        UPDATE recordings
        SET error = ?2, retries = retries + 1, updated_at = ?3
        WHERE id = ?1 AND state = 'transferring'
        ",
                (&recording_id, Failure::Drive.as_str(), clock.now()),
            )?;
            Ok(())
        })
    })
    .await;
}

/// `false` when the row was no longer `transferring`, or already names a file.
///
/// The `IS NULL` guard is what stops two concurrent deliveries from both
/// uploading and the second overwriting the first, which orphans a file in the
/// Shared Drive. The loser deletes what it made.
fn set_drive_file(
    accounts: &Accounts,
    recording_id: &str,
    drive_file_id: &str,
) -> rusqlite::Result<bool> {
    accounts.with(|connection| {
        let changed = connection.execute(
            "
        UPDATE recordings SET drive_file_id = ?2
        WHERE id = ?1 AND state = 'transferring' AND drive_file_id IS NULL
        ",
            (recording_id, drive_file_id),
        )?;
        Ok(changed > 0)
    })
}

fn set_drive_permission(
    accounts: &Accounts,
    recording_id: &str,
    permission_id: &str,
) -> rusqlite::Result<bool> {
    accounts.with(|connection| {
        let changed = connection.execute(
            "
        UPDATE recordings SET drive_permission_id = ?2
        WHERE id = ?1 AND state = 'transferring' AND drive_permission_id IS NULL
        ",
            (recording_id, permission_id),
        )?;
        Ok(changed > 0)
    })
}

/// `false` when the row is not `transferring`, which is the whole check: a
/// delivery called against a recording that already ended must not reach the
/// provider at all.
fn set_gcs_object(
    accounts: &Accounts,
    recording_id: &str,
    gcs_object: &str,
) -> rusqlite::Result<bool> {
    accounts.with(|connection| {
        let changed = connection.execute(
            "UPDATE recordings SET gcs_object = ?2 WHERE id = ?1 AND state = 'transferring'",
            (recording_id, gcs_object),
        )?;
        Ok(changed > 0)
    })
}

/// `false` when the row was no longer `transferring`, which is somebody else
/// having moved it.
fn finish_delivery(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
    expires_at: i64,
) -> rusqlite::Result<bool> {
    let now = clock.now();
    accounts.with(|connection| {
        let changed = connection.execute(
            "
        UPDATE recordings
        SET expires_at = ?2,
            state = 'ready',
            -- Cleared, or a delivery that failed once and then worked would be
            -- `ready` still carrying `drive_failed`, and the status route would
            -- tell a candidate their recording had not been delivered.
            error = NULL,
            ready_at = ?3,
            updated_at = ?3
        WHERE id = ?1 AND state = 'transferring'
        ",
            (recording_id, expires_at, now),
        )?;
        Ok(changed > 0)
    })
}

fn delivery_handles(
    accounts: &Accounts,
    recording_id: &str,
) -> rusqlite::Result<(Option<String>, Option<String>, Option<String>)> {
    accounts.with(|connection| {
        connection.query_row(
            "SELECT drive_file_id, drive_permission_id, gcs_object FROM recordings WHERE id = ?1",
            [recording_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
    })
}

/// Records the deletion and gives up everything that described what was
/// deleted.
///
/// One statement, because the schema's own `CHECK` refuses a half-tombstone: a
/// row with `deleted_at` set and a recipient address still in it is not a state
/// this table has.
fn tombstone(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
    deleted_by: &str,
) -> rusqlite::Result<bool> {
    let now = clock.now();
    accounts.with(|connection| {
        let changed = connection.execute(
            "
        UPDATE recordings
        SET state = 'deleted',
            deleted_at = ?2,
            deleted_by = ?3,
            room_name = NULL,
            recipient_email = NULL,
            gcs_object = NULL,
            drive_file_id = NULL,
            drive_permission_id = NULL,
            updated_at = ?2
        WHERE id = ?1 AND deleted_at IS NULL
        ",
            (recording_id, now, deleted_by),
        )?;
        Ok(changed > 0)
    })
}
