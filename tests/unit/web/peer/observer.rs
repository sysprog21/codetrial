use super::*;
use serde_json::Value;

fn fixture() -> (Accounts, Session) {
    let accounts = Accounts::open(crate::accounts::GitHubLoginConfig {
        oauth: None,
        session_secret: "observer-tests".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    accounts.initialize_schema().unwrap();
    accounts.with(|c|c.execute_batch("INSERT INTO users(id,github_id,login,created_at,updated_at) VALUES(1,1,'alice',1,1),(2,2,'bob',1,1),(3,3,'eve',1,1)")).unwrap();
    let settings=serde_json::from_value(json!({"topic":"","difficulty":"easy","language":"python","roundMinutes":25,"interviewerFirst":0,"saveTranscript":false,"saveAudio":false})).unwrap();
    let created = peer::create(
        &accounts,
        1,
        settings,
        &crate::config::PeerConfig::default(),
        100,
    )
    .unwrap()
    .unwrap();
    peer::accept(&accounts, 2, created["inviteToken"].as_str().unwrap(), 101)
        .unwrap()
        .unwrap();
    let mut s = accounts
        .with(|c| peer::load(c, created["sessionId"].as_str().unwrap()))
        .unwrap()
        .unwrap();
    s.rounds = vec![peer::Round {
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
    (accounts, s)
}

#[test]
fn observer_accepts_only_fixed_identities_after_both_members_consented() {
    let (_, mut s) = fixture();
    let identity = s.members[0].identity.clone();
    assert_eq!(member_slot(&s, &identity), None);
    s.observer_consented = [true, false];
    assert_eq!(member_slot(&s, &identity), None);
    s.observer_consented = [true, true];
    assert_eq!(member_slot(&s, &identity), Some(0));
    assert_eq!(member_slot(&s, "self-declared-payload-identity"), None);
    assert_eq!(member_slot(&s, &s.members[1].identity), Some(1));
}

#[test]
fn transcription_wav_contains_only_the_supplied_mono_samples() {
    let wav = capture::wav(&[1, -2, i16::MAX]);
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16000);
    assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
    assert_eq!(&wav[44..], &[1, 0, 254, 255, 255, 127]);
}

#[test]
fn advice_requires_server_roles_exact_transcript_evidence_and_permitted_narrative() {
    let (_, s) = fixture();
    let segments = vec![record::Segment {
        slot: 1,
        round_index: Some(0),
        started_at: 100,
        ended_at: 110,
        text: "I will check the empty array before indexing it".into(),
    }];
    let valid = json!({"advice":[{"roundIndex":0,"role":"candidate","strengths":["Considered an empty input"],"improvements":[],"evidence":["check the empty array"]}]});
    assert_eq!(
        advice::validate(&valid, &s, 1, &segments).unwrap()[0]["slot"],
        1
    );
    let mut invalid = valid.clone();
    invalid["advice"][0]["role"] = json!("interviewer");
    assert!(advice::validate(&invalid, &s, 1, &segments).is_err());
    invalid = valid.clone();
    invalid["advice"][0]["evidence"] = json!(["invented quotation"]);
    assert!(advice::validate(&invalid, &s, 1, &segments).is_err());
    invalid = valid.clone();
    invalid["advice"][0]["evidence"] = json!([]);
    assert!(advice::validate(&invalid, &s, 1, &segments).is_err());
    invalid = valid;
    invalid["advice"][0]["improvements"] = json!(["Speak more confidently and avoid filler words"]);
    assert!(advice::validate(&invalid, &s, 1, &segments).is_err());
}

#[test]
fn advice_evidence_needs_a_real_excerpt_and_copy_checks_match_whole_words() {
    let (_, s) = fixture();
    let segments = vec![record::Segment {
        slot: 1,
        round_index: Some(0),
        started_at: 100,
        ended_at: 110,
        text: "unthe quick brown fox jumps over it and check the empty array".into(),
    }];
    let advice_of = |improvement: &str, evidence: &str| json!({"advice":[{"roundIndex":0,"role":"candidate","strengths":[],"improvements":[improvement],"evidence":[evidence]}]});
    let short = advice_of("Considered an empty input", "array");
    assert_eq!(
        advice::validate(&short, &s, 1, &segments).unwrap_err(),
        "ungrounded_advice"
    );
    let inside_a_word = advice_of(
        "Explain the quick brown fox jumps over",
        "check the empty array",
    );
    assert!(advice::validate(&inside_a_word, &s, 1, &segments).is_ok());
    let copied = advice_of(
        "Explain quick brown fox jumps over it",
        "check the empty array",
    );
    assert_eq!(
        advice::validate(&copied, &s, 1, &segments).unwrap_err(),
        "verbatim_advice"
    );
}

#[test]
fn transient_transcripts_never_persist_and_advice_remains_member_scoped_and_end_only() {
    let (accounts, mut s) = fixture();
    accounts.with(|c| peer::save(c, &s)).unwrap();
    record::begin(&accounts, &s.id, 100).unwrap();
    let segment = record::Segment {
        slot: 0,
        round_index: Some(0),
        started_at: 100,
        ended_at: 110,
        text: "technical discussion".into(),
    };
    record::append(&accounts, &s.id, 0, &segment).unwrap();
    let count: i64 = accounts
        .with(|c| c.query_row("SELECT count(*) FROM peer_transcripts", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        record::view_at(&accounts, &s.id, 1, true, 111)
            .unwrap()
            .unwrap_err(),
        "not_found"
    );
    let items = json!([{"slot":0,"roundIndex":0,"role":"interviewer","strengths":["Asked about edge cases"],"improvements":[],"evidence":["technical discussion"]},{"slot":1,"roundIndex":0,"role":"candidate","strengths":[],"improvements":[],"evidence":[]}]);
    record::finish(&accounts, &s.id, &items, None, 110).unwrap();
    assert!(
        record::view_at(&accounts, &s.id, 1, false, 111)
            .unwrap()
            .unwrap()["advice"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    s.status = "complete".into();
    accounts.with(|c| peer::save(c, &s)).unwrap();
    let view = record::view_at(&accounts, &s.id, 1, false, 111)
        .unwrap()
        .unwrap();
    assert_eq!(view["advice"].as_array().unwrap().len(), 1);
    assert_eq!(view["advice"][0]["slot"], 0);
    assert_eq!(
        record::view_at(&accounts, &s.id, 3, false, 111)
            .unwrap()
            .unwrap_err(),
        "not_found"
    );
    s.settings.save_transcript = true;
    accounts.with(|c| peer::save(c, &s)).unwrap();
    record::append(&accounts, &s.id, 0, &segment).unwrap();
    assert_eq!(
        record::view_at(&accounts, &s.id, 2, true, 111)
            .unwrap()
            .unwrap()["segments"][0]["text"],
        "technical discussion"
    );
}

#[test]
fn saved_transcript_withdrawal_prevents_later_persistence_and_expiry_refuses_reads_before_sweep() {
    let (accounts, mut s) = fixture();
    s.settings.save_transcript = true;
    s.status = "complete".into();
    accounts.with(|c| peer::save(c, &s)).unwrap();
    record::begin(&accounts, &s.id, 100).unwrap();
    let segment = record::Segment {
        slot: 0,
        round_index: Some(0),
        started_at: 100,
        ended_at: 110,
        text: "saved words".into(),
    };
    record::append(&accounts, &s.id, 0, &segment).unwrap();
    assert_eq!(
        record::withdraw(&accounts, &s.id, 3, 111)
            .unwrap()
            .unwrap_err(),
        "not_found"
    );
    assert_eq!(
        record::view_at(&accounts, &s.id, 2, true, 111)
            .unwrap()
            .unwrap()["segments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        record::withdraw(&accounts, &s.id, 1, 111).unwrap().unwrap()["withdrawn"],
        true
    );
    record::append(&accounts, &s.id, 1, &segment).unwrap();
    let count: i64 = accounts
        .with(|c| c.query_row("SELECT count(*) FROM peer_transcripts", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        record::view_at(&accounts, &s.id, 2, true, 112)
            .unwrap()
            .unwrap_err(),
        "not_found"
    );
    record::finish(&accounts, &s.id, &json!([]), None, 200).unwrap();
    assert!(
        record::view_at(&accounts, &s.id, 1, false, 200 + 86400 - 1)
            .unwrap()
            .is_ok()
    );
    assert_eq!(
        record::view_at(&accounts, &s.id, 1, false, 200 + 86400)
            .unwrap()
            .unwrap_err(),
        "not_found"
    );
    record::sweep(&accounts, 200 + 86400).unwrap();
    let count: i64 = accounts
        .with(|c| c.query_row("SELECT count(*) FROM peer_observations", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn dropping_observers_cancels_owned_provider_work() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Cancelled(Arc<AtomicBool>);
    impl Drop for Cancelled {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    let started = Arc::new(tokio::sync::Notify::new());
    let task_started = Arc::clone(&started);
    let observers = PeerObservers::new();
    let task = tokio::spawn(async move {
        let _cancelled = Cancelled(flag);
        task_started.notify_one();
        std::future::pending::<()>().await;
    });
    let (stop, _) = watch::channel(false);
    let (_, ready) = watch::channel(None);
    observers.tasks.lock().unwrap().insert(
        "session".into(),
        Actor {
            task,
            stop,
            ready,
            audio: Arc::new(Mutex::new(None)),
        },
    );
    started.notified().await;
    drop(observers);
    tokio::time::timeout(Duration::from_secs(1), async {
        while !cancelled.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn observer_advice_provider_runs_only_after_session_end_and_never_requests_speech() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let recorded = Arc::clone(&calls);
    let app=axum::Router::new().fallback(move|axum::Json(request):axum::Json<Value>|{
        let recorded=Arc::clone(&recorded);
        async move {
            recorded.fetch_add(1,Ordering::SeqCst);
            assert_eq!(request["generationConfig"]["responseModalities"],json!(["TEXT"]));
            assert!(request["generationConfig"].get("speechConfig").is_none());
            let prompt=request["contents"][0]["parts"][0]["text"].as_str().unwrap();
            let role=if prompt.contains("\"memberSlot\":0"){"interviewer"}else{"candidate"};
            let advice=json!({"advice":[{"roundIndex":0,"role":role,"strengths":["Discussed an input boundary"],"improvements":[],"evidence":["empty array"]}]}).to_string();
            axum::Json(json!({"candidates":[{"content":{"parts":[{"text":advice}]}}]}))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/generateContent", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut config = crate::config::load_peer_observer(&std::collections::BTreeMap::from([(
        "GOOGLE_API_KEY".into(),
        "observer-test-key".into(),
    )]))
    .unwrap()
    .unwrap();
    config.endpoint = Some(endpoint);
    let (_, mut s) = fixture();
    s.status = "active".into();
    let segments = vec![record::Segment {
        slot: 1,
        round_index: Some(0),
        started_at: 100,
        ended_at: 110,
        text: "Check the empty array".into(),
    }];
    assert_eq!(
        advice::generate(&config, &s, &segments, false)
            .await
            .unwrap_err(),
        "session_active"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    s.status = "complete".into();
    let result = advice::generate(&config, &s, &segments, false)
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(result.as_array().unwrap().len(), 2);
    assert_eq!(result[0]["slot"], 0);
    assert_eq!(result[1]["slot"], 1);
    server.abort();
}

#[test]
fn unsaved_advice_rejects_transcript_copies_but_allows_short_technical_terms() {
    let (_, mut session) = fixture();
    let spoken = "Use binary search on the sorted array";
    let mut segment = record::Segment {
        slot: 1,
        round_index: Some(0),
        started_at: 100,
        ended_at: 110,
        text: spoken.into(),
    };
    let mut response = json!({"advice":[{"roundIndex":0,"role":"candidate","strengths":["Use binary search on the sorted array"],"improvements":[],"evidence":[spoken]}]});
    assert_eq!(
        advice::validate(&response, &session, 1, &[segment.clone()]),
        Err("verbatim_advice")
    );
    response["advice"][0]["strengths"] =
        json!(["Explained: USE binary SEARCH, on the sorted array."]);
    assert_eq!(
        advice::validate(&response, &session, 1, &[segment.clone()]),
        Err("verbatim_advice")
    );
    response["advice"][0]["strengths"] = json!([]);
    response["advice"][0]["improvements"] = json!([spoken]);
    assert_eq!(
        advice::validate(&response, &session, 1, &[segment.clone()]),
        Err("verbatim_advice")
    );
    response["advice"][0]["improvements"] = json!([]);
    response["advice"][0]["strengths"] = json!(["Applied binary search appropriately"]);
    assert!(advice::validate(&response, &session, 1, &[segment.clone()]).is_ok());
    session.settings.save_transcript = true;
    response["advice"][0]["strengths"] = json!([spoken]);
    assert_eq!(
        advice::validate(&response, &session, 1, &[segment.clone()]),
        Err("verbatim_advice")
    );
    session.settings.save_transcript = false;
    let unspaced: String = (0x4e00..0x4e28).filter_map(char::from_u32).collect();
    segment.text = unspaced.clone();
    response["advice"][0]["evidence"] = json!([unspaced]);
    response["advice"][0]["strengths"] = json!([format!("Observed: {}", segment.text)]);
    assert_eq!(
        advice::validate(&response, &session, 1, &[segment]),
        Err("verbatim_advice")
    );
}

#[test]
fn advice_rejects_copied_phrases_across_ordered_transcript_segments() {
    let (_, session) = fixture();
    let mut segments = [
        record::Segment {
            slot: 1,
            round_index: Some(0),
            started_at: 100,
            ended_at: 110,
            text: "I used the hash map".into(),
        },
        record::Segment {
            slot: 1,
            round_index: Some(0),
            started_at: 110,
            ended_at: 120,
            text: "to find each matching value".into(),
        },
    ];
    let response = json!({"advice":[{"roundIndex":0,"role":"candidate","strengths":["I used the hash map to find each matching value"],"improvements":[],"evidence":["I used the hash map"]}]});
    assert_eq!(
        advice::validate(&response, &session, 1, &segments),
        Err("verbatim_advice")
    );
    segments.reverse();
    assert_eq!(
        advice::validate(&response, &session, 1, &segments),
        Err("verbatim_advice")
    );
    let unspaced: String = (0x4e00..0x4e28).filter_map(char::from_u32).collect();
    segments[1].text = unspaced.chars().take(20).collect();
    segments[0].text = unspaced.chars().skip(20).collect();
    let response = json!({"advice":[{"roundIndex":0,"role":"candidate","strengths":[unspaced],"improvements":[],"evidence":[segments[1].text]}]});
    assert_eq!(
        advice::validate(&response, &session, 1, &segments),
        Err("verbatim_advice")
    );
}

#[test]
fn unsaved_spoken_excerpts_are_not_retained_inside_advice_json() {
    let (accounts, mut s) = fixture();
    s.status = "complete".into();
    accounts.with(|c| peer::save(c, &s)).unwrap();
    record::begin(&accounts, &s.id, 100).unwrap();
    let spoken = "PRIVATE-SPOKEN-FIXTURE-e95f";
    let segment = record::Segment {
        slot: 1,
        round_index: Some(0),
        started_at: 100,
        ended_at: 110,
        text: spoken.into(),
    };
    record::append(&accounts, &s.id, 0, &segment).unwrap();
    let response = json!({"advice":[{"roundIndex":0,"role":"candidate","strengths":["Discussed the edge case"],"improvements":[],"evidence":[spoken],"unexpectedRawTranscript":spoken}]});
    let retained = Value::Array(advice::validate(&response, &s, 1, &[segment]).unwrap());
    assert!(!retained.to_string().contains(spoken));
    assert!(retained[0].get("evidence").is_none());
    assert_eq!(retained[0]["evidenceRefs"][0]["segmentIndex"], 0);
    record::finish(&accounts, &s.id, &retained, None, 110).unwrap();
    let stored: String = accounts
        .with(|c| {
            c.query_row(
                "SELECT advice_json FROM peer_observations WHERE session_id=?1",
                [&s.id],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert!(!stored.contains(spoken));
    let count: i64 = accounts
        .with(|c| c.query_row("SELECT count(*) FROM peer_transcripts", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(count, 0);
    let unsanitized = json!([{"slot":1,"roundIndex":0,"role":"candidate","strengths":[],"improvements":[],"evidence":[spoken],"rawTranscript":spoken}]);
    record::finish(&accounts, &s.id, &unsanitized, None, 111).unwrap();
    let stored: String = accounts
        .with(|c| {
            c.query_row(
                "SELECT advice_json FROM peer_observations WHERE session_id=?1",
                [&s.id],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert!(!stored.contains(spoken));
}

#[test]
fn recovery_leaves_fresh_observers_alone_and_expires_a_stale_lease_once() {
    let (accounts, s) = fixture();
    accounts.with(|c| peer::save(c, &s)).unwrap();
    record::begin(&accounts, &s.id, 100).unwrap();
    record::recover(&accounts, 219).unwrap();
    assert_eq!(
        record::view_at(&accounts, &s.id, 1, false, 219)
            .unwrap()
            .unwrap()["status"],
        "observing"
    );
    record::heartbeat(&accounts, &s.id, 200).unwrap();
    record::recover(&accounts, 300).unwrap();
    assert_eq!(
        record::view_at(&accounts, &s.id, 1, false, 300)
            .unwrap()
            .unwrap()["status"],
        "observing"
    );
    record::recover(&accounts, 320).unwrap();
    let view = record::view_at(&accounts, &s.id, 1, false, 320)
        .unwrap()
        .unwrap();
    assert_eq!(view["status"], "unavailable");
    assert_eq!(view["error"], "observer_interrupted");
    assert!(view["advice"].as_array().unwrap().is_empty());
    let before: i64 = accounts
        .with(|c| {
            c.query_row(
                "SELECT expires_at FROM peer_observations WHERE session_id=?1",
                [&s.id],
                |r| r.get(0),
            )
        })
        .unwrap();
    record::recover(&accounts, 400).unwrap();
    let after: i64 = accounts
        .with(|c| {
            c.query_row(
                "SELECT expires_at FROM peer_observations WHERE session_id=?1",
                [&s.id],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn observer_reads_settle_the_hard_deadline_before_capture_timeout_can_discard_the_session() {
    let (accounts, mut s) = fixture();
    s.status = "active".into();
    s.current_round = Some(0);
    s.hard_deadline = Some(200);
    s.rounds[0].deadline = Some(300);
    accounts.with(|c| peer::save(c, &s)).unwrap();
    let before = settled_session(&accounts, &s.id, 199).unwrap().unwrap();
    assert_eq!(before.status, "active");
    assert!(before.rounds[0].sealed_at.is_none());
    let closed = settled_session(&accounts, &s.id, 200).unwrap().unwrap();
    assert_eq!(closed.status, "finalizing");
    assert!(closed.closed());
    assert!(closed.rounds[0].sealed_at.is_some());
    let stored = accounts.with(|c| peer::load(c, &s.id)).unwrap().unwrap();
    assert_eq!(stored.status, "finalizing");
    assert_eq!(stored.version, closed.version);
    let repeated = settled_session(&accounts, &s.id, 201).unwrap().unwrap();
    assert_eq!(repeated.version, closed.version);
}

#[test]
fn a_chunk_straddling_the_hard_deadline_keeps_only_its_authorized_prefix() {
    let mut chunk = capture::Chunk {
        slot: 1,
        elapsed_ms: 29999,
        samples: (0..32).collect(),
    };
    capture::clip_to_deadline(&mut chunk, 30000);
    assert_eq!(chunk.samples, (0..16).collect::<Vec<_>>());
    chunk.elapsed_ms = 30000;
    capture::clip_to_deadline(&mut chunk, 30000);
    assert!(chunk.samples.is_empty());
    let mut before = capture::Chunk {
        slot: 0,
        elapsed_ms: 0,
        samples: vec![1, 2, 3],
    };
    capture::clip_to_deadline(&mut before, 30000);
    assert_eq!(before.samples, vec![1, 2, 3]);
}

#[test]
fn recovery_never_extends_or_revives_an_already_expired_observation() {
    let (accounts, s) = fixture();
    record::begin(&accounts, &s.id, 100).unwrap();
    accounts
        .with(|c| {
            c.execute(
                "UPDATE peer_observations SET expires_at=200,last_seen_at=0 WHERE session_id=?1",
                [&s.id],
            )
        })
        .unwrap();
    record::recover(&accounts, 201).unwrap();
    let expiry = accounts
        .with(|c| {
            c.query_row(
                "SELECT expires_at FROM peer_observations WHERE session_id=?1",
                [&s.id],
                |r| r.get::<_, i64>(0),
            )
        })
        .unwrap();
    assert_eq!(
        expiry, 200,
        "recovery cannot grant expired conversation a new retention window"
    );
    record::sweep(&accounts, 201).unwrap();
    assert_eq!(
        accounts
            .with(|c| c.query_row(
                "SELECT count(*) FROM peer_observations WHERE session_id=?1",
                [&s.id],
                |r| r.get::<_, i64>(0)
            ))
            .unwrap(),
        0
    );
}

#[test]
fn transcript_withdrawn_before_the_observer_begins_is_still_honored() {
    let (accounts, mut s) = fixture();
    s.settings.save_transcript = true;
    s.status = "active".into();
    accounts.with(|c| peer::save(c, &s)).unwrap();
    assert_eq!(
        record::withdraw(&accounts, &s.id, 1, 100).unwrap().unwrap()["withdrawn"],
        true
    );
    record::begin(&accounts, &s.id, 101).unwrap();
    let segment = record::Segment {
        slot: 0,
        round_index: Some(0),
        started_at: 101,
        ended_at: 110,
        text: "late words".into(),
    };
    record::append(&accounts, &s.id, 0, &segment).unwrap();
    let count: i64 = accounts
        .with(|c| c.query_row("SELECT count(*) FROM peer_transcripts", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn a_withdrawal_before_capture_outlives_recovery_and_the_sweep() {
    let (accounts, mut s) = fixture();
    s.settings.save_transcript = true;
    s.status = "pending".into();
    accounts.with(|c| peer::save(c, &s)).unwrap();
    record::withdraw(&accounts, &s.id, 1, 100).unwrap().unwrap();

    // No observer ever ran, so nothing refreshed the row. Recovery must not
    // shorten it the way it shortens a capture that died.
    record::recover(&accounts, 400).unwrap();
    record::sweep(&accounts, 90_000).unwrap();
    record::begin(&accounts, &s.id, 90_001).unwrap();
    let segment = record::Segment {
        slot: 0,
        round_index: Some(0),
        started_at: 90_001,
        ended_at: 90_010,
        text: "words said after the withdrawal".into(),
    };
    record::append(&accounts, &s.id, 0, &segment).unwrap();
    let count: i64 = accounts
        .with(|c| c.query_row("SELECT count(*) FROM peer_transcripts", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(count, 0, "a later start must still find the withdrawal");
}

#[test]
fn spoken_words_in_evidence_do_not_void_the_advice() {
    let (_, s) = fixture();
    let segments = vec![record::Segment {
        slot: 1,
        round_index: Some(0),
        started_at: 100,
        ended_at: 110,
        text: "The accent mark in this string needs escaping".into(),
    }];

    // Evidence is only checked against the transcript and never shown, so a
    // quoted word the narrative filter refuses is not a reason to drop advice.
    let advice = json!({"advice":[{"roundIndex":0,"role":"candidate","strengths":["Noticed an escaping problem"],"improvements":[],"evidence":["The accent mark in this string"]}]});
    assert_eq!(
        advice::validate(&advice, &s, 1, &segments).unwrap()[0]["slot"],
        1
    );
}

fn start_fixture() -> (
    PeerObservers,
    Arc<Accounts>,
    crate::config::PeerObserverConfig,
    crate::config::Provider,
) {
    let accounts = Accounts::open(crate::accounts::GitHubLoginConfig {
        oauth: None,
        session_secret: "start-ready-test".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    accounts.initialize_schema().unwrap();
    let config = crate::config::load_peer_observer(&std::collections::BTreeMap::from([(
        "GOOGLE_API_KEY".to_owned(),
        "start-ready-key".to_owned(),
    )]))
    .unwrap()
    .unwrap();
    let provider = crate::config::Provider {
        id: "p".into(),
        url: "wss://p.example".into(),
        api_key: "k".into(),
        api_secret: "s".into(),
        google_api_keys: vec!["g".into()],
    };
    (PeerObservers::new(), Arc::new(accounts), config, provider)
}

/// An observer that is already in the room, with the outcome of its join.
fn resident(
    observers: &PeerObservers,
    id: &str,
    outcome: Option<Result<(), &'static str>>,
) -> watch::Sender<Option<Result<(), &'static str>>> {
    let (stop, _) = watch::channel(false);
    let (sender, ready) = watch::channel(outcome);
    observers.tasks.lock().unwrap().insert(
        id.into(),
        Actor {
            task: tokio::spawn(std::future::pending::<()>()),
            stop,
            ready,
            audio: Arc::new(Mutex::new(None)),
        },
    );
    sender
}

#[tokio::test]
async fn a_start_reuses_the_observer_already_in_the_room() {
    let (observers, accounts, config, provider) = start_fixture();
    let _joined = resident(&observers, "s", Some(Ok(())));
    assert_eq!(
        observers
            .start_ready(accounts, config, "s".into(), provider)
            .await,
        Ok(())
    );
}

#[tokio::test]
async fn a_start_reports_why_the_observer_could_not_join() {
    let (observers, accounts, config, provider) = start_fixture();
    let _failed = resident(&observers, "s", Some(Err("observer_connect_failed")));
    assert_eq!(
        observers
            .start_ready(accounts, config, "s".into(), provider)
            .await,
        Err("observer_connect_failed")
    );
}

#[tokio::test]
async fn a_start_waits_for_a_join_that_finishes_late() {
    let (observers, accounts, config, provider) = start_fixture();
    let joining = resident(&observers, "s", None);
    let late = tokio::spawn(async move {
        tokio::task::yield_now().await;
        joining.send(Some(Ok(()))).unwrap();
        joining
    });
    assert_eq!(
        observers
            .start_ready(accounts, config, "s".into(), provider)
            .await,
        Ok(())
    );
    drop(late.await.unwrap());
}

#[tokio::test(start_paused = true)]
async fn a_join_that_never_finishes_times_out_instead_of_hanging_the_start() {
    let (observers, accounts, config, provider) = start_fixture();
    let _silent = resident(&observers, "s", None);
    assert_eq!(
        observers
            .start_ready(accounts, config, "s".into(), provider)
            .await,
        Err("observer_connect_timeout")
    );
}

#[tokio::test]
async fn a_start_is_refused_when_every_observer_slot_is_taken() {
    let (observers, accounts, config, provider) = start_fixture();
    let _held = (0..MAX_OBSERVERS)
        .map(|n| resident(&observers, &format!("other-{n}"), Some(Ok(()))))
        .collect::<Vec<_>>();
    assert_eq!(
        observers
            .start_ready(accounts, config, "new".into(), provider)
            .await,
        Err("observer_at_capacity")
    );
}

/// An observation whose transcription provider answers every request with the
/// text currently held in the returned cell.
async fn observation_with_provider()
-> (Observation, Arc<Mutex<String>>, tokio::task::JoinHandle<()>) {
    let reply = Arc::new(Mutex::new(String::from("hello")));
    let served = Arc::clone(&reply);
    let app = axum::Router::new().fallback(move || {
        let text = served.lock().unwrap().clone();
        async move {
            let output = json!({"text": text}).to_string();
            axum::Json(json!({"candidates":[{"content":{"parts":[{"text":output}]}}]}))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/generateContent", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut config = crate::config::load_peer_observer(&std::collections::BTreeMap::from([(
        "GOOGLE_API_KEY".into(),
        "observation-test-key".into(),
    )]))
    .unwrap()
    .unwrap();
    config.endpoint = Some(endpoint);
    let (accounts, session) = fixture();
    let observation = Observation {
        accounts: Arc::new(accounts),
        config,
        session,
        audio: Arc::new(Mutex::new(None)),
        origin_epoch: 1000,
        maximum_ms: u64::MAX,
        segments: vec![],
        bytes: 0,
        incomplete: false,
    };
    (observation, reply, server)
}

type Jobs = JoinSet<Result<record::Segment, &'static str>>;

fn speech(slot: usize, elapsed_ms: u64, samples: usize) -> capture::Chunk {
    capture::Chunk {
        slot,
        elapsed_ms,
        samples: vec![7; samples],
    }
}

#[tokio::test]
async fn a_spoken_chunk_becomes_a_transcription_job_timed_from_the_origin() {
    let (mut observation, _, server) = observation_with_provider().await;
    let mut jobs = Jobs::new();
    observation
        .chunk(speech(1, 5000, capture::SAMPLE_RATE * 2), &mut jobs)
        .await;
    assert_eq!(jobs.len(), 1);
    assert!(!observation.incomplete);
    let segment = jobs.join_next().await.unwrap().unwrap().unwrap();
    assert_eq!(segment.slot, 1);
    assert_eq!(segment.started_at, 1005);
    assert_eq!(segment.ended_at, 1007);
    assert_eq!(segment.round_index, Some(0));
    assert_eq!(segment.text, "hello");
    server.abort();
}

#[tokio::test]
async fn a_silent_chunk_is_never_sent_for_transcription() {
    let (mut observation, _, server) = observation_with_provider().await;
    let mut jobs = Jobs::new();
    let mut silent = speech(0, 0, 160);
    silent.samples.fill(0);
    observation.chunk(silent, &mut jobs).await;
    assert_eq!(jobs.len(), 0);
    assert!(!observation.incomplete);
    server.abort();
}

#[tokio::test]
async fn at_most_four_transcriptions_run_at_once() {
    let (mut observation, _, server) = observation_with_provider().await;
    let mut jobs = Jobs::new();
    for _ in 0..3 {
        jobs.spawn(std::future::pending());
    }
    observation.chunk(speech(0, 0, 160), &mut jobs).await;
    assert_eq!(jobs.len(), 4);
    assert!(!observation.incomplete);
    observation.chunk(speech(0, 0, 160), &mut jobs).await;
    assert_eq!(jobs.len(), 4);
    assert!(observation.incomplete, "a refused chunk marks the record");
    jobs.abort_all();
    server.abort();
}

#[tokio::test]
async fn a_failed_audio_write_marks_the_capture_incomplete_and_drops_the_writer() {
    let (mut observation, _, server) = observation_with_provider().await;
    let dir = std::env::temp_dir().join(format!("codetrial-observation-{}", std::process::id()));
    crate::peer_audio::ensure_private_directory(&dir).unwrap();
    let writer = PeerAudioWriter::begin(&dir, "failing", 60).unwrap();
    *observation.audio.lock().unwrap() = Some(writer);
    let mut jobs = Jobs::new();
    observation.chunk(speech(2, 0, 160), &mut jobs).await;
    assert!(observation.incomplete);
    assert!(observation.audio.lock().unwrap().is_none());
    std::fs::remove_dir_all(dir).unwrap();
    server.abort();
}

#[tokio::test]
async fn a_transcription_may_be_at_most_sixteen_kibibytes() {
    let (mut observation, reply, server) = observation_with_provider().await;
    for (length, expected) in [
        (16 * 1024, Ok(())),
        (16 * 1024 + 1, Err("invalid_transcription")),
    ] {
        *reply.lock().unwrap() = "a".repeat(length);
        let mut jobs = Jobs::new();
        observation.chunk(speech(0, 0, 160), &mut jobs).await;
        let result = jobs.join_next().await.unwrap().unwrap();
        assert_eq!(result.map(|_| ()), expected, "{length} bytes");
    }
    server.abort();
}

fn segment(text: &str) -> record::Segment {
    record::Segment {
        slot: 0,
        round_index: Some(0),
        started_at: 1,
        ended_at: 2,
        text: text.into(),
    }
}

#[tokio::test]
async fn a_transcribed_segment_is_kept_until_the_transcript_budget_is_spent() {
    let (mut observation, _, server) = observation_with_provider().await;
    observation.segment(Ok(Ok(segment("hi")))).await;
    assert_eq!(observation.segments.len(), 1);
    assert_eq!(observation.bytes, 2);
    assert!(!observation.incomplete);
    observation.segment(Ok(Ok(segment("")))).await;
    assert_eq!(observation.segments.len(), 1);

    observation.bytes = record::MAX_TRANSCRIPT_BYTES - 2;
    observation.segment(Ok(Ok(segment("hi")))).await;
    assert_eq!(observation.segments.len(), 2, "exactly the budget fits");
    assert!(!observation.incomplete);

    observation.segment(Ok(Ok(segment("h")))).await;
    assert_eq!(observation.segments.len(), 2, "one byte over is dropped");
    assert!(observation.incomplete);
    server.abort();
}

fn advice_segments() -> Vec<record::Segment> {
    vec![record::Segment {
        slot: 1,
        round_index: Some(0),
        started_at: 100,
        ended_at: 110,
        text: "I was implementing the check for the empty array".into(),
    }]
}

fn advice_entry(strengths: &[&str], improvements: &[&str], evidence: &[&str]) -> Value {
    json!({"advice":[{"roundIndex":0,"role":"candidate","strengths":strengths,"improvements":improvements,"evidence":evidence}]})
}

#[test]
fn advice_lists_hold_at_most_six_nonblank_entries_each_within_the_length_bound() {
    let (_, s) = fixture();
    let segments = advice_segments();
    let check = |value: Value| advice::validate(&value, &s, 1, &segments);
    let six = ["Considered an empty input"; 6];
    assert!(check(advice_entry(&six, &[], &["check for the empty array"])).is_ok());
    let seven = ["Considered an empty input"; 7];
    assert_eq!(
        check(advice_entry(&seven, &[], &["check for the empty array"])).unwrap_err(),
        "invalid_advice"
    );
    assert_eq!(
        check(advice_entry(&["   "], &[], &["check for the empty array"])).unwrap_err(),
        "invalid_advice"
    );
    let long = "a ".repeat(751);
    assert_eq!(
        check(advice_entry(&[&long], &[], &["check for the empty array"])).unwrap_err(),
        "invalid_advice"
    );
}

#[test]
fn a_gap_rule_binds_improvements_but_not_strengths() {
    let (_, s) = fixture();
    let segments = advice_segments();
    let phrase = "Explain the approach to avoid transcription ambiguity";
    assert!(
        advice::validate(
            &advice_entry(&[phrase], &[], &["check for the empty array"]),
            &s,
            1,
            &segments
        )
        .is_ok()
    );
    assert_eq!(
        advice::validate(
            &advice_entry(&[], &[phrase], &["check for the empty array"]),
            &s,
            1,
            &segments
        )
        .unwrap_err(),
        "prohibited_advice"
    );
}

#[test]
fn a_single_word_of_evidence_needs_twelve_characters() {
    let (_, s) = fixture();
    let segments = advice_segments();
    let with = |evidence: &str| {
        advice::validate(
            &advice_entry(&["Considered an empty input"], &[], &[evidence]),
            &s,
            1,
            &segments,
        )
    };
    assert!(with("implementing").is_ok());
    assert_eq!(with("implementin").unwrap_err(), "ungrounded_advice");
}

#[test]
fn an_entry_without_advice_needs_no_evidence() {
    let (_, s) = fixture();
    let segments = advice_segments();
    let result = advice::validate(&advice_entry(&[], &[], &[]), &s, 1, &segments).unwrap();
    assert_eq!(result.len(), 1);
}

#[test]
fn capture_chunks_and_wav_headers_have_exact_sizes() {
    assert_eq!(capture::CHUNK_SAMPLES, 480_000);
    let wav = capture::wav(&[1, 2, 3]);
    assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 6);
}

fn two_rounds(mut s: Session) -> Session {
    let mut second = s.rounds[0].clone();
    second.index = 1;
    second.started_at = Some(200);
    s.rounds.push(second);
    s
}

#[test]
fn a_chunk_belongs_to_the_round_that_began_before_it_and_to_none_that_changes_inside_it() {
    let (_, s) = fixture();
    let s = two_rounds(s);
    assert_eq!(round_for(&s, 150, 160), Some(0));
    assert_eq!(round_for(&s, 100, 110), Some(0));
    assert_eq!(round_for(&s, 250, 260), Some(1));
    assert_eq!(round_for(&s, 200, 210), Some(1));
    assert_eq!(round_for(&s, 150, 250), None, "crosses the second round");
    assert_eq!(round_for(&s, 150, 200), None, "ends as the second begins");
    assert_eq!(round_for(&s, 50, 60), None, "before any round");
}

#[tokio::test]
async fn withdrawing_audio_closes_the_open_writer() {
    let (observers, _, _, _) = start_fixture();
    let sender = resident(&observers, "s", Some(Ok(())));
    let dir = std::env::temp_dir().join(format!("codetrial-withdraw-{}", std::process::id()));
    crate::peer_audio::ensure_private_directory(&dir).unwrap();
    let writer = PeerAudioWriter::begin(&dir, "withdrawn", 60).unwrap();
    let slot = Arc::clone(&observers.tasks.lock().unwrap()["s"].audio);
    *slot.lock().unwrap() = Some(writer);
    assert!(dir.join("withdrawn.wav.part").exists());
    assert!(observers.withdraw_audio("s").await);
    assert!(slot.lock().unwrap().is_none());
    assert!(!dir.join("withdrawn.wav.part").exists());
    assert!(observers.withdraw_audio("unknown").await);
    drop(sender);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn the_retention_task_recovers_and_sweeps_without_being_asked() {
    let (accounts, s) = fixture();
    accounts.with(|c| peer::save(c, &s)).unwrap();
    record::begin(&accounts, &s.id, 100).unwrap();
    let accounts = Arc::new(accounts);
    let _tasks = spawn_retention(Arc::clone(&accounts));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let rows: i64 = accounts
                .with(|c| c.query_row("SELECT COUNT(*) FROM peer_observations", [], |r| r.get(0)))
                .unwrap();
            if rows == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the retention task removes an expired observation");
}

/// Runs the observer against a session shaped by `shape`, and a provider whose
/// room cannot be reached, so a session that passes every check ends at the
/// connection rather than at a refusal.
async fn run_observer(
    shape: impl FnOnce(&mut Session),
    provider_id: &str,
) -> Result<(), &'static str> {
    let (accounts, mut s) = fixture();
    s.rounds.clear();
    s.expires_at = i64::MAX / 2;
    s.observer_consented = [true, true];
    s.provider_id = Some("p".into());
    s.room_name = Some("room".into());
    shape(&mut s);
    accounts.with(|c| peer::save(c, &s)).unwrap();
    accounts
        .with(|c| {
            c.execute(
                "DELETE FROM peer_members WHERE session_id=?1 AND slot>=?2",
                rusqlite::params![s.id, i64::try_from(s.members.len()).unwrap()],
            )
        })
        .unwrap();
    let config = crate::config::load_peer_observer(&std::collections::BTreeMap::from([(
        "GOOGLE_API_KEY".to_owned(),
        "run-key".to_owned(),
    )]))
    .unwrap()
    .unwrap();
    let provider = crate::config::Provider {
        id: provider_id.into(),
        url: "ws://127.0.0.1:1".into(),
        api_key: "k".into(),
        api_secret: "s".into(),
        google_api_keys: vec!["g".into()],
    };
    let (_stop, receiver) = watch::channel(false);
    let (ready, _) = watch::channel(None);
    tokio::time::timeout(
        Duration::from_secs(30),
        run(
            Arc::new(accounts),
            config,
            s.id.clone(),
            provider,
            receiver,
            Arc::new(Mutex::new(None)),
            &ready,
        ),
    )
    .await
    .expect("the observer reaches its connection attempt")
}

#[tokio::test]
async fn the_observer_joins_only_a_consented_open_two_member_session_on_its_own_provider() {
    assert_eq!(
        run_observer(|_| {}, "p").await,
        Err("observer_connect_failed"),
        "a session that passes every check goes on to connect"
    );
    assert_eq!(
        run_observer(|s| s.observer_consented = [true, false], "p").await,
        Err("observer_consent_required")
    );
    assert_eq!(
        run_observer(|s| s.observer_consented = [false, true], "p").await,
        Err("observer_consent_required")
    );
    assert_eq!(
        run_observer(|s| s.members.truncate(1), "p").await,
        Err("observer_consent_required")
    );
    assert_eq!(
        run_observer(|s| s.status = "cancelled".into(), "p").await,
        Err("observer_consent_required")
    );
    assert_eq!(
        run_observer(|_| {}, "other").await,
        Err("provider_mismatch")
    );
    assert_eq!(
        run_observer(|s| s.room_name = None, "p").await,
        Err("room_unavailable")
    );
}

#[test]
fn only_audio_from_a_consenting_member_is_subscribed() {
    let (_, mut s) = fixture();
    let member = s.members[0].identity.clone();
    s.observer_consented = [true, true];
    assert!(audio_wanted(&s, &member, TrackKind::Audio));
    assert!(!audio_wanted(&s, &member, TrackKind::Video));
    assert!(!audio_wanted(&s, "stranger", TrackKind::Audio));
    s.observer_consented = [true, false];
    assert!(!audio_wanted(&s, &member, TrackKind::Audio));
}

#[test]
fn the_start_wait_lasts_ten_minutes_and_the_data_stream_limit_is_twelve_kib() {
    let now = tokio::time::Instant::now();
    assert_eq!(start_deadline(now) - now, Duration::from_secs(600));
    assert_eq!(MAX_DATA_STREAM_BYTES, 12_288);
}

#[test]
fn the_observer_starts_once_the_current_round_is_active() {
    let (_, mut s) = fixture();
    s.rounds[0].status = "active".into();
    s.current_round = Some(0);
    assert!(round_is_active(&s));
    s.rounds[0].status = "waiting".into();
    assert!(!round_is_active(&s));
    s.rounds[0].status = "active".into();
    s.current_round = None;
    assert!(!round_is_active(&s), "no current round");
    s.current_round = Some(5);
    assert!(!round_is_active(&s), "current round out of range");
}

#[test]
fn the_start_wait_is_abandoned_for_each_reason_alone() {
    let (_, mut s) = fixture();
    let now = tokio::time::Instant::now();
    let later = now + Duration::from_secs(1);
    assert!(!start_abandoned(&s, false, now, later));
    assert!(start_abandoned(&s, true, now, later), "stopped");
    assert!(start_abandoned(&s, false, now, now), "deadline reached");
    assert!(start_abandoned(&s, false, later, now), "deadline passed");
    s.status = "cancelled".into();
    assert!(start_abandoned(&s, false, now, later), "session closed");
}

#[test]
fn epoch_seconds_truncate_milliseconds() {
    assert_eq!(epoch_seconds(0), 0);
    assert_eq!(epoch_seconds(999), 0);
    assert_eq!(epoch_seconds(1_000), 1);
    assert_eq!(epoch_seconds(5_999), 5);
    assert_eq!(
        epoch_seconds(u64::MAX),
        i64::try_from(u64::MAX / 1000).unwrap()
    );
}

#[test]
fn capture_runs_to_the_hard_deadline_or_the_two_hour_cap_whichever_is_first() {
    let w = capture_window(1100, 1000, 1_000_500);
    assert_eq!((w.maximum, w.deadline, w.maximum_ms), (100, 1100, 99_500));
    let cap = i64::try_from(crate::config::MAX_CAPTURE_SECONDS).unwrap();
    let w = capture_window(1000 + cap + 500, 1000, 1_000_000);
    assert_eq!(w.maximum, crate::config::MAX_CAPTURE_SECONDS);
    assert_eq!(w.deadline, 1000 + cap);
    assert_eq!(w.maximum_ms, 7_200_000);
    let w = capture_window(900, 1000, 1_000_000);
    assert_eq!((w.maximum, w.deadline, w.maximum_ms), (0, 900, 0));
}

#[test]
fn only_a_deleted_audio_record_counts_as_withdrawn() {
    assert!(is_withdrawn("deleted"));
    assert!(!is_withdrawn("recording"));
    assert!(!is_withdrawn("ready"));
}

#[test]
fn audio_is_written_only_when_saved_and_not_withdrawn() {
    assert!(audio_writer_wanted(true, false));
    assert!(!audio_writer_wanted(true, true));
    assert!(!audio_writer_wanted(false, false));
    assert!(!audio_writer_wanted(false, true));
}

#[test]
fn a_cancelled_or_failed_capture_is_interrupted() {
    assert!(!interrupted(false, None));
    assert!(interrupted(true, None));
    assert!(interrupted(false, Some("observer_disconnected")));
    assert!(interrupted(true, Some("observer_disconnected")));
}

#[test]
fn a_capture_chunk_is_full_at_thirty_seconds_and_a_tail_is_any_leftover() {
    assert!(!capture::chunk_is_full(capture::CHUNK_SAMPLES - 1));
    assert!(capture::chunk_is_full(capture::CHUNK_SAMPLES));
    assert!(capture::chunk_is_full(capture::CHUNK_SAMPLES + 1));
    assert!(!capture::has_tail(&[]));
    assert!(capture::has_tail(&[0]));
}

#[tokio::test]
async fn closing_the_room_still_collects_what_the_capture_tasks_send() {
    let (mut observation, _, server) = observation_with_provider().await;

    // With room for one chunk, the second send cannot finish until the drain
    // has taken the first, so the drain cannot be skipped without a loss.
    let (sender, mut chunks) = mpsc::channel(1);
    let mut tracks = JoinSet::new();
    tracks.spawn(async move {
        for _ in 0..2 {
            sender
                .send(speech(1, 0, capture::SAMPLE_RATE))
                .await
                .unwrap();
        }
    });
    let mut jobs = Jobs::new();
    drain_tracks(&mut tracks, &mut chunks, &mut observation, &mut jobs).await;
    assert!(tracks.is_empty());
    assert!(
        !jobs.is_empty(),
        "a chunk sent while closing was transcribed"
    );
    server.abort();
}

/// The observer's failure arm writes the same status, and reports it the same
/// way. The write's result is read in the source, since the arm runs only when
/// the observer task fails and the database fails with it.
#[test]
fn the_observer_failure_arm_reports_a_status_write_that_failed() {
    let source = include_str!("../../../../src/web/peer/observer.rs");
    let production = source.split("#[cfg(test)]").next().unwrap();
    assert!(
        !production.contains("let _ = db(&accounts"),
        "a status write's result is not discarded"
    );
}
