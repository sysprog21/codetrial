// Scoped routing and observation for tests that exercise the real report path.

#[derive(Default)]
struct ScopedReportState {
    endpoint: String,
    entries: Mutex<Vec<(i64, std::time::Instant)>>,
}

static SCOPED_REPORTS: Mutex<Vec<(String, Arc<ScopedReportState>)>> = Mutex::new(Vec::new());

pub(crate) struct ScopedReport {
    scope: String,
    state: Arc<ScopedReportState>,
}

impl ScopedReport {
    pub(crate) fn register(scope: &str, endpoint: &str) -> Result<Self, String> {
        let url = reqwest::Url::parse(endpoint).map_err(|error| error.to_string())?;
        let local = url
            .host_str()
            .and_then(|host| {
                host.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .ok()
            })
            .is_some_and(|address| address.is_loopback());
        if scope.is_empty()
            || url.scheme() != "http"
            || !local
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("report fixture requires a named scope and loopback HTTP endpoint".into());
        }
        let mut scopes = SCOPED_REPORTS.lock().unwrap();
        if scopes.iter().any(|(name, _)| name == scope) {
            return Err("report fixture scope is already registered".into());
        }
        let state = Arc::new(ScopedReportState {
            endpoint: endpoint.into(),
            ..Default::default()
        });
        scopes.push((scope.into(), Arc::clone(&state)));
        Ok(Self {
            scope: scope.into(),
            state,
        })
    }

    pub(crate) fn entries(&self) -> Vec<(i64, std::time::Instant)> {
        self.state.entries.lock().unwrap().clone()
    }
}

impl Drop for ScopedReport {
    fn drop(&mut self) {
        SCOPED_REPORTS
            .lock()
            .unwrap()
            .retain(|(name, state)| name != &self.scope || !Arc::ptr_eq(state, &self.state));
    }
}

pub(crate) fn scoped_report_endpoint(scope: &str) -> Option<String> {
    SCOPED_REPORTS
        .lock()
        .unwrap()
        .iter()
        .find(|(name, _)| name == scope)
        .map(|(_, state)| state.endpoint.clone())
}

pub(crate) fn note_bounded_report(scope: &str, seed: i64) {
    if let Some((_, state)) = SCOPED_REPORTS
        .lock()
        .unwrap()
        .iter()
        .find(|(name, _)| name == scope)
    {
        state
            .entries
            .lock()
            .unwrap()
            .push((seed, std::time::Instant::now()));
    }
}

#[test]
fn scoped_report_routes_are_local_unique_and_removed_on_drop() {
    assert!(ScopedReport::register("", "http://127.0.0.1:1/").is_err());
    for endpoint in [
        "https://127.0.0.1/",
        "http://example.com/",
        "http://user@127.0.0.1/",
    ] {
        assert!(ScopedReport::register("invalid-scope", endpoint).is_err());
    }
    let first = ScopedReport::register("report-scope-first", "http://127.0.0.1:1234/").unwrap();
    let other = ScopedReport::register("report-scope-other", "http://127.0.0.1:4321/").unwrap();
    assert!(ScopedReport::register("report-scope-first", "http://127.0.0.1:1234/").is_err());
    assert_eq!(
        scoped_report_endpoint("report-scope-first").as_deref(),
        Some("http://127.0.0.1:1234/")
    );
    assert!(scoped_report_endpoint("unknown-report-scope").is_none());
    note_bounded_report("report-scope-first", 17);
    assert_eq!(first.entries().len(), 1);
    assert!(other.entries().is_empty());
    drop(first);
    assert!(scoped_report_endpoint("report-scope-first").is_none());
    assert!(scoped_report_endpoint("report-scope-other").is_some());
}
