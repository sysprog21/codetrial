# Rust recipes

Each recipe is a smell, the change, the proof, and the site in this tree that
taught it. A recipe that ends "leave it" is as much a result as one that
rewrites. Run the gates in [rust.md](../rust.md) before handing back.

## Take `Option<&str>`, not `Option<&String>`

A parameter typed `Option<&String>` names the owning type, so a caller holding a
`&str` has to allocate before calling. `Option<&str>` accepts both.

- **Site:** `dispatch_or_release_consent` in `src/web/token.rs`. One caller; it
  holds an `Option<String>` and passes `interview.as_ref()`.
- **Change:** the parameter becomes `Option<&str>`, the caller passes
  `interview.as_deref()`, and the body builds the owned `String` that
  `NoShowConsent::new` takes with `to_owned()` where it used `clone()`.
- **Proof:** `rg -n 'dispatch_or_release_consent' .` lists the one caller;
  clippy at `-D warnings` is clean; `cargo test --locked --test web token`
  passes before and after. That filter runs the changed closure, since the tests
  at `tests/web/token.rs:1547` and `:1909` post an `interviewId` and expect a
  dispatch. It does not reach the release branch after a refusal: the refusal
  tests post no interview, so a recipe that touched that branch would need a new
  test first.

## Leave a return type that other closures share

`pending_candidate_line` in `src/agent/prompts/recovery.rs` returns `Option<&String>`.
Returning `Option<&str>` looks like the same recipe and is not. Its caller in
`src/livekit/session.rs` applies one `missing` closure to this value and to
`first_candidate`, which would both have to change, and the re-export in
`src/agent.rs` and the assertion in `tests/unit/agent.rs` with them. The cost
is in the `opening` closure there: it calls `std::slice::from_ref(line)` to
build the `&[String]` that `transcript_tail` takes, so with a `&str` it would
have to allocate `[line.to_owned()]` on a path that allocates nothing today. A
type-level tidy-up that adds an allocation is not a win. Leave it.

## Leave a `&Option<T>` closure parameter that only calls `as_deref`

`credential` in `src/web/auth.rs` and `oauth_credential` in `src/web/mod.rs`
take `&Option<String>` and call `.as_deref()` first. Taking `Option<&str>` moves
that call to each of the four callers, which adds code at every site for no
saving, and a closure is local to one function. Leave them.

## Recipes to add when a site is found

Not written until the tree has a real site, so each one stays a true story:

- a function returning `Vec<T>` that every caller iterates and may stop early,
  to `impl Iterator<Item = T>`;
- a bare `u16` or `u64` that is a port or a duration, to `NonZeroU16` or
  `Duration` (`rg 'port: u16' .` finds the first);
- free functions over one collection, to methods on a newtype.

A `bool` parameter becomes an enum when a call site stops being readable; the
build only refuses more than three (`clippy::fn_params_excessive_bools`).
