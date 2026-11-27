//! The `tests` module of `src/livekit/turn/agent_state.rs`, which declares this
//! file
//! by path.

use super::*;

#[test]
fn agent_state_attributes_preserve_existing_values() {
    let attributes = agent_state_attributes(
        HashMap::from([("role".to_string(), "interviewer".to_string())]),
        AGENT_STATE_SPEAKING,
    );

    assert_eq!(attributes.get("role"), Some(&"interviewer".to_string()));
    assert_eq!(
        attributes.get(LIVEKIT_AGENT_STATE),
        Some(&AGENT_STATE_SPEAKING.to_string())
    );
}

#[test]
fn the_turn_window_rides_beside_the_agent_state() {
    let attributes = turn_window_attributes(
        agent_state_attributes(HashMap::new(), AGENT_STATE_LISTENING),
        3_000,
    );
    assert_eq!(attributes[TURN_WINDOW_ATTRIBUTE], "3000");
    assert_eq!(attributes[LIVEKIT_AGENT_STATE], AGENT_STATE_LISTENING);
    let attributes = agent_state_attributes(attributes, AGENT_STATE_SPEAKING);
    assert_eq!(attributes[TURN_WINDOW_ATTRIBUTE], "3000");
}

#[test]
fn the_page_reads_the_turn_window_under_the_same_key() {
    let page = std::fs::read_to_string("web/lib.js").expect("the page is readable");
    assert!(
        page.contains(&format!(
            "export const TURN_WINDOW_ATTRIBUTE = \"{TURN_WINDOW_ATTRIBUTE}\";"
        )),
        "web/lib.js must name {TURN_WINDOW_ATTRIBUTE}"
    );
}
