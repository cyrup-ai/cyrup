---
title: Branch Leaves Rustfmt Violations In Its Own New Lines
priority: LOW
stage: qa
status: completed
updated: 2026-08-23 08:32
---

# Format the six files this branch took from rustfmt-clean to rustfmt-dirty

**Owns files** (all six, and nothing else). Hunk counts re-measured on the current tree with
`rustfmt 1.9.0-stable`, not inherited from an earlier pass:

| File | Hunks | Lines now | Lines after |
| --- | ---: | ---: | ---: |
| [`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs) | 2 | 408 | 414 |
| [`crates/cyrup-config/src/settings/manager.rs`](../../crates/cyrup-config/src/settings/manager.rs) | 10 | 538 | 554 |
| [`crates/cyrup-config/src/settings/tests/merge_and_scope.rs`](../../crates/cyrup-config/src/settings/tests/merge_and_scope.rs) | 5 | 510 | 515 |
| [`crates/cyrup-config/src/settings/tests/write_refusal.rs`](../../crates/cyrup-config/src/settings/tests/write_refusal.rs) | 10 | 320 | 340 |
| [`crates/cyrup-config/src/trust.rs`](../../crates/cyrup-config/src/trust.rs) | 4 | 762 | 775 |
| [`crates/cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) | 3 | 126 | 131 |
| **Total** | **34** | | |

> ### Correction — this task shrank from seven files to six
>
> An earlier revision of this spec listed **seven** files / **37** hunks and made
> `crates/cyrup-config/src/lock.rs` the interesting one ("the one non-whitespace edit … the
> `keyed_lock` import reorders below `CancelToken`"). **That is now stale.** On the current tree
> `crates/cyrup-config/src/lock.rs` is already rustfmt-clean:
>
> ```
> $ rustfmt --check --color never --edition 2024 --config skip_children=true \
>     crates/cyrup-config/src/lock.rs ; echo $?
> 0
> ```
>
> Its `use cyrup_core::keyed_lock::{KeyedGuard, KeyedLockMap, KeyedLocks};` already sits *below*
> `use cyrup_core::CancelToken;`, and its two `ConfigError::Lock { … }` reflows are already applied.
> The predicted `+11 / −4` for that file landed with the config-lock work that is now in `main`.
> **Do not add `crates/cyrup-config/src/lock.rs` back to the command line** — it is in scope for
> nothing here, and passing it is merely a no-op that muddies the change set.
>
> Consequence: **there is no import reorder left in this task.** All 34 remaining hunks are pure
> whitespace + trailing commas, in all six files. Every "one `use` line moves, verify it by eye"
> caveat from the old spec is deleted.

> ### Run this LAST
>
> All six files are owned by other queued tasks. Formatting touches the exact lines everybody else
> is editing, so scheduling it early guarantees rework — the same rule the repo already applied in
> [`_backlog/RESOURCES_RUSTFMT_DRIFT.md`](_backlog/RESOURCES_RUSTFMT_DRIFT.md).
>
> **Two hard ordering edges, both against
> [`MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md`](./MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md).**
> That task must land **before** this one, or its own definition of done becomes uncheckable:
>
> - its DoD #5 requires `rustfmt --edition 2024 --check crates/cyrup-tools/src/lock.rs` to print
>   **exactly two** `Diff in` headers — the two this task removes;
> - its DoD #4 requires `sha256sum crates/cyrup-core/src/keyed_lock.rs` to print
>   `eee73e6e3cb5ecb44dc5569a20e06e731390366d7aa1bdc879d6d1b7bcb10794` — the *unformatted* hash,
>   which this task changes to `67d75688671dcd09e02f78d6b8f692c4e286293e26397c3c95f6bf420afe4328`.
>
> Running in the correct order costs nothing: that task adds only `///` lines and grows
> `crates/cyrup-tools/src/lock.rs` from 408 to 429 lines, and **neither of this task's two
> `lock.rs` edits carries a line number** — both are anchored on unique body text that its diff
> does not touch. Verified: its pinned `mod tests` region hash
> `92342984087deeb02ce3383288b4627d52b8d0d520bab1fe9d5db65c5ceba3b0` is **byte-identical before and
> after** this task's formatting, so the two tasks do not collide.

## Description

Two review findings, one root cause. `fe86c7f` ran `cargo fmt` over `cyrup-tools`, and separately
drove a compiler-span script that appended `.await` textually across `cyrup-config` and
`cyrup-core`. Neither pass reformatted what it had just written. What survives today is 34 rustfmt
hunks sitting entirely on lines this branch authored, in six files.

The job is to format exactly those six files. It is not to run `cargo fmt`.

## Measurement — re-verified on the current tree

Toolchain: `rustfmt 1.9.0-stable (88d9e12ae1 2026-08-18)`, the pinned `stable` channel
([`rust-toolchain.toml`](../../rust-toolchain.toml), `components = ["rustfmt", "clippy"]`). There is
no `rustfmt.toml` or `.rustfmt.toml` anywhere in the tree, so these are stock defaults at
`edition = "2024"` ([`Cargo.toml:88`](../../Cargo.toml), under `[workspace.package]`).

Current package-level dirt (`cargo fmt -p <pkg> -- --check`, counting `^Diff in` headers):

| Package | Dirty hunks now | Owned by this task |
| --- | ---: | ---: |
| `cyrup-tools` | 2 | 2 (all of them) |
| `cyrup-config` | 29 | 29 (all of them) |
| `cyrup-core` | 49 | 3 |
| `cyrup` | 174 | 0 |
| `cyrup-session-svc` | 1062 | 0 |
| `cyrup-tui` | 2711 | 0 |
| `cyrup-ext-subagents` | 3038 | 0 |
| workspace (`cargo fmt --all -- --check`) | **13199** | **34** |

Two of those numbers are the whole argument against a blanket format:
`cargo fmt -p cyrup-ext-subagents` would rewrite 3038 hunks; a single file inside it is no safer,
`discovery/management.rs` alone is 165 hunks, most of them older than the branch.

### The scope rule

> Format a touched file **iff every rustfmt hunk it still has sits on a line this branch wrote.**

That rule selected these six and absorbs zero pre-existing drift by construction. It is the reason
`crates/cyrup-tools/src/lock.rs` is in scope despite having been dirty before the branch (the
branch's own fmt pass cleaned it; the two survivors are both branch-written), and the reason
`crates/cyrup-core/src/lib.rs` is out of scope despite being touched (its 3 hunks all predate the
branch).

**Re-check the set without git** — the frozen six are exactly reproducible from per-package
`--check` output, because the dirt in two of the three packages is already 100% this task's:

```bash
cargo fmt -p cyrup-tools -- --check --color never 2>/dev/null \
  | grep '^Diff in' | sed 's|:[0-9]*:$||' | sort | uniq -c
#   2 .../crates/cyrup-tools/src/lock.rs                      <- the whole package

cargo fmt -p cyrup-config -- --check --color never 2>/dev/null \
  | grep '^Diff in' | sed 's|:[0-9]*:$||' | sort | uniq -c
#  10 .../settings/manager.rs
#   5 .../settings/tests/merge_and_scope.rs
#  10 .../settings/tests/write_refusal.rs
#   4 .../trust.rs                                            <- the whole package

cargo fmt -p cyrup-core -- --check --color never 2>/dev/null \
  | grep -c 'keyed_lock.rs'
#   3                                                          <- of 49 in the package
```

If those three commands still print exactly that, the frozen list holds and the exact edits below
apply byte-for-byte. If a sibling task changed one of the six first, the *rule* is authoritative and
the byte-exact edit list is not — fall back to the `rustfmt` command in **Required path**, which
re-derives the correct output from whatever the file now says, and skip the input-hash precondition
in the definition of done.

## Required path

One command, from the repo root. Nothing else formats anything.

```bash
rustfmt --edition 2024 --config skip_children=true \
  crates/cyrup-tools/src/lock.rs \
  crates/cyrup-config/src/settings/manager.rs \
  crates/cyrup-config/src/settings/tests/merge_and_scope.rs \
  crates/cyrup-config/src/settings/tests/write_refusal.rs \
  crates/cyrup-config/src/trust.rs \
  crates/cyrup-core/src/keyed_lock.rs
```

This is the required mechanism, not a suggestion, and it is *proven* equivalent to the exact edit
list in **Exact edits** below: applying those 33 replacements to scratch copies of the six files and
re-running `rustfmt --check` on the result exits `0` with empty stdout and empty stderr. Use the
edit list to review the change or to reproduce it by hand; use the command to produce it.

Every part of the command is load-bearing:

- **`rustfmt`, not `cargo fmt`.** `cargo fmt` has no file granularity; its smallest unit is `-p`.
  It happens to be true *today* that `cargo fmt -p cyrup-tools -p cyrup-config` would produce an
  identical result — the dirt in both packages is exactly this task's 2 + 29 — but that is a
  coincidence of the current queue state, and pending tasks edit those packages. It is never true
  for `cyrup-core`: `cargo fmt -p cyrup-core` drags in 46 unrelated hunks to fix `keyed_lock.rs`'s 3.
- **`--edition 2024` is mandatory, and omitting it fails silently on stdout.** Invoked directly,
  rustfmt defaults to edition 2015, cannot parse `async fn`, and writes to stderr:
  ```
  error[E0670]: `async fn` is not permitted in Rust 2015
  ```
  In `--check` mode that presents as **zero** `Diff in` lines — a clean bill of health for a file it
  never read — while the exit code is `1`. `cargo fmt` passes the edition from the manifest, which
  is why this failure mode only exists here. Verified on the current tree.
- **`--config skip_children=true`** stops rustfmt descending into `mod foo;` declarations. All six
  files are leaf modules today, so it is currently a no-op — but without it, pointing rustfmt at a
  file that later gains a child module reformats the child too. Verified accepted on stable 1.9.0:
  it is absent from `--help`, yet an unknown key is rejected outright (`--config bogus_option=true`
  → `invalid key=val pair`), and on `crates/cyrup-config/src/settings/mod.rs` it takes the reported
  hunk count from **25 to 0**.
- **No `--backup`.** It would litter `*.rs.bk` files.

## What changes

Expected: **+150 / −85** across the six files, 34 hunks.

```
crates/cyrup-tools/src/lock.rs                              +8   -2
crates/cyrup-config/src/settings/manager.rs                +76  -60
crates/cyrup-config/src/settings/tests/merge_and_scope.rs  +10   -5
crates/cyrup-config/src/settings/tests/write_refusal.rs    +31  -11
crates/cyrup-config/src/trust.rs                           +17   -4
crates/cyrup-core/src/keyed_lock.rs                         +8   -3
```

**Nothing but reflow and trailing commas.** All six files are byte-identical to their formatted
output once whitespace and commas are stripped — measured, not asserted; the per-file stripped
hashes are in the definition of done. No token is added, removed, reordered, or renamed. No import
moves.

Where the churn concentrates:

- `settings/manager.rs` — four `})` terminators where `.await?` was glued onto a
  `store.with_lock(scope, &mut |current| { … })` call, which forces the whole closure body to
  re-indent by four columns. Those four are 60 of the 76 added lines. By name: the bodies of `set`,
  `set_nested`, `persist_nested`, and `set_enable_analytics`. The remaining six are convenience
  setters whose `.await` pushed the call or the signature past the width limit:
  `set_mermaid_rendering_mode`, `set_editor_padding_x`, `set_autocomplete_max_visible` (its
  signature is 101 columns and splits three ways), `set_image_width_cells`,
  `set_http_idle_timeout_ms`, `set_show_images`.
- `settings/tests/merge_and_scope.rs` and `settings/tests/write_refusal.rs` — uniformly
  `.await.unwrap()` and `).await` chains the script left mid-line.
- `trust.rs` — `TrustStore::set`'s signature (103 columns once `async ` went on) plus three
  `.await.unwrap()` / `matches!(…)` chains in `mod tests`.
- `keyed_lock.rs` — the `PendingEntry { … }` literal in `KeyedLocks::guard`, and the identical
  `self.map.remove_if(…)` line in `KeyedGuard::drop` and in `PendingEntry::drop`.
- `cyrup-tools/src/lock.rs` — the bodies of `FileMutationLocks::new` and `FileMutationLocks::guard`,
  the two functions this branch rewrote and the only rustfmt violations left in the whole package.

## Exact edits

The complete, byte-verified change. Each **Find** block was checked against the file on disk and
**matches exactly the stated number of times**; 32 of the 33 match exactly once. Apply verbatim —
these are the bytes the required command produces.

### `crates/cyrup-tools/src/lock.rs` — 2 hunks, 2 edits

**1.** Match count **1**.

Find:

```rust
        Self { inner: KeyedLocks::new(Arc::clone(&map)), map }
```

Replace with:

```rust
        Self {
            inner: KeyedLocks::new(Arc::clone(&map)),
            map,
        }
```

**2.** Match count **1**.

Find:

```rust
        self.inner.guard(key, cancel).await.map_err(|_| error::aborted())
```

Replace with:

```rust
        self.inner
            .guard(key, cancel)
            .await
            .map_err(|_| error::aborted())
```

### `crates/cyrup-config/src/settings/manager.rs` — 10 hunks, 9 edits

**1.** Match count **1**.

Find:

```rust
        self.store.with_lock(scope, &mut |current| {
            let mut doc = match current.map(Settings::parse) {
                Some(Ok(s)) => s,
                // Absent file: create it. This is the ONLY branch that may start from an empty doc.
                None => Settings::default(),
                // Corruption that appeared BETWEEN the load and this locked write. Returning `None`
                // leaves the file untouched; the message is surfaced below so the caller can tell
                // the write did not happen (CFG-001).
                Some(Err(e)) => {
                    corrupt = Some(format!("parse error: {e}"));
                    return None;
                }
            };
            // CFG-062 — "clear" means the key is GONE, not present-and-null. Pi's clearing setters
            // assign `undefined` (`setShellPath` settings-manager.ts:883-887, `setShellCommandPrefix`
            // :914-918, `setNpmCommand` :924-928 @v0.83.0) and `persistScopedSettings` serializes
            // through `JSON.stringify(mergedSettings, null, 2)` (:605), which OMITS
            // undefined-valued properties. `serde_json` has no `undefined`, so `None::<String>`
            // arrives here as `Value::Null` and used to persist as `"shellPath": null` — a value
            // upstream cannot write, and one that a lower layer's `deep_merge` treats as a real
            // override (both sides let a project `null` blank a global value, so the divergence is
            // the WRITE, not the merge).
            if json.is_null() {
                doc.obj.remove(&key_owned);
            } else {
                doc.obj.insert(key_owned.clone(), json.clone());
            }
            Some(doc.to_pretty())
        }).await?;
```

Replace with:

```rust
        self.store
            .with_lock(scope, &mut |current| {
                let mut doc = match current.map(Settings::parse) {
                    Some(Ok(s)) => s,
                    // Absent file: create it. This is the ONLY branch that may start from an empty doc.
                    None => Settings::default(),
                    // Corruption that appeared BETWEEN the load and this locked write. Returning `None`
                    // leaves the file untouched; the message is surfaced below so the caller can tell
                    // the write did not happen (CFG-001).
                    Some(Err(e)) => {
                        corrupt = Some(format!("parse error: {e}"));
                        return None;
                    }
                };
                // CFG-062 — "clear" means the key is GONE, not present-and-null. Pi's clearing setters
                // assign `undefined` (`setShellPath` settings-manager.ts:883-887, `setShellCommandPrefix`
                // :914-918, `setNpmCommand` :924-928 @v0.83.0) and `persistScopedSettings` serializes
                // through `JSON.stringify(mergedSettings, null, 2)` (:605), which OMITS
                // undefined-valued properties. `serde_json` has no `undefined`, so `None::<String>`
                // arrives here as `Value::Null` and used to persist as `"shellPath": null` — a value
                // upstream cannot write, and one that a lower layer's `deep_merge` treats as a real
                // override (both sides let a project `null` blank a global value, so the divergence is
                // the WRITE, not the merge).
                if json.is_null() {
                    doc.obj.remove(&key_owned);
                } else {
                    doc.obj.insert(key_owned.clone(), json.clone());
                }
                Some(doc.to_pretty())
            })
            .await?;
```

**2.** Match count **2** — this exact text appears twice, as the body of `set_nested` and again as the body of `persist_nested`. **Replace both occurrences**; they are two of the ten hunks.

Find:

```rust
        self.ensure_scope_writable(scope)?;
        let path_owned: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        let mut corrupt: Option<String> = None;
        self.store.with_lock(scope, &mut |current| {
            let mut doc = match current.map(Settings::parse) {
                Some(Ok(s)) => s,
                None => Settings::default(),
                Some(Err(e)) => {
                    corrupt = Some(format!("parse error: {e}"));
                    return None;
                }
            };
            set_value_at_path(&mut doc.obj, &path_owned, value.clone());
            Some(doc.to_pretty())
        }).await?;
        if let Some(message) = corrupt {
            return Err(ConfigError::SettingsWriteRefused { scope, message });
        }
```

Replace with:

```rust
        self.ensure_scope_writable(scope)?;
        let path_owned: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        let mut corrupt: Option<String> = None;
        self.store
            .with_lock(scope, &mut |current| {
                let mut doc = match current.map(Settings::parse) {
                    Some(Ok(s)) => s,
                    None => Settings::default(),
                    Some(Err(e)) => {
                        corrupt = Some(format!("parse error: {e}"));
                        return None;
                    }
                };
                set_value_at_path(&mut doc.obj, &path_owned, value.clone());
                Some(doc.to_pretty())
            })
            .await?;
        if let Some(message) = corrupt {
            return Err(ConfigError::SettingsWriteRefused { scope, message });
        }
```

**3.** Match count **1**.

Find:

```rust
            Value::String(mode.as_str().to_string()),
        ).await
```

Replace with:

```rust
            Value::String(mode.as_str().to_string()),
        )
        .await
```

**4.** Match count **1**.

Find:

```rust
        self.set(SettingsScope::Global, "editorPaddingX", clamped).await
```

Replace with:

```rust
        self.set(SettingsScope::Global, "editorPaddingX", clamped)
            .await
```

**5.** Match count **1**.

Find:

```rust
    pub async fn set_autocomplete_max_visible(&mut self, max_visible: f64) -> Result<(), ConfigError> {
        let clamped = (max_visible.floor() as i64).clamp(3, 20);
        self.set(SettingsScope::Global, "autocompleteMaxVisible", clamped).await
```

Replace with:

```rust
    pub async fn set_autocomplete_max_visible(
        &mut self,
        max_visible: f64,
    ) -> Result<(), ConfigError> {
        let clamped = (max_visible.floor() as i64).clamp(3, 20);
        self.set(SettingsScope::Global, "autocompleteMaxVisible", clamped)
            .await
```

**6.** Match count **1**.

Find:

```rust
            clamped.into(),
        ).await
```

Replace with:

```rust
            clamped.into(),
        )
        .await
```

**7.** Match count **1**.

Find:

```rust
            timeout_ms.floor() as i64,
        ).await
```

Replace with:

```rust
            timeout_ms.floor() as i64,
        )
        .await
```

**8.** Match count **1**.

Find:

```rust
            show.into(),
        ).await
```

Replace with:

```rust
            show.into(),
        )
        .await
```

**9.** Match count **1**.

Find:

```rust
            }).await?;
```

Replace with:

```rust
            })
            .await?;
```

### `crates/cyrup-config/src/settings/tests/merge_and_scope.rs` — 5 hunks, 5 edits

**1.** Match count **1**.

Find:

```rust
    mgr.set(SettingsScope::Global, "shellPath", Some("~/bin/bash"))
        .await.unwrap();
```

Replace with:

```rust
    mgr.set(SettingsScope::Global, "shellPath", Some("~/bin/bash"))
        .await
        .unwrap();
```

**2.** Match count **1**.

Find:

```rust
        Value::Bool(true),
    )
    .await.unwrap();
```

Replace with:

```rust
        Value::Bool(true),
    )
    .await
    .unwrap();
```

**3.** Match count **1**.

Find:

```rust
    mgr.set(SettingsScope::Global, "shellPath", None::<&str>)
        .await.unwrap();
```

Replace with:

```rust
    mgr.set(SettingsScope::Global, "shellPath", None::<&str>)
        .await
        .unwrap();
```

**4.** Match count **1**.

Find:

```rust
        Value::Null,
    )
    .await.unwrap();
```

Replace with:

```rust
        Value::Null,
    )
    .await
    .unwrap();
```

**5.** Match count **1**.

Find:

```rust
    mgr.set(SettingsScope::Global, "defaultModel", "new")
        .await.unwrap();
```

Replace with:

```rust
    mgr.set(SettingsScope::Global, "defaultModel", "new")
        .await
        .unwrap();
```

### `crates/cyrup-config/src/settings/tests/write_refusal.rs` — 10 hunks, 10 edits

**1.** Match count **1**.

Find:

```rust
            false.into(),
        ).await,
```

Replace with:

```rust
            false.into(),
        )
        .await,
```

**2.** Match count **1**.

Find:

```rust
        mgr.persist_nested(SettingsScope::Global, &["outputPad"], 0.into()).await,
```

Replace with:

```rust
        mgr.persist_nested(SettingsScope::Global, &["outputPad"], 0.into())
            .await,
```

**3.** Match count **1**.

Find:

```rust
    assert_refused(mgr.set_autocomplete_max_visible(9.0).await, SettingsScope::Global);
    assert_refused(mgr.set_http_idle_timeout_ms(1000.0).await, SettingsScope::Global);
```

Replace with:

```rust
    assert_refused(
        mgr.set_autocomplete_max_visible(9.0).await,
        SettingsScope::Global,
    );
    assert_refused(
        mgr.set_http_idle_timeout_ms(1000.0).await,
        SettingsScope::Global,
    );
```

**4.** Match count **1**.

Find:

```rust
        .await.unwrap();
```

Replace with:

```rust
        .await
        .unwrap();
```

**5.** Match count **1**.

Find:

```rust
            true.into(),
        ).await,
```

Replace with:

```rust
            true.into(),
        )
        .await,
```

**6.** Match count **1**.

Find:

```rust
        mgr.persist_nested(SettingsScope::Global, &["outputPad"], 1.into()).await,
```

Replace with:

```rust
        mgr.persist_nested(SettingsScope::Global, &["outputPad"], 1.into())
            .await,
```

**7.** Match count **1**.

Find:

```rust
    assert!(mgr.set(SettingsScope::Global, "theme", "light").await.is_err());
```

Replace with:

```rust
    assert!(
        mgr.set(SettingsScope::Global, "theme", "light")
            .await
            .is_err()
    );
```

**8.** Match count **1**.

Find:

```rust
    );

    mgr.set(SettingsScope::Global, "theme", "light").await.unwrap();
```

Replace with:

```rust
    );

    mgr.set(SettingsScope::Global, "theme", "light")
        .await
        .unwrap();
```

**9.** Match count **1**.

Find:

```rust
    mgr.set(SettingsScope::Global, "theme", "light").await.unwrap();
    mgr.set_nested(
```

Replace with:

```rust
    mgr.set(SettingsScope::Global, "theme", "light")
        .await
        .unwrap();
    mgr.set_nested(
```

**10.** Match count **1**.

Find:

```rust
    )
    .await.unwrap();
```

Replace with:

```rust
    )
    .await
    .unwrap();
```

### `crates/cyrup-config/src/trust.rs` — 4 hunks, 4 edits

**1.** Match count **1**.

Find:

```rust
    pub async fn set(&self, cwd: &Path, decision: Option<TrustDecision>) -> Result<(), ConfigError> {
```

Replace with:

```rust
    pub async fn set(
        &self,
        cwd: &Path,
        decision: Option<TrustDecision>,
    ) -> Result<(), ConfigError> {
```

**2.** Match count **1**.

Find:

```rust
        store.set(&root, Some(TrustDecision::Trusted)).await.unwrap();
```

Replace with:

```rust
        store
            .set(&root, Some(TrustDecision::Trusted))
            .await
            .unwrap();
```

**3.** Match count **1**.

Find:

```rust
        store.set(&cwd, Some(TrustDecision::Untrusted)).await.unwrap();
```

Replace with:

```rust
        store
            .set(&cwd, Some(TrustDecision::Untrusted))
            .await
            .unwrap();
```

**4.** Match count **1**.

Find:

```rust
        assert!(matches!(store.nearest(&cwd).await, Err(ConfigError::Trust(_))));
```

Replace with:

```rust
        assert!(matches!(
            store.nearest(&cwd).await,
            Err(ConfigError::Trust(_))
        ));
```

### `crates/cyrup-core/src/keyed_lock.rs` — 3 hunks, 3 edits

**1.** Match count **1**.

Find:

```rust
        let _pending = PendingEntry { map: Arc::clone(&self.map), key: key.clone() };
```

Replace with:

```rust
        let _pending = PendingEntry {
            map: Arc::clone(&self.map),
            key: key.clone(),
        };
```

**2.** Match count **1**.

Find:

```rust
        // that has just cloned the Arc is observed (strong_count > 1) and the entry is kept.
        self.map.remove_if(&self.key, |_, v| Arc::strong_count(v) == 1);
```

Replace with:

```rust
        // that has just cloned the Arc is observed (strong_count > 1) and the entry is kept.
        self.map
            .remove_if(&self.key, |_, v| Arc::strong_count(v) == 1);
```

**3.** Match count **1**.

Find:

```rust
        // guard's own drop does the eviction.
        self.map.remove_if(&self.key, |_, v| Arc::strong_count(v) == 1);
```

Replace with:

```rust
        // guard's own drop does the eviction.
        self.map
            .remove_if(&self.key, |_, v| Arc::strong_count(v) == 1);
```

## Do not touch

- **Any file not in the six.** In particular not `crates/cyrup-config/src/lock.rs` (already clean —
  see the correction at the top), not `crates/cyrup-core/src/lib.rs` (3 hunks, all older than the
  branch), not `crates/cyrup-ext-subagents/src/discovery/management.rs` (165), not
  `crates/cyrup/src/subcommands.rs` (24 — the largest single file in the `cyrup` package now that
  `main.rs` has been decomposed into `bootstrap.rs`, `prelaunch.rs`, `interactive.rs`, `actions.rs`,
  `session_launch.rs`, `predispatch.rs`; `main.rs` itself is down to 3), not
  `crates/cyrup-session-svc/`, not `crates/cyrup-tui/`. Those hunks are unreachable without
  absorbing thousands of hunks of unrelated drift; that is a separate per-package drift-absorption
  job, not this one.
- **`cargo fmt` at any scope.** Not `--all`, not `-p`. See **Required path**.
- **The `mod tests` body in `crates/cyrup-tools/src/lock.rs`.** Superseded advice on this task was
  to restore it to pre-branch text so a field comment's "no test changes at all" claim would read
  true. That is impossible — the pre-branch file was itself rustfmt-dirty inside `mod tests`, so
  "tests restored" and "file is rustfmt-clean" cannot both hold; fmt-clean wins.
  [`MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md`](./MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md)
  reached the same conclusion independently (its rejected alternative #5) and owns that sentence.
  Nothing about the test text is in scope here — and nothing needs to be: this task's two `lock.rs`
  edits are both in production code, and the `mod tests` region hashes identically before and after
  (`92342984087deeb02ce3383288b4627d52b8d0d520bab1fe9d5db65c5ceba3b0`).
- **Logic, signatures as declared, imports.** No token changes. `TrustStore::set` and
  `SettingsManager::set_autocomplete_max_visible` have their *parameter lists wrapped onto separate
  lines*; the signatures themselves are unchanged.

## Definition of done

No test is written or run, and no git command is used, at any step.

1. **The six files are clean, and the command exits `0`.**

   ```bash
   rustfmt --check --color never --edition 2024 --config skip_children=true \
     crates/cyrup-tools/src/lock.rs \
     crates/cyrup-config/src/settings/manager.rs \
     crates/cyrup-config/src/settings/tests/merge_and_scope.rs \
     crates/cyrup-config/src/settings/tests/write_refusal.rs \
     crates/cyrup-config/src/trust.rs \
     crates/cyrup-core/src/keyed_lock.rs
   echo $?    # must print 0
   ```

   The exit code is the assertion that matters. A missing `--edition 2024` also prints nothing on
   stdout, but exits `1` — so "no output" alone proves nothing. Check `$?`.

2. **No token was added or removed — only whitespace and commas.** This is the git-free replacement
   for a `git diff` read: the whitespace-and-comma-stripped content of each file must still hash to
   the value recorded here, which was taken from the files *before* formatting.

   ```bash
   for f in crates/cyrup-tools/src/lock.rs \
            crates/cyrup-config/src/settings/manager.rs \
            crates/cyrup-config/src/settings/tests/merge_and_scope.rs \
            crates/cyrup-config/src/settings/tests/write_refusal.rs \
            crates/cyrup-config/src/trust.rs \
            crates/cyrup-core/src/keyed_lock.rs; do
     printf '%s  %s\n' "$(tr -d '[:space:],' < "$f" | sha256sum | cut -d' ' -f1)" "$f"
   done
   ```

   must print exactly:

   ```
   6b7c89c1e996022b21868c1325c2be0d2dca90e38f3ee39dbcd48097353350e0  crates/cyrup-tools/src/lock.rs
   c543fa8db3eb798bc15fa0ec4777619b3c580716fed6d4660f5e191d4d1b4288  crates/cyrup-config/src/settings/manager.rs
   482a6bc6a9181c750d3d01480ba2e7b7fb626db955ab5f639c9302653d92262b  crates/cyrup-config/src/settings/tests/merge_and_scope.rs
   a10de2ff40d8a5afe6dd7752e5b0e7182a090d48054aacc96b373f27f50fbe2a  crates/cyrup-config/src/settings/tests/write_refusal.rs
   e3578acc95b810b1388b81b57abd865b5a40f7b20dc98cad9415c9670c753879  crates/cyrup-config/src/trust.rs
   782b143605570d30423e7eacf0fbb7824ce733cf308a11f80a96934dff447e20  crates/cyrup-core/src/keyed_lock.rs
   ```

   `crates/cyrup-tools/src/lock.rs` is the one exception: if
   [`MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md`](./MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md)
   landed first (it must have — see the ordering note), that task adds `///` lines, so its stripped
   hash will differ from the value above. Its own DoD pins the full-file hash
   `41c2f39f4e714a29e4beac98ebeb5860800dfdbad6ebb7ed9a15d7962dfb8882` at 429 lines; take the
   stripped hash of *that* state as your baseline instead. The other five are unaffected.

3. **Exact-output hashes** — only meaningful if the inputs were untouched since this spec was
   written. First confirm the precondition, then confirm the result:

   | File | sha256 before (precondition) | sha256 after (required) |
   | --- | --- | --- |
   | `crates/cyrup-tools/src/lock.rs` | `fb9ae905c8fd58214428fb45dbc6f825b3ed7ec6374c91161bd277569c79a6be` | `2e7bd6bf345993eb780053a24a77c2c8125c207216a8366c1336060f57afb982` |
   | `crates/cyrup-config/src/settings/manager.rs` | `7bbeaaf9d5283144dc91f3df85096da74c2a52b3ceab168716972eef49599468` | `2c533bdbf5028523052ed2eb623c130693c7458caf15751ba99efbdc6bcaeab1` |
   | `crates/cyrup-config/src/settings/tests/merge_and_scope.rs` | `47daf2ceaf1d4a6e238078c7092f259f3e6e5400562bbe7299f6374c9f46607e` | `3ed76b32f502494ecc8f4755e184783aefec5786d2588e45b3e651ac87aba5ff` |
   | `crates/cyrup-config/src/settings/tests/write_refusal.rs` | `55a8b182a7b2a816f4e853a970a1706f5a89fb9fdf9d78391dffb8d6c212f0c7` | `b3a33e2132a264c740a4698bbca62304944d5c380fab73a684d59b6de9997f03` |
   | `crates/cyrup-config/src/trust.rs` | `efde5eb49ce5a60c1c96d2a9dd58e63ad8d88fe7037cb59e363a43b642639f8c` | `ed1fed3d56ef544aa1f7ab55a0d95ba5a488b94e738332f4f016a7165324168f` |
   | `crates/cyrup-core/src/keyed_lock.rs` | `eee73e6e3cb5ecb44dc5569a20e06e731390366d7aa1bdc879d6d1b7bcb10794` | `67d75688671dcd09e02f78d6b8f692c4e286293e26397c3c95f6bf420afe4328` |

   For each file whose *before* hash matched, its *after* hash is mandatory. For any file whose
   *before* hash did not match, a sibling task edited it first: skip its row and rely on items 1, 2
   and 4 instead. `crates/cyrup-tools/src/lock.rs` is expected not to match, for the reason in
   item 2.

4. **Line counts.** `wc -l` over the six prints `414`, `554`, `515`, `340`, `775`, `131`
   respectively — same caveat for `crates/cyrup-tools/src/lock.rs` if the sibling task grew it
   first (it becomes `429 + 6 = 435`).

5. **Both damaged packages are now fully clean, and `cyrup-core` absorbed no drift.**

   ```bash
   cargo fmt -p cyrup-tools  -- --check ; echo $?   # 0
   cargo fmt -p cyrup-config -- --check ; echo $?   # 0
   cargo fmt -p cyrup-core   -- --check --color never 2>/dev/null | grep -c '^Diff in'      # 46, was 49
   cargo fmt -p cyrup-core   -- --check --color never 2>/dev/null | grep -c 'keyed_lock.rs' # 0
   ```

6. **Nothing outside the six moved.** The workspace-wide hunk count drops by exactly 34:

   ```bash
   cargo fmt --all -- --check --color never 2>/dev/null | grep -c '^Diff in'   # 13165, was 13199
   ```

   Any other number means a file outside the six was reformatted, or one inside it was not.

7. **It still parses.** Not needed if the required `rustfmt` command produced the change —
   rustfmt refuses to write a file it cannot parse, so item 1 already proves it. Required only if
   the edit list was applied by hand: `cargo check -p cyrup-tools -p cyrup-config -p cyrup-core`
   succeeds.

8. Commit alone, as a formatting-only change, so the churn is isolated for future blame readers.
   State in the message that the diff is whitespace and trailing commas only, with no token change
   and no import move.

## Not in this task

- No new tests, no benchmarks, no documentation. Another team owns those. The two `settings/tests/`
  files and the `mod tests` blocks in `trust.rs` and `lock.rs` are *reformatted*, never rewritten —
  no assertion, name, or behaviour changes.
- No `cargo fmt --check` CI gate. There is none in-tree today — no `.github/`, no fmt target in
  [`xtask`](../../xtask) (`main.rs`, `features.rs`, `tsdata.rs`; the only `fmt` in it is
  `std::fmt::Write`), no active git hooks — and adding one would fail immediately on the 13165
  hunks of pre-existing drift this task deliberately leaves alone.
- The remaining 13165 workspace hunks, including the branch-authored ones tangled up inside the
  already-dirty files. If someone wants those, it is a per-package drift-absorption task in the
  shape of [`_backlog/RESOURCES_RUSTFMT_DRIFT.md`](_backlog/RESOURCES_RUSTFMT_DRIFT.md), scheduled
  and committed one package at a time.

## QA verdict — 2026-08-23 08:32 — PASS (9/10)

Reviewed against the tree on disk with `rustfmt 1.9.0-stable (88d9e12ae1 2026-08-18)`. No git used.

**The defect is fixed.** All six files are rustfmt-clean at edition 2024.

| DoD | Result |
| --- | --- |
| 1. `rustfmt --check` over the six | **exit 0**, empty output. Control run on `crates/cyrup-core/src/lib.rs` with the identical flag set printed 3 `Diff in` headers and exited 1, proving rustfmt actually parsed the files and the silence is not the edition-2015 false negative the spec warns about. |
| 2. Stripped hashes (no token change) | 4/6 byte-identical to the pre-format values recorded here: `manager.rs` `c543fa8d…`, `merge_and_scope.rs` `482a6bc6…`, `write_refusal.rs` `a10de2ff…`, `trust.rs` `e3578acc…`. See note below for the other two. |
| 3. Exact-output hashes | 4/6 match the required *after* column exactly (`2c533bdb…`, `3ed76b32…`, `b3a33e21…`, `ed1fed3d…`). |
| 4. Line counts | `554`, `515`, `340`, `775` match exactly. |
| 5. Package cleanliness | `cargo fmt -p cyrup-tools -- --check` → 0 hunks; `-p cyrup-config` → 0 hunks; `-p cyrup-core` → **46** (was 49), `keyed_lock.rs` → **0**. Exactly as specified. |
| 6. Workspace-wide | `cargo fmt --all -- --check` → **13165** `Diff in` headers, exactly the predicted 13199 − 34. Nothing outside the six moved. |
| 7. Still parses | Implied by 1 — rustfmt will not accept a file it cannot parse at edition 2024. |
| 8. Isolated commit | Not verifiable under the no-git rule; not held against the change. |

**The two files whose hashes could not match, and why that is not a defect.**
`crates/cyrup-tools/src/lock.rs` (445 lines, not the predicted 435) and
`crates/cyrup-core/src/keyed_lock.rs` (202 lines, not 131) were both edited by sibling tasks that
landed *after* this one, so the "run this LAST" ordering was not honoured for them. Inspection shows
the later edits, not a token change by this task:

- `cyrup-tools/src/lock.rs` now builds `Self { inner: KeyedLocks::new(map.clone()), map }` (was
  `Arc::clone(&map)`) and the guard chain gained `.map(MutationGuard)` — the map-alias and
  `MutationGuard` tasks.
- `keyed_lock.rs` collapsed the two duplicated `self.map.remove_if(…)` drop paths into a single
  `fn evict_if_unreferenced(&self, key: &K)` helper — the `KeyedLocks` doc/alias tasks.

Both files are nonetheless rustfmt-clean today, which is the property this task exists to establish,
and item 5 confirms their packages carry zero and zero-in-`keyed_lock.rs` hunks respectively. The
spec's own escape hatch applies: *"the rule is authoritative and the byte-exact edit list is not."*
Consequence for future readers: the pinned `mod tests` region hash `92342984…` and the
`429 + 6 = 435` line prediction in item 4 are now stale. Stale predictions in a spec, not false
claims in shipped source.

**Nothing false was introduced.** This change adds no comment, doc line, or prose to any source
file — it is reflow and trailing commas only, so there is no new factual claim to be wrong. The
spec's own load-bearing claims were re-checked against the tree and all hold: no `rustfmt.toml` or
`.rustfmt.toml` anywhere outside `target/`; `edition = "2024"` under `[workspace.package]` at
`Cargo.toml:88`; `crates/cyrup-config/src/lock.rs` is already clean (exit 0), so keeping it off the
command line was correct; no `.github/` directory; `xtask/src` is exactly `features.rs`, `main.rs`,
`tsdata.rs` with no fmt target. No `*.rs.bk` litter was left anywhere in the tree.

**Rating 9/10.** One point off only because the git-free "no token was added or removed" proof in
items 2–4 is unrecoverable for two of the six files once the ordering it depends on was not
followed; the guarantee for those two now rests on reading the code rather than on a hash. The
delivered state is correct, complete, and absorbed zero unrelated drift.
