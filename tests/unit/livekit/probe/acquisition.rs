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
fn answers(id: &str, result: Value) -> Vec<(GeminiFunctionCall, Value)> {
    vec![(
        GeminiFunctionCall {
            id: id.into(),
            name: "read_editor".into(),
            args: json!({"private":"not retained"}),
        },
        result,
    )]
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

/// Runs the calling test again in a child process so the module's global
/// installation slots cannot meet another test's.
fn isolated(test: &str, env: &str) -> bool {
    let name = format!("{}::{test}", module_path!().split_once("::").unwrap().1);
    crate::test_isolation::isolated_checked(&name, env, false, |_| {})
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

    if isolated(
        "the_real_dispatch_and_bind_boundaries_admit_only_the_expected_scope",
        "CODETRIAL_ACQUISITION_ARMING_TEST",
    ) {
        return;
    }
    let arming = Arming::new(limits());
    let installed = install_arming(&arming).unwrap();
    assert!(install_arming(&arming).is_err());

    let runtime = tokio::runtime::Builder::new_multi_thread()
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
    if isolated(
        "a_blocked_or_removed_observer_leaves_no_capture_installed",
        "CODETRIAL_ACQUISITION_ARMING_BLOCKED_TEST",
    ) {
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
    if isolated(
        "a_teardown_clears_only_its_own_dispatch_hook",
        "CODETRIAL_ACQUISITION_ARMING_HOOK_TEST",
    ) {
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
