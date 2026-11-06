# Writing Rust in CodeTrial

The build owns what a machine can ask; this page owns what it cannot. Fix a
finding rather than silence it, and design before polishing: rustfmt, clippy and
the compiler already spend their attention on the rest.

## What the build enforces

`./scripts/test.sh` runs both, and a bare `cargo clippy --all-targets` agrees
with it because the levels live in `Cargo.toml`, not on a command line. Every
workspace package, including the Rust hook checker, inherits them through
`[lints] workspace = true`. The Rust lane's `workspace-lints` gate checks actual
Cargo membership, including globs and implicit path dependencies, and refuses
an opt-out. The portable source check needs no Cargo metadata.

| Owner | Refuses |
|---|---|
| `[workspace.lints.rustdoc]` | broken documentation links and public links to private items |
| `[workspace.lints.rust]` | `unsafe` (forbid), `pub` items nothing outside the crate can name, unused lifetimes, ignored must-use results and stale lint expectations |
| `[workspace.lints.clippy]` | `unwrap`, `panic!`, `todo!`, `dbg!`, `allow` attributes and any suppression without a `reason`, `.clone()` on an `Arc`/`Rc`, redundant clones, `+` on strings, a function with more than three bool parameters, needless `Option`/`Result` wrapping, a panic inside a function returning `Result`, glob imports, a missing `;` after a unit expression, `as` casts that drop a sign, synchronous guards across await, discarded futures and `mem::forget` |
| `scripts/check-rust-style.py` | `#[allow]`, including inside `cfg_attr`, and `#![deny(warnings)]`; `#[ignore]` without a reason; `.expect("")`; TODO, FIXME, XXX, HACK, `ponytail:` and the untracked working documents named in a `//`, `///` or `/* */` comment; CJK in Rust or `web/`; a file past its ceiling; a lint in the required set loosened in `Cargo.toml` |

`tests/test_check_rust_style.py` holds the checker to each rule. The required
set in the checker and the table in `Cargo.toml` change together, and the test
fails when only one does.

These lints are off on purpose, so do not add them back to make a report look
fuller:

- `missing_debug_implementations`: a type holding a credential has no `Debug`
  so that no log line can print it, and its doc comment says so
  (`AgentConfig`, `GitHubOauth`, `TokenConfig`). The lint asks for the derive
  that policy forbids. Redact by hand with `impl fmt::Debug` only when a type
  must be printable and holds a secret, as `Provider` does.
- `needless_pass_by_value` and `unused_async`: they flag `Value`/`Response`
  builders with dozens of callers and event handlers that share one `async`
  shape. The churn buys no clarity.
- `indexing_slicing`, `as_conversions`, `print_stderr`: the server's logs are
  `eprintln!` and its hot paths index; a blanket ban would be waived everywhere
  and mean nothing.

## Suppressing a lint

`#[expect(clippy::name, reason = "why")]` on the narrowest item. The reason
names the constraint, not the lint ("responses are immediately returned by HTTP
handlers"). `expect` fails the build once the lint stops firing, so a stale
waiver cannot outlive its cause. `allow` is refused outright.

A target-dependent waiver is `#[cfg_attr(not(test), expect(..., reason = ".."))]`,
because an `expect` that holds in one build and not the other is itself a
finding. Integration test crates open with one `#![expect(clippy::unwrap_used,
reason = ...)]` naming only the lints that file uses; unit tests get the same
through `clippy.toml`.

## File size

New Rust files stay within 1500 lines. Existing larger files are recorded in
`scripts/rust-style-ceilings.json` and may only shrink: after splitting one by
responsibility, run `scripts/check-rust-style.py --ratchet` to lower its entry.
`scripts/ceiling-hotspots.py` ranks them by churn times size, as one input to
choosing which to split first. Count a file's mutants before splitting it with
`cargo mutants --list -f <file> | wc -l`; `make mutants` downloads a compiler
first. Never raise an entry to land a change; a new module is the answer. The
gate passes `--baseline origin/main`, so a raised entry or a new exemption fails
against what main records, and under CI a baseline ref that was not fetched
fails too, so a shallow checkout cannot skip it. Test bodies live under
`tests/unit/` and answer to the same ceiling.

### Splitting a file

What splitting `src/agent/prompts.rs` (3944 lines, six modules under
`src/agent/prompts/`, commit f122e38) needed, in order:

1. **Plan before moving.** Choose the file with `scripts/ceiling-hotspots.py` and
   a look at the open pull requests (`gh pr list --state open --json
   number,files`), list every top-level item with its line span, and group by
   responsibility. A dependency order between the new modules with no cycle is
   the test of a good cut.
2. **Keep the old file as the facade.** It keeps its `#[path]` test module and
   gains `mod x;` plus `pub use x::{...}` and `pub(crate) use` lines with the
   same names and visibility the parent re-exported, so no caller changes. This
   is the layout `src/livekit.rs` beside `src/livekit/` already has.
3. **Move by line span, never by retyping**, and prove it: after `cargo fmt`,
   compare each original item with the new files ignoring whitespace, trailing
   commas, `pub(...)` and the path rewrite below. Expect only small differences:
   rustfmt wrapping a closure body in braces, and a name imported under step 4
   losing its path.
4. **Rewrite `super::` in bodies to `crate::agent::`**, since `super` is now the
   facade, then import the names used three or more times so the code reads as
   before. Widen a private item to `pub(super)` only where a sibling needs it.
5. **Tests that reach private items** through `use super::*` keep working if the
   facade carries a `#[cfg(test)] use self::{...}` group naming them, and the
   `use super::...` of the parent's own types they read; the test file is not
   edited. Group the facade's re-exports by visibility (`pub use`, then
   `pub(crate) use`), as the parent re-exported them.
6. **Look for what names the file.** `git grep -n 'path/to/file'` found a test
   that hashes the file with `include_bytes!` (a digest that would silently stop
   changing when a prompt did), the `Applies to` lines of two review rules, a
   skill doc line, and comments in Rust and in `web/`. A name inside a format string (`{NAME}`) is a compile error if
   its import is missed, which is the safe way to fail.
7. **Run `scripts/check-rust-style.py --ratchet`** so the old entry leaves
   `scripts/rust-style-ceilings.json`, then clippy at `-D warnings`, then
   `./scripts/test.sh rust`.

A comment added to a file that sits exactly at its ceiling fails the checker, so
a note for a ceilinged file goes here or in the split, not in the file.

## Judgment, which no gate checks

Review for these, in this order.

1. Make invalid states unrepresentable. An enum beats a flag plus a comment
   saying "only meaningful when"; a newtype beats a bare `u64` that could be
   seconds or milliseconds; a fallible constructor validates once.
2. Own, then borrow. Take `&str`, `&[T]`, `&Path`, never `&String` or `&Vec<T>`;
   take a value when the function keeps it. A clone that exists to end a borrow
   is a design question: try `mem::take`, a narrower scope, or splitting the
   struct before cloning.
3. Errors carry a cause. Absence is `Option`, a failure the caller can act on is
   a `Result` with a typed error, an invariant is `expect("why it cannot fail")`.
   A `Display` message is lowercase with no trailing period. Never panic on
   input that arrived over the wire.
4. One function, one job, three indentation levels. Early-return with
   `let ... else`; extract the arm that grew a second purpose.
5. Name by role: no `Manager`, `Helper` or `Util`; a getter is `name()`, not
   `get_name()`; `as_` is free, `to_` allocates, `into_` consumes.
6. Visibility is a promise. `pub(crate)` before `pub`; a field is private unless
   the struct is plain data.
7. Locks and `.await`. No `std::sync` guard alive across an await; keep a
   critical section to a few lines and call no closure inside it.
8. Bound work at input boundaries: payload sizes, retries and queue growth.
   A bounded channel needs an explicit full-queue policy; dropping a message or
   awaiting capacity can change ordering or stall the room loop. Track spawned
   tasks through shutdown, and check cancellation between dequeue and commit.
9. Preserve stored keys, wire names, missing-field errors and report goldens.
   A style refactor does not authorize a contract migration or a new dependency.
10. Measure a representative workload before changing allocation or dispatch
    for speed. Clones inside a loop are leads, not proof of expensive copying.
11. A comment explains why. See codetrial-conventions for register and length.

Provider discovery and room routing live in `src/config/providers.rs`;
`src/config.rs` re-exports their public API. Diagnostics print only hosts or
endpoint authorities, because a configured URL can contain credentials in its
userinfo, path, query or fragment.

## Idioms this tree already follows

Worked before-and-after cases, including the ones deliberately left alone, are
in [recipes/rust.md](recipes/rust.md).

- `Arc::clone(&x)`, so a refcount bump reads differently from a deep copy. Where
  the target is a trait object, name the source type so the coercion happens at
  the call: `Arc::<FakeProvider>::clone(&provider)`. An `as Arc<dyn T>` cast is
  not an option; it is a trivial cast.
- A `std::sync::Mutex` is locked with `.lock().unwrap_or_else(|error|
  error.into_inner())`, not `.unwrap()`: a poisoned lock is recovered. All 14
  such locks were checked (2026-11-06) and each guards state that is written
  whole, so the next taker sees a valid value, and a `Drop` that takes a lock
  must not panic while a task unwinds. Where the reason is not obvious (the
  accounts connection, the dispatch maps, the cooldown map, the pressure
  counters, the task admission state) a comment at the declaration gives it. A
  new lock gets the same question: if a panic halfway through an update would
  leave its state inconsistent, recovering is wrong, and the update should be
  one step or the lock should not be recovered.
- Casts that can lose information use `try_from`, `cast_unsigned` or a clamp
  first. A cast that is correct by construction carries an `expect` saying why.
- A glob import is a re-export hub that its test module reads through
  `use super::*`; everywhere else, name what you import.
- Test bodies are not in `src/`: declare `#[cfg(test)] #[path = ...] mod tests;`
  and put the body under `tests/unit/`. Assert on behavior a caller can observe,
  and give an assertion a message when two of them in a row would fail alike.
- Pause Tokio time only when the test controls every awaited event. A real
  local socket can lose a race to an auto-advanced connection timeout; complete
  its setup before pausing, or pass explicit instants to the timer logic.
- Fixtures that need multi-byte text use Greek or emoji, never CJK.

## Before handing back Rust

```sh
cargo fmt
cargo clippy --locked --workspace --all-targets
cargo doc --locked --workspace --no-deps
python3 scripts/check-rust-style.py
python3 scripts/check-rust-style.py --workspace
cargo test --locked <one filter>
```

Then `./scripts/test.sh` per codetrial-verify. Say which of these ran; "clippy is
clean" without the checker is half a report.

## Reference synthesis and audit limits

The six reference projects under externals inform this policy; they are
reference material, not additional installed skills or required CI tools.

| Reference | Adopted here | Deliberately not adopted |
|---|---|---|
| rustgrade | Verify scanner hits against callers; inspect queue growth and lock lifetimes | A numeric grade as proof of safety; blanket bans on loop allocations |
| BeautifulRust | Manifest-owned lints; narrow, reasoned expectations that expire | Blanket pedantic, Debug or documentation derives for application internals |
| rust-style-guide-audit | Distinguish application guidance from library contracts; reproduce every finding | Chrome-specific rules; treating text matches as type-aware analysis |
| rust-engineering | Domain boundaries, locality, cancellation and compatibility before polish | Traits, wrappers or dependencies added solely to satisfy a pattern |
| opinionated-rust-skills | Name errors for invalid conditions; keep defaults near their fields | A new default-annotation crate or macros for a single validation |
| rust-patterns | Validate at boundaries; explicit ownership and failure values | Type-state, static dispatch or actors without a concrete need |

The Python style checker scans text, not expanded Rust syntax. Multiline
spellings are checked, but strings, comments and macro expansion can defeat a
text rule. Clippy provides compiler-aware checks; neither tool establishes
shutdown correctness, secret redaction, bounded memory or behavioral coverage.
Keep those in the judgment pass and cite an observable regression when fixing
one. A passing gate is evidence for its checks, not a production certification.
