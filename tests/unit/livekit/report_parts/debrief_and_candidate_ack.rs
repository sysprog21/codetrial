// Part of report.rs: included in place, so every item here shares that file's
// module.

/// What the reviewer said about each follow-up lands beside it, and the
/// server decides what the reviewer cannot: a follow-up never handed to the
/// interviewer was not reached, whatever the response claims, and one handed
/// over that no report judged is unknown rather than missed.
#[test]
fn the_debrief_places_each_follow_up_judgment_beside_its_follow_up() {
    use crate::agent::{
        EvidenceKind, EvidenceSource, FRAMEWORK_VERSION, FrameworkEvidence, FrameworkPhase,
    };
    let config = report_test_config();
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let texts = boot.problem.variant().follow_ups;
    assert_eq!(texts.len(), 3);
    let judged = || {
        serde_json::json!({
            "codingScore": 80,
            "followUps": [
                {"index": 1, "raised": true, "assessment": "You extended the map to three."},
                {"index": 2, "raised": false, "assessment": null},
            ],
        })
    };

    let mut released = RuntimeState::default();
    for phase in [FrameworkPhase::Test, FrameworkPhase::Optimizations] {
        released.framework_evidence.push(FrameworkEvidence {
            at_ms: 0,
            phase,
            source: EvidenceSource::CandidateSpeech,
            kind: EvidenceKind::Observed,
            confidence: 100,
            summary: "Synthetic testing and complexity.".into(),
            framework_version: FRAMEWORK_VERSION,
        });
    }
    let mut report = judged();
    stamp_report_debrief(&mut report, &boot, &released);
    assert!(
        report.get("followUps").is_none(),
        "the judgment moved into the debrief"
    );
    assert_eq!(
        report["debrief"]["followUps"],
        serde_json::json!([
            {"text": texts[0], "raised": true, "assessment": "You extended the map to three."},
            {"text": texts[1], "raised": false, "assessment": null},
            {"text": texts[2], "raised": null, "assessment": null},
        ])
    );

    let mut early = judged();
    stamp_report_debrief(&mut early, &boot, &RuntimeState::default());
    for entry in early["debrief"]["followUps"].as_array().unwrap() {
        assert_eq!(entry["raised"], false, "{entry}");
        assert!(entry["assessment"].is_null(), "{entry}");
    }

    // A transcript cut at its opening cannot show a follow-up went unasked.
    let mut cut = released.clone();
    cut.transcript = vec![format!(
        "Candidate: {}",
        "a".repeat(crate::agent::MAX_TRANSCRIPT_BYTES)
    )];
    let mut report = judged();
    stamp_report_debrief(&mut report, &boot, &cut);
    assert_eq!(report["debrief"]["followUps"][0]["raised"], true);
    assert!(report["debrief"]["followUps"][1]["raised"].is_null());

    let mut uncertain = released.clone();

    // The reviewer sees only the marker for line 0, so a positive judgment
    // rests on other evidence, here the candidate's answer, and is kept.
    uncertain.transcript = vec![
        "Interviewer: Could you extend that to three?".into(),
        "Candidate: For three I would keep a second map of pair sums.".into(),
    ];
    uncertain.interviewer_delivery.uncertain.insert(0);
    let mut report = judged();
    stamp_report_debrief(&mut report, &boot, &uncertain);
    assert_eq!(report["debrief"]["followUps"][0]["raised"], true);
    assert!(report["debrief"]["followUps"][1]["raised"].is_null());
    assert!(report["debrief"]["followUps"][1]["assessment"].is_null());

    let mut lost = final_report(
        None,
        0,
        Some("model unavailable"),
        boot.problem,
        crate::agent::ReportRounds::of(&released),
        boot.interview_mode,
        false,
    );
    stamp_report_debrief(&mut lost, &boot, &released);
    for entry in lost["debrief"]["followUps"].as_array().unwrap() {
        assert!(entry["raised"].is_null(), "{entry}");
        assert!(entry["assessment"].is_null(), "{entry}");
    }
}

#[test]
fn coding_assessment_excludes_auxiliary_images_even_if_supplied() {
    let config = report_test_config();
    let boot = crate::runtime::bootstrap(&config, "interview-example", Some("two-sum"), 45);
    let mut state = RuntimeState::default();
    let frozen = freeze_assessment(
        &boot,
        &mut state,
        20.0,
        vec![ReportBoard {
            label: "Example drawing",
            bytes: vec![0xff, 0xd8, 0xff],
        }],
    );
    assert!(frozen.report_boards().is_empty());
    assert!(
        !frozen
            .prompt
            .contains("The labeled images attached to this message")
    );
}

#[test]
fn frozen_drawing_receipt_uses_received_snapshots_even_without_report_images() {
    let config = report_test_config();
    let boot = bootstrap(&config, "interview-example", Some("two-sum"), 45);
    for count in [0, 1, 2] {
        let mut state = RuntimeState {
            board_snapshots: count,
            ..RuntimeState::default()
        };
        let frozen = freeze_assessment(
            &boot,
            &mut state,
            20.0,
            vec![ReportBoard {
                label: "Example drawing",
                bytes: vec![0xff, 0xd8, 7],
            }],
        );
        assert!(frozen.report_boards().is_empty());
        assert_eq!(
            frozen.drawing_received(),
            count != 0,
            "snapshot count {count}"
        );
        state.board_snapshots = if count == 0 { 1 } else { 0 };
        assert_eq!(
            frozen.drawing_received(),
            count != 0,
            "later live state must not change the frozen receipt"
        );
    }
}

// The candidate half of report delivery, run through the production report path
// over a recorded room. The agent side is `run_recovery` and `deliver_report`;
// only the candidate's receipt and the room are stand-ins. This is local
// evidence. It certifies no SDK data channel.

struct AckEvent {
    sender: &'static str,
    payload: Vec<u8>,
}

impl DeliveryEvent for AckEvent {
    fn acknowledges(&self, candidate: &str, id: &str, candidate_gone: bool) -> bool {
        is_report_receipt(
            Some(crate::runtime::TOPIC_CONTROL),
            Some(self.sender),
            candidate,
            &self.payload,
            id,
            candidate_gone,
        )
    }
}

/// A room whose published packets are recorded and answered by
/// `candidate_ack`, the way the page answers a received report.
struct AckingRoom {
    participant: super::super::room_link::ParticipantLink,
    sender: tokio::sync::mpsc::UnboundedSender<AckEvent>,
    events: tokio::sync::mpsc::UnboundedReceiver<AckEvent>,
    candidate_acks: bool,
    receipts: Vec<serde_json::Value>,
}

impl AckingRoom {
    fn new(participant: super::super::room_link::ParticipantLink, candidate_acks: bool) -> Self {
        let (sender, events) = tokio::sync::mpsc::unbounded_channel();
        Self {
            participant,
            sender,
            events,
            candidate_acks,
            receipts: Vec::new(),
        }
    }
}

impl RecoveryRoom for AckingRoom {
    fn candidate_present(&self) -> bool {
        true
    }

    async fn next(&mut self) -> RecoveryEvent {
        std::future::pending().await
    }

    async fn notify(&mut self, _: RecoveryNotice) {}

    async fn publish(
        &mut self,
        packet: DataPacket,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let Self {
            participant,
            sender,
            events,
            candidate_acks,
            receipts,
        } = self;
        deliver_report(
            |packet| {
                if *candidate_acks
                    && let Some(ack) = candidate_ack(packet.topic.as_deref(), &packet.payload)
                {
                    receipts.push(serde_json::from_slice(&ack.payload).unwrap());
                    sender
                        .send(AckEvent {
                            sender: "candidate",
                            payload: ack.payload,
                        })
                        .unwrap();
                }
                let participant = participant.clone();
                async move { participant.publish_data(packet).await }
            },
            events,
            "candidate",
            "room",
            || true,
            packet,
        )
        .await
    }
}

async fn deliver_final_report_to(
    candidate_acks: bool,
) -> (Vec<super::super::room_link::Sent>, Vec<serde_json::Value>) {
    let config = report_test_config();
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let mut live = RuntimeState::default();
    let mut frozen = freeze_assessment(&boot, &mut live, 12.0, Vec::new());
    let keys = GeminiKeys::single("ack-delivery");
    let (room, recorder) = super::super::room_link::RoomLink::recording("ack-delivery");
    let mut acking = AckingRoom::new(room.participant(), candidate_acks);
    run_recovery(
        &mut acking,
        RecoveryReport {
            boot: &boot,
            state: &mut frozen.state,
            reason: "manual_end",
            keys: &keys,
            refused: false,
        },
        Ok(Ok(serde_json::json!({}))),
        async { Ok(Ok(serde_json::json!({}))) },
        tokio::time::Instant::now,
        async {},
    )
    .await
    .unwrap();
    (recorder.take(), acking.receipts)
}

/// Declared before running: one final report goes out on the report topic, the
/// candidate acknowledges it once, and the delivery stops there, so exactly one
/// packet is recorded and exactly one receipt is produced.
#[tokio::test(start_paused = true)]
async fn an_acknowledged_final_report_is_sent_once_and_receipted_once() {
    let (sent, receipts) = deliver_final_report_to(true).await;
    let reports: Vec<_> = sent
        .iter()
        .filter_map(|item| match item {
            super::super::room_link::Sent::Data { topic, message } => {
                Some((topic.as_deref(), message))
            }
            _ => None,
        })
        .collect();
    assert_eq!(reports.len(), 1, "{sent:?}");
    assert_eq!(reports[0].0, Some(TOPIC_REPORT));
    assert!(is_final_report(&serde_json::to_vec(reports[0].1).unwrap()));
    assert_eq!(receipts.len(), 1, "{receipts:?}");
    assert_eq!(receipts[0]["type"], "report_received");
}

/// Declared before running: with no acknowledgement the identical bytes go out
/// `DELIVERY_ATTEMPTS` times and nothing is receipted, so the receipt above is
/// what ended the first delivery, not the end of the room.
#[tokio::test(start_paused = true)]
async fn an_unacknowledged_final_report_is_retransmitted_without_a_receipt() {
    let (sent, receipts) = deliver_final_report_to(false).await;
    assert_eq!(sent.len(), DELIVERY_ATTEMPTS, "{sent:?}");
    assert!(sent.windows(2).all(|pair| pair[0] == pair[1]), "{sent:?}");
    assert!(receipts.is_empty());
}

/// Declared before running: the helper acknowledges only an object on the
/// report topic, with the digest of the received bytes, and what it builds is
/// accepted by `is_report_receipt` only for that candidate and report.
#[test]
fn the_candidate_ack_matches_the_receipt_the_agent_accepts() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/report-receipt.json")).unwrap();
    let report = br#"{"codingScore":80}"#;
    let ack = candidate_ack(Some(TOPIC_REPORT), report).expect("a report is acknowledged");
    assert_eq!(ack.topic.as_deref(), Some(crate::runtime::TOPIC_CONTROL));
    assert!(ack.reliable);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&ack.payload).unwrap(),
        fixture["cases"][0]["payload"]
    );
    let id = crate::sha256_hex(&[report]);
    let accepts = |sender, id: &str| {
        is_report_receipt(
            Some(crate::runtime::TOPIC_CONTROL),
            sender,
            "candidate",
            &ack.payload,
            id,
            false,
        )
    };
    assert!(accepts(Some("candidate"), &id));
    assert!(!accepts(Some("peer"), &id));
    assert!(!accepts(Some("candidate"), "another-report"));

    // Not a report: another topic, bytes that are not JSON, JSON that is not an
    // object, and an agent control message all draw no receipt.
    for (topic, payload) in [
        (Some(crate::runtime::TOPIC_CONTROL), report.as_slice()),
        (Some("lk.transcription"), report.as_slice()),
        (None, report.as_slice()),
        (Some(TOPIC_REPORT), b"invalid".as_slice()),
        (Some(TOPIC_REPORT), b"[1]".as_slice()),
        (Some(TOPIC_REPORT), b"\"report\"".as_slice()),
    ] {
        assert!(candidate_ack(topic, payload).is_none(), "{topic:?}");
    }
    assert!(is_final_report(report));
    assert!(!is_final_report(br#"{"reportRecovery":{}}"#));
    assert!(!is_final_report(b"invalid"));
}
