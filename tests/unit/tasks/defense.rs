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

pub(super) fn executable(tool: &Path) {
    let mut permissions = std::fs::metadata(tool).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(tool, permissions).unwrap();
}
