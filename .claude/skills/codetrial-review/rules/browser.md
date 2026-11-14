# The browser pages

`web/*.js` and the pages that load them. There is no build step and the
candidate keeps clicking while the page awaits, so most defects here are about
state that changed during an await, a control left reachable in the wrong
phase, or an assistive-technology state that disagrees with the screen. The UI
rules
[codetrial-web](../../codetrial-web/SKILL.md#what-review-checks-in-a-ui-change)
already states are not repeated here, nor is the browser support rule in
[platform.md](../../codetrial-web/platform.md).

## Re-read state after every await, and fence async results with a generation

Applies to: `web/app.js`, `web/interview.js`, `web/history.js`,
`web/audio-output.js`, `web/avatar/*.js`

The lobby and interview pages await sign-in, `/api/session`, judges, PDF
parsing and `setSinkId`, while the candidate keeps clicking. Flag values
captured before an await and used after it (`checkedGrounding()` before
`recordGitHubLogin`, a global `problem.id` re-read after sign-in), reloads that
reuse the current `historyLoad` instead of bumping it, late judge callbacks
that switch language after `ending`, an older save resolving last and re-arming
a guard, an unmute in `.finally` on a routing request that lost to a newer one,
and multi-tab storage written from a stale in-memory copy. Also flag aborted
loaders whose cleanup runs after a new load starts (two WebGL contexts).

Seen: 12 PRs. A delete reload and a `pageshow` refresh share one generation,
and a stale `GET /api/reports` repaints deleted rows.

Not a finding: a read, map and write with no await in between. It cannot
interleave with another tab's save.

## Every phase change repaints control availability, and every handler checks the phase

Applies to: `web/interview.js`, `web/example-board.js`, `web/whiteboard.js`

`state.phase` moves through `live`, `ending`, `report` and `report_recovery`,
alongside pause and the behavioral round. Controls are enabled through
`updateRunAvailability()` and similar functions. Flag transitions that do not
call them (`endInterview`, `showReport`, `round_state`, pause): the overlays
are opaque but not `inert`, so Tab still reaches Run, Restore or Remove behind
them. Flag handlers with no phase guard (`removeCandidateCase` writes cases
back to `sessionStorage` after the end, `setLanguage` refuses `report` but not
`ending`, a Skip confirmed during `ending` before `end_interview` was
published). Flag controls enabled before `connect()` resolves, or End disabled
for 180 s while waiting for readiness in practice mode.

Seen: 9 PRs. Run stays enabled after the report, and plain Enter behind the
overlay starts a run.

Not a finding: a control that `codingClosed()` already disables, where the
handler's own early return is defense in depth.

## Move focus before disabling or hiding the focused control

Applies to: `web/*.js`

Disabling or hiding `document.activeElement` drops focus to `<body>`, so the
next Tab starts at the top of the page. Recurring triggers: font-size buttons
at the end of their range, Restore when the round changes, the Unclear button
hidden by the next caption (including when its parent bar times out), a picker
emptied from another tab, the last Undo or Clear on the board, Hide avatar.
Move focus to a sensible neighbour first, and check that the target can take
focus (the editor is removed in whiteboard mode and disabled during pauses).
The reverse also happens: a control that keeps focus after a mouse action
(splitter drag, Unclear click) swallows the candidate's next keystrokes.

Seen: 8 PRs. Hiding the focused Unclear button mid-caption sends focus to
`<body>`.

Not a finding: `aria-disabled` plus a guard in the handler, which keeps focus
on purpose.

## ARIA states must match what is on screen

Applies to: `web/*.js`, `web/*.html`, `src/web/setup.rs`

Flag these patterns: a button inside an `aria-live` region (its hide and unhide
are announced as content), a `role="status"` region that is `display:none`
until filled (screen readers skip the first message), `aria-pressed` combined
with a label that flips ("Dark mode, pressed" while light), `role="img"` over
focusable children, `aria-expanded` with no `aria-controls` target, status
lines announced on first load ("0/5 cases ready"), `aria-label`s that drop the
visible words (WCAG 2.5.3) or are identical on every row, reasons put in
`<option title>` (never shown), checkbox descriptions outside a `<label>`, and
live regions that read both platforms' chords.

Seen: 11 PRs. An editor theme toggle that announces the opposite of what is on
screen after the OS theme changes mid-interview.

Not a finding: `aria-labelledby` pointing at a visible heading instead of a new
label attribute.

## Keyboard handlers: IME, repeat, exact modifiers, scope

Applies to: `web/interview.js`, `web/app.js`, `web/lib.js`, `web/editor*.js`,
`web/splitter.js`

The page already checks `event.isComposing || event.keyCode === 229` in several
places (WebKit ends composition before the confirming keydown). New Enter,
Ctrl+Enter or undo handlers must do the same. Flag missing `event.repeat`
handling (a held chord starts repeated runs, a held Enter on Restore confirms a
two-press guard), modifier checks that accept extra modifiers, `preventDefault`
on `document` that takes a chord from unrelated fields (STAR textareas in the
report), matches on `event.key` that fail on non-Latin layouts (fall back to
`event.code` only for non-ASCII keys, not for punctuation on Dvorak), and
missing platform chords (Ctrl+Shift+Z on Linux).

Seen: 7 PRs. A splitter that takes Alt+Enter, so `isYieldShortcut` sees
`defaultPrevented` and the turn is never yielded.

Not a finding: a modifier combination that no browser or OS binds, noted only
as a nit.

## Browser storage: guard it, scope it, survive quota, don't latch suggestions

Applies to: `web/history.js`, `web/app.js`, `web/practice-insights.js`,
`web/audio-output.js`, `web/lib.js`

Reads and writes go through `readStored`/`writeStored` (try/catch).
`web/lib.js` stays DOM-free. Flag a quota failure that throws past the
review-store write and loses the whole attempt, a fallback that strips code
from the attempt that was just saved, and `setItem` inside a read. Flag keys
that are not scoped to the signed-in account (user B inherits user A's starts),
tabs that overwrite each other's lists, values restored from storage without
revalidation, and suggested values saved as if chosen (a suggested or
ceiling-clamped duration later latched into `manualDuration`).

Seen: 6 PRs. A fallback that removes `code` from every row, including the
attempt that triggered it, which brings back the "(final code was not saved)"
symptom.

Not a finding: a byte budget that is already enforced separately from the row
cap, with a test asserting the stored size.

## Lobby selection flags must be cleared on every path that invalidates them

Applies to: `web/app.js`, `web/problem-picker.js`, `web/problem-grid.js`

`manualProblem`, `keptDraw`, `searchVisibilityException` and `manualDuration`
decide whether `recommend()` runs and whether a hidden card can stay selected.
Flag a flag that is set and never cleared (one Random click leaves every later
`recommend()` uniform, which offers passed problems), filters that hide a card
but keep it selected (Start launches a problem the lobby no longer shows), and
a `null` problem that short-circuits `recommend()` and leaves Start dead. Walk
the bfcache `pageshow` restore, report deletion, sign-in merge, and filter
changes during the history load.

Seen: 6 PRs. Switching the status filter to "Passed before" with nothing
passed, then back to All, leaves Start disabled.

Not a finding: re-hiding a released draw when the comment and description say
it matches "Try again" behavior.

## Report Markdown export: escape, fence safely, keep raw language ids

Applies to: `web/render.js`, `web/download.js`, `web/app.js`,
`web/interview.js`

`reportMarkdown` sends every text field through `mdText` and fences code using
`longestRun` + 1 backticks. Flag new fields that skip this (test inputs and
program output are candidate-controlled). Flag hardcoded triple-backtick fences
around code, display labels ("C++", "not recorded") used as the fence info
string (sanitized to `notrecorded`, highlighting lost), `?? fallback` where
`""` also needs the fallback, an indented line after `- item` with no blank
line (CommonMark folds it into the paragraph), sections emitted when empty
(Test Run History in whiteboard sessions), and UTC `toISOString()` dates in
filenames next to local labels.

Seen: 5 PRs. A run snapshot fenced with three backticks, which a ``` in a code
comment closes early.

Not a finding: an export that omits a link the issue only asked for in the HTML
card, filed as a follow-up.

## Media devices: routing, aliases, and tracks that end without `ended`

Applies to: `web/interview.js`, `web/audio-output.js`, `web/audio-check.js`,
`web/devices.js`, `web/self-view.js`

Flag these: `track.attach` starts playback (and unmutes) before `setSinkId`
settles, so the first words come out of the default device. `enumerateDevices`
returns empty `deviceId`s before the microphone grant, so a saved device looks
missing and is erased. Chrome's `default` and `communications` rows are
aliases, so count physical inputs and do not count option rows (but group by
`groupId` carefully). `track.stop()` fires no `ended`, so integrity events and
self views have to be cleared on every stop path (offline fallback, stranded
switch). LiveKit's `isMuted` differs from the page's Mic state, so a muted
candidate's new input goes live during `replaceTrack`. Error classifiers that
only know Chromium's spellings (Firefox `AbortError`, `NotFoundError` on
unplug), and a stale `error` kept after re-enable.

Seen: 6 PRs. A muted candidate who switches microphones sends live audio until
the await settles.

Not a finding: a muted-slider value that cannot be told apart from a one-step
increase, handled by a documented rule.

## Editor heuristics must consult token context, and pairs must be consistent

Applies to: `web/editor.js`, `web/interview.js` (`applyEditorEdit`),
`web/tokenizer.js`, `web/syntax-parser.js`, `web/highlight.js`

Auto-indent, auto-pair, type-over and bracket matching read raw characters.
Flag raw-character logic that ignores comments and strings (`# Steps:` and `//
setup {` indent the next line, a `/*` inside a regex or raw string latches
comment state). Flag Python f-string interpolations treated as plain strings
(both by the grammar and by the scanner fallback used before the grammar is
ready). Flag substring tests where membership is needed (`"([{".includes(data)`
accepts `""` and `"(["`). Flag inserting a pair without the matching type-over
and Backspace pair-delete, or type-over that eats a closer the candidate typed
themselves. Flag synchronous throws from `prepareLanguage`/`tokenize` handled
with `.catch`.

Seen: 6 PRs. Typing `(` before `x` in `f(x)` and then `)` swallows the
keystroke and leaves `f((x)`.

Not a finding: a limitation stated in the PR description and tracked as a
follow-up. Ask for an issue, not a TODO.

## Keep per-keystroke and per-frame paths cheap

Applies to: `web/interview.js`, `web/editor.js`, `web/runners.js`,
`web/captions.js`, `web/example-board.js`

Flag a full `tokenize`, a whole-buffer comparison or an `innerHTML` swap on
every `input` or `selectionchange` (each keystroke paints twice). Flag
`JSON.stringify` of every judge input when only failing rows show it, preparing
every language's WASM up front (about 5.7 MB), re-enumerating devices on every
2 s retry, encoding a full JPEG every second while offline, and parsing every
Live frame (audio included) into a `Value` twice.

Seen: 7 PRs. A cache keyed on `code` that misses on every keystroke, so bracket
matching reparses the buffer on each one.

Not a finding: a cache with an explicit invalidation that a browser test shows
is reused on caret moves.

## Failure text must name the real cause and offer an action that works

Applies to: `web/*.js`, `src/web/token.rs`, `src/livekit/report.rs`

Flag messages that send the candidate somewhere they cannot go: "test again"
with no test-again button, "Try again" after a media element is stuck on
`MEDIA_ERR_SRC_NOT_SUPPORTED`, "Retry media preflight" for a Strict refusal
that comes back unchanged, "could not read your PDF" when the vendored PDF.js
(`web/vendor/pdfjs/FETCH`) was never fetched, a 403 naming guest mode to an
operator who never enabled it, a partial PDF read reported as "Parsed locally",
and blocked multi-file downloads with no hint about how to recover.

Seen: 7 PRs. A camera-busy hint that asks the candidate to tick a box which
also drops the integrity worker and later gets cleared by Skip.

Not a finding: wording that follows a maintainer's explicit request (clear a
refusal note only after a successful route).
