//! Reading a student's checkout: the lines chosen from it, its worktrees
//! and the edits made in them.

use std::time::Duration;

use super::tests::{CPU, SALT, student_repo};
use super::*;

fn added(file: &str, line: usize) -> Added {
    Added {
        file: file.to_owned(),
        line,
        text: String::new(),
    }
}

/// The order and the file rule of `scripts/defense.py`, computed with its own
/// HMAC for this salt, student and head.
#[test]
fn lines_are_chosen_as_the_course_script_chooses_them() {
    let cpu = "4-pipeline/src/main/scala/mycpu/pipeline/CPU.scala";
    let hazard = "4-pipeline/src/main/scala/mycpu/pipeline/Hazard.scala";
    let divider = "2-rv32im/src/main/scala/mycpu/rv32im/Divider.scala";
    let candidates = [
        added(cpu, 10),
        added(cpu, 20),
        added(hazard, 5),
        added(divider, 7),
        added(divider, 9),
    ];
    let chosen: Vec<(String, usize)> = choose(SALT, "F74121234", "abc123", &candidates, 3)
        .into_iter()
        .map(|c| (c.file, c.line))
        .collect();
    assert_eq!(
        chosen,
        [
            (cpu.to_owned(), 20),
            (hazard.to_owned(), 5),
            (divider.to_owned(), 9)
        ]
    );
}

#[test]
fn graded_files_are_the_review_scripts() {
    assert!(graded("4-pipeline/src/main/scala/mycpu/pipeline/CPU.scala"));
    assert!(graded("software/programs/quiz-nn/routines.S"));
    assert!(!graded(
        "4-pipeline/src/test/scala/mycpu/pipeline/CPUSpec.scala"
    ));
    assert!(!graded("6-extra/src/main/scala/X.scala"));
    assert!(!graded("4-pipeline/src/main/scala/.scala"));
    assert!(!graded("software/programs/quiz-a/b/routines.S"));
}

#[test]
fn added_lines_follow_the_hunks_and_skip_what_is_not_worth_asking() {
    let diff = "diff --git a/f.scala b/f.scala\n--- a/f.scala\n+++ b/f.scala\n@@ -3,0 +4,2 @@\n+  io.out := a && b\n+  }\n@@ -9 +11 @@\n-old\n+  // a comment that is long\n";
    let lines = added_lines(diff);
    assert_eq!(
        lines
            .iter()
            .map(|a| (a.line, a.text.as_str()))
            .collect::<Vec<_>>(),
        [
            (4, "  io.out := a && b"),
            (5, "  }"),
            (11, "  // a comment that is long")
        ]
    );
    assert!(worth_asking(&lines[0].text));

    // An added line that starts with `++ ` is no file header.
    let diff = "diff --git a/f.scala b/f.scala\n--- a/f.scala\n+++ b/f.scala\n@@ -3,0 +4,2 @@\n+++ Seq(a, b)\n+  io.out := a && b\n";
    assert_eq!(
        added_lines(diff)
            .iter()
            .map(|a| (a.file.as_str(), a.line))
            .collect::<Vec<_>>(),
        [("f.scala", 4), ("f.scala", 5)]
    );
    assert!(!worth_asking(&lines[1].text));
    assert!(!worth_asking(&lines[2].text));
    assert!(!worth_asking("  ((((((((((((  ))))))))))))"));
    assert!(
        !worth_asking(&format!("  val x = {}", "a".repeat(400))),
        "a line past the bound would swell the result"
    );
}

#[test]
fn context_names_the_exercise_above_the_line() {
    let source = "a\n// [CA26: Exercise 4.2] stall\nb\nc\nd\n// [CA26: Exercise 4.3] fwd\ne";
    let (near, exercise) = context(source, 4, 1);
    assert_eq!(near, ["b", "c", "d"]);
    assert_eq!(exercise.as_deref(), Some("4.2"));
    assert_eq!(context(source, 7, 6).1.as_deref(), Some("4.3"));
    assert_eq!(context(source, 1, 6).1, None);
}

#[test]
fn specs_follow_the_stamped_commit_after_head_changes() {
    let repo = student_repo("spec-stamp");
    let path = "4-pipeline/src/test/scala/mycpu/pipeline/CPUSpec.scala";
    repo.write(path, "package mycpu.pipeline\nclass CPUSpec\n");
    let head = repo.commit("original spec");
    let checkout = Checkout::open(repo.path()).unwrap();
    // The student's configuration does not change what is read.
    repo.git(&["config", "grep.lineNumber", "true"]);
    repo.git(&["config", "grep.column", "true"]);
    repo.write(path, "package changed\nclass CPUSpec\n");
    repo.commit("change spec");
    assert_eq!(
        checkout.specs(&head, "4-pipeline"),
        ["mycpu.pipeline.CPUSpec"]
    );
}

#[test]
fn a_graded_file_renamed_out_of_the_graded_tree_is_dirty() {
    let repo = student_repo("rename-graded");
    repo.git(&["mv", CPU, "moved.scala"]);
    let checkout = Checkout::open(repo.path()).unwrap();
    assert_eq!(checkout.stamp().unwrap_err(), "workspace_dirty");
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

#[test]
fn a_checkout_that_cannot_be_defended_says_why() {
    let repo = student_repo("refuse");
    let checkout = Checkout::open(repo.path()).unwrap();

    // The ref `scripts/review.py` fetches upstream into serves as well.
    let upstream = repo.git(&["rev-parse", "refs/remotes/upstream/main"]);
    repo.git(&["update-ref", "-d", "refs/remotes/upstream/main"]);
    repo.git(&["update-ref", "refs/mycpu-review/upstream", &upstream]);
    assert_eq!(checkout.stamp().unwrap().upstream, upstream);
    repo.git(&["update-ref", "-d", "refs/mycpu-review/upstream"]);
    repo.git(&["update-ref", "refs/remotes/upstream/main", &upstream]);

    repo.write(CPU, "changed\n");
    assert_eq!(checkout.stamp().unwrap_err(), "workspace_dirty");
    repo.git(&["checkout", "--", CPU]);
    repo.write("4-pipeline/src/main/scala/mycpu/pipeline/New.scala", "x\n");
    assert_eq!(checkout.stamp().unwrap_err(), "workspace_dirty");
    std::fs::remove_file(
        repo.path()
            .join("4-pipeline/src/main/scala/mycpu/pipeline/New.scala"),
    )
    .unwrap();
    // Work outside the graded files does not stop a defense.
    repo.write("notes.txt", "mine\n");
    checkout.stamp().unwrap();

    repo.write("STUDENT_ID", "f74121234\n");
    repo.commit("lower case");
    assert_eq!(checkout.stamp().unwrap().student_id, "f74121234");
    // The course's rule: at least one digit.
    repo.write("STUDENT_ID", "ABCDEFG\n");
    repo.commit("no digit");
    assert_eq!(checkout.stamp().unwrap_err(), "workspace_student_id");

    repo.git(&["update-ref", "-d", "refs/remotes/upstream/main"]);
    assert_eq!(checkout.stamp().unwrap_err(), "workspace_upstream");

    let nested = repo.path().join("4-pipeline");
    assert_eq!(Checkout::open(&nested).unwrap_err(), "workspace_not_git");
}

#[test]
fn edits_happen_in_a_worktree_that_leaves_the_checkout_alone() {
    let repo = student_repo("worktree");
    let checkout = Checkout::open(repo.path()).unwrap();
    let stamp = checkout.stamp().unwrap();
    let dir;
    {
        let worktree = Worktree::create(&checkout, &stamp.head).unwrap();
        dir = worktree.dir().to_path_buf();
        let file = dir.join(CPU);
        let before = std::fs::read_to_string(&file).unwrap();
        {
            let _restore = worktree
                .edit(CPU, 3, Some("  io.flush := taken || valid"))
                .unwrap();
            assert!(
                std::fs::read_to_string(&file)
                    .unwrap()
                    .contains("taken || valid")
            );
            assert!(worktree.edit("../escape", 1, None).is_err());
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
        {
            let _restore = worktree.edit(CPU, 3, None).unwrap();
            assert!(!std::fs::read_to_string(&file).unwrap().contains("io.flush"));
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
        assert!(worktree.edit(CPU, 99, None).is_err());
    }
    assert!(!dir.exists(), "the worktree is removed");
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
    assert_eq!(
        repo.git(&["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
}

/// A build that puts a link where the edited file was cannot make the
/// restore write through it into the student's tree.
#[cfg(unix)]
#[test]
fn a_restore_replaces_a_link_the_run_left_and_never_follows_it() {
    let repo = student_repo("restore-link");
    let checkout = Checkout::open(repo.path()).unwrap();
    let worktree = Worktree::create(&checkout, &checkout.stamp().unwrap().head).unwrap();
    let file = worktree.dir().join(CPU);
    let before = std::fs::read_to_string(&file).unwrap();
    let theirs = repo.path().join(CPU);
    let untouched = std::fs::read_to_string(&theirs).unwrap();
    {
        let _restore = worktree.edit(CPU, 3, Some("io.flush := false.B")).unwrap();
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(&theirs, &file).unwrap();
    }
    assert_eq!(std::fs::read_to_string(&theirs).unwrap(), untouched);
    assert!(
        !std::fs::symlink_metadata(&file)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
}

/// When the worktree cannot be removed, only its own registration goes,
/// the one recorded when it was made, whatever a build wrote to its `.git`.
#[test]
fn a_worktree_that_cannot_be_removed_unregisters_only_itself() {
    let repo = student_repo("unregister");
    let checkout = Checkout::open(repo.path()).unwrap();
    let worktree = Worktree::create(&checkout, &checkout.stamp().unwrap().head).unwrap();
    let dir = worktree.dir().to_path_buf();
    let worktrees = repo.path().join(".git/worktrees");
    let own: Vec<_> = std::fs::read_dir(&worktrees)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(own.len(), 1);

    // A locked worktree is not removed by `remove --force`, so the fallback
    // runs.
    repo.git(&["worktree", "lock", &dir.display().to_string()]);
    let victim = worktrees.join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::write(
        dir.join(".git"),
        format!("gitdir: {}\n", worktrees.join("victim/../..").display()),
    )
    .unwrap();
    drop(worktree);
    assert!(!dir.exists(), "the worktree's files are gone");
    assert!(!own[0].exists(), "its own registration is gone");
    assert!(
        victim.exists(),
        "nothing else under the git directory is touched"
    );
    assert!(repo.path().join(".git/objects").exists());
}

#[test]
fn programs_are_the_built_elfs_with_plain_names() {
    let repo = student_repo("programs");
    repo.write("build/sw/mycpu/rv32im_zicsr/fib.elf", "");
    repo.write("build/sw/mycpu/rv32im_zicsr/mine-2.elf", "");
    repo.write("build/sw/mycpu/rv32im_zicsr/notes.txt", "");
    repo.write("build/sw/mycpu/rv32im_zicsr/.hidden.elf", "");
    let checkout = Checkout::open(repo.path()).unwrap();
    assert_eq!(checkout.programs("rv32im_zicsr"), ["fib.elf", "mine-2.elf"]);
    assert!(checkout.programs("rv32i").is_empty());
}

#[test]
fn a_short_selection_is_filled_from_the_lines_it_passed_over() {
    let cpu = "4-pipeline/src/main/scala/mycpu/pipeline/CPU.scala";
    let hazard = "4-pipeline/src/main/scala/mycpu/pipeline/Hazard.scala";
    // Whatever the key's order, two files and three lines give three lines.
    let candidates = [added(cpu, 10), added(cpu, 20), added(hazard, 5)];
    for n in 0..32 {
        let student = format!("S{n:08}");
        let chosen = choose(SALT, &student, "abc123", &candidates, 3);
        assert_eq!(chosen.len(), 3, "{student}");
        assert_eq!(chosen.iter().filter(|c| c.file == hazard).count(), 1);
    }
}

#[cfg(unix)]
#[test]
fn an_edit_never_follows_a_committed_link_out_of_the_worktree() {
    let repo = student_repo("link");
    let link = "4-pipeline/src/main/scala/mycpu/pipeline/Link.scala";
    let target = repo.path().join("lab-notes.txt");
    std::os::unix::fs::symlink(&target, repo.path().join(link)).unwrap();
    repo.commit("a link");
    let checkout = Checkout::open(repo.path()).unwrap();
    let head = repo.git(&["rev-parse", "HEAD"]);
    let worktree = Worktree::create(&checkout, &head).unwrap();
    assert!(worktree.edit(link, 1, Some("changed")).is_err());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "lab\n");
}

/// Another server's worktree remains available while this attempt runs.
#[test]
fn concurrent_attempts_keep_their_worktrees() {
    let repo = student_repo("stale");
    let checkout = Checkout::open(repo.path()).unwrap();
    let head = repo.git(&["rev-parse", "HEAD"]);
    let other = Worktree::create(&checkout, &head).unwrap();
    let worktree = Worktree::create(&checkout, &head).unwrap();
    assert!(other.dir().exists());
    let listed = repo.git(&["worktree", "list", "--porcelain"]);
    assert!(listed.contains(&other.dir().display().to_string()));
    assert!(listed.contains(&worktree.dir().display().to_string()));
}

/// Checking a checkout reads its index and never refreshes it: the index is
/// the student's, even where only its cached file times are stale.
#[test]
fn checking_a_checkout_leaves_its_index_alone() {
    let repo = student_repo("index");
    let index = repo.path().join(".git/index");
    let before = std::fs::read(&index).unwrap();
    // Same content, new file times: what makes `git status` rewrite an index.
    std::thread::sleep(Duration::from_millis(1100));
    repo.write(
        CPU,
        &std::fs::read_to_string(repo.path().join(CPU)).unwrap(),
    );
    let checkout = Checkout::open(repo.path()).unwrap();
    checkout.stamp().unwrap();
    assert_eq!(std::fs::read(&index).unwrap(), before);
}
