use super::*;

#[tokio::test]
async fn peer_sign_in_returns_only_to_fixed_destination() {
    let (github_base, github_server) = spawn_mock_github().await;
    let path = account_db_path("peer-auth");
    let mut config = web_config();
    config.github_client_id = Some("client".into());
    config.github_client_secret = Some("secret".into());
    config.session_secret = Some("session-secret".into());
    config.db_path = Some(path.clone());
    config.github_oauth_base_url = Some(github_base.clone());
    config.github_api_base_url = Some(github_base);
    initialize_account_database(&path).unwrap();
    let (base, server) = spawn_web_server(config).await;
    let client = http_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let session: Value = client
        .get(format!("{base}/api/session"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["peerEnabled"], false);
    for (next, expected) in [
        ("peer-join", "/peer-lobby"),
        ("https://evil.example", "/"),
        ("//evil.example", "/"),
        ("", "/"),
    ] {
        let login = client
            .get(format!("{base}/api/login"))
            .query(&[("next", next)])
            .send()
            .await
            .unwrap();
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
            .query(&[("code", "ok"), ("state", &state)])
            .header("cookie", cookies)
            .send()
            .await
            .unwrap();
        assert_eq!(callback.status(), 302);
        assert_eq!(callback.headers()["location"], expected);
    }
    server.shutdown().await;
    github_server.shutdown().await;
    remove_database(path).await;
}

#[tokio::test]
async fn peer_availability_is_reported_before_sign_in() {
    let mut config = web_config();
    config.peer = Some(codetrial::config::PeerConfig::default());
    let (base, server) = spawn_web_server(config).await;
    let session: Value = http_client_builder()
        .build()
        .unwrap()
        .get(format!("{base}/api/session"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["peerEnabled"], true);
    assert_eq!(session["signedIn"], false);
    server.shutdown().await;
}
