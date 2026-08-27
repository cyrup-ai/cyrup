---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# Surface Serialization Failures On Author-Supplied impl Serialize Values Instead Of Substituting null

**Severity:** medium · **Effort:** M · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

18 public methods take an author-supplied `impl Serialize` and swallow an encode failure into a plausible literal. `rg -n 'impl Serialize' crates/cyrup-ext-sdk/src` lists them; `rg -n 'serde_json::to_(string|value)' crates/cyrup-ext-sdk/src --glob '!src/example.rs' | rg 'unwrap_or|\.ok\(\)'` counts 34 such fallbacks. Nothing documents the substitution: `rg -ni 'serializ' crates/cyrup-ext-sdk/src/ctx crates/cyrup-ext-sdk/src/api.rs crates/cyrup-ext-sdk/src/provider.rs | rg '///'` returns 10 doc lines, none mentioning it.

**The destructive one.** `crates/cyrup-ext-sdk/src/api.rs:104`:

```rust
Outcome::Mutate(serde_json::to_value(v).unwrap_or(Value::Null))
```

reached from `Outcome::mutate` (api.rs:103), `replace_tool_input` (:114), `replace_messages` (:122), `replace_message` (:126); `Outcome::handled` (:108) has the same line. That `Null` is lowered at api.rs:148 `RawOutcome::Mutate(v.to_string())` → `"null"` → `crates/cyrup-ext-sdk/src/guest.rs:196` `HookOutcome::Mutate(s)`. Host side, `crates/cyrup-ext/src/host/live.rs:2262` `decode_outcome` parses the string — **`"null"` parses fine, so the early `return HookOutcome::Noop` at :2265 does not catch it** — and `decode_patch` (:2277-2279) returns `Some(EventPatch::ToolInput(Null))` unconditionally for `EventKind::ToolCall`. The tool's arguments are replaced with `null` and the tool runs with them.

Same shape, `"null"` across the boundary with `Ok(...)` returned: `src/ctx/session.rs:22` (`append_entry` → host returns `Ok(entry_id)` for a null-bodied entry), `src/ctx/command.rs:163` (`set_model`), `:176` (`send_message`), `src/ctx/tool_call.rs:58` (`emit_update`), `src/ctx/base.rs:85` (`emit`), `src/ctx/ui.rs:281` (`custom`), `src/provider.rs:123/151/172`. Full list: `grep -n 'to_string(&' crates/cyrup-ext-sdk/src/ctx/*.rs crates/cyrup-ext-sdk/src/provider.rs`.

**Folded-in sub-case: `CommandCtx::set_model` returns a `Result` that can never be `Err`.** `src/ctx/command.rs:162` is `pub fn set_model(&self, model: impl Serialize) -> Result<(), String>`; :163 is the infallible `to_string(...).unwrap_or_else`, :166 calls the void WIT import, :167 `return Ok(())`, :172 `Ok(())` on the host arm. `wit/world.wit:786` is `set-model: func(model-json: string);` (void) and the host drops failures at `crates/cyrup-ext/src/host/live.rs:565` (`let Ok(guest) = guest_of(self) else { return };`) and :576 (`let _ = guest.services.control(...)`). Contrast the neighbour: `wit/world.wit:796` `set-thinking-level: func(level: string) -> result<_, string>;`, whose comment says "The `result` stays: it carries a real backend failure (no session attached), not a tier refusal", propagated at `src/ctx/models.rs:76`. Two adjacent methods teach contradictory things about the same interface.

## Why it matters

`serde_json` encoding is genuinely fallible for author types — a map with a struct/enum key ("key must be a string"), a `#[serde(flatten)]` over a non-map, a hand-written `Serialize` returning `Err`. When it fails the author gets no signal and the runtime acts on the substitute. Five of these methods already return `Result<_, String>`, so the error has a free home.

## Fix

- For the five that already return `Result<_, String>` — `Session::append_entry` (src/ctx/session.rs:21), `CommandCtx::set_model` (:162), `send_message` (:175), `send_user_message` (:180), `navigate` (:134) — replace `serde_json::to_string(&x).unwrap_or_else(|_| …)` with `serde_json::to_string(&x).map_err(|e| format!("<method>: {e}"))?`. This also makes `set_model`'s `Result` carry the one failure the SDK can actually see; do not remove the signature (`src/example.rs:737` does `match ctx.set_model(target) {`).
- For `Outcome::mutate`/`handled` (api.rs:103-109), add fallible `try_mutate`/`try_handled -> Result<Self, String>` and make the infallible ones fall back to `Outcome::Noop` rather than `Mutate(Null)` — `Noop` is the only substitute that cannot corrupt the event, and `decode_outcome` already maps an undecodable mutate to Noop (live.rs:2265).
- For the notify-shaped remainder (`Ctx::emit`, `ToolCall::emit_update`, `Ui::custom`, `ProviderStream::emit`/`on_payload`/`on_response`), keep the signature but route the failure through `Ui::notify`/an `ext.log` rather than the literal, and document the substitution on each method.

No literal `module::name(` WIT call path changes, so `src/tests/world_import_coverage.rs` is unaffected. If the host-side `set_model` failure should also reach the guest, that needs `set-model: func(model-json: string) -> result<_, string>;` in `wit/world.wit:786` plus `live.rs:565` — a cross-crate change, file separately.

## Acceptance Criteria

- [ ] `grep -n 'unwrap_or(Value::Null)' crates/cyrup-ext-sdk/src/api.rs` returns nothing
- [ ] `grep -n 'try_mutate\|try_handled' crates/cyrup-ext-sdk/src/api.rs` shows both fallible constructors returning `Result<Self, String>`
- [ ] `grep -n 'to_string(&' crates/cyrup-ext-sdk/src/ctx/session.rs crates/cyrup-ext-sdk/src/ctx/command.rs` shows `map_err` on the five Result-returning methods (append_entry, set_model, send_message, send_user_message, navigate) and no `unwrap_or_else`
- [ ] Every remaining `impl Serialize` fallback in `src/ctx/` and `src/provider.rs` has a `///` line stating what happens on an encode failure (`rg -B4 'unwrap_or_else' crates/cyrup-ext-sdk/src/ctx crates/cyrup-ext-sdk/src/provider.rs` shows a doc mention above each)
- [ ] `cargo test -p cyrup-ext-sdk` and `cargo test -p cyrup-ext` both pass at their baseline counts
- [ ] `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` reports 0 warnings and `cargo clippy -p cyrup-ext-sdk --all-targets` reports 0 warnings
- [ ] `crates/cyrup-ext-sdk/src/example.rs:737`'s `match ctx.set_model(target)` still compiles unchanged
