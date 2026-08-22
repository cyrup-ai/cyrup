---
stage: new
status: done
updated: 2026-08-22 19:32
---

# Take The Crate's Four Live Clippy Warnings To Zero

**Crate:** `crates/cyrup-session-svc` · **Severity:** medium · **Effort:** small

## Description

`cargo clippy -p cyrup-session-svc --all-targets` emits exactly four warnings today: `src/bash.rs:219` a no-op `drop(sink);` on a closure that does not implement Drop (verified deletable — `buffer.finish()` at :220 still compiles because the `&mut buffer` loan is dead after the `match`); `src/bash.rs:138` `run_bash` at 8/7 arguments, with exactly one caller at `src/session/bash.rs:122`; `src/tests/fork_parent_and_unsaved_guard.rs:64` a needless `&` (clippy offers an autofix); and `src/tests/round9_l5res.rs:578` a `Mutex<Vec<(String, PathBuf, Vec<(String,String)>)>>` tripping type_complexity. The same pass settles a policy contradiction: `src/session/mod.rs:310` silences `too_many_arguments` on a 10-parameter `from_parts` with no stated rationale while `run_bash` warns forever, so a reviewer has no rule to apply to the next wide function. There is no `-D warnings` gate anywhere in the repo (no `.cargo/config.toml`, no `.github/workflows`), so nothing mechanical stops the count growing — at zero, adding a gate becomes a one-line option.

## Acceptance Criteria

- [ ] `cargo clippy -p cyrup-session-svc --all-targets 2>&1 | grep -c '^warning'` returns 0.
- [ ] `src/bash.rs:219` `drop(sink);` is deleted and no `#[allow(clippy::drop_non_drop)]` was added anywhere.
- [ ] `run_bash`'s 8/7 warning is resolved either by a `RunBashArgs` params struct (with `src/session/bash.rs:122`, the sole caller, updated) or by `#[allow(clippy::too_many_arguments)]` carrying a one-line rationale — and if the allow route is taken, the bare allow at `src/session/mod.rs:310` gains the identical rationale so both sites state one policy.
- [ ] `src/tests/round9_l5res.rs` declares a named `type SeenExec = (String, PathBuf, Vec<(String, String)>);` with a comment naming command/cwd/env, and the `&` at `src/tests/fork_parent_and_unsaved_guard.rs:64` is gone.
- [ ] `cargo test -p cyrup-session-svc` still reports 311 passing tests.

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### Delete the no-op `drop(sink)` in `run_bash` (clippy::drop_non_drop fires today)

`OVERSTATED` · severity **low** · effort **small** · dimension `lint-debt`

**Evidence.** /home/user/cyrup/crates/cyrup-session-svc/src/bash.rs:219 — `drop(sink);` immediately before `let (output, truncated, full_output_path) = buffer.finish();` at :220. Live clippy output: `warning: call to std::mem::drop with a value that does not implement Drop` / `note: argument has type {closure@crates/cyrup-session-svc/src/bash.rs:179:20: 179:33}` / `#[warn(clippy::drop_non_drop)] on by default`. Verified by deletion + `cargo check`: compiles.

**Why it matters.** This is one of only two live clippy warnings against this crate's production code, in a crate otherwise held to a zero-warning standard. More concretely: a maintainer editing this function will preserve a statement that does nothing, or assume borrow ordering is explicitly managed at this point when it is not.

**Fix.** Delete line 219. Verified: `buffer.finish()` at :220 still compiles because the closure's loan is dead after the `match`. Do not silence with `#[allow(clippy::drop_non_drop)]` — there is nothing to preserve. If a future edit genuinely needs an explicit borrow boundary, brace the `match operations { ... }` block and bind `status` from it.

**Verifier correction.** Evidence holds exactly as written and I verified the fix compiles — only the severity is inflated. `cargo clippy -p cyrup-session-svc --all-targets` does emit this warning at bash.rs:219 pointing at the closure declared at bash.rs:179. I deleted line 219 and ran `cargo check -p cyrup-session-svc`: clean, 0 errors (NLL ends the `&mut buffer` loan at the closure's last use inside the `match`, so `buffer.finish()` at the next line still compiles). I then restored the file; `git status` is clean. Corrected scope: this is a one-line deletion of a statement that compiles to nothing — real, worth doing, but cosmetic, so low rather than medium.

### Fold `run_bash`'s 8 parameters into a params struct, or make its `too_many_arguments` handling consistent with `AgentSession::from_parts`

`OVERSTATED` · severity **low** · effort **small** · dimension `lint-debt`

**Evidence.** /home/user/cyrup/crates/cyrup-session-svc/src/bash.rs:138 `pub(crate) async fn run_bash(` — live `warning: this function has too many arguments (8/7)`. Sole caller: /home/user/cyrup/crates/cyrup-session-svc/src/session/bash.rs:122. Contrasting policy: /home/user/cyrup/crates/cyrup-session-svc/src/session/mod.rs:310 `#[allow(clippy::too_many_arguments)]` above `pub(crate) fn from_parts(` (mod.rs:311) with 10 params and no rationale.

**Why it matters.** Not a latent bug (the types make transposition impossible), but the crate currently applies two opposite policies to the same lint 300 lines apart: one over-wide fn warns forever, a wider one is silenced with no stated reason. A reviewer has no rule to apply to the next wide function, and this is one of the crate's two permanent production warnings.

**Fix.** Cheapest correct option (small effort): add `#[allow(clippy::too_many_arguments)]` at bash.rs:138 WITH a one-line rationale, and retrofit the same rationale onto session/mod.rs:310 so both sites state the policy. Fuller option (medium effort): introduce `struct RunBashArgs<'a> { proc: &'a Arc<dyn ProcOps>, shell: &'a ShellConfig, operations: Option<&'a dyn BashOperations>, cwd: PathBuf, command: String, bin_dir: Option<&'a Path> }` in bash.rs, reduce `run_bash` to `(args, cancel, on_chunk)`, and update the single call site at session/bash.rs:122. Do NOT reuse `BashOptions` — it is a different, caller-facing bag.

**Verifier correction.** The factual core checks out; two supporting claims do not, and the severity is inflated. VERIFIED: the warning is live at bash.rs:138 (8/7); the 8 params are exactly as listed at bash.rs:139-146; there is exactly one caller, session/bash.rs:122 (`rg run_bash` outside tests returns only the definition, that call, and two comments); session/mod.rs:310 does carry a bare `#[allow(clippy::too_many_arguments)]` over `from_parts` (10 params, mod.rs:312-321) with no rationale comment. REFUTED sub-claims: (a) the 'swap-two-arguments bug waiting to happen' rationale does not survive contact with the signature — all eight params have mutually distinct types (`&Arc<dyn ProcOps>`, `&ShellConfig`, `Option<&dyn BashOperations>`, `PathBuf`, `String`, `Option<&Path>`, `CancelToken`, `BashChunkSink`); no two can be transposed without a type error, so the safety argument is empty and only readability/consistency remain. (b) 'A natural home already exists: BashOptions' is wrong — `BashOptions` (bash.rs:74-100) carries `exclude_from_context` and `id`, which `run_bash` never receives, and carries none of `proc`/`shell`/`cwd`/`command`/`bin_dir`; the finding's own fix correctly proposes a *new* struct instead. Corrected scope: a readability + policy-consistency cleanup on one internal fn with one caller — low, not medium.

### Fix the two remaining test-code warnings so the crate is genuinely at zero

`CONFIRMED` · severity **low** · effort **small** · dimension `lint-debt`

**Evidence.** /home/user/cyrup/crates/cyrup-session-svc/src/tests/fork_parent_and_unsaved_guard.rs:64:60 — `faux_text(&format!("ANSWER-{i}"))`, `warning: the borrowed expression implements the required traits` (clippy::needless_borrows_for_generic_args, auto-fixable). /home/user/cyrup/crates/cyrup-session-svc/src/tests/round9_l5res.rs:578:11 — `seen: Mutex<Vec<(String, PathBuf, Vec<(String, String)>)>>` on `struct RecordingBashOps` (declared at :577), `warning: very complex type used` (clippy::type_complexity); the tuple is written at :590-593 as (command, cwd, env).

**Why it matters.** These two plus the two bash.rs findings are the entire delta between this crate and a clean `cargo clippy`. Verified there is no `.cargo/config.toml` and no CI workflow setting `-D warnings`, so nothing mechanical stops the count from growing — the only defence today is that four is small enough for a human to notice. At zero, a `-D warnings` gate becomes a one-line option; at four it is not.

**Fix.** (1) Drop the `&` at fork_parent_and_unsaved_guard.rs:64 (or run `cargo clippy --fix --lib -p cyrup-session-svc --tests`, which clippy itself offers for this one). (2) At round9_l5res.rs:577 add `type SeenExec = (String, PathBuf, Vec<(String, String)>);` with a comment naming the three fields (command, cwd, env), and declare `seen: Mutex<Vec<SeenExec>>`.
