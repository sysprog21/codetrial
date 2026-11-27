//! A scripted interview over the task `new` starts from, driven through the
//! same session the live room drives: the learner's page messages and the
//! interviewer's tool calls, in the order an interview at a technology company
//! would produce them. No model is involved, so this pins what the platform
//! does with such a conversation, not how a voice model words it.
use std::sync::Arc;

use serde_json::{Value, json};

use crate::tasks::Phase;
use crate::tasks::access::{Admission, Preparation, TaskService};
use crate::tasks::package::{LoadedSet, decode};
use crate::tasks::session::{
    Action, ActionKind, Cause, CheckState, RevisionMessage, RunMessage, Support, TaskSession,
    Trigger,
};
use crate::tasks::test_support::{SITE, assignment, preparation};

const EXAMPLE: &str = include_str!("../../../problem-bank/task-examples/rubik.json");

/// The set the instructor would build from the example, unencrypted.
fn set() -> Arc<LoadedSet> {
    let example: Value = serde_json::from_str(EXAMPLE).unwrap();
    let id = example["problem"]["id"].as_str().unwrap();
    let package = json!({
        "packageVersion": 1, "setId": "classroom", "setVersion": 7,
        "rules": {"durationMin": 15, "maxHintRungs": 3, "closesAt": null,
            "lookAwaySeconds": 8, "rulesNote": null},
        "problems": [example["problem"]],
        "judges": {id: example["judge"]},
        "variants": {id: example["variant"]},
        "sidecars": {id: example["sidecar"]},
    });
    Arc::new(decode("classroom", 7, &serde_json::to_vec(&package).unwrap()).unwrap())
}

#[expect(
    clippy::panic_in_result_fn,
    reason = "a test helper: an unexpected result is the test failing"
)]
fn admit(language: &str, compiler_explorer: bool) -> Result<TaskSession, &'static str> {
    let service = TaskService::new();
    service.insert_unlocked(1, SITE, set());
    let admission = service
        .begin_admission(
            1,
            ("learner", 42),
            &assignment("classroom", 7),
            "rubik",
            "interview",
            Preparation {
                language: language.to_owned(),
                compiler_explorer,
                ..preparation()
            },
            100,
        )
        .map_err(|error| error.code)?;
    match admission {
        Admission::New(task) => Ok(TaskSession::new(task, 100)),
        Admission::Replayed(_) => panic!("unexpected replay"),
    }
}

fn action(id: &str, action: ActionKind, revision: Option<&str>, target: Option<&str>) -> Action {
    Action {
        version: 1,
        request_id: id.to_owned(),
        action,
        revision_id: revision.map(str::to_owned),
        target_id: target.map(str::to_owned),
    }
}

fn capture(id: &str, revision: &str, code: &str) -> RevisionMessage {
    RevisionMessage {
        version: 1,
        request_id: id.to_owned(),
        revision_id: revision.to_owned(),
        code: code.to_owned(),
        trigger: Trigger::Run,
    }
}

fn run(id: &str, revision: &str, passed: u32, total: u32, diagnostics: &str) -> RunMessage {
    RunMessage {
        version: 1,
        request_id: id.to_owned(),
        run_id: id.to_owned(),
        revision_id: revision.to_owned(),
        passed,
        total,
        diagnostics: diagnostics.to_owned(),
    }
}

/// The interviewer records a check once a learner turn answered it.
fn check(session: &mut TaskSession, id: &str, turn: &str) {
    let recorded = session.answer_tool(
        "record_task_check",
        &json!({"checkId": id, "state": "covered", "revisionId": session.current.id, "turnId": turn}),
    );
    assert_eq!(recorded, json!({"accepted": true}), "check {id}");
}

/// The learner discussing a marked stage, as the page's button sends it.
fn discuss(session: &mut TaskSession, id: &str, target: &str, at: u64) {
    let revision = session.current.id.clone();
    session
        .action(
            action(id, ActionKind::Discuss, Some(&revision), Some(target)),
            at,
        )
        .unwrap();
    assert_eq!(session.active_target_id.as_deref(), Some(target));
}

/// The learner's edit, acknowledged, captured and run against every case.
fn edit_and_run(session: &mut TaskSession, revision: &str, code: &str, passed: u32, at: u64) {
    let total = session.task.exercise.0.judge()["cases"]
        .as_array()
        .unwrap()
        .len() as u32;
    session.acknowledge_edit(revision, code, at).unwrap();
    session
        .revision(capture(&format!("cap-{revision}"), revision, code), at + 1)
        .unwrap();
    assert!(
        session
            .run(
                run(&format!("run-{revision}"), revision, passed, total, "run"),
                at + 2
            )
            .unwrap()
    );
}

#[test]
fn the_default_task_loads_in_a_set_and_names_what_to_optimize() {
    let set = set();
    let record = &set.records["rubik"];
    assert_eq!(record.sidecar().languages, ["c"]);
    let cases = record.judge()["cases"].as_array().unwrap();
    // Required cases are bound by exact label to indexes into the judge's.
    assert_eq!(record.required_cases().len(), 4);
    for &index in record.required_cases() {
        assert!(index < cases.len());
    }
    let targets: Vec<_> = record
        .sidecar()
        .completion_targets
        .iter()
        .map(|target| target.id.as_str())
        .collect();
    assert_eq!(
        targets,
        [
            "table-reuse",
            "rank-cost",
            "queue-size",
            "transition-tables"
        ]
    );
}

/// The page is told the optimization level only when the task fixes one, and
/// the learner's view carries nothing that would answer the task.
#[test]
fn the_learners_view_names_the_optimization_the_task_fixes() {
    let service = TaskService::new();
    service.insert_unlocked(1, SITE, set());
    let view = service
        .load_task(1, &assignment("classroom", 7), "rubik")
        .unwrap();
    assert_eq!(view["optimization"], "O1");
    assert_eq!(view["languages"], json!(["c"]));
    let text = view.to_string();
    for private in ["unrank_state", "back[there]", "Optimizing before measuring"] {
        assert!(!text.contains(private), "the view leaks {private:?}");
    }
}

#[tokio::test]
async fn a_c_attempt_needs_compiler_explorer_and_the_task_offers_nothing_else() {
    assert_eq!(admit("c", false).err(), Some("language_unavailable"));
    assert!(admit("c", true).is_ok());
    assert_eq!(admit("python", true).err(), Some("language_unsupported"));
}

#[tokio::test]
async fn jim_is_told_the_task_and_nothing_that_would_answer_it() {
    let mut session = admit("c", true).unwrap();
    session.start(100);
    let prompt = session
        .task
        .prompt(
            Phase::Work,
            Some("transition-tables"),
            &session.current.code,
        )
        .unwrap();
    assert!(prompt.contains("Cube Solve Planner"));
    assert!(prompt.contains("minimumTurns"));
    assert!(prompt.contains("TASK_OPTIMIZE_TABLES"));

    // The projection is an allowlist: the reference, the optimal approach and
    // the pitfalls ship for the review's overlap check and are never his.
    for private in [
        "unrank_state",
        "back[there]",
        "Optimizing before measuring",
        "scanning table levels",
        "transition tables over the 5040",
    ] {
        assert!(!prompt.contains(private), "the prompt leaks {private:?}");
    }
    let editor = session.answer_tool("read_editor", &json!({}));
    assert_eq!(editor["untrusted"], true);
    assert!(editor["code"].as_str().unwrap().contains("build_table"));
}

/// The interview as it would go: the starter is correct and too slow, so the
/// learner reads it, predicts, measures, takes a hint, changes one thing at a
/// time and ends with every case passing in time, and can say why.
#[tokio::test]
async fn a_full_interview_takes_the_slow_solver_to_one_that_finishes_in_time() {
    let mut session = admit("c", true).unwrap();
    session.start(100);
    assert_eq!(session.phase, Phase::Work);
    let starter = session.current.code.clone();

    session.observe_turn(
        "t1",
        "The service needs the fewest turns to solve a position, from a table built once. The starter is correct but the run times out.",
        110,
    );
    session.observe_turn(
        "t2",
        "For two cubies swapped, the table walks home in eleven lookups, so the answer is eleven.",
        120,
    );
    check(&mut session, "trace", "t2");

    // Measure before changing anything: the starter, run as it is, times out.
    let measured = format!("{starter}\n/* measured: build_table is the whole run */\n");
    session.acknowledge_edit("rev-1", &measured, 130).unwrap();
    session
        .revision(capture("cap-1", "rev-1", &measured), 131)
        .unwrap();
    let total = session.task.exercise.0.judge()["cases"]
        .as_array()
        .unwrap()
        .len() as u32;
    assert!(
        session
            .run(
                run(
                    "run-1",
                    "rev-1",
                    0,
                    total,
                    "Compiler Explorer timed out before the run completed."
                ),
                132
            )
            .unwrap()
    );

    // Stuck on where the time goes: one hint rung, linked only to evidence
    // given after it.
    discuss(&mut session, "discuss-1", "table-reuse", 140);
    let hint = session
        .action(action("hint-1", ActionKind::Hint, None, None), 145)
        .unwrap()
        .expect("a hint is released");
    assert!(hint.contains("hint rung 1"));
    assert_eq!(session.hint_rungs_used, 1);
    // A whole-task hint clears the target the learner had named.
    assert_eq!(session.active_target_id, None);
    session.observe_turn(
        "t3",
        "It builds the three point six million position table on every call, twelve times for twelve cases, so the table has to outlive the call.",
        150,
    );
    session
        .record_check_with_support("contract", "covered", "rev-1", "t3", Some("hint-1"))
        .unwrap();
    assert_eq!(session.checks["contract"].support, Support::ConceptualHint);
    assert_eq!(session.checks["trace"].support, Support::None);

    // The first change that matters: build it once. Every case now passes.
    let reused = measured
        .replace(
            "unsigned char *step = build_table();",
            "static unsigned char *step;\n    if (!step)\n        step = build_table();",
        )
        .replace("    free(step);\n", "");
    edit_and_run(&mut session, "rev-2", &reused, total, 160);

    // Then the cost inside one build: ranks instead of whole positions.
    discuss(&mut session, "discuss-2", "queue-size", 180);
    session.observe_turn(
        "t4",
        "The queue holds fourteen bytes per position, about fifty megabytes, against a three and a half megabyte table. Queuing the thirty-two bit rank cuts it to fifteen.",
        190,
    );
    let leaner = reused.replace(
        "state_t *queue = malloc(sizeof(*queue) * N);",
        "uint32_t *queue = malloc(sizeof(*queue) * N);",
    );
    edit_and_run(&mut session, "rev-3", &leaner, total, 200);

    // A targeted follow-up is bounded server evidence and counts as support.
    let followup =
        session.answer_tool("request_targeted_followup", &json!({"checkId": "revision"}));
    let request = followup["supportRequestId"].as_str().unwrap().to_owned();
    session.observe_turn(
        "t5",
        "I changed one thing per run, and kept every case and the table byte for byte the same before and after.",
        210,
    );
    session
        .record_check_with_support("revision", "covered", "rev-3", "t5", Some(&request))
        .unwrap();
    assert_eq!(
        session.checks["revision"].support,
        Support::TargetedFollowup
    );

    // Finish names the acknowledged revision and freezes it.
    session
        .action(
            action("finish", ActionKind::Finish, Some("rev-3"), None),
            220,
        )
        .unwrap();
    assert_eq!(session.phase, Phase::WrapUp);
    assert_eq!(session.final_revision_id.as_deref(), Some("rev-3"));
    assert!(
        session
            .checks
            .values()
            .all(|check| check.state == CheckState::Covered)
    );

    session.feedback(230);
    assert_eq!(session.phase, Phase::Feedback);
    assert_eq!(session.outcome.as_ref().unwrap().cause, Cause::Finish);
    let result = crate::tasks::report::ended(&session, "room-1");
    assert_eq!(result["assessmentMode"], "task");
    assert_eq!(result["taskAssessment"]["finalRevisionId"], "rev-3");
    assert!(
        result["taskAssessment"]["finalCode"]
            .as_str()
            .unwrap()
            .contains("uint32_t *queue")
    );
    // Every step the learner took is evidence the review can cite.
    for revision in ["initial", "rev-1", "rev-2", "rev-3"] {
        assert!(session.revisions.contains_key(revision), "{revision}");
    }
}

/// A learner who goes quiet and never changes the starter: the idle
/// nudge fires once, hints stop at the limit, and the work as it stands is
/// what the timed ending keeps.
#[tokio::test]
async fn a_stalled_interview_is_nudged_and_hints_stop_at_the_limit() {
    let mut session = admit("c", true).unwrap();
    session.start(100);
    session.observe_turn("t1", "I am not sure how to count the smaller ones.", 110);
    for index in 0..4 {
        session
            .action(
                action(&format!("hint-{index}"), ActionKind::Hint, None, None),
                120,
            )
            .unwrap();
    }
    assert_eq!(session.hint_rungs_used, 3);
    // The learner was silent past the idle window: one nudge, not a stream.
    assert!(session.tick(220, false).is_some());
    assert!(session.tick(221, false).is_none());
    // Time runs out on the starter: the ending is a timeout, not a completion.
    session.tick(2000, false);
    assert_eq!(session.phase, Phase::Feedback);
    assert!(session.final_revision_id.is_some());
    assert_eq!(session.outcome.as_ref().unwrap().cause, Cause::Timeout);
    assert!(
        session
            .checks
            .values()
            .all(|check| check.state == CheckState::Open)
    );
}
