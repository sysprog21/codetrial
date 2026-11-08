//! Peer flows use real OAuth callbacks and HTTP requests for each member.
use super::*;

struct PeerFixture {
    base: String,
    server: TestServer,
    github: TestServer,
    path: std::path::PathBuf,
    client: reqwest::Client,
    cookies: Vec<String>,
    dispatcher: std::sync::Arc<RecordingDispatcher>,
}

async fn oauth_cookie(client: &reqwest::Client, base: &str) -> String {
    let login = client
        .get(format!("{base}/api/login"))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 302);
    let authorize = reqwest::Url::parse(login.headers()["location"].to_str().unwrap()).unwrap();
    let state = authorize
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    let cookies = login
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|cookie| cookie.to_str().unwrap().split(';').next().unwrap())
        .collect::<Vec<_>>()
        .join("; ");
    let callback = client
        .get(format!("{base}/api/callback"))
        .query(&[("code", MOCK_GITHUB_CODE), ("state", &state)])
        .header("cookie", cookies)
        .send()
        .await
        .unwrap();
    assert_eq!(callback.status(), 302);
    callback
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap())
        .find(|value| value.starts_with("codetrial_session="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

impl PeerFixture {
    async fn new(label: &str) -> Self {
        Self::configured(label, |_| {}).await
    }

    async fn configured(label: &str, configure: impl FnOnce(&mut WebServerConfig)) -> Self {
        let path = account_db_path(label);
        initialize_account_database(&path).unwrap();
        let client = http_client_builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let (github_base, github) = spawn_github_stub_with_id(
            (
                axum::http::StatusCode::FORBIDDEN,
                json!({"message":"rate limited"}),
            ),
            303,
        )
        .await;
        let mut config = web_config();
        config.peer = Some(codetrial::config::PeerConfig {
            max_sessions: 1,
            ..Default::default()
        });
        config.github_client_id = Some(MOCK_GITHUB_CLIENT_ID.into());
        config.github_client_secret = Some(MOCK_GITHUB_CLIENT_SECRET.into());
        config.session_secret = Some("peer-test-secret".into());
        config.db_path = Some(path.clone());
        config.github_oauth_base_url = Some(github_base.clone());
        config.github_api_base_url = Some(github_base);
        configure(&mut config);
        let dispatcher = std::sync::Arc::new(RecordingDispatcher::default());
        let (base, server) =
            spawn_web_server_with_dispatcher(config.clone(), std::sync::Arc::clone(&dispatcher))
                .await;
        let mut cookies = vec![oauth_cookie(&client, &base).await];
        for id in [404, 505] {
            let (stub_base, stub) = spawn_github_stub_with_id(
                (
                    axum::http::StatusCode::FORBIDDEN,
                    json!({"message":"rate limited"}),
                ),
                id,
            )
            .await;
            config.github_oauth_base_url = Some(stub_base.clone());
            config.github_api_base_url = Some(stub_base);
            let (other_base, other_server) = spawn_web_server(config.clone()).await;
            cookies.push(oauth_cookie(&client, &other_base).await);
            other_server.shutdown().await;
            stub.shutdown().await;
        }
        Self {
            base,
            server,
            github,
            path,
            client,
            cookies,
            dispatcher,
        }
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        member: usize,
        body: Value,
    ) -> reqwest::Response {
        self.client
            .request(method, format!("{}{path}", self.base))
            .header("cookie", &self.cookies[member])
            .header("origin", &self.base)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn post(&self, path: &str, member: usize, body: Value) -> Value {
        let response = self
            .request(reqwest::Method::POST, path, member, body)
            .await;
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert!(status.is_success(), "POST {path}: {status} {body}");
        body
    }

    async fn get(&self, path: &str, member: usize) -> Value {
        let response = self
            .request(reqwest::Method::GET, path, member, Value::Null)
            .await;
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert!(status.is_success(), "GET {path}: {status} {body}");
        body
    }

    /// The state with the server's wall clock removed, for comparing two reads
    /// that may straddle a second boundary.
    async fn get_without_clock(&self, path: &str, member: usize) -> Value {
        let mut state = self.get(path, member).await;
        state.as_object_mut().unwrap().remove("serverNow");
        state
    }

    async fn create(&self) -> (String, Value) {
        let state = self.post("/api/peer/sessions", 0, json!({"topic":"","difficulty":"easy","language":"python","roundMinutes":25,"interviewerFirst":0})).await;
        let id = state["sessionId"].as_str().unwrap().to_string();
        (format!("/api/peer/sessions/{id}"), state)
    }

    async fn joined(&self) -> String {
        let (path, state) = self.create().await;
        self.post(
            "/api/peer/invites/accept",
            1,
            json!({"token":state["inviteToken"]}),
        )
        .await;
        path
    }

    async fn active(&self) -> String {
        let path = self.joined().await;
        for member in 0..2 {
            self.post(&format!("{path}/enter"), member, json!({})).await;
        }
        for member in 0..2 {
            let state = self.get(&path, member).await;
            self.post(
                &format!("{path}/ready"),
                member,
                json!({"expectedVersion":state["version"],"ready":true}),
            )
            .await;
        }
        for member in 0..2 {
            let state = self.get(&path, member).await;
            let confirmed = self
                .post(
                    &format!("{path}/start"),
                    member,
                    json!({"expectedVersion":state["version"],"observerConsent":true}),
                )
                .await;
            if member == 0 {
                assert!(confirmed["currentRound"].is_null());
                assert!(confirmed["hardDeadline"].is_null());
            }
        }
        path
    }

    async fn close(self) {
        self.server.shutdown().await;
        self.github.shutdown().await;
        remove_database(self.path).await;
    }
}

async fn error(response: reqwest::Response, status: u16, code: &str) {
    let actual = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(actual, status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

#[tokio::test]
async fn peer_membership_and_invites_require_verified_identity_and_origin() {
    let fixture = PeerFixture::new("peer-membership").await;
    let session = fixture.get("/api/session", 0).await;
    assert_eq!(session["identityVerified"], true);
    assert_eq!(session["peerEnabled"], true);
    assert_eq!(session["peerMaxRoundMinutes"], 45);
    let response = fixture
        .client
        .post(format!("{}/api/peer/sessions", fixture.base))
        .header("cookie", &fixture.cookies[0])
        .header("origin", "https://evil.example")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    error(response, 403, "origin_mismatch").await;
    let claimed = record_login(&fixture.client, &fixture.base, "self-declared").await;
    let response = fixture
        .client
        .post(format!("{}/api/peer/sessions", fixture.base))
        .header("cookie", &claimed)
        .header("origin", &fixture.base)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    error(response, 403, "verified_identity_required").await;
    for (method, route) in [
        (reqwest::Method::GET, "/api/peer/sessions"),
        (reqwest::Method::GET, "/api/peer/sessions/unknown"),
        (reqwest::Method::POST, "/api/peer/invites/accept"),
        (reqwest::Method::POST, "/api/peer/sessions/unknown/enter"),
        (reqwest::Method::POST, "/api/peer/sessions/unknown/token"),
        (
            reqwest::Method::PUT,
            "/api/peer/sessions/unknown/rounds/0/snapshot",
        ),
        (
            reqwest::Method::GET,
            "/api/peer/sessions/unknown/rounds/0/guide",
        ),
        (
            reqwest::Method::PUT,
            "/api/peer/sessions/unknown/rounds/0/feedback",
        ),
    ] {
        let response = fixture
            .client
            .request(method, format!("{}{route}", fixture.base))
            .header("cookie", &claimed)
            .header("origin", &fixture.base)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        error(response, 403, "verified_identity_required").await;
    }
    let (path, state) = fixture.create().await;
    let token = state["inviteToken"].clone();
    error(
        fixture
            .request(
                reqwest::Method::POST,
                "/api/peer/invites/accept",
                0,
                json!({"token":token}),
            )
            .await,
        410,
        "invite_unavailable",
    )
    .await;
    let accepted = fixture
        .post("/api/peer/invites/accept", 1, json!({"token":token}))
        .await;
    assert_eq!(accepted["members"].as_array().unwrap().len(), 2);
    assert!(accepted["hardDeadline"].is_null());
    for member in [1, 2] {
        error(
            fixture
                .request(
                    reqwest::Method::POST,
                    "/api/peer/invites/accept",
                    member,
                    json!({"token":token}),
                )
                .await,
            410,
            "invite_unavailable",
        )
        .await;
    }
    assert_eq!(
        fixture
            .request(reqwest::Method::GET, &path, 2, Value::Null)
            .await
            .status(),
        404
    );
    let conn = rusqlite::Connection::open(&fixture.path).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM peer_active_users", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(conn);
    fixture.close().await;
}

#[tokio::test]
async fn peer_regenerated_invite_invalidates_previous_token() {
    let fixture = PeerFixture::new("peer-regenerate").await;
    let (path, state) = fixture.create().await;
    error(
        fixture
            .request(
                reqwest::Method::POST,
                &format!("{path}/invite"),
                0,
                json!({"expectedVersion":999_999}),
            )
            .await,
        409,
        "version_conflict",
    )
    .await;
    let renewed = fixture
        .post(
            &format!("{path}/invite"),
            0,
            json!({"expectedVersion":state["version"]}),
        )
        .await;
    error(
        fixture
            .request(
                reqwest::Method::POST,
                "/api/peer/invites/accept",
                1,
                json!({"token":state["inviteToken"]}),
            )
            .await,
        410,
        "invite_unavailable",
    )
    .await;
    fixture
        .post(
            "/api/peer/invites/accept",
            1,
            json!({"token":renewed["inviteToken"]}),
        )
        .await;
    fixture.close().await;
}

#[tokio::test]
async fn peer_tokens_wait_for_both_members_and_keep_identity() {
    let fixture = PeerFixture::new("peer-token").await;
    let path = fixture.joined().await;
    fixture.post(&format!("{path}/enter"), 0, json!({})).await;
    error(
        fixture
            .request(
                reqwest::Method::POST,
                &format!("{path}/token"),
                0,
                json!({}),
            )
            .await,
        409,
        "waiting_for_partner",
    )
    .await;
    fixture.post(&format!("{path}/enter"), 1, json!({})).await;
    let a = fixture.post(&format!("{path}/token"), 0, json!({})).await;
    let b = fixture.post(&format!("{path}/token"), 1, json!({})).await;
    let again = fixture.post(&format!("{path}/token"), 0, json!({})).await;
    let a_claims = claims(a["token"].as_str().unwrap());
    let b_claims = claims(b["token"].as_str().unwrap());
    assert_eq!(a_claims["video"]["room"], b_claims["video"]["room"]);
    assert_eq!(
        a_claims["sub"],
        claims(again["token"].as_str().unwrap())["sub"]
    );
    assert_ne!(a_claims["sub"], b_claims["sub"]);
    assert_eq!(
        a_claims["exp"].as_u64().unwrap() - a_claims["nbf"].as_u64().unwrap(),
        300
    );
    assert_eq!(a_claims["video"]["agent"], false);
    assert!(
        fixture.dispatcher.rooms().is_empty(),
        "peer tokens must never dispatch an AI agent"
    );
    let state = fixture.get(&path, 0).await;
    assert_eq!(a_claims["sub"], state["members"][0]["identity"]);
    fixture.post(&format!("{path}/leave"), 0, json!({})).await;
    let conn = rusqlite::Connection::open(&fixture.path).unwrap();
    assert_eq!(conn.query_row("SELECT count(*) FROM peer_active_users WHERE user_id = (SELECT id FROM users WHERE github_id=303)", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    drop(conn);
    fixture.close().await;
}

#[tokio::test]
async fn peer_rounds_lock_guides_and_enforce_snapshot_writer_lease() {
    let fixture = PeerFixture::new("peer-workspace").await;
    let path = fixture.active().await;
    let state = fixture.get(&path, 0).await;
    assert_eq!(state["currentRound"]["index"], 0);
    assert_eq!(state["currentRound"]["status"], "active");
    assert_eq!(state["currentRound"]["role"], "interviewer");
    let deadline = state["currentRound"]["deadline"].as_i64().unwrap();
    assert_eq!(
        deadline - state["currentRound"]["startedAt"].as_i64().unwrap(),
        25 * 60
    );
    assert_eq!(
        fixture
            .request(
                reqwest::Method::GET,
                &format!("{path}/rounds/0/guide"),
                1,
                Value::Null
            )
            .await
            .status(),
        404
    );
    let guide = fixture.get(&format!("{path}/rounds/0/guide"), 0).await;
    assert_eq!(guide["hints"].as_array().unwrap().len(), 3);
    assert_eq!(
        fixture
            .request(
                reqwest::Method::GET,
                &format!("{path}/rounds/1/problem"),
                0,
                Value::Null
            )
            .await
            .status(),
        404
    );
    let writer = fixture
        .post(&format!("{path}/rounds/0/writer"), 1, json!({}))
        .await;
    let snapshot = json!({"lease":writer["lease"],"expectedRevision":0,"language":"python","code":"print(1)","tests":null,"final":false});
    error(
        fixture
            .request(
                reqwest::Method::PUT,
                &format!("{path}/rounds/0/snapshot"),
                0,
                snapshot.clone(),
            )
            .await,
        403,
        "not_writer",
    )
    .await;
    let saved = fixture
        .request(
            reqwest::Method::PUT,
            &format!("{path}/rounds/0/snapshot"),
            1,
            snapshot.clone(),
        )
        .await;
    assert!(saved.status().is_success());
    error(
        fixture
            .request(
                reqwest::Method::PUT,
                &format!("{path}/rounds/0/snapshot"),
                1,
                snapshot.clone(),
            )
            .await,
        409,
        "version_conflict",
    )
    .await;
    fixture
        .post(&format!("{path}/rounds/0/writer"), 1, json!({}))
        .await;
    let mut stale = snapshot;
    stale["expectedRevision"] = json!(1);
    error(
        fixture
            .request(
                reqwest::Method::PUT,
                &format!("{path}/rounds/0/snapshot"),
                1,
                stale,
            )
            .await,
        409,
        "lease_lost",
    )
    .await;
    let state = fixture.get(&path, 0).await;
    error(
        fixture
            .request(
                reqwest::Method::POST,
                &format!("{path}/rounds/0/finish"),
                0,
                json!({"expectedVersion":999_999}),
            )
            .await,
        409,
        "version_conflict",
    )
    .await;
    fixture
        .post(
            &format!("{path}/rounds/0/finish"),
            0,
            json!({"expectedVersion":state["version"]}),
        )
        .await;
    let writer = fixture
        .post(&format!("{path}/rounds/0/writer"), 1, json!({}))
        .await;
    let final_snapshot = json!({"lease":writer["lease"],"expectedRevision":1,"language":"python","code":"print(2)","tests":null,"final":true});
    let response = fixture
        .request(
            reqwest::Method::PUT,
            &format!("{path}/rounds/0/snapshot"),
            1,
            final_snapshot,
        )
        .await;
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    fixture.close().await;
}

fn ratings(kind: &str) -> Value {
    let names: &[&str] = if kind == "candidate" {
        &[
            "clarification",
            "reasoning",
            "correctness",
            "testing",
            "complexity",
            "technicalExplanation",
        ]
    } else {
        &[
            "preparation",
            "questionQuality",
            "hintPacing",
            "followUp",
            "feedbackSpecificity",
        ]
    };
    Value::Object(names.iter().map(|name| ((*name).to_string(), json!({"score":3,"example":"Used a boundary case","improvement":"Explain the empty input case"}))).collect())
}

#[tokio::test]
async fn peer_feedback_is_private_until_both_submit_and_swap_reverses_roles() {
    let fixture = PeerFixture::new("peer-feedback").await;
    let path = fixture.active().await;
    let first_problem = fixture.get(&format!("{path}/rounds/0/problem"), 0).await;
    let early = fixture
        .request(
            reqwest::Method::PUT,
            &format!("{path}/rounds/0/feedback"),
            0,
            json!({"submitted":true,"dimensions":ratings("candidate")}),
        )
        .await;
    assert_eq!(early.status(), 409);
    let state = fixture.get(&path, 0).await;
    fixture
        .post(
            &format!("{path}/rounds/0/finish"),
            0,
            json!({"expectedVersion":state["version"]}),
        )
        .await;
    let writer = fixture
        .post(&format!("{path}/rounds/0/writer"), 1, json!({}))
        .await;
    let response = fixture.request(reqwest::Method::PUT, &format!("{path}/rounds/0/snapshot"), 1, json!({"lease":writer["lease"],"expectedRevision":0,"language":"python","code":"print(1)","tests":null,"final":true})).await;
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    for submitted in [false, true] {
        let response = fixture
            .request(
                reqwest::Method::PUT,
                &format!("{path}/rounds/0/feedback"),
                0,
                json!({"submitted":submitted,"dimensions":ratings("candidate")}),
            )
            .await;
        assert!(
            response.status().is_success(),
            "{}",
            response.text().await.unwrap()
        );
        let hidden = fixture.get(&format!("{path}/rounds/0/feedback"), 1).await;
        assert_eq!(hidden["feedback"].as_array().unwrap().len(), 0);
    }
    let mut invalid = ratings("interviewer");
    invalid["preparation"]["score"] = json!(5);
    assert_eq!(
        fixture
            .request(
                reqwest::Method::PUT,
                &format!("{path}/rounds/0/feedback"),
                1,
                json!({"submitted":true,"dimensions":invalid})
            )
            .await
            .status(),
        400
    );
    let mut invalid = ratings("interviewer");
    invalid["personality"] = json!({"score":3,"example":"","improvement":""});
    assert_eq!(
        fixture
            .request(
                reqwest::Method::PUT,
                &format!("{path}/rounds/0/feedback"),
                1,
                json!({"submitted":true,"dimensions":invalid})
            )
            .await
            .status(),
        400
    );
    let response = fixture
        .request(
            reqwest::Method::PUT,
            &format!("{path}/rounds/0/feedback"),
            1,
            json!({"submitted":true,"dimensions":ratings("interviewer")}),
        )
        .await;
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    for member in 0..2 {
        let revealed = fixture
            .get(&format!("{path}/rounds/0/feedback"), member)
            .await;
        assert_eq!(revealed["feedback"].as_array().unwrap().len(), 2);
    }
    for member in 0..2 {
        let state = fixture.get(&path, member).await;
        fixture
            .post(
                &format!("{path}/advance"),
                member,
                json!({"expectedVersion":state["version"]}),
            )
            .await;
    }
    let state = fixture.get(&path, 0).await;
    assert_eq!(state["currentRound"]["index"], 1);
    assert_eq!(state["currentRound"]["role"], "candidate");
    let second_problem = fixture.get(&format!("{path}/rounds/1/problem"), 0).await;
    assert_ne!(first_problem["page"], second_problem["page"]);
    fixture
        .post(
            &format!("{path}/rounds/1/finish"),
            1,
            json!({"expectedVersion":state["version"]}),
        )
        .await;
    let writer = fixture
        .post(&format!("{path}/rounds/1/writer"), 0, json!({}))
        .await;
    let response = fixture.request(reqwest::Method::PUT,
        &format!("{path}/rounds/1/snapshot"), 0,
        json!({"lease":writer["lease"],"expectedRevision":0,"language":"python","code":"print(2)","tests":null,"final":true})).await;
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    for member in 0..2 {
        let state = fixture.get(&path, member).await;
        fixture
            .post(
                &format!("{path}/advance"),
                member,
                json!({"expectedVersion":state["version"]}),
            )
            .await;
    }
    let complete = fixture.get(&path, 0).await;
    assert_eq!(complete["status"], "complete");
    assert_eq!(complete["currentRound"]["index"], 1);
    error(
        fixture
            .request(
                reqwest::Method::POST,
                &format!("{path}/token"),
                0,
                json!({}),
            )
            .await,
        410,
        "session_closed",
    )
    .await;
    let conn = rusqlite::Connection::open(&fixture.path).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM reports", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(conn);
    fixture.close().await;
}

#[tokio::test]
async fn peer_snapshot_bounds_count_utf8_bytes_and_body_size() {
    let fixture = PeerFixture::new("peer-snapshot-bounds").await;
    let path = fixture.active().await;
    let writer = fixture
        .post(&format!("{path}/rounds/0/writer"), 1, json!({}))
        .await;
    // The multi-byte fixture distinguishes UTF-8 bytes from character count.
    let code = "\u{03bb}".repeat(25 * 1024);
    let response = fixture.request(reqwest::Method::PUT, &format!("{path}/rounds/0/snapshot"), 1, json!({"lease":writer["lease"],"expectedRevision":0,"language":"python","code":code,"tests":null,"final":false})).await;
    error(response, 400, "invalid_snapshot").await;
    let response = fixture.request(reqwest::Method::PUT, &format!("{path}/rounds/0/snapshot"), 1, json!({"lease":writer["lease"],"expectedRevision":0,"language":"python","code":"x".repeat(65*1024),"tests":null,"final":false})).await;
    error(response, 400, "invalid_snapshot").await;
    let response = fixture.request(reqwest::Method::PUT, &format!("{path}/rounds/0/snapshot"), 1, json!({"lease":writer["lease"],"expectedRevision":0,"language":"python","code":"x".repeat(2*1024*1024+1),"tests":null,"final":false})).await;
    assert_eq!(response.status(), 413);
    fixture.close().await;
}

#[tokio::test]
async fn peer_enter_bounds_active_rooms_even_after_members_leave() {
    let fixture = PeerFixture::new("peer-capacity").await;
    let first = fixture.active().await;
    let second = fixture.joined().await;
    error(
        fixture
            .request(
                reqwest::Method::POST,
                &format!("{second}/enter"),
                0,
                json!({}),
            )
            .await,
        409,
        "already_active",
    )
    .await;
    for member in 0..2 {
        fixture
            .post(&format!("{first}/leave"), member, json!({}))
            .await;
    }
    error(
        fixture
            .request(
                reqwest::Method::POST,
                &format!("{second}/enter"),
                0,
                json!({}),
            )
            .await,
        503,
        "at_capacity",
    )
    .await;
    fixture.close().await;
}

#[path = "peer_discovery.rs"]
mod discovery;

#[path = "peer_artifacts.rs"]
mod artifacts;

#[path = "peer_routes.rs"]
mod routes;

#[tokio::test]
async fn writer_recovery_renews_only_the_same_expired_lease_and_cannot_steal_a_takeover() {
    let fixture = PeerFixture::new("peer-resume-writer").await;
    let path = fixture.active().await;
    let route = format!("{path}/rounds/0/writer");
    let first = fixture.post(&route, 1, json!({})).await;
    let second = fixture.post(&route, 1, json!({})).await;
    error(
        fixture
            .request(
                reqwest::Method::POST,
                &route,
                1,
                json!({"previousLease":first["lease"]}),
            )
            .await,
        409,
        "lease_lost",
    )
    .await;
    let id = path.rsplit('/').next().unwrap();
    let connection = rusqlite::Connection::open(&fixture.path).unwrap();
    connection.execute("UPDATE peer_rounds SET writer_lease_until=1,state_json=json_set(state_json,'$.writer_lease_until',1) WHERE session_id=?1 AND idx=0",[id]).unwrap();
    let recovered = fixture
        .post(&route, 1, json!({"previousLease":second["lease"]}))
        .await;
    assert_ne!(recovered["lease"], second["lease"]);
    let response = fixture.request(reqwest::Method::PUT, &format!("{path}/rounds/0/snapshot"), 1,
        json!({"lease":recovered["lease"],"expectedRevision":0,"language":"python","code":"preserved local code","tests":null,"final":false})).await;
    assert_eq!(response.status(), 200);
    let malformed = fixture
        .request(
            reqwest::Method::POST,
            &route,
            1,
            json!({"previousLease":null}),
        )
        .await;
    assert_eq!(malformed.status(), 400);
    drop(connection);
    fixture.close().await;
}
