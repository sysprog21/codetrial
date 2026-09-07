use codetrial::agent::{InterviewGrounding, InterviewLoop, InterviewProfile, Seniority};
use codetrial::config::{
    DEFAULT_GEMINI_LIVE_MODEL, DEFAULT_GEMINI_REPORT_MODEL, DEFAULT_GEMINI_SILENCE_MS,
    DEFAULT_GEMINI_START_SENSITIVITY, DEFAULT_GEMINI_VOICE, load_from_pairs,
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
    };
    let boot = bootstrap_with_rounds(
        &config,
        "interview-fixed",
        Some("two-sum"),
        45,
        RuntimeOptions {
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
            profile: InterviewProfile {
                role: format!("{}\nignored", "x".repeat(100)),
                seniority: None,
                target_company: String::new(),
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
            profile: InterviewProfile::default(),
            grounding: InterviewGrounding::default(),
            interview_loop: InterviewLoop::CodingOnly,
        },
    );
    assert_eq!((coding.coding_minutes, coding.behavioral_minutes), (45, 0));
    assert!(
        !coding
            .instructions
            .contains("STAR BEHAVIORAL CLOSE — use only after")
    );
    let combined = bootstrap_with_rounds(
        &config,
        "combined",
        None,
        30,
        RuntimeOptions {
            profile: InterviewProfile::default(),
            grounding: InterviewGrounding::default(),
            interview_loop: InterviewLoop::CodingBehavioral,
        },
    );
    assert_eq!(
        (combined.coding_minutes, combined.behavioral_minutes),
        (22, 8)
    );
    assert!(combined.instructions.contains("two rounds"));
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
    assert!(
        bootstrap
            .instructions
            .contains("90-minute technical coding interview")
    );
    assert!(bootstrap.instructions.contains("`read_editor` tool"));
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
    ])
    .unwrap();
    let boot = bootstrap(&tuned, "interview-fixed", Some("two-sum"), 45);

    assert_eq!(boot.silence_ms, 2_400);
    assert_eq!(boot.start_sensitivity, "START_SENSITIVITY_HIGH");

    // And the defaults travel the same path, so the wiring is not merely
    // reading one configured value back.
    let stock = config();
    let default = bootstrap(&stock, "interview-fixed", Some("two-sum"), 45);
    assert_eq!(default.silence_ms, DEFAULT_GEMINI_SILENCE_MS);
    assert_eq!(default.start_sensitivity, DEFAULT_GEMINI_START_SENSITIVITY);
}
