//! The `tests` module of `src/web/policy.rs`, which declares this file by path.
//! Everything here reaches into `src/web/policy.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use super::*;

/// The policy a server with this one LiveKit endpoint would serve. Built by
/// calling the function rather than by standing a server up: what these
/// cases pin is which origins the string names, and `tests/web.rs` already
/// proves the string reaches a response as a header.
fn policy_for(url: &str) -> String {
    content_security_policy(&WebServerConfig {
        web_dir: std::path::PathBuf::new(),
        github_client_id: None,
        github_client_secret: None,
        session_secret: None,
        db_path: None,
        github_oauth_base_url: None,
        github_api_base_url: None,
        room_prefix: "interview".to_string(),
        fixed_room_name: None,
        production: true,
        compiler_explorer_enabled: false,
        trusted_proxy_hops: 0,
        recording: None,
        pool: crate::config::ProviderPool {
            providers: vec![crate::config::Provider {
                id: crate::config::PRIMARY_PROVIDER_ID.to_string(),
                url: url.to_string(),
                api_key: "devkey".to_string(),
                api_secret: "devsecret".to_string(),
                google_api_key: String::new(),
            }],
        },
        probe_provider_quota: false,
    })
}

/// The model is fetched by the browser, so the policy has to name its host
/// or the download is blocked. Asserted because that failure is silent: a
/// blocked fetch reaches the page as the same neutral panel every other
/// avatar failure shows, so dropping this origin breaks the avatar with the
/// whole suite still green.
#[test]
fn the_policy_names_the_host_the_avatar_model_is_fetched_from() {
    let policy = policy_for("wss://example.livekit.cloud");
    assert!(policy.contains(AVATAR_MODEL_ORIGIN), "{policy}");

    // In connect-src and not somewhere else in the string: it is fetched, not
    // executed, so script-src naming it would be both wrong and wider.
    let connect = policy
        .split("; ")
        .find(|directive| directive.starts_with("connect-src "))
        .unwrap_or_default();
    assert!(connect.contains(AVATAR_MODEL_ORIGIN), "{connect}");
}

/// Both cloud domains fail over to regional hosts and need their own wildcard.
/// A deployment on one has no reason to reach the other.
#[test]
fn cloud_domains_admit_regional_hosts_without_widening_to_each_other() {
    for (url, domain, other_domain) in [
        (
            "wss://example.livekit.cloud",
            "livekit.cloud",
            "livekit.run",
        ),
        ("wss://example.livekit.run", "livekit.run", "livekit.cloud"),
    ] {
        let policy = policy_for(url);
        assert!(policy.contains(&format!("https://*.{domain}")), "{policy}");
        assert!(policy.contains(&format!("wss://*.{domain}")), "{policy}");
        assert!(!policy.contains(&format!("*.{other_domain}")), "{policy}");
    }

    // Regional signaling hosts have labels below both the project and cloud
    // domains. The configured project's wildcard must therefore cover this
    // shape, not merely a sibling project directly under `livekit.cloud`.
    assert_eq!(
        livekit_cloud_domain("wss://conversation-xxx.otokyo1b.production.livekit.cloud"),
        Some("livekit.cloud")
    );
}

/// A self-hosted server has no region indirection, so widening its policy
/// to a shared SaaS domain would buy nothing and reach every tenant on it.
/// Nor can a host earn the wildcard by merely ending in the domain's
/// characters, by hiding it in a path, or by carrying userinfo.
#[test]
fn only_a_host_actually_under_a_cloud_domain_earns_the_wildcard() {
    for url in [
        "wss://livekit.example.org",
        "wss://evil-livekit.cloud",
        "wss://attacker.example/.livekit.cloud",
        "wss://livekit.cloud.attacker.example",
        "wss://user@attacker.example",
    ] {
        let policy = policy_for(url);
        assert!(
            !policy.contains("*.livekit."),
            "{url} earned a wildcard: {policy}"
        );
    }
    // The configured host itself is still named, wildcard or not.
    let policy = policy_for("wss://livekit.example.org");
    assert!(policy.contains("wss://livekit.example.org"), "{policy}");
}

/// A host is case-insensitive and may carry userinfo or a port; the suffix
/// test is none of those things on its own.
/// Each conjunct in `url_origin`'s guard, exercised by the input only it
/// rejects. The guard is what keeps a mistyped `LIVEKIT_URL` from making
/// the whole policy unrepresentable as a header, and every one of its four
/// terms was reachable with `&&` replaced by `||` without a test noticing.
#[test]
fn a_malformed_url_is_refused_one_term_at_a_time() {
    assert_eq!(url_origin("://host.example"), None, "empty scheme");
    assert_eq!(url_origin("wss://"), None, "empty host");
    assert_eq!(url_origin("wss://host .example"), None, "space in the host");
    assert_eq!(
        url_origin("w s://host.example"),
        None,
        "space in the scheme"
    );

    // The control characters either end of the printable range, which is what
    // the bound is actually about: a header cannot carry them.
    assert_eq!(
        url_origin("wss://host\u{7f}.example"),
        None,
        "DEL in the host"
    );
    assert_eq!(
        url_origin("wss://host\u{1}.example"),
        None,
        "SOH in the host"
    );

    assert_eq!(
        url_origin("wss://host.example:7880/rtc?x=1"),
        Some("wss://host.example:7880".to_string()),
        "a well-formed URL still keeps its port and loses its path"
    );
}

/// A server-side URL may need userinfo, but a CSP source is only a scheme and
/// host. Leaving the userinfo in makes the browser discard the entry, which is
/// a silent connection failure for a self-hosted deployment.
#[test]
fn csp_origins_strip_userinfo() {
    let policy = policy_for("wss://key:secret@project.example:7880");
    assert!(policy.contains("https://project.example:7880"), "{policy}");
    assert!(policy.contains("wss://project.example:7880"), "{policy}");
    assert!(!policy.contains("key:secret"), "{policy}");
}

/// `validate_livekit_url` accepts the scheme case-insensitively, so an
/// uppercase one is a URL the server starts on. Every reader here matches
/// the scheme exactly, and each fails differently: the quota probe finds no
/// HTTP origin and treats an exhausted project as available, and the policy
/// loses the sibling origin the SDK needs for `/settings/regions`.
#[test]
fn an_uppercase_scheme_is_normalized_before_anything_matches_on_it() {
    assert_eq!(
        url_origin("WSS://Example.LiveKit.Cloud"),
        Some("wss://Example.LiveKit.Cloud".to_string()),
        "the scheme is case-insensitive per RFC 3986; the host is left alone"
    );
    assert_eq!(
        livekit_http_origin("WSS://host.example"),
        Some("https://host.example".to_string()),
        "without this the quota probe never runs and the project is assumed available"
    );

    // All four schemes, not just the one a cloud deployment uses. A self-hosted
    // LiveKit is reached over `ws://` in development, and the arm that pairs it
    // with `http://` is as load bearing there as the `wss://` arm is in
    // production.
    for (configured, sibling) in [
        ("wss://host.example", "https://host.example"),
        ("ws://host.example", "http://host.example"),
        ("https://host.example", "wss://host.example"),
        ("http://host.example", "ws://host.example"),
    ] {
        let policy = policy_for(configured);
        assert!(policy.contains(sibling), "{configured}: {policy}");
        assert!(policy.contains(configured), "{configured}: {policy}");
    }

    let policy = policy_for("WSS://uppercase-scheme.livekit.cloud");
    assert!(
        policy.contains("wss://uppercase-scheme.livekit.cloud")
            && policy.contains("https://uppercase-scheme.livekit.cloud"),
        "both origins must survive an uppercase scheme: {policy}"
    );
}

#[test]
fn a_cloud_host_is_recognized_through_case_userinfo_and_port() {
    for url in [
        "wss://Project.LiveKit.Cloud",
        "wss://token@project.livekit.cloud",
        "wss://project.livekit.cloud:443",
    ] {
        let policy = policy_for(url);
        assert!(policy.contains("wss://*.livekit.cloud"), "{url}: {policy}");
    }
}

/// `LIVEKIT_CLOUD_DOMAINS` mirrors a predicate inside the vendored SDK, and
/// nothing else keeps the two in step. An SDK bump that adds a third cloud
/// domain would otherwise surface in production as a failover to a host the
/// policy never named, which is the one failure this list exists to stop.
#[test]
fn the_domain_list_still_matches_the_vendored_sdk() {
    let sdk = std::fs::read_to_string("web/vendor/livekit-client.js")
        .expect("the vendored LiveKit SDK is fetched");

    // Scoped to the `endsWith` call that gates region lookup. A bare search for
    // the string would also catch `cloud-api.livekit.io`, which is a service
    // URL and says nothing about failover.
    let mut found: Vec<String> = sdk
        .match_indices("endsWith(\".livekit.")
        .filter_map(|(at, hit)| {
            // Past the opening dot, so this reads as the bare domain the
            // constant holds rather than the suffix the SDK tests with.
            let rest = &sdk[at + hit.len() - ".livekit.".len() + 1..];
            Some(rest[..rest.find("\")")?].to_string())
        })
        .collect();
    found.sort();
    found.dedup();
    assert_eq!(
        found, LIVEKIT_CLOUD_DOMAINS,
        "the SDK's cloud domains drifted from LIVEKIT_CLOUD_DOMAINS"
    );
}

/// Three directives that look redundant next to `default-src 'self'` and are
/// not. Dropping one does not remove a rule, it falls back to `default-src`,
/// which names `'self'` and nothing else.
///
/// The avatar is not assembled out of files under the origin. A `.vrm` is a
/// GLB, so every texture in it is bufferView-backed: GLTFLoader mints a
/// `blob:` URL per image, and the same is true of the recorded audio the
/// page plays back. `data:` is there for the inline placeholder the panel
/// shows before the model arrives. `font-src` is the one whose fallback is
/// currently identical, and it is pinned with the others because the value
/// this test defends is the directive being stated rather than inherited:
/// the day `default-src` is narrowed, an inherited rule changes with it and
/// nothing here would have said which of the three were meant to.
///
/// Asserted as the whole directive, not with `contains`, because the schemes
/// are the point and a directive reduced to `'self'` still contains its own
/// name. The failure this stops is silent in the browser too: a blocked
/// texture reaches the page as the same neutral panel every other avatar
/// failure shows.
#[test]
fn the_policy_admits_the_schemes_the_avatar_is_assembled_from() {
    let policy = policy_for("wss://example.livekit.cloud");
    let directive = |name: &str| {
        policy
            .split("; ")
            .find(|directive| directive.split(' ').next() == Some(name))
            .map(str::to_string)
    };

    assert_eq!(
        directive("img-src"),
        Some("img-src 'self' data: blob:".to_string()),
        "{policy}"
    );
    assert_eq!(
        directive("media-src"),
        Some("media-src 'self' blob:".to_string()),
        "{policy}"
    );
    assert_eq!(
        directive("font-src"),
        Some("font-src 'self'".to_string()),
        "{policy}"
    );

    // The fallback they would inherit, named here so the paragraph above is
    // checked rather than asserted: it really does carry `'self'` alone.
    assert_eq!(
        directive("default-src"),
        Some("default-src 'self'".to_string()),
        "{policy}"
    );
}
