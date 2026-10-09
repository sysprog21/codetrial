use super::*;

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
