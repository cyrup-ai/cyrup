---
stage: new
status: done
updated: 2026-08-22 19:32
---

# Decompose SessionBuilder::build's 1177-Line Async Function

**Crate:** `crates/cyrup-session-svc` · **Severity:** high · **Effort:** large

## Description

`SessionBuilder::build` spans `src/builder.rs:595-1771` — 1177 lines with 100 `let` bindings at the function's own indent level (147 including nested) — and is the only large method in an `impl SessionBuilder` whose other 14 methods are 6-22 line setters. It already documents its own seams with 13 numbered banners: 605 settings+trust, 692 auth, 697 session tree, 801 model resolution, 807 tools+isolation+policy, 900 live host-services, 926 extension host, 996 resources discovery (246 lines), 1242 context store + system prompt (179 lines), 1421 transcript seed, 1444 ext host seams, 1450 agent loop (252 lines), 1702 assemble. The crate paid down this exact shape once already (6297-line session.rs into 21 modules, largest now 696), so the pattern and reviewer appetite exist. Plan for wide interfaces rather than narrow tuples: step 5 alone reads roughly 11 inbound values (cfg 24 references, settings 18, packages 10, provider 7, manager 7, host 6, cwd 6, agent_dir 6, ext_host 5, live 3, trusted 2) and binds ~20 names of which six are consumed later, and the final assembly at L1702-1771 is a struct literal fed by 27 distinct bindings from every earlier step. Do BUILDER_HELPERS_MODULE_SPLIT first — it removes ~990 lines and is what makes this reviewable.

## Acceptance Criteria

- [ ] BUILDER_HELPERS_MODULE_SPLIT has landed and `src/builder/` exists before this work starts (builder/mod.rs around 1800 lines at the outset).
- [ ] `build()` lives in `src/builder/build.rs` and is under 250 lines; the extracted steps are `pub(super)` functions in `src/builder/steps/`, covering at minimum resources discovery, the agent loop, context store + system prompt, and the session tree.
- [ ] Step boundaries use a shared `BuildCtx` or per-step Params/Output structs — no step takes more than 4 positional arguments — and the final assembly block stays inline.
- [ ] No file under `src/builder/` exceeds 800 lines, and `rg -c '^    let ' crates/cyrup-session-svc/src/builder/build.rs` is under 30.
- [ ] `git diff src/lib.rs` shows the `pub use builder::{…}` group at lines 47-50 unchanged; `cargo test -p cyrup-session-svc` reports 311 passing and `cargo clippy -p cyrup-session-svc --all-targets` reports zero warnings.

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### Decompose SessionBuilder::build() — a single 1177-line async fn with 13 self-documented numbered steps

`CONFIRMED` · severity **high** · effort **large** · dimension `large-files`

**Evidence.** /home/user/cyrup/crates/cyrup-session-svc/src/builder.rs:595 `pub async fn build(self) -> Result<AgentSession, SessionServiceError>` closes at L1771 — 1177 lines, 100 top-level `let` bindings. It is the only large method in `impl SessionBuilder` (L437-1772); the other 14 methods (new:439, trust_store:461, trust_prompt:469, provider_resolver:477, stream_fn:487, key_resolver:495, skills_override:505, context_files_override:517, settings_store:535, auth:542, cli_settings:554, with_native_extension:561, load_persisted_catalog_overlay:573) are 6-22 line setters. Its own numbered banners: 605 (1. settings+trust), 692 (2. auth), 697 (2b. session tree), 801 (3. model resolution), 807 (4. tools+isolation+policy), 900 (4a. live host-services), 926 (4b. extension host), 996 (5. resources discovery, 246 lines), 1242 (6. context store + system prompt, 179 lines), 1421 (7. transcript seed), 1444 (8. ext host seams), 1450 (9. agent loop, 252 lines), 1702 (10. assemble). Verified coupling: `awk NR>=1702 && NR<=1771 | grep -oE '^\s+[a-z_]+,'` yields 27 field bindings (telemetry_enabled, shell, dynamic_tools, handle, bash_session_env, read_model_vision, cwd, session_dir, settings, auth, resources, startup_diagnostics, model_config, catalog_overlay, ext_host, guest_providers, system_prompt, host_services, agent, manager, fanout, provider_swap, services, model_ref, session_cancel, session_id, model_fallback_message, extras).

**Why it matters.** 1177 lines with 100 live bindings in one scope is past what a reader can hold; the compiler cannot tell you which step owns which value, and every edit risks colliding with a name bound 600 lines away. The crate already paid down this exact shape once (6297-line session.rs -> 21 modules under src/session/, largest now 696), so both the pattern and the reviewer appetite are established, and builder.rs is now the second-largest file in the crate.

**Fix.** Do finding #3 FIRST — moving the 570-line free-function tail and the 424-line test mod out drops builder.rs to ~1780 lines and makes this refactor reviewable. Then introduce `src/builder/` mirroring `src/session/`: `builder/mod.rs` (SessionConfig, SessionTarget, NoTools, ExtensionFlagValue, SessionBuilder + setters) and `builder/build.rs` holding build(). Extract steps into `builder/steps/*.rs` as `pub(super) async fn`, but plan for wide interfaces: define one `pub(super) struct BuildCtx` (or per-step Params/Output structs) rather than positional args — the measured fan-in is ~11 values for step 5 and 27 for the final assembly, so narrow tuple signatures will not work. Highest value per unit of risk, in order: step 5 resources (L996-1241), step 9 agent loop (L1450-1701), step 6 context store + prompt (L1242-1420), step 2b session tree (L697-800). Leave step 10 inline. Keep src/lib.rs:47-50 `pub use builder::{...}` unchanged.
