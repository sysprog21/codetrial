use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::config::RecordingConfig;

use super::*;

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
