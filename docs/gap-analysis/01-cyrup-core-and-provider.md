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

> ### Reconciliation 2026-08-14 — sweeps 1 and 2 applied, counts re-derived
>
> **cyrup HEAD `380c713`** (this file was written against `04c1ba2`), tree clean. Two whole-backlog
> parity sweeps have landed since this file was last edited: **sweep 1 — 232 items across 11 crates**,
> and **sweep 2**, run under the same rules. Area agents were forbidden from editing documentation so
> that a single writer could reconcile all sixteen files in one pass; this block, and the dispositions
> written into the `## Open items` rows below, are that reconciliation. **Every status in this file
> that predates this block is stale — including the header notes above it and the
> `## Status of every item…` table.**
>
> **No ID was renumbered, merged or deleted.** A refuted item keeps its ID with the refutation
> recorded in its row, so nobody re-derives it. Refutations are corrections to *this analysis*, not
> failures of the sweep — see `00-residual-ledger.md`, which now publishes the measured error rate.
>
> **The test architecture changed underneath every path citation in this file.** The integration
> tests were relocated into their crates as unit tests (`63d729a` / `c3982b5` / `d973906`), taking the
> suite from **310 integration binaries to 6 + 8 gated** behind a new **`cyrup-it`** harness crate.
> The gate is now **6440 tests / 6440 passed / 8 skipped in 16.4 s**. Any citation of the form
> `crates/<crate>/tests/<x>.rs` in this file is stale unless it names `cyrup-it`, and note that
> `cyrup-it` is `required-features = ["it"]`, so **the gate does not build or run it**.
>
> **Still a static analysis.** Neither sweep executed the suite: area agents were restricted to
> `cargo check -p <crate> [--all-targets]` and the orchestrator ran the gate once over the combined
> work. Every red-before/green-after claim below is a reasoned argument plus a type-check, and every
> `Verify` line in this file remains a design, not an observation.
>
> **Area 01 — recount: 41 rows → 12 open (0 critical · 1 high · 5 medium · 6 low) + 1 tracker
> (`PROV-004`), and one new item filed and closed on arrival (`PROV-053`).** The header's
> "40 items — 0 critical, 6 high, 14 medium, 20 low" is stale in every column.
>
> **All six of the area's highs are dispositioned.** `PROV-048` closed in sweep 1; `PROV-030` closed
> in sweep 2; **`PROV-027`, `PROV-028` and `PROV-029` were REFUTED at HEAD** — all three were already
> fixed, none of them by sweep 1, so this file had been stale on three of its six highs for at least
> one pass. `PROV-047` is the only high left and it is partially closed with three one-line residuals
> in other crates.
>
> **The most important lesson in this area is about deferrals, not code.** `PROV-030` sat open through
> sweep 1 on the stated ground that "cyrup-provider has no crypto/JWT dep". That premise was checkable
> in about thirty seconds against `Cargo.lock` and was false — `ring` 0.17 was already resolved
> through rustls. It was *also* masking a second, independent defect (`PROV-053`: `EnvAuthContext`
> never expanded `~`, so the Vertex ADC arm was unreachable on every machine) that would have kept the
> feature dead even if the wire API had landed. **Verify a deferral's stated blocker before accepting
> the deferral.**
>
> Two disclosed changes outside the area's ownership, neither authorised in advance: `Cargo.toml`
> gains `ring = { version = "0.17" }` under `[workspace.dependencies]` (additive and provably
> graph-neutral — ring 0.17.14 is already in `Cargo.lock` via rustls and quinn-proto), and
> `crates/cyrup-provider/src/auth/testdata/service_account_test_key.pem` is a checked-in throwaway
> 2048-bit PKCS#8 key that authenticates nothing and is never used at runtime, present only because
> `ring` cannot generate RSA keys at test time. If a secret scanner rejects it, delete the file and
> the two signing tests.
>
> **`PROV-011` is the one L left and was consciously not started**; everything sweep 2 learned is
> transcribed into its row so the next attempt starts further along — in particular that the field's
> home may be `crate::context::ToolDef` rather than `cyrup_core::Tool`, which materially shrinks the
> blast radius, and that `resolveGoogleFunctionCallingMode` (google-shared.ts:311-323 @v0.83.0) puts
> the tool-choice override BEFORE the VALIDATED arm, which is the part a naive port gets backwards.


## Status since the c8bd2ab baseline

> **THIS TABLE IS A FILING-TIME SNAPSHOT, NOT A LIVE STATUS — re-stated 2026-08-19.** A
> `**new — open (…)**` cell records what the item looked like when it was FILED at the pass named in
> the header; it is not re-audited on later passes. The 2026-08-14 reconciliation above already says
> so ("Every status in this file that predates this block is stale"), but the cells still *read* like
> live counts and have been miscounted downstream as such. **`## Open items` is the only authoritative
> status in this file.** Four rows below are closed there and open here: `PROV-030` (`:168`, closed
> 2026-08-14), `PROV-036` (`:174`, closed 2026-08-15), `PROV-047` (`:185`, closed 2026-08-15 — struck
> in place because it was being counted as a live `high` outside this file) and `PROV-048` (`:186`,
> closed 2026-08-14).

| ID | Status | Evidence at cyrup `04c1ba2` / pi `v0.83.0`–`v0.84.1` |
|---|---|---|
| PROV-052 | **FIXED 2026-08-13** | Two separable defects, both closed. **(a) Feature graph:** the `faux` edge in `crates/cyrup/Cargo.toml` `[dependencies]` moved to `[dev-dependencies]`, and `crates/cyrup-test-support` removed from the workspace `default-members` (its `[dependencies]` faux edge unified into a plain root `cargo build`). `cargo tree -p cyrup -e features --edges normal \| grep -c faux` = **0** (was 1); `cargo tree -p cyrup -e features \| grep faux` still reports the dev edge. **(b) Resolution:** `crates/cyrup-provider/src/unconfigured.rs` (new, always compiled) supplies a zero-model provider; `crates/cyrup/src/provider.rs`'s `select_provider` `None` arm returns it and the `Some("faux")` arm is deleted, matching pi where `faux.ts` is absent from `providers/all.ts`, is not a `KnownProvider`, and has zero matches under `packages/coding-agent/src/` @v0.83.0. The empty catalog raises `SessionServiceError::NoModels` (`cyrup-session-svc/src/builder.rs:1453-1455`) ⇒ `main.rs:1899-1902` `no_models_available()` ⇒ `format_no_models_available_message()`, i.e. pi `main.ts:852-855` + `auth-guidance.ts:14-16` — same text, stderr, exit 1. Guarded by `crates/cyrup-provider/tests/faux_not_in_normal_build.rs` (RED-then-GREEN demonstrated mechanically). The five integration tests that spawn `CARGO_BIN_EXE_cyrup` with `--model faux/faux-1` are kept alive by a default-off, test-only `faux` feature on the `cyrup` package enabled through a **self-dev-dependency**, so `cargo test` has the arm and `cargo build`/`--release`/`install` do not. Bare `env -i` `cyrup -p hi`: `No more faux responses queued` → `No models available. Use /login to log into a provider via OAuth or API key. …`. **The item's own Fix text was wrong**: it prescribed defaulting to `google`, but `args.ts:87-88` @v0.83.0 applies no default — `(default: google)` at `args.ts:239` is a stale help line in pi itself. See the item body. |
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
| PROV-011 | **closed** | **CLOSED 2026-08-14 (sweep 6).** The widened scope (four sites, since grown to six) was correct and every one is ported; what remained after sweep 5 was two plumbing frames — `agent.rs:818` and `cyrup-ext/src/wrapper.rs`'s `RegisteredTool` delegation — which made the whole opt-in path dead. See the body. |
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
| PROV-047 | ~~**new — open (high)**, repair pass~~ **CLOSED 2026-08-15** *(see `## Open items`)* | `httpProxy` reaches only the streaming wire APIs; OAuth, the agent proxy transport and extension HTTP bypass it — **closed: both production call sites are live at HEAD**, `cyrup-session-svc/src/builder.rs:296-299` (`apply_http_proxy_settings` calls `cyrup_provider::configure_http_proxy(proxy.clone())` unconditionally, including with `None`, so clearing the setting clears the global) and `crates/cyrup/src/main.rs:177` (the bootstrap call, deliberately ABOVE the package/config and credential-print pre-dispatches, either of which can egress before a session exists). |
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

> **RECOUNTED 2026-08-14 (sweeps 3-6 reconciliation) — counted set: 0 critical, 1 high, 4 medium, 6 low = 11**, plus the one `tracker` (`PROV-004`) and 30 rows now marked CLOSED. `PROV-053` was filed and closed in sweep 2; `PROV-011` closed in sweep 6. `PROV-030` was re-verified at HEAD by sweep 6 and remains correctly closed (`api/google_vertex.rs` is 717 lines with a real `ApiImpl::run`, registered `api/mod.rs:156`, exported `lib.rs:51`, 16 unit tests, zero `todo!`/`unimplemented!`).
>
> *(Previous edition: 0 / 1 / 5 / 6 = 12, 29 closed.)* The "0 critical, 6 high, 14 medium, 20 low = 40" above is superseded.

> **RECOUNTED 2026-08-14 (sweeps 7-8 reconciliation, third edition) — counted set UNCHANGED at 0 critical, 1 high, 4 medium, 6 low = 11**, plus the one `tracker` (`PROV-004`). The table now carries **43 rows: 31 fully closed, 11 open (4 of them partially), 1 `tracker`**. Sweep 8 **filed and closed `PROV-M01` in the same pass** — a real live behaviour defect on `github-copilot`, found by the assigned audit of hand-written delegating trait impls, and the **third instance of the dropped-delegation class** after `TOOL-024` (`RegisteredTool`) and `EXT-M03` (`WasmTool`), and the **first on a non-`Tool` trait**. `PROV-036` and `PROV-037` stay open but their fix sites were corrected — **both land in `crates/cyrup-session-svc`, outside this area's crates**; scheduling either against a provider-only agent will produce a blocked pass, which is exactly the failure the ledger's orchestration section names.

> **RECOUNTED 2026-08-14 (sweep 9, fourth edition) — counted set: 0 critical, 4 high, 8 medium, 11 low = 23**, plus the one `tracker` (`PROV-004`). The table now carries **57 rows: 33 fully closed, 23 open (4 of them partially), 1 `tracker`**. *(Previous edition: 0 / 1 / 4 / 6 = 11, 31 closed, 43 rows.)*
>
> **This is the largest single filing this area has taken, and its provenance is different from every pass before it.** Sweeps 1-8 read the BACKLOG against the code. Sweep 9 enumerated a finite SURFACE — providers, wire-api ids, and the four compat interfaces — mechanically on both sides and diffed in both directions, so it could see the things nobody had written an item for. Fourteen ids: **`PROV-054` … `PROV-067`**, three of them **high**, two **filed and closed in the same pass** (`PROV-061`, `PROV-062`).
>
> **The three highs are all one class and it is the class this project has already shipped four of.** `PROV-023`/`024`/`033`/`034` were each a compat flag defaulting the wrong way; `PROV-054` (grok-4.5 on the wrong WIRE API, on the xai DEFAULT model), `PROV-055` (16 opencode rows leaking a `session_id` header pi suppresses) and `PROV-056` (kimi-coding sending a non-adaptive thinking block plus a beta header pi suppresses, on every model the provider has) are the same shape at data level rather than code level. **A compat flag defaulting the wrong way is a wire difference nobody sees**, and cyrup's resolvers invent a default wherever the catalog is silent — so a stale catalog does not degrade to "missing", it degrades to "confidently wrong".
>
> **`PROV-060` is the one to read first if you only read one.** It refutes the premise `PROV-004` and `PARITY-GAPS.md:931` (`OQ-5`) both rest on — that catalog accuracy is "not statically auditable" — by showing the `*.models.ts` files are full data literals at `b0c2a90e`, the very revision cyrup's manifest names as its provenance floor. That refutation is what makes `PROV-054` … `PROV-059` measurable at all, and it hands `PROV-018` its drift check. Nine sweeps inherited the "unverifiable" verdict; the data was one `git show` away.
>
> **Six of the fourteen are `cyrup-original`** (`PROV-058`, `PROV-061`, `PROV-063`, `PROV-064`, `PROV-065`, `PROV-067`) — surfaces cyrup has that pi does not. Not all are defects, and two are deliberate, but every one is now KNOWN, which is the point: an invented surface is how divergence enters while everyone is looking at parity.
>
> **SWEEP 10, 2026-08-15 — the catalog regeneration LANDED and it closed ten rows in one commit:**
> `PROV-004`, `PROV-018`, `PROV-054`, `PROV-055`, `PROV-056`, `PROV-057`, `PROV-058`, `PROV-060`,
> `PROV-064`, `PROV-065`, plus `PROV-059` with 3 of its 119 differences **REFUTED** and 7 preserved
> as tagged deltas. The routing note below was correct and was followed exactly: they closed as one
> regeneration, not as six hand edits. `xtask/` holds the generator; `cargo run -p xtask --
> gen-catalogs --check` is the drift guard. **The three highs were confirmed at the PORTED TAG, not
> just at `b0c2a90e`** — pi's `ai/scripts/generate-models.ts` is in git at `v0.83.0` and hardcodes
> all three (`:378`/`:1408` for grok-4.5's Responses routing, `:1666` for opencode's
> `openai-nosession`, `:1861-1864` for kimi-coding's `forceAdaptiveThinking`), which is stronger
> evidence than sweep 9 had. **That same script is also why 3 of `PROV-059`'s differences are
> refuted:** where pi hardcodes a value, `v0.83.0` beats `b0c2a90e`, and the Codex GPT-5.6
> `contextWindow` is `272000` there — cyrup was right and `b0c2a90e` is the stale side.
>
> **Residue this did NOT remove, stated plainly:** `b0c2a90e` is 13 days older than `v0.83.0` and
> the catalog data for that window is in git at no revision. Catalog parity is now a claim about
> `b0c2a90e` plus an unbounded delta, and the manifest note says so.

> **SWEEP 11, 2026-08-15 — the non-catalog remainder of the area.** Ten rows dispositioned:
> `PROV-S05`, `PROV-041`, `PROV-063`, `PROV-066`, `PROV-067`, `PROV-036`, `PROV-037` **CLOSED**;
> `PROV-047` and `PROV-025` **CLOSED as already-done at HEAD** (one of them a `high` whose three
> residuals had all landed, one a full port whose row still carried a `rg … = 0 hits` evidence line
> that now returns 12 production hits); `PROV-035` **narrowed to its second render site only**.
> Left open with their real size stated: `PROV-014`, `PROV-040`, `PROV-042`.
>
> **Two lessons, and the first one is the area's recurring shape.** *(1) Two of the ten were stale
> rows, not gaps* — a 22% stale rate on the batch, above the ledger's published ~12%, and both were
> checkable in under a minute by re-running the item's OWN grep. `PROV-025`'s row still quoted
> `rg 'deferred_tools_mode|DeferredToolsMode' crates/ = 0 hits`; `PROV-047`'s three residuals were
> each one grep away. **Re-run an item's stated evidence before writing any code for it.**
> *(2) `PROV-041`'s Verify clause was executed rather than deferred, and it found 20 more instances* —
> every `models.ts:NNN` citation in `cyrup-provider` re-resolved at v0.83.0, of which 20 named the
> wrong construct, INCLUDING a second copy of the exact `:198-214` mis-citation the item was filed
> for. A false-citation item is never "three instances"; it is a sampling of a population, and the
> population is worth counting.
>
> **The `PROV-053` standing lead is discharged and came back CLEAN** — all 42 `ctx.env(...)` sites in
> the crate read against their upstream operator, no second instance of the `Some("")` class. Written
> into `PROV-053`'s row so it is not re-derived.

> **RECOUNTED 2026-08-19 (round-2 refresh, fifth edition) — counted set: 0 critical, 1 high, 3 medium,
> 2 low = 6**, of which two are partially closed (`PROV-035`, `PROV-042`). The table carries **60 rows:
> 54 fully closed, 6 open.** `PROV-004` is no longer a live tracker — it closed with sweep 10's
> regeneration and its cell is struck. *(Previous edition: 0 / 4 / 8 / 11 = 23, 33 closed, 57 rows.)*
> **The delta since sweep 11 is three rows, all filed after it:** `PROV-068` (`high`, still open — an
> explicit `null` in `thinkingLevelMap` read as UNSUPPORTED), `PROV-069` (`critical`, closed the same
> day) and `PROV-070` (`low`, filed 2026-08-19 — the `moonshotai/Kimi-K3` Together addition, which
> had shipped in `2add245` with no row here). **Counts in this file are DERIVED from the table, not
> carried forward:** the four editions above each restate the whole set for that reason, and the
> `## Status since the c8bd2ab baseline` table is a filing-time snapshot that must not be counted
> (see the note at its head).

> **RECOUNTED 2026-09-04 (area-01 audit pass, sixth edition) — counted set: 0 critical, 0 high, 2
> medium, 2 low = 4.** The table carries **60 rows: 56 fully closed, 4 open.** *(Previous edition:
> 0 / 1 / 3 / 2 = 6, 54 closed.)* This file's `4fb5e40` docs-baseline commit turned out to be its own
> last edit (`46a18e6`'s only parent is `4fb5e40`), so everything below was re-checked against the
> 210 commits landed on `crates/cyrup-core` and `crates/cyrup-provider` since, not against a stale
> intermediate state. **Two closures, both personally re-verified on both sides rather than taken
> from a commit message:**
> - **`PROV-068` CLOSED, REFUTED.** cyrup `24b6ffe` (2026-08-20) had already resolved it in the
>   direction the row's own hypothesis warned against — `null` really does mean unsupported — but per
>   this pass's rule that a commit message is a hypothesis, the citation trail was re-read
>   independently at the file's own ported baseline `v0.83.0`, not only at the commit's later tags:
>   `models.ts:668`, `openai-completions.ts:774`, `model-registry.test.ts:1012-1019`,
>   `max-thinking.test.ts:59-66`, `together-models.test.ts:24`. All five hold at `v0.83.0`. See the
>   row for the full trail.
> - **`PROV-035` fully CLOSED.** Its residual second render site (`collect_cache_misses` behind a
>   settable `showCacheMissNotices`) is live at HEAD with no commit naming the item: the setting
>   (`cyrup-config/src/settings/effective.rs`), the pending-flag plumbing
>   (`cyrup-tui/src/app/events.rs`), and the notice text itself
>   (`cyrup-tui/src/transcript/notices.rs::push_cache_miss_notice`) were each read and matched against
>   `interactive-mode.ts:3455-3476` @v0.83.0 field-for-field.
>
> **The remaining four were re-confirmed unchanged, each by re-running the item's own evidence at
> HEAD**, and are left exactly as filed: `PROV-014` (`providers/all.rs:38-40` and `:377` still list
> `qwen-token-plan`/`qwen-token-plan-cn`/`radius` as `NOT REGISTERED`), `PROV-042` (`grep -rn
> 'HostEvent::BeforeProviderHeaders' crates/` still finds only the reducer, the host-event match arm
> and one test — no production dispatch site), `PROV-040` (`rg 'fetch_deferred|cancel_deferred'
> crates/` is still 0 hits) and `PROV-070` (`xtask/src/main.rs:74` still says Together's hand-port is
> "20 rows"; it is 21 — the K3 addition's one-line residual is untouched). `PROV-070`'s own
> speculative question — what `PROV-068`'s resolution implies for the K2.x siblings' two-rung ladder —
> is answered by `PROV-068`'s closure and by `together.rs`'s updated in-source comment (`24b6ffe`):
> the siblings are not the bug, they are pi's own catalog data (`together-models.test.ts:24`), so K3's
> full-ladder `None` stays a deliberate cyrup-original choice, not a hedge.
>
> **Excluded from this pass, stated plainly:** pi has moved three patch tags past this file's recorded
> latest (`v0.84.1` → `v0.84.4`). `git -C tmp/pi diff --stat v0.84.1..v0.84.4 -- packages/ai/src` shows
> real surface growth touching this area's territory — a new `cloudflare-gateway-binding.ts` API, a
> widened `constrained-sampling.ts`, a new `ToolChoice`/`thinkingTokenBudgetField` family on
> `types.ts`, and three-figure-line rewrites of `mistral-conversations.ts` and
> `openai-completions.ts` — none of which this pass read closely enough to file as evidenced items.
> Filing against a diff-stat without reading both the new upstream logic and the matching cyrup call
> site would be exactly the citation-without-verification failure this file's own repair passes have
> spent several sweeps correcting elsewhere; better to publish the gap than to file weak rows. Left
> for a pass scoped to `v0.84.1..v0.84.4` drift specifically.

> **Routing note.** `PROV-054` … `PROV-059` all have the same fix site and it is **not a hand edit**: they close together through `PROV-018` / `PROV-060`'s regeneration. Scheduling them individually will produce six agents each hand-patching one catalog row and each invalidating `catalog_manifest.json`. Schedule `PROV-060` + `PROV-018` as one piece of work and close the six as its verification.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~PROV-M01~~ | ~~medium~~ **FILED AND CLOSED 2026-08-14** | parity-bug | S | Two hand-written `impl Provider` decorators dropped the trait-defaulted half of the surface pi's object spread carries — `github-copilot`'s credential filter was discarded in the overlay configuration — **FILED AND CLOSED 2026-08-14**: sweep 8. See the body below. |
| ~~PROV-030~~ | ~~high~~ **CLOSED 2026-08-14** | not-ported | L | `google-vertex` is registered with 10 models and no wire API — every request dies with `NoApiImpl` — **CLOSED 2026-08-14**: sweep 2 — the area's headline `high`. `api/google_vertex.rs` + `auth/google_adc.rs` ported end to end (express-mode vs ADC split, `resolveProject`/`resolveLocation` with pi's two verbatim throw strings, `{location}` interpolation, ADC search order, refresh-token exchange, RS256 JWT-bearer assertion, google-auth-library's 5-minute eager refresh); body/decoder delegate to `api::google_generative_ai` because pi's two `buildParams` are line-for-line identical at v0.83.0. **Sweep 1's stated blocker was false**: `ring` 0.17 was already resolved in Cargo.lock via rustls, so RS256 needed no new crate. `KNOWN_DANGLING = ["google-vertex"]` deleted from `every_catalog_row_names_a_registered_api` — the Verify clause is now enforced with no carve-out. |
| ~~PROV-027~~ | ~~high~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug | S | Copilot's Claude models send `x-api-key`; pi sends `Authorization: Bearer` — **REFUTED, CLOSED 2026-08-14**: sweep 2 — **REFUTED at HEAD**, and not in sweep 1's fixedIds, so it had been stale for at least one pass. `api/anthropic_messages.rs:434` carries the `model.provider === "github-copilot"` branch documented as "the branch Pi tests FIRST inside createClient", with a fixture at :2399. |
| ~~PROV-029~~ | ~~high~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug | S | Copilot + Codex login flows written but unreachable; flow registry has no production caller — **REFUTED, CLOSED 2026-08-14**: sweep 2 — **REFUTED at HEAD.** `providers/github_copilot.rs:157` wires `GitHubCopilotLogin` (the flow WITH `login`) with an explanatory block at :141-146; `providers/openai_codex.rs:137` wires `OpenAiCodexOAuthFlow`. Both dead-ends are gone. |
| ~~PROV-028~~ | ~~high~~ **CLOSED 2026-08-14 — REFUTED** | not-ported | S | `github-copilot-headers.ts` unported — no `X-Initiator`/`Openai-Intent`/`Copilot-Vision-Request` — **REFUTED, CLOSED 2026-08-14**: sweep 2 — **REFUTED at HEAD.** `crates/cyrup-provider/src/api/github_copilot_headers.rs` exists (223 lines) and is consumed by `anthropic_messages.rs`, `openai_responses.rs` and `openai_completions.rs` — the same three impls pi applies `buildCopilotDynamicHeaders` in. The item's `rg -i 'X-Initiator\|Copilot-Vision\|Openai-Intent'` evidence is stale. |
| ~~PROV-047~~ | ~~high~~ **CLOSED 2026-08-15** | parity-bug | M | `httpProxy` reaches only the streaming wire APIs — OAuth, the agent proxy transport and extension HTTP bypass it — **PARTIALLY CLOSED 2026-08-14**: sweep 2 — provider half landed: `sse::configure_http_proxy`/`configured_http_proxy` consulted inside `node_http_proxy::get_proxy_env` for `http_proxy`/`https_proxy` only (pi's `??=` ambient-wins precedence preserved), `sse::build_client_for(target_url)`, and all five OAuth flows converted off the proxy-blind `build_client()`. **One of the item's three cyrup-side claims is REFUTED**: `wire.rs:472` is not production code — the only `build_client()` in `wire.rs` is at :529 inside `mod tests` (the `#[cfg(test)]` boundary is at :235). **RESIDUAL — three lines in three other crates, and until the first lands the fix is inert in production**: (1) `configure_http_proxy(...)` beside the existing `configure_http_idle_timeout(timeout_ms)` in `cyrup-session-svc/src/builder.rs`; (2) `cyrup-agent/src/proxy.rs:468`; (3) `cyrup-ext/src/caps/http.rs:599`. One corner is not reproduced and is stated in-source: pi's `??=` leaves an ambient `HTTPS_PROXY=""` in place and `getProxyEnv`'s `\|\|` then skips it, so upstream that means "no proxy"; here empty and unset are indistinguishable. — **CLOSED 2026-08-15**: sweep 11 — **all three residuals were VERIFIED LANDED at HEAD**, so the row was stale, not the code: (1) `cyrup-session-svc/src/builder.rs:286` calls `cyrup_provider::configure_http_proxy(proxy.clone())`; (2) `cyrup-agent/src/proxy.rs:478` builds its transport with `cyrup_provider::build_client_for(&url)` and carries the PROV-047 rationale at `:468-477`; (3) `cyrup-ext/src/caps/http.rs:710-723` `client_builder()` ends `.no_proxy()` with the reasoning inline, and `client_through` adds its proxy AFTER that call so the ported resolver is the single authority. The only `build_client()` left anywhere in production is none — every remaining hit is under `#[cfg(test)]`. The `??=`-empty-string corner stays as the one documented, in-source non-reproduction. |
| ~~PROV-048~~ | ~~high~~ **CLOSED 2026-08-14** | parity-bug | S | A lone-surrogate `\uXXXX` escape in a provider SSE frame kills the whole turn — **CLOSED 2026-08-14**: sweep 1 — one predicate over the SSE JSON repair path; lone-surrogate escape no longer kills the turn. |
| ~~PROV-003~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | M | `ApiKeyAuth` has no `login`; `Models` has no `login`/`logout` (OAuth flow half now closed) — **CLOSED 2026-08-14**: sweep 1 — closed in FULL. The `ApiKeyAuth::login` half was already done at HEAD by c8c86bc (the item was stale on that point); anthropic api-key login and `Models::login`/`logout` are now in. The status line still saying "partially-closed" is superseded. |
| ~~PROV-011~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | L | `constrainedSampling` / grammar-constrained tools not modeled — **four** affected sites (six at HEAD) — **CLOSED 2026-08-14**: sweep 6 — the ends were ported by sweeps 2-5; the two frames between them (`cyrup-agent/src/agent.rs:818` hard-coding `constrained_sampling: None`, and `cyrup-ext/src/wrapper.rs`'s hand-written `impl Tool for RegisteredTool` omitting the method) dropped the value, so no tool could opt in at all. Both closed; pinned by `agent_loop.rs::prov011_a_tools_constrained_sampling_declaration_reaches_the_provider`. **Do not add opt-ins to cyrup-tools' built-ins — no pi built-in declares `constrainedSampling`** (three grep hits at v0.83.0, all in `types.ts:463` / `tool-definition-wrapper.ts:14,:42`). |
| PROV-014 | medium | parity-bug | M | radius + qwen-token-plan ×2 unregistered (pi-messages half closed) — **NOT STARTED 2026-08-15 (sweep 11), and the reason is a scheduling one, recorded so the next attempt does not re-derive it.** Half of the item is already done and this row did not say so: `env_api_keys.rs:53-54`,`:66` carries all three arms (`QWEN_TOKEN_PLAN_API_KEY`, `QWEN_TOKEN_PLAN_CN_API_KEY`, `RADIUS_API_KEY`) with the `env-api-keys.ts` citations. What remains splits into two pieces of very different size. **(a) qwen-token-plan ×2 — S, but it belongs to the CATALOG owner.** pi's providers are three-line `createProvider` calls (`providers/qwen-token-plan.ts` @v0.83.0: base URL `https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1`, `envApiKeyAuth("Qwen Token Plan API key")`, `openAICompletionsApi()`), so cyrup needs two `providers/fleet.rs` rows and two catalog JSON files sourced from `qwen-token-plan{,-cn}.models.ts`. **Hand-writing those two files is exactly the failure the Routing note above forbids**: they must come out of `xtask gen-catalogs` or `catalog_manifest.json` is invalidated on the next `--check`. Schedule with the generator, not against a provider agent. **(b) radius — M, and it is a NEW PROVIDER KIND, not a fleet row.** `providers/radius.ts` @v0.83.0 is hand-written, not `createProvider`: a `pi-messages` provider with a gateway option, `lazyOAuth`, and a `refreshModels` that reads a persisted store, imports a legacy pre-`ModelsStore` catalog when the stored entry is absent and the credential is OAuth, and refreshes dynamically — plus the whole of `providers/radius-config.ts` (`DEFAULT_RADIUS_GATEWAY`, `normalizeRadiusGatewayUrl`, `getRadiusModels`, `getRadiusModelsFromConfig`, `loadRadiusGatewayConfig`). The OAuth flow at `auth/oauth/radius.rs` is ported and ready; the provider around it is not. `builtin_oauth.rs:17`'s "radius … has no built-in provider in cyrup" is the accurate statement of that, and it is now the only radius claim in-tree that is not stale. — **PARTIALLY CLOSED 2026-09-04** (commit `1471a16f`; residual **low**): all three v0.83.0 built-ins — and v0.84.4's fourth, `qwen-token-plan-individual` — are now REGISTERED, resolve auth from their env vars, and stream against a faux origin. **qwen-token-plan ×3** are `providers/fleet.rs` members (`QWEN_TOKEN_PLAN`, `QWEN_TOKEN_PLAN_CN`, `QWEN_TOKEN_PLAN_INDIVIDUAL`; id/name/`baseUrl`/`envApiKeyAuth` label field-for-field against `ai/src/providers/qwen-token-plan{,-cn,-individual}.ts:6-15` @v0.84.4, registered `all.ts:118-120`) with `FleetCatalog::Dynamic` — a NEW two-variant enum on `FleetSpec.catalog` (`Embedded(&str)` | `Dynamic`) — and the provider-level `baseUrl` upstream's `createProvider` carries, now `FleetSpec.base_url` → `WireProvider::with_base_url`. **Why no catalog files, stated as evidence not convenience:** pi's rows for these are models.dev `alibaba-token-plan[-cn]` data (`ai/scripts/generate-models.ts:2303-2380` @v0.84.4) generated into gitignored `providers/data/*.json`; the providers were added at `bbb91fa8a` (v0.81.0~25) and `c03d78bdc`, both AFTER `b0c2a90e`, the last revision at which any `*.models.ts` was a data literal and the only revision `xtask gen-catalogs` can extract from — so the rows are in git at NO revision (`git -C tmp/pi log --all --diff-filter=A -- 'packages/ai/src/providers/data/*.json'` is empty), and both runtime sources (models.dev, pi.dev) are unreachable from this workspace (proxy 403). The previous row's own instruction — the files must come from the generator, never by hand — is therefore satisfied by shipping none: the runtime catalog arrives through the pi.dev overlay (`remote_catalog.rs` fetches `/api/models/providers/<id>` for every configured provider; `AuthStore::has_auth` counts the env key, so an exported `QWEN_TOKEN_PLAN_API_KEY` triggers it in rpc/interactive) and through `models.json`. `catalog_data.rs::DYNAMIC_ONLY_PROVIDERS` pins the four dynamic-only ids in BOTH directions so an accidentally empty catalog still fails. **radius** is a new provider kind, `crates/cyrup-provider/src/providers/radius.rs` (`RadiusProvider`, `RadiusProviderOptions`, `radius_provider_with`, `radius_auth`): a `pi-messages` provider over a `WireProvider` (every `Provider` method delegated by name, PROV-M01) with `envApiKeyAuth("Radius API key", [RADIUS_API_KEY])` + a gateway-bound `RadiusOAuth` (`radius.ts:30-33` @v0.84.4; `builtin_oauth.rs` gained the `"radius"` arm and its `:17` claim is gone), and the whole of `radius-config.ts` @v0.84.4 (byte-identical at v0.83.0): `DEFAULT_RADIUS_GATEWAY` (`:4`), `RadiusGatewayModel`/`RadiusGatewayConfig` (`:6-20`), `is_radius_gateway_model`/`sanitize_radius_gateway_config` (`:26-50`, rows FILTERED never fatal), `radius_credential_config`/`radius_models` (`:57-73`, read from `Credential::Oauth.ext["gatewayConfig"]`), `radius_models_from_config` (`:61-68`), `truncate_http_body` (`:75-78`), `load_radius_gateway_config` (`:80-96`, both error strings verbatim). `RadiusProvider::refresh_models` is `radius.ts:35-78` @v0.84.4 — legacy import, `allowNetwork`/abort gate, key via `resolve_provider_auth` (= `resolveRefreshCredential`, `models.ts:448-474`), fetch, publish to the attached `ModelsStore` — deduplicated with `RefreshDedup`; restore is the overlay loader's job (`RemoteCatalog::load_overlay` → `CatalogOverlay::apply`), the same split `remote_catalog.rs` documents, with ONE `[CYRUP-DELTA]`: the persisted entry stamps `last_modified = checked_at` so the overlay's pi #7016 staleness guard (vacuous for a provider with no embedded rows) does not discard it. Shell: `crates/cyrup/src/provider.rs::pi_dev_catalog_providers` excludes `radius` from the pi.dev fetch list, mirroring `model-runtime.ts:183-189` @v0.84.4 (`provider.id === "radius" ? provider : withRemoteCatalog(…)`). `env_api_keys.rs` gained the v0.84.4 `qwen-token-plan-individual` arm (`env-api-keys.ts:83`, same variable as `qwen-token-plan`). **Tests** (all red before — the ids/symbols did not exist, the registry asserted their ABSENCE, and the shell test counted two pi.dev requests where one is now issued): `fleet.rs::qwen_token_plan_members_match_upstream`, `fleet_has_nineteen_providers`; `all.rs::registry_contains_implemented_provider_ids` (three ids moved from the not-yet list to the expected list; `baseten` entered the not-yet list), `radius_registers_with_both_auth_strategies_and_takes_the_overlay`; `builtin_oauth.rs::only_the_five_built_ins_carry_oauth`, `subscription_split_matches_upstream`; `env_api_keys.rs::qwen_token_plan_and_radius_rows_match_upstream`; `radius.rs` ×16 (sanitize filter/reject, models-from-config stamping, legacy credential config, `truncate_http_body` 512-char cap, origin-resolved `/v1/config`, loopback `GET /v1/config` request shape with/without bearer, 503 and invalid-body messages verbatim, cancelled token opens no socket, static without a store, refresh publishes + restores through the real overlay loader with the built-in floor stamp, cache-only issues no request, legacy import, 500 persists nothing, a gateway row STREAMS over `POST /v1/messages` with `Authorization: Bearer` from `RADIUS_API_KEY`); `catalog_data.rs::every_registered_provider_has_a_non_empty_catalog` (two-directional exemption); `crates/cyrup/src/tests/catalog_refresh_modes.rs::update_models_never_fetches_radius_from_pi_dev`; `help.rs` env-block row for the Individual plan. `cargo nextest run -p cyrup-provider`: 1151 passed; `-p cyrup` green; clippy `-D warnings` and `RUSTDOCFLAGS='-D warnings' cargo doc` clean on both. **RESIDUAL (low), three pieces, none a registration gap:** (1) the three Qwen catalogs stay `Dynamic` until the models.dev/pi.dev data is reachable from a workspace — same constraint class as PROV-004's Coverage note; the git-derivable facts (ids, compat, thinking maps, `qwen-token-plan-models.test.ts` @v0.84.4) are recorded on each `fleet.rs` member for that day; (2) the shell never calls `Provider::refresh_models` — its refresh drives `RemoteCatalog` directly — so `RadiusProvider`'s gateway refresh runs only through `Models::refresh_with` on a store-attached instance (`all_providers()` constructs it static); wiring pi's `Models.refresh` trigger into `spawn_model_catalog_refresh`/`refresh_model_catalogs` with the `auth.json`-backed credential store is an S shell change; (3) `configureRadiusProviders` (`model-runtime.ts:219-233` @v0.84.4 — a `models.json` block with `"oauth": "radius"` becomes a `radiusProvider({ id, name, gateway })` instance) is not wired: `cyrup-config/src/model/compose.rs` composes the block's models but no `RadiusProvider` is constructed for it; `RadiusProviderOptions` is the ready-made shape. Separately noted, NOT this item: `baseten` (`all.ts:95` @v0.84.4) is the one v0.84.x built-in still unregistered — it is in `all.rs`'s not-yet guard with no ledger id. |
| ~~PROV-016~~ | ~~medium~~ **CLOSED 2026-08-14** | stale-port | S | Tool-argument coercion ignores `allOf` — **CLOSED 2026-08-14**: sweep 1 — confirmed at HEAD exactly as filed; citations resolved clean at v0.83.0. |
| ~~PROV-018~~ | ~~medium~~ **CLOSED 2026-08-15** | tooling | M | No catalog generator, no drift check — **CLOSED 2026-08-15**: sweep 10 — `xtask` exists and generates all 35 catalogs plus `catalog_manifest.json` from one pinned revision. **The Fix's stated mechanism was replaced and the reason is recorded**: it said to run pi's `npm run generate-models` "because the tree can no longer simply be read", which PROV-060 refutes — at `b0c2a90e` the `*.models.ts` modules are data literals, so `git show` + a scanner needs no `npm install`, no generator run and no network. Drift check: `gen-catalogs --check` (byte-exact) and `--diff` (structural), plus the `#[ignore]`d `gen_catalogs_check_reports_no_drift_against_the_pinned_revision`. |
| ~~PROV-019~~ | ~~medium~~ **CLOSED 2026-08-14** | stale-port | S | `max_output_tokens` floor of 16 unported in both Responses APIs — **CLOSED 2026-08-14**: sweep 1 — landed together on both openai routes plus azure and codex, sharing one `SessionAffinityFormat`. PROV-034 additionally required CORRECTING a test that pinned `strict == false` with no compat (a new test-defect instance). |
| ~~PROV-021~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `ANTHROPIC_AUTH_TOKEN` bearer env unsupported — **CLOSED 2026-08-14**: sweep 1 — `ANTHROPIC_AUTH_TOKEN` bearer env supported. |
| ~~PROV-024~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `sessionAffinityFormat` unported on openai-completions — **CLOSED 2026-08-14**: sweep 1 — landed together on both openai routes plus azure and codex, sharing one `SessionAffinityFormat`. PROV-034 additionally required CORRECTING a test that pinned `strict == false` with no compat (a new test-defect instance). |
| ~~PROV-032~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | `Provider::filterModels` unported — Copilot filter has zero production callers — **CLOSED 2026-08-14**: sweep 1 — landed together, as PROV-032's own Fix predicted. Two deltas documented in-tree: `check_auth` resolves against the provider's first catalog row (cyrup's `ApiKeyAuth::resolve` takes a `&Model`), and pi's optional `ApiKeyAuth.check?` (auth/types.ts:173) has no cyrup counterpart and no upstream implementor. |
| ~~PROV-033~~ | ~~medium~~ **CLOSED 2026-08-14** | stale-port | S | openai-responses carries pi's deleted `sendSessionIdHeader`; `x-session-id` unreachable — **CLOSED 2026-08-14**: sweep 1 — landed together on both openai routes plus azure and codex, sharing one `SessionAffinityFormat`. PROV-034 additionally required CORRECTING a test that pinned `strict == false` with no compat (a new test-defect instance). |
| ~~PROV-035~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | M | `cache-stats.ts` unported — no cache-waste accounting, no cache-miss notices — **PARTIALLY CLOSED 2026-08-14**: sweep 2 — `crates/cyrup-provider/src/cache_stats.rs` is a full port of `cache-stats.ts` (CACHE_TTL_MS, NOISE_FLOOR_TOKENS, `detect_miss`, `as_previous_request`, `scan`, `compute_cache_waste`, `collect_cache_misses`, `detect_cache_miss`), including the arithmetic that looks like a bug upstream: the noise floor is `<=` so exactly 1024 is NOT a miss, `reportedCache` is sticky, a promptless turn does not clear `prev`, and a model switch is deliberately not a reset. **Two forced shape changes, both documented in the module header**: the scan takes `&[CacheScanEntry]` (`SessionEntry` lives in cyrup-session, which DEPENDS on cyrup-provider), and misses are keyed by INDEX because pi keys its result map on the `AssistantMessage` OBJECT REFERENCE. **RESIDUAL:** both render sites — the `/session` `Cache Re-billed:` line at `cyrup-tui/src/app.rs:4192` *(pre-split)* under pi's `stats.cost > 0 \|\| cacheWaste.missedTokens > 0` guard, and `collect_cache_misses` behind a `showCacheMissNotices` setting that does not exist. — **FIRST RENDER SITE CLOSED 2026-08-15**: sweep 11 — `state::cache_scan_entries` is the adapter the module header prescribed, `AgentSession::cache_waste()` runs `compute_cache_waste` over the session's full model registry as pi's `modelRuntime` argument does (`interactive-mode.ts:5660`), and the TUI `/session` block emits the `Cache Re-billed` row under pi's exact guard with its `missedCost >= 0.0001` split and its singular/plural `1 miss`/`N misses` (`:5704-5711`). The app.rs offset in this row was stale, and `40821ed` has since deleted the file: the renderer is `C::SessionInfo` at `app/execute_session.rs:147-213`, with the guard at `:181`, the `breakdown.len() > 1` gate at `:184` and the two `Cache Re-billed` formats at `:204`/`:208`. **RESIDUAL NARROWED to the second site only** — `collect_cache_misses` re-injecting per-message notices at render time (pi `interactive-mode.ts:3354-3355`, `:3456`, `:4166` — re-verified at `v0.83.0` 2026-08-19; the anchor is spelled out because the preceding citation is now a cyrup `app/` path) behind a `showCacheMissNotices` setting. That is a new `cyrup-config` settings key plus a transcript-render path, ~M, and was NOT started. — **SECOND SITE CLOSED 2026-09-04**: personally verified at HEAD `2571969`, landed sometime between this file's `4fb5e40` baseline and now (no commit names PROV-035). The setting exists — `crates/cyrup-config/src/settings/effective.rs::show_cache_miss_notices`, `getShowCacheMissNotices` default `false`, matching `interactive-mode.ts` @v0.83.0 — and is threaded live into `cyrup-tui/src/app/state.rs::AppState.show_cache_miss_notices`, re-read on settings apply (`app/run_arms.rs`) and on the `/settings` toggle (`app/execute_misc.rs::"showCacheMissNotices"`). The notice fires from `crates/cyrup-tui/src/app/events.rs`: the assistant-message-end handler sets `cache_miss_check_pending` under pi's exact non-terminal guard (`!matches!(stop_reason, Aborted | Error)`, mirroring `interactive-mode.ts:3752`'s `else` branch), and the pending flag is settled by an async call to `AgentSession::last_cache_miss()` (`cyrup-session-svc/src/session/stats.rs:74-80`, which itself calls `cyrup_provider::cache_stats::collect_cache_misses`) before pushing `transcript.push_cache_miss_notice(&miss)`. `push_cache_miss_notice` (`cyrup-tui/src/transcript/notices.rs:161-182`) reproduces `addCacheMissNotice` field-for-field, re-verified against `interactive-mode.ts:3455-3476` @v0.83.0: the `missedTokens < 20_000 && missedCost < 0.1` skip threshold, the `~$X.XX` cost suffix gated at `>= 0.01`, and all three labels ("Cache miss", "Cache miss after model switch", `"Cache miss after {n}m idle"`) byte-for-byte. Both render sites are now live; nothing left open under this id. |
| PROV-042 | medium — **PARTIALLY CLOSED 2026-08-14** | not-ported | M | `transformHeaders` unported — `before_provider_headers` has no seam — **PARTIALLY CLOSED 2026-08-14**: sweep 1 — the Models-level seam is in (`StreamOptions.transform_headers`, applied at models.ts:480's position and stripped at :483's). **RESIDUAL:** the session-svc closure folding in `merge_provider_attribution_headers`, plus the `before_provider_headers` extension event and its WIT/event-catalog ABI bump (area 06). Nothing left inside cyrup-provider. — **RESIDUAL RE-MEASURED 2026-08-15 (sweep 11), and it is now ONE seam, not three.** Two of the three named residuals are done: the ABI half landed in full (`cyrup-ext/src/event.rs:59` `BeforeProviderHeaders = 31` with its name and `from_u8`, the `EventPatch::ProviderHeaders` reducer at `contract.rs:148-151`, `host/live.rs:2081-2082` and `:2303`, the WIT comment at `cyrup-ext{,-sdk}/wit/world.wit:326-327`, and the guest side at `cyrup-ext-sdk/src/api.rs:789-796` + `macros.rs:227-230`), and the attribution closure is installed — `AgentSession::into_shared` sets `agent.set_header_fn(...)` → `headers_for_model_ref` → `merge_provider_attribution_headers` (`session.rs:640-651`, `:3069-3084`, landed as AGENT-029). **What is genuinely left is the EMITTER and only the emitter**: `grep -rn 'HostEvent::BeforeProviderHeaders' crates/` finds the reducer, the enum, and one test constructing it — and no production site that emits it. So an extension can subscribe to `before_provider_headers` today and will never be called, which is worse than the documented refusal the item warned about. Also note the ordering half is still unreproduced for a second reason: cyrup's attribution rides the AGENT's `header_fn`, not `StreamOptions::transform_headers`, so pi's guarantee that the hook sees the attribution set already merged has no single point where both are present. Landing the emitter means moving the attribution closure onto `transform_headers` (the seam `collection.rs:556-560` already applies at pi's `models.ts:480` position) and dispatching the event from inside it. ~M, one crate. |
| ~~PROV-049~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `repair_json`'s invalid-`\u` arm doubles the backslash where pi emits `\u` unchanged — **CLOSED 2026-08-14**: sweep 1 — one predicate over the SSE JSON repair path; lone-surrogate escape no longer kills the turn. |
| ~~PROV-050~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `parse_partial` deletes astral characters written as surrogate pairs from recovered tool arguments — **CLOSED 2026-08-14**: sweep 1 — one predicate over the SSE JSON repair path; lone-surrogate escape no longer kills the turn. |
| ~~PROV-004~~ | ~~**tracker**~~ **CLOSED 2026-08-15** | tooling | M | The five newest catalogs were never field-diffed — **CLOSED 2026-08-15**: sweep 10 — its premise ("no longer checkable from this workspace") was false, and its coverage hole is gone: all 35 catalogs, including all five, are now generated from one revision and diffed field-by-field by `gen-catalogs`. |
| ~~PROV-015~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED** | not-ported | S | `ApiStreamOptions` has no `openai-completions` variant — **REFUTED, CLOSED 2026-08-14**: sweep 1 — **REFUTED, not fixed.** Its Impact is false at both tags: `OpenAICompletionsOptions`' only own members are `toolChoice`, `reasoningEffort` and (v0.84.1) `thinkingBudgets`, all three already on cyrup's `StreamOptions`. Reasoning recorded in-tree on the `ApiStreamOptions` enum. |
| ~~PROV-017~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | `Provider` trait exposes no `name`/`base_url`/`headers` — **CLOSED 2026-08-14**: sweep 1 — landed together, as PROV-032's own Fix predicted. Two deltas documented in-tree: `check_auth` resolves against the provider's first catalog row (cyrup's `ApiKeyAuth::resolve` takes a `&Model`), and pi's optional `ApiKeyAuth.check?` (auth/types.ts:173) has no cyrup counterpart and no upstream implementor. |
| ~~PROV-020~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `toolResult` JSONL key order diverges: `isError` emitted too early — **CLOSED 2026-08-14**: sweep 1 — `toolResult` JSONL key order corrected. |
| ~~PROV-023~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `prompt_cache_options` unported — one-shot requests implicitly cache-write — **CLOSED 2026-08-14**: sweep 1 — landed together on both openai routes plus azure and codex, sharing one `SessionAffinityFormat`. PROV-034 additionally required CORRECTING a test that pinned `strict == false` with no compat (a new test-defect instance). |
| ~~PROV-025~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED** | parity-bug | M | `deferredToolsMode: "kimi"` unported — **REFUTED, CLOSED 2026-08-15**: sweep 11 — **fully ported at HEAD and has been for at least one sweep; this row was stale, not the code.** The item's evidence (`rg --type rust 'deferred_tools_mode\|DeferredToolsMode' crates/` = 0 hits) now returns 12 production hits. `DeferredToolsMode::Kimi` is on `ModelCompat` (`api/compat.rs`), detected `None` and resolved with `.or(detected)` in `get_compat`, and BOTH halves of pi's rendering are in `api/openai_completions.rs`: `deferred_tool_names` is the separate `getDeferredToolNames` accessor (`openai-completions.ts:91-101`, insertion-ordered) filtering the top-level `tools` array (`:719-721`), and `convert_messages` pushes the two-key `{role:"system", tools:[…]}` message at upstream's exact position — after the image/`lastRole` handling, immediately before the `continue` (`:1266-1276`). Verified line-for-line against `git show v0.83.0:packages/ai/src/api/openai-completions.ts`. A body test already exists at `openai_completions.rs:2380` and states its own red-before condition. |
| ~~PROV-031~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | M | `Models` has no `get_available`/`check_auth`/`login`/`logout` — **CLOSED 2026-08-14**: sweep 1 — landed together, as PROV-032's own Fix predicted. Two deltas documented in-tree: `check_auth` resolves against the provider's first catalog row (cyrup's `ApiKeyAuth::resolve` takes a `&Model`), and pi's optional `ApiKeyAuth.check?` (auth/types.ts:173) has no cyrup counterpart and no upstream implementor. |
| ~~PROV-034~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | openai-responses always emits `"strict": false` — **CLOSED 2026-08-14**: sweep 1 — landed together on both openai routes plus azure and codex, sharing one `SessionAffinityFormat`. PROV-034 additionally required CORRECTING a test that pinned `strict == false` with no compat (a new test-defect instance). |
| ~~PROV-036~~ | ~~low~~ **CLOSED 2026-08-15** | not-ported | S | `getUsageCostBreakdown` unported — one cost total, no per-model breakdown — **CLOSED 2026-08-15**: sweep 11 — ported as `cyrup-session-svc/src/state.rs::usage_cost_breakdown` (1:1 with `core/usage-totals.ts:37-70` @v0.83.0: the `provider/responseModel ?? model` key, the literal `Tools/summaries` bucket for toolResult + compaction + branch-summary usage, the `cost > 0 \|\| tokens > 0` filter and the cost-DESCENDING sort), surfaced as `AgentSession::usage_cost_breakdown()`, and rendered in `cyrup-tui/src/app/execute_session.rs:184-192`'s `/session` block under pi's `usageBreakdown.length > 1` guard (`interactive-mode.ts:5699`). Sweep 8's routing note was right that this needs `cyrup-session-svc` + `cyrup-tui` together; both landed in one pass. Pinned by `prov036_breakdown_keys_attribute_sort_and_reconcile`, which asserts pi's own stated invariant — the rows sum to `SessionStats::cost` exactly. |
| ~~PROV-037~~ | ~~low~~ **CLOSED 2026-08-15** | not-ported | S | Two `auth-guidance.ts` formatters and the OAuth-expiry preflight branch unported — **CLOSED 2026-08-15**: sweep 11 — sweep 8's correction was right (ONE formatter missing, not two, and the fix site is `cyrup-session-svc`). `format_no_api_key_found_message` is in `auth_guidance.rs` **with the `UNKNOWN_PROVIDER` → "the selected model" carve-out** (`auth-guidance.ts:23`), alongside a factored `format_oauth_reauthenticate_message` for the string pi builds inline and identically at `agent-session.ts:1188-1192` and `:432-436`. The preflight at `session.rs` step 3 now reproduces `agent-session.ts:1182-1195` in full: no model ⇒ `NoModelSelected`; not configured ⇒ a live re-check before refusing; on refusal ⇒ the OAuth-expiry message when the stored credential is `oauth` (pi `isUsingOAuth`, `model-runtime.ts:368-370`), else `formatNoApiKeyFoundMessage`. Carried by a new `SessionServiceError::AuthPreflightRefused` whose `Display` is `{0}` so pi's text reaches the user unprefixed. **One `[CYRUP-DELTA]` recorded in-source**: the second chance refreshes the `AuthStore` snapshot and re-asks `has_configured_auth` rather than composing a whole `Models` per refusal — same observable difference (a credential written after startup now proceeds), and the ambient-auth gap it does not cover is identical before and after. |
| ~~PROV-038~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | Catalog roster guard is a tautology; 5 catalogs get no per-field checks — **CLOSED 2026-08-14**: sweep 1 — closed together. The `tests/` path in both items was stale: the file is `crates/cyrup-provider/src/tests/catalog_data.rs` (moved in 63d729a/c3982b5). Blind spot 1's "the roster test currently passes" is confirmed — it did, tautologically. |
| ~~PROV-039~~ | ~~low~~ **CLOSED 2026-08-14** | stale-port | S | `catalog_manifest.json` staleness floor predates the newest embedded data — **CLOSED 2026-08-14**: sweep 1 — closed together. The `tests/` path in both items was stale: the file is `crates/cyrup-provider/src/tests/catalog_data.rs` (moved in 63d729a/c3982b5). Blind spot 1's "the roster test currently passes" is confirmed — it did, tautologically. |
| PROV-040 | low | upstream-drift | M | `fetchDeferred`/`cancelDeferred` unported — **RE-CONFIRMED AT HEAD, NOT STARTED 2026-08-15 (sweep 11).** `rg 'fetch_deferred\|cancel_deferred' crates/` is still exactly 0 hits, so the item is accurate as filed; the data half remains fully ported. Size, measured rather than estimated: four seams (`ApiImpl` in `api/mod.rs`, `Provider` in `provider.rs`, `Models` in `collection.rs`, and `faux.rs` mirroring `providers/faux.ts:567-660`) plus `DeferredFetchOptions.wait` on the request options — the same shape and roughly the same reach as `PROV-S05`'s `RefreshModelsContext` threading, which took one focused pass. Note for whoever takes it: this is v0.84.1-only drift (`git show v0.83.0:packages/ai/src/types.ts \| grep fetchDeferred` is empty), so it lands as a documented forward-port with the same `[CYRUP-DELTA]` treatment `PROV-063` just received, or it waits for the v0.84.1 rebase. |
| ~~PROV-041~~ | ~~low~~ **CLOSED 2026-08-15** | stale-port | S | False in-tree provenance citations, incl. a wrong "1:1 port" claim — **PARTIALLY CLOSED 2026-08-14**: sweep 1 + 2 — down to ONE live instance: `crates/cyrup-ext-subagents/src/extension.rs` ("PROV-003 — cyrup ships no login flow at all"), plus the unbuilt CI citation lint. Instance (1) was corrected by sweep 1, instance (3) was already gone, and the fourth instance sweep 1 folded in (cyrup-core's stale `raw_stop_reason` comment) is fixed — see PROV-012 in the status table. — **CLOSED 2026-08-15**: sweep 11. Two corrections to the row itself: the `cyrup-ext-subagents/src/extension.rs` instance was ALREADY fixed at HEAD (`:12558-12566` now records the correction explicitly), and instance (3) was **not** gone — `providers/openai_codex.rs` still claimed the `openai-codex-responses` impl was "not registered today … cannot yet stream" while `api::register_builtins` registers it; corrected and pinned by `prov041_openai_codex_responses_is_registered`. **The item's Verify clause was then executed rather than deferred**: all 126 `*models.ts:NNN` citations in `cyrup-provider` were re-resolved against `git show v0.83.0:packages/ai/src/models.ts`, and **20 were wrong at the tag they name** — `provider.rs` ×4 (`refresh_models` cited `:63`, which is `ModelsApiStreamOptions`; `stream_simple` cited `:71`, a prose line; `baseUrl`/`headers` off by one), `collection.rs` ×12 (four section headers, `getAuth` ×3 at `:216` = `mergeHeaders`' closing brace, the refresh-dispatch header repeating the exact `:198-214` error this item was filed for, `getSupportedThinkingLevels` ×3 at `:670` = the `return true` beneath the branch), `catalog.rs` ×3 (the catch-and-skip contract cited the `refreshModels?` docblock), `wire.rs` ×2 (`applyAuth` at `:240-241`/`:252`, both inside other functions) and `simple_options.rs` ×1. All corrected with the construct named and `@v0.83.0` stamped. **RESIDUAL, stated plainly:** the CI citation lint is still not built; the sweep above was manual. Its natural home is now the `xtask` PROV-018 landed (a `git show`-based extractor already exists there), ~S. |
| ~~PROV-043~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | Bedrock has no request retry where pi inherits the AWS SDK's default — **CLOSED 2026-08-14**: sweep 1 — Bedrock request retry and `AWS_BEDROCK_FORCE_HTTP1` ported. |
| ~~PROV-044~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | `AWS_BEDROCK_FORCE_HTTP1` unported; client negotiates h2 with no override — **CLOSED 2026-08-14**: sweep 1 — Bedrock request retry and `AWS_BEDROCK_FORCE_HTTP1` ported. |
| ~~PROV-045~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | openai-responses `reasoning` branch drops the xAI `include` and the summary-only trigger — **CLOSED 2026-08-14**: sweep 1 — landed together on both openai routes plus azure and codex, sharing one `SessionAffinityFormat`. PROV-034 additionally required CORRECTING a test that pinned `strict == false` with no compat (a new test-defect instance). |
| ~~PROV-046~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Boolean tool-arg coercion accepts `"True"`/`" true "` where pi rejects the call — **CLOSED 2026-08-14**: sweep 1 — confirmed at HEAD exactly as filed; citations resolved clean at v0.83.0. |
| ~~PROV-051~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Codex header-phase timeout substituted with a whole-stream read timeout; pi's message and abort/timeout distinction lost — **CLOSED 2026-08-14**: sweep 1 — Codex header-phase timeout restored with pi's message and the abort/timeout distinction. |
| ~~PROV-S04~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | `estimateContextTokens`' message-anchored added-tool accounting unported — **CLOSED 2026-08-14**: sweep 1 — CLASSIFICATION CORRECTED: the item said the added-tool block was post-baseline (3d8f7435) and cyrup was "a faithful stale port". `git show v0.83.0:packages/ai/src/utils/estimate.ts` has it at :118-133, byte-identical to v0.84.1. It was a port omission. Citation-sweep miss of the same class as the nine already recorded. |
| ~~PROV-S05~~ | ~~low~~ **CLOSED 2026-08-15** | not-ported | M | `Models::refresh` has no `force`, no abort signal, no per-provider error map — **CLOSED 2026-08-15**: sweep 11 — `Models::refresh_with(provider, ModelsRefreshOptions)` returns `ModelsRefreshResult { aborted, errors }` and reproduces every clause of `models.ts:276-328` @v0.83.0 in upstream's order, including the two a shorter port drops: an ABORTED provider records **no** error (`:305`'s `if (!signal?.aborted)` guard — cancellation is not a provider failure), and any failure is followed by the `allowNetwork:false` cache-restore re-invocation whose own failure is swallowed (`:313-322`), built WITHOUT `force` exactly as upstream builds it. `Provider::refresh_models` now takes a `RefreshModelsContext` (pi `models.ts:34-44`), forwarded unchanged by both delegating decorators. **The abort was treated as guarantee-sensitive and is tested as two separate properties**, because a signal that is accepted and ignored is the failure mode this project keeps finding: `prov_s05_cancel_actually_aborts_an_in_flight_refresh` parks a provider inside its fetch, asserts the refresh has NOT settled, cancels, and requires it to return within 5 s (it can only do so via the token it was handed) with `aborted == true` and an EMPTY error map; `prov_s05_a_pre_cancelled_refresh_calls_no_provider` pins `:286`'s pre-check. `refresh(Option<&str>)` survives as the compatibility shape and its doc now says outright that `refresh(None) == Ok(())` for a wholly failed refresh is the hole, and that callers who care must use `refresh_with`. Two `[CYRUP-DELTA]`s recorded: `provider: Option<&str>` has no upstream counterpart, and `credential`/`store` are not threaded because the persisting fetcher owns both. The false "1:1 port" doc above the method (PROV-041 instance 1) was rewritten in the same edit. |
| PROV-053 | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | **FILED AND CLOSED IN THE SAME PASS (sweep 2).** `EnvAuthContext` diverged from pi's `defaultProviderAuthContext()` (ai/src/auth/context.ts:22-40 @v0.83.0) in two ways: `file_exists` did not expand a leading `~` (pi does, at :29-33), and `env` returned `Some("")` for a blank variable where pi returns undefined unless `value.trim().length > 0` (:24-25). The first made the Vertex ADC arm unreachable **on every machine** — `ctx.fileExists(VERTEX_ADC_PATH)` was always false — and was PROV-030's real hidden blocker. The second is the more general hazard: every precedence chain ported from a JS `??` or truthiness test reads `Some("")` as CONFIGURED, which INVERTS the upstream semantics; the concrete instance is `GOOGLE_CLOUD_API_KEY=""` winning the coalesce at providers/google_vertex.rs:301-304 and suppressing the ADC fallback. Both fixed in `auth/types.rs` with the pi citations inline and pinned by two tests. **Worth a sweep: any other `ctx.env(...)`-fed `??` chain in this crate may have been written around the old `Some("")` behaviour.** — **THAT SWEEP WAS RUN 2026-08-15 (sweep 11) AND CAME BACK CLEAN.** All 42 `ctx.env(...)` call sites in `cyrup-provider` were read against their upstream operator: every JS-`\|\|`/truthiness site filters empty explicitly (`auth/helpers.rs`, `env_api_keys.rs::get_provider_env_value`, `providers/anthropic.rs`, `utils/node_http_proxy.rs`, `auth/google_adc.rs::provider_env_value`), the `OverlayEnvContext` at `auth/resolve.rs:124-129` correctly falls THROUGH an empty overlay value (pi `env[name] \|\| base.env(name)`, `auth/resolve.ts:73-78`), and the two nullish-`??` chains — `providers/amazon_bedrock.rs` and `providers/google_vertex.rs` — deliberately let a stored `""` win the coalesce and then fail the surrounding truthiness test, with the reasoning written out above each. No second instance of the PROV-053 class exists in this crate. Recorded here so nobody re-derives it. |
| ~~PROV-054~~ | ~~**high**~~ **CLOSED 2026-08-15** | stale-port | S | `xai/grok-4.5` routed over the WRONG WIRE API on the xai DEFAULT model — **CLOSED 2026-08-15**: sweep 10 — closed BY THE REGENERATION, not by hand, exactly as this item's Fix required. `xai.json` now comes from `b0c2a90e` via `cargo run -p xtask -- gen-catalogs`: `api: openai-responses`, `compat {supportsLongCacheRetention:false}`, `thinkingLevelMap {off:null,minimal:null}`, and the five retired rows gone in the same write. Confirmed at the PORTED TAG too — `XAI_RESPONSES_MODEL_ID = "grok-4.5"` (`ai/scripts/generate-models.ts:378` @v0.83.0), selected at `:1408`, compat at `:390-392`, map at `:386-389`. |
| ~~PROV-055~~ | ~~**high**~~ **CLOSED 2026-08-15** | stale-port | S | `opencode` leaked a `session_id` header pi suppresses on every `openai-responses` row — **CLOSED 2026-08-15**: sweep 10 — closed by the regeneration; all **19** rows (16 + the GPT-5.6 trio PROV-057 added) now carry `sessionAffinityFormat: "openai-nosession"`. Confirmed at the ported tag: pi sets it on every `@ai-sdk/openai` OpenCode variant at `ai/scripts/generate-models.ts:1666` @v0.83.0. The **stopgap this item proposed was NOT taken**: `detect_session_affinity_format` is unchanged and still answers `Openai` for opencode — the fix is data, which is what upstream resolves it from. |
| ~~PROV-056~~ | ~~**high**~~ **CLOSED 2026-08-15** | stale-port | S | `kimi-coding` sent a non-adaptive thinking block plus a beta pi suppresses, on every model the provider has — **CLOSED 2026-08-15**: sweep 10 — closed by the regeneration; all **5** rows (3 + the two PROV-057 added) carry `forceAdaptiveThinking: true` and the two Kimi-For-Coding variants carry `allowEmptySignature: true`. Confirmed at the ported tag: `ai/scripts/generate-models.ts:1861-1864` @v0.83.0 builds every row that way. The zero pricing on the same three rows (PROV-059(a)) went with it. |
| ~~PROV-057~~ | ~~medium~~ **CLOSED 2026-08-15** | stale-port | M | 25 model ids that resolve in pi and errored here — **CLOSED 2026-08-15**: sweep 10 — the regeneration added exactly the 25 this item enumerated, no more and no fewer; the count and the per-catalog breakdown reproduced independently by `gen-catalogs --diff`. |
| ~~PROV-058~~ | ~~medium~~ **CLOSED 2026-08-15** | cyrup-original | M | 16 retired models cyrup still offered — **CLOSED 2026-08-15**: sweep 10 — the regeneration removed exactly the 16 enumerated; `xai` is back to 3 rows. The xai half is confirmed at the PORTED TAG rather than only at `b0c2a90e`: pi drops those five by name via `XAI_BUILTIN_EXCLUDED_MODEL_IDS` (`ai/scripts/generate-models.ts:379-385` @v0.83.0, applied `:2078`). |
| ~~PROV-059~~ | ~~medium~~ **CLOSED 2026-08-15 — 109 fixed, 3 REFUTED, 7 PRESERVED** | stale-port | M | 119 non-compat field differences — **CLOSED 2026-08-15**: sweep 10 — **109 of the 119 fixed by the regeneration** (`cost` 49, `maxTokens` 36, `contextWindow` 22, `api` 1, `thinkingLevelMap` 1), independently reproducing this item's own per-field and per-provider tallies. **3 REFUTED**: the `openai-codex` GPT-5.6 `contextWindow`s — claim (d) — are `272000` at BOTH v0.83.0 (`ai/scripts/generate-models.ts:2352`) and v0.84.1 (`:2541`), and v0.83.0's comment at `:2349` reads "formerly 372k"; `b0c2a90e`'s `372000` is the value pi REPLACED before the ported tag. **6 PRESERVED**: the GPT-5.6 luna/terra cost rows on `openai`/`azure`/`openai-codex` are a documented v0.84.1 forward-port (the 2026-07-30 price cut) already pinned by three tests; reverting them would bill users 5x. **1 PRESERVED**: groq `qwen/qwen3-32b` (PROV-064). All 10 are carried explicitly in the generator's `DELTAS` table with citations, not silently. **SWEEP-11 REVIEW (adversarial verification).** The regeneration was re-audited INDEPENDENTLY of `xtask` — pi's `*.models.ts` parsed with node and compared field-by-field against every embedded row — and the result confirms the accounting exactly: **1087 rows across 35 catalogs match `pi@b0c2a90e` byte-for-byte except these 10**, with zero missing and zero extra rows. Each of the three warrants was re-verified at the tag: `CODEX_GPT_56_CONTEXT = 272000` at v0.83.0 `:2352` AND v0.84.1 `:2541`; `OPENAI_GPT_56_STANDARD_COSTS` at v0.84.1 `:391-392` matches the pinned literals, and the azure clone really does drop `tiers` (`:2713-2723`); the groq override really did move to `qwen/qwen3.6-27b` (v0.84.1 `:870`). **ONE GAP FOUND, not fixed, and now recorded in `catalog_manifest.json` itself: the GPT-5.6 price-cut forward-port is INCOMPLETE.** At v0.84.1 upstream applies `OPENAI_GPT_56_STANDARD_COSTS` to FOUR provider families — `openai`, the derived azure clone, `openai-codex`, and **`cloudflare-ai-gateway`** ("Cloudflare AI Gateway passes OpenAI usage through at OpenAI list prices", `ai/scripts/generate-models.ts:2311-2315`) — but only the first three were ported. `cloudflare-ai-gateway`'s `gpt-5.6-luna`/`terra` rows still carry b0c2a90e's PRE-cut rates, so the same model is priced 5x (Luna) / 1.25x (Terra) higher on that route than on the other three. This was NOT widened unilaterally: completing or reverting the forward-port is an owner decision and must move all four families together. Also fixed: the manifest note claimed "one signed-off row divergence" while the table already carried ten — the count and the per-catalog list are now DERIVED from `DELTAS` so they cannot go stale again, which is the same failure mode PROV-060 exists to prevent. |
| ~~PROV-060~~ | ~~medium~~ **CLOSED 2026-08-15** | tooling | M | Provenance split across two revisions; the "not statically auditable" premise REFUTED — **CLOSED 2026-08-15**: sweep 10 — `xtask/` now holds a dependency-free `gen-catalogs` (a `git show` extractor + a 600-line scanner for pi's generated data-literal subset). One revision generates all 35 files **and** the manifest in one command; the manifest carries a per-provider source map, and its note records the irreducible 13-day residue to `v0.83.0`. `--check` is the byte-exact drift check (PROV-018's), `--diff` is the structural one. |
| ~~PROV-061~~ | ~~medium~~ **CLOSED 2026-08-14, SUPERSEDED 2026-08-15** | cyrup-original | S | `fireworks` `glm-5p2` / `glm-5p2-fast` carried two INVENTED compat flags present at neither provenance revision — `sendSessionAffinityHeaders: true` made cyrup emit three affinity headers pi never sends — **FILED AND CLOSED 2026-08-14** (sweep 9), then **SUPERSEDED 2026-08-15 by `DRIFT-052`**: the values are correct against pi `b9497c8c1` (v0.84.0+, closes #7676) and are restored as a cited forward-port in `xtask`'s `DELTAS` table. The provenance analysis stands; the outcome is reversed. See the body below. |
| ~~PROV-062~~ | ~~low~~ **FILED AND CLOSED 2026-08-14** | stale-port | S | `providers/all.rs`'s port-status table omits `all.ts:115-117` and its summary asserts the opposite of `PROV-014`; every line number in it is a `91585d9a` offset under a declared `v0.83.0` baseline — **FILED AND CLOSED 2026-08-14** (sweep 9). See the body below. |
| ~~PROV-063~~ | ~~low~~ **CLOSED 2026-08-15** | cyrup-original | S | `ModelCompat::supports_finish_reason` is a v0.84.1 flag with no v0.83.0 warrant — **CLOSED 2026-08-15**: sweep 11 — the cheap option this item named was taken: a `[CYRUP-DELTA]` tag on the field at `api/compat.rs` naming v0.84.1 as its warrant (`types.ts:548`, read at `openai-completions.ts:578`/`:584`/`:1499`/`:1551`) and recording why it is kept rather than deleted. Its INERTNESS is now a pinned property rather than an observation: `supports_finish_reason_is_a_v0841_forward_port_that_stays_inert` asserts `detect_compat(..) == true` across every provider shape AND walks `builtin_catalog()` asserting no row sets the key and no row resolves `false`, so the v0.84.1 inference branch stays unreachable. **Stated loudly: that test cannot go red before this change** — the item proposed no code change and the fix is the tag; it goes red the day somebody makes the delta live. |
| ~~PROV-064~~ | ~~low~~ **CLOSED 2026-08-15** | cyrup-original | S | `groq` `qwen/qwen3-32b`'s removed `thinkingLevelMap` carried no `[CYRUP-DELTA]` tag — **CLOSED 2026-08-15**: sweep 10 — tag added at `providers/fleet.rs` above `groq_qwen3_32b_no_longer_carries_the_retargeted_thinking_level_map`, naming the upstream symbol and both revisions. This item's warning was correct and acted on: the regeneration DOES re-introduce the map, so it is carried as an explicit `DELTAS` entry in `xtask/src/main.rs` that hard-errors if upstream stops setting the key. |
| ~~PROV-065~~ | ~~low~~ **CLOSED 2026-08-15** | cyrup-original | S | `openrouter-images.json` has no `*.models.ts` counterpart — **CLOSED 2026-08-15**: sweep 10 — the asymmetry is now encoded rather than noted: the generator's roster names `packages/ai/src/image-models.generated.ts` as this file's source and takes the `openrouter` sub-record, the manifest records that per-provider source path, and a test asserts it. Regenerating it from that file produced **zero** row or field differences, confirming this item's "verified exact". The other half of the asymmetry — `together.models.ts` has no catalog file because `providers/together.rs` hand-ports its rows — is recorded in the same roster. **COUNT CORRECTED 2026-08-19: it is 21 rows, not 20** — pi's 20 plus the `moonshotai/Kimi-K3` addition (`PROV-070`), which `xtask/src/main.rs:65` still describes as "20 rows". |
| ~~PROV-066~~ | ~~low~~ **CLOSED 2026-08-15** | not-ported | S | `open_router_routing` typed `serde_json::Value` where pi declares a structured `OpenRouterRouting` — **CLOSED 2026-08-15**: sweep 11 — ported as `api::compat::OpenRouterRouting` (1:1 with `types.ts:660-727` @v0.83.0) with `deny_unknown_fields` on it and on every nested object, plus the four union arms as untagged enums (`sort` string-or-spec, `max_price` number-or-string per field, and both percentile cutoffs). **One correction to this item:** upstream declares **13** fields, not 11 — `zdr` and `enforce_distillable_text` were missed by the count. Two details a naive port loses and this one does not: the field names are the WIRE names (snake_case), NOT the enclosing `ModelCompat`'s `rename_all = "camelCase"`; and `sort.partition` is `string \| null`, a three-state, so it is `Option<Option<String>>` behind a present-key-only deserializer — a plain `Option<String>` maps `null` to `None` and `skip_serializing_if` then DELETES the user's explicit null from the request. Three tests: a full-population round trip, the misspelled-key rejection at top level and one level down, and the explicit-null survival. **Key-order note recorded in-source**: cyrup's `serde_json` has no `preserve_order`, so the old `Value` path emitted alphabetically and this emits in pi's declaration order — neither matches pi's insertion order, and no catalog sets the key, so the only producer is a user's `models.json`. |
| ~~PROV-067~~ | ~~low~~ **CLOSED 2026-08-15** | cyrup-original | S | The wire-api registry is a fn-pointer factory table where pi's laziness is per-module dynamic `import()` — **CLOSED 2026-08-15**: sweep 11 — the sign-off this item asked for is now a `[CYRUP-DELTA, mechanism]` block in `api/mod.rs`'s header, naming `api/*.lazy.ts` and `lazyApi` (`api/lazy.ts:66-75` @v0.83.0, verified present at the tag), stating why the substitution is forced (Rust has no dynamic `import()`, so there is no module-load event to defer and no import cache to share — the only deferrable thing left is construction of the impl value) and why it is equivalent (nothing on either side depends on module-load timing). Pinned by `prov067_registry_constructs_nothing_until_the_first_get`, which asserts the id set equals pi's 10 `KnownApi` entries, that `builtin_registry()` constructs nothing, that `contains` stays free, and that the first `get` constructs exactly one impl. **Stated loudly: that test cannot go red before this change** — it goes red if a factory is replaced by an eager `register_impl`, or if the id set drifts. |
| ~~PROV-068~~ | ~~high~~ **CLOSED 2026-09-04 — REFUTED** | port-bug | S | An explicit `null` in `thinkingLevelMap` read as UNSUPPORTED, collapsing most reasoning models to two rungs — **REFUTED, CLOSED 2026-09-04**: landed in cyrup `24b6ffe` (between this file's `4fb5e40` baseline and HEAD `2571969`), and independently re-verified here against the ported tag rather than taken from the commit message. `null` really does mean unsupported; the "no provider-specific value" reading this row proposed is what ABSENCE already means, and the two are kept distinct on both ends. **Upstream, read at `v0.83.0` (the file's own ported baseline, not just the commit's later citations):** `if (mapped === null) return false;` (`packages/ai/src/models.ts:668`); the wire gate that proves absence and null are different cases, `else if (model.thinkingLevelMap?.off !== null)` (`packages/ai/src/api/openai-completions.ts:774`) — an absent `off` still emits the generic level name, a null `off` suppresses the parameter outright. Two upstream tests pin the exact three-way reading: `{off,minimal,low,medium: null, xhigh: "max"}` → `["high","xhigh"]` (`packages/coding-agent/test/model-registry.test.ts:1012-1019` @v0.83.0) and `{xhigh: null, max: "max"}` → `[off,minimal,low,medium,high,max]` (`packages/ai/test/max-thinking.test.ts:59-66` @v0.83.0). And the Kimi K2.6 row this item's own report was about is upstream's, verbatim: `thinkingLevelMap: { minimal: null, low: null, medium: null }` (`packages/ai/test/together-models.test.ts:24` @v0.83.0) — a two-rung ladder is pi's own catalog behaviour for that model, not a cyrup defect. **cyrup at HEAD**, `crates/cyrup-provider/src/collection.rs::get_supported_thinking_levels`, unchanged in logic (`Some(None) => false`, matching `mapped === null`) but now carries this citation trail in its doc comment. `crates/cyrup-provider/src/providers/together.rs`'s `moonshotai/Kimi-K3` row keeps `thinking_level_map: None` (full ladder) as a documented, deliberate choice for a model with no upstream row (`PROV-070`), not a hedge on this item's resolution — inverting the reading would have changed 159 catalog models and put `off` on models that cannot disable reasoning. |
| ~~PROV-069~~ | ~~critical~~ **CLOSED 2026-08-15** | port-bug | S | The model's `max_tokens` never reached the request, so the server applied its own ceiling — **CLOSED 2026-08-15** in `e677555`. **Root cause:** `GenConfig::max_tokens` has no production writer (`grep -rn '\.max_tokens(' crates/ | grep -v tests` is empty; the builder at `agent.rs:2294` is test-only), and the body emitted the key only when that option was `Some`, so it was never emitted at all. **The catalog's `max_tokens` — all 1087 rows, regenerated from pinned `b0c2a90e`, reconciled field-by-field, covered by tests — was decorative.** **MEASURED against the live provider, not inferred:** the same prompt to `moonshotai/Kimi-K3` with `max_tokens` omitted returns `finish_reason: length` at `completion_tokens: 2048` (**Together's default cap**), of which **1438 were reasoning tokens** — leaving ~610 for the visible answer; with `max_tokens: 131072` it returns `finish_reason: stop` at 3135 tokens. Fix sends the caller's ceiling when present and the model's otherwise, which is upstream's own rule at `anthropic-messages.ts:989` (`options?.maxTokens ?? model.maxTokens`) and the stated intent of `adjustMaxTokensForThinking`; recorded as a `CYRUP-DELTA` because `openai-completions.ts:716` guards on the caller value alone. **Test note:** every other wire test hand-supplies `max_tokens: Some(...)`, proving serialisation and hiding the only path that ships — 7112 tests passed while the product sent no ceiling. Verified RED by removing the fallback. **Two wrong diagnoses recorded so they are not retried:** the agent loop is a faithful port (`agent.rs:593` == `agent-loop.ts:196-200`) and was never implicated; and a port of pi's `isRecoverableLength` was drafted and REVERTED because it routes a truncated turn into `run_auto_compaction`, which at 3% of a 1M window trades truncation for compaction spam — it treats the symptom. **`PROV-068` compounds this** (reasoning pinned to `high` burns the ceiling) but is a separate defect and remains open. |
| PROV-070 | low | cyrup-original | S | **`moonshotai/Kimi-K3` is a Together roster row cyrup ships and pi does not — the first deliberate ADDITION to a ported provider catalog, and it had no row in this file.** Landed `2add245` (2026-08-15). The model is `providers/together.rs:278-288` (`cost(3.0, 15.0, 0.3)`, `1_000_000` context, `131_072` max_tokens, `together_compat(false, Some(ThinkingFormat::Together))`), under a 21-line provenance comment at `:257-277` recording that every value except `max_tokens` was MEASURED from `GET https://api.together.xyz/v1/models` on 2026-08-15 and that `131_072` was verified with a live request that returned `finish_reason: stop`. Upstream's `together.models.ts` @`b0c2a90e` stops at K2.7-Code — K3 shipped 2026-07-26, after the pinned revision — so this is a signed-off divergence, not drift. **It is GUARDED, which is why the severity is `low`**: sweep 10's roster test was renamed and widened to `full_catalog_ported_from_pi_plus_recorded_additions` (`:442`), asserting `models.len() == 20 + ADDITIONS.len()` (`:448`) against `const ADDITIONS: &[&str] = &["moonshotai/Kimi-K3"]` (`:447`), so an ACCIDENTAL extra row still fails while a NAMED one does not. **NOT a regeneration hazard — checked, against the obvious assumption from `PROV-018`/`PROV-060`:** `together` has no entry in the generator's `CATALOGS` at all (`xtask/src/main.rs:65-66`: "cyrup hand-ports Together's rows as Rust literals … so this generator cannot own them") and the manifest note restates the exception (`xtask/src/main.rs:588`), so nothing regenerates this roster and the `DELTAS` table is the wrong instrument for it. **The live hook is `PROV-068`, which is still open at `high`:** this row's `thinking_level_map` is `None` (`providers/together.rs:286`) where both Kimi siblings — K2.6 (`:247`) and K2.7-Code (`:290`) — pass `Some(m())`, and `m` (`:100`) is `level_map(&[("minimal", None), ("low", None), ("medium", None)])`, i.e. the three explicit nulls `get_supported_thinking_levels` (`collection.rs:787-810`) reads as UNSUPPORTED — `Some(None) => false` at `:800`. `None` was chosen deliberately so K3 keeps the full ladder while `PROV-068` is open, and the reasoning is in-source at `providers/together.rs:272-277`. **`PROV-068` must revisit this row either way it resolves** — if explicit-null comes to mean "supported, send no provider value" the asymmetry is pointless; if it keeps meaning "unsupported" then the siblings are the bug and K3 is the template. **RESIDUAL, one line:** `xtask/src/main.rs:65` still says cyrup "hand-ports Together's **20** rows"; it is 21 (re-verified 2026-09-04: now at `:74`, text unchanged). — **`PROV-068` RESOLVED 2026-09-04, REFUTED — this row's "either way" is settled, not pointless.** `PROV-068` closed on the "unsupported" reading, and the siblings are NOT the bug: `together-models.test.ts:24` @v0.83.0 shows the K2.6 map `{minimal: null, low: null, medium: null}` is pi's own catalog data, not a cyrup invention, so K2.6/K2.7-Code's two-rung ladder is a correct port. K3's `thinking_level_map: None` therefore stays exactly what `together.rs`'s comment (updated in the same commit, `24b6ffe`) now says: a deliberate cyrup-original choice for a model with no upstream row to copy, not a hedge against an open question. This item's own severity and scope are otherwise unchanged — it is still the one-line `xtask` residual above. |

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

## PROV-011 — `constrainedSampling` / grammar-constrained tools not modeled — four affected sites, not two — **CLOSED 2026-08-14**

**Kind** parity-bug · **Severity** medium · **Effort** L · **Confidence** confirmed

> **CLOSED 2026-08-14 (sweep 6)** — the type, the accessor and all six provider consumers were already landed by sweeps 2-5; sweep 6 closed the **two plumbing frames in the middle**, which is why five passes of provider-side re-verification kept reporting the item "clean". (1) `crates/cyrup-agent/src/agent.rs:818` hard-coded `constrained_sampling: None` when building the `ToolDef` from each `Tool`; it now reads `t.constrained_sampling().cloned()` (pi `tool-definition-wrapper.ts:14`, `:42`; `agent-loop.ts:301` `tools: context.tools`). (2) `crates/cyrup-ext/src/wrapper.rs`'s `impl Tool for RegisteredTool` hand-delegates eleven surface methods and omitted this one, so a WASM guest's declaration was read off the descriptor and discarded one frame later (pi's `wrapRegisteredTool` is `return { ...tool, execute }`, `core/extensions/wrapper.ts:21-22` — a spread that cannot drop a field). Extension-registered and WASM-guest tools are the **only** tools that can declare `constrainedSampling`, and every one of them reaches the loop through that wrapper, so both frames were required for any tool to opt in at all. Net effect of the gap: a `strict: "require"` declaration — which upstream **fails the request** over when the model cannot honor it — degraded silently to an ordinary unconstrained tool call, so the loudest arm of the feature was also the unreachable one. Landed sites: type `cyrup-core/src/constrained_sampling.rs` (re-exported `cyrup-provider/src/context.rs:44`), accessor `cyrup-core/src/tool.rs:156`, `WasmTool` forward `cyrup-ext/src/host/live.rs:1795`, resolvers `cyrup-provider/src/utils/constrained_sampling.rs`, consumers `anthropic_messages.rs:1250`, `openai_completions.rs:791`, `openai_responses.rs:955`, `google_generative_ai.rs:345`+`:894` (`resolve_google_function_calling_mode`), and two beyond the item's four — `bedrock_converse_stream.rs:1639` and `mistral_conversations.rs:469`. Pinned by `crates/cyrup-agent/src/tests/agent_loop.rs::prov011_a_tools_constrained_sampling_declaration_reaches_the_provider` (three tools — grammar config / explicit `false` / silent — asserting `tools.len() == 3` **before** asserting the silent tool's field is absent, so the negative arm cannot pass vacuously) and by `cyrup-ext/src/wrapper.rs::every_surface_method_delegates`, whose `Fixed` fixture now declares a **distinct non-default** value (the trait default is `None`, so a defaulted fixture compared `None` to `None` and stayed green with the delegation deleted).
>
> **CORRECTION, do not carry forward (sweep 6).** pi's built-in `Edit`/`Write`/`Read`/`Bash` do **not** declare `constrainedSampling` at any line: `git grep -n constrainedSampling v0.83.0 -- packages/coding-agent/src packages/agent/src` returns exactly three hits — the `ToolDefinition` field at `extensions/types.ts:463` and the two `tool-definition-wrapper.ts` copies at `:14` and `:42`. All four built-ins were opened directly; the previously circulated cites (`edit.ts:311`, `write.ts:200`, `read.ts:222`, `bash.ts:337`) are `execute` signatures and `parameters:` entries. The gap PROV-011 closed is the **plumbing** so an extension-registered or WASM-guest tool can opt in — adding opt-ins to `cyrup-tools`' built-ins would be a divergence **from** pi, not parity with it. `cyrup-core/src/tool.rs:152-155` and `constrained_sampling.rs:20-26` already state this correctly.

**cyrup** *(as filed; the grep below is STALE — the same pattern now returns roughly 60 hits)* — `rg --type rust 'constrained_sampling|supports_strict_tools|supports_openai_grammar_tools' crates/` returned **zero hits workspace-wide**. The scope recorded when this was filed (anthropic-messages + openai-completions) is understated; there are four consuming sites upstream and all four are unported. Two of them are actively wrong rather than merely absent: `api/openai_responses.rs:810-828` hard-codes `strict: false` on every tool (PROV-034), and `api/google_generative_ai.rs:321-332` maps `tool_choice` only, so it can never emit `VALIDATED`. Do not confuse this with `supports_strict_mode`, which *is* ported on the completions compat (`compat.rs:109`/`:225`, consumed at `openai_completions.rs:694`) — that flag is not the constrained-sampling resolver.

**upstream** — `pi/packages/ai/src/api/constrained-sampling.ts` @v0.83.0 is a dedicated module exporting `resolveJsonSchemaStrictSampling` (`:84`), `resolveGrammarConstrainedSampling` (`:101`) and `createGrammarToolInputProperties` (`:136`). Consumed at (1) `anthropic-messages.ts`, (2) `openai-completions.ts`, (3) `openai-responses.ts:262-272`,`:301-306` + `openai-responses-shared.ts:344-378` (`convertResponsesTools` with `supportsStrictMode`/`supportsOpenAIGrammarTools`), (4) `google-shared.ts:311-323` `resolveGoogleFunctionCallingMode`, which returns `FunctionCallingConfigMode.VALIDATED` when any tool resolves strict.

**Impact** — Models that can be grammar-constrained still emit free-form tool arguments, so the malformed-argument retries pi avoids by construction still occur. The Google leg additionally never asks Gemini to validate function calls against the declared schema, which is the one route where upstream gets a server-side guarantee rather than a hint.

**Fix** — Add `constrained_sampling` to `cyrup_core::Tool`; port `constrained-sampling.ts` as `cyrup-provider/src/utils/constrained_sampling.rs`; add `supports_strict_tools` / `supports_openai_grammar_tools` to `ModelCompat`/`ResolvedCompat`/`ResolvedResponsesCompat`; apply in `anthropic_messages.rs::convert_tools`, `openai_completions.rs::convert_tools`, the new `convert_responses_tools` options struct (PROV-034 creates the landing point), and a `resolve_google_function_calling_mode` in `google_generative_ai.rs:321-332`.

**Verify** — Per route: a tool declaring `constrainedSampling` on a strict-capable model serializes the merged strict schema byte-equal to pi's, and on a non-capable model the schema is unchanged; the Google route emits `functionCallingConfig.mode: "VALIDATED"` exactly when `resolveGoogleFunctionCallingMode` would.

## PROV-014 — radius + qwen-token-plan ×2 unregistered (a v0.83.0 port bug, not lag) — **PARTIALLY CLOSED 2026-09-04**

**Kind** parity-bug · **Severity** ~~medium~~ low (residual) · **Effort** M · **Confidence** confirmed (partially closed — registration, auth and streaming closed 2026-09-04; three low residuals listed below)

**cyrup** — `pi-messages` is done: `api/pi_messages.rs` exists and is registered at `api/mod.rs:151`. The three providers are not: `providers/all.rs:140-240` pushes no `radius`, `qwen-token-plan` or `qwen-token-plan-cn`; `env_api_keys.rs:34-73` `api_key_env_vars` has no arm for any of them; there are no catalog files. `providers/builtin_oauth.rs:17` states outright that radius has no built-in provider.

**upstream** — `pi/packages/ai/src/providers/all.ts` @**v0.83.0** already registers `qwenTokenPlanProvider()`, `qwenTokenPlanCnProvider()` and `radiusProvider()`; `pi/packages/ai/src/env-api-keys.ts` @v0.83.0 already maps `QWEN_TOKEN_PLAN_API_KEY`, `QWEN_TOKEN_PLAN_CN_API_KEY` and `RADIUS_API_KEY`. All three predate the ported baseline — this is a port omission, not expected lag, and the item's original `upstream-drift` classification was wrong.

**Impact** — Three providers are unreachable. A user with `RADIUS_API_KEY` or a Qwen token plan gets "not configured" in an environment where pi works.

**Fix** — qwen-token-plan ×2 are cheap: two `providers/fleet.rs` members, two catalogs, two `env_api_keys.rs` arms. radius additionally needs its OAuth flow wired — the flow module already exists at `auth/oauth/radius.rs`, so this is a `builtin_oauth.rs` arm plus a provider constructor, the same shape as PROV-029's fix.

**Verify** — Each provider resolves from its env var and streams against a faux origin; `api_key_env_vars` reports the new variables; the roster test (PROV-038, once it walks the directory) covers the new catalogs automatically.

**Note** — Duplicates PARITY-GAPS PB-1/PB-2, both re-confirmed at HEAD. Fix once, close both.

**CLOSURE 2026-09-04 (commit `1471a16f`), personally verified on both sides — the Rust at HEAD and the TypeScript at `v0.84.4` (ADR-0006 target; the ported baseline `v0.83.0` re-read where the two differ).**

*What the row asserted and what was true at HEAD before the change.* `providers/all.rs::builtin_providers_with` constructed none of the three; `providers/fleet.rs::FLEET` had 16 members; `builtin_oauth.rs:17` said radius "has no built-in provider in cyrup"; `all.rs`'s guard test asserted the three ids ABSENT. The `env_api_keys.rs` half was already in (`:53-54`, `:66`, as the 2026-08-15 row said) but lacked v0.84.4's `qwen-token-plan-individual` arm (`env-api-keys.ts:83`).

*Upstream, read at `v0.84.4`.* `packages/ai/src/providers/all.ts:118-121` registers `qwenTokenPlanProvider()`, `qwenTokenPlanCnProvider()`, `qwenTokenPlanIndividualProvider()`, `radiusProvider()` (v0.83.0 `all.ts:115-117` has the first, second and fourth; the Individual plan is `c03d78bdc`, #7659). `providers/qwen-token-plan.ts:6-15` / `qwen-token-plan-cn.ts:6-15` / `qwen-token-plan-individual.ts:6-15`: three-line `createProvider` calls — id, name, `baseUrl`, `envApiKeyAuth(<label>, [<var>])`, `openAICompletionsApi()`. `providers/radius.ts:20-82` and `providers/radius-config.ts:1-96` as itemised in the row. `env-api-keys.ts:81-83`, `:93`. `coding-agent/src/core/model-runtime.ts:183-189` (radius excluded from `withRemoteCatalog`) and `:219-233` (`configureRadiusProviders`). `ai/test/qwen-token-plan-models.test.ts:42-69` pins the id sets; `ai/scripts/generate-models.ts:290-336`, `:2303-2380` is where the rows are built from models.dev.

*cyrup at HEAD (`1471a16f`).* `crates/cyrup-provider/src/providers/fleet.rs` — `FleetCatalog` (`Embedded` | `Dynamic`), `FleetSpec.catalog` + `FleetSpec.base_url`, the `fleet_catalog!` helper, members `QWEN_TOKEN_PLAN` / `QWEN_TOKEN_PLAN_CN` / `QWEN_TOKEN_PLAN_INDIVIDUAL` placed after `openrouter` in upstream's order, `FleetSpec::is_dynamic`, `provider_with` applying `with_base_url`. `crates/cyrup-provider/src/providers/radius.rs` — new, as itemised in the row. `providers/builtin_oauth.rs::builtin_provider_oauth` — `"radius"` arm. `providers/all.rs::builtin_providers_with` — `radius_provider_with` pushed after the fleet; header table re-derived at `v0.84.4` (`all.ts:91-130`), "39 of 40 registered", `baseten` the one outstanding. `providers/mod.rs` / `lib.rs` — re-exports. `env_api_keys.rs::api_key_env_vars` — the Individual arm. `tests/catalog_data.rs::DYNAMIC_ONLY_PROVIDERS` (+ `tests/thinking_max.rs` consulting it). `crates/cyrup/src/provider.rs::pi_dev_catalog_providers`, applied in `spawn_model_catalog_refresh_with` and `refresh_model_catalogs_with`.

*Design decisions (DESIGN-GUIDANCE, recorded in the commit body too).* (a) `FleetCatalog` — a domain enum for "where do this member's rows come from", chosen over an empty-string `catalog_json` (indistinguishable from a broken `include_str!`) and over an `Option<&str>` (no place to say WHY it is absent). (b) `RadiusProvider` wraps a `WireProvider` and delegates every `Provider` method by name (the PROV-M01 rule) rather than re-implementing streaming; its refresh state is a `RefreshJob` snapshot so the deduplicated future is `'static`. (c) Publish/restore split (Functional-Core-style separation of the decision from the registry) instead of interior mutability behind `Provider::models() -> &[Model]` — the same choice `remote_catalog.rs` already made; rejected: `RwLock`/`ArcSwap` on the catalog (cannot hand out a borrowed slice), changing the trait (touches every provider and decorator). (d) `last_modified = checked_at` on the persisted radius entry — rejected: special-casing radius inside `remote_models()`'s staleness guard (the guard's intent is preserved without it). (e) A 15s per-request budget on the config fetch, matching `RemoteCatalog`'s documented rationale; the abort token is honoured independently.

*Verify (re-runnable).* `cargo nextest run -p cyrup-provider -E 'test(radius) | test(qwen_token_plan) | test(fleet_has_nineteen) | test(registry_contains_implemented) | test(only_the_five_built_ins) | test(every_registered_provider_has_a_non_empty_catalog)'` and `cargo nextest run -p cyrup -E 'test(update_models_never_fetches_radius_from_pi_dev) | test(the_env_help_block_and_the_read_set_are_the_same_set)'`; `git -C tmp/pi show v0.84.4:packages/ai/src/providers/radius.ts` / `radius-config.ts` / `all.ts` for the line citations above.

*Residual — low, three pieces (also in the row):* (1) Qwen embedded rows unobtainable from git; `Dynamic` until the data source is reachable. (2) The shell does not drive `Provider::refresh_models`, so the radius gateway refresh is library-reachable only (`Models::refresh_with` on a `with_models_store` instance). (3) `configureRadiusProviders` for `models.json` `"oauth": "radius"` blocks is not wired to `RadiusProvider::new(RadiusProviderOptions { id, name, gateway })`. PARITY-GAPS PB-1/PB-2 are the cross-cutting duplicates and are left to the ledger agent.

## PROV-016 — Tool-argument coercion ignores `allOf`; treats `anyOf`/`oneOf` as alternatives

**Kind** stale-port · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `cyrup/crates/cyrup-provider/src/validate.rs:104-108` is `schema.get("anyOf").or_else(|| schema.get("oneOf"))` — the two are mutually exclusive alternatives; `rg 'allOf|all_of' crates/cyrup-provider/src/validate.rs` returns nothing. The module was substantially rewritten since this was filed (see PROV-S01, closed) and these lines were not touched.

**upstream** — `pi/packages/ai/src/utils/validation.ts:14-16` @**v0.83.0** declares all three (`allOf?` `:14`, `anyOf?` `:15`, `oneOf?` `:16`); `:189-201` @v0.83.0 — inside `coerceWithJsonSchema` (`:186`) — runs a sequential `allOf` merge over each nested schema (`:189-193`), then an INDEPENDENT (non-`else`) `anyOf` pass (`:195-197`), then an independent `oneOf` pass (`:199-201`). **The code is byte-identical at v0.84.1 but the offsets are not:** `validation.ts:14-16` is unchanged, while the coercion block moves to `:196-208` (`coerceWithJsonSchema` at `:193`). Cite the v0.83.0 numbers — that is the tag this item is classified against.

**Impact** — Tool arguments whose schema uses `allOf` composition (common in generated and MCP schemas) are not coerced, so string-typed numbers and booleans reach the tool uncoerced and it errors on input pi would have accepted. Schemas carrying both `anyOf` and `oneOf` get only the first applied.

**Fix** — In `validate.rs:104-108`, replace the `or_else` with three sequential independent passes mirroring `validation.ts:189-201` @v0.83.0.

**Verify** — Unit tests beside the existing coercion suite: an `allOf`-composed object coerces every branch's properties; a schema carrying both `anyOf` and `oneOf` applies both.

## PROV-018 — No catalog generator and no drift check

**Kind** tooling · **Severity** medium · **Effort** M · **Confidence** confirmed (partially closed)

**cyrup** — Generator half absent: there is no `xtask` directory anywhere in the repo and no mechanical diff against a named pi revision. `crates/cyrup-provider/src/tests/catalog_data.rs` is a roster-count / non-empty guard with hand-picked spot values, and its roster assertion is itself defective (PROV-038). The provenance half landed but has since degraded — split out as PROV-039 so this item stays scoped to tooling.

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

> **⚠ CORRECTION 2026-08-14 (sweep 9) — this item's central premise is REFUTED, and the row is left
> in place unchanged otherwise.** The **upstream** paragraph above ("Not obtainable … it cannot be
> reproduced today at any tag") and `PARITY-GAPS.md:931` (`OQ-5`) both rest on the observation that
> every `*.models.ts` is a two-line re-export. **That is true only from `a9f6a3159` onward.** At its
> DIRECT PARENT `b0c2a90e` — the very revision `catalog_manifest.json` names as cyrup's provenance
> floor — the files are still full data literals, because `a9f6a3159` (`feat(ai): separate generated
> model data (#6765)`) is the commit that both added `.gitignore:11` and converted them.
> `git log --oneline b0c2a90e..a9f6a3159` returns exactly one commit. The whole catalog is therefore
> checkable with `git show b0c2a90e:packages/ai/src/providers/<p>.models.ts` plus a ~12-line node
> script — **no generator run, no `npm install`, no network.** Sweep 9 did it: 35/35 catalogs parsed,
> 1072 upstream models vs 1078 in cyrup, 1027 compared field-by-field. The results are filed as
> `PROV-054` … `PROV-059`, and the provenance mechanism as `PROV-060`. Nine sweeps inherited the
> "unverifiable" verdict from this paragraph; the data was one `git show` away the whole time.
> **This does not close `PROV-004`** — the ledgered coverage hole and its `PROV-018` owner both
> stand, and `PROV-018`'s drift check should be exactly the recipe in `PROV-060`.

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

**cyrup** — `rg -i 'compute_cache_waste|cache_waste|detect_cache_miss|collect_cache_misses|show_cache_miss' crates/` returns a single unrelated hit (`cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:1050`, an MCP cache test) — the module is entirely absent. ~~`crates/cyrup-tui/src/app.rs:4192-4218` is cyrup's whole `/session` renderer: a markdown table of file / id / message counts / token counts / cost, with no cache-waste line.~~ *(Re-pointed 2026-08-19: `40821ed` deleted `app.rs`; the renderer is now `app/execute_session.rs:147-213`, and per this row's first-render-site closure it DOES emit `Cache Re-billed` — `:204`/`:208`.)* No settings key corresponds to pi's `showCacheMissNotices` (`rg showCacheMissNotices` and `rg show_cache_miss_notices` are both empty).

**upstream** — `pi/packages/coding-agent/src/core/cache-stats.ts` @v0.83.0 — `CACHE_TTL_MS = 5*60*1000` (`:8`), `NOISE_FLOOR_TOKENS` (`:11`), `interface CacheMiss` (`:14`), `interface CacheWasteTotals` (`:25`), `interface ModelPriceSource` (`:33`), `computeCacheWaste(entries, models)` (`:138`), `collectCacheMisses(...)` (`:147`), `detectCacheMiss(...)` (`:158`). Both consumers are live in `modes/interactive/interactive-mode.ts` @v0.83.0: `:5660` `computeCacheWaste(entries, this.session.modelRuntime)` feeding the `Cache Re-billed: $X (N tokens, M misses)` line at `:5705-5711`, and `:3354-3355` `collectCacheMisses(...)` gated on `getShowCacheMissNotices()`, re-injecting per-message miss notices into the transcript at render time (also `:3456`, `:4166`).

**Impact** — Prompt-cache misses are the single largest avoidable cost in a long session, and cyrup gives the user no signal at all. pi prints `Cache Re-billed: $0.42 (128,000 tokens, 3 misses)` in `/session` and marks the individual assistant messages that paid for a re-billed prefix, so a user can see that a mid-session tool-set change is invalidating the cache every turn. In cyrup that money is spent silently and the only visible number is the total cost, which looks like normal usage. The primitives all exist — `Usage.cache_read`/`cache_write` are carried and `compute_cost` (`cyrup-provider/src/usage.rs:37-58`) already knows the cache-write premium — so this is arithmetic over the session entries plus two render sites, not a new subsystem.

**Fix** — Port `cache-stats.ts` as `crates/cyrup-provider/src/cache_stats.rs` (`CACHE_TTL_MS`, `NOISE_FLOOR_TOKENS`, `CacheMiss`, `CacheWasteTotals`, `detect_cache_miss`, `collect_cache_misses`, `compute_cache_waste`) taking a price source that `cyrup_provider::Model` already satisfies. Wire `compute_cache_waste` into what is now `crates/cyrup-tui/src/app/execute_session.rs:147` so `/session` gains pi's `Cache Re-billed` line under the same `stats.cost > 0 || cacheWaste.missedTokens > 0` guard, and wire `collect_cache_misses` into the transcript render path behind a `showCacheMissNotices` setting.

**Verify** — A fixture session whose second assistant turn has `cache_read: 0` after a first turn with a large `cache_write` yields `missCount == 1` and `missedTokens` equal to the re-billed prefix; `/session` prints the line only when `missedTokens > 0`, and formats `$X` only when `missedCost >= 0.0001` (pi's threshold at `interactive-mode.ts:5708`).

## PROV-M01 — Two hand-written `impl Provider` decorators dropped the trait-defaulted half of the surface pi's object spread carries — **FILED AND CLOSED 2026-08-14**

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed — **fixed and pinned in the same pass (sweep 8)**

**upstream** — `withRemoteCatalog` is an **object spread**: `return { ...provider, getModels: …, refreshModels: … }`
(`packages/coding-agent/src/core/remote-catalog-provider.ts:52-54` @v0.83.0). Every other member of the
`Provider` interface — `id`, `name`, `baseUrl?`, `headers?`, `auth`, `getModels`, `refreshModels?`,
`filterModels?` (`:105-110`), `stream`, `streamSimple` (`packages/ai/src/models.ts:76-119` @v0.83.0) —
**survives by construction**. There is no upstream counterpart to `ConfigProvider` at all:
`applyProviderConfig` folds the registration into the shared `ModelRegistry.models` array rather than
wrapping anything (`packages/coding-agent/src/core/model-registry.ts:917-940` @v0.83.0), so nothing
upstream can drop it.

**cyrup** — Rust has no spread. `impl Provider for RemoteCatalogProvider` named **6 of the trait's
11** methods, silently dropping `name`, `base_url`, `headers` and `filter_models`.
`impl Provider for ConfigProvider` named **4 of 11**, additionally dropping `refresh_models` and
`stream_simple`. **All the dropped members carry a trait DEFAULT, so the decorator returned a
plausible answer rather than failing.**

**Impact** — a live behaviour defect, not a latent one. `github-copilot` is the one built-in that
installs a `filter_models` (`filter_github_copilot_models` via `WireProvider::with_filter_models`,
`providers/github_copilot.rs:178`), and `all_providers_with_overlay` maps **every** built-in through
`CatalogOverlay::apply` (`providers/all.rs:148-157`). So in the overlay configuration
`Models::get_available` (`collection.rs:419`) called `filter_models` on the decorator, got the
identity default, and **offered the user all 29 Copilot models regardless of what the OAuth
credential's `availableModelIds` entitled**. Proven by running the new test against the pre-fix code:
it returned all 29 ids instead of the 1 entitled id. The `ConfigProvider` `name` drop is directly
observable and self-evidently wrong: `ConfigProvider::new(id, name, …)` **takes** a display name and
stores it on the inner `WireProvider` (which overrides `Provider::name`, `wire.rs:113-115`), and no
caller could ever read it back — `Provider::name()` fell through to the default `self.id().as_str()`,
so a guest registration declaring `"Acme Machines, Inc."` displayed as `acme` in every provider
picker and status line.

**Fix — LANDED.** `crates/cyrup-provider/src/remote_catalog.rs:299-352` (verified at HEAD: `name`,
`base_url`, `headers`, `filter_models` added, each carrying a `PROV-M01` doc block naming the
mechanism and the consequence) and `crates/cyrup-provider/src/config_provider.rs:86-152` (`name`,
`base_url`, `headers`, `filter_models`, `refresh_models`, `stream_simple`, plus the
`#[async_trait::async_trait]` attribute the async arm requires). **`get_model` is deliberately NOT
delegated on `RemoteCatalogProvider`, and the reason is recorded in-source**: its default derives from
`models()`, which this type overrides to the MERGED catalog — which is what upstream's `Models` sees
through the spread's `getModels`. Delegating it to `inner` would have been the bug.

**Verify — DONE.** Three tests, each verified load-bearing by deleting the delegation and watching it
fail. `remote_catalog.rs::the_decorator_forwards_every_surface_method_the_spread_carries` — a
`Decorated` fixture whose every defaulted method carries a **distinct non-default** value (name ≠ id,
`base_url` `Some`, `headers` `Some`, and a `filter_models` that really narrows), with
presence-before-absence assertions on the inner first so the fixture cannot silently lose a
declaration; it also pins that `get_model` still resolves against the merged catalog.
`remote_catalog.rs::overlaying_github_copilot_keeps_its_credential_filter` — the **production** path:
takes the real `github-copilot` built-in, asserts the bare provider narrows to 1 entitled id
(presence first), then wraps it through `CatalogOverlay::apply` and asserts the narrowing survives.
`config_provider.rs::the_registrations_display_name_survives_the_wrapper`.

> **The invariant this row establishes, and it is wider than this area.** The omission is invisible
> *precisely because* the trait default is a reasonable answer — `name`→id, `base_url`/`headers`→
> `None`, `filter_models`→identity, `render_kind`→`Default`, `constrained_sampling`→`None`,
> `read_stream`→a `Cursor` over the whole file, `detect_image_mime`→extension-based. Every one of
> those is indistinguishable from a working delegation **for any fixture that leaves the inner at ITS
> default**. The rule that holds the line is not "audit `Tool` impls": it is **every hand-written
> same-trait decorator, every defaulted method, a fixture value that CONTRADICTS the default, ideally
> in both directions.** See the register entry in `00-residual-ledger.md`.

## PROV-036 — `getUsageCostBreakdown` unported — `/session` shows one cost total

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `rg 'cost_breakdown|UsageCostBreakdown|get_usage_cost_breakdown' crates/` returns zero hits. ~~`crates/cyrup-tui/src/app.rs:4203`,`:4216`~~ *(re-pointed 2026-08-19: `app/execute_session.rs:164`)* render a single `| cost | ${:.3} |` row from `stats.cost`. The totals half of pi's `usage-totals.ts` IS ported — `add_usage_totals` at `crates/cyrup-tui/src/status.rs:168`, called from `app/events_fold.rs:96` and `status.rs:153` — so only the breakdown is missing.

**upstream** — `pi/packages/coding-agent/src/core/usage-totals.ts:30-36` `interface UsageCostBreakdownEntry` and `:37-62` `getUsageCostBreakdown(entries: SessionEntry[])`, keyed `${provider}/${responseModel ?? model}` — so an OpenRouter `auto` route is attributed to the concrete model it resolved to — with a bucket literally named **`Tools/summaries`** absorbing toolResult, branch-summary and compaction usage "so the breakdown reconciles with the session total". Rendered at `modes/interactive/interactive-mode.ts:5665`,`:5701-5705`, gated on `usageBreakdown.length > 1`.

**Impact** — In any session that switched models — `/model`, a compaction/summary model override, an OpenRouter `auto` route, or a subagent on a different model — the user cannot see which model spent the money. pi itemises `openrouter/anthropic/claude-sonnet-4.5: $0.31 (412k tokens)` under the total; cyrup shows only the sum. cyrup already carries `AssistantMessage.response_model`, so the data is present and unused.

**Fix** — Port the breakdown half next to the existing totals code: `usage_cost_breakdown(entries: &[SessionEntry]) -> Vec<UsageCostBreakdownEntry>` keyed on `provider/response_model.unwrap_or(model)`, with the `Tools/summaries` bucket reproduced by name and by membership (toolResult + branch summary + compaction, not merely "unattributed"). Render it in what is now `crates/cyrup-tui/src/app/execute_session.rs:147-213` under pi's `len() > 1` guard.

**Verify** — A fixture session with turns on two different models yields two entries whose `cost` sums to `stats.cost` exactly; a turn whose assistant message carries `responseModel` is attributed to the response model, not the requested one; compaction usage lands in `Tools/summaries`; a single-model session renders no breakdown at all.

> **FIX SITE CORRECTED 2026-08-14 (sweep 8) — this row is NOT schedulable against a provider-side or
> `cyrup-session`-side agent, and it stays open only because sweep 8 declined to land half of it.**
> Re-verified genuinely unported: `grep -rn 'usage_cost_breakdown|UsageCostBreakdown|cost_breakdown'
> crates` returns **zero**. All the *input* data exists in `crates/cyrup-session`:
> `KnownEntry::Message`/`Compaction`/`BranchSummary` are pi's exact three arms, `Compaction.usage` and
> `BranchSummary.usage` are both present (`entry.rs:105-107`, `:118-121`), and `AssistantMessage`
> carries `provider`/`model`/`response_model`/`usage` (`cyrup-core/src/message.rs:437-460`). The
> sibling `addUsageToTotals` half is already ported **twice** (`cyrup-session-svc/src/state.rs:145`,
> `cyrup-tui/src/status.rs:170`). **The blocker is the CONSUMER**: the reader is `SessionStats`, built
> by `SessionStats::from_entries` in `crates/cyrup-session-svc/src/state.rs` and rendered by the
> `/session` handler at `crates/cyrup-tui/src/app/execute_session.rs:147-213`. Landing the pure function in
> `cyrup-session` alone produces **a function with no reader** — the "declared surface with no
> consumer" failure these area files repeatedly name. **Route to one agent owning
> `cyrup-session` + `cyrup-session-svc` + `cyrup-tui`.** Port target `usage-totals.ts:31-69`
> @v0.83.0; render only when the breakdown has more than one entry
> (`interactive-mode.ts:5697-5702`).

## PROV-037 — Two `auth-guidance.ts` formatters unported; the preflight's message text and OAuth-expiry branch diverge

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

> **Scope corrected down from the auditor's medium.** The claim that cyrup has *no* submit-time
> auth preflight is **false** and is recorded as rejected in `## Coverage`:
> `crates/cyrup-session-svc/src/session.rs:1071-1090` `prepare_and_assemble` step 3 runs
> `has_configured_auth` and returns `SessionServiceError::NoConfiguredAuth` **before** assembly or
> any HTTP, citing `agent-session.ts:1062-1075`, and the model it checks is the active one
> (`compaction_model` is kept in sync at `session.rs:3880`). What remains is message text plus one
> branch.

> **COUNT AND LOCATION CORRECTED 2026-08-14 (sweep 8) — the formatter half is ONE function short,
> not two, and it does not live where this body says.** pi's `auth-guidance.ts` @v0.83.0 exports
> **four**: `getProviderLoginHelp`, `formatNoModelsAvailableMessage`, `formatNoModelSelectedMessage`,
> `formatNoApiKeyFoundMessage`. cyrup's port is `crates/cyrup-session-svc/src/auth_guidance.rs` and it
> carries the **first three** (`:15`, `:25`, `:33`, read at HEAD). `crates/cyrup/src/diagnostics.rs`
> still carries its own copies of the first two (`:210`, `:216`), which is why an earlier count said
> "two of four". **The only missing formatter is `formatNoApiKeyFoundMessage`** —
> `grep -rn 'No API key found' crates/` is still zero. **FIX SITE: `crates/cyrup-session-svc`,
> outside this area's crates.** The OAuth-expiry preflight half below is untouched and was **not**
> re-verified by sweep 8.

**cyrup** — ~~`crates/cyrup/src/diagnostics.rs:155-166` ports exactly two of pi's four functions~~ — `get_provider_login_help()` and `format_no_models_available_message()`. `formatNoModelSelectedMessage` and `formatNoApiKeyFoundMessage` have no counterpart: `rg 'No model selected|No API key found' crates/` returns nothing (the only near-match, `cyrup-config/src/login.rs:739`, is the unrelated "No API key providers available."). The OAuth-expiry branch is also absent: `rg 'Authentication failed for|re-authenticate' crates/` = 0 hits, and the preflight has no `checkAuth` second chance — it consults the cached `has_configured_auth` only.

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

**cyrup** — `crates/cyrup-provider/src/tests/catalog_data.rs:48-79` `const CATALOGS: &[(&str, &str)]` lists 30 `include_str!`ed catalogs, and `:85-86` asserts `CATALOGS.len() == 30, "catalog roster drifted from the file set"`. That compares the array against its own literal length, so it can never observe the drift its message claims to detect. The directory holds **35** files; absent from the array are `amazon-bedrock.json`, `github-copilot.json`, `google-vertex.json`, `openai-codex.json` and `openrouter-images.json`. The stated purpose at `:81-83` — "Production loaders swallow a parse error into `Vec::default()`, so without this a typo'd catalog ships as an empty provider" — is served for the *registered* providers by the sibling test, but the **per-model field assertions** (empty id, `context_window == 0`, empty `base_url`) run only over the 30-entry array, and `openrouter-images.json` is uncovered entirely because it is an images provider absent from `all_providers()`.

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

**cyrup** — The DATA half is fully ported: `crates/cyrup-core/src/message.rs:172-188` `StopReason::Deferred`, `:462-475` `deferred: Option<Box<DeferredHandle>>`, `:497-505` `struct DeferredHandle` in pi's field order, serialized at `:563-565`, round-tripped by `crates/cyrup-test-support/src/tests/deferred_interop.rs`. The BEHAVIOUR half is absent: `rg 'fetch_deferred|cancel_deferred' crates/` = 0 hits; neither `ApiImpl` (`api/mod.rs`) nor `Provider` (`provider.rs:17-52`) nor `Models` (`collection.rs`) declares them. cyrup's own doc at `message.rs:182-186` admits it.

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

## PROV-052 — **FIXED 2026-08-13** — The shipped binary's default model was the in-process faux TEST provider, so a bare `cyrup -p hi` failed with the internal string "No more faux responses queued"

**Kind** parity-bug · **Severity** **critical** (raised from `high` on the fix pass: the product did not work out of the box, and `cargo tree -p cyrup -e features --edges normal` proved the test double was compiled into the ordinary build of the shipped binary, not merely reachable) · **Effort** S · **Confidence** **confirmed — reproduced in the shipped binary; both sides read** · **observed 2026-08-13** (headless-binary; [`REPRO-LOG.md`](REPRO-LOG.md)) · **FIXED 2026-08-13**

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

---

### FIXED 2026-08-13 — and the Fix above was **wrong about pi's mechanism**

**The item's own `Fix` and `Verify` text asserted that the correct default is `google`** ("change
`provider.rs:356` so `None` falls through to the registry lookup for `google` (pi's documented
default)", "must print a credential/`/login` message naming `google`"). **That is not what pi does,
and it was not implemented.** Read at the tag:

* `pi/packages/coding-agent/src/cli/args.ts:87-88` @v0.83.0 — `else if (arg === "--provider" && i + 1 < args.length) { result.provider = args[++i]; }`. There is **no** `?? "google"` anywhere in the parser; `ParsedArgs.provider` (`:13`) is `string | undefined` and stays `undefined`.
* The string `--provider <name>    Provider name (default: google)` at `args.ts:239` is a **stale help line in pi itself** — it documents a default pi's own code does not apply. cyrup ports it verbatim at `crates/cyrup/src/cli.rs:871`, which is correct parity and was deliberately left alone.
* What pi actually does with no `--provider`/`--model` and no credential: `ModelRuntime.getAvailable()` is empty ⇒ `findInitialModel` falls through steps 1-4 and returns `{ model: undefined }` (`core/model-resolver.ts:648-650`) ⇒ `createAgentSession` sets `modelFallbackMessage = formatNoModelsAvailableMessage()` (`core/sdk.ts:216-218`) ⇒ `main.ts:852-855`:

```ts
if (appMode !== "interactive" && !session.model) {
    console.error(chalk.red(formatNoModelsAvailableMessage()));
    process.exit(1);
}
```

  with `formatNoModelsAvailableMessage()` = `` `No models available. ${getProviderLoginHelp()}` `` (`core/auth-guidance.ts:6-16`). Provider-agnostic, actionable, `/login`.

Had the item's `Fix` been implemented as written, cyrup would have emitted a **google-specific**
credential error where pi emits a provider-agnostic `/login` message — a second parity bug in place
of the first. Recorded here because the ledger's ~20%-citation-error rule applies to *Fix* text too,
not only to line citations.

**pi has no faux fallback of any kind, and none is reachable from its CLI.**
`packages/ai/src/providers/faux.ts` is exported from the `pi-ai` package for tests only
(`packages/ai/src/index.ts:36`); it is **absent from `packages/ai/src/providers/all.ts`**, it is not
a member of `KnownProvider`, and `git grep faux v0.83.0 -- packages/coding-agent/src/` matches
**zero files**. So neither the `None` arm nor the `Some("faux")` arm of cyrup's `select_provider`
had an upstream referent.

#### What changed

Two separable defects, both closed.

**1. The feature graph — the test double is out of the normal build.**

`crates/cyrup-provider/Cargo.toml:14-17`'s comment claimed `faux` was "gated for downstream
consumers". It was not, and naming cannot gate it: **Cargo features are additive and unified per
package across everything built in one invocation**, so one `features = ["faux"]` edge in any
`[dependencies]` section turns the feature on for *every* consumer of `cyrup-provider` in that
build. Two enabling edges were found:

* `crates/cyrup/Cargo.toml:41-43` — `cyrup-provider = { workspace = true, features = ["faux"] }` in `[dependencies]` of the **binary**. This is the one `--edges normal` reported. **Moved to `[dev-dependencies]`.**
* `crates/cyrup-test-support/Cargo.toml:17` — a `[dependencies]` edge in a crate that was in the workspace `default-members`. It does not show under `cargo tree -p cyrup` (test-support is only ever a dev-dependency of others), but a plain `cargo build` at the workspace root builds it alongside the binary and unifies the feature in anyway. **`crates/cyrup-test-support` removed from `default-members`** (it stays a `members` entry, so `cargo test`/`--workspace` still build it). Its own `[dependencies]` edge is left in place: its `src/` *is* the scripted harness.

The other seven `features = ["faux"]` edges (`cyrup-tui:93`, `cyrup-ext:54`, `cyrup-session:35`,
`cyrup-agent:26`, `cyrup-modes:27`, `cyrup-sdk:28`, `cyrup-session-svc:59`) were verified to already
be in `[dev-dependencies]` and were left untouched. The `[features]` comment in
`crates/cyrup-provider/Cargo.toml` now states the real rule and the invariant.

Five integration tests spawn `CARGO_BIN_EXE_cyrup` and script a whole offline turn through
`--model faux/faux-1` (`one_shot_parity.rs`, `piped_stdin_trim.rs`, `unknown_flag_exit.rs`,
`extension_load_failure_exit.rs`, `auth_credential_print.rs`) — moving the edge alone would have
silently killed them, and deleting them would have traded one defect for a coverage hole. They are
kept by a **default-off, test-only `faux` feature on the `cyrup` package itself**
(`faux = ["cyrup-provider/faux"]`), enabled solely by a **self-dev-dependency**
(`cyrup = { path = ".", features = ["faux"] }` in `[dev-dependencies]`). `cargo test` resolves
dev-dependencies and therefore compiles the `#[cfg(feature = "faux")] Some("faux")` arm into the
binary it spawns; `cargo build`, `cargo build --release` and `cargo install` do not resolve them and
therefore do not. Both requirements hold simultaneously, and neither test nor product is weakened.

Evidence, both directions:

```
$ cargo tree -p cyrup -e features --edges normal | grep faux      # BEFORE
├── cyrup-provider feature "faux"
│   └── cyrup-provider v0.0.0 (…/crates/cyrup-provider) (*)

$ cargo tree -p cyrup -e features --edges normal | grep -c faux   # AFTER
0

$ cargo tree -p cyrup -e features | grep faux                     # AFTER, dev edges included
├── cyrup-provider feature "faux"
```

**2. Default model resolution — pi's actual mechanism, ported.**

pi's "no model" state is `model: undefined`. cyrup's `SessionBuilder` takes a non-optional
`Arc<dyn Provider>`, so the state is represented by a provider with an **empty catalog**:
`crates/cyrup-provider/src/unconfigured.rs` (**new**, always compiled, never feature-gated).
`crates/cyrup/src/provider.rs`'s `select_provider` now reads
`None => Ok(Arc::new(UnconfiguredProvider::new()))`, and the `Some("faux")` arm is compiled out of
every normal build (`#[cfg(feature = "faux")]`, reached only by the self-dev-dependency above). In
the shipped binary an explicit `--provider faux` or a `faux/…` prefix is the ordinary
unknown-provider error, matching pi, where `faux` is not in `providers/all.ts`. Verified on the
plainly-built binary:

```
$ cyrup --model faux/faux-1 -p hi
cyrup: model targets provider 'faux', which is not a known provider. Available providers:
amazon-bedrock, ant-ling, anthropic, … zai-coding-cn. (Declare a custom one under "providers" in
<agent-dir>/models.json; there is intentionally no silent fallback.)
```

**No new error path was written**, because pi's was already ported and simply unreachable: an empty
catalog raises `SessionServiceError::NoModels` at `crates/cyrup-session-svc/src/builder.rs:1453-1455`,
which `crates/cyrup/src/main.rs`'s `no_models_available()` (`:1899-1902`, already citing
`main.ts:795-798`) renders as `cyrup::format_no_models_available_message()`
(`crates/cyrup/src/diagnostics.rs:157-165`, already a 1:1 port of `auth-guidance.ts:6-16`) on
**stderr** with **exit 1**, from both non-interactive arms (`main.rs:659` rpc, `main.rs:767`
print/json). The faux fallback was the only thing standing between the user and that message.

`CYRUP-DELTA`: pi holds `model?: Model` and tolerates absence; cyrup holds a provider with zero
models. Documented at the top of `unconfigured.rs` with pi's file:line. Observably identical —
same text, same stream, same exit code, same mode gate.

#### Reproduction, before and after

Identical fixture both times: scrubbed `env -i`, scratch `HOME` and agent dir, **no provider keys**,
**no `--model`**, **no `--provider`**.

```
$ cd <scratch> && env -i PATH=/usr/bin:/bin HOME=$TD CYRUP_AGENT_DIR=$TD/agent cyrup -p hi
```

BEFORE (`target/debug/cyrup` @ 85bc8bd):

```
No more faux responses queued
EXIT=1
```

AFTER:

```
No models available. Use /login to log into a provider via OAuth or API key. See:
  docs/providers.md
  docs/models.md
EXIT=1
```

byte-identical to `formatNoModelsAvailableMessage()`, on **stderr** (`2>/dev/null` yields empty
stdout), exit 1 — pi `main.ts:852-855`.

And the product now works out of the box once a credential exists — same fixture plus one env key,
proving the `default_launch_model` upgrade path (pi `findInitialModel` step 4) is intact and that
the run reaches the real vendor:

```
$ env -i … ANTHROPIC_API_KEY=sk-ant-bogus cyrup -p hi
http 401: {"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"},…}
```

#### Tests

* **`crates/cyrup/src/provider.rs` → `no_flags_resolve_to_an_empty_catalog_never_to_a_test_double`** — RED before (`select_provider(None, None, …)` returned `FauxProvider`, `id() == "faux"`), GREEN after. Asserts the no-flag provider has id `unconfigured` and an **empty** catalog, and that `faux/faux-1` / `--provider faux` are unknown-provider errors.
* It **replaces `defaults_and_faux_resolve_to_faux`**, which was a **test-defect** by the ledger's rule: it pinned behaviour pi does not have. Proof cited in the test's own doc comment — `faux.ts` absent from `providers/all.ts`, not a `KnownProvider`, zero matches under `packages/coding-agent/src/` at v0.83.0.
* **`crates/cyrup-provider/tests/faux_not_in_normal_build.rs`** (new) — the invariant is a **Cargo feature-graph** property, which no `#[cfg]`-based Rust test can express (the resolver decides `feature = "faux"` before any Rust is parsed, and the resolver is what regressed), so the guard runs `cargo tree -p cyrup -e features --edges normal` and fails on any `feature "faux"` line, with the offending lines and the fix in the assertion message. **Demonstrated RED-then-GREEN mechanically on this pass**: re-adding `features = ["faux"]` to `crates/cyrup/Cargo.toml`'s `[dependencies]` produced `test result: FAILED. 1 passed; 1 failed`, printing `├── cyrup-provider feature "faux"`; reverting produced `test result: ok. 2 passed; 0 failed`. Its companion asserts the feature is **still** reachable on a dev edge, so the guard cannot be satisfied by deleting the double and stranding nine crates' offline oracle.
* **`crates/cyrup-provider/src/unconfigured.rs`** — three new tests: the catalog is empty; the message is byte-identical to `formatNoModelsAvailableMessage()`; the (session-unreachable) direct `stream()` yields an actionable `error` terminal rather than a scripted answer.
* Stale `faux` prose corrected in the same pass at `crates/cyrup/src/{provider.rs,main.rs,lib.rs}` — including `provider.rs`'s "a non-provider prefix maps to faux (ledgered) — no warn", which this item's Fix text specifically called out.


## Findings filed 2026-08-14 (sweep 9 — the mechanical provider / wire-api / compat-flag surface enumeration)

Filed by the surface sweep that enumerated **providers, wire APIs and per-provider compat flags** on
both sides by command rather than by eye — pi `packages/ai/src/providers/all.ts` (registration list),
`packages/ai/src/types.ts` (the four compat interfaces), `packages/ai/src/api/*` (wire-api ids) and
all 35 `*.models.ts` catalogs, against `crates/cyrup-provider/src/{providers,api}`. **88 upstream
entries vs 86 in cyrup; 1027 models compared field-by-field.**

> **⚠ PROVENANCE OF THIS SECTION — state it before acting on any row below.** The catalog half of
> this enumeration is **INCOMPLETE by construction, and the incompleteness has a specific shape.**
> pi gitignores `packages/ai/src/providers/data/` (`.gitignore:11`) at the ported tag `v0.83.0`, so
> **the catalogs below were read at `b0c2a90e`, a revision 13 days EARLIER than `v0.83.0`** — the
> last revision at which the `*.models.ts` files are still full data literals rather than two-line
> re-exports. Every catalog claim in `PROV-054` … `PROV-061` is therefore measured against
> `b0c2a90e`, **not** against the ported baseline, and a clean refresh to `b0c2a90e` still leaves an
> unmeasured 13-day residue. The compat-interface, wire-api-id and registration halves of the sweep
> were read at `v0.83.0` directly and carry no such caveat. See `PROV-060`.

> **Nothing in this section was closed by hand-patching catalog data, with one deliberate exception.**
> `PROV-061` is invention — wrong at *both* provenance revisions — so removing it moves the data
> TOWARD every upstream revision at once and cannot be undone by a future regeneration. Everything
> else in `PROV-054` … `PROV-059` is provenance lag, and hand-patching those would fix a handful of
> rows while destroying the one property the catalogs still have: a single stated provenance
> revision. Those close correctly only through `PROV-018`'s bulk regeneration, in the commit that
> rewrites `catalog_manifest.json`.

## PROV-054 — `xai/grok-4.5` is routed over the WRONG WIRE API (`openai-completions` where pi uses `openai-responses`), and it is the xai default model

**Kind** stale-port · **Severity** **high** · **Effort** S · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**upstream** — pi `packages/ai/src/providers/xai.models.ts:25-42` @`b0c2a90e` — `"grok-4.5": { api: "openai-responses", compat: {"supportsLongCacheRetention":false}, thinkingLevelMap: {"off":null,"minimal":null} }`. Contrast `:10` and `:47` where `grok-4.3` and `grok-build-0.1` are `api: "openai-completions"`. At `91585d9a` grok-4.5 WAS `openai-completions` with compat `{supportsStore/supportsDeveloperRole/supportsReasoningEffort:false}` — **pi moved it to the Responses API before the ported tag.**

**cyrup** — `crates/cyrup-provider/src/providers/catalog/xai.json` still carries the pre-move row verbatim: `"api": "openai-completions"`, the three old compat flags, no `thinkingLevelMap`, and no `supportsLongCacheRetention: false`.

**Impact** — The highest-severity item on this surface, and **not a flag — the protocol.** cyrup builds a Chat-Completions body and POSTs it to the Completions path for xAI's flagship model, where pi builds a Responses body (`input[]` not `messages[]`, a `reasoning` object, a different SSE event grammar). Every `grok-4.5` request diverges wholesale. Compounding: `thinkingLevelMap {off:null, minimal:null}` is absent so `off`/`minimal` are not suppressed, and `supportsLongCacheRetention:false` is absent so the resolver defaults it **true** (`detect_compat` gives `true` for xai) and will offer long cache retention xAI rejects. **This is on the default path** — `CFG-045` (`05-cyrup-config-and-resources.md:423`) makes `grok-4.5` the xai default model. No existing id covers it: `grep -rn 'grok-4.5' docs/gap-analysis/` hits only the model-resolver item, never the api mismatch.

**Fix** — Regenerate `xai.json` from `b0c2a90e` (`PROV-018` / `PROV-060`); do NOT hand-patch the single row, because the same file also carries five retired models (`PROV-058`) and two field diffs (`PROV-059`) and a one-row patch leaves the manifest lying about all three. If `PROV-018` is not imminent, the **whole `xai.json` file** may be replaced in one commit with the `b0c2a90e` extraction and `catalog_manifest.json` amended in the same commit to record xai's revision explicitly.

**Verify** — `pick(&selection(), "xai", "grok-4.5").api == "openai-responses"`; its `thinking_level_map` maps `off` and `minimal` to `None`; `get_responses_compat` resolves `supports_long_cache_retention == false`; and a wire test asserting the request body for `xai/grok-4.5` is a Responses envelope (`input`) rather than a Completions one (`messages`). All four are RED today.

**CLOSED 2026-08-15 (sweep 10) — by the regeneration, not by hand.** `xtask/src/main.rs`
`gen-catalogs` rewrote `xai.json` (and the other 29 that moved) from `b0c2a90e` in one commit with
`catalog_manifest.json`. All four Verify conditions are green: `api == "openai-responses"`,
`thinking_level_map` maps `off`/`minimal` to `None`, `get_responses_compat` resolves
`supports_long_cache_retention == false`, and `WireProvider` (`wire.rs:215`) now dispatches the row
to the Responses impl. Confirmed at the **ported tag** as well as at `b0c2a90e`:
`XAI_RESPONSES_MODEL_ID = "grok-4.5"` (`ai/scripts/generate-models.ts:378` @v0.83.0), consumed at
`:1408`; `XAI_RESPONSES_COMPAT` `:390-392`; `XAI_RESPONSES_EFFORT_LEVEL_MAP` `:386-389`. Test:
`tests/catalog_data.rs::xai_grok_4_5_is_routed_over_the_responses_api`, RED at HEAD on all four
assertions. `providers/fleet.rs`'s `every_catalog_parses_with_expected_count` asserted every fleet
row was `openai-completions` — a wrong invariant that this row corrects; the carve-out is spelled as
an id, so a second row drifting off the protocol still fails.

## PROV-055 — `opencode`: `sessionAffinityFormat: "openai-nosession"` missing on all 16 `openai-responses` rows, so cyrup leaks a `session_id` header pi suppresses

**Kind** stale-port · **Severity** **high** · **Effort** S · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**upstream** — pi `packages/ai/src/providers/opencode.models.ts` @`b0c2a90e` sets `compat {"sessionAffinityFormat":"openai-nosession"}` on `gpt-5`, `gpt-5-codex`, `gpt-5-nano`, `gpt-5.1`, `gpt-5.1-codex`, `gpt-5.1-codex-max`, `gpt-5.1-codex-mini`, `gpt-5.2`, `gpt-5.2-codex`, `gpt-5.3-codex`, `gpt-5.4`, `gpt-5.4-mini`, `gpt-5.4-nano`, `gpt-5.4-pro`, `gpt-5.5`, `gpt-5.5-pro` (all `api: openai-responses`). Consumed at `packages/ai/src/api/openai-responses.ts:232-241`.

**cyrup** — all 16 rows in `crates/cyrup-provider/src/providers/catalog/opencode.json` carry `"compat": null`.

**Impact** — **Exactly the `PROV-023`/`024`/`033`/`034` shape, and unconditionally live.** The `openai-responses` header gate is `if (sessionId)` with **no** `sendSessionAffinityHeaders` guard (`openai-responses.ts:232`), so the *format alone* decides the headers. With the flag omitted, `get_responses_compat` (`api/compat.rs:267-286`) falls back to `detect_session_affinity_format` (`compat.rs:84-92`), which returns `Openai` for provider `opencode` — and cyrup then emits a `session_id` header (`api/openai_responses.rs:553-555`) that pi deliberately suppresses, on all 16 models, on every request that carries a session id. A silent wire difference that leaks a session identifier to OpenCode Zen. This is the fourth instance of the class the ledger already tracks four of; the class is not "occasionally wrong", it is "wrong by default whenever the catalog is silent".

**Fix** — `PROV-018` / `PROV-060` regeneration of `opencode.json`. **A stopgap worth considering separately, because it removes the whole class:** `detect_session_affinity_format` invents a default for a field pi resolves purely from data. Auditing it against the providers that actually declare the flag upstream — and preferring `NoSession` where pi's catalogs say so — would be a code fix in this crate rather than a data fix, but it must not be done by guessing; derive it from the `b0c2a90e` catalogs.

**Verify** — For each of the 16 ids, `get_responses_compat(model).session_affinity_format == SessionAffinityFormat::OpenaiNoSession`, and `build_headers` with a session id present emits **no** `session_id` header. RED today on all 16.

**CLOSED 2026-08-15 (sweep 10) — by the regeneration.** All **19** `openai-responses` rows (the 16
this item enumerated plus the GPT-5.6 trio PROV-057 added) now carry
`sessionAffinityFormat: "openai-nosession"`. Confirmed at the ported tag: pi builds every
`@ai-sdk/openai` OpenCode variant with that compat at `ai/scripts/generate-models.ts:1666` @v0.83.0.
**The stopgap this item floated was deliberately NOT taken** — `detect_session_affinity_format` is
untouched and still answers `Openai` for `opencode`, because upstream resolves this purely from
data and a changed default would diverge everywhere the data is right. Tests:
`tests/catalog_data.rs::every_opencode_responses_row_suppresses_the_session_id_header` (scoped to
the whole api, plus a MIRROR asserting the detector is unchanged) and the wire half,
`api/openai_responses.rs::opencode_responses_rows_never_emit_a_session_id_header`, which drives
`build_headers` with a session id over the SHIPPED catalog and asserts no `session_id` while
`x-client-request-id` survives. Both RED at HEAD on every row.

## PROV-056 — `kimi-coding`: `forceAdaptiveThinking` (×3) and `allowEmptySignature` (×1) missing — two wire divergences per request on every model of the provider

**Kind** stale-port · **Severity** **high** · **Effort** S · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**upstream** — pi `packages/ai/src/providers/kimi-coding.models.ts` @`b0c2a90e` — `k2p7` compat `{"forceAdaptiveThinking":true}`; `kimi-k2-thinking` compat `{"forceAdaptiveThinking":true}`; `kimi-for-coding` compat `{"allowEmptySignature":true,"forceAdaptiveThinking":true}`. All three carry `{}` at `91585d9a`, so this is the same one-week provenance gap.

**cyrup** — `crates/cyrup-provider/src/providers/catalog/kimi-coding.json`: all three rows have `"compat": null`.

**Impact** — `forceAdaptiveThinking` is read **raw off `model.compat`**, not through a resolver, at three sites in pi's `anthropic-messages` route (`api/anthropic-messages.ts:815`, `:858`, `:1033`): it forces `thinking.type: "adaptive"` plus `output_config.effort`, and at `:858` it **suppresses** the interleaved-thinking beta header (`needsInterleavedBeta = interleavedThinking && model.compat?.forceAdaptiveThinking !== true`). cyrup therefore sends the non-adaptive thinking block **and** the `interleaved-thinking-2025-05-14` beta to an upstream pi has flagged as requiring the adaptive format — two divergences per request, on all three models, which is every model this provider has. `allowEmptySignature:true` missing on `kimi-for-coding` additionally makes cyrup convert empty-signature thinking blocks to text instead of replaying `signature:""` (`anthropic-messages.ts:967`), corrupting thinking replay. Note the interaction with `PROV-059`: the same three rows are also priced at zero.

**Fix** — `PROV-018` / `PROV-060` regeneration of `kimi-coding.json`.

**Verify** — `get_anthropic_compat` is not the right probe (pi excludes `forceAdaptiveThinking` from its resolver and cyrup correctly matches that) — assert on the raw model: `pick(&selection(),"kimi-coding","k2p7").compat.unwrap().force_adaptive_thinking == Some(true)` for all three, `allow_empty_signature == Some(true)` for `kimi-for-coding`, plus a `build_params` test asserting `thinking.type == "adaptive"` and the ABSENCE of the interleaved beta header. RED today.

**CLOSED 2026-08-15 (sweep 10) — by the regeneration.** All **5** rows the provider now has (3 + 2
added by PROV-057) carry `forceAdaptiveThinking: true`; `kimi-for-coding` and `k3` carry
`allowEmptySignature: true`. Confirmed at the ported tag: `ai/scripts/generate-models.ts:1861-1864`
@v0.83.0 constructs every kimi-coding row with the flag, conditionally adding
`allowEmptySignature`. The zero pricing on the same rows (PROV-059(a)) was fixed in the same write.
Tests: `tests/catalog_data.rs::kimi_coding_rows_force_adaptive_thinking` probes the RAW
`model.compat` exactly as this item's Verify demanded (`get_anthropic_compat` is deliberately not
the probe — pi excludes the flag from its resolver and cyrup correctly matches that), and
`api/anthropic_messages.rs::kimi_coding_catalog_rows_send_adaptive_thinking_and_no_interleaved_beta`
asserts the wire effect over the shipped catalog: `thinking.type == "adaptive"` and NO
`interleaved-thinking-2025-05-14` beta. Both RED at HEAD.

## PROV-057 — 25 catalog models present upstream are absent from cyrup, across 9 catalogs

**Kind** stale-port · **Severity** medium · **Effort** M · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**upstream** — pi `packages/ai/src/providers/{azure-openai-responses,cloudflare-ai-gateway,kimi-coding,moonshotai,moonshotai-cn,openai,opencode,opencode-go,openrouter,vercel-ai-gateway}.models.ts` @`b0c2a90e`.

**cyrup** — enumerated exhaustively, by catalog:

| catalog | absent model ids |
|---|---|
| `azure-openai-responses` | `gpt-realtime-2.1` |
| `cloudflare-ai-gateway` | `gpt-5.6-luna`, `gpt-5.6-sol`, `gpt-5.6-terra`, `workers-ai/@cf/zai-org/glm-5.2` |
| `kimi-coding` | `k3`, `kimi-for-coding-highspeed` |
| `moonshotai` | `kimi-k3` |
| `moonshotai-cn` | `kimi-k3` |
| `openai` | `gpt-realtime-2.1` |
| `opencode` | `gpt-5.6-luna`, `gpt-5.6-sol`, `gpt-5.6-terra` |
| `opencode-go` | `grok-4.5`, `kimi-k3` |
| `openrouter` | `kwaipilot/kat-coder-air-v2.5`, `kwaipilot/kat-coder-pro-v2.5`, `meta/muse-spark-1.1`, `moonshotai/kimi-k3` |
| `vercel-ai-gateway` | `anthropic/claude-opus-4.7-fast`, `anthropic/claude-opus-4.8-fast`, `kwaipilot/kat-coder-air-v2.5`, `kwaipilot/kat-coder-pro-v2.5`, `moonshotai/kimi-k3`, `thinkingmachines/inkling` |

**Impact** — Each is a model id that resolves in pi and errors here. The root cause is **one provenance decision, not nine bugs**: 31 of 35 catalogs were extracted at `91585d9a` (2026-07-10) though `b0c2a90e` (2026-07-17, the last in-git revision) was available, and 12 of those 31 changed in the intervening week and were never refreshed. `catalog_manifest.json` documents the split. See `PROV-060`.

**Fix** — `PROV-018` / `PROV-060`. Do not add rows by hand.

**Verify** — After regeneration, `models.get_model(provider, id).is_some()` for all 25.

**CLOSED 2026-08-15 (sweep 10).** The regeneration added exactly the 25 ids enumerated above —
`gen-catalogs --diff` reports `25 missing rows` and the per-catalog breakdown matches this item's
table row for row, an independent reproduction of the sweep-9 count. Test:
`tests/catalog_data.rs::every_model_the_regeneration_added_now_resolves`, which asserts
`get_model` resolves all 25 through model selection rather than against the raw JSON. RED at HEAD.

## PROV-058 — 16 catalog models exist in cyrup that upstream retired before the ported tag

**Kind** cyrup-original · **Severity** medium · **Effort** M · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**cyrup** — `crates/cyrup-provider/src/providers/catalog/{xai,vercel-ai-gateway,openrouter}.json`. Exhaustive list: **xai** `grok-3`, `grok-3-fast`, `grok-4.20-0309-non-reasoning`, `grok-4.20-0309-reasoning`, `grok-code-fast-1` (cyrup ships 8 xai models, pi ships 3); **vercel-ai-gateway** `anthropic/claude-3.5-haiku`, `arcee-ai/trinity-large-preview`, `meituan/longcat-flash-chat`, `meituan/longcat-flash-thinking-2601`, `mistral/devstral-small`, `mistral/pixtral-large`, `xiaomi/mimo-v2-flash`, `xiaomi/mimo-v2-pro`; **openrouter** `arcee-ai/trinity-mini`, `liquid/lfm-2.5-1.2b-thinking:free`, `openai/gpt-oss-120b:free`.

**upstream** — all 16 were removed between `91585d9a` and `b0c2a90e`, i.e. **before** the ported tag `v0.83.0`.

**Impact** — This is the reverse face of `PROV-057`'s single provenance gap rather than free invention, but it is a real divergence and it is the class this project has no habit of tracking: cyrup **offers, autocompletes and will attempt to stream** 16 model ids pi retired, and their pricing rows keep accruing in usage totals. An id that is gone upstream is usually gone at the vendor too, so the reachable outcome is a model that appears in `/model`, is selectable, and then fails at the API.

**Fix** — `PROV-018` / `PROV-060` regeneration, which deletes them as a side effect of replacing the file. Do not delete by hand for the reason in the section preamble.

**Verify** — After regeneration, `models.get_model(provider, id).is_none()` for all 16, and `xai`'s catalog has exactly 3 rows.

**CLOSED 2026-08-15 (sweep 10).** The regeneration removed exactly the 16 enumerated — `--diff`
reports `16 retired rows` — and `xai` is back to 3. Test:
`tests/catalog_data.rs::every_model_upstream_retired_is_gone`, which also pins the xai row count.
RED at HEAD. **The xai half is confirmed at the PORTED TAG, not merely at `b0c2a90e`:** pi drops
those five by NAME through `XAI_BUILTIN_EXCLUDED_MODEL_IDS`
(`ai/scripts/generate-models.ts:379-385` @v0.83.0, applied at `:2078`), so this half is not
models.dev churn and cannot come back on a later refresh.

## PROV-059 — 119 non-compat catalog field differences on models present in both sides

**Kind** stale-port · **Severity** medium · **Effort** M · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**upstream** — pi `packages/ai/src/providers/*.models.ts` @`b0c2a90e` vs `crates/cyrup-provider/src/providers/catalog/*.json` — **1027 shared models compared field-by-field.**

**cyrup** — breakdown by field: `cost` 55, `maxTokens` 36, `contextWindow` 25, `thinkingLevelMap` 2, `api` 1. By provider: `openrouter` 97, `openai-codex` 5, `vercel-ai-gateway` 5, `kimi-coding` 3, `azure-openai-responses` 2, `openai` 2, `xai` 2, `cerebras` 1, `groq` 1, `opencode` 1. (The single `api` difference is `PROV-054`; one of the two `thinkingLevelMap` differences is `PROV-064`, which is deliberate.)

**Impact** — **These are behavioural, not cosmetic.** Concrete worst cases:
* **(a)** `kimi-coding` `k2p7` / `kimi-for-coding` / `kimi-k2-thinking` all have `cost {input:0, output:0, cacheRead:0, cacheWrite:0}` in cyrup against real prices upstream (0.95/4/0.19 and 0.6/2.5/0.15) — **every kimi-coding session reports zero spend.**
* **(b)** `cerebras` `zai-glm-4.7` has `cacheRead: 0` in cyrup vs `2.25` upstream (`cerebras.models.ts:55`) — cached reads billed at zero.
* **(c)** `openai` / `openai-codex` / `azure` `gpt-5.6-luna` and `gpt-5.6-terra` carry roughly 5× understated cost tiers (luna `input` 0.2 vs 1; terra `input` 2 vs 2.5 with every tier scaled down).
* **(d)** `openai-codex` `gpt-5.6-luna`/`sol`/`terra` have `contextWindow: 272000` in cyrup vs `372000` upstream — **cyrup compacts 100k tokens early on all three.**

`maxTokens` and `contextWindow` feed overflow estimation and compaction triggers; `cost` feeds `/session` and every usage total.

**Fix** — `PROV-018` / `PROV-060`. **`PROV-004` is the nearest existing tracker and it explicitly classifies this as unverifiable** ("no longer checkable from this workspace"); that premise is refuted — see the correction block under `PROV-004` and see `PROV-060`. These are demonstrated wrong values now, not audit debt.

**Verify** — The regeneration recipe in `PROV-060` re-run against `b0c2a90e` yields zero field differences on the 1027 shared models, and that recipe becomes `PROV-018`'s drift check.

**CLOSED 2026-08-15 (sweep 10) — 109 fixed, 3 REFUTED, 7 PRESERVED.** The regeneration applied
**129** field differences in total, of which 109 are this item's (`cost` 49, `maxTokens` 36,
`contextWindow` 22, `api` 1, `thinkingLevelMap` 1) and 20 are the compat rows of PROV-054/055/056.
Before the pins, `gen-catalogs --diff` reproduced this item's tallies EXACTLY — `cost` 55,
`maxTokens` 36, `contextWindow` 25, `api` 1 — and its per-provider split (`openrouter` 97,
`openai-codex` 5, `vercel-ai-gateway` 5, `kimi-coding` 3, `azure` 2, `openai` 2, `xai` 2,
`cerebras` 1, `groq` 1, `opencode` 1) row for row, which is a strong independent confirmation of
sweep 9's measurement. Worst cases (a)-(c) are fixed and asserted by
`tests/catalog_data.rs::the_regenerated_field_values_match_the_pinned_revision`.

**Claim (d) is REFUTED — 3 of the 119.** The three `openai-codex` GPT-5.6 `contextWindow`s are
`272000` at BOTH v0.83.0 (`ai/scripts/generate-models.ts:2352`) and v0.84.1 (`:2541`); v0.83.0's own
comment at `:2349` reads *"GPT-5.6 follows Codex's 272k catalog limit (formerly 372k)"*. `372000` is
the FORMER value, still sitting in `b0c2a90e`'s generated data 13 days before the tag. cyrup's
`272000` was right and taking `b0c2a90e` here would have inflated the window 100k past the real
limit — the opposite of the filed impact. The Codex rows are hardcoded in the generator script
rather than fetched from models.dev, and that script IS in git at the ported tag, so for these rows
`v0.83.0` beats `b0c2a90e`. Guarded by
`tests/catalog_data.rs::the_codex_gpt_5_6_context_window_stays_at_the_ported_tags_272k`.

**7 PRESERVED, each as an explicit `DELTAS` entry with citations (`xtask/src/main.rs`).** Six are
the GPT-5.6 luna/terra cost rows on `openai`, `azure-openai-responses` and `openai-codex`: a
documented v0.84.1 forward-port of OpenAI's 2026-07-30 price cut, already pinned by three tests, and
reverting it to the `b0c2a90e`/v0.83.0 literals would bill users 5x (Luna) and 1.25x (Terra) over
the real rate. That is a live decision a previous pass took deliberately, so it is preserved and
tagged rather than silently reverted — but it IS a divergence from the ported tag and is now
findable as one. The seventh is groq `qwen/qwen3-32b` (PROV-064). The generator hard-errors if any
pin becomes a no-op or names a row upstream has dropped, so a stale exception cannot rot in silence.

## PROV-060 — Catalog provenance is split across two pi revisions, both predate the ported tag — and the "not statically auditable" premise is REFUTED

**Kind** tooling · **Severity** medium · **Effort** M · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**upstream** — pi `.gitignore:11` gitignores `packages/ai/src/providers/data/` at `v0.83.0`; `a9f6a3159` (`feat(ai): separate generated model data (#6765)`) is the commit that added that line, and **`b0c2a90e` is its direct parent** — `git log --oneline b0c2a90e..a9f6a3159` returns exactly one commit.

**cyrup** — `crates/cyrup-provider/src/providers/catalog_manifest.json` records `generatedAt` `2026-07-17T09:00:03Z` / `source` `pi@b0c2a90e`, but **its own note concedes only 4 catalogs** (`amazon-bedrock`, `github-copilot`, `google-vertex`, `openai-codex`) came from `b0c2a90e` while the other **31 came from `91585d9a`** (2026-07-10). Twelve of those 31 changed in the intervening week — `azure-openai-responses`, `cerebras`, `cloudflare-ai-gateway`, `kimi-coding`, `moonshotai`, `moonshotai-cn`, `openai`, `opencode`, `opencode-go`, `openrouter`, `vercel-ai-gateway`, `xai`.

**Impact** — Two distinct things, and the second is the important one.

1. **That single unrefreshed week is the root cause of `PROV-054` … `PROV-059`** — 25 missing models, 16 retired-but-shipped models, 27 of the 28 compat differences and most of the 119 field differences. The manifest's *value* is right (`PROV-039` correctly demanded the LATEST revision, and it was set) but it **describes a floor the catalogs do not actually sit on**, which is a worse failure than the one `PROV-039` closed: the drift guard now reports a provenance the data does not have.
2. **`PROV-004`'s and `PARITY-GAPS.md:931` (`OQ-5`)'s "not statically auditable" verdict is FALSE.** Both record, as settled, that "every `*.models.ts` at v0.84.1 is a two-line re-export, so no pricing, context-window, maxTokens or compat-flag claim about the 35 embedded catalogs can be checked by reading this workspace", and `PROV-004` is downgraded to "a verification task, not a fix task" on that basis. The re-export form begins at `a9f6a3159`; at its parent `b0c2a90e` — **precisely the revision cyrup's own manifest names as its provenance floor** — the files are full data literals. The entire catalog is checkable with `git show b0c2a90e:packages/ai/src/providers/<p>.models.ts` plus a ~12-line node script: no generator run, no `npm install`, no network. Nine sweeps read the backlog and inherited the "unverifiable" verdict; the data was one `git show` away.

**Residue that a clean refresh does NOT remove, stated so nobody claims parity from it:** both revisions predate `v0.83.0` (2026-07-30) by 13–20 days. Even a perfect refresh to `b0c2a90e` leaves an **unmeasurable** 13-day window, because from `a9f6a3159` onward the data is genuinely not in git. Any future claim of catalog parity at `v0.83.0` is therefore a claim about `b0c2a90e` plus an unbounded delta, and should say so.

**Fix** — (1) Make `PROV-018`'s `xtask gen-catalogs` extract from `b0c2a90e` via `git show`, rewrite all 35 files **and** `catalog_manifest.json` in one commit, and make the manifest **per-provider** (`{provider: {generatedAt, source}}`) so a split can never again be described by a single value — `remote_catalog.rs`'s floor comparison then becomes per provider, which is what `PROV-039`'s Fix already proposed. (2) Ship the same extraction as `PROV-018`'s **drift check**: it is a diff, so it can run in CI against a pinned pi worktree. (3) Record the irreducible 13-day residue in the manifest note itself, not only here.

**Verify** — `cargo xtask gen-catalogs` reproduces all 35 files byte-for-byte from `b0c2a90e`; the manifest names a revision per provider; the drift check fails when a catalog is hand-edited (this is the guard `PROV-061` had to be closed without).

**CLOSED 2026-08-15 (sweep 10).** `xtask/` now holds a dependency-free `gen-catalogs`:
`src/tsdata.rs` is a scanner for the data-literal subset pi's generator emits (refusing, never
skipping, anything outside it), and `src/main.rs` extracts all 35 catalogs plus the manifest from
one revision via `git show`. **Both of this item's claims are actioned.** (1) The split is gone:
one revision generates every file, and `catalog_manifest.json` now carries a per-provider
`{source, module}` map so a future split cannot hide behind a single value — the shape `PROV-039`'s
Fix asked for. (2) The "not statically auditable" verdict is refuted in the tree, not only here:
`tests/catalog_data.rs`'s module header, which carried it, is rewritten, and the same correction is
owed to `PARITY-GAPS.md:931` (`OQ-5`), which still records it — **left open, outside this area's
files**. The irreducible 13-day residue is recorded in the manifest note itself, as this item's Fix
(3) required, and asserted by
`tests/catalog_data.rs::the_catalog_manifest_names_one_revision_per_provider`. Drift check:
`gen-catalogs --check` (byte-exact, PROV-018's) and `--diff` (structural), plus the `#[ignore]`d
`gen_catalogs_check_reports_no_drift_against_the_pinned_revision`. Verified idempotent: a second run
reports `all 36 files match pi@b0c2a90e`.

## PROV-061 — `fireworks` `glm-5p2` / `glm-5p2-fast` carried two INVENTED compat flags — **FILED AND CLOSED 2026-08-14**, **SUPERSEDED BY `DRIFT-052` 2026-08-15**

> **SUPERSEDED 2026-08-15 by `DRIFT-052` (`12-upstream-drift-pi-core.md`) — this item's
> analysis stands; its OUTCOME is reversed by evidence it did not have.** Everything below
> about provenance is correct: pi carries neither flag at `91585d9a` or `b0c2a90e`, and the
> block WAS pattern-matched off the fourteen `anthropic-messages` rows with nothing behind
> it. But pi `b9497c8c1` ("fix(ai): correct Fireworks GLM prompt caching, closes #7676",
> first tag **v0.84.0**, unchanged at v0.84.2) sets both keys on exactly these two rows, via
> the shared `openAICompat` constant in `processFireworksModels`
> (`ai/scripts/generate-models.ts:1239-1244` @v0.84.2, applied at `:1274-1280`) — the v0.83.0
> shape this item restored is an upstream BUG and #7676 is the report of it. So the values
> are back, as a signed-off forward-port carried by `xtask/src/main.rs`'s `DELTAS` table
> (`WHY_FIREWORKS_GLM_COMPAT`) rather than by hand, which is the difference that matters:
> they now have a citation and `gen-catalogs --check` still reproduces the tree.
>
> **The test named in Verify below no longer exists under that name.**
> `fireworks_openai_completions_rows_carry_no_invented_affinity_or_cache_flags` is now
> `fireworks_openai_completions_rows_carry_pi_s_openai_compat`, with the same whole-provider
> scope and the same row-count pin, asserting the pinned values instead of their absence.
>
> **`supportsLongCacheRetention: false` is NOT inert, contrary to the Impact paragraph
> below.** `detect_compat` computes it as `!(is_together || is_cloudflare_workers_ai ||
> is_cloudflare_ai_gateway || is_nvidia || is_ant_ling)` (`api/compat.rs:678-682`), and
> fireworks is on none of those lists, so absent resolves to **true** — cyrup requested a
> retention Fireworks does not honour. The old reading ("inert because `cacheControlFormat`
> is not `anthropic`") checked only one of its consumers.

**Kind** cyrup-original · **Severity** medium · **Effort** S · **Confidence** confirmed · **Filed and closed** 2026-08-14 (sweep 9)

**cyrup** — `crates/cyrup-provider/src/providers/catalog/fireworks.json`, rows `accounts/fireworks/models/glm-5p2` and `accounts/fireworks/routers/glm-5p2-fast`, compat `{supportsStore:false, supportsDeveloperRole:false, sendSessionAffinityHeaders:true, supportsLongCacheRetention:false}`.

**upstream** — pi has compat `{"supportsStore":false,"supportsDeveloperRole":false}` for both rows at **BOTH** provenance revisions (`91585d9a` and `b0c2a90e`, `fireworks.models.ts:61-68` and `:224-231`). The two extra flags exist in neither, so this is **invention, not staleness** — which is why it was closable here while `PROV-054` … `PROV-059` are not: removing it moves the data toward every upstream revision at once.

**Impact** — A genuine cyrup-original with a live wire effect, and the mechanism is visible in the file: these two are the **only** `openai-completions` rows in `fireworks.json`; the other 14 are `anthropic-messages` and legitimately carry `{sendSessionAffinityHeaders:true, supportsEagerToolInputStreaming:false, supportsCacheControlOnTools:false, supportsLongCacheRetention:false}`. The flag block was **pattern-matched across the whole provider instead of copied per row** — plausibly encouraged by pi's own doc comment at `types.ts:604-611`, which names Fireworks as the motivating case for `sendSessionAffinityHeaders`. Effect: on the `openai-completions` route the emission gate is `if (sessionId && compat.sendSessionAffinityHeaders)` (`openai-completions.ts:647`), and `detect_compat` gives `false` for fireworks (`api/compat.rs:491`), so the catalog value alone decided it — cyrup emitted `session_id`, `x-client-request-id` and `x-session-affinity` on these two models where pi emits none. `supportsLongCacheRetention:false` is inert here (`cacheControlFormat` is not `"anthropic"` for fireworks, so `getCompatCacheControl` short-circuits at `openai-completions.ts:883`) but is still an unfounded value. **This is the `CYRUP_SHARE_VIEWER_URL` shape**: a flag that reads as deliberate and has no upstream warrant.

**Fix — LANDED.** Both invented keys deleted from both rows; `supportsStore`/`supportsDeveloperRole` kept exactly as upstream has them. No other row in the file was touched — the fourteen `anthropic-messages` rows keep `sendSessionAffinityHeaders: true` because that **is** upstream's value for them.

**Verify — DONE.** `crates/cyrup-provider/src/tests/catalog_data.rs` →
`fireworks_openai_completions_rows_carry_no_invented_affinity_or_cache_flags`. RED before on both
rows. The assertion is scoped to **every** `openai-completions` row of the provider rather than the
two ids, so re-introducing the pattern-match on a future row fails too; it also asserts the two
legitimate flags are still present, so the test cannot be satisfied by deleting the compat block, and
it pins the row count at 2 with a message telling the next reader to re-derive the scope if that
changes. `cargo check -p cyrup-provider --all-targets` green.

## PROV-062 — `providers/all.rs`'s port-status table omits `all.ts:115-117` and its guard asserted the opposite of `PROV-014` — **FILED AND CLOSED 2026-08-14**

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed and closed** 2026-08-14 (sweep 9)

**upstream** — pi `packages/ai/src/providers/all.ts:115-117` @`v0.83.0` — `qwenTokenPlanProvider()`, `qwenTokenPlanCnProvider()`, `radiusProvider()`.

**cyrup** — an in-tree documentation defect, in the file `PROV-014` is about. `crates/cyrup-provider/src/providers/all.rs`'s header tabulated pi's `builtinProviders()` line by line and **had no row for `all.ts:115`, `:116` or `:117`** — the mapping silently ended at 33 of 36 entries. The prose immediately below then asserted "Every provider pi's `builtinProviders()` constructs is registered below", which is false. The guard test went further: where the not-yet-ported assertion used to be, it recorded "Every built-in provider pi ships is now ported, so there is no not-yet list left to assert against" — **so the file that documents the gap denied it, in the same header that scolds an earlier sweep for exactly this failure mode.**

**Root cause, found while fixing and worth more than the fix** — every line number in that table was a **`91585d9a` offset carried under a declared `v0.83.0` baseline**. At `91585d9a`, `amazonBedrockProvider()` really is `all.ts:72` as the table said; at `v0.83.0` it is `:89`. And `git show 91585d9a:packages/ai/src/providers/all.ts | grep -n 'qwenTokenPlan\|radiusProvider'` returns **nothing** — the three providers did not exist at the revision the table was transcribed from. The omission was not carelessness; it was a faithful transcription of the wrong revision, which is the same defect class as the nine wrong-at-the-tag citations the 2026-08-12 repair pass corrected (`PROV-041`), and it is why the omission survived every subsequent read of the file.

**Fix — LANDED.** (1) The whole table re-derived at `v0.83.0` (offsets `:89`–`:126`) with three new rows for `qwen-token-plan`, `qwen-token-plan-cn` and `radius` marked **`✗ NOT REGISTERED — PROV-014`**. (2) The false summary replaced with "33 of pi's 36 built-in providers are registered below", scoped so the surviving true half (every registered provider's api ids have impls) is still stated and still names the test that enforces it. (3) The deleted not-yet-ported assertion **restored** with the three ids, with a message telling a future porter to remove the id from the array and the NOT-REGISTERED row from the table in the same commit. (4) A note in the header recording what the table used to say, so a reader who remembers the old text can see it was wrong rather than assume the file regressed.

**Verify — DONE.** `crates/cyrup-provider/src/providers/all.rs` →
`registry_contains_implemented_provider_ids` now asserts `qwen-token-plan`, `qwen-token-plan-cn` and
`radius` are **absent** from `default_models(...)`'s provider ids. It is green today and goes red the
moment one is registered without a working stream path — the "absent until real" invariant the array
was created for and was deleted with. `cargo check -p cyrup-provider --all-targets` green. **This does
not close `PROV-014`**, which is the actual port work.

## PROV-063 — `ModelCompat::supports_finish_reason` is a v0.84.1 flag with no v0.83.0 warrant

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**cyrup** — `crates/cyrup-provider/src/api/compat.rs:126-131` (`ModelCompat`), `:302-303` (`ResolvedCompat`), detected `true` at `:469`, resolved at `:519-521`; sole consumer `api/openai_completions.rs:1709`.

**upstream** — `git grep supportsFinishReason v0.83.0 -- packages/ai` is **empty**. The flag first appears at v0.84.1 (`types.ts:548`, `openai-completions.ts:578`, `:584`, `:1499`, `:1551`).

**Assessment — a knowing forward-port, correctly labelled, and inert.** cyrup's doc comment cites the v0.84.1 lines explicitly rather than claiming a v0.83.0 warrant. It is the **only** field in cyrup's `ModelCompat` with no v0.83.0 counterpart — all 38 pi flags across the four compat interfaces are present and no other extras exist. No embedded catalog sets it (`grep` over `providers/catalog/*.json` is empty) and `detect_compat` pins it `true`, so the only branch that reads it (`openai_completions.rs:1709`) is unreachable in every shipped configuration and behaviour is byte-identical to v0.83.0.

**Impact** — None today. Filed because an inventoried cyrup-original is the point: this is the one place a reader auditing "does cyrup's compat model match pi's" will find a field that is not in the baseline, and without a row they will either re-derive the analysis or, worse, treat its presence as licence for the next one.

**Fix** — No code change proposed. Either keep it with a `[CYRUP-DELTA]` tag naming v0.84.1 as its warrant (the cheap option, and consistent with how the project marks forward-ports elsewhere), or fold it into whatever item lands the v0.84.1 rebase. **Do not silently delete it** — it is real upstream behaviour, just not at this tag.

**Verify** — A test asserting `detect_compat(..).supports_finish_reason == true` for every provider, so the flag's inertness is a pinned property rather than an observation that decays.

## PROV-064 — `groq` `qwen/qwen3-32b` has its `thinkingLevelMap` deliberately removed relative to the ported tag, with no `[CYRUP-DELTA]` tag

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**cyrup** — `crates/cyrup-provider/src/providers/catalog/groq.json` has no `thinkingLevelMap` on `qwen/qwen3-32b`, with the rationale and a guard test at `providers/fleet.rs:257-278`.

**upstream** — pi at `b0c2a90e` gives the row `thinkingLevelMap {minimal:null, low:null, medium:null, high:"default"}`. `fleet.rs:257-262` explains that upstream retargeted the sole generator override from `qwen/qwen3-32b` (v0.83.0 `generate-models.ts:837`) to `qwen/qwen3.6-27b` (v0.84.1 `:870`), and the test at `:264` asserts the absence.

**Impact** — A deliberate, documented and tested v0.84.1 forward-port — **but it IS a divergence from v0.83.0** and it should carry a `[CYRUP-DELTA]` tag rather than only a prose note, because the tag is what makes it findable by the mechanism the project uses to find accepted divergences. Effect at the ported tag: pi maps `high` to the literal `"default"` and pins `low`/`medium` to `null` for this model; cyrup passes the raw effort through. It is one of only **two** `thinkingLevelMap` differences in the entire 1027-model comparison — the other is `PROV-054`, which is not deliberate.

**Fix** — Add the `[CYRUP-DELTA]` tag at `providers/fleet.rs:257-262` naming the v0.83.0 value it diverges from and the v0.84.1 commit that justifies it. Note the interaction with `PROV-060`: a regeneration from `b0c2a90e` will **re-introduce** the map and turn the guard test red, so the regeneration must carry this exception explicitly or it will be silently reverted.

**Verify** — The `[CYRUP-DELTA]` grep finds it; `PROV-018`'s generator has a named exception list containing this row, and its drift check reports the row as an accepted difference rather than as noise.

## PROV-065 — `openrouter-images.json` is a catalog file pi has no `*.models.ts` counterpart for

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**cyrup** — `crates/cyrup-provider/src/providers/catalog/openrouter-images.json`, 35 rows.

**upstream** — pi keeps image models in `packages/ai/src/image-models.generated.ts` (`IMAGE_MODELS.openrouter`) rather than in `providers/*.models.ts`, so the file has **no upstream counterpart by name**. Content verified **exact**: the 35 ids match `image-models.generated.ts` @`b0c2a90e` with zero differences in either direction.

**Impact** — A benign structural original. Filed for one reason only: so the **35-vs-35 catalog count is not mistaken for a clean one-to-one mapping.** The true mapping is 34 shared names, plus `together` (inline Rust in cyrup, `together.models.ts` in pi — **21/20 as of 2026-08-15: pi's 20 rows exact, plus the signed-off `moonshotai/Kimi-K3` addition, `PROV-070`**), plus `openrouter-images`, against pi's 37 `*.models.ts` files. A future sweep that compares directory counts and stops will conclude the catalog set is complete; it is not (`PROV-014` names three providers with no catalog at all).

**Fix** — No code change. `PROV-018`'s generator must source this file from `image-models.generated.ts`, not from `providers/*.models.ts`, and `catalog_manifest.json`'s per-provider note (`PROV-060`) should record that different source path.

**Verify** — The generator reproduces `openrouter-images.json` from `image-models.generated.ts`; a comment in the file names its upstream source so the next reader does not go looking for `openrouter-images.models.ts`.

## PROV-066 — `open_router_routing` is typed `serde_json::Value` where pi declares a structured `OpenRouterRouting`

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**cyrup** — `crates/cyrup-provider/src/api/compat.rs:147` types `open_router_routing` as `serde_json::Value`.

**upstream** — pi `packages/ai/src/types.ts:664-707` declares `OpenRouterRouting` with 11 fields, including the `sort` string-or-object union and the five-key `max_price` object.

**Impact** — Wire-identical: the value is passed through verbatim as the `provider` request field on both sides. What is lost is **validation** — cyrup accepts any JSON a user writes into `openRouterRouting` and forwards it, where pi rejects a misspelled key at the type level. The failure mode is a silently ignored routing preference (OpenRouter ignores unknown members), which presents as "my `order` never takes effect" with nothing anywhere saying why.

**Verified clean while here, recorded so it is not re-derived** — the headline shape difference this finding started from is a **non-issue**: `ResolvedCompat` (`compat.rs:297-326`) has no counterpart for `openRouterRouting` or `vercelGatewayRouting`, and neither does it need one. pi never reads the resolved copies — both emission sites read the RAW `model.compat` (`openai-completions.ts:823` `if (model.compat?.openRouterRouting)` and `:828` `if (model.compat?.vercelGatewayRouting)`) — so pi's two resolved fields are dead. cyrup does the same at `openai_completions.rs:436-454`, and the vercel gateway shape matches exactly (only/order gate, `providerOptions.gateway` envelope). Documented at `compat.rs:294-296`.

**Fix** — Give `open_router_routing` a typed struct mirroring `types.ts:664-707`, with `deny_unknown_fields` and an untagged enum for `sort`. Serialization must stay byte-identical (skip-if-none on every optional).

**Verify** — A round-trip test over a fully populated routing object producing byte-identical JSON to today's `Value` path, plus a test that a misspelled key is a config error rather than a silent pass-through.

## PROV-067 — The wire-api registry is an eager fn-pointer factory table where pi's laziness is per-module dynamic `import()`

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**upstream** — pi `packages/ai/src/types.ts:16-26` `KnownApi` (10 ids) and `:30` `KnownImagesApi` (`openrouter-images`); the `api/*.lazy.ts` modules perform dynamic `import()`.

**cyrup** — **zero diff on the id sets.** `lib.rs:187-201` declares exactly the same 10 (`anthropic-messages`, `openai-completions`, `openai-responses`, `azure-openai-responses`, `google-generative-ai`, `google-vertex`, `mistral-conversations`, `bedrock-converse-stream`, `pi-messages`, `openai-codex-responses`), `api/mod.rs:129-171` registers all 10, and `images/openrouter.rs` covers `openrouter-images`. What differs is the **mechanism**: pi's laziness is per-module dynamic `import()` (`api/*.lazy.ts`); cyrup's is a fn-pointer factory table with get-or-init (`api/mod.rs:80-119`).

**Impact** — None observable: same laziness, same ids, same construction points. Filed under the standing **port-mechanism fidelity** rule, which says port the literal mechanism and do not substitute an idiomatic-Rust design without explicit sign-off. Rust has no dynamic `import()`, so a factory table is very likely the right answer — but that is a *decision*, and right now it exists nowhere except in the shape of the code, so a future reader auditing this file cannot distinguish "considered and chosen" from "nobody noticed".

**Fix** — No code change proposed. Record the substitution in `api/mod.rs`'s header as a `[CYRUP-DELTA, mechanism]` naming `api/*.lazy.ts` and stating why the factory table is equivalent (nothing observable depends on module-load timing; construction is still deferred to first `get`).

**Verify** — The `[CYRUP-DELTA]` grep finds it; a test asserting `ApiRegistry` constructs nothing until the first `get`, so the "same observable laziness" claim is pinned rather than asserted.


## Coverage

**Read first-hand at cyrup HEAD `04c1ba2`** (branch `david/cyrup`, tree clean; docs HEAD `a9000b1`). In `crates/cyrup-provider`: `api/mod.rs` (`register_builtins`, `ApiRegistry::get`/`contains`), `api/compat.rs` (`ModelCompat`/`ResolvedCompat`/`ResolvedResponsesCompat` in full), `api/openai_responses.rs` (`build_params`, `build_headers`, `convert_responses_tools`, the reasoning tail, incomplete-details mapping), `api/azure_openai_responses.rs` (`build_params`), `api/openai_codex_responses.rs` (body build), `api/anthropic_messages.rs` (`build_headers`, `is_oauth` derivation), `api/google_generative_ai.rs` (tools/toolConfig, thought-signature retention, `map_stop_reason`), `api/bedrock_converse_stream.rs` (client build, send path, error body, `raw_stop_reason` producers), `validate.rs` in full, `utils/{error_body,provider_retry,overflow,estimate}.rs` in full, `stream/sse.rs` (client build, idle timeout, retry loop, error path), `collection.rs` (whole public surface + `refresh` + `AuthHelper::apply_auth`), `provider.rs`, `wire.rs` (stream dispatch), `env_api_keys.rs`, `catalog.rs`, `providers/{all,builtin_oauth,github_copilot,openai_codex,google_vertex,amazon_bedrock}.rs`, `providers/catalog_manifest.json`, all 35 `providers/catalog/*.json` (mechanically, for compat-key and api-id distribution), `auth/mod.rs`, `auth/oauth/{load,github_copilot,openai_codex}.rs`, `tests/catalog_data.rs` (in full, both tests). In `crates/cyrup-core`: `message.rs` (StopReason, AssistantMessage, DeferredHandle, both hand-written serializers). Outside the area, only to close or open items honestly: `cyrup-ext/src/{wrapper.rs:137-143,event.rs:124-125,facade.rs:588-592}`, `cyrup-ext-subagents/src/extension.rs:11290-11315`, `cyrup-session-svc/src/{attribution.rs,builder.rs:1213-1214,session.rs:1071-1090,2726-2745}`, `cyrup-tui/src/{app/execute_session.rs:147-213,status.rs:150-175}`, `cyrup/src/{diagnostics.rs:150-170,provider.rs:71-130}`, `cyrup-config/src/login.rs:360,739,784`.

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
