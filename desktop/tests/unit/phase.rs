use super::*;

/// A restart keeps where the interview runs, this window or the browser, which
/// the launch being replaced already learned, and waits for the new server's
/// ready line.
#[test]
fn a_restart_keeps_where_the_interview_runs_and_waits_for_the_new_server() {
    let waiting = |webrtc| {
        Restart::Now(Phase::Starting {
            server_ready: false,
            webrtc: Some(webrtc),
        })
    };
    assert_eq!(Phase::Serving.restart_request(), waiting(true));
    assert_eq!(Phase::InBrowser.restart_request(), waiting(false));
}

/// A server that is ready but has not heard whether this web view has WebRTC
/// prints its request once, and has stopped admitting interviews: the request
/// is kept for when the phase is settled, not dropped.
#[test]
fn a_restart_asked_while_the_phase_is_unsettled_is_kept_for_later() {
    let starting = Phase::Starting {
        server_ready: true,
        webrtc: None,
    };
    assert_eq!(starting.restart_request(), Restart::WhenServing);
}

/// With no server running, there is nothing to start again.
#[test]
fn a_restart_asked_after_the_server_failed_is_ignored() {
    assert_eq!(Phase::Failed.restart_request(), Restart::Never);
}
