---
stage: qa
status: completed
updated: 2026-08-22 18:55
---

# Decompose `cyrup-permission-system/src/extension.rs` — QA rework (round 2)

## QA verdict: 9 / 10 — one factual error to correct

Five and a half of the six rework items are done to production quality and **must not be
reopened**. Independently re-verified:

- **Item 1** — every test module now has exactly one `use cyrup_ext::…`, merged alphabetically into
  the third-party group, one blank line between groups. No duplicates anywhere.
- **Item 2** — the dossier banner is gone from `tests/support.rs`; the Wave1b pi-parity provenance
  survives in the module doc, repointed at `src/extension/`.
- **Item 3** — both stray separators are gone; no `// ====` banner sits at the top of any file in
  the tree. `enabled_switch.rs` kept its `extension-config.ts:11-12,88 → index.ts:1473-1477`
  citation in the module doc, and `install.rs`'s module doc still says "binary wiring entry point".
- **Item 4** — 18 of the 19 path tokens are correct, and each destination was checked against where
  the symbol actually lives. Ten paragraphs were re-wrapped after their swap; all read correctly,
  with no word added, dropped or altered, list indentation intact, and no paragraph merged into its
  neighbour. Both DoD greps come back empty, including the workspace-wide one. `docs/**` untouched.
- **Item 5** — `mod.rs` reads doc → imports → `#[cfg(doc)]` imports → `mod` declarations →
  `use warnings::WarningSink;` → re-exports → struct → `guard`, exactly as specified.
- **Item 6** — `.flux/todo/PERMISSION_TEST_ENV_LOCK.md` exists; all six tests it names were verified
  to exist at the stated paths and to hold no env lock.
- `cargo check --all-targets` clean, **0** clippy findings under `src/extension/`,
  `cargo check -p cyrup-ext-subagents -p cyrup-session-svc` clean, `cargo doc` still 29 warnings
  **identical to the pre-refactor baseline**.

One reference in the sweep names a module that does not do what the sentence says.

## The one outstanding item — `status.rs:5` names a module that does not drive the pill

`crates/cyrup-permission-system/src/status.rs` now ends its module doc with:

```rust
//! `session_start`/`before_agent_start` and cleared at `session_shutdown` (pi `index.ts:2122`),
//! from `extension/config.rs`, `extension/agent_start.rs` and `extension/native.rs`.
```

`extension/agent_start.rs` **never touches the status pill**. The complete set of `status::` call
sites in the extension tree is:

| Call | Site |
| ---- | ---- |
| `status::sync_status(services, config)` | `extension/config.rs:167`, inside `sync_status_when_possible` |
| `status::clear_status(s)` | `extension/native.rs:267`, the `session_shutdown` arm |

`agent_start.rs` contains no `status::` call at all, and its own doc says so in as many words:
"The status pill is NOT synced here: pi's `before_agent_start` reaches it through
`refreshExtensionConfig(ctx)` … which cyrup's `BeforeAgentStart` arm now calls before this
function". The `before_agent_start` sync the sentence describes happens because
`extension/native.rs`'s `BeforeAgentStart` arm calls `refresh_extension_config`
(`extension/config.rs`), which calls `sync_status_when_possible`.

**Fix** — drop the middle module, keeping every other word:

```rust
//! `session_start`/`before_agent_start` and cleared at `session_shutdown` (pi `index.ts:2122`),
//! from `extension/config.rs` and `extension/native.rs`.
```

Re-wrap only if the edited line exceeds 100 **characters** (not bytes — `—` and `→` in these files
are multi-byte, and `awk`'s `length()` over-counts them).

### Why this slipped through — do not "fix" the cause

The relocated doc in `extension/agent_start.rs` contradicts itself: line 31 says "Also syncs the
`\"yolo\"` status pill (pi `syncPermissionSystemStatus`, `:2136`)" and line 38 says "The status pill
is NOT synced here". **Both sentences are pre-existing** — they sit at lines 2044 and 2051 of the
original single-file `extension.rs` — so they are out of scope for this task and for the
decomposition, which must not edit relocated prose. Leave them. They are only noted here so the
fixer trusts the call sites over the prose.

## Definition of done

```bash
cd /home/user/cyrup
grep -n 'agent_start' crates/cyrup-permission-system/src/status.rs   # empty
cargo check -p cyrup-permission-system --all-targets                 # clean
cargo doc   -p cyrup-permission-system --no-deps                     # still 29 warnings
```

- `status.rs` is the only file that changes.
- No other reference from round 1 is touched; no relocated doc inside `src/extension/` is edited.
