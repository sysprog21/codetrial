//! What the browser is allowed to reach: the Content-Security-Policy the
//! pages are served under, the headers that carry it, and the LiveKit origin
//! derivation the policy is built from.

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, Request, header};
use axum::middleware::Next;
use axum::response::Response;

use super::WebServerConfig;

/// Where the browser sends compiled-language runs. Kept in step with the
/// fallback in `web/runners.js`, because the policy below has to name the same
/// origin the page will actually reach for.
pub(crate) const COMPILER_EXPLORER_ORIGIN: &str = "https://godbolt.org";

/// Where the avatar model is fetched from. Kept in step with `MODEL_URL` in
/// `web/avatar/model.js`, which a browser test asserts: the model is not served
/// from this origin any more, so a page that cannot reach this host renders the
/// neutral panel instead of Jim.
const AVATAR_MODEL_ORIGIN: &str = "https://raw.githubusercontent.com";

/// Built once, at startup. Parsing the policy per response was both wasted
/// work and a silent fail-open: a header value that would not parse dropped
/// the whole policy from every response with nothing said about it.
/// `url_origin` rejects anything that is not header-safe, so the fallback
/// below is unreachable and exists only so a bad origin costs the LiveKit host
/// rather than the policy.
pub(crate) fn content_security_policy_header(config: &WebServerConfig) -> HeaderValue {
    HeaderValue::from_str(&content_security_policy(config))
        .unwrap_or_else(|_| HeaderValue::from_static("default-src 'self'"))
}

/// The honest ceiling first: `script-src` has to keep `'unsafe-eval'`, because
/// the JavaScript runner evaluates candidate code inside a blob Worker and blob
/// workers inherit this document's policy, and Pyodide compiles wasm. So this
/// is not a policy that stops script injection outright.
///
/// It still buys the things that need no eval: no framing, no `<base>`
/// rewriting, no plugins, no inline handlers (both pages carry zero), and a
/// `connect-src` that names every origin the page is allowed to talk to
/// instead of all of them.
///
/// Pyodide used to add `https://cdn.jsdelivr.net` to both `script-src` and
/// `connect-src`. It is served from `web/vendor/pyodide/` now.
///
/// The avatar model origin is the one third-party name a page carries
/// unconditionally, including a page with compiled runs withdrawn. It is not
/// gated because there is no switch to gate it on: the avatar is not
/// configurable, and inventing a knob to narrow one CSP entry would be a
/// deployment concept nobody asked for. Two things are true and worth being
/// honest about. `connect-src` matches origins, not paths, so naming the
/// model's host grants every public file on it; and `script-src` already keeps
/// `'unsafe-eval'` for the reason above, so this policy was never the thing
/// standing between injected script and the network. It buys the same thing it
/// bought before: a list a reviewer can read, rather than `*`.
pub(crate) fn content_security_policy(config: &WebServerConfig) -> String {
    let mut connect = vec!["'self'".to_string(), AVATAR_MODEL_ORIGIN.to_string()];
    if config.compiler_explorer_enabled {
        connect.push(COMPILER_EXPLORER_ORIGIN.to_string());
    }

    // Every LiveKit endpoint the page may reach, in one list. The recording
    // template is served from this origin and joins the room the recording
    // project owns; where that project is named separately it may not be one of
    // the pool's, and a template that cannot open its own signaling socket
    // records a page that never joined anything.
    let endpoints = config
        .pool
        .providers
        .iter()
        .map(|it| it.url.as_str())
        .chain(
            config
                .recording
                .as_ref()
                .and_then(|it| it.livekit.as_ref())
                .map(|it| it.url.as_str()),
        );

    // LiveKit Cloud answers `/settings/regions` with a regional hostname and
    // the SDK retries there, so the configured host alone is not enough.
    // Self-hosted deployments have no such indirection and keep their exact
    // origins.
    //
    // One pass, so a third source of endpoints is added to the iterator above
    // and both the exact origins and the wildcard follow from it. The wildcard
    // names the domain it was granted for, because a deployment on one cloud
    // domain has no reason to reach the other.
    let mut cloud: Vec<&'static str> = Vec::new();
    for url in endpoints {
        connect.extend(livekit_origins(url));
        if let Some(domain) = livekit_cloud_domain(url).filter(|it| !cloud.contains(it)) {
            cloud.push(domain);
            connect.push(format!("https://*.{domain}"));
            connect.push(format!("wss://*.{domain}"));
        }
    }
    if !config.production {
        // The browser checks stand up a Compiler Explorer mock and a LiveKit
        // server on loopback ports this process never learns about, so a local
        // run has to allow them. Production excludes these local origins.
        connect.extend(
            [
                "http://127.0.0.1:*",
                "http://localhost:*",
                "ws://127.0.0.1:*",
                "ws://localhost:*",
            ]
            .map(str::to_string),
        );
    }

    [
        "default-src 'self'".to_string(),
        "base-uri 'self'".to_string(),
        "object-src 'none'".to_string(),
        "frame-ancestors 'none'".to_string(),
        "form-action 'self'".to_string(),
        "style-src 'self'".to_string(),
        "font-src 'self'".to_string(),
        "img-src 'self' data: blob:".to_string(),
        "media-src 'self' blob:".to_string(),
        "worker-src 'self' blob:".to_string(),
        "script-src 'self' blob: 'unsafe-eval'".to_string(),
        // `blob:` is here for the avatar, and it is not optional. A .vrm is a
        // GLB, so its textures are always bufferView-backed, and GLTFLoader
        // mints a `blob:` URL per image and hands it to ImageBitmapLoader,
        // which reads it with `fetch`. `fetch` is governed by `connect-src`,
        // and `'self'` does not cover `blob:` any more than it does for the
        // four directives above. Without it every texture fails and the avatar
        // silently falls back to the neutral panel on Chrome.
        format!("connect-src blob: {}", connect.join(" ")),
    ]
    .join("; ")
}

/// Both schemes on the LiveKit host, because the SDK needs both: it opens the
/// signaling WebSocket at the configured `wss://` URL but also makes plain
/// HTTPS calls to the same host, region settings among them. Naming only the
/// `wss://` origin lets the socket open and then blocks those, which surfaces
/// as a connection that fails for no stated reason.
pub(crate) fn livekit_origins(url: &str) -> Vec<String> {
    let Some(origin) = url_origin(url) else {
        return Vec::new();
    };
    let Some((scheme, host)) = origin.split_once("://") else {
        return Vec::new();
    };
    let sibling = match scheme {
        "wss" => "https",
        "ws" => "http",
        "https" => "wss",
        "http" => "ws",
        _ => return vec![origin],
    };
    vec![format!("{sibling}://{host}"), origin]
}

/// The two domains `livekit-client.js` treats as LiveKit Cloud, which is what
/// decides whether it looks up regions at all. Kept in step with the vendored
/// SDK: a domain it considers cloud and this list does not is a connection that
/// fails over to a host the policy never named.
pub(crate) const LIVEKIT_CLOUD_DOMAINS: [&str; 2] = ["livekit.cloud", "livekit.run"];

/// The cloud domain an endpoint sits under, if any. Userinfo and an explicit
/// port are part of the authority but not of the hostname the decision turns
/// on, and a host is case-insensitive where the suffix test is not: an operator
/// who pasted `wss://Project.LiveKit.Cloud` would otherwise get a policy that
/// names the configured host and blocks the region it fails over to, which
/// reads as a connection that dies for no stated reason.
pub(crate) fn livekit_cloud_domain(url: &str) -> Option<&'static str> {
    let origin = url_origin(url)?;
    let (_, authority) = origin.split_once("://")?;
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .split(':')
        .next()
        .unwrap_or(authority)
        .to_ascii_lowercase();
    LIVEKIT_CLOUD_DOMAINS.into_iter().find(|domain| {
        host.strip_suffix(*domain)
            .is_some_and(|rest| rest.ends_with('.'))
    })
}

/// The HTTP origin of a LiveKit endpoint, which is where its REST surface
/// lives. Same host, sibling scheme -- the SDK does this same swap before it
/// fetches `/settings/regions`, and the quota probe needs it for
/// `/rtc/validate`.
pub(crate) fn livekit_http_origin(url: &str) -> Option<String> {
    let origin = url_origin(url)?;
    let (scheme, _, http) = crate::config::livekit_scheme(&origin)?;
    Some(format!("{http}{}", &origin[scheme.len()..]))
}

/// Scheme and authority, without pulling in a URL parser for one field. The
/// printable-ASCII check is what keeps a mistyped `LIVEKIT_URL` from making the
/// whole policy unrepresentable as a header: one origin is dropped instead.
///
/// The scheme is lowered here rather than by each caller. RFC 3986 says it is
/// case-insensitive and `validate_livekit_url` agrees, accepting the scheme
/// with `eq_ignore_ascii_case`, so `WSS://host` is a URL an operator can
/// configure and the server will start on. Every reader below then matches the
/// scheme exactly, and each one fails differently on the uppercase form: the
/// quota probe finds no HTTP origin and treats the project as available, so an
/// exhausted project is never passed over, and `livekit_origins` finds no
/// sibling, so the CSP omits the `https://` origin the SDK needs for
/// `/settings/regions` and the socket opens onto blocked requests. One
/// normalization at the boundary is the alternative to three readers each
/// remembering.
pub(crate) fn url_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    let printable = |value: &str| value.bytes().all(|byte| (0x21..=0x7e).contains(&byte));
    (!scheme.is_empty() && !host.is_empty() && printable(scheme) && printable(host))
        .then(|| format!("{}://{host}", scheme.to_ascii_lowercase()))
}

/// How long a browser should refuse to reach this host over plain HTTP.
///
/// A year, which is what makes the promise worth anything: a shorter window
/// leaves a candidate who last visited outside it open to being downgraded on
/// the hotel wifi they take the interview from.
///
/// `includeSubDomains` is deliberately absent. It is a promise made on behalf
/// of every name under the parent domain, and this process has never seen them:
/// the recording template is served from
/// `CODETRIAL_RECORDING_TEMPLATE_BASE_URL`
/// and may be a sibling host nobody here configured, so the directive could
/// take a service off the air that this server does not own and cannot fix.
/// `preload` is absent for the same reason and one more: it is a submission to
/// a list baked into browser binaries, and getting back off it takes months.
const STRICT_TRANSPORT: &str = "max-age=31536000";

/// The headers every response carries, resolved once at startup.
///
/// A struct rather than the bare policy, because HSTS is conditional and the
/// condition is settled before the first request arrives. Asking the config per
/// response would be work in the hot path to answer a question that cannot
/// change while the process runs.
#[derive(Clone)]
pub(crate) struct SecurityHeaders {
    policy: HeaderValue,
    strict_transport: Option<HeaderValue>,
}

/// Sent only in production, which is the same gate the `Secure` cookie flag
/// uses and for a related reason.
///
/// A user agent must ignore this header when it arrives over plain HTTP, so in
/// development it would be inert rather than wrong. The gate is here for the
/// developer who does terminate TLS locally: without it, one run pins
/// `localhost` for a year in a browser they also use for everything else, and
/// there is no way to serve the retraction over the scheme it now refuses.
pub(crate) fn security_header_state(config: &WebServerConfig) -> SecurityHeaders {
    SecurityHeaders {
        policy: content_security_policy_header(config),
        strict_transport: config
            .production
            .then(|| HeaderValue::from_static(STRICT_TRANSPORT)),
    }
}

pub(crate) async fn security_headers(
    State(headers_for): State<SecurityHeaders>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );

    // Only geolocation, because only geolocation changes anything. Permissions
    // Policy already defaults camera, microphone and geolocation to `self`, so
    // naming the first two would restate the default and buy nothing: a
    // same-origin frame inherits them either way, and a cross-origin one gets
    // them under neither. Nothing here asks for a location, so nothing this
    // page ever embeds should be able to, and `script-src` keeping
    // `'unsafe-eval'` is the reason that is worth stating rather than assuming.
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("geolocation=()"),
    );
    if let Some(strict_transport) = headers_for.strict_transport {
        headers.insert(header::STRICT_TRANSPORT_SECURITY, strict_transport);
    }
    headers.insert(header::CONTENT_SECURITY_POLICY, headers_for.policy);
    response
}

#[cfg(test)]
mod tests {
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

    /// The SDK gates region lookup on `.livekit.cloud` or `.livekit.run`, so
    /// The model is fetched by the browser, so the policy has to name its host
    /// or the download is blocked. Asserted because that failure is silent: a
    /// blocked fetch reaches the page as the same neutral panel every other
    /// avatar failure shows, so dropping this origin breaks the avatar with the
    /// whole suite still green.
    #[test]
    fn the_policy_names_the_host_the_avatar_model_is_fetched_from() {
        let policy = policy_for("wss://example.livekit.cloud");
        assert!(policy.contains(AVATAR_MODEL_ORIGIN), "{policy}");

        // In connect-src and not somewhere else in the string: it is fetched,
        // not executed, so script-src naming it would be both wrong and wider.
        let connect = policy
            .split("; ")
            .find(|directive| directive.starts_with("connect-src "))
            .unwrap_or_default();
        assert!(connect.contains(AVATAR_MODEL_ORIGIN), "{connect}");
    }

    /// both fail over to a regional host and both need the wildcard. Each gets
    /// its own: a deployment on one has no reason to reach the other.
    #[test]
    fn each_cloud_domain_gets_only_its_own_wildcard() {
        let cloud = policy_for("wss://example.livekit.cloud");
        assert!(cloud.contains("https://*.livekit.cloud"), "{cloud}");
        assert!(cloud.contains("wss://*.livekit.cloud"), "{cloud}");
        assert!(!cloud.contains("*.livekit.run"), "{cloud}");

        let run = policy_for("wss://example.livekit.run");
        assert!(run.contains("https://*.livekit.run"), "{run}");
        assert!(run.contains("wss://*.livekit.run"), "{run}");
        assert!(!run.contains("*.livekit.cloud"), "{run}");
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

        // The control characters either end of the printable range, which is
        // what the bound is actually about: a header cannot carry them.
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
        assert_eq!(
            livekit_origins("WSS://host.example"),
            vec![
                "https://host.example".to_string(),
                "wss://host.example".to_string()
            ],
            "the sibling origin is what the SDK reaches for regions"
        );

        // All four schemes, not just the one a cloud deployment uses. A
        // self-hosted LiveKit is reached over `ws://` in development, and the
        // arm that pairs it with `http://` is as load bearing there as the
        // `wss://` arm is in production.
        for (configured, sibling) in [
            ("wss://host.example", "https://host.example"),
            ("ws://host.example", "http://host.example"),
            ("https://host.example", "wss://host.example"),
            ("http://host.example", "ws://host.example"),
        ] {
            assert_eq!(
                livekit_origins(configured),
                vec![sibling.to_string(), configured.to_string()],
                "{configured} must also name {sibling}"
            );
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

        // Scoped to the `endsWith` call that gates region lookup. A bare search
        // for the string would also catch `cloud-api.livekit.io`, which is a
        // service URL and says nothing about failover.
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
}
