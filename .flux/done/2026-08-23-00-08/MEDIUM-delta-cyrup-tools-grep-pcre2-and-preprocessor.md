---
stage: qa
status: completed
updated: 2026-08-29 11:30
---

# grep: pcre2 (`-P`) and the preprocessor flags — QA rework

QA verdict: **8.5/10**. The live defect is genuinely fixed and RED-proven, every citation in the
implementation checks out against source, and the suite, clippy and rustdoc are clean. One item in
the delivered scope is incomplete, and it is the only reason this is not a pass.

Everything else from the previous revision is **done and verified** and has been removed from this
file:

- `--engine` / `--pre` / `--pre-glob` consume their values; the leak that silently applied the next
  config line is closed (proven RED: reverting the arms reproduces `case: Some(Insensitive)` for
  `--engine\n-i`).
- `PCRE2_IS_DECLINED` and `PREPROCESSOR_IS_DECLINED` exist beside `QUIET_IS_REFUSED`.
- All four unicode names write `no_unicode`, cross pairs included.
- The PCRE2 hint and its guard are correctly left untouched; `rg_pattern_error`'s doc now pins
  `release.yml:177`, `:60-61`, `:69` and `grep.ts:309-310` — all four verified against the real
  ripgrep 14.1.0 workflow and pi source.
- `rgconfig.rs`'s module header no longer claims a single refusal.

---

## 1. Outstanding — `--no-pre` is the one negation left on the catch-all

`Pre::name_negated` returns `Some("no-pre")`
([`defs.rs`, `struct Pre` @5354](../../../tmp/ripgrep-14.1.0/crates/core/flags/defs.rs)), so
`--no-pre` is a real ripgrep 14.1.0 flag. It currently falls through
[`rgconfig.rs`](../../../crates/cyrup-tools/src/tools/rgconfig.rs)'s catch-all, while **every other
negation in the two groups is handled**: `--no-pcre2`, `--no-auto-hybrid-regex` and
`--no-search-zip` all reach explicit arms.

That inconsistency is the defect. `PREPROCESSOR_IS_DECLINED`'s doc enumerates the group it explains,
and §2.2's stated principle is that the catch-all is for flags the module *does not know* — this is
one it knows.

**It is not a live bug, and the fix must not pretend otherwise.** `--no-pre` is a switch:

```rust
// ripgrep 14.1.0, defs.rs:5431-5438 — Pre::update
FlagValue::Switch(yes) => {
    assert!(!yes, "there is no affirmative switch for --pre");
    args.pre = None;
    return Ok(());
}
```

It consumes no value, so it cannot leak one. The change is completeness, not behaviour.

### Required change

In `apply_long`, fold it into the existing preprocessor arm — it takes no value, so it must **not**
call `take()`:

```rust
"pre" | "pre-glob" => {
    take();
}
// `--no-pre` and `--no-search-zip` are switches (`Pre::update` asserts there is no affirmative
// switch for `--pre`, defs.rs:5431-5435), so they consume nothing.
"no-pre" | "search-zip" | "no-search-zip" => {}
```

Extend the existing guard `declined_value_taking_flags_consume_their_value`
([`grep.rs`](../../../crates/cyrup-tools/src/tools/grep.rs)) with `--no-pre` in the switch batch:

```rust
assert_eq!(
    RgFlags::parse("-P\n-z\n--pcre2\n--no-pcre2\n--pre\n--no-pre\n--search-zip\n--no-search-zip\n"),
    RgFlags::default()
);
```

Note `--pre` immediately followed by `--no-pre` is the case that proves the two arms coexist without
`--pre`'s `take()` swallowing the following flag — `take()` consuming `--no-pre` is *correct* here
(that is ripgrep's own two-line value form), and the assertion still holds because both are inert.

### Definition of done for this rework

1. `--no-pre` reaches an explicit arm and does **not** call `take()`.
2. `PREPROCESSOR_IS_DECLINED`'s doc names `--no-pre` alongside `--pre`, `--pre-glob` and `-z`.
3. The extended guard passes; the full `cyrup-tools` suite, clippy and rustdoc stay clean.

---

## 2. Still outstanding — the one decision that needs a human

Unchanged from the previous revision, and not something `/exec` can close:

> **May `cyrup-tools` carry an OFF-BY-DEFAULT `pcre2` cargo feature that pulls `grep-pcre2` →
> `pcre2` → `pcre2-sys`, making `cyrup-tools`' dependency subtree — pure Rust today — require a C
> toolchain when the feature is enabled?**

Verified live against the crates.io API: exactly three new crates (`grep-pcre2` 0.1.10 →
`pcre2` 0.2.11 → `pcre2-sys` 0.2.10); `cc`, `pkg-config`, `libc`, `log` and `grep-matcher` are all
already in `Cargo.lock`; `pcre2` appears in it zero times. Default builds stay byte-identical; the
cost lands only on whoever enables it. The workspace has declined a C dependency three times running
(`Cargo.toml:240-274`), so refusal is the standing default and what is owed is the ratification —
recorded in `PCRE2_IS_DECLINED` if refused, or a `docs/adr/` entry if taken.

---

## 3. Do NOT do in this task — file separately

QA found that the value-leak class this task closed for three flags **remains open for twenty
others**. Enumerated by cross-referencing every ripgrep flag whose `doc_variable` returns `Some`
against `apply_long`'s arms: 35 value-taking long flags exist, cyrup handles 15, and these 20 still
fall through the catch-all carrying a value:

`--after-context`, `--before-context`, `--color`, `--colors`, `--context`, `--context-separator`,
`--dfa-size-limit`, `--field-context-separator`, `--field-match-separator`, `--file`, `--generate`,
`--hostname-bin`, `--hyperlink-format`, `--max-columns`, `--path-separator`, `--regex-size-limit`,
`--regexp`, `--replace`, `--threads`, `--type-clear`

Each has the identical failure shape: written in ripgrep's documented two-line form with a value
that begins with `-`, the value is re-read as a top-level flag and applied. `--replace` then `-i`,
`--max-columns` then `-v`, `--context` then `-F` all silently change the search today.

This is a **separate task** — it needs a value-arity table for flags whose semantics cyrup ignores,
which is a different change from recognising a named group. Do not widen this task to cover it.

---

## 4. Out of scope — unchanged

- **Do not delete, reword, or gate the PCRE2 hint.** Pi's binary has the feature
  (`release.yml:177`), Pi passes the block through to the model (`grep.ts:309-310`), and removing it
  would create a parity gap. An early revision of this file prescribed exactly that; it was wrong.
- Do not write `cfg!(feature = "pcre2")` in `cyrup-tools` before §2 is answered — the feature is not
  declared, and the `cfg` fires rustc's `unexpected_cfgs` warning.
- Do not add `grep-pcre2` before §2 is answered.
- Do not turn any of these flags into an error — this module's contract is that a config never makes
  the tool fail.
- Do not implement `--pre` or `--pre-glob`: `grep`'s `FsOps` seam (`ops/mod.rs:437`) has no process
  capability, and a subprocess would bypass the `TraversalFs`/`ProtectedFs` decorators.
- Do not implement an in-process `-z`: not parity, and unreachable anyway (the workspace's
  `async-compression` pin covers 4 of ripgrep's 7 formats, with an async decoder against a blocking
  reader).
