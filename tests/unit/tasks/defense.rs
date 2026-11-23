use std::path::{Path, PathBuf};

use std::process::Command;

use std::time::Duration;

use serde_json::json;

use super::*;

const SALT: &[u8] = b"0123456789abcdef";

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

/// A throwaway git repository shaped like a student's checkout.
pub(super) struct Repo(PathBuf);

impl Repo {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "codetrial-defense-test-{label}-{}",
            std::process::id()
        ));
        let _gone = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Self(dir);
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(["-c", "user.name=T", "-c", "user.email=t@example.invalid"])
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _gone = std::fs::remove_dir_all(&self.0);
    }
}

const CPU: &str = "4-pipeline/src/main/scala/mycpu/pipeline/CPU.scala";
const HAZARD: &str = "4-pipeline/src/main/scala/mycpu/pipeline/Hazard.scala";

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
    assert_eq!(checkout.stamp().unwrap_err(), "workspace_student_id");
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

#[cfg(unix)]
#[test]
fn a_run_past_its_limit_loses_its_whole_process_group() {
    let dir = std::env::temp_dir();
    let marker = dir.join(format!("codetrial-defense-orphan-{}", std::process::id()));
    let script = format!("(sleep 2; touch {}) & sleep 30", marker.display());
    let ran = run(
        "sh",
        &["-c".to_owned(), script],
        &dir,
        &[],
        Duration::from_millis(300),
    );
    assert_eq!(ran.ended, Ended::Timeout);
    std::thread::sleep(Duration::from_millis(2500));
    assert!(!marker.exists(), "the background child was killed too");
}

#[test]
fn a_run_keeps_only_the_tail_of_its_output() {
    let ran = run(
        "sh",
        &[
            "-c".to_owned(),
            "i=0; while [ $i -lt 20000 ]; do echo line-$i; i=$((i+1)); done; echo done >&2; exit 3"
                .to_owned(),
        ],
        &std::env::temp_dir(),
        &[("UNUSED".to_owned(), "1".to_owned())],
        Duration::from_secs(30),
    );
    assert_eq!(ran.ended, Ended::Exited(3));
    assert!(ran.tail.len() <= TAIL_BYTES);
    assert!(ran.tail.ends_with("done\n"));
    assert!(!ran.tail.contains("line-0\n"));
}

#[cfg(unix)]
#[test]
fn continuous_output_is_drained_until_timeout() {
    let ran = run(
        "sh",
        &[
            "-c".to_owned(),
            "while :; do printf '0123456789'; done".to_owned(),
        ],
        &std::env::temp_dir(),
        &[],
        Duration::from_millis(500),
    );
    assert_eq!(ran.ended, Ended::Timeout);
    assert_eq!(ran.tail.len(), TAIL_BYTES);
    assert!(ran.tail.bytes().all(|byte| byte.is_ascii_digit()));
}

/// A process that left the group, still holding the output pipe, keeps the
/// end from coming: what the command printed is kept all the same.
#[cfg(unix)]
#[test]
fn output_is_kept_when_an_escaped_process_holds_the_pipe() {
    let ran = run(
        "sh",
        &[
            "-c".to_owned(),
            "setsid sleep 5 & echo printed-before-exit; exit 0".to_owned(),
        ],
        &std::env::temp_dir(),
        &[],
        Duration::from_secs(10),
    );
    assert_eq!(ran.ended, Ended::Exited(0));
    assert_eq!(ran.tail, "printed-before-exit\n");
}

#[test]
fn a_command_that_does_not_exist_is_a_telemetry_gap() {
    let ran = run(
        "codetrial-no-such-tool",
        &[],
        &std::env::temp_dir(),
        &[],
        Duration::from_secs(1),
    );
    assert!(matches!(ran.ended, Ended::NotStarted(_)));
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
