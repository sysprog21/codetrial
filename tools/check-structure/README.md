# check-structure

A structural linter for CodeTrial's Rust code. It judges how the code is
divided into modules and what each module exposes, which clippy and rustfmt do
not look at. The rule behind every lint is the same: a module should be deep
(much hidden behind a small interface), and what it hides should stay hidden.

`docs/development.md` ("Architecture lints") says how the gates run it. This
page says what it checks, how, and where it stops.

## Scope

It checks:

- Rust source of the Cargo workspace it is started in, test code excluded from
  what counts as a use. For CodeTrial that is the root crate and
  `scripts/hook-check`; `desktop/`, `vendor/` and this tool are separate
  workspaces and are not read.
- One build configuration per run: the host target with default features,
  unless `--target`, `--features`, `--all-features` or
  `--no-default-features` (or Cargo's own `build.target` and `RUSTFLAGS`)
  choose another. Code behind a `cfg` that configuration turns off, such as a
  `#[cfg(windows)]` module on Linux, is not seen. Run once per configuration
  that matters.

It does not check:

- File length. `scripts/check-rust-style.py` owns that, with a per-file ceiling
  that only ratchets down.
- Style, correctness, unsafe use, lint suppression or formatting: clippy,
  rustfmt and `scripts/check-rust-style.py`.
- Anything outside Rust: `web/`, Python, shell, workflows.
- Behavior. A passing run says the structure is within bounds, not that the
  code is right.

It reads tokens and the index, not types, so it stops short in a few places:

- A closure or async block with no parameters whose body only calls another
  function can look like a forwarder.
- A `#[cfg(test)]` or `#[test]` function written directly in a production
  module is not seen as test code; a test module is. A module named `test`,
  `tests`, `*_tests` or `test_*` counts as test code whatever it holds.
- A run limited to one crate or module keeps a tangle when its member types
  are homed there, not when only the code that ties them is written there.

## Lints

| Lint | Refuses |
|---|---|
| `module-cycle` | Sibling modules that use each other in a circle, so there is no order to read them in. A module and the modules inside it never form a cycle, and neither do crates, which Cargo keeps in order. For a circle of more than two modules the report names the layering, lowest first, that has the fewest ties against it, and those ties: the ones to cut. Up to 20 modules the layering is the best there is; past that it is built greedily. |
| `tangle` | Things (a type with everything written for it, a free function, a constant) that know each other in a circle across more than one module, or a circle of more than seven. Where an `impl` block is written does not matter, so moving one changes nothing. |
| `field-leak` | Code outside a type that writes, borrows, indexes or calls a method on a field the type guards, directly or through a getter or setter. Reading a copy, or borrowing plain data, is not a leak. A type guards a field when it keeps at least one field to itself and has a `self` method that uses it. Visibility keywords play no part. |
| `shallow-module` | A crate or a module with children whose interface shows more than seven functions and at least as many as it hides, spread over more than seven things or concentrated in one. |
| `wide-directory` | A directory holding more than 15 Rust files and directories that hold Rust files. |

The number seven is what a reader can hold in mind at once. Each report has a
picture under it showing the offending edges.

## How it works

1. `cargo metadata` finds the workspace, its members and their targets.
2. `rust-analyzer scip` writes a SCIP index of the workspace: for every
   document, each name written there and the symbol it resolves to. Indexing
   also builds what compiling the code needs (build scripts, proc macros), so
   generated items are seen where the code uses them. A small protobuf reader
   in `src/scip/` decodes only the fields the tool reads.
3. `src/model/` turns the index plus the source text, which `src/source/` reads
   token by token, into a plain model: the
   module tree, files, items, and every reference from code to an item outside
   `use` declarations. Wrappers, forwarders and accessors are seen through, so
   a use of a forwarder counts as a use of the function at the end of its chain.
4. `src/graph.rs` builds the graph of which things know which; its strongly
   connected components are the tangles.
5. `src/lint/` runs the lints over the model and `src/format/` prints each as a
   `rustc`-style diagnostic with a picture.

The index is cached under `target/check-structure/`, one directory per
workspace and configuration, and rebuilt only when a source file of a package
cargo resolves from a path (the members, path dependencies, `[patch]` targets), the
lock file, the rust-analyzer version or its settings change. A directory with a
manifest of its own that is none of those (`desktop/`, a checkout under
`externals/`, this tool) is not read, so editing it does not reindex. A cold run on CodeTrial takes
about a quarter of an hour (the dependencies' build scripts dominate), a warm
one about five minutes. While it indexes, a terminal shows a spinner with the
elapsed time; a pipe or CI log gets one plain line a minute instead.
`rust-analyzer` is a rustup component that
`rust-toolchain.toml` does not list: `rustup component add rust-analyzer`.

## Usage

Run from the repository root, since the tool checks the workspace around the
current directory:

```sh
cargo run --release --manifest-path tools/check-structure/Cargo.toml \
    --target-dir target/structure-tool -- \
    --allow tools/check-structure/debt.allow --ratchet
```

```text
check-structure [crate_or_mod] [--allow <path> [--ratchet]] [--update-allow <path>] [--timings] [--all-ties]
                [--target <triple>] [-F, --features <features>]
                [--all-features] [--no-default-features]
```

| Option | Effect |
|---|---|
| `crate_or_mod` | Check only one crate or module, for example `codetrial::agent`. |
| `--allow <path>` | Suppress the listed findings. A finding not listed still fails. |
| `--ratchet` | With `--allow`, also fail while the list holds an entry that is fixed, or a circle that has since shrunk and is listed under another key, so the debt only shrinks. Judges the whole list, so it cannot be given a `crate_or_mod`. |
| `--update-allow <path>` | Write the current findings to `<path>` and exit 0. Cannot be combined with `--allow` or a `crate_or_mod`, which would drop every entry outside it. |
| `--timings` | Print on stderr how long the index, the model and the lints took. |
| `--all-ties` | List every tie of a module cycle or tangle. Without it, the report shows the seven dearest and counts the rest. |
| `--target`, `--features`, `--all-features`, `--no-default-features` | Choose the configuration, as `cargo check` takes them. |

Exit status: 0 clean, 1 findings (or a stale entry under `--ratchet`), 2 the
tool could not run (no workspace, no `rust-analyzer`, a malformed index).
When `GITHUB_ACTIONS=true`, each finding is also written as an
`::error file=..,line=..` workflow annotation.

## The debt list

`debt.allow` has one `<lint> <key>` per line, sorted. It records the findings
that existed when the gate was added, so the gate could start refusing new
ones. Fix a finding by changing the structure, not the list: decide what the
module is meant to hide, and narrow its interface rather than moving the leak
to another module. Remove the entry in the same change with `--update-allow`.

## Tests

`cargo test --manifest-path tools/check-structure/Cargo.toml` runs unit tests
for the parsing, graph and lint logic, and end-to-end tests that copy the
crates under `tests/check-structure/fixtures/` (each a Cargo project) and the
variants under `tests/check-structure/variants/`, run the binary on them through a real `rust-analyzer`, and
compare the reported keys. A variant is a fixture with a file changed, which
pins what a fix must do to make a finding go away and what a mere shuffle must
not.
