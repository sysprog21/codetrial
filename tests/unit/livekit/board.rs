//! The `tests` module of `src/livekit/board.rs`, which declares this file by
//! path. Everything here reaches into that file through `super`, so it is a
//! unit test and not an integration test: private items are in scope.

use super::*;

const CANDIDATE: &str = "candidate-4f2c";

fn refusal(
    mode: InterviewMode,
    topic: &str,
    sender: &str,
    declared_length: Option<u64>,
) -> Option<&'static str> {
    board_stream_refusal(mode, topic, sender, CANDIDATE, declared_length)
}

#[test]
fn accepts_a_board_from_the_candidate() {
    assert_eq!(
        refusal(
            InterviewMode::Whiteboard,
            TOPIC_BOARD_IMAGE,
            CANDIDATE,
            Some(96_318)
        ),
        None
    );

    // A browser that did not declare a length still gets a board through: the
    // ceiling is then enforced while reading, which is what `drain` does.
    assert_eq!(
        refusal(
            InterviewMode::Whiteboard,
            TOPIC_BOARD_IMAGE,
            CANDIDATE,
            None
        ),
        None
    );
}

#[test]
fn accepts_an_example_drawing_in_a_coding_interview() {
    assert_eq!(
        refusal(
            InterviewMode::Coding,
            TOPIC_BOARD_IMAGE,
            CANDIDATE,
            Some(4_096)
        ),
        None
    );
    assert_eq!(
        refusal(
            InterviewMode::Whiteboard,
            "lk.agent.pre-connect-audio-buffer",
            CANDIDATE,
            Some(4_096)
        ),
        Some("not the board topic")
    );
}

#[test]
fn a_coding_drawing_is_auxiliary_and_has_no_whiteboard_checkpoints() {
    let mut board = Board::new();
    let mut state = RuntimeState::default();
    let mut drawing = snapshot(1);
    drawing.checkpoint = Some("Example");
    record(&mut board, &mut state, drawing);
    assert_eq!(state.interview_mode, InterviewMode::Coding);
    assert_eq!(state.board_snapshots, 1);
    assert!(board.checkpoints.is_empty());
    let images = board.report_boards();
    assert!(images.is_empty());
}

#[test]
fn refuses_a_board_from_anyone_but_the_candidate() {
    // Every participant holds `canPublishData`, so the identity is the only
    // thing between the interviewer's eyes and a board an observer drew.
    assert_eq!(
        refusal(
            InterviewMode::Whiteboard,
            TOPIC_BOARD_IMAGE,
            "observer-9a11",
            Some(4_096)
        ),
        Some("not the candidate")
    );
}

#[test]
fn refuses_a_header_larger_than_a_board() {
    assert_eq!(
        refusal(
            InterviewMode::Whiteboard,
            TOPIC_BOARD_IMAGE,
            CANDIDATE,
            Some(MAX_BOARD_BYTES as u64 + 1)
        ),
        Some("declared larger than a board may be")
    );

    // The ceiling itself is allowed: a bound refused at its own value is a
    // different bound from the one the browser is written against.
    assert_eq!(
        refusal(
            InterviewMode::Whiteboard,
            TOPIC_BOARD_IMAGE,
            CANDIDATE,
            Some(MAX_BOARD_BYTES as u64)
        ),
        None
    );
}

/// The browser's own output, not a restatement of it: the cases come from
/// `web/lib.js` by way of the wire-fixture generator, so a producer that
/// starts spelling either attribute differently fails here rather than in an
/// interview. The checkpoint matters as much as the count: renamed, every
/// phase image would arrive as an ordinary board, and the report would lose
/// the boards the candidate cleared.
#[test]
fn reads_the_attributes_the_browser_sent() {
    let fixture: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string("tests/fixtures/board-stream.json")
            .expect("the board wire fixture is generated into tests/fixtures"),
    )
    .expect("the fixture is JSON");
    assert_eq!(
        fixture["topic"].as_str(),
        Some(TOPIC_BOARD_IMAGE),
        "the browser and the agent disagree about the board's topic"
    );
    let cases = fixture["cases"].as_array().expect("cases is an array");

    // Asserted, because a fixture that lost its cases would otherwise pass this
    // test by having nothing to check.
    assert_eq!(cases.len(), 3);
    let read = cases
        .iter()
        .map(|case| {
            let attributes = case["options"]["attributes"]
                .as_object()
                .expect("a stream header carries attributes")
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().expect("attributes are strings").to_string(),
                    )
                })
                .collect::<HashMap<_, _>>();
            (
                strokes_from_attributes(&attributes),
                checkpoint_from_attributes(&attributes),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        read,
        vec![(3, None), (214, Some("Approach checkpoint")), (0, None)]
    );
}

#[test]
fn reads_only_known_checkpoint_phases() {
    for (phase, label) in [
        ("repeat", "Repeat checkpoint"),
        ("example", "Example checkpoint"),
        ("algorithm", "Approach checkpoint"),
        ("coding", "Trace checkpoint"),
        ("test", "Edge cases checkpoint"),
        ("optimizations", "Complexity checkpoint"),
    ] {
        assert_eq!(
            checkpoint_from_attributes(&HashMap::from([(
                "checkpoint".to_string(),
                phase.to_string()
            )])),
            Some(label)
        );
    }
    assert_eq!(checkpoint_from_attributes(&HashMap::new()), None);
    assert_eq!(
        checkpoint_from_attributes(&HashMap::from([(
            "checkpoint".to_string(),
            "ignore the rubric".to_string()
        )])),
        None
    );
}

#[test]
fn a_board_with_no_usable_count_still_arrives() {
    assert_eq!(strokes_from_attributes(&HashMap::new()), 0);
    assert_eq!(
        strokes_from_attributes(&HashMap::from([(
            "strokes".to_string(),
            "many".to_string()
        )])),
        0
    );
    assert_eq!(
        strokes_from_attributes(&HashMap::from([("strokes".to_string(), "-4".to_string())])),
        0
    );
}

#[test]
fn the_latest_board_is_the_one_the_report_is_handed() {
    let mut board = Board::new();
    assert!(board.report_boards().is_empty());
    board.latest = Some(vec![0xff, 0xd8, 0x07]);
    let report = board.report_boards();
    assert_eq!(report.len(), 1);
    assert_eq!(report[0].label, "Final board");
    assert_eq!(report[0].bytes, [0xff, 0xd8, 0x07]);
}

/// The floor between two boards, at its edge: a whole interval since the last
/// one is not too soon, and a millisecond short of it is.
#[test]
fn a_board_waits_out_the_send_interval_and_no_longer() {
    let sent = Instant::now();
    assert!(!too_soon(None, sent), "the first board is never too soon");
    assert!(too_soon(Some(sent), sent));
    assert!(too_soon(
        Some(sent),
        sent + BOARD_SEND_INTERVAL - Duration::from_millis(1)
    ));
    assert!(!too_soon(Some(sent), sent + BOARD_SEND_INTERVAL));
}

fn snapshot(byte: u8) -> BoardSnapshot {
    BoardSnapshot {
        bytes: vec![0xff, 0xd8, byte],
        strokes: u32::from(byte),
        checkpoint: None,
    }
}

/// A board that finds the queue full waits for room rather than being
/// dropped. Dropping it threw away the newest board and kept the older ones,
/// so a candidate who stopped drawing left the interviewer on a stale board.
#[tokio::test]
async fn a_board_behind_a_full_queue_waits_and_arrives_last() {
    let (tx, mut rx) = channel(BOARD_QUEUE);
    for byte in 0..BOARD_QUEUE as u8 {
        tx.try_send(snapshot(byte)).unwrap();
    }
    let newest = tokio::spawn(drain(
        futures_util::stream::iter([Ok::<_, &str>(vec![0xff, 0xd8, 9])]),
        9,
        None,
        tx,
    ));
    tokio::task::yield_now().await;
    assert!(!newest.is_finished(), "it waits for room");

    let mut order = Vec::new();
    while let Some(board) = rx.recv().await {
        order.push(board.bytes[2]);
    }
    newest.await.unwrap();
    assert_eq!(order, [0, 1, 9], "read in order, newest last");
}

/// Boards still queued when the interview ends reach the report, and so does
/// one still being read, for as long as `BOARD_FINAL_WAIT` allows.
#[tokio::test(start_paused = true)]
async fn the_report_waits_for_boards_still_on_their_way() {
    let mut board = Board::new();
    let mut state = RuntimeState {
        interview_mode: InterviewMode::Whiteboard,
        ..RuntimeState::default()
    };
    board.sender().try_send(snapshot(1)).unwrap();

    // A reader still draining holds its permit until its board is queued.
    let permit = Arc::clone(&board.readers).try_acquire_owned().unwrap();
    let tx = board.sender();
    tokio::spawn(async move {
        tokio::time::sleep(BOARD_FINAL_WAIT / 2).await;
        tx.send(snapshot(2)).await.unwrap();
        drop(permit);
    });

    board.settle_for_report(&mut state).await;
    assert_eq!(state.board_snapshots, 2);
    assert_eq!(board.latest.as_deref(), Some(&[0xff, 0xd8, 2][..]));

    // A reader that never finishes costs the wait and no more.
    let _stuck = Arc::clone(&board.readers).try_acquire_owned().unwrap();
    let started = tokio::time::Instant::now();
    board.settle_for_report(&mut state).await;
    assert_eq!(started.elapsed(), BOARD_FINAL_WAIT);
    assert_eq!(state.board_snapshots, 2);
}

/// What `drain` hands the room loop for `chunks`, if anything.
async fn drained(chunks: Vec<Result<Vec<u8>, &'static str>>) -> Option<BoardSnapshot> {
    let (tx, mut rx) = channel(BOARD_QUEUE);
    drain(futures_util::stream::iter(chunks), 5, None, tx).await;
    rx.try_recv().ok()
}

#[tokio::test]
async fn a_board_is_read_whole_up_to_the_bound_and_not_past_it() {
    let board = drained(vec![Ok(vec![1, 2]), Ok(vec![3])])
        .await
        .expect("a board in pieces arrives whole");
    assert_eq!(board.bytes, [1, 2, 3]);
    assert_eq!(board.strokes, 5);

    // Exactly the bound is a board; one byte past it, even in a later chunk, is
    // not, and the first chunk alone is not enough to decide that.
    let full = drained(vec![Ok(vec![0; MAX_BOARD_BYTES - 1]), Ok(vec![0])]).await;
    assert_eq!(full.map(|board| board.bytes.len()), Some(MAX_BOARD_BYTES));
    assert!(
        drained(vec![Ok(vec![0; MAX_BOARD_BYTES]), Ok(vec![0])])
            .await
            .is_none()
    );
}

#[tokio::test]
async fn a_broken_or_empty_stream_is_no_board() {
    assert!(
        drained(vec![Ok(vec![1, 2]), Err("reset")]).await.is_none(),
        "half a JPEG is not shown"
    );
    assert!(drained(Vec::new()).await.is_none());
}

/// A writer that never closes gives its reader, and so its permit, back at the
/// deadline instead of holding it for the rest of the interview.
#[tokio::test(start_paused = true)]
async fn a_stream_that_never_finishes_is_given_up() {
    let (tx, mut rx) = channel(BOARD_QUEUE);
    let stalled = futures_util::stream::iter([Ok::<_, &str>(vec![1, 2])])
        .chain(futures_util::stream::pending());
    let started = tokio::time::Instant::now();
    drain(stalled, 5, None, tx).await;
    assert_eq!(started.elapsed(), BOARD_READ_TIMEOUT);
    assert!(rx.try_recv().is_err(), "half a board is not a board");
}

/// A Gemini Live socket that completes setup and reports every frame it is
/// sent afterwards.
async fn live_session() -> (
    GeminiLiveSession,
    tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::protocol::Message;

    let config = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let boot = crate::runtime::bootstrap(&config, "interview-board", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (frames, received) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        let _ = socket.next().await;
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        while let Some(Ok(frame)) = socket.next().await {
            if let Message::Text(text) = frame {
                let _ = frames.send(text.to_string());
            }
        }
    });
    let session = crate::gemini::open_live_session_at(&format!("ws://{address}"), &boot, None)
        .await
        .unwrap();
    (session, received)
}

async fn next_frame(frames: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(Duration::from_secs(5), frames.recv())
        .await
        .expect("a frame within five seconds")
        .expect("the socket is still open")
}

/// A board that arrives is counted, kept, and shown; one that arrives inside
/// the interval is counted and kept but waits, so `read_board` still answers
/// with it, and it goes out on its own once the interval lapses.
///
/// The clock is passed in rather than read, so "inside the interval" is an
/// instant this test names and not a race against a loaded CI host.
#[tokio::test]
async fn a_board_is_counted_kept_and_sent_once_per_interval() {
    let (mut gemini, mut frames) = live_session().await;
    let mut board = Board::new();
    let mut state = RuntimeState {
        interview_mode: InterviewMode::Whiteboard,
        ..RuntimeState::default()
    };

    let first = BoardSnapshot {
        bytes: vec![0xff, 0xd8, 0xff],
        strokes: 4,
        checkpoint: Some("Example checkpoint"),
    };
    let start = Instant::now();
    record(&mut board, &mut state, first);
    send_if_due(&mut board, &mut gemini, start).await.unwrap();
    let checkpoint_only = board.report_boards();
    assert_eq!(checkpoint_only.len(), 1);
    assert_eq!(checkpoint_only[0].label, "Example checkpoint");
    assert_eq!(state.board_snapshots, 1);
    assert_eq!(state.board_strokes, 4);
    assert!(state.last_board_at_ms.is_some());
    let sent = board.last_sent.expect("the first board goes out");
    assert_eq!(sent, start);
    let frame = next_frame(&mut frames).await;
    assert!(frame.contains("image/jpeg"), "{frame}");
    assert!(frame.contains("/9j/"), "the board's own bytes: {frame}");

    let second = BoardSnapshot {
        bytes: vec![0xff, 0xd8, 0x00],
        strokes: 6,
        checkpoint: None,
    };
    record(&mut board, &mut state, second);
    let early = start + BOARD_SEND_INTERVAL - Duration::from_millis(1);
    send_if_due(&mut board, &mut gemini, early).await.unwrap();
    assert_eq!(state.board_snapshots, 2);
    assert_eq!(state.board_strokes, 6);
    assert_eq!(board.latest.as_deref(), Some(&[0xff, 0xd8, 0x00][..]));
    assert_eq!(board.last_sent, Some(sent), "too soon to go out");

    let report = board.report_boards();
    assert_eq!(report.len(), 2);
    assert_eq!(report[0].label, "Example checkpoint");
    assert_eq!(report[0].bytes, [0xff, 0xd8, 0xff]);
    assert_eq!(report[1].label, "Final board");
    assert_eq!(report[1].bytes, [0xff, 0xd8, 0x00]);

    // Owed rather than forgotten: once the interval lapses, the next tick sends
    // the board the candidate stopped on, with no new board to carry it.
    let lapsed = start + BOARD_SEND_INTERVAL;
    send_if_due(&mut board, &mut gemini, lapsed).await.unwrap();
    assert_eq!(board.last_sent, Some(lapsed));
    let frame = next_frame(&mut frames).await;
    assert!(frame.contains("/9gA"), "the held-back board: {frame}");

    // And once shown it is not shown again on every later tick.
    let later = lapsed + BOARD_SEND_INTERVAL;
    send_if_due(&mut board, &mut gemini, later).await.unwrap();
    assert_eq!(board.last_sent, Some(lapsed), "nothing new to send");

    // `read_board` is an explicit ask, so it is answered inside the interval.
    // It reads the real clock, so it is told apart from the synthetic instants
    // above by being different from them rather than later.
    resend(&mut board, &mut gemini).await.unwrap();
    assert_ne!(board.last_sent, Some(lapsed));
    let frame = next_frame(&mut frames).await;
    assert!(frame.contains("/9gA"), "the current board again: {frame}");
}

/// Each phase keeps its own image: a second phase is added after the first
/// rather than written over it, and the same phase again replaces its image in
/// place rather than adding a second one under the same label.
#[test]
fn a_checkpoint_is_kept_per_phase() {
    let mut board = Board::new();
    let mut state = RuntimeState {
        interview_mode: InterviewMode::Whiteboard,
        ..RuntimeState::default()
    };
    for (bytes, checkpoint) in [
        (vec![0xff, 0xd8, 0x01], "Example checkpoint"),
        (vec![0xff, 0xd8, 0x02], "Approach checkpoint"),
        (vec![0xff, 0xd8, 0x03], "Example checkpoint"),
    ] {
        let snapshot = BoardSnapshot {
            bytes,
            strokes: 1,
            checkpoint: Some(checkpoint),
        };
        record(&mut board, &mut state, snapshot);
    }

    let labels = board
        .checkpoints
        .iter()
        .map(|(label, bytes)| (*label, bytes.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        [
            ("Example checkpoint", vec![0xff, 0xd8, 0x03]),
            ("Approach checkpoint", vec![0xff, 0xd8, 0x02]),
        ]
    );
}

/// Clearing the board between steps does not take back the work drawn before
/// it: the count the interviewer is told falls to zero, while Coding, Test and
/// Optimizations stay reachable.
#[test]
fn a_cleared_board_keeps_the_work_drawn_before_it() {
    let mut board = Board::new();
    let mut state = RuntimeState {
        interview_mode: InterviewMode::Whiteboard,
        ..RuntimeState::default()
    };
    let mut drawn = |strokes| {
        let snapshot = BoardSnapshot {
            bytes: vec![0xff, 0xd8],
            strokes,
            checkpoint: None,
        };
        record(&mut board, &mut state, snapshot);
        (state.board_strokes, crate::agent::written_work(&state))
    };

    // One stroke short, because a floor that is only tested from zero is a
    // floor any number above zero would pass.
    assert_eq!(drawn(crate::agent::MIN_BOARD_STROKES - 1), (2, false));
    assert_eq!(drawn(crate::agent::MIN_BOARD_STROKES), (3, true));
    assert_eq!(drawn(0), (0, true), "cleared, and still drawn");
}

#[tokio::test]
async fn asking_for_a_board_before_one_arrived_sends_nothing() {
    let (mut gemini, mut frames) = live_session().await;
    let mut board = Board::new();
    resend(&mut board, &mut gemini).await.unwrap();
    assert_eq!(board.last_sent, None);
    assert!(frames.try_recv().is_err());
}

#[tokio::test]
async fn a_coding_drawing_owns_realtime_images_until_the_interview_ends() {
    let (mut gemini, mut frames) = live_session().await;
    let mut board = Board::new();
    let mut state = RuntimeState::default();
    assert!(board.allows_camera(true));
    assert!(!board.allows_camera(false));

    record(&mut board, &mut state, snapshot(1));
    assert!(!board.allows_camera(true));
    send_if_due(&mut board, &mut gemini, Instant::now())
        .await
        .unwrap();
    let drawing: serde_json::Value = serde_json::from_str(&next_frame(&mut frames).await).unwrap();
    assert!(drawing.get("clientContent").is_none());
    assert_eq!(drawing["realtimeInput"]["video"]["mimeType"], "image/jpeg");
    assert_eq!(drawing["realtimeInput"]["video"]["data"], "/9gB");
    assert!(!board.unsent);

    resend(&mut board, &mut gemini).await.unwrap();
    let resent: serde_json::Value = serde_json::from_str(&next_frame(&mut frames).await).unwrap();
    assert_eq!(resent, drawing);
    assert!(!board.allows_camera(true));

    let mut cleared = snapshot(0);
    cleared.strokes = 0;
    record(&mut board, &mut state, cleared);
    assert!(
        !board.allows_camera(true),
        "clearing does not restore the camera input"
    );
    assert!(board.report_boards().is_empty());
}

#[tokio::test]
async fn a_failed_handover_image_keeps_the_drawing_and_does_not_abort() {
    let (mut gemini, _frames) = live_session().await;
    gemini.shutdown().await.unwrap();
    let mut board = Board::new();
    let mut state = RuntimeState::default();
    board.sender().try_send(snapshot(7)).unwrap();
    resend_for_handover(&mut board, &mut state, &mut gemini).await;
    assert_eq!(state.board_snapshots, 1);
    assert_eq!(state.board_strokes, 7);
    assert_eq!(board.latest.as_deref(), Some(&[0xff, 0xd8, 7][..]));
    assert!(board.unsent, "a failed write must leave the drawing owed");
    assert!(
        resend(&mut board, &mut gemini).await.is_err(),
        "the socket really refuses writes"
    );
}

#[tokio::test]
async fn a_handover_sends_queued_ink_even_inside_the_throttle_interval() {
    let (mut gemini, mut frames) = live_session().await;
    let mut board = Board::new();
    let mut state = RuntimeState::default();
    record(&mut board, &mut state, snapshot(1));
    send_if_due(&mut board, &mut gemini, Instant::now())
        .await
        .unwrap();
    next_frame(&mut frames).await;
    board.sender().try_send(snapshot(7)).unwrap();
    resend_for_handover(&mut board, &mut state, &mut gemini).await;
    let frame = next_frame(&mut frames).await;
    assert!(
        frame.contains("/9gH"),
        "the newest image precedes the reply: {frame}"
    );
    assert!(!board.unsent);
    assert_eq!(state.board_snapshots, 2);
}

use super::super::{media, settle_example_handover};

fn camera_for_drawing_handover() -> media::CandidateMedia {
    use ::livekit::webrtc::video_source::native::NativeVideoSource;
    use ::livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
    use ::livekit::webrtc::video_stream::native::NativeVideoStream;
    let source = NativeVideoSource::new(
        VideoResolution {
            width: 4,
            height: 4,
        },
        false,
    );
    let track = ::livekit::prelude::LocalVideoTrack::create_video_track(
        "test-camera",
        RtcVideoSource::Native(source),
    );
    let mut media = media::CandidateMedia::new();
    media.video = Some(NativeVideoStream::new(track.rtc_track()));
    media
}

#[tokio::test]
async fn example_handover_only_consumes_ink_and_replaces_the_camera_in_coding() {
    for yielded in [false, true] {
        for surface in [None, Some("speech"), Some("example")] {
            for mode in [InterviewMode::Coding, InterviewMode::Whiteboard] {
                for behavioral in [false, true] {
                    let eligible = yielded
                        && surface == Some("example")
                        && mode == InterviewMode::Coding
                        && !behavioral;
                    let label = format!(
                        "yield={yielded}, surface={surface:?}, mode={mode:?}, behavioral={behavioral}"
                    );
                    let (mut gemini, mut received) = live_session().await;
                    let mut state = RuntimeState {
                        interview_mode: mode,
                        behavioral_round_started: behavioral,
                        ..RuntimeState::default()
                    };
                    let mut board = Board::new();
                    board
                        .sender()
                        .try_send(BoardSnapshot {
                            bytes: vec![0xff, 0xd8, 7],
                            strokes: 7,
                            checkpoint: None,
                        })
                        .unwrap();
                    let mut media = camera_for_drawing_handover();
                    settle_example_handover(
                        yielded,
                        surface,
                        &mut state,
                        &mut board,
                        &mut media,
                        &mut gemini,
                    )
                    .await;
                    assert_eq!(state.board_snapshots, u32::from(eligible), "{label}");
                    assert_eq!(media.video.is_none(), eligible, "{label}");
                    if !eligible {
                        gemini.send_context("ordinary turn", false).await.unwrap();
                    }
                    let message =
                        serde_json::from_str::<serde_json::Value>(&next_frame(&mut received).await)
                            .unwrap();
                    assert_eq!(
                        message.get("realtimeInput").is_some(),
                        eligible,
                        "{label}: {message:?}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn an_empty_drawing_handover_preserves_the_camera() {
    let (mut gemini, mut received) = live_session().await;
    let mut state = RuntimeState::default();
    let mut board = Board::new();
    let mut media = camera_for_drawing_handover();
    settle_example_handover(
        true,
        Some("example"),
        &mut state,
        &mut board,
        &mut media,
        &mut gemini,
    )
    .await;
    assert!(media.video.is_some());
    assert_eq!(state.board_snapshots, 0);
    gemini.send_context("ordinary turn", false).await.unwrap();
    let message =
        serde_json::from_str::<serde_json::Value>(&next_frame(&mut received).await).unwrap();
    assert!(message.get("realtimeInput").is_none());
}
