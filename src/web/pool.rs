//! Provider pooling: which LiveKit project an interview runs on, and whether
//! that project can still take a connection.
//!
//! `config` decides what the pool *is* (discovery, ordering, resolving a room
//! name back to its project). This module decides what the pool *does* at
//! runtime: it keeps each project's quota verdict fresh in the background, and
//! picks a project that can actually answer when a candidate asks for a token.
//!
//! The probing is automatic on purpose. Learning that a project is spent from a
//! candidate's failed interview is learning it from the worst possible place:
//! LiveKit refuses the socket with 429, which reaches the browser as "could not
//! establish signal connection" and says nothing about quota. A pool that
//! probes at startup and on a timer knows before anyone asks.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use futures_util::future::join_all;

use crate::config::{Provider, ProviderPool};
use crate::token::{LivekitTokenInput, livekit_token};

use super::token::suffix;
use super::{AppState, WebServerConfig};

/// How long a project's quota verdict is trusted.
///
/// LiveKit Cloud bills connection minutes against a period, so a project that
/// has run out stays out for the rest of it, and one with room to spare does
/// not fill up inside a minute. Long enough that a burst of interview starts
/// costs one probe, short enough that topping a project up brings it back
/// without a restart.
const PROVIDER_QUOTA_TTL: Duration = Duration::from_secs(60);

/// How long the probe itself may take, which is not the shared client's
/// 30-second backstop.
///
/// This runs while a candidate waits on `/api/token`, and its answer is only an
/// optimization: a project that does not reply promptly is treated as available
/// and tried, which is what would have happened without any probe at all.
/// Waiting half a minute to learn something optional is the worse trade.
const PROVIDER_QUOTA_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Whether a verdict of this age may still be answered from cache.
///
/// A function taking a `Duration` rather than the comparison inline, so the
/// boundary can be stated. Inline it needed an `Instant` exactly
/// `PROVIDER_QUOTA_TTL` old, which a monotonic clock cannot be asked for: time
/// passes between constructing one and reading it, so `<` and `<=` behaved
/// identically for every input a test could build and the difference between
/// them was unfalsifiable. A `Duration` is exact.
fn is_fresh(age: Duration) -> bool {
    age < PROVIDER_QUOTA_TTL
}

/// How often the background refresher re-probes every project.
///
/// Derived from the TTL rather than written beside it, because the relation is
/// the design: a verdict has to be replaced before it can expire or
/// `not_known_exhausted` starts paying for an HTTP probe on the request path
/// again, which is the cost the refresher exists to remove. Two independent
/// constants held in step by a comment would let someone halve the TTL for a
/// faster recovery and silently put that probe back, with no test failing.
const PROVIDER_QUOTA_REFRESH: Duration = Duration::from_secs(PROVIDER_QUOTA_TTL.as_secs() / 2);

/// What each project last said about its own quota, so a run of interview
/// starts does not re-ask a project that just answered.
///
/// Keyed by provider id rather than URL, because the id is what the room name
/// carries and therefore what the agent resolves the same project from.
#[derive(Clone, Default)]
pub(crate) struct ProviderQuota(Arc<Mutex<HashMap<String, (bool, Instant)>>>);

impl ProviderQuota {
    /// Whether nothing has said this project is out of connection minutes.
    ///
    /// Not "is available": the probe answers false only for an explicit 429,
    /// and a malformed origin, a signing failure or a timeout all come back
    /// true. That is deliberate, and the name has to carry it, or a caller
    /// reads a positive quota assertion into what is really the absence of a
    /// refusal.
    ///
    /// From cache where the last answer is recent enough, from the project
    /// itself otherwise.
    pub(crate) async fn not_known_exhausted(&self, provider: &Provider) -> bool {
        let now = Instant::now();
        {
            let cache = self.0.lock().unwrap_or_else(|error| error.into_inner());
            if let Some((verdict, at)) = cache.get(&provider.id)
                && is_fresh(now.duration_since(*at))
            {
                return *verdict;
            }
        }
        let verdict = probe_not_exhausted(provider).await;
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(provider.id.clone(), (verdict, Instant::now()));
        verdict
    }

    /// Probes every project at once and replaces the cached verdicts.
    ///
    /// Concurrent rather than sequential: the probes are independent, and a
    /// nine-project pool run in series would take nine timeouts to finish where
    /// one is enough. Returns the ids that came back exhausted, in pool order,
    /// for the caller to report.
    pub(crate) async fn refresh_all(&self, pool: &ProviderPool) -> Vec<String> {
        let verdicts = join_all(pool.providers.iter().map(|provider| async move {
            (provider.id.clone(), probe_not_exhausted(provider).await)
        }))
        .await;

        let now = Instant::now();
        let mut cache = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let mut exhausted = Vec::new();
        for (id, verdict) in verdicts {
            if !verdict {
                exhausted.push(id.clone());
            }
            cache.insert(id, (verdict, now));
        }
        exhausted
    }
}

/// Whether a project still has connection minutes.
///
/// `/rtc/validate` answers this without opening a socket, creating a room or
/// spending a minute of the quota it is asking about: 200 where the project has
/// room, 429 where it does not. The room management API is not a substitute --
/// it answers 200 on an exhausted project, because the limit is on media
/// connections rather than on the Twirp surface.
///
/// Only an explicit 429 takes a project out. A probe that cannot be sent at all
/// says nothing about quota, and refusing every interview because this server
/// briefly lost the network would be a worse failure than the one being
/// avoided: the candidate would be turned away from a project that works.
async fn probe_not_exhausted(provider: &Provider) -> bool {
    let Some(origin) = super::policy::livekit_http_origin(&provider.url) else {
        return true;
    };
    let Ok(token) = livekit_token(LivekitTokenInput {
        api_key: &provider.api_key,
        api_secret: &provider.api_secret,
        name: "quota-probe",
        identity: "quota-probe",
        room: "quota-probe",
        metadata: "",
        now_seconds: crate::current_epoch_seconds(),
        agent: false,
    }) else {
        return true;
    };
    let response = crate::http_client()
        .get(format!("{origin}/rtc/validate"))
        .bearer_auth(token)
        .timeout(PROVIDER_QUOTA_PROBE_TIMEOUT)
        .send()
        .await;
    !matches!(response, Ok(response) if response.status() == StatusCode::TOO_MANY_REQUESTS)
}

/// Which project this interview runs on, skipping any that has run out.
pub(crate) enum ProviderChoice<'a> {
    Ready(String, &'a crate::config::Provider),
    NoneConfigured,
    AllExhausted,
}

/// Round-robin as before, but a project that answers "out of minutes" is passed
/// over rather than handed to the candidate.
///
/// The counter advances per attempt, not per request, so a dead project does
/// not pin every later interview to the one after it.
pub(crate) async fn room_and_available_provider(state: &AppState) -> ProviderChoice<'_> {
    // A pinned development room names the project it belongs to, so it is
    // preferred while that project can still take a connection, and only while.
    // A pin whose project is out of minutes is a room nobody can join: the
    // token mints, the browser opens the socket, and LiveKit answers 429, which
    // reaches the candidate as "could not establish signal connection" with
    // nothing on this side saying why.
    //
    // Resolved straight from the pool rather than through `room_and_provider`:
    // the pin is the room name, so nothing here rotates and no counter is spent
    // on it. Spending one anyway would advance the rotation on every pinned
    // request, which is the opposite of what a pin is for.
    if !state.config.production
        && let Some(room_name) = state
            .config
            .fixed_room_name
            .as_deref()
            .filter(|name| !name.is_empty())
    {
        let Some(provider) = state
            .config
            .pool
            .for_room(room_name, &state.config.room_prefix)
        else {
            return ProviderChoice::NoneConfigured;
        };
        if state.provider_quota.not_known_exhausted(provider).await {
            return ProviderChoice::Ready(room_name.to_string(), provider);
        }
        eprintln!(
            "livekit provider {} owns pinned room {room_name} but is out of connection minutes; falling back to the pool",
            provider.id
        );
    }

    if state.config.pool.providers.is_empty() {
        return ProviderChoice::NoneConfigured;
    }
    for _ in 0..state.config.pool.providers.len() {
        // Bumped per attempt rather than once per request: adding the attempt
        // number to one shared value locally would leave the next request
        // starting at the exhausted project again, so the project immediately
        // after a dead one absorbs its share as well as its own.
        let counter = state.provider_counter.fetch_add(1, Ordering::Relaxed);

        // A fresh rotation name, never the pinned one: falling back off an
        // exhausted pin has to drop the pinned name along with the project. The
        // room name is the only thing a separately launched `codetrial
        // run-livekit ROOM` resolves the project from, so keeping
        // `interview-local` while minting credentials for another project is
        // exactly how the candidate and the agent end up in different ones.
        let (room_name, provider) = room_and_provider(&state.config, counter);
        let Some(provider) = provider else {
            return ProviderChoice::NoneConfigured;
        };
        if state.provider_quota.not_known_exhausted(provider).await {
            return ProviderChoice::Ready(room_name, provider);
        }

        // Deliberately silent. This fires exactly when the quota already knows
        // the project is spent, so it reported a fact the refresher's own
        // `livekit quota:` line already carries, once per interview, for as
        // long as a project stays exhausted. A rotation that finds a provider
        // is not news; running out of all of them is, and that is answered
        // below.
    }
    ProviderChoice::AllExhausted
}

/// Picks the provider first and writes its id into the room name, because the
/// agent process is launched separately and the room name is the only thing it
/// is handed. Recomputing the choice on the other side, from a pool assembled
/// by a second directory scan, is how a candidate ends up holding a token for
/// one LiveKit project while the agent waits in another.
///
/// The primary contributes no segment, so a single-provider deployment keeps
/// the `<prefix>-<suffix>` room names it already has.
///
/// No pinned-room shortcut lives here, because the fallback off an exhausted
/// pin needs a room name that carries its provider id, which is the one thing
/// the pinned name does not have.
pub(crate) fn room_and_provider(
    config: &WebServerConfig,
    counter: usize,
) -> (String, Option<&crate::config::Provider>) {
    let provider = config.pool.select(counter);

    // The primary contributes no segment, so a single-provider deployment keeps
    // the room names it already has. Comparing exactly is sound only because
    // `is_provider_id` reserves the word case-insensitively upstream, so no
    // provider spelled `Primary` can reach this line. The asymmetry is
    // load-bearing: do not "fix" one of the two comparisons on its own.
    let segment = provider
        .map(|provider| provider.id.as_str())
        .filter(|id| *id != crate::config::PRIMARY_PROVIDER_ID)
        .map_or(String::new(), |id| format!("-{id}"));
    (
        format!("{}{segment}-{}", config.room_prefix, suffix(8)),
        provider,
    )
}

/// What the startup probe says it found.
///
/// A string rather than an `eprintln!` in three arms, because the arms are the
/// part worth checking and a branch that only prints is a branch no test can
/// reach. The caller prints it.
///
/// Projects are named individually rather than counted. "8 of 9 available"
/// tells an operator to go looking; naming the one that is spent tells them
/// which account to top up, and whether it is the one their pinned room
/// depends on.
fn pool_health_line(exhausted: &[String], total: usize) -> String {
    match exhausted.len() {
        0 => format!("livekit quota: all {total} project(s) can take connections"),
        spent if spent == total => format!(
            "livekit quota: every project is out of connection minutes ({}); interviews will be refused until one is topped up",
            exhausted.join(", ")
        ),
        _ => format!(
            "livekit quota: {} of {total} project(s) available; out of minutes: {}",
            total - exhausted.len(),
            exhausted.join(", ")
        ),
    }
}

/// The line for a change in verdicts, or `None` when nothing moved.
///
/// Only the change is worth printing. Repeating the same verdict every thirty
/// seconds is how a log stops being read, and the startup line already said
/// what the steady state is.
fn quota_change_line(previous: &[String], current: &[String]) -> Option<String> {
    if previous == current {
        return None;
    }
    Some(match current {
        [] => "livekit quota: every project can take connections again".to_string(),
        spent => format!("livekit quota: now out of minutes: {}", spent.join(", ")),
    })
}

/// Probes the pool once and says what it found.
///
/// The refresher's first action rather than a step before the listener binds:
/// an operator learns the same thing either way, and blocking startup on one
/// probe per project would make a slow network delay the server rather than
/// just the answer.
async fn report_pool_health(quota: &ProviderQuota, pool: &ProviderPool) -> Vec<String> {
    let exhausted = quota.refresh_all(pool).await;
    eprintln!("{}", pool_health_line(&exhausted, pool.providers.len()));
    exhausted
}

/// Stops the refresher when the server it belongs to goes away.
///
/// A bare `handle.spawn` outlives its caller: the loop would go on probing on a
/// thirty-second beat after the server that wanted it was dropped, and nothing
/// would hold a handle to stop it. Aborting on drop ties the loop's life to the
/// server's, which is what every caller already assumes.
#[derive(Default)]
pub(crate) struct QuotaRefresher(Option<tokio::task::JoinHandle<()>>);

impl Drop for QuotaRefresher {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

/// Keeps every project's verdict fresh for as long as the server runs.
///
/// Silent when there is no runtime to spawn on, matching the recording workers:
/// `web_router` is public and can be built outside one, and a server whose
/// quota cache is merely cold still works. A cold cache costs an inline probe
/// on the first request per project, which is what happened before this
/// existed.
pub(crate) fn spawn_provider_quota_refresher(
    quota: ProviderQuota,
    pool: ProviderPool,
) -> QuotaRefresher {
    if pool.providers.is_empty() {
        return QuotaRefresher::default();
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        eprintln!("livekit quota: no runtime to refresh provider quotas on");
        return QuotaRefresher::default();
    };
    QuotaRefresher(Some(handle.spawn(async move {
        // The first pass runs immediately and says what it found, so the
        // startup log carries the pool's actual state. Held by the task rather
        // than by `ProviderQuota`, because it is the reporting state of this
        // one loop and not part of what a project last said.
        let mut previous = report_pool_health(&quota, &pool).await;
        loop {
            tokio::time::sleep(PROVIDER_QUOTA_REFRESH).await;

            let exhausted = quota.refresh_all(&pool).await;
            if let Some(line) = quota_change_line(&previous, &exhausted) {
                eprintln!("{line}");
                previous = exhausted;
            }
        }
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PRIMARY_PROVIDER_ID, Provider, provider_id_from_room};

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
                        api_key: "k".to_string(),
                        api_secret: "s".to_string(),
                        google_api_key: String::new(),
                    })
                    .collect(),
            },
        }
    }

    /// A stub that answers every probe with one status, standing in for a
    /// LiveKit project with or without minutes left.
    async fn quota_stub(status: axum::http::StatusCode) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = axum::Router::new().route(
            "/rtc/validate",
            axum::routing::get(move || async move { status }),
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

        // And the verdicts landed in the cache the request path reads, so a
        // token request pays no probe of its own.
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

        // Not merely shorter: short enough that a pass can be missed and the
        // next one still lands inside the TTL.
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

        // All of them, which is the one an operator has to act on immediately.
        // The count and the names both matter, so a guard that fired on the
        // wrong comparison would be reporting the wrong emergency.
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

        // The abort lands at the task's next scheduling point, not inside
        // `drop`, so this yields rather than asserting straight away.
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
                // whatever the suffix looks like, and `for_room` resolves that
                // miss back to the primary. Asserting the round trip here would
                // pin the wrong rule.
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
}
