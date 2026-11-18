//! How the binary's modes read their configuration.
//!
//! Split out of `tests/cli.rs`, which is still the test target.

use super::*;

/// Every mode that reads configuration refuses without a file, and says which
/// file it wanted. One test over the modes rather than one test each: the
/// assertion is the same sentence three times, and a mode added to MODES that
/// forgets this belongs in this list rather than in a fourth copy.
///
/// `web` is not here: a missing config is a cold start for it, covered by
/// `binary_web_serves_a_setup_page_when_no_config_exists`.
#[test]
fn binary_modes_that_read_configuration_require_a_primary_config_file() {
    for args in [
        &["run-livekit", "interview-fixed"][..],
        &["check-gemini"][..],
    ] {
        let (code, stdout, stderr) = run_cli_args(args);

        assert_eq!(code, 1, "{args:?}: {stderr}");
        assert!(stdout.is_empty(), "{args:?}: {stdout}");
        assert!(
            stderr.contains("required configuration file is missing"),
            "{args:?}: {stderr}"
        );
    }
}

/// A length outside 10 to 90 is named once at startup, by `codetrial web` and
/// by the agent modes alike, as `docs/interview-length.md` says: `check-gemini`
/// would otherwise boot its session on 90 without a word, and a second reader
/// that printed it too would say it twice.
#[test]
fn binary_modes_name_a_duration_that_was_clamped_once() {
    let dir = temp_path("duration-clamped");
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("codetrial.env.local");
    std::fs::write(
        &config,
        "CODETRIAL_DURATION_MIN=500\nCODETRIAL_WEB_ADDR=127.0.0.1:1\n",
    )
    .unwrap();
    let runs: Vec<_> = ["web", "check-gemini"]
        .into_iter()
        .map(|mode| {
            (
                mode,
                run_cli_args(&[mode, "--config", config.to_str().unwrap()]),
            )
        })
        .collect();
    let _ = std::fs::remove_dir_all(dir);

    for (mode, (code, _, stderr)) in runs {
        assert_eq!(code, 1, "{mode}: {stderr}");
        assert_eq!(
            stderr
                .matches("CODETRIAL_DURATION_MIN \"500\" is outside 10 to 90; read as 90")
                .count(),
            1,
            "{mode}: {stderr}"
        );
    }
}
