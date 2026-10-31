// Part of recovery_live.rs: included in place, so every item here shares that
// file's module.

impl<'a> LiveEffects<'a> {
    /// Everything that can be built without touching a provider: the candidate
    /// join token is signed here, locally, and not used until the permitted
    /// join.
    fn new(
        scenario: &'static Scenario,
        config: crate::config::AgentConfig,
        recording_cap: Option<u32>,
        duration: u32,
        gate: &'a RecoveryGate,
        stop_file: Option<PathBuf>,
    ) -> Result<Self, String> {
        let letter = &scenario.name[..1];
        let stamp = crate::current_epoch_millis();
        let room_name = format!("ct-live-{letter}-{stamp}");
        let candidate_identity = format!("candidate-live{letter}");
        let body = json!({"problemId": "two-sum", "durationMin": duration}).to_string();
        let minted = crate::web::token_response(
            &crate::web::TokenConfig {
                api_key: &config.livekit_api_key,
                api_secret: &config.livekit_api_secret,
                server_url: &config.livekit_url,
                recording_max_min: recording_cap,
            },
            body.as_bytes(),
            &room_name,
            &candidate_identity,
            crate::current_epoch_seconds(),
        )
        .map_err(|error| format!("cannot mint the candidate token: {error}"))?;
        if minted.duration_min != json!(duration) {
            return Err(format!(
                "the token endpoint's rules gave {} minutes, not {duration}",
                minted.duration_min
            ));
        }
        let (probe, readers) = tokio::sync::mpsc::unbounded_channel();
        super::super::super::LIVE_READER_PROBES
            .lock()
            .unwrap()
            .push((room_name.clone(), probe));
        let (terminal_probe, terminals) = tokio::sync::mpsc::unbounded_channel();
        super::super::super::LIVE_TERMINAL_PROBES
            .lock()
            .unwrap()
            .push((room_name.clone(), terminal_probe));
        Ok(Self {
            scenario,
            agent_identity: crate::runtime::agent_identity(&room_name),
            config: Arc::new(config),
            gate,
            stop_file,
            room_name,
            candidate_identity,
            token: minted.token,
            shared: Shared::new(),
            link: None,
            agent: None,
            agent_finished: Arc::new(std::sync::OnceLock::new()),
            agent_end: None,
            readers,
            first_reader: None,
            kept_readers: Vec::new(),
            ready_at: None,
            left_at: None,
            end_sent: false,
            joined: false,
            setup_s: SETUP_S,
            script_stop: std::time::Instant::now() + script_budget(scenario),
            hard_stop: std::time::Instant::now() + Duration::from_secs(scenario.outer_cap_s),
            tearing_down: false,
            gate_entered: false,
            ended_before_end: false,
            agent_after_return: None,
            end_while_pending: None,
            terminals,
            terminal: None,
            deleted_at: None,
            delete_after_gate: None,
        })
    }

    fn stop_requested(&self) -> bool {
        self.stop_file.as_deref().is_some_and(Path::exists)
    }

    fn agent_running(&self) -> bool {
        self.agent.is_some() && self.agent_finished.get().is_none()
    }

    /// Whether the interview is over, by the task and by what the candidate's
    /// side of the room saw.
    fn interview_over(&self) -> bool {
        !self.agent_running() || self.shared.observed.lock().unwrap().presence.ended()
    }

    /// Polls `done` until it holds, the time runs out, the launcher asks for a
    /// stop or the interviewer's task finishes when `until_agent_done` is not
    /// what is being waited for.
    async fn wait_for(
        &self,
        within: Duration,
        what: &str,
        mut done: impl FnMut(&Self) -> bool,
    ) -> Result<(), String> {
        let until = tokio::time::Instant::now() + within;
        loop {
            if done(self) {
                return Ok(());
            }
            if self.stop_requested() {
                return Err(format!("stopped by the launcher while waiting for {what}"));
            }
            if !self.tearing_down && std::time::Instant::now() >= self.script_stop {
                return Err(format!("the outer wall cap came while waiting for {what}"));
            }
            if tokio::time::Instant::now() >= until {
                return Err(format!("timed out after {within:?} waiting for {what}"));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Notes the first recovery terminal the room loop reported.
    fn drain_terminals(&mut self) {
        while let Ok(reported) = self.terminals.try_recv() {
            self.terminal.get_or_insert(reported);
        }
    }

    fn drain_readers(&mut self) {
        while let Ok(reader) = self.readers.try_recv() {
            if self.first_reader.is_none() {
                self.ready_at = Some(std::time::Instant::now());
                self.first_reader = Some(reader);
            } else {
                self.kept_readers.push(reader);
            }
        }
    }

    // LiveKit closes a room that only agents remain in once its departure
    // timeout passes (the server default is 20 s), long before the interview's
    // 90 s absence grace: the first live run of the absence scenario saw
    // `RoomClosed` about 28 s after the candidate left. Production now creates
    // its rooms explicitly in `/api/token`; the absence and room-closed
    // scenarios use that same call, so they exercise the room production
    // actually creates.
    async fn create_production_room(&self) -> Result<(), String> {
        super::super::super::room_service::create_interview_room(
            &self.config.livekit_url,
            &self.config.livekit_api_key,
            &self.config.livekit_api_secret,
            &self.room_name,
            crate::current_epoch_seconds(),
        )
        .await
        .map_err(|error| redact(&error.to_string()))
    }

    async fn join(&mut self) -> Result<(), String> {
        let began = std::time::Instant::now();
        self.joined = true;
        if matches!(
            self.scenario.end_mode,
            EndMode::Absence | EndMode::RoomClosed
        ) {
            self.create_production_room().await?;
        }
        let config = std::sync::Arc::clone(&self.config);
        let room = self.room_name.clone();
        let finished = std::sync::Arc::clone(&self.agent_finished);
        let shared = std::sync::Arc::clone(&self.shared);
        self.agent = Some(tokio::spawn(async move {
            let outcome = super::super::super::run_room_with_slot(
                &config,
                &room,
                crate::current_epoch_seconds(),
                None,
            )
            .await;
            let _ = finished.set(std::time::Instant::now());
            shared.note(Sighting::AgentTaskFinished);
            match outcome {
                Ok(super::super::super::RoomRunOutcome::Completed) => AgentEnd::Completed,
                Ok(super::super::super::RoomRunOutcome::NoCandidate) => AgentEnd::NoCandidate,
                Ok(super::super::super::RoomRunOutcome::FailedBeforeCandidate(error))
                | Err(error) => AgentEnd::Failed(redact(&error.to_string())),
            }
        }));
        self.link = Some(
            connect_candidate(
                &self.config.livekit_url,
                &self.token,
                &self.shared,
                &self.agent_identity,
            )
            .await?,
        );
        emit(
            "candidate_joined",
            json!({"room": self.room_name, "identity": self.candidate_identity}),
        );
        self.wait_for(
            Duration::from_secs(self.setup_s).saturating_sub(began.elapsed()),
            "the interview to start",
            |this| !this.agent_running() || this.first_reader_probed(),
        )
        .await
        .ok();
        self.drain_readers();
        if self.first_reader.is_none() {
            return Err(format!(
                "the interview did not start: agent running {}",
                self.agent_running()
            ));
        }
        emit(
            "interview_ready",
            json!({"after_ms": began.elapsed().as_millis() as u64}),
        );
        Ok(())
    }

    fn first_reader_probed(&self) -> bool {
        // `readers` is drained by `&mut self`; peeking needs the same, so the
        // waiter reads the flag the loop sets through the channel's length.
        !self.readers.is_empty()
    }

    async fn publish(&self, topic: &str, payload: &Value) -> Result<(), String> {
        let link = self.link.as_ref().ok_or("the candidate is not connected")?;
        let packet = super::super::super::browser_packet(topic, payload)
            .map_err(|error| error.to_string())?;
        link.room
            .local_participant()
            .publish_data(packet)
            .await
            .map_err(|error| redact(&error.to_string()))
    }

    async fn leave(&mut self) -> Result<(), String> {
        let link = self.link.take().ok_or("the candidate is not connected")?;
        self.left_at = Some(std::time::Instant::now());
        self.shared.note(Sighting::CandidateLeft);
        let closed = link.room.close().await;
        link.observer.abort();
        closed.map_err(|error| redact(&error.to_string()))?;
        if self.scenario.end_mode == EndMode::Manual {
            // A transient absence is held while the recovery is pending, and
            // the interview must still be running at the end of it.
            tokio::time::sleep(TRANSIENT_ABSENCE).await;
            if self.interview_over() {
                self.ended_before_end = true;
                return Err("the interview ended during a transient absence".into());
            }
        }
        Ok(())
    }

    async fn rejoin(&mut self) -> Result<(), String> {
        let left = self.left_at.ok_or("the candidate never left")?;
        if left.elapsed() >= Duration::from_secs(RETURN_WITHIN_S) {
            return Err("the return is not before the absence grace".into());
        }
        if self.interview_over() {
            self.ended_before_end = true;
            return Err("the interview ended before the candidate returned".into());
        }
        let link = connect_candidate(
            &self.config.livekit_url,
            &self.token,
            &self.shared,
            &self.agent_identity,
        )
        .await?;
        self.shared.note(Sighting::CandidateReturned);
        let seen = {
            let identity = self.agent_identity.clone();
            let began = std::time::Instant::now();
            loop {
                let present = link
                    .room
                    .remote_participants()
                    .keys()
                    .any(|who| who.0 == identity);
                if present || began.elapsed() >= RETURN_VISIBILITY {
                    break present;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        };
        self.agent_after_return = Some(seen);
        self.link = Some(link);
        Ok(())
    }

    async fn end(&mut self) -> Result<(), String> {
        let pending = !self.interview_over() && self.gate_entered && !self.end_sent;
        self.end_while_pending = Some(pending);
        if self.interview_over() && self.scenario.scripted_end {
            self.ended_before_end = true;
        }
        self.publish(
            crate::runtime::TOPIC_CONTROL,
            &json!({"type": "end_interview", "reason": "candidate_ended"}),
        )
        .await?;
        self.end_sent = true;
        Ok(())
    }

    /// The server closes this run's own named room while the recovery write is
    /// held. It refuses to run before the gate has really been entered, so the
    /// close can only arrive during the recovery.
    async fn delete_own_room(&mut self) -> Result<(), String> {
        let held = self.gate_entered;
        self.delete_after_gate = Some(held);
        if !held {
            return Err("room_delete before the recovery write was held".into());
        }
        self.deleted_at = Some(std::time::Instant::now());
        super::super::super::room_service::delete_room(
            &self.config.livekit_url,
            &self.config.livekit_api_key,
            &self.config.livekit_api_secret,
            &self.room_name,
            crate::current_epoch_seconds(),
        )
        .await
        .map_err(|error| redact(&error.to_string()))
    }

    async fn seed(&self) -> Result<(), String> {
        for code in [SEED_STARTER, SEED_SOLUTION] {
            self.publish(
                crate::runtime::TOPIC_CODE_UPDATE,
                &json!({"code": code, "language": "python"}),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok(())
    }

    /// Everything after the script: the candidate and the interviewer's task
    /// are retired, the room is checked for anyone left and then deleted, and
    /// every Live reader is checked finished.
    async fn teardown(&mut self) -> Teardown {
        let mut notes = Vec::new();
        self.tearing_down = true;
        if let Some(link) = self.link.take() {
            let _ = link.room.close().await;
            link.observer.abort();
        }
        if self.agent.is_some() && self.agent_end.is_none() {
            // An End the cleanup sent starts the ending window; anything else
            // only has the cleanup's own half minute before the abort.
            let patience = if self.end_sent {
                // What the cap leaves, keeping twenty seconds for the room
                // checks and the delete that must not be skipped.
                (self.hard_stop - Duration::from_secs(20))
                    .saturating_duration_since(std::time::Instant::now())
                    .min(Duration::from_secs(ENDING_S))
            } else {
                Duration::from_secs(CLEANUP_S / 2)
            };
            let finished = self
                .wait_for(patience, "the interviewer to finish", |this| {
                    this.agent_finished.get().is_some()
                })
                .await;
            if finished.is_err() {
                if let Some(handle) = &self.agent {
                    handle.abort();
                }
                notes.push("the interviewer's task was aborted".to_owned());
                self.agent_end = Some(AgentEnd::Aborted);
            }
        }
        if self.agent_end.is_none()
            && let Some(handle) = self.agent.take()
        {
            self.agent_end = Some(handle.await.unwrap_or(AgentEnd::Aborted));
        }
        self.drain_readers();
        self.drain_terminals();
        super::super::super::LIVE_TERMINAL_PROBES
            .lock()
            .unwrap()
            .retain(|(room, _)| *room != self.room_name);
        if !self.joined {
            // The join permit was never spent: no room was created, so no
            // provider call is made to look for one.
            return Teardown {
                participants: None,
                room_gone_after_delete: true,
                readers_unfinished: 0,
                notes,
            };
        }
        let participants = self.settled_participants().await;
        let deleted = super::super::super::room_service::delete_room(
            &self.config.livekit_url,
            &self.config.livekit_api_key,
            &self.config.livekit_api_secret,
            &self.room_name,
            crate::current_epoch_seconds(),
        )
        .await;
        if let Err(error) = &deleted {
            notes.push(format!(
                "deleting the room failed: {}",
                redact(&error.to_string())
            ));
        }
        let gone = matches!(self.listed_participants().await, Ok(None));
        let mut unfinished = 0;
        let began = std::time::Instant::now();
        let readers: Vec<_> = self.first_reader.iter().chain(&self.kept_readers).collect();
        while readers.iter().any(|reader| !reader.is_finished())
            && began.elapsed() < Duration::from_secs(5)
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        for reader in readers {
            if !reader.is_finished() {
                unfinished += 1;
            }
        }
        Teardown {
            participants: match participants {
                Ok(listed) => listed,
                Err(error) => {
                    notes.push(format!("listing the room failed: {error}"));
                    None
                }
            },
            room_gone_after_delete: gone,
            readers_unfinished: unfinished,
            notes,
        }
    }

    /// Identities in the room once LiveKit has had a few seconds to apply the
    /// last disconnect. `Ok(None)` when the room no longer exists.
    async fn settled_participants(&self) -> Result<Option<Vec<String>>, String> {
        let began = std::time::Instant::now();
        loop {
            let listed = self.listed_participants().await?;
            let empty = listed.as_ref().is_none_or(Vec::is_empty);
            if empty || began.elapsed() >= Duration::from_secs(10) {
                return Ok(listed);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn listed_participants(&self) -> Result<Option<Vec<String>>, String> {
        let token = crate::token::livekit_room_admin_token(
            &self.config.livekit_api_key,
            &self.config.livekit_api_secret,
            &self.room_name,
            crate::current_epoch_seconds(),
        )
        .map_err(|error| error.to_string())?;
        let (status, text) = super::super::super::room_service::post_room_service(
            &crate::config::livekit_http_base(&self.config.livekit_url),
            &token,
            "ListParticipants",
            &json!({"room": self.room_name}),
        )
        .await
        .map_err(|error| redact(&error.to_string()))?;
        if super::super::super::room_service::is_twirp_not_found(status, &text) {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(format!("ListParticipants answered {status}"));
        }
        let body: Value =
            serde_json::from_str(&text).map_err(|_| "ListParticipants was not JSON")?;
        Ok(Some(
            body["participants"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|who| who["identity"].as_str().map(str::to_owned))
                .collect(),
        ))
    }

    fn facts(&self, teardown: &Teardown) -> Facts {
        let observed = self.shared.observed.lock().unwrap();
        let ready = self.ready_at;
        let finished = self.agent_finished.get().copied();
        let elapsed = |from: Option<std::time::Instant>| match (from, finished) {
            (Some(from), Some(to)) => Some(to.saturating_duration_since(from).as_secs_f64()),
            _ => None,
        };
        Facts {
            gate_entered: self.gate_entered,
            agent: self.agent_end.clone().unwrap_or(AgentEnd::Aborted),
            final_copies: observed.tally.final_copies,
            final_distinct: observed.tally.final_ids.len(),
            provisional: observed.tally.provisional,
            ack_failures: observed.ack_failures,
            ended_before_end: self.ended_before_end,
            agent_after_return: self.agent_after_return,
            end_while_pending: self.end_while_pending,
            deadline_elapsed_s: elapsed(ready),
            absence_elapsed_s: elapsed(self.left_at),
            participants_left: teardown.participants.clone(),
            readers_unfinished: teardown.readers_unfinished,
            room_gone_after_delete: teardown.room_gone_after_delete,
            terminal: self.terminal.map(|(name, _)| name.to_owned()),
            delete_after_gate: self.delete_after_gate,
            terminal_after_delete: match (self.terminal, self.deleted_at) {
                (Some((_, at)), Some(deleted)) => Some(at >= deleted),
                _ => None,
            },
        }
    }
}

struct Teardown {
    participants: Option<Vec<String>>,
    room_gone_after_delete: bool,
    readers_unfinished: usize,
    notes: Vec<String>,
}

impl Effects for LiveEffects<'_> {
    async fn perform(&mut self, action: &str) -> Result<(), String> {
        emit("action", json!({"action": action}));
        match action {
            "candidate_join" => self.join().await,
            "seed_fixture" => self.seed().await,
            "controlled_recovery_trigger" => {
                self.drain_readers();
                let reader = self.first_reader.as_ref().ok_or("no Live reader to end")?;
                reader.abort();
                Ok(())
            }
            "thinking_hold" => {
                self.publish(
                    crate::runtime::TOPIC_CONTROL,
                    &json!({"type": "thinking", "thinking": true}),
                )
                .await
            }
            "thinking_release" => {
                self.publish(
                    crate::runtime::TOPIC_CONTROL,
                    &json!({"type": "thinking", "thinking": false}),
                )
                .await
            }
            "candidate_leave" => self.leave().await,
            "candidate_rejoin" => self.rejoin().await,
            "room_delete" => self.delete_own_room().await,
            FINAL_ACTION_ID => self.end().await,
            other => Err(format!("{other} has no live effect")),
        }
    }

    async fn await_gate_entered(&mut self) -> Result<(), String> {
        let until = tokio::time::Instant::now() + GATE_WAIT;
        loop {
            if self.stop_requested() {
                return Err("stopped by the launcher before the gate was entered".into());
            }
            if !self.agent_running() {
                return Err("the interviewer ended before the gate was entered".into());
            }
            let slice = Duration::from_secs(1)
                .min(until.saturating_duration_since(tokio::time::Instant::now()));
            if self.gate.entered(slice).await.is_ok() {
                self.gate_entered = true;
                emit("gate_entered", json!({"phase": RECOVERY_GATE_PHASE}));
                return Ok(());
            }
            if tokio::time::Instant::now() >= until {
                return Err("the prepared recovery write was not reached".into());
            }
        }
    }

    async fn await_terminal(&mut self, scenario: &Scenario) -> Result<(), String> {
        let within = match scenario.end_mode {
            EndMode::Manual => Duration::from_secs(ENDING_S),
            EndMode::ServerDeadline => {
                let hard = super::super::super::interview_hard_deadline(SCENARIO_DURATION_MIN);
                let spent = self.ready_at.map_or(Duration::ZERO, |at| at.elapsed());
                hard.saturating_sub(spent) + Duration::from_secs(ENDING_S)
            }
            EndMode::Absence => {
                super::super::super::CANDIDATE_ABSENCE_LIMIT + Duration::from_secs(60)
            }
            // The close is already issued; the loop only has to observe it.
            EndMode::RoomClosed => Duration::from_secs(60),
        };
        let terminal = self
            .wait_for(within, "the expected terminal", |this| {
                this.agent_finished.get().is_some()
            })
            .await;
        if let Some(handle) = self.agent.take() {
            if terminal.is_ok() {
                self.agent_end = Some(handle.await.unwrap_or(AgentEnd::Aborted));
            } else {
                self.agent = Some(handle);
            }
        }
        self.drain_terminals();
        emit(
            "terminal",
            json!({
                "mode": scenario.end_mode.manifest_name(),
                "observed": terminal.is_ok(),
                "loop_terminal": self.terminal.map(|(name, _)| name),
            }),
        );
        terminal
    }

    async fn release_gate(&mut self) {
        self.gate.release();
    }
}

/// Wall time the scripted part of a run may take: the cap less the final
/// cleanup minute. The ending window is inside it, because the terminal wait
/// and the report belong to the script, not to the cleanup.
fn script_budget(scenario: &Scenario) -> Duration {
    Duration::from_secs(scenario.outer_cap_s - CLEANUP_S)
}

/// The scenario the controller selected for this invocation, by its letter.
fn scenario_for(letter: &str) -> Option<&'static Scenario> {
    SCENARIOS
        .iter()
        .find(|scenario| letter.len() == 1 && scenario.name.starts_with(letter))
}

/// Writes the manifest of one scenario for a pinned binary. It does nothing
/// unless the launcher wrapper (or the test below) asks for it with
/// `CODETRIAL_LIVE_MANIFEST_OUT`, so the ordinary lane passes it vacuously; it
/// takes no permit and touches no provider.
#[test]
fn live_write_manifest() {
    let Ok(out) = std::env::var("CODETRIAL_LIVE_MANIFEST_OUT") else {
        return;
    };
    let letter = std::env::var("CODETRIAL_LIVE_SCENARIO").expect("set CODETRIAL_LIVE_SCENARIO");
    let candidate = PathBuf::from(
        std::env::var("CODETRIAL_LIVE_CANDIDATE").expect("set CODETRIAL_LIVE_CANDIDATE"),
    );
    let digest = crate::sha256_hex(&[&std::fs::read(&candidate).expect("read the pinned binary")]);
    let manifest = match scenario_for(&letter) {
        Some(scenario) => scenario.manifest(&candidate, &digest),
        None => continuity::arm_for(&letter)
            .expect("the scenario is one of a, b, c, d, e, s0, s20, s39, s41, s60")
            .manifest(
                &candidate,
                &digest,
                &continuity::clips_fixture_sha(std::env::var("CODETRIAL_LIVE_CLIPS").ok()),
            ),
    };
    std::fs::write(out, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
}

/// NOT RUN against LiveKit or Gemini. Hosts the real room loop in this
/// process, with a real SDK candidate in a real room and the configured
/// Gemini, under the B0.1 launcher that sets the permit variables. It spends
/// LiveKit and Gemini quota.
///
/// One scenario per invocation, chosen by `CODETRIAL_LIVE_SCENARIO` (a, b, c,
/// d or e) and checked against the manifest the launcher holds. Before any
/// permit it checks the variables and manifest, that the manifest names the
/// selected scenario with exactly its declared actions, ceiling, cap and end
/// mode, that the bootstrap built from the manifest has the hard deadline the
/// scenario observes, and that the candidate token is signed under the token
/// endpoint's own duration rules. Every side effect after that, including
/// starting the room loop, is behind a permit.
///
/// Real here: the LiveKit room and SDK transport on both ends, the candidate's
/// data packets, the interviewer's room loop and recovery driver, Gemini Live
/// and the report call. Not real: the speech (nothing is spoken; the editor
/// state is authored), the browser page (the receipt is the same digest the
/// page sends, built by `candidate_ack`), and the recovery trigger, which ends
/// the first Live reader so the loop takes its own Close path.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "NOT RUN: spends LiveKit and Gemini quota; needs the B0.1 launcher variables, CODETRIAL_LIVE_CONFIG, CODETRIAL_LIVE_SCENARIO and budget approval"]
async fn live_sdk_control_presence_and_report_delivery() {
    let mut permits = PermitClient::from_env()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let letter = std::env::var("CODETRIAL_LIVE_SCENARIO")
        .expect("set CODETRIAL_LIVE_SCENARIO to a, b, c, d or e");
    let scenario = scenario_for(&letter).expect("CODETRIAL_LIVE_SCENARIO is one of a, b, c, d, e");
    assert_eq!(
        permits.manifest["run_id"],
        json!(scenario.name.replace('_', "-")),
        "the manifest is for the selected scenario"
    );
    assert_eq!(
        permits.manifest["actions"],
        json!(scenario.actions),
        "the manifest lists the declared actions"
    );
    assert_eq!(
        permits.manifest["action_ceiling"],
        json!(scenario.ceiling),
        "the hard ceiling includes the reserved End"
    );
    assert_eq!(
        permits.manifest["outer_wall_cap_s"],
        json!(scenario.outer_cap_s)
    );
    assert_eq!(
        permits.manifest["end_mode"],
        json!(scenario.end_mode.manifest_name())
    );

    // The cap the server would apply to the token, from the same variable the
    // browser plan reads; unset means the product ceiling.
    let recording_cap = std::env::var("CODETRIAL_RECORDING_MAX_MINUTES")
        .ok()
        .map(|raw| raw.parse::<u32>().expect("a whole number of minutes"));
    let duration = duration_min_from_manifest(&permits.manifest, recording_cap)
        .unwrap_or_else(|error| panic!("{error}"));
    let config = crate::config::load_from_pairs(
        crate::config::read_config_file(Path::new(
            &std::env::var("CODETRIAL_LIVE_CONFIG").expect("set CODETRIAL_LIVE_CONFIG"),
        ))
        .expect("read the isolated live config"),
    )
    .unwrap_or_else(|_| panic!("invalid isolated live config"));
    assert!(
        !config.google_api_keys.is_empty(),
        "the live config names no Gemini key"
    );
    register_secrets(config.google_api_keys.iter().cloned().chain([
        config.livekit_api_key.clone(),
        config.livekit_api_secret.clone(),
    ]));
    let boot = crate::runtime::bootstrap(&config, "live-recovery", Some("two-sum"), duration);
    assert_eq!(
        super::super::super::interview_hard_deadline(boot.duration_min).as_secs(),
        u64::from(duration) * 60 + 120
    );

    // Armed before the loop exists, so the loop that starts takes it.
    let gate = RecoveryGate::arm();
    let mut effects = LiveEffects::new(
        scenario,
        config.clone(),
        recording_cap,
        duration,
        &gate,
        permits.stop_file.clone(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(effects.wired());
    emit(
        "start",
        json!({"scenario": scenario.name, "room": effects.room_name, "duration_min": duration}),
    );

    // Not wrapped in a timeout: dropping the script between a grant and its
    // record would lose a spent permit. Every wait in it ends at `script_stop`.
    let result = run_script(&mut permits, scenario, &mut effects).await;
    gate.release();
    let cleanup = cleanup_end_if_needed(&mut permits, &mut effects, &result).await;
    let teardown = effects.teardown().await;
    let facts = effects.facts(&teardown);
    let problems = judge(scenario, &facts);
    let (ended_at, transient_leaves, receipts_exactly) = {
        let observed = effects.shared.observed.lock().unwrap();
        (
            observed.presence.ended_at().map(|at| at.as_secs_f64()),
            observed.presence.transient_leaves,
            observed.tally.exactly(scenario),
        )
    };
    emit(
        "result",
        json!({
            "scenario": scenario.name,
            "script": format!("{result:?}"),
            "cleanup_end": format!("{cleanup:?}"),
            "permits": permits.granted,
            "agent": format!("{:?}", facts.agent),
            "final_receipts": facts.final_copies,
            "receipts_exactly_as_declared": receipts_exactly,
            "distinct_final_reports": facts.final_distinct,
            "provisional_reports": facts.provisional,
            "ack_failures": facts.ack_failures,
            "gate_entered": facts.gate_entered,
            "end_while_pending": facts.end_while_pending,
            "agent_after_return": facts.agent_after_return,
            "transient_leaves": transient_leaves,
            "ended_at_s": ended_at,
            "deadline_elapsed_s": facts.deadline_elapsed_s,
            "absence_elapsed_s": facts.absence_elapsed_s,
            "participants_left": facts.participants_left,
            "room_gone_after_delete": facts.room_gone_after_delete,
            "loop_terminal": facts.terminal,
            "delete_after_gate": facts.delete_after_gate,
            "terminal_after_delete": facts.terminal_after_delete,
            "readers_unfinished": facts.readers_unfinished,
            "teardown_notes": teardown.notes,
            "problems": problems,
        }),
    );
    if let Err(error) = cleanup {
        eprintln!("cleanup End failed: {error:?}");
    }
    result.expect("the scripted live run completed");
    assert!(problems.is_empty(), "{problems:?}");
}

// Local tests. Everything below runs in the ordinary lane.

/// Where the fixture's scratch files go, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ct-live-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// How the in-test service answers `permit`. `status` is always answered
/// faithfully so a client can join and the fault is the permit's.
#[derive(Clone, Copy, Debug)]
enum Reply {
    Faithful,
    NotJson,
    WrongVersion,
    Oversized,
    HangsUp,
    Silent,
    GrantsAnotherAction,
    GrantsAnotherSequence,
    Denies,
    NoVerdict,
}

/// A faithful in-test copy of the launcher's decision and framing, over a Unix
/// socket. It is not the launcher: `launcher_*` below drives the real one.
struct Identity {
    run_id: String,
    digest: String,
}

struct Service {
    socket: PathBuf,
    manifest: PathBuf,
    identity: Identity,
    permits: Arc<AtomicUsize>,
    connections: Arc<AtomicUsize>,
    events: Arc<std::sync::Mutex<Vec<String>>>,
    _task: tokio::task::JoinHandle<()>,
}

/// Starts the service for `manifest`, written where the worker reads it, with
/// the digest the launcher would have computed over those bytes.
fn start_service(
    scratch: &Scratch,
    reply: Reply,
    allowed: &[&str],
    ceiling: usize,
    manifest: &Value,
) -> Service {
    let socket = scratch.path("permit.sock");
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let raw = serde_json::to_vec(manifest).unwrap();
    let manifest_path = scratch.path("run-manifest.json");
    std::fs::write(&manifest_path, &raw).unwrap();
    let identity = Identity {
        run_id: manifest["run_id"].as_str().unwrap().to_owned(),
        digest: crate::sha256_hex(&[&raw]),
    };
    let permits = Arc::new(AtomicUsize::new(0));
    let connections = Arc::new(AtomicUsize::new(0));
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let granted = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let allowed: Vec<String> = allowed.iter().map(|id| (*id).to_owned()).collect();
    let (run_id, digest) = (identity.run_id.clone(), identity.digest.clone());
    let task = tokio::spawn({
        let (permits, connections, events) = (
            std::sync::Arc::clone(&permits),
            std::sync::Arc::clone(&connections),
            std::sync::Arc::clone(&events),
        );
        async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                connections.fetch_add(1, Ordering::SeqCst);
                let (permits, events, granted) = (
                    std::sync::Arc::clone(&permits),
                    std::sync::Arc::clone(&events),
                    std::sync::Arc::clone(&granted),
                );
                let (allowed, run_id, digest) = (allowed.clone(), run_id.clone(), digest.clone());
                tokio::spawn(async move {
                    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
                    let mut line = String::new();
                    let mut reader = tokio::io::BufReader::new(&mut stream);
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                    let faithful = |request: &Value| {
                        let mut granted = granted.lock().unwrap();
                        if request["v"] != 1
                            || request["run_id"] != run_id
                            || request["manifest_sha256"] != digest
                        {
                            return json!({"v": 1, "granted": false, "reason": "run_identity_mismatch"});
                        }
                        match request["op"].as_str() {
                            Some("status") => {
                                json!({"v": 1, "ok": true, "phase": "open", "count": granted.len()})
                            }
                            Some("permit") => {
                                let action = request["action_id"].as_str().unwrap_or_default();
                                let seq = request["seq"].as_u64();
                                if !allowed.iter().any(|id| id == action) {
                                    json!({"v": 1, "granted": false, "reason": "unknown_action"})
                                } else if granted.iter().any(|id| id == action) {
                                    json!({"v": 1, "granted": false, "reason": "duplicate_action"})
                                } else if seq != Some(granted.len() as u64 + 1)
                                    || granted.len() + 1 > ceiling
                                {
                                    json!({"v": 1, "granted": false, "reason": "sequence_mismatch"})
                                } else {
                                    granted.push(action.to_owned());
                                    json!({"v": 1, "granted": true, "action_id": action, "seq": seq})
                                }
                            }
                            _ => json!({"v": 1, "granted": false, "reason": "malformed_request"}),
                        }
                    };
                    let op = request["op"].as_str().unwrap_or_default().to_owned();
                    if op == "permit" {
                        permits.fetch_add(1, Ordering::SeqCst);
                        events.lock().unwrap().push(format!(
                            "permit:{}",
                            request["action_id"].as_str().unwrap_or_default()
                        ));
                    }
                    let verdict = faithful(&request);
                    let body: Vec<u8> = if op != "permit" {
                        format!("{verdict}\n").into_bytes()
                    } else {
                        match reply {
                            Reply::Faithful => format!("{verdict}\n").into_bytes(),
                            Reply::NotJson => b"not json\n".to_vec(),
                            Reply::WrongVersion => b"{\"v\":2,\"granted\":true}\n".to_vec(),
                            Reply::Oversized => {
                                // A grant that would be exact if read whole.
                                let mut padded = verdict.clone();
                                padded["pad"] = json!("x".repeat(MAX_REPLY_BYTES + 1));
                                format!("{padded}\n").into_bytes()
                            }
                            Reply::HangsUp => return,
                            Reply::Silent => {
                                tokio::time::sleep(Duration::from_secs(5)).await;
                                return;
                            }
                            Reply::GrantsAnotherAction => {
                                let mut other = verdict.clone();
                                other["action_id"] = json!("another_action");
                                format!("{other}\n").into_bytes()
                            }
                            Reply::GrantsAnotherSequence => {
                                let mut other = verdict.clone();
                                other["seq"] = json!(99);
                                format!("{other}\n").into_bytes()
                            }
                            Reply::Denies => {
                                b"{\"v\":1,\"granted\":false,\"reason\":\"draining\"}\n".to_vec()
                            }
                            Reply::NoVerdict => b"{\"v\":1}\n".to_vec(),
                        }
                    };
                    let _ = stream.write_all(&body).await;
                });
            }
        }
    });
    Service {
        socket,
        manifest: manifest_path,
        identity,
        permits,
        connections,
        events,
        _task: task,
    }
}

/// The variables the launcher would give a worker for this service.
fn variables(service: &Service) -> impl Fn(&str) -> Option<String> + use<> {
    let values = std::collections::HashMap::from([
        (
            "BROWSER_CHECK_RUN_MANIFEST",
            service.manifest.to_str().unwrap().to_owned(),
        ),
        ("BROWSER_CHECK_RUN_ID", service.identity.run_id.clone()),
        (
            "BROWSER_CHECK_RUN_MANIFEST_SHA256",
            service.identity.digest.clone(),
        ),
        (
            "BROWSER_CHECK_BUDGET_SOCKET",
            service.socket.to_str().unwrap().to_owned(),
        ),
        (
            "BROWSER_CHECK_BUDGET_STOP",
            stop_file(service).to_str().unwrap().to_owned(),
        ),
    ]);
    move |name| values.get(name).cloned()
}

fn stop_file(service: &Service) -> PathBuf {
    service.manifest.with_file_name("stop")
}

/// Declared before running: the launcher's stop request is the stop file
/// appearing, and it is read, not assumed.
#[tokio::test]
async fn the_stop_request_is_the_launchers_stop_file() {
    let (_scratch, service, client) = joined(Reply::Faithful, &["candidate_join"]).await;
    assert!(!client.stop_requested());
    std::fs::write(stop_file(&service), "").unwrap();
    assert!(client.stop_requested());
}

fn simple_manifest(actions: &[&str]) -> Value {
    json!({"run_id": "run-test", "actions": actions})
}

async fn joined(reply: Reply, actions: &[&str]) -> (Scratch, Service, PermitClient) {
    let scratch = Scratch::new("permit");
    let mut allowed = actions.to_vec();
    allowed.push(FINAL_ACTION_ID);
    let manifest = simple_manifest(actions);
    let service = start_service(&scratch, reply, &allowed, actions.len() + 1, &manifest);
    let client = PermitClient::from_variables(variables(&service))
        .await
        .unwrap();
    (scratch, service, client)
}

/// Declared before running: every scripted action is granted once, in order,
/// at the next sequence number, and the reserved End is granted last.
#[tokio::test]
async fn the_permit_client_is_granted_each_scripted_action_once_in_order() {
    let actions = ["candidate_join", "mic_1", "edit_1"];
    let (_scratch, service, mut client) = joined(Reply::Faithful, &actions).await;
    for action in actions.iter().copied().chain([FINAL_ACTION_ID]) {
        client.permit(action).await.unwrap();
    }
    assert_eq!(client.granted, ["candidate_join", "mic_1", "edit_1", "end"]);
    assert_eq!(service.permits.load(Ordering::SeqCst), 4);

    // A granted action asked for again is refused without asking, and that
    // refusal kills the client.
    let before = service.connections.load(Ordering::SeqCst);
    assert!(matches!(
        client.permit("mic_1").await,
        Err(PermitError::Denied(_))
    ));
    assert!(matches!(
        client.permit("edit_1").await,
        Err(PermitError::Dead(_))
    ));
    assert_eq!(service.connections.load(Ordering::SeqCst), before);
}

/// Declared before running: a second client joining mid-run starts at the
/// launcher's count, so a restarted worker cannot spend an action again.
#[tokio::test]
async fn a_client_joining_mid_run_continues_from_the_launchers_count() {
    let actions = ["candidate_join", "mic_1", "edit_1"];
    let (_scratch, service, mut first) = joined(Reply::Faithful, &actions).await;
    first.permit("candidate_join").await.unwrap();
    first.permit("mic_1").await.unwrap();
    let mut second = PermitClient::from_variables(variables(&service))
        .await
        .unwrap();
    assert_eq!(second.next_seq, 3);
    // The action the first client already spent is refused by the service.
    assert!(second.permit("mic_1").await.is_err());
    let mut third = PermitClient::from_variables(variables(&service))
        .await
        .unwrap();
    third.permit("edit_1").await.unwrap();
}

/// Declared before running: each reply that is not an exact grant for this
/// action and sequence fails closed with no effect, the failure is not retried
/// (one permit connection), and the dead client then refuses without asking.
#[tokio::test]
async fn the_permit_client_fails_closed_on_every_reply_that_is_not_an_exact_grant() {
    for (reply, takes_at_least) in [
        (Reply::NotJson, Duration::ZERO),
        (Reply::WrongVersion, Duration::ZERO),
        (Reply::Oversized, Duration::ZERO),
        (Reply::HangsUp, Duration::ZERO),
        (Reply::GrantsAnotherAction, Duration::ZERO),
        (Reply::GrantsAnotherSequence, Duration::ZERO),
        (Reply::Denies, Duration::ZERO),
        (Reply::NoVerdict, Duration::ZERO),
        (Reply::Silent, PERMIT_TIMEOUT),
    ] {
        let actions = ["candidate_join", "mic_1"];
        let (_scratch, service, mut client) = joined(reply, &actions).await;
        let dispatched = AtomicUsize::new(0);
        let started = std::time::Instant::now();
        let outcome = client
            .guarded("candidate_join", async {
                dispatched.fetch_add(1, Ordering::SeqCst);
            })
            .await;
        assert!(outcome.is_err(), "{reply:?} was treated as granted");
        assert_eq!(dispatched.load(Ordering::SeqCst), 0, "{reply:?} dispatched");
        assert!(started.elapsed() >= takes_at_least, "{reply:?}");
        assert!(
            started.elapsed() < PERMIT_TIMEOUT + Duration::from_millis(900),
            "{reply:?} outlived the permit timeout: {:?}",
            started.elapsed()
        );
        assert_eq!(
            service.permits.load(Ordering::SeqCst),
            1,
            "{reply:?} retried"
        );
        let connections = service.connections.load(Ordering::SeqCst);
        assert!(client.permit("mic_1").await.is_err(), "{reply:?}");
        assert!(client.guarded("mic_1", async {}).await.is_err());
        assert_eq!(
            service.connections.load(Ordering::SeqCst),
            connections,
            "{reply:?}: a dead client contacted the launcher"
        );
        assert!(client.granted.is_empty());
    }
}

/// Declared before running: a launcher that is gone (no socket) fails closed
/// as a lost launcher, not a denial, and a duplicate action is refused by the
/// client before it asks.
#[tokio::test]
async fn an_unreachable_launcher_and_a_duplicate_action_are_refused() {
    let actions = ["candidate_join", "mic_1"];
    let (scratch, service, mut client) = joined(Reply::Faithful, &actions).await;
    client.permit("candidate_join").await.unwrap();
    let permits = service.permits.load(Ordering::SeqCst);
    assert!(matches!(
        client.permit("candidate_join").await,
        Err(PermitError::Denied(_))
    ));
    assert_eq!(service.permits.load(Ordering::SeqCst), permits);

    let gone = Scratch::new("gone");
    let mut stranded = client;
    stranded.dead = None;
    stranded.socket = gone.path("missing.sock");
    assert!(matches!(
        stranded.permit("mic_1").await,
        Err(PermitError::Lost(_))
    ));
    drop(scratch);
}

/// Declared before running: the variables and the manifest are refused unless
/// all four are present, the digest is the manifest's, and the run ID matches.
#[tokio::test]
async fn the_permit_client_refuses_an_incomplete_or_mismatched_environment() {
    let scratch = Scratch::new("env");
    let manifest = simple_manifest(&["candidate_join"]);
    let service = start_service(
        &scratch,
        Reply::Faithful,
        &["candidate_join", "end"],
        2,
        &manifest,
    );
    let complete = variables(&service);
    PermitClient::from_variables(&complete).await.unwrap();

    for name in BUDGET_VARIABLES {
        let result = PermitClient::from_variables(|asked| {
            (asked != name).then(|| complete(asked)).flatten()
        })
        .await;
        assert!(
            matches!(result, Err(PermitError::Setup(ref why)) if why.contains("incomplete")),
            "missing {name}"
        );
    }
    let digest = PermitClient::from_variables(|name| {
        if name == "BROWSER_CHECK_RUN_MANIFEST_SHA256" {
            Some("0".repeat(64))
        } else {
            complete(name)
        }
    })
    .await;
    assert!(matches!(digest, Err(PermitError::Setup(ref why)) if why.contains("hash")));
    let other_run = PermitClient::from_variables(|name| {
        if name == "BROWSER_CHECK_RUN_ID" {
            Some("run-other".into())
        } else {
            complete(name)
        }
    })
    .await;
    assert!(matches!(other_run, Err(PermitError::Setup(ref why)) if why.contains("run ID")));
}

/// Declared before running: the dispatch of a guarded action does not start
/// before its grant is received, and a refusal leaves it unstarted.
#[tokio::test]
async fn the_permit_is_taken_before_the_side_effect() {
    let (_scratch, service, mut client) = joined(Reply::Faithful, &["candidate_join"]).await;
    let events = std::sync::Arc::clone(&service.events);
    client
        .guarded("candidate_join", async {
            events.lock().unwrap().push("effect:candidate_join".into());
        })
        .await
        .unwrap();
    assert_eq!(
        *service.events.lock().unwrap(),
        ["permit:candidate_join", "effect:candidate_join"]
    );

    let (_scratch, service, mut denied) = joined(Reply::Denies, &["candidate_join"]).await;
    let events = std::sync::Arc::clone(&service.events);
    let outcome = denied
        .guarded("candidate_join", async {
            events.lock().unwrap().push("effect:candidate_join".into());
        })
        .await;
    assert!(outcome.is_err());
    assert_eq!(*service.events.lock().unwrap(), ["permit:candidate_join"]);
}

/// The launcher's own service, over a fake worker that is this test binary.
///
/// Declared before running: the real `scripts/interview-run-budget.py run`
/// starts this binary on the one test below that is the worker; the worker
/// reads the launcher's variables, takes each manifest action and then End
/// through `PermitClient`, and dispatches by appending to a log. The ledger the
/// launcher wrote and the dispatch log agree, in order, and the launcher
/// reports a passing budget.
#[test]
fn launcher_grants_exactly_what_the_native_worker_dispatches() {
    let scratch = Scratch::new("launcher");
    let outcome = run_launcher(&scratch, "normal", &["candidate_join", "mic_1", "edit_1"]);
    assert!(outcome.status.success(), "{}", outcome.stderr);
    let dispatched = std::fs::read_to_string(scratch.path("dispatch.log")).unwrap();
    assert_eq!(
        dispatched.split_whitespace().collect::<Vec<_>>(),
        ["candidate_join", "mic_1", "edit_1", "end"]
    );
    let ledger = std::fs::read_to_string(scratch.path("out/budget-ledger.jsonl")).unwrap();
    let granted: Vec<String> = ledger
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["event"] == "grant")
        .map(|event| event["action_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(granted, ["candidate_join", "mic_1", "edit_1", "end"]);
}

/// Declared before running: a worker that asks the launcher for an action it
/// already spent is refused as a duplicate, the launcher records the fault and
/// fails the run, and each action was dispatched once.
#[test]
fn launcher_refuses_and_records_a_repeated_action() {
    let scratch = Scratch::new("launcher-dup");
    let outcome = run_launcher(&scratch, "duplicate", &["candidate_join", "mic_1"]);
    assert!(!outcome.status.success(), "{}", outcome.stderr);
    let dispatched = std::fs::read_to_string(scratch.path("dispatch.log")).unwrap();
    assert_eq!(
        dispatched.split_whitespace().collect::<Vec<_>>(),
        ["candidate_join", "mic_1"]
    );
    let denied = std::fs::read_to_string(scratch.path("denied.log")).unwrap();
    assert!(
        denied.contains("\"granted\":false") && denied.contains("duplicate_action"),
        "{denied}"
    );
    let ledger = std::fs::read_to_string(scratch.path("out/budget-ledger.jsonl")).unwrap();
    assert!(
        ledger
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .any(|event| event["event"] == "fault" && event["reason"] == "duplicate_action"),
        "{ledger}"
    );
}

struct LauncherRun {
    status: std::process::ExitStatus,
    stderr: String,
}

fn child_test_name() -> String {
    // libtest names a test without its crate.
    let module = module_path!().split_once("::").unwrap().1;
    format!("{module}::permit_worker_child")
}

fn run_launcher(scratch: &Scratch, mode: &str, actions: &[&str]) -> LauncherRun {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let candidate = scratch.path("candidate");
    std::fs::write(&candidate, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = std::fs::metadata(&candidate).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&candidate, permissions).unwrap();
    let manifest = json!({
        "schema_version": 1,
        "run_id": "run-native-1",
        "binaries": {"candidate": {
            "path": candidate.to_str().unwrap(),
            "sha256": crate::sha256_hex(&[&std::fs::read(&candidate).unwrap()]),
        }},
        "settings_sha256": "a".repeat(64),
        "fixture_sha256": "a".repeat(64),
        "selected_duration_s": 10,
        "outer_wall_cap_s": 30,
        "cleanup_reserve_s": 4,
        "end_mode": "explicit_end",
        "actions": actions,
        "action_ceiling": actions.len() + 1,
        "limits": {},
        "scopes": {"scripted_actions": "hard", "wall_time": "hard"},
    });
    let manifest_path = scratch.path("manifest.json");
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let output = std::process::Command::new("python3")
        .arg(root.join("scripts/interview-run-budget.py"))
        .args(["run", "--manifest"])
        .arg(&manifest_path)
        .arg("--artifact-dir")
        .arg(scratch.path("out"))
        .args([
            "--allow-short-reserve",
            "1",
            "--sample-interval",
            "0.05",
            "--reap-budget",
            "1.5",
            "--",
        ])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", &child_test_name(), "--nocapture"])
        .env("CODETRIAL_PERMIT_WORKER_DIR", &scratch.0)
        .env("CODETRIAL_PERMIT_WORKER_MODE", mode)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("python3 runs the launcher");
    LauncherRun {
        status: output.status,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}
