use super::*;

/// A restart keeps whether the interview runs in this window or the browser,
/// which the launch being replaced already learned, and waits for the new
/// server's ready line.
#[test]
fn a_restart_keeps_where_the_interview_runs_and_waits_for_the_new_server() {
    let waiting = |webrtc| Phase::Starting {
        server_ready: false,
        webrtc: Some(webrtc),
    };
    assert_eq!(Phase::Serving.restarting(), Some(waiting(true)));
    assert_eq!(Phase::InBrowser.restarting(), Some(waiting(false)));
}

/// A server that has not reported ready, or is gone, has no keys to restart
/// onto: asking then must leave the shell as it is.
#[test]
fn a_restart_asked_before_the_server_served_or_after_it_failed_is_ignored() {
    let starting = Phase::Starting {
        server_ready: true,
        webrtc: None,
    };
    assert_eq!(starting.restarting(), None);
    assert_eq!(Phase::Failed.restarting(), None);
}
