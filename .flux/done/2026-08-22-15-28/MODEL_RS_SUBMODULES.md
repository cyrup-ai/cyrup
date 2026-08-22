---
stage: qa
status: completed
updated: 2026-08-22 18:47
---

# Decompose model.rs Into Submodules — Rework

The split landed in [`5eacfc4`](.) and QA rated it 8/10. Three defects remain, all rustdoc
regressions plus one over-wide visibility marker. This file is scoped to exactly those.

## Verification already banked — do not redo

The original file was recovered from `fb86ad5:crates/cyrup-config/src/model.rs` (3,791 lines) and
diffed line-by-line against the new tree. Results, so the rework does not re-litigate them:

- **Every moved non-test range is byte-identical.** `resolver.rs` (384 lines), `glob.rs` (596),
  `cycler.rs` (43), `defaults.rs` (139), `schema.rs` (196), `validate.rs` (318), `compose.rs` (404),
  and the multi-range `select.rs` / `load.rs` all match the original character-for-character, modulo
  the four `pub(super)` markers and the two `include_str!` paths. The only other delta is one fewer
  blank line at the two joins where non-adjacent ranges were concatenated.
- **All 41 test bodies and both fixtures are byte-identical**, after normalising the `include_str!`
  paths and the `pub(super)` on `model` / `oai`.
- **The public surface is provably unchanged**: the original declared 31 top-level `pub` items and
  one `pub(crate)` (`apply_models_json`); [`model/mod.rs`](../../crates/cyrup-config/src/model/mod.rs)
  re-exports exactly those 31 plus that one. Zero dropped, zero added.
- `cargo fmt -p cyrup-config -- --check` shows **no diff in any `model/` file**. The five files it
  does flag (`env.rs`, `env_keys.rs`, `keybindings.rs`, `login.rs`, `models_store.rs`) are untouched
  and were already unformatted.

## Warning provenance — the research this rework turns on

`cargo doc -p cyrup-config --no-deps` reports **16** warnings; with `--document-private-items`, **17**.
Twelve fall under `src/model/`. Rustdoc only checks intra-doc links on *documented* items, so in
default mode a broken link on a private item is silent — which is why the private-items run is the
one that tells the whole truth.

| Warning | Site | Verdict |
|---|---|---|
| links to private item `resolver` | `mod.rs:5:25` | **NEW** |
| links to private item `glob` | `mod.rs:5:85` | **NEW** |
| links to private item `cycler` | `mod.rs:6:56` | **NEW** |
| links to private item `defaults` | `mod.rs:6:89` | **NEW** |
| links to private item `select` | `mod.rs:7:45` | **NEW** |
| links to private item `schema` | `mod.rs:8:7` | **NEW** |
| links to private item `load` | `mod.rs:8:20` | **NEW** |
| links to private item `validate` | `mod.rs:8:31` | **NEW** |
| links to private item `compose` | `mod.rs:8:46` | **NEW** |
| unresolved link to `load_models_file_reporting` | `schema.rs:10:7` | **NEW** |
| unresolved link to `AuthStore::has_auth` | `compose.rs:79:45` | pre-existing |
| unresolved link to `glob_match_chars` | `glob.rs:38:7` | pre-existing |

**Proof of the two pre-existing verdicts.** `AuthStore::has_auth` was already unresolvable in the
monolith: the fn signature spells `&crate::auth::AuthStore` in full and the file never imported the
type. `glob_match_chars` appears exactly **once** in the original 3,791 lines — the link itself — so
it has never named a real item; it is a rename leftover. Neither is caused by the split, both are
[`CARGO_DOC_WARNINGS.md`](CARGO_DOC_WARNINGS.md)'s business, and **both must be left exactly as they
are.**

> **Correction to the QA-authored definition of done.** It asked for "zero `unresolved link`
> warnings anywhere in `src/model/` other than `AuthStore::has_auth`". That is wrong — it was written
> before `--document-private-items` was run, and `glob_match_chars` survives too. The corrected
> numbers are in the DoD below.

## 1. `model/schema.rs:10` — the one genuinely broken cross-reference

The `ModelsJsonOauth` doc explains why the enum has a single variant, and points at the function that
converts the resulting serde failure into a user-facing message. That target moved to `load.rs`, so
the reference is now dead.

Current lines 8–11:

```rust
/// key. Modelling it as a single-variant enum reproduces that: serde fails the load, and
/// [`load_models_file_reporting`] turns the failure into Pi's empty-snapshot-plus-one-message
/// contract.
```

Replace with:

```rust
/// key. Modelling it as a single-variant enum reproduces that: serde fails the load, and
/// [`load_models_file_reporting`](crate::model::load_models_file_reporting) turns the failure
/// into Pi's empty-snapshot-plus-one-message contract.
```

**Use the explicit-path form, not a bare path link.** `[`crate::model::load_models_file_reporting`]`
would also resolve, but it changes the *rendered* text to the full path. The `[text](path)` form
keeps the visible prose word-for-word identical to what is there now — which is what the original
task's "do not reword any doc comment" rule was protecting — and it is this repo's established
idiom: 64 occurrences of `](crate::` across `crates/`, e.g.
[`user_message_selector.rs:5`](../../crates/cyrup-tui/src/user_message_selector.rs),
[`image.rs:538`](../../crates/cyrup-tui/src/image.rs).

The reflow across the second and third lines is required: on one line the sentence runs to ~137
characters, and the widest doc line anywhere in `model/` today is 105. Split as shown.

**Do NOT** add `use super::load::load_models_file_reporting;` — an import referenced only from a doc
comment triggers `unused_imports`.

## 2. `model/mod.rs:5-8` — nine links into private modules

Current:

```rust
//! Split by concern: [`resolver`] matches patterns and expands `--models` scope, [`glob`] is the
//! self-contained minimatch engine it filters with, [`cycler`] walks the scoped set, [`defaults`]
//! holds the curated per-provider table, [`select`] is the CLI/initial/restore decision layer, and
//! [`schema`] / [`load`] / [`validate`] / [`compose`] are the `models.json` pipeline: types, read,
//! judge, apply.
```

Replace with the same sentence, link brackets removed:

```rust
//! Split by concern: `resolver` matches patterns and expands `--models` scope, `glob` is the
//! self-contained minimatch engine it filters with, `cycler` walks the scoped set, `defaults`
//! holds the curated per-provider table, `select` is the CLI/initial/restore decision layer, and
//! `schema` / `load` / `validate` / `compose` are the `models.json` pipeline: types, read,
//! judge, apply.
```

Two other routes exist and are both **rejected**:

- Making the modules `pub` silences the warning by minting `cyrup_config::model::resolver::X` paths.
  The facade exists precisely to prevent that; it would undo the refactor's one hard guarantee.
- `#[doc(hidden)] pub mod` hides them from docs but still adds the callable paths. Same objection.

Re-pointing the links at public types (`[`ModelResolver`]`, `[`ModelCycler`]`, …) was also
considered and rejected: the sentence names *files* a maintainer will open, not API a caller will
use, and a private module is unreachable from rendered docs anyway. Plain code spans say exactly
what is true.

## 3. `model/defaults.rs:61` — `KNOWN_PROVIDERS` is wider than it needs to be

```rust
pub(super) const KNOWN_PROVIDERS: &[&str] = &[
```

Every reference lives in `defaults.rs`: line 111 (`first_default_or_first`) and lines 212, 213, 226,
303, 308 inside its own `mod tests`, which as a child module already sees its parent's private
items. The only other mentions crate-wide are prose inside comments at
[`env_keys.rs:76`](../../crates/cyrup-config/src/env_keys.rs) and `:244` — not code.

**Fix:** drop the marker.

```rust
const KNOWN_PROVIDERS: &[&str] = &[
```

The other three markers are load-bearing and stay: `glob_match` (`glob.rs:15`, called from
`resolver.rs`), `first_default_or_first` (`defaults.rs:110`, called from `select.rs`), and
`render_schema_errors` (`validate.rs:313`, called from `load.rs`).

## Definition of done

- `cargo doc -p cyrup-config --no-deps` reports **6** warnings, down from 16 — none of them under
  `src/model/`.
- `cargo doc -p cyrup-config --no-deps --document-private-items` reports **7**, down from 17 — the
  only two under `src/model/` being `AuthStore::has_auth` (`compose.rs`) and `glob_match_chars`
  (`glob.rs`), both untouched.
- `KNOWN_PROVIDERS` is private; the other three `pub(super)` markers are unchanged.
- `cargo build -p cyrup-config` and `cargo check --workspace --all-targets` stay clean.
- `cargo clippy -p cyrup-config --all-targets` still reports only `config_value.rs:556`,
  `model/validate.rs:67`, `model/validate.rs:207`.
- `cargo test -p cyrup-config` still passes 223, with 41 under `model::`.
- `cargo fmt -p cyrup-config -- --check` still shows no diff in any `model/` file.
- Exactly three files change: `model/schema.rs`, `model/mod.rs`, `model/defaults.rs`. No item
  renamed, no signature touched, no test body altered, no other doc comment reworded.
