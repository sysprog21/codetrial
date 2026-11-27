//! What the report rules say about credit the candidate did not earn.
//!
//! Kept apart from `tests/agent/prompts.rs`, which is at its size ceiling;
//! like it, this compiles as a module of the `tests/agent.rs` binary.

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

    // Preconditions: these confirm the fixture is the misstated resale
    // restatement, whose contract holds the rule it left out, and pass without
    // the grounding rule. The clause loop below is the regression check.
    assert!(prompt.contains("over all day pairs with i < j"));

    // The rule the restatement left out was posed on screen, not only in the
    // contract, so leaving it out still counts as a gap.
    assert!(prompt.contains("selling on a later day"));
    assert!(prompt.contains("Interviewer: That's right"));

    for mode in [InterviewMode::Coding, InterviewMode::Whiteboard] {
        let rules = report_system_instruction(mode)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        // A rule written on the board, or typed as an editor comment, is
        // restated, but not one the code itself implies. Neither the final code
        // nor the final board dates it, and the browser takes the Repeat
        // checkpoint only when Repeat is recorded, so a board rule counts only
        // when that image shows it, and a comment only when a `repeat` row from
        // `editor_snapshot` quotes it, the only repeat row written only before
        // they moved on; a speech row or a pause-time note does not date it.
        let judged = match mode {
            InterviewMode::Whiteboard => {
                "Judge a restatement as it stands when the candidate first moves on from it to any other step: by then it holds the relevant inputs, output and rules they stated in their own words, wrote on the board that the Repeat checkpoint image shows, only when it is the first checkpoint image (writing added after that image is taken, even before they move on, is shown in no image and earns no credit), corrected themselves, or settled with their own question, never what is inferred from the board itself. Something seen only on a later checkpoint or the final board does not show it was there before they moved on. A restatement first given"
            }
            _ => {
                "Judge a restatement as it stands when the candidate first moves on from it to any other step: by then it holds the relevant inputs, output and rules they stated in their own words, wrote as a comment in the code that a `repeat` phase row from `editor_snapshot` in the rolling assessment quotes after `Quote:`, never what its summary says (a row from `candidate_speech` with the same words may be speech after they moved on, and a pause-time note does not date a comment; a comment under six words is not recorded on its own and counts only when such a row quotes it with the comment lines around it), corrected themselves, or settled with their own question, never what is inferred from the code itself. A comment present only in the final code does not show it was there before they moved on; for any other step it still counts as clarification, as the rule on typed editor comments above says. A restatement first given"
            }
        };
        assert!(rules.contains(judged), "{mode:?} rules miss {judged}");
        assert_eq!(
            rules.contains("phase row from `editor_snapshot` in the rolling assessment quotes"),
            mode == InterviewMode::Coding,
            "only the editor rules date a comment by a phase row"
        );
        assert_eq!(
            rules.contains("the Repeat checkpoint image shows"),
            mode == InterviewMode::Whiteboard,
            "only the board rules date a rule by the Repeat checkpoint"
        );
        for rule in [
            "The contract in the brief is the private specification the interviewer never reads out; it is not what the candidate restated, and it holds rules the candidate was never shown",
            "A restatement first given after they moved on to another step earns no credit for what that step showed them.",
            "A gap is a misstated input, output or rule, a left-out input or output, or a relevant rule the scenario posed to the candidate on screen that the restatement still lacks or misstates at that point, rather than lost to recognition; a rule only the contract or the constraints hold is not.",
            "A gap is not a strength, and it puts Repeat below the 90–100 band by how much the restatement missed",
            "one left-out rule with the rest right is a minor gap (75–89), and a left-out input or output, or more than one left-out rule, is a material gap (60–74)",
            "A restatement that only leaves parts out is still a correct partial one and completes the step, so it does not reach 0–39; a misstated input, output or rule is placed by the rubric's anchors",
            "The interviewer's confirmation or rephrasing of a restatement earns it no credit",
            "whether they accurately restated the problem",
        ] {
            assert!(rules.contains(rule), "{mode:?} rules miss {rule}");
        }
    }
}

/// A repeat row can be speech recorded after the candidate moved on: with
/// another step recorded, a restatement said aloud is still recorded, and its
/// words can be those of a comment typed while fixing the code. Only a row
/// from `editor_snapshot` is written while no other step is recorded, so the
/// editor rule, pinned above, names that source, and the row the report reads
/// says which one it is.
#[test]
fn a_late_spoken_restatement_does_not_date_the_same_comment() {
    let rule = "the sell day must come after the buy day";
    let mut state = RuntimeState {
        transcript: vec![format!("Candidate: {rule}")],
        code_templates: [("python".to_string(), String::new())].into(),
        code: format!("# {rule}\n"),
        ..RuntimeState::default()
    };
    let example = json!({"phase": "example", "source": "candidate_speech",
        "kind": "observed", "confidence": 90, "summary": "Worked a case."});
    record_framework_evidence(&mut state, &example).unwrap();
    let steps = json!([{"step": "repeat", "quote": rule, "summary": "Said and typed."}]);
    let late = super::framework::judge(&mut state, steps);
    assert_eq!(late.len(), 1);

    let rows = rolling_assessment(&state.framework_evidence, &[]);
    assert!(
        rows.contains(&format!("- repeat (observed, candidate_speech, confidence 70): Said and typed. Quote: \"{rule}\"")),
        "{rows}"
    );
}
