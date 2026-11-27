// Included in the livekit test module so existing test paths stay unchanged.

/// `RuntimeActivity::accept_event` with the turn timing read the room loop
/// takes from it.
pub(super) fn accept_timed(
    event: &GeminiEvent,
    activity: &mut RuntimeActivity,
    probes: &mut TurnProbes,
    held: bool,
    now: Instant,
) -> bool {
    super::session::accept_event_noting_silence(event, activity, probes, held, now).0
}
