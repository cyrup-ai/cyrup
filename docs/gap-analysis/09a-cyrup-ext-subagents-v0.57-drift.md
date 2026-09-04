# 09a — cyrup-ext-subagents: the v0.47.1 → v0.57.0 drift pass

**This file supplements `09-cyrup-ext-subagents.md`. It does not supersede it, and no item in it is
edited by this pass.** `09` remains the area's file of record for `SUBA-001`…`SUBA-071`, for the
status table, and for the Trackers section. This document adds `SUBA-072`…`SUBA-091` and records
three corrections to `09`'s existing evidence (see `## Corpus health`). `PARITY-GAPS.md`,
`00-residual-ledger.md` and the README baseline table are likewise untouched here; where this pass
found one of them factually wrong at HEAD, the correction is written below rather than applied there,
so a single maintainer can reconcile all of them in one pass.

## Scope

**Port measured:** `crates/cyrup-ext-subagents/` at cyrup HEAD `6db22a7`
(*Merge pull request #67 — claude/decompose-discovery-management*), working tree clean. **204 `.rs`
files, 181,306 lines under `src/`** (tests included; the crate keeps most of its tests in-module).

**Upstream measured:** `nicobailon/pi-subagents` at tag **v0.57.0**. Clone HEAD is `9593a1cd`
(*chore: deslop utility comments (#1569)*, 2026-08-26), which is **ahead of the tag**; everything
below is settled at the tag. v0.57.0 carries **229 `.ts` files under `src/`**.

**The unanalyzed window.** `09` settled every claim at **v0.43.0** (the ported baseline) or
**v0.47.1** (then-latest). The range `v0.47.1..v0.57.0` had never been analyzed. Measured in this
clone:

| measurement | value |
|---|---|
| commits in range (all paths, incl. merges) | 305 |
| commits touching `src/` (incl. merges) | 245 |
| commits touching `src/` (`--no-merges`) | 244 |
| `src/` diffstat | **168 files changed, +21,385 / −7,307** |
| whole-tree diffstat | 357 files changed, +49,921 / −13,590 |
| `feat:` / `BREAKING` subjects, `src/`-touching | **54** |

The tasking brief for this pass carried the figures *330 commits / 174 src files / +22,871 / −7,443 /
58 feats*. Those are close but not reproducible from this clone at these tags; the table above is
what `git log`/`git diff` return here and is what the reader should use. The discrepancy does not
change any finding — it is recorded so the next pass does not re-derive it as a contradiction.

Ten of the twenty items below entered inside that window. Nine are **in-baseline (≤ v0.43.0)** — that
is, they were portable at the tag `09` measured against and were missed, not lagged. One
(`SUBA-085`) entered in `v0.43.0..v0.47.1`. The window's headline features — external CLI adapters
and the CLI capability contract, workflow scripts from files, runtime MCP direct tools, runtime agent
registration, separated global context inheritance, live context-window usage, subagent default
provider, the max-thinking ceiling — are represented below by `SUBA-074`, `SUBA-078`, `SUBA-079`,
`SUBA-084` and `SUBA-088`; the rest were either already tracked or cut (`## Already tracked`).

**Item count added by this pass: 20 ids (`SUBA-072`…`SUBA-091`), of which 1 (`SUBA-080`) is REFUTED
and 8 are CARRIED-UNVERIFIED.** Counted, verified, schedulable: **11 items — 1 critical, 8 high,
2 medium.**

## Methodology

Every upstream claim was settled with `git show v0.57.0:<path>`, never by reading the clone's working
tree, because clone-HEAD line numbers and file existence both mislead here (the clone is 12 commits
past the tag). Where an item's window matters, the same claim was re-checked at `v0.43.0` and/or
`v0.47.1` with the same command, and the two are never mixed silently: each item carries a
`**Window**` line stating which tag the behaviour first existed at.

**The restructure trap, and how absence was established.** `crates/cyrup-ext-subagents/src/extension.rs`
**no longer exists as a file** — it is now the directory `src/extension/` (`index`/`mod`, `executor/`,
`host/`, `models/`, `tool/`, …). Every `extension.rs:NNNN` citation in `09` is therefore
*unresolvable*, not merely stale: `SUBA-005`'s `:6557`, `SUBA-043`'s `:6543-6690`, `SUBA-047`'s
`:18993`, `SUBA-064`'s `:7805`/`:7825`, and the citations inside `SUBA-016`, `SUBA-055` and
`SUBA-057`. **A reader of `09` must not conclude a feature is absent because its cited path is gone.**
The more dangerous direction is the false negative, and this pass hit one: upstream's
`restoreActiveJobs` reads as absent under every name upstream uses and is **fully present** in the
port as `resume_tracking` (`src/extension/executor/status.rs:27`, wired on `SessionStart` at
`src/extension/host/native_impl.rs:347`, with `src/extension/executor/paths.rs:566-630` pinning both
of its subtleties). It is recorded under `## Refuted`-adjacent notes in `## Already tracked` so nobody
re-derives it.

Accordingly, **no absence claim below rests on resolving a cited path.** Each was established by
grepping the current tree for the behaviour *by identifier and by concept*, in both camelCase and
snake_case, plus the env-var spellings, plus the crate's own tests — the port's tests are treated as
evidence of presence, and several candidate findings died there.

**Severity** is `docs/gap-analysis/README.md:509-512` applied literally: `critical` = data loss,
silent wrong output, a permission bypass, or a crash on a normal path, **with no reachability
qualifier**. **Effort** is `S` under a day · `M` a few days · `L` a week+ or needs design.
**`[CYRUP-DELTA]`** in a port comment marks a deliberate divergence and is a decision, not a gap —
two candidate findings were dropped on that basis, and one item below (`SUBA-083`) exists precisely
*because* the divergence carries no such marker.

Each confirmed item passed an adversarial refutation pass instructed to reject anything it could not
personally re-read on both sides. Two severities were corrected downward by that pass and both
corrections are applied and recorded at the item.

---

## Summary — confirmed items

| ID | Sev | Eff | Subsystem | Title |
|---|---|---|---|---|
| ~~SUBA-072~~ | ~~critical~~ **CLOSED 2026-09-04** | M | foreground exec / tool allowlisting | Capability ceiling's `allowedTools` and `denyExtensions` axes are resolved and propagated but never applied to the child |
| ~~SUBA-073~~ | ~~medium~~ **CLOSED 2026-09-04** | M | config / permissions / frontmatter | Subagent permission policy never reaches a spawned child; `permission:` frontmatter is accepted and inert |
| ~~SUBA-074~~ | ~~high~~ **CLOSED 2026-09-04** | L | external runners / agent schema | Stage 1 (refusal) closed at `bf8b0f9`; **stage 2 ported at `af1a8a76`** — the capability/status contract, the hardened external-CLI runner, the generic no-adapter path (upstream's in-baseline `v0.43.0` half) and the `claude-code`/`claude-code-writer` adapter. A declared external profile now RUNS as the foreign process and resolves no model; `codex-exec`, `cursor-agent` and the whole `external-job` protocol stay refused by name through the new exhaustive `RunnerDispatch` |
| ~~SUBA-075~~ | ~~high~~ **CLOSED 2026-09-04** | M | fork context / thinking | Forked child sessions are not sanitized: signed/redacted Anthropic thinking blocks inherited, no thinking-off override |
| ~~SUBA-076~~ | ~~high~~ **CLOSED 2026-09-04** | S | acceptance / evidence scoring | Evidence checks are scored binary where upstream is tri-state, producing two spurious acceptance rejections |
| ~~SUBA-077~~ | ~~high~~ **CLOSED 2026-09-04** | S | foreground exec / deadlines | A foreground run with no explicit timeout has NO wall-clock deadline, and there is no global `timeoutMs` |
| ~~SUBA-078~~ | ~~high~~ **CLOSED 2026-09-04** | M | discovery settings / thinking | `subagents.maxThinking` ceiling entirely absent — no parse, no bound, no enforcement, no env propagation |
| ~~SUBA-079~~ | ~~high~~ **CLOSED 2026-09-04** | S | fork context / launch policy | `defaultContext: fork` hard-fails when the parent is unpersisted where upstream falls back to fresh; no config rung; no `context:"profile"` |
| ~~SUBA-081~~ | ~~high~~ **PARTIALLY CLOSED 2026-09-04** | M | discovery / settings overrides | Ten `agentOverrides` fields never apply, and a legal `tools: "inherit"` fails the settings load — 6 of 10 landed, 4 fields remain |
| ~~SUBA-082~~ | ~~high~~ **CLOSED 2026-09-04** | M | discovery / acceptance | **Promoted out of `## Carried` 2026-09-04** (upstream re-read at v0.57.0 AND v0.64.0, confirmed exactly as filed) **and ported at `5a4ae4ed`**: `acceptanceRole:`/`acceptance:` are in the schema, the role is the primary input to `infer_level`, and `acceptance:` is the single-agent launch default |
| ~~SUBA-083~~ | ~~high~~ **CLOSED 2026-09-04** | S | config / launch mode | `asyncByDefault` default is inverted, making the documented `asyncByDefault:false` opt-out a no-op |
| ~~SUBA-084~~ | ~~high~~ **CLOSED 2026-09-04** | M | discovery / runtime registry | **Promoted out of `## Carried` 2026-09-04** (both sides read at v0.57.0 and v0.64.0) **and ported at `dee8b9d0`**: `RuntimeAgentRegistry`, `AgentSource::Runtime` at source rank 4, the three collision checks, merge inside `run_discovery`, clear on `SessionShutdown`, public `register_agent`. Effort was M, not the filed L; the v0.64.0 event bridge is a recorded residual |
| ~~SUBA-085~~ | ~~high~~ **CLOSED 2026-09-04** | S | missions | `mission.resolve-decision` ported at `5e3aa1c8` — the seventh verb, the store transition, and upstream's open-decision status gate; the goal driver moves past a resolved decision, pinned by test |
| ~~SUBA-086~~ | ~~high~~ **CLOSED 2026-09-04** | M | discovery / diagnostics | **Promoted out of `## Carried` 2026-09-04** (both sides read; three corrections to the filed text recorded in the section) **and ported at `275c1f85`**: `AgentDiscoveryDiagnostic`, `parse_agent_file_checked`, `find_blocking_agent_diagnostic`, rendered by `list`/`get`/`models`/doctor and enforced at both delegation seams |
| ~~SUBA-087~~ | ~~medium~~ **PARTIALLY CLOSED 2026-09-04** | M | background control / child-scoped stop | **Promoted out of `## Carried` 2026-09-04** (both sides read at v0.57.0 and v0.64.0; one filing error corrected) **and ported at `2d9d0d0a`**: `childId` on the tool, `control/stop-requests/` queue with `targetIndex`/`childId`, `child_identity`/`child_stop` modules, the runner stops ONE step and keeps the run alive with pi's events and texts. **Review fix `6cf2cb9f`:** the stop-request file name draws a v7 uuid, so same-millisecond requests drain in write order (the review reproduced a 1-in-25 order flake in this row's tests). Residual: a `ParallelGroup`/`DynamicGroup`'s members are one step to cyrup's status, so a `tasks[]` fan-out's members are not individually addressable — **filed as `SUBA-093` (medium)** |
| ~~SUBA-088~~ | ~~medium~~ **CLOSED 2026-09-04** | M | config / discovery / model ladder | **Promoted out of `## Carried` 2026-09-04** (both sides read at v0.57.0 and v0.64.0; two citation corrections and one impact correction) **and ported at `ba24e5e5`**: `subagents.defaultProvider` + per-agent `agentOverrides.<name>.defaultProvider` parse with upstream's messages, `AgentDefinition::model_provider` stamped per `applySubagentDefaultModel`, the ladder takes `agent.model_provider ?? parent-session provider` and QUALIFIES a bare id to `provider/id` on the child's `--model`, the `models` report resolves per agent. Residuals (low): v0.64.0's `providerOverrides` and the discovery-cache provider key are not ported; a bare id that only ANOTHER provider offers is qualified onto the agent's/parent's provider and fails in the child, where upstream's registry-preferred `resolveExactIdMatches` would resolve it to that provider (upstream throws at the parent only when NO provider offers the id) — corrected wording, review 2026-09-04 |
| ~~SUBA-089~~ | ~~medium~~ **CLOSED 2026-09-04** | S | model-fallback ladder (foreground + background) | **Promoted out of `## Carried` 2026-09-04** (both sides read at v0.47.1, v0.57.0 and v0.64.0; confirmed exactly as filed, one correction: the filing's "retryable patterns present and correct" missed that the same upstream commit added the `connection` + whitespace + `error`/`reset`/`closed`/`aborted` pattern) **and ported at `cde2ddfc`**: `is_retryable_model_failure_attempt` is the ladder's sole retry gate — `tool_count > 0` never re-dispatches, the two empty-output sentinels, the no-activity clause, and per-message `errorMessage` corroboration over the new `AttemptSignal::message_errors`; the connection pattern lands with it. Residual (low): cyrup never emits the v0.64.0 terminal-stopReason sentinel it now recognises |
| ~~SUBA-090~~ | ~~medium~~ **PARTIALLY CLOSED 2026-09-04** | S | background completion notify / `display` | **Promoted out of `## Carried` 2026-09-04** (both sides read at v0.43.0, v0.57.0 and v0.64.0; confirmed exactly as filed — predicate verbatim at all three tags, `scheduleOrigin` clause the only tag-to-tag change) **and ported at `79ee7eff`**: `completion_notice_display(ClassifiedOutcome)` is pi's `notify.ts:402` predicate reduced to its one cyrup-reachable clause (`status !== "completed"`), `format_completion_message` computes `display` from the same `classify_outcome` that picks the header word, the false "Always `true`" doc is gone, `trigger_turn` stays `true` (no `triggerTurn:false` input exists). **Residual (medium, area 08/03 seam, not this crate):** on the trigger-turn path `session-svc inject.rs:125-160` drops `display` (`AgentMessage::Custom` has no such field) so the hidden notice still renders on screen — **filed as `SUBA-094` (medium)**; the grouped `formatGroupedCompletion` form stays with `SUBA-017` |
| ~~SUBA-091~~ | ~~medium~~ **CLOSED 2026-09-04** | S | fleet inspector / transcript containment | **Promoted out of `## Carried` 2026-09-04** (both sides read at v0.57.0 and v0.64.0 plus the upstream landing commit `9ceb5650`; confirmed exactly as filed, line drift only) **and ported at `681f6255`**: `FleetState::trusted_session_roots` is pi's `state.trustedSessionRoots` (`index.ts:895-898`: `defaultSessionDir` tilde-expanded + resolved, then the parent's subagent session root, deduped), seeded by `SubagentExecutor::fleet_state` through the pure `paths::trusted_session_roots`, and `async_detail` passes `unique_paths(state.trusted_session_roots)` where the literal `&[]` was, so the session-JSONL tail renders in the detail pane; the containment gate is unchanged. Residuals (low): pi's `trustedSessionFiles`/`trustedSessionFileRoot` rung and `trackedJob?.sessionRoot` are not carried; `subagent status`'s cyrup-original root triple differs from pi's `trustedSessionRootsForStatus` |
| ~~SUBA-092~~ | ~~high~~ **CLOSED 2026-09-04** | M | discovery / agent schema | `excludeTools:`/`allowNestedSubagents:` ported at `247ff97b` — frontmatter, settings-override, serializer, and the spawn-plan tool subtraction / nested-fanout grant. v0.64.0's cross-field custom-override precedence change (`31562d76`) is a recorded residual, not this row |
| ~~SUBA-093~~ | ~~medium~~ **CLOSED 2026-09-04** | M | background status model / child-scoped stop | `SUBA-087`'s residual, filed as its own item 2026-09-04 **and ported at `07f2df0d`**: `background/flat_index.rs` is pi's declaration-time flatten, so a `ParallelGroup` publishes one `RunStatus::steps` entry PER MEMBER named by its own agent; `ChainRunContext::step_slot` (`StepSlot::{Exclusive,Shared}`) carries pi's per-member `ctx.flatIndex` through the telemetry fold, `child_index`, the steer paths, the artifact index and the per-child stop handle, which `ExecSingleStepExecutor::run_single` now registers per DISPATCH. `subagent({action:"stop", id, childId:"step:1"})` against a 3-task fan-out stops member 1 alone. Residuals (low): a `DynamicGroup` keeps one shared slot (upstream's runtime splice, `:4155`, is not ported); group members go `Running` at group dispatch rather than per worker claim |
| SUBA-094 | medium | M | completion notify / session-svc inject seam | `SUBA-090`'s residual, filed as its own item 2026-09-04 — **FIX SITE `crates/cyrup-session-svc` + `crates/cyrup-agent` (areas 08/03)**: `inject_message` drops `display` on the trigger-turn path (`AgentMessage::Custom` has no such field), so a `display: false` completion notice still renders |

> **RE-AUDITED 2026-09-04, cyrup HEAD `2571969`** (baseline `4fb5e40`, 09/09a combined pass). Of the
> eleven items counted "confirmed, schedulable" above, **nine are now closed and one is
> partially closed** — all nine landed in two commits, `bf8b0f9` (SUBA-072/073/074-stage1/075/076)
> and `c94a360` (SUBA-077), plus `SUBA-078`/`SUBA-079`/`SUBA-083` whose landing commits were not
> individually identified (the workspace history around them was squashed/rebased — `git log -S` on
> their defining symbols lands on `3f9380f`, a commit whose own subject is unrelated, so treat the
> commit attribution as approximate; the CODE was read directly and is not in question).
> Every closure above was verified by reading the actual current code (function bodies, call sites,
> tests) — not by trusting the commit message — per this ledger's evidence rule. Only `SUBA-074`
> (stage 2 residual) and `SUBA-085` remain open and fully unchanged from this file's original filing;
> both were re-confirmed absent by the same greps this file used originally, re-run at HEAD.
> **The `## Already tracked` and `## Corpus health` sections below are UNCHANGED and not re-audited
> this pass** — several of their cross-references (e.g. "`SUBA-021`/`VL-S1` says … no ceiling
> concept" in Corpus health §3) describe area 09's evidence as of the original filing and are now
> further stale in the direction this closure record already fixes; a maintainer reconciling area 09
> and 09a together should re-derive them rather than trust the prose as still current.
>
> **One new item filed this pass, from the window past this file's own scope**: upstream moved from
> v0.57.0 (this file's tag) to **v0.64.0** since the original filing. Per the task brief's
> conservative-skim instruction, `git -C tmp/pi-subagents diff --stat v0.57.0..v0.64.0` was skimmed
> (141 files, +14684/−3625, dominated by the still-out-of-scope `workflowScript`/watchdog
> subsystems) rather than re-swept item-by-item, and exactly one high-confidence, both-sides-read
> defect was filed: **`SUBA-092`** (agent-level `excludeTools:`/`allowNestedSubagents:`, window
> v0.57.0..v0.62.0). The rest of the window was NOT exhaustively walked — treat it as unaudited, not
> as clean.

> **SECOND PASS 2026-09-04, cyrup code HEAD `275c1f85` (five code commits on
> `claude/beautiful-feynman-odz1v5` after `a4805955`, which is `main`).** Five rows closed, each on
> the confirmed bar — the Rust read at HEAD after the landing commit, the TypeScript read at the
> named tag(s) with `git -C tmp/pi-subagents show`, the landing commit's diff read rather than its
> subject trusted, and an independent review that re-read both sides again (`cargo clippy -p
> cyrup-ext-subagents --all-targets -- -D warnings` clean; `cargo nextest run -p cyrup-ext-subagents`
> 2666/2666 at `275c1f85`). In landing order: **`SUBA-085`** (`5e3aa1c8`), **`SUBA-092`**
> (`247ff97b`), **`SUBA-082`** (`5a4ae4ed`), **`SUBA-084`** (`dee8b9d0`), **`SUBA-086`** (`275c1f85`).
> Per ADR-0006 every port targets **v0.64.0**; where a row was filed at v0.57.0 the section says what
> v0.64.0 changed and whether the port took it.

> **REVIEW FIXES 2026-09-04, `6cf2cb9f` (code) — the batch-2 review of the five carried-medium ports
> blocked on one defect and returned four ledger corrections, all landed here.** Blocking:
> `SUBA-087`'s stop-request tests were order-flaky (reproduced 1-in-25) because two requests written
> in one millisecond tie on the file name's `ts` prefix and a random v4 uuid decided the drain
> order — fixed at the source (`control::stop_request_file_name` now draws `Uuid::now_v7()` from the
> crate's monotonic shared context, so a `ts` tie drains in write order; pinned by
> `control::tests::same_millisecond_stop_requests_drain_in_write_order`, 24 same-`ts` requests, red
> under v4 by construction). Ledger: `SUBA-088`'s residual (3) overstated upstream's failure mode
> (corrected in the row and the section); the two PARTIALLY CLOSED rows (`SUBA-087`, `SUBA-090`)
> each carried a MEDIUM residual that `scripts/count_open_items.py` could not see (struck id ⇒
> closed) — each residual is now its own open row, **`SUBA-093`** and **`SUBA-094`**, so the census
> counts them; `route_child_stop_requests` no longer swallows a failed `status.json` write after an
> accepted child stop (`tracing::warn`). Item count: 23 ids, `SUBA-072`…`SUBA-094`.
>
> **Three rows left `## Carried — NOT adversarially verified` this pass.** `SUBA-082`, `SUBA-084` and
> `SUBA-086` were first held to the confirmed bar — every upstream line each filing quoted was re-read
> at v0.57.0 and again at v0.64.0, and all three verdicts came back CONFIRMED (with `SUBA-084`'s
> effort corrected L→M and three corrections to `SUBA-086`'s filed text, recorded in its section) —
> **then** ported. Each now has a full section in the confirmed set above the `## Carried` heading, in
> id order; a one-line pointer stays at its old location. `scripts/count_open_items.py`'s
> hand-enumerated `carried_high` list was emptied in the same commit, so the three count once, as
> closed rows of the table, and not a second time as open carried rows.
>
> **Residual leads recorded by the five closures, NOT filed as rows — ownerless until a pass reads
> them on the confirmed bar** (the citations below were read by the implementers and the reviewer;
> this ledger pass re-resolved each `git show` line but did not port-side re-read them):
> - **v0.63.0 `0128385f` (#1799, 2026-08-31, first tag v0.63.0) — `inferLevel` omits inferred
>   acceptance for read-only reviewers.** `git show v0.64.0:src/runs/shared/acceptance.ts:105`
>   (`readOnlyAgent` feeds `inferredReadOnly`), `:107,110-111` (`dynamicResolvesReadOnly` guard),
>   `:137` (the read-only branch returns level `none`, not `attested`). cyrup's
>   `exec/acceptance/model/level.rs::infer_level` is deliberately the v0.57.0/v0.62.0 body
>   (`SUBA-082`'s closure says why: the crate's lattice maps the read-only branch to `Attested`).
>   Candidate `upstream-drift`, medium.
> - **v0.63.0 `31562d76` (#1798, 2026-09-01) — custom-agent override precedence.**
>   `git show v0.64.0:src/agents/agents.ts:1476` `applyCustomAgentOverride` now delegates to
>   `applyBuiltinOverride` for every key, dropping the frontmatter-presence gate; cyrup's
>   `discovery/merge.rs::apply_custom_override` still implements v0.62.0's fill-unset contract
>   (R-SA-010) for all 20 override fields, the two `SUBA-092` added included. Cross-field; candidate
>   `upstream-drift`, medium.
> - **v0.64.0 runtime-agent EVENT bridge** — `git show v0.64.0:src/agents/runtime-agent-events.ts:4-5`
>   (`pi-subagents:runtime-agent-register:v1`), `:29-48` `registerAgentViaEvents` (synchronous emit,
>   handler mutates `request.result` in place), `:51-70` the listener; re-exported at
>   `src/api/agents.ts:3-10`. cyrup's `SharedBus` (`crates/cyrup-ext/src/bus.rs:83-91`,
>   `[CYRUP-DELTA]`) queues emits and passes payloads by value, so this needs a request/response
>   topic design. Candidate `upstream-drift`, medium/L.
> - **Five `RuntimeAgentDefinition` fields with no `AgentDefinition` landing** — `mcpDirectTools`,
>   `inheritGlobalContext`, `mutationTools`, `skillPath`, `defaultToolTimeoutMs` — are validated with
>   upstream's messages and then REFUSED with a marked `[CYRUP-DELTA] SUBA-084` error
>   (`discovery/runtime_registry.rs` `UNREPRESENTABLE_FIELDS`), never silently dropped. Each closes
>   with the row that lands its field.
> - **Tooling, not parity:** the workspace is not `rustfmt`-clean at `a4805955` under the pinned
>   toolchain (no `rustfmt.toml`; `cargo fmt --all -- --check` reports ~14 900 hunks, reproducible
>   from the base commit's own files). Every implementer hit it and reverted the churn by hand.
>   Repo-level decision, ownerless.

Carried-but-unverified (`## Carried — NOT adversarially verified`): **no rows remain** — the last,
`SUBA-091`, was promoted, confirmed and CLOSED on 2026-09-04 at `681f6255` — see `## ~~SUBA-091~~`.
(The three highs that sat here,
`SUBA-082`/`SUBA-084`/`SUBA-086`, were promoted and closed on 2026-09-04 — see the blockquote
above; `SUBA-087` was promoted, confirmed and PARTIALLY CLOSED on 2026-09-04 at `2d9d0d0a` — see
`## ~~SUBA-087~~`; `SUBA-088` was promoted, confirmed and CLOSED on 2026-09-04 at `ba24e5e5` — see
`## ~~SUBA-088~~`; `SUBA-089` was promoted, confirmed and CLOSED on 2026-09-04 at `cde2ddfc` — see
`## ~~SUBA-089~~`; `SUBA-090` was promoted, confirmed and PARTIALLY CLOSED on 2026-09-04 at
`79ee7eff` — see `## ~~SUBA-090~~`.) All were re-checked port-side at cyrup HEAD `2571969` this
pass: every zero-hit grep this file recorded for them still returns zero hits — none of the 210
commits since baseline `4fb5e40` touched any of these symbols/behaviours. Each was then held to
the confirmed bar (upstream re-read with `git show` at the named tags) BEFORE its port landed, so
the section's lower evidence standard no longer applies to any live row; the section is kept as a
record with one-line pointers.
Refuted: `SUBA-080`.

---

## ~~SUBA-072~~ — ~~critical~~ **CLOSED 2026-09-04** — The capability ceiling's `allowedTools` and `denyExtensions` axes are resolved, intersected and propagated but never applied to the spawned child

> **CLOSED 2026-09-04, cyrup HEAD `2571969`** (re-audit against baseline `4fb5e40`, area 09/09a pass).
> Landed by `bf8b0f9 feat(subagents): close five v0.57.0 parity gaps in cyrup-ext-subagents`
> (2026-08-27) — verified by reading the current code, not the commit message.
> `crates/cyrup-ext-subagents/src/exec/spawn_plan.rs::build_attempt_spawn_plan` now resolves
> `capability_ceiling` at `:325` (`preflight_capability_ceiling`) and threads
> `ceiling_allowed_tools`/`ceiling_deny_extensions` through both the tool-allowlist block and the
> extension block: `explicit_tool_allowlist = agent.tools.is_some() || ceiling_allowed_tools.is_some()`
> (`:731`, matching `pi-args.ts:473-476`'s `allowedToolSet !== undefined` test — this is the exact
> fix this item's own Fix line asked for), the declared/undeclared arms both intersect against
> `ceiling_allowed_tools` (`:667-683`), and `ceiling_deny_extensions` empties the extension paths and
> MCP selections and forces the no-extensions equivalent (`:792-814`, `:1075-1077`). The stale
> in-source claim this item quoted ("cyrup has no capability ceiling … `allowedToolSet` is permanently
> `undefined`") is gone from the current file. 11 tests at `spawn_plan.rs:1544-1780` cover both axes
> including the item's own Verify recipe (`the_capability_ceilings_allowed_tools_axis_gates_both_the_declared_and_undeclared_arms`,
> `a_capability_ceilings_deny_extensions_axis_strips_all_extension_paths_and_forces_no_extensions`,
> `a_capability_ceiling_excluding_read_fails_the_launch_when_read_is_required`).
>
> **Also revises `SUBA-021` / `PARITY-GAPS VL-S1`** exactly as this item's own "Relation to corpus"
> instructed: `SUBA-021` (area 09) is already closed there citing the ceiling landing in sweep 10;
> this closure record supersedes its residual claim of non-application.

**Kind** parity-bug · **Severity** critical · **Effort** M · **Confidence** confirmed
**Subsystem** foreground execution / tool allowlisting (`exec/spawn_plan.rs`)
**Window** in-baseline (≤ v0.43.0) — `git cat-file -e v0.43.0:src/runs/shared/capability-ceiling.ts` succeeds.

**upstream** — `git show v0.57.0:src/runs/shared/pi-args.ts`. `resolvePiLaunchToolPlan` at **`:423`**
intersects the call-site and inherited ceilings and builds `allowedToolSet` at **`:430-433`**. The
resolved ceiling then drives **four** independent narrowings:
- **`:439-441`** throws ``Capability ceiling from ${sources} excludes required tool 'read' for lazy skill loading.`` when `requireReadTool` is set and the set lacks `read`.
- **`:444-455`** `declaredBuiltinTools` becomes `[...allowedToolSet]` on the `input.tools === undefined` arm, and is `.filter((tool) => !allowedToolSet || allowedToolSet.has(tool))` on the declared arm.
- **`:457-463`** `toolExtensionPaths` is `[]` when `denyExtensions`; **`:464`** `resolvedMcpSelections` is likewise `[]`; **`:467-469`** the surviving MCP selections are filtered through `allowedToolSet`; **`:514`**/**`:527`** force `disableAmbientExtensions` and empty `configuredExtensions`.
- **`:473-476`** `explicitToolAllowlist` is true whenever `allowedToolSet !== undefined`, so `buildPiArgs` at **`:662`** always emits `--tools <ceiling set>` or `--no-tools` for a ceilinged child, and **`:668`** pushes `--no-extensions` under `disableAmbientExtensions`.

**cyrup** — `crates/cyrup-ext-subagents/src/exec/capability_ceiling.rs` defines `allowed_tools` at
`:85` and `deny_extensions` at `:91`, parses them at `:192`/`:195` and intersects them at `:390`/`:392`.
`grep -rn 'allowed_tools\|deny_extensions' --include=*.rs crates/cyrup-ext-subagents/src` returns
**only** `capability_ceiling.rs` (definition, parse, intersect, tests) plus the unrelated
`watchdog/review.rs` — **no consumer anywhere in `src/exec/spawn_plan.rs`**. In `spawn_plan.rs` the
ceiling is resolved at `:309` and only the AGENTS axis is enforced (`assert_agent_allowed`, `:313`);
the tool-allowlist branch gates solely on `let explicit_tool_allowlist = agent.tools.is_some();`
(**`:397`**) and builds the allowlist from `builtin_tools` + `effective_mcp_tools` with no ceiling
filter; the extension branch gates solely on `agent.extensions`; the ceiling is then only
base64-encoded into the child env at `:876-891`. **The port's own comment at `spawn_plan.rs:417-420`
still asserts** *"cyrup has no capability ceiling (tracked as SUBA-021), so `allowedToolSet` is
permanently `undefined`"* — stale since `capability_ceiling.rs` landed. Nothing on the CHILD side
reads it either: `CAPABILITY_CEILING_ENV` hits only `spawn_plan.rs` (write side) and
`capability_ceiling.rs` (constants).

**Impact** — A host that registers a ceiling `{allowedTools: ["read"], denyExtensions: true}` for a
session gets **no tool bound and no extension bound at all** in cyrup. An agent whose frontmatter
declares `tools: [read, write, bash]` is spawned with `--tools read,write,bash`; an agent that
declares no `tools:` is spawned with the **full ambient tool set and full ambient extension
discovery** — no `--tools`, no `--no-extensions` — because `explicit_tool_allowlist` is `false`.
Upstream spawns the first with `--tools read` and the second with `--tools read`, both with
`--no-extensions` and with MCP direct tools and tool-extension paths stripped. Because the agents
axis *is* enforced and the ceiling *is* propagated to the child env, **the ceiling presents as armed
while two of its three axes silently permit exactly the widening it exists to prevent.** That is a
permission bypass under `README.md:510`, hence `critical`.

**Fix** — In `exec/spawn_plan.rs`, feed the already-resolved `capability_ceiling` into the tool plan:
(a) `explicit_tool_allowlist = agent.tools.is_some() || !effective_mcp_tools.is_empty() || ceiling_allowed_tools.is_some()`, mirroring `pi-args.ts:473-476`; (b) intersect `builtin_tools` and the
MCP selections against the allowed set on both arms of `pi-args.ts:444-455`; (c) under
`deny_extensions`, empty the extension paths and MCP selections and push cyrup's `--no-extensions`
equivalent; (d) land the `requireReadTool` throw at the same time — it is `SUBA-014`'s companion and
the two share the branch. Delete the stale claim at `spawn_plan.rs:417-420` in the same change.

**Verify** — With a ceiling `{allowedTools:["read"]}` registered: an agent declaring
`tools: [read, write, bash]` must spawn with `--tools read`; an agent declaring no `tools:` must also
spawn with `--tools read`. With `{denyExtensions:true}`: the child argv must carry the
no-extensions flag and no tool-extension path or MCP direct tool. With `requireReadTool` and a
ceiling lacking `read`, the launch must fail with pi's message.

**Relation to corpus** — **REVISION of `SUBA-021` / PARITY-GAPS `VL-S1`.** `SUBA-021`'s evidence
(`rg 'capability_ceiling' = 0`, "no ceiling concept") is now factually wrong at HEAD — the subsystem
landed in sweep 10 — and the residual defect is *materially worse* than the one `SUBA-021` filed,
because an unimplemented ceiling is visibly absent whereas this one presents as enforced. Either
raise `SUBA-021` to `critical` and rewrite its body, or supersede it with this row.

---

## ~~SUBA-073~~ — ~~medium~~ **CLOSED 2026-09-04** — Subagent permission policy never reaches a spawned child: `config.permissions` and agent `permission:`/`permissions:` frontmatter are accepted and inert

> **CLOSED 2026-09-04, cyrup HEAD `2571969`.** Landed by `bf8b0f9` (same commit as `SUBA-072`),
> verified by reading the current code. `crates/cyrup-ext-subagents/src/exec/permissions.rs` now
> exists (`validate_permission_rules`/`validate_permission_config`/`resolve_permission_rules`/
> `encode_permission_rules`), its output is written into the child env at
> `exec/spawn_plan.rs:1270-1292` under `permission_arbiter::PERMISSION_POLICY_ENV`, and
> `discovery/frontmatter.rs` now lists both `"permission"` and `"permissions"` in `KNOWN_FIELDS`
> (`:121-122`) with upstream's mutual-exclusion error reproduced verbatim at `:939` ("cannot declare
> both permission and permissions frontmatter"). Test coverage at `spawn_plan.rs:3138-3170` asserts
> the env key is present when a policy resolves and absent when none does. This closes the item's
> whole Verify recipe (policy reaches env; mutual-exclusion error; frontmatter round-trips as a real
> field rather than `extra_fields`).

**Kind** not-ported · **Severity** medium *(corrected down from `critical` as filed — see the note below; `high` is defensible)* · **Effort** M · **Confidence** confirmed
**Subsystem** config / permissions / discovery frontmatter
**Window** in-baseline (≤ v0.43.0) — `v0.43.0:src/runs/shared/permissions.ts` and `v0.43.0:src/shared/types.ts` both carry it.

**upstream** — `git show v0.57.0:src/shared/types.ts` **`:2268`** declares
`permissions?: PermissionConfig` on `ExtensionConfig`, documented at `:2267` as *"Opt-in native tool
permissions. Bash remains outside this policy."* `git show v0.57.0:src/runs/shared/permissions.ts`
(99 lines) defines `PERMISSION_POLICY_ENV = "PI_SUBAGENT_PERMISSION_POLICY"` (**`:8`**),
`validatePermissionRules` (**`:21`**), `validatePermissionConfig` (**`:35`**), `resolvePermissionRules`
(**`:44`**), `permissionDecision` (**`:50`**) and `encodePermissionRules` (**`:55`**).
`src/extension/config.ts` runs `validatePermissionConfig(config.permissions)` on every config read.
`git show v0.57.0:src/agents/agents.ts` **`:2033`** throws
``Agent '${localName}' cannot declare both permission and permissions frontmatter.`` and then parses
`frontmatter.permissions ?? frontmatter.permission` through `validatePermissionRules`;
`agent-serializer.ts` carries both spellings in `KNOWN_FIELDS`. `async-execution.ts`,
`api/preflight.ts` call `resolvePermissionRules(ctx.permissions, agentConfig.permissions)` and
`pi-args.ts` writes the encoded policy into the child env.

**cyrup** — `grep -rn 'PERMISSION_POLICY_ENV' crates/cyrup-ext-subagents/src/exec/ crates/cyrup-ext-subagents/src/spawn/`
→ **0 hits**; there is no writer anywhere in the workspace. Every hit crate-wide is a READ site: the
child-side gate `src/watchdog/permission_arbiter.rs:355` (cyrup's `CYRUP_SUBAGENT_*` spelling) and
`src/prompt_runtime.rs:1399,1442,2225-2227,2446,2467`. The crate states it in-tree at
`src/watchdog/permission_arbiter.rs:60-63`: *"The parent-side half (`validatePermissionConfig`,
`resolvePermissionRules`, `encodePermissionRules`, and `pi-args.ts:713-758`'s env writes) is still
unported, so a policy reaches a child today only if something outside this crate sets
`PERMISSION_POLICY_ENV`; that is the remaining work, and it lives in `exec/`, not here."* On the
frontmatter side, `src/discovery/frontmatter.rs:72-116 KNOWN_FIELDS` contains **neither** `permission`
nor `permissions` (grep for `permission` in that range: 0 hits), and the crate's own tests PIN the
demotion — `frontmatter.rs:1213-1216` asserts a `permission:` block lands in `extra_fields` and
`present_fields`. `SubagentExtensionConfig` (`src/registration/mod.rs:79-245`) has no `permissions`
key.

**Impact** — An operator who writes `{"permissions": {"rules": {"write": "deny"}}}` in subagent
config, or an agent author who writes `permission: {"*": ask, bash: {"*": ask, "git *": allow}}` in
an agent file, gets the value accepted with no error and silently not enforced: the child spawns with
no policy env var, `permission_arbiter`'s gate is never armed, and the denied tool runs. Upstream's
mutual-exclusion error for declaring both spellings is also absent. The child-side enforcement
machinery is fully ported and permanently unreachable.

**Severity note (correction applied).** Filed `critical`; corrected to `medium` by the refutation
pass, on three grounds read literally against `README.md:510`. (1) This is not a bypass of an
*enforcing* system: a cyrup subagent child is still gated by `cyrup-permission-system`, wired into
every spawn, with the child→parent ask-forwarding spool live at `spawn/nested_events.rs:781`; upstream
itself documents `permissions` as **opt-in** and leaves bash to pi-guard. (2) Upstream's own normal
state is "no policy, no gate" — `resolvePermissionRules` returns `undefined` on an empty merged map
and no handler is installed — which is exactly the state cyrup is permanently in; the divergence is
that cyrup cannot *leave* it. (3) No data loss, no crash, no silent wrong output. **`high` is
defensible** on the frontmatter half alone: an agent file that literally reads
`permission: {...}` is accepted, round-tripped through `extra_fields`, re-serialized on rewrite and
never enforced, with no diagnostic — and `registration/authority.rs:22` states the crate's own
principle that *"a config key that is parsed and ignored is a permission bypass"*. `critical` is not
defensible given (1).

**Fix** — Port `permissions.ts`'s parent half as `exec/permissions.rs`
(`validate_permission_rules`/`validate_permission_config`/`resolve_permission_rules`/`encode_permission_rules`),
add `permissions` to `SubagentExtensionConfig` with the config-load validation, add both `permission`
and `permissions` to `frontmatter.rs`'s `KNOWN_FIELDS` with upstream's mutual-exclusion error, and
write the encoded policy into the child env in `exec/spawn_plan.rs` beside the existing tool-budget
encoder. The child side needs no work.

**Verify** — A child launched under `{"permissions":{"rules":{"write":"deny"}}}` must have
`CYRUP_SUBAGENT_PERMISSION_POLICY` set and must refuse a `write`; an agent declaring both
`permission:` and `permissions:` must fail to load with pi's message; an agent-level rule must merge
over the global config per `resolvePermissionRules`'s precedence.

**Relation to corpus** — New. Not covered by `SUBA-061` (whose four keys are `asyncWidget`,
`inlineToolDisplay`, `fleetKeybindings`, `legacyChainControls`), not by `SUBA-064` (`authorityPolicy`),
and not by area `10`, which owns the permission-system crate rather than this crate's parent-side
encoder. The discovery-lens and config-lens halves are merged here because both land in one place:
the env write in `exec/spawn_plan.rs`.

---

## ~~SUBA-074~~ — ~~Agent `runner:` frontmatter is ignored entirely, so a profile upstream runs as a sandboxed read-only foreign CLI runs in cyrup as a full-capability native child~~

> **STAGE 1 CLOSED 2026-09-04, cyrup HEAD `2571969`.** Landed by `bf8b0f9` (same commit as
> `SUBA-072`/`SUBA-073`), verified by reading the current code — this is exactly the item's own Fix
> stage (1): "stop the silent widening." `discovery/frontmatter.rs` adds `"runner"` to `KNOWN_FIELDS`
> (`:126`) and parses it into `AgentDefinition::runner`; `parseAgentRunnerFrontmatter`'s Pi-only-field
> guard is ported at `:1001-1012` (rejects `tools`/`permission`/etc. alongside a non-`pi` runner) and
> `validateCodeOwnedProfileRunner` at `:990-1020`. **The refusal is live on the production launch
> path, not only tested**: `exec/mod.rs::run_sync`'s Step 0b (`:287-303`) calls
> `agent.runner.as_ref().and_then(AgentRunnerConfig::refusal_reason)` **before** the model-fallback
> ladder and fails the run with pi's message via `pre_spawn_failure`, with a same-file test asserting
> a `pi` runner never hits it (`exec/mod.rs:1602-1614`). `runner::AgentRunnerConfig::refusal_reason`
> (`src/runner/mod.rs:102-120`) names both `external-cli` and `external-job` and cites
> "SUBA-074 stage 2" in its own refusal text. **Stage 2 — the three adapters
> (`claude-code-adapter.ts`/`codex-exec-adapter.ts`/`cursor-agent-adapter.ts`), the capability
> contract, preflight and the external-job protocol — is UNCHANGED and remains open under this same
> ID**, since no separate ID was ever filed for it (checked: no `.flux` task or ledger row names a
> stage-2-specific ID). Re-scope this item's own Kind/Severity/Effort to stage 2 only next time it is
> picked up — stage 1's `L` effort is spent; what remains is genuinely the adapter/protocol work the
> original Fix called out as separable.

> **STAGE 2 CLOSED 2026-09-04, cyrup HEAD `3e9633c4`.** Landed by `af1a8a76`, both sides re-read
> at `v0.64.0` (ADR-0006 target) before implementing. What shipped: the capability/status contract
> (`runner/contract.rs` + new `runner/status.rs` — `resolveExternalCliRunnerStatus`,
> `normalizeExternalCliRunnerStatus` incl. the legacy `grok-build` branch,
> `externalCliReceiptMetadata`, the six per-adapter `safety` blocks and the seven unsupported
> reasons with their two prompt-file overrides); the hardened runner (`exec/external_cli/` —
> sealed environments, bounded logs and byte tails, JSONL framing with the oversized-line rule,
> the preflight probe with its `(binary, mtime, spec)` cache and typed invalidation, prompt-file
> delivery, process-GROUP teardown on a deadline or a stop); **the generic no-adapter
> `external-cli` path**, which is the in-baseline (`v0.43.0`) half this row noted was "never ported
> either" — so the item is no longer window lag PLUS baseline lag; and **one adapter**,
> `claude-code`/`claude-code-writer` (32-key env allowlist, plan/`acceptEdits` argv, the JSONL
> parser, the version regex and the fourteen required help strings). `SingleResult` gained
> `runner` and `externalProcess`, both optional on the wire.
>
> **The stage-1 gate was replaced, not extended.** `AgentRunnerConfig::refusal_reason() ->
> Option<String>` is gone; `runner::dispatch::resolve_runner_dispatch` returns an exhaustive
> `RunnerDispatch::{NativePi, ExternalCli(launch), Refused(reason)}` with no `_` arm, so
> "did not refuse" can no longer be what selects the native child. That ordering was
> non-negotiable: with the supported set non-empty and the gate still an `Option`, `None` would
> again mean "spawn a full-capability native child", silently. The external arm returns from
> `exec/mod.rs`'s Step 0b before `run_fallback_ladder`, because upstream resolves NO model for an
> external runner at all (`api/preflight.ts:322-343`).
>
> **Deferred, with reasons, still refused by name:** `codex-exec` (a second output channel — the
> `--output-last-message` artifact — and a second terminal vocabulary); `cursor-agent`
> (prompt-file delivery, the `--add-dir` handoff dir, the `skipOversizedLine` rule); and the whole
> `external-job` protocol — that one on a CONTRACT argument, not size:
> `v0.64.0:src/api/external-job-provider.ts:1-2` is a pure embedder registry with **zero in-repo
> providers**, so porting it into cyrup (which has no host-registration surface) would produce a
> path that can never succeed, replacing an honest "not supported" with a dishonest "no provider
> registered".
>
> **Three anchor/count corrections to the text below.** The refusal is at `exec/mod.rs:308-325`,
> not `:287-303`; the tools doc is `discovery/types.rs:1025`, not `:728-730`; and the claude-code
> env allowlist is **32** keys (`claude-code-adapter.ts:10-43`). Also ported here as a tag-to-tag
> correction inside this item's own surface: `validateExternalRunnerProfile`'s Pi-only field list
> is **seventeen** at `v0.64.0` (`agents.ts:1906`), not the fourteen stage 1 pinned —
> `excludeTools`, `allowNestedSubagents` and `mutationTools` were added after that port.
>
> **Tests (34 added).** The behavioural fail-before/pass-after is
> `run_sync_executes_a_generic_external_cli_profile_and_resolves_no_model`: at the previous HEAD
> every `external-cli` runner took `pre_spawn_failure` (exit 1, no output); it now runs the foreign
> process, delivers its stdout, and carries `model: None` with an empty ladder plus a runner and
> process receipt. `only_a_pi_runner_is_honourable_today` is FALSIFIED by this change and was
> split rather than deleted — `runner/dispatch.rs` pins both halves (claude-code + generic
> dispatch to a launch; codex/cursor/external-job still refuse), and `spawn_plan.rs`'s hop-C test
> now asserts the rebuilt config still dispatches EXTERNALLY.
>
> **Residuals (low).** (1) `StepResult` carries no external-runner receipt, so an ASYNC run's
> `runner`/`externalProcess` do not survive the background projection
> (`background/runner_main.rs`); the run itself executes identically on both paths. (2) The
> external-runner consumers outside this crate's spawn path are unported: the steer/resume
> refusals (`subagent-executor.ts:1135`, `:1589-1597`, `:1847-1852`), the `RunStatus::steps[].runner`
> projection, and `api/preflight.ts`'s own external branch. Each depends only on the contract types
> that now exist. (3) `runner_to_json_string` re-emits a `capabilities:` block in upstream's
> capability order rather than the author's — strictly closer to upstream than the previous
> alphabetical order.

**Kind** not-ported · **Severity** high · **Effort** L · **Confidence** confirmed
**Subsystem** external runners / agent definition schema
**Window** in-baseline (≤ v0.43.0) for the `runner:` key and generic external-cli dispatch; **v0.47.1..v0.57.0** for the adapter ids, capability contract, preflight, hardened runner and the entire external-job protocol.

**upstream** — `git show v0.57.0:src/agents/agents.ts` **`:121`** `runner?: AgentRunnerConfig`;
**`:1803`** `parseAgentRunnerFrontmatter` (type must be `pi` | `external-cli` | `external-job`;
`external-cli` requires a non-empty `command`; an optional code-owned `adapter` id rejected unless one
of the recognised set; `args` alongside `adapter` rejected because the adapter owns its argv;
`promptDelivery: "stdin"` only); **`:1864`** `validateExternalRunnerProfile`, which HARD-FAILS such a
profile that also declares any of `tools, model, fallbackModels, thinking, extensions,
subagentOnlyExtensions, maxSubagentDepth, completionGuard, skills, skill, skillPath, toolBudget,
permission, permissions` — **`:1869`** ``Agent '${agentName}' uses runner.type='${runner.type}' and
declares unsupported Pi-only fields: ${unsupported.join(", ")}.`` **`:1950`**
`validateCodeOwnedProfileRunner`, imported at **`:12`** from `runs/shared/external-cli-contract.ts`.
The execution branch in `src/runs/background/subagent-runner.ts` never launches a pi child for
`external-cli`, and a separate branch handles `external-job`. **In-baseline:**
`git show v0.43.0:src/agents/agents.ts` already parses `runner.type` = `pi`|`external-cli` with
`command`/`args`/`promptDelivery`, and `git show v0.43.0:src/runs/shared/external-cli-runner.ts` is
already a working runner. The window added the capability contract (`external-cli-contract.ts`), the
hardened runner (env allowlists, bounded logs, JSONL framing, prompt-file delivery, process-tree
kill), the preflight probe (`external-cli-preflight.ts`), three adapters
(`claude-code-adapter.ts`, `codex-exec-adapter.ts`, `cursor-agent-adapter.ts`) and the whole
external-job protocol (`api/external-job-provider.ts`, `external-job-bridge.ts`,
`external-job-runner.ts`).

**cyrup** — `grep -rn 'runner' crates/cyrup-ext-subagents/src/discovery/frontmatter.rs` → **0 hits**,
and `KNOWN_FIELDS` (`frontmatter.rs:72-116`) has no `runner` entry, so the key falls through to
`extra_fields` — the nested-block round-trip is pinned by the crate's own
`permission_style_nested_block_round_trips_into_extra_fields` test at `frontmatter.rs:1209`.
`src/discovery/types.rs:702-838 AgentDefinition` has no `runner` or adapter field. Workspace-wide,
`grep -rniE 'external.cli|externalcli|external_job|external-job' --include=*.rs crates/cyrup-ext-subagents/src`
returns **one** hit and it is a doc comment (`src/background/runner_main.rs:4173`, citing
`external-cli-runner.ts:108` only for a verbatim error string). `grep -rn 'codex|cursor-agent|claude-code'`
across `crates/` matches only `cyrup-provider`'s OpenAI-Codex HTTP provider and
`cyrup-tui/src/auth_select.rs` — a different subsystem, with no argv construction, no process spawn
and no JSONL parser.

**Impact** — An agent file declaring `runner: {type: external-cli, adapter: claude-code}` (or
`{type: external-job, provider: …}`) is **neither rejected nor honoured**: the block is round-tripped
into `extra_fields` and the agent loads as an ordinary native agent against the session's own model.
Because upstream FORBIDS `tools:` and `permission:` on such a profile, the profile carries no tools
declaration — and `AgentDefinition::tools == None` in the port means *"no allowlist restriction, all
builtin tools available"* (`discovery/types.rs:728-730`). So the exact profile upstream runs as a
plan-mode, read-only, no-MCP, curated-env foreign process, cyrup runs as a **native child with the
full builtin tool surface in the workspace**, with no error and no diagnostic. Upstream's guard
rejecting Pi-only fields on such a profile is also absent, so a contradictory definition is accepted
silently. `high` rather than `critical`: cyrup's own runtime permission system still gates the
resulting tool calls, so this is a silent widening of the *declared* capability envelope rather than
a bypass of an enforcement point.

**Fix** — Two landable stages. **(1) Stop the silent widening, today, at `S` effort:** add `runner`
to `KNOWN_FIELDS` and to `AgentDefinition`, port `parseAgentRunnerFrontmatter` +
`validateExternalRunnerProfile`, and **refuse to launch** a non-`pi` runner with a named error until
stage 2 lands. That converts a silent capability widening into a loud unsupported-feature error and
is worth doing independently. **(2) Port the runners:** `external-cli-runner.ts` + the capability
contract + preflight + the three adapters, then the external-job protocol; each is a separable
change.

**Verify** — An agent declaring `runner: {type: external-cli, command: "…"}` plus `tools: [read]`
must fail to load with pi's "unsupported Pi-only fields" message. After stage 1, an
`external-cli` profile must fail the launch with a named error rather than spawning a native child.
After stage 2, it must spawn the foreign CLI with the adapter's argv and never a pi child.

**Relation to corpus** — **REVISION of PARITY-GAPS `VL-S14`** (*"`runner: external-cli` agents
unsupported"*, `medium`), whose scope and severity are both now wrong: the subsystem tripled inside
the window and gained a second runner type (`external-job`) that `VL-S14` does not name, and the
consequence is not "unsupported" but a silent capability widening. Two provenance corrections for
whoever works it: **there is no `grok-build-adapter.ts` at v0.57.0** (renamed to
`cursor-agent-adapter.ts`; `grok-build` survives only as a legacy receipt id), and **the baseline
half was never ported either**, so this is not pure window lag.

---

## ~~SUBA-075~~ — ~~high~~ **CLOSED 2026-09-04** — Forked child sessions are not sanitized: signed and redacted Anthropic thinking blocks are inherited verbatim and no thinking-off override is applied to the branch

> **CLOSED 2026-09-04, cyrup HEAD `2571969`.** Landed by `bf8b0f9`, verified by reading the current
> code. `crates/cyrup-ext-subagents/src/fork_context.rs` now carries
> `forked_child_requires_thinking_off` (`:266`), `sanitize_unsafe_thinking_blocks` (`:371`) and
> `append_thinking_off_entry` (`:418`) — `grep -ci thinking fork_context.rs` is no longer 0. The
> `replace_existing` third parameter this item's Fix asked for is on `apply_thinking_suffix`
> (`exec/spawn_plan.rs:130-153`), with a test (`:4540`) pinning that a `:7b`-suffixed id is not
> mistaken for a thinking-level suffix. The rewrite echoes untouched JSONL lines verbatim per the
> commit's own stated reason (no `preserve_order` on this crate's `serde_json`).

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed
**Subsystem** fork context / thinking level
**Window** in-baseline (≤ v0.43.0) — all three functions exist at `v0.43.0:src/shared/fork-context.ts`.

**upstream** — `git show v0.57.0:src/shared/fork-context.ts`: **`:106`**
`forkedChildRequiresThinkingOff(model, availableModels, preferredProvider)` — true for an unknown
model, a model whose `provider` is `anthropic`, or whose `api` is `anthropic-messages`; **`:118`**
`isUnsafeAnthropicThinkingBlock` (true for `redacted_thinking`, and for `thinking` blocks carrying
`redacted: true` or a non-empty `thinkingSignature`/`signature` on an Anthropic provider/api/model);
**`:140`** `appendThinkingOffEntry` appends a `{type:"thinking_level_change", thinkingLevel:"off"}`
entry to the branched session; **`:153`** `sanitizeUnsafeThinkingBlocks` strips those blocks from
every assistant entry; **`:189`** `createForkContextResolver` rewrites the branched session file with
the sanitized entries (default `forceThinkingOffForIndex` true) and returns `thinkingOverride: "off"`.
`subagent-executor.ts` builds `forkThinkingRequirements` per child index; the resulting override
becomes `options.thinkingOverride`, which `execution.ts` feeds to
`applyThinkingSuffix(model, thinking, /*replaceExisting=*/ options.thinkingOverride !== undefined)` —
and with `replaceExisting` true an existing `:<level>` suffix is **REPLACED**.

**cyrup** — `crates/cyrup-ext-subagents/src/fork_context.rs` is **529 lines** and
`grep -ci 'thinking' crates/cyrup-ext-subagents/src/fork_context.rs` returns **0**: no forced
thinking-off, no `forceThinkingOffForIndex` analogue, no branch sanitization. Crate-wide,
`grep -rniE 'redacted_thinking|sanitize_unsafe|thinking_off|requires_thinking_off|replace_existing' --include=*.rs`
returns exactly **one** hit and it is an unrelated test name
(`discovery/frontmatter.rs:1363 thinking_off_is_preserved_as_explicit_off_distinct_from_unset`).
`ForkContextResolver::resolve` (`fork_context.rs:140-208`) branches via `create_branched_session` and
hands the path straight back with no post-write pass. `src/exec/spawn_plan.rs:124-139`
`apply_thinking_suffix(model, thinking)` takes **no** `replace_existing` parameter and returns the
model UNCHANGED when it already carries a recognized suffix (`:133-137`); its only call site
(`spawn_plan.rs:323`) passes `agent.thinking` alone.

**Impact** — A `context: "fork"` subagent branching a parent session that contains signed or redacted
Anthropic thinking blocks is launched with those blocks intact and with thinking still enabled.
Upstream forces the branch to thinking-off and strips the blocks precisely because the Anthropic
messages API rejects thinking blocks whose signatures do not match the new request context — so the
cyrup child fails at the provider on a normal fork path against an Anthropic model. The missing
`replace_existing` compounds it: even if a thinking-off override existed, a model id already carrying
`:high` would keep `:high`. `high` not `critical`: the failure surfaces as a provider rejection turned
into a subagent error result, and it needs the non-default `context: "fork"`, an Anthropic-family
child model, and a parent transcript that actually carries such blocks.

**Fix** — Port the three functions into `fork_context.rs`, run `sanitize_unsafe_thinking_blocks` +
`append_thinking_off_entry` over the branched session before `ForkContextResolver::resolve` returns,
return a `thinking_override` on the resolution, and add the `replace_existing` arm to
`apply_thinking_suffix` (`exec/spawn_plan.rs:124`) so the override replaces an existing `:<level>`
suffix rather than deferring to it.

**Verify** — Fork a parent transcript containing one `redacted_thinking` block and one signed
`thinking` block against an Anthropic child model: the branched session file must contain neither
block and must end with a `thinking_level_change → off` entry, and the child must spawn with the
model id's `:high` suffix replaced by `:off`.

**Relation to corpus** — New. Area 09 has no fork-context row at all. Minor citation note for
whoever works it: the port's `apply_thinking_suffix` doc cites `pi-args.ts:186-200`, which is the
v0.43.0 range; at v0.57.0 `applyThinkingSuffix` has moved.

---

## ~~SUBA-076~~ — ~~high~~ **CLOSED 2026-09-04** — Acceptance evidence checks are scored binary where upstream is tri-state, so an honest `changedFiles: []` and an omitted `noStagedFiles` each produce a spurious acceptance REJECTION

> **CLOSED 2026-09-04, cyrup HEAD `2571969`.** Landed by `bf8b0f9`, verified by reading the current
> code. `crates/cyrup-ext-subagents/src/exec/acceptance/model/checks.rs:27`
> (`report_evidence_status`) now returns `RuntimeCheckStatus::NotApplicable` for `Some([])`, matching
> upstream's tri-state; `run_structural_checks`' `NoStagedFiles` arm (`:65`) is a report-derived
> `continue` when the child said nothing, deferring to the parent's own `git status` check in the same
> list, exactly as this item's Fix asked for. Module doc at `checks.rs:2-3` cites
> `reportEvidenceStatus`/`checkNoStagedFiles`/`runStructuralChecks` by name.

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** acceptance / evidence scoring
**Window** in-baseline (≤ v0.43.0) for the `changed-files`/`tests-added` tri-state; **v0.47.1..v0.57.0** for the `no-staged-files` skip.

**upstream** — `git show v0.57.0:src/runs/shared/acceptance.ts`. `reportEvidenceStatus` at **`:932`**
returns `AcceptanceRuntimeCheckStatus`, not a boolean: for `"changed-files"` it returns `"failed"`
only when the field is not a string array, and otherwise
`report.changedFiles.length === 0 ? "not-applicable" : "passed"` — identically for `"tests-added"`.
Every other kind is binary. `runStructuralChecks` at **`:961`** opens its loop with
**`:964`** `if (kind === "no-staged-files" && report.noStagedFiles === undefined) continue;` — the
report-derived check is SKIPPED, and only the parent-side real `checkNoStagedFiles(cwd)`
(`git status --short`, pushed unconditionally at **`:976`** when the kind is requested) decides. The
tri-state is recorded as the check status with the message at **`:972`**
``${kind} evidence explicitly reported as not applicable.`` `evaluateAcceptance` rejects on
`runtimeChecks.some((check) => check.status === "failed")` only, so `not-applicable` does **not**
reject. The `no-staged-files` `continue` is absent from `git show v0.47.1:src/runs/shared/acceptance.ts`
(`bd5664a0 fix: trust parent staged-file acceptance check (#1385)`).

**cyrup** — `crates/cyrup-ext-subagents/src/exec/acceptance/model/checks.rs:14-42`
`report_evidence_present` returns a plain `bool`:
`ChangedFiles => report.changed_files.as_ref().is_some_and(|v| !v.is_empty())`,
`TestsAdded => …is_some_and(|v| !v.is_empty())`, `NoStagedFiles => report.no_staged_files == Some(true)`.
`run_structural_checks` (`:170-196`) iterates `for kind in evidence` with **no skip clause** and maps
the bool binary: `status: if present { RuntimeCheckStatus::Passed } else { RuntimeCheckStatus::Failed }`,
message `"{kind} evidence missing from child report."`, then pushes the parent-side
`check_no_staged_files(cwd)` at `:192-194`. `grep -rn 'NotApplicable' --include=*.rs` shows
`RuntimeCheckStatus::NotApplicable` is produced at exactly two sites, `checks.rs:125,132` — both
inside `check_no_staged_files`'s git-unavailable branch — **never** for an evidence check.
`src/exec/acceptance/model/evaluate.rs:160,208,219` reject on
`.any(|c| c.status == RuntimeCheckStatus::Failed)`.

**Impact** — Two spurious rejections on normal paths. **(1)** A child under
`acceptance: {evidence: ["changed-files"]}` that correctly reports `changedFiles: []` — a reviewer, an
oracle, a genuine no-op task — is accepted upstream with `evidence:changed-files = not-applicable` and
**REJECTED** by the port with `evidence:changed-files failed / changed-files evidence missing from
child report`. **(2)** With `evidence: ["no-staged-files"]` and a clean workspace, a child that simply
omits `noStagedFiles` is accepted upstream (the parent's own `git status` passes) and rejected by the
port — even though the port's own `git status` check *in the very same list* passed. In both cases
the ledger flips to `rejected` and the caller is told the child failed acceptance when it did not.
`high` not `critical`: the wrong verdict is loud (an explicit `rejected` status carrying a named
message), it fails closed rather than admitting bad work, and nothing is lost or bypassed.

**Fix** — One function. Change `report_evidence_present` to return `RuntimeCheckStatus`, giving
`ChangedFiles`/`TestsAdded` upstream's three arms (not-a-string-array → `Failed`, empty →
`NotApplicable`, else `Passed`), add the third message arm, and add the
`NoStagedFiles && report.no_staged_files.is_none() → continue` skip at the top of
`run_structural_checks`'s loop.

**Verify** — `evidence: ["changed-files"]` with `changedFiles: []` must accept, with the
`not-applicable` status and pi's message; with `changedFiles: "oops"` (not an array) must reject.
`evidence: ["no-staged-files"]` with `noStagedFiles` omitted and a clean worktree must accept with
exactly one `no-staged-files` check in the list.

**Relation to corpus** — New. Area 09 has no acceptance-scoring row (`SUBA-028` is acceptance
*cancellation*), and this pass confirmed the acceptance tree is otherwise substantially complete —
this is a defect inside ported code, not a missing subsystem. Both halves are one function; file and
fix together.

---

## ~~SUBA-077~~ — ~~high~~ **CLOSED 2026-09-04** — A foreground subagent run with no explicit timeout has NO wall-clock deadline, and there is no global `config.timeoutMs`

> **CLOSED 2026-09-04, cyrup HEAD `2571969`.** Landed by
> `c94a360 feat(subagents): give foreground runs a wall-clock deadline and a global timeoutMs`
> (2026-08-28), verified by reading the current code.
> `crates/cyrup-ext-subagents/src/extension/tool/params.rs::resolve_foreground_timeout` (`:330`) now
> resolves `explicit .or(agent_default) .or(config_default) .or(Some(DEFAULT_FOREGROUND_TIMEOUT_MS))`
> (the `1_800_000` constant pinned by a same-file test asserting it against pi's 30-minute default,
> `:659`), and `registration/mod.rs` carries `timeout_ms` on `SubagentExtensionConfig`, RAW rather
> than typed per the commit's own stated reason (upstream degrades an invalid value rather than
> erroring the whole config). The commit's own message records that it additionally fixed **two
> surfaces `SUBA-077`'s own filing had not named** — the `/run` slash command and the top-level
> parallel path, the latter of which had been hard-coding `None` and so dropping an explicit
> call-site `timeoutMs` outright. Also **discharges the contradiction `SUBA-077`'s own "Relation to
> corpus" flagged**: area 09's `SUBA-051` Fix line said "do not apply it to foreground runs, which
> already have their own default" — that was false before this closure and is moot now that the
> foreground path has a real default of its own.

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** foreground execution / deadlines
**Window** in-baseline (≤ v0.43.0) for the 30-minute foreground default; **v0.47.1..v0.57.0** for the `config.timeoutMs` rung.

**upstream** — `git show v0.57.0:src/runs/foreground/subagent-executor.ts` **`:2656`**
`export const DEFAULT_FOREGROUND_TIMEOUT_MS = 30 * 60 * 1000;`; **`:2684`**
`resolveConfigDefaultTimeoutMs` validates `config.timeoutMs` as a positive integer; **`:2719`**
`resolveSingleAgentLaunchTimeout(params, async, configDefaultTimeoutMs)` computes **`:2721`**
`const foregroundDefault = configDefaultTimeoutMs ?? DEFAULT_FOREGROUND_TIMEOUT_MS` and applies it to
every non-async launch. Its `!async` arm does not test `isComposite`, so the backstop applies to
single, `tasks: []` and chain launches alike at the one shared call site **`:5914-5917`**; the same
resolution appears again at **`:4440`**. The `config.timeoutMs` rung is a window addition, and commit
`0ed0afee` states the defect it fixes verbatim: agent frontmatter `timeoutMs` was applied only to
single-agent launches, so parallel and chain launches "never adopt it and fall back to the built-in
30-minute foreground default … with no global knob to raise the default."

**cyrup** — `crates/cyrup-ext-subagents/src/extension/tool/params.rs:264-280`
`resolve_foreground_timeout` validates `0` and the `timeoutMs`/`maxRuntimeMs` alias mismatch and then
returns `Ok(p.timeout_ms.or(p.max_runtime_ms))` — **no default at all**. Its caller
`src/extension/tool/routing.rs:370-372` does `resolve_foreground_timeout(p)…?.or(launch_defaults.1)`,
where `launch_defaults.1` is only the agent's own frontmatter `timeoutMs`
(`src/extension/executor/nested_control.rs:148-172`).
`grep -rn '1_800_000\|1800000\|30 \* 60' --include=*.rs` hits only `src/background/wait.rs:86`,
`src/background/mod.rs:43,57` (`DEFAULT_ASYNC_CHILD_TIMEOUT_MS`) and `src/extension/wait_tool.rs:65`
— the async side has its default, the foreground side has none. `SubagentExtensionConfig`
(`src/registration/mod.rs:79-245`) has no `timeout_ms` field; grepping `timeout_ms` in that file
returns only `worktree_setup_hook_timeout_ms`.

**Impact** — `subagent({agent:"x", task:"…"})` run in the foreground against an agent with no
frontmatter `timeoutMs` has **no wall-clock deadline** in cyrup: a child whose bash tool blocks
forever hangs the orchestrator's turn indefinitely with no signal, where upstream terminates it at 30
minutes with `Subagent timed out after 1800000ms.` Separately, an operator who sets
`subagents.timeoutMs` gets nothing — upstream uses it to replace the backstop for single, parallel,
chain and plain single-agent async launches alike, which is the only way to raise a long fan-out's
ceiling without passing `timeoutMs` on every call. `high` not `critical`: an unbounded hang is none
of the four `critical` conditions — but it sits at the top of `high`, because the failure is silent
and open-ended.

**Fix** — Give `resolve_foreground_timeout` a `config_default: Option<u64>` parameter and have it
return `p.timeout_ms.or(p.max_runtime_ms).or(agent_default).or(config_default).or(Some(DEFAULT_FOREGROUND_TIMEOUT_MS))`
for every non-async launch, mirroring `:2719-2725`'s precedence, and add `timeout_ms` to
`SubagentExtensionConfig` with upstream's positive-integer validation. Apply it at **all** foreground
call sites in `routing.rs`, not just the single-agent one — the parallel path drops an explicit
`timeoutMs` today.

**Verify** — A foreground agent with no frontmatter `timeoutMs` whose child sleeps must be terminated
at the default with pi's message; `subagents.timeoutMs: 60000` must replace that default on single,
`tasks: []` and chain launches; an explicit call-site `timeoutMs` must still win over both.

**Relation to corpus** — **REVISION-adjacent to `SUBA-051`**, which covers the ASYNC child default
and whose Fix line explicitly instructs *"Do not apply it to foreground runs, which already have
their own default."* **That instruction is wrong at HEAD and following it would leave the foreground
path unbounded forever.** Distinct item because the fix site (`extension/tool/params.rs` + a new
config key) differs from `SUBA-051`'s (`background` step construction).

---

## ~~SUBA-078~~ — ~~high~~ **CLOSED 2026-09-04** — `subagents.maxThinking` ceiling is entirely absent — no settings parse, no per-agent bound, no enforcement, no env propagation to nested children

> **CLOSED 2026-09-04, cyrup HEAD `2571969`** (`feat(subagents): port the subagents.maxThinking
> reasoning ceiling`, folded into the workspace history under commit `3f9380f`'s range), verified by
> reading the current code. `crates/cyrup-ext-subagents/src/exec/thinking_ceiling.rs` exists and is
> consumed, not merely present: `exec/spawn_plan.rs` resolves `inherited_thinking_ceiling` and
> `intersect_thinking_ceilings` at `:386-396` (asserted with `assert_thinking_within_ceiling` before
> the spawn plan is built) and again at `:1321-1330` where the resolved ceiling is written into the
> child env under `THINKING_CEILING_ENV`, so it crosses the re-exec boundary and can only tighten
> going down (`intersect` takes the lowest, matching upstream's monotonic contract). `exec/mod.rs:53-54`
> documents the module by ID. A same-file test (`spawn_plan.rs:4463`,
> `the_thinking_ceiling_crosses_the_spawn_boundary_only_when_one_is_set`) covers the env-propagation
> half of this item's own Verify.

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed
**Subsystem** discovery settings / thinking level
**Window** v0.47.1..v0.57.0 (`547112ec feat: add max thinking ceiling for subagents #1397`).

**upstream** — `git show v0.57.0:src/shared/thinking-ceiling.ts` (56 lines):
**`:4`** `SUBAGENT_THINKING_CEILING_ENV = "PI_SUBAGENT_THINKING_CEILING"`; **`:8`**
`parseThinkingLevel`; **`:16`** `compareThinkingLevels`; **`:23`** `intersectThinkingCeilings`, which
takes the **LOWEST** so a bound can only tighten down a nested subtree; **`:29`**
`decodeThinkingCeiling`; **`:42`** `assertThinkingWithinCeiling`, which throws
``Thinking level '<x>' exceeds configured maximum '<y>' for agent '<a>' run '<r>'.``
`git show v0.57.0:src/agents/agents.ts` puts `maxThinking?: ThinkingLevel` on `SubagentSettings`, on
`AgentConfig` and on `AgentDiscoveryResult`, parses it with
``Subagent settings in '<file>' have invalid 'maxThinking'; expected one of off, minimal, low, medium,
high, xhigh, or max.``, and stamps it onto every merged agent via `resolveSubagentMaxThinking`
(project beats user) + `applySubagentMaxThinking`. It is enforced in
`src/runs/foreground/execution.ts` before the pi-args build **and** per model candidate, folded
monotonically, re-intersected and written to the child env in `src/runs/shared/pi-args.ts`, and
reported as a `"thinking_ceiling"` refusal by `src/api/preflight.ts`.

**cyrup** — `grep -rn 'max_thinking\|maxThinking\|thinking_ceiling\|THINKING_CEILING' --include=*.rs crates/cyrup-ext-subagents/src`
→ **0 hits** (a workspace-wide grep finds only two unrelated test function names in `cyrup-provider`
and `cyrup-config`). `src/discovery/types.rs:505-541 SubagentSettings` carries `default_model`,
`default_thinking`, `default_extensions`, `disable_builtins`, `disable_thinking`, `model_scope` — no
`max_thinking` — and `src/discovery/mod.rs:1174-1191 AgentDiscoveryResult` has no such field.
`parse_subagent_settings` (`src/discovery/mod.rs:655-705`) deserializes with no
`deny_unknown_fields`, so an authored `maxThinking` is dropped without diagnostic. The port's only
thinking handling on the launch path is `apply_thinking_suffix` (`src/exec/spawn_plan.rs:124-139`),
which applies the agent's level unconditionally.

**Impact** — An operator who sets `subagents.maxThinking: "low"` (or a per-agent `maxThinking`) gets
no bound and no error: an agent declaring `thinking: xhigh` is spawned with `--model <id>:xhigh` and
burns the reasoning budget the ceiling was configured to cap. Upstream hard-refuses the run — against
both the chosen model and every fallback candidate — and inherits the bound down every nesting level
through the ceiling env var, so a child can only tighten it. There is no
`CYRUP_SUBAGENT_THINKING_CEILING`, so even a bound that existed could not survive the re-exec.
`high` not `critical`: the run's answer is correct, it simply consumes more reasoning budget than the
operator capped — a configured resource bound silently ignored, not a permission bypass (the separate
CAPABILITY ceiling governs access and is `SUBA-072`).

**Fix** — Port `thinking-ceiling.ts` as `exec/thinking_ceiling.rs` (compare / intersect / decode /
assert), add `max_thinking` to `SubagentSettings`, `AgentDefinition` and `AgentDiscoveryResult` with
upstream's parse error and project-beats-user resolution, assert it in `exec/fallback.rs` per model
candidate as well as once before the spawn-plan build, and write the intersected ceiling into the
child env in `exec/spawn_plan.rs` beside the capability ceiling.

**Verify** — `subagents.maxThinking: "low"` plus an agent declaring `thinking: xhigh` must refuse the
run with pi's message; a nested child must inherit the bound through the env var and must not be able
to widen it; a fallback candidate that would exceed the ceiling must be refused too.

**Relation to corpus** — New. **NOT** covered by `SUBA-021` / `VL-S1`, which is the CAPABILITY
ceiling (tools/agents/extensions) — a different mechanism, a different env var, and already partly
landed. Merges three lens candidates (foreground-exec, discovery-settings, shared-config) that are
one subsystem.

---

## ~~SUBA-079~~ — ~~high~~ **CLOSED 2026-09-04** — An agent's `defaultContext: fork` hard-fails the launch when the parent session is not yet persisted, where upstream falls back to fresh — plus no config `defaultSubagentContext` rung and no `context: "profile"`

> **CLOSED 2026-09-04, cyrup HEAD `2571969`**, verified by reading the current code — all three
> sub-claims land. **(1)** `fork_context.rs::can_prefer_fork_from_snapshot` (`:109`) and the instance
> method `ForkContextHandle::can_prefer_fork` (`:544`) test availability (persisted file + leaf id)
> before an *inherited* preference downgrades to fresh, with `resolve_effective_context` (`:172-188`)
> taking a `can_prefer_fork: bool` and keeping the strict path for an *explicit* call-site `Fork`
> separately — exactly the split this item's Fix asked for, tested at `:842-852`. **(2)**
> `registration/mod.rs::default_subagent_context` (`:377-381`, `Option<serde_json::Value>`, RAW like
> `timeout_ms`) feeds `fork_context::resolve_default_subagent_context` (`:205`), tested at
> `fork_context.rs:860-876` including upstream's invalid-value error. **(3)** the schema enum is
> `["fresh", "fork", "profile"]` (`extension/tool/schema.rs:401`, pinned at `:743`), and the
> `Profile` policy branch's refusal — `context: "profile" requires agent '<name>' to declare
> defaultContext.` — is live at `fork_context.rs:165-180`, tested at `:803`.

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** fork context / launch context policy
**Window** v0.47.1..v0.57.0.

**upstream** — `git show v0.57.0:src/shared/fork-context.ts` **`:80-84`**
`resolveSubagentLaunchContext`:
```ts
if (input.explicitContext !== undefined) return input.explicitContext;
const preferredContext = input.defaultSubagentContext ?? input.agentDefaultContext ?? "fresh";
return preferredContext === "fork" && input.canUseImplicitFork ? "fork" : "fresh";
```
with the comment at **`:86-87`** *"Explicit `context: "fork"` stays strict and does not use this
preference"*, `canPreferFork` at **`:88`** and `canPreferForkFromSnapshot` at **`:95`** returning
false when there is no persisted parent session file or no leaf id. **The config rung OUTRANKS the
agent's own default.** `git show v0.57.0:src/extension/config.ts:140-142` refuses any
`defaultSubagentContext` other than `"fresh"`/`"fork"` with
`config.defaultSubagentContext must be "fresh" or "fork"`.
`git show v0.57.0:src/runs/foreground/subagent-executor.ts` **`:2521`**
`resolveAgentDefaultContextPolicy` adds the `params.context === "profile"` branch, which REQUIRES
every requested agent to declare `defaultContext` (**`:2532`**/**`:2537`**
``context: "profile" requires agent '<n>' to declare defaultContext.``) and ignores the config
default. `git show v0.57.0:src/extension/schemas.ts:319-322` declares
`enum: ["fresh", "fork", "profile"]`; the same enum is `["fresh","fork"]` at both v0.43.0 and v0.47.1.

**cyrup** — `crates/cyrup-ext-subagents/src/fork_context.rs:74-88` — the doc enumerates exactly three
rungs (*"1. `call_site_context` 2. `agent_default_context` 3. `ContextMode::default` (Fresh)"*) and
the body is `call_site_context.or(agent_default_context).unwrap_or_default()`: **no availability test
and no distinction between an explicit and an inherited `Fork`.** `resolve` (`:140-208`) then returns
`Err(SubagentError::ForkRequiresPersistedParent)` / `ForkRequiresLeaf` for either origin, and the
module doc at `:26-30` states it *"MUST fail hard rather than silently downgrading to fresh context"*.
Call sites propagate the error (`src/extension/executor/foreground.rs:156-159`,
`src/extension/executor/background.rs:104`).
`grep -rn 'can_prefer_fork\|implicit_fork\|default_subagent_context\|defaultSubagentContext' --include=*.rs`
→ **0 hits**. `src/extension/tool/schema.rs:399-403` declares `"enum": ["fresh", "fork"]` and the test
at `:741` pins that two-value enum.

**Impact** — Three user-visible behaviours. **(1)** An agent whose frontmatter says
`defaultContext: fork`, launched from a session that has not persisted yet (a brand-new session before
the first assistant append, or an in-memory session), runs **fresh** upstream and **errors out
entirely** in the port with "fork requires a persisted parent" — the user never asked for fork at the
call site, so the agent author's preference turns a working launch into a failed one. **(2)**
`subagents.defaultSubagentContext: "fork"` (or `"fresh"` to override agents that declare fork) has no
representation and is dropped. **(3)** `context: "profile"` is rejected by the port's closed enum, so
a caller cannot say "honour each agent's declared `defaultContext` and fail loudly if one has none."
`high` not `critical`: the port's behaviour is a loud, explicit error that aborts the launch before
any subprocess spawns (`exec/mod.rs:1402` proves zero filesystem side effects) — a failed launch the
user must retry, not silent corruption.

**Fix** — Split explicit from inherited in `resolve_effective_context`: keep the strict path for an
explicit call-site `Fork`, and for an inherited preference test availability first (a
`can_prefer_fork(session)` mirroring `canPreferForkFromSnapshot` — persisted parent file plus leaf id)
and downgrade to `Fresh` when unavailable. Add `default_subagent_context` to
`SubagentExtensionConfig` **above** the agent default in the precedence chain, with upstream's
validation error. Add `"profile"` to the schema enum at `tool/schema.rs:399` and the policy branch
with pi's message, updating the pinning test at `:741`.

**Verify** — An agent with `defaultContext: fork` launched from an unpersisted session must run fresh,
not error; an explicit `context: "fork"` from the same session must still error.
`defaultSubagentContext: "fresh"` must override an agent that declares fork.
`context: "profile"` against an agent with no `defaultContext` must fail with pi's message.

**Relation to corpus** — New. No `SUBA` row covers fork-context resolution policy; `VL-S2`'s
`chatProgress`/workflow scope does not reach it. Merges the foreground-exec-lens and
shared-config-lens candidates, which are the same function.

---

## ~~SUBA-081~~ — ~~high~~ **PARTIALLY CLOSED 2026-09-04** — Ten settings-override fields never apply, and a legal upstream `tools: "inherit"` fails the settings load instead of being applied

> **PARTIALLY CLOSED 2026-09-04, cyrup HEAD `2571969`** (`.flux` records: `SUBA-081` moved to `done/`
> after QA — verified against the actual code, not the task record). **6 of the 10 landed.**
> `discovery/types.rs::AgentOverrideConfig` grew from 13 to 18 fields, adding `description` (`:608`,
> applied in `merge.rs:406-408` — deliberately NOT clearable, matching upstream's plain-string shape),
> `output` (`:613`, via `apply_output_override`), `default_reads` (`:617`), `extensions` (confirmed at
> `merge.rs` past the `tools` override block) and `tool_budget`. **`tools: "inherit"` is fixed
> properly, not merely un-crashed**: `ToolsOverrideField` (`types.rs:444-483`) is now a real 3-way enum
> (`Unset`/`ExplicitClear`/`Inherit`/`Value`) rather than the old `OverrideField`'s 2-way
> `Deserialize`, with the module doc at `:424-440` explaining why collapsing `"inherit"` into the clear
> sentinel would have inverted a security boundary. The false completeness claim this item's evidence
> quoted (`types.rs:411-414`, *"a field-for-field port … pi has no others"*) is deleted, replaced with
> an accurate per-field upstream citation list — the item's own Fix instruction "delete the
> completeness claim" is done.
>
> **Residual — 4 fields still not on the struct**, confirmed absent by the same grep this item used:
> `acceptance_role`, `output_mode`, `default_provider`, `fast`. `grep -c 'acceptance_role\|output_mode\|default_provider\|\bfast\b' discovery/types.rs` inside the
> `AgentOverrideConfig` block still returns 0. The severity note this item filed for the `"inherit"`
> hard-failure case (the sharpest of the ten) no longer applies since that specific case is fixed; the
> remaining four are the silent-no-op class only, which the item's own text already treats as milder
> than the `"inherit"` crash. Kept at `high` pending the next pass's judgement — no new observation
> changes the calculus, only the count of affected fields (10 → 4).

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** confirmed
**Subsystem** discovery / merge (settings overrides)
**Window** in-baseline (≤ v0.43.0) for `description`/`extensions`/`toolBudget`/`acceptanceRole`; **v0.47.1..v0.57.0** for `output`/`outputMode`/`defaultReads`/`defaultProvider`/`fast` and the `"inherit"` tools variant.

**upstream** — `git show v0.57.0:src/agents/agents.ts:81-101` — `BuiltinAgentOverrideConfig` has
**22** fields, read in full and in order: `description`, `output?: string | false`,
`outputMode?: OutputMode`, `defaultReads?: string[] | false`, `model`, `defaultProvider?: string | false`,
`fallbackModels`, `fast?: boolean`, `thinking`, `systemPromptMode`, `inheritProjectContext`,
`inheritSkills`, `defaultContext`, `acceptanceRole?: AcceptanceRole | false`, `disabled`,
`systemPrompt`, `skills`, **`tools?: string[] | false | "inherit"`**, `extensions?: string[] | false`,
`subagentOnlyExtensions`, `completionGuard`, `toolBudget?: ToolBudgetConfig | false`.
`git show v0.43.0:src/agents/agents.ts:82` opens the same interface with **17** fields, which already
include `description`, `extensions`, `toolBudget` and `acceptanceRole` — so those four were portable
at the measured baseline. `applyBuiltinOverride` applies each field, with `false` meaning delete;
**`:1237-1246`** `applyToolsOverride` treats the literal `"inherit"` specially:
```ts
if (toolsOverride === "inherit") { delete target.tools; delete target.mcpDirectTools; return; }
```
— drop the allowlist so the builtin inherits the parent's full tool set.

**cyrup** — `crates/cyrup-ext-subagents/src/discovery/types.rs:432-477 AgentOverrideConfig` declares
exactly **13** fields, read in full: `model`, `fallback_models`, `thinking`, `system_prompt_mode`,
`inherit_project_context`, `inherit_skills`, `default_context`, `disabled`, `system_prompt`, `skills`,
`tools`, `subagent_only_extensions`, `completion_guard` — with **no** `description`, `extensions`,
`tool_budget`, `acceptance_role`, `output`, `output_mode`, `default_reads`, `default_provider` or
`fast`. `src/discovery/merge.rs:387-464 apply_builtin_override` applies only those 13. **The struct's
own doc at `types.rs:411-414` claims it is** *"a field-for-field port of pi's
`BuiltinAgentOverrideConfig` (`agents.ts:82-100`) — every field below is exactly one pi override
field, and pi has no others"* — which is false even at the v0.43.0 baseline the port measured
against. For `"inherit"`: `tools` is `OverrideField<Vec<ToolRef>>` (`:470`); `OverrideField`'s
hand-written `Deserialize` (`types.rs:359-386`) is an untagged `enum Raw<U> { Value(U), Clear(OverrideClearSentinel) }`
and `OverrideClearSentinel` (`types.rs:299-330`) accepts EXCLUSIVELY the JSON literal `false` — so the
string `"inherit"` matches neither arm, `serde_json::from_value` fails, and
`src/discovery/mod.rs:668-669` maps that to `SubagentError::MalformedSettings`, which
`mod.rs:787-794` propagates out of the settings read as a hard error.

**Impact** —
`subagents.agentOverrides.reviewer = {description: "…", extensions: ["./x.ts"], toolBudget: {…},
acceptanceRole: "read-only", outputMode: "file-only", defaultProvider: "anthropic"}` changes six real
things upstream and **silently changes nothing** in the port — no error tells the operator the
override did nothing. Worse, `subagents.agentOverrides.worker.tools = "inherit"` — a legal upstream
value meaning "drop the allowlist, inherit the parent's tools" — does not merely fail to apply: it
**fails the settings load** with `MalformedSettings`, so a pi-shaped `settings.json` takes agent
discovery down until the key is removed. `high` not `critical`: the nine ignored fields are a silent
config no-op rather than wrong run output, and the `"inherit"` case produces a named, surfaced error
rather than a crash — but a legal pi-shaped settings file killing discovery is squarely above
`medium`.

**Fix** — Add the nine missing fields to `AgentOverrideConfig` with the right `OverrideField` /
plain-bool shapes and apply each in `apply_builtin_override`, mirroring upstream's per-field
validators and their error text. Extend `OverrideField`'s `Deserialize` (or add a `tools`-specific
enum) with an `Inherit` arm accepting the literal string `"inherit"`, and have
`apply_builtin_override` clear both `tools` and `mcp_direct_tools` on that arm. **Delete the
completeness claim at `types.rs:411-414`** and replace it with an assertion pinned against a
checked-in copy of upstream's field list, so the set cannot silently drift again.

**Verify** — Each of the nine fields set in `agentOverrides` must be observable on the merged agent
(`acceptanceRole: false` must delete, per upstream's `| false` semantics).
`agentOverrides.<n>.tools = "inherit"` must load successfully and produce an agent with no tool
allowlist and no MCP direct tools. A settings file containing all 22 upstream fields must load
without error.

**Relation to corpus** — New. `SUBA-061` names four *config* keys, not override fields; nothing in
the corpus covers `AgentOverrideConfig`'s field set, and the port's own doc comment asserting
completeness is the reason no prior pass caught it. Merges the two discovery-lens override candidates
because they are one struct and one apply function. **Note:** the ignored `acceptanceRole` override is
permission-adjacent — an operator who writes `acceptanceRole: "read-only"` gets no restriction and no
error — but the port has no `acceptance_role` on `AgentDefinition` at all, which is `SUBA-082`'s
broader gap; land the two together.

---

## ~~SUBA-082~~ — ~~high~~ **CLOSED 2026-09-04** — Agent `acceptanceRole:` and `acceptance:` frontmatter are not in the schema, so the acceptance classifier is driven purely by the agent-name regex

> **PROMOTED OUT OF `## Carried — NOT adversarially verified`, CONFIRMED, AND CLOSED 2026-09-04 —
> landing commit `5a4ae4ed`, cyrup code HEAD `275c1f85`.** The carried filing was held to the
> confirmed bar before any code was written: every line it quoted was re-read with
> `git -C tmp/pi-subagents show v0.57.0:<path>` and again at v0.64.0, the port side was re-read at
> `a4805955`, and the verdict was CONFIRMED exactly as filed — the row's v0.57.0 line numbers
> (`agents.ts:144-145`, `:1873-1884`, `:2011-2014`; `agent-serializer.ts:24-25`) are all exact; at
> v0.64.0 the same code sits at `agents.ts:156-157`, `:1913-1924`, `:2046-2050` and
> `agent-serializer.ts:26-27`, unchanged in substance. Window claim verified: `3c635cc1`
> (*feat: add per-agent acceptance roles (#481)*, 2026-07-15) first appears in **v0.35.0**, so
> in-baseline as filed. Then ported, then both sides re-read after the port by an independent review.

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed (2026-09-04, both tags)
**Subsystem** discovery / acceptance
**Window** in-baseline (≤ v0.43.0) · `3c635cc1` → v0.35.0

**upstream (re-read at v0.57.0 and v0.64.0)** — `AgentConfig.defaultAcceptance?: AcceptanceInput` and
`acceptanceRole?: AcceptanceRole` (`v0.57.0:src/agents/agents.ts:144-145` = `v0.64.0:156-157`;
`AcceptanceRole = "read-only" | "writer"`, `v0.64.0:src/shared/types.ts:31`).
`parseAgentAcceptanceFrontmatter` (`v0.57.0:agents.ts:1873-1884` = `v0.64.0:1913-1924`): blank →
undefined; YAML-parse; throw ``Agent '<name>' has invalid acceptance frontmatter: …``; then
`validateAcceptanceInput(parsed, `Agent '<name>' acceptance frontmatter`)` with the errors joined.
`acceptanceRole` is compared exactly and throws ``Agent '${localName}' has invalid acceptanceRole
frontmatter; expected 'read-only' or 'writer'.`` (`v0.57.0:2011-2014` = `v0.64.0:2046-2050`); both
are `KNOWN_FIELDS` (`agent-serializer.ts:24-25` / `:26-27`) and `serializeAgent` re-emits them
(`v0.64.0:agent-serializer.ts:103-110`). The role is the PRIMARY input to the classifier —
`v0.57.0:src/runs/shared/acceptance.ts:77-104 inferLevel`: intent classified on `"worker"` when a
role is declared (`:90`), `rolePatchTask` with `stripSeverityCompounds` (`:93-96`,
`task-intent.ts:78-82`), `readOnlyAgent = role === "read-only" || (role === undefined && /\b(?:reviewer|oracle|scout|researcher|analyst)\b/)`
(`:98-99`), `writeTask` gains a `writer` arm (`:100-102`), `roleResolvesReadOnly` cancels the
dynamic/dynamicGroup escalations (`:104-109`), reasons *declared writer acceptance role* (`:124`) /
*declared read-only acceptance role* (`:133`). Threaded into every launch:
`v0.64.0:src/runs/foreground/execution.ts:1834`, every background launch in
`src/runs/background/async-execution.ts:978,1036,1044,1122,1130,1768,1799`, and the single-agent
launch default `applySingleAgentLaunchDefaults` (`v0.64.0:src/runs/foreground/subagent-executor.ts:2690-2692`:
`params.acceptance === undefined && agent.defaultAcceptance !== undefined`). Mirrored test:
`v0.64.0:test/unit/acceptance.test.ts:91-165`.

**cyrup before `5a4ae4ed` (re-read at `a4805955`)** — `discovery/frontmatter.rs:72-127 KNOWN_FIELDS`
had neither key (its doc claimed to mirror upstream's list "exactly" — false); both were demoted to
`extra_fields` unvalidated (`:1075-1081`); `AgentDefinition` (`discovery/types.rs:939`) had no
`acceptance_role`/`default_acceptance`; `AcceptanceResolveInput` (`exec/acceptance/model/level.rs:43-51`)
had no role, and `:81-91` said so in a comment; `resolve_run_acceptance` (`exec/mod.rs:217-223`)
classified on `&agent.name` + task only; `single_agent_launch_defaults`
(`extension/executor/nested_control.rs:148-173`) had no acceptance slot; `serialize_agent` had no
emission arm, so the keys would have been deleted on the first management rewrite.
`rg -n -i 'acceptance_role|acceptanceRole|default_acceptance|defaultAcceptance' src` → 9 hits, all
comments/doc/one settings fixture asserting nothing.

**What landed (`5a4ae4ed`, 43 files, +1399/−57; re-read at `275c1f85`)** —
`discovery/types.rs` `AgentDefinition::{default_acceptance: Option<serde_json::Value>, acceptance_role: Option<AcceptanceRole>}`
(`:1126`, `:1132`); `exec/acceptance/model/types.rs` `AcceptanceRole {ReadOnly, Writer}` +
`parse_exact`. `discovery/frontmatter.rs` `KNOWN_FIELDS` now carries `"acceptance"`/`"acceptanceRole"`
(`:130-131`); `parse_agent_acceptance_frontmatter` (called at `:1229`) ports the YAML-parse →
`Agent '<name>' has invalid acceptance frontmatter: …` → `validate_acceptance_input(&value, "Agent '<name>' acceptance frontmatter")`
chain (a new `serde_yml = "0.0.13"` dependency, so both `checked` scalars and
`{ level: "none", reason: … }` maps parse as upstream's `parseYaml` does); `acceptanceRole` (`:1247`)
uses upstream's verbatim refusal, under the crate's existing per-file-skip `[CYRUP-DELTA]`.
`discovery/management/frontmatter_write.rs` `serialize_agent` emits `acceptance:` (compact JSON for
an object, bare scalar otherwise, `""` under preserve) and `acceptanceRole:`; `agent_crud.rs`
`merge_fields` preserves both across an update; `management/render.rs` adds `Acceptance:` /
`Acceptance role:` detail lines (`agent-management.ts:901-902`). `exec/agent_config.rs`
`AgentConfig` and the hop-2 `ResolvedAgentPersona` carry both, so a background child sees the role.
`exec/acceptance/model/level.rs` `AcceptanceResolveInput::acceptance_role` and `infer_level`
re-ported line for line from `v0.57.0:acceptance.ts:77-104` (with `strip_severity_compounds` added
to `exec/task_intent.rs`); `lattice/contract.rs` `AcceptanceContract::{heuristic_default_for_role, resolve_effective_for_role}`;
`exec/mod.rs` `resolve_run_acceptance` passes `agent.acceptance_role`. Launch default:
`nested_control.rs` `single_agent_launch_defaults` returns a fourth slot and
`extension/tool/routing.rs` `route_single` fills `p.acceptance` only when the call omitted it —
single-agent only, never chain/parallel, per `docs/agents.md:326`.

**Verify (each fails at `a4805955` by construction — every test names a symbol absent there — and
passes at `275c1f85`)** — `src/tests/acceptance_role_inference.rs` (new, seven cases mirroring
`v0.64.0:test/unit/acceptance.test.ts:91-173`: explorer+read-only → read-only branch with reason
*declared read-only acceptance role*; reviewer+writer on "Handle the authentication flow" → `checked`,
*declared writer acceptance role*; worker+read-only on implementation wording → `checked` (task intent
wins); worker+writer on "Review only; do not edit files" → read-only branch; explorer+read-only +
"Audit the security posture" → not escalated; explorer+read-only + `dynamic` → not escalated);
`frontmatter.rs` `acceptance_role_frontmatter_parses_exactly_and_is_a_known_field`,
`invalid_acceptance_role_frontmatter_skips_the_file`,
`acceptance_frontmatter_parses_scalar_json_flow_map_and_block_defaults`,
`invalid_acceptance_frontmatter_skips_the_file_with_upstreams_message`;
`frontmatter_write.rs` `serialize_agent_round_trips_acceptance_and_acceptance_role`;
`exec/mod.rs` `run_sync_threads_the_agents_declared_acceptance_role_into_the_inferred_floor`;
`tests/read_only_agent_name_alternation.rs` now passes `acceptance_role: None` explicitly and is
unchanged otherwise (the `undefined` branch regression guard). Crate: 2630/2630 at `5a4ae4ed`.

**Falsification** — `acceptanceRole: writer` on a `security-reviewer` given "Handle the authentication
flow" must resolve `checked` with reason *declared writer acceptance role*; `acceptanceRole: read-only`
on a `worker` given "Explore the authentication flow" must take the read-only branch;
`acceptance: checked` on an agent file must reach `RunOptions::acceptance` for a `subagent({agent,
task})` call that omits `acceptance`. Any of those failing reopens the row.

**Residuals — recorded, not closed by this row.** (1) **`infer_level` is the v0.57.0/v0.62.0 body,
not v0.64.0's**: `0128385f` (#1799, first tag v0.63.0) makes the name-classified `readOnlyAgent` feed
`inferredReadOnly` (`v0.64.0:acceptance.ts:105`), adds a `dynamicResolvesReadOnly` guard
(`:107,110-111`) and returns level `none` instead of `attested` on the read-only branch (`:137`) —
that collides with the crate's deliberate always-attest lattice mapping (`lattice/contract.rs`
`None => Attested`), so it was left for its own row; the new tests assert branch/reason/evidence
rather than the level so they survive that port. Ownerless lead, see the summary blockquote. (2) The
`/run` slash surface does not apply the `acceptance:` launch default (its foreground branch uses the
flat legacy `run_foreground` signature) — same standing gap as `output`/`outputMode`/`skill` there.
(3) `SUBA-081`'s remainder, `agentOverrides.<n>.acceptanceRole` (incl. `false` to clear), is still not
modeled; the `types.rs` doc note now says so precisely. (4) Management create/update inputs
`config.acceptance`/`config.acceptanceRole` (`agent-management.ts:385-386,576-587` @v0.64.0) are not
ported; `merge_fields` preserves existing values. (5) `completion-guard.ts:78-80` `isWriterRole` is not
ported — its only consumer `validateImplementationToolContract` is unported (0 hits), so there is no
seam. (6) `spawn/chain_graph.rs` `evaluate_dynamic_group_acceptance` still passes role `None`
(upstream passes `step.parallel.acceptanceRole`, `subagent-runner.ts:4106` @v0.64.0) — the seam has
no persona map. (7) `strip_severity_compounds` is applied only inside `rolePatchTask`; upstream also
applies it in `classifyTaskMutationIntent`/`taskMayMutate` (`task-intent.ts:175,207` @v0.64.0,
`2318fb07` in v0.48.0) — noted in `exec/task_intent.rs`'s doc.

---

## ~~SUBA-083~~ — ~~high~~ **CLOSED 2026-09-04** — `asyncByDefault`'s default is inverted, and the documented `asyncByDefault:false` opt-out is a no-op

> **CLOSED 2026-09-04, cyrup HEAD `2571969`**, verified by reading the current code.
> `registration/mod.rs`'s `impl Default for SubagentExtensionConfig` now sets `async_by_default: true`
> (`:441`), matching upstream's `config.asyncByDefault !== false` semantics (absent key = true). The
> resolution site (`extension/tool/params.rs:337`,
> `async_param.unwrap_or(cfg.async_by_default)`) is unchanged in shape — only the default flipped —
> so a stock install now backgrounds by default and `asyncByDefault: false` is a real opt-out in both
> directions, closing the item's whole Verify recipe.

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** config / launch mode
**Window** in-baseline (≤ v0.43.0) — identical at `v0.43.0:src/extension/config.ts`.

**upstream** — `git show v0.57.0:src/extension/config.ts` **`:222-224`**:
```ts
export function resolveAsyncByDefault(config: Pick<ExtensionConfig, "asyncByDefault">): boolean {
	return config.asyncByDefault !== false;
}
```
— **an ABSENT key means TRUE.** `git show v0.57.0:src/extension/index.ts:9` states the contract in
the module header — *"Toggle: async parameter (default: true; set `asyncByDefault:false` in
config.json to opt out)"* — and the boolean is threaded into every launch surface:
`subagent-executor.ts`'s `const requestedAsync = params.async ?? asyncByDefault;`, the fanout-child
path and the slash bridge. `git show v0.57.0:src/extension/schemas.ts:324` repeats it in the `async`
param's own description: *"Run in background unless `asyncByDefault:false`."*

**cyrup** — `crates/cyrup-ext-subagents/src/registration/mod.rs:272` `async_by_default: false` inside
`impl Default for SubagentExtensionConfig`, pinned by the test at `:1016`
(`assert!(!cfg.async_by_default)`). `src/extension/tool/params.rs:337`
`let requested_async = async_param.unwrap_or(cfg.async_by_default);` — the same `??` shape with the
opposite tier-5 default. The field's doc comment (`registration/mod.rs:80-82`) describes the semantics
without noting the flip, and `grep -c 'CYRUP-DELTA' crates/cyrup-ext-subagents/src/registration/mod.rs`
→ **0**, so this is **not** a marked divergence. (Contrast the sibling field two lines down,
`max_subagent_spawns_per_session: 40`, whose doc cites `func-SA §4.7` as a cyrup requirement — that
one is a decision of record; this one is not.)

**Impact** — On a stock install with no `config.json`, `subagent({agent, task})` returns immediately
with an async run id upstream (the caller then waits or polls) and **blocks the parent turn until the
child finishes** in the port. Every launch surface — the tool, `/run`, fan-out children — takes the
opposite mode by default, and the `asyncByDefault: false` opt-out documented in upstream's own header
is inert in the port because the port already behaves that way. **Correction to the filing text,
applied:** the claim that "a user following upstream documentation cannot reach upstream's default
behaviour at all" is **false** — setting `asyncByDefault: true` does work in the port, proven by
`src/extension/tool/params.rs:532-544` (which deserializes the camelCase key from a real config value
and asserts an omitted `async` then backgrounds) and by
`crates/cyrup-it/tests/subagents/registration_commands_integration.rs:496-505`. The key is honoured in
both directions; only the absent-key default is inverted, which is what makes the documented
`false` opt-out a no-op. `high` not `critical`: no data loss, no wrong output, no bypass, no crash.

**Fix** — Either flip `registration/mod.rs:272` to `true` (and its pinning test at `:1016`), matching
`resolveAsyncByDefault`'s `!== false` semantics exactly — or, if the foreground default is an
intentional product decision, **write it down**: a `[CYRUP-DELTA]` block at the field naming the
divergence and its rationale, as the sibling `max_subagent_spawns_per_session` field does. What is not
acceptable is the current state: a silent flip of a documented default, with a doc comment that does
not mention it.

**Verify** — With no config file present, `subagent({agent, task})` must return an async run id
without blocking; with `asyncByDefault: false`, the same call must block until completion. Both
directions must be covered by the pinning test.

**Relation to corpus** — New. Not covered by `SUBA-061`'s four ignored keys — the key here **is**
honoured; its default is inverted.

---

## ~~SUBA-084~~ — ~~high~~ **CLOSED 2026-09-04** — Runtime agent registration is entirely absent: no `registerAgent` API, no `runtime` source tier, no runtime/configured collision checks

> **PROMOTED OUT OF `## Carried — NOT adversarially verified`, CONFIRMED, AND CLOSED 2026-09-04 —
> landing commit `dee8b9d0`, cyrup code HEAD `275c1f85`.** Verified on the confirmed bar before the
> port: `git -C tmp/pi-subagents show v0.57.0:src/agents/runtime-agent-registry.ts` is 424 lines (the
> filing said 418 — harmless) with the 32-field `RuntimeAgentDefinition` exactly as filed; v0.64.0's
> is 429 lines and 35 fields (+`excludeTools`, +`allowNestedSubagents`, +`inheritGlobalContext`,
> +`mutationTools`, −`defaultTurnBudget`) and adds a cross-extension EVENT bridge absent at v0.57.0.
> All five wiring claims verified at both tags (discovery merge, slash merge, management list section
> + merge, `sourceRank` runtime = 4, clear on cleanup). Port side at `a4805955`: zero hits for every
> symbol across the WHOLE workspace (`rg -e 'runtime_agent|RuntimeAgent|register_agent|registerAgent|runtime-agent-register|pi-subagents\.runtime' crates --glob '*.rs'`),
> `AgentSource` four variants, `TieredAgents`/`merge_tiers` four tiers, `AgentDiscoveryConfig`
> on-disk inputs only, the `SessionShutdown` arm (`extension/host/native_impl.rs:397-420`) clearing
> nothing agent-related, `lib.rs` exporting no registration API. Refutation attempts (generic bus
> mechanism, other names, the `registration` module, `BUILTIN_AGENT_NAMES`) all negative — nothing
> partial existed. **Effort corrected L → M**: one new module plus a fifth enum variant threaded through
> the exhaustive matches, which is what landed. Port target v0.64.0 per ADR-0006.

**Kind** not-ported · **Severity** high · **Effort** M *(filed L)* · **Confidence** confirmed (2026-09-04, both tags)
**Subsystem** discovery / runtime registry
**Window** v0.47.1..v0.57.0 · `2c031d06 (#1320)`

**upstream (re-read at v0.64.0; v0.57.0 diffed against it)** — `src/agents/runtime-agent-registry.ts`:
caps 200 / 128 / 4096 / 1 MiB / 8192 (`:10-16`); `RuntimeAgentDefinition` (`:18-54`); name-sensitive
defaults (`:78-88`); `validateString`/`StringList`/`PositiveInteger`/`Boolean` (`:116-146`);
`validateRunner` with fourteen refusals (`:156-184`); `validateAcceptance`/`validateToolBudget`
(`:186-196`); `validateDefinition` — supported set, unknown-field error, the five enum scalars
(`:198-285`); `normalizeAliases`/`identityKeys` (`:287-295`); `assertNoIdentityCollisions` /
`assertNoRuntimeCollision` / `assertNoBuiltinCollision` (`:297-323`); `toAgentConfig` stamping
`source: "runtime"` and `filePath: runtime:<name>` (`:325-369`); `registerRuntimeAgent` — name cap,
validate, code-owned-profile check, builtin collision, 200 cap, runtime collision, idempotent
`dispose()` by record identity (`:371-398`); `clearRuntimeAgentsForPi`/`listRuntimeAgentConfigs`
(`:400-406`); `assertNoConfiguredCollision` (`:408-421`); `mergeRuntimeAgents` — filters disabled,
no-op when empty, fails closed (`:423-429`). Public API `src/api/agents.ts:2,12`. `AgentSource`
includes `"runtime"` (`agents.ts:30`) at `sourceRank` 4 (`:687`). Wired: `extension/index.ts:528-546`
(`discoverAgentsForRuntime` re-snapshots all four tiers when the registry is non-empty), `:971`
(clear in cleanup); `slash/slash-commands.ts:120-130`; `agent-management.ts:132-141,254,744,849`.
Tests `test/unit/runtime-agent-registration.test.ts:81-104,220-230,274-303,305-319,321-329,331-356,358-366`.

**What landed (`dee8b9d0`, 22 files, +2460/−30; re-read at `275c1f85`)** — NEW
`discovery/runtime_registry.rs`: the constants (`MAX_RUNTIME_AGENTS_PER_OWNER = 200` at `:60` …), the
35-field `RuntimeAgentDefinition` (`:162`) whose `to_value()` feeds ONE Value-based
`validate_definition` so typed and untyped input share upstream's messages verbatim,
`validate_runner`, `normalize_aliases`/`identity_keys`, the three `assert_no_*_collision` checks plus
`assert_no_configured_collision`, `to_agent_definition` (source `Runtime`, `file_path`
`runtime:<name>`, name-sensitive defaults, `thinking:false` → `"off"`),
`RuntimeAgentRegistry::{register, register_value, list, clear}` in upstream's check order,
`RuntimeAgentRegistration::dispose` (idempotent, record-id based), `merge_runtime_agents`.
`discovery/types.rs` `AgentSource::Runtime` (`:59`) threaded through every exhaustive match
(`discovery/mod.rs::source_rank` Runtime = 4, `chain_run_precedence`, `merge.rs::apply_overrides`,
`management/helpers.rs::source_str` `"runtime"`, `management/handlers.rs::agent_in_list_scope` always
visible, `registration/doctor.rs::SourceCounts::record`). `AgentDiscoveryConfig::runtime_agents` and
the merge inside `run_discovery` — the single seam every cyrup discovery consumer shares — against all
four tiers at `Both` scope when non-empty, as `index.ts:528-546`. `SubagentExecutor` owns the registry
(upstream's per-`ExtensionAPI` WeakMap partition); `extension/executor/resolve.rs::discovery_config`
is now `&self` and fills the list; `session_state.rs::teardown_session` clears it (reached from the
`SessionShutdown` arm). Public `SubagentsExtension::register_agent` / `SubagentExecutor::register_agent`
and `lib.rs` re-exports of `RuntimeAgentDefinition`/`RuntimeAgentRegistration`/`RuntimeAgentRegistry`/`RuntimeThinking`.

**Verify (21 tests; each names a symbol absent at `a4805955`, so the module fails to compile there;
all pass at `275c1f85`)** — unit, in `runtime_registry.rs` (9): definition/name/runner validation
against upstream's strings, nested-field labels, unrepresentable-field refusal, reserved-name guard
before builtin collision, merge no-op/append/identity refusal. Integration,
`src/tests/runtime_agent_registration_integration.rs` (12), ported 1:1 from upstream's test file:
`runtime_agent_reaches_discovery_without_writing_config`, `runtime_agent_is_listed_by_management_list`
(all three scopes), `fails_closed_for_builtin_identity`, `fails_closed_for_duplicate_runtime_identity`,
`rejects_malformed_nested_definition_fields`,
`fails_closed_when_cwd_discovery_introduces_configured_collision`,
`fails_closed_against_configured_agent_hidden_by_scope`,
`management_list_fails_closed_on_scoped_configured_collision`, `dispose_is_idempotent_and_removes_agent`,
`registry_caps_at_200_per_owner`, `executor_discovery_sees_registered_agent_and_session_shutdown_clears_it`,
`runtime_source_outranks_project_in_name_resolution`; `merge.rs` `merge_tiers_matches_precedence_rank_ordering`
pins the fifth variant. Crate 2651/2651 at `dee8b9d0`.

**Residuals — recorded, not closed by this row.** (1) **The v0.64.0 EVENT registration bridge is not
ported** — `runtime-agent-events.ts:4-5,29-48,51-70` and `api/agents.ts:3-10` rely on a synchronous
emit whose handler mutates `request.result` in place; cyrup's `SharedBus`
(`crates/cyrup-ext/src/bus.rs:83-91`, `[CYRUP-DELTA]`) queues emits and passes payloads by value, so
this needs a request/response topic design. Ownerless lead, see the summary blockquote (upstream
tests `:143-218` cover it). (2) **Five definition fields have no `AgentDefinition` landing and are
refused, not dropped**: `mcpDirectTools`, `inheritGlobalContext`, `mutationTools`, `skillPath`,
`defaultToolTimeoutMs` — validated with upstream's messages, then refused by name with a
`[CYRUP-DELTA] SUBA-084` error (`UNREPRESENTABLE_FIELDS`). The verifier's list of nine shrank to five
because `SUBA-082` and `SUBA-092` landed first; drop each from the list when its field lands. (3)
Builtin-collision roster is cyrup's seven shipped names; upstream's `builtin-names.ts` also holds the
six code-owned adapter names cyrup does not ship (`SUBA-074` stage 2) — the read-only ones are still
refused via `validate_code_owned_profile_runner`, the `-writer` twins are not. (4) The `models` report
has no `source: runtime agent config` label (`AgentModelSourceInfo` has no runtime variant; the agent
is still listed). (5) `AgentSource::Runtime` shares `precedence_rank` 0 with `Project` for totality;
it never enters `merge_tiers` (pinned). (6) Unknown-field messages list keys sorted rather than in
insertion order — text-only.

---

## ~~SUBA-085~~ — ~~high~~ **CLOSED 2026-09-04** — `mission.resolve-decision` unported: a mission decision is write-once and permanently open, so the goal driver proposes the same next action forever

> **CLOSED 2026-09-04, landing commit `5e3aa1c8`, re-read at cyrup code HEAD `275c1f85`.** Upstream
> re-read at both v0.57.0 and v0.64.0 with `git -C tmp/pi-subagents show`; the three mission files
> are byte-identical between the two tags (`git diff --stat v0.57.0 v0.64.0 -- src/missions/{actions,store,types}.ts`
> is empty), and `git log -S needs_decision v0.43.0..v0.47.1 -- src/missions/store.ts` = `1dec33dd`,
> confirming the window. **What landed**: `src/missions/types.rs:693 MissionDecisionResolution {id, resolution}`
> and `:732 MissionUpdateInput::resolve_decision` (counted by `is_empty()` at `:755`) —
> `missions/types.ts:188`; `src/missions/store.rs:1091-1117` the resolve block after the append loop
> (find by id; `Decision '<id>' was not found in mission '<id>'` / `Decision '<id>' is already resolved`
> verbatim; `status = Resolved`, `resolved_at = created_at`, trimmed resolution through
> `required_string(.., "mission.update.resolveDecision.resolution")`) — `store.ts:497-508`;
> `store.rs:1162-1192` the decision status gate (`has_open_decisions` / `candidate_status` /
> held `needs_decision`; back to `active` when the last open decision resolves) — `store.ts:521-529`,
> replacing the v0.43.0-era `update.status.unwrap_or(current.status)`; `src/missions/actions.rs:56
> MISSION_ACTIONS` (seven, upstream's order) and `:69 MUTATING_MISSION_ACTIONS` (five), `:89
> MissionAction::ResolveDecision`, `:977-1016` the handler arm in upstream's check order (mission id →
> `validate_mission_id(params.id, "id")` → `mission.resolve-decision requires a non-empty summary` →
> store) with the `Resolved decision <id> for mission <id>.` receipt — `actions.ts:32-40`, `:391-397`;
> `actions.rs:759 format_mission` `; resolution: <text>` suffix (`actions.ts:314`) and `:867`
> `mission.list`'s `decisions: N open, M resolved` tally (`:361-366`); `src/extension/tool/text.rs:227
> SUBAGENT_ACTIONS` carries the verb in pi's position (`shared/types.ts:2715`), from which
> `schema.rs` derives the action enum; `routing.rs:892` dispatches it through the `mission.*` arm and
> `is_mutating()` refuses it child-safe (`subagent-executor.ts:197`); `resources/docs/missions.md`
> and `tool-reference.md` advertise it. **The goal driver needed no change** — `mission_state_action`
> still returns the first OPEN decision (`goal-driver.ts:94-95` @v0.64.0), so resolving is what moves
> it on. **Verify, each clause a passing test**: next ready action moves past the decision
> (`goal_driver.rs:876 resolving_the_open_decision_moves_the_next_ready_action_past_it`); empty
> `summary` fails with upstream's message (`actions.rs:1244`, `routing_tests.rs:939`); unknown id fails
> rather than no-ops (`actions.rs:1244`, `store.rs:1749`); plus `store.rs:1719/:1797/:1847`,
> `actions.rs:1173`, `mission_action_vocabulary_matches_upstream_exactly` (7 entries),
> `child_safe_mission_gating_matches_upstreams_mutating_set` (5 verbs). Nine tests, each naming a
> symbol absent at `a4805955`; crate 2601/2601 at `5e3aa1c8`. **Falsification**: any of those going
> red, or `MissionDecisionStatus::Resolved` again produced only by the on-disk parser. **Behavioural
> note, upstream-faithful but new in cyrup**: `mission.close … completed` while a decision is open now
> yields `needs_decision` and the receipt says so — `decisionStatus`'s gate. **Not this row**: other
> `v0.47.1..v0.64.0` `store.ts` changes (`upsertWorkflowChildren`, `projectMissionDirectory`, the
> longer `MissionNotFoundError` text) are untouched.

**Kind** not-ported · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** missions
**Window** v0.43.0..v0.47.1 (`1dec33dd feat: add mission dispatch ledger`).

**upstream** — `git show v0.57.0:src/missions/actions.ts` **`:32-39`** `MISSION_ACTIONS` has **seven**
entries — `mission.create`, `mission.list`, `mission.show`, `mission.update`,
**`mission.resolve-decision`**, `mission.attach-run`, `mission.close` — and the handler at
**`:391-397`**:
```ts
if (action === "mission.resolve-decision") {
	const missionId = requireMissionId(params);
	const decisionId = validateMissionId(params.id, "id");
	if (typeof params.summary !== "string" || !params.summary.trim())
		throw new Error("mission.resolve-decision requires a non-empty summary");
	const record = updateMission(location, missionId, { resolveDecision: { id: decisionId, resolution: params.summary.trim() } });
	return textResult(`Resolved decision ${decisionId} for mission ${record.id}. …`);
}
```
`MissionUpdateInput` carries `resolveDecision?: { id: string; resolution: string }`, and the verb is
listed in `MUTATING_MANAGEMENT_ACTIONS` (`subagent-executor.ts`) and in `SUBAGENT_ACTIONS`
(`shared/types.ts`).

**cyrup** — `grep -rn 'resolve_decision\|ResolveDecision' --include=*.rs crates/cyrup-ext-subagents/src`
→ **0 hits**. `src/missions/types.rs:700-717 MissionUpdateInput` carries
`add_decisions: Vec<MissionDecisionInput>` with the doc *"Append decisions (always as NEW, open
decisions with fresh ids)"* and has **no** `resolve_decision` field; `is_empty()` at `:721-737`
enumerates every field and confirms the set is closed. `MissionDecision` does carry
`status: Open|Resolved`, `resolved_at` and `resolution`, but `MissionDecisionStatus::Resolved` is
produced at **exactly one site** — `src/missions/store.rs:355`, the on-disk PARSER
(`Some("resolved") => …`) — never by a mutation. `src/extension/tool/text.rs:187-229` advertises six
`mission.*` verbs, not seven.

**Impact** — In cyrup a mission decision can be **opened and never closed**.
`src/missions/goal_driver.rs:382-394` computes the mission's next ready action as
`record.decisions.iter().find(|item| item.status == MissionDecisionStatus::Open)` — and since nothing
can flip that status, a mission that ever records one decision returns that same decision as its next
ready action on every subsequent evaluation, and its autonomous progression is wedged. Upstream clears
it with one `mission.resolve-decision` call. There is no workaround under another name: `mission.update`
can only append new open decisions. `high` not `critical`: nothing is lost (the decision persists
correctly, it simply cannot be closed), there is no bypass and no panic — it is a functional stall of
autonomous progression plus a permanently stale continuation notice.

**Fix** — Add `resolve_decision: Option<MissionDecisionResolution>` to `MissionUpdateInput` (and to
`is_empty()`), implement the find/guard/mutate block in `store.rs` mirroring upstream's
(`status = Resolved`, `resolved_at`, `resolution`), add the seventh enum variant and its wire strings
in `missions/actions.rs`, `extension/tool/text.rs` and `extension/tool/schema.rs`, and reproduce the
non-empty-summary and unknown-id errors verbatim.

**Verify** — Create a mission, record one decision, resolve it, and assert `goal_driver`'s next ready
action moves past it; a `mission.resolve-decision` with an empty `summary` must fail with upstream's
message; one with an unknown decision id must fail rather than silently no-op.

**Relation to corpus** — Discharges one of the seven unowned verbs `SUBA-005` (tracker) explicitly
owes an owner for. `SUBA-005` proposes no schedulable work by its own reclassification, so this is
the first schedulable filing of the behaviour and is not a duplicate of a counted row.

> **RE-VERIFIED OPEN 2026-09-04, cyrup HEAD `2571969`.** `grep -rn 'resolve_decision\|ResolveDecision'
> crates/cyrup-ext-subagents/src/missions/` is still 0 hits; `MissionUpdateInput` (`missions/types.rs:689-717`)
> still has no `resolve_decision` field (`add_decisions` only); `extension/tool/text.rs`'s advertised
> `mission.*` verbs are still the same six (`create`/`list`/`show`/`update`/`attach-run`/`close`), no
> `mission.resolve-decision`. Unchanged, no code found closing it.

---

## ~~SUBA-086~~ — ~~high~~ **CLOSED 2026-09-04** — Per-agent parse diagnostics are absent: a malformed agent file is silently degraded to defaults instead of being reported by name and blocking its own agent name

> **PROMOTED OUT OF `## Carried — NOT adversarially verified`, CONFIRMED WITH THREE CORRECTIONS, AND
> CLOSED 2026-09-04 — landing commit `275c1f85` (= cyrup code HEAD).** Upstream: the filing's v0.57.0
> lines (`agents.ts:229-234`, `:244-264`, `:1923-2110`, `:2106`, `:2267-2273`;
> `agent-management.ts:760-764`) are all accurate; at v0.64.0 the same machinery is
> `agents.ts:238-278` (type, `AGENT_SOURCE_PRIORITY`, `agentDefinitionPriority`,
> `findBlockingAgentDiagnostic` — byte-identical between the tags), `:1959-2154` (per-file try/catch,
> catch at `:2149-2151`), `:2651-2662` (`discoveryDiagnostics` scope filter), `:2670`/`:2705-2710`;
> `agent-management.ts:177-181`, `:818-825`, `:946`, `:985-989`, `:1074`, `:1084-1089`;
> `subagent-executor.ts:2336-2350`; `preflight.ts:264-271`; `slash-commands.ts:891-896`;
> `doctor.ts:146-150`. **Corrections to the filed text.** (a) "every field parser degrades a bad value
> to None / agent loads with defaults" was STALE at `a4805955` for the fields `SUBA-073`/`SUBA-074`
> ported — `package`, `toolBudget`, `turnBudget`, `permission(s)`, `runner`, code-owned squat, `async`,
> `timeoutMs` did a per-FILE `tracing::warn! + return None` (`frontmatter.rs:868-1052`,
> `[CYRUP-DELTA]`); the warn reached only the tracing log, never `list`/`get`/doctor/the delegation
> error, and the dropped file made the name fall through to a lower-tier definition or
> `AgentNotFound` — so the user-visible outcome the row described was exact, the mechanism was not.
> (b) `outputMode`, `toolTimeoutMs`, `fast`, `allowNestedSubagents` (and `SUBA-082`'s two keys) DID
> match the "coerced to absent" description — unparsed, into `extra_fields`, no warn. (c) The example
> `defaultContext: forked` is wrong: upstream also degrades an unrecognised `defaultContext` silently
> (`v0.64.0:agents.ts:2011-2015`); `timeoutMs: 30s` and `outputMode: file` are valid examples.
> Severity `high` stands — a broken project-tier override of a builtin/user name silently ran the
> unbroken lower-tier agent (wrong agent runs). Port target v0.64.0.

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed (2026-09-04, both tags)
**Subsystem** discovery / diagnostics
**Window** v0.47.1..v0.57.0 · `e973fa3c`

**What landed (`275c1f85`, 10 files, +1089/−173)** — `discovery/types.rs:1353
AgentDiscoveryDiagnostic { source, file_path, error, name, runtime_name, package_specified, discovery_priority }`
+ `label()` (`agents.ts:238-249`). `discovery/frontmatter.rs:869 parse_agent_file_checked -> Result<Option<AgentDefinition>, AgentDiscoveryDiagnostic>`:
every former warn-and-skip site returns the diagnostic with pi's verbatim message (package now carries
`identity.ts:15`'s `Agent '<n>' package is invalid after sanitization.`), the three throws cyrup never
checked are added — `toolTimeoutMs` (`:1186` = `agents.ts:2031-2038`, ≤ 2147483647), `outputMode`
(`:1199` = `:2041-2044`), `fast` (`:1212` = `:2057-2062`) — and missing name/description stays
`Ok(None)` (`:1970-1972`); `parse_agent_file` (`:787`) is the diagnostic-dropping wrapper for the ~20
CRUD/serializer callers. `discovery/mod.rs:1107 AgentFileScan`, `:1134 walk_agent_dir_checked`,
`walk_agent_dirs`, `expand_manifest_agent_entry`, `scan_{builtin,package}_agents_checked`,
`:1448 scan_agent_tiers_scoped -> (TieredAgents, Vec<AgentDiscoveryDiagnostic>)`, `run_discovery` →
`AgentDiscoveryResult::agent_diagnostics` (`:1415`), scope-narrowed for free because the excluded tier
is never walked. `mod.rs:595 find_blocking_agent_diagnostic` + `agent_definition_priority`
(rank × 1 000 000, builtin 0 / package 1 / user 2 / project 3 / runtime 4 via `source_rank` — NOT the
inverted `AgentSource::precedence_rank`) = `agents.ts:251-278`; `mod.rs:551 blocking_candidates` =
the Found/Ambiguous/NotFound candidate rule of `subagent-executor.ts:2337-2338`. Consumers:
`management/handlers.rs:57 append_agent_diagnostic_lines` (`Invalid agent definitions:` /
`- <name ?? file_path> (<source>): <error>`, `:818-825`) — list appends it after Chains and before
proactive suggestions, UNFILTERED as `:946`; `:42 diagnostics_for_scope` (`:177-181`); `handle_get`
(`:264`) blocking check, raw then `sanitize_name`, BEFORE ambiguity/not-found (`:1084-1089`);
`handle_models` (`:422`) append when no agent requested (`:1074`). `extension/tool/routing.rs:151
canonicalize_execution_params` keeps the whole discovery result and checks the blocking diagnostic
FIRST — `Agent '<name>' has invalid configuration: <error>` + ` (<location>)`
(`subagent-executor.ts:2336-2350`); `extension/executor/resolve.rs:223 resolve_agent_with_model_scope`
the same for background/chain/slash launches via `error.rs:60 SubagentError::InvalidAgentConfiguration { name, error }`;
`registration/doctor.rs:1025` prints `- invalid agent <name> (<source>): <error>` (`doctor.ts:146-150`).

**Verify (15 tests; 11 name symbols with 0 `git grep` hits at `a4805955` so cannot compile there, 4
assert strings with 0 hits there; all pass at `275c1f85`, crate 2666/2666)** — `frontmatter.rs`:
`invalid_timeout_ms_is_reported_as_a_named_diagnostic_and_the_wrapper_still_skips`,
`every_upstream_throw_carries_its_verbatim_message` (outputMode/toolTimeoutMs/fast/allowNestedSubagents/async/permission+permissions/acceptanceRole/toolBudget),
`valid_tool_timeout_output_mode_and_fast_still_round_trip_as_extra_fields`,
`invalid_package_diagnostic_carries_upstreams_message_and_package_specified`,
`a_packaged_agents_diagnostic_carries_its_runtime_name`, `missing_description_is_still_a_silent_skip_not_a_diagnostic`;
`discovery/mod.rs`: `a_malformed_project_agent_file_is_reported_by_name_and_scoped_like_upstream`,
`a_broken_higher_tier_definition_blocks_the_valid_lower_tier_one`,
`a_broken_lower_tier_definition_does_not_block_the_valid_higher_tier_one`,
`find_blocking_agent_diagnostic_with_no_candidates_returns_the_trimmed_name_match`,
`a_packaged_diagnostic_matches_by_runtime_name_and_gates_local_name_on_a_matching_candidate`;
`management/mod.rs` `list_and_models_render_invalid_agent_definitions_and_get_refuses_a_blocked_name`;
`doctor.rs` `build_doctor_report_lists_each_invalid_agent_definition`; `resolve.rs`
`resolve_agent_refuses_a_name_whose_outranking_definition_is_malformed`; `routing_tests.rs`
`dispatch_refuses_an_agent_whose_outranking_definition_is_malformed` (incl. the `(task 2)` suffix).
Existing frontmatter skip tests still pass via the wrapper; doctor count assertions unchanged.

**Residuals — recorded, not closed by this row.** (1) `discovery_priority` is always `None`: cyrup's
`AgentDefinition` carries no per-directory ordinal (pi stamps it at `agents.ts:2471,2476`), so
blocking reduces to source rank — exact for every cross-tier collision, differs only for a same-name
collision across two directories of the SAME tier. (2) Valid `toolTimeoutMs`/`outputMode`/`fast` are
validated only and still round-trip via `extra_fields` (deliberately not added to `KNOWN_FIELDS`, since
no typed field consumes them yet); wiring them is separate work. (3) cyrup's `get`/`models` take no
`agentScope`, so their `diagnostics_for_scope` filter is the `both` identity. (4) No preflight API in
cyrup (`preflight.ts:267` has no seam); the slash path is covered through `resolve.rs`. (5) Upstream
`parsePackageName` treats a whitespace-only `package:` as an error; cyrup treats it as absent —
pre-existing.

---

## ~~SUBA-087~~ — ~~medium~~ **PARTIALLY CLOSED 2026-09-04** — Child-scoped stop (`childId`) is unported: `stop` can only terminate an entire async run and its whole descendant subtree

> **PROMOTED OUT OF `## Carried — NOT adversarially verified`, CONFIRMED WITH ONE CORRECTION, AND
> PARTIALLY CLOSED 2026-09-04 — landing commit `2d9d0d0a` (code), on top of `53fe416b`.**
> Upstream re-read with `git show` at BOTH tags. The filing's shapes are accurate; its one filing
> error is the directory of `async-stop-action.ts` — it is `src/runs/foreground/`, not
> `runs/background/` (content and range `:24-86` exact at both tags, byte-identical between them).
> `child-identity.ts` is 36 lines at v0.57.0 and 51 at v0.64.0: the only tag-to-tag difference is
> the `includeNested` option (`:27,34-42`), whose sole consumer is `slash/slash-commands.ts:1110`
> (cyrup's `/subagents-stop` takes no `childId`) — not ported. `subagent-runner.ts`'s ranges moved
> (`:2837-2887` as filed → `:2955-3031` at v0.64.0) and v0.64.0 additionally emits
> `subagent.child-status` (`:2956-2974`, shape `shared/types.ts:2299-2315`), which IS ported.
> Port-side at `53fe416b`, before the change: `rg 'childId|child_id|target_index|stop-requests|
> stop_requested|step\.stop' crates/cyrup-ext-subagents/src` found only steer's `target_index`,
> the workflow-graph node ids and one test name — nothing on the stop path, exactly as filed.
> Severity `medium` stands; effort M was exact. Port target v0.64.0.

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed (2026-09-04, both tags)
**Subsystem** background control / child-scoped stop
**Window** v0.47.1..v0.57.0 · `31a230cb (#1373)`, `de594cfd (#1375)`

**upstream (v0.64.0)** — `src/runs/shared/child-identity.ts:16-18` `asyncStatusChildIdentity` =
`step.workflowKey ?? step.runId ?? \`step:${index}\``; `:20-22` candidates (non-empty, de-duplicated);
`:24-47` `resolveAsyncStatusChild` → exactly-one match resolves, else `not_found` (`Child '<id>' was
not found under async run '<run>'.`, `:46`) / `ambiguous` (`Child '<id>' is ambiguous under async run
'<run>'.`, `:45`); `:49-51` `isStoppableAsyncStatusStep` = `pending|running`.
`src/runs/foreground/async-stop-action.ts:24-86` `stopAsyncRun(state, runId, kill, location,
childId)`: reconcile (`:33`), running|queued gate (`:41-47`), resolve (`:48-58`), stoppable gate with
`Child '${childId}' in async run '${status.runId}' is ${child.step.status}; stop only supports pending
or running children.` (`:59-65`), `deliverStopRequest({… targetIndex: child?.index, childId: child?.id
?? childId})` (`:68`), receipt `Stop requested for child ${child.id} in async run ${asyncId}.` (`:75`).
`src/runs/background/control-channel.ts:54-61` `StopRequest{type,ts,source,reason,targetIndex,childId}`;
`:98` `STOP_REQUESTS_DIR = "stop-requests"`; `:175-184` `assertChildIndex` (0..=1 000 000) and
`validStopChildId` (non-blank, ≤256 chars, no CR/LF); `:190-192` `<ts padStart 13>-<uuid>.json`;
`:297-310` `requestAsyncStop` (throws `stop childId must be a non-empty string without newlines and at
most 256 characters.`); `:553-567` `parseStopRequest` drops invalid targeting; `:569-620`
`consumeStopRequestFile`/`Payloads` (queue files name-sorted, then legacy `stop.json`, result sorted
by `ts`); `:642-653` `deliverStopRequest`; `:690` every drained request → `onStop`.
`src/runs/background/subagent-runner.ts:2595-2596` `activeChildStops`/`childStopRequests`; `:2955-3031`
`childStopTargetId`, `appendChildStatusEvent` (`subagent.child-status`, `version:1`, `reason:"user"`,
`source:"async"`), `markChildStopRequested` (pending|running gate, `stopRequested`/`stopRequestedAt`,
`subagent.step.stop_requested`), `markChildStopped` (`stopped`, `error = stopMessage`, `exitCode 1`,
`subagent.step.stopped`), `stopChildStep` (`targetIndex === undefined` → `stopRunner`; refused →
`subagent.step.stop_failed` `Child is not pending or running.`; live → `stop()`; pending →
`subagent.step.stop_queued`); `:3048-3055` `registerStepStop` fires at once when a request is already
recorded; `:3182-3190` `stoppedStepResult`; `:3842` `step.stopped = true` on `stopRunner`;
`:4219-4222,4335-4342` precedence `stopped → timedOut → childStopped → interrupted` and the
`subagent.step.stopped` + terminal child-status on settle; `:4937-4941` sequential skip
(`childStopResult`, `flatIndex++; continue`). `src/extension/schemas.ts:306` `childId:
Type.Optional(Type.String({minLength:1, maxLength:256, description:"Stable child identity for
child-scoped stop requests."}))`; `src/runs/foreground/subagent-executor.ts:303,6163,6184` threading;
`src/shared/types.ts:1882-1883,1904` the three step fields.

**cyrup (at `2d9d0d0a`)** — NEW `crates/cyrup-ext-subagents/src/background/child_identity.rs`:
`identity_from_parts` (`:93`, the rung order pinned by test — cyrup's `StepStatus` has no
`workflowKey`/`runId`, so every real identity is the positional `step:<index>` over
`RunStatus::steps`, the same index space steer's `target_index`, the transcript `index` and
`output-<i>.log` use), `candidates_from_parts` (`:108`), `resolve_async_status_child` (`:146`) →
`AsyncStatusChildResolution::{Resolved, NotFound(msg), Ambiguous(msg)}` (`:61`) with upstream's two
sentences, `is_stoppable_step_state` (`:195`). NEW `background/child_stop.rs`: `ChildStopRegistry`
(`:54`; pi's two maps — `record`/`recorded`/`is_requested`, `register_active` fires at once when
already requested (`:111`), `cancel_active` (`:136`)), pure `mark_child_stop_requested` (`:172` →
`ChildStopMarking::{Requested{child_id,agent,was_pending}, NotStoppable}`), `mark_child_stopped`
(`:217`, idempotent, `stopRequestedAt` precedence recorded → step → now), `child_status_event`
(`:279`). `background/mod.rs:762-773` `StepStatus.stop_requested`/`stop_requested_at`/`stopped`
(serde default, skipped when unset). `background/control.rs:624` `StopRequest` +
`target_index`/`child_id`, `for_child` (`:671`), `is_child_scoped` (`:686`);
`MAX_STOP_TARGET_INDEX`/`MAX_STOP_CHILD_ID_LENGTH` (`:693,697`); `is_valid_stop_child_id` /
`validate_stop_child_id` (`:702,715`, upstream's sentence); `stop_requests_dir` (`:744`) with
`stop_request_path` kept as the read-only legacy path (`:753`); `stop_request_file_name` (`:760`);
`StopOutcome::{Requested, ChildRequested{child_id}, NotStoppable, ChildUnresolved(String),
ChildNotStoppable{run_id,state}}` (`:773`); `stop(…, child_id)` (`:826`: reconcile → running|queued
→ resolve → stoppable gate → targeted request, upstream's order); `deliver_stop_request` (`:939`,
now queue-backed), `deliver_child_stop_request` (`:955`), `request_async_stop` (`:976`, both
validators); `parse_stop_request` (`:1706`); `pending_stop_request_paths`/`has_pending_stop_request`
/`peek_stop_requests` (`:1752-1767`); `check_stop_inbox_now` (`:1795`) and `consume_stop_request`
(`:1854`) now see ONLY whole-run requests, `consume_child_stop_requests` (`:1868`) drains the targeted
ones (queue files name-sorted + legacy file, sorted by `ts`, forged/invalid files consumed and
dropped). `background/runner_main.rs:1282` `ControlFlags::child_stops`; `:1472` the pending-target
skip (`skip_child_stopped_step`, `:2397`, pi `:4937-4941`); `:1489` per-step handle
`interrupt_cancel.child_token()` registered BEFORE `mark_step_running`, cleared at `:1538`; `:2753`
`ExecSingleStepExecutor::child_stops` read back at `:3216` as the child's `RunOptions::interrupt`;
`:2077` `MidFlightVerb::{RunStop, RunTimeout, ChildStop, Interrupt}` in pi's precedence inside
`settle_step_result` (`:2098`) — a child-stopped step ends `Stopped` with `STOP_MESSAGE`, the
promoted stopped `SingleResult`, `subagent.step.stopped` + child-status `stopped`
(`append_child_stopped_events`, `:2453`), and the loop ADVANCES (cyrup's loop already advances past
a failed step; upstream's `exitCode !== 0 → break` chain rule is pre-existing drift, not this row);
`:2372` `mark_remaining_stopped` now stamps `step.stopped`; `:1772-1779` terminal child-status for
recorded children on a whole-run stop; `:3747` the watcher routes child-scoped requests before the
run-wide probes; `route_child_stop_requests` (`:3964`, pi `stopChildStep`: derive identity, gate,
record, `stop_requested` + child-status `stopping`, cancel the live handle else `stop_queued`, or
`stop_failed`). `extension/tool/params.rs:102` `child_id`; `extension/tool/schema.rs:360` `childId`
(bounds + description verbatim); `extension/tool/routing.rs:1397` threads it;
`extension/executor/control.rs:140-145` `control_stop(…, child_id)` rendering the three new outcomes
with upstream's texts (`:223-240`); `host/slash.rs`, `tui/fleet_overlay.rs`, `executor/notices.rs`
callers pass `None`.

**Design decisions (recorded in the commit body)** — domain enums for the expected outcomes
(`AsyncStatusChildResolution`, the three new `StopOutcome` variants, `ChildStopMarking`) rather than
`Result<_, String>`/bool+Option; functional-core status transitions in `child_stop.rs` with the
watcher/loop as the shell; the per-step stop handle as a CHILD token of the run-wide interrupt token
held in a shared registry (no `SingleStepExecutor` signature change, run-wide verbs still cancel
through the parent); `childId` validated at the write boundary and dropped on read rather than
newtyped, so the tool boundary answers with upstream's sentence instead of a serde error.

**Verify (each fails at `53fe416b` by construction — every test names a symbol absent there — and
passes at `2d9d0d0a`; crate 2696/2696 via `cargo nextest run -p cyrup-ext-subagents`)** —
`child_identity.rs` `identity_falls_back_workflow_key_then_run_id_then_position`,
`candidates_keep_rung_order_and_dedupe`, `resolves_a_positional_child_with_its_state_and_agent`,
`an_unknown_child_reports_upstreams_not_found_sentence`, `ambiguity_is_reported_with_upstreams_sentence`,
`only_pending_and_running_children_are_stoppable`; `child_stop.rs`
`mark_child_stop_requested_gates_on_pending_or_running`, `mark_child_stopped_stamps_the_step_and_is_idempotent`,
`registry_applies_a_queued_request_at_registration_and_cancels_live_ones`, `child_status_event_has_pis_shape`;
`control.rs` `stop_child_id_is_validated_with_upstreams_message`,
`stop_requests_are_queued_per_file_drained_oldest_first_and_split_by_scope`,
`stop_with_a_child_id_resolves_gates_and_targets_the_request` (+ the pre-existing stop tests moved to
the queue), and from the review fix `6cf2cb9f` `same_millisecond_stop_requests_drain_in_write_order`
(24 same-`ts` child requests drain in write order; fails under the v4 name with probability
1 − 1/24!); `routing_tests.rs` `stop_with_child_id_reaches_control_stop_and_writes_a_targeted_request`,
`stop_refuses_a_child_that_is_not_pending_or_running_with_upstreams_text` (verbatim sentence),
`stop_reports_an_unknown_child_with_upstreams_not_found_text`; `runner_main.rs`
`child_scoped_stop_requests_are_routed_to_one_step_and_never_the_whole_run`; `schema.rs` `childId`
bounds. `crates/cyrup-it/tests/subagents/background_runner_main_integration.rs` (gated,
`cargo nextest run -p cyrup-it --features it --test subagents`)
`a_child_scoped_stop_stops_one_chain_step_and_the_next_step_still_completes` (step 0 torn down
mid-sleep, step 1 completes with its own output, run `Failed` not `Stopped`, events
`stop_requested` → child-status `stopping` → `step.stopped` → child-status `stopped`, no
`run.stopped`) and `a_child_scoped_stop_for_a_pending_step_is_queued_and_skips_it_when_reached`
(`stop_queued`, step never started, `durationMs: 0`). The `it`-gated pair could not be executed
in this session's shared tree — the `cyrup-it` build.rs nested `it-bins` build hit ENOSPC on every
attempt (disk shared by nine concurrent tracks; crate-level tests and clippy are green) — a
maintainer must run them once before treating the cyrup-it half as verified.

**Falsification** — `subagent({action:"stop", id, childId:"step:1"})` against a running chain must
answer `Stop requested for child step:1 in async run <id>.`, write ONE file under
`control/stop-requests/` carrying `targetIndex: 1, childId: "step:1"`, and leave `status.json`
`state: running`; the same call against a `complete` child must answer the verbatim refusal and
write nothing; a plain `stop` must still end the run `Stopped`. Any of those failing reopens the row.

**Residuals — recorded, not closed by this row.** (1) ~~**medium — the filing's headline scenario is
not delivered**~~ — **FILED AS `SUBA-093` 2026-09-04 (review fix `6cf2cb9f`) AND CLOSED THERE
2026-09-04 at `07f2df0d`**, which flattens a `ParallelGroup`'s members into `RunStatus::steps` and
registers a stop handle per DISPATCH; the text below is the residual as filed: a `ParallelGroup`/`DynamicGroup`
is ONE entry in `RunStatus::steps` (`pending_step_status_for`, `runner_main.rs:1159-1170` at HEAD)
and its members reach `parallel_groups` only after the group settles (`record_step_outcome`,
`:2550-2602`), so a `tasks[]` fan-out's members have no live per-child status to resolve against
and `step:0` stops the WHOLE group. Upstream flattens members into
`steps[]` (one `flatIndex` each). Closing it means live per-member status entries (the telemetry
pump, steer targeting, the transcript index and `output-<i>.log` all key on the same top-level
index), which is a status-model change beyond this row; the identity scheme here already follows
upstream's flat index once that lands. (2) `includeNested` / the slash `childId` form
(`slash-commands.ts:1110`) and the RPC `stop` surface (`extension/rpc.ts:561-687`) are cyrup-absent
surfaces. (3) The foreground executor registers no child stops (no control inbox). (4) cyrup's
chain loop advances past a child-stopped step exactly as it advances past a failed one; upstream
breaks the chain on any non-zero step (`subagent-runner.ts:5267`) — pre-existing drift on the
failure path, unchanged here. (5) The whole-run `stop` request is now queue-backed; an OLDER parent
writing the legacy `control/stop.json` is still honoured (read on every drain), but a NEWER parent
stopping an older runner is not (that runner reads only `stop.json`) — a one-release skew accepted
as upstream accepted it. (6) **test determinism — FIXED at `6cf2cb9f` (review 2026-09-04):**
`stop_request_file_name` named a request `<ts:013>-<v4 uuid>.json`, so two requests written in one
millisecond tied on `ts` and the random uuid decided the name sort that `consumeStopRequestPayloads`'
stable `ts` sort then preserved (`control-channel.ts:190-192`, `:597`, `:612` @v0.64.0 — upstream's
`randomUUID()` has the same nondeterminism in production); the review reproduced it as a 1-in-25
failure of the `[1, 2]` order assertions in `stop_with_a_child_id_resolves_gates_and_targets_the_request`
and `runner_main`'s `child_scoped_stop_requests_are_routed_to_one_step_and_never_the_whole_run`
(`SUBA-090` residual 3 had already observed it). The fix is at the source, not the fixtures: the
name now draws `uuid::Uuid::now_v7()` from the uuid crate's shared monotonic `ContextV7` (counter
within a millisecond, timestamp bumped on wrap), whose hyphenated hex string is time-then-counter
ordered, so a `ts` tie drains in WRITE order within one process — `[CYRUP-DELTA]` in uuid version
only (collision-freedom is unchanged; two parents in two processes still tie arbitrarily, as
upstream). Rejected: distinct `ts` in the fixtures (hides the same coin toss from the one production
path that writes twice in a millisecond, `ancestor-stop`'s cascade) and a sequence prefix in the
name (would change the on-disk shape a pi parent reads). Pinned by
`control::tests::same_millisecond_stop_requests_drain_in_write_order`; the four stop-request tests
looped 60× clean after the fix. Same commit: `route_child_stop_requests` reports a failed
`status.json` write after an accepted child stop with `tracing::warn` instead of `let _ =`
(pi's `writeStatusPayload` at `subagent-runner.ts:2988` is likewise best-effort, but silent).

---

## ~~SUBA-088~~ — ~~medium~~ **CLOSED 2026-09-04** — `subagents.defaultProvider` and per-agent `modelProvider` are unported, and the foreground launch path passes no preferred provider into candidate resolution at all

> **PROMOTED OUT OF `## Carried — NOT adversarially verified`, CONFIRMED WITH THREE CORRECTIONS,
> AND CLOSED 2026-09-04 — landing commit `ba24e5e5` (code), parent `16edcde2`; port-side evidence
> measured at `615bbb1d`, and neither intervening commit (`c02f1f30`, `16edcde2`) touches
> `crates/cyrup-ext-subagents`.** Upstream re-read
> with `git show` at BOTH tags. The five type fields and `applySubagentDefaultModel` are exactly as
> filed at v0.57.0 (`agents.ts:86,116,132,177,1155-1168`). **Two citation errors:** the filed
> `v0.57.0:agents.ts:997-1004` and `:1045-1051` land in the `toolBudget` override parse and the head of
> `readSubagentSettings` — the `defaultProvider` parse and `resolveSubagentDefaultProvider` live
> elsewhere (v0.64.0 lines below); and `AgentConfig::preferred_provider` had drifted from the filed
> `agent_config.rs:349` to `:417` (it is on `RunOptions`, not the agent config). **One impact
> correction that changed the fix:** the filing says the gap is "which provider a BARE id resolves
> against" inside `build_model_candidates` — but cyrup's launch path never resolves against a
> registry at all (foreground `available_models` is the persona's own list, and the bare id was
> forwarded verbatim as `--model <id>` for the CHILD to resolve), so the preference has to be applied
> by QUALIFYING the id to `provider/id` before spawn, or the child's own default-provider resolution
> decides. Port-side at `615bbb1d`, before the change: `rg 'default_provider|defaultProvider|
> model_provider|modelProvider|preferred_model_provider|provider_overrides'
> crates/cyrup-ext-subagents/src` found only the fork-thinking predicate's parent rung
> (`foreground.rs:952-958`, with the comment "`AgentDefinition` declares no `modelProvider`"), the
> `types.rs:570` doc listing the override key as deliberately unmodeled, and the settings test at
> `discovery/mod.rs:2309` that tolerates and DROPS the key — exactly as filed. `RunOptions::
> preferred_provider` was `None` at every launch site and consumed by nothing on the launch path.
> Severity `medium` stands; effort M was exact. Port target v0.64.0.

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed (2026-09-04, both tags)
**Subsystem** config / discovery / model ladder
**Window** v0.47.1..v0.57.0 · `cc112354 (#1394)`

**upstream (v0.64.0)** — `src/agents/agents.ts:59` `modelProvider` on the override base, `:90`
`BuiltinAgentOverrideConfig.defaultProvider?: string | false`, `:126`
`AgentModelSourceInfo.defaultProvider`, `:144` `AgentConfig.modelProvider?: string`, `:192`
`SubagentSettings.defaultProvider?: string`; `:1086-1089` override parse (`false` | non-empty
trimmed string, else `Builtin override '${name}' in '${filePath}' has invalid 'defaultProvider';
expected a non-empty string or false.`); `:1147-1153` settings parse (`Subagent settings in
'${filePath}' have invalid 'defaultProvider'; expected a non-empty string.`); `:1242-1249`
`resolveSubagentDefaultProvider` (project beats user when the project scope exists); `:1266-1279`
`applySubagentDefaultModel(agents, defaultModel, defaultProvider)` whose guard `if (agent.model !==
undefined && (agent.modelProvider !== undefined || !defaultProvider)) return agent;` stamps
`modelProvider` onto EVERY agent lacking one, including agents that pin a `model`; `:1387-1390`
`applyBuiltinOverride` (`false` → `delete next.modelProvider`, string → set), `:1481` the custom path
delegates to it; `modelProvider` is NOT a frontmatter key (no parser reads it). Consumers:
`src/runs/foreground/execution.ts:1881-1887` `buildModelCandidates(options.modelOverride ??
agent.model, agent.fallbackModels, options.availableModels, agent.modelProvider ??
options.preferredModelProvider, {...})`; `src/runs/shared/model-fallback.ts:412-418`
`buildModelCandidates(primary, fallbacks, available, preferredProvider?, options?)` resolving each
candidate through `resolveSubagentModelCandidate(model, available, preferredProvider)` (`:207-218`
→ `resolveExactIdMatches` `:115-126`, the preferred provider's exact-id match wins);
`src/runs/foreground/subagent-executor.ts:3648` `const currentProvider = parentModel?.provider`,
`:3825` `preferredModelProvider: currentProvider`, `:6390` the fork predicate's
`agentConfig?.modelProvider ?? parentModel?.provider`, `:1297` `currentModelProvider:
parentModel?.provider` for the async runner; `src/runs/background/async-execution.ts:930`
`a.modelProvider ?? ctx.currentModelProvider`; `src/agents/agent-management.ts:1012,1025,1050` the
`models` report resolves with `agent.modelProvider ?? preferredProvider`, `:742` the list line
prints `${modelProvider}/${model}` for a bare id. **v0.57.0 → v0.64.0:** parse/resolve/apply are
byte-equivalent for this key; v0.64.0 adds `providerOverrides` (`selectProviderOverrides`,
`:1231-1240`, per-provider override maps merged over `overrides`) and threads
`preferredModelProvider` into the discovery cache key (`:2425-2428,2457,2505,2539-2571`).

**cyrup (at `ba24e5e5`)** — `crates/cyrup-ext-subagents/src/discovery/types.rs`:
`SubagentSettings::default_provider: Option<String>`, `AgentOverrideConfig::default_provider:
OverrideField<String>` (and `is_empty`), `AgentDefinition::model_provider: Option<ProviderId>`; the
override census doc now reads "19 modeled, 3 unmodeled". `discovery/mod.rs`:
`validate_default_provider` and `validate_override_default_providers` (upstream's two messages, the
file path dropped exactly as the sibling `validate_default_thinking` drops it), trimmed storage in
`parse_subagent_settings`, `default_provider: project.or(user)` in
`resolve_layered_subagent_settings`. `discovery/merge.rs`: `resolve_default_provider` (project wins
when the project scope exists), `apply_default_model(merged, default_model, default_provider)`
mirroring the `:1269` guard, the builtin full-replace arm and the custom fill arm (gate vacuously
open — no frontmatter key). `exec/fallback.rs`: `build_model_candidates(override, primary,
fallbacks, available, preferred_provider: Option<&ProviderId>)` and `build_model_candidates_scoped`
(same, before `scope`); new pure `qualify_model_candidate` (bare id → `provider/id`, thinking suffix
kept, qualified ids never rewritten) and `provider_of` (`normalizeParentModel`'s two-non-empty-halves
rule); dedup on the qualified spelling, allowlist accepting either spelling.
`exec/agent_config.rs`: `AgentConfig::model_provider`, `ResolvedAgentPersona::model_provider`
(serde default; hop 2 carries it), `RunOptions::preferred_provider` documented as the parent rung.
`exec/mod.rs::resolve_model_candidates` passes `agent.model_provider.as_ref().or(opts.
preferred_provider.as_ref())`. `extension/executor/foreground.rs`: `ResolvedRunAgent::
preferred_provider` = `provider_of(remembered_parent_model)` → `RunOptions`, and
`fork_requires_thinking_off` honours `agent.model_provider` first. `background/runner_main.rs::
build_step_run_options`: `preferred_provider: provider_of(self.inherited_session_model)`.
`extension/executor/reports.rs::run_models_report`: per-agent `provider_for(agent)` =
`agent.model_provider ?? session provider` at all three resolution sites. The `cyrup-it` persona
and agent-config fixtures gained the new field (`model_provider: None` / `default_provider: None`);
`discovery/runtime_registry.rs:18`'s pre-existing broken intra-doc link (private
`extension::executor` path, from SUBA-084) was repointed at the public re-export so the required
rustdoc check passes.

**Design decision (recorded per DESIGN-GUIDANCE, in the commit body):** functional core /
imperative shell — the provider preference is a pure decision (`qualify_model_candidate`) the
existing ladder shell applies; no new type. Rejected: a `QualifiedModelId` newtype (every consumer
takes the plain `ModelId` string, as does upstream; ~30 conversions for no check it would remove);
resolving against a live registry on the launch path (a larger behavioural change than the row, and
would make the ladder depend on host availability); folding the provider into `ModelOverride`
(orthogonal to the override/inherit decision). Documented inference: with no registry, any `/` is a
provider prefix (upstream: only a REGISTERED provider's prefix) — the convention the fork predicate
and the models report already used.

**Tests (all fail before / pass after — the parse-rejection test was run against HEAD and failed
with `Ok(..)`; the rest do not compile at HEAD because the fields/parameter did not exist):**
`discovery::tests::parse_subagent_settings_reads_and_trims_default_provider`,
`…::parse_subagent_settings_rejects_invalid_default_provider_with_upstreams_message`,
`…::parse_subagent_settings_validates_override_default_provider`;
`discovery::merge::tests::default_provider_stamps_agents_that_pin_a_model_but_no_provider`,
`…::default_provider_project_wins_over_user`,
`…::override_default_provider_sets_and_false_clears_model_provider`;
`exec::fallback::tests::bare_candidate_is_qualified_by_the_preferred_provider`,
`…::qualified_candidates_and_no_preference_are_left_untouched`,
`…::qualification_dedups_against_the_qualified_spelling_and_keeps_qualified_allowlist_entries`,
`…::provider_of_and_qualify_follow_the_parent_model_rules`;
`exec::tests::a_bare_persona_model_spawns_qualified_by_the_agents_provider_then_the_parents` (the
launch chain `resolve_model_candidates` → `build_attempt_spawn_plan` puts `openai-codex/gpt-5` on
`--model` for `model: gpt-5` + a stamped provider, `anthropic/gpt-5` under the parent rung alone, and
bare `gpt-5` with neither — the last being the pre-SUBA-088 argv). Checks: `cargo fmt --all --
--check`, `cargo clippy -p cyrup-ext-subagents --all-targets -- -D warnings`, `cargo nextest run -p
cyrup-ext-subagents` (2707/2707), `RUSTDOCFLAGS='-D warnings' cargo doc -p cyrup-ext-subagents
--no-deps`, `CYRUP_IT_BIN_DIR=<dir> cargo check -p cyrup-it --features it --tests` — all clean.
Running the `cyrup-it` suite itself needs the nested it-bins build, which hit ENOSPC on this shared
disk (399 MB free at the end of the run); a maintainer must run it once.

**Falsification** — with `~/.cyrup/agents/settings.json` `{"subagents":{"defaultProvider":
"openai-codex"}}` and an agent whose frontmatter says `model: gpt-5`, a foreground run must spawn
the child with `--model openai-codex/gpt-5` (the `attempt-0.jsonl` tee / spawn argv), and
`subagents-models` must resolve that agent under `openai-codex`; `{"defaultProvider":"  "}` must abort
discovery with `invalid 'defaultProvider'; expected a non-empty string`; an
`agentOverrides.<name>.defaultProvider: false` must leave that agent's id bare. Any of those failing
reopens the row.

**Residuals — recorded, not closed by this row.** (1) **low — v0.64.0 `providerOverrides`**
(`selectProviderOverrides`, `agents.ts:1231-1240`): per-provider override maps selected by the
parent's provider are not modeled; the key is dropped by serde as before. (2) **low —
`preferredModelProvider` in the discovery cache key** (`:2425-2428`): cyrup's discovery is not cached
by provider; irrelevant until (1) lands. (3) **low — a bare id is qualified UNCONDITIONALLY, where upstream
only PREFERS the provider** (wording corrected by the 2026-09-04 review): `qualify_model_candidate`
rewrites a bare `<id>` to `<agent.model_provider ?? parent provider>/<id>` with no registry in hand,
whereas `resolveExactIdMatches` (`runs/shared/model-fallback.ts:115-126` @v0.64.0) takes the
preferred provider's exact-id match IF it exists and otherwise falls back to the UNIQUE exact-id
match across the whole registry (`exactMatches.length === 1`), throwing `Unknown subagent model
'<id>' in the active Pi model registry.` at the parent only when NO provider offers the id. So the
observable divergence is narrower than "fails in the child instead of at the parent": when exactly
one OTHER provider offers the bare id, upstream resolves to that provider and cyrup forces the
preferred one, which then fails in the CHILD (`Unknown model …` from its own registry); when no
provider offers it both fail, upstream at the parent and cyrup in the child. Not a regression
against pre-port cyrup (the child resolved a bare id against its default provider,
`crates/cyrup/src/provider.rs` `select_provider`); closing it means resolving the ladder against
`HostServices` models on the launch path. (4) `AgentModelSourceInfo.defaultProvider` (`:126`) is not
carried — cyrup's provenance is a `Copy` enum and upstream has no consumer of the field. (5) The
`formatAgentCapabilitiesLine` list line (`agent-management.ts:727-745`, `${modelProvider}/${model}`
for a bare id) is not rendered by cyrup at all; the describe view prints the raw `model:` as before.
(6) Runtime-registered agents (`SUBA-084`) cannot declare `modelProvider` — upstream's
`registerAgent` does not carry it either (`runtime-agent-registry.ts` has no such field at v0.64.0).

---

## ~~SUBA-089~~ — ~~medium~~ **CLOSED 2026-09-04** — The model-fallback retry decision ignores whether the failed attempt already ran tools, so a half-completed mutating run is re-dispatched

> **PROMOTED OUT OF `## Carried — NOT adversarially verified`, CONFIRMED EXACTLY AS FILED, AND
> CLOSED 2026-09-04 — landing commit `cde2ddfc` (code), parent `f81573bb`.** Upstream re-read with
> `git show` at v0.47.1, v0.57.0 and v0.64.0. Every filed upstream line resolves: v0.57.0
> `model-fallback.ts:461-474` (`messageError` + `isRetryableModelFailureAttempt`, the `:469`
> `toolCount > 0` refusal, `:471-473` the correlation clauses), `execution.ts:2051` the sole
> foreground gate and `:2058` `if (!retryableModelFailure || modelIndex === modelsToTry.length - 1)
> break`, and `v0.47.1:execution.ts:1633` the bare `isRetryableModelFailure(result.error)` — so the
> window is exact (`d8d1408d fix: retry provider connection errors`, 2026-08-25, first tag v0.57.0).
> **One correction to the filing's *Relation* note** ("retryable patterns … present and correct"):
> `d8d1408d` is a two-part change — the SAME commit added
> `/connection\s+(?:error|reset|closed|aborted)/i` to `RETRYABLE_MODEL_FAILURE_PATTERNS`
> (`v0.57.0:model-fallback.ts:428`) precisely because the broader text would otherwise re-run a
> child that had done real work, and the narrowed gate is what makes it safe. Cyrup had only
> `connection refused`; `is_retryable_model_failure(Some("APIConnectionError: Connection closed."))`
> was `false` at HEAD (upstream's own test asserts `true`, `test/unit/model-fallback.test.ts:203-205`
> @v0.57.0). Both halves are ported together. **Port-side at `f81573bb`, before the change:**
> `rg 'is_retryable_model_failure_attempt|message_errors' crates/cyrup-ext-subagents/src` → 0 hits;
> `exec/fallback.rs:1329` `if !is_retryable_model_failure(signal.error.as_deref())` was the whole
> retry gate, after the timed_out/detached/success/startup/`is_last_candidate` arms (the ordering is
> net-equivalent to upstream's `!retryable || last`); `StartupEvidence::{message_count, tool_count}`
> existed but were read only by `is_retryable_subagent_startup_failure`, a separate same-model
> relaunch gate consulted earlier and only for silent exits — it does not block the re-dispatch.
> **v0.57.0 → v0.64.0:** the predicate gains a second empty-output sentinel
> (`/^Subagent produced no output after terminal assistant stopReason "[^"]+"\.$/`,
> `v0.64.0:model-fallback.ts:533`, produced by `shared/utils.ts:472`
> `formatEmptyTerminalAssistantResponseError`), and the BACKGROUND runner gates on the same predicate
> (`background/subagent-runner.ts:90,2090,2097` — at v0.57.0 `:1993`). Cyrup's background runner
> reaches the ladder through `exec::run_sync` → `run_fallback_ladder`, so one gate covers both.
> Severity `medium` stands; effort S was exact. Port target v0.64.0.

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (2026-09-04, three tags)
**Subsystem** model-fallback ladder (foreground + background)
**Window** v0.47.1..v0.57.0 · `d8d1408d`

**upstream (v0.64.0)** — `src/runs/shared/model-fallback.ts:489`
`/connection\s+(?:error|reset|closed|aborted)/i` (between `temporar(?:ily)? unavailable` and
`connection refused`); `:524-528` `messageError(message)` — the `errorMessage` string of any
message object, no role filter, untrimmed; `:530-537`:
```ts
export function isRetryableModelFailureAttempt(input: { error: string | undefined; messages?: readonly unknown[]; toolCount?: number }): boolean {
	if (!isRetryableModelFailure(input.error)) return false;
	if ((input.toolCount ?? 0) > 0) return false;
	if (input.error === "Subagent produced no output (possible model cold-start or empty response)." || /^Subagent produced no output after terminal assistant stopReason "[^"]+"\.$/.test(input.error ?? "")) return true;
	if ((input.toolCount ?? 0) === 0 && (input.messages?.length ?? 0) === 0) return true;
	const error = input.error?.trim();
	return Boolean(error && input.messages?.some((message) => messageError(message)?.trim() === error));
}
```
Call sites: `src/runs/foreground/execution.ts:2144` `isRetryableModelFailureAttempt({ error:
result.error, messages: result.messages, toolCount: result.progressSummary?.toolCount })`, `:2151`
`if (!retryableModelFailure || modelIndex === modelsToTry.length - 1) break modelAttemptsLoop;`;
`src/runs/background/subagent-runner.ts:2090` `({ error, messages: run.messages, toolCount:
run.toolCount })`, `:2097` the same break. `result.messages` is every `message_end` message
(`execution.ts:1122,1190`; `subagent-runner.ts:854`). Tests: `test/unit/model-fallback.test.ts:317-319`
(`Connection error`, `APIConnectionError: Connection closed.`, `Connection reset by peer` retryable)
and `:341-346` "does not retry raw process stderr after child activity" (the four attempt cases).

**cyrup (at `cde2ddfc`)** — `crates/cyrup-ext-subagents/src/exec/fallback.rs:802`
`is_retryable_model_failure_attempt(&AttemptSignal) -> bool`, upstream's five clauses in order over
`signal.error`, `signal.startup.tool_count`, `signal.startup.message_count` and the new
`AttemptSignal::message_errors: Vec<String>` (`:974`); `:761-775`
`EMPTY_OUTPUT_AFTER_STOP_REASON_PREFIX/SUFFIX` + `is_empty_output_sentinel` (exact, untrimmed match
of `exec::output::EMPTY_OUTPUT_ERROR` or the anchored stopReason form — non-empty, no inner quote);
`:1448` the ladder gate is now `if !is_retryable_model_failure_attempt(&signal)`, in the same
position (after timeout/detach/success/startup/last-candidate); `:475` `RetryPattern::WsThenAny`
(`first\s+(?:a|b|…)`, at least one whitespace character — distinct from `OptionalWsBetween`'s
`\s*`), `:510` the `connection` entry in upstream's position, `:611` its matcher arm.
`exec/output.rs:751` `message_error_messages(&[SubagentEvent]) -> Vec<String>` — every
`MessageEnd`'s string `errorMessage`, any role, untrimmed, order kept (pi's `messageError`).
`exec/attempt_runner.rs:191` `run_attempt` fills `message_errors` from
`progress.message_end_events` (`:591`/`:624` interrupted and timed-out attempts likewise; `:560`
setup failure empty — nothing ran). Doc comments on the ladder (`run_fallback_ladder` step 3) and
the module header name the new gate and cite `execution.ts:2144,2151` / `subagent-runner.ts:2090,2097`.

**Design decision (recorded per DESIGN-GUIDANCE, in the commit body):** functional core /
imperative shell — the decision stays a pure function over the signal, like its neighbour
`is_retryable_subagent_startup_failure(&AttemptSignal)`; the shell (`attempt_runner`) supplies one
new fact. `message_errors` lives on `AttemptSignal`, not `StartupEvidence`, because that struct's
stated contract is "every field is a reason NOT to relaunch" and this list is corroborating
evidence FOR advancing. Rejected: a `RetryableAttemptInput` struct mirroring upstream's object
(ceremony; the signal already carries all four inputs); a regex dependency for the stopReason
sentinel (the crate hand-rolls every pattern; a prefix/suffix strip is exact for `"[^"]+"`); a
`\s*`-based approximation with the existing `OptionalWsBetween` (would match `connectionreset`,
which upstream's `\s+` does not).

**Tests (fail before / pass after):** `exec::fallback::tests::retryable_error_after_tools_ran_does_not_advance_the_ladder`
and `…::uncorroborated_retryable_text_after_messages_stops_but_corroborated_advances` — RED run
recorded against the bare gate (`is_retryable_model_failure(signal.error.as_deref())` swapped back
in, everything else in place): both fail with the ladder advancing to `b`; GREEN at `cde2ddfc`.
`…::attempt_predicate_matches_upstreams_stderr_after_activity_cases` (upstream's four cases
verbatim), `…::attempt_predicate_never_advances_once_a_tool_ran`,
`…::attempt_predicate_empty_output_sentinels_advance_despite_messages` (both sentinels; four
near-misses refused), `…::attempt_predicate_correlates_a_trimmed_message_error_message`,
`…::attempt_predicate_still_requires_a_retryable_text` — name a symbol absent at `f81573bb` and so
cannot compile there; the first of them was additionally observed RED in this session before the
connection pattern landed (`APIConnectionError: Connection closed.` not retryable).
`…::a_dropped_provider_connection_is_retryable_but_only_across_real_whitespace` plus the three
upstream strings added to `retryable_error_text_is_classified_as_retryable`'s list — fail at
`f81573bb` (`connection` pattern absent). `exec::output::tests::message_error_messages_collects_every_message_end_error_message_untrimmed`
— symbol absent at `f81573bb`. Checks: `cargo fmt --all -- --check`, `cargo clippy -p
cyrup-ext-subagents --all-targets -- -D warnings`, `cargo nextest run -p cyrup-ext-subagents`
(2716/2716), `RUSTDOCFLAGS='-D warnings' cargo doc -p cyrup-ext-subagents --no-deps` — all clean. No
crate outside `cyrup-ext-subagents` constructs `AttemptSignal`/`StartupEvidence` (`rg` across
`crates/`), so the added field is API-additive.

**Falsification** — a foreground or background child whose `attempt-N.jsonl` shows ≥1
`tool_execution_start` and whose run then ends with an error such as `connection reset by peer`
or `overloaded` must produce ONE row in `model_attempts` and no `[fallback] … Retrying with …`
note, even with fallback models configured; the same error with zero tools and zero messages must
still advance; `APIConnectionError: Connection closed.` from a child that emitted an assistant
turn WITHOUT an `errorMessage` must stop the ladder, and WITH a matching `errorMessage` must
advance. Any of those failing reopens the row.

**Residuals — recorded, not closed by this row.** (1) **low — cyrup never emits the v0.64.0
terminal-stopReason sentinel it now recognises**: `formatEmptyTerminalAssistantResponseError`
(`shared/utils.ts:462-474`) prefers the last assistant `errorMessage`, then `Subagent produced no
output after terminal assistant stopReason "<reason>".` for a non-`stop` reason; cyrup's
empty-output re-diagnosis (`exec/output.rs` `EMPTY_OUTPUT_ERROR`) emits only the cold-start form.
Unfiled `v0.57.0..v0.64.0` drift in the empty-output diagnosis, not this row. (2) **low —
`recordRetryableModelFailure`** (the per-session failed-model cache upstream updates only when the
attempt predicate says retryable, `execution.ts:2145`) has no cyrup counterpart; already outside this
row's scope. (3) **text-only —** the crate evaluates patterns per line, so `connection\n reset`
(whitespace run containing a newline) does not match where JS `\s+` would; the same pre-existing
limitation applies to `rate\s*limit`, and no producer emits either shape.

## ~~SUBA-090~~ — ~~medium~~ **PARTIALLY CLOSED 2026-09-04** — Completion notices are always rendered: the port hardcodes `display: true` where upstream hides a plain successful background completion and groups a batch

> **PROMOTED OUT OF `## Carried — NOT adversarially verified`, CONFIRMED EXACTLY AS FILED, AND
> PARTIALLY CLOSED 2026-09-04 — landing commit `79ee7eff` (code), parent `48b4e8fb`.** Upstream
> re-read with `git show` at v0.43.0, v0.57.0 and v0.64.0. Every filed upstream line resolves:
> `v0.57.0:src/runs/background/notify.ts:239` carries the predicate verbatim and `:241-247` the
> `pi.sendMessage({customType: "subagent-notify", content, display}, {triggerTurn: items.some((item)
> => item.triggerTurn)})`; `v0.43.0:notify.ts:173` is the same expression without the
> `scheduleOrigin` clause (in-baseline, window exact); `v0.57.0:notify.ts:379` /
> `v0.64.0:notify.ts:605` `triggerTurn: result.triggerTurn !== false` per completion;
> `v0.57.0:notify.ts:211-226` / `v0.64.0:notify.ts:376-393` `formatGroupedCompletion`. **No
> correction to the filing.** **Port-side at `48b4e8fb`, before the change:** `watch.rs:780-781`
> `display: true, trigger_turn: true,` (the filing's `:745-746` moved under the workspace rustfmt
> pass), no branch on outcome or source in `format_completion_message`, the struct doc at `:610-625`
> still asserting *"Always `true` (pi's `display: true`)"*; `rg 'Background tasks completed|
> format_grouped_completion|schedule' crates/cyrup-ext-subagents/src/background/{watch,mod}.rs` → 0.
> **v0.57.0 → v0.64.0:** no change to the predicate or the send; `:402`/`:404-410` are the same
> lines re-numbered. Severity `medium` stands; effort S was exact. Port target v0.64.0. **Why
> PARTIAL, not CLOSED:** the crate now emits the right `display`, but the consumer seam outside the
> crate drops it on the trigger-turn path (residual 1 below) — a fix that is correct and necessary
> in this crate and not, by itself, sufficient to keep the notice off the screen.

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (2026-09-04, three tags)
**Subsystem** background completion notify / `display`
**Window** in-baseline (≤ v0.43.0)

**upstream (v0.64.0)** — `src/runs/background/notify.ts:399-412`:
```ts
function sendCompletion(pi: Pick<ExtensionAPI, "sendMessage">, items: PendingCompletion[]): boolean {
	if (items.length === 0) return true;
	const details = items.map((item) => item.details);
	const content = details.length === 1 ? formatSingleCompletion(details[0]!) : formatGroupedCompletion(details);
	const display = details.some((detail) => detail.source === "foreground" || detail.status !== "completed" || detail.scheduleOrigin !== undefined);
	try {
		pi.sendMessage({ customType: "subagent-notify", content, display }, { triggerTurn: items.some((item) => item.triggerTurn) });
```
`:440` `const status = stopped ? "stopped" : paused ? "paused" : result.success ? "completed" :
"failed"` (the `status` input); `:36-56` `SubagentNotifyDetails { status, source?: "async" |
"foreground", scheduleOrigin?, … }`; `:605` `triggerTurn: result.triggerTurn !== false`; `:608-616`
a foreground or non-`completed` completion is emitted at once, a `completed` one goes to the
batcher. Same at `v0.57.0:notify.ts:235-249` and, minus `scheduleOrigin`, at
`v0.43.0:notify.ts:169-178`.

**cyrup (at `79ee7eff`)** — `crates/cyrup-ext-subagents/src/background/watch.rs`
`completion_notice_display(ClassifiedOutcome) -> bool` (`outcome != Completed`): pi's `:402`
reduced to its one cyrup-reachable clause, with the doc stating that `ResultFile`
(`background/mod.rs` `ResultFile{id, run_id, agent, mode, state, success, cwd, session_file,
results}`) carries neither `source` (detached-foreground completions are not ported) nor
`scheduleOrigin` (durable schedules are not ported), so those clauses are vacuously false here —
NOT that upstream is unconditional; `format_completion_message` classifies once and feeds the same
`classify_outcome` result to both the header word and `display`; `CompletionMessage::display` and
`::trigger_turn` docs rewritten to cite `notify.ts:402`/`:605` @v0.64.0 (`trigger_turn` stays `true`:
cyrup has no per-result `triggerTurn: false` input, so pi's default is the only reachable value);
`HostServicesCompletionSink::deliver` forwards the computed `display` to
`HostServices::inject_message` unchanged (it already did). Design: no new type — a pure predicate
over the existing `ClassifiedOutcome` enum, kept out of the I/O sink (Functional Core); rejected
adding dead `source`/`schedule_origin` fields to `ResultFile` with no producer, and the grouped
form (that is `SUBA-017`'s batcher).

**Tests (fail before / pass after):**
`background::watch::tests::a_plain_successful_background_completion_is_not_displayed`,
`…::format_completion_message_reproduces_notify_ts_layout` (now `assert!(!msg.display)` on the
completed fixture) and
`…::install_completion_watcher_fires_exactly_one_notify_and_deletes_the_result` (now asserts the
`display` the sink receives is `false`) — RED run recorded against a stub predicate returning
`true` (HEAD's behaviour, everything else in place): all three fail with `display == true`; GREEN
at `79ee7eff`. `…::failed_paused_and_stopped_completions_are_displayed` (failed, `Complete`+
`success:false`, paused, stopped — all displayed, all still `trigger_turn`) is a regression guard
that is green at HEAD by construction. `crates/cyrup-it/tests/subagents/companions_hostservices_proof.rs`
`background_completion_injects_a_turn_triggering_message_on_the_real_host_services` now asserts
`!display` on the recorded `inject_message` call (fails at `48b4e8fb`: `display == true`); **not
run in this session** — the `it`-feature target could not be built (ENOSPC, 15 MB free on `/`).
Checks: `cargo fmt --all -- --check`, `cargo clippy -p cyrup-ext-subagents --all-targets -- -D
warnings`, `RUSTDOCFLAGS='-D warnings' cargo doc -p cyrup-ext-subagents --no-deps` — clean;
`cargo nextest run -p cyrup-ext-subagents` 2717/2718, the one failure an unrelated pre-existing
ordering flake in `SUBA-087`'s stop-request tests (residual 3).

**Falsification** — a `ResultFile` with `state: Complete, success: true` must reach
`HostServices::inject_message` with `display == false` and `trigger_turn == true`; any of
`Failed`, `Complete`+`success: false`, `Paused`, `Stopped` (or a child with `stopped: true`) must
reach it with `display == true`. Either failing reopens the row.

**Residuals — recorded, not closed by this row.** (1) **medium — the trigger-turn seam drops
`display` (area 08 session-svc / area 03 session, not this crate) — FILED AS `SUBA-094` 2026-09-04
so the census counts it:**
`crates/cyrup-session-svc/src/session/inject.rs:125-160` `inject_message(content, custom_type,
display, details, trigger_turn)` consults `display` ONLY in the idle, non-trigger-turn branch
(`append_custom_message(&kind, .., display, details)`); with `trigger_turn = true` it builds
`AgentMessage::Custom{kind, payload, details, timestamp}` (`crates/cyrup-agent/src/event.rs:39-51`
— no `display` field) and `spawn_run`s over it, so a `display: false` notice is still rendered
from `message_end`. Until `display` travels with the Custom message through `inject.rs`,
`AgentMessage::Custom` and the TUI renderer, this crate's fix is inert at the screen. That is the
gap between PARTIAL and CLOSED, and it is an M-effort cross-crate change. (2) **low — grouped
form:** `formatGroupedCompletion` (`Background tasks completed (N): **a**, **b**` header + numbered
blocks) and the `completed`-only batching that feeds it remain `SUBA-017` (completion batching,
in-baseline, not-ported); the `display` predicate is now independent of it, as the filing said.
(3) **test-flake, `SUBA-087`'s, observed here:** in the full-crate run one of
`background::runner_main::tests::child_scoped_stop_requests_are_routed_to_one_step_and_never_the_whole_run`
/ `background::control::tests::stop_with_a_child_id_resolves_gates_and_targets_the_request` fails
intermittently with two stop requests read back in the opposite order
(`[(Some(2), "step:2", "c"), (Some(1), "step:1", "b")]` vs the expected `1, 2`); each passes
alone and in the `background::` group. Directory-order dependence, not this row — diagnosed and
FIXED at `6cf2cb9f` (same-millisecond `ts` tie decided by a v4 uuid; now v7), see `SUBA-087`
residual (6).

---

## ~~SUBA-091~~ — ~~medium~~ **CLOSED 2026-09-04** — The fleet inspector passes an EMPTY trusted-root list to the transcript reader, so the session-transcript fallback always refuses

> **PROMOTED OUT OF `## Carried — NOT adversarially verified`, CONFIRMED EXACTLY AS FILED, AND
> CLOSED 2026-09-04 — landing commit `681f6255` (code), parent `6e9da1b0`; this row written in a
> separate docs commit after the implementer's own row edit was lost.** Upstream re-read with `git
> show` at v0.57.0 and v0.64.0, and the upstream landing commit `9ceb5650` (`fix: pass trusted
> session roots to fleet transcripts (#1174)`, 2026-08-15, first tag v0.51.0 — inside the filed
> v0.47.1..v0.57.0 window) read as a diff. Every filed upstream shape resolves: the `asyncDetail(item,
> state)` call is byte-identical at `v0.57.0:src/tui/fleet.ts:544-554` and
> `v0.64.0:src/tui/fleet.ts:550-560` (`sessionRoots:` at `:551` / `:557`); before `9ceb5650` it was
> `formatAsyncRunTranscript(status, item.run.asyncDir, { index: item.index, lines: TRANSCRIPT_LINES })`
> — the shape the port still had. **No correction to the filing; line drift only:** the port call
> had moved from the filed `fleet.rs:842-848` to `:884-890` under the workspace rustfmt pass, and the
> containment gate from `fleet_view.rs:143-161` to `:163-177` (`read_session_transcript_tail`
> `:618-632` → `:685-701`). **Port-side at `6e9da1b0`, before the change:** `tui/fleet.rs:889` `&[]`
> as the `session_roots` argument, no `[CYRUP-DELTA]` note; `rg 'trusted_session_roots|
> trusted_session_file' crates/cyrup-ext-subagents/src` → 0; `FleetState` (`tui/fleet_state.rs:
> 438-474`) had no roots field and `AsyncRunView` (`:372-390`) no `session_root`; `detail_lines(item,
> error)` (`:931`) and its one caller (`:1901`) were the only route into `async_detail`. **One
> refinement to the filed fix, not to the finding:** the filing proposed reusing `status.rs`'s
> `transcript_session_roots` resolver; that triple is `[async root, project subagents dir, temp
> artifacts dir]` — artifact directories pi's fleet does NOT trust — so the port seeds pi's own
> `state.trustedSessionRoots` composition and the status path's differing triple is residual (3).
> Severity `medium` stands; effort S was exact. Port target v0.64.0.

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (2026-09-04, both tags + upstream landing commit)
**Subsystem** fleet inspector / transcript containment
**Window** v0.47.1..v0.57.0 · `9ceb5650 (#1174)`, first tag v0.51.0

**upstream (v0.64.0)** — `src/tui/fleet.ts:550-560`:
```ts
function asyncDetail(item: Extract<FleetItem, { kind: "async" }>, state: SubagentState): string[] {
	const status = readStatus(item.run.asyncDir);
	if (status) {
		const trackedJob = state.fleetJobs?.get(item.runId) ?? state.asyncJobs.get(item.runId);
		const lines = formatAsyncRunTranscript(status, item.run.asyncDir, {
			index: item.index,
			lines: TRANSCRIPT_LINES,
			sessionRoots: uniquePaths([...(state.trustedSessionRoots ?? []), trackedJob?.sessionRoot]),
			trustedSessionFiles: [item.step?.sessionFile ?? item.run.sessionFile].filter((value): value is string => Boolean(value)),
			trustedSessionFileRoot: state.trustedSessionFileRoot,
		}).split("\n");
```
`:606-617` `detailLines(item, error, state)` (`v0.57.0:600-611`) threads `state` into the async arm only; `:629-631`
`uniquePaths` (`path.resolve` + `Set`). `src/extension/index.ts:447` `trustedSessionRoots: []` at
state construction, re-seeded on every session start at `:895-898`:
```ts
state.trustedSessionRoots = [...new Set([
	...(config.defaultSessionDir ? [path.resolve(expandTilde(config.defaultSessionDir))] : []),
	...(state.parentSessionFile ? [getSubagentSessionRoot(state.parentSessionFile)] : []),
])];
```
`:283-290` `getSubagentSessionRoot(parentSessionFile)` = `path.join(path.dirname(parent),
path.basename(parent, ".jsonl"))` when a parent exists (else a `mkdtempSync` temp dir — a branch
`:897`'s guard never reaches); `:894` `trustedSessionFileRoot = parentSessionFile ?
path.join(getAgentDir(), "sessions") : undefined`. Consumer: `src/runs/background/fleet-view.ts:252`
`readSessionTranscriptTail(sessionFile, maxLines, trustedRoots, trustedFiles = [], trustedFileRoot?)`
→ `readContainedTextTail(.., trustedRoots, "session", ..)`, whose gate at `:135` is
`trustedRoots.length === 0 && (!trustedFileRoot || trustedFiles.length === 0)` → `Refusing to read
session transcript path without a trusted root: …`; the two call sites `:542` (per-step) and `:573`
(run-level) pass `options.sessionRoots ?? []`. `trackedJob.sessionRoot` is
`getSubagentSessionRoot(parentSessionFile)` at spawn (`src/runs/foreground/subagent-executor.ts:
1687,1941,2043`), recorded in `status.json` (`async-status.ts:416`) and the tracker
(`async-job-tracker.ts:171`). Test: `test/unit/fleet.test.ts:1028` "passes current-session trusted
roots to async session transcript fallback" (added by `9ceb5650`). **v0.57.0 → v0.64.0:** no change
to the call or the seeding; the `fleet.ts` lines above are the same lines re-numbered (+6).

**cyrup (at `681f6255`, == HEAD)** — `crates/cyrup-ext-subagents/src/tui/fleet_state.rs:452-459`
`FleetState::trusted_session_roots: Vec<PathBuf>` (pi `state.trustedSessionRoots`; the doc cites
`index.ts:447`/`:895-898`). `src/extension/executor/paths.rs:150-162` `subagent_session_root(parent)`
(`index.ts:283-290`, present-parent branch only — the `mkdtemp` branch is unreachable from a
trusted-root seed and the doc says so) and `:180-195` `trusted_session_roots(default_session_dir,
parent_session_file)` (`index.ts:895-898`: `expand_tilde` + `resolve_against_process_cwd` (`:204`)
= Node's `path.resolve`, an empty configured value is pi's falsy `defaultSessionDir`, the parent rung
deduped against the first — pure, no I/O). `src/extension/executor/status.rs:246-249`
`SubagentExecutor::fleet_state` seeds the field from the config snapshot's `default_session_dir` and
the live `HostServices::session_file()` (the snapshot is now taken once, `:234`, where it was taken
twice). `src/tui/fleet.rs:894-899` `async_detail(item, run, step_index, state)` passes
`&unique_paths(state.trusted_session_roots …)` (`:909-916`) where the literal `&[]` was; `:960-964`
`detail_lines(item, error, state)` (pi `:606-617`) and its caller `SubagentFleetComponent` at
`:1934-1938` pass `&self.state`; the doc at `:877-892` carries the `[CYRUP-DELTA]` note for the
unported `trustedSessionFiles`/`trustedSessionFileRoot` rung (residual 1). `unique_paths`
(`:1036-1047`) is pi's `uniquePaths`. The containment gate itself is unchanged:
`src/background/fleet_view.rs:163-177` `read_contained_text_tail` still refuses an empty root list
and any file outside every root; `:685-701` `read_session_transcript_tail`; `:817` / `:909-918`
`format_async_run_transcript`'s third rung. (Two in-code citations are off, doc-only, not changed by this row: `fleet.rs:877` cites
`asyncDetail` as `fleet.ts:551-585` — its first line is `:550`; `fleet.rs:957` cites `detailLines` as
`fleet.ts:588-599` @v0.64.0 — it is `:606-617` (`:588` is inside `externalDetail`).)

**Design decision (recorded per DESIGN-GUIDANCE, in the commit body):** no new type — the roots are
a plain `Vec<PathBuf>` like the sibling `TranscriptTarget::trusted_roots` (`fleet.rs:1032`) and the
existing `session_roots: &[PathBuf]` parameter; the invariant (absolute, deduplicated, pi's order) is
produced by one pure seeding function kept out of the executor's I/O, which is where pi computes it
(`index.ts:895-898`). Rejected: (a) unioning `SubagentExecutor::transcript_session_roots`'s
cyrup-original triple into the fleet roots — artifact directories, not session roots, and pi's fleet
does not trust them; (b) adding `session_root` to `AsyncRunView`/`TrackedJob` to mirror
`trackedJob?.sessionRoot` — cyrup's `RunStatus` records no session root (`rg session_root
src/background` → 0), and for a run of the current session pi's value IS the parent rung the state
already lists (`subagent-executor.ts:1687`), so the union would add nothing (the restored-run case
is residual 2); (c) copying the roots onto `FleetSnapshot` — the component already owns `state`, and
pi threads `state` through `detailLines`.

**Tests (RED established by the implementer by restoring the `&[]` argument with everything else in
place — both fleet tests fail with the `without a trusted root` refusal; GREEN with the real
argument; the other four fail pre-fix by construction, the symbols did not exist; this ledger pass
re-ran the six by name at `681f6255`: 6/6 pass):**
`tui::fleet::tests::async_detail_reads_the_session_transcript_tail_inside_a_trusted_session_root`
(pi's `fleet.test.ts:1028` case: a run recording only a session JSONL under a trusted root renders
`assistant: TRUSTED SESSION FALLBACK` under `Session transcript tail from <file>` with neither
`without a trusted root` nor `Session read failed`),
`…::async_detail_still_refuses_a_session_file_outside_every_trusted_root` (a file outside every
root → `Warnings:` + `Session read failed for … outside trusted roots` + `(no transcript lines
available yet)`, contents never leaked; an empty root list → `without a trusted root`, pi's `[]`
default at `index.ts:447`);
`extension::executor::paths::tests::subagent_session_root_is_the_parents_dir_joined_with_its_jsonl_less_basename`,
`…::trusted_session_roots_are_pis_two_rungs_in_pis_order`,
`…::trusted_session_roots_expand_tilde_resolve_relative_dedupe_and_start_empty`;
`extension::executor::status::tests::fleet_state_seeds_trusted_session_roots_from_the_configured_default_session_dir`.
Checks at `681f6255`: `cargo fmt --all -- --check`, `cargo clippy -p cyrup-ext-subagents
--all-targets -- -D warnings` — clean (both re-run by this ledger pass); the landing commit
additionally reports `RUSTDOCFLAGS='-D warnings' cargo doc -p cyrup-ext-subagents --no-deps` clean
and `cargo nextest run -p cyrup-ext-subagents --no-fail-fast` 2723/2724, the one failure
`SUBA-087`'s pre-existing ordering flake (`SUBA-090` residual 3; fixed at `6cf2cb9f`), untouched file, passes alone.

**Falsification** — with `subagents.defaultSessionDir` configured and a background run whose
`status.json` records a `sessionFile` under it but has no `output-<i>.log` and an empty
`recentOutput`, opening the run in the fleet inspector must show `Session transcript tail from
<file>` followed by the JSONL tail and no `Warnings:` block; the same run with its session file
outside every trusted root must show `Session read failed for … outside trusted roots` and never
its contents. Either failing reopens the row.

**Residuals — recorded, not closed by this row.** (1) **low — the `trustedSessionFiles` /
`trustedSessionFileRoot` rung** (`fleet.ts:558-559`; `fleet-view.ts:252`, gate `:135`; seeded at
`index.ts:894`, v0.57.0+): pi additionally permits the run's own recorded `sessionFile` when it sits
under `<agentDir>/sessions`, even with `trustedSessionRoots` empty. `format_async_run_transcript`
has no such parameters and `read_contained_text_tail`'s gate is roots-only, so a session file
recorded directly under the Pi sessions base (not under a subagent session root or
`defaultSessionDir`) is refused where pi reads it. (2) **low — `trackedJob?.sessionRoot`**
(`fleet.ts:557`; `status.json` `sessionRoot`, `async-status.ts:416`): a run restored from a
PREVIOUS parent session carries that parent's subagent session root in pi and is trusted through the
tracker; cyrup's `RunStatus` and `TrackedJob` record no session root, so only the current parent's
rung is trusted. (3) **low — `subagent status view:transcript` root composition**
(`status.rs:413-425` `transcript_session_roots` = `[async root, project subagents dir, temp
artifacts dir]`, cyrup-original) differs from pi's `trustedSessionRootsForStatus`
(`subagent-executor.ts:576-581` = `defaultSessionDir` + the parent's subagent session root — the
same pair the fleet now uses): the two cyrup consumers trust different roots. A separate fidelity
question for the status path, flagged by the verifier. (4) **note, not a gap:** the parent-session
rung is seeded for parity, but at HEAD no cyrup child writes under it — `foreground.rs:640-647`
(`[CYRUP-DELTA]`) runs a child with neither an explicit `sessionDir` nor a configured
`default_session_dir` under `--no-session` — so the configured-directory rung is the one the tests
exercise. (5) **ledger tooling, for the final ledger agent:**
`docs/gap-analysis/scripts/count_open_items.py:379` still hand-enumerates `carried_medium =
["SUBA-087", …, "SUBA-091"]`; all five are now table rows and would count twice.

---

## Carried — NOT adversarially verified

> **2026-09-04: three of the eight rows this section was written for — `SUBA-082`, `SUBA-084`,
> `SUBA-086` — were held to the confirmed bar, confirmed, ported and CLOSED; each now has a full
> section in the confirmed set above (in id order) and only a pointer remains here; `SUBA-087`,
> `SUBA-088`, `SUBA-089`, `SUBA-090` and — last — `SUBA-091` (`681f6255`) followed the same day.
> Nothing is carried at this section's lower standard any more; the pointers below are the record.**

> **READ THIS BEFORE ACTING ON ANYTHING IN THIS SECTION.** The refutation pass for this batch was
> capped at twelve items. The eight items below were produced by the same analyst lenses as the
> confirmed set and are carried forward **unrefuted**. They are held to a lower evidence standard and
> **must not be counted alongside the verified items** — the same treatment `README.md` gives
> `DRIFT-023` / `DRIFT-040` as *leads*.
>
> **What this writer did personally verify:** the **port-side zero-hit greps** for every one of them,
> re-run against the current tree at HEAD `6db22a7`, plus the port line ranges quoted in
> `SUBA-089`, `SUBA-090` and `SUBA-091`. Those results are marked *(re-verified)* below.
> **What was NOT re-verified: every upstream line number in this section.** They are the analyst's,
> reproduced as filed, and a maintainer must settle each with `git show v0.57.0:<path>` before
> scheduling the item. Where a filing asserts an upstream shape, treat it as a hypothesis.

### ~~SUBA-082~~ — **PROMOTED AND CLOSED 2026-09-04** — see `## ~~SUBA-082~~` in the confirmed set above (landing commit `5a4ae4ed`)

### ~~SUBA-084~~ — **PROMOTED AND CLOSED 2026-09-04** — see `## ~~SUBA-084~~` in the confirmed set above (landing commit `dee8b9d0`)

### ~~SUBA-086~~ — **PROMOTED AND CLOSED 2026-09-04** — see `## ~~SUBA-086~~` in the confirmed set above (landing commit `275c1f85`)

### ~~SUBA-087~~ — **PROMOTED AND PARTIALLY CLOSED 2026-09-04** — see `## ~~SUBA-087~~` in the confirmed set above (landing commit `2d9d0d0a`)

### ~~SUBA-088~~ — **PROMOTED AND CLOSED 2026-09-04** — see `## ~~SUBA-088~~` in the confirmed set above (landing commit `ba24e5e5`)

### ~~SUBA-089~~ — **PROMOTED AND CLOSED 2026-09-04** — see `## ~~SUBA-089~~` in the confirmed set above (landing commit `cde2ddfc`)

### ~~SUBA-090~~ — **PROMOTED AND PARTIALLY CLOSED 2026-09-04** — see `## ~~SUBA-090~~` in the confirmed set above (landing commit `79ee7eff`)

### ~~SUBA-091~~ — **PROMOTED AND CLOSED 2026-09-04** — see `## ~~SUBA-091~~` in the confirmed set above (landing commit `681f6255`)

---

## ~~SUBA-092~~ — ~~high~~ **CLOSED 2026-09-04** — Agent `excludeTools:`/`allowNestedSubagents:` (frontmatter and settings-override) are unported: a declared tool exclusion has no effect, and nested-subagent authorization can only ever come from an explicit `tools:` allowlist

> **CLOSED 2026-09-04, landing commit `247ff97b`, re-read at cyrup code HEAD `275c1f85`.** Ported at
> v0.64.0 per ADR-0006: the consumer `runs/shared/pi-args.ts` is byte-identical between v0.62.0 and
> v0.64.0 for the tool-plan/exclusion logic (`git -C tmp/pi-subagents diff v0.62.0 v0.64.0 -- src/runs/shared/pi-args.ts`
> touches only task-delivery/watchdog lines); `b26da18e` (#1778) is the introducing commit. **What
> landed**: `discovery/types.rs:997 AgentDefinition::exclude_tools: Option<Vec<String>>` and `:1005
> allow_nested_subagents: Option<bool>` (`agents.ts:140-141`); `AgentOverrideConfig::exclude_tools:
> OverrideField<Vec<String>>` / `allow_nested_subagents: OverrideField<bool>` (`:687`, `:693`, in
> `is_empty` at `:742-743`; `false` → `ExplicitClear`, pinned by
> `parse_subagent_settings_reads_the_suba092_false_shapes`) (`agents.ts:104-105`, `:1097-1102`,
> `parseOverrideStringArrayOrFalse` `:921-940`); `discovery/frontmatter.rs:95-96` both keys in
> `KNOWN_FIELDS` (`agent-serializer.ts:12-13`), `:956` `excludeTools` via `parse_frontmatter_list`
> (`agents.ts:1988`, `frontmatter.ts:46-57`), `:1166` `allowNestedSubagents` strict `true`/`false` with
> the crate's per-file skip+warn (`agents.ts:2061-2066`); `frontmatter_write.rs:92-110` serializer arms
> (`agent-serializer.ts:74-78`); `merge.rs:463-471` builtin arm full-replace (`agents.ts:1404-1405`
> @v0.64.0: `false` → delete) and `:757-771` custom arm fill-unset gated on frontmatter presence
> (`agents.ts:1547-1552` @v0.62.0 — see residual 1); `exec/agent_config.rs:54,58` on `AgentConfig`
> and `:193,197` on `ResolvedAgentPersona` so chain/parallel/background dispatch carries them
> (`async-execution.ts:948-949,1011-1012,1741-1742`); `exec/spawn_plan.rs::resolve_child_tools`
> `:698-712` trims, dedups and subtracts `exclude_tools` from the ceiling-filtered declared builtins
> (`pi-args.ts:502-504` `effectiveDeclaredBuiltinTools`), `:743-750` `fanout_authorized =
> effective.includes(subagent) || (allow_nested_subagents == Some(true) && !excluded(subagent) &&
> ceiling.is_none_or(has subagent))` (`:505-509`), `:804` MCP-name exclusion (`:478`), the filtered
> list also feeding `--tools`/`--no-tools` and `REQUIRED_CHILD_TOOLS` (`:550,565`), and `:827-834`
> `--exclude-tools <csv>` on the no-allowlist arm (`:776-777`) — consumed by cyrup's own CLI flag
> `crates/cyrup/src/cli/args.rs:106` → `cyrup-session-svc/src/builder.rs::select_active_tools`.
> **Verify, each clause a passing test** (`exec/spawn_plan.rs`):
> `exclude_tools_on_an_agent_with_no_allowlist_reaches_the_child_as_exclude_tools` (no `tools:` +
> `excludeTools:[bash]` → `--exclude-tools bash`, no `--tools`); `exclude_tools_subtracts_from_an_explicit_allowlist`
> (`tools:[bash,edit]` + `excludeTools:[bash]` → `--tools edit`; excluding all → `--no-tools`);
> `allow_nested_subagents_grants_fanout_without_an_explicit_tools_allowlist` (unset/false → fanout env
> 0, true → 1 and `RegistrationMode::ChildSafe`); `excluding_the_subagent_tool_revokes_fanout_from_both_the_allowlist_and_the_nested_grant`;
> `allow_nested_subagents_is_vetoed_by_a_ceiling_that_omits_the_subagent_tool` (`:508`); plus the
> parse/serialize/override-merge tests in `frontmatter.rs`, `frontmatter_write.rs`, `merge.rs`,
> `discovery/mod.rs`, `agent_config.rs` — 16 in all, each naming a field absent at `a4805955`; crate
> 2613/2613 at `247ff97b`. **Residuals — recorded, not closed.** (1) **Custom-agent override
> precedence at v0.64.0**: `31562d76` (#1798, first tag v0.63.0) made `applyCustomAgentOverride`
> delegate to `applyBuiltinOverride` for EVERY key; cyrup's `apply_custom_override` still implements
> v0.62.0's fill-unset (R-SA-010) for all 20 fields, these two included, for consistency — a
> cross-field change, ownerless lead in the summary blockquote. (2) Management surface not ported:
> `agentUpdate`'s `config.excludeTools` (`agent-management.ts:487-497`), the `excludes:` suffix in
> list (`:738`), `Excluded tools:` in show (`:885`); update/rename preserve an author's values. (3)
> `AgentDefinition::is_nested_fanout_eligible` (test-only consumers) does not consult the new fields.
> (4) pi's `internalTools` exclusion (`pi-args.ts:517`) has no counterpart by the existing
> `[CYRUP-DELTA]` on structured output. (5) The direct-MCP exclusion arm (`spawn_plan.rs:804`) is
> untested in isolation. (6) Runtime-registry threading of the two fields landed with `SUBA-084`.
> Also: the `crates/cyrup-it` `intercom` fixture was pre-broken by these two new fields and was
> repaired inside `SUBA-082`'s commit (`5a4ae4ed`).

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed
**Subsystem** discovery / agent definition schema (new since this file's own v0.57.0 scope)
**Window** v0.57.0..v0.62.0 (`b26da18e feat: add per-agent tool exclusions (#1778)`, 2026-08-31 —
first tag containing it is v0.62.0; absent at v0.57.0 by `git cat-file -e`). Filed 2026-09-04 during
the area-09/09a re-audit, from the diff-stat skim `git -C tmp/pi-subagents diff --stat v0.57.0..v0.64.0`
this file's own scope note anticipates, per the task brief's "conservative, evidenced new items only."

**upstream** — `git show v0.62.0:src/agents/agents.ts`: `excludeTools?: string[]` added to
`BuiltinAgentOverrideBase` (`:73`), `BuiltinAgentOverrideConfig` as `string[] | false` (`:103`), and
`AgentConfig` itself (`:137`) — i.e. it is BOTH a frontmatter key on the agent's own definition and a
settings-override field, parsed by `parseOverrideStringArrayOrFalse` (`:1093-1094`) and applied at
`applyBuiltinOverride` (`:1381`) and `applyCustomAgentOverride` (`:1547-1549`).
`git show v0.62.0:src/agents/agent-serializer.ts:12` adds `"excludeTools"` to `KNOWN_FIELDS` (next to
the pre-existing `"allowNestedSubagents"`, `:13`, which this crate also has no field for). Consumed at
`git show v0.62.0:src/runs/shared/pi-args.ts:502-508`:
```ts
const excludeTools = [...new Set((input.excludeTools ?? []).map(t => t.trim()).filter(Boolean))];
const excludedToolSet = new Set(excludeTools);
const effectiveDeclaredBuiltinTools = declaredBuiltinTools.filter(tool => !excludedToolSet.has(tool));
const fanoutAuthorized = effectiveDeclaredBuiltinTools.includes("subagent") || (
	input.allowNestedSubagents === true && !excludedToolSet.has("subagent") &&
	(!allowedToolSet || allowedToolSet.has("subagent"))
);
```
— `excludeTools` subtracts from the declared builtin set **regardless of whether `tools:` was set at
all** (it filters `declaredBuiltinTools`, which is the full ambient set on the no-`tools:` arm per
`SUBA-072`'s own evidence), and `allowNestedSubagents` is an INDEPENDENT grant of `subagent` access
for an agent that declares no explicit tool allowlist. `preflight.ts` threads both into the launch
contract (`:114`, `:360`, `:440`, `:491`).

**cyrup** — `grep -rniE 'excludeTools|exclude_tools' crates/cyrup-ext-subagents/src` hits only
`exec/mcp_direct_tools.rs`, a **different, pre-existing `excludeTools`** scoped to MCP-server tool
expansion (`McpDirectToolSelection`'s own filter), not the agent-level key this item is about.
`grep -rniE 'allowNestedSubagents|allow_nested_subagents' crates/cyrup-ext-subagents/src` → **0 hits**
anywhere in the crate. `discovery/frontmatter.rs`'s `KNOWN_FIELDS` has neither `"excludeTools"` nor
`"allowNestedSubagents"` (both would fall to `extra_fields` if authored), and
`discovery/types.rs::AgentDefinition`/`AgentOverrideConfig` (already read in full for `SUBA-081`) have
no such fields — `AgentOverrideConfig`'s 18 fields (post-`SUBA-081` partial closure) do not include
either. `exec/spawn_plan.rs`'s tool-allowlist block (already read in full for `SUBA-072`) has no
subtractive step after `declared_builtin_tools`/`ceiling_allowed_tools` are assembled.

**Impact** — An agent author writing `excludeTools: [bash]` in frontmatter, or an operator writing
`agentOverrides.<n>.excludeTools: ["bash"]` in settings, expecting an otherwise-unrestricted agent to
lose `bash` specifically, gets no restriction at all: the field round-trips into `extra_fields` (or is
silently dropped from a settings override, since the struct has no slot to deserialize it into) and
the agent keeps the full ambient tool set. This is the SAME shape as `SUBA-072`'s pre-fix defect —
a declared, negative tool constraint that is accepted and inert — but at the agent-schema layer
instead of the ceiling layer, and `SUBA-072`'s own closure does not cover it (the ceiling's
`allowedTools`/`denyExtensions` axes are a session-level bound; `excludeTools` is a per-agent
declaration with no ceiling involvement at all). Separately, `allowNestedSubagents` has no cyrup
representation, so there is no way to grant an agent `subagent` access without also declaring a full
`tools:` allowlist that names it — a narrower capability model than upstream's, not a widening, but a
real behavioural difference an agent author relying on upstream's docs would hit.

**Fix** — Add `exclude_tools: Option<Vec<String>>` (frontmatter, on `AgentDefinition`) and
`OverrideField<Vec<String>>` (on `AgentOverrideConfig`), wire both into `KNOWN_FIELDS`, and in
`exec/spawn_plan.rs` filter `declared_builtin_tools` (and any MCP-selected names) through the
exclusion set at the same point `ceiling_allowed_tools` is applied, before `explicit_tool_allowlist`
is decided — mirroring `pi-args.ts:502-508`'s `effectiveDeclaredBuiltinTools`. Add
`allow_nested_subagents: Option<bool>` similarly and fold it into the `subagent`-tool fanout-grant
check beside the existing allowlist test.

**Verify** — An agent (frontmatter or override) declaring `excludeTools: [bash]` with no `tools:` must
spawn without `bash` in its allowlist and with every other builtin tool present; the same agent with
`tools: [bash, edit]` must lose `bash` and keep `edit`. An agent with no `tools:` and
`allowNestedSubagents: true` must be able to call `subagent`; the same agent with
`allowNestedSubagents` unset or false must not.

**Relation to corpus** — New; not covered by `SUBA-072`/`SUBA-073`/`SUBA-081` (checked: none of their
closure evidence, re-read for this pass, mentions `excludeTools` or `allowNestedSubagents`). Distinct
from area 09's `SUBA-006`/`SUBA-014`, which are about the *existing* `tools:` allowlist mechanism.

---

## ~~SUBA-093~~ — ~~medium~~ **CLOSED 2026-09-04** — A child-scoped `stop` could not address a `ParallelGroup` member: cyrup's status model had ONE step per group, upstream flattens members into `steps[]`

> **CLOSED 2026-09-04 — landing commit `07f2df0d`** (code), on top of `71447a19`. Both sides read
> for this pass: cyrup at HEAD, `nicobailon/pi-subagents` at **v0.64.0** (the ADR-0006 parity
> target) via `git show`. The filing was accurate in every particular and is reproduced below with
> the port-side half rewritten to what now exists.

> **Filed 2026-09-04 from `SUBA-087`'s residual (1)** (review fix `6cf2cb9f`), so the residual is a
> counted row rather than prose inside a closed one. It is the filing's headline scenario for
> `SUBA-087` (`subagent({action:"stop", id, childId})` against a `tasks[]` fan-out); everything else
> in that row landed.

**Kind** port limitation (status model) · **Severity** medium · **Effort** M · **Confidence** confirmed
(both sides read for `SUBA-087` at v0.57.0 and v0.64.0; cyrup re-read at HEAD; re-verified at
v0.64.0 for this closure)

**cyrup (before, at `71447a19`)** — `crates/cyrup-ext-subagents/src/background/runner_main.rs`
`pending_step_status_for`: a `RunnerStep::ParallelGroup` became ONE `StepStatus` labelled
`<parallel:N tasks>`; its members' per-child detail reached `RunStatus::parallel_groups` only when
the group settled (`record_step_outcome`). `background/child_identity.rs` resolves a `childId`
against `RunStatus::steps` by index/agent/workflow key, so `step:<i>` for a group index targeted the
group's single entry and `route_child_stop_requests` fired the ONE stop handle registered for that
top-level index — the whole group was torn down. The telemetry pump, steer targeting, the child's
intercom presence label, the artifact quadruple and `output-<i>.log` all keyed on the same top-level
index, published once per group into an `Arc<AtomicUsize>` (`current_flat_index`) that every
concurrently-running member read the same value from.

**upstream** — `src/runs/background/subagent-runner.ts` @v0.64.0: every member of a parallel group
is its own flat step. `:2612-2652` is the declaration-time flatten (`flatStepCount`, one
`initialStatusSteps.push` per `step.parallel` task, `agent: task.agent`); `:1294` `flatIndex` on the
step context, spread per member at each dispatch site (`:4245` parallel, `:4640` dynamic, `:5017`
sequential); the flat index is the `stepIndex`/`childIndex` of every event and file a step produces
(`:1472-1508`, `:1709`, `:1746`, `:1762`, `:1829`), the `output-${fi}.log` and steer paths
(`:4243-4257`), and the key of `registerStepStop` (`:4268`, `:3048-3055`) and the
`childStopRequests.has(fi)` gate (`:4221`). `markChildStopRequested` (`:2979-2991`) and
`stopChildStep` (`:3015-3031`) therefore address one member, and a member torn down by its own stop
records `exitCode: 1` (`:4286-4295`), not the paused-success an interrupt yields.

**Fix (landed)** — `07f2df0d`:
* NEW `crates/cyrup-ext-subagents/src/background/flat_index.rs` — the flatten as PURE functions
  (no I/O, no clock, no status handle): `flat_step_width` (a `ParallelGroup` is as wide as its
  member list, everything else is 1), `flat_base`, `flat_range`, `flat_total`, and
  `pending_step_statuses_for`, which yields one `Pending` `StepStatus` per member named by that
  member's OWN agent. Its module docs state the two index spaces the crate now distinguishes.
* `background/runner_main.rs` — `publish_initial_status` and `append_steps` build the flat list;
  `run_inner` derives `flat_range(&steps, cursor)` once per iteration and keys `mark_step_running`
  (over the whole block), `current_step`, the `subagent.step.*` `stepIndex`, the loop-top queued-stop
  skip (only for a step that occupies exactly one slot — pi's sequential branch), the
  `mark_remaining_{paused,timed_out,stopped}` sweeps and `settle_step_result` on it.
  `record_step_outcome` takes a slot RANGE and folds a group's per-child outcomes onto the members'
  own entries; `settle_step_result` marks each child-stopped member `Stopped` (pi's
  `markChildStopped`) with `subagent.step.stopped` + terminal `subagent.child-status`, and the run
  stays alive.
* `ExecSingleStepExecutor` — the `current_flat_index` atomic is GONE. Every per-child surface reads
  `ctx.step_slot` instead, which fixes the telemetry tag, `RunOptions::child_index`, the steer
  inbox/ack/capability paths, the artifact index and the stop-handle lookup in one substitution.
  `run_single` registers and clears its own stop token per DISPATCH (pi's `registerStop` at each of
  its three dispatch sites), refuses to spawn when a stop is already queued against its slot (pi
  `:4221`), and returns pi's stopped result (`exitCode: 1`) for a child it stopped — without which a
  stopped member came back as an interrupt's paused-success, its group's aggregate stayed
  successful, and the RUN ended `Complete` (observed, then fixed, during this port).
* `spawn/chain_graph.rs` — `ChainRunContext::step_slot`, and `dispatch_group` re-stamps it per
  fanned-out child (pi's `{...ctx, flatIndex: fi}`).

**Design decisions (recorded in the commit body)** — the invariant encoded is "which flat slot does
this dispatch own, and does it own it ALONE", as the domain enum `StepSlot::{Exclusive(usize),
Shared(usize)}` on the per-dispatch context, with `GroupSlotLayout::{PerMember, SharedSlot}` deciding
the mapping at the one place a group fans out. `StepSlot::exclusive_index()` is the only way to
obtain an index for a per-child stop handle and returns `None` for a shared slot, so two live
siblings can never be registered under one index — the failure mode that would make a stop kill the
wrong child; `index()` stays available for the accumulating surfaces (event `stepIndex`,
`output-<i>.log`, telemetry fold) that tolerate sharing. Rejected: a `FlatStepIndex` newtype (the two
index spaces coexist in exactly ONE function, and the newtype would have to cross
`ChildStopRegistry`, the JSON event payloads, `child_identity`'s `step:<i>` parse and the
TUI/`cyrup-it` readers to buy a check `flat_range` already makes structural); a bare
`flat_index: usize` + `exclusive: bool` pair (the bool is droppable at a call site and silently means
"safe"); an extra `run_single` parameter (a wider trait change for information the context already
carries, and further from upstream). Migration cost: one new `ChainRunContext` field (5 construction
sites, 3 in tests), `record_step_outcome` takes a `&Range<usize>`, and
`ParallelGroupStatus::group_step_index` now means the group's flat BASE — read only by
`runner_main`'s own sweeps, which moved with it. `status.json`'s `steps` array is longer for a
parallel run, which is what upstream writes; `chain_step_count` still counts top-level steps.

**Verify (each fails before, passes after; crate 2734/2734, `cyrup-it` 493/493)** — unit:
`flat_index.rs` `a_parallel_group_is_as_wide_as_its_member_list_and_every_other_shape_is_one`,
`flat_base_accumulates_group_widths_ahead_of_the_cursor`,
`a_base_past_the_end_is_the_append_position_and_its_range_is_empty`,
`a_parallel_group_publishes_one_pending_entry_per_member_named_by_its_own_agent`,
`a_dynamic_group_and_a_single_step_each_publish_exactly_one_entry`; `chain_graph.rs`
`every_parallel_group_member_is_dispatched_under_its_own_exclusive_flat_slot` (group base 4 →
`Exclusive(4/5/6)`), `dynamic_group_members_share_one_slot_and_it_is_marked_shared`;
`runner_main.rs` `a_parallel_groups_member_outcomes_land_on_their_own_flat_status_entries`,
`a_single_step_records_on_its_own_slot_and_flat_bases_skip_a_groups_width`. Integration:
`crates/cyrup-it/tests/subagents/background_runner_main_integration.rs`
`a_child_scoped_stop_kills_one_fan_out_member_and_its_siblings_still_complete` (real 3-member
fan-out, real child processes via the scripted fixture, `childId: "step:1"` delivered 300ms in).
Every unit test fails before by construction (the symbols did not exist). The integration test was
additionally run against a SIMULATED pre-change tree (parallel width forced back to 1 and
`GroupSlotLayout::SharedSlot` for a parallel group) and fails there with
`steps: ["<parallel:3 tasks>"]`, run state `Complete` and `recent_output: ["DONE","DONE","DONE"]` —
the stop had no observable effect at all. After: `["first","second","only"]`, member 1 `Stopped`
with `stop_requested`/`stop_requested_at`/`stopped` and the stop message, members 0 and 2 `Complete`
with their own `DONE`, events `subagent.step.stop_requested`(stepIndex 1, childId `step:1`, agent
`second`) → child-status `stopping` → `subagent.step.stopped` → child-status `stopped`, no
`subagent.run.stopped`, run `Failed`.

**Falsification** — `subagent({action:"stop", id, childId:"step:1"})` against a running 3-task
`tasks[]` group must tear down member 1 only, the other two completing with their own outputs, and
`status.json` must carry three `steps` entries named by their own agents. Any of those failing
reopens the row.

**Residuals — recorded, not closed by this row.** (1) **low — the DYNAMIC half.** A
`RunnerStep::DynamicGroup` still occupies ONE flat slot, which is also what upstream DECLARES for it
(`subagent-runner.ts:2656-2670`, a single `expand:<agent>` placeholder); upstream then SPLICES that
entry into one-per-materialized-item at expansion time (`:4155`, shifting every later group's
`start` and every later workflow node's `flatIndex`), and cyrup does not, because a dynamic group's
width is unknown until dispatch and the splice would move the flat base of every later step mid-run.
Its members are marked `StepSlot::Shared`, which keeps them out of the per-child stop registry
rather than letting them corrupt each other's handles, so a dynamic fan-out remains addressable only
as a whole. (2) **low — `Running` granularity `[CYRUP-DELTA]`.** Every member of a group is marked
`Running` when the GROUP is dispatched, where pi marks each member as its own worker claims it
(`:4236-4238`); cyrup's fan-out happens behind `chain_graph::walk_chain`, which reports no per-member
start. Nothing keys on the distinction — `is_stoppable_step_state` accepts `Pending` and `Running`
alike, and `route_child_stop_requests` finds the live handle either way — but under a concurrency
limit a member can read `Running` slightly before its worker claims a permit. (3) **low — the
dynamic placeholder's label** stays cyrup's `<dynamic:<collect>>` rather than upstream's
`expand:<agent>` (`:2659`); a rename with no behavioural content, deliberately not taken. (4) The
one-`SingleResult`-per-top-level-step shape of `ResultFile::results` is unchanged: a group still
contributes its aggregate, where upstream contributes one entry per member. The RUN's terminal state
now agrees with upstream (a stopped member fails the aggregate), but a `ResultFile` reader still sees
one collapsed record for the group — pre-existing, and out of this row's scope.

---

## SUBA-094 — A `display: false` completion notice still renders: the session-svc trigger-turn injection drops `display` because `AgentMessage::Custom` cannot carry it

> **Filed 2026-09-04 from `SUBA-090`'s residual (1)** (review fix `6cf2cb9f`), so the residual is a
> counted row rather than prose inside a closed one. **FIX SITE `crates/cyrup-session-svc` and
> `crates/cyrup-agent` (areas 08 / 03), not this crate** — filed here so the enumeration is not
> lost; the area-08 ledger agent may re-home it.

**Kind** port-bug (cross-crate seam) · **Severity** medium · **Effort** M · **Confidence** confirmed
(both sides read for `SUBA-090` at v0.43.0, v0.57.0 and v0.64.0; cyrup re-read at HEAD).

**cyrup** — `crates/cyrup-ext-subagents` now computes `display` per pi's predicate
(`SUBA-090`, `79ee7eff`: `completion_notice_display` = `outcome != Completed`) and hands it to
`inject_message(content, custom_type, display, details, trigger_turn)` with `trigger_turn = true`.
`crates/cyrup-session-svc/src/session/inject.rs:117-160` consults `display` ONLY in the idle,
non-trigger-turn branch (`append_custom_message(&kind, .., display, details)`); on the trigger-turn
path it builds `AgentMessage::Custom { kind, payload, details, timestamp }`
(`crates/cyrup-agent/src/event.rs:39-51` — no `display` field) and `spawn_run`s over it, so the TUI
renders the notice from `message_end` regardless.

**upstream** — `src/runs/background/notify.ts` @v0.64.0: `display` computed at `:402`
(`details.some(d => d.source === "foreground" || d.status !== "completed" || d.scheduleOrigin !==
undefined)`) and passed on the `sendMessage` call at `:408` together with `triggerTurn` (`:603-617`
default `true`); pi's `sendMessage({customType, content, display, triggerTurn})` honours `display:
false` on a triggering message — the model sees the notice, the screen does not.

**Impact** — every plain successful background completion is still drawn on screen; the `SUBA-090`
fix is inert at the one surface a user sees.

**Fix** — carry `display` with the Custom message: an `Option<bool>`/`bool` on
`AgentMessage::Custom`, threaded by `inject.rs`'s trigger-turn branch and honoured by the TUI
renderer (`cyrup-tui/src/app/extension_render.rs` reads the message off `message_end`).

**Verify** — a background run that completes cleanly must reach the model (next turn sees the
notice) and NOT be drawn on screen; a failed one must be drawn. Today both are drawn.

---

## Refuted

Recorded so it is never re-derived.

### SUBA-080 — REFUTED — "Fleet-view run transcripts are rendered without terminal-control sanitization"

**The upstream evidence was accurate.** `git show v0.57.0:src/runs/background/fleet-view.ts` does
define `safeTranscriptLines` and apply it at four sites; `run-status.ts` applies
`lines.map(safeTerminalText)` at the end of `formatRememberedForegroundTranscript`;
`shared/display-text.ts:139` is `safeTerminalText` with the described code-point classes.
**The port-side claim is wrong: neutralization is provided by the host layer.**

- **TUI fleet-detail path.** `crates/cyrup-ext-subagents/src/tui/fleet.rs:837 async_detail` →
  `src/tui/fleet_overlay.rs` (`cyrup_ext::OverlayLine`) → `crates/cyrup-tui/src/overlay.rs:167,195`
  `to_ratatui_line`/`to_ratatui_span` → painted by ratatui `Paragraph`/`frame.render_widget`.
  ratatui-core 0.1.2 filters every control-character grapheme at **both** paint points:
  `…/ratatui-core-0.1.2/src/text/span.rs:314` `.filter(|g| !g.contains(char::is_control))`
  (`Span::styled_graphemes`) and `…/src/buffer/buffer.rs:351`, the same filter in
  `Buffer::set_stringn`.
- **`subagent` status-tool path.** `src/extension/executor/status.rs:354` returns the string as a
  tool result, rendered through `crates/cyrup-tui/src/transcript/tool_result.rs:61,68,90,94,99`
  `result_text`, every branch of which calls `crate::ansi::sanitize_display_text` —
  `crates/cyrup-tui/src/ansi.rs:25` `sanitize_binary_output(strip_ansi(text)).replace('\r', "")`,
  where `strip_ansi` (`:60`) consumes whole CSI/OSC sequences and `sanitize_binary_output` (`:36`)
  drops every C0 except TAB/LF/CR plus U+FFF9..=U+FFFB.
- `crates/cyrup-tui/src/ansi.rs:7-18` states the contract explicitly: *"ratatui filters control
  characters out of every grapheme run before it reaches a cell … so a bare `ESC` can never be written
  to the terminal and an escape sequence cannot **execute** — no cursor moves, no title rewrite, no
  hidden text."*
- An in-crate sanitizer exists as well: `src/tui/fleet_transcript.rs:88,123,152`
  (`BINARY_CONTENT_PLACEHOLDER`, `looks_like_binary_content`, `safe_display_text`), `:553`
  `safe_transcript_event`, with tests at `:1835-1864` and `:2106`. In `wrapped_detail`
  (`src/tui/fleet.rs:1803-1856`) the sanitized structured-transcript renderer is the **preferred**
  branch; `detail_lines`/`async_detail` is only the fallback when the child has no readable transcript
  events.
- Workspace-wide check for a second sanitizer:
  `grep -rnE "is_unsafe_display_code_point|safe_terminal_text|binary content omitted|fn safe_display_text" --include=*.rs crates/ | grep -v cyrup-ext-subagents`
  → 0 hits — i.e. the neutralization is not a duplicated in-crate copy, it is the host's.

**Verdict:** a different shape achieving the same observable behaviour. Not a gap. Note that
`SUBA-091` was a *different* defect in the same function; it was confirmed and CLOSED at `681f6255` on
2026-09-04 (see `## ~~SUBA-091~~`).

---

## Already tracked

Dropped rather than filed, with the item that owns each. Recorded in full so the next pass does not
re-derive them.

| Candidate | Owner | Why dropped |
|---|---|---|
| 23 of upstream's 52 tool verbs unadvertised (port advertises 30) | `SUBA-005` (tracker) | This is the census, not the work, and `SUBA-005` already owns exactly it and names the same unowned verbs. Its re-measured figures (**30 vs 52**; `+validate`/`+debug.run`, `−append-step`/`−approve-checkpoint`/`−reject-checkpoint` via `7ece6f35`) belong in `SUBA-005`'s body as a maintenance update. The one verb with real wedging behaviour behind it is filed as `SUBA-085`. |
| `schedule.*` (nine verbs) unported | `SUBA-016` / PARITY-GAPS `PB-11` | Fully covered, including the corrected nine-verb count and the BLOCKED-on-`workflowScript` determination; the port states it at `background/control.rs:367`. |
| `refine` / `refine.show` / `refine.rollback` + `/subagents-refine` | `VL-S13` (+ `VL-S11`) | Covered as the agent-refinement WRITE half; `exec/agent_refinements.rs:12-20` documents the split and the read half is live on the production spawn path. |
| Herdr subsystem: six inspector/project verbs, focus, `/subagents-inspect-rpc` | `VL-S6` (+ `PB-8`) | Covered. The port carries a written decision at `tui/fleet.rs:58-60` with an implemented and tested fallback; the RPC half belongs to the unported RPC bridge. |
| `/subagents-detach` unregistered; no configurable detach shortcut | `VL-S11` + `VL-S15` | The command is in `VL-S11`'s three-command list; the shortcut half is the host-seam gap `VL-S15`, not a subagents defect. |
| `workflowScript` runtime, `chatProgress`, `workflowScriptPath`, `action:"validate"`, mission workflow state | `VL-S2` | Covered; the port documents the absence at ~29 sites. **Worth recording in `VL-S2`'s body: the file TRIPLED inside the window (502 / 703 / 1522 lines at v0.43.0 / v0.47.1 / v0.57.0).** |
| Durable workflow receipts, `workflowChildren` summaries, one-use child permit, detach-reconcile | `VL-S2` | A genuinely new layer (four files, all zero bytes at **both** v0.43.0 and v0.47.1), but every one is unreachable without the `workflowScript` runtime and a Workflow RunMode the port cannot represent. A scope-growth note in `VL-S2`, not four rows. The one piece independent of the runtime — `childId` stop — is kept as `SUBA-087`. |
| `children.list` unadvertised | `SUBA-055` closure note / `SUBA-005` | The port declines it with a written, correct reason at `extension/tool/text.rs:190-197`: it lists retained workflow children, so it would advertise a permanently-empty listing. |
| `agentContract` at run level and on every child schema (and `gate`/`gateOn`) | `SUBA-024` / `VL-S10` | Covered; the port names it at `spawn/chain_graph.rs:1859`. Upstream defines `gateOn` as applying only "for chain steps with `agentContract`", so it is the same item. |
| `worktree.discard` + `handoffPath`; parallel-handoff manifest never written | `SUBA-024` / `VL-S10` / `SUBA-005` / `SUBA-064` | The manifest half is the parallel-handoff item; the verb is on `SUBA-005`'s unowned list; and `SUBA-064` already records the prerequisite that whoever lands `worktree.discard` lands the authority gate in the same change. |
| `authorityPolicy` validators exist but the production loader never calls them | `SUBA-064` | Still open, and its Fix already prescribes wiring `validate_authority_policy` into config load beside `validate_missions`. This refines `SUBA-064`'s evidence (the subsystem has since landed; only the loader call is missing) rather than adding a behaviour it does not own. |
| `subagent_wait` missing `nonBlocking`, `stopOnAttention`, auto-drain; no durable wake subscription | `VL-S8` | The non-blocking-subscription and auto-drain half is `VL-S8` verbatim; `stopOnAttention` is a one-flag window addition on the same schema and belongs in that item's body. |
| Durable completion replay / output archives / wait-completion payloads | `SUBA-056` | Covered in full, including the archive and replay-record shapes. |
| Async status snapshots + on-demand child inspection over RPC | `PB-8` | Downstream of the entirely-unported RPC bridge; adds nothing schedulable until `PB-8` lands. |
| Session lease / process-terminal record / owned-process-tree verification | `VL-S3` / `VL-S4` / `SUBA-023` | Already filed on both sides. Additionally established: **the port's KILL path is stronger than upstream's** (process-group negation at `spawn/signal.rs:503-510` vs upstream's single-pid kills); only the terminal-proof record is missing, which is what the existing rows name. |
| Four ignored config keys (`asyncWidget`, `inlineToolDisplay`, `fleetKeybindings`, `legacyChainControls`) + `mainWindowRenderer` | `SUBA-061` | The four are `SUBA-061` verbatim; `mainWindowRenderer` is the same shape and belongs in that list as a fifth key. |
| 21 of 44 `ExtensionConfig` keys unmodelled (bulk census) | `SUBA-061` + the rows above | A census, not a behaviour. Its high-value members are filed individually here (`permissions` → `SUBA-073`, `timeoutMs` → `SUBA-077`, `maxThinking` → `SUBA-078`, `defaultProvider` → `SUBA-088`, `defaultSubagentContext` → `SUBA-079`) or already owned; filing the census too would double-count every one. |

### Dropped as justified divergences or non-gaps

- **`maxSubagentSpawnsPerSession` defaults to 40 where upstream is unlimited** — the field's doc at
  `registration/mod.rs:89-91` cites `func-SA §4.7` as the cyrup requirement setting the default: a
  decision of record. (Contrast `async_by_default` two lines above, which carries no such citation —
  filed as `SUBA-083`.)
- **`PI_SUBAGENT_FS_RETRY_MAX_TOTAL_MS` clamp** — not a gap. Upstream's knob exists because its
  writers are synchronous and `Atomics.wait` parks the Node event loop; the port's writers are async
  (`background/atomic.rs:75`) and never block a shared loop, so there is no behaviour to reproduce.
- **`/prompt-workflow` and `/chain-prompts` reimplemented over `workflowScript` upstream** — not a
  gap. The port reaches the same observable behaviour through the chain shape: same three-tier
  discovery, same eight reserved names, same `' -> '` / `' -- '` splitting, same `{previous}`
  threading, same two error strings (`registration/prompt_workflows.rs`).
- **`--no-context-files` not passed to the child** — not a gap. `src/prompt_runtime.rs`'s
  `BeforeAgentStart` hook (`:1966-1981`) runs `strip_project_context` (`:850-852`) over the child's
  assembled system prompt, removing the project-context block the child itself loaded. Same
  observable result by a different mechanism.
- **`restoreActiveJobs` / restart resumption of in-flight runs** — **not a gap; present under a
  different name.** `src/extension/executor/status.rs:27 resume_tracking`, wired at
  `extension/host/native_impl.rs:347` on `SessionStart`, pinned by `executor/paths.rs:566-630`, which
  asserts both upstream behaviours (terminal runs not re-tracked; restored events cursor seeded at
  EOF). **This is exactly the restructure trap the methodology warns about** — the behaviour reads as
  absent under every name upstream uses.
- **`alignForkedSessionCwd`** — absence could not be established. The port passes the child's cwd into
  `SessionLayout::new(root, cwd)` before branching (`extension/executor/resolve.rs:316-325` →
  `fork_context.rs:190`), so the header may already carry the correct cwd structurally. Dropped rather
  than softened, per the evidence rules.

### Cut at the twenty-item cap — confirmed absent, file next pass

Each was confirmed absent and each was cut on ratio, not on doubt. Recorded so the next pass does not
spend the search again.

- **Per-run logical fan-out budget** (`maxSubagentSpawnsPerRun`, hard cap 64) — zero hits for
  `run_fanout`/`fanout_budget`/`spawns_per_run`. The port does bound a run tree by depth
  (`spawn/depth.rs`, enforced at `runner_main.rs:1219-1241`) and by each step's own width, so the
  exposure is cost, not correctness. M–L.
- **Per-session active async-run capacity** (`maxActiveAsyncRunsPerSession` +
  `capacity.abandonedSlotRelease`) — L: a file-backed slot pool with process-liveness reclamation, for
  a resource-accounting behaviour with no correctness consequence.
- **`toolTimeoutMs` at every level** (call param, frontmatter, config, env) and the fast-tool defaults
  — zero hits crate-wide for every spelling. **Partly subsumed:** once `SUBA-077` restores the
  foreground run-level deadline, a wedged tool call is bounded by the run rather than unbounded.
  Re-file after `SUBA-077` lands, when the residual is per-call granularity alone.
- **Agent-level `outputMode` default never consulted** — the port states it in-tree at
  `frontmatter.rs:730-744`. Same fix session as `SUBA-081`'s `agentOverrides.<n>.outputMode`; land
  the two together.
- **Context-overflow classification** (`contextOverflow` flag + terminal note) — zero hits for
  `context_overflow`; S effort, but the consequence is a less actionable error message.
- **TTL model-exclusion store for fallback candidates** (`modelExclusions`) — M–L, a latency and quota
  optimisation with no correctness consequence.
- **Async retention sweep (30-day) and the active-run / terminal-run / result index layer** — these
  compound (an unswept async root makes the full-directory rescan in `run_status.rs:592-655`
  progressively slower), but both are performance and disk growth, both L.
- **Live context-window usage** (`window` / `windowPeak` across `TokenUsage`, progress, formatters and
  every status surface) — zero hits for `window_peak`; `format_fleet_tokens` is single-argument and
  its tests pin the old string. Observability, M across four surfaces.
- **Prompt Audit drawer; external jobs in FleetView; active task labels; async capacity in the status
  line; fast mode; pruned fork context; `extensionBindings`; structured-output acceptance capture;
  launch-contract preflight; capability audit; `debug.run`** — each confirmed absent, each cut on
  ratio: large-effort UI subsystems, dependents of other unported subsystems (external jobs →
  `SUBA-074`), or low-value diagnostics.

---

## Corpus health

Five things a maintainer should fix in the ledger before the next pass.

**(1) The corpus does not end at `SUBA-066` — it ends at `SUBA-071`.**
`09-cyrup-ext-subagents.md` carries `SUBA-067`…`SUBA-071` (three test-defects filed
`Status FIXED`/`OPEN`, plus `SUBA-070` and a REFUTED `SUBA-071`). A "start at `SUBA-067`" instruction
would have collided with five live ids. **This batch starts at `SUBA-072`.**

**(2) The README baseline table is a full major-version stale for this upstream.** It records
`pi-subagents` latest as **v0.47.1** with a delta of "151 files, +10,254 / −1,333". The tag is
**v0.57.0** and the unmeasured window is the table in `## Scope` above. Area 09's own header also
states that every claim was settled at v0.43.0 or v0.47.1. Update both, and re-read PARITY-GAPS §1d.

**(3) Three high-traffic rows now carry evidence that is factually wrong at HEAD, and one would cause
harm if followed.**
- `SUBA-021` / `VL-S1` says `rg 'capability_ceiling' = 0` and "no ceiling concept". The subsystem
  landed in sweep 10; the residual defect is *worse* than the one filed (`SUBA-072`).
- `VL-S14` rates `runner: external-cli` **medium** / "unsupported". The key is neither rejected nor
  applied, which is a capability widening, and the subsystem tripled and gained a second runner type
  inside the window (`SUBA-074`).
- **`SUBA-051`'s Fix line instructs *"Do not apply it to foreground runs, which already have their
  own default"* — the foreground path has no default at all** (`extension/tool/params.rs:264-280`),
  so following that instruction leaves the foreground unbounded permanently (`SUBA-077`).

This is the third edition's *"a true line number carrying an untrue claim"* class, and it is now the
dominant failure mode in this area's ledger.

**(4) The restructure trap is real and it cuts both ways.** `src/extension.rs` no longer exists, so
every `extension.rs:NNNN` citation in area 09 is **unresolvable**, not merely stale. The more
dangerous direction is the false negative: `restoreActiveJobs` reads as absent under every name
upstream uses and is fully present as `resume_tracking`, with a test pinning both of its subtleties.
Every absence claim in this batch was established by grepping the current tree for the behaviour by
identifier **and** by concept, in both camelCase and snake_case, plus env-var names — never by
resolving a cited path. Adopt that as the standing rule for this area.

**(5) Two in-source comments assert things about upstream that upstream contradicts, and both hid a
defect.**
- `background/watch.rs:605-609` says pi uses `display: true` unconditionally; `notify.ts:239`
  computes it (`SUBA-090` — comment removed and the predicate ported at `79ee7eff`, 2026-09-04).
- `discovery/types.rs:411-414` says `AgentOverrideConfig` is *"a field-for-field port … and pi has no
  others"* while pi had four more at the measured baseline and nine more at v0.57.0 (`SUBA-081`).

**A completeness claim written in a doc comment is not evidence, and neither a citation audit nor a
compile catches it.** Add both to the known-traps list, and prefer a checked-in pinned copy of the
upstream field list plus an assertion over a prose claim.

### One note in the ledger's favour

The lenses independently confirmed large ported subsystems **complete and correct**: the acceptance
tree (~10,140 lines, nine evidence kinds, `stopRules`, verify memoization, workspace fingerprinting —
`SUBA-076` is a defect *inside* it, not a hole in it), nested events (1,992 lines plus the child
control inbox), MCP direct tools (2,816 lines including the header cache-identity fix), the fallback
ladder's R-SA-036 ordering, the turn / tool / usage / spawn budgets, agent memory, model scope, and
the four-tier discovery merge with its deliberately asymmetric same-tier rule.

**The remaining distance in this crate is concentrated in three places**, and a planner should read
the twenty items above through that partition:
1. **The parent side of policy surfaces whose child side is already implemented** — `SUBA-072`
   (capability ceiling), `SUBA-073` (permissions). Both are "the enforcement machinery is ported and
   permanently unreachable", and both are small relative to what they unlock.
2. **The agent-definition schema's missing keys** — `SUBA-074`, `SUBA-081`, `SUBA-082`, `SUBA-088`,
   with `SUBA-086` as the amplifier that converts all of them from silence into user-visible errors.
   **Land `SUBA-086` first.** *(2026-09-04: `SUBA-086` landed at `275c1f85` and `SUBA-082` at
   `5a4ae4ed`, `SUBA-088` at `ba24e5e5`; `SUBA-081`'s remaining fields and `SUBA-074` stage 2 are
   what is left of this partition.)*
3. **The external-runner / `workflowScript` execution model** — `SUBA-074` stage 2, `VL-S2` and its
   dependents. This is the genuinely large remainder and the only part that needs design.
