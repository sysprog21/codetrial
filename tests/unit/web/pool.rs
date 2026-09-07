//! The `tests` module of `src/web/pool.rs`, which declares this file by path.
//! Everything here reaches into `src/web/pool.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use super::*;
use crate::config::{PRIMARY_PROVIDER_ID, Provider, provider_id_from_room};

/// The LiveKit credentials every project in `config_with` is built from,
/// and the pair [`quota_stub`] verifies the probe's token against. Named
/// rather than repeated so the two cannot drift: a stub holding a different
/// secret from the config under test would refuse every probe, which reads
/// as a quota answer rather than as a broken fixture. Nothing here is a
/// real credential; the server it signs for exists only in this binary.
const STUB_API_KEY: &str = "k";
const STUB_API_SECRET: &str = "s";

fn config_with(ids: &[&str]) -> WebServerConfig {
    WebServerConfig {
        web_dir: std::path::PathBuf::new(),
        github_client_id: None,
        github_client_secret: None,
        session_secret: None,
        db_path: None,
        github_oauth_base_url: None,
        github_api_base_url: None,
        room_prefix: "interview".to_string(),
        fixed_room_name: None,
        production: false,
        compiler_explorer_enabled: false,
        trusted_proxy_hops: 0,
        recording: None,
        probe_provider_quota: false,
        pool: ProviderPool {
            providers: ids
                .iter()
                .map(|id| Provider {
                    id: (*id).to_string(),
                    url: "wss://host.example".to_string(),
                    api_key: STUB_API_KEY.to_string(),
                    api_secret: STUB_API_SECRET.to_string(),
                    google_api_key: String::new(),
                })
                .collect(),
        },
    }
}

/// Whether a probe carries a credential this project would accept: a
/// bearer JWT naming this project as its issuer and signed with this
/// project's secret.
///
/// Read backwards from what `probe_not_exhausted` mints, and deliberately
/// through the crate's own verifier rather than a second copy of it, so a
/// change to the signing that this file does not follow shows up as a
/// refusal here instead of passing unnoticed.
///
/// One of three copies of this shape; `tests/web.rs` and
/// `src/livekit/rooms.rs` carry the others, and the one in
/// `src/livekit/rooms.rs` states why they stay copies.
fn probe_credential_accepted(
    headers: &axum::http::HeaderMap,
    api_key: &str,
    api_secret: &str,
) -> bool {
    let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    if crate::token::livekit_token_issuer(token).as_deref() != Some(api_key) {
        return false;
    }

    // `rsplit_once` splits a four-segment string as happily as a three-segment
    // one, so a structural check belongs somewhere -- but it is already above.
    // `livekit_token_issuer` takes the payload as everything between the first
    // dot and the last, and base64url has no dot in it, so a token with a
    // segment glued on fails to decode there and never reaches this line. A
    // check here would be a line no test could reach; the test in
    // `tests/web.rs` asks for the four-segment case instead, so that a parser
    // which stopped refusing it is a failure rather than a silent widening.
    let Some((signing_input, signature)) = token.rsplit_once('.') else {
        return false;
    };
    crate::token::verify_hs256(api_secret, signing_input, signature)
}

/// A stub that answers one status to a probe carrying this project's
/// credential, standing in for a LiveKit project with or without minutes
/// left, and 401 to anything else.
///
/// The credential check is the difference between a test that exercises the
/// probe and one that only exercises its URL. `/rtc/validate` is an
/// authenticated endpoint: the real LiveKit answers 401 to an anonymous GET
/// and never reaches the quota question at all. A stub that answered its
/// status to any caller would let the probe drop `.bearer_auth`, sign with
/// the wrong project's secret, or mint a token for a project it is not
/// asking about, and every test here would still pass -- while production
/// learned nothing about quota, because 401 is not 429 and this module
/// reads everything that is not 429 as "still has minutes". That is the
/// same shape as the GitHub stub that answered one profile to any bearer
/// token, which let a broken token exchange pass the whole suite.
///
/// A pool is more than one project, so the check is against this project's
/// key and secret rather than merely against a well-formed token: probing
/// project B with project A's credential is a failure only a per-project
/// check can see, and it is the failure a rotation over several projects is
/// most able to make.
async fn quota_stub(status: axum::http::StatusCode) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = axum::Router::new().route(
        "/rtc/validate",
        axum::routing::get(move |headers: axum::http::HeaderMap| async move {
            if !probe_credential_accepted(&headers, STUB_API_KEY, STUB_API_SECRET) {
                return axum::http::StatusCode::UNAUTHORIZED;
            }
            status
        }),
    );
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("ws://127.0.0.1:{port}"), server)
}

/// `refresh_all` decides which projects the rotation may still use, so
/// returning the wrong set is the difference between routing around a spent
/// project and routing into it. Dropping the negation reported exactly the
/// projects that still had minutes, and nothing noticed.
/// The credential check on the quota stub is a tripwire, and this is what
/// proves the wire is live.
///
/// The only other caller reads the stub's answer through `refresh_all`,
/// which reads everything that is not 429 as "still has minutes" -- so a
/// stub that quietly stopped checking would go on passing there: the probe
/// would arrive with no credential, be refused, and the refusal would read
/// as a healthy project. Asking the stub directly is the only place the
/// refusal is observable on its own, and without it this check is pinned
/// only in the direction that does not matter, the one where it refuses
/// everything.
#[tokio::test]
async fn the_quota_stub_refuses_a_probe_that_carries_the_wrong_credential() {
    /// The credential the probe mints, so the test asks with what
    /// `probe_not_exhausted` asks with rather than with a token shaped
    /// like it.
    fn probe_token(api_key: &str, api_secret: &str) -> String {
        crate::token::livekit_token(crate::token::LivekitTokenInput {
            api_key,
            api_secret,
            name: "quota-probe",
            identity: "quota-probe",
            room: "quota-probe",
            metadata: "",
            now_seconds: 1_700_000_000,
            agent: false,
        })
        .unwrap()
    }

    let (url, server) = quota_stub(axum::http::StatusCode::OK).await;

    // The pool entry is a `ws://` URL and the probe reaches the same server
    // over `http://`, through the conversion production uses.
    let origin = super::super::policy::livekit_http_origin(&url).unwrap();
    let probe = format!("{origin}/rtc/validate");
    let client = crate::http_client();

    for (why, bearer) in [
        ("no credential at all", None),
        (
            "a bearer that is not a token",
            Some("not-a-jwt".to_string()),
        ),
        (
            "a token signed with another project's secret",
            Some(probe_token(STUB_API_KEY, "someone-elses-secret")),
        ),
        // Well formed, correctly signed, and for a project this stub does not
        // serve. A pool holds more than one LiveKit project, so reaching for
        // the wrong entry's credentials is a real way to get this wrong and one
        // only a per-project check can see.
        (
            "a token minted for another project",
            Some(probe_token("other-key", STUB_API_SECRET)),
        ),
    ] {
        let request = client.get(&probe);
        let request = match bearer {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        assert_eq!(
            request.send().await.unwrap().status(),
            401,
            "the stub must refuse {why}, or it is not checking anything"
        );
    }

    // And the credential the pool actually mints is accepted, so the four
    // refusals are a check rather than a stub that refuses everything.
    let accepted = client
        .get(&probe)
        .bearer_auth(probe_token(STUB_API_KEY, STUB_API_SECRET))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), 200);

    server.abort();
}

#[tokio::test]
async fn refresh_all_names_the_projects_that_refused() {
    let (spent_url, spent_server) = quota_stub(axum::http::StatusCode::TOO_MANY_REQUESTS).await;
    let (healthy_url, healthy_server) = quota_stub(axum::http::StatusCode::OK).await;

    let mut config = config_with(&["spent", "healthy"]);
    config.pool.providers[0].url = spent_url;
    config.pool.providers[1].url = healthy_url;

    let quota = ProviderQuota::default();
    assert_eq!(
        quota.refresh_all(&config.pool).await,
        vec!["spent".to_string()],
        "only the project that answered 429 is out"
    );

    // And the verdicts landed in the cache the request path reads, so a token
    // request pays no probe of its own.
    assert!(!quota.not_known_exhausted(&config.pool.providers[0]).await);
    assert!(quota.not_known_exhausted(&config.pool.providers[1]).await);

    spent_server.abort();
    healthy_server.abort();
}

/// The relation the refresher depends on, asserted rather than described.
///
/// Deriving the interval from the TTL put the arithmetic in one place but
/// left it unchecked: turning the division into a multiplication gives a
/// refresh slower than the expiry it exists to beat, so verdicts go stale
/// between passes and the probe lands back on the request path. Nothing
/// about that fails a test unless the relation itself is one.
#[test]
fn a_verdict_is_refreshed_before_it_can_expire() {
    assert!(
        PROVIDER_QUOTA_REFRESH < PROVIDER_QUOTA_TTL,
        "refresh every {:?} cannot keep a {:?} cache warm",
        PROVIDER_QUOTA_REFRESH,
        PROVIDER_QUOTA_TTL
    );

    // Not merely shorter: short enough that a pass can be missed and the next
    // one still lands inside the TTL.
    assert!(PROVIDER_QUOTA_REFRESH * 2 <= PROVIDER_QUOTA_TTL);
}

/// The cache boundary, stated exactly. At the TTL the verdict is stale, so
/// the next caller re-probes rather than answering from a record that has
/// just expired.
#[test]
fn a_verdict_is_stale_the_instant_it_reaches_the_ttl() {
    assert!(is_fresh(Duration::ZERO), "a verdict taken now is fresh");
    assert!(is_fresh(PROVIDER_QUOTA_TTL - Duration::from_nanos(1)));
    assert!(
        !is_fresh(PROVIDER_QUOTA_TTL),
        "at the TTL it has expired, not just after it"
    );
    assert!(!is_fresh(PROVIDER_QUOTA_TTL + Duration::from_secs(1)));
}

/// The three arms an operator reads. Each one is a different instruction:
/// nothing to do, top one account up, or every interview is about to be
/// refused. Before these were strings the arms only printed, so a mutant
/// could collapse all three into one and no test could tell.
#[test]
fn the_startup_line_says_which_of_the_three_situations_this_is() {
    assert_eq!(
        pool_health_line(&[], 9),
        "livekit quota: all 9 project(s) can take connections"
    );
    assert_eq!(
        pool_health_line(&["primary".to_string()], 9),
        "livekit quota: 8 of 9 project(s) available; out of minutes: primary"
    );

    // All of them, which is the one an operator has to act on immediately. The
    // count and the names both matter, so a guard that fired on the wrong
    // comparison would be reporting the wrong emergency.
    assert_eq!(
        pool_health_line(&["a".to_string(), "b".to_string()], 2),
        "livekit quota: every project is out of connection minutes (a, b); \
         interviews will be refused until one is topped up"
    );

    // One project, and it is spent: still the every-project case.
    assert_eq!(
        pool_health_line(&["only".to_string()], 1),
        "livekit quota: every project is out of connection minutes (only); \
         interviews will be refused until one is topped up"
    );
}

/// The refresher prints only on a change, so "did anything move" is the
/// whole decision.
#[test]
fn a_verdict_that_did_not_move_prints_nothing() {
    let spent = vec!["primary".to_string()];
    assert_eq!(quota_change_line(&spent, &spent), None, "nothing moved");
    assert_eq!(quota_change_line(&[], &[]), None, "still all healthy");

    assert_eq!(
        quota_change_line(&[], &spent).as_deref(),
        Some("livekit quota: now out of minutes: primary")
    );
    assert_eq!(
        quota_change_line(&spent, &[]).as_deref(),
        Some("livekit quota: every project can take connections again"),
        "recovery is the line an operator is waiting for"
    );
    assert_eq!(
        quota_change_line(&spent, &["other".to_string()]).as_deref(),
        Some("livekit quota: now out of minutes: other"),
        "a different project going spent is a change, not a repeat"
    );
}

/// The guard exists so the refresher cannot outlive the server that wanted
/// it. Nothing proved it: `drop` could have been empty and every test still
/// passed, which is precisely the leak this type was added to stop.
#[tokio::test]
async fn dropping_the_refresher_aborts_the_task_it_owns() {
    let task = tokio::spawn(async {
        // Never finishes on its own, so anything that ends it is the abort.
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
    let watch = task.abort_handle();
    assert!(!watch.is_finished(), "the task is running before the drop");

    drop(QuotaRefresher(Some(task)));

    // The abort lands at the task's next scheduling point, not inside `drop`,
    // so this yields rather than asserting straight away.
    for _ in 0..100 {
        if watch.is_finished() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("the task outlived the QuotaRefresher that owned it");
}

/// The room name is a contract between two modules that never call each
/// other: this one mints it, and `config::provider_id_from_room` reads it
/// back in a process that was handed nothing else. Until now nothing tied
/// the two together. `tests/config.rs` hand-writes an example of what this
/// function mints, which is a copy of the format rather than a check of it,
/// so changing the separator here left that test green.
///
/// The dashed id is the case that matters. `provider_id_from_room` splits
/// at the *final* dash, and its doc rests that choice on the suffix
/// alphabet holding no dash, which is a fact about a constant in a third
/// module. Round-tripping an id that itself contains a dash is what makes
/// that assumption checkable from here: a dash in the suffix would move the
/// split point and hand back `eu-west-<part>` instead of `eu-west`.
#[test]
fn every_room_name_this_mints_parses_back_to_the_provider_it_named() {
    let config = config_with(&[PRIMARY_PROVIDER_ID, "eu", "eu-west"]);

    // Several draws each, because the suffix is random and one sample says
    // nothing about an alphabet.
    for counter in 0..30 {
        let (room_name, provider) = room_and_provider(&config, counter);
        let provider = provider.expect("a non-empty pool always answers");
        let parsed = provider_id_from_room(&room_name, &config.room_prefix);

        if provider.id == PRIMARY_PROVIDER_ID {
            // The primary contributes no segment, so what parses out is
            // whatever the suffix looks like, and `for_room` resolves that miss
            // back to the primary. Asserting the round trip here would pin the
            // wrong rule.
            assert_eq!(
                config
                    .pool
                    .for_room(&room_name, &config.room_prefix)
                    .map(|it| it.id.as_str()),
                Some(PRIMARY_PROVIDER_ID),
                "a segment-less room must resolve to the primary: {room_name}"
            );
        } else {
            assert_eq!(
                parsed,
                Some(provider.id.as_str()),
                "minted {room_name} for {} and read back {parsed:?}",
                provider.id
            );
        }
    }
}
