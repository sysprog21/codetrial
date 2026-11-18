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
    if isolation::isolated!("CODETRIAL_SOCKET_FIXTURE_TEST") {
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

#[tokio::test(flavor = "current_thread")]
async fn loopback_connect_keeps_the_connected_sender_within_its_declared_bound() {
    if isolation::isolated!("CODETRIAL_CONNECTED_SOCKET_FIXTURE_TEST") {
        return;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "ws://{}/?key=sequence-test-key",
        listener.local_addr().unwrap()
    );
    let registration = install(&endpoint).unwrap();
    let stream = connect(&endpoint).await.unwrap().unwrap();
    let (_peer, _) = listener.accept().await.unwrap();
    let (before, connected) = registration.sender_buffers().unwrap();
    assert!((1..=MAX_BUFFER).contains(&before));
    assert!((1..=MAX_BUFFER).contains(&connected));
    assert_eq!(
        usize::try_from(connected).unwrap(),
        socket2::SockRef::from(&stream).send_buffer_size().unwrap()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn loopback_sender_constraint_reduces_an_oversized_connected_buffer() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (_peer, _) = listener.accept().await.unwrap();
    let socket = socket2::SockRef::from(&stream);
    socket
        .set_send_buffer_size(usize::try_from(MAX_BUFFER * 2).unwrap())
        .unwrap();
    let raised = socket.send_buffer_size().unwrap();
    assert!(
        raised > usize::try_from(MAX_BUFFER).unwrap(),
        "the fixture must start oversized"
    );
    let constrained = constrain_sender(&stream).unwrap();
    let actual = socket.send_buffer_size().unwrap();
    assert_eq!(actual, usize::try_from(constrained).unwrap());
    assert!((1..=MAX_BUFFER).contains(&constrained));
    assert!(actual < raised, "the connected buffer must be reduced");
}
