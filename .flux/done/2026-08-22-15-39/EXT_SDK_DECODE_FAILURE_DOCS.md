---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# Stop Collapsing Host-To-Guest Decode Failures Into The Documented "Absent/Unchanged" Answer

**Severity:** low · **Effort:** M · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

Three host→guest decode paths silently fold an unparseable payload into a value the doc gives a different meaning to.

**1. `ProviderStream::on_payload` (`crates/cyrup-ext-sdk/src/provider.rs:150`).** Its doc at :130-147 quotes the must-invoke contract and states `None` = unchanged; the body ends `.and_then(|s| serde_json::from_str(&s).ok())` at `provider.rs:158`, so a parse failure is indistinguishable from "unchanged". The whole point of the method (EXT-M05, provider.rs:139-145) is letting a redaction/audit extension rewrite an outbound provider request — the swallow means the un-redacted original goes out with no error anywhere.

**2. `Ctx::get_flag` (`crates/cyrup-ext-sdk/src/ctx/tools.rs:52`).** The doc at :51 says "`None` when the flag is unregistered or has no value" — an explicit enumeration. The wasm arm ends `.and_then(|s| serde_json::from_str(&s).ok())` at :56, adding a third, undocumented `None` case.

**3. `ctx::parse_json` (`crates/cyrup-ext-sdk/src/ctx/mod.rs:65`).** The `Null`-on-failure behaviour IS documented at the definition (`src/ctx/mod.rs:62-63`, "Parse a host JSON string; `Value::Null` on failure"), but none of its 12 call sites say so. `rg -n 'super::parse_json' crates/cyrup-ext-sdk/src/ctx` → tools.rs:14/26/43, ui.rs:344/357/368, models.rs:25/34/50, session.rs:12/15/18. Each of those getters documents `Null`/empty as meaning "absent", not "unparseable".

See also `crates/cyrup-ext-sdk/src/guest.rs:416` for the same shape at the guest boundary.

## Why it matters

On every one of these paths the producer is cyrup-ext's own host, which serializes a `serde_json::Value` before handing the string back (`crates/cyrup-ext/src/host/live.rs`), so the unparseable branch is defensive-only and practically unreachable — this is a doc-accuracy defect, not a live bug. But the house standard is that an inaccurate comment is worse than none, and two of these doc comments enumerate exactly what `None` means while the code has an extra, silent meaning.

## Fix

1. Change `ProviderStream::on_payload` (`src/provider.rs:150`) to return `Result<Option<Value>, String>`, using `.transpose()` over `serde_json::from_str(&s).map_err(...)`, so a parse failure is a hard error the provider must handle rather than "unchanged". Changing the Rust return type does not alter the literal `provider_stream::on_payload(` call, so `src/tests/world_import_coverage.rs` still passes.
2. For `Ctx::get_flag` (`src/ctx/tools.rs:52`) either widen to `Result<Option<Value>, String>` or amend the doc at tools.rs:51 to state the third case explicitly.
3. Keep `parse_json`'s `Null` fallback, and add to the doc of each of the 12 getters listed above that a returned `Null`/empty also covers "the host sent JSON this SDK could not parse".

If EXT_SDK_CTX_MISSING_DOCS is being done around the same time, fold step 3 into that pass.

## Acceptance Criteria

- [ ] `grep -n -A12 'fn on_payload' crates/cyrup-ext-sdk/src/provider.rs` shows a `Result<Option<Value>, String>` return and a `map_err`, with no `.ok()` swallow
- [ ] `crates/cyrup-ext-sdk/src/ctx/tools.rs`'s `get_flag` either returns a `Result` or its doc enumerates the unparseable-JSON case alongside unregistered/no-value
- [ ] Each of the 12 `super::parse_json` call sites (`rg -n 'super::parse_json' crates/cyrup-ext-sdk/src/ctx` → tools 14/26/43, ui 344/357/368, models 25/34/50, session 12/15/18) has a `///` on its enclosing method mentioning the unparseable case
- [ ] `cargo test -p cyrup-ext-sdk` passes and `cargo test -p cyrup-ext` passes at 293
- [ ] `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` reports 0 warnings, 0 errors and `cargo clippy -p cyrup-ext-sdk --all-targets` reports 0 warnings
