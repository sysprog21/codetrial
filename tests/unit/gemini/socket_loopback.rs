use crate::test_isolation as isolation;

use super::*;

#[test]
fn loopback_registration_rejects_every_non_fixture_destination() {
    for endpoint in [
        "wss://127.0.0.1:9/?key=sequence-test-key",
        "ws://localhost:9/?key=sequence-test-key",
        "ws://127.0.0.2:9/?key=sequence-test-key",
        "ws://127.0.0.1:9/",
        "ws://127.0.0.1:9?key=sequence-test-key",
        "ws://127.0.0.1:9/?key=other",
        "ws://127.0.0.1:9/?key=sequence-test-key&other=1",
        "ws://user@127.0.0.1:9/?key=sequence-test-key",
        "ws://127.0.0.1:9/other?key=sequence-test-key",
    ] {
        assert!(install(endpoint).is_err());
    }
}

#[test]
fn loopback_registration_is_scoped_one_shot_and_drops_without_a_connection() {
    let name = format!(
        "{}::loopback_registration_is_scoped_one_shot_and_drops_without_a_connection",
        module_path!().split_once("::").unwrap().1,
    );
    if isolation::isolated_checked(&name, "CODETRIAL_SOCKET_FIXTURE_TEST", false, |_| {}) {
        return;
    }
    let endpoint = "ws://127.0.0.1:9/?key=sequence-test-key";
    let first = crate::gemini::GeminiLiveSession::loopback_send_limit_for_test(endpoint).unwrap();
    assert!(install(endpoint).is_err());
    assert!(
        claim("ws://127.0.0.1:8/?key=sequence-test-key")
            .unwrap()
            .is_none()
    );
    assert!(claim(endpoint).unwrap().is_some());
    assert!(claim(endpoint).is_err());
    assert_eq!(first.sender_buffers(), None);
    drop(first);
    let second = install(endpoint).unwrap();
    assert!(claim(endpoint).unwrap().is_some());
    drop(second);
    assert!(claim(endpoint).unwrap().is_none());
}

#[test]
fn loopback_buffer_admission_uses_actual_getters_not_requested_capacity() {
    assert!(admitted(0).is_err());
    assert_eq!(admitted(4608).unwrap(), 4608);
    assert_eq!(admitted(16384).unwrap(), 16384);
    assert!(admitted(16385).is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn loopback_connect_without_registration_passes_through_without_opening_a_socket() {
    assert!(
        connect("ws://127.0.0.1:9/?key=sequence-test-key")
            .await
            .unwrap()
            .is_none()
    );
}
