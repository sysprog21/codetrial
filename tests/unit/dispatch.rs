//! The `tests` module of `src/dispatch.rs`, which declares this file by path.
//! Everything here reaches into `src/dispatch.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use super::*;

/// The capacity slot is released by dropping, so an interview that panics
/// gives its slot back like one that ends. A cap's worth of leaked slots
/// would refuse every interview after them for the life of the process.
#[tokio::test]
async fn a_panicking_interview_gives_its_capacity_back() {
    let live = Arc::new(Mutex::new(HashSet::new()));
    live.lock().unwrap().insert("interview-boom".to_string());
    let slot = Slot {
        live: Arc::clone(&live),
        room_name: "interview-boom".to_string(),
    };

    let task = tokio::spawn(async move {
        let _held = slot;
        panic!("the interview blew up");
    });
    assert!(task.await.is_err(), "the task must have panicked");

    assert!(
        live.lock().unwrap().is_empty(),
        "a panicking interview must not keep its slot"
    );
}

/// The cap is the configured one, not a constant. An operator who set it to
/// one gets one, and the refusal names the number they chose.
#[tokio::test]
async fn the_configured_cap_is_the_one_enforced() {
    let live = Arc::new(Mutex::new(HashSet::new()));
    live.lock().unwrap().insert("interview-first".to_string());
    let dispatcher = LocalDispatcher {
        config: crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap(),
        runtime: tokio::runtime::Handle::current(),
        live: Arc::clone(&live),
        max_concurrent: 1,
    };
    let provider = Provider {
        id: crate::config::PRIMARY_PROVIDER_ID.to_string(),
        url: "wss://primary.example".to_string(),
        api_key: "primary-key".to_string(),
        api_secret: "primary-secret".to_string(),
        google_api_key: String::new(),
    };

    assert!(
        !dispatcher.ensure_agent("interview-second", &provider),
        "a second room must be refused at a cap of one"
    );

    // The room already running is still admitted, because a reload asks for the
    // same room and refusing it would break the page that is open.
    assert!(dispatcher.ensure_agent("interview-first", &provider));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_concurrent_burst_never_overbooks_and_released_capacity_returns() {
    let live = Arc::new(Mutex::new(HashSet::new()));
    let dispatcher = LocalDispatcher {
        config: crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap(),
        runtime: tokio::runtime::Handle::current(),
        live: Arc::clone(&live),
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
            match dispatcher.reserve(&format!("interview-burst-{index}")) {
                Reservation::New(slot) => Some(slot),
                Reservation::Full => None,
                Reservation::Existing => panic!("burst ids are unique"),
            }
        }));
    }
    let mut held = Vec::new();
    for task in tasks {
        held.extend(task.await.unwrap());
    }
    assert_eq!(held.len(), 8);
    assert_eq!(live.lock().unwrap().len(), 8);

    // Named from a slot that actually won: which eight of the hundred get in is
    // up to the scheduler, so the serial loop's assumption that it is the first
    // eight does not survive running them at once.
    let winner = held[0].room_name.clone();
    assert!(matches!(dispatcher.reserve(&winner), Reservation::Existing));
    held.truncate(3);
    assert_eq!(live.lock().unwrap().len(), 3);
    for index in 100..105 {
        assert!(matches!(
            dispatcher.reserve(&format!("interview-burst-{index}")),
            Reservation::New(_)
        ));
    }
    // The temporary slots above drop at the end of each assertion.
    assert_eq!(live.lock().unwrap().len(), 3);
}

/// A provider's own Google key wins, and an empty one leaves the base key
/// alone rather than blanking it.
#[test]
fn provider_credentials_replace_the_base_without_blanking_the_key() {
    let base = crate::config::load_from_pairs([
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
        google_api_key: String::new(),
    };

    let inherited = agent_config_for(&base, &provider);
    assert_eq!(inherited.livekit_url, "wss://second.example");
    assert_eq!(inherited.google_api_key, "primary-google");

    provider.google_api_key = "second-google".to_string();
    assert_eq!(
        agent_config_for(&base, &provider).google_api_key,
        "second-google"
    );
}
