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
