//! Real HTTP admission and local dispatch against a held loopback handshake.
//! Slots and signaling attempts are observable; connected LiveKit agents are
//! zero because the fixture never completes the WebSocket upgrade.

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{Router, extract::State, http::StatusCode, routing::get};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

const CHILD: &str = "CODETRIAL_DISPATCH_HTTP_CHILD";
const DIRECTORY: &str = "CODETRIAL_DISPATCH_HTTP_DIRECTORY";

struct Files(PathBuf);

struct HeldClock(tokio::task::JoinHandle<()>);

impl Drop for HeldClock {
    fn drop(&mut self) {
        self.0.abort();
        tokio::time::resume();
    }
}

fn held_clock() -> HeldClock {
    // The SDK's five-second handshake deadline must not become a CI scheduling
    // deadline. Keep virtual time frozen while real loopback I/O progresses. A
    // runnable cooperative task prevents Tokio's automatic idle-time jump; the
    // parent still enforces its independent wall-clock child deadline.
    tokio::time::pause();
    HeldClock(tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    }))
}

struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// The vendored WebSocket transport ignores NO_PROXY. Scrub proxies only in a
// child, and discard its raw SDK output so authentication headers and URLs
// cannot end up in a test log. Only the counts file crosses this boundary.
async fn isolated(name: &str) -> bool {
    let exact = format!("dispatch::tests::http_probe::{name}");
    if std::env::var(CHILD).as_deref() == Ok(exact.as_str()) {
        let path = PathBuf::from(std::env::var_os(DIRECTORY).unwrap()).join("panic.json");
        std::panic::set_hook(Box::new(move |panic| {
            // Preserve a useful failure anchor, never the panic payload: SDK
            // errors and assertion values can contain authentication material.
            if let Some(location) = panic.location() {
                let _ = std::fs::write(
                    &path,
                    json!({"file": location.file(), "line": location.line()}).to_string(),
                );
            }
        }));
        return false;
    }
    let files = Files(std::env::temp_dir().join(format!(
        "codetrial-dispatch-http-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    )));
    let mut directory = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory.create(&files.0).unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", &exact, "--test-threads=1"])
        .env(CHILD, &exact)
        .env(DIRECTORY, &files.0)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for key in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        command.env_remove(key);
    }
    let mut child = Child(command.spawn().unwrap());
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            child.0.kill().unwrap();
            child.0.wait().unwrap();
            panic!("loopback dispatch probe exceeded its child deadline");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let failure = std::fs::read_to_string(files.0.join("panic.json")).unwrap_or_default();
    assert!(
        status.success(),
        "loopback dispatch probe failed: {name} ({status}) {failure}"
    );
    let result = files.0.join("counts.json");
    assert!(std::fs::metadata(&result).unwrap().len() < 8192);
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
    assert!(
        !rows.is_empty(),
        "the child must record actual measurements"
    );
    for row in rows {
        eprintln!("loopback dispatch probe {name} {row}");
    }
    true
}

#[derive(Clone)]
struct Gate {
    attempts: Arc<AtomicUsize>,
    validations: Arc<AtomicUsize>,
    overflow: Arc<AtomicUsize>,
    pending: mpsc::Sender<oneshot::Sender<()>>,
}

async fn handshake(State(gate): State<Gate>) -> StatusCode {
    gate.attempts.fetch_add(1, Ordering::SeqCst);
    let (release, held) = oneshot::channel();
    if gate.pending.try_send(release).is_err() {
        gate.overflow.fetch_add(1, Ordering::SeqCst);
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    let _ = held.await;
    StatusCode::UNAUTHORIZED
}

async fn validate(State(gate): State<Gate>) -> StatusCode {
    gate.validations.fetch_add(1, Ordering::SeqCst);
    StatusCode::UNAUTHORIZED
}

struct Probe {
    base: String,
    client: reqwest::Client,
    dispatcher: Arc<LocalDispatcher>,
    gate: Gate,
    pending: mpsc::Receiver<oneshot::Sender<()>>,
    held: Vec<oneshot::Sender<()>>,
    servers: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Probe {
    fn drop(&mut self) {
        // Panic cleanup is process-bounded too: the parent kills/reaps a child
        // that fails to finish. No detached agent can survive the child exit.
        self.held.clear();
        for server in &self.servers {
            server.abort();
        }
    }
}

impl Probe {
    async fn start(pinned: bool, max_concurrent: usize) -> Self {
        let signaling = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", signaling.local_addr().unwrap());
        let (pending, receiver) = mpsc::channel(8);
        let gate = Gate {
            attempts: Arc::new(AtomicUsize::new(0)),
            validations: Arc::new(AtomicUsize::new(0)),
            overflow: Arc::new(AtomicUsize::new(0)),
            pending,
        };
        let app = Router::new()
            .route("/rtc", get(handshake))
            .route("/rtc/v1", get(handshake))
            .route("/rtc/validate", get(validate))
            .route("/rtc/v1/validate", get(validate))
            .with_state(gate.clone());
        let signal_server = tokio::spawn(async move {
            axum::serve(signaling, app).await.unwrap();
        });
        let provider = Provider {
            id: crate::config::PRIMARY_PROVIDER_ID.to_string(),
            url: url.clone(),
            api_key: "probe-key".to_string(),
            api_secret: "probe-secret".to_string(),
            google_api_keys: vec!["probe-google".to_string()],
        };
        let dispatcher = Arc::new(LocalDispatcher {
            config: crate::config::load_from_pairs([
                ("LIVEKIT_URL", url.as_str()),
                ("LIVEKIT_API_KEY", "probe-key"),
                ("LIVEKIT_API_SECRET", "probe-secret"),
                ("GOOGLE_API_KEY", "probe-google"),
            ])
            .unwrap(),
            runtime: tokio::runtime::Handle::current(),
            state: Arc::new(Mutex::new(DispatchState::default())),
            max_concurrent,
        });
        let database = PathBuf::from(std::env::var_os(DIRECTORY).unwrap()).join(format!(
            "accounts-{}.db",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        crate::accounts::initialize_account_database(&database).unwrap();
        let config = crate::web::WebServerConfig {
            web_dir: PathBuf::from(std::env::var_os(DIRECTORY).unwrap()),
            github_client_id: None,
            github_client_secret: None,
            session_secret: Some("probe-session-secret".to_string()),
            db_path: Some(database),
            github_oauth_base_url: None,
            github_api_base_url: None,
            room_prefix: "interview".to_string(),
            fixed_room_name: pinned.then(|| "interview-local".to_string()),
            production: !pinned,
            compiler_explorer_enabled: false,
            guest_mode_enabled: true,
            trusted_proxy_hops: 1,
            allowed_hosts: Vec::new(),
            recording: None,
            pool: crate::config::ProviderPool {
                providers: vec![provider],
            },
            probe_provider_quota: false,
            tasks: None,
            task_mode_requested: false,
            update_check_commit: None,
            provision_rooms: false,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let service = crate::web::web_service_with_dispatcher(
            config,
            Some(std::sync::Arc::<LocalDispatcher>::clone(&dispatcher)),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, service).await.unwrap();
        });
        Self {
            base,
            client: crate::bundled_roots_client_builder()
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
            dispatcher,
            gate,
            pending: receiver,
            held: Vec::new(),
            servers: vec![server, signal_server],
        }
    }

    async fn token(&self, client_ip: &str, cookie: Option<&str>, selection: &str) -> (u16, Value) {
        let mut body = json!({"selectionMode": selection});
        if selection == "practice" {
            body["problemId"] = json!("chargeback-pair-match");
        }
        let mut request = self
            .client
            .post(format!("{}/api/token", self.base))
            .header("X-Forwarded-For", client_ip)
            .json(&body);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        assert_ne!(status, 429, "rate limiting must not contaminate the probe");
        let mut body: Value = response.json().await.unwrap();
        body.as_object_mut().unwrap().remove("token");
        (status, body)
    }

    async fn login(&self, name: &str) -> String {
        let response = self
            .client
            .post(format!("{}/api/login", self.base))
            .json(&json!({"login": name}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    async fn attempt(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let release = loop {
            match self.pending.try_recv() {
                Ok(release) => break release,
                Err(mpsc::error::TryRecvError::Disconnected) => panic!("signaling gate closed"),
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
            assert!(
                std::time::Instant::now() < deadline,
                "spawned interviewer must reach loopback signaling"
            );
            tokio::task::yield_now().await;
        };
        self.held.push(release);
    }

    fn counts(&self, live: usize, guests: usize, attempts: usize) -> Value {
        let state = self.dispatcher.state.lock().unwrap();
        assert_eq!(
            state.live.len(),
            live,
            "held handshake expired or admission count changed"
        );
        assert_eq!(state.guest_live.len(), guests);
        assert_eq!(self.gate.attempts.load(Ordering::SeqCst), attempts);
        assert_eq!(self.gate.overflow.load(Ordering::SeqCst), 0);
        json!({"reservations": live, "guest_reservations": guests,
            "signaling_attempts": attempts, "validate_probes": self.gate.validations.load(Ordering::SeqCst)})
    }

    async fn wait_slots(&self, live: usize, guests: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let matches = {
                let state = self.dispatcher.state.lock().unwrap();
                state.live.len() == live && state.guest_live.len() == guests
            };
            if matches {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "controlled startup refusal must release capacity"
            );
            tokio::task::yield_now().await;
        }
    }

    async fn close(&mut self) {
        for release in self.held.drain(..) {
            release
                .send(())
                .expect("handshake must still be held before controlled refusal");
        }
        self.wait_slots(0, 0).await;
        for server in &self.servers {
            server.abort();
        }
    }
}

fn record(rows: Vec<Value>) {
    let path = PathBuf::from(std::env::var_os(DIRECTORY).unwrap()).join("counts.json");
    std::fs::write(path, serde_json::to_vec(&rows).unwrap()).unwrap();
}

#[tokio::test]
async fn guest_pinned_reload_and_cross_client_reuse_are_refused() {
    if isolated("guest_pinned_reload_and_cross_client_reuse_are_refused").await {
        return;
    }
    let _clock = held_clock();
    let mut probe = Probe::start(true, 8).await;
    assert_eq!(probe.token("192.0.2.1", None, "practice").await.0, 200);
    probe.attempt().await;
    let same = probe.token("192.0.2.1", None, "practice").await;
    let other = probe.token("192.0.2.2", None, "practice").await;
    assert_eq!(same.0, 503);
    assert_eq!(same, other);
    let owner = probe.login("fixture-one").await;
    assert_eq!(
        probe.token("192.0.2.3", Some(&owner), "practice").await.0,
        503
    );
    assert_eq!(probe.token("192.0.2.2", None, "strict").await.0, 401);
    let mut row = probe.counts(1, 1, 1);
    row["case"] = json!(
        "guest pinned: Start admitted; reload/cross-client/owner reuse refused; Strict refused"
    );
    probe.close().await;
    record(vec![row, probe.counts(0, 0, 1)]);
}

#[tokio::test]
async fn guest_unpinned_capacity_returns_after_startup_failure() {
    if isolated("guest_unpinned_capacity_returns_after_startup_failure").await {
        return;
    }
    let _clock = held_clock();
    let mut probe = Probe::start(false, 3).await;
    let first = probe.token("192.0.2.1", None, "practice").await;
    assert_eq!(first.0, 200);
    probe.attempt().await;
    let second = probe.token("192.0.2.1", None, "practice").await;
    assert_eq!(second.0, 200);
    assert_ne!(first.1["roomName"], second.1["roomName"]);
    probe.attempt().await;
    assert_eq!(probe.token("192.0.2.1", None, "practice").await.0, 503);
    assert_eq!(probe.token("192.0.2.2", None, "practice").await.0, 503);
    let mut rows = vec![probe.counts(2, 2, 2)];
    let owner = probe.login("fixture-owner").await;
    assert_eq!(
        probe.token("192.0.2.3", Some(&owner), "practice").await.0,
        200
    );
    probe.attempt().await;
    rows.push(probe.counts(3, 2, 3));
    probe.held.remove(0).send(()).unwrap();
    probe.wait_slots(2, 1).await;
    assert_eq!(probe.token("192.0.2.1", None, "practice").await.0, 200);
    probe.attempt().await;
    rows.push(probe.counts(3, 2, 4));
    assert_eq!(
        probe.token("192.0.2.3", Some(&owner), "practice").await.0,
        503
    );
    probe.close().await;
    rows.push(probe.counts(0, 0, 4));
    record(rows);
}

#[tokio::test]
async fn signed_in_pinned_reuse_and_strict_fresh_rooms_are_distinct() {
    if isolated("signed_in_pinned_reuse_and_strict_fresh_rooms_are_distinct").await {
        return;
    }
    let _clock = held_clock();
    let mut rows = Vec::new();
    for pinned in [false, true] {
        for selection in ["practice", "strict"] {
            let mut probe = Probe::start(pinned, 8).await;
            let first_owner = probe.login("fixture-first").await;
            let other_owner = probe.login("fixture-second").await;
            let first = probe
                .token("192.0.2.1", Some(&first_owner), selection)
                .await;
            assert_eq!(first.0, 200);
            probe.attempt().await;
            let reload = probe
                .token("192.0.2.1", Some(&first_owner), selection)
                .await;
            let cross_account = probe
                .token("192.0.2.2", Some(&other_owner), selection)
                .await;
            assert_eq!(reload.0, 200);
            assert_eq!(cross_account.0, 200);
            let reuse = pinned && selection == "practice";
            if reuse {
                assert_eq!(first.1["roomName"], reload.1["roomName"]);
                assert_eq!(first.1["roomName"], cross_account.1["roomName"]);
                assert_eq!(probe.token("192.0.2.3", None, "practice").await.0, 503);
            } else {
                assert_ne!(first.1["roomName"], reload.1["roomName"]);
                assert_ne!(reload.1["roomName"], cross_account.1["roomName"]);
                probe.attempt().await;
                probe.attempt().await;
            }
            let expected = if reuse { 1 } else { 3 };
            let mut row = probe.counts(expected, 0, expected);
            row["pinned"] = json!(pinned);
            row["selection"] = json!(selection);
            row["cross_account_reuse"] = json!(reuse);
            rows.push(row);
            probe.close().await;
        }
    }
    record(rows);
}
