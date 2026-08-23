---
stage: exec
status: done
updated: 2026-08-23 00:03
---

# The paths.ts Port Is Split Across Two Tree Levels, and the Production Connect Path Picked the POSIX-Only Half

> Source: `intercom-hygiene-audit` workflow. Severity **high**, effort **medium**.
> Every claim below was produced by a finder agent and then reproduced by an independent
> adversarial verifier; findings that did not reproduce were dropped.

## Scope

- `crates/cyrup-intercom/src/paths.rs`
- `crates/cyrup-intercom/src/transport/target.rs`
- `crates/cyrup-intercom/src/connect.rs`
- `crates/cyrup-intercom/src/bin/cyrup_intercom_child_fixture.rs`

## Description

One upstream file, `pi-intercom/broker/paths.ts`, is ported into two modules at different levels
of the tree: `src/paths.rs` at the crate root and `src/transport/target.rs` under `transport/`.
`getBrokerSocketPath` (`paths.ts:65-74`) is ported TWICE — as `paths::broker_socket_path`
(src/paths.rs:90, self-documented at src/paths.rs:6 as the POSIX arm only) and as
`transport::target::broker_socket_path_for` (src/transport/target.rs:199, both platform arms,
delegating down to the former at :204). Sibling runtime-file paths are split the same way:
`broker_pid_path`/`broker_spawn_lock_path` at src/paths.rs:96,102 vs `broker_port_file_path` at
src/transport/target.rs:192. Two callable spellings of one upstream function at two module paths,
and the production session connect path picked the POSIX-only one: src/connect.rs:461 calls it
with no `cfg` gate and hands the result to `IntercomClient::connect` at :504. On POSIX the two
spellings compute the identical path, so this is latent, not currently-firing; on Windows they
diverge and the client cannot reach the broker at all.

## Why it matters

The crate deliberately carries full Windows support on the broker side (named-pipe
`BrokerListener` arms, src/broker/listener.rs:44,117-119) and full Windows/TCP target resolution
in `transport/target.rs`, and `broker/lifecycle.rs:118` was already changed away from the POSIX-
only spelling with a comment saying why. The client half was not, so on Windows `ensure_broker`
confirms the broker is connectable via `broker_connect_target` (src/transport/spawn.rs:305,378)
and then `connect_once` immediately tries a different, wrong endpoint — every session connect
fails on that platform. The root cause is structural: one upstream file answering "where does the
broker live" has two homes at two tree levels, and the crate-root one is the shorter, more obvious
import.

## Evidence

- src/paths.rs:90 `pub fn broker_socket_path(intercom_dir: &Path) -> PathBuf`, doc at :89 cites "`getBrokerSocketPath`, `paths.ts:65-74`; Unix branch only"; module doc src/paths.rs:6 "This module is the POSIX arm only"
- src/transport/target.rs:199 `pub fn broker_socket_path_for(platform: Platform, agent_dir: &Path) -> PathBuf` cites the same `getBrokerSocketPath` (`paths.ts:65-74`); :200-203 is the Windows named-pipe arm, :204 falls through to `crate::paths::broker_socket_path(&crate::paths::intercom_dir_path(agent_dir))`
- src/connect.rs:67 `use crate::paths::{broker_socket_path, intercom_dir_path};` and src/connect.rs:461 `let socket = broker_socket_path(&intercom_dir_path(&params.agent_dir));` inside `async fn connect_once` (declared src/connect.rs:455)
- src/connect.rs:504 `let client = Arc::new(IntercomClient::connect(&socket, registration, session_id).await?);`; `IntercomClient::connect` (src/transport/client.rs:347-353) wraps the path as `BrokerConnectTarget::Socket`, so the TCP arm is unreachable from this call site
- `grep -cE 'cfg\(unix\)|cfg\(windows\)|cfg\(not\(' src/connect.rs` = 0 — `connect_once` is not platform-gated
- Windows is a real target, not a dead branch: src/transport/stream.rs:66 dispatches `BrokerConnectTarget::Socket(path) => connect_socket(path)`, whose `#[cfg(windows)]` arm at src/transport/stream.rs:111,119 opens `path` as a named pipe via `ClientOptions::new().open(path)`; the broker binds `\\.\pipe\pi-intercom-…` through the `#[cfg(windows)]` arms of src/broker/listener.rs:44,117-119. So on Windows the client would open `<intercomDir>\broker.sock` as a pipe name while the broker listens on a pipe name that never matches.
- src/broker/lifecycle.rs:117-118: "// `paths::broker_socket_path(...)` read, which hard-coded the POSIX arm." followed by `let listen_target = crate::transport::target::broker_listen_target(&agent_dir);` — the broker half was fixed for exactly this reason; the client half was not
- src/transport/spawn.rs:305 and :378 both resolve via `target::broker_connect_target(agent_dir)`; `pub async fn ensure_broker` (src/transport/spawn.rs:55) returns `Result<()>` and hands no target back, so `connect_once` re-resolves one itself — and re-resolves it differently than the liveness check that just passed
- src/paths.rs:10-12 states both the client (`broker_connect_target`) and the broker (`broker_listen_target`) "resolve through" `crate::transport::target`; src/connect.rs does not
- `grep -rn 'ensure_connected' src/`: `connect::ensure_connected` (src/connect.rs:385 → `connect_once`) is the sole connect path for every caller — src/extension.rs:294,532,565; src/tools/intercom.rs:141; src/tools/contact_supervisor.rs:63; src/seams.rs:99,212,292
- The same POSIX-only spelling is used a second time outside `connect.rs`: src/bin/cyrup_intercom_child_fixture.rs:40 imports and :77 calls `broker_socket_path(&intercom_dir_path(&agent_dir))`

## Required fix

Make there be exactly one module answering "where does the broker live", the way `broker/` now has
one module per protocol concern. Move the transport-shaped half of the `paths.ts` port under
`transport/` — relocate `broker_socket_path`, `broker_pid_path`, `broker_spawn_lock_path` next to
`broker_port_file_path` in `transport/target.rs` (or a new `transport/paths.rs` that `target.rs`
uses), leaving `src/paths.rs` with the cyrup-home/agent-dir resolution and the runtime-dir/mode
helpers that are not transport-specific. Rename the POSIX-only spelling to something that cannot
be mistaken for the general one (e.g. `unix_socket_path`) and/or make it `pub(crate)`.
Independently of the move, fix the two call sites that picked it: change src/connect.rs:461 to
`transport::target::broker_connect_target(&params.agent_dir)` + `IntercomClient::connect_target`
(src/transport/client.rs:374), matching src/broker/lifecycle.rs:118 and
src/transport/spawn.rs:305; do the same at src/bin/cyrup_intercom_child_fixture.rs:77. Keep every
`paths.ts:NNN` citation attached to the item it moves with.

## Acceptance Criteria

- [ ] The fix above applied as written; no scope beyond it.
- [ ] Port fidelity preserved — no `broker.ts`/`client.ts`/`paths.ts` citation dropped, no
      `[CYRUP-DELTA]` note removed, no ported branch collapsed.
- [ ] Baseline recorded before the change and matched after:
      `cargo clippy -p cyrup-intercom --all-targets`, `cargo test -p cyrup-intercom --lib`.
- [ ] `cargo build -p cyrup` still succeeds.
