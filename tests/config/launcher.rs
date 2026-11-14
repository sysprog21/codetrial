//! What a program that starts the server as its child hands it through the
//! environment, which a `--data-dir` launch reads no configuration from.

/// Off unless asked for: a server whose stdin is `/dev/null`, which is every
/// service manager's default, would otherwise exit the moment it started.
#[test]
fn exiting_with_stdin_is_only_on_when_a_launcher_asks() {
    let exits = codetrial::config::exit_with_stdin;
    for off in [None, Some(""), Some("0"), Some("false"), Some("maybe")] {
        assert!(!exits(off), "{off:?} should leave it off");
    }
    for on in ["1", "true", "yes"] {
        assert!(exits(Some(on)), "{on} should turn it on");
    }
}

/// A code too short or too odd to stand in for the one `run_web` makes is no
/// code, so a launcher that set it carelessly gets no Change keys at all.
#[test]
fn a_change_keys_code_is_taken_only_when_strong_enough() {
    let code = codetrial::config::change_keys_code;
    assert_eq!(
        code(Some(" 0123456789abcdef0123456789abcdef\n")).as_deref(),
        Some("0123456789abcdef0123456789abcdef")
    );
    for weak in [
        None,
        Some(""),
        Some("0123456789abcdef0123456"),
        Some("0123456789abcdef 0123456789abcdef"),
        Some("0123456789abcdef0123456789abcde'"),
    ] {
        assert_eq!(code(weak), None, "{weak:?} should give no code");
    }
}
