//! The report Gemini is asked for, and whether what came back is one.
//!
//! Two halves that belong together: `report_response_schema` is the shape the
//! model is told to produce, and everything below it decides whether a response
//! actually is that shape. They are one concern because a schema nobody
//! validates against is a suggestion, and a validator that has drifted from the
//! schema refuses valid reports.
//!
//! The strictness is deliberate. A report reaches a candidate, so a field that
//! quietly defaults is a verdict nobody wrote.

use super::{InterviewMode, Problem, RUBRIC_VERSION};

/// Lowercase ASCII words, split on anything that is not a letter or a digit
/// and inside identifiers where their case changes, the way `spelled_words` in
/// scripts/problem_bank/rules.py splits them: `minStackCreate` is min, stack,
/// create, and `LRUCache` is lru, cache.
///
/// Public because the generator, the prompt tests and this validator have to
/// agree on one splitting rule. A second copy is how they stop agreeing.
pub fn spelled_words(text: &str) -> Vec<String> {
    let characters = text.chars().collect::<Vec<_>>();
    let mut words = Vec::new();
    let mut current = String::new();
    for (at, &character) in characters.iter().enumerate() {
        if !character.is_ascii_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        let previous = at.checked_sub(1).map(|before| characters[before]);
        let next = characters.get(at + 1);
        let lower_then_upper = character.is_ascii_uppercase()
            && previous
                .is_some_and(|before| before.is_ascii_lowercase() || before.is_ascii_digit());
        let acronym_ends = character.is_ascii_uppercase()
            && previous.is_some_and(|before| before.is_ascii_uppercase())
            && next.is_some_and(char::is_ascii_lowercase);
        if (lower_then_upper || acronym_ends) && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        current.push(character.to_ascii_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Whether text names a published problem rather than only its interview
/// scenario. Candidate-facing report fields use this to refuse published
/// titles, LeetCode, and Leet Code while allowing ordinary single-word titles
/// such as "Triangle" that a scenario may legitimately use.
pub fn names_published_problem(title: &str, text: &str) -> bool {
    let text_words = spelled_words(text);
    if text_words.iter().any(|word| word == "leetcode")
        || text_words.windows(2).any(|pair| pair == ["leet", "code"])
    {
        return true;
    }

    // One word, not merely one token. "Triangle" is a word a scenario may use
    // on its own account, so matching it would refuse honest prose. "LRUCache"
    // and "MinStack" have no space either, and the whole point of this check is
    // that a report must not spell them, so the split decides rather than the
    // absence of punctuation.
    let title_words = spelled_words(title);
    if title_words.len() == 1
        && title
            .chars()
            .all(|character| character.is_ascii_alphabetic())
    {
        return false;
    }
    let target = title_words.concat();
    (0..text_words.len()).any(|start| {
        let mut joined = String::new();
        for word in &text_words[start..] {
            joined.push_str(word);
            if joined == target {
                return true;
            }
            if joined.len() >= target.len() {
                return false;
            }
        }
        false
    })
}

/// An evaluation that could not be produced is not an evaluation of zero.
///
/// This used to emit `codingScore: 0`, `communicationScore: 0` and
/// `decision: "NO_HIRE"`, so a Gemini outage reached the candidate as a
/// rejection with 0/100 on both axes, and was then saved to their history and
/// synced to `/api/reports`. The `error: true` flag it set alongside was
/// dropped by the browser's own sanitizer, so nothing downstream could tell the
/// difference between a failed provider and a failed candidate. `incomplete`
/// is the shape the browser already renders as "No evaluation", with no
/// verdict, no scores and no badge, and it is the only flag: an `error: true`
/// that travelled beside it reached no consumer and was a second name for the
/// same fact.
pub fn fallback_report(hints_used: u32, note: &str) -> serde_json::Value {
    // Long enough for the reason to survive alongside the sentence that frames
    // it. At 240 the boilerplate about the runner and the editor filled the
    // budget and the actual cause was cut off mid-word, so a lost report said
    // only that it was lost.
    //
    // The advice is the one thing true of every cause. It used to send the
    // candidate to the agent logs and GOOGLE_API_KEY, which a candidate cannot
    // read and which a 503, the usual cause, has nothing to do with; the note
    // already names a rejected credential when that is what happened.
    let note = note.chars().take(600).collect::<String>();
    let note = note.trim_end().trim_end_matches('.');
    serde_json::json!({
        "incomplete": true,
        "summary": format!(
            "The automatic evaluation could not be completed: {note}. Your session ran end-to-end, but no scores were produced, so nothing here is an assessment of your work. Try the interview again later; if this keeps happening, whoever runs this server can find the cause in the agent logs."
        ),
        "improvementPlan": [],
        "frameworkAssessment": null,
        "hintsUsed": hints_used,
    })
}

pub fn report_response_schema() -> serde_json::Value {
    // `responseSchema` accepts only Gemini's OpenAPI subset, which has no
    // `additionalProperties`: sending it fails the whole call with a 400 naming
    // an unknown field, so every report came back as the incomplete fallback.
    // Object closure is enforced on the response instead, by the `unknown
    // field` arm of `validate_report_candidate`. Array and numeric bounds are
    // in the subset and are still sent.
    let text = || serde_json::json!({ "type": "STRING" });
    let strings = |min: u32, max: u32| {
        serde_json::json!({
            "type": "ARRAY", "minItems": min, "maxItems": max,
            "items": { "type": "STRING" }
        })
    };
    let feedback = serde_json::json!({
        "type": "OBJECT",
        "propertyOrdering": ["strengths", "improvements"],
        "properties": { "strengths": strings(2, 4), "improvements": strings(2, 4) },
        "required": ["strengths", "improvements"]
    });
    let plan = serde_json::json!({
        "type": "OBJECT",
        "propertyOrdering": ["phase", "weakness", "impact", "frequency", "drill", "durationMin", "successCriterion", "selfReview"],
        "properties": {
            "phase": { "type": "STRING", "enum": IMPROVEMENT_PHASES },
            "weakness": text(), "impact": { "type": "STRING", "enum": ["high", "medium", "low"] },
            "frequency": { "type": "INTEGER", "minimum": 1, "maximum": 99 },
            "drill": text(), "durationMin": { "type": "INTEGER", "minimum": 1, "maximum": 30 },
            "successCriterion": text(), "selfReview": strings(1, 4)
        },
        "required": ["phase", "weakness", "impact", "frequency", "drill", "durationMin", "successCriterion", "selfReview"]
    });

    // No `weaknessTags`: `apply_weakness_tags` derives the row from the plan
    // and overwrote whatever the model wrote, so asking for it bought output
    // tokens, each a repeated improvement, that nothing ever read.
    let assessment_row = serde_json::json!({
        "type": "OBJECT", "propertyOrdering": ["phase", "score"],
        "properties": {
            "phase": { "type": "STRING", "enum": IMPROVEMENT_PHASES },
            "score": { "type": "INTEGER", "minimum": 0, "maximum": 100, "nullable": true }
        },
        "required": ["phase", "score"]
    });
    serde_json::json!({
        "type": "OBJECT",
        "propertyOrdering": ["codingScore", "communicationScore", "decision", "summary", "codingFeedback", "communicationFeedback", "improvementPlan", "frameworkAssessment"],
        "properties": {
            "codingScore": { "type": "INTEGER", "minimum": 0, "maximum": 100 },
            "communicationScore": { "type": "INTEGER", "minimum": 0, "maximum": 100 },
            "decision": { "type": "STRING", "enum": ["HIRE", "NO_HIRE"] },
            "summary": text(), "codingFeedback": feedback.clone(), "communicationFeedback": feedback,
            "improvementPlan": { "type": "ARRAY", "minItems": 0, "maxItems": 8, "items": plan },
            "frameworkAssessment": {
                "type": "OBJECT", "propertyOrdering": ["rubricVersion", "phases"],
                "properties": {
                    // No `enum` here. Gemini's subset types `Schema.enum` as
                    // repeated string, so an integer entry fails the whole
                    // request with "Invalid value ... (TYPE_STRING)". The exact
                    // value is pinned on the response by `strict_integer`.
                    "rubricVersion": {
                        "type": "INTEGER",
                        "minimum": RUBRIC_VERSION,
                        "maximum": RUBRIC_VERSION
                    },
                    "phases": { "type": "ARRAY", "minItems": 10, "maxItems": 10, "items": assessment_row }
                },
                "required": ["rubricVersion", "phases"]
            }
        },
        "required": ["codingScore", "communicationScore", "decision", "summary", "codingFeedback", "communicationFeedback", "improvementPlan", "frameworkAssessment"]
    })
}

pub fn validate_report(
    raw: &serde_json::Value,
    hints_used: u32,
    problem: &Problem,
) -> Result<serde_json::Value, Vec<String>> {
    let mut report = validate_report_candidate(raw, problem)?;
    report
        .as_object_mut()
        .expect("validated object")
        .insert("hintsUsed".to_string(), serde_json::json!(hints_used));
    Ok(report)
}

/// Whether a model response is a report, and the report if it is.
///
/// What comes back is not `raw`: the fields the server owns are applied to the
/// accepted copy, so the plan is in its final order and the phase rows carry
/// the tags that follow from it. A caller that keeps `raw` instead keeps a
/// report the model happened to order, which is not the one the candidate is
/// shown. `validate_report` adds the last of those fields, `hintsUsed`, which
/// is counted here rather than claimed by the model.
pub fn validate_report_candidate(
    raw: &serde_json::Value,
    problem: &Problem,
) -> Result<serde_json::Value, Vec<String>> {
    let snapped = snap_plan_weaknesses(raw);
    let raw = &snapped;
    let mut errors = Vec::new();
    let Some(object) = raw.as_object() else {
        return Err(vec!["$: expected object".to_string()]);
    };
    exact_keys(
        object,
        &[
            "codingScore",
            "communicationScore",
            "decision",
            "summary",
            "codingFeedback",
            "communicationFeedback",
            "improvementPlan",
            "frameworkAssessment",
        ],
        "$",
        &mut errors,
    );
    strict_integer(
        object.get("codingScore"),
        0,
        100,
        "$.codingScore",
        &mut errors,
    );
    strict_integer(
        object.get("communicationScore"),
        0,
        100,
        "$.communicationScore",
        &mut errors,
    );
    strict_enum(
        object.get("decision"),
        &["HIRE", "NO_HIRE"],
        "$.decision",
        &mut errors,
    );
    strict_text(
        object.get("summary"),
        MAX_SUMMARY_TEXT,
        "$.summary",
        &mut errors,
    );
    for key in ["codingFeedback", "communicationFeedback"] {
        validate_feedback(object.get(key), &format!("$.{key}"), &mut errors);
    }
    let improvements = object
        .get("codingFeedback")
        .and_then(|v| v.get("improvements"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            object
                .get("communicationFeedback")
                .and_then(|v| v.get("improvements"))
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten(),
        )
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .collect::<Vec<_>>();
    validate_improvement_plan(object.get("improvementPlan"), &improvements, &mut errors);
    validate_framework_assessment(object.get("frameworkAssessment"), &mut errors);

    // An original problem has no published title to leak, and an empty one
    // matches nothing, so the same walk covers both kinds.
    let source_title = problem.source_title().unwrap_or("");
    for key in [
        "summary",
        "codingFeedback",
        "communicationFeedback",
        "improvementPlan",
    ] {
        if let Some(value) = object.get(key) {
            validate_observable_judgments(value, &format!("$.{key}"), &mut errors);
            validate_published_problem_names(value, &format!("$.{key}"), source_title, &mut errors);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut report = raw.clone();
    sort_improvement_plan(&mut report);
    apply_weakness_tags(&mut report);
    Ok(report)
}

/// `validate_report_candidate`, which refuses picture-only credit in coding
/// reports and any STAR plan item when
/// the platform never opened the behavioral round, and clears the STAR scores
/// of the report it accepts.
///
/// The interviewer can ask a behavioral question without the platform's
/// transition, so the transcript can hold an answer to a round the report
/// card shows as skipped or not configured. A plan item is refused rather
/// than cut out: it copies a feedback improvement, and removing both can
/// leave a list below the two it must hold, so the repair loop rewrites them
/// from the coding round instead. A score is settled here: a phase row holds
/// only its phase and score, so nulling it shrinks nothing, and null is the
/// only right answer for an unopened round. Refusing it would spend a repair,
/// and a model that kept scoring a strong out-of-turn answer would run out of
/// repairs and leave the candidate with no report at all.
pub fn validate_report_for_round(
    raw: &serde_json::Value,
    problem: &Problem,
    behavioral_round_opened: bool,
    mode: InterviewMode,
    drawing_received: bool,
) -> Result<serde_json::Value, Vec<String>> {
    let report = validate_report_candidate(raw, problem).and_then(|report| {
        let errors = example_drawing_judgments(&report, problem, mode, drawing_received);
        if errors.is_empty() {
            Ok(report)
        } else {
            Err(errors)
        }
    });
    if behavioral_round_opened {
        return report;
    }
    let mut unopened = unopened_round_errors(raw);
    match report {
        Ok(mut report) if unopened.is_empty() => {
            clear_star_scores(&mut report);
            Ok(report)
        }
        Ok(_) => Err(unopened),
        Err(mut errors) => {
            errors.append(&mut unopened);
            Err(errors)
        }
    }
}

// Coding assessment describes code and conversation. The optional drawing is
// shown separately, so references to it belong in neither score's feedback.
fn example_drawing_judgments(
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
                matches!(
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
                )
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

fn unopened_round_errors(raw: &serde_json::Value) -> Vec<String> {
    let items = raw
        .get("improvementPlan")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten();
    items
        .enumerate()
        .filter_map(|(index, item)| {
            let phase = item.get("phase").and_then(serde_json::Value::as_str)?;
            is_star(phase).then(|| {
                format!(
                    "$.improvementPlan[{index}].phase: the behavioral round never opened, so no improvement may address {phase}; replace this item and the feedback improvement it copies with one grounded in the coding round"
                )
            })
        })
        .collect()
}

fn clear_star_scores(report: &mut serde_json::Value) {
    let rows = report
        .get_mut("frameworkAssessment")
        .and_then(|assessment| assessment.get_mut("phases"))
        .and_then(serde_json::Value::as_array_mut)
        .into_iter()
        .flatten();
    for row in rows {
        if row
            .get("phase")
            .and_then(serde_json::Value::as_str)
            .is_some_and(is_star)
        {
            row["score"] = serde_json::Value::Null;
        }
    }
}

/// What a self-review list emptied by the filter is given instead, so the
/// plan item still has the one check the schema requires.
pub(crate) const SELF_REVIEW_REPLACEMENT: &str = "Review this step against the interview evidence";

/// What a success criterion the filter refuses is given instead. The schema
/// requires one, and the rest of the plan item, which passed the same filter,
/// is still the model's.
pub(crate) const SUCCESS_CRITERION_REPLACEMENT: &str =
    "Complete the drill again without the weakness above appearing";

/// What `sanitize_report_candidate` took out of a report.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sanitized {
    /// Self-review checks dropped.
    pub checks: usize,
    /// Success criteria replaced.
    pub criteria: usize,
}

impl Sanitized {
    pub(crate) fn is_empty(self) -> bool {
        self == Self::default()
    }
}

/// Remove the self-review checks that judge delivery or personality, replace
/// a success criterion that does, and say how many of each.
///
/// A self-review check is optional coaching text, unlike a score, decision, or
/// feedback improvement, and a success criterion only says when its drill is
/// done. Keeping the rest of a report when one of them judges delivery or
/// personality is more useful than turning an otherwise complete interview
/// into an incomplete one: a criterion asking for eye contact, refused through
/// both repairs, cost a candidate every score. This is the provider's fallback
/// for when its repairs run out or its calls stop answering, not part of
/// validation: while a repair can still be asked for, the model rewriting the
/// text gives the candidate something specific, and `validate_report` stays
/// strict so no other caller learns to lean on it.
///
/// Only a string that trips the judgment filter is touched. A malformed entry
/// or an empty list is left for validation to refuse, because nothing unsafe
/// was taken out of it. A list over the length limit can come back within it,
/// and is then accepted: what is left is the model's own safe checks. A list
/// this emptied is refilled, and a refused criterion replaced, because the
/// schema requires one, not because the line is worth reading; both are fixed
/// text rather than model-authored evidence, so neither can smuggle the same
/// judgment back.
pub(crate) fn sanitize_report_candidate(
    mut report: serde_json::Value,
) -> (serde_json::Value, Sanitized) {
    let Some(items) = report
        .get_mut("improvementPlan")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return (report, Sanitized::default());
    };
    let refused = |text: &serde_json::Value| {
        text.as_str()
            .is_some_and(|text| !refused_judgments(text, true).is_empty())
    };
    let mut sanitized = Sanitized::default();
    for item in items {
        if let Some(criterion) = item.get_mut("successCriterion")
            && refused(criterion)
        {
            *criterion = serde_json::json!(SUCCESS_CRITERION_REPLACEMENT);
            sanitized.criteria += 1;
        }
        let Some(checks) = item
            .get_mut("selfReview")
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        let before = checks.len();
        checks.retain(|check| !refused(check));
        let removed = before - checks.len();
        sanitized.checks += removed;
        if removed > 0 && checks.is_empty() {
            checks.push(serde_json::json!(SELF_REVIEW_REPLACEMENT));
        }
    }
    (report, sanitized)
}

/// Reject published-problem names from candidate-facing `summary`,
/// `codingFeedback`, `communicationFeedback`, and `improvementPlan` fields.
///
/// The validator receives the complete fields, rather than a copied list of
/// strings, so a newly nested strength, improvement, drill, or self-review is
/// checked before it can reach history, Markdown, or replay.
fn validate_published_problem_names(
    value: &serde_json::Value,
    path: &str,
    title: &str,
    errors: &mut Vec<String>,
) {
    visit_strings(value, path, &mut |path, text| {
        if names_published_problem(title, text) {
            errors.push(format!("{path}: names the published problem"));
        }
    });
}

/// Call `visit` with every string in a report field and the path that names it.
///
/// One recursion rather than one per rule. The two validators here are driven
/// from the same four fields over the same tree, so a fifth candidate-facing
/// field, or any change to how a path is spelled, used to be two edits that had
/// to agree: the paths reach the candidate through the repair prompt, so a
/// disagreement is not caught by either validator failing.
fn visit_strings(value: &serde_json::Value, path: &str, visit: &mut impl FnMut(&str, &str)) {
    match value {
        serde_json::Value::String(text) => visit(path, text),
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                visit_strings(item, &format!("{path}[{index}]"), visit);
            }
        }
        serde_json::Value::Object(object) => {
            for (key, item) in object {
                visit_strings(item, &format!("{path}.{key}"), visit);
            }
        }
        _ => {}
    }
}

/// Which weaknesses tag a phase is not a judgment: the rule was always
/// "the weakness of every plan item whose phase is this one", which is a
/// `filter` the model was being asked to run by hand across ten rows. It got it
/// wrong often enough to lose reports over, so the rows are filled here from
/// the plan they had to agree with. Runs after `sort_improvement_plan` and not
/// before: a phase can hold more items than a row has tags, and what the cap
/// drops has to be the cheapest of them rather than whichever the model wrote
/// last.
///
/// The counterpart copy, `improvementPlan[].weakness` against the feedback
/// improvements, stays the model's to write; see `snap_plan_weaknesses`.
fn apply_weakness_tags(report: &mut serde_json::Value) {
    let tags = |phase: &str| {
        report
            .get("improvementPlan")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| item.get("phase").and_then(serde_json::Value::as_str) == Some(phase))
            .filter_map(|item| item.get("weakness").cloned())
            .take(MAX_WEAKNESS_TAGS)
            .collect::<Vec<_>>()
    };
    let derived = IMPROVEMENT_PHASES.map(tags);
    let Some(rows) = report
        .get_mut("frameworkAssessment")
        .and_then(|assessment| assessment.get_mut("phases"))
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for (row, tags) in rows.iter_mut().zip(derived) {
        if let Some(row) = row.as_object_mut() {
            row.insert("weaknessTags".to_string(), serde_json::Value::Array(tags));
        }
    }
}

/// Each plan weakness that differs from exactly one feedback improvement only
/// in case, spacing or a closing full stop, rewritten to that improvement.
///
/// The plan item is a mapping the model makes, one per improvement, and a
/// copy that changed nothing but the capitals or a trailing period was a
/// rejected report and a whole repair call. Referencing improvements by index
/// instead would spare the copy, but a wrong index is a plan item silently
/// attached to someone else's weakness, where a wrong copy is refused and
/// repaired; so the copy stays, and only what cannot change its meaning is
/// forgiven. A match against two improvements, or a paraphrase, is left for
/// validation to refuse.
fn snap_plan_weaknesses(raw: &serde_json::Value) -> serde_json::Value {
    let normal = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim_end_matches('.')
            .to_lowercase()
    };
    let improvements = ["codingFeedback", "communicationFeedback"]
        .iter()
        .filter_map(|key| raw.get(*key)?.get("improvements")?.as_array())
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .collect::<Vec<_>>();
    let mut snapped = raw.clone();
    let Some(items) = snapped
        .get_mut("improvementPlan")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return snapped;
    };
    for item in items {
        let Some(weakness) = item.get("weakness").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if improvements.contains(&weakness.trim()) {
            continue;
        }
        let matches = improvements
            .iter()
            .filter(|improvement| normal(improvement) == normal(weakness))
            .collect::<Vec<_>>();
        if let [only] = matches.as_slice() {
            item["weakness"] = serde_json::Value::String(only.to_string());
        }
    }
    snapped
}

/// Plan order is presentation, not judgment: the same items in the wrong
/// sequence are the same assessment. The model got this wrong often enough that
/// whole reports were rejected over it and the candidate saw "no evaluation",
/// so the order is applied here instead of demanded from the model. Stable, so
/// items of equal impact and frequency keep the order they were written in.
fn sort_improvement_plan(report: &mut serde_json::Value) {
    let Some(items) = report
        .get_mut("improvementPlan")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    items.sort_by_key(|item| {
        let rank = match item.get("impact").and_then(serde_json::Value::as_str) {
            Some("high") => 3,
            Some("medium") => 2,
            _ => 1,
        };
        let frequency = item
            .get("frequency")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        std::cmp::Reverse((rank, frequency))
    });
}

fn validate_observable_judgments(value: &serde_json::Value, path: &str, errors: &mut Vec<String>) {
    visit_strings(value, path, &mut |path, text| {
        let improvement = IMPROVEMENT_PATHS
            .iter()
            .any(|prefix| path.starts_with(prefix));
        for (reason, phrases) in refused_judgments(text, improvement) {
            errors.push(judgment_error(path, reason, &phrases));
        }
    });
}

/// The longest a validation error gets, both in the repair prompt and in the
/// note a candidate reads when no repair succeeded.
pub(crate) const MAX_ERROR_CHARS: usize = 240;

/// Names as many phrases as fit in `MAX_ERROR_CHARS` and counts the rest. Cut
/// at the bound instead, a field with many phrases loses its last one midway,
/// and the repair never learns to remove it.
fn judgment_error(path: &str, reason: &str, phrases: &[&str]) -> String {
    let quoted = phrases
        .iter()
        .map(|phrase| format!("{phrase:?}"))
        .collect::<Vec<_>>();
    let naming = |named: usize| {
        let list = quoted[..named].join(", ");
        match quoted.len() - named {
            0 => format!("{path}: {reason} ({list})"),
            more => format!("{path}: {reason} ({list}, and {more} more)"),
        }
    };

    // One phrase always, even past the bound: a path that long is cut by the
    // repair prompt anyway, and an error naming nothing gives a repair less.
    (2..=quoted.len())
        .rev()
        .map(naming)
        .find(|error| error.chars().count() <= MAX_ERROR_CHARS)
        .unwrap_or_else(|| naming(1))
}

/// Where a report tells the candidate what to fix: the two improvement lists
/// and the plan copied from them, self-review checks included.
const IMPROVEMENT_PATHS: [&str; 3] = [
    "$.codingFeedback.improvements",
    "$.communicationFeedback.improvements",
    "$.improvementPlan",
];

/// A class of claim a report may not make about a person, the reason the
/// repair is told, and the phrases that make it. Phrases are named in the
/// error because the lists are ours and the model cannot see them: told only
/// that a check is unsupported, a repair swaps "appeared nervous" for "sounded
/// confident" and fails again. Each phrase is whole words, spelled with the
/// spaces `refused_judgments` pads the text with.
struct JudgmentRule {
    reason: &'static str,
    /// Only where the report says what to fix, rather than everywhere.
    improvements_only: bool,
    phrases: &'static [&'static str],
    /// Words that break the rule only when a speech word follows, directly or
    /// through "language", as "a Japanese response" or "a Spanish-language
    /// answer" does.
    before_speech: &'static [&'static str],
}

/// What a report calls a candidate's spoken turn. Not "text" or "characters",
/// which a text exercise's input has too.
const SPEECH_WORDS: &[&str] = &[
    "response",
    "responses",
    "answer",
    "answers",
    "reply",
    "replies",
    "remark",
    "remarks",
    "statement",
    "statements",
    "explanation",
    "explanations",
    "utterance",
    "utterances",
    "speech",
    "transcript",
    "transcripts",
    "transcription",
    "turn",
    "turns",
];

const JUDGMENT_RULES: [JudgmentRule; 3] = [
    JudgmentRule {
        reason: "unsupported delivery or personality judgment",
        improvements_only: false,
        phrases: &[
            " accent ",
            " accents ",
            " dialect ",
            " dialects ",
            " typing speed ",
            " typing pace ",
            " typed slowly ",
            " typed quickly ",
            " type slowly ",
            " type quickly ",
            " speech rate ",
            " filler word ",
            " filler words ",
            " disfluency ",
            " disfluencies ",
            // A disfluency by another name: an accepted success criterion asked
            // for an answer "without hesitation". Not the verb, since "do not
            // hesitate to ask a clarifying question" is coaching about the
            // conversation rather than a judgment of how it sounded.
            " hesitation ",
            " hesitations ",
            " hesitant ",
            " hesitantly ",
            " eye contact ",
            " posture ",
            " body language ",
            " facial expression ",
            " facial expressions ",
            " voice tone ",
            " vocal tone ",
            " tone of voice ",
            " attractiveness ",
            " physical appearance ",
            " nervous ",
            " nervousness ",
            " nervously ",
            " anxious ",
            " anxiety ",
            " confident demeanor ",
            " lacked confidence ",
            " lack of confidence ",
            " personality ",
            " introvert ",
            " extrovert ",
            " charisma ",
            " appeared confident ",
            " appears confident ",
            " seemed confident ",
            " looked confident ",
            " sounded confident ",
            " come across as confident ",
            " comes across as confident ",
        ],
        before_speech: &[],
    },
    // The language a transcript came out in is the recognizer's output, not the
    // candidate's: a report that said "an irrelevant response in Mandarin"
    // failed an English speaker for it. Named languages only with "in", except
    // the two that name nothing but speech, and none of "another language" or
    // "switched languages", because a candidate changing C++ for Python is a
    // coding event. The cost is a technical sentence such as "input in Chinese
    // characters" on a text exercise, refused and repaired; matching only after
    // a speech word would miss "answered the question in Japanese". The
    // adjective, "a Japanese response", is refused only before a speech word,
    // where it names the transcript's language rather than an input's.
    JudgmentRule {
        reason: "the recognizer's language attributed to the candidate",
        improvements_only: false,
        phrases: &[
            " in chinese ",
            " in japanese ",
            " in korean ",
            " in hindi ",
            " in spanish ",
            " in french ",
            " in german ",
            " in portuguese ",
            " mandarin ",
            " cantonese ",
            " non english ",
            " foreign language ",
            " native language ",
            " broken english ",
            " english proficiency ",
            " english fluency ",
            " fluent english ",
            " language barrier ",
        ],
        before_speech: &[
            "chinese",
            "japanese",
            "korean",
            "hindi",
            "spanish",
            "french",
            "german",
            "portuguese",
        ],
    },
    // Improvements only: the summary may say evidence was limited by
    // transcription. After one misrecognized turn a report asked for
    // explanations "in English to avoid transcription ambiguity", which is what
    // this refuses. A neutral mention of transcription is left alone: a report
    // that suggested typing a key point as a code comment "when a transcription
    // gap occurs" was refused through both repairs, and an incomplete report
    // costs the candidate their verdict. Not "english" alone either: "describe
    // the approach in plain English" is ordinary advice, and the live prompt
    // asks the interviewer for it. "Avoid" and "prevent" are what turn a
    // mention into blame: a seeded report asked for answers "clearly
    // articulated to avoid transcription ambiguity".
    JudgmentRule {
        reason: "a speech-recognition gap is not the candidate's weakness",
        improvements_only: true,
        phrases: &[
            " avoid transcription ",
            " prevent transcription ",
            " in english ",
            " speak english ",
            " your english ",
            " spoken english ",
            " primary language ",
            " interview language ",
            " language of the interview ",
            " audible ",
            " audibly ",
            " speak more clearly ",
            " speak clearly ",
            " enunciate ",
            " enunciation ",
            " pronounce ",
            " pronunciation ",
        ],
        before_speech: &[],
    },
];

/// Every rule `text` breaks, each with every phrase it breaks it with, so a
/// repair need not discover another violation in the same field on its next
/// attempt. Listed phrases come in list order, then adjectives before a speech
/// word in the order the text uses them. `improvement` says whether the field
/// tells the candidate what to fix.
fn refused_judgments(text: &str, improvement: bool) -> Vec<(&'static str, Vec<&'static str>)> {
    let words = spelled_words(text);
    let padded = format!(" {} ", words.join(" "));
    let mut matches = Vec::new();
    for rule in JUDGMENT_RULES
        .iter()
        .filter(|rule| improvement || !rule.improvements_only)
    {
        let mut phrases = rule
            .phrases
            .iter()
            .filter(|phrase| padded.contains(**phrase))
            .map(|phrase| phrase.trim())
            .collect::<Vec<_>>();
        for (at, word) in words.iter().enumerate() {
            let Some(adjective) = rule.before_speech.iter().find(|name| **name == word) else {
                continue;
            };
            let next = match words.get(at + 1).map(String::as_str) {
                Some("language") => words.get(at + 2),
                _ => words.get(at + 1),
            };
            if next.is_some_and(|next| SPEECH_WORDS.contains(&next.as_str()))
                && !phrases.contains(adjective)
            {
                phrases.push(*adjective);
            }
        }
        if !phrases.is_empty() {
            matches.push((rule.reason, phrases));
        }
    }
    matches
}

fn exact_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    expected: &[&str],
    path: &str,
    errors: &mut Vec<String>,
) {
    for key in expected {
        if !object.contains_key(*key) {
            errors.push(format!("{path}.{key}: required"));
        }
    }
    for key in object.keys() {
        if !expected.contains(&key.as_str()) {
            errors.push(format!("{path}.{key}: unknown field"));
        }
    }
}

fn strict_integer(
    value: Option<&serde_json::Value>,
    min: i64,
    max: i64,
    path: &str,
    errors: &mut Vec<String>,
) -> Option<i64> {
    match value
        .and_then(serde_json::Value::as_i64)
        .filter(|v| (min..=max).contains(v))
    {
        Some(value) => Some(value),
        None => {
            errors.push(format!("{path}: expected integer {min}..={max}"));
            None
        }
    }
}

fn strict_enum<'a>(
    value: Option<&'a serde_json::Value>,
    allowed: &[&str],
    path: &str,
    errors: &mut Vec<String>,
) -> Option<&'a str> {
    match value
        .and_then(serde_json::Value::as_str)
        .filter(|v| allowed.contains(v))
    {
        Some(value) => Some(value),
        None => {
            errors.push(format!("{path}: invalid enum"));
            None
        }
    }
}

fn strict_text(
    value: Option<&serde_json::Value>,
    max: usize,
    path: &str,
    errors: &mut Vec<String>,
) -> Option<String> {
    match value
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty() && value.chars().count() <= max)
    {
        Some(value) => Some(value.trim().to_string()),
        None => {
            errors.push(format!(
                "{path}: expected non-empty string of at most {max} characters"
            ));
            None
        }
    }
}

fn validate_string_array(
    value: Option<&serde_json::Value>,
    min: usize,
    max: usize,
    text_max: usize,
    path: &str,
    errors: &mut Vec<String>,
) {
    let Some(items) = value.and_then(serde_json::Value::as_array) else {
        errors.push(format!("{path}: expected array"));
        return;
    };
    if !(min..=max).contains(&items.len()) {
        errors.push(format!("{path}: expected {min}..={max} items"));
    }
    let mut seen = std::collections::HashSet::new();
    for (index, item) in items.iter().take(max.saturating_add(1)).enumerate() {
        if let Some(text) = strict_text(Some(item), text_max, &format!("{path}[{index}]"), errors)
            && !seen.insert(text)
        {
            errors.push(format!("{path}[{index}]: duplicate"));
        }
    }
}

fn validate_feedback(value: Option<&serde_json::Value>, path: &str, errors: &mut Vec<String>) {
    let Some(object) = value.and_then(serde_json::Value::as_object) else {
        errors.push(format!("{path}: expected object"));
        return;
    };
    exact_keys(object, &["strengths", "improvements"], path, errors);
    validate_string_array(
        object.get("strengths"),
        2,
        4,
        400,
        &format!("{path}.strengths"),
        errors,
    );
    validate_string_array(
        object.get("improvements"),
        2,
        4,
        400,
        &format!("{path}.improvements"),
        errors,
    );
}

fn validate_improvement_plan(
    value: Option<&serde_json::Value>,
    weaknesses: &[&str],
    errors: &mut Vec<String>,
) {
    let Some(items) = value.and_then(serde_json::Value::as_array) else {
        errors.push("$.improvementPlan: expected array".to_string());
        return;
    };
    if items.len() > 8 {
        errors.push("$.improvementPlan: expected at most 8 items".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for (index, item) in items.iter().take(9).enumerate() {
        let path = format!("$.improvementPlan[{index}]");
        let Some(object) = item.as_object() else {
            errors.push(format!("{path}: expected object"));
            continue;
        };
        exact_keys(
            object,
            &[
                "phase",
                "weakness",
                "impact",
                "frequency",
                "drill",
                "durationMin",
                "successCriterion",
                "selfReview",
            ],
            &path,
            errors,
        );
        strict_enum(
            object.get("phase"),
            &IMPROVEMENT_PHASES,
            &format!("{path}.phase"),
            errors,
        );
        let weakness = strict_text(
            object.get("weakness"),
            400,
            &format!("{path}.weakness"),
            errors,
        );
        if let Some(weakness) = weakness {
            if !weaknesses.contains(&weakness.as_str()) {
                errors.push(format!(
                    "{path}.weakness: must exactly reference a feedback improvement"
                ));
            }
            if !seen.insert(weakness) {
                errors.push(format!("{path}.weakness: duplicate"));
            }
        }
        strict_enum(
            object.get("impact"),
            &["high", "medium", "low"],
            &format!("{path}.impact"),
            errors,
        );
        strict_integer(
            object.get("frequency"),
            1,
            99,
            &format!("{path}.frequency"),
            errors,
        );
        strict_text(object.get("drill"), 400, &format!("{path}.drill"), errors);
        strict_integer(
            object.get("durationMin"),
            1,
            30,
            &format!("{path}.durationMin"),
            errors,
        );
        strict_text(
            object.get("successCriterion"),
            400,
            &format!("{path}.successCriterion"),
            errors,
        );
        validate_string_array(
            object.get("selfReview"),
            1,
            4,
            240,
            &format!("{path}.selfReview"),
            errors,
        );
    }
    let expected = weaknesses
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let actual = seen
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    if actual != expected {
        errors.push(
            "$.improvementPlan: must contain exactly one item for every feedback improvement"
                .to_string(),
        );
    }
}

fn validate_framework_assessment(value: Option<&serde_json::Value>, errors: &mut Vec<String>) {
    let Some(object) = value.and_then(serde_json::Value::as_object) else {
        errors.push("$.frameworkAssessment: expected object".to_string());
        return;
    };
    exact_keys(
        object,
        &["rubricVersion", "phases"],
        "$.frameworkAssessment",
        errors,
    );
    strict_integer(
        object.get("rubricVersion"),
        i64::from(RUBRIC_VERSION),
        i64::from(RUBRIC_VERSION),
        "$.frameworkAssessment.rubricVersion",
        errors,
    );
    let Some(rows) = object.get("phases").and_then(serde_json::Value::as_array) else {
        errors.push("$.frameworkAssessment.phases: expected array".to_string());
        return;
    };
    if rows.len() != IMPROVEMENT_PHASES.len() {
        errors.push("$.frameworkAssessment.phases: expected exactly 10 ordered phases".to_string());
    }
    for (index, expected_phase) in IMPROVEMENT_PHASES.iter().enumerate() {
        let path = format!("$.frameworkAssessment.phases[{index}]");
        let Some(row) = rows.get(index).and_then(serde_json::Value::as_object) else {
            errors.push(format!("{path}: expected object"));
            continue;
        };

        // `weaknessTags` is allowed and never required: the response schema no
        // longer asks for it, and a report this validated once carries the tags
        // `apply_weakness_tags` wrote, which is the shape `final_report` hands
        // back here. Its contents are not checked, since that function
        // overwrites them from the plan.
        let row = row
            .iter()
            .filter(|(key, _)| key.as_str() != "weaknessTags")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<serde_json::Map<_, _>>();
        exact_keys(&row, &["phase", "score"], &path, errors);
        if row.get("phase").and_then(serde_json::Value::as_str) != Some(*expected_phase) {
            errors.push(format!("{path}.phase: expected {expected_phase}"));
        }
        if row.get("score") != Some(&serde_json::Value::Null) {
            strict_integer(row.get("score"), 0, 100, &format!("{path}.score"), errors);
        }
    }
}

/// The longest summary the grader may write, and the number `web/lib.js` has to
/// bound the same field at. Named rather than typed into the validator call
/// because the browser's copy was 300 for the life of the field: every real
/// summary runs 400 characters and up, so every candidate read one that stopped
/// mid-sentence, and nothing in either tree pointed at the other. Held together
/// by `the_summary_bound_is_the_same_number_on_both_sides`.
pub const MAX_SUMMARY_TEXT: usize = 1200;

/// What one phase row holds, and what `strings(0, ...)` declares for it in the
/// response schema. One constant because the deriver fills the row and the
/// schema describes it, and a row longer than the schema admits is a report the
/// model is blamed for.
const MAX_WEAKNESS_TAGS: usize = 4;

fn is_star(phase: &str) -> bool {
    matches!(phase, "Situation" | "Task" | "Action" | "Result")
}

const IMPROVEMENT_PHASES: [&str; 10] = [
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

/// The report, or a stub saying why there is none.
///
/// Every fallback is logged, because the note it carries reaches the candidate
/// and nobody else. A schema the provider refused answered 400 on every
/// interview for the life of a branch while the suite stayed green, and the
/// only place that was visible was a sentence in the candidate's own report.
/// An operator reading the log sees it now, on the line that produced it, with
/// the validation errors rather than only the fact that there were some: which
/// field the model got wrong is the whole diagnostic, and the stub the
/// candidate receives cannot carry it.
pub fn final_report(
    raw_report: Option<&serde_json::Value>,
    hints_used: u32,
    error_note: Option<&str>,
    problem: &Problem,
) -> serde_json::Value {
    let (reason, errors) = match (raw_report, error_note) {
        (Some(raw_report), None) => match validate_report(raw_report, hints_used, problem) {
            Ok(report) => return report,
            Err(errors) => ("report schema validation failed", errors),
        },
        _ => (error_note.unwrap_or("report generation failed"), Vec::new()),
    };

    // Bounded, because a wholly wrong shape produces one per field and this is
    // a log line, not the report.
    let detail = errors
        .iter()
        .take(5)
        .map(|error| format!(" | {error}"))
        .collect::<String>();
    eprintln!("codetrial report_incomplete reason={reason}{detail}");
    fallback_report(hints_used, reason)
}
