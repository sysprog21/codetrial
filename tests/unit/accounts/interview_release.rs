//! Giving an interview's claimed room back, and the three reasons it is not.

use super::*;

fn accounts_with_user() -> Accounts {
    let accounts = Accounts::open(crate::accounts::GitHubLoginConfig {
        oauth: None,
        session_secret: "interview-release".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    accounts.initialize_schema().unwrap();
    accounts
        .with(|c| {
            c.execute_batch(
                "INSERT INTO users(id,github_id,login,created_at,updated_at) VALUES(1,1,'alice',1,1)",
            )
        })
        .unwrap();
    accounts
}

/// The release is guarded three ways, and each guard keeps a room bound that
/// something else still depends on: the room it names, a recording made in
/// it, and another pending interview waiting for the same consent's slot.
#[test]
fn a_claimed_room_is_released_only_when_nothing_depends_on_it() {
    let accounts = accounts_with_user();
    create_interview(&accounts, "int-1", 1, "2026-08-21", 5).unwrap();
    assert!(claim_interview_room(&accounts, "int-1", 1, "interview-aaa").unwrap());

    assert!(
        !release_interview_room(&accounts, "int-1", 1, "interview-other").unwrap(),
        "a release for a room it no longer holds unbinds nothing"
    );

    accounts
        .with(|c| {
            c.execute(
                "INSERT INTO recordings (id, account_id, interview_id, room_name, idempotency_key,
                    recipient_email, state, created_at, updated_at)
                 VALUES ('rec-1', 1, 'int-1', 'interview-aaa', 'k', 'one@example.test', 'starting', 20, 20)",
                [],
            )
        })
        .unwrap();
    assert!(
        !release_interview_room(&accounts, "int-1", 1, "interview-aaa").unwrap(),
        "a recording started in the room keeps it bound"
    );
    accounts
        .with(|c| c.execute("DELETE FROM recordings", []))
        .unwrap();

    create_interview(&accounts, "int-2", 1, "2026-08-21", 6).unwrap();
    assert!(
        !release_interview_room(&accounts, "int-1", 1, "interview-aaa").unwrap(),
        "another pending interview holds the consent's pending slot"
    );
    accounts
        .with(|c| c.execute("DELETE FROM interviews WHERE id = 'int-2'", []))
        .unwrap();

    assert!(release_interview_room(&accounts, "int-1", 1, "interview-aaa").unwrap());
    assert!(
        claim_interview_room(&accounts, "int-1", 1, "interview-bbb").unwrap(),
        "and the room is free to claim again"
    );
}
