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

use super::RUBRIC_VERSION;

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
    let note = note.chars().take(240).collect::<String>();
    serde_json::json!({
        "incomplete": true,
        "summary": format!(
            "The automatic evaluation could not be completed: {note}. Your session ran end-to-end, but no scores were produced, so nothing here is an assessment of your work. Check the agent logs and GOOGLE_API_KEY, then try again."
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
    let assessment_row = serde_json::json!({
        "type": "OBJECT", "propertyOrdering": ["phase", "score", "weaknessTags"],
        "properties": {
            "phase": { "type": "STRING", "enum": IMPROVEMENT_PHASES },
            "score": { "type": "INTEGER", "minimum": 0, "maximum": 100, "nullable": true },
            "weaknessTags": strings(0, 4)
        },
        "required": ["phase", "score", "weaknessTags"]
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
) -> Result<serde_json::Value, Vec<String>> {
    let mut report = validate_report_candidate(raw)?;
    report
        .as_object_mut()
        .expect("validated object")
        .insert("hintsUsed".to_string(), serde_json::json!(hints_used));
    Ok(report)
}

pub fn validate_report_candidate(
    raw: &serde_json::Value,
) -> Result<serde_json::Value, Vec<String>> {
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
    strict_text(object.get("summary"), 1200, "$.summary", &mut errors);
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
    validate_framework_assessment(
        object.get("frameworkAssessment"),
        object.get("improvementPlan"),
        &mut errors,
    );
    for key in [
        "summary",
        "codingFeedback",
        "communicationFeedback",
        "improvementPlan",
    ] {
        if let Some(value) = object.get(key) {
            validate_observable_judgments(value, &format!("$.{key}"), &mut errors);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(raw.clone())
}

fn validate_observable_judgments(value: &serde_json::Value, path: &str, errors: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => {
            let normalized = text
                .chars()
                .map(|character| {
                    if character.is_alphanumeric() {
                        character.to_ascii_lowercase()
                    } else {
                        ' '
                    }
                })
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let padded = format!(" {normalized} ");
            const UNSUPPORTED: &[&str] = &[
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
                " nervous demeanor ",
                " confident demeanor ",
                " personality ",
                " introvert ",
                " extrovert ",
                " charisma ",
                " appeared nervous ",
                " appears nervous ",
                " seemed nervous ",
                " looked nervous ",
                " sounded nervous ",
                " come across as nervous ",
                " comes across as nervous ",
                " appeared confident ",
                " appears confident ",
                " seemed confident ",
                " looked confident ",
                " sounded confident ",
                " come across as confident ",
                " comes across as confident ",
            ];
            if UNSUPPORTED.iter().any(|phrase| padded.contains(phrase)) {
                errors.push(format!(
                    "{path}: unsupported delivery or personality judgment"
                ));
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                validate_observable_judgments(item, &format!("{path}[{index}]"), errors);
            }
        }
        serde_json::Value::Object(object) => {
            for (key, item) in object {
                validate_observable_judgments(item, &format!("{path}.{key}"), errors);
            }
        }
        _ => {}
    }
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
    let mut previous_order = None;
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
        let impact = strict_enum(
            object.get("impact"),
            &["high", "medium", "low"],
            &format!("{path}.impact"),
            errors,
        );
        let frequency = strict_integer(
            object.get("frequency"),
            1,
            99,
            &format!("{path}.frequency"),
            errors,
        );
        if let (Some(impact), Some(frequency)) = (impact, frequency) {
            let rank = match impact {
                "high" => 3,
                "medium" => 2,
                _ => 1,
            };
            let order = (rank, frequency);
            if previous_order.is_some_and(|previous| previous < order) {
                errors.push(format!(
                    "{path}: items must be sorted by impact then frequency descending"
                ));
            }
            previous_order = Some(order);
        }
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

fn validate_framework_assessment(
    value: Option<&serde_json::Value>,
    plan: Option<&serde_json::Value>,
    errors: &mut Vec<String>,
) {
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
    let plan = plan
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for (index, expected_phase) in IMPROVEMENT_PHASES.iter().enumerate() {
        let path = format!("$.frameworkAssessment.phases[{index}]");
        let Some(row) = rows.get(index).and_then(serde_json::Value::as_object) else {
            errors.push(format!("{path}: expected object"));
            continue;
        };
        exact_keys(row, &["phase", "score", "weaknessTags"], &path, errors);
        if row.get("phase").and_then(serde_json::Value::as_str) != Some(*expected_phase) {
            errors.push(format!("{path}.phase: expected {expected_phase}"));
        }
        if row.get("score") != Some(&serde_json::Value::Null) {
            strict_integer(row.get("score"), 0, 100, &format!("{path}.score"), errors);
        }
        validate_string_array(
            row.get("weaknessTags"),
            0,
            4,
            400,
            &format!("{path}.weaknessTags"),
            errors,
        );
        if let Some(tags) = row
            .get("weaknessTags")
            .and_then(serde_json::Value::as_array)
        {
            // Trimmed on both sides, like every other comparison in this
            // module: `validate_improvement_plan` matches a plan weakness to a
            // feedback improvement after trimming, so a tag that is an exact
            // copy of an accepted weakness apart from surrounding whitespace
            // has to be accepted here too. Comparing raw rejected the whole
            // report over a trailing space and spent the one repair on it.
            let allowed = plan
                .iter()
                .filter(|item| {
                    item.get("phase").and_then(serde_json::Value::as_str) == Some(*expected_phase)
                })
                .filter_map(|item| item.get("weakness").and_then(serde_json::Value::as_str))
                .map(str::trim)
                .collect::<std::collections::HashSet<_>>();
            for tag in tags.iter().filter_map(serde_json::Value::as_str) {
                if !allowed.contains(tag.trim()) {
                    errors.push(format!(
                        "{path}.weaknessTags: tag has no same-phase improvement"
                    ));
                }
            }
        }
    }
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
) -> serde_json::Value {
    let (reason, errors) = match (raw_report, error_note) {
        (Some(raw_report), None) => match validate_report(raw_report, hints_used) {
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
