//! The `tests` module of `src/web/recordings/workers.rs`, which declares this
//! file by path.
//! Everything here reaches into `src/web/recordings/workers.rs` through
//! `super`, so it is a unit
//! test and not an integration test: private items are in scope.

use super::*;

/// A throwaway RSA key, generated for this test and used nowhere else. The
/// delivery client parses the key material at construction, so asserting
/// that a configured deployment gets a client needs one that parses.
const TEST_ONLY_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDoSml5zRLkE3B9\nK2p8uBW7Xeccy5KN5xOcCBIJ914tq32oMvPmgmOsI0EqbswJe05Csknu6XtDEWv3\na6dvFgYxnE8EvHmtWJedpJC2HbkWaHrNmh6rk6A9/D/AGGUijL6bZoWwuM3neNF7\ng4V+3YS5itPa+iAEeCH6ng+78bNNGaxZ4NxyqHqBa0yTFxKEpJNJt1r6fDl5LBBu\ngPwbG4VCHhG10uqN4ZNiMb/YjODT0nBGYz9ffR0FEooLpj9hU8olBeVekG2/RlNO\nWkahsuo6nWTWPBWo3Omi+t6ig0Eeic5ExEU1J0jSdAEByjzMMXOVbGle3EjzaNE7\nZy2kZD2zAgMBAAECggEAAZc8F8kgjAbQTJtsPplIwv2UnigBQ30OpDvIXxIDaQaM\n/g+a/Hr9y9HY3TSUaDdNrU1/Z7KRI8+QPa7ksyuPRCQhOaE39D2G5mJHbB+27rOv\njf4pAnyP8DDxKaEMXgLV27+0g7oc5QtDA7ZUuX8g0u1NVgrhPJ69Jhg4w/6Ews5c\nBKvztBawn7hh3WNpqD6TLm4+FXVQi/rvW0hD20nTsb9yM1+18m78YmF6ObPm6pdu\nq7/pp4QZ6nm4VS2zEdTuw+oWmvwRc0F10JCsmVc5noVm/q9oqc6gVYUX2oj7CR2T\nnKDhocF7xnUFJJKKJcP48CerIg16h44i8sGcl/GZsQKBgQD90JHwTa+zon41/jO9\n+najINdtH78C9cBWctnMtEjDyd11lSOdxD18uHIjHpgF+WWstwyT60w2ECAvTOhe\n+ANQeI+WKE3tIiq3QQHlChc0UH5KZan0G5QPq6pQWr9Pj5+7uNe8eMKTEQf+mCEt\ntv5aUc4gQVBdtWR5qFvUxtdEnwKBgQDqSmaqUKNkAerNmd0ChHuuGZzExrAZ0Kum\nQ+tnamLbXs3wR4rmxhEUOO2PGqbd/n3Q+cK+1UdSXRZhDq32xjflzo9AK3ffPuH1\newYPTkwHBChTXrAp3ViFn5l20NOE7VTu2DdrHKUz4MpnYEE3LVQFUUg7H+EqwfM5\nGqP3DNA6bQKBgQC1o+7dD2ufXbl/AGWdHsKKabVh5ec3whGcjGLsCVVNsIhpXor3\nm/n46LLeCUX4eIvX98Prk+edhRrTXvGpDUqp6y2u4zcpblstfDtT403J5ZULvwfK\np3XlZQ/ko5zn3jwNBvJ1ceKlhvm2rL6JzbznfEXMdZGDDo5SNjdJ5ecmtwKBgBc6\n3UcRy8GEtyU/ljxDqoeunm6cTKWinQJVRafxUm/xzHWAgnMzPEpHArbnq5fjPdJU\nkUyelP3DoQ5qiDEpoi0099si9DW8ZGcUlZs65irj7KOnhcwA2GAXXP384pwRdBRi\nd8w1AORN64OodY7k/amxT3odRRQaOuV0kMFUEelZAoGAZX6IVBljoQHlrMURpamm\nfD19TtC1Ggdl5k1raWY4dBIZv+mgETBhb9etYp8VHYinBV7SeKGt6KrU4eZDTe3h\nJMlxhF00N0zksKn37lWNJ3ayeBsiDoVHfsfIU9OpkT4j9it8J/W3WUAFqG3Lc8lv\nOgA+s0vjN/n+Kc5h8HNZONs=\n-----END PRIVATE KEY-----";

/// A deployment with credentials gets a delivery client, and one without
/// gets a warning.
///
/// The `None` is a real state: the recording still runs and the sweeper
/// still resolves rows, but nothing delivers or deletes. That is worth a
/// warning and not a panic inside a router constructor, and both halves
/// were decided here with nothing asserting either.
#[test]
fn a_delivery_client_is_built_only_when_the_credentials_are_there() {
    let mut recording = crate::config::RecordingConfig {
        livekit: None,
        gcs_bucket: "codetrial-staging".to_string(),
        gcs_prefix: "codetrial".to_string(),
        drive_id: "0AKfixtureDriveId".to_string(),
        service_account_json: serde_json::json!({
            "type": "service_account",
            "client_email": "codetrial@example.iam.gserviceaccount.com",
            "private_key": TEST_ONLY_PRIVATE_KEY,
        })
        .to_string(),
        max_minutes: 45,
        bitrate: 2000,
        kill_switch: false,
        template_base_url: "https://recording.codetrial.example".to_string(),
    };
    assert!(
        delivery_provider(&recording).is_some(),
        "a service account with an address and a key builds a client"
    );

    recording.service_account_json = "{}".to_string();
    assert!(
        delivery_provider(&recording).is_none(),
        "a service account naming nobody delivers nothing"
    );
}

/// A pass that did nothing says nothing, and a pass that did says what.
///
/// Both loops run on a timer against whatever the database holds, so the
/// quiet case is the common one and logging it would bury the passes worth
/// reading. The counts are in the line because an operator reading it has
/// no other view of what the sweep moved.
#[test]
fn a_worker_pass_reports_only_when_it_moved_something() {
    use crate::recording::{DeliveryOutcome, SweepOutcome};

    assert_eq!(sweep_line(&SweepOutcome::default()), None);
    assert_eq!(delivery_line(&DeliveryOutcome::default()), None);

    let swept = SweepOutcome {
        retried: 1,
        deleted: 2,
        failed: 3,
    };
    assert_eq!(
        sweep_line(&swept).as_deref(),
        Some("recording sweep retried 1 deleted 2 failed 3")
    );

    let delivered = DeliveryOutcome {
        delivered: 4,
        retrying: 5,
        failed: 6,
    };
    assert_eq!(
        delivery_line(&delivered).as_deref(),
        Some("recording delivery delivered 4 retrying 5 failed 6")
    );

    // One field is enough to make a pass worth reporting, whichever it is.
    for outcome in [
        SweepOutcome {
            retried: 1,
            ..SweepOutcome::default()
        },
        SweepOutcome {
            deleted: 1,
            ..SweepOutcome::default()
        },
        SweepOutcome {
            failed: 1,
            ..SweepOutcome::default()
        },
    ] {
        assert!(sweep_line(&outcome).is_some(), "{outcome:?}");
    }
}

/// The guard exists so a sweeper cannot outlive the server that wanted it.
/// Nothing would prove that: `drop` could be empty and every other test
/// still passes, which is exactly the leak this type was added to stop.
#[tokio::test]
async fn dropping_the_workers_aborts_the_tasks_they_own() {
    let mut workers = RecordingWorkers::default();
    let watches: Vec<_> = (0..2)
        .map(|_| {
            // Never finishes on its own, so anything that ends it is the abort.
            let task = tokio::spawn(async {
                loop {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }
            });
            let watch = task.abort_handle();
            workers.0.push(task);
            watch
        })
        .collect();
    assert!(
        watches.iter().all(|watch| !watch.is_finished()),
        "both tasks are running before the drop"
    );

    drop(workers);

    // The abort lands at each task's next scheduling point rather than inside
    // `drop`, so this yields rather than asserting straight away.
    for _ in 0..100 {
        if watches.iter().all(|watch| watch.is_finished()) {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("a task outlived the RecordingWorkers that owned it");
}
