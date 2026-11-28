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
| `[workspace.lints.clippy]` | `unwrap`, `panic!`, `todo!`, `dbg!`, `allow` attributes and any suppression without a `reason`, `.clone()` on an `Arc`/`Rc`, redundant clones, `+` on strings, a function with more than three bool parameters, needless `Option`/`Result` wrapping, a panic inside a function returning `Result`, glob imports, a missing `;` after a unit expression, `as` casts that drop a sign or widen losslessly (use `From`), synchronous guards across await, discarded futures, `mem::forget`, `unimplemented!`, a `_` arm standing for one variant, a `_` arm on `Err` that panics or exits without naming the error, `to_owned`/`to_vec` where `clone` is meant, `to_string` on a `&&str`, enum glob imports, `Rc<Mutex>`, a `continue` that changes nothing, `x = y.clone()` where `x.clone_from(&y)` reuses the buffer, a stable sort of primitives, a `;` after a block, a `Result` turned into an `Option` only to be dropped (`.ok();`; a best-effort one says so in an `expect`), a `bool` matched as two arms or turned into 0 or 1 by an `if`, `map_or(false, ..)` for `is_some_and`, `assert!(a == b)` for `assert_eq!`, or-patterns spelled arm by arm, a format argument named twice, a long literal without separators and a `Duration` built in the wrong unit |
| `scripts/check-rust-style.py` | `#[allow]`, including inside `cfg_attr`, and `#![deny(warnings)]`; an `expect` of `warnings` or a lint group, whose expectation any one lint meets, so it waives the rest unseen; `cfg(clippy)`; a `clippy.toml` key or `.cargo/config.toml` rustflag that loosens the table; `#[ignore]` without a reason; `.expect("")`; an `Err(_) => {}` arm or an empty `if let Err(..)`; TODO, FIXME, XXX, HACK, `ponytail:` and the untracked working documents named in a `//`, `///` or `/* */` comment; CJK in Rust or `web/`; a file past its ceiling; a lint in the required set loosened in `Cargo.toml` |

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
finding. Test code needs no waiver for `unwrap` or `panic!`: `clippy.toml`
allows both in unit and integration tests alike, so a `#![expect]` at the top
of a `tests/*.rs` file is stale the day it is written.

## File size

New Rust files stay within 1500 lines. By default, the checker derives each
limit from the committed file at the same path in HEAD, with a floor of 1500;
working files may shrink freely but larger files may not grow. Without Git
every file gets 1500 and the gate's baseline cannot be read, so a source
archive fails the gate; run it from a clone.
`scripts/ceiling-hotspots.py` ranks the larger files by churn times size, as
one input to choosing which to split first. Count a file's mutants before
splitting it with `cargo mutants --list -f <file> | wc -l`; `make mutants`
downloads a compiler first.

The gate passes `--baseline origin/main`. For a larger file present in both
that baseline and HEAD, its limit is the smallest size across those two trees
and every reachable committed version at that path in `origin/main..HEAD`,
with a floor of 1500. All reachable commits count, including merged history.
A committed shrink automatically ratchets the limit; a new or renamed path
gets 1500, and deletion retires the allowance. Split a large file by
responsibility rather than grow it. An explicit baseline requires resolvable
HEAD and baseline refs, common history and a complete, non-shallow
repository; an error fails locally and in CI. Diverged refs are supported,
but the branch must still satisfy any smaller baseline limit. Test bodies
live under `tests/unit/` and answer to the same ceiling.

### Splitting a file

What splitting `src/agent/prompts.rs` into the facade-plus-modules layout under
`src/agent/prompts/` needed, in order:

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
   A hub whose siblings already open with `use super::*` (`src/livekit/`) takes
   new modules the same way; clippy decides whether the `expect` for
   `wildcard_imports` is owed, so run it rather than copy the attribute.
5. **Tests that reach private items** through `use super::*` keep working if the
   facade carries a `#[cfg(test)] use self::{...}` group naming them, and the
   `use super::...` of the parent's own types they read; the test file is not
   edited. Group the facade's re-exports by visibility (`pub use`, then
   `pub(crate) use`), as the parent re-exported them.
6. **Look for what names the file.** `git grep -n 'path/to/file'` found a test
   that hashes the file with `include_bytes!` (a digest that would silently stop
   changing when a prompt did), the `Applies to` lines of two review rules, a
   skill doc line, and comments in Rust and in `web/`. A name inside a format string (`{NAME}`) is a compile error if
   its import is missed, which is the safe way to fail. The three splits of
   `src/livekit.rs`, `src/agent.rs` and `src/gemini.rs` found two more kinds:
   a test that reads the source as text (`include_str!`, then splits on `async
   fn name`) fails with "is still defined here" in `cargo test --lib` and not in
   clippy, and a source digest that hashes the old file (the report evaluation's
   identity) stops covering the code that left it.
7. **Check the split**: when the user has asked for commits, commit it first,
   then run clippy at `-D warnings` and `./scripts/test.sh rust`. The old file's
   limit drops to its committed size on its own; there is no table to update.

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
  error.into_inner())`, not `.unwrap()`: a poisoned lock is recovered. Each such
  lock guards state that is written whole, so the next taker sees a valid value,
  and a `Drop` that takes a lock must not panic while a task unwinds. Where the
  reason is not obvious (the accounts connection, the dispatch maps, the
  cooldown map, the pressure counters, the task admission state) a comment at
  the declaration gives it. A new lock gets the same question: if a panic
  halfway through an update would leave its state inconsistent, recovering is
  wrong, and the update should be one step or the lock should not be recovered.
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
python3 scripts/check-rust-style.py --baseline origin/main
python3 scripts/check-rust-style.py --workspace
cargo test --locked <one filter>
```

Then `./scripts/test.sh` per codetrial-verify. Say which of these ran; "clippy is
clean" without the checker is half a report.

## Audit limits

The Python style checker scans text, not expanded Rust syntax. Multiline
spellings are checked, but strings, comments and macro expansion can defeat a
text rule. Clippy provides compiler-aware checks; neither tool establishes
shutdown correctness, secret redaction, bounded memory or behavioral coverage.
Keep those in the judgment pass and cite an observable regression when fixing
one. A passing gate is evidence for its checks, not a production certification.

## Structure lints

`tools/check-structure` indexes the root Cargo workspace (the root crate and
`scripts/hook-check`; test code is classified apart and does not count as a use) with `rust-analyzer scip` and reports
module-cycle, tangle, field-leak, shallow-module and wide-directory findings.
It is slow on a cold index (minutes), so it is not in `scripts/test.sh`, the
hooks or the CI gate lanes; `.github/workflows/structure.yml` runs it once a day
and fails on anything `tools/check-structure/debt.allow` does not list. Nothing
else catches a new tangle or leak before that run, so run it yourself:

- **before** a structural refactor, for the baseline (`--update-allow` into a
  scratch file and diff it against `debt.allow`);
- **after** any change that moves a type, adds a module or a file to a
  directory, or adds a field another module touches;
- **before calling a structural refactor done**, with `--ratchet`.

```sh
cargo run --release --manifest-path tools/check-structure/Cargo.toml \
    --target-dir target/structure-tool -- \
    --allow tools/check-structure/debt.allow --ratchet
```

Without `--allow` it prints every finding with the places that cause it; that
report is where to start. Apply `--update-allow` to `debt.allow` only when the
diff against the current list is removals. A new `module-cycle`, `tangle`,
`shallow-module` or `wide-directory` means the change is wrong: reshape it, do
not list it.

### Reading a finding

| Lint | What it says | Fix |
|---|---|---|
| `module-cycle` | Sibling modules use each other. Where a type is *declared* decides it, not where its `impl` sits. | Move the type to a leaf both sides depend on (`livekit/streams.rs` under `media` and `turn`), or pass the value in instead of importing a constant from the other side. |
| `tangle` | Things that know each other in a circle across modules, or in a circle of more than seven within one module. | Break the one edge that points up a layer. |
| `field-leak` | Code outside a type writes, borrows or calls a method on a field the type guards, directly or through a getter or setter. | Give the owner a method that does the whole step. |
| `shallow-module` | A module shows many functions and hides few. | Hide more or merge; splitting it keeps the interface. |
| `wide-directory` | More than 15 files and directories. | Group by concept into a subdirectory (`turn/activity/`), counting first. |

### What is not a fix

The tool follows forwarders and accessors, but not every one: it accepted a
predicate (`has_entries`) and a pass-through (`cost_line`) that hid nothing. It
cannot tell an honest fix from a quiet one, so judge it yourself. Not a fix:

- a getter, a setter or a one-line forwarder that moves the leak;
- a type split or a wrapper made only so the field is no longer "guarded"
  (a bag of fields with no methods is fine when a real seam needs it, such as
  receivers a `select!` borrows independently, and says so in its comment);
- code moved to another module to hide the edge.

An intent method carries behavior the caller should not know the fields for:
stamp the clocks a result moved, take and clear, decide and write together.
Where only a setter would do, keep the entry and say why in the PR.

### Pitfalls met doing this

- A fix can create the next finding. Moving a type to quiet one lint made a
  cycle; check the report again after every move, not at the end.
- A new module can push its directory past 15 entries. Count first, and group
  related modules in one subdirectory.
- Tests that read source (`include_str!` plus a function name, or a body
  split on `"\n}\n"`) follow a function by name. Moving or renaming one needs
  `rg` over `tests/` for the file and the symbol, and the assertions must stay
  as strict, not be rewritten to pass.
- A moved or new file gets the 1500-line ceiling, not its old one. A test file
  at its ceiling grows by an `include!` parts file, as `session_parts/` does,
  never by raising the ceiling. `scripts/check-rust-style.py` reports it.
- Splitting a long function so the leak leaves with it: keep the awaits and
  their order, carry the locals in a struct, and let the existing tests prove
  nothing moved.
- Fix, then shrink the list with `--update-allow`. `--ratchet` also fails on an
  entry that no longer matches, so a fix without the list edit fails too.
- Break a cycle by moving the type to the layer that owns it or by inverting
  the dependency behind a trait, not by a re-export alone. A moved module's
  hard-coded test names (`livekit::startup::recovery::tests::…`) and
  `include_str!` depths change with it.
- The tool's own tests are under `tests/check-structure/`.
