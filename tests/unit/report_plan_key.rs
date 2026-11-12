use serde_json::json;

use super::plan_key;
use crate::agent::{InterviewMode, get_problem, sanitize_example_drawing_candidate};

/// The key forgives case, spacing and a final period, including the blank a
/// stripped period leaves behind.
#[test]
fn a_plan_key_forgives_case_spacing_and_a_final_period() {
    assert_eq!(plan_key("Improve  X ."), "improve x");
    assert_eq!(plan_key("improve x"), "improve x");
    assert_eq!(plan_key("Improve X.."), "improve x");
    assert_ne!(plan_key("Improve X"), plan_key("Improve Y"));
}

/// Two improvements that differ only in case or a final period are one key, so
/// a weakness cannot say which it means. It is left for validation to refuse,
/// as the validator's own matching leaves it, and does not take whichever
/// replacement happened to be written last.
#[test]
fn a_weakness_two_replaced_improvements_could_name_is_left_unresolved() {
    let drawing = "Draw a diagram of the pointer moves before coding.";
    let report = json!({
        "summary": "You explained a map-based approach.",
        "codingFeedback": {
            "strengths": ["Correct core"],
            "improvements": [drawing, drawing.to_lowercase().trim_end_matches('.')]
        },
        "improvementPlan": [{"weakness": drawing}]
    });
    let (clean, _) = sanitize_example_drawing_candidate(
        report,
        get_problem(Some("two-sum")),
        InterviewMode::Coding,
        true,
    );
    let notes = clean["codingFeedback"]["improvements"].as_array().unwrap();
    let weakness = clean["improvementPlan"][0]["weakness"].as_str().unwrap();
    assert_ne!(notes[0], notes[1], "each item has its own omission note");
    assert!(
        notes.iter().all(|note| note != weakness),
        "{weakness:?} took one of {notes:?}"
    );
}
