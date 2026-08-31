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

/// A dotted path, where a numeric segment indexes an array. Without the index
/// the file-output assertions read as a chain of brackets rather than as the
/// place in the request they are talking about.
fn field<'a>(value: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(value, |value, key| {
        match key.parse::<usize>() {
            Ok(index) => value.get(index),
            Err(_) => value.get(key),
        }
        .unwrap_or_else(|| panic!("{path} should be present, missing at {key}"))
    })
}

/// Every key the body sends, against the names the decoder actually binds.
///
/// The sibling decode test cannot do this job, which is worth stating because
/// it looks like it should. Its decoder ignores an unrecognized key exactly as
/// the server does: a probe adding one to this body was answered `requested
/// room does not exist`, and a fixture carrying one still decodes and still
/// passes. So a misspelled key is invisible to both, and would record at the
/// wrong size or into no bucket with only a finished recording to show it.
///
/// The names below are still written by hand, but they are no longer loose:
/// the destructure in that test is exhaustive, so a rename upstream stops the
/// build in the function next to this one.
#[test]
fn contract_start_egress_sends_only_keys_the_decoder_binds() {
    const REQUEST: &[&str] = &[
        "room_name",
        "layout",
        "audio_only",
        "audio_mixing",
        "video_only",
        "custom_base_url",
        "file_outputs",
        "stream_outputs",
        "segment_outputs",
        "image_outputs",
        "webhooks",
        "preset",
        "advanced",
    ];
    const ENCODING_OPTIONS: &[&str] = &[
        "width",
        "height",
        "depth",
        "framerate",
        "audio_codec",
        "audio_bitrate",
        "audio_quality",
        "audio_frequency",
        "video_codec",
        "video_bitrate",
        "video_quality",
        "key_frame_interval",
    ];
    const FILE_OUTPUT: &[&str] = &[
        "file_type",
        "filepath",
        "disable_manifest",
        "s3",
        "gcp",
        "azure",
        "aliOSS",
    ];
    const GCP_UPLOAD: &[&str] = &["credentials", "bucket", "proxy"];

    let check = |value: &Value, allowed: &[&str], path: &str| {
        for key in value
            .as_object()
            .unwrap_or_else(|| panic!("{path} should be an object"))
            .keys()
        {
            assert!(
                allowed.contains(&key.as_str()),
                "{path}.{key} is not a key the decoder binds, so it would be dropped in silence"
            );
        }
    };

    let request = json("start-egress-request.json");
    check(&request, REQUEST, "$");
    check(field(&request, "advanced"), ENCODING_OPTIONS, "$.advanced");
    for (index, output) in field(&request, "file_outputs")
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        check(output, FILE_OUTPUT, &format!("$.file_outputs[{index}]"));
        check(
            field(output, "gcp"),
            GCP_UPLOAD,
            &format!("$.file_outputs[{index}].gcp"),
        );
    }
}

/// The start body, decoded by the generated implementation the server decodes
/// it with.
///
/// This replaces a list of field names copied out of the protobuf by hand. The
/// copy was the weaker half of the thing this file exists to prevent: it went
/// stale in silence, it checked names but never the enum spellings or the value
/// types beside them, and it was already wrong, missing two fields of
/// `EncodingOptions` that the destructure below named on the first compile.
///
/// Decoding matters here because this provider fails quietly. A probe that
/// added an unknown key to this exact body was answered `requested room does
/// not exist`, the same answer the correct body gets, so protojson drops what
/// it does not recognize rather than refusing it. A misspelled key would record
/// at the wrong size or into no bucket, and only a finished recording would
/// show it.
// `audio_quality` and `video_quality` are deprecated upstream. They are named
// only because the destructure is exhaustive, and naming them is the point:
// this body must not start sending them.
#[allow(deprecated)]
#[test]
fn contract_start_egress_decodes_as_the_message_livekit_expects() {
    use livekit_protocol::{
        AudioCodec, EncodedFileOutput, EncodedFileType, EncodingOptions,
        RoomCompositeEgressRequest, VideoCodec, encoded_file_output, room_composite_egress_request,
    };

    let request: RoomCompositeEgressRequest =
        serde_json::from_value(json("start-egress-request.json"))
            .expect("the start body should decode as the message LiveKit decodes it as");

    // Exhaustive on purpose. A field renamed or added upstream stops the build
    // here, which is why this is a destructure rather than a set of getters.
    let RoomCompositeEgressRequest {
        room_name,
        layout,
        audio_only,
        audio_mixing: _,
        video_only,
        custom_base_url,
        file_outputs,
        stream_outputs,
        segment_outputs,
        image_outputs,
        webhooks,
        output,
        options,
    } = request;

    assert!(!room_name.is_empty());
    assert!(custom_base_url.ends_with(recording::TEMPLATE_PATH));
    assert!(!audio_only && !video_only, "the replay needs both");
    assert!(
        layout.is_empty() && output.is_none(),
        "the template is named by custom_base_url, not by a layout or a legacy output"
    );
    assert!(
        stream_outputs.is_empty() && segment_outputs.is_empty() && image_outputs.is_empty(),
        "one file, nothing streamed or segmented"
    );
    assert!(
        webhooks.is_empty(),
        "delivery is driven by the room webhook"
    );

    // `advanced`, never `preset`: the agreed 2 Mbps ceiling is not a value any
    // EncodingOptionsPreset carries.
    let Some(room_composite_egress_request::Options::Advanced(EncodingOptions {
        width,
        height,
        framerate,
        video_codec,
        video_bitrate,
        audio_codec,
        audio_bitrate,
        depth: _,
        audio_frequency: _,
        audio_quality: _,
        video_quality: _,
        key_frame_interval: _,
    })) = options
    else {
        panic!("a preset would silently replace the agreed encode");
    };

    // The protobuf types these signed and the constants unsigned, so the
    // comparison names the conversion instead of letting a cast hide it.
    assert_eq!(
        (width, height, framerate, video_bitrate, audio_bitrate),
        (
            i32::try_from(recording::OUTPUT_WIDTH).unwrap(),
            i32::try_from(recording::OUTPUT_HEIGHT).unwrap(),
            i32::try_from(recording::OUTPUT_FRAMERATE).unwrap(),
            i32::try_from(recording::OUTPUT_VIDEO_BITRATE).unwrap(),
            i32::try_from(recording::OUTPUT_AUDIO_BITRATE).unwrap(),
        )
    );

    // Compared as enums. The decoder already refused any other spelling, so
    // these are about the value rather than the casing the fixture writes.
    assert_eq!(video_codec, VideoCodec::H264Main as i32);
    assert_eq!(audio_codec, AudioCodec::Aac as i32);

    let [
        EncodedFileOutput {
            file_type,
            filepath,
            disable_manifest,
            output: file_output,
        },
    ] = file_outputs.as_slice()
    else {
        panic!("exactly one file output");
    };
    assert_eq!(*file_type, EncodedFileType::Mp4 as i32);
    assert!(
        *disable_manifest,
        "the manifest would outlive the recording"
    );
    assert_eq!(filepath, "codetrial/Rf2Kq7XmZ0pB4nT8sVdLwA.mp4");
    let Some(encoded_file_output::Output::Gcp(gcp)) = file_output else {
        panic!("the file output goes straight to the staging bucket");
    };
    assert!(!gcp.bucket.is_empty() && !gcp.credentials.is_empty());
}

/// A fixture is only a specification while the code still agrees with it, and
/// nothing else compares the two: every other assertion here reads the fixture
/// alone, so the builder could drift away from it without failing anything.
#[test]
fn contract_start_egress_body_matches_the_fixture() {
    let request = json("start-egress-request.json");
    let text = |path: &str| field(&request, path).as_str().unwrap().to_string();

    // Derived, not repeated. The builder appends the template path, so taking
    // it back off proves that round trip instead of restating its result.
    let template_base_url = text("custom_base_url")
        .strip_suffix(recording::TEMPLATE_PATH)
        .expect("custom_base_url should end with the template path")
        .to_string();

    assert_eq!(
        recording::StartEgress {
            room_name: text("room_name"),
            template_base_url,
            filepath: text("file_outputs.0.filepath"),
            bucket: text("file_outputs.0.gcp.bucket"),
            service_account_json: text("file_outputs.0.gcp.credentials"),
            bitrate: field(&request, "advanced.video_bitrate").as_u64().unwrap() as u32,
        }
        .body(),
        request,
        "StartEgress::body and the fixture have drifted"
    );
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
