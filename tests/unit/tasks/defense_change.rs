//! The change each line item asks for, and the edges of an attempt's runs:
//! the bounds of what it reads, keeps and refuses.

use std::path::Path;
use std::time::Duration;

use serde_json::json;

use super::tests::{
    CPU, HAZARD, Repo, config, executable, fake_make, student_repo, trace as obs_trace,
};
use super::*;

/// What the course's `scripts/defense.py` `change_of` answers for each line.
#[test]
fn the_change_is_the_course_scripts_change() {
    let cases: [(&str, Option<&str>, &str); 16] = [
        (
            "when (a && b) { x := 1.U }",
            Some("when (a || b) { x := 1.U }"),
            "`&&` becomes `||`",
        ),
        (
            "io.valid := a || !b",
            Some("io.valid := a && !b"),
            "`||` becomes `&&`",
        ),
        (
            "val hit = tag =/= addr",
            Some("val hit = tag === addr"),
            "`=/=` becomes `===`",
        ),
        (
            "val eq = x === y",
            Some("val eq = x =/= y"),
            "`===` becomes `=/=`",
        ),
        (
            "when (count <= 3.U)",
            Some("when (count < 3.U)"),
            "`<=` becomes `<`",
        ),
        ("when (a < b)", Some("when (a <= b)"), "`<` becomes `<=`"),
        ("x := a << 2", None, "delete the line"),
        (
            "io.out := !io.in",
            Some("io.out := io.in"),
            "the `!` is removed",
        ),
        ("io.out := a != b", None, "delete the line"),
        (
            "val w = 31.U + width",
            Some("val w = 32.U + width"),
            "the constant 31.U becomes 32.U",
        ),
        ("val n = r12.U", None, "delete the line"),
        ("val s = a + b", Some("val s = a - b"), "`+` becomes `-`"),
        ("val y = Cat(a, b)", None, "delete the line"),
        (
            "state := 0.U(4.W)",
            Some("state := 1.U(4.W)"),
            "the constant 0.U becomes 1.U",
        ),
        ("when (x =< y)", None, "delete the line"),
        (
            "val x = 2.5 + y",
            Some("val x = 2.5 - y"),
            "`+` becomes `-`",
        ),
    ];
    for (line, changed, what) in cases {
        let change = change_of(line);
        assert_eq!(change.changed.as_deref(), changed, "{line}");
        assert_eq!(change.what, what, "{line}");
    }
}

fn attempt_over(repo: &Repo, config: Config, tools: Tools) -> Attempt {
    let checkout = Checkout::open(repo.path()).unwrap();
    let plan = plan(&config, &checkout).unwrap();
    Attempt::new(config, checkout, plan, tools)
}

/// A model that retires three instructions, the bug model's third differing.
/// A program that reads `long` pads the log with a few megabytes of marks;
/// one that reads `quiet` writes no log. Each run adds the paths of its log
/// and its program to `paths`.
fn vsim(paths: &Path) -> String {
    format!(
        r#"#!/bin/sh
for a; do case $a in +elf=*) elf=${{a#+elf=}};; +retire-log=*) log=${{a#+retire-log=}};; esac; done
printf '%s\n%s\n' "$log" "$elf" >> '{paths}'
what=$(cat "$elf")
[ "$what" = quiet ] && exit 0
case $0 in *-no-flush/Vsim) third='pc=0000000c x';; *) third='pc=00000008 c';; esac
{{ echo 'pc=00000000 a'; [ "$what" = long ] && yes 'mark padding' | head -n 200000; echo 'pc=00000004 b'; echo "$third"; echo 'end cycles=3 instret=3 exit=0'; }} > "$log"
"#,
        paths = paths.display()
    )
}

/// The tools of the bug item: no `sbt`, and a `make` building `vsim`.
fn bug_tools(repo: &Repo) -> Tools {
    let dir = repo.path().join("build");
    std::fs::create_dir_all(&dir).unwrap();
    Tools {
        sbt: "sbt".to_owned(),
        make: fake_make(&dir, &vsim(&dir.join("sim-paths"))),
    }
}

/// What the claim's exercise check says of `said` for the package's 4.4.
fn exercise_claimed(label: &str, said: &str) -> Option<bool> {
    let repo = student_repo(label);
    repo.write("build/sw/mycpu/rv32im_zicsr/fib.elf", "");
    let tools = bug_tools(&repo);
    let mut attempt = attempt_over(&repo, Config::parse(&config()).unwrap(), tools);
    attempt.begin("bug", 1).unwrap();
    attempt.build_models(|| true).unwrap();
    for model in [Model::Clean, Model::Bug] {
        attempt.simulate(model, "fib.elf", 0, 8, 2).unwrap();
    }
    let claim = attempt.claim_bug("fib.elf", 2, "s", "r", said, 3).unwrap();
    assert_eq!(claim.first_divergence, Some(2));
    claim.exercise_matches
}

/// The exercise a claim names is the first `<task>.<n>` standing as numbers
/// of their own, whole.
#[cfg(unix)]
#[test]
fn a_claims_exercise_is_the_first_whole_number_it_names() {
    let cases = [
        ("4.4", true),
        ("see .5, then 4.4", true),
        ("x14.4", false),
        ("4.45", false),
    ];
    for (n, (said, matches)) in cases.into_iter().enumerate() {
        assert_eq!(
            exercise_claimed(&format!("exercise-{n}"), said),
            Some(matches),
            "{said}"
        );
    }
}

/// A retire log of a few megabytes is read whole, a run that wrote none is
/// not called too long, and every run's files go when they are done with.
#[cfg(unix)]
#[test]
fn a_simulation_reads_its_whole_log_and_leaves_no_files_behind() {
    let repo = student_repo("sim-edges");
    for (name, what) in [
        ("fib.elf", ""),
        ("long.elf", "long"),
        ("quiet.elf", "quiet"),
    ] {
        repo.write(&format!("build/sw/mycpu/rv32im_zicsr/{name}"), what);
    }
    let tools = bug_tools(&repo);
    let mut attempt = attempt_over(&repo, Config::parse(&config()).unwrap(), tools);
    attempt.begin("bug", 1).unwrap();
    assert_eq!(
        attempt
            .simulate(Model::Clean, "other.elf", 0, 8, 1)
            .unwrap_err(),
        "program_unknown",
        "a plain name that was never built"
    );
    attempt.build_models(|| true).unwrap();

    let long = attempt.simulate(Model::Clean, "long.elf", 0, 8, 2).unwrap();
    assert_eq!(long.error, None);
    assert_eq!(long.retire.first(), Some(&(0, "pc=00000000 a".to_owned())));
    let quiet = attempt
        .simulate(Model::Clean, "quiet.elf", 0, 8, 3)
        .unwrap();
    assert_eq!(quiet.error, None, "no log is not a log too long");
    assert!(quiet.retire.is_empty());
    assert_eq!(
        attempt
            .sims()
            .iter()
            .map(|sim| sim.program.as_str())
            .collect::<Vec<_>>(),
        ["long.elf", "quiet.elf"]
    );

    let paths = std::fs::read_to_string(repo.path().join("build/sim-paths")).unwrap();
    let paths: Vec<&Path> = paths.lines().map(Path::new).collect();
    let (log, program) = (paths[0], paths[1]);
    assert!(!log.parent().unwrap().exists(), "a run's files go with it");
    assert!(program.exists(), "the programs stay while the attempt does");
    drop(attempt);
    assert!(!program.parent().unwrap().exists(), "and go with it");
}

/// A model is one this task's build made: another task's, however new, is
/// not taken for it.
#[cfg(unix)]
#[test]
fn a_model_is_only_one_its_task_built() {
    let repo = student_repo("stray-model");
    repo.write("build/sw/mycpu/rv32im_zicsr/fib.elf", "");
    let built = bug_tools(&repo).make;
    let make = repo.path().join("build/make-and-stray");
    std::fs::write(
        &make,
        format!(
            "#!/bin/sh\n'{built}' \"$@\" || exit\nmkdir -p build/t9-stale\nprintf '#!/bin/sh\\nexit 7\\n' > build/t9-stale/Vsim\nchmod +x build/t9-stale/Vsim\ntouch -d tomorrow build/t9-stale/Vsim\n"
        ),
    )
    .unwrap();
    executable(&make);
    let tools = Tools {
        sbt: "sbt".to_owned(),
        make: make.display().to_string(),
    };
    let mut attempt = attempt_over(&repo, Config::parse(&config()).unwrap(), tools);
    attempt.begin("bug", 1).unwrap();
    attempt.build_models(|| true).unwrap();
    let clean = attempt.simulate(Model::Clean, "fib.elf", 0, 8, 2).unwrap();
    assert_eq!(clean.exit, Some(0));
    assert_eq!(clean.retire.len(), 3);
}

/// A model build that could not start, or ran out of time, says which.
#[cfg(unix)]
#[test]
fn a_model_build_that_cannot_finish_says_why() {
    let repo = student_repo("model-error");
    let dir = repo.path().join("build");
    std::fs::create_dir_all(&dir).unwrap();
    let missing = Tools {
        sbt: "sbt".to_owned(),
        make: dir.join("no-such-make").display().to_string(),
    };
    let mut attempt = attempt_over(&repo, Config::parse(&config()).unwrap(), missing);
    attempt.begin("bug", 1).unwrap();
    assert_eq!(
        attempt.build_models(|| true).unwrap_err(),
        "model_build_failed"
    );
    let why = attempt.bug_brief().build_error.unwrap();
    assert!(why.ends_with("not found"), "{why}");
    drop(attempt);

    let slow = dir.join("slow-make");
    std::fs::write(&slow, "#!/bin/sh\nexec sleep 30\n").unwrap();
    executable(&slow);
    let tools = Tools {
        sbt: "sbt".to_owned(),
        make: slow.display().to_string(),
    };
    let mut no_time = Config::parse(&config()).unwrap();
    no_time.model_seconds = 0;
    let mut attempt = attempt_over(&repo, no_time, tools);
    attempt.begin("bug", 1).unwrap();
    assert_eq!(
        attempt.build_models(|| true).unwrap_err(),
        "model_build_failed"
    );
    assert_eq!(
        attempt.bug_brief().build_error.as_deref(),
        Some("the model build timed out")
    );
}

/// The student's line is run at the original's indentation, with every spec
/// it may name, and the build reads the student's `externals/`.
#[cfg(unix)]
#[test]
fn a_line_runs_at_its_indentation_with_the_students_externals() {
    let repo = student_repo("indent");
    for spec in ["HazardSpec", "CPUSpec"] {
        repo.write(
            &format!("4-pipeline/src/test/scala/mycpu/pipeline/{spec}.scala"),
            &format!("package mycpu.pipeline\n\nclass {spec}\n"),
        );
    }
    repo.commit("specs");
    let dir = repo.path().join("build");
    std::fs::create_dir_all(&dir).unwrap();
    let sbt = dir.join("sbt");
    std::fs::write(
        &sbt,
        format!(
            "#!/bin/sh\nprintf '[info] - edited:%s|\\n' \"$(sed -n 3p {HAZARD})\"\nprintf '[info] - env:%s;%s;%s\\n' \"$EXTERNALS\" \"$BOOTCAMP_DIR\" \"${{BOOTCAMP_SRC-unset}}\"\necho '[info] All tests passed.'\n"
        ),
    )
    .unwrap();
    executable(&sbt);
    let tools = Tools {
        sbt: sbt.display().to_string(),
        make: "make".to_owned(),
    };
    let mut attempt = attempt_over(&repo, Config::parse(&config()).unwrap(), tools);
    let hazard = attempt
        .plan()
        .lines
        .iter()
        .find(|l| l.file == HAZARD)
        .unwrap()
        .clone();
    let changed = hazard.changed_line.clone().unwrap();
    let specs = attempt.specs_for(&hazard.id).unwrap();
    attempt.begin(&hazard.id, 1).unwrap();
    attempt
        .predict(&hazard.id, specs.clone(), "every spec fails", 2)
        .unwrap();
    assert_eq!(
        attempt.check_run(
            &hazard.id,
            &changed,
            &["mycpu.pipeline.OtherSpec".to_owned()]
        ),
        Err("spec_unknown")
    );

    let run = attempt
        .run_specs(&hazard.id, &changed, &specs, &|| 3)
        .unwrap();
    assert!(
        run.summary
            .contains("[info] - edited:  io.stall := ex.load || ex.rd === id.rs1|"),
        "{}",
        run.summary
    );
    assert!(
        matches!(&run.outcome, Outcome::Tests { failed } if failed.is_empty()),
        "{:?}",
        run.outcome
    );
    assert_eq!(run.matched, Some(false));
    let root = Checkout::open(repo.path()).unwrap().root().to_path_buf();
    let externals = root.join("externals");
    let bootcamp = std::env::var_os("BOOTCAMP_DIR")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| externals.join("chisel-bootcamp"), |dir| root.join(dir));
    let env = format!(
        "[info] - env:{};{};\n",
        externals.display(),
        bootcamp.display()
    );
    assert!(run.summary.contains(&env), "{}", run.summary);
}

#[test]
fn a_summary_keeps_what_fits_its_byte_bound_exactly() {
    const BYTES: usize = 4096;
    let line = |bytes: usize| format!("- {}", "x".repeat(bytes - 2));
    let later = line(BYTES - 2000 - 2);
    let both = format!("{}\n{later}\n", line(2000));
    assert_eq!(summary_of(&both).len(), BYTES, "two lines that fill it");
    let over = format!("{}\n{later}\n", line(2001));
    assert_eq!(summary_of(&over), format!("{later}\n"), "one byte over");
    let alone = line(BYTES - 1);
    assert_eq!(summary_of(&format!("{alone}\n")), format!("{alone}\n"));
    let long = format!("- before\n{}\n", line(BYTES));
    assert_eq!(
        summary_of(&long),
        "- before\n",
        "a line past the bound by itself is left out, not the lines before it"
    );

    // Only a failure or a compile error opens indented details.
    let tail = "[error] Compilation failed\n[error]   stray\n[error] Foo.scala:3:5: type mismatch\n[error]   found: Int\n";
    assert_eq!(
        summary_of(tail),
        "[error] Compilation failed\n[error] Foo.scala:3:5: type mismatch\n[error]   found: Int\n"
    );
}

#[test]
fn either_closing_line_of_a_passing_run_says_the_specs_ran() {
    let specs = ["mycpu.pipeline.CPUSpec".to_owned()];
    for tail in [
        "[info] All tests passed.\n",
        "[info] Tests: succeeded 2, failed 0\n",
    ] {
        assert!(
            matches!(outcome_of(&Ended::Exited(0), tail, &specs), Outcome::Tests { failed } if failed.is_empty()),
            "{tail}"
        );
    }
}

#[test]
fn a_trace_needs_a_record_size_and_a_first_cycle() {
    let header = json!({"first": 0, "record_bytes": 0, "fields": []});
    let trailer = json!({"records": 0, "end": {}}).to_string();
    let mut data = header.to_string().into_bytes();
    data.push(b'\n');
    data.extend(trailer.as_bytes());
    data.extend(u64::try_from(trailer.len()).unwrap().to_le_bytes());
    data.extend(b"OBSEND01");
    assert_eq!(
        read_trace(&data).unwrap_err(),
        "the header has no record size"
    );

    let mut unplaced = obs_trace(3, 3);
    let header_end = unplaced.iter().position(|&b| b == b'\n').unwrap();
    let header = String::from_utf8(unplaced[..header_end].to_vec()).unwrap();
    let without_first = header.replace("\"first\":40,", "");
    assert_ne!(header, without_first);
    unplaced.splice(..header_end, without_first.into_bytes());
    assert_eq!(
        read_trace(&unplaced).unwrap_err(),
        "the header has no first cycle",
        "a window with no first cycle would be drawn at the wrong cycles"
    );
}

/// A relative path is the one in the directory the command runs in.
#[cfg(unix)]
#[test]
fn a_relative_command_is_the_one_in_its_directory() {
    let dir = std::env::temp_dir().join(format!("codetrial-resolve-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let tool = dir.join("bin/say");
    std::fs::write(&tool, "#!/bin/sh\necho from-dir\n").unwrap();
    executable(&tool);
    let ran = run("bin/say", &[], &dir, &[], Duration::from_secs(5));
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(ran.ended, Ended::Exited(0));
    assert_eq!(ran.tail.trim(), "from-dir");
}

#[test]
fn task_directories_and_exercises_are_whole() {
    assert!(!graded("4-/src/main/scala/mycpu/X.scala"));
    assert!(select::exercise_id("4.12"));
    for partial in ["4.", ".4", "4.x", "x.4"] {
        assert!(!select::exercise_id(partial), "{partial}");
    }
    assert!(worth_asking(&format!("val x = {}", "a".repeat(292))));
    assert_eq!(
        checkout::without_credentials("backup@2024/mycpu"),
        "backup@2024/mycpu",
        "an `@` with no host's `:` after it is part of a path"
    );
}

#[test]
fn a_spec_is_a_spec_class_in_a_plain_package() {
    let repo = student_repo("spec-names");
    let tests = "4-pipeline/src/test/scala/mycpu/pipeline";
    repo.write(
        &format!("{tests}/CPUSpec.scala"),
        "package mycpu.pipeline\nclass CPUSpec\n",
    );
    repo.write(
        &format!("{tests}/Helpers.scala"),
        "package mycpu.pipeline\nobject Helpers\n",
    );
    repo.write(
        &format!("{tests}/OddSpec.scala"),
        "package `my-cpu`\nclass OddSpec\n",
    );
    let head = repo.commit("specs");
    let checkout = Checkout::open(repo.path()).unwrap();
    assert_eq!(
        checkout.specs(&head, "4-pipeline"),
        ["mycpu.pipeline.CPUSpec"]
    );
}

#[test]
fn a_rename_outside_the_graded_files_leaves_a_checkout_clean() {
    let repo = student_repo("rename-notes");
    std::fs::create_dir_all(repo.path().join("notes")).unwrap();
    repo.git(&["mv", "lab-notes.txt", "notes/lab.txt"]);
    let checkout = Checkout::open(repo.path()).unwrap();
    checkout.stamp().unwrap();
}

#[cfg(unix)]
#[test]
fn an_edit_stays_on_a_line_of_a_plain_file_of_the_worktree() {
    let repo = student_repo("edit-edges");
    let alias = "4-pipeline/src/main/scala/mycpu/pipeline/Alias.scala";
    std::os::unix::fs::symlink("CPU.scala", repo.path().join(alias)).unwrap();
    let head = repo.commit("an alias");
    let checkout = Checkout::open(repo.path()).unwrap();
    let worktree = Worktree::create(&checkout, &head).unwrap();
    let file = worktree.dir().join(CPU);
    let before = std::fs::read_to_string(&file).unwrap();
    assert!(
        worktree.edit(alias, 3, Some("x")).is_err(),
        "a link, even to a file of the worktree"
    );
    assert!(
        worktree
            .edit(&format!("4-pipeline/../{CPU}"), 3, Some("x"))
            .is_err(),
        "a path through `..`"
    );
    // Five lines and the final newline: the sixth, empty, is the last.
    drop(worktree.edit(CPU, 6, Some("x")).unwrap());
    assert!(worktree.edit(CPU, 7, None).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
}
