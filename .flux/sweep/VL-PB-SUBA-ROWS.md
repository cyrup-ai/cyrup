# Sweep B — VL / PB / SUBA rows, audited at 14e6c56

Scope enumerated from the file itself: `grep -n '^\*\*\(VL\|PB\)-'` over
`docs/gap-analysis/PARITY-GAPS.md` (§1a, §1b, §1c, §1d, §3a's `VL-P` table) plus §3b's
self-declared still-open set. Upstream reads are pinned with `git -C tmp/<upstream> show <tag>:<path>`
at the tag each row names. No repository file was edited; this file is the only output.

## Summary

| row | verdict | severity still right? |
|---|---|---|
| PB-1 | **CLOSED** | n/a |
| PB-2 | **CLOSED** | n/a |
| PB-3 | **CLOSED** | n/a |
| PB-4 | **CLOSED** | n/a |
| PB-5 | **CLOSED** | n/a |
| PB-6 | STILL OPEN, citations drifted | yes — *medium* |
| PB-7 | STILL OPEN, citations drifted | yes — *large* |
| PB-9 | STILL OPEN, citations drifted | yes — *large* |
| PB-14 | STILL OPEN, citations drifted | yes — *small* |
| PB-15 | **CLOSED** | n/a |
| PB-16 | **CLOSED** | n/a |
| PB-17 | **CLOSED** (row already says so) | n/a |
| PB-18 | **CLOSED** | n/a |
| PB-19 | **CLOSED** | n/a |
| PB-20 | **CLOSED** | n/a |
| PB-21 | **CLOSED** | n/a |
| PB-22 | **CLOSED** | n/a |
| PB-23 | **CLOSED** | n/a |
| PB-24 | **CLOSED** | n/a |
| PB-25 | **CLOSED** | n/a |
| PB-26 | **CLOSED** | n/a |
| PB-27 | **CLOSED** | n/a |
| PB-28 | **CLOSED** | n/a |
| PB-29 | **CLOSED** (row already says so) | n/a |
| PB-30 | **CLOSED** | n/a |
| PB-32 | **CLOSED** | n/a |
| PB-33 | **CLOSED** | n/a |
| PB-34 | **CLOSED** | n/a |
| PB-35 | **CLOSED** | n/a |
| PB-36 | **CLOSED** | n/a |
| PB-37 | **CLOSED** | n/a |
| PB-38 | **CLOSED** | n/a |
| PB-39 | **CLOSED** (row already says so) | n/a |
| PB-40 | **CLOSED** | n/a |
| PB-41 | **CLOSED** | n/a |
| VL-S1 | CLOSED — closure holds, one citation drifted | n/a |
| VL-S2 | CLOSED — closure holds | n/a |
| VL-S3 | CLOSED — closure holds | n/a |
| VL-S4 | CLOSED — closure holds | n/a |
| VL-S5 | CLOSED — closure holds; **trailing 2026-09-16 paragraph is PREMISE FALSE** | n/a |
| VL-S6 | CLOSED — closure holds; its own "42-verb list" re-measure is stale | n/a |
| VL-S7 | CLOSED — **but its severity caveat is PREMISE FALSE** | no — see row |
| VL-S8 | CLOSED — closure holds | n/a |
| VL-S9 | CLOSED — closure holds, one citation drifted | n/a |
| VL-S10 | CLOSED — closure holds | n/a |
| VL-S11 | CLOSED — closure holds | n/a |
| VL-S12 | CLOSED — closure holds | n/a |
| VL-S13 | CLOSED — closure holds; **its re-measure bullet is PREMISE FALSE** | n/a |
| VL-S14 | CLOSED — closure holds | n/a |
| VL-S15 | CLOSED — closure holds | n/a |
| VL-I1 | **CLOSED** | n/a |
| VL-I2 | **CLOSED** | n/a |
| VL-I3 | **CLOSED** | n/a |
| VL-I4 | **CLOSED** | n/a |
| VL-I5 | **CLOSED** | n/a |
| VL-I6 | **CLOSED** | n/a |
| VL-P1 | **CLOSED** | n/a |
| VL-P2 | **CLOSED** | n/a |
| VL-P3 | **CLOSED** | n/a |
| VL-P4 | STILL OPEN, citations current | yes |
| VL-P5 | STILL OPEN, citations current | yes |
| VL-P6 | STILL OPEN, citations drifted | yes |
| VL-P7 | **CLOSED** | n/a |
| VL-P8 | **CLOSED** | n/a |
| VL-P9 | **CLOSED** | n/a |
| VL-P10 | **CLOSED** — **and its zero-hit premise is FALSE** | n/a |
| VL-P11 | superseded → PB-29, which is now CLOSED | n/a |
| VL-P12 | STILL OPEN, citations drifted | yes |
| VL-P13 | **PREMISE FALSE** | n/a — retire |
| VL-P14 | **CLOSED** | n/a |
| VL-P15 | STILL OPEN, citations drifted | yes |
| VL-P16 | STILL OPEN, citations drifted | yes |
| VL-P17 | STILL OPEN, citations drifted (upstream citation CORRECT at v0.84.1) | yes |
| VL-P18 | **CLOSED** | n/a |
| VL-P19 | **CLOSED** | n/a |
| VL-P20 | **CLOSED** | n/a |
| VL-P21 | **CLOSED** | n/a |
| VL-P22 | STILL OPEN, citations drifted | yes |
| VL-P23 | STILL OPEN, citations current | yes |
| VL-P24 | STILL OPEN, citations current | yes |
| VL-P25 | **CLOSED** | n/a |
| SUBA-024 (§3b) | **NARROWED** — **and §3b's zero-hit sentence is PREMISE FALSE** | drop to *low* |
| SUBA-026 (§3b) | **CLOSED** — **§3b's narrowing sentence is PREMISE FALSE** | n/a |
| SUBA-054 (§3b) | **CLOSED** | n/a |

---

## §1b arithmetic

**Current text** (`:1318-1327`):

> All **23** entries (`PB-8`…`PB-14`, `PB-31`, `VL-S1`…`VL-S15`) were re-greped against the code this
> pass. **TEN closed** — `PB-10`, `PB-11`, `PB-13`, `PB-31`, `VL-S1`, `VL-S2`, `VL-S7`, `VL-S9`,
> `VL-S14`, `VL-S15`. **ONE is partially closed and says which half** — `PB-12`. **TWELVE stay open
> with refreshed evidence** — `PB-8`, `PB-9`, `PB-14`, `VL-S3`, `VL-S4`, `VL-S5`, `VL-S6`, `VL-S8`,
> `VL-S10`, `VL-S11`, `VL-S12`, `VL-S13`. 10 + 1 + 12 = 23, which is the whole section. Closed
> entries are struck and keep their bodies as history, per this directory's id-retention rule.
> **Since that sweep, FOUR of the twelve have closed** — `PB-8`, `VL-S5`, `VL-S13`, and, on
> 2026-09-20, `VL-S3` + `VL-S4` together (they are one feature: `process-terminal.ts` imports
> `canonicalSessionId` and `inspectSessionLease`, and the proof folds the lease state into
> `sessionProjection`). Each carries its closure note in its own row; this paragraph records the
> sweep's arithmetic, not the current open set.

Two defects. (a) "**FOUR** of the twelve have closed" then names **five** rows — `PB-8`, `VL-S5`,
`VL-S13`, `VL-S3`, `VL-S4`. `VL-S3` and `VL-S4` are one *feature*; they are two *rows* of the twelve,
and the paragraph is counting rows everywhere else. (b) The paragraph has not been advanced past
2026-09-20, and **six more rows closed on 2026-09-19/21** (`PB-12`, `VL-S6`, `VL-S8`, `VL-S10`,
`VL-S11`, `VL-S12`), so a reader taking the last sentence as current sees eight open rows where
there are two.

**Corrected** (paste over `:1318-1327`):

> **SWEEP 2026-09-16 at cyrup `cc7818b`, and the reason it mattered: because area 09 left these rows
> owned HERE, nothing in the area files was ever going to correct them.** All **23** entries
> (`PB-8`…`PB-14`, `PB-31`, `VL-S1`…`VL-S15`) were re-greped against the code that pass.
> **TEN closed** — `PB-10`, `PB-11`, `PB-13`, `PB-31`, `VL-S1`, `VL-S2`, `VL-S7`, `VL-S9`, `VL-S14`,
> `VL-S15`. **ONE was partially closed and said which half** — `PB-12`. **TWELVE stayed open with
> refreshed evidence** — `PB-8`, `PB-9`, `PB-14`, `VL-S3`, `VL-S4`, `VL-S5`, `VL-S6`, `VL-S8`,
> `VL-S10`, `VL-S11`, `VL-S12`, `VL-S13`. 10 + 1 + 12 = 23, which is the whole section. Closed
> entries are struck and keep their bodies as history, per this directory's id-retention rule.
>
> **Re-derived from the rows themselves at `14e6c56` (2026-09-22), because the running total above
> had been amended four times and was wrong in both directions.** Of the twelve, **TEN have since
> closed**: `PB-8` (2026-09-18), `VL-S10` and `VL-S13` (2026-09-19), `VL-S5` (2026-09-19),
> `VL-S3` + `VL-S4` (2026-09-20 — one feature, **two rows**; the previous edition counted them as
> one and so said "FOUR" while naming five), and `VL-S6`, `VL-S8`, `VL-S11`, `VL-S12` (2026-09-21).
> `PB-12`'s partial closure became full on 2026-09-19. **The current open set of this section is
> exactly TWO rows — `PB-9` and `PB-14`** — against **TWENTY-ONE** closed. 21 + 2 = 23, which is
> still the whole section. Both survivors were re-greped at `14e6c56`; both are genuinely open and
> both carry drifted cyrup addresses, refreshed in their own rows.

---

## PB-6 · Changelog-on-upgrade is absent; `lastChangelogVersion` is never read or written

**Verdict:** STILL OPEN, citations drifted.

**Evidence**

```
$ grep -rn 'last_changelog_version\|lastChangelogVersion' crates/ --include=*.rs
crates/cyrup-config/src/settings/effective.rs:705:    /// `lastChangelogVersion` (Pi `getLastChangelogVersion`, :655-657).
crates/cyrup-config/src/settings/effective.rs:706:    pub fn last_changelog_version(&self) -> Option<String> {
crates/cyrup-config/src/settings/effective.rs:707:        self.merged.get_str("lastChangelogVersion")
```

Three hits, all inside the getter's own definition: the zero-caller claim holds, and no setter
exists (`grep -rn 'set_last_changelog' crates/` → 0). `settings.rs` no longer exists as a monolith —
the file was split into `cyrup-config/src/settings/`.

```
$ grep -rn 'changelog' crates/cyrup-tui/src/app/submit.rs
130:            "changelog" => {
133:                    .push_block("What's New", "No changelog entries found.");
$ grep -n 'collapse' crates/cyrup-tui/src/app/settings_rows.rs
181:        SettingRow::toggle("collapseChangelog", "Collapse changelog", eff.collapse_changelog())
```

The `enableInstallTelemetry` counter-example the row draws still holds:
`cyrup-config/src/policy.rs:27` and `cyrup-session-svc/src/builder.rs:1779` both consume it.

**Replacement text** (the `- cyrup:` and `- observable:` bullets):

```markdown
- cyrup: `crates/cyrup-config/src/settings/effective.rs:705-707` (`last_changelog_version`) has zero callers workspace-wide and no setter exists; `/changelog` is hardcoded at `crates/cyrup-tui/src/app/submit.rs:130-133` to `push_block("What's New", "No changelog entries found.")` *(was `settings.rs:994` / `submit.rs:111-113`; the settings monolith was split into `cyrup-config/src/settings/`, re-resolved by symbol 2026-09-22)*
- observable: after upgrading, pi shows the new entries once and records the version; cyrup shows nothing. The `collapseChangelog` settings row (`app/settings_rows.rs:181`) toggles a value nothing reads. (`enableInstallTelemetry`, the row beside it, **does** have live consumers — `cyrup-config/src/policy.rs:27`, `cyrup-session-svc/src/builder.rs:1779` — so it is not part of this claim.)
```

---

## PB-7 · The npm package channel is unported, and `npmCommand` is inert

**Verdict:** STILL OPEN, citations drifted. The refusal *message* the row quotes is also gone —
the misleading "unsupported source (OCI deferred)" string has been replaced by a typed
`ResourceError::UnsupportedNpm`, so the row's parenthetical no longer describes the code.

**Evidence**

```
$ sed -n '78,82p' crates/cyrup-resources/src/package/source.rs
        if trimmed.starts_with("npm:") {
            // npm channel dropped in the Rust port (R-09-021): no JS runtime.
            return Err(ResourceError::UnsupportedNpm);
        }
$ grep -rn 'UnsupportedNpm' crates/cyrup-resources/src/
crates/cyrup-resources/src/error.rs:45:    UnsupportedNpm,
crates/cyrup-resources/src/package/source.rs:81:            return Err(ResourceError::UnsupportedNpm);
crates/cyrup-resources/src/tests/resources/git_url.rs:158: (test)
$ grep -rn 'npm_command\|npmCommand' crates/ --include=*.rs
crates/cyrup-config/src/settings/effective.rs:373-376   (definition)
crates/cyrup-config/src/settings/tests/merge_and_scope.rs:463-481  (tests only)
```

`npm_command` still has zero production callers.

**Replacement text** (the `- cyrup:` bullet):

```markdown
- cyrup: `crates/cyrup-resources/src/package/source.rs:79-81` returns `Err(ResourceError::UnsupportedNpm)` for any `npm:` spec **(the misleading "unsupported source (OCI deferred)" message area 05 `CFG-009` recorded is gone — the arm is now typed and its comment names R-09-021; the channel is still absent)**; `crates/cyrup-config/src/settings/effective.rs:373-376` (`npm_command`) has zero production callers anywhere — its only references are `settings/tests/merge_and_scope.rs:463-481`
```

---

## PB-9 · `clarify: true` is advertised but shows no preview/edit UI

**Verdict:** STILL OPEN, citations drifted. The load-bearing zero-hit holds.

**Evidence**

```
$ grep -rn 'ChainClarify\|chain_clarify' crates/cyrup-ext-subagents/src/
(no output)
```

Every surviving `clarify` hit in the crate is a **different** feature — the `contact_supervisor`
supervisor-clarify intercom ask (`exec/drive_attempt.rs:370-374` → `tui::intercom::spawn_clarify`,
`exec::RunOptions::clarify`) — and must not be mistaken for this row's UI.

The three dead `extension.rs` addresses now resolve:

```
$ grep -rn '"clarify"' crates/cyrup-ext-subagents/src/
extension/tool/schema.rs:556:  props.insert("clarify".to_string(), … "Show TUI to preview/edit before execution. …"
extension/tool/params.rs:782
extension/host/native_impl.rs:1100
$ sed -n '1098,1102p' crates/cyrup-ext-subagents/src/extension/host/native_impl.rs
    // `:475` — the `[async]` badge, suppressed while clarifying.
    … args.get("clarify") != Some(&serde_json::Value::Bool(true))
        " [async]"
$ grep -n 'open_overlay' crates/cyrup-ext-subagents/src/extension/host/slash.rs
147:   .is_some_and(|services| services.open_overlay(Box::new(overlay)));
```

**Replacement text** (the first bullet and the `- cyrup:` bullet):

```markdown
- **Re-greped 2026-09-22 at `14e6c56`:** `grep -rn 'ChainClarify\|chain_clarify' crates/cyrup-ext-subagents/src/` still returns **0**. No clarify UI exists. **Do not read the crate's other `clarify` hits as this feature** — `exec/drive_attempt.rs:370-374`, `exec/RunOptions::clarify` and `tui::intercom::spawn_clarify` are the `contact_supervisor` supervisor-clarify ask (R-SA-037), a different mechanism that shares the word. The dead `extension.rs` citations are re-resolved below.
- cyrup: `extension/tool/schema.rs:556` declares the param with the description "Show TUI to preview/edit before execution. Explicit clarify: true keeps the run foreground for the clarify UI; omitted clarify can still run in the background when async: true is set."; the flag is read at `extension/host/native_impl.rs:1098-1102` (the async→foreground downgrade and the `[async]`-badge suppression, now one site) and listed at `extension/tool/params.rs:782` — **no read produces a UI**
- observable: cyrup accepts `clarify: true`, forces the run foreground, and launches immediately with the model's unmodified prompt. The tool description promises a UI that does not exist. The seam it needs is live: `HostServices::open_overlay` is already consumed in production by this same crate at `extension/host/slash.rs:147`.
```

---

## PB-14 · The "skills not found" warning is unported on BOTH surfaces

**Verdict:** STILL OPEN, citations drifted. One sub-claim has gone false: the stale deferral note
the row quotes from `discovery/management.rs:1276-1277` no longer exists.

**Evidence**

```
$ grep -rn 'skills_warning\|skillsWarning' crates/cyrup-ext-subagents/src/
crates/cyrup-ext-subagents/src/artifacts.rs:544:/// `durationMs`/`skills`/`skillsWarning`, which `SingleResult` does not carry in this crate (they
$ grep -rn 'entirely absent today' crates/cyrup-ext-subagents/src/
(no output)
```

Exactly one hit, and it is still a confession rather than a port. The run-side discard is now at
`exec/mod.rs:1155-1170`: `resolve_skills_with_fallback` is called at `:1155`, and `resolution.missing`
is read at `:1167` **for one purpose only** — the orchestration-skill hard failure — and then dropped.

```
$ grep -n 'resolution.missing\|\.missing' crates/cyrup-ext-subagents/src/exec/mod.rs
1167:            .missing
```

Management side: `discovery/skills.rs:152` (`resolve_skills`) still has no caller outside its own
module; production goes through `resolve_skills_with_fallback` (`:195`).

**Replacement text** (the first bullet and the `- cyrup:` bullet):

```markdown
- **Re-greped 2026-09-22 at `14e6c56`:** `grep -rn 'skills_warning\|skillsWarning' crates/cyrup-ext-subagents/src/` returns exactly **one** hit and it is a confession, not a port — `artifacts.rs:544` documents `skillsWarning` as one of the fields *"which `SingleResult` does not carry in this crate"*. Both surfaces stay unported. **One sub-claim below has gone false and is corrected in place**: the stale deferral note that said the skills subsystem is "entirely absent today" is gone (`grep -rn 'entirely absent today' crates/cyrup-ext-subagents/src/` → 0), and `discovery/management.rs` no longer exists as a monolith
- cyrup: `exec/mod.rs:1155` calls `resolve_skills_with_fallback` and reads `resolution.missing` at `:1167` **for exactly one purpose** — pi's orchestration-skill hard failure (`execution.ts:938-946`) — then discards it; `SingleResult` has no `skills_warning` field and `artifacts.rs:544` documents omitting it. On the management side `discovery/skills.rs:152` (`resolve_skills`) still has zero callers outside its own module — production reaches the fallback form at `:195` instead
- observable: `subagent({action:"create", config:{skills:"typo"}})` reports success with no warning, **and** a run with the same typo produces no warning either. (The `Skills not found:` string at `exec/mod.rs:1172` is a different thing: the hard failure for a missing *orchestration* skill, exit 1.)
```

---

## PB-1 / PB-2 / PB-3 / PB-4 / PB-5 — all CLOSED

**PB-1 · `radius` not registered — CLOSED.**
`crates/cyrup-provider/src/providers/all.rs:260` pushes `radius_provider_with(…)` (comment at
`:257` cites `all.ts:121` @v0.84.4 / `:117` @v0.83.0); `env_api_keys.rs:71` maps
`"radius" => RADIUS_API_KEY`. Upstream verified: `git -C tmp/pi show v0.83.0:packages/ai/src/providers/all.ts`
line 117 is `radiusProvider(),`.

**PB-2 · `qwen-token-plan*` not registered — CLOSED.**
Registered as fleet providers with dynamic catalogs (`all.rs:39-41`, the `openai-completions` fleet
at `:250-251`); `env_api_keys.rs:53-59` maps all three keys including the v0.84.x
`qwen-token-plan-individual`.

**PB-3 · `Models::refresh` has no options/result — CLOSED.**
`crates/cyrup-provider/src/collection.rs:431` is `refresh_with(&self, provider, ModelsRefreshOptions)
-> ModelsRefreshResult`, with `allow_network`/`force`/`cancel` in and `aborted`/`errors` out, and a
clause-by-clause table against `models.ts:276-328` @v0.83.0 in its doc. `refresh` (`:509`) survives
as the compatibility shape and says so.

**PB-4 · No `docs` arm in compact-read classification — CLOSED.**
`crates/cyrup-tui/src/transcript/tool_args.rs:521` `compact_read_classification` runs all three arms
in upstream's order, with `docs_classification` (`:468`, `CompactReadKind::Docs` at `:446`/`:480`)
between `SKILL.md` and `COMPACT_RESOURCE_FILE_NAMES`. The `getReadmePath`-has-no-counterpart blocker
and OQ-2 are discharged.

**PB-5 · `PI_CODING_AGENT`/`AI_AGENT` never stamped — CLOSED.**
`crates/cyrup-tools/src/tools/bash.rs:309` pushes `CYRUP_CODING_AGENT=true` and `:321` pushes
`AI_AGENT=cyrup` into every bash child; `cyrup-ext-subagents/src/exec/spawn_plan.rs:913`/`:923` do
the same for every re-exec'd subagent child, with the hard-rename `[CYRUP-DELTA]` recorded at
`:911-923`. Both are pinned by `spawn_plan.rs:4193-4243`.

---

## PB-22 … PB-41 — all CLOSED

Proof per row, one line each (every address re-greped at `14e6c56`):

* **PB-22** `crates/cyrup-provider/src/api/google_vertex.rs` exists; `all.rs:88-93` records the
  residual as closed by it and the self-contradicting port-status table the row demanded be
  rewritten now reads `| 103 | google-vertex | ✓ |` (`:24`).
* **PB-23** `api/anthropic_messages/claude_code.rs:56-76` `resolve_is_oauth` branches on
  `model.provider == GITHUB_COPILOT_PROVIDER` **before** the `sk-ant-oat` test, exactly as
  `anthropic-messages.ts:867-888` does.
* **PB-24** `api/github_copilot_headers.rs` exists (`X_INITIATOR`, `COPILOT_VISION_REQUEST`,
  `build_copilot_dynamic_headers`, `apply_copilot_dynamic_headers`) and is applied on **all three**
  routes: `anthropic_messages/headers.rs:151`, `openai_completions/headers.rs:71`,
  `openai_responses/headers.rs:52`.
* **PB-25** `cyrup-agent/src/agent/lifecycle.rs:191-233`: the fast-path guard is hoisted **above**
  both drains (`:193-196`) and each drain requeues with `push_front` on `Err` (`:212`, `:230`).
  Both halves the row required, in one function. `agent.rs` no longer exists.
* **PB-26** `cyrup-session-svc/src/session/mod.rs:493` `is_run_active()` = `!is_idle()` =
  `driver_tx || agent.is_running()`, and it is what `prompt_run` (`session/run.rs:122`) and
  `prepare` (`:480`) consult, both citing AGENT-030.
* **PB-27** `cyrup-resources/src/discovery/mod.rs:387` `discover_system_prompt_file` and `:479`
  `discover_append_system_prompt_file`, called from production at
  `cyrup-session-svc/src/builder.rs:1611`/`:1621` and from `discovery/blocking.rs:116`.
* **PB-28** `tree_selector.rs:394` `search_query` with the `Type to search:` line (`:983-990`),
  printable keys appended at `:1231`, and `:290` records that `from_digit` was deleted and the
  filter modes moved to the seven `app.tree.filter.*` chords.
* **PB-29** already struck in-row; re-verified at `app/run_action.rs:107` and `:154`.
* **PB-30** `cyrup/src/signals.rs` now runs `kill_tracked_detached_children()` (`:287`),
  `runtime.dispose().await` (`:324`) and `process::exit` with pi's codes on the **first** delivery
  (`first_delivery_exit_code`, `:224`), with the repeat force-exit at `:304`.
* **PB-33** `cyrup-tui/src/editor/mod.rs:110-115` — `Snapshot` carries `pastes` (and
  `paste_counter`), documented against `editor.ts:218`.
* **PB-34** `editor/motion.rs:141` `word_left_target` / `:179` `word_right_target` segment through
  `word_segments`, which marks a whole paste marker `atomic: true` (`:110`), porting
  `word-navigation.ts`'s `isAtomicSegment`; `edit.rs:114`/`:126` delete to those targets.
* **PB-35** `cyrup-provider/src/lib.rs:165-166` exports `build_client_for`,
  `build_client_for_target`, `build_client_with_proxy`, `configure_http_proxy`; all five OAuth
  flows, `cyrup-agent/src/proxy/transport.rs:58-61` and `cyrup-ext/src/caps/http.rs:139-148` are on
  the per-target resolver, each citing PROV-047.
* **PB-36** `cyrup-core/src/json.rs:164-183` — `repair_json`'s `Some('u')` branch now drops an
  unpaired high or low surrogate escape (and keeps a paired run verbatim), so `repaired != json` and
  the retry succeeds.
* **PB-37** `cyrup/src/startup_ui.rs:218` calls `selector.set_all_rows(…)` and `:221-226`
  `set_session_cwds(…)`; `session_selector.rs:322` `toggle_scope` is the live Tab handler.
* **PB-38** `startup_ui.rs:241-245` — `run_resume_picker`'s `on_apply` now matches
  `SessionSelectorOutcome::Rename` and calls `cyrup_session_svc::rename_session_file_at`.
* **PB-39** already struck in-row; the pre-launch half the row flagged as the part to re-check is
  also done — `startup_ui.rs:233-238` goes through `delete_session_file_at` and prints
  `method.status_message()` / `Failed to delete: {e}`.
* **PB-40** `cyrup-config/src/trust.rs:387` `trust_options(cwd, include_session_only)` with both
  ephemeral rows at `:407-424`; `startup_ui.rs:759` records that the pre-launch path now asks for
  `includeSessionOnly: true`.
* **PB-41** `cyrup/src/prelaunch.rs:214-226` states in-tree that project trust is **not** resolved
  pre-launch any more, and `trust_prompt_callback` (`:237`) is handed to the builder, which invokes
  it only on `TrustOutcome::NeedsPrompt` — i.e. after `pre_trust_extension_verdict`
  (`cyrup-session-svc/src/builder.rs:783-788`) and the store. pi's tier order is restored.

---

## PB-15 … PB-18, PB-32 (§1c) — all CLOSED

* **PB-32** `crates/cyrup-permission-system/src/extension/agent_start.rs:200-222`
  `should_expose_tool` has **one** bypass (`read` + allowed skills, `:217`) and the doc at
  `:183-198` records the bash arm's deletion, names it a live permission bypass, and cites the
  reproduction in `REPRO-LOG.md §PERM-009`.
* **PB-15** `cyrup-provider/src/api/compat.rs:868` `unsupported_temperature_reason` with pi's three
  reasons (api / provider / model), consumed as the gate on every `temperature` insert (`:825`,
  `:904`). `grep -rn "does not support temperature" crates/` is no longer 0.
* **PB-16** `cyrup-permission-system/src/extension/consts.rs:31`
  `PERMISSION_REQUEST_EVENT_CHANNEL = "cyrup-permission-system:permission-request"` and
  `extension/events.rs` is "the two event-bus publications"; `cyrup-ext/src/native.rs` now carries
  15 `bus` references where the row measured zero.
* **PB-17** already struck in-row. Re-verified: `forwarding.rs` calls `audit.review(…)` at `:736`,
  `:795`, `:848`, `:1070`, `:1148`, `:1171`, `:1182` — the row's "grep returns zero" is long gone.
* **PB-18** `cyrup-permission-system/src/config_modal.rs` is the PERM-007 port of
  `config-modal.ts:63-122` with `ConfigController` and `PermissionSystemSettingsOverlay`.

---

## PB-19 … PB-21, VL-I1 … VL-I6 (§1d) — all CLOSED

* **PB-19** `broker/lifecycle.rs:118` calls `transport::target::broker_listen_target(&agent_dir)`;
  `broker/listener.rs` binds `Tcp` (`:77`) or `Unix` (`:105`) off it and publishes the chosen port
  (`:73`, `:288`). The "zero callers of any kind" claim is refuted.
* **PB-20** `crates/cyrup-intercom/resources/skills/pi-intercom/SKILL.md` ships (the row's
  `find … ! -name '*.rs'` now returns it), discovered through `resources.rs` and answered on
  `EventKind::ResourcesDiscover` (`resources.rs:17`). The row's own instruction was followed —
  `resources.rs:2` records it as the **v0.10.1** text.
* **PB-21** `identity.rs:153` `name_poll_ms()` has a production caller at `session_state.rs:574`,
  which builds the poll interval; presence goes out through `update_presence_full`
  (`session_state.rs:720`).
* **VL-I1** `broker/mailbox.rs` + `broker/state.rs:82-84` (`mailbox_messages`,
  `queue_mailbox_message`, `flush_mailbox_for_session` on re-register). `connect.rs:47-50` records
  that the row's "there is no mailbox, no queue, no redelivery" was true of the v0.7.0 shape only.
* **VL-I2** `ui/inline_message.rs:295` `format_inbound_delivery_metadata` is rendered into the
  injected body at `:113`; `MessageReceipt` / `MessageReceiptStatus` are live in
  `session_state.rs:20`; the dedupe window is documented at `connect.rs:344`.
* **VL-I3** `tools/intercom/mod.rs:431` advertises the eight-action enum including `cancel`,
  dispatched at `:308`; `supersedes`/`retry_of` ride the envelope (`session_state.rs:976-977`) and
  render at `inline_message.rs:302-306`; the broker's `handle_cancel_message`
  (`broker/receipts.rs:88`) resolves a real session key instead of always refusing.
* **VL-I4** `broker/extension_state.rs` (`ExtensionStateManager`, held at `broker/state.rs:140`),
  `EXTENSION_BUS_FEATURE` advertised on `registered` and `extension_state_commit` dispatched
  (`broker/dispatch.rs:104-114`).
* **VL-I5** `connect.rs:655-665` resolves `ENV_INTERCOM_STABLE_ID` then `config.stable_id`, with
  pi's falsy-`||` semantics spelled out. `grep -rni 'stable_id'` is no longer zero.
* **VL-I6** `crates/cyrup-intercom/src/cwd.rs` exists (`normalize_cwd:38`, `same_cwd:60`) and
  `"list-cwd"` is advertised (`tools/intercom/mod.rs:431`) and dispatched (`:307`).

---

## VL-S1 … VL-S15 — closures verified; four notes need correcting

Every one of the fifteen closures still greps. Four rows carry statements that have since gone
false, and those are worth more than the confirmations.

### VL-S1 — CLOSED, one citation drifted

`exec/capability_ceiling.rs:68` declares `CAPABILITY_CEILING_ENV`. The env WRITE is at
`exec/spawn_plan.rs:1221`, not `:557`; the read-back guard is at `:1493`.
`background/scheduled_runs/ceiling_gate.rs:37-39` is the second consumer, as claimed.

**Replacement text** (closure bullet, one clause):

```markdown
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `exec/capability_ceiling.rs` exists and `CAPABILITY_CEILING_ENV = "CYRUP_SUBAGENT_CAPABILITY_CEILING_V1"` is declared at `:68` and **written into the child env** at `exec/spawn_plan.rs:1221` *(was `:557`)*, with the survive-the-re-exec guard at `:1493`. The `exec/mod.rs:1428` comment this row quotes ("no capability ceiling in this port") no longer exists. A second consumer landed with the SCOPE sequence: `background/scheduled_runs/ceiling_gate.rs:37-39` makes the ceiling a precondition for persisting a schedule. **The body below is the original filing and is kept as history.**
```

### VL-S5 — CLOSED, but the row's trailing 2026-09-16 paragraph is PREMISE FALSE

**The false sentence** (the bullet beginning *"Re-greped this pass, and cyrup now says so in its own
source"*):

> `background/async_retention/scan.rs:56` defines `RECOVERY_DESCRIPTOR_FILE = "recovery-descriptor.json"`
> as a *reader* … and `:381` carries the explicit `[CYRUP-DELTA] no cyrup writer produces
> recovery-descriptor.json today`. **The read half now exists and the write half still does not** —
> which is strictly worse than before…

**The truth:** the writer landed on 2026-09-19 and the row's own closure header says so, but the
superseded paragraph was left un-marked, so a reader who scrolls past the header meets a flat
contradiction.

```
$ grep -n 'RECOVERY_DESCRIPTOR_FILE\|recovery-descriptor.json\|CYRUP-DELTA' \
    crates/cyrup-ext-subagents/src/background/async_retention/scan.rs
56:/// [CYRUP-DELTA] upstream's discovery worker takes only `asyncDirRoot` because its `runSkipReason`
```

Neither the constant nor the no-writer delta is at those lines any more. The writer is at
`extension/executor/background.rs:808`
(`RunDir::for_existing(&run_paths.run_dir).recovery_descriptor()`).

**Replacement text** (replace that whole bullet):

```markdown
- ~~**Re-greped 2026-09-16, kept as history and now WRONG:** *"`background/async_retention/scan.rs:56` defines `RECOVERY_DESCRIPTOR_FILE` as a reader … `:381` carries the explicit `[CYRUP-DELTA] no cyrup writer produces recovery-descriptor.json today` … the read half now exists and the write half still does not."*~~ — **false at `14e6c56`.** The writer landed 2026-09-19 (`extension/executor/background.rs:808`), and with it both quoted lines were deleted: `grep -n 'RECOVERY_DESCRIPTOR_FILE\|CYRUP-DELTA' background/async_retention/scan.rs` now shows `:56` carrying an unrelated discovery-worker note and nothing at `:381`. Struck rather than deleted so the next reader does not re-derive the contradiction. The `extension.rs:4269-4285` citation below is dead.
```

### VL-S7 — CLOSED, and its severity caveat is PREMISE FALSE

**The false sentence** (closure bullet):

> **The scope caveat this row records STILL HOLDS and is the reason it was rated medium:**
> `worktree.discard` and `worktree.cleanup` are among the seventeen verbs cyrup's action list is
> still missing, so those arms have nothing to attach to.

**The truth:** both verbs are advertised **and** `worktree.discard` is gated through this very
policy. The condition the row named as the trigger for raising it to critical has already occurred.

```
$ awk '/^pub\(crate\) const SUBAGENT_ACTIONS/,/^\];/' \
    crates/cyrup-ext-subagents/src/extension/tool/text.rs | grep -c '^\s*"'
59
$ … | grep -o 'worktree\.[a-z]*'
worktree.discard
worktree.cleanup
$ sed -n '109,138p' crates/cyrup-ext-subagents/src/registration/authority.rs
    pub fn for_tool_action(action: &str) -> Option<Self> {
        …
            "worktree.discard" => Some(Self::DiscardWorktree),
```

`worktree.cleanup` returns `None` **deliberately** — `authority.rs:118-119` records that upstream
gates it on nothing and it is plan-only — so that half is parity, not a hole.

**Replacement text** (closure bullet):

```markdown
- **CLOSED, re-verified in code 2026-09-22 at `14e6c56`.** `registration/authority.rs` is the port and names the upstream file in its own header; `validate_authority_policy` exists; the gate is consulted from the live `stop`/`steer` path, from `schedule.create` (`extension/tool/routing.rs:1268-1275` → `AuthorityAction::for_tool_action`, mapping at `registration/authority.rs:113`), and — **since LANES_2 and VL-S6** — from `worktree.discard` (`:120`), `inspector.open` (`:136`) and `project.open` (`:137`). **The scope caveat this row used to carry is now FALSE and is struck:** ~~"`worktree.discard` and `worktree.cleanup` are among the seventeen verbs cyrup's action list is still missing, so those arms have nothing to attach to"~~ — `SUBAGENT_ACTIONS` is **59** verbs (`extension/tool/text.rs:234`) and both `worktree.*` verbs are in it. `worktree.discard` arrived already gated; `worktree.cleanup` returns `None` **deliberately**, because upstream gates it on nothing and it is plan-only (`authority.rs:118-119`). **The "becomes critical the day `worktree.discard` lands" clause is therefore spent — that day came and the gate was already there**, which is why this closure holds at *medium* rather than reopening. **The body below is the original filing and is kept as history.**
```

### VL-S13 — CLOSED, and its re-measure bullet is PREMISE FALSE

**The false sentence:**

> **Re-measured this pass against the verb set:** `refine`, `refine.show` and `refine.rollback` are
> three of the seventeen verbs absent from cyrup's 42-verb action list (`extension/tool/text.rs:215`)
> — so the "no `refine*` verb in the enum at `extension.rs:6557`" claim is still TRUE, at a new
> address.

**The truth:** all three are advertised, and the list is 59 verbs, not 42.

```
$ awk '/^pub\(crate\) const SUBAGENT_ACTIONS/,/^\];/' extension/tool/text.rs | grep '^\s*"' | tr -d ' "'
… lane.recordSupersession, refine, refine.show, refine.rollback, inspector.open, …
$ sed -n '234,238p' crates/cyrup-ext-subagents/src/extension/tool/text.rs
/// As of VL-S6 this list names all **57** verbs of upstream's `SUBAGENT_ACTIONS`
/// (`shared/types.ts:2801` @v0.68.0) plus **2** cyrup dispatches that upstream does not advertise
/// there — `append-step` and `inspect` — for **59**.
```

The same stale "42-verb list" figure appears in **VL-S6**'s history bullet, in **PB-11**'s closure
note and in **§3b**'s `SUBA-055` clause. All four were written against a list that has since
absorbed the seventeen.

**Replacement text** (replace that bullet):

```markdown
- ~~**Re-measured 2026-09-16 against the verb set:** *"`refine`, `refine.show` and `refine.rollback` are three of the seventeen verbs absent from cyrup's 42-verb action list"*~~ — **false at `14e6c56`, in both halves.** All three verbs are advertised, and the list is **59**, not 42: `extension/tool/text.rs:234` states it "names all **57** verbs of upstream's `SUBAGENT_ACTIONS` (`shared/types.ts:2801` @v0.68.0) plus **2** cyrup dispatches that upstream does not advertise there — `append-step` and `inspect`". **The "42-verb list" figure is stale wherever it appears** — VL-S6's history bullet, PB-11's closure note and §3b's `SUBA-055` clause all carry it; enumerate `SUBAGENT_ACTIONS` rather than quoting the number. The read-half-only statement at `exec/agent_refinements.rs:12-20` was not re-resolved this pass.
```

### VL-S2, VL-S3, VL-S4, VL-S6, VL-S8, VL-S9, VL-S10, VL-S11, VL-S12, VL-S14, VL-S15 — closures hold

One proof each:

* **VL-S2** `workflows/scripted/` exists; `missions/workflow_state.rs` implements the store.
* **VL-S3** `background/session_lease/` (directory present, 7 modules).
* **VL-S4** `background/process_terminal/` (directory present, 8 modules).
* **VL-S6** the seven verbs are in `SUBAGENT_ACTIONS` and dispatched at
  `extension/tool/routing.rs:1544-1624`; `crates/cyrup-herdr` exists.
* **VL-S8** `extension/wait_tool.rs:24` `WAIT_TOOL_NAME = "bg_wait"`; every self-reference in the
  description splices the const (`:64`, `:76`, `:159`).
* **VL-S9** `exec/usage_budget.rs` exists; the budget rides `RunnerConfig` at
  `extension/executor/background.rs:71`/`:271`/`:453` *(the row's `:620` has drifted — same clause,
  new offsets)*.
* **VL-S10** `handoff/` (9 modules) exists; `background/async_retention/scan.rs:276`/`:426`
  `has_unresolved_run_handoff` reads real files.
* **VL-S11 + VL-S12** `registration/slash_commands.rs:95` `SlashCommandName` holds the eighteen
  including `Subagents`(`:140`), `SubagentsDetach`(`:147`), `SubagentsSteer`(`:152`),
  `SubagentsInspectRpc`(`:157`), `SubagentsRefine`(`:135`); and
  `git grep -n 'SlashCommandName::\(Chain\|Parallel\|RunChain\|ChainPrompts\)' -- crates/` is empty.
* **VL-S14** `exec/external_cli/` (`mod/run/env/framing/preflight/prompt` + `adapters/`).
* **VL-S15** `cyrup-ext/src/native.rs:406` `register_shortcut`, carried at `facade.rs:502`,
  dispatched at `native.rs:651`.

---

## SUBA-024 (§3b) · `parallel-handoff` / `agent-contract`

**Verdict:** **NARROWED** — and the sentence §3b uses to keep it open is **PREMISE FALSE**.

**The false sentence** (§3b, the *low* bullet):

> **`SUBA-024` (`parallel-handoff` / `agent-contract`) — STILL OPEN**, `"handoffPath"` is still
> zero-hit (§1b VL-S10)

**The truth:** `handoffPath` has production hits, and VL-S10 — the row it cross-references — was
**closed 2026-09-19**, which makes the citation self-refuting.

```
$ grep -rn 'handoffPath\|handoff_path' crates/ --include=*.rs | head
crates/cyrup-ext-subagents/src/spawn/cleanup_plan/metadata.rs:96:  fn discover_handoff_paths(
crates/cyrup-ext-subagents/src/spawn/cleanup_plan/model.rs:349:   pub handoff_path: Option<PathBuf>,
crates/cyrup-ext-subagents/src/spawn/cleanup_plan/mod.rs:83-85,168
crates/cyrup-ext-subagents/src/background/runner_main/status.rs:341,344
crates/cyrup-ext-subagents/src/background/records.rs:423-426
```

The **agent-contract** half is genuinely still open, and cyrup says so in its own source:

```
$ sed -n '2316,2321p' crates/cyrup-ext-subagents/src/spawn/chain_graph.rs
// G78 — `reportOptional: isAgentContractV1(step.agentContract ?? params.agentContract)`
// … this crate has no agent-contract concept at all yet (`agent-contract.ts` is unported),
// so no cyrup step can declare one and the predicate is `false` for every run.
report_optional: false,
```

**Severity:** the row's *low* was set for two halves; one is gone. It should stay *low* but be
re-scoped — the remaining consequence is a single hard-coded `report_optional: false`.

**Replacement text** (the `SUBA-024` clause of §3b's *low* bullet):

```markdown
**`SUBA-024` (`parallel-handoff` / `agent-contract`) — NARROWED 2026-09-22, still open for its agent-contract half only**: ~~`"handoffPath"` is still zero-hit (§1b VL-S10)~~ — **false**, and self-refuting: `VL-S10` closed 2026-09-19 and `handoff/` (9 modules) is the writer, so `handoffPath` now has production hits (`background/records.rs:423-426`, `background/runner_main/status.rs:341-344`, `spawn/cleanup_plan/{mod,model,metadata}.rs`). What remains is `agent-contract.ts`, unported: `spawn/chain_graph.rs:2316-2321` records that "this crate has no agent-contract concept at all yet … the predicate is `false` for every run", so `reportOptional` is a hard-coded `false`. That is the whole residual and it keeps the row at *low*
```

---

## SUBA-026 (§3b) · interactive admin UI and selector

**Verdict:** **CLOSED.** §3b's narrowing sentence is **PREMISE FALSE** — it was written in the same
commit (`cbdb27a`) that landed the thing it says keeps the row open.

**The false sentence:**

> What keeps `SUBA-026` open is the interactive admin UI and the selector — `src/slash/selector.ts`,
> 147 L @v0.68.0, NOT `src/tui/selector.ts`, which exists at no tag

**The truth.** The upstream-path half is right and stands (verified below). The *cyrup* half is not:
the interactive admin UI is ported, and the selector is a documented deliberate delta, not a gap.

```
$ git -C tmp/pi-subagents cat-file -e v0.68.0:src/slash/selector.ts && echo EXISTS
EXISTS
$ for t in $(git -C tmp/pi-subagents tag); do \
    git -C tmp/pi-subagents cat-file -e $t:src/tui/selector.ts 2>/dev/null && echo "FOUND at $t"; done
(no output — src/tui/selector.ts exists at NO tag, as the row says)

$ sed -n '1,13p' crates/cyrup-ext-subagents/src/extension/host/slash_admin.rs
//! `/subagents` — the admin surface (pi `src/slash/subagents-admin.ts`, 460 lines, registered at
//! `slash-commands.ts:869-875`).
//! … the no-UI branch (upstream's `metadataFor` text dump) and the interactive branch (upstream's
//! `selectAgent` → `chooseModel`/`chooseThinking`/`editSystemPrompt` → `persistSettingsField`
//! loop) both hang off this one entry point, exactly as `openSubagentsAdmin` does.
$ grep -n 'selectAgent\|pick_from' crates/cyrup-ext-subagents/src/registration/subagents_admin.rs
1206:/// pi `selectAgent` (`subagents-admin.ts:162-183`).
1207:pub(crate) fn select_agent(
$ sed -n '31,36p' crates/cyrup-ext-subagents/src/registration/subagents_admin.rs
//! 3. **`selectFromList` (`:216`) takes upstream's `ctx.ui.select` branch, never `ctx.ui.custom`.**
//!    Upstream picks between them on `typeof ctx.ui.custom === "function"` — a test cyrup cannot
//!    make: `HostServices::custom` is always present and answers `Option<String>`, whose `None`
//!    cannot be told apart from a dismissal … `ctx.ui.select` is upstream's own documented fallback
//!    (`:224-227`)
$ git log --oneline -1 -- crates/cyrup-ext-subagents/src/registration/subagents_admin.rs
cbdb27a feat(subagents): the slash surface — 18 of 18 commands, and the wait tool's real name
$ git log --oneline -1 -- docs/gap-analysis/PARITY-GAPS.md
cbdb27a  (the same commit)
```

So the sentence was stale the moment it was written: the batch that landed `/subagents` also wrote
the line saying `/subagents` is what keeps the row open.

**Replacement text** (the `SUBA-026` clause of §3b's *low* bullet):

```markdown
~~**`SUBA-026` (interactive admin UI and selector)**~~ — **CLOSED 2026-09-22.** The three slash commands landed with §1b `VL-S11` and the table is **18** at `registration/slash_commands.rs:228` (the "17-variant match at `:83-121`" was stale on count AND line). **The narrowing this row carried was stale in the same commit that wrote it (`cbdb27a`)**: `/subagents` — the interactive admin surface — is `extension/host/slash_admin.rs:26` over `registration/subagents_admin.rs`, which ports `openSubagentsAdmin` (`subagents-admin.ts:396`) including `selectAgent` (`:1206`) and the `chooseModel`/`chooseThinking`/`editSystemPrompt` loop, with the no-UI `metadataFor` branch. The remaining half — `src/slash/selector.ts`, 147 L @v0.68.0 (**NOT** `src/tui/selector.ts`, which `git cat-file -e <tag>:src/tui/selector.ts` proves exists at no tag; keep that correction) — is **not a gap but a recorded `[CYRUP-DELTA]`**: `subagents_admin.rs:31-36` states that `selectFromList` deliberately takes upstream's own documented `ctx.ui.select` fallback (`:224-227`) because `HostServices::custom` is always present and its `None` cannot be told apart from a dismissal. Preferring the overlay would turn "this host paints no custom overlay" into a spurious cancel
```

---

## SUBA-054 (§3b) · `defaultReads` never reaches a single run — CLOSED

**Verdict:** **CLOSED.**

```
$ grep -rn 'reads:' crates/cyrup-ext-subagents/src/extension/executor/foreground.rs
1178:            reads: agent.default_reads.clone(),
$ sed -n '1414,1420p' crates/cyrup-ext-subagents/src/exec/spawn_plan.rs
    // SUBA-054 / pi `task = readsInstruction + task` (`subagent-executor.ts:3873`), which runs
    // BEFORE `injectSingleOutputInstruction` (`:3874`) …
    let reads_instruction = opts.reads.as_deref().map_or_else(String::new, |reads| {
        crate::spawn::chain_graph::build_single_reads_instruction(reads, &opts.cwd)
    });
$ sed -n '440,453p' crates/cyrup-ext-subagents/src/exec/agent_config.rs
    /// SUBA-054 — the run's declared read paths, pi's `reads` binding at
    /// `runs/foreground/subagent-executor.ts:3869` …
    /// Before this existed, `defaultReads` was parsed off frontmatter and rendered in agent
    /// listings but never reached a run …
```

Producer, carrier and consumer are all production. §3b's *"STILL OPEN, and it is one of only three
mediums left in area 09"* no longer holds.

**Replacement text** (the `SUBA-054` clause of §3b's *medium* bullet):

```markdown
~~**`SUBA-054` (`defaultReads` never reaches a single run — also UW-16)**~~ — **CLOSED 2026-09-22.** `RunOptions::reads` (`exec/agent_config.rs:440-453`) is produced from the persona at `extension/executor/foreground.rs:1178` (`reads: agent.default_reads.clone()`) and consumed in `build_task_text` at `exec/spawn_plan.rs:1414-1420`, which prepends `build_single_reads_instruction` ahead of every other injected block — pi's `task = readsInstruction + task` (`subagent-executor.ts:3873`, BEFORE `injectSingleOutputInstruction` at `:3874`). `agent_config.rs:449-452` records the closed hole in-tree: *"Before this existed, `defaultReads` was parsed off frontmatter and rendered in agent listings but never reached a run"*. **Check UW-16 separately — this closure does not automatically discharge it**
```

Also stale in the same paragraph, and worth fixing while it is open:

* the `SUBA-055` clause says *"`guide` is in the 42-verb list"* — the list is **59**;
* the closing sentence *"The still-open rows are ALL in §1b as well (VL-S10, VL-S11, UW-16)"* names
  two rows that have since closed. With `SUBA-054`, `SUBA-024`'s handoff half and `SUBA-026` all
  moving, **§3b's only remaining open item is `SUBA-024`'s agent-contract half**.

---

## VL-P table (§3a) — row by row

### CLOSED

* **VL-P1** `baseten` is registered as a fleet provider with a dynamic catalog
  (`providers/all.rs:16`, `:60-64`; `providers/fleet.rs:162` maps `BASETEN_API_KEY`), and the
  `Baseten` thinking format plus `chat_template_args` are ported (`api/compat.rs:49`, `:338-345`).
* **VL-P2** `qwen-token-plan-individual` registered (`providers/all.rs:41`), env key at
  `env_api_keys.rs:59`.
* **VL-P3** `samplingParams` threaded — `utils/simple_options.rs:58-101` `merge_sampling_params`,
  with pi's `if (options?.samplingParams)` empty-map semantics preserved.
* **VL-P7** Copilot policy-state fallback at `providers/github_copilot.rs:322-328`
  (`model_picker_enabled`, `policy.state`), citing `github-copilot.ts:91-96`.
* **VL-P8** `BeforeToolCallResult.terminate` is live: `cyrup-agent/src/hooks.rs:58-68`
  `terminate: TerminateHint`, cited to AGENT-022.
* **VL-P9** `Agent::reset()` rejects mid-run —
  `cyrup-agent/src/agent/lifecycle.rs:113-116` returns `AgentError::RunActive(BusyEntry::Reset)`.
  (`agent.rs:1604-1616` is dead; the file no longer exists.)
* **VL-P14** `auth check` is ported — `cyrup/src/credential_print.rs:41-89`
  (`CredentialPrintKind::Check`, usage at `:76`, `AuthCheckResult` at `:89`), with the 0/1/2 exits.
* **VL-P18** both history ids are rebindable — `cyrup-tui/src/keymap.rs:471`
  `"tui.editor.historyPrevious" => E::HistoryPrevious` and the forward half at `:384`.
* **VL-P19** the alt-screen program exists — `crates/cyrup-tui/src/altscreen/`
  (`document/exit/flash/images/keys/mod/mouse/out/prompt_nav/scroll`), the eight `tui.altScreen.*`
  ids at `keymap.rs:1894-1913`, `--tui-mode` parsed at `cli/args.rs:190` and consumed by
  `cli/argv.rs:147-177` (so the `SEAM-051` exit-1 is gone), and
  `app/input_reader.rs:488` routes `Event::Mouse` into `altscreen::mouse::map_reader_event`
  instead of dropping it.
* **VL-P20** mermaid is rendered — `cyrup-tui/src/markdown/mermaid.rs`, with the
  `markdown.mermaid` setting at `cyrup-config/src/settings/effective.rs:478-485`.
* **VL-P21** the transformer seam exists on both sides — `cyrup-ext/wit/world.wit:340-346`/`:581-584`
  and `cyrup-ext/src/native.rs:416` `register_markdown_transformer`, with the pipeline documented at
  `cyrup-tui/src/markdown/mod.rs:184-200`. (Note VL-P20 no longer waits on this.)
* **VL-P25** every registered provider now has a catalog source: 35 embedded JSON catalogs plus the
  five deliberate dynamic ones (`radius`, the three `qwen-token-plan*`, `baseten` —
  `providers/all.rs:57`, "NO embedded catalog by design"). The row's own closure condition
  ("closes with PB-2, VL-P1, VL-P2") is met.

### VL-P10 · No compact-and-retry after a recoverable `length` stop — **CLOSED, and its premise is FALSE**

**The false inference:** the row's evidence is `rg is_recoverable_length crates/ = 0`. The literal
grep is still zero — **and it always would be**, because cyrup names the predicate differently. The
behaviour is ported.

```
$ sed -n '101,110p' crates/cyrup-provider/src/utils/overflow.rs
        // Case 3: Length-stop overflow (Xiaomi MiMo style) — server truncates oversized input to
        // fit the context window, leaving no room for output.
        if message.stop_reason == StopReason::Length && message.usage.output == 0 {
            … if (input_tokens as u128) * 100 >= (window as u128) * 99 { return true; }
$ sed -n '412,425p' crates/cyrup-session-svc/src/session/auto_compaction.rs
        // The predicate is pi's exact one — `stopReason === "error" || === "length"` …
        if will_retry { let _ = self.agent.pop_trailing_assistant_if(|a| matches!(
            a.stop_reason, StopReason::Error | StopReason::Length)); }
$ sed -n '444,447p' crates/cyrup-session-svc/src/session/run.rs
        if assistant.stop_reason != cyrup_core::StopReason::Length {
            *Self::lock(&self.overflow_recovery_attempted) = false;
        }
```

`is_context_overflow`'s case 3 **is** `isRecoverableLengthStop`; a `Length` response triggers
overflow recovery, the recovery compacts, and `will_retry` re-drives the interrupted turn with the
one-shot brake (`SEAM-112`) preventing the unbounded loop. This is a **zero-hit-assertion failure**
of exactly the class this sweep was told to hunt: the symbol is absent, the capability is not.

**Replacement text** (the VL-P10 table row):

```markdown
| ~~VL-P10~~ | ~~No compact-and-retry after a recoverable `length` stop~~ — **CLOSED 2026-09-22; the row's evidence was a false zero-hit.** `rg is_recoverable_length crates/` is still 0 and always would be: cyrup names the predicate differently. `utils/overflow.ts:171-173`'s `isRecoverableLengthStop` **is** `is_context_overflow`'s **case 3** (`cyrup-provider/src/utils/overflow.rs:101-109`: `StopReason::Length && usage.output == 0 && input*100 >= window*99`), which drives overflow recovery; the compact-and-**retry** half is `session/auto_compaction.rs:412-425` (`will_retry` + `pop_trailing_assistant_if(Error|Length)`, pi's exact predicate) with the one-shot brake at `session/run.rs:444-446` (SEAM-112) | — | — | closed — do not re-file on the symbol name |
```

### VL-P13 · Ambiguous bare `--model` silently picks the first catalog match — **PREMISE FALSE**

**The false sentence** (the row's upstream cell): *"`core/model-resolver.ts:469-503` (errors
`Model "…" is ambiguous across providers: …`)"*.

**The truth at the pinned tag:** upstream returns `undefined` on ambiguity and falls through to
partial matching. It never errors, and the quoted message exists nowhere in the tree.

```
$ git -C tmp/pi grep -n 'ambiguous across providers' v0.83.0 -- packages/
(no output — the quoted error message does not exist at v0.83.0)
$ git -C tmp/pi show v0.83.0:packages/coding-agent/src/core/model-resolver.ts | sed -n '73,78p;117,120p'
/**
 * Find an exact model reference match.
 * Supports either a bare model id or a canonical provider/modelId reference.
 * When matching by bare id, ambiguous matches across providers are rejected.
 */
	const idMatches = availableModels.filter((model) => model.id.toLowerCase() === normalizedReference);
	return idMatches.length === 1 ? idMatches[0] : undefined;
```

"Rejected" in the doc comment means *this exact-match helper returns `undefined`*, and
`tryMatchModel` then proceeds to partial matching — it is not an error path. Cyrup already matches
that and says so in its own source:

```
$ grep -n 'ambiguous' crates/cyrup-config/src/model/resolver.rs
34:/// Pi has no "ambiguous" concept — an ambiguous bare id resolves via partial matching, never errors.
76:/// one provider is ambiguous and yields `None` (`:118`).
163:/// Pi never errors on an ambiguous bare id; it falls through to partial matching, which always
443: fn ambiguous_bare_id_resolves_via_partial_like_pi()
```

`cyrup-config/src/model.rs` is also gone — the module is `cyrup-config/src/model/`.

**Replacement text** (the VL-P13 table row):

```markdown
| ~~VL-P13~~ | ~~Ambiguous bare `--model` silently picks the first catalog match~~ — **PREMISE FALSE, retired 2026-09-22.** The upstream cell claimed `model-resolver.ts:469-503` *"errors `Model "…" is ambiguous across providers: …`"*. `git -C tmp/pi grep -n 'ambiguous across providers' v0.83.0 -- packages/` returns **nothing**: `findExactModelReferenceMatch` returns `undefined` on an ambiguous bare id and `tryMatchModel` falls through to **partial matching**, which always picks one. The doc comment's word "rejected" describes the exact-match helper's `undefined`, not an error. cyrup reproduces upstream exactly and records the same finding independently at `cyrup-config/src/model/resolver.rs:34`, `:163` and `:443` (`ambiguous_bare_id_resolves_via_partial_like_pi`). `cyrup-config/src/model.rs:1139-1143` is dead — the module is now `cyrup-config/src/model/` | — | — | retired; there is no gap |
```

### STILL OPEN

* **VL-P4** — citations current. `grep -rn 'thinking_token_budget\|supports_thinking_token_budget'
  crates/ --include=*.rs` → **0**. Blocked on `PROV-015` as the row says.
* **VL-P5** — citations current. `grep -rn 'TelemetryContext\|telemetry_context' crates/
  --include=*.rs` → **0**.
* **VL-P6** — citations drifted. `auth/store.rs:24-53` `CredentialStore` still takes no options
  argument on `read`/`list`/`modify`/`delete`; `auth/resolve.rs` (770 lines) still has **no**
  timeout or cancellation on the refresh path (`grep -n 'timeout\|cancel' auth/resolve.rs` → 0) —
  the row's `:198` is now the double-checked-refresh body inside `resolve_stored_oauth` at
  `:150-200`. **cyrup cell → `auth/store.rs:24-53` (no options arg); `auth/resolve.rs:150-200`
  refresh unbounded, no `DEFAULT_OAUTH_REFRESH_TIMEOUT_MS` counterpart anywhere.**
* **VL-P12** — citations drifted. The hook is still a pure field diff:
  `cyrup-ext/src/hooks.rs:72-82` `after_tool_call` snapshots `content`/`is_error`/`details`/`usage`/
  `terminate` and diffs. The `images.autoResize` toggle is wired (`app/settings_rows.rs:101`
  → `eff.image_auto_resize()`) and the primitive exists for the `read` tool only
  (`cyrup-tools/src/tools/read.rs:425-427`, `image_proc::process_image`), so an image returned by
  **any other** tool is never normalized. **cyrup cell → `cyrup-ext/src/hooks.rs:72-82` (was
  `:58-113`); primitive `cyrup-tools/src/tools/read.rs:425-427` (was `:265`); toggle
  `cyrup-tui/src/app/settings_rows.rs:101`.**
* **VL-P15** — citations drifted. `cyrup-resources/src/package/manifest.rs:114`
  `serde_json::from_str(&text)?` (the `pi` block) and `:107` `toml::from_str(&text)?` (the
  `cyrup.toml` branch) — both still hard-fail where pi's try/catch yields `null`. **cyrup cell →
  `manifest.rs:114` (was `:87`); same at `:107` (was `:80`).**
* **VL-P16** — citations drifted. `cyrup-provider/src/remote_catalog.rs:652-655` is one `send()`
  with no retry wrapper; `utils/retry.rs` is the **assistant-call** retry loop (`retry.ts`), a
  different mechanism, and nothing in the workspace ports `management-http.ts`. **cyrup cell →
  `remote_catalog.rs:652-655` (was `:544-547`) — one `send()`; any transport error or 5xx is
  terminal. `utils/retry.rs` is `retry.ts`, not `management-http.ts`; do not conflate them.**
* **VL-P17** — citations drifted on the cyrup side only. **Rule-4 note: the upstream citation is
  CORRECT at the tag this section pins.** §3a is the `v0.83.0 → v0.84.1` window, and at v0.84.1
  `detectTerminalThemeForAuto` really does start both promises:

  ```
  $ git -C tmp/pi show v0.84.1:packages/coding-agent/src/modes/interactive/theme/theme.ts | sed -n '791,811p'
  export async function detectTerminalThemeForAuto({ … }) {
      let colorSchemePromise … = ui.queryTerminalColorScheme?.({ timeoutMs });
      const backgroundThemePromise = detectTerminalBackgroundTheme({ ui, timeoutMs, env });
      try { const colorScheme = await colorSchemePromise; if (colorScheme) return colorScheme; } …
      return (await backgroundThemePromise).theme;
  }
  ```

  At **v0.83.0** the same function is strictly sequential, so anyone re-checking this at the wrong
  tag will wrongly call the row false. cyrup is still sequential:
  `cyrup-tui/src/theme.rs:1416-1424` early-returns on `query_color_scheme` and only then calls
  `detect_terminal_background_theme`. **cyrup cell → `cyrup-tui/src/theme.rs:1416-1424` (was
  `:1334-1343`) — early return, then fall through.**
* **VL-P22** — citations drifted; both halves still open. The torn-tail repair is still gated out:
  `cyrup-session/src/manager/load.rs:36-53` sets `recovered = true` on a malformed line and
  `manager/lifecycle.rs:121-122` rewrites only `if migrated && !recovered`. Fork is still
  non-atomic: `cyrup-session/src/store.rs:326-362` `create_exclusive` writes header+entries
  **straight to the freshly created fd** (`:338-341` says so deliberately, mirroring pi's `"wx"`).
  **cyrup cell → `cyrup-session/src/manager/load.rs:36-53` (`load` skips malformed lines, returns
  `recovered`) and `manager/lifecycle.rs:106`, `:121-122`, where the rewrite is gated
  `if migrated && !recovered` — a recovered file is provably never rewritten.
  `store.rs:326-362` `create_exclusive` writes straight to the destination fd. (Was
  `manager.rs:851-888` / `:114-117` / `store.rs:86-116`; the manager monolith was split into
  `cyrup-session/src/manager/`.)**
* **VL-P23** — citations current. `ls crates/ | grep -i 'protocol\|client'` → nothing; no crate
  decodes framed CBOR.
* **VL-P24** — citations current. `grep -rn 'CredentialSynchronizationError\|enqueue_credential'
  crates/ --include=*.rs` → **0**.

---

## Cross-references checked

* **PB-29 "supersedes VL-P11"** — holds; both are now closed, so VL-P11's table row should carry
  "→ PB-29, **CLOSED**".
* **PB-30 "duplicate with area 12 `DRIFT-049` — schedule once in area 08"** — moot; closed.
* **PB-25 + PB-26 "must land in the same change"** — moot; both closed.
* **PB-33 "ships with `TUI-044`" / PB-34 "ships with `TUI-042`, fold in `TUI-049`"** — moot; closed.
* **VL-S15 "does NOT close UW-7"** — still correct: `register_shortcut` is a chord registration, not
  `on_terminal_input`. Untouched by this sweep.
* **VL-S2 "NOT closed by it and still owed: VL-S12's reverse-lag half"** — now discharged; VL-S12
  closed 2026-09-21.
* **VL-S7 → `SUBA-064`** — see the PREMISE FALSE above; the cross-reference's *escalation clause* is
  spent.
* **VL-S11 / §3b `SUBA-026` shared wrong path (`src/tui/selector.ts`)** — the correction holds and
  is re-proved here at every tag.
* **§3b "The still-open rows are ALL in §1b as well (VL-S10, VL-S11, UW-16)"** — two of the three
  named rows have closed; the sentence needs the rewrite given under SUBA-054.
* **PB-14 → UW-16 / `discovery/management.rs:1276-1277`** — that deferral note is gone; the
  cross-reference target no longer exists at that address.
* **VL-P20 "has nowhere to attach until VL-P21 lands"** — both landed; the dependency is discharged.
* **VL-P25 "closes with PB-2, VL-P1, VL-P2"** — all three closed, so VL-P25 closes with them.

## Unverifiable, and why

* **PB-37's screen behaviour** — the row itself says *"Verification requires a live run in a real
  terminal with two project dirs."* This sweep verified the two code halves it names
  (`set_all_rows`, `toggle_scope`, `set_session_cwds`) and no more; the rendering was not observed.
* **VL-P12's blast radius** — whether any non-`read` tool actually returns an image block in
  practice was not established; the verdict rests on the hook being a pure field diff, which is
  greppable, not on a measured failure.
* **Test-vs-production classification** — every "has callers" claim above was filtered by path
  (`/tests/`, `tests.rs`, `#[cfg(test)]` blocks) and by eye on the surrounding code. Two symbols are
  reported as *production-callerless but test-referenced* rather than zero-hit:
  `cyrup-config/src/settings/effective.rs:373` `npm_command` (PB-7, referenced only from
  `settings/tests/merge_and_scope.rs:463-481`) and
  `cyrup-ext-subagents/src/discovery/skills.rs:152` `resolve_skills` (PB-14, referenced only from
  its own module's tests). Neither changes its row's verdict.
* **No `cargo` was run**, per the brief — so nothing here rests on a compile or a test result; every
  claim is a grep, a `sed -n`, or a `git -C tmp/<upstream> show <tag>:<path>`.
