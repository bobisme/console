set shell := ["bash", "-euo", "pipefail", "-c"]

default: check

# Run every repository gate used before merging or releasing.
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    node web/smoke.cjs

# Install the current local agent and packer binaries, even at the same version.
install:
    cargo install --path crates/console-agent --locked --force
    cargo install --path crates/console-pack --locked --force
