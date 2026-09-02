.PHONY: all build clean indent check fetch-vendor verify-vendor web \
	hooks uninstall-hooks

all: build

build: fetch-vendor
	cargo build --release

clean:
	cargo clean

# The formatter chain lives in scripts/indent.sh, because the gate has to run
# the same tools over the same files in the same order; two copies of that list
# would agree only until one of them was edited. Clippy stays here: it is a
# lint rather than a formatter, and `make check` runs it too.
indent:
	@./scripts/indent.sh --write
	cargo clippy --all-targets -- -D warnings

# No verify-vendor prerequisite: scripts/test.sh runs it, and having both hash
# 12MB of wasm twice per check bought nothing.
check:
	./scripts/test.sh
	./scripts/gemini-check.sh

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

web: fetch-vendor
	cargo run -- web
