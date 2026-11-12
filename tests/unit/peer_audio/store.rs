use super::*;
use crate::accounts::peer::{self, Settings};
use std::sync::atomic::{AtomicU64, Ordering};
static SERIAL: AtomicU64 = AtomicU64::new(0);
fn fixture() -> (Accounts, String, std::path::PathBuf) {
    let accounts = Accounts::open(crate::accounts::GitHubLoginConfig {
        oauth: None,
        session_secret: "audio-test".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    accounts.initialize_schema().unwrap();
    accounts.with(|c|c.execute_batch("INSERT INTO users(id,github_id,login,created_at,updated_at) VALUES(1,1,'alice',1,1)")).unwrap();
    let state = peer::create(
        &accounts,
        1,
        Settings {
            topic: String::new(),
            difficulty: "easy".into(),
            language: "python".into(),
            round_minutes: 25,
            interviewer_first: 0,
            save_transcript: false,
            save_audio: true,
        },
        &crate::config::PeerConfig::default(),
        100,
    )
    .unwrap()
    .unwrap();
    let id = state["sessionId"].as_str().unwrap().to_owned();
    let dir = std::env::temp_dir().join(format!(
        "codetrial-audio-store-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    ensure_private_directory(&dir).unwrap();
    (accounts, id, dir)
}

#[test]
fn withdrawal_prevents_a_late_capture_finish_from_resurrecting_audio() {
    let (accounts, id, dir) = fixture();
    mark_started(&accounts, &id, 100, 60).unwrap();
    let mut writer = PeerAudioWriter::begin(&dir, &id, 60).unwrap();
    writer.append(0, 0, &[1, 2, 3]).unwrap();
    withdraw(&accounts, &id, 110).unwrap();
    let artifact = writer.finish().unwrap();
    mark_ready(&accounts, &id, &artifact, 111).unwrap();
    assert_eq!(record(&accounts, &id).unwrap().unwrap().status, "deleted");
    sweep(&accounts, &dir, 112).unwrap();
    assert!(!audio_path(&dir, &id).unwrap().exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn retention_expires_saved_audio_and_retries_a_failed_unlink() {
    let (accounts, id, dir) = fixture();
    mark_started(&accounts, &id, 100, 60).unwrap();
    let mut writer = PeerAudioWriter::begin(&dir, &id, 60).unwrap();
    writer.append(1, 0, &[42]).unwrap();
    let artifact = writer.finish().unwrap();
    mark_ready(&accounts, &id, &artifact, 200).unwrap();
    let expires = 200 + RETENTION_SECONDS;
    sweep(&accounts, &dir, expires - 1).unwrap();
    assert!(audio_path(&dir, &id).unwrap().exists());
    let path = audio_path(&dir, &id).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    sweep(&accounts, &dir, expires).unwrap();
    let purged = accounts
        .with(|c| {
            c.query_row(
                "SELECT purged_at FROM peer_audio_records WHERE session_id=?1",
                [&id],
                |r| r.get::<_, Option<i64>>(0),
            )
        })
        .unwrap();
    assert!(
        purged.is_none(),
        "failed deletion must remain eligible for retry"
    );
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, b"synthetic audio").unwrap();
    sweep(&accounts, &dir, expires + 1).unwrap();
    assert!(!path.exists());
    let value = record(&accounts, &id).unwrap().unwrap();
    assert_eq!(value.status, "deleted");
    assert_eq!(value.bytes, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_capture_is_swept_without_retaining_a_partial_file() {
    let (accounts, id, dir) = fixture();
    mark_started(&accounts, &id, 100, 60).unwrap();
    let writer = PeerAudioWriter::begin(&dir, &id, 60).unwrap();
    drop(writer);
    mark_failed(&accounts, &id, 101).unwrap();
    sweep(&accounts, &dir, 101).unwrap();
    assert_eq!(record(&accounts, &id).unwrap().unwrap().status, "deleted");
    assert!(
        !audio_path(&dir, &id)
            .unwrap()
            .with_extension("wav.part")
            .exists()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_withdrawal_before_capture_is_recorded_and_the_observer_sees_it() {
    let (accounts, id, dir) = fixture();
    assert!(record(&accounts, &id).unwrap().is_none());
    withdraw(&accounts, &id, 120).unwrap();
    assert_eq!(record(&accounts, &id).unwrap().unwrap().status, "deleted");
    mark_started(&accounts, &id, 130, 7200).unwrap();
    assert_eq!(
        record(&accounts, &id).unwrap().unwrap().status,
        "deleted",
        "starting capture must not resurrect a refused recording"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn saved_audio_is_kept_for_exactly_twenty_four_hours() {
    let (accounts, id, dir) = fixture();
    mark_started(&accounts, &id, 100, 60).unwrap();
    let artifact = PeerAudioArtifact {
        bytes: 44,
        duration_ms: 0,
    };
    mark_ready(&accounts, &id, &artifact, 1_000).unwrap();
    assert_eq!(RETENTION_SECONDS, 86_400);
    assert_eq!(
        record(&accounts, &id).unwrap().unwrap().expires_at,
        1_000 + 86_400
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// A capture that failed and is started again before the sweep runs is a new
/// capture, with its own deadline. Keeping the failed row's expiry made the
/// sweep unlink the part file of a recording that was still being written.
#[test]
fn a_capture_restarted_after_a_failure_is_not_swept_mid_recording() {
    let (accounts, id, dir) = fixture();
    mark_started(&accounts, &id, 100, 60).unwrap();
    mark_failed(&accounts, &id, 101).unwrap();
    mark_started(&accounts, &id, 110, 60).unwrap();
    let revived = record(&accounts, &id).unwrap().unwrap();
    assert_eq!(revived.status, "recording");
    assert_eq!(revived.expires_at, 110 + 60 + 300);

    let writer = PeerAudioWriter::begin(&dir, &id, 60).unwrap();
    sweep(&accounts, &dir, 120).unwrap();
    assert!(
        audio_path(&dir, &id)
            .unwrap()
            .with_extension("wav.part")
            .exists(),
        "the sweep left the part file of the live capture"
    );
    drop(writer);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A process killed mid-capture leaves its part file behind, which only the
/// sweep can remove. The writer is never dropped here, so its own cleanup does
/// not run.
#[test]
fn the_sweep_removes_the_part_file_a_killed_process_left() {
    let (accounts, id, dir) = fixture();
    mark_started(&accounts, &id, 100, 60).unwrap();
    let part = audio_path(&dir, &id).unwrap().with_extension("wav.part");
    let _leaked = std::mem::ManuallyDrop::new(PeerAudioWriter::begin(&dir, &id, 60).unwrap());
    assert!(part.exists(), "the killed capture left its part file");
    mark_failed(&accounts, &id, 101).unwrap();
    sweep(&accounts, &dir, 101).unwrap();
    assert!(!part.exists(), "the sweep removed it");
    assert!(record(&accounts, &id).unwrap().unwrap().purged_at.is_some());
    std::fs::remove_dir_all(dir).unwrap();
}

/// The sweep reads which rows are expired and removes their files afterwards,
/// and a capture can start again in between. A row that is no longer expired
/// when the sweep reaches it is left alone, with its file, and a row it does
/// take is marked deleted before any file is touched, so a start that follows
/// finds the refusal and not a file about to vanish.
#[test]
fn a_row_revived_after_the_sweep_read_it_is_not_purged() {
    let (accounts, id, dir) = fixture();
    mark_started(&accounts, &id, 100, 60).unwrap();
    mark_failed(&accounts, &id, 101).unwrap();
    let part = audio_path(&dir, &id).unwrap().with_extension("wav.part");
    let _writer = std::mem::ManuallyDrop::new(PeerAudioWriter::begin(&dir, &id, 60).unwrap());

    // The sweep had read this row as expired; then the capture started again.
    mark_started(&accounts, &id, 110, 60).unwrap();
    purge(&accounts, &dir, 120, vec![id.clone()]).unwrap();
    assert!(part.exists(), "the live capture's file was kept");
    assert_eq!(record(&accounts, &id).unwrap().unwrap().status, "recording");

    // A row that is expired is claimed, then purged.
    purge(&accounts, &dir, 110 + 60 + 300, vec![id.clone()]).unwrap();
    assert!(!part.exists());
    let purged = record(&accounts, &id).unwrap().unwrap();
    assert_eq!(purged.status, "deleted");
    assert!(purged.purged_at.is_some());
    std::fs::remove_dir_all(dir).unwrap();
}
