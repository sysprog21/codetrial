use std::sync::{Arc, LazyLock};

use serde_json::json;

use crate::tasks::access::{Admission, Assignment, PinnedTask, Preparation, TaskService};
use crate::tasks::package::{Download, LoadedSet, open_package};

/// The instructor site the fixture assignments name.
pub(crate) const SITE: &str = "https://teacher.github.io/course";

pub(crate) fn assignment(set_id: &str, version: u32) -> Assignment {
    Assignment::new(SITE, set_id, version).unwrap()
}

/// The PIN `tests/fixtures/task-mode/package.enc` was built with.
pub(crate) const PIN: &str = "123456";

pub(crate) fn download() -> Download {
    Download {
        manifest: include_bytes!("../../../tests/fixtures/task-mode/manifest.json").to_vec(),
        ciphertext: include_bytes!("../../../tests/fixtures/task-mode/package.enc").to_vec(),
        site_time: None,
    }
}

/// The fixture set, decoded once per test process: the key derivation is
/// deliberately slow and most tests only need its result.
pub(crate) static SET: LazyLock<Arc<LoadedSet>> = LazyLock::new(|| {
    let fixture = download();
    Arc::new(open_package("classroom", 7, &fixture.manifest, fixture.ciphertext, PIN).unwrap())
});

pub(crate) fn preparation() -> Preparation {
    Preparation {
        rules_acknowledged_at: "2026-10-07T00:00:00Z".to_owned(),
        calibration: json!({"ok": true, "metric": "keypoints", "baseline": 0.5, "keyboard": 0.7, "limit": 0.75}),
        language: "python".to_owned(),
        compiler_explorer: true,
    }
}

/// A service with the fixture set unlocked for account 1.
pub(crate) fn unlocked_service() -> TaskService {
    let service = TaskService::new();
    service.insert_unlocked(1, SITE, SET.clone());
    service
}

pub(crate) fn admit(service: &TaskService, request_key: &str, now: u64) -> PinnedTask {
    match service
        .begin_admission(
            1,
            ("learner", 42),
            &assignment("classroom", 7),
            "delimiter-closer",
            request_key,
            preparation(),
            now,
        )
        .unwrap()
    {
        Admission::New(task) => task,
        Admission::Replayed(_) => panic!("unexpected replay"),
    }
}

/// An admitted attempt at the customized fixture task.
pub(crate) async fn admitted() -> PinnedTask {
    admit(&unlocked_service(), "fixture", 100)
}

/// A throwaway git repository, as a student's checkout is, removed when
/// dropped. Its commits are made without the user's hooks or signing.
pub(crate) struct Repo(std::path::PathBuf);

impl Repo {
    /// An empty repository on `main` in a directory of its own, named by
    /// `label`.
    pub(crate) fn new(label: &str) -> Self {
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

    pub(crate) fn git(&self, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
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

    pub(crate) fn write(&self, path: &str, text: &str) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    pub(crate) fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _gone = std::fs::remove_dir_all(&self.0);
    }
}
