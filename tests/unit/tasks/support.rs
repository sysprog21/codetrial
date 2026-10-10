use std::sync::{Arc, LazyLock};

use serde_json::json;

use crate::tasks::access::{Admission, Assignment, PinnedTask, Preparation, TaskService};
use crate::tasks::package::{Download, LoadedSet, open_package};

/// The instructor site the fixture assignments name.
pub const SITE: &str = "https://teacher.github.io/course";

pub fn assignment(set_id: &str, version: u32) -> Assignment {
    Assignment::new(SITE, set_id, version).unwrap()
}

/// The PIN `tests/fixtures/task-mode/package.enc` was built with.
pub const PIN: &str = "123456";

pub fn download() -> Download {
    Download {
        manifest: include_bytes!("../../../tests/fixtures/task-mode/manifest.json").to_vec(),
        ciphertext: include_bytes!("../../../tests/fixtures/task-mode/package.enc").to_vec(),
        site_time: None,
    }
}

/// The fixture set, decoded once per test process: the key derivation is
/// deliberately slow and most tests only need its result.
pub static SET: LazyLock<Arc<LoadedSet>> = LazyLock::new(|| {
    let fixture = download();
    Arc::new(open_package("classroom", 7, &fixture.manifest, fixture.ciphertext, PIN).unwrap())
});

pub fn preparation() -> Preparation {
    Preparation {
        rules_acknowledged_at: "2026-10-07T00:00:00Z".to_owned(),
        calibration: json!({"ok": true, "metric": "keypoints", "baseline": 0.5, "keyboard": 0.7, "limit": 0.75}),
        language: "python".to_owned(),
    }
}

/// A service with the fixture set unlocked for account 1.
pub fn unlocked_service() -> TaskService {
    let service = TaskService::new();
    service.insert_unlocked(1, SITE, SET.clone());
    service
}

pub fn admit(service: &TaskService, request_key: &str, now: u64) -> PinnedTask {
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
pub async fn admitted() -> PinnedTask {
    admit(&unlocked_service(), "fixture", 100)
}
