use super::*;
use crate::config::GeminiContextCompression;
use std::time::Duration;

fn state() -> RuntimeState {
    RuntimeState {
        context_compression: Some(GeminiContextCompression {
            trigger_tokens: 30_000,
            target_tokens: 8_000,
        }),
        ..RuntimeState::default()
    }
}

/// Two turns, the second far smaller: the provider cut the context.
fn cut(activity: &mut RuntimeActivity, state: &mut RuntimeState) {
    for prompt in [20_000, 8_000] {
        activity
            .tokens
            .observe_prompt_tokens(state.context_compression, prompt);
        activity.observe_turn_complete(state);
    }
}

#[test]
fn a_checkpoint_waits_for_a_cut_and_for_the_gates_to_open() {
    let now = Instant::now();
    let mut state = state();
    let mut activity = RuntimeActivity::new(now);
    let quiet = now + Duration::from_secs(600);

    assert!(!activity.context_checkpoint_due(&state, quiet, || panic!("nothing is pending")));
    cut(&mut activity, &mut state);
    assert!(activity.context_checkpoint_due(&state, quiet, || (false, false)));
    assert!(!activity.context_checkpoint_due(&state, quiet, || (true, false)));
    assert!(!activity.context_checkpoint_due(&state, quiet, || (false, true)));
}

#[test]
fn a_checkpoint_that_went_out_is_no_longer_owed_and_the_cut_is_read_again() {
    let now = Instant::now();
    let mut state = state();
    let mut activity = RuntimeActivity::new(now);
    cut(&mut activity, &mut state);
    assert!(activity.tokens.context_refresh_needed());
    assert!(state.context_cut_plausible);

    activity.context_refreshed(&mut state);
    assert!(!activity.tokens.context_refresh_needed());
    assert!(
        !state.context_cut_plausible,
        "the context sits near the target"
    );
}
