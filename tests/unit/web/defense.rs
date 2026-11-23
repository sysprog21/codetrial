//! The `tests` module of `src/web/defense.rs`: the defense panel's refusals,
//! and its routes driven as the page drives them, against an attempt held by
//! a signed-in learner.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower_service::Service;

use super::refusal;
use crate::config::ProviderPool;
use crate::tasks::access::{Admission, PinnedTask, Preparation, TaskService};
use crate::tasks::defense::{Attempt, Checkout, Config, Slot, TASK_ID, Tools, plan};
use crate::tasks::test_support::{SITE, assignment, preparation};
use crate::web::WebServerConfig;

/// Each code the defense adds answers with its own status, retry hint and
/// sentence; a code that lost its arm would fall through to the 503 a failed
/// setup gets.
#[test]
fn every_defense_refusal_has_its_status_and_sentence() {
    let precondition = StatusCode::PRECONDITION_FAILED;
    let conflict = StatusCode::CONFLICT;
    let bad = StatusCode::BAD_REQUEST;
    let unavailable = StatusCode::SERVICE_UNAVAILABLE;
    let table = [
        (
            "workspace_required",
            precondition,
            false,
            "A defense needs your ca2026-mycpu checkout: restart CodeTrial with --task-mode --workspace <checkout>.",
        ),
        (
            "workspace_not_git",
            precondition,
            false,
            "The --workspace path is not the top of a git checkout.",
        ),
        (
            "workspace_upstream",
            precondition,
            false,
            "Your checkout has not fetched the course's upstream main: run `make review` once, or git fetch https://github.com/sysprog21/ca2026-mycpu.git main:refs/mycpu-review/upstream.",
        ),
        (
            "workspace_student_id",
            precondition,
            false,
            "Commit a valid STUDENT_ID (scripts/student-id.py check) before the defense.",
        ),
        (
            "workspace_dirty",
            conflict,
            false,
            "A graded file differs from HEAD: commit your work, then Start again.",
        ),
        (
            "workspace_no_lines",
            precondition,
            false,
            "Your commits add no line the defense can ask about yet.",
        ),
        (
            "defense_none",
            StatusCode::NOT_FOUND,
            false,
            "No defense attempt is running.",
        ),
        (
            "defense_busy",
            conflict,
            true,
            "A run is still going: wait for it to finish.",
        ),
        (
            "defense_closed",
            conflict,
            false,
            "The defense panel acts only while the attempt is under way.",
        ),
        ("item_unknown", bad, false, "No such defense item."),
        (
            "items_over",
            conflict,
            false,
            "The items are over: answer Jim's wrap-up questions.",
        ),
        (
            "item_not_begun",
            conflict,
            false,
            "Read the item's overview and press Begin first.",
        ),
        (
            "spec_unknown",
            bad,
            false,
            "Choose specs of the item's task.",
        ),
        (
            "prediction_invalid",
            bad,
            false,
            "Say in a sentence how the change fails.",
        ),
        (
            "prediction_unrun",
            bad,
            false,
            "Run every spec your prediction names.",
        ),
        (
            "claim_final",
            conflict,
            false,
            "The bug has been stated already.",
        ),
        (
            "claim_unrun",
            conflict,
            false,
            "Run the program on both models before stating the bug.",
        ),
        (
            "run_limit",
            conflict,
            false,
            "This item has had all the runs it may have.",
        ),
        (
            "prediction_required",
            conflict,
            false,
            "Record your prediction before the run.",
        ),
        (
            "edit_mismatch",
            bad,
            false,
            "Make exactly the change the item states.",
        ),
        (
            "model_not_built",
            conflict,
            false,
            "Build the two models first.",
        ),
        (
            "model_build_failed",
            unavailable,
            true,
            "The models did not build.",
        ),
        (
            "program_unknown",
            bad,
            false,
            "Choose one of your built programs.",
        ),
        (
            "claim_invalid",
            bad,
            false,
            "Give the cycle, the signal, the rule and the exercise.",
        ),
        (
            "run_failed",
            unavailable,
            true,
            "The run could not be recorded.",
        ),
        (
            "run_lost",
            unavailable,
            true,
            "The run could not be recorded.",
        ),
    ];
    for (code, status, retryable, message) in table {
        assert_eq!(refusal(code), Some((status, retryable, message)), "{code}");
    }
    assert_eq!(refusal("setup_failed"), None, "not the defense's code");
}

use crate::tasks::test_support::Repo;

const HAZARD: &str = "4-pipeline/src/main/scala/mycpu/pipeline/Hazard.scala";

/// Upstream with a blank, the student's commit that fills it, and two specs.
fn student_repo() -> Repo {
    let repo = Repo::new("routes");
    repo.write(
        HAZARD,
        "class Hazard {\n  // [CA26: Exercise 4.2] stall\n  ???\n}\n",
    );
    for spec in ["HazardSpec", "CPUSpec"] {
        repo.write(
            &format!("4-pipeline/src/test/scala/mycpu/pipeline/{spec}.scala"),
            &format!("package mycpu.pipeline\n\nclass {spec}\n"),
        );
    }
    let upstream = repo.commit("upstream");
    repo.git(&["update-ref", "refs/remotes/upstream/main", &upstream]);
    repo.write(
        HAZARD,
        "class Hazard {\n  // [CA26: Exercise 4.2] stall\n  io.stall := ex.load && ex.rd === id.rs1\n}\n",
    );
    repo.write("STUDENT_ID", "F74121234\n");
    repo.commit("hazard");
    repo
}

/// An `sbt` that fails HazardSpec whenever it runs it, and passes otherwise.
fn fake_sbt(dir: &Path) -> String {
    let sbt = dir.join("sbt");
    std::fs::write(
        &sbt,
        "#!/bin/sh\nif echo \"$2\" | grep -q HazardSpec; then\n  echo '[info] Tests: succeeded 1, failed 1'\n  echo '[error] Failed tests:'\n  printf '[error] \\tmycpu.pipeline.HazardSpec\\n'\n  exit 1\nfi\necho '[info] Tests: succeeded 1, failed 0'\necho '[info] All tests passed.'\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&sbt).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&sbt, permissions).unwrap();
    sbt.display().to_string()
}

fn defense_config() -> Value {
    json!({
        "course": "ca2026-mycpu",
        "salt": "MDEyMzQ1Njc4OWFiY2RlZg==",
        "lines": 1,
        "tasks": [{"dir": "4-pipeline", "project": "pipeline", "arch": "rv32im_zicsr"}],
        "bugs": [{"task": "4-pipeline", "variant": "Forward", "mutant": "no-flush",
                  "rule": "4.4: a taken branch flushes the younger instructions", "exercise": "4.4"}],
        "specSeconds": 900,
        "modelSeconds": 1200,
        "simSeconds": 120
    })
}

/// The learner's account id: neither the guest's 0 nor any id a constant
/// could stand in for.
const LEARNER: i64 = 7;
const SESSION: &str = "session-seven";
const SECRET: &str = "session-secret";

/// A task-mode server whose signed-in learner `LEARNER` holds the active
/// defense attempt `slot`, that learner's session cookie, and the admitted
/// task, which holds the attempt active until it is dropped.
fn server(db_path: &Path, slot: &Slot) -> (axum::Router, String, PinnedTask) {
    crate::accounts::initialize_account_database(db_path).unwrap();
    let connection = rusqlite::Connection::open(db_path).unwrap();
    connection
        .execute(
            "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES (?1, 707, 'seven', 1, 1)",
            [LEARNER],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES (?1, ?2, 9999999999, 1)",
            (crate::accounts::session_key(SESSION), LEARNER),
        )
        .unwrap();

    let plaintext = json!({"packageVersion": 1, "setId": "mycpu", "setVersion": 1,
        "rules": {"durationMin": 60, "maxHintRungs": 0, "closesAt": null,
            "lookAwaySeconds": 8, "rulesNote": null},
        "defense": defense_config()});
    let set = crate::tasks::package::decode("mycpu", 1, &serde_json::to_vec(&plaintext).unwrap())
        .unwrap();
    let service = TaskService::new();
    service.insert_unlocked(LEARNER, SITE, Arc::new(set));
    let admission = service
        .begin_admission(
            LEARNER,
            ("seven", 707),
            &assignment("mycpu", 1),
            TASK_ID,
            "routes",
            Preparation {
                defense: Some(slot.clone()),
                ..preparation()
            },
            100,
        )
        .unwrap();
    let Admission::New(task) = admission else {
        panic!("a first admission is new")
    };

    let config = WebServerConfig {
        tasks: Some(service),
        task_mode_requested: true,
        web_dir: PathBuf::new(),
        github_client_id: None,
        github_client_secret: None,
        session_secret: Some(SECRET.to_owned()),
        db_path: Some(db_path.to_path_buf()),
        github_oauth_base_url: None,
        github_api_base_url: None,
        room_prefix: "interview".to_owned(),
        fixed_room_name: None,
        production: false,
        compiler_explorer_enabled: false,
        guest_mode_enabled: false,
        default_duration_min: crate::config::DEFAULT_DURATION_MIN,
        trusted_proxy_hops: 0,
        allowed_hosts: Vec::new(),
        recording: None,
        peer: None,
        peer_observer: None,
        probe_provider_quota: false,
        update_check_commit: None,
        provision_rooms: false,
        pool: ProviderPool {
            providers: Vec::new(),
        },
    };
    let cookie = format!(
        "codetrial_session={SESSION}.{}",
        crate::token::sign_hs256(SECRET, &format!("codetrial_session={SESSION}"))
    );
    (
        crate::web::web_router(config, None, None, None),
        cookie,
        task,
    )
}

/// Sends one request to `router` from this computer, as the task page does,
/// and answers with its status and JSON body.
async fn send(
    router: &mut axum::Router,
    cookie: Option<&str>,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = Request::builder()
        .uri(path)
        .header("host", "127.0.0.1:3000");
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let request = match body {
        Some(body) => request
            .method("POST")
            .header("content-type", "application/json")
            .header("x-codetrial-task-request", "1")
            .body(Body::from(body.to_string())),
        None => request.method("GET").body(Body::empty()),
    }
    .unwrap();
    let response = router.call(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The panel's routes act on the attempt of the learner who holds it, only
/// while it is open, and tell Jim what each step showed.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn the_panel_routes_act_on_the_signed_in_learners_open_attempt() {
    let base =
        std::env::temp_dir().join(format!("codetrial-defense-routes-{}", std::process::id()));
    let repo = student_repo();
    let tools = base.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let config = Config::parse(&defense_config()).unwrap();
    let checkout = Checkout::open(repo.path()).unwrap();
    let plan = plan(&config, &checkout).unwrap();
    let tools = Tools {
        sbt: fake_sbt(&tools),
        make: "make".to_owned(),
    };
    let slot = Slot::new(Attempt::new(config, checkout, plan, tools));
    let db_path = base.join("accounts.db");
    let (mut router, cookie, _task) = server(&db_path, &slot);
    let learner = Some(cookie.as_str());

    // Only the learner holding the attempt reaches it; a guest has none.
    let (status, body) = send(&mut router, None, "/api/defense", None).await;
    assert_eq!((status, &body["code"]), (404, &json!("defense_none")));
    let (status, body) = send(&mut router, learner, "/api/defense", None).await;
    assert_eq!(
        (status, &body["code"]),
        (409, &json!("defense_closed")),
        "the panel waits for the room to start the work"
    );

    slot.set_open(true);
    slot.set_items_open(true);
    let (status, view) = send(&mut router, learner, "/api/defense", None).await;
    assert_eq!(status, 200, "{view}");
    let item = view["lines"][0]["id"].as_str().unwrap().to_owned();
    assert_ne!(item, "bug");

    // Opening a line item asks Jim for that line item's overview.
    let (status, begun) = send(
        &mut router,
        learner,
        "/api/defense/begin",
        Some(json!({"item": item})),
    )
    .await;
    assert_eq!(status, 200, "{begun}");
    assert_eq!(begun["lines"][0]["begun"], true, "{begun}");
    let notes = slot.take_notes();
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0].contains(&format!("line item {item}")),
        "{}",
        notes[0]
    );
    let (status, begun) = send(
        &mut router,
        learner,
        "/api/defense/begin",
        Some(json!({"item": "bug"})),
    )
    .await;
    assert_eq!(status, 200, "{begun}");
    assert_eq!(begun["bugBegun"], true, "{begun}");
    let notes = slot.take_notes();
    assert!(notes[0].contains("opened the bug item"), "{}", notes[0]);

    // A prediction that names a failing spec, sent without `compiles`, is one
    // that the change compiles.
    let hazard = "mycpu.pipeline.HazardSpec";
    let changed = slot.try_lock().unwrap().plan().lines[0]
        .changed_line
        .clone()
        .unwrap();
    let mut run = async |fails: Vec<&str>, specs: Vec<&str>| {
        let (status, predicted) = send(
            &mut router,
            learner,
            "/api/defense/predict",
            Some(json!({"item": item, "fails": fails, "how": "the stall goes"})),
        )
        .await;
        assert_eq!(status, 200, "{predicted}");
        assert!(slot.take_notes()[0].contains("failing specs"));
        let (status, ran) = send(
            &mut router,
            learner,
            "/api/defense/run",
            Some(json!({"item": item, "edited": changed, "specs": specs})),
        )
        .await;
        assert_eq!(status, 200, "{ran}");
        slot.take_notes().remove(0)
    };

    // Jim hears which specs failed, and that none did when none did.
    let failed = run(vec![hazard], vec![hazard]).await;
    assert!(
        failed.contains("these specs failed: [\"mycpu.pipeline.HazardSpec\"]"),
        "{failed}"
    );
    let passed = run(vec![], vec!["mycpu.pipeline.CPUSpec"]).await;
    assert!(passed.contains("every spec it ran passed"), "{passed}");

    drop(router);
    drop(repo);
    let _gone = std::fs::remove_dir_all(&base);
}
