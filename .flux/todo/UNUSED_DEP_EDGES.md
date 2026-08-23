---
stage: new
status: done
updated: 2026-08-23 00:06
---

# Remove 27 unused dependency edges, hoist 25 crate-local pins into [workspace.dependencies], and kill the tracing/regex version drift

> Found by an eight-lens workspace hygiene sweep. Every count below was reproduced
> against the tree before this task was filed.
> **Priority:** medium · **Effort:** medium
> **Crates:** `cyrup-mcp`, `cyrup-tools`, `cyrup-sdk`, `cyrup-ext`, `cyrup-modes`, `cyrup-permission-system`, `cyrup-session`, `cyrup-session-svc`, `cyrup-provider`, `cyrup-test-support`, `cyrup-intercom`, `cyrup-ext-subagents`, `cyrup`, `cyrup-resources`, `cyrup-config`, `cyrup-tui`

Three manifest-hygiene problems on the same set of `Cargo.toml` files.

**1. 27 declared-but-unused dependency edges across 13 crates**, proven by rustc's own `-W unused_crate_dependencies`:

```
cyrup_ext:               cyrup_session, dashmap
cyrup_ext_subagents:     cyrup_session_svc
cyrup_intercom:          cyrup_permission_system
cyrup_mcp:               cyrup_tools, jsonschema, tokio_util
cyrup_modes:             async_trait, tokio_stream
cyrup_permission_system: cyrup_config, cyrup_provider, cyrup_session_svc, cyrup_test_support
cyrup_provider:          futures_core
cyrup_sdk:               async_trait, cyrup_ext, tempfile, tokio, tokio_stream
cyrup_session:           futures
cyrup_session_svc:       futures
cyrup_test_support:      cyrup_resources
cyrup_tools:             futures_core, grep_matcher, tokio_util, unicode_segmentation
```

Six are whole intra-workspace crate edges (cyrup-mcp→cyrup-tools, cyrup-ext→cyrup-session, cyrup-sdk→cyrup-ext, cyrup-test-support→cyrup-resources, cyrup-ext-subagents→cyrup-session-svc, cyrup-intercom→cyrup-permission-system) plus **all four** of cyrup-permission-system's dev-dependencies — these misrepresent the architecture's layering and force needless rebuild fan-out. None is behind a non-default feature: only cyrup-tools (`inline-images`), cyrup-ext (`wasm-host`), cyrup-session-svc (`wasm-host`), cyrup-provider (`faux`) and cyrup (`faux`) have feature tables at all, and none gates a flagged dep. Spot-verified in this repo: `jsonschema` has **0** references in `crates/cyrup-mcp/src` (its only real consumer is `crates/cyrup-ext-subagents/src/exec/structured.rs:72`), and every one of the 8 `cyrup_tools` / 3 `tokio_util` hits in cyrup-mcp is inside a **doc comment**, not code. `futures` is unused in **every** target of the `cyrup` package (lib, bin, and all three integration tests).

**2. 25 external crates pinned crate-locally at 41 declaration sites.** The root `Cargo.toml` states externals "are added to this table when first used" and then names the pending set (ratatui, clap, tracing, tracing-subscriber, fs4, sha2, indexmap, rustc-hash, directories, blake3, toml, serde_yml, gix, wasmtime, wasmtime-wasi, unicode-width) — all 16 are now in use and none was ever added. Also `notify` **is** in `[workspace.dependencies]` at 8.2.0 yet `crates/cyrup-resources/Cargo.toml:29` re-pins the literal `"8.2.0"`, and two intra-workspace dev-deps bypass the table with raw `path =` entries (`cyrup-intercom/Cargo.toml:86`, `cyrup-permission-system/Cargo.toml:85`).

**3. Two pins have already drifted** (re-verified by grep just now): `tracing` is `"0.1.44"` in cyrup-ext-subagents:83, cyrup-ext:35, cyrup-intercom:51, cyrup-mcp:90, cyrup-session-svc:37 but `"0.1"` in `crates/cyrup/Cargo.toml:104`; `regex` is `"1.12.4"` in cyrup-ext:44 and cyrup-mcp:107 but `"1"` in `crates/cyrup-permission-system/Cargo.toml:62`.

**4. Related lockfile cost:** Cargo.lock resolves 786 packages over 727 names — 51 names at 2+ versions, 59 redundant copies (35 names / 41 copies excluding the windows-* family). The clearly self-inflicted one: cyrup-ext-subagents, cyrup-mcp and cyrup-permission-system each pin `sha2 = "0.11.0"` while the rest of the graph (wasmtime-environ, cranelift-codegen, oauth2, secret-service) resolves sha2 0.10.9; `cargo tree -i` confirms sha2 0.11.0 is the sole reverse-dependency root of digest 0.11.3, block-buffer 0.12.1 and crypto-common 0.2.2 — one version choice in three manifests compiles four extra crates.

## Acceptance Criteria

- [ ] `RUSTFLAGS="-W unused_crate_dependencies" cargo check --workspace --all-targets` reports zero `extern crate ... is unused` messages, or each survivor carries a comment in its Cargo.toml explaining why it must stay
- [ ] All six unused intra-workspace edges are gone from the manifests (cyrup-mcp→cyrup-tools, cyrup-ext→cyrup-session, cyrup-sdk→cyrup-ext, cyrup-test-support→cyrup-resources, cyrup-ext-subagents→cyrup-session-svc, cyrup-intercom→cyrup-permission-system) and `cargo build --workspace --all-targets` still succeeds
- [ ] Every remaining external dependency used by 2+ member crates is declared once in [workspace.dependencies] and referenced as `workspace = true`; `grep -E '^(tracing|regex|sha2|ratatui|toml|clap|indexmap|unicode-width|rustc-hash|serde_yml|fs4|directories|blake3|wasmtime)\\s*=' crates/*/Cargo.toml` returns no literal version strings
- [ ] tracing and regex resolve to exactly one version each across the workspace (no `0.1` vs `0.1.44`, no `1` vs `1.12.4`), and crates/cyrup-resources/Cargo.toml uses `notify.workspace = true` instead of re-pinning 8.2.0
- [ ] The two raw `path =` dev-deps (cyrup-intercom/Cargo.toml:86, cyrup-permission-system/Cargo.toml:85) go through [workspace.dependencies]
- [ ] A decision on sha2 is recorded: either all three crates move to the 0.10.x the rest of the graph uses (dropping the duplicate digest/block-buffer/crypto-common copies) or a comment states why 0.11.0 is required; `cargo tree -d` shows the redundant-copy count dropped and Cargo.lock is committed

## Verifying command

```bash
cd /home/user/cyrup && RUSTFLAGS="-W unused_crate_dependencies" cargo check --workspace --all-targets --message-format=json 2>/dev/null | grep -o 'extern crate `[a-z_]*` is unused in crate `[a-z_]*`' | sort -u && grep -Hn '^tracing\s*=' crates/*/Cargo.toml && grep -Hn '^regex\s*=' crates/*/Cargo.toml
```
