---
stage: new
status: done
updated: 2026-08-22 23:08
---

# Absorb The rustfmt 1.9 Drift In cyrup-resources src/

**Owns files:** `crates/cyrup-resources/src/discovery.rs`,
`crates/cyrup-resources/src/package/{manifest,source,store}.rs`

> **Run this LAST**, after every other cyrup-resources hygiene task has landed. Formatting is the one
> job that touches every file, so scheduling it first guarantees conflicts and scheduling it early
> guarantees rework.

## Description

`cargo fmt -p cyrup-resources -- --check` under `rustfmt 1.9.0-stable (2026-08-18)` reports **10
hunks outside `src/tests/`**:

| File | Hunks |
| --- | --- |
| `src/discovery.rs` | 430, 952, 1793 |
| `src/package/manifest.rs` | 684, 720 |
| `src/package/source.rs` | 234, 269 |
| `src/package/store.rs` | 141, 176, 187 |

The repo was formatted with an older rustfmt; these are version-bump drift (call-argument wrapping,
attribute expansion), not sloppiness.

**Fix:** run `cargo fmt -p cyrup-resources`, then **restore everything under `src/tests/`** before
committing:

```bash
cargo fmt -p cyrup-resources
git checkout -- crates/cyrup-resources/src/tests/
```

Commit the four files alone as a formatting-only change so the churn is isolated in `git blame`.

### Why `src/tests/` is excluded

The 6 rustfmt hunks under `src/tests/resources/` are inside test bodies that were moved
**byte-for-byte** during the recent decomposition. They were confirmed pre-existing by re-running
rustfmt on the extracted functions in isolation. They are deliberately left alone so that split
stays a verifiable pure move. Reformatting them here would erase that property for no gain.

## Acceptance Criteria

- [ ] `cargo fmt -p cyrup-resources -- --check` reports diffs in **no** `src/` file outside `src/tests/`
- [ ] The 6 hunks under `src/tests/resources/` are **still present and unchanged**
- [ ] The commit contains only the four files above, and no logic change (`git diff --stat` is
      whitespace-only under `git diff -w`, which must come back empty)
- [ ] `cargo test -p cyrup-resources` unchanged: `103 passed; 0 failed; 1 ignored`
