//! The credentialed acceptance for recording, and the rules it applies.
//!
//! Two halves. The rules are Rust and run in every `cargo test`: what a good
//! media run looks like, and whether the document
//! `scripts/recording-integration.sh` emits is the shape the schema promises.
//! The run itself needs an isolated LiveKit project, a staging bucket and a
//! Shared Drive, so it is skipped unless `CODETRIAL_RECORDING_INTEGRATION=1`
//! says an operator has them.
//!
//! Splitting it that way is the point. A harness whose judgement lives inside a
//! shell script nobody can run without credentials is a harness nobody can
//! check; the thresholds below are the part that can be wrong quietly, so they
//! are the part with tests.

use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

/// The 720p30 the contract fixes, and the tolerances around the rest.
///
/// Duration is compared against how long the room was actually up rather than
/// against a constant: a run cut short by a flaky provider should fail for
/// being short, not pass because sixty seconds was hard-coded somewhere.
const EXPECTED_WIDTH: i64 = 1280;
const EXPECTED_HEIGHT: i64 = 720;
const EXPECTED_FPS: f64 = 30.0;
const FPS_TOLERANCE: f64 = 1.0;
const DURATION_TOLERANCE: f64 = 0.10;

/// Long enough that a frame or two of a camera coming up cannot pass for a
/// candidate being on screen.
const MIN_CANDIDATE_SECONDS: f64 = 5.0;

/// The candidate and the interviewer. One is a recording of somebody talking to
/// themselves.
const EXPECTED_AUDIO_TRACKS: i64 = 2;

/// Fields a `--phase=media` run can prove on its own.
const MEDIA_REQUIRED: [&str; 10] = [
    "recording_id",
    "width",
    "height",
    "fps",
    "bitrate_kbps",
    "duration_seconds",
    "expected_duration_seconds",
    "candidate_video_seconds",
    "audio_tracks",
    "avatar_rendered",
];

/// The presentation fields no file can answer for, named once.
///
/// The schema annotates these three and requires the document to declare them,
/// and the checker refuses a document that claims any of them was measured.
/// Three statements of one set, so the set is written here and asserted equal
/// to the schema rather than typed out again beside it.
const ATTESTED: [&str; 3] = ["candidate_video_seconds", "audio_tracks", "avatar_rendered"];

/// Fields only a delivery-and-cleanup run can add.
const DELIVERY_REQUIRED: [&str; 5] = [
    "drive_file_id",
    "delivery_created_at",
    "permission_expires_at",
    "delivery_verified_at",
    "cleanup_status",
];

/// Every field a completed lifecycle document carries, in schema order.
///
/// Derived rather than written out again: the two lists share eleven names, and
/// a second copy is a second place for a new field to be forgotten.
fn lifecycle_required() -> impl Iterator<Item = &'static str> {
    MEDIA_REQUIRED.into_iter().chain(DELIVERY_REQUIRED)
}

/// Missing and unknown fields, independent of the acceptance phase.
///
/// The document is staged: delivery and cleanup append their facts to the
/// media document. A media test must accept that earlier stage while a
/// lifecycle test requires all of it.
fn field_faults(document: &Value, required: &[&str]) -> Vec<String> {
    let mut faults = Vec::new();
    for field in required {
        if document.get(field).is_none() {
            faults.push(format!("{field} is missing"));
        }
    }

    // The schema forbids extra properties, so the checker has to as well: a
    // document this accepts and the schema rejects is two contracts, and the
    // one an operator reads is the schema.
    for field in document
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, _)| key)
    {
        if !lifecycle_required().any(|known| known == field) {
            faults.push(format!("{field} is not a field of this document"));
        }
    }
    faults
}

/// What the media values are wrong about, assuming every field is present.
fn media_value_faults(document: &Value) -> Vec<String> {
    let mut faults = Vec::new();
    let integer = |field: &str| document[field].as_i64();
    let number = |field: &str| document[field].as_f64();

    if integer("width") != Some(EXPECTED_WIDTH) || integer("height") != Some(EXPECTED_HEIGHT) {
        faults.push(format!(
            "dimensions are {:?}x{:?}, not {EXPECTED_WIDTH}x{EXPECTED_HEIGHT}",
            integer("width"),
            integer("height")
        ));
    }
    match number("fps") {
        Some(fps) if (fps - EXPECTED_FPS).abs() <= FPS_TOLERANCE => {}
        fps => faults.push(format!("fps is {fps:?}, not {EXPECTED_FPS}")),
    }
    match (
        number("duration_seconds"),
        number("expected_duration_seconds"),
    ) {
        (Some(actual), Some(expected))
            if expected > 0.0 && (actual - expected).abs() / expected <= DURATION_TOLERANCE => {}
        (actual, expected) => faults.push(format!(
            "duration is {actual:?} against an expected {expected:?}, outside 10%"
        )),
    }
    match number("candidate_video_seconds") {
        Some(seconds) if seconds >= MIN_CANDIDATE_SECONDS => {}
        seconds => faults.push(format!(
            "the candidate was on screen for {seconds:?}, under {MIN_CANDIDATE_SECONDS}"
        )),
    }
    if integer("audio_tracks") != Some(EXPECTED_AUDIO_TRACKS) {
        faults.push(format!(
            "{:?} audio sources, not {EXPECTED_AUDIO_TRACKS}",
            integer("audio_tracks")
        ));
    }
    match number("bitrate_kbps") {
        Some(bitrate) if bitrate > 0.0 => {}

        // A file the container reports no bitrate for is a file `ffprobe` could
        // not read as video, whatever else it measured.
        bitrate => faults.push(format!("the bitrate is {bitrate:?}")),
    }
    if document["avatar_rendered"] != json!(true) {
        faults.push("the avatar did not render".to_string());
    }
    if document["recording_id"]
        .as_str()
        .unwrap_or_default()
        .is_empty()
    {
        faults.push("the recording id is empty".to_string());
    }
    faults
}

/// What the delivery values are wrong about, assuming every field is present.
fn delivery_value_faults(document: &Value) -> Vec<String> {
    let mut faults = Vec::new();
    if document["drive_file_id"]
        .as_str()
        .unwrap_or_default()
        .is_empty()
    {
        faults.push("the Drive file id is empty".to_string());
    }
    let parse_time = |field: &str| {
        document[field]
            .as_str()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
    };
    match (
        parse_time("delivery_created_at"),
        parse_time("permission_expires_at"),
        parse_time("delivery_verified_at"),
    ) {
        (Some(delivered), Some(expires), Some(verified))
            if expires > delivered
                && expires > verified
                && expires <= verified + Duration::hours(24) => {}
        (delivered, expires, verified) => faults.push(format!(
            "permission expiry is {expires:?} for delivery {delivered:?} and verification {verified:?}, not active and within 24 hours of verification"
        )),
    }
    let expected_cleanup = json!({"drive_file_absent": true, "gcs_object_absent": true});
    if document["cleanup_status"] != expected_cleanup {
        faults.push("cleanup did not prove both provider artifacts absent".to_string());
    }
    faults
}

/// What is wrong with a media-only acceptance, or nothing.
fn media_faults(document: &Value) -> Vec<String> {
    let faults = field_faults(document, &MEDIA_REQUIRED);
    if !faults.is_empty() {
        return faults;
    }
    media_value_faults(document)
}

/// What is wrong with a completed lifecycle acceptance, or nothing.
///
/// A missing field stops the run, because the value checks below would then be
/// reading a default rather than a measurement. Once the fields are there, both
/// sets of values are reported: an operator who has paid for a credentialed run
/// wants every fault in it, not the first one.
fn lifecycle_faults(document: &Value) -> Vec<String> {
    let faults = field_faults(document, &lifecycle_required().collect::<Vec<_>>());
    if !faults.is_empty() {
        return faults;
    }
    let mut faults = media_value_faults(document);
    faults.extend(delivery_value_faults(document));
    faults
}

/// The document the harness staged, from wherever it was told to write it.
fn acceptance_document() -> Value {
    let path = std::env::var("CODETRIAL_RECORDING_ACCEPTANCE_JSON")
        .unwrap_or_else(|_| "target/recording-acceptance.json".to_string());
    serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("no acceptance document at {path}: {error}")),
    )
    .unwrap_or_else(|error| panic!("the acceptance document at {path} is not JSON: {error}"))
}

/// A run this repository would accept.
fn passing_document() -> Value {
    json!({
        "recording_id": "yZ2f9Qm1a8Kd0Lb7Xc3Rte",
        "width": 1280,
        "height": 720,
        "fps": 30.0,
        "bitrate_kbps": 1980.0,
        "duration_seconds": 61.2,
        "expected_duration_seconds": 60.0,
        "candidate_video_seconds": 47.5,
        "audio_tracks": 2,
        "avatar_rendered": true,
        "drive_file_id": "drive-file-123",
        "delivery_created_at": "2026-09-01T10:00:00Z",
        "permission_expires_at": "2026-09-02T10:00:00Z",
        "delivery_verified_at": "2026-09-01T10:00:00Z",
        "cleanup_status": {"drive_file_absent": true, "gcs_object_absent": true}
    })
}

#[test]
fn a_good_run_is_accepted() {
    assert_eq!(lifecycle_faults(&passing_document()), Vec::<String>::new());
}

#[test]
fn every_threshold_can_fail() {
    // One at a time, because a document that is wrong in one way has to fail on
    // that way rather than on whichever check happens to run first.
    let cases = [
        ("width", json!(640), "dimensions"),
        ("fps", json!(24.0), "fps"),
        ("duration_seconds", json!(30.0), "duration"),
        ("candidate_video_seconds", json!(2.0), "on screen"),
        ("audio_tracks", json!(1), "audio sources"),
        ("bitrate_kbps", json!(0.0), "bitrate"),
        ("avatar_rendered", json!(false), "avatar"),
        ("recording_id", json!(""), "recording id"),
    ];
    for (field, value, expected) in cases {
        let mut document = passing_document();
        document[field] = value.clone();
        let faults = media_faults(&document);
        assert!(
            faults.iter().any(|fault| fault.contains(expected)),
            "{field} = {value} should fail on {expected}, got {faults:?}"
        );
    }
}

#[test]
fn a_media_phase_document_needs_no_delivery_or_cleanup_facts() {
    let mut document = passing_document();
    for field in DELIVERY_REQUIRED {
        document.as_object_mut().unwrap().remove(field);
    }
    assert_eq!(media_faults(&document), Vec::<String>::new());
    assert_eq!(
        lifecycle_faults(&document),
        vec![
            "drive_file_id is missing",
            "delivery_created_at is missing",
            "permission_expires_at is missing",
            "delivery_verified_at is missing",
            "cleanup_status is missing",
        ]
    );
}

#[test]
fn delivery_and_cleanup_evidence_cannot_overstate_the_provider_result() {
    let mut document = passing_document();
    document["drive_file_id"] = json!("");
    assert!(
        lifecycle_faults(&document)
            .iter()
            .any(|fault| fault.contains("Drive file id"))
    );

    let mut document = passing_document();
    document["permission_expires_at"] = json!("2026-09-02T10:00:01Z");
    assert!(
        lifecycle_faults(&document)
            .iter()
            .any(|fault| fault.contains("active and within 24 hours of verification"))
    );

    let mut document = passing_document();
    document["permission_expires_at"] = json!("2026-09-01T10:00:00Z");
    assert!(
        lifecycle_faults(&document)
            .iter()
            .any(|fault| fault.contains("active and within 24 hours of verification"))
    );

    // Expired between delivery and the read, which only the liveness half of
    // the rule can see: this is still after `delivery_created_at` and still
    // inside the 24-hour ceiling, so both other bounds accept it.
    let mut document = passing_document();
    document["delivery_verified_at"] = json!("2026-09-01T20:00:00Z");
    document["permission_expires_at"] = json!("2026-09-01T15:00:00Z");
    assert!(
        lifecycle_faults(&document)
            .iter()
            .any(|fault| fault.contains("active and within 24 hours of verification"))
    );

    let mut document = passing_document();
    document["cleanup_status"]["gcs_object_absent"] = json!(false);
    assert!(
        lifecycle_faults(&document)
            .iter()
            .any(|fault| fault.contains("both provider artifacts"))
    );
}

#[test]
fn a_field_nobody_declared_is_refused() {
    let mut document = passing_document();
    document["notes"] = json!("a run somebody annotated");
    assert_eq!(
        media_faults(&document),
        vec!["notes is not a field of this document".to_string()],
        "the schema forbids extra properties and this is what enforces it"
    );
}

#[test]
fn a_missing_field_is_named_rather_than_defaulted() {
    // A document with a field missing used to read as a zero, which passed the
    // audio check by counting no tracks as none expected.
    for field in lifecycle_required() {
        let mut document = passing_document();
        document.as_object_mut().unwrap().remove(field);
        let faults = lifecycle_faults(&document);
        assert_eq!(faults, vec![format!("{field} is missing")]);
    }
}

#[test]
fn the_schema_and_the_checker_agree_on_the_fields() {
    // The schema is what the script writes against and the checker is what
    // reads it. Two lists that can drift apart is one list too many, so this is
    // the seam that says they have not.
    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/fixtures/recording/media-output.schema.json").unwrap(),
    )
    .unwrap();
    let required = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| field.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(required, MEDIA_REQUIRED.map(str::to_string).to_vec());

    let properties = schema["properties"].as_object().unwrap();
    for field in lifecycle_required() {
        assert!(properties.contains_key(field), "{field} has no schema");
    }
    assert_eq!(
        properties.len(),
        lifecycle_required().count(),
        "the schema describes a field the checker does not know about"
    );
    assert_eq!(schema["additionalProperties"], json!(false));

    // The attested set, twice: whichever properties the schema annotates, and
    // the ones this checker knows cannot be measured. `properties` orders its
    // keys rather than keeping the file's order, so both sides are sorted.
    let mut expected = ATTESTED;
    expected.sort_unstable();
    let annotated = properties
        .iter()
        .filter(|(_, schema)| schema["x-codetrial-evidence"] == json!("operator-attestation"))
        .map(|(field, _)| field.as_str())
        .collect::<Vec<_>>();
    assert_eq!(annotated, expected, "the schema annotates a different set");

    // The document the script emits is the third copy of the field list and the
    // only one no gate reads. Catching a drifted key here costs a grep;
    // catching it in the acceptance costs a provisioned project and a sat
    // interview.
    let emitter = std::fs::read_to_string("scripts/recording-integration.sh").unwrap();
    for field in MEDIA_REQUIRED {
        assert!(
            emitter.contains(&format!("\"{field}\":")),
            "scripts/recording-integration.sh never writes {field}"
        );
    }
}

#[test]
fn the_script_refuses_before_it_creates_anything() {
    // The order the script promises: tools, then configuration, then the
    // template, and only then a room. Checked by running it with nothing
    // configured, which is the state every developer machine is in.
    let script = Path::new("scripts/recording-integration.sh");
    assert!(script.exists(), "the harness has to exist to be skipped");

    let refused = std::process::Command::new("sh")
        .arg(script)
        .arg("--phase=media")
        .env_remove("CODETRIAL_RECORDING_INTEGRATION")
        .env_remove("CODETRIAL_RECORDING_GCS_BUCKET")
        .output()
        .unwrap();
    assert_eq!(
        refused.status.code(),
        Some(2),
        "a harness with no credentials refuses rather than half-running: {refused:?}"
    );
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        said.contains("CODETRIAL_RECORDING_INTEGRATION"),
        "and says what is missing: {said}"
    );

    let unknown = std::process::Command::new("sh")
        .arg(script)
        .arg("--phase=whatever")
        .env("CODETRIAL_RECORDING_INTEGRATION", "1")
        .output()
        .unwrap();
    assert_eq!(unknown.status.code(), Some(2), "{unknown:?}");
}

/// The credentialed run itself.
///
/// Skipped, loudly, unless an operator has said they have the isolated project
/// from task 0. `Ok` rather than `ignore`, so `scripts/test.sh` runs this file
/// and proves the skip works rather than proving nothing.
#[test]
fn media_acceptance() {
    if std::env::var("CODETRIAL_RECORDING_INTEGRATION").as_deref() != Ok("1") {
        eprintln!(
            "skipping the recording media acceptance: set CODETRIAL_RECORDING_INTEGRATION=1 \
             and run ./scripts/recording-integration.sh --phase=media with the isolated project"
        );
        return;
    }

    // The document the script left behind, not a run this test performs. The
    // script owns the provider calls and the cleanup trap; this owns the
    // judgement, which is the half that can be wrong without anybody noticing.
    let document = acceptance_document();

    let faults = media_faults(&document);
    assert!(
        faults.is_empty(),
        "the recording was not acceptable: {faults:?}"
    );
}

/// The completed, credentialed lifecycle acceptance.
///
/// This is opt-in separately from the media check because a media phase has no
/// Drive file or cleanup result yet. The all-phases procedure enables it only
/// after the script has appended those provider facts.
#[test]
fn lifecycle_acceptance() {
    if std::env::var("CODETRIAL_RECORDING_INTEGRATION").as_deref() != Ok("1")
        || std::env::var("CODETRIAL_RECORDING_LIFECYCLE_ACCEPTANCE").as_deref() != Ok("1")
    {
        eprintln!(
            "skipping the recording lifecycle acceptance: set CODETRIAL_RECORDING_INTEGRATION=1 \
             CODETRIAL_RECORDING_LIFECYCLE_ACCEPTANCE=1 after every lifecycle phase"
        );
        return;
    }

    let document = acceptance_document();

    let faults = lifecycle_faults(&document);
    assert!(
        faults.is_empty(),
        "the completed recording lifecycle was not acceptable: {faults:?}"
    );
}
