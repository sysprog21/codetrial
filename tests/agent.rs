// `prompt_samples` names every prompt builder in one `json!` literal, which
// outgrew the macro's default expansion depth.
#![recursion_limit = "256"]

use codetrial::agent::*;
use codetrial::runtime::{TOPIC_CODE_UPDATE, TOPIC_CONTROL, TOPIC_INTEGRITY, TOPIC_TEST_RESULTS};
use serde_json::Value;
use serde_json::json;
use sha2::{Digest, Sha256};

#[path = "common/words.rs"]
mod words;
use words::shared_run;

/// The defaults src/runtime.rs supplies at the one production call site, so a
/// test that cares about a single argument does not spell out the other five.
fn instructions(problem: &Problem, duration_min: u32) -> String {
    build_instructions_for_plan(
        problem,
        duration_min,
        &InterviewProfile::default(),
        &InterviewGrounding::default(),
        InterviewLoop::CodingBehavioral,
    )
}

/// Whether a piece of text gives away which published problem an exercise
/// was written from, by the rule `words::names_title` shares with the
/// generator.
fn names_source(problem: &Problem, text: &str) -> bool {
    words::names_title(problem.source_title().unwrap_or(""), text)
}

struct IntegrityEventInput<'a> {
    seq: u64,
    prev_hash: &'a str,
    event_type: &'a str,
    at: &'a str,
    severity: &'a str,
    source: &'a str,
    duration_ms: u64,
    detail: Option<&'a str>,
}

fn integrity_event(input: IntegrityEventInput<'_>) -> Value {
    integrity_event_with_source_ids(input, &[])
}

fn integrity_event_with_source_ids(
    input: IntegrityEventInput<'_>,
    source_event_ids: &[&str],
) -> Value {
    let mut event = json!({
        "type": input.event_type,
        "at": input.at,
        "severity": input.severity,
        "source": input.source,
        "durationMs": input.duration_ms,
        "seq": input.seq,
        "prevHash": input.prev_hash,
        "detail": input.detail,
        "sourceEventIds": source_event_ids,
    });
    let mut body = json!({
        "seq": input.seq,
        "prevHash": input.prev_hash,
        "type": input.event_type,
        "at": input.at,
        "severity": input.severity,
        "source": input.source,
        "durationMs": input.duration_ms,
        "detail": input.detail,
    });
    if !source_event_ids.is_empty() {
        body["sourceEventIds"] = json!(source_event_ids);
    }
    let digest = Sha256::digest(canonical_json(&body).as_bytes());
    event["hash"] = Value::String(digest.iter().map(|byte| format!("{byte:02x}")).collect());
    event
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(object) => format!(
            "{{{}}}",
            object
                .iter()
                .map(|(key, value)| format!(
                    "{}:{}",
                    serde_json::to_string(key).unwrap(),
                    canonical_json(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => serde_json::to_string(value).unwrap(),
    }
}

/// A state whose editor holds code the candidate wrote. Coding, Test and
/// Optimizations evidence is refused without it, so a test about something
/// else that records those phases starts here.
fn with_written_code(state: RuntimeState) -> RuntimeState {
    // An empty starter on file, so a packet a test sends later is measured
    // against it instead of becoming the language's first buffer itself.
    let code_templates = [(state.language.clone(), String::new())].into();
    RuntimeState {
        code: "def solve(nums):\n    return sorted(nums)\n".to_string(),
        code_templates,
        ..state
    }
}

/// A state whose clock has reached the point the browser announces the warning
/// at, which the agent now checks before believing the packet.
fn near_time_up(state: RuntimeState) -> RuntimeState {
    let planned = u64::from(state.coding_minutes + state.behavioral_minutes) * 60;
    RuntimeState {
        started_at: std::time::Instant::now()
            - std::time::Duration::from_secs(planned - TIME_WARNING_S),
        ..state
    }
}

/// One candidate turn that outruns the 12,000 bytes a cold restart recovers
/// on its own, so whatever came before it is out of the replacement's view.
fn overflowing_turn() -> String {
    format!("Candidate: {}", "so ".repeat(4_000))
}

/// What `EvidenceLedger::prompt_view` renders for a session nobody has typed
/// in yet (`early`), and for one a few minutes in (`working`).
///
/// Real output, and held to it. `prompt_view` is crate-internal, so this file
/// cannot call it. The projections live in the fixture named below, and the
/// unit test `the_prompt_samples_carry_what_prompt_view_renders` builds the
/// two ledgers and fails when the fixture stops matching.
/// Frozen strings here used to be checked against nothing, so a change to the
/// projection's shape would have been regenerated straight into the prompt
/// golden while the samples described a ledger production no longer sends.
fn evidence_projection(which: &str) -> String {
    let projections: Value =
        serde_json::from_str(include_str!("fixtures/evidence-projections.json"))
            .expect("the projection fixture parses");
    projections[which]
        .as_str()
        .expect("the projection fixture holds this sample")
        .to_string()
}

/// Every prompt the agent sends, in one place, so the frozen fixture and the
/// regeneration path cannot drift apart.
fn prompt_samples() -> Value {
    let empty = evidence_projection("empty");
    let early = evidence_projection("early");
    let working = evidence_projection("working");
    let working_changed = evidence_projection("workingChanged");
    let working_interim = evidence_projection("workingInterim");
    let working_report = evidence_projection("workingReport");
    let excerpt = changed_excerpt(
        "python",
        "def two_sum(nums, target):\n    return []\n",
        "def two_sum(nums, target):\n    seen = {}\n    return []\n",
    )
    .expect("an edit has an excerpt");
    let problem = get_problem(Some("two-sum"));
    let full_profile = InterviewProfile {
        role: "backend engineer".to_string(),
        seniority: Some(Seniority::Staff),
        target_company: "Example Co".to_string(),
        practice_focus: "Test boundaries".to_string(),
    };
    let cold_state = RuntimeState {
        code: "def two_sum(nums, target):".to_string(),
        ..RuntimeState::default()
    };
    let behavioral_state = RuntimeState {
        behavioral_round_started: true,
        transcript: vec![
            "Interviewer: Tell me about a tricky debugging problem you solved.".to_string(),
            "Candidate: I cannot think of an example right now.".to_string(),
        ],
        ..RuntimeState::default()
    };
    json!({
        "resume": resume(false),
        "resumeBehavioral": resume(true),
        "roundStarted": round_started(),
        "roundSkipped": round_skipped(),
        "coldRestartBehavioral": cold_restart(&behavioral_state),
        "coldRestartBehavioralInFlight": cold_restart(&RuntimeState {
            behavioral_round_started: true,
            behavioral_round_transcript_start: 1,
            behavioral_round_prior_turn: Some((0, "Interviewer: That covers the code.".to_string())),
            transcript: vec!["Interviewer: That covers the code. Tell me about a tricky bug you tracked down.".to_string()],
            ..RuntimeState::default()
        }),
        "coldRestartBehavioralOpened": cold_restart(&RuntimeState {
            behavioral_round_started: true,
            behavioral_round_transcript_start: 1,
            transcript: vec!["Candidate: The map lookup is constant time.".to_string()],
            ..RuntimeState::default()
        }),
        "coldRestartBehavioralTruncated": cold_restart(&RuntimeState {
            behavioral_round_started: true,
            behavioral_round_transcript_start: 0,
            transcript: vec![overflowing_turn()],
            ..RuntimeState::default()
        }),
        "timeBehavioral": behavioral_time_warning(),
        "instructions": instructions(problem, 45),
        "instructionsProfile": build_instructions_for_plan(
            problem,
            45,
            &full_profile,
            &InterviewGrounding::default(),
            InterviewLoop::CodingBehavioral,
        ),
        "greeting": greeting(problem),
        "languageChoice": language_choice("C++", LanguageChoiceContext::Start),
        "languageSwitch": language_choice("Java", LanguageChoiceContext::SwitchWithCode),
        "silenceBehavioral": behavioral_silence_nudge(),
        "silenceEmpty": silence_nudge(&empty, None),
        "silenceEarly": silence_nudge(&early, None),
        "silenceWorking": silence_nudge(&working, Some(&excerpt)),
        "coldRestart": cold_restart(&cold_state),
        "coldRestartEmpty": cold_restart(&RuntimeState::default()),
        "review": proactive_review(&working_changed, Some(&excerpt)),
        "reviewWithoutExcerpt": proactive_review(&working, None),
        "time": time_warning(),
        "wrapCandidate": wrap_up("candidate_ended", false),
        "wrapTimer": wrap_up("time_up", false),
        "wrapBehavioral": wrap_up("time_up", true),
        "wrapComplete": wrap_up("interview_complete", false),
        "interim": interim_review_prompt(&InterimReviewInput {
            problem,
            transcript_window: "Candidate: I will use a hash map.",
            code: "seen = {}",
            language: "python",
            already_recorded: "Candidate restated the inputs and the return shape.",
            evidence: &working_interim,
        }),
        "interimEmpty": interim_review_prompt(&InterimReviewInput {
            problem,
            transcript_window: "",
            code: "",
            language: "python",
            already_recorded: "",
            evidence: &empty,
        }),
        "interimSystem": interim_system_instruction(),
        "reportSystem": report_system_instruction(),
        "testsPass": test_results_reaction("3/3 passed", true, None),
        "testsFail": test_results_reaction(
            "2/3 passed",
            false,
            changed_excerpt("python", "", "def two_sum(nums, target):\n    return []").as_deref(),
        ),
        "testsSetupError": test_setup_error_reaction("The runner could not start.", None),
        "logHint": log_hint_text(2),
        "hintRung": hint_rung_text(2, 2, "Compare the current value with what you recorded."),
        "hintRungWithheld": hint_rung_withheld_text(2),
        "report": report_prompt(ReportPromptInput {
            problem,
            transcript: "Candidate: I will use a hash map.",
            rolling_assessment: "",
            final_code: "def two_sum(nums, target): return []",
            language: "python",
            hints_used: 2,
            hint_rung: 2,
            volunteered_hints: 0,
            duration_min: 45,
            elapsed_min: 12.4,
            test_summary: "Latest test run: 2/3 cases passed.\n- CANDIDATE CASE empty input with input [[]]: got []",
            practice_level: None,
            evidence: &working_report,
        }),
        "reportEmpty": report_prompt(ReportPromptInput {
            problem,
            transcript: "",
            rolling_assessment: "",
            final_code: "",
            language: "python",
            hints_used: 0,
            hint_rung: 0,
            volunteered_hints: 0,
            duration_min: 45,
            elapsed_min: 0.0,
            test_summary: "",
            practice_level: None,
            evidence: "",
        }),
        "reportHalfElapsed": report_prompt(ReportPromptInput {
            problem,
            transcript: "",
            rolling_assessment: "",
            final_code: "",
            language: "python",
            hints_used: 0,
            hint_rung: 0,
            volunteered_hints: 0,
            duration_min: 45,
            elapsed_min: 12.5,
            test_summary: "",
            practice_level: None,
            evidence: "",
        }),

        // Assembled by the real builder rather than written out here. A
        // hand-copied literal would freeze the shape this fixture believes in,
        // which is the one thing a golden fixture must not do: the two labels
        // could then drift and nothing would notice.
        "reportProgressive": report_prompt(ReportPromptInput {
            problem,
            transcript: "Candidate: I will use a hash map.",
            rolling_assessment: &rolling_assessment(

                // A real row, not a hand-copied approximation of one. `at_ms`
                // is the only field a fixture cannot freeze, and the rendering
                // deliberately leaves it out.
                &[FrameworkEvidence {
                    at_ms: 0,
                    phase: FrameworkPhase::Algorithm,
                    source: EvidenceSource::CandidateSpeech,
                    kind: EvidenceKind::Observed,
                    confidence: 90,
                    summary: "Candidate chose a hash map and said why.".to_string(),
                    framework_version: FRAMEWORK_VERSION,
                }],
                &["Candidate named the duplicate-value case unprompted.".to_string()],
            ),
            final_code: "def two_sum(nums, target): return []",
            language: "python",
            hints_used: 2,
            hint_rung: 2,
            volunteered_hints: 0,
            duration_min: 45,
            elapsed_min: 12.4,
            test_summary: "Latest test run: 2/3 cases passed.",
            practice_level: None,
            evidence: "",
        }),
        "reportMultiline": report_prompt(ReportPromptInput {
            problem,
            transcript: "Candidate: I will use a hash map.",
            rolling_assessment: "",
            final_code: "def two_sum(nums, target):\n    return [0, 1]",
            language: "python",
            hints_used: 1,
            hint_rung: 1,
            volunteered_hints: 0,
            duration_min: 45,
            elapsed_min: 12.0,
            test_summary: "Latest test run (run #1, python): 2/3 cases passed.",
            practice_level: None,
            evidence: "",
        }),
    })
}

fn valid_strict_report() -> Value {
    let improvements = [
        ("Algorithm", "Explain complexity"),
        ("Test", "Test boundaries"),
        ("Action", "Name your own action"),
        ("Result", "State the result"),
    ];
    let phases = [
        "Repeat",
        "Example",
        "Algorithm",
        "Coding",
        "Test",
        "Optimizations",
        "Situation",
        "Task",
        "Action",
        "Result",
    ];
    json!({
        "codingScore": 82,
        "communicationScore": 74,
        "decision": "HIRE",
        "summary": "You produced a grounded solution and explained the main trade-offs.",
        "codingFeedback": {"strengths": ["Correct core", "Clear implementation"], "improvements": ["Explain complexity", "Test boundaries"]},
        "communicationFeedback": {"strengths": ["Clear narration", "Direct answers"], "improvements": ["Name your own action", "State the result"]},
        "improvementPlan": improvements.iter().map(|(phase, weakness)| json!({
            "phase": phase, "weakness": weakness, "impact": "medium", "frequency": 1,
            "drill": "Practice the missing step", "durationMin": 5,
            "successCriterion": "State it without prompting", "selfReview": ["Grounded in evidence"]
        })).collect::<Vec<_>>(),
        "frameworkAssessment": {"rubricVersion": 1, "phases": phases.iter().map(|phase| json!({
            "phase": phase, "score": 75,
            "weaknessTags": improvements.iter().filter(|(assigned, _)| assigned == phase).map(|(_, weakness)| *weakness).collect::<Vec<_>>()
        })).collect::<Vec<_>>()}
    })
}

const FRAMEWORK_PHASE_NAMES: [&str; 10] = [
    "Repeat",
    "Example",
    "Algorithm",
    "Coding",
    "Test",
    "Optimizations",
    "Situation",
    "Task",
    "Action",
    "Result",
];

fn exact_fixture_keys(value: &Value, expected: &[&str], path: &str) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("{path} must be an object"));
    let actual = object
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let expected = expected
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected, "{path} has schema drift");
}

/// What the report model reads: the system instruction and the brief.
fn model_report_input(brief: String) -> String {
    format!("{}\n\n{brief}", report_system_instruction())
}

/// Puts the interview past its coding round, which is the state the browser's
/// one `round_transition` announcement arrives in.
///
/// Spends the budget rather than winding `started_at` back: `Instant` counts
/// from boot, and subtracting the default thirty-seven minute budget panics
/// outright on a machine that has been up for less than that.
fn past_the_coding_round(state: &mut RuntimeState) {
    state.coding_minutes = 0;
}

/// Past the coding round with Test and Optimizations evidence recorded, so the
/// round transition's completion gate opens the behavioral round.
fn past_the_coding_gate(state: &mut RuntimeState) {
    past_the_coding_round(state);
    for phase in ["test", "optimizations"] {
        record_framework_evidence(state, &json!({"phase": phase, "source": "candidate_speech", "kind": "observed", "confidence": 90, "summary": format!("candidate completed {phase}")})).unwrap();
    }
}

fn evaluation_reaction(case: &Value, state: &mut RuntimeState) -> String {
    let reaction = &case["reaction"];
    let code = reaction["code"].as_str().expect("reaction code is text");
    if !code.is_empty() {
        let result = apply_data_event(
            state,
            TOPIC_CODE_UPDATE,
            &json!({"code": code, "language": "python"}),
            99.0,
        );
        assert!(
            result.update_last_code_change,
            "scenario code did not reach production state"
        );
        assert_eq!(state.code, code);
    }
    match reaction["kind"].as_str().expect("reaction kind is text") {
        "silence" => silence_nudge("", changed_excerpt("python", "", code).as_deref()),
        "proactive" => proactive_review("", changed_excerpt("python", "", code).as_deref()),
        "tests_failed" | "tests_passed" => {
            let all_passed = reaction["kind"] == "tests_passed";
            let before = state.test_runs;
            let result = apply_data_event(
                state,
                TOPIC_TEST_RESULTS,
                &json!({
                    "language": "python", "passed": if all_passed { 3 } else { 1 },
                    "total": 3, "cases": [], "setupError": null
                }),
                99.0,
            );
            assert_eq!(state.test_runs, before + 1);
            assert!(state.last_test_run.is_some());
            result
                .generate_reply
                .expect("test event produces a reaction")
        }
        "round_gate" => {
            past_the_coding_round(state);
            apply_data_event(
                state,
                TOPIC_CONTROL,
                &json!({"type":"round_transition", "round":"behavioral"}),
                99.0,
            )
            .generate_reply
            .expect("round gate produces a reaction")
        }
        "report" => model_report_input(report_prompt(ReportPromptInput {
            problem: get_problem(Some("two-sum")),
            transcript: case["transcript"].as_str().expect("transcript is text"),
            rolling_assessment: "",
            final_code: code,
            language: "python",
            hints_used: case["hintsUsed"].as_u64().expect("hint count is integer") as u32,
            hint_rung: 0,
            volunteered_hints: 0,
            duration_min: 45,
            elapsed_min: 20.0,
            test_summary: "No trusted server-side test was available.",
            practice_level: None,
            evidence: "",
        })),
        other => panic!("unknown reaction kind {other}"),
    }
}

fn record_evaluation_evidence(state: &mut RuntimeState, item: &Value, id: &str) -> String {
    let recorded = record_framework_evidence(state, item)
        .unwrap_or_else(|error| panic!("{id}: invalid evidence: {error}"));
    let recorded_json = framework_evidence_json(&recorded);
    for key in ["phase", "source", "kind", "confidence", "summary"] {
        assert_eq!(
            recorded_json[key], item[key],
            "{id}: evidence drift at {key}"
        );
    }
    recorded_json["phase"].as_str().unwrap().to_string()
}

// Cross-implementation wire fixtures.
//
// Every data-channel topic has a producer in `web/lib.js` and a consumer in
// `src/agent.rs`, and each used to be tested only against itself. That is how
// the integrity hash diverged: both sides passed their own tests, the agent
// refused every event the browser sent, and the gate stayed green while camera
// interviews shipped reports with the evidence section empty.
//
// The fixtures below are real producer output, generated by
// `scripts/gen-wire-fixtures.mjs`, which also runs `--check` in
// `scripts/test.sh` so a producer change cannot leave them stale. Do not
// hand-edit them; regenerating them is the only thing that makes them evidence.

fn wire_fixture(raw: &str) -> (String, Vec<(String, Value)>) {
    let fixture: Value = serde_json::from_str(raw).expect("fixture parses");
    let topic = fixture["topic"]
        .as_str()
        .expect("fixture names the topic it was published on")
        .to_string();
    let cases = fixture["cases"]
        .as_array()
        .expect("fixture carries cases")
        .iter()
        .map(|case| {
            let name = case["name"].as_str().expect("case is named").to_string();
            (name, case["payload"].clone())
        })
        .collect();
    (topic, cases)
}

fn wire_case<'a>(cases: &'a [(String, Value)], name: &str) -> &'a Value {
    cases
        .iter()
        .find(|(case, _)| case == name)
        .map(|(_, payload)| payload)
        .unwrap_or_else(|| panic!("the fixture no longer carries the {name} case"))
}

/// The agent's reaction to a test run is chosen from `passed`, `total`, and
/// `setupError`. A renamed field on the producer side does not fail anything;
///
/// Builds a chain the way `web/interview.js` does: every event takes the next
/// sequence number and hangs off the previous hash, whatever the agent decides
/// to do with it.
struct Chain {
    seq: u64,
    prev_hash: String,
}

impl Chain {
    fn new() -> Self {
        Self {
            seq: 0,
            prev_hash: String::new(),
        }
    }

    fn push(&mut self, state: &mut RuntimeState, event_type: &str, severity: &str) {
        self.push_citing(state, event_type, severity, &[]);
    }

    fn push_citing(
        &mut self,
        state: &mut RuntimeState,
        event_type: &str,
        severity: &str,
        source_event_ids: &[&str],
    ) {
        self.seq += 1;
        let event = integrity_event_with_source_ids(
            IntegrityEventInput {
                seq: self.seq,
                prev_hash: &self.prev_hash,
                event_type,
                at: "2026-08-15T00:00:00.000Z",
                severity,
                source: if event_type == "INTEGRITY_HEARTBEAT" {
                    "heartbeat"
                } else {
                    "camera"
                },
                duration_ms: 0,
                detail: None,
            },
            source_event_ids,
        );
        self.prev_hash = event["hash"].as_str().expect("hash").to_string();
        apply_data_event(state, "integrity", &event, TEST_REACTION_COOLDOWN_S);
    }
}

fn stored_types(state: &RuntimeState) -> Vec<&str> {
    state
        .integrity_events
        .iter()
        .filter_map(|event| event["type"].as_str())
        .collect()
}

// The tests live in `tests/agent/`, declared below, as modules of this target
// rather than targets of their own: cargo discovers only `tests/*.rs`, and this
// crate statically links libwebrtc, so a file per subject there would pay that
// link once per file. Helpers and imports stay here.
//
// `include_str!` resolves against the file that writes it, so a fixture path
// that moved into `tests/agent/` carries one more `../` than it does here.

#[path = "agent/prompts.rs"]
mod prompts;

#[path = "agent/framework.rs"]
mod framework;

#[path = "agent/wire.rs"]
mod wire;

#[path = "agent/integrity.rs"]
mod integrity;

#[path = "agent/report.rs"]
mod report;

#[path = "agent/problems.rs"]
mod problems;

#[path = "agent/runtime.rs"]
mod runtime;
