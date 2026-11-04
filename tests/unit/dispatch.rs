//! The `tests` module of `src/dispatch.rs`, which declares this file by path.
//! Everything here reaches into `src/dispatch.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use super::*;

#[path = "dispatch_http.rs"]
mod http_probe;

/// The capacity slot is released by dropping, so an interview that panics
/// gives its slot back like one that ends. A cap's worth of leaked slots
/// would refuse every interview after them for the life of the process.
#[tokio::test]
async fn a_panicking_interview_gives_its_capacity_back() {
    let state = Arc::new(Mutex::new(DispatchState::default()));
    state
        .lock()
        .unwrap()
        .live
        .insert("interview-boom".to_string(), true);
    let slot = Slot {
        state: Arc::clone(&state),
        guest: false,
        room_name: "interview-boom".to_string(),
        released: Arc::default(),
    };

    let task = tokio::spawn(async move {
        let _held = slot;
        panic!("the interview blew up");
    });
    assert!(task.await.is_err(), "the task must have panicked");

    assert!(
        state.lock().unwrap().live.is_empty(),
        "a panicking interview must not keep its slot"
    );
}

/// Change keys saves only while nothing runs here, and an interview still
/// writing its report is running: restarting would lose the report.
#[tokio::test]
async fn an_interview_writing_its_report_keeps_the_dispatcher_busy() {
    let dispatcher = LocalDispatcher {
        config: crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap(),
        runtime: tokio::runtime::Handle::current(),
        state: Arc::default(),
        max_concurrent: 1,
    };
    assert!(dispatcher.is_idle());
    let Reservation::New(slot) = dispatcher.reserve_with_kind("interview-busy", false) else {
        panic!("an idle dispatcher admits a new room");
    };
    assert!(!dispatcher.is_idle());
    slot.assessment_finished();
    assert!(!dispatcher.is_idle());
    drop(slot);
    assert!(dispatcher.is_idle());
}

/// The cap is the configured one, not a constant. An operator who set it to
/// one gets one, and the refusal names the number they chose.
#[tokio::test]
async fn the_configured_cap_is_the_one_enforced() {
    let state = Arc::new(Mutex::new(DispatchState::default()));
    state
        .lock()
        .unwrap()
        .live
        .insert("interview-first".to_string(), true);
    let dispatcher = LocalDispatcher {
        config: crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap(),
        runtime: tokio::runtime::Handle::current(),
        state: Arc::clone(&state),
        max_concurrent: 1,
    };
    let provider = Provider {
        id: crate::config::PRIMARY_PROVIDER_ID.to_string(),
        url: "wss://primary.example".to_string(),
        api_key: "primary-key".to_string(),
        api_secret: "primary-secret".to_string(),
        google_api_keys: Vec::new(),
    };

    assert!(
        dispatcher.ensure_agent("interview-second", &provider, AgentJob::Interview)
            == Err(DispatchRefusal::AtCapacity),
        "a second room must be refused at a cap of one"
    );

    // The room already running is still admitted, because a reload asks for the
    // same room and refusing it would break the page that is open.
    assert!(
        dispatcher
            .ensure_agent("interview-first", &provider, AgentJob::Interview)
            .is_ok()
    );
}

/// The no-show-aware entry point must retain the local dispatcher's admission
/// checks instead of accepting a room without reserving or staffing it.
#[tokio::test]
async fn no_show_dispatch_enforces_the_configured_cap() {
    let state = Arc::new(Mutex::new(DispatchState::default()));
    state
        .lock()
        .unwrap()
        .live
        .insert("interview-first".to_string(), true);
    let dispatcher = LocalDispatcher {
        config: crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap(),
        runtime: tokio::runtime::Handle::current(),
        state,
        max_concurrent: 1,
    };
    let provider = Provider {
        id: crate::config::PRIMARY_PROVIDER_ID.to_string(),
        url: "wss://primary.example".to_string(),
        api_key: "primary-key".to_string(),
        api_secret: "primary-secret".to_string(),
        google_api_keys: Vec::new(),
    };

    assert_eq!(
        dispatcher.ensure_agent_with_no_show_release(
            "interview-second",
            &provider,
            "candidate-second",
            None
        ),
        Err(DispatchRefusal::AtCapacity)
    );
}

/// Removes a temporary account database even when its test unwinds early.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        Self(std::env::temp_dir().join(format!(
            "codetrial-{label}-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

/// An account database with one interview whose consent has claimed
/// `room_name`, as `/api/token` leaves it before dispatch. The scratch file is
/// returned first so it outlives the connection that holds it open.
fn claimed_interview(
    label: &str,
    github_id: i64,
    interview_id: &str,
    room_name: &str,
) -> (Scratch, Arc<crate::accounts::Accounts>, i64) {
    let path = Scratch::new(label);
    crate::accounts::initialize_account_database(&path.0).unwrap();
    let accounts = Arc::new(
        crate::accounts::Accounts::open(crate::accounts::GitHubLoginConfig {
            oauth: None,
            session_secret: "session".into(),
            db_path: path.0.clone(),
            oauth_base_url: String::new(),
            api_base_url: String::new(),
        })
        .unwrap(),
    );
    let session = crate::accounts::create_session(
        &accounts,
        &crate::accounts::GitHubProfile {
            github_id,
            login: label.into(),
            avatar_url: None,
            verified_email: None,
        },
    )
    .unwrap();
    let account_id = crate::accounts::session_user(&accounts, &session)
        .unwrap()
        .unwrap()
        .id;
    crate::accounts::create_interview(&accounts, interview_id, account_id, "v1", 1)
        .unwrap()
        .unwrap();
    assert!(
        crate::accounts::claim_interview_room(&accounts, interview_id, account_id, room_name)
            .unwrap()
    );
    (path, accounts, account_id)
}

/// The task's shared cleanup helper is the production edge between a no-show
/// outcome and the database release. Exercising it directly makes removing the
/// call observable without waiting through a LiveKit room deadline.
#[tokio::test]
async fn release_no_show_gives_the_claimed_room_back() {
    let interview_id = "interview-release-no-show";
    let room_name = "interview-release-room";
    let (_path, accounts, account_id) =
        claimed_interview("dispatch-release", -1, interview_id, room_name);

    assert!(
        release_no_show(
            Some(NoShowConsent::new(
                Arc::clone(&accounts),
                interview_id.into(),
                account_id,
                room_name.into(),
            )),
            room_name,
        )
        .await
    );
    assert!(
        !release_no_show(
            Some(NoShowConsent::new(
                Arc::clone(&accounts),
                interview_id.into(),
                account_id,
                room_name.into(),
            )),
            room_name,
        )
        .await
    );

    let room: Option<String> = accounts
        .with(|connection| {
            connection.query_row(
                "SELECT room_name FROM interviews WHERE id = ?1",
                [interview_id],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(room, None);

    assert!(
        crate::accounts::claim_interview_room(&accounts, interview_id, account_id, room_name)
            .unwrap()
    );
    accounts
        .with(|connection| {
            connection.execute(
                "INSERT INTO recordings (
                    id, account_id, interview_id, room_name, idempotency_key,
                    recipient_email, state, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?3, ?5, 'starting', 1, 1)",
                (
                    "recording-release-race",
                    account_id,
                    interview_id,
                    room_name,
                    "candidate@example.com",
                ),
            )?;
            Ok(())
        })
        .unwrap();
    assert!(
        !release_no_show(
            Some(NoShowConsent::new(
                Arc::clone(&accounts),
                interview_id.into(),
                account_id,
                room_name.into(),
            )),
            room_name,
        )
        .await
    );
    let recorded_room: Option<String> = accounts
        .with(|connection| {
            connection.query_row(
                "SELECT room_name FROM interviews WHERE id = ?1",
                [interview_id],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(recorded_room.as_deref(), Some(room_name));

    drop(accounts);
}

#[test]
fn pre_candidate_outcomes_release_consent() {
    assert!(no_show_release_required(
        &crate::livekit::RoomRunOutcome::NoCandidate
    ));
    assert!(no_show_release_required(
        &crate::livekit::RoomRunOutcome::FailedBeforeCandidate(Box::new(std::io::Error::other(
            "startup failed"
        )))
    ));
    assert!(!no_show_release_required(
        &crate::livekit::RoomRunOutcome::Completed
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_concurrent_burst_never_overbooks_and_released_capacity_returns() {
    let state = Arc::new(Mutex::new(DispatchState::default()));
    let dispatcher = LocalDispatcher {
        config: crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap(),
        runtime: tokio::runtime::Handle::current(),
        state: Arc::clone(&state),
        max_concurrent: 8,
    };

    // Reserved from concurrent tasks released together, not in a loop: the cap
    // is a check and an insert under one lock, and a serial caller cannot tell
    // that apart from a version that overbooks when a hundred requests arrive
    // at once.
    let dispatcher = Arc::new(dispatcher);
    let gate = Arc::new(tokio::sync::Barrier::new(100));
    let mut tasks = Vec::new();
    for index in 0..100 {
        let dispatcher = Arc::clone(&dispatcher);
        let gate = Arc::clone(&gate);
        tasks.push(tokio::spawn(async move {
            gate.wait().await;
            match dispatcher.reserve_with_kind(&format!("interview-burst-{index}"), false) {
                Reservation::New(slot) => Some(slot),
                Reservation::Full { .. } => None,
                Reservation::GuestRoomInUse => {
                    panic!("signed-in reservations must not hit the guest room limit")
                }
                Reservation::Existing | Reservation::Finalizing | Reservation::Closed => {
                    panic!("burst ids are unique and the dispatcher is open")
                }
            }
        }));
    }
    let mut held = Vec::new();
    for task in tasks {
        held.extend(task.await.unwrap());
    }
    assert_eq!(held.len(), 8);
    assert_eq!(state.lock().unwrap().live.len(), 8);

    // Named from a slot that actually won: which eight of the hundred get in is
    // up to the scheduler, so the serial loop's assumption that it is the first
    // eight does not survive running them at once.
    let winner = held[0].room_name.clone();
    assert!(matches!(
        dispatcher.reserve_with_kind(&winner, false),
        Reservation::Existing
    ));
    held.truncate(3);
    assert_eq!(state.lock().unwrap().live.len(), 3);
    for index in 100..105 {
        assert!(matches!(
            dispatcher.reserve_with_kind(&format!("interview-burst-{index}"), false),
            Reservation::New(_)
        ));
    }
    // The temporary slots above drop at the end of each assertion.
    assert_eq!(state.lock().unwrap().live.len(), 3);
}

#[tokio::test]
async fn guest_capacity_is_limited_separately_and_released() {
    let state = Arc::new(Mutex::new(DispatchState::default()));
    let dispatcher = LocalDispatcher {
        config: crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap(),
        runtime: tokio::runtime::Handle::current(),
        state: Arc::clone(&state),
        max_concurrent: 8,
    };

    let first = dispatcher.reserve_with_kind("guest-one", true);
    let second = dispatcher.reserve_with_kind("guest-two", true);
    assert!(matches!(first, Reservation::New(_)));
    assert!(matches!(second, Reservation::New(_)));
    assert!(matches!(
        dispatcher.reserve_with_kind("guest-three", true),
        Reservation::Full {
            guest_cap: true,
            ..
        }
    ));

    // Signed-in interviews can still use the remaining global capacity.
    let signed_in = dispatcher.reserve_with_kind("signed-in-one", false);
    assert!(matches!(signed_in, Reservation::New(_)));
    drop(first);
    assert!(matches!(
        dispatcher.reserve_with_kind("guest-three", true),
        Reservation::New(_)
    ));
}

/// A provider's own Google key wins, over the global list as well as the
/// global key, and an empty one leaves the base credentials alone rather than
/// blanking them.
#[test]
fn provider_credentials_replace_the_base_without_blanking_the_key() {
    let mut base = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://primary.example"),
        ("LIVEKIT_API_KEY", "primary-key"),
        ("LIVEKIT_API_SECRET", "primary-secret"),
        ("GOOGLE_API_KEY", "primary-google"),
    ])
    .unwrap();
    let mut provider = Provider {
        id: "second".to_string(),
        url: "wss://second.example".to_string(),
        api_key: "second-key".to_string(),
        api_secret: "second-secret".to_string(),
        google_api_keys: Vec::new(),
    };

    let inherited = agent_config_for(&base, &provider);
    assert_eq!(inherited.livekit_url, "wss://second.example");
    assert_eq!(inherited.google_api_keys, ["primary-google"]);

    base.google_api_keys = vec!["global-first".into(), "global-second".into()];
    provider.google_api_keys = vec!["second-google".into()];
    let own_key = agent_config_for(&base, &provider);
    assert_eq!(own_key.google_api_keys, ["second-google"]);
    assert_eq!(
        crate::gemini::GeminiKeys::from_config(&own_key)
            .select()
            .unwrap(),
        "second-google"
    );
    provider.google_api_keys.clear();
    let inherited = agent_config_for(&base, &provider);
    assert_eq!(
        crate::gemini::GeminiKeys::from_config(&inherited)
            .select()
            .unwrap(),
        "global-first"
    );
    provider.google_api_keys = vec!["provider-first".into(), "provider-second".into()];
    let selected = agent_config_for(&base, &provider);
    assert_eq!(selected.google_api_keys, provider.google_api_keys);
    assert_eq!(selected.livekit_url, inherited.livekit_url);
    assert_eq!(selected.livekit_api_key, inherited.livekit_api_key);
    assert_eq!(selected.livekit_api_secret, inherited.livekit_api_secret);
}

#[tokio::test]
async fn guest_and_signed_in_interviews_cannot_share_a_live_room() {
    let state = Arc::new(Mutex::new(DispatchState::default()));
    let dispatcher = LocalDispatcher {
        config: crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap(),
        runtime: tokio::runtime::Handle::current(),
        state: Arc::clone(&state),
        max_concurrent: 4,
    };

    // A guest must not join a room already reserved by a signed-in interview.
    let signed_in_slot = match dispatcher.reserve_with_kind("shared-room", false) {
        Reservation::New(slot) => slot,
        _ => panic!("signed-in reservation should succeed"),
    };
    assert!(matches!(
        dispatcher.reserve_with_kind("shared-room", true),
        Reservation::GuestRoomInUse
    ));
    drop(signed_in_slot);

    // A signed-in user must not join a room reserved by a guest.
    let guest_slot = match dispatcher.reserve_with_kind("shared-room", true) {
        Reservation::New(slot) => slot,
        _ => panic!("guest reservation should succeed after release"),
    };
    assert!(matches!(
        dispatcher.reserve_with_kind("shared-room", false),
        Reservation::GuestRoomInUse
    ));
    drop(guest_slot);

    assert!(state.lock().unwrap().live.is_empty());
    assert!(state.lock().unwrap().guest_live.is_empty());
}

#[tokio::test]
async fn a_room_finalizing_its_report_cannot_be_reused_for_a_new_interview() {
    let state = Arc::new(Mutex::new(DispatchState::default()));
    let dispatcher = LocalDispatcher {
        config: crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap(),
        runtime: tokio::runtime::Handle::current(),
        state: Arc::clone(&state),
        max_concurrent: 2,
    };
    let Reservation::New(slot) = dispatcher.reserve_with_kind("local-room", false) else {
        panic!("fresh slot");
    };
    assert!(matches!(
        dispatcher.reserve_with_kind("local-room", false),
        Reservation::Existing
    ));
    slot.assessment_finished();
    assert!(matches!(
        dispatcher.reserve_with_kind("local-room", false),
        Reservation::Finalizing
    ));
    assert_eq!(state.lock().unwrap().live.len(), 1);
    // Other rooms can still use the unoccupied capacity.
    let Reservation::New(other) = dispatcher.reserve_with_kind("another-room", false) else {
        panic!("spare slot");
    };
    drop(slot);
    assert!(matches!(
        dispatcher.reserve_with_kind("local-room", false),
        Reservation::New(_)
    ));
    drop(other);
    assert!(state.lock().unwrap().live.is_empty());
}

#[tokio::test]
async fn released_guest_room_can_be_reserved_again_without_losing_its_marker() {
    let state = Arc::new(Mutex::new(DispatchState::default()));
    let dispatcher = LocalDispatcher {
        config: crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap(),
        runtime: tokio::runtime::Handle::current(),
        state: Arc::clone(&state),
        max_concurrent: 4,
    };

    let Reservation::New(first_slot) = dispatcher.reserve_with_kind("guest-reused-room", true)
    else {
        panic!("the first guest reservation should succeed");
    };
    drop(first_slot);

    let Reservation::New(second_slot) = dispatcher.reserve_with_kind("guest-reused-room", true)
    else {
        panic!("the released guest room should be reservable again");
    };

    {
        let state = state.lock().unwrap();
        assert!(state.live.contains_key("guest-reused-room"));
        assert!(state.guest_live.contains("guest-reused-room"));
    }

    assert!(matches!(
        dispatcher.reserve_with_kind("guest-reused-room", false),
        Reservation::GuestRoomInUse
    ));

    drop(second_slot);

    let state = state.lock().unwrap();
    assert!(!state.live.contains_key("guest-reused-room"));
    assert!(!state.guest_live.contains("guest-reused-room"));
}

#[tokio::test]
async fn local_dispatcher_enforces_guest_admission_capacity() {
    let state = Arc::new(Mutex::new(DispatchState::default()));
    let config = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://primary.example"),
        ("LIVEKIT_API_KEY", "primary-key"),
        ("LIVEKIT_API_SECRET", "primary-secret"),
        ("GOOGLE_API_KEY", "primary-google"),
    ])
    .unwrap();

    let dispatcher = LocalDispatcher {
        config,
        runtime: tokio::runtime::Handle::current(),
        state: Arc::clone(&state),
        max_concurrent: 8,
    };

    assert!(dispatcher.supports_guest_admission());

    let first = dispatcher.reserve_with_kind("guest-capacity-one", true);
    let second = dispatcher.reserve_with_kind("guest-capacity-two", true);

    assert!(matches!(first, Reservation::New(_)));
    assert!(matches!(second, Reservation::New(_)));

    let provider = Provider {
        id: crate::config::PRIMARY_PROVIDER_ID.to_string(),
        url: "wss://primary.example".to_string(),
        api_key: "primary-key".to_string(),
        api_secret: "primary-secret".to_string(),
        google_api_keys: vec!["primary-google".to_string()],
    };

    assert_eq!(
        RoomDispatcher::ensure_guest_agent(
            &dispatcher,
            "guest-capacity-three",
            &provider,
            "candidate-three"
        ),
        Err(DispatchRefusal::AtCapacity),
    );
}

/// The lobby's billing notice names video only on this answer, so it has to
/// be the setting the local interviewer will run with, both ways.
#[tokio::test]
async fn candidate_video_reports_the_local_agent_setting() {
    for (value, expected) in [("true", true), ("false", false)] {
        let dispatcher = LocalDispatcher {
            config: crate::config::load_from_pairs([
                ("LIVEKIT_URL", "wss://primary.example"),
                ("LIVEKIT_API_KEY", "primary-key"),
                ("LIVEKIT_API_SECRET", "primary-secret"),
                ("GOOGLE_API_KEY", "primary-google"),
                ("CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED", value),
            ])
            .unwrap(),
            runtime: tokio::runtime::Handle::current(),
            state: Arc::default(),
            max_concurrent: 1,
        };
        assert_eq!(dispatcher.candidate_video(), Some(expected), "{value}");
    }
}

fn release_dispatcher(max_concurrent: usize) -> LocalDispatcher {
    LocalDispatcher {
        config: crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap(),
        runtime: tokio::runtime::Handle::current(),
        state: Arc::default(),
        max_concurrent,
    }
}

fn started(dispatcher: &LocalDispatcher, room: &str, guest: bool, candidate: &str) -> Slot {
    match dispatcher.reserve_for(room, guest, Some(candidate.to_string())) {
        Reservation::New(slot) => slot,
        _ => panic!("{room} should be free to start"),
    }
}

/// The room task's wait for a candidate, driven with no LiveKit room. Nothing
/// is ever sent on the channel, so the wait ends only by release or by the
/// five-minute limit, and `bound` is far inside that limit. `None` is a wait
/// still going when the bound ran out.
async fn wait_within(slot: &Slot, bound: std::time::Duration) -> Option<Option<(String, String)>> {
    let (_sender, mut events) =
        tokio::sync::mpsc::unbounded_channel::<::livekit::prelude::RoomEvent>();
    let mut deferred = Vec::new();
    tokio::time::timeout(
        bound,
        crate::livekit::arriving_candidate(&slot.room_name, &mut events, &mut deferred, Some(slot)),
    )
    .await
    .ok()
    .map(Result::unwrap)
}

async fn waited_out(slot: &Slot) -> Option<(String, String)> {
    wait_within(slot, std::time::Duration::from_secs(5))
        .await
        .expect("a released start must stop waiting at once")
}

/// A second release, and one from a start that is not this one, both find
/// nothing to undo.
#[tokio::test]
async fn a_start_is_released_once_and_only_by_its_own_identity() {
    let dispatcher = release_dispatcher(2);
    let _slot = started(&dispatcher, "interview-once", false, "candidate-a");

    assert!(!dispatcher.release_start("interview-once", "candidate-b"));
    assert!(!dispatcher.release_start("interview-other", "candidate-a"));
    assert!(dispatcher.release_start("interview-once", "candidate-a"));
    assert!(
        !dispatcher.release_start("interview-once", "candidate-a"),
        "a repeated release must do nothing"
    );
}

/// Between a release and the slot dropping, the agent is leaving the room and
/// giving consent back. A token for the room then must not be handed the start
/// as `Existing`, since nobody will staff it, so it waits as a room finishing
/// its report does, until the slot is gone.
#[tokio::test]
async fn a_released_room_is_refused_until_its_slot_drops() {
    let dispatcher = release_dispatcher(2);
    let slot = started(&dispatcher, "interview-leaving", false, "candidate-a");
    assert!(dispatcher.release_start("interview-leaving", "candidate-a"));

    assert!(matches!(
        dispatcher.reserve_for("interview-leaving", false, Some("candidate-b".into())),
        Reservation::Finalizing
    ));
    drop(slot);
    assert!(matches!(
        dispatcher.reserve_for("interview-leaving", false, Some("candidate-b".into())),
        Reservation::New(_)
    ));
}

/// The page can give up while the room task is still joining, before it has
/// reached its wait. The release must still be there when it does.
#[tokio::test]
async fn a_release_before_the_room_task_waits_is_not_lost() {
    let dispatcher = release_dispatcher(1);
    let slot = started(&dispatcher, "interview-early", false, "candidate-a");
    assert!(!slot.is_released());

    assert!(dispatcher.release_start("interview-early", "candidate-a"));

    // Asked before the join and again by the wait after it, so the first asking
    // must not use up what the second needs.
    assert!(slot.is_released());
    assert_eq!(waited_out(&slot).await, None);
    assert!(
        !slot.claim_join(),
        "a candidate arriving after the release must not reopen the start"
    );
}

/// Without a release the wait goes on, so the test above is not passing on a
/// wait that ends by itself.
#[tokio::test]
async fn an_unreleased_start_keeps_waiting() {
    let dispatcher = release_dispatcher(1);
    let slot = started(&dispatcher, "interview-waiting", false, "candidate-a");
    assert!(
        wait_within(&slot, std::time::Duration::from_millis(100))
            .await
            .is_none()
    );
}

/// A join and a release that race agree on one winner, every time. Both run on
/// threads of their own released together, since a serial caller cannot tell
/// a check and a set under one lock from the two under two.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_join_racing_a_release_has_exactly_one_winner() {
    let dispatcher = Arc::new(release_dispatcher(1));
    for round in 0..200 {
        let slot = Arc::new(started(&dispatcher, "interview-race", false, "candidate-a"));
        let gate = Arc::new(std::sync::Barrier::new(2));
        let join = {
            let (slot, gate) = (Arc::clone(&slot), Arc::clone(&gate));
            std::thread::spawn(move || {
                gate.wait();
                slot.claim_join()
            })
        };
        let release = {
            let (dispatcher, gate) = (Arc::clone(&dispatcher), Arc::clone(&gate));
            std::thread::spawn(move || {
                gate.wait();
                dispatcher.release_start("interview-race", "candidate-a")
            })
        };
        let (joined, released) = (join.join().unwrap(), release.join().unwrap());
        assert!(
            joined != released,
            "round {round}: joined={joined} released={released}"
        );
        drop(Arc::into_inner(slot).expect("both threads are done with the slot"));
    }
}

/// A candidate who drops and reconnects mid-interview is still the one who
/// joined, so the start never goes back to a state a release could end.
#[tokio::test]
async fn a_reconnect_does_not_reopen_a_joined_start() {
    let dispatcher = release_dispatcher(1);
    let slot = started(&dispatcher, "interview-joined", false, "candidate-a");

    assert!(slot.claim_join());
    assert!(!dispatcher.release_start("interview-joined", "candidate-a"));
    assert!(slot.claim_join(), "a second arrival is still a join");
    assert!(!dispatcher.release_start("interview-joined", "candidate-a"));
}

/// A fixed development room is handed out again and again, and a token from
/// the start before is valid for hours. It must not end the start after it.
#[tokio::test]
async fn a_token_from_an_earlier_start_in_a_reused_room_releases_nothing() {
    let dispatcher = release_dispatcher(1);
    drop(started(
        &dispatcher,
        "interview-pinned",
        false,
        "candidate-a",
    ));
    let _second = started(&dispatcher, "interview-pinned", false, "candidate-b");

    assert!(!dispatcher.release_start("interview-pinned", "candidate-a"));
    assert!(dispatcher.release_start("interview-pinned", "candidate-b"));
}

/// A reload is handed the room already running, with a token of its own. That
/// token is not the start's, so it cannot end it.
#[tokio::test]
async fn a_second_token_for_a_running_room_cannot_release_it() {
    let dispatcher = release_dispatcher(2);
    let _slot = started(&dispatcher, "interview-shared", false, "candidate-a");
    assert!(matches!(
        dispatcher.reserve_for("interview-shared", false, Some("candidate-c".into())),
        Reservation::Existing
    ));

    assert!(!dispatcher.release_start("interview-shared", "candidate-c"));
    assert!(dispatcher.release_start("interview-shared", "candidate-a"));
}

/// A released guest start gives its place under the cap of two back now, not
/// when the five-minute wait would have run out.
#[tokio::test]
async fn a_released_guest_start_gives_its_guest_slot_back() {
    let dispatcher = release_dispatcher(8);
    let first = started(&dispatcher, "guest-released", true, "candidate-a");
    let _second = started(&dispatcher, "guest-kept", true, "candidate-b");
    assert!(matches!(
        dispatcher.reserve_with_kind("guest-third", true),
        Reservation::Full {
            guest_cap: true,
            ..
        }
    ));

    assert!(dispatcher.release_start("guest-released", "candidate-a"));
    assert_eq!(waited_out(&first).await, None);
    settle_interview(Ok(crate::livekit::RoomRunOutcome::NoCandidate), None, first).await;

    assert!(
        !dispatcher
            .state
            .lock()
            .unwrap()
            .guest_live
            .contains("guest-released")
    );
    assert!(matches!(
        dispatcher.reserve_with_kind("guest-third", true),
        Reservation::New(_)
    ));
}

/// A released signed-in start hands its consent claim back through the same
/// no-show path a candidate who never came takes, and its slot with it.
#[tokio::test]
async fn a_released_start_gives_its_consent_and_slot_back() {
    let (interview_id, room_name) = ("interview-released-start", "interview-released-room");
    let (_path, accounts, account_id) =
        claimed_interview("dispatch-released-start", -2, interview_id, room_name);

    let dispatcher = release_dispatcher(1);
    let slot = started(&dispatcher, room_name, false, "candidate-a");
    assert!(dispatcher.release_start(room_name, "candidate-a"));
    assert_eq!(waited_out(&slot).await, None);
    let no_show = NoShowConsent::new(
        Arc::clone(&accounts),
        interview_id.into(),
        account_id,
        room_name.into(),
    );
    settle_interview(
        Ok(crate::livekit::RoomRunOutcome::NoCandidate),
        Some(no_show),
        slot,
    )
    .await;

    let room: Option<String> = accounts
        .with(|connection| {
            connection.query_row(
                "SELECT room_name FROM interviews WHERE id = ?1",
                [interview_id],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(room, None, "the consent claim must be released");
    assert!(dispatcher.state.lock().unwrap().live.is_empty());
    assert!(dispatcher.state.lock().unwrap().starts.is_empty());
    drop(accounts);
}

/// Closing is part of the one admission decision, so after it no way of
/// starting a room gets in, while a room already admitted is still served and
/// still counts as live until it ends.
#[tokio::test]
async fn a_closed_dispatcher_admits_no_new_room_but_keeps_the_ones_it_has() {
    let dispatcher = release_dispatcher(8);
    let provider = Provider {
        id: crate::config::PRIMARY_PROVIDER_ID.to_string(),
        url: "wss://primary.example".to_string(),
        api_key: "primary-key".to_string(),
        api_secret: "primary-secret".to_string(),
        google_api_keys: Vec::new(),
    };
    let admitted = started(&dispatcher, "interview-before-close", false, "candidate");
    assert!(!dispatcher.is_idle());

    dispatcher.close_admission();

    assert!(matches!(
        dispatcher.reserve_with_kind("interview-after-close", false),
        Reservation::Closed
    ));
    assert!(matches!(
        dispatcher.reserve_with_kind("guest-after-close", true),
        Reservation::Closed
    ));
    assert_eq!(
        dispatcher.ensure_agent("interview-via-ensure", &provider, AgentJob::Interview),
        Err(DispatchRefusal::Stopping)
    );
    assert_eq!(
        dispatcher.ensure_guest_agent("guest-via-ensure", &provider, "candidate"),
        Err(DispatchRefusal::Stopping)
    );
    assert_eq!(
        dispatcher.ensure_agent_with_no_show_release("no-show", &provider, "candidate", None),
        Err(DispatchRefusal::Stopping)
    );
    // The room that was admitted is still the dispatcher's, and still live.
    assert!(matches!(
        dispatcher.reserve_with_kind("interview-before-close", false),
        Reservation::Existing
    ));
    assert!(!dispatcher.is_idle());
    drop(admitted);
    assert!(dispatcher.is_idle());
    assert_eq!(
        dispatcher.state.lock().unwrap().live.len(),
        0,
        "refusals left no room behind"
    );
}

#[test]
fn a_dispatcher_with_nothing_to_close_ignores_the_request() {
    struct Plain;
    impl RoomDispatcher for Plain {
        fn ensure_agent(&self, _: &str, _: &Provider, _: AgentJob) -> Result<(), DispatchRefusal> {
            Ok(())
        }
    }
    let provider = Provider {
        id: crate::config::PRIMARY_PROVIDER_ID.to_string(),
        url: "wss://primary.example".to_string(),
        api_key: "primary-key".to_string(),
        api_secret: "primary-secret".to_string(),
        google_api_keys: Vec::new(),
    };
    let plain = Plain;
    plain.close_admission();
    assert_eq!(
        plain.ensure_agent("room", &provider, AgentJob::Interview),
        Ok(())
    );
}
