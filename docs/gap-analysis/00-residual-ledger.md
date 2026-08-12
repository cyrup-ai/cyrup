# 00 — Residual ledger

Ranked, cross-cutting view. The per-area files hold the evidence; this file is for **picking the
next work item**.

> **UPDATE — GitHub Copilot, 2026-08-11, cyrup HEAD `097bdde`.** `PROV-005` (four providers
> unimplemented) is **CLOSED** — `amazon-bedrock`, `openai-codex`, `google-vertex` and
> `github-copilot` are all constructed in `providers/all.rs:177-197`, and `register_builtins`
> (`api/mod.rs:131-163`) now registers 9 factories including the formerly dangling
> `bedrock-converse-stream`. **But the Copilot port that closed it carries three new highs**, filed
> as `PROV-027`/`028`/`029` in `01-cyrup-core-and-provider.md` under `## GitHub Copilot findings`:
>
> * **PROV-027** (parity-bug, S) — Copilot's 9 Claude models send `x-api-key`; pi has an explicit
>   Copilot branch sending `Authorization: Bearer`. cyrup picks the scheme purely from
>   `api_key.contains("sk-ant-oat")`, which a Copilot `tid=…;proxy-ep=…` token never matches.
> * **PROV-028** (not-ported, S) — `pi/packages/ai/src/api/github-copilot-headers.ts` has no
>   counterpart, so `X-Initiator`, `Openai-Intent` and `Copilot-Vision-Request` are absent on **all
>   three** of Copilot's wire routes. Images fail loudly; `X-Initiator` misreports silently.
> * **PROV-029** (parity-bug, S) — the Copilot **and** Codex login flows are fully ported and
>   unreachable: the providers wire the runtime-half strategy (`refresh`/`to_auth` only), so
>   `/login` hits the trait default `LoginUnsupported`, and `register_bundled_oauth_flow_loaders`
>   has zero production callers.
>
> Two things generalise past Copilot. **(1) A "not implemented" item closing means the subsystem now
> exists, not that it is right** — `PROV-005` was closed by code that was never itself audited, and
> three highs were sitting in it. Re-read what a closure *added*, not merely that it added
> something. **(2) `PROV-003`'s "deprioritised, out of scope" status below is stale**: `cf26010`
> landed all 11 OAuth flows under `auth/oauth/`. PROV-029 is what remains of the Copilot/Codex half
> — one field assignment per provider, not a flow to write. The rest of PROV-003 was not re-audited
> here.

> **UPDATE `6104dfa` + `7fd0d9c` (2026-08-07).** `control` and `includeProgress` are ported for real
> and live in the schema — the workaround the maintainer rejected is undone. `SUBA-N04` closed (the
> field was dropped in THREE places, not one). `TOOL-019` closed and revert-proved twice. `PROV-006`
> closed as a process-global **idle** timeout mirroring pi's `configureHttpDispatcher` — deliberately
> not a total-request deadline, which would kill a healthy model streaming a token every 20 s.
> `PROV-008` capped at pi's 4000 chars. All four verification minors closed.
> **3932 passed / 0 failed / 8 ignored across 225 suites.**
>
> Still open from that pass, and stated in both commit messages rather than implied: `SUBA-N03`, the
> schema/dispatch guard test, `StopReason::Pending`, the `notifyChannels: ["async"]` replay hop, and
> the false `subagent-executor.ts:3022` citation still sitting in `extension.rs`.

> **REWRITTEN 2026-08-07 after a code-level audit of every high/critical item at HEAD `8d00f06`.**
> The previous version of this file was baselined at `1806375`, **eight commits behind**, and an
> auditor called it *"the most dangerous file in the directory"* — a planner reading it would have
> scheduled about twelve already-completed items. That is exactly what was happening. Everything
> below was verified by **reading code at HEAD**, not by trusting commit messages.

## The real number: 4 actionable highs, not 17 — **now 7, see the Copilot rows**

38 high/critical items were audited on 2026-08-07. **31 CLOSED, 5 OPEN, 2 PARTIAL.** The
2026-08-11 Copilot pass added three more highs to the bottom of this table; the heading count of 4
is the 2026-08-07 figure and is kept for provenance.

| ID | area | state | why it matters |
|---|---|---|---|
| **SUBA-S01** | 09 | OPEN | **The worst failure mode in the open set.** A declared `outputSchema` never reaches the child *in any form* — `STRUCTURED_OUTPUT_SCHEMA_ENV`/`_CAPTURE_ENV` (`exec/structured.rs:253/:257`) have **zero** non-self-referential references workspace-wide. What runs instead (`extract_structured_output_value`, `:68-103`) takes the last non-error assistant message and returns the first parseable ` ```json ` fence. Usually a confusing hard failure on a child that was never asked; **occasionally a coincidental fence validates and silently becomes the structured result feeding chain output bindings.** Effort M. |
| **SUBA-N03** | 09 | OPEN | Overrides refused on the async/background branch (`extension.rs:4875-4893`) — the same "advertised but rejected" defect `SUBA-041` existed to remove, surviving on a second path. `asyncByDefault`/`forceTopLevelAsync` can route *every* top-level call here. **It is EIGHT params, not seven** — `timeoutMs`/`maxRuntimeMs` is refused just above at `:4862-4868`. Effort L. |
| **EXT-S02** | 06 | OPEN | Extension slash commands never reach the TUI `/` autocomplete. `commands.rs:31` types `SlashCommand::name` as `&'static str`, so a runtime-discovered name is *unrepresentable*. **Severity correction: the commands DO execute** — an unknown `/foo` falls through `dispatch()` to `Dispatch::Prompt`. Effort M; one type change unlocks three features. |
| **TUI-S01** | 07 | PARTIAL | 6 of 9 `UiEffect` mutators wired. `SetHeader`/`SetFooter`/`SetWidget` are stored on `AppState` with **zero read sites**. Silent from the extension author's side: the API accepts the call and returns success. |
| **PROV-027** | 01 | OPEN (2026-08-11) | Copilot's 9 Claude models authenticate with `x-api-key`; pi's explicit Copilot branch (`anthropic-messages.ts:867-888`) sends `Authorization: Bearer`. cyrup has no provider branch — the scheme comes from `api_key.contains("sk-ant-oat")` (`anthropic_messages.rs:435-437`), which a `tid=…;proxy-ep=…` Copilot token never matches. Every request on Copilot's `anthropic-messages` route is unauthenticated. Effort S. |
| **PROV-028** | 01 | OPEN (2026-08-11) | `github-copilot-headers.ts` unported: `X-Initiator`, `Openai-Intent`, `Copilot-Vision-Request` absent on **all three** Copilot wire routes. Image turns are rejected (a loud failure on a normal path); `X-Initiator` misreports every agent-loop request silently. Easy to miss because the *static* editor-identity headers ARE present via `model.headers`. Effort S. |
| **PROV-029** | 01 | OPEN (2026-08-11) | Copilot **and** Codex logins are fully ported and unreachable — the providers wire the runtime-half strategy, so `/login` returns `LoginUnsupported`, and `register_bundled_oauth_flow_loaders` has zero production callers. "Advertised but rejected", the same shape as `SUBA-N03`. Invisible from the flow side: the flows' own tests drive them directly and pass. Effort S. |

**Deliberately out of scope**: `CFG-005` — the OAuth *acquisition* cluster, deprioritised by the
maintainer. `PROV-003` was in this line too and its status is **stale**: `cf26010` landed all 11
OAuth flows, and the Copilot/Codex residue is now `PROV-029` above, which is a wiring fix rather
than the deprioritised work. Also deferred as subsystems: `steer`, the four `schedule*` verbs,
`watchdog/`, FleetView.

## The mediums were audited too, and they are NOT stale — 169 genuinely open

> **Medium audit, 2026-08-07, HEAD `8d00f06`. 178 items read against code. 166 OPEN, 3 PARTIAL,
> 7 CLOSED, 2 WRONG. Closure rate 3.9%** — against **82%** for the highs. **The high result does not
> generalise.**

The mechanism is plain once stated: every commit since the analysis was written was *explicitly*
high-targeted (`513e45a`'s own message: "close the 8 remaining **high-severity** gap items"). **All 7
closures were collateral of a high fix**, never independent work — `SESS-021` rode `SEAM-031`,
`CFG-024`/`CFG-029` rode `CFG-022`, `CFG-033` rode `CFG-011`, and so on. `git log 1806375..HEAD`
returns **zero** commits over `retry.rs`, `compat.rs`, `cyrup-core/src/message.rs`, `env_keys.rs`,
`hooks.rs`, `openai_completions.rs`. Seven whole areas came back **100% open** (06, 07, 09, 10, 11,
08, 02 — 108 items, 0 closed).

**The count went UP, not down: 157 in the Open-items tables + 21 uncounted sweep mediums = 178.**
That is structural defect A firing a second time — this ledger documented the two-table problem and
then enumerated only the first table anyway.

### Promoted medium → high (silent failures outrank loud ones)

| ID | promoted because |
|---|---|
| **`TOOL-019`** | **A regression one of OUR OWN fixes introduced.** Before `cfe351e`'s `TOOL-004` fix both mutators used temp-file + `rename(2)`, which *accidentally* capped an unlocked cross-registry race at "last writer wins, file intact". That fix moved to `write_in_place`, so an unlocked race now **interleaves bytes — silent file corruption**, through the exact seam `TOOL-004`'s own Fix text recommended. `registry.rs` builds `FileMutationLocks::new()` per-`ToolRegistry`; pi's `file-mutation-queue.ts:4` is a **module-scope** Map, one per process. Effort S. |
| **`PROV-006`** | **No request timeout at ANY layer** — `git grep tokio::time::timeout` over `cyrup-agent`/`cyrup-provider` is empty, and reqwest's default is unlimited. A provider that accepts the connection then stalls hangs the turn forever. **And it is a silent no-op on a shipped control**: the TUI picker's "30 sec" is validated, persisted and threaded through four crates to `StreamOptions.timeout_ms`, then dropped. pi installs a global undici dispatcher with 300 s `bodyTimeout`/`headersTimeout` at startup. Effort S-M. |
| **`PROV-010` / `AGENT-014` / `DRIFT-012`** | One defect, three ids. `anthropic_messages.rs` and `google_generative_ai.rs` **default a stop-reason-less stream to `Stop`**, so **a truncated stream is transcribed as a cleanly completed turn** and persisted to JSONL. pi throws. Filed medium as "a cosmetic wire-shape gap". Effort S. |
| **`SUBA-S06`** | `drive_attempt`'s `tokio::select!` has **no `child.wait()` arm**. On the default no-`timeoutMs` path a child that exits without closing inherited stdio hangs the orchestrator's tool call forever — the same class as `8d00f06`'s critical, and the crate's own `exec/acceptance.rs` names this exact hazard and guards it one layer down. Effort S. |
| **`TUI-N02`** | `/reload` silently fails the permission gate **OPEN**. |

### The clusters — 169 items collapse to ~25 moves

The single highest-leverage one:

**C5 — "the extension ABI is lossy; batch every WIT change into ONE bump" — 15 ids.** `EXT-009/011/014/015/006/012/021/023/024/035`, `SEAM-012/025`, `TOOL-015/021/022`. There are exactly two
`world.wit` copies, tied by `tests/wit_world_sync.rs`, and `EXT-028`'s just-landed contract says *any*
export change bumps the minor. **Doing these separately means fifteen minor bumps and fifteen
guest-refusal cliffs.** One `cyrup:ext@0.4.0` closes all fifteen. Host-side prerequisite that must
land in the same move: `cyrup-core/src/tool.rs::prompt_guidelines(&self) -> &[&str]` — a guest cannot
return `&'static str`, so `impl Tool for WasmTool` can never carry guidelines until that returns owned
strings. Effort **L-once vs 15×M**.

**C10 — the subagent second-hop config boundary** — `SUBA-006/007/008/014` *plus* the already-known
`SUBA-N03/N04/N05/N06`. Same one fact: `RunnerConfig` is strictly narrower than `RunOptions`.

**C2 — no HTTP timeout or retry budget** — `PROV-006` + 3 more.

## Open work that appears in NO high row — a table-driven planner will miss all of it

- **`includeProgress` and `control` are UNTRACKED.** Both are still de-advertised and refused
  unconditionally at HEAD (`extension.rs:4815`, pinned by tests at `:8780` and `:9804`). The
  maintainer explicitly **rejected their schema-removal as a workaround**, but they were folded into
  `SUBA-041`'s CLOSED row rather than re-filed. File them.
- **`SUBA-N04` is filed `medium` and should be `high`.** `background/runner_main.rs:1734` drops
  `acceptance: None` — and the same site *also* silently drops `share` (`:1724`), `session_dir`
  (`:1725`), `skills` (`:1730`) and `include_progress` (`:1732`). It is the **silent twin** of
  `SUBA-N03`'s loud refusal, reachable today through the `tasks:[{…}]` surface that `SUBA-041`'s own
  entry recommends as the workaround — so the documented workaround silently loses acceptance.
- **`chainDir` is a third live instance of the `SUBA-041` class**: advertised in the schema,
  deserialized into `SubagentToolParams::chain_dir`, read only by `provided_keys()`, consumed by
  **nothing** on any path, and no test complains.
- **The schema is `additionalProperties: true`** — 30 properties advertised, **32 parsed**. Removing
  `control`/`includeProgress` from the schema never made them unreachable, so the shortcut did not
  even work on its own terms.
- **The `PROV-007` closure residual**: `extension.rs:7385 registry_models()` binds to
  `builtin_catalog()` (pi's credential-blind `getModels()`) where pi's six call sites use
  `getAvailable()`. Recorded only in a `[CYRUP-DELTA]` doc comment.

## Suggested order

**0. The schema/dispatch drift test — highest leverage-per-hour in the backlog.** Nothing today fails
when a property is added to `subagent_tool_parameters` without being wired into `route_single`. That
absence is what let `SUBA-041`'s nine-parameter defect exist, what let it recur as `SUBA-N03`, and
what lets `chainDir` sit advertised-and-dead right now. Effort S.

**1. The subagent child-hop options plumbing — one root cause, four surfaces.** The second-hop
`RunnerConfig` boundary is strictly narrower than the foreground `RunOptions`; everything below is
that single fact surfacing differently. Upstream's shape is readable and known
(`pi-subagents` @v0.34.0 `runs/background/async-execution.ts` carries `artifactsDir`, `shareEnabled`,
`skills`, `outputMode`, `acceptance`, `timeoutMs`, and `executeAsyncSingle` wires each one). **This is
a port, not a design problem.** Order within it: `SUBA-N04` (S, silent) → `SUBA-N03` (L, loud) →
`includeProgress`/`control` (same dispatcher).

**2. `SUBA-S01`** — the silent-wrong structured-output path. Cost the child-side `structured_output`
tool registration before committing; the crate doc scopes it to an outer layer.

**3. TUI chrome slots — one missing concept, three items.** `TUI-S01` residual + `TUI-014` + `TUI-S06`
all reduce to "the TUI has no chrome slot for a host/extension-owned surface". Doing them separately
means touching the same layout code three times. Note `write_terminal_title` exists (`app.rs:4720`)
with exactly one caller, so pi's `updateTerminalTitle()` is still absent.

**4. `EXT-S02`** — widen `SlashCommand` to owned strings; one type change, three features.

**5. The 27 `test-defect` items as one sweep.** More attractive now: the suite is at **zero failures**
for the first time (`1abbd4d` retired the standing "3 failed"), so any new failure is signal.

**6. A second surface-driven sweep.** The first added 58 items and there is no reason to believe one
pass exhausted the class.

**7. The Copilot cluster — three items, one afternoon, and they interlock.** `PROV-027` and
`PROV-028` both edit `anthropic_messages.rs` `build_headers`, and `PROV-028` needs the same
provider guard `PROV-027` introduces, so do them together and take the other two routes in the same
pass. `PROV-029` is independent and is a field assignment per provider. All three are effort S and
all three are the difference between "the Copilot provider is registered" and "the Copilot provider
works".

**8. Audit what the recent closures actually shipped, not that they shipped.** `PROV-005` closed by
adding four providers; one of them arrived with three highs and nobody looked, because the item was
about existence. `amazon-bedrock`, `google-vertex` and `openai-codex` came in the same sweep and
have had no read-against-upstream pass at all — Codex is already implicated in `PROV-029`. Same
question for `cf26010`'s other ten OAuth flows. This is the closure-side twin of structural defect
A: an item-shaped audit cannot see a defect inside the code that closed an item.

## Corrections to carry forward — these are wrong ABOUT THE CODE

1. **`SUBA-N03`'s justification cites a pi precedent that does not exist — and so does cyrup's own
   source comment at `extension.rs:4862`.** Both claim the refusal "mirrors pi's own precedent of
   erroring on `timeoutMs` + async (`subagent-executor.ts:3022`)". Those lines at v0.34.0 are
   intercom-receipt construction, entirely unrelated, and no such refusal exists anywhere upstream.
   Upstream **honours** `timeoutMs` on the async path (`schemas.ts:265-266`, `tool-description.ts:25,:73`,
   `async-execution.ts:850`). **Fix the doc comment regardless of whether `SUBA-N03` is scheduled** —
   it is a false claim about upstream sitting in the provenance record that `CLAUDE.md` says those
   comments exist to guarantee.
2. **`EXT-S01`'s Impact was wrong about pi in a way that could have inverted a security default.** It
   claimed pi "prints one diagnostic line and runs with the extension absent." pi calls
   `process.exit(1)` before mode dispatch, in every mode (`coding-agent/src/main.ts:843-849`).
   Copying it literally would have converted a fail-**closed** abort of the permission gate into a
   fail-**open** session. The implementer got it right despite the item.
3. **`CFG-002`'s prescribed Fix was wrong** and the implementer correctly ignored it. pi throws
   unconditionally (`provider-composer.ts:167-169`); `radius` only special-cases the model baseUrl.
4. **`PROV-026` (low) should be struck outright** — `seed.json`, `seed_catalog()` and
   `seed_catalog_parses` no longer exist, so the test-defect is physically gone.

## Structural defects in this analysis — fix these or they recur

**A. Surface-sweep highs live in a SECOND table and are invisible to first-table enumeration.** Not
hypothetical: it cost `SEAM-S01` an entire audit pass this round. Area 07 reads as "zero highs" from
its Open-items table while `TUI-S01` (high) sits at `07:590`; area 05's "7 highs" misses `CFG-S01` at
`05:551`; area 06 has one high in the main table and two in the sweep. **Every enumeration to date has
undercounted.** Fix: merge the sweep rows into the Open-items table in every file.

**B. The surface-sweep item template carries no verified upstream trace.** `EXT-S01..S06` and
`SEAM-S01..S05` share a boilerplate shape — `**cyrup** — ABSENT.` / blank Impact / *"port the upstream
behaviour named above"*. One was actively misleading (correction 2 above). **Treat any item with that
shape as unverified until re-read.**

**C. An item-driven analysis cannot see behaviour nobody wrote an item for.** The adversarial pass
refutes *claims*; with no claim there is nothing to refute. The surface sweep is the countermeasure —
see the README. Treat the open count as a floor.

**D. Files contradict themselves.** `06` line 36 reads `~~EXT-028~~ **CLOSED** 513e45a | Open |`;
`12` marks `DRIFT-026` closed at line 34 while line 3 still says "the only high remains DRIFT-026";
`10` describes `PERM-001`/`PERM-005` as open in prose while its own table strikes both. Areas 01–05
all still declare themselves baselined at `1806375`.

## Item kinds (unchanged from the last full pass)

| kind | n | note |
|---|---|---|
| `parity-bug` | 114 | ported, then drifted — still the largest bucket |
| `not-ported` | 100 | predates the baseline, never built |
| `upstream-drift` | 49 | landed upstream after 2026-07-10 |
| **`test-defect`** | **27** | see suggested-order 5 |
| `cyrup-original` | 14 | no upstream basis |
| `stale-port` | 13 | carries behaviour upstream changed or deleted |
| other | 6 | tooling, tracking, release-hygiene |

## By area

Highs and mediums have both now been audited against code at HEAD `8d00f06`. **Lows have not** — and
by the medium result (3.9% closure) they are very likely almost all still open, plus an uncounted
tail of sweep `-S` lows.

**The honest total: ~7 actionable highs + 169 open mediums + ~135 unaudited lows.** The port's
remaining work is overwhelmingly *medium* — and, per the kind table, mostly `parity-bug` (ported then
drifted) rather than `not-ported`. That is a different project from "finish building the features".
The high count moved 4 → 7 on 2026-08-11 from a single provider read end to end, which is the
sharpest available evidence for defect C below: the floor is a floor.

| file | nominal open | audited highs |
|---|---|---|
| [01-cyrup-core-and-provider](01-cyrup-core-and-provider.md) | 23 (21 − `PROV-005` closed + 3 Copilot) | 3 open (`PROV-027`/`028`/`029`); `PROV-003` status stale, see the Copilot update |
| [02-cyrup-agent](02-cyrup-agent.md) | 14 | 0 |
| [03-cyrup-session](03-cyrup-session.md) | 24 | 0 |
| [04-cyrup-tools](04-cyrup-tools.md) | 26 | 0 |
| [05-cyrup-config-and-resources](05-cyrup-config-and-resources.md) | 32 | 1 open (`CFG-005`, deprioritised) |
| [06-cyrup-ext](06-cyrup-ext.md) | 30 | 1 open (`EXT-S02`) |
| [07-cyrup-tui](07-cyrup-tui.md) | 33 | 1 partial (`TUI-S01`) |
| [08-cyrup-session-svc-and-modes](08-cyrup-session-svc-and-modes.md) | 18 | 0 |
| [09-cyrup-ext-subagents](09-cyrup-ext-subagents.md) | 37 | 2 open (`SUBA-N03`, `SUBA-S01`) |
| [10-cyrup-permission-system](10-cyrup-permission-system.md) | 20 | 0 |
| [11-cyrup-intercom](11-cyrup-intercom.md) | 22 | 0 |
| [12-upstream-drift-pi-core](12-upstream-drift-pi-core.md) | 32 | 0 |
