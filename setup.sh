#!/bin/bash
# Setup script for the cyrup dev container.
# Runs before Claude Code launches. Everything here was verified against this image.
set -euo pipefail

cd /home/user/cyrup

# 1. REQUIRED — the image ships rustc 1.94.1, but the workspace declares
#    rust-version = "1.96". Without this, cargo refuses to compile ANYTHING:
#      "error: rustc 1.94.1 is not supported by the following packages"
#    rust-toolchain.toml pins channel = "stable", which resolves to whatever
#    stale stable the image baked in, so it must be refreshed explicitly.
rustup update stable

# 2. rustfmt + clippy are declared in rust-toolchain.toml (rustup normally
#    installs them lazily on first use). Do it up front so `cargo clippy
#    --workspace --all-targets` — the README's REQUIRED gate — is ready to run.
rustup component add rustfmt clippy

# 3. wasm32-wasip2 — deliberately NOT auto-installed by rust-toolchain.toml.
#    Needed by the guest crate cyrup-ext-sdk and by the 23 wasm integration
#    tests (`cargo nextest run -p cyrup-it --features it,wasm-host`).
#    Costs ~4s and ~35MB. Drop this line if you never touch extensions.
rustup target add wasm32-wasip2

# 4. cargo-nextest — the README's everyday gate is `cargo nextest run
#    --workspace` (6,855 tests, ~18s), and .config/nextest.toml is the repo's
#    deadlock tripwire (slow-timeout + leaky-test detection). Without nextest
#    that whole safety net is inert.
#
#    NOTE: the official installer at https://get.nexte.st is BLOCKED by this
#    org's egress policy (403 at the proxy), so we build from crates.io
#    instead — index.crates.io is on the direct-access list. This compiles
#    from source and is the slowest step here by far.
#    ==> If you can allowlist get.nexte.st in the egress policy, this becomes
#        a ~2s binary download instead of a multi-minute compile.
if ! command -v cargo-nextest >/dev/null 2>&1; then
  cargo install cargo-nextest --locked
fi

# 5. Pre-download all 789 crates in Cargo.lock so the first build in-session
#    doesn't spend minutes on the network.
cargo fetch --locked

# ---------------------------------------------------------------------------
# OPTIONAL — uncomment to trade setup time for a ready-to-go session.
#
# Pre-warm the build cache. Measured on this image: ~7m45s and 11GB of disk
# for the debug profile. Worth it if you want the agent productive instantly;
# skip it if you'd rather have the session start fast.
# cargo build --workspace --all-targets
#
# Faster linking via lld (clang + lld are already in the image). This changes
# RUSTFLAGS, so it invalidates any cache built without it and diverges from
# what devs get locally — enable deliberately, not by default.
# export RUSTFLAGS="-C link-arg=-fuse-ld=lld"
# ---------------------------------------------------------------------------

echo "cyrup dev env ready: $(rustc --version)"
