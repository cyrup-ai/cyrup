---
stage: exec
status: done
updated: 2026-08-22 16:16
---

# Decompose ctx.rs Into Submodules By Separation Of Concerns

## Description

Split [`crates/cyrup-ext-sdk/src/ctx.rs`](../../crates/cyrup-ext-sdk/src/ctx.rs) — 1,633 lines /
73 KB, the largest Rust file in the crate — into a `src/ctx/` submodule directory. Pure move
refactor: no behaviour change, no public-API change.

### The boundary principle: one submodule per WIT import interface

`ctx.rs` is not a grab bag. It is the safe-Rust front for the `cyrup:ext` world's IMPORT surface,
and every item in it belongs to exactly one WIT interface. That mapping IS the separation of
concerns, and it is already visible in the file as banner comments inside `impl Ctx`
(`// --- base-context state + lifecycle ---` at `:121`, `// --- active-tool / command
introspection ---` at `:259`, `// --- proc ---` at `:446`). Do not invent a different axis
(`types.rs` / `impls.rs` / by line count) — cut on the interface boundary.

| Submodule | WIT interface(s) it fronts | Items |
|---|---|---|
| `base.rs` | `ctx-state`, `bus`, `control.abort`/`shutdown` | `ExtMode`, `Ctx` struct, accessors, lifecycle/state getters |
| `tools.rs` | `ext-tools`, `registration` | `impl Ctx`: active-tool/command/flag/provider introspection |
| `exec.rs` | `exec` | `ExecResult`, `impl Ctx::exec` |
| `fs.rs` | `ext-fs` | `impl Ctx`: `read_file`/`write_file` |
| `http.rs` | `http-client` | `HttpRequest`, `HttpResponse`, `HttpStreamResponse`, `impl Ctx` http_* |
| `proc.rs` | `proc` | `ProcSpawnOptions`, `impl Ctx` proc_* |
| `ui.rs` | `ui` | `NotifyKind`, `Ui` + its 36 methods |
| `session.rs` | `session` | `Session`, `SessionGet`, `session_call` |
| `models.rs` | `models` | `Models` |
| `command.rs` | `control` | `CommandCtx`, `Control`, `control()` |
| `with_session.rs` | (guest-side callback registry) | `WithSessionFn`, `WITH_SESSION`, `register_with_session`, `run_with_session`, `opts_with_callback`, `ReplacedSessionContext` |
| `tool_call.rs` | `host-tool` | `Signal`, `ToolCall` |

`impl Ctx` is deliberately SPLIT across `base.rs`/`tools.rs`/`exec.rs`/`fs.rs`/`http.rs`/`proc.rs`.
Multiple inherent `impl` blocks for one type across modules of the same crate are legal, idiomatic,
and render as a single page in rustdoc. Splitting this way is what lets the private wire-conversion
helpers (`HttpRequest::to_wit`, `HttpResponse::from_wit`, `HttpStreamResponse::from_wit`,
`ProcSpawnOptions::env_json`, `NotifyKind::to_wit`) stay private to their own module instead of
being widened to `pub(super)` — do NOT widen them.

### Layout: `mod.rs`, not `ctx.rs` + `ctx/`

Use `src/ctx/mod.rs`. That is this crate's own convention
([`src/tests/mod.rs`](../../crates/cyrup-ext-sdk/src/tests/mod.rs)) and the workspace's (45 `mod.rs`
files vs. 3 sibling-file modules).

---

## Cut plan

Line anchors are against the current [`ctx.rs`](../../crates/cyrup-ext-sdk/src/ctx.rs). `impl Ctx`
opens at `:56` and closes at `:545`.

| Target file | Source lines | Approx. size |
|---|---|---|
| `ctx/mod.rs` | `1-14` (module docs), `15` (`#![allow]`), `1209-1211` (`parse_json`) + new mod/`pub use` block | ~55 |
| `ctx/base.rs` | `27-50` (`ExtMode`), `52-54` (`Ctx`), `57-119`, `121-257` | ~235 |
| `ctx/tools.rs` | `259-324` | ~70 |
| `ctx/exec.rs` | `325-352`, `547-558` | ~45 |
| `ctx/fs.rs` | `347-382` | ~38 |
| `ctx/http.rs` | `381-444`, `558-644` | ~155 |
| `ctx/proc.rs` | `446-544`, `645-673` | ~132 |
| `ctx/ui.rs` | `674-1129` | ~456 |
| `ctx/session.rs` | `1130-1207` | ~78 |
| `ctx/models.rs` | `1213-1297` | ~85 |
| `ctx/command.rs` | `1295-1506` | ~212 |
| `ctx/with_session.rs` | `1508-1580` | ~73 |
| `ctx/tool_call.rs` | `1582-1633` | ~52 |

`ui.rs` at ~456 lines stays WHOLE. It is one WIT interface and one type, and it lands in-family with
[`descriptor.rs`](../../crates/cyrup-ext-sdk/src/descriptor.rs) (452) and
[`events.rs`](../../crates/cyrup-ext-sdk/src/events.rs) (465). Sub-splitting `Ui` into
dialogs/chrome/theme/working-indicator would fragment one interface across nine ~40-line files and
break the boundary principle above. Do not do it.

Move doc comments, banner comments and `#[cfg_attr(...)]` attributes with their items, verbatim. The
`// --- proc ---` banner at `:446-450` becomes `proc.rs`'s `//!` module doc; same for the
`:121-124` and `:259` banners in `base.rs`/`tools.rs`.

---

## `ctx/mod.rs`

```rust
//! <lines 1-14 of the current ctx.rs, verbatim>
//!
//! ## Submodules
//! One per `cyrup:ext` WIT import interface — the axis every item in this module already sorts on.
//! All are private; the types are re-exported flat so `cyrup_ext_sdk::ctx::Ctx` (and every other
//! path an author or the `guest` glue already uses) resolves exactly as it did when this was one
//! file.
#![allow(clippy::needless_return)]

mod base;
mod command;
mod exec;
mod fs;
mod http;
mod models;
mod proc;
mod session;
mod tool_call;
mod tools;
mod ui;
mod with_session;

pub use base::{Ctx, ExtMode};
pub use command::CommandCtx;
pub use exec::ExecResult;
pub use http::{HttpRequest, HttpResponse, HttpStreamResponse};
pub use models::Models;
pub use proc::ProcSpawnOptions;
pub use session::Session;
pub use tool_call::{Signal, ToolCall};
pub use ui::{NotifyKind, Ui};
pub use with_session::{
    register_with_session, run_with_session, ReplacedSessionContext, WithSessionFn,
};

/// Parse a host JSON string; `Value::Null` on failure. Private to `ctx` — a child module reaches it
/// as `super::parse_json`, which is why it is not `pub(crate)`.
fn parse_json(s: String) -> serde_json::Value {
    serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)
}
```

Three things this block is load-bearing for:

1. **`#![allow(clippy::needless_return)]` propagates.** A lint-level inner attribute on `ctx` applies
   to every descendant module, including ones declared `mod ui;` with bodies in other files. Do not
   duplicate it into the submodules.
2. **Private `mod` + flat `pub use` keeps the API surface byte-identical.** `ctx::ui::Ui` was never
   reachable (it was one file); making the submodules private keeps it that way, so nothing widens.
3. **`register_with_session` / `run_with_session` / `WithSessionFn` MUST be re-exported.** They are
   not in `lib.rs`'s `pub use ctx::{…}` list — callers reach them by full path:
   [`guest.rs:396`](../../crates/cyrup-ext-sdk/src/guest.rs) calls `crate::ctx::run_with_session`,
   and [`cyrup-it/tests/ext/ergonomic.rs:458-472`](../../crates/cyrup-it/tests/ext/ergonomic.rs)
   calls both `cyrup_ext_sdk::ctx::register_with_session` and `::run_with_session`. Dropping either
   from the re-export list breaks an out-of-crate caller.

---

## Per-file imports — and the four traps that produce new warnings

The single top-of-file `use` block splits per submodule. Three of the four traps below are
`unused_imports` warnings that only appear on the HOST target (the one `cargo test` uses), so a
wasm-only check will not catch them.

**Trap 1 — `parse_json` is wasm-only at 3 of its 4 call sites.** In `tools.rs` (`:265`, `:277`,
`:294`), `ui.rs` (`:1011`, `:1024`, `:1035`) and `models.rs` (`:1232`, `:1241`, `:1257`) every call
is inside `#[cfg(target_arch = "wasm32")]`. A plain `use super::parse_json;` is then unused on the
host. **Do not** reach for `#[cfg(target_arch = "wasm32")] use …`. Call it fully qualified —
`super::parse_json(...)`, and `.map(super::parse_json)` at `:1011`/`:1035` — in every submodule,
including `session.rs` where the calls are unconditional. Zero imports, zero cfg noise, uniform.

**Trap 2 — `HashMap` in `proc.rs` is wasm-only.** `ProcSpawnOptions::env_json` (`:670-674`) is
`#[cfg(target_arch = "wasm32")]` and is the only `HashMap` user that lands in `proc.rs`. Put
`use std::collections::HashMap;` INSIDE the `env_json` body, not at file scope. (In `with_session.rs`
`HashMap` is used unconditionally by `WITH_SESSION`, so a file-scope `use` is correct there.)

**Trap 3 — `opts_with_callback` crosses a sibling boundary.** It is defined in `with_session.rs`
(`:1542`) and called from `command.rs` (`:1387`, `:1401`, `:1413`). A private item in
`ctx::with_session` is NOT visible to its sibling `ctx::command`. Change it to
`pub(super) fn opts_with_callback` — that makes it visible in `ctx` and therefore in every
descendant of `ctx`. `session_call`/`SessionGet` (used only in `session.rs`) and `control`/`Control`
(used only in `command.rs`) stay fully private, unchanged.

**Trap 4 — one doc link loses its scope.** [`ctx.rs:213`](../../crates/cyrup-ext-sdk/src/ctx.rs)
links `` [`ToolCall::is_cancelled`] `` from what becomes `base.rs`, where `ToolCall` is not in scope.
Note this target does not exist even today — `ToolCall` has `signal()`, and `Signal` has
`is_aborted()` — so it is already a `broken_intra_doc_links` warning. Fix it while moving:
`` [`Signal::is_aborted`](crate::ctx::Signal::is_aborted) ``. A `crate::ctx::…` path needs no import,
so it cannot create an unused one. Every OTHER doc link in the file resolves after the split without
help, because the type it names is already imported for a real signature or field — verified across
all 42 link sites. Use the same `crate::ctx::…` form for any link a later edit strands.

Exact import set per file:

- **`base.rs`** — `use serde::Serialize;` (`emit`); `use super::{Models, Session, Ui};`
- **`tools.rs`** — `use serde_json::Value;`; `use super::Ctx;`
- **`exec.rs`** — `use crate::descriptor::ExecOptions;`; `use super::Ctx;`
- **`fs.rs`** — `use super::Ctx;` only
- **`http.rs`** — `use super::Ctx;` only
- **`proc.rs`** — `use super::Ctx;` only (see Trap 2)
- **`ui.rs`** — `use crate::descriptor::DialogOptions;`; `use crate::widget::WidgetPlacement;`;
  `use serde::Serialize;`; `use serde_json::Value;`
- **`session.rs`** — `use serde::Serialize;`; `use serde_json::Value;`
- **`models.rs`** — `use serde_json::Value;`
- **`command.rs`** — `use crate::descriptor::{CompactOptions, ForkOptions, NavigateOptions,
  NewSessionOptions, SwitchSessionOptions};`; `use serde::Serialize;`;
  `use super::{Ctx, Models, ReplacedSessionContext, Session, Ui};`
  (`system_prompt_options` keeps its inline `serde_json::Value` path — no import)
- **`with_session.rs`** — `use core::cell::RefCell;`; `use std::collections::HashMap;`;
  `use serde::Serialize;`; `use serde_json::json;`; `use super::CommandCtx;`
- **`tool_call.rs`** — `use serde::Serialize;`; `use serde_json::Value;`; `use super::Ctx;`

`serde_json::to_string(...)` is already written as a full path at every call site — leave it.

---

## Three out-of-crate references that BREAK on the rename

`ctx.rs` is read as a FILE by three places. Two hard-break. Find them with
`rg -n 'cyrup-ext-sdk/src/ctx\.rs|include_str!\("\.\./ctx\.rs"\)' crates/`.

### 1. `src/tests/world_import_coverage.rs:33` — `include_str!("../ctx.rs")` (compile error)

[This test](../../crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs) concatenates SDK sources
at compile time and asserts every WIT import has a caller, matching on the substring
`` {module}::{name}( ``. The absolute `crate::guest::bindings::cyrup::ext::…::…(` call paths survive
the move verbatim, so the ONLY thing needed is a complete file list. `exec`, `ext-fs`, `http-client`
and `proc` are called from nowhere else in the crate, so an incomplete list fails loudly rather than
silently — but fix it properly:

```rust
const SDK_SOURCES: &str = concat!(
    include_str!("../ctx/base.rs"),
    include_str!("../ctx/command.rs"),
    include_str!("../ctx/exec.rs"),
    include_str!("../ctx/fs.rs"),
    include_str!("../ctx/http.rs"),
    include_str!("../ctx/mod.rs"),
    include_str!("../ctx/models.rs"),
    include_str!("../ctx/proc.rs"),
    include_str!("../ctx/session.rs"),
    include_str!("../ctx/tool_call.rs"),
    include_str!("../ctx/tools.rs"),
    include_str!("../ctx/ui.rs"),
    include_str!("../ctx/with_session.rs"),
    include_str!("../guest.rs"),
    include_str!("../provider.rs"),
    include_str!("../api.rs"),
    include_str!("../widget.rs"),
);
```

Keep the allowlist — do NOT swap it for a runtime walk of `src/`. The list is deliberate
("Every `.rs` file in this crate that may hold a binding call"); widening it to the whole crate
would let a doc-comment mention in `example.rs` satisfy a coverage check and weaken the test.

Then add the drift guard. This file already carries a meta-guard of exactly this shape —
`every_world_interface_is_classified`, whose rationale is "a new interface would be silently outside
the coverage test — the same 'nobody is looking' shape the test exists to catch, one level up". A
hand-maintained `include_str!` list over a DIRECTORY is that same shape, one level down:

```rust
/// `SDK_SOURCES` is a hand-maintained `include_str!` list, and `src/ctx/` is a DIRECTORY of one
/// submodule per WIT import interface. A new submodule nobody adds to the list puts its binding
/// calls outside the coverage check above — a false "unwired" failure at best. Containment is
/// checked on CONTENT, not on the file name, because `include_str!` inlines the content and only
/// the content proves the file is actually in there.
#[test]
fn every_ctx_submodule_is_in_sdk_sources() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ctx"));
    let mut missing: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dir).expect("src/ctx is a directory") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        let body = std::fs::read_to_string(&path).expect("readable source file");
        let probe: String = body.chars().take(200).collect();
        if !probe.is_empty() && !SDK_SOURCES.contains(probe.as_str()) {
            missing.push(path.display().to_string());
        }
    }
    assert!(
        missing.is_empty(),
        "these `src/ctx/` submodules are not in SDK_SOURCES, so their binding calls are invisible \
         to `every_declared_world_import_has_a_caller_in_the_sdk`: {missing:?}"
    );
}
```

### 2. `crates/cyrup-ext/src/tests/wit_world_sync.rs:223` — `cited_files()` (panic in two tests)

[`cited_files()`](../../crates/cyrup-ext/src/tests/wit_world_sync.rs) hands paths to `read()`, which
`panic!`s on a missing file (`:27-29`). It is consumed by
`no_struck_pi_citation_is_restored_as_a_live_citation` (`:278`) and
`every_subscribed_at_citation_names_the_event_pi_subscribes_on_that_line` (`:357`). Both die the
moment `ctx.rs` disappears — and the pi citations that used to be in that one file are now spread
across all thirteen submodules, so the citation lint must see all of them. Enumerate the directory:

```rust
fn cited_files() -> Vec<PathBuf> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![
        crate_dir.join("wit/world.wit"),
        crate_dir.join("../cyrup-ext-sdk/wit/world.wit"),
        crate_dir.join("../cyrup-ext-sdk/src/api.rs"),
        crate_dir.join("src/host/services.rs"),
        crate_dir.join("src/host/live.rs"),
        crate_dir.join("src/event.rs"),
        crate_dir.join("src/native.rs"),
        crate_dir.join("src/registry.rs"),
    ];
    // The SDK's `ctx` is a DIRECTORY of submodules, one per WIT import interface, and its pi
    // citations moved with the items. Enumerate rather than naming files, so a later submodule
    // cannot fall outside the citation lint by being added and not listed.
    let ctx_dir = crate_dir.join("../cyrup-ext-sdk/src/ctx");
    let mut ctx: Vec<PathBuf> = std::fs::read_dir(&ctx_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", ctx_dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    ctx.sort();
    files.append(&mut ctx);
    files
}
```

`unwrap_or_else`/`panic!`/`expect` are fine here: the file opens with
`#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]`
(`:15`), which is what keeps the workspace's `unwrap_used = "deny"` off it.

### 3. `crates/cyrup-ext/src/build/abi.rs` — no change needed (verify only)

`ABI_SOURCE_ROOTS` (`:17`) contains `"cyrup-ext-sdk/src"`, and `collect_abi_files` recurses into
subdirectories, so the new files are picked up automatically. `build.rs` emits `rerun-if-changed`
for the DIRECTORY as well as each file, so the rebuild triggers on the add/remove. The
`ABI_FINGERPRINT` VALUE changes — that is the correct and intended outcome (source moved ⇒ the
Tier-1 artifact cache key must move, EXT-028), and
`the_cache_key_tracks_the_wit_and_sdk_sources_outside_the_extension_crate` recomputes rather than
pinning a literal, so it stays green. Its one hard assertion is that
`cyrup-ext-sdk/src/guest.rs` is in the list — untouched by this task. **Do not edit `abi.rs` or
`build.rs`.** If that test reports a stale fingerprint, it is a stale build cache, not a code
defect: `touch crates/cyrup-ext/build.rs` and rebuild.

### Prose-only mentions (fix for accuracy, they break nothing)

- [`crates/cyrup-tui/src/app/execute.rs:19`](../../crates/cyrup-tui/src/app/execute.rs) — `cyrup-ext-sdk/src/ctx.rs` → `cyrup-ext-sdk/src/ctx/`
- [`crates/cyrup-ext/src/host/services.rs:300`](../../crates/cyrup-ext/src/host/services.rs) — same
- [`crates/cyrup-ext-sdk/src/example.rs:870,892`](../../crates/cyrup-ext-sdk/src/example.rs) — the bare `` `ctx.rs` `` in prose → `` `ctx/ui.rs` `` (both refer to the dialog path)

Leave `docs/gap-analysis/06-cyrup-ext.md` and `docs/adr/ADR-0002-*.md` ALONE. Those are dated,
mostly struck-through historical records with `ctx.rs:NNNN` line pins; rewriting closed entries
falsifies the record. This refactor is not their subject.

---

## Move-fidelity checks

The refactor is correct when the moved code is byte-identical modulo import lines. Two cheap proofs:

```bash
cd crates/cyrup-ext-sdk
# 1. No item was dropped. Compare the sorted set of public item names before/after.
git show HEAD:src/ctx.rs | grep -oE '^(pub (struct|enum|fn|type)|impl) [A-Za-z_]+' | sort > /tmp/before
cat src/ctx/*.rs        | grep -oE '^(pub (struct|enum|fn|type)|impl) [A-Za-z_]+' | sort > /tmp/after
diff /tmp/before /tmp/after

# 2. Every WIT binding call survived. This set must be IDENTICAL, not merely similar.
git show HEAD:src/ctx.rs | grep -oE 'ext::[a-z_]+::[a-z_0-9]+\(' | sort -u > /tmp/b
cat src/ctx/*.rs        | grep -oE 'ext::[a-z_]+::[a-z_0-9]+\(' | sort -u > /tmp/a
diff /tmp/b /tmp/a
```

Use `git mv`-free plain adds plus `git rm src/ctx.rs`; review the diff with `git diff -M40%` so
the move is legible as a move.

---

## Definition of done

- [ ] `crates/cyrup-ext-sdk/src/ctx.rs` is gone; `crates/cyrup-ext-sdk/src/ctx/` holds `mod.rs` plus
      the twelve submodules named in the table, cut on the WIT-interface boundary
- [ ] `ctx/mod.rs` re-exports all 14 public types AND `register_with_session` / `run_with_session` /
      `WithSessionFn`; every submodule is declared `mod`, not `pub mod`
- [ ] No item body changed: both move-fidelity diffs above are empty
- [ ] `opts_with_callback` is `pub(super)`; no other visibility widened — in particular `to_wit`,
      `from_wit` and `env_json` are still private to their own module
- [ ] `src/tests/world_import_coverage.rs` lists all thirteen `ctx/` files and carries the
      `every_ctx_submodule_is_in_sdk_sources` guard
- [ ] `crates/cyrup-ext/src/tests/wit_world_sync.rs::cited_files` enumerates `src/ctx/` instead of
      naming `ctx.rs`
- [ ] `cargo check -p cyrup-ext-sdk` and
      `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` are clean — no new warnings, and
      specifically zero `unused_imports` on the host target (Traps 1 and 2)
- [ ] `cargo clippy -p cyrup-ext-sdk --all-targets` is clean
- [ ] `cargo test -p cyrup-ext-sdk` and `cargo test -p cyrup-ext` pass

No third-party sources needed in `./tmp` — this is entirely in-tree.
