---
stage: new
status: done
updated: 2026-08-22 18:30
severity: medium
effort: medium
category: module-structure
---

# Decompose src/proxy.rs Into A src/proxy/ Tree Along Its Own Five Section Banners

## Description
[`crates/cyrup-agent/src/proxy.rs`](../../crates/cyrup-agent/src/proxy.rs) is 1175 lines: 803 lines of implementation plus a 372-line inline test module (`#[cfg(test)]` at L804, `mod tests` at L806). It is the largest non-test file in the crate and the only `src` file that mixes wire types, a stateful reconstruction engine, request-body projection, HTTP/SSE transport, and a trait adapter in one compilation unit.

The file already carries five section banners that partition it cleanly, with no item straddling a boundary (`grep -n '^// ---'` returns banner pairs at L30/32, L83/85, L295/297, L431/433, L705/708):

| Banner | Span | Contents |
| --- | --- | --- |
| L31 Wire protocol | L34-82 | `ProxyAssistantMessageEvent` (49 lines) |
| L84 Client-side partial reconstruction | L87-294 | `ProxyMessageBuilder` (L92), its impl (L97), `empty_partial` (L271) — 208 lines |
| L296 Options + request body | L299-430 | `ProxyStreamOptions` (L305), `ProxyRequestOptions` (L346), `ProxyThinkingBudgets` (L377), `cache_retention_wire` (L388), `build_proxy_request_options` (L398), `model_wire` (L423) — 132 lines |
| L432 Transport | L435-704 | `stream_proxy` (L446), `run_proxy` (L458), the 44-arm `status_text` table (L593), `proxy_error_message` (L657), `error_terminal` (L692) — 270 lines |
| L706 StreamFn adapter | L710-803 | `ProxyStreamFn` (L722), its impls (L728, L779), `model_thinking_to_unified` (L792) — 94 lines |

The just-completed [`src/agent/`](../../crates/cyrup-agent/src/agent/) decomposition set a 34-385 line per-file granularity (`wc -l` over `src/agent/**`: largest is `lifecycle.rs` at 385, then `run/tools/exec.rs` 337, `run/mod.rs` 276, `builder.rs` 249, `run/stream.rs` 246). proxy.rs's implementation half alone is more than 2x the largest post-decomposition file, and the whole file is 3x it — so two structural conventions now coexist in one crate with no rule a contributor can follow.

The banners are the author's own admission of where the seams are, but they do a module boundary's job with none of its enforcement: there is no privacy boundary between the reconstruction state machine and the transport, so file-private details like `ProxyMessageBuilder::set_content` (L261) and the `partial` field are freely reachable from `run_proxy`.

## Scope
IN scope: moving the contents of `src/proxy.rs` into a `src/proxy/` directory module, splitting the 16 inline tests to sit beside the code they exercise, and updating the stale `proxy.rs` path references in [`src/agent/mod.rs:21`](../../crates/cyrup-agent/src/agent/mod.rs) and `src/proxy.rs:766`.

OUT of scope, explicitly:
- Any behavior change. This is a move-only refactor; no signature, no logic, no wire format changes.
- Adding, removing, or rewriting tests. The 16 existing tests move verbatim.
- Adding doc comments or fixing rustdoc warnings — that is owned by the queued **CARGO_DOC_WARNINGS** task and must not be touched here.
- The 27 `unwrap`/`expect` calls in non-test src, and the 3 outstanding clippy diagnostics.
- `cargo fmt` churn beyond the files this task moves.

## Approach
1. Create `src/proxy/mod.rs` carrying the existing module doc (proxy.rs:1-14) verbatim, the `mod` declarations, and `pub use` re-exports. Follow the `src/agent/mod.rs` pattern: private `mod` declarations plus explicit `pub use`.
2. Split on the banners, one file per section, each keeping its banner's titled comment as the file's `//!` doc so the Pi `proxy.ts` line references survive:
   - `wire.rs` — L34-82 (`ProxyAssistantMessageEvent`)
   - `builder.rs` — L87-294 (`ProxyMessageBuilder`, `empty_partial`)
   - `options.rs` — L299-430 (`ProxyStreamOptions`, `ProxyRequestOptions`, `ProxyThinkingBudgets`, `cache_retention_wire`, `build_proxy_request_options`, `model_wire`)
   - `transport.rs` — `stream_proxy`, `run_proxy`, `error_terminal`
   - `http_status.rs` — `status_text` + `proxy_error_message`, split out because the 64-line 44-arm status table is a lookup table with no coupling to the transport control flow
   - `stream_fn.rs` — L710-803 (`ProxyStreamFn`, `model_thinking_to_unified`)
3. Keep `proxy_error_message` `pub(crate)` — [`src/tests/area02_backlog.rs:1175-1224`](../../crates/cyrup-agent/src/tests/area02_backlog.rs) calls `crate::proxy::proxy_error_message` six times, so `mod.rs` must `pub(crate) use http_status::proxy_error_message;` to keep that path resolving.
4. Re-export exactly the set [`src/lib.rs:31-33`](../../crates/cyrup-agent/src/lib.rs) already re-exports — `stream_proxy, ProxyAssistantMessageEvent, ProxyMessageBuilder, ProxyStreamFn, ProxyStreamOptions` — so `lib.rs` needs no edit. Everything else (`ProxyRequestOptions`, `empty_partial`, `status_text`, `error_terminal`, `model_wire`, …) stays private to `proxy`, tightening visibility where the single-file layout could not.
5. Move each test to the file it targets, keeping the `#[cfg(test)]` + four-lint `#[allow]` header from proxy.rs:804-805 on each new module: `wire_enum_deserializes_pi_camelcase_tags` → `wire.rs`; the seven rebuild/partial tests (L862-1009) → `builder.rs`; `request_body_serializes_pi_serializable_subset`, `request_body_omits_unset_fields` → `options.rs`; `model_thinking_lowers_to_unified`, `proxy_stream_fn_threads_thinking_budgets_into_wire_body`, `agent026_proxy_stream_fn_threads_sampling_params_into_wire_body` → `stream_fn.rs`; the two `#[tokio::test]` transport tests (L1130, L1153) → `transport.rs`. Duplicate the small shared helpers `model()`, `usage_json()`, `ev()` (proxy.rs:811-827) into whichever test modules need them rather than adding a shared test-util module — three trivial fixtures do not justify a new module.
6. Delete `src/proxy.rs`; use `git mv` for the largest section so history is preserved on at least the primary split.
7. Fix the two stale path references from step 1's scope: `src/agent/mod.rs:21` says "PORTED in `proxy.rs`" and the `proxy.rs:766` inline comment references "proxy.rs field decl".

## Acceptance Criteria
- [ ] `crates/cyrup-agent/src/proxy.rs` no longer exists; `ls crates/cyrup-agent/src/proxy/` lists `mod.rs`, `wire.rs`, `builder.rs`, `options.rs`, `transport.rs`, `http_status.rs`, `stream_fn.rs`.
- [ ] `wc -l crates/cyrup-agent/src/proxy/*.rs` shows every file at or under 385 lines (the `src/agent/lifecycle.rs` ceiling).
- [ ] `cargo test -p cyrup-agent` reports 140 passed, 0 failed — unchanged from before this task.
- [ ] `cargo clippy -p cyrup-agent --all-targets --message-format=short 2>&1 | grep -cE '^[^ ].*: (warning|error)'` returns 3 or fewer (currently exactly 3).
- [ ] `git diff --stat` on `crates/cyrup-agent/src/lib.rs` is empty — the re-export block at L31-33 needed no change.
- [ ] `grep -rn 'proxy\.rs' crates/cyrup-agent/src/` returns nothing.
- [ ] `grep -n 'pub ' crates/cyrup-agent/src/proxy/mod.rs` shows only the five `lib.rs` names as `pub`, with `proxy_error_message` re-exported as `pub(crate)`.
