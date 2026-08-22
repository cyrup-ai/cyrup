---
stage: new
status: done
updated: 2026-08-22 18:45
---

# Split rpc.rs's Wire-Type Layer into Its Own Module

## Problem

`crates/cyrup-modes/src/rpc.rs:45-350` is 306 lines (~170 code) of pure wire-type declarations sitting inside the 1736-line mode implementation:

- `rpc.rs:45-69` — `QueueModeArg` (`:49`), its `From` impl (`:54`), `queue_mode_str` (`:64`)
- `rpc.rs:71-215` — `SessionCommand` (enum at `:84`, last variant `Unknown` at `:214`)
- `rpc.rs:217-296` — `RpcResponse` (`:226`), hand-written `Deserialize` (`:253`), `impl RpcResponse` ok/err (`:281`)
- `rpc.rs:298-350` — `RpcOut` (`:313`), hand-written `Serialize` (`:340`)

The `// Wire types` banner at `rpc.rs:41-43` is not followed by another banner until `// Run loop` at `rpc.rs:615-617`, so it currently mislabels lines 352-613 as well.

These doc comments cite a *separate upstream file* twelve times — `rpc.rs:46, 72, 115, 135, 136, 244, 356, 363, 392, 407, 466, 1559` — e.g. `rpc-types.ts:20-72` for `SessionCommand` (`rpc.rs:71-81`), the `get_state` shape (`rpc.rs:115`, `:1559`), the dialog envelopes (`rpc.rs:363`). Upstream's `modes/rpc/` directory keeps types in `rpc-types.ts`, imported by both `rpc-mode.ts` and `rpc-client.ts` (stated in the crate's own comment at `rpc.rs:244-246`). cyrup collapsed that boundary, so the mode implementation and the shared protocol vocabulary live in one file, and `rpc_client.rs:96` has to reach into the mode module (`use crate::rpc::RpcResponse`) for a type upstream exposes from a types module.

The block has zero back-references into the run loop or the command switch — its only outward call is `crate::to_json_event` from `RpcOut`'s `Serialize` at `rpc.rs:344`, which targets the separate `json_event` module. Splitting here does not fight the 1:1-port property; it restores a file boundary the port dropped.

## Fix

Pure move, no behaviour change, no signature change.

1. Convert `src/rpc.rs` to `src/rpc/mod.rs`.
2. Move `rpc.rs:45-350` **verbatim** into `src/rpc/types.rs`, re-exported from `mod.rs` so `src/lib.rs:37` (`pub use rpc::{run_rpc, QueueModeArg, RpcOut, RpcResponse, SessionCommand};`) is unchanged.
3. Point `src/rpc_client.rs:96` at the types module.
4. Fix the `// Wire types` banner so it no longer labels the run-loop-adjacent code left behind at 352-613.
5. Optionally, in the same move, take `write_out` (`rpc.rs:1629-1644`, doc included) and `read_lines` (`rpc.rs:1646-1678`) into `src/rpc/jsonl.rs` — both cite upstream's third file, `modes/rpc/jsonl.ts` (cited in `read_lines`'s doc at `rpc.rs:1650`).

**Ordering note:** the optional `jsonl.rs` step overlaps with SURFACE_RPC_READER_IO_ERROR, which rewrites `read_lines`'s signature. Land that task first and rebase, or skip step 5 here.

Do not reflow, rename, or reword anything that moves — a reviewer must be able to confirm the move with `diff`.

## Acceptance Criteria

- [ ] `crates/cyrup-modes/src/rpc/types.rs` contains the QueueModeArg, SessionCommand, RpcResponse and RpcOut declarations, moved verbatim from the old rpc.rs:45-350
- [ ] `crates/cyrup-modes/src/lib.rs:37` is unchanged and `cyrup-modes`' public API is identical (`cargo public-api` or a manual check that `pub use rpc::{run_rpc, QueueModeArg, RpcOut, RpcResponse, SessionCommand}` still resolves)
- [ ] `src/rpc_client.rs` imports `RpcResponse` from the types module rather than the mode module
- [ ] The `// Wire types` banner no longer spans code that is not wire types
- [ ] `cargo check -p cyrup-modes --all-targets` and `cargo check -p cyrup -p cyrup-sdk -p cyrup-it` succeed
- [ ] `cargo test -p cyrup-modes` passes with the same result set as before
- [ ] `cargo clippy -p cyrup-modes --all-targets --no-deps` reports no new warnings

## Source

- Identified by the cyrup-modes hygiene audit (workflow `cyrup-modes-hygiene-audit`)
- Severity: low | Size: medium
