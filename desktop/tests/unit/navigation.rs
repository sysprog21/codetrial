use std::collections::HashSet;
use std::path::Path;

use super::*;

fn url(text: &str) -> Url {
    Url::parse(text).expect("test URLs parse")
}

#[test]
fn only_the_server_and_the_start_page_load_in_the_window() {
    let server = url("http://127.0.0.1:3101/");
    for start in [
        "tauri://localhost/index.html",
        "http://tauri.localhost/index.html",
    ] {
        let start = url(start);
        assert_eq!(
            judge(&url("http://127.0.0.1:3101/interview?x=1"), &server, &start),
            Verdict::Server
        );
        assert_eq!(
            judge(&start.join("index.html?error=x").unwrap(), &server, &start),
            Verdict::StartPage(Ask::Nothing)
        );

        // Another port on the same host is another server, and the name
        // `localhost` is another origin for the page's storage and cookies.
        assert_eq!(
            judge(&url("http://127.0.0.1:3102/"), &server, &start),
            Verdict::OpenInBrowser
        );
        assert_eq!(
            judge(&url("http://localhost:3101/"), &server, &start),
            Verdict::OpenInBrowser
        );
        assert_eq!(
            judge(&url("https://github.com/login"), &server, &start),
            Verdict::OpenInBrowser
        );

        for refused in [
            "file:///etc/passwd",
            "data:text/html,x",
            "javascript:alert(1)",
            "about:blank",
        ] {
            assert_eq!(
                judge(&url(refused), &server, &start),
                Verdict::Refuse,
                "{refused}"
            );
        }
    }
}

#[test]
fn only_the_start_page_can_ask_for_the_browser_or_report_webrtc() {
    let server = url("http://127.0.0.1:3101/");
    let start = url("tauri://localhost/index.html?browser=x");
    for (page, ask) in [
        ("tauri://localhost/index.html?reopen", Ask::Reopen),
        ("tauri://localhost/index.html?webrtc=yes", Ask::WebRtc(true)),
        ("tauri://localhost/index.html?webrtc=no", Ask::WebRtc(false)),
        ("tauri://localhost/index.html?browser=x", Ask::Nothing),
        ("tauri://localhost/index.html?error=x", Ask::Nothing),
    ] {
        assert_eq!(
            judge(&url(page), &server, &start),
            Verdict::StartPage(ask),
            "{page}"
        );
    }

    // The same query from the server is a page of the server's, not a request.
    for page in [
        "http://127.0.0.1:3101/?reopen",
        "http://127.0.0.1:3101/?webrtc=yes",
    ] {
        assert_eq!(judge(&url(page), &server, &start), Verdict::Server);
    }
}

#[test]
fn the_first_navigation_is_the_start_page_on_either_scheme() {
    let server = url("http://127.0.0.1:3101/");

    // What the window loads first: Tauri drops `index.html` from the URL of
    // `WebviewUrl::App("index.html")`, as the tab #510 opened showed.
    for (http_scheme, first) in [
        (true, "http://tauri.localhost/"),
        (false, "tauri://localhost"),
    ] {
        let start = start_page(http_scheme);
        assert_eq!(
            judge(&url(first), &server, &start),
            Verdict::StartPage(Ask::Nothing),
            "{first}"
        );

        // The page's own report reaches the app instead of the browser, and so
        // does a failure shown before the page ever loaded.
        let webrtc = url(first).join("index.html?webrtc=yes").unwrap();
        assert_eq!(
            judge(&webrtc, &server, &start),
            Verdict::StartPage(Ask::WebRtc(true))
        );
        let mut failure = start.clone();
        failure.query_pairs_mut().append_pair("error", "x");
        assert_eq!(
            judge(&failure, &server, &start),
            Verdict::StartPage(Ask::Nothing)
        );

        // Against the other platform's start page the window's own page is a
        // stranger: sent to the browser over http, as #510 saw, refused over
        // tauri.
        assert_eq!(
            judge(&url(first), &server, &start_page(!http_scheme)),
            if http_scheme {
                Verdict::OpenInBrowser
            } else {
                Verdict::Refuse
            }
        );
    }
}

#[test]
fn a_download_keeps_its_name_and_never_overwrites() {
    let dir = std::env::temp_dir().join(format!(
        "codetrial-desktop-download-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let none = HashSet::new();

    let first = download_path(&dir, Path::new("/tmp/suggested/report.json"), &none);
    assert_eq!(first, dir.join("report.json"));
    std::fs::write(&first, "").unwrap();
    let second = download_path(&dir, Path::new("report.json"), &none);
    assert_eq!(second, dir.join("report (1).json"));
    std::fs::write(&second, "").unwrap();
    assert_eq!(
        download_path(&dir, Path::new("report.json"), &none),
        dir.join("report (2).json")
    );

    assert_eq!(
        download_path(&dir, Path::new(""), &none),
        dir.join("download")
    );
    assert_eq!(
        download_path(&dir, Path::new(".profile"), &none),
        dir.join(".profile")
    );
    assert_eq!(
        download_path(&dir, Path::new("../../escape.txt"), &none),
        dir.join("escape.txt")
    );

    // Two downloads in flight before either writes: the second is numbered past
    // the name the first was given, though nothing is on disk yet.
    let mut given = HashSet::new();
    let pending = download_path(&dir, Path::new("notes.txt"), &given);
    assert_eq!(pending, dir.join("notes.txt"));
    given.insert(pending);
    assert_eq!(
        download_path(&dir, Path::new("notes.txt"), &given),
        dir.join("notes (1).txt")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_the_servers_setup_page_and_the_browser_start_page_get_the_code() {
    let server = url("http://127.0.0.1:3101/");
    for start in [
        "tauri://localhost/index.html",
        "http://tauri.localhost/index.html",
    ] {
        let start = url(start);
        let code_for = |page: &str| change_keys_code_for(&url(page), &server, &start);
        assert_eq!(
            code_for("http://127.0.0.1:3101/setup"),
            Some(ChangeKeysCode::Fill)
        );
        assert_eq!(
            code_for(start.join("index.html?browser=").unwrap().as_str()),
            Some(ChangeKeysCode::Show)
        );

        // The server's own `?browser` is just a server page, and only its Setup
        // page has the field: not another path, not another port, not the start
        // page while it is starting, failing or asking.
        for nothing in [
            "http://127.0.0.1:3101/?browser=",
            "http://127.0.0.1:3101/",
            "http://127.0.0.1:3101/setup/x",
            "http://127.0.0.1:3102/setup",
            "http://localhost:3101/setup",
        ] {
            assert_eq!(code_for(nothing), None, "{nothing}");
        }
        for query in ["", "?error=x", "?webrtc=no", "?reopen", "?browser=&reopen"] {
            let page = start.join(&format!("index.html{query}")).unwrap();
            assert_eq!(code_for(page.as_str()), None, "{page}");
        }
        assert_eq!(code_for(start.join("setup").unwrap().as_str()), None);
    }
}
