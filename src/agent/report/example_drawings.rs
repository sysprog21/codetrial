//! The rule that keeps an optional example drawing out of the coding and
//! communication assessment.

use super::{spelled_words, visit_strings};
use crate::agent::{InterviewMode, Problem};

/// What "your image" may modify without being the candidate's picture, on an
/// exercise that processes images.
const IMAGE_NOUNS: &[&str] = &[
    "rotation",
    "processing",
    "filter",
    "filters",
    "resize",
    "resizing",
    "scaling",
    "size",
    "sizes",
    "data",
    "buffer",
    "matrix",
    "pixels",
    "pipeline",
    "transform",
    "transformation",
    "convolution",
];

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

            // A sentence at a time, as the sanitizer that repairs a refusal
            // reads it: a word in one sentence cannot make a reference in the
            // next the candidate's.
            let visual_reference = text.split_inclusive(['.', '!', '?']).any(|sentence| {
                let words = spelled_words(sentence);
                words.iter().enumerate().any(|(index, word)| {
                    // References to exercise input are not references to an
                    // optional drawing. Apply this exception to each mention,
                    // so an input image cannot excuse a drawing in the same
                    // item.
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
                                matches!(
                                    word.as_str(),
                                    "prior" | "previous" | "past" | "experience"
                                )
                            }))
                    {
                        return false;
                    }

                    // The candidate's, said in any person: "your", "their",
                    // "you drew", "they drew", and "the example drawing", but
                    // not "for example, drawing": there the word opens an
                    // illustration and does not name a drawing.
                    let for_example = before.len() >= 2
                        && before[before.len() - 2] == "for"
                        && before[before.len() - 1] == "example";
                    let owned = before.last().is_some_and(|word| {
                        matches!(word.as_str(), "your" | "their" | "his" | "her")
                            || (word == "example" && !for_example)
                    }) || after
                        .first()
                        .is_some_and(|word| matches!(word.as_str(), "you" | "they" | "he" | "she"))
                        && after.get(1).is_some_and(|word| word == "drew");
                    let produced = before.iter().rev().take(4).any(|word| {
                        matches!(
                            word.as_str(),
                            "provided" | "providing" | "uploaded" | "drew"
                        )
                    });
                    let recommendation = words.first().is_some_and(|word| word == "draw");

                    // The exercise's own word is not refused on a loose cue,
                    // but "your" or "you drew" names the candidate's picture
                    // whatever the problem also calls its figures.
                    if problem_words.contains(word) {
                        return owned;
                    }
                    owned || produced || recommendation
                })
            });

            // An image may be the exercise's input, rather than the candidate's
            // optional drawing. Keep ordinary reasoning about that input.
            let normalized = format!(" {} ", words.join(" "));

            // "Your image" before an image-processing noun is feedback on the
            // exercise, not on the candidate's picture. A sentence at a time
            // too: a noun that opens the next sentence does not modify the word
            // before it.
            let your_image = text.split_inclusive(['.', '!', '?']).any(|sentence| {
                let words = spelled_words(sentence);
                words.windows(3).any(|at| {
                    at[0] == "your" && at[1] == "image" && !IMAGE_NOUNS.contains(&at[2].as_str())
                }) || words.ends_with(&["your".to_string(), "image".to_string()])
            });
            let candidate_image = your_image
                || ["provided an image", "uploaded an image"]
                    .iter()
                    .any(|phrase| normalized.contains(&format!(" {phrase} ")));
            if visual_reference || candidate_image {
                errors.push(format!("{path}: example drawings are outside coding and communication assessment; remove the visual reference and any credit or deduction based on it. Use only code, tests or the candidate's actual conversation under the original rubric, never the interviewer's interpretation of a picture"));
            }
        });
    }
    errors
}
