use super::*;
use crate::tasks::runnable;

/// The error table in `docs/task-mode.md` is the contract the page is written
/// against, so every row the server answers is checked against it: a code
/// that lost its arm would fall through to the 503 a failed setup gets.
#[tokio::test]
async fn every_server_error_answers_as_the_contract_table_says() {
    let mut checked = 0;
    for line in include_str!("../../../docs/task-mode.md").lines() {
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        let [code, status, retryable, _] = cells[..] else {
            continue;
        };
        let (Some(code), Ok(status)) = (
            code.strip_prefix('`')
                .and_then(|code| code.strip_suffix('`')),
            status.parse::<u16>(),
        ) else {
            continue;
        };
        let response = task_error(AccessError::new(String::leak(code.to_owned())));
        assert_eq!(response.status().as_u16(), status, "{code}");
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["code"], code);
        assert_eq!(body["retryable"].to_string(), retryable, "{code}");

        // The fallback is a failed setup's; any other code reaching it has lost
        // its own arm, whatever its status happens to share.
        if code != "setup_failed" {
            assert_ne!(body["message"], "The task could not connect.", "{code}");
        }
        checked += 1;
    }

    // The page-only and wire-only rows are not the server's to answer; this
    // only proves the table was found and read.
    assert!(checked > 20, "only {checked} rows read");
}

#[test]
fn an_assignment_page_names_a_valid_set_and_task() {
    assert_eq!(
        task_page("/t/classroom/delimiter-closer"),
        Some(("classroom", "delimiter-closer"))
    );
    assert_eq!(task_page("/t/classroom/3sum"), Some(("classroom", "3sum")));

    // Each half is checked on its own: a set id may not start with a digit, and
    // neither may hold an upper-case letter.
    for path in [
        "/t/Classroom/delimiter-closer",
        "/t/3class/delimiter-closer",
        "/t/classroom/Delimiter",
        "/t/classroom",
        "/x/classroom/delimiter-closer",
    ] {
        assert_eq!(task_page(path), None, "{path}");
    }
}

#[test]
fn a_return_path_is_refused_for_any_one_reason() {
    let page = "/t/classroom/delimiter-closer";
    let query = "?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7";
    assert_eq!(
        task_return_path(&format!("{page}{query}")).as_deref(),
        Some(&format!("{page}{query}")[..])
    );

    // A URL parser would quietly drop a fragment or a tab and rebuild a clean
    // link from what is left; these are refused before it sees them.
    for tail in ["#frag", "\t", "%0A\n"] {
        assert_eq!(
            task_return_path(&format!("{page}{query}{tail}")),
            None,
            "{tail:?}"
        );
    }
    // The length limit, exactly: the site's path pads the link to it.
    let at_length = |length: usize| {
        let head = format!("{page}?site=https%3A%2F%2Fteacher.github.io%2F");
        let tail = "&version=7";
        format!(
            "{head}{}{tail}",
            "a".repeat(length - head.len() - tail.len())
        )
    };
    assert!(task_return_path(&at_length(4096)).is_some());
    assert_eq!(task_return_path(&at_length(4097)), None);
}

#[test]
fn a_task_is_offered_in_the_languages_this_server_can_run() {
    let languages = json!(["cpp", "python", "java", "javascript"]);
    assert_eq!(
        offered_languages(&languages, true),
        ["cpp", "python", "java", "javascript"]
    );
    // With Compiler Explorer off, C, C++ and Java cannot be run here.
    assert_eq!(
        offered_languages(&languages, false),
        ["python", "javascript"]
    );
    assert!(offered_languages(&json!(["c", "java"]), false).is_empty());
    for language in ["python", "javascript"] {
        assert!(runnable(language, false), "{language}");
    }
    for language in ["c", "cpp", "java"] {
        assert!(
            runnable(language, true) && !runnable(language, false),
            "{language}"
        );
    }
    assert!(!runnable("rust", true));
}

/// The page keeps the default task's address through a GitHub sign-in, since a
/// sign-in leaves the page and returns to the address it left from.
#[test]
fn the_default_tasks_address_survives_a_sign_in() {
    let path = crate::tasks::package::Assignment::built_in_path();
    assert_eq!(task_return_path(&path).as_deref(), Some(path.as_str()));
}

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
        assert_eq!(
            defense_refusal(code),
            Some((status, retryable, message)),
            "{code}"
        );
    }
    assert_eq!(
        defense_refusal("setup_failed"),
        None,
        "not the defense's code"
    );
}
