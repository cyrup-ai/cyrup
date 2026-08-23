---
stage: new
status: done
updated: 2026-08-22 23:07
---

# Fix All Four Rustdoc Warnings In cyrup-resources

**Owns files:** `crates/cyrup-resources/src/discovery.rs`, `crates/cyrup-resources/src/package/manifest.rs`

## Description

`cargo doc -p cyrup-resources --no-deps` emits exactly 4 warnings. All were reproduced verbatim.
Three are distinct defects; one is a two-line symptom of the third.

### 1. `discovery.rs:154` — unresolved intra-doc link (broken_intra_doc_links)

```
/// `<package_global_dir>/packages/<id>` until [`PackageStore::packages_root`] stopped doubling
```
`PackageStore` is not imported in `discovery.rs` (the file's `use crate::package::{...}` list does
not include it), so the link resolves to nothing.

**Fix:** qualify it — ``[`crate::package::PackageStore::packages_root`]``, or the display-preserving
form ``[`PackageStore::packages_root`](crate::package::PackageStore::packages_root)``.

### 2. `discovery.rs:377` and `:378` — public docs link private items

Both lines link `crate::package::install::ensure_git_ignore` and `crate::package::install::git_clone`,
which are private. rustdoc attributes both warnings to **`scope_base_dir`** — which is the tell for
defect 3 below, because neither line is about `scope_base_dir`.

### 3. `discovery.rs:367-397` — one doc run, fused across two functions (the root cause)

Verified with `sed -n '365,400p'`: an unbroken `///` run starts at **367** with
*"Resolve a settings-declared package entry to its on-disk working tree"* and continues without a
break to **397**, landing on `pub fn scope_base_dir` at **398**. Lines 367-394 describe
`resolve_configured_package` (which sits undocumented at **444**); only 395-397 describe
`scope_base_dir`.

The public API doc for `scope_base_dir` therefore renders ~30 lines about a different function.

**Fix:** split the run at 394/395. Move 367-394 (through `…, :1235).`) down to sit immediately above
`fn resolve_configured_package` at 444. Leave 395-397 as the sole doc for `scope_base_dir`. This
also silences warnings 2 automatically, since those lines move onto a private fn.

### 4. `package/manifest.rs:37` — public docs link private item

```
///   2. [`PiPackageJson`] accepts a `cyrup` key alongside pi's `pi` key.
```
`PiPackageJson` is an internal serde shape. Linking it from public docs is the defect, not its
privacy.

**Fix:** demote the link to a plain code span: `` `PiPackageJson` ``.

## Acceptance Criteria

- [ ] `cargo doc -p cyrup-resources --no-deps 2>&1 | grep -c warning` returns **0**
- [ ] `scope_base_dir`'s rendered doc describes only `scope_base_dir`
- [ ] `resolve_configured_package` carries the moved doc block
- [ ] No prose is reworded — this is a re-attachment plus two link-form changes
- [ ] `cargo test -p cyrup-resources` unchanged: `103 passed; 0 failed; 1 ignored`
