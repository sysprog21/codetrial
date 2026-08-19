.PHONY: all build clean indent check verify-vendor serve web

all: build

build:
	cargo build --release

clean:
	cargo clean

indent:
	cargo fmt
	cargo clippy --all-targets -- -D warnings

# No verify-vendor prerequisite: scripts/test.sh runs it, and having both hash
# 12MB of wasm twice per check bought nothing.
check:
	./scripts/test.sh
	./scripts/gemini-check.sh

# Kept as a target so `make verify-vendor` still works on its own; the logic
# lives in one place because two copies had already drifted apart.
verify-vendor:
	@./scripts/verify-vendor.sh

serve:
	@if [ "$${CODETRIAL_SKIP_CONFIG:-}" != 1 ] && \
		[ -z "$${LIVEKIT_URL:-}" -o -z "$${LIVEKIT_API_KEY:-}" -o -z "$${LIVEKIT_API_SECRET:-}" ] && \
		[ ! -f config/codetrial.env.local ]; then \
		echo "Error: config/codetrial.env.local does not exist. Copy config/codetrial.env.example to config/codetrial.env.local before running 'make serve'."; \
		exit 1; \
	fi
	cargo run -- serve

web:
	cargo run -- web
