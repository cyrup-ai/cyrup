# 12 — pi core drift since cyrup HEAD

This area tracks behavior that `pi/` core (packages `ai/`, `agent/`, `coding-agent/`, `tui/`) has changed or added since cyrup's port was written, plus stale ports and defects the port carries against it — provider/API wire behavior, session and compaction entry formats, the extension context surface, the model resolver, retry classification, and the tests that pin any of it. Measured against pi HEAD `a0bb4a48` (itself a merge of `refactor/sqlite-session-connection-cleanup`, i.e. pi is mid-refactor); cyrup has no single pinned pi baseline, so each item cites the specific upstream commit that opened the gap. Headline: eight of the thirty-six pre-existing items are genuinely closed by the 28-commit batch and survived adversarial re-verification with **zero overturns**, while four new items land — a session-format interop defect (DRIFT-037), a silent model-resolver zero-match (DRIFT-038), a timing-assertion test defect (DRIFT-039), and pi's unabsorbed harness-v2 rearchitecture (DRIFT-040) — and DRIFT-009 was materially misdescribed with its severity going **up**. The only `high` remains DRIFT-026: the Google converter silently drops signed empty reasoning blocks and stalls Gemini mid-task. Re-baselined against cyrup HEAD `1806375` on 2026-08-03.

## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| DRIFT-001 | **closed** | Message-anchored tool loading. Confirmed at HEAD on all four halves — producer (`cyrup-ext/src/wrapper.rs:40-58` derives `added_tool_names`; a tool never sets it, matching pi), persistence (`cyrup-session-svc/src/event.rs:387-422`; empty-vec elision at `cyrup-core/src/message.rs:494-500` matches `agent-loop.ts:783`), turn boundary (`cyrup-session-svc/src/hooks.rs:170-180` → `session.rs:4206-4217`, which really does call `refresh_extension_tools()` and drain `take_pending_active_tools()`), provider (`cyrup-provider/src/utils/deferred_tools.rs:68`). Commits `f777e44` + `c2a7acb`. **c2a7acb's self-flagged "dedupes with the flag OFF" debt is pi parity, not a defect** — `pi/packages/ai/src/utils/deferred-tools.ts` populates `uniqueTools` by normalized name *before* the `if (!enabled)` early return. Do not file it. Residual system-prompt half is DRIFT-033. |
| DRIFT-002 | **closed** | `normalize_tool_call_id`. `cyrup-provider/src/api/openai_completions.rs:724-751` re-diffed clause-for-clause against `pi/packages/ai/src/api/openai-completions.ts:1006-1030`: pipe split, per-part sanitize, empty-`item_id` collapse, `<= 40` early return, 8-char hash, `Math.max(1, 40-len-1)` floor, openai 40-char truncation. Commit `168857d`. |
| DRIFT-003 | **closed** | `content_block_start` payload. Verified on the **production** path, not its tests: `cyrup-provider/src/api/anthropic_messages.rs:1531-1537` seeds `text`, `:1545-1560` seeds `thinking` **and** `signature`, each defaulting to `""` — identical to `pi/packages/ai/src/api/anthropic-messages.ts:588-604`. Commit `168857d`; upstream `59ad3dea` (#7358). |
| DRIFT-004 | **partially closed** | `user_bash` from RPC is done (`cyrup-modes/src/rpc.rs:1019-1058`); the `UserBashEventResult.operations` backend seam is not. Remainder open below. |
| DRIFT-005 | **closed** | Summarization isolation. `cyrup-session/src/compaction/summarize.rs:228-239` carries `cache_retention: None`, a fresh `session_id`, and `retry_assistant_call` — matching `pi/packages/coding-agent/src/core/compaction/compaction.ts:562-581` and the harness fork at `:129-130`. The single-choke-point claim was verified, not assumed: exactly two non-test `impl … Summarizer for` exist and both route through `complete_summarization`. Commit `bb301b6` (landed before anyone worked it). |
| DRIFT-006 | **closed** | Summarization retry observability. Four production `with_observer` sites (`cyrup-session-svc/src/session.rs:1278,:1547,:1856,:3722`); `compact.rs:49-61` forwards the observer into the retry loop rather than storing it. pi `243f64be` (abort reported as unsuccessful) is also already absorbed — `cyrup-provider/src/utils/retry.rs:190-215` matches `pi/packages/ai/src/utils/retry.ts:172-191` arm for arm. Commit `ace01cb`. |
| DRIFT-007 | **closed** | Runtime catalog overlay. `cyrup-provider/src/remote_catalog.rs` exists with `https://pi.dev` at `:68`, a 4h refresh interval at `:71`, and a complete ETag lifecycle (`:520` read, `:612` capture, `:637` persist, `:586` keep-across-transient-failure). Commit `289c089`. Overlay only — it does **not** close DRIFT-009's offline floor. |
| DRIFT-008 | **closed** | `ThinkingLevel::Max` exists at `cyrup-core/src/message.rs:30-39`, `Max` declared last so the ascending clamp works. Catalogs carry exactly **one** distinct `xhigh` mapping across all 105 occurrences (`"xhigh": "xhigh"`), so no xhigh→max mismapping remains; `"max": "max"` appears in 12 files. Commit `6d29542`. *(Evidence note: the prior filing's greps were written unspaced and return 0 — the JSON is pretty-printed. Use `grep -h -o '"xhigh": *"[a-z]*"'`.)* |
| DRIFT-009 | **open, was misdescribed** | The prior mechanism ("the source cyrup regenerates from no longer exists") is false — all 37 `*.models.ts` are still tracked. What left the tree is the **values**. Severity raised to medium. Restated below. |
| DRIFT-010 | open | `get_available_thinking_levels` RPC verb. |
| DRIFT-011 | **closed** | `usage` on tool results / compaction / branch summaries. `cyrup-core/src/message.rs:489-494`, `cyrup-session/src/entry.rs:64-71` and `:80-86`, `cyrup-ext/wit/world.wit:172`, all with the correct elision. Commits `f777e44` + `bb301b6`. Missing consumer is DRIFT-031. |
| DRIFT-012 | open | `StopReason::Pending` / `rawStopReason`. Upstream surface widened. |
| DRIFT-013 | open | Z.AI `max_completion_tokens`. |
| DRIFT-014 | open | DNS transport failures not retryable. **Prior fix instruction was wrong** — corrected below. |
| DRIFT-015 | open | Extension context surface. Upstream still widening. |
| DRIFT-016 | open | `Current date:` in the system prompt. |
| DRIFT-017 | open | Bash session env vars + stale-var scrub. |
| DRIFT-018 | open | Constrained sampling. |
| DRIFT-019 | open | Radius / Qwen Token Plan / `pi-messages`. |
| DRIFT-020 | open | `sendSessionIdHeader` vs `sessionAffinityFormat`. |
| DRIFT-021 | open | Streams without `finish_reason`. |
| DRIFT-022 | open (tracking) | Alternate-screen TUI mode. Do not implement yet. |
| DRIFT-023 | open (tracking) | `ModelRegistry` → `ModelRuntime`. |
| DRIFT-024 | open | AGENTS.md double-load in nested worktrees. |
| DRIFT-025 | open | `${@:-default}` prompt-template defaults. |
| ~~DRIFT-026~~ **CLOSED** `513e45a` | open | Google converter drops signed empty blocks. Still the only `high`. |
| DRIFT-027 | open | `deferredToolsMode: "kimi"`. |
| DRIFT-028 | open | OpenRouter cache breakpoints. **Strengthened** — `~anthropic/…` ids ship in cyrup's own catalog. |
| DRIFT-029 | open | Single bash cancel slot. |
| DRIFT-030 | open | `ANTHROPIC_AUTH_TOKEN`. |
| DRIFT-031 | open | Usage cost breakdown. |
| DRIFT-032 | open | llama.cpp router / HF model search. |
| DRIFT-033 | open | Mid-run tool addition never reaches the system prompt. **Fix sketch corrected** — the work is in `cyrup-session-svc`, not `cyrup-agent`. |
| DRIFT-034 | open | WIT world still `@0.1.0` after a deliberate ABI break. |
| DRIFT-035 | open | TEST DEFECT: prompt tests pin DRIFT-016. |
| DRIFT-036 | open | TEST DEFECT: `settle()` fixed 50 ms sleep. |
| DRIFT-037 | **new** | Compaction `retainedTail` unported, `firstKeptEntryId` still required. |
| DRIFT-038 | **new** | `resolve_scope` has no exact-reference short-circuit. |
| DRIFT-039 | **new** | TEST DEFECT: parallel-tool test asserts wall-clock and completion order. |
| DRIFT-040 | **new** | pi harness-v2 rearchitecture unabsorbed (tracking). |

Closed: 8. Partially closed: 1 (DRIFT-004, remainder open below).

## Open items

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| DRIFT-026 | high | upstream-drift | M | Google converter drops signed EMPTY text/thinking blocks, breaking the Gemini reasoning chain |
| DRIFT-009 | medium | upstream-drift | M | Embedded catalog floor pinned to pi@91585d9a with no in-tree regeneration source |
| DRIFT-012 | medium | upstream-drift | M | `StopReason::Pending` and `rawStopReason` missing |
| DRIFT-013 | medium | upstream-drift | S | Z.AI sent `max_completion_tokens`, which it ignores |
| DRIFT-014 | medium | upstream-drift | M | DNS transport failures not classified retryable |
| DRIFT-015 | medium | upstream-drift | L | Extension context surface drifted (scopedModels, outputPad, markdown, providers) |
| DRIFT-017 | medium | upstream-drift | M | Bash tool exposes no session env vars and does not scrub stale ones |
| DRIFT-028 | medium | upstream-drift | S | OpenRouter Anthropic cache breakpoints skip tool results and miss `~anthropic/*` aliases |
| DRIFT-029 | medium | upstream-drift | M | Concurrent user bash: single cancel slot makes abort miss and `is_bash_running` lie |
| DRIFT-030 | medium | upstream-drift | S | `ANTHROPIC_AUTH_TOKEN` bearer-token auth unsupported |
| DRIFT-033 | medium | port-divergence | M | A mid-run tool addition never reaches the system prompt |
| DRIFT-034 | medium | release-hygiene | S | Deliberate WIT ABI break in `f777e44` left the world version UNBUMPED at 0.1.0 |
| DRIFT-037 | medium | upstream-drift | M | Compaction entries: `retainedTail` unported and `firstKeptEntryId` still required |
| DRIFT-038 | medium | upstream-drift | S | `resolve_scope` has no exact-reference short-circuit, so an id containing `[`, `*` or `?` never matches itself |
| DRIFT-004 | medium | upstream-drift | M | RPC `bash`: `UserBashEventResult.operations` backend seam unported |
| DRIFT-010 | low | upstream-drift | S | `get_available_thinking_levels` RPC verb not implemented |
| DRIFT-016 | low | upstream-drift | S | `Current date:` still injected into the system prompt |
| DRIFT-018 | low | upstream-drift | L | Constrained sampling (strict JSON schema + Lark/regex grammars) absent |
| DRIFT-019 | low | upstream-drift | L | Radius + Qwen Token Plan providers and the `pi-messages` wire API missing |
| DRIFT-020 | low | upstream-drift | S | openai-responses affinity keys on removed `sendSessionIdHeader` |
| DRIFT-021 | low | upstream-drift | S | Cannot handle streams that never send a `finish_reason` |
| DRIFT-022 | low | tracking | L | Alternate-screen / fullscreen TUI mode not ported |
| DRIFT-023 | low | tracking | L | Model registry → ModelRuntime refactor not absorbed |
| DRIFT-024 | low | upstream-drift | S | AGENTS.md loaded twice in nested git worktrees |
| DRIFT-025 | low | upstream-drift | S | `${@:-default}` prompt-template defaults render literally |
| DRIFT-027 | low | upstream-drift | S | openai-completions has no `deferredToolsMode: "kimi"` |
| DRIFT-031 | low | upstream-drift | M | No usage cost breakdown — per-model attribution and `Tools/summaries` unsurfaced |
| DRIFT-032 | low | upstream-drift | L | llama.cpp router integration and Hugging Face model search entirely unported |
| DRIFT-035 | low | test-defect | S | Prompt tests assert the `Current date:` footer, pinning DRIFT-016 |
| DRIFT-036 | low | test-defect | S | `settle()` uses a fixed 50 ms sleep as the only synchronization |
| DRIFT-039 | low | test-defect | S | `a_02_2_parallel_completion_vs_source_order` asserts a wall-clock bound and a completion ORDER it cannot control |
| DRIFT-040 | low | tracking | L | pi's agent-harness v2 rearchitecture entirely unabsorbed |

## DRIFT-026 — Google converter drops signed EMPTY text/thinking blocks, breaking the Gemini reasoning chain

**Kind** upstream-drift · **Severity** high · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-provider/src/api/google_generative_ai.rs:708` `assistant_parts`. The `Content::Text` arm skips at `:716-718` (`if text.trim().is_empty() { continue; }`) **before** `resolve_thought_signature` is called on the very next line `:719`, so a signed empty text part is discarded together with its signature. The `Content::Thinking` arm skips at `:732-734`, before the `if same` branch at `:735` that would have resolved `thinking_signature`.

**upstream** — `pi/packages/ai/src/api/google-shared.ts:134` resolves the signature **first**, and `:139` skips only `if ((!block.text || block.text.trim() === "") && !thoughtSignature)`. The thinking arm mirrors it at `:147`/`:150`. The old unconditional skip is deliberately retained in the cross-provider `else` at `:158`. pi `6138f5a0` (#7362, 2026-07-31); its in-place comment names the symptom.

**Impact** — Gemini's reasoning chain breaks across turns: the model intermittently ends mid-task turns with a thought-only STOP — empty completion, no tool call — and the agent loop stalls with no error to explain it.

**Fix** — In `google_generative_ai.rs::assistant_parts`, hoist signature resolution above both empty-skips and gate each skip on the signature being absent, mirroring `google-shared.ts:134/139` and `:147/:150`. Keep the unconditional skip on the non-Google/cross-provider path.

**Verify** — Unit test in `google_generative_ai.rs`: an assistant message whose text block is `""` but carries a valid thought signature must serialize a part retaining the signature; the same block with no signature must still be dropped; repeat for `Content::Thinking`. No existing test pins the current behavior — the only signature assertions, `:1659-1661`, are on `is_valid_thought_signature`.

## DRIFT-009 — Embedded catalog floor pinned to pi@91585d9a with no in-tree regeneration source

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-provider/src/providers/catalog/*.json` (31 catalogs) were refreshed by `6d29542` to pi@`91585d9a` (2026-07-10). `cyrup-provider/src/catalog.rs:1-11` was rewritten by `289c089`/`6d29542` and no longer claims regeneration from `*.models.ts`; the stale provenance now survives only in `cyrup-provider/tests/catalog_data.rs:8-11,:96,:141-200`.

**upstream** — All 37 `pi/packages/ai/src/providers/*.models.ts` are still tracked (`git -C pi ls-files packages/ai/src/providers/ | grep -c 'models.ts'` → 37), but pi `a9f6a315` turned each into a wrapper — `pi/packages/ai/src/providers/anthropic.models.ts` is now 8 lines, `import values from "./data/anthropic.json" with { type: "json" }` + `flattenModelCatalog("anthropic", values)`, and `packages/ai/src/providers/data/` is `pi/.gitignore:11`. The values left the tree; the files did not. pi has re-run generate-models repeatedly since (`890b3547`, `21f92b31`, `3a40794e`, `e8d97fee`, `c23b58a6`, `a8228963`, `1ae06409`) and landed value changes cyrup cannot re-derive: `b889a0ce` GPT-5.6 pricing, `6a884b4a`/`8b937370`/`aba32450`/`c2c32feb` kimi-k3, `ce48d9b4` Copilot long-context pricing tiers, `35f12c8c` gpt-5.6 context window, `c1b7856e` removed xAI models.

**Impact** — Three weeks of catalog drift with no regeneration source. Wrong prices and context windows on `--offline` runs, on first launch before the DRIFT-007 overlay fetches, and whenever pi.dev is unreachable. Silent — a wrong price is never surfaced as an error.

**Fix** — Seed the embedded floor from the same pi.dev artifact `cyrup-provider/src/remote_catalog.rs` already fetches (the only source still available) as an explicit snapshot step, and repoint the provenance comment at `cyrup-provider/tests/catalog_data.rs:8-11` at that artifact instead of `*.models.ts`.

**Verify** — Snapshot the pi.dev artifact and diff it against the 31 embedded catalogs; the diff must be reproducible and the generating step recorded in `catalog.rs`'s header. Assert in `catalog_data.rs` that every embedded catalog carries the recorded source revision.

## DRIFT-012 — `StopReason::Pending` and `rawStopReason` missing

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-core/src/message.rs:93-100`: `StopReason` is exactly `Stop, Length, ToolUse, Error, Aborted` — five variants, no `Pending`. `grep -rn 'raw_stop_reason\|rawStopReason' crates --include=*.rs` → 0.

**upstream** — pi `f9a49869` added `pending`, then a per-API family: `926eb15c` (anthropic), `fe1c9b6d` (openai-completions), `e5ef8d06` (openai-responses), `23cb385b` (google), `5a53f086` (mistral), `637737ca` (bedrock — n/a to cyrup). Each decoder also retains the provider's raw reason.

**Impact** — A stream that terminates without a decidable reason is coerced into one of five buckets, most often `Stop`, so the loop treats an incomplete turn as complete. The raw provider reason is unavailable for diagnostics.

**Fix** — Add `Pending` to `StopReason` at `cyrup-core/src/message.rs:93-100` and a `raw_stop_reason: Option<String>` alongside it (elided when `None`); thread both through the five decoder sites in `cyrup-provider/src/api/{anthropic_messages,openai_completions,openai_responses,google_generative_ai,mistral}.rs` — one per upstream commit above.

**Verify** — Per-API unit tests feeding a terminal event with no/unknown finish reason: assert `StopReason::Pending` and that `raw_stop_reason` carries the wire string verbatim. Golden JSONL must elide both keys for existing fixtures so snapshots do not churn.

## DRIFT-013 — Z.AI sent `max_completion_tokens`, which it ignores

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-provider/src/api/compat.rs:312-317`: `use_max_tokens = base_url.contains("chutes.ai") || is_moonshot || is_cloudflare_ai_gateway || is_together || is_nvidia || is_ant_ling`. No `is_zai` — even though `is_zai` is defined 34 lines above at `:278-281` (`provider == "zai" || provider == "zai-coding-cn" || base_url.contains("api.z.ai") || base_url.contains("open.bigmodel.cn")`) and already used at `:304` and `:331`. Consumed at `:355` (`max_tokens_field`).

**upstream** — pi `2fe21b40` (#7174) adds the Z.AI family to the `max_tokens` branch of the same predicate in `pi/packages/ai/src/api/openai-completions.ts`.

**Impact** — Requests to Z.AI / `api.z.ai` / `open.bigmodel.cn` carry a token cap the provider ignores, so output length is effectively unbounded — overlong completions and unexpected cost.

**Fix** — Add `|| is_zai` to the `use_max_tokens` disjunction at `compat.rs:312-317`.

**Verify** — Unit test: a `zai` / `zai-coding-cn` model resolves `max_tokens_field == "max_tokens"`; an `openai` model still resolves `max_completion_tokens`. The existing assertions at `openai_completions.rs:2278`/`:2350` must stay green — confirmed they are on an `openai` model and do **not** pin the bug.

## DRIFT-014 — DNS transport failures not classified retryable

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** medium

**cyrup** — `cyrup/crates/cyrup-provider/src/utils/retry.rs:34-74` `RETRYABLE_PROVIDER_PATTERNS`. Exactly seven literals are missing versus upstream, independently re-derived: `"524"`, `"getaddrinfo"`, `"ENOTFOUND"`, `"EAI_AGAIN"`, `"socket connection was closed"`, `"stream ended before a terminal response event"`, `"ResourceExhausted"`. cyrup already carries the three retry-guidance literals and `"http2 request did not get a response"`.

**upstream** — `pi/packages/ai/src/utils/retry.ts:26-88` `RETRYABLE_PROVIDER_ERROR_PATTERN`.

**Impact** — A transient DNS failure or an upstream 524 aborts the turn instead of retrying, surfacing as a hard error mid-run.

**Fix** — **Touch only `retry.rs:34-74`.** The prior filing's instruction to add these to "both lists", citing `cyrup-ext-subagents/src/exec/fallback.rs:370`, is wrong and must not be followed: `fallback.rs:345-347` is `RETRYABLE_MODEL_FAILURE_PATTERNS`, whose own doc comment cites `model-fallback.ts:98-133` and which is a structural port of a *different* upstream — `pi-subagents/src/runs/shared/model-fallback.ts:278ff`, whose entries (`quota`, `billing`, `credit`, `unauthori[sz]ed`, `model.*disabled`) do not overlap pi-core's. Injecting pi-core literals there would be an unprompted divergence from pi-subagents. Separately, the literals alone are **not sufficient**: pi's are Node-shaped (`getaddrinfo`, `ENOTFOUND`), and `grep -rn 'getaddrinfo\|dns error\|failed to lookup' crates --include=*.rs` → 0 — what string reqwest/hyper actually emits on a failed lookup is undetermined. Determine it first, then add the Rust-side literal alongside pi's.

**Verify** — Unit test over `RETRYABLE_PROVIDER_PATTERNS` asserting each of the seven upstream literals classifies retryable, **plus** a test using the actual reqwest/hyper DNS error string. Do not close on the literals alone.

## DRIFT-015 — Extension context surface drifted (scopedModels, outputPad, markdown, providers)

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-ext/wit/world.wit` carries no `render-options` record, no `scoped-models`, no markdown-transformer registration and no native `register-provider` overload. `1d87913` made the extension seam live but did not widen it here; the residual it left — re-invoking the guest renderer on expand/collapse — is unchanged.

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:326` (`scopedModels`), `:1139` (`outputPad`), `:1148` (`MarkdownTransformer`), `:1287` (`registerMarkdownTransformer`). Still widening: `04b15259` (#7191 expose `ctx.scopedModels`), `b6fb91e5` (#7215 scoped models in the TUI extension context), `a3ee1d28` (#7032 expose UNAVAILABLE scoped models), `518855dd` (#7045 `outputPad` to custom renderers), `714978bf` (#7231 "Markdown api" — width-aware transformers across `extensions/{index,loader,runner,types}.ts` plus a new `modes/interactive/components/markdown-transform.ts`), `019e4ad6` (accept native extension providers), `ea781d68` (#7369 forward `autocompleteMaxVisible` to custom editors).

**Impact** — Extensions written against current pi cannot be ported: they cannot read scoped models, cannot control output padding, cannot transform markdown, and native extensions cannot register providers.

**Fix** — Widen `cyrup-ext/wit/world.wit` (and the byte-identical `cyrup-ext-sdk/wit/world.wit`) with a `render-options` record carrying `output-pad` and width, a `scoped-models` accessor on the context, a `register-markdown-transformer` host call plus its guest export, and a native `register-provider` path in `cyrup-ext/src/native`. Bundle with DRIFT-034 — this breaks the world again, so bump the package version once.

**Verify** — A fixture guest component exercising each new call must instantiate and round-trip; assert the two `world.wit` copies stay byte-identical.

## DRIFT-017 — Bash tool exposes no session env vars and does not scrub stale ones

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tools/src/tools/bash.rs`: `grep -n 'spawn_hook\|SESSION_ID\|expose_session'` returns exactly one hit, `:111` (`match &self.opts.spawn_hook`). cyrup has pi's `spawnHook` half and neither the five-var set nor the scrub.

**upstream** — pi `bb3d7d39` (#6967, 2026-07-22), `resolveSpawnContext` at `pi/packages/coding-agent/src/core/tools/bash.ts:157-183`: it unconditionally `delete`s `PI_SESSION_ID`, `PI_SESSION_FILE`, `PI_PROVIDER`, `PI_MODEL`, `PI_REASONING_LEVEL` from the inherited env, then re-sets them from ctx only when `exposeSessionEnvironment` — documented "Default: true" at `bash.ts:193`. The same commit added `pi/packages/coding-agent/docs/environment-variables.md` (88 lines).

**Impact** — Scripts run by the agent cannot discover the session they belong to; worse, a **stale inherited** `CYRUP_SESSION_ID`/`PI_SESSION_ID` from an ancestor process passes through unchanged, so a child attributes work to the wrong session. `9b3afd7` confirmed subagent children spawn descendants, so that stale id has more paths to travel.

**Fix** — In `bash.rs`, **before** applying `spawn_hook` (i.e. before `:111`, so a hook can still override), remove the five `CYRUP_*`/`PI_*` session vars from the child env, then set them from the session when a new `expose_session_environment` option (default true) is on. The scrub is the load-bearing half — ship it even if the expose half is deferred.

**Verify** — Test that a bash child with a poisoned `CYRUP_SESSION_ID` in the parent env sees either the current session's id or no id at all, never the poisoned one; and that with the option off all five are absent.

## DRIFT-028 — OpenRouter Anthropic cache breakpoints skip tool results and miss `~anthropic/*` aliases

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — (a) `cyrup/crates/cyrup-provider/src/api/openai_completions.rs:614-624` `add_cache_control_to_last_conversation_message` accepts only `role == Some("user") || role == Some("assistant")` at `:617`. (b) `cyrup/crates/cyrup-provider/src/api/compat.rs:323` gates the detector on `provider == "openrouter" && id.starts_with("anthropic/")` — yet `crates/cyrup-provider/src/providers/catalog/openrouter.json` ships four `~anthropic/…` ids, so the alias miss is reachable from cyrup's own embedded catalog, not hypothetical.

**upstream** — `pi/packages/ai/src/api/openai-completions.ts:922` and its sibling `addCacheControlToMessage` at `:953` both accept `"user" || "assistant" || "tool"`. The alias widening is **build-time**: `pi/packages/ai/scripts/generate-models.ts:626` uses `/^~?anthropic\//`; pi's runtime detector at `openai-completions.ts:1443` is also `startsWith("anthropic/")`.

**Impact** — Cost-only but continuous: in an agent loop the last message is almost always a tool result, so the cache breakpoint lands one message too early on every turn, and `~anthropic/*` models get no breakpoint at all.

**Fix** — Asymmetric, and the fix must say so. Add `|| role == Some("tool")` at `openai_completions.rs:617` (straight runtime parity). For the alias half, widen `compat.rs:323` to `^~?anthropic/` — a deliberate small divergence in *placement*, since cyrup has no build-time generator; note it inline as a `[CYRUP-DELTA]`.

**Verify** — Test that a conversation ending in a tool result gets `cache_control` on that message; and that an `openrouter` model id beginning `~anthropic/` (take one verbatim from `openrouter.json`) resolves the anthropic cache-control format.

## DRIFT-029 — Concurrent user bash: single cancel slot makes abort miss and `is_bash_running` lie

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/session.rs:379` `bash_cancel: Mutex<Option<CancelToken>>` — one slot, initialized `:481`; `:3917` overwrites any in-flight token; `:3940` and `:3977` both clear it unconditionally; `abort_bash` reads that one slot at `:4070`; `is_bash_running` at `:4077`. The RPC dispatcher calls in with no serialization (`cyrup-modes/src/rpc.rs:1043`), so two concurrent user bash calls need no exotic setup.

**upstream** — pi `2efa728d` (#7103, 2026-07-27) replaced the single controller with a Set: `pi/packages/coding-agent/src/core/agent-session.ts:337` declaration, `:2771` add, `:2795` delete-in-finally, `:2832-2836` iterate a copy, `:2839-2841` `size > 0`.

**Impact** — With two bash commands in flight, abort cancels only the most recent — the first is orphaned and keeps running — and the first to finish clears the slot, so `is_bash_running` reports false while a command is still executing.

**Fix** — Replace `bash_cancel` with a keyed set (`Mutex<HashMap<BashId, CancelToken>>`): insert at `:3917`, remove only the *own* key in the two completion paths `:3940`/`:3977`, iterate a snapshot in `abort_bash` at `:4070`, and make `is_bash_running` a non-empty check at `:4077`.

**Verify** — Test starting two long-running bash calls then `abort_bash`: both must terminate; and after the shorter one completes, `is_bash_running` must still be true while the longer one runs.

## DRIFT-030 — `ANTHROPIC_AUTH_TOKEN` bearer-token auth unsupported

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `grep -rn ANTHROPIC_AUTH_TOKEN crates` → 0. `cyrup/crates/cyrup-config/src/env_keys.rs:31-34` returns exactly `&["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]` for `provider == "anthropic"`; `cyrup-provider/src/env_api_keys.rs:38-39` mirrors it.

**upstream** — pi `24e5cc04` (#6148, fixes #5871): `pi/packages/ai/src/auth/env-api-keys.ts:29` defines the constant, `:76` returns it first for anthropic, `:147` excludes it from the api-key pick; `pi/packages/ai/src/providers/anthropic.ts:21-26` resolves it to an `Authorization: Bearer` header.

**Impact** — Users behind an Anthropic-compatible gateway that issues bearer tokens — the common enterprise proxy setup — cannot authenticate with cyrup at all.

**Fix** — Add `ANTHROPIC_AUTH_TOKEN` first in the anthropic list at `env_keys.rs:31-34` and `env_api_keys.rs:38-39`, exclude it from the `x-api-key` pick, and resolve it into `ResolvedAuth.headers` (`cyrup-provider/src/auth/types.rs:53` — the mechanism already exists) as `Authorization: Bearer <token>`.

**Verify** — Test that with only `ANTHROPIC_AUTH_TOKEN` set the anthropic request carries `Authorization: Bearer …` and no `x-api-key`; with both set, the bearer token wins and `x-api-key` is absent.

## DRIFT-033 — A mid-run tool addition never reaches the system prompt

**Kind** port-divergence · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/hooks.rs:170-180` `prepare_next_turn` sets only `update.tools`, never `update.system_prompt`. The consumer side is already wired: `cyrup/crates/cyrup-agent/src/agent.rs:560-562` reads `if let Some(prompt) = u.system_prompt { self.system_prompt = prompt; }`. The reason is stated in place at `hooks.rs:158-160`: cyrup has one prompt slot instead of pi's override/base pair, so re-pushing would undo a `before_agent_start` sanitization mid-run.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:519-540` re-pushes `context.systemPrompt` at every turn boundary — cited by `hooks.rs:150-152` itself.

**Impact** — A tool registered mid-run becomes callable (DRIFT-001 closed that) but is never described to the model, so the model has no reason to call it until the next run.

**Fix** — The remaining work is entirely in `cyrup-session-svc`, **not** `cyrup-agent`. (a) Split cyrup's single prompt slot into base + override so a re-push cannot undo a mid-run sanitization; (b) set `update.system_prompt` at `hooks.rs:179`. Until then, promote the `hooks.rs:158-160` comment into a documented `[CYRUP-DELTA]` in `cyrup-session-svc/src/lib.rs` — the divergence currently lives only in a private doc comment on a non-public method.

**Verify** — Test registering a tool mid-run and asserting the next turn's system prompt names it, while a `before_agent_start` sanitization applied earlier in the same run survives that turn boundary.

## DRIFT-034 — Deliberate WIT ABI break in `f777e44` left the world version UNBUMPED at 0.1.0

**Kind** release-hygiene · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-ext/wit/world.wit:172` now declares `on-tool-result: func(call-id, name, input-json, content-json, is-error, details-json, usage-json: option<string>) -> hook-outcome`. Both `cyrup-ext/wit/world.wit:10` and `cyrup-ext-sdk/wit/world.wit:10` still say `package cyrup:ext@0.1.0;`. `diff -q` between the two copies is clean — a byte-identity invariant maintained by hand, with no test enforcing it.

**upstream** — n/a: this seam is a documented cyrup divergence (pi loads TypeScript via `jiti`; cyrup uses the WASM Component Model). The break was introduced by cyrup commit `f777e44`, which records it only in its message.

**Impact** — A guest component built against the old world fails to instantiate, with the same version string on both sides so nothing explains why. Extension authors see an opaque instantiation error.

**Fix** — Bump both `world.wit` copies to `cyrup:ext@0.2.0`, record the break in `cyrup-ext/src/lib.rs`'s doc header, and add an `include_str!`-based byte-identity assertion between the two files. Bundle with DRIFT-015, which will break the world again — bump once.

**Verify** — The identity test fails if either file is edited alone; a fixture guest built against `@0.1.0` is rejected with a version-mismatch message naming both versions.

## DRIFT-037 — Compaction entries: `retainedTail` unported and `firstKeptEntryId` still required, so a pi-harness-written compaction silently degrades to `Entry::Unknown`

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/entry.rs:57-73`: `KnownEntry::Compaction { base, summary, first_kept_entry_id: EntryId, tokens_before, details?, usage?, from_hook? }`. `first_kept_entry_id` at `:60` is a bare `EntryId` — the only field in the variant with no `#[serde(default)]`/`skip_serializing_if`, so it is **required** on the wire. There is no `retained_tail` field (`grep -rn 'retained_tail\|retainedTail' crates --include=*.rs` → 0 workspace-wide). The failure is silent by construction: `entry.rs:170-175` documents `Entry::Unknown` as the fallback for a known-tag-but-unparseable body. There is no `deny_unknown_fields`, so a line carrying both keys parses and then drops `retainedTail` on re-serialization.

**upstream** — pi `9e7582aa` (#6594, 2026-07-21, sqlite session storage). `pi/packages/agent/src/harness/types.ts:403-412`: `firstKeptEntryId?: string` (now optional) and `retainedTail?: AgentMessage[]`. `pi/packages/agent/src/harness/session/session.ts:456-478` `appendCompaction(summary, firstKeptEntryId, tokensBefore, details?, fromHook?, usage?, retainedTail?)` writes both; `:243` `getPathToRootOrCompaction` (renamed from `getPathToRoot` by the same commit) stops the walk at `:253-254` — `if (current.type === "compaction") { if (current.retainedTail) break; stopAtEntryId = current.firstKeptEntryId ?? null; }` — the `?? null` showing upstream explicitly tolerates absence. Declared canonical in `pi/packages/coding-agent/docs/session-format.md:240` (a verbatim example with `retainedTail` and no `firstKeptEntryId`), `:245`, `:327`, `:342`.

**Impact** — Two losses on the R-00-013 interop contract. (1) A compaction written without `firstKeptEntryId` never deserializes as a known entry, so its summary *and* its retained tail never reach the context rebuild — the session resumes with the whole pre-compaction prefix silently absent, no error, no warning. (2) An entry that does parse drops `retainedTail` on re-export, rewriting it into a form pi's own reader then handles differently. Bounded to medium because pi's **live** coding-agent path has not moved — `pi/packages/coding-agent/src/core/session-manager.ts:69-71` still declares `firstKeptEntryId: string` required and `grep -rn retainedTail packages/coding-agent/src` returns nothing — but the format doc already calls the new shape canonical, and harness-v2 (DRIFT-040) is where pi is investing.

**Fix** — In `entry.rs:60` make `first_kept_entry_id` `Option<EntryId>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`, and add `retained_tail: Option<Vec<AgentMessage>>` with the same elision so legacy entries stay byte-identical. Teach the context-rebuild walk pi's `getPathToRootOrCompaction` rule: on a compaction carrying `retained_tail`, treat it as a self-contained checkpoint — emit summary + tail and stop — otherwise fall back to the `first_kept_entry_id` walk, mirroring pi's `?? null` tolerance when that is absent too.

**Verify** — Extend `cyrup-test-support`'s `interop.rs` (R-00-013) with the pi-shaped line copied verbatim from `session-format.md:240` (`retainedTail` present, no `firstKeptEntryId`): assert it parses as `KnownEntry::Compaction` and not `Entry::Unknown`, that re-export is byte-identical, and that a context rebuild yields exactly the summary plus the tail and nothing older. A second case with both keys present must round-trip both.

## DRIFT-038 — `resolve_scope` has no exact-reference short-circuit, so a model id containing `[`, `*` or `?` never matches itself

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-config/src/model.rs:236-282` `resolve_scope`. The glob branch at `:257` fires on `pattern.contains('*') || pattern.contains('?') || pattern.contains('[')`, strips an optional `:level` suffix at `:259-267`, then goes straight to the filter at `:268-273` (`glob_match(glob_pattern, &format!("{}/{}", m.provider, m.id)) || glob_match(glob_pattern, m.id.as_str())`). No exact-match attempt first — `grep -rn 'find_exact_model_reference' crates` → 0. The non-glob `else` branch at `:274-280`, which does exact parsing via `self.parse_pattern`, is structurally unreachable for such an id. cyrup's own comment at `:257` cites `model-resolver.ts:264`, i.e. the port is anchored to the pre-fix revision.

**upstream** — pi `da8dd872` (2026-07-23, "fix(coding-agent): resolve bracketed scoped model ids as literals", fixes #6210). `pi/packages/coding-agent/src/core/model-resolver.ts:297-303` inserts, after the same `:level` strip (`:285-295`) and before the minimatch filter (`:307-310`), a `findExactModelReferenceMatch(globPattern, availableModels)` short-circuit with `continue`. The helper at `:79-115` trims, lowercases, compares canonical `provider/id`, returns the model on exactly one match and `undefined` on more than one (so ambiguity still globs), then falls back to a slash-split provider/modelId compare with the same rule.

**Impact** — A model whose id literally contains a glob metacharacter is read as a pattern and never matches itself, so naming it in a model scope resolves to **zero** models — silently, with no error. This is pi issue #6210. It reaches two cyrup surfaces that both landed in this batch: subagent `modelScope` (SUBA-003, `46c3868`) and settings-declared models (CFG-002, `51bb11a`), so the blast radius grew at the same time as the gap.

**Fix** — Port `findExactModelReferenceMatch` into `model.rs` as a private fn: lowercase-compare `format!("{}/{}", m.provider, m.id)` against the trimmed reference, return `None` on more than one hit so ambiguity falls through to globbing, then the slash-split fallback. Call it inside the glob branch at `model.rs:268`, immediately after the `:level` strip and before the filter, `continue`-ing on a hit and routing through the existing push closure so the `seen` de-dup still applies. Run it on `glob_pattern` (suffix already stripped), matching pi's ordering exactly.

**Verify** — Unit tests in `model.rs`: an available model whose id contains brackets resolves to exactly itself; the same with a `:high` suffix resolves with `thinking_level: High`; `*sonnet*` still fans out to every sonnet; and two available models sharing a canonical id fall through to the glob path rather than silently picking one.

## DRIFT-004 — RPC `bash`: `UserBashEventResult.operations` backend seam unported

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — The `user_bash` half is closed and re-verified: `cyrup/crates/cyrup-modes/src/rpc.rs:1019-1058`, `SessionCommand::Bash` calls `session.execute_bash_with_user_event(&command, BashOptions { exclude_from_context, id: bash_id }, None)` at `:1043-1050`, with the string-or-number RPC id threaded at `:1032-1034` and the carve-out documented at `:1041-1042`. The third argument is not a disguised seam — `execute_bash_with_user_event` (`cyrup-session-svc/src/session.rs:4001-4012`) types it as `crate::bash::BashChunkSink`, an output sink. There is no Rust trait or type named `BashOperations`; the ten grep hits for that name (`cyrup-tools/src/ops/local.rs:6,8,467,790`; `cyrup-session-svc/src/bash.rs:73`, `session.rs:3932`; `cyrup-tools/src/ops/shell.rs:88`; `tests/round3.rs:186,290`) are all doc comments citing pi's `createLocalBashOperations`. *(The prior filing's `grep … → 0` was wrong; the substantive claim survives.)*

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:576` passes `operations: eventResult?.operations`; the option is `BashToolOptions.operations?: BashOperations` at `pi/packages/coding-agent/src/core/tools/bash.ts:186-187`.

**Impact** — An extension can block or replace a bash **result** but cannot supply the backend the command executes **through**, so sandboxed, remote or recorded bash backends supplied per call are impossible.

**Fix** — Introduce a `BashOperations` trait in `cyrup-tools/src/ops/` matching pi's `createLocalBashOperations` surface (the doc comments already name it), make the local implementation the default, add an optional per-call override to `BashOptions` in `cyrup-session-svc/src/bash.rs`, and populate it from the user-bash event result at `cyrup-modes/src/rpc.rs:1043-1050`.

**Verify** — Test that an extension returning a custom `operations` from the user-bash event has its backend used for that call and not for the next; and that with no override the local backend still runs.

## DRIFT-010 — `get_available_thinking_levels` RPC verb not implemented

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `grep -rn 'GetAvailableThinkingLevels\|get_available_thinking_levels' crates` → 0. The producer exists but is internal-only: `available_thinking_levels()` in `cyrup/crates/cyrup-session-svc/src/session.rs` is consumed solely by `clamp_thinking_level`.

**upstream** — pi `c1793952` (#6865); `pi/packages/coding-agent/src/modes/rpc/rpc-types.ts:39,:161` and `pi/docs/rpc.md:316-328`.

**Impact** — An RPC embedder cannot discover which thinking levels the current model supports, so it must guess and let the server clamp silently.

**Fix** — Add the variant to the command enum in `cyrup-session-svc/src/command.rs` and a dispatch arm in `cyrup-modes/src/rpc.rs` beside the `AbortRetry`/`Bash` arms at `:1010-1058`, returning `session.available_thinking_levels()`.

**Verify** — RPC round-trip test asserting the response matches `available_thinking_levels()` for a model with a restricted ladder and for one with the full set.

## DRIFT-016 — `Current date:` still injected into the system prompt

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/builder.rs:333-337`, the format string at `:335`: `write!(out, "\n\nCurrent date: {:04}-{:02}-{:02}", d.year(), u8::from(d.month()), …)`.

**upstream** — pi `f4e9ca74` ("remove current date from system prompt", 2026-07-14, fixes #6621); `grep -n "Current date" pi/packages/coding-agent/src/core/system-prompt.ts` → nothing at HEAD.

**Impact** — The system prompt differs from pi's byte-for-byte, and the date changes daily, so prompt-cache prefixes break across midnight.

**Fix** — Delete the write at `builder.rs:333-337`, and the two assertions in DRIFT-035 with it.

**Verify** — Prompt golden snapshots contain no `Current date:`; the same session rendered on two different simulated dates produces identical prompts.

## DRIFT-018 — Constrained sampling (strict JSON schema + Lark/regex grammars) absent

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** high

**cyrup** — `grep -rn 'constrained_sampling\|ConstrainedSampling\|GrammarFormat' crates --include=*.rs` → 0.

**upstream** — pi `24bace27` (#6341) added `pi/packages/ai/src/api/constrained-sampling.ts`, plus follow-up `34239180`.

**Impact** — Callers cannot force schema- or grammar-constrained output; structured-output use cases fall back to prompt-and-parse.

**Fix** — Port `constrained-sampling.ts` to `cyrup-provider/src/api/constrained_sampling.rs` and thread the option through `StreamOptions` to the APIs that support it.

**Verify** — Per-API tests asserting the constraint payload is emitted in the provider's expected shape and omitted for providers that do not support it.

## DRIFT-019 — Radius + Qwen Token Plan providers and the `pi-messages` wire API missing

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** medium

**cyrup** — `cyrup/crates/cyrup-provider/src/providers/fleet.rs:43-60` lists 16 openai-completions members, none of them `radius`, `qwen-token-plan` or `qwen-token-plan-cn`; there is no `pi_messages.rs` under `cyrup-provider/src/api/`.

**upstream** — pi `bbb91fa8` (#6858 Qwen Token Plan built-in), `961fa6c1` (Radius gateway), `a9f5b1c1` (Radius OAuth through the gateway), `4c1a0b92` (Qwen thinking controls for token-plan reasoning models).

**Impact** — Those providers are unusable from cyrup. Consistent with the documented provider-coverage divergence, but the gap is growing.

**Fix** — Add the fleet members and their catalogs; `pi-messages` needs a new module under `cyrup-provider/src/api/` plus registration in `api/mod.rs`. Radius OAuth is blocked on the documented absence of any OAuth flow in cyrup.

**Verify** — `providers/all.rs`'s port-status table updated; a catalog present for each new provider; a faux round-trip per new wire API.

## DRIFT-020 — openai-responses affinity keys on removed `sendSessionIdHeader`

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-provider/src/api/compat.rs:152` declares `send_session_id_header: Option<bool>` on `ModelCompat`, `:175` the resolved bool, `:188` `c.and_then(|c| c.send_session_id_header).unwrap_or(true)`; sole consumer `cyrup-provider/src/api/openai_responses.rs:427`. `grep -rn 'session_affinity_format\|sessionAffinityFormat' crates` → 0.

**upstream** — pi `298665cf` (#6496, "support OpenRouter session affinity") replaced the boolean with a format enum.

**Impact** — OpenRouter session affinity cannot be expressed; and with DRIFT-007 now ingesting pi-shaped catalog JSON at runtime, an unknown `sessionAffinityFormat` key is dropped silently, with no regeneration step where a human would notice.

**Fix** — Replace `send_session_id_header` with `session_affinity_format` through the declare/resolve/consume chain (`compat.rs:152,:175,:188` → `openai_responses.rs:427`), keeping the boolean as a deprecated alias mapping onto the enum. No test pins the current shape, so nothing must be unwound.

**Verify** — Catalog entries carrying `sessionAffinityFormat` resolve it; a legacy `sendSessionIdHeader: false` still suppresses the header.

## DRIFT-021 — Cannot handle streams that never send a `finish_reason`

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `grep -rn 'supports_finish_reason' crates --include=*.rs` → 0. The sibling field it should mirror, `supports_usage_in_streaming`, is present through the full declare/resolve/detect chain in `cyrup/crates/cyrup-provider/src/api/compat.rs`.

**upstream** — pi `2c304124` ("support streams without finish reasons"), `pi/packages/ai/src/api/openai-completions.ts`.

**Impact** — Against an endpoint that never emits a finish reason, the turn either hangs or is coerced into a wrong stop reason. Interacts with DRIFT-012: without `Pending` there is no honest representation either.

**Fix** — Add `supports_finish_reason` to `ModelCompat`/`ResolvedCompat` in `compat.rs`, mirroring `supports_usage_in_streaming` exactly, and consume it in `openai_completions.rs` to close the stream on `[DONE]` when the flag is false.

**Verify** — Faux test replaying a stream with no `finish_reason`: with the flag false the turn completes cleanly; with it true, existing behavior is unchanged.

## DRIFT-022 — Alternate-screen / fullscreen TUI mode not ported

**Kind** tracking · **Severity** low · **Effort** L · **Confidence** high

**cyrup** — `grep -rn 'AlternateScreen' crates --include=*.rs` returns four hits, all in `cyrup/crates/cyrup-tui/src/startup_selector.rs` (`:20` import, `:44` enter, `:52`/`:62` leave) — the one-shot startup session picker, **not** a UI mode. Recorded so nobody mistakes it for the feature. `grep -rn 'ui_mode\|ui-mode' crates/cyrup/src/cli.rs` → 0: no `--alt`/`--ui-mode` flag and no alternate-screen transcript viewport.

**upstream** — pi `f074efd9` (ui mode setting), `c13ffe18` (alternate-screen renderer), `ea1e77e2` (alternate-screen viewport layouts), `3c717842` (fullscreen transcript navigation), `8ac92f83` (draggable transient scrollbars), `6129a353` (configurable fullscreen scrollbars), `b3ed27b3` (scrollbar thumb theme color).

**Impact** — cyrup has only the inline viewport; no fullscreen transcript navigation. Feature absence, not a defect.

**Fix** — Do not implement yet — the upstream surface is still moving. When taken, it is a `cyrup-tui` renderer mode plus a `--ui-mode` flag in `crates/cyrup/src/cli.rs`.

**Verify** — n/a while tracking.

## DRIFT-023 — Model registry → ModelRuntime refactor not absorbed

**Kind** tracking · **Severity** low · **Effort** L · **Confidence** high

**cyrup** — `ModelRegistry` appears across `cyrup/crates/cyrup-provider`; there is no cyrup `ModelRuntime` type.

**upstream** — pi `9993c969`. Both `model-registry.ts` and `model-runtime.ts` exist in `pi/packages/coding-agent/src/core/` at HEAD, so the migration is in progress, not finished. Upstream keeps building on it: `ab366ebe` (#7325 custom compaction through the provider via the new model runtime), `bd9e09db` (expose dynamic provider refresh).

**Impact** — Divergence compounds: DRIFT-007 and `51bb11a` each bolted a payload onto the old registry, so a later port has more to unwind.

**Fix** — Deferred by design; re-scope once pi finishes the migration and deletes `model-registry.ts`. Related in kind to DRIFT-040 — both are "pi is mid-refactor, track don't port".

**Verify** — n/a while tracking.

## DRIFT-024 — AGENTS.md loaded twice in nested git worktrees

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/context_files.rs` de-duplicates only by exact path — `seen.insert(cf.path.clone())` at `:100` and `:118`, nowhere else — and `grep -c worktree` on the file returns 0, so nothing detects a worktree root shadowing its main repo root.

**upstream** — pi `cced6a21` (#7221) added `findShadowedContextFile`.

**Impact** — In a nested worktree the same guidance is injected twice, wasting context and doubling any instruction weight.

**Fix** — Port `findShadowedContextFile` into `context_files.rs`: before the `seen` insert at `:100`/`:118`, drop a candidate whose path is shadowed by a nearer worktree root.

**Verify** — Test with a worktree nested inside its main repo, both carrying AGENTS.md: exactly one is loaded, the nearer one.

## DRIFT-025 — `${@:-default}` prompt-template defaults render literally

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-resources/src/prompt.rs:243-246`: the `:-` branch requires `num` to be non-empty and all-ASCII-digits, so `${@:-none}` (where `num` is `@`) falls through; the `${@:N}` branch then rejects it via the digits-only `start_str` guard at `:262-264`, returning `None`.

**upstream** — pi `64f83c85` ("support all-argument prompt defaults", closes #6695), `pi/packages/coding-agent/src/core/prompt-templates.ts:70-80`.

**Impact** — A template written for pi renders `${@:-none}` literally into the prompt instead of substituting the default.

**Fix** — Allow `num == "@"` in the `:-` branch at `prompt.rs:243-246`, substituting all arguments when present and the default when absent.

**Verify** — Unit tests: `${@:-none}` with no args yields `none`, with args yields the joined args; `${1:-x}` unchanged.

## DRIFT-027 — openai-completions has no `deferredToolsMode: "kimi"`

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `grep -rn 'deferred_tools_mode\|deferredToolsMode' crates` → 0. Neither `ModelCompat` (`cyrup/crates/cyrup-provider/src/api/compat.rs:73`) nor `ResolvedCompat` (`:205`) has the field, so a catalog entry carrying it is silently dropped.

**upstream** — pi `f16b4e0c` + `70c57632`; `pi/packages/ai/src/api/openai-completions.ts:157` lists `deferredToolsMode` in the detected-compat key union alongside `cacheControlFormat`, and `:1517` shows the declared-vs-detected precedence the sibling field uses.

**Impact** — Kimi models get the default deferred-tools rendering, so tool-heavy prompts are shaped wrongly for that provider.

**Fix** — Add `deferred_tools_mode` to `ModelCompat`/`ResolvedCompat` following the `cacheControlFormat` pattern exactly (declare, resolve, detect, precedence), and branch on it in `cyrup-provider/src/utils/deferred_tools.rs`.

**Verify** — Test that a catalog entry declaring `deferredToolsMode: "kimi"` survives resolution and changes the rendered tool payload; default mode unchanged.

## DRIFT-031 — No usage cost breakdown — per-model attribution and `Tools/summaries` unsurfaced

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `grep -rn 'cost_breakdown\|CostBreakdown\|usage_cost' crates --include=*.rs` → 0. `cyrup/crates/cyrup-tui/src/app.rs:2403-2419` (`C::SessionInfo`) builds a seven-row markdown table from `session.session_stats()` — messages / user / assistant / tool results / input tokens / output tokens / cache tokens. No cost figure, no per-model rows.

**upstream** — pi `2fd38684` (#6671), `getUsageCostBreakdown` at `pi/packages/coding-agent/src/core/usage-totals.ts:37-71`, consumed at `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:5732`.

**Impact** — Users cannot see what a session cost, nor which model or which tool/summary traffic drove it.

**Fix** — DRIFT-011 already supplies the data (`usage` on tool results, compaction and branch summaries). Port `getUsageCostBreakdown` into `cyrup-session-svc` and widen the `SessionInfo` panel at `app.rs:2403-2419` with per-model rows and a `Tools/summaries` bucket. Deferred explicitly in `f777e44`'s message.

**Verify** — Test a session using two models and containing a compaction: the breakdown attributes tokens per model, the compaction's usage lands in the summaries bucket, and totals match `session_stats()`.

## DRIFT-032 — llama.cpp router integration and Hugging Face model search entirely unported

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** medium

**cyrup** — `grep -rni llama crates --include=*.rs -l` → 5 incidental files, all comments or unrelated model-name strings.

**upstream** — pi `f1a466b1` (2026-07-17) plus follow-ons `3da591ab`, `864b35c4`, `6abb4b06`, `2a2b0a39` (#7072), `0c32e83a` (#7258).

**Impact** — No local-model workflow: cyrup cannot drive a llama.cpp router or discover models on Hugging Face.

**Fix** — Large, self-contained feature port into `cyrup-provider` plus a discovery surface. Defer until the provider-coverage items above are settled.

**Verify** — n/a until scoped.

## DRIFT-035 — TEST DEFECT: prompt tests assert the `Current date:` footer, pinning DRIFT-016

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/tests.rs:82` — `assert!(out.contains("Current date: 2026-06-28"), "date footer")` in `a06_1` — and `:113` — `assert!(out.contains("Current date: 2026-06-28"), "footer kept")` in `a06_2_custom_prompt_keeps_tail`. Both defend the write site at `cyrup-session/src/prompt/builder.rs:335`.

**upstream** — pi removed the footer in `f4e9ca74`; `pi/packages/coding-agent/src/core/system-prompt.ts` has no such string at HEAD.

**Impact** — Two green assertions actively defend known drift. Neither assertion message signals whether it encodes parity or pinned debt, so the next reader will assume parity and leave it.

**Fix** — Annotate both assertions with `// DRIFT-016: pins known drift — delete with builder.rs:335` **now**, even though DRIFT-016 itself is deferred; delete both when DRIFT-016 lands.

**Verify** — Deleting the footer in `builder.rs` fails exactly these two assertions and nothing else.

## DRIFT-036 — TEST DEFECT: `settle()` uses a fixed 50 ms sleep as the only synchronization

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/tests/summarization_retry_events.rs:97-103`: `async fn settle()` is `for _ in 0..10 { tokio::task::yield_now().await }` then `tokio::time::sleep(Duration::from_millis(50))`, while events are drained by a separately spawned collector task. The correct poll-until-observed pattern already exists in the same file at `:419-429`.

**upstream** — pi's equivalent suites synchronize on observed state, not elapsed time (see the rendezvous pattern at `pi/packages/agent/test/agent-loop.test.ts:589-612`).

**Impact** — Under load the drain may not have happened when assertions run. The **negative** assertion among the five call sites is the worse half: it passes vacuously whenever the drain has not happened, so it proves nothing while looking green.

**Fix** — Replace `settle()` with the poll-until-observed helper already present at `:419-429`, bounded by a generous timeout; convert the negative assertion into "poll until the positive precondition holds, then assert the absence".

**Verify** — Both variants pass with the sleep removed entirely and under a single-worker runtime; the negative case must fail if the event it forbids is emitted. Fix in one pass with DRIFT-039 via a shared poll helper.

## DRIFT-039 — TEST DEFECT: `a_02_2_parallel_completion_vs_source_order` asserts a wall-clock upper bound and a completion ORDER it cannot control

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-agent/tests/agent_loop.rs:327`: `assert!(elapsed < Duration::from_millis(115), "parallel ran concurrently, took {elapsed:?}")`. The two `SpanTool`s sleep 80 ms and 50 ms (`:264-265`), and `elapsed` is measured at `:276` **before** `agent.prompt("go")` and read at `:281` **after** `wait_for_idle()` — so the budget also covers the faux stream and idle settling, not just the tool sleeps. Concurrent floor ≥80 ms, entire remaining margin under 35 ms, on a `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` runtime in a suite `cargo test` runs many-at-once. `:300-301` additionally asserts `ends == vec!["fast", "slow"]` and `assert_ne!(ends, starts)`, requiring the 50 ms tool's `ToolExecutionEnd` to be observed before the 80 ms one — a 30 ms scheduling margin. A sweep of all 12 `assert`-on-`elapsed` sites in the workspace confirms this is the tightest by a wide margin (`cyrup-ext/tests/native_dispatch.rs:875` is 3.75×; `:710` <2 s; `cyrup-ext-subagents/src/background/wait.rs:613-614,:679,:795` and `spawn/worktree.rs:1656` seconds-scale; `cyrup-tui/tests/terminal_theme_query.rs:224` and `src/terminal_query.rs:400` <1 s; `tui/intercom.rs:918-919` a 5× window).

**upstream** — pi proves the same property **causally**, with no timer. `pi/packages/agent/test/agent-loop.test.ts:589-612`: a `firstDone` promise plus a `releaseFirst` resolver; the first tool awaits inside `execute`, the second sets `parallelObserved = true` only if it runs while `firstResolved` is still false; the assertion is `expect(parallelObserved).toBe(true)` at `:675`. The mirror test at `:787-859` asserts `false` for the forced-sequential case. No elapsed-time assertion exists anywhere in pi's parallel-tool coverage.

**Impact** — A phantom red on an unrelated change whenever the machine is loaded — exactly the condition that produced the three prior instances of this shape in this repo (`providers/anthropic.rs`, `round9_l5res.rs`, `caps/proc.rs`). Worse, the diagnostic reads "parallel ran concurrently", so a contributor who hits it goes hunting a concurrency regression in `cyrup-agent` that is not there.

**Fix** — Replace the timing proof with a rendezvous, mirroring `agent-loop.test.ts:589-612`. Give `SpanTool` an optional `Arc<Notify>`/oneshot pair: the slow tool waits on a gate; the fast tool sets an `AtomicBool overlap_observed` if it enters while the slow tool has not exited, then releases the gate. Assert `overlap_observed` and delete the `elapsed` bound at `:327` along with the now-unused `Instant` at `:276`/`:281`. Derive the completion-order assertion at `:300` from release order rather than racing sleeps. Keep the `spans` vector for the sequential test at `:350-354`, where `s[0].2 <= s[1].1` is a genuine non-overlap invariant.

**Verify** — The rewritten test passes with both tool sleeps set to 0 ms (nothing timing-derived remains), still **fails** when both tools are forced to `ExecMode::Sequential`, and stays green under `taskset -c 0` / a single-worker runtime — which the current version cannot.

## DRIFT-040 — pi's agent-harness v2 rearchitecture (durable harness, session repositories, storage/search split) entirely unabsorbed

**Kind** tracking · **Severity** low · **Effort** L · **Confidence** high

**cyrup** — cyrup ports `pi/packages/agent/src/harness/{session,compaction,system-prompt}` into `cyrup-session`. None of the v2 surface exists: no repository/facade layer over sessions, no shutdown lifecycle, no swappable search index, no per-session store queue. `cyrup/crates/cyrup-session/src/entry.rs` and the compaction tree walk are still shaped for the pre-v2 `getPathToRoot` model — DRIFT-037 is the concrete symptom of exactly this drift.

**upstream** — `git -C pi log --oneline --no-merges --since=2026-07-15 -- packages/agent/src/harness | wc -l` → 37, and `pi/packages/agent/docs/` now carries four harness design docs (`agent-harness.md`, `durable-harness.md`, `harness-v2.md`, `harness.md`). Representative commits, all after cyrup's baseline: `9e7582aa` (#6594 sqlite session storage + `getPathToRootOrCompaction`), `82c48598` (shutdown lifecycle), `bc031ae4` (await mutations during shutdown), `871a9904` (unify harness task tracking), `62f3c61b` (per-session store queues), `4e81b796` (restrict session construction to repositories), `2700511e`, `a1bb7e44` (expose session stores through factories), `3c2b71b5` (swappable search index), `15f71948`, `2b765ef5`, `9cde1725`, `9c3b271b`, `5e48d41e` (repo facade), `6a9591ae`. pi HEAD `a0bb4a48` is itself a merge of `refactor/sqlite-session-connection-cleanup`.

**Impact** — Structural rather than behavioral today — cyrup's session layer works — but this is the largest single unabsorbed surface in pi core and where every future harness fix will land. Each such fix will be phrased against the v2 API and will need manual re-derivation. DRIFT-037 is the first concrete interop defect it produced; expect more. Filed so the next reader knows the divergence is deliberate-by-omission, not oversight.

**Fix** — Do **not** port now; the surface is still moving. Track as with DRIFT-022/DRIFT-023. When taken, the entry points are the repository/factory seam (`4e81b796`, `2700511e`, `a1bb7e44`) and the Storage-vs-Search split (`d5c37c65`, `3c2b71b5`) — cyrup's JSONL store becomes one `Storage` impl rather than the only one. `pi/packages/storage/sqlite-node` stays out of scope per the existing no-counterpart rule; only `packages/agent/src/harness` is cyrup's.

**Verify** — n/a while tracking. Re-scope once `git -C pi log --oneline -- packages/agent/src/harness` goes quiet for a release cycle, then diff the harness `types.ts` against `cyrup-session/src/entry.rs` and `agent_message.rs` to size the real delta.

## Coverage

**Read at cyrup HEAD `1806375`** (tree clean, `git status --porcelain` empty): `cyrup-core/src/message.rs`; `cyrup-provider/src/{api/{anthropic_messages,openai_completions,openai_responses,google_generative_ai,compat}.rs, utils/{retry,deferred_tools}.rs, remote_catalog.rs, models_store.rs, catalog.rs, env_api_keys.rs, auth/types.rs, providers/{fleet.rs,catalog/*.json}}` and `cyrup-provider/tests/catalog_data.rs`; `cyrup-session/src/{entry.rs, prompt/{builder,context_files,tests}.rs, compaction/summarize.rs}`; `cyrup-session-svc/src/{session.rs, hooks.rs, event.rs, compact.rs, bash.rs}` and `tests/summarization_retry_events.rs`; `cyrup-agent/src/agent.rs` and `tests/agent_loop.rs`; `cyrup-config/src/{model.rs, env_keys.rs}`; `cyrup-tools/src/tools/bash.rs` and `src/ops/{local,shell}.rs`; `cyrup-ext/{wit/world.wit, src/wrapper.rs}`; `cyrup-ext-sdk/wit/world.wit`; `cyrup-modes/src/rpc.rs`; `cyrup-tui/src/{app.rs, startup_selector.rs}`; `cyrup-ext-subagents/src/exec/fallback.rs`.

**Read at pi HEAD `a0bb4a48`**: `packages/ai/src/{types.ts, api/{anthropic-messages,openai-completions,google-shared,constrained-sampling}.ts, utils/{retry,deferred-tools}.ts, auth/env-api-keys.ts, providers/{anthropic.ts,*.models.ts}, scripts/generate-models.ts}`; `packages/agent/src/harness/{types.ts, session/session.ts}` and `packages/agent/docs/`; `packages/agent/test/agent-loop.test.ts`; `packages/coding-agent/src/core/{system-prompt.ts, model-resolver.ts, session-manager.ts, compaction/compaction.ts, agent-session.ts, prompt-templates.ts, tools/bash.ts, extensions/types.ts}`; `packages/coding-agent/src/modes/rpc/{rpc-mode,rpc-types}.ts`; `packages/coding-agent/docs/session-format.md`; `docs/rpc.md`; `.gitignore`. Also read `pi-subagents/src/runs/shared/model-fallback.ts` to settle DRIFT-014's fix instruction.

**Method** — Read-only throughout; no cargo, no npm. Every `closed` status was re-derived by opening the cyrup file at HEAD *and* the upstream file and comparing clause by clause, never by trusting a commit message. Result: **zero overturns** across the eight closures and the one partial. Five evidence errors in the prior filing were found and corrected in place — DRIFT-008's greps were written unspaced and return 0 against pretty-printed JSON; DRIFT-004's `BashOperations` grep returns 10 hits, not 0; DRIFT-028's "`~anthropic` appears nowhere" is false, and its falsity *strengthens* the item; DRIFT-026's pi thinking-arm refs were off by one; DRIFT-039's stated margin was if anything generous. None changed a status. Two prior fix *instructions* were wrong and are corrected: DRIFT-014 (do not touch `fallback.rs`) and DRIFT-033 (the work is in `cyrup-session-svc`, not `cyrup-agent`).

**Taken on trust / not re-derived** — DRIFT-019, DRIFT-023 and DRIFT-032 rest on their cited greps alone (simple-absence items). `6d29542`'s model-by-model catalog fidelity to pi@`91585d9a` was not validated; only the thinking-level mappings were checked. The prior reassessment's broader drift survey (commit counts, the newly-cleared non-gaps `57cde869`, `7303cbac`, `f9476a61`, `4523528b`) was accepted, since those only justify *not* filing items; `243f64be`'s clearance **was** re-verified firsthand because it underpins DRIFT-006's closure.

**Blind spots** — (1) DRIFT-014's load-bearing half is unresolved: the seven missing literals are certain, but what string reqwest/hyper emits on a failed DNS lookup is not (`grep 'getaddrinfo\|dns error\|failed to lookup' crates` → 0), and determining it needs a running process, which the no-cargo rule forbids. Do not close DRIFT-014 on literals alone. (2) DRIFT-034's byte-identity between the two `world.wit` copies was true at HEAD but is hand-maintained with nothing keeping it true. (3) Test-defect hunting covered the two known shapes plus a sweep of all 12 `assert`-on-`elapsed` sites; it did **not** cover assertions on event *counts* or on log output, where the same "pins current-but-wrong behavior" shape could hide.

**Confirmed clean** — no test pins DRIFT-026 (the only `high`; the only signature assertions in `google_generative_ai.rs` are `:1659-1661` on `is_valid_thought_signature`), DRIFT-013 (the `max_completion_tokens` assertions are on a correct `openai` model) or DRIFT-020 (zero assertions on `send_session_id_header`). DRIFT-035, DRIFT-036 and DRIFT-039 remain the only known test defects in this area.

**Items routed elsewhere and not assessed here** — pi `c2275d67` → area 07; `027a5847`/`74caa264`/`0563a7c0`/`58c0bc2f`/`66eead65` → areas 05/06; `35a0d5d6`/`45203abf`/`bff5ab71`/`0d008b74`/`bb226f9c` → area 07. Routing was checked against the crate map and looks correct.
