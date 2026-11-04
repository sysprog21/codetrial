use super::*;

#[test]
fn unfinished_rooms_are_reported_together_with_the_shutdowns_own_error() {
    let alone = unfinished(Ok(()));
    assert!(
        alone.starts_with("rooms or reports were still running"),
        "{alone}"
    );
    assert_eq!(
        unfinished(Err("web server failed: slow".into())),
        "web server failed: slow; rooms or reports were still running when the settle bound passed"
    );
}

/// A config directory holding one provider file per id, removed on drop.
struct Providers(std::path::PathBuf);

impl Providers {
    fn new(label: &str, ids: &[&str]) -> Self {
        let dir =
            std::env::temp_dir().join(format!("codetrial-launch-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for id in ids {
            std::fs::write(
                dir.join(format!("codetrial.env.{id}")),
                format!(
                    "LIVEKIT_URL=wss://{id}.example\nLIVEKIT_API_KEY=k-{id}\nLIVEKIT_API_SECRET=s-{id}\n"
                ),
            )
            .unwrap();
        }
        Self(dir)
    }
}

impl Drop for Providers {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_pool_gains_the_discovered_providers_in_the_order_asked_for() {
    let dir = Providers::new("pool", &["eu", "tokyo"]);
    let mut pool = crate::config::ProviderPool::default();
    extend_pool(&mut pool, false, &dir.0, "tokyo");
    let ids: Vec<_> = pool.providers.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["tokyo", "eu"]);
    let report = pool_report(&pool.providers);
    assert_eq!(report.len(), 1, "{report:?}");
    assert!(
        report[0].starts_with("provider pooling: 2 provider(s) over 2 project(s): tokyo="),
        "{report:?}"
    );
    assert!(pool_report(&[]).is_empty());
}

#[test]
fn a_project_listed_twice_is_reported_as_a_warning() {
    let dir = Providers::new("twice", &["eu"]);
    let mut pool = crate::config::ProviderPool::default();
    extend_pool(&mut pool, false, &dir.0, "");
    let twin = pool.providers[0].clone();
    pool.providers.push(crate::config::Provider {
        id: "copy".into(),
        ..twin
    });
    let report = pool_report(&pool.providers);
    assert_eq!(report.len(), 2, "{report:?}");
    assert!(report[1].starts_with("WARNING: eu and copy are the same LiveKit project"));
}

#[test]
fn the_launch_reads_its_cap_length_and_compiler_switch_from_the_values() {
    let values = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    assert_eq!(
        read_max_concurrent(&values(&[("CODETRIAL_MAX_CONCURRENT_INTERVIEWS", "5")])),
        5
    );
    assert_eq!(
        duration_setting(&values(&[("CODETRIAL_DURATION_MIN", "45")]))
            .unwrap()
            .0,
        45
    );
    assert_eq!(
        duration_setting(&values(&[])).unwrap().0,
        crate::config::DEFAULT_DURATION_MIN
    );
    for (value, enabled) in [("true", true), ("false", false)] {
        assert_eq!(
            compiler_explorer_enabled(&values(&[("CODETRIAL_COMPILER_EXPLORER_ENABLED", value)])),
            enabled
        );
    }
}

/// The length a launch runs on and what the operator is told about it come from
/// one function, so the number in a warning is the one the server uses, and the
/// recording check sees the same clamped length the endpoints grant.
#[test]
fn a_duration_setting_is_read_once_and_named_when_not_used_as_written() {
    let values = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    let read = |value: &str| {
        let pairs = values(&[("CODETRIAL_DURATION_MIN", value)]);
        duration_setting(&pairs)
    };
    let agent = |value: &str| {
        crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
            ("CODETRIAL_DURATION_MIN", value),
        ])
    };
    let unset = values(&[]);
    assert_eq!(
        duration_setting(&unset),
        Ok((crate::config::DEFAULT_DURATION_MIN, None))
    );
    for (value, used) in [("", 45), ("  ", 45), ("10", 10), (" 60 ", 60), ("90", 90)] {
        assert_eq!(read(value), Ok((used, None)), "{value:?}");
    }
    for (value, used) in [
        ("-30", 10),
        ("0", 10),
        ("5", 10),
        ("91", 90),
        ("120", 90),
        ("500", 90),
        ("4294967296", 90),
        ("99999999999999999999", 90),
        ("-99999999999999999999", 10),
        ("+99999999999999999999", 90),
    ] {
        let (length, said) = read(value).unwrap();
        assert_eq!(length, used, "{value:?}");
        let said = said.unwrap_or_else(|| panic!("{value:?} was accepted silently"));
        assert!(said.contains(value), "{said}");
        assert!(said.ends_with(&format!("read as {used}")), "{said}");

        // The agent path (`check-gemini`, `run-livekit`) reads the same length.
        assert_eq!(
            agent(value).unwrap().default_duration_min,
            used,
            "{value:?}"
        );
    }

    // Text that is no number is refused on both paths, since reading `3O` as
    // the built-in 45 would quietly run the wrong length on a host nobody
    // watches.
    for value in ["3O", "30min", "45.5", "abc", "-", "+", "1e2"] {
        let error = read(value).expect_err(value);
        assert!(error.contains("CODETRIAL_DURATION_MIN"), "{error}");
        assert!(error.contains(value), "{error}");
        let refused = agent(value)
            .err()
            .unwrap_or_else(|| panic!("{value:?} was accepted by the agent"));
        assert!(
            refused
                .invalid_entries
                .iter()
                .any(|entry| entry.contains("CODETRIAL_DURATION_MIN")),
            "{value:?}: {refused}"
        );
    }
}
