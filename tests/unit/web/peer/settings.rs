use super::*;
use serde_json::json;

#[test]
fn absent_settings_use_defaults_and_explicit_settings_keep_their_values() {
    let config = PeerConfig {
        round_minutes: 30,
        ..PeerConfig::default()
    };
    let defaults = parse(&json!({}), &config).unwrap();
    assert_eq!(defaults.topic, "");
    assert_eq!(defaults.difficulty, "easy");
    assert_eq!(defaults.language, "python");
    assert_eq!(defaults.round_minutes, 30);
    assert_eq!(defaults.interviewer_first, 0);
    let explicit = parse(&json!({"topic":"arrays","difficulty":"medium","language":"javascript","roundMinutes":20,"interviewerFirst":1}), &config).unwrap();
    assert_eq!(explicit.topic, "arrays");
    assert_eq!(explicit.difficulty, "medium");
    assert_eq!(explicit.language, "javascript");
    assert_eq!(explicit.round_minutes, 20);
    assert_eq!(explicit.interviewer_first, 1);
}

#[test]
fn malformed_supplied_settings_are_refused_instead_of_defaulted() {
    let config = PeerConfig::default();
    for body in [json!(null), json!([]), json!("settings"), json!(25)] {
        assert_eq!(parse(&body, &config).unwrap_err(), "invalid_settings");
    }
    for key in ["topic", "difficulty", "language"] {
        for value in [json!(null), json!(123), json!(true), json!([]), json!({})] {
            let mut body = json!({});
            body[key] = value;
            assert_eq!(
                parse(&body, &config).unwrap_err(),
                "invalid_settings",
                "{body}"
            );
        }
    }
    for key in ["roundMinutes", "interviewerFirst"] {
        for value in [
            json!(null),
            json!("25"),
            json!(true),
            json!(-1),
            json!(1.5),
            json!([]),
            json!({}),
        ] {
            let mut body = json!({});
            body[key] = value;
            assert_eq!(
                parse(&body, &config).unwrap_err(),
                "invalid_settings",
                "{body}"
            );
        }
    }
    assert!(parse(&json!({"roundMinutes":u64::MAX}), &config).is_err());
    assert!(parse(&json!({"interviewerFirst":u64::MAX}), &config).is_err());
}
