//! The `tests` module of `src/livekit/board.rs`, which declares this file by
//! path. Everything here reaches into that file through `super`, so it is a
//! unit test and not an integration test: private items are in scope.

use super::*;

const CANDIDATE: &str = "candidate-4f2c";

fn refusal(
    mode: InterviewMode,
    topic: &str,
    sender: &str,
    declared_length: Option<u64>,
) -> Option<&'static str> {
    board_stream_refusal(mode, topic, sender, CANDIDATE, declared_length)
}

#[test]
fn accepts_a_board_from_the_candidate() {
    assert_eq!(
        refusal(
            InterviewMode::Whiteboard,
            TOPIC_BOARD_IMAGE,
            CANDIDATE,
            Some(96_318)
        ),
        None
    );

    // A browser that did not declare a length still gets a board through: the
    // ceiling is then enforced while reading, which is what `drain` does.
    assert_eq!(
        refusal(
            InterviewMode::Whiteboard,
            TOPIC_BOARD_IMAGE,
            CANDIDATE,
            None
        ),
        None
    );
}

#[test]
fn refuses_a_board_no_interview_asked_for() {
    // The editor's own interview has no board, so a stream on this topic is a
    // client sending one anyway.
    assert_eq!(
        refusal(
            InterviewMode::Coding,
            TOPIC_BOARD_IMAGE,
            CANDIDATE,
            Some(4_096)
        ),
        Some("this interview has no whiteboard")
    );
    assert_eq!(
        refusal(
            InterviewMode::Whiteboard,
            "lk.agent.pre-connect-audio-buffer",
            CANDIDATE,
            Some(4_096)
        ),
        Some("not the board topic")
    );
}

#[test]
fn refuses_a_board_from_anyone_but_the_candidate() {
    // Every participant holds `canPublishData`, so the identity is the only
    // thing between the interviewer's eyes and a board an observer drew.
    assert_eq!(
        refusal(
            InterviewMode::Whiteboard,
            TOPIC_BOARD_IMAGE,
            "observer-9a11",
            Some(4_096)
        ),
        Some("not the candidate")
    );
}

#[test]
fn refuses_a_header_larger_than_a_board() {
    assert_eq!(
        refusal(
            InterviewMode::Whiteboard,
            TOPIC_BOARD_IMAGE,
            CANDIDATE,
            Some(MAX_BOARD_BYTES as u64 + 1)
        ),
        Some("declared larger than a board may be")
    );

    // The ceiling itself is allowed: a bound refused at its own value is a
    // different bound from the one the browser is written against.
    assert_eq!(
        refusal(
            InterviewMode::Whiteboard,
            TOPIC_BOARD_IMAGE,
            CANDIDATE,
            Some(MAX_BOARD_BYTES as u64)
        ),
        None
    );
}

/// The browser's own output, not a restatement of it: the cases come from
/// `web/lib.js` by way of the wire-fixture generator, so a producer that
/// starts spelling the attribute differently fails here rather than in an
/// interview.
#[test]
fn reads_the_stroke_count_the_browser_sent() {
    let fixture: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string("tests/fixtures/board-stream.json")
            .expect("the board wire fixture is generated into tests/fixtures"),
    )
    .expect("the fixture is JSON");
    assert_eq!(
        fixture["topic"].as_str(),
        Some(TOPIC_BOARD_IMAGE),
        "the browser and the agent disagree about the board's topic"
    );
    let cases = fixture["cases"].as_array().expect("cases is an array");

    // Asserted, because a fixture that lost its cases would otherwise pass
    // this test by having nothing to check.
    assert_eq!(cases.len(), 3);
    let counts = cases
        .iter()
        .map(|case| {
            let attributes = case["options"]["attributes"]
                .as_object()
                .expect("a stream header carries attributes")
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().expect("attributes are strings").to_string(),
                    )
                })
                .collect::<HashMap<_, _>>();
            strokes_from_attributes(&attributes)
        })
        .collect::<Vec<_>>();
    assert_eq!(counts, vec![3, 214, 0]);
}

#[test]
fn a_board_with_no_usable_count_still_arrives() {
    assert_eq!(strokes_from_attributes(&HashMap::new()), 0);
    assert_eq!(
        strokes_from_attributes(&HashMap::from([(
            "strokes".to_string(),
            "many".to_string()
        )])),
        0
    );
    assert_eq!(
        strokes_from_attributes(&HashMap::from([("strokes".to_string(), "-4".to_string())])),
        0
    );
}
