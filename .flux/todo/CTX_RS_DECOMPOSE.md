---
stage: exec
status: done
updated: 2026-08-22 16:44
---

# Decompose ctx.rs Into Submodules — Outstanding Rework

The decomposition itself is complete and verified. `src/ctx.rs` is gone; `src/ctx/` holds `mod.rs`
plus twelve submodules cut on the WIT-import-interface boundary; content fidelity is exact; both
breaking file-path references are fixed; host and `wasm32-wasip2` builds, clippy, and 310 tests are
green with zero warnings. **Everything below is what is left.** Do not redo the split.

---

## 1. The drift guard's vacuity backstop is dead code (must fix)

[`crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs:76-79`](../../crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs)

```rust
assert!(
    !missing.is_empty() || SDK_SOURCES.contains("mod base;"),
    "the `src/ctx/` scan found nothing to check, so this guard would be vacuous"
);
```

**This assertion can never fire.** `ctx/mod.rs:36` declares `mod base;`, and `ctx/mod.rs` is
unconditionally in `SDK_SOURCES` (`:38`), so `SDK_SOURCES.contains("mod base;")` is a constant
`true` and the whole disjunction is a constant `true`. If `read_dir` ever yielded zero `.rs` files —
the exact scenario the message names — `missing` would be empty, the second operand would still be
`true`, and the guard would pass while checking nothing.

This is the anti-pattern the file exists to catch, one level further down: a check that reads as
covering something it does not. It is also inconsistent with the file's own idiom — `:99`
("so this test would be vacuous") and `:201` (`assert!(checked >= 60, …)`) are both real
non-vacuity checks — and with the sibling guard this same change added to `cyrup-ext`, whose
`assert!(!ctx.is_empty(), …)` is correct.

**Fix:** assert on what was actually scanned, not on a constant. Count the `.rs` files the loop
accepted and require the real number:

```rust
fn every_ctx_submodule_is_in_sdk_sources() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ctx"));
    let mut missing: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    for entry in std::fs::read_dir(dir).expect("src/ctx is a directory") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        scanned += 1;
        let body = std::fs::read_to_string(&path).expect("readable source file");
        let probe: String = body.chars().take(200).collect();
        if !probe.is_empty() && !SDK_SOURCES.contains(probe.as_str()) {
            missing.push(path.display().to_string());
        }
    }
    // Non-vacuity, for real this time: a scan that finds nothing passes the containment loop
    // trivially, so the count is the only thing that proves the guard did any work.
    assert!(
        scanned >= 13,
        "the `src/ctx/` scan found only {scanned} `.rs` file(s) — this guard would be vacuous"
    );
    assert!(
        missing.is_empty(),
        "these `src/ctx/` submodules are not in SDK_SOURCES, so their binding calls are invisible \
         to `every_declared_world_import_has_a_caller_in_the_sdk`: {missing:?}"
    );
}
```

**Prove the fix, both directions** — the guard was already shown to catch a missing file, so only
the new backstop needs demonstrating:

- Temporarily lower the threshold to `scanned >= 99`; the test must fail naming the real count.
  Restore it to `13`.
- `cargo test -p cyrup-ext-sdk` green afterwards.

Keep the threshold a literal `13` (one per submodule plus `mod.rs`), not `> 0`: it then also catches
a submodule being deleted without its `include_str!` line going too.

## 2. `base.rs`'s module doc does not account for `register_tool` (must fix)

[`crates/cyrup-ext-sdk/src/ctx/base.rs:1-4`](../../crates/cyrup-ext-sdk/src/ctx/base.rs) says the
file holds "the `ctx-state` getters, the `bus` emit/unsubscribe pair and the two `control` ops pi
puts on the BASE `ExtensionContext` (`abort`/`shutdown`), plus [`ExtMode`] and the [`Ctx`] type
itself."

`base.rs:92` is `Ctx::register_tool`, which no clause covers: it is late tool registration through
`crate::guest::register_tool_late`, not a `ctx-state`, `bus` or `control` op. Under a layout whose
whole claim is "one submodule per WIT import interface", the one member that fronts NO interface is
the one that most needs saying out loud — otherwise a reader looking for it goes to `tools.rs`
(which fronts `ext-tools`/`registration`) and does not find it.

Its PLACEMENT is correct and must not change — the cut plan assigned it to `base.rs` deliberately,
and it is not an `ext-tools` import. Only the doc needs a clause, e.g. append:

```rust
//! [`Ctx::register_tool`] also lives here rather than in `tools`: it fronts no WIT import at all —
//! it hands a descriptor to the guest's own `register_tool_late` for the host to pick up at its
//! next tool refresh — so it belongs with the type rather than with the `ext-tools` introspection.
```

## Definition of done

- [ ] `every_ctx_submodule_is_in_sdk_sources` asserts on a scanned-file count, and the threshold has
      been shown to fail when raised past the real count
- [ ] `base.rs`'s module doc accounts for `register_tool`, with its placement unchanged
- [ ] `cargo check -p cyrup-ext-sdk` and `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` still
      clean; `cargo clippy -p cyrup-ext-sdk --all-targets` still clean
- [ ] `cargo test -p cyrup-ext-sdk` (17) and `cargo test -p cyrup-ext` (293) still pass

Do not touch anything else. In particular `cyrup-it`'s `--features it` compile errors
(`usage_budget`, `steer_ack_dir`, `steer_capability_path`) are pre-existing, tracked separately in
`CYRUP_IT_COMPILE_ERRORS.md`, and out of scope here.
