//! The length a token request that names none is granted: the operator's
//! `CODETRIAL_DURATION_MIN`, held to the endpoint's range and the recording
//! cap.
//!
//! Split out of `token.rs`, which is at its size ceiling.

use super::*;

/// A request that names no length gets the operator's `CODETRIAL_DURATION_MIN`,
/// held to the same range and recording cap as a length the lobby asked for.
#[test]
fn token_without_a_length_gets_the_configured_default() {
    for (default_duration_min, recording_max_min, expected) in [
        (30, None, json!(30)),
        (DEFAULT_DURATION_MIN, None, json!(DEFAULT_DURATION_MIN)),
        // Inside the range the endpoint offers, whatever the file says.
        (5, None, json!(MIN_DURATION_MIN)),
        (120, None, json!(MAX_DURATION_MIN)),
        // A default the cap cannot record is shortened like any other length.
        (60, Some(45), json!(45)),
        (30, Some(45), json!(30)),
        // The built-in default on a deployment that can only record thirty
        // minutes: handing back forty-five loses the same stretch.
        (DEFAULT_DURATION_MIN, Some(30), json!(30)),
    ] {
        let response = token_response(
            &TokenConfig {
                api_key: "devkey",
                api_secret: "devsecret",
                server_url: "wss://example.livekit.cloud",
                recording_max_min,
                default_duration_min,
            },
            b"{}",
            "interview-fixed",
            "candidate-fixed",
            2000,
        )
        .unwrap();

        assert_eq!(
            response.duration_min, expected,
            "default {default_duration_min} under a cap of {recording_max_min:?}"
        );
        // The agent is handed the length in the signed room metadata.
        let metadata = claims(&response.token)["metadata"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            serde_json::from_str::<Value>(&metadata).unwrap()["durationMin"],
            expected,
            "metadata for default {default_duration_min} under a cap of {recording_max_min:?}"
        );
    }
}

/// The route hands the operator's length to the token, not the built-in one:
/// `token_response` is tested above with the field already set, so this is the
/// test that notices `/api/token` going back to the constant.
#[tokio::test]
async fn the_token_route_grants_the_configured_default_to_a_request_naming_none() {
    for (configured, expected) in [(30, 30), (60, 60), (120, MAX_DURATION_MIN)] {
        let mut config = web_config();
        config.guest_mode_enabled = true;
        config.default_duration_min = configured;
        let (base, server) = spawn_web_server_with_guest_dispatcher(config).await;

        let response = http_client()
            .post(format!("{base}/api/token"))
            .json(&json!({"problemId": "two-sum"}))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200, "configured {configured}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["durationMin"], expected, "configured {configured}");
        server.shutdown().await;
    }
}

/// `/api/session` reports the operator's length to a signed-in candidate as it
/// does to a signed-out one. A default of 60 cannot be mistaken for the
/// built-in 45, which is what a branch that forgot the field would send.
#[tokio::test]
async fn the_signed_in_session_reports_the_configured_default() {
    let (mut config, cookie, path) = signed_in_web_config("signed-in-default-duration");
    config.default_duration_min = 60;
    let (base, server) = spawn_web_server_with_guest_dispatcher(config).await;

    let session: Value = http_client()
        .get(format!("{base}/api/session"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(session["signedIn"], true);
    assert_eq!(session["defaultDurationMin"], 60);
    server.shutdown().await;
    remove_database(path).await;
}
