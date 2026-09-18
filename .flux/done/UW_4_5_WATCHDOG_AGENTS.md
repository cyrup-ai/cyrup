---
stage: qa
status: completed
updated: 2026-09-18
---

# UW-5 / UW-4 — the two inert watchdog agents

OBJECTIVE: replace the two stub agent implementations that make the entire `watchdog/` subtree
(18 modules, ~18k lines, fully wired) incapable of producing an outcome.

These are one task because they are the same shape, in the same subsystem, bound in adjacent lines
of the same file — and because fixing one without the other leaves the subsystem still unable to
report.

## The two stubs

### UW-5 — `NoDecisionPermissionAgent` (schedule this FIRST; it is a live production defect)

- `crates/cyrup-ext-subagents/src/watchdog/permission_arbiter.rs:600` — `pub struct NoDecisionPermissionAgent`,
  `impl WatchdogPermissionAgent` at `:603`, returning `Ok(None)`.
- Trait: `WatchdogPermissionAgent` at `permission_arbiter.rs:583`.
- **Bound in production** at `crates/cyrup-ext-subagents/src/prompt_runtime.rs:2407`
  (inside `.with_permission_gate(…)`) and `:2858`.
- Consequence, documented in-tree at `prompt_runtime.rs:2775`: an `ask`-tier tool is denied as
  `malformed` — *"Watchdog permission arbiter returned no decision."* (`permission_arbiter.rs:734`,
  `:747`).
- **Blast radius GREW when UW-6 closed.** A real, fully-merged permission policy now reaches the
  child (`exec/spawn_plan.rs:1178`, `:1202`; carried on `RunnerConfig` at
  `extension/executor/background.rs:625`), so an `ask`-tier rule can actually be hit in production
  where previously the gate received no policy at all. **Every ask-tier tool inside a subagent
  denies right now.** A delegated agent is silently capped.
- Upstream: `src/watchdog/permission-arbiter.ts` (156 lines @v0.68.0) —
  `createWatchdogPermissionArbiter` at `:41`, constructing `new Agent({… streamFunction })` at
  `:102`, exported as `requestWatchdogPermission` at `:145`.

### UW-4 — `NoTurnReviewAgent`

- `crates/cyrup-ext-subagents/src/watchdog/review.rs:876` — `pub struct NoTurnReviewAgent`,
  `impl WatchdogReviewAgent` at `:879`, whose `run` returns `Ok(Vec::new())` at `:880`.
- Trait: `WatchdogReviewAgent` at `review.rs:631`, `async fn run(&self, turn: WatchdogReviewTurn<'_>)`
  at `:638`. The trait doc at `:617-628` already states the **three contracts an implementation MUST
  honour** and they are code on `WatchdogReviewTurn`, not prose: expose `read_only_tools` +
  `warn_tool` as the tool list and route every `watchdog_warn` through `WatchdogWarnTool::execute`;
  refuse any tool `WatchdogReviewTurn::block_reason` names **at execution time**, even one the
  harness supplied from outside the tool list; abort when `cancel` fires. Honour all three.
- **Bound in BOTH production paths** — `watchdog/register_main.rs:168` and `prompt_runtime.rs:2438`.
- Consequence: a review model is resolved on every agent-end boundary, but **no warning can ever be
  emitted**. `/subagents-watchdog status` reports *"real model review"* (`register_main.rs:191`) over
  a machine that cannot produce a finding. That status string is itself a defect to fix or make true.
- Upstream: `src/watchdog/review.ts` (363 lines @v0.68.0) —
  `await agent.prompt(buildReviewPrompt(request, selection))` at `:295`, inside
  `createMainWatchdogReview` (`:249`). Prompt builders at `:207-220`
  (`buildWatchdogSystemPrompt`) and `:222-231` (`buildReviewPrompt`). Tool policy at `:271-274`
  (read-only list), `:285-287` (`beforeToolCall` block), `:288` (`toolExecution: "sequential"`),
  `:290-292` (cancel).

## The seam exists — this is not blocked

`cyrup-agent` and `cyrup-provider` are **already direct dependencies** of `cyrup-ext-subagents`
(see its `Cargo.toml`). The direct analog of upstream's `new Agent({… streamFunction })` +
`await agent.prompt(…)` is:

- `cyrup_agent::Agent::builder(model: ModelRef, stream_fn: Arc<dyn StreamFn>)` —
  `crates/cyrup-agent/src/agent/facade.rs:48`
- `Agent::prompt(&self, input: impl Into<PromptInput>) -> Result<RunHandle, AgentError>` —
  `crates/cyrup-agent/src/agent/lifecycle.rs:142`

Model resolution for the review side already exists:
`watchdog/model_selection.rs` (`BuiltinWatchdogModelRegistry`, `WatchdogReviewModelSelection`
carries the resolved model **and its credential overlay**), reached through
`MainWatchdogReview::new(registry, AmbientReviewAuth, <agent>, cwd)`.

## Definition of done

Both stubs are **replaced** by implementations that run a real model turn, bound at every production
site (`prompt_runtime.rs:2407`, `:2858`, `:2438`; `watchdog/register_main.rs:168`). Tests drive the
production binding — not a `FixedAgent` double in isolation (`permission_arbiter.rs:807` already has
those; they prove the arbiter, not the agent).

Concretely provable outcomes:

1. An `ask`-tier tool inside a subagent can be **approved** by the arbiter and can run.
2. The arbiter still **fails closed** when no decision can be reached (provider error, cancel,
   malformed model output). A child has no human to ask — that behaviour is correct and must survive.
3. A watchdog review can emit an actual `watchdog_warn` finding through the production path.
4. `/subagents-watchdog status` either reports "real model review" truthfully, or reports what is
   actually bound.

**No `allow(dead_code)`. No new stub. No disclosure in place of completion.** `PARITY-GAPS.md` names
UW-4/UW-5 as *"the canonical example of 'closing a not-implemented item means the subsystem exists,
not that it is correct.'"* Close it correctly.

## Rules

- Upstream reads pinned at `v0.68.0` via `git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`.
  Never a working tree, never unpinned HEAD.
- Any deliberate divergence gets a `[CYRUP-DELTA]` comment saying what and **why**.
- Workspace stays `cargo fmt` clean and clippy clean under every README gate.
- Baseline before this work: **10414/10414 passing.** Do not regress it.

---

# [AUG] Augmentation pass — 2026-09-18

Research only. Every anchor below was re-verified in the tree at `/home/user/cyrup` and every
upstream read is `git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`
(`v0.68.0` = `f3ccf47dc236b6c0fcc0d897cec4a9e6da3e916d`).

## [AUG] 0. Verdict up front

**`alreadyImplemented = false`, confirmed by exhaustive grep.**

* `grep -rn "impl WatchdogReviewAgent"` over the workspace returns FOUR hits: `review.rs:879`
  (`NoTurnReviewAgent`) and three `#[cfg(test)]` doubles (`review.rs:1295` `Capturing`,
  `review.rs:1516` `Capture`). No production implementation exists.
* `grep -rn "impl WatchdogPermissionAgent"` returns FOUR hits: `permission_arbiter.rs:603`
  (`NoDecisionPermissionAgent`) and three `#[cfg(test)]` doubles (`:810` `FixedAgent`, `:822`
  `FailingAgent`, `:834` `HangingAgent`).
* `grep -rn "cyrup_agent::Agent\b\|cyrup_agent::AgentBuilder\|ProviderStreamFn"` over
  `crates/cyrup-ext-subagents/src` returns **zero** construction sites. Every `cyrup_agent::*`
  mention in the crate is a TYPE (`AgentMessage`, `AgentEvent`), exactly as
  `cyrup-ext-subagents/Cargo.toml:60-64` claims. **This task will be the first production
  construction of a `cyrup_agent::Agent` outside `cyrup-session-svc` in the whole workspace.**
  That is a real fact the executor should internalise: there is no in-crate precedent to copy, and
  §4 below names the one that has to be created.

## [AUG] 1. Seed anchors — verification table

### 1a. cyrup anchors: CONFIRMED

| seed claim | verdict |
|---|---|
| `permission_arbiter.rs:600` `pub struct NoDecisionPermissionAgent` | **CONFIRMED** (`:599` derive, `:600` struct) |
| `permission_arbiter.rs:603` `impl WatchdogPermissionAgent` | **CONFIRMED** (`:602` `#[async_trait]`, `:603` impl, `:604` `decide`, `:608` `Ok(None)`) |
| `permission_arbiter.rs:583` trait `WatchdogPermissionAgent` | **CONFIRMED** (`:582` `#[async_trait]`, `:589` `async fn decide`) |
| `prompt_runtime.rs:2407` binds `NoDecisionPermissionAgent` | **CONFIRMED** — inside `.with_permission_gate(…)` at `:2399-2408`, in `prompt_runtime_from_env` |
| `prompt_runtime.rs:2775` documents the ask-tier consequence | **CONFIRMED** — a doc comment on the test `an_ask_tier_tool_fails_closed_and_writes_both_audit_records` (`:2779`) |
| `review.rs:876` `pub struct NoTurnReviewAgent` | **CONFIRMED** |
| `review.rs:879` `impl WatchdogReviewAgent` | **CONFIRMED** |
| `review.rs:631` trait `WatchdogReviewAgent`, `:638` `async fn run` | **CONFIRMED** |
| `register_main.rs:168` binds `NoTurnReviewAgent` | **CONFIRMED** — inside the `options.review.unwrap_or_else` production arm (`:156-179`) |
| `prompt_runtime.rs:2438` binds `NoTurnReviewAgent` | **CONFIRMED** — inside `child_watchdog_review` (`:2423-2460`) |
| `register_main.rs:191` `"real model review"` | **CONFIRMED** (`:189-196`), rendered by `:526` `format!("Review model call: {…}")` |
| `exec/spawn_plan.rs:1178`, `:1202` ship `PERMISSION_POLICY_ENV` | **CONFIRMED** verbatim |
| `extension/executor/background.rs:625` `RunnerConfig.permission_rules` | **CONFIRMED** (`:625-627`) |
| `cyrup-agent/src/agent/facade.rs:48` `Agent::builder(ModelRef, Arc<dyn StreamFn>)` | **CONFIRMED** |
| `cyrup-agent/src/agent/lifecycle.rs:142` `Agent::prompt(...) -> Result<RunHandle, AgentError>` | **CONFIRMED** |

### 1b. cyrup anchors: STALE / WRONG — corrected

1. **`prompt_runtime.rs:2858` is NOT a production binding.** The seed (and `PARITY-GAPS.md:1594`)
   says `NoDecisionPermissionAgent` is "bound in production at … `:2858`". Line 2858 sits inside
   `#[cfg(test)] mod permission_gate_tests`, which opens at `prompt_runtime.rs:2710-2711` and closes
   at `:2866`; the binding is the assertion body of
   `a_child_with_no_policy_installs_no_gate` (`:2848-2865`). **There is exactly ONE production
   binding of the arbiter: `prompt_runtime.rs:2407`.** The executor must not "wire" `:2858`; it is a
   test that asserts an unarmed child installs no gate and must keep passing unchanged.
2. **`permission_arbiter.rs:734` is not the no-decision message.** `:734` is
   `let approved = decision.decision == "approve";` inside the `Ok(Some(decision))` arm. The
   `Ok(None)` → `malformed` arm is `:743-749`; the sentence *"Watchdog permission arbiter returned no
   decision."* is `:747`. Cite `:743-749`.
3. **`review.rs:880` is the signature, not the return.** `:880` is `async fn run(&self, _turn: …)`;
   `Ok(Vec::new())` is `:881`.
4. **Trait doc span.** Seed says `review.rs:617-628`; the doc block is `:616-629`.
5. **Two in-tree comments carry stale line numbers and should be repaired by this task** (they name
   the very sites being changed, so leaving them wrong is a fresh defect):
   * `review.rs:899` — *"bound in BOTH production paths (`register_main.rs:168`,
     `prompt_runtime.rs:1761`)"*. `prompt_runtime.rs:1761` is a `PermissionGate { … }` struct literal;
     the real site is `:2438`.
   * `review.rs:761` — *"both cyrup production paths bind that instead (`register_main.rs:176`,
     `prompt_runtime.rs:1764`)"*. The child's `with_session_context` is `prompt_runtime.rs:2441`.
     (`register_main.rs:176` is correct — the closure body; the call is `:175`.)

### 1c. Upstream anchors: ALL STALE — they are v0.43.0 numbers, not v0.68.0

The seed's upstream citations reproduce `PARITY-GAPS.md`, which reproduces the two cyrup module
headers, which both declare themselves ports of **v0.43.0** (`permission_arbiter.rs:1-2` "145 lines
@v0.43.0"; `review.rs:1-2` "302 lines @v0.43.0"). **Every one of them is wrong at v0.68.0.**

`permission-arbiter.ts` — 156 lines @v0.68.0 (145 @v0.43.0):

| seed says | v0.68.0 truth |
|---|---|
| `createWatchdogPermissionArbiter` at `:41` | **`:43`** |
| `new Agent({… streamFunction })` at `:102` | **`:111`** (the `streamFn` wrapper is `:105-110`) |
| `requestWatchdogPermission` at `:145` | **`:156`** |
| module-doc `:37-40` `conciseReason` | **`:38-41`** |
| module-doc `:42-145` the returned function | **`:44-153`** |
| module-doc `:126-155` the turn | **`:111-132`** |
| module-doc `:130-135` system prompt | **`:113-118`** |
| module-doc `:151` user prompt | **`:129`** |
| module-doc `:143` `beforeToolCall` | **`:126`** |
| module-doc `:147-153` the `Promise.race` | **`:134-142`** |
| module-doc `:44,52-61` the audit pair | **`:48`, `:54-63`** |
| module-doc `:66-72,127-137` fail-closed table | **`:72-75`, `:143-145`** |
| module-doc `:73` pre-turn cancel | **`:75`** |

`review.ts` — 363 lines @v0.68.0 (302 @v0.43.0):

| seed says | v0.68.0 truth |
|---|---|
| `agent.prompt(buildReviewPrompt(...))` at `:295` | **`:350`** |
| `createMainWatchdogReview` at `:249` | **`:253`** (the turn body moved to `runWatchdogAttempt`, `:268-363`) |
| `buildWatchdogSystemPrompt` `:207-220` | **`:221-236`** |
| `buildReviewPrompt` `:222-231` | **`:238-247`** |
| read-only tool filter `:271-274` | **`:304-308`** (the filter itself is `:305`) |
| `beforeToolCall` block `:285-287` | **`:338-340`** |
| `toolExecution: "sequential"` `:288` | **`:341`** |
| cancel listeners `:290-292` | **`:345-347`** |
| `createWatchdogWarnTool` `:180-205` | **`:194-219`** |
| `finalStopReason` fold `:233-243` | **`:356-358`** |
| `WATCHDOG_ALLOWED_TOOL_NAMES` `:20` | **`:24`** |

**Decision the executor must make and record.** The two cyrup modules are internally consistent
v0.43.0 ports with ~60 v0.43.0 citations each. Renumbering all of them is mechanical but large and
would collide with §7's file list. **Recommendation:** leave the existing v0.43.0 citations alone
(they are correct *at the tag they name*), and have every NEW line this task adds cite **v0.68.0**
explicitly in the form `review.ts:350 @v0.68.0`, with one `[CYRUP-DELTA]`-style note at the top of
each new block saying which tag the surrounding file is pinned to and which tag the new code is
pinned to. Do NOT silently mix.

## [AUG] 2. Upstream at v0.68.0, read in full, load-bearing blocks quoted

### 2a. `src/watchdog/permission-arbiter.ts` — the whole turn (`:77-142`)

```ts
 77	let decision: PermissionDecisionParams | undefined;
 78	const tool: AgentTool<typeof PermissionDecisionParams, { recorded: boolean }> = {
 79		name: "watchdog_permission_decision",
 80		label: "Watchdog permission decision",
 81		description: "Approve or deny this exact child tool call. Call exactly once.",
 82		parameters: PermissionDecisionParams,
 83		executionMode: "sequential",
 84		async execute(_toolCallId, params) {
 85			if (!decision) decision = params;            // FIRST call wins; later calls are ignored
 86			return { content: [{ type: "text", text: "Permission decision recorded." }], details: { recorded: true } };
 87		},
 88	};
...
 94	const run = async (): Promise<WatchdogPermissionResult> => {
 95		const config = childResolvedConfig(childConfig);
 96		const selection = await resolveWatchdogReviewModel(request.ctx, config);
 97		const auth = selection.auth;
 98		const sessionId = request.ctx.sessionManager.getSessionId();
 99		const registeredProvider = (request.ctx.modelRegistry as {
100			getRegisteredProviderConfig?: (provider: string) => { api?: string; streamSimple?: StreamFn } | undefined;
101		}).getRegisteredProviderConfig?.(selection.model.provider);
102		const baseStreamFn = options.streamFn ?? (registeredProvider?.streamSimple && registeredProvider.api === selection.model.api
103			? registeredProvider.streamSimple
104			: streamSimple);
105		const streamFn: StreamFn = (model, context, streamOptions) => baseStreamFn(model, context, {
106			...streamOptions,
107			...(auth.apiKey ? { apiKey: auth.apiKey } : {}),
108			env: auth.env || streamOptions?.env ? { ...(auth.env ?? {}), ...(streamOptions?.env ?? {}) } : undefined,
109			headers: { ...opencodeSessionHeaders(model, sessionId), ...(streamOptions?.headers ?? {}), ...(auth.headers ?? {}) },
110		});
111		agent = new Agent({
112			initialState: {
113				systemPrompt: [ …four lines… ].join("\n"),
119				model: selection.model,
120				thinkingLevel: selection.thinkingLevel,
121				tools: [tool],
122			},
123			convertToLlm,
124			...agentStreamOptions(streamFn),
125			getApiKey: (providerName) => providerName === selection.model.provider ? auth.apiKey : undefined,
126			beforeToolCall: async ({ toolCall }) => toolCall.name === tool.name ? undefined : { block: true, reason: `Permission arbiter tool '${toolCall.name}' is not allowed.` },
127			toolExecution: "sequential",
128		});
129		await agent.prompt(`Tool: ${request.toolName}\nRedacted arguments: ${preview}`);
130		if (!decision) return finish(false, "Watchdog permission arbiter returned no decision.", "malformed");
131		const approved = decision.decision === "approve";
132		return finish(approved, decision.reason, decision.decision);
133	};
134	return await Promise.race([
135		run(),
136		new Promise<...>((resolve) => { timeout = setTimeout(() => { agent?.abort(); resolve(finish(false, "Watchdog permission decision timed out.", "timeout")); }, childConfig.agentEndTimeoutMs); }),
137		new Promise<...>((resolve) => {
138			abort = () => { agent?.abort(); resolve(finish(false, "Watchdog permission decision was cancelled.", "cancelled")); };
139			request.signal?.addEventListener("abort", abort, { once: true });
140			request.ctx.signal?.addEventListener("abort", abort, { once: true });
141		}),
142	]);
```

Eight facts this block carries that the cyrup seam does NOT currently express:

1. **`:85` first-call-wins.** Two `watchdog_permission_decision` calls in one turn: the FIRST is
   recorded, later ones are ignored but still answered `"Permission decision recorded."`. cyrup's
   `WatchdogPermissionTurn` has no tool object at all — only `decision_tool_schema`
   (`permission_arbiter.rs:572`) — so the name (`:79`), label (`:80`), description (`:81`),
   `executionMode` (`:83`) and the first-wins latch are all unexpressed. Compare
   `WatchdogWarnTool` (`review.rs:450-507`), which exists precisely because the review seam had the
   same hole and it was closed there. **The arbiter seam needs the same treatment:** a
   `WatchdogPermissionDecisionTool` carrying `name`/`label`/`description`/`parameters`/`SEQUENTIAL`
   and a first-wins `record(params)`.
2. **`:95-96` the arbiter resolves its OWN model**, via `childResolvedConfig` →
   `resolveWatchdogReviewModel(ctx, config)`. cyrup's turn hands the agent
   `config: &ChildWatchdogConfig` (`permission_arbiter.rs:566`) and nothing else — so the bound
   agent must itself call `register_child::child_resolved_config` (`register_child.rs:65`) then
   `review::resolve_watchdog_review_model` (`review.rs:270`). Both are already `pub`.
3. **`:96` reads `request.ctx`** — i.e. `ctx.model`, the LIVE session model. **cyrup's
   `WatchdogPermissionTurn` carries no session context at all.** See §3c: this is a genuine gap and
   the single most likely cause of a "wired but always denies" outcome.
4. **`:98-104` prefers the SESSION's registered provider** over the generic `streamSimple`, gated on
   `registeredProvider.api === selection.model.api`. cyrup has no analogue of
   `getRegisteredProviderConfig`. See §4b — this is the seam that has to be built, and it is also
   what makes the reachability test possible offline.
5. **`:107-109` the credential overlay** is applied per stream call: `apiKey`, merged `env`, merged
   `headers` with `auth.headers` LAST (it wins). cyrup's `WatchdogReviewAuth`
   (`review.rs:83-91`) has exactly these three fields already.
6. **`:125` `getApiKey`** answers the key only for the selected model's own provider — a
   cross-provider request gets `undefined`. Maps onto `AgentBuilder::key_resolver`
   (`cyrup-agent/src/agent/builder.rs:97`).
7. **`:127` + `:83` two distinct sequential settings** — agent-wide and per-tool, exactly the pair
   `review.rs:660-663` already documents for the review.
8. **`:134-142` upstream drift vs v0.43.0 — THREE behaviour changes cyrup does not have.**
   `git diff v0.43.0 v0.68.0 -- src/watchdog/permission-arbiter.ts` shows the race was restructured:
   * **timeout** now RESOLVES `finish(false, "Watchdog permission decision timed out.", "timeout")`
     (`:136`). v0.43.0 REJECTED, so the message reached the catch and came out prefixed
     `"Watchdog permission arbiter failed closed: …"`. cyrup reproduces v0.43.0
     (`permission_arbiter.rs:727` + `:764`).
   * **mid-turn cancel** now resolves `decision: "cancelled"` (`:138`). v0.43.0 only called
     `agent.abort()`, so it surfaced as `decision: "error"` with the `failed closed:` prefix. cyrup
     reproduces v0.43.0 and *documents that choice in prose* at `permission_arbiter.rs:751-754`.
   * **`completed` idempotence latch** (`:49`, `:52-53`) — only ONE `permission.decision` audit
     record is appended even when a losing race arm also calls `finish`. cyrup's `finish`
     (`:631-654`) has no latch; today it cannot double-fire because its `tokio::select!`
     (`:717-730`) has exactly one winner, but the executor must not introduce a second `finish`
     caller without adding the latch.

   **Every one of these three still fails closed** (`approved: false` on all arms), so adopting them
   is safe for the security property and is the honest read of the pinned tag. **Recommendation:**
   adopt all three, update the module doc's fail-closed table (`permission_arbiter.rs:12-21`) and
   the two tests that pin the old strings (see §6d), and mark the `:751-754` prose as superseded.
   If the executor declines, it MUST leave a `[CYRUP-DELTA]` saying the file stays pinned at
   v0.43.0 for the race semantics and why — silence here would re-create exactly the
   "documented-wrong" defect §1b.5 flags.

### 2b. `src/watchdog/review.ts` — the whole turn (`:294-358`)

```ts
294	const diffBaseline = options.diffBaseline?.();
295	let clarification: { question: string; evidence: string } | undefined;
296	let warned = false;
297	let toolCount = 0;
298	const warnRequest = request.allowClarification ? { ...request, emitWarning: (warning) => { … } } : request;
304	const tools = [
305		...(options.createReadOnlyTools ?? createReadOnlyTools)(ctx.cwd).filter((tool) => WATCHDOG_ALLOWED_TOOL_NAMES.has(tool.name) && tool.name !== "watchdog_warn"),
306		createWatchdogWarnTool(warnRequest),
307		...(diffBaseline ? [createWatchdogDiffTool(diffBaseline)] : []),
308	];
...
324	const agent = new Agent({
325		initialState: {
326			systemPrompt: buildWatchdogSystemPrompt(ctx, { hasScope, guidance, hasDiff }),
331			model: selection.model,
332			thinkingLevel: selection.thinkingLevel,
333			tools,
334		},
335		convertToLlm,
336		...agentStreamOptions(streamFn),
337		getApiKey: (providerName) => providerName === selection.model.provider ? auth.apiKey : undefined,
338		beforeToolCall: async ({ toolCall }) => !clarification && (WATCHDOG_ALLOWED_TOOL_NAMES.has(toolCall.name) || (request.allowClarification && toolCall.name === "watchdog_ask"))
339			? undefined
340			: { block: true, reason: `Watchdog reviews are read-only; tool '${toolCall.name}' is not allowed.` },
341		toolExecution: "sequential",
342	});
343	// Include rejected/invalid calls as well as read-only work and findings.
344	agent.subscribe((event) => { if (event.type === "tool_execution_start") toolCount++; });
345	const abort = () => agent.abort();
346	ctx.signal?.addEventListener("abort", abort, { once: true });
347	request.signal?.addEventListener("abort", abort, { once: true });
348	try {
349		if (ctx.signal?.aborted || request.signal?.aborted) return { result: { stopReason: "aborted" } };
350		await agent.prompt(buildReviewPrompt(request, selection));
351	} finally {
352		ctx.signal?.removeEventListener("abort", abort);
353		request.signal?.removeEventListener("abort", abort);
354	}
355	if (ctx.signal?.aborted || request.signal?.aborted) return { result: { stopReason: "aborted" } };
356	const terminal = agent.state.messages.findLast((message) => message.role === "assistant");
357	const reason = terminal && "stopReason" in terminal ? terminal.stopReason : undefined;
358	const stopReason = reason === "error" || reason === "aborted" || reason === "length" ? reason : "stop";
```

**What is IN SCOPE for this task from this block** (`:304-308` minus the diff tool, `:324-342`,
`:345-354`, `:356-358`): the tool list, the two-layer tool policy, the two sequential flags, the
`getApiKey` narrowing, the cancel wiring around `prompt`, the abort re-check after it, and the
terminal-assistant fold.

**What is OUT of scope and must NOT be smuggled in** — these are separate v0.68.0 features that
cyrup's v0.43.0-pinned port does not have at all, and the executor should file/flag rather than
build them:
* `watchdog_diff` (`:10`, `:294`, `:307`, `:329`) — a whole `diff-tool.ts` module, unported.
* `watchdog_ask` / clarification yield (`:26`, `:295`, `:298-303`, `:309-323`, `:338`, `:361`).
* `loadWatchdogGuidance` / `WATCHDOG.md` (`:11`, `:234`, `:328`).
* `importance` on `WatchdogWarnParams` (`:30`) — cyrup's `WatchdogWarning` (`types.rs`) has no
  `importance` field.
* `errorMessage` on the result (`:359`, `:361`) — cyrup's `WatchdogReviewResult` has
  `warnings` + `stop_reason` only.
* `toolCount` (`:297`, `:344`).
* Prompt-text drift: `:230` dropped "medium/high confidence" and `:231` says "unsupported guesses"
  where cyrup's `build_watchdog_system_prompt` (`review.rs:543,547`) still has the v0.43.0 wording;
  `:227`'s scope sentence was rewritten entirely vs cyrup `review.rs:530-533`.

Adopting any of those is a DIFFERENT task. Building the model turn does not require them, and the
turn is what UW-4 names. **Flag them in the executor's PR body so they get ids.**

### 2c. `src/shared/agent-stream-options.ts` (5 lines) and `opencode-session-headers.ts` (30 lines)

`agentStreamOptions(streamFn)` returns `{ streamFunction, streamFn }` — a pure
belt-and-braces spread for two spellings of the same option. **No cyrup counterpart is needed**:
`AgentBuilder::new(stream_fn)` takes exactly one. Note it as `[CYRUP-DELTA]`, do not port it.

`opencodeSessionHeaders` matters and has a doc comment that is directly about this task:

```ts
13	/**
14	 * OpenCode session-routing headers for internal subagent model calls.
15	 *
16	 * Pi's own session path emits these from coding-agent's provider-attribution
17	 * merge, but subagent-internal calls (watchdog review, permission arbiter,
18	 * task-mutation arbiter, prompt audit) stream through bare Agents that bypass
19	 * that path. Without them OpenCode falls back to client-IP affinity and loses
20	 * prompt-cache routing (see pi issue #4847).
23	export function opencodeSessionHeaders(model, sessionId): ProviderHeaders | undefined {
27		if (!sessionId) return undefined;
28		if (model.provider !== "opencode" && model.provider !== "opencode-go" && !matchesOpenCodeHost(model.baseUrl)) return undefined;
29		return { "x-opencode-session": sessionId, "x-opencode-client": "pi" };
30	}
```

Upstream states the design in `:17-18`: **these calls are meant to be bare Agents.** That is the
single best justification available for the shape §4 proposes.

cyrup HAS this logic, at `crates/cyrup-session-svc/src/attribution.rs:26` (`OPENCODE_HOST`),
`:109` (the provider/host predicate) and `:114` (the two headers) — but `cyrup-session-svc` is a
**dev-dependency only** of `cyrup-ext-subagents`
(`cyrup-ext-subagents/Cargo.toml`, `[dev-dependencies]`), so it is unreachable from production code
here. Three options, in preference order:
1. **(recommended)** a ~15-line private port in the new turn module, `[CYRUP-DELTA]`-noting that
   the identical logic exists at `attribution.rs:100-118` and cannot be depended on from this
   crate's production graph. `HostServices::session_id()` (`cyrup-ext/src/host/services.rs:~425`)
   supplies the id.
2. lift the helper into `cyrup-provider` and have both call it — cleaner, but touches a crate this
   task otherwise does not, and collides with §7.
3. omit it — **do not**: the headers are the prompt-cache routing for opencode users, and omitting
   them silently degrades every watchdog call on that provider.

## [AUG] 3. Upstream behaviour → cyrup seam, exhaustively

### 3a. Seams that ALREADY EXIST (do not rebuild)

| upstream (v0.68.0) | cyrup seam, verified |
|---|---|
| `permission-arbiter.ts:45-48` audit request record | `permission_arbiter.rs:665-674` |
| `:50-65` `finish` + decision record | `permission_arbiter.rs:631-654` |
| `:69-75` config decode / disabled / pre-cancel | `permission_arbiter.rs:676-706` |
| `:95` `childResolvedConfig` | `register_child.rs:65` `child_resolved_config` (`pub`) |
| `:96` `resolveWatchdogReviewModel` | `review.rs:270` `resolve_watchdog_review_model` (`pub`) |
| `:113-118` system prompt | `permission_arbiter.rs:547-555` `permission_arbiter_system_prompt` |
| `:129` user prompt | `permission_arbiter.rs:559-561` `permission_arbiter_prompt` |
| `:12-15` params schema | `permission_arbiter.rs:533-543` `permission_decision_parameters_schema` |
| `:130-132`, `:143-145` fail-closed mapping | `permission_arbiter.rs:732-768` |
| `review.ts:221-236` system prompt | `review.rs:520-556` `build_watchdog_system_prompt` |
| `:238-247` review prompt | `review.rs:561-581` `build_review_prompt` |
| `:194-219` `createWatchdogWarnTool` | `review.rs:450-507` `WatchdogWarnTool` (name/label/description/parameters/`SEQUENTIAL`/`execute`) |
| `:305` read-only EXPOSE list | `review.rs:80` `WATCHDOG_REVIEW_READ_ONLY_TOOL_NAMES` |
| `:24` PERMIT list | `review.rs:61` `WATCHDOG_ALLOWED_TOOL_NAMES` |
| `:338-340` `beforeToolCall` | `review.rs:585-592` `watchdog_tool_call_block_reason`, and `WatchdogReviewTurn::block_reason` at `:681` |
| `:341` agent-wide sequential | `WatchdogReviewTurn::tool_execution_sequential` (`review.rs:663`) |
| `:356-358` terminal fold | `review.rs:598-614` `final_stop_reason` |
| `:137-174` model + thinking resolution | `review.rs:270-310` (+ `model_selection.rs`) |
| `createReadOnlyTools(ctx.cwd)` | **`cyrup_tools::read_only_tools(cwd, Backend, ToolsOptions)`**, `crates/cyrup-tools/src/registry.rs:195-202` — filters the eight builtins to exactly `{read,grep,find,ls}`, which is `createReadOnlyToolDefinitions` (see `registry.rs:74-79`) |
| `new Agent({...})` | `cyrup_agent::Agent::builder(ModelRef, Arc<dyn StreamFn>)` → `AgentBuilder` (`builder.rs:32-295`): `.system_prompt` `:58`, `.model` `:67`, `.thinking_level` `:73`, `.tools` `:79`, `.hooks` `:91`, `.key_resolver` `:97`, `.tool_execution` `:115`, `.session_id` `:121`, `.headers` `:151`, `.api_key` `:189`, `.provider_env` `:198` |
| `beforeToolCall` (agent-level) | `cyrup_agent::Hooks::before_tool_call` → `BeforeOutcome::Block { reason, terminate }` (`cyrup-agent/src/hooks.rs:27-72`) |
| `toolExecution: "sequential"` | `cyrup_agent::queue::ToolExecution::Sequential` (`queue.rs:34-38`) |
| `await agent.prompt(...)` | `Agent::prompt` (`lifecycle.rs:142`) → `RunHandle::finished() -> Vec<AgentMessage>` (`lifecycle.rs:26-28`) |
| `agent.abort()` | `Agent::abort` (`facade.rs:210`) |
| `agent.state.messages` fold | `serde_json::to_value(&AgentMessage)` emits `{"role":"assistant",…,"stopReason":"…"}` — `AssistantMessage`'s hand-written serializer writes `role` at `cyrup-core/src/message/assistant.rs:150` and `stopReason` at `:168`; `StopReason` is `rename_all="camelCase"` (`stop_reason.rs:74`). **So `WatchdogReviewAgent::run`'s existing `Result<Vec<Value>, String>` return needs NO trait change**: map the run's messages through `to_value` and `final_stop_reason` reads them verbatim |
| provider construction from credentials | `cyrup_provider::providers::all::all_providers_with(store: Arc<dyn CredentialStore>, registry: Arc<ApiRegistry>) -> Vec<Arc<dyn Provider>>` (`providers/all.rs:213-218`), `cyrup_provider::api::builtin_registry()` (`api/mod.rs:169`), and **`cyrup_config::AuthStore` implements `cyrup_provider::CredentialStore`** (`cyrup-config/src/auth.rs:464`). `watchdog_config_dirs()` (`register_main.rs:90`) already resolves the process's `ConfigDirs` on both production paths |
| provider → `StreamFn` | `cyrup_agent::ProviderStreamFn::new(Arc<dyn Provider>)` (`cyrup-agent/src/stream_fn.rs:28-38`) |
| the warn tool as an agent tool | `cyrup_core::Tool` (`cyrup-core/src/tool.rs:184-335`): `name`/`parameters`/`execution_mode`/`description`/`label`/`execute` — one impl wrapping `WatchdogWarnTool::execute` |

### 3b. Dependency reality check

`cyrup-ext-subagents/Cargo.toml` production deps already include `cyrup-agent`, `cyrup-provider`,
`cyrup-config`, `cyrup-core`, `cyrup-ext`. **`cyrup-tools` is NOT** — it is a dev-dependency, and
its entry carries a deliberate comment: *"production code in this crate never names `cyrup_tools`
… An alias would silently widen a REFUSAL vocabulary to any name a future registry adds"*.

That comment is about `HOST_BUILTIN_TOOL_NAMES`, a **name list**, not about tool implementations.
Using `cyrup_tools::read_only_tools` to obtain four real `Arc<dyn Tool>` handles is a different use
and does not widen any refusal vocabulary (the refusal vocabulary here is
`WATCHDOG_ALLOWED_TOOL_NAMES`, a local `[&str; 5]`). **Promoting `cyrup-tools` to a production
dependency is correct and adds no new compile surface** — `cyrup-tools`' own deps are
`cyrup-core` + leaf crates only (no cycle), and the existing comment already records that
*"`cyrup-tools` is already normal-reachable through `cyrup-ext` with the same default features"*.
The executor MUST amend that Cargo.toml comment in the same commit to say the crate is now a
production dependency for the watchdog review's read-only tool set, and that the
`HOST_BUILTIN_TOOL_NAMES` oracle stays a separate, test-only statement.

### 3c. Seams that DO NOT EXIST — and what has to be built

**GAP-1 (blocking for UW-5's "approve" outcome): `WatchdogPermissionTurn` carries no session
model.** Upstream `:96` resolves against `request.ctx`, i.e. `ctx.model`. cyrup's turn
(`permission_arbiter.rs:564-575`) carries `config`, two prompts, a schema and a cancel token.
`resolve_watchdog_review_model` (`review.rs:270-310`) takes a `WatchdogModelContext { registry,
current_model }`; with `current_model: None` **and** no `config.main.model`, it returns
`Err("Main watchdog review cannot run because the current Pi session model is unavailable and
subagents.watchdog.main.model is not configured.")` (`review.rs:292-298`). `child_resolved_config`
sets `main.model = config.model` (`register_child.rs:73`), which is `None` unless the orchestrator
configured `subagents.watchdog.children.model`. **So without this gap closed, an `ask` in the
common configuration still denies — with a different message.** That would be a textbook
"wired but still cannot produce the outcome" failure.

Fix: extend `WatchdogPermissionTurn` with the same live-session slice the review already has —
`review::WatchdogSessionContext` (`review.rs:688-694`) resolved through a
`WatchdogSessionContextFn` (`review.rs:702`) — and thread it from the ONE production binding. The
child runtime already holds the late-bound slot
(`prompt_runtime.rs:1432` `services: Arc<Mutex<Option<Arc<dyn HostServices>>>>`) and
`child_watchdog_review` (`prompt_runtime.rs:2441-2458`) already builds exactly this closure; reuse
it rather than writing a second one. `PermissionGate` (`prompt_runtime.rs:1462-1467`) needs the
extra field, and `with_permission_gate` (`:1749-1775`) the extra parameter.

**GAP-2 (blocking for an offline reachability test, and for prompt-cache/provider fidelity):
there is no `getRegisteredProviderConfig` analogue on `HostServices`.** Verified:
`grep -rn "fn registered_provider\|getRegisteredProviderConfig"` over `crates/` returns **nothing**,
and `cyrup-ext/src/host/services.rs`'s `// --- models ---` block (`:541-553`) offers only
`models() -> Value`, `current_model() -> Option<String>`, `context_usage()`,
`thinking_level()`. Meanwhile `LiveHostServices` (`cyrup-session-svc/src/host_services.rs:616-617`)
**already holds `provider: Arc<dyn Provider>`** — the session's own provider — and
`cyrup-ext` already depends on `cyrup-provider` (`cyrup-ext/Cargo.toml:26`).

Build it: one defaulted trait method on `HostServices`, e.g.

```rust
/// pi `ctx.modelRegistry.getRegisteredProviderConfig(provider)`
/// (`permission-arbiter.ts:99-104`, `review.ts:276-281` @v0.68.0) — the SESSION's own provider for
/// `provider_id`, so a subagent-internal model call streams through the same transport the session
/// does instead of rebuilding one. `None` = "not this provider", which is upstream's fall-through
/// to the generic `streamSimple`.
fn registered_provider(&self, _provider_id: &str) -> Option<Arc<dyn cyrup_provider::Provider>> { None }
```

implemented in `LiveHostServices` by returning `self.provider` when `provider.id() == provider_id`
(the `ProviderSwap` (`cyrup-session-svc/src/provider_swap.rs`) is the live-swappable holder; prefer
reading through it so a mid-session `/model` cross-provider swap is honoured). **Honour upstream's
api-equality guard**: only use it when its `api` matches the selected model's `api`
(`WatchdogModelInfo::api`, `model_selection.rs:60`), else fall back to the builtin stack.

**GAP-3 (non-blocking, quality): no `WatchdogPermissionDecisionTool` value.** See §2a.1. Build it
as the mirror of `WatchdogWarnTool`, in `permission_arbiter.rs`, carrying `:79-83`'s five
descriptors and a `record(&self, params) -> Result<(), String>` with the `:85` first-wins latch.
Without it the bound agent has to re-derive the tool NAME and DESCRIPTION — and the description is
prompt text the model reads.

**GAP-4 (non-blocking): opencode session headers unreachable from this crate.** See §2c.

**GAP-5 (non-blocking, must be stated): `NoTurnReviewAgent` / `NoDecisionPermissionAgent` do not
disappear.** Both are still needed as documented fixtures for the ~20 existing unit tests in their
own modules (`review.rs:1385,1412,1459,1479,1577`; `permission_arbiter.rs:968,984,999,1008,1110`)
and for `prompt_runtime.rs:2858`. **Move them under `#[cfg(test)]` or into a `pub` test-support
position and update their docs to say they are no longer the production default** — leaving a
`pub` type whose doc says "the honest stand-in for a deployment with no provider bound" while
production no longer binds it is the same doc-asserts-wiring defect §1b.5 flags. Do NOT
`#[allow(dead_code)]` them.

## [AUG] 4. THE PRODUCTION CALL SITES — be exact

This is the most important section. There are **three** production bindings and **one** false one.

### 4a. The three real sites

1. **`crates/cyrup-ext-subagents/src/watchdog/register_main.rs:177`** — the ORCHESTRATOR's review.
   Inside `register_main_watchdog`'s `options.review.unwrap_or_else` arm (`:156-179`).
   Reached in production by
   `crates/cyrup-ext-subagents/src/extension/host/mod.rs:239` (`SubagentsExtension::new` →
   `register_main_watchdog(Arc::new(move || watchdog_executor.host_services()), &cwd,
   RegisterMainWatchdogOptions::default())`), whose runtime is then driven from the extension's
   `on_event` at the agent-end boundary → `MainWatchdogRuntime::review_delta`
   (`watchdog/runtime.rs:1267`, the review call itself at `:1301`).
   Replace `Arc::new(super::review::NoTurnReviewAgent)` with the real agent. The `review_description`
   at `:189-196` becomes TRUE the moment this lands (definition-of-done item 4 discharges for free;
   assert it, do not just claim it).

2. **`crates/cyrup-ext-subagents/src/prompt_runtime.rs:2438`** — the CHILD's review, inside
   `child_watchdog_review` (`:2423-2460`), called from `prompt_runtime_from_env` at `:2366-2368`
   (`raw_watchdog_config.as_ref().map(|_| child_watchdog_review(&cwd, &services))`).
   Reached in production by `crates/cyrup/src/session_launch.rs:130`
   (`prompt_runtime_extension_for_env()`, `prompt_runtime.rs:2221`) → `with_native_extension` at
   `:133`. Same replacement; the live-session closure at `:2441-2458` stays as-is.

3. **`crates/cyrup-ext-subagents/src/prompt_runtime.rs:2407`** — the CHILD's permission arbiter,
   inside the `.with_permission_gate(…)` call at `:2399-2408`, same
   `prompt_runtime_from_env` → `session_launch.rs:130` production chain.
   The gate's runtime consumer is `PermissionGate::evaluate`
   (`prompt_runtime.rs:1496-1531`), whose `Ask` arm (`:1515-1529`) calls
   `request_watchdog_permission(&WatchdogPermissionRequest{…}, self.arbiter.as_ref())`, dispatched
   from the runtime's `on_event` `ToolCall` arm. **`cancel: None` at `:1522` is a residual** — the
   arbiter's cancel path is dead on the production call because no token is threaded; fix it with
   the `HostCtx`'s cancel if one is available, or leave a `[CYRUP-DELTA]` naming it.

### 4b. The false site

**`prompt_runtime.rs:2858` is inside `#[cfg(test)] mod permission_gate_tests` (`:2710-2866`).**
See §1b.1. Do not "wire" it; do not count it.

### 4c. The seam-owner sites the change also lands on

* `prompt_runtime.rs:1462-1467` (`struct PermissionGate`) and `:1749-1775`
  (`with_permission_gate`) — for GAP-1's session-context parameter.
* `cyrup-ext/src/host/services.rs` `// --- models ---` block (`:541-553`) and
  `cyrup-session-svc/src/host_services.rs:1126` (`impl HostServices for LiveHostServices`) — for
  GAP-2.

## [AUG] 5. The reachability tests

Two are required — one per stub — and both must drive a PRODUCTION entry point. A third is
recommended.

### 5a. UW-4, the strong one: a real model turn emits a real `watchdog_warn` finding

**Home:** a new file in `crates/cyrup-it/tests/subagents/`, declared in
`crates/cyrup-it/tests/subagents/main.rs` (module list at `:73-120`), run by
`cargo nextest run -p cyrup-it --features it` (README `:278`); the target is
`required-features = ["it"]` (`cyrup-it/Cargo.toml:176-178`). Model it on
`extension_end_to_end_smoke.rs`, which is the crate's own stated "single most important test"
and already assembles a real `SessionBuilder` session with the real `SubagentsExtension`.

**Production entry point it drives:** `cyrup_test_support::harness::create_harness_with_extensions`
with `HarnessOptions { native_extensions: vec![Arc::new(SubagentsExtension::new(…))],
queue_responses: true, responses: […] }` (`cyrup-test-support/src/harness.rs:97-99`, `:364-365`).
That constructs the extension through `SubagentsExtension::new` → `register_main_watchdog`
(`extension/host/mod.rs:239`) → the REAL review, and it installs `LiveHostServices` over the
harness's `ScriptedProvider` (`harness.rs:340-368`, `builder.rs:1142`).

**Why it can run offline.** With GAP-2 closed, the review's agent asks
`HostServices::registered_provider(<session model's provider>)` and gets the harness's
`ScriptedProvider` back, so the nested turn consumes scripted responses. `queue_responses: true`
selects the QUEUE-consuming faux flavour (`harness.rs:342-346`), so the script is
`[ <main turn: plain assistant text>, <review turn: an assistant message calling watchdog_warn
   with severity=blocker/summary/evidence/recommendedAction>, … ]`.
Model resolution takes the INHERITED-session-model arm: the harness's model is
`faux/faux-1`; `register_main.rs:115-118` builds a `WatchdogModelInfo` from
`services.current_model()` and falls back to the bare info when the builtin catalog does not know it
(`.unwrap_or(info)`), and that arm does **not** consult `has_configured_auth`
(only `resolve_configured_model`, `review.rs:220`, does).

**What it asserts:**
1. a `watchdog_warn` finding reached the runtime — observed at the OUTPUT edge, via the injected
   warning message (`register_main.rs:197-213` `display_warning` →
   `HostServices::inject_message` with `SUBAGENT_WATCHDOG_WARNING_TYPE`), or via
   `watchdog().get_snapshot(None).last_warning`;
2. the finding's `summary`/`evidence` are the SCRIPTED ones, so the assertion is on data that can
   only have come through the model turn;
3. `snapshot.review_description == "real model review"` — now true rather than aspirational;
4. `snapshot.failed_reviews == 0`.

**Why it fails over a gutted implementation.** `NoTurnReviewAgent` returns `Ok(Vec::new())`, so no
tool call happens, `emit_warning` is never invoked, `accept_warning`
(`runtime.rs:1772-1794`) never runs, `last_warning` stays `None` and no message is injected —
assertions 1 and 2 fail. It also fails if the implementation runs a turn but forgets to wire
`WatchdogWarnTool::execute` to `request.emit_warning`, or exposes the tool under a different name,
or never surfaces the tool at all.

### 5b. UW-4's negative half: the read-only policy holds at EXECUTION time

Same harness, script the review turn to call **`write`** (a tool the review was never GIVEN). Assert
the run's tool result is the block reason
`"Watchdog reviews are read-only; tool 'write' is not allowed."`
(`review.rs:589-591`) and that no file was written. This is upstream `:338-340`, and it is the
layer the trait doc (`review.rs:627-628`) says an implementation is most likely to skip — a
tool-LIST filter alone passes 5a and fails this.

### 5c. UW-5, the in-crate one: an `ask`-tier tool is APPROVED and runs

**Home:** extend `crates/cyrup-ext-subagents/src/prompt_runtime.rs`'s existing
`mod permission_gate_tests` (`:2710-2866`), which ALREADY drives the production surface:
`env_extension(policy, audit)` (`:2736-2746`) builds the extension through the real
`prompt_runtime_extension_from`, and each test goes through `ext.init(&mut api)` +
`ext.on_event(&call("write"), &ctx)` — i.e. the real subscription and the real dispatch, not a
direct call to the gate. `an_ask_tier_tool_fails_closed_and_writes_both_audit_records` (`:2779`) is
the template.

Give `env_extension` a third env var — `CHILD_WATCHDOG_CONFIG_ENV`
(`watchdog/child_status.rs`) carrying an ARMED child config — so the arbiter gets past
`:688-696`'s "child watchdog is disabled" arm, and drive:

* **approval:** with the scripted provider reachable (GAP-2) and the arbiter's model resolved,
  script a `watchdog_permission_decision{decision:"approve", reason:"…"}` call. Assert
  `ext.on_event(&call("write"), &ctx)` is `HookOutcome::Noop` (the tool RUNS) and the audit's
  second line is `{"decision":"approve","approved":true,"reason":"<the scripted reason>"}`.
  **Gutted, `NoDecisionPermissionAgent` returns `Ok(None)` → `Block` with
  `"…returned no decision."` and `decision:"malformed"` — both assertions fail.**
* **deny:** script `{"decision":"deny","reason":"…"}`. Assert `Block` and that the reason carried
  into the block string is the MODEL's own reason (`prompt_runtime.rs:1528`), not a generic one.
  Gutted, the reason would be the malformed sentence.

If the executor cannot reach a scripted provider from this in-crate module (no `LiveHostServices`
there — the child runtime's `services` slot is `None` in these tests), the approve/deny pair must
move to `crates/cyrup-it/tests/subagents/` alongside 5a, driving the same harness with a child
config in `HarnessOptions`-supplied env. **Do not settle for a test that only proves a nicer
denial** — item 1 of the definition of done is *approval*, and a denial-only test passes over an
implementation that can never approve.

### 5d. The fail-closed regression guard (definition-of-done item 2)

Keep — and extend — the existing coverage so the security property is pinned independently of the
new agent:
* `an_ask_tier_tool_fails_closed_and_writes_both_audit_records` (`prompt_runtime.rs:2779`) must keep
  passing: an armed policy with a DISABLED child watchdog still denies with `unavailable`.
* Add: with the real agent bound and the provider erroring (script a provider error / no
  credentials), `on_event` must still `Block`, the audit must record `approved:false`, and the
  decision must be `error` (or `timeout`). **Assert `approved == false` explicitly** — that single
  assertion is what stands between this change and a silent fail-OPEN regression.
* `permission_arbiter.rs`'s existing `FailingAgent`/`HangingAgent` unit tests (`:819-850` and their
  drivers) pin the mapping and must stay green; if §2a.8's v0.68.0 race semantics are adopted, the
  expected STRINGS change (no `failed closed:` prefix on timeout; `cancelled` instead of `error`
  mid-turn) — update them deliberately and say so in the commit, do not weaken them.

## [AUG] 6. Behaviour that MUST be preserved

1. **Fail-closed on every non-approval.** All eight arms of the table at
   `permission_arbiter.rs:12-21` return `approved: false`. `request_watchdog_permission` returns
   `WatchdogPermissionResult`, never a `Result` — deliberately (`:657-660`: *"a caller that has to
   interpret an error to decide whether a tool may run is one bad `match` away from failing open"*).
   **Do not change that signature.** A child has no human; "no answer" is never "allowed".
2. **Both audit records, always.** `permission.request` before any work (`:665-674`) and
   `permission.decision` after (`:640-648`), joined by `requestCreatedAt`. The new agent must not
   introduce an early return that skips `finish`.
3. **Redaction before the model and before disk.** `permission_args_preview`
   (`:301-321`) → `redact` (`:261-295`) → `redact_secret_values` (`:236-258`). The preview is what
   goes into the arbiter's prompt (`:711`) AND into the audit (`:671`). The case-insensitivity at
   `:220-235` is called out in-tree as a security property; the new turn must consume the SAME
   preview string and must not re-serialize raw args anywhere.
4. **`bash` and the four internal tools are never gated** (`permission_decision`, `:484-494`;
   `INTERNAL_TOOLS`, `:371-376`) — enforced twice on purpose. A model turn must never be reached for
   them.
5. **An undecodable policy blocks everything** (`PermissionPolicy::Invalid`,
   `prompt_runtime.rs:1481-1491`, `:1503-1508`) — a `[CYRUP-DELTA]` that is strictly safer than
   upstream's process kill. Keep it.
6. **The review's two-layer tool policy.** The EXPOSE list (`review.rs:80`) subtracts
   `watchdog_warn` so nothing can shadow the bound tool (`:74-79`); the PERMIT list (`:61`)
   includes it because `beforeToolCall` re-checks it. Both layers, both places. §5b pins layer two.
7. **Freeform assistant text is ignored** (`review.ts:229` / `review.rs:17-20`). A warning exists
   ONLY if `watchdog_warn` was called. Do not parse prose into findings.
8. **A clean review is `stop` with zero warnings** — `final_stop_reason` (`review.rs:598-614`)
   maps everything that is not `error`/`aborted`/`length` to `Stop`, INCLUDING an empty message
   list. A review that rambled is CLEAN, not partially parsed.
9. **`MainWatchdogReview::review` returns `warnings: Vec::new()`** (`review.rs:862-865`) — findings
   travel through the EMITTER, not the result. Do not start populating `warnings`; the runtime's
   `accept_warning` path (`runtime.rs:1322-1323` vs `:1381-1386`) would then double-count.
10. **Both cancel checks around the turn survive** — before model resolution
    (`review.rs:818-823`) and after it (`:836-841`), which is upstream `:258` and `:274`, plus
    `:349`/`:355` around `prompt`. The new agent must additionally abort the nested `Agent` when
    `turn.cancel` fires (upstream `:345-347`) — contract 3 of the trait doc (`review.rs:629`).
11. **The arbiter's `agentEndTimeoutMs` bound** (`permission_arbiter.rs:717-730`, upstream `:136`)
    stays OUTSIDE the agent, and the timeout must cancel the token so the nested run actually stops.
12. **`is_inert`** (`prompt_runtime.rs:1788-1804`) must keep answering the same for every
    configuration — adding a field to `PermissionGate` must not change when the extension installs.
13. **Emission-guard semantics** (`runtime.rs:1772-1794`): threshold, dedup identity, budget. The
    warn tool's result text already tells the model whether the runtime took it
    (`review.rs:415-425`) so a rejected warning is not simply re-sent. Route every call through
    `WatchdogWarnTool::execute` and return its text — do not synthesise a success string.
14. **`WatchdogWarnTool::execute`'s `Err` must surface to the model as a tool error**
    (`review.rs:493-498`), not be swallowed: a malformed warning becomes a correction the model can
    act on, or it becomes silence.
15. **Baseline 10414/10414.** The two stub types are referenced by ~20 existing tests (§3c GAP-5);
    keep every one of them compiling and passing.

## [AUG] 7. Files expected to be touched

**Owned by this task (the executor's sibling must not touch these):**

| file | why |
|---|---|
| `crates/cyrup-ext-subagents/src/watchdog/review.rs` | the real `WatchdogReviewAgent`; retire `NoTurnReviewAgent` from production; fix the stale comments at `:761` and `:893-905` |
| `crates/cyrup-ext-subagents/src/watchdog/permission_arbiter.rs` | the real `WatchdogPermissionAgent`; `WatchdogPermissionDecisionTool` (GAP-3); session context on `WatchdogPermissionTurn` (GAP-1); optionally the v0.68.0 race semantics (§2a.8) |
| `crates/cyrup-ext-subagents/src/watchdog/agent_turn.rs` *(new)* | the shared nested-`Agent` construction both agents use: model → `ModelRef`, thinking → `ModelThinkingLevel`, provider resolution (session-registered → builtin stack), credential overlay, opencode headers, the `Hooks` impl that enforces `beforeToolCall`, the `cyrup_core::Tool` adapters, cancel/abort plumbing, and the `Vec<AgentMessage>` → `Vec<Value>` fold. Register in `watchdog/mod.rs` |
| `crates/cyrup-ext-subagents/src/watchdog/mod.rs` | module declaration |
| `crates/cyrup-ext-subagents/src/watchdog/register_main.rs` | `:168` binding |
| `crates/cyrup-ext-subagents/src/prompt_runtime.rs` | `:2407`, `:2438` bindings; `PermissionGate` (`:1462-1467`) + `with_permission_gate` (`:1749-1775`) for GAP-1; the `cancel: None` residual at `:1522`; `permission_gate_tests` additions (§5c) |
| `crates/cyrup-ext-subagents/Cargo.toml` | promote `cyrup-tools` to `[dependencies]`; amend the existing dev-dep comment |
| `crates/cyrup-ext/src/host/services.rs` | GAP-2: `registered_provider` on `HostServices` (defaulted `None`) |
| `crates/cyrup-session-svc/src/host_services.rs` | GAP-2: the `LiveHostServices` impl |
| `crates/cyrup-it/tests/subagents/<new file>.rs` + `crates/cyrup-it/tests/subagents/main.rs` | §5a/§5b (and §5c if it must move) |
| `docs/gap-analysis/PARITY-GAPS.md` | close UW-4 and UW-5 with re-greped citations; correct the `:2858` claim and the v0.43.0-vs-v0.68.0 upstream line numbers (`:1587-1597`); update the `:1890` summary line |

**Read-only for this task (do not edit):** `watchdog/runtime.rs`, `watchdog/model_selection.rs`,
`watchdog/register_child.rs`, `watchdog/emission_guard.rs`, `watchdog/types.rs`,
`exec/spawn_plan.rs`, `extension/executor/background.rs`, `extension/host/mod.rs`,
`crates/cyrup/src/session_launch.rs`, `crates/cyrup-session-svc/src/provider_swap.rs`.

**Collision warning for the sibling executor (PB-8, the RPC bridge):** `prompt_runtime.rs` and
`cyrup-ext/src/host/services.rs` are both large, shared files. `PB-8` works the inter-extension bus
(`HostServices::emit_event`, `services.rs:~460`) and `extension/tool/routing.rs`. The overlap is
`services.rs` only, in two DIFFERENT method blocks (`// --- models ---` at `:541-553` here vs the
bus block there). Coordinate on that one file; nothing else overlaps.

## [AUG] 8. Blockers

**None.** Every type this task needs is `pub` and already in the production dependency graph, or is
one defaulted trait method away (GAP-2). The two genuinely NEW things are small and named:
`HostServices::registered_provider` (§3c GAP-2) and the session-context field on
`WatchdogPermissionTurn` (§3c GAP-1).

The one thing that is NOT small, and which the executor must not discover late: **without GAP-2 the
reachability tests cannot run offline**, and without GAP-1 **UW-5 cannot produce an approval in the
default configuration**. Both are part of this task. Closing UW-4/UW-5 without them would be the
sixth repetition of the failure mode this programme has been told to stop.

## [AUG] 9. Gates

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2
cargo clippy -p cyrup-it --features it --all-targets
cargo nextest run --workspace --features test-fixtures      # baseline 10414/10414
cargo nextest run -p cyrup-it --features it                 # the §5a/§5b home
```


---

# [EXEC] Execution pass — 2026-09-18

Both stubs are replaced by real nested-`cyrup_agent::Agent` turns, bound at every production site,
with mutation-checked reachability tests. `cargo nextest run --workspace --features test-fixtures`:
**10435/10435 passed, 9 skipped** (baseline 10414; +21 from this change).

## The production call sites — three, exactly as §4a said

1. **`crates/cyrup-ext-subagents/src/watchdog/register_main.rs:168`** — the ORCHESTRATOR's review.
   `Arc::new(super::review::ModelTurnReviewAgent::new(cwd.to_path_buf(), Arc::clone(&turn_services)))`
   inside `register_main_watchdog`'s `options.review.unwrap_or_else` arm. Reached from
   `extension/host/mod.rs:259` (`SubagentsExtension::new`), driven by `MainWatchdogRuntime::review_delta`
   (`watchdog/runtime.rs:1301`).
2. **`crates/cyrup-ext-subagents/src/prompt_runtime.rs:2491`** — the CHILD's review, inside
   `child_watchdog_review`. Reached from `crates/cyrup/src/session_launch.rs:130`.
3. **`crates/cyrup-ext-subagents/src/prompt_runtime.rs:2445`** — the CHILD's permission arbiter,
   `ModelTurnPermissionAgent::new(arbiter_registry, AmbientReviewAuth, child_services_fn(...))`
   inside `.with_permission_gate(...)`, same `session_launch.rs:130` chain. Its runtime consumer is
   `PermissionGate::evaluate`'s `Ask` arm.

`prompt_runtime.rs:2858` was NOT wired — §1b.1 was right, it is inside `#[cfg(test)] mod
permission_gate_tests` and `a_child_with_no_policy_installs_no_gate` still passes unchanged (it
gained the new `None` session argument only).

## What landed

* **`watchdog/agent_turn.rs`** *(new, 620 lines, pinned @v0.68.0)* — the shared nested-`Agent`
  construction: model/thinking onto `ModelRef`/`ModelThinkingLevel`, the
  session-registered-provider preference with upstream's api-equality guard and the builtin-stack
  fallback, the credential overlay (`api_key`/`env`/merged `headers`), the private
  `opencodeSessionHeaders` port (GAP-4), the `Hooks::before_tool_call` execution-time policy, the
  `key_resolver` narrowing, agent-wide `ToolExecution::Sequential`, cancel→`Agent::abort` around
  `prompt`, the `cyrup_core::Tool` adapters for `watchdog_warn` and `watchdog_permission_decision`,
  and the `Vec<AgentMessage> -> Vec<Value>` fold (no trait-signature change, as §3a predicted).
* **`watchdog/review.rs`** — `ModelTurnReviewAgent`; `NoTurnReviewAgent` moved under `#[cfg(test)]`
  with a doc that says it is a fixture (GAP-5, no `allow(dead_code)`); the "what has no production
  caller" block and the two stale line-number comments (§1b.5) repaired.
* **`watchdog/permission_arbiter.rs`** — `ModelTurnPermissionAgent`; `WatchdogPermissionDecisionTool`
  with the `:79-83` descriptors and the `:85` first-wins latch (GAP-3); `session` on
  `WatchdogPermissionRequest`/`WatchdogPermissionTurn` (GAP-1); `NoDecisionPermissionAgent` under
  `#[cfg(test)]`; **the v0.68.0 race semantics adopted in full** (§2a.8, see below).
* **`prompt_runtime.rs`** — the two bindings; `PermissionGate.session` +
  `with_permission_gate(..., session)`; `child_services_fn` and `child_watchdog_session_context`
  factored out so the child's review and its arbiter cannot resolve different models; a
  `[CYRUP-DELTA]` on the `cancel: None` residual naming exactly what it costs.
* **`cyrup-ext/src/host/services.rs`** — `HostServices::registered_provider`, defaulted `None`
  (GAP-2). Landed in the `// --- models ---` block, as the collision note promised.
* **`cyrup-session-svc/src/host_services.rs` + `builder.rs`** — the `LiveHostServices` impl over the
  live `ProviderSwap`, late-bound with `attach_provider_swap` beside the existing
  `attach_event_bus`, so a mid-session cross-provider `/model` swap is honoured.
* **`cyrup-ext-subagents/Cargo.toml`** — `cyrup-tools` promoted to `[dependencies]`; the old dev-dep
  comment rewritten to say what is now true and to keep the `HOST_BUILTIN_TOOL_NAMES` oracle stated
  as the separate, test-only thing it is.

## Upstream drift (§2a.8): ADOPTED, all three

`permission-arbiter.ts`'s race is now the v0.68.0 shape — timeout resolves `timeout` with NO
`failed closed:` prefix (`:136`), a mid-turn cancel resolves `cancelled` (`:138`), and a `completed`
latch makes `finish` idempotent (`:49,:52-53`). The module-doc fail-closed table was rewritten and
carries a paragraph saying which rows moved and why; `a_hanging_agent_denies_on_the_configured_timeout`
was updated deliberately (its first assertion — that a hung turn DENIES — is unchanged) and three
tests were added: the mid-turn-cancel arm, the `finish` latch, and the decision tool's first-wins
behaviour.

## Reachability, and the mutations that proved it

* `crates/cyrup-it/tests/subagents/watchdog_model_turn_integration.rs` (§5a/§5b) — real
  `SubagentsExtension` in a real session, watchdog armed through the real
  `/subagents-watchdog session on` handler, a turn that writes a file, then the nested review turn
  calling `watchdog_warn`. Asserts `last_warning`'s summary/evidence/recommendedAction are the
  SCRIPTED strings, `review_description == "real model review"`, `failed_reviews == 0`, and — read
  off the faux provider's captured wire request — that the reviewer was offered exactly
  `{find, grep, ls, read, watchdog_warn}`.
* `crates/cyrup-it/tests/subagents/watchdog_permission_arbiter_integration.rs` (§5c/§5d) — approve
  (tool RUNS, audit says `{"decision":"approve","approved":true}` with the model's reason), deny
  (block carries the MODEL's sentence), provider error (`approved:false`, `decision:"error"`), and
  no-tool (`malformed`). It had to live here, not in `permission_gate_tests`: that module has no
  `LiveHostServices`, so the arbiter can reach no provider there.
* **Mutations run, each restored afterwards:**
  - `ModelTurnReviewAgent::run -> Ok(Vec::new())` → both §5a/§5b tests FAILED.
  - `ModelTurnPermissionAgent::decide -> Ok(None)` → approve, deny and provider-error FAILED;
    `malformed` still passed (it IS the gutted behaviour).
  - `PermissionGate::evaluate` passing `session: None` (GAP-1 removed) → approve FAILED.
  - `LiveHostServices::registered_provider -> None` (GAP-2 removed) → 5 of 6 FAILED.

## What §5b could NOT prove, stated rather than glossed

The execution-time refusal SENTENCE (`review.ts:338-340 @v0.68.0`) is not observable end to end.
cyrup's nested agent gets its tool list from `agent_turn.rs` and nowhere else, so a tool the permit
list rejects is also a tool the agent does not have, and the loop answers "Tool write not found"
before any hook runs — which the captured wire request in the test shows. Upstream's second layer is
for harness-supplied tools; here it guards a future widening of `cyrup_tools::read_only_tools`. The
layer IS pinned, by `agent_turn`'s `the_tool_policy_hook_blocks_with_upstreams_own_sentence`, which
drives the same `Hooks` seam the loop calls and asserts the exact string plus the absent
`terminate` key. The end-to-end test asserts what it can: `write`/`edit`/`bash`/`subagent` are never
offered, the named `write` comes back as an error result, the review survives it and still warns,
and no file is written.

## Deliberately out of scope (§2b), flagged for ids

Three v0.68.0 review features have no cyrup port at all and were NOT smuggled in: `watchdog_diff`
(a whole `diff-tool.ts`), `watchdog_ask`/the clarification yield, and
`loadWatchdogGuidance`/`WATCHDOG.md`. Also unported: `importance` on `WatchdogWarnParams`,
`errorMessage` on the review result, `toolCount`, and the v0.68.0 prompt-text drift. The
`ModelTurnReviewAgent` doc names all of them.

## Gates

```
cargo fmt --all -- --check                                  clean
cargo clippy --workspace --all-targets -- -D warnings       clean
cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2        clean
cargo clippy -p cyrup-it --features it --all-targets        clean
cargo nextest run --workspace --features test-fixtures      10435/10435 passed, 9 skipped
cargo nextest run -p cyrup-it --features it                 549/550 passed (see below)
```

## The one red in `cyrup-it --features it`, and the proof it is not this change

`companions_hostservices_proof::background_completion_injects_a_turn_triggering_message_on_the_real_host_services`
fails with "inject_message never fired". It is **pre-existing and unrelated to UW-4/UW-5**, proven
rather than asserted: every file this task changed was reverted to `git show HEAD:<path>` (and the
three new files deleted), the test was re-run, and it FAILED identically; the change was then
restored from a scratchpad tar and the workspace suite re-run green.

The reason it was never seen before is that `crates/cyrup-it/tests/subagents/` did not COMPILE at
HEAD — `ResultFile::schedule_origin` landed in `background/records.rs:593` without updating that
file's struct literal, and the PB-8 sibling repaired that one line. So the whole `subagents` `it`
target has been unrunnable, and this failure surfaced with it. It lives in the background
completion-delivery path (`extension/executor/notices.rs`'s `install_completion_watcher` ->
`background/watch/`), which this task touches nowhere.

The other 9 failures seen on a first run were the suite's own environment guards
(`support::env::no_ambient_provider_credentials` in 8 targets plus
`session_svc::model_registry::selector_lists_configured_non_faux_provider_and_hides_unconfigured`),
caused by ambient `AWS_*`/`ANTHROPIC_BASE_URL`/`CLOUDSDK_AUTH_ACCESS_TOKEN` in this container. They
pass with those unset, which is what `cargo run -p xtask -- it` does for the whole suite.

---

# [FIX] — QA remediation pass

Closes every item in `qa-UW-4_UW-5.json` (verdict `pass`, 3 minor defects + 4 fake tests). Each
fake test was repaired by making its assertion DISCRIMINATE and then proving it with the mutation
the QA reviewer described — applied to production, run, observed red, restored.

## Defects

### D1 — `agent_turn.rs:465`: the pre-prompt cancel arm folded a CANCELLED review to a CLEAN one

**Was:** `if request.cancel.is_cancelled() { return Ok(Vec::new()); }`. `final_stop_reason(&[])`
hits its no-assistant-message arm and answers `ReviewStopReason::Stop`, so
`MainWatchdogReview::review` handed back `{ warnings: [], stop_reason: Some(Stop) }` — a passed
review — where `review.ts:349 @v0.68.0`, the line the comment directly above it cites, returns
`{ stopReason: "aborted" }`.

**Now:** the arm returns `aborted_before_prompt()` (`agent_turn.rs`), one terminal assistant message
`{"role":"assistant","stopReason":"aborted"}` — the smallest value the fold maps to `Aborted`, and
the same shape the MID-turn abort produces naturally (`cyrup-agent`'s `settle_aborted`,
`agent/run/assistant_stream.rs:137-143`). The two keys are spelled as `AssistantMessage`'s own
serializer writes them (`cyrup-core` `message/assistant.rs:150` `role`, `:168` `stopReason`). No
`errorMessage`: upstream's pre-prompt return is the bare `{ stopReason }` (`:349`), unlike its
post-turn return (`:358`). A `[CYRUP-DELTA]` on the helper states WHAT (a message list, not a result
object) and WHY (this seam's contract is the list the fold reads), and that WHY is true.

`review_delta` now reports such a review through its `Some(reason) if reason != Stop` arm
(`runtime.rs:1326-1332`) as `fail("… stop reason 'aborted'.")` with a `failed_reviews` increment.
The arbiter is unaffected: `Aborted != Error`, so `decide` still reaches `decision_tool.decision()`,
still finds `None`, and still denies as `malformed`.

**Pinned by:** `agent_turn::tests::a_turn_cancelled_before_the_prompt_reports_aborted_not_stop`.

### D2 — `prompt_runtime.rs:2861`: doc asserting wiring that is no longer true

The doc on `an_ask_tier_tool_fails_closed_and_writes_both_audit_records` still said the production
gate binds `NoDecisionPermissionAgent` and that the arm reports `malformed`. Both were false after
UW-5: the gate binds `ModelTurnPermissionAgent` (`prompt_runtime.rs`'s single `with_permission_gate`
call), and this env ships no `CHILD_WATCHDOG_CONFIG_ENV`, so `request_watchdog_permission` takes its
"the child watchdog is disabled" arm (`permission-arbiter.ts:74 @v0.68.0`) and denies as
`unavailable` before any agent runs — which is the sentence the test's own assertion already
required. Rewritten to say exactly that, and to name where the `malformed` arm IS covered.

### D3 — `review.rs:956`: nothing pinned that the review agent installs the execution-time policy

`ModelTurnReviewAgent` now exposes `tool_call_block_reason()` — the ONLY source of the policy — and
builds its turn through `turn_request()`, which `run` calls and adds nothing to. Two independent
pins result: a mutation of the accessor and a mutation of the install site each turn a test red
(see M2/M4 below). The accessor is `pub` because the layer is unreachable end to end — `cyrup-agent`
locates the tool before running `before_tool_call` (`agent/run/tools/preflight.rs:17,80`), so a tool
the permit list rejects is answered "Tool <name> not found" first — and an out-of-crate test has to
reach the production value to assert the sentence.

## Fake tests

### F1 — `agent_turn.rs:622` defined the closure it asserted on

`the_arbiters_block_reason_permits_only_its_one_tool` declared a verbatim copy of the arbiter's
closure and asserted on it. Rewritten to take the closure from
`ModelTurnPermissionAgent::tool_call_block_reason()` — the value `decide` installs, via the new
`ModelTurnPermissionAgent::turn_request()` — and to drive it through the real `WatchdogToolPolicy`
`Hooks` seam, asserting `BeforeOutcome::Block` with the exact sentence for five names.
`the_tool_policy_hook_blocks_with_upstreams_own_sentence` was moved onto the production review
policy the same way (it, too, had been building its own `WatchdogToolPolicy`).

### F2/F3 — the two `assert!(!PathBuf::from("probe").exists())` assertions were vacuous

They resolved against the TEST PROCESS's cwd (the cargo target dir), which no tool in either session
ever writes to. Fixed at the root:

- `cyrup-test-support`'s `Harness` gained `cwd()`, returning the session temp dir every relative
  tool path resolves against.
- `watchdog_model_turn_integration::armed_harness` no longer `std::mem::forget`s its two tempdirs;
  it returns them, so the REVIEWER's cwd (the extension `work_dir`) can be named. Both probes are
  now checked in `work_dir` AND `harness.cwd()`.
- `watchdog_permission_arbiter_integration::run_ask_tier_turn` returns the `Harness`, and every
  deny/fail-closed test asserts against `harness.cwd()`.
- Each negative assertion is backed by a CONTROL in the positive test: the same expression over the
  same directory finds `watchdog-probe.txt` / `arbiter-probe.txt` when the write really happened.

### F4 — `a_review_that_names_a_write_tool_is_refused_at_execution_time` overstated itself

The `|| wire.contains("Tool write not found")` disjunct made the real block-reason string
unreachable as a requirement. The disjunct is gone, and the test now asserts BOTH layers, each where
it actually fires:

- layer one, the tool LIST (`review.ts:305`): the reviewer was never offered `write`, and the exact
  `"toolName":"write" … "Tool write not found"` wire result came back;
- layer two, EXECUTION TIME (`:338-340`): `ModelTurnReviewAgent::tool_call_block_reason()` — the
  policy production installs — refuses `write` with
  `"Watchdog reviews are read-only; tool 'write' is not allowed."` and permits `read`.

The doc now states plainly why layer two cannot be reached by a scripted response (layer one always
wins in cyrup) instead of implying the end-to-end path exercises it.

## Fail-closed, re-verified per failure mode

The arbiter must never let an unanswerable `ask` through. Existing coverage (provider error,
no-tool/`malformed`, timeout, mid-turn cancel, malformed config, disabled child) all drove DOUBLES.
Added, driving the REAL `ModelTurnPermissionAgent`:

| mode | test |
|---|---|
| no model bound | `permission_arbiter::tests::the_real_arbiter_denies_when_no_model_can_be_resolved` (`error`, `failed closed:` prefix, both audit rows) |
| cancelled | `permission_arbiter::tests::the_real_arbiter_denies_a_cancelled_request` |
| child disabled | `permission_arbiter::tests::the_real_arbiter_denies_when_the_child_watchdog_is_disabled` |
| provider error | `watchdog_permission_arbiter_integration::a_provider_failure_during_the_arbiter_turn_still_denies` (pre-existing) |
| **malformed model OUTPUT** | `watchdog_permission_arbiter_integration::an_arbiter_that_names_an_invalid_verdict_denies_and_nothing_runs` — the model DOES call its one tool but names `"maybe"`, the latch refuses it (`:85-86`), `decide` reports `Ok(None)`, the ask denies, and the probe file is absent |

## Mutations run (each applied to production, observed red, restored)

| # | mutation | observed |
|---|---|---|
| M1 | `agent_turn.rs` cancel arm back to `Ok(Vec::new())` | `a_turn_cancelled_before_the_prompt_reports_aborted_not_stop` FAIL — `left: Stop, right: Aborted` |
| M2 | `ModelTurnReviewAgent::tool_call_block_reason` -> `Arc::new(\|_\| None)` | `review::tests::the_production_review_turn_installs_the_expose_list_and_the_execution_time_policy` + `agent_turn::tests::the_tool_policy_hook_blocks_with_upstreams_own_sentence` FAIL; and in `cyrup-it`, `a_review_that_names_a_write_tool_is_refused_at_execution_time` FAIL on the exact sentence |
| M3 | `ModelTurnPermissionAgent::tool_call_block_reason` -> `\|_\| None` | `the_production_arbiter_turn_installs_one_tool_and_refuses_every_other_name` + `the_arbiters_block_reason_permits_only_its_one_tool` FAIL |
| M4 | install site only: `turn_request`'s `block_reason:` -> `Arc::new(\|_\| None)` (accessor intact) | review pin FAIL, hook test PASS — the two pins discriminate separately |
| M5 | `permission_arbiter::finish` forced `approved = true` (fail OPEN) | 11 deny tests FAIL, including all three new real-arbiter ones |
| M6 | `PermissionGate::evaluate`'s Ask arm returns `None` (gate fails open, audit untouched) | the four arbiter-integration deny tests FAIL |
| M7 | both probe assertions reverted to `PathBuf::from("probe").exists()` | the POSITIVE controls FAIL — `an_ask_tier_tool_is_approved_…` reports `cwd: /tmp/cyrup-harness-xOcHm1`, proving the old form could not see a write that demonstrably happened |

## Gates

```text
cargo fmt --all -- --check                                  clean
cargo clippy --workspace --all-targets -- -D warnings       clean
cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2        clean
cargo clippy -p cyrup-it --features it --all-targets        clean
cargo doc --workspace --no-deps --bins                      no errors
cargo nextest run --workspace --features test-fixtures      10472 tests run: 10472 passed, 9 skipped
cargo nextest run -p cyrup-it --features it                 551 tests run: 550 passed, 1 failed, 0 skipped
```

(The `it` suite needs `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` unset — the
`no_ambient_provider_credentials` guard — which `cargo run -p xtask -- it` does for you.)

## The remaining red, attributed

`companions_hostservices_proof::background_completion_injects_a_turn_triggering_message_on_the_real_host_services`
("inject_message never fired") fails at **pure `HEAD` (5eb05b3)**, proven by A/B: this task's files
were restored with `git show HEAD:<path>` (test still red), then the PB-8 sibling's working-tree
files too (test still red), then everything restored. Neither this pass nor PB-8 caused it.

`fleet_inspector_integration::the_fleet_status_widget_is_published_and_cleared_through_live_host_services`
WAS red and is now fixed here, though it is PB-8's fallout rather than this task's: PB-8 correctly
added a second `set_widget(ASYNC_STATUS_SNAPSHOT_WIDGET_KEY, None, …)` to the shutdown block
(upstream clears both slots — `extension/index.ts:1063` and `:1098`), which broke the test's
`widgets.last()` assumption. The test now looks the fleet-status clear up BY KEY and additionally
asserts the async slot is removed. Left red it would have hidden the next real regression, so it was
repaired rather than reported.
