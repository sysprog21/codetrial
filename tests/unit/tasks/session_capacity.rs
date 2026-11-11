//! What a session does when one of its tables is full.

use super::*;

#[tokio::test]
async fn only_an_evicted_revision_that_was_run_is_recorded_as_dropped() {
    let mut session = working().await;
    session
        .revision(capture("ran", "z-ran", "ran", "run"), 101)
        .unwrap();
    session
        .run(practice("result", "z-ran", 1, 2, ""), 101)
        .unwrap();
    // Sorted before it, so these are the ones evicted, and none was run.
    for index in 0..12 {
        session
            .revision(
                capture(
                    &format!("talk-{index}"),
                    &format!("a-{index:02}"),
                    "talk",
                    "discuss",
                ),
                102 + index,
            )
            .unwrap();
    }
    assert!(session.capture_overflow);
    assert!(!session.revisions.contains_key("a-00"));
    assert!(session.revisions.contains_key("z-ran"));
    assert!(session.dropped_run_revision_ids.is_empty());
}

/// Ids are random in the page, so the revision that was run can sort before
/// the ones that were only talked about; it is still the evidence, and the
/// talk-only ones are what a full table gives up first.
#[tokio::test]
async fn a_run_revision_is_kept_whatever_it_sorts_as() {
    let mut session = working().await;
    session
        .revision(capture("ran", "a-ran", "ran", "run"), 101)
        .unwrap();
    session
        .run(practice("result", "a-ran", 1, 2, ""), 101)
        .unwrap();
    for index in 0..12 {
        session
            .revision(
                capture(
                    &format!("talk-{index}"),
                    &format!("z-{index:02}"),
                    "talk",
                    "discuss",
                ),
                102 + index,
            )
            .unwrap();
    }
    assert!(session.capture_overflow);
    assert!(session.revisions.contains_key("a-ran"));
    assert!(!session.revisions.contains_key("z-00"));
    assert!(session.dropped_run_revision_ids.is_empty());
}

/// With nothing but run revisions left to give up, the table still makes room,
/// and says which run lost its code.
#[tokio::test]
async fn a_full_table_of_run_revisions_drops_one_and_says_so() {
    let mut session = working().await;
    for index in 0..11 {
        let id = format!("r-{index:02}");
        session
            .revision(
                capture(&format!("ran-{index}"), &id, "ran", "run"),
                101 + index,
            )
            .unwrap();
        session
            .run(
                practice(&format!("result-{index}"), &id, 1, 2, ""),
                101 + index,
            )
            .unwrap();
    }
    assert!(session.capture_overflow);
    assert!(session.dropped_run_revision_ids.contains("r-00"));
    assert!(!session.revisions.contains_key("r-00"));
    assert!(session.revisions.contains_key("r-10"));
    assert!(!session.dropped_run_revision_ids.contains("r-10"));
}

/// The request ids a session remembers are bounded: one more is refused at the
/// cap and not before it, and an id already seen is answered as a repeat, not
/// refused, however full the set is.
#[tokio::test]
async fn request_ids_are_refused_at_the_cap_and_not_before() {
    let mut session = working().await;
    for index in 0..MAX_REQUESTS - 1 {
        session.requests.insert(format!("old-{index}"));
    }
    assert_eq!(session.request(1, "new-1"), Ok(true));
    session.requests.insert("new-1".to_owned());
    assert_eq!(session.requests.len(), MAX_REQUESTS);
    assert!(session.request(1, "new-2").is_err(), "one past the cap");
    assert_eq!(
        session.request(1, "new-1"),
        Ok(false),
        "a repeat is not a new request"
    );
}
