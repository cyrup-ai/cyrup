---
stage: exec
status: done
updated: 2026-08-22
---

# All Four Callers Discard The chmod Result That paths.rs Was Deliberately Changed To Return

> Source: `intercom-hygiene-audit` workflow. Severity **medium**, effort **small**.
> Every claim below was produced by a finder agent and then reproduced by an independent
> adversarial verifier; findings that did not reproduce were dropped.

## Scope

- `crates/cyrup-intercom/src/broker/lifecycle.rs`
- `crates/cyrup-intercom/src/transport/spawn.rs`
- `crates/cyrup-intercom/src/paths.rs`

## Description

`paths::restrict_intercom_runtime_file` returns `std::io::Result<()>` (crates/cyrup-
intercom/src/paths.rs:131) explicitly so a chmod failure cannot be lost — its doc states the rule
("A chmod failure is never silently discarded anywhere in pi, so this must propagate rather than
swallow", src/paths.rs:121-125) and a regression test pins it (src/paths.rs:236-245), whose
comment records that the pre-fix code "returned `()` and discarded the `set_permissions` result
via `let _ = ...`". I re-ran `grep -rn 'restrict_intercom_runtime_file' --include=*.rs` over the
whole repo: exactly four call sites exist, and all four discard the Result with `let _ =` —
src/broker/lifecycle.rs:160, :187, :192 and src/transport/spawn.rs:412. The signature change and
its test therefore guard a value no caller reads: a chmod failure is still silently discarded,
exactly as before the fix, just one stack frame higher.

## Why it matters

The crate paid for a guarantee it does not get. A Result-returning signature, a `# Errors` doc
section and a dedicated regression test all assert that chmod failures propagate, but 4 of 4
callers throw the value away, so the observable behaviour is identical to the pre-fix code the
test was written to prevent. That is a maintenance trap in both directions: a future reader sees
the test pass and concludes runtime files are guaranteed 0600, while in fact a `set_permissions`
failure (EPERM on a filesystem that rejects the mode, a racing unlink, an exotic mount) leaves
`broker.sock`/`broker.port.json`/`broker.pid` at umask mode with no log line, no error and no test
that would ever notice. The port file carries this run's `stateId` credential and the socket is
the broker's whole authority surface, so the mode is defence-in-depth that the code claims to
enforce and silently does not.

## Evidence

- Re-ran `grep -rn 'restrict_intercom_runtime_file' --include=*.rs .` from the repo root: 4 call sites, all `let _ =` — crates/cyrup-intercom/src/broker/lifecycle.rs:160, :187, :192, crates/cyrup-intercom/src/transport/spawn.rs:412 (the other hits are the definition at src/paths.rs:131, two test calls at src/paths.rs:243/:252, and a doc mention in crates/cyrup-it/tests/intercom/broker_startup_fail_fast.rs:9)
- crates/cyrup-intercom/src/paths.rs:131 — `pub fn restrict_intercom_runtime_file(file_path: &Path) -> std::io::Result<()>`, body `set_permissions(...)?`
- crates/cyrup-intercom/src/paths.rs:121-125 (doc, re-read verbatim) — "A chmod failure is never silently discarded anywhere in pi, so this must propagate rather than swallow."
- crates/cyrup-intercom/src/paths.rs:236-245 — test `restrict_runtime_file_propagates_set_permissions_failure`, whose doc comment names the exact anti-pattern the call sites still use
- crates/cyrup-intercom/src/broker/lifecycle.rs:186-187 — `std::fs::write(&port_path, endpoint.to_port_file_body())?;` then `let _ = paths::restrict_intercom_runtime_file(&port_path);`; :191-192 — same shape for `pid_path`
- crates/cyrup-intercom/src/broker/lifecycle.rs:101 — `pub async fn run() -> std::io::Result<()>`, so `?` is available at :160/:187/:192 with no signature change; the arm at :158-161 evaluates to `Option<String>` inside `run`, so `?` there compiles unchanged
- Measured, not assumed: compiled and ran a program calling `std::fs::write` then reading `permissions().mode() & 0o777` under the session umask (0022) — result 644. `std::fs::write` takes no mode, so the discarded chmod is the only thing narrowing these files to 0600
- CORRECTION to the original evidence: the containing directory is created 0700 and that failure IS propagated — crates/cyrup-intercom/src/paths.rs:110-118 `ensure_intercom_runtime_dir` does `create_dir_all(...)?` then `set_permissions(dir, 0o700)?`, called with `?` at src/broker/lifecycle.rs:113. So a discarded file chmod does not expose `stateId` to other unprivileged users; the 0700 dir already blocks traversal
- No suppressing annotation exists at any of the four sites: `grep -rn 'CYRUP-DELTA' src/broker/lifecycle.rs` returns :138, :204, :248 — none of them at :160/:187/:192; and `grep -rn 'best-effort|fire-and-forget|deliberately discard' src/` returns no hit near these calls

## Required fix

Propagate at the three broker-startup sites, where propagation is free: `broker::lifecycle::run`
already returns `std::io::Result<()>` (src/broker/lifecycle.rs:101), so replace `let _ =
paths::restrict_intercom_runtime_file(x);` with `paths::restrict_intercom_runtime_file(x)?;` at
src/broker/lifecycle.rs:160, :187 and :192 — a broker that cannot secure its own runtime files
should fail startup rather than publish them, which is the uncaught-throw semantics the paths.rs
doc cites for `broker.ts:128-130,140-141`. At src/transport/spawn.rs:412 the enclosing
`acquire_spawn_lock` returns `bool` and propagation is not available, so log instead: `if let
Err(e) = paths::restrict_intercom_runtime_file(lock_path) { tracing::warn!(error = %e, "failed to
restrict spawn lock permissions"); }`. Either way the guarantee the test asserts becomes real.
(The separate create-at-umask-then-chmod window could be closed with
`OpenOptions::new().mode(INTERCOM_RUNTIME_FILE_MODE)`, matching upstream's `writeFileSync(..., {
mode })`, but that is a distinct change and the 0700 dir already contains the exposure.)

## Acceptance Criteria

- [ ] The fix above applied as written; no scope beyond it.
- [ ] Port fidelity preserved — no `broker.ts`/`client.ts`/`paths.ts` citation dropped, no
      `[CYRUP-DELTA]` note removed, no ported branch collapsed.
- [ ] Baseline recorded before the change and matched after:
      `cargo clippy -p cyrup-intercom --all-targets`, `cargo test -p cyrup-intercom --lib`.
- [ ] `cargo build -p cyrup` still succeeds.
