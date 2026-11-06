use super::*;
use crate::accounts::peer::Settings;

#[tokio::test]
async fn the_retention_task_removes_an_expired_record_without_being_asked() {
    let accounts = Accounts::open(crate::accounts::GitHubLoginConfig {
        oauth: None,
        session_secret: "audio-retention".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    accounts.initialize_schema().unwrap();
    accounts.with(|c|c.execute_batch("INSERT INTO users(id,github_id,login,created_at,updated_at) VALUES(1,1,'alice',1,1)")).unwrap();
    let state = peer::create(
        &accounts,
        1,
        Settings {
            topic: String::new(),
            difficulty: "easy".into(),
            language: "python".into(),
            round_minutes: 25,
            interviewer_first: 0,
            save_transcript: false,
            save_audio: true,
        },
        &crate::config::PeerConfig::default(),
        100,
    )
    .unwrap()
    .unwrap();
    let id = state["sessionId"].as_str().unwrap().to_owned();
    peer_audio::mark_started(&accounts, &id, 100, 60).unwrap();
    peer_audio::mark_failed(&accounts, &id, 101).unwrap();
    let dir = std::env::temp_dir().join(format!("codetrial-retention-{}", std::process::id()));
    let accounts = Arc::new(accounts);
    let _tasks = spawn_audio_retention(Arc::clone(&accounts), dir);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let record = peer_audio::record(&accounts, &id).unwrap().unwrap();
            if record.purged_at.is_some() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the retention task sweeps an expired record on its own");
}
