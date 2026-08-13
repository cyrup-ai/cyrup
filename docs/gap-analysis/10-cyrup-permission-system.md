# 10 — cyrup-permission-system

Covers `cyrup/crates/cyrup-permission-system/` — the allow/ask/deny gate, its policy manager and evaluator, the wildcard matcher, the prompt/dedup layer, the parent↔child ask-forwarding spool, the prompt sanitizers, the audit log and the install probe — measured against `pi-permission-system/` at upstream HEAD `9affcc9` = **v0.8.0**, which is now also the latest upstream tag.

**Headline: this area carries the workspace's only open `critical`.** `PERM-009` is a permission bypass — a configured `tools.bash: deny` is defeated and the allow-listed command actually executes — and `PERM-023` is a second fail-open, where the install probe declines to attach the gate at all for an operator whose only policy artifact is agent-markdown frontmatter. Both were mis-rated on the 2026-08-12 audit pass and are corrected below. **Do not read this area as tail-cleanup**: the two items at the top of the table are the reason the crate exists, and everything under them is genuinely tail. The crate has **caught up completely on upstream drift** — every behavioural change in `v0.7.1..v0.8.0` is ported — and the two remaining highs from the last pass (the background-subagent forwarding anchor, and the once-per-session forwarding watcher) are genuinely closed. Under the two fail-opens sits a long tail: a gate that is text-only where pi is modal, a forwarding path that writes no audit entries, a dedup cache whose concurrency half is implemented but unwired, and a cluster of string-unit and diagnostic mismatches.

> **Re-audited 2026-08-12, cyrup HEAD `04c1ba2` (branch `david/cyrup`, last code commit; `a9000b1` is docs-only), against `pi-permission-system` v0.8.0 (`9affcc9`), `pi-intercom` v0.10.1 (`30dcbdd`) and `pi-subagents` v0.47.1 (`9e9fd13`).**
>
> **Closed this pass: 10** — `PERM-004`, `PERM-006`, `PERM-010`, `PERM-015`, `PERM-016`, `PERM-018`, `PERM-S01`, `PERM-S02`, `PERM-S03`, `PERM-S04`. Four prior closures (`PERM-001`, `PERM-002`, `PERM-003`, `PERM-005`) were re-attacked at HEAD and **all four held**; in particular the `PERM-010` landing was checked for re-latching `PERM-002`'s install probe and does not (`ext_config.rs:286-287` freezes the legacy 3-key template).
> **Reopened: 0.** No prior closure was overturned.
> **Newly filed: 7** — `PERM-025` … `PERM-031`, all `low`, five from the surface-driven sweep and two from the refuter's own sweep of code the audit did not walk.
> **Re-rated: 1** — `PERM-022` medium → **low** (its failure mode is a false-FAIL, not a false-PASS, so it hides no defect).
>
> ---
>
> **REPAIR PASS, same day (2026-08-12), applying the completeness critique.** Two severities were
> re-derived against `README.md:106-107`'s own definition (`critical` = data loss, silent wrong
> output, **a permission bypass**, or a crash on a normal path) rather than against the shape of the
> surrounding tail. Nothing else in this file changed; no ID was renumbered, merged or deleted, and
> no item was reclassified as a tracker — every open item here proposes work.
> - **`PERM-009` medium → `critical`.** Both sides re-read at **both** upstream tags this pass:
>   `shouldExposeTool` has no bash branch at v0.7.1 (`index.ts:2049-2075`, the ported baseline) **or**
>   at v0.8.0 (`index.ts:1790-1816`), so cyrup's extra branch is an in-baseline parity bug rather than
>   drift — and `manager.rs:205-215` resolves a bash *command* rule above the tool-level state, so the
>   still-exposed tool's allow-listed command actually executes. A configured deny is defeated. It is
>   now the first row of the open table.
> - **`PERM-023` medium → `high`.** Same fail-open class, narrower trigger: `is_installed`
>   (`extension.rs:2159-2175`) never looks at `agents_dir`, which `manager_paths_for` (`:390-401`)
>   wires and `manager.rs:500-503` enforces, so an operator whose only artifact is
>   `<agent_dir>/agents/<name>.md` frontmatter gets no extension attached and their `permission:`
>   deny rules are silently inert. Held at `high` rather than `critical` because the deployment shape
>   is narrow and the host's own approval flow still runs — a policy layer is not applied, rather than
>   a process running unguarded. Reasoning recorded in `## Coverage` → *Severity re-derivation*.
>
> **Open set after the repair pass: 21 items — 1 critical, 1 high, 6 medium, 13 low.**
>
> **Version-lag result: ZERO `upstream-drift` items.** `git diff v0.7.1..v0.8.0` was read per shipping file. Of the 28 changed files, 11 are non-shipping (package/lock/CHANGELOG/README + 7 tests); `permission-manager.ts`, `jsonc-config.ts`, `permission-dialog.ts`, `system-prompt-sanitizer.ts`, `permission-forwarding.ts` and `before-agent-start-cache.ts` are extract/export refactors with no behavioural delta. The five genuinely behavioural v0.8.0 changes are **all ported**: `PermanentApprovalStore` removal (PERM-004), the 500-char wildcard cap (PERM-015), review-stream un-gating (`logging.rs:168-170`), the `enabled` master switch (PERM-010), and merge-preserving config save + prototype-pollution key skip (PERM-007 half / PERM-016). This corroborates `PARITY-GAPS` §3d independently. **The `pi-intercom` v0.9.2..v0.10.1 and `pi-subagents` v0.43.0..v0.47.1 deltas were NOT swept here** — the four `-S` items in this file that touch those repos are now closed, and areas 09/11 own the new territory.

## Status since the 2026-08-03 re-baseline

| ID | Status | Note |
|---|---|---|
| PERM-001 | **closed** | `513e45a`. Hop-2 anchor now real: `parent_anchor.rs` publish/clear/resolve → `detached_runner_env_overlay` → `spawn_detached.rs:178/:227` → `runner_main.rs:2502`. Second half also closed — `is_subagent_child` is any-non-empty across three keys (`extension.rs:2093-2112`), matching `index.ts:94/:100`. |
| PERM-002 | **closed** | `6df5183`. Re-attacked specifically for a PERM-010 re-latch: `is_pristine_default_file` (`ext_config.rs:312-316`) now compares against **two** templates, and `enabled` is excluded from `EXTENSION_CONFIG_KEYS` (`ext_config.rs:118` = upstream `extension-config.ts:144-148`) so a save cannot inject it. Test `extension.rs:2617`. |
| PERM-003 | **closed** | `6df5183`. Re-proved by grep: the only non-`cfg(test)` `PermissionManager::new` is `extension.rs:224` inside `manager_with_warnings`; all four constructors funnel through `from_parts_full` (`:877-917`). |
| PERM-004 | **closed** | Upstream deletion verified from the diffstat (`src/permanent-approval-store.ts | 93 --`). `stores.rs` is now 135 lines holding only `SessionApprovalStore`; `gate.rs:167-183` evaluates two rulesets. Regression test `tests/permanent_approvals_file_is_inert.rs`. |
| PERM-005 | **closed** | `513e45a`. Four callers (`extension.rs:1964/:1977/:1983/:2019`) matching pi's four hooks; idempotent; the disqualifying branch tears down; config is a `SharedExtensionConfig` re-snapshotted every poll; terminal pre-loop returns are now a retry loop. |
| PERM-006 | **closed** | Dedup moved inside `prompt_decision` (`extension.rs:1384-1406` / `:1469`), reached by all three ask surfaces (`:1176`, `:1275`, `:1497`), mirroring `index.ts:1580-1631`. **Note the original item misdescribed upstream**: pi's key is `requestId \0 sha256(fingerprint)` where `requestId` is the toolCallId, so pi re-prompts on a new tool call too — it never "asks once" per skill file. The structural gap is nonetheless genuinely closed. |
| PERM-007 | partially closed | Command + `has_ui` guard + full config-write path landed. The **modal** half is open, and the in-tree rationale at `extension.rs:701-708` is stale. Rewritten below. |
| PERM-008 | partially closed | `logging.rs` (431 lines) landed with ten call sites in `extension.rs`, including v0.8.0's un-gated `review` stream. `forwarding.rs` has **zero** logging across all 1125 lines. Rewritten below. |
| PERM-009 | still open — **raised to `critical`** | Untouched in code. Strengthened this pass: the divergence is not merely cosmetic exposure — the allow-listed bash command actually executes. Re-rated in the repair pass after re-reading `shouldExposeTool` at **both** v0.7.1 and v0.8.0: neither has a bash branch, so this is an in-baseline permission bypass. |
| PERM-010 | **closed** | `enabled: bool` at `ext_config.rs:51/:80`, parsed as the inequality `!= Some(&Value::Bool(false))` (`:324`), enforced by the early return at `extension.rs:2220-2223`. Tests `ext_config.rs:833-848`, `extension.rs:2561`. |
| PERM-011 | partially closed | The three runtime-API methods are ported (`extension.rs:608/:628/:693`). No publish seam and no permission-request event channel. Rewritten below. |
| PERM-012 | still open | Untouched. Fix lands in `cyrup-provider`. Confirmed a `supports_temperature` compat flag exists (`api/compat.rs:133`) but is consulted only by `anthropic_messages.rs`. |
| PERM-013 | still open | Untouched. |
| PERM-014 | still open | Untouched. The unwired half is now measured, and `dedup.rs`'s own module doc asserts wiring that does not exist. |
| PERM-015 | **closed** | `wildcard.rs:24` 500-unit cap, short-circuiting to `regex: None` before escaping, measured in UTF-16 units to match JS `String.length`. One documented stricter delta (`None` never matches; pi's `/$^/` matches the empty string). |
| PERM-016 | **closed** | `common.rs:173` `is_prototype_pollution_key`, consumed at `common.rs:204` and `ext_config.rs:436`. Test `manager.rs:1181`. |
| PERM-017 | still open (revisit-trigger only) | Unchanged; still a documented deliberate omission. Scope clarified: this item covers the **forwarding** root only. The same env key's policy-root use is now `PERM-025`. |
| PERM-018 | **closed** | `resolved_config_path_for` (`extension.rs:427-429`) is used by `is_installed` (`:2173`) and `config_path_line` (`:848-850`); the raw `config_path_for` carries a doc forbidding external use. Test `extension.rs:2706`. |
| PERM-019 | still open | The PERM-010 landing did **not** re-latch it (second frozen template added), so PERM-010's warned hazard is discharged. The underlying cyrup-original fragility remains. |
| PERM-020 | partially closed | Two of three named sites fixed. Site (2) `ask.rs:390-415` remains, and a **fourth** site was found at `tests/prompt_dedup.rs:73-81`. Rewritten below. |
| PERM-021 | still open | Untouched. Confirmed this pass that the **behaviour** is correct and matches upstream; only the assertion is too weak. |
| PERM-022 | still open, **re-rated low** | Facts upheld, severity corrected — `await_spooled_request` returns on the first poll that finds the artifact, and the failure mode is an explicit `panic!`, so the test can only false-FAIL. |
| PERM-023 | still open — **raised to `high`** | Untouched in code. Re-rated in the repair pass: the probe's blind spot is a fail-open on a policy layer the manager enforces (`manager.rs:500-503` over `manager_paths_for`'s `agents_dir`, `extension.rs:394`), i.e. the same class as PERM-009 with a narrower trigger. |
| PERM-024 | still open | Untouched. |
| PERM-025 | **new** | `PI_PERMISSION_SYSTEM_POLICY_AGENT_DIR` has no cyrup analog — zero hits workspace-wide. |
| PERM-026 | **new** | A `resources_discover` reload updates config but never re-syncs the yolo status pill. |
| PERM-027 | **new** | `lifecycle.reload` debug entries unported. |
| PERM-028 | **new** | `sensitive_log_metadata` counts UTF-8 bytes where pi counts UTF-16 units; `permission_decision_scope` does not trim. |
| PERM-029 | **new** | The shipped JSON Schema and starter policy example are not ported. |
| PERM-030 | **new** | Ask-dialog formatters count Unicode scalars where pi counts UTF-16 units. |
| PERM-031 | **new** | The forwarding watcher drops upstream's per-scan `hasUI` guard. |
| PERM-S01 | **closed** | Both halves landed: `exec/mod.rs:1794-1803` writes `CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID`; `connect.rs:377-383` registers under `HostServices::session_id()`. Ownership moves to **area 11**. |
| PERM-S02 | **closed** | `session_state.rs:147` `remove_pending_inbound` + `inbound.rs:169-171` `dismiss_incoming_ask` with three production callers. Ownership moves to **area 11**. |
| PERM-S03 | **closed** | `crates/cyrup-intercom/src/extension.rs:437-445` appends `intercom_sent` on the `/intercom` command path. Ownership moves to **area 11**. |
| PERM-S04 | **closed** | `ProcessForwardedOptions { preserve_location }` (`forwarding.rs:498-518`), passed on both the startup scan and every wake. Tests `tests/forwarding_preserve_location.rs`. |

Closed this pass: **10**. Reopened: **0**. Newly filed: **7**.

## Open items

> **⚠ THIS IS NOW THE COMPLETE OPEN SET FOR THIS AREA.** The four `-S` surface-sweep items that
> previously lived in a second table are all closed, so the split table that caused `SEAM-S01` to
> escape a full pass on 2026-08-07 no longer exists here. Their closure evidence is in the status
> table above; `PERM-S01`/`S02`/`S03` are intercom/subagents concerns and **area 11 owns them going
> forward**. Per structural defect A in `00-residual-ledger.md`, treat this count as a **floor**.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| PERM-009 | **critical** | parity-bug | S | `should_expose_tool` keeps `bash` advertised despite a tool-level deny — **and the allow-listed command executes** — **observed 2026-08-13, reproduced in the shipped binary** |
| PERM-032 | low | *unclassified — lead* | M | A permission-**denied** tool result breaks the next provider request on `together/openai/gpt-oss-20b` (3/3), while two other models handle it fine — **new, observed 2026-08-13, low confidence** |
| PERM-023 | high | cyrup-original | S | Install probe ignores agent-scoped `permission:` frontmatter the manager enforces — the gate never attaches |
| PERM-007 | medium | not-ported | M | `/permission-system` renders text where upstream opens a live settings overlay |
| PERM-008 | medium | not-ported | M | The forwarding path writes no audit entries — 8 review + 3 debug sites unported |
| PERM-011 | medium | not-ported | M | Yolo runtime API has no publish seam; permission-request event channel absent |
| PERM-012 | medium | not-ported | M | `registerModelOptionCompatibilityGuard` (temperature stripping) has no cyrup counterpart |
| PERM-014 | medium | not-ported | M | Concurrent-duplicate ask collapse implemented in `dedup.rs` but never wired |
| PERM-020 | medium | test-defect | S | Two env-mutating test sites still not serialized on a lock |
| PERM-013 | low | not-ported | M | `before_agent_start` result caching not ported |
| PERM-017 | low | not-ported | S | Forwarding-root agent-dir env overrides not ported (documented as deliberate) |
| PERM-019 | low | cyrup-original | S | Install state inferred from a byte-exact template comparison |
| PERM-021 | low | test-defect | S | PERM-003 test's re-arm assertion covers only the policy warning |
| PERM-022 | low | test-defect | S | Spool test's child bound is shorter than the poll window |
| PERM-024 | low | not-ported | S | Extension config not refreshed on `before_agent_start` |
| PERM-025 | low | not-ported | S | Policy-root env override `PI_PERMISSION_SYSTEM_POLICY_AGENT_DIR` has no cyrup analog |
| PERM-026 | low | parity-bug | S | A `resources_discover` reload never re-syncs the yolo status pill |
| PERM-027 | low | not-ported | S | `lifecycle.reload` debug entries never written |
| PERM-028 | low | parity-bug | S | `sensitive_log_metadata` length unit and untrimmed `decisionScope` |
| PERM-029 | low | not-ported | S | Shipped permissions JSON Schema and starter policy example not ported |
| PERM-030 | low | parity-bug | S | Ask-dialog formatters count Unicode scalars where pi counts UTF-16 units |
| PERM-031 | low | parity-bug | S | Forwarding watcher drops upstream's per-scan `hasUI` guard |

## PERM-009 — `should_expose_tool` keeps `bash` advertised despite a tool-level deny, and the allow-listed command then executes

**Kind** parity-bug · **Severity** **critical** *(raised from medium in the repair pass)* · **Effort** S · **Confidence** **confirmed — reproduced in the shipped binary** · **observed 2026-08-13** (headless-binary; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Reproduced 2026-08-13. This is a live, end-to-end permission bypass in the shipped binary, not a
> code-shape concern.** Scratch `HOME`, a fresh git repo, and a canary file
> (`PERM009_CANARY_9f3a.txt`) created seconds before the run so no model could hallucinate it.
>
> * **Control** — `{"tools": {"bash": "deny"}}` alone: the bash tool is not advertised, the model
>   says outright "We cannot run shell commands", and **nothing executes**.
> * **Bypass** — the same file plus a single narrower rule, `"bash": {"git status": "allow"}`: the
>   model is handed the bash tool and runs the command **for real**. The returned output contains
>   `PERM009_CANARY_9f3a.txt` and `No commits yet` — genuine `git(1)` output from that specific repo.
> * **Scope** — a command *not* on the allow list (`whoami`) is still refused, so the bypass grants
>   exactly the allow-listed command: precisely the mechanism predicted below.
>
> Model: Together `openai/gpt-oss-20b`. The item's own Verify ("assert `bash` is absent from the
> exposed tool set **and** that a `git status` call is not executed") **fails on both halves today**.
> No factual correction was needed anywhere in the item; the Impact's "`git status` **executes**" is
> now a first-hand observation rather than a prediction. `critical` is correct and, if anything,
> understated.

> **Why critical.** `README.md:106-107` defines `critical` as data loss, silent wrong output, **a
> permission bypass**, or a crash on a normal path. This is a permission bypass on a configured
> policy: an operator writes `tools.bash: deny`, cyrup shows the model a `bash` tool anyway, and the
> command runs. It was previously rated `medium` on the shape of the code (a three-line predicate)
> rather than on the consequence. It is the top item in this area and the only open `critical` in the
> workspace.

**cyrup** — `crates/cyrup-permission-system/src/extension.rs:1651-1653`, verified verbatim at HEAD: `if tool_name == "bash" && mgr.get_bash_permissions(agent_name).any_allow() { return true; }`, sitting immediately after the ported read/skills bypass at `:1648-1650`, with its justification comment at `:1624-1631` calling it "the mandate-directed analog of the read/skills bypass".

**upstream** — read in full at **both** tags this pass, because the classification turns on it:
- **v0.7.1 (the ported baseline)** — `pi-permission-system/src/index.ts:2049-2075` `shouldExposeTool`: tool-level `applyPatternApprovalState(..., sessionApprovals, permanentApprovals).state !== "deny"` → true, then **only** the read/skills bypass, then `return false`.
- **v0.8.0 (latest)** — `index.ts:1790-1816`, byte-identical but for the dropped `permanentApprovals` argument (PERM-004's upstream deletion).

Neither tag has a bash branch. So cyrup's branch is an **in-baseline parity bug, not drift** — it was never in either upstream — and cyrup's in-tree citation at `extension.rs:1624` (`index.ts:2049-2075`) is the correct *v0.7.1* offset that simply needs re-resolving to `:1790-1816` at v0.8.0 (see the citation hazard in Coverage), not a wrong file.

**Impact** — Both `check_permission` implementations resolve a bash *command* rule **above** the tool-level state (upstream `permission-manager.ts:944-953`; cyrup `manager.rs:205-215`, whose own comment says "command rules OUTRANK the tool-level `bash` fallback"). So under `tools.bash: deny` plus `bash: {"git status": allow}`: pi hides `bash` from the model entirely and nothing runs; cyrup exposes `bash`, the model calls it, `find_compiled_match` returns the `allow` from the command rule, and `git status` **executes**. The operator's tool-level deny is defeated by a narrower allow they wrote for a different purpose, with nothing in the transcript saying the deny was overridden. This is a divergence in what the model can *do*, not merely in what it is shown, and it is silent in both directions — the tool list looks normal and the execution looks approved. Caveat retained and unresolved: the branch cites a spec mandate and `spec/` is absent from this workspace, so the requirement of record cannot be read here (see *Taken on trust* in Coverage) — that is a reason to raise the question with a human, not a reason to hold the severity down, since the README's rules make an unverifiable in-source claim not a decision of record.

**Fix** — Delete `extension.rs:1651-1653` and its justification comment at `:1624-1631`, and refresh the citation at `:1624` to `index.ts:1790-1816` @v0.8.0. Nothing pins the divergence: `grep -rn 'any_allow\|get_bash_permissions' src/ tests/` shows the only exposure test (`tests/context_hygiene.rs:128`) denies `write` and asserts `bash` stays exposed as a **default-ask** tool (`:152`), not as a denied-with-allow-rule tool — so the test suite goes green on the deletion. If the absent spec mandate is later produced and genuinely requires the branch, the correct shape is still not this one: pi's read/skills bypass is paired with a `tool_call` handler that *restricts* the exposed tool to skill paths only, so any bash analog must likewise re-gate execution to the allow-listed commands rather than deferring to `check_permission`'s command-first precedence.

**Verify** — Deny `bash` at tool level while leaving a narrow `bash("git status")` allow; assert `bash` is absent from the exposed tool set **and** that a `git status` call is not executed. Add the mirror case (`tools.bash: deny` with no command rules) to prove the deletion did not break the ordinary deny path, and keep `tests/context_hygiene.rs:128-152` green unchanged.

## PERM-007 — `/permission-system` renders text where upstream opens a live settings overlay

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — The write path is now complete and is *not* the gap: the command constant is `extension.rs:96`, registration is `extension.rs:1909-1916` (`api.register_command(PERMISSION_SYSTEM_COMMAND, …)`), servicing is `:1931-1955` with pi's `has_ui` guard (`!ctx.has_ui` → notify `PERMISSION_SYSTEM_COMMAND_REQUIRES_UI` and return), the handler body is `:737-813`, and both toggle rows go through `save_extension_config` (`:574-604`) → `ExtensionConfig::save` (`ext_config.rs:408-488`, merge-in-place, corrupt-file refusal, symlink write-through). What is missing is the surface: the command returns a formatted `String` from `render_settings` (`extension.rs:834-843`). Its in-tree rationale at `:701-708` — "HostServices exposes no custom-overlay seam" — is **stale**: `fn open_overlay` exists at `crates/cyrup-ext/src/host/services.rs:224`, has a live impl at `crates/cyrup-session-svc/src/host_services.rs:676`, and a production caller at `crates/cyrup-ext-subagents/src/extension.rs:9908`.

**upstream** — `pi-permission-system/src/config-modal.ts:63-122` `openPermissionSystemSettingsModal` is a real `await ctx.ui.custom<void>(…, { overlay: true, overlayOptions })` building the ZellijSettingsModal/ZellijModal pair; it is registered at `src/index.ts:1500-1511`, which also supplies `getConfigPath: getPermissionSystemConfigPath` (`:1509`). The `hasUI` guard cyrup ports lives at `src/common.ts:188-198` `createPermissionSystemCommandHandler`.

**Impact** — The operator gets a read-only dump plus two blind toggles instead of pi's live editor: no per-field navigation, no in-place validation, no visible current-vs-pending state, and no discovery of the settings that exist. Combined with PERM-029 (no schema, no example) the practical way to configure anything non-trivial remains hand-editing `config.json` outside the app.

**Fix** — Replace the `String` return of `render_settings` (`extension.rs:834-843`) with an `open_overlay` call on `HostServices` (`crates/cyrup-ext/src/host/services.rs:224`), modelled on the subagents caller at `crates/cyrup-ext-subagents/src/extension.rs:9908`; port `config-modal.ts:63-122`'s field list and commit semantics, routing each commit through the existing `save_extension_config` (`extension.rs:574-604`) so the write path is reused unchanged. Delete the stale rationale comment at `extension.rs:701-708`. Same as `PARITY-GAPS` PB-18.

**Verify** — Invoke `/permission-system` with a live `HostServices`; assert `open_overlay` was called (not a text return), toggle `yoloMode` inside the overlay, and assert both that `config.json` on disk changed and that the running gate auto-approves the next ask.

## PERM-008 — The forwarding path writes no audit entries: 8 review + 3 debug sites unported

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — The logging module itself landed: `crates/cyrup-permission-system/src/logging.rs` (431 lines) ports `logging.ts` including `LOGS_DIR_ENV_KEY = CYRUP_PERMISSION_SYSTEM_LOGS_DIR` (`:52`), per-write env re-resolution (`:75-82`) and v0.8.0's un-gating of the `review` stream (`:168-170`, ungated, vs the `debug`-gated `:141-146`). Ten decision-path call sites exist in `extension.rs` (debug `:483`/`:599`/`:651`/`:678`; review/decision `:1073`/`:1142`/`:1179`/`:1239`/`:1278`/`:1314`/`:1390`/`:1410`/`:1433`/`:1453`/`:1503`/`:1517`). The hole is the forwarding path: `grep -n 'logger\|review\|logging\|tracing\|write_debug' crates/cyrup-permission-system/src/forwarding.rs` returns **zero** across all 1125 lines.

**upstream** — `pi-permission-system/src/index.ts` writes eight review entries on the forwarding path — `:1032` `forwarded_permission.request_created`, `:1058` `.response_received`, `:1080` `.response_timed_out`, `:1173` `.expired`, `:1184` `.auto_approved`, `:1187` `.prompted`, `:1228` `.approved`/`.denied` — plus `logPermissionForwardingEntry` (`:735`) writing `permission_forwarding.warning`/`.error`, and three debug entries at `:641` `permission_forwarding.watcher_close_error`, `:660` `.watch_setup_error` and `:688` `permission_request.session_id_error`. (Minor correction to the prior write-up: `LOGS_DIR_ENV_KEY` is declared upstream in `extension-config.ts:43`, not `logging.ts`.)

**Impact** — The one subsystem whose failures are hardest to reproduce — a child ask that sat in the spool and fail-closed — is the one with no trail. A stuck or silently-denied subagent leaves nothing on disk saying whether the request was ever created, whether a response was seen, whether it expired, or whether a malformed/unbound response was discarded. Field diagnosis stays guesswork, and PERM-031's silent deny would likewise be invisible.

**Fix** — Thread the existing `logging::write_review_entry` / `write_debug_entry` handles into `forwarding.rs` (the watcher task already carries `SharedExtensionConfig` from PERM-005's fix, so the config needed for the debug gate is in scope) and add the eleven sites: request creation and response read in `wait_for_forwarded_approval`, the expiry/auto-approve/prompted/approved/denied arms of `resolve_forwarded_decision`, the malformed/unbound discard paths, and the three watcher-error sites. Same as `PARITY-GAPS` PB-17.

**Verify** — With the logs dir set and `debug: true`, drive a forwarded child ask through approve, through timeout, and through a malformed response; assert `forwarded_permission.request_created`, `.approved`, `.response_timed_out` and a `permission_forwarding.warning` entry each land in the JSONL.

## PERM-011 — Yolo runtime API has no publish seam; permission-request event channel absent

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — The methods exist as inherent methods on the extension: `extension.rs:608` `yolo_mode` (pi `index.ts:1481`), `:628` `set_yolo_mode` (pi `setYoloModeFromRuntimeApi`, `index.ts:1421-1468` — cyrup reproduces the persist-failure invariant: on save failure the live config is untouched and `changed:false, persisted:false` is returned), `:693` `toggle_yolo_mode`, with the option/result shapes in `yolo_api.rs`. **Open half A**: there is no publish seam, so `set_yolo_mode`'s only caller is `toggle_yolo_mode` (`:694`) and `toggle_yolo_mode` has none; `yolo_api.rs:7-19` admits this — and its claim at `:16` that the three methods are "reached through the `/permission-system` command" is contradicted by `extension.rs:721-728`, which routes the command through `save_extension_config` and never through `set_yolo_mode` (the same doc-asserts-wiring-that-does-not-exist pattern PERM-014 calls out). **Open half B**: the event channel is entirely absent — `grep -rn 'events.emit\|emit_event\|permission-request' crates/cyrup-permission-system/src` returns 0.

**upstream** — `pi-permission-system/src/index.ts:150` `PERMISSION_REQUEST_EVENT_CHANNEL = "pi-permission-system:permission-request"`, `:1518-1527` `emitPermissionRequestEvent`, `:1531-1546` `emitPermissionStateEvent`, fired at `:1606`, `:1612` and `:1626`; the runtime API is registered alongside at `:1481-1485`.

**Impact** — No other extension or front-end can read or flip yolo mode, and nothing can observe permission requests programmatically, so any external approval UI or policy observer is impossible to build. The ported methods are dead code that reads as done.

**Fix** — Half A: expose the three methods through the native-extension runtime-API seam so a second extension can call them, then correct the false claim at `yolo_api.rs:16`. Half B: add a native-extension event accessor on `SharedBus` in `crates/cyrup-ext` (area 06 owns the seam), then emit the request/state events from `prompt_decision` (`extension.rs:1384-1469`) at the three points pi fires them. Same as `PARITY-GAPS` UW-9 + PB-16.

**Verify** — From a second extension, toggle yolo mode and assert the next ask auto-approves; subscribe to the permission-request channel and assert an event is observed for a gated call, with a matching state event on the decision.

## PERM-012 — `registerModelOptionCompatibilityGuard` (temperature stripping) has no cyrup counterpart

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-provider/src/api/openai_responses.rs:359-360`, `openai_codex_responses.rs:707-708` and `azure_openai_responses.rs:386-387` each do `if let Some(temp) = opts.temperature { obj.insert("temperature".to_string(), json!(temp)) }` with no model or API guard. cyrup *does* have a `supports_temperature` compat flag at `api/compat.rs:133`, but it is consulted only in `anthropic_messages.rs:702`; grep shows zero references in any of the three `*_responses.rs` files. `grep -rn 'does not support temperature\|unsupported_temperature\|model_option_compat' crates --include=*.rs` = 0.

**upstream** — `pi-permission-system/src/model-option-compatibility.ts:62-81` `getUnsupportedTemperatureReason` guards exactly those three APIs (`GUARDED_TEMPERATURE_APIS`, `:11-15`) plus the `openai-codex` provider, `codex`-token model ids and reasoning models; `:89` `stripUnsupportedTemperatureFromPayload`; `:164` `registerModelOptionCompatibilityGuard`, lazily registered from `index.ts:1485-1497` and awaited as the **first** statement of the `session_start` handler (`index.ts:1829`). The file is absent from the `v0.7.1..v0.8.0` diffstat, so this predates the ported baseline — a port omission, not lag.

**Impact** — Requests to reasoning models that reject `temperature` fail at the provider with an API error, where pi silently strips the option and succeeds.

**Fix** — Land the guard in `cyrup-provider`, not this crate: extend the existing `api/compat.rs:133` `supports_temperature` decision with the model-id/provider predicates from `model-option-compatibility.ts:62-81`, and gate the three inserts at `openai_responses.rs:359-360`, `openai_codex_responses.rs:707-708` and `azure_openai_responses.rs:386-387` on it. pi solves a core-model problem from inside the extension; cyrup should solve it in core.

**Verify** — Issue a request against a reasoning model with a non-default temperature through each of the three APIs; assert the wire body omits `temperature` and the call succeeds.

## PERM-013 — `before_agent_start` result caching not ported

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `on_before_agent_start` (`crates/cyrup-permission-system/src/extension.rs:1577-1623`) recomputes the exposed tool set, calls `set_active_tools` unconditionally (`:1590`), and re-runs `sanitize_available_tools_section` (`:1593`) and `resolve_skill_prompt_entries` (`:1604`) on **every** turn. There is no cache key of any kind: `grep -rn 'last_active_tools\|last_prompt_state\|agent_start_cache' src/` returns only a doc mention at `extension.rs:466`. `policy_cache_stamp` exists (`manager.rs:429`) but is private and feeds only the resolve cache (`:396`).

**upstream** — `pi-permission-system/src/before-agent-start-cache.ts` (whole 31-line file: `createActiveToolsCacheKey` `:15`, `createBeforeAgentStartPromptStateKey` `:19`, `shouldApplyCachedAgentStartState` `:29`), consumed at `index.ts:1893-1895` (call `setActiveTools` only when the key changed) and `:1898-1907` (short-circuit returning the cached systemPrompt/entries), invalidated by `invalidateAgentStartCache` (`index.ts:1326-1331`). `getPolicyCacheStamp` is deliberately **public** on `permission-manager.ts:781` precisely so the agent-start cache can key on it.

**Impact** — Per-turn CPU on tool filtering, prompt sanitation and skill-entry resolution that pi pays once. The recomputed result is identical, so this is cost, not wrong behaviour — hence low.

**Fix** — Make `policy_cache_stamp` (`manager.rs:429`) public, add a `before_agent_start_cache` module porting the three functions from `before-agent-start-cache.ts`, key it on the stamp plus the registry tool list, and short-circuit `extension.rs:1590-1604` on a hit. Invalidate it from `refresh_config_and_manager` (`:472-495`), pi's `invalidateAgentStartCache` placement.

**Verify** — Drive three consecutive `before_agent_start` events with an unchanged policy; assert `set_active_tools` is called once and the sanitizer runs once, and that a mid-session policy edit re-runs both.

## PERM-014 — Concurrent-duplicate ask collapse implemented in `dedup.rs` but never wired

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/dedup.rs` still defines `Lookup` (`:108`), `Pending` (`:119`) with its async wait (`:129`), `PendingOwner` (`:160`) with `resolve` / `forget` (`:160`/`:175`), `DedupCache::lookup` (`:202`) and `begin_pending` (`:254`). `grep -rn 'begin_pending\|\.lookup(' src/ tests/` outside `dedup.rs` returns **nothing**. The live path is `get` (`extension.rs:1386`) plus `remember_prompt_decision` (`:1469`) — the decision enters the cache only **after** the human answers. A second defect found this pass: `dedup.rs`'s own module doc at `:13-16` asserts that "the caller (the gate's ask path, `extension.rs`) must register the in-flight decision with `begin_pending` BEFORE awaiting the human, mirroring pi's ordering exactly" — a claim about wiring that does not exist, which is precisely how this item stayed invisible to earlier passes.

**upstream** — `pi-permission-system/src/index.ts:1598-1642`: `decisionPromise` is built, then `rememberPermissionPromptDecision(..., decisionPromise)` runs at `:1631` **before** `await decisionPromise` at `:1635`, with a `forgetPermissionPromptDecision` rollback in the catch at `:1638-1640`. A concurrent identical ask therefore hits `:1581-1594` and awaits the same promise. (The TTL half is already correct: `dedup.rs:35` `CACHE_TTL` = 2 min = `index.ts:155`.)

**Impact** — Two concurrently in-flight calls with an identical dedup key can both raise a prompt, so the operator answers the same question twice and the two answers can disagree. Reachability was checked rather than assumed: `crates/cyrup-agent/src/loop_fn.rs:62` documents tool execution as "Default parallel", so two identical in-flight asks are a real configuration.

**Fix** — Replace the `get`/`remember_prompt_decision` pair inside `prompt_decision` (`extension.rs:1386` and `:1469`) with `DedupCache::lookup` + `begin_pending`: register the pending entry before awaiting the human, await `Pending` on a hit, resolve through `PendingOwner::resolve` on the answer and `forget` on an error path, mirroring `index.ts:1631`/`:1635`/`:1638-1640`. Then the module doc at `dedup.rs:13-16` becomes true.

**Verify** — Fire two concurrent identical asks; assert exactly one prompt is raised and both callers receive the same decision. Add a second test where the prompt errors and assert the entry is forgotten rather than latched.

## PERM-017 — Forwarding-root agent-dir env overrides not ported (documented as deliberate)

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/forwarding.rs:63-68` declares only `FORWARDING_AGENT_DIR_ENV = CYRUP_PERMISSION_SYSTEM_FORWARDING_AGENT_DIR` and documents the three subagent-only middle levels as N/A; `forwarding_root_dir` (`:126-135`) consults exactly one variable.

**upstream** — `pi-permission-system/src/permission-forwarding.ts:62-92` @v0.8.0 resolves five levels in order: `PERMISSION_FORWARDING_AGENT_DIR_ENV_KEY` → `PI_DELEGATED_AUTH_RUNTIME_DIR` → `PI_MULTI_AUTH_RUNTIME_DIR` → `PI_PERMISSION_SYSTEM_POLICY_AGENT_DIR` → `defaultAgentDir()`; the three middle levels are all guarded by `options.isSubagent`. The `v0.7.1..v0.8.0` diff of that file is a pure `normalizeAgentName` extraction — no level added or removed.

**Impact** — None today: cyrup has no analog of pi's delegated/multi-auth runtime dirs, so the three middle levels have no meaning in this process model. **This item exists as a revisit trigger, not as work**: if cyrup ever grows a delegated-auth runtime dir, the forwarding root must learn the same precedence or a subagent will spool into the wrong tree.

**Fix** — No action while the middle levels remain meaningless. When a delegated-auth runtime dir lands, extend `forwarding_root_dir` (`forwarding.rs:126-135`) to the five-level chain and mark the subagent-only levels with the same `isSubagent` guard.

**Verify** — n/a until triggered. Scope note: this item covers the **forwarding** root only; the policy-file root's use of the same upstream env key is `PERM-025`.

## PERM-019 — Install state inferred from a byte-exact template comparison

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/ext_config.rs:312-316` `is_pristine_default_file` is a byte-exact `text == default_config_content() || text == LEGACY_DEFAULT_CONFIG_CONTENT` compare, feeding the install probe at `extension.rs:2173-2174`.

**upstream** — `pi-permission-system` has no install probe at all: `index.ts:1308` registers unconditionally and the only on/off is `enabled` (`extension-config.ts:88` → `index.ts:1475-1477`). So there is no upstream analog to drift from — this is a cyrup-original mechanism.

**Impact** — Any future whitespace, key-ordering or comment change to the config template silently re-arms the gate for every already-materialized agent dir: their on-disk file stops matching the new literal, reads as "hand-edited", and `is_installed` flips to true. The **concrete** hazard PERM-010 warned about is discharged — the `enabled` landing added a second frozen template (`ext_config.rs:286-287`) rather than editing the old literal — but the fragility that made it a hazard is unchanged, and each future template revision adds another frozen constant.

**Fix** — Replace the byte compare with a semantic one: parse both sides through `jsonc::parse_config` and compare the resulting key/value maps, so formatting changes are inert and only a real value edit reads as installed. Longer term, follow PERM-023's suggestion and shrink the probe to "any hand-authored artifact exists", which retires `is_pristine_default_file` and the frozen-template list entirely.

**Verify** — Rewrite an agent's `config.json` with the same values but different whitespace and key order; assert `is_installed` stays false. Change one value; assert it flips true.

## PERM-020 — Two env-mutating test sites still not serialized on a lock

**Kind** test-defect · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — Two of the three previously named sites are fixed: `not_installed_without_policy_or_env_returns_none` now runs inside `without_install_env` (`extension.rs:2244-2262`, helper `:2467-2480` taking `env_lock()` at `:2468`) with the old false comment replaced by an explicit correction at `:2245-2257`, and the shared construction path is guarded by `with_config_env_lock` (`:2317-2326`). **Still open, site (2)**: `crates/cyrup-permission-system/src/ask.rs:390-415` `forwarding_channel_denies_when_no_parent_anchor` does an unlocked `unsafe { std::env::remove_var(PARENT_SESSION_ENV_VAR) }` at `:404` and `set_var` at `:407` while `ForwardingAskChannel::confirm` (`ask.rs:295`) and every other test's `resolve_config_path` / `resolve_logs_dir` call `std::env::var` concurrently — in Rust 2024 that pairing is **UB**, not merely flaky. It is a `src/` unit test, so it *can* reach the crate-private lock. **A fourth site found this pass**: `crates/cyrup-permission-system/tests/prompt_dedup.rs:67-81` `ensure_subagent_child` does an unlocked `set_var("CYRUP_SUBAGENT_CHILD", "1")` inside a `Once` while both `#[tokio::test]`s in that binary run concurrently; being an integration binary it cannot reach the crate-private `ext_config::env_lock()`, so that site needs its own lock.

**upstream** — n/a (test hygiene; pi's suite runs one process per file under bun and has no shared-env hazard).

**Impact** — Undefined behaviour on every run of the crate's test suite, not merely order-dependent flakiness — and flakiness in exactly the class the earlier fix was written to eliminate, which gets re-run rather than believed.

**Fix** — Take `crate::ext_config::env_lock()` at the top of `ask.rs:390-415`. For `tests/prompt_dedup.rs:67-81`, either export the lock behind a `pub(crate)`-plus-test-only feature or give the integration binary its own `static ENV_LOCK: Mutex<()>` held across the `set_var` and the whole test body. **Do not mark this item closed on the `ask.rs` fix alone** — the fourth site is the one a same-file patch will miss.

**Verify** — Run the crate's unit and integration tests repeatedly at high `--test-threads`, ideally under `cargo +nightly miri` or with `RUST_BACKTRACE` env-race instrumentation; assert no order-dependent failures and no unsynchronized `setenv`/`getenv` pairing remains (grep `src/ tests/` for `set_var`/`remove_var` and check each for a held lock).

## PERM-021 — PERM-003 test's re-arm assertion covers only the policy warning

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/extension.rs:2894-2902`: the re-arm assertion is `assert!(host.warnings().len() > before, …)` — a bare count, satisfied by the **policy** warning alone. It cannot be satisfied by the config warning: `WarningSink::reset` (`:212-214`, called at `:2000`) clears only `shown`, while `last_config_warning` (`:292`) is cleared only by a clean load (`:501-504`) or a successful save (`:597`/`:670`).

**upstream** — That suppression matches pi exactly: `index.ts:1367-1372`'s `result.warning !== lastConfigWarning` memo survives `resetShownWarnings` (`:1333-1335`), so pi also reports a still-broken `config.json` once per process. The **behaviour is correct**; only the assertion overstates what it proves.

**Impact** — The test's comment claims it proves both warning channels re-arm across a session boundary; it proves only one. A future change that stops re-arming the policy warning while leaving the config path alone would still pass. No user-visible consequence today.

**Fix** — Assert on the warning **contents**, not the count: capture `host.warnings()` before and after and assert the delta contains the policy-warning text; then add a sibling test that reaches the config channel via a clean-load-then-corrupt sequence (so `last_config_warning` is legitimately cleared) and assert that text appears too. Correct the comment at `:2894` to state which channel each assertion covers.

**Verify** — Break only the policy file and assert the config-channel assertion fails; break only `config.json` after a clean load and assert the config assertion fires.

## PERM-022 — Spool test's child bound is shorter than the poll window

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/tests/forwarding_spawn_env.rs:326` spawns the child as `spawn_child(agent_dir.path(), &parent_id, &sentinel, 8_000)` (threaded to `CYRUP_PERMISSION_FORWARDING_TIMEOUT_MS` at `:249`) while `:327` polls for up to 15 s (`await_spooled_request(..., Duration::from_secs(15))`). At bound expiry the child deletes the artifact every assertion at `:329-352` reads — `forwarding.rs:474` `let _ = std::fs::remove_file(&request_path);` then `cleanup_location_if_empty`. Its sibling at `:383` uses `20_000`.

**upstream** — n/a (test hygiene).

**Impact** — **Re-rated low this pass.** `await_spooled_request` (`tests/forwarding_spawn_env.rs:283-302`) returns on the first poll that finds a `.json`, sleeping 20 ms between passes, so the artifact is normally read milliseconds after it is written; the 8 s/15 s mismatch bites only if the task is starved for the entire 8 s window. The failure mode is `None` → an explicit `panic!` at `:330-336`, so this test can only **false-FAIL**, never false-PASS — it hides no defect. That is the same reasoning used to dismiss `tests/forwarding_preserve_location.rs:157` as a non-instance, applied consistently. What remains is a misleading panic message ("the child's ask never reached the parent's forwarding spool") for a scheduling outcome the test cannot control.

**Fix** — Raise the child bound at `forwarding_spawn_env.rs:326` from `8_000` to `20_000`, matching its sibling at `:383`, so the artifact outlives the poll window by construction.

**Verify** — Run the test under `--test-threads` saturation; assert it passes deterministically.

## PERM-023 — Install probe ignores agent-scoped `permission:` frontmatter the manager enforces, so the gate never attaches

**Kind** cyrup-original · **Severity** high *(raised from medium in the repair pass)* · **Effort** S · **Confidence** high

> **Why high.** This is the same fail-open class as `PERM-009` with a narrower trigger, and it was
> rated on code shape (a probe predicate) rather than on consequence (an operator's `permission:`
> deny rules are silently not enforced). Held below `critical` because the deployment shape is
> narrow — the operator's *only* policy artifact must be agent markdown — and because the host's own
> approval flow still runs, so what is lost is one policy layer, not all gating.

**cyrup** — `is_installed` (`crates/cyrup-permission-system/src/extension.rs:2159-2175`) recognises exactly four signals: `CYRUP_PERMISSION_SYSTEM` (`:2160`), `<agent_dir>/cyrup-permissions.jsonc` and `<cwd>/.cyrup/agent/cyrup-permissions.jsonc` (`:2163-2166`), and a non-pristine **resolved** `config.json` (`:2173-2174`). The engine it guards reads strictly more: `manager_paths_for` (`:390-401`) wires `agents_dir: agent_dir.join("agents")` (`:394`) and `project_agents_dir` (`:396`), and `manager.rs:500-508` `load_agent_permissions` → `load_agent_permissions_from(Some(&self.paths.agents_dir), …)` loads `<agents_dir>/<agent>.md` frontmatter as an **enforced** policy layer. Nothing in `is_installed` looks at either agents dir.

**upstream** — `pi-permission-system` has no install probe: `index.ts:1308` registers unconditionally and the only on/off is `enabled` (`extension-config.ts:88` → `index.ts:1475-1477`). Agent markdown is first-class policy upstream — `permission-manager.ts:715-745` `loadAgentPermissionsFrom` via `resolveAgentMarkdownPath` (`:582-595`). cyrup's probe is therefore a cyrup-original whose signal set is strictly narrower than the engine behind it.

**Impact** — An operator whose only policy artifact is a persona's markdown frontmatter gets `is_installed == false`, no extension attached, and zero gating; their deny rules are silently inert. Fail-open direction, and the failure is quiet from the operator's side because the artifact they authored looks authoritative.

**Fix** — Extend `extension.rs:2163-2166` to also return true when `<agent_dir>/agents/` or `<cwd>/.cyrup/agent/agents/` exists and is non-empty — neither directory is ever written by this crate, so it carries none of the self-footprint problem that produced PERM-002. Longer term, land the shrink described in PERM-019: with the `enabled` switch now in place (PERM-010), the probe can become "any hand-authored artifact exists", which retires PERM-019 as well.

**Verify** — With no policy file, no env var and no `config.json`, write `<agent_dir>/agents/coder.md` carrying a `permission:` frontmatter deny; assert `permission_extension_for_env(agent_dir, cwd)` is `Some` (today `None`) and that the deny is enforced. Serialize on `crate::ext_config::env_lock()`.

## PERM-024 — Extension config is not refreshed on `before_agent_start`

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/extension.rs:1967-1979` — the `BeforeAgentStart` arm calls only `maybe_start_forwarding_watcher(ctx)` (`:1977`) and `on_before_agent_start` (`:1978`). `refresh_config_and_manager` (`:472-495`) still has exactly two callers: `SessionStart` (`:2007`) and `ResourcesDiscover` (`:2036`). So `self.config` is a snapshot updated only at those two events.

**upstream** — `pi-permission-system/src/index.ts:1875-1878`: the `before_agent_start` handler is `runtimeContext = ctx;` → `refreshExtensionConfig(ctx);` (`:1877`) → `startForwardedPermissionPolling(ctx);` (`:1878`). pi re-reads `config.json` at the top of every turn, in addition to the session_start path (`:1821`) and the resources_discover reload branch (`:1848`).

**Impact** — Editing `config.json` mid-session takes effect only at the next session start or resource reload. Low, and lower than when first filed: PERM-005's `SharedExtensionConfig` now keeps the *watcher's* view of `yoloMode`/`forwardedPromptTimeoutSeconds` live independently, so the stale window is confined to the in-process gate. It is also the missing half that makes PERM-007's modal feel instantaneous once it lands.

**Fix** — Call `self.refresh_config_and_manager(&ctx.cwd)` at the top of the `BeforeAgentStart` arm (`extension.rs:1977`), before `maybe_start_forwarding_watcher`, matching pi's ordering. pi's `refreshExtensionConfig` refreshes only the config, not the manager; if a per-turn manager rebuild proves expensive, split `refresh_config_and_manager` and call only the config half — and land PERM-013's cache first so the rebuild is keyed rather than unconditional.

**Verify** — Start a session with `yoloMode: false`, drive one `before_agent_start` plus an ask and assert a prompt; rewrite `config.json` with `yoloMode: true`; drive a second `before_agent_start` and identical ask and assert it auto-approves. Today the second ask still prompts.

## PERM-025 — Policy-root env override `PI_PERMISSION_SYSTEM_POLICY_AGENT_DIR` has no cyrup analog

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/extension.rs:390-401` `manager_paths_for` builds `global_config_path` (`:393`), `agents_dir` (`:394`), `legacy_global_settings_path` (`:397`) and `global_mcp_config_path` (`:398`) as bare `agent_dir.join(...)` with no environment consultation, and it is the only producer of `ManagerPaths` (called from all three constructors at `:318`/`:346`/`:376` and from `refresh_config_and_manager` `:493`). `is_installed` (`:2159-2175`) likewise probes only `agent_dir.join(POLICY_FILE)` and the project dir. `grep -rn 'POLICY_AGENT_DIR' crates/` across the whole cyrup workspace = **0 hits**; no such const is declared anywhere.

**upstream** — `pi-permission-system/src/permission-manager.ts:29` @v0.8.0 `const PERMISSION_POLICY_AGENT_DIR_ENV_KEY = "PI_PERMISSION_SYSTEM_POLICY_AGENT_DIR";`, `:31-33` `defaultPolicyAgentDir()` returning `resolve(override)` when the trimmed env value is non-empty else `getAgentDir()`, and `:35-38` the four default path builders (`pi-permissions.jsonc`, `agents`, `settings.json`, `mcp.json`) that consume it. The constructor at `:625-630` falls back to each builder, and `createPermissionManagerForCwd` (`index.ts:1287-1301`) supplies only `projectGlobalConfigPath` / `projectAgentsDir` / `onWarning` — so **every** global policy path in a live pi session comes from that override. It is documented as a supported knob (upstream `README.md:133`/`:247`/`:264`). Absent from the `v0.7.1..v0.8.0` diff, so this is a port omission, not lag.

**Impact** — **Severity corrected down to low from the audit's medium**, with the caveat stated inline: the audit rated it medium on a fail-open — with the policy relocated, `is_installed` probes the un-overridable location, returns false, and `permission_extension_for_env` attaches no extension at all. That consequence **cannot occur today**, precisely because the key is unread: no cyrup deployment can relocate the policy root, so probe and engine never disagree. The precondition that would make it latent was also checked — no subagent spawn site writes an isolated `CYRUP_AGENT_DIR` into a child's overlay — so there is no trigger either. What remains is a documented upstream knob that is silently inert, the same class and the same rating as PERM-017.

**Fix** — Add `POLICY_AGENT_DIR_ENV_KEY = "CYRUP_PERMISSION_SYSTEM_POLICY_AGENT_DIR"` alongside `ext_config::CONFIG_PATH_ENV_KEY` (`ext_config.rs:31`) and `forwarding::FORWARDING_AGENT_DIR_ENV` (`forwarding.rs:68`); add a `policy_agent_dir(agent_dir) -> PathBuf` helper applying the trim / non-empty / `resolve` precedence of `defaultPolicyAgentDir`; route **both** `manager_paths_for`'s four global paths (`extension.rs:393`/`:394`/`:397`/`:398`) **and** `is_installed`'s global-policy probe (`:2164`) through it, so probe and engine agree by construction exactly as PERM-018 made them agree for `config.json`.

**Verify** — Point `CYRUP_PERMISSION_SYSTEM_POLICY_AGENT_DIR` at a directory holding `cyrup-permissions.jsonc` with a `bash` deny, leave the agent dir empty of policy, and assert (a) `permission_extension_for_env(agent_dir, cwd)` returns `Some` and (b) a `bash` tool call is blocked. Serialize the test on `crate::ext_config::env_lock()`.

## PERM-026 — A `resources_discover` reload never re-syncs the yolo status pill

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/extension.rs:2027-2038` — the `HostEvent::ResourcesDiscover` arm clears dedup (`:2033`), resets warnings (`:2035`) and calls `refresh_config_and_manager(&ctx.cwd)` (`:2036`), then returns `Noop`. `refresh_config_and_manager` itself (`:472-495`) reloads config, emits `config.loaded`, rebuilds the manager and clears the skill cache — it contains no status call. The only status-sync sites in the crate are the `SessionStart` arm (`:2022-2024`), `on_before_agent_start` (`:1609-1611`), `sync_status_when_possible` (`:542-546`, reached only from the two config **writers**) and `clear_status` on shutdown (`:2042-2044`).

**upstream** — `pi-permission-system/src/index.ts:1382-1385` `refreshExtensionConfig` delegates to `applyExtensionConfigSideEffects` (`:1356-1380`), whose `:1364-1366` is `if (runtimeContext?.hasUI) { syncPermissionSystemStatus(runtimeContext, result.config); }`. `refreshExtensionConfig` is called from the session_start path (`:1821`), from `before_agent_start` (`:1877`) **and** from the resources_discover reload branch (`:1848`) — so upstream the pill is re-synced on every one of those.

**Impact** — An operator who edits `"yoloMode"` in `config.json` and triggers a resource reload gets the new gating behaviour immediately (the config is live) while the status bar keeps the stale pill: the gate silently auto-approves with no `yolo` indicator, or still shows `yolo` after it was turned off, until the next `before_agent_start` repaints it. Low because the next turn corrects it, but that window is exactly when a mis-set yolo flag matters.

**Fix** — Move the status sync into `refresh_config_and_manager` (`extension.rs:472-495`), immediately after the config assignment at `:478`, reusing the existing `self.sync_status_when_possible(&config)` helper (`:542-546`) which already collapses pi's ctx/runtimeContext branches. That makes `SessionStart`'s separate sync at `:2022-2024` redundant and gives `ResourcesDiscover` the behaviour for free, matching pi's placement inside `applyExtensionConfigSideEffects`.

**Verify** — With a `HostServices` recording `set_status`, drive `SessionStart` under `yoloMode: false`, rewrite `config.json` to `yoloMode: true`, dispatch `HostEvent::ResourcesDiscover`, and assert a `set_status("yolo")` was recorded **before** any `BeforeAgentStart` is dispatched.

## PERM-027 — `lifecycle.reload` debug entries never written

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/extension.rs` — the complete set of `write_debug_entry` call sites is `:483` (`config.loaded`), `:599` (`config.saved`), `:651` (`yolo_mode.update_failed`) and `:678` (`yolo_mode.updated`). The `SessionStart` arm (`:1991-2025`) receives `HostEvent::SessionStart { reason }` and never reads `reason`; the `ResourcesDiscover` arm (`:2027-2038`) writes no lifecycle record. `grep -rn 'lifecycle' crates/cyrup-permission-system/` returns only unrelated doc prose.

**upstream** — `pi-permission-system/src/index.ts:1834-1842` — inside the `session_start` handler, `if (event.reason === "reload") { writeDebugEntry("lifecycle.reload", { triggeredBy: "session_start", reason: event.reason, cwd: ctx.cwd }); }`; and `:1844-1857` — the whole `resources_discover` body is gated on the same reason and writes the identical event with `triggeredBy: "resources_discover"` and `cwd: runtimeContext?.cwd ?? null`. Both exist at v0.7.1, so this is a port omission rather than lag.

**Impact** — With `debug: true` the JSONL trail shows a `config.loaded` line for a reload that is byte-indistinguishable from the one a fresh session start writes, so an operator diagnosing "my policy edit did/didn't take effect" cannot tell from the trail whether a reload actually fired or which surface fired it. This is the diagnostic half of PERM-024 and PERM-026 — the two items whose symptom is "the reload did not do what I expected".

**Fix** — Add a `write_debug_entry("lifecycle.reload", &json!({"triggeredBy": …, "reason": …, "cwd": …}))` call at the tail of the `SessionStart` arm (`extension.rs:~2024`, gated on `reason == "reload"` — the field is already destructured away at `:1991`) and at the tail of the `ResourcesDiscover` arm (`:2037`). cyrup's `HostEvent::ResourcesDiscover` carries no `reason`, so pass the constant `"reload"` there, consistent with the existing comment at `:2030-2032` that treats every dispatch as the reload case.

**Verify** — With `debug: true`, dispatch `SessionStart { reason: "reload" }` and `ResourcesDiscover`, then assert the debug JSONL contains two `lifecycle.reload` lines whose `triggeredBy` values are `session_start` and `resources_discover` — and that a `SessionStart { reason: "startup" }` produces none.

## PERM-028 — `sensitive_log_metadata` length unit and untrimmed `decisionScope`

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-permission-system/src/logging.rs:253-264` `sensitive_log_metadata` emits `json!({ "present": true, "length": value.len(), "sha256": hex })` — `str::len()` is UTF-8 **bytes**. It is called on the prompt and on the command/denial-reason at `extension.rs:960`, `:966`, `:1189`, `:1287`, `:1396-1397` and `:1459-1460`. (The hash is not affected: node's `update(string)` defaults to utf8, so both sides hash the same bytes.) Separately, `permission_decision_scope` (`extension.rs:982-994`) returns the raw `&str` for the first non-empty of target/command/path/toolName/skillName, filtering on `!s.is_empty()` with no trim. The crate already establishes the correct convention for this exact hazard: `wildcard.rs:81` uses `pattern.encode_utf16().count()` with the rationale spelled out at `wildcard.rs:21-23`.

**upstream** — `pi-permission-system/src/index.ts:370-380` `createSensitiveLogMetadata` returns `{ present: true, length: value.length, sha256: … }` — JS `String.prototype.length` is UTF-16 code units. `index.ts:581-592` `getPermissionDecisionScope` applies `getNonEmptyString` — which **trims** (`common.ts:15-22`) — to `target`/`command`/`path`, then falls through to a raw `details.toolName ?? details.skillName`.

**Impact** — For any non-ASCII prompt, path or bash command, the `promptMetadata.length` / `commandMetadata.length` / `denialReasonMetadata.length` a cyrup review entry records differs from pi's for byte-identical input ("é" is 1 upstream, 2 here). The field exists precisely so two entries can be correlated and the plaintext redacted, so an audit pipeline cross-checking a cyrup trail against a pi trail, or re-deriving length after redaction, sees spurious mismatches. `decisionScope` likewise carries leading/trailing whitespace pi strips — and a whitespace-only command is *selected* by cyrup where pi skips it and falls through — so the same logical scope keys differently.

**Fix** — Change `logging.rs:263` to `"length": value.encode_utf16().count()`, matching `wildcard.rs:81`. Change `permission_decision_scope` (`extension.rs:982-994`) to trim each candidate before the `!s.is_empty()` filter and to return the trimmed value, mirroring `common::get_non_empty_string` (`common.rs:20`) — note pi applies `getNonEmptyString` to target/command/path **only** and falls through to a raw `toolName`/`skillName`, so keep those two unfiltered to stay exact. Land alongside PERM-030, which is the same defect class.

**Verify** — Unit-test `sensitive_log_metadata(Some("café"))` and assert `length == 4` (not 5); unit-test `sensitive_log_metadata(Some("a\u{1F600}b"))` and assert `length == 4` (surrogate pair counts 2). For the scope, assert a result with `command: Some("  git status  ")` produces `decisionScope == "git status"`, and one with `command: Some("   ")` falls through to the next candidate.

## PERM-029 — Shipped permissions JSON Schema and starter policy example not ported

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `find crates/cyrup-permission-system -type f ! -name '*.rs'` returns exactly one path, `crates/cyrup-permission-system/Cargo.toml` — no schema, no example config; a repo-wide `find . -name '*.schema.json'` finds neither anywhere in cyrup. `grep -rn 'permissions.schema\|\$schema' crates/cyrup-permission-system/src/` = 0, so nothing in the crate emits, validates against, or even names a schema. The hole is visible from inside the crate: the save path deliberately **preserves** a `"$schema"` key it can never have written (`ext_config.rs:394` names `$schema` in the preserved-keys doc, and the save fixture at `:1026` carries `"$schema": "https://example.invalid/permissions.json"`).

**upstream** — `pi-permission-system` @v0.8.0 ships both files and declares them in `package.json:14-15` (`"config/config.example.json"`, `"schemas/permissions.schema.json"`); `schemas/permissions.schema.json:3` declares `"$id": "https://pi-coding-agent.local/schemas/permissions.schema.json"`; `scripts/validate-artifacts.mjs:50` loads the schema and `:56` validates the example against it, wired into the package `check` script; `README.md:612`/`:614` document the tree and `:659` instructs "Add `"$schema": "./schemas/permissions.schema.json"` to your config for autocomplete support", with a CLI validation recipe at `:655`.

**Impact** — An operator hand-authoring `cyrup-permissions.jsonc` — which PERM-007's text-only command makes the normal way to configure anything — has no starter template and no machine-readable schema, so an editor gives no completion or validation and a typo'd category key silently degrades to the ask default with only the (deduped) parse warning as feedback. It also means nothing validates cyrup's own policy shape in CI, where upstream runs `validate:artifacts` on every check.

**Fix** — Add `crates/cyrup-permission-system/schemas/cyrup-permissions.schema.json` (a rebranded port of upstream's schema over `defaultPolicy`/`tools`/`bash`/`mcp`/`skills`/`special` and the agent layers) and `crates/cyrup-permission-system/config/config.example.json`; register both in `Cargo.toml`'s `include` so they ship with the crate; either embed the schema via `include_str!` behind a `/permission-system schema` emit or name its path from `render_settings` (`extension.rs:834-843`) beside the existing `Config file:` line. Add a test that parses the example through `crate::jsonc::parse_config` and feeds it to `PermissionManager`, which is what upstream's `validate-artifacts.mjs` buys.

**Verify** — Assert the example file parses through `jsonc::parse_config` and yields a `PermissionManager` whose `check_permission` returns the states the example's comments claim; assert the schema file is present in the packaged crate.

## PERM-030 — Ask-dialog formatters count Unicode scalars where pi counts UTF-16 units

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — A **third** counting unit, in the one place a human actually reads. `crates/cyrup-permission-system/src/gate.rs:248-255` `truncate_inline_text` tests `value.chars().count() > max_length` and slices with `chars().take(max_length)` — Unicode scalars. It governs every inline limit in the prompt: `TOOL_INPUT_PREVIEW_MAX_LENGTH` 200 (`gate.rs:242`), `TOOL_TEXT_SUMMARY_MAX_LENGTH` 80 (`gate.rs:245`) and the 40-unit edit-reference cap (`gate.rs:321`). Separately `gate.rs:456` renders the write-tool summary with `format_count(content.chars().count(), "character", "characters")`.

**upstream** — `pi-permission-system/src/permission-prompts.ts:91-93` truncates with `value.length > maxLength ? `${value.slice(0, maxLength)}…`` — UTF-16 code units — against the same three constants at `permission-prompts.ts:8`, `:9` and `:128`; `permission-prompts.ts:193` uses `formatCount(content.length, …)`.

**Impact** — For any astral-plane character — an emoji in a bash command, in a file path, or in `write` content — pi counts 2 and cyrup counts 1, so the approval dialog shows a different character count and truncates at a different boundary for byte-identical input. Cosmetic per instance, but it is the text on which a human bases an allow/deny, and it is the same defect class as PERM-028 in a higher-stakes surface.

**Fix** — Apply PERM-028's one-line convention at `gate.rs:249` (the test), `:250` (the slice must take UTF-16 units and re-decode, not `chars().take`) and `:456`, matching `wildcard.rs:81`. Land with a crate-wide sweep for `chars().count()` / `str::len()` standing in for JS `String.length`, so the three units in this crate collapse to one.

**Verify** — Unit-test `truncate_inline_text("a\u{1F600}…", 2)` and assert the boundary matches pi's `slice(0, 2)` on the same input; assert the write summary for content containing one emoji reports the UTF-16 count.

## PERM-031 — Forwarding watcher drops upstream's per-scan `hasUI` guard

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `process_forwarded_requests` (`crates/cyrup-permission-system/src/forwarding.rs:528-545`) has **no** `has_ui` test at all. The only one is at watcher (re)start in `maybe_start_forwarding_watcher` (`extension.rs:1678-1695`: `if !self.install_watcher || !ctx.has_ui { self.stop_forwarding_watcher(); return; }`), which is re-evaluated only when a `ToolCall` / `Input` / `BeforeAgentStart` / `SessionStart` hook fires. Between hooks the spawned task keeps polling on its own ticker, and its decision path terminates in `AskOutcome::NoLiveChannel => denied()` (`forwarding.rs:~716-718`), which writes a nonce-bound DENY response the child consumes at `forwarding.rs:441-451` as a **final** answer.

**upstream** — `pi-permission-system/src/index.ts:1113-1116` — `processForwardedPermissionRequests` opens with `if (!ctx.hasUI) { return; }`, re-checked on **every** scan. While the UI is away the spool is simply not serviced and each request stays on disk until the UI returns or the child's own bound expires.

**Impact** — A mid-session UI detachment converts pending subagent asks into hard denials instead of leaving them queued: where pi defers, cyrup answers "denied" on behalf of an absent human, and the child treats it as the operator's decision. Fail-closed direction, hence low, but it is a behavioural divergence rather than a substrate difference — and per PERM-008 it leaves no log line. The crate itself asserts the scenario is real: PERM-005's fix comment ("a UI that detached mid-session left the watcher prompting into a dead backend") was written for exactly this hazard but closed only the next-hook teardown half.

**Fix** — Thread the live `has_ui` state into the watcher task the same way PERM-005 threaded `SharedExtensionConfig` (`forwarding.rs:732`) — a shared flag updated by the event arms — and open `process_forwarded_requests` (`forwarding.rs:528-545`) with an early return when it is false, mirroring `index.ts:1113-1116`. Requests then remain on disk for the child's own bound to expire, which is pi's behaviour.

**Verify** — Start a watcher with `has_ui: true`, spool a child request, flip `has_ui` to false without dispatching any hook, and assert that after several poll intervals no response artifact exists and the request is still on disk; flip it back and assert the request is then serviced.

## PERM-032 — A permission-denied tool result breaks the next provider request on `together/openai/gpt-oss-20b`

**Kind** *unclassified — lead* · **Severity** low · **Effort** M · **Confidence** **low — reproduced 3/3 against one model, but not isolated to cyrup; may be a provider-side format issue** · **observed 2026-08-13** (headless-binary; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Filed as a lead, at low confidence, deliberately.** It is filed rather than dropped because the
> controls below rule out the obvious innocent explanations, and because the failure is on the
> permission system's own `Block` path — but the kind is left **unclassified** because the decisive
> experiment (dumping the serialized request body on the block path and comparing it to the
> tool-error path) was not run. **Do not schedule work from this item until that experiment is
> run.**

**cyrup** — Suspected but **not confirmed**: the `HookOutcome::Block` path at `crates/cyrup-permission-system/src/extension.rs:1546-1549` and the agent's block handler downstream of it, i.e. the shape of the message cyrup synthesises to represent a denied tool call. No citation is offered as evidence here; the item rests on the behavioural controls below.

**upstream** — Not read for this item. pi's denial-result message shape must be compared against cyrup's as the first step of the investigation.

**Impact** — Reproducible **3/3** on `--provider together --model openai/gpt-oss-20b`: whenever the permission gate **blocks** a tool call, the follow-up request fails with

```
http 400: {"error":{"message":"Input validation error","type":"invalid_request_error"}}
```

Three controls, all on the same fixture, isolate it as far as it was taken:

| variation | result |
|---|---|
| same model, same config, same prompt, command **allowed** | turn completes normally |
| same model, ordinary tool **error** (read of a nonexistent file) | completes normally — *"The read attempt failed – the file /nonexistent/zzz.txt does not exist…"* |
| same **denied** call on `openai/gpt-oss-120b` | handled fine — *"I'm unable to run that command due to policy restrictions."* |
| same **denied** call on `Qwen/Qwen3.5-9B` | handled fine |

So the denial-result message shape is accepted by two models and rejected by a third, and it is the *denial* specifically — not tool failure in general — that triggers it. That is suggestive of a serialization defect on the block path rather than a pure provider quirk, but it is equally consistent with a Together-side gpt-oss-20b harmony-format constraint. User-visible effect where it bites: a denied tool call ends the turn with a raw HTTP 400 instead of the model explaining the refusal.

**Fix** — **Investigate before fixing.** (1) Capture the serialized request body for the turn following a block, and diff it against the body following an ordinary tool error on the same model. (2) Read pi's denial-result construction at v0.83.0 and compare field for field. (3) If cyrup's shape diverges, this becomes a `parity-bug` and the fix is to match pi; if it does not, close the item as a provider constraint and record the negative result so nobody re-derives it.

**Verify** — Once classified: a test asserting the request body cyrup emits after a `HookOutcome::Block` matches pi's shape field for field, plus a live run on `together/openai/gpt-oss-20b` where a denied `bash` call is answered by the model rather than by an HTTP 400.

## Coverage

**Read first-hand at cyrup HEAD `04c1ba2`** (branch `david/cyrup`), `crates/cyrup-permission-system/src/`: `lib.rs` (full), `extension.rs` (full, 3315 lines — every event arm, the layered gate, the install probe, the command handler, the config writers, the watcher lifecycle, the anchor publisher and the unit-test module), `ext_config.rs` (`:1-1060` in full plus targeted reads of the save tests), `forwarding.rs` (constants, path resolution, child wait, `process_forwarded_requests`, `resolve_forwarded_decision`, the watcher task), `gate.rs` (`:1-220` in full plus a full function outline of `:220-887`), `evaluate.rs` (full), `stores.rs` (full), `wildcard.rs` (full), `dedup.rs` (`:1-280`), `logging.rs` (`:40-265`), `yolo_api.rs` (full), `skill.rs` (`:60-150`), `manager.rs` (`check_permission` `:142-322`, `resolve_permissions`/`policy_cache_stamp` `:380-455`, method outline), `common.rs` (symbol outline). Tests read or grepped: `forwarding_spawn_env.rs`, `forwarding_preserve_location.rs`, `forwarding_subprocess.rs`, `prompt_dedup.rs`, `permanent_approvals_file_is_inert.rs`, `context_hygiene.rs`, plus a crate-wide timing/env-mutation hunt. Cross-crate for the `-S` closures and PERM-007/011/012: `crates/cyrup-ext-subagents/src/{exec/mod.rs, background/parent_anchor.rs, background/runner_main.rs, background/spawn_detached.rs, spawn/intercom_target.rs, extension.rs}`, `crates/cyrup-intercom/src/{connect.rs, session_state.rs, inbound.rs, extension.rs, tools/intercom.rs}`, `crates/cyrup-ext/src/host/services.rs`, `crates/cyrup-session-svc/src/host_services.rs`, `crates/cyrup-provider/src/api/{openai_responses.rs, openai_codex_responses.rs, azure_openai_responses.rs, compat.rs, anthropic_messages.rs}`.

**Read first-hand upstream at the v0.8.0 tag** via `git show v0.8.0:<path>`: `index.ts` (2236 lines — header constants, `getActiveAgentName`/`getSessionId`/`isSubagentExecutionContext`, `applyPatternApprovalState`/`getPatternApprovalSubject`/`createConfigEvaluationRule`/`getPermissionDecisionScope`/`persistSessionApprovalDecision`, `waitForForwardedPermissionApproval`, `processForwardedPermissionRequests`, `confirmPermission`, `createPermissionManagerForCwd`, `applyExtensionConfigSideEffects`/`refreshExtensionConfig`/`saveExtensionConfig`/`setYoloModeFromRuntimeApi`, the registration block, `promptPermission`, the watcher, `resolveAgentName`/`shouldExposeTool`, `refreshSessionRuntimeState` and all six event handlers), `extension-config.ts`, `common.ts`, `permission-manager.ts` (head + `checkPermission`), `permission-forwarding.ts`, `wildcard-matcher.ts`, `evaluate-permission.ts`, `session-approval-store.ts`, `permission-prompts.ts`, `before-agent-start-cache.ts`, `config-modal.ts`, `yolo-mode.ts`, `bash-filter.ts`, `model-option-compatibility.ts`, `tests/wildcard-redos.test.ts`, `package.json`, root `index.ts`.

**Severity re-derivation (repair pass, 2026-08-12).** The completeness critique's finding 4 was
checked rather than accepted, and it holds on both sides: `extension.rs:1651-1653` exists as quoted,
`manager.rs:205-215` resolves the bash command rule above the tool-level state as quoted, and
`shouldExposeTool` has no bash branch at **either** upstream tag (`index.ts:2049-2075` @v0.7.1 —
re-read specifically because the ported baseline, not the latest tag, governs the classification —
and `index.ts:1790-1816` @v0.8.0). `PERM-009` is therefore `critical`. The finding's class was then
applied to the rest of the file rather than to the one item it named, which produced exactly one more
change and two deliberate non-changes:
- **`PERM-023` medium → high** — a second fail-open on a configured policy. Re-verified this pass:
  `is_installed` (`extension.rs:2159-2175`) probes the env var, the two `cyrup-permissions.jsonc`
  locations and the resolved `config.json`, and nothing else; `manager_paths_for` (`:390-401`) wires
  `agents_dir` / `project_agents_dir`, and `manager.rs:500-503` `load_agent_permissions` loads
  `<agents_dir>/<agent>.md` as an enforced layer. Held below `critical`: the trigger requires agent
  markdown to be the operator's *only* policy artifact, and the host's own approval flow still runs,
  so one policy layer is skipped rather than all gating.
- **`PERM-031` stays low** — it is fail-*closed* (a pending child ask becomes a deny). The definition
  covers bypass, not over-refusal.
- **`PERM-014` stays medium** — a duplicate prompt is a double-ask, not a bypass; both answers are
  the operator's own, and the divergent-answer case resolves to whichever the caller reads, not to an
  unasked approval.

**Closure standard applied** — nothing accepted on a commit message. All four prior closures were re-attacked at HEAD with a specific kill hypothesis each (PERM-001: does the writer chain actually reach the detached `Command`? — yes, `spawn_detached.rs:178`/`:227`. PERM-002: does PERM-010's new key re-latch the probe? — no, second frozen template. PERM-003: is there a second production `PermissionManager` builder? — no, every other hit is `cfg(test)`. PERM-005: are all four hooks really wired and is the config really live? — yes, and the terminal returns are now a retry loop). Ten new closures were each proved by reading the Rust at HEAD **and** the TypeScript at v0.8.0, with the upstream deletion in PERM-004 verified from the diffstat rather than from prose.

**Method notes.** The version-lag sweep read `git diff v0.7.1..v0.8.0` per shipping file and is recorded in the update block above; its result — **zero** upstream-drift items — is a measurement, not an assumption, and it independently corroborates `PARITY-GAPS` §3d. The surface-driven sweep enumerated every `export` / module-level const / `process.env` reference across all 24 upstream `src/*.ts` files at v0.8.0 in one pass, then ripgrepped `crates/` for each consumer; that is what produced PERM-025 (an env key with literally zero occurrences anywhere in cyrup), PERM-027 (an event name with zero occurrences), PERM-029 (two shipped artifacts with zero counterparts) and PERM-026/PERM-028 (call-site and unit mismatches invisible to any item-driven pass). PERM-030 and PERM-031 came from a second sweep of code the first pass did not walk — `gate.rs`'s formatter family and the watcher's abort/detach path — which is itself evidence that one sweep does not exhaust the class.

**Rejected with reason** (checked this pass, deliberately **not** filed — do not re-derive these):
1. `BashFilter` (upstream `src/bash-filter.ts`) has no cyrup counterpart, but `git grep BashFilter v0.8.0` shows its only consumers are three test files. Dead on both sides.
2. `skill_input` (`index.ts:107`) and `createPermissionRequestId` (`index.ts:1514`) are dead upstream.
3. `getActiveAgentName` reading `active_agent` session entries (`index.ts:275-295`) has no writer in pi or pi-subagents at these tags; the live writer is `<active_agent>` prompt injection from `pi-subagents/src/runs/shared/pi-args.ts:609-612`, whose cyrup analog is the `CYRUP_SUBAGENT_AGENT_NAME` env var written at `crates/cyrup-ext-subagents/src/exec/mod.rs:1677` and read at `extension.rs:1860-1865` — a documented mechanism swap with no behavioural loss in the process-per-subagent model.
4. cyrup's skill-read audit records the cwd-**injected** `toolInput` (`extension.rs:1033-1041` injects before `:1044`) where pi records the raw `event.input` (`index.ts:1970`, injection at `:2047-2050`) — an audit-cosmetic one-key difference, too thin to file.
5. `WarningSink::notify` deliberately drops pi's `hasUI` half of the guard (documented CYRUP-DELTA at `extension.rs:193-200`).
6. `tests/forwarding_preserve_location.rs:157`'s `wait_until(2s, || !dir.is_dir())` is a wait-for-a-bad-thing-not-to-happen check that can only false-PASS in the *safe* direction, so it is not an instance of the uncontrollable-timing test-defect class. The same reasoning re-rated PERM-022 to low.
7. `wildcard::compile`'s `regex: None` fallback on a build failure resolves to never-match, which falls through to Ask / the category default — the safe direction.
8. PERM-015's stricter delta (`None` matches nothing; pi's `/$^/` matches the empty string) is documented at `wildcard.rs:43-49` and unreachable on the decision path.

**Handoffs to other areas.** `PERM-S01`/`S02`/`S03` are pi-intercom / pi-subagents items that the 2026-08-03 surface sweep filed into this file; all three now close on evidence in `crates/cyrup-intercom` and `crates/cyrup-ext-subagents`, and **area 11 owns that surface going forward** — including the unswept `pi-intercom` v0.9.2..v0.10.1 and `pi-subagents` v0.43.0..v0.47.1 deltas. PERM-012's fix lands in `crates/cyrup-provider` (three `api/*_responses.rs` files plus `compat.rs`), not this crate. PERM-011's event-channel half needs a native-extension accessor on `SharedBus` in `crates/cyrup-ext` (area 06). PERM-007's modal half needs `HostServices::open_overlay`, already live (areas 07/08).

**Nothing was executed** (no cargo, no bun, per task rules). Every "blocks / denies / re-prompts / goes stale" claim is by code reading on both sides. In particular PERM-014's concurrent-duplicate window, PERM-022's flake, PERM-026's stale-pill window and PERM-031's detach window have never been observed running.

**Blind spots** — not read line by line, so a defect there would not have been found by this pass. In this crate: `manager.rs` outside `check_permission` / `resolve_permissions` / `policy_cache_stamp` — specifically the ~550 lines of layer construction, `build_layers`, `compile_from_layers`, the trusted-floor `resolve_layered_default_permission` analog, `create_action_resource_targets`, `create_mcp_permission_targets` and `resolve_agent_markdown_path`'s traversal guard; `sanitize/tools.rs` and `sanitize/skills.rs` bodies (only their function outlines were compared to upstream); `jsonc.rs`, `ordered.rs`, `status.rs`, `error.rs`; `gate.rs:220-887` (outline-compared only — the whole `format_*` prompt family); `ext_config.rs:1061-1291` (its remaining save tests). Upstream: `zellij-modal.ts` (1037 lines) and `config-modal.ts` internals, `model-option-compatibility.ts`'s guard body, `tool-registry.ts`, `status.ts`, and the six test files other than `wildcard-redos.test.ts`.

**Prompt-string fidelity is unverified as a class.** `permission-prompts.ts` is a new v0.8.0 file that **moved** ~330 lines of formatters out of `index.ts`. cyrup has a function for every export and `formatDenyReason` / `formatAskPrompt` / the external-directory family were spot-read, but the literals in `formatToolInputForPrompt`'s structured-edit summaries (`edit #N replaces X lines at ANCHOR through ANCHOR`, the append/prepend/delete arms) were **not** diffed against `gate.rs:332-547`. A drifted literal there changes what the human is shown in the approval dialog and would not have been caught. PERM-030 was found in this file's neighbourhood, which raises rather than lowers the prior that more is hiding there — **this is the highest-value target for the next pass.**

**Citation hazard.** This crate's in-tree upstream citations are systematically offset: they were written against v0.7.1 `index.ts` line numbers and the v0.8.0 file is ~700 lines shorter with the formatters extracted (e.g. `extension.rs:1625` cites `index.ts:2049-2075` for `shouldExposeTool`, now `index.ts:1790-1816`; `gate.rs:187`/`:198` cite `index.ts:352-382` for functions now in `permission-prompts.ts:48-77`). Every citation relied on above was re-resolved by reading the v0.8.0 file, but the crate's ~200 stale citations were not corrected, and a reader who trusts them will land in the wrong place. Also note two stale doc-comment claims found this pass that assert wiring which does not exist — `dedup.rs:13-16` (inside PERM-014) and `yolo_api.rs:16` (inside PERM-011) — plus one stale rationale, `extension.rs:701-708` (inside PERM-007). Doc comments in this crate are not evidence.

**Taken on trust / uncheckable.** `spec/` is absent from this workspace, so PERM-009's bash-exposure branch (`extension.rs:1624-1631` cites a spec mandate) and PERM-019's install-probe design cannot be checked against a requirement of record — absence was treated as neither confirming nor refuting. **This is the one open question in the area that needs a human**: if a mandate really requires `bash` to stay exposed under a tool-level deny, someone must say so and say what re-gates execution, because per `README.md:208-212` an in-source `R-NN-NNN`/spec reference is not a decision of record and cannot hold a `critical` down. Until then PERM-009 is scheduled as work. PERM-025's fail-open consequence is reasoned from `is_installed`'s structure and was **downgraded** on that basis: no cyrup deployment can relocate the policy root while the key is unread, and no spawn site writes an isolated `CYRUP_AGENT_DIR`, so there is no trigger today.

**Known incomplete-fix trap.** PERM-020 names two sites in different compilation units (`src/ask.rs` and `tests/prompt_dedup.rs`); a patch that touches only `ask.rs` will look closed while the integration-binary site remains, and the other nine integration-test binaries were **not** swept for the same unlocked-`set_var` shape — only those grep surfaced.
