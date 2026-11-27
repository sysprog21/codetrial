use super::*;
use crate::tasks::test_support::{admitted, rename};

async fn session() -> TaskSession {
    let mut session = TaskSession::new(admitted().await, 100);
    session.start(100);
    session.observe_turn(
        "turn-1",
        "I traced the loop and checked what was still open.",
        110,
    );
    session
        .record_check("trace", "covered", "initial", "turn-1")
        .unwrap();
    session
}

fn review() -> Value {
    let dimension = json!({"rating": null, "reason": "No reliable opportunity was captured.", "insufficientReason": "missing_turns", "support": "none", "evidence": []});
    json!({"dimensions": DIMENSIONS.into_iter().map(|name| (name.to_owned(), dimension.clone())).collect::<serde_json::Map<_,_>>(), "gaps": [], "nextActions": ["Trace a boundary input and explain the state."]})
}

#[tokio::test]
async fn learning_review_distinguishes_independent_and_supported_evidence() {
    let mut session = session().await;
    let mut value = review();
    let dimension = &mut value["dimensions"]["reasoningParticipation"];
    dimension["rating"] = json!(3);
    dimension["reason"] = json!("The learner independently traced the input.");
    dimension["insufficientReason"] = Value::Null;
    dimension["evidence"] =
        json!([{"kind": "turn", "id": "turn-1"}, {"kind": "revision", "id": "initial"}]);
    validate(&session, &value, "").unwrap();
    session
        .checks
        .get_mut("trace")
        .unwrap()
        .turn_support
        .insert("turn-1".to_owned(), Support::ConceptualHint);
    assert!(validate(&session, &value, "").is_err());
    value["dimensions"]["reasoningParticipation"]["rating"] = json!(2);
    value["dimensions"]["reasoningParticipation"]["support"] = json!("conceptual_hint");
    validate(&session, &value, "").unwrap();

    value["dimensions"]["reasoningParticipation"]["support"] = json!("targeted_followup");
    assert!(validate(&session, &value, "").is_err());

    // A later hint does not retroactively penalize a different independent
    // turn.
    session.observe_turn(
        "turn-2",
        "I corrected the boundary case independently.",
        120,
    );
    value["dimensions"]["reasoningParticipation"]["rating"] = json!(3);
    value["dimensions"]["reasoningParticipation"]["support"] = json!("none");
    value["dimensions"]["reasoningParticipation"]["evidence"] =
        json!([{"kind": "turn", "id": "turn-2"}]);
    validate(&session, &value, "").unwrap();
}

#[tokio::test]
async fn reviews_refuse_hiring_unsupported_judgments_code_and_dangling_references() {
    let session = session().await;
    for text in [
        "```python\nreturn solution\n```",
        "Hire this learner.",
        "They cheated.",
        "Their accent needs work.",
    ] {
        let mut value = review();
        value["dimensions"]["implementationOwnership"]["reason"] = json!(text);
        assert!(validate(&session, &value, "").is_err(), "accepted {text}");
    }
    let mut value = review();
    value["hireRecommendation"] = json!("yes");
    assert!(validate(&session, &value, "").is_err());
    let mut value = review();
    value["dimensions"]["implementationOwnership"]["evidence"] =
        json!([{"kind": "turn", "id": "absent"}]);
    assert!(validate(&session, &value, "").is_err());
    let mut value = review();
    value["dimensions"]["implementationOwnership"]["insufficientReason"] = Value::Null;
    assert!(validate(&session, &value, "").is_err());
    let mut value = review();
    value["dimensions"]["implementationOwnership"]["rating"] = json!(0);
    value["dimensions"]["implementationOwnership"]["insufficientReason"] = Value::Null;
    value["dimensions"]["implementationOwnership"]["evidence"] =
        json!([{"kind":"turn", "id":"turn-1"}]);
    assert!(validate(&session, &value, "").is_err());
}

/// Value refusals are listed together too, each named by its dimension or list,
/// since each refusal spends one of the review's repairs (#744).
#[tokio::test]
async fn value_refusals_across_dimensions_and_lists_come_at_once() {
    let session = session().await;
    let mut value = review();
    for name in ["implementationOwnership", "revisionAndImprovement"] {
        value["dimensions"][name]["insufficientReason"] = json!("unknown");
    }
    value["gaps"] = json!("not a list");
    assert_eq!(
        validate(&session, &value, "").unwrap_err().0,
        [
            "learning review dimension implementationOwnership: null rating requires an insufficient-evidence reason",
            "learning review dimension revisionAndImprovement: null rating requires an insufficient-evidence reason",
            "learning review gaps: invalid learning review list",
        ]
        .join("; ")
    );
}

/// A refusal of the review's shape names the object, what is wrong with it and
/// the key, since that message is all the repair prompt and the log get (#744).
/// An unknown key that is not a short plain identifier is text the model wrote,
/// so it is reported without being repeated.
#[tokio::test]
async fn a_refused_review_shape_names_the_object_and_the_key() {
    let session = session().await;
    let refusal = |value: Value| validate(&session, &value, "").unwrap_err().to_string();

    assert_eq!(refusal(json!([])), "learning review is not an object");
    let mut value = review();
    value.as_object_mut().unwrap().remove("gaps");
    assert_eq!(refusal(value), r#"learning review is missing "gaps""#);
    let mut value = review();
    value["summary"] = json!("A summary.");
    assert_eq!(
        refusal(value),
        r#"learning review has unknown field "summary""#
    );
    let mut value = review();
    rename(&mut value, "nextActions", "next_actions");
    assert_eq!(
        refusal(value),
        r#"learning review is missing "nextActions" and has unknown field "next_actions""#
    );

    let mut value = review();
    value["dimensions"] = json!([]);
    assert_eq!(
        refusal(value),
        "learning review dimensions is not an object"
    );
    let mut value = review();
    value["dimensions"]["overall"] = json!({});
    assert_eq!(
        refusal(value),
        r#"learning review dimensions has unknown field "overall""#
    );
    let mut value = review();
    let dimension = value["dimensions"]["testingAndDiagnosis"]
        .as_object_mut()
        .unwrap();
    dimension.remove("support");
    dimension.remove("evidence");
    assert_eq!(
        refusal(value),
        r#"learning review dimension testingAndDiagnosis is missing "support", "evidence""#
    );
    let mut value = review();
    value["dimensions"]["reasoningParticipation"]["evidence"] =
        json!([{"type": "turn", "id": "turn-1"}]);
    assert_eq!(
        refusal(value),
        r#"learning review dimension reasoningParticipation evidence reference is missing "kind" and has unknown field "type""#
    );

    // Every problem in one refusal, since each refusal spends a repair: all the
    // unknown keys of an object, up to a cap, and every misshapen dimension.
    let mut value = review();
    for key in ["summary", "overall", "score"] {
        value[key] = json!("x");
    }
    assert_eq!(
        refusal(value),
        r#"learning review has unknown fields "overall", "score", "summary""#
    );
    let mut value = review();
    for n in 0..9 {
        value[format!("k{n}")] = json!(1);
    }
    assert_eq!(
        refusal(value),
        r#"learning review has 9 unknown fields, among them "k0", "k1", "k2", "k3", "k4", "k5", "k6", "k7""#
    );
    let mut value = review();
    value["summary"] = json!("x");
    value["9lives"] = json!(1);
    assert_eq!(
        refusal(value),
        r#"learning review has 2 unknown fields, among them "summary""#
    );
    let mut value = review();
    for name in DIMENSIONS {
        rename(
            &mut value["dimensions"][name],
            "insufficientReason",
            "insufficient_reason",
        );
    }
    let one = |name: &str| {
        format!(
            r#"learning review dimension {name} is missing "insufficientReason" and has unknown field "insufficient_reason""#
        )
    };
    assert_eq!(refusal(value), DIMENSIONS.map(one).join("; "));
    let typed = |name: &str| {
        format!(
            r#"learning review dimension {name} evidence reference is missing "kind" and has unknown field "type""#
        )
    };
    let mut value = review();
    for name in DIMENSIONS {
        value["dimensions"][name]["evidence"] = json!([
            {"type": "turn", "id": "turn-1"},
            {"type": "revision", "id": "initial"}
        ]);
    }
    assert_eq!(
        refusal(value),
        DIMENSIONS.map(typed).join("; "),
        "each dimension once, however many of its references share the fault"
    );

    // A dimension's references are read even when its own keys are refused:
    // snake_case and `type` in every dimension are eight places, which fit in
    // one refusal, and used to take two.
    let mut value = review();
    for name in DIMENSIONS {
        rename(
            &mut value["dimensions"][name],
            "insufficientReason",
            "insufficient_reason",
        );
        value["dimensions"][name]["evidence"] = json!([{"type": "turn", "id": "turn-1"}]);
    }
    assert_eq!(
        refusal(value),
        DIMENSIONS
            .into_iter()
            .flat_map(|name| [one(name), typed(name)])
            .collect::<Vec<_>>()
            .join("; ")
    );

    // A value fault in a dimension whose shape passed comes in the same refusal
    // as a shape fault in another, after it, and used to wait for the next.
    let mut value = review();
    rename(
        &mut value["dimensions"]["testingAndDiagnosis"],
        "insufficientReason",
        "insufficient_reason",
    );
    value["dimensions"]["reasoningParticipation"]["support"] = json!("guessed");
    assert_eq!(
        refusal(value),
        [
            one("testingAndDiagnosis"),
            "learning review dimension reasoningParticipation: invalid support provenance"
                .to_owned(),
        ]
        .join("; ")
    );

    // A dimension whose references are refused is not read for its values,
    // which would name the reference again as a dangling one.
    let mut value = review();
    value["dimensions"]["reasoningParticipation"]["evidence"] =
        json!([{"type": "turn", "id": "turn-1"}]);
    value["dimensions"]["revisionAndImprovement"]["support"] = json!("guessed");
    assert_eq!(
        refusal(value),
        [
            typed("reasoningParticipation"),
            "learning review dimension revisionAndImprovement: invalid support provenance"
                .to_owned(),
        ]
        .join("; ")
    );

    let mut value = review();
    value["dimensions"]["reasoningParticipation"]
        .as_object_mut()
        .unwrap()
        .remove("support");
    value["dimensions"]["revisionAndImprovement"]["evidence"] =
        json!([{"type": "turn", "id": "turn-1"}]);
    assert_eq!(
        refusal(value),
        [
            r#"learning review dimension reasoningParticipation is missing "support""#.to_owned(),
            typed("revisionAndImprovement"),
        ]
        .join("; ")
    );

    // A dimension that is not an object is named, and one after it is still
    // read.
    let mut value = review();
    value["dimensions"]["testingAndDiagnosis"] = Value::Null;
    value["dimensions"]["revisionAndImprovement"]
        .as_object_mut()
        .unwrap()
        .remove("support");
    assert_eq!(
        refusal(value),
        [
            "learning review dimension testingAndDiagnosis is not an object",
            r#"learning review dimension revisionAndImprovement is missing "support""#,
        ]
        .join("; ")
    );

    // Every level at once: snake_case throughout used to be told one level per
    // refusal, the review, then `dimensions`, then each dimension. The renamed
    // dimension is not read, so its own keys add nothing.
    let mut value = review();
    for name in DIMENSIONS {
        rename(
            &mut value["dimensions"][name],
            "insufficientReason",
            "insufficient_reason",
        );
    }
    rename(&mut value, "nextActions", "next_actions");
    rename(
        &mut value["dimensions"],
        "reasoningParticipation",
        "reasoning_participation",
    );
    assert_eq!(
        refusal(value),
        [
            r#"learning review is missing "nextActions" and has unknown field "next_actions""#
                .to_owned(),
            r#"learning review dimensions is missing "reasoningParticipation" and has unknown field "reasoning_participation""#
                .to_owned(),
            one("implementationOwnership"),
            one("testingAndDiagnosis"),
            one("revisionAndImprovement"),
        ]
        .join("; ")
    );
    let mut value = review();
    for n in 0..5 {
        value[format!("next-actions-{n}")] = json!(1);
    }
    assert_eq!(
        refusal(value),
        "learning review has 5 unknown fields whose names are not identifiers"
    );

    // One refusal names at most `MAX_REFUSALS` places, however many references
    // carry a key of their own.
    let mut value = review();
    for name in DIMENSIONS {
        let references: Vec<Value> = (0..8)
            .map(|n| {
                let mut reference = json!({"kind": "turn", "id": "turn-1"});
                reference[format!("k{n}")] = json!(1);
                reference
            })
            .collect();
        value["dimensions"][name]["evidence"] = json!(references);
    }
    let why = refusal(value);
    assert!(why.ends_with("; and 24 more"), "{why}");
    assert_eq!(why.matches("; ").count(), 8, "{why}");

    // The eighth reference is read, and a ninth is refused as a value.
    let good = json!({"kind": "turn", "id": "turn-1"});
    let extra = json!({"kind": "turn", "id": "turn-1", "note": "x"});
    let mut eight = vec![good.clone(); 7];
    eight.push(extra.clone());
    let mut value = review();
    value["dimensions"]["reasoningParticipation"]["evidence"] = json!(eight);
    assert_eq!(
        refusal(value),
        r#"learning review dimension reasoningParticipation evidence reference has unknown field "note""#
    );
    let mut nine = vec![good; 8];
    nine.push(extra);
    let mut value = review();
    value["dimensions"]["reasoningParticipation"]["evidence"] = json!(nine);
    assert_eq!(
        refusal(value),
        "learning review dimension reasoningParticipation: invalid dimension evidence"
    );

    let named = "a".repeat(64);
    let mut value = review();
    value[named.as_str()] = json!(1);
    assert_eq!(
        refusal(value),
        format!(r#"learning review has unknown field "{named}""#)
    );
    for key in [
        "a".repeat(65),
        "9lives".to_owned(),
        "int main() { return 0; }".to_owned(),
    ] {
        let mut value = review();
        value[key.as_str()] = json!(1);
        assert_eq!(
            refusal(value),
            "learning review has an unknown field whose name is not an identifier",
            "{key}"
        );
    }
}

#[tokio::test]
async fn sparse_and_overflowed_sessions_have_explicit_nulls_and_server_metadata() {
    let mut session = TaskSession::new(admitted().await, 100);
    let value = review();
    validate(&session, &value, "").unwrap();
    session.capture_overflow = true;
    let stamped = stamp(&session, &value, "session-1");
    assert_eq!(stamped["assessmentMode"], "task");
    assert_eq!(stamped["taskAssessment"]["captureOverflow"], true);
    assert_eq!(stamped["taskAssessment"]["taskId"], "delimiter-closer");
    let reference = "if not stack or stack.pop() != pairs[char]: return False";
    assert!(reference_overlap(reference, reference));
    assert!(!reference_overlap(
        "Explain the state before each step.",
        reference
    ));
    let mut value = value;
    value["nextActions"] = json!([reference]);
    assert!(validate(&session, &value, reference).is_err());
}

#[tokio::test]
async fn server_unavailable_review_matches_the_browser_wire_fixture() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../fixtures/task-review.json")).unwrap();
    assert_eq!(fixture["topic"], crate::runtime::TOPIC_TASK_REVIEW);
    let mut session = session().await;
    session.final_revision_id = Some("initial".to_owned());
    assert_eq!(
        unavailable(&session, "room-1"),
        fixture["cases"][0]["payload"]
    );
}

#[tokio::test]
async fn support_follows_the_cited_turn_not_the_rest_of_its_check() {
    let mut session = session().await;
    session
        .action(
            crate::tasks::session::Action {
                version: 1,
                request_id: "clue".to_owned(),
                action: crate::tasks::session::ActionKind::Hint,
                revision_id: None,
                target_id: None,
            },
            111,
        )
        .unwrap();
    session.observe_turn(
        "turn-2",
        "After the clue I traced the nested case again.",
        112,
    );
    session
        .record_check_with_support("trace", "covered", "initial", "turn-2", Some("hint-1"))
        .unwrap();
    assert_eq!(session.checks["trace"].support, Support::ConceptualHint);
    let mut value = review();
    let dimension = &mut value["dimensions"]["reasoningParticipation"];
    dimension["rating"] = json!(3);
    dimension["reason"] = json!("The learner traced the input before any hint.");
    dimension["insufficientReason"] = Value::Null;
    dimension["evidence"] = json!([{"kind": "turn", "id": "turn-1"}]);
    validate(&session, &value, "").unwrap();
    let dimension = &mut value["dimensions"]["reasoningParticipation"];
    dimension["rating"] = json!(2);
    dimension["support"] = json!("conceptual_hint");
    assert!(validate(&session, &value, "").is_err());
    value["dimensions"]["reasoningParticipation"]["evidence"] =
        json!([{"kind": "turn", "id": "turn-2"}]);
    validate(&session, &value, "").unwrap();
}

#[tokio::test]
async fn unrecognized_turns_support_no_rating_and_the_model_never_reads_them() {
    let mut session = session().await;

    // Greek letters stand for a turn the recognizer returned in another script;
    // the script, not any language, is what the rule tests.
    let garbled =
        "\u{3b1}\u{3b2}\u{3b3}\u{3b4} \u{3b5}\u{3b6}\u{3b7}\u{3b8} \u{3b9}\u{3ba}\u{3bb}\u{3bc}";
    session.observe_turn("turn-2", garbled, 112);
    let mut value = review();
    let dimension = &mut value["dimensions"]["reasoningParticipation"];
    dimension["rating"] = json!(1);
    dimension["reason"] = json!("The learner began to explain.");
    dimension["insufficientReason"] = Value::Null;
    dimension["evidence"] = json!([{"kind": "turn", "id": "turn-2"}]);
    assert!(validate(&session, &value, "").is_err());
    let dimension = &mut value["dimensions"]["reasoningParticipation"];
    dimension["rating"] = Value::Null;
    dimension["insufficientReason"] = json!("garbled_turns");
    validate(&session, &value, "").unwrap();
    let prompt = prompt(&session);
    assert!(!prompt.contains(garbled));
    assert!(prompt.contains(crate::agent::UNRECOGNIZED_TURN));
    assert!(prompt.contains("I traced the loop"));
}

#[tokio::test]
async fn validated_review_matches_the_browser_wire_fixture() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../fixtures/task-review.json")).unwrap();
    let expected = &fixture["cases"][1]["payload"];
    let session = session().await;
    let mut value = review();
    value["dimensions"]["reasoningParticipation"] = json!({"rating": 3,
        "reason": "The learner traced the loop state before any hint.",
        "insufficientReason": null, "support": "none",
        "evidence": [{"kind": "turn", "id": "turn-1"}]});
    value["gaps"] = json!(["No practice run was captured."]);
    value["nextActions"] = json!(["Run the public cases and predict each result first."]);
    validate(&session, &value, "").unwrap();
    assert_eq!(&stamp(&session, &value, "room-1"), expected);
}

#[tokio::test]
async fn ended_attempt_matches_the_browser_wire_fixture() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../fixtures/task-review.json")).unwrap();
    let mut session = session().await;
    session
        .acknowledge_edit("draft", "print('so far')", 150)
        .unwrap();
    let end = serde_json::from_value(json!({"version": 1, "requestId": "hide",
        "outcome": "invalid", "cause": "visibility", "durationMs": 0}))
    .unwrap();
    session.end(end, 220).unwrap();
    assert_eq!(ended(&session, "room-1"), fixture["cases"][2]["payload"]);
}

#[tokio::test]
async fn advice_never_coaches_a_learner_to_be_easier_to_transcribe() {
    let session = session().await;
    for advice in [
        "Speak more clearly to avoid transcription ambiguity.",
        "Explain your answer in English next time.",
        "Make your explanation audible.",
    ] {
        for field in ["gaps", "nextActions"] {
            let mut value = review();
            value[field] = json!([advice]);
            assert!(validate(&session, &value, "").is_err(), "{field}: {advice}");
        }
    }
    // A neutral mention of transcription in a dimension reason stays allowed.
    let mut value = review();
    value["dimensions"]["reasoningParticipation"]["reason"] =
        json!("Evidence was limited by transcription.");
    validate(&session, &value, "").unwrap();
}

#[tokio::test]
async fn an_attempt_ended_under_a_rule_is_a_record_without_ratings() {
    let mut session = session().await;
    session
        .acknowledge_edit("draft", "print('so far')", 150)
        .unwrap();
    let end = serde_json::from_value(json!({"version": 1, "requestId": "e",
        "outcome": "invalid", "cause": "visibility", "durationMs": 0}))
    .unwrap();
    session.end(end, 220).unwrap();
    let result = ended(&session, "room-1");
    let assessment = &result["taskAssessment"];
    assert_eq!(result["assessmentMode"], "task");
    assert_eq!(assessment["outcome"]["outcome"], "invalid");
    assert_eq!(assessment["outcome"]["cause"], "visibility");
    assert_eq!(assessment["outcome"]["at"], 120);
    assert_eq!(assessment["finalRevisionId"], "draft");
    assert_eq!(assessment["finalCode"], "print('so far')");
    assert_eq!(assessment["githubLogin"], "learner");
    assert_eq!(assessment["rulesAcknowledgedAt"], "2026-10-07T00:00:00Z");
    assert_eq!(assessment["calibration"]["metric"], "keypoints");
    for rated in ["dimensions", "gaps", "nextActions", "feedbackUnavailable"] {
        assert!(
            assessment.get(rated).is_none() && result.get(rated).is_none(),
            "{rated}"
        );
    }
}

#[tokio::test]
async fn review_prompt_contains_public_assignment_requirements_only() {
    let session = session().await;
    let text = prompt(&session);
    let line = text
        .lines()
        .find(|line| line.starts_with("PUBLIC TASK REQUIREMENTS"))
        .unwrap();
    let projection: Value = serde_json::from_str(line.split_once(": ").unwrap().1).unwrap();
    let public = session.task.exercise.live_projection("python");
    for field in [
        "brief",
        "contract",
        "completionTargets",
        "understandingChecks",
    ] {
        assert!(!projection[field].is_null());
        assert_eq!(projection[field], public[field]);
    }
    for field in [
        "reference",
        "referenceCode",
        "interactionPrompt",
        "privateNotes",
        "hints",
        "clarifications",
    ] {
        assert!(projection.get(field).is_none());
    }
}

#[tokio::test]
async fn the_review_reads_turns_in_the_order_they_were_spoken() {
    let mut session = TaskSession::new(admitted().await, 100);
    session.start(100);
    for index in 1..=11 {
        session.observe_turn(&format!("turn-{index}"), &format!("point {index}"), 101);
    }
    let prompt = prompt(&session);
    let second = prompt.find("\"turn-2\"").unwrap();
    let tenth = prompt.find("\"turn-10\"").unwrap();
    assert!(second < tenth, "turn-10 was read before turn-2");
}

#[tokio::test]
async fn a_hinted_turn_past_the_reference_cap_still_counts_as_supported() {
    let mut session = session().await;
    // Fill the check's reference list with independent turns first.
    for index in 2..=8 {
        let turn = format!("turn-{index}");
        session.observe_turn(&turn, "I traced the loop state once more.", 111);
        session
            .record_check("trace", "covered", "initial", &turn)
            .unwrap();
    }
    session
        .action(
            crate::tasks::session::Action {
                version: 1,
                request_id: "clue".to_owned(),
                action: crate::tasks::session::ActionKind::Hint,
                revision_id: None,
                target_id: None,
            },
            112,
        )
        .unwrap();
    session.observe_turn("turn-9", "After the clue I traced it again.", 113);
    session
        .record_check_with_support("trace", "covered", "initial", "turn-9", Some("hint-1"))
        .unwrap();
    assert!(
        !session.checks["trace"]
            .turn_ids
            .contains(&"turn-9".to_owned())
    );
    let mut value = review();
    let dimension = &mut value["dimensions"]["reasoningParticipation"];
    dimension["rating"] = json!(3);
    dimension["reason"] = json!("The learner traced the loop again.");
    dimension["insufficientReason"] = Value::Null;
    dimension["evidence"] = json!([{"kind": "turn", "id": "turn-9"}]);
    assert!(
        validate(&session, &value, "").is_err(),
        "a turn after a hint was rated as independent"
    );
}

#[test]
fn text_is_tokenized_into_words_and_single_punctuation() {
    // Words keep their underscores; each mark is its own token; whitespace and
    // the end of the text are none.
    assert_eq!(
        tokens("stack_top = pairs[c]\t+ 1"),
        ["stack_top", "=", "pairs", "[", "c", "]", "+", "1"]
    );
    assert!(tokens(" \n").is_empty());
}

#[tokio::test]
async fn a_testing_rating_cites_a_run_that_ran_tests() {
    let mut session = session().await;
    for (revision, run, passed, total, diagnostics) in [
        ("tested", "ran", 1, 2, ""),
        ("crashed", "broke", 0, 0, "SyntaxError: invalid syntax"),
    ] {
        let message: crate::tasks::session::RevisionMessage = serde_json::from_value(json!({
            "version": 1, "requestId": format!("capture-{run}"), "revisionId": revision,
            "code": format!("# {revision}"), "trigger": "run"}))
        .unwrap();
        session.revision(message, 111).unwrap();
        let result: crate::tasks::session::RunMessage = serde_json::from_value(json!({
            "version": 1, "requestId": format!("result-{run}"), "runId": run,
            "revisionId": revision, "passed": passed, "total": total,
            "diagnostics": diagnostics}))
        .unwrap();
        session.run(result, 112).unwrap();
    }
    let rated = |run: &str| {
        let mut value = review();
        let dimension = &mut value["dimensions"]["testingAndDiagnosis"];
        dimension["rating"] = json!(3);
        dimension["reason"] = json!("The learner ran the tests and read the failure.");
        dimension["insufficientReason"] = Value::Null;
        dimension["evidence"] =
            json!([{"kind": "turn", "id": "turn-1"}, {"kind": "run", "id": run}]);
        validate(&session, &value, "").map_err(|error| error.0)
    };
    assert_eq!(rated("ran"), Ok(()));
    // A rating and a reason for having none contradict each other.
    let mut value = review();
    let dimension = &mut value["dimensions"]["reasoningParticipation"];
    dimension["rating"] = json!(3);
    dimension["evidence"] = json!([{"kind": "turn", "id": "turn-1"}]);
    assert_eq!(
        validate(&session, &value, "").map_err(|error| error.0),
        Err("learning review dimension reasoningParticipation: numeric rating needs reliable evidence".to_owned())
    );
    // A runner failure ran no test, so it is no evidence about testing.
    assert_eq!(
        rated("broke"),
        Err(
            "learning review dimension testingAndDiagnosis: numeric rating needs reliable evidence"
                .to_owned()
        )
    );
}

#[tokio::test]
async fn a_zero_cites_an_unresolved_check_and_a_two_names_its_support() {
    let mut session = session().await;
    session.observe_turn("turn-2", "I am not sure what the stack is for.", 111);
    session
        .record_check("contract", "unresolved", "initial", "turn-2")
        .unwrap();
    let rated = |session: &TaskSession, rating: u64, support: &str, turn: &str| {
        let mut value = review();
        let dimension = &mut value["dimensions"]["reasoningParticipation"];
        dimension["rating"] = json!(rating);
        dimension["reason"] = json!("What the learner said about the loop.");
        dimension["insufficientReason"] = Value::Null;
        dimension["support"] = json!(support);
        dimension["evidence"] = json!([{"kind": "turn", "id": turn}]);
        validate(session, &value, "").map_err(|error| error.0)
    };
    assert_eq!(rated(&session, 0, "none", "turn-2"), Ok(()));
    // turn-1 covered its check, so it is no unresolved opportunity.
    assert_eq!(
        rated(&session, 0, "none", "turn-1"),
        Err("learning review dimension reasoningParticipation: zero needs an observed unresolved opportunity".to_owned())
    );
    // A two needs both the support named and the evidence bound to it.
    assert_eq!(
        rated(&session, 2, "conceptual_hint", "turn-1"),
        Err("learning review dimension reasoningParticipation: supported rating needs support provenance".to_owned())
    );
    session
        .checks
        .get_mut("trace")
        .unwrap()
        .turn_support
        .insert("turn-1".to_owned(), Support::ConceptualHint);
    assert_eq!(
        rated(&session, 2, "none", "turn-1"),
        Err("learning review dimension reasoningParticipation: supported rating needs support provenance".to_owned())
    );
    assert_eq!(rated(&session, 2, "conceptual_hint", "turn-1"), Ok(()));
}

#[tokio::test]
async fn the_record_says_how_the_attempt_ended_once_it_has() {
    let mut session = session().await;
    assert!(session.evidence().outcome().is_none());
    assert!(session.conclude(crate::tasks::session::Cause::Timeout, 0, 200));
    let outcome = session.evidence().outcome().expect("the attempt has ended");
    assert_eq!(outcome.cause, crate::tasks::session::Cause::Timeout);
}

#[tokio::test]
async fn a_session_keeps_its_result_for_the_learner_who_reloads() {
    use crate::tasks::access::ResultLookup;
    use crate::tasks::test_support::{admit_in_room, unlocked_service};
    let service = unlocked_service();
    let task = admit_in_room(&service, "room-1");
    let session = TaskSession::new(task, 100);
    session.retain_result("room-1", json!({"x": 1}), 200);
    drop(session);
    assert_eq!(
        service.result(1, "room-1", 201),
        ResultLookup::Ready(json!({"x": 1}))
    );
}

#[tokio::test]
async fn the_review_is_read_against_the_reference_the_task_declares() {
    let session = session().await;
    assert_eq!(
        session.facts().review_reference(),
        session.task.exercise.review_reference()
    );
}
