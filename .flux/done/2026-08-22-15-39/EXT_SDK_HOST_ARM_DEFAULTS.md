---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# State The ctx Host-Target Inertness Rule And Fix Its Three Outliers

**Severity:** low · **Effort:** S · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

`crates/cyrup-ext-sdk/src/ctx/mod.rs:10-11` states the contract: "On `wasm32` each method calls the generated WIT import; on the host target (unit tests) the methods return inert defaults". The real convention is finer than that, is written nowhere, and has three outliers.

**The unwritten rule.** `rg -n -A4 'cfg\(not\(target_arch = "wasm32"\)\)' crates/cyrup-ext-sdk/src/ctx crates/cyrup-ext-sdk/src/provider.rs | rg -c 'Ok\('` → 7 and `| rg -c 'Err\("'` → 13. In practice: a `Result<(), _>` fire-and-forget op returns `Ok(())` inert (base.rs:234/246, models.rs:84, ui.rs:384, command.rs:172, command.rs:218), while anything that would have to fabricate host data returns `Err` (exec.rs:28, fs.rs:22/37, http.rs:22/45/59, proc.rs:29/43/57/72/100, session.rs:30, provider.rs:75). Defensible, but unstated, so the next method added picks a side by coin flip.

**Outlier 1 — the one `Result<data>` that fakes success.** `grep -rn -- '-> Result<' crates/cyrup-ext-sdk/src/ctx/*.rs` shows every non-`()` Result returns `Err` on the host target EXCEPT `system_prompt_options` (declared `src/ctx/command.rs:53`), whose host arm is `Ok(serde_json::Value::Null)` at `src/ctx/command.rs:60`. A host-target unit test doing `ctx.system_prompt_options()?.get("cwd")` gets `None` and passes green while asserting nothing.

**Outlier 2 — `Ctx::cwd()` reads the real environment.** `rg -n 'std::env|current_dir' crates/cyrup-ext-sdk/src` returns exactly one hit: `src/ctx/base.rs:193`, inside `Ctx::cwd()` (fn at :187). It is the sole host arm touching the runner's environment, making a host-target test's result depend on the working directory rather than the code under test, and its doc at :180-188 says nothing about the exception.

**Outlier 3 — session getters return `null` where the host returns `[]`.** `src/ctx/session.rs:78-82` is `#[cfg(not(target_arch = "wasm32"))] { let _ = which; "null".into() }` for all three variants, so `Session::entries()`/`branch()`/`tree()` (:11-19) all yield `Value::Null` through `super::parse_json`. The host's own no-session fallback differs: `crates/cyrup-ext/src/host/live.rs:520` and `:523` return `"[]"` for entries and branch, `:526` returns `"null"` only for tree. So `ctx.session().entries().as_array()` is `None` on the host target and always `Some` in the guest — an `if let Some(entries) = …` body is never exercised by a host-target test, the exact silent-skip the inert-default convention exists to prevent. Sibling collection getters already do the right thing: `src/ctx/models.rs:28` and `:37`, `src/ctx/tools.rs:29` and `:46`, `src/ctx/ui.rs:360` all default to `Value::Array(vec![])`.

No current in-tree host test hits any of these (`rg '\.cwd\(\)|session\(\)\.(entries|branch|tree)\(\)' crates/cyrup-ext-sdk/src crates/cyrup-it/tests/ext/ergonomic.rs` returns nothing, and `rg -n 'system_prompt_options' crates/cyrup-ext-sdk/src/example.rs crates/cyrup-ext-sdk/src/tests/` returns no caller) — so this is latent divergence plus a false blanket claim in mod.rs, not an active failure. It becomes an active one the moment EXT_SDK_ERGONOMIC_TESTS_DARK moves 25 host-target tests into this crate.

## Fix

1. State the rule in the module doc at `src/ctx/mod.rs:10-11`: a `Result<(), _>` fire-and-forget op returns `Ok(())` inert on the host target; anything that would have to fabricate host data returns `Err("<iface> unavailable on host target")`; collection getters return an empty collection matching the host's own no-session fallback.
2. Bring `src/ctx/command.rs:60` onto the data side — `Err("system_prompt_options unavailable on host target".into())` — or, if a host-target caller genuinely needs a bag, return the one-key `{"cwd": …}` shape the doc names.
3. In `src/ctx/session.rs`'s `session_call`, return per-variant defaults matching `crates/cyrup-ext/src/host/live.rs:520/523/526`: `"[]"` for `SessionGet::Entries` and `SessionGet::Branch`, `"null"` for `SessionGet::Tree`.
4. For `Ctx::cwd()` (src/ctx/base.rs:187), either return `String::new()` like the other string getters (`system_prompt` at base.rs:216-223, `editor_text` at ui.rs:303-310) or keep the `current_dir()` read (base.rs:193) and document the exception BOTH in the method doc and in the `src/ctx/mod.rs` contract, so the one documented exception is documented where the rule is stated.

No WIT call path changes, so `src/tests/world_import_coverage.rs` is unaffected.

## Acceptance Criteria

- [ ] `crates/cyrup-ext-sdk/src/ctx/mod.rs`'s module doc states the three-way rule (inert `Ok(())` for void ops, `Err` for fabricated data, empty-collection defaults matching the host fallback)
- [ ] `grep -n 'Ok(serde_json::Value::Null)' crates/cyrup-ext-sdk/src/ctx/command.rs` returns nothing
- [ ] `grep -n -A6 'fn session_call' crates/cyrup-ext-sdk/src/ctx/session.rs` shows per-variant host defaults `"[]"` for Entries and Branch and `"null"` for Tree, matching `crates/cyrup-ext/src/host/live.rs:520/523/526`
- [ ] `rg -n 'std::env|current_dir' crates/cyrup-ext-sdk/src` either returns nothing, or its one hit in `Ctx::cwd` is documented both at the method and in `src/ctx/mod.rs`
- [ ] `cargo test -p cyrup-ext-sdk` passes and `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` reports 0 warnings, 0 errors
