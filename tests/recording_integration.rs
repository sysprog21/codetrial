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

/// Every field the schema requires, in the order it lists them.
const REQUIRED: [&str; 10] = [
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

/// What is wrong with this run, or nothing.
///
/// Every failure, not the first: an operator reading a failed acceptance wants
/// the whole list, because a second credentialed run costs minutes and a
/// provisioned project.
fn media_faults(document: &Value) -> Vec<String> {
    let mut faults = Vec::new();
    for field in REQUIRED {
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
        if !REQUIRED.contains(&field.as_str()) {
            faults.push(format!("{field} is not a field of this document"));
        }
    }
    if !faults.is_empty() {
        return faults;
    }

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
        "avatar_rendered": true
    })
}

#[test]
fn a_good_run_is_accepted() {
    assert_eq!(media_faults(&passing_document()), Vec::<String>::new());
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
    for field in REQUIRED {
        let mut document = passing_document();
        document.as_object_mut().unwrap().remove(field);
        let faults = media_faults(&document);
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
    assert_eq!(required, REQUIRED.map(str::to_string).to_vec());

    let properties = schema["properties"].as_object().unwrap();
    for field in REQUIRED {
        assert!(properties.contains_key(field), "{field} has no schema");
    }
    assert_eq!(
        properties.len(),
        REQUIRED.len(),
        "the schema describes a field the checker does not know about"
    );
    assert_eq!(schema["additionalProperties"], json!(false));
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
    let path = std::env::var("CODETRIAL_RECORDING_ACCEPTANCE_JSON")
        .unwrap_or_else(|_| "target/recording-acceptance.json".to_string());
    let document: Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("no acceptance document at {path}: {error}")),
    )
    .unwrap_or_else(|error| panic!("the acceptance document at {path} is not JSON: {error}"));

    let faults = media_faults(&document);
    assert!(
        faults.is_empty(),
        "the recording was not acceptable: {faults:?}"
    );
}
