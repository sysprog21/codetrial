# Review the changed user flow

The accessibility, theme and key-handling rules reviews keep raising are in
[What review checks in a UI change](SKILL.md#what-review-checks-in-a-ui-change);
this file adds the walk through a flow. Check only the controls and states the
change touches. A finding is evidence for a proposed fix, not permission to
redesign the page.

- Keyboard: Tab and Shift+Tab reach every control, a composite one such as a
  radio group as a single stop whose items the arrow keys move between, and
  each activates with its native keys; hidden or inert content takes no focus.
  The editor splitter's drag has a keyboard alternative.
- Semantics: actions are buttons and navigation is links. Icon buttons have an
  accessible name and inputs a real label, not only a placeholder. Decorative
  icons are hidden from assistive technology.
- Forms: input accepts paste and submits by keyboard. A failed submission keeps
  what was entered, ties the error to its field and moves focus to the first
  invalid one.
- Async states: walk idle, loading, success, empty and error, and the retry after
  an error. Pending work shows feedback and blocks a duplicate submission; a
  status change is announced without taking focus; an empty result does not look
  like an unfinished load; error text names a next step.
- Layout: narrow widths, zoom, long labels and reduced motion. Ask what a
  pointer or finger would hit (`document.elementFromPoint` at the control's
  centre) rather than where its rect says it is: a parent's `overflow` clips a
  menu without moving its box, and a sticky header or on-screen keyboard covers
  a control that still reports a visible position.

The CodeTrial transitions to walk when the change touches them: lobby account
and history loading, microphone and camera preflight, entering the interview,
reconnecting, ending the session, and showing or deleting a saved report. A
pending login or Start keeps the controls until it completes, and an error
releases them for a retry without losing the chosen problem, language or
interview settings. During an interview, editor shortcuts, captions and media
controls stay usable together. Deleting a report moves focus to the row that
took its place, or to the status line when none remain, and announces the
result.

Inspect the rendered DOM and accessibility tree alongside screenshots, with
synthetic interview content. For each failure record the route, viewport and
theme, action and observed result so a browser check can reproduce it; keeping
screenshots and console output is in
[Browser investigation](../codetrial-verify/SKILL.md#browser-investigation).
