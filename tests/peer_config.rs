use codetrial::config::{PeerConfig, load_peer};
use std::collections::BTreeMap;

#[test]
fn peer_configuration_has_defaults_and_caps_invite_lifetime() {
    let mut values = BTreeMap::new();
    let config = load_peer(&values).unwrap();
    assert_eq!(config, PeerConfig::default());
    assert_eq!(config.max_sessions, 8);
    assert_eq!(config.round_minutes, 25);
    assert_eq!(config.hard_max_minutes, 90);
    assert_eq!(config.invite_ttl_days, 7);
    assert_eq!(config.reconnect_grace_seconds, 120);
    values.insert("CODETRIAL_PEER_INVITE_TTL_DAYS".into(), "30".into());
    assert_eq!(load_peer(&values).unwrap().invite_ttl_days, 14);
}

#[test]
fn peer_configuration_rejects_invalid_settings() {
    for (key, value) in [
        ("CODETRIAL_PEER_MAX_SESSIONS", "0"),
        ("CODETRIAL_PEER_ROUND_MINUTES", "abc"),
        ("CODETRIAL_PEER_HARD_MAX_MINUTES", "0"),
        ("CODETRIAL_PEER_INVITE_TTL_DAYS", "0"),
        ("CODETRIAL_PEER_RECONNECT_GRACE_SECONDS", "-1"),
    ] {
        let values = BTreeMap::from([(key.into(), value.into())]);
        assert!(load_peer(&values).unwrap_err().to_string().contains(key));
    }
}

#[test]
fn peer_configuration_reads_overrides() {
    let values = BTreeMap::from([
        ("CODETRIAL_PEER_MAX_SESSIONS".into(), "12".into()),
        ("CODETRIAL_PEER_ROUND_MINUTES".into(), "30".into()),
        ("CODETRIAL_PEER_HARD_MAX_MINUTES".into(), "100".into()),
        ("CODETRIAL_PEER_INVITE_TTL_DAYS".into(), "2".into()),
        ("CODETRIAL_PEER_RECONNECT_GRACE_SECONDS".into(), "60".into()),
    ]);
    assert_eq!(
        load_peer(&values).unwrap(),
        PeerConfig {
            max_sessions: 12,
            round_minutes: 30,
            hard_max_minutes: 100,
            invite_ttl_days: 2,
            reconnect_grace_seconds: 60,
        }
    );
}

#[test]
fn peer_default_round_must_fit_inside_the_hard_session_limit() {
    let values = BTreeMap::from([("CODETRIAL_PEER_HARD_MAX_MINUTES".into(), "20".into())]);
    let message = load_peer(&values).unwrap_err().to_string();
    assert!(message.contains("CODETRIAL_PEER_ROUND_MINUTES"));
    assert!(message.contains("CODETRIAL_PEER_HARD_MAX_MINUTES"));
}

#[test]
fn peer_hard_limit_must_cover_both_rounds() {
    let limit = |minutes: &str| {
        load_peer(&BTreeMap::from([
            ("CODETRIAL_PEER_ROUND_MINUTES".into(), "25".into()),
            ("CODETRIAL_PEER_HARD_MAX_MINUTES".into(), minutes.into()),
        ]))
    };
    assert!(limit("25").is_err());
    assert!(limit("49").is_err());
    assert!(limit("50").is_ok());
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
        std::os::unix::fs::symlink(&outside, web.join("exposed")).unwrap();
        assert!(!codetrial::config::peer_audio_storage_is_private(
            &outside, &web
        ));
        std::fs::remove_file(web.join("exposed")).unwrap();
        assert!(codetrial::config::peer_audio_storage_is_private(
            &outside, &web
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

#[test]
fn observer_configurations_are_equal_only_when_every_part_matches() {
    let values = BTreeMap::from([("GOOGLE_API_KEY".into(), "observer-key".into())]);
    let config = codetrial::config::load_peer_observer(&values)
        .unwrap()
        .unwrap();
    let mut other_model = config.clone();
    other_model.model = "another-model".into();
    assert_ne!(config, other_model);
    let mut other_audio = config.clone();
    other_audio.audio_dir = Some("/somewhere/private".into());
    assert_ne!(config, other_audio);
    let separate = codetrial::config::load_peer_observer(&values)
        .unwrap()
        .unwrap();
    assert_ne!(
        config, separate,
        "separately loaded keys rotate independently"
    );
}

#[test]
fn observer_debug_output_names_the_model_and_hides_the_keys() {
    let values = BTreeMap::from([("GOOGLE_API_KEY".into(), "observer-key".into())]);
    let config = codetrial::config::load_peer_observer(&values)
        .unwrap()
        .unwrap();
    let debug = format!("{config:?}");
    assert!(debug.contains("PeerObserverConfig"), "{debug}");
    assert!(debug.contains(&config.model), "{debug}");
    assert!(debug.contains("[redacted]"), "{debug}");
    assert!(debug.contains("audio_available: false"), "{debug}");
    assert!(!debug.contains("observer-key"), "{debug}");
}

#[cfg(unix)]
#[test]
fn a_link_to_a_parent_or_a_child_of_the_audio_directory_exposes_it() {
    let base = std::env::temp_dir().join(format!(
        "codetrial-audio-links-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let web = base.join("web");
    let parent = base.join("private");
    let audio = parent.join("audio");
    let child = audio.join("session");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::create_dir_all(&child).unwrap();
    assert!(codetrial::config::peer_audio_storage_is_private(
        &audio, &web
    ));
    for target in [&parent, &child] {
        let link = web.join("link");
        std::os::unix::fs::symlink(target, &link).unwrap();
        assert!(
            !codetrial::config::peer_audio_storage_is_private(&audio, &web),
            "{}",
            target.display()
        );
        std::fs::remove_file(&link).unwrap();
    }
    std::fs::remove_dir_all(&base).unwrap();
}
