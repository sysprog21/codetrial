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
        interview_duration_min(&values(&[("CODETRIAL_DURATION_MIN", "45")])),
        45
    );
    assert_eq!(interview_duration_min(&values(&[])), DEFAULT_DURATION_MIN);
    for (value, enabled) in [("true", true), ("false", false)] {
        assert_eq!(
            compiler_explorer_enabled(&values(&[("CODETRIAL_COMPILER_EXPLORER_ENABLED", value)])),
            enabled
        );
    }
}
