//! The `tests` module of `src/web/setup_edit.rs`, which declares this file by
//! path.

use super::*;
use std::sync::Arc;

use crate::config::{PRIMARY_PROVIDER_ID, Provider, ProviderPool};

const NEW_KEY: &str = "new-key";
const NEW_SECRET: &str = "new-secret";

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "codetrial-setup-edit-{}",
            crate::accounts::random_token(12).unwrap()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn config(&self, contents: &str) -> PathBuf {
        let path = self.0.join("codetrial.env.local");
        std::fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn merging_rewrites_only_the_replaced_keys() {
    let contents = "\u{feff}# keep this\r\n LIVEKIT_API_KEY = 'old'\r\nSESSION_SECRET=keep\r\n\
                    #LIVEKIT_API_KEY=commented\r\nLIVEKIT_API_KEY=duplicate\r\nGOOGLE_API_KEYS=a,b\r\n";
    let merged = merge(
        contents,
        &[
            ("LIVEKIT_API_KEY", "new"),
            ("GOOGLE_API_KEY", "google"),
            ("GOOGLE_API_KEYS", ""),
        ],
    );
    assert_eq!(
        merged,
        "\u{feff}# keep this\r\nLIVEKIT_API_KEY=new\r\nSESSION_SECRET=keep\r\n\
         #LIVEKIT_API_KEY=commented\r\nLIVEKIT_API_KEY=new\r\nGOOGLE_API_KEYS=\r\nGOOGLE_API_KEY=google\r\n"
    );

    // Read back the way a launch reads it: the new key replaces the list.
    let mut values = BTreeMap::new();
    crate::config::apply_config_file(&mut values, crate::config::parse_config(&merged).unwrap());
    assert_eq!(crate::config::google_api_keys(&values).unwrap(), ["google"]);

    // An unterminated last line is ended before an appended one, an empty file
    // gets no blank line ahead of its first, and a blank replacement for a key
    // the file does not set adds nothing.
    assert_eq!(
        merge("", &[("LIVEKIT_API_KEY", "new")]),
        "LIVEKIT_API_KEY=new\n"
    );
    assert_eq!(
        merge(
            "LIVEKIT_URL=wss://old.example",
            &[("GOOGLE_API_KEY", "google"), ("GOOGLE_API_KEYS", "")]
        ),
        "LIVEKIT_URL=wss://old.example\nGOOGLE_API_KEY=google\n"
    );
}

#[test]
fn only_a_standalone_loopback_launch_without_a_proxy_may_change_keys() {
    let scratch = Scratch::new();
    let path = scratch.config("LIVEKIT_API_KEY=old\n");
    let context = |path: &Path, bound: &str, standalone, hops| {
        SetupContext::new(
            path.to_path_buf(),
            BTreeMap::new(),
            bound.parse().unwrap(),
            standalone,
            hops,
        )
        .unwrap()
    };
    assert!(context(&path, "127.0.0.1:3000", true, 0).is_some());
    assert!(context(&path, "[::1]:3000", true, 0).is_some());
    for (bound, standalone, hops) in [
        ("0.0.0.0:3000", true, 0),
        ("127.0.0.1:3000", false, 0),
        ("127.0.0.1:3000", true, 1),
    ] {
        assert!(
            context(&path, bound, standalone, hops).is_none(),
            "{bound} {standalone} {hops}"
        );
    }
    #[cfg(unix)]
    {
        let link = scratch.0.join("link.env");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(context(&link, "127.0.0.1:3000", true, 0).is_none());
    }
}

/// A LiveKit project that takes `NEW_KEY` and `NEW_SECRET` and nothing else,
/// checked through the crate's own verifier.
async fn livekit() -> (String, tokio::task::JoinHandle<()>) {
    livekit_calling(|| {}).await
}

/// `livekit`, running `meanwhile` while a probe waits for its answer.
async fn livekit_calling(
    meanwhile: impl Fn() + Clone + Send + Sync + 'static,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let router = axum::Router::new().route(
        "/twirp/livekit.RoomService/ListRooms",
        axum::routing::post(move |headers: HeaderMap| async move {
            meanwhile();
            let token = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
                .unwrap_or("");
            let accepted = crate::token::livekit_token_issuer(token).as_deref() == Some(NEW_KEY)
                && token.rsplit_once('.').is_some_and(|(input, signature)| {
                    crate::token::verify_hs256(NEW_SECRET, input, signature)
                });
            if accepted {
                (StatusCode::OK, "{\"rooms\":[]}")
            } else {
                (StatusCode::UNAUTHORIZED, "invalid api key")
            }
        }),
    );
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, task)
}

/// Starts no interviewer and is never running one.
struct Idle;

impl RoomDispatcher for Idle {
    fn is_idle(&self) -> bool {
        true
    }

    fn supports_guest_admission(&self) -> bool {
        true
    }

    fn ensure_agent(
        &self,
        _: &str,
        _: &Provider,
        _: super::super::AgentJob,
    ) -> Result<(), super::super::DispatchRefusal> {
        Ok(())
    }

    fn ensure_guest_agent(
        &self,
        _: &str,
        _: &Provider,
        _: &str,
    ) -> Result<(), super::super::DispatchRefusal> {
        Ok(())
    }
}

struct Server {
    scratch: Scratch,
    path: PathBuf,
    contents: String,
    base: String,
    code: String,
    client: reqwest::Client,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    /// A running server whose config names the LiveKit project at `livekit`
    /// with keys that project refuses, so a save has to bring the new pair.
    async fn start(livekit: &str, dispatcher: Option<Arc<dyn RoomDispatcher>>) -> Self {
        let scratch = Scratch::new();
        let contents = format!(
            "# keep this\nLIVEKIT_URL={livekit}\nLIVEKIT_API_KEY=old-key\n\
             LIVEKIT_API_SECRET=old-secret\nSESSION_SECRET=keep-session\n"
        );
        let path = scratch.config(&contents);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound = listener.local_addr().unwrap();
        let values = crate::config::parse_config(&contents)
            .unwrap()
            .into_iter()
            .collect();
        let context = SetupContext::new(path.clone(), values, bound, true, 0)
            .unwrap()
            .unwrap();
        let code = context.code().to_string();
        let config = super::super::WebServerConfig {
            web_dir: PathBuf::new(),
            github_client_id: None,
            github_client_secret: None,
            session_secret: None,
            db_path: None,
            github_oauth_base_url: None,
            github_api_base_url: None,
            room_prefix: "interview".to_string(),
            fixed_room_name: None,
            production: false,
            compiler_explorer_enabled: false,
            guest_mode_enabled: true,
            trusted_proxy_hops: 0,
            allowed_hosts: Vec::new(),
            recording: None,
            probe_provider_quota: false,
            provision_rooms: false,
            update_check_commit: None,
            tasks: None,
            task_mode_requested: false,
            pool: ProviderPool {
                providers: vec![Provider {
                    id: PRIMARY_PROVIDER_ID.to_string(),
                    url: livekit.to_string(),
                    api_key: "old-key".to_string(),
                    api_secret: "old-secret".to_string(),
                    google_api_keys: Vec::new(),
                }],
            },
        };
        let service = super::super::web_router(config, dispatcher, None, Some(Arc::new(context)))
            .into_make_service_with_connect_info::<SocketAddr>();
        let task = tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });
        Self {
            scratch,
            path,
            contents,
            base: format!("http://{bound}"),
            code,
            client: crate::bundled_roots_client_builder()
                .no_proxy()
                .build()
                .unwrap(),
            task,
        }
    }

    async fn save(&self, body: Value) -> (StatusCode, Value) {
        let response = self
            .client
            .post(format!("{}/api/setup", self.base))
            .json(&body)
            .send()
            .await
            .unwrap();
        (response.status(), response.json().await.unwrap())
    }

    async fn summary(&self) -> Value {
        self.client
            .get(format!("{}/api/setup", self.base))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    /// The config as it was, and nothing left beside it.
    fn unchanged(&self) {
        assert_eq!(std::fs::read_to_string(&self.path).unwrap(), self.contents);
        assert_eq!(std::fs::read_dir(&self.scratch.0).unwrap().count(), 1);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn change_keys_answers_only_its_loopback_host_and_code() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use sha2::Digest as _;

    let (livekit, _livekit) = livekit().await;
    let server = Server::start(&livekit, None).await;
    for (method, path) in [
        (reqwest::Method::GET, "/setup"),
        (reqwest::Method::GET, "/api/setup"),
        (reqwest::Method::POST, "/api/setup"),
    ] {
        let status = server
            .client
            .request(method, format!("{}{path}", server.base))
            .header(header::HOST, "rebound.example")
            .json(&json!({ "code": server.code }))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }

    let summary = server.summary().await;
    assert_eq!(
        summary,
        json!({
            "configured": true,
            "livekitEndpoint": livekit,
            "googleApiKey": false,
            "restartPending": false,
        })
    );
    for body in [
        json!({ "livekitApiKey": NEW_KEY, "livekitApiSecret": NEW_SECRET }),
        json!({ "code": "wrong", "livekitApiKey": NEW_KEY, "livekitApiSecret": NEW_SECRET }),
    ] {
        assert_eq!(server.save(body).await.0, StatusCode::FORBIDDEN);
    }
    server.unchanged();

    // The page's own script and style are all its policy lets run.
    let page = server
        .client
        .get(format!("{}/setup", server.base))
        .send()
        .await
        .unwrap();
    let policy = page.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .to_string();
    let page = page.text().await.unwrap();
    for tag in ["script", "style"] {
        let (_, rest) = page.split_once(&format!("<{tag}>")).unwrap();
        let (inline, _) = rest.split_once(&format!("</{tag}>")).unwrap();
        let allowed = format!(
            "{tag}-src 'sha256-{}'",
            STANDARD.encode(sha2::Sha256::digest(inline.as_bytes()))
        );
        assert!(
            policy.split("; ").any(|directive| directive == allowed),
            "{policy}"
        );
    }
    assert!(!policy.contains("unsafe-inline"), "{policy}");
}

#[tokio::test]
async fn a_save_writes_only_the_new_keys_and_this_process_keeps_the_old_ones() {
    let (livekit, _livekit) = livekit().await;
    let server = Server::start(&livekit, Some(Arc::new(Idle))).await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&server.path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    let (status, body) = server
        .save(json!({
            "code": server.code,
            "livekitUrl": "",
            "livekitApiKey": NEW_KEY,
            "livekitApiSecret": NEW_SECRET,
        }))
        .await;
    assert_eq!((status, body), (StatusCode::OK, json!({ "saved": true })));
    assert_eq!(
        std::fs::read_to_string(&server.path).unwrap(),
        format!(
            "# keep this\nLIVEKIT_URL={livekit}\nLIVEKIT_API_KEY={NEW_KEY}\n\
             LIVEKIT_API_SECRET={NEW_SECRET}\nSESSION_SECRET=keep-session\n"
        )
    );
    assert_eq!(std::fs::read_dir(&server.scratch.0).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&server.path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // Saved for the next start, not for this one: the page says a restart is
    // due, a second save waits for it, and tokens still name the old key.
    assert_eq!(server.summary().await["restartPending"], true);
    let (status, _) = server
        .save(json!({ "code": server.code, "livekitApiKey": "another-key" }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let token: Value = server
        .client
        .post(format!("{}/api/token", server.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        crate::token::livekit_token_issuer(token["token"].as_str().unwrap()).as_deref(),
        Some("old-key")
    );
}

#[tokio::test]
async fn a_save_with_nothing_to_change_or_keys_that_fail_leaves_the_file() {
    let (livekit, _livekit) = livekit().await;
    let server = Server::start(&livekit, None).await;
    for body in [
        json!({ "code": server.code, "livekitUrl": " ", "googleApiKey": "" }),
        json!({ "code": server.code, "livekitApiKey": "old-key" }),
        json!({ "code": server.code, "livekitApiKey": NEW_KEY, "livekitApiSecret": "wrong" }),
        json!({ "code": server.code, "livekitApiKey": "key\nSESSION_SECRET=injected" }),
    ] {
        let (status, error) = server.save(body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    }
    server.unchanged();
    assert_eq!(server.summary().await["restartPending"], false);
}

/// Says nothing about whether an interview is running, so it keeps the
/// trait's answer.
struct Unknown;

impl RoomDispatcher for Unknown {
    fn ensure_agent(
        &self,
        _: &str,
        _: &Provider,
        _: super::super::AgentJob,
    ) -> Result<(), super::super::DispatchRefusal> {
        Ok(())
    }
}

/// A save waits until no interview is running, and a dispatcher that cannot
/// tell is taken to have one. `LocalDispatcher` can, and how it counts its
/// rooms is tested in `tests/unit/dispatch.rs`.
#[tokio::test]
async fn a_dispatcher_that_cannot_rule_out_an_interview_refuses_the_save() {
    let (livekit, _livekit) = livekit().await;
    let server = Server::start(&livekit, Some(Arc::new(Unknown))).await;
    let (status, _) = server
        .save(json!({
            "code": server.code,
            "livekitApiKey": NEW_KEY,
            "livekitApiSecret": NEW_SECRET,
        }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    server.unchanged();
}

/// The file is compared again once the keys are checked, which can take
/// seconds, so an edit saved meanwhile is kept and the save refused.
#[tokio::test]
async fn an_edit_saved_while_the_keys_are_checked_is_kept() {
    let config = Arc::new(std::sync::OnceLock::<PathBuf>::new());
    let (livekit, _livekit) = livekit_calling({
        let config = Arc::clone(&config);
        move || {
            if let Some(path) = config.get() {
                let contents = std::fs::read_to_string(path).unwrap();
                std::fs::write(path, contents.replace("keep-session", "edited-session")).unwrap();
            }
        }
    })
    .await;
    let server = Server::start(&livekit, Some(Arc::new(Idle))).await;
    config.set(server.path.clone()).unwrap();
    let (status, error) = server
        .save(json!({
            "code": server.code,
            "livekitApiKey": NEW_KEY,
            "livekitApiSecret": NEW_SECRET,
        }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(
        std::fs::read_to_string(&server.path).unwrap(),
        server.contents.replace("keep-session", "edited-session")
    );
    assert_eq!(std::fs::read_dir(&server.scratch.0).unwrap().count(), 1);
}

#[cfg(windows)]
#[test]
fn windows_replacement_is_owner_only_in_a_folder_everyone_can_read() {
    use windows_permissions::constants::{SeObjectType, SecurityInformation};
    use windows_permissions::wrappers::{GetNamedSecurityInfo, SetNamedSecurityInfo};
    use windows_permissions::{LocalBox, SecurityDescriptor};

    let scratch = Scratch::new();
    let path = scratch.config("LIVEKIT_API_KEY=old\n");
    let owner = GetNamedSecurityInfo(
        &path,
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Owner,
    )
    .unwrap()
    .owner()
    .unwrap()
    .to_string();

    // A DACL as its flags and its entries. Windows sets flags of its own, such
    // as `AI`, so of those only `P`, protected, is asserted.
    let split = |sddl: std::ffi::OsString| {
        let sddl = sddl.to_string_lossy().into_owned();
        let (flags, entries) = sddl.split_once('(').unwrap();
        (flags.to_string(), format!("({entries}"))
    };
    let dacl = |target: &Path| {
        split(
            GetNamedSecurityInfo(
                target,
                SeObjectType::SE_FILE_OBJECT,
                SecurityInformation::Dacl,
            )
            .unwrap()
            .as_sddl()
            .unwrap(),
        )
    };
    let protect = |target: &Path, sddl: String| {
        let descriptor: LocalBox<SecurityDescriptor> = sddl.parse().unwrap();
        SetNamedSecurityInfo(
            target,
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
            None,
            None,
            descriptor.dacl(),
            None,
        )
        .unwrap();
    };

    // Everyone may read what the folder holds, and the config is its owner's.
    protect(
        &scratch.0,
        format!("D:P(A;OICI;FA;;;{owner})(A;OICI;FR;;;WD)"),
    );
    protect(&path, format!("D:P(A;;FA;;;{owner})"));
    assert!(dacl(&scratch.0).1.contains(";WD)"));

    replace_file(&path, "LIVEKIT_API_KEY=new\n").unwrap();
    let (_, owner_only) = split(
        format!("D:P(A;;FA;;;{owner})")
            .parse::<LocalBox<SecurityDescriptor>>()
            .unwrap()
            .as_sddl()
            .unwrap(),
    );
    let (flags, entries) = dacl(&path);
    assert!(flags.contains('P'), "{flags}");
    assert_eq!(entries, owner_only);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "LIVEKIT_API_KEY=new\n"
    );
}
