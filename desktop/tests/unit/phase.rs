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
            tab_open: !webrtc,
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
        tab_open: false,
    };
    assert_eq!(starting.restart_request(), Restart::WhenServing);
}

/// With no server running, there is nothing to start again.
#[test]
fn a_restart_asked_after_the_server_failed_is_ignored() {
    assert_eq!(Phase::Failed.restart_request(), Restart::Never);
}

/// The first launch without WebRTC opens a tab on the server in the browser. A
/// restart from there does not open another: the person saved from a tab on
/// the same port, and the new server answers in that one.
#[test]
fn a_restart_in_the_browser_opens_no_second_tab() {
    let ready = |phase| match phase {
        Phase::Starting {
            webrtc, tab_open, ..
        } => Phase::Starting {
            server_ready: true,
            webrtc,
            tab_open,
        },
        other => other,
    };
    let first = ready(Phase::Starting {
        server_ready: false,
        webrtc: Some(false),
        tab_open: false,
    });
    assert_eq!(
        first.settle(),
        Some((Phase::InBrowser, Show::Browser { open_tab: true }))
    );

    let Restart::Now(restarted) = Phase::InBrowser.restart_request() else {
        panic!("a server shown in the browser is restarted at once");
    };
    assert_eq!(
        restarted.settle(),
        None,
        "not before the new server is ready"
    );
    assert_eq!(
        ready(restarted).settle(),
        Some((Phase::InBrowser, Show::Browser { open_tab: false }))
    );

    let Restart::Now(restarted) = Phase::Serving.restart_request() else {
        panic!("a server shown in this window is restarted at once");
    };
    assert_eq!(
        ready(restarted).settle(),
        Some((Phase::Serving, Show::Window))
    );
}
