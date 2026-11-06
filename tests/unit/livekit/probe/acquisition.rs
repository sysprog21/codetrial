use super::*;
use crate::livekit::latency::ReplyLatency;
use serde_json::json;
use std::time::Instant;

fn limits() -> Limits {
    Limits {
        batches: 3,
        calls: 4,
        bytes: 8192,
    }
}
fn identity() -> IdentityView<'static> {
    IdentityView {
        room: "fixture",
        session: 7,
        socket: 1,
        trace_turn: None,
        clock: json!({"clock_uncertainty_ms":null}),
    }
}
fn call(id: &str) -> GeminiFunctionCall {
    GeminiFunctionCall {
        id: id.into(),
        name: "read_editor".into(),
        args: json!({"private":"not retained"}),
    }
}
fn answers(id: &str, result: Value) -> Vec<(GeminiFunctionCall, Value)> {
    vec![(call(id), result)]
}

#[test]
fn assigned_scope_and_positive_limits_are_required() {
    assert!(Capture::new("", 7, limits()).is_err());
    assert!(Capture::new("fixture", 0, limits()).is_err());
    for bad in [
        Limits {
            batches: 0,
            ..limits()
        },
        Limits {
            calls: 0,
            ..limits()
        },
        Limits {
            bytes: 1,
            ..limits()
        },
    ] {
        assert!(Capture::new("fixture", 7, bad).is_err());
    }
}

#[test]
fn room_and_session_admit_but_socket_changes_are_retained() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    let _ = capture.record(identity(), &answers("a", json!("authored")));
    let mut next = identity();
    next.socket = 2;
    let _ = capture.record(next, &answers("b", json!("second")));
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.failure, None);
    assert_eq!(snapshot.batches[1].identity.socket, 2);
    assert_eq!(snapshot.batches[1].sequence, 1);
    assert_eq!(snapshot.retained_calls, 2);
    assert_eq!(
        snapshot.retained_bytes,
        serde_json::to_vec(&snapshot.batches).unwrap().len()
    );
    assert!(
        !serde_json::to_string(&snapshot.batches)
            .unwrap()
            .contains("not retained")
    );
    for (room, session) in [("", 0), ("foreign", 7), ("fixture", 8)] {
        let capture = Capture::new("fixture", 7, limits()).unwrap();
        let mut view = identity();
        view.room = room;
        view.session = session;
        let _ = capture.record(view, &answers("a", json!(null)));
        let _ = capture.record(identity(), &answers("b", json!(null)));
        assert_eq!(capture.snapshot().failure, Some(Failure::Scope));
        assert!(capture.snapshot().batches.is_empty());
    }
}

#[test]
fn byte_budget_counts_utf8_escaping_and_array_punctuation_before_cloning() {
    let executed = answers("a", json!({"value":"é\n\\\""}));
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    let _ = capture.record(identity(), &executed);
    let exact = capture.snapshot().retained_bytes;
    for (bytes, accepted) in [(exact, true), (exact - 1, false)] {
        let capture = Capture::new("fixture", 7, Limits { bytes, ..limits() }).unwrap();
        let _ = capture.record(identity(), &executed);
        assert_eq!(capture.snapshot().batches.len(), usize::from(accepted));
        assert_eq!(
            capture.snapshot().failure,
            (!accepted).then_some(Failure::Limit)
        );
    }
    let capture = Capture::new(
        "fixture",
        7,
        Limits {
            bytes: exact,
            ..limits()
        },
    )
    .unwrap();
    let _ = capture.record(identity(), &executed);
    let _ = capture.record(identity(), &answers("b", json!("second")));
    assert_eq!(capture.snapshot().failure, Some(Failure::Limit));
    assert_eq!(capture.snapshot().retained_bytes, exact);
}

#[test]
fn count_limits_and_ambiguous_ids_latch_failure() {
    for caps in [
        Limits {
            batches: 1,
            ..limits()
        },
        Limits {
            calls: 1,
            ..limits()
        },
    ] {
        let capture = Capture::new("fixture", 7, caps).unwrap();
        let _ = capture.record(identity(), &answers("a", json!(null)));
        let _ = capture.record(identity(), &answers("b", json!(null)));
        let _ = capture.record(identity(), &[]);
        assert_eq!(capture.snapshot().failure, Some(Failure::Limit));
        assert_eq!(capture.snapshot().retained_calls, 1);
    }
    for ids in [vec![""], vec!["same", "same"]] {
        let capture = Capture::new("fixture", 7, limits()).unwrap();
        let batch = ids
            .into_iter()
            .flat_map(|id| answers(id, json!(null)))
            .collect::<Vec<_>>();
        let _ = capture.record(identity(), &batch);
        assert_eq!(capture.snapshot().failure, Some(Failure::AmbiguousId));
        assert!(capture.snapshot().batches.is_empty());
    }
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    let _ = capture.record(identity(), &answers("same", json!(null)));
    let _ = capture.record(identity(), &answers("same", json!(null)));
    assert_eq!(capture.snapshot().failure, Some(Failure::AmbiguousId));
    assert_eq!(capture.snapshot().batches.len(), 1);
}

#[test]
fn identity_view_preserves_trace_and_existing_paired_clock_policy() {
    let now = Instant::now();
    let mut latency = ReplyLatency::default();
    assert_eq!(latency.acquisition_identity().session, 0);
    latency.bind("fixture", 7, 1, 1500);
    let before = latency.acquisition_identity();
    assert_eq!(before.trace_turn, None);
    assert!(before.clock["emitted_ms"].is_null());
    assert!(before.clock["start_unix_ms"].is_null());
    assert!(before.clock["emitted_unix_ms"].as_u64().is_some());
    assert_eq!(before.clock["clock_uncertainty_ms"], Value::Null);
    latency.anchor(now);
    let after = latency.acquisition_identity();
    assert_eq!(after.trace_turn, None);
    assert!(after.clock["start_unix_ms"].as_u64().is_some());
    assert!(after.clock["emitted_unix_ms"].as_u64().is_some());
    assert!(after.clock["emitted_ms"].as_i64().is_some());
    assert!(after.clock["clock_uncertainty_ms"].as_u64().is_some());
    latency.prompt(now);
    let turn = latency.acquisition_identity().trace_turn;
    assert!(turn.is_some());
    assert_eq!(latency.acquisition_identity().trace_turn, turn);
}

#[test]
fn constructor_rejects_scope_metadata_before_cloning_it() {
    assert!(
        Capture::new(
            "fixture",
            7,
            Limits {
                bytes: 2,
                ..limits()
            }
        )
        .is_err()
    );
    let escaped_room = "\"".repeat(1024);
    assert!(
        Capture::new(
            &escaped_room,
            7,
            Limits {
                bytes: 2048,
                ..limits()
            }
        )
        .is_err()
    );
    let multibyte_room = "é".repeat(1024);
    assert!(
        Capture::new(
            &multibyte_room,
            7,
            Limits {
                bytes: 1536,
                ..limits()
            }
        )
        .is_err()
    );
    let large_room = "x".repeat(8192);
    assert!(
        Capture::new(
            &large_room,
            7,
            Limits {
                bytes: 256,
                ..limits()
            }
        )
        .is_err()
    );
}

fn admitted_ticket(capture: &Arc<Capture>, id: &str) -> Ticket {
    let sequence = capture
        .record(identity(), &answers(id, json!("executed")))
        .unwrap();
    Ticket {
        capture: Arc::clone(capture),
        sequence,
    }
}

#[test]
fn completion_preserves_exact_array_bytes_and_original_object_clock() {
    let prototype = Capture::new("fixture", 7, limits()).unwrap();
    let _ = admitted_ticket(&prototype, "a");
    let null_bytes = prototype.snapshot().retained_bytes;
    for (sent, allowance, accepted) in [(true, 0, true), (false, 0, false), (false, 1, true)] {
        let capture = Capture::new(
            "fixture",
            7,
            Limits {
                bytes: null_bytes + allowance,
                ..limits()
            },
        )
        .unwrap();
        let ticket = admitted_ticket(&capture, "a");
        let before = capture.snapshot();
        assert!(before.batches[0].identity.clock.is_object());
        ticket.complete(sent);
        let after = capture.snapshot();
        assert_eq!(after.batches[0].identity, before.batches[0].identity);
        assert_eq!(after.batches[0].sequence, 0);
        assert_eq!(after.batches[0].calls, before.batches[0].calls);
        assert_eq!(after.retained_calls, 1);
        assert_eq!(
            after.batches[0].local_send_completed,
            accepted.then_some(sent)
        );
        assert_eq!(after.failure, (!accepted).then_some(Failure::Limit));
        assert_eq!(
            after.retained_bytes,
            before.retained_bytes + usize::from(accepted && !sent)
        );
        assert_eq!(
            after.retained_bytes,
            serde_json::to_vec(&after.batches).unwrap().len()
        );
    }
}

#[test]
fn rejected_or_latched_admission_cannot_update_an_earlier_batch() {
    for rejected in [Failure::Scope, Failure::Limit, Failure::AmbiguousId] {
        let caps = Limits {
            batches: if rejected == Failure::Limit { 1 } else { 3 },
            ..limits()
        };
        let capture = Capture::new("fixture", 7, caps).unwrap();
        assert!(capture.record(identity(), &[]).is_none());
        assert_eq!(capture.snapshot().failure, None);
        let ticket = admitted_ticket(&capture, "first");
        let mut view = identity();
        if rejected == Failure::Scope {
            view.room = "foreign";
        }
        let id = if rejected == Failure::AmbiguousId {
            "first"
        } else {
            "second"
        };
        assert!(
            capture
                .record(view, &answers(id, json!("rejected")))
                .is_none()
        );
        assert!(
            capture
                .record(identity(), &answers("later", json!("latched")))
                .is_none()
        );
        let before = capture.snapshot();
        assert_eq!(before.failure, Some(rejected));
        ticket.complete(false);
        let after = capture.snapshot();
        assert_eq!(
            after, before,
            "a latched capture refuses completion of an admitted ticket too"
        );
    }
}

#[test]
fn sequence_mismatch_and_duplicate_completion_fail_closed() {
    for sequence in [0, 1] {
        let capture = Capture::new("fixture", 7, limits()).unwrap();
        let ticket = admitted_ticket(&capture, "first");
        if sequence == 0 {
            ticket.complete(true);
        } else {
            drop(ticket);
        }
        let before = capture.snapshot();
        Ticket {
            capture: Arc::clone(&capture),
            sequence,
        }
        .complete(false);
        let after = capture.snapshot();
        assert_eq!(after.failure, Some(Failure::Scope));
        assert_eq!(after.batches, before.batches);
        assert_eq!(after.retained_bytes, before.retained_bytes);
    }
}

#[test]
fn dropping_a_ticket_leaves_completion_unknown() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    let ticket = admitted_ticket(&capture, "first");
    let before = capture.snapshot();
    drop(ticket);
    assert_eq!(capture.snapshot(), before);
    assert_eq!(capture.snapshot().batches[0].local_send_completed, None);
}

#[test]
fn owner_gate_changes_are_scoped_to_the_post_sample_tool_send_boundary() {
    use super::owner::{Gate, Stage, Transition};
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    assert!(Gate::new(&capture, 0, Stage::ToolSend, None).is_err());
    for stage in [
        Stage::Acknowledgement,
        Stage::BoardSettlement,
        Stage::BoardResend,
    ] {
        assert!(Gate::new(&capture, 1, stage, Some(Transition::Hold(true))).is_err());
    }
    assert!(Gate::new(&capture, 1, Stage::ToolSend, Some(Transition::Hold(true))).is_ok());
}

#[test]
fn completion_can_finish_admitted_sequences_out_of_order() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    let first = admitted_ticket(&capture, "first");
    let second = admitted_ticket(&capture, "second");
    second.complete(false);
    first.complete(true);
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.failure, None);
    assert_eq!(snapshot.batches[0].calls[0].id, "first");
    assert_eq!(snapshot.batches[0].local_send_completed, Some(true));
    assert_eq!(snapshot.batches[1].calls[0].id, "second");
    assert_eq!(snapshot.batches[1].local_send_completed, Some(false));
    assert_eq!(
        snapshot.retained_bytes,
        serde_json::to_vec(&snapshot.batches).unwrap().len()
    );
}

fn bound_scope_records(capture: &Arc<Capture>, room: &str, session: u64) -> bool {
    let mut id = identity();
    id.room = room;
    id.session = session;
    capture
        .record(id, &answers("a", json!("authored")))
        .is_some()
}

#[test]
fn an_unarmed_observer_never_arms_itself_from_a_bind() {
    let arming = Arming::detached(limits());
    arming.on_bind("fixture", 7);
    assert!(arming.capture().is_none());
    assert_eq!(arming.failure(), Some(ArmFailure::BindBeforeAssignment));
    arming.on_assignment("fixture");
    assert!(arming.capture().is_none(), "a failed observer stays failed");
}

#[test]
fn an_assignment_alone_admits_nothing_until_its_session_binds() {
    let arming = Arming::detached(limits());
    arming.on_assignment("fixture");
    assert!(arming.capture().is_none());
    assert_eq!(arming.failure(), None);
    arming.on_bind("fixture", 7);
    let capture = arming
        .capture()
        .expect("the expected room's bind admits it");
    assert!(bound_scope_records(&capture, "fixture", 7));
    assert!(!bound_scope_records(&capture, "fixture", 8));
    assert!(!bound_scope_records(&capture, "other", 7));
}

#[test]
fn a_second_room_or_a_foreign_bind_is_a_fixture_failure() {
    for foreign in ["assign", "bind"] {
        let arming = Arming::detached(limits());
        arming.on_assignment("fixture");
        match foreign {
            "assign" => arming.on_assignment("other"),
            _ => arming.on_bind("other", 7),
        }
        assert_eq!(arming.failure(), Some(ArmFailure::UnrelatedRoom));
        arming.on_bind("fixture", 7);
        assert!(arming.capture().is_none(), "{foreign}");
    }
}

#[test]
fn the_first_bound_session_is_latched_and_zero_is_refused() {
    let arming = Arming::detached(limits());
    arming.on_assignment("fixture");
    arming.on_bind("fixture", 0);
    assert_eq!(arming.failure(), Some(ArmFailure::Session));

    let arming = Arming::detached(limits());
    arming.on_assignment("fixture");
    arming.on_assignment("fixture");
    arming.on_bind("fixture", 7);
    arming.on_bind("fixture", 7);
    assert!(
        arming.capture().is_some(),
        "repeats of the same scope are fine"
    );
    assert_eq!(arming.failure(), None);
    arming.on_bind("fixture", 8);
    assert_eq!(arming.failure(), Some(ArmFailure::Session));
    assert!(arming.capture().is_none());
}

#[test]
fn the_real_dispatch_and_bind_boundaries_admit_only_the_expected_scope() {
    use crate::config::{PRIMARY_PROVIDER_ID, Provider};
    use crate::dispatch::LocalDispatcher;
    use crate::web::{AgentJob, RoomDispatcher};

    if crate::test_isolation::isolated!("CODETRIAL_ACQUISITION_ARMING_TEST") {
        return;
    }
    let arming = Arming::new(limits());
    let installed = install_arming(&arming).unwrap();
    assert!(install_arming(&arming).is_err());

    // A current-thread runtime that is never driven: the room task the
    // dispatcher spawns is queued and dropped with it, so it can never reach
    // for the network, however the scheduler happens to fall.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let dispatcher = LocalDispatcher {
        config: crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap(),
        runtime: runtime.handle().clone(),
        state: Arc::default(),
        max_concurrent: 2,
    };
    let provider = Provider {
        id: PRIMARY_PROVIDER_ID.to_string(),
        url: "wss://primary.example".to_string(),
        api_key: "primary-key".to_string(),
        api_secret: "primary-secret".to_string(),
        google_api_keys: Vec::new(),
    };
    let mut latency = ReplyLatency::default();

    // Nothing is observed before the assignment and the bind.
    assert!(observe(&latency, &answers("early", json!("x"))).is_none());

    dispatcher
        .ensure_agent("fixture-room", &provider, AgentJob::Interview)
        .unwrap();
    assert_eq!(
        arming.failure(),
        None,
        "the expected room's own bind comes first"
    );
    latency.bind("fixture-room", 7, 1, 1500);
    assert!(observe(&latency, &answers("a", json!("x"))).is_some());

    // A second room reaching dispatch fails the fixture and stops capture.
    dispatcher
        .ensure_agent("other-room", &provider, AgentJob::Interview)
        .unwrap();
    assert_eq!(arming.failure(), Some(ArmFailure::UnrelatedRoom));
    assert!(observe(&latency, &answers("b", json!("x"))).is_none());

    drop(installed);
    drop(runtime);
}

#[test]
fn a_blocked_or_removed_observer_leaves_no_capture_installed() {
    if crate::test_isolation::isolated!("CODETRIAL_ACQUISITION_ARMING_BLOCKED_TEST") {
        return;
    }

    // Another capture holds the one slot: the fixture must see a failure rather
    // than believe its scope went live.
    let foreign = Capture::new("foreign", 1, limits()).unwrap();
    let held = install(&foreign).unwrap();
    let arming = Arming::new(limits());
    let installed = install_arming(&arming).unwrap();
    arming.on_assignment("fixture");
    bound("fixture", 7);
    assert_eq!(arming.failure(), Some(ArmFailure::Install));
    drop(installed);
    drop(held);

    // Removing the observer takes its capture out, and an event that was
    // already in flight afterwards installs nothing, because that capture would
    // have no owner left to remove it.
    let arming = Arming::new(limits());
    let installed = install_arming(&arming).unwrap();
    arming.on_assignment("fixture");
    bound("fixture", 7);
    assert!(arming.capture().is_some());
    drop(installed);
    arming.on_bind("fixture", 7);
    arming.on_assignment("fixture");
    let probe = Capture::new("probe", 1, limits()).unwrap();
    assert!(install(&probe).is_ok(), "a removed observer left a capture");
}

#[test]
fn a_teardown_clears_only_its_own_dispatch_hook() {
    if crate::test_isolation::isolated!("CODETRIAL_ACQUISITION_ARMING_HOOK_TEST") {
        return;
    }
    let first = Arming::new(limits());
    let installed = install_arming(&first).unwrap();
    // A successor that slipped in while this observer was being removed.
    let successor: crate::dispatch::AssignmentHook = Arc::new(|_| {});
    crate::dispatch::set_assignment_hook(Some(Arc::clone(&successor)));
    drop(installed);
    assert!(crate::dispatch::assignment_hook_is(&successor));
    crate::dispatch::clear_assignment_hook(&successor);
    assert!(!crate::dispatch::assignment_hook_is(&successor));

    // And a plain reinstall after a removal is armed from the first event.
    let second = Arming::new(limits());
    let guard = install_arming(&second).unwrap();
    crate::dispatch::notify_assigned("room");
    bound("room", 7);
    assert_eq!(second.failure(), None);
    assert!(second.capture().is_some());
    drop(guard);
}

fn calls(ids: &[&str]) -> Vec<GeminiFunctionCall> {
    ids.iter().map(|id| call(id)).collect()
}

#[test]
fn a_received_call_is_recorded_before_it_executes() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    capture.receive(identity(), &calls(&["a", "b"]), false);
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.failure, None);
    assert_eq!(snapshot.batches.len(), 0);
    assert_eq!(snapshot.receipts.len(), 2);
    let first = &snapshot.receipts[0];
    assert_eq!(
        (first.id.as_str(), first.name.as_str()),
        ("a", "read_editor")
    );
    assert_eq!((first.identity.session, first.identity.socket), (7, 1));
    assert!(!first.discarded);
    assert!(
        snapshot
            .receipts
            .iter()
            .all(|receipt| receipt.batch.is_none())
    );
    // The arguments are never retained, only what identifies the call.
    assert!(
        !serde_json::to_string(&snapshot.receipts)
            .unwrap()
            .contains("not retained")
    );
}

#[test]
fn executing_a_batch_links_only_the_receipts_it_answers() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    capture.receive(identity(), &calls(&["a", "b"]), false);
    capture
        .record(identity(), &answers("a", json!("result")))
        .unwrap();
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.failure, None);
    assert_eq!(snapshot.receipts[0].batch, Some(0));
    assert_eq!(snapshot.receipts[1].batch, None, "received, never executed");
}

#[test]
fn a_call_received_during_a_discard_is_kept_and_marked() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    capture.receive(identity(), &calls(&["late"]), true);
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.failure, None);
    assert!(snapshot.receipts[0].discarded);
}

#[test]
fn receipts_fail_closed_on_scope_reuse_and_limits() {
    let scoped = Capture::new("fixture", 7, limits()).unwrap();
    let mut elsewhere = identity();
    elsewhere.session = 8;
    scoped.receive(elsewhere, &calls(&["a"]), false);
    assert_eq!(scoped.snapshot().failure, Some(Failure::Scope));
    assert!(scoped.snapshot().receipts.is_empty());

    let reused = Capture::new("fixture", 7, limits()).unwrap();
    reused.receive(identity(), &calls(&["a"]), false);
    reused.receive(identity(), &calls(&["a"]), false);
    assert_eq!(reused.snapshot().failure, Some(Failure::AmbiguousId));

    let twice = Capture::new("fixture", 7, limits()).unwrap();
    twice.receive(identity(), &calls(&["a", "a"]), false);
    assert_eq!(twice.snapshot().failure, Some(Failure::AmbiguousId));

    let blank = Capture::new("fixture", 7, limits()).unwrap();
    blank.receive(identity(), &calls(&[""]), false);
    assert_eq!(blank.snapshot().failure, Some(Failure::AmbiguousId));

    let many = Capture::new("fixture", 7, limits()).unwrap();
    many.receive(identity(), &calls(&["1", "2", "3", "4", "5"]), false);
    assert_eq!(many.snapshot().failure, Some(Failure::Limit));
    assert!(
        many.snapshot().receipts.is_empty(),
        "nothing partial is kept"
    );

    let tiny = Capture::new(
        "fixture",
        7,
        Limits {
            bytes: 150,
            ..limits()
        },
    )
    .unwrap();
    tiny.receive(
        identity(),
        &calls(&["a-long-call-id-that-will-not-fit-in-the-byte-budget"]),
        false,
    );
    assert_eq!(tiny.snapshot().failure, Some(Failure::Limit));
}

#[test]
fn a_latched_failure_stops_receipts_too() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    capture.receive(identity(), &calls(&["a"]), false);
    capture.receive(identity(), &calls(&["a"]), false);
    assert_eq!(capture.snapshot().failure, Some(Failure::AmbiguousId));
    capture.receive(identity(), &calls(&["fresh"]), false);
    assert_eq!(capture.snapshot().receipts.len(), 1);
}

fn receipt_bytes(capture: &Capture) -> (usize, usize) {
    let snapshot = capture.snapshot();
    (
        snapshot.retained_receipt_bytes,
        serde_json::to_vec(&snapshot.receipts).unwrap().len(),
    )
}

#[test]
fn receipt_bytes_are_the_serialized_array_before_and_after_linking() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    assert_eq!(
        receipt_bytes(&capture),
        (2, 2),
        "an empty array is two bytes"
    );
    capture.receive(identity(), &calls(&["a"]), false);
    let (counted, actual) = receipt_bytes(&capture);
    assert_eq!(counted, actual);
    capture.receive(identity(), &calls(&["b", "c"]), false);
    let (counted, actual) = receipt_bytes(&capture);
    assert_eq!(counted, actual, "commas between receipts are counted");
    capture
        .record(identity(), &answers("b", json!("result")))
        .unwrap();
    let (counted, actual) = receipt_bytes(&capture);
    assert_eq!(counted, actual, "null becomes a sequence number");
    assert_eq!(capture.snapshot().failure, None);
}

#[test]
fn the_receipt_byte_limit_is_exact() {
    let probe = Capture::new("fixture", 7, limits()).unwrap();
    probe.receive(identity(), &calls(&["a"]), false);
    let one = receipt_bytes(&probe).1;
    for (bytes, fits) in [(one, true), (one - 1, false)] {
        let capture = Capture::new("fixture", 7, Limits { bytes, ..limits() }).unwrap();
        capture.receive(identity(), &calls(&["a"]), false);
        assert_eq!(capture.snapshot().failure.is_none(), fits, "limit {bytes}");
    }
    // A second receipt that does not fit leaves the first alone.
    let capture = Capture::new(
        "fixture",
        7,
        Limits {
            bytes: one,
            ..limits()
        },
    )
    .unwrap();
    capture.receive(identity(), &calls(&["a"]), false);
    let before = receipt_bytes(&capture);
    capture.receive(identity(), &calls(&["b"]), false);
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.failure, Some(Failure::Limit));
    assert_eq!(snapshot.receipts.len(), 1);
    assert_eq!(receipt_bytes(&capture), before);
}

#[test]
fn a_batch_answers_only_the_receipt_from_its_own_socket() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    capture.receive(identity(), &calls(&["a"]), false);
    let mut later = identity();
    later.socket = 2;
    capture
        .record(later, &answers("a", json!("result")))
        .unwrap();
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.failure, None);
    assert_eq!(
        snapshot.receipts[0].batch, None,
        "received on socket 1, executed on 2"
    );
    assert_eq!(
        snapshot.batches.len(),
        1,
        "the execution itself is still recorded"
    );
}

#[test]
fn a_reply_keeps_its_newest_view_and_where_it_first_appeared() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    capture.receive(identity(), &calls(&["a"]), false);
    capture.reply(identity(), "interviewer-0", "Hel", false);
    capture.record(identity(), &answers("a", json!("ok")));
    capture.reply(identity(), "interviewer-0", "Hello", false);
    capture.receive(identity(), &calls(&["b"]), false);
    let mut later = identity();
    later.trace_turn = Some(9);
    later.clock = json!({"clock_uncertainty_ms":5});
    capture.reply(later, "interviewer-0", "Hello there", true);
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.failure, None);
    let [reply] = snapshot.replies.as_slice() else {
        panic!("one segment, one reply: {:?}", snapshot.replies);
    };
    assert_eq!(reply.text, "Hello there");
    assert!(reply.final_segment && !reply.revised);
    assert_eq!(reply.views, 3);
    // First seen after the receipt and before the batch that answered it.
    assert_eq!((reply.receipts_before, reply.batches_before), (1, 0));
    // The turn went on across a second call and an answer.
    assert_eq!((reply.receipts_last, reply.batches_last), (2, 1));
    assert_eq!(reply.identity, Identity::from_view(&identity()));
}

#[test]
fn a_rewritten_view_is_marked_and_a_reopened_final_one_too() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    capture.reply(identity(), "interviewer-0", "Hello there", false);
    capture.reply(identity(), "interviewer-0", "Hello", false);
    capture.reply(identity(), "interviewer-1", "Fine", true);
    capture.reply(identity(), "interviewer-1", "Fine", false);
    capture.reply(identity(), "interviewer-2", "Fine", true);
    capture.reply(identity(), "interviewer-2", "Fine and", true);
    // Ordinary growth afterwards does not make the page's view trustworthy.
    capture.reply(identity(), "interviewer-0", "Hello there, again", true);
    capture.reply(identity(), "interviewer-1", "Fine, thanks", true);
    let revised: Vec<_> = capture
        .snapshot()
        .replies
        .iter()
        .map(|reply| reply.revised)
        .collect();
    assert_eq!(revised, [true, true, false]);
}

#[test]
fn a_segment_id_on_another_socket_is_another_reply() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    capture.reply(identity(), "interviewer-0", "one", true);
    let mut next = identity();
    next.socket = 2;
    capture.reply(next, "interviewer-0", "two", true);
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.failure, None);
    assert_eq!(snapshot.replies.len(), 2);
    assert_eq!(snapshot.replies[0].text, "one");
    assert_eq!(snapshot.replies[1].identity.socket, 2);
}

#[test]
fn replies_fail_closed_on_scope_count_and_bytes() {
    for (room, session) in [("foreign", 7), ("fixture", 8)] {
        let capture = Capture::new("fixture", 7, limits()).unwrap();
        let mut view = identity();
        view.room = room;
        view.session = session;
        capture.reply(view, "interviewer-0", "x", true);
        let snapshot = capture.snapshot();
        assert_eq!(snapshot.failure, Some(Failure::Scope));
        assert!(snapshot.replies.is_empty());
    }
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    for index in 0..5 {
        capture.reply(identity(), &format!("interviewer-{index}"), "x", true);
    }
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.failure, Some(Failure::Limit));
    assert_eq!(snapshot.replies.len(), 4);

    // A view that no longer fits is dropped whole, and the earlier one stays.
    let small = Limits {
        bytes: 400,
        ..limits()
    };
    let capture = Capture::new("fixture", 7, small).unwrap();
    capture.reply(identity(), "interviewer-0", "short", false);
    capture.reply(identity(), "interviewer-0", &"long".repeat(200), false);
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.failure, Some(Failure::Limit));
    assert_eq!(snapshot.replies[0].text, "short");
}

#[test]
fn the_reply_byte_limit_is_exact() {
    let probe = Capture::new("fixture", 7, limits()).unwrap();
    probe.reply(identity(), "interviewer-0", "exact", true);
    let exact = serde_json::to_vec(&probe.snapshot().replies).unwrap().len();
    for (bytes, failure) in [(exact, None), (exact - 1, Some(Failure::Limit))] {
        let capture = Capture::new("fixture", 7, Limits { bytes, ..limits() }).unwrap();
        capture.reply(identity(), "interviewer-0", "exact", true);
        assert_eq!(capture.snapshot().failure, failure);
    }
}

#[test]
fn the_running_byte_counts_are_the_serialized_lengths() {
    // Each view is charged for its own change, so the running count has to stay
    // what serializing the whole array would give, through revisions that grow,
    // shrink and reopen a reply.
    let roomy = Limits {
        calls: 8,
        ..limits()
    };
    let capture = Capture::new("fixture", 7, roomy).unwrap();
    for (id, text, last) in [
        ("interviewer-0", "Hel", false),
        ("interviewer-0", "Hello there", true),
        ("interviewer-1", "Fine", true),
        ("interviewer-0", "Hi", false),
        ("interviewer-2", "\"quoted\" \u{3b1}", true),
    ] {
        capture.reply(identity(), id, text, last);
        capture.boundary(identity(), Ending::Complete, last);
        let snapshot = capture.snapshot();
        assert_eq!(snapshot.failure, None);
        assert_eq!(
            snapshot.retained_reply_bytes,
            serde_json::to_vec(&snapshot.replies).unwrap().len()
        );
        assert_eq!(snapshot.reply_bytes.len(), snapshot.replies.len());
        assert_eq!(
            snapshot.retained_boundary_bytes,
            serde_json::to_vec(&snapshot.boundaries).unwrap().len()
        );
    }
}

#[test]
fn the_boundary_byte_limit_is_exact() {
    let probe = Capture::new("fixture", 7, limits()).unwrap();
    probe.boundary(identity(), Ending::Complete, true);
    probe.boundary(identity(), Ending::Interrupted, false);
    let exact = serde_json::to_vec(&probe.snapshot().boundaries)
        .unwrap()
        .len();
    for (bytes, failure, kept) in [(exact, None, 2), (exact - 1, Some(Failure::Limit), 1)] {
        let capture = Capture::new("fixture", 7, Limits { bytes, ..limits() }).unwrap();
        capture.boundary(identity(), Ending::Complete, true);
        capture.boundary(identity(), Ending::Interrupted, false);
        let snapshot = capture.snapshot();
        assert_eq!(snapshot.failure, failure);
        assert_eq!(snapshot.boundaries.len(), kept);
    }
}

#[test]
fn a_latched_failure_stops_replies_too() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    capture.receive(identity(), &calls(&[""]), false);
    capture.reply(identity(), "interviewer-0", "x", true);
    assert!(capture.snapshot().replies.is_empty());
}

#[test]
fn a_boundary_keeps_arrival_order_and_what_preceded_it() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    capture.boundary(identity(), Ending::Complete, true);
    capture.receive(identity(), &calls(&["a"]), false);
    capture.record(identity(), &answers("a", json!("ok")));
    capture.reply(identity(), "interviewer-0", "Hi", false);
    let mut next = identity();
    next.socket = 2;
    capture.boundary(next, Ending::Interrupted, false);
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.failure, None);
    let seen: Vec<_> = snapshot
        .boundaries
        .iter()
        .map(|boundary| {
            (
                boundary.ending,
                boundary.accepted,
                boundary.identity.socket,
                boundary.receipts_before,
                boundary.batches_before,
                boundary.replies_before,
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            (Ending::Complete, true, 1, 0, 0, 0),
            (Ending::Interrupted, false, 2, 1, 1, 1)
        ]
    );
}

#[test]
fn boundaries_fail_closed_on_scope_count_and_bytes() {
    let mut view = identity();
    view.session = 8;
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    capture.boundary(view, Ending::Complete, true);
    assert_eq!(capture.snapshot().failure, Some(Failure::Scope));

    let capture = Capture::new("fixture", 7, limits()).unwrap();
    for _ in 0..5 {
        capture.boundary(identity(), Ending::Complete, true);
    }
    let snapshot = capture.snapshot();
    assert_eq!(snapshot.failure, Some(Failure::Limit));
    assert_eq!(snapshot.boundaries.len(), 4);

    let probe = Capture::new("fixture", 7, limits()).unwrap();
    probe.boundary(identity(), Ending::Complete, true);
    let exact = serde_json::to_vec(&probe.snapshot().boundaries)
        .unwrap()
        .len();
    for (bytes, failure) in [(exact, None), (exact - 1, Some(Failure::Limit))] {
        let capture = Capture::new("fixture", 7, Limits { bytes, ..limits() }).unwrap();
        capture.boundary(identity(), Ending::Complete, true);
        assert_eq!(capture.snapshot().failure, failure);
    }
}

#[test]
fn only_turn_endings_are_boundaries() {
    use crate::gemini::GeminiEvent;
    if crate::test_isolation::isolated!("CODETRIAL_ACQUISITION_BOUNDARY_TEST") {
        return;
    }
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    let _installed = install(&capture).unwrap();
    let mut latency = ReplyLatency::default();
    latency.bind("fixture", 7, 1, 1500);
    boundary(&latency, &GeminiEvent::OutputTranscript("x".into()), true);
    boundary(&latency, &GeminiEvent::TurnComplete, true);
    boundary(&latency, &GeminiEvent::Interrupted, false);
    let endings: Vec<_> = capture
        .snapshot()
        .boundaries
        .iter()
        .map(|boundary| (boundary.ending, boundary.accepted))
        .collect();
    assert_eq!(
        endings,
        [(Ending::Complete, true), (Ending::Interrupted, false)]
    );
}

#[test]
fn a_single_view_reply_starts_with_matching_counts() {
    let capture = Capture::new("fixture", 7, limits()).unwrap();
    capture.receive(identity(), &calls(&["a"]), false);
    capture.record(identity(), &answers("a", json!("ok")));
    capture.reply(identity(), "interviewer-0", "Hi", true);
    let reply = &capture.snapshot().replies[0];
    assert_eq!((reply.receipts_before, reply.batches_before), (1, 1));
    assert_eq!((reply.receipts_last, reply.batches_last), (1, 1));
}

#[test]
fn the_report_carries_replies_and_boundaries() {
    let arming = Arming::detached(limits());
    arming.on_assignment("fixture");
    arming.on_bind("fixture", 7);
    let capture = arming.capture().unwrap();
    capture.reply(identity(), "interviewer-0", "Hi", true);
    capture.boundary(identity(), Ending::Interrupted, false);
    let report = arming.report();
    assert_eq!(report["capture"]["replies"][0]["text"], "Hi");
    let boundary = &report["capture"]["boundaries"][0];
    assert_eq!(boundary["ending"], "Interrupted");
    assert_eq!(boundary["accepted"], false);
    assert_eq!(boundary["replies_before"], 1);
}
