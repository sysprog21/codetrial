use super::*;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
static SERIAL: AtomicU64 = AtomicU64::new(0);
fn dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "codetrial-peer-audio-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    path
}

#[test]
fn saved_audio_aligns_both_members_without_overwriting_the_other_channel() {
    let path = dir();
    let mut writer = PeerAudioWriter::begin(&path, "session_1", 1).unwrap();
    writer.append(0, 0, &[10, 20]).unwrap();
    writer.append(1, 0, &[30, 40]).unwrap();
    writer.append(0, 1, &[50]).unwrap();
    let artifact = writer.finish().unwrap();
    let bytes = fs::read(path.join("session_1.wav")).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 2);
    assert_eq!(
        u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
        16_000
    );
    assert_eq!(&bytes[44..52], &[10, 0, 30, 0, 20, 0, 40, 0]);
    assert!(bytes[52..108].iter().all(|byte| *byte == 0));
    assert_eq!(&bytes[108..112], &[50, 0, 0, 0]);
    assert_eq!(artifact.bytes, 112);
    assert!(artifact.duration_ms >= 1);
    assert!(!path.join("session_1.wav.part").exists());
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn capture_is_private_and_an_unfinished_capture_is_removed() {
    let path = dir();
    {
        let _writer = PeerAudioWriter::begin(&path, "private", 1).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path.join("private.wav.part"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    assert!(!path.join("private.wav.part").exists());
    assert!(!path.join("private.wav").exists());
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn capture_refuses_path_traversal_duplicate_files_invalid_slots_and_excess_duration() {
    let path = dir();
    for id in ["../outside", "a/b", "", "a.wav", "a\\b"] {
        assert!(PeerAudioWriter::begin(&path, id, 1).is_err());
    }
    let mut writer = PeerAudioWriter::begin(&path, "bounded", 1).unwrap();
    assert!(PeerAudioWriter::begin(&path, "bounded", 1).is_err());
    assert!(writer.append(2, 0, &[1]).is_err());
    writer.append(0, 1000, &[1]).unwrap();
    writer.append(0, 999, &[1]).unwrap();
    drop(writer);
    let mut writer = PeerAudioWriter::begin(&path, "completed", 1).unwrap();
    writer.append(0, 0, &[1]).unwrap();
    writer.finish().unwrap();
    assert!(path.join("completed.wav").exists());
    assert!(!path.join("completed.wav.part").exists());
    assert!(PeerAudioWriter::begin(&path, "completed", 1).is_err());
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn a_final_chunk_straddling_the_deadline_keeps_the_bounded_recording_prefix() {
    let path = dir();
    let mut writer = PeerAudioWriter::begin(&path, "deadline", 1).unwrap();
    writer.append(0, 0, &[10]).unwrap();
    writer.append(1, 999, &[42; 32]).unwrap();
    writer.append(0, 1001, &[99]).unwrap();
    let artifact = writer.finish().unwrap();
    assert_eq!(artifact.duration_ms, 1000);
    assert_eq!(artifact.bytes, 44 + 16_000 * 4);
    let bytes = fs::read(path.join("deadline.wav")).unwrap();
    assert_eq!(&bytes[44..48], &[10, 0, 0, 0]);
    assert_eq!(&bytes[bytes.len() - 4..], &[0, 0, 42, 0]);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn the_header_sizes_describe_the_data_that_follows() {
    let path = dir();
    let mut writer = PeerAudioWriter::begin(&path, "header", 1).unwrap();
    writer.append(0, 0, &[1, 2, 3]).unwrap();
    writer.finish().unwrap();
    let bytes = fs::read(path.join("header.wav")).unwrap();
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 36 + 12);
    assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 12);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn one_chunk_is_bounded_at_thirty_seconds_of_samples() {
    let path = dir();
    let mut writer = PeerAudioWriter::begin(&path, "chunk", 60).unwrap();
    writer.append(0, 0, &vec![1; 480_000]).unwrap();
    assert!(writer.append(0, 0, &vec![1; 480_001]).is_err());
    drop(writer);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn the_duration_bound_accepts_its_limit_and_refuses_zero_and_beyond() {
    let path = dir();
    assert!(PeerAudioWriter::begin(&path, "zero", 0).is_err());
    assert!(PeerAudioWriter::begin(&path, "over", MAX_AUDIO_SECONDS + 1).is_err());
    assert!(PeerAudioWriter::begin(&path, "limit", MAX_AUDIO_SECONDS).is_ok());
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn a_session_id_may_be_at_most_128_bytes() {
    let path = dir();
    assert!(audio_path(&path, &"a".repeat(128)).is_ok());
    assert!(audio_path(&path, &"a".repeat(129)).is_err());
    fs::remove_dir_all(path).unwrap();
}
