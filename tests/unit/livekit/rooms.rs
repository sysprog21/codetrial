//! The `tests` module of `src/livekit/rooms.rs`, which declares this file by
//! path.
//! Everything here reaches into `src/livekit/rooms.rs` through `super`, so it
//! is a unit
//! test and not an integration test: private items are in scope.

use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The LiveKit credentials [`RoomService::config`] hands the production
/// code and the pair the server verifies its callers against. Named rather
/// than written twice so the two cannot drift: a server holding a different
/// secret from the config under test would refuse every call, and a refusal
/// reads here as LiveKit saying no rather than as a broken fixture.
/// Neither is a real credential; the project they name exists only in this
/// binary.
const STUB_API_KEY: &str = "devkey";
const STUB_API_SECRET: &str = "devsecret";

/// The bearer token on a request, or `None` when it carries no
/// `Authorization` header.
///
/// The header name is compared without regard to case because that is what
/// HTTP says it is; the value is read exactly as it was sent, since
/// lowercasing the head the way the `content-length` scan does would
/// destroy a token whose base64url is case significant.
fn bearer_token(head: &str) -> Option<&str> {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.trim())
        .and_then(|value| value.strip_prefix("Bearer "))
}

/// The `video` grant a token carries, without checking who signed it.
///
/// The decode is `livekit_token_issuer` read one claim further along, and
/// the same warning applies: nothing here is trusted. It runs after the
/// signature check below, so by the time a caller acts on it the claims are
/// ones this project's secret has already vouched for.
fn token_video_grant(token: &str) -> Option<serde_json::Value> {
    let (signing_input, _) = token.rsplit_once('.')?;
    let (_, payload) = signing_input.split_once('.')?;
    let claims: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    claims.get("video").cloned()
}

/// Whether the grant in a token is the one this Twirp method needs:
/// `roomList` and nothing more for `ListRooms`, `roomAdmin` and nothing
/// more everywhere else.
///
/// This is checked because a call site can reach for the wrong minter and
/// nothing else would notice. `tests/token.rs` pins what each token
/// *contains* down to the null `roomAdmin` on a list token, and every test
/// here goes through a production function, so *which* of the two each call
/// site mints is pinned nowhere: swap `livekit_room_list_token` and
/// `livekit_room_admin_token` between `validate_livekit_credentials` and
/// `livekit_room_service_request` and both files still pass. The direction
/// that matters is the credential check's: its whole job is to prove a key
/// works as cheaply as a key can be proved, and handing that probe the
/// authority to mutate a room is an over-grant on the one path with the
/// least excuse for one.
///
/// "Matches" is the needed grant present and true and the other one absent,
/// rather than merely the needed one present. Present-only would catch the
/// straight swap -- neither token carries the other's grant -- but it would
/// wave through a token carrying both, and a token carrying both is exactly
/// the over-grant this exists to see. Absent rather than false-or-absent
/// because both minters omit the key they do not use, so anything spelling
/// it out is a third shape that should come past here for a reading.
///
/// It stops there. Expiry, the room name inside a `roomAdmin` grant and the
/// identity are all unread, and a real LiveKit reads every one of them:
/// this is a test double noticing that a caller picked the wrong token, not
/// an emulation of LiveKit's authorization.
fn grant_fits_method(token: &str, method: &str) -> bool {
    let Some(video) = token_video_grant(token) else {
        return false;
    };
    let granted = |name: &str| video.get(name).and_then(serde_json::Value::as_bool) == Some(true);
    let quiet = |name: &str| video.get(name).is_none();

    // Every method named, and anything else refused. A catch-all reading "and
    // everything else needs roomAdmin" would hand an assumed grant to the next
    // call this file learns to make, which is the one nobody has thought about
    // yet; refusing it costs whoever adds it one line here and a moment
    // deciding which token it should carry.
    //
    // `quiet` is the half no test reaches, and deliberately: neither minter
    // spells out the grant it does not use, so a token carrying both is a third
    // shape nothing in the tree produces and a fixture would have to forge. It
    // is here because that shape is exactly the over-grant this check exists to
    // see -- `validate_livekit_credentials` holding room authority it has no
    // use for -- and the day something starts minting one, this refuses it
    // rather than reading only the half it expected.
    match method {
        "ListRooms" => granted("roomList") && quiet("roomAdmin"),
        "ListParticipants" | "RemoveParticipant" => granted("roomAdmin") && quiet("roomList"),
        _ => false,
    }
}

/// Whether a RoomService call carries a credential this project would
/// accept: a bearer JWT naming this project as its issuer, signed with this
/// project's secret, and granting what the method being called needs.
///
/// Read backwards from what `livekit_room_service_request` and
/// `validate_livekit_credentials` mint, and deliberately through the
/// crate's own verifier rather than a second copy of it, so a change to the
/// signing this file does not follow surfaces as a refusal here instead of
/// passing unnoticed.
///
/// It is the third copy of a shape `src/web/pool.rs` and `tests/web.rs`
/// also carry, and stays one. Not because sharing is impossible: the
/// `pool.rs` copy is in this same crate, and a `#[cfg(test)]` module at the
/// crate root would reach both with no production surface at all. It stays
/// because what is actually common is three lines -- the issuer, the split,
/// the signature -- while each site's token source differs (two read a
/// `HeaderMap`, this one a raw request head), this one adds
/// `grant_fits_method` that the others have no analogue for, and the doc
/// each carries is about its own call path. Sharing the three lines would
/// leave every part that matters duplicated anyway. The `tests/web.rs` copy
/// could not be shared even if that trade came out the other way: an
/// integration test links the plain rlib, from which `#[cfg(test)]` items
/// have already been stripped, so no visibility makes them reachable.
///
/// The order is the order LiveKit's own receiver uses and is forced: `iss`
/// names the project whose secret verifies the token, so it comes out of an
/// unverified payload first, and the grant is only read once the signature
/// has vouched for the claims it sits in.
fn room_service_credential_accepted(
    head: &str,
    path: &str,
    api_key: &str,
    api_secret: &str,
) -> bool {
    let Some(token) = bearer_token(head) else {
        return false;
    };
    if crate::token::livekit_token_issuer(token).as_deref() != Some(api_key) {
        return false;
    }
    let Some((signing_input, signature)) = token.rsplit_once('.') else {
        return false;
    };
    if !crate::token::verify_hs256(api_secret, signing_input, signature) {
        return false;
    }
    grant_fits_method(token, path.rsplit('/').next().unwrap_or_default())
}

/// A RoomService that answers what the test tells it to and remembers what
/// it was asked, once the caller proves it holds this project's
/// credentials; anything else gets the 401 a real LiveKit answers an
/// unsigned Twirp call with.
///
/// A real socket rather than an injected transport, because what these
/// functions do with a reply is bound up with how they make the request:
/// the Twirp path, the bearer token and the status handling are the parts
/// worth pinning, and none of them survives being stubbed out one layer up.
/// Replies are answered in order and the last one repeats, so a caller that
/// polls needs one entry rather than twenty.
///
/// The credential check is here because the bearer token was the one thing
/// on that list nothing looked at: the head was scanned for
/// `content-length:` and for nothing else, so every call was answered on
/// its path and its body alone. Dropping `.bearer_auth`, signing with
/// another project's secret, or minting a token for a project this server
/// does not serve would all have left every assertion below intact.
/// `validate_livekit_credentials` exists to prove a LiveKit credential
/// works and was the worst of it: it reported success against a server that
/// never read one, so the Setup form's whole answer rested on a check no
/// test performed. That is the same shape as the GitHub stub that answered
/// one profile to any bearer token and let a broken token exchange pass the
/// whole suite.
struct RoomService {
    url: String,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl RoomService {
    async fn start(replies: Vec<(u16, &'static str)>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        tokio::spawn(async move {
            let mut replies = replies.into_iter().peekable();
            let mut last = (200, "{}");
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                if replies.peek().is_some() {
                    last = replies.next().unwrap();
                }

                // Headers to the blank line, then exactly the declared body.
                let mut raw = Vec::new();
                let mut byte = [0u8; 1];
                while !raw.ends_with(b"\r\n\r\n") {
                    match socket.read(&mut byte).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => raw.push(byte[0]),
                    }
                }
                let head = String::from_utf8_lossy(&raw).to_string();
                let path = head
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default()
                    .to_string();
                let length: usize = head
                    .to_ascii_lowercase()
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0u8; length];
                if length > 0 && socket.read_exact(&mut body).await.is_err() {
                    return;
                }
                let accepted =
                    room_service_credential_accepted(&head, &path, STUB_API_KEY, STUB_API_SECRET);
                recorded
                    .lock()
                    .unwrap()
                    .push((path, String::from_utf8_lossy(&body).to_string()));

                // Recorded, and its scripted reply consumed, whether or not the
                // credential passed: a refused call is still a call the
                // production code made, and `requests()` is what the tests read
                // the method and the body out of. A refusal that vanished from
                // that list would look like a request that was never sent.
                let (status, reply) = if accepted {
                    last
                } else {
                    (401, r#"{"code":"unauthenticated","msg":"invalid token"}"#)
                };
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });
        Self { url, seen }
    }

    fn config(&self) -> AgentConfig {
        crate::config::load_from_pairs([
            ("LIVEKIT_URL", self.url.as_str()),
            ("LIVEKIT_API_KEY", STUB_API_KEY),
            ("LIVEKIT_API_SECRET", STUB_API_SECRET),
            ("GOOGLE_API_KEY", "google"),
        ])
        .unwrap()
    }

    fn requests(&self) -> Vec<(String, String)> {
        self.seen.lock().unwrap().clone()
    }
}

/// The request this makes, and what it does with each kind of answer.
///
/// The Twirp path is a contract with LiveKit that no test named: a wrong
/// method segment reaches a real server and 404s, which
/// `is_participant_gone` then has to be careful not to read as a departed
/// participant. The success path returned the parsed body and the failure
/// path had to keep the method, the status and the body, because that
/// string is the only thing the caller can tell them apart by.
#[tokio::test]
async fn a_room_service_call_names_its_method_and_carries_the_answer() {
    let service = RoomService::start(vec![(200, r#"{"participants":[{"identity":"a"}]}"#)]).await;
    let config = service.config();

    let participants = list_room_participants(&config, "interview-abc12345", 1_700_000_000)
        .await
        .unwrap();
    assert_eq!(participants["participants"][0]["identity"], "a");

    let (path, body) = service.requests().into_iter().next().unwrap();
    assert_eq!(path, "/twirp/livekit.RoomService/ListParticipants");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["room"],
        "interview-abc12345"
    );

    // A refusal is an error, and the message carries all three parts the caller
    // reads it for.
    let refusing = RoomService::start(vec![(503, r#"{"msg":"busy"}"#)]).await;
    let error = list_room_participants(&refusing.config(), "interview-abc12345", 1_700_000_000)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("ListParticipants"), "{error}");
    assert!(error.contains("503"), "{error}");
    assert!(error.contains("busy"), "{error}");
}

/// The Setup form's credential check, and the sentence it hands back.
///
/// This is what a reader acts on: the form shows LiveKit's own account of
/// the refusal, because a status alone says "did not work" to somebody who
/// already knows that. It is bounded because a wrong host answers with an
/// HTML page and the form is one line, and the boundary is the part worth
/// pinning: an off-by-one there either truncates a reason that fit or lets
/// a whole page through.
#[tokio::test]
async fn a_credential_check_reports_what_livekit_said() {
    let accepted = RoomService::start(vec![(200, "{}")]).await;
    validate_livekit_credentials(&accepted.url, STUB_API_KEY, STUB_API_SECRET, 1_700_000_000)
        .await
        .expect("a project that answers ListRooms has usable credentials");
    let (path, _) = accepted.requests().into_iter().next().unwrap();
    assert_eq!(path, "/twirp/livekit.RoomService/ListRooms");

    // A refusal the token cannot account for: the credentials are this
    // project's, and the server says no anyway, which is what a key that has
    // been revoked or was never on this server looks like. It is the scripted
    // reply that is under test here, because the message the Setup form repeats
    // is the only thing a reader can act on.
    let refused = RoomService::start(vec![(401, r#"{"msg":"invalid api key"}"#)]).await;
    let error =
        validate_livekit_credentials(&refused.url, STUB_API_KEY, STUB_API_SECRET, 1_700_000_000)
            .await
            .unwrap_err();
    assert!(error.contains("401"), "{error}");
    assert!(error.contains("invalid api key"), "{error}");
    assert!(
        !error.ends_with("..."),
        "a reason that fits is not truncated"
    );

    // And a secret that is not this project's is refused by the server itself,
    // over a script that would otherwise answer 200. This is the case the check
    // exists for and the one that used to pass: the whole point of this
    // function is that a working credential and a broken one come back
    // differently, and against a server that read no credential they did not.
    let wrong_secret = RoomService::start(vec![(200, "{}")]).await;
    let error = validate_livekit_credentials(
        &wrong_secret.url,
        STUB_API_KEY,
        "not-this-project's-secret",
        1_700_000_000,
    )
    .await
    .unwrap_err();
    assert!(error.contains("401"), "{error}");
}

/// The credential check on the RoomService server is a tripwire, and this
/// is what proves the wire is live.
///
/// Every other test in this file reads the server's answer through the
/// production code, and each of those that scripts a success would go on
/// passing if the server quietly stopped checking: the call would arrive
/// with no credential at all and still be answered 200. Asking the server
/// directly is the only place the refusal is observable on its own, which
/// is what keeps the check from decaying back into the header scan it
/// replaced.
#[tokio::test]
async fn the_room_service_refuses_a_call_that_carries_the_wrong_credential() {
    /// The two minters the production code above reaches for, called the
    /// way it calls them, so a change to either fails here rather than
    /// being copied into this fixture and agreeing with itself.
    fn list_token(api_key: &str, api_secret: &str) -> String {
        crate::token::livekit_room_list_token(api_key, api_secret, 1_700_000_000).unwrap()
    }
    fn admin_token(api_key: &str, api_secret: &str) -> String {
        crate::token::livekit_room_admin_token(
            api_key,
            api_secret,
            "interview-abc12345",
            1_700_000_000,
        )
        .unwrap()
    }

    let service = RoomService::start(vec![(200, "{}")]).await;
    let client = crate::http_client();
    let call = |method: &str, bearer: Option<String>| {
        let url = format!("{}/twirp/livekit.RoomService/{method}", service.url);
        async move {
            let request = client.post(url).json(&serde_json::json!({}));
            let request = match bearer {
                Some(token) => request.bearer_auth(token),
                None => request,
            };
            request.send().await.unwrap().status()
        }
    };

    for (why, method, bearer) in [
        ("no credential at all", "ListRooms", None),
        (
            "a bearer that is not a token",
            "ListRooms",
            Some("not-a-jwt".to_string()),
        ),
        (
            "a token signed with another project's secret",
            "ListRooms",
            Some(list_token(STUB_API_KEY, "someone-elses-secret")),
        ),
        // Well formed, correctly signed, and for a project this server does not
        // serve. A pool holds more than one LiveKit project, so reaching for
        // the wrong entry's credentials is a real way to get this wrong and one
        // that only a per-project check can see.
        (
            "a token minted for another project",
            "ListRooms",
            Some(list_token("other-key", STUB_API_SECRET)),
        ),
        // A method nothing in this file sends, asked with a credential that is
        // otherwise perfect. This is what fail-closed means here and it is the
        // only thing that asks for it: with a catch-all arm reading "everything
        // else needs roomAdmin", the next call somebody teaches this module to
        // make would arrive holding a grant nobody chose for it, and would be
        // answered.
        (
            "a method this server was never taught",
            "DeleteRoom",
            Some(admin_token(STUB_API_KEY, STUB_API_SECRET)),
        ),
        // The two halves of the swap. Both are this project's, both verify, and
        // each asks a method the other's grant is for: a room-admin token on
        // the credential check's path is the over-grant, since `ListRooms`
        // needs nothing but `roomList` and that path exists to prove a key as
        // cheaply as a key can be proved; a list token on a path that
        // administers a room is the same mistake read the other way, and is the
        // half a real LiveKit would refuse outright.
        (
            "a room-admin token on the credential check's path",
            "ListRooms",
            Some(admin_token(STUB_API_KEY, STUB_API_SECRET)),
        ),
        (
            "a list token on a path that administers a room",
            "ListParticipants",
            Some(list_token(STUB_API_KEY, STUB_API_SECRET)),
        ),
    ] {
        assert_eq!(
            call(method, bearer).await,
            401,
            "the server must refuse {why}, or it is not checking anything"
        );
    }

    // And each credential on the path production sends it to is accepted, so
    // the six refusals are a check rather than a server that refuses
    // everything.
    assert_eq!(
        call("ListRooms", Some(list_token(STUB_API_KEY, STUB_API_SECRET))).await,
        200
    );
    assert_eq!(
        call(
            "ListParticipants",
            Some(admin_token(STUB_API_KEY, STUB_API_SECRET))
        )
        .await,
        200
    );
}

/// A wrong host answers with a page, and the form takes one line of it.
#[tokio::test]
async fn a_credential_check_bounds_the_reason_it_repeats() {
    // Longer than the limit by one, which is the only length that tells the
    // boundary apart from its neighbours.
    const OVER: &str = concat!(
        "0123456789012345678901234567890123456789012345678901234567890123456789",
        "0123456789012345678901234567890123456789012345678901234567890123456789",
        "0123456789012345678901234567890123456789012345678901234567890123456789",
    );
    assert_eq!(OVER.len(), 210);

    let wordy = RoomService::start(vec![(500, OVER)]).await;
    let error =
        validate_livekit_credentials(&wordy.url, STUB_API_KEY, STUB_API_SECRET, 1_700_000_000)
            .await
            .unwrap_err();
    assert!(
        error.ends_with("..."),
        "an over-long reason is cut and marked"
    );
    assert!(
        error.contains(&OVER[..200]) && !error.contains(&OVER[..201]),
        "exactly the limit is repeated: {error}"
    );

    // Exactly the limit, which is the length that tells `>` from `>=`: a reason
    // that fits to the last character is whole, not cut.
    let exact = &OVER[..200];
    let fitting = RoomService::start(vec![(500, exact)]).await;
    let error =
        validate_livekit_credentials(&fitting.url, STUB_API_KEY, STUB_API_SECRET, 1_700_000_000)
            .await
            .unwrap_err();
    assert!(
        error.ends_with(exact),
        "a reason of exactly the limit is not marked as cut: {error}"
    );
}

/// A late duplicate is evicted by name.
///
/// This runs when a second agent joins after isolation has already passed,
/// which is the case isolation cannot cover: the room was clear when it
/// looked. It takes an identity rather than the participant it came from,
/// because the identity is all it needs and a participant cannot be built
/// to test with.
#[tokio::test]
async fn a_late_duplicate_is_evicted_by_name() {
    let service = RoomService::start(vec![(200, "{}")]).await;

    evict_duplicate_agent(
        &service.config(),
        "interview-abc12345",
        "agent-late",
        1_700_000_000,
    )
    .await
    .unwrap();

    let (path, body) = service.requests().into_iter().next().unwrap();
    assert_eq!(path, "/twirp/livekit.RoomService/RemoveParticipant");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["identity"],
        "agent-late"
    );
}

/// A removal that finds nobody is done, and every other refusal is not.
///
/// The predicate is unit tested above; this is the path that consults it,
/// which is where losing the race with a departing agent used to end the
/// interview.
#[tokio::test]
async fn a_removal_consults_the_predicate_before_giving_up() {
    let gone = RoomService::start(vec![(
        404,
        r#"{"code":"not_found","msg":"participant does not exist"}"#,
    )])
    .await;
    remove_room_participant(
        &gone.config(),
        "interview-abc12345",
        "agent-old",
        1_700_000_000,
    )
    .await
    .expect("a participant that already left is not a failure");
    let (path, _) = gone.requests().into_iter().next().unwrap();
    assert_eq!(path, "/twirp/livekit.RoomService/RemoveParticipant");

    let refused = RoomService::start(vec![(401, r#"{"code":"unauthenticated"}"#)]).await;
    remove_room_participant(
        &refused.config(),
        "interview-abc12345",
        "agent-old",
        1_700_000_000,
    )
    .await
    .expect_err("a refusal that is not a departed participant stays fatal");
}

/// Isolation lists, removes what it finds, and stops.
///
/// The loop is the part worth pinning: it re-lists after removing, so the
/// last pass is the one that confirms the room is clear, and it gives up
/// loudly rather than spinning forever. Nothing asserted that it removed
/// anything at all.
#[tokio::test]
async fn isolation_removes_a_duplicate_and_confirms_the_room_is_clear() {
    // Crowded once, then empty, which is the ordinary case: list, remove, list
    // again and find nothing.
    let service = RoomService::start(vec![
        (
            200,
            r#"{"participants":[{"identity":"agent-old","kind":"AGENT"},{"identity":"agent-me","kind":"AGENT"}]}"#,
        ),
        (200, r#"{}"#),
        (200, r#"{"participants":[]}"#),
    ])
    .await;

    isolate_local_agent(
        &service.config(),
        "interview-abc12345",
        "agent-me",
        1_700_000_000,
    )
    .await
    .unwrap();

    let methods: Vec<String> = service
        .requests()
        .into_iter()
        .map(|(path, _)| path.rsplit('/').next().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        methods,
        vec![
            "ListParticipants".to_string(),
            "RemoveParticipant".to_string(),
            "ListParticipants".to_string(),
        ],
        "it lists, removes what it found, and lists again to confirm"
    );
}

#[test]
fn duplicate_agent_identities_ignores_candidate_and_local_agent() {
    let participants = serde_json::json!({
        "participants": [
            {
                "identity": "candidate-fixed",
                "kind": "STANDARD",
                "permission": { "agent": false }
            },
            {
                "identity": "interviewer-interview-fixed",
                "kind": "AGENT",
                "permission": { "agent": true }
            },
            {
                "identity": "hosted-agent",
                "kind": "AGENT",
                "permission": { "agent": true }
            },
            {
                "identity": "legacy-agent",
                "permission": { "agent": true }
            }
        ]
    });

    assert_eq!(
        duplicate_agent_identities(&participants, "interviewer-interview-fixed"),
        vec!["hosted-agent".to_string(), "legacy-agent".to_string()]
    );
}

#[test]
fn livekit_http_base_maps_websocket_urls_to_room_service_host() {
    assert_eq!(
        livekit_http_base("wss://example.livekit.cloud/"),
        "https://example.livekit.cloud"
    );
    assert_eq!(
        livekit_http_base("ws://localhost:7880"),
        "http://localhost:7880"
    );

    // Everything `validate_livekit_url` lets through. The scheme is matched
    // case-insensitively there, and a query, a fragment and a path are all
    // accepted, so each of these is a URL a deployment can be holding.
    assert_eq!(
        livekit_http_base("WSS://example.livekit.cloud"),
        "https://example.livekit.cloud"
    );
    assert_eq!(
        livekit_http_base("Ws://localhost:7880"),
        "http://localhost:7880"
    );
    assert_eq!(
        livekit_http_base("wss://example.livekit.cloud/?region=eu"),
        "https://example.livekit.cloud"
    );

    // The path stays, because a proxy mounts RoomService under it. Only the
    // query and the fragment come off, and neither can precede a path.
    assert_eq!(
        livekit_http_base("wss://example.livekit.cloud/livekit/"),
        "https://example.livekit.cloud/livekit"
    );
    assert_eq!(
        livekit_http_base("https://example.livekit.cloud/sfu#frag"),
        "https://example.livekit.cloud/sfu"
    );

    // Shapes validation also accepts, neither of which this may mangle.
    assert_eq!(
        livekit_http_base("wss://user:pass@example.livekit.cloud/sfu"),
        "https://user:pass@example.livekit.cloud/sfu"
    );
    assert_eq!(livekit_http_base("ws://[::1]:7880"), "http://[::1]:7880");
}

/// `validate_livekit_url` compares the scheme without regard to case, so
/// every spelling it accepts reaches here. The host keeps the case it was
/// given: DNS does not care, and a path might.
#[test]
fn livekit_http_base_rewrites_a_scheme_in_any_case() {
    assert_eq!(
        livekit_http_base("WSS://Example.LiveKit.Cloud"),
        "https://Example.LiveKit.Cloud"
    );
    assert_eq!(
        livekit_http_base("Ws://localhost:7880/"),
        "http://localhost:7880"
    );
    assert_eq!(
        livekit_http_base("HTTPS://example.livekit.cloud"),
        "https://example.livekit.cloud"
    );
}

/// All three parts are required, and nothing said so.
///
/// The predicate exists because the opposite shipped. A removal that finds
/// nothing to remove has done what it was asked to do, and this used to
/// propagate the 404 instead: the agent greeted the candidate, tried to
/// evict a duplicate that had already left on its own, took the `not_found`
/// as fatal, and exited. The candidate was left in a room with no
/// interviewer, the status pill correctly reading "Waiting" and nothing on
/// screen explaining why. Losing that race is the common case, not the
/// exceptional one, which is why the first assertion here is the one that
/// has to keep passing.
///
/// The three refusals are the other half. Each conjunct is load bearing for
/// a different reason: the method keeps a 404 from any other RoomService
/// call fatal, the status distinguishes a refusal from a failure, and the
/// Twirp code separates a participant that is gone from an HTML error page
/// at the wrong route. Dropping any one of them widens this to a
/// participant that is still there, and the caller treats that as a
/// successful eviction.
///
/// Each case therefore withholds exactly one part and supplies the other
/// two, which is what makes them separate the conjuncts. A case missing two
/// at once is refused whichever conjunct is dropped, so it pins nothing.
#[test]
fn a_participant_is_gone_only_when_all_three_parts_say_so() {
    let gone = "RemoveParticipant failed: 404 {\"code\":\"not_found\"}";
    assert!(is_participant_gone(gone));

    for missing in [
        // Another RoomService call answering 404 is still fatal.
        "ListParticipants failed: 404 {\"code\":\"not_found\"}",
        // The right call and the right code, but not a refusal.
        "RemoveParticipant failed: 500 {\"code\":\"not_found\"}",
        // A 404 from the wrong route, which is an error page and not Twirp.
        "RemoveParticipant failed: 404 <html>no such route</html>",
    ] {
        assert!(!is_participant_gone(missing), "{missing}");
    }
}

#[test]
fn room_service_response_rejects_malformed_success_json() {
    let error = parse_room_service_response("ListParticipants", "not json")
        .unwrap_err()
        .to_string();

    assert!(error.contains("ListParticipants returned invalid JSON"));
}
