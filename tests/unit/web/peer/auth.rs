use super::is_snapshot_path;

#[test]
fn snapshot_paths_are_classified_after_percent_decoding() {
    assert!(is_snapshot_path("/api/peer/sessions/a/rounds/0/snapshot"));
    assert!(is_snapshot_path("/api/peer/sessions/a/rounds/0/%73napshot"));
    assert!(is_snapshot_path("/api/peer/sessions/a/rounds/0%2Fsnapshot"));
    assert!(!is_snapshot_path("/api/peer/sessions/a/rounds/0/snapshots"));
    assert!(!is_snapshot_path("/api/peer/sessions/a/%zz"));
}
