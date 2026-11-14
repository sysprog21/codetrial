//! What the report rules say about credit the candidate did not earn.
//!
//! Split out of `tests/agent/prompts.rs`, which is at its size ceiling; like
//! it, this compiles as a module of the `tests/agent.rs` binary.

use super::*;

/// The brief hands the reviewer the contract, so a restatement that misstated
/// the output and dropped the buy-before-sell rule was credited as accurate,
/// with the interviewer's "That's right" read as confirmation. The rule that
/// keeps the contract apart from what the candidate restated has to reach
/// every surface's reviewer beside that contract. It lowers the chance of
/// unsupported credit; only a model can show it is heeded.
#[test]
fn contract_is_not_credited_as_candidate_restatement() {
    let prompt = report_prompt(ReportPromptInput {
        problem: get_problem(Some("best-time-to-buy-and-sell-stock")),
        interview_mode: InterviewMode::Coding,
        board_attached: false,
        transcript: "Candidate: So I get the list of daily prices and return the maximum price.\n\
                     Interviewer: That's right, go ahead with an example.",
        rolling_assessment: "",
        final_code: "",
        language: "c",
        hints_used: 0,
        hint_rung: 0,
        volunteered_hints: 0,
        duration_min: 45,
        elapsed_min: 10.0,
        test_summary: "",
        practice_level: None,
        evidence: "",
        behavioral_round: BehavioralRound::Opened,
        follow_ups_released: false,
        communication_evidence: CommunicationEvidence::Sufficient,
    });
    assert!(prompt.contains("over all day pairs with i < j"));
    assert!(prompt.contains("Interviewer: That's right"));

    for mode in [InterviewMode::Coding, InterviewMode::Whiteboard] {
        let rules = report_system_instruction(mode)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for rule in [
            "The contract in the brief states the inputs, output and rules of the exercise; it is not what the candidate restated",
            "Credit a restatement only for the relevant inputs, output and rules the candidate stated in their own words",
            "A misstated output, or a relevant contract rule the restatement leaves out, is a gap, not a strength",
            "Repeat is not scored as an accurate restatement",
            "The interviewer's confirmation or rephrasing of a restatement earns it no credit",
            "whether they accurately restated the problem",
        ] {
            assert!(rules.contains(rule), "{mode:?} rules miss {rule}");
        }
    }
}
