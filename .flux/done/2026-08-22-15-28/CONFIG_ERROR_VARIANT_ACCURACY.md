---
stage: qa
status: completed
updated: 2026-08-22 21:05
---

# Fix Error Variants That Mislabel Or Silently Swallow Config Failures

## Description

`cyrup-config`'s error surface misreports four distinct classes of failure. All four live in the
same two enums ([`src/error.rs`](../../crates/cyrup-config/src/error.rs)) and the same handful of
I/O call sites, so they are one session's work.

### 1. `FileModelsStore` returns `Ok(())` for writes it dropped on the floor

[`src/models_store.rs:249`](../../crates/cyrup-config/src/models_store.rs) discards a
`Result<(), ConfigError>`:

```rust
let _ = crate::lock::write_atomic(&self.path, text.as_bytes(), true);
```

Compounding it, **three** sites degrade a failed lock acquisition to an unlocked write —
`models_store.rs:226`, `:287`, `:305` — each `crate::lock::FileLock::acquire(&self.path).ok()`.
So two independent failures can stack behind one `Ok(())`: the caller believes the model catalog
was persisted when nothing reached disk. `update_read_state` still runs, so the in-memory snapshot
diverges from the file.

### 2. `auth.rs` reports credential-write I/O failures as `AuthError::Lock`

In `AuthStore::modify` ([`src/auth.rs:294-295`](../../crates/cyrup-config/src/auth.rs)):

```rust
crate::lock::write_atomic(&self.path, text.as_bytes(), true)
    .map_err(|e| AuthError::Lock(e.to_string()))?;
```

A full disk, EACCES, or a failed `rename` on the credential file renders to the user as
`lock: …` (`error.rs:59-60`), and `e.to_string()` flattens the `ConfigError`, destroying the
source chain. The `FileLock::acquire` mapping at `auth.rs:269-270` is correct and should stay.

**Scope note:** the fix needs a new variant such as `AuthError::Config(#[from] ConfigError)`. Do
**not** route it through the existing `AuthError::Io(#[from] std::io::Error)` at `error.rs:55-56` —
`write_atomic` and `FileLock::acquire` return `ConfigError`, not `io::Error`.

### 3. `ConfigError::Trust` is a catch-all, so non-trust errors print "trust store:"

`grep -c 'ConfigError::Trust' src/settings.rs` returns **4**, none of them about the trust store:

- `settings.rs:765` — `Invalid httpIdleTimeoutMs setting: {v}`
- `settings.rs:776` — `Invalid websocketConnectTimeoutMs setting: {v}`
- `settings.rs:1255` — `"poisoned lock"`
- `settings.rs:1644` — a formatted validation failure

The legitimate constructions are `trust.rs:111`, `:116`, `:129`. A settings validation error and a
poisoned `RwLock` both render with the trust-store prefix.

`trust.rs:646` is `assert!(matches!(store.nearest(&cwd), Err(ConfigError::Trust(_))))` — an
in-crate matcher any variant split must keep compiling.

### 4. `ConfigError::Io` never names the failing path, though the path is always in scope

`error.rs:40` is `Io(#[from] std::io::Error)`. Every construction site holds the path and throws
it away: `env.rs:243`, `env.rs:247`, `settings.rs:1193`, `settings.rs:1207`, `trust.rs:105`.
`AuthError::Io` has the same gap at `auth.rs:164` (inside `read_file_uncached`, whose read is at
`:161`).

The sibling variant already does this right — `error.rs:42-43`:
`#[error("lock contention on {path}")] Lock { path: PathBuf }`.

**Note while editing:** `lock.rs` is not the only write path — `keybindings.rs:331` deliberately
uses plain `std::fs::write` per the rationale at `keybindings.rs:311-313`, so `?`-conversions
from `io::Error` exist outside `lock.rs`.

## Acceptance Criteria

- [ ] `grep -n 'let _ = crate::lock::write_atomic' src/models_store.rs` returns nothing; `write_all` returns a `Result` that `ModelsStore::write` and `ModelsStore::delete` propagate
- [ ] `FileLock::acquire(&self.path).ok()` no longer appears in `models_store.rs` (all three sites at :226, :287, :305) — a failed acquisition reaches the caller
- [ ] The `write_atomic` call in `AuthStore::modify` maps to a variant carrying the `ConfigError` source; `AuthError::Lock` is constructed only at the `FileLock::acquire` site
- [ ] `grep -c 'ConfigError::Trust' src/settings.rs` returns 0
- [ ] `ConfigError::Io` and `AuthError::Io` carry the offending path and render it in their `#[error(...)]` string; all six sites supply it
- [ ] `cargo build -p cyrup-config` and `cargo test -p cyrup-config` pass with no failures beyond those in `TEST_FAILURES.md`; the `trust.rs:646` `matches!` assertion still compiles

## Outcome — completed

Landed in `a7aa779`, merged to `main` via #46 (squashed into `7e221a3`).

All four failure classes fixed. `let _ = crate::lock::write_atomic` is gone from `models_store.rs`,
as are all three `FileLock::acquire(&self.path).ok()` sites; `grep -c 'ConfigError::Trust'` over the
settings layer returns 0; `AuthError::Lock` is now constructed only at the `FileLock::acquire` site
(`auth.rs:273`).

**This carried a breaking public API change, called out in #46's description.** `ConfigError::Io`
and `AuthError::Io` became struct variants `{ path, source }`, which deletes both
`From<std::io::Error>` impls. That is the only way to carry the path this task asked for. Nothing
outside `cyrup-config` matches either variant, so `cargo check --workspace` stayed green.

**One test was renamed and its assertion tightened**, which a reviewer should not mistake for a
weakening: `an_unwritable_path_is_not_an_error` → `an_unwritable_path_fails_the_write_and_reads_as_no_overlay`.
The old test asserted the very bug this task fixed, so it had to change with the behaviour.

**Left deliberately undone:** `models_store.rs:301` still swallows a serialization failure via
`if let Ok(value) = serde_json::to_value(&entry)`. It sits outside this task's acceptance criteria
and was not folded in. Worth its own task.

## Correction to this task file's evidence

The final acceptance criterion (line 82) says "all six sites supply it" for `ConfigError::Io`. The
count was wrong — there were **eight** construction sites, not six. All eight were handled. Two of
the six named, `env.rs:243` and `env.rs:247`, turned out to be `std::env::current_dir()` failures
with no path to name; they were routed to `ConfigError::Dir` instead, which is the semantically
correct variant, with the io error's text preserved in the message.
