# Choosing a web platform feature

`web/` has no transpiler, no polyfill layer and no bundler, so the API a change
calls is the whole browser compatibility story: nothing downstream rewrites it
for an older engine. This page is the rule for choosing one, the choices the
tree has already made, and how to consult Modern Web Guidance, the curated
guide set the Chrome, Edge, Firefox and MDN teams publish for exactly this
decision. The tokens, theme and accessibility rules stay in
[SKILL.md](SKILL.md#what-review-checks-in-a-ui-change).

## Which browsers

What the tree commits to today:

- Task mode admits desktop Chromium and Firefox only (`docs/task-mode.md`).
  The page reports its own check to the server, and that check in
  `web/task-room.js` is the one user-agent read the tree allows: an admission
  policy, not feature detection.
- The lobby, the interview and peer pages name no browser and keep Safari
  paths, so treat them as serving Chromium, Firefox and Safari.

Until a document states a narrower target, the rule is the Baseline one Modern
Web Guidance assumes:

- Baseline Widely available: use it directly, with no fallback and no vendor
  prefix. `webkitAudioContext` was removed for this reason, and every audio
  context in `web/` is built unprefixed.
- Baseline Newly available, or Limited availability: feature-detect, and write
  the fallback here in a few lines so the feature degrades to something that
  still works. Never a polyfill and never a library dropped into `web/vendor/`
  (the vendored-bytes rule in [SKILL.md](SKILL.md#vendored-assets)). Say in the
  pull request what a browser without the feature shows.
- The exception is a CSS declaration or an HTML attribute value an engine
  without it treats harmlessly: it is its own fallback, with no `@supports` or
  script check. Scroll containment on the task page's dialog is one, since
  Safari skips it and `showModal()` still keeps the page inert; so is
  `hidden="until-found"`, which an engine without it reads as plain `hidden`
  while the picker's own toggle still opens the grid.

Widely available means 30 months after the feature became Baseline in all
engines. Read the status from the guide's "Baseline status for" or "Browser
support for" line, or from webstatus.dev; do not judge it from memory, since
training data lags the platform.

## What the gate holds

`tests/browser/platform.test.js` holds the rules a text scan can hold without
guessing at intent, over `web/` and the pages `src/` builds as strings; its
test names list them. Everything else on this page is held by review.

## Consulting Modern Web Guidance

Before writing a UI primitive (an overlay, a menu, a form field, a scroll or
motion effect), or client code that formats dates, sanitizes markup or runs a
long task, search the guides for the capability in two to five words, then
retrieve the matching ids:

```sh
npx -y modern-web-guidance@latest search "light dismiss dialog"
npx -y modern-web-guidance@latest retrieve "dialog,prevent-page-scroll-under-modal-dialog"
```

Both need network. A maintainer may keep a checkout in the untracked
`externals/modern-web-guidance/`; its `skills/modern-web-guidance/guides/`
holds the same guides as Markdown, one directory per discipline, and `grep -rl`
there answers without fetching. Skip the guides for Rust, CI, tests, wire
fixtures and the problem bank.

The guides are written in plain HTML, CSS and JavaScript, which is what `web/`
is, so a pattern usually applies as written. A guide does not override this
repository: the role tokens in `web/theme.css`, the no-build rule, the vendored
bytes and the review rules in [SKILL.md](SKILL.md) win where they disagree, and
the disagreement is worth raising rather than resolving silently.

The package's `legacy-scan` is for a deliberate modernization sweep, not a
feature change, and stays out of the gate because it needs network. It covers about a fifth of the guides, and each finding has to
be checked against its `keepWhen` and `dropWhen` rubric: a kept fallback, a
vendored file or a policy gate still matches and is not a finding.

## What the tree already does

Keep these; a change that replaces one needs a reason the guide does not give.

| Need | Here | Guide |
|---|---|---|
| Confirm before an irreversible step | Native `<dialog>` opened with `showModal()`; `web/task.css` holds scroll with `overscroll-behavior` on `dialog:modal` and on `::backdrop`, a progressive enhancement (Limited availability, not detectable) | `dialog`, `prevent-page-scroll-under-modal-dialog` |
| Enter acts in a field | `event.isComposing` and `event.keyCode === 229`: WebKit delivers `compositionend` before the confirming keydown | `ime-safe-enter-submit` |
| Remote text into the page | `textContent`, or markup built with `escapeHtml` from `web/lib.js` on every interpolated value; the Sanitizer API (`setHTML`) is Limited availability, so nothing relies on it | `sanitize-untrusted-html` |
| Collapsed problem grid still found by find-in-page | `hidden="until-found"` and `beforematch` in `web/problem-grid.js`; Limited availability, and where unsupported it is plain `hidden` and the picker's own `<details>` toggle still opens it | `search-hidden-content` |
| A viewport-tall layout | `dvh`, never `vh`: the mobile address bar changes the visible height | `responsive-design` |
| The editor textareas (`#editor`, `#code`) show no scrollbar | `scrollbar-width: none` beside `::-webkit-scrollbar` for Safari before 18.2 | `customize-scrollbar-color-and-thickness` |
| Messages between contexts | `postMessage` only to Workers and AudioWorklets the page starts itself; a `message` listener on `window` would need an `event.origin` check first | `validate-origins` |
| A request that must not hang | `AbortSignal.timeout()` | |

## In new code

- A new modal is a `<dialog>` opened with `showModal()`, without `role`,
  `aria-modal`, a focus trap, `inert` toggling or an Escape handler: the
  element provides them. The interview page's `#audio-check`,
  `#ending-overlay` and `#report-modal` are older `role="dialog"` sections
  whose `inert` and focus `web/interview.js` manages by hand, and
  `#ending-overlay` is deliberately not modal so Jim's captions stay announced.
  Moving them to `<dialog>` is its own change with its own browser test.
- A menu, tooltip or toast is a `popover`, not a `<dialog>`. Popover is Newly
  available (2025-01) and invoker commands (`command`, `commandfor`) Newly
  available (2025-12), so either needs the guide's fallback. Anchor
  positioning, interest invokers, `<dialog closedby>`, `popover="hint"` and
  the Scheduler API are Limited availability: detect them and never make them
  the only path.
- Long work on the main thread, such as highlighting a large buffer,
  yields with `globalThis.scheduler?.yield?.()`, falling back to a
  `setTimeout(resolve, 0)` promise where it is missing (`break-up-long-tasks`).
- A field styled as invalid uses `:user-invalid`, which waits for the user to
  interact; `:invalid` marks a required field red before anyone types.
- Every new animation or transition joins its sheet's
  `prefers-reduced-motion` block (`web/styles.css`, `web/task.css`).
- Detect a feature with `in` or a property check, never by user agent. The
  `navigator.platform` test in `web/interview.js` chooses Cmd or Ctrl, both
  in shortcut labels and in which modifier the undo and redo keys accept; it
  stays because its replacement, `navigator.userAgentData`, is Chromium-only.
- Dates, times and numbers shown to a person go through `Intl` rather than
  hand-built strings; `Intl.DurationFormat` is Newly available (2025-03).

When a guide decided a non-obvious choice, such as a fallback, an override or
a property that only works in a pair, name its id and the rule it states in the
pull request, so a reviewer checks the choice against the guide rather than
against memory. The code comment states the reason itself, not the guide.
