use std::sync::Arc;

use tokio::net::TcpListener;

use super::*;

/// A refused review is the learner's to be told is unavailable, and the
/// operator's to be told why: a model that keeps failing validation is not a
/// model that never answered. The reason is bounded and on one line, since it
/// is written into a log.
#[test]
fn a_refused_review_attempt_says_which_attempt_and_why_on_one_line() {
    let note = refusal_note("room-a", 1, "the answer was refused: gaps\nwere not closed");
    assert_eq!(
        note,
        "task review room=room-a attempt 1 refused: the answer was refused: gaps\\nwere not closed"
    );
    let long = refusal_note("room-a", 2, &"x".repeat(1000));
    assert!(long.ends_with("..."), "{long}");
    assert!(long.chars().count() < 260, "{}", long.chars().count());
}

/// A refusal of several places is noted by whole places, as many as fit, and a
/// count of the rest, while the repair is told all five places of this one.
/// Cut at the bound, this snake_case review was noted with a key cut midway
/// and nothing to say that three more dimensions were refused.
#[tokio::test]
async fn a_refusal_note_keeps_whole_places_and_counts_the_rest() {
    let mut session =
        crate::tasks::session::TaskSession::new(crate::tasks::test_support::admitted().await, 100);
    session.start(100);
    let (mut value, _, _) = reviews();
    let rename = crate::tasks::test_support::rename;
    rename(&mut value, "nextActions", "next_actions");
    rename(
        &mut value["dimensions"],
        "reasoningParticipation",
        "reasoning_participation",
    );
    for name in [
        "implementationOwnership",
        "testingAndDiagnosis",
        "revisionAndImprovement",
    ] {
        rename(
            &mut value["dimensions"][name],
            "insufficientReason",
            "insufficient_reason",
        );
    }
    let places = crate::tasks::report::refusals(&session, &value, "").unwrap_err();
    assert_eq!(places.len(), 5);
    let (refused, note) = judged(&session, &value.to_string(), "", "room-a").unwrap_err();
    assert_eq!(refused, format!("{REFUSED}{}", places.join("; ")));
    assert_eq!(
        refusal_note("room-a", 0, &note),
        format!(
            "task review room=room-a attempt 0 refused: the answer was refused: {}; and 4 more",
            places[0]
        )
    );

    // And the loop logs that note, not the refusal the repair is told.
    let (review, _, _) = reviews();
    let (accepted, _, logged) = repaired(&[&value, &review]).await;
    assert!(accepted, "the repair is accepted");
    assert_eq!(
        logged,
        [format!(
            "task review room=task-review-room attempt 0 refused: the answer was refused: {}; and 4 more",
            places[0]
        )]
    );
}

/// One refusal reaches the repair with at most `MAX_REFUSALS` places and a
/// count of the rest, however many places the answer is refused at.
#[tokio::test]
async fn a_repair_is_told_the_capped_places_and_a_count() {
    let mut session =
        crate::tasks::session::TaskSession::new(crate::tasks::test_support::admitted().await, 100);
    session.start(100);
    let (mut value, _, _) = reviews();
    for name in [
        "reasoningParticipation",
        "implementationOwnership",
        "testingAndDiagnosis",
        "revisionAndImprovement",
    ] {
        let references: Vec<Value> = (0..8)
            .map(|n| {
                let mut reference = json!({"kind": "turn", "id": "turn-1"});
                reference[format!("k{n}")] = json!(1);
                reference
            })
            .collect();
        value["dimensions"][name]["evidence"] = json!(references);
    }
    let places = crate::tasks::report::refusals(&session, &value, "").unwrap_err();
    assert_eq!(places.len(), 32);
    let (refused, _) = judged(&session, &value.to_string(), "", "room-a").unwrap_err();
    assert_eq!(
        refused,
        format!("{REFUSED}{}", crate::tasks::report::joined(&places))
    );
    assert!(refused.ends_with("; and 24 more"), "{refused}");
}

/// Whole places fill the note up to its bound and not one character past it,
/// and a first place too long for the note is cut with the rest still counted.
#[test]
fn a_refusal_note_is_cut_only_inside_a_place_too_long_for_it() {
    let lead = REFUSED;
    let fit = vec!["a".repeat(150), "b".repeat(24)];
    assert_eq!(noted(&fit), format!("{lead}{}; {}", fit[0], fit[1]));
    assert_eq!(noted(&fit).chars().count(), MAX_NOTE_CHARS);
    let past = vec!["a".repeat(150), "b".repeat(25)];
    assert_eq!(noted(&past), format!("{lead}{}; and 1 more", past[0]));
    let long = vec!["\u{3b1}".repeat(300), "y".to_owned()];
    let cut = noted(&long);
    assert_eq!(cut.chars().count(), MAX_NOTE_CHARS);
    assert!(cut.starts_with(&format!("{lead}\u{3b1}\u{3b1}")), "{cut}");
    assert!(cut.ends_with("\u{3b1}...; and 1 more"), "{cut}");
    assert!(refusal_note("room-a", 0, &cut).ends_with("; and 1 more"));
}

/// The answer's size is checked at the limit itself: one at it is read, one
/// past it is refused. A comparison that was off by one in either direction
/// would refuse a review that fits or accept one that does not.
#[test]
fn an_answer_is_read_up_to_the_limit_and_refused_one_past_it() {
    assert!(fits_the_limit(&"x".repeat(MAX_REPORT_RESPONSE_BYTES)));
    assert!(!fits_the_limit(&"x".repeat(MAX_REPORT_RESPONSE_BYTES + 1)));
    assert!(fits_the_limit(""));
}

/// A valid review, and two that the validator refuses: one missing `gaps`, one
/// with an unknown `summary` whose text must never reach a prompt.
fn reviews() -> (Value, Value, Value) {
    let dimension = json!({"rating": null, "reason": "No reliable opportunity was captured.",
        "insufficientReason": "missing_turns", "support": "none", "evidence": []});

    // Named here rather than read from the validator, which is what is being
    // exercised.
    let dimensions: serde_json::Map<_, _> = [
        "reasoningParticipation",
        "implementationOwnership",
        "testingAndDiagnosis",
        "revisionAndImprovement",
    ]
    .into_iter()
    .map(|name| (name.to_owned(), dimension.clone()))
    .collect();
    let review = json!({"dimensions": dimensions,
        "gaps": [], "nextActions": ["Trace a boundary input and explain the state."]});
    let mut missing = review.clone();
    missing.as_object_mut().unwrap().remove("gaps");
    let mut unknown = review.clone();
    unknown["summary"] = json!("REJECTED_SUMMARY_TEXT");
    (review, missing, unknown)
}

const NOTE: &str = "A prior response failed the schema";
const MISSING: &str = r#"learning review is missing \"gaps\""#;
const UNKNOWN: &str = r#"learning review has unknown field \"summary\""#;

/// A refusal as the loop records it, inside the JSON array a repair carries.
fn refused(reason: &str) -> String {
    format!(r#""the answer was refused: {reason}""#)
}

/// Each repair carries the repair note and the reasons every attempt before it
/// was refused, oldest first: at temperature 0, repairs that resent one fixed
/// prompt got the same answer back every time (#744). The reasons travel as a
/// JSON array of strings, and the rejected answer's own text never does.
#[tokio::test]
async fn a_rejected_task_review_is_asked_for_again_with_the_repair_note() {
    let (review, missing, unknown) = reviews();
    // Served in order: the missing key first, then the unknown one.
    let (accepted, observed, _) = repaired(&[&missing, &unknown, &review]).await;
    assert!(accepted, "the second repair is accepted");
    assert_eq!(observed.len(), 3);
    assert!(!observed[0].contains(NOTE) && !observed[0].contains(MISSING));
    for prompt in &observed[1..] {
        assert!(prompt.starts_with(observed[0].as_str()) && prompt.contains(NOTE));
    }
    assert!(observed[1].contains(&format!("[{}]", refused(MISSING))));
    assert!(observed[2].contains(&format!("[{},{}]", refused(MISSING), refused(UNKNOWN))));
    assert!(
        observed
            .iter()
            .all(|prompt| !prompt.contains("REJECTED_SUMMARY_TEXT"))
    );
}

/// Two refusals for the same reason still give the last repair a prompt it has
/// not seen: with only the last reason it was byte for byte the one before it,
/// and at temperature 0 that call could only get the same answer back.
#[tokio::test]
async fn a_review_refused_twice_alike_is_told_both_times() {
    let (review, _, unknown) = reviews();
    let (accepted, observed, _) = repaired(&[&unknown, &unknown, &review]).await;
    assert!(accepted, "the second repair is accepted");
    assert_eq!(observed.len(), 3);
    assert_ne!(observed[1], observed[2]);
    assert!(observed[2].contains(&format!("[{},{}]", refused(UNKNOWN), refused(UNKNOWN))));
}

/// A rejected answer never reaches a later prompt, but for a key that is a
/// plain identifier, which the refusal names on purpose. A value the model
/// wrote and a key that is not an identifier can carry implementation code,
/// so neither is repeated, and the named key shows the repairs carry the
/// refusal at all.
#[tokio::test]
async fn task_feedback_exhausts_repairs_without_reusing_rejected_provider_content() {
    let session =
        crate::tasks::session::TaskSession::new(crate::tasks::test_support::admitted().await, 100);
    let observed = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let captured = Arc::clone(&observed);
    let answer = json!({"hire": true, "private": "REJECTED_PROVIDER_IMPLEMENTATION",
        "int main() { REJECTED_KEY }": 1})
    .to_string();
    let app = axum::Router::new().route(
        "/",
        axum::routing::post(move |axum::Json(request): axum::Json<Value>| {
            let captured = Arc::clone(&captured);
            let answer = answer.clone();
            async move {
                captured.lock().unwrap().push(request);
                axum::Json(json!({"candidates":[{"content":{"parts":[{"text": answer}]}}]}))
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let keys = GeminiKeys::from_config(&crate::gemini::tests::live_config(&[(
        "GOOGLE_API_KEYS",
        "task-repair-test-key",
    )]));
    let review = generate_task_feedback_at(
        &keys,
        &url,
        &session,
        "private reference",
        "task-repair-room",
    )
    .await;
    server.abort();
    assert!(review.is_none());
    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 1 + MAX_REPORT_REPAIRS);
    // Thinking would spend the output budget and truncate the JSON.
    assert!(
        observed.iter().all(|request| {
            request["generationConfig"]["thinkingConfig"]["thinkingBudget"] == 0
        })
    );
    for marker in ["REJECTED_PROVIDER_IMPLEMENTATION", "REJECTED_KEY"] {
        assert!(
            observed
                .iter()
                .all(|request| !request.to_string().contains(marker)),
            "{marker}"
        );
    }
    let named = |request: &Value| {
        request["contents"][0]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains(r#"\"hire\""#)
    };
    assert!(!named(&observed[0]));
    assert!(observed[1..].iter().all(named));
}

/// Runs the review against a local stand-in that serves `served` in order, and
/// returns whether the last of them was the review accepted, with every prompt
/// sent and every line logged.
async fn repaired(served: &[&Value]) -> (bool, Vec<String>, Vec<String>) {
    let mut session =
        crate::tasks::session::TaskSession::new(crate::tasks::test_support::admitted().await, 100);
    session.start(100);
    let last = served.last().expect("at least one answer");
    let expected = crate::tasks::report::stamp(&session, last, "task-review-room");

    // Popped from the end, so reversed here.
    let answers = Arc::new(std::sync::Mutex::new(
        served
            .iter()
            .rev()
            .map(|answer| answer.to_string())
            .collect::<Vec<_>>(),
    ));
    let observed = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&observed);
    let app = axum::Router::new().route(
        "/",
        axum::routing::post(move |axum::Json(request): axum::Json<Value>| {
            let captured = Arc::clone(&captured);
            let answers = Arc::clone(&answers);
            async move {
                let prompt = request["contents"][0]["parts"][0]["text"].as_str().unwrap();
                captured.lock().unwrap().push(prompt.to_owned());
                let text = answers.lock().unwrap().pop().unwrap();
                axum::Json(json!({"candidates":[{"content":{"parts":[{"text": text}]}}]}))
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let keys = GeminiKeys::from_config(&crate::gemini::tests::live_config(&[(
        "GOOGLE_API_KEYS",
        "task-review-key",
    )]));
    let mut logged = Vec::new();
    let result = reviewed_at(&keys, &url, &session, "", "task-review-room", |line| {
        logged.push(line);
    })
    .await;
    server.abort();
    let observed = observed.lock().unwrap().clone();
    (result == Some(expected), observed, logged)
}
