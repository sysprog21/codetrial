// Part of recovery_live.rs: included in place, so every item here shares that
// file's module.

/// The worker half of the two launcher tests. It does nothing unless the
/// launcher test started it with a scratch directory, so the ordinary lane
/// passes it vacuously and it is not a test of anything on its own.
#[tokio::test]
async fn permit_worker_child() {
    let Ok(dir) = std::env::var("CODETRIAL_PERMIT_WORKER_DIR") else {
        return;
    };
    let mode = std::env::var("CODETRIAL_PERMIT_WORKER_MODE").unwrap();
    let dir = PathBuf::from(dir);
    let note = |name: &str, text: &str| {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(name))
            .unwrap();
        writeln!(file, "{text}").unwrap();
    };
    let mut client = PermitClient::from_env().await.unwrap();
    let actions: Vec<String> = client.manifest["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|action| action.as_str().unwrap().to_owned())
        .collect();
    let mut script: Vec<String> = actions.clone();
    if mode != "duplicate" {
        script.push(FINAL_ACTION_ID.into());
    }
    for action in script {
        match client
            .guarded(&action, async { note("dispatch.log", &action) })
            .await
        {
            Ok(()) => {}
            Err(error) => {
                note("denied.log", &error.to_string());
                std::process::exit(3);
            }
        }
    }
    if mode == "duplicate" {
        // The client refuses a repeat itself, so the launcher is asked
        // directly: its own refusal and fault record are what is under test.
        let seq = client.next_seq;
        let reply = client
            .call(json!({"op": "permit", "action_id": actions[0], "seq": seq}))
            .await
            .expect("the launcher answers");
        note("denied.log", &reply.to_string());
        std::process::exit(3);
    }
}

/// Declared before running: the five action lists are the ones the task
/// declares, with the ceilings 6/4/5/6/5 (the reserved End included), End
/// scripted in (a) and (d) only.
#[test]
fn the_five_scenarios_declare_the_explicit_action_lists_and_ceilings() {
    let scripts: Vec<Vec<&str>> = SCENARIOS.iter().map(Scenario::script).collect();
    assert_eq!(
        scripts,
        [
            vec![
                "candidate_join",
                "seed_fixture",
                "controlled_recovery_trigger",
                "candidate_leave",
                "candidate_rejoin",
                "end"
            ],
            vec![
                "candidate_join",
                "seed_fixture",
                "controlled_recovery_trigger"
            ],
            vec![
                "candidate_join",
                "seed_fixture",
                "controlled_recovery_trigger",
                "candidate_leave"
            ],
            vec![
                "candidate_join",
                "seed_fixture",
                "thinking_hold",
                "controlled_recovery_trigger",
                "thinking_release",
                "end"
            ],
            vec![
                "candidate_join",
                "seed_fixture",
                "controlled_recovery_trigger",
                "room_delete"
            ],
        ]
    );
    assert_eq!(
        SCENARIOS.iter().map(|s| s.ceiling).collect::<Vec<_>>(),
        [6, 4, 5, 6, 5]
    );
    for scenario in &SCENARIOS {
        assert_eq!(
            scenario.ceiling,
            scenario.actions.len() + 1,
            "{}",
            scenario.name
        );
        assert!(!scenario.actions.contains(&FINAL_ACTION_ID));
    }
    assert_eq!(
        SCENARIOS.iter().map(|s| s.report).collect::<Vec<_>>(),
        [true, true, false, true, false]
    );

    // Every scenario has a run ID, and a manifest end mode pairing, of its own:
    // the matrix gate tells the scenarios apart by them.
    let mut ids: Vec<_> = SCENARIOS.iter().map(|s| s.name.replace('_', "-")).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), SCENARIOS.len());
    let mut modes: Vec<_> = SCENARIOS
        .iter()
        .map(|s| s.end_mode.manifest_name())
        .collect();
    modes.sort();
    assert_eq!(
        modes,
        [
            "absence",
            "manual",
            "manual",
            "room_closed",
            "server_deadline"
        ]
    );
}

/// Declared before running: the wall budgets are the declared ones. The three
/// non-deadline cases take setup + observation + ending + cleanup = 660s under
/// the 15 minute cap; the deadline case is 2 + 12 + 5 + 1 = 20 minutes under a
/// 20 minute cap. The absence grace and the loop's tick fit the 180s
/// observation, and the transient return is well before the grace.
#[test]
fn the_scenario_budgets_fit_their_caps() {
    for scenario in &SCENARIOS {
        let total = SETUP_S + scenario.observation_s() + ENDING_S + CLEANUP_S;
        match scenario.end_mode {
            EndMode::ServerDeadline => {
                assert_eq!(scenario.observation_s(), 12 * 60);
                assert_eq!(total, 20 * 60);
                assert_eq!(scenario.outer_cap_s, 20 * 60);
            }
            EndMode::Manual | EndMode::Absence | EndMode::RoomClosed => {
                assert_eq!(total, 660);
                assert!(total < 15 * 60);
                assert_eq!(scenario.outer_cap_s, 15 * 60);
            }
        }
        assert!(
            scenario.selected_duration_s() + scenario.cleanup_reserve_s() <= scenario.outer_cap_s,
            "{}",
            scenario.name
        );
    }
    let absence =
        super::super::super::CANDIDATE_ABSENCE_LIMIT.as_secs_f64() + crate::agent::WATCH_TICK_S;
    assert!(absence < OBSERVATION_S as f64, "{absence}");
    assert!(RETURN_WITHIN_S < super::super::super::CANDIDATE_ABSENCE_LIMIT.as_secs());
}

/// Declared before running: the controller accepts the manifest each scenario
/// builds. The controller's own `validate` is the judge, not a copy of its
/// rules.
#[test]
fn the_launcher_validates_each_scenarios_manifest() {
    let scratch = Scratch::new("manifests");
    let candidate = scratch.path("candidate");
    std::fs::write(&candidate, "x").unwrap();
    for scenario in &SCENARIOS {
        let manifest = scenario.manifest(&candidate, &"c".repeat(64));
        let path = scratch.path("manifest.json");
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let output = std::process::Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/interview-run-budget.py"))
            .args(["validate", "--manifest"])
            .arg(&path)
            .output()
            .expect("python3 runs the controller");
        assert!(
            output.status.success(),
            "{}: {}{}",
            scenario.name,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            duration_min_from_manifest(&manifest, None),
            Ok(SCENARIO_DURATION_MIN),
            "{}",
            scenario.name
        );
    }
}

/// Declared before running: the bootstrap duration comes from the validated
/// manifest by the duration rules, and the real hard-deadline helper turns the
/// server-deadline case's 10 minutes into 12 minutes.
#[test]
fn the_deadline_scenario_bootstrap_has_a_twelve_minute_hard_deadline() {
    let scratch = Scratch::new("deadline");
    let candidate = scratch.path("candidate");
    std::fs::write(&candidate, "x").unwrap();
    let manifest = SCENARIOS[1].manifest(&candidate, &"c".repeat(64));
    let duration = duration_min_from_manifest(&manifest, None).unwrap();
    let config = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let boot = crate::runtime::bootstrap(&config, "live-recovery", Some("two-sum"), duration);
    assert_eq!(boot.duration_min, 10);
    assert_eq!(
        super::super::super::interview_hard_deadline(boot.duration_min),
        Duration::from_secs(12 * 60)
    );
    assert_eq!(manifest["observation_target_s"], 12 * 60);
}

/// Declared before running: the duration rules are the token endpoint's.
/// Outside 10..=90,
/// a fraction, a string, a missing field, a recording cap below the request, a
/// deadline run whose observation is not the server's deadline and a manual run
/// observing longer than its interview are all refused; a cap below the floor
/// clamps to the floor rather than panicking, as the token endpoint does.
#[test]
fn the_manifest_duration_follows_the_product_range_and_the_recording_cap() {
    let base = |end_mode: &str, minutes: Value, target: u64| json!({"end_mode": end_mode, "duration_min": minutes, "observation_target_s": target});
    for (manifest, cap, expected) in [
        (base("server_deadline", json!(10), 720), None, Ok(10)),
        (
            base("server_deadline", json!(90), 90 * 60 + 120),
            None,
            Ok(90),
        ),
        (base("manual", json!(10), 180), None, Ok(10)),
        (base("absence", json!(10), 600), None, Ok(10)),
        (base("room_closed", json!(10), 180), None, Ok(10)),
        (base("room_closed", json!(10), 601), None, Err(())),
        (base("manual", json!(10), 180), Some(5), Ok(10)),
        (base("manual", json!(20), 180), Some(15), Err(())),
        (base("manual", json!(9), 180), None, Err(())),
        (base("manual", json!(91), 180), None, Err(())),
        (base("manual", json!(10.5), 180), None, Err(())),
        (base("manual", json!("10"), 180), None, Err(())),
        (base("manual", json!(0), 180), None, Err(())),
        (base("manual", json!(10), 601), None, Err(())),
        (base("server_deadline", json!(10), 600), None, Err(())),
        (base("server_deadline", json!(10), 721), None, Err(())),
        (base("explicit_end", json!(10), 180), None, Err(())),
        (
            json!({"end_mode": "manual", "observation_target_s": 180}),
            None,
            Err(()),
        ),
    ] {
        assert_eq!(
            duration_min_from_manifest(&manifest, cap).map_err(|_| ()),
            expected,
            "{manifest} cap {cap:?}"
        );
    }
}

/// Records what a script did, and answers the barriers.
#[derive(Default)]
struct ScriptedEffects {
    log: Vec<String>,
    gate_entered_after_trigger: bool,
}

impl Effects for ScriptedEffects {
    async fn perform(&mut self, action: &str) -> Result<(), String> {
        self.log.push(format!("effect:{action}"));
        Ok(())
    }
    async fn await_gate_entered(&mut self) -> Result<(), String> {
        assert_eq!(
            self.log.last().map(String::as_str),
            Some("effect:controlled_recovery_trigger"),
            "the gate is awaited right after the trigger"
        );
        self.gate_entered_after_trigger = true;
        self.log.push("barrier:gate_entered".into());
        Ok(())
    }
    async fn await_terminal(&mut self, scenario: &Scenario) -> Result<(), String> {
        assert!(self.gate_entered_after_trigger);
        self.log
            .push(format!("barrier:terminal:{:?}", scenario.end_mode));
        Ok(())
    }
    async fn release_gate(&mut self) {
        assert!(
            self.log
                .iter()
                .any(|entry| entry.starts_with("barrier:terminal")),
            "the gate is retired only after the expected terminal"
        );
        self.log.push("release_gate".into());
    }
}

/// Declared before running: each scenario's script takes one permit before each
/// of its effects, in order, within its ceiling; the gate is awaited after the
/// trigger and released only after the terminal; barriers take no permit.
#[tokio::test]
async fn each_scenario_script_takes_a_permit_before_each_effect_within_its_ceiling() {
    for scenario in &SCENARIOS {
        let scratch = Scratch::new("script");
        let mut allowed = scenario.actions.to_vec();
        allowed.push(FINAL_ACTION_ID);
        let manifest = simple_manifest(scenario.actions);
        let service = start_service(
            &scratch,
            Reply::Faithful,
            &allowed,
            scenario.ceiling,
            &manifest,
        );
        let mut permits = PermitClient::from_variables(variables(&service))
            .await
            .unwrap();
        let mut effects = ScriptedEffects::default();
        run_script(&mut permits, scenario, &mut effects)
            .await
            .unwrap_or_else(|error| panic!("{}: {error:?}", scenario.name));

        let script = scenario.script();
        assert_eq!(permits.granted, script, "{}", scenario.name);
        assert!(permits.granted.len() <= scenario.ceiling);
        let effects_done: Vec<_> = effects
            .log
            .iter()
            .filter_map(|entry| entry.strip_prefix("effect:"))
            .collect();
        assert_eq!(effects_done, script, "{}", scenario.name);
        assert_eq!(service.permits.load(Ordering::SeqCst), script.len());
        assert_eq!(effects.log.last().map(String::as_str), Some("release_gate"));
        // Automatic terminals spend no End; cleanup may still ask for it once.
        if !scenario.scripted_end {
            cleanup_end(&mut permits, &mut effects).await.unwrap();
            assert_eq!(permits.granted.last().map(String::as_str), Some("end"));
            assert!(permits.granted.len() <= scenario.ceiling);
        } else {
            assert!(cleanup_end(&mut permits, &mut effects).await.is_err());
            assert_eq!(effects.log.iter().filter(|e| *e == "effect:end").count(), 1);
        }
    }
}

/// Declared before running: when the launcher refuses the second action, the
/// first effect happened, the second did not, and the script ends in a permit
/// error.
#[tokio::test]
async fn a_refused_permit_stops_the_script_before_its_effect() {
    let scenario = &SCENARIOS[1];
    let scratch = Scratch::new("refused");
    // Only the first action and End are allowed; seed_fixture is unknown.
    let manifest = simple_manifest(scenario.actions);
    let service = start_service(
        &scratch,
        Reply::Faithful,
        &["candidate_join", "end"],
        scenario.ceiling,
        &manifest,
    );
    let mut permits = PermitClient::from_variables(variables(&service))
        .await
        .unwrap();
    let mut effects = ScriptedEffects::default();
    let result = run_script(&mut permits, scenario, &mut effects).await;
    assert!(matches!(result, Err(ScriptError::Permit(_))), "{result:?}");
    assert_eq!(effects.log, ["effect:candidate_join"]);
}

/// Declared before running: the unwired live effects stop the very first
/// permitted step with an error, and nothing past it runs.
#[tokio::test]
async fn the_unwired_live_effects_stop_the_script_at_the_first_action() {
    let scenario = &SCENARIOS[0];
    let (_scratch, service, mut permits) = joined(Reply::Faithful, scenario.actions).await;
    let result = run_script(&mut permits, scenario, &mut UnwiredEffects).await;
    assert!(
        matches!(result, Err(ScriptError::Effect(ref why)) if why.starts_with("NOT RUN")),
        "{result:?}"
    );
    assert_eq!(service.permits.load(Ordering::SeqCst), 1);
}

/// Declared before running: once the launcher's stop file exists no further
/// action starts, whichever action is next, and cleanup asks for the reserved
/// End exactly once when the candidate had joined and End was not yet spent.
#[tokio::test]
async fn a_stop_request_starts_no_action_and_cleanup_spends_end_once() {
    let scenario = &SCENARIOS[0];
    let (_scratch, service, mut permits) = joined(Reply::Faithful, scenario.actions).await;
    let mut effects = ScriptedEffects::default();
    permits
        .guarded("candidate_join", effects.perform("candidate_join"))
        .await
        .unwrap()
        .unwrap();
    std::fs::write(stop_file(&service), "").unwrap();
    let result = run_script(&mut permits, scenario, &mut effects).await;
    assert_eq!(result, Err(ScriptError::Stopped));
    assert_eq!(effects.log, ["effect:candidate_join"]);
    assert_eq!(service.permits.load(Ordering::SeqCst), 1);

    cleanup_end_if_needed(&mut permits, &mut effects, &result)
        .await
        .unwrap();
    assert_eq!(effects.log, ["effect:candidate_join", "effect:end"]);
    cleanup_end_if_needed(&mut permits, &mut effects, &result)
        .await
        .unwrap();
    assert_eq!(effects.log.len(), 2, "End is spent once");
    assert_eq!(permits.granted, ["candidate_join", "end"]);

    // A run whose candidate never joined has no room to end.
    let (_scratch, service, mut unjoined) = joined(Reply::Faithful, scenario.actions).await;
    let mut effects = ScriptedEffects::default();
    cleanup_end_if_needed(&mut unjoined, &mut effects, &Err(ScriptError::Stopped))
        .await
        .unwrap();
    assert!(effects.log.is_empty());
    assert_eq!(service.permits.load(Ordering::SeqCst), 0);
}

/// Declared before running: the armed gate reaches the queue that
/// `run_room_with_slot` fills its `RoomLoop` from, with the recovery write's
/// phase, and the handles are the fixture's. This checks the hand-off only; it
/// does not run a room loop, so that the loop honours the gate is covered by
/// the recovery matrices on a hand-built loop, not here.
#[tokio::test]
async fn the_armed_recovery_gate_reaches_the_room_loops_barrier_queue() {
    let gate = RecoveryGate::arm();
    let queued = super::super::super::take_live_write_barriers();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].phase, RECOVERY_GATE_PHASE);
    assert!(
        super::super::super::take_live_write_barriers().is_empty(),
        "a loop takes the gates once"
    );
    assert!(gate.entered(Duration::from_millis(20)).await.is_err());
    queued[0].entered.notify_one();
    gate.entered(Duration::from_secs(1)).await.unwrap();
    let released = std::sync::Arc::clone(&queued[0].release);
    let waiter = tokio::spawn(async move { released.notified().await });
    tokio::task::yield_now().await;
    gate.release();
    tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("release wakes the holder")
        .unwrap();
}

/// Declared before running: the tally acknowledges a report once per received
/// copy, counts distinct final reports by their bytes, and does not count a
/// provisional report or a non-report. A report-producing case needs exactly
/// one distinct final report and the others none.
#[test]
fn the_report_tally_counts_distinct_final_reports() {
    let final_report = br#"{"codingScore":80}"#;
    let provisional = br#"{"reportRecovery":{"cause":"unavailable"}}"#;
    let mut tally = ReportTally::default();
    assert!(tally.observe(Some("control"), final_report).is_none());
    assert!(tally.observe(Some("report"), b"invalid").is_none());
    assert_eq!(tally.receipts, 0);
    assert!(!tally.matches(&SCENARIOS[0]) && tally.matches(&SCENARIOS[2]));

    assert!(tally.observe(Some("report"), provisional).is_some());
    assert_eq!((tally.provisional, tally.final_ids.len()), (1, 0));
    assert!(tally.observe(Some("report"), final_report).is_some());
    assert!(tally.observe(Some("report"), final_report).is_some());
    assert_eq!((tally.receipts, tally.final_ids.len()), (3, 1));
    assert!(tally.matches(&SCENARIOS[0]) && !tally.matches(&SCENARIOS[2]));
    assert!(
        tally
            .observe(Some("report"), br#"{"codingScore":81}"#)
            .is_some()
    );
    assert!(
        !tally.matches(&SCENARIOS[0]),
        "two distinct finals are not one"
    );
}

// Offline checks of the live fixture's own logic: the observer, the receipt
// count, the verdict, the redaction and the manifest writer.

fn at(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// Declared before running: a candidate leaving, closing its own connection and
/// returning is a sighting of the candidate and ends nothing. Only the
/// interviewer leaving, its task finishing or the server closing the room is an
/// ending, and a candidate who never returns still ends nothing by itself.
#[test]
fn a_transient_leave_is_not_an_ending() {
    let mut seen = PresenceObserver::default();
    seen.note(at(1), Sighting::Control("interviewer_state".into()));
    seen.note(at(2), Sighting::CandidateLeft);
    seen.note(at(2), Sighting::OwnClose);
    assert!(!seen.ended(), "a leave in progress is not an ending");
    seen.note(at(12), Sighting::CandidateReturned);
    assert!(!seen.ended(), "a return is not an ending");
    assert_eq!((seen.transient_leaves, seen.candidate_absent), (1, false));

    let mut gone = PresenceObserver::default();
    gone.note(at(2), Sighting::CandidateLeft);
    gone.note(at(2), Sighting::OwnClose);
    assert!(!gone.ended(), "absence alone is not an ending");
    assert_eq!(gone.transient_leaves, 0);
    gone.note(at(95), Sighting::AgentTaskFinished);
    assert!(gone.ended());
    assert_eq!(gone.ended_at(), Some(at(95)));

    for ending in [
        Sighting::AgentLeft,
        Sighting::AgentTaskFinished,
        Sighting::RoomClosed("RoomDeleted".into()),
    ] {
        let mut seen = PresenceObserver::default();
        seen.note(at(1), ending.clone());
        assert!(seen.ended(), "{ending:?}");
    }
    let mut first = PresenceObserver::default();
    first.note(at(30), Sighting::AgentTaskFinished);
    first.note(at(20), Sighting::AgentLeft);
    first.note(at(40), Sighting::AgentLeft);
    assert_eq!(first.ended_at(), Some(at(20)), "the earliest ending wins");
}

/// Declared before running: a second copy of the same final report is a second
/// receipt, so it is not the exactly-one a report case needs, while the
/// distinct-report count alone would still accept it. A no-report case needs
/// none, and a provisional report is neither.
#[test]
fn a_second_final_receipt_is_not_success() {
    let mut tally = ReportTally::default();
    assert!(!tally.exactly(&SCENARIOS[0]) && tally.exactly(&SCENARIOS[2]));
    assert!(
        tally
            .observe(Some("report"), br#"{"reportRecovery":{}}"#)
            .is_some()
    );
    assert!(
        !tally.exactly(&SCENARIOS[0]),
        "a provisional report is not final"
    );
    assert!(
        tally
            .observe(Some("report"), br#"{"codingScore":80}"#)
            .is_some()
    );
    assert!(tally.exactly(&SCENARIOS[0]) && !tally.exactly(&SCENARIOS[2]));
    assert!(
        tally
            .observe(Some("report"), br#"{"codingScore":80}"#)
            .is_some()
    );
    assert!(
        tally.matches(&SCENARIOS[0]),
        "distinct bytes alone would accept the resend"
    );
    assert!(
        !tally.exactly(&SCENARIOS[0]),
        "a resend is a second receipt"
    );
    assert_eq!((tally.final_copies, tally.final_ids.len()), (2, 1));
}

fn passing_facts(scenario: &Scenario) -> Facts {
    Facts {
        gate_entered: true,
        agent: AgentEnd::Completed,
        final_copies: usize::from(scenario.report),
        final_distinct: usize::from(scenario.report),
        provisional: 0,
        ack_failures: 0,
        ended_before_end: false,
        agent_after_return: (scenario.name.starts_with('a')).then_some(true),
        end_while_pending: scenario.scripted_end.then_some(true),
        deadline_elapsed_s: Some(725.0),
        absence_elapsed_s: Some(92.0),
        participants_left: Some(Vec::new()),
        readers_unfinished: 0,
        room_gone_after_delete: true,
        terminal: (scenario.end_mode == EndMode::RoomClosed).then(|| "RoomClosed".to_owned()),
        delete_after_gate: (scenario.end_mode == EndMode::RoomClosed).then_some(true),
        terminal_after_delete: (scenario.end_mode == EndMode::RoomClosed).then_some(true),
    }
}

/// Declared before running: the verdict passes the facts each scenario
/// declares, and each single departure from them is named.
#[test]
fn the_verdict_names_each_departure_from_a_scenario() {
    for scenario in &SCENARIOS {
        assert_eq!(
            judge(scenario, &passing_facts(scenario)),
            Vec::<String>::new()
        );
    }

    // Each applies to (a), whose script has the leave, the return and End.
    type Departure = (&'static str, fn(&mut Facts), &'static str);
    let departures: [Departure; 12] = [
        ("gate", |f| f.gate_entered = false, "never entered"),
        ("agent", |f| f.agent = AgentEnd::Aborted, "did not complete"),
        (
            "failed",
            |f| f.agent = AgentEnd::Failed("x".into()),
            "did not complete",
        ),
        (
            "two receipts",
            |f| f.final_copies += 1,
            "final report receipt",
        ),
        (
            "two reports",
            |f| f.final_distinct += 1,
            "final report receipt",
        ),
        ("ack", |f| f.ack_failures = 1, "acknowledgements"),
        (
            "early",
            |f| f.ended_before_end = true,
            "before the scripted End",
        ),
        (
            "left",
            |f| f.participants_left = Some(vec!["x".into()]),
            "participants left",
        ),
        ("room", |f| f.room_gone_after_delete = false, "still listed"),
        ("reader", |f| f.readers_unfinished = 1, "Live reader"),
        (
            "end",
            |f| f.end_while_pending = Some(false),
            "still pending",
        ),
        (
            "return",
            |f| f.agent_after_return = Some(false),
            "after the return",
        ),
    ];
    for (label, change, needle) in departures {
        let scenario = &SCENARIOS[0];
        let mut facts = passing_facts(scenario);
        change(&mut facts);
        let problems = judge(scenario, &facts);
        assert!(
            problems.iter().any(|p| p.contains(needle)),
            "{label}: {problems:?}"
        );
    }
    let report_in_c = Facts {
        final_copies: 1,
        final_distinct: 1,
        ..passing_facts(&SCENARIOS[2])
    };
    assert!(
        !judge(&SCENARIOS[2], &report_in_c).is_empty(),
        "absence leaves with no report"
    );
    let no_report_in_b = Facts {
        final_copies: 0,
        final_distinct: 0,
        ..passing_facts(&SCENARIOS[1])
    };
    assert!(!judge(&SCENARIOS[1], &no_report_in_b).is_empty());
    let early_deadline = Facts {
        deadline_elapsed_s: Some(300.0),
        ..passing_facts(&SCENARIOS[1])
    };
    assert!(judge(&SCENARIOS[1], &early_deadline)[0].contains("hard deadline"));
    let early_absence = Facts {
        absence_elapsed_s: Some(20.0),
        ..passing_facts(&SCENARIOS[2])
    };
    assert!(judge(&SCENARIOS[2], &early_absence)[0].contains("grace"));
    let unobserved = Facts {
        deadline_elapsed_s: None,
        ..passing_facts(&SCENARIOS[1])
    };
    assert!(
        !judge(&SCENARIOS[1], &unobserved).is_empty(),
        "no measurement is not a pass"
    );
}

/// Declared before running: a credential carried in a URL query never reaches
/// a log through an error's text.
#[test]
fn redaction_removes_credentials_carried_in_a_query() {
    for (text, secret) in [
        (
            "wss://x/rtc?access_token=SECRETJWT&protocol=15",
            "SECRETJWT",
        ),
        ("GET /v1?key=AIzaSECRET failed", "AIzaSECRET"),
        ("bad url (https://h/p?foo=1&key=ABC123)", "ABC123"),
        ("a token=XYZ and key=QRS", "XYZ"),
    ] {
        let clean = redact(text);
        assert!(!clean.contains(secret), "{text} -> {clean}");
        assert!(clean.contains("REDACTED"), "{clean}");
    }
    for (text, secret) in [
        ("WSS://x/rtc?Access_Token=UPPERJWT", "UPPERJWT"),
        ("Authorization: Bearer HEADERJWT", "HEADERJWT"),
        ("{\"api_key\": \"JSONKEY\"}", "JSONKEY"),
    ] {
        assert!(!redact(text).contains(secret), "{text}");
    }
    register_secrets(["registered-secret-value".to_owned()]);
    assert!(!redact("oops registered-secret-value here").contains("registered-secret-value"));
    assert_eq!(redact("no credentials here"), "no credentials here");
    assert!(redact("wss://x/rtc?access_token=T&protocol=15").ends_with("&protocol=15"));
}

/// Declared before running: the selector accepts exactly a, b, c, d and e.
#[test]
fn the_scenario_selector_takes_one_letter() {
    for (letter, name) in [
        ("a", "a_manual_end_with_transient_leave"),
        ("b", "b_server_deadline"),
        ("c", "c_absence_expiry"),
        ("d", "d_end_during_pending_recovery_with_thinking_hold"),
        ("e", "e_room_closed_during_recovery"),
    ] {
        assert_eq!(scenario_for(letter).map(|s| s.name), Some(name));
    }
    for bad in ["", "f", "ab", "a_manual", "A", " a", "e_room"] {
        assert!(scenario_for(bad).is_none(), "{bad:?}");
    }
}

/// Declared before running: the manifest writer, started as the launcher
/// wrapper starts it, writes for each letter a manifest the controller accepts
/// with its binary pin verified, naming the scenario's own actions. The pinned
/// binary is this test binary.
#[test]
fn the_manifest_writer_produces_what_the_controller_validates() {
    let scratch = Scratch::new("writer");
    let name = format!(
        "{}::live_write_manifest",
        module_path!().split_once("::").unwrap().1
    );
    for scenario in &SCENARIOS {
        let letter = &scenario.name[..1];
        let out = scratch.path(&format!("manifest-{letter}.json"));
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env("CODETRIAL_LIVE_MANIFEST_OUT", &out)
            .env("CODETRIAL_LIVE_SCENARIO", letter)
            .env("CODETRIAL_LIVE_CANDIDATE", std::env::current_exe().unwrap())
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let manifest: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(manifest["actions"], json!(scenario.actions));
        assert_eq!(manifest["run_id"], json!(scenario.name.replace('_', "-")));
        let checked = std::process::Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/interview-run-budget.py"))
            .args(["validate", "--verify-binaries", "--manifest"])
            .arg(&out)
            .output()
            .unwrap();
        assert!(
            checked.status.success(),
            "{}: {}{}",
            scenario.name,
            String::from_utf8_lossy(&checked.stdout),
            String::from_utf8_lossy(&checked.stderr)
        );
    }
}

fn private_gate() -> (RecoveryGate, super::super::WriteBarrier) {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    (
        RecoveryGate {
            entered: std::sync::Arc::clone(&entered),
            release: std::sync::Arc::clone(&release),
        },
        super::super::WriteBarrier {
            phase: RECOVERY_GATE_PHASE,
            entered,
            release,
        },
    )
}

/// Declared before running: ending the first Live reader, as the trigger does
/// through the probe's inspection, is a hang-up from the loop's side. The room
/// loop's own Close path opens a replacement on a local socket and reaches the
/// recovery brief write, where the gate is entered while the recovery has not
/// returned; releasing it lets the replacement finish and the old socket's
/// reader is finished. This runs the real recovery driver on a local Gemini
/// socket, not a live one.
#[tokio::test]
async fn ending_the_first_reader_drives_the_close_path_to_the_recovery_gate() {
    if super::isolated("live::ending_the_first_reader_drives_the_close_path_to_the_recovery_gate") {
        return;
    }
    let (listener, endpoint) = super::listener().await;
    let mut rig = super::Rig::new(endpoint).await;
    let (gate, barrier) = private_gate();
    rig.loops.recovery_writes.push_back(barrier);
    let reader = rig
        .gemini
        .inspect_reader_for_test()
        .expect("setup owns its reader");
    assert!(!reader.is_finished());
    reader.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let began = std::time::Instant::now();
    while !reader.is_finished() {
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "the reader did not end"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(2), rig.gemini.next_event())
            .await
            .expect("ending the reader closes the event stream")
            .is_none(),
        "the loop would see a hang-up"
    );
    let server = tokio::spawn(super::recording_fresh_gemini(
        listener,
        vec![super::FreshStep::Ready],
    ));
    let done = std::sync::atomic::AtomicBool::new(false);
    let (flow, entered_while_pending) = tokio::join!(
        async {
            let flow = rig.run().await;
            done.store(true, Ordering::SeqCst);
            flow
        },
        async {
            gate.entered(Duration::from_secs(8)).await.unwrap();
            let pending = !done.load(Ordering::SeqCst);
            gate.release();
            pending
        }
    );
    assert!(entered_while_pending, "the gate is entered mid-recovery");
    assert!(flow.is_continue());
    let _ = rig.old_frames().await;
    assert!(reader.is_finished());
    assert_eq!(server.await.unwrap().len(), 1, "one replacement socket");
}

/// Declared before running: the fixture's setup is local until the join permit.
/// It signs a candidate token under the token endpoint's rules with the
/// candidate identity the agent matches, durationMin 10 and a Two Sum plan,
/// names a room of its own and arms the reader probe, and none of it touches a
/// provider.
#[tokio::test]
async fn the_live_effects_sign_a_candidate_token_locally() {
    use base64::Engine as _;
    let config = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let (gate, _barrier) = private_gate();
    let effects = LiveEffects::new(&SCENARIOS[1], config, None, 10, &gate, None).unwrap();
    assert!(effects.room_name.starts_with("ct-live-b-"));
    assert_eq!(effects.candidate_identity, "candidate-liveb");
    assert_eq!(
        effects.agent_identity,
        crate::runtime::agent_identity(&effects.room_name)
    );
    let claims: Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(effects.token.split('.').nth(1).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(claims["sub"], "candidate-liveb");
    assert_eq!(claims["video"]["room"], json!(effects.room_name));
    assert_eq!(claims["video"]["canPublishData"], true);
    assert_ne!(claims["video"]["agent"], true, "a candidate, not an agent");
    let metadata: Value = serde_json::from_str(claims["metadata"].as_str().unwrap()).unwrap();
    assert_eq!(metadata["candidateIdentity"], "candidate-liveb");
    assert_eq!(metadata["durationMin"], 10);
    assert_eq!(metadata["problemId"], "two-sum");
    assert!(crate::livekit::candidate_identity_matches(
        "candidate-liveb",
        claims["metadata"].as_str().unwrap()
    ));
    assert!(
        super::super::super::take_live_reader_probe(&effects.room_name).is_some(),
        "the reader probe is armed for the loop that starts"
    );
    assert!(
        effects.link.is_none() && effects.agent.is_none(),
        "nothing is joined"
    );
}

/// Declared before running: the scripted part's budget is the cap less the
/// cleanup minute, so the server-deadline case keeps its whole ending window
/// after the 12 minute deadline, and no case's budget exceeds its cap.
#[test]
fn the_script_budget_keeps_the_ending_window_inside_the_cap() {
    for scenario in &SCENARIOS {
        let budget = script_budget(scenario);
        assert!(budget < Duration::from_secs(scenario.outer_cap_s));
        assert_eq!(
            budget + Duration::from_secs(CLEANUP_S),
            Duration::from_secs(scenario.outer_cap_s)
        );

        // A typical join, the deadline or the scripted observation, then the
        // whole ending window.
        assert!(
            budget.as_secs() >= 30 + scenario.observation_s().min(300) + ENDING_S,
            "{}",
            scenario.name
        );
    }
    assert!(
        script_budget(&SCENARIOS[1]).as_secs() >= 30 + 12 * 60 + ENDING_S,
        "the deadline case reaches its report inside the budget"
    );
}

/// Declared before running: a deadline observation far past the hard deadline
/// plus the ending window is not the deadline ending it.
#[test]
fn a_deadline_far_past_the_hard_deadline_is_not_a_pass() {
    let late = Facts {
        deadline_elapsed_s: Some(720.0 + ENDING_S as f64 + 60.0),
        ..passing_facts(&SCENARIOS[1])
    };
    assert!(judge(&SCENARIOS[1], &late)[0].contains("hard deadline"));
    let edge = Facts {
        deadline_elapsed_s: Some(720.0 + ENDING_S as f64),
        ..passing_facts(&SCENARIOS[1])
    };
    assert!(judge(&SCENARIOS[1], &edge).is_empty());
}

// The room-closed case (e): the server deletes the room while the recovery
// write is held, so the agent's SDK reports a real `Disconnected` and the loop
// takes `Terminal::RoomClosed` and leaves without a report.

fn room_closed() -> &'static Scenario {
    scenario_for("e").unwrap()
}

/// Declared before running: the room-closed case is ordered join, seed,
/// trigger, delete; the delete is the driving action, the gate is awaited after
/// the trigger and before the delete, End is reserved and not scripted, and the
/// manifest says `room_closed` under its own run ID.
#[test]
fn the_room_closed_case_deletes_the_room_after_the_gate_and_reserves_end() {
    let scenario = room_closed();
    assert_eq!(scenario.script(), scenario.actions);
    assert_eq!(scenario.actions.last(), Some(&"room_delete"));
    assert!(!scenario.scripted_end);
    assert_eq!(scenario.ceiling, scenario.actions.len() + 1);
    assert!(!scenario.report);
    let manifest = scenario.manifest(Path::new("/pinned"), &"d".repeat(64));
    assert_eq!(manifest["run_id"], json!("e-room-closed-during-recovery"));
    assert_eq!(manifest["end_mode"], json!("room_closed"));
    assert_eq!(manifest["action_ceiling"], json!(5));
    assert_eq!(
        manifest["actions"],
        json!([
            "candidate_join",
            "seed_fixture",
            "controlled_recovery_trigger",
            "room_delete"
        ])
    );
    assert_eq!(manifest["outer_wall_cap_s"], json!(900));
    assert_eq!(
        scenario.selected_duration_s() + scenario.cleanup_reserve_s(),
        SETUP_S + OBSERVATION_S + ENDING_S + CLEANUP_S
    );
}

fn closed_by_server(facts: &mut Facts) {
    facts.terminal = Some("RoomClosed".into());
    facts.delete_after_gate = Some(true);
    facts.terminal_after_delete = Some(true);
}

/// Declared before running: the verdict passes the room-closed facts and names
/// each departure: any other terminal or none, a delete before the gate or
/// none, a terminal before the delete, a final report receipt, an unfinished
/// loop, a participant left and an unfinished reader.
#[test]
fn the_room_closed_verdict_names_each_departure() {
    let scenario = room_closed();
    let mut passing = passing_facts(scenario);
    closed_by_server(&mut passing);
    assert_eq!(judge(scenario, &passing), Vec::<String>::new());
    // A participants listing that found no room is acceptable: it is gone.
    assert!(passing.participants_left.is_some());
    let gone = Facts {
        participants_left: None,
        ..passing_facts(scenario)
    };
    assert_eq!(judge(scenario, &gone), Vec::<String>::new());

    type Departure = (&'static str, fn(&mut Facts), &'static str);
    let departures: [Departure; 13] = [
        ("end", |f| f.terminal = Some("End".into()), "not RoomClosed"),
        (
            "absent",
            |f| f.terminal = Some("Absent".into()),
            "not RoomClosed",
        ),
        (
            "deadline",
            |f| f.terminal = Some("Deadline".into()),
            "not RoomClosed",
        ),
        ("none", |f| f.terminal = None, "not RoomClosed"),
        (
            "delete before the gate",
            |f| f.delete_after_gate = Some(false),
            "after the recovery write was held",
        ),
        (
            "no delete",
            |f| f.delete_after_gate = None,
            "after the recovery write was held",
        ),
        (
            "early terminal",
            |f| f.terminal_after_delete = Some(false),
            "after the room was deleted",
        ),
        (
            "unobserved order",
            |f| f.terminal_after_delete = None,
            "after the room was deleted",
        ),
        (
            "report receipt",
            |f| {
                f.final_copies = 1;
                f.final_distinct = 1;
            },
            "final report receipt",
        ),
        ("gate", |f| f.gate_entered = false, "never entered"),
        ("loop", |f| f.agent = AgentEnd::Aborted, "did not complete"),
        (
            "left",
            |f| f.participants_left = Some(vec!["x".into()]),
            "participants left",
        ),
        ("reader", |f| f.readers_unfinished = 1, "Live reader"),
    ];
    for (label, change, needle) in departures {
        let mut facts = passing_facts(scenario);
        closed_by_server(&mut facts);
        change(&mut facts);
        let problems = judge(scenario, &facts);
        assert!(
            problems.iter().any(|p| p.contains(needle)),
            "{label}: {problems:?}"
        );
    }
    // The other four cases do not read the room-closed facts.
    for other in &SCENARIOS[..4] {
        let mut facts = passing_facts(other);
        facts.terminal = Some("RoomClosed".into());
        facts.delete_after_gate = Some(false);
        assert_eq!(judge(other, &facts), Vec::<String>::new(), "{}", other.name);
    }
}

/// Declared before running: the run's own `room_delete` is refused, and issues
/// no request, before the recovery write is held.
#[tokio::test]
async fn room_delete_is_refused_before_the_recovery_gate_is_entered() {
    let config = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "ws://127.0.0.1:9"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let (gate, _barrier) = private_gate();
    let mut effects =
        LiveEffects::new(room_closed(), config, None, 10, &gate, None).expect("local setup");
    let error = effects.perform("room_delete").await.unwrap_err();
    assert!(
        error.contains("before the recovery write was held"),
        "{error}"
    );
    assert!(effects.deleted_at.is_none(), "no delete was issued");
    assert_eq!(effects.delete_after_gate, Some(false));
    let facts = effects.facts(&Teardown {
        participants: None,
        room_gone_after_delete: true,
        readers_unfinished: 0,
        notes: Vec::new(),
    });
    assert_eq!(facts.terminal_after_delete, None);
    // The order is read from the instants the delete and the terminal carry.
    let now = std::time::Instant::now();
    let later = now + Duration::from_secs(1);
    let teardown = Teardown {
        participants: None,
        room_gone_after_delete: true,
        readers_unfinished: 0,
        notes: Vec::new(),
    };
    effects.deleted_at = Some(later);
    effects.terminal = Some(("RoomClosed", now));
    assert_eq!(effects.facts(&teardown).terminal_after_delete, Some(false));
    effects.terminal = Some(("RoomClosed", later));
    assert_eq!(effects.facts(&teardown).terminal_after_delete, Some(true));
    assert_eq!(
        effects.facts(&teardown).terminal.as_deref(),
        Some("RoomClosed")
    );
    assert!(
        judge(room_closed(), &facts)
            .iter()
            .any(|p| p.contains("after the recovery write was held"))
    );
}

/// Declared before running: a terminal the loop reports reaches only the probe
/// registered for its own room, with the name the judge reads and a time.
#[test]
fn the_live_terminal_probe_is_keyed_by_room_and_carries_the_terminal_name() {
    use super::super::Terminal;
    let (mine, mut mine_rx) = tokio::sync::mpsc::unbounded_channel();
    let (other, mut other_rx) = tokio::sync::mpsc::unbounded_channel();
    {
        let mut probes = super::super::super::LIVE_TERMINAL_PROBES.lock().unwrap();
        probes.push(("probe-room-mine".into(), mine));
        probes.push(("probe-room-other".into(), other));
    }
    let before = std::time::Instant::now();
    super::super::super::note_live_terminal("probe-room-mine", Terminal::RoomClosed.name());
    let (name, at) = mine_rx.try_recv().unwrap();
    assert_eq!(name, "RoomClosed");
    assert!(at >= before);
    assert!(
        other_rx.try_recv().is_err(),
        "another room's probe is silent"
    );
    super::super::super::LIVE_TERMINAL_PROBES
        .lock()
        .unwrap()
        .retain(|(room, _)| !room.starts_with("probe-room-"));
    let names = [
        Terminal::End {
            topic: String::new(),
            payload: Value::Null,
        }
        .name(),
        Terminal::Deadline.name(),
        Terminal::Absent.name(),
        Terminal::RoomClosed.name(),
    ];
    assert_eq!(names, ["End", "Deadline", "Absent", "RoomClosed"]);
}

/// Declared before running: with the recovery write held, the SDK's
/// `Disconnected` makes the real recovery driver take `Terminal::RoomClosed`,
/// the probe the live fixture reads reports exactly that, and the loop leaves
/// as production does (no hand-built terminal probe short-circuits it). This is
/// a local socket and a recorded room, not a live one.
#[tokio::test]
async fn a_room_disconnect_while_the_recovery_write_is_held_is_reported_as_room_closed() {
    if super::isolated(
        "live::a_room_disconnect_while_the_recovery_write_is_held_is_reported_as_room_closed",
    ) {
        return;
    }
    let (listener, endpoint) = super::listener().await;
    let mut rig = super::Rig::new(endpoint).await;
    rig.loops.recovery_terminal = None;
    let (live_terminal, mut reported) = tokio::sync::mpsc::unbounded_channel();
    super::super::super::LIVE_TERMINAL_PROBES
        .lock()
        .unwrap()
        .push((rig.room.name().to_owned(), live_terminal));
    let (gate, barrier) = private_gate();
    rig.loops.recovery_writes.push_back(barrier);
    let reader = rig
        .gemini
        .inspect_reader_for_test()
        .expect("setup owns its reader");
    reader.abort();
    let server = tokio::spawn(super::recording_fresh_gemini(
        listener,
        vec![super::FreshStep::Ready],
    ));
    let room = rig._room_sender.clone();
    let (flow, entered) = tokio::join!(rig.run(), async {
        gate.entered(Duration::from_secs(8)).await.unwrap();
        let held_at = std::time::Instant::now();
        room.send(RoomEvent::Disconnected {
            reason: ::livekit::DisconnectReason::RoomDeleted,
        })
        .unwrap();
        held_at
    });
    assert!(flow.is_break(), "RoomClosed leaves the room");
    let (name, at) = reported.try_recv().expect("the loop reported its terminal");
    assert_eq!(name, "RoomClosed");
    assert!(
        at >= entered,
        "the terminal came after the gate and the close"
    );
    assert!(reported.try_recv().is_err(), "one terminal");
    gate.release();
    server.abort();
}
