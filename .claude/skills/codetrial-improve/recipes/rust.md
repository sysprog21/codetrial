# Rust recipes

Each recipe is a smell, the change, and the proof. A recipe that ends "leave it"
is as much a result as one that rewrites. Run the gates in [rust.md](../rust.md)
before handing back.

## Take `Option<&str>`, not `Option<&String>`

A parameter typed `Option<&String>` names the owning type, so a caller holding a
`&str` has to allocate before calling. Take `Option<&str>` instead and let
callers pass `.as_deref()`; it accepts both.

- **Proof:** `rg -n '<function name>' .` lists every caller; clippy at
  `-D warnings` is clean; the tests that reach the changed function pass before
  and after. A branch no test reaches needs a new test first.

## Leave a return type that other closures share

`pending_candidate_line` in `src/agent/prompts/recovery.rs` returns `Option<&String>`.
Returning `Option<&str>` looks like the same recipe and is not. Its caller in
`src/livekit/session.rs` applies one `missing` closure to this value and to
`first_candidate`, which would both have to change, and the re-export in
`src/agent.rs` and the assertion in `tests/unit/agent/mod.rs` with them. The cost
is in the `opening` closure there: it calls `std::slice::from_ref(line)` to
build the `&[String]` that `transcript_tail` takes, so with a `&str` it would
have to allocate `[line.to_owned()]` on a path that allocates nothing today. A
type-level tidy-up that adds an allocation is not a win. Leave it.

## Leave a `&Option<T>` closure parameter that only calls `as_deref`

`credential` in `src/web/auth.rs` and `oauth_credential` in `src/web/mod.rs`
take `&Option<String>` and call `.as_deref()` first. Taking `Option<&str>` moves
that call to each of the four callers, which adds code at every site for no
saving, and a closure is local to one function. Leave them.

## Give a leaked field an owner method, not a getter

`field-leak` on `RuntimeActivity::last_code_change` named four places that wrote
it. Replace the write with a method on the owner that does the whole step:
`stamp_packet_effects(&result, at)` stamps the three clocks a data packet's
reduction says it moved, and `note_board_snapshot(recorded, now)` restarts the
idle count and the clock together. A `last_code_change()` getter would have
quieted the tool and left every caller knowing the field.

- **Proof:** the tool's report loses the entry; clippy at `-D warnings` is
  clean; the tests that reach the callers pass. Where only a one-field setter is
  left (`live_exit`), keep the entry and say why.

## Break a module cycle at the type, not the use

Moving the tasks' `Hints` out of `record.rs` into `hints.rs` removed an
`attempt` and `record` cycle; the first attempt had put it in the module that
needed it. Likewise `Ingress` and `CandidateStreams` live in `livekit/streams.rs`
because `media` already uses three items of `turn`, so `turn` could not name a
`media` type. Decide the layer first, then move the type to the leaf both sides
may depend on. Passing a value in (a deadline instead of importing its constant
from `side_calls`) cuts an edge without a new module.

- **Proof:** a second run of the tool shows no new `module-cycle`; a cycle that
  moved elsewhere is not fixed.

## Leave a forwarder the tool accepts

`EvidenceLedger::entries.is_empty()` and `metrics.cost_line()` could be hidden
behind `has_entries()` and `cost_line()`, and the tool stops reporting them.
Neither hides a rule; each only passes the field through. Leave the entry and
name it in the PR, so the next reader sees the list is honest.

## Recipes to add when a site is found

Not written until the tree has a real site, so each one stays a true story:

- a function returning `Vec<T>` that every caller iterates and may stop early,
  to `impl Iterator<Item = T>`;
- a bare `u16` or `u64` that is a port or a duration, to `NonZeroU16` or
  `Duration` (`rg 'port: u16' .` finds the first);
- free functions over one collection, to methods on a newtype.

A `bool` parameter becomes an enum when a call site stops being readable; the
build only refuses more than three (`clippy::fn_params_excessive_bools`).
