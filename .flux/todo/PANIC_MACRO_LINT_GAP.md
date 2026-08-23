---
stage: new
status: done
updated: 2026-08-23 00:06
---

# Close the panic-policy gap: deny clippy::unreachable/todo/unimplemented (3 live sites, one documented as a deliberate dodge) and remove the 5 indexing_slicing escapes in escape_reassembly.rs

> Found by an eight-lens workspace hygiene sweep. Every count below was reproduced
> against the tree before this task was filed.
> **Priority:** medium · **Effort:** small
> **Crates:** `cyrup`, `cyrup-ext`, `cyrup-ext-sdk`, `cyrup-tui`, `workspace-root`

**The lint table has a hole.** `[workspace.lints.clippy]` (verified verbatim) denies `unwrap_used`, `expect_used`, `panic`, `indexing_slicing` and warns `return_self_not_must_use` — but not `unreachable`, `todo` or `unimplemented`, which lower to the exact same `std::panic` (and, in the guest crate, the same wasm trap). Three non-test call sites already exist (verified by grep):

```
crates/cyrup/src/main.rs:1190:        AppMode::Interactive => unreachable!("interactive mode is handled before this match"),
crates/cyrup-ext/src/caps/http.rs:547:                        unreachable!("just matched StreamSlot::Idle above")
crates/cyrup-ext-sdk/src/example.rs:73:        unreachable!("demo_boom: this entry renderer always faults (X15 fixture)")
```

The policy is being routed around **in writing**: `crates/cyrup-ext-sdk/src/example.rs:68-69` reads *"`unreachable!` rather than `panic!`: the workspace denies `clippy::panic`, and the trap is identical either way."* The worst site is `main.rs:1190` — the shipped binary's mode dispatch aborts the process instead of returning an error.

**Five `indexing_slicing` escapes in one file.** Of the workspace's 842 clippy panic-policy allow attributes, only 6 sit in shipping code — 5 of them clustered in `crates/cyrup-tui/src/escape_reassembly.rs` (verified at lines 409, 449, 569, 601, 706; the sixth is a static-regex expect at `cyrup-ext/src/caps/proc/npx_resolver.rs:65`). Three of the five (569, 601, 706) justify themselves with the comment *"callers guarantee `buf.len() >= 3`"* — precisely the unenforced invariant `indexing_slicing` exists to reject. `decode_csi_modifier_key_code` (:570), `decode_csi_special_key_code` (:602) and `decode_csi_u_encoded_key_code` (:707) are private fns over `&[u8]` whose bodies open with `&buf[2..buf.len() - 1]`; a caller passing a shorter buffer underflows `usize` and panics in the TUI input reader thread. The precondition holds today only because `decode_csi` (:450) checks `buf.len() < 3` before dispatching — nothing in the type system keeps a fourth caller honest.

This is distinct from the queued `STR_SLICE_PANIC_LINT` task, which concerns `&str` slicing (`clippy::string_slice`); these are byte-slice/index escapes.

## Acceptance Criteria

- [ ] [workspace.lints.clippy] in the root Cargo.toml denies `unreachable`, `todo` and `unimplemented` alongside the existing four denies
- [ ] crates/cyrup/src/main.rs:1190 returns an error from the mode dispatch instead of aborting; the shipped binary contains no `unreachable!`/`todo!`/`unimplemented!` outside tests
- [ ] crates/cyrup-ext/src/caps/http.rs:547 is rewritten as a let-else / explicit Err path with no abort macro
- [ ] The one intentional trap fixture (cyrup-ext-sdk/src/example.rs) uses a narrowly-scoped per-item `#[allow(..., reason = "...")]`, and the misleading doc comment at example.rs:68-69 describing the deny-panic workaround is removed
- [ ] All five `#[allow(clippy::indexing_slicing)]` attributes in crates/cyrup-tui/src/escape_reassembly.rs (lines 409, 449, 569, 601, 706) are gone, with the bodies rewritten using `buf.get(..)` / `buf.get(n)` and let-else
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes with the widened lint table, and the existing escape-sequence decoding tests in cyrup-tui still pass (including short-buffer inputs, which must now return Err rather than panic)

## Verifying command

```bash
cd /home/user/cyrup && sed -n '/^\[workspace.lints.clippy\]/,/^$/p' Cargo.toml && grep -rn --include=*.rs -E '^[^/]*\b(unreachable|todo|unimplemented)!' crates/cyrup/src crates/cyrup-ext/src/caps crates/cyrup-ext-sdk/src && grep -n '#\[allow(clippy::indexing_slicing)\]' crates/cyrup-tui/src/escape_reassembly.rs
```
