use super::*;
use crate::tasks::session::Phase;

fn fixture(name: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(format!("tests/fixtures/task-mode/{name}.json")).unwrap(),
    )
    .unwrap()
}

fn record(fixture: &Value) -> Result<TaskRecord, TaskError> {
    TaskRecord::from_bank(
        &fixture["problem"],
        &fixture["judge"],
        &fixture["variant"],
        fixture.get("sidecar"),
    )
}

#[test]
fn sessions_use_their_own_exercise_and_release_it() {
    let one = Arc::new(record(&fixture("customized")).unwrap());
    let two = Arc::new(record(&fixture("uncustomized")).unwrap());
    let weak_one = Arc::downgrade(&one);
    let weak_two = Arc::downgrade(&two);
    let first = Exercise(one);
    let second = Exercise(two);
    assert_eq!(first.id(), "delimiter-closer");
    assert_eq!(second.id(), "two-sum");
    assert!(
        first
            .starter("python")
            .unwrap()
            .contains("delimitersNestCleanly")
    );
    assert!(
        second
            .starter("python")
            .unwrap()
            .contains("matchDisputedCharge")
    );
    assert!(
        first
            .task_prompt(Phase::Work, Some("closer-branch"), "draft")
            .unwrap()
            .contains("complete the marked branch")
    );
    assert!(
        !second
            .task_prompt(Phase::Work, None, "draft")
            .unwrap()
            .contains("complete the marked branch")
    );
    assert_eq!(first.hints().len(), 3);
    assert_ne!(first.hints(), second.hints());
    assert!(
        first
            .starter("python")
            .unwrap()
            .contains("TASK_COMPLETE_CLOSER")
    );

    // Hint rungs are counted by the attempt's session (tests/unit/tasks/
    // session.rs); what matters here is that each exercise is its own and
    // nothing keeps it alive once its sessions end.
    drop(first);
    drop(second);
    assert!(weak_one.upgrade().is_none());
    assert!(weak_two.upgrade().is_none());
}

#[test]
fn rust_sidecars_refuse_python_contract_errors() {
    let base = fixture("customized");
    let mut mutations = Vec::new();
    let mut bad = base.clone();
    bad["sidecar"]["unknown"] = json!(true);
    mutations.push(bad);
    let mut bad = base.clone();
    bad["sidecar"]["promptVersion"] = json!(2);
    mutations.push(bad);
    let mut bad = base.clone();
    bad["sidecar"]["promptVersion"] = json!(true);
    mutations.push(bad);
    let mut bad = base.clone();
    bad["sidecar"]["promptVersion"] = json!(1.0);
    mutations.push(bad);
    let mut bad = base.clone();
    bad["sidecar"]["interactionPrompt"] = json!("{{optimal}}");
    mutations.push(bad);
    let mut bad = base.clone();
    bad["sidecar"]["interactionPrompt"] = json!("{{title");
    mutations.push(bad);
    let mut bad = base.clone();
    bad["sidecar"]["interactionPrompt"] = json!("x".repeat(4097));
    mutations.push(bad);
    let mut bad = base.clone();
    bad["sidecar"]["maxHintRungs"] = json!(4);
    mutations.push(bad);
    let mut bad = base.clone();
    bad["sidecar"]["requiredCases"] = json!(["nested"]);
    mutations.push(bad);
    let mut bad = base.clone();
    bad["sidecar"]["requiredCases"] = json!(["nested: s=\"{[]}\"", "nested: s=\"{[]}\""]);
    mutations.push(bad);
    let mut bad = base.clone();
    bad["problem"]["starterCode"]["python"] =
        json!("# TASK_COMPLETE_CLOSER # TASK_COMPLETE_CLOSER");
    mutations.push(bad);
    let mut bad = base.clone();
    bad["problem"]["starterCode"]["python"] = json!("# TASK_COMPLETE_CLOSER_SUFFIX");
    mutations.push(bad);
    let mut bad = base.clone();
    bad["sidecar"] = Value::Null;
    mutations.push(bad);
    let mut bad = base.clone();
    let case = bad["judge"]["cases"][4].clone();
    bad["judge"]["cases"].as_array_mut().unwrap().push(case);
    mutations.push(bad);
    for (index, bad) in mutations.iter().enumerate() {
        assert!(record(bad).is_err(), "mutation {index} accepted");
    }
    assert_eq!(record(&base).unwrap().required_case_indices(), &[4, 2, 5]);
}

#[test]
fn task_prompt_recovers_custom_instructions_and_substitutes_once() {
    let mut data = fixture("customized");
    data["sidecar"]["interactionPrompt"] = json!("{ {{title}} { {{language}} {{target}} {{phase}}");
    let exercise = Exercise(Arc::new(record(&data).unwrap()));
    let initial = exercise
        .task_prompt(Phase::Work, Some("closer-branch"), "draft")
        .unwrap();
    let recovery = exercise
        .task_prompt(Phase::WrapUp, Some("closer-branch"), "revision")
        .unwrap();
    assert!(initial.contains("Template Delimiter Audit"));
    assert!(recovery.contains("Complete the branch without changing the surrounding contract."));
    assert!(recovery.contains("wrap-up"));
    assert!(
        exercise
            .task_prompt(Phase::Work, Some("missing"), "")
            .is_err()
    );
    assert_eq!(
        interpolate("a{{{title}}{", &BTreeMap::from([("title", "sample")])).unwrap(),
        "a{sample{"
    );
    assert!(interpolate("}} {{title}}", &BTreeMap::from([("title", "sample")])).is_err());
}

#[test]
fn task_bank_prompts_have_only_allowlisted_material_and_match_golden() {
    let problems: Value =
        serde_json::from_str(include_str!("../../../problem-bank/problems.json")).unwrap();
    let variants: Value =
        serde_json::from_str(include_str!("../../../problem-bank/variants.json")).unwrap();
    let judges: Value =
        serde_json::from_str(include_str!("../../../problem-bank/judges.json")).unwrap();
    let mut hashes = BTreeMap::new();
    for problem in problems.as_array().unwrap() {
        let id = problem["id"].as_str().unwrap();
        let exercise = Exercise(Arc::new(
            TaskRecord::from_bank(problem, &judges[id], &variants[id], None).unwrap(),
        ));
        let initial = exercise.task_prompt(Phase::Work, None, "").unwrap();
        let recovery = exercise.task_prompt(Phase::WrapUp, None, "").unwrap();
        for prompt in [&initial, &recovery] {
            assert!(
                !prompt.contains(problem["optimal"].as_str().unwrap()),
                "optimal leak for {id}"
            );
            assert!(
                !prompt.contains(problem["pitfalls"].as_str().unwrap()),
                "pitfalls leak for {id}"
            );
            assert!(!prompt.contains("\"optimal\":"));
            assert!(!prompt.contains("\"pitfalls\":"));
            assert!(!prompt.contains("\"guide\":"));
            if let Some(guide) = crate::agent::problems::guide_for(id) {
                assert!(!prompt.contains(guide), "guide leak for {id}");
            }
        }
        let digest = |text: &str| {
            ring::digest::digest(&ring::digest::SHA256, text.as_bytes())
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        hashes.insert(
            id.to_owned(),
            json!({"live": digest(&initial), "recovery": digest(&recovery)}),
        );
    }
    assert_eq!(hashes.len(), crate::agent::PROBLEMS.len());
    assert!(hashes.len() >= 150);
    let actual = format!("{}\n", serde_json::to_string_pretty(&hashes).unwrap());
    let path = "tests/golden/task-prompts.json";
    if std::env::var("UPDATE_PROMPT_GOLDEN").as_deref() == Ok("1") {
        std::fs::write(path, &actual).unwrap();
    }
    assert_eq!(actual, std::fs::read_to_string(path).unwrap());
}

#[test]
fn instructor_rendered_prompts_match_the_runtime_composition() {
    for (fixture_name, id) in [
        ("customized", "delimiter-closer"),
        ("uncustomized", "two-sum"),
    ] {
        let exercise = Exercise(Arc::new(record(&fixture(fixture_name)).unwrap()));
        let expected =
            std::fs::read_to_string(format!("tests/golden/task-preview/{id}.txt")).unwrap();
        assert_eq!(
            exercise.task_prompt(Phase::Ready, None, "").unwrap(),
            expected
        );
    }
}

#[test]
fn downloaded_records_refuse_missing_and_malformed_bank_fields() {
    let base = fixture("customized");
    for (section, field) in [
        ("variant", "brief"),
        ("variant", "clarifications"),
        ("variant", "followUps"),
        ("judge", "kind"),
        ("judge", "checker"),
        ("problem", "topics"),
        ("problem", "difficulty"),
        ("problem", "origin"),
    ] {
        let mut bad = base.clone();
        bad[section][field] = Value::Null;
        assert!(record(&bad).is_err(), "accepted invalid {section}.{field}");
    }
    let mut bad = base.clone();
    bad["variant"]["examples"][0]["case"] = json!(true);
    assert!(record(&bad).is_err());
    let mut bad = base;
    bad["variant"]["unknown"] = json!("private notes");
    assert!(record(&bad).is_err());
}

#[test]
fn every_public_judge_and_starter_matches_the_instructor_preview() {
    let problems: Value =
        serde_json::from_str(include_str!("../../../problem-bank/problems.json")).unwrap();
    let variants: Value =
        serde_json::from_str(include_str!("../../../problem-bank/variants.json")).unwrap();
    let judges: Value =
        serde_json::from_str(include_str!("../../../problem-bank/judges.json")).unwrap();
    let expected: Value =
        serde_json::from_str(include_str!("../../golden/task-posed.json")).unwrap();
    for problem in problems.as_array().unwrap() {
        let id = problem["id"].as_str().unwrap();
        let record = TaskRecord::from_bank(problem, &judges[id], &variants[id], None).unwrap();
        let value = json!({"judge": record.judge(), "starterCode": record.starters, "optimal": record.optimal, "pitfalls": record.pitfalls});
        let bytes = serde_json::to_vec(&value).unwrap();
        let hash: String = ring::digest::digest(&ring::digest::SHA256, &bytes)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(json!(hash), expected[id], "preview mismatch for {id}");
    }
}

#[test]
fn shared_downloaded_record_refusals_match_the_instructor_tool() {
    let refusals: Value =
        serde_json::from_str(include_str!("../../fixtures/task-mode/refusals.json")).unwrap();
    for refusal in refusals.as_array().unwrap() {
        let mut row = fixture(refusal["fixture"].as_str().unwrap_or("customized"));
        let patches = refusal
            .get("patches")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| vec![refusal.clone()]);
        for patch in patches {
            let mut target = &mut row;
            for key in patch["path"].as_array().unwrap() {
                let key = key.as_str().unwrap();
                if target.is_array() {
                    target = &mut target[key.parse::<usize>().unwrap()];
                } else {
                    target = &mut target[key];
                }
            }
            *target = patch["value"].clone();
        }
        assert!(record(&row).is_err(), "accepted {}", refusal["name"]);
    }
}

#[test]
fn renamed_class_method_has_matching_starters_and_practice_operations() {
    let record = record(&fixture("class-renamed-method")).unwrap();
    let expected: Value = serde_json::from_str(include_str!(
        "../../golden/task-preview/class-renamed-method.json"
    ))
    .unwrap();
    assert_eq!(
        json!({"judge": record.judge, "starterCode": record.starters}),
        expected
    );
}

#[test]
fn task_ids_may_start_with_a_digit_but_not_a_dash() {
    assert!(crate::tasks::record_id("3sum"));
    assert!(crate::tasks::record_id("delimiter-closer"));
    for bad in ["", "-x", "Upper", "under_score", &"a".repeat(65)] {
        assert!(!crate::tasks::record_id(bad), "{bad}");
    }
}

#[test]
fn the_accessors_read_the_named_check_and_the_private_notes() {
    let mut data = fixture("customized");
    data["problem"]["referenceCode"] = json!("def is_valid(s):\n    return True\n");
    let exercise = Exercise(Arc::new(record(&data).unwrap()));
    assert_eq!(
        exercise.check_question("contract"),
        Some("What must remain true as the loop advances?")
    );
    assert_eq!(exercise.check_question("missing"), None);
    assert_eq!(
        exercise.reference_code(),
        Some("def is_valid(s):\n    return True\n")
    );
    let (optimal, pitfalls) = exercise.reference_notes();
    assert!(optimal.starts_with("Single pass with a stack"), "{optimal}");
    assert!(
        pitfalls.starts_with("Popping from an empty stack"),
        "{pitfalls}"
    );

    // A record without reference code says so rather than offering an empty
    // one.
    let plain = Exercise(Arc::new(record(&fixture("customized")).unwrap()));
    assert_eq!(plain.reference_code(), None);
}

/// Each of these breaks exactly one rule a completion target or a check must
/// keep, and the refusal names that rule. Breaking two at once would let a
/// check that stopped working hide behind the other.
#[test]
fn each_target_and_check_rule_refuses_on_its_own() {
    let base = fixture("customized");
    let target = |patch: &dyn Fn(&mut Value)| {
        let mut row = base.clone();
        patch(&mut row);
        record(&row).map(|_| ()).unwrap_err().0
    };
    let with_starter = |row: &mut Value, marker: &str| {
        row["problem"]["starterCode"]["python"] = json!(format!(
            "class Solution:\n    def isValid(self, s: str) -> bool:\n        # TASK_COMPLETE_CLOSER\n        # {marker}\n        return True\n"
        ));
    };
    let second = |row: &mut Value, id: &str, marker: &str| {
        let mut extra = row["sidecar"]["completionTargets"][0].clone();
        extra["id"] = json!(id);
        extra["marker"] = json!(marker);
        row["sidecar"]["completionTargets"]
            .as_array_mut()
            .unwrap()
            .push(extra);
    };
    let marker_error = "missing, invalid or duplicate completion marker";
    let target_error = "invalid completion target";
    let check_error = "invalid understanding check";

    let long = format!("T{}", "A".repeat(128));
    assert_eq!(
        target(&|row| {
            with_starter(row, &long);
            second(row, "long", &long);
        }),
        marker_error
    );
    // One byte shorter is a marker like any other.
    let longest = format!("T{}", "A".repeat(127));
    let mut row = base.clone();
    with_starter(&mut row, &longest);
    second(&mut row, "longest", &longest);
    assert!(record(&row).is_ok());

    for bad in ["", "9LIVES", "TASK-X", "TASKx"] {
        assert_eq!(
            target(&|row| {
                with_starter(row, bad);
                second(row, "other", bad);
            }),
            marker_error,
            "{bad:?}"
        );
    }
    // The same marker under a second id: only the duplicate rule refuses it.
    assert_eq!(
        target(&|row| second(row, "again", "TASK_COMPLETE_CLOSER")),
        marker_error
    );
    // A marker the starter does not carry.
    assert_eq!(
        target(&|row| second(row, "absent", "TASK_ABSENT")),
        marker_error
    );

    assert_eq!(
        target(&|row| {
            with_starter(row, "TASK_OTHER");
            second(row, "closer-branch", "TASK_OTHER");
        }),
        target_error
    );
    for (field, value) in [
        ("id", json!("Bad Id")),
        ("language", json!("javascript")),
        ("goal", json!(" ")),
        ("goal", json!("x".repeat(1025))),
    ] {
        assert_eq!(
            target(&|row| row["sidecar"]["completionTargets"][0][field] = value.clone()),
            target_error,
            "{field}"
        );
    }

    for (index, field, value) in [
        (0, "id", json!("Bad Id")),
        (1, "id", json!("trace")),
        (2, "question", json!(" ")),
        (2, "question", json!("x".repeat(1025))),
    ] {
        assert_eq!(
            target(&|row| row["sidecar"]["understandingChecks"][index][field] = value.clone()),
            check_error,
            "{index} {field}"
        );
    }
}

#[test]
fn identifiers_start_with_a_letter_and_hold_only_lowercase_digits_and_dashes() {
    for good in ["a", "closer-branch", "trace2", &"a".repeat(64)] {
        assert!(crate::tasks::identifier(good), "{good}");
    }
    for bad in ["", "2trace", "-x", "a_b", "aB", "a b", &"a".repeat(65)] {
        assert!(!crate::tasks::identifier(bad), "{bad}");
    }
    // The message is what the page is shown, so it is the error's text.
    assert_eq!(
        TaskError("invalid task".to_owned()).to_string(),
        "invalid task"
    );
}

/// The bank's structural rules, one broken at a time and each named by its
/// refusal, as `each_target_and_check_rule_refuses_on_its_own` does for the
/// sidecar.
#[test]
fn each_bank_rule_refuses_on_its_own() {
    let refused = |name: &str, patch: &dyn Fn(&mut Value)| {
        let mut row = fixture(name);
        patch(&mut row);
        record(&row).map(|_| ()).err().map(|error| error.0)
    };
    let function = |patch: &dyn Fn(&mut Value)| refused("customized", patch);
    let class = |patch: &dyn Fn(&mut Value)| refused("class-renamed-method", patch);
    let text = Some("expected nonblank bank text".to_owned());
    let identifier = Some("invalid identifier".to_owned());
    let rename = Some("invalid variant entry rename".to_owned());
    let lengths = Some("invalid class case lengths".to_owned());

    assert_eq!(
        function(&|row| row["problem"]["summary"] = json!(" ")),
        text
    );
    assert_eq!(
        function(&|row| row["problem"]["referenceCode"] = json!("")),
        text
    );

    assert_eq!(
        function(&|row| row["judge"]["paramNames"] = json!(["1s"])),
        identifier
    );
    assert_eq!(
        function(&|row| row["judge"]["paramNames"] = json!(["s-x"])),
        identifier
    );
    assert_eq!(
        function(&|row| row["judge"]["paramNames"] = json!(["s_x"])),
        None
    );

    // An original problem carries neither an imported title nor examples.
    let origin = Some("invalid problem origin".to_owned());
    assert_eq!(
        function(&|row| row["problem"]["title"] = json!("Valid Parentheses")),
        origin
    );
    assert_eq!(
        function(&|row| row["problem"]["examples"] = json!([])),
        origin
    );

    // Only an argument type may be null, and only that.
    assert_eq!(
        function(&|row| row["judge"]["argTypes"] = json!([null, "string"])),
        None
    );
    assert_eq!(
        function(&|row| row["judge"]["argTypes"] = json!([" "])),
        text
    );
    assert_eq!(
        function(&|row| row["judge"]["paramTypes"] = json!([null])),
        text
    );

    assert_eq!(
        function(&|row| row["variant"]["className"] = json!("Audit")),
        rename
    );
    assert_eq!(
        function(&|row| row["judge"]["className"] = json!("Audit")),
        rename
    );
    assert_eq!(
        function(&|row| row["variant"]["entry"] = json!("IsVALID")),
        rename
    );
    assert_eq!(
        function(&|row| row["variant"]["entry"] = json!("DelimitersNest")),
        rename
    );
    assert_eq!(
        class(&|row| row["variant"]["className"] = json!("bidLedger")),
        rename
    );
    assert_eq!(
        class(&|row| row["variant"]["entry"] = json!("bidLedger")),
        rename
    );

    // A term is a plain word on both sides; a parameter may keep its
    // underscore.
    let term = Some("invalid term rename".to_owned());
    assert_eq!(
        function(&|row| row["variant"]["terms"] = json!({"pair_s": "links"})),
        term
    );
    assert_eq!(
        function(&|row| row["variant"]["terms"] = json!({"pairs": "link_s"})),
        term
    );
    assert_eq!(
        function(&|row| row["variant"]["parameters"] = json!({"s": "text_in"})),
        None
    );

    let cases = Some("invalid judge cases".to_owned());
    assert_eq!(function(&|row| row["judge"]["cases"] = json!([])), cases);
    assert_eq!(
        function(&|row| {
            let case = row["judge"]["cases"][0].clone();
            row["judge"]["cases"] = Value::Array(vec![case; 1001]);
        }),
        cases
    );

    assert_eq!(
        class(&|row| row["judge"]["cases"][0]["input"] = json!([[], []])),
        lengths
    );
    assert_eq!(
        class(&|row| {
            row["judge"]["cases"][0]["input"][1]
                .as_array_mut()
                .unwrap()
                .pop();
        }),
        lengths
    );
    assert_eq!(
        class(&|row| {
            row["judge"]["cases"][0]["expected"]
                .as_array_mut()
                .unwrap()
                .pop();
        }),
        lengths
    );
    assert_eq!(
        class(&|row| row["judge"]["cases"][0]["input"][0][0] = json!("MaxStack")),
        lengths
    );
}

#[test]
fn a_starter_may_fill_the_code_limit_and_no_more() {
    let limit = crate::tasks::session::MAX_CODE_BYTES;
    let base = fixture("customized");
    let raw = base["problem"]["starterCode"]["python"].as_str().unwrap();
    // The limit is the posed starter's, which the variant's renames lengthen.
    let posed = record(&base).unwrap().starters["python"].len() - raw.len();
    let padded = |size: usize| {
        let mut row = base.clone();
        let mut starter = raw.to_owned();
        starter.push('#');
        starter.push_str(&"x".repeat(size - posed - starter.len()));
        row["problem"]["starterCode"]["python"] = json!(starter);
        record(&row).map(|_| ()).map_err(|error| error.0)
    };
    assert_eq!(padded(limit), Ok(()));
    assert_eq!(
        padded(limit + 1),
        Err("starter exceeds the code byte limit".to_owned())
    );
}

#[test]
fn a_task_may_require_twenty_cases_and_no_more() {
    let required = |count: usize| {
        let mut row = fixture("customized");
        let case = row["judge"]["cases"][0].clone();
        let labels: Vec<String> = (0..count)
            .map(|index| format!("required {index}"))
            .collect();
        for label in &labels {
            let mut extra = case.clone();
            extra["label"] = json!(label);
            row["judge"]["cases"].as_array_mut().unwrap().push(extra);
        }
        row["sidecar"]["requiredCases"] = json!(labels);
        record(&row).map(|_| ()).map_err(|error| error.0)
    };
    assert_eq!(required(20), Ok(()));
    assert_eq!(required(21), Err("too many required cases".to_owned()));
}

#[test]
fn a_task_may_mark_eight_targets_and_must_ask_the_core_checks() {
    let targets = |count: usize| {
        let mut row = fixture("customized");
        let template = row["sidecar"]["completionTargets"][0].clone();
        let mut starter = row["problem"]["starterCode"]["python"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut list = vec![template.clone()];
        for index in 1..count {
            let marker = format!("TASK_EXTRA_{index}");
            starter.push_str(&format!("# {marker}\n"));
            let mut target = template.clone();
            target["id"] = json!(format!("extra-{index}"));
            target["marker"] = json!(marker);
            list.push(target);
        }
        row["problem"]["starterCode"]["python"] = json!(starter);
        row["sidecar"]["completionTargets"] = json!(list);
        record(&row).map(|_| ()).map_err(|error| error.0)
    };
    let count_error = Err("invalid target or check count".to_owned());
    assert_eq!(targets(8), Ok(()));
    assert_eq!(targets(9), count_error);

    let mut row = fixture("customized");
    row["sidecar"]["understandingChecks"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert_eq!(
        record(&row).map(|_| ()).map_err(|error| error.0),
        count_error
    );
}

/// The reference task the instructor tool starts banks from is one the
/// learner's CodeTrial accepts.
#[test]
fn the_shipped_example_task_is_a_valid_record() {
    let row: Value = serde_json::from_str(include_str!(
        "../../../problem-bank/task-examples/delimiter-closer.json"
    ))
    .unwrap();
    let exercise = Exercise(Arc::new(record(&row).unwrap()));
    assert_eq!(exercise.completion_targets().len(), 1);
    assert!(exercise.task_prompt(Phase::Ready, None, "").is_ok());
}
