use codetrial::config::update_check_commit;

/// The update check requires the setting, a supported platform, and a
/// release commit recorded by CI. Local builds and unsupported platforms
/// must not receive update notices.
#[test]
fn the_update_check_needs_the_setting_a_supported_platform_and_a_ci_commit() {
    // Update checking is opt-in.
    assert_eq!(update_check_commit(None, "abc1234", true), None);
    assert_eq!(update_check_commit(Some("false"), "abc1234", true), None);

    // Unsupported platforms must not receive update notices.
    assert_eq!(update_check_commit(Some("true"), "abc1234", false), None);

    // A local build without a CI release commit must not receive notices.
    assert_eq!(update_check_commit(Some("true"), "", true), None);

    // Supported platforms, including Windows CLI builds, can use the existing
    // update-check mechanism when explicitly enabled.
    assert_eq!(
        update_check_commit(Some("true"), "abc1234", true).as_deref(),
        Some("abc1234")
    );
    assert_eq!(
        update_check_commit(Some("1"), "abc1234", true).as_deref(),
        Some("abc1234")
    );
}
