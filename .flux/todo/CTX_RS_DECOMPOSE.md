---
stage: qa
status: needs-rework
updated: 2026-08-22 16:52
---

# Decompose ctx.rs — Two False Comment Claims Introduced By The Last Rework

The previous rework's **code** is correct and stays: `every_ctx_submodule_is_in_sdk_sources` now
counts scanned files and asserts `scanned >= 13`, which genuinely fires (demonstrated: raised to 99
it fails naming the real count of 13). `register_tool` is still at `ctx/base.rs:96`. Builds, clippy
and 310 tests are green with zero warnings.

**What is outstanding is two comments that read plausibly and resolve to something untrue.** Both
were added by that rework, and both are the exact class this repo polices hardest — the EXT-036 /
EXT-072 / EXT-073 apparatus, and the citation lint in `wit_world_sync.rs`, exist because a comment
that resolves to the wrong thing is worth less than none. Neither is a behaviour bug; both are
load-bearing *rationale* text, which is why they matter.

---

## 1. The threshold comment cites a scenario that cannot reach the assertion

[`crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs:78-81`](../../crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs)

```rust
// Non-vacuity: a scan that finds nothing satisfies the containment loop trivially, so the COUNT
// is the only thing that proves this guard did any work. The literal is one per submodule plus
// `mod.rs`, not `> 0`, so deleting a submodule without deleting its `include_str!` line is
// caught here too — the direction the containment check cannot see.
```

The first sentence is correct and should stay. **The `not > 0` justification is false.** Deleting a
submodule while leaving its `include_str!` line is a hard COMPILE error, so the test binary never
builds and this assertion never runs. Verified directly:

```
error: couldn't read `./definitely_not_here.rs`: No such file or directory (os error 2)
 --> t.rs:1:17
  |
1 | const X: &str = include_str!("./definitely_not_here.rs");
```

The compiler already guards that direction, far more forcefully than an assertion could.

What the literal `13` **actually** buys over `> 0` is the opposite case: deleting a submodule
*together with* its `include_str!` line. That shrinks the tree silently — the containment loop still
passes, because every remaining file is still listed — and the count is the only thing that notices.
It makes a structural shrink a deliberate edit of the number rather than an unremarked one.

**Fix:** keep `scanned >= 13`; restate the second half with the reachable scenario, e.g.

```rust
    // Non-vacuity: a scan that finds nothing satisfies the containment loop trivially, so the COUNT
    // is the only thing that proves this guard did any work. The literal is one per submodule plus
    // `mod.rs` rather than `> 0` so that REMOVING a submodule is deliberate too: dropping the file
    // and its `include_str!` line together leaves the containment loop still passing over a smaller
    // tree, and only the count notices. (The opposite slip — dropping the file but keeping the
    // `include_str!` — needs no guard here; it does not compile.)
```

## 2. `base.rs` claims `register_tool` "fronts no WIT import at all" — it fronts `registration.register-tool`

[`crates/cyrup-ext-sdk/src/ctx/base.rs:6-8`](../../crates/cyrup-ext-sdk/src/ctx/base.rs)

```rust
//! [`Ctx::register_tool`] also lives here rather than in `tools`: it fronts no WIT import at all —
//! it hands a descriptor to the guest's own `register_tool_late` for the host to pick up at its
//! next tool refresh — so it belongs with the type rather than with the `ext-tools` introspection.
```

The chain, verified end to end:

| Site | Fact |
|---|---|
| `ctx/base.rs:103` | `crate::guest::register_tool_late(tool)` |
| `guest.rs:117` | `registration::register_tool(&lower_tool_descriptor(&tool.descriptor))` |
| `guest.rs:29` | `use bindings::cyrup::ext::{registration, types, ui};` — a WIT bindings module |
| `wit/world.wit` | `interface registration { register-tool: func(t: tool-descriptor); … }` |
| `world_import_coverage.rs:155` | `registration` is in `IMPORT_INTERFACES` |

So `Ctx::register_tool` reaches a declared WIT import one hop away. Worse for the argument as
written: the interface it reaches is `registration`, which is one of the **two interfaces `tools.rs`
is documented to front** (`//! … the `ext-tools` and `registration` WIT imports`). The stated reason
therefore points at the opposite conclusion from the one it draws.

The charitable reading — "the method body contains no direct `bindings::` call" — is literally true
of the body, but the surrounding doc is entirely about which WIT interface each submodule fronts, so
that is not how it will be read, and the conclusion still does not follow from it.

**The PLACEMENT stays.** `register_tool` belongs on `base.rs` and must not move: the cut plan
assigned it there, and it is a *write* performed through the guest's own late-registration path
(which also buffers into `LATE_TOOLS`), not part of the read-only introspection surface `tools.rs`
collects. Only the stated reason is wrong.

**Fix:** replace the clause with the true one, e.g.

```rust
//! [`Ctx::register_tool`] also lives here rather than in `tools`, and it is the one member of this
//! file that does front a WIT import: `guest::register_tool_late` calls `registration.register-tool`
//! and buffers the executable half into `LATE_TOOLS`, so the host can re-materialize the tool at its
//! next refresh. It sits with the type rather than in `tools` because that module is the read-only
//! introspection surface over `ext-tools`/`registration`, and this is a registration WRITE — the
//! runtime twin of the `init`-time registrations `guest::push_registrations` flushes.
```

Adjust the wording if a shorter form reads better; the requirement is only that no clause claims
there is no WIT import behind it, and that any reason given survives being checked.

## Definition of done

- [ ] `world_import_coverage.rs`'s threshold comment no longer cites the non-compiling scenario, and
      states the reachable one it actually guards; `scanned >= 13` itself unchanged
- [ ] `ctx/base.rs`'s `register_tool` clause no longer claims it fronts no WIT import, and names
      `registration.register-tool` reached via `guest::register_tool_late`; placement unchanged at
      `base.rs:96`
- [ ] Every other claim in both comments re-checked against the source before it is left standing
- [ ] `cargo check -p cyrup-ext-sdk` and `--target wasm32-wasip2` still clean;
      `cargo clippy -p cyrup-ext-sdk --all-targets` still clean
- [ ] `cargo test -p cyrup-ext-sdk` (17) and `cargo test -p cyrup-ext` (293) still pass
- [ ] `cargo doc -p cyrup-ext-sdk --no-deps` still reports zero warnings under `ctx/`

Do not touch anything else. `cyrup-it`'s `--features it` errors remain pre-existing and out of scope
(`CYRUP_IT_COMPILE_ERRORS.md`).
