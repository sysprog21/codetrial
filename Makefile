.PHONY: all build clean indent check fetch-vendor verify-vendor serve web

all: build

build: fetch-vendor
	cargo build --release

clean:
	cargo clean

# Rust, then Python, then shell. `cargo fmt` ships with the toolchain, but ruff
# and shfmt do not, so a missing one is a skip with a message rather than a
# failed target: formatting is not part of the gate for these two, and someone
# without the tools should not be blocked from running the half that works.
#
# shfmt takes its style from .editorconfig, which already declares this repo's
# shell settings, so the rules live in one place rather than in a flag here.
indent:
	cargo fmt
	cargo clippy --all-targets -- -D warnings
	@if command -v ruff >/dev/null 2>&1; then \
		ruff format scripts; \
	else \
		echo "ruff not installed; skipping Python formatting"; \
	fi
	@if command -v shfmt >/dev/null 2>&1; then \
		shfmt -w scripts/*.sh; \
	else \
		echo "shfmt not installed; skipping shell formatting"; \
	fi

# No verify-vendor prerequisite: scripts/test.sh runs it, and having both hash
# 12MB of wasm twice per check bought nothing.
check:
	./scripts/test.sh
	./scripts/gemini-check.sh

# The big MediaPipe binaries are downloaded rather than committed, so anything
# that serves or checks web/ needs this first. It is a no-op once the pinned
# bytes are on disk.
fetch-vendor:
	@./scripts/fetch-vendor.sh

# Kept as a target so `make verify-vendor` still works on its own; the logic
# lives in one place because two copies had already drifted apart.
verify-vendor:
	@./scripts/verify-vendor.sh

serve: fetch-vendor
	@if [ "$${CODETRIAL_SKIP_CONFIG:-}" != 1 ] && \
		[ -z "$${LIVEKIT_URL:-}" -o -z "$${LIVEKIT_API_KEY:-}" -o -z "$${LIVEKIT_API_SECRET:-}" ] && \
		[ ! -f config/codetrial.env.local ]; then \
		echo "Error: config/codetrial.env.local does not exist. Copy config/codetrial.env.example to config/codetrial.env.local before running 'make serve'."; \
		exit 1; \
	fi
	cargo run -- serve

web: fetch-vendor
	cargo run -- web
