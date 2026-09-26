//! What the shipped pages and the problem bank promise the browser.
//!
//! Split out of `tests/web.rs`, which is still the test target: cargo
//! discovers only `tests/*.rs`, so this compiles as a module of that one
//! binary rather than relinking the crate for a file of its own.

use super::*;

#[tokio::test]
async fn static_home_markup_matches_frontend_contract() {
    let (base, server) = spawn_web_server(WebServerConfig {
        web_dir: Path::new("web").to_path_buf(),
        github_client_id: None,
        github_client_secret: None,
        session_secret: None,
        db_path: None,
        github_oauth_base_url: None,
        github_api_base_url: None,
        room_prefix: "interview".to_string(),
        fixed_room_name: None,
        production: false,
        compiler_explorer_enabled: true,
        trusted_proxy_hops: 0,
        recording: None,
        pool: Default::default(),
        probe_provider_quota: false,
    })
    .await;
    let html = reqwest::get(base).await.unwrap().text().await.unwrap();

    for text in [
        "Practice a live technical interview",
        "30 min",
        "45 min",
        "60 min",
        "Start interview",
    ] {
        assert!(html.contains(text), "missing home contract text: {text}");
    }

    // Every problem in the bank must reach the lobby. Read the titles from the
    // bank rather than restating them, so adding a problem cannot pass here by
    // being forgotten in two places at once.
    let problems = browser_problem_bank();
    for problem in problems.as_array().unwrap() {
        let title = problem["title"].as_str().unwrap();
        assert!(
            html.contains(title),
            "lobby is missing problem card: {title}"
        );
    }

    server.shutdown().await;
}

#[tokio::test]
async fn static_interview_markup_exposes_offline_surface() {
    let (base, server) = spawn_web_server(WebServerConfig {
        web_dir: Path::new("web").to_path_buf(),
        github_client_id: None,
        github_client_secret: None,
        session_secret: None,
        db_path: None,
        github_oauth_base_url: None,
        github_api_base_url: None,
        room_prefix: "interview".to_string(),
        fixed_room_name: None,
        production: false,
        compiler_explorer_enabled: true,
        trusted_proxy_hops: 0,
        recording: None,
        pool: Default::default(),
        probe_provider_quota: false,
    })
    .await;
    let html = reqwest::get(format!("{base}/interview"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    for text in [
        "CODETRIAL",
        "Problem",
        "Transcript",
        "Python 3",
        "JavaScript",
        "C",
        "C++",
        "Java",
        "Run tests",
        "End interview",
        "Test results",
    ] {
        assert!(
            html.contains(text),
            "missing interview contract text: {text}"
        );
    }

    server.shutdown().await;
}

#[test]
fn static_interview_script_keeps_data_topic_publish_contract() {
    let topics = fs::read_to_string("web/lib.js").unwrap();
    let source = fs::read_to_string("web/interview.js").unwrap();

    // The browser topic table is compared against the Rust constants
    // themselves, so renaming a topic on either side fails here rather than
    // silently splitting the two halves of the data channel. The payload shapes
    // behind these topics are asserted in tests/browser/lib.test.js.
    for (key, topic) in [
        ("code", TOPIC_CODE_UPDATE),
        ("control", TOPIC_CONTROL),
        ("integrity", TOPIC_INTEGRITY),
        ("report", TOPIC_REPORT),
        ("tests", TOPIC_TEST_RESULTS),
        ("transcript", TOPIC_TRANSCRIPTION),
    ] {
        let entry = format!("{key}: {topic:?}");
        assert!(
            topics.contains(&entry),
            "web/lib.js topics must carry {entry}"
        );
    }
    assert!(
        source.contains("publishData(new TextEncoder().encode(JSON.stringify(payload))"),
        "publish must send JSON-encoded payloads on the data channel"
    );
}

#[test]
fn static_interview_script_keeps_transcript_and_report_contract() {
    let source = fs::read_to_string("web/interview.js").unwrap();
    let transcript = source_block(
        &source,
        "async function consumeTranscript",
        "async function toggleMicrophone",
    );
    let guard = source_block(
        &source,
        "room.on(livekit.RoomEvent.DataReceived",
        "room.on(livekit.RoomEvent.ParticipantAttributesChanged",
    );
    let report = source_block(
        &source,
        "function receiveReport",
        "function playRemoteAudio",
    );

    // acceptsReport itself is covered by tests/browser/lib.test.js; this only
    // pins that the handler still consults it before touching the report.
    assert!(
        guard.contains("if (!acceptsReport(topic, participant)) return"),
        "report handler must stay gated on acceptsReport"
    );

    // LiveKit stream attribute names, which the Rust side sets in
    // transcript_stream_options.
    for snippet in [
        r#"attrs["lk.segment_id"]"#,
        r#"attrs["lk.transcription_final"] === "true""#,
        r#"participant?.identity === room.localParticipant.identity ? "you" : "interviewer""#,
        "updateTranscriptSegment(id, speaker, text",
    ] {
        assert!(
            transcript.contains(snippet),
            "missing static transcript contract: {snippet}"
        );
    }

    // The report card and markdown export are asserted behaviorally in
    // tests/browser/render.test.js. What only Rust can check is that the
    // browser still routes a report through the sanitizer before rendering it.
    for snippet in [
        "sanitizeReport(JSON.parse(new TextDecoder().decode(payload)))",
        "setLocalAudioEnabled(false)",
        "saveHistory()",
        "renderReport()",
        "room.disconnect()",
        "state.room = null",
    ] {
        assert!(
            report.contains(snippet),
            "missing static report receive contract: {snippet}"
        );
    }

    // The report card and the markdown export are asserted behaviorally in
    // tests/browser/render.test.js; what only Rust can see is that the browser
    // still routes both through the sanitizer before rendering.
}

#[test]
fn static_interview_script_leaves_candidate_identity_to_the_server() {
    let source = fs::read_to_string("web/interview.js").unwrap();

    assert!(
        source
            .contains("JSON.stringify({ problemId: problem.page, durationMin, interviewId, interviewLoop, interviewProfile, ...(interviewGrounding ? { interviewGrounding } : {}) })")
    );
    assert!(!source.contains("candidateIdentity"));
}

/// Two copies of one decision: this list admits a judge into the bank, and the
/// arms of `candidateTypeMatches` in web/runners.js decide whether a candidate
/// may write a case against it. A type added here and to the harnesses but not
/// to the browser refuses every candidate-authored case for that scenario, and
/// nothing else notices, because an unmatched shape falls through to `false`
/// rather than failing.
#[test]
fn every_supported_arg_type_is_one_the_browser_matches() {
    let runners = std::fs::read_to_string("web/runners.js").expect("web/runners.js is readable");
    let after = runners
        .split_once("function candidateTypeMatches")
        .expect("the browser still names the matcher candidateTypeMatches")
        .1;

    // Counted rather than split on the first "\n}\n", which only found the
    // right brace while every nested one stayed indented. A closing brace at
    // column zero, or one inside a template literal, would have cut the body
    // short and reported it as a missing arm.
    let mut depth = 0usize;
    let mut end = None;
    for (at, character) in after.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(at);
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &after[..end.expect("the matcher still has a body")];
    for arg_type in SUPPORTED_ARG_TYPES {
        assert!(
            body.contains(&format!("\"{arg_type}\"")),
            "{arg_type} is admitted into the bank but no arm of candidateTypeMatches names it"
        );
    }
}

#[test]
fn static_problem_bank_and_judges_cover_each_problem() {
    let problems = browser_problem_bank();
    let judges = browser_judges();
    let problems = problems.as_array().unwrap();
    let judges = judges.as_object().unwrap();

    let supported_arg_type = |arg_type: &serde_json::Value| {
        arg_type.is_null()
            || arg_type
                .as_str()
                .is_some_and(|name| SUPPORTED_ARG_TYPES.contains(&name))
    };
    let supported_signature_type = |signature_type: &serde_json::Value| {
        matches!(
            signature_type.as_str(),
            Some(
                "boolean"
                    | "character[][]"
                    | "double"
                    | "double[]"
                    | "integer"
                    | "integer[]"
                    | "integer[][]"
                    | "list<double>"
                    | "list<integer>"
                    | "list<list<integer>>"
                    | "list<list<string>>"
                    | "list<string>"
                    | "ListNode"
                    | "ListNode[]"
                    | "Node"
                    | "string"
                    | "string[]"
                    | "TreeNode"
                    | "void",
            )
        )
    };

    for problem in problems {
        let id = problem["id"].as_str().unwrap();
        let spec = judges
            .get(id)
            .unwrap_or_else(|| panic!("missing judge for {id}"));
        assert!(
            spec["cases"].as_array().unwrap().len() >= 3,
            "not enough judge cases for {id}"
        );
        for language in ["python", "javascript", "c", "cpp", "java"]
            .into_iter()
            .filter(|language| spec["kind"] != "class" || *language != "c")
        {
            assert!(
                problem["starterCode"].get(language).is_some(),
                "missing {language} starter for {id}"
            );
        }
        if spec["kind"] == "class" {
            let class_name = spec["className"]
                .as_str()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| panic!("missing class name for {id}"));

            // The harness instantiates this name and the candidate defines it,
            // and the two come from different files: the judge is renamed from
            // the bank while the starters are rewritten from the variant. A
            // rename that reached one and not the other would compile to a
            // missing symbol on the candidate's first run, which is the worst
            // place to find out.
            for (language, starter) in problem["starterCode"]
                .as_object()
                .unwrap_or_else(|| panic!("starterCode must be an object for {id}"))
            {
                assert!(
                    starter
                        .as_str()
                        .is_some_and(|code| code.contains(class_name)),
                    "{id}: the {language} starter does not define {class_name}"
                );
            }
        }
        if spec["kind"] == "function" {
            let param_names = spec["paramNames"]
                .as_array()
                .unwrap_or_else(|| panic!("paramNames must be an array for {id}"));
            let param_types = spec["paramTypes"]
                .as_array()
                .unwrap_or_else(|| panic!("paramTypes must be an array for {id}"));
            assert_eq!(
                param_names.len(),
                param_types.len(),
                "paramNames length must match paramTypes length for {id}"
            );
            for case in spec["cases"].as_array().unwrap() {
                assert_eq!(
                    param_types.len(),
                    case["input"].as_array().unwrap().len(),
                    "paramTypes length must match case input arity for {id}"
                );
            }
            for param_name in param_names {
                assert!(
                    param_name.as_str().is_some_and(|name| !name.is_empty()),
                    "empty paramName for {id}"
                );
            }
            for param_type in param_types {
                assert!(
                    supported_signature_type(param_type),
                    "unsupported paramType for {id}: {param_type:?}"
                );
            }
            assert!(
                supported_signature_type(&spec["returnType"]),
                "unsupported returnType for {id}: {:?}",
                spec["returnType"]
            );
        }
        if let Some(arg_types) = spec.get("argTypes") {
            let arg_types = arg_types
                .as_array()
                .unwrap_or_else(|| panic!("argTypes must be an array for {id}"));
            for case in spec["cases"].as_array().unwrap() {
                assert_eq!(
                    arg_types.len(),
                    case["input"].as_array().unwrap().len(),
                    "argTypes length must match case input arity for {id}"
                );
            }
            for arg_type in arg_types {
                assert!(
                    supported_arg_type(arg_type),
                    "unsupported argType for {id}: {arg_type:?}"
                );
            }
        }
        if let Some(arg_types) = spec.get("constructorArgTypes") {
            let arg_types = arg_types
                .as_array()
                .unwrap_or_else(|| panic!("constructorArgTypes must be an array for {id}"));
            for case in spec["cases"].as_array().unwrap() {
                let args = &case["input"].as_array().unwrap()[1].as_array().unwrap()[0];
                assert_eq!(
                    arg_types.len(),
                    args.as_array().unwrap().len(),
                    "constructorArgTypes length must match constructor arity for {id}"
                );
            }
            for arg_type in arg_types {
                assert!(
                    supported_arg_type(arg_type),
                    "unsupported constructorArgType for {id}: {arg_type:?}"
                );
            }
        }
        if matches!(
            spec.get("outputType").and_then(|value| value.as_str()),
            Some(
                "linkedList" | "randomList" | "graphNode" | "binaryTree" | "nextTree" | "quadTree"
            )
        ) {
            for case in spec["cases"].as_array().unwrap() {
                assert!(
                    case["expected"].as_array().is_some(),
                    "list output expected value must be an array for {id}"
                );
            }
        }
        if let Some(output_param) = spec.get("outputParam") {
            let output_param = output_param
                .as_u64()
                .unwrap_or_else(|| panic!("outputParam must be a number for {id}"));
            for case in spec["cases"].as_array().unwrap() {
                assert!(
                    case["input"]
                        .as_array()
                        .is_some_and(|input| (output_param as usize) < input.len()),
                    "outputParam out of bounds for {id}"
                );
            }
        }
        if let Some(output_param) = spec.get("outputPrefixParam") {
            let output_param = output_param
                .as_u64()
                .unwrap_or_else(|| panic!("outputPrefixParam must be a number for {id}"));
            for case in spec["cases"].as_array().unwrap() {
                assert!(
                    case["input"]
                        .as_array()
                        .is_some_and(|input| (output_param as usize) < input.len()),
                    "outputPrefixParam out of bounds for {id}"
                );
                assert!(
                    case["expected"].as_array().is_some(),
                    "outputPrefixParam expected output must be an array for {id}"
                );
            }
        }
    }

    // Problems whose judges cannot be plain equality need their custom checker,
    // and the tricky inputs must stay in the fixture set.
    assert_eq!(judges["two-sum"]["checker"], "indexPair");
    assert_eq!(
        judges["longest-palindromic-substring"]["checker"],
        "palindrome"
    );
    assert_eq!(judges["course-schedule-ii"]["checker"], "dependencyOrder");
    assert_eq!(
        judges["convert-sorted-array-to-binary-search-tree"]["checker"],
        "balancedBst"
    );
    for (id, label) in [
        ("two-sum", "duplicates"),
        ("merge-sorted-array", "duplicates"),
        ("remove-element", "remove every"),
        ("remove-duplicates-from-sorted-array", "several duplicate"),
        ("remove-duplicates-from-sorted-array-ii", "single repeated"),
        ("majority-element", "not the first"),
        ("rotate-array", "k larger"),
        ("best-time-to-buy-and-sell-stock", "decreasing"),
        ("best-time-to-buy-and-sell-stock-ii", "multiple small rises"),
        ("jump-game", "late unreachable"),
        ("jump-game-ii", "three hops needed"),
        ("h-index", "h capped"),
        ("insert-delete-getrandom-o1", "removed value"),
        ("product-of-array-except-self", "two zeros"),
        ("gas-station", "wraparound"),
        ("candy", "plateau between"),
        ("trapping-rain-water", "right boundary"),
        ("roman-to-integer", "compound subtractive"),
        ("integer-to-roman", "compound subtractive"),
        ("length-of-last-word", "trailing spaces"),
        ("longest-common-prefix", "empty string"),
        ("reverse-words-in-a-string", "collapse internal spaces"),
        ("zigzag-conversion", "single row"),
        (
            "find-the-index-of-the-first-occurrence-in-a-string",
            "later occurrence",
        ),
        ("text-justification", "uneven spaces go left"),
        ("valid-palindrome", "digits count"),
        ("is-subsequence", "not substring"),
        ("container-with-most-water", "interior best"),
        ("two-sum-ii-input-array-is-sorted", "duplicate values"),
        ("3sum", "many duplicates"),
        ("happy-number", "loops through four"),
        (
            "longest-substring-without-repeating-characters",
            "left edge",
        ),
        ("minimum-window-substring", "duplicate required"),
        (
            "substring-with-concatenation-of-all-words",
            "duplicate words",
        ),
        ("minimum-size-subarray-sum", "no qualifying window"),
        ("valid-sudoku", "duplicate in box only"),
        ("spiral-matrix", "single column"),
        ("rotate-image", "negative values"),
        ("set-matrix-zeroes", "first column marker"),
        ("game-of-life", "column turns into row"),
        ("ransom-note", "insufficient multiplicity"),
        ("isomorphic-strings", "two sources one target"),
        ("word-pattern", "many pattern letters share one word"),
        ("valid-anagram", "same letters wrong multiplicity"),
        ("group-anagrams", "duplicate words preserved"),
        ("contains-duplicate-ii", "k zero"),
        ("longest-consecutive-sequence", "duplicates in long run"),
        ("summary-ranges", "negative to positive range"),
        ("insert-interval", "touching endpoints merge"),
        (
            "minimum-number-of-arrows-to-burst-balloons",
            "touching endpoints share check",
        ),
        ("simplify-path", "dot names are directories"),
        ("min-stack", "duplicate minimum survives one pop"),
        (
            "evaluate-reverse-polish-notation",
            "negative division truncates toward zero",
        ),
        ("basic-calculator", "nested subtraction"),
        ("linked-list-cycle", "tail points to itself"),
        ("add-two-numbers", "carry extends result"),
        ("merge-two-sorted-lists", "negative values"),
        ("copy-list-with-random-pointer", "duplicate values"),
        ("reverse-linked-list-ii", "negative values"),
        ("reverse-nodes-in-k-group", "k equals one"),
        ("remove-nth-node-from-end-of-list", "remove head"),
        (
            "remove-duplicates-from-sorted-list-ii",
            "all values duplicated",
        ),
        ("rotate-list", "k larger than length"),
        ("partition-list", "relative order preserved"),
        ("maximum-depth-of-binary-tree", "left skew depth four"),
        ("same-tree", "same values different null side"),
        ("invert-binary-tree", "sparse tree keeps null positions"),
        ("symmetric-tree", "cross sparse mirror"),
        (
            "construct-binary-tree-from-preorder-and-inorder-traversal",
            "left skew",
        ),
        (
            "construct-binary-tree-from-inorder-and-postorder-traversal",
            "right skew",
        ),
        (
            "populating-next-right-pointers-in-each-node-ii",
            "missing middle children",
        ),
        (
            "flatten-binary-tree-to-linked-list",
            "branching preorder chain",
        ),
        ("path-sum", "partial path is not enough"),
        ("sum-root-to-leaf-numbers", "skewed digits"),
        (
            "binary-tree-maximum-path-sum",
            "all negative picks one node",
        ),
        ("binary-search-tree-iterator", "left skew bst"),
        ("count-complete-tree-nodes", "partial final level"),
        (
            "lowest-common-ancestor-of-a-binary-tree",
            "ancestor is one target",
        ),
        (
            "binary-tree-right-side-view",
            "left depth visible after right ends",
        ),
        ("average-of-levels-in-binary-tree", "mixed signs average"),
        (
            "binary-tree-level-order-traversal",
            "sparse keeps left to right",
        ),
        (
            "binary-tree-zigzag-level-order-traversal",
            "four levels alternate",
        ),
        (
            "minimum-absolute-difference-in-bst",
            "minimum not parent child",
        ),
        ("kth-smallest-element-in-a-bst", "left skew middle"),
        (
            "validate-binary-search-tree",
            "deep descendant violates ancestor",
        ),
        ("surrounded-regions", "edge connected pocket stays"),
        ("clone-graph", "chain graph deep copy"),
        ("evaluate-division", "disconnected components"),
        ("course-schedule", "long loop"),
        ("course-schedule-ii", "branching dependencies"),
        ("snakes-and-ladders", "unreachable trap"),
        ("minimum-genetic-mutation", "target not approved"),
        ("word-ladder", "shorter route through decoy"),
        ("implement-trie-prefix-tree", "prefix is not word"),
        (
            "design-add-and-search-words-data-structure",
            "dot matches exactly one letter",
        ),
        ("word-search-ii", "duplicate board paths return word once"),
        (
            "letter-combinations-of-a-phone-number",
            "four choices digit",
        ),
        ("combinations", "choose all numbers"),
        ("permutations", "negative value"),
        ("combination-sum", "unsorted crate sizes"),
        ("n-queens-ii", "two towers impossible"),
        ("generate-parentheses", "four pairs"),
        ("word-search", "cannot reuse cell"),
        (
            "convert-sorted-array-to-binary-search-tree",
            "two values allow either root",
        ),
        ("merge-intervals", "unsorted"),
        ("valid-parentheses", "interleaved"),
    ] {
        let labels = judges[id]["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|case| case["label"].as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            labels.contains(label),
            "{id} judge fixtures should cover the {label} case, got: {labels}"
        );
    }
}

/// The rubric (optimal approach, pitfalls) must stay server-side, so the Rust
/// table cannot simply be generated from the browser bank. Cross-check the
/// fields both sides duplicate instead, so drift fails here rather than
/// silently giving the agent a different problem than the candidate sees.
#[test]
fn rust_problem_bank_matches_the_browser_problem_bank() {
    // The loader finds the default in the page map, so the map's default is the
    // server's, or a link naming nothing opens a different exercise than the
    // agent falls back to.
    let pages: Value =
        serde_json::from_str(&fs::read_to_string("web/problem-pages.json").unwrap()).unwrap();
    let defaults = pages
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, entry)| entry["default"] == true)
        .map(|(id, _)| id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(defaults, [codetrial::agent::DEFAULT_PROBLEM_ID]);
    let browser = browser_problem_bank();
    let browser = browser.as_array().unwrap();

    assert_eq!(
        browser.len(),
        codetrial::agent::PROBLEMS.len(),
        "problem count differs between web/problems/ and src/agent.rs"
    );

    // Matched by id, not position: the browser list is in lobby display order.
    for problem in codetrial::agent::PROBLEMS {
        let entry = browser
            .iter()
            .find(|entry| entry["id"].as_str() == Some(problem.id))
            .unwrap_or_else(|| panic!("web/problems/ is missing {}", problem.id));
        assert_eq!(
            entry["title"].as_str(),
            Some(problem.variant().title),
            "title differs for {}",
            problem.id
        );
        assert_eq!(
            entry["difficulty"].as_str(),
            Some(problem.difficulty),
            "difficulty differs for {}",
            problem.id
        );
    }
}

#[tokio::test]
#[ignore = "browser-dependent: hermetic and about 30s idle, but a real browser \
            under a loaded machine has been measured failing on page.goto past \
            a two minute navigation timeout. CI runs it as its own step; \
            locally, `cargo test --test web -- --ignored` when the machine is \
            not busy"]
async fn browser_check_accepts_running_rust_server_offline_interview() {
    let (mut config, cookie, db_path) = signed_in_web_config("browser-offline");
    config.web_dir = Path::new("web").to_path_buf();
    config.pool = Default::default();
    let (base, server) = spawn_web_server(config).await;
    let output = tokio::task::spawn_blocking(move || {
        Command::new("sh")
            .arg("scripts/browser-check.sh")
            .env("CODETRIAL_WEB_URL", base)
            .env("BROWSER_CHECK_AGENT", "offline")
            .env("BROWSER_CHECK_SESSION_COOKIE", cookie)
            // The mock, not godbolt.org. The live service is what the default
            // reaches for, and it turned this into a four minute test that a
            // network hiccup could fail: the mock runs the same editor and
            // results pipeline against fixed responses in a fraction of it.
            // Whether godbolt's API still looks the way this code expects is a
            // real question, but it is not one a commit should be blocked on.
            .env("BROWSER_CHECK_COMPILER_EXPLORER_BASE_URL", "mock")
            .output()
    })
    .await
    .unwrap()
    .expect("browser check should run");

    // Exit 3 is browser-check.sh saying Playwright is not installed. Skipped
    // rather than failed, matching tests/browser/face-detector.test.js: the
    // suite has to stay runnable without an 800 MB download. CI installs
    // Chromium, so CI never takes this arm.
    if output.status.code() == Some(3) {
        eprintln!(
            "skipping offline browser check: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        server.shutdown().await;
        remove_database(db_path).await;
        return;
    }

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    server.shutdown().await;
    remove_database(db_path).await;
}

#[tokio::test]
#[ignore = "credentialed browser check; requires LiveKit and Google env"]
async fn browser_check_accepts_running_rust_server_with_rust_agent() {
    let (mut config, cookie, db_path) = signed_in_web_config("browser-rust");
    config.web_dir = Path::new("web").to_path_buf();
    config.pool = primary_pool(
        &std::env::var("LIVEKIT_URL").expect("set LIVEKIT_URL"),
        &std::env::var("LIVEKIT_API_KEY").expect("set LIVEKIT_API_KEY"),
        &std::env::var("LIVEKIT_API_SECRET").expect("set LIVEKIT_API_SECRET"),
    );
    let (base, server) = spawn_web_server(config).await;
    let output = tokio::task::spawn_blocking(move || {
        Command::new("sh")
            .arg("scripts/browser-check.sh")
            .env("CODETRIAL_WEB_URL", base)
            .env("BROWSER_CHECK_AGENT", "rust")
            .env("BROWSER_CHECK_SESSION_COOKIE", cookie)
            .output()
    })
    .await
    .unwrap()
    .expect("browser check should run");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    server.shutdown().await;
    remove_database(db_path).await;
}

#[test]
fn browser_check_loads_credentials_for_external_rust_server() {
    let env_file = std::env::temp_dir().join(format!(
        "codetrial-env-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(
        &env_file,
        "LIVEKIT_URL=wss://example.livekit.cloud\nLIVEKIT_API_KEY=key\nLIVEKIT_API_SECRET=secret\nGOOGLE_API_KEY=google\n",
    )
    .unwrap();

    let output = Command::new("sh")
        .arg("scripts/browser-check.sh")
        .env("CODETRIAL_WEB_URL", "http://127.0.0.1:1")
        .env("CODETRIAL_CONFIG_ENV", &env_file)
        .env("BROWSER_CHECK_AGENT", "rust")
        .env("BROWSER_CHECK_VALIDATE_ENV_ONLY", "1")
        .env_remove("LIVEKIT_URL")
        .env_remove("LIVEKIT_API_KEY")
        .env_remove("LIVEKIT_API_SECRET")
        .env_remove("GOOGLE_API_KEY")
        .output()
        .expect("browser check should run");

    fs::remove_file(env_file).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn browser_check_prints_server_log_on_node_failure() {
    let script = fs::read_to_string("scripts/browser-check.sh").unwrap();
    let driver = fs::read_to_string("scripts/browser-check.cjs").unwrap();

    assert!(script.contains("SERVER_LOG=\"$SERVER_LOG\""));
    assert!(script.contains("scripts/browser-check.cjs"));
    assert!(driver.contains("fs.readFileSync(process.env.SERVER_LOG"));
}

#[test]
fn browser_check_bounds_room_service_requests() {
    let driver = fs::read_to_string("scripts/livekit-room-service.cjs").unwrap();
    let room_service = source_block(
        &driver,
        "async function roomService",
        "\n/// Protobuf JSON omits",
    );

    assert!(room_service.contains("signal: AbortSignal.timeout(30000)"));
}

#[test]
fn static_interview_script_keeps_exit_fallback_short() {
    let source = fs::read_to_string("web/interview.js").unwrap();
    let ending = source_block(&source, "function endInterview", "function leaveRoom");

    // Short, but never shorter than the agent takes to answer: report
    // generation is bounded by REPORT_TIMEOUT and a timer-driven end spends
    // WRAP_UP_WAIT before that. Revealing the escape hatch first loses reports
    // that arrive. The wait itself is named and checked against those constants
    // in the_browser_escape_hatch_outlasts_the_report_deadline, so what is left
    // here is that this block is the one that waits.
    assert!(ending.contains("REPORT_ESCAPE_WAIT_MS"));
    assert!(!ending.contains("25000"), "shorter than REPORT_TIMEOUT");
}

#[test]
fn static_interview_script_flushes_code_before_tests() {
    let source = fs::read_to_string("web/interview.js").unwrap();
    let run_tests = source_block(
        &source,
        "async function runTests",
        "\nfunction renderResults",
    );

    assert!(source.contains("function flushPendingCodePublish"));
    assert!(
        run_tests.find("flushPendingCodePublish()").unwrap()
            < run_tests.find("runBrowserTests").unwrap()
    );
    assert!(
        run_tests.find("flushPendingCodePublish()").unwrap()
            < run_tests.find("publish(topics.tests").unwrap()
    );
}

#[test]
fn static_interview_script_has_no_audio_unlock_overlay() {
    let source = fs::read_to_string("web/interview.js").unwrap();
    let styles = fs::read_to_string("web/styles.css").unwrap();

    assert!(!source.contains("Click to enable interviewer audio"));
    assert!(!source.contains("showAudioUnlock"));
    assert!(!styles.contains("audio-unlock"));
}

#[test]
fn static_interview_script_marks_agent_ready_visually() {
    let source = fs::read_to_string("web/interview.js").unwrap();
    let styles = fs::read_to_string("web/styles.css").unwrap();

    assert!(source.contains("function setAgentStateLabel"));
    assert!(source.contains(r#"setAgentStateLabel("Waiting", false)"#));
    assert!(
        source.contains(
            r#"setAgentStateLabel(labels[value] || providerUiState("live").label, value === "listening")"#
        )
    );
    assert!(source.contains(r#"classList.toggle("ready", ready)"#));
    assert!(styles.contains(".agent-pill.ready"));
}

/// The clamp above is the fallback, not the authority. `token_duration_min`
/// also bounds the length by the recording cap, which is per deployment and
/// so cannot be pinned by a constant the way the range is: the only way the
/// page can know it is to be told, and the only way it stays told is to read
/// the answer rather than the request.
#[test]
fn browser_interview_takes_the_length_the_server_granted() {
    let interview = fs::read_to_string("web/interview.js").unwrap();

    assert!(
        interview.contains("applyGrantedDuration(connection.durationMin)"),
        "web/interview.js never reads the length /api/token granted"
    );

    // Ahead of `connectLiveKit`, because the replay's opening lifecycle event
    // writes the round split down and a split from the requested length would
    // be the first thing recorded.
    let applied = interview
        .find("applyGrantedDuration(connection.durationMin)")
        .expect("the call is asserted above");
    let connected = interview
        .find("await connectLiveKit(connection")
        .expect("the page joins the room");
    assert!(
        applied < connected,
        "the length is applied after the room is joined"
    );
}

/// `src/config.rs` owns the interview length and says the browser lobby mirrors
/// the range, which nothing checked. The two are not interchangeable: the
/// browser countdown and `run_room`'s server-side hard deadline are both built
/// from this number, so a lobby offering a length the server clamps gives the
/// candidate a timer that disagrees with the process that will actually end
/// their interview.
#[test]
fn browser_interview_duration_matches_the_server_clamp() {
    let interview = fs::read_to_string("web/interview.js").unwrap();
    let page = fs::read_to_string("web/index.html").unwrap();

    let clamp = call_arguments(
        &interview[interview.find("let durationMin = ").unwrap()..],
        "clamp(",
    );
    assert_eq!(
        clamp.len(),
        3,
        "clamp takes a value and two bounds: {clamp:?}"
    );

    let parsed = |value: &str| -> u32 {
        value
            .parse()
            .unwrap_or_else(|error| panic!("{value} is not a bound: {error}"))
    };
    assert_eq!(
        parsed(clamp[1]),
        MIN_DURATION_MIN,
        "web/interview.js clamps below the server minimum"
    );
    assert_eq!(
        parsed(clamp[2]),
        MAX_DURATION_MIN,
        "web/interview.js clamps above the server maximum"
    );

    // The fallback the query string gets when it carries no duration.
    let fallback = clamp[0]
        .rsplit("||")
        .next()
        .expect("the clamped value falls back to a literal")
        .trim();
    assert_eq!(
        parsed(fallback),
        DEFAULT_DURATION_MIN,
        "web/interview.js falls back to a different default than the server"
    );

    // Every button the lobby offers has to be a length the server will honour,
    // or the candidate picks one number and is given another without being
    // told.
    let mut offered = Vec::new();
    for chunk in page.split("data-duration=\"").skip(1) {
        let value = chunk.split('"').next().expect("data-duration is quoted");
        let minutes: u32 = value
            .parse()
            .unwrap_or_else(|error| panic!("data-duration={value} is not a number: {error}"));
        assert!(
            (MIN_DURATION_MIN..=MAX_DURATION_MIN).contains(&minutes),
            "web/index.html offers {minutes} minutes, which the server clamps to \
             {MIN_DURATION_MIN}..={MAX_DURATION_MIN}"
        );
        offered.push(minutes);
    }
    assert!(!offered.is_empty(), "the lobby offers no durations at all");

    // The preselected button decides what a candidate who touches nothing gets.
    let selected = page
        .split("duration-button selected\"")
        .nth(1)
        .expect("one duration button is preselected");
    assert_eq!(
        browser_number(selected, "data-duration=\"", '"'),
        DEFAULT_DURATION_MIN,
        "the preselected lobby duration is not the server default"
    );

    // The lobby derives its length from the checked difficulty, so the two
    // assertions above are only worth anything if exactly one box starts
    // checked: two would make the derived length ambiguous, and none would
    // leave `suggestedDuration` deciding from an empty set.
    //
    // What that box derives is checked in the browser rather than here, by "a
    // candidate who touches nothing gets the server's own default length" in
    // tests/browser/lobby.test.js, which asserts that the length app.js renders
    // on an untouched lobby is the preselected one this test just pinned to
    // DEFAULT_DURATION_MIN. There is no longer a lobby-side literal to read:
    // app.js derives the length, so the constant reaches the candidate through
    // the markup. Restating the mapping here in Rust would be a second copy
    // that drifts: swapping which difficulty yields thirty keeps every number
    // inside the cap, so a hand-written mirror of the mapping went on passing
    // while the untouched default had moved.
    //
    // Each `<input>` start tag is read whole so the check does not care what
    // order its attributes are written in.
    let checked = page
        .split("<input")
        .skip(1)
        .map(|tag| &tag[..tag.find('>').unwrap_or(tag.len())])
        .filter(|tag| tag.contains("name=\"difficulty\"") && tag.contains("checked"))
        .count();
    assert_eq!(
        checked, 1,
        "exactly one difficulty starts checked, or the length it derives is ambiguous"
    );

    // The other half of this, that a suggested length is one a default
    // deployment can record to the end, is a `const` assertion beside the two
    // constants in src/config.rs and a rendered-attribute check in
    // tests/browser/lobby.test.js. Neither belongs here: one is a compile-time
    // relation and the other is what a browser paints.
}

/// A card the filter cannot show is a problem nobody can reach.
///
/// web/app.js hides every card whose `data-difficulty` is not among the checked
/// boxes, so a generated card carrying a level the fieldset never offers is
/// invisible for good, is never recommended, and nothing fails to say so. The
/// cards come from problem-bank/problems.json through
/// scripts/gen-problem-cards.py and the boxes are hand-written, which is
/// exactly the seam a new difficulty string would slip through.
///
/// Both sides are read off the rendered page rather than restated here, because
/// a third copy of the level names is a third place to forget one.
#[test]
fn browser_every_problem_card_has_a_difficulty_the_filter_offers() {
    let page = fs::read_to_string("web/index.html").unwrap();

    // Each `<input>` start tag read whole, so the check does not care what
    // order its attributes are written in.
    let offered: std::collections::BTreeSet<&str> = page
        .split("<input")
        .skip(1)
        .map(|tag| &tag[..tag.find('>').unwrap_or(tag.len())])
        .filter(|tag| tag.contains("name=\"difficulty\""))
        .filter_map(|tag| tag.split("value=\"").nth(1))
        .filter_map(|value| value.split('"').next())
        .collect();
    assert!(
        !offered.is_empty(),
        "web/index.html offers no difficulty filter at all"
    );

    for chunk in page.split("data-difficulty=\"").skip(1) {
        let level = chunk.split('"').next().expect("data-difficulty is quoted");
        assert!(
            offered.contains(level),
            "web/index.html has a {level} problem card, which the difficulty filter never shows"
        );
    }
}
