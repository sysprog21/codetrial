use std::path::Path;

use serde_json::json;

pub(super) const SALT: &[u8] = b"0123456789abcdef";

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

pub(super) fn executable(tool: &Path) {
    let mut permissions = std::fs::metadata(tool).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(tool, permissions).unwrap();
}
