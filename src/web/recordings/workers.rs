//! The background halves of recording, which no request drives.
//!
//! A sweeper that retries and expires, and a delivery worker that moves a
//! finished artifact to its destination. Named apart from the rest of
//! `recordings` because nothing here answers a caller: these run on a timer
//! against whatever the database holds, and their failures are logged rather
//! than returned.
//!
//! Split out of `recordings.rs` along the line its module doc already drew.

use std::sync::Arc;
use std::time::Duration;

use crate::accounts::Accounts;

/// How often the sweeper runs. The shortest retry backoff, because a schedule
/// checked less often than its own first step is a schedule with a different
/// first step.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// The timers this module owns, stopped when the server that started them is.
///
/// Held rather than detached. A router is built per test and more than one can
/// share a runtime, so a sweeper nothing aborted went on scanning a database
/// its own server had finished with, and a delivery worker went on uploading
/// from it. `QuotaRefresher` is the same shape for the same reason.
#[derive(Default)]
pub(crate) struct RecordingWorkers(Vec<tokio::task::JoinHandle<()>>);

impl Drop for RecordingWorkers {
    fn drop(&mut self) {
        for handle in self.0.drain(..) {
            handle.abort();
        }
    }
}

/// Starts the sweeper and the delivery worker, which always run as a pair.
///
/// The sweeper resolves recordings nobody is looking after, now and then
/// repeatedly: now, because a process that died mid-interview left rows no
/// request will ever touch again, and repeatedly, because the retry schedule is
/// minutes long and a sweep that only ran at startup would turn every provider
/// hiccup into a failed recording. The delivery worker moves what the sweeper
/// finds finished.
///
/// Silent when there is no runtime to spawn on. `web_router` is public and can
/// be built outside one; a router that serves without these is degraded, and a
/// panic at construction is worse.
pub(crate) fn spawn_recording_workers(
    accounts: Arc<Accounts>,
    recorder: crate::recording::Recorder,
) -> RecordingWorkers {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        eprintln!("recording is configured but there is no runtime to sweep or deliver on");
        return RecordingWorkers::default();
    };
    let mut workers = RecordingWorkers::default();

    let sweeping = (accounts.clone(), recorder.clone());
    workers.0.push(handle.spawn(async move {
        let (accounts, recorder) = sweeping;
        loop {
            let outcome = crate::recording::sweep_recordings(&accounts, &recorder).await;
            if let Some(line) = sweep_line(&outcome) {
                eprintln!("{line}");
            }
            tokio::time::sleep(SWEEP_INTERVAL).await;
        }
    }));

    // A deployment whose credentials do not build a delivery client records and
    // hands nothing over, which `delivery_provider` has already warned about.
    // The sweeper still runs: it is what expires those rows.
    if let Some(delivery) = recorder.delivery.clone() {
        workers.0.push(handle.spawn(async move {
            loop {
                let outcome = crate::recording::run_delivery_queue(
                    &accounts,
                    &recorder,
                    delivery.as_ref() as &dyn crate::recording::DeliveryProvider,
                )
                .await;
                if let Some(line) = delivery_line(&outcome) {
                    eprintln!("{line}");
                }
                tokio::time::sleep(DELIVERY_INTERVAL).await;
            }
        }));
    }
    workers
}

/// What a sweep is worth saying, or nothing when it did nothing.
///
/// A pass that finds no work is the ordinary case and runs every minute, so
/// saying so would bury the passes that moved something. Pulled out of the loop
/// for the reason `quota_change_line` is out of its own: the line deciding
/// whether an operator hears about a sweep is worth pinning, and inside a
/// spawned loop nothing could reach it.
fn sweep_line(outcome: &crate::recording::SweepOutcome) -> Option<String> {
    (*outcome != crate::recording::SweepOutcome::default()).then(|| {
        format!(
            "recording sweep retried {} deleted {} failed {}",
            outcome.retried, outcome.deleted, outcome.failed
        )
    })
}

/// The same for a delivery pass.
fn delivery_line(outcome: &crate::recording::DeliveryOutcome) -> Option<String> {
    (*outcome != crate::recording::DeliveryOutcome::default()).then(|| {
        format!(
            "recording delivery delivered {} retrying {} failed {}",
            outcome.delivered, outcome.retrying, outcome.failed
        )
    })
}

/// How often the delivery queue is asked whether anything is due.
///
/// Ten seconds, because the queue's own `run_after` is what schedules a retry
/// and this only decides how late the first attempt can be. A recording that
/// waits ten seconds for its upload to begin is a recording nobody noticed
/// waiting.
const DELIVERY_INTERVAL: Duration = Duration::from_secs(10);

/// The delivery client, or a warning and none.
///
/// A `None` here is a deployment that records and never hands anything over,
/// which the binary refuses to start in: `codetrial web` parses the key before
/// it listens. It is reachable from the public router
/// constructors, which is what the tests build, and there a warning is the
/// right answer rather than a panic inside a constructor.
pub(crate) fn delivery_provider(
    recording: &crate::config::RecordingConfig,
) -> Option<Arc<dyn crate::recording::DeliveryProvider>> {
    match crate::delivery::GoogleDelivery::new(
        &recording.service_account_json,
        &recording.gcs_bucket,
        &recording.drive_id,
        Arc::new(|| crate::current_epoch_seconds() as i64),
    ) {
        Ok(delivery) => Some(Arc::new(delivery)),
        Err(error) => {
            eprintln!("WARNING: recordings cannot be delivered or deleted: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
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
                // Never finishes on its own, so anything that ends it is the
                // abort.
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

        // The abort lands at each task's next scheduling point rather than
        // inside `drop`, so this yields rather than asserting straight away.
        for _ in 0..100 {
            if watches.iter().all(|watch| watch.is_finished()) {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("a task outlived the RecordingWorkers that owned it");
    }
}
