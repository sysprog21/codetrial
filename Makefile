.PHONY: all build clean indent indent-check check mutants explore-properties fetch-vendor \
	fetch-clang verify-vendor web hooks uninstall-hooks

all: build

# webrtc-sys needs Clang 21+ on Linux. When the caller has not chosen a C++
# compiler, an x86_64 Linux build uses the pinned one scripts/fetch-clang.sh
# installs, a no-op once it is on disk; anything else keeps its own CXX.
ifeq ($(shell uname -sm),Linux x86_64)
ifeq ($(origin CXX),default)
CXX := $(CURDIR)/target/clang/bin/clang++
export CXX
CLANG := fetch-clang
endif
endif

build: fetch-vendor $(CLANG)
	cargo build --release

clean:
	cargo clean

# The formatter chain lives in scripts/indent.sh, because the gate has to run
# the same tools over the same files in the same order; two copies of that list
# would agree only until one of them was edited. Clippy stays here: it is a
# lint rather than a formatter, and `make check` runs it too.
indent: $(CLANG)
	@./scripts/indent.sh --write
	cargo clippy --all-targets -- -D warnings

# The same chain against a copy of the tree, so nothing is rewritten. Unlike the
# gate, a formatter the tree needs and lacks fails here rather than skipping.
indent-check:
	@./scripts/indent.sh --check

# No verify-vendor prerequisite: scripts/test.sh runs it, and having both hash
# 12MB of wasm twice per check bought nothing.
check: $(CLANG)
	./scripts/test.sh
	./scripts/gemini-check.sh

# Mutation testing, deliberately outside `make check`: one mutant is a rebuild
# and a full `cargo test`, which is five minutes here, so a whole-tree pass is
# measured in days rather than minutes. It is a periodic audit, run at the
# points where "the tests pass" needs to mean something stronger.
#
# MUTANTS_SCOPE is the default because a scope is the whole decision. The only
# mutation run this tree ever kept covered the agent and LiveKit half and left
# the accounts, delivery and retention half unmutated -- the half where a
# surviving mutant is a way a candidate's recording outlives the promise made
# about it. That is what this scope names, and it is where a run should start
# when nobody has said otherwise. Override it for anything else:
#
#     make mutants MUTANTS_SCOPE='-f src/web/token.rs'
#
# The output lands in mutants.out/, which is gitignored: it is a report about
# one revision on one machine, and the last one to sit in a working tree went
# five days and nine commits stale, naming line numbers that no longer existed.
# Regenerate it rather than reading an old one.
#
# `--timeout` is absolute and generous on purpose. cargo-mutants otherwise
# derives one from how long the baseline took, and the baseline runs alone
# while every mutant after it runs alongside `-j` siblings: a suite that took
# eleven seconds by itself takes minutes under eight of them, so a derived
# timeout marks live mutants TIMEOUT. A timeout is neither caught nor missed,
# so that reads as a clean run while measuring nothing.
#
# Kept out of MUTANTS_SCOPE for exactly that reason. Bundled in with the file
# filters, the override above silently dropped the timeout with the scope it
# meant to replace, and the run it produced is the one this paragraph describes:
# clean-looking and measuring nothing.
MUTANTS_FLAGS = -j 6 --timeout 900
MUTANTS_SCOPE ?= -f 'src/recording/*.rs' -f src/accounts.rs -f 'src/accounts/*.rs' \
	-f src/delivery.rs -f src/web/auth.rs

# `--cargo-arg=--locked` rather than a bare `--locked`: cargo-mutants owns its
# own flags and forwards this one to every cargo it runs, which is what keeps a
# mutant from being judged against a lockfile it quietly updated.
mutants: fetch-vendor $(CLANG)
	EPHYRA_REGRESSIONS=0 cargo mutants --cargo-arg=--locked $(MUTANTS_FLAGS) $(MUTANTS_SCOPE)

# New inputs for the properties in tests/unit/tasks/*_properties.rs, whose fixed seeds draw
# the same ones every run; docs/development.md says how to use it.
explore-properties: fetch-vendor $(CLANG)
	@seed=$${SEED:-$$(date +%s)}; echo "EPHYRA_SEED=$$seed"; \
	EPHYRA_SEED=$$seed EPHYRA_TESTS=20000 EPHYRA_REGRESSIONS=0 cargo test --locked --lib property_tests

# The hooks run the fast half of the gate over the staged content and hold the
# commit message to the rules the log already follows. The wrappers resolve the
# active worktree's scripts, and they are opt-in because git will not install a
# hook for a contributor and neither will a checkout.
hooks:
	@./scripts/install-git-hooks.sh

uninstall-hooks:
	@./scripts/install-git-hooks.sh --uninstall

# The big MediaPipe binaries are downloaded rather than committed, so anything
# that serves or checks web/ needs this first. It is a no-op once the pinned
# bytes are on disk.
fetch-vendor:
	@./scripts/fetch-vendor.sh

# Kept as a target so `make verify-vendor` still works on its own; the logic
# lives in one place because two copies had already drifted apart.
verify-vendor:
	@./scripts/verify-vendor.sh

fetch-clang:
	@sh scripts/fetch-clang.sh target/clang > /dev/null

web: fetch-vendor $(CLANG)
	cargo run -- web
