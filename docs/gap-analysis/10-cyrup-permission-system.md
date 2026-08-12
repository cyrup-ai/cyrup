# 10 — cyrup-permission-system

Covers `cyrup/crates/cyrup-permission-system/` — the allow/ask/deny gate, its policy manager and evaluator, the wildcard matcher, the prompt/dedup layer, the parent↔child ask-forwarding spool, the prompt sanitizers and the install probe — measured against `pi-permission-system/` at upstream HEAD `9affcc9` = v0.8.0, where cyrup's port baseline is v0.7.1 (so the whole `v0.7.1..v0.8.0` delta is legitimately unported rather than broken). Headline: the three highest-value c8bd2ab items landed — the install latch and the silently-swallowed policy/config parse failures are genuinely fixed, and child ask-forwarding works for foreground subagents — but the crate still reads a store upstream deleted at top last-match-wins priority, starts its forwarding watcher exactly once per session, and gates itself behind a probe whose signal set is strictly narrower than the engine it guards. Re-baselined against HEAD `1806375` on 2026-08-03; every closure below was re-derived from code at HEAD, never from a commit message.

## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| PERM-001 | partially closed | Foreground half closed by `51bb11a`, independently re-proved (a real subprocess writes a real spool artifact). Background/detached half re-proved still broken by construction — remains open. |
| PERM-002 | **closed** | `6df5183`. `is_installed` now requires a *non-pristine* `config.json`; five refutation angles run, all failed, including tracing the wiring that makes the gate live in production. |
| PERM-003 | **closed** | `6df5183`. `manager_with_warnings` is the only production `PermissionManager` builder; both policy- and config-parse failures cross `HostServices::notify`. Follow-ons filed as PERM-021 / PERM-024. |
| PERM-004 | open | Untouched. Second upstream proof found: pi's `shouldExposeTool` now passes `applyPatternApprovalState` only `sessionApprovals`. |
| PERM-005 | open | Untouched. Four distinct defects, not one. |
| PERM-006 | open | Untouched. |
| PERM-007 | open | Untouched; upstream line refs corrected below. |
| PERM-008 | open | Untouched, but the prior evidence was **factually wrong** — `config.debug` *is* read. Corrected below. |
| PERM-009 | open | Untouched. The original line ref was correct; a later refresh "corrected" it into error. Restored below. |
| PERM-010 | open | Untouched. Blocked behind PERM-019. |
| PERM-011 | open | Untouched; config-field line ref corrected. |
| PERM-012 | open | Untouched. Fix lands in `cyrup-provider`, not this crate. |
| PERM-013 | open | Untouched. |
| PERM-014 | open | Untouched. `8854601` narrowed the window; did not close it. |
| PERM-015 | open | Untouched. |
| PERM-016 | open | Untouched; upstream refs were off by one, corrected. |
| PERM-017 | open (revisit-trigger only) | Untouched. Further correction: cyrup **documents** the omission as deliberate. Not a defect. |
| PERM-018 | open | Untouched. |
| PERM-019 | open | Untouched. |
| PERM-020 | partially closed | `f777e44` landed the `ext_config` half (five `env_lock()` sites, not four). Three unlocked sites remain — open. |
| PERM-021 | open | Untouched. |
| PERM-022 | open | Untouched. The crate's only uncontrollable-timing assertion. |
| PERM-023 | **new** | Install probe ignores agent-markdown `permission:` frontmatter the manager enforces. |
| PERM-024 | **new** | No extension-config refresh on `before_agent_start`. |

No status was overturned this round. Closed count: 2.

## Open items

> **⚠ THIS TABLE IS NOT THE COMPLETE OPEN SET.** 4 further items from the 2026-08-03
> surface-driven sweep live in their own table under `## Surface-sweep findings` (line ~309), with
> `-S` ids — **including 0 rated critical/high**. Enumerating only this table undercounts the
> area by 4 items, which is exactly how `SEAM-S01` (high) escaped a full audit pass on
> 2026-08-07. Count BOTH tables. See structural defect A in `00-residual-ledger.md`.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~PERM-001~~ **CLOSED** `513e45a` | high | parity-bug | M | Background/detached subagent runs cannot forward asks — parent anchor never reaches hop-2 |
| ~~PERM-005~~ **CLOSED** `513e45a` | high | parity-bug | M | Parent forwarding watcher started only at SessionStart, never retried |
| PERM-004 | medium | stale-port | M | `PermanentApprovalStore` read at highest last-match-wins priority on a file upstream deleted |
| PERM-006 | medium | not-ported | S | Skill-read and external-directory asks bypass the prompt-dedup cache |
| PERM-007 | medium | not-ported | L | No `/permission-system` command, no settings modal, no config-write path |
| PERM-008 | medium | not-ported | M | Debug/review audit log entirely unported |
| PERM-009 | medium | parity-bug | S | `should_expose_tool` keeps `bash` advertised despite a tool-level deny |
| PERM-010 | medium | not-ported | S | `enabled` master switch (v0.8.0) not ported |
| PERM-011 | medium | not-ported | M | No runtime yolo-mode API and no permission-request event channel |
| PERM-012 | medium | not-ported | M | `registerModelOptionCompatibilityGuard` (temperature stripping) has no cyrup counterpart |
| PERM-014 | medium | not-ported | M | Concurrent-duplicate ask collapse implemented in `dedup.rs` but never wired |
| PERM-018 | medium | cyrup-original | S | `is_installed` probes the raw config path, ignoring `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` |
| PERM-020 | medium | test-defect | S | Three env-mutating test sites still not serialized on `env_lock()` |
| PERM-022 | medium | test-defect | S | Spool test reads an artifact the child deletes on a timer |
| PERM-023 | medium | cyrup-original | S | Install probe ignores agent-scoped `permission:` frontmatter the manager enforces |
| PERM-013 | low | not-ported | M | `before_agent_start` result caching not ported |
| PERM-015 | low | not-ported | S | Wildcard patterns have no length bound |
| PERM-016 | low | not-ported | S | `parse_simple_yaml_map` does not drop `__proto__`/`constructor`/`prototype` |
| PERM-017 | low | not-ported | S | Forwarding-root agent-dir env overrides not ported (documented as deliberate) |
| PERM-019 | low | cyrup-original | S | Install state inferred from a byte-exact template comparison |
| PERM-021 | low | test-defect | S | PERM-003 test's re-arm assertion covers only the policy warning |
| PERM-024 | low | not-ported | S | Extension config not refreshed on `before_agent_start` |

## PERM-001 — Background/detached subagent runs cannot forward asks: parent anchor never reaches hop-2

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** high

**cyrup** — Foreground half is closed: `crates/cyrup-permission-system/src/extension.rs:99` aliases `CHILD_ENV_VAR` to `cyrup_ext_subagents::spawn::nested_events::CHILD_ENV`; the writer is `crates/cyrup-ext-subagents/src/spawn/nested_events.rs:776-778` (`child_role_env`); the parent anchor is written in the production planner at `crates/cyrup-ext-subagents/src/exec/mod.rs:1049-1056` (`opts.parent_session_id` → `.or_else(env::var(PARENT_SESSION_ENV_VAR))` → `env_overlay.insert`), fed for foreground runs from `crates/cyrup-ext-subagents/src/extension.rs:1104`; role selection at `crates/cyrup-permission-system/src/extension.rs:1121-1133`. The remaining break: `crates/cyrup-ext-subagents/src/background/runner_main.rs:1741` passes `parent_session_id: None`, and its justifying comment at `:1736-1740` ("inherited `CYRUP_SUBAGENT_PARENT_SESSION` in its OWN env from the hop-1 spawn") is false for a root orchestrator — a workspace-wide grep shows the only writer of that var is `exec/mod.rs:1055`, and no process sets it for itself. `crates/cyrup-ext-subagents/src/background/spawn_detached.rs:106-109` documents that `spawn_detached_runner_with_command` adds no env overlay, confirmed by reading the builder at `:169-200` (args/stdio/process_group only). With the anchor absent, `crates/cyrup-permission-system/src/ask.rs:295` reads an empty target and `forwarding.rs:406-408` takes the null-target `denied()` branch. Second gap: `is_subagent_child` (`extension.rs:1051-1053`) is a strict `== Some("1")` on one key.

**upstream** — `pi-permission-system/src/permission-forwarding.ts:9` defines `SUBAGENT_ENV_HINT_KEYS` = `PI_IS_SUBAGENT` / `PI_SUBAGENT_SESSION_ID` / `PI_AGENT_ROUTER_SUBAGENT`, ORed on any non-empty value at `pi-permission-system/src/index.ts:93-103`, with a subagent-sessions session-dir containment fallback at `src/index.ts:696-709`, feeding `canRequestPermissionConfirmation` at `src/index.ts:711-716`. The two are not exact analogs: cyrup's is a wiring-time role selector, pi's a per-ctx runtime predicate.

**Impact** — A background/detached subagent that hits an ask is fail-closed denied with no prompt ever shown to the operator, who sees an unexplained tool denial. Foreground delegation works; background silently does not.

**Fix** — Thread the parent session id explicitly into the detached path: give `spawn_detached_runner_with_command` (`crates/cyrup-ext-subagents/src/background/spawn_detached.rs:169-200`) an env-overlay parameter, populate it from the same resolution the foreground planner uses at `exec/mod.rs:1049-1056`, and replace `runner_main.rs:1741`'s `parent_session_id: None`. Delete the false comment at `:1736-1740`. Separately widen `is_subagent_child` (`crates/cyrup-permission-system/src/extension.rs:1051-1053`) to accept any non-empty value across the cyrup analogs of the three upstream keys.

**Verify** — Spawn a background subagent under an installed gate with an ask-triggering tool; assert a request artifact appears in the parent's forwarding spool carrying `target_session_id` == the root orchestrator's session id — the same on-disk assertion `crates/cyrup-permission-system/tests/forwarding_spawn_env.rs:303-343` already makes for the foreground path.

## PERM-005 — Parent forwarding watcher started only at SessionStart, never retried

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/extension.rs:847-864` `maybe_start_forwarding_watcher` has exactly one caller, the `SessionStart` arm at `extension.rs:1009`; `stop_forwarding_watcher` (`:866-871`) is called only from `SessionShutdown` (`:1041`). The guard at `:848` is `!self.install_watcher || !ctx.has_ui` and returns **without** stopping a live watcher. Config is taken by value into the task (`forwarding.rs:661` `config: ExtensionConfig`, cloned at `extension.rs:862`). The watcher body has two terminal pre-loop returns: `forwarding.rs:665-667` (session id unresolvable) and `:668-671` (forwarding-location error).

**upstream** — `pi-permission-system/src/index.ts` calls `startForwardedPermissionPolling` at `:1825` (via `refreshSessionRuntimeState`), `:1878` (`before_agent_start`), `:1935` (input) and `:1951` (tool_call), and stops it at `:1727` and `:1872`.

**Impact** — Four failure modes: a session whose id is not yet resolved at SessionStart never gets a watcher for its whole life; a UI that attaches later never arms one; a UI that detaches leaves a watcher running; and a mid-session `yoloMode` / `forwardedPromptTimeoutSeconds` change never reaches a running watcher. In each case forwarded child asks sit in the spool until they fail closed.

**Fix** — Call `maybe_start_forwarding_watcher` from the `before_agent_start`, input and tool_call paths as pi does, make it idempotent (no-op when a live handle exists), have the `extension.rs:848` guard call `stop_forwarding_watcher` before returning, and replace the by-value `ExtensionConfig` at `forwarding.rs:661` with a shared handle read per poll iteration. Convert the terminal returns at `forwarding.rs:665-671` into retry-on-next-tick.

**Verify** — Drive a session whose id resolves only after the first turn and assert a forwarded child request is picked up; toggle `yoloMode` mid-session and assert the running watcher auto-approves the next forwarded request.

## PERM-004 — `PermanentApprovalStore` read at highest last-match-wins priority on a file upstream deleted in v0.8.0

**Kind** stale-port · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/gate.rs:136-153` `apply_pattern_approval_state` still takes `permanent_rules` as its 4th param (`:141`) and passes it as the **last** ruleset to `evaluate::evaluate` (`:148-149`), so it wins under last-match-wins. Live sites: `extension.rs:83` (`PERMANENT_APPROVALS_FILE`), `:171` (field), `:306`, `:399`, `:480-482`, `:596-597`, `:817`, `:829`. Store body at `stores.rs:87-204`, including the unwired `save_rules` at `:175`.

**upstream** — `pi-permission-system/src/permanent-approval-store.ts` was deleted in v0.8.0; `pi-permission-system/src/index.ts:1795-1803` `shouldExposeTool` now calls `applyPatternApprovalState` with `sessionApprovals` only — there is no third argument left to pass.

**Impact** — A permanent approval recorded under the old model outranks a later config-level or session-level deny, so a rule the operator believes they revoked keeps allowing the tool.

**Fix** — Delete the `permanent_approvals` field and its call sites in `extension.rs`, drop the 4th param of `gate::apply_pattern_approval_state`, remove `stores.rs:87-204`, and remove the two tests that **pin the divergence**: `evaluate.rs:59-68` `permanent_beats_session_beats_config_last_match_wins` (it explicitly asserts `[config, session, permanent]` ordering) and the permanent-store tests at `stores.rs:224-236` / `:259-300`. Read `pi-permission-system/src/permission-prompts.ts` (added by `a33ac2c`) before executing — it is the v0.8.0 replacement shape.

**Verify** — Record a permanent approval, then add a config-level deny for the same pattern; assert the deny wins. A crate-wide grep for `permanent` should return nothing outside migration/cleanup code.

## PERM-006 — Skill-read and external-directory asks bypass the prompt-dedup cache

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/extension.rs:649-671` `prompt_decision` runs pre-check (`:651`) → yolo (`:654-659`) → human lock (`:660-665`) → channel selection (`:666-671`) with no `self.dedup` access. Only `resolve_ask` (`:679-717`) touches dedup — `get` at `:693`, `remember` at `:713`. Callers `resolve_skill_read` (`:508`) and `resolve_external_directory` (`:583`) never receive a `call_id`, so they cannot build a dedup key.

**upstream** — `pi-permission-system/src/index.ts` routes skill-read and external-directory prompts through the same dedup-keyed prompt path as tool asks (the prompt cache is keyed on the request, not the call channel).

**Impact** — Repeated reads of the same skill file or the same external directory re-prompt the operator every time within one session, where pi asks once.

**Fix** — Thread a synthetic key (`skill:<path>` / `extdir:<path>`) through `resolve_skill_read` (`extension.rs:508`) and `resolve_external_directory` (`:583`), and move the `dedup.get` / `remember` pair out of `resolve_ask` into `prompt_decision` (`:649`) so all three channels share it.

**Verify** — Trigger two identical skill reads in one session; assert exactly one prompt.

## PERM-007 — No `/permission-system` command, no settings modal, no config-write path

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** high

**cyrup** — `impl ExtensionConfig` (`crates/cyrup-permission-system/src/ext_config.rs:68-230`) has no writer beyond the private `ensure_on_disk` (`:149`). `init` (`crates/cyrup-permission-system/src/extension.rs:945-966`) calls only `api.subscribe(&[…])` and registers no command, even though `InitApi` exposes `register_command` (`crates/cyrup-ext/src/native.rs:256`).

**upstream** — `pi-permission-system/src/extension-config.ts:158` `readExistingConfig`, `:186` `mergeExtensionFields`, `:223` `resolveWriteTarget`, `:240` `savePermissionSystemConfig` — the last called from `pi-permission-system/src/index.ts:1404`; plus `src/config-modal.ts` and `src/zellij-modal.ts` for the UI.

**Impact** — Every configuration change requires hand-editing `config.json` outside the app, and (per PERM-024) takes effect only at the next session start.

**Fix** — Add a merge/save path to `ext_config.rs` mirroring `mergeExtensionFields` / `resolveWriteTarget`, register a `/permission-system` command in `extension.rs:945-966` via `InitApi::register_command`, and surface a settings view through the extension-UI command seam.

**Verify** — Toggle `yoloMode` from the command; assert `config.json` on disk changes and the running gate honours it.

## PERM-008 — Debug/review audit log entirely unported

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — Correction to the c8bd2ab evidence: the claim that `ExtensionConfig::debug` is never read is **false at HEAD** — `config.debug` is read at `crates/cyrup-permission-system/src/forwarding.rs:596-605`, gating a parent-side `notify("Subagent '<who>' is waiting for permission approval.")`, a faithful port of pi `index.ts:1432-1441`. What is genuinely absent is the audit log: the crate has no logging module (grep for `logs` / `LOGS_DIR` across `src/` returns nothing), and none of `decide` (`extension.rs:427`), `resolve_skill_read` (`:508`), `resolve_external_directory` (`:583`), `resolve_ask` (`:679`) or `apply_decision` (`:724`) has a logging side effect.

**upstream** — `pi-permission-system/src/logging.ts` (whole file); `LOGS_DIR_ENV_KEY` and `getPermissionSystemLogsDir` at `pi-permission-system/src/extension-config.ts:43` and `:56`; `writeDebugEntry` called 11 times in `src/index.ts` alone and 31 logging call sites across `src/`, including `logPermissionForwardingWarning` on malformed/unbound forwarding responses.

**Impact** — No post-hoc record of why a tool call was allowed or denied. The forwarding path also silently swallows malformed/unbound responses where pi logs them, making field diagnosis of a stuck subagent ask guesswork.

**Fix** — Add `crates/cyrup-permission-system/src/logging.rs` porting `src/logging.ts`, add a `CYRUP_PERMISSION_SYSTEM_LOGS_DIR` key alongside `CONFIG_PATH_ENV_KEY` (`ext_config.rs:24`), and call `write_debug_entry` at the five decision sites plus the forwarding warning paths in `forwarding.rs`.

**Verify** — With the logs dir set and `debug: true`, drive an allow, a deny and a malformed forwarding response; assert three entries land on disk.

## PERM-009 — `should_expose_tool` keeps `bash` advertised despite a tool-level deny

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** medium

**cyrup** — `crates/cyrup-permission-system/src/extension.rs:837-839`: `if tool_name == "bash" && mgr.get_bash_permissions(agent_name).any_allow() { return true; }`, with the justification doc at `:809-813`, fed by `manager.rs:388` `get_bash_permissions` and `types.rs:177` `any_allow`. Line-ref note: `:837-839` is correct — an earlier refresh "corrected" it to `:836-838`, which is off by one.

**upstream** — `pi-permission-system/src/index.ts:1791-1816` `shouldExposeTool` contains only the read/skills bypass at `:1811-1813` — there is no bash branch. cyrup's own doc comment at `extension.rs:813` still cites `index.ts:2049-2075`, a pre-v0.8.0 offset.

**Impact** — With a tool-level `bash` deny plus any narrow bash allow rule, the model still sees `bash` advertised and burns turns attempting it. Caveat: the branch cites a spec mandate and `spec/` is absent from this workspace, so it may be spec-directed rather than a defect — confirm before removing.

**Fix** — Delete `extension.rs:837-839` and its justification comment, and refresh the stale citation at `:813` to `index.ts:1791-1816`. No test pins the divergence: `should_expose_tool`'s only caller is `extension.rs:770`, and the one exposure test (`crates/cyrup-permission-system/tests/context_hygiene.rs:124-157`) denies `write`, not `bash`.

**Verify** — Deny `bash` at tool level while leaving a narrow `bash(git status)` allow; assert `bash` is absent from the exposed tool set.

## PERM-010 — `enabled` master switch (v0.8.0) not ported

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/ext_config.rs:29-38` — `ExtensionConfig` has exactly three fields; `normalize` (`:211-229`) and `default_config_content` (`:178-190`) read and write only those three.

**upstream** — `pi-permission-system/src/extension-config.ts:12` (`enabled?: boolean` on the interface), `:30` (`enabled: true` in `DEFAULT_EXTENSION_CONFIG`), `:65` (carried by `cloneDefaultConfig`), `:88` (`record.enabled !== false`), enforced by the early `if (!extensionConfig.enabled) { return; }` at `pi-permission-system/src/index.ts:1475-1477`.

**Impact** — No in-config way to disable the gate; an operator must delete or move policy files, or unset an env var they may not have set.

**Fix** — Add `enabled: bool` (default true) to `ExtensionConfig`, normalize as `!= Some(false)`, and early-return in `extension.rs`'s registration path. **Land PERM-019 first** — `ext_config.rs:281` is a golden byte assertion on the template, and updating that literal silently re-latches PERM-002 for every already-materialized agent dir.

**Verify** — Set `"enabled": false` with policy files still present; assert no gating events and no prompts.

## PERM-011 — No runtime yolo-mode API and no permission-request event channel

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/extension.rs:178` `config: Mutex<ExtensionConfig>` is private with no setter (`:173` is the start of its doc comment); read at `:650`, `:793`, `:862`, `:1013`; the only mutation is the assignment at `:324` inside `refresh_config_and_manager` (`:320-331`). On the seam side, `EventBus` exists at `crates/cyrup-ext/src/host/services.rs:976-1000` (subscribe `:976`, emit `:985`, `subscribers_for` `:997`) but the `HostServices` trait a `NativeExtension` receives has no `emit`, and `InitApi` (`crates/cyrup-ext/src/native.rs:244-281`) exposes only subscribe / register_tool / register_command / register_message_renderer / register_tool_renderer.

**upstream** — `pi-permission-system/src/index.ts:1481-1485` registers a runtime API with `getYoloMode` / `setYoloMode` / `toggleYoloMode`, and emits permission-request events other extensions consume.

**Impact** — No other extension or front-end can read or flip yolo mode, and nothing can observe permission requests programmatically. Blocks any external approval UI.

**Fix** — Expose `emit` on the `HostServices` trait in `crates/cyrup-ext/src/host/services.rs` (prerequisite, hence effort M), then add `get_yolo_mode` / `set_yolo_mode` / `toggle_yolo_mode` on `PermissionSystemExtension` and emit a `permission_request` event from `prompt_decision` (`extension.rs:649`).

**Verify** — From a second extension, toggle yolo mode and assert the next ask auto-approves; assert a `permission_request` event is observed for a gated call.

## PERM-012 — `registerModelOptionCompatibilityGuard` (temperature stripping) has no cyrup counterpart

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-provider/src/api/openai_responses.rs:345-347` inserts `temperature` unconditionally from `opts.temperature`, with no reasoning-model check.

**upstream** — `pi-permission-system/src/model-option-compatibility.ts:164` `registerModelOptionCompatibilityGuard`, lazily registered from `pi-permission-system/src/index.ts:1488-1497` and awaited at the top of `session_start`, `src/index.ts:1829`.

**Impact** — Requests to reasoning models that reject `temperature` fail at the provider with an API error, where pi silently strips the option.

**Fix** — Land the guard in `cyrup-provider`, not this crate: gate the `temperature` insert in `api/openai_responses.rs:345-347` on a model-capability check mirroring the incompatible-option table in `model-option-compatibility.ts`. pi solves a core-model problem from inside the extension; cyrup should solve it in core.

**Verify** — Issue a request against a reasoning model with a non-default temperature; assert the wire body omits `temperature` and the call succeeds.

## PERM-014 — Concurrent-duplicate ask collapse implemented in `dedup.rs` but never wired

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/dedup.rs` defines `Lookup` (`:104`), `Pending` (`:115`) with its async wait (`:125`), `PendingOwner` (`:146`) with `resolve` (`:156`) / `forget` (`:171`), `DedupCache::lookup` (`:198`) and `begin_pending` (`:250`). A grep for `lookup(` / `begin_pending(` across `src/` and `tests/` outside `dedup.rs` returns nothing; their only exercisers are `dedup.rs`'s own tests at `:339` and `:376`. The live path uses only `get` (`extension.rs:693`) and `remember` (`:713`).

**upstream** — `pi-permission-system/src/index.ts` collapses concurrent duplicate asks onto a single pending prompt rather than raising two.

**Impact** — Two same-turn calls with an identical dedup key can both raise a prompt, so the operator answers the same question twice. `8854601` (AGENT-002) narrowed the window by deferring parallel execution until the whole batch is prepared, but `before_tool_call` still runs sequentially through the same gate, so it stays reachable.

**Fix** — Replace the `get` / `remember` pair at `extension.rs:693` and `:713` with `DedupCache::lookup` + `begin_pending`, awaiting `Pending` on a hit and resolving through `PendingOwner` on the prompt result.

**Verify** — Fire two concurrent identical asks; assert one prompt and two identical decisions.

## PERM-018 — `is_installed` probes the raw config path and ignores `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH`

**Kind** cyrup-original · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/extension.rs:1103` calls `config_path_for(agent_dir)`, a bare `agent_dir.join(CONFIG_DIR).join(CONFIG_FILE)` at `extension.rs:298-301` with no env consultation. Every other consumer goes through `ExtensionConfig::load` (`ext_config.rs:81`) / `load_with_result` (`:92`) → `resolve_config_path` (`:69-77`), where `CONFIG_PATH_ENV_KEY` (`:24`) wins outright, and `ensure_on_disk` materializes on the **resolved** path (`:93-94`).

**upstream** — `pi-permission-system` has no install probe at all; config location is single-sourced through `pi-permission-system/src/extension-config.ts:223` `resolveWriteTarget`, so a probe/loader disagreement cannot arise.

**Impact** — With the override set, the default path is never created, the probe never fires, and the gate stays off — an operator who relocated their config gets no permission enforcement at all.

**Fix** — Make `config_path_for` (`extension.rs:298-301`) return `ExtensionConfig::resolve_config_path(...)` so probe and loader agree by construction; `derive_parts` (`:307`) and `refresh_config_and_manager` (`:323`) inherit the fix.

**Verify** — Point `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` at a non-pristine config outside the agent dir; assert `permission_extension_for_env` returns `Some`. Serialize the test on `crate::ext_config::env_lock()`.

## PERM-020 — Three env-mutating test sites still not serialized on `env_lock()`

**Kind** test-defect · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `f777e44` landed the `ext_config` half: `env_lock()` (`crates/cyrup-permission-system/src/ext_config.rs:232-241`) is taken at five sites — `:253`, `:268`, `:321`, `:348`, `:369` — plus `without_install_env` at `extension.rs:1345`. Three sites remain unlocked. (1) `not_installed_without_policy_or_env_returns_none` (`extension.rs:1144-1173`) does an unlocked save/remove/restore of `INSTALL_ENV_VAR` at `:1156` and `:1163-1164`, and its own comment at `:1150-1156` ("No other test in this crate reads or writes `INSTALL_ENV_VAR`") is now demonstrably false — `without_install_env` does so at `:1348-1354`. (2) `forwarding_channel_denies_when_no_parent_anchor` (`ask.rs:390-410`) does the same unlocked remove/restore of `PARENT_SESSION_ENV_VAR` at `:404` and `:407`. (3) Every `extension.rs` unit test constructing `PermissionSystemExtension` (e.g. `:1465`, `:1321`) reaches `resolve_config_path` (`ext_config.rs:70`) and reads `CONFIG_PATH_ENV_KEY` unlocked while `env_var_overrides_default_config_path` (`ext_config.rs:369-386`) has it set process-wide.

**upstream** — n/a (test hygiene; pi's suite runs one process per file under bun and has no shared-env hazard).

**Impact** — Flaky, order-dependent failures in the exact class `f777e44` was written to eliminate, now in the module it wasn't looking at. A test that intermittently fails gets re-run rather than believed. Not an instance: `tests/forwarding_persist.rs:97` / `:118` sets and removes `CYRUP_SUBAGENT_CHILD` unlocked, but that is a separate integration-test process and can only race its own siblings.

**Fix** — Take `crate::ext_config::env_lock()` at the top of the two named tests, and make the shared `PermissionSystemExtension` construction helper in `extension.rs`'s test module acquire it too. Delete the now-false comment at `extension.rs:1150-1156`.

**Verify** — Run the crate's unit tests repeatedly at high `--test-threads`; assert no order-dependent failures.

## PERM-022 — `spawn_env_alone_carries_a_child_ask_into_the_parent_spool` reads an artifact the child deletes on a timer

**Kind** test-defect · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/tests/forwarding_spawn_env.rs:314` spawns the child with an 8000 ms bound (threaded to `CYRUP_PERMISSION_FORWARDING_TIMEOUT_MS` at `:237`) while `:315` polls for up to 15 s at 20 ms (`await_spooled_request`, `:271-290`). At bound expiry the child deletes its own request file — `forwarding.rs:472-474` `let _ = std::fs::remove_file(&request_path);` followed by `cleanup_location_if_empty` — and every assertion at `:317-343` depends on that file. Every sibling uses 20000 (`forwarding_spawn_env.rs:371`; `tests/forwarding_subprocess.rs:236`, `:260`).

**upstream** — n/a (test hygiene).

**Impact** — Under load the poll outlives the child's bound and the test panics with a misleading "the child's ask never reached the parent's forwarding spool" — a scheduling outcome the test cannot control, reported as a parity failure. This is the crate's only instance of that shape. Re-checked and *not* an instance: `forwarded_timeout_fail_closes_the_child` (`tests/forwarding_subprocess.rs:281-289`) asserts `elapsed() >= 1000 ms` against a 1200 ms bound the child itself enforces, so load only makes it more true.

**Fix** — Raise the child bound at `forwarding_spawn_env.rs:314` from 8000 to 20000, matching its siblings, so the artifact outlives the poll window by construction.

**Verify** — Run the test under `--test-threads` saturation; assert it passes deterministically.

## PERM-023 — Install probe ignores agent-scoped `permission:` frontmatter the manager actually enforces

**Kind** cyrup-original · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `is_installed` (`crates/cyrup-permission-system/src/extension.rs:1095-1105`) recognises exactly four signals: `CYRUP_PERMISSION_SYSTEM` (`:1096`), `<agent_dir>/cyrup-permissions.jsonc` and `<cwd>/.cyrup/agent/cyrup-permissions.jsonc` (`:1099-1101`), and a non-pristine `<agent_dir>/cyrup-permission-system/config.json` (`:1103-1104`). The policy engine reads strictly more: `manager_paths_for` (`extension.rs:283-295`) wires `agents_dir: agent_dir.join("agents")` and `project_agents_dir: <cwd>/.cyrup/agent/agents`, and `load_agent_permissions_from` (`manager.rs:564-580`) reads `<agents_dir>/<agent_name>.md`, extracts YAML frontmatter via `common::extract_frontmatter` and enforces its `permission:` block through `normalize_raw_permission` (traversal guarded by `resolve_agent_markdown_path`, `manager.rs:585-601`). Nothing in `is_installed` looks at either agents dir.

**upstream** — `pi-permission-system` has no install probe: `pi-permission-system/src/index.ts` registers unconditionally on load and the only on/off is v0.8.0's `enabled` (`src/extension-config.ts:88` → `src/index.ts:1475-1477`). Agent-markdown frontmatter is a first-class policy layer upstream (`pi-permission-system/src/permission-manager.ts:591-604` `resolveAgentMarkdownPath`, cited by cyrup at `manager.rs:583`). cyrup's probe is therefore a cyrup-original whose signal set is strictly narrower than the engine it guards.

**Impact** — An operator whose only policy artifact is a persona's markdown frontmatter gets `is_installed == false`, no extension attached, and zero gating; their deny rules are silently inert. Same class as PERM-018 — probe and consumer disagree about which files count. Not a `6df5183` regression: before that commit `config.json` only existed after the extension had already run, so a frontmatter-only agent dir was equally un-probed. Low would be defensible, since an absent gate is loud ("no prompts at all") rather than a silent partial bypass; kept at medium because an artifact the operator authored is enforced-but-unreached. No test covers the case — `tests/layers_wired.rs:75-107`'s helper `ext_with_global` (`:45-52`) also writes `cyrup-permissions.jsonc`, so that scenario probes as installed.

**Fix** — Extend `extension.rs:1099-1101` to also return true when `<agent_dir>/agents/` or `<cwd>/.cyrup/agent/agents/` exists and is non-empty — neither directory is ever written by this crate, so it carries none of the self-footprint problem that produced PERM-002. Long-term: land PERM-010's `enabled` switch and shrink the probe to "any hand-authored artifact exists", which fixes PERM-018 and PERM-019 by construction.

**Verify** — With no policy file, no env var and no config, write `<agent_dir>/agents/coder.md` carrying a `permission:` frontmatter deny; assert `permission_extension_for_env(agent_dir, cwd)` is `Some` (today `None`). Serialize on `crate::ext_config::env_lock()`.

## PERM-024 — Extension config is not refreshed on `before_agent_start`, so a mid-session config edit never takes effect

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `on_before_agent_start` (`crates/cyrup-permission-system/src/extension.rs:760-806`) computes the exposed tool set, sanitizes the prompt, caches skill entries and syncs the status pill — and never calls `refresh_config_and_manager` (`:320-331`). That function has exactly two callers, both in `on_event`: `SessionStart` at `:1005` and `ResourcesDiscover` at `:1026`. So `self.config` (`yolo_mode`, `debug`, `forwarded_prompt_timeout_seconds`) is a construction-time snapshot updated only at those two events.

**upstream** — `pi-permission-system/src/index.ts:1875-1878` — the `before_agent_start` handler's second statement is `refreshExtensionConfig(ctx)`, immediately before `startForwardedPermissionPolling(ctx)`, in addition to the session_start path (`:1821`, inside `refreshSessionRuntimeState`) and the resources_discover reload branch (`:1848`). pi re-reads `config.json` at the top of every turn.

**Impact** — Editing `config.json` mid-session — the only way to change anything today, since PERM-007 means there is no in-app editor — has no effect until the next session start. It is also the only path that clears a stale PERM-003 config warning: `report_config_warning`'s clear-on-clean branch (`extension.rs:335-338`) is reachable only from `refresh_config_and_manager`. Low today; it is the missing half that makes PERM-007's modal work when it lands.

**Fix** — Call `self.refresh_config_and_manager(&ctx.cwd)` at the top of `on_before_agent_start` (`extension.rs:761`), matching pi's ordering. pi's `refreshExtensionConfig` refreshes only the config, not the manager; if a per-turn manager rebuild is judged too expensive, split `refresh_config_and_manager` and call only the config half.

**Verify** — Start a session with `yoloMode: false`, drive one `before_agent_start` plus an ask and assert a prompt is raised; rewrite `config.json` with `yoloMode: true`; drive a second `before_agent_start` and identical ask and assert it auto-approves. Today the second ask still prompts.

## Coverage

**Read at HEAD `1806375`**: `cyrup/crates/cyrup-permission-system/src/` — `extension.rs` (full event dispatch, install probe, decision paths, forwarding watcher), `ext_config.rs`, `forwarding.rs`, `ask.rs`, `dedup.rs`, `gate.rs` (`apply_pattern_approval_state`), `evaluate.rs`, `stores.rs`, `wildcard.rs`, `common.rs`, `manager.rs` (warning callbacks, agent-markdown loading, `get_bash_permissions`), `types.rs`, `sanitize/skills.rs` (module boundaries) — plus `tests/forwarding_spawn_env.rs`, `tests/forwarding_subprocess.rs`, `tests/forwarding_persist.rs`, `tests/context_hygiene.rs`, `tests/layers_wired.rs`. Cross-crate: `cyrup/crates/cyrup-ext-subagents/src/{exec/mod.rs, extension.rs, spawn/nested_events.rs, background/runner_main.rs, background/spawn_detached.rs}`; `cyrup/crates/cyrup-ext/src/{native.rs, facade.rs, host/services.rs}`; `cyrup/crates/cyrup-session-svc/src/{factory.rs, builder.rs}`; `cyrup/crates/cyrup/src/main.rs`; `cyrup/crates/cyrup-provider/src/api/openai_responses.rs`. Upstream: `pi-permission-system/src/{index.ts, extension-config.ts, permission-forwarding.ts, permission-manager.ts, wildcard-matcher.ts, common.ts, model-option-compatibility.ts}` at v0.8.0.

**Closure standard applied**: nothing accepted on a commit message. PERM-002 and PERM-003 were each re-proved from code, including a seam neither prior write-up checked — `cyrup/crates/cyrup/src/main.rs:433/:527/:601` → `cyrup-session-svc/src/factory.rs:78/:137/:163` → `builder.rs:687` `load_native_with_services` → `cyrup-ext/src/facade.rs:215-220` `set_host_services` **before** `init`, so `WarningSink`'s `OnceLock` is bound before any event fires and PERM-003's notifications genuinely reach production rather than only the test's `NotifyRecorder`. For PERM-002 the "is there another writer of `config.json`" search was re-run workspace-wide: the only write outside `ext_config.rs:158` is a test at `extension.rs:1415`. PERM-001's background gap was proved by construction (grep for every writer of `CYRUP_SUBAGENT_PARENT_SESSION`; read the detached `Command` builder) rather than by trusting `runner_main.rs`'s own comment, which is false.

**Defect-class hunt.** *Tests asserting a bug*: two live instances, both folded into PERM-004's fix rather than filed separately to avoid a duplicate — `evaluate.rs:59-68` pins the ruleset order upstream deleted, and `stores.rs:224-236` / `:259-300` pin the deleted store. A third instance, a now-false test comment, is inside PERM-020. *Tests asserting an uncontrollable timing outcome*: a crate-wide grep for `sleep` / `elapsed()` / `Instant::now` / `from_millis` / `from_secs` across `src/` and `tests/` yields exactly one — PERM-022.

**Negative results (checked, nothing filed)**: `tests/context_hygiene.rs:124-157` denies `write`, not `bash`, so it does not pin PERM-009 — that divergence is genuinely untested and safe to remove. `should_expose_tool` has exactly one caller (`extension.rs:770`). `init` (`extension.rs:945-966`) registers no tool or command, matching pi. `sanitize/skills.rs:300`'s `PermissionManager::new` is inside the `#[cfg(test)]` module opened at `:283`, so it does not contradict PERM-003's "only production builder" claim.

**Nothing was executed** (no cargo, no npm, per task rules). Every "denies / blocks / flakes / re-latches" claim is by code reading. PERM-001's background finding has never been run against a live background subagent under an installed gate.

**Blind spots** — not audited line-by-line, so a defect there would not have been found: `jsonc.rs`, `ordered.rs`, `status.rs`, `error.rs`, `gate.rs`'s `format_*` family, `manager.rs::check_permission`, `sanitize/tools.rs` past ~line 120. Upstream `config-modal.ts`, `zellij-modal.ts`, `model-option-compatibility.ts` internals and the new `permission-prompts.ts` were not read line by line; the last should be read before executing PERM-004.

**Taken on trust / uncheckable**: `spec/` is absent from this workspace, so the `R-*` / `§8.2` citations underpinning PERM-009's bash bypass and PERM-004's read-through permanent store cannot be checked — either could be spec-mandated; absence was treated as neither confirming nor refuting. PERM-003 is verified to the `HostServices::notify` boundary and through the wiring that binds it, but whether `NotifyKind::Warning` renders in each front-end is unaudited.

**Observed, not filed** (endemic; would duplicate what other areas find): this crate's upstream doc citations are systematically offset against v0.8.0 `index.ts` — e.g. `manager.rs:12` and `extension.rs:813` cite `index.ts:2049-2075` for `shouldExposeTool`, which now lives at `index.ts:1791-1816`.



---

## Surface-sweep findings (2026-08-03, HEAD `9219dcd`)

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at
all, rather than checking a list of known items. That inversion exists because the
item-driven method missed pi's stray-OSC-reply swallow (`pi/packages/tui/src/tui.ts:788-794`)
— a real, user-reported bug — and by construction cannot see behaviour nobody wrote an item
for. IDs use an `-SNN` suffix to mark their provenance.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| PERM-S01 | medium | not-ported | M | Id-based supervisor addressing is structurally dead: cyrup never registers under its own session id AND never propagates the orchestrator session id to children — `orchestrator_session_id`/`preferred_supervisor_target` are unreachable code |
| PERM-S02 | medium | not-ported | S | `dismissIncomingAsk`'s queue splice is unported — an inbound ask answered mid-run is re-injected after the flush |
| PERM-S03 | low | not-ported | S | `/intercom <target> <message>` writes no `intercom_sent` transcript entry — human-initiated sends are missing from the session audit trail |
| PERM-S04 | low | not-ported | S | `processForwardedPermissionRequests`' `preserveLocation` option is unported — the parent watcher tears down its own forwarding inbox on every 250 ms scan |

## PERM-S01 — Id-based supervisor addressing is structurally dead: cyrup never registers under its own session id AND never propagates the orchestrator session id to children — `orchestrator_session_id`/`preferred_supervisor_target` are unreachable code

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**upstream** — TWO cooperating writers, both at the ported baseline. (a) `pi-intercom` v0.7.0 `index.ts:835` registers with `nextClient.connect(buildRegistration(), currentSessionId)` — the broker session id IS the pi session id, so it is broker-resolvable by definition; `index.ts:612-614` `publishIntercomSessionId` then mirrors it into `process.env.PI_INTERCOM_SESSION_ID` (called at `:946` from `startSessionRuntime`, restored at `:1066`), which the child inherits because pi spawns with inherited env. (b) `pi-subagents` v0.34.0 `src/runs/shared/pi-args.ts:223-224`: `if (input.parentSessionId) env[SUBAGENT_ORCHESTRATOR_SESSION_ID_ENV] = input.parentSessionId;` — fed from `src/runs/foreground/execution.ts:218` `parentSessionId: options.parentSessionId`. The consumer is `pi-intercom/index.ts:86-88` (`SUBAGENT_ORCHESTRATOR_SESSION_ID` || `INTERCOM_SESSION_ID`) → `resolveSupervisorTarget` (`:886-893`), which tries the id FIRST and only falls back to the presence name.

**cyrup** — ABSENT. Writers: `grep -rn 'ORCHESTRATOR_SESSION_ID|INTERCOM_SESSION_ID' crates/ --include=*.rs` → 21 hits, every one a const decl, a read, a doc comment or a test; zero writers. Same grep with `--include=*` over the whole repo (excluding target/) → nothing outside .rs. `grep -rn 'set_var' crates/cyrup-intercom/src` → only doc comments noting `#![forbid(unsafe_code)]`. Registration id: `crates/cyrup-intercom/src/connect.rs:353-357` passes `env::var(ENV_INTERCOM_SESSION_ID).or_else(last_session_id())` — never `HostServices::session_id()`; `build_registration` (`connect.rs:407-424`) uses the host session id for the NAME only. On first connect that id is `None`, so `crates/cyrup-intercom/src/broker/mod.rs:287-288` assigns `uuid::Uuid::new_v4()`. Consumers are fully ported and unit-tested but unreachable: `identity.rs:69-70` (fallback), `identity.rs:112-116` `preferred_supervisor_target`, `tools/contact_supervisor.rs:43-52` `resolve_supervisor`, `identity.rs:193-203` (a test that pins the dead fallback).

**Impact** — cyrup's broker session id is a random UUID unrelated to the cyrup session id, and no child ever receives an orchestrator session id, so `resolve_supervisor` (`contact_supervisor.rs:43-52`) always skips its first branch and resolves the supervisor by presence NAME only. `presence_name` (`identity.rs:89-99`) returns the raw trimmed session title when one is set, so two supervisors sharing a title are genuinely ambiguous: `broker/routing.rs:26-33` returns >1 id and `broker/mod.rs` answers `delivery_failed`, leaving every child's `contact_supervisor` (`need_decision`, `interview_request`, `progress_update`) permanently undeliverable. A supervisor that renames mid-run strands its already-spawned children the same way. Fixing this needs BOTH halves — writing `CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID` alone would resolve against a broker that has never heard of that id.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## PERM-S02 — `dismissIncomingAsk`'s queue splice is unported — an inbound ask answered mid-run is re-injected after the flush

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**upstream** — `pi-intercom` v0.7.0 `index.ts:455-459`: `dismissIncomingAsk(messageId)` has TWO effects — `replyTracker.dismissPendingAsk(messageId)` AND `pendingIdleMessages.splice(findIndex(e => e.message.id === messageId), 1)`. Four call sites: `:755` (non-interactive busy auto-reply), `:1568` (`send` with `replyTo`), `:1711` (`reply`, on "Session not found") and `:1718` (`reply`, on success). The queue it prunes is filled by `queueIdleMessage` (`:711-714`) from the busy-with-UI branch at `:763` and drained wholesale by `flushIdleMessages` (`:685-710`).

**cyrup** — ABSENT. `grep -n 'pending_inbound|dismiss' crates/cyrup-intercom/src/{session_state,inbound,reply_tracker,seams}.rs crates/cyrup-intercom/src/tools/intercom.rs` → `SharedIntercomState` exposes exactly three queue operations (`session_state.rs:125` `push_pending_inbound`, `:132` `take_pending_inbound` (a `mem::take`), `:139` `pending_inbound_len`) and no per-entry removal API at all, so no caller could prune it. `reply_tracker.rs:147-159` `mark_replied`/`dismiss_pending_ask` touch only `pending_asks`/`pending_turn_contexts`/`current_turn_context`. All three answer paths call the tracker and stop there: `tools/intercom.rs:144-150` (`send` + `reply_to` → `mark_replied`), `:224-233` (`reply` → `mark_replied`/`dismiss_pending_ask`), `inbound.rs:284-290` (auto-reply → `mark_replied`).

**Impact** — Reachable on the normal path. A peer's ask arrives while a run is in flight: `inbound.rs:322-341` records it in the tracker, surfaces it, then `decide_inbound_policy` returns `Queue` (busy + has_ui) and `queue_idle_message` parks it (`inbound.rs:143-146`). Still inside that same run the model calls `intercom({action:"pending"})` — which reads `tracker.pending_asks`, populated at receipt — sees the ask and answers it with `intercom({action:"reply"})`. The tracker is cleared but the entry is still in `pending_inbound`. On `AgentEnd`/`TurnEnd` (`extension.rs:333`,`:361`) → `schedule_inbound_flush(state, 0)` → `flush_idle_messages` (`inbound.rs:179-183`) drains it and `trigger_turn_over_inbound` injects the already-answered message with `trigger`, driving a fresh turn and re-queueing a turn context. The model sees the question it just answered with no staleness marker, answers again, and the peer receives a second unsolicited reply. Same shape via the `send`-with-`replyTo` path and the subagent-clarify seam (`seams.rs:249`).

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## PERM-S03 — `/intercom <target> <message>` writes no `intercom_sent` transcript entry — human-initiated sends are missing from the session audit trail

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi-intercom` v0.7.0 `index.ts:1877-1885`: after the ComposeOverlay reports `result.sent`, `pi.appendEntry("intercom_sent", { to, message: { text: result.text }, messageId: result.messageId, timestamp: Date.now() })` — the same custom entry type the tool paths write at `:1561`, `:1651` and `:1719`. pi audits an overlay send identically to a tool send.

**cyrup** — ABSENT. `grep -n 'append_entry' crates/cyrup-intercom/src/extension.rs crates/cyrup-intercom/src/ui/compose.rs` → two hits in `extension.rs`, both inside `//!`/`///` doc text; zero in `compose.rs`. Read the whole path: `extension.rs:210-214` resolves the target, calls `compose_send(client, &target_id, &message)` and returns `format!("Message sent to {target}.")`; `ui/compose.rs:203-220` `compose_send` does `client.send(...)`, checks `delivered`, returns the `SendResult` — neither touches `HostServices::append_entry`. The tool arms in the same crate DO (`tools/intercom.rs:129-144`, `:167-190`, `:230-241`), so the omission is specific to the command path.

**Impact** — Every message a human sends via `/intercom` is invisible in the transcript: the model has no record it went out (so it may duplicate it), a resumed or compacted session loses it, and a transcript-based review of "what did this session send its peers" is silently incomplete while looking complete — because tool sends in the same session ARE recorded, the absence reads as "no human sends happened" rather than "human sends are not logged".

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## PERM-S04 — `processForwardedPermissionRequests`' `preserveLocation` option is unported — the parent watcher tears down its own forwarding inbox on every 250 ms scan

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi-permission-system` v0.7.1 `index.ts:1357-1359` `processForwardedPermissionRequests(ctx, options: { preserveLocation?: boolean } = {})`; `:1501-1503` `if (!options.preserveLocation) cleanupPermissionForwardingLocationIfEmpty(location)`. The only production caller is the watcher scan at `:1935`: `void processForwardedPermissionRequests(currentContext, { preserveLocation: true })`. So the parent's `requests/` + `responses/` + session-root dirs are created once by `startForwardedPermissionPolling` (`:1988-1990` `ensurePermissionForwardingLocation`) and never removed by the polling loop for the life of the session. The child-side paths (`:1338`, `:1353`) deliberately keep the unguarded cleanup.

**cyrup** — ABSENT. `grep -rn 'preserve_location|preserveLocation' crates/cyrup-permission-system/src/` → zero. `forwarding.rs:501-506` `process_forwarded_requests(default_agent_dir, session_id, services, config)` has no options parameter, and `:562` calls `cleanup_location_if_empty(&location)` unconditionally on every invocation; `:337-346` `remove_dir`s all three of `requests_dir`, `responses_dir`, `session_root` when empty. The watcher `spawn_forwarding_watcher` (`forwarding.rs:658-695`) calls `ensure_location` exactly once at `:673`, then `process_forwarded_requests` on the startup scan (`:675`) and again on every wake (`:692`), tick = `CONTROL_INBOX_POLL_INTERVAL` (250 ms, `cyrup-ext-subagents/src/background/control.rs`).

**Impact** — The realistic consequence is a narrow lost-request race, not a persistent break. Within 250 ms of session start the parent removes its own (empty) spool dirs; a child's `wait_for_forwarded_approval` (`forwarding.rs:410-431`) then does `ensure_location` → `write_json_atomic`, whose temp file is `create_new` (`:288-289`) inside `requests_dir`. If a parent scan lands between the child's `create_dir_all` and its `create_new`, the write fails and `:429-431` returns `denied()` — a fail-closed permission denial with no prompt shown and no log line (there is no logging module, PERM-008). The window is microseconds wide and recurs after every completed request, so it is real but rare. I am NOT carrying the claim's second consequence: `notify::PollWatcher` re-walks its root each interval and recovers when the directory reappears, and the fallback ticker has the identical 250 ms period, so a "dead watcher" changes nothing observable. Distinct from PERM-001 (that is the missing parent anchor); this reaches a similar silent-deny by a different route, and PERM-001's fix does not close it.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

