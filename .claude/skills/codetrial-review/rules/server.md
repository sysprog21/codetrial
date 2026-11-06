# The web server and Rust code in general

`src/web/`, the token and dispatch entry points, and any Rust file. The web
side can run without the agent on the same host, and every field the browser
sends may come from something other than the shipped page.

## Treat browser-supplied fields as untrusted at every sink

Applies to: `src/agent/events.rs`, `src/livekit/*.rs`, `src/agent/prompts.rs`,
`src/agent/prompts/*.rs`, `web/render.js`, `web/lib.js`, `src/web/mod.rs`

Data-channel payloads, board streams and saved reports come from a client that
may not be the shipped page. Check each new sink: `eprintln!` lines with
`key=value` fields (a reason with a space or newline forges further `codetrial`
lines), prompts (candidate handwriting on a board needs the same
`untrusted_note` treatment as fenced code), Markdown export (unescaped
`input`/`got`/`error` renders remote images), object lookups
(`labels[value]` on a plain object walks the prototype for `"constructor"`), and size budgets
(`MAX_REPORT_BYTES` 64 KB is only safe because `sanitizeReport` bounds every
field, and an unbounded new field breaks it).

Seen: 6 PRs. An `end` reason logged raw, unlike the `code`/`language` fields
next to it.

Not a finding: a candidate-claimed count (board strokes) that only lifts a
refusal and does not record evidence. That is not a trust boundary.

## Web server: cover web-only deployments, guests, and every dispatcher

Applies to: `src/web/token.rs`, `src/web/mod.rs`, `src/web/auth.rs`,
`src/web/setup.rs`, `src/dispatch.rs`

`main.rs` serves the web side alone when there is no `GOOGLE_API_KEY`. In that
case `state.dispatcher` is `None` and the agent runs on another host with its
own environment. Flag limits enforced only in `LocalDispatcher` (the two-guest
ceiling), flags that `/api/session` reads from the web process's own
environment (`candidateVideo`), and rate-limit buckets that let a caller drop
the cookie for a second budget or let NAT neighbours lock out signed-in users.
Predicates such as `guest_mode_available` belong on `AppState` with one
definition (one now exists in `src/web/mod.rs`). Setup writes secrets: check
the file mode (0600, `create_new`, no symlink follow) and `Host` (a loopback
bind does not stop DNS rebinding).

Seen: 4 PRs. Guests bucketed by the same IP with the same size, which doubles
the per-IP ceiling.

Not a finding: symlink and ACL hardening inside the user's own data directory,
where anyone able to plant a file could also write `SESSION_SECRET` to
`codetrial.env.local`.

## Rust hygiene: no dead seams, no widened visibility, keep doc comments attached

Applies to: `src/**/*.rs`

Flag parameters that every caller passes as `None` or ignores (`_mode`, an
`image` argument), `pub` added only to get past `dead_code` (use `pub(crate)`
and an allow, as `src/web/mod.rs` does for `background`), `pub` single-key
entry points left with no callers, "unreachable" 500s that exist because three
`Option`s are meant to move together (keep `Option<Owner>`), and `?` on a check
the handler already enforced with a 400. Flag an item inserted between an
existing `///` comment and the item it documents: rustdoc then attaches the
comment to the new item (`NoShowConsent` above `RoomDispatcher`, a helper
inserted between `resend` and its doc).

Seen: 9 PRs. A struct inserted below the trait's doc comment, so the trait
loses its "must be idempotent per room" contract.

Not a finding: a test-visible read-only constant that pins a boundary to the
constant itself.
