/// Whether a scroll box rests at its end, and whether the candidate moved it.
///
/// Shared by the caption bar and the Transcript tab, so both follow new text
/// by one rule. Nothing here touches the page: each check reads the geometry
/// of the box it is given, so a test can pass a plain object.

// Slack for the fractional scrollTop that zoomed layouts report against whole
// pixels of scrollHeight and clientHeight.
const SCROLL_END_SLACK_PX = 2;

export function atScrollEnd(box) {
  return (
    box.scrollTop + box.clientHeight >= box.scrollHeight - SCROLL_END_SLACK_PX
  );
}

/// The browser moves a box on its own only by clamping it into a range that
/// shrank, which leaves it at the smaller of where it was and the new bottom:
/// a box that shrank under a follow stays put, a box that grew taller under a
/// candidate near the end lands on the new bottom, and a hidden box, whose
/// range is zero, sits at the top. Measured from `seenTop`, where the box was
/// last seen resting, anything else is the candidate scrolling.
export function scrolledByCandidate(box, seenTop) {
  const bottom = Math.max(0, box.scrollHeight - box.clientHeight);
  // Under zoom scrollTop is fractional while the bottom is whole pixels.
  return (
    Math.abs(box.scrollTop - Math.min(seenTop, bottom)) > SCROLL_END_SLACK_PX
  );
}
