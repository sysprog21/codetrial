use codetrial::agent::{InterviewGrounding, InterviewLoop, InterviewProfile, Seniority};
use codetrial::config::{
    DEFAULT_GEMINI_LIVE_MODEL, DEFAULT_GEMINI_REPORT_MODEL, DEFAULT_GEMINI_VOICE, load_from_pairs,
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
fn bootstrap_keeps_mode_immutable_in_its_prompt_contract() {
    let config = config();
    let practice = bootstrap_with_rounds(
        &config,
        "interview-fixed",
        Some("two-sum"),
        45,
        RuntimeOptions {
            ..RuntimeOptions::default()
        },
    );

    // One interview now, and the frameworks are spoken rather than hidden: the
    // agent names the step it is moving to and says what it listens for in a
    // behavioral answer, while the rubric and the scores stay its own.
    assert!(practice.instructions.contains("REACTO CODING FLOW"));
    assert!(practice.instructions.contains("WHAT STAYS HIDDEN"));
    assert!(!practice.instructions.contains("PRACTICE MODE"));
    assert!(!practice.instructions.contains("SCORED MODE"));
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
