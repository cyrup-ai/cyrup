---
stage: new
status: done
updated: 2026-08-22 18:30
severity: medium
effort: small
category: tooling-gate
---

# Make The Documented Clippy Gate Able To Fail

## Description

The repo documents its clippy gate as an exit-code check in two places:

- [`README.md:145`](../../README.md) — `cargo clippy --workspace --all-targets  # REQUIRED — the no-panic policy only fires here`
- [`spec/flux/README.md:117`](../../spec/flux/README.md) — `cargo clippy --workspace --all-targets --features test-fixtures; echo "exit=$?"   # MUST be 0`

[`README.md:149-151`](../../README.md) states the policy is expressed as `[workspace.lints.clippy]`, which at [`Cargo.toml:97-101`](../../Cargo.toml) denies exactly four lints: `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`. Every other lint is warn-level, and clippy exits 0 on warnings — so the documented contract ("exit MUST be 0") is satisfied today *while the crate is not clean*.

Verified by running it:

```
$ cargo clippy -p cyrup-agent --all-targets ; echo exit=$?
exit=0
```

with three diagnostics in that same output (`cyrup-agent (lib) generated 2 warnings` + `(lib test) generated 3 warnings (2 duplicates)`):

- `clippy::collapsible_if` — [`crates/cyrup-agent/src/agent/run/mod.rs:144`](../../crates/cyrup-agent/src/agent/run/mod.rs), nested `if let Some(f) = &self.header_fn { if let Some(h) = f(model) {` inside `headers_for`
- `clippy::needless_return` — [`crates/cyrup-agent/src/proxy.rs:584`](../../crates/cyrup-agent/src/proxy.rs), a bare `return;` as the last statement of the cancel branch
- `clippy::err_expect` — [`crates/cyrup-agent/src/tests/area02_backlog.rs:837`](../../crates/cyrup-agent/src/tests/area02_backlog.rs), `.err().expect("a reset is refused")`, suggested `expect_err`

All three are default-on warn lints and machine-fixable; clippy reports `--fix --lib -p cyrup-agent` applies 2 suggestions and `--tests` applies 1. The `collapsible_if` fix is a let-chain, which `edition = "2024"` / `rust-version = "1.96"` ([`Cargo.toml:88-89`](../../Cargo.toml)) accepts.

Why it matters: there is no CI in this repository, so the documented command is the only enforcement that exists. A gate that cannot fail lets warning count drift upward indefinitely with no signal, and the next contributor sees warnings on a build the docs call passing.

Scope constraint measured: `cargo clippy -p cyrup-provider` reports 23 warnings, so a workspace-wide `-D warnings` would not pass today.

## Scope

In scope: zeroing the three `cyrup-agent` clippy diagnostics above, and adding a strict, crate-scoped clippy invocation to the two documented gate blocks.

Out of scope:
- Cleaning `cyrup-provider`'s 23 warnings or any other crate — do not widen `-D warnings` to the workspace.
- Changing `[workspace.lints.clippy]` in `Cargo.toml`. The four deny lints stay exactly as they are; no new deny entries.
- Rustdoc warnings — owned by the queued `CARGO_DOC_WARNINGS` task. Do not touch doc comments here.
- Feature-combination builds — owned by queued `BUILD_FEATURE_COMBINATIONS`.
- Any behavior change. The three fixes are semantics-preserving rewrites.

## Approach

1. Run `cargo clippy --fix -p cyrup-agent --lib --tests --allow-dirty`.
2. Read all three diffs before accepting. Specifically confirm the `run/mod.rs:144` rewrite reads as a clean let-chain (`if let Some(f) = &self.header_fn && let Some(h) = f(model) {`) and that the doc comment above `headers_for` is untouched. If `--fix` leaves any of the three unapplied, hand-edit it.
3. Re-run `cargo clippy -p cyrup-agent --all-targets` and confirm zero `warning:` lines attributed to `cyrup-agent`.
4. Edit [`README.md:145`](../../README.md) to keep the existing workspace line and add a strict crate-scoped line directly under it:
   `cargo clippy -p cyrup-agent --all-targets -- -D warnings  # cyrup-agent is warning-clean; keep it that way`
5. Edit [`spec/flux/README.md:117`](../../spec/flux/README.md) the same way, preserving `--features test-fixtures` on the strict line so both commands cover the same target set.

Decision, stated rather than left open: the strict flag is scoped to `-p cyrup-agent` and not the workspace, because `cyrup-provider`'s 23 warnings would make a workspace-wide `-D warnings` fail on arrival — a documented command that fails the moment it lands is worse than the current gate.

6. Verify the newly documented commands by running them verbatim and checking `exit=0`.

## Acceptance Criteria

- [ ] `cargo clippy -p cyrup-agent --all-targets -- -D warnings; echo exit=$?` prints `exit=0`
- [ ] `cargo clippy -p cyrup-agent --all-targets --features test-fixtures -- -D warnings; echo exit=$?` prints `exit=0`
- [ ] `cargo clippy -p cyrup-agent --all-targets 2>&1 | grep -c '^warning:'` is `0`
- [ ] `grep -n 'D warnings' README.md spec/flux/README.md` shows one strict `-p cyrup-agent` line in each file
- [ ] `git diff Cargo.toml` is empty — `[workspace.lints.clippy]` unchanged
- [ ] `cargo test -p cyrup-agent` still passes 140/140
- [ ] `git diff --stat` touches only the three cited source files plus the two READMEs
