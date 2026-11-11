// Real SDK delivery with a hand-built driver clock and local model endpoints.

#[derive(Clone, Copy, Debug, PartialEq)]
enum G2Case {
    EndThenDeadline,
    DeadlineFirst,
    RoomClosed,
    AllReady,
}

impl G2Case {
    fn selected() -> Self {
        match std::env::var("CODETRIAL_G2_CASE").as_deref() {
            Ok("end") => Self::EndThenDeadline,
            Ok("deadline") => Self::DeadlineFirst,
            Ok("closed") => Self::RoomClosed,
            Ok("all") => Self::AllReady,
            _ => panic!("CODETRIAL_G2_CASE is end, deadline, closed or all"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::EndThenDeadline => "g2-end-then-deadline",
            Self::DeadlineFirst => "g2-deadline-first",
            Self::RoomClosed => "g2-room-closed",
            Self::AllReady => "g2-all-ready",
        }
    }

    fn actions(self) -> Vec<&'static str> {
        let mut actions = vec!["candidate_join", "agent_join"];
        if matches!(self, Self::RoomClosed | Self::AllReady) {
            actions.push("room_delete");
        }
        actions.push("controlled_driver_start");
        if !matches!(self, Self::RoomClosed | Self::AllReady) {
            actions.push("room_delete");
        }
        actions
    }

    fn initial_count(self) -> usize {
        usize::from(self != Self::RoomClosed)
    }

    fn terminal(self) -> &'static str {
        match self {
            Self::EndThenDeadline => "End",
            Self::DeadlineFirst | Self::AllReady => "Deadline",
            Self::RoomClosed => "RoomClosed",
        }
    }
}

struct G2TerminalRegistration(String);

impl Drop for G2TerminalRegistration {
    fn drop(&mut self) {
        super::super::super::LIVE_TERMINAL_PROBES
            .lock()
            .unwrap()
            .retain(|(name, _)| name != &self.0);
    }
}

fn g2_native_config() -> crate::config::AgentConfig {
    let mut pairs = crate::config::read_config_file(Path::new(
        &std::env::var("CODETRIAL_LIVE_CONFIG").expect("isolated native config path"),
    ))
    .expect("read isolated native config");
    pairs.retain(|(name, _)| name != "GOOGLE_API_KEY" && name != "GOOGLE_API_KEYS");
    pairs.push(("GOOGLE_API_KEY".into(), "g2-fixture-only-key".into()));
    let config = crate::config::load_from_pairs(pairs).expect("valid isolated native config");
    register_secrets([
        config.livekit_api_key.clone(),
        config.livekit_api_secret.clone(),
    ]);
    config
}

async fn g2_delete(config: &crate::config::AgentConfig, room: &str) -> Result<(), String> {
    tokio::time::timeout(
        Duration::from_secs(10),
        super::super::super::room_service::delete_room(
            &config.livekit_url,
            &config.livekit_api_key,
            &config.livekit_api_secret,
            room,
            crate::current_epoch_seconds(),
        ),
    )
    .await
    .map_err(|_| "native room delete timed out".to_owned())?
    .map_err(|error| redact(&error.to_string()))
}

async fn g2_queue_until(
    source: &mut tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
    target: &tokio::sync::mpsc::UnboundedSender<RoomEvent>,
    closed: bool,
) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let event = source.recv().await.ok_or_else(|| "native agent event stream ended before required input".to_owned())?;
            let matched = if closed {
                matches!(&event, RoomEvent::Disconnected { .. })
            } else {
                matches!(&event, RoomEvent::DataReceived { topic, payload, .. }
                    if topic.as_deref() == Some(crate::runtime::TOPIC_CONTROL)
                        && serde_json::from_slice::<Value>(payload).is_ok_and(|body| body["type"] == "end_interview"))
            };
            target.send(event).map_err(|_| "driver event receiver ended".to_owned())?;
            if matched { return Ok(()); }
        }
    }).await.map_err(|_| "native input arrival was not observed in 20 seconds".to_owned())?
}

fn g2_seed_report_state(state: &mut crate::agent::RuntimeState) {
    state.code = SEED_SOLUTION.into();
    state.language = "python".into();
    crate::livekit::tests::receive_test_run(state);
    for phase in ["test", "optimizations"] {
        crate::agent::record_framework_evidence(
            state,
            &json!({
                "phase": phase, "source": crate::livekit::tests::observed_source(phase),
                "kind": "observed", "confidence": 90, "summary": "Synthetic G2 fixture evidence."
            }),
        )
        .unwrap();
    }
    state.behavioral_round_started = true;
    state.transcript.push("Candidate: I keep each previously visited value in a map so that I can find its matching complement without scanning the array again.".into());
    let rounds = crate::agent::ReportRounds::of(state);
    assert!(
        rounds.behavioral_opened && rounds.follow_ups_released && rounds.communication_assessed()
    );
}

impl super::Rig {
    async fn g2_repeat_deadline(&mut self) -> Result<std::ops::ControlFlow<()>, String> {
        let boot = crate::runtime::bootstrap(&self.config, self.room.name(), Some("two-sum"), 45);
        let interview = super::super::super::InterviewContext {
            config: &self.config,
            keys: &self.keys,
            boot: &boot,
            live_endpoint: Some(&self.endpoint),
            started_at: std::time::Instant::now(),
            candidate_identity: "candidate",
            events: &self.events,
            slot: None,
        };
        let mut context = self.turn.context(
            &mut self.output,
            &mut self.gemini,
            &mut self.board,
            &mut self.media,
        );
        super::super::super::on_hard_deadline(&self.room, &mut context, &mut self.loops, interview)
            .await
            .map_err(|error| redact(&error.to_string()))
    }
}

type G2DriverOutcome = (super::Rig, Vec<String>, bool, bool, bool);

struct G2NativeRun {
    config: crate::config::AgentConfig,
    room_name: String,
    agent_identity: String,
    rig: Option<super::Rig>,
    candidate: Option<CandidateLink>,
    bridge: Option<tokio::task::JoinHandle<()>>,
    driver: Option<tokio::task::JoinHandle<G2DriverOutcome>>,
    shared: Arc<Shared>,
    fixture: super::G2ReportFixture,
    terminals: tokio::sync::mpsc::UnboundedReceiver<(&'static str, std::time::Instant)>,
    _terminal_scope: G2TerminalRegistration,
    attempted_join: bool,
    attempted_delete: bool,
    joined_driver: bool,
    first_break: bool,
    guard_reentered: bool,
    guard_ok: bool,
    outcomes: Vec<String>,
}

impl G2NativeRun {
    async fn new(config: crate::config::AgentConfig, room_name: String) -> Self {
        let agent_identity = crate::runtime::agent_identity(&room_name);
        let mut rig = super::Rig::new("ws://127.0.0.1:9/".into()).await;
        rig.config = config.clone();
        rig.loops.recovery_terminal = None;
        rig.loops.recovery_inputs = None;
        g2_seed_report_state(&mut rig.turn.state);
        let fixture = super::G2ReportFixture::start(&room_name).await;
        let (terminal, terminals) = tokio::sync::mpsc::unbounded_channel();
        super::super::super::LIVE_TERMINAL_PROBES
            .lock()
            .unwrap()
            .push((room_name.clone(), terminal));
        Self {
            config,
            room_name: room_name.clone(),
            agent_identity,
            rig: Some(rig),
            candidate: None,
            bridge: None,
            driver: None,
            shared: Shared::new(),
            fixture,
            terminals,
            _terminal_scope: G2TerminalRegistration(room_name),
            attempted_join: false,
            attempted_delete: false,
            joined_driver: false,
            first_break: false,
            guard_reentered: false,
            guard_ok: false,
            outcomes: Vec::new(),
        }
    }

    async fn delete(&mut self, permits: &mut PermitClient) -> Result<(), String> {
        self.attempted_delete = true;
        permits
            .guarded("room_delete", g2_delete(&self.config, &self.room_name))
            .await
            .map_err(|error| error.to_string())?
    }

    async fn exercise(
        &mut self,
        permits: &mut PermitClient,
        case: G2Case,
        mutant: bool,
    ) -> Result<(), String> {
        if permits.stop_requested() {
            return Err("launcher stopped before joins".into());
        }
        self.attempted_join = true;
        let token = crate::web::token_response(
            &crate::web::TokenConfig {
                api_key: &self.config.livekit_api_key,
                api_secret: &self.config.livekit_api_secret,
                server_url: &self.config.livekit_url,
                recording_max_min: None,
            },
            br#"{"problemId":"two-sum","durationMin":10}"#,
            &self.room_name,
            "candidate",
            crate::current_epoch_seconds(),
        )
        .map_err(|error| format!("candidate token: {error}"))?
        .token;
        if permits.stop_requested() {
            return Err("launcher stopped before candidate join".into());
        }
        self.candidate = Some(
            permits
                .guarded(
                    "candidate_join",
                    connect_candidate(
                        &self.config.livekit_url,
                        &token,
                        &self.shared,
                        &self.agent_identity,
                    ),
                )
                .await
                .map_err(|error| error.to_string())??,
        );
        if permits.stop_requested() {
            return Err("launcher stopped before agent join".into());
        }
        let (agent, mut native_events) = permits
            .guarded("agent_join", async {
                tokio::time::timeout(
                    Duration::from_secs(30),
                    super::super::super::join_room(
                        &self.config,
                        &self.room_name,
                        &self.agent_identity,
                        crate::current_epoch_seconds(),
                    ),
                )
                .await
                .map_err(|_| "agent connect timed out".to_owned())?
                .map_err(|error| redact(&error.to_string()))
            })
            .await
            .map_err(|error| error.to_string())??;
        self.rig.as_mut().unwrap().room = super::super::super::room_link::RoomLink::live(agent);
        let (forward, received) = tokio::sync::mpsc::unbounded_channel();
        self.rig.as_mut().unwrap().events = tokio::sync::Mutex::new(received);
        if case != G2Case::RoomClosed {
            if permits.stop_requested() {
                return Err("launcher stopped before End".into());
            }
            permits
                .guarded(
                    FINAL_ACTION_ID,
                    self.candidate
                        .as_ref()
                        .unwrap()
                        .room
                        .local_participant()
                        .publish_data(DataPacket {
                            payload: serde_json::to_vec(
                                &json!({"type":"end_interview", "reason":"candidate_ended"}),
                            )
                            .unwrap(),
                            topic: Some(crate::runtime::TOPIC_CONTROL.into()),
                            reliable: true,
                            ..Default::default()
                        }),
                )
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| redact(&error.to_string()))?;
            g2_queue_until(&mut native_events, &forward, false).await?;
        }
        if matches!(case, G2Case::RoomClosed | G2Case::AllReady) {
            if permits.stop_requested() {
                return Err("launcher stopped before delete".into());
            }
            self.delete(permits).await?;
            g2_queue_until(&mut native_events, &forward, true).await?;
        }
        self.bridge = Some(tokio::spawn(async move {
            while let Some(event) = native_events.recv().await {
                if forward.send(event).is_err() {
                    break;
                }
            }
        }));
        if permits.stop_requested() {
            return Err("launcher stopped before driver".into());
        }
        permits
            .permit("controlled_driver_start")
            .await
            .map_err(|error| error.to_string())?;
        let mut rig = self.rig.take().unwrap();
        rig.loops.deadline_at = if matches!(case, G2Case::DeadlineFirst | G2Case::AllReady) {
            tokio::time::Instant::now()
        } else {
            tokio::time::Instant::now() + Duration::from_secs(540)
        };
        self.driver = Some(tokio::spawn(async move {
            let first = rig
                .run_result()
                .await
                .map_err(|error| redact(&error.to_string()));
            let first_break = matches!(&first, Ok(std::ops::ControlFlow::Break(())));
            let mut outcomes = vec![format!("{first:?}")];
            let mut guard_reentered = false;
            let mut guard_ok = false;
            if case == G2Case::EndThenDeadline && first_break {
                let repeated = rig.g2_repeat_deadline().await;
                guard_reentered = true;
                guard_ok = matches!(&repeated, Ok(std::ops::ControlFlow::Break(())));
                outcomes.push(format!("{repeated:?}"));
            }
            (rig, outcomes, first_break, guard_reentered, guard_ok)
        }));
        let stop_file = permits.stop_file.clone();
        let joined = tokio::select! {
            biased;
            () = async {
                loop {
                    if stop_file.as_deref().is_some_and(Path::exists) { break; }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            } => return Err("launcher stopped during driver; cleanup follows".into()),
            result = tokio::time::timeout(
                Duration::from_secs(if mutant { 1020 } else { 540 }),
                self.driver.as_mut().unwrap(),
            ) => result.map_err(|_| "driver join timed out".to_owned())?
                .map_err(|error| format!("driver join failed: {error}"))?,
        };
        self.driver.take();
        self.rig = Some(joined.0);
        self.outcomes = joined.1;
        self.first_break = joined.2;
        self.guard_reentered = joined.3;
        self.guard_ok = joined.4;
        self.joined_driver = true;
        Ok(())
    }

    async fn cleanup(&mut self, permits: &mut PermitClient) -> Vec<String> {
        let mut failures = Vec::new();
        if let Some(driver) = self.driver.take() {
            driver.abort();
            let _ = tokio::time::timeout(Duration::from_secs(2), driver).await;
            failures.push("driver aborted: cardinality acceptance is inconclusive".into());
        }
        if let Some(rig) = &mut self.rig {
            if !g2_agent_closed(
                tokio::time::timeout(Duration::from_secs(10), rig.room.close()).await,
            ) {
                failures.push("agent Room close did not complete".into());
            }
            if !matches!(
                tokio::time::timeout(Duration::from_secs(1), rig.gemini.shutdown()).await,
                Ok(Ok(()))
            ) {
                failures.push("local Gemini close did not complete".into());
            }
            if let Some(mut old) = rig.old.take()
                && !matches!(
                    tokio::time::timeout(Duration::from_secs(2), &mut old).await,
                    Ok(Ok(_))
                )
            {
                old.abort();
                let _ = old.await;
                failures.push("local Gemini peer did not join".into());
            }
        }
        if self.attempted_join
            && !self.attempted_delete
            && let Err(error) = self.delete(permits).await
        {
            failures.push(format!("cleanup room delete: {error}"));
        }
        if let Some(candidate) = self.candidate.take() {
            if !matches!(
                tokio::time::timeout(Duration::from_secs(10), candidate.room.close()).await,
                Ok(Ok(())) | Ok(Err(::livekit::RoomError::AlreadyClosed))
            ) {
                failures.push("candidate Room close did not complete".into());
            }
            let mut observer = candidate.observer;
            if !matches!(
                tokio::time::timeout(Duration::from_secs(10), &mut observer).await,
                Ok(Ok(()))
            ) {
                observer.abort();
                let _ = observer.await;
                failures.push("candidate observer did not join".into());
            }
        }
        if let Some(mut bridge) = self.bridge.take()
            && !matches!(
                tokio::time::timeout(Duration::from_secs(10), &mut bridge).await,
                Ok(Ok(()))
            )
        {
            bridge.abort();
            let _ = bridge.await;
            failures.push("SDK event bridge did not join".into());
        }
        failures
    }
}

/// NOT RUN: local-ready scheduling with real SDK packets/receipts, not the
/// production room-loop clock or proof of every native arrival ordering.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "NOT RUN: real LiveKit Room; requires a prospective G2 manifest, launcher permits and isolated native config; Gemini endpoints stay local"]
async fn g2_native_report_generation_and_guarded_deadline_reentry() {
    let case = G2Case::selected();
    let mutant = match std::env::var("CODETRIAL_G2_GUARD_MUTANT").as_deref() {
        Err(_) => false,
        Ok("1") => {
            assert_eq!(case, G2Case::EndThenDeadline);
            true
        }
        _ => panic!("CODETRIAL_G2_GUARD_MUTANT is absent or 1"),
    };
    let cap = if mutant { 1200 } else { 720 };
    let mut permits = PermitClient::from_env()
        .await
        .expect("prospective G2 launcher budget");
    let run_id = format!(
        "{}{}",
        case.name(),
        if mutant { "-guard-mutant" } else { "" }
    );
    assert_eq!(permits.manifest["run_id"], run_id);
    assert_eq!(permits.manifest["actions"], json!(case.actions()));
    assert_eq!(permits.manifest["action_ceiling"], case.actions().len() + 1);
    assert_eq!(permits.manifest["outer_wall_cap_s"], cap);
    assert_eq!(
        permits.manifest["observed_budgets"]["external_gemini_http"],
        0
    );
    assert_eq!(
        permits.manifest["observed_budgets"]["gemini_live_connections"],
        0
    );
    assert_eq!(
        permits.manifest["observed_budgets"]["native_room_connections"],
        2
    );
    assert_eq!(
        permits.manifest["observed_budgets"]["local_report_http_attempts"],
        if mutant { 10 } else { 5 }
    );
    let settings = g2_settings(&permits.manifest);
    assert_eq!(settings["case"], case.name());
    assert_eq!(settings["guard_mutant"], mutant);
    let room_name = settings["room_name"].as_str().unwrap().to_owned();
    assert_eq!(
        permits.manifest["named_resources"],
        json!([{"kind":"room", "name":room_name}])
    );
    let config = g2_native_config();
    g2_require_deny_proxy(&config, &settings);
    let mut run = G2NativeRun::new(config, room_name).await;
    let result = run.exercise(&mut permits, case, mutant).await;
    let cleanup = run.cleanup(&mut permits).await;
    assert!(
        result.is_ok() && cleanup.is_empty() && run.joined_driver,
        "INCONCLUSIVE: native driver/setup/cleanup incomplete: {result:?} {cleanup:?}"
    );
    if case == G2Case::EndThenDeadline {
        assert!(
            run.first_break && run.guard_reentered && run.outcomes.len() == 2,
            "INCONCLUSIVE: real End must finish and the guarded deadline handler must run"
        );
        if !mutant {
            assert!(
                run.guard_ok,
                "INCONCLUSIVE: baseline deadline re-entry must return Ok(Break), not an unrelated error"
            );
        }
    } else if case != G2Case::AllReady {
        assert!(
            run.first_break,
            "INCONCLUSIVE: terminal handler did not leave normally"
        );
    }
    let mut expected_permits = case.actions();
    if case != G2Case::RoomClosed {
        expected_permits.push(FINAL_ACTION_ID);
    }
    let mut granted = permits
        .granted
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    granted.sort();
    expected_permits.sort();
    assert_eq!(
        granted, expected_permits,
        "one reserved End and only declared nonfinal actions"
    );
    let entries = run.fixture.scope.entries();
    let inputs = run.fixture.bodies.lock().unwrap().len();
    let terminal = run
        .terminals
        .try_recv()
        .expect("real driver terminal observed")
        .0;
    let observed = run.shared.observed.lock().unwrap();
    emit(
        "g2_counts",
        json!({"case":case.name(),"bounded_entries":entries.len(),"local_http_attempts":inputs,"final_copies":observed.tally.final_copies,"final_distinct":observed.tally.final_ids.len(),"terminal":terminal,"outcomes":run.outcomes,"driver_joined":run.joined_driver,"first_break":run.first_break,"guard_reentered":run.guard_reentered,"guard_ok":run.guard_ok}),
    );
    if mutant {
        assert_eq!(
            entries.len(),
            2,
            "mutant precondition: exactly two actual bounded entries"
        );
        assert_eq!(
            inputs, 2,
            "mutant precondition: exactly two actual local report HTTP requests"
        );
        eprintln!("G2_GUARD_MUTANT_REACHED_TWO_REAL_GENERATIONS");
    }
    assert_eq!(
        entries.len(),
        case.initial_count(),
        "initial bounded generation cardinality; accepted regeneration was never requested"
    );
    assert_eq!(
        inputs,
        case.initial_count(),
        "valid local report fixture uses one HTTP attempt per generation"
    );
    assert_eq!(terminal, case.terminal());
    assert!(
        run.terminals.try_recv().is_err(),
        "one real recovery terminal"
    );
    assert_eq!(observed.tally.provisional, 0);
    if matches!(case, G2Case::EndThenDeadline | G2Case::DeadlineFirst) {
        assert_eq!(observed.tally.final_ids.len(), 1);
        assert!(
            observed.tally.final_copies >= 1,
            "actual SDK final packet received"
        );
        assert_eq!(observed.ack_failures, 0);
    } else {
        assert_eq!(
            observed.tally.final_copies, 0,
            "closed native room delivers no final packet"
        );
    }
}

#[test]
fn g2_native_rows_declare_finite_zero_gemini_budgets() {
    for case in [
        G2Case::EndThenDeadline,
        G2Case::DeadlineFirst,
        G2Case::RoomClosed,
        G2Case::AllReady,
    ] {
        assert_eq!(case.actions().len() + 1, 5);
        assert!(!case.actions().contains(&"retry_report"));
        assert!(!case.actions().contains(&FINAL_ACTION_ID));
    }
    assert_eq!(4 * 720 + 1200, 68 * 60);
}

fn g2_agent_closed(
    result: Result<
        Result<(), Box<dyn std::error::Error + Send + Sync>>,
        tokio::time::error::Elapsed,
    >,
) -> bool {
    match result {
        Ok(Ok(())) => true,
        Ok(Err(error)) => matches!(
            error.downcast_ref::<::livekit::RoomError>(),
            Some(::livekit::RoomError::AlreadyClosed)
        ),
        Err(_) => false,
    }
}

fn g2_require_deny_proxy(config: &crate::config::AgentConfig, settings: &Value) {
    let proxy = std::env::var("CODETRIAL_G2_DENY_PROXY")
        .expect("run through the G2 non-forwarding proxy wrapper");
    let parsed = reqwest::Url::parse(&proxy).expect("local denial proxy URL");
    assert_eq!(parsed.scheme(), "http");
    assert!(
        parsed
            .host_str()
            .and_then(|host| host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .ok())
            .is_some_and(|address| address.is_loopback())
    );
    assert!(parsed.username().is_empty() && parsed.password().is_none());
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        assert_eq!(
            std::env::var(key).as_deref(),
            Ok(proxy.as_str()),
            "mandatory outside-HTTP denial"
        );
    }
    let bypass = std::env::var("NO_PROXY").expect("explicit native host bypasses");
    assert_eq!(std::env::var("no_proxy").as_deref(), Ok(bypass.as_str()));
    let host = reqwest::Url::parse(&config.livekit_url)
        .unwrap()
        .host_str()
        .unwrap()
        .to_owned();
    let mut native = Vec::new();
    for entry in bypass.split(',') {
        assert!(
            !entry.is_empty()
                && !entry.contains('*')
                && !entry.to_ascii_lowercase().contains("googleapis.com")
        );
        if entry != "127.0.0.1" && entry != "::1" {
            native.push(entry.to_owned());
        }
    }
    assert!(
        bypass.split(',').any(|entry| entry == host),
        "the configured native host is explicitly allowed"
    );
    let configured = reqwest::Url::parse(&config.livekit_url).unwrap();
    let configured_port = configured.port_or_known_default().unwrap();
    let endpoints = settings["native_endpoints"].as_array().unwrap();
    assert!(
        endpoints
            .iter()
            .any(|endpoint| endpoint["host"] == host && endpoint["port"] == configured_port),
        "the configured SDK host:port must be prospectively allowed for CONNECT"
    );
    let mut endpoint_hosts = endpoints
        .iter()
        .map(|endpoint| {
            let endpoint_host = endpoint["host"].as_str().unwrap();
            let port = endpoint["port"].as_u64().unwrap();
            assert!(
                (1..=65535).contains(&port)
                    && !endpoint_host.contains('*')
                    && !endpoint_host
                        .to_ascii_lowercase()
                        .contains("googleapis.com")
            );
            endpoint_host.to_owned()
        })
        .collect::<Vec<_>>();
    endpoint_hosts.sort();
    endpoint_hosts.dedup();
    let actual_allow: Vec<Value> = serde_json::from_str(
        &std::env::var("CODETRIAL_G2_PROXY_ALLOW")
            .expect("proxy exports its actual exact CONNECT allowlist"),
    )
    .unwrap();
    let as_pairs = |values: &[Value]| {
        let mut pairs = values
            .iter()
            .map(|endpoint| {
                (
                    endpoint["host"].as_str().unwrap().to_owned(),
                    endpoint["port"].as_u64().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        pairs.sort();
        pairs
    };
    assert_eq!(
        as_pairs(&actual_allow),
        as_pairs(endpoints),
        "actual proxy CONNECT policy equals prospectively hashed settings"
    );
    let mut declared = settings["native_hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    native.sort();
    declared.sort();
    assert_eq!(
        endpoint_hosts, declared,
        "HTTP bypasses are only the declared SDK endpoint hosts"
    );
    declared.retain(|entry| entry != "127.0.0.1" && entry != "::1");
    assert_eq!(
        native, declared,
        "only prospectively named native hosts bypass the restricted proxy"
    );
}

fn g2_settings(manifest: &Value) -> Value {
    let bytes = std::fs::read(
        std::env::var("CODETRIAL_G2_SETTINGS").expect("prospective G2 settings path"),
    )
    .unwrap();
    assert_eq!(
        crate::sha256_hex(&[&bytes]),
        manifest["settings_sha256"].as_str().unwrap()
    );
    let settings: Value = serde_json::from_slice(&bytes).unwrap();
    let room = settings["room_name"].as_str().unwrap();
    assert!(
        !room.is_empty()
            && room.len() <= 63
            && room
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    );
    assert!(
        room.starts_with(manifest["run_id"].as_str().unwrap()),
        "cleanup owns only the prospectively named run room"
    );
    settings
}

/// Controller-owned idempotent cleanup of the one prospectively named Room.
#[tokio::test]
#[ignore = "NOT RUN: controller-owned G2 named-resource cleanup; requires the frozen manifest/settings and native config"]
async fn g2_cleanup_named_native_room() {
    let bytes =
        std::fs::read(std::env::var("CODETRIAL_G2_MANIFEST").expect("prospective manifest path"))
            .unwrap();
    let manifest: Value = serde_json::from_slice(&bytes).unwrap();
    let settings = g2_settings(&manifest);
    let requested =
        std::env::var("CODETRIAL_G2_CLEAN_ROOM").expect("controller supplied exact named room");
    assert_eq!(settings["room_name"], requested);
    assert_eq!(
        manifest["named_resources"],
        json!([{"kind":"room", "name":requested}])
    );
    let config = g2_native_config();
    g2_require_deny_proxy(&config, &settings);
    g2_delete(&config, &requested)
        .await
        .expect("bounded owned-room cleanup");
}

/// Offline config metadata only: writes host/port without calling a provider.
#[test]
#[ignore = "offline config metadata operation; requires selected config and new private output; no provider or Room call"]
fn g2_write_native_endpoint_settings() {
    let out = std::env::var("CODETRIAL_G2_ENDPOINTS_OUT")
        .expect("new private endpoint metadata output path");
    let config = g2_native_config();
    let url = reqwest::Url::parse(&config.livekit_url).expect("selected native URL parses");
    assert!(matches!(url.scheme(), "ws" | "wss"));
    let host = url.host_str().expect("selected native URL has a host");
    let port = url
        .port_or_known_default()
        .expect("selected native URL has a port");
    let bytes = serde_json::to_vec_pretty(&json!({
        "native_hosts":[host], "native_endpoints":[{"host":host,"port":port}]
    }))
    .unwrap();
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .expect("endpoint metadata output must be new");
    std::io::Write::write_all(&mut output, &bytes).expect("write endpoint metadata only");
}
