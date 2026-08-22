---
stage: qa
status: completed
updated: 2026-08-22 18:00
---

# Decompose agent.rs Into Submodules — QA Rework

The split is done and correct. `crates/cyrup-agent/src/agent.rs` is gone, replaced by the 14-file
[`src/agent/`](../../crates/cyrup-agent/src/agent) tree; build, clippy (3 diagnostics, all matching
the pre-split baseline), 140/140 tests, and the whole-workspace build all pass, and the moves-only
proof shows no unexplained body line. **None of that is reopened here.**

What remains is repairing what the split broke in the documentation layer, plus one banner that now
points at nothing. Every instruction below was validated against a scratch crate that reproduces the
exact privacy topology — see [Empirical validation](#empirical-validation).

## 1. Four intra-doc links (must fix)

`cargo doc --no-deps` lints **public items only**, which is why these survived the exec pass. The
full picture needs:

```bash
cargo doc -p cyrup-agent --no-deps --document-private-items
```

That reports five unresolved links under `src/agent/`. Four are regressions — each target sat in the
same module scope before the split, so each link resolved — and **one is pre-existing and must be
left alone**: `run/mod.rs`'s `` [`Agent::run`] `` names a method `Agent` has never had (the real one
is `start_run`). It was broken before the split and is not this task's business.

All four regressions live in just two files:

### `run/mod.rs` — three links, one import block

[`run/mod.rs:81`](../../crates/cyrup-agent/src/agent/run/mod.rs) (`RunCtx::new` doc) →
`` [`Agent::start_run`] ``, [`:135`](../../crates/cyrup-agent/src/agent/run/mod.rs) (`headers_for`
doc) → `` [`Agent::set_headers`] ``, [`:191`](../../crates/cyrup-agent/src/agent/run/mod.rs)
(`emit_run_failure` doc) → `` [`emit_standalone`] ``.

Insert at the top of the `use` block, keeping the file's existing alphabetical-ish grouping:

```rust
// Scope-only imports: these two names appear in intra-doc links below and resolved implicitly
// while this code lived in the same file as `Agent` and `emit_standalone`.
#[allow(unused_imports)]
use super::lifecycle::emit_standalone;
#[allow(unused_imports)]
use super::Agent;
```

`use super::Agent;` alone repairs the first two links — `Agent` is `pub` in `agent/mod.rs`, so the
`run` subtree can name it directly, and no widening is involved.

### `mod.rs` — one link

[`mod.rs:63`](../../crates/cyrup-agent/src/agent/mod.rs), the doc on `Agent`'s private `running_tx`
field → `` [`SettlementGuard`] ``. Add after the `pub(crate) use run::{…};` line:

```rust
// Scope-only import: `SettlementGuard` appears in the `running_tx` doc below and resolved
// implicitly while it lived in the same file as `Agent`.
#[allow(unused_imports)]
use lifecycle::SettlementGuard;
```

### The two widenings this requires

`emit_standalone` and `SettlementGuard` are **private to `agent::lifecycle`**, and the modules that
name them cannot reach them — privacy runs downward only:

- `agent::run` is a *sibling subtree* of `agent::lifecycle`, not a descendant.
- `agent/mod.rs` is `lifecycle`'s **ancestor**; a parent cannot see a child's private items.

So the imports above do not compile until both items are widened to `pub(super)` (= `pub(in
crate::agent)`), which is the smallest visibility that makes them nameable from anywhere under
`agent`:

| File | Line | Change |
|---|---|---|
| [`lifecycle.rs`](../../crates/cyrup-agent/src/agent/lifecycle.rs) | 34 | `async fn emit_standalone(` → `pub(super) async fn emit_standalone(` |
| [`lifecycle.rs`](../../crates/cyrup-agent/src/agent/lifecycle.rs) | 66 | `struct SettlementGuard {` → `pub(super) struct SettlementGuard {` |

Leave `SettlementGuard`'s **fields private** — nothing outside `lifecycle` constructs or reads it,
and widening the struct alone is what the link needs. These two rows are the complete addition to
the split's visibility table; **no other item's visibility changes.**

### Do not fix these by editing doc text

The `agent.ts:NNN` / `sdk.ts:NNN` offsets and the `AGENT-0NN` / `R-02-0NN` anchors are referenced by
other files and by the conformance docs. Link text stays byte-identical; only imports and the two
`pub(super)` keywords are added. The same rule produced the existing precedent at
[`prompt.rs:3-6`](../../crates/cyrup-agent/src/agent/prompt.rs), where this failure mode was already
found and fixed for `` [`Agent::prompt`] `` — match that comment style so all four sites read alike.

### The trap to avoid

The `pub(super)` fix is only free because **both link sites sit in non-public documentation**:
`running_tx` is a private field, and `RunCtx` is `pub(crate)`. If either link were moved into a
genuinely public doc, `pub(super)` would swap the unresolved-link warning for a
`private_intra_doc_links` warning ("public documentation for X links to private item Y") — confirmed
by experiment, and the reason this task specifies the imports at these exact sites rather than as a
general pattern. Do not relocate these doc comments.

## 2. The `// The public Agent` banner (must fix)

[`facade.rs:14-16`](../../crates/cyrup-agent/src/agent/facade.rs) heads `pub struct Subscription`
with a banner announcing a type that now lives in `mod.rs`:

```rust
// ---------------------------------------------------------------------------
// The public Agent
// ---------------------------------------------------------------------------

/// The detach handle [`Agent::subscribe`] returns …
pub struct Subscription {
```

Cut those three lines (plus the blank line that follows them) out of `facade.rs`, and paste the
banner into [`mod.rs`](../../crates/cyrup-agent/src/agent/mod.rs) between `use tokio::sync::watch;`
and the `/// The stateful, high-level agent front-ends…` doc, so it heads the struct it names:

```rust
use tokio::sync::watch;

// ---------------------------------------------------------------------------
// The public Agent
// ---------------------------------------------------------------------------

/// The stateful, high-level agent front-ends and extensions use (func-02 R-02-057).
pub struct Agent {
```

`facade.rs` then opens on `Subscription`'s own doc, under its `//!` header, which already says what
the file is.

Three other relocated banners were audited and are correct where they are — **leave them**:
`run/mod.rs`'s "The run context…", `run/tools/mod.rs`'s "Internal run-loop types", and `prompt.rs`'s
"Public entry-point helpers" (redundant with that file's `//!` header, but not wrong; churning it
buys nothing). A sweep for prose staleness — doc comments saying "this file", "above", "below" —
found **no** other site invalidated by the split.

## Empirical validation

Every claim above was proven in a throwaway crate at
[`tmp/docscope/`](../../tmp/docscope) that mirrors the real topology: `agent/mod.rs` holding a `pub
struct Agent` with a private field whose doc links to `SettlementGuard`; `agent/lifecycle.rs`
holding a private `emit_standalone` + `SettlementGuard`; `agent/run/mod.rs` holding a `pub(crate)`
item whose docs link to `Agent::start_run`, `Agent::set_headers` and `emit_standalone`.

- Before the fix it reproduces **exactly the four warnings** seen in the real crate.
- After the fix — the two `pub(super)` widenings and the scoped imports — `cargo doc
  --document-private-items` is **clean**, and `cargo build` / `cargo clippy` emit **no**
  `unused_imports` (the `#[allow]` covers them).
- The public-vs-private trap in §1 was found there: with the field declared `pub`, the same fix
  produces `public documentation for `state` links to private item `SettlementGuard``; with the
  field private — as `running_tx` is — it is silent.

`tmp/` is gitignored, and the crate carries its own `[workspace]` key so it never joins the
workspace. Delete `tmp/docscope/` when the rework lands.

## Expected output after the fix

```bash
cargo doc -p cyrup-agent --no-deps --document-private-items 2>&1 | grep -E '^warning'
```

Exactly **two** warnings may name `src/agent/`, both pre-existing:

1. `unresolved link to `Agent::run`` (`run/mod.rs:190`) — note the *detail* line changes wording
   once `Agent` is in scope, from "no item named `Agent` in scope" to "the struct `Agent` has no
   field or associated item named `run`". Same warning, same count; only the explanation moves.
2. `unresolved link to `crate::state::GenConfig::timeout_ms`` (`builder.rs:193`) — that path has
   never resolved; the type is `GenerationConfig`.

The four in §1 must be gone. Warnings from `loop_fn.rs` and `proxy.rs` are untouched files and stay.

## Definition of done

- [ ] `cargo doc -p cyrup-agent --no-deps --document-private-items` names no link under `src/agent/`
      except the two pre-existing ones above
- [ ] The four links resolve with their doc text byte-identical to what it is now
- [ ] Exactly two visibility changes: `emit_standalone` and `SettlementGuard` to `pub(super)`, each
      paired with the scoped import that consumes it; `SettlementGuard`'s fields stay private
- [ ] The `// The public Agent` banner heads `pub struct Agent` in `mod.rs`; `facade.rs` no longer
      carries it
- [ ] `cargo build -p cyrup-agent` clean; `cargo clippy -p cyrup-agent --all-targets` still exactly
      3 baseline diagnostics; `cargo test -p cyrup-agent` still 140 passed
- [ ] `tmp/docscope/` deleted
- [ ] No behavior change: the only edits are 3 import lines with their comments, 2 `pub(super)`
      keywords, and 3 relocated banner lines
