# 01 — cyrup-core + cyrup-provider

This area covers `cyrup/crates/cyrup-core` (message/type model, JSONL serialization) and `cyrup/crates/cyrup-provider` (wire APIs, providers, catalogs, auth, streaming, validation), measured against `pi/packages/ai/`, `pi/packages/agent/` and the provider-facing half of `pi/packages/coding-agent/src/core/`. The ported baseline is **pi `v0.83.0`**; post-baseline drift is measured against **pi `v0.84.1`**.

> **Re-audited 2026-08-12, cyrup HEAD `04c1ba2` (last code commit; docs HEAD `a9000b1`, branch `david/cyrup`, tree clean), against pi `v0.83.0` (ported baseline) and pi `v0.84.1` (latest).**
>
> **10 items closed** this pass — `PROV-006`, `PROV-008`, `PROV-010`, `PROV-012`, `PROV-022`, `PROV-026`, `PROV-S01`, `PROV-S02`, `PROV-S03` and, most consequentially, `PROV-007` (the two-model `seed.json` is physically gone). **1 item re-opened**: `PROV-004`, not as a refutation of the 2026-08-03 field diff but as *scope* — the catalog set grew from 30 to 35 and the five newest were never diffed, and by the constraint recorded in Coverage they can no longer be diffed from this workspace at all. **17 items newly filed** (`PROV-030` … `PROV-046`), one of them **high**.
>
> **The headline is `PROV-030`.** `google-vertex` is registered as a built-in provider with 10 catalog models, resolves auth including the ADC arm, appears in `/model` — and has no wire API implementation, so every request dies at `wire.rs` with `no API implementation for google-vertex`. That is the exact failure mode `PROV-005`'s own Fix text warned about for `bedrock-converse-stream`; it was fixed there and shipped here in the same sweep. `PROV-005` itself stays **closed** (both halves it asserted really do hold) — the defect is new and carries its own id, per the ledger's stable-id rule.
>
> **Two baseline corrections carried into this pass.** (1) This file previously measured against `pi@91585d9a` and declared itself re-baselined at cyrup `1806375`; both were stale. Everything below is measured against `v0.83.0`/`v0.84.1` at cyrup `04c1ba2`. That reclassification alone moves four items off `upstream-drift` and onto port bug (`PROV-021`, `PROV-023`, `PROV-024`, `PROV-025`) — the behaviour was available at the recorded baseline and was not taken. (2) The 2026-08-11 addendum's claim that OAuth is "entirely absent" is retired: `cf26010` landed 11 flow modules and `OAuthAuth::login`. `PROV-003` is now **partially closed**, not open-and-deprioritised.
>
> **Open set as of the re-audit: 36 items — 0 critical, 4 high, 12 medium, 20 low.** *(Superseded the same day by the repair pass recorded immediately below — the current figure is 40 counted + 1 tracker. This line is kept because the re-audit's 117-closed / 176-filed arithmetic is stated against it.)* All of them are in the single `## Open items` table below; the split-table hazard the previous revision warned about (structural defect A in `00-residual-ledger.md`) is retired by consolidation, not by a second warning.

> **REPAIR PASS 2026-08-12 (later same day), after the completeness critique.** Four things changed;
> **nothing was renumbered, merged or deleted** — `PROV-004` keeps its id and its body, and the one
> reclassification below is a marker, not a removal.
>
> 1. **Critic finding 8 applied (`PROV-030`).** Confirmed at HEAD: `providers/all.rs:176-197` really
>    does push all four providers, *and* the same file's port-status table at `all.rs:12-47` still
>    says `amazon-bedrock` (`:12`), `google-vertex` (`:23`) and `openai-codex` (`:34`) are
>    "**pending**", with the summary line at `:46-47` naming all four — `github-copilot` included,
>    even though the table row at `:21` says "ported" and the registration comment at `:192-193`
>    explicitly records that the table was stale and was left alone. An engineer opening the file
>    named by `PROV-030` reads that header first and concludes the item is wrong. The doc correction
>    is folded into `PROV-030`'s **Fix** as a mandatory part of that change rather than filed
>    separately, because it is a comment in the same file the fix edits.
> 2. **Critic finding 9 applied — full citation sweep of this file.** Every "identical at both tags"
>    / "@v0.83.0" / "@v0.84.1" claim was re-resolved by `git -C pi show <tag>:<path>` at the tag
>    actually named. **Nine items carried citations that are wrong at the tag they name**
>    (`PROV-003`, `PROV-009`, `PROV-016`, `PROV-020`, `PROV-023`, `PROV-024`, `PROV-028`,
>    `PROV-029`, `PROV-031`); three more were tightened (`PROV-030`, `PROV-032`, `PROV-046`); and one
>    **section-level** claim — "every upstream line cited is present at both v0.83.0 and v0.84.1",
>    over the whole Copilot block — was struck as false. Five of the nine are the exact `AGENT-020`
>    defect, a v0.84.1 offset asserted to hold at v0.83.0: `PROV-020`/`PROV-009`
>    (`agent-loop.ts:777-791` is v0.84.1; v0.83.0 is `:773-787`) and `PROV-028`
>    (`openai-completions.ts:646-652` is v0.84.1; v0.83.0 is `:638-645`). The worst
>    was `PROV-029`, a **high**, which quoted `isSubscription: true` from
>    `providers/github-copilot.ts:16` "@v0.83.0" — that property does not exist at v0.83.0; it is a
>    v0.84.1 addition. Every corrected line is listed under `## Coverage → Citation sweep`.
>    `PROV-019`, `PROV-027`, `PROV-011`, `PROV-042`, `PROV-045` and `PROV-034` re-verified **clean**
>    at both tags and are recorded there too, so they are not re-checked next pass.
> 3. **Five items absorbed from the `packages/ai/src/utils/` + `packages/coding-agent/src/bun/`
>    sweep** — the surface README blind spot 1 predicted and critique finding 11 named
>    (`sanitize-unicode.ts`, `json-parse.ts`, `node-http-proxy.ts`, `abort-signals.ts`,
>    `event-stream.ts`, `hash.ts`, `typebox-helpers.ts`, `provider-env.ts`; `bun/cli.ts`,
>    `register-bedrock.ts`, `restore-sandbox-env.ts`). Filed as **`PROV-047` … `PROV-051`**, two of
>    them **high**. `sanitizeSurrogates` lands here: the *outbound* direction is correctly ported as
>    a documented no-op, and the *inbound* direction — a lone surrogate escape arriving in a provider
>    SSE frame — kills the whole turn (`PROV-048`).
> 4. **One item reclassified as a `tracker` (critique finding 14's class).** `PROV-004` proposes no
>    work of its own — its entire Fix is "this is `PROV-018`'s `xtask gen-catalogs` and nothing else;
>    do not re-derive by hand". It keeps its id, its severity label and its body, gains a `tracker`
>    marker, and is **excluded from the severity counts**, because an item that schedules nothing is
>    bookkeeping, not backlog.
>
> **Open set after this repair: 40 items — 0 critical, 6 high, 14 medium, 20 low, plus 1 tracker
> (`PROV-004`, not counted).** 41 rows in the `## Open items` table.

## Status since the c8bd2ab baseline

| ID | Status | Evidence at cyrup `04c1ba2` / pi `v0.83.0`–`v0.84.1` |
|---|---|---|
| PROV-001 | closed | Holds. `ModelCostTier{input_tokens_above,input,output,cache_read,cache_write}` and `ModelCost.tiers` at `cyrup-provider/src/model.rs:20-49`; `select_rates` at `usage.rs:18-45` (strict `>`, highest matching threshold wins, tier REPLACES all four rates) and `compute_cost` at `:37-58` incl. the 1h cache-write rule. vs pi `types.ts:774-782` @v0.84.1 and `models.ts:639-659` @v0.83.0 `calculateCost` (`matchedThreshold = -1` seed). Statement-equivalent. |
| PROV-002 | closed | Holds. `ThinkingLevel{Minimal..Xhigh,Max}` at `cyrup-core/src/message.rs:30-40` and `ModelThinkingLevel` at `:46-56`, `Max` declared **last** in both so the ascending ladder the clamp walks is intact; `:86` maps `ThinkingLevel::Max => ModelThinkingLevel::Max`. vs pi `types.ts:82-83` @v0.84.1 and `models.ts:661` `EXTENDED_THINKING_LEVELS`. The `:max` suffix parsing in `cyrup-ext-subagents` remains area 09's. |
| PROV-003 | partially-closed | Login half landed: `auth/oauth/` holds 11 flow modules (anthropic, github_copilot, kimi_coding, openai_codex, openrouter, radius, xai + device_code/callback/pkce/page/query/sha256/random), and `OAuthAuth::login` is declared at `auth/mod.rs:118-131` with a `LoginUnsupported` default; `auth/oauth/github_copilot.rs:821` and `openai_codex.rs:1038` are real impls. Still open: `ApiKeyAuth` (`auth/mod.rs:59-70`) has only `name`+`resolve` and no `login`, where pi gives anthropic an api-key `login` (`providers/anthropic.ts:9-14` @v0.83.0) — the same hole `providers/google_vertex.rs:41-43` documents for `vertexAuth.login`; and `Models` has no `login`/`logout` (PROV-031). See also PROV-029 (two flows unreachable) and PROV-041 (the in-tree comment at `cyrup-ext-subagents/src/extension.rs:11300-11302` still asserts "cyrup ships no login flow at all", now false). |
| PROV-004 | **tracker** (re-opened 2026-08-12, low, **excluded from the counts**) | Not a refutation of the 2026-08-03 field diff, which stands for the 30 catalogs it covered. Scope: `providers/catalog/` now holds **35** files — `amazon-bedrock.json`, `github-copilot.json`, `google-vertex.json`, `openai-codex.json`, `openrouter-images.json` were added after that diff and none has been field-checked. `providers/google_vertex.rs:17-27` records its 10 rows as taken from pi `b0c2a90e` (2026-07-17), a different revision from the manifest's. Audit-coverage debt, not a demonstrated wrong value — and per Coverage constraint 4 it is no longer checkable from this workspace, so it is a verification task, not a fix task. Tracked further by PROV-038/PROV-039. |
| PROV-005 | closed (2026-08-11, re-confirmed 2026-08-12) | Both halves it asserted hold at HEAD. `api/mod.rs:130-163` `register_builtins` registers **9** factories incl. `BEDROCK_CONVERSE_STREAM` and `OPENAI_CODEX_RESPONSES`; `providers/all.rs:175-197` pushes `amazon-bedrock` `:177`, `openai-codex` `:183`, `google-vertex` `:187`, `github-copilot` `:194`. Read at HEAD, not from a commit message. **Follow-on defects in the code that closed it now number four: `PROV-027`/`028`/`029` (Copilot) and `PROV-030` (google-vertex has no wire api).** Re-opening this id to carry `PROV-030` was considered and rejected — it would double-count one defect and violate the stable-id rule. |
| PROV-006 | **closed** | `utils/provider_retry.rs` is a whole module: `DEFAULT_MAX_RETRY_DELAY_MS = 60_000` `:34`, `ProviderRetry::from_options` reading `StreamOptions.max_retries`/`max_retry_delay_ms` `:58-63`, retryable-status set + `Retry-After` honouring + interruptible sleep in the loop at `stream/sse.rs:300-355`. Consumed in production by **seven** api impls (`anthropic_messages.rs:210`, `openai_completions.rs:146`, `openai_responses.rs:207`, `azure_openai_responses.rs:184`, `google_generative_ai.rs:151`, `mistral_conversations.rs:153`, `pi_messages.rs:224`). Idle timeout: `sse.rs:55-92` `configure_http_idle_timeout`/`with_idle_timeout` over `reqwest::ClientBuilder::read_timeout`, per-request override `StreamOptions.timeout_ms` at `:152-157`, wired to settings at `cyrup-session-svc/src/builder.rs:1213-1214`. vs pi `utils/provider-retry.ts:1-45` @v0.83.0 (same 60s default, same 408/409/429/5xx + `x-should-retry` set) and `coding-agent/src/core/http-dispatcher.ts:4` `DEFAULT_HTTP_IDLE_TIMEOUT_MS = 300_000`. Tests at `sse.rs:853`, `:886`. **Residual filed separately: bedrock is the one impl with no retry — PROV-043.** |
| PROV-007 | **closed** | The 2-model stub is physically gone. There is no `cyrup-provider/src/catalog/seed.json` and no `seed_catalog()`; `rg --type rust seed_catalog crates/` returns only two descriptive comments (`cyrup-ext-subagents/Cargo.toml:39`, `tests/registration_commands_integration.rs:113`). The six production sites now go through `cyrup-ext-subagents/src/extension.rs:11306-11308` `registry_models()` → `cyrup-provider/src/catalog.rs:38-44` `builtin_catalog()` = `default_models(CreateModelsOptions::default()).get_models(None)`, guarded at `catalog.rs:52-75` (≥25 providers, google/mistral/groq/openrouter/together present, the real 1M Sonnet 4.5 window pinned). Residual carried forward as **PROV-031**: `builtin_catalog()` is pi's credential-blind `getModels()`, not `getAvailable()` (`models.ts:151-152` @v0.83.0). |
| PROV-008 | **closed** | `utils/error_body.rs:23` `MAX_PROVIDER_ERROR_BODY_CHARS = 4000`, `:41-50` `truncate_error_text` reproducing pi's `... [truncated N chars]` marker, `:57-59` `normalize_error_body` (trim then cap). Applied on the single non-2xx funnel at `stream/sse.rs:271` feeding `ProviderError::Http` at `:337-341`, and again at `api/bedrock_converse_stream.rs:507`. vs pi `utils/error-body.ts:16` and `extractBody` `:76-84` @v0.84.1. |
| PROV-009 | closed | Holds. Producer `cyrup-ext/src/wrapper.rs:137-143` (`additive_delta` guard → `result.added_tool_names = union_in_order(...)`, skipped when empty); serializer `cyrup-core/src/message.rs:729-733` emits `addedToolNames` only when non-empty. vs pi `packages/agent/src/agent-loop.ts:773-787` `createToolResultMessage`'s conditional spread (`:783`) **@v0.83.0**. *(Citation corrected in the 2026-08-12 repair pass: the previously recorded `:777-791` is the **v0.84.1** offset — the function moved from `:773` to `:777` between the tags while the body stayed byte-identical.)* |
| PROV-010 | **closed** | And then some. `cyrup-core/src/message.rs:163-188` `enum StopReason { Pending, Stop, Length, ToolUse, Error, Aborted, Deferred }` — `Pending` at `:166` with the pi non-terminal-partial citation, `Deferred` at `:188` (a v0.84.0 addition), `is_settled()` at `:195`. Exactly pi `types.ts:391` @v0.84.1. Round-trip proven at `cyrup-test-support/tests/deferred_interop.rs`. The *behaviour* half of `deferred` is not ported — see **PROV-040**. |
| PROV-011 | still-open (medium) | Unchanged, and the recorded scope was understated — see the widened body below: **four** affected sites, not two. |
| PROV-012 | **closed** | `cyrup-core/src/message.rs:479-492` declares `raw_stop_reason: Option<String>` in pi's slot with the citation; the hand-written serializer emits it at `:571-573`, `len` accounts for it at `:542`. vs pi `types.ts:426` @v0.84.1 (between `errorMessage?` and `timestamp`). Round-trips at `cyrup-test-support/tests/deferred_interop.rs:54`, `:70`. A live producer exists: `api/bedrock_converse_stream.rs:1762`/`:1792`/`:1895` carries it off the raw Bedrock stop reason, so the round-trip is not vacuous. **Caveat:** the in-tree doc at `message.rs:487-489` still says "cyrup does not set it yet on any decoder" — that comment is stale, not the closure; folded into PROV-041's citation cleanup. |
| PROV-013 | closed | Holds. `usage` is a skippable field on `Message::ToolResult` (`cyrup-core/src/message.rs:700-735`), `len` widened `:711-715`, emitted `:725-728` only when `Some`. vs pi `types.ts:436-437` @v0.84.1. |
| PROV-014 | partially-closed (medium) | `pi-messages` half **closed**: `api/pi_messages.rs` exists and is registered at `api/mod.rs:151` with the `PI_MESSAGES` constant in `lib.rs`. Provider half **still open, and it is a port bug, not drift** — pi `providers/all.ts` @v0.83.0 already registers `qwenTokenPlanProvider()`, `qwenTokenPlanCnProvider()` and `radiusProvider()`, and `env-api-keys.ts` @v0.83.0 already maps `QWEN_TOKEN_PLAN_API_KEY`, `QWEN_TOKEN_PLAN_CN_API_KEY`, `RADIUS_API_KEY`. cyrup `providers/all.rs:140-240` pushes none; `env_api_keys.rs:34-73` has no such arm; `providers/builtin_oauth.rs:17` states outright that radius has no built-in provider. Duplicates PARITY-GAPS PB-1/PB-2, both confirmed at HEAD. |
| PROV-015 | still-open (low) | `stream.rs:210-226` `ApiStreamOptions` now has **seven** variants (Anthropic, OpenAiResponses, AzureOpenAiResponses, OpenAiCodexResponses, Bedrock, Google, Mistral) — two more than when filed — and still no `OpenAiCompletions`. pi `types.ts:1-10` @v0.84.1 imports `OpenAICompletionsOptions` and keys it into `ApiOptionsMap`. More urgent than when filed: `thinkingBudgets` (v0.84.1) has nowhere to land without the variant. |
| PROV-016 | still-open (medium) | Unchanged in substance despite `validate.rs` being substantially rewritten: `:104-108` is still `schema.get("anyOf").or_else(|| schema.get("oneOf"))` and `rg 'allOf\|all_of' crates/cyrup-provider/src/validate.rs` is empty. pi `utils/validation.ts:14-16` + `:189-201` **@v0.83.0** runs `allOf` then INDEPENDENT `anyOf` then `oneOf`; the code is byte-identical at v0.84.1 but the block moves to `:196-208`. Stale port. |
| PROV-017 | still-open (low) | `provider.rs:17-52` — trait is `id()`, `models()`, `provider_auth()`, `get_model()`, `refresh_models()`, `stream()`; no `name`/`base_url`/`headers`. pi `models.ts:75-81` @v0.83.0 carries all four on `Provider`. Note the data already exists: `WireProvider::new` takes a display name (`providers/google_vertex.rs:117` passes "Google Vertex AI"); only the trait accessor is missing. |
| PROV-018 | still-open (medium) | Generator half genuinely absent — there is no `xtask` directory anywhere in the repo and no mechanical drift check. The provenance half has **degraded** since it was blessed: `providers/catalog_manifest.json` still says `generatedAt 2026-07-10T16:34:43Z` / `source pi@91585d9a…` / "the 31 embedded catalogs" against 35 files and a 2026-07-17 extraction. It is load-bearing at `providers/all.rs:78-94` → `remote_catalog.rs:188-196`. Split out as **PROV-039**. |
| PROV-019 | still-open (medium) | Both sites, both sub-divergences. `api/openai_responses.rs:356-358` and `azure_openai_responses.rs:383-385` insert `max_output_tokens` raw; `rg MIN_OUTPUT_TOKENS crates/cyrup-provider/src` finds only the unrelated 1024 answer floor in `utils/simple_options.rs:22`. pi `api/openai-responses.ts:32`,`:289-290` and `azure-openai-responses.ts:26`,`:292-293` — byte-identical at v0.83.0 AND v0.84.1. |
| PROV-020 | still-open (low) | Still open, and the in-tree comment justifying it is provably wrong. `cyrup-core/src/message.rs:716-734` emits `role, toolCallId, toolName, content, isError, details, usage, addedToolNames, timestamp`; pi's `createToolResultMessage` object literal (`packages/agent/src/agent-loop.ts:773-787` @**v0.83.0**; `:777-791` at v0.84.1 — the previously recorded number was the wrong tag's) orders `… details, usage, ...addedToolNames, isError, timestamp`, and pi's session write path is a bare `JSON.stringify`, so that literal IS the on-disk byte order. `isError` is three keys too early. |
| PROV-021 | **misdescribed** (still-open, medium) | Kind corrected: filed `upstream-drift`, it is a **port bug** against v0.83.0. pi `env-api-keys.ts:29` @v0.83.0 exports `ANTHROPIC_AUTH_TOKEN_ENV`, `:73-76` returns all three with the inline carve-out comment, `:147` implements the `getEnvApiKey` skip. cyrup: `rg ANTHROPIC_AUTH_TOKEN crates/` = 0 hits; `env_api_keys.rs:39` is `"anthropic" => Some(&["ANTHROPIC_OAUTH_TOKEN","ANTHROPIC_API_KEY"])`. |
| PROV-022 | **closed** | `api/compat.rs:86` `supports_finish_reason: Option<bool>` on `ModelCompat`, `:216` on `ResolvedCompat`, `:362` detected default `true`, `:407-409` resolved. Consumed at `api/openai_completions.rs:1547-1562` — the `!supports_finish_reason` inference arm sits **ahead of** the error branch, matching pi `openai-completions.ts:578-580`/`:584-586` @v0.84.1. Fixture at `openai_completions.rs:2633` sets `supports_finish_reason: Some(false)`. |
| PROV-023 | still-open (low) | Unchanged, **kind corrected to port bug**: pi `api/openai-responses.ts:75` `supportsExplicitPromptCacheMode: model.compat?.… ?? false` *(cited as `:72` before the repair pass — `:72` is `supportsStrictMode`)*, `:278`, `:285` `prompt_cache_options` are all present at v0.83.0. cyrup `openai_responses.rs:336-354` builds only `prompt_cache_key`/`prompt_cache_retention`/`store`; `ResolvedResponsesCompat` (`compat.rs:175-186`) carries four fields, none of them this. |
| PROV-024 | **misdescribed** (still-open, medium) | Two corrections. (1) Kind: pi `types.ts:569` @v0.83.0 already declares `sessionAffinityFormat` on `OpenAICompletionsCompat` (and `:579` on the responses compat) — port bug, not drift. (2) The item covers only openai-completions; the openai-**responses** route is equally wrong and in a worse way, split out as **PROV-033** because that side needs a field *deletion*. cyrup `openai_completions.rs:228-233` emits the fixed OpenAI triple gated on `send_session_affinity_headers`, with no way to select `openrouter` or `openai-nosession`. |
| PROV-025 | still-open (low) | Unchanged, **kind corrected to port bug**: pi `types.ts:567` @v0.83.0 already declares `deferredToolsMode?: "kimi"` on `OpenAICompletionsCompat`. cyrup: `rg 'deferred_tools_mode\|DeferredToolsMode' crates/` = 0 hits; `ModelCompat` (`compat.rs:73-167`) has no such member. |
| PROV-026 | **closed** | Struck — the artefact is physically gone. No `catalog/seed.json`, no `seed_catalog_parses`. The replacement guard is `catalog.rs:52-75`, which pins the real 1M Sonnet 4.5 window rather than the retired 200k. Matches correction 4 in `00-residual-ledger.md`. |
| PROV-027 | still-open (high) | `api/anthropic_messages.rs:470-536` `build_headers` has **no provider branch**; the scheme is chosen solely by `is_oauth`, derived at `:434-437` from `api_key.contains("sk-ant-oat")`, and the non-OAuth arm at `:524-531` emits `x-api-key`. pi `api/anthropic-messages.ts:866-888` @**both** v0.83.0 and v0.84.1 branches on `model.provider === "github-copilot"` **before** the OAuth test. Blast radius re-measured by parsing the catalog: `github-copilot.json` has 28 rows, exactly **9** on `anthropic-messages`. |
| PROV-028 | still-open (high) | `rg -i 'X-Initiator\|Copilot-Vision\|Openai-Intent' crates/cyrup-provider/src` returns only the login flow's unrelated `openai-intent: chat-policy` (`auth/oauth/github_copilot.rs:666`) and its test; there is no `api/github_copilot_headers.rs` and no dynamic-header call in any api impl. pi `api/github-copilot-headers.ts` @v0.83.0 exports `inferCopilotInitiator`/`hasCopilotVisionInput`/`buildCopilotDynamicHeaders`, applied under the Copilot guard in `anthropic-messages.ts:867-871`, `openai-completions.ts:638-645` *(corrected from `:646-652`, which is the v0.84.1 offset)* and `openai-responses.ts:223-230`. |
| PROV-029 | still-open (high) | Wiring re-traced, which is the part that could have refuted it. cyrup ships two Copilot OAuth types — `GitHubCopilotLogin` (`auth/oauth/github_copilot.rs`, real `login` at `:821`) and `GitHubCopilotOAuth` (`providers/github_copilot.rs:410`, refresh/to_auth only) — and `github_copilot_auth()` (`providers/github_copilot.rs:142-146`) wires the **second**. Same shape for Codex: `openai_codex_auth()` (`providers/openai_codex.rs:129-131`) wires `OpenAiCodexOAuth`, not `OpenAiCodexOAuthFlow` (`auth/oauth/openai_codex.rs:516`). `/login` resolves through `provider.provider_auth().oauth` (`cyrup-config/src/login.rs:784`), so both dead-end on the `LoginUnsupported` default at `auth/mod.rs:124-131`. `providers/builtin_oauth.rs:37-56` still has exactly four arms; `register_bundled_oauth_flow_loaders` (`auth/oauth/load.rs:111`) still has zero production callers. pi `providers/github-copilot.ts:16` and `openai-codex.ts:13` @v0.83.0 both carry `lazyOAuth({… load: load*OAuth })` *(the Codex line was recorded as `:15` and the quoted literal included the v0.84.1-only `isSubscription: true`; both corrected in the repair pass)*. |
| PROV-S01 | **closed** | All three halves ported. (a) `additionalProperties` sub-schema `validate.rs:363-370` (`.filter(\|s\| s.is_object())`, matching pi's `typeof === "object"` guard), tested `:630-650`. (b) tuple-form `items` `:172`, `:413`, test `:661`. (c) cross-coercions `:221` (Null→`""`), `:239-247` (`""`/`false`/`0`→null), `:277-278`, `:296-297`, `:319-330`. vs pi `utils/validation.ts:58-165` @v0.84.1; each arm carries its upstream citation in-tree. **One narrow permissiveness delta filed separately: PROV-046.** |
| PROV-S02 | **closed** | `utils/estimate.rs:121-148` — `latest_prefix_timestamp = i64::MIN` seed `:125`, `usage_applies_to_prefix = assistant.timestamp >= latest_prefix_timestamp` `:132`, `max()` update for every message `:145`. vs pi `utils/estimate.ts` `getLastAssistantUsageInfo` @v0.84.1 (`Number.NEGATIVE_INFINITY` seed, same gate, same forward walk); the in-tree `:64`/`:83` citations line up. |
| PROV-S03 | **closed, byte-compared** | `utils/overflow.rs:15-40` carries all **25** `OVERFLOW_PATTERNS` in pi's order — including both previously missing (`prompt has [\d,]+ tokens?, but the configured context size is [\d,]+ tokens?` DS4, and `range of input length should be` DashScope/Qwen) — plus the 3 `NON_OVERFLOW_PATTERNS` at `:43-48`. Identical set, order and comments to pi `utils/overflow.ts:37-62`/`:74-78` @v0.84.1. The Xiaomi MiMo length-stop case is also ported at `overflow.rs:100-108`. (pi's `isRecoverableLength` predicate remains absent — that is PARITY-GAPS VL-P10, not this item.) |
| PROV-S04 | still-open (low) | `utils/estimate.rs:179-186` — `estimate_context_tokens` is a bare early return on `last_usage_index.is_some()` with no added-tool accounting. pi `utils/estimate.ts:114-131` @v0.84.1 collects `addedToolNames` from every `toolResult` after that index, sizes exactly those tools, and adds the result to BOTH `tokens` and `trailingTokens`. |
| PROV-S05 | still-open (low) | Confirmed open; the proposed raise to medium was **rejected**. `collection.rs:317-337` `refresh(Option<&str>)` → `Result<(),ProviderError>` discards every result of its `join_all` and returns unconditional `Ok(())`; `provider.rs:44-48` `refresh_models()` takes no context. But the behaviours pi's options carry are largely reproduced by a different mechanism the original item did not mention: `crates/cyrup/src/provider.rs:71-130` splits pi's `refresh({allowNetwork:false})` restore from the network refresh, gates the network path on mode (`mode_refreshes_catalogs`, mirroring pi's rpc/interactive-only triggers) and restricts the fetch to configured providers exactly as pi's `resolveRefreshCredential` bail does. What genuinely remains is the `errors`/`aborted` result shape, `force`, and the abort signal — API-shape and error-reporting residue. Duplicates PARITY-GAPS PB-3. **Separate defect in the same lines: PROV-041.** |
| PROV-030 | **new — open (high)** | `google-vertex` provider registered with 10 models and no wire API. **Widened in the repair pass:** the same file's port-status doc table (`providers/all.rs:12-47`) still calls `amazon-bedrock`/`google-vertex`/`openai-codex` "pending (NOT registered)" although `:176-197` pushes all four — correcting it is now part of PROV-030's Fix. |
| PROV-031 | **new — open (low)** | `Models` has no `get_available`/`check_auth`/`login`/`logout`. |
| PROV-032 | **new — open (medium)** | `Provider::filterModels` unported; `filter_github_copilot_models` has zero production callers. |
| PROV-033 | **new — open (medium)** | openai-responses carries a `sendSessionIdHeader` flag pi **deleted**, and can never emit `x-session-id`. |
| PROV-034 | **new — open (low)** | openai-responses always emits `"strict": false`; pi omits the key unless `supportsStrictMode`. |
| PROV-035 | **new — open (medium)** | `core/cache-stats.ts` entirely unported — no cache-waste accounting, no cache-miss notices. |
| PROV-036 | **new — open (low)** | `getUsageCostBreakdown` unported — `/session` shows one cost total. |
| PROV-037 | **new — open (low)** | Two `auth-guidance.ts` formatters unported; the preflight's message text and OAuth-expiry branch diverge. |
| PROV-038 | **new — open (low)** | TEST DEFECT: the catalog roster guard compares an array against its own literal length. |
| PROV-039 | **new — open (low)** | `catalog_manifest.json` claims 31 catalogs from `pi@91585d9a`; it is the live overlay staleness floor. |
| PROV-040 | **new — open (low)** | `fetchDeferred`/`cancelDeferred` unported — the deferred data model round-trips but no handle can be redeemed. |
| PROV-041 | **new — open (low)** | Three false in-tree provenance citations, one of them a "1:1 port" claim for a function missing four features. |
| PROV-042 | **new — open (medium)** | `ModelsStreamTransforms.transformHeaders` unported — `before_provider_headers` has no seam. |
| PROV-043 | **new — open (low)** | Bedrock is the only api impl with no request retry; pi inherits the AWS SDK's 3-attempt default. |
| PROV-044 | **new — open (low)** | `AWS_BEDROCK_FORCE_HTTP1` unported and cyrup's client negotiates h2. |
| PROV-045 | **new — open (low)** | openai-responses `reasoning` branch drops pi's xAI `include` clause and its `reasoningSummary`-only trigger. |
| PROV-046 | **new — open (low)** | Tool-argument boolean coercion is more permissive than pi's — `"True"`/`" true "` accepted. |
| PROV-047 | **new — open (high)**, repair pass | `httpProxy` reaches only the streaming wire APIs; OAuth, the agent proxy transport and extension HTTP bypass it. |
| PROV-048 | **new — open (high)**, repair pass | A lone-surrogate `\uXXXX` escape in a provider SSE frame kills the whole turn; `JSON.parse` accepts it. |
| PROV-049 | **new — open (medium)**, repair pass | `repair_json`'s invalid-`\u` arm doubles the backslash where pi emits `\u` unchanged — divergent tool arguments. |
| PROV-050 | **new — open (medium)**, repair pass | `parse_partial` deletes every astral character written as a surrogate pair from recovered tool arguments. |
| PROV-051 | **new — open (low)**, repair pass | Codex header-phase timeout substituted with a whole-stream read timeout; pi's message and abort/timeout distinction lost. |

## Open items

> **This table is the complete open set for area 01 — 41 rows: 40 counted items plus the one
> `tracker` (`PROV-004`), including the `-S` surface-sweep ids, everything filed on 2026-08-12 and
> the five absorbed by the repair pass.** The previous revision split them across three tables and
> carried a warning about it; the warning is retired by consolidation. Bodies are grouped below by
> provenance (main / surface sweep / Copilot / 2026-08-12 / repair pass), but this is the only count
> that matters. **Counted set: 0 critical, 6 high, 14 medium, 20 low = 40.** The `tracker` row
> proposes no work and is deliberately outside that arithmetic.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| PROV-052 | **high** | parity-bug | S | The shipped binary's default model is the in-process **faux TEST provider** — a bare `cyrup -p hi` fails with the internal string `No more faux responses queued` — **new, observed 2026-08-13** |
| PROV-030 | high | not-ported | L | `google-vertex` is registered with 10 models and no wire API — every request dies with `NoApiImpl` |
| PROV-027 | high | parity-bug | S | Copilot's Claude models send `x-api-key`; pi sends `Authorization: Bearer` |
| PROV-029 | high | parity-bug | S | Copilot + Codex login flows written but unreachable; flow registry has no production caller |
| PROV-028 | high | not-ported | S | `github-copilot-headers.ts` unported — no `X-Initiator`/`Openai-Intent`/`Copilot-Vision-Request` |
| PROV-047 | high | parity-bug | M | `httpProxy` reaches only the streaming wire APIs — OAuth, the agent proxy transport and extension HTTP bypass it |
| PROV-048 | high | parity-bug | S | A lone-surrogate `\uXXXX` escape in a provider SSE frame kills the whole turn |
| PROV-003 | medium | not-ported | M | `ApiKeyAuth` has no `login`; `Models` has no `login`/`logout` (OAuth flow half now closed) |
| PROV-011 | medium | parity-bug | L | `constrainedSampling` / grammar-constrained tools not modeled — **four** affected sites |
| PROV-014 | medium | parity-bug | M | radius + qwen-token-plan ×2 unregistered (pi-messages half closed) |
| PROV-016 | medium | stale-port | S | Tool-argument coercion ignores `allOf` |
| PROV-018 | medium | tooling | M | No catalog generator, no drift check |
| PROV-019 | medium | stale-port | S | `max_output_tokens` floor of 16 unported in both Responses APIs |
| PROV-021 | medium | parity-bug | S | `ANTHROPIC_AUTH_TOKEN` bearer env unsupported |
| PROV-024 | medium | parity-bug | S | `sessionAffinityFormat` unported on openai-completions |
| PROV-032 | medium | not-ported | S | `Provider::filterModels` unported — Copilot filter has zero production callers |
| PROV-033 | medium | stale-port | S | openai-responses carries pi's deleted `sendSessionIdHeader`; `x-session-id` unreachable |
| PROV-035 | medium | not-ported | M | `cache-stats.ts` unported — no cache-waste accounting, no cache-miss notices |
| PROV-042 | medium | not-ported | M | `transformHeaders` unported — `before_provider_headers` has no seam |
| PROV-049 | medium | parity-bug | S | `repair_json`'s invalid-`\u` arm doubles the backslash where pi emits `\u` unchanged |
| PROV-050 | medium | parity-bug | S | `parse_partial` deletes astral characters written as surrogate pairs from recovered tool arguments |
| PROV-004 | **tracker** *(low, not counted)* | tooling | M | The five newest catalogs were never field-diffed (and no longer can be from this workspace) — proposes no work of its own; closed by PROV-018 |
| PROV-015 | low | not-ported | S | `ApiStreamOptions` has no `openai-completions` variant |
| PROV-017 | low | not-ported | S | `Provider` trait exposes no `name`/`base_url`/`headers` |
| PROV-020 | low | parity-bug | S | `toolResult` JSONL key order diverges: `isError` emitted too early |
| PROV-023 | low | parity-bug | S | `prompt_cache_options` unported — one-shot requests implicitly cache-write |
| PROV-025 | low | parity-bug | M | `deferredToolsMode: "kimi"` unported |
| PROV-031 | low | not-ported | M | `Models` has no `get_available`/`check_auth`/`login`/`logout` |
| PROV-034 | low | parity-bug | S | openai-responses always emits `"strict": false` |
| PROV-036 | low | not-ported | S | `getUsageCostBreakdown` unported — one cost total, no per-model breakdown |
| PROV-037 | low | not-ported | S | Two `auth-guidance.ts` formatters and the OAuth-expiry preflight branch unported |
| PROV-038 | low | test-defect | S | Catalog roster guard is a tautology; 5 catalogs get no per-field checks |
| PROV-039 | low | stale-port | S | `catalog_manifest.json` staleness floor predates the newest embedded data |
| PROV-040 | low | upstream-drift | M | `fetchDeferred`/`cancelDeferred` unported |
| PROV-041 | low | stale-port | S | False in-tree provenance citations, incl. a wrong "1:1 port" claim |
| PROV-043 | low | not-ported | S | Bedrock has no request retry where pi inherits the AWS SDK's default |
| PROV-044 | low | not-ported | S | `AWS_BEDROCK_FORCE_HTTP1` unported; client negotiates h2 with no override |
| PROV-045 | low | parity-bug | S | openai-responses `reasoning` branch drops the xAI `include` and the summary-only trigger |
| PROV-046 | low | parity-bug | S | Boolean tool-arg coercion accepts `"True"`/`" true "` where pi rejects the call |
| PROV-051 | low | parity-bug | S | Codex header-phase timeout substituted with a whole-stream read timeout; pi's message and abort/timeout distinction lost |
| PROV-S04 | low | not-ported | S | `estimateContextTokens`' message-anchored added-tool accounting unported |
| PROV-S05 | low | not-ported | M | `Models::refresh` has no `force`, no abort signal, no per-provider error map |

## PROV-003 — `ApiKeyAuth` has no `login`; `Models` has no `login`/`logout` (OAuth flow half closed)

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed (partially closed)

**cyrup** — `cyrup/crates/cyrup-provider/src/auth/mod.rs:118-131` — `OAuthAuth::login` now exists with a `LoginUnsupported` default, and `auth/oauth/` holds 11 flow modules with real `login` impls (`github_copilot.rs:821`, `openai_codex.rs:1038`). What remains: `auth/mod.rs:59-70` `trait ApiKeyAuth { fn name(); async fn resolve(); }` still has no `login`, so no api-key provider can prompt for a key; and `Models` (`collection.rs`) exposes neither `login` nor `logout`, so the crate boundary pi draws is not reproduced (PROV-031). `providers/google_vertex.rs:41-43` documents the same hole for `vertexAuth.login`.

**upstream** — `pi/packages/ai/src/providers/anthropic.ts:12-15` @v0.83.0 (inside `anthropicApiKeyAuth()`, `:9`) `login: async (interaction) => ({type:"api_key", key: await interaction.prompt({type:"secret", message:"Enter Anthropic API key"})})` on the api-key auth object; `pi/packages/ai/src/models.ts:168`,`:171` @v0.83.0 declare `Models.login(providerId, type, interaction)` and `Models.logout(providerId)`.

> **Citations corrected in the 2026-08-12 repair pass** (critique finding 9). Previously recorded as
> `anthropic.ts:9-14` (which cuts the `login` literal in half — `:9` is the enclosing function, the
> field is `:12-15`) and `models.ts:167`/`:170` (off by one at v0.83.0; the declarations are `:168`
> and `:171`). Re-resolved with `git -C pi show v0.83.0:<path>`; the code is unchanged at v0.84.1
> but the `models.ts` offsets there are `:194`/`:197`.

**Impact** — Interactive api-key entry has no provider-declared path: a provider cannot say "prompt the user for a key like this". OAuth providers are now largely reachable, so this is no longer the blocking gap it was filed as — but two of the flows that exist are still unreachable (PROV-029), and the `Models`-level entry points are absent.

**Fix** — Add `async fn login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, AuthError>` to `ApiKeyAuth` (`auth/mod.rs:59-70`) with a default that returns `LoginUnsupported`, mirroring the `OAuthAuth` shape already there; implement it for anthropic in `providers/anthropic.rs`. Add `Models::login`/`Models::logout` under PROV-031.

**Verify** — A no-credential api-key `login` for anthropic produces a stored credential that `auth/resolve.rs` then consumes with no env var present; every provider in `all_providers()` whose auth advertises a login type can actually be driven to a credential.

**Note** — The "deprioritised by the user, filed not scheduled" status recorded against this item predates `cf26010` and no longer describes what is left. The remaining work is small and unrelated to the flow port that was deprioritised.

## PROV-005 — Three of nine baseline wire APIs and four providers unimplemented

> **CLOSED 2026-08-11, re-confirmed 2026-08-12 at HEAD `04c1ba2`.** Both halves it asserted hold:
> nine registered factories (`api/mod.rs:130-163`) and all four providers pushed in
> `providers/all.rs:175-197`. The body below is the 2026-08-03 evidence, kept so the closure can be
> re-audited. **Follow-on defects in the code that closed it: `PROV-027`/`028`/`029` (Copilot) and
> `PROV-030` (google-vertex has no wire api). Re-opening this id to carry `PROV-030` was considered
> and rejected — one defect, one id.**

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/api/mod.rs:122-148` `register_builtins` registers exactly six factories (openai-completions, anthropic-messages, openai-responses, azure-openai-responses, google-generative-ai, mistral-conversations). `cyrup/crates/cyrup-provider/src/lib.rs:166-174` `known_api` declares 7 constants, the 7th being `BEDROCK_CONVERSE_STREAM` — a dangling declaration with no registered factory.

**upstream** — `pi/packages/ai/src/types.ts:16-26` `KnownApi` lists 10 text wire APIs; `pi/packages/ai/src/providers/all.ts` ships 38 built-ins.

**Impact** — Bedrock, Copilot, Vertex and Codex users cannot use cyrup at all. Worse, a `models.json` custom provider can select `bedrock-converse-stream` and get a runtime "no factory" failure rather than a config-time rejection.

**Fix** — S-sized mitigation first: gate or delete `known_api::BEDROCK_CONVERSE_STREAM` so an api with no factory cannot be selected. Full fix: port the three missing `ApiImpl`s, register them, add the four catalogs.

**Verify** — A `models.json` naming an unregistered api is rejected at load; each new api round-trips a faux stream.

## PROV-011 — `constrainedSampling` / grammar-constrained tools not modeled — four affected sites, not two

**Kind** parity-bug · **Severity** medium · **Effort** L · **Confidence** confirmed

**cyrup** — `rg --type rust 'constrained_sampling|supports_strict_tools|supports_openai_grammar_tools' crates/` returns **zero hits workspace-wide**. The scope recorded when this was filed (anthropic-messages + openai-completions) is understated; there are four consuming sites upstream and all four are unported. Two of them are actively wrong rather than merely absent: `api/openai_responses.rs:810-828` hard-codes `strict: false` on every tool (PROV-034), and `api/google_generative_ai.rs:321-332` maps `tool_choice` only, so it can never emit `VALIDATED`. Do not confuse this with `supports_strict_mode`, which *is* ported on the completions compat (`compat.rs:109`/`:225`, consumed at `openai_completions.rs:694`) — that flag is not the constrained-sampling resolver.

**upstream** — `pi/packages/ai/src/api/constrained-sampling.ts` @v0.83.0 is a dedicated module exporting `resolveJsonSchemaStrictSampling` (`:84`), `resolveGrammarConstrainedSampling` (`:101`) and `createGrammarToolInputProperties` (`:136`). Consumed at (1) `anthropic-messages.ts`, (2) `openai-completions.ts`, (3) `openai-responses.ts:262-272`,`:301-306` + `openai-responses-shared.ts:344-378` (`convertResponsesTools` with `supportsStrictMode`/`supportsOpenAIGrammarTools`), (4) `google-shared.ts:311-323` `resolveGoogleFunctionCallingMode`, which returns `FunctionCallingConfigMode.VALIDATED` when any tool resolves strict.

**Impact** — Models that can be grammar-constrained still emit free-form tool arguments, so the malformed-argument retries pi avoids by construction still occur. The Google leg additionally never asks Gemini to validate function calls against the declared schema, which is the one route where upstream gets a server-side guarantee rather than a hint.

**Fix** — Add `constrained_sampling` to `cyrup_core::Tool`; port `constrained-sampling.ts` as `cyrup-provider/src/utils/constrained_sampling.rs`; add `supports_strict_tools` / `supports_openai_grammar_tools` to `ModelCompat`/`ResolvedCompat`/`ResolvedResponsesCompat`; apply in `anthropic_messages.rs::convert_tools`, `openai_completions.rs::convert_tools`, the new `convert_responses_tools` options struct (PROV-034 creates the landing point), and a `resolve_google_function_calling_mode` in `google_generative_ai.rs:321-332`.

**Verify** — Per route: a tool declaring `constrainedSampling` on a strict-capable model serializes the merged strict schema byte-equal to pi's, and on a non-capable model the schema is unchanged; the Google route emits `functionCallingConfig.mode: "VALIDATED"` exactly when `resolveGoogleFunctionCallingMode` would.

## PROV-014 — radius + qwen-token-plan ×2 unregistered (a v0.83.0 port bug, not lag)

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** confirmed (partially closed)

**cyrup** — `pi-messages` is done: `api/pi_messages.rs` exists and is registered at `api/mod.rs:151`. The three providers are not: `providers/all.rs:140-240` pushes no `radius`, `qwen-token-plan` or `qwen-token-plan-cn`; `env_api_keys.rs:34-73` `api_key_env_vars` has no arm for any of them; there are no catalog files. `providers/builtin_oauth.rs:17` states outright that radius has no built-in provider.

**upstream** — `pi/packages/ai/src/providers/all.ts` @**v0.83.0** already registers `qwenTokenPlanProvider()`, `qwenTokenPlanCnProvider()` and `radiusProvider()`; `pi/packages/ai/src/env-api-keys.ts` @v0.83.0 already maps `QWEN_TOKEN_PLAN_API_KEY`, `QWEN_TOKEN_PLAN_CN_API_KEY` and `RADIUS_API_KEY`. All three predate the ported baseline — this is a port omission, not expected lag, and the item's original `upstream-drift` classification was wrong.

**Impact** — Three providers are unreachable. A user with `RADIUS_API_KEY` or a Qwen token plan gets "not configured" in an environment where pi works.

**Fix** — qwen-token-plan ×2 are cheap: two `providers/fleet.rs` members, two catalogs, two `env_api_keys.rs` arms. radius additionally needs its OAuth flow wired — the flow module already exists at `auth/oauth/radius.rs`, so this is a `builtin_oauth.rs` arm plus a provider constructor, the same shape as PROV-029's fix.

**Verify** — Each provider resolves from its env var and streams against a faux origin; `api_key_env_vars` reports the new variables; the roster test (PROV-038, once it walks the directory) covers the new catalogs automatically.

**Note** — Duplicates PARITY-GAPS PB-1/PB-2, both re-confirmed at HEAD. Fix once, close both.

## PROV-016 — Tool-argument coercion ignores `allOf`; treats `anyOf`/`oneOf` as alternatives

**Kind** stale-port · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/validate.rs:104-108` is `schema.get("anyOf").or_else(|| schema.get("oneOf"))` — the two are mutually exclusive alternatives; `rg 'allOf|all_of' crates/cyrup-provider/src/validate.rs` returns nothing. The module was substantially rewritten since this was filed (see PROV-S01, closed) and these lines were not touched.

**upstream** — `pi/packages/ai/src/utils/validation.ts:14-16` @**v0.83.0** declares all three (`allOf?` `:14`, `anyOf?` `:15`, `oneOf?` `:16`); `:189-201` @v0.83.0 — inside `coerceWithJsonSchema` (`:186`) — runs a sequential `allOf` merge over each nested schema (`:189-193`), then an INDEPENDENT (non-`else`) `anyOf` pass (`:195-197`), then an independent `oneOf` pass (`:199-201`). **The code is byte-identical at v0.84.1 but the offsets are not:** `validation.ts:14-16` is unchanged, while the coercion block moves to `:196-208` (`coerceWithJsonSchema` at `:193`). Cite the v0.83.0 numbers — that is the tag this item is classified against.

**Impact** — Tool arguments whose schema uses `allOf` composition (common in generated and MCP schemas) are not coerced, so string-typed numbers and booleans reach the tool uncoerced and it errors on input pi would have accepted. Schemas carrying both `anyOf` and `oneOf` get only the first applied.

**Fix** — In `validate.rs:104-108`, replace the `or_else` with three sequential independent passes mirroring `validation.ts:189-201` @v0.83.0.

**Verify** — Unit tests beside the existing coercion suite: an `allOf`-composed object coerces every branch's properties; a schema carrying both `anyOf` and `oneOf` applies both.

## PROV-018 — No catalog generator and no drift check

**Kind** tooling · **Severity** medium · **Effort** M · **Confidence** confirmed (partially closed)

**cyrup** — Generator half absent: there is no `xtask` directory anywhere in the repo and no mechanical diff against a named pi revision. `crates/cyrup-provider/tests/catalog_data.rs` is a roster-count / non-empty guard with hand-picked spot values, and its roster assertion is itself defective (PROV-038). The provenance half landed but has since degraded — split out as PROV-039 so this item stays scoped to tooling.

**upstream** — pi's catalog data is generated and gitignored (`pi/.gitignore:11`); every `pi/packages/ai/src/providers/*.models.ts` @v0.84.1 is a two-line re-export of `./data/<provider>.json`, produced by `npm run generate-models`. There are 39 such modules at v0.84.1 against cyrup's 35 embedded catalogs.

**Impact** — Nothing warns when the embedded catalogs fall behind pi. Users get stale context windows, stale pricing and missing models with no signal. This is how PROV-004 arose the first time and how it has now re-opened at a different scope.

**Fix** — Add `cyrup/xtask` with `gen-catalogs` that runs pi's `npm run generate-models` (the tree can no longer simply be read), consumes `packages/ai/src/providers/data/*.json` plus the image models, emits every `providers/catalog/*.json`, and **rewrites `catalog_manifest.json`** (PROV-039 depends on that write). Add an `#[test] #[ignore]` drift check that re-runs the generator into a temp dir and diffs.

**Verify** — `cargo xtask gen-catalogs` against a named pi tag reproduces the current tree byte-for-byte; the ignored drift test fails when pointed at a newer pi.

## PROV-019 — `max_output_tokens` floor of 16 unported in BOTH Responses APIs

**Kind** stale-port · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/api/openai_responses.rs:356-358`: `if let Some(max) = opts.max_tokens { obj.insert("max_output_tokens", json!(max)); }` — raw, no floor. Identically at `azure_openai_responses.rs:383-385`. `rg MIN_OUTPUT_TOKENS crates/cyrup-provider/src` finds only `utils/simple_options.rs:22`, an unrelated 1024 answer floor. Two sub-divergences in the same three lines: (a) no `max(v, 16)` clamp; (b) cyrup gates on `Some(_)` where pi gates on JS truthiness, so `max_tokens: Some(0)` emits `"max_output_tokens": 0` where pi omits the key entirely.

**upstream** — `pi/packages/ai/src/api/openai-responses.ts:32` `const OPENAI_RESPONSES_MIN_OUTPUT_TOKENS = 16` (with the issue link in the comment above it) and `:289-290` `if (options?.maxTokens) params.max_output_tokens = Math.max(options.maxTokens, OPENAI_RESPONSES_MIN_OUTPUT_TOKENS)`. Same constant and clamp at `azure-openai-responses.ts:26`,`:292-293`. **Byte-identical AND at identical offsets at v0.83.0 and v0.84.1 — re-verified line-for-line in the 2026-08-12 repair pass** (critique finding 9; this item's citations were among the ones checked and they are clean, so do not re-derive them). A stale port.

**Impact** — Any caller setting a small `max_tokens` on an `openai-responses` or `azure-openai-responses` model gets a hard HTTP 400 and a failed turn where pi silently clamps and succeeds. pi added the clamp in response to a filed issue, so the path is reached in practice; the likely producers are compaction/summary calls and small user overrides, neither exotic.

**Fix** — Add `const OPENAI_RESPONSES_MIN_OUTPUT_TOKENS: u64 = 16;` to each file and change both sites to `if let Some(max) = opts.max_tokens.filter(|m| *m > 0) { obj.insert("max_output_tokens", json!(max.max(OPENAI_RESPONSES_MIN_OUTPUT_TOKENS))); }` — the `.filter` reproduces pi's truthiness gate. Cite the pi lines and the issue link, as pi does.

**Verify** — Extend the existing body-shape test (`openai_responses.rs`, today asserting `max_tokens: Some(100)` ⇒ `100`, which stays valid): `Some(4)` ⇒ `16`; `Some(0)` ⇒ key ABSENT; `None` ⇒ absent. Mirror all three in `azure_openai_responses.rs`.

## PROV-021 — `ANTHROPIC_AUTH_TOKEN` bearer-token env unsupported

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

> **Kind corrected this pass.** Filed as `upstream-drift`; the support is present at the recorded
> v0.83.0 baseline, so it is a port omission.

**cyrup** — `rg --type rust ANTHROPIC_AUTH_TOKEN crates/` returns zero hits workspace-wide. `cyrup/crates/cyrup-provider/src/env_api_keys.rs:39` is `"anthropic" => Some(&["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"])`. `providers/anthropic.rs` resolves only into `ModelAuth.api_key` (→ `x-api-key`); no env path produces an `Authorization: Bearer` header.

**upstream** — `pi/packages/ai/src/env-api-keys.ts:29` @v0.83.0 exports `ANTHROPIC_AUTH_TOKEN_ENV`; `:73-76` returns all three vars for anthropic with the inline carve-out comment that it "participates in env discovery/status, but `getEnvApiKey()` skips it because requests must pass it as `Authorization: Bearer`"; `:147` implements that carve-out (`envKeys.find(key => key !== ANTHROPIC_AUTH_TOKEN_ENV)`); `providers/anthropic.ts` resolves it BEFORE the other two into `{ auth: { headers: { Authorization: "Bearer …" } }, source: ANTHROPIC_AUTH_TOKEN_ENV }`.

**Impact** — `ANTHROPIC_AUTH_TOKEN` is the standard variable for Anthropic-compatible gateways and proxies that authenticate with a bearer token rather than `x-api-key`. A user with only that set gets "not configured" from cyrup in an environment where pi works. It also affects auth STATUS reporting, since `env_api_keys.rs` is what the login/status pickers consult.

**Fix** — (1) `env_api_keys.rs:39` → `Some(&["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"])`, reproducing pi's `getEnvApiKey` carve-out wherever the first element would otherwise be turned into a literal api key. (2) `providers/anthropic.rs` — replace `env_key([...])` with a bespoke `ApiKeyAuth` (in-tree template: `providers/cloudflare.rs:71-113`) that, after the stored-credential branch, probes `ANTHROPIC_AUTH_TOKEN` first and returns `AuthResult { auth: ModelAuth { api_key: None, headers: Some({"Authorization": Some(format!("Bearer {t}"))}), base_url: None }, source: Some("ANTHROPIC_AUTH_TOKEN") }`, falling through to the existing two.

**Verify** — With only `ANTHROPIC_AUTH_TOKEN=t`, resolve yields `Authorization: Bearer t` and NO `x-api-key`; with both it and `ANTHROPIC_API_KEY`, the bearer wins; with only `ANTHROPIC_API_KEY`, behaviour is unchanged; `api_key_env_vars("anthropic")` reports all three.

## PROV-024 — `sessionAffinityFormat` unported on openai-completions

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

> **Kind corrected this pass** (was `upstream-drift`; the field exists at v0.83.0), and **scope
> split**: the openai-responses route is a separate, worse defect requiring a field deletion, filed
> as **PROV-033**. Land both together so there is one enum.

**cyrup** — `cyrup/crates/cyrup-provider/src/api/openai_completions.rs:228-233` — when affinity is on, cyrup unconditionally injects the three OpenAI headers `session_id`, `x-client-request-id`, `x-session-affinity`, with no provider branch. `api/compat.rs:108` carries only the boolean `send_session_affinity_headers`; `rg session_affinity_format crates/` returns nothing. Only pi's `openai` format is expressible; `openrouter` and `openai-nosession` are unreachable.

**upstream** — `pi/packages/ai/src/types.ts:569` @**v0.83.0** declares `sessionAffinityFormat?: SessionAffinityFormat` on `OpenAICompletionsCompat` (the doc block above it spells out all three header sets), and `:579` puts it on `OpenAIResponsesCompat`. `pi/packages/ai/src/api/openai-completions.ts:647-656` @v0.83.0 branches: `openrouter` sends ONLY `x-session-id` (`:648-649`); `openai` adds `session_id` (`:651-652`); both non-openrouter forms send `x-client-request-id` + `x-session-affinity`. `:1473` auto-detects `sessionAffinityFormat: isOpenRouter ? "openrouter" : "openai"` (`isOpenRouter` defined `:1404`); `:1515` resolves the catalog override.

> **Citations corrected in the 2026-08-12 repair pass** (critique finding 9). Every upstream number
> in this item was wrong at the tag it named, and none of the wrong ones matched v0.84.1 either —
> they appear to come from an intermediate revision. Re-resolved with
> `git -C pi show v0.83.0:<path>`: `types.ts:578` → `:579`; `openai-completions.ts:650-659` →
> `:647-656`; `:1477` → `:1473`; `:1520` → `:1515`. **v0.84.1 offsets for the same, byte-identical
> code:** `types.ts:595`/`:605`; `openai-completions.ts:655-664`, `:1527`, `:1572`.

**Impact** — Latent on the shipped catalogs — `send_session_affinity_headers` is set only by `fireworks.json`, `cloudflare-ai-gateway.json` and `cloudflare-workers-ai.json`, all of which correctly want the `openai` form — but wrong by construction. A user `models.json` (or the next catalog refresh, since pi's generator emits the field) written against pi's documented schema is silently ignored, and an OpenRouter completions model gets three headers it does not read while missing the one it does, losing sticky routing and its prompt-prefix cache hit rate.

**Fix** — Add `SessionAffinityFormat { Openai, OpenaiNosession, Openrouter }` beside `CacheControlFormat`/`ThinkingFormat` in `compat.rs`, add `session_affinity_format: Option<..>` to `ModelCompat` and the resolved field to `ResolvedCompat`, auto-detect `Openrouter` in `detect_compat`, resolve the override in `get_compat`. Branch `openai_completions.rs:228-233` exactly as `openai-completions.ts:647-656` @v0.83.0.

**Verify** — Extend the header test at `openai_completions.rs:2807`: an openrouter model with affinity on emits `x-session-id` and NOT the triple; an openai model emits the triple unchanged; `"openai-nosession"` emits `x-client-request-id` + `x-session-affinity` but not `session_id`.

## PROV-004 — The five newest catalogs were never field-diffed, and no longer can be from this workspace

**Kind** tooling · **Severity** low · **Effort** M · **Confidence** confirmed · **`tracker` — excluded from the severity counts**

> **Reclassified as a `tracker` in the 2026-08-12 repair pass** (critique finding 14's class). The id,
> the severity label and the body below are all unchanged and retained; what changed is that this row
> no longer counts as backlog. The reason is its own **Fix**: it proposes no work of its own —
> "This is PROV-018's `xtask gen-catalogs` and nothing else… do not re-derive by hand". An item whose
> entire remedy is another item's remedy is bookkeeping: it records a known coverage hole and names
> its owner. Scheduling it produces nothing that scheduling `PROV-018` does not. It stays in the table
> so the coverage hole is not forgotten, and it stays out of the arithmetic so the count means work.
> **If `PROV-018` lands, close this by running the diff, not by re-auditing.**

> **Re-opened 2026-08-12 as scope, not as a refutation.** The 2026-08-03 from-scratch field diff of
> 30 catalogs against pi @`91585d9a` stands exactly as recorded: 26 providers zero diffs, `openai`
> exactly 7 (`supportsToolSearch`, a deliberate forward-port), zero id-set differences.

**cyrup** — `crates/cyrup-provider/src/providers/catalog/` now holds **35** files. Five were added after that diff and have never been compared field-by-field against upstream: `amazon-bedrock.json` (109 rows, the largest cyrup ships), `github-copilot.json` (28), `google-vertex.json` (10), `openai-codex.json`, `openrouter-images.json`. They were also extracted at differing revisions — `providers/google_vertex.rs:17-27` records `pi b0c2a90e` (2026-07-17) against the manifest's `91585d9a` (2026-07-10).

**upstream** — Not obtainable. `pi/.gitignore:11` excludes `packages/ai/src/providers/data/*.json`, and every `packages/ai/src/providers/*.models.ts` at v0.83.0 and v0.84.1 is a two-line re-export of that gitignored data. The original diff was possible only because the data was still committed at `91585d9a`; it cannot be reproduced today at any tag.

**Impact** — Audit-coverage debt rather than a demonstrated defect: no wrong value has been found in the five, and none can be found without running pi's generator. But this is precisely the surface where PROV-004 originally found drift, and it is now the least-reviewed data in the crate — including whether pi's generator sets `supportsStrictMode`, `sessionAffinityFormat` or `supportsExplicitPromptCacheMode` on rows where cyrup's copies do not (PROV-023/024/033/034 all become partly data problems if so).

**Fix** — This is PROV-018's `xtask gen-catalogs` and nothing else: once the generator exists, the diff is a command rather than an audit. Until then, do not re-derive by hand — record the constraint and move on.

**Verify** — `cargo xtask gen-catalogs` against the pi revision each provider module names reproduces all 35 files byte-for-byte.

## PROV-015 — `ApiStreamOptions` has no `openai-completions` variant

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/stream.rs:210-226` `enum ApiStreamOptions` has seven variants (Anthropic, OpenAiResponses, AzureOpenAiResponses, OpenAiCodexResponses, Bedrock, Google, Mistral) — two more than when this was filed — and still no `OpenAiCompletions`.

**upstream** — `pi/packages/ai/src/types.ts:1-10` @v0.84.1 imports `OpenAICompletionsOptions` from `./api/openai-completions.ts` and keys it into `ApiOptionsMap`.

**Impact** — Callers cannot pass openai-completions-specific per-request options. Now more pressing than when filed: `thinkingBudgets` (pi `openai-completions.ts`, v0.84.1) has nowhere to land without the variant, so the next completions-only option cannot be ported at all until this is done.

**Fix** — Add the variant at `stream.rs:210-226` and destructure it in `openai_completions.rs::build_params`. Purely additive: the prerequisite (`reasoning_effort` covering Minimal..Max) is already satisfied.

**Verify** — A completions-only option set through the new variant reaches the request body; the other apis reject it as before.

## PROV-017 — `Provider` trait exposes no `name` / `base_url` / `headers`

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/provider.rs:17-52` — the trait is `id()`, `models()`, `provider_auth()` (defaulted `None`), `get_model()`, `refresh_models()`, `stream()`. No `name`, `base_url` or `headers`.

**upstream** — `pi/packages/ai/src/models.ts:75-81` @v0.83.0 — `Provider` carries `readonly id`, `readonly name`, `readonly baseUrl?`, `readonly headers?: ProviderHeaders`.

**Impact** — Provider pickers and status output can only show the machine id, and provider-level default headers or base URL must be duplicated per model. The oddity is that the data already exists — `WireProvider::new` takes a display name (`providers/google_vertex.rs:117` passes `"Google Vertex AI"`) — and is simply not reachable through the trait.

**Fix** — Add defaulted `fn name(&self) -> &str { self.id().as_str() }`, `fn base_url(&self) -> Option<&str> { None }` and `fn headers(&self) -> Option<&HeaderMap> { None }` at `provider.rs:17-52`, overriding in `WireProvider` (which already holds the name) and `providers/fleet.rs`. Lands naturally with PROV-032, which adds `filter_models` to the same trait.

**Verify** — Every provider in `all_providers()` reports a human display name distinct from its id wherever pi has one.

## PROV-020 — `toolResult` JSONL key order diverges: `isError` emitted too early

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-core/src/message.rs:716-734` — the hand-written `Serialize` for `Message::ToolResult` writes `role`, `toolCallId`, `toolName`, `content`, **`isError` (`:720`)**, `details?` (`:722`), `usage?` (`:726`), `addedToolNames?` (`:732`), `timestamp` (`:734`). The comment at `:707-711` claims the new keys "sit next to `details` so every pre-existing key position is unchanged" — that claim is wrong, and it is why the defect survived two passes.

**upstream** — `pi/packages/agent/src/agent-loop.ts:773-787` @**v0.83.0** — `createToolResultMessage` (`:773`) is the sole construction site; the object literal (`:774-786`) inserts `role` `:775`, `toolCallId` `:776`, `toolName` `:777`, `content` `:780`, `details` `:781`, `usage` `:782`, the `...addedToolNames` conditional spread `:783`, `isError` `:784`, `timestamp` `:785`. pi's session write path is a bare `JSON.stringify(entry)`, so that literal order IS the on-disk byte order. `pi/packages/ai/src/types.ts:430-446` @v0.84.1 agrees — `isError` at `:444`, after `addedToolNames` at `:443`.

> **Citations corrected in the 2026-08-12 repair pass** (critique finding 9), and this is the same
> defect the critique caught on `AGENT-020`. `agent-loop.ts:777-791` is the **v0.84.1** offset —
> the function moved from `:773` to `:777` between the tags while the body stayed byte-identical, so
> the number was asserted against the wrong tag on the very item whose whole claim is about byte
> order. The `types.ts` offsets were also each off by one to two (`:445` → `:444`, `:441` → `:443`).
> The item's substance is unaffected: `isError` really is emitted three keys too early in cyrup.

**Impact** — Cosmetic on parse; nothing fails today because `cyrup-test-support::interop` compares `serde_json::Value`. But it falsifies the crate's own byte-fidelity claim — a cyrup-exported session JSONL is not byte-identical to pi's for any `toolResult` line, which is the single property the hand-written serializer exists to provide, and it would break any future golden diff taken against real pi output.

**Fix** — Move `st.serialize_field("isError", is_error)` from `message.rs:720` to immediately before `timestamp` at `:734`. Pure reordering — no `len` change, no serde attribute change, no deserialize change. Correct the comment at `:707-711` in the same edit.

**Verify** — The reorder is safe: the existing round-trip test asserts only `details < usage` positionally (true in pi too) and the old-shape fixture carries none of the optional keys, so both stay green. Extend the round-trip test with `find("details") < find("usage") < find("addedToolNames") < find("isError") < find("timestamp")`.

## PROV-023 — `prompt_cache_options` unported — one-shot requests implicitly cache-write

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

> **Kind corrected this pass** (was `upstream-drift`): the flag and the body key are both present at
> the recorded v0.83.0 baseline.

**cyrup** — `cyrup/crates/cyrup-provider/src/api/openai_responses.rs:336-354` builds `model`/`input`/`stream`/`prompt_cache_key`/`prompt_cache_retention`/`store` and never emits `prompt_cache_options`; `rg 'prompt_cache_options|supports_explicit_prompt_cache' crates/` returns nothing. `ResolvedResponsesCompat` (`compat.rs:175-186`) carries only four fields. Cyrup already models the retention tri-state the flag pairs with, so only the flag and one body key are missing.

**upstream** — `pi/packages/ai/src/api/openai-responses.ts:75` @v0.83.0 `supportsExplicitPromptCacheMode: model.compat?.supportsExplicitPromptCacheMode ?? false`, `:278` `const disableImplicitPromptCache = cacheRetention === "none" && compat.supportsExplicitPromptCacheMode`, `:285` `prompt_cache_options: disableImplicitPromptCache ? { mode: "explicit" } : undefined`. All three offsets hold **unchanged at v0.84.1** (verified line-for-line).

> **Citation corrected in the 2026-08-12 repair pass** (critique finding 9). The flag was cited at
> `openai-responses.ts:72`; `:72` is `supportsStrictMode: model.compat?.supportsStrictMode ?? false`
> — a *different* flag, and the one `PROV-034` is about. `supportsExplicitPromptCacheMode` is `:75`.
> This is a wrong-construct citation, not merely a shifted one: a reader checking `:72` would have
> concluded the item confused two compat flags.

**Impact** — Money, quietly. pi turns implicit prompt caching OFF for exactly the requests whose prompts are one-shot (compaction summaries, branch summaries — run with `cacheRetention: "none"`); cyrup sends no `prompt_cache_options`, so OpenAI implicitly cache-WRITES those and bills the cache-write premium. Correctness unaffected; confined to one model family.

**Fix** — Add `supports_explicit_prompt_cache_mode: Option<bool>` to `ModelCompat` and surface it on `ResolvedResponsesCompat` with pi's `?? false` default alongside `supports_tool_search` (`compat.rs:171-193` is the exact precedent; upstream landing point is `openai-responses.ts:75`). In `openai_responses.rs`, after the `prompt_cache_retention` insert, add `if cache == CacheRetention::None && compat.supports_explicit_prompt_cache_mode { obj.insert("prompt_cache_options", json!({"mode":"explicit"})); }`. The flag MUST stay default-false — older OpenAI models reject the parameter. Land with PROV-033/PROV-034, which extend the same struct.

**Verify** — Body-shape test: flag on + retention none ⇒ `"prompt_cache_options":{"mode":"explicit"}` and NO `prompt_cache_key`; long/short retention ⇒ neither; flag absent ⇒ neither regardless of retention (the older-model regression guard).

## PROV-025 — `deferredToolsMode: "kimi"` unported

**Kind** parity-bug · **Severity** low · **Effort** M · **Confidence** confirmed

> **Kind corrected this pass** (was `upstream-drift`): the field is declared at v0.83.0.

**cyrup** — `rg --type rust 'deferred_tools_mode|DeferredToolsMode' crates/` = 0 hits; `ModelCompat` (`compat.rs:73-167`) has no such member, and the completions impl never splits deferred tools. Cyrup's deferred work (PROV-009, closed) implements exactly two renderings — `api/anthropic_messages.rs` and `api/openai_responses.rs` — so this is a third rendering, not a duplicate.

**upstream** — `pi/packages/ai/src/types.ts:567` @**v0.83.0** `deferredToolsMode?: "kimi"` on `OpenAICompletionsCompat`; `pi/packages/ai/src/api/openai-completions.ts` threads it (`const deferredNames = compat.deferredToolsMode === "kimi" ? getDeferredToolNames(context.messages) : new Set()`), applies the serialization, and defaults/resolves it in `detectCompat`/`getCompat`.

**Impact** — Kimi models (cyrup ships `kimi-coding.json`, `moonshotai.json`, `moonshotai-cn.json`) always receive the full tool schema set every turn, so the prompt-prefix cache churns and per-turn prompt tokens stay high on exactly the provider family upstream added the mode for. Cost and latency only — no wrong output.

**Fix** — Add `deferred_tools_mode: Option<DeferredToolsMode>` (one-variant enum `Kimi`) to `ModelCompat`/`ResolvedCompat`, `None` in `detect_compat`, resolved in `get_compat`. In `openai_completions.rs::build_params`, when `Kimi`, call the already-ported `crate::utils::deferred_tools::split_deferred_tools(...)` and apply pi's serialization. Read `getDeferredToolNames` end-to-end first — it is a different accessor from `splitDeferredTools`, working off message names rather than the placement map.

**Verify** — Body test: a model with `compat: {"deferredToolsMode": "kimi"}` and a transcript whose tool result carries `addedToolNames: ["late"]` serializes `late` in Kimi's deferred form and omits it from the `tools` prefix; without the flag the full tool array is emitted unchanged.

---

## Surface-sweep findings (2026-08-03, re-audited 2026-08-12)

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at all, rather than checking a list of known items. IDs use an `-SNN` suffix to mark their provenance. Three of the five (`PROV-S01`, `S02`, `S03`) closed this pass; the two below remain. Their rows are in the single `## Open items` table above — this section holds bodies only.

## PROV-S04 — `estimateContextTokens`' message-anchored added-tool accounting unported

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/utils/estimate.rs:179-186` — `estimate_context_tokens` does `let estimate = estimate_messages(...); if estimate.last_usage_index.is_some() { return estimate; }`: a bare early return with no added-tool accounting.

**upstream** — `pi/packages/ai/src/utils/estimate.ts:114-131` @v0.84.1 — when `estimate.lastUsageIndex !== null`, pi collects `addedToolNames` from every `toolResult` after that index, sizes exactly those tools via `estimateToolsTokens`, and adds the result to BOTH `tokens` and `trailingTokens` before returning. Post-baseline (`3d8f7435`, message-anchored tool loading); at v0.83.0 the same site was a bare early return, so cyrup's code is a faithful stale port.

**Impact** — cyrup does implement deferred/message-anchored tool loading (PROV-009, closed), so tools genuinely arrive mid-conversation and their schema tokens are charged by the provider but counted by nobody. Under-reports the context estimate, chiefly into `CompactionEntry.tokensBefore`. Under-counting is the safer direction of error, which is why this stays low.

**Fix** — Port `estimate.ts:114-131` into `estimate.rs:179-186`: replace the early return with the added-tool collection, size via the existing `estimate_tools_tokens`, and add to both totals.

**Verify** — A fixture whose last usage block is followed by a `toolResult` carrying `addedToolNames: ["late"]` reports a context estimate larger than the same fixture without it, by exactly `estimate_tools_tokens(["late"])`, in both `tokens` and `trailing_tokens`.

## PROV-S05 — `Models::refresh` has no `force`, no abort signal, no per-provider error map

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

> **Severity held at low this pass.** A proposed raise to medium was rejected: the user-visible
> behaviours pi's options carry are largely reproduced by a different mechanism — see below.

**cyrup** — `cyrup/crates/cyrup-provider/src/collection.rs:317-337` `pub async fn refresh(&self, provider: Option<&str>) -> Result<(), ProviderError>`; the all-provider path is `futures::future::join_all(refreshes).await` at `:335` with **every result discarded** and an unconditional `Ok(())`. `Provider::refresh_models` (`provider.rs:44-48`) takes no context argument.

**upstream** — `pi/packages/ai/src/models.ts:46-56` @v0.83.0 `ModelsRefreshOptions{allowNetwork, force, signal}` and `ModelsRefreshResult{aborted, errors: ReadonlyMap<string, Error>}`; `:147` the declaration, `:276-328` the implementation — per-provider credential resolution under the store lock, per-provider errors collected rather than rejected, and on ANY failure a re-invocation with `allowNetwork: false` to restore the persisted catalog. v0.84.1 adds `providers?: readonly string[]` (`models.ts:67`), `ModelsPublication`/generation-checked `publish()` (`:320-361`) and per-provider `AbortController` superseding.

**Impact** — Narrower than the raw diff. `crates/cyrup/src/provider.rs:71-130` already splits pi's `allowNetwork:false` restore from the network refresh, gates the network path on mode (`mode_refreshes_catalogs`, mirroring pi's rpc/interactive-only triggers) and restricts the fetch to configured providers exactly as pi's `resolveRefreshCredential` bail does; freshness/persistence policy lives in `remote_catalog.rs`. What genuinely remains: a caller cannot cancel an in-flight refresh, cannot force past the freshness window, and cannot learn which provider failed — `refresh(None)` reports success unconditionally, so a wholly failed refresh is indistinguishable from a clean one.

**Fix** — Give `refresh` a `RefreshOptions { allow_network, force, cancel: CancellationToken }` and a `RefreshResult { aborted: bool, errors: HashMap<ProviderId, ProviderError> }`, collecting per-provider results from the `join_all` at `collection.rs:335` instead of discarding them; thread a `RefreshContext` into `Provider::refresh_models` (`provider.rs:44-48`). Correct the false doc citation above it in the same edit (PROV-041).

**Verify** — With one provider stubbed to fail, `refresh(None)` returns `errors` naming exactly that provider and leaves the others' catalogs updated; cancelling the token mid-flight returns `aborted: true`; `force` re-fetches inside the freshness window.

**Note** — Duplicates PARITY-GAPS PB-3.

---

## GitHub Copilot findings (2026-08-11, re-audited 2026-08-12)

The `github-copilot` provider did not exist when this file was written — `PROV-005` recorded it as one of four missing providers. These three items are gaps **in that new code**, found by reading the port against pi rather than by re-checking a list. All three re-verified unchanged at `04c1ba2`.

> **The blanket claim "every upstream line cited is present at both v0.83.0 and v0.84.1" was FALSE
> and is struck** (2026-08-12 repair pass, critique finding 9). Two of the three items carried a
> citation that does not hold at the tag this file classifies against:
> `openai-completions.ts:646-652` in `PROV-028` is the **v0.84.1** offset (v0.83.0: `:638-645`), and
> `PROV-029` quoted a property (`isSubscription: true`) that **does not exist at v0.83.0 at all**.
> Per-item verification now lives in each item's `upstream` paragraph; there is no section-level
> both-tags guarantee. What *did* verify clean at both tags: `anthropic-messages.ts:867-888` (the
> Copilot branch) and `:890` (the OAuth branch that follows it), and
> `openai-responses.ts:223-230`.

Copilot concentrates risk because it is the only built-in that (a) drives all three wire APIs from one catalog — of its 28 rows, **9** are `anthropic-messages`, the rest `openai-completions`/`openai-responses` — and (b) derives its request base URL from the credential rather than the model. Anything the API layer special-cases on `provider === "github-copilot"` has to be ported three times, and two of the three call sites were missed.

## PROV-027 — Copilot's Claude models send `x-api-key`; pi's Copilot branch sends `Authorization: Bearer`

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed

**upstream** — `pi/packages/ai/src/api/anthropic-messages.ts:866-888` opens with the comment `// Copilot: Bearer auth, selective betas.` and branches on `model.provider === "github-copilot"` **before** the OAuth-token test at `:890`, constructing the client with `apiKey: null, authToken: apiKey` — i.e. `Authorization: Bearer <copilot token>` — and only the selective betas (fine-grained-tool-streaming / interleaved-thinking), deliberately **without** the Claude-Code identity headers the OAuth branch adds.

**cyrup** — `cyrup/crates/cyrup-provider/src/api/anthropic_messages.rs:470-536` `build_headers` has **no provider branch at all**. The auth scheme is chosen solely by `is_oauth`, derived at `:434-437` from `is_oauth_token(key)` = `api_key.contains("sk-ant-oat")`. A Copilot token is a claim string of the form `tid=…;exp=…;proxy-ep=proxy.individual.githubcopilot.com;st=dotcom` — no `sk-ant-oat` substring — so `is_oauth` is false and `:524-531` emits `x-api-key: tid=…`.

**Impact** — All **9** Copilot Claude rows present the Copilot token in a header GitHub's Anthropic-compatible edge does not read: every request on the `anthropic-messages` route arrives unauthenticated. The betas half of pi's branch is incidentally already correct — the non-OAuth arm emits exactly the selective set — so the only defect is the scheme.

**Fix** — In `build_headers` (`anthropic_messages.rs:470`), test `model.provider.as_str() == "github-copilot"` **before** the `is_oauth` test and take the bearer path without the Claude-Code identity headers, mirroring pi's ordering. The ordering matters for its own sake: a Copilot token that ever did contain `sk-ant-oat` must still not acquire the Claude-Code identity.

**Verify** — Assert on emitted headers, both directions: a model with `provider == "github-copilot"` and a `tid=…` key yields `authorization: Bearer tid=…` and **no** `x-api-key`, and its `anthropic-beta` contains neither `claude-code-20250219` nor `oauth-2025-04-20`; a mirror case with `provider == "anthropic"` and the same key still yields `x-api-key`.

## PROV-028 — `github-copilot-headers.ts` unported — no `X-Initiator` / `Openai-Intent` / `Copilot-Vision-Request`

**Kind** not-ported · **Severity** high · **Effort** S · **Confidence** confirmed

**upstream** — `pi/packages/ai/src/api/github-copilot-headers.ts` exports `inferCopilotInitiator` (last message role `!== "user"` → `"agent"`, else `"user"`), `hasCopilotVisionInput` (any `user` or `toolResult` message with an `image` content part) and `buildCopilotDynamicHeaders`, returning `X-Initiator: user|agent`, `Openai-Intent: conversation-edits`, plus `Copilot-Vision-Request: "true"` when images are present. Imported and applied at **all three** api impls, each guarded by `model.provider === "github-copilot"`. Offsets **@v0.83.0** (the ported baseline): `anthropic-messages.ts:867-871` (import `:41`), `openai-completions.ts:638-645` (import `:52`), `openai-responses.ts:223-230` (import `:25`). Offsets **@v0.84.1**, where the code is byte-identical: `anthropic-messages.ts:867-871` and `openai-responses.ts:223-230` are unmoved; `openai-completions.ts` shifts to `:646-653` (import `:53`).

> **Citation corrected in the 2026-08-12 repair pass** (critique finding 9). `openai-completions.ts`
> was cited as `:646-652`, which is the **v0.84.1** offset, under a section header asserting every
> cited line held at both tags. At v0.83.0 the Copilot header block is `:638-645`. This is the
> `AGENT-020` defect on a **high**: the claim was checkable, was checked, and was wrong. The item's
> substance is unaffected — the block exists at both tags and cyrup ports none of it.

**cyrup** — ABSENT. No counterpart file; `rg -i 'X-Initiator|Copilot-Vision|Openai-Intent' crates/cyrup-provider/src` returns only the login flow's unrelated `openai-intent: chat-policy` on the model-policy POST (`auth/oauth/github_copilot.rs:666`) and its test at `:1345`. The only Copilot special-case that *was* ported into the API layer is the reasoning-effort suppression at `api/openai_responses.rs:398` — so the provider check exists on one of three routes and carries none of these headers.

**Impact** — Two consequences that fail differently. (1) **Images**: pi's own comment is `// Copilot requires Copilot-Vision-Request header when sending images`; without it an image turn against Copilot is rejected rather than degraded — a loud failure on a normal path, on all three routes. (2) **`X-Initiator`**: this is how Copilot distinguishes a user-initiated request from an agent follow-up, which is quota-relevant; omitting it does not fail, it silently misreports every request in an agent loop. The static headers Copilot's edge also requires (`User-Agent`, `Editor-Version`, `Editor-Plugin-Version`, `Copilot-Integration-Id`) *are* present, baked into every catalog row's `model.headers` — which is what makes this easy to miss: Copilot traffic is not header-less, it is missing exactly the per-request three.

**Fix** — Port the module as `cyrup-provider/src/api/github_copilot_headers.rs` (three pure functions over `&[Message]`, no I/O) and apply it at the three header builders — `anthropic_messages.rs:470`, `openai_completions.rs`, `openai_responses.rs:412` — guarded on `model.provider.as_str() == "github-copilot"`. Header precedence must match pi: after `model.headers`, so the dynamic set wins over the baked editor identity, and before `opts.headers`. Land with PROV-027; they touch the same function on the Anthropic route.

**Verify** — Per route, assert the emitted header map: a Copilot text turn ending in a `user` message has `X-Initiator: user` and no `Copilot-Vision-Request`; one ending in an assistant or `toolResult` message has `X-Initiator: agent`; one containing an image part in a `user` **or** a `toolResult` message has `Copilot-Vision-Request: true`; a non-Copilot model on the same route has none of the three. Run all four against each of the three impls — the class of bug here is "ported to one route, missed on the others".

## PROV-029 — Copilot and Codex login flows are written but unreachable

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed

**upstream** — `pi/packages/ai/src/providers/github-copilot.ts:16` @**v0.83.0** gives the provider `oauth: lazyOAuth({ name: "GitHub Copilot", load: loadGitHubCopilotOAuth })`, so `provider.auth.oauth.login` resolves — lazily — to the full flow. `providers/openai-codex.ts:13` @v0.83.0 does the same: `oauth: lazyOAuth({ name: "OpenAI (ChatGPT Plus/Pro)", load: loadOpenAICodexOAuth })`. The laziness is a bundling concern only; the value is always reachable. At **v0.84.1** both gain `isSubscription: true`, which reflows the Codex literal onto five lines (`openai-codex.ts:13-17`) while `github-copilot.ts:16` stays a one-liner.

> **Citation corrected in the 2026-08-12 repair pass** (critique finding 9), and this is the worst
> instance found in the file — on a **high**. The quoted v0.83.0 source included
> `isSubscription: true`, a property that **does not exist at v0.83.0**; it is a v0.84.1 addition.
> And `providers/openai-codex.ts:15` @v0.83.0 is `models: Object.values(OPENAI_CODEX_MODELS)`, not
> the OAuth line — the OAuth line is `:13`. `:15` *is* the `isSubscription: true` line at v0.84.1,
> so both errors point the same way: v0.84.1 was read and attributed to v0.83.0.
> **The item's classification is unaffected** — `lazyOAuth({… load: load*OAuth })` is present at
> both tags, so the provider-side `login` really is reachable upstream and unreachable in cyrup.
> **One consequence for the Impact paragraph:** the subscription marker cannot be sourced from pi
> v0.83.0. It is sourced from cyrup instead — `providers/github_copilot.rs:597` and
> `providers/openai_codex.rs:451` both implement `fn is_subscription(&self) -> bool`, so cyrup's own
> `/login` list renders both with the marker regardless of what pi did at the baseline.

**cyrup** — Both flows are **fully ported and cannot be called.** cyrup has two structs per provider: a runtime half implementing only `refresh`/`to_auth` (`providers/github_copilot.rs:410` `GitHubCopilotOAuth`, `providers/openai_codex.rs:276` `OpenAiCodexOAuth`) and a login-capable flow (`auth/oauth/github_copilot.rs` `GitHubCopilotLogin`, whose `login` at `:821` runs the whole device-code + policy-acceptance grant; `auth/oauth/openai_codex.rs:516` `OpenAiCodexOAuthFlow`, `login` at `:1038`). The provider's `ProviderAuth` wires the **runtime half** — `github_copilot.rs:142-146`, `openai_codex.rs:129-131` — and neither runtime struct implements `login`, so it falls through to the trait default at `auth/mod.rs:124-131`, `Err(OAuthError::LoginUnsupported)`. The flows are reachable only via `OAuthFlowId::{GithubCopilot,OpenAiCodex}` through `load.rs`, and `register_bundled_oauth_flow_loaders` (`auth/oauth/load.rs:111`) has **zero production callers** — the only invocations workspace-wide are its own tests (`load.rs:239`, `:277`, `:322`), so the registry is always `OAuthFlowLoaders::default()`, all-`None`. Meanwhile `/login` resolves its strategy from the provider: `cyrup-config/src/login.rs:784` calls `oauth.login(interaction)` off `provider.provider_auth()`. Contrast the four providers that work — `anthropic`, `kimi-coding`, `xai`, `openrouter` — which route through `providers/builtin_oauth.rs:37-56`, returning the login-capable struct directly; `builtin_oauth.rs:14-16` states the exemption in prose ("`github-copilot` … and `openai-codex` wire their own OAuth inside …"), which is exactly where the two got dropped.

**Impact** — `/login` lists GitHub Copilot and OpenAI Codex (cyrup's own `is_subscription` returns `true` for both — `providers/github_copilot.rs:597`, `providers/openai_codex.rs:451` — so they render with the subscription marker), the user selects one, and the flow ends in `LoginUnsupported`. A credential for either can only arrive by hand-placing it in `auth.json`. Both providers' login code is complete, tested at the loopback level, and dead. This is the "advertised but rejected" shape, and it is invisible from the flow side: every test in `auth/oauth/github_copilot.rs` drives `GitHubCopilotLogin` directly and passes.

**Fix** — Point both providers at the login-capable struct: either extend `builtin_provider_oauth` (`providers/builtin_oauth.rs:37`) with `"github-copilot"` and `"openai-codex"` arms and have the two `*_auth()` constructors use it — deleting the prose exemption at `:14-16` — or have `github_copilot_auth`/`openai_codex_auth` construct the flow struct directly. The flow structs already delegate `refresh`/`to_auth` to the runtime halves, so nothing else changes. Separately, either populate the flow registry at startup or delete it: a registry with no production caller is a second, silent way to reach the same dead end.

**Verify** — Two assertions, because either alone passes today. (1) Registry-independent: for every provider in `all_providers()` whose `provider_auth().oauth` is `Some`, calling `login` with a scripted interaction must not return `LoginUnsupported` — a table-driven test that fails when a future provider repeats this. (2) End to end: drive `/login` for `github-copilot` against a loopback GitHub and assert a credential lands in the store, entering through `cyrup_config::login::login` rather than the flow struct.

**Relation to PROV-003** — PROV-003 is now `partially-closed`, not "zero OAuth flows"; this item is not a restatement of it. The flows exist; what is missing is one field assignment per provider.

---

## Findings filed 2026-08-12 (cyrup HEAD `04c1ba2`, pi `v0.83.0`/`v0.84.1`)

Seventeen items. Four came from re-reading code that closed an earlier item — the discipline the ledger records as "closing a *not implemented* item means the subsystem exists, not that it is correct" — and four (`PROV-043`…`PROV-046`) came from a second reader taking a different lens (env-var sweep, per-arm strictness) over files the first had already read for other reasons.

## PROV-030 — `google-vertex` is registered with 10 models and no wire API; every request dies with `NoApiImpl`

**Kind** not-ported · **Severity** high · **Effort** L · **Confidence** confirmed

**cyrup** — `crates/cyrup-provider/src/providers/all.rs:186-190` pushes `google_vertex_provider_with(...)` into the built-in set (the four-provider block is `:176-197`). **The same file's port-status doc table contradicts that code and will mislead anyone who opens the file this item names:** `all.rs:12-47` still lists `amazon-bedrock` as "**pending** (bedrock-converse-stream)" (`:12`), `google-vertex` as "**pending** (vertex auth)" (`:23`) and `openai-codex` as "**pending** (codex oauth)" (`:34`), and the summary line at `:46-47` reads "Pending (NOT registered — no fabrication, they slot in when their auth/wire lands): `amazon-bedrock`, `github-copilot`, `google-vertex`, `openai-codex`" — naming all four, `github-copilot` included, even though the table row at `:21` already says "ported" and the registration comment at `:192-193` explicitly records that "`all.rs`'s own port-status table had it marked *pending*" and then leaves the table alone. So the header says `google-vertex` is not registered; forty lines later it is registered; and this item says it is registered but has no wire api. All three statements are in one file and only the last two are true. `providers/google_vertex.rs:60` declares `GOOGLE_VERTEX_API = "google-vertex"` and `:114-123` builds a plain `WireProvider` over the shared registry; the module doc at `:31-36` admits it — "**The wire api.** Every row's `api` is `google-vertex`, and this crate has no `api/google_vertex.rs`". `api/mod.rs:130-163` `register_builtins` registers nine factories — openai-completions, anthropic-messages, openai-responses, azure-openai-responses, google-generative-ai, pi-messages, bedrock-converse-stream, openai-codex-responses, mistral-conversations — none of them `google-vertex`, and `ls crates/cyrup-provider/src/api/` has no such file. All **10** rows of `providers/catalog/google-vertex.json` carry `"api": "google-vertex"`. The request path is `wire.rs:158-166`: `match registry.get(&model.api) { Some(imp) => imp, None => { sink.send(ProviderError::NoApiImpl(...).into_error_event(...)); return; } }`, rendering as `"no API implementation for google-vertex"` (`error.rs:80-82`).

**upstream** — `pi/packages/ai/src/types.ts:16-26` @**v0.83.0** lists `"google-vertex"` as its own member of `KnownApi`, at `:25`; the same union is `:17-27` at v0.84.1 with `"google-vertex"` at `:26` (one line of drift above it, no change to the union's contents). The implementation is `pi/packages/ai/src/api/google-vertex.ts` (present at v0.83.0; +3 lines in v0.83.0..v0.84.1), registered through `api/google-vertex.lazy.ts` and re-exported from `compat.ts:17`. `pi/packages/ai/src/providers/google-vertex.ts:89-93` @v0.83.0 declares `Provider<"google-vertex">` with `vertexAuth`.

> **Citation tightened in the repair pass:** `types.ts:16-27` was recorded as holding "at both
> v0.83.0 and v0.84.1"; it is the union of the two ranges rather than either one. Per-tag offsets
> are now given separately.

**Impact** — The Vertex provider is fully advertised — it appears in `all_providers()`, contributes 10 models to `builtin_catalog()` (so `/model`, `--model`, `--provider google-vertex` and the subagents model registry all offer them), resolves auth through the fully-ported `GoogleVertexApiKeyAuth` including the ADC arm — and then every single stream terminates immediately. A user who has run `gcloud auth application-default login` and selected a Gemini model sees a hard failure on the first turn, with an error naming an internal registry key. Verified precisely scoped: parsing all 35 catalogs, `google-vertex` is the **only** dangling api id — `openrouter-images` resolves through the separate images registry (`images/mod.rs:38`, `images/openrouter.rs`).

**Fix** — Port `pi/packages/ai/src/api/google-vertex.ts` as `crates/cyrup-provider/src/api/google_vertex.rs`. It shares nearly everything with `google_generative_ai.rs` via pi's `api/google-shared.ts`, so factor the shared converters out first — cyrup already has `requires_tool_call_id`, `resolve_thought_signature`, `map_stop_reason` and `convert_tools` there. Add `known_api::GOOGLE_VERTEX` in `lib.rs` and register the factory in `register_builtins` (`api/mod.rs:130-163`). The base-URL template interpolation (`https://{location}-aiplatform.googleapis.com`, `google_vertex.rs:63`) belongs in the new impl. **S-sized mitigation if the port cannot land now:** make an unregistered api a construction-time rejection rather than a per-request one — refuse to push a provider whose catalog names an api the registry does not `contains()` (`api/mod.rs:116-119` already has the predicate), so the provider is absent from `/model` instead of present-and-broken.

**Fix, part 2 — the stale in-source doc table, and it ships with EITHER of the above** (added by the 2026-08-12 repair pass per critique finding 8). Rewrite `providers/all.rs:12-47` so it describes the code beneath it: mark `amazon-bedrock` (`:12`), `github-copilot` (`:21`), `google-vertex` (`:23`) and `openai-codex` (`:34`) as **registered**, and replace the summary line at `:46-47` — which currently names all four as "Pending (NOT registered)" — with the real residual, which after this pass is exactly one line: *`google-vertex` is registered but has no wire api (PROV-030); `openai-codex`/`github-copilot` are registered and their login flows are unreachable (PROV-029).* Do the same edit even if only the S-sized mitigation lands, in which case the line says `google-vertex` is deliberately withheld from the registry. Delete the apologetic parenthetical at `:192-193` once the table it complains about is correct. **This is not cosmetic and must not be deferred to a docs sweep:** the header is the first thing an engineer picking up PROV-030 reads, it flatly denies the item's premise, and the item was filed only because a reader ignored it. Cheap regression guard: extend the roster test PROV-038 rewrites so it asserts the set of ids in `all_providers()` matches the set the table marks registered — a doc table that can go stale silently will.

**Verify** — A table-driven test over `all_providers()`: for every provider and every model it ships, `builtin_registry().contains(&model.api)` must be true. That test fails today on 10 rows and would have caught this the moment google-vertex landed (fold it into PROV-038's directory walk). Then a faux-origin round trip through the new impl, mirroring the existing `google_generative_ai.rs` decoder tests.

## PROV-031 — `Models` has no `get_available` / `check_auth` / `login` / `logout`

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

> **Severity corrected down from the auditor's medium.** Every behaviour exists at another layer —
> login/logout at `cyrup-config/src/login.rs:784`, the auth check at `provider_auth_status`
> (`login.rs:360`), availability filtering at `cyrup-session-svc/src/session.rs:2726-2728`. The
> auditor's framing also conflated two upstream registries: pi's **subagents** consume the
> coding-agent `ModelRegistry.getAvailable()` (`model-registry.ts:644-646`), which `session.rs:2725`
> cites and implements — not `Models.getAvailable()`. This is a crate-boundary / API-shape gap; the
> only residual *behavioural* loss is `filterModels`, which is PROV-032.

**cyrup** — `crates/cyrup-provider/src/collection.rs` public surface is `create_models` `:56`, `set_provider` `:72`, `delete_provider` `:78`, `clear_providers` `:83`, `get_providers` `:90`, `get_provider` `:95`, `get_models` `:101`, `get_model` `:117`, `get_auth` `:128`, `get_auth_with` `:137`, `stream` `:165`, `complete` `:221`, `stream_simple` `:236`, `complete_simple` `:297`, `refresh` `:317`. No `get_available`, no `check_auth`, no `login`, no `logout`; `rg 'fn check_auth|auth_check' crates/cyrup-provider` returns nothing. Two consumers work around it: `cyrup-ext-subagents/src/extension.rs:11306-11308` binds availability to `builtin_catalog()` (PROV-007's residual), and `cyrup-session-svc/src/session.rs:2726-2728` re-implements filtering as `full_model_registry().filter(|m| self.has_configured_auth(m))`, with `AuthCheck` living in `cyrup-config/src/login.rs` rather than the provider crate.

**upstream** — `pi/packages/ai/src/models.ts:127-190` @v0.83.0 (`interface Models` opens at `:127`) — `checkAuth(providerId)` `:150`, `getAvailable(providerId?)` `:153` (documented at `:152` as "Return models whose providers have complete auth configuration"; the implementation is `:394-409`, and `:407` is where `Provider.filterModels` is applied), `login(providerId, type, interaction)` `:168`, `logout(providerId)` `:171`. All four present at the ported baseline; v0.84.1 only adds an `AuthOperationOptions` argument to three of them.

> **Citations corrected in the 2026-08-12 repair pass** (critique finding 9). Every declaration was
> off by one at v0.83.0 (`:149`→`:150`, `:152`→`:153`, `:151`→`:152`, `:167`→`:168`, `:170`→`:171`),
> and the `filterModels` application was cited as `:405-410` when the enclosing `getAvailable` body
> is `:394-409` and the call itself is `:407`. Re-resolved with `git -C pi show v0.83.0:models.ts`.

**Impact** — No behaviour is lost that PROV-032 does not already cover. What is lost is the crate boundary: a host or embedder using `cyrup-provider` directly cannot ask "which models can I actually use" or "is this provider configured" without reaching into `cyrup-config`, and the two existing availability filters are duplicated implementations that can drift from each other.

**Fix** — Add to `Models` (`collection.rs`): `pub async fn check_auth(&self, provider: &str) -> Option<AuthCheck>` resolving through the existing `resolve_provider_auth` without triggering an OAuth refresh; `pub async fn get_available(&self, provider: Option<&str>) -> Vec<Model>` = `get_models` filtered by `check_auth(..).is_some()` then passed through `Provider::filter_models` (PROV-032); and `login`/`logout` delegating to `ProviderAuth::oauth`/`api_key` and the `CredentialStore`. Move `AuthCheck` from `cyrup-config/src/login.rs` into `cyrup-provider/src/auth/types.rs` and re-export it. Then repoint `session.rs:2726` and `extension.rs:11306` at it and delete both local copies.

**Verify** — With a credential stored for exactly one provider, `get_available(None)` returns only that provider's models while `get_models(None)` still returns all 35 catalogs' worth; `check_auth` on an unconfigured provider returns `None` and makes no network call; `cyrup-session-svc` and `cyrup-ext-subagents` delegate rather than re-filter.

## PROV-032 — `Provider::filterModels` unported — the Copilot filter is complete, tested, and has zero production callers

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-provider/src/providers/github_copilot.rs:363-386` `pub fn filter_github_copilot_models(models: &[Model], credential: Option<&Credential>) -> Vec<Model>` is a complete port (OAuth-credential-only, `availableModelIds` array, one non-string voids the whole filter). `rg filter_github_copilot_models crates/` shows its only callers are its own tests (`:969`, `:980`, `:984`, `:989`, `:995`, `:1001`, `:1004`) — zero production call sites. Its own doc at `:357-361` says why: "Pi applies this only in `Models.getAvailable()` (`models.ts:407`), and cyrup's `Models` has no `get_available` counterpart". `provider.rs:17-52` has no `filter_models` member, so no provider can express credential-scoped availability at all.

**upstream** — `pi/packages/ai/src/models.ts:111` @v0.83.0 — `filterModels?(models: readonly Model<TApi>[], credential: Credential | undefined): readonly Model<TApi>[]`, documented at `:105-110` as "Optional provider policy for credential-specific model availability. `getModels()` remains the complete synchronous catalog; `Models.getAvailable()` applies this filter after confirming that provider auth is configured." Implemented for Copilot at `providers/github-copilot.ts:19-27` (the `filterModels:` property; identical at v0.84.1) and applied inside `getAvailable` at `models.ts:407`. The transport of the option through `createProvider` is `models.ts:545`/`:618`.

> **Citations tightened in the 2026-08-12 repair pass** (critique finding 9): `models.ts:107-111`
> covered the declaration but is a doc-comment range, `providers/github-copilot.ts:20-26` clips both
> ends of the `filterModels` property (`:19-27`), and `models.ts:405-410` names a range around the
> call rather than the call (`:407`). Substance unchanged.

**Impact** — A GitHub Copilot Business/Enterprise account whose token authorises a subset of models is offered the entire 28-row catalog in `/model`, in `--model`, and in the subagents model registry. Selecting an unauthorised row produces a provider-side rejection mid-turn instead of the model simply not being listed, and the user has no way to tell which rows are real. The port already computes the answer — `fetch_available_model_ids` runs during login (`providers/github_copilot.rs:561`, `auth/oauth/github_copilot.rs:613`) and stores `availableModelIds` on the credential — and throws it away at read time.

**Fix** — Add `fn filter_models(&self, models: &[Model], credential: Option<&Credential>) -> Vec<Model> { models.to_vec() }` as a defaulted method on `Provider` (`provider.rs:17-52`); override it for the Copilot provider to delegate to the existing `filter_github_copilot_models`; call it from the new `Models::get_available` (PROV-031) after the auth check, exactly where pi calls it. Lands with PROV-031 — it has no independent call site.

**Verify** — Construct a Copilot provider with an OAuth credential whose `ext.availableModelIds` names 3 of the 28 catalog rows: `get_models(Some("github-copilot"))` still returns 28 and `get_available(Some("github-copilot"))` returns exactly those 3; with an api-key credential, or a credential whose `availableModelIds` contains a non-string, both return 28.

## PROV-033 — openai-responses carries pi's **deleted** `sendSessionIdHeader` flag and can never emit `x-session-id`

**Kind** stale-port · **Severity** medium · **Effort** S · **Confidence** confirmed

> **Kind corrected from the auditor's `cyrup-original`/parity-bug framing.** `sendSessionIdHeader`
> is not a cyrup invention: `git -C pi grep sendSessionIdHeader v0.83.0 v0.84.1` hits
> `packages/ai/CHANGELOG.md:168` — "Removed the `OpenAIResponsesCompat.sendSessionIdHeader` flag…
> Replace `sendSessionIdHeader: false` with `sessionAffinityFormat: \"openai-nosession\"` (#6496)".
> cyrup ported a flag pi later deleted, and the in-tree citation was accurate at the older revision.
> The fix is unchanged; the classification is `stale-port`, and it is the clean example of that kind.

**cyrup** — `crates/cyrup-provider/src/api/compat.rs:154-158` declares `pub send_session_id_header: Option<bool>` on `ModelCompat`, `:180` on `ResolvedResponsesCompat`, `:193` resolves it `?? true`. `api/openai_responses.rs:436-448` (inside `build_headers`, `:412`) — when a `session_id` is present it emits `session_id` gated on that flag and then `x-client-request-id` unconditionally; there is no path to `x-session-id`. `rg 'session_affinity_format|SessionAffinityFormat' crates/` = 0 hits. `ResolvedResponsesCompat` (`compat.rs:175-186`) carries only `supports_developer_role`, `send_session_id_header`, `supports_long_cache_retention`, `supports_tool_search` — missing `sessionAffinityFormat`, `supportsStrictMode` and `supportsOpenAIGrammarTools`.

**upstream** — `pi/packages/ai/src/types.ts:575-590` @v0.83.0 `OpenAIResponsesCompat = { supportsDeveloperRole?, sessionAffinityFormat?, supportsLongCacheRetention?, supportsStrictMode?, supportsOpenAIGrammarTools?, supportsToolSearch?, supportsExplicitPromptCacheMode? }` — no `sendSessionIdHeader`. `api/openai-responses.ts:49` `detectSessionAffinityFormat`, `:70` `sessionAffinityFormat: model.compat?.sessionAffinityFormat ?? detectSessionAffinityFormat(model)`, and the three-way header branch at `:233-241`: `if (compat.sessionAffinityFormat === "openrouter") { headers["x-session-id"] = sessionId } else { if (compat.sessionAffinityFormat === "openai") headers.session_id = sessionId; headers["x-client-request-id"] = sessionId }`.

**Impact** — Two failures. (1) A `models.json` written against pi's current schema — `"compat": {"sessionAffinityFormat": "openrouter"}` — is silently ignored on the responses route (the field is not in `ModelCompat`, and serde defaults it away), so an OpenRouter-backed responses model gets `session_id` + `x-client-request-id`, headers OpenRouter does not read, and never gets the `x-session-id` it uses for sticky routing. Sticky routing is what keeps the prompt-prefix cache warm, so this is a silent cost and latency regression on exactly the provider the format exists for. (2) `sendSessionIdHeader` survives in cyrup as a configurable knob upstream has deleted, so anyone setting it is configuring a field that no longer exists anywhere else, and the `openai`/`openai-nosession` distinction is unreachable.

**Fix** — Add `SessionAffinityFormat { Openai, OpenaiNosession, Openrouter }` to `compat.rs` beside `CacheControlFormat`, put `session_affinity_format: Option<SessionAffinityFormat>` on `ModelCompat`, resolve it on `ResolvedResponsesCompat` with a `detect_session_affinity_format(model)` default mirroring `openai-responses.ts:49`, and rewrite `openai_responses.rs:436-448` as pi's three-way branch. **DELETE `send_session_id_header`** in the same change (`compat.rs:154-158`, `:180`, `:193` and the `openai_responses.rs` use), taking pi's documented migration (`false` ⇒ `"openai-nosession"`). Land with PROV-024 so both routes share one enum. While in `ResolvedResponsesCompat`, add the two other missing members (`supports_strict_mode ?? false`, `supports_openai_grammar_tools ?? false`) per `openai-responses.ts:72-73` — PROV-034 and PROV-011 both need them.

**Verify** — Extend the header test at `openai_responses.rs:1787-1798` (which currently asserts `session_id` + `x-client-request-id`): a model whose baseUrl is openrouter.ai emits `x-session-id` and NEITHER of the other two; an openai model emits `session_id` + `x-client-request-id`; `"openai-nosession"` emits `x-client-request-id` only. Then `rg send_session_id_header crates/` must return zero.

## PROV-034 — openai-responses always emits `"strict": false`; pi omits the key entirely unless `supportsStrictMode`

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-provider/src/api/openai_responses.rs:810-827` `convert_responses_tools(tools, defer_loading)` builds `{type, name, description, parameters, defer_loading?}` and then unconditionally `o.insert("strict".to_string(), json!(false))` at `:824`. The function takes no compat argument at all; its three call sites pass only a `defer_loading` bool (`openai_responses.rs:376`, `azure_openai_responses.rs:391`, `openai_codex_responses.rs:715`). `ResolvedResponsesCompat` (`compat.rs:175-186`) has no `supports_strict_mode`.

**upstream** — `pi/packages/ai/src/api/openai-responses-shared.ts:344-378` @v0.83.0 `convertResponsesTools(tools, options)`: `const supportsStrictMode = options?.supportsStrictMode ?? true;` … the function-tool literal is built **without** `strict`, and only `if (supportsStrictMode) { functionTool.strict = constrainedStrict ?? defaultStrict; }` (`:375-377`) adds it. The caller supplies `supportsStrictMode: compat.supportsStrictMode` (`openai-responses.ts:301-304`), and `getCompat` defaults that to **false** (`openai-responses.ts:72`). So for every model that does not opt in, pi sends **no** `strict` key; cyrup sends `"strict": false`.

**Impact** — A wire-shape divergence on every openai-responses, azure-openai-responses and openai-codex-responses request that carries tools. On OpenAI's own endpoint `strict: false` and an absent key are equivalent, so nothing breaks there; the exposure is OpenAI-compatible gateways and Azure deployments that reject unknown or explicitly-false schema fields, and any model whose catalog sets `supportsStrictMode: true` — where pi would send the constrained value from `resolveJsonSchemaStrictSampling` and cyrup still sends a hard `false`, defeating grammar-constrained sampling. It also blocks PROV-011: the strict/grammar work has nowhere to attach while the value is a literal.

**Fix** — Change `convert_responses_tools` (`openai_responses.rs:810`) to take a `ConvertResponsesToolsOptions { defer_loading, supports_strict_mode, supports_openai_grammar_tools }`, omit the `strict` key when `!supports_strict_mode`, and thread `ResolvedResponsesCompat::supports_strict_mode` (added by PROV-033) from the three call sites. That struct also becomes PROV-011's landing point on this route.

**Verify** — Body-shape test alongside the existing one at `openai_responses.rs:1655`: with no compat, `body["tools"][0]` has NO `"strict"` key at all; with `compat: {"supportsStrictMode": true}`, `strict` is present and `false`. Mirror both in `azure_openai_responses.rs` and `openai_codex_responses.rs` — the class of bug here is "fixed on one route, missed on the others".

## PROV-035 — `core/cache-stats.ts` entirely unported: no cache-waste accounting, no cache-miss notices

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `rg -i 'compute_cache_waste|cache_waste|detect_cache_miss|collect_cache_misses|show_cache_miss' crates/` returns a single unrelated hit (`cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:1050`, an MCP cache test) — the module is entirely absent. `crates/cyrup-tui/src/app.rs:4192-4218` is cyrup's whole `/session` renderer: a markdown table of file / id / message counts / token counts / cost, with no cache-waste line. No settings key corresponds to pi's `showCacheMissNotices` (`rg showCacheMissNotices` and `rg show_cache_miss_notices` are both empty).

**upstream** — `pi/packages/coding-agent/src/core/cache-stats.ts` @v0.83.0 — `CACHE_TTL_MS = 5*60*1000` (`:8`), `NOISE_FLOOR_TOKENS` (`:11`), `interface CacheMiss` (`:14`), `interface CacheWasteTotals` (`:25`), `interface ModelPriceSource` (`:33`), `computeCacheWaste(entries, models)` (`:138`), `collectCacheMisses(...)` (`:147`), `detectCacheMiss(...)` (`:158`). Both consumers are live in `modes/interactive/interactive-mode.ts` @v0.83.0: `:5660` `computeCacheWaste(entries, this.session.modelRuntime)` feeding the `Cache Re-billed: $X (N tokens, M misses)` line at `:5705-5711`, and `:3354-3355` `collectCacheMisses(...)` gated on `getShowCacheMissNotices()`, re-injecting per-message miss notices into the transcript at render time (also `:3456`, `:4166`).

**Impact** — Prompt-cache misses are the single largest avoidable cost in a long session, and cyrup gives the user no signal at all. pi prints `Cache Re-billed: $0.42 (128,000 tokens, 3 misses)` in `/session` and marks the individual assistant messages that paid for a re-billed prefix, so a user can see that a mid-session tool-set change is invalidating the cache every turn. In cyrup that money is spent silently and the only visible number is the total cost, which looks like normal usage. The primitives all exist — `Usage.cache_read`/`cache_write` are carried and `compute_cost` (`cyrup-provider/src/usage.rs:37-58`) already knows the cache-write premium — so this is arithmetic over the session entries plus two render sites, not a new subsystem.

**Fix** — Port `cache-stats.ts` as `crates/cyrup-provider/src/cache_stats.rs` (`CACHE_TTL_MS`, `NOISE_FLOOR_TOKENS`, `CacheMiss`, `CacheWasteTotals`, `detect_cache_miss`, `collect_cache_misses`, `compute_cache_waste`) taking a price source that `cyrup_provider::Model` already satisfies. Wire `compute_cache_waste` into `crates/cyrup-tui/src/app.rs:4192` so `/session` gains pi's `Cache Re-billed` line under the same `stats.cost > 0 || cacheWaste.missedTokens > 0` guard, and wire `collect_cache_misses` into the transcript render path behind a `showCacheMissNotices` setting.

**Verify** — A fixture session whose second assistant turn has `cache_read: 0` after a first turn with a large `cache_write` yields `missCount == 1` and `missedTokens` equal to the re-billed prefix; `/session` prints the line only when `missedTokens > 0`, and formats `$X` only when `missedCost >= 0.0001` (pi's threshold at `interactive-mode.ts:5708`).

## PROV-036 — `getUsageCostBreakdown` unported — `/session` shows one cost total

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `rg 'cost_breakdown|UsageCostBreakdown|get_usage_cost_breakdown' crates/` returns zero hits. `crates/cyrup-tui/src/app.rs:4203`,`:4216` render a single `| cost | ${:.3} |` row from `stats.cost`. The totals half of pi's `usage-totals.ts` IS ported — `add_usage_totals` at `crates/cyrup-tui/src/status.rs:170`, called from `app.rs:4523` and `status.rs:155` — so only the breakdown is missing.

**upstream** — `pi/packages/coding-agent/src/core/usage-totals.ts:30-36` `interface UsageCostBreakdownEntry` and `:37-62` `getUsageCostBreakdown(entries: SessionEntry[])`, keyed `${provider}/${responseModel ?? model}` — so an OpenRouter `auto` route is attributed to the concrete model it resolved to — with a bucket literally named **`Tools/summaries`** absorbing toolResult, branch-summary and compaction usage "so the breakdown reconciles with the session total". Rendered at `modes/interactive/interactive-mode.ts:5665`,`:5701-5705`, gated on `usageBreakdown.length > 1`.

**Impact** — In any session that switched models — `/model`, a compaction/summary model override, an OpenRouter `auto` route, or a subagent on a different model — the user cannot see which model spent the money. pi itemises `openrouter/anthropic/claude-sonnet-4.5: $0.31 (412k tokens)` under the total; cyrup shows only the sum. cyrup already carries `AssistantMessage.response_model`, so the data is present and unused.

**Fix** — Port the breakdown half next to the existing totals code: `usage_cost_breakdown(entries: &[SessionEntry]) -> Vec<UsageCostBreakdownEntry>` keyed on `provider/response_model.unwrap_or(model)`, with the `Tools/summaries` bucket reproduced by name and by membership (toolResult + branch summary + compaction, not merely "unattributed"). Render it in `crates/cyrup-tui/src/app.rs:4192-4218` under pi's `len() > 1` guard.

**Verify** — A fixture session with turns on two different models yields two entries whose `cost` sums to `stats.cost` exactly; a turn whose assistant message carries `responseModel` is attributed to the response model, not the requested one; compaction usage lands in `Tools/summaries`; a single-model session renders no breakdown at all.

## PROV-037 — Two `auth-guidance.ts` formatters unported; the preflight's message text and OAuth-expiry branch diverge

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

> **Scope corrected down from the auditor's medium.** The claim that cyrup has *no* submit-time
> auth preflight is **false** and is recorded as rejected in `## Coverage`:
> `crates/cyrup-session-svc/src/session.rs:1071-1090` `prepare_and_assemble` step 3 runs
> `has_configured_auth` and returns `SessionServiceError::NoConfiguredAuth` **before** assembly or
> any HTTP, citing `agent-session.ts:1062-1075`, and the model it checks is the active one
> (`compaction_model` is kept in sync at `session.rs:3880`). What remains is message text plus one
> branch.

**cyrup** — `crates/cyrup/src/diagnostics.rs:155-166` ports exactly two of pi's four functions — `get_provider_login_help()` and `format_no_models_available_message()`. `formatNoModelSelectedMessage` and `formatNoApiKeyFoundMessage` have no counterpart: `rg 'No model selected|No API key found' crates/` returns nothing (the only near-match, `cyrup-config/src/login.rs:739`, is the unrelated "No API key providers available."). The OAuth-expiry branch is also absent: `rg 'Authentication failed for|re-authenticate' crates/` = 0 hits, and the preflight has no `checkAuth` second chance — it consults the cached `has_configured_auth` only.

**upstream** — `pi/packages/coding-agent/src/core/auth-guidance.ts:18-25` @v0.83.0 defines both missing formatters. Consumers, all in `core/agent-session.ts` @v0.83.0: `:418`/`:438` (`_getRequiredRequestAuth` throws `formatNoApiKeyFoundMessage(model.provider)` both when the resolver reports "authHeader requires a resolved API key" and when auth resolves to nothing), `:1179`/`:1791` (`throw new Error(formatNoModelSelectedMessage())`), `:1194` (the same after the `hasConfiguredAuth || await checkAuth(...)` preflight at `:1183-1185`), and the OAuth-specific branch at `:1186-1193`: `Authentication failed for "<provider>". Credentials may have expired or network is unavailable. Run '/login <provider>' to re-authenticate.`

**Impact** — The refusal happens at the right time; it says the wrong thing. A user with an expired OAuth token gets cyrup's generic `NoConfiguredAuth` rather than pi's message naming the provider, distinguishing expiry from a network outage, and telling them to run `/login <provider>`. And because cyrup has no `checkAuth` second chance, a provider whose credential is present but not in the cached configured-auth set is refused where pi would re-check and proceed.

**Fix** — Add `format_no_model_selected_message()` and `format_no_api_key_found_message(provider: &str)` to `crates/cyrup/src/diagnostics.rs` next to the two already there (both are pure string builders over `get_provider_login_help`) and re-export from `crates/cyrup/src/lib.rs:45`. Then extend the existing preflight at `crates/cyrup-session-svc/src/session.rs:1071-1090` to reproduce `agent-session.ts:1177-1195` exactly: no model ⇒ `formatNoModelSelectedMessage`; not configured ⇒ fall back to `Models::check_auth` (PROV-031) before refusing; on refusal, the OAuth-expiry message when the provider is OAuth-backed, else `formatNoApiKeyFoundMessage`.

**Verify** — With no credential for the selected provider, submitting is refused before any HTTP request (assert a faux origin receives zero connections — this already passes; the new assertion is on the text) and the message names the provider and `/login`; with an OAuth provider whose stored token fails to refresh, the message is the expiry variant; with no model selected at all, the no-model-selected variant.

## PROV-038 — TEST DEFECT: the catalog roster guard compares an array against its own literal length

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** confirmed

> **Impact corrected down from the auditor's medium.** The headline claim — that a parse error in
> any of the five uncovered catalogs "ships silently and degrades the provider to zero models" — is
> **wrong for four of them**, and is recorded as rejected in `## Coverage`: the very next test,
> `every_registered_provider_has_a_non_empty_catalog` (`catalog_data.rs:106-115`), iterates
> `all_providers()` and fails on any provider exposing zero models.

**cyrup** — `crates/cyrup-provider/tests/catalog_data.rs:48-79` `const CATALOGS: &[(&str, &str)]` lists 30 `include_str!`ed catalogs, and `:85-86` asserts `CATALOGS.len() == 30, "catalog roster drifted from the file set"`. That compares the array against its own literal length, so it can never observe the drift its message claims to detect. The directory holds **35** files; absent from the array are `amazon-bedrock.json`, `github-copilot.json`, `google-vertex.json`, `openai-codex.json` and `openrouter-images.json`. The stated purpose at `:81-83` — "Production loaders swallow a parse error into `Vec::default()`, so without this a typo'd catalog ships as an empty provider" — is served for the *registered* providers by the sibling test, but the **per-model field assertions** (empty id, `context_window == 0`, empty `base_url`) run only over the 30-entry array, and `openrouter-images.json` is uncovered entirely because it is an images provider absent from `all_providers()`.

**upstream** — pi needs no equivalent: `packages/ai/src/providers/*.models.ts` are TypeScript modules that fail at build time, each a generated two-line re-export whose data comes from `npm run generate-models` (`pi/.gitignore:11`). The reference the roster is meant to track is the 39 `*.models.ts` modules at v0.84.1.

**Impact** — Per-model field defects in the five newest, least-reviewed catalogs — including the 109-row `amazon-bedrock.json`, the largest cyrup ships and never independently field-diffed (PROV-004) — go uncaught, and `openrouter-images.json` has no guard at all. It is also the direct reason PROV-030 went unnoticed: a roster test that walked the directory and cross-checked each row's `api` against the registry would have failed the moment `google-vertex.json` landed. And the assertion message actively misleads a reader into believing roster drift is covered.

**Fix** — Replace the hand-maintained `CATALOGS` array with a directory walk. `include_str!` cannot take a glob, so either (a) generate the array from a `build.rs` that reads `src/providers/catalog/*.json`, or (b) drop `include_str!` and read the directory at test time from `CARGO_MANIFEST_DIR`, asserting that every `.json` present parses non-empty AND that the set of file stems equals the provider ids in `all_providers()` plus the images providers. In the same test, assert `builtin_registry().contains(&model.api)` for every row — that is PROV-030's regression guard and costs one line — and assert the manifest count equals the file count (PROV-039).

**Verify** — Delete a byte from `amazon-bedrock.json` and the test must fail (today it passes). Add a sixth catalog file without touching the test and it must fail until the provider is registered. Set any row's `api` to an unregistered value and it must fail.

## PROV-039 — `catalog_manifest.json` claims 31 catalogs from `pi@91585d9a`; it is the live overlay staleness floor

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-provider/src/providers/catalog_manifest.json` records `"generatedAt": "2026-07-10T16:34:43Z"`, `"source": "pi@91585d9a3829831b07560901c4b3e9bbe3b4e35a"`, and a note describing "the 31 embedded catalogs under providers/catalog/*.json". The directory holds **35** files, and `providers/google_vertex.rs:17-27` states its 10 rows are "the verbatim contents of pi `packages/ai/src/providers/google-vertex.models.ts` at commit `b0c2a90e` (2026-07-17)" — a week after the manifest's timestamp. The value is not decorative: `providers/all.rs:78-94` `builtin_model_data_generated_at()` feeds `remote_catalog.rs:188-196` `remote_models(entry, local_generated_at)`, which discards a persisted overlay whose `last_modified <= local_generated_at`.

**upstream** — `pi/packages/coding-agent/src/core/remote-catalog-provider.ts:6` `REMOTE_CATALOG_REFRESH_INTERVAL_MS` and `:32-40`/`:44` `withRemoteCatalog` — upstream's staleness comparison is against the shipped build's own model data, which pi regenerates wholesale, so a single generation timestamp is always accurate. cyrup's manifest is the stand-in for that guarantee and no longer holds.

**Impact** — The staleness floor is about a week earlier than the newest embedded data. A pi.dev overlay whose `Last-Modified` falls between 2026-07-10 and 2026-07-17 is accepted and can shadow the freshly-extracted `google-vertex` rows (and, if their extraction revisions differ, `amazon-bedrock`/`github-copilot`/`openai-codex`) with older pricing and context windows — exactly what the manifest's own note says it exists to prevent. **The in-tree counter-argument is incomplete**: `providers/github_copilot.rs:34-38` argues a newer extraction "cannot violate the floor invariant" because a lower `generatedAt` only makes the overlay more likely to be accepted and an overlay can never REMOVE a model — but `catalog.rs:9-12` states the overlay CAN replace a model by id, so an accepted-but-older overlay still shadows a newer embedded row.

**Fix** — Set `generatedAt` to the LATEST pi revision any embedded catalog was taken from (currently `b0c2a90e`, 2026-07-17) and correct the count in the note. Better: make the manifest per-provider (`{provider: {generatedAt, source}}`) and have `remote_models` compare per provider rather than against a single global floor. Either way the bump belongs to whatever produces catalogs — PROV-018's `xtask gen-catalogs` must rewrite it — and until that exists, PROV-038's directory walk should assert the count.

**Verify** — The manifest's catalog count equals the number of files in `src/providers/catalog/`; `generatedAt` is not earlier than any per-provider extraction revision recorded in the provider modules' doc comments; a `remote_catalog` test in which an overlay dated 2026-07-14 is discarded, not accepted.

## PROV-040 — `fetchDeferred` / `cancelDeferred` unported — the deferred data model round-trips but no handle can be redeemed

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — The DATA half is fully ported: `crates/cyrup-core/src/message.rs:172-188` `StopReason::Deferred`, `:462-475` `deferred: Option<Box<DeferredHandle>>`, `:497-505` `struct DeferredHandle` in pi's field order, serialized at `:563-565`, round-tripped by `crates/cyrup-test-support/tests/deferred_interop.rs`. The BEHAVIOUR half is absent: `rg 'fetch_deferred|cancel_deferred' crates/` = 0 hits; neither `ApiImpl` (`api/mod.rs`) nor `Provider` (`provider.rs:17-52`) nor `Models` (`collection.rs`) declares them. cyrup's own doc at `message.rs:182-186` admits it.

**upstream** — Genuine post-baseline drift, checked by tag: `git show v0.84.1:packages/ai/src/types.ts | grep fetchDeferred` hits `:271-276` (`fetchDeferred?` / `cancelDeferred?` on `ProviderStreams`, with `DeferredFetchOptions{wait?}` long-poll and `DeferredCancelOptions`) while the same grep at v0.83.0 returns nothing. Plumbed through `models.ts:143-148` (`Provider`), `:217-222` (`Models`), `:706-731` (dispatch, throwing `Provider ${model.provider} does not support deferred responses` when absent), `:835-857` (multi-api composition) and `api/lazy.ts:69-93`. Only `providers/faux.ts:567`,`:633` implements it today; no real provider does.

**Impact** — A pi session JSONL containing a `stopReason: "deferred"` assistant message loads and re-exports correctly in cyrup, and then the handle is inert: no API polls it, none cancels it. Low today because no first-party pi provider produces a deferred handle either, so the reachable consequence is interop with a third-party provider that does, plus an embedder that cannot express the capability. It becomes user-visible the moment any provider starts returning deferred responses.

**Fix** — Add optional `fetch_deferred`/`cancel_deferred` to the `ApiImpl` trait (`api/mod.rs`) defaulting to a `ProviderError` reproducing pi's message (`models.ts:713-716`), thread them through `Provider` and `Models`, and implement them in `crates/cyrup-provider/src/faux.rs` mirroring `providers/faux.ts:567-660` so the path is exercisable. Carry `DeferredFetchOptions.wait` on the request options.

**Verify** — Against the faux provider: a scripted deferred turn yields an assistant message with `StopReason::Deferred` and a populated `DeferredHandle`; `fetch_deferred(model, handle, {wait: 0})` returns the settled turn; `cancel_deferred` succeeds; the same calls against any real provider return the exact `does not support deferred responses` text pi throws.

## PROV-041 — False in-tree provenance citations, including a wrong "1:1 port" claim

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — Three instances. (1) `crates/cyrup-provider/src/collection.rs:306-307`: "1:1 port of Pi `refresh`, models.ts:198-214" — the method it documents is at `:317`. (2) `crates/cyrup-ext-subagents/src/extension.rs:11299-11302`: "`[CYRUP-DELTA]` … cyrup-provider has no checkAuth/getAvailable port yet (PROV-003 — cyrup ships no login flow at all)", while `auth/oauth/` now holds 11 flow modules and `OAuthAuth::login` exists at `auth/mod.rs:124`. (3) Found while checking PROV-030: `crates/cyrup-provider/src/providers/openai_codex.rs:134-136` says the `openai-codex-responses` impl is "not registered today" when `api/mod.rs:158-161` registers it.

**upstream** — `pi/packages/ai/src/models.ts:196-216` @v0.83.0 is `CreateModelsOptions` (`:196-200`) followed by `mergeHeaders` (`:202-216`). `refresh` is declared at `:147` and implemented at `:276-328`, with its options/result types at `:46-56` — and the signature is emphatically not 1:1 with cyrup's `Option<&str>` → `Result<(), ProviderError>` (PROV-S05).

**Impact** — `CLAUDE.md` makes these citations the provenance record for the port, so a false one is worse than none: a reader who checks `models.ts:198-214` finds `mergeHeaders` and either concludes the port is incoherent or, more likely, stops checking. Concretely, the "1:1 port" claim asserts equivalence for a function missing four upstream features, which is part of why PROV-S05 sat at low for two passes; the subagents citation attributes a live limitation to an item whose stated cause no longer exists, which is how PROV-003's status went stale in the ledger.

**Fix** — (1) Correct `collection.rs:306-307` to cite `models.ts:147` (declaration) and `:276-328` (implementation) at a named tag, and replace "1:1 port of Pi `refresh`" with an explicit delta note naming the four missing pieces (options object, per-provider error map, abort signal, cache-restore-on-failure) pointing at PROV-S05. (2) Correct `extension.rs:11299-11302` to say the flows exist and the missing piece is `Models::get_available`/`check_auth` (PROV-031). (3) Delete the "not registered today" claim at `openai_codex.rs:134-136`.

**Verify** — Every `models.ts:NNN` citation in `cyrup-provider` resolves to the construct it names at the tag it names. A cheap systematic guard: a CI lint extracting `<file>.ts:<line>` citations from doc comments and checking each against a pinned upstream worktree — the same class of defect as the ledger's correction 1.

## PROV-042 — `ModelsStreamTransforms.transformHeaders` unported — `before_provider_headers` has no seam

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `rg 'transform_headers|transformHeaders|before_provider_headers' crates/` returns zero hits workspace-wide. `collection.rs`'s `stream`/`stream_simple` (`:165`, `:236`) take a `StreamOptions` with no transform hook, and header assembly terminates inside `AuthHelper::apply_auth`/`merge_headers` at `collection.rs:377-404` (auth headers merged with option headers) with no post-merge callback. The pieces on either side ARE ported: `cyrup-session-svc/src/attribution.rs:117` `merge_provider_attribution_headers` and `session.rs:2734-2741` compute the attribution set, and the two sibling extension hooks `before_provider_request` / `after_provider_response` are fully wired (`cyrup-ext/src/event.rs:124-125`, `cyrup-ext/src/facade.rs:588-592`, applied per api impl).

**upstream** — `pi/packages/ai/src/models.ts:58-64` @v0.83.0 `interface ModelsStreamTransforms { transformHeaders?: (headers: ProviderHeaders) => ProviderHeaders | Promise<ProviderHeaders> }`, mixed into `ModelsApiStreamOptions` and `ModelsSimpleStreamOptions`; applied at `:480-483` (`if (options?.transformHeaders) headers = await options.transformHeaders(headers ?? {})`, then stripped from what is passed on) and again at `coding-agent/src/core/model-runtime.ts:449-451`. Its production consumer is `coding-agent/src/core/sdk.ts:318-327`, which folds in `mergeProviderAttributionHeaders` and **then** runs the `before_provider_headers` extension hook (`core/extensions/runner.ts:1049-1065`, type at `extensions/types.ts:687`, registration at `:1212`).

**Impact** — Two things. (1) No extension can observe or modify outbound provider headers. `before_provider_headers` is one of three provider-lifecycle hooks and the only one missing, so an extension that adds a corporate proxy header, strips an identifying header, or rewrites an `Authorization` for a gateway simply cannot exist in cyrup while its two siblings work — and because there is no seam, this is invisible from the extension author's side rather than a documented refusal. (2) Attribution headers are attached by a different mechanism (threaded onto the agent at construction, `session.rs:2732-2733`), so they are computed once per model rather than per request, and the ordering guarantee upstream provides — attribution merged BEFORE the hook, so extensions see the final set and win — cannot be reproduced.

**Fix** — Add `transform_headers: Option<Arc<dyn Fn(HeaderMap) -> BoxFuture<HeaderMap> + Send + Sync>>` to `StreamOptions` (`stream.rs`), apply it in `AuthHelper::apply_auth` (`collection.rs:377-404`) after auth and option headers are merged and before the options reach the api impl — pi's exact position at `models.ts:480` — and strip it from what the impl sees. Then in `cyrup-session-svc`, supply a closure that calls `merge_provider_attribution_headers` and dispatches the new `before_provider_headers` extension event. The WIT / event-catalog half belongs to area 06 and must land in the same ABI bump as the other pending export changes.

**Verify** — A `transform_headers` closure appending `x-test: 1` is observed on the wire by a faux origin for **every** api impl, not just one; the closure receives the already-merged auth + option headers, so removing `x-api-key` inside it actually suppresses it; the transform is not visible to the api impl as an option field. Then an extension registering `before_provider_headers` sees the attribution headers already present and its return value wins.

## PROV-043 — Bedrock is the only api impl with no request retry, where pi inherits the AWS SDK's 3-attempt default

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-provider/src/api/bedrock_converse_stream.rs:449-470` builds its own client via `build_client_for_target(..., opts.timeout_ms)` and issues a single `request.send()` inside a `tokio::select!`; `rg -i retry crates/cyrup-provider/src/api/bedrock_converse_stream.rs` finds only comments about the turn-level classifier. It is the one api impl absent from the `ProviderRetry::from_options(opts)` call list — the other seven are `anthropic_messages.rs:210`, `openai_completions.rs:146`, `openai_responses.rs:207`, `azure_openai_responses.rs:184`, `google_generative_ai.rs:151`, `mistral_conversations.rs:153`, `pi_messages.rs:224`.

**upstream** — `pi/packages/ai/src/api/bedrock-converse-stream.ts:223` `new BedrockRuntimeClient(config)` with a config (`:150-222`) that sets credentials, region, token and `requestHandler` but never `maxAttempts` or `retryStrategy` — so the AWS SDK v3 **standard** retry mode applies: 3 attempts with jittered backoff on throttling and 5xx, inside a single pi turn.

**Impact** — A Bedrock `ThrottlingException` that pi swallows transparently becomes a visible turn failure in cyrup, falling through to the coarser turn-level classifier (`cyrup-session-svc/src/session.rs:4023`) instead of being retried in place. On the largest catalog cyrup ships (109 rows), on a provider whose throttling is routine.

**Fix** — Wrap the `request.send()` at `bedrock_converse_stream.rs:449-470` in `ProviderRetry::from_options(opts)` like the other seven impls, mapping the SDK's throttling/5xx error shapes onto the retryable predicate in `utils/provider_retry.rs`. Defaults must match the AWS standard mode (3 attempts) rather than the crate's own default, since that is what pi inherits.

**Verify** — A faux Bedrock origin returning `ThrottlingException` twice then a valid event stream completes the turn; the same origin returning it four times fails with the SDK-equivalent attempt count, not on the first response.

## PROV-044 — `AWS_BEDROCK_FORCE_HTTP1` unported, and cyrup's client negotiates h2 with no override

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — An env-var sweep over the Bedrock surface — `git grep -ohE '"AWS_[A-Z0-9_]+"' v0.83.0 -- packages/ai/src` yields 13 names — finds cyrup covering 12 (`AWS_REGION`, `AWS_DEFAULT_REGION`, `AWS_PROFILE`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_BEARER_TOKEN_BEDROCK`, `AWS_BEDROCK_SKIP_AUTH`, `AWS_BEDROCK_FORCE_CACHE`, `AWS_CONTAINER_CREDENTIALS_FULL_URI`/`_RELATIVE_URI`, `AWS_WEB_IDENTITY_TOKEN_FILE` across `api/bedrock_converse_stream.rs` and `providers/amazon_bedrock.rs`) and missing exactly one: `rg AWS_BEDROCK_FORCE_HTTP1 crates/` = 0 hits.

**upstream** — `pi/packages/ai/src/api/bedrock-converse-stream.ts:206-209` — `else if (getProviderEnvValue("AWS_BEDROCK_FORCE_HTTP1", options.env) === "1") { config.requestHandler = new NodeHttpHandler(); }`, with the comment "Some custom endpoints require HTTP/1.1 instead of HTTP/2".

**Impact** — Not moot in cyrup: the workspace pins `reqwest` with the `http2` feature (`Cargo.toml:151`) and `stream/sse.rs:141`,`:158` builds the client with no `http1_only()`, so ALPN negotiates h2 against any endpoint that offers it. A user behind a custom Bedrock endpoint or corporate gateway that requires HTTP/1.1 has the failure pi added this escape hatch for, and no override.

**Fix** — Read `AWS_BEDROCK_FORCE_HTTP1` alongside the other Bedrock env vars in `api/bedrock_converse_stream.rs` and, when `"1"`, build that request's client with `reqwest::ClientBuilder::http1_only()`. Thread it through `build_client_for_target` (`stream/sse.rs:152-157`) rather than duplicating client construction.

**Verify** — With `AWS_BEDROCK_FORCE_HTTP1=1`, the client offers only `http/1.1` in ALPN against a faux origin that logs the negotiated protocol; unset, h2 is offered as today; the variable has no effect on any non-Bedrock impl.

## PROV-045 — openai-responses `reasoning` branch drops pi's xAI `include` clause and its `reasoningSummary`-only trigger

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-provider/src/api/openai_responses.rs:382-404`: inside `if model.reasoning`, the branch is keyed solely on `clamp_thinking_level(model, opts.reasoning) != Off`, and there is no `model.provider == "xai"` clause anywhere in `build_params`.

**upstream** — `pi/packages/ai/src/api/openai-responses.ts:311-327` @v0.83.0 differs in two ways in the same block. (a) The first arm fires on `if (options?.reasoningEffort || options?.reasoningSummary)` — a caller setting only `reasoningSummary` gets `reasoning: {effort: "medium", summary}` **plus** `include: ["reasoning.encrypted_content"]`, where cyrup falls to the `off` arm and discards the summary that `reasoning_summary_or_auto` (`openai_responses.rs:388`) is wired to read. (b) `if (model.provider === "xai") params.include = ["reasoning.encrypted_content"];` sits **outside** the if/else, so an xAI reasoning model gets `include` even on the off path.

**Impact** — Neither divergence is reachable from the embedded catalogs today — `xai.json` is 8/8 `openai-completions`, and no cyrup caller sets `reasoning_summary` outside the Codex route — but both are reachable from a user `models.json` that routes an xAI model through `openai-responses`, which is exactly what the compat surface exists to allow. The consequence is a dropped reasoning summary and, on xAI, missing encrypted reasoning content that the next turn cannot replay.

**Fix** — Reproduce `openai-responses.ts:311-327` literally: gate the first arm on `reasoning_effort.is_some() || reasoning_summary.is_some()`, and lift the xAI `include` assignment out of the if/else so it applies on both paths. Mirror in `azure_openai_responses.rs` only if pi does (it does not — check before copying).

**Verify** — Body-shape tests: a model with `reasoning` and only `reasoning_summary` set emits `reasoning.effort == "medium"`, the summary, and `include: ["reasoning.encrypted_content"]`; an `xai`-provider model with reasoning OFF still emits `include`; a non-xai model with reasoning off emits neither.

## PROV-046 — Boolean tool-arg coercion accepts `"True"` / `" true "` where pi rejects the call

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-provider/src/validate.rs:307-318` `coerce_boolean` does `s.trim().to_ascii_lowercase().as_str()` before matching `"true"`/`"false"`.

**upstream** — `pi/packages/ai/src/utils/validation.ts:94-100` @**v0.83.0** compares exactly — `if (typeof value === "string") { if (value === "true") return true; if (value === "false") return false; }` — no trim, no case fold; anything else falls through unchanged (the `case "boolean"` arm runs `:90-111`, with the numeric `1`/`0` arm at `:102-109` and the passthrough `return value` at `:110`) and is then rejected by the type check. **Byte-identical, and at the same offsets, at v0.84.1.**

> **Citation corrected in the 2026-08-12 repair pass** (critique finding 9). Recorded as
> `validation.ts:89-99 @v0.84.1`: the range was off by ~5 (it starts one line before the `case`
> label and stops mid-arm), and the tag named was the *latest*, not the ported baseline the item is
> classified against. Both tags carry the identical text at `:90-111`, so the classification never
> turned on it — but the item is a strictness comparison, and a range that clips the arm it compares
> is exactly the citation a reader cannot check.

**Impact** — A model emitting `{"recursive": "True"}` has its call **executed** by cyrup with `true` and **refused** by pi with a schema error. That is a silent behavioural divergence on the tool-dispatch path, not a cosmetic one: the two agents take different actions on identical model output. Same class in `coerce_number`/`coerce_integer` (`validate.rs:279`,`:296` use `parse_*(s.trim())`), but there pi's `Number(value)` also tolerates surrounding whitespace, so only the boolean arm actually diverges. Note this is the *permissive* direction, which is why it is low — but it is still a divergence, and PROV-S01 closed by comparing arm presence, not arm strictness.

**Fix** — In `validate.rs:307-318`, match `"true"`/`"false"` exactly with no `trim()` and no case fold, per `validation.ts:94-100` @v0.83.0. Leave the numeric arms alone (verify `Number(" 1 ") === 1` before touching them).

**Verify** — Coercion tests: `"true"` ⇒ `true`; `"True"`, `" true "`, `"TRUE"` ⇒ left unchanged and then rejected by the type check, matching pi; `1`/`0` ⇒ `true`/`false` (unchanged, `validation.ts:102-109`); `null` ⇒ `false` (unchanged).

---

## Findings absorbed 2026-08-12 from the `packages/ai/src/utils/` + `packages/coding-agent/src/bun/` sweep

Five items, filed by the repair pass. Their provenance is the **surface-driven sweep** README blind
spot 1 prescribes and critique finding 11 named as a hole: eleven upstream files that appeared in no
gap-analysis file at all — `packages/ai/src/utils/{sanitize-unicode,node-http-proxy,event-stream,abort-signals,hash,json-parse,typebox-helpers,provider-env}.ts`
and `packages/coding-agent/src/bun/{cli,register-bedrock,restore-sandbox-env}.ts` — read at **both**
`v0.83.0` and `v0.84.1` with every exported symbol traced to its cyrup consumer by ripgrep over
`crates/`. Six of the eleven turned out to be faithfully ported or genuinely N/A and are recorded
under `## Coverage`; `json-parse.ts` and `node-http-proxy.ts` are the two that produced defects, and
in both cases the *structure* is a good port with a specific arm wrong.

**`sanitizeSurrogates` is the item the sweep was chartered on, and it splits in two.** The
**outbound** direction is correctly ported as a deliberate no-op — `api/compat.rs:455-462`
`sanitize_surrogates` returns `text.to_string()` with a doc comment explaining that a Rust `String`
is well-formed UTF-8 by type invariant, so an unpaired surrogate is unrepresentable and there is
nothing to strip — and it is applied at every one of pi's ~30 call sites. That half needs no work.
The **inbound** direction has no counterpart at all, and it is where the damage is: `PROV-048`.

## PROV-047 — The `httpProxy` setting reaches only the streaming wire APIs; OAuth, the agent proxy transport and extension HTTP bypass it

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** confirmed

**cyrup** — `crates/cyrup-session-svc/src/builder.rs:229-239` `http_proxy_overlay()` turns the `httpProxy` setting into a `ProviderEnv` map `{HTTP_PROXY, HTTPS_PROXY}`, and `:1200-1203` attaches it via `agent_builder.provider_env(overlay)` — and nowhere else. That overlay is read by exactly one code path: `build_client_for_target(url, ctx, auth.env.as_ref(), timeout)` (`crates/cyrup-provider/src/stream/sse.rs:181-192`), used by the streaming api impls (`anthropic_messages.rs:185`, `openai_completions.rs:121`, `openai_responses.rs:182`, `azure_openai_responses.rs:154`, `google_generative_ai.rs:126`, `openai_codex_responses.rs:332`, `pi_messages.rs:199`, `mistral_conversations.rs:123`, `bedrock_converse_stream.rs:450`) plus `images/openrouter.rs:97`. Every **other** outbound client is built by `build_client()` (`stream/sse.rs:140-144`), which applies only the idle timeout and consults neither `resolve_http_proxy_url_for_target` nor the overlay. Its production callers, enumerated by `rg 'build_client\(\)' --include='*.rs' crates/`: five OAuth flows (`auth/oauth/anthropic.rs:443`, `openai_codex.rs:552`, `xai.rs:525`, `openrouter.rs:372`, `radius.rs:468`), the agent proxy transport (`crates/cyrup-agent/src/proxy.rs:455`), and — not previously noted — the non-streaming provider dispatch at `crates/cyrup-provider/src/wire.rs:472`. The extension HTTP capability is a third case, worse: `crates/cyrup-ext/src/caps/http.rs:599-600` `client_builder()` is a bare `reqwest::Client::builder()` with no proxy handling of any kind. Note the asymmetry this creates *even for env-var users*: `build_client_with_proxy` calls `.no_proxy()` on the negative arm (`sse.rs:165`) so provider traffic uses cyrup's ported resolver, while `build_client()` silently falls back to reqwest's own built-in env detection — two different `no_proxy`/`all_proxy` implementations inside one process.

**upstream** — `pi/packages/coding-agent/src/core/http-dispatcher.ts:43-48` @**v0.83.0** — `applyHttpProxySettings(httpProxy)` writes `process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy` **process-wide** (`:46`). `:79-93` `configureHttpDispatcher()` installs an `undici.EnvHttpProxyAgent` (`:85`) as the global dispatcher with `bodyTimeout`/`headersTimeout` (`:87-88`), and `:103` calls `undici.install?.()` so `globalThis.fetch` runs on that same dispatcher. Invoked at startup from `packages/coding-agent/src/cli.ts:18` and `rpc-entry.ts:10`, re-applied from `main.ts:744-745`. Consequence: **every** `fetch()` in the pi process is proxied — OAuth token exchange, model-catalog refresh, extension HTTP, the agent proxy transport — not just provider streaming. `packages/ai/src/utils/node-http-proxy.ts:92-112` is the *second*, per-request layer on top of that, not a replacement for it. At v0.84.1 the same code sits at `:45-50` / `:81-93` / `:108` (+2 then +5 of drift; contents unchanged).

**Impact** — On a corporate network whose only egress is an HTTPS proxy, a user who configures it the documented way — `httpProxy` in `settings.json` rather than shell env vars — gets working model streaming and then a hard failure on `cyrup` OAuth login, on every silent OAuth token refresh, on the `ProxyStreamFn` transport, and on every extension HTTP call. The error is a bare connect/DNS failure that never mentions that a proxy was configured and ignored, so the user cannot attribute it. Env-var users are partially rescued by reqwest's default detection — but by a *different* resolver than the one cyrup ported, so `no_proxy` / `all_proxy` / bare-hostname semantics diverge between provider traffic and everything else in the same process. Filed high rather than medium because the failure is total for the affected population, silent as to cause, and the mechanism (a setting that reaches one of four egress paths) will keep producing new instances every time a new client is built.

**Fix** — Give the proxy the same process-global treatment the idle timeout already has. Add `configure_http_proxy(Option<String>)` beside `configure_http_idle_timeout` in `crates/cyrup-provider/src/stream/sse.rs`, set it from `crates/cyrup-session-svc/src/builder.rs:1200` right next to the existing `configure_http_idle_timeout(timeout_ms)` call at `:1213`, and change `build_client()` into `build_client_for(target_url)` that runs `utils::node_http_proxy::resolve_http_proxy_url_for_target` against the configured value overlaid on the ambient env — then thread the target URL through the five OAuth `client()` methods, `cyrup-agent/src/proxy.rs:455` and `cyrup-provider/src/wire.rs:472`. Apply the same resolver inside `cyrup-ext/src/caps/http.rs:599` `client_builder()` and add `.no_proxy()` there so reqwest's competing detection is retired process-wide and the ported resolver is the single authority. Keep `http_proxy_overlay` as well — it is pi's second layer and is correct — rather than replacing it.

**Verify** — With `settings.json` = `{"httpProxy":"http://127.0.0.1:PORT"}` and **no** `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` in the environment, point a loopback proxy that logs CONNECT targets at that port. Assert (a) an OAuth token-exchange request and (b) an extension `http.fetch` both appear in the proxy log. Both are absent today; only the provider stream shows up. Then a negative test: with `NO_PROXY` naming the OAuth host, that request must bypass — proving the ported resolver, not reqwest's, is the one deciding.

## PROV-048 — A lone-surrogate `\uXXXX` escape in a provider SSE frame kills the whole turn, because `serde_json` rejects what `JSON.parse` accepts

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-provider/src/utils/json_parse.rs:104-113` `parse_json_with_repair`: `serde_json::from_str` → on failure `repair_json` → retry **only** `if repaired != json` → else `None`. serde_json hard-errors on unpaired surrogate escapes (`serde_json-1.0.150/src/read.rs:911-913` `ErrorCode::LoneLeadingSurrogateInHexEscape`, `:957-959` for a high surrogate not followed by a low one), and `repair_json` re-emits a syntactically well-formed `\uD83D` **verbatim** (`json_parse.rs:67-75`: four valid hex digits ⇒ copied unchanged, `index += 6`), so `repaired == json` and the function returns `None`. Both SSE callers treat `None` as fatal to the stream: `api/anthropic_messages.rs:1439-1449` emits `"Could not parse Anthropic SSE event {event}"` and `return`s; `api/google_generative_ai.rs:975-985` emits `"Could not parse Gemini SSE chunk"` and `return`s. `api/compat.rs:455-462` `sanitize_surrogates` is a deliberate no-op (correct **outbound** — a Rust `String` cannot hold a lone surrogate) so nothing on the cyrup side ever neutralises an **inbound** one.

**upstream** — `pi/packages/ai/src/utils/json-parse.ts:85-95` @**v0.83.0** `parseJsonWithRepair` calls `JSON.parse`, which **accepts** `"\ud83d"` and yields a JS string holding the lone surrogate. `packages/ai/src/api/anthropic-messages.ts:467` (`const event = parseJsonWithRepair<RawMessageStreamEvent>(sse.data)`) therefore parses the frame normally and the turn continues. The lone surrogate is stripped on the way back **out** by `packages/ai/src/utils/sanitize-unicode.ts:21-25` `sanitizeSurrogates`, whose regex is `/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g` — imported by nine wire APIs at v0.83.0 (`anthropic-messages`, `bedrock-converse-stream`, `google-generative-ai`, `google-shared`, `google-vertex`, `mistral-conversations`, `openai-completions`, `openai-responses-shared`, `openrouter-images`) and applied to every outbound text/thinking/system field. Its own docstring (`sanitize-unicode.ts:4-5`) names the reason: unpaired surrogates "cause JSON serialization errors in many API providers" — the same characters, arriving inbound. **`json-parse.ts` and `sanitize-unicode.ts` are byte-identical, at identical offsets, at v0.83.0 and v0.84.1** (verified, not assumed); `anthropic-messages.ts:467` is also unmoved.

**Impact** — One provider frame carrying an unpaired surrogate escape aborts the **entire assistant turn** in cyrup with `Could not parse Anthropic SSE event content_block_delta`, discarding all streamed content, where pi renders the text and drops only the bad code unit. The same `parse_json_with_repair` weakness applies to any JSON produced by a JS/TS peer whose `JSON.stringify` well-formed-escapes a lone surrogate — an MCP server response, an extension payload, or a **pi-written session JSONL being resumed by cyrup** — each of which becomes a hard parse failure rather than a lossy but successful read. That last case is the one that makes this high: the interop guarantee this port exists to provide is that a pi session opens in cyrup.

**Fix** — Make cyrup's parse tolerate exactly what `JSON.parse` tolerates, and converge on pi's end state by deleting the offending code unit. In `crates/cyrup-provider/src/utils/json_parse.rs::repair_json`, extend the `Some('u')` valid-hex arm (`:67-75`): if the decoded value is in `0xD800..=0xDBFF` and is **not** immediately followed by a `\u` escape in `0xDC00..=0xDFFF`, emit nothing (drop the escape); if it is in `0xDC00..=0xDFFF` and was not preceded by a high surrogate, likewise drop it. That makes `repaired != json`, so `parse_json_with_repair:108-111` retries and succeeds — and it is `sanitizeSurrogates` semantics applied at the escape level, so the resulting string matches what pi would have sent on the next request. Apply the same rule in `parse_partial::parse_string`'s `'u'` arm (`:280-291`) — that is `PROV-050`, and the two should land as one change since they are the same three-line predicate.

**Verify** — `parse_json_with_repair(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi \ud83d there"}}"#)` returns `Some` with `text == "hi  there"`; and an `anthropic_messages` decoder test that feeds that exact frame followed by `message_stop` terminates with `StreamEvent::Done`, not with the `Could not parse Anthropic SSE event` error terminal. Red today on both. Add the paired case as a regression guard: `"\ud83d\ude00"` must survive as `😀`, not be dropped.

## PROV-049 — `repair_json` mis-handles an invalid `\u` escape: pi keeps `\u` and skips 2, cyrup doubles the backslash and skips 1

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-provider/src/utils/json_parse.rs:67-89`. The `Some('u')` match arm only `continue`s when the next four chars are hex; otherwise the comment at `:76` says "fall through to invalid-escape handling below" and control reaches `:87-88` `repaired.push_str("\\\\"); index += 1;` — emitting a **doubled** backslash and reprocessing `u` as a literal. `VALID_JSON_ESCAPES` at `:17` does contain `'u'`, but the `match` arm at `:67` shadows it, so the `Some(nc) if VALID_JSON_ESCAPES.contains(&nc)` guard at `:78` is never reached for `u`.

**upstream** — `pi/packages/ai/src/utils/json-parse.ts` @**v0.83.0**, byte-identical and at identical offsets at v0.84.1. The `if (nextChar === "u")` block at `:60-67` only `continue`s on a valid `/^[0-9a-fA-F]{4}$/` run; on failure control falls to `:69` `if (VALID_JSON_ESCAPES.has(nextChar))` — and `VALID_JSON_ESCAPES` at `:3` is `new Set(['"', "\\", "/", "b", "f", "n", "r", "t", "u"])`, which **contains `"u"`** — so pi emits `\u` unchanged with `index += 1` plus the `for` loop's own `index++` = 2 consumed. pi's output for such an input is therefore byte-identical to its input, `repairedJson !== json` is false at `:90`, and `parseJsonWithRepair` rethrows.

**Impact** — Divergent tool invocations on the same provider bytes, silently. For a model-emitted argument blob `{"path":"C:\users\bob"}` (unescaped Windows path — a routine model mistake), pi's repair is a no-op ⇒ parse fails ⇒ `parseStreamingJson` falls through to `partial-json` and typically yields `{}`, so pi **drops** the argument. cyrup produces `{"path":"C:\\users\\bob"}`, parses it, and hands the tool a real path — a **different filesystem operation** than pi would have performed, with nothing in either transcript indicating the repair diverged. The mirror case (`"\uZZZZ"`) also differs. Medium rather than high because cyrup's behaviour is arguably the more useful one; it is still a silent behavioural fork on the tool-dispatch path, and parity is the requirement.

**Fix** — In `crates/cyrup-provider/src/utils/json_parse.rs`, restructure the escape handling so a `\u` with an invalid hex run falls into the `VALID_JSON_ESCAPES` branch rather than the invalid-escape branch: after the 4-hex test fails, emit `\` + `u` and advance `index += 2`, exactly as `json-parse.ts:69-73` does. The simplest shape is to test `VALID_JSON_ESCAPES.contains(&nc)` first for all characters including `u`, with the 4-hex `\uXXXX` fast path checked before it. Land with `PROV-048`, which edits the adjacent arm of the same `match`.

**Verify** — `assert_eq!(repair_json(r#"{"p":"C:\users\bob"}"#), r#"{"p":"C:\users\bob"}"#)` — the repair must be a **no-op**, so `parse_json_with_repair` returns `None` and `parse_streaming_json` falls to the partial parser, matching pi. Red today (it currently returns the double-escaped form and parses it).

## PROV-050 — `parse_partial` silently deletes every astral character written as a surrogate pair

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-provider/src/utils/json_parse.rs:280-291`. Each `\uXXXX` escape is decoded **independently**: `char::from_u32(code)` returns `None` for every code point in `0xD800..=0xDFFF`, so for `\uD83D` the branch pushes **nothing** and advances 4, then does the same for the following `\uDE00` — the pair is gone with no diagnostic. Separately, when `hex.len() != 4` or the digits are not hex, `code` is `None` and `self.pos` is never advanced (`:285-290` only advances inside the `if let Some(code)`), so the raw hex characters leak into the decoded string as literals. This is the recovery path the module exists for — `json_parse.rs:12-13` states its purpose is that "the prior cyrup behaviour … discarded a truncated tool call's arguments".

**upstream** — `pi/packages/ai/src/utils/json-parse.ts:104-121` @**v0.83.0** (identical offsets at v0.84.1) delegates the tolerant parse to the `partial-json` npm package (`import { parse as partialParse } from "partial-json"`, `:1`; called at `:113` and `:117`), which completes the truncated document and hands it to `JSON.parse`. `JSON.parse` combines `\uD83D\uDE00` into `U+1F600` per the JSON spec, so pi's recovered arguments keep the character. Reached from every wire API's tool-call finalisation (`anthropic-messages.ts:659`,`:695`; `openai-completions.ts:331`,`:533`; `openai-responses-shared.ts:633`,`:640`,`:690`; `bedrock-converse-stream.ts:500`,`:566`; `mistral-conversations.ts:467`,`:482`; `pi-messages.ts:248`).

**Impact** — Silent data loss in tool inputs. When a tool call's arguments arrive truncated — the exact case this parser handles — and the model wrote non-ASCII as `\u` escapes (routine when a model emits JSON), every emoji, astral CJK extension character and mathematical symbol is deleted from the arguments the tool receives, while BMP characters survive. A `write`/`edit` call recovered this way writes silently corrupted content, and nothing in the transcript marks the deletion. The non-advancing `pos` on a malformed escape is a second, quieter corruption in the same arm.

**Fix** — In `crates/cyrup-provider/src/utils/json_parse.rs::parse_string`, handle the surrogate pair the way serde_json's own reader does (`serde_json-1.0.150/src/read.rs:957-969`): on a high surrogate `0xD800..=0xDBFF`, look ahead for a `\u` escape decoding into `0xDC00..=0xDFFF` and combine as `((hi - 0xD800) << 10 | (lo - 0xDC00)) + 0x1_0000`; drop a genuinely unpaired surrogate (matching `sanitizeSurrogates` and `PROV-048`'s fix, so the two arms agree); and when `code` is `None`, advance past the malformed escape rather than leaving `pos` unmoved. Ship with `PROV-048` — same file, same predicate, and shipping them apart leaves the two `\u` decoders disagreeing about surrogates.

**Verify** — `parse_streaming_json_object(Some(r#"{"msg":"hi 😀"#))` yields `msg == "hi 😀"` (red today — it yields `"hi "`); `parse_streaming_json_object(Some(r#"{"msg":"x \u12"#))` does not leak `12` into the value; a lone `\ud83d` in a truncated blob is dropped, not preserved, matching `PROV-048`.

## PROV-051 — Codex header-phase timeout substituted with a whole-stream reqwest read timeout, losing pi's message and its abort/timeout distinction

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-provider/src/api/openai_codex_responses.rs:331-336` passes `opts.timeout_ms` straight into `build_client_for_target(...)`, which becomes `reqwest::ClientBuilder::read_timeout` (`crates/cyrup-provider/src/stream/sse.rs:86-95` `with_idle_timeout`, applied at `:181-192`). There is no separate deadline on the header phase and no dedicated message — a stall surfaces as a generic `ProviderError::Transport`. `combineAbortSignals` has no counterpart: `rg 'combine_abort|CombinedAbortSignal' crates/` returns 0 hits, and `crates/cyrup-provider/src/utils/` has no `abort_signals.rs`.

**upstream** — `pi/packages/ai/src/api/openai-codex-responses.ts:401-419` @**v0.83.0** (`:402-420` at v0.84.1 — one line of drift, contents identical) builds `const headerTimeoutSignal = httpTimeoutMs !== undefined && httpTimeoutMs > 0 ? AbortSignal.timeout(httpTimeoutMs) : undefined` (`:401`), merges it with the caller's signal via `combineAbortSignals([options?.signal, headerTimeoutSignal])` (`:403`) — `packages/ai/src/utils/abort-signals.ts:6-41`, byte-identical **and at identical offsets** at v0.83.0 and v0.84.1 (verified, not assumed) — passes **only** the merged signal to `fetch`, and calls `combinedSignal.cleanup()` in a `finally`, which removes the listeners (`abort-signals.ts:35-39`) the moment headers arrive, so the timeout stops applying to the body. On failure it distinguishes the two causes at `:412-413`: `if (headerTimeoutSignal?.aborted && !options?.signal?.aborted) throw new Error(\`Codex SSE response headers timed out after ${httpTimeoutMs}ms\`)`.

**Impact** — A Codex endpoint that accepts the TCP connection and never returns response headers — the corporate-proxy-swallowing-the-request case, i.e. the same population `PROV-047` affects — yields an unattributable transport error in cyrup instead of a message naming the timeout and its configured value, so the user cannot tell a stalled endpoint from a network drop or from their own cancellation. Secondarily, because cyrup's deadline is per-read across the entire body while pi's header signal is explicitly cleaned up once headers land, a long legitimately-quiet body read is bounded by a number pi had already stopped applying.

**Fix** — In `crates/cyrup-provider/src/api/openai_codex_responses.rs`, wrap **only** the connect/header phase (`open_sse(...)`) in `tokio::time::timeout(Duration::from_millis(n))` when `opts.timeout_ms` is `Some(n)` with `n > 0`. On elapse, first check the `CancelToken` (pi's `!options?.signal?.aborted` guard) and, if not cancelled, emit `format!("Codex SSE response headers timed out after {n}ms")` as the terminal error. Leave the client's `read_timeout` in place for the body phase — that is the correct analogue of pi's global undici `bodyTimeout`/`headersTimeout` (`core/http-dispatcher.ts:87-88` @v0.83.0). Porting `combineAbortSignals` itself is **not** required: its only upstream consumer at either tag is this call site, and `CancelToken` + `tokio::time::timeout` covers it — record that as the mechanism difference rather than filing a second item.

**Verify** — Loopback server that accepts the connection and never writes a byte; drive `openai_codex_responses` with `timeout_ms = Some(200)` and assert the terminal error text is exactly `Codex SSE response headers timed out after 200ms`. Then cancel the `CancelToken` before the deadline and assert the terminal is the aborted one, not the timeout message. Red today on both.

## PROV-052 — The shipped binary's default model is the in-process faux TEST provider, so a bare `cyrup -p hi` fails with the internal string "No more faux responses queued"

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** **confirmed — reproduced in the shipped binary; both sides read** · **observed 2026-08-13** (headless-binary; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Filed 2026-08-13 from a live run.** `rg 'faux' docs/gap-analysis/*.md` shows every existing
> mention treats faux as a *test* provider; **no item covers it being the shipped default.** This is
> a clean instance of README structural blind spot 1 — nobody wrote an item for "what happens when
> you just run the binary", so no pass could see it.

**cyrup** — `crates/cyrup/src/provider.rs:356` — `select_provider`'s match is `None | Some("faux") => Ok(Arc::new(FauxProvider::new()))`, so **an absent provider *and* an absent model prefix route to the in-process test double**. The doc comment at `:345-347` states this as intended ("No explicit provider/prefix, or an explicit `faux` ⇒ the in-process `FauxProvider`"), and `:421-424` extends it: a `--model` whose prefix is not a known provider also maps to faux, with the comment "a non-provider prefix maps to faux (ledgered) — no warn". Meanwhile `crates/cyrup/src/cli.rs:871` prints `--provider <name>    Provider name (default: google)` in the shipped help — **the documented default and the actual default disagree.**

**upstream** — `pi/packages/coding-agent/src/cli/args.ts:239` @v0.83.0 prints the identical help line, `--provider <name>    Provider name (default: google)`. pi has **no faux provider on any production path**: `packages/ai/src/providers/faux.ts` is the test double and is not reachable from model resolution. An out-of-box `pi -p hi` with no credential therefore cannot produce this failure — it resolves google and reports a missing credential.

**Impact** — Reproduced with a scratch `HOME` and agent dir, no credentials, **no `--offline`**, no `--model`/`--provider`:

```
$ cyrup --no-session --no-extensions -p hi </dev/null
EXIT=1
stdout: (empty)
stderr: No more faux responses queued
```

Contrast, same fixture, provider named explicitly — the correct, actionable error:

```
$ cyrup ... --provider google -p hi        -> provider 'google' is not configured (no credential or env key)
$ cyrup ... --model openai/gpt-4o -p hi    -> provider 'openai' is not configured (no credential or env key)
```

And interactively on a first run with no credentials, the footer **advertises the test double as the live model**:

```
39| 0.0%/128k (auto) • xp                                          faux/faux-1
```

So the out-of-box experience for a new user with no API key is a **test-harness internal string and exit 1**, where pi gives credential guidance — and the interactive session presents itself as connected to a model named `faux/faux-1`. This is the first thing anyone who installs the binary sees.

**Fix** — Make the default resolve to the documented provider: change `provider.rs:356` so `None` falls through to the registry lookup for `google` (pi's documented default) and only an **explicit** `Some("faux")` reaches `FauxProvider`. Re-examine `:421-424` in the same change — a `--model` with an unrecognised prefix should report the unknown provider, as it does for a recognised-but-unconfigured one, rather than silently becoming a test double; that arm's "(ledgered) — no warn" comment should cite this item or be deleted. Audit the test suite for fixtures that rely on the implicit default and make them pass `--provider faux` explicitly, so the test double stays reachable on purpose.

**Verify** — `cyrup -p hi` with no credentials, no flags and a scratch `HOME` must print a credential/`/login` message naming `google` and must **not** print `No more faux responses queued`. Interactive: the footer on a first run must not read `faux/faux-1`. `cyrup --provider faux -p hi` must still reach the faux provider, and the existing faux-backed tests must stay green once they name it.

## Coverage

**Read first-hand at cyrup HEAD `04c1ba2`** (branch `david/cyrup`, tree clean; docs HEAD `a9000b1`). In `crates/cyrup-provider`: `api/mod.rs` (`register_builtins`, `ApiRegistry::get`/`contains`), `api/compat.rs` (`ModelCompat`/`ResolvedCompat`/`ResolvedResponsesCompat` in full), `api/openai_responses.rs` (`build_params`, `build_headers`, `convert_responses_tools`, the reasoning tail, incomplete-details mapping), `api/azure_openai_responses.rs` (`build_params`), `api/openai_codex_responses.rs` (body build), `api/anthropic_messages.rs` (`build_headers`, `is_oauth` derivation), `api/google_generative_ai.rs` (tools/toolConfig, thought-signature retention, `map_stop_reason`), `api/bedrock_converse_stream.rs` (client build, send path, error body, `raw_stop_reason` producers), `validate.rs` in full, `utils/{error_body,provider_retry,overflow,estimate}.rs` in full, `stream/sse.rs` (client build, idle timeout, retry loop, error path), `collection.rs` (whole public surface + `refresh` + `AuthHelper::apply_auth`), `provider.rs`, `wire.rs` (stream dispatch), `env_api_keys.rs`, `catalog.rs`, `providers/{all,builtin_oauth,github_copilot,openai_codex,google_vertex,amazon_bedrock}.rs`, `providers/catalog_manifest.json`, all 35 `providers/catalog/*.json` (mechanically, for compat-key and api-id distribution), `auth/mod.rs`, `auth/oauth/{load,github_copilot,openai_codex}.rs`, `tests/catalog_data.rs` (in full, both tests). In `crates/cyrup-core`: `message.rs` (StopReason, AssistantMessage, DeferredHandle, both hand-written serializers). Outside the area, only to close or open items honestly: `cyrup-ext/src/{wrapper.rs:137-143,event.rs:124-125,facade.rs:588-592}`, `cyrup-ext-subagents/src/extension.rs:11290-11315`, `cyrup-session-svc/src/{attribution.rs,builder.rs:1213-1214,session.rs:1071-1090,2726-2745}`, `cyrup-tui/src/{app.rs:4192-4218,status.rs:155-175}`, `cyrup/src/{diagnostics.rs:150-170,provider.rs:71-130}`, `cyrup-config/src/login.rs:360,739,784`.

**Added by the 2026-08-12 repair pass** — in `crates/cyrup-provider`: `utils/json_parse.rs` in full (`repair_json`, `parse_json_with_repair`, `parse_streaming_json`, `parse_partial::parse_string`), `utils/node_http_proxy.rs` in full including its ten tests, `utils/hash.rs`, `stream.rs:612-660` (the `AssistantMessageEventStream` aliases), `stream/sse.rs:138-192` (all three client builders and their exact caller sets), `api/compat.rs:450-465`, `api/anthropic_messages.rs:1435-1452` and `api/google_generative_ai.rs:972-988` (the SSE parse-failure terminals), `api/openai_codex_responses.rs:328-340`, `providers/all.rs:1-51` (the port-status doc table — this is what `PROV-030`'s widened Fix is about) and `:170-200`, `providers/{github_copilot.rs:597,openai_codex.rs:451}` (`is_subscription`). Outside the area: `cyrup-session-svc/src/builder.rs:222-242` (`http_proxy_overlay`), `cyrup-agent/src/proxy.rs:455`, `cyrup-ext/src/caps/http.rs:595-605` (`client_builder`), and `crates/cyrup-provider/src/wire.rs:472`. Registry source consulted for two claims: `serde_json-1.0.150/src/read.rs:911-913`, `:957-969`.

**Read first-hand upstream**, via `git -C pi show <tag>:<path>` at **both** `v0.83.0` (the ported baseline) and `v0.84.1` (latest): `packages/ai/src/types.ts` (KnownApi, KnownProvider, StreamOptions, all four compat interfaces, ToolResultMessage, AssistantMessage, ProviderStreams), `models.ts` (Models/Provider interfaces, `refresh`, `getAvailable`, `filterModels`, `ModelsStreamTransforms`, `calculateCost`), `env-api-keys.ts`, `utils/{validation,overflow,estimate,error-body,provider-retry}.ts`, `api/{openai-responses,openai-responses-shared,azure-openai-responses,openai-completions,openai-codex-responses,anthropic-messages,bedrock-converse-stream,google-shared,google-generative-ai,github-copilot-headers,constrained-sampling,lazy}.ts`, `providers/{all,anthropic,github-copilot,openai-codex,google-vertex,faux}.ts`; `packages/agent/src/agent-loop.ts` (`createToolResultMessage`, read as a literal, not only as an interface); `packages/coding-agent/src/core/{auth-guidance,usage-totals,cache-stats,agent-session,sdk,model-registry,http-dispatcher,remote-catalog-provider}.ts` and `modes/interactive/interactive-mode.ts:3340-3360`, `:5640-5720`; `packages/ai/CHANGELOG.md` (which is what established PROV-033's true kind).

**Version-lag sweep** performed with `git diff --stat v0.83.0..v0.84.1 -- packages/ai/src` (52 files, +1545/−507) and per-file diffs on `types.ts`, `models.ts`, `openai-completions.ts`, `simple-options.ts`, `openai-responses-shared.ts`, `error-body.ts`, `overflow.ts`, `validation.ts`. Everything it surfaced maps to an existing entry **except** `fetchDeferred`/`cancelDeferred`, filed as PROV-040 — it appears in no other area file and in no PARITY-GAPS VL-P row.

### Surface sweep — `packages/ai/src/utils/` + `packages/coding-agent/src/bun/` (added by the 2026-08-12 repair pass)

The sweep critique finding 11 called for: eleven upstream files that appeared in **no** gap-analysis
file at any prior pass, read at **both** `v0.83.0` and `v0.84.1`, with every exported symbol traced
to its cyrup consumer by ripgrep over `crates/`. Files read: `packages/ai/src/utils/{sanitize-unicode,node-http-proxy,event-stream,abort-signals,hash,json-parse,typebox-helpers,provider-env}.ts`
and `packages/coding-agent/src/bun/{cli,register-bedrock,restore-sandbox-env}.ts`, plus
`packages/coding-agent/src/core/http-dispatcher.ts` (which is what made `PROV-047` legible).
Yield: `PROV-047` … `PROV-051`.

**Confirmed covered by this sweep — do not re-derive:**

- **`sanitize-unicode.ts:21-25` `sanitizeSurrogates` (outbound) → `api/compat.rs:455-462` `sanitize_surrogates`.** Applied at every one of pi's call sites: `anthropic_messages.rs:685/691/836/845/1032/1042/1071/1096/1101/1106`, `bedrock_converse_stream.rs:1304/1333/1456`, `google_generative_ai.rs:317/665/723/755/777/789`, `mistral_conversations.rs:321/508/534/542/580/924/931`, `openai_completions.rs:1049/1115/1184/1209/1304/1311`, `openai_responses.rs:546/558/616/689/703`, with `images/openrouter.rs:167-168` documenting the same no-op inline. **The no-op is CORRECT outbound**: `&str`/`String` are well-formed UTF-8 by type invariant, so an unpaired surrogate is unrepresentable and there is nothing to strip. pi's `google-vertex.ts:473` and `google-shared.ts` sites map onto the same shared helper. The **inbound** direction is a separate, real gap and is `PROV-048`.
- **`node-http-proxy.ts` (all of it) → `crates/cyrup-provider/src/utils/node_http_proxy.rs`**, a faithful line-for-line port: `DEFAULT_PROXY_PORTS` `:12-22`, `getProxyEnv`'s four-way lower/upper overlay-then-ambient precedence with the JS `||` empty-string skip `:38-59`, `shouldProxyHostname`'s `.every` semantics including `*`, exact host, `host:port` port-qualification and the leading-`.`/`*` suffix rule `:62-106` (the `rsplit_once(':')` split reproduces `^(.+):(\d+)$` including the `::1` case), the `${protocol}://` prefix for a scheme-less value `:134-136`, and `UNSUPPORTED_PROXY_PROTOCOL_MESSAGE` verbatim `:25` with the `Got {protocol}` suffix `:32-33`. Ten unit tests at `:195-309`. What is **not** covered is where the resolver is *called from* — `PROV-047`.
- **`resolveHttpProxyUrlForTarget` wiring into the wire APIs → `stream/sse.rs:181-192` `build_client_for_target`**, called by all nine streaming impls and by `images/openrouter.rs:97`; the negative arm's `.no_proxy()` (`sse.rs:165`) correctly suppresses reqwest's competing detection so the ported resolver alone decides. Bedrock specifically is covered here despite having had no read-against-upstream pass otherwise.
- **`hash.ts` `shortHash` → `utils/hash.rs:27-40`.** `s.encode_utf16()` matches `charCodeAt` (UTF-16 code units, not `char`s), `wrapping_mul` matches `Math.imul`, both seeds and all four mix constants match `hash.ts:3-11`, and `to_base36` (`:9-23`) reproduces `Number.prototype.toString(36)`. Four cross-checked reference vectors pinned at `hash.rs:74-80`.
- **`event-stream.ts` `EventStream` / `AssistantMessageEventStream` / `createAssistantMessageEventStream` → `stream.rs:629-655`**, aliasing `cyrup_core::FinalizingStream`/`FinalizingSink` and supplying pi's `isComplete` (`Done|Error`) and `extractResult` closures literally; `collect_message` (`stream.rs:612-628`) is the `result()` equivalent. Where pi's `finalResultPromise` would hang forever if `end()` is called with no result and no terminal pushed (`event-stream.ts:38-48`), cyrup synthesises an error message (`synth_terminal_less_message`, `stream.rs:657+`) — a strict improvement, not a lost behaviour, recorded so it is not re-filed as a divergence.
- **`json-parse.ts` `repairJson` structural control flow → `utils/json_parse.rs:36-101`**: the in-string state machine, the trailing-backslash-at-EOF case, the valid-4-hex `\uXXXX` passthrough, the raw-control-character escape table (`json_parse.rs:23-32` vs `json-parse.ts:10-25`) and the invalid-escape doubling are all present, and the index arithmetic was checked against pi's `for (…; index++)` at each `continue`. `parseStreamingJson`'s four-stage fallback **order** (strict-with-repair → partial → partial-of-repaired → empty) is reproduced at `json_parse.rs:119-135`. Three defects *inside* this otherwise-correct port are `PROV-048`/`049`/`050`.
- **`abort-signals.ts` `combineAbortSignals`** — its **only** upstream consumer at either tag is `openai-codex-responses.ts:403`, and that call site is ported (cyrup uses `CancelToken` + reqwest `read_timeout`). The signal-merge mechanism has no second consumer to port. The residual diagnostic gap at that call site is `PROV-051`.
- **`bun/cli.ts:2,8 registerBunOAuthFlows()`** (→ `packages/ai/src/bun-oauth.ts:11-21`) → cyrup compiles all seven flows in statically: `auth/oauth/{anthropic,openai_codex,github_copilot,openrouter,kimi_coding,xai,radius}.rs`, exported as `load_*_oauth` from `auth/oauth/mod.rs:60-61` — a 1:1 set with no bundler seam needed. (Whether those loaders are *reachable* is `PROV-029`, a different question.)
- **`bun/cli.ts:13` `process.env.PI_CODING_AGENT = "true"`** — already filed and deliberately **not** re-filed here: `04-cyrup-tools.md:416-430` `TOOL-031` and `PARITY-GAPS.md:108-111` `PB-5` already cite `cli.ts:13` and `rpc-entry.ts:7`. The v0.84.1 `AI_AGENT` half is in the same item.

**Ruled mechanism-N/A by this sweep, with the reason stated so the carve-out is checkable:**

- `packages/coding-agent/src/bun/restore-sandbox-env.ts` (whole file) — a workaround for oven-sh/bun#27802, stated in its own header: a Bun-**compiled** binary inside a sandbox sees an empty `process.env`, so it re-reads `/proc/self/environ`. It self-disables on any non-Bun runtime (`if (!process.versions?.bun) return;`, `:20`). Rust has no equivalent defect — `std::env::vars()` reads the real `environ` block the kernel handed the process. There is no bug to work around, so nothing to port.
- `packages/ai/src/utils/provider-env.ts:15-39 getBunSandboxEnvValue` — the same Bun workaround, duplicated into the ai package for direct consumers (stated at `:10-13`). N/A for the identical reason. `getProviderEnvValue` (`:45-52`) carries the only portable behaviour and is already covered by cyrup's `ProviderEnv` overlay + `AuthContext::env` precedence (exercised by `node_http_proxy.rs:38-59`). *(Critique finding 11 flagged `provider-env.ts` as unread and correctly guessed it was coverage; this records it.)*
- `bun/cli.ts:6` `process.emitWarning = (() => {})` — silences Node runtime deprecation writes to stderr that would corrupt the TUI's alternate screen. A Rust binary emits no such warnings and has no `emitWarning` channel to override.
- `bun/cli.ts:10,14,15` — the `await import("./register-bedrock.ts")` / `await import("../cli.ts")` **ordering** exists so a bundler cannot statically follow the import chain into the Node-only AWS SDK (reason spelled out at `packages/ai/src/api/bedrock-converse-stream.lazy.ts:4-13`). Rust has no bundler and no module-evaluation order to stage; `cyrup-provider` links `bedrock_converse_stream` unconditionally.
- `packages/coding-agent/src/bun/register-bedrock.ts` (whole file) + `bedrock-converse-stream.lazy.ts:15-30` `setBedrockProviderModule` — an override seam existing **only** because pi loads bedrock through a variable specifier a bundler cannot resolve, so the Bun single-file build must inject a statically imported module instead (stated verbatim at `bedrock-converse-stream.lazy.ts:17-21`). cyrup's `ApiRegistry` (`api/mod.rs:80-118`) is already a lazy get-or-init factory registry and bedrock is registered unconditionally at `api/mod.rs:152-155`; `ApiRegistry::register_impl` (`:96-98`) already provides the same substitution capability for an embedder. **This is the file the critique specifically flagged as unread on a provider with no upstream pass — it is genuinely N/A, and now says so.**
- `packages/ai/src/utils/typebox-helpers.ts` `StringEnum` (whole file) — a TypeScript/typebox **authoring** helper that makes TS extension authors emit `{type:"string", enum:[…]}` instead of typebox's default `anyOf`/`const` encoding, which Google's API rejects (`:4-5`). It has **zero runtime consumers** in pi at either tag — every hit is documentation, a test or an example extension. cyrup extension tools declare raw JSON Schema through the WIT `register-tool` seam, so the `anyOf`/`const` shape is never generated and there is no `Type.Unsafe` to wrap. The constraint it encodes (Google rejects `anyOf`/`const`) belongs to the Google schema-conversion layer in `api/google_generative_ai.rs`, not to a helper.

### Citation sweep (2026-08-12 repair pass, critique finding 9)

Every "@v0.83.0" / "@v0.84.1" / "identical at both tags" claim in this file was re-resolved with
`git -C /Users/davidmaple/cyrup.ai/pi show <tag>:<path>` against the tag actually named. Recorded so
the next pass re-checks the *corrections*, not the whole file.

**Corrected — 9 wrong at the named tag, 3 tightened.** The five rows marked ★ are the `AGENT-020`
defect exactly: a v0.84.1 offset asserted to hold at v0.83.0.

| item | was | is @v0.83.0 | @v0.84.1 |
|---|---|---|---|
| ★ PROV-020 / PROV-009 | `agent-loop.ts:777-791` | `:773-787` (literal `:775-785`, spread `:783`) | `:777-791` |
| ★ PROV-020 | `types.ts:445` / `:441` | — | `isError :444`, `addedToolNames :443` |
| ★ PROV-028 | `openai-completions.ts:646-652` | `:638-645` | `:646-653` |
| ★ PROV-029 (**high**) | `github-copilot.ts:16` quoted with `isSubscription: true` | `:16`, **no `isSubscription`** — it is a v0.84.1 addition | `:16` with it |
| ★ PROV-029 | `openai-codex.ts:15` | `:13` (`:15` is `models:`) | `:13-17` |
| PROV-023 | `openai-responses.ts:72` for `supportsExplicitPromptCacheMode` | `:75` (`:72` is `supportsStrictMode` — wrong construct, not a shift) | same |
| PROV-024 | `types.ts:578`; `openai-completions.ts:650-659`, `:1477`, `:1520` | `:579`; `:647-656`, `:1473`, `:1515` | `:605`; `:655-664`, `:1527`, `:1572` |
| PROV-031 | `models.ts:149/152/151/167/170`, `:405-410` | `:150/153/152/168/171`, impl `:394-409` w/ call `:407` | +26 |
| PROV-003 | `anthropic.ts:9-14`; `models.ts:167`/`:170` | `:12-15`; `:168`/`:171` | `models.ts:194`/`:197` |
| PROV-016 | "identical at v0.83.0 and v0.84.1" for `validation.ts:189-201` | `:189-201` correct | code identical, offsets `:196-208` |
| PROV-032 | `models.ts:107-111`, `:405-410`; `github-copilot.ts:20-26` | `:111` (doc `:105-110`), `:407`; `:19-27` | `github-copilot.ts` unmoved; `models.ts` shifts — **not re-derived, cite v0.83.0** |
| PROV-046 | `validation.ts:89-99` **@v0.84.1** | `:94-100` (arm `:90-111`), and the ported tag is v0.83.0 | identical offsets |
| PROV-030 | `types.ts:16-27` "at both tags" | `:16-26` (`"google-vertex"` at `:25`) | `:17-27` (at `:26`) |

Also struck: the **section-level** claim under `## GitHub Copilot findings` that "every upstream line
cited is present at both v0.83.0 and v0.84.1". It was false (PROV-028, PROV-029) and it is the shape
of claim that suppresses per-item checking. Per-item tags now live in each `upstream` paragraph.

**Re-verified clean at both tags — do not re-derive:** PROV-019 (`openai-responses.ts:32`,`:289-290`;
`azure-openai-responses.ts:26`,`:292-293` — byte-identical *and* at identical offsets), PROV-027
(`anthropic-messages.ts:867-888`, `:890`), PROV-011 (`constrained-sampling.ts:84`,`:101`,`:136`;
`google-shared.ts:311`/`:321`), PROV-042 (`models.ts:58-64`, `:480`/`:483`), PROV-045
(`openai-responses.ts:313`,`:319`,`:321`,`:327`), PROV-034 (`openai-responses-shared.ts:344`,`:346`,
`:376`), PROV-021 (`env-api-keys.ts:29`,`:75-76`,`:147`), PROV-025 (`types.ts:567` @v0.83.0, exact),
PROV-033 (`openai-responses.ts:49`,`:70`,`:234`), and the whole of `json-parse.ts` /
`sanitize-unicode.ts` / `abort-signals.ts` (byte-identical at identical offsets at both tags).

**Method note for the next pass.** Nine of roughly forty citation clusters were wrong, five of them
the same way. README:224-225 already warns not to fix a citation by shifting it; the complementary
rule this pass suggests is: **never write "identical at both tags" — write the offset for each tag
separately, or write only the classification tag.** A single number cannot be true at two tags unless
it has been checked at two tags, and the phrase reads as though it has been.

**Rejected this pass, with reasons — do not re-derive these.**

1. *"PROV-005 should be re-opened to carry the google-vertex dangling api."* Rejected. Both halves PROV-005 asserted hold at HEAD (nine factories, four providers pushed); re-opening would double-count one defect in the plan and break the stable-id rule. The work lives under PROV-030.
2. *"cyrup has no submit-time auth preflight; the turn is burned against the provider's 401."* Rejected as factually wrong — `cyrup-session-svc/src/session.rs:1071-1090` refuses before assembly and before any HTTP, citing `agent-session.ts:1062-1075`. PROV-037 survives at reduced scope (message text, the `checkAuth` second chance, the OAuth-expiry variant) and reduced severity.
3. *"A parse error in any of the five uncovered catalogs ships silently as a zero-model provider."* Rejected for the four registered ones — `catalog_data.rs:106-115` `every_registered_provider_has_a_non_empty_catalog` iterates `all_providers()` and fails on zero models. PROV-038 survives at low, scoped to the per-model field assertions and to `openrouter-images.json`, which no test touches.
4. *"`sendSessionIdHeader` is a cyrup invention with no upstream referent."* Rejected — `packages/ai/CHANGELOG.md:168` records pi **removing** it in #6496 with a documented migration. PROV-033 is therefore `stale-port`, not `cyrup-original`, and the in-tree citation was accurate at the revision it was written against.
5. *"`Models` lacking `getAvailable`/`checkAuth` is medium."* Rejected — every behaviour exists at another layer (`cyrup-config/src/login.rs`, `cyrup-session-svc/src/session.rs:2726`), and pi's subagents consume the coding-agent `ModelRegistry.getAvailable()`, not `Models.getAvailable()`. PROV-031 stands at low; the only behavioural residue is PROV-032.
6. *"PROV-S05 should rise to medium."* Rejected — `crates/cyrup/src/provider.rs:71-130` already reproduces the allow-network split, the mode gate and the configured-provider restriction. The residue is the result shape, `force` and the abort signal.
7. *"PROV-004's re-opening is medium."* Rejected — it is audit-coverage debt with no demonstrated wrong value, and blind spot 4 below says none can be demonstrated from this workspace. Low, and it is PROV-018's `xtask` that closes it. **Repair pass, 2026-08-12:** for that same reason it is now a `tracker` and out of the counts — an item whose entire Fix is another item's Fix schedules nothing.

**Rejected in the 2026-08-12 repair pass, with reasons — do not re-derive these either.**

8. *"`sanitizeSurrogates` is unported — file it."* **Rejected as stated.** The outbound direction is ported *correctly*, as a documented no-op at `api/compat.rs:455-462` applied at ~30 call sites; a Rust `String` cannot hold an unpaired surrogate, so there is nothing to strip and the no-op is not a shortcut. What is genuinely missing is the **inbound** direction, which pi gets for free from `JSON.parse`'s tolerance and cyrup does not get from `serde_json`. That is `PROV-048`, and it is a `json-parse.ts` defect, not a `sanitize-unicode.ts` one. Filing it against `sanitizeSurrogates` would have pointed the fix at the wrong file.
9. *"`combineAbortSignals` (`packages/ai/src/utils/abort-signals.ts`) is unported — file it."* **Rejected.** Its only consumer at v0.83.0 or v0.84.1 is `openai-codex-responses.ts:403`, and that call site *is* ported by a different mechanism (`CancelToken` + reqwest `read_timeout`). A signal-merge primitive with one consumer whose behaviour is reproduced is a mechanism difference, not a gap. The one behaviour genuinely lost at that call site — the header-phase deadline and its distinct message — is `PROV-051`.
10. *"`register-bedrock.ts` matters because bedrock has had no read-against-upstream pass."* **Rejected on the merits, and recorded because the critique raised it by name.** The file exists solely because pi's bundler cannot resolve a variable module specifier (`bedrock-converse-stream.lazy.ts:17-21`); cyrup's `ApiRegistry` is already a lazy factory registry with an unconditional bedrock registration (`api/mod.rs:152-155`) and an equivalent substitution hook (`:96-98`). Genuinely N/A. **This does not discharge blind spot 3** — bedrock's *wire* implementation is still unread against upstream, and `PROV-043`/`PROV-044` are what that unread state has produced so far.
11. *"`typebox-helpers.ts` `StringEnum` is unported."* **Rejected** — zero runtime consumers upstream at either tag, and cyrup's extension tools declare raw JSON Schema through the WIT seam so typebox's `anyOf`/`const` encoding is never produced in the first place.
12. *"`PROV-029` should be downgraded — pi v0.83.0 does not mark these providers as subscriptions, so the `/login` list may not show them."* **Rejected**, and the item's Impact was corrected instead. The `isSubscription: true` in the item's quoted upstream really is a v0.84.1 property that does not exist at v0.83.0 (that error is fixed), but the marker in *cyrup's* `/login` comes from cyrup's own `is_subscription` impls (`providers/github_copilot.rs:597`, `providers/openai_codex.rs:451`), which return `true` regardless. Both providers are listed and both dead-end. Severity unchanged at high.

**Deliberate non-duplication.** Verified at HEAD and already filed elsewhere; recorded here as re-audit evidence only, not as findings: PARITY-GAPS **PB-1** (radius unregistered — confirmed, `providers/all.rs:140-240`, `env_api_keys.rs:34-73`), **PB-2** (qwen-token-plan ×2 — same lines; both are v0.83.0 port bugs, not lag), **PB-3** (`Models::refresh` — folded into PROV-S05), **VL-P1** baseten, **VL-P2** qwen-token-plan-individual, **VL-P3** `samplingParams`, **VL-P4** `supportsThinkingTokenBudget`, **VL-P5** `telemetryContext`, **VL-P6** `AuthOperationOptions` / OAuth refresh timeout — **area 08's Coverage explicitly hands this to area 01**, scoped as the 15-second `AbortSignal.timeout` threaded through `ModelRuntime.create` / `resolveModelScope` / `listModels` / `refresh`; accepted here, still tracked under VL-P6 rather than given a `PROV-` id, and note it overlaps `PROV-S05`'s missing abort signal, so the two should be scheduled together — **VL-P7** Copilot policy-state fallback, **VL-P10**'s `isRecoverableLength` predicate (confirmed absent: `rg is_recoverable_length crates/` = 0 against `utils/overflow.ts:167-173` @v0.84.1), **VL-P16** management-HTTP retry, **VL-P25** catalog set.

**Blind spots — the next pass should start here.**

1. **No build, no tests run** (task rule). Every fix sketch is unverified-by-compile and the suite's greenness at HEAD is assumed. Specifically, PROV-038's claim that the roster test currently *passes* is inferred from reading `assert_eq!(CATALOGS.len(), 30)` as a tautology; it was not executed.
2. **The SSE decoders are still not line-diffed.** The four large `api/*.rs` decoders (~8k lines: `anthropic_messages`, `openai_completions`, `openai_responses`, `google_generative_ai`) plus the three newest (`bedrock_converse_stream`, `openai_codex_responses`, `pi_messages`) were read only along the paths each item required — headers, `build_params`, tool conversion, stop-reason mapping. Per-event decode fidelity (content-block index bookkeeping, partial-JSON accumulation, usage merging, signature carry-over) is unaudited. This was the prior pass's blind spot too and it has grown by three files.
3. **Three of the four new providers were not read end to end.** The 2026-08-11 pass read `github-copilot` against pi and found three highs. This pass read `google-vertex` closely enough to find PROV-030 but did not do the same for `amazon-bedrock` (109 rows), `openai-codex` (beyond PROV-029 and its temperature handling) or `pi-messages`. The ledger's suggested-order item 8 asks for exactly that; by the Copilot precedent, assume findings are there.
4. **Catalog data accuracy is unauditable from this workspace.** pi does not commit `packages/ai/src/providers/data/*.json` (`pi/.gitignore:11`); every `*.models.ts` at v0.84.1 is a two-line re-export. No per-model pricing, context-window, `maxTokens` or compat-flag claim about the 35 embedded catalogs can be checked — including whether pi's generator now sets `supportsStrictMode` / `sessionAffinityFormat` / `supportsExplicitPromptCacheMode` on models where cyrup's copies do not. PROV-004's original closure rested on a diff taken at `91585d9a`, when the data was still committed, and cannot be reproduced today. Same constraint as PARITY-GAPS OQ-5. **This is why PROV-018's generator is the highest-leverage tooling item in the area.**
5. **Nine of the eleven OAuth flows were not audited.** `cf26010` landed 11 flows under `auth/oauth/`. This pass read `load.rs` (for PROV-029's registry claim) and the Copilot/Codex wiring; `anthropic.rs`, `kimi_coding.rs`, `openrouter.rs`, `xai.rs`, `radius.rs`, `device_code.rs`, `pkce.rs`, `callback.rs` and `page.rs` were not read against their upstream counterparts at all. **PROV-003 is recorded `partially-closed` on the basis that the files EXIST** — precisely the "implementation is not correctness" trap the method warns about. That closure is deliberately flagged as weak.
6. **Area boundaries not crossed.** PROV-035, PROV-036, PROV-037 and PROV-042 each have a consuming half outside this area (cyrup-tui's `/session` renderer and transcript, cyrup-session-svc's submit path, cyrup-ext's WIT event catalog). Consuming line numbers are cited but those crates' surrounding logic was not audited, so the effort estimates for the wiring halves are rougher than for the provider halves. The `:max` thinking-suffix parsing in `cyrup-ext-subagents` (PROV-002's tail) is still area 09's and still unverified here.
7. **Not compared against upstream at all:** `images/` (pi's image surface is four modules plus a 40-entry generated catalog against cyrup's 35 — the count delta is confirmed via PARITY-GAPS, not derived here), `faux.rs`, `legacy_api_aliases.rs`, `session_resources.rs`, `models_store.rs`, `remote_catalog.rs` beyond the staleness-floor lines, and `truncation_parity.rs`.

**Blind spots added or narrowed by the 2026-08-12 repair pass.**

8. **NARROWED — `packages/ai/src/utils/` is no longer unread.** All eight files critique finding 11
   named were read at both tags this pass and are accounted for above: two produced defects
   (`json-parse.ts`, `node-http-proxy.ts`), four are confirmed-covered (`sanitize-unicode.ts`,
   `event-stream.ts`, `hash.ts`, `abort-signals.ts`), two are N/A with the reason stated
   (`provider-env.ts`, `typebox-helpers.ts`). `packages/coding-agent/src/bun/` is likewise closed.
   **What this does NOT close:** `packages/ai/src/utils/` has more files than those eight, and the
   sweep took the critique's list rather than enumerating the directory at the tag. Next pass should
   run `git -C pi ls-tree v0.83.0 packages/ai/src/utils/` and diff against this list.
9. **NEW — the citation layer had a ~20% error rate and nothing checks it.** Nine of roughly forty
   citation clusters in this file were wrong; five were the same defect (a v0.84.1 offset asserted
   at v0.83.0), and one (`PROV-023`'s `:72`) named a *different construct* entirely. This is a
   property of the method, not of any one author: nothing re-resolves a citation once written, and
   the phrase "identical at both tags" actively discourages re-checking. `PROV-041` already proposes
   the durable countermeasure — a CI lint extracting `<file>.ts:<line>` citations from **in-tree doc
   comments** and checking each against a pinned upstream worktree. The same lint pointed at
   `docs/gap-analysis/*.md` would have caught all nine of these. Widen `PROV-041`'s Fix to cover
   both surfaces when it is scheduled.
10. **NEW — the sweep's five new items were verified on the cyrup side by reading, and on the
    upstream side at both tags, but none was reproduced.** In particular `PROV-047`'s claim that
    reqwest's built-in env detection is what rescues env-var users on the `build_client()` path is
    read from reqwest's documented default, not observed; and `PROV-048`'s claim that serde_json
    rejects the escape rests on reading `read.rs:911-913` rather than running it. Both are
    falsifiable in one unit test each, which is what their `Verify` lines describe.
11. **NEW — `crates/cyrup-ext/src/caps/http.rs` is inside `PROV-047`'s fix and outside this area's
    audit.** Only `client_builder()` (`:595-605`) was read. Whether the extension HTTP capability
    diverges from pi's extension `fetch` in any *other* respect — redirect policy, timeout, header
    allow-list — is unexamined here and belongs to area 06.
