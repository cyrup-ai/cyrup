#!/usr/bin/env bash
# SessionStart hook — brings a fresh container up to the point where the README's
# gates (`cargo clippy --workspace --all-targets`, `cargo nextest run --workspace`)
# actually run, and puts the tracked upstreams where the gap-analysis docs expect them.
#
# Wired from .claude/settings.json. `setup.sh` delegates here so there is ONE
# setup path, not two that drift.
#
# Everything here is idempotent: the container is cached after the hook completes,
# so a warm start re-runs this and should do almost nothing.
set -euo pipefail

# Remote (Claude Code on the web) only — a local checkout keeps its own toolchain
# and should not have 8 upstream clones dropped into it uninvited.
# Run it anywhere by hand with: CYRUP_INIT_FORCE=1 .claude/hooks/session-start.sh
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ] && [ "${CYRUP_INIT_FORCE:-}" != "1" ]; then
  exit 0
fi

REPO="${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
cd "$REPO"

# Log to stderr: stdout of a SessionStart hook is consumed by Claude Code (JSON in
# async mode, session context otherwise) and must stay clean.
log()  { printf '[init] %s\n' "$*" >&2; }
warn() { printf '[init] WARNING: %s\n' "$*" >&2; }

# ---------------------------------------------------------------------------
# 1. CARGO_INCREMENTAL=0
#
# Incremental artifacts are the single biggest consumer of the container's fixed
# writable allowance — this workspace is 21 crates / ~786k lines, and a debug
# build of it was measured at ~11GB WITHOUT counting incremental state. The
# allowance is per-session and `df` reports the disk, not the allowance, so
# running out shows up as "no space left on device" at 20% used. Off it stays.
# ---------------------------------------------------------------------------
export CARGO_INCREMENTAL=0
if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  # Guard the append: the hook also fires on resume/clear/compact.
  if ! grep -qs '^export CARGO_INCREMENTAL=0$' "$CLAUDE_ENV_FILE"; then
    echo 'export CARGO_INCREMENTAL=0' >> "$CLAUDE_ENV_FILE"
  fi
fi
log "CARGO_INCREMENTAL=0"

# ---------------------------------------------------------------------------
# 2. Rust
#
# The image ships a stale stable (1.94.1 as of this writing) while the workspace
# declares rust-version = "1.96". Below the floor, cargo refuses to compile
# ANYTHING: "error: rustc 1.94.1 is not supported by the following packages".
# rust-toolchain.toml pins channel = "stable", which resolves to whatever stable
# the image baked in, so it has to be refreshed explicitly.
#
# The floor is READ FROM Cargo.toml rather than hardcoded, so bumping
# rust-version there is all that is needed to move this gate.
# ---------------------------------------------------------------------------
ver_ge() { [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n1)" = "$2" ]; }

RUST_FLOOR="$(sed -n 's/^[[:space:]]*rust-version[[:space:]]*=[[:space:]]*"\([0-9.]*\)".*/\1/p' Cargo.toml | head -n1)"
: "${RUST_FLOOR:=1.96}"

rust_now() { rustc --version 2>/dev/null | awk '{print $2}'; }

if ver_ge "$(rust_now)" "$RUST_FLOOR"; then
  log "rust $(rust_now) already satisfies the workspace floor ${RUST_FLOOR}"
else
  log "rust $(rust_now) is below the workspace floor ${RUST_FLOOR} — updating stable"
  rustup update stable --no-self-update
fi

if ! ver_ge "$(rust_now)" "$RUST_FLOOR"; then
  echo "[init] FATAL: rustc $(rust_now) is below the workspace floor ${RUST_FLOOR} even after" >&2
  echo "[init]        'rustup update stable'. Nothing in this workspace will compile." >&2
  exit 1
fi
log "rustc $(rust_now)"

# rustfmt + clippy are declared in rust-toolchain.toml, which installs them lazily
# on first use. Do it up front so `cargo fmt --all -- --check` and
# `cargo clippy --workspace --all-targets -- -D warnings` are ready to run.
rustup component add rustfmt clippy >/dev/null 2>&1 || warn "could not add rustfmt/clippy"

# wasm32-wasip2 is deliberately NOT auto-installed by rust-toolchain.toml, but two
# of the README's REQUIRED gates need it:
#   cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2
#   cargo nextest run -p cyrup-it --features it,wasm-host
# ~35MB, a few seconds.
rustup target add wasm32-wasip2 >/dev/null 2>&1 || warn "could not add wasm32-wasip2"

# ---------------------------------------------------------------------------
# 3. cargo-nextest
#
# The everyday gate is `cargo nextest run --workspace`, and .config/nextest.toml
# is this repo's DEADLOCK TRIPWIRE (slow-timeout + leaky-test detection). Without
# nextest that whole safety net is inert.
#
# The official installer at https://get.nexte.st is BLOCKED by this org's egress
# policy (403 at the proxy), but github.com is reachable — so take the prebuilt
# release binary from there. That is a ~12MB download instead of the multi-minute
# from-source `cargo install`, which stays as the fallback.
# ---------------------------------------------------------------------------
NEXTEST_VERSION="0.9.144"
CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"

if cargo nextest --version 2>/dev/null | grep -q "cargo-nextest ${NEXTEST_VERSION}"; then
  log "cargo-nextest ${NEXTEST_VERSION} already installed"
else
  log "installing cargo-nextest ${NEXTEST_VERSION}"
  mkdir -p "$CARGO_BIN"
  NEXTEST_TGZ="$(mktemp -t nextest-XXXXXX.tar.gz)"
  NEXTEST_URL="https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-${NEXTEST_VERSION}/cargo-nextest-${NEXTEST_VERSION}-x86_64-unknown-linux-gnu.tar.gz"

  if curl -sSfL --retry 3 --retry-delay 2 -o "$NEXTEST_TGZ" "$NEXTEST_URL" \
     && tar xzf "$NEXTEST_TGZ" -C "$CARGO_BIN" cargo-nextest; then
    chmod +x "$CARGO_BIN/cargo-nextest"
    log "cargo-nextest installed from release binary"
  else
    warn "release-binary download failed — falling back to 'cargo install' (slow, compiles from source)"
    cargo install cargo-nextest --locked --version "$NEXTEST_VERSION" \
      || warn "cargo-nextest install FAILED — 'cargo nextest run' will not work"
  fi
  rm -f "$NEXTEST_TGZ"
fi
command -v cargo-nextest >/dev/null 2>&1 && log "$(cargo nextest --version 2>/dev/null | head -n1)"

# ---------------------------------------------------------------------------
# 4. Tracked upstreams -> ./tmp
#
# docs/gap-analysis/ is a ledger measured against these repos, and its hard rule
# is: read upstream with `git -C <repo> show <tag>:<path>`, NEVER from a working
# tree. That rule needs FULL history and ALL TAGS locally — hence no --depth and
# no --single-branch. The paths below are the ones the docs already cite
# (`tmp/pi`, `tmp/code_puppy_core_plugins`, ...), so they must not be renamed.
#
# `tmp/` is already in .gitignore, so none of this becomes repo content.
#
# Non-fatal by design: a network failure here must not stop a session from
# starting, because the toolchain above is what the build actually needs.
# ---------------------------------------------------------------------------
UPSTREAMS=(
  # Ported (5)
  "pi|https://github.com/earendil-works/pi"
  "pi-subagents|https://github.com/nicobailon/pi-subagents"
  "pi-permission-system|https://github.com/MasuRii/pi-permission-system"
  "pi-intercom|https://github.com/nicobailon/pi-intercom"
  "code_puppy_core_plugins|https://github.com/mpfaffenberger/code_puppy_core_plugins"
  # The flux upstream is two repos; the ported surface is in core_plugins, but
  # docs/gap-analysis cites tmp/code_puppy for the host side.
  "code_puppy|https://github.com/mpfaffenberger/code_puppy"
  # Not ported — areas 13 and 15 are port PLANS measured against these.
  "pi-mcp-adapter|https://github.com/nicobailon/pi-mcp-adapter"
  "pi-acp|https://github.com/svkozak/pi-acp"
)

mkdir -p tmp
upstream_failures=0

for entry in "${UPSTREAMS[@]}"; do
  name="${entry%%|*}"
  url="${entry##*|}"
  dest="tmp/${name}"

  if [ -d "$dest/.git" ]; then
    # Already cloned: pull the latest tags and fast-forward the default branch.
    # --force so a retagged upstream updates instead of erroring out.
    if git -C "$dest" fetch --quiet --tags --force --prune origin 2>/dev/null; then
      git -C "$dest" merge --quiet --ff-only '@{u}' >/dev/null 2>&1 || true
      log "$(printf '%-24s updated  %s' "$name" "$(git -C "$dest" describe --tags --abbrev=0 2>/dev/null || echo '(no tag)')")"
    else
      warn "$name: fetch failed — leaving the existing clone as-is"
      upstream_failures=$((upstream_failures + 1))
    fi
  else
    if git clone --quiet "$url" "$dest" 2>/dev/null; then
      log "$(printf '%-24s cloned   %s' "$name" "$(git -C "$dest" describe --tags --abbrev=0 2>/dev/null || echo '(no tag)')")"
    else
      warn "$name: clone from ${url} failed"
      rm -rf "$dest"
      upstream_failures=$((upstream_failures + 1))
    fi
  fi
done

if [ "$upstream_failures" -gt 0 ]; then
  warn "${upstream_failures}/${#UPSTREAMS[@]} upstream(s) unavailable — gap-analysis work needing them will be blocked"
fi

# ---------------------------------------------------------------------------
# 5. Pre-fetch the crate graph so the first build in-session is not a download.
# index.crates.io is on this proxy's direct-access list.
# ---------------------------------------------------------------------------
cargo fetch --locked >/dev/null 2>&1 || warn "cargo fetch --locked failed — the first build will fetch instead"

log "ready: $(rustc --version), $(cargo nextest --version 2>/dev/null | head -n1 || echo 'nextest MISSING'), CARGO_INCREMENTAL=${CARGO_INCREMENTAL}"
