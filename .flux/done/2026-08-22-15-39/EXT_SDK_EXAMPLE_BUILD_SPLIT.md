---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# Split example.rs's 937-Line build() Into Per-Concern Installer Modules

**Severity:** medium · **Effort:** L · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

`crates/cyrup-ext-sdk/src/example.rs` is 1015 lines and a single function is 937 of them. `sed -n '79p;1015p' crates/cyrup-ext-sdk/src/example.rs` → `pub fn build() -> ExtensionApi {` at :79 and the file's final `}` at :1015. `awk 'NR>79 && /^[a-zA-Z#]/' crates/cyrup-ext-sdk/src/example.rs` returns nothing — there is no other top-level item after `build()` opens.

Inside it: `grep -c '^    api\.' crates/cyrup-ext-sdk/src/example.rs` → 51 top-level registrations, of which `grep -c '^    api\.register_command'` → 26 are commands.

The ordering is chronological-by-gap-closed, not by concern. `grep -n '^    api\.'` shows: event hooks at :83, :121, :138, :154, :163 — then a `register_command` wedged at :184 — then hooks again :203-:323; commands :355-:732; renderers :752-:762; provider :795; autocomplete :829; commands again :838-:927; shortcut :945; flag :960; commands again :964-:999; bus :982.

Corroborating symptom: the module doc at `example.rs:6-7` still says the file "demonstrates: a `tool_call` permission gate (block), a notify hook (`agent_start`), and a dynamically-registered streaming tool (`demo_echo`)" — three of the 51 registrations. The same accretion is already showing up as a stale comment.

## Why it matters

`lib.rs:15` documents `example` as the bundled reference extension and it is a `pub mod` (`lib.rs:26`), so an author reading the reference must scan a 937-line body to find any one seam — "where are this extension's event handlers" is unanswerable without reading the whole function. Every new seam demo appends to the same function, so it grows monotonically and any edit touches a 937-line body. This is the same accretion `ctx.rs` had before its decomposition, one level down (a function instead of a file).

Nothing is broken and no lint fires — this is pure structural debt, and the fix is pure motion with real blast radius: host tests in `crates/cyrup-ext` and `crates/cyrup-it` read this fixture's registrations by name, so registration order and names must be preserved exactly.

## Fix

Convert to `src/example/` with `mod.rs` holding only:

```rust
pub fn build() -> ExtensionApi {
    let mut api = ExtensionApi::new();
    hooks::install(&mut api);
    tools::install(&mut api);
    commands_capability::install(&mut api);
    commands_ui::install(&mut api);
    commands_session::install(&mut api);
    renderers::install(&mut api);
    provider::install(&mut api);
    wiring::install(&mut api);
    api
}
```

Cut lines (all already delimited by the existing banner comments):

- `hooks` = :82-330 (`on_tool_call` … `on_session_before_tree`), moving the stray `register_command` at :184 out
- `tools` = :331-353 + :848-858
- `commands_capability` = :413-748 (execdemo/httpdemo/fswrite/fsread/httpstream/proc*)
- `commands_ui` = :856-940 (dialog/confirm/input/select/editor demos) + the shortcut at :941-954
- `commands_session` = :354-412 + :708-748 + :837-847 + :996-1012
- `renderers` = the four `struct Demo*Renderer` at :23-77 (declared :23/:40/:56/:70, impls ending :77) plus :750-762
- `provider` = :764-826 + :827-836
- `wiring` = :955-995 (flag + bus)

**Registration order within `build()` must be preserved verbatim** — move blocks unchanged and keep the installer call order identical to the current top-to-bottom order. Also refresh the module doc at :6-7 so it describes the module set rather than three of 51 registrations.

If EXT_SDK_MODELS_SET_MODEL lands first, the raw `crate::guest::bindings::…::set_model` block at :251-260 will already be gone; otherwise carry it into `hooks` unchanged.

## Acceptance Criteria

- [ ] `crates/cyrup-ext-sdk/src/example/mod.rs` exists and its `build()` body is under 15 lines; `crates/cyrup-ext-sdk/src/example.rs` no longer exists
- [ ] The ordered sequence of registrations is byte-identical before and after: capture `git show HEAD:crates/cyrup-ext-sdk/src/example.rs | grep -oE '^    api\.[a-z_]+\("[^"]*"?' > /tmp/before.txt`, then concatenate the same grep over the new installer files in installer-call order and `diff` the two — no differences
- [ ] `cargo test -p cyrup-ext` passes at 293 and `cargo test -p cyrup-ext-sdk` passes at its baseline count
- [ ] `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` still links and emits the component
- [ ] `cargo clippy -p cyrup-ext-sdk --all-targets` reports 0 warnings
- [ ] The module doc of `src/example/mod.rs` describes the installer modules rather than naming three individual registrations
