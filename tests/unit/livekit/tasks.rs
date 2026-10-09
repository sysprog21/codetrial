use super::*;
use crate::tasks::session::OutcomeKind;
use crate::tasks::test_support::admitted;

#[test]
fn healthy_sockets_refill_recovery_budget_across_more_than_one_session_limit() {
    let mut now = Instant::now();
    let mut budget = RecoveryBudget::new(now);
    for _ in 0..super::super::GEMINI_RESTART_LIMIT * 2 {
        now += super::super::HEALTHY_GEMINI_SOCKET;
        budget.failed(now);
        assert!(budget.start_attempt(now));
        assert_eq!(budget.attempts, 1);
        budget.recovered(now);
    }
}

#[test]
fn failed_recovery_contexts_wait_and_exhaust_without_false_healthy_resets() {
    let mut now = Instant::now();
    let mut budget = RecoveryBudget::new(now);
    budget.failed(now);
    for _ in 0..super::super::GEMINI_RESTART_LIMIT {
        assert!(budget.start_attempt(now));
        budget.failed_attempt(now);
        assert!(!budget.start_attempt(now + Duration::from_millis(1)));
        now += super::super::COLD_OPEN_BACKOFF;
    }
    assert!(budget.exhausted());
    budget.failed(now + Duration::from_secs(600));
    assert!(!budget.start_attempt(now + Duration::from_secs(600)));
}

#[tokio::test]
async fn timed_wrap_up_preserves_remaining_work_and_finish_allows_stopping() {
    let mut session = TaskSession::new(admitted().await, 100);
    session.start(100);
    session.tick(880, false, false);
    assert_eq!(session.phase, Phase::WrapUp);
    for _ in 0..3 {
        assert!(session.next_wrap_up_check().is_some());
    }
    assert!(session.next_wrap_up_check().is_none());
    assert!(!wrap_up_should_stop(&session));
    session
        .acknowledge_edit("late-edit", "latest learner work", 990)
        .unwrap();
    session.tick(1000, false, false);
    assert_eq!(session.phase, Phase::Feedback);
    assert_eq!(session.revisions["late-edit"].code, "latest learner work");
    assert!(wrap_up_should_stop(&session));
}

#[tokio::test]
async fn lost_room_interrupts_the_attempt_with_its_frozen_work_but_never_setup() {
    let mut session = TaskSession::new(admitted().await, 100);
    assert!(lost_room_review(&mut session, "lost-room").is_none());
    assert_eq!(session.phase, Phase::Ready);
    session.start(101);
    session
        .acknowledge_edit("last-edit", "preserved learner code", 102)
        .unwrap();
    let review = lost_room_review(&mut session, "lost-room").unwrap();
    let assessment = &review["taskAssessment"];
    assert_eq!(assessment["sessionId"], "lost-room");
    assert_eq!(assessment["outcome"]["outcome"], "interrupted");
    assert_eq!(assessment["outcome"]["cause"], "room_loss");
    assert_eq!(assessment["finalRevisionId"], "last-edit");
    assert_eq!(assessment["finalCode"], "preserved learner code");
    assert!(assessment.get("dimensions").is_none());
    assert_eq!(
        session.revisions["last-edit"].code,
        "preserved learner code"
    );
    assert!(lost_room_review(&mut session, "lost-room").is_none());
}

#[tokio::test]
async fn a_page_that_left_is_never_finalized_as_completed() {
    let rejoin = Duration::from_secs(120);
    let gone = |seconds| Some(Duration::from_secs(seconds));
    let mut session = TaskSession::new(admitted().await, 100);
    assert_eq!(page_loss(&session, None, rejoin), PageLoss::Wait);

    // Before Start the account is freed once the page has been away briefly,
    // whether it left or never joined.
    assert_eq!(page_loss(&session, gone(2), rejoin), PageLoss::Wait);
    assert_eq!(page_loss(&session, gone(10), rejoin), PageLoss::Stop);
    // Before Start nothing is interrupted, however short the rejoin window.
    assert_eq!(
        page_loss(&session, gone(2), Duration::from_secs(1)),
        PageLoss::Wait
    );
    session.start(100);
    assert_eq!(page_loss(&session, gone(5), rejoin), PageLoss::Wait);
    assert_eq!(
        page_loss(&session, gone(120), rejoin),
        PageLoss::Interrupted
    );
    // A deadline passing while it is gone is the session's: `expire` knows.
    session.page_left();
    let deadline = session.deadline_at.unwrap();
    session.tick(deadline, false, false);
    assert_eq!(session.outcome.unwrap().outcome, OutcomeKind::Interrupted);
    assert_eq!(page_loss(&session, gone(500), rejoin), PageLoss::Wait);
}

#[test]
fn successful_returns_do_not_exhaust_provider_recovery() {
    let now = Instant::now();
    let mut budget = RecoveryBudget::new(now);
    for _ in 0..super::super::GEMINI_RESTART_LIMIT * 3 {
        budget.allow_immediate(now);
        budget.failed(now);
        assert!(budget.start_attempt(now));
        budget.recovered(now);
        assert_eq!(budget.attempts, 0);
    }
    budget.allow_immediate(now);
    assert!(budget.start_attempt(now));
    budget.failed_attempt(now);
    assert_eq!(budget.attempts, 1);
}

#[test]
fn deliberate_restart_can_succeed_after_the_last_real_recovery() {
    let now = Instant::now();
    let mut budget = RecoveryBudget::new(now);
    budget.attempts = super::super::GEMINI_RESTART_LIMIT;
    budget.allow_immediate(now);
    assert!(budget.start_attempt(now));
    budget.recovered(now);
    assert_eq!(budget.attempts, super::super::GEMINI_RESTART_LIMIT);
    budget.allow_immediate(now);
    assert!(budget.start_attempt(now));
    budget.failed_attempt(now);
    assert!(budget.exhausted());
}

#[tokio::test]
async fn a_replacement_connection_is_briefed_with_the_conversation_it_missed() {
    let mut session = TaskSession::new(admitted().await, 100);
    session.start(100);
    session.observe_turn("turn-1", "The stack holds the openers still waiting.", 101);

    // Greek letters stand for a turn the recognizer returned in another script;
    // the script, not any language, is what the rule tests.
    session.observe_turn(
        "turn-2",
        "\u{3b1}\u{3b2}\u{3b3}\u{3b4} \u{3b5}\u{3b6}\u{3b7}\u{3b8} \u{3b9}\u{3ba}\u{3bb}\u{3bc}",
        102,
    );
    let briefing = recovery_conversation(
        &session,
        "What must stay true as the loop advances?",
        "I think the top of the stack",
    );
    assert!(briefing.contains("The stack holds the openers still waiting."));
    assert!(briefing.contains(crate::agent::UNRECOGNIZED_TURN));
    assert!(!briefing.contains("\u{3b1}\u{3b2}"));
    assert!(briefing.contains("What must stay true as the loop advances?"));
    assert!(briefing.contains("owed a reply"));
    assert!(briefing.contains("I think the top of the stack"));
    assert!(briefing.contains("BEGIN UNTRUSTED RECENT CONVERSATION"));
    let settled = recovery_conversation(&session, "", "  ");
    assert!(settled.contains("No learner speech is waiting"));
}

#[test]
fn a_deliberate_restart_is_not_shown_as_an_outage_until_it_fails() {
    let now = Instant::now();
    let mut budget = RecoveryBudget::new(now);
    budget.allow_immediate(now);
    budget.failed(now);
    assert_eq!(budget.outage_availability(), None);
    assert!(budget.start_attempt(now));
    budget.failed_attempt(now);
    assert_eq!(budget.outage_availability(), Some(Interviewer::Degraded));
    let mut budget = RecoveryBudget::new(now);
    budget.allow_immediate(now);
    budget.failed(now);
    assert!(budget.start_attempt(now));
    budget.recovered(now);
    assert_eq!(budget.attempts, 0);
    assert_eq!(budget.outage_availability(), Some(Interviewer::Degraded));
}

/// The next frame the fixture socket received, failing rather than waiting
/// forever when none is sent.
async fn next_frame(
    frames: &mut tokio::sync::mpsc::UnboundedReceiver<Option<String>>,
) -> Option<String> {
    tokio::time::timeout(Duration::from_secs(5), frames.recv())
        .await
        .expect("no frame was sent")
        .unwrap()
}

/// A Gemini Live socket on a local port: completes setup, then hands every
/// later frame to the test through the returned channel, a close included.
async fn live_fixture(
    task: &PinnedTask,
) -> (
    GeminiLiveSession,
    tokio::sync::mpsc::UnboundedReceiver<Option<String>>,
) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let config = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "task-test-key"),
    ])
    .unwrap();
    let keys = GeminiKeys::from_config(&config);
    let boot = TaskLive::new(&config, task);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let (frames, received) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        let _setup = socket.next().await;
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    let _ = frames.send(Some(text.to_string()));
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                    let _ = frames.send(None);
                    return;
                }
                Some(Ok(_)) => {}
            }
        }
    });
    let gemini = crate::gemini::live_session_with_keys_at(&url, &keys, &boot, None)
        .await
        .unwrap();
    (gemini, received)
}

#[tokio::test]
async fn a_replaced_socket_is_closed_rather_than_left_reading() {
    let task = admitted().await;
    let (mut gemini, mut old) = live_fixture(&task).await;
    let (replacement, _new) = live_fixture(&task).await;
    let mut usage = LiveUsage::new();
    retire_socket(&mut gemini, replacement, &mut usage, "task-room").await;
    assert_eq!(usage.socket, 2);
    let closed = tokio::time::timeout(Duration::from_secs(5), old.recv())
        .await
        .expect("the old socket stayed open");
    assert_eq!(closed, Some(None));
}

#[tokio::test]
async fn the_briefing_asks_for_a_reply_only_when_one_is_owed_and_nothing_else_will() {
    let task = admitted().await;
    let session = TaskSession::new(task.clone(), 100);
    let briefing_turn = |frame: Option<String>| {
        let frame: Value = serde_json::from_str(&frame.unwrap()).unwrap();
        frame["clientContent"]["turnComplete"].as_bool().unwrap()
    };

    // Speech the dropped socket never answered: the briefing asks for the
    // reply.
    let (mut gemini, mut frames) = live_fixture(&task).await;
    let mut pending = VecDeque::new();
    brief_replacement(&mut gemini, &session, "", "I think the stack", &mut pending)
        .await
        .unwrap();
    assert!(briefing_turn(next_frame(&mut frames).await));
    // Nothing owed: the briefing is context only, and Jim waits.
    let (mut gemini, mut frames) = live_fixture(&task).await;
    brief_replacement(&mut gemini, &session, "", "  ", &mut pending)
        .await
        .unwrap();
    assert!(!briefing_turn(next_frame(&mut frames).await));
    // Owed, but a queued action is replayed next and draws the reply itself.
    let (mut gemini, mut frames) = live_fixture(&task).await;
    let mut pending = VecDeque::from(["Discuss this acknowledged revision.".to_owned()]);
    brief_replacement(&mut gemini, &session, "", "I think the stack", &mut pending)
        .await
        .unwrap();
    assert!(!briefing_turn(next_frame(&mut frames).await));
    assert!(next_frame(&mut frames).await.is_some());
    assert!(pending.is_empty());
}

#[tokio::test]
async fn a_session_dropped_without_shutdown_still_closes_its_socket() {
    let task = admitted().await;
    let (gemini, mut frames) = live_fixture(&task).await;
    drop(gemini);
    let closed = tokio::time::timeout(Duration::from_secs(5), frames.recv())
        .await
        .expect("the dropped session kept its socket open");
    assert_eq!(closed, Some(None));
}

#[test]
fn a_started_attempt_holds_the_next_one_for_the_backoff() {
    let now = Instant::now();
    let mut budget = RecoveryBudget::new(now);
    budget.failed(now);
    assert!(budget.start_attempt(now));
    // No failure reported yet: the attempt itself sets the wait.
    assert!(!budget.start_attempt(now + super::super::COLD_OPEN_BACKOFF / 2));
    assert!(budget.start_attempt(now + super::super::COLD_OPEN_BACKOFF));
}

/// The cap is the largest revision the session accepts, at JSON's worst
/// spelling of each byte, plus room for the envelope around it, and no more.
#[test]
fn a_task_message_holds_the_largest_revision_and_little_else() {
    let worst = serde_json::to_vec(&json!({"type": "task.revision", "version": 1,
        "requestId": "r".repeat(64), "revisionId": "v".repeat(64),
        "code": "\0".repeat(MAX_CODE_BYTES), "trigger": "discuss"}))
    .unwrap()
    .len();
    assert!(worst <= MAX_TASK_MESSAGE_BYTES, "{worst}");
    assert!(MAX_TASK_MESSAGE_BYTES - worst <= 4096, "{worst}");
}

#[test]
fn each_recorded_usage_is_counted_into_the_room_total() {
    let mut usage = LiveUsage::new();
    let turn = crate::gemini::TokenUsage {
        prompt: 7,
        response: 3,
        total: 10,
        ..Default::default()
    };
    usage.record("task-room", turn);
    usage.record("task-room", turn);
    assert_eq!(usage.events, 2);
    assert_eq!((usage.total.prompt, usage.total.total), (14, 20));
    let summary = usage.summary("task-room", "live-model", super::super::LiveOutcome::Ok);
    assert!(
        summary.starts_with("codetrial live_usage room=task-room "),
        "{summary}"
    );
    assert!(summary.contains(" model=live-model "), "{summary}");
    assert!(summary.contains(" sockets=1 "), "{summary}");
}

#[test]
fn the_learner_is_speaking_for_three_seconds_after_their_last_word() {
    assert!(speaking(10, 10));
    assert!(speaking(10, 12));
    assert!(!speaking(10, 13));
}

#[test]
fn a_wrap_up_question_waits_for_an_answer_or_its_time() {
    let wait = default_number("wrapUpAnswerSeconds");
    let quiet = 0;
    // The first one is asked as soon as the learner is quiet.
    assert!(wrap_up_due(None, 0, quiet, 100));
    assert!(!wrap_up_due(None, 0, 99, 100));
    // The next waits for a turn completed after the question...
    assert!(!wrap_up_due(Some(100), 100, quiet, 101));
    assert!(wrap_up_due(Some(100), 101, quiet, 101));
    // ...or for the time an answer is given, exactly.
    assert!(!wrap_up_due(Some(100), 100, quiet, 100 + wait - 1));
    assert!(wrap_up_due(Some(100), 100, quiet, 100 + wait));
    // Speech still holds it, even when it is due.
    assert!(!wrap_up_due(Some(100), 101, 100 + wait, 100 + wait));
}

#[tokio::test]
async fn failed_wrap_up_send_preserves_the_question_and_its_budget() {
    let mut session = TaskSession::new(admitted().await, 100);
    session.start(100);
    session.tick(880, false, false);
    let (mut failed, mut closed) = live_fixture(&session.task).await;
    failed.shutdown().await.unwrap();
    assert!(next_frame(&mut closed).await.is_none());
    let first = session.next_wrap_up_check().unwrap();
    let mut asked_at = None;
    for _ in 0..4 {
        assert!(
            send_wrap_up_question(&mut failed, &mut session, &first, &mut asked_at, 880)
                .await
                .is_err()
        );
        assert_eq!(asked_at, None);
        assert_eq!(
            session.next_wrap_up_check().as_deref(),
            Some(first.as_str())
        );
    }

    let (mut replacement, mut frames) = live_fixture(&session.task).await;
    send_wrap_up_question(&mut replacement, &mut session, &first, &mut asked_at, 900)
        .await
        .unwrap();
    assert_eq!(asked_at, Some(900));
    assert!(!wrap_up_due(asked_at, 0, 0, 901));
    let frame = next_frame(&mut frames).await.unwrap();
    assert!(frame.contains(session.task.exercise.check_question(&first).unwrap()));
    let mut asked = vec![first];
    while let Some(id) = session.next_wrap_up_check() {
        assert!(!asked.contains(&id));
        send_wrap_up_question(&mut replacement, &mut session, &id, &mut asked_at, 920)
            .await
            .unwrap();
        assert!(next_frame(&mut frames).await.is_some());
        asked.push(id);
    }
    assert_eq!(asked.len(), default_number("wrapUpChecks") as usize);
    replacement.shutdown().await.unwrap();
}
