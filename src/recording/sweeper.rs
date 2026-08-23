use std::sync::Arc;

use crate::accounts::{Accounts, blocking};

use super::store::{SELECT_RECORDING, row_to_recording};
use super::summary::deletion_reason;
use super::*;

/// Moves the row and then stops the provider, in that order.
///
/// The order matters and is not obvious. Calling the provider first meant two
/// callers racing on one recording, `/end` and a `room_finished` webhook say,
/// both sent a stop, and a process that died between a successful stop and the
/// write left a row that stayed active forever. Moving first makes the row the
/// record of intent, and only the caller that moved it talks to the provider.
///
/// A failed stop leaves the row where it was moved to and the job still
/// running, which the sweeper finds once the row goes stale. That is the cost
/// of this order and it is the smaller one: the alternative loses the intent.
/// The `bool` is whether this call moved the row, and it is reported even when
/// the provider then refuses. The move happened; a caller that only audited on
/// `Ok` lost the record of a state change that is now in the database.
pub async fn stop_recording(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    recording: &Recording,
    next: RecordingState,
    reason: Option<&str>,
) -> Result<(RecordingState, bool), StopFailure> {
    let (state, mine) = {
        let accounts = accounts.clone();
        let clock = recorder.clock.clone();
        let recording_id = recording.id.clone();
        let reason = reason.map(str::to_string);
        blocking(move || {
            transition(
                &accounts,
                clock.as_ref(),
                &recording_id,
                next,
                reason.as_deref(),
            )
        })
        .await
        .map_err(|error| StopFailure {
            state: recording.state,
            moved: false,
            error: format!("could not record the stop: {error}"),
        })?
        .ok_or_else(|| StopFailure {
            state: recording.state,
            moved: false,
            error: "the recording went away while it was being stopped".to_string(),
        })?
    };
    if !mine {
        // Somebody else moved it, and whatever they did includes telling the
        // provider. There is nothing left here, and the caller has to be told
        // that rather than counting a move it did not make.
        return Ok((state, false));
    }
    if let Some(egress_id) = &recording.egress_id {
        if let Err(error) = recorder
            .provider
            .stop(
                egress_id,
                recording.room_name.as_deref().unwrap_or_default(),
            )
            .await
        {
            // The row moved and the provider did not agree. Both go back, so
            // the caller can audit the change it made and the sweeper can keep
            // asking about the job.
            return Err(StopFailure {
                state,
                moved: true,
                error,
            });
        }

        // Written only after the provider agreed. A row with an egress id and
        // no `stopped_at` is a job that still needs stopping, which is the only
        // thing that can say so once the state is `failed` and terminal.
        let accounts = accounts.clone();
        let clock = recorder.clock.clone();
        let id = recording.id.clone();
        if let Err(error) = blocking(move || mark_stopped(&accounts, clock.as_ref(), &id)).await {
            eprintln!(
                "recording {} was stopped and the row does not know: {error}",
                recording.id
            );
        }
    }
    Ok((state, true))
}

/// Stops a job whose recording stopped being active while the provider was
/// answering.
///
/// Both paths that hand a fresh egress id to a row need this: the start route
/// and the sweeper's retry. A candidate can withdraw consent, a room can
/// finish, a tab can close, all while `StartRoomCompositeEgress` is still in
/// flight, and the row that comes back is terminal with a live job attached.
pub async fn settle_new_egress(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    recording_id: &str,
    egress_id: &str,
) {
    let settled = {
        let accounts = accounts.clone();
        let recording_id = recording_id.to_string();
        blocking(move || recording_by_id(&accounts, &recording_id)).await
    };
    let Ok(Some(settled)) = settled else { return };
    if settled.state.provider_should_run() {
        return;
    }
    match recorder
        .provider
        .stop(egress_id, settled.room_name.as_deref().unwrap_or_default())
        .await
    {
        Ok(()) => {
            let accounts = accounts.clone();
            let clock = recorder.clock.clone();
            let id = settled.id.clone();
            let _ = blocking(move || mark_stopped(&accounts, clock.as_ref(), &id)).await;
        }
        Err(error) => eprintln!(
            "recording {} was stopped before it started and its job is still running: {error}",
            settled.id
        ),
    }
}

/// A stop whose row moved and whose provider did not agree.
///
/// Both halves matter to the caller: the state change is real and has to be
/// audited, and the job is still running and has to be swept for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopFailure {
    pub state: RecordingState,
    pub moved: bool,
    pub error: String,
}

impl std::fmt::Display for StopFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.error)
    }
}

/// Writes down that the provider is no longer running this job.
pub fn mark_stopped(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
) -> rusqlite::Result<()> {
    let now = clock.now();
    accounts.with(|connection| {
        connection.execute(
            "UPDATE recordings SET stopped_at = ?2 WHERE id = ?1 AND stopped_at IS NULL",
            (recording_id, now),
        )?;
        Ok(())
    })
}

/// Recordings whose provider job should not be running and is not known to have
/// stopped.
///
/// Two cases need it. A candidate withdraws consent, the row moves to `failed`,
/// and the stop fails: `failed` is terminal, so no state will ever say the job
/// is still going, and the missing `stopped_at` is what does. And a stop that
/// arrived before the recording had an egress id leaves a `finalizing` row that
/// was never able to name the job it wanted stopped.
///
/// `room_name IS NOT NULL` because the adapter resolves credentials from the
/// room: a tombstoned row has given its room name up, and asking to stop a job
/// without one fails every minute forever. A tombstone also means the media is
/// already gone, so there is nothing left to stop.
pub fn unstopped_jobs(accounts: &Accounts) -> rusqlite::Result<Vec<Recording>> {
    accounts.with(|connection| {
        let mut statement = connection.prepare(&format!(
            "{SELECT_RECORDING}
        WHERE egress_id IS NOT NULL
          AND stopped_at IS NULL
          AND room_name IS NOT NULL
          AND state NOT IN ('starting', 'recording')
        "
        ))?;
        let rows = statement
            .query_map([], row_to_recording)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Takes the next retry, so that only one sweeper makes it.
///
/// Guarded on the `updated_at` the caller read, which is what stops two
/// processes from both reconciling and both starting. Touching `updated_at`
/// also restarts the staleness clock, because an attempt is now in flight.
pub fn claim_retry(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
    seen_updated_at: i64,
) -> rusqlite::Result<bool> {
    let now = clock.now();
    accounts.with(|connection| {
        let changed = connection.execute(
            "
        UPDATE recordings SET updated_at = ?3
        WHERE id = ?1
          AND updated_at = ?2
          AND state = 'starting'
          AND egress_id IS NULL
        ",
            (recording_id, seen_updated_at, now),
        )?;
        Ok(changed > 0)
    })
}

/// One minute, five, then fifteen.
///
/// A provider that refused once is usually busy; a provider that has refused
/// three times over sixteen minutes is not going to answer for this interview,
/// which will be over by then.
pub const RETRY_BACKOFF_SECONDS: [i64; 3] = [60, 300, 900];

/// How long to wait before retry number `retries + 1`, or `None` when there is
/// no retry left to make.
///
/// `retries` counts retries, and the first attempt is not one. That is why the
/// route that makes the first attempt does not increment it when the provider
/// refuses: doing so made the first retry five minutes out rather than one, and
/// the schedule this names is one minute, then five, then fifteen.
pub fn retry_delay(retries: i64) -> Option<i64> {
    usize::try_from(retries)
        .ok()
        .and_then(|index| RETRY_BACKOFF_SECONDS.get(index))
        .copied()
}

/// How long a row may sit untouched before the pipeline decides nobody owns it.
///
/// Untouched, not unfinished: a recording that has been running for forty
/// minutes is a recording. One whose row has not moved for fifteen is a process
/// that died holding it.
pub const STALE_SECONDS: i64 = 900;

/// The slack on top of the interview's own maximum, before a `recording` row is
/// treated as abandoned.
///
/// `recording` is the one state with no periodic signal. LiveKit sends
/// `egress_updated` on a status change and not as a heartbeat, so a healthy
/// forty-minute interview touches its row once at the start and not again;
/// applying the fifteen-minute rule to it failed every recording that outlived
/// its own first quarter of an hour. What a recording cannot legitimately do is
/// outlive `CODETRIAL_RECORDING_MAX_MINUTES`, so that is the bound, plus enough
/// for the provider to finish writing and say so.
pub const RECORDING_STALE_SLACK_SECONDS: i64 = 300;

/// How long this state may sit untouched.
pub fn stale_after(state: RecordingState, max_minutes: u32) -> i64 {
    match state {
        RecordingState::Recording => i64::from(max_minutes) * 60 + RECORDING_STALE_SLACK_SECONDS,
        _ => STALE_SECONDS,
    }
}

pub fn bump_retry(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
) -> rusqlite::Result<()> {
    let now = clock.now();
    accounts.with(|connection| {
        connection.execute(
            "UPDATE recordings SET retries = retries + 1, updated_at = ?2 WHERE id = ?1",
            (recording_id, now),
        )?;
        Ok(())
    })
}

/// What one sweep did, so the caller can say so rather than sweeping silently.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepOutcome {
    pub retried: usize,
    pub failed: usize,
    /// Recordings whose media was deleted by this pass.
    pub deleted: usize,
}

/// Resolves recordings nobody is looking after.
///
/// Run at startup, because a process that died mid-interview leaves rows that
/// no request will ever touch again, and on an interval, because the retry
/// schedule is the only thing that turns a provider's bad minute into a
/// recording rather than a failure.
///
/// The kill switch is handled here too. It refuses new starts at
/// [`begin_recording`], and an operator who throws it while interviews are
/// running means the running ones as well.
pub async fn sweep_recordings(accounts: &Arc<Accounts>, recorder: &Recorder) -> SweepOutcome {
    let now = recorder.clock.now();
    let kill_switch = recorder.config.kill_switch;

    // Before the stale read, not after it. A restart longer than the stale
    // window leaves exactly the row this repairs, and a sweep that read it as
    // stale first would fail the recording it had just queued a delivery for.
    requeue_orphaned_work(accounts, now).await;

    let cutoff = if kill_switch { None } else { Some(now - 60) };
    let stale = {
        let accounts = accounts.clone();
        match blocking(move || stale_active(&accounts, cutoff)).await {
            Ok(stale) => stale,
            Err(error) => {
                eprintln!("WARNING: recording sweep could not read its work: {error}");
                return SweepOutcome::default();
            }
        }
    };

    // Bounded, because a ledger that only grows is a table that eventually
    // matters. LiveKit gives up retrying long before this.
    {
        let accounts = accounts.clone();
        let before = now - EVENT_RETENTION_SECONDS;
        if let Err(error) = blocking(move || prune_webhook_events(&accounts, before)).await {
            eprintln!("WARNING: could not prune the webhook ledger: {error}");
        }
    }

    let mut outcome = SweepOutcome::default();

    // Retention, before the stale pass rather than after it. A recording whose
    // media is due to go does not need chasing for a webhook first, and a row
    // this pass tombstones is one the pass below no longer sees.
    delete_due_recordings(accounts, recorder, now, &mut outcome).await;
    for recording in stale {
        sweep_stale_recording(
            accounts,
            recorder,
            &recording,
            now,
            kill_switch,
            &mut outcome,
        )
        .await;
    }
    stop_orphaned_jobs(accounts, recorder).await;
    outcome
}

/// A transferring recording nobody is delivering.
///
/// The transition and the queue insert are two writes, so a process that died
/// between them leaves a row that owes a delivery and a queue that has never
/// heard of it. Repaired here rather than guarded against with a wider
/// transaction: this is the sweep that already exists to find work nothing is
/// moving.
async fn requeue_orphaned_work(accounts: &Arc<Accounts>, now: i64) {
    let accounts = accounts.clone();
    match blocking(move || requeue_orphaned_deliveries(&accounts, now)).await {
        Ok(0) => {}
        Ok(requeued) => eprintln!("recording sweep queued {requeued} orphaned deliveries"),
        Err(error) => eprintln!("WARNING: could not queue orphaned deliveries: {error}"),
    }
}

/// Recordings whose media retention says is due to go.
///
/// Only with a delivery provider: without one there is nothing holding media to
/// delete, and a tombstone written anyway would claim a deletion that never
/// happened.
async fn delete_due_recordings(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    now: i64,
    outcome: &mut SweepOutcome,
) {
    let Some(delivery) = recorder.delivery.as_ref() else {
        return;
    };
    let due = {
        let accounts = accounts.clone();
        blocking(move || due_for_deletion(&accounts, now)).await
    };
    let due = match due {
        Ok(due) => due,
        Err(error) => {
            eprintln!("WARNING: retention could not read its work: {error}");
            return;
        }
    };
    for recording in due {
        let reason = {
            let accounts = accounts.clone();
            let recording = recording.clone();
            blocking(move || Ok(deletion_reason(&accounts, &recording)))
                .await
                .unwrap_or("expiry")
        };
        match delete_recording(accounts, recorder, delivery.as_ref(), &recording, reason).await {
            Ok(_) => outcome.deleted += 1,

            // Already audited as `recording_cleanup_failed`, which is the line
            // an operator acts on. Counted here so the sweep's own summary says
            // a pass had trouble.
            Err(_) => outcome.failed += 1,
        }
    }
}

/// One stale row, and the four different things being stale can mean.
///
/// Every branch ends the same way, and it is the reason this is written around
/// `moved_by_this_call` rather than around the state: two sweepers can read the
/// same row, and the one whose transition did not land must not audit or count
/// an ending it did not cause.
async fn sweep_stale_recording(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    recording: &Recording,
    now: i64,
    kill_switch: bool,
    outcome: &mut SweepOutcome,
) {
    let age = now - recording.updated_at;
    if kill_switch {
        if fail_stale(
            accounts,
            recorder,
            recording,
            Some(Failure::KillSwitch.as_str()),
        )
        .await
        {
            audit(
                "recording_killed",
                &recording.id,
                &[
                    ("reason", Failure::KillSwitch.as_str()),
                    ("recovery", Failure::KillSwitch.recovery()),
                ],
            );
            outcome.failed += 1;
        }
        return;
    }

    // A start that never got an egress id is the one case worth another
    // provider call: everything else is waiting on the provider to speak, and
    // asking again would not make it.
    let never_started =
        recording.state == RecordingState::Starting && recording.egress_id.is_none();
    if never_started && retry_delay(recording.retries).is_some_and(|delay| age >= delay) {
        if retry_start(accounts, recorder, recording).await {
            outcome.retried += 1;
        }
        return;
    }

    // Out of retries and still with nothing to show for them. This is its own
    // ending rather than the stale one: the provider refused every time, which
    // is a different thing from nobody having said anything, and
    // `Failure::Start` is the word for it.
    if never_started && retry_delay(recording.retries).is_none() {
        if fail_stale(accounts, recorder, recording, Some(Failure::Start.as_str())).await {
            audit(
                "recording_failed",
                &recording.id,
                &[
                    ("reason", Failure::Start.as_str()),
                    ("recovery", Failure::Start.recovery()),
                ],
            );
            outcome.failed += 1;
        }
        return;
    }
    if age < stale_after(recording.state, recorder.config.max_minutes) {
        return;
    }

    // A delivery that already failed keeps its own reason. Overwriting it with
    // `abandoned` would change the recovery from "retry the delivery" to
    // "record again", for a recording whose media may still exist.
    let already = recording
        .error
        .as_deref()
        .and_then(Failure::parse)
        .filter(|failure| *failure == Failure::Drive);
    let reason = already.unwrap_or(Failure::LostWebhook);

    // `None` where the row already says why: `transition` coalesces, and a
    // reason passed here would replace it.
    if fail_stale(
        accounts,
        recorder,
        recording,
        already.is_none().then_some(reason.as_str()),
    )
    .await
    {
        audit(
            "recording_abandoned",
            &recording.id,
            &[
                ("state", recording.state.as_str()),
                ("reason", reason.as_str()),
                ("recovery", reason.recovery()),
                ("age_seconds", &age.to_string()),
            ],
        );
        outcome.failed += 1;
    }
}

/// Moves a stale recording to `failed`, and says whether this call is the one
/// that moved it. The audit line belongs to whoever gets `true`.
async fn fail_stale(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    recording: &Recording,
    reason: Option<&str>,
) -> bool {
    moved_by_this_call(
        &recording.id,
        stop_recording(
            accounts,
            recorder,
            recording,
            RecordingState::Failed,
            reason,
        )
        .await,
    )
}

/// Jobs this side has finished with that the provider is still running.
///
/// The stop that was supposed to end them failed after the row had already
/// moved, and `failed` is terminal, so nothing else will ever ask again.
async fn stop_orphaned_jobs(accounts: &Arc<Accounts>, recorder: &Recorder) {
    let orphans = {
        let accounts = accounts.clone();
        blocking(move || unstopped_jobs(&accounts))
            .await
            .unwrap_or_default()
    };
    for orphan in orphans {
        let Some(egress_id) = &orphan.egress_id else {
            continue;
        };
        match recorder
            .provider
            .stop(egress_id, orphan.room_name.as_deref().unwrap_or_default())
            .await
        {
            Ok(()) => {
                let accounts = accounts.clone();
                let clock = recorder.clock.clone();
                let id = orphan.id.clone();
                let _ = blocking(move || mark_stopped(&accounts, clock.as_ref(), &id)).await;
            }
            Err(error) => {
                audit(
                    "recording_still_running",
                    &orphan.id,
                    &[("error", &error), ("action", "stop_by_hand")],
                );
            }
        }
    }
}

async fn retry_start(accounts: &Arc<Accounts>, recorder: &Recorder, recording: &Recording) -> bool {
    // Taken before anything is asked of the provider. Two sweepers can read the
    // same stale row, both find no job for the room, and both start one; the
    // unique index guards row creation and says nothing about who owns a retry.
    let claimed = {
        let accounts = accounts.clone();
        let clock = recorder.clock.clone();
        let id = recording.id.clone();
        let seen = recording.updated_at;
        blocking(move || claim_retry(&accounts, clock.as_ref(), &id, seen)).await
    };
    if !claimed.unwrap_or(false) {
        return false;
    }

    let room_name = recording.room_name.clone().unwrap_or_default();

    // Asked before starting, not after failing. A start that succeeded and
    // whose id this side never wrote down leaves a job nothing knows about;
    // starting again would make a second one, and neither would be stoppable by
    // a candidate withdrawing consent.
    match recorder.provider.active_for_room(&room_name).await {
        Ok(Some(egress_id)) => {
            eprintln!(
                "recording {} already had an Egress job; adopting it rather than starting a second",
                recording.id
            );
            let stored = {
                let accounts = accounts.clone();
                let clock = recorder.clock.clone();
                let id = recording.id.clone();
                let egress_id = egress_id.clone();
                blocking(move || record_egress_id(&accounts, clock.as_ref(), &id, &egress_id))
                    .await
                    .unwrap_or(false)
            };
            if stored {
                settle_new_egress(accounts, recorder, &recording.id, &egress_id).await;
            }
            return stored;
        }
        Ok(None) => {}
        Err(error) => {
            // Not knowing is not permission to start a second one.
            eprintln!(
                "recording {} could not be reconciled with the provider: {error}",
                recording.id
            );
            let accounts = accounts.clone();
            let clock = recorder.clock.clone();
            let id = recording.id.clone();
            let _ = blocking(move || bump_retry(&accounts, clock.as_ref(), &id)).await;
            return false;
        }
    }

    let request = StartEgress {
        room_name: room_name.clone(),
        template_base_url: recorder.config.template_base_url.clone(),
        bucket: recorder.config.gcs_bucket.clone(),
        filepath: gcs_object_path(&recorder.config.gcs_prefix, &recording.id),
        service_account_json: recorder.config.service_account_json.clone(),
        bitrate: recorder.config.bitrate,
    };
    let clock = recorder.clock.clone();
    match recorder.provider.start(&request).await {
        Ok(egress_id) => {
            let stored = {
                let accounts = accounts.clone();
                let id = recording.id.clone();
                let egress_id = egress_id.clone();
                blocking(move || record_egress_id(&accounts, clock.as_ref(), &id, &egress_id))
                    .await
                    .unwrap_or(false)
            };
            if stored {
                // The row can have moved while the provider was answering, and
                // a retry has the same window the first attempt does.
                settle_new_egress(accounts, recorder, &recording.id, &egress_id).await;
            } else {
                // Either the row already names another job, or the write
                // failed. Either way this attempt owns a job the row does not,
                // so it stops the one it just made rather than leaving two.
                eprintln!(
                    "recording {} started a job its row does not name; stopping it",
                    recording.id
                );
                let _ = recorder
                    .provider
                    .stop(&egress_id, &room_name)
                    .await
                    .inspect_err(|error| {
                        eprintln!("recording {} left a job running: {error}", recording.id)
                    });
            }
            stored
        }
        Err(error) => {
            eprintln!(
                "recording {} could not start on retry: {error}",
                recording.id
            );
            let accounts = accounts.clone();
            let id = recording.id.clone();
            let _ = blocking(move || bump_retry(&accounts, clock.as_ref(), &id)).await;
            false
        }
    }
}

/// Whether this call is the one that moved the row.
///
/// A provider that refused the stop does not undo the move: the row is in its
/// new state either way, and a caller that audited only on success lost the
/// record of a change that is now in the database. The refusal is the
/// sweeper's, and the missing `stopped_at` is what brings it back.
fn moved_by_this_call(
    recording_id: &str,
    outcome: Result<(RecordingState, bool), StopFailure>,
) -> bool {
    match outcome {
        Ok((_, moved)) => moved,
        Err(failure) => {
            report_stop_failure(recording_id, None, &failure);
            failure.moved
        }
    }
}

/// One place that decides which of the two stop failures this is.
///
/// `moved` is the difference. With it the transition happened and the provider
/// refused; without it the transition never happened and the provider was never
/// called, and reporting that as a refusal sends an operator to the wrong
/// service.
pub fn report_stop_failure(recording_id: &str, step: Option<&str>, failure: &StopFailure) {
    let event = if failure.moved {
        "recording_stop_refused"
    } else {
        "recording_stop_not_recorded"
    };
    match step {
        Some(step) => audit(
            event,
            recording_id,
            &[("step", step), ("error", &failure.error)],
        ),
        None => audit(event, recording_id, &[("error", &failure.error)]),
    };
}
