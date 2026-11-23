//! What a run and the result keep: the test output a learner reads, the
//! logs that can be compared, the stamp's remote and the size of the
//! defense section.

use serde_json::{Value, json};

use super::*;

/// What the review is told of the panel's runs, and whether one said
/// anything: a defense's testing is rated on these, not on session runs. A
/// gap's reason, which can be the student's own output, is not told.
#[test]
fn the_review_is_told_what_each_run_showed() {
    let facts = json!({"items": [
        {"id": "q1", "runs": [
            {"prediction": {"fails": ["a.ASpec"], "compiles": true}, "outcome": {"kind": "tests", "failed": ["a.ASpec"]},
             "matched": true, "tail": "never sent"},
            {"prediction": {"fails": [], "compiles": false}, "outcome": {"kind": "telemetryGap", "reason": "timeout"},
             "matched": null}]},
        {"id": "q2", "runs": []}]});
    assert!(record::ran(&facts));
    assert_eq!(
        record::run_summary(&facts),
        json!([
            {"item": "q1", "runs": [
                {"predicted": ["a.ASpec"], "predictedCompiles": true,
                 "outcome": {"kind": "tests", "failed": ["a.ASpec"]}, "matched": true},
                {"predicted": [], "predictedCompiles": false,
                 "outcome": {"kind": "telemetryGap"}, "matched": null}]},
            {"item": "q2", "runs": []}])
    );
    let gaps_only = json!({"items": [{"id": "q1", "runs": [{"matched": null}]}]});
    assert!(
        !record::ran(&gaps_only),
        "a run that said nothing is no evidence"
    );
    assert!(!record::ran(&json!({"items": []})));
}

/// What the page shows of a run: ScalaTest's own lines, not sbt's.
#[test]
fn a_run_summary_keeps_the_test_lines_and_drops_the_rest() {
    let tail = "[info] welcome to sbt 1.13.1\n[info] loading project definition from /tmp/w/project\n[info] compiling 3 Scala sources to /tmp/w/target\n[info] HazardSpec:\n[info] - stalls a load-use pair *** FAILED ***\n[info]   1 was not equal to 0 (HazardSpec.scala:42)\n[info] - forwards from EX *** FAILED ***\n[info] Run completed in 3 seconds.\n[info] Tests: succeeded 0, failed 2, canceled 0, ignored 0, pending 0\n[info] *** 2 TESTS FAILED ***\n[error] Failed tests:\n[error] \tmycpu.pipeline.HazardSpec\n[error] Total time: 19 s\n";
    let summary = summary_of(tail);
    assert_eq!(
        summary,
        "[info] HazardSpec:\n[info] - stalls a load-use pair *** FAILED ***\n[info]   1 was not equal to 0 (HazardSpec.scala:42)\n[info] - forwards from EX *** FAILED ***\n[info] Tests: succeeded 0, failed 2, canceled 0, ignored 0, pending 0\n[info] *** 2 TESTS FAILED ***\n"
    );
    let compile = "[info] compiling 3 Scala sources\n[error] /tmp/w/4-pipeline/src/main/scala/mycpu/pipeline/Hazard.scala:77:5: type mismatch;\n[error]  found   : chisel3.Bool\n[error]  required: chisel3.UInt\n[error]     val x: UInt = a && b\n[error]                     ^\n[error] one error found\n[error] (pipeline / Compile / compileIncremental) Compilation failed\n";
    assert_eq!(
        summary_of(compile).lines().count(),
        6,
        "a compile error keeps the lines that explain it"
    );
    // Lines indented under a line that opens no details are not kept.
    assert_eq!(
        summary_of("[info] HazardSpec:\n[info]   + a note\n"),
        "[info] HazardSpec:\n"
    );
    let long: String = (0..500).map(|n| format!("[info] - case {n}\n")).collect();
    let bounded = summary_of(&long);
    assert!(bounded.len() <= 4096 && bounded.lines().count() <= 40);
    assert!(
        bounded.ends_with("[info] - case 499\n"),
        "the end of the run is kept"
    );
    // Forty wide lines are over 4 KiB: the earliest whole lines go.
    let wide: String = (0..40)
        .map(|n| format!("[info] - case {n:03} {}\n", "x".repeat(180)))
        .collect();
    let capped = summary_of(&wide);
    assert!(capped.len() <= 4096, "{}", capped.len());
    assert!(capped.ends_with(&format!("[info] - case 039 {}\n", "x".repeat(180))));
    assert!(
        capped.lines().all(|line| line.len() == 198),
        "no line is cut"
    );
    assert!(capped.lines().count() < 40);
}

#[test]
fn only_a_log_that_ended_by_itself_is_compared() {
    for how in ["exit=0", "exit=3", "halt=2", "stop=assert", "stop=finish"] {
        let log = format!("pc=00000000 a\nend cycles=1 instret=1 {how}\n");
        assert!(obs::ended_by_itself(&log), "{how}");
    }
    for log in [
        "pc=00000000 a\nend cycles=9 instret=1 stop=timeout\n",
        "pc=00000000 a\nend cycles=9 instret=1 stop=signal\n",
        "pc=00000000 a\nend cycles=9 instret=1 stop=vga-window\n",
        "pc=00000000 a\npc=00000004 b\n",
        "",
    ] {
        assert!(!obs::ended_by_itself(log), "{log:?}");
    }
}

#[test]
fn a_remote_url_keeps_no_credentials() {
    for (url, kept) in [
        (
            "https://student:ghp_secret@github.com/s/ca2026-mycpu.git",
            "https://github.com/s/ca2026-mycpu.git",
        ),
        (
            "https://github.com/s/ca2026-mycpu.git?token=abc#x",
            "https://github.com/s/ca2026-mycpu.git",
        ),
        ("https://user@host", "https://host"),
        (
            "git@github.com:s/ca2026-mycpu.git",
            "github.com:s/ca2026-mycpu.git",
        ),
        ("student:token@host:s/mycpu.git", "host:s/mycpu.git"),
        ("./relative/dir:with-colon", "./relative/dir:with-colon"),
        ("/srv/git/mycpu", "/srv/git/mycpu"),
    ] {
        assert_eq!(checkout::without_credentials(url), kept);
    }
}

/// An attempt that uses every run of every item, each with the longest
/// prediction and every spec of the largest task, still leaves a 64 KiB
/// result 24 KiB for its review and stamp; a defense has no code of its own.
#[test]
fn a_full_attempt_fits_the_result() {
    let specs: Vec<String> = (0..19)
        .map(|n| format!("mycpu.csrtrap.SomeLongSpecName{n:02}Spec"))
        .collect();
    let how = "é".repeat(512);
    let run = |n: usize| {
        json!({"item": "q", "startedAt": 100 + n, "endedAt": 200 + n, "edited": "x".repeat(200),
            "specs": specs, "prediction": {"at": 90 + n, "fails": specs, "compiles": true, "how": how},
            "outcome": {"kind": "tests", "failed": specs}, "matched": (n > 0).then_some(false),
            "conclusive": false})
    };
    let items: Vec<Value> = (0..5)
        .map(|item| {
            json!({"id": format!("q{item}"), "file": "3-csr-trap/src/main/scala/mycpu/csrtrap/CPU.scala",
                "line": 100, "quote": "q".repeat(select::MAX_LINE_BYTES), "exercise": "3.1", "change": "`&&` becomes `||`",
                "changedLine": "c".repeat(select::MAX_LINE_BYTES),
                "runs": (0..attempt::MAX_RUNS_PER_ITEM).map(run).collect::<Vec<_>>(),
                "pendingPrediction": {"at": 1, "fails": specs, "compiles": true, "how": how}})
        })
        .collect();
    let facts = json!({"stamp": {}, "items": items, "bug": {"checks": null}, "scored": true});
    let explanations: Vec<Value> = (0..5)
        .map(|item| {
            json!({"item": format!("q{item}"), "rating": 3, "account": 2,
            "reason": "r".repeat(400), "evidence": [{"kind": "turn", "id": "turn-1"}]})
        })
        .collect();
    let section = record::marked(
        facts,
        &json!(explanations),
        &record::Hints::default(),
        false,
        true,
    );
    let bytes = section.to_string().len();
    assert!(
        bytes <= 40 * 1024,
        "the defense section takes {bytes} bytes"
    );
    let runs = section["items"][0]["predict"]["runs"].as_array().unwrap();
    assert_eq!(runs.len(), attempt::MAX_RUNS_PER_ITEM);
    assert_eq!(
        runs[1]["prediction"]["how"], how,
        "the run that sets the mark is whole"
    );
    assert!(runs[0]["prediction"].is_null() && runs[2]["prediction"].is_null());
    assert_eq!(
        runs[2]["outcome"],
        json!({"kind": "tests", "failedCount": 19})
    );
}

#[test]
fn a_confirmed_prediction_suggests_the_top_mark_only_when_conclusive() {
    let run = |matched: Value, conclusive: bool, predicted_at: u64| {
        json!({"startedAt": 50, "prediction": {"at": predicted_at}, "matched": matched,
            "conclusive": conclusive})
    };
    let mark = |runs: &[Value], account: Value| record::change_mark(runs, &account, true);
    assert_eq!(mark(&[run(json!(true), true, 40)], json!(null)).0, json!(3));
    assert_eq!(
        mark(&[run(json!(true), false, 40)], json!(null)).0,
        json!(1),
        "one passing spec does not confirm that nothing fails"
    );
    assert_eq!(mark(&[run(json!(false), false, 40)], json!(3)).0, json!(2));
    assert_eq!(
        mark(&[run(json!(null), false, 40)], json!(null)).1,
        json!("telemetry_gap")
    );
    assert_eq!(mark(&[], json!(null)).1, json!("missing_run"));
    assert_eq!(
        mark(
            &[run(json!(true), true, 40), run(json!(null), false, 70)],
            json!(null)
        )
        .0,
        json!(3),
        "a later run that said nothing does not undo a confirmed one"
    );
    assert_eq!(
        mark(
            &[run(json!(false), false, 40), run(json!(true), true, 70)],
            json!(null)
        )
        .0,
        json!(1),
        "a prediction made after a run showed the failures confirms nothing"
    );
    let unconfirmed = [run(json!(false), false, 40)];
    assert_eq!(
        record::change_mark(&unconfirmed, &json!(null), false),
        (json!(null), json!("feedback_unavailable")),
        "an outage of the review is never a lower mark"
    );
}

#[test]
fn the_bug_is_marked_by_what_the_claim_checks() {
    // The marks read the claim's checks, kept beside what it stated.
    let bug = |checks: Value, built: bool, error: Value| json!({"checks": checks, "modelsBuilt": built, "buildError": error});
    let claim =
        |close: Value, matches: Value| json!({"cycleClose": close, "exerciseMatches": matches});
    let mark = |value: Value| record::bug_mark(&value);
    assert_eq!(mark(bug(json!(null), true, json!(null))).0, json!(0));
    assert_eq!(mark(bug(json!(null), false, json!(null))).0, json!(0));
    assert_eq!(
        mark(bug(json!(null), false, json!("the model did not build"))),
        (json!(null), json!("telemetry_gap"))
    );
    assert_eq!(
        mark(bug(claim(json!(false), json!(false)), true, json!(null))).0,
        json!(1)
    );
    assert_eq!(
        mark(bug(claim(json!(true), json!(false)), true, json!(null))).0,
        json!(2)
    );
    assert_eq!(
        mark(bug(claim(json!(null), json!(true)), true, json!(null))).0,
        json!(2)
    );
    assert_eq!(
        mark(bug(claim(json!(true), json!(true)), true, json!(null))).0,
        json!(3)
    );
}

/// A replacement connection is told what the last run of each item showed:
/// the specs that failed, that every spec passed, that the change did not
/// compile, or that the run said nothing.
#[test]
fn a_replacement_is_told_what_each_last_run_showed() {
    let item = |id: &str, outcome: Value| {
        json!({"id": id, "begunAt": 10, "pendingPrediction": null,
            "runs": [{"outcome": outcome}]})
    };
    let facts = json!({"items": [
        item("q1", json!({"kind": "tests", "failed": ["p.ASpec"]})),
        item("q2", json!({"kind": "tests", "failed": []})),
        item("q3", json!({"kind": "compileError"})),
        item("q4", json!({"kind": "telemetryGap", "reason": "timeout"}))]});
    let told = super::progress(&facts, &Hints::default());
    for said in [
        r#"q1 opened, 1 run(s), the last showing failed: ["p.ASpec"]"#,
        "q2 opened, 1 run(s), the last showing every spec it ran passing",
        "q3 opened, 1 run(s), the last showing the change did not compile",
        "q4 opened, 1 run(s), the last showing a telemetry gap",
    ] {
        assert!(told.contains(said), "{said:?} not in {told}");
    }
}
