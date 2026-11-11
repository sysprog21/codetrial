//! The rule that keeps an optional example drawing out of the coding and
//! communication assessment.

use super::{spelled_words, visit_strings};
use crate::agent::{InterviewMode, Problem};

// Coding assessment describes code and conversation. The optional drawing is
// shown separately, so references to it belong in neither score's feedback.
pub(super) fn example_drawing_judgments(
    raw: &serde_json::Value,
    problem: &Problem,
    mode: InterviewMode,
    drawing_received: bool,
) -> Vec<String> {
    if mode.is_whiteboard() || !drawing_received {
        return Vec::new();
    }
    let variant = problem.variant();
    let problem_words = spelled_words(&format!(
        "{} {} {} {} {} {}",
        problem.title,
        problem.summary,
        variant.title,
        variant.brief_text(),
        variant.contract,
        variant.constraints.join(" ")
    ));
    let mut errors = Vec::new();
    for key in [
        "summary",
        "codingFeedback",
        "communicationFeedback",
        "improvementPlan",
        "followUps",
    ] {
        let Some(value) = raw.get(key) else {
            continue;
        };
        visit_strings(value, &format!("$.{key}"), &mut |path, text| {
            let words = spelled_words(text);
            let visual_reference = words.iter().enumerate().any(|(index, word)| {
                if problem_words.contains(word) {
                    return false;
                }

                // References to exercise input are not references to an
                // optional drawing. Apply this exception to each mention, so an
                // input image cannot excuse a drawing in the same item.
                if matches!(
                    word.as_str(),
                    "picture" | "pictures" | "diagram" | "diagrams"
                ) && index
                    .checked_sub(1)
                    .and_then(|previous| words.get(previous))
                    .is_some_and(|previous| matches!(previous.as_str(), "input" | "source"))
                {
                    return false;
                }
                if !matches!(
                    word.as_str(),
                    "drawing"
                        | "drawings"
                        | "diagram"
                        | "diagrams"
                        | "sketch"
                        | "sketches"
                        | "picture"
                        | "pictures"
                        | "whiteboard"
                ) {
                    return false;
                }
                let before = &words[..index];
                let after = &words[index + 1..];
                if (word == "picture" && before.last().is_some_and(|word| word == "big"))
                    || (word == "sketch" && after.first().is_some_and(|word| word == "out"))
                    || (word == "drawing"
                        && after.first().is_some_and(|word| word == "on")
                        && after.get(1).is_some_and(|word| {
                            matches!(word.as_str(), "prior" | "previous" | "past" | "experience")
                        }))
                {
                    return false;
                }
                let owned = before.last().is_some_and(|word| word == "your")
                    || after.starts_with(&["you".to_string(), "drew".to_string()]);
                let produced = before.iter().rev().take(4).any(|word| {
                    matches!(
                        word.as_str(),
                        "provided" | "providing" | "uploaded" | "drew"
                    )
                });
                let recommendation = words.first().is_some_and(|word| word == "draw");
                owned || produced || recommendation
            });

            // An image may be the exercise's input, rather than the candidate's
            // optional drawing. Keep ordinary reasoning about that input.
            let normalized = format!(" {} ", words.join(" "));
            let candidate_image = ["your image", "provided an image", "uploaded an image"]
                .iter()
                .any(|phrase| normalized.contains(&format!(" {phrase} ")));
            if visual_reference || candidate_image {
                errors.push(format!("{path}: example drawings are outside coding and communication assessment; remove the visual reference and any credit or deduction based on it. Use only code, tests or the candidate's actual conversation under the original rubric, never the interviewer's interpretation of a picture"));
            }
        });
    }
    errors
}
