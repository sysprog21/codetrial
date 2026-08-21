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

/// Where the recording template reads the replay it is rendering. Fixed here
/// beside the webhook route because the template is served by this repo and the
/// path is part of what task 1 pinned.
pub const REPLAY_ROUTE: &str = "/api/recording/replay";

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

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

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
    /// Where media goes and where it is deleted from. `None` is a deployment
    /// whose credentials could not build a client: it still records, and the
    /// sweeper still resolves rows, but nothing delivers or deletes, which is
    /// the state the binary refuses to start in.
    pub delivery: Option<Arc<dyn DeliveryProvider>>,
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
        //
        // A recording the delivery queue still owes work on is not stale,
        // whatever its `updated_at` says. An upload runs for up to half an hour
        // and touches nothing while it does, and a second delivery waits behind
        // it, so without this the sweeper fails a transfer that is running and
        // the worker then deletes the file it had just uploaded. Every
        // outstanding row, not only the claimed one, for the queued case. The
        // kill switch still takes them, because a kill switch that waited for
        // an upload is not one.
        let mut statement = connection.prepare(&format!(
            "{SELECT_RECORDING}
        WHERE state IN ('starting', 'recording', 'finalizing', 'transferring')
          AND (?1 IS NULL OR updated_at <= ?1)
          AND (?1 IS NULL OR id NOT IN (SELECT recording_id FROM delivery_queue))
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
    /// Recordings whose media was deleted by this pass.
    pub deleted: usize,
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

    // A transferring recording nobody is delivering. The transition and the
    // queue insert are two writes, so a process that died between them leaves a
    // row that owes a delivery and a queue that has never heard of it. Repaired
    // here rather than guarded against with a wider transaction: this is the
    // sweep that already exists to find work nothing is moving.
    //
    // Before the stale read, not after it. A restart longer than the stale
    // window leaves exactly such a row, and a sweep that read it as stale first
    // would fail the recording it had just queued a delivery for.
    {
        let accounts = accounts.clone();
        match blocking(move || requeue_orphaned_deliveries(&accounts, now)).await {
            Ok(0) => {}
            Ok(requeued) => eprintln!("recording sweep queued {requeued} orphaned deliveries"),
            Err(error) => eprintln!("WARNING: could not queue orphaned deliveries: {error}"),
        }
    }

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

    // Retention, before the stale pass rather than after it. A recording whose
    // media is due to go does not need chasing for a webhook first, and a row
    // this pass tombstones is one the pass below no longer sees.
    if let Some(delivery) = recorder.delivery.as_ref() {
        let due = {
            let accounts = accounts.clone();
            blocking(move || due_for_deletion(&accounts, now)).await
        };
        match due {
            Ok(due) => {
                for recording in due {
                    let reason = {
                        let accounts = accounts.clone();
                        let recording = recording.clone();
                        blocking(move || Ok(deletion_reason(&accounts, &recording)))
                            .await
                            .unwrap_or("expiry")
                    };
                    match delete_recording(
                        accounts,
                        recorder,
                        delivery.as_ref(),
                        &recording,
                        reason,
                    )
                    .await
                    {
                        Ok(_) => outcome.deleted += 1,

                        // Already audited as `recording_cleanup_failed`, which
                        // is the line an operator acts on. Counted here so the
                        // sweep's own summary says a pass had trouble.
                        Err(_) => outcome.failed += 1,
                    }
                }
            }
            Err(error) => eprintln!("WARNING: retention could not read its work: {error}"),
        }
    }

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

// The delivery queue: the work between a recording that has a file and one that
// has been handed to the person it belongs to.
//
// One row per recording that still owes a delivery, and none for one that does
// not. Success deletes the row; giving up deletes it too, because
// `recordings.state` and `recordings.error` are where an outcome lives and a
// queue that kept its own history would be a second answer that can disagree.

/// Three attempts, at zero, one minute and five. The first is the delivery
/// itself; a Drive call that failed twice in six minutes is failing for a
/// reason another minute will not fix, and the bytes are still in the staging
/// bucket for an operator to retry deliberately.
pub const DELIVERY_ATTEMPTS: i64 = 3;
pub const DELIVERY_BACKOFF_SECONDS: [i64; 2] = [60, 300];

/// How long a claim is honoured before another worker may take the row.
///
/// A worker that died with the process holds its claim forever otherwise, and
/// the recording sits in `transferring` until the sweeper abandons it.
///
/// An hour, which is longer than any upload this pipeline produces: a
/// forty-five minute interview at the configured bitrate is a few hundred
/// megabytes, and even a slow link finishes inside it. The number matters
/// because a claim that expires under a running upload is exactly how two
/// workers end up uploading at once, and the resumable session the first one
/// holds is invisible to the second.
pub const DELIVERY_CLAIM_SECONDS: i64 = 3600;

/// Put a recording in line for delivery.
///
/// Called when a recording reaches `transferring`, and idempotent: a webhook
/// LiveKit sent twice must not become two deliveries. `run_after` is now,
/// because the first attempt is the delivery itself rather than a retry.
pub fn enqueue_delivery(accounts: &Accounts, recording_id: &str, now: i64) -> rusqlite::Result<()> {
    accounts.with(|connection| {
        connection.execute(
            "
        INSERT INTO delivery_queue (recording_id, attempts, run_after, created_at, updated_at)
        VALUES (?1, 0, ?2, ?2, ?2)
        ON CONFLICT (recording_id) DO NOTHING
        ",
            (recording_id, now),
        )?;
        Ok(())
    })
}

/// Take one due row, or nothing.
///
/// The claim and the attempt count move in the same statement as the selection,
/// so two workers cannot both take the same row: SQLite serializes the writes,
/// and the second one finds a row whose `claimed_at` no longer matches what it
/// selected on.
///
/// Rows are due when `run_after` has passed and they are either unclaimed or
/// claimed longer ago than a delivery can take. The recording itself is read
/// back with the claim, because a queue row on its own says nothing about what
/// to deliver.
pub fn claim_delivery(
    accounts: &Accounts,
    now: i64,
) -> rusqlite::Result<Option<(Recording, String)>> {
    accounts.with(|connection| {
        let transaction = rusqlite::Transaction::new_unchecked(
            connection,
            rusqlite::TransactionBehavior::Immediate,
        )?;

        // The claim is a fresh token every time, so a worker whose claim
        // expired cannot finish or reschedule the row its replacement now
        // holds: its writes name a claim the row no longer has. A failure here
        // is the system's random source, which is not a condition to deliver
        // through: without a token this claim could be written over by the
        // worker it replaced.
        let claim = crate::accounts::random_token(16)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let claimed: Option<String> = transaction
            .query_row(
                "
        UPDATE delivery_queue
        SET claimed_at = ?1, claim = ?3, attempts = attempts + 1, updated_at = ?1
        WHERE recording_id = (
            SELECT recording_id FROM delivery_queue
            WHERE run_after <= ?1
              AND (claimed_at IS NULL OR claimed_at <= ?2)
            ORDER BY run_after
            LIMIT 1
        )
        RETURNING recording_id
        ",
                (now, now - DELIVERY_CLAIM_SECONDS, &claim),
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            })?;
        let Some(recording_id) = claimed else {
            transaction.commit()?;
            return Ok(None);
        };
        let recording = transaction
            .query_row(
                &format!("{SELECT_RECORDING} WHERE id = ?1"),
                [&recording_id],
                row_to_recording,
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            })?;
        transaction.commit()?;
        Ok(recording.map(|recording| (recording, claim)))
    })
}

/// How many attempts a queued delivery has already had.
pub fn delivery_attempts(accounts: &Accounts, recording_id: &str) -> rusqlite::Result<Option<i64>> {
    accounts.with(|connection| {
        connection
            .query_row(
                "SELECT attempts FROM delivery_queue WHERE recording_id = ?1",
                [recording_id],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            })
    })
}

/// The row is done with, whichever way it ended.
///
/// Named with the claim it was done under. A worker whose claim expired while
/// it worked would otherwise delete the row its replacement is holding, and the
/// replacement's delivery would finish into a queue that had forgotten it.
pub fn finish_delivery_work(
    accounts: &Accounts,
    recording_id: &str,
    claim: &str,
) -> rusqlite::Result<()> {
    accounts.with(|connection| {
        connection.execute(
            "DELETE FROM delivery_queue WHERE recording_id = ?1 AND claim = ?2",
            (recording_id, claim),
        )?;
        Ok(())
    })
}

/// Release a failed claim so the next attempt can happen later.
///
/// Returns whether there is another attempt. When there is not, the row is gone
/// and the caller is the one that has to fail the recording: a queue row for a
/// delivery nobody will attempt again is work that never happens, and the
/// sweeper would eventually abandon the recording with a less useful reason.
pub fn reschedule_delivery(
    accounts: &Accounts,
    recording_id: &str,
    claim: &str,
    error: &str,
    now: i64,
) -> rusqlite::Result<Reschedule> {
    accounts.with(|connection| {
        // One transaction, because the read and the write are one decision: a
        // row reclaimed between them would be read as this worker's and written
        // as somebody else's.
        let transaction = rusqlite::Transaction::new_unchecked(
            connection,
            rusqlite::TransactionBehavior::Immediate,
        )?;

        // Under this claim, or not at all. A row that has been reclaimed is
        // somebody else's work now, and rescheduling it would push their
        // attempt into the future for a failure that was not theirs.
        let attempts: i64 = match transaction.query_row(
            "SELECT attempts FROM delivery_queue WHERE recording_id = ?1 AND claim = ?2",
            (recording_id, claim),
            |row| row.get(0),
        ) {
            Ok(attempts) => attempts,

            // Not this worker's row any more: it was reclaimed while this
            // attempt ran, or finished by somebody else. Saying "exhausted"
            // here would fail a recording another worker is in the middle of
            // delivering, and take its Drive file with it.
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(Reschedule::NotOurs),
            Err(error) => return Err(error),
        };
        let Some(backoff) = DELIVERY_BACKOFF_SECONDS.get(attempts.max(1) as usize - 1) else {
            // The row stays, still claimed. Deleting it here would leave a
            // `transferring` recording with an empty queue until the caller
            // fails it, which is exactly what the sweeper's repair looks for:
            // it would queue the row again and the next claim would reopen it,
            // turning an operator-only retry into an automatic one. The caller
            // deletes it after the failure is written down.
            transaction.commit()?;
            return Ok(Reschedule::Exhausted);
        };
        transaction.execute(
            "
        UPDATE delivery_queue
        SET claimed_at = NULL, claim = NULL, run_after = ?2, error = ?3, updated_at = ?4
        WHERE recording_id = ?1 AND claim = ?5
        ",
            (recording_id, now + backoff, error, now, claim),
        )?;
        transaction.commit()?;
        Ok(Reschedule::Again)
    })
}

/// What became of a failed attempt. Three answers, because the caller does
/// something different with each and a bool could only carry two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reschedule {
    /// Due again after the backoff.
    Again,
    /// Out of attempts. The caller fails the recording.
    Exhausted,
    /// The row is somebody else's now. The caller does nothing at all.
    NotOurs,
}

/// How long a delivered recording is readable before its permission expires and
/// the retention sweep deletes it. The contract's twenty-four hours, in one
/// place, because the Drive permission and the deletion deadline have to be the
/// same number or one of them is a lie.
pub const RETENTION_SECONDS: i64 = 24 * 60 * 60;

/// What one pass of the delivery queue did, for the caller to log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DeliveryOutcome {
    pub delivered: usize,
    pub retrying: usize,
    pub failed: usize,
}

/// Work the queue until there is nothing due.
///
/// This is what makes completion asynchronous: the webhook that ends a
/// recording queues the delivery and answers, and the upload happens here,
/// under a claim, with a schedule behind it.
pub async fn run_delivery_queue(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    delivery: &dyn DeliveryProvider,
) -> DeliveryOutcome {
    let mut outcome = DeliveryOutcome::default();
    loop {
        let now = recorder.clock.now();
        let claimed = {
            let accounts = accounts.clone();
            match blocking(move || claim_delivery(&accounts, now)).await {
                Ok(claimed) => claimed,
                Err(error) => {
                    eprintln!("WARNING: the delivery queue could not read its work: {error}");
                    return outcome;
                }
            }
        };
        let Some((recording, claim)) = claimed else {
            return outcome;
        };

        // A delivery an operator asked for again. `retry_delivery` is the
        // recovery `drive_failed` names, and this is where it happens: the row
        // goes back to `transferring` and the attempt runs like any other. The
        // guard is in the statement rather than in the transition table,
        // because "failed for this one reason" is not a state the table has and
        // giving it one would make every other failure re-openable too.
        let recording = if recording.state == RecordingState::Failed {
            let reopened = {
                let accounts = accounts.clone();
                let clock = recorder.clock.clone();
                let id = recording.id.clone();
                blocking(move || reopen_delivery(&accounts, clock.as_ref(), &id)).await
            };
            match reopened {
                Ok(Some(recording)) => recording,
                _ => recording,
            }
        } else {
            recording
        };

        // A row that left `transferring` while it waited: a withdrawal, a kill
        // switch, an operator. Its media is somebody else's problem now, and
        // delivering it would hand out a file that has been recalled.
        if recording.state != RecordingState::Transferring {
            let accounts = accounts.clone();
            let id = recording.id.clone();
            let claim = claim.clone();
            let _ = blocking(move || finish_delivery_work(&accounts, &id, &claim)).await;
            continue;
        }

        // Read from the row rather than from the account, and read now rather
        // than at start: the address was copied into the row when the recording
        // began, which is the address the candidate consented with.
        let recipient = {
            let accounts = accounts.clone();
            let id = recording.id.clone();
            blocking(move || recipient_of(&accounts, &id)).await
        };
        let recipient = match recipient {
            Ok(Some(recipient)) => recipient,

            // A read that failed is a bad moment for the database, not a
            // recording without an address. The claim is left to expire, which
            // is what makes the next pass pick this row up again.
            Err(error) => {
                eprintln!("WARNING: a delivery could not read its recipient: {error}");
                return outcome;
            }

            // No address on a `transferring` row is a state the schema's own
            // `CHECK` does not have, so this is a row somebody wrote around it.
            // There is nobody to share with and no retry that invents one, so
            // it is failed with the code an operator can act on.
            Ok(None) => {
                outcome.failed += 1;
                let accounts = accounts.clone();
                let clock = recorder.clock.clone();
                let id = recording.id.clone();
                let claim = claim.clone();
                let _ = blocking(move || {
                    // The row is released only once the failure is written
                    // down. Deleting it first leaves a `transferring` recording
                    // with an empty queue, which is what the sweeper's repair
                    // looks for, and the attempt nobody authorized happens by
                    // itself.
                    let moved = transition(
                        &accounts,
                        clock.as_ref(),
                        &id,
                        RecordingState::Failed,
                        Some(Failure::Drive.as_str()),
                    );
                    if moved.is_ok() {
                        let _ = finish_delivery_work(&accounts, &id, &claim);
                    }
                    moved
                })
                .await;
                audit(
                    "recording_failed",
                    &recording.id,
                    &[
                        ("reason", Failure::Drive.as_str()),
                        ("recovery", Failure::Drive.recovery()),
                        ("step", "recipient"),
                    ],
                );
                continue;
            }
        };

        let delivered = deliver_recording(
            accounts,
            recorder,
            delivery,
            &recording,
            &recipient,
            RETENTION_SECONDS,
        )
        .await;
        let id = recording.id.clone();
        match delivered {
            Ok(_) => {
                outcome.delivered += 1;
                let accounts = accounts.clone();
                let claim = claim.clone();
                let _ = blocking(move || finish_delivery_work(&accounts, &id, &claim)).await;
            }
            Err(failure) => {
                let again = {
                    let accounts = accounts.clone();
                    let id = id.clone();
                    let reason = failure.as_str().to_string();
                    let claim = claim.clone();

                    // Read again rather than reused. `now` is from before the
                    // upload, and an attempt that took longer than the backoff
                    // would schedule its retry in the past.
                    let failed_at = recorder.clock.now();
                    blocking(move || {
                        reschedule_delivery(&accounts, &id, &claim, &reason, failed_at)
                    })
                    .await
                };
                match again {
                    Ok(Reschedule::Again) => {
                        outcome.retrying += 1;
                        continue;
                    }

                    // Reclaimed while this attempt ran, or already finished.
                    // Failing the recording here would stop a delivery another
                    // worker is in the middle of and delete the file it
                    // uploaded.
                    Ok(Reschedule::NotOurs) => continue,
                    Ok(Reschedule::Exhausted) => {}
                    Err(error) => {
                        eprintln!("WARNING: a delivery could not be rescheduled: {error}");
                        continue;
                    }
                }

                // Out of attempts, or a queue row that has gone. The recording
                // is failed here rather than left for the sweeper, because
                // `drive_failed` says what an operator can do about it and
                // `abandoned` does not.
                outcome.failed += 1;
                let accounts = accounts.clone();
                let clock = recorder.clock.clone();
                let id = id.clone();
                let claim = claim.clone();
                let moved = blocking(move || {
                    let moved = transition(
                        &accounts,
                        clock.as_ref(),
                        &id,
                        RecordingState::Failed,
                        Some(Failure::Drive.as_str()),
                    );

                    // Only once the failure is written down. A transition that
                    // errored and a row that was deleted anyway is a
                    // `transferring` recording with an empty queue, which the
                    // sweeper repairs into an attempt nobody authorized.
                    //
                    // A database that cannot write this leaves the row queued
                    // and the next pass tries again. That is the right answer
                    // for a moment of trouble and no answer at all for a
                    // database that stays broken, which is a condition nothing
                    // else here survives either.
                    if moved.is_ok() {
                        let _ = finish_delivery_work(&accounts, &id, &claim);
                    }
                    moved
                })
                .await;
                if let Ok(Some((_, true))) = moved {
                    audit(
                        "recording_failed",
                        &recording.id,
                        &[
                            ("reason", Failure::Drive.as_str()),
                            ("recovery", Failure::Drive.recovery()),
                            ("attempts", &DELIVERY_ATTEMPTS.to_string()),
                        ],
                    );
                }
            }
        }
    }
}

/// Queue any `transferring` recording the queue does not know about.
///
/// Idempotent and cheap: the insert names rows the queue is missing, so a
/// database where nothing went wrong writes nothing.
pub fn requeue_orphaned_deliveries(accounts: &Accounts, now: i64) -> rusqlite::Result<usize> {
    accounts.with(|connection| {
        connection.execute(
            "
        INSERT INTO delivery_queue (recording_id, attempts, run_after, created_at, updated_at)
        SELECT id, 0, ?1, ?1, ?1 FROM recordings
        WHERE state = 'transferring'
          AND id NOT IN (SELECT recording_id FROM delivery_queue)
        ",
            [now],
        )
    })
}

/// Put a recording that failed its delivery back in line for one.
///
/// Only from `failed` with `drive_failed`, and only while the media is still
/// there: a tombstoned row has nothing to deliver, and a recording that failed
/// for any other reason has no file in the staging bucket to try again with.
///
/// Returns the reopened row, or `None` when the guard refused, so the caller
/// works from what the database says rather than from what it asked for.
pub fn reopen_delivery(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
) -> rusqlite::Result<Option<Recording>> {
    accounts.with(|connection| {
        let moved = connection.execute(
            "
        UPDATE recordings
        SET state = 'transferring', updated_at = ?2
        WHERE id = ?1 AND state = 'failed' AND error = ?3 AND deleted_at IS NULL
        ",
            (recording_id, clock.now(), Failure::Drive.as_str()),
        )?;
        if moved == 0 {
            return Ok(None);
        }
        audit(
            "recording_delivery_reopened",
            recording_id,
            &[("reason", Failure::Drive.as_str())],
        );
        connection
            .query_row(
                &format!("{SELECT_RECORDING} WHERE id = ?1"),
                [recording_id],
                row_to_recording,
            )
            .map(Some)
    })
}

fn recipient_of(accounts: &Accounts, recording_id: &str) -> rusqlite::Result<Option<String>> {
    accounts.with(|connection| {
        connection.query_row(
            "SELECT recipient_email FROM recordings WHERE id = ?1",
            [recording_id],
            |row| row.get(0),
        )
    })
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
    /// Take back one grant. The permission carries the file it is on, and that
    /// file is not necessarily one the caller may delete: a delivery that
    /// reused an earlier attempt's file and then created its own permission has
    /// to revoke that grant and leave the file.
    fn revoke<'a>(
        &'a self,
        drive_file_id: &'a str,
        permission_id: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;

    /// Delete the Shared Drive file.
    fn delete_file<'a>(&'a self, drive_file_id: &'a str) -> BoxFuture<'a, Result<(), String>>;

    /// Delete the staged object.
    fn delete_object<'a>(&'a self, gcs_object: &'a str) -> BoxFuture<'a, Result<(), String>>;

    /// All three, in that order, for a caller with nothing to record between
    /// them.
    ///
    /// Retention does not use this: it writes down each step as it succeeds, so
    /// a revoke that worked and a delete that did not can be resumed rather
    /// than repeated from the start. This is for the cleanup a lost delivery
    /// does, where there is no row left to record progress on.
    fn revoke_and_delete<'a>(
        &'a self,
        drive_file_id: Option<&'a str>,
        permission: Option<(&'a str, &'a str)>,
        gcs_object: Option<&'a str>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let Some((file_id, permission_id)) = permission {
                self.revoke(file_id, permission_id).await?;
            }
            if let Some(drive_file_id) = drive_file_id {
                self.delete_file(drive_file_id).await?;
            }
            if let Some(gcs_object) = gcs_object {
                self.delete_object(gcs_object).await?;
            }
            Ok(())
        })
    }
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
    retention_seconds: i64,
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

    // Read from the clock here rather than taken from the caller. The deadline
    // is twenty-four hours of readable file, and an upload that took forty
    // minutes would otherwise hand the candidate twenty-three hours and twenty
    // minutes of it.
    let expires_at = recorder.clock.now() + retention_seconds;

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

    // Three steps, each written down as it succeeds. One call covering all
    // three records nothing between them, so a revoke that worked and a file
    // deletion that did not could only be retried from the start: the row still
    // named a permission that was already gone, and the retry's revoke failed
    // on it forever. Clearing each handle as its step lands makes the row the
    // progress, and every step idempotent by construction, because a handle
    // that is null is a step nothing repeats.
    let mut failed = None;
    if let (Some(file_id), Some(permission_id)) =
        (drive_file_id.as_deref(), drive_permission_id.as_deref())
    {
        match delivery.revoke(file_id, permission_id).await {
            Ok(()) => {
                let accounts = accounts.clone();
                let id = recording.id.clone();
                if let Err(error) =
                    blocking(move || clear_handle(&accounts, &id, "drive_permission_id")).await
                {
                    // The grant is gone and the row still names it. Reported
                    // rather than swallowed: the next pass repeats a revoke
                    // that will `404`, which is harmless, and an operator
                    // reading `cleanup_failed` learns the database is the thing
                    // that is broken.
                    failed = Some(("clear_permission", error.to_string()));
                }
            }
            Err(error) => failed = Some(("revoke", error)),
        }
    }
    if failed.is_none()
        && let Some(file_id) = drive_file_id.as_deref()
    {
        match delivery.delete_file(file_id).await {
            Ok(()) => {
                let accounts = accounts.clone();
                let id = recording.id.clone();
                if let Err(error) =
                    blocking(move || clear_handle(&accounts, &id, "drive_file_id")).await
                {
                    failed = Some(("clear_file", error.to_string()));
                }
            }
            Err(error) => failed = Some(("delete_file", error)),
        }
    }
    if failed.is_none()
        && let Some(object) = gcs_object.as_deref()
    {
        match delivery.delete_object(object).await {
            Ok(()) => {
                let accounts = accounts.clone();
                let id = recording.id.clone();
                if let Err(error) =
                    blocking(move || clear_handle(&accounts, &id, "gcs_object")).await
                {
                    failed = Some(("clear_object", error.to_string()));
                }
            }
            Err(error) => failed = Some(("delete_object", error)),
        }
    }
    if let Some((step, error)) = failed {
        let error = format!("{step}: {error}");

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
                    ("step", step),
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
    let outcome = delivery
        .revoke_and_delete(drive_file_id, permission, object)
        .await;
    let deleted = outcome.is_ok();
    report_cleanup(recording_id, step, outcome);

    // A handle that names a file this call just deleted is worse than no
    // handle: a later attempt reads it, skips the upload, and tries to share
    // something that is not there. Guarded on the value, so a row that has
    // moved on to some other file keeps it.
    if deleted && let Some(drive_file_id) = drive_file_id {
        let accounts = accounts.clone();
        let recording_id = recording_id.to_string();
        let drive_file_id = drive_file_id.to_string();
        let _ = blocking(move || forget_drive_file(&accounts, &recording_id, &drive_file_id)).await;
    }
}

/// Give up a Drive handle whose file is gone.
fn forget_drive_file(
    accounts: &Accounts,
    recording_id: &str,
    drive_file_id: &str,
) -> rusqlite::Result<()> {
    accounts.with(|connection| {
        connection.execute(
            "
        UPDATE recordings
        SET drive_file_id = NULL, drive_permission_id = NULL
        WHERE id = ?1 AND drive_file_id = ?2 AND deleted_at IS NULL
        ",
            (recording_id, drive_file_id),
        )?;
        Ok(())
    })
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
        let transaction = rusqlite::Transaction::new_unchecked(
            connection,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let changed = transaction.execute(
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

        // The replay goes with the media, in the same write. The consent
        // disclosure promises deletion, and a replay is the interview without
        // the video: what was said, what was typed, what the tests said.
        // Keeping it after the file is gone keeps the interview.
        if changed > 0 {
            transaction.execute(
                "
        DELETE FROM replay_events
        WHERE interview_id = (SELECT interview_id FROM recordings WHERE id = ?1)
        ",
                [recording_id],
            )?;
        }
        transaction.commit()?;
        Ok(changed > 0)
    })
}

/// One page of an account's recordings, newest first.
///
/// Paged by `created_at` and `id` rather than by offset: an offset shifts under
/// a row being inserted or deleted, which for this list means an interview
/// appearing twice or not at all while a candidate scrolls. The cursor is the
/// last row of the previous page.
pub const RECORDING_PAGE: i64 = 20;

pub fn recordings_for_account(
    accounts: &Accounts,
    account_id: i64,
    before: Option<(i64, String)>,
) -> rusqlite::Result<Vec<Recording>> {
    accounts.with(|connection| {
        // The first page starts above every row rather than at a sentinel that
        // has to be reasoned about: `created_at < i64::MAX` is true for any
        // timestamp this table can hold.
        let (created_at, id) = before.unwrap_or((i64::MAX, String::new()));
        let mut statement = connection.prepare(&format!(
            "{SELECT_RECORDING}
        WHERE account_id = ?1
          AND (created_at < ?2 OR (created_at = ?2 AND id < ?3))
        ORDER BY created_at DESC, id DESC
        LIMIT ?4
        "
        ))?;
        let rows = statement
            .query_map(
                (account_id, created_at, id, RECORDING_PAGE + 1),
                row_to_recording,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// What a reader may know about one recording.
///
/// Deliberately not a `Recording`: that carries the room name and the egress
/// id, and a history page has no use for either. What is here is what a person
/// asks about their own interview, plus the two dates that say whether it can
/// still be watched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingSummary {
    pub id: String,
    pub interview_id: String,
    pub state: RecordingState,
    pub error: Option<String>,
    pub created_at: i64,
    pub ready_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub deleted_at: Option<i64>,
    pub quota_exceeded: bool,
}

pub fn recording_summary(
    accounts: &Accounts,
    recording_id: &str,
    account_id: i64,
) -> rusqlite::Result<Option<RecordingSummary>> {
    accounts.with(|connection| {
        connection
            .query_row(
                "
        SELECT id, interview_id, state, error, created_at, ready_at, expires_at,
               deleted_at, quota_exceeded
        FROM recordings WHERE id = ?1 AND account_id = ?2
        ",
                (recording_id, account_id),
                |row| {
                    let state: String = row.get(2)?;
                    Ok(RecordingSummary {
                        id: row.get(0)?,
                        interview_id: row.get(1)?,
                        state: RecordingState::parse(&state).unwrap_or(RecordingState::Failed),
                        error: row.get(3)?,
                        created_at: row.get(4)?,
                        ready_at: row.get(5)?,
                        expires_at: row.get(6)?,
                        deleted_at: row.get(7)?,
                        quota_exceeded: row.get::<_, i64>(8)? != 0,
                    })
                },
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            })
    })
}

/// Recordings whose media should not exist any more.
///
/// Two reasons, and they are different promises. A recording past `expires_at`
/// has had its twenty-four hours. A recording whose candidate withdrew consent
/// has had a person say stop, and it has no deadline at all: nothing set one,
/// because it may never have been delivered.
///
/// Withdrawal is read from the interview as well as from the recording's own
/// error, because the two say it at different moments. A withdrawal during the
/// interview fails the recording with `consent_withdrawn`; one after delivery
/// leaves a `ready` row the transition table will not move, and the interview
/// is the only place that says stop.
///
/// `cleanup_failed` rows come back too. That is what makes a partial deletion
/// resumable: the handles the last attempt could not remove are still on the
/// row, and the ones it did remove are not.
///
/// A recording whose Egress job was never confirmed stopped is not due, however
/// old it is. Tombstoning gives up the room name, which is the only thing the
/// orphan sweep has to stop that job with, and a job nobody can stop keeps
/// writing media into a bucket this pass has just emptied. The sweep below
/// stops it first; the next pass takes the row.
pub fn due_for_deletion(accounts: &Accounts, now: i64) -> rusqlite::Result<Vec<Recording>> {
    accounts.with(|connection| {
        let mut statement = connection.prepare(&format!(
            "{SELECT_RECORDING}
        WHERE deleted_at IS NULL
          AND state IN ('ready', 'failed', 'cleanup_failed')
          AND (egress_id IS NULL OR stopped_at IS NOT NULL)
          AND ((expires_at IS NOT NULL AND expires_at <= ?1)
               OR error = ?2
               OR interview_id IN (
                   SELECT id FROM interviews WHERE consent_withdrawn_at IS NOT NULL
               ))
        ORDER BY id
        "
        ))?;
        let rows = statement
            .query_map((now, Failure::ConsentWithdrawn.as_str()), row_to_recording)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Why this recording's media is being deleted, in the words the tombstone
/// keeps. `operator` is the third, and it is a person running the cleanup
/// script rather than anything this module decides.
fn deletion_reason(accounts: &Accounts, recording: &Recording) -> &'static str {
    if recording.error.as_deref() == Some(Failure::ConsentWithdrawn.as_str()) {
        return "consent_withdrawn";
    }
    let asked: rusqlite::Result<(Option<String>, bool)> = accounts.with(|connection| {
        connection.query_row(
            "
        SELECT deleted_by,
               (SELECT consent_withdrawn_at IS NOT NULL FROM interviews WHERE id = interview_id)
        FROM recordings WHERE id = ?1
        ",
            [&recording.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
    });
    let (asked_by, withdrawn) = asked.unwrap_or((None, false));
    if withdrawn {
        return "consent_withdrawn";
    }

    // A person who brought this deletion forward said so on the row before the
    // sweeper found it. Their word for it survives, because "expiry" would say
    // the deadline came and it did not.
    if asked_by.as_deref() == Some("operator") {
        return "operator";
    }
    "expiry"
}

/// Give up one handle, now that what it named is gone.
///
/// The column name is a literal from this module rather than a value from
/// anywhere else: three call sites, three constants, and no way for a caller to
/// name a column this function was not written for.
fn clear_handle(accounts: &Accounts, recording_id: &str, column: &str) -> rusqlite::Result<()> {
    let statement = match column {
        "drive_permission_id" => "UPDATE recordings SET drive_permission_id = NULL WHERE id = ?1",
        "drive_file_id" => "UPDATE recordings SET drive_file_id = NULL WHERE id = ?1",
        "gcs_object" => "UPDATE recordings SET gcs_object = NULL WHERE id = ?1",
        other => unreachable!("no handle called {other}"),
    };
    accounts.with(|connection| {
        connection.execute(statement, [recording_id])?;
        Ok(())
    })
}

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
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            })?;
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
