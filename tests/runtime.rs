use codetrial::config::{
    DEFAULT_GEMINI_LIVE_MODEL, DEFAULT_GEMINI_REPORT_MODEL, DEFAULT_GEMINI_VOICE, load_from_pairs,
};
use codetrial::runtime::{agent_identity, bootstrap};

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
