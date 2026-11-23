use std::path::Path;

use serde_json::{Value, json};

use super::*;

pub(super) const SALT: &[u8] = b"0123456789abcdef";

/// Each attempt draws its bug afresh, so the same student and commit do
/// not always get the same one, and the result names the one drawn.
#[test]
fn each_attempt_draws_its_own_bug() {
    let repo = student_repo("draw");
    let checkout = Checkout::open(repo.path()).unwrap();
    let mut eight = config();
    eight["bugs"] = Value::Array(
        (0..8)
            .map(|n| {
                json!({"task": "4-pipeline", "variant": null, "mutant": format!("m{n}"),
                    "rule": "a rule", "exercise": null})
            })
            .collect(),
    );
    let config = Config::parse(&eight).unwrap();
    let drawn: std::collections::BTreeSet<usize> = (0..40)
        .map(|_| {
            let plan = plan(&config, &checkout).unwrap();
            assert_eq!(plan.bug.mutant, format!("m{}", plan.bug_index));
            plan.bug_index
        })
        .collect();
    assert!(drawn.iter().all(|&index| index < 8));
    // Forty draws of eight all landing on one is 8^-39: a fixed draw.
    assert!(drawn.len() > 1, "every attempt got bug {drawn:?}");
}

pub(super) fn config() -> serde_json::Value {
    json!({
        "course": "ca2026-mycpu",
        "salt": "MDEyMzQ1Njc4OWFiY2RlZg==",
        "lines": 3,
        "tasks": [{"dir": "4-pipeline", "project": "pipeline", "arch": "rv32im_zicsr"}],
        "bugs": [{"task": "4-pipeline", "variant": "Forward", "mutant": "no-flush",
                  "rule": "4.4: a taken branch flushes the younger instructions", "exercise": "4.4"}],
        "specSeconds": 900,
        "modelSeconds": 1200,
        "simSeconds": 120
    })
}

#[test]
fn the_package_object_is_closed_and_bounded() {
    let parsed = Config::parse(&config()).unwrap();
    assert_eq!(parsed.salt_bytes().unwrap(), SALT);
    let refused = |key: &str, value: serde_json::Value| {
        let mut bad = config();
        bad[key] = value;
        Config::parse(&bad).unwrap_err()
    };
    refused("course", json!("other"));
    refused("salt", json!("c2hvcnQ="));
    refused("lines", json!(0));
    refused("simSeconds", json!(1));
    refused("extra", json!(1));
    refused(
        "tasks",
        json!([{"dir": "../x", "project": "p", "arch": "a"}]),
    );
    let mut bad = config();
    bad["bugs"][0]["task"] = json!("2-rv32im");
    Config::parse(&bad).unwrap_err();
    bad = config();
    bad["bugs"][0]["mutant"] = json!("a b");
    Config::parse(&bad).unwrap_err();
}

/// The cases `scripts/problem_bank/task_package.py` is held to as well, so the
/// tool that builds a package and the program that opens it cannot disagree.
#[test]
fn the_defense_object_follows_the_shared_cases() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../fixtures/task-mode/defense-cases.json")).unwrap();
    assert!(cases.len() > 20);
    for case in cases {
        assert_eq!(
            Config::parse(&case["defense"]).is_ok(),
            case["valid"] == true,
            "{}",
            case["name"]
        );
    }
}

pub(super) use crate::tasks::test_support::Repo;

pub(super) const CPU: &str = "4-pipeline/src/main/scala/mycpu/pipeline/CPU.scala";
pub(super) const HAZARD: &str = "4-pipeline/src/main/scala/mycpu/pipeline/Hazard.scala";

/// Upstream with blanks, then the student's commits that fill them.
pub(super) fn student_repo(label: &str) -> Repo {
    let repo = Repo::new(label);
    repo.write(
        CPU,
        "class CPU {\n  // [CA26: Exercise 4.4] flush\n  ???\n}\n",
    );
    repo.write(
        HAZARD,
        "class Hazard {\n  // [CA26: Exercise 4.2] stall\n  ???\n}\n",
    );
    repo.write("lab-notes.txt", "lab\n");
    let upstream = repo.commit("upstream");
    repo.git(&["update-ref", "refs/remotes/upstream/main", &upstream]);
    repo.write(
        CPU,
        "class CPU {\n  // [CA26: Exercise 4.4] flush\n  io.flush := taken && valid\n  io.next := pc + 4.U\n}\n",
    );
    repo.write("STUDENT_ID", "F74121234\n");
    let first = repo.commit("cpu");
    // A push review asked about the next-PC line already.
    let note = json!({"questions": [{"file": CPU, "quote": "io.next := pc + 4.U"}]});
    repo.git(&[
        "notes",
        "--ref=refs/notes/mycpu-review",
        "add",
        "-m",
        &note.to_string(),
        &first,
    ]);
    repo.write(
        HAZARD,
        "class Hazard {\n  // [CA26: Exercise 4.2] stall\n  io.stall := ex.load && ex.rd === id.rs1\n}\n",
    );
    repo.commit("hazard");
    repo
}

#[test]
fn a_checkout_is_stamped_and_its_unasked_lines_are_the_candidates() {
    let repo = student_repo("stamp");
    let checkout = Checkout::open(repo.path()).unwrap();
    let stamp = checkout.stamp().unwrap();
    assert_eq!(stamp.student_id, "F74121234");
    assert!(!stamp.auditor);
    assert_eq!(stamp.head, repo.git(&["rev-parse", "HEAD"]));
    let candidates: Vec<String> = checkout
        .candidates(&stamp)
        .unwrap()
        .into_iter()
        .map(|c| c.text.trim().to_owned())
        .collect();
    assert_eq!(
        candidates,
        [
            "io.flush := taken && valid",
            "io.stall := ex.load && ex.rd === id.rs1"
        ]
    );

    let plan = plan(&Config::parse(&config()).unwrap(), &checkout).unwrap();
    assert_eq!(plan.lines.len(), 2, "two files, two lines");
    let flush = plan.lines.iter().find(|l| l.file == CPU).unwrap();
    assert_eq!(flush.line, 3);
    assert_eq!(flush.exercise.as_deref(), Some("4.4"));
    assert_eq!(
        flush.changed_line.as_deref(),
        Some("io.flush := taken || valid")
    );
    assert!(edit_matches(flush, "   io.flush := taken || valid "));
    assert!(!edit_matches(flush, "io.flush := taken && valid"));
    assert_eq!(plan.bug.mutant, "no-flush");
}

/// A trace of two 16-bit fields over three cycles from cycle 40.
pub(super) fn trace(records: u64, trailer_records: u64) -> Vec<u8> {
    let header = json!({"first": 40, "record_bytes": 4, "fields": [
        {"name": "core.pc", "bits": 16, "offset": 0, "bytes": 2},
        {"name": "core.stall", "bits": 1, "offset": 2, "bytes": 1},
        {"name": "retire.valid", "bits": 1, "offset": 3, "bytes": 1}]});
    let mut data = header.to_string().into_bytes();
    data.push(b'\n');
    for cycle in 0..records {
        let pc = u16::try_from(0x100 + 4 * cycle).unwrap();
        data.extend(pc.to_le_bytes());
        data.extend([u8::from(cycle == 1), 1]);
    }
    let trailer =
        json!({"records": trailer_records, "end": {"cycles": 99, "instret": 80, "how": "exit=0"}})
            .to_string();
    data.extend(trailer.as_bytes());
    data.extend(u64::try_from(trailer.len()).unwrap().to_le_bytes());
    data.extend(b"OBSEND01");
    data
}

#[test]
fn a_trace_reads_into_the_table_of_its_window() {
    let table = read_trace(&trace(3, 3)).unwrap();
    assert_eq!(table.first, 40);
    assert_eq!(table.fields, ["core.pc", "core.stall", "retire.valid"]);
    assert_eq!(
        table.rows,
        [vec![0x100, 0, 1], vec![0x104, 1, 1], vec![0x108, 0, 1]]
    );
    assert_eq!(table.end["how"], "exit=0");

    read_trace(&trace(3, 2)).unwrap_err();
    let mut cut = trace(3, 3);
    cut.truncate(cut.len() - 3);
    read_trace(&cut).unwrap_err();
    read_trace(b"no header").unwrap_err();
}

#[test]
fn the_first_divergence_is_the_bug_models_cycle_of_the_first_differing_record() {
    let clean =
        "pc=00000000 a\nidle 2\npc=00000004 b\npc=00000008 c\nend cycles=5 instret=3 exit=0\n";
    let same = clean;
    let bug = "pc=00000000 a\nidle 2\npc=00000004 b\nidle 1\npc=0000000c x\nend cycles=6 instret=3 exit=1\n";
    assert_eq!(first_divergence(clean, same), None);
    assert_eq!(first_divergence(clean, bug), Some(5));
    let short = "pc=00000000 a\nend cycles=1 instret=1 exit=0\n";
    assert_eq!(first_divergence(clean, short), Some(3));
    assert_eq!(
        retire_window(bug, 3, 3),
        [
            (3, "pc=00000004 b".to_owned()),
            (5, "pc=0000000c x".to_owned())
        ]
    );
}

#[test]
fn an_sbt_run_says_which_specs_failed_or_that_none_ran() {
    let specs = [
        "mycpu.pipeline.HazardSpec".to_owned(),
        "mycpu.pipeline.CPUSpec".to_owned(),
    ];
    let failing = "[info] - stalls *** FAILED ***\n[info] Tests: succeeded 3, failed 1, canceled 0\n[info] *** 1 TEST FAILED ***\n[error] Failed tests:\n[error] \tmycpu.pipeline.HazardSpec\n[error] (pipeline / Test / testOnly) sbt.TestsFailedException: Tests unsuccessful\n";
    assert!(matches!(
        outcome_of(&Ended::Exited(1), failing, &specs),
        Outcome::Tests { failed } if failed == ["mycpu.pipeline.HazardSpec"]
    ));
    let passing = "[info] Tests: succeeded 4, failed 0\n[info] All tests passed.\n";
    assert!(matches!(
        outcome_of(&Ended::Exited(0), passing, &specs),
        Outcome::Tests { failed } if failed.is_empty()
    ));
    let compile = "[error] CPU.scala:3:5: type mismatch\n[error] (pipeline / Compile / compileIncremental) Compilation failed\n";
    assert!(matches!(
        outcome_of(&Ended::Exited(1), compile, &specs),
        Outcome::CompileError
    ));
    assert!(matches!(
        outcome_of(
            &Ended::Exited(1),
            "[error] dependency resolution failed",
            &specs
        ),
        Outcome::TelemetryGap { .. }
    ));
    for (ended, output) in [
        (Ended::Exited(0), "[info] project loaded"),
        (Ended::Exited(1), "[info] Tests: succeeded 3, failed 1"),
    ] {
        assert!(matches!(
            outcome_of(&ended, output, &specs),
            Outcome::TelemetryGap { .. }
        ));
    }
    // A failing exit that names none of the specs asked for says nothing.
    let other = "[info] Tests: succeeded 1, failed 1\n[error] Failed tests:\n[error] \tmycpu.pipeline.OtherSpec\n";
    assert!(matches!(
        outcome_of(&Ended::Exited(1), other, &specs),
        Outcome::TelemetryGap { reason } if reason == "unclassified sbt failure"
    ));
    assert!(matches!(
        outcome_of(&Ended::Timeout, failing, &specs),
        Outcome::TelemetryGap { .. }
    ));
}

/// Stand-ins for the course's tools: an `sbt` that fails HazardSpec when the
/// stall line no longer reads `&&`, and a `make` that builds a `Vsim` whose
/// bug model retires a different third instruction.
fn fake_tools(dir: &Path) -> Tools {
    let sbt = dir.join("sbt");
    std::fs::write(
        &sbt,
        "#!/bin/sh\n[ -f \"$(dirname \"$0\")/compile-error\" ] && { echo '[error] Hazard.scala: type mismatch'; echo '[error] Compilation failed'; exit 1; }\n[ -f build/sw/mycpu/rv32im_zicsr/fib.elf ] || { echo 'fib.elf not found: build the programs first'; exit 2; }\nif grep -q 'ex.load && ex.rd' 4-pipeline/src/main/scala/mycpu/pipeline/Hazard.scala || ! echo \"$2\" | grep -q HazardSpec; then\n  echo '[info] Tests: succeeded 2, failed 0'; echo '[info] All tests passed.'; exit 0\nfi\necho '[info] Tests: succeeded 1, failed 1'\necho '[error] Failed tests:'\nprintf '[error] \\tmycpu.pipeline.HazardSpec\\n'\nexit 1\n",
    )
    .unwrap();
    let vsim = r#"#!/usr/bin/env python3
import json, os, struct, sys
a = dict(x[1:].split("=", 1) for x in sys.argv[1:])
bug = os.path.dirname(os.path.abspath(sys.argv[0])).endswith("-no-flush")
third = "pc=0000000c x" if bug else "pc=00000008 c"
open(a["retire-log"], "w").write("pc=00000000 a\npc=00000004 b\nidle 3\n" + third + "\nend cycles=6 instret=3 exit=0\n")
first, count = int(a["obs-start"]), int(a["obs-cycles"])
n = max(0, min(count, 6 - first))
head = {"first": first, "record_bytes": 4, "fields": [{"name": "core.pc", "bits": 32, "offset": 0, "bytes": 4}]}
body = b"".join(struct.pack("<I", 4 * (first + i)) for i in range(n))
trailer = json.dumps({"records": n, "end": {"cycles": 6, "instret": 3, "how": "exit=0"}}).encode()
open(a["obs-trace"], "wb").write(json.dumps(head).encode() + b"\n" + body + trailer + struct.pack("<Q", len(trailer)) + b"OBSEND01")
"#;
    executable(&sbt);
    Tools {
        sbt: sbt.display().to_string(),
        make: fake_make(dir, vsim),
    }
}

/// A `make` that writes `vsim` as `build/t4-forward[-<mutant>]/Vsim`.
pub(super) fn fake_make(dir: &Path, vsim: &str) -> String {
    let make = dir.join("make");
    std::fs::write(
        &make,
        format!(
            "#!/bin/sh\nname=t4-forward\nfor a in \"$@\"; do case $a in MUTANT=*) name=$name-${{a#MUTANT=}};; esac; done\nmkdir -p build/$name\ncat > build/$name/Vsim <<'PY'\n{vsim}PY\nchmod +x build/$name/Vsim\n"
        ),
    )
    .unwrap();
    executable(&make);
    make.display().to_string()
}

pub(super) fn executable(tool: &Path) {
    let mut permissions = std::fs::metadata(tool).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(tool, permissions).unwrap();
}

#[cfg(unix)]
#[test]
fn an_attempt_records_predictions_runs_and_the_bug_claim() {
    let repo = student_repo("attempt");
    for spec in ["HazardSpec", "CPUSpec"] {
        repo.write(
            &format!("4-pipeline/src/test/scala/mycpu/pipeline/{spec}.scala"),
            &format!("package mycpu.pipeline\n\nclass {spec}\n"),
        );
    }
    repo.commit("specs");
    repo.write("build/sw/mycpu/rv32im_zicsr/fib.elf", "");
    let checkout = Checkout::open(repo.path()).unwrap();
    let config = Config::parse(&config()).unwrap();
    let plan = plan(&config, &checkout).unwrap();
    let tools = fake_tools(&repo.path().join("build"));
    let mut attempt = Attempt::new(config, checkout, plan, tools);
    let hazard = attempt
        .plan()
        .lines
        .iter()
        .find(|l| l.file == HAZARD)
        .unwrap()
        .clone();
    let changed = hazard.changed_line.clone().unwrap();
    let hazard_spec = "mycpu.pipeline.HazardSpec".to_owned();
    let both = [hazard_spec.clone(), "mycpu.pipeline.CPUSpec".to_owned()];
    assert_eq!(
        attempt.specs_for(&hazard.id).unwrap(),
        [both[1].clone(), both[0].clone()]
    );
    let clock = || 100;

    // Nothing acts on an item before its overview is read and it is opened.
    assert_eq!(
        attempt
            .run_specs(&hazard.id, &changed, &both, &clock)
            .unwrap_err(),
        "item_not_begun"
    );
    assert_eq!(
        attempt.predict(&hazard.id, vec![], "x", 95).unwrap_err(),
        "item_not_begun"
    );
    assert_eq!(
        attempt.predict_no_compile(&hazard.id, "x", 95).unwrap_err(),
        "item_not_begun"
    );
    assert_eq!(attempt.begin("q9", 90).unwrap_err(), "item_unknown");
    let view = panel_view(&attempt, &Hints::default(), true);
    assert_eq!(view["lines"][0]["begun"], false);
    assert_eq!(view["bugBegun"], false);
    assert!(attempt.begin(&hazard.id, 90).unwrap());
    let view = panel_view(&attempt, &Hints::default(), true);
    let opened = view["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|line| line["id"] == hazard.id.as_str())
        .unwrap();
    assert_eq!(opened["begun"], true);
    let kept = record::facts(&attempt);
    let kept_item = kept["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == hazard.id.as_str())
        .unwrap();
    assert_eq!(
        kept_item["begunAt"], 90,
        "the result says when it was opened"
    );
    assert!(kept["bug"]["begunAt"].is_null());
    assert!(!attempt.begin(&hazard.id, 91).unwrap(), "opened once");
    assert_eq!(attempt.begun_at(&hazard.id), Some(90));

    assert_eq!(
        attempt
            .run_specs(&hazard.id, &changed, &both, &clock)
            .unwrap_err(),
        "prediction_required"
    );
    assert_eq!(
        attempt
            .predict(&hazard.id, vec!["mycpu.Other".to_owned()], "x", 10)
            .unwrap_err(),
        "spec_unknown"
    );
    attempt
        .predict(
            &hazard.id,
            vec![hazard_spec.clone()],
            "the load-use stall goes",
            11,
        )
        .unwrap();
    assert_eq!(
        attempt.check_run(
            &hazard.id,
            &changed,
            &[hazard_spec.clone(), hazard_spec.clone()]
        ),
        Err("spec_unknown")
    );
    assert_eq!(
        attempt
            .run_specs(&hazard.id, &hazard.quote, &both, &clock)
            .unwrap_err(),
        "edit_mismatch"
    );
    let run = attempt
        .run_specs(&hazard.id, &changed, &both, &clock)
        .unwrap();
    assert!(
        matches!(&run.outcome, Outcome::Tests { failed } if *failed == std::slice::from_ref(&hazard_spec))
    );
    assert_eq!(run.matched, Some(true));
    assert_eq!(run.prediction.at, 11);
    assert_eq!(
        attempt
            .run_specs(&hazard.id, &changed, &both, &clock)
            .unwrap_err(),
        "prediction_required",
        "a prediction answers one run"
    );
    attempt
        .predict(&hazard.id, Vec::new(), "nothing fails", 12)
        .unwrap();
    let wrong = attempt
        .run_specs(&hazard.id, &changed, &both, &clock)
        .unwrap();
    assert_eq!(wrong.matched, Some(false));
    assert!(!wrong.conclusive);
    attempt
        .predict(&hazard.id, vec![hazard_spec], "the stall goes", 13)
        .unwrap();
    let cpu_only = [both[1].clone()];
    assert_eq!(
        attempt
            .run_specs(&hazard.id, &changed, &cpu_only, &clock)
            .unwrap_err(),
        "prediction_unrun",
        "a run must include every spec the prediction names"
    );
    attempt
        .predict(&hazard.id, Vec::new(), "nothing fails", 14)
        .unwrap();
    let partial = attempt
        .run_specs(&hazard.id, &changed, &cpu_only, &clock)
        .unwrap();
    assert_eq!(partial.matched, Some(true));
    assert!(!partial.conclusive, "one passing spec shows nothing");
    for at in 15..18 {
        attempt
            .predict(&hazard.id, Vec::new(), "nothing fails", at)
            .unwrap();
        attempt
            .run_specs(&hazard.id, &changed, &cpu_only, &clock)
            .unwrap();
    }
    attempt
        .predict(&hazard.id, Vec::new(), "nothing fails", 18)
        .unwrap();
    assert_eq!(
        attempt
            .run_specs(&hazard.id, &changed, &cpu_only, &clock)
            .unwrap_err(),
        "run_limit",
        "at most {MAX_RUNS_PER_ITEM} runs an item"
    );
    // The prediction that could not run is still in the record.
    assert_eq!(
        facts(&attempt)["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == hazard.id.as_str())
            .unwrap()["pendingPrediction"]["at"],
        18
    );
    assert_eq!(repo.git(&["status", "--porcelain", "--", "4-pipeline"]), "");

    // A change that breaks the build is a prediction like any other.
    let flush = attempt
        .plan()
        .lines
        .iter()
        .find(|l| l.file == CPU)
        .unwrap()
        .clone();
    let flushed = flush.changed_line.clone().unwrap();
    let marker = repo.path().join("build/compile-error");
    std::fs::write(&marker, "").unwrap();
    attempt.begin(&flush.id, 18).unwrap();
    attempt
        .predict(&flush.id, Vec::new(), "nothing fails", 19)
        .unwrap();
    let missed = attempt
        .run_specs(&flush.id, &flushed, &cpu_only, &clock)
        .unwrap();
    assert!(matches!(missed.outcome, Outcome::CompileError));
    assert_eq!(
        missed.matched,
        Some(false),
        "it predicted a build that works"
    );
    attempt
        .predict_no_compile(&flush.id, "the types no longer agree", 20)
        .unwrap();
    assert!(!attempt.prediction(&flush.id).unwrap().compiles);
    let broke = attempt
        .run_specs(&flush.id, &flushed, &cpu_only, &clock)
        .unwrap();
    assert_eq!(broke.matched, Some(true));
    assert!(broke.conclusive, "a build that fails as predicted shows it");
    std::fs::remove_file(&marker).unwrap();

    let brief = serde_json::to_string(&attempt.bug_brief()).unwrap();
    assert!(!brief.contains("no-flush") && !brief.contains("flushes"));
    assert_eq!(attempt.build_models(|| true).unwrap_err(), "item_not_begun");
    assert_eq!(
        attempt
            .simulate(Model::Bug, "fib.elf", 0, 8, 20)
            .unwrap_err(),
        "item_not_begun"
    );
    assert_eq!(
        attempt
            .claim_bug("fib.elf", 3, "s", "r", "4.4", 20)
            .unwrap_err(),
        "item_not_begun",
        "no claim before the bug item's overview is read"
    );
    assert!(attempt.begin("bug", 20).unwrap());
    assert_eq!(
        panel_view(&attempt, &Hints::default(), true)["bugBegun"],
        true
    );
    assert_eq!(record::facts(&attempt)["bug"]["begunAt"], 20);
    assert_eq!(
        attempt
            .simulate(Model::Bug, "fib.elf", 0, 8, 20)
            .unwrap_err(),
        "model_not_built"
    );

    // An attempt that ends during the builds starts no further one, and a later
    // build makes only the model still missing.
    let asked = std::cell::Cell::new(0);
    let stopped = attempt.build_models(|| {
        asked.set(asked.get() + 1);
        asked.get() == 1
    });
    assert_eq!(stopped.unwrap_err(), "defense_closed");
    assert!(!attempt.bug_brief().built, "one model of two");
    attempt.build_models(|| true).unwrap();
    assert!(attempt.bug_brief().built);
    assert_eq!(
        attempt
            .simulate(Model::Bug, "../fib.elf", 0, 8, 20)
            .unwrap_err(),
        "program_unknown"
    );
    let clean = attempt
        .simulate(Model::Clean, "fib.elf", 0, 1000, 21)
        .unwrap();
    assert_eq!(clean.count, MAX_WINDOW);
    assert_eq!(clean.retire.len(), 3);
    assert_eq!(
        attempt
            .claim_bug("fib.elf", 5, "s", "r", "4.4", 21)
            .unwrap_err(),
        "claim_unrun",
        "a claim needs the program run on both models"
    );
    let bug = attempt.simulate(Model::Bug, "fib.elf", 4, 2, 22).unwrap();
    assert_eq!(bug.retire, [(5, "pc=0000000c x".to_owned())]);
    assert_eq!(bug.table.as_ref().unwrap().rows, [vec![16], vec![20]]);

    assert_eq!(
        attempt
            .claim_bug("other.elf", 7, "s", "r", "4.4", 29)
            .unwrap_err(),
        "program_unknown"
    );
    // At the slack's edge: the first divergence is cycle 5.
    let claim = attempt
        .claim_bug(
            "fib.elf",
            5 + CYCLE_SLACK,
            "id.flush",
            "a taken branch flushes",
            "Exercise 4.4.",
            30,
        )
        .unwrap();
    assert_eq!(claim.first_divergence, Some(5));
    assert_eq!(claim.cycle_close, Some(true));
    assert_eq!(claim.exercise_matches, Some(true));
    assert_eq!(
        attempt
            .claim_bug("fib.elf", 40, "x", "y", "4.2", 31)
            .unwrap_err(),
        "claim_final",
        "a second claim could probe the first one's checks"
    );
    let shown = panel_view(&attempt, &Hints::default(), true).to_string();
    assert!(shown.contains("Exercise 4.4."));
    for hidden in [
        "firstDivergence",
        "cycleClose",
        "exerciseMatches",
        "no-flush",
        "flushes the younger",
    ] {
        assert!(!shown.contains(hidden), "the panel shows {hidden}");
    }

    // The first run of the hazard item confirmed, conclusively, the failure it
    // predicted: later runs, made after seeing it, change nothing. The other
    // item never ran. A claim that matches both checks earns 3.
    let explanations = json!([
        {"item": hazard.id, "rating": 3, "account": 3, "reason": "named the stall and its cycle", "evidence": []}
    ]);
    let section = record(&attempt, &explanations, &Hints::default(), true);
    let items = section["items"].as_array().unwrap();
    let ran = items
        .iter()
        .find(|i| i["id"] == hazard.id.as_str())
        .unwrap();
    assert_eq!(ran["explain"]["suggestedMark"], 3);
    assert_eq!(ran["predict"]["suggestedMark"], 3);
    assert_eq!(
        ran["predict"]["runs"].as_array().unwrap().len(),
        MAX_RUNS_PER_ITEM
    );
    let idle = items
        .iter()
        .find(|i| i["id"] != hazard.id.as_str())
        .unwrap();

    // Its first run that said anything did not confirm its prediction; the
    // review gave no account of it, so 1.
    assert_eq!(idle["predict"]["suggestedMark"], 1);
    assert_eq!(idle["explain"]["insufficientReason"], "missing_turns");
    assert_eq!(section["bug"]["suggestedMark"], 3);
    assert_eq!(section["bug"]["bugIndex"], 0);
    let text = section.to_string();
    assert!(!text.contains("no-flush") && !text.contains("flushes the younger"));
    // The student keeps the result, and their bug is the same next time.
    for hidden in ["firstDivergence", "cycleClose", "exerciseMatches"] {
        assert!(!text.contains(hidden), "the result shows {hidden}");
    }
    assert_eq!(section["bug"]["claim"]["exercise"], "Exercise 4.4.");
    assert_eq!(section["marksAreSuggestions"], true);
    assert_eq!(section["marksWithheld"], false);

    // No review came back: what rests on it is null and says why, never a lower
    // mark; what the record alone shows keeps its mark.
    let unreviewed = record(&attempt, &Value::Null, &Hints::default(), true);
    let ran = &unreviewed["items"][0];
    let ran = if ran["id"] == hazard.id.as_str() {
        ran
    } else {
        &unreviewed["items"][1]
    };
    assert_eq!(ran["explain"]["insufficientReason"], "feedback_unavailable");
    assert_eq!(ran["predict"]["suggestedMark"], 3);
    assert_eq!(unreviewed["bug"]["suggestedMark"], 3);

    // An invalid or interrupted attempt is a record with no ratings.
    let ended = record(&attempt, &explanations, &Hints::default(), false);
    assert_eq!(ended["marksWithheld"], true);
    assert_eq!(ended["bug"]["suggestedMark"], json!(null));
    assert!(ended["items"].as_array().unwrap().iter().all(|item| {
        item["explain"]["suggestedMark"].is_null() && item["predict"]["suggestedMark"].is_null()
    }));
}
