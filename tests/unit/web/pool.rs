//! The `tests` module of `src/web/pool.rs`, which declares this file by path.
//! Everything here reaches into `src/web/pool.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use super::*;
use crate::config::{PRIMARY_PROVIDER_ID, Provider, provider_id_from_room};
use std::sync::atomic::{AtomicUsize, Ordering};

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
                    google_api_keys: Vec::new(),
                })
                .collect(),
        },
    }
}

/// Whether a probe carries a credential this project would accept: a
/// bearer JWT naming this project as its issuer and signed with this
/// project's secret.
///
/// Read backwards from what `probe_verdict` mints, and deliberately
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
/// left, and 401 to anything else. The returned counter says how many times
/// the project was asked, for the tests whose claim is about that rather than
/// about what it answered.
///
/// The credential check is the difference between a test that exercises the
/// probe and one that only exercises its URL. `/rtc/validate` is an
/// authenticated endpoint: the real LiveKit answers 401 to an anonymous GET
/// and never reaches the quota question at all. A stub that answered its
/// status to any caller would let the probe drop `.bearer_auth`, sign with
/// the wrong project's secret, or mint a token for a project it is not
/// asking about, and every test here would still pass -- while production
/// learned nothing about quota, because the old module read every non-429 as
/// "still has minutes". That is the same shape as the GitHub stub that
/// answered one profile to any bearer token, which let a broken token
/// exchange pass the whole suite.
///
/// A pool is more than one project, so the check is against this project's
/// key and secret rather than merely against a well-formed token: probing
/// project B with project A's credential is a failure only a per-project
/// check can see, and it is the failure a rotation over several projects is
/// most able to make.
///
/// [`quota_stub`] is this without the counter. Written once so the credential
/// tripwire above cannot hold in one stub and quietly lapse in the other.
async fn counting_quota_stub(
    status: axum::http::StatusCode,
) -> (
    String,
    std::sync::Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let probes = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = probes.clone();
    let app = axum::Router::new().route(
        "/rtc/validate",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, Ordering::Relaxed);
                if !probe_credential_accepted(&headers, STUB_API_KEY, STUB_API_SECRET) {
                    return axum::http::StatusCode::UNAUTHORIZED;
                }
                status
            }
        }),
    );
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("ws://127.0.0.1:{port}"), probes, server)
}

/// [`counting_quota_stub`] for the tests that do not count probes.
async fn quota_stub(status: axum::http::StatusCode) -> (String, tokio::task::JoinHandle<()>) {
    let (url, _probes, server) = counting_quota_stub(status).await;
    (url, server)
}

/// `refresh_all` decides which projects the rotation may still use, so
/// returning the wrong verdict is the difference between routing around an
/// unavailable project and routing into it. Dropping the negation reported
/// exactly the projects that still had minutes, and nothing noticed.
/// The credential check on the quota stub is a tripwire, and this is what
/// proves the wire is live.
///
/// The only other caller reads the stub's answer through `refresh_all`,
/// which previously read everything that was not 429 as "still has minutes" --
/// so a
/// stub that quietly stopped checking would go on passing there: the probe
/// would arrive with no credential, be refused, and the refusal would read
/// as a healthy project. Asking the stub directly is the only place the
/// refusal is observable on its own, and without it this check is pinned
/// only in the direction that does not matter, the one where it refuses
/// everything.
#[tokio::test]
async fn the_quota_stub_refuses_a_probe_that_carries_the_wrong_credential() {
    /// The credential the probe mints, so the test asks with what
    /// `probe_verdict` asks with rather than with a token shaped
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
async fn refresh_all_preserves_each_project_verdict() {
    let (spent_url, spent_probes, spent_server) =
        counting_quota_stub(axum::http::StatusCode::TOO_MANY_REQUESTS).await;
    let (healthy_url, healthy_probes, healthy_server) =
        counting_quota_stub(axum::http::StatusCode::OK).await;

    let mut config = config_with(&["spent", "healthy"]);
    config.pool.providers[0].url = spent_url;
    config.pool.providers[1].url = healthy_url;

    let quota = ProviderQuota::new(true);
    assert_eq!(
        quota.refresh_all(&config.pool).await,
        vec![
            ("spent".to_string(), ProviderVerdict::OutOfMinutes),
            ("healthy".to_string(), ProviderVerdict::Available),
        ],
        "every cached verdict remains distinguishable"
    );

    // And the verdicts landed in the cache the request path reads, so a token
    // request pays no probe of its own. Counted, because a stub answers a fresh
    // probe exactly as the cache would, and the verdicts alone cannot tell the
    // two apart. `refresh_all` is awaited here, which is what the server-level
    // test cannot do: there, a token request may land between the refresher's
    // probe and its cache write.
    assert_eq!(
        quota.verdict_for(&config.pool.providers[0]).await,
        ProviderVerdict::OutOfMinutes
    );
    assert_eq!(
        quota.verdict_for(&config.pool.providers[1]).await,
        ProviderVerdict::Available
    );
    assert_eq!(
        (
            spent_probes.load(Ordering::Relaxed),
            healthy_probes.load(Ordering::Relaxed)
        ),
        (1, 1),
        "the request path read the refreshed cache rather than probing again"
    );

    spent_server.abort();
    healthy_server.abort();
}

#[tokio::test]
async fn a_project_refusing_the_probe_credential_is_excluded() {
    let (url, probes, server) = counting_quota_stub(axum::http::StatusCode::OK).await;
    let (healthy_url, healthy_server) = quota_stub(axum::http::StatusCode::OK).await;
    let mut config = config_with(&["refused", "healthy"]);
    config.pool.providers[0].url = url;
    config.pool.providers[0].api_secret = "rotated-secret".to_string();
    config.pool.providers[1].url = healthy_url;

    let quota = ProviderQuota::new(true);
    assert_eq!(
        quota.verdict_for(&config.pool.providers[0]).await,
        ProviderVerdict::CredentialRefused(axum::http::StatusCode::UNAUTHORIZED),
        "a 401 is a refusal, not an availability"
    );
    assert_eq!(
        probes.load(Ordering::Relaxed),
        1,
        "the first verdict is probed"
    );

    // The second lookup is the claim: counting the probes is what tells a
    // cached refusal apart from a fresh one, because a stub that answers 401
    // every time answers both the same way.
    assert_eq!(
        quota.verdict_for(&config.pool.providers[0]).await,
        ProviderVerdict::CredentialRefused(axum::http::StatusCode::UNAUTHORIZED),
        "a refusal still inside its cache window is not re-probed"
    );
    assert_eq!(
        probes.load(Ordering::Relaxed),
        1,
        "the second lookup asked the cache, not the project"
    );

    // And excluded, which is the word in this test's name and a stronger claim
    // than the verdict above. The rotation reaches for whatever is still
    // available, so what matters is that the refused project is the only one
    // missing from that set while a healthy project beside it stays in. A
    // second cache, because the one above is warm and this asks what a cold
    // pool decides.
    let rotation = ProviderQuota::new(true);
    assert_eq!(
        unavailable_projects(&rotation.refresh_all(&config.pool).await),
        vec!["refused: credential refused (401)".to_string()],
        "the refused project leaves the rotation and the healthy one does not"
    );

    server.abort();
    healthy_server.abort();
}

#[tokio::test]
async fn a_forbidden_probe_credential_is_excluded() {
    let (url, server) = quota_stub(axum::http::StatusCode::FORBIDDEN).await;
    let mut config = config_with(&["forbidden"]);
    config.pool.providers[0].url = url;

    let quota = ProviderQuota::new(true);
    assert_eq!(
        quota.verdict_for(&config.pool.providers[0]).await,
        ProviderVerdict::CredentialRefused(axum::http::StatusCode::FORBIDDEN),
        "the 403 must keep this project out of rotation"
    );

    server.abort();
}

#[tokio::test]
async fn a_refused_project_returns_after_the_cache_ttl() {
    let (url, server) = quota_stub(axum::http::StatusCode::OK).await;
    let mut config = config_with(&["refused"]);
    config.pool.providers[0].url = url;
    config.pool.providers[0].api_secret = "rotated-secret".to_string();
    let quota = ProviderQuota::new(true);

    assert!(matches!(
        quota.verdict_for(&config.pool.providers[0]).await,
        ProviderVerdict::CredentialRefused(_)
    ));
    config.pool.providers[0].api_secret = STUB_API_SECRET.to_string();

    assert_eq!(
        quota
            .verdict_for_at(
                &config.pool.providers[0],
                Instant::now() + PROVIDER_QUOTA_TTL,
            )
            .await,
        ProviderVerdict::Available,
        "the first stale read must re-probe with the repaired credential"
    );

    server.abort();
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

/// The operator needs the unavailable cause, because topping up a project
/// cannot repair a credential refusal. Before these were strings the arms only
/// printed, so a mutant could collapse them and no test could tell.
#[test]
fn pool_health_lines_name_exhausted_and_refused_projects() {
    assert_eq!(
        pool_health_line(&[], 9),
        "livekit quota: all 9 project(s) can take connections"
    );
    assert_eq!(
        pool_health_line(&[("primary".to_string(), ProviderVerdict::OutOfMinutes)], 9),
        "livekit quota: 8 of 9 project(s) available; unavailable: primary: out of connection minutes"
    );
    assert_eq!(
        pool_health_line(
            &[(
                "refused".to_string(),
                ProviderVerdict::CredentialRefused(axum::http::StatusCode::FORBIDDEN),
            )],
            1,
        ),
        "livekit quota: every project is unavailable (refused: credential refused (403)); \
         interviews will be refused until one recovers"
    );
    assert_eq!(
        pool_health_line(
            &[
                ("spent".to_string(), ProviderVerdict::OutOfMinutes),
                (
                    "refused".to_string(),
                    ProviderVerdict::CredentialRefused(axum::http::StatusCode::FORBIDDEN),
                ),
            ],
            2,
        ),
        "livekit quota: every project is unavailable (spent: out of connection minutes, refused: credential refused (403)); \
         interviews will be refused until one recovers"
    );
}

/// The refresher prints only on a change, so "did anything move" is the
/// whole decision.
#[test]
fn a_verdict_that_did_not_move_prints_nothing() {
    let spent = vec![("primary".to_string(), ProviderVerdict::OutOfMinutes)];
    let refused = vec![(
        "primary".to_string(),
        ProviderVerdict::CredentialRefused(axum::http::StatusCode::UNAUTHORIZED),
    )];
    assert_eq!(quota_change_line(&spent, &spent), None, "nothing moved");
    assert_eq!(quota_change_line(&[], &[]), None, "still all healthy");

    assert_eq!(
        quota_change_line(&[], &spent).as_deref(),
        Some("livekit quota: now unavailable: primary: out of connection minutes")
    );
    assert_eq!(
        quota_change_line(&spent, &[]).as_deref(),
        Some("livekit quota: every project can take connections again"),
        "recovery is the line an operator is waiting for"
    );
    assert_eq!(
        quota_change_line(&spent, &refused).as_deref(),
        Some("livekit quota: now unavailable: primary: credential refused (401)"),
        "a refused credential is a change even when the provider id is the same"
    );
}

/// A disabled probe and an empty pool are separate reasons not to spawn the
/// refresher. Combining them with `&&` starts a task for either disabled
/// configuration, even though there is no useful work for that task to do.
#[tokio::test]
async fn a_refresher_starts_only_for_an_enabled_nonempty_pool() {
    let disabled =
        spawn_provider_quota_refresher(ProviderQuota::new(false), config_with(&["a"]).pool);
    assert!(disabled.is_empty(), "disabled quota probing starts no task");

    let empty = spawn_provider_quota_refresher(ProviderQuota::new(true), config_with(&[]).pool);
    assert!(empty.is_empty(), "an empty pool starts no task");
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

/// The pool's code comes from its worst verdict, whichever order the projects
/// are listed in. Both orders are asserted because `max_by_key` keeps the last
/// of equal keys, so a ranking that stopped distinguishing the verdicts would
/// still pass whenever the right one happened to come last.
#[test]
fn the_worst_verdict_speaks_for_the_pool_in_any_order() {
    let refused = ProviderVerdict::CredentialRefused(axum::http::StatusCode::UNAUTHORIZED);
    for verdicts in [
        [refused, ProviderVerdict::OutOfMinutes],
        [ProviderVerdict::OutOfMinutes, refused],
    ] {
        assert_eq!(ProviderVerdict::worst(verdicts), refused, "{verdicts:?}");
        assert_eq!(
            ProviderVerdict::worst(verdicts).code(),
            "livekit_provider_credential_refused"
        );
    }
    for verdicts in [
        [ProviderVerdict::OutOfMinutes, ProviderVerdict::Available],
        [ProviderVerdict::Available, ProviderVerdict::OutOfMinutes],
    ] {
        assert_eq!(
            ProviderVerdict::worst(verdicts),
            ProviderVerdict::OutOfMinutes,
            "{verdicts:?}"
        );
    }
    // No verdict at all is the pool the rotation found nothing in.
    assert_eq!(
        ProviderVerdict::worst([]),
        ProviderVerdict::OutOfMinutes,
        "an empty pool reports the quota code it always did"
    );
}
