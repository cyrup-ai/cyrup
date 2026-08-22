---
stage: new
status: done
updated: 2026-08-22 18:45
---

# Fix the Stale lib.rs Front Page and Document the 13 Undocumented Wire Items

## Problem

**1. The crate front page has drifted from the export list below it.** `crates/cyrup-modes/src/lib.rs:7-16` is a four-entry bullet list — `run_print`, `run_json`, `run_rpc`, and `RpcClient` (added later, `lib.rs:14-16`) — but the closing sentence at `lib.rs:18-19` still reads "All three are adapters over the same seam". `RpcClient` is the protocol's *client*, not an adapter over the seam at all, so both the count and the claim are stale. The doc also describes only the mode entry points while `lib.rs` exports seven groups; two with real consumers get no mention on the front page: the json_event wire projection at `lib.rs:34` (`is_upstream_wire_event`, `to_json_event`, `JsonAgentSessionEvent`), which `crates/cyrup-sdk/src/lib.rs:100-103` re-exports as an embedder-facing API, and the raw_stdout writers at `lib.rs:36`, which `crates/cyrup/src/output_guard.rs:35` re-exports for the binary. A reader landing on the rustdoc page has no signal either exists.

**2. Thirteen public fields/variants on the wire types are undocumented.** The crate documents its public surface exhaustively elsewhere — every `RpcClientOptions` field carries a pi line reference (`rpc_client.rs:191-203`), `RpcResponse`'s first three fields are documented (`rpc.rs:229-235`), and `RpcOut`'s last two variants have multi-line docs (`rpc.rs:316-326`) — so the gaps are conspicuous rather than uniform, and they land on exactly the items a consumer decoding the wire reads first:

- `crates/cyrup-modes/src/rpc.rs:50`, `:51` — `QueueModeArg::All` / `::OneAtATime`; the kebab-case wire strings `"all"` / `"one-at-a-time"` are visible only in the `#[serde(rename_all)]` attribute.
- `crates/cyrup-modes/src/rpc.rs:236`, `:238`, `:240` — `RpcResponse::success` / `::data` / `::error`; the mutual exclusivity of `data` and `error` is stated only in the type-level prose, and the `skip_serializing_if` omission behaviour is undocumented per field.
- `crates/cyrup-modes/src/rpc.rs:314`, `:315` — `RpcOut::Response` / `::Event`, the two variants that are the whole point of the enum, sitting next to two that are documented.
- `crates/cyrup-modes/src/rpc_client.rs:215-218` — every field of `ModelInfo`; `context_window` is camelCase on the wire via `rename_all` with nothing saying so.
- `crates/cyrup-modes/src/rpc_client.rs:225`, `:226` — every field of `ForkMessage`; same for `reasoning`.

No `missing_docs` lint is configured — the workspace lints at `/home/user/cyrup/Cargo.toml:97-101` are the four clippy no-panic denials only — so nothing catches these.

## Fix

Doc-only; no signature or behaviour change.

1. In `crates/cyrup-modes/src/lib.rs`, reword line 18 so the invariant is scoped to the three modes (e.g. "The three modes are adapters over the same seam ..."), leaving the `RpcClient` bullet outside the claim.
2. Add two short bullets after `lib.rs:16`: one naming the json_event projection (`[to_json_event]` / `[is_upstream_wire_event]` — the shared wire projection both `run_json` and `run_rpc` write through), one naming `[write_raw_stdout]` / `[flush_raw_stdout]` (the retrying protocol-stream writer every mode's output goes through, TOOL-037).
3. Add a one-line `///` to each of the 13 items above, matching the surrounding style: for the `QueueModeArg` variants give the literal wire string; for `RpcResponse::data`/`::error` state that exactly one is present and the other key is omitted, mirroring pi's `success`/`error` helpers already cited at `rpc.rs:220-222`; for `RpcOut::Response`/`::Event` state the untagged discriminant (`"type":"response"` vs the event's own tag) already explained at `rpc.rs:300-302`; for `ModelInfo`/`ForkMessage` name the camelCase wire key each field maps to.

Optionally follow up by adding `missing_docs = "warn"` to this crate's `[lints.rust]` so the gap cannot reopen — but that is a separate decision, since it would apply to the whole crate.

Note: the crate's 9 existing rustdoc/cargo-doc warnings are owned by `.flux/todo/CARGO_DOC_WARNINGS.md`; do not fix those here, but do not introduce new ones either.

## Acceptance Criteria

- [ ] `crates/cyrup-modes/src/lib.rs:18` no longer claims "All three" over a four-bullet list; the seam invariant is scoped to run_print/run_json/run_rpc
- [ ] The lib.rs front page names the json_event projection and the raw_stdout writers
- [ ] All 13 items (rpc.rs:50, 51, 236, 238, 240, 314, 315 and rpc_client.rs:215, 216, 217, 218, 225, 226) carry a `///` doc comment
- [ ] The QueueModeArg variant docs state the literal wire strings `"all"` and `"one-at-a-time"`; the ModelInfo/ForkMessage field docs name their camelCase wire keys
- [ ] `cargo doc -p cyrup-modes --no-deps` produces no NEW warnings beyond the 9 already tracked in CARGO_DOC_WARNINGS.md
- [ ] `cargo check -p cyrup-modes --all-targets` succeeds and no public signature changed

## Source

- Identified by the cyrup-modes hygiene audit (workflow `cyrup-modes-hygiene-audit`)
- Severity: low | Size: small
