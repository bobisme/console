set shell := ["bash", "-euo", "pipefail", "-c"]

default: check

# Run every repository gate used before merging or releasing.
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    node web/smoke.cjs
    node web/smoke.cjs --cart carts/lantern-leap.cart --frames 180 --input-mask 16 --expect-audio

# Real-browser diagnostic fault containment. Requires agent-browser and an
# explicit Chromium executable so missing browser infrastructure never skips.
browser-diagnostics:
    test -n "${CONSOLE_BROWSER:-}" || { echo "CONSOLE_BROWSER must name a Chromium executable" >&2; exit 2; }
    out="$(mktemp --suffix=.console.html)"; trap 'rm -f "$out"' EXIT; cargo run -q -p console-pack -- carts/lantern-leap.cart -o "$out"; node web/diagnostics-smoke.cjs "$out"

# Install the current local agent and packer binaries, even at the same version.
install:
    cargo install --path crates/console-agent --locked --force
    cargo install --path crates/console-pack --locked --force
