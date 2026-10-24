#![allow(clippy::panic, clippy::unwrap_used)] // see [lints.clippy] in Cargo.toml

use codetrial::agent::{
    InterviewGrounding, InterviewLoop, InterviewMode, InterviewProfile, Seniority,
};
use codetrial::config::{
    DEFAULT_GEMINI_END_SENSITIVITY, DEFAULT_GEMINI_LIVE_MODEL, DEFAULT_GEMINI_REPORT_MODEL,
    DEFAULT_GEMINI_SILENCE_MS, DEFAULT_GEMINI_START_SENSITIVITY, DEFAULT_GEMINI_VOICE,
    load_from_pairs,
};
use codetrial::runtime::{RuntimeOptions, agent_identity, bootstrap, bootstrap_with_rounds};

fn config() -> codetrial::config::AgentConfig {
    load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap()
}

#[test]
fn example_drawing_is_explicit_and_coding_only() {
    let config = config();
    for mode in [InterviewMode::Coding, InterviewMode::Whiteboard] {
        for enabled in [false, true] {
            let boot = bootstrap_with_rounds(
                &config,
                "example-opt-in",
                Some("two-sum"),
                45,
                RuntimeOptions {
                    example_drawing_enabled: enabled,
                    interview_mode: mode,
                    ..Default::default()
                },
            );
            let active = enabled && mode == InterviewMode::Coding;
            assert_eq!(boot.example_drawing_enabled, active);
            assert_eq!(boot.instructions.contains("Example drawing is an"), active);
            let tools = codetrial::gemini::live_tool_declarations(
                boot.interview_loop,
                mode,
                boot.example_drawing_enabled,
            );
            assert_eq!(
                tools
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["name"] == "read_board"),
                active || mode == InterviewMode::Whiteboard
            );
            let system = codetrial::agent::report_system_instruction_for_examples(
                mode,
                boot.example_drawing_enabled,
            );
            assert_eq!(system.contains("CODING EXAMPLE DRAWINGS:"), active);
        }
    }
    for value in [
        serde_json::json!(false),
        serde_json::json!("true"),
        serde_json::json!(1),
        serde_json::Value::Null,
    ] {
        let parsed = codetrial::agent::parse_participant_metadata(Some(
            &serde_json::json!({"exampleDrawing":value}).to_string(),
        ));
        assert!(!parsed.example_drawing_enabled);
    }
    assert!(
        codetrial::agent::parse_participant_metadata(Some("{\"exampleDrawing\":true}"))
            .example_drawing_enabled
    );
}

/// Unlike example drawing, candidate-led coding is not coding-only: the
/// Algorithm and Coding steps it is scoped to exist at a whiteboard too, so
/// the instructions carry it in both modes.
#[test]
fn candidate_led_is_explicit_in_both_modes() {
    let config = config();
    for mode in [InterviewMode::Coding, InterviewMode::Whiteboard] {
        for enabled in [false, true] {
            let boot = bootstrap_with_rounds(
                &config,
                "candidate-led-opt-in",
                Some("two-sum"),
                45,
                RuntimeOptions {
                    candidate_led_enabled: enabled,
                    interview_mode: mode,
                    ..Default::default()
                },
            );
            assert_eq!(boot.candidate_led_enabled, enabled);
            assert_eq!(boot.instructions.contains("CANDIDATE-LED CODING"), enabled);
        }
    }
    for value in [
        serde_json::json!(false),
        serde_json::json!("true"),
        serde_json::json!(1),
        serde_json::Value::Null,
    ] {
        let parsed = codetrial::agent::parse_participant_metadata(Some(
            &serde_json::json!({"candidateLed":value}).to_string(),
        ));
        assert!(!parsed.candidate_led_enabled);
    }
    assert!(
        codetrial::agent::parse_participant_metadata(Some("{\"candidateLed\":true}"))
            .candidate_led_enabled
    );
}

#[test]
fn bootstrap_resolves_requested_problem_and_room() {
    let config = config();
    let bootstrap = bootstrap(&config, "interview-fixed", Some("merge-intervals"), 30);

    assert_eq!(bootstrap.room_name, "interview-fixed");
    assert_eq!(bootstrap.problem.id, "merge-intervals");
    assert_eq!(bootstrap.duration_min, 30);
}

#[test]
fn bootstrap_carries_only_the_validated_optional_profile() {
    let config = config();
    let profile = InterviewProfile {
        role: "Backend engineer".to_string(),
        seniority: Some(Seniority::Senior),
        target_company: "Example Co".to_string(),
        practice_focus: String::new(),
    };
    let boot = bootstrap_with_rounds(
        &config,
        "interview-fixed",
        Some("two-sum"),
        45,
        RuntimeOptions {
            example_drawing_enabled: false,
            profile: profile.clone(),
            ..RuntimeOptions::default()
        },
    );
    assert_eq!(boot.profile, profile);
    assert!(boot.instructions.contains("Backend engineer"));
    assert!(boot.instructions.contains("candidate selected senior"));
    let normalized = bootstrap_with_rounds(
        &config,
        "direct",
        None,
        45,
        RuntimeOptions {
            example_drawing_enabled: false,
            profile: InterviewProfile {
                role: format!("{}\nignored", "x".repeat(100)),
                seniority: None,
                target_company: String::new(),
                practice_focus: String::new(),
            },
            ..RuntimeOptions::default()
        },
    );
    assert_eq!(normalized.profile.role.chars().count(), 80);
    assert!(!normalized.profile.role.contains('\n'));
    assert!(
        bootstrap(&config, "legacy", None, 45)
            .profile
            .role
            .is_empty()
    );
}

#[test]
fn bootstrap_carries_ephemeral_grounding_without_changing_profile() {
    let config = config();
    let grounding = InterviewGrounding {
        requirements: vec!["Own services".into()],
        skills: vec![],
        anchors: vec!["Led a migration".into()],
    };
    let boot = bootstrap_with_rounds(
        &config,
        "room",
        None,
        45,
        RuntimeOptions {
            example_drawing_enabled: false,
            grounding: grounding.clone(),
            ..RuntimeOptions::default()
        },
    );
    assert_eq!(boot.grounding, grounding);
    assert_eq!(boot.profile, InterviewProfile::default());
    assert!(boot.instructions.contains("Led a migration"));
}

#[test]
fn bootstrap_owns_validated_round_plan_and_budgets() {
    let config = config();
    let coding = bootstrap_with_rounds(
        &config,
        "coding",
        None,
        45,
        RuntimeOptions {
            example_drawing_enabled: false,
            profile: InterviewProfile::default(),
            grounding: InterviewGrounding::default(),
            interview_loop: InterviewLoop::CodingOnly,
            interview_mode: InterviewMode::Coding,
            ..RuntimeOptions::default()
        },
    );
    assert_eq!((coding.coding_minutes, coding.behavioral_minutes), (45, 0));
    assert!(
        coding
            .instructions
            .contains("coding only. The REACTO coding round owns all 45 minutes.")
    );
    let combined = bootstrap_with_rounds(
        &config,
        "combined",
        None,
        30,
        RuntimeOptions {
            example_drawing_enabled: false,
            profile: InterviewProfile::default(),
            grounding: InterviewGrounding::default(),
            interview_loop: InterviewLoop::CodingBehavioral,
            interview_mode: InterviewMode::Coding,
            ..RuntimeOptions::default()
        },
    );
    assert_eq!(
        (combined.coding_minutes, combined.behavioral_minutes),
        (22, 8)
    );
    assert!(combined.instructions.contains(
        "two rounds: the REACTO coding round has 22 minutes and the STAR behavioral reserve has 8 minutes."
    ));
}

#[test]
fn bootstrap_preserves_example_visibility_with_an_accent() {
    let config = config();
    for accent in [None, Some("British")] {
        for examples_hidden in [false, true] {
            let boot = bootstrap_with_rounds(
                &config,
                "interview-fixed",
                Some("two-sum"),
                45,
                RuntimeOptions {
                    accent,
                    examples_hidden,
                    ..RuntimeOptions::default()
                },
            );
            assert_eq!(
                boot.instructions
                    .contains("none are on their screen: never point them at an example."),
                examples_hidden,
                "accent={accent:?}, examples_hidden={examples_hidden}"
            );
            assert_eq!(
                boot.instructions
                    .contains("implement and one or two worked examples"),
                !examples_hidden,
                "accent={accent:?}, examples_hidden={examples_hidden}"
            );
            assert_eq!(
                boot.instructions.contains("natural British accent"),
                accent.is_some()
            );
            assert_eq!(
                boot.instructions
                    .matches("[INTERVIEWER SPEAKING STYLE]")
                    .count(),
                usize::from(accent.is_some())
            );
        }
    }
}

#[test]
fn bootstrap_preserves_python_gemini_defaults_and_prompts() {
    let config = config();

    // Both ends of the clamp. Only the ceiling was pinned, and the floor is the
    // side with teeth: the prompt says "a {duration}-minute interview" and the
    // round plan divides this number, so a zero reaches Gemini as an interview
    // with no time in it and a coding round of `0 - 8` minutes saturating back
    // to zero.
    assert_eq!(
        bootstrap(&config, "interview-fixed", Some("two-sum"), 0).duration_min,
        10
    );
    let bootstrap = bootstrap(&config, "interview-fixed", Some("two-sum"), 500);

    assert_eq!(bootstrap.duration_min, 90);
    assert_eq!(bootstrap.live_model, DEFAULT_GEMINI_LIVE_MODEL);
    assert_eq!(bootstrap.report_model, DEFAULT_GEMINI_REPORT_MODEL);
    assert_eq!(bootstrap.voice, DEFAULT_GEMINI_VOICE);
    assert_eq!(bootstrap.accent, None);
    assert!(
        bootstrap
            .instructions
            .contains("90-minute coding interview over video")
    );
    assert!(
        bootstrap
            .instructions
            .contains("`read_editor`: to check recorded phases")
    );
    assert!(
        bootstrap
            .greeting
            .contains("[SYSTEM EVENT] The interview starts now")
    );
}

#[test]
fn agent_identity_is_room_scoped_and_predictable() {
    assert_eq!(
        agent_identity("interview-fixed"),
        "interviewer-interview-fixed"
    );
}

/// The two endpointing knobs `bootstrap` copies out of the config.
///
/// `src/gemini.rs` pins that both reach the Gemini setup frame, but it asserts
/// them against `boot.silence_ms` and `boot.start_sensitivity` themselves, so
/// it holds however this struct was filled: wire the field to a literal here
/// and that test still passes while the operator's configured values stop
/// leaving the process. Absent or wrong, Gemini substitutes its own defaults
/// and the wait before the interviewer replies -- and whether it cuts the
/// candidate off -- stops being anything anyone can tune.
#[test]
fn bootstrap_carries_the_configured_endpointing_window() {
    let tuned = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
        ("GEMINI_SILENCE_MS", "2400"),
        ("GEMINI_START_SENSITIVITY", "HIGH"),
        ("GEMINI_END_SENSITIVITY", "low"),
    ])
    .unwrap();
    let boot = bootstrap(&tuned, "interview-fixed", Some("two-sum"), 45);

    assert_eq!(boot.silence_ms, 2_400);
    assert_eq!(boot.start_sensitivity, "START_SENSITIVITY_HIGH");
    assert_eq!(boot.end_sensitivity, "END_SENSITIVITY_LOW");

    // And the defaults travel the same path, so the wiring is not merely
    // reading one configured value back.
    let stock = config();
    let default = bootstrap(&stock, "interview-fixed", Some("two-sum"), 45);
    assert_eq!(default.silence_ms, DEFAULT_GEMINI_SILENCE_MS);
    assert_eq!(default.start_sensitivity, DEFAULT_GEMINI_START_SENSITIVITY);
    assert_eq!(default.end_sensitivity, DEFAULT_GEMINI_END_SENSITIVITY);
}

/// The board reaches Gemini on the stream a camera frame does, so a whiteboard
/// interview never forwards the camera, even where the operator enabled it.
#[test]
fn a_whiteboard_interview_keeps_the_camera_off_the_board_stream() {
    let enabled = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
        ("CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED", "true"),
    ])
    .unwrap();
    let held_at = |interview_mode| {
        bootstrap_with_rounds(
            &enabled,
            "interview-fixed",
            Some("two-sum"),
            45,
            RuntimeOptions {
                example_drawing_enabled: false,
                interview_mode,
                ..RuntimeOptions::default()
            },
        )
        .candidate_video
    };
    assert!(held_at(InterviewMode::Coding));
    assert!(!held_at(InterviewMode::Whiteboard));
}
