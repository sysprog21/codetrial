use codetrial::config::{PeerConfig, load_peer};
use std::collections::BTreeMap;

#[test]
fn peer_configuration_is_opt_in_and_caps_invite_lifetime() {
    let mut values = BTreeMap::new();
    assert_eq!(load_peer(&values).unwrap(), None);
    values.insert("CODETRIAL_PEER_ENABLED".into(), "true".into());
    assert_eq!(load_peer(&values).unwrap(), Some(PeerConfig::default()));
    let config = load_peer(&values).unwrap().unwrap();
    assert_eq!(config.max_sessions, 8);
    assert_eq!(config.round_minutes, 25);
    assert_eq!(config.hard_max_minutes, 90);
    assert_eq!(config.invite_ttl_days, 7);
    assert_eq!(config.reconnect_grace_seconds, 120);
    values.insert("CODETRIAL_PEER_INVITE_TTL_DAYS".into(), "30".into());
    assert_eq!(load_peer(&values).unwrap().unwrap().invite_ttl_days, 14);
}

#[test]
fn peer_configuration_rejects_invalid_enabled_settings() {
    for (key, value) in [
        ("CODETRIAL_PEER_ENABLED", "ture"),
        ("CODETRIAL_PEER_MAX_SESSIONS", "0"),
        ("CODETRIAL_PEER_ROUND_MINUTES", "abc"),
        ("CODETRIAL_PEER_HARD_MAX_MINUTES", "0"),
        ("CODETRIAL_PEER_INVITE_TTL_DAYS", "0"),
        ("CODETRIAL_PEER_RECONNECT_GRACE_SECONDS", "-1"),
    ] {
        let values = BTreeMap::from([
            ("CODETRIAL_PEER_ENABLED".into(), "true".into()),
            (key.into(), value.into()),
        ]);
        assert!(load_peer(&values).unwrap_err().to_string().contains(key));
    }
}

#[test]
fn disabled_peer_ignores_unused_limits_and_enabled_peer_reads_overrides() {
    let mut values = BTreeMap::from([("CODETRIAL_PEER_MAX_SESSIONS".into(), "invalid".into())]);
    assert_eq!(load_peer(&values).unwrap(), None);
    values.extend([
        ("CODETRIAL_PEER_ENABLED".into(), "true".into()),
        ("CODETRIAL_PEER_MAX_SESSIONS".into(), "12".into()),
        ("CODETRIAL_PEER_ROUND_MINUTES".into(), "30".into()),
        ("CODETRIAL_PEER_HARD_MAX_MINUTES".into(), "100".into()),
        ("CODETRIAL_PEER_INVITE_TTL_DAYS".into(), "2".into()),
        ("CODETRIAL_PEER_RECONNECT_GRACE_SECONDS".into(), "60".into()),
    ]);
    assert_eq!(
        load_peer(&values).unwrap(),
        Some(PeerConfig {
            max_sessions: 12,
            round_minutes: 30,
            hard_max_minutes: 100,
            invite_ttl_days: 2,
            reconnect_grace_seconds: 60,
        })
    );
}

#[test]
fn peer_default_round_must_fit_inside_the_hard_session_limit() {
    let values = BTreeMap::from([
        ("CODETRIAL_PEER_ENABLED".into(), "true".into()),
        ("CODETRIAL_PEER_HARD_MAX_MINUTES".into(), "20".into()),
    ]);
    let message = load_peer(&values).unwrap_err().to_string();
    assert!(message.contains("CODETRIAL_PEER_ROUND_MINUTES"));
    assert!(message.contains("CODETRIAL_PEER_HARD_MAX_MINUTES"));
}

#[test]
fn silent_observer_configuration_is_optional_and_never_formats_credentials() {
    let values = BTreeMap::from([(
        "GOOGLE_API_KEYS".into(),
        "first-observer-key,second-observer-key".into(),
    )]);
    let config = codetrial::config::load_peer_observer(&values)
        .unwrap()
        .unwrap();
    assert!(config.audio_dir.is_none());
    assert_eq!(config, config.clone());
    let debug = format!("{config:?}");
    assert!(!debug.contains("first-observer-key"));
    assert!(!debug.contains("second-observer-key"));
    assert!(
        codetrial::config::load_peer_observer(&BTreeMap::new())
            .unwrap()
            .is_none()
    );
}

#[test]
fn audio_storage_cannot_be_served_under_the_static_web_root() {
    let base = std::env::temp_dir().join(format!(
        "codetrial-audio-separation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let web = base.join("web");
    let public_audio = web.join("private-audio");
    let outside = base.join("web-private");
    std::fs::create_dir_all(&public_audio).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    assert!(!codetrial::config::peer_audio_storage_is_private(
        &public_audio,
        &web
    ));
    assert!(codetrial::config::peer_audio_storage_is_private(
        &outside, &web
    ));
    assert!(!codetrial::config::peer_audio_storage_is_private(
        &web, &web
    ));
    assert!(!codetrial::config::peer_audio_storage_is_private(
        &base.join("missing-audio"),
        &web
    ));
    assert!(codetrial::config::peer_audio_storage_is_private(
        &outside,
        &base.join("missing-web")
    ));
    #[cfg(unix)]
    {
        let link = base.join("audio-link");
        std::os::unix::fs::symlink(&public_audio, &link).unwrap();
        assert!(!codetrial::config::peer_audio_storage_is_private(
            &link, &web
        ));
        let web_link = base.join("web-link");
        std::os::unix::fs::symlink(&web, &web_link).unwrap();
        assert!(!codetrial::config::peer_audio_storage_is_private(
            &public_audio,
            &web_link
        ));
    }
    std::fs::remove_dir_all(base).unwrap();
}
