//! The `tests` module of `src/lib.rs`, which declares this file by path.
//! Everything here reaches into `src/lib.rs` through `super`, so it is a unit
//! test and not an integration test: private items are in scope.

/// The empty path is what a `PathBuf` defaults to, and joining a file name
/// onto it yields that name alone, which resolves against the working
/// directory. That is the fallback this function has for when the
/// executable cannot be located, not the answer it gives when it can, and
/// the difference decides where a released binary looks for its config.
#[test]
fn exe_dir_names_the_folder_the_running_binary_sits_in() {
    let exe = std::env::current_exe().expect("a running test binary has a path");
    assert_eq!(
        super::exe_dir(),
        exe.parent().expect("an executable sits in a folder")
    );
}

#[test]
fn sha256_hex_matches_known_vectors_and_concatenates_parts() {
    assert_eq!(
        super::sha256_hex(&[]),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        super::sha256_hex(&[b"39"]),
        "0b918943df0962bc7a1824c0555a389347b4febdc7cf9d1254406d80ce44e3f9"
    );
    for parts in [vec![b"abc".as_slice()], vec![b"a".as_slice(), b"", b"bc"]] {
        assert_eq!(
            super::sha256_hex(&parts),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}

/// The 30-second backstop is why these clients exist instead of
/// `reqwest::Client::new()`, and no test can afford to wait it out. reqwest
/// prints a client's total timeout in its `Debug` output and leaves the field
/// out when none is set, so that is where the backstop is visible. A reqwest
/// that stops printing it fails here loudly rather than passing quietly.
#[test]
fn shared_http_clients_carry_the_backstop_timeout() {
    for client in [super::http_client(), super::no_redirect_http_client()] {
        let debug = format!("{client:?}");
        assert!(debug.contains("TotalTimeout: 30s"), "{debug}");
    }
}

/// LiveKit's transport builds its TLS client on the process-wide provider, so
/// this has to leave ring installed. Checked in a child, because any test in
/// this process that joined a room may have installed one already.
#[test]
fn install_crypto_provider_makes_ring_the_process_default() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::install_crypto_provider_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("CODETRIAL_CRYPTO_PROVIDER_TEST_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn install_crypto_provider_child() {
    if std::env::var_os("CODETRIAL_CRYPTO_PROVIDER_TEST_CHILD").is_none() {
        return;
    }
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    super::install_crypto_provider();
    let installed = rustls::crypto::CryptoProvider::get_default()
        .expect("a provider is installed")
        .cipher_suites
        .iter()
        .map(|suite| suite.suite())
        .collect::<Vec<_>>();
    let ring = rustls::crypto::ring::default_provider()
        .cipher_suites
        .iter()
        .map(|suite| suite.suite())
        .collect::<Vec<_>>();
    assert_eq!(installed, ring);
}
