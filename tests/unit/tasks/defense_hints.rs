//! Hints in a defense: the package's budget, the slot's record of them, the
//! cap they put on an item's marks, and when Jim may be granted one.

use serde_json::{Value, json};

use super::tests::{config, student_repo};
use super::*;

/// A package that allows hints lets Jim reserve them on opened items until the
/// budget is spent, and each caps its item's suggested marks at 2; one that
/// allows none refuses them, as a strict exam.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn hints_are_reserved_within_the_budget_and_cap_their_item() {
    use crate::web::defense::{BeginRequest, begin, with_attempt};

    let repo = student_repo("hints");
    let mut package = config();
    package["hints"] = json!(2);
    let slot = prepare(Config::parse(&package).unwrap(), repo.path()).unwrap();
    slot.set_open(true);
    slot.set_items_open(true);
    let item = slot.items()[0].clone();
    let input: BeginRequest = serde_json::from_value(json!({"item": item})).unwrap();
    let opened = with_attempt(slot.clone(), move |attempt, slot| {
        begin(input, attempt, slot)
    })
    .await;
    assert_eq!(opened.status().as_u16(), 200);
    // The slot counts within the budget: on the item, and in all.
    assert_eq!(slot.give_hint(&item), Ok((1, 1)));
    assert_eq!(slot.give_hint(&item), Ok((2, 2)));
    assert_eq!(slot.give_hint(&item), Err("hints_spent"), "two in all");
    assert_eq!(slot.give_hint("bug"), Err("hints_spent"));
    let hints = slot.hints();
    assert_eq!((hints.allowed, hints.given[&item]), (2, 2));
    assert!(
        slot.progress()
            .contains(&format!("{item} opened, no run yet, 2 hints given")),
        "a replacement connection is told the hints given: {}",
        slot.progress()
    );

    // The result says how many each item had, and caps its marks.
    let mut facts = slot.kept();
    // A conclusive first run: part b would be 3 without the hint.
    facts["items"][0]["runs"] = json!([{"startedAt": 1, "endedAt": 2, "matched": true,
        "conclusive": true, "outcome": {"kind": "tests", "failed": ["p.ASpec"]}}]);
    let explanations: Vec<Value> = slot
        .items()
        .iter()
        .map(|id| json!({"item": id, "rating": 3, "account": 3, "reason": "r", "evidence": []}))
        .collect();
    let section = record::marked(facts, &json!(explanations), &hints, false, true);
    let hinted = &section["items"][0];
    assert_eq!(hinted["hints"], 2);
    assert_eq!(
        hinted["explain"]["suggestedMark"], 2,
        "a hinted item is 2 at most"
    );
    assert_eq!(section["items"][1]["explain"]["suggestedMark"], 3);
    // The cap is the item's: part b too, had it a mark above 2.
    assert_eq!(
        hinted["predict"]["suggestedMark"], 2,
        "part b is capped too"
    );
    assert_eq!(section["hintsAllowed"], 2);

    // A hint on the bug item caps its mark as well: a claim with the cycle and
    // the exercise right is 3 without one.
    facts = slot.kept();
    facts["bug"]["checks"] = json!({"cycleClose": true, "exerciseMatches": true});
    let unhinted = record::marked(facts.clone(), &json!(explanations), &hints, false, true);
    assert_eq!(unhinted["bug"]["suggestedMark"], 3);
    let mut on_bug = hints;
    on_bug.given.insert("bug".to_owned(), 1);
    let capped = record::marked(facts, &json!(explanations), &on_bug, false, true);
    assert_eq!(
        (
            capped["bug"]["suggestedMark"].clone(),
            capped["bug"]["hints"].clone()
        ),
        (json!(2), json!(1))
    );

    // A package with no hints keeps the strict exam.
    let strict = prepare(
        Config::parse(&config()).unwrap(),
        student_repo("strict").path(),
    )
    .unwrap();
    assert_eq!(strict.hints_allowed(), 0);
    assert_eq!(strict.give_hint(&item), Err("hints_spent"));
    let mut too_many = config();
    too_many["hints"] = json!(4);
    assert!(Config::parse(&too_many).is_err());
}

/// Jim is granted a hint only for an attempt: after the learner has spoken
/// since the item was opened, and not twice without an answer between; each
/// is recorded against the item and not as a task's hint rung.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_hint_needs_an_answer_since_the_item_opened_and_the_last_hint() {
    use crate::tasks::access::{Admission, Preparation, TaskService};
    use crate::tasks::test_support::{SITE, assignment, preparation};
    use crate::web::defense::{BeginRequest, begin, with_attempt};

    let repo = student_repo("hint-session");
    let mut package = config();
    package["hints"] = json!(3);
    let plaintext = json!({"packageVersion": 1, "setId": "mycpu", "setVersion": 1,
        "rules": {"durationMin": 60, "maxHintRungs": 0, "closesAt": null,
            "lookAwaySeconds": 8, "rulesNote": null},
        "defense": package});
    let set = crate::tasks::package::decode("mycpu", 1, &serde_json::to_vec(&plaintext).unwrap())
        .unwrap();
    let service = TaskService::new();
    service.insert_unlocked(1, SITE, std::sync::Arc::new(set));
    let assignment = assignment("mycpu", 1);
    let defense_config = service.defense_config(1, &assignment).unwrap();
    let slot = prepare((*defense_config).clone(), repo.path()).unwrap();
    let Admission::New(task) = service
        .begin_admission(
            1,
            ("learner", 42),
            &assignment,
            TASK_ID,
            "hint-key",
            Preparation {
                defense: Some(slot.clone()),
                ..preparation()
            },
            100,
        )
        .unwrap()
    else {
        panic!("a first admission is new")
    };
    // Begin stamps the real clock, so the session runs on it too.
    let now = crate::current_epoch_seconds();
    let mut session = crate::tasks::session::TaskSession::new(task, now);
    session.start(now);
    slot.set_open(true);
    slot.set_items_open(true);
    let item = slot.items()[0].clone();

    // Not before the item is opened, nor before the learner answers in it.
    assert_eq!(
        session.defense_hint(&item, Some(now)),
        Err("item_not_begun")
    );
    assert_eq!(session.defense_hint("q9", Some(now)), Err("item_not_begun"));
    session.observe_turn("turn-1", "I'm ready.", now);
    let input: BeginRequest = serde_json::from_value(json!({"item": item})).unwrap();
    let opened = with_attempt(slot.clone(), move |attempt, slot| {
        begin(input, attempt, slot)
    })
    .await;
    assert_eq!(opened.status().as_u16(), 200);
    let opened_at = slot.begun_at(&item).unwrap();
    assert!(
        session.defense_hint(&item, None).is_err(),
        "no answer since the item opened"
    );
    assert!(
        session.defense_hint(&item, Some(opened_at)).is_err(),
        "speech begun before the item opened, or in its second, answers something else"
    );
    session.observe_turn_begun("turn-1b", "Ready when you are.", opened_at, opened_at + 1);
    assert!(
        session.defense_hint(&item, None).is_err(),
        "nor once it is captured after the item opened"
    );

    // Jim reserves the hint while replying to an answer not yet captured: the
    // learner's speech counts, once.
    let granted = session.defense_hint(&item, Some(opened_at + 1)).unwrap();
    assert_eq!(granted["supportRequestId"], "defense-hint-1");
    assert_eq!(
        session.hint_rungs_used, 0,
        "a defense hint is not a task's hint rung"
    );
    assert!(
        session.defense_hint(&item, Some(opened_at + 1)).is_err(),
        "the same answer earns one hint"
    );
    // That answer is captured when the reply completes: still no new answer.
    session.observe_turn("turn-2", "It says the core is out of reset.", opened_at + 1);
    assert!(
        session.defense_hint(&item, None).is_err(),
        "never two without an answer between"
    );
    session.observe_turn("turn-3", "It also checks halted.", opened_at + 2);
    assert_eq!(
        session.defense_hint(&item, None).unwrap()["supportRequestId"],
        "defense-hint-2"
    );
    assert_eq!(slot.hints().given[&item], 2);
    // Once work is over no hint is given, however many remain.
    session.observe_turn("turn-4", "And the trap vector.", opened_at + 3);
    session.phase = crate::tasks::session::Phase::WrapUp;
    assert_eq!(
        session.defense_hint(&item, None),
        Err("hints are given only while the items are under way")
    );
    session.phase = crate::tasks::session::Phase::Work;

    // The review may rate a dimension 2 with the hint, citing an answer after
    // it; an answer before every hint is no provenance; and an independent 3
    // after a hint stands, a hint being about one item.
    let rows: Vec<Value> = slot
        .items()
        .iter()
        .map(|id| {
            json!({"item": id, "rating": null, "account": null,
            "reason": "Nothing to rate.", "evidence": []})
        })
        .collect();
    let review = |rating: Value, support: &str, turn: &str| {
        let unrated = json!({"rating": null, "reason": "Too little was said.",
            "insufficientReason": "missing_turns", "support": "none", "evidence": []});
        json!({"dimensions": {
                "reasoningParticipation": {"rating": rating, "reason": "The learner improved the answer.",
                    "insufficientReason": null, "support": support,
                    "evidence": [{"kind": "turn", "id": turn}]},
                "implementationOwnership": unrated, "testingAndDiagnosis": unrated,
                "revisionAndImprovement": unrated},
            "gaps": [], "nextActions": [], "explanations": rows})
    };
    let validate = |value: Value| crate::tasks::report::validate(&session, &value, "");
    assert!(session.after_defense_hint("turn-3"));
    assert!(!session.after_defense_hint("turn-1"));
    validate(review(json!(2), "conceptual_hint", "turn-3")).unwrap();
    assert!(validate(review(json!(2), "conceptual_hint", "turn-1")).is_err());
    validate(review(json!(3), "none", "turn-3")).unwrap();
    assert!(crate::tasks::report::prompt(&session).contains("Jim gave hints"));
}
