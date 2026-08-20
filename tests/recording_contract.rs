//! The provider contract, checked against bytes rather than against prose.
//!
//! Nothing in this file exercises CodeTrial's own recording code, because
//! there is none yet: these fixtures are the specification the lifecycle and
//! transfer tasks are written against. What they pin is the part a Rust
//! reviewer cannot see by reading LiveKit's documentation once, which is that
//! the same provider speaks two JSON casings depending on which side of the
//! call you are on, and that a webhook signature covers the request body
//! rather than the message identity.

use std::path::{Path, PathBuf};

use serde_json::Value;

use codetrial::recording;
use codetrial::token::{WebhookRejection, verify_livekit_webhook};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recording")
}

/// Bytes, not a parsed value. The webhook signature covers exactly what is on
/// disk, so a test that re-serialized the body would be checking a signature
/// over something the fixture never claimed to have signed.
fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(fixture_dir().join(name))
        .unwrap_or_else(|error| panic!("fixture {name} should be readable: {error}"))
}

fn json(name: &str) -> Value {
    serde_json::from_slice(&bytes(name))
        .unwrap_or_else(|error| panic!("fixture {name} should be JSON: {error}"))
}

fn field<'a>(value: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(value, |value, key| {
        value
            .get(key)
            .unwrap_or_else(|| panic!("{path} should be present, missing at {key}"))
    })
}

#[test]
fn contract_start_egress() {
    let request = json("start-egress-request.json");

    // Twirp speaks protobuf field names. Sending `roomName` would be accepted
    // by protojson and read identically, which is exactly why the wrong casing
    // survives review: it only shows up as a difference on the way back.
    assert!(
        request.get("roomName").is_none(),
        "the Twirp request is snake_case; a camelCase key here means the fixture drifted"
    );
    assert!(field(&request, "room_name").is_string());

    // Egress appends `url`, `token` and `layout` to whatever this names, so it
    // has to be the template path itself and carry no query string of its own.
    let template = field(&request, "custom_base_url").as_str().unwrap();
    assert!(
        template.ends_with(recording::TEMPLATE_PATH),
        "custom_base_url must end with {}, got {template}",
        recording::TEMPLATE_PATH
    );
    assert!(
        !template.contains('?'),
        "Egress appends its own query parameters, so the template URL must not carry any"
    );
    assert!(
        template.starts_with("https://"),
        "Egress fetches the template over the public internet"
    );

    // `advanced`, never `preset`: the agreed 2 Mbps ceiling is not a value any
    // EncodingOptionsPreset carries.
    assert!(
        request.get("preset").is_none(),
        "a preset would silently replace the agreed encode"
    );
    let advanced = field(&request, "advanced");
    assert_eq!(advanced["width"], recording::OUTPUT_WIDTH);
    assert_eq!(advanced["height"], recording::OUTPUT_HEIGHT);
    assert_eq!(advanced["framerate"], recording::OUTPUT_FRAMERATE);
    assert_eq!(advanced["video_bitrate"], recording::OUTPUT_VIDEO_BITRATE);
    assert_eq!(advanced["audio_bitrate"], recording::OUTPUT_AUDIO_BITRATE);
    assert_eq!(advanced["video_codec"], recording::OUTPUT_VIDEO_CODEC);
    assert_eq!(advanced["audio_codec"], recording::OUTPUT_AUDIO_CODEC);

    let output = &field(&request, "file_outputs")[0];
    assert_eq!(output["file_type"], recording::OUTPUT_FILE_TYPE);
    assert!(
        output["gcp"]["bucket"].is_string(),
        "the file output goes straight to the staging bucket; CodeTrial never holds the media"
    );

    // Written out rather than taken apart and put back together. Splitting the
    // fixture on `/` and `.mp4` and asking the formatter to rebuild it passes
    // for any string of that shape, which is a test of `rsplit_once`.
    assert_eq!(output["filepath"], "codetrial/Rf2Kq7XmZ0pB4nT8sVdLwA.mp4");

    // The literal repeats on purpose. Hoisting it into a variable shared with
    // the line above would make this a round trip again, which is the thing
    // that was wrong with the version this replaced.
    assert_eq!(
        recording::gcs_object_path("codetrial", "Rf2Kq7XmZ0pB4nT8sVdLwA"),
        "codetrial/Rf2Kq7XmZ0pB4nT8sVdLwA.mp4"
    );
    assert_eq!(
        recording::gcs_object_path("codetrial/", "Rf2Kq7XmZ0pB4nT8sVdLwA"),
        "codetrial/Rf2Kq7XmZ0pB4nT8sVdLwA.mp4",
        "a prefix configured with a trailing slash must not produce a double one"
    );

    let response = json("start-egress-response.json");
    assert_eq!(response["status"], "EGRESS_STARTING");
    assert!(
        response["egress_id"].is_string(),
        "the egress id is the handle every later call needs, and it is snake_case on the way back"
    );
}

#[test]
fn contract_stop_egress() {
    let started = json("start-egress-response.json");
    let request = json("stop-egress-request.json");

    // One field, and it is the one the start response handed back. A stop that
    // named the room instead would stop whatever egress that room has now,
    // which after a restart is not necessarily this one.
    assert_eq!(
        request.as_object().map(|fields| fields.len()),
        Some(1),
        "StopEgressRequest carries egress_id and nothing else"
    );
    assert_eq!(request["egress_id"], started["egress_id"]);

    let response = json("stop-egress-response.json");
    assert_eq!(response["egress_id"], started["egress_id"]);

    // Not COMPLETE. Stopping is a request, and the recording is only finished
    // when the `egress_ended` webhook says so.
    assert_eq!(response["status"], "EGRESS_ENDING");
}

#[test]
fn contract_duplicate_webhook() {
    let cases = json("webhook-cases.json");
    let now = cases["now"].as_u64().unwrap();
    let api_key = cases["apiKey"].as_str().unwrap();
    let api_secret = cases["apiSecret"].as_str().unwrap();

    let first = bytes("webhook-egress-ended.json");
    let retry = bytes("webhook-egress-ended-duplicate.json");
    for (name, body) in [("egress_ended", &first), ("egress_ended retry", &retry)] {
        let authorization = authorization_for(&cases, name);
        assert_eq!(
            verify_livekit_webhook(api_key, api_secret, authorization, body, now),
            Ok(()),
            "a retry is a fresh, validly signed message and must not be refused as a forgery"
        );
    }

    let first: Value = serde_json::from_slice(&first).unwrap();
    let retry: Value = serde_json::from_slice(&retry).unwrap();

    // The webhook body is lowerCamelCase, unlike the Twirp call above, because
    // upstream marshals it with protojson directly instead of through twirp.
    assert!(
        first.get("egress_info").is_none(),
        "the webhook body is camelCase; a snake_case key here means the fixture drifted"
    );
    assert!(field(&first, "egressInfo.egressId").is_string());

    assert_eq!(
        first["id"], retry["id"],
        "the event id is what makes a retry recognizable"
    );
    assert_ne!(
        first["createdAt"], retry["createdAt"],
        "and the delivery time is not, so it cannot be part of the dedup key"
    );
    assert_ne!(
        first, retry,
        "the bodies differ, so dedup cannot be a hash of the message either"
    );

    // int64 crosses protojson as a string. A consumer that reads this as a
    // number gets null and a recording that never finalizes.
    assert!(field(&first, "createdAt").is_string());
    assert!(field(&first, "egressInfo.startedAt").is_string());
}

#[test]
fn contract_invalid_signature() {
    let cases = json("webhook-cases.json");
    let now = cases["now"].as_u64().unwrap();
    let api_key = cases["apiKey"].as_str().unwrap();
    let api_secret = cases["apiSecret"].as_str().unwrap();

    let mut refused = Vec::new();
    for case in cases["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let body = bytes(case["body"].as_str().unwrap());
        let verdict = case["verdict"].as_str().unwrap();
        let outcome = verify_livekit_webhook(
            api_key,
            api_secret,
            case["authorization"].as_str().unwrap(),
            &body,
            now,
        );
        let expected = match verdict {
            "accepted" => Ok(()),
            "malformed" => Err(WebhookRejection::Malformed),
            "unknown_key" => Err(WebhookRejection::UnknownKey),
            "bad_signature" => Err(WebhookRejection::BadSignature),
            "expired" => Err(WebhookRejection::Expired),
            "body_mismatch" => Err(WebhookRejection::BodyMismatch),
            other => panic!("fixture case {name} has an unknown verdict {other}"),
        };
        assert_eq!(outcome, expected, "webhook case {name}");
        if let Err(rejection) = outcome {
            refused.push(rejection);
        }
    }

    // Against the variants themselves, not a count. A count is satisfied by
    // five copies of the same rejection, which is the coverage gap a fixture
    // set drifts into: the interesting cases are the ones nobody rewrites.
    for rejection in [
        WebhookRejection::Malformed,
        WebhookRejection::UnknownKey,
        WebhookRejection::BadSignature,
        WebhookRejection::Expired,
        WebhookRejection::BodyMismatch,
    ] {
        assert!(
            refused.contains(&rejection),
            "no fixture reaches {rejection}"
        );
    }

    // The signature covers the body, so a message that verified a moment ago
    // stops verifying the instant one byte of it changes. Derived here rather
    // than committed, because this is a property of the algorithm and not a
    // case someone has to remember to regenerate.
    let mut tampered = bytes("webhook-egress-ended.json");
    let last = tampered.len() - 1;
    tampered[last] = b' ';
    assert_eq!(
        verify_livekit_webhook(
            api_key,
            api_secret,
            authorization_for(&cases, "egress_ended"),
            &tampered,
            now
        ),
        Err(WebhookRejection::BodyMismatch)
    );
}

/// The three strings that must have exactly one owner. A test asserting the
/// document repeats the constants is the only thing standing between "fixed
/// here and nowhere else" and a second spelling arriving in a later task.
#[test]
fn contract_document_pins_the_fixed_strings() {
    let contract = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/recording-contract.md"),
    )
    .expect("the contract document should be readable");

    for pinned in [
        recording::WEBHOOK_ROUTE,
        recording::TEMPLATE_PATH,
        recording::EGRESS_SERVICE,
        recording::EGRESS_START_METHOD,
        recording::EGRESS_STOP_METHOD,
        &recording::gcs_object_path("{prefix}", "{recording_id}"),
    ] {
        assert!(
            contract.contains(pinned),
            "docs/recording-contract.md should name {pinned}"
        );
    }
}

fn authorization_for<'a>(cases: &'a Value, name: &str) -> &'a str {
    cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap_or_else(|| panic!("fixture case {name} should exist"))["authorization"]
        .as_str()
        .unwrap()
}
