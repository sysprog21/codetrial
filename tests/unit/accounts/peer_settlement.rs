use super::*;

fn fixture() -> (Accounts, Session) {
    let accounts = Accounts::open(crate::accounts::GitHubLoginConfig {
        oauth: None,
        session_secret: "settlement-test".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    accounts.initialize_schema().unwrap();
    accounts.with(|c| c.execute_batch("INSERT INTO users(id,github_id,login,created_at,updated_at) VALUES(1,1,'alice',1,1),(2,2,'bob',1,1)")).unwrap();
    let created = create(
        &accounts,
        1,
        Settings {
            topic: String::new(),
            difficulty: "easy".into(),
            language: "python".into(),
            round_minutes: 25,
            interviewer_first: 0,
            save_transcript: false,
            save_audio: false,
        },
        &crate::config::PeerConfig::default(),
        100,
    )
    .unwrap()
    .unwrap();
    accept(&accounts, 2, created["inviteToken"].as_str().unwrap(), 101)
        .unwrap()
        .unwrap();
    let session = accounts
        .with(|c| load(c, created["sessionId"].as_str().unwrap()))
        .unwrap()
        .unwrap();
    (accounts, session)
}

fn activate(session: &mut Session) {
    session.status = "active".into();
    session.current_round = Some(0);
    session.hard_deadline = Some(5500);
    session.rounds = vec![Round {
        index: 0,
        interviewer_slot: 0,
        candidate_slot: 1,
        problem_id: "two-sum".into(),
        status: "active".into(),
        version: 0,
        started_at: Some(100),
        deadline: Some(1600),
        finishing_at: None,
        sealed_at: None,
        writer_lease: None,
        writer_lease_until: 0,
        final_capture_incomplete: false,
    }];
}

#[test]
fn settlement_waits_thirty_seconds_for_final_capture_and_preserves_snapshot() {
    let (accounts, mut session) = fixture();
    activate(&mut session);
    accounts.with(|c| {
        save(c, &session)?;
        let lease = writer(&mut session, 1, 0, 1599)?.unwrap()["lease"].clone();
        let body = json!({"lease":lease,"expectedRevision":0,"language":"python","code":"return 42","tests":{"passed":1,"total":1,"cases":[]}});
        snapshot(c, &mut session, 1, 0, &body, 1599)?.unwrap();
        let version = session.version;
        settle(c, &mut session, 1599)?;
        assert_eq!(session.version, version);
        assert_eq!(session.rounds[0].status, "active");
        settle(c, &mut session, 1600)?;
        assert_eq!(session.rounds[0].status, "finishing");
        assert_eq!(session.version, version + 1, "a timed transition invalidates stale state");
        assert!(session.rounds[0].sealed_at.is_none());
        settle(c, &mut session, 1629)?;
        assert_eq!(session.rounds[0].status, "finishing");
        settle(c, &mut session, 1630)?;
        assert_eq!(session.rounds[0].status, "feedback");
        assert_eq!(session.rounds[0].sealed_at, Some(1630));
        assert!(session.rounds[0].final_capture_incomplete);
        assert_eq!(read_snapshot(c, &session, 0)?.unwrap()["code"], "return 42");
        assert_eq!(snapshot(c, &mut session, 1, 0, &body, 1630)?.unwrap_err(), "round_sealed");
        let settled = session.version;
        settle(c, &mut session, 1631)?;
        assert_eq!(session.version, settled, "idempotent settlement must not invalidate proposals");
        Ok(())
    }).unwrap();
}

#[test]
fn final_snapshot_seals_early_and_rejects_old_leases_and_revisions() {
    let (accounts, mut session) = fixture();
    activate(&mut session);
    accounts.with(|c| {
        save(c, &session)?;
        assert_eq!(writer(&mut session, 0, 0, 1590)?.unwrap_err(), "not_writer");
        let old = writer(&mut session, 1, 0, 1590)?.unwrap()["lease"].clone();
        let lease = writer(&mut session, 1, 0, 1591)?.unwrap()["lease"].clone();
        let mut body = json!({"lease":old,"expectedRevision":0,"language":"python","code":"first","tests":null});
        assert_eq!(snapshot(c, &mut session, 1, 0, &body, 1592)?.unwrap_err(), "lease_lost");
        body["lease"] = lease;
        assert_eq!(snapshot(c, &mut session, 0, 0, &body, 1592)?.unwrap_err(), "not_writer");
        assert_eq!(snapshot(c, &mut session, 1, 0, &body, 1592)?.unwrap()["revision"], 1);
        assert_eq!(snapshot(c, &mut session, 1, 0, &body, 1593)?.unwrap_err(), "version_conflict");
        settle(c, &mut session, 1600)?;
        body["expectedRevision"] = json!(1);
        assert_eq!(snapshot(c, &mut session, 1, 0, &body, 1601)?.unwrap_err(), "final_required");
        body["final"] = json!(true);
        body["code"] = json!("final");
        assert_eq!(snapshot(c, &mut session, 1, 0, &body, 1601)?.unwrap()["revision"], 2);
        assert_eq!(session.rounds[0].sealed_at, Some(1601));
        assert!(!session.rounds[0].final_capture_incomplete);
        assert_eq!(read_snapshot(c, &session, 0)?.unwrap()["code"], "final");
        Ok(())
    }).unwrap();
}

#[test]
fn hard_cutoff_and_pending_expiry_release_reservations_at_the_boundary() {
    let (accounts, mut session) = fixture();
    accounts
        .with(|c| {
            c.execute(
                "INSERT INTO peer_active_users(user_id,session_id) VALUES(1,?1),(2,?1)",
                [&session.id],
            )?;
            let expiry = session.expires_at;
            settle(c, &mut session, expiry - 1)?;
            assert_eq!(session.status, "pending");
            settle(c, &mut session, expiry)?;
            assert_eq!(session.status, "cancelled");
            assert_eq!(session.outcome.as_deref(), Some("no_show"));
            let count: i64 =
                c.query_row("SELECT count(*) FROM peer_active_users", [], |r| r.get(0))?;
            assert_eq!(count, 0);
            activate(&mut session);
            c.execute(
                "INSERT INTO peer_active_users(user_id,session_id) VALUES(1,?1),(2,?1)",
                [&session.id],
            )?;
            settle(c, &mut session, 5499)?;
            assert_eq!(session.status, "active");
            let held: i64 =
                c.query_row("SELECT count(*) FROM peer_active_users", [], |r| r.get(0))?;
            assert_eq!(held, 2, "a running session keeps its members reserved");
            settle(c, &mut session, 5500)?;
            assert_eq!(session.status, "finalizing");
            assert_eq!(session.outcome.as_deref(), Some("partial"));
            assert!(session.rounds[0].sealed_at.is_some());
            let count: i64 =
                c.query_row("SELECT count(*) FROM peer_active_users", [], |r| r.get(0))?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
}

#[test]
fn idle_lobby_requires_both_members_to_be_idle() {
    let (accounts, mut session) = fixture();
    session.status = "lobby".into();
    for member in &mut session.members {
        member.entered_at = Some(100);
        member.last_seen_at = Some(100);
        member.ready = true;
    }
    session.members[1].last_seen_at = Some(200);
    session.start_confirmed = [true, false];
    accounts
        .with(|c| {
            c.execute(
                "INSERT INTO peer_active_users(user_id,session_id) VALUES(1,?1),(2,?1)",
                [&session.id],
            )?;
            settle(c, &mut session, 799)?;
            assert_eq!(session.status, "lobby");
            settle(c, &mut session, 800)?;
            assert_eq!(session.status, "pending");
            assert!(
                session
                    .members
                    .iter()
                    .all(|m| m.entered_at.is_none() && !m.ready)
            );
            assert_eq!(session.start_confirmed, [false; 2]);
            let count: i64 =
                c.query_row("SELECT count(*) FROM peer_active_users", [], |r| r.get(0))?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
}

#[test]
fn feedback_reveals_submitted_partner_work_after_twenty_four_hours_only() {
    let (accounts, mut session) = fixture();
    activate(&mut session);
    session.rounds[0].status = "feedback".into();
    session.rounds[0].sealed_at = Some(1600);
    accounts.with(|c| {
        save(c, &session)?;
        c.execute("INSERT INTO peer_feedback(session_id,round_idx,author_slot,kind,rubric_version,payload_json,submitted_at) VALUES(?1,0,0,'candidate','peer-rubric-v1','{\"submitted\":true}',1700),(?1,0,1,'interviewer','peer-rubric-v1','{\"private\":true}',NULL)", [&session.id])?;
        let hidden = read_feedback(c, &session, 1, 0, 1600 + 86400 - 1)?.unwrap();
        assert_eq!(hidden["revealed"], false);
        assert_eq!(hidden["feedback"].as_array().unwrap().len(), 1);
        assert_eq!(hidden["feedback"][0]["authorSlot"], 1);
        let revealed = read_feedback(c, &session, 1, 0, 1600 + 86400)?.unwrap();
        assert_eq!(revealed["revealed"], true);
        assert_eq!(revealed["feedback"].as_array().unwrap().len(), 2);
        let partner = read_feedback(c, &session, 0, 0, 1600 + 86400)?.unwrap();
        assert_eq!(partner["feedback"].as_array().unwrap().len(), 1, "another member's unsubmitted draft remains private");
        c.execute("UPDATE peer_feedback SET submitted_at=1701 WHERE author_slot=1", [])?;
        let mutual = read_feedback(c, &session, 0, 0, 1701)?.unwrap();
        assert_eq!(mutual["revealed"], true);
        assert_eq!(mutual["feedback"].as_array().unwrap().len(), 2);
        Ok(())
    }).unwrap();
}

#[test]
fn lease_expiry_and_utf8_snapshot_size_are_enforced() {
    let (accounts, mut session) = fixture();
    activate(&mut session);
    accounts.with(|c| {
        save(c, &session)?;
        let lease = writer(&mut session, 1, 0, 100)?.unwrap()["lease"].clone();
        let mut body = json!({"lease":lease,"expectedRevision":0,"language":"python","code":"x","tests":null});
        assert_eq!(snapshot(c, &mut session, 1, 0, &body, 220)?.unwrap_err(), "lease_lost");
        let lease = writer(&mut session, 1, 0, 221)?.unwrap()["lease"].clone();
        body["lease"] = lease;
        // The byte width, rather than the character count, crosses the cap.
        body["code"] = json!("é".repeat(24 * 1024 + 1));
        assert_eq!(snapshot(c, &mut session, 1, 0, &body, 222)?.unwrap_err(), "invalid_snapshot");
        body["code"] = json!("é".repeat(24 * 1024));
        assert_eq!(snapshot(c, &mut session, 1, 0, &body, 222)?.unwrap()["revision"], 1);
        session.current_round = Some(1);
        assert_eq!(snapshot(c, &mut session, 1, 0, &body, 223)?.unwrap_err(), "round_unavailable");
        Ok(())
    }).unwrap();
}

#[test]
fn snapshot_test_summary_is_rebuilt_from_known_fields() {
    let (accounts, mut session) = fixture();
    activate(&mut session);
    accounts
        .with(|c| {
            save(c, &session)?;
            let lease = writer(&mut session, 1, 0, 100)?.unwrap()["lease"].clone();
            let case =
                json!({"label":"a","pass":false,"got":"1","expected":"2","timeMs":3,"extra":"<b>"});
            let mut body = json!({"lease":lease,"expectedRevision":0,"language":"python","code":"x",
            "tests":{"passed":0,"total":1,"cases":[case],"code":"dropped","at":1}});
            let saved = snapshot(c, &mut session, 1, 0, &body, 110)?.unwrap();
            assert_eq!(saved["revision"], 1);
            let stored = read_snapshot(c, &session, 0)?.unwrap();
            assert!(stored["tests"].get("code").is_none());
            assert!(stored["tests"]["cases"][0].get("extra").is_none());
            for bad in [
                json!({}),
                json!(5),
                json!({"passed":"<img>","total":1,"cases":[]}),
                json!({"passed":0,"total":1,"cases":5}),
                json!({"passed":0,"total":1,"cases":[null]}),
                json!({"passed":0,"total":1,"cases":[{"label":"a","pass":"yes","timeMs":1}]}),
            ] {
                body["expectedRevision"] = json!(1);
                body["tests"] = bad;
                assert_eq!(
                    snapshot(c, &mut session, 1, 0, &body, 111)?.unwrap_err(),
                    "invalid_snapshot"
                );
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn leaving_a_closed_session_is_harmless() {
    let (accounts, mut session) = fixture();
    accounts
        .with(|c| {
            save(c, &session)?;
            session.status = "complete".into();
            assert!(action(c, &mut session, 0, "leave", &json!({}), 130)?.is_ok());
            assert_eq!(
                action(c, &mut session, 0, "ready", &json!({}), 131)?.unwrap_err(),
                "session_closed"
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn the_swap_waits_for_the_members_own_round_zero_feedback() {
    let (accounts, mut session) = fixture();
    activate(&mut session);
    session.rounds[0].status = "feedback".into();
    session.rounds[0].sealed_at = Some(1600);
    accounts.with(|c| {
        save(c, &session)?;
        let body = json!({"expectedVersion":session.version});
        assert_eq!(action(c, &mut session, 0, "advance", &body, 1700)?.unwrap_err(), "feedback_required");
        let dimensions = json!({"clarification":{"score":"not_observed","example":"","improvement":""}});
        c.execute(
            "INSERT INTO peer_feedback (session_id,round_idx,author_slot,kind,rubric_version,payload_json,submitted_at) VALUES (?1,0,0,'candidate','peer-rubric-v1',?2,1700)",
            rusqlite::params![session.id, dimensions.to_string()],
        )?;
        action(c, &mut session, 0, "advance", &body, 1701)?.unwrap();
        Ok(())
    }).unwrap();
}

#[test]
fn feedback_stores_only_the_rubric_fields() {
    let (accounts, mut session) = fixture();
    activate(&mut session);
    session.rounds[0].status = "feedback".into();
    session.rounds[0].sealed_at = Some(1600);
    accounts
        .with(|c| {
            save(c, &session)?;
            let body = json!({"submitted":false,"extra":"x".repeat(1000),
            "dimensions":{"preparation":{"score":2,"example":"e","improvement":"i","extra":"y"}}});
            feedback(c, &session, 1, 0, &body, 1700)?.unwrap();
            let stored: String =
                c.query_row("SELECT payload_json FROM peer_feedback", [], |r| r.get(0))?;
            let stored: serde_json::Value = serde_json::from_str(&stored).unwrap();
            assert!(stored.get("extra").is_none());
            assert!(stored["dimensions"]["preparation"].get("extra").is_none());
            assert_eq!(stored["dimensions"]["preparation"]["score"], 2);
            Ok(())
        })
        .unwrap();
}

/// A rubric that was submitted is final: the upsert that saves a draft would
/// otherwise let it be replaced after the partner may already have read it.
#[test]
fn a_submitted_feedback_is_not_replaced() {
    let (accounts, mut session) = fixture();
    activate(&mut session);
    session.rounds[0].status = "feedback".into();
    session.rounds[0].sealed_at = Some(1600);
    accounts
        .with(|c| {
            save(c, &session)?;
            c.execute(
                "INSERT INTO peer_feedback(session_id,round_idx,author_slot,kind,rubric_version,payload_json,submitted_at)
                 VALUES(?1,0,1,'interviewer','peer-rubric-v1','{\"submitted\":true}',1700)",
                [&session.id],
            )?;
            let body = json!({"submitted":false,
                "dimensions":{"preparation":{"score":2,"example":"e","improvement":"i"}}});
            assert_eq!(
                feedback(c, &session, 1, 0, &body, 1800)?.unwrap_err(),
                "feedback_submitted"
            );
            let stored: String =
                c.query_row("SELECT payload_json FROM peer_feedback", [], |r| r.get(0))?;
            assert_eq!(stored, "{\"submitted\":true}");
            Ok(())
        })
        .unwrap();
}

#[test]
fn leaving_one_session_keeps_the_reservation_held_by_another() {
    let (accounts, mut session) = fixture();
    let user = session.members[0].user_id;
    // `create` takes the connection itself, so it runs before `with` holds it.
    let other = create(
        &accounts,
        2,
        session.settings.clone(),
        &crate::config::PeerConfig::default(),
        110,
    )
    .unwrap()
    .unwrap();
    accounts
        .with(|c| {
            save(c, &session)?;
            c.execute(
                "INSERT INTO peer_active_users(user_id,session_id) VALUES(?1,?2)",
                rusqlite::params![user, other["sessionId"].as_str().unwrap()],
            )?;
            action(c, &mut session, 0, "leave", &json!({}), 120)?.unwrap();
            let held: i64 = c.query_row(
                "SELECT count(*) FROM peer_active_users WHERE user_id=?1",
                [user],
                |r| r.get(0),
            )?;
            assert_eq!(held, 1, "only this session's reservation is released");
            Ok(())
        })
        .unwrap();
}

fn start(c: &Connection, session: &mut Session, slot: usize) -> rusqlite::Result<PeerResult<()>> {
    let body = json!({"expectedVersion":session.version,"observerConsent":true});
    action(c, session, slot, "start", &body, 200)
}

fn ready_lobby(session: &mut Session) {
    session.status = "lobby".into();
    for member in &mut session.members {
        member.ready = true;
        member.entered_at = Some(110);
    }
}

#[test]
fn starting_needs_both_members_present_and_ready() {
    let (accounts, mut session) = fixture();
    accounts
        .with(|c| {
            save(c, &session)?;
            ready_lobby(&mut session);
            session.members.truncate(1);
            assert_eq!(
                start(c, &mut session, 0)?.unwrap_err(),
                "waiting_for_partner"
            );
            assert!(
                !session.start_confirmed[0],
                "a refused start confirms nothing"
            );

            let mut session = session.clone();
            session.members.push(session.members[0].clone());
            session.members[1].slot = 1;
            session.members[1].ready = false;
            assert_eq!(
                start(c, &mut session, 0)?.unwrap_err(),
                "waiting_for_partner"
            );

            session.members[1].ready = true;
            session.members[1].entered_at = None;
            assert_eq!(
                start(c, &mut session, 0)?.unwrap_err(),
                "waiting_for_partner"
            );

            session.members[1].entered_at = Some(110);
            start(c, &mut session, 0)?.unwrap();
            assert_eq!(session.start_confirmed, [true, false]);
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_topic_with_exactly_two_problems_can_start_but_fewer_cannot() {
    let (accounts, session) = fixture();
    accounts
        .with(|c| {
            save(c, &session)?;
            for (topic, available, starts) in [
                ("Binary Search", 2, true),
                ("Memoization", 1, false),
                ("Nonexistent", 0, false),
            ] {
                let mut session = session.clone();
                ready_lobby(&mut session);
                session.settings.topic = topic.into();
                assert_eq!(
                    super::rounds::eligible_problems(&session.settings).len(),
                    available
                );
                start(c, &mut session, 0)?.unwrap();
                let second = start(c, &mut session, 1)?;
                if starts {
                    second.unwrap();
                    assert_eq!(session.rounds.len(), 2, "{topic}");
                    assert_eq!(session.current_round, Some(0));
                } else {
                    assert_eq!(second.unwrap_err(), "problems_unavailable", "{topic}");
                    assert!(session.rounds.is_empty());
                    assert_eq!(session.start_confirmed, [true, false], "{topic}");
                }
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn the_second_round_runs_for_the_configured_minutes_from_the_swap() {
    let (accounts, mut session) = fixture();
    activate(&mut session);
    let mut second = session.rounds[0].clone();
    second.index = 1;
    second.interviewer_slot = 1;
    second.candidate_slot = 0;
    second.status = "prepared".into();
    second.started_at = None;
    second.deadline = None;
    session.rounds.push(second);
    session.rounds[0].status = "feedback".into();
    session.rounds[0].sealed_at = Some(1600);
    accounts
        .with(|c| {
            save(c, &session)?;
            for slot in 0..2 {
                c.execute(
                    "INSERT INTO peer_feedback (session_id,round_idx,author_slot,kind,rubric_version,payload_json,submitted_at) VALUES (?1,0,?2,'candidate','peer-rubric-v1','{}',1700)",
                    rusqlite::params![session.id, slot],
                )?;
            }
            for slot in 0..2 {
                let body = json!({"expectedVersion":session.version});
                action(c, &mut session, slot, "advance", &body, 1700)?.unwrap();
            }
            assert_eq!(session.current_round, Some(1));
            assert_eq!(session.rounds[1].started_at, Some(1700));
            assert_eq!(session.rounds[1].deadline, Some(1700 + 25 * 60));
            Ok(())
        })
        .unwrap();
}

fn with_second_round(session: &mut Session) {
    activate(session);
    let mut second = session.rounds[0].clone();
    second.index = 1;
    second.status = "prepared".into();
    session.rounds.push(second);
}

#[test]
fn a_snapshot_is_readable_only_up_to_the_current_round() {
    let (accounts, mut session) = fixture();
    with_second_round(&mut session);
    accounts
        .with(|c| {
            let read = |s: &Session, index| read_snapshot(c, s, index).map(|r| r.err());
            assert_eq!(read(&session, 0)?, None);
            assert_eq!(read(&session, 1)?, Some("round_unavailable"));
            assert_eq!(read(&session, 2)?, Some("round_unavailable"));

            session.current_round = Some(1);
            assert_eq!(
                read(&session, 0)?,
                None,
                "the finished round stays readable"
            );
            assert_eq!(read(&session, 1)?, None);
            assert_eq!(read(&session, 2)?, Some("round_unavailable"));

            session.current_round = None;
            assert_eq!(read(&session, 0)?, Some("round_unavailable"));
            Ok(())
        })
        .unwrap();
}

#[test]
fn feedback_text_is_capped_at_fifteen_hundred_bytes_per_field() {
    let (accounts, mut session) = fixture();
    activate(&mut session);
    session.rounds[0].status = "feedback".into();
    session.rounds[0].sealed_at = Some(1600);
    accounts
        .with(|c| {
            save(c, &session)?;
            let submit = |example: usize, improvement: usize| {
                let field = json!({"score":2,"example":"e".repeat(example),"improvement":"i".repeat(improvement)});
                let body = json!({"submitted":false,"dimensions":{"preparation":field}});
                feedback(c, &session, 1, 0, &body, 1700).map(|r| r.err())
            };
            assert_eq!(submit(1500, 1500)?, None);
            assert_eq!(submit(1501, 1)?, Some("invalid_feedback"));
            assert_eq!(submit(1, 1501)?, Some("invalid_feedback"));
            Ok(())
        })
        .unwrap();
}

#[test]
fn test_summary_text_is_kept_cut_at_its_limit_on_a_character_boundary() {
    let (accounts, mut session) = fixture();
    activate(&mut session);
    accounts
        .with(|c| {
            save(c, &session)?;
            let lease = writer(&mut session, 1, 0, 100)?.unwrap()["lease"].clone();
            let mut save_tests = |tests: Value, revision: u64| {
                let body = json!({"lease":lease,"expectedRevision":revision,"language":"python","code":"x","tests":tests});
                snapshot(c, &mut session, 1, 0, &body, 110).map(|r| r.err())
            };
            let long = format!("a{}", "é".repeat(5000));
            let case = json!({"label":long,"pass":true,"timeMs":1,"input":"x".repeat(5000)});
            assert_eq!(
                save_tests(json!({"passed":1,"total":1,"setupError":long,"cases":[case]}), 0)?,
                None
            );
            assert_eq!(
                save_tests(json!({"passed":1,"total":1,"setupError":5,"cases":[]}), 1)?,
                Some("invalid_snapshot")
            );
            let stored = read_snapshot(c, &session, 0)?.unwrap();
            let tests = &stored["tests"];
            assert_eq!(tests["setupError"].as_str().unwrap().len(), 8 * 1024 - 1);
            assert_eq!(tests["cases"][0]["label"].as_str().unwrap().len(), 199);
            assert_eq!(tests["cases"][0]["input"].as_str().unwrap().len(), 4 * 1024);
            Ok(())
        })
        .unwrap();
}

#[test]
fn cancel_and_terminate_close_the_session_differently() {
    for (name, status) in [("cancel", "cancelled"), ("terminate", "finalizing")] {
        let (accounts, mut session) = fixture();
        activate(&mut session);
        accounts
            .with(|c| {
                save(c, &session)?;
                let body = json!({"expectedVersion":session.version});
                action(c, &mut session, 0, name, &body, 300)?.unwrap();
                assert_eq!(session.status, status);
                assert_eq!(session.outcome.as_deref(), Some("partial"));
                assert_eq!(session.rounds[0].sealed_at, Some(300));
                assert!(session.rounds[0].final_capture_incomplete);
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn a_heartbeat_records_presence_without_changing_the_version() {
    let (accounts, mut session) = fixture();
    accounts
        .with(|c| {
            save(c, &session)?;
            let version = session.version;
            action(c, &mut session, 1, "heartbeat", &json!({}), 300)?.unwrap();
            assert_eq!(session.members[1].last_seen_at, Some(300));
            assert_eq!(session.version, version);
            Ok(())
        })
        .unwrap();
}

#[test]
fn readiness_needs_both_members_present_and_no_round_running() {
    let (accounts, session) = fixture();
    accounts
        .with(|c| {
            save(c, &session)?;
            let ready = |s: &mut Session| {
                let body = json!({"expectedVersion":s.version,"ready":true});
                action(c, s, 0, "ready", &body, 300).map(|r| r.err())
            };
            let mut lone = session.clone();
            lone.members.truncate(1);
            lone.members[0].entered_at = Some(110);
            assert_eq!(ready(&mut lone)?, Some("waiting_for_partner"));

            let mut absent = session.clone();
            absent.members[0].entered_at = Some(110);
            assert_eq!(ready(&mut absent)?, Some("waiting_for_partner"));

            let mut running = session.clone();
            ready_lobby(&mut running);
            running.current_round = Some(0);
            assert_eq!(ready(&mut running)?, Some("waiting_for_partner"));

            let mut present = session.clone();
            ready_lobby(&mut present);
            present.members[0].ready = false;
            present.start_confirmed = [true, true];
            assert_eq!(ready(&mut present)?, None);
            assert!(present.members[0].ready);
            assert_eq!(present.start_confirmed, [false, false]);
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_snapshot_extends_the_lease_and_only_a_final_one_seals_a_finishing_round() {
    let (accounts, mut session) = fixture();
    activate(&mut session);
    accounts
        .with(|c| {
            save(c, &session)?;
            let lease = writer(&mut session, 1, 0, 100)?.unwrap()["lease"].clone();
            let mut body = json!({"lease":lease,"expectedRevision":0,"language":"python","code":"x","tests":null,"final":true});
            let version = session.version;
            let saved = snapshot(c, &mut session, 1, 0, &body, 110)?.unwrap();
            assert_eq!(saved["leaseUntil"], 230);
            assert_eq!(session.rounds[0].writer_lease_until, 230);
            assert_eq!(session.rounds[0].status, "active", "a final save on a running round seals nothing");
            assert_eq!(session.version, version);

            session.rounds[0].status = "finishing".into();
            body["expectedRevision"] = json!(1);
            snapshot(c, &mut session, 1, 0, &body, 111)?.unwrap();
            assert_eq!(session.rounds[0].status, "feedback");
            assert_eq!(session.rounds[0].sealed_at, Some(111));
            assert!(!session.rounds[0].final_capture_incomplete);
            assert_eq!(session.version, version + 1);
            Ok(())
        })
        .unwrap();
}

#[test]
fn feedback_requires_known_dimensions_valid_scores_and_a_full_form_to_submit() {
    let (accounts, mut session) = fixture();
    activate(&mut session);
    session.rounds[0].status = "feedback".into();
    session.rounds[0].sealed_at = Some(1600);
    accounts
        .with(|c| {
            save(c, &session)?;
            let field = |score: Value| json!({"score":score,"example":"e","improvement":"i"});
            let send = |dimensions: Value, submitted: bool| {
                let body = json!({"submitted":submitted,"dimensions":dimensions});
                feedback(c, &session, 1, 0, &body, 1700).map(|r| r.err())
            };
            let unknown = json!({"nonsense":field(json!(2))});
            assert_eq!(send(unknown, false)?, Some("invalid_feedback"));
            let partial = json!({"preparation":field(json!(2))});
            assert_eq!(send(partial, true)?, Some("invalid_feedback"));
            for bad in [json!(0), json!(5), json!("great"), json!(null)] {
                let one = json!({"preparation":field(bad.clone())});
                assert_eq!(send(one, false)?, Some("invalid_feedback"), "{bad}");
            }
            for good in [json!(1), json!(4), json!("not_observed")] {
                let one = json!({"preparation":field(good.clone())});
                assert_eq!(send(one, false)?, None, "{good}");
            }
            let full: serde_json::Map<String, Value> = [
                "preparation",
                "questionQuality",
                "hintPacing",
                "followUp",
                "feedbackSpecificity",
            ]
            .iter()
            .map(|name| ((*name).to_owned(), field(json!(3))))
            .collect();
            assert_eq!(send(Value::Object(full), true)?, None);
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_change_made_inside_a_transaction_is_stored_and_an_untouched_session_is_not() {
    let (accounts, session) = fixture();
    let version = session.version;
    let touched = transact(&accounts, &session.id, 1, 102, |_, s, _| {
        s.status = "cancelled".into();
        Ok(Ok(()))
    })
    .unwrap();
    assert!(touched.is_ok());
    let stored = accounts.with(|c| load(c, &session.id)).unwrap().unwrap();
    assert_eq!(stored.status, "cancelled");
    let (accounts, session) = fixture();
    transact(&accounts, &session.id, 1, 102, |c, _, _| {
        c.execute(
            "UPDATE peer_sessions SET status='cancelled' WHERE id=?1",
            [&session.id],
        )?;
        Ok(Ok(()))
    })
    .unwrap()
    .unwrap();
    let column: String = accounts
        .with(|c| {
            c.query_row(
                "SELECT status FROM peer_sessions WHERE id=?1",
                [&session.id],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(
        column, "cancelled",
        "an untouched session is not written back over a change made in the transaction"
    );
    let stored = accounts.with(|c| load(c, &session.id)).unwrap().unwrap();
    assert_eq!(stored.version, version);
}

#[test]
fn a_problem_either_member_met_in_either_role_is_chosen_last() {
    let (accounts, mut session) = fixture();
    accounts
        .with(|c| {
            save(c, &session)?;
            let all = super::rounds::eligible_problems(&session.settings);
            assert!(all.len() > 3);
            for (user, kind, problem) in [
                (1, "candidate", all[0].id),
                (2, "interviewer", all[1].id),
                (2, "candidate", all[2].id),
            ] {
                c.execute(
                    "INSERT INTO peer_problem_exposures (user_id,problem_id,kind,at) VALUES (?1,?2,?3,1)",
                    params![user, problem, kind],
                )?;
            }
            ready_lobby(&mut session);
            start(c, &mut session, 0)?.unwrap();
            start(c, &mut session, 1)?.unwrap();
            let met = [all[0].id, all[1].id, all[2].id];
            for round in &session.rounds {
                assert!(!met.contains(&round.problem_id.as_str()), "{}", round.problem_id);
            }
            Ok(())
        })
        .unwrap();
}
