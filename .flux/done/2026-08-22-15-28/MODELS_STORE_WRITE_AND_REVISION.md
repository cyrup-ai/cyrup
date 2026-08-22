---
stage: qa
status: completed
updated: 2026-08-22 23:58
---

# Fix models_store Write Contract And file_revision Field Mismatch

## Description

Two contract defects in [`src/models_store.rs`](../../crates/cyrup-config/src/models_store.rs).
Both are latent, both are cheap, both belong to the same file and session.

### 1. `write` reports `Ok(())` for a write it never performed (:299-307)

```rust
let _guard = crate::lock::FileLock::acquire(&self.path).map_err(store_err)?;
let mut all = self.read_all();
if let Ok(value) = serde_json::to_value(&entry) {        // :301
    all.insert(provider_id.to_string(), value);
    self.write_all(&all).map_err(store_err)?;
    self.update_read_state(&all, None);
}
Ok(())                                                   // :307
```

If `to_value` returns `Err`, the arm is skipped and the function returns `Ok(())` — the caller is
told a single-provider catalog update persisted when it did not. The sibling `delete` at `:319-322`
has no equivalent guard; the asymmetry is confirmed. `store_err` (`:187`) takes
`crate::error::ConfigError`, so `serde_json::Error` needs its own conversion path.

**Reachability, stated so the implementer does not chase it: this `Err` arm is unreachable today.**
Every production `ModelsStoreEntry` reaching `write` is built in
`crates/cyrup-provider/src/remote_catalog.rs:625,642,662,703` from a deserialized remote catalog;
serde_json cannot parse a non-finite `f64`, and `number.rs:183 from_f64` maps non-finite to
`Value::Null` rather than erroring. serde_json's only `f64` `Err` arms are float map keys and
arbitrary precision; all maps here are `String`-keyed and every type is a plain derive. The value is
contract hygiene plus disarming a trap that arms itself the day a cost is computed rather than
parsed. **The failure mode is narrower than "the catalog stops persisting"** — the guard skips only
the insert for one `provider_id`, and `write_all` is inside the guard, so the file is left untouched:
a lost single-provider update reported as success, never a corrupted store.

**Explicitly out of scope, do not bundle:** the `AuthStore` sibling at `auth.rs:292` is infallible by
construction (`Credential`'s only unbounded field is `Oauth.ext: Map<String, Value>`, and
`Value::Number` cannot hold NaN/Infinity). `OrderedObject::stringify`'s `unwrap_or_else` at
`models_store.rs:78-80` is likewise optional — `to_string_pretty` over a `&Value` has no reachable
error either.

### 2. `file_revision` puts sub-second fractions where nanoseconds-since-epoch are documented (:153-183)

The doc at `:153-155` specifies Pi's `getFileRevision` as `${dev}:${ino}:${size}:${mtimeNs}:${ctimeNs}`.
The unix branch (`:158-169`) instead uses `meta.mtime_nsec()` / `meta.ctime_nsec()` (confirmed at
`:166` and `:167`) — the **fractional-second components** (0..1e9), not full timestamps. The
`#[cfg(not(unix))]` branch (`:170-182`) does the opposite: `d.as_nanos()`, real nanoseconds since
epoch. The two branches disagree about what the revision string means, and only one matches the doc.

On this machine's filesystem both `nsec` fields carry full resolution (sample `823677862`), so the
degenerate `dev:ino:size:0:0` revision — where writes within one coarse tick are indistinguishable
and `read_all` (`:228`, `:240`) serves a stale snapshot — requires a coarse-granularity filesystem
and was **not** reproduced. The actionable core is the measured value mismatch against the documented
contract and the cfg-branch disagreement, not an observed stale read. Fix by composing full
nanoseconds on unix (`mtime() * 1_000_000_000 + mtime_nsec()`, saturating).

**Scope:** `src/models_store.rs` only.

## Acceptance Criteria

- [ ] `rg -n 'if let Ok\(value\) = serde_json::to_value' src/models_store.rs` returns 0 matches
- [ ] `write` propagates a serialization failure as a `ProviderError` via a dedicated conversion (not `store_err`, which takes `ConfigError` at `:187`), so no path through `write` returns `Ok(())` without having called `write_all`
- [ ] `src/auth.rs:292` and `src/models_store.rs:78-80` are unchanged
- [ ] `rg -n 'mtime_nsec\(\)|ctime_nsec\(\)' src/models_store.rs` shows each call combined with its whole-seconds counterpart (`mtime()` / `ctime()`) rather than formatted alone
- [ ] Both `#[cfg(unix)]` and `#[cfg(not(unix))]` branches produce nanoseconds-since-epoch for fields 4 and 5, matching the doc at `:153-155`, using checked or saturating arithmetic
- [ ] `cargo test -p cyrup-config` still reports 222 passed / 0 failed, including `read_answers_from_the_snapshot_until_the_file_revision_changes` (`:403`)
- [ ] `cargo clippy -p cyrup-config --all-targets` 0 warnings; `cargo fmt -p cyrup-config -- --check` 0 hunks

## Outcome — completed

Landed in `9196227`. All seven acceptance criteria verified mechanically at QA.

`write` no longer discards a serialization `Err`: the value is produced with `?` and carried through
`ConfigError::Serde` (`error.rs:52`) into `store_err`, so no path returns `Ok(())` without having
called `write_all`. `file_revision` composes whole seconds with the sub-second fraction on unix via a
saturating `epoch_nanos` helper, so both `cfg` branches now emit nanoseconds-since-epoch and agree
with the documented contract.

Both defects were latent, as the task stated: the serialization arm is unreachable today, and the
`file_revision` degeneracy needs a coarse-granularity filesystem and was not reproduced here. The fix
is justified by the measured value mismatch and the cfg-branch disagreement, not by an observed
failure.

Carve-outs respected and verified unchanged: `auth.rs:292` (infallible by construction) and
`models_store.rs:78-80` (`stringify`'s `unwrap_or_else`, no reachable error).
`read_answers_from_the_snapshot_until_the_file_revision_changes` still passes.
