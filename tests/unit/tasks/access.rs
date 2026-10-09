use super::*;
use crate::tasks::package::SetConfig;
use crate::tasks::test_support::{
    PIN, SET, SITE, admit, assignment, download, preparation, unlocked_service,
};

#[tokio::test]
async fn the_right_pin_opens_the_set_and_a_wrong_one_or_a_damaged_download_does_not() {
    let service = TaskService::new();
    let opened = service
        .open(1, &assignment("classroom", 7), download(), PIN, 100)
        .await
        .unwrap();
    assert_eq!(opened["setId"], "classroom");
    assert_eq!(opened["tasks"].as_array().unwrap().len(), 2);
    assert!(
        service
            .load_task(1, &assignment("classroom", 7), "delimiter-closer")
            .is_ok()
    );

    let wrong = service
        .open(2, &assignment("classroom", 7), download(), "654321", 100)
        .await;
    assert_eq!(wrong.unwrap_err().code, "invalid_pin");
    let mut damaged = download();
    *damaged.ciphertext.last_mut().unwrap() ^= 1;
    let damaged = service
        .open(2, &assignment("classroom", 7), damaged, PIN, 100)
        .await;
    assert_eq!(damaged.unwrap_err().code, "package_invalid");
    let renamed = service
        .open(2, &assignment("other-set", 7), download(), PIN, 100)
        .await;
    assert_eq!(renamed.unwrap_err().code, "package_invalid");
    assert_eq!(
        service
            .load_task(2, &assignment("classroom", 7), "delimiter-closer")
            .unwrap_err()
            .code,
        "unlock_required"
    );
}

#[tokio::test]
async fn an_unlock_is_refused_before_any_download_when_the_link_or_pin_is_malformed() {
    for site in [
        "http://teacher.github.io/course",
        "https://user:pw@example.org",
        "not a url",
    ] {
        let refused = Assignment::new(site, "classroom", 7);
        assert_eq!(refused.unwrap_err().code, "site_invalid", "{site}");
    }
    for (set, version) in [
        ("../etc", 7),
        ("classroom", 0),
        ("classroom", i32::MAX as u32 + 1),
    ] {
        let refused = Assignment::new(SITE, set, version);
        assert_eq!(refused.unwrap_err().code, "site_invalid", "{set} {version}");
    }

    // The largest version a learner's CodeTrial and the packaging tool agree
    // on.
    assert!(Assignment::new(SITE, "classroom", i32::MAX as u32).is_ok());
    let refused = TaskService::new()
        .unlock(1, &assignment("classroom", 7), "12345", 100)
        .await;
    assert_eq!(refused.unwrap_err().code, "invalid_pin");
}

#[tokio::test]
async fn the_public_task_carries_its_rules_and_nothing_private() {
    let service = unlocked_service();
    let task = service
        .load_task(1, &assignment("classroom", 7), "delimiter-closer")
        .unwrap();
    assert_eq!(task["rules"]["durationMin"], 15);
    assert_eq!(task["rules"]["lookAwaySeconds"], 8);
    assert_eq!(task["rules"]["closesAt"], Value::Null);
    for private in [
        "optimal",
        "pitfalls",
        "guide",
        "referenceCode",
        "interactionPrompt",
        "key",
    ] {
        assert!(task.get(private).is_none(), "{private}");
    }
    assert_eq!(
        service
            .load_task(1, &assignment("classroom", 7), "missing")
            .unwrap_err()
            .code,
        "task_not_found"
    );
    assert_eq!(
        service
            .load_task(2, &assignment("classroom", 7), "delimiter-closer")
            .unwrap_err()
            .code,
        "unlock_required"
    );
}

#[tokio::test]
async fn a_retried_start_replays_one_attempt_and_a_second_start_is_refused() {
    let service = unlocked_service();
    let task = admit(&service, "start-1", 100);
    assert_eq!(task.session_ordinal, 1);
    assert_eq!(task.github_login, "learner");
    assert_eq!(task.preparation, preparation());
    let replay = |key: &str, now| {
        service.begin_admission(
            1,
            ("learner", 42),
            &assignment("classroom", 7),
            "delimiter-closer",
            key,
            preparation(),
            now,
        )
    };
    assert_eq!(replay("start-1", 101).err().unwrap().code, "setup_pending");
    service.finish_admission(
        1,
        "start-1",
        task.claim_id,
        Some(json!({"roomName": "room-1"})),
    );
    match replay("start-1", 102).unwrap() {
        Admission::Replayed(response) => assert_eq!(response["roomName"], "room-1"),
        Admission::New(_) => panic!("a retry started a second attempt"),
    }
    assert_eq!(
        replay("start-2", 103).err().unwrap().code,
        "active_task_session"
    );
    let expired = 100 + default_number("pendingAdmissionSeconds");
    assert_eq!(
        replay("start-1", expired).err().unwrap().code,
        "request_key_expired"
    );
    // The attempt ending releases the account for the next one.
    drop(task);
    assert_eq!(admit(&service, "start-3", 200).session_ordinal, 2);
}

#[tokio::test]
async fn a_start_that_failed_before_dispatch_frees_its_request_key() {
    let service = unlocked_service();
    let task = admit(&service, "start-1", 100);
    service.finish_admission(1, "start-1", task.claim_id, None);
    drop(task);
    assert_eq!(admit(&service, "start-1", 101).session_ordinal, 2);
}

#[tokio::test]
async fn a_closed_set_refuses_new_attempts() {
    let service = TaskService::new();
    let closed = Arc::new(LoadedSet {
        config: SetConfig {
            closes_at: Some("2026-01-01T00:00:00Z".to_owned()),
            ..SET.config.clone()
        },
        records: SET.records.clone(),
    });
    service.insert_unlocked(1, SITE, closed);
    let start = |now| {
        service.begin_admission(
            1,
            ("learner", 42),
            &assignment("classroom", 7),
            "delimiter-closer",
            "start",
            preparation(),
            now,
        )
    };
    let close = crate::tasks::package::parse_close("2026-01-01T00:00:00Z").unwrap();
    assert_eq!(start(close).err().unwrap().code, "set_closed");
    assert!(start(close - 1).is_ok());
}

#[tokio::test]
async fn the_result_is_pending_while_the_attempt_lasts_then_held_for_its_room_only() {
    let service = unlocked_service();
    let task = admit(&service, "start", 100);
    service.finish_admission(
        1,
        "start",
        task.claim_id,
        Some(json!({"roomName": "room-1"})),
    );
    assert_eq!(service.result(1, "room-1", 150), ResultLookup::Pending);
    task.retain_result("room-1", json!({"assessmentMode": "task", "x": 1}), 200);
    drop(task);
    assert_eq!(
        service.result(1, "room-1", 201),
        ResultLookup::Ready(json!({"assessmentMode": "task", "x": 1}))
    );
    assert_eq!(service.result(1, "room-2", 201), ResultLookup::Unavailable);
    assert_eq!(service.result(2, "room-1", 201), ResultLookup::Unavailable);
    let expiry = 200 + default_number("reviewRetentionSeconds");
    assert_eq!(
        service.result(1, "room-1", expiry),
        ResultLookup::Unavailable
    );
}

#[tokio::test]
async fn a_result_too_large_to_keep_drops_the_review_but_keeps_the_ending_and_code() {
    let service = unlocked_service();
    let task = admit(&service, "start", 100);
    let huge = "x".repeat(default_number("reviewBytes") as usize);
    // Code at the byte limit, every byte of it escaped twice over in JSON.
    let code = "\"".repeat(crate::tasks::session::MAX_CODE_BYTES);
    let outcome = json!({"outcome": "completed", "cause": "finish", "at": 600, "durationMs": 0});
    task.retain_result(
        "room-1",
        json!({"taskAssessment": {"finalRevisionId": "rev", "finalCode": code,
            "outcome": outcome, "gaps": [huge]}}),
        200,
    );
    drop(task);
    let ResultLookup::Ready(kept) = service.result(1, "room-1", 201) else {
        panic!("no result kept");
    };
    assert_eq!(kept["feedbackUnavailable"], true);
    assert_eq!(kept["taskAssessment"]["finalRevisionId"], "rev");
    assert_eq!(kept["taskAssessment"]["finalCode"], code);
    assert_eq!(kept["taskAssessment"]["outcome"], outcome);
    assert!(kept["taskAssessment"].get("gaps").is_none());
}

#[tokio::test]
async fn an_oversized_invalid_result_never_claims_its_review_was_lost() {
    let service = unlocked_service();
    let task = admit(&service, "start", 100);
    let outcome = json!({"outcome": "invalid", "cause": "visibility", "at": 60, "durationMs": 0});
    let huge = "x".repeat(default_number("reviewBytes") as usize);
    task.retain_result(
        "room-1",
        json!({"taskAssessment": {"outcome": outcome, "pad": huge}}),
        200,
    );
    drop(task);
    let ResultLookup::Ready(kept) = service.result(1, "room-1", 201) else {
        panic!("no result kept");
    };
    assert_eq!(kept["taskAssessment"]["outcome"], outcome);
    assert!(kept.get("feedbackUnavailable").is_none());
}

#[tokio::test]
async fn each_version_of_a_set_is_unlocked_on_its_own_and_a_replay_must_name_the_same() {
    let service = unlocked_service();
    let other_version = Arc::new(LoadedSet {
        config: SetConfig {
            version: 8,
            ..SET.config.clone()
        },
        records: SET.records.clone(),
    });
    service.insert_unlocked(1, SITE, other_version);
    // Unlocking version 8 left version 7 where it was.
    assert_eq!(
        service
            .load_task(1, &assignment("classroom", 7), "delimiter-closer")
            .unwrap()["version"],
        7
    );
    assert_eq!(
        service
            .load_task(1, &assignment("classroom", 9), "delimiter-closer")
            .unwrap_err()
            .code,
        "unlock_required"
    );
    let task = admit(&service, "start", 100);
    assert_eq!(task.config.version, 7);
    let replay = service.begin_admission(
        1,
        ("learner", 42),
        &assignment("classroom", 8),
        "delimiter-closer",
        "start",
        preparation(),
        101,
    );
    assert_eq!(replay.err().unwrap().code, "client_override");
}

#[tokio::test]
async fn one_process_keeps_a_bounded_number_of_unlocked_sets() {
    let service = TaskService::new();
    for owner in 1..=(MAX_UNLOCKED_SETS as i64 + 1) {
        service
            .open(owner, &assignment("classroom", 7), download(), PIN, 100)
            .await
            .unwrap();
    }
    assert_eq!(
        service
            .load_task(1, &assignment("classroom", 7), "delimiter-closer")
            .unwrap_err()
            .code,
        "unlock_required"
    );
    assert!(
        service
            .load_task(2, &assignment("classroom", 7), "delimiter-closer")
            .is_ok()
    );
}

#[tokio::test]
async fn two_sites_naming_the_same_set_and_version_are_two_assignments() {
    let service = unlocked_service();
    let elsewhere = Assignment::new("https://other.github.io/course", "classroom", 7).unwrap();
    assert_eq!(
        service
            .load_task(1, &elsewhere, "delimiter-closer")
            .unwrap_err()
            .code,
        "unlock_required"
    );
    let task = admit(&service, "start", 100);
    let replay = service.begin_admission(
        1,
        ("learner", 42),
        &elsewhere,
        "delimiter-closer",
        "start",
        preparation(),
        101,
    );
    assert_eq!(replay.err().unwrap().code, "client_override");
    drop(task);
}

#[tokio::test]
async fn open_keeps_one_package_per_assignment_and_evicts_the_oldest() {
    let service = TaskService::new();
    let here = assignment("classroom", 7);
    let there = Assignment::new("https://other.github.io/course", "classroom", 7).unwrap();
    for assignment in [&here, &there] {
        service
            .open(1, assignment, download(), PIN, 100)
            .await
            .unwrap();
    }
    for assignment in [&here, &there] {
        assert!(service.load_task(1, assignment, "delimiter-closer").is_ok());
    }

    // Reopening `here` makes it the newest, so filling the rest of the cap
    // evicts `there` first.
    service.open(1, &here, download(), PIN, 100).await.unwrap();
    for owner in 2..=MAX_UNLOCKED_SETS as i64 {
        service
            .open(owner, &here, download(), PIN, 100)
            .await
            .unwrap();
    }
    assert_eq!(
        service
            .load_task(1, &there, "delimiter-closer")
            .unwrap_err()
            .code,
        "unlock_required"
    );
    assert!(service.load_task(1, &here, "delimiter-closer").is_ok());
}

#[tokio::test]
async fn one_process_runs_one_attempt_whoever_asks() {
    let service = unlocked_service();
    service.insert_unlocked(2, SITE, SET.clone());
    let first = admit(&service, "start", 100);
    let second = service.begin_admission(
        2,
        ("other", 7),
        &assignment("classroom", 7),
        "delimiter-closer",
        "start",
        preparation(),
        101,
    );
    assert_eq!(second.err().unwrap().code, "active_task_session");
    drop(first);
}

#[tokio::test]
async fn only_the_running_attempts_own_room_is_pending() {
    let service = unlocked_service();
    let task = admit(&service, "start", 100);
    // Before the admission answers with its room, no room is the attempt's.
    assert_eq!(service.result(1, "room-1", 101), ResultLookup::Unavailable);
    service.finish_admission(
        1,
        "start",
        task.claim_id,
        Some(json!({"roomName": "room-1"})),
    );
    assert_eq!(service.result(1, "room-1", 102), ResultLookup::Pending);
    assert_eq!(
        service.result(1, "stale-room", 102),
        ResultLookup::Unavailable
    );
    assert_eq!(service.result(2, "room-1", 102), ResultLookup::Unavailable);
    drop(task);
}

#[tokio::test(start_paused = true)]
async fn the_site_clock_runs_on_from_the_download_and_ignores_the_learners() {
    let unlocked = Unlocked {
        set: SET.clone(),
        site_clock: Some((1_000, Instant::now())),
    };
    tokio::time::advance(std::time::Duration::from_secs(30)).await;
    assert_eq!(unlocked.now(5), 1_030);
    // Without a site clock the learner's is all there is.
    let local = Unlocked {
        set: SET.clone(),
        site_clock: None,
    };
    assert_eq!(local.now(5), 5);
}

#[test]
fn a_service_is_equal_only_to_itself() {
    let service = TaskService::new();
    assert!(service == service.clone());
    assert!(service != TaskService::new());
}

#[test]
fn each_admission_is_its_own_claim_and_the_oldest_keys_are_forgotten_first() {
    let service = unlocked_service();
    let first = admit(&service, "key-0", 100);
    let first_claim = first.claim_id;
    drop(first);
    let second = admit(&service, "key-1", 100);
    assert_ne!(second.claim_id, first_claim);
    drop(second);
    let held = |key: &str| {
        service
            .0
            .state
            .lock()
            .unwrap()
            .admissions
            .contains_key(&(1, key.to_owned()))
    };
    for index in 2..MAX_ADMISSIONS {
        drop(admit(&service, &format!("key-{index}"), 100));
    }
    // Exactly the cap is held...
    assert!(held("key-0"));
    drop(admit(&service, "one-more", 100));
    // ...and one past it lets the oldest go, and only that one.
    assert!(!held("key-0"));
    assert!(held("key-1"));
}

#[test]
fn the_least_recently_opened_set_is_the_one_let_go() {
    let service = TaskService::new();
    let held = |owner: i64| {
        service
            .0
            .state
            .lock()
            .unwrap()
            .unlocked
            .contains_key(&(owner, assignment("classroom", 7)))
    };
    for owner in 1..=MAX_UNLOCKED_SETS as i64 {
        service.insert_unlocked(owner, SITE, SET.clone());
    }
    // Opening the first again makes it the most recent.
    service.insert_unlocked(1, SITE, SET.clone());
    assert!(held(1) && held(2));
    service.insert_unlocked(99, SITE, SET.clone());
    assert!(held(1));
    assert!(!held(2));
    assert!(held(3));
}

#[test]
fn a_failed_setup_forgets_its_own_key_and_no_other() {
    let service = unlocked_service();
    drop(admit(&service, "earlier", 100));
    let failing = admit(&service, "failing", 100);
    service.finish_admission(1, "failing", failing.claim_id, None);
    let state = service.0.state.lock().unwrap();
    assert_eq!(
        state.admission_order.iter().collect::<Vec<_>>(),
        [&(1, "earlier".to_owned())]
    );
    assert!(!state.admissions.contains_key(&(1, "failing".to_owned())));
}
