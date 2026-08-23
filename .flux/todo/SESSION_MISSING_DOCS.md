---
stage: new
status: done
updated: 2026-08-23 00:47
---

# Document cyrup-session's Public API And Enable missing_docs

> Found by a six-lens hygiene audit of `crates/cyrup-session`, run after the `manager/`
> decomposition landed in PR #53. Every claim below was reproduced against the tree.
> **Priority:** medium · **Effort:** large

rustdoc's coverage report puts cyrup-session at **310/535 documented = 57.9%**. Compiling with `-W missing_docs` yields **243 warnings** inside `crates/cyrup-session`, and nothing prevents the next PR from adding more: `grep -rn missing_docs --include=*.rs --include=*.toml crates/ Cargo.toml` returns **nothing** — neither `lib.rs` (which sets only `#![forbid(unsafe_code)]`) nor `[workspace.lints]` enables the lint.

## Description

243 of cyrup-session's public items carry no doc comment (57.9% documented), and no crate in the workspace enables `missing_docs`. Document the undocumented surface, then turn the lint on so the gap cannot silently reopen.

## Where the 243 warnings are

```
 53  src/compaction/hooks.rs     (worst single file)
 40  src/entry.rs
 28  src/agent_message.rs
 15  src/error.rs
 13  src/prompt/context_files.rs
 11  src/listing.rs
  9  src/compaction/summarize.rs
  8  src/prompt/cache.rs
  8  src/manager/tree.rs
 ... 16 more files, 3-6 each
```

By kind: 157 struct fields, 51 enum variants, 27 methods, 4 associated functions, 2 constants, 1 enum, 1 struct. Module-level `//!` docs are in good shape — **zero modules flagged** — so this is purely item-level.

## Why it matters here specifically

The gap sits on the crate's most-exported types, not on obscure internals:

- `src/error.rs` is **11.8% documented**. `SessionError` (re-exported at `lib.rs:41`) has a type-level doc but **all 11 variants and all 4 of their named fields are bare** — `NotASession { path }`, `NotFound { what }`, `AmbiguousSelector { prefix, n }` render with empty descriptions.
- `src/entry.rs` is 34.4%; `Entry`/`KnownEntry` are re-exported at `lib.rs:40`.
- `src/agent_message.rs` is 42.9% and exports 7 public types at `lib.rs:35-38`.
- `CompactionError` (`compaction/error.rs:7`) is an undocumented public **enum** whose variants mostly *are* documented — the type itself is bare.
- `context.rs:23` (`COMPACTION_SUMMARY_SUFFIX`) and `context.rs:37` (`SessionContext::empty()`) are undocumented while their `PREFIX`/type siblings directly above them are documented, so rustdoc renders half of each pair blank.

## Staged plan

1. Add `#![warn(missing_docs)]` to `crates/cyrup-session/src/lib.rs` now — visible, non-blocking, stops the bleeding.
2. Clear the cheap high-visibility files first: `error.rs` (15) and `context.rs` (3). These are the ones users read in error messages and rustdoc landing pages.
3. Then the bulk: `hooks.rs` (53), `entry.rs` (40), `agent_message.rs` (28) = 121 of the 243. **Coordinate with DEAD_HOOK_SEAMS** — if `compaction/hooks.rs` is deleted there, 53 of these warnings disappear rather than needing 53 doc comments. Sequence that task first.
4. Promote to `#![deny(missing_docs)]` (crate-level or per-module) as each file reaches zero.

Do not write filler. A field whose meaning is genuinely obvious from its name and type still needs one clause saying what it means to a *caller* — if that clause cannot be written, the field probably should not be public (see API_SURFACE_NARROWING).

## Acceptance Criteria

- [ ] `crates/cyrup-session/src/lib.rs` contains `#![warn(missing_docs)]` (or stronger)
- [ ] `RUSTFLAGS='-W missing_docs' cargo check -p cyrup-session --lib --message-format=short 2>&1 | grep -c 'missing documentation'` drops from 243 to 0 for `src/error.rs` and `src/context.rs`, and the crate-wide count falls below 100
- [ ] All 11 `SessionError` variants and their named fields have doc comments, and `CompactionError` (compaction/error.rs:7) has a type-level doc
- [ ] `cargo +nightly rustdoc -p cyrup-session -- -Z unstable-options --show-coverage` reports total documented coverage above 85% (from 57.9%)
- [ ] No doc comment added is a restatement of the item's name (e.g. `/// The path.` on `path: PathBuf`); each says what the value means to a caller
- [ ] `cargo doc --no-deps -p cyrup-session` produces no new warnings and `cargo clippy --all-targets -p cyrup-session` reports 0 findings

## Evidence

```bash
cd /home/user/cyrup && RUSTFLAGS='-W missing_docs' cargo check -p cyrup-session --lib --message-format=short 2>&1 | grep 'missing documentation' | grep '^crates/cyrup-session/' | tee /tmp/cs.txt | wc -l && sed 's/:[0-9]*:[0-9]*:.*//' /tmp/cs.txt | sort | uniq -c | sort -rn | head && grep -rn missing_docs --include=*.rs --include=*.toml crates/ Cargo.toml
```
