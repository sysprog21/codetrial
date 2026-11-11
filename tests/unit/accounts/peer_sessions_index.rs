use crate::accounts::{Accounts, GitHubLoginConfig};

/// The cap on waiting invitations counts one account's pending sessions on
/// every create, so the count has to be answered from an index and not by
/// reading every session in the table.
#[test]
fn the_invitation_cap_count_is_answered_from_an_index() {
    let accounts = Accounts::open(GitHubLoginConfig {
        oauth: None,
        session_secret: "peer-sessions-index".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    accounts.initialize_schema().unwrap();
    let plan: String = accounts
        .with(|connection| {
            let mut statement = connection.prepare(
                "EXPLAIN QUERY PLAN
                 SELECT count(*) FROM peer_sessions WHERE creator_id=?1 AND status='pending'",
            )?;
            let rows = statement
                .query_map([1], |row| row.get::<_, String>(3))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows.join("; "))
        })
        .unwrap();
    assert!(plan.contains("USING"), "{plan}");
    assert!(plan.contains("peer_sessions_by_creator"), "{plan}");
    assert!(!plan.contains("SCAN"), "{plan}");
}
