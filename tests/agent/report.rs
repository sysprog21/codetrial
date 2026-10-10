//! Generating, validating and stamping the report a candidate reads.
//!
//! Split out of `tests/agent.rs`, which is still the test target: cargo
//! discovers only `tests/*.rs`, so this compiles as a module of that one
//! binary rather than relinking the crate for a file of its own.
//!
//! `include_str!` resolves against this file, so every fixture path here is
//! one directory deeper than it was in the parent.

use super::*;

#[test]
fn coding_reports_exclude_drawings_and_keep_the_original_conversation_rubric() {
    let problem = get_problem(Some("two-sum"));
    let valid = valid_strict_report();
    for (field, text) in [
        (
            "codingFeedback",
            "You provided a clear example drawing to illustrate the tree structure.",
        ),
        ("communicationFeedback", "You provided an example drawing."),
        (
            "communicationFeedback",
            "Your picture made the explanation clear.",
        ),
        (
            "codingFeedback",
            "Your pictures demonstrate a correct solution.",
        ),
        (
            "codingFeedback",
            "The image shows a correct solution in your drawing.",
        ),
        (
            "codingFeedback",
            "You interpreted the input image correctly and drew a useful picture.",
        ),
        (
            "communicationFeedback",
            "Your drawing quality demonstrated understanding.",
        ),
        (
            "communicationFeedback",
            "You explained your diagram clearly.",
        ),
        (
            "codingFeedback",
            "You demonstrated understanding by providing a valid sketch with a root and two children.",
        ),
    ] {
        let mut image_credit = valid.clone();
        image_credit[field]["strengths"][0] = json!(text);
        let errors =
            validate_report_for_round(&image_credit, problem, true, InterviewMode::Coding, true)
                .unwrap_err();
        assert!(
            errors.iter().any(
                |error| error.starts_with(&format!("$.{field}.strengths[0]"))
                    && error.contains("original rubric")
            ),
            "{errors:?}"
        );
        validate_report_for_round(
            &image_credit,
            problem,
            true,
            InterviewMode::Whiteboard,
            true,
        )
        .expect("whiteboard assessment still evaluates its own board work");
    }
    let mut explained = valid;
    explained["communicationFeedback"]["strengths"][0] = json!(
        "You told Jim that each tree level has its own mean and explained how to add its values and divide by its node count."
    );
    let accepted =
        validate_report_for_round(&explained, problem, true, InterviewMode::Coding, true).unwrap();
    assert_eq!(
        accepted["communicationFeedback"]["strengths"],
        explained["communicationFeedback"]["strengths"]
    );
    assert_eq!(
        accepted["communicationScore"], explained["communicationScore"],
        "no automatic picture bonus"
    );
    assert_eq!(accepted["codingScore"], explained["codingScore"]);
    explained["codingFeedback"]["strengths"][0] = json!(
        "You explained how to scan the input image pixel by pixel and track connected regions."
    );
    validate_report_for_round(&explained, problem, true, InterviewMode::Coding, true)
        .expect("reasoning about an exercise's image input is ordinary conversation");
    for text in [
        "You explained that the input image shows two connected regions.",
        "You described how the image shows connected regions in the exercise input.",
        "You traced the pixels in the input picture accurately.",
        "You identified the regions in the source pictures.",
    ] {
        explained["codingFeedback"]["strengths"][0] = json!(text);
        validate_report_for_round(&explained, problem, true, InterviewMode::Coding, true)
            .expect("exercise input imagery remains assessable");
    }
}

#[test]
fn coding_feedback_cannot_deduct_for_drawings_or_recommend_them_for_points() {
    let problem = get_problem(Some("two-sum"));
    for (field, item) in [
        ("codingFeedback", "Your drawing was incorrect."),
        (
            "communicationFeedback",
            "Draw a diagram to earn more communication points.",
        ),
    ] {
        let mut report = valid_strict_report();
        report[field]["improvements"][0] = json!(item);
        let plan_index = if field == "codingFeedback" { 0 } else { 2 };
        report["improvementPlan"][plan_index]["weakness"] = json!(item);
        let errors = validate_report_for_round(&report, problem, true, InterviewMode::Coding, true)
            .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.starts_with(&format!("$.{field}.improvements[0]"))),
            "{errors:?}"
        );
    }
}

#[test]
fn strict_report_validation_is_atomic_and_server_owns_hints() {
    let valid = valid_strict_report();
    let report =
        validate_report(&valid, 3, get_problem(Some("two-sum"))).expect("fixture is valid");
    assert_eq!(report["hintsUsed"], 3);

    let mut hostile = valid.clone();
    hostile["codingScore"] = json!(120);
    hostile["decision"] = json!("MAYBE");
    hostile
        .as_object_mut()
        .unwrap()
        .insert("extra".into(), json!(true));
    let errors = validate_report_candidate(&hostile, get_problem(Some("two-sum")))
        .unwrap_err()
        .join("\n");
    assert!(errors.contains("$.codingScore"));
    assert!(errors.contains("$.decision"));
    assert!(errors.contains("$.extra"));

    let incomplete = final_report(Some(&hostile), 3, None, get_problem(Some("two-sum")));
    assert_eq!(incomplete["incomplete"], true);
    assert!(incomplete.get("codingScore").is_none());
    assert!(incomplete.get("decision").is_none());
}

/// A STAR plan item is refused rather than cut out, since cutting it also cuts
/// the feedback improvement it copies and a list can then fall below the two
/// it must hold. A STAR score shrinks nothing, so the server nulls it and the
/// report is accepted. Another fault is reported beside a refused item, not
/// instead of it.
#[test]
fn a_round_that_never_opened_refuses_star_plan_items_and_clears_star_scores() {
    let problem = get_problem(Some("two-sum"));
    let star = valid_strict_report();
    let opened = validate_report_for_round(&star, problem, true, InterviewMode::Coding, true)
        .expect("an opened round keeps STAR");
    assert_eq!(opened["frameworkAssessment"]["phases"][9]["score"], 75);
    let errors =
        validate_report_for_round(&star, problem, false, InterviewMode::Coding, true).unwrap_err();
    assert_eq!(errors.len(), 2, "{errors:?}");
    for path in ["$.improvementPlan[2].phase", "$.improvementPlan[3].phase"] {
        assert!(
            errors.iter().any(|error| error.starts_with(path)),
            "{path} missing from {errors:?}"
        );
    }

    // Written from the coding round, but with the out-of-turn answer still
    // scored: accepted, with every STAR score cleared and the rest kept.
    let mut coding = star.clone();
    coding["communicationFeedback"]["improvements"] =
        json!(["Narrate the invariant", "Say what each test is for"]);
    for (index, phase, weakness) in [
        (2, "Algorithm", "Narrate the invariant"),
        (3, "Test", "Say what each test is for"),
    ] {
        coding["improvementPlan"][index]["phase"] = json!(phase);
        coding["improvementPlan"][index]["weakness"] = json!(weakness);
    }
    let accepted = validate_report_for_round(&coding, problem, false, InterviewMode::Coding, true)
        .expect("STAR scores alone are settled, not refused");
    let rows = accepted["frameworkAssessment"]["phases"]
        .as_array()
        .unwrap();
    assert!(rows[..6].iter().all(|row| row["score"] == 75));
    assert!(rows[6..].iter().all(|row| row["score"].is_null()));
    assert!(rows[6..].iter().all(|row| row["weaknessTags"] == json!([])));

    let mut both = star;
    both["codingScore"] = json!(120);
    let errors =
        validate_report_for_round(&both, problem, false, InterviewMode::Coding, true).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("$.codingScore"))
    );
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("$.improvementPlan[2].phase"))
    );
}

#[test]
fn published_problem_word_splitting_preserves_every_boundary() {
    assert_eq!(
        spelled_words("-camelCase 3Sum LRUCache"),
        ["camel", "case", "3", "sum", "lru", "cache"],
        "punctuation, lower-to-upper, digit-to-upper, and acronym boundaries all split words"
    );
}

#[test]
fn report_naming_the_published_problem_is_refused() {
    let problem = get_problem(Some("3sum"));
    for (path, mutate) in [
        (
            "$.summary",
            Box::new(|report: &mut Value| report["summary"] = json!("This is 3 Sum."))
                as Box<dyn Fn(&mut Value)>,
        ),
        (
            "$.codingFeedback.strengths[0]",
            Box::new(|report: &mut Value| {
                report["codingFeedback"]["strengths"][0] = json!("Found this on LeetCode.")
            }),
        ),
        (
            "$.communicationFeedback.improvements[0]",
            Box::new(|report: &mut Value| {
                report["communicationFeedback"]["improvements"][0] =
                    json!("Explain the Leet Code solution.")
            }),
        ),
        (
            "$.improvementPlan[0].drill",
            Box::new(|report: &mut Value| {
                report["improvementPlan"][0]["drill"] = json!("Practice 3Sum.")
            }),
        ),
    ] {
        let mut report = valid_strict_report();
        mutate(&mut report);
        let errors = validate_report_candidate(&report, problem).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error == &format!("{path}: names the published problem")),
            "{path} was not refused: {errors:?}"
        );
    }
}

#[test]
fn an_original_problem_report_cannot_name_a_practice_site() {
    let problem = get_problem(Some("fixed-capacity-ring-buffer"));
    assert!(names_source(problem, "You found this on Leet Code."));
    let mut report = valid_strict_report();
    report["summary"] = json!("You found this on LeetCode.");
    let errors = validate_report_candidate(&report, problem).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error == "$.summary: names the published problem")
    );
}

#[test]
fn report_naming_only_the_scenario_is_accepted() {
    let problem = get_problem(Some("triangle"));
    let mut report = valid_strict_report();
    report["summary"] = json!("You explained the Triangle scenario with a grounded trace.");
    validate_report_candidate(&report, problem)
        .unwrap_or_else(|errors| panic!("scenario wording was refused: {errors:?}"));
}

#[test]
fn strict_report_validation_rejects_each_semantic_drift_class() {
    type Mutation = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Mutation)> = vec![
        (
            "missing key",
            Box::new(|report| {
                report.as_object_mut().unwrap().remove("summary");
            }),
        ),
        (
            "extra key",
            Box::new(|report| {
                report
                    .as_object_mut()
                    .unwrap()
                    .insert("hintsUsed".into(), json!(999));
            }),
        ),
        (
            "wrong score type",
            Box::new(|report| report["codingScore"] = json!("82")),
        ),
        (
            "duplicate phase",
            Box::new(|report| {
                report["frameworkAssessment"]["phases"][9]["phase"] = json!("Action")
            }),
        ),
        (
            "unknown phase",
            Box::new(|report| {
                report["frameworkAssessment"]["phases"][0]["phase"] = json!("Warmup")
            }),
        ),
        (
            "bad rubric",
            Box::new(|report| report["frameworkAssessment"]["rubricVersion"] = json!(2)),
        ),
        (
            "missing weakness drill",
            Box::new(|report| {
                report["improvementPlan"].as_array_mut().unwrap().pop();
            }),
        ),
        (
            "oversize trimmed text",
            Box::new(|report| report["summary"] = json!(format!("{}x", " ".repeat(1200)))),
        ),
    ];
    for (name, mutate) in cases {
        let mut report = valid_strict_report();
        mutate(&mut report);
        assert!(
            validate_report_candidate(&report, get_problem(Some("two-sum"))).is_err(),
            "accepted {name}"
        );
    }
}

/// The ladder is served, not held: each request that asks for a hint gets the
/// next rung and no other, the last waits for an approach of the candidate's
/// own, and a hint nobody asked for is counted without spending a rung.
#[test]
fn log_hint_hands_out_one_rung_per_request_and_holds_the_last_for_an_approach() {
    let problem = get_problem(Some("3sum"));
    let [first, second, third] = problem.variant().hints else {
        panic!("three rungs");
    };
    let mut state = RuntimeState {
        hint_ladder: problem.variant().hints,
        ..RuntimeState::default()
    };

    let unrequested = record_hint(&mut state, false);
    assert_eq!(unrequested, "Recorded. Total hints so far: 1.");
    assert_eq!(
        state.hint_rungs_given, 0,
        "an unrequested hint spends no rung"
    );
    assert_eq!(state.volunteered_hints, 1);

    let one = record_hint(&mut state, true);
    assert!(one.contains(first) && !one.contains(second), "{one}");
    let two = record_hint(&mut state, true);
    assert!(two.contains(second) && !two.contains(third), "{two}");

    let held = record_hint(&mut state, true);
    assert!(
        !held.contains(third),
        "the key step before any approach: {held}"
    );
    assert!(held.contains("stays withheld") && held.contains("Give no clue this turn"));
    assert!(
        !held.contains(first) && !held.contains(second),
        "a withheld request hands the model a clue to improvise from: {held}"
    );
    assert_eq!(state.hint_rungs_given, 2);
    assert_eq!(
        state.hints_used, 3,
        "a withheld rung gave the candidate nothing, so it is not a hint"
    );
    assert!(held.contains("Not counted as a hint; total hints so far: 3."));

    // Only an approach unlocks it: evidence for another phase does not, and
    // neither does an Algorithm phase the interviewer only inferred.
    for (phase, source, kind) in [
        ("repeat", "candidate_speech", "observed"),
        ("example", "candidate_speech", "inferred"),
        ("algorithm", "candidate_speech", "inferred"),
        ("repeat", "session_timing", "skipped"),
    ] {
        record_framework_evidence(
            &mut state,
            &json!({
                "phase": phase,
                "source": source,
                "kind": kind,
                "confidence": 90,
                "summary": "Candidate restated the task."
            }),
        )
        .unwrap();
        assert!(
            record_hint(&mut state, true).contains("stays withheld"),
            "{phase} {kind} evidence released the key step"
        );
    }
    assert_eq!(state.hints_used, 3, "withheld requests are never counted");
    let mut skipped = state.clone();
    skipped.framework_evidence.push(FrameworkEvidence {
        phase: FrameworkPhase::Algorithm,
        kind: EvidenceKind::Skipped,
        ..skipped.framework_evidence[0].clone()
    });
    assert!(
        record_hint(&mut skipped, true).contains("stays withheld"),
        "skipped Algorithm evidence released the key step"
    );
    assert_eq!(state.hint_rungs_given, 2);

    // Code of their own is an approach too: Coding evidence releases the key
    // step without the Algorithm phase ever being named.
    let mut coded = state.clone();
    coded.framework_evidence.push(FrameworkEvidence {
        phase: FrameworkPhase::Coding,
        kind: EvidenceKind::Observed,
        ..coded.framework_evidence[0].clone()
    });
    assert!(
        record_hint(&mut coded, true).contains(third),
        "Coding evidence did not release the key step"
    );

    record_framework_evidence(
        &mut state,
        &json!({
            "phase": "algorithm",
            "source": "candidate_speech",
            "kind": "observed",
            "confidence": 90,
            "summary": "Candidate proposed pinning one value."
        }),
    )
    .unwrap();
    let three = record_hint(&mut state, true);
    assert!(three.contains(third), "{three}");
    assert!(record_hint(&mut state, true).contains("Every rung is used"));
}

#[test]
fn greeting_introduces_the_scenario_and_never_the_published_problem() {
    for mode in [InterviewMode::Coding, InterviewMode::Whiteboard] {
        // The template's own rules, once per surface; the loop is for what each
        // problem brings. Both greetings owe the same things, so both are held
        // to them, matched without case because one says it mid-sentence and
        // the other opens a sentence with it.
        let opening = greeting(mode);
        let lower = opening.to_lowercase();
        assert!(
            opening.contains("may ask for a hint if they get stuck"),
            "{mode:?}"
        );
        assert!(
            opening.contains("without naming any published problem, practice site"),
            "{mode:?}"
        );
        assert!(
            lower.contains("do not volunteer a constraint, edge case, or hint"),
            "{mode:?}"
        );

        // The scenario reaches the interviewer through THE EXERCISE, which the
        // greeting points it at; repeated in the greeting it was billed twice
        // on every turn.
        assert!(opening.contains("introduce THE EXERCISE"));
        for problem in PROBLEMS {
            let variant = problem.variant();
            // The plan the greeting goes out with, for the surface it opens.
            let exercise = match mode {
                InterviewMode::Coding => instructions(problem, 45),
                InterviewMode::Whiteboard => board_instructions(problem, 45),
            };
            assert!(
                exercise.contains(variant.title),
                "{} lost its title",
                problem.id
            );
            for line in variant.brief {
                assert!(exercise.contains(line), "{} lost its brief", problem.id);
            }
            assert!(
                !opening.contains(variant.title),
                "{} repeats its title",
                problem.id
            );

            // The summary is the published statement in a sentence, and what
            // the interviewer is handed to open with is what it paraphrases
            // aloud.
            assert!(
                !opening.contains(problem.summary),
                "{} opens from the published statement",
                problem.id
            );
            assert!(
                !names_source(problem, &opening),
                "{} names its source",
                problem.id
            );
            assert!(
                !opening.contains(problem.optimal),
                "{} exposed its private optimal approach",
                problem.id
            );
            for secret in variant
                .hints
                .iter()
                .chain(variant.follow_ups)
                .chain(variant.constraints)
            {
                assert!(
                    !opening.contains(secret),
                    "{} exposed private variant text",
                    problem.id
                );
            }
        }
    }
}

#[test]
fn improvement_plans_are_linked_bounded_deduplicated_and_ranked() {
    let mut raw = valid_strict_report();
    raw["improvementPlan"].as_array_mut().unwrap().swap(0, 3);
    raw["improvementPlan"][0]["impact"] = json!("low");
    raw["improvementPlan"][1]["impact"] = json!("high");
    let ranked = validate_report_candidate(&raw, get_problem(Some("two-sum")))
        .expect("order is fixed, not refused");
    assert_eq!(
        ranked["improvementPlan"][0]["weakness"], raw["improvementPlan"][1]["weakness"],
        "the high-impact item leads the plan the candidate reads"
    );
    assert_eq!(
        ranked["improvementPlan"].as_array().unwrap().len(),
        raw["improvementPlan"].as_array().unwrap().len(),
        "sorting a plan neither drops nor invents an item"
    );

    let mut unrelated = valid_strict_report();
    unrelated["improvementPlan"][0]["weakness"] = json!("unrelated advice");
    let errors = validate_report_candidate(&unrelated, get_problem(Some("two-sum")))
        .unwrap_err()
        .join("\n");
    assert!(errors.contains("exactly reference"));
    assert!(errors.contains("exactly one item"));
}

/// An outage is not a candidate. The fallback used to emit `NO_HIRE` with 0/100
/// on both axes, so a Gemini failure reached the candidate as a rejection and
/// was then written to their history and synced to `/api/reports`. The
/// `error: true` flag beside it was dropped by the browser's own sanitizer, so
/// nothing downstream could tell a failed provider from a failed candidate.
#[test]
fn a_report_that_could_not_be_produced_is_not_a_rejection() {
    let report = fallback_report(2, "model unavailable");

    assert_eq!(report["incomplete"], serde_json::json!(true));
    assert!(
        report.get("decision").is_none(),
        "no verdict may be invented: {report}"
    );
    assert!(
        report.get("codingScore").is_none(),
        "no score may be invented: {report}"
    );
    assert!(report.get("communicationScore").is_none());

    // The reason still reaches the candidate, and says plainly that this is not
    // an assessment of them.
    let summary = report["summary"].as_str().unwrap();
    assert!(summary.contains("model unavailable"), "{summary}");
    assert!(
        summary.contains("nothing here is an assessment of your work"),
        "{summary}"
    );
    assert_eq!(report["hintsUsed"], serde_json::json!(2));

    // Advice the candidate can take, whatever the cause. The key and the logs
    // are the operator's, and a 503 has nothing to do with the key.
    assert!(
        summary.contains("Try the interview again later"),
        "{summary}"
    );
    assert!(!summary.contains("GOOGLE_API_KEY"), "{summary}");

    // A cause that ends its own sentence is not given a second full stop.
    let summary =
        fallback_report(0, "Gemini unavailable (status=503): Try again later.")["summary"]
            .as_str()
            .unwrap()
            .to_string();
    assert!(
        summary.contains("Try again later. Your session"),
        "{summary}"
    );

    // One flag, not two. `error: true` used to travel beside `incomplete` and
    // reached no consumer, because the browser's sanitizer dropped it.
    assert!(report.get("error").is_none(), "{report}");
}

#[test]
fn final_report_matches_frontend_publish_contract() {
    let raw = json!({
        "codingScore": 120,
        "communicationScore": "bad",
        "decision": "MAYBE",
        "summary": 42,
        "codingFeedback": {
            "strengths": ["a", "b", "c", "d", "e"],
            "improvements": ["x"],
        },
        "communicationFeedback": "bad",
    });
    let sanitized = final_report(Some(&raw), 2, None, get_problem(Some("two-sum")));
    let fallback = final_report(
        None,
        1,
        Some("model unavailable"),
        get_problem(Some("two-sum")),
    );

    assert_eq!(sanitized["incomplete"], true);
    assert_eq!(fallback, fallback_report(1, "model unavailable"));
    assert!(sanitized.get("codingScore").is_none());
    assert!(sanitized.get("decision").is_none());
    assert_eq!(sanitized["hintsUsed"], 2);
    assert_eq!(fallback["incomplete"], true);
    assert_eq!(fallback["hintsUsed"], 1);
}

/// Whitespace must not decide whether a report survives.
///
/// The plan gate trims a weakness before matching it, because `strict_text`
/// returns a trimmed string, so `"Explain complexity "` is an accepted plan
/// weakness. The tag gate compared raw, so the assessment row naming that same
/// weakness without the space failed. The model is not asked to keep the two
/// byte-identical and has no reason to, and the cost of disagreeing was the
/// whole report plus the one repair it is allowed.
#[test]
fn a_weakness_tag_matches_its_plan_item_across_stray_whitespace() {
    let mut raw = valid_strict_report();
    let plan = raw["improvementPlan"].as_array_mut().unwrap();
    let item = plan
        .iter_mut()
        .find(|item| item["phase"] == "Algorithm")
        .expect("the fixture plans an Algorithm item");
    let padded = format!(" {} ", item["weakness"].as_str().unwrap());
    item["weakness"] = json!(padded);

    assert!(
        validate_report_candidate(&raw, get_problem(Some("two-sum"))).is_ok(),
        "a tag and its plan weakness that differ only in surrounding whitespace \
         must not cost the candidate their report"
    );
}

/// The report validator's bounds are load-bearing, so their edges are pinned.
///
/// This is the only thing standing between a model's free text and a report
/// the candidate is shown, and each of these could be moved by one, or deleted
/// outright, without a test objecting.
#[test]
fn report_validation_holds_its_bounds_and_its_ordering() {
    // An array outside its item count is refused. Without the array check at
    // all, a report with one strength, or with five, would be shown as written.
    for strengths in [json!(["only one"]), json!(["a", "b", "c", "d", "e"])] {
        let mut report = valid_strict_report();
        report["codingFeedback"]["strengths"] = strengths;
        let errors = validate_report_candidate(&report, get_problem(Some("two-sum")))
            .expect_err("an out-of-range array is not a report")
            .join("\n");
        assert!(errors.contains("$.codingFeedback.strengths"), "{errors}");
    }

    // Two identical entries are one entry said twice, which reads as two
    // independent observations of the same weakness.
    let mut repeated = valid_strict_report();
    repeated["codingFeedback"]["strengths"] = json!(["Same point", "Same point"]);
    assert!(
        validate_report_candidate(&repeated, get_problem(Some("two-sum")))
            .expect_err("a duplicate is not a second strength")
            .join("\n")
            .contains("duplicate")
    );

    // The plan is ordered by impact and then by how often the weakness came up,
    // so the first thing the candidate reads is what costs them most. Impacts
    // are set on the fixture's own plan, which keeps it consistent with the
    // feedback and the assessment it cross-references.
    let impacts = |values: [(&str, u32); 4]| {
        let mut report = valid_strict_report();
        for (index, (impact, frequency)) in values.iter().enumerate() {
            report["improvementPlan"][index]["impact"] = json!(impact);
            report["improvementPlan"][index]["frequency"] = json!(frequency);
        }
        report
    };

    // Order is applied here rather than demanded of the model, which used to
    // lose an otherwise sound report over the sequence of four items.
    fn ranks(report: &serde_json::Value) -> Vec<(&str, u64)> {
        report["improvementPlan"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                (
                    item["impact"].as_str().unwrap(),
                    item["frequency"].as_u64().unwrap(),
                )
            })
            .collect()
    }

    // The rarest item leads because it is the costliest, which is the whole of
    // what impact outranking frequency means. Collapse high into medium and
    // this order inverts.
    let buried = impacts([("medium", 9), ("high", 1), ("medium", 2), ("medium", 1)]);
    assert_eq!(
        ranks(
            &validate_report_candidate(&buried, get_problem(Some("two-sum")))
                .expect("a burying order is sorted, not refused")
        ),
        [("high", 1), ("medium", 9), ("medium", 2), ("medium", 1)],
    );

    // Medium outranks low the same way, which the case above cannot show:
    // collapse medium into the same rank as low and frequency decides instead,
    // so the item that came up nine times leads a costlier one.
    let over_low = impacts([("low", 9), ("medium", 1), ("low", 2), ("low", 1)]);
    assert_eq!(
        ranks(
            &validate_report_candidate(&over_low, get_problem(Some("two-sum")))
                .expect("a medium item leads a frequent low one")
        ),
        [("medium", 1), ("low", 9), ("low", 2), ("low", 1)],
    );

    // And within one rank it is the frequency that orders them.
    let by_frequency = impacts([("medium", 1), ("medium", 9), ("medium", 2), ("medium", 1)]);
    assert_eq!(
        ranks(
            &validate_report_candidate(&by_frequency, get_problem(Some("two-sum")))
                .expect("frequency order is applied")
        ),
        [("medium", 9), ("medium", 2), ("medium", 1), ("medium", 1)],
    );
}

/// A plan may name eight things, and eight is allowed.
///
/// Every entry has to answer to a feedback improvement, so a plan this long
/// needs a report whose feedback is equally long. The limit sits one past what
/// that can produce, which is why nothing had reached it: moved down by one it
/// rejects a full report, and turned into an equality it stops refusing the
/// long ones entirely.
#[test]
fn an_improvement_plan_may_carry_eight_entries() {
    let improvements = [
        ("Repeat", "Restate the constraints"),
        ("Example", "Walk a worked example"),
        ("Algorithm", "Explain complexity"),
        ("Test", "Test boundaries"),
        ("Situation", "Set the scene"),
        ("Task", "Name the goal"),
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
    let mut report = valid_strict_report();
    report["codingFeedback"]["improvements"] = json!(
        improvements[..4]
            .iter()
            .map(|(_, weakness)| *weakness)
            .collect::<Vec<_>>()
    );
    report["communicationFeedback"]["improvements"] = json!(
        improvements[4..]
            .iter()
            .map(|(_, weakness)| *weakness)
            .collect::<Vec<_>>()
    );
    report["improvementPlan"] = json!(
        improvements
            .iter()
            .map(|(phase, weakness)| json!({
                "phase": phase, "weakness": weakness, "impact": "medium", "frequency": 1,
                "drill": "Practice the missing step", "durationMin": 5,
                "successCriterion": "State it without prompting",
                "selfReview": ["Grounded in evidence"],
            }))
            .collect::<Vec<_>>()
    );
    report["frameworkAssessment"] = json!({
        "rubricVersion": 1,
        "phases": phases.iter().map(|phase| json!({
            "phase": phase, "score": 75,
            "weaknessTags": improvements.iter()
                .filter(|(assigned, _)| assigned == phase)
                .map(|(_, weakness)| *weakness)
                .collect::<Vec<_>>(),
        })).collect::<Vec<_>>()
    });

    assert!(
        validate_report_candidate(&report, get_problem(Some("two-sum"))).is_ok(),
        "eight is inside the limit: {:?}",
        validate_report_candidate(&report, get_problem(Some("two-sum"))).err()
    );
}

/// A plan weakness that differs from its improvement only in case, spacing or
/// a closing full stop is the same improvement, and is written back as it; one
/// that matches two improvements that way, or paraphrases one, is refused.
#[test]
fn a_plan_weakness_is_matched_across_case_spacing_and_a_full_stop() {
    let mut raw = valid_strict_report();
    let improvement = raw["codingFeedback"]["improvements"][0]
        .as_str()
        .unwrap()
        .to_string();
    let item = raw["improvementPlan"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["weakness"] == improvement.as_str())
        .expect("the fixture plans every improvement");
    item["weakness"] = json!(format!(
        "{}.",
        improvement.to_uppercase().replace(' ', "  ")
    ));
    let accepted = validate_report_candidate(&raw, get_problem(Some("two-sum")))
        .expect("a copy that changed nothing but its case is the same weakness");
    assert!(
        accepted["improvementPlan"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["weakness"] == improvement.as_str())
    );

    let mut paraphrased = valid_strict_report();
    paraphrased["improvementPlan"][0]["weakness"] = json!(format!(
        "Try to {}",
        paraphrased["improvementPlan"][0]["weakness"]
            .as_str()
            .unwrap()
    ));
    assert!(validate_report_candidate(&paraphrased, get_problem(Some("two-sum"))).is_err());

    // Two improvements that differ only in case: a weakness matching both is
    // not snapped to either, and the plan is refused.
    let mut ambiguous = valid_strict_report();
    let first = ambiguous["codingFeedback"]["improvements"][0]
        .as_str()
        .unwrap()
        .to_string();
    ambiguous["codingFeedback"]["improvements"][1] = json!(first.to_uppercase());
    for item in ambiguous["improvementPlan"].as_array_mut().unwrap() {
        if item["weakness"] == first.as_str() {
            item["weakness"] = json!(format!("{first}."));
        }
    }
    let errors = validate_report_candidate(&ambiguous, get_problem(Some("two-sum")))
        .unwrap_err()
        .join("\n");
    assert!(errors.contains("exactly reference"), "{errors}");
}

/// The model no longer writes the phase rows' tags; the server derives them
/// from the plan, and a report that already carries them, as `final_report`
/// hands back, still validates.
#[test]
fn phase_rows_need_no_tags_from_the_model() {
    let mut raw = valid_strict_report();
    for row in raw["frameworkAssessment"]["phases"].as_array_mut().unwrap() {
        row.as_object_mut().unwrap().remove("weaknessTags");
    }
    let accepted = validate_report_candidate(&raw, get_problem(Some("two-sum")))
        .expect("a row without tags is the shape the schema asks for");
    let tagged = accepted["frameworkAssessment"]["phases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| !row["weaknessTags"].as_array().unwrap().is_empty())
        .count();
    assert!(tagged > 0, "the tags are derived from the plan: {accepted}");
    validate_report_candidate(&accepted, get_problem(Some("two-sum")))
        .expect("the derived tags validate again");
    assert!(
        report_response_schema()["properties"]["frameworkAssessment"]["properties"]["phases"]
            ["items"]["properties"]
            .get("weaknessTags")
            .is_none()
    );
}

#[test]
fn drawing_guard_requires_a_received_drawing() {
    let mut report = valid_strict_report();
    report["codingFeedback"]["strengths"][0] = json!("Picture traversal was explained correctly.");
    let problem = get_problem(Some("two-sum"));
    validate_report_for_round(&report, problem, true, InterviewMode::Coding, false)
        .expect("without a received drawing, ordinary words must not activate the guard");
    let errors =
        validate_report_for_round(&report, problem, true, InterviewMode::Coding, true).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.contains("example drawings"))
    );
}

#[test]
fn drawing_guard_keeps_the_exercises_own_diagram_vocabulary() {
    let mut problem = *get_problem(Some("two-sum"));
    problem.summary = "The input diagram contains labeled nodes.";
    let mut report = valid_strict_report();
    report["codingFeedback"]["strengths"][0] = json!("You traversed the diagram correctly.");
    validate_report_for_round(&report, &problem, true, InterviewMode::Coding, true)
        .expect("the exercise's own diagram is assessable");
    report["codingFeedback"]["strengths"][0] = json!("Your drawing explained the diagram clearly.");
    assert!(
        validate_report_for_round(&report, &problem, true, InterviewMode::Coding, true).is_err()
    );
}
