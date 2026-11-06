//! Serves the real web service and the local interviewer in this process, built
//! by the same startup phases and the same `launch::serve` that `codetrial web`
//! runs, with the arming observer installed before the first request, so a
//! browser run can leave behind the tool-call evidence of the interview it
//! drove.
//!
//! `serve_web_with_arming` is the ignored entry a launcher runs; the ordinary
//! tests beside it drive `serve` with a throwaway config so the fixture itself
//! is checked without credentials or a browser.
//!
//! Unix only (`probe.rs` gates the module): a TERM is a stop request, which
//! needs Unix signals, and the launcher keeps it in a session of its own.
//!
//! Where it differs from `codetrial web`, on purpose: it is always a launcher's
//! child (no Setup page, as with the desktop app's server), it does not clear
//! the operator's web error log, task mode is off, and it reads the process
//! environment exactly as the product does, so an ambient `NODE_ENV` or
//! `CODETRIAL_*` setting reaches it too. It waits up to its settle bound for
//! admitted rooms and reports to finish before the snapshot, and a room still
//! running at the bound fails the run.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;

use super::acquisition::{Arming, Limits, install_arming};
use crate::launch::{
    Bounds, CliOptions, LaunchedBy, finish, load_values, local_agent_config, prepare,
    serve as serve_web,
};
use crate::livekit::latency::ReplyLatency;

/// A launcher gives a config file and a directory the fixture owns; the
/// account database, the ready file, the stop file and the report all live in
/// that directory under fixed names. The account database therefore has no
/// default to fall back on: the default is the operator's own `codetrial.db`.
struct Settings {
    config: PathBuf,
    dir: PathBuf,
    addr: String,
    deadline: Duration,
    /// How long open connections may hold the shutdown up before the run
    /// reports without them.
    drain: Duration,
    /// How long admitted rooms and their reports may take to finish after the
    /// last connection has gone, before the run reports them as unfinished.
    settle: Duration,
    /// Skips the startup probe of the provider's quota. Only the
    /// credential-free tests set it; the entry a launcher runs keeps the
    /// product's behavior.
    offline: bool,
}

impl Settings {
    fn from_lookup(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let required = |name: &str| {
            get(name)
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| format!("{name} must be set"))
        };
        let seconds = |name: &str, default: u64| {
            get(name)
                .map(|text| text.parse().map_err(|_| format!("{name} is not seconds")))
                .transpose()
                .map(|value| Duration::from_secs(value.unwrap_or(default)))
        };
        Ok(Self {
            config: required("CODETRIAL_FIXTURE_CONFIG")?,
            dir: required("CODETRIAL_FIXTURE_DIR")?,
            addr: get("CODETRIAL_FIXTURE_ADDR").unwrap_or_else(|| "127.0.0.1:0".into()),
            deadline: seconds("CODETRIAL_FIXTURE_DEADLINE_S", 7200)?,

            // From the launcher, which sizes its stop wait from this and the
            // settle; the default is what it passes.
            drain: seconds("CODETRIAL_FIXTURE_DRAIN_S", 10)?,
            settle: seconds("CODETRIAL_FIXTURE_SETTLE_S", 120)?,
            offline: false,
        })
    }

    fn db(&self) -> PathBuf {
        self.dir.join("accounts.db")
    }

    fn ready(&self) -> PathBuf {
        self.dir.join("ready")
    }

    fn stop(&self) -> PathBuf {
        self.dir.join("stop")
    }

    fn out(&self) -> PathBuf {
        self.dir.join("out.json")
    }
}

/// Serves until `stop` resolves, then writes what the observer saw and how the
/// server ended to `out.json`. The ready file holds the bound address; it is
/// written once the listener is bound, so a request that arrives before the
/// accept loop has run waits in the socket's backlog rather than being refused.
fn serve(
    settings: &Settings,
    stop: impl Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    clear_previous(settings)?;

    // Loopback only: it runs with a throwaway session secret and an account
    // database nobody else should reach.
    let addr: std::net::SocketAddr = settings
        .addr
        .parse()
        .map_err(|error| format!("{} is not an address: {error}", settings.addr))?;

    // Canonical first, as `launch::reachable_from_elsewhere` does: a dual-stack
    // socket names 127.0.0.1 as ::ffff:127.0.0.1.
    if !addr.ip().to_canonical().is_loopback() {
        return Err(format!("{addr} is not a loopback address"));
    }
    let options = CliOptions {
        config_path: Some(settings.config.display().to_string()),
        ..Default::default()
    };
    let mut values = load_values(&options)?;
    values.insert(
        "CODETRIAL_DB_PATH".into(),
        settings.db().display().to_string(),
    );

    let prepared = prepare(&values, &options)?;
    let listener = std::net::TcpListener::bind(addr)
        .map_err(|error| format!("cannot bind {addr}: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let bound = listener.local_addr().map_err(|error| error.to_string())?;
    let (mut config, setup) = finish(
        prepared.config,
        &values,
        &options,
        bound,
        LaunchedBy::Parent,
    )?;
    config.probe_provider_quota &= !settings.offline;
    let agent = local_agent_config(values)?
        .ok_or("the fixture hosts the interviewer and needs a Gemini key")?;

    let arming = Arming::new(Limits {
        batches: 512,
        calls: 2048,
        bytes: 4 << 20,
    });
    let installed = install_arming(&arming)?;
    publish(&settings.ready(), &bound.to_string())?;
    let outcome = serve_web(
        listener,
        config,
        setup,
        Some(agent),
        prepared.max_concurrent,
        stop,
        Bounds {
            drain: Some(settings.drain),
            settle: Some(settings.settle),
        },
    );

    let mut report = arming.report();
    report["bound"] = json!(bound.to_string());
    report["outcome"] = json!(outcome.clone().err().unwrap_or_else(|| "ok".into()));
    drop(installed);
    ended(outcome, publish(&settings.out(), &format!("{report:#}\n")))
}

/// How a run ended, once its report was or was not written: when both failed,
/// both are said, the serving failure first.
fn ended(outcome: Result<(), String>, written: Result<(), String>) -> Result<(), String> {
    match (outcome, written) {
        (Err(served), Err(unwritten)) => Err(format!("{served}; and {unwritten}")),
        (outcome, written) => outcome.and(written),
    }
}

/// Written beside and renamed into place, so a reader never sees half a file.
fn publish(path: &Path, text: &str) -> Result<(), String> {
    let partial = path.with_extension("partial");
    std::fs::write(&partial, text)
        .and_then(|()| std::fs::rename(&partial, path))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Set once a TERM can be received, so a test sends one only then.
static TERM_REGISTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Resolves once the stop file exists or the deadline passes.
async fn stop_file(path: PathBuf, deadline: Duration) {
    let began = Instant::now();
    while !path.exists() && began.elapsed() < deadline {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// The stop file, or a TERM, which a launcher that cannot reach the file sends
/// to ask the same thing: drain, settle and write the report. Registered when
/// first polled, inside the fixture's runtime, which is after the ready file is
/// written: a TERM in that first moment still finds the default action and ends
/// the fixture without a report, so a launcher asks with the stop file and
/// keeps the fixture out of group-wide signals. Fails loudly if it cannot be
/// registered, since the fixture would otherwise carry on without its contract.
async fn stop_requested(path: PathBuf, deadline: Duration) {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("the TERM handler registers");
    TERM_REGISTERED.store(true, std::sync::atomic::Ordering::SeqCst);

    // Once this returns the handler stays installed and a later TERM does
    // nothing, so a launcher's escalation after the stop file is the KILL.
    tokio::select! {
        () = stop_file(path, deadline) => {}
        _ = term.recv() => {}
    }
}

/// Before anything can fail: a run that never reports must not leave an
/// earlier run's report to be read as its own.
fn clear_previous(settings: &Settings) -> Result<(), String> {
    for stale in [settings.ready(), settings.out()] {
        match std::fs::remove_file(&stale) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("cannot clear {}: {error}", stale.display()));
            }
            _ => {}
        }
    }
    Ok(())
}

fn serve_until_stop_file(settings: &Settings) -> Result<(), String> {
    clear_previous(settings)?;
    if settings.stop().exists() {
        return Err(format!(
            "{} already exists and would end the run before it began",
            settings.stop().display()
        ));
    }
    serve(settings, stop_requested(settings.stop(), settings.deadline))
}

#[test]
#[ignore = "serves until stopped; an interview run through it spends provider quota"]
fn serve_web_with_arming() {
    // The observer's hooks are process-wide, so another test's room or bind in
    // this process would fail it for good. The launcher runs this one alone.
    assert!(
        std::env::args().any(|arg| arg == "--exact"),
        "run this with --exact so no other test shares the process"
    );
    let settings = Settings::from_lookup(&|name| std::env::var(name).ok()).unwrap();
    serve_until_stop_file(&settings).unwrap();
}

fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "codetrial-serving-{label}-{}-{}",
        std::process::id(),
        crate::current_epoch_millis()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Settings for a throwaway directory: a fake config, loopback port zero, and
/// no quota probe.
fn throwaway(dir: &Path) -> Settings {
    let config = dir.join("codetrial.env.local");
    std::fs::write(
        &config,
        "LIVEKIT_URL=wss://primary.example\nLIVEKIT_API_KEY=primary-key\n\
         LIVEKIT_API_SECRET=primary-secret\nGOOGLE_API_KEY=google-key\n",
    )
    .unwrap();
    Settings {
        config,
        dir: dir.to_path_buf(),
        addr: "127.0.0.1:0".into(),
        deadline: Duration::from_secs(60),
        drain: Duration::from_secs(10),
        settle: Duration::from_secs(10),
        offline: true,
    }
}

fn get(addr: &str, path: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn wait_for(path: &Path) -> String {
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(30) {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.is_empty()
        {
            return text;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{} never appeared", path.display());
}

fn lookup(
    config: Option<&str>,
    dir: Option<&str>,
    deadline: Option<&str>,
) -> impl Fn(&str) -> Option<String> {
    let (config, dir, deadline) = (
        config.map(String::from),
        dir.map(String::from),
        deadline.map(String::from),
    );
    move |name| match name {
        "CODETRIAL_FIXTURE_CONFIG" => config.clone(),
        "CODETRIAL_FIXTURE_DIR" => dir.clone(),
        "CODETRIAL_FIXTURE_DEADLINE_S" => deadline.clone(),
        _ => None,
    }
}

#[test]
fn the_fixture_owns_one_directory_and_never_defaults_the_database() {
    let settings = Settings::from_lookup(&lookup(Some("/c"), Some("/d"), None)).unwrap();
    assert_eq!(settings.db(), Path::new("/d/accounts.db"));
    for (name, path) in [
        ("ready", settings.ready()),
        ("stop", settings.stop()),
        ("out.json", settings.out()),
    ] {
        assert_eq!(path, Path::new("/d").join(name));
    }

    // A blank directory counts as missing: a blank database path is read as
    // absent by the launch code and would open the operator's own database.
    for blank in [None, Some(""), Some("   "), Some("\t")] {
        for (config, dir, missing) in [
            (blank, Some("/d"), "CODETRIAL_FIXTURE_CONFIG"),
            (Some("/c"), blank, "CODETRIAL_FIXTURE_DIR"),
        ] {
            assert_eq!(
                Settings::from_lookup(&lookup(config, dir, None))
                    .err()
                    .unwrap(),
                format!("{missing} must be set")
            );
        }
    }
    assert!(Settings::from_lookup(&lookup(Some("/c"), Some("/d"), Some("soon"))).is_err());
    assert_eq!(settings.settle, Duration::from_secs(120));
    assert_eq!(settings.drain, Duration::from_secs(10));
    for (key, value, read) in [
        ("CODETRIAL_FIXTURE_SETTLE_S", "later", None),
        ("CODETRIAL_FIXTURE_DRAIN_S", "soon", None),
        ("CODETRIAL_FIXTURE_DRAIN_S", "3", Some(3)),
    ] {
        let lookup = |name: &str| match name {
            "CODETRIAL_FIXTURE_CONFIG" => Some("/c".to_string()),
            "CODETRIAL_FIXTURE_DIR" => Some("/d".to_string()),
            name if name == key => Some(value.to_string()),
            _ => None,
        };
        let result = Settings::from_lookup(&lookup);
        match read {
            None => assert_eq!(result.err().unwrap(), format!("{key} is not seconds")),
            Some(seconds) => assert_eq!(result.unwrap().drain, Duration::from_secs(seconds)),
        }
    }
}

#[test]
fn a_stop_file_left_from_an_earlier_run_refuses_the_start() {
    let dir = scratch("stale-stop");
    let settings = throwaway(&dir);
    std::fs::write(settings.stop(), "").unwrap();
    // An earlier run's report goes too, so it cannot be kept as this one's.
    std::fs::write(settings.out(), "{\"outcome\":\"ok\"}").unwrap();
    std::fs::write(settings.ready(), "127.0.0.1:1").unwrap();
    let error = serve_until_stop_file(&settings).unwrap_err();
    assert!(
        error.contains("would end the run before it began"),
        "{error}"
    );
    assert!(!settings.db().exists() && !settings.out().exists());
    assert!(!settings.ready().exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_run_that_fails_and_cannot_report_says_both() {
    let fail = |why: &str| Err::<(), String>(why.into());
    assert_eq!(ended(Ok(()), Ok(())), Ok(()));
    assert_eq!(ended(fail("served"), Ok(())), fail("served"));
    assert_eq!(ended(Ok(()), fail("unwritten")), fail("unwritten"));
    assert_eq!(
        ended(fail("served"), fail("unwritten")),
        fail("served; and unwritten")
    );
}

#[test]
fn an_address_off_loopback_refuses_the_start() {
    let dir = scratch("public-addr");
    for (addr, refusal) in [
        ("0.0.0.0:0", "not a loopback address"),
        ("192.0.2.1:0", "not a loopback address"),
        ("localhost:0", "not an address"),
    ] {
        let mut settings = throwaway(&dir);
        settings.addr = addr.into();
        let error = serve_until_stop_file(&settings).unwrap_err();
        assert!(error.contains(refusal), "{addr}: {error}");
        assert!(!settings.db().exists() && !settings.out().exists());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_fixture_serves_the_real_service_and_reports_what_the_observer_saw() {
    if crate::test_isolation::isolated!("CODETRIAL_SERVING_FIXTURE_TEST") {
        return;
    }
    let dir = scratch("serve");
    let settings = throwaway(&dir);
    let (ready, out, db) = (settings.ready(), settings.out(), settings.db());
    let stop = std::sync::Arc::new(tokio::sync::Notify::new());
    let stopping = std::sync::Arc::clone(&stop);
    let server =
        std::thread::spawn(move || serve(&settings, async move { stopping.notified().await }));

    let addr = wait_for(&ready);
    assert!(get(&addr, "/healthz").starts_with("HTTP/1.1 200"), "{addr}");
    assert!(
        get(&addr, "/setup").starts_with("HTTP/1.1 404"),
        "a child has no Setup"
    );
    assert!(db.is_file(), "the named database is the one opened");

    // The product's own boundaries: the dispatcher's assignment, the latency
    // bind, the executed tool answers and the local send outcome.
    crate::dispatch::notify_assigned("fixture-room");
    let mut latency = ReplyLatency::default();
    latency.bind("fixture-room", 7, 1, 1500);
    super::acquisition::observe(&latency, &fixture_answers("call-1"))
        .expect("the armed scope records the batch")
        .complete(true);
    stop.notify_one();
    server.join().unwrap().unwrap();

    let report: serde_json::Value = serde_json::from_str(&wait_for(&out)).unwrap();
    assert_eq!(report["outcome"], "ok", "{report}");
    assert_eq!(report["bound"], addr, "{report}");
    // Present and null, not merely absent: a missing key must not pass.
    assert_eq!(report.get("arming_failure"), Some(&serde_json::Value::Null));
    let capture = &report["capture"];
    assert_eq!(
        capture.get("failure"),
        Some(&serde_json::Value::Null),
        "{report}"
    );
    assert!(capture["retained_bytes"].as_u64().unwrap() > 0, "{report}");
    assert!(
        !report.to_string().contains("not retained"),
        "arguments are not kept"
    );
    assert_eq!(capture["retained_calls"], 1, "{report}");
    assert_eq!(capture["batches"].as_array().unwrap().len(), 1, "{report}");
    let batch = &capture["batches"][0];
    assert_eq!(batch["identity"]["room"], "fixture-room", "{report}");
    assert_eq!(batch["identity"]["session"], 7, "{report}");
    assert_eq!(batch["calls"][0]["id"], "call-1", "{report}");
    assert_eq!(batch["calls"][0]["name"], "read_editor", "{report}");
    assert_eq!(batch["calls"][0]["result"], "authored result", "{report}");
    assert_eq!(batch["local_send_completed"], true, "{report}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_term_is_a_stop_request_and_the_run_still_reports() {
    if crate::test_isolation::isolated!("CODETRIAL_SERVING_FIXTURE_TEST") {
        return;
    }
    let dir = scratch("term");
    let settings = throwaway(&dir);
    let (ready, out) = (settings.ready(), settings.out());
    let server = std::thread::spawn(move || serve_until_stop_file(&settings));
    wait_for(&ready);

    // The handler is registered when the stop future is first polled, which the
    // bound address does not wait for.
    let began = Instant::now();
    while !TERM_REGISTERED.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(began.elapsed() < Duration::from_secs(30), "no TERM handler");
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = std::process::Command::new("kill")
        .args(["-TERM", &std::process::id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    server.join().unwrap().unwrap();
    let report: serde_json::Value = serde_json::from_str(&wait_for(&out)).unwrap();
    assert_eq!(report["outcome"], "ok", "{report}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_client_holding_a_half_sent_request_cannot_keep_the_run_from_reporting() {
    if crate::test_isolation::isolated!("CODETRIAL_SERVING_FIXTURE_DRAIN_TEST") {
        return;
    }
    let dir = scratch("drain");
    let mut settings = throwaway(&dir);
    settings.drain = Duration::from_secs(1);
    let (ready, out) = (settings.ready(), settings.out());
    let stop = std::sync::Arc::new(tokio::sync::Notify::new());
    let stopping = std::sync::Arc::clone(&stop);
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = serve(&settings, async move { stopping.notified().await });
        let _ = done.send(result);
    });

    let addr = wait_for(&ready);
    // A login whose body never finishes arriving, held open past the stop.
    use std::io::Write;
    let mut held = std::net::TcpStream::connect(&addr).unwrap();
    write!(
        held,
        "POST /api/login HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Content-Length: 1000\r\n\r\n{{\"partial\":"
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    stop.notify_one();
    let result = finished
        .recv_timeout(Duration::from_secs(20))
        .expect("the run reported within its drain bound");
    let error = result.unwrap_err();
    assert!(error.contains("shutdown bound"), "{error}");

    let report: serde_json::Value = serde_json::from_str(&wait_for(&out)).unwrap();
    assert!(
        report["outcome"]
            .as_str()
            .unwrap()
            .contains("shutdown bound"),
        "{report}"
    );
    drop(held);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_run_replaces_the_artifacts_an_earlier_run_left() {
    if crate::test_isolation::isolated!("CODETRIAL_SERVING_FIXTURE_STALE_TEST") {
        return;
    }
    let dir = scratch("stale-artifacts");
    let settings = throwaway(&dir);
    std::fs::write(settings.out(), "{\"outcome\":\"ok\",\"stale\":true}").unwrap();
    std::fs::write(settings.ready(), "127.0.0.1:1").unwrap();
    // A run that fails to start leaves neither file behind.
    let mut broken = throwaway(&dir);
    broken.config = dir.join("missing.env.local");
    assert!(serve(&broken, async {}).is_err());
    assert!(!settings.out().exists() && !settings.ready().exists());
    std::fs::write(settings.out(), "{\"outcome\":\"ok\",\"stale\":true}").unwrap();
    std::fs::write(settings.ready(), "127.0.0.1:1").unwrap();

    // A stop that has already happened: the run starts, serves nothing,
    // reports.
    serve(&settings, async {}).unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(settings.out()).unwrap()).unwrap();
    assert!(report.get("stale").is_none(), "{report}");
    assert_eq!(report["outcome"], "ok", "{report}");
    let ready = std::fs::read_to_string(settings.ready()).unwrap();
    assert_ne!(ready, "127.0.0.1:1");
    assert_eq!(report["bound"], ready.as_str(), "{report}");
    assert!(
        !settings.dir.join("out.partial").exists(),
        "published by rename"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn fixture_answers(id: &str) -> Vec<(crate::gemini::GeminiFunctionCall, serde_json::Value)> {
    vec![(
        crate::gemini::GeminiFunctionCall {
            id: id.into(),
            name: "read_editor".into(),
            args: json!({"private": "not retained"}),
        },
        json!("authored result"),
    )]
}
