// Included in the session test module so existing test paths stay unchanged.

#[test]
fn real_tool_dispatch_records_only_tools_before_reply_audio() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    let mut state = RuntimeState {
        code: "return 1;".into(),
        ..RuntimeState::default()
    };
    let call = GeminiFunctionCall {
        id: "read-1".into(),
        name: TOOL_READ_EDITOR.into(),
        args: serde_json::json!({}),
    };
    activity.note_candidate_finished(start, false);
    let response = execute_timed_tool_batch(
        &mut state,
        &mut activity,
        vec![call.clone(), call.clone()],
        0,
        true,
        &mut None,
    );
    assert!(
        response[0].1["result"]
            .as_str()
            .is_some_and(|code| code.contains("return 1;"))
    );
    let first = activity
        .turn_timing
        .note_audio(start + Duration::from_secs(1), true)
        .unwrap();
    assert_eq!(first.tool_calls, 2);
    assert_eq!(first.tool_wait, Some(first.tool_exec));
    assert!(first.pre_tool.is_some());
    execute_timed_tool_batch(
        &mut state,
        &mut activity,
        vec![call.clone(), call.clone()],
        0,
        true,
        &mut None,
    );
    assert!(
        activity
            .turn_timing
            .finish(start + Duration::from_secs(2), TimingOutcome::NoAudio)
            .is_none()
    );
    activity.note_candidate_finished(start + Duration::from_secs(3), false);
    execute_timed_tool_batch(&mut state, &mut activity, vec![call], 0, false, &mut None);
    let next = activity
        .turn_timing
        .note_audio(start + Duration::from_secs(4), true)
        .unwrap();
    assert_eq!(
        next.tool_calls, 0,
        "discarded/held tool responses must not be carried forward"
    );
}
