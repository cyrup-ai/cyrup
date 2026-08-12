# 01 — cyrup-core + cyrup-provider

This area covers `cyrup/crates/cyrup-core` (message/type model, JSONL serialization) and `cyrup/crates/cyrup-provider` (wire APIs, providers, catalogs, auth, streaming, validation), measured against `pi/packages/ai/` and `pi/packages/agent/` at cyrup's own porting baseline `pi@91585d9a` (2026-07-10), with post-baseline drift measured against pi HEAD (v0.83.0). Headline finding: the crate is materially healthier than at the last pass — tiered pricing, the `max` thinking level, all 31 catalogs, deferred/message-anchored tool loading and `ToolResultMessage.usage` all really landed and survive adversarial re-reading — but a stale-port cluster remains (no provider-request retry/timeout, unbounded error bodies, missing `allOf`, missing `max_output_tokens` floor), OAuth is still entirely absent, and a two-model stub catalog is still driving production model ranking inside `cyrup-ext-subagents`. Re-baselined against HEAD `1806375` on 2026-08-03; every closure below was re-decided by reading code at HEAD and the corresponding upstream file, never by reading a commit message.

**Addendum 2026-08-11, HEAD `097bdde`.** Two headline claims above are now stale: OAuth is no longer "entirely absent" (`cf26010` landed 11 flows under `auth/oauth/`), and the four missing providers are all registered (`PROV-005` closed). The Copilot provider was then read end to end against pi and yielded **three new highs** — `PROV-027`/`028`/`029`, filed under `## GitHub Copilot findings` at the foot of this file and listed in the Open-items table. Nothing else in this file was re-audited at `097bdde`; treat every other status as of 2026-08-07 or earlier.

## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| PROV-001 | CLOSED | `ModelCostTier` + `tiers` at `cyrup-provider/src/model.rs:19-48`; `select_rates`/`compute_cost` at `usage.rs:18-58` are statement-equivalent to pi `models.ts:639-658` (pi's `matchedThreshold = -1` seed and cyrup's `None` seed differ only on `inputTokensAbove: 0`, which both accept). Data half re-verified by an independent mechanical catalog diff: 7 `tiers` blocks at `providers/catalog/openai.json:642,729,766,804,838,875,912`, all `inputTokensAbove: 272000`, field-equal to pi. |
| PROV-002 | CLOSED | `Max` present and declared last in `ThinkingLevel`/`ModelThinkingLevel` (`cyrup-core/src/message.rs:29-57`); ladder + clamp match pi `models.ts:663-693` (`collection.rs:396-465`); wire bytes correct in all three APIs. The bug-pinning test is gone — `cyrup-provider/tests/thinking_max.rs` now pins the ladder, the clamp and the per-API wire bytes. The `:max` suffix parsing in `cyrup-ext-subagents` is area 09's to confirm. |
| PROV-003 | OPEN | No `login` on either auth trait; no `auth/oauth/`. User has deprioritised — filed, not scheduled. |
| PROV-004 | CLOSED | Independent from-scratch full-field diff of 30 cyrup catalogs against 35 pi `*.models.ts` at `91585d9a`: 26 providers zero diffs, `openai` exactly 7 (`supportsToolSearch`, a deliberate forward-port from pi `3d8f7435`), zero id-set differences. Post-baseline drift stays under PROV-018. |
| PROV-005 | CLOSED (2026-08-11, HEAD `097bdde`) | Both halves. `register_builtins` (`api/mod.rs:131-163`) now registers **9** factories including `BEDROCK_CONVERSE_STREAM` (`:154`, impl at `api/bedrock_converse_stream.rs`) and `OPENAI_CODEX_RESPONSES` — the dangling declaration is gone. All four providers are constructed and pushed in `providers/all.rs`: `amazon_bedrock` `:177`, `openai_codex` `:183`, `google_vertex` `:187`, `github_copilot` `:194`. Verified by reading both files at HEAD, not by commit message. **The Copilot port that closed this carries three new highs — see `PROV-027`/`028`/`029`.** Closing a "provider missing" item is not the same as the provider being right. |
| PROV-006 | OPEN | `timeout_ms`/`max_retries`/`max_retry_delay_ms` still read by nobody. |
| PROV-007 | OPEN | Severity RAISED to high — the 2-model `seed.json` drives six production call sites in `cyrup-ext-subagents`, two of which hard-error for 29 of 31 providers. |
| PROV-008 | OPEN | Re-classified stale-port: pi's 4000-char cap existed at cyrup's own baseline. |
| PROV-009 | CLOSED | Splitter, both renderings AND the producer chain verified whole. `cyrup-ext/src/wrapper.rs:143` really populates `addedToolNames`; it survives the loop and reaches persistence at `cyrup-session-svc/src/event.rs:387-391`. The c2a7acb "dedupes with the flag OFF" caveat is NOT debt — pi does the same at `deferred-tools.ts:13-15`. |
| PROV-010 | OPEN | `StopReason` still lacks `pending`; now entangled with PROV-022. |
| PROV-011 | OPEN | Zero hits for constrained-sampling anywhere in the workspace. |
| PROV-012 | OPEN | `rawStopReason` still absent; line refs corrected (each 1 lower than previously cited). |
| PROV-013 | CLOSED | `ToolResultMessage.usage` at `cyrup-core/src/message.rs:490-495`, emitted `:555-558`, `len` widened `:544-547`, round-trip proven both directions (`:677-698`, `:704-717`) and forward-compat at `:723-770`. Whether any tool populates it is AGENT-005, area 02. |
| PROV-014 | OPEN | `pi-messages` + radius + qwen-token-plan ×2 still unported. |
| PROV-015 | OPEN | Prerequisite now satisfied (`reasoning_effort` covers Minimal..Max); a pure additive-variant change remains. |
| PROV-016 | OPEN | Re-classified stale-port: pi's `allOf` handling existed at `91585d9a:validation.ts:14,189-193`. |
| PROV-017 | OPEN | Unchanged. |
| PROV-018 | PARTIALLY CLOSED | Provenance half really landed and is really load-bearing (`providers/catalog_manifest.json`, whose `generatedAt` matches `git show 91585d9a --stat` exactly, consumed as the overlay staleness floor at `remote_catalog.rs:181-196`). Generator half genuinely absent: no `xtask`, no mechanical drift check. Remains OPEN at reduced scope. |
| PROV-019 | OPEN (new) | `max_output_tokens` floor of 16 unported in BOTH Responses APIs — a stale port, missed by every prior pass. |
| PROV-020 … PROV-026 | OPEN (new) | Filed below; continue the existing numbering. |
| PROV-027 … PROV-029 | OPEN (new) | Added 2026-08-11 at HEAD `097bdde`, in `github-copilot` code that did not exist when this file was written. **All three high.** Bodies under `## GitHub Copilot findings`. |

## Open items

> **⚠ THIS TABLE IS NOT THE COMPLETE OPEN SET.** 5 further items from the 2026-08-03
> surface-driven sweep live in their own table under `## Surface-sweep findings`, with `-S` ids —
> **including 0 rated critical/high**. Enumerating only this table undercounts the area by 5 items,
> which is exactly how `SEAM-S01` (high) escaped a full audit pass on 2026-08-07. Count BOTH
> tables. See structural defect A in `00-residual-ledger.md`.
>
> The 2026-08-11 Copilot items (`PROV-027`/`028`/`029`, **all three high**) are listed **here** as
> well as in their own section, deliberately — that is defect A's prescribed fix applied to the one
> batch added since it was written. The sweep rows still need the same treatment.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| PROV-003 | high | not-ported | L | Zero OAuth login flows; `/login` dead-ends |
| PROV-007 | high | parity-bug | S | 2-model `seed_catalog()` stub is the registry subagents rank against |
| PROV-027 | high | parity-bug | S | Copilot's Claude models send `x-api-key`; pi sends `Authorization: Bearer` |
| PROV-028 | high | not-ported | S | `github-copilot-headers.ts` unported — no `X-Initiator`/`Openai-Intent`/`Copilot-Vision-Request` |
| PROV-029 | high | parity-bug | S | Copilot + Codex login flows written but unreachable; flow registry has no production caller |
| PROV-006 | medium | not-ported | M/L | `timeout_ms`/`max_retries`/`max_retry_delay_ms` declared, read by nobody |
| PROV-008 | medium | stale-port | S | HTTP error bodies unbounded; pi truncates at 4000 chars |
| PROV-010 | medium | upstream-drift | S | `StopReason` lacks `pending` |
| PROV-011 | medium | upstream-drift | L | `constrainedSampling` / grammar-constrained tools not modeled |
| PROV-014 | medium | upstream-drift | M | `pi-messages` API and three providers landed after baseline |
| PROV-016 | medium | stale-port | S | Tool-argument coercion ignores `allOf` |
| PROV-018 | medium | tooling | M | No catalog generator, no drift check |
| PROV-019 | medium | stale-port | S | `max_output_tokens` floor of 16 unported in both Responses APIs |
| PROV-021 | medium | upstream-drift | S | `ANTHROPIC_AUTH_TOKEN` bearer env unsupported |
| PROV-022 | medium | upstream-drift | S | `supportsFinishReason` unported — finish_reason-less streams error every turn |
| PROV-012 | low | upstream-drift | S | `AssistantMessage.rawStopReason` not carried |
| PROV-015 | low | not-ported | S | `ApiStreamOptions` has no `openai-completions` variant |
| PROV-017 | low | not-ported | S | `Provider` trait exposes no `name`/`base_url`/`headers` |
| PROV-020 | low | parity-bug | S | `toolResult` JSONL key order diverges: `isError` emitted too early |
| PROV-023 | low | upstream-drift | S | `prompt_cache_options` unported — one-shot requests implicitly cache-write |
| PROV-024 | low | upstream-drift | S | `sessionAffinityFormat` unported — OpenRouter would get the OpenAI header triple |
| PROV-025 | low | upstream-drift | M | `deferredToolsMode: "kimi"` unported |
| PROV-026 | low | test-defect | S | `seed_catalog_parses` pins the stale 200k Sonnet 4.5 context window |

## PROV-003 — Zero OAuth login flows; `login` absent from both auth traits, `/login` dead-ends

**Kind** not-ported · **Severity** high · **Effort** L · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/auth/mod.rs:49-62` `trait ApiKeyAuth { fn name(); async fn resolve(model, ctx, cred) }` has no `login`; `:64-75` `trait OAuthAuth { fn name(); async fn refresh(); async fn to_auth() }` likewise. The directory is `helpers.rs mod.rs resolve.rs store.rs types.rs` — no `oauth/`. Stored OAuth credentials can be used; none can be obtained.

**upstream** — `pi/packages/ai/src/providers/anthropic.ts:9-14` `login: async (interaction) => ({type:"api_key", key: await interaction.prompt(...)})` and `:44` `lazyOAuth({name, load: loadAnthropicOAuth})`, importing `../auth/oauth/load.ts`; pi implements 11 flows under `pi/packages/ai/src/auth/oauth/`.

**Impact** — No cyrup user can log in to any subscription or OAuth provider. `/login` has nowhere to go, and every such provider is unreachable without hand-placing credentials.

**Fix** — Port `pi/packages/ai/src/auth/oauth/*` (pkce, device code, local callback page, `load.ts`) into a new `cyrup-provider/src/auth/oauth/`; add `login` to both traits at `auth/mod.rs:49-75`; wire per-provider `login` for anthropic, github-copilot, openrouter, xai, kimi-coding, radius.

**Verify** — A no-credential `login` for at least anthropic produces a stored credential that `auth/resolve.rs` then consumes with no env var present.

**Note** — Deprioritised by the user. Keep filed, do not schedule. PROV-014's radius half is blocked behind it.

## PROV-007 — 2-model `seed_catalog()` stub is the registry `cyrup-ext-subagents` validates and ranks against

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/catalog.rs:20-24` `seed_catalog()` reads `cyrup/crates/cyrup-provider/src/catalog/seed.json`, which is exactly two models: `claude-sonnet-4-5` with `"baseUrl": "https://api.anthropic.com/v1/messages"` (`:7` — a full endpoint path where every real catalog stores an origin) and `"contextWindow": 200000` (`:19` — where the real `providers/catalog/anthropic.json` and pi both say 1000000), plus `gpt-4o` with `"https://api.openai.com/v1/chat/completions"` (`:27`). Six PRODUCTION call sites in `cyrup/crates/cyrup-ext-subagents/src/extension.rs`: `:1988` (via `seed_catalog_available_models`, defined `:6873-6874`), `:6156`, `:6191`, `:6285`, `:6313`, `:7468`. `:6156-6181` `provider_ranked_full_ids_from_catalog` reads `cost`/`reasoning`/`context_window`/`max_tokens` off the seed into `RankedCandidate` → `filter_dominated`. `:6285-6292` and `:6313-6320` are hard `Err(SubagentError::MalformedSettings("No models found in the current registry for provider '{provider}'."))` guards. `:6191` `write_provider_catalog_file` filters the seed to build the real probe list.

**upstream** — `pi-subagents/src/profiles/profiles.ts` and `pi-subagents/src/agents/agent-management.ts` always consult the live registry via `ctx.modelRegistry.getAvailable()`; there is no seed or stub catalog anywhere in the repo.

**Impact** — 29 of 31 implemented providers cannot run `/subagents-refresh-provider-models` or `/subagents-generate-profiles` at all — hard error. For the two that can, profile ranking uses stale metadata (200k instead of 1M context) and a path-shaped `baseUrl`.

**Fix** — Swap all six production sites to `default_models(CreateModelsOptions::default()).get_models(None)`, then delete or `#[cfg(test)]`-gate `seed_catalog` (it is re-exported at `cyrup-provider/src/lib.rs:54`, so gating requires removing that too). Land with PROV-026.

**Verify** — `/subagents-generate-profiles` succeeds for a provider that is neither anthropic nor openai; deleting the `seed_catalog` symbol makes the compiler prove there are no remaining non-test callers.

## PROV-005 — Three of nine baseline wire APIs and four providers unimplemented

> **CLOSED 2026-08-11 at HEAD `097bdde`** — see the status table. Both halves landed: 9 registered
> factories including `bedrock-converse-stream`, and all four providers in `all.rs`. The body below
> is the 2026-08-03 evidence, kept so the closure can be re-audited. **Follow-on: the Copilot
> provider this closed carries `PROV-027`/`028`/`029`, all high.**

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/api/mod.rs:122-148` `register_builtins` registers exactly six factories (openai-completions, anthropic-messages, openai-responses, azure-openai-responses, google-generative-ai, mistral-conversations). `cyrup/crates/cyrup-provider/src/lib.rs:166-174` `known_api` declares 7 constants, the 7th being `BEDROCK_CONVERSE_STREAM` — a dangling declaration with no registered factory. The catalog sweep confirms no `amazon-bedrock`, `github-copilot`, `google-vertex` or `openai-codex`.

**upstream** — `pi/packages/ai/src/types.ts:16-26` `KnownApi` lists 10 text wire APIs; `pi/packages/ai/src/providers/all.ts` ships 38 built-ins; the four missing catalogs are 106, 25, 10 and 7 models respectively.

**Impact** — Bedrock, Copilot, Vertex and Codex users cannot use cyrup at all. Worse, a `models.json` custom provider can select `bedrock-converse-stream` and get a runtime "no factory" failure rather than a config-time rejection.

**Fix** — S-sized mitigation first: gate or delete `known_api::BEDROCK_CONVERSE_STREAM` at `lib.rs:173` so an api with no factory cannot be selected. Full fix: port `pi/packages/ai/src/api/bedrock-converse-stream.ts`, `google-vertex.ts` and `openai-codex-responses.ts` as new `ApiImpl`s, register them at `api/mod.rs:122-148`, and add the four catalogs.

**Verify** — A `models.json` naming an unregistered api is rejected at load; each new api round-trips a faux stream through `cyrup-provider`'s `faux` feature.

## PROV-006 — `timeout_ms` / `max_retries` / `max_retry_delay_ms` declared, threaded, read by nobody

**Kind** not-ported · **Severity** medium · **Effort** M/L · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/stream.rs:176` `timeout_ms`, `:179` `websocket_connect_timeout_ms`, `:182` `max_retries`, `:185` `max_retry_delay_ms`. `cyrup/crates/cyrup-provider/src/stream/sse.rs:121-165` `open_sse` has no `.timeout(..)` and no retry loop; a non-2xx returns `Err(ProviderError::Http)` on the first attempt at `:152-158`. `cyrup/crates/cyrup-provider/src/utils/retry.rs:113-134` is the SESSION-level `RetryPolicy` (`enabled`/`max_retries`/`base_delay_ms`, a port of pi `retry.ts:97-103`) — a different mechanism from provider-request retry.

**upstream** — `pi/packages/ai/src/utils/provider-retry.ts` is imported by all five wire impls and implements per-request retry with backoff plus a request timeout, throwing through `formatProviderError(normalizeProviderError(err))`.

**Impact** — A transient 429/503, or a connection that hangs, kills the turn immediately. Users who configure `maxRetries` see it silently ignored — the field exists, is threaded, and does nothing.

**Fix** — Port `provider-retry.ts` to `cyrup-provider/src/utils/provider_retry.rs`; wrap `open_sse` (`stream/sse.rs:121-165`) in it and apply `.timeout(Duration::from_millis(timeout_ms))` on the reqwest builder. Land together with PROV-008 — the retry path is where the truncated error body surfaces.

**Verify** — Mock origin returns 503 twice then 200: the stream succeeds after two retries and honours `max_retry_delay_ms`; a never-responding origin fails at `timeout_ms` rather than hanging.

## PROV-008 — HTTP error bodies copied verbatim and unbounded; pi truncates at 4000 chars

**Kind** stale-port · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/stream/sse.rs:152-158`: `let message = resp.text().await.unwrap_or_default();` goes straight into `ProviderError::Http { status, message }` with no cap.

**upstream** — `pi/packages/ai/src/utils/error-body.ts:16` `export const MAX_PROVIDER_ERROR_BODY_CHARS = 4000` and `:81` `return truncateErrorText(trimmed, MAX_PROVIDER_ERROR_BODY_CHARS)`. Both present at `91585d9a` — cyrup had this available at port time and did not take it.

**Impact** — A gateway that returns a multi-megabyte HTML error page dumps it into the TUI and into the session JSONL, bloating the transcript and, on the following turn, the prompt.

**Fix** — Port `error-body.ts` to `cyrup-provider/src/utils/error_body.rs` (`MAX_PROVIDER_ERROR_BODY_CHARS`, `truncate_error_text`) and route `sse.rs:152-158` through it.

**Verify** — Mock origin returns a 100 kB 500 body; `ProviderError::Http.message` is ≤4000 chars and ends with pi's truncation marker.

## PROV-010 — `StopReason` lacks `"pending"` — pi in-flight state cannot round-trip

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-core/src/message.rs:92-100` `enum StopReason { Stop, Length, ToolUse, Error, Aborted }`, derived `Deserialize`, with no unknown-variant fallback; `AssistantMessage.stop_reason` is a required non-`Option` at `:362`.

**upstream** — `pi/packages/ai/src/types.ts:391` is now `"pending"|"stop"|"length"|"toolUse"|"error"|"aborted"`; `91585d9a:types.ts:375` had only the five, so this is genuine post-baseline drift.

**Impact** — Loading a pi session JSONL written mid-turn fails deserialization outright rather than degrading. The interop round-trip guarantee is not total.

**Fix** — Add `Pending` to `StopReason` at `message.rs:92-100` with `#[serde(rename = "pending")]`; audit the exhaustive matches the compiler surfaces. Land with PROV-022 — pi's guard is now `if ((compat.supportsFinishReason && !hasFinishReason) || output.stopReason === "pending") throw` (`openai-completions.ts:580`), so the two are entangled.

**Verify** — `cyrup-test-support::interop` round-trips a fixture assistant message carrying `"stopReason":"pending"`.

## PROV-011 — `Tool.constrainedSampling` / grammar-constrained tools not modeled

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** confirmed

**cyrup** — `grep -rn 'constrained_sampling|ConstrainedSampling|supports_openai_grammar_tools|supports_strict_tools' cyrup/crates --include=*.rs` returns zero hits workspace-wide. `cyrup/crates/cyrup-provider/src/api/anthropic_messages.rs:1180-1210` `convert_tools` emits no strict-schema merge. (`supports_strict_mode` IS correctly ported in `compat.rs` and consumed by `openai_completions.rs::convert_tools` — a different flag; do not confuse them.)

**upstream** — `pi/packages/ai/src/api/anthropic-messages.ts:1303` `resolveJsonSchemaStrictSampling(tool, supportsStrictTools)` and `:1313-1319` the `strict === true` schema merge; `constrainedSampling` sits on pi's `Tool` type.

**Impact** — Models that can be grammar-constrained still emit free-form tool arguments, so malformed-argument retries that pi avoids by construction still occur in cyrup.

**Fix** — Add `constrained_sampling` to `cyrup_core::Tool`, `supports_strict_tools` to `ModelCompat`/`ResolvedCompat`, port `resolveJsonSchemaStrictSampling` into `cyrup-provider/src/utils/`, and apply it in `anthropic_messages.rs::convert_tools` and the openai-completions equivalent.

**Verify** — A tool declaring `constrainedSampling` on a strict-capable model serializes the merged strict schema byte-equal to pi's; on a non-capable model the schema is unchanged.

## PROV-014 — `pi-messages` API and three providers (radius, qwen-token-plan ×2) landed after baseline

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/lib.rs:166-174` `known_api` has 7 constants, none `pi-messages`; there is no `src/api/pi_messages.rs`. The 31 embedded catalogs include no `radius`, `qwen-token-plan` or `qwen-token-plan-cn`; `cyrup/crates/cyrup-provider/tests/catalog_data.rs:86` pins the roster at 30 + images. `cyrup/crates/cyrup-provider/src/env_api_keys.rs:35-72` has no radius or qwen arms.

**upstream** — `pi/packages/ai/src/env-api-keys.ts:79-81` maps `"qwen-token-plan": "QWEN_TOKEN_PLAN_API_KEY"` and `"qwen-token-plan-cn": "QWEN_TOKEN_PLAN_CN_API_KEY"`; `pi/packages/ai/src/providers/all.ts` registers radius; `pi/packages/ai/src/api/pi-messages.ts` is a new wire API.

**Impact** — Three providers are unreachable and one wire API unimplemented relative to current pi.

**Fix** — qwen-token-plan ×2 are cheap: two `providers/fleet.rs:45-53` members, two catalogs, two `env_api_keys.rs` arms. radius is blocked behind PROV-003 (OAuth). `pi-messages` is a fresh `ApiImpl` plus an `api/mod.rs:122-148` registration and a `known_api` constant.

**Verify** — Each new provider resolves from its env var and streams against a faux origin; the `catalog_data.rs:86` roster count is updated deliberately, not silently.

## PROV-016 — Tool-argument coercion ignores `allOf`; treats `anyOf`/`oneOf` as alternatives

**Kind** stale-port · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/validate.rs:93-98` is `schema.get("anyOf").or_else(|| schema.get("oneOf"))` — the two are treated as mutually exclusive alternatives; `grep -n allOf cyrup/crates/cyrup-provider/src/validate.rs` returns nothing.

**upstream** — `pi/packages/ai/src/utils/validation.ts:14` declares `allOf?: JsonSchemaObject[]`; `:189-193` runs a sequential `if (Array.isArray(schema.allOf))` loop, followed by INDEPENDENT (non-`else`) `anyOf` and `oneOf` steps. Present at `91585d9a` — a stale port, not drift.

**Impact** — Tool arguments whose schema uses `allOf` composition (common in generated schemas) are not coerced, so string-typed numbers and booleans reach the tool uncoerced and it errors on input pi would have accepted. Schemas carrying both `anyOf` and `oneOf` only get the first applied.

**Fix** — In `validate.rs:93-98`, replace the `or_else` with three sequential independent passes mirroring `validation.ts:189-200`.

**Verify** — Unit tests: an `allOf`-composed object coerces every branch's properties; a schema carrying both `anyOf` and `oneOf` applies both.

## PROV-018 — No catalog generator and no drift check; the 31 catalogs can silently fall behind

**Kind** tooling · **Severity** medium · **Effort** M · **Confidence** confirmed (partially closed)

**cyrup** — Provenance half landed and is load-bearing: `cyrup/crates/cyrup-provider/src/providers/catalog_manifest.json` records `generatedAt: "2026-07-10T16:34:43Z"` / `source: "pi@91585d9a3829831b07560901c4b3e9bbe3b4e35a"` — verified against `git show 91585d9a --stat`, whose commit date is `Fri Jul 10 16:34:43 2026 +0200`, so the manifest is accurate rather than decorative — and `cyrup/crates/cyrup-provider/src/remote_catalog.rs:181-196` `remote_models` discards a persisted overlay whose `last_modified <= local_generated_at`, citing pi `remote-catalog-provider.ts:32-40`. Generator half absent: no `xtask` directory, `grep xtask cyrup/Cargo.toml` empty, and `cyrup/crates/cyrup-provider/tests/catalog_data.rs:84-100` is only a roster-count / non-empty / non-empty-baseUrl guard with hand-picked spot values at `:155-175` (`sonnet_4_6_max_tokens_is_128k`, `retired_claude_models_are_gone`) — not a mechanical diff against a named pi revision.

**upstream** — pi's catalog data is now gitignored generated JSON under `pi/packages/ai/src/providers/data/` (`pi/.gitignore:11`); each `pi/packages/ai/src/providers/*.models.ts` at HEAD is an 8-line wrapper (`import values from "./data/openai.json"` + `flattenModelCatalog`). `pi/packages/ai/src/image-models.generated.ts` is 40 entries at HEAD vs cyrup's 35.

**Impact** — Nothing warns when the 31 embedded catalogs fall behind pi. Users get stale context windows, stale pricing and missing models with no signal. This is exactly how PROV-004 arose.

**Fix** — Add a `cyrup/xtask` with `gen-catalogs` that runs pi's `npm run generate-models` (the tree can no longer simply be read), consumes `packages/ai/src/providers/data/*.json` plus `image-models.generated.ts`, emits the 31 `providers/catalog/*.json` including `openrouter-images.json`, and rewrites `catalog_manifest.json`. Add an `#[test] #[ignore]` drift check that re-runs the generator into a temp dir and diffs. Do NOT add a `PROVENANCE.txt` — `catalog_manifest.json` supersedes that older sketch.

**Verify** — `cargo xtask gen-catalogs` against `91585d9a` reproduces the current tree byte-for-byte; the ignored drift test fails when pointed at pi HEAD.

## PROV-019 — `max_output_tokens` floor of 16 unported in BOTH Responses APIs; a small `max_tokens` produces a hard 400

**Kind** stale-port · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/api/openai_responses.rs:342-344`: `if let Some(max) = opts.max_tokens { obj.insert("max_output_tokens", json!(max)); }` — raw, no floor. Identically at `cyrup/crates/cyrup-provider/src/api/azure_openai_responses.rs:369-371`. `grep -n 'MIN_OUTPUT_TOKENS' cyrup/crates/cyrup-provider/src` returns nothing. Two sub-divergences in the same three lines: (a) no `max(v, 16)` clamp; (b) cyrup gates on `Some(_)` where pi gates on JS truthiness, so `max_tokens: Some(0)` emits `"max_output_tokens": 0` where pi omits the key entirely.

**upstream** — `pi/packages/ai/src/api/openai-responses.ts:28` carries the comment `// OpenAI Responses rejects max_output_tokens below 16: https://github.com/earendil-works/pi/issues/6265`, `:29` `const OPENAI_RESPONSES_MIN_OUTPUT_TOKENS = 16;`, `:237` `if (options?.maxTokens) { params.max_output_tokens = Math.max(options.maxTokens, OPENAI_RESPONSES_MIN_OUTPUT_TOKENS); }`. The same constant and clamp exist in `pi/packages/ai/src/api/azure-openai-responses.ts`. Both present at `91585d9a` and unchanged at HEAD (`openai-responses.ts:32,290`; `azure-openai-responses.ts:26,293`) — a stale port, not drift. Reading both `buildParams` end-to-end against cyrup's shows this is the ONLY divergence in either function.

**Impact** — Any caller setting a small `max_tokens` on an `openai-responses` or `azure-openai-responses` model gets a hard HTTP 400 and a failed turn where pi silently clamps and succeeds. pi added the clamp in response to a filed issue, so the path is reached in practice; the likely producers are compaction/summary calls and small user overrides, neither exotic. There is no workaround short of the caller knowing the magic number.

**Fix** — Add `const OPENAI_RESPONSES_MIN_OUTPUT_TOKENS: u64 = 16;` to each file and change both sites to `if let Some(max) = opts.max_tokens.filter(|m| *m > 0) { obj.insert("max_output_tokens", json!(max.max(OPENAI_RESPONSES_MIN_OUTPUT_TOKENS))); }` — the `.filter` reproduces pi's truthiness gate. Cite the pi lines and the issue link in the doc comment, as pi does.

**Verify** — Extend `build_params_basic_shape` (`openai_responses.rs:1547-1563`, today `max_tokens: Some(100)` ⇒ `100`, an assertion that stays valid): `Some(4)` ⇒ `16`; `Some(0)` ⇒ key ABSENT; `None` ⇒ absent. Mirror all three in `azure_openai_responses.rs` next to the existing check at `:612`.

## PROV-021 — `ANTHROPIC_AUTH_TOKEN` bearer-token env unsupported; corporate Anthropic gateways cannot authenticate

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `grep -rn ANTHROPIC_AUTH_TOKEN cyrup/crates --include=*.rs` returns zero hits workspace-wide. `cyrup/crates/cyrup-provider/src/env_api_keys.rs:39` is `"anthropic" => Some(&["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"])`. `cyrup/crates/cyrup-provider/src/providers/anthropic.rs:34-37` `anthropic_auth()` = `ProviderAuth::with_api_key(env_key(["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]))`, resolving only into `ModelAuth.api_key` (→ `x-api-key`). No env path produces an `Authorization: Bearer` header.

**upstream** — Added post-baseline by pi `24e5cc04` "fix(ai): support Anthropic bearer token env (#6148)" (`91585d9a:env-api-keys.ts:69-71` returned only the two vars). At HEAD, `pi/packages/ai/src/env-api-keys.ts:29` exports `ANTHROPIC_AUTH_TOKEN_ENV`; `:70-74` returns all three with an inline comment that it "participates in env discovery/status, but getEnvApiKey() skips it because requests must pass it as Authorization: Bearer"; `:147` implements that carve-out (`envKeys.find(key => key !== ANTHROPIC_AUTH_TOKEN_ENV)`); `pi/packages/ai/src/providers/anthropic.ts:20-31` resolves it BEFORE the other two into `{ auth: { headers: { Authorization: "Bearer …" } }, source: ANTHROPIC_AUTH_TOKEN_ENV }`.

**Impact** — `ANTHROPIC_AUTH_TOKEN` is the standard variable for Anthropic-compatible gateways and proxies that authenticate with a bearer token rather than `x-api-key`. A user with only that set gets "not configured" from cyrup's anthropic provider in an environment where pi works. It also affects auth STATUS reporting, since `env_api_keys.rs` is what the login/status pickers consult.

**Fix** — (1) `env_api_keys.rs:39` → `Some(&["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"])`, mirroring pi's `getEnvApiKey` carve-out wherever the first element would be turned into a literal api key. (2) `providers/anthropic.rs:34-37` — replace `env_key([...])` with a bespoke `ApiKeyAuth` (in-tree template: `providers/cloudflare.rs:71-113`) that, after the stored-credential branch, probes `ANTHROPIC_AUTH_TOKEN` first and returns `AuthResult { auth: ModelAuth { api_key: None, headers: Some({"Authorization": Some(format!("Bearer {t}"))}), base_url: None }, source: Some("ANTHROPIC_AUTH_TOKEN") }`, falling through to the existing two.

**Verify** — With only `ANTHROPIC_AUTH_TOKEN=t`, resolve yields `Authorization: Bearer t` and NO `x-api-key`; with both it and `ANTHROPIC_API_KEY` the bearer wins; with only `ANTHROPIC_API_KEY` behavior is unchanged; `api_key_env_vars("anthropic")` reports all three.

## PROV-022 — `supportsFinishReason` unported; a provider that never sends `finish_reason` errors every turn

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/api/openai_completions.rs:1499-1511` unconditionally converts "stream ended with no finish_reason" into a terminal error: `if !saw_finish_reason && stop_reason != Error && stop_reason != Aborted { message.stop_reason = StopReason::Error; message.error_message = Some("Stream ended without finish_reason") }`. There is no compat escape hatch: `grep -n supports_finish_reason cyrup/crates/cyrup-provider/src/api/compat.rs` is empty, and neither `ModelCompat` (`compat.rs:73…`) nor `ResolvedCompat` (`:205-222`) carries the field. This is a FAITHFUL baseline port — pi at `91585d9a:openai-completions.ts:453-454` threw unconditionally — so the item is drift, correctly classified, not a cyrup bug.

**upstream** — pi `2c304124` "fix(ai): support streams without finish reasons", post-baseline. At HEAD `pi/packages/ai/src/api/openai-completions.ts:574-576` `if (!hasFinishReason && !compat.supportsFinishReason) { output.stopReason = output.content.some(b => b.type === "toolCall") ? "toolUse" : "stop"; }` and `:580` narrows the throw. `pi/packages/ai/src/types.ts:529` `supportsFinishReason?: boolean`; `openai-completions.ts:1451` defaults it true in `detectCompat`, `:1501` resolves the catalog override.

**Impact** — cyrup cannot talk to any OpenAI-compatible endpoint that omits `finish_reason` (llama.cpp / vLLM / LM Studio-style local servers, some gateway shims). Every completed turn terminates as `StopReason::Error` even though the content arrived intact. pi makes this a one-line `models.json` opt-out; cyrup has no workaround short of patching source.

**Fix** — Add `supports_finish_reason: Option<bool>` to `ModelCompat` and `supports_finish_reason: bool` to `ResolvedCompat` (`compat.rs:205-222`), default `true` in `detect_compat`, resolve in `get_compat` — the same two-line pattern `send_session_affinity_headers` uses at `compat.rs:372` / `:422-424`. At `openai_completions.rs:1499-1511`, insert pi's inference branch ahead of the error branch and gate the error on the flag. Land with PROV-010.

**Verify** — Decoder test: SSE with `delta.content` then `[DONE]` and no `finish_reason` — default (true) still yields `StopReason::Error` (regression guard; the existing assertion at `openai_completions.rs:2755` stays green); with `compat: {"supportsFinishReason": false}` yields `Stop`, or `ToolUse` when a tool-call block is present.

## PROV-012 — `AssistantMessage.rawStopReason` not carried

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `grep -n 'raw_stop_reason|rawStopReason' cyrup/crates/cyrup-core/src/message.rs` returns nothing. The struct is `cyrup/crates/cyrup-core/src/message.rs:334-365`; the manual serializer at `:367-411` emits role, content, api, provider, model, responseModel?, responseId?, diagnostics?, usage, stopReason, errorMessage?, timestamp — byte-exact against pi except for the missing field. `len` at `:379-383`, `errorMessage`/`timestamp` at `:405-409`.

**upstream** — `pi/packages/ai/src/types.ts:399-412`; `rawStopReason?: string` sits at `:411`, between `errorMessage` (`:410`) and `timestamp` (`:412`).

**Impact** — The provider's untranslated stop reason is lost, so diagnostics cannot distinguish e.g. a content filter from a generic `stop`, and a pi session carrying it does not survive re-export byte-identically.

**Fix** — Add `raw_stop_reason: Option<String>` near `message.rs:362-364`, emit it between `errorMessage` and `timestamp` at `:405-409`, bump `len` at `:379`, and populate it from each api's raw finish field.

**Verify** — Interop round-trip of a pi fixture carrying `rawStopReason` is byte-identical.

## PROV-015 — `ApiStreamOptions` has no `openai-completions` variant

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/stream.rs:210-221` `enum ApiStreamOptions` has exactly five variants (Anthropic, OpenAiResponses, AzureOpenAiResponses, Google, Mistral); the doc at `:204-209` states that only variants whose fields are not already on `StreamOptions` are modeled, and `stream.rs` carries only the unified `reasoning: ModelThinkingLevel`.

**upstream** — `pi/packages/ai/src/types.ts` `OpenAICompletionsStreamOptions` exists as its own member of the api-options union.

**Impact** — Callers cannot pass openai-completions-specific per-request options, and any future completions-only option has nowhere to go.

**Fix** — Add the variant at `stream.rs:210-221` and destructure it in `openai_completions.rs::build_params`. The prerequisite is now satisfied: `openai_completions.rs:230-241` `reasoning_effort` covers Minimal..Max inclusive, so this is purely additive.

**Verify** — A completions-only option set through the new variant reaches the request body; the other apis reject it as before.

## PROV-017 — `Provider` trait exposes no `name` / `base_url` / `headers`

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/provider.rs:17-30` — the trait's only identity method is `fn id(&self) -> &ProviderId`; `models()`, `provider_auth()` (defaulted `None`, `:27-29`) and `get_model()` follow. No `name`, `base_url` or `headers`.

**upstream** — `pi/packages/ai/src/types.ts` `Provider` carries `name`, `baseUrl` and `headers` alongside `id`, and pi's UI renders the display name and endpoint from them.

**Impact** — Provider pickers and status output can only show the machine id, and provider-level default headers or base URL must be duplicated per model.

**Fix** — Add defaulted `fn name(&self) -> &str { self.id().as_str() }`, `fn base_url(&self) -> Option<&str> { None }` and `fn headers(&self) -> Option<&HeaderMap> { None }` at `provider.rs:17-30`, overriding in `providers/fleet.rs` and `providers/anthropic.rs`.

**Verify** — Each of the 31 providers reports a human display name distinct from its id wherever pi has one.

## PROV-020 — `toolResult` JSONL key order diverges from pi: `isError` emitted before `details`/`usage`/`addedToolNames`

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-core/src/message.rs:530-568` — the hand-written `Serialize` for `Message::ToolResult` writes `role`, `toolCallId`, `toolName`, `content`, **`isError` (`:553`)**, `details?` (`:554-557`), `usage?` (`:555-558`), `addedToolNames?` (`:559-563`), `timestamp` (`:564`). The doc at `:505-509` states the serializer exists specifically for "Pi's field order"; the comment at `:540-543` explicitly chose the new keys' slots so as to preserve cyrup's own pre-existing positions. `AssistantMessage`'s serializer (`:367-411`) IS byte-exact against pi, so the defect is isolated to this one message kind.

**upstream** — `pi/packages/agent/src/agent-loop.ts:773-786` `createToolResultMessage` is the sole construction site; the object literal's order is `role, toolCallId, toolName, content, details, usage, addedToolNames, isError, timestamp`. pi's session write path is a bare `JSON.stringify(entry)` (`pi/packages/coding-agent/src/core/session-manager.ts:940,952,959`), so that literal order IS the on-disk byte order. `pi/packages/ai/src/types.ts:415-431` agrees — `isError` at `:429`, after `addedToolNames` (`:428`).

**Impact** — Cosmetic on parse; nothing fails today because `cyrup-test-support::interop` compares `serde_json::Value`. But it falsifies the crate's own byte-fidelity claim — a cyrup-exported session JSONL is not byte-identical to pi's for any toolResult line, which is the single property the hand-written serializer exists to provide, and it would break any future golden diff taken against real pi output.

**Fix** — Move `st.serialize_field("isError", is_error)` from `message.rs:553` to immediately before `timestamp` at `:564`. Pure reordering — no `len` change, no serde attribute change, no deserialize change. Correct the now-wrong comment at `:540-543`.

**Verify** — The reorder is safe: the round-trip test at `message.rs:677-698` asserts only `details < usage` positionally (true in pi too), so it does not pin the bug and will not break; `old_shape_tool_result_reads_and_re_exports_unchanged` (`:703-717`) uses a fixture with no `details`/`usage`/`addedToolNames`, so `isError` stays adjacent to `timestamp` and it stays green. Extend `:677` with `find("details") < find("usage") < find("addedToolNames") < find("isError") < find("timestamp")`.

## PROV-023 — `supportsExplicitPromptCacheMode` / `prompt_cache_options` unported; one-shot requests implicitly cache-write

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/api/openai_responses.rs:320-341` builds `model`/`input`/`stream`/`prompt_cache_key`/`prompt_cache_retention`/`store` and never emits `prompt_cache_options`; `grep -rn prompt_cache_options cyrup/crates` and `grep -rn supports_explicit_prompt_cache cyrup/crates` both return nothing. Cyrup already models the retention tri-state the flag pairs with (`resolve_cache_retention` / `CacheRetention::None` at `:321,336-340`), so only the flag and one body key are missing.

**upstream** — pi `241431c6` "fix(agent,ai): don't cache write compaction or branch summaries (#6618)", post-baseline. At HEAD `pi/packages/ai/src/api/openai-responses.ts:278` `const disableImplicitPromptCache = cacheRetention === "none" && compat.supportsExplicitPromptCacheMode;` and `:285` `prompt_cache_options: disableImplicitPromptCache ? { mode: "explicit" } : undefined`; `:75` resolves `supportsExplicitPromptCacheMode: model.compat?.supportsExplicitPromptCacheMode ?? false`.

**Impact** — Money, quietly. pi turns implicit prompt caching OFF for exactly the requests whose prompts are one-shot (compaction summaries, branch summaries — run with `cacheRetention: "none"`); cyrup sends no `prompt_cache_options`, so OpenAI implicitly cache-WRITES those and bills the cache-write premium. Correctness unaffected; confined to one model family.

**Fix** — Add `supports_explicit_prompt_cache_mode: Option<bool>` to `ModelCompat` and surface it on `ResolvedResponsesCompat` with pi's `?? false` default alongside `supports_tool_search` (`compat.rs:171-193` is the exact precedent). In `openai_responses.rs`, after the `prompt_cache_retention` insert at `:336-340`, add `if cache == CacheRetention::None && compat.supports_explicit_prompt_cache_mode { obj.insert("prompt_cache_options", json!({"mode":"explicit"})); }`. Then set the flag on the gpt-5.6-* entries in `providers/catalog/openai.json`. The flag MUST stay default-false — older OpenAI models reject the parameter.

**Verify** — Body-shape test in `openai_responses.rs` (pattern at `:1547-1563`): flag on + retention none ⇒ body carries `"prompt_cache_options":{"mode":"explicit"}` and NO `prompt_cache_key`; long/short retention ⇒ neither; flag absent ⇒ neither regardless of retention (the older-model regression guard).

## PROV-024 — `sessionAffinityFormat` unported; OpenRouter would get the OpenAI header triple instead of `x-session-id`

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (currently latent)

**cyrup** — `cyrup/crates/cyrup-provider/src/api/openai_completions.rs:214-219` — when affinity is on, cyrup unconditionally injects all three OpenAI headers `session_id`, `x-client-request-id`, `x-session-affinity`, with no provider branch. `cyrup/crates/cyrup-provider/src/api/compat.rs:108` carries only the boolean `send_session_affinity_headers` (resolved `:220,372,422-424`); `grep -n session_affinity_format cyrup/crates/cyrup-provider/src` returns nothing.

**upstream** — Post-baseline: `91585d9a:openai-completions.ts` has no `x-session-id` and only the boolean at `:521`, defaulted false at `:1252`. At HEAD `pi/packages/ai/src/types.ts:109` `export type SessionAffinityFormat = "openai" | "openai-nosession" | "openrouter"` with the field on both compat interfaces (`:571`, `:581`); `pi/packages/ai/src/api/openai-completions.ts:650-659` branches — `openrouter` sends ONLY `x-session-id`, `openai` adds `session_id`, both non-openrouter forms send `x-client-request-id` + `x-session-affinity`; `:1477` auto-detects `isOpenRouter ? "openrouter" : "openai"`, `:1520` resolves the override.

**Impact** — Latent but wrong-by-construction. Latency independently verified: `grep -l sendSessionAffinityHeaders cyrup/crates/cyrup-provider/src/providers/catalog/*.json` returns exactly `fireworks.json`, `cloudflare-ai-gateway.json` and `cloudflare-workers-ai.json`, all of which correctly want the `openai` form; no openrouter entry enables it today. The moment a catalog refresh flips one on — pi's generator now emits the field — cyrup sends OpenRouter three headers it does not understand and omits the one it does, silently losing sticky routing and its prompt-cache hit rate. `openai-nosession` is also unreachable.

**Fix** — Add `SessionAffinityFormat { Openai, OpenaiNosession, Openrouter }` beside `CacheControlFormat`/`ThinkingFormat` in `compat.rs`, add `session_affinity_format: Option<..>` to `ModelCompat` and the resolved field to `ResolvedCompat` (`:205-222`), auto-detect `Openrouter` in `detect_compat`, resolve the override in `get_compat`. Branch `openai_completions.rs:214-219` exactly as `openai-completions.ts:650-659` does.

**Verify** — Extend the header test at `openai_completions.rs:2807`: an openrouter model with affinity on emits `x-session-id` and NOT the triple; an openai model emits the triple unchanged; `"openai-nosession"` emits `x-client-request-id` + `x-session-affinity` but not `session_id`.

## PROV-025 — `deferredToolsMode: "kimi"` unported; Kimi's provider-native deferred-tool serialization unavailable

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — `grep -c split_deferred_tools cyrup/crates/cyrup-provider/src/api/openai_completions.rs` = 0 — the completions impl never splits. `ModelCompat` (`compat.rs:73…`) has no `deferred_tools_mode`. Cyrup's deferred work (PROV-009, closed) implements exactly two renderings: `api/anthropic_messages.rs:890,1199-1201` and `api/openai_responses.rs:735-756`. This is a third rendering added upstream after the baseline, not a duplicate of PROV-009.

**upstream** — pi `f16b4e0c` "add deferred tools support for kimi in openai-completions api", post-baseline. `pi/packages/ai/src/types.ts` adds `deferredToolsMode?: "kimi"` to `OpenAICompletionsCompat`; `pi/packages/ai/src/api/openai-completions.ts:157,160` thread it, `:723` `const deferredNames = compat.deferredToolsMode === "kimi" ? getDeferredToolNames(context.messages) : new Set()`, `:1224` applies the serialization, `:1476`/`:1519` default and resolve it.

**Impact** — Kimi models (cyrup ships `kimi-coding.json`, `moonshotai.json`, `moonshotai-cn.json`) always receive the full tool schema set every turn, so the prompt-prefix cache churns and per-turn prompt tokens stay high on exactly the provider family upstream added the mode for. Cost and latency only — no wrong output.

**Fix** — Add `deferred_tools_mode: Option<DeferredToolsMode>` (a one-variant enum `Kimi`) to `ModelCompat`/`ResolvedCompat`, `None` in `detect_compat`, resolved in `get_compat`. In `openai_completions.rs::build_params`, when `Kimi`, call the already-ported `crate::utils::deferred_tools::split_deferred_tools(&ctx.messages, &ctx.tools, true, &identity)` and apply pi's serialization from `openai-completions.ts:723,1224`. Read `getDeferredToolNames` end-to-end first — it is a different accessor from `splitDeferredTools`, working off message names rather than the placement map.

**Verify** — Body test: a model with `compat: {"deferredToolsMode": "kimi"}` and a transcript whose tool result carries `addedToolNames: ["late"]` serializes `late` in Kimi's deferred form and omits it from the `tools` prefix; without the flag the full tool array is emitted unchanged.

## PROV-026 — TEST DEFECT: `seed_catalog_parses` pins the stale 200k Sonnet 4.5 context window that PROV-004 corrected everywhere else

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/catalog.rs:33-43` `seed_catalog_parses` asserts `assert_eq!(anthropic.context_window, 200_000)` against the sole anthropic entry of `cyrup/crates/cyrup-provider/src/catalog/seed.json` — `claude-sonnet-4-5`, `"baseUrl": "https://api.anthropic.com/v1/messages"` (`:7`), `"contextWindow": 200000` (`:19`), `"maxTokens": 64000` (`:20`). Commit 6d29542 corrected the same model to `1000000` in the real `providers/catalog/anthropic.json`, and `cyrup/crates/cyrup-provider/tests/catalog_data.rs` asserts that value. Both suites are green; the seed one asserts the wrong fact about the same model id.

**upstream** — `pi/packages/ai/src/providers/anthropic.models.ts` at `91585d9a` — `claude-sonnet-4-5` `contextWindow: 1000000, maxTokens: 64000`; the mechanical catalog diff reports zero field differences on anthropic, so cyrup's real catalog is right and the seed is wrong. Upstream has no `seed.json` analog at all; `ctx.modelRegistry.getAvailable()` is always the live registry.

**Impact** — Harmless as a fixture assertion in isolation, but the fixture is NOT test-only. `seed_catalog()` feeds six production call sites in `cyrup-ext-subagents` (PROV-007), including `provider_ranked_full_ids_from_catalog` (`extension.rs:6156-6181`), which reads `context_window`/`max_tokens`/`cost`/`reasoning` off these two entries to drive `filter_dominated` and write real profile files. The stale 200k propagates into subagent model ranking, and this green test is why nobody notices. Same shape as the three defects this project has already found.

**Fix** — Land with PROV-007, not before. Either (a) delete `seed.json` + `seed_catalog()` once the six production call sites move to `default_models(CreateModelsOptions::default()).get_models(None)` — also dropping the `pub use` at `cyrup-provider/src/lib.rs:54` — deleting this test with them; or (b) keep the fixture for tests only, `#[cfg(test)]`-gate `seed_catalog`, refresh `seed.json` from `providers/catalog/anthropic.json` (contextWindow 1000000, origin-only `baseUrl`) and flip the assertion to `1_000_000`. Do NOT flip the number alone while production still reads the seed.

**Verify** — After the fix, prove there are no non-test callers of `seed_catalog` (delete the symbol so the compiler proves it). Then assert that `seed_catalog()`'s anthropic entry and `default_models(..).get_model("anthropic","claude-sonnet-4-5")` agree on `context_window`, and that every `seed.json` `baseUrl` is origin-only — `tests/catalog_data.rs` already asserts non-empty baseUrls for the real catalogs, so the seed is the only place path-shaped baseUrls survive.

## Coverage

**Read at HEAD 1806375.** `cyrup-core/src/message.rs` in full (both hand-written serializers and all round-trip tests). In `cyrup-provider`: `model.rs`, `usage.rs`, `collection.rs`, `catalog.rs` + `catalog/seed.json`, `remote_catalog.rs`, `providers/catalog_manifest.json`, `provider.rs`, `validate.rs`, `stream.rs`, `stream/sse.rs`, `utils/retry.rs`, `utils/deferred_tools.rs`, `utils/refresh.rs`, `env_api_keys.rs`, `auth/mod.rs`, `api/mod.rs`, `api/compat.rs`, `api/openai_responses.rs` and `api/azure_openai_responses.rs` `build_params` end-to-end, and the deferred / thinking / session-affinity / finish_reason paths of `api/openai_completions.rs` and `api/anthropic_messages.rs`. Tests: `tests/catalog_data.rs`, `tests/thinking_max.rs`, `tests/remote_catalog.rs`. Traced outside the area to close PROV-009 honestly: `cyrup-ext/src/wrapper.rs:143`, `cyrup-ext/src/facade.rs:354`, `cyrup-agent/src/agent.rs:978-989`, `cyrup-agent/src/hooks.rs:181`, `cyrup-session-svc/src/event.rs:387-391`; and the six `seed_catalog` production call sites in `cyrup-ext-subagents/src/extension.rs`.

**Independent catalog verification.** PROV-004's closure was re-derived from scratch rather than reusing the prior pass's scripts: a fresh TS→JSON parser was written, all 35 `pi/packages/ai/src/providers/*.models.ts` were extracted at `pi@91585d9a` via `git ls-tree` + `git show`, and every field of every model was compared against the 30 corresponding cyrup catalogs. Result: 26 providers with zero diffs; `openai` with exactly 7, all `["gpt-5.4","compat","<absent>",{"supportsToolSearch":true}]`, traced via `git log -S supportsToolSearch` to pi `3d8f7435` "feat(ai): support message-anchored tool loading (#6474)" — the previous pass cited `4c186103`, which is a docs commit that touched no model data; corrected here. Zero id-set differences anywhere. Five pi files have no cyrup counterpart: four are PROV-005's unported providers, one is `together` (hand-written Rust by design). The same run independently validates the DATA halves of PROV-001 (tiers) and PROV-002 (thinkingLevelMap).

**Test-defect hunt, both shapes.** Shape 1 (pins current-but-wrong behavior): one found — PROV-026. Cleared candidates: `message.rs:677-698` asserts only `details < usage` positionally, which is true in pi too, so it does not pin PROV-020; `openai_completions.rs:2755` asserts "Stream ended without finish_reason", correct for the default compat and still correct after PROV-022; `openai_responses.rs:1558` asserts `max_output_tokens == 100`, still correct after PROV-019. Shape 2 (asserts an uncontrollable scheduling outcome): two candidates examined, both cleared — `utils/refresh.rs::concurrent_callers_share_one_fetch_then_retry` uses `tokio::join!` under `#[tokio::test]`'s default current-thread runtime, so both futures are polled in one task and the memo registers before the first await yields (deterministic, not filed); `tests/remote_catalog.rs` spawns a real TCP mock origin but every assertion is on response content, never ordering or elapsed time (not filed).

**Blind spots.** (1) The four large `api/*.rs` SSE decoders (~8k lines), `images/` and `faux.rs` are still not line-by-line diffed against pi; the per-event decode paths remain unaudited and per-event stream-fidelity bugs there would not have been caught by this pass. (2) No `cargo build/check/test/clippy` was run, per the task rules, so every fix sketch is unverified-by-compile and the suite's greenness at HEAD is assumed, not observed. (3) Four things the prior pass consciously declined to file — the Cloudflare baseUrl-substitution relocation, `StreamOptions.fetch`, and DRIFT-002/003 (which belong to area 12) — were not re-litigated; there is no evidence either way here. (4) The `:max` thinking-suffix parsing in `cyrup-ext-subagents`' `THINKING_LEVELS` (PROV-002's tail) belongs to area 09 and was not verified.

**Taken on trust.** That commit-message-only debt landing outside this area has been picked up by its owning areas: f777e44's deferred turn-boundary tool refresh (area 02 — its footprint is real and visible at `cyrup-session-svc/src/hooks.rs:163`, which documents the run-start tool snapshot), the deliberate WIT ABI break in both `on-tool-result` copies (area 06), pi's `getUsageCostBreakdown` "Tools/summaries" bucket (session/TUI), and 289c089's unported `operations` on `UserBashEventResult` (session-svc). None of it is filed here and all of it currently exists only in commit messages.



---

## Surface-sweep findings (2026-08-03, HEAD `9219dcd`)

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at
all, rather than checking a list of known items. That inversion exists because the
item-driven method missed pi's stray-OSC-reply swallow (`pi/packages/tui/src/tui.ts:788-794`)
— a real, user-reported bug — and by construction cannot see behaviour nobody wrote an item
for. IDs use an `-SNN` suffix to mark their provenance.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| PROV-S01 | medium | not-ported | M | Tool-argument coercion covers strictly less than pi's: no `additionalProperties` sub-schema, no tuple-form `items`, and no null/boolean<->number cross-coercions |
| PROV-S02 | low | not-ported | S | `getLastAssistantUsageInfo`'s newer-prefix-timestamp guard is unported in all three cyrup context estimators |
| PROV-S03 | low | not-ported | S | Two context-overflow detection patterns missing (23 of pi's 25) — DS4-style servers and DashScope/Qwen fail the turn instead of auto-compacting |
| PROV-S04 | low | not-ported | S | `estimateContextTokens`' message-anchored added-tool accounting unported — deferred tools loaded after the last usage block are invisible to every context estimate |
| PROV-S05 | low | not-ported | M | `Models.refresh` lost the post-baseline refresh contract: no `force`, no abort signal, no per-provider error map, no cache-restore-on-failure |

## PROV-S01 — Tool-argument coercion covers strictly less than pi's: no `additionalProperties` sub-schema, no tuple-form `items`, and no null/boolean<->number cross-coercions

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**upstream** — pi/packages/ai/src/utils/validation.ts:145-152 `applySchemaObjectCoercion` coerces every key NOT in `properties` against `schema.additionalProperties` when it is an object. `:156-165` `applySchemaArrayCoercion` handles `Array.isArray(schema.items)` positionally (tuple schemas) before the single-schema branch. `:58-130` `coercePrimitiveByType`: `null -> 0` for number/integer (:61-63,:76-78), `null -> false` for boolean (:91-93), `null -> ""` for string (:113-115), `boolean -> 1|0` for number/integer (:70-72,:85-87), `number 1|0 -> true|false` for boolean (:102-109), `""|0|false -> null` for type null (:121-126). Reached from `validateToolArguments` (:283) on the non-TypeBox branch — i.e. raw JSON Schema from extensions and MCP.

**cyrup** — ABSENT. 

**Impact** — A model emitting `{"limit": null}` against `{"type":"number"}`, or `{"recursive": 1}` against `{"type":"boolean"}` — routine LLM output — is a hard `ToolValidationError::Schema` in cyrup and a coerced successful call in pi. Dictionary-shaped args under `additionalProperties: {...}` pass through uncoerced. Tuple-form `items` gets no coercion at all. Blast radius is WASM/native extension tools and MCP tools, whose schemas are raw JSON Schema.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## PROV-S02 — `getLastAssistantUsageInfo`'s newer-prefix-timestamp guard is unported in all three cyrup context estimators

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/ai/src/utils/estimate.ts:63-87 — forward walk maintaining `latestPrefixTimestamp = Math.max(...)` (:83) and gating on `assistant.timestamp >= latestPrefixTimestamp` (:73), with the comment "A newer prefix message was inserted after this response (for example, a compaction summary), so its usage cannot describe the current prefix." Verified present at cyrup's baseline: `git -C pi show 91585d9a:packages/ai/src/utils/estimate.ts` lines 62-86 are identical. Stale port, not drift.

**cyrup** — ABSENT. 

**Impact** — 

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## PROV-S03 — Two context-overflow detection patterns missing (23 of pi's 25) — DS4-style servers and DashScope/Qwen fail the turn instead of auto-compacting

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/ai/src/utils/overflow.ts:55 `/prompt has [\d,]+ tokens?, but the configured context size is [\d,]+ tokens?/i` (DS4 server) — verified present at cyrup's baseline, `git -C pi show 91585d9a:packages/ai/src/utils/overflow.ts` line 54. STALE PORT. `overflow.ts:58` `/range of input length should be/i` (DashScope / Qwen Token Plan) — verified ABSENT at baseline (grep returns nothing). DRIFT.

**cyrup** — ABSENT. 

**Impact** — Against a DS4-style local OpenAI-compatible server an oversized prompt returns an error cyrup does not recognise, so the turn hard-fails and the user must `/compact` by hand; pi compacts and retries transparently. The DashScope half is latent until DRIFT-019 lands.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## PROV-S04 — `estimateContextTokens`' message-anchored added-tool accounting unported — deferred tools loaded after the last usage block are invisible to every context estimate

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/ai/src/utils/estimate.ts:118-132: when `estimate.lastUsageIndex !== null`, pi collects `addedToolNames` from every `toolResult` after that index, sizes exactly those tools via `estimateToolsTokens`, and adds it to both `tokens` and `trailingTokens`. Post-baseline addition (`3d8f7435`, message-anchored tool loading #6474); at 91585d9a the same site was a bare early return.

**cyrup** — ABSENT. 

**Impact** — cyrup does implement deferred/message-anchored tool loading (PROV-009/DRIFT-001, both closed: `addedToolNames` produced at `cyrup-ext/src/wrapper.rs:143`, persisted at `cyrup-session-svc/src/event.rs:387-391`), so tools genuinely arrive mid-conversation and their schema tokens are charged by the provider but counted by nobody. Under-reports the context estimate. Live blast radius is the same narrow set as finding 1 — chiefly `CompactionEntry.tokensBefore` — since `clamp_max_tokens_to_context` is unreachable (see finding 1's impact_correction). Under-counting is the safer direction of error.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## PROV-S05 — `Models.refresh` lost the post-baseline refresh contract: no `force`, no abort signal, no per-provider error map, no cache-restore-on-failure

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**upstream** — pi/packages/ai/src/models.ts:276-328 `refresh(options: ModelsRefreshOptions)` takes `{allowNetwork, force, signal}` (:46-51) and returns `ModelsRefreshResult {aborted, errors: ReadonlyMap<string, Error>}` (:53-56); resolves a credential per provider (OAuth refresh under the store lock, `resolveRefreshCredential` :330-354), passes `RefreshModelsContext` (:34-44) into `Provider.refreshModels`, collects per-provider errors instead of rejecting (:304-312), and on ANY failure re-invokes `refreshModels` with `allowNetwork: false` to restore the persisted catalog (:313-322). Verified pure drift: `git -C pi show 91585d9a:packages/ai/src/models.ts` has `async refresh(provider?: string): Promise<void>` with `Promise.allSettled` and no context object.

**cyrup** — ABSENT. 

**Impact** — A caller cannot cancel an in-flight refresh, cannot force past the freshness window, and cannot learn which provider failed — `refresh(None)` reports success unconditionally, so a wholly failed refresh is indistinguishable from a clean one. No cache-restore fallback. Genuinely narrower than the raw diff: cyrup relocated freshness/allow-network/persistence policy into `remote_catalog.rs:286+` and auth availability into `cyrup-config`, so this is the residue.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.


## GitHub Copilot findings (2026-08-11, HEAD `097bdde`)

The `github-copilot` provider did not exist when this file was written — `PROV-005` recorded it as
one of four missing providers, and it has since been ported (`providers/github_copilot.rs`,
`auth/oauth/github_copilot.rs`, `catalog/github-copilot.json`, registered at
`providers/all.rs:192-197`). These three items are gaps **in that new code**, found by reading the
port against pi rather than by re-checking a list. All three are `parity-bug`/`not-ported` against
the ported baseline, not upstream drift: every upstream line cited below is present at `v0.83.0`.

Copilot concentrates risk because it is the only built-in that (a) drives all three wire APIs from
one catalog — 9 rows `anthropic-messages`, 7 `openai-completions`, 12 `openai-responses` — and
(b) derives its request base URL from the credential rather than the model. Anything the API layer
special-cases on `provider === "github-copilot"` therefore has to be ported three times, and two of
the three call sites were missed.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| PROV-027 | high | parity-bug | S | Copilot's Claude models send `x-api-key`; pi's Copilot branch sends `Authorization: Bearer` |
| PROV-028 | high | not-ported | S | `github-copilot-headers.ts` unported — no `X-Initiator` / `Openai-Intent` / `Copilot-Vision-Request` on any of the three routes |
| PROV-029 | high | parity-bug | S | Copilot and Codex login flows are written but unreachable — the provider wires the runtime-half strategy and the flow registry has no production caller |

## PROV-027 — Copilot's Claude models send `x-api-key`; pi's Copilot branch sends `Authorization: Bearer`

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed

**upstream** — `pi/packages/ai/src/api/anthropic-messages.ts:867-888` opens with the comment
`// Copilot: Bearer auth, selective betas.` and branches on `model.provider === "github-copilot"`
*before* the OAuth-token test, constructing the client with `apiKey: null, authToken: apiKey` —
i.e. `Authorization: Bearer <copilot token>` — and only the selective betas
(fine-grained-tool-streaming / interleaved-thinking), deliberately **without** the Claude-Code
identity headers (`anthropic-beta: claude-code-…,oauth-…`, `user-agent: claude-cli/…`, `x-app: cli`)
that the OAuth branch at `:890-911` adds. Present at `v0.83.0` (`git -C pi show
v0.83.0:packages/ai/src/api/anthropic-messages.ts | grep -n Copilot` → `:867`), so this is a
port omission, not drift.

**cyrup** — `cyrup/crates/cyrup-provider/src/api/anthropic_messages.rs:493-534` `build_headers` has
**no provider branch at all**. The auth scheme is chosen solely by `is_oauth`, derived at `:159-163`
from `is_oauth_token(key)`, which is `api_key.contains("sk-ant-oat")` (`:435-437`). A Copilot token
is a claim string of the form `tid=…;exp=…;proxy-ep=proxy.individual.githubcopilot.com;st=dotcom` —
no `sk-ant-oat` substring — so `is_oauth` is `false` and `:527` emits
`x-api-key: tid=…;proxy-ep=…` where pi emits `authorization: Bearer …`.

**Impact** — All **9** Copilot Claude models (`claude-opus-4.5/4.6/4.7/4.8`,
`claude-sonnet-4/4.5/4.6/5`, `claude-haiku-4.5`) present the Copilot token in a header GitHub's
Anthropic-compatible edge does not read, i.e. every request on the `anthropic-messages` route
arrives unauthenticated. `claude-fable-5` is unaffected — it is an `openai-completions` row, which
uses a bearer by construction. The betas half of pi's branch is incidentally already correct: the
non-OAuth branch at `:524-530` emits exactly the selective set, so the only defect is the scheme.

**Fix** — In `build_headers` (`anthropic_messages.rs:493`), test
`model.provider.as_str() == "github-copilot"` **before** the `is_oauth` test and take the bearer
path without the Claude-Code identity headers, mirroring pi's ordering. Note the ordering matters
for its own sake: pi's Copilot branch precedes the OAuth branch, so a Copilot token that ever did
contain `sk-ant-oat` would still not acquire the Claude-Code identity.

**Verify** — Assert on emitted headers, both directions: a model with `provider ==
"github-copilot"` and a `tid=…` key yields `authorization: Bearer tid=…` and **no** `x-api-key`,
and its `anthropic-beta` contains neither `claude-code-20250219` nor `oauth-2025-04-20`; a
mirror case with `provider == "anthropic"` and the same key still yields `x-api-key`.

## PROV-028 — `github-copilot-headers.ts` unported — no `X-Initiator` / `Openai-Intent` / `Copilot-Vision-Request` on any of the three routes

**Kind** not-ported · **Severity** high · **Effort** S · **Confidence** confirmed

**upstream** — `pi/packages/ai/src/api/github-copilot-headers.ts` (37 lines, added `ba93da9a9`
2026-06-10, present at `v0.83.0`) exports `inferCopilotInitiator` (last message role `!== "user"` →
`"agent"`, else `"user"`), `hasCopilotVisionInput` (any `user` or `toolResult` message with an
`image` content part) and `buildCopilotDynamicHeaders`, which returns
`X-Initiator: user|agent`, `Openai-Intent: conversation-edits`, plus
`Copilot-Vision-Request: "true"` when images are present. It is imported and applied at **all
three** API impls: `anthropic-messages.ts:524-543`, `openai-completions.ts:646-652`,
`openai-responses.ts:224-230`, each guarded by `model.provider === "github-copilot"`.

**cyrup** — ABSENT. No counterpart file; `grep -rni "X-Initiator\|Copilot-Vision\|Openai-Intent"
crates/cyrup-provider/src` returns nothing outside the login flow's unrelated
`openai-intent: chat-policy` on the model-policy POST
(`auth/oauth/github_copilot.rs:666`). The only Copilot special-case that *was* ported into the API
layer is the reasoning-effort suppression at `api/openai_responses.rs:398`
(pi `openai-responses.ts:329`) — so the provider check exists on one of three routes and carries
none of these headers.

**Impact** — Two distinct consequences, and they fail differently. (1) **Images**: pi's own comment
is `// Copilot requires Copilot-Vision-Request header when sending images`; without it an image
turn against Copilot is rejected rather than degraded — a loud failure on a normal path, on all
three routes. (2) **`X-Initiator`**: this is how Copilot distinguishes a user-initiated request
from an agent follow-up, which is quota-relevant; omitting it does not fail, it silently
misreports every request in an agent loop as the wrong kind. The static headers Copilot's edge
also requires (`User-Agent`, `Editor-Version`, `Editor-Plugin-Version`, `Copilot-Integration-Id`)
*are* present, baked into every catalog row's `model.headers` — which is what makes this gap easy
to miss: Copilot traffic is not header-less, it is missing exactly the per-request three.

**Fix** — Port the module as `cyrup-provider/src/api/github_copilot_headers.rs` (three pure
functions over `&[Message]`, no I/O) and apply it at the three header builders:
`anthropic_messages.rs:493` `build_headers`, `openai_completions.rs` and
`openai_responses.rs:412` `build_headers`, guarded on `model.provider.as_str() ==
"github-copilot"`. Header precedence must place these where pi does — after `model.headers`, so the
dynamic set wins over the baked editor identity, and before `opts.headers`. Land with PROV-027;
they touch the same function on the Anthropic route.

**Verify** — Per route, assert the emitted header map: a Copilot text turn ending in a `user`
message has `X-Initiator: user` and no `Copilot-Vision-Request`; one ending in an assistant or
`toolResult` message has `X-Initiator: agent`; one containing an image part in a `user` **or** a
`toolResult` message has `Copilot-Vision-Request: true`; and a non-Copilot model on the same route
has none of the three. Run all four against each of the three API impls — the class of bug here is
"ported to one route, missed on the others".

## PROV-029 — Copilot and Codex login flows are written but unreachable: the provider wires the runtime-half strategy, and the flow registry has no production caller

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed

**upstream** — `pi/packages/ai/src/providers/github-copilot.ts:16` gives the provider
`oauth: lazyOAuth({ name: "GitHub Copilot", isSubscription: true, load: loadGitHubCopilotOAuth })`,
so `provider.auth.oauth.login` resolves — lazily — to the full flow in
`auth/oauth/github-copilot.ts:329-359` (`loginGitHubCopilot`). `providers/openai-codex.ts:15` does
the same with `loadOpenAICodexOAuth`. The laziness is a bundling concern only; the value is always
reachable.

**cyrup** — Both flows are **fully ported and cannot be called.** cyrup has two structs per
provider: a runtime half implementing only `refresh`/`to_auth`
(`providers/github_copilot.rs:410` `GitHubCopilotOAuth`, `providers/openai_codex.rs:276`
`OpenAiCodexOAuth`) and a login-capable flow (`auth/oauth/github_copilot.rs:393`
`GitHubCopilotLogin`, whose `login` at `:821-823` runs the whole device-code + policy-acceptance
grant; `auth/oauth/openai_codex.rs:516` `OpenAiCodexOAuthFlow`). The provider's `ProviderAuth`
wires the **runtime half** — `github_copilot.rs:142-147`, `openai_codex.rs:129-131` — and neither
runtime struct implements `login`, so it falls through to the trait default at `auth/mod.rs:127-131`,
`Err(OAuthError::LoginUnsupported)`. The flows are reachable only via
`OAuthFlowId::{GithubCopilot,OpenAiCodex}` through `load.rs`, and
`register_bundled_oauth_flow_loaders` (`auth/oauth/load.rs:111`) has **zero production callers** —
the only four call sites workspace-wide are its own tests (`load.rs:239,277,322`). The registry is
therefore always `OAuthFlowLoaders::default()`, all-`None`. Meanwhile `/login` resolves its
strategy from the provider: `cyrup-config/src/login.rs:778-784` calls
`provider.auth.oauth.login(interaction)`, fed by `cyrup-tui/src/app.rs:2441-2453`
(`provider_oauth_strategy`) and `:2395-2409` (`build_login_inputs`), both reading
`provider.provider_auth()`. Contrast the four providers that work: `anthropic`, `kimi-coding`,
`xai` and `openrouter` route through `providers/builtin_oauth.rs:37-55`
`builtin_provider_oauth`, which returns the login-capable struct directly — and
`builtin_oauth.rs:14-16` states the exemption in prose ("`github-copilot` … and `openai-codex` wire
their own OAuth inside …"), which is exactly where the two got dropped.

**Impact** — `/login` lists GitHub Copilot and OpenAI Codex (they advertise `is_subscription:
true`, so they also render with the subscription marker), the user selects one, and the flow ends
in `LoginUnsupported`. A credential for either provider can only arrive by hand-placing it in
`auth.json`. Both providers' login code is complete, tested at the loopback level, and dead. This
is the "advertised but rejected" shape, and it is invisible from the flow side: every test in
`auth/oauth/github_copilot.rs` drives `GitHubCopilotLogin` directly and passes.

**Relation to PROV-003** — `PROV-003` (zero OAuth login flows) is recorded as OPEN and
"deprioritised by the maintainer" in `00-residual-ledger.md`. That status predates `cf26010`, which
landed all 11 flows under `auth/oauth/`. This item is **not** a restatement of PROV-003: the flows
exist, and what is missing is one field assignment per provider. PROV-003's own status needs
re-auditing at HEAD — not done here.

**Fix** — Point both providers at the login-capable struct. Either extend
`builtin_provider_oauth` (`providers/builtin_oauth.rs:37`) with `"github-copilot"` and
`"openai-codex"` arms returning `GitHubCopilotLogin`/`OpenAiCodexOAuthFlow` and have the two
`*_auth()` constructors use it — deleting the prose exemption at `:14-16` — or have
`github_copilot_auth`/`openai_codex_auth` construct the flow struct directly. The flow structs
already delegate `refresh`/`to_auth` to the runtime halves
(`auth/oauth/github_copilot.rs:827-834`), so nothing else changes. Separately, either populate the
flow registry at startup or delete it: a registry with no production caller is a second, silent
way to reach the same dead end.

**Verify** — Two assertions, because either alone passes today. (1) Registry-independent: for every
provider in `all_providers()` whose `provider_auth().oauth` is `Some`, calling `login` with a
scripted interaction must not return `LoginUnsupported` — a table-driven test that fails when a
future provider repeats this. (2) End to end: drive `/login` for `github-copilot` against a
loopback GitHub and assert a credential lands in the store, the same way the flow's own
`login_completes_the_device_flow_and_records_available_models` test does, but entering through
`cyrup_config::login::login`.
