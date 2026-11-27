//! What the shipped pages and the problem bank promise the browser.
//!
//! Split out of `tests/web.rs`, which is still the test target: cargo
//! discovers only `tests/*.rs`, so this compiles as a module of that one
//! binary rather than relinking the crate for a file of its own.

use super::*;

#[test]
fn preflight_interviewer_choices_match_server_choices() {
    let html = fs::read_to_string("web/interview.html").unwrap();
    for (id, choices) in [
        ("interviewer-voice", codetrial::agent::INTERVIEWER_VOICES),
        ("interviewer-accent", codetrial::agent::INTERVIEWER_ACCENTS),
    ] {
        let opening = format!("<select id=\"{id}\">");
        let select = html
            .split_once(&opening)
            .expect("the interviewer selector must exist")
            .1
            .split_once("</select>")
            .expect("the interviewer selector must have a closing boundary")
            .0;
        let values: Vec<_> = select
            .split("<option")
            .skip(1)
            .map(|option| {
                let tag = option.split_once('>').expect("an option must close").0;
                tag.split_once("value=\"")
                    .expect("each option must have a value")
                    .1
                    .split_once('"')
                    .expect("the option value must close")
                    .0
            })
            .collect();
        let expected: Vec<_> = std::iter::once("")
            .chain(choices.iter().copied())
            .chain(std::iter::once("Random"))
            .collect();
        assert_eq!(values, expected, "{id} must match the server's choices");
    }
}

#[tokio::test]
async fn static_home_markup_matches_frontend_contract() {
    let (base, server) = spawn_web_server(WebServerConfig {
        tasks: None,
        task_mode_requested: false,
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
        guest_mode_enabled: false,
        default_duration_min: DEFAULT_DURATION_MIN,
        trusted_proxy_hops: 0,
        allowed_hosts: Vec::new(),
        recording: None,
        peer: None,
        peer_observer: None,
        pool: Default::default(),
        probe_provider_quota: false,
        update_check_commit: None,
        provision_rooms: false,
    })
    .await;
    let html = http_client()
        .get(base)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    for text in [
        "Start Interview",
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
        tasks: None,
        task_mode_requested: false,
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
        guest_mode_enabled: false,
        default_duration_min: DEFAULT_DURATION_MIN,
        trusted_proxy_hops: 0,
        allowed_hosts: Vec::new(),
        recording: None,
        peer: None,
        peer_observer: None,
        pool: Default::default(),
        probe_provider_quota: false,
        update_check_commit: None,
        provision_rooms: false,
    })
    .await;
    let html = http_client()
        .get(format!("{base}/interview"))
        .send()
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
fn task_topic_table_matches_the_task_room() {
    let topics = fs::read_to_string("web/lib.js").unwrap();
    let start = topics.find("export const TASK_TOPICS").unwrap();
    let table = &topics[start..start + topics[start..].find("});").unwrap()];
    let expected = [
        ("connected", TOPIC_TASK_CONNECTED),
        ("start", TOPIC_TASK_START),
        ("end", TOPIC_TASK_END),
        ("thinking", TOPIC_TASK_THINKING),
        ("error", TOPIC_TASK_ERROR),
        ("transcript", TOPIC_TASK_TRANSCRIPT),
        ("action", TOPIC_TASK_ACTION),
        ("revision", TOPIC_TASK_REVISION),
        ("run", TOPIC_TASK_RUN),
        ("state", TOPIC_TASK_STATE),
        ("review", TOPIC_TASK_REVIEW),
    ];
    for (key, topic) in expected {
        let entry = format!("{key}: {topic:?}");
        assert!(table.contains(&entry), "TASK_TOPICS must carry {entry}");
    }
    assert_eq!(table.matches(": \"").count(), expected.len());
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
        "livekit.RoomEvent.DataReceived",
        "livekit.RoomEvent.ParticipantAttributesChanged",
    );
    let report = source_block(
        &source,
        "function receiveReport",
        "function playRemoteAudio",
    );

    // The anchor above is only the event name, because the formatter wraps the
    // call; compare compacted to pin that the handler is on the room.
    assert!(
        compact(&source).contains("room.on(livekit.RoomEvent.DataReceived,"),
        "DataReceived handler must stay registered with room.on"
    );

    // acceptsReport itself is covered by tests/browser/lib.test.js; this only
    // pins that the handler still consults it before touching the report.
    assert!(
        guard.contains("if (!acceptsReport(topic, participant)) return"),
        "report handler must stay gated on acceptsReport"
    );

    // LiveKit stream attribute names, which the Rust side sets in
    // transcript_stream_options.
    let transcript = compact(transcript);
    for snippet in [
        r#"attrs["lk.segment_id"]"#,
        r#"attrs["lk.transcription_final"] === "true""#,
        r#"participant?.identity === room.localParticipant.identity ? "you" : "interviewer""#,
        "updateTranscriptSegment(id, speaker, text",
    ] {
        assert!(
            transcript.contains(&compact(snippet)),
            "missing static transcript contract: {snippet}"
        );
    }

    // The report card and markdown export are asserted behaviorally in
    // tests/browser/render.test.js. What only Rust can check is that the
    // browser still routes a report through the sanitizer before rendering it.
    assert_eq!(call_arguments(report, "sanitizeReport("), ["raw"]);
    let report = compact(report);

    // The first call above is the provisional branch, which returns early; the
    // report that is rendered goes through its own.
    assert!(report.contains(&compact("state.report = sanitizeReport(raw);")));
    assert!(report.contains("constraw=JSON.parse(newTextDecoder().decode(payload))"));
    for snippet in [
        "setLocalAudioEnabled(false)",
        "saveHistory()",
        "renderReport()",
        "room?.disconnect()",
        "state.room = null",
    ] {
        assert!(
            report.contains(&compact(snippet)),
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

    // The shared request fields follow the mode-specific problem selection.
    // Strict assignment must retain the optional execution restriction too.
    assert!(compact(&source).contains(
        "...(namedDuration?{durationMin}:{}),interviewId,interviewLoop,interviewMode:mode,...(!whiteboard?{language:state.language}:{}),interviewProfile,...(interviewGrounding?{interviewGrounding}:{}),...(nodes.interviewerAccent.value?{interviewerAccent:nodes.interviewerAccent.value}:{}),...(nodes.interviewerVoice.value?{interviewerVoice:nodes.interviewerVoice.value}:{}),...(nodes.hideExamples.checked?{hideExamples:true}:{}),...(exampleDrawingEnabled?{exampleDrawing:true}:{}),...(candidateLedEnabled?{candidateLed:true}:{}),...(!editorOptions.execution?{codeExecution:false}:{})})"
    ));
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
        (
            "fixed-capacity-ring-buffer",
            "wrap after removing the oldest",
        ),
        (
            "fixed-capacity-ring-buffer",
            "rejected push keeps later order",
        ),
        ("two-sum", "duplicates"),
        ("merge-sorted-array", "duplicates"),
        ("merge-sorted-array", "trailing zero in valid prefix"),
        ("merge-sorted-array", "leading zero in valid prefix"),
        ("merge-sorted-array", "leading zero changes position"),
        ("merge-sorted-array", "trailing zero in nums2"),
        ("merge-sorted-array", "leading zero in nums2"),
        ("merge-sorted-array", "zero in both inputs"),
        ("merge-sorted-array", "repeated zeros in both inputs"),
        ("remove-element", "remove every"),
        ("remove-element", "last unchecked element is the target"),
        ("remove-element", "only element is the target"),
        ("remove-duplicates-from-sorted-array", "several duplicate"),
        ("remove-duplicates-from-sorted-array-ii", "single repeated"),
        ("majority-element", "not the first"),
        ("rotate-array", "k larger"),
        ("best-time-to-buy-and-sell-stock", "decreasing"),
        ("best-time-to-buy-and-sell-stock-ii", "multiple small rises"),
        ("happy-number", "single digit that settles"),
        ("happy-number", "seven steps to one"),
        ("happy-number", "reaches seven before one"),
        ("jump-game", "late unreachable"),
        ("jump-game-ii", "three hops needed"),
        ("jump-game-ii", "two pads with room to spare"),
        ("jump-game-ii", "three pads with room to spare"),
        ("jump-game-ii", "range grows after the first pad"),
        ("jump-game-ii", "weaker pad sits closer to the end"),
        ("jump-game-ii", "two pads in range both reach farther"),
        ("jump-game-ii", "one pad with charge remaining"),
        ("jump-game-ii", "two pads with just enough range"),
        ("jump-game-ii", "empty pads between charging stops"),
        (
            "jump-game-ii",
            "three charged pads followed by two empty pads",
        ),
        ("jump-game-ii", "second pad has room beyond the destination"),
        ("jump-game-ii", "last pad shares the second flight's range"),
        (
            "jump-game-ii",
            "last pad in range beside a farther-reaching pad",
        ),
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
        ("longest-common-prefix", "middle string breaks prefix"),
        ("longest-common-prefix", "same substring appears later"),
        ("longest-common-prefix", "later string ends first"),
        ("longest-common-prefix", "middle differs at second letter"),
        ("longest-common-prefix", "matching column after mismatch"),
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
            "left edge must not move backward",
        ),
        (
            "longest-substring-without-repeating-characters",
            "repeat in the middle of the stretch",
        ),
        (
            "longest-substring-without-repeating-characters",
            "left edge jumps more than one key",
        ),
        (
            "longest-substring-without-repeating-characters",
            "longest stretch away from both ends",
        ),
        (
            "longest-substring-without-repeating-characters",
            "every key distinct",
        ),
        (
            "longest-substring-without-repeating-characters",
            "case and symbols count",
        ),
        (
            "longest-substring-without-repeating-characters",
            "space before and after the keys",
        ),
        (
            "longest-substring-without-repeating-characters",
            "leading space only",
        ),
        (
            "longest-substring-without-repeating-characters",
            "trailing space only",
        ),
        ("minimum-window-substring", "duplicate required"),
        (
            "substring-with-concatenation-of-all-words",
            "duplicate words",
        ),
        ("minimum-size-subarray-sum", "no qualifying window"),
        ("minimum-size-subarray-sum", "middle pair exceeds target"),
        (
            "minimum-size-subarray-sum",
            "one early file and a later run",
        ),
        (
            "minimum-size-subarray-sum",
            "exact prefix and larger final pair",
        ),
        ("valid-sudoku", "duplicate in box only"),
        ("valid-sudoku", "duplicate in last column only"),
        ("valid-sudoku", "duplicate in last row only"),
        ("valid-sudoku", "duplicate in first column only"),
        ("valid-sudoku", "duplicate in first row only"),
        ("valid-sudoku", "duplicate in far box only"),
        ("valid-sudoku", "duplicate in edge box only"),
        ("valid-sudoku", "another partial board"),
        ("valid-sudoku", "fully filled board"),
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
        ("insert-interval", "no existing blocks"),
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
        ("basic-calculator", "minus before a group"),
        ("basic-calculator", "minus inside a group"),
        ("basic-calculator", "minus on the first number only"),
        ("basic-calculator", "space after a leading minus"),
        ("basic-calculator", "minus before a two-digit number"),
        ("basic-calculator", "number added after a negated group"),
        ("basic-calculator", "space before a minus inside a group"),
        ("search-in-rotated-sorted-array", "last entry after a wrap"),
        (
            "search-in-rotated-sorted-array",
            "first entry before a wrap",
        ),
        ("search-in-rotated-sorted-array", "target in a single entry"),
        (
            "search-in-rotated-sorted-array",
            "negative target after a wrap",
        ),
        ("search-in-rotated-sorted-array", "last entry, not wrapped"),
        (
            "search-in-rotated-sorted-array",
            "first entry in a wrapped quartet",
        ),
        (
            "search-in-rotated-sorted-array",
            "wrapped pair with target first",
        ),
        (
            "search-in-rotated-sorted-array",
            "smallest entry after a wrap",
        ),
        (
            "search-in-rotated-sorted-array",
            "target between entries before a wrap",
        ),
        (
            "search-in-rotated-sorted-array",
            "target between entries after a wrap",
        ),
        (
            "search-in-rotated-sorted-array",
            "last entry before a five-entry wrap",
        ),
        (
            "search-in-rotated-sorted-array",
            "smallest entry in a six-entry wrap",
        ),
        (
            "search-in-rotated-sorted-array",
            "smallest entry in a seven-entry wrap",
        ),
        (
            "search-in-rotated-sorted-array",
            "last entry before a six-entry wrap",
        ),
        (
            "search-in-rotated-sorted-array",
            "pair not wrapped with target last",
        ),
        (
            "search-in-rotated-sorted-array",
            "target between the wrap and the midpoint",
        ),
        (
            "search-in-rotated-sorted-array",
            "target past the midpoint before the wrap",
        ),
        (
            "search-in-rotated-sorted-array",
            "target before the midpoint of a dump wrapped at its end",
        ),
        (
            "search-in-rotated-sorted-array",
            "target before the midpoint of a seven-entry dump wrapped at its end",
        ),
        (
            "search-in-rotated-sorted-array",
            "entry just past the midpoint of a seven-entry wrap",
        ),
        (
            "search-in-rotated-sorted-array",
            "wrapped pair with target last",
        ),
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
        ("merge-intervals", "chained overlaps"),
        ("merge-intervals", "later window inside the first"),
        ("merge-intervals", "four windows in one chain"),
        ("merge-intervals", "overlap after a separate first window"),
        ("merge-intervals", "overlapping windows apart in the input"),
        ("merge-intervals", "window joining two earlier ones"),
        ("merge-intervals", "later window inside a second window"),
        (
            "merge-intervals",
            "touching endpoints after a separate first window",
        ),
        ("merge-intervals", "zero-length and zero-end windows"),
        ("merge-intervals", "longer window first at a shared start"),
        ("merge-intervals", "window starting at the time limit"),
        ("lru-cache", "insert after an overwrite at capacity"),
        ("lru-cache", "overwrite the newest key at capacity"),
        ("lru-cache", "zero is a stored value"),
        ("lru-cache", "refresh from the middle at capacity 3"),
        ("lru-cache", "rewrite with the same value at capacity"),
        ("lru-cache", "overwrite a stored zero at capacity 3"),
        ("valid-parentheses", "interleaved"),
        ("add-binary", "wide value plus one"),
        ("add-binary", "wide odd value plus zero"),
        ("add-binary", "wide all ones plus one"),
        ("add-binary", "two wide values with multi-bit carry"),
        ("add-binary", "maximum length all ones plus one"),
        ("factorial-trailing-zeroes", "one fewer scheduled job"),
        ("factorial-trailing-zeroes", "medium scheduling batch"),
        ("factorial-trailing-zeroes", "two hundred scheduled jobs"),
        ("factorial-trailing-zeroes", "thousand scheduled jobs"),
        ("factorial-trailing-zeroes", "large odd batch"),
        ("factorial-trailing-zeroes", "half capacity"),
        ("factorial-trailing-zeroes", "maximum job count"),
        ("triangle", "cheapest values not connected"),
        ("triangle", "cheaper step misses the deep saving"),
        (
            "best-time-to-buy-and-sell-stock-iii",
            "best trades span a dip",
        ),
        (
            "best-time-to-buy-and-sell-stock-iii",
            "small rise then two equal rises",
        ),
        ("word-pattern", "fewer words than letters"),
        ("word-pattern", "extra word repeats an earlier one"),
        ("word-pattern", "alternating words for a mirrored template"),
        ("word-pattern", "new letter takes a word already used"),
        ("word-pattern", "third letter reuses the first word"),
        (
            "word-pattern",
            "one-letter words named like template letters",
        ),
        ("word-pattern", "one word repeated 130 times"),
        ("word-pattern", "second word extends the first"),
        ("word-pattern", "second word shortens the first"),
        ("word-pattern", "second word reorders the first"),
        ("word-pattern", "words differ only at the ninth letter"),
        ("word-pattern", "second word is constructor"),
        ("word-pattern", "last letter has no word"),
        (
            "word-pattern",
            "twelfth word is new where its letter repeats",
        ),
        (
            "longest-palindromic-substring",
            "mirrored ends: s=\"abacdfgdcaba\"",
        ),
        (
            "longest-palindromic-substring",
            "whole string even: s=\"abba\"",
        ),
        (
            "longest-palindromic-substring",
            "matching prefix: s=\"aab\"",
        ),
        (
            "longest-palindromic-substring",
            "matching suffix: s=\"baa\"",
        ),
        ("longest-palindromic-substring", "mixed case: s=\"aA\""),
        (
            "longest-palindromic-substring",
            "digit at the centre: s=\"ab1ba\"",
        ),
        (
            "longest-palindromic-substring",
            "capital at the centre: s=\"abBba\"",
        ),
        (
            "longest-palindromic-substring",
            "middle mirror: s=\"abacdedcfgf\"",
        ),
        (
            "longest-palindromic-substring",
            "even mirror beats odd mirror: s=\"abacca\"",
        ),
        (
            "longest-palindromic-substring",
            "odd mirror beats even mirror: s=\"aabcb\"",
        ),
        (
            "longest-palindromic-substring",
            "run of three at the centre: s=\"baaab\"",
        ),
        (
            "longest-palindromic-substring",
            "run of four at the centre: s=\"baaaab\"",
        ),
        (
            "longest-palindromic-substring",
            "two equal characters: s=\"aa\"",
        ),
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
    assert!(compact(&source).contains(
        r#"setAgentStateLabel(labels[value]||providerUiState("live").label,value==="listening")"#
    ));
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
        .find("connectLiveKit(connection, preflight, presenting)")
        .expect("the page joins the room");
    assert!(compact(&interview).contains(
        "awaitPromise.race([connectLiveKit(connection,preflight,presenting),startupFailed])"
    ));
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

    // The fallback the query string gets when it names no usable duration: the
    // literal after the colon of `namedDuration ? clamp(...) : <fallback>`.
    let declaration = &interview[interview.find("let durationMin = ").unwrap()..];
    let fallback = declaration[..declaration.find(';').unwrap()]
        .rsplit(':')
        .next()
        .expect("the declaration falls back to a literal")
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

    // The preselected button is what a candidate who touches nothing sees until
    // `/api/session` names the operator's default.
    let selected = page
        .split("duration-button selected\"")
        .nth(1)
        .expect("one duration button is preselected");
    assert_eq!(
        browser_number(selected, "data-duration=\"", '"'),
        DEFAULT_DURATION_MIN,
        "the preselected lobby duration is not the built-in default"
    );

    // The lobby derives its length from the checked difficulty, so the two
    // assertions above are only worth anything if exactly one box starts
    // checked: two would make the derived length ambiguous, and none would
    // leave `suggestedDuration` deciding from an empty set.
    //
    // What that box derives is checked in the browser rather than here, by "an
    // untouched lobby keeps the built-in length when the answer names none" in
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

#[path = "contract_parts/browser_check_runner.rs"]
mod browser_check_runner;
include!("contract_parts/browser_checks_and_long_interview_range.rs");
