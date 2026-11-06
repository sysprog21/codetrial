use super::*;
use crate::accounts::peer::{Settings, create};
use std::sync::atomic::{AtomicUsize, Ordering};

fn accounts() -> (Arc<Accounts>, String) {
    let accounts = Accounts::open(crate::accounts::GitHubLoginConfig {
        oauth: None,
        session_secret: "sweep-test".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    accounts.initialize_schema().unwrap();
    accounts.with(|c|c.execute_batch("INSERT INTO users(id,github_id,login,created_at,updated_at)VALUES(1,1,'sweep',1,1)")).unwrap();
    let state = create(
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
    let id = state["sessionId"].as_str().unwrap().to_owned();
    (Arc::new(accounts), id)
}

async fn stub(
    status: axum::http::StatusCode,
) -> (
    crate::config::ProviderPool,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let app = axum::Router::new().route(
        "/twirp/livekit.RoomService/DeleteRoom",
        axum::routing::post(move || {
            let count = Arc::clone(&count);
            async move {
                count.fetch_add(1, Ordering::Relaxed);
                (status, axum::Json(serde_json::json!({})))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (
        crate::config::ProviderPool {
            providers: vec![crate::config::Provider {
                id: "primary".into(),
                url,
                api_key: "key".into(),
                api_secret: "secret".into(),
                google_api_keys: vec![],
            }],
        },
        calls,
        task,
    )
}

#[tokio::test]
async fn sweep_deletes_a_closed_room_once_and_retries_failures_after_backoff() {
    for status in [
        axum::http::StatusCode::OK,
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
    ] {
        let (accounts, id) = accounts();
        accounts
            .with(|c| {
                let mut s = peer::load(c, &id)?.unwrap();
                s.status = "cancelled".into();
                s.room_name = Some("interview-peerroom".into());
                s.provider_id = Some("primary".into());
                s.room_generation = 1;
                peer::save(c, &s)
            })
            .unwrap();
        let (pool, calls, server) = stub(status).await;
        sweep_once(Arc::clone(&accounts), &pool, 200).await.unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let session = accounts.with(|c| peer::load(c, &id)).unwrap().unwrap();
        assert_eq!(session.delete_attempts, 1);
        if status.is_success() {
            assert_eq!(session.room_deleted_at, Some(200));
        } else {
            assert!(session.room_deleted_at.is_none());
            assert_eq!(session.next_delete_at, 260, "30s doubled once");
        }
        sweep_once(Arc::clone(&accounts), &pool, 201).await.unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        if !status.is_success() {
            sweep_once(Arc::clone(&accounts), &pool, session.next_delete_at)
                .await
                .unwrap();
            assert_eq!(calls.load(Ordering::Relaxed), 2);
        }
        server.abort();
    }
}

#[tokio::test]
async fn sweep_settles_expired_sessions_without_polling() {
    let (accounts, id) = accounts();
    let expires = accounts
        .with(|c| peer::load(c, &id))
        .unwrap()
        .unwrap()
        .expires_at;
    sweep_once(
        Arc::clone(&accounts),
        &crate::config::ProviderPool::default(),
        expires,
    )
    .await
    .unwrap();
    assert_eq!(
        accounts
            .with(|c| peer::load(c, &id))
            .unwrap()
            .unwrap()
            .status,
        "cancelled"
    );
}

#[tokio::test]
async fn old_closed_practice_is_removed_as_the_sweeper_starts() {
    let (accounts, id) = accounts();
    accounts
        .with(|c| {
            c.execute(
                "UPDATE peer_sessions SET status='cancelled', created_at=1, room_name=NULL WHERE id=?1",
                [&id],
            )
        })
        .unwrap();
    let _tasks = spawn_sweeper(
        Arc::clone(&accounts),
        crate::config::ProviderPool::default(),
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while accounts.with(|c| peer::load(c, &id)).unwrap().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the first prune runs before the hourly wait");
}

#[tokio::test]
async fn dropping_background_tasks_aborts_the_owned_sweeper() {
    let (accounts, _) = accounts();
    assert_eq!(Arc::strong_count(&accounts), 1);
    let tasks = spawn_sweeper(
        Arc::clone(&accounts),
        crate::config::ProviderPool::default(),
    );

    // The retention task prunes once as it starts, and that pass holds one more
    // handle until it ends.
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&accounts) != 4 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the sweeper keeps one handle per task");
    drop(tasks);
    tokio::task::yield_now().await;
    assert_eq!(Arc::strong_count(&accounts), 1);
}

#[tokio::test]
async fn a_stalled_waiting_room_delete_cannot_delay_round_settlement() {
    let (accounts, id) = accounts();
    accounts
        .with(|c| {
            let mut session = peer::load(c, &id)?.unwrap();
            session.status = "active".into();
            session.hard_deadline = Some(200);
            peer::save(c, &session)
        })
        .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let observed = Arc::clone(&started);
    let app = axum::Router::new().route(
        "/twirp/livekit.RoomService/DeleteRoom",
        axum::routing::post(move || {
            let observed = Arc::clone(&observed);
            async move {
                observed.notify_one();
                std::future::pending::<()>().await;
                axum::Json(serde_json::json!({}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let pool = crate::config::ProviderPool {
        providers: vec![crate::config::Provider {
            id: "primary".into(),
            url,
            api_key: "key".into(),
            api_secret: "secret".into(),
            google_api_keys: vec![],
        }],
    };
    accounts.with(|c|{
        let settings=serde_json::to_string(&peer::load(c,&id)?.unwrap().settings).unwrap();
        c.execute("INSERT INTO peer_availability(availability_id,user_id,status,settings_json,provider_id,room_name,participant_identity,created_at,expires_at) VALUES('stalled',1,'expired',?1,'primary','interview-stalled','waiting-identity',1,2)",[settings]).map(|_|())
    }).unwrap();
    let db = Arc::clone(&accounts);
    let cleanup = tokio::spawn(async move { sweep_once(db, &pool, 200).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    let status = accounts
        .with(|c| peer::load(c, &id))
        .unwrap()
        .unwrap()
        .status;
    assert_eq!(
        status, "finalizing",
        "deadline settlement must precede cleanup"
    );
    assert!(!cleanup.is_finished(), "the deletion stub is still stalled");
    let settings = accounts
        .with(|c| peer::load(c, &id))
        .unwrap()
        .unwrap()
        .settings;
    let second = peer::create(
        &accounts,
        1,
        settings.clone(),
        &crate::config::PeerConfig::default(),
        201,
    )
    .unwrap()
    .unwrap();
    let second_id = second["sessionId"].as_str().unwrap();
    accounts
        .with(|c| {
            let mut session = peer::load(c, second_id)?.unwrap();
            session.status = "active".into();
            session.hard_deadline = Some(300);
            peer::save(c, &session)
        })
        .unwrap();
    peer::discovery::set_profile(&accounts, 1, "Sweeper", 201)
        .unwrap()
        .unwrap();
    let available = peer::discovery::prepare(
        &accounts,
        1,
        settings,
        Some(("interview-fresh", "primary")),
        &crate::config::PeerConfig::default(),
        201,
    )
    .unwrap()
    .unwrap();
    peer::discovery::publish(&accounts, &available.id, 1, true, 201)
        .unwrap()
        .unwrap();
    settle_once(Arc::clone(&accounts), 300).await.unwrap();
    assert_eq!(
        accounts
            .with(|c| peer::load(c, second_id))
            .unwrap()
            .unwrap()
            .status,
        "finalizing"
    );
    assert!(
        peer::discovery::lobby(&accounts, 2, 300).unwrap()["available"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    cleanup.abort();
    server.abort();
    assert_eq!(
        status, "finalizing",
        "deadline settlement must happen before any room cleanup awaits the network"
    );
}

#[tokio::test]
async fn cleanup_limits_in_flight_deletes_to_four() {
    let (accounts, first) = accounts();
    let settings = accounts
        .with(|c| peer::load(c, &first))
        .unwrap()
        .unwrap()
        .settings;
    let mut ids = vec![first];
    for _ in 0..4 {
        ids.push(
            peer::create(
                &accounts,
                1,
                settings.clone(),
                &crate::config::PeerConfig::default(),
                100,
            )
            .unwrap()
            .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    accounts
        .with(|c| {
            for (index, id) in ids.iter().enumerate() {
                let mut s = peer::load(c, id)?.unwrap();
                s.status = "cancelled".into();
                s.provider_id = Some("primary".into());
                s.room_name = Some(format!("interview-cleanup-{index}"));
                peer::save(c, &s)?;
            }
            Ok(())
        })
        .unwrap();
    let (requests, mut pending) = tokio::sync::mpsc::channel::<tokio::sync::oneshot::Sender<()>>(8);
    let app = axum::Router::new().route(
        "/twirp/livekit.RoomService/DeleteRoom",
        axum::routing::post(move || {
            let requests = requests.clone();
            async move {
                let (release, wait) = tokio::sync::oneshot::channel();
                requests.send(release).await.unwrap();
                let _ = wait.await;
                axum::Json(serde_json::json!({}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let pool = crate::config::ProviderPool {
        providers: vec![crate::config::Provider {
            id: "primary".into(),
            url,
            api_key: "key".into(),
            api_secret: "secret".into(),
            google_api_keys: vec![],
        }],
    };
    let db = Arc::clone(&accounts);
    let cleanup = tokio::spawn(async move { cleanup_once(db, &pool, 200).await });
    let mut releases = Vec::new();
    for _ in 0..4 {
        releases.push(
            tokio::time::timeout(std::time::Duration::from_secs(2), pending.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert!(matches!(
        pending.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    releases.pop().unwrap().send(()).unwrap();
    releases.push(
        tokio::time::timeout(std::time::Duration::from_secs(2), pending.recv())
            .await
            .unwrap()
            .unwrap(),
    );
    for release in releases {
        release.send(()).unwrap();
    }
    cleanup.await.unwrap().unwrap();
    server.abort();
    accounts
        .with(|c| {
            for id in ids {
                assert_eq!(peer::load(c, &id)?.unwrap().room_deleted_at, Some(200));
            }
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn a_stalled_delete_times_out_and_keeps_a_durable_retry() {
    let (accounts, id) = accounts();
    accounts
        .with(|c| {
            let mut s = peer::load(c, &id)?.unwrap();
            s.status = "cancelled".into();
            s.provider_id = Some("primary".into());
            s.room_name = Some("interview-timeout".into());
            peer::save(c, &s)
        })
        .unwrap();
    let app = axum::Router::new().route(
        "/twirp/livekit.RoomService/DeleteRoom",
        axum::routing::post(|| async {
            std::future::pending::<()>().await;
            axum::Json(serde_json::json!({}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let pool = crate::config::ProviderPool {
        providers: vec![crate::config::Provider {
            id: "primary".into(),
            url,
            api_key: "key".into(),
            api_secret: "secret".into(),
            google_api_keys: vec![],
        }],
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        cleanup_once(Arc::clone(&accounts), &pool, 200),
    )
    .await
    .unwrap()
    .unwrap();
    let session = accounts.with(|c| peer::load(c, &id)).unwrap().unwrap();
    assert!(session.room_deleted_at.is_none());
    assert_eq!(session.delete_attempts, 1);
    assert!(session.next_delete_at > 200);
    server.abort();
}

#[test]
fn claiming_a_closed_room_leases_it_for_thirty_seconds() {
    let (accounts, id) = accounts();
    accounts
        .with(|c| {
            let mut s = peer::load(c, &id)?.unwrap();
            s.status = "cancelled".into();
            s.room_name = Some("interview-peerroom".into());
            s.provider_id = Some("primary".into());
            s.next_delete_at = 0;
            peer::save(c, &s)
        })
        .unwrap();
    assert_eq!(claim_rooms(&accounts, 500).unwrap().len(), 1);
    let session = accounts.with(|c| peer::load(c, &id)).unwrap().unwrap();
    assert_eq!(session.next_delete_at, 530);
    assert!(claim_rooms(&accounts, 501).unwrap().is_empty());
}
