//! Values the config reader must refuse or read as written, not guess at.

use codetrial::config::{load_from_pairs, read_config_file};

fn base() -> [(&'static str, &'static str); 4] {
    [
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
    ]
}

/// `0` is how an operator keeps a shared key's quota for the final report, so
/// a typo for it must not become the default of six interim reviews.
#[test]
fn a_malformed_interim_review_limit_is_refused() {
    for typo in ["off", "none", "-1", "2.5"] {
        let Err(error) = load_from_pairs(
            base()
                .into_iter()
                .chain([("CODETRIAL_MAX_INTERIM_REVIEWS", typo)]),
        ) else {
            panic!("{typo:?} must not be read as the default");
        };
        assert_eq!(
            error.invalid_entries,
            ["CODETRIAL_MAX_INTERIM_REVIEWS must be a whole number".to_string()],
            "{typo:?}"
        );
    }
    assert!(
        load_from_pairs(
            base()
                .into_iter()
                .chain([("CODETRIAL_MAX_INTERIM_REVIEWS", " 3 ")])
        )
        .is_ok()
    );
}

/// A file copied from shell syntax. Left as `export NODE_ENV`, the key matches
/// nothing and the server starts on the local defaults without saying so.
#[test]
fn an_export_prefix_does_not_hide_a_setting() {
    let path = std::env::temp_dir().join(format!("codetrial-export-{}.env", std::process::id()));
    std::fs::write(
        &path,
        "export NODE_ENV=production\nexport   SESSION_SECRET=s3cret\nPLAIN=1\n",
    )
    .unwrap();
    let read = read_config_file(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        read.unwrap(),
        [
            ("NODE_ENV".to_string(), "production".to_string()),
            ("SESSION_SECRET".to_string(), "s3cret".to_string()),
            ("PLAIN".to_string(), "1".to_string()),
        ]
    );
}
