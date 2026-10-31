// Part of contract.rs: included in place, so every item here shares that file's
// module.

/// A card the filter cannot show is a problem nobody can reach.
///
/// web/app.js hides every card whose `data-difficulty` is not among the checked
/// boxes, so a generated card carrying a level the fieldset never offers is
/// invisible for good, is never recommended, and nothing fails to say so. The
/// cards come from problem-bank/problems.json through
/// scripts/gen-problem-cards.py and the boxes are hand-written, which is
/// exactly the seam a new difficulty string would slip through.
///
/// Both sides are read off the rendered page rather than restated here, because
/// a third copy of the level names is a third place to forget one.
#[test]
fn browser_every_problem_card_has_a_difficulty_the_filter_offers() {
    let page = fs::read_to_string("web/index.html").unwrap();

    // Each `<input>` start tag read whole, so the check does not care what
    // order its attributes are written in.
    let offered: std::collections::BTreeSet<&str> = page
        .split("<input")
        .skip(1)
        .map(|tag| &tag[..tag.find('>').unwrap_or(tag.len())])
        .filter(|tag| tag.contains("name=\"difficulty\""))
        .filter_map(|tag| tag.split("value=\"").nth(1))
        .filter_map(|value| value.split('"').next())
        .collect();
    assert!(
        !offered.is_empty(),
        "web/index.html offers no difficulty filter at all"
    );

    for chunk in page.split("data-difficulty=\"").skip(1) {
        let level = chunk.split('"').next().expect("data-difficulty is quoted");
        assert!(
            offered.contains(level),
            "web/index.html has a {level} problem card, which the difficulty filter never shows"
        );
    }
}

// browser-check long-interview validation
//
// Validation-only fixtures for BROWSER_CHECK_FLOW=long-interview. Each case
// runs through both entry points, scripts/browser-check.sh and
// scripts/browser-check.cjs, because the shell's own
// BROWSER_CHECK_VALIDATE_ENV_ONLY exit never reaches the script that drives the
// browser.

const LONG_RUN_ID: &str = "run-long-1";

struct LongCase {
    name: &'static str,
    env: Vec<(&'static str, &'static str)>,
    unset: Vec<&'static str>,
    manifest: Value,
    /// A fragment both entry points print, or `None` when both accept.
    refusal: Option<&'static str>,
}

/// A qualification run: ninety minutes, manual End, the whole interview
/// observed. A case patches it; `null` deletes a key.
fn long_base_manifest() -> Value {
    json!({
        "run_kind": "qualification",
        "end_mode": "manual",
        "duration_min": 90,
        "observation_target_s": 5400,
        "selected_duration_s": 5400,
    })
}

fn long_case(
    name: &'static str,
    env: &[(&'static str, &'static str)],
    manifest: Value,
    refusal: Option<&'static str>,
) -> LongCase {
    LongCase {
        name,
        env: env.to_vec(),
        unset: Vec::new(),
        manifest,
        refusal,
    }
}

fn long_cases() -> Vec<LongCase> {
    let smoke = |end: &str, duration: u32, target: u32| {
        json!({
            "run_kind": "smoke", "end_mode": end, "duration_min": duration,
            "observation_target_s": target, "selected_duration_s": target,
        })
    };
    let experiment = |duration: u32| {
        json!({
            "run_kind": "experiment", "duration_min": duration,
            "observation_target_s": duration * 60, "selected_duration_s": duration * 60,
            "experiment_profile_min": duration,
        })
    };
    let at = |duration: &'static str| [("BROWSER_CHECK_DURATION_MIN", duration)];
    let pilot = json!({"run_kind": "pilot", "duration_min": 60,
                       "observation_target_s": 3600, "selected_duration_s": 3600});
    let mut cases = vec![
        // Accepted.
        long_case("qualification_90", &[], json!({}), None),
        long_case("pilot_60", &at("60"), pilot, None),
        long_case("experiment_20", &at("20"), experiment(20), None),
        long_case("experiment_30", &at("30"), experiment(30), None),
        long_case(
            "smoke_shorter_target",
            &at("10"),
            smoke("manual", 10, 30),
            None,
        ),
        long_case(
            "smoke_deadline_observes_grace",
            &at("20"),
            smoke("server_deadline", 20, 1320),
            None,
        ),
        long_case(
            "smoke_longest_deadline",
            &at("90"),
            smoke("server_deadline", 90, 5520),
            None,
        ),
        // The wall windows, the archive bound and the stimulus reserve.
        long_case(
            "wall_cap_without_the_outside_windows",
            &[],
            json!({"outer_wall_cap_s": 5400 + 600}),
            Some("is not the selected duration 5400 plus 660s"),
        ),
        long_case(
            "wall_cap_too_generous",
            &[],
            json!({"outer_wall_cap_s": 5400 + 661}),
            Some("is not the selected duration 5400 plus 660s"),
        ),
        long_case(
            "cleanup_reserve_not_end_report_plus_teardown",
            &[],
            json!({"cleanup_reserve_s": 300}),
            Some("is not the end and report window plus teardown, 360s"),
        ),
        long_case(
            "archive_bound_missing",
            &[],
            json!({"max_archive_bytes": null}),
            Some("max_archive_bytes must be a positive integer"),
        ),
        long_case(
            "archive_bound_below_the_samplers_worst_case",
            &[],
            json!({"max_archive_bytes": 1000}),
            Some("is below the"),
        ),
        long_case(
            "archive_bound_above_the_derived_ceiling",
            &[],
            json!({"max_archive_bytes": 1u64 << 40}),
            Some("is above the"),
        ),
        long_case(
            "reserve_shorter_than_the_resolved_grace",
            &[("BROWSER_CHECK_HANDOVER_GRACE_S", "100")],
            json!({}),
            Some("shorter than the resolved handover grace 100s"),
        ),
        long_case(
            "reserve_exactly_the_grace_plus_margin",
            &[("BROWSER_CHECK_HANDOVER_GRACE_S", "90")],
            json!({}),
            None,
        ),
        // Duration spelling and bounds.
        long_case(
            "duration_below_product_floor",
            &at("9"),
            json!({}),
            Some("outside the product range"),
        ),
        long_case(
            "duration_above_product_ceiling",
            &at("91"),
            json!({}),
            Some("outside the product range"),
        ),
        long_case(
            "duration_zero",
            &at("0"),
            json!({}),
            Some("whole number of minutes"),
        ),
        long_case(
            "duration_negative",
            &at("-1"),
            json!({}),
            Some("whole number of minutes"),
        ),
        long_case(
            "duration_empty",
            &at(""),
            json!({}),
            Some("whole number of minutes"),
        ),
        long_case(
            "duration_fractional",
            &at("20.5"),
            json!({}),
            Some("whole number of minutes"),
        ),
        long_case(
            "duration_leading_zero",
            &at("090"),
            json!({}),
            Some("whole number of minutes"),
        ),
        long_case(
            "duration_exponent",
            &at("1e1"),
            json!({}),
            Some("whole number of minutes"),
        ),
        long_case(
            "duration_in_range_but_unsupported",
            &at("45"),
            json!({"duration_min": 45}),
            Some("must be one of 10, 20, 30, 60, 90"),
        ),
        // Duration against run kind, and the manifest's own claim.
        long_case(
            "qualification_needs_ninety",
            &at("60"),
            json!({"duration_min": 60}),
            Some("requires a 90 minute interview"),
        ),
        long_case(
            "pilot_needs_sixty",
            &at("90"),
            json!({"run_kind": "pilot"}),
            Some("requires a 60 minute interview"),
        ),
        long_case(
            "manifest_duration_disagrees",
            &at("90"),
            json!({"duration_min": 60}),
            Some("does not match BROWSER_CHECK_DURATION_MIN"),
        ),
        long_case(
            "manifest_without_duration",
            &at("90"),
            json!({"duration_min": null}),
            Some("does not match BROWSER_CHECK_DURATION_MIN"),
        ),
        // Observation target.
        long_case(
            "full_run_shortened",
            &[],
            json!({"observation_target_s": 5399, "selected_duration_s": 5399}),
            Some("must observe the whole interview"),
        ),
        long_case(
            "full_run_negative_target",
            &[],
            json!({"observation_target_s": -1}),
            Some("observation_target_s must be a positive integer"),
        ),
        long_case(
            "full_run_missing_target",
            &[],
            json!({"observation_target_s": null}),
            Some("observation_target_s must be a positive integer"),
        ),
        long_case(
            "pilot_shortened",
            &at("60"),
            json!({"run_kind": "pilot", "duration_min": 60, "observation_target_s": 3000, "selected_duration_s": 3000}),
            Some("must observe the whole interview"),
        ),
        long_case(
            "smoke_longer_than_interview",
            &at("10"),
            smoke("manual", 10, 601),
            Some("is longer than the 600s interview"),
        ),
        long_case(
            "deadline_smoke_without_grace",
            &at("20"),
            smoke("server_deadline", 20, 1200),
            Some("observes the duration plus 120s"),
        ),
        long_case(
            "selected_shorter_than_target",
            &[],
            json!({"selected_duration_s": 5000}),
            Some("selected_duration_s must be an integer that covers"),
        ),
        // Run kind and end mode.
        long_case(
            "clip_is_native_only",
            &at("10"),
            json!({"run_kind": "clip", "end_mode": "native_close", "duration_min": 10, "observation_target_s": 300, "selected_duration_s": 300}),
            Some("native-worker-only"),
        ),
        long_case(
            "qualification_by_deadline",
            &[],
            json!({"end_mode": "server_deadline"}),
            Some("cannot end by server_deadline"),
        ),
        long_case(
            "smoke_by_absence_is_not_a_browser_run",
            &at("20"),
            smoke("absence", 20, 300),
            Some("cannot end by absence"),
        ),
        long_case(
            "smoke_by_terminal_cancel_is_not_a_browser_run",
            &at("20"),
            smoke("terminal_cancel", 20, 300),
            Some("cannot end by terminal_cancel"),
        ),
        long_case(
            "legacy_explicit_end",
            &[],
            json!({"end_mode": "explicit_end"}),
            Some("not a long interview"),
        ),
        long_case(
            "unknown_run_kind",
            &[],
            json!({"run_kind": "marathon"}),
            Some("run_kind must be one of"),
        ),
        long_case(
            "missing_run_kind",
            &[],
            json!({"run_kind": null}),
            Some("run_kind must be one of"),
        ),
        long_case(
            "experiment_without_profile",
            &at("20"),
            json!({"run_kind": "experiment", "duration_min": 20, "observation_target_s": 1200, "selected_duration_s": 1200}),
            Some("must name its frozen profile"),
        ),
        long_case(
            "experiment_profile_disagrees",
            &at("20"),
            json!({"run_kind": "experiment", "duration_min": 20, "observation_target_s": 1200, "selected_duration_s": 1200, "experiment_profile_min": 30}),
            Some("does not match the 20 minute interview"),
        ),
        long_case(
            "experiment_profile_off_menu",
            &at("60"),
            json!({"run_kind": "experiment", "duration_min": 60, "observation_target_s": 3600, "selected_duration_s": 3600, "experiment_profile_min": 60}),
            Some("must name its frozen profile"),
        ),
        long_case(
            "profile_on_a_qualification",
            &[],
            json!({"experiment_profile_min": 20}),
            Some("applies to experiment runs only"),
        ),
        // Recording cap: a clamp is a refusal, never a quiet shorter run.
        long_case(
            "recording_cap_would_clamp",
            &[("CODETRIAL_RECORDING_MAX_MINUTES", "60")],
            json!({}),
            Some("would clamp the token to 60 minutes"),
        ),
        long_case(
            "recording_cap_covers_duration",
            &[("CODETRIAL_RECORDING_MAX_MINUTES", "90")],
            json!({}),
            None,
        ),
        long_case(
            "recording_cap_malformed",
            &[("CODETRIAL_RECORDING_MAX_MINUTES", "ninety")],
            json!({}),
            Some("CODETRIAL_RECORDING_MAX_MINUTES must be a whole number"),
        ),
        // Identity and environment.
        long_case(
            "run_id_disagrees",
            &[("BROWSER_CHECK_RUN_ID", "run-other")],
            json!({}),
            Some("BROWSER_CHECK_RUN_ID does not match the manifest"),
        ),
        long_case(
            "manifest_hash_disagrees",
            &[(
                "BROWSER_CHECK_RUN_MANIFEST_SHA256",
                "0000000000000000000000000000000000000000000000000000000000000000",
            )],
            json!({}),
            Some("BROWSER_CHECK_RUN_MANIFEST_SHA256 does not match the manifest"),
        ),
        long_case(
            "capture_directory_missing",
            &[(
                "BROWSER_CHECK_CAPTURE",
                "/nonexistent-long-run/capture.json",
            )],
            json!({}),
            Some("BROWSER_CHECK_CAPTURE directory"),
        ),
        long_case(
            "agent_without_interviewer",
            &[("BROWSER_CHECK_AGENT", "home")],
            json!({}),
            Some("needs BROWSER_CHECK_AGENT=rust or dispatch"),
        ),
        long_case(
            "soak_seconds_do_not_apply",
            &[("BROWSER_CHECK_SOAK_SECONDS", "660")],
            json!({}),
            Some("BROWSER_CHECK_SOAK_SECONDS applies to BROWSER_CHECK_FLOW=soak"),
        ),
        long_case(
            "soak_verdict_mode_does_not_apply",
            &[("BROWSER_CHECK_SOAK_VERDICT_MODE", "pilot")],
            json!({}),
            Some("BROWSER_CHECK_SOAK_VERDICT_MODE applies to BROWSER_CHECK_FLOW=soak"),
        ),
        // A 660 second soak is the old flow, not a long interview.
        long_case(
            "legacy_manifest_is_not_a_long_interview",
            &[],
            json!({"run_kind": null, "end_mode": "explicit_end", "duration_min": null,
                   "observation_target_s": null, "selected_duration_s": 660}),
            Some("run_kind must be one of"),
        ),
    ];
    let mut defaulted = long_case(
        "duration_defaults_to_twenty",
        &[],
        smoke("manual", 20, 1200),
        None,
    );
    defaulted.unset = vec!["BROWSER_CHECK_DURATION_MIN"];
    cases.push(defaulted);
    cases
}

/// The entry points under test: the shell script and the script it drives.
fn long_entry(entry: &str) -> Command {
    let mut command = if entry == "shell" {
        let mut command = Command::new("sh");
        command.arg("scripts/browser-check.sh");
        command
    } else {
        let mut command = Command::new("node");
        command.arg("scripts/browser-check.cjs");
        command
    };
    command
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("BROWSER_CHECK_VALIDATE_ENV_ONLY", "1")
        .env("LIVEKIT_URL", "wss://example.livekit.cloud")
        .env("LIVEKIT_API_KEY", "key")
        .env("LIVEKIT_API_SECRET", "secret")
        .env("GOOGLE_API_KEY", "google");
    command
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Runs one validation-only fixture through one entry point and returns
/// (exit code, stderr).
fn run_long_case(case: &LongCase, entry: &str) -> (Option<i32>, String) {
    let dir = unique_temp_path("long-interview", "");
    fs::create_dir_all(&dir).unwrap();
    let candidate = dir.join("candidate");
    fs::write(&candidate, "candidate").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut manifest = json!({
        "schema_version": 1,
        "run_id": LONG_RUN_ID,
        "binaries": {"candidate": {"path": candidate, "sha256": hex_sha256(b"candidate")}},
        "settings_sha256": "0".repeat(64),
        "fixture_sha256": "0".repeat(64),
        "actions": ["candidate_join", "probe"],
        "action_ceiling": 3,
        "limits": {},
        "scopes": {"scripted_actions": "hard", "wall_time": "hard"},
    });
    for patch in [long_base_manifest(), case.manifest.clone()] {
        for (key, value) in patch.as_object().unwrap() {
            if value.is_null() {
                manifest.as_object_mut().unwrap().remove(key);
            } else {
                manifest[key] = value.clone();
            }
        }
    }

    // The wall windows of a long interview follow from its selected duration
    // unless a case states them: setup, end and report, and teardown sit
    // outside it, and the archive bound is the derived floor.
    let selected = manifest["selected_duration_s"].as_u64().unwrap_or(0);
    let cap = manifest
        .get("outer_wall_cap_s")
        .and_then(Value::as_u64)
        .unwrap_or(selected + 660);
    for (key, value) in [
        ("outer_wall_cap_s", json!(selected + 660)),
        ("cleanup_reserve_s", json!(360)),
        (
            "max_archive_bytes",
            json!((cap / 30 + 2) * 4096 + 256 * 1024),
        ),
    ] {
        if manifest.get(key).is_none() && case.manifest.get(key).is_none() {
            manifest[key] = value;
        }
    }
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let manifest_path = dir.join("manifest.json");
    fs::write(&manifest_path, &bytes).unwrap();

    let mut command = long_entry(entry);
    command
        .env("BROWSER_CHECK_FLOW", "long-interview")
        .env("BROWSER_CHECK_AGENT", "rust")
        .env("BROWSER_CHECK_DURATION_MIN", "90")
        .env("BROWSER_CHECK_RUN_MANIFEST", &manifest_path)
        .env("BROWSER_CHECK_RUN_ID", LONG_RUN_ID)
        .env("BROWSER_CHECK_RUN_MANIFEST_SHA256", hex_sha256(&bytes))
        .env("BROWSER_CHECK_BUDGET_SOCKET", dir.join("budget.sock"));
    for (name, value) in &case.env {
        command.env(name, value);
    }
    for name in &case.unset {
        command.env_remove(name);
    }
    let output = command.output().expect("browser check should run");
    fs::remove_dir_all(&dir).unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn browser_check_long_interview_validation_agrees_across_both_entry_points() {
    let cases = long_cases();
    // A table that stopped being discovered would pass by running nothing.
    assert!(
        cases.len() >= 40,
        "only {} fixtures discovered",
        cases.len()
    );
    let mut ran = 0;
    for case in &cases {
        for entry in ["shell", "node"] {
            let (code, stderr) = run_long_case(case, entry);
            ran += 1;
            match case.refusal {
                None => assert_eq!(code, Some(0), "{} via {entry}: {stderr}", case.name),
                Some(fragment) => {
                    assert_eq!(code, Some(2), "{} via {entry}: {stderr}", case.name);
                    assert!(
                        stderr.contains(fragment),
                        "{} via {entry} should say {fragment:?}: {stderr}",
                        case.name
                    );
                }
            }
        }
    }
    assert_eq!(ran, cases.len() * 2);
}

#[test]
fn browser_check_duration_choice_is_refused_by_every_other_flow_on_both_entry_points() {
    // The legacy flows keep their timing, and a duration selected for one is
    // refused rather than relabelled as a long interview.
    for flow in ["voice", "report", "barge", "avatar", "soak"] {
        for entry in ["shell", "node"] {
            let output = long_entry(entry)
                .env("BROWSER_CHECK_FLOW", flow)
                .env("BROWSER_CHECK_AGENT", "rust")
                .env("BROWSER_CHECK_DURATION_MIN", "90")
                .output()
                .expect("browser check should run");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                output.status.code(),
                Some(2),
                "{flow} via {entry}: {stderr}"
            );
            assert!(
                stderr.contains(
                    "BROWSER_CHECK_DURATION_MIN applies to BROWSER_CHECK_FLOW=long-interview"
                ),
                "{flow} via {entry}: {stderr}"
            );
        }
    }

    // The soak itself, at its default 660 seconds, still validates.
    for entry in ["shell", "node"] {
        let output = long_entry(entry)
            .env("BROWSER_CHECK_FLOW", "soak")
            .env("BROWSER_CHECK_AGENT", "rust")
            .env("BROWSER_CHECK_SOAK_SECONDS", "660")
            .output()
            .expect("browser check should run");
        assert_eq!(
            output.status.code(),
            Some(0),
            "soak via {entry}: {output:?}"
        );
    }
}

#[test]
fn browser_check_long_interview_range_is_the_products() {
    // The harness mirrors the product's range and may only narrow it.
    let config = fs::read_to_string("src/config.rs").unwrap();
    let constant = |name: &str| -> u64 {
        config
            .lines()
            .find_map(|line| line.strip_prefix(&format!("pub const {name}: u32 = ")))
            .and_then(|rest| rest.trim_end_matches(';').parse().ok())
            .unwrap_or_else(|| panic!("src/config.rs no longer defines {name}"))
    };
    let output = Command::new("node")
        .args([
            "-e",
            "const p = require('./scripts/long-interview-plan.cjs'); \
             console.log(JSON.stringify([p.PRODUCT_MIN_DURATION_MIN, \
             p.PRODUCT_MAX_DURATION_MIN, p.HARNESS_DURATIONS_MIN, p.DEFAULT_DURATION_MIN]))",
        ])
        .output()
        .unwrap();
    let range: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(range[0], constant("MIN_DURATION_MIN"));
    assert_eq!(range[1], constant("MAX_DURATION_MIN"));
    assert_eq!(range[2], json!([10, 20, 30, 60, 90]));
    assert_eq!(range[3], 20);
    for minutes in range[2].as_array().unwrap() {
        let minutes = minutes.as_u64().unwrap();
        assert!((constant("MIN_DURATION_MIN")..=constant("MAX_DURATION_MIN")).contains(&minutes));
    }
}
