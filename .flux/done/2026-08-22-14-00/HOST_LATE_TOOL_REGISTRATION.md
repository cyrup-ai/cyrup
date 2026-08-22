---
stage: qa
status: completed
updated: 2026-08-22 14:39
---

# HA-1 / MCP-037 — COMPLETED

**Resolved 2026-08-22.** The rework below was carried out, and the scan its item 4 asked for found
three further instances of the same defect rather than one: `with_home` had lost its doc AND its
`#[must_use]` to `into_arc`, and `rebuild_command_registry` and `register_surface` had each lost a
doc block to an adjacent insertion. All four were returned to their owners, and every insertion this
branch made was re-scanned — 17 items verified, 0 attribute thefts remaining.

Root cause, recorded for the next person: an insertion anchored on an item's signature line lands
*after* that item's doc comment and attributes, because both bind to what follows. Anchor above the
doc block instead.

Final gates: `cargo check --workspace --all-targets`, `cargo check -p cyrup-ext
--no-default-features --all-targets` and `cargo check -p cyrup-it --features it --test mcp` all
clean; clippy silent on every changed file; `cargo nextest run --workspace` 7859 run / 7858 passed
with only the pre-existing `rpc_cycle_model`; `cargo doc -p cyrup-mcp --no-deps` silent on
`into_arc`/`self_weak`.

Still open by design, and owned by other units: `runtime::initialize_mcp` has no production caller
(MCP-011/MCP-030), so the mid-session demonstration cannot be run end to end; and the latent
`sync_tool_surface`/generation-swap race is unreachable while `on_session_start` is MCP-008's stub.

---

## The rework as filed — QA verdict at the time: 7/10

All five documentation items from the previous round are met and verified independently — the field
doc names `into_arc` and states the invariant, line 123 no longer singles out one coercer, the
unbound branch names the right discriminator, the four adjudicated-accurate mentions are untouched,
and `cargo doc -p cyrup-mcp --no-deps | grep -c "into_arc\|self_weak"` is `0`.

Both factual claims added in that round also check out: there is **no** bare
`Arc::new(McpExtension…)` anywhere in the tree, and the three in-crate unit tests do hold the value
directly rather than as an `Arc`.

But the round that added `into_arc` — two rounds back — inserted it **between `with_home`'s
documentation and `with_home` itself**, and nothing has caught it since. It compiles, clippy is
silent, the suite passes, and `cargo doc` does not warn.

---

## The defect

`crates/cyrup-mcp/src/extension.rs`, lines ~287-309, currently reads:

```rust
    /// Pin the home directory the config ladder's home-anchored rungs resolve against (see
    /// the `home` field). Production never calls this; a test that must be hermetic always does.
    #[must_use]
    /// Wrap into the `Arc` an extension is used as, binding the self-handle in the same step.
    ///
    /// The ONLY supported way to build an `Arc<McpExtension>`. …
    pub fn into_arc(self) -> Arc<Self> {
        let ext = Arc::new(self);
        let _ = ext.self_weak.set(Arc::downgrade(&ext));
        ext
    }

    pub fn with_home(mut self, home: PathBuf) -> Self {
```

Attributes and doc comments both bind to the *following* item, so all of it landed on `into_arc`:

1. **`with_home` lost its documentation.** The "Pin the home directory…" paragraph now documents
   `into_arc`, which does not pin anything. Rustdoc concatenates the two blocks, so `into_arc`'s
   rendered page opens by describing a different method.
2. **`with_home` lost its `#[must_use]`.** It is a consuming builder returning `Self` — dropping
   the result silently does nothing, which is precisely what the lint exists to catch. Both its
   siblings still carry it (`new` at :257, `with_config` at :264); `with_home` is now the only one
   without.
3. **`into_arc` carries a `#[must_use]` it was never given.** Two rounds ago rustc warned "unused
   attribute" on the one I wrote for `into_arc`; I removed mine and left this one, never asking why
   a second existed. That warning was the tell, and I misread it.

## Required change

Restore the two lines to `with_home` and leave `into_arc` with only its own doc:

```rust
    /// Wrap into the `Arc` an extension is used as, binding the self-handle in the same step.
    ///
    /// The ONLY supported way to build an `Arc<McpExtension>`. …
    /// One constructor cannot diverge.
    pub fn into_arc(self) -> Arc<Self> {
        let ext = Arc::new(self);
        // Infallible: `ext` was created on the line above, so nothing else holds the `OnceLock`.
        let _ = ext.self_weak.set(Arc::downgrade(&ext));
        ext
    }

    /// Pin the home directory the config ladder's home-anchored rungs resolve against (see
    /// the `home` field). Production never calls this; a test that must be hermetic always does.
    #[must_use]
    pub fn with_home(mut self, home: PathBuf) -> Self {
```

Do **not** add a `#[must_use]` to `into_arc` while doing so: `Arc<T>` already carries it, which is
why rustc called the explicit one redundant.

## Definition of done

1. `with_home` has its doc comment and its `#[must_use]` back.
2. `into_arc` has exactly one doc block — its own — and no `#[must_use]`.
3. `cargo doc -p cyrup-mcp --no-deps` still emits nothing for `into_arc`/`self_weak`, and
   `into_arc`'s rendered doc no longer opens with the home-directory paragraph.
4. No other method in this `impl` block lost an attribute or doc to an insertion — scan the whole
   block, not just this pair.

## Verification

- `cargo check --workspace --all-targets`
- `cargo check -p cyrup-ext --no-default-features --all-targets`
- `cargo check -p cyrup-it --features it --test mcp`
- `cargo clippy --workspace --all-targets` — silent on every changed file
- `cargo nextest run --workspace` — 7858/7859, `rpc_cycle_model` excepted
- `cargo doc -p cyrup-mcp --no-deps 2>&1 | grep -c "into_arc\|self_weak"` — `0`

None of these caught the defect, which is the point: **verify item 1 by reading the two functions,
not by running a gate.**

## What this round changes about the pattern

The previous four findings were all stale prose — comments that were true when written. This one is
different and worse: an edit that silently moved an attribute and a doc block off the function they
belonged to. The lesson is not "grep the old home" but a mechanical one about how these edits are
being made:

**An insertion anchored on a `pub fn` line lands after that function's doc comment and attributes,
not before them.** Anchor on the blank line preceding the doc block, or verify the three lines above
and below every insertion point afterwards.

## Out of scope, unchanged

The latent `sync_tool_surface` / generation-swap race (unreachable while `on_session_start` is
MCP-008's stub) and `runtime::initialize_mcp` having no production caller (MCP-011/MCP-030).
