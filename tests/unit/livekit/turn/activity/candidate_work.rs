use super::*;
use std::time::Duration;

#[test]
fn a_board_snapshot_counts_as_work_and_a_recorded_one_restarts_the_idle_count() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.completed_coding_nudges = 2;
    let later = start + Duration::from_secs(30);

    activity.note_board_snapshot(false, later);
    assert_eq!(activity.last_code_change, later);
    assert_eq!(activity.completed_coding_nudges, 2, "unchanged ledger");

    activity.note_board_snapshot(true, later);
    assert_eq!(activity.completed_coding_nudges, 0);
}
