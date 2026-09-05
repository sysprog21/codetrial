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

/// Resolves recordings nobody is looking after, now and then repeatedly.
///
/// Now, because a process that died mid-interview left rows no request will
/// ever touch again. Repeatedly, because the retry schedule is minutes long and
/// a sweep that only ran at startup would turn every provider hiccup into a
/// failed recording.
///
/// Silent when there is no runtime to spawn on. `web_router` is public and can
/// be built outside one; a router that serves without a sweeper is degraded,
/// and a panic at construction is worse.
pub(crate) fn spawn_recording_sweeper(
    accounts: Arc<Accounts>,
    recorder: crate::recording::Recorder,
) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        eprintln!("recording is configured but there is no runtime to sweep on");
        return;
    };
    handle.spawn(async move {
        loop {
            let outcome = crate::recording::sweep_recordings(&accounts, &recorder).await;
            if outcome != crate::recording::SweepOutcome::default() {
                eprintln!(
                    "recording sweep retried {} deleted {} failed {}",
                    outcome.retried, outcome.deleted, outcome.failed
                );
            }
            tokio::time::sleep(SWEEP_INTERVAL).await;
        }
    });
}

/// How often the delivery queue is asked whether anything is due.
///
/// Ten seconds, because the queue's own `run_after` is what schedules a retry
/// and this only decides how late the first attempt can be. A recording that
/// waits ten seconds for its upload to begin is a recording nobody noticed
/// waiting.
const DELIVERY_INTERVAL: Duration = Duration::from_secs(10);

/// The upload that a finished recording still owes.
///
/// Separate from the sweeper, because they answer different questions: the
/// sweeper looks for rows nothing is moving, and this moves them. Running them
/// on one timer would tie the pace of deliveries to the pace of a scan.
///
/// A deployment whose credentials do not build a delivery client gets a warning
/// and no worker rather than a panic in a router constructor. The binary does
/// not reach that case: `codetrial web` parses the key before it listens and
/// refuses to start without one. It is reachable from
/// this function, which is public and is what the tests build, and there the
/// warning is the right answer.
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

pub(crate) fn spawn_delivery_worker(accounts: Arc<Accounts>, recorder: crate::recording::Recorder) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        eprintln!("recording is configured but there is no runtime to deliver on");
        return;
    };
    let Some(delivery) = recorder.delivery.clone() else {
        return;
    };
    handle.spawn(async move {
        loop {
            let outcome = crate::recording::run_delivery_queue(
                &accounts,
                &recorder,
                delivery.as_ref() as &dyn crate::recording::DeliveryProvider,
            )
            .await;
            if outcome != crate::recording::DeliveryOutcome::default() {
                eprintln!(
                    "recording delivery delivered {} retrying {} failed {}",
                    outcome.delivered, outcome.retrying, outcome.failed
                );
            }
            tokio::time::sleep(DELIVERY_INTERVAL).await;
        }
    });
}
