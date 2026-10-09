use super::*;
use std::sync::Arc;

use codetrial::tasks::access::{PinnedTask, TaskService};
use codetrial::tasks::package::Download;

const PIN: &str = "123456";
const SITE: &str = "https://teacher.github.io/course";

/// A service where `owner` has already opened the fixture set, as a
/// successful unlock against the instructor's site would leave it.
async fn unlocked_for(owner: i64) -> TaskService {
    let service = TaskService::new();
    let download = Download {
        manifest: include_bytes!("../fixtures/task-mode/manifest.json").to_vec(),
        ciphertext: include_bytes!("../fixtures/task-mode/package.enc").to_vec(),
        site_time: None,
    };
    service
        .open(
            owner,
            &codetrial::tasks::access::Assignment::new(SITE, "classroom", 7).unwrap(),
            download,
            PIN,
            100,
        )
        .await
        .unwrap();
    service
}

#[derive(Default)]
struct TaskDispatcher {
    tasks: std::sync::Mutex<Vec<PinnedTask>>,
}
impl RoomDispatcher for TaskDispatcher {
    fn ensure_agent(
        &self,
        _: &str,
        _: &codetrial::config::Provider,
        job: AgentJob,
    ) -> Result<(), DispatchRefusal> {
        let AgentJob::Task { task, .. } = job else {
            panic!("task admission fell back to an interview")
        };
        self.tasks.lock().unwrap().push(task);
        Ok(())
    }
}

/// The web server with an interviewer to dispatch, as a configured
/// CodeTrial has.
async fn spawn_task_server(config: WebServerConfig) -> (String, TestServer) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = spawn_test_server(
        listener,
        codetrial::web::web_service_with_dispatcher(
            config,
            Some(Arc::new(TaskDispatcher::default())),
        ),
    );
    (url, server)
}

async fn code(response: reqwest::Response) -> (u16, String) {
    let status = response.status().as_u16();
    let body = response.json::<Value>().await.unwrap();
    (status, body["code"].as_str().unwrap_or_default().to_owned())
}

fn admission() -> Value {
    json!({"site":"https://teacher.github.io/course","version":7,"requestKey":"request-1",
        "language":"python","supportedPlatform":true,
        "rulesAcknowledgedAt":"2026-10-07T00:00:00Z",
        "calibration":{"ok":true,"metric":"keypoints","baseline":0.5,"keyboard":0.7,"limit":0.75}})
}

#[tokio::test]
async fn task_routes_answer_loopback_required_when_task_mode_is_off() {
    let (config, cookie, db) = signed_in_web_config("task-off");
    let (url, server) = spawn_web_server(config).await;
    let client = http_client();
    let unlock = client
        .post(format!("{url}/api/task-sets/classroom/unlock"))
        .header("Cookie", &cookie)
        .header("x-codetrial-task-request", "1")
        .json(&json!({"site":"https://teacher.github.io/course","version":7,"pin":PIN}))
        .send()
        .await
        .unwrap();
    assert_eq!(code(unlock).await, (403, "loopback_required".to_owned()));
    let load = client
        .get(format!(
            "{url}/api/task-sets/classroom/tasks/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7"
        ))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(code(load).await, (403, "loopback_required".to_owned()));
    server.shutdown().await;
    remove_database(db).await;
}

#[tokio::test]
async fn task_routes_answer_only_a_page_naming_this_machine() {
    let (mut config, cookie, db) = signed_in_web_config("task-host");
    config.tasks = Some(unlocked_for(1).await);
    let (url, server) = spawn_task_server(config).await;
    let client = http_client();
    let load = |host: &'static str| {
        client
            .get(format!(
                "{url}/api/task-sets/classroom/tasks/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7"
            ))
            .header("Cookie", &cookie)
            .header("Host", host)
            .send()
    };
    // A DNS name rebound to this machine, or a proxy nobody declared.
    for host in ["attacker.example", "class.example:443"] {
        assert_eq!(
            code(load(host).await.unwrap()).await,
            (403, "loopback_required".to_owned()),
            "{host}"
        );
    }
    for host in ["localhost:3000", "127.0.0.1", "[::1]:3000"] {
        assert_eq!(load(host).await.unwrap().status(), 200, "{host}");
    }
    server.shutdown().await;
    remove_database(db).await;
}

#[tokio::test]
async fn a_signed_out_page_is_told_to_sign_in_in_the_task_error_shape() {
    let (mut config, _, db) = signed_in_web_config("task-signed-out");
    config.tasks = Some(TaskService::new());
    let (url, server) = spawn_web_server(config).await;
    let client = http_client();
    for response in [
        client
            .get(format!(
                "{url}/api/task-sets/classroom/tasks/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7"
            ))
            .send()
            .await
            .unwrap(),
        client
            .get(format!("{url}/api/task-reviews/room-1"))
            .send()
            .await
            .unwrap(),
    ] {
        assert_eq!(
            code(response).await,
            (401, "authentication_required".to_owned())
        );
    }
    server.shutdown().await;
    remove_database(db).await;
}

#[tokio::test]
async fn task_unlock_requires_a_session_the_page_header_and_an_https_site() {
    let (mut config, cookie, db) = signed_in_web_config("task-unlock");
    config.tasks = Some(TaskService::new());
    let (url, server) = spawn_web_server(config).await;
    let client = http_client();
    let endpoint = format!("{url}/api/task-sets/classroom/unlock");
    let body = |site: &str, pin: &str| json!({"site": site, "version": 7, "pin": pin});
    let https = "https://teacher.github.io/course";
    assert_eq!(
        client
            .post(&endpoint)
            .json(&body(https, PIN))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let no_header = client
        .post(&endpoint)
        .header("Cookie", &cookie)
        .json(&body(https, PIN))
        .send()
        .await
        .unwrap();
    assert_eq!(code(no_header).await, (403, "csrf_rejected".to_owned()));
    let send = |value: Value| {
        client
            .post(&endpoint)
            .header("Cookie", &cookie)
            .header("x-codetrial-task-request", "1")
            .json(&value)
            .send()
    };
    // Refused before anything is fetched: no network is reached by this test.
    assert_eq!(
        code(
            send(body("http://teacher.github.io/course", PIN))
                .await
                .unwrap()
        )
        .await,
        (400, "site_invalid".to_owned())
    );
    assert_eq!(
        code(send(body(https, "12345")).await.unwrap()).await,
        (403, "invalid_pin".to_owned())
    );
    assert_eq!(
        code(send(json!({"pin": PIN})).await.unwrap()).await,
        (400, "site_invalid".to_owned())
    );
    server.shutdown().await;
    remove_database(db).await;
}

#[tokio::test]
async fn task_requests_from_a_cross_site_page_are_refused() {
    let (mut config, cookie, db) = signed_in_web_config("task-proxy-csrf");
    config.tasks = Some(TaskService::new());
    let (url, server) = spawn_web_server(config).await;
    let client = http_client();
    let endpoint = format!("{url}/api/task-sets/classroom/unlock");
    let unlock = |site: &'static str| {
        client
            .post(&endpoint)
            .header("Cookie", &cookie)
            .header("x-codetrial-task-request", "1")
            .header("Origin", "https://class.example")
            .header("Sec-Fetch-Site", site)
            // A plain-HTTP site: past the CSRF check the request stops at
            // `site_invalid` without any download.
            .json(&json!({"site":"http://teacher.example","version":7,"pin":PIN}))
            .send()
    };
    for site in ["cross-site", "same-site"] {
        assert_eq!(
            code(unlock(site).await.unwrap()).await,
            (403, "csrf_rejected".to_owned())
        );
    }
    assert_eq!(
        code(unlock("same-origin").await.unwrap()).await,
        (400, "site_invalid".to_owned())
    );
    server.shutdown().await;
    remove_database(db).await;
}

#[tokio::test]
async fn task_admission_needs_the_rules_and_calibration_replays_one_room_and_takes_no_overrides() {
    let (mut config, cookie, db) = signed_in_web_config("task-admission");
    config.tasks = Some(unlocked_for(1).await);
    let dispatcher = Arc::new(TaskDispatcher::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = spawn_test_server(
        listener,
        codetrial::web::web_service_with_dispatcher(config, Some(dispatcher.clone())),
    );
    let client = http_client();
    let public = client
        .get(format!(
            "{url}/api/task-sets/classroom/tasks/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7"
        ))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(public.headers()["cache-control"], "no-store");
    let public = public.json::<Value>().await.unwrap();
    assert_eq!(public["rules"]["lookAwaySeconds"], 8);
    for private in [
        "key",
        "guide",
        "optimal",
        "pitfalls",
        "interactionPrompt",
        "referenceCode",
    ] {
        assert!(public.get(private).is_none());
    }
    let admit = |value: Value| {
        client
            .post(format!(
                "{url}/api/task-sets/classroom/tasks/delimiter-closer/attempts"
            ))
            .header("Cookie", &cookie)
            .header("x-codetrial-task-request", "1")
            .json(&value)
            .send()
    };
    for key in [
        "durationMin",
        "maxHintRungs",
        "accountId",
        "problemId",
        // Named by the route; a body naming another is not taken.
        "setId",
        "taskId",
        "setVersion",
        "fullscreen",
    ] {
        let mut bad = admission();
        bad[key] = json!(999);
        assert_eq!(
            code(admit(bad).await.unwrap()).await,
            (400, "client_override".to_owned()),
            "{key}"
        );
    }
    for (field, value, expected) in [
        ("rulesAcknowledgedAt", Value::Null, "rules_unacknowledged"),
        (
            "rulesAcknowledgedAt",
            json!("yesterday"),
            "rules_unacknowledged",
        ),
        ("calibration", Value::Null, "calibration_required"),
        // A language no task may name, and one this task does not.
        ("language", json!("rust"), "language_unsupported"),
        ("language", json!("java"), "language_unsupported"),
        // Each limit on its own: a short value that is no object, and an object
        // past the size limit.
        ("calibration", json!("calibrated"), "calibration_required"),
        (
            "calibration",
            json!({"ok": true, "note": "x".repeat(5000)}),
            "calibration_required",
        ),
    ] {
        let mut bad = admission();
        bad[field] = value;
        assert_eq!(code(admit(bad).await.unwrap()).await.1, expected, "{field}");
    }
    assert!(dispatcher.tasks.lock().unwrap().is_empty());
    let first = admit(admission()).await.unwrap();
    assert_eq!(first.status(), 200);
    let first = first.json::<Value>().await.unwrap();
    let second = admit(admission())
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(first, second);
    {
        let tasks = dispatcher.tasks.lock().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].github_login, "one");
        assert_eq!(tasks[0].github_id, 101);
        assert_eq!(tasks[0].preparation.calibration["limit"], 0.75);
    }
    assert_eq!(first["durationMin"], 15);
    let metadata: Value = serde_json::from_str(
        claims(first["token"].as_str().unwrap())["metadata"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(metadata["taskId"], "delimiter-closer");
    assert!(metadata.get("durationMin").is_none());
    let mut other = admission();
    other["requestKey"] = json!("new-key");
    assert_eq!(
        code(admit(other).await.unwrap()).await.1,
        "active_task_session"
    );
    server.shutdown().await;
    drop(dispatcher);
    remove_database(db).await;
}

#[tokio::test]
async fn a_start_without_interviewer_credentials_is_refused_before_any_claim() {
    let (mut config, cookie, db) = signed_in_web_config("task-no-credentials");
    config.tasks = Some(unlocked_for(1).await);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = spawn_test_server(
        listener,
        codetrial::web::web_service_with_dispatcher(config, None),
    );
    let response = http_client()
        .post(format!(
            "{url}/api/task-sets/classroom/tasks/delimiter-closer/attempts"
        ))
        .header("Cookie", &cookie)
        .header("x-codetrial-task-request", "1")
        .json(&admission())
        .send()
        .await
        .unwrap();
    assert_eq!(
        code(response).await,
        (412, "credentials_missing".to_owned())
    );
    server.shutdown().await;
    remove_database(db).await;
}

/// A second signed-in account in the same database, for refusals that need
/// somebody to be refused.
fn second_task_account(db: &std::path::Path) -> String {
    let connection = rusqlite::Connection::open(db).unwrap();
    connection
        .execute(
            "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES (2, 202, 'two', 1, 1)",
            [],
        )
        .unwrap();
    insert_session(&connection, "session-two", 2);
    format!(
        "codetrial_session={}",
        signed_cookie("session-two", "session-secret")
    )
}

#[tokio::test]
async fn an_unlocked_set_is_account_bound_in_memory_and_gone_after_a_restart() {
    let (mut config, cookie, db) = signed_in_web_config("task-unlocked");
    let other = second_task_account(&db);
    config.tasks = Some(unlocked_for(1).await);
    let (url, server) = spawn_task_server(config.clone()).await;
    let client = http_client();
    let task_url = format!(
        "{url}/api/task-sets/classroom/tasks/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7"
    );
    let load = |cookie: String| client.get(&task_url).header("Cookie", cookie).send();
    let opened = load(cookie.clone()).await.unwrap();
    assert_eq!(opened.status(), 200);
    assert_eq!(
        opened.json::<Value>().await.unwrap()["taskId"],
        "delimiter-closer"
    );
    // Account one's unlock opens nothing for account two.
    assert_eq!(
        code(load(other.clone()).await.unwrap()).await,
        (403, "unlock_required".to_owned())
    );
    server.shutdown().await;

    // A restarted process holds nothing unlocked: the same signed-in account
    // has to enter the PIN again.
    config.tasks = Some(TaskService::new());
    let (url, server) = spawn_task_server(config).await;
    let response = client
        .get(format!(
            "{url}/api/task-sets/classroom/tasks/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7"
        ))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(code(response).await, (403, "unlock_required".to_owned()));
    server.shutdown().await;
    remove_database(db).await;
}

#[tokio::test]
async fn device_sign_in_is_offered_only_where_task_mode_is_on() {
    for (task_mode, client_id) in [
        (false, "public-client-id"),
        (true, "public-client-id"),
        (true, " "),
    ] {
        let (mut config, _, db) =
            signed_in_web_config(&format!("task-device-{task_mode}-{}", client_id.len()));
        config.github_client_id = Some(client_id.to_owned());
        config.tasks = task_mode.then(TaskService::new);
        let (url, server) = spawn_web_server(config).await;
        let client = http_client();
        let session = client
            .get(format!("{url}/api/session"))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        // A blank id is no app to sign in with (test builds carry none).
        assert_eq!(session["deviceLogin"], task_mode && client_id != " ");
        if !task_mode {
            // A device code from a server others can reach is a phishing lure.
            let start = client
                .post(format!("{url}/api/github/device"))
                .header("x-codetrial-task-request", "1")
                .send()
                .await
                .unwrap();
            assert!(!start.status().is_success(), "{}", start.status());
        }
        let rebound = client
            .post(format!("{url}/api/github/device"))
            .header("x-codetrial-task-request", "1")
            .header("Host", "attacker.example")
            .send()
            .await
            .unwrap();
        assert_eq!(code(rebound).await, (403, "loopback_required".to_owned()));
        server.shutdown().await;
        remove_database(db).await;
    }
}

#[tokio::test]
async fn a_task_is_refused_at_load_when_no_interviewer_can_join() {
    let (mut config, cookie, db) = signed_in_web_config("task-no-key-load");
    config.tasks = Some(unlocked_for(1).await);
    let (url, server) = spawn_web_server(config).await;
    let response = http_client()
        .get(format!(
            "{url}/api/task-sets/classroom/tasks/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7"
        ))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        code(response).await,
        (412, "credentials_missing".to_owned())
    );
    server.shutdown().await;
    remove_database(db).await;
}

#[tokio::test]
async fn a_malformed_assignment_query_is_refused_in_the_task_error_shape() {
    let (mut config, cookie, db) = signed_in_web_config("task-bad-query");
    config.tasks = Some(unlocked_for(1).await);
    let (url, server) = spawn_task_server(config).await;
    for query in [
        "",
        "?site=https%3A%2F%2Fteacher.github.io%2Fcourse",
        "?version=7&site=x&version=x",
    ] {
        let response = http_client()
            .get(format!(
                "{url}/api/task-sets/classroom/tasks/delimiter-closer{query}"
            ))
            .header("Cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(
            code(response).await,
            (400, "site_invalid".to_owned()),
            "{query}"
        );
    }
    server.shutdown().await;
    remove_database(db).await;
}

#[tokio::test]
async fn only_the_canonical_assignment_path_is_a_task_page() {
    let (config, _, db) = signed_in_web_config("task-page-path");
    let (url, server) = spawn_web_server(config).await;
    let client = http_client();
    let page = client
        .get(format!("{url}/t/classroom/delimiter-closer"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.text().await.unwrap().contains("task.js"));

    // The page reads its set and task from the path as sent, so a doubled slash
    // it would misread is not served as the task page.
    let doubled = client
        .get(format!("{url}/t//classroom/delimiter-closer"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!doubled.contains("task.js"), "{doubled}");
    server.shutdown().await;
    remove_database(db).await;
}

/// GitHub's half of the device flow: a new device code for each start, and
/// each poll answered with the next of `answers`, which names the code it
/// expects. The profile endpoint answers only the token a poll issued, so a
/// sign-in that never polled cannot pass.
async fn spawn_device_github(answers: Vec<(&'static str, Value)>) -> (String, TestServer) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let answers = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from(
        answers,
    )));
    let issued = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let router = axum::Router::new()
        .route(
            "/login/device/code",
            axum::routing::post(move || {
                let code = issued.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                async move {
                    axum::Json(json!({"device_code": format!("device-{code}"),
                        "user_code": "ABCD-1234",
                        "verification_uri": "https://github.com/login/device", "interval": 5,
                        "expires_in": 900}))
                }
            }),
        )
        .route(
            "/login/oauth/access_token",
            axum::routing::post(move |body: String| {
                let answers = answers.clone();
                async move {
                    let posted: Value = serde_json::from_str(&body).unwrap();
                    let (code, answer) = answers.lock().unwrap().pop_front().unwrap();
                    assert_eq!(posted["device_code"], code);
                    assert_eq!(posted["client_id"], "public-client-id");
                    assert_eq!(
                        posted["grant_type"],
                        "urn:ietf:params:oauth:grant-type:device_code"
                    );
                    axum::Json(answer)
                }
            }),
        )
        .route(
            "/user",
            axum::routing::get(|headers: axum::http::HeaderMap| async move {
                if headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    != Some("Bearer device-token")
                {
                    return (axum::http::StatusCode::UNAUTHORIZED, axum::Json(json!({})));
                }
                (
                    axum::http::StatusCode::OK,
                    axum::Json(json!({"id": 404, "login": "learner", "avatar_url": null})),
                )
            }),
        );
    let server = spawn_test_server(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    );
    (format!("http://{addr}"), server)
}

#[tokio::test]
async fn a_device_sign_in_waits_while_pending_and_signs_in_once_approved() {
    let (github, github_server) = spawn_device_github(vec![
        ("device-1", json!({"error": "authorization_pending"})),
        ("device-1", json!({"error": "slow_down", "interval": 15})),
        ("device-1", json!({"error": "access_denied"})),
        ("device-2", json!({"access_token": "device-token"})),
    ])
    .await;
    let (mut config, _, db) = signed_in_web_config("task-device-poll");
    config.github_client_id = Some("public-client-id".to_owned());
    config.github_oauth_base_url = Some(github.clone());
    config.github_api_base_url = Some(github);
    config.tasks = Some(TaskService::new());
    let (url, server) = spawn_web_server(config).await;
    let client = http_client();
    let post = |path: &str, cookie: Option<&str>| {
        let request = client
            .post(format!("{url}{path}"))
            .header("x-codetrial-task-request", "1");
        match cookie {
            Some(cookie) => request.header("Cookie", cookie),
            None => request,
        }
    };

    // Without the device cookie there is nothing to poll for.
    let orphan = post("/api/github/device/poll", None).send().await.unwrap();
    assert_eq!(orphan.status(), 400);
    assert_eq!(orphan.json::<Value>().await.unwrap()["restart"], true);

    let start = || async {
        let start = post("/api/github/device", None).send().await.unwrap();
        assert_eq!(start.status(), 200);
        let device = start.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let shown = start.json::<Value>().await.unwrap();
        assert_eq!(shown["userCode"], "ABCD-1234");
        device
    };
    let device = start().await;

    let poll = |device: String| async move {
        let response = post("/api/github/device/poll", Some(&device))
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let cookies: Vec<String> = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect();
        (status, response.json::<Value>().await.unwrap(), cookies)
    };
    let (status, body, _) = poll(device.clone()).await;
    assert_eq!((status, body), (202, json!({"pending": true})));
    let (status, body, _) = poll(device.clone()).await;
    assert_eq!(
        (status, body),
        (202, json!({"pending": true, "interval": 15}))
    );
    // A declined code is finished with: the page starts a new one.
    let (status, body, _) = poll(device.clone()).await;
    assert_eq!(status, 400);
    assert_eq!(body["restart"], true);
    let device = start().await;
    let (status, body, cookies) = poll(device.clone()).await;
    assert_eq!((status, body), (200, json!({"signedIn": true})));
    let session = cookies
        .iter()
        .find(|cookie| !cookie.starts_with(device.split('=').next().unwrap()))
        .expect("a session cookie")
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let signed_in = client
        .get(format!("{url}/api/session"))
        .header("Cookie", &session)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(signed_in["user"]["login"], "learner");

    server.shutdown().await;
    github_server.shutdown().await;
    remove_database(db).await;
}

/// Only "not signed in" becomes the task error that offers a sign-in. A
/// session that cannot be read is a server fault, and telling the learner to
/// sign in again would send them round a loop that cannot end.
#[tokio::test]
async fn only_a_missing_session_is_answered_as_a_sign_in() {
    let (mut config, cookie, db) = signed_in_web_config("task-owner-fault");
    config.tasks = Some(TaskService::new());
    let (url, server) = spawn_web_server(config).await;
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE sessions")
        .unwrap();
    let response = http_client()
        .get(format!(
            "{url}/api/task-sets/classroom/tasks/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7"
        ))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    assert_ne!(
        response.json::<Value>().await.unwrap()["code"],
        "authentication_required"
    );
    server.shutdown().await;
    remove_database(db).await;
}

#[tokio::test]
async fn a_signed_in_learner_with_no_attempt_in_a_room_has_no_result_there() {
    let (mut config, cookie, db) = signed_in_web_config("task-no-result");
    config.tasks = Some(TaskService::new());
    let (url, server) = spawn_web_server(config).await;
    let response = http_client()
        .get(format!("{url}/api/task-reviews/room-1"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(code(response).await, (404, "result_unavailable".to_owned()));
    server.shutdown().await;
    remove_database(db).await;
}
