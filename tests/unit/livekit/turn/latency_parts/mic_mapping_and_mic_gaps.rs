// Part of latency.rs: included in place, so every item here shares that file's
// module.

// Microphone dequeue gaps. Declared before running: a gap is the span between
// two frames taken from one stream; it is counted over a threshold only when
// strictly longer; nothing is measured from a first frame, across a break or
// before two frames exist; and the figures are `none`, never 0, until a gap has
// been measured.

fn mic_fields(line: &str) -> std::collections::HashMap<String, String> {
    line.split_whitespace()
        .filter(|token| token.starts_with("mic_gap_"))
        .filter_map(|token| token.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn mic_state(trace: &ReplyLatency) -> [String; 4] {
    let fields = mic_fields(&trace.mic_gaps().fields());
    [
        fields["mic_gap_max_ms"].clone(),
        fields["mic_gap_over_1s"].clone(),
        fields["mic_gap_over_10s"].clone(),
        fields["mic_gap_over_30s"].clone(),
    ]
}

/// Instants on the paused tokio clock, advanced by hand.
fn paused_now() -> Instant {
    tokio::time::Instant::now().into_std()
}

#[tokio::test(start_paused = true)]
async fn mic_gaps_are_unknown_until_two_frames_have_been_taken() {
    let mut trace = ReplyLatency::default();
    assert_eq!(mic_state(&trace), ["none"; 4]);
    trace.mic_dequeued(paused_now());
    assert_eq!(mic_state(&trace), ["none"; 4], "one frame is no gap");
    tokio::time::advance(Duration::from_millis(10)).await;
    trace.mic_dequeued(paused_now());
    assert_eq!(mic_state(&trace), ["10", "0", "0", "0"]);
}

#[tokio::test(start_paused = true)]
async fn mic_gap_thresholds_are_strictly_greater_than_and_nested() {
    let mut trace = ReplyLatency::default();
    trace.mic_dequeued(paused_now());
    // Exactly at each threshold: counted by none of them.
    for (wait_ms, want) in [
        (1_000u64, ["1000", "0", "0", "0"]),
        (1_001, ["1001", "1", "0", "0"]),
        (10_000, ["10000", "2", "0", "0"]),
        (10_001, ["10001", "3", "1", "0"]),
        (30_000, ["30000", "4", "2", "0"]),
        (30_001, ["30001", "5", "3", "1"]),
        // A shorter gap leaves the maximum alone.
        (20, ["30001", "5", "3", "1"]),
    ] {
        tokio::time::advance(Duration::from_millis(wait_ms)).await;
        trace.mic_dequeued(paused_now());
        assert_eq!(mic_state(&trace), want, "after a {wait_ms} ms gap");
    }
}

#[tokio::test(start_paused = true)]
async fn a_break_in_the_stream_is_not_a_gap_and_forgets_the_last_frame() {
    let mut trace = ReplyLatency::default();
    trace.mic_dequeued(paused_now());
    tokio::time::advance(Duration::from_millis(50)).await;
    trace.mic_dequeued(paused_now());
    // The track went away for two minutes (candidate left or muted-and-gone).
    trace.mic_stream_broken();
    tokio::time::advance(Duration::from_secs(120)).await;
    trace.mic_dequeued(paused_now());
    assert_eq!(mic_state(&trace), ["50", "0", "0", "0"]);
    tokio::time::advance(Duration::from_millis(5)).await;
    trace.mic_dequeued(paused_now());
    assert_eq!(mic_state(&trace), ["50", "0", "0", "0"]);
}

#[tokio::test(start_paused = true)]
async fn mic_gaps_belong_to_the_interview_and_ride_the_clock_lines_and_the_summary() {
    let mut trace = ReplyLatency::default();
    trace.bind("room", 4, 1, 1500);
    let start = paused_now();
    let anchor = trace.anchor(start);
    assert_eq!(
        mic_fields(&anchor)["mic_gap_max_ms"],
        "none",
        "nothing measured at the start"
    );
    trace.mic_dequeued(paused_now());
    tokio::time::advance(Duration::from_millis(2_500)).await;
    trace.mic_dequeued(paused_now());
    // Socket replacements restart episodes and sockets, never the figures.
    trace.begin_episode();
    trace.installed_replacement();
    trace.set_socket(2);
    tokio::time::advance(Duration::from_secs(60)).await;
    let line = trace.clock_sample(paused_now()).unwrap();
    assert!(line.starts_with("interview clock: "), "{line}");
    assert!(line.ends_with(" room=room"), "{line}");
    let fields = mic_fields(&line);
    assert_eq!(fields["mic_gap_max_ms"], "2500");
    assert_eq!(fields["mic_gap_over_1s"], "1");
    assert_eq!(fields["mic_gap_over_10s"], "0");
    let summary = trace.mic_summary(paused_now()).unwrap();
    assert!(summary.starts_with("mic gaps: pid="), "{summary}");
    assert_eq!(mic_fields(&summary), fields);
    assert!(summary.contains(" elapsed_ms=62500 "), "{summary}");
    // An unanchored interview has no clock to print a summary against.
    assert!(ReplyLatency::default().mic_summary(start).is_none());
}
