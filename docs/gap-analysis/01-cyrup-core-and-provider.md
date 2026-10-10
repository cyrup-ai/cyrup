# 01 — cyrup-core + cyrup-provider

This area covers `cyrup/crates/cyrup-core` (message/type model, JSONL serialization) and `cyrup/crates/cyrup-provider` (wire APIs, providers, catalogs, auth, streaming, validation), measured against `pi/packages/ai/`, `pi/packages/agent/` and the provider-facing half of `pi/packages/coding-agent/src/core/`. The ported baseline is **pi `v0.83.0`**; post-baseline drift is measured against **pi `v0.84.1`**.

> ### CLOSURES 2026-10-10 — four lows (`PROV-123`, `PROV-146`, `PROV-150`, `PROV-151`), with area 05's `CFG-102`; `PROV-150` and `PROV-151` were filed and closed in this pass
>
> On `claude/zealous-bell-x0u1h0` off `main` @ `62502ff8` (which merged #215, `52b1aa33`). Upstream read
> at pi **f1b2e77f5** through `git -C tmp/pi show` only. `git -C tmp/pi log f1b2e77f5..42a3497d0` over
> `api/simple-options.ts`, the three OpenAI-compatible adapters, `scripts/generate-models.ts` and
> `test/sampling-options.test.ts` is empty, so nothing relevant changed after the pin. **#215 landed the
> production code for `PROV-123` and `PROV-146`** (as `CFG-104`'s request half). This closure re-verified
> each against its own Verify line, added
> `prov123_each_openai_compatible_adapter_applies_model_level_params_on_a_direct_stream` and
> `prov146_resolve_yields_none_when_no_layer_applies`, and rewrote the two stale docs in
> `crates/cyrup-provider/src/tests/sampling_params.rs`. The review of that closure filed `PROV-150` and
> `PROV-151`, and the same pass then closed both. Every new or changed test was shown red with its fix
> undone (mutations recorded in the rows). Gates on the final tree, run in order:
> `cargo nextest run --workspace --features test-fixtures --no-fail-fast` (15347 tests run: 15347 passed
> (1 slow), 12 skipped; +3 over the `PROV-123`/`PROV-146` run's 15344, the `PROV-150` test and the two
> `gpt-6.1-sol` row tests), `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings`
> (clean), `cargo fmt --all -- --check` (clean), `cargo check -p cyrup-it --features it --test session_svc`
> (Finished). All four passed on the first full run. The earlier `PROV-123`/`PROV-146` run had one failure,
> `cyrup-agent tests::settlement_latch::concurrent_starts_admit_exactly_one_run` (`accepted` 2, not 1), in a
> crate this branch does not touch. It passed when re-run alone and was green in that run's full re-run.
>
> - `PROV-123` — **closed.** Its Fix ("copy through unchanged") is superseded by pi `76dfb88f6`; both sites
>   resolve now, and cyrup mirrors that.
> - `PROV-146` — **closed.** It is stored as a struct, not the Fix's `BTreeMap` (`[CYRUP-DELTA]`, Rust shape only).
> - `PROV-150` — **closed.** `AzureOpenAiResponsesOptions.reasoning_summary` (the openai-responses
>   `ReasoningSummary` type) and pi's summary-only fallback: effort `medium`, the chosen summary or `"auto"`,
>   and sampling resolved at `medium`. pi `test/sampling-options.test.ts:198-215` is ported for azure.
> - `PROV-151` — **closed.** `openai/gpt-6.1-sol` came from `gen-catalogs --only openai`. The run also
>   pulled a pi.dev tier for `gpt-daybreak-blue-latest`, which is not from `12c416e1a`; it was reverted and
>   recorded on `PROV-149`. `azure-openai-responses/gpt-6.1-sol` was added by hand from pi's Azure clone
>   rule, because pi.dev 404s that stem (`PROV-145`). Every field was re-derived from `generate-models.ts`
>   @f1b2e77f5.
>
> **Counted set (`python3 -I docs/gap-analysis/scripts/count_open_items.py`)** — base `origin/main` @
> `62502ff8`: `01  29 open (0 crit · 0 high · 1 med · 28 low), 0 trackers, 110 closed`;
> `05  7 open (0 · 0 · 0 · 7), 88 closed`; `TOTAL  135 open (0 · 0 · 5 · 130), 17 trackers, 1016 closed`.
> This tree: `01  27 open (0 · 0 · 1 · 26), 0 trackers, 114 closed`; `05  6 open (0 · 0 · 0 · 6), 89 closed`;
> `TOTAL  132 open (0 · 0 · 5 · 127), 17 trackers, 1021 closed`. Area 01 is +2 filed and −4 closed.
>
> **Lead recorded, not filed (area 06, not edited here):** `ModelCall::stream_options`
> (`crates/cyrup-ext/src/host/model_calls.rs:197-212`) does not carry the extension's `reasoning` into
> `StreamOptions`. A direct `models.stream` from an extension therefore resolves the `off` per-level entry,
> although flat model defaults do reach the request. pi's direct `stream()` also resolves at
> `options?.reasoningEffort ?? "off"`, so this may already match pi; that was not checked. Nothing filed.
>
> **Filed (2, both low, both closed above), from this closure's review:**
> - `PROV-150` (not-ported): cyrup's `AzureOpenAiResponsesOptions` had no `reasoning_summary`, so pi's Azure
>   summary-only request (`reasoning.effort` `medium`, and sampling resolved at `medium`) could not be made.
>   It was the one case of pi `test/sampling-options.test.ts:135-215` that `PROV-146` could not port.
> - `PROV-151` (stale-port): `openai.json` and `azure-openai-responses.json` lacked pi's `gpt-6.1-sol`. pi
>   `12c416e1a` adds it to `openai`, Azure and `openai-codex` (its message and
>   `test/supports-xhigh.test.ts:112-126` @f1b2e77f5); `CFG-102` regenerated only `openai-codex`.

> ### PIN 2026-10-09 — pi v1.1.0 drift triage: cyrup `6b14575` × pi **`f1b2e77f5`** (= `v1.1.0-11-gf1b2e77f5`), `packages/ai`
>
> **Window read:** `git log --no-merges v1.0.1..f1b2e77f5 -- packages/ai` = **37** commits (the lane's own
> count said 36; the verifier's 37 is the measured figure, and every commit has a disposition). Upstream read
> through `git -C tmp/pi show <commit>` / `show f1b2e77f5:<path>` only; cyrup read at `6b14575`. Nothing was run.
> The pin is an untagged commit eleven past `v1.1.0`, chosen deliberately so that post-tag fixes are in scope;
> every row cites `@f1b2e77f5` (README *CURRENT PINS* records the departure from the tag-only rule).
>
> **Filed (11):** `PROV-139` (medium), `PROV-140`…`PROV-149` (low). **Moved elsewhere:** the three v1.1.0 retry
> literals (`8b5708dbb`, `5b6c792b4`) are growth on area 12's open `DRIFT-060`, which owns `utils/retry.rs`'s
> lists by precedent and already recorded `3874b3e98`'s `model is at capacity`; that row is retitled and its
> `subscription_sharing_*` half marked closed (landed through `PROV-118`).
>
> **Read in scope and deliberately NOT filed:**
> * *Release / changelog only:* `4c6fb7cfe`, `200387122`, `997d31f28`, `28dcce2ba`, `75a99721d`, `cd32f7725`
>   (v1.0.2), `d78dc83d6` (v1.0.3), `7c10bd433` (v1.0.4), `abe508e1b` (v1.1.0); `dce4ae6f7` and `750105c80`
>   (changelog edits whose substance the filed rows carry).
> * *Tests / README only:* `311f0e020` (output-limit assertions after `27075fe07`, covered by `PROV-142`);
>   `f6127a1bf`'s `packages/ai` half (README; the coding-agent half is `EXT-110`).
> * *Type-only, no runtime behaviour:* `6b5854454`; `3ba22ce17` and `f284a2460` (how the event stream stores
>   its start; cyrup's `ResponseTimer`, `timing.rs:66-93`, already has the behaviour); `7f9e1198f`'s
>   `packages/ai` half (compat/model types moved into TypeBox schemas; a field-name diff of `types.ts`
>   before against `types.ts` + `compat-schema.ts` + `model-schema.ts` after shows no new keys).
> * *Already in cyrup:* `36a686ee8` (ported, ledger UPDATE 2026-10-09); `8d8ae2fc2` (`PROV-136`, closed);
>   `fe11328b0` (faux-provider perf only); `a2eef9eb6` (Kimi K3 cacheWrite already 0 in all three catalogs).
> * *Behaviour already as fixed:* `6b07b4e57` (cyrup never had Mistral's total deadline; reqwest
>   `read_timeout` re-arms per read, `stream/sse.rs:40`, `:141`; residual, covered by the shared
>   `[CYRUP-DELTA]`: upstream's Mistral header wait defaults to 60 s, cyrup's to the global 300 s idle value);
>   `43d376399` (radius has no embedded rows, so the overlay already replaces); `bde882c74` (a started OAuth
>   refresh runs in a detached spawn, `wire.rs:250`, `resolve.rs:199`, and always persists).
> * *Folded into filed rows:* `943a10e74`, `ce950d78f` and `f76c1db66`'s catalog half → `PROV-149`; the
>   anthropic-messages half of `f76c1db66` needs no code (pi.dev serves `claude-haiku-5-5` with full compat);
>   `ce8972a0e` → `PROV-147` and `PROV-148` (its `classifier-shared.ts` extraction is a refactor over
>   `PROV-104`'s unported apis); `76dfb88f6` → `PROV-146` (its `reasoningEffort` rewrite is
>   behaviour-equivalent); `a37306d43` → `PROV-145`.
>
> **Leads recorded, not filed:**
> * **`VL-P6` caution** (`PARITY-GAPS.md:2418`): when the auth-operation signal is ported, it must cancel only
>   the lock wait and leave the refresh bounded by the 15 s timeout alone, per `refreshStoredOAuthCredential`
>   (`auth/resolve.ts` @f1b2e77f5); otherwise porting `VL-P6` reintroduces `bde882c74`'s bug.
> * **models.json `compat` became one open object** (`7f9e1198f`): pi replaced the `compat` union
>   (OpenAICompletions | OpenAIResponses | AnthropicMessages) with a single `ProviderCompatSchema`
>   (`additionalProperties: true`, `packages/ai/src/providers/compat-schema.ts:604-607`). cyrup leaves
>   `compat` to serde under a `[CYRUP-DELTA]` (`crates/cyrup-config/src/model/validate.rs:246-249`). Whether
>   serde now rejects a mixed-arm compat block pi accepts was not checked.
>
> **Ownership questions raised (README *Decisions for the maintainers*):** the `azure` rename (`PROV-145`) is an
> area 05 migration decision, and urgent for tooling because the overlay and `gen-catalogs` already 404 on the
> old stem; `7f9e1198f`'s published JSON Schemas are area 05's (`CFG-106`) and its theme strictness area 07's
> (`TUI-178`).

> ### CLOSURES 2026-10-09 — one low (`PROV-134`), with `SEAM-128` (area 08) and `CFG-085` (area 05) in one PR
>
> The three rows are one feature and none of their Verify lines can be met alone, so they closed
> together in five commits on `claude/hopeful-dirac-squ75k`. **Gates, measured on the combined tree
> rather than per lane:** `cargo nextest run --workspace --features test-fixtures` (the canonical
> command, `README.md:28`) → **14903 passed · 0 failed · 12 skipped**, against a measured
> `main`/`515a0d0a` baseline of 14868 passed · 0 failed · 12 skipped;
> `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all` clean. Every new
> test was shown red before its fix.
>
> **`PROV-134` — Verify, quoted in full:** *"a catalog row carrying `inputLimits` round-trips it
> through parse, store and overlay; an image request against a model whose row sets `images.resize`
> honours it; a row without the field still parses."* Clause by clause:
>
> - *parse* — `cyrup-provider tests::input_limits::a_text_only_rows_declared_limits_survive_the_parse_untouched`
>   and `::the_image_and_classifier_wire_mirrors_both_copy_the_field`. The mirrors are the clause
>   that matters: `ImageModelWire`/`ClassifierModelWire` exist only to make `type` mandatory, so a
>   field on the public struct but absent from the mirror compiles, serializes correctly and reads
>   back `None`.
> - *store* — `cyrup-config tests::models_store_images::an_input_limits_profile_survives_a_restart_on_every_type`,
>   added by this closure because nothing covered the store leg: it writes a full four-key profile on
>   a chat, an image AND a classifier row, asserts the camelCase keys are really on disk, and asserts
>   all three come back through `FileModelsStore`. Red-proved three ways — neutering the
>   `ClassifierModelWire` copy, the `ImageModelWire` copy, and the chat `Model`'s serde each failed it
>   on that leg alone while the other two assertions still passed.
> - *overlay* — `::the_live_overlay_body_keeps_its_profile_on_chat_and_image_rows_alike` and
>   `::an_overlay_row_served_without_the_field_is_stamped_on_all_three_variants`.
> - *an image request honours `images.resize`* — the consumers, which are the paths by which images
>   ENTER a provider request: `cyrup-session-svc tests::prompt_image_normalization::*` (9) and
>   `tests::read_model_resize::*` (2). **This clause does NOT mean `generate_images`** — see the
>   correction in the row.
> - *a row without the field still parses* — `::a_row_with_no_input_limits_still_parses_weak_guard`,
>   reported as the weak guard it is: it would pass with the feature deleted, and earns its keep only
>   against a non-`Option` field or a missing `#[serde(default)]`.
>
> **`maxRequestBytes`, `images.maxPerMessage` and `images.maxPerRequest` are modelled, round-tripped
> and deliberately inert, and that is parity, not cyrup lagging.** Upstream declares them
> (`types.ts:1087-1094`), generates them (`generate-models.ts:1000-1008`), schema-validates them
> (`model-config.ts:161-166`) and asserts them in its own tests, and has no runtime reader for any of
> the three: `packages/coding-agent/docs/models.md:89` says outright that pi "does not yet rewrite or
> reject history based on them". Only `images.resize` has a consumer, on either side. A cyrup-side
> enforcement would be an invented surface. This is recorded so a later pass reads the absence as
> intended rather than as an unwired field.

> ### CLOSURES 2026-09-28 — twelve lows (`PROV-070`, `PROV-072`, `PROV-076`, `PROV-077`, `PROV-079`, `PROV-081`, `PROV-086`, `PROV-089`, `PROV-093`, `PROV-094`, `PROV-095`, `PROV-100`); no partials
>
> Landed on `claude/lows-next`, not yet committed. Each row and body section carries its evidence. Every
> closure has a test that the lane showed red without its fix. **The combined tree has not passed the gates.**
> The integration pass ran only `cargo fmt --all -- --check`, which passed and changed no files. Clippy and
> `cargo nextest` were not run on the combined diff, because `/` had 2.9 GB free, below the pass's 4 GB floor
> (`target/` is 24 GB). Until they run, treat "green" as per-lane only. **Counted set after this pass
> (`count_open_items.py`): 0 critical · 0 high · 5 medium · 4 low = 9 open, 81 closed** (was 5 medium ·
> 16 low = 21 open, 69 closed). The nine provider-wire rows are listed below; `PROV-070`, `PROV-072` and
> `PROV-089` were closed by a separate lane in the same batch, and their evidence is in their rows. By row:
>
> - `PROV-076` — Kimi's top-level `usage.cached_tokens` is the third cache-read fallback in `parse_usage`.
> - `PROV-077` — Anthropic OAuth sends `claude-cli/2.1.280`; the test asserts the exact value.
> - `PROV-079` — openai-completions drops empty text parts from a user content array.
> - `PROV-081` — Bedrock fills `cache_write_1h` from `cacheDetails` `ttl:"1h"`, so one-hour writes are priced right.
> - `PROV-086` — OpenRouter sends `x-session-id` on completions and on anthropic-messages. **Ledger correction:**
>   the Anthropic half did not need `PROV-099` and was ported now; `PROV-099` stays open and untouched.
> - `PROV-093` — `supportsMaxOutputTokens` gates `max_output_tokens`; explicit-mode models get `ttl:"30m"`, not
>   `"24h"`. **Ledger correction:** `supportsExplicitPromptCacheMode` already existed (`PROV-023`); catalog flags
>   stay with `PROV-071`.
> - `PROV-094` — both Responses adapters send `tool_choice`, with a forced function in the Responses shape.
> - `PROV-095` — the seven pi-parity adapters send `cyrup (<platform> <release>; <arch>)` beneath every overlay.
>   **Ledger correction:** Codex was an eighth `getPiUserAgent` site at v0.87.1; it now uses the same builder.
>   Windows still omits the release.
> - `PROV-100` — `thinkingTokenBudgetField`, `supportsThinkingTokenBudget`, `vllmPriority` and
>   `$var:thinking.budget` ported. **Ledger correction:** the row missed `supportsThinkingTokenBudget`.
>
> Area 05 carries a one-line dated note, because its 2026-09-24 resolution named `vllmPriority` and
> `supportsMaxOutputTokens` as unported keys.
>
> **The separate batch, same date, same gate status (fmt only; per-lane green):** `PROV-070`, `PROV-072` and
> `PROV-089`. One exception to "every closure has a test that the lane showed red": `PROV-072` is a deletion
> and adds no test. By row:
>
> - `PROV-070` — **reclassified `cyrup-original` → converged `upstream-drift`: the row's own falsifier fired.**
>   `pi.dev/api/models/providers/together` (re-fetched 2026-09-28, 22 rows) serves `moonshotai/Kimi-K3`, so
>   `providers/together.rs:264-274` now carries pi's row field for field (`1_048_576` context, not the measured
>   `1_000_000`; the three-null `thinkingLevelMap`, so the ladder is `off`/`high`). The `ADDITIONS` guard is
>   gone; `full_catalog_ported_from_pi` asserts 21 rows, `kimi_k3_is_pi_s_served_row` pins K3, and
>   `xtask/src/main.rs:110-112` says 21. The rest of pi.dev's Together roster drift is `PROV-071`'s; K3's
>   `inputLimits` is area 05 `CFG-085`'s.
> - `PROV-072` — **closed by deletion.** `auth/oauth/load.rs`, its re-exports, `OAuthError::FlowUnavailable` and
>   the registry-shaped Copilot/Kimi factories are gone; `RadiusOptions` is in `auth/oauth/radius.rs:87-93` as
>   the port of `RadiusOAuthOptions`, and `providers/builtin_oauth.rs:25-37` records why `load.ts` has no port.
>   No removed symbol is referenced in `crates/` or `xtask/`. Area 12 `DRIFT-019` carries a one-line dated note,
>   because it cites `auth/oauth/load.rs:59`.
> - `PROV-089` — `xtask/src/main.rs:97` pins `IMAGES_REV = "v0.87.1"` for `openrouter-images` only, whatever
>   `--rev` says; the catalog is 55 rows, JSON-identical to `IMAGE_MODELS.openrouter` @v0.87.1, and the manifest
>   records `pi@v0.87.1`. **Ledger correction:** one existing MAI row was renamed, not four. `IMAGES_REV` must
>   stay `v0.87.1` after the next pi tag, which deletes the source file.

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


> ### PROVENANCE CORRECTION — 2026-09-14. The pins below are revised; **this file was not re-read.**
>
> Everything above this block is history and is correct as written. The passes it narrates were
> audited at **pi `v0.83.0` (ported baseline) and pi `v0.84.1` (drift target)**; the newest
> whole-file pass is **2026-09-04 at cyrup `2571969`**, with per-item closure stamps running to
> 2026-09-05 against the `824a539e`-era tree. Every citation, closure and severity below still means
> exactly what it meant at those pins, and **nothing in this block re-verifies any of it. No item was
> re-read, no row was re-derived, no count, severity or status changed.** This block states only how
> old the file is.
>
> | | audited at (history — do not rewrite) | current pin (authoritative, per `README.md`'s baselines table) | window this file has never measured |
> |---|---|---|---|
> | `pi` | `v0.83.0` ported / **`v0.84.1`** drift | **`v0.85.1`** | `v0.84.1..v0.85.1` — `packages/ai` **94 files, +7 355 / −948** (103 non-merge commits on that path; 711 repo-wide), releases v0.84.2 · v0.84.3 · v0.84.4 · v0.85.0 · v0.85.1. The provider-facing half of `packages/coding-agent` is a further **338 files, +25 297 / −5 625** |
> | `cyrup` | `2571969` (2026-09-04) | **`b28d3ff`** — ledger's last recorded code baseline is `824a539e` | `2571969..b28d3ff` = **1 506 files, +219 934 / −41 853**, 48 non-merge commits under `crates/`+`xtask`. From the ledger's own baseline, `824a539e..b28d3ff` = **453 files, +98 509 / −15 880**, 31 commits. (`9aeba769..b28d3ff` is docs-only, so the code window ends at `9aeba769`) |
> | `pi-permission-system` | `v0.8.0` | `v0.8.0` — re-checked 2026-09-14, unchanged | none |
> | `pi-intercom` | `v0.10.1` as the header above records it | **`v0.13.0`** — re-checked 2026-09-14, still newest | out of this area's scope |
> | `pi-acp` | — | `v0.0.33` — re-checked 2026-09-14, unchanged | area 15's |
> | `pi-subagents` · `pi-mcp-adapter` · `code_puppy_core_plugins` | — | `v0.67.0` · `v2.33.0` · `v0.0.50` (ported surface byte-identical across all 39 tags) | areas 09/09a · 13 · 14 |
>
> **Read every row below as one pi minor release and 1 506 changed cyrup files out of date.** A row
> that reads `upstream-drift`, `missing` or `CLOSED` has not been tested against either newer side;
> a closure resting on a `v0.84.1` premise may have had its premise moved out from under it (see
> `PROV-054` in the census section immediately following). Re-derive both sides before quoting any
> row, per `README.md` → *Working an item*.

> ### RE-MEASURE — 2026-09-24. **cyrup `ea23ca2`, pi `v0.87.1`.** This block supersedes the pin table immediately above.
>
> | | measured at | window read this pass | window still unread |
> |---|---|---|---|
> | `cyrup` | **`ea23ca2`** (2026-09-24) | `9aeba769..ea23ca2` — 46 commits, and **zero bytes changed in this area's crates**: `git diff --quiet 9aeba769 ea23ca2 -- crates/cyrup-provider crates/cyrup-core crates/cyrup-config crates/cyrup-agent crates/cyrup-session crates/cyrup-modes crates/cyrup-resources crates/cyrup-tools xtask` exits 0. The window's code landed in `cyrup-ext-subagents`, the new `cyrup-herdr`, `cyrup-it`, `cyrup-tui`, `cyrup-ext`, `cyrup-session-svc` and `crates/cyrup` (none of it provider-facing; `git grep 'refresh_models\|RadiusProviderOptions\|radius_provider_with' -- crates ':!crates/cyrup-provider'` is still empty) | — |
> | `pi` `packages/ai` | **`v0.87.1`** (2026-09-22) | `v0.85.1..v0.87.1` — `packages/ai` **127 files, +4 910 / −2 058**, 67 non-merge commits (releases v0.86.0 · v0.86.1 · v0.87.0 · v0.87.1). Read: `CHANGELOG.md` in full; source diffs of `utils/{retry,overflow}.ts`, `providers/{all,meta,opencode,opencode-go,opencode-headers,radius}.ts`, `auth/oauth/{meta,load}.ts`, `env-api-keys.ts`, `api/openai-completions.ts` (the two v0.86.1..v0.87.1 commits), `api/bedrock-converse-stream.ts`, the `claudeCodeVersion` constant, `utils/transcript.ts` (head only). **Plus five `v0.84.1..v0.85.1` leads from the census below, promoted after both sides were re-read at this pass's pins** | `v0.85.1..v0.87.1`: `api/anthropic-messages.ts` (273 lines changed), `api/openai-responses*.ts`, `api/openai-codex-responses.ts`, `api/google-*.ts` beyond the one fix filed, `api/mistral-conversations.ts`, `types.ts` (+219), `utils/{estimate,event-stream,text}.ts`, `scripts/generate-models.ts` (+705 lines) — see `### 2026-09-24 census note` below. ~~`v0.84.1..v0.85.1`: every census lead below not promoted is still UNVERIFIED~~ — **both halves of this cell were read by the SECOND PASS below; nothing remains** |
>
> **Every open row was re-read against both sides at these pins; none closed** — the cyrup side of each is byte-identical to its 2026-09-14/16 reading, and each upstream falsifier was re-run at v0.87.1 (stamps in the rows). **Ten items filed, `PROV-073` … `PROV-082`**, bodies under `## Findings filed 2026-09-24`. Upstream is cited only as `git -C tmp/pi show <tag>:<path>`.
>
> **SECOND PASS, 2026-09-24 (same pins) — the "window still unread" cell above is now CLOSED.** Read in
> full: `api/anthropic-messages.ts`, `api/openai-responses.ts`, `api/openai-responses-shared.ts`,
> `api/azure-openai-responses.ts`, `api/openai-codex-responses.ts`, `types.ts`, `utils/transcript.ts`,
> `utils/{text,estimate,event-stream}.ts`, `api/{transform-messages,simple-options,pi-messages}.ts`,
> `models.ts`, `compat.ts`, `index.ts`, every `api/google-*.ts` and `api/mistral-conversations.ts` commit in
> the window, the two `api/openai-completions.ts` commits the first pass skipped, the whole
> `scripts/generate-models.ts` diff, `image-models.generated.ts` and `models.generated.ts`. The first pass's
> `bedrock-converse-stream.ts` read was re-used, and its census leads were re-read on both sides. **Every
> 2026-09-14 lead and every 2026-09-24 census lead is dispositioned** (tables in the two census sections).
> **Eighteen items filed, `PROV-083` … `PROV-100`**; `PROV-040`, `PROV-071`, `PROV-078` amended with growth;
> closed `PROV-025`/`PROV-S04` annotated (their upstream targets were deleted in v0.86.0); `PROV-054`'s
> closure re-audited and upheld. No item closed. **Still unread in `packages/ai` v0.84.1..v0.87.1: nothing
> port-relevant** — `test/**` was read only as evidence for specific items, and `CHANGELOG.md`/`README.md`
> are not port surfaces. The pi.dev catalog DATA (gitignored at every tag) remains unreadable from git by
> construction; that is `PROV-071`'s standing limitation, not an unread window. Post-tag commits
> `v0.87.1..b45597504` touching `packages/ai` are recorded as leads under the 2026-09-24 census note.

## RESOLVED — 2026-09-14 census of the `v0.84.1..v0.85.1` window (historical leads, every one dispositioned 2026-09-24)

> **2026-09-24, second pass — EVERY lead in this section is now resolved; none remains unverified.**
> Each was re-read on both sides (cyrup `ea23ca2`, pi `v0.87.1` via `git show`), so the bullets below are
> kept as the historical census, not as open work. Dispositions:
>
> | lead | disposition |
> |---|---|
> | Anthropic: beta namespace / body `betas[]` | **Struck as a mechanism difference that costs no behaviour** — cyrup's header overlay reproduces pi's rules exactly: a caller `anthropic-beta` header replaces the computed set and a `None` suppresses it (`api/anthropic_messages/headers.rs`, overlay merged last). The one real difference, the interleaved-thinking gate, is folded into `PROV-091` |
> | Anthropic: server-side refusal fallback | **→ `PROV-090`** |
> | Anthropic: mid-conversation per-turn effort | **→ `PROV-091`** |
> | Anthropic: `claudeCodeVersion` | `PROV-077` (first pass) |
> | Responses: `supportsMaxOutputTokens` · GPT-5.6 `ttl:"30m"` | **→ `PROV-093`** (one item, one code block) |
> | Responses: `supportsAdditionalTools` | **→ `PROV-083`** — upstream re-anchored it at mid-conversation system messages in v0.86.0, so it is no longer a separate compat-key gap |
> | Codex SSE EOF | **→ `PROV-084`**, which is wider: cyrup's shared framer also drops the trailing Anthropic event pi has flushed since before v0.83.0 |
> | Completions: `reasoning_details` rewrite | **→ `PROV-092`** |
> | Completions: `thinkingTokenBudgetField` + `$var`, `vllmPriority` | **→ `PROV-100`** (the `$var` half is resolved: cyrup mis-resolves it as an effort) |
> | Completions: DeepSeek · Kimi `cached_tokens` | `PROV-074` · `PROV-076` (first pass) |
> | Google: `thinkingLevelMap` / `resolveGoogleThinkingLevel` | **→ `PROV-087`** (merged with v0.86.0 #9455); finish-reason half is `PROV-075` |
> | Bedrock: redacted reasoning · empty-key sanitize · response headers | **→ `PROV-097`** — the "(b) is a lead" half was read: `convert.rs` replays `tc.arguments` verbatim |
> | Mistral tool-call key | `PROV-073` (first pass) |
> | `pi` default `User-Agent` | **→ `PROV-095`** |
> | `NO_PROXY` subdomains / IPv6 / case | **→ `PROV-096`** |
> | Retry: eighth literal | **→ growth on `DRIFT-057`** (area 12, same list, same fix). The `errorMessage` deletion half is **struck — already ported**: `utils/retry.rs` sets `error_message = None` on a backoff abort |
> | `normalizeOptionalNulls` | **Struck — already ported**: `crates/cyrup-provider/src/validate.rs::normalize_optional_nulls` removes a `null` optional whose schema rejects null |
> | `toolChoice` on `SimpleStreamOptions` | **Struck — refuted**: `utils/simple_options.rs:127` forwards `base.tool_choice`. Reading it found a different, real gap → **`PROV-094`** |
> | Copilot rate-limited discovery / selective policies | **→ `PROV-098`** |
> | OpenRouter `anthropic-messages` route | **→ `PROV-099`**. xAI half: **resolved, no gap** — all three `xai.json` rows are `openai-responses` |
> | `Models.streamDeferred()` | **→ growth on `PROV-040`** (row amended) |
> | Generator rewrite | **→ growth on `PROV-071`** (row amended with the v0.87.1 generator deltas) and `PROV-083`/`PROV-090`/`PROV-091` for the behaviour it feeds |
> | cyrup-side: XAI_1 manifest · `CatalogOverlaySlot` · `preserve_order` | **Struck as leads** — cyrup-only surfaces with no upstream claim to test; `PROV-071`'s row already records XAI_1 as shipped state, and the overlay slot is a fix with no residual behaviour gap found |
> | `PROV-054` supersession flag | **Resolved — closure holds**: every embedded `xai` row is `openai-responses`, from the live pi.dev fetch |
> | `PROV-040` / `PROV-060` / adjacency notes | bookkeeping, applied |

> **2026-09-24 — five leads below are now items**, re-read on both sides at cyrup `ea23ca2` / pi `v0.87.1`: the Mistral tool-call key → `PROV-073`; DeepSeek `useMaxTokens` → `PROV-074`; the Google non-STOP finish-reason override → `PROV-075`; Kimi top-level `cached_tokens` → `PROV-076`; `claudeCodeVersion` → `PROV-077` (which has moved again, to `2.1.280`, at v0.87.1). The second pass the same day dispositioned every remaining lead — see the table immediately below.


**Nothing in this section is an item.** No `PROV-`/`DRIFT-` id is assigned, because id assignment
belongs to a pass that read both sides and this one did not read both sides everywhere. No row in
`## Open items` is opened, closed, re-ranked or otherwise touched by anything here. This is a
worklist for the next pass, grouped by surface; each entry states what was read on which side, and
where only one side was read it says so in those words.

Census method: upstream read only via `git -C tmp/pi show v0.85.1:<path>` and
`git diff v0.84.1..v0.85.1 -- <path>`, covering every non-test source file in the `packages/ai/src/`
window plus `CHANGELOG.md`, `scripts/generate-models.ts` and `scripts/openrouter-reasoning-options.ts`;
cyrup read at `b28d3ff` across `crates/cyrup-provider/src/{api,utils,providers,auth}`,
`crates/cyrup-core/src/message/` and `xtask/src/main.rs`. **Caveat carried from the censusing pass:
every cyrup citation here must be re-derived before an item is filed from it** — the port moves
faster than this directory does.

### Anthropic Messages

- **Messages moves to the SDK Beta namespace; `anthropic-beta` becomes a request-body `betas[]`** · M
  — upstream `api/anthropic-messages.ts:580` (`client.beta.messages.create`), `:978` `getBetaFeatures()`,
  `:177-179` three new beta ids (`server-side-fallback-2026-07-01`, `mid-conversation-output-config-2026-07-01`,
  `thinking-binding-controls-2026-08-01`), imports at `:1-14` @v0.85.1. Betas are no longer baked into
  `defaultHeaders` at client construction: `getBetaFeatures(model, context, isOAuthToken, options)`
  computes them per request; an `anthropic-beta` entry in `model.headers`/`options.headers` REPLACES
  the computed set and an explicit `null` yields `[]`; interleaved-thinking is newly gated on
  `model.reasoning && options.thinkingEnabled === true`. cyrup builds a joined `anthropic-beta`
  HEADER at `crates/cyrup-provider/src/api/anthropic_messages/headers.rs:97`,`:109` with constants at
  `:16-17` — the pre-v0.85.0 shape. Both sides read. **Lead only** on whether the URL/path also needs
  the beta change and whether the interleaved gate is reachable in cyrup's call graph.
- **Server-side refusal fallback (`fallbacks`, `allowedFallbackModels`, fallback-model repricing)** · M
  — upstream `types.ts:307-311`,`:723`; `api/anthropic-messages.ts:1169-1172`, `:596-605` (message_start
  rewrites `output.model` and picks `usageModel` from the fallback's cost), `:616`+`:778`
  (`calculateCost(usageModel, …)`), `:597-601` (a `fallback` content block after output has started is
  a hard error). Landed v0.84.3 (#8017, #8285). cyrup has no `allowed_fallback_models` anywhere
  (`grep -rn allowed_fallback_models crates/` empty); the only `fallbacks` hit in cyrup-provider is
  OpenRouter's unrelated `allow_fallbacks` (`api/compat.rs:241`). Both sides read. Cost attribution is
  the user-visible half: a fallback response would be billed at the requested model's rate.
- **Mid-conversation per-turn effort: `supportsMidConvoEffort`, `providerThinkingLevel`, synthetic
  `system` effort messages, `block_binding.prefix_mismatch_behavior`** · L — upstream `types.ts:716`,
  `:437`; `api/anthropic-messages.ts:513`, `:1121-1128` (forces `thinking:{type:"adaptive",
  block_binding:{prefix_mismatch_behavior:"drop_block"}}` + `output_config.effort`), `:1404-1419`
  `insertThinkingLevelMessages()`, `:1333-1340`. v0.85.0: every historical assistant turn that recorded
  its own `providerThinkingLevel` gets a preceding `{role:"system", content:[], output_config:{effort}}`
  message injected, one more is appended for the active effort, temperature is suppressed. The
  generator enables it only for providers `anthropic` and `openrouter` and merges
  `thinkingLevelMap:{off:null}`. cyrup's `AssistantMessage` (`crates/cyrup-core/src/message/assistant.rs:30-90`)
  has no `provider_thinking_level`; `api/compat.rs:296` has no `supports_mid_convo_effort`. Both sides
  read. **This is a transcript-shape change as well as a persisted-field change** — it crosses
  cyrup-core and cyrup-session; recorded here because the wire behaviour is the provider's.
- **`claudeCodeVersion` 2.1.75 → 2.1.251 on the OAuth `user-agent`** · S — upstream
  `api/anthropic-messages.ts:81` @v0.85.1. cyrup pins the old value at
  `api/anthropic_messages/headers.rs:20`, consumed at `:112`. Both sides read. Trivial, but it is a
  live header on every OAuth Anthropic request.

### OpenAI Responses and Codex

- **`OpenAIResponsesCompat.supportsMaxOutputTokens`** · S — upstream `types.ts:664`;
  `api/openai-responses.ts:78` (defaults `true`), `:314`. v0.85.0 (#8941): a compat gate so
  Codex-protocol gateways that reject `max_output_tokens` can suppress the parameter. cyrup emits it
  unconditionally whenever `max_tokens > 0` (`api/openai_responses/params.rs:130-135`) and neither
  `ModelCompat` (`api/compat.rs:296`) nor `ResolvedResponsesCompat` (`:449`) carries the key. Both
  sides read. **Same code block as the open `PROV-019`** (`max_output_tokens` floor of 16) — schedule
  together.
- **GPT-5.6+ long prompt cache: `prompt_cache_retention:"24h"` → `prompt_cache_options.ttl:"30m"`** · S
  — upstream `api/openai-responses.ts:83-89` (`getPromptCacheRetention` suppresses `"24h"` under
  `supportsExplicitPromptCacheMode`), `:91-99` new `getPromptCacheOptions()`, `:310`. v0.85.1; the two
  keys became mutually exclusive. cyrup emits `"24h"` on the `Long` branch regardless of the flag and
  has no `ttl` arm (`api/openai_responses/params.rs:112-126`). Both sides read. **Same block as the
  open `PROV-023`.**
- **Message-anchored `additional_tools` deferred-tool mode (`supportsAdditionalTools`)** · M — upstream
  `types.ts:658`; `api/openai-responses.ts:285-296` (three-way `deferredToolsMode`),
  `api/openai-responses-shared.ts:123`,`:321-327` (`{type:"additional_tools", role:"developer", tools}`
  input item), `api/openai-codex-responses.ts:531-537`. v0.84.2 (#7709): a capable model gets the
  developer input item anchored at the tool result instead of the `tool_search_call`/`tool_search_output`
  pair; tool-search remains the fallback. cyrup's `ModelCompat` has `supports_tool_search` but no
  `supports_additional_tools` (`api/compat.rs:296`, `:449-470`). Both sides read **on the compat
  surface only** — cyrup's Responses deferred-tool emitter was NOT read, so the emitter half is
  unverified.
- **Codex SSE: terminal events not followed by a blank line are now processed** · S — upstream
  `api/openai-codex-responses.ts:784-788`, `:813` (`if (done) break` moved AFTER frame draining).
  v0.85.0 (#9047): EOF now flushes the decoder, synthesizes a frame terminator and drains before
  breaking. **UPSTREAM SIDE ONLY** — cyrup's Codex SSE splitter was not located by name
  (`grep -rn 'fn parse_sse' …/openai_codex_responses/` is empty; the crate has a shared
  `stream/sse.rs`) and its EOF handling was not read. Lead, not a defect claim — and worth checking
  because a shared reader would make the same truncation wider here than it was upstream.

### OpenAI-compatible completions

- **`reasoning_details` replay rewritten: three variants, delta merging, thinking-block anchoring** · M
  — upstream `api/openai-completions.ts:203-228`, `:264-278` `appendOpenAIReasoningDetail`, `:337-343`,
  `:660-670`, `:1280-1291` (incl. `parseLegacyEncryptedReasoningDetail`), `:1352-1354`. v0.84.3/v0.84.4
  (#7994, #8246, #8605): all three variants (`reasoning.text`/`.summary`/`.encrypted`) are recognised,
  consecutive text/summary deltas are CONCATENATED, the serialized array is stored once on the THINKING
  block's `thinkingSignature`, replay prefers it and falls back to the legacy per-tool-call encrypted
  form; raw-reasoning replay is restricted to `reasoning`/`reasoning_content`/`reasoning_text`. cyrup
  implements the OLD shape: `api/openai_completions/decode.rs:303-304` cites "Pi `reasoning_details`
  handling, L422-435" (pre-rewrite lines) and `convert.rs:267-289` rebuilds the array by JSON-parsing
  each tool call's `thought_signature`. Both sides read. Consequence is lost/garbled reasoning replay
  on OpenRouter.
- **`thinkingTokenBudgetField` and the `{"$var":"thinking.budget"}` chat-template variable** · M —
  upstream `types.ts:74`,`:92`,`:613-621`; `api/openai-completions.ts:863`,`:969-971`,`:997-1003`,
  `:1005-1016`,`:1054-1056`; `api/simple-options.ts:57-62`,`:64-70`,`:72-75`. v0.84.3 (#8275)
  generalized the vLLM-only `supportsThinkingTokenBudget` boolean into a field-name enum, hoisted the
  default budget table and answer-room clamp into `simple-options.ts`, and exposed the computed budget
  to `chat_template_kwargs`. cyrup has neither the new field nor the older boolean, and the boolean is
  also absent from pi at the ported `v0.83.0` — so this is **pure post-baseline drift, not a port
  bug**. Both sides read on the compat surface; cyrup's `chat_template_kwargs` resolver was not read,
  so the `$var` half is a lead.
- **`vllmPriority` compat flag → top-level `priority` request field** · S — upstream `types.ts:636-642`;
  `api/openai-completions.ts:859-861`, `:185`/`:191`, `:1715` (passthrough, **not** auto-detected).
  v0.85.0 (#9004), off by default and never set by the generated catalog. Absent from cyrup's
  `ModelCompat` (`api/compat.rs:296`) and from `api/openai_completions/params.rs`. Both sides read. Low
  blast radius, but it is one of the compat-flag family `PROV-023`/`024`/`033`/`034` showed is where
  silent wire divergence lives.
- **DeepSeek detection: case-insensitive base URL, and DeepSeek added to `useMaxTokens`** · S —
  upstream `api/openai-completions.ts:1598`, `:1619`, `:1608`. v0.84.2 (#7933 + "send max_tokens to
  DeepSeek APIs") hoisted `isDeepSeek` above `isNonStandard`, lowercased the hostname test, and made
  DeepSeek send `max_tokens` rather than `max_completion_tokens`. cyrup's `detect_compat` takes
  `base_url` verbatim with no lowering (`api/compat.rs:584-586`), tests `contains("deepseek.com")` at
  `:614`/`:640`, and its `use_max_tokens` at `:631-637` does NOT include `is_deepseek`. Both sides
  read. **Same failure family and same expression as the closed `DRIFT-013`** (Z.AI's dropped `isZai`)
  — a known-recurring site.
- **Kimi top-level `usage.cached_tokens` counted as cache reads** · S — upstream
  `api/openai-completions.ts:1511`,`:1519-1520` (third fallback after `prompt_tokens_details.cached_tokens`
  and `prompt_cache_hit_tokens`), v0.84.3 (#8075). cyrup's chain stops after `prompt_cache_hit_tokens`
  (`api/openai_completions/finalize.rs:47-50`). Both sides read. Consequence is mis-costed Kimi turns.

### Google, Bedrock, Mistral

- **Google: `thinkingLevelMap` honoured for custom models; a non-STOP finish reason no longer becomes
  `toolUse`** · M — upstream `api/google-shared.ts:31-51` new `resolveGoogleThinkingLevel()` (throws on
  an unmappable value) plus the `GoogleThinkingLevel`→`GoogleApiThinkingLevel`/`ResolvedGoogleThinkingLevel`
  rename; `api/google-generative-ai.ts:219` and `api/google-vertex.ts:236` add
  `&& output.stopReason === "stop"` to the tool-call override; `convertTools` gained
  `supportsStrictMode`; `index.ts:13` export rename. v0.84.2/v0.84.3 (#8059, #8135). cyrup rewrites to
  `ToolUse` unconditionally whenever a tool call is present **and additionally clears `error_message`**
  (`api/google_generative_ai/parts.rs:65-70`) — the pre-fix shape plus an extra cyrup behaviour;
  `grep -rn resolve_google_thinking_level crates/` is empty. Both sides read for the map and the
  finish-reason halves. **Sits next to the open `DRIFT-048`** (Google tool-call-id rule), same file
  family.
- **Bedrock: redacted reasoning round-trip, empty-tool-argument-key sanitization, raw response headers** · M
  — upstream `api/bedrock-converse-stream.ts:641-657` (`delta.reasoningContent.redactedContent`
  buffering + `[Reasoning redacted]` placeholder), `:991-996` (replay), `:900-912`
  `sanitizeBedrockDocument`, `:493-510` `addResponseHeadersMiddleware`. v0.84.2/v0.84.3 (#8314, #7882,
  #8234). cyrup: (a) sets `redacted: false` unconditionally (`api/bedrock_converse_stream/blocks.rs:148`)
  and `grep -rn redactedContent crates/` is empty; (c) builds a one-key header map
  (`…/driver.rs:224-236`) — pi's pre-fix behaviour, and notable because cyrup uses a native reqwest
  transport and **already has the full header map in hand at `:210`**; (b) no sanitizer
  (`grep` = 0 hits) but cyrup's tool-argument replay path was NOT read, so (b) is a lead.
- **Mistral streaming tool-call key `{id}:{index}` → `index ?? callId`** · S — upstream
  `api/mistral-conversations.ts:692-696` @v0.85.1, v0.84.4 (#8387). Previously a continuation chunk
  that omitted the id derived a different synthetic id and opened a second block, splitting the
  arguments. cyrup builds the composite key at `api/mistral_conversations/blocks.rs:26-35` with the
  synthetic id derived exactly as pi's old code did at `:33`. Both sides read — this is the pre-fix
  shape, so fragmented Mistral tool calls should split here as they did upstream before v0.84.4.

### Transport, retry, proxy, validation

- **`pi` default `User-Agent` on seven adapters (new `utils/pi-user-agent.ts`)** · S — upstream
  `utils/pi-user-agent.ts:1-19` @v0.85.1 (new), applied at `api/openai-completions.ts:756`,
  `api/openai-responses.ts:240`, `api/azure-openai-responses.ts:257`, `api/anthropic-messages.ts:288`,
  `api/google-generative-ai.ts:348`, `api/google-vertex.ts:395`, and Mistral's `buildMistralHeaders`.
  v0.84.3 (#8305): `User-Agent: pi (<platform> <release>; <arch>)` unless overridden. In cyrup only the
  Codex adapter builds such a UA (`api/openai_codex_responses/headers.rs:95`,`:122`) plus a separate
  catalog-fetch UA (`remote_catalog.rs:88`); no default UA on the other six. Both sides read.
- **`NO_PROXY` matches subdomains of a bare domain entry; IPv6-bracket and port parsing** · S —
  upstream `utils/node-http-proxy.ts:41-70` new `parseNoProxyEntry()`, `:37-39` `stripBrackets()`,
  `:83-116` rewritten `shouldProxyHostname` (exact match AND `endsWith("."+domain)` at `:110`), `:125`.
  v0.85.0 (#8737). cyrup carries the OLD algorithm literally, including the regex comment
  (`utils/node_http_proxy.rs:102-123`, exact-host branch at `:117`) and does not lowercase the
  hostname. Both sides read. A corporate `NO_PROXY=internal.corp` now behaves differently upstream.
- **Retry: a new eighth retryable phrase, and an aborted retry DELETES `errorMessage`** · S — upstream
  `utils/retry.ts:44` (`"exceeded request buffer limit while retrying upstream"`) and `:205-207`
  (`const { errorMessage: _e, ...rest } = response`), v0.84.2. cyrup's pattern list carries
  `"provider.?returned.?error"` at `utils/retry.rs:72` but no buffer-limit phrase (grep = 0 hits).
  Both sides read for the pattern; the key-deletion half is a serde `skip_serializing_if` question NOT
  chased in cyrup. **`DRIFT-014` covered the older seven literals; this is a new eighth.**
- **`validateToolArguments` drops `null` for optional non-nullable properties before coercion** · S —
  upstream `utils/validation.ts:240-269` new `normalizeOptionalNulls()`, called at `:319`, v0.84.2.
  Strict schemas wrap optionals in `anyOf:[T,null]`, so models now emit explicit `null` for omitted
  optionals and this recursively deletes those keys. cyrup has the strict-schema half
  (`utils/constrained_sampling.rs:217`,`:233`) but **the cyrup side was UNVERIFIED at the census** *(resolved 2026-09-24: `validate.rs::normalize_optional_nulls` exists — already ported)* — no
  `normalize_optional_nulls` counterpart was located and cyrup's tool-argument validator was not read.
  Filed because the two halves shipped as one change. Adjacent to open `PROV-016`/`PROV-046`.

- **`toolChoice` becomes a provider-neutral `SimpleStreamOptions` field across every adapter** · M —
  upstream `types.ts:82` (`export type ToolChoice = "auto" | "none"`) and `:314-316`, threaded through
  `api/openai-completions.ts:733-737`, `api/openai-responses.ts:218-221`,
  `api/azure-openai-responses.ts:173-176`+`:311-313`, `api/anthropic-messages.ts:856-859`,
  `api/google-generative-ai.ts:307-310`, `api/google-vertex.ts:319-322`,
  `api/bedrock-converse-stream.ts:511-514`, `api/pi-messages.ts:438`. v0.84.3 added the neutral field
  so every `streamSimple` forwards it into the provider-specific options instead of each adapter
  digging it out of a cast; v0.84.3 also fixed Azure Responses ignoring it and v0.84.4 (#8607) fixed
  Chat Completions dropping an explicit `toolChoice` when no tools are defined, and pi-messages'
  `streamSimple` was fixed to read `options?.toolChoice` rather than an `extra` bag. cyrup's
  `SimpleStreamOptions` (`crates/cyrup-provider/src/utils/simple_options.rs:38-43`) carries only
  `base`, `reasoning`, `thinking_budgets` — no `tool_choice` — while cyrup's `StreamOptions` does have
  one (`crate::stream::ToolChoice`, used at `api/openai_completions/params.rs:181-184`), **so the gap
  is specifically the simple-request plumbing.** Both sides read. Note the closed-and-refuted
  `PROV-015` concerns `ApiStreamOptions`' per-API variants, which is a different surface.

### Auth, models, providers, catalog

- **GitHub Copilot login: rate-limited model discovery, selective sequential policy enablement** · M —
  upstream `auth/oauth/github-copilot.ts:93-120` (`parseGitHubCopilotModelCatalog` now also returns
  `policyModelIds`), `:122-152` `fetchWithRateLimitRetry` (429 + `retry-after`, elapsed-time budget),
  `:382-405`, `:407-430` (sequential, stops the batch on rate-limit), `:462-483` (login enables only
  `unconfigured` policies for models pi knows). v0.84.2/v0.84.3 (#6187, #7850) — login no longer fires
  `Promise.all` over every known model. **Upstream side read in full; cyrup's enable-models path NOT
  read** (`crates/cyrup-provider/src/auth/oauth/github_copilot.rs`, login calls at `:527`/`:787`). It
  matters because `PROV-029` closed on the grounds that cyrup's Copilot login is now REACHABLE, and a
  reachable flow that fans out over every model is the failure upstream spent two releases fixing.
- **OpenRouter gains an `anthropic-messages` route; xAI loses `openai-completions` entirely** · M —
  upstream `providers/openrouter.ts:22-25` (api map with both) and `providers/xai.ts:7`,`:22`
  (`Provider<"openai-responses">`) @v0.85.1. v0.85.0 for OpenRouter (this is what carries the Anthropic
  per-turn-effort work there); v0.84.3 (#8124) moved ALL built-in xAI models to Responses with
  encrypted reasoning replay and made Grok 4.6 the default, `XAI_RESPONSES_COMPAT` now applied
  unconditionally. cyrup registers OpenRouter as completions-only (`providers/fleet.rs:170`). Both
  sides read for OpenRouter; the xAI half is a lead (cyrup's xAI routing lives in the generated
  catalog, not read upstream). **See the `PROV-054` supersession flag below.**
- **`Models.streamDeferred()`** · S — upstream `models.ts:217-221` (interface) and `:711-731`
  (implementation; `fetchDeferred` now delegates to `.result()`), v0.84.4.
  `grep -rn 'fn fetch_deferred\|fn stream_deferred' crates/ --include=*.rs` = 0 hits. Both sides
  checked. **This is item growth on the open `PROV-040`, not a separate surface** — amend that item.
- **Model-catalog generator rewrite** · L — upstream `scripts/generate-models.ts` @v0.85.1 plus the new
  `scripts/openrouter-reasoning-options.ts:11-23`. New `ANTHROPIC_ALLOWED_FALLBACK_MODELS`,
  `supportsAnthropicMidConvoEffort()`, `VERIFIED_ANTHROPIC_MID_CONVO_EFFORT_PROVIDERS = {anthropic,
  openrouter}`, `applyAnthropicMessagesCompatMetadata()`, `applyAnthropicAllowedFallbackModelMetadata()`,
  `OPENAI_ADDITIONAL_TOOLS_MODEL_IDS`, `OPENAI_CODEX_ADDITIONAL_TOOLS_MODEL_IDS`, `processZaiModels()`
  (new `zai-coding-cn` provider, GLM-5.2 `off:"none"`), `QWEN_TOKEN_PLAN_REASONING_EFFORT_FALLBACK_MODEL_IDS`,
  `DEEPSEEK_V4_FLASH_THINKING_LEVEL_MAP`, unconditional `XAI_RESPONSES_COMPAT`; OpenRouter thinking
  levels DERIVED from OpenRouter's own `reasoning` metadata so reasoning-mandatory models never receive
  `effort:"none"` (#8614, #8454); Fireworks GLM routed to completions; Baseten GLM-5.2 text-only;
  Xiaomi deprecations; catalog additions (GPT-6 Astra, deepseek-v4-flash-vision-exp,
  deepseek-v4-pro-0813, qwen3.8-flash) and removals (Grok Build 0.1). cyrup's catalogs are regenerated
  by `xtask gen-catalogs` pinned to `DEFAULT_REV = "b0c2a90e"` (`xtask/src/main.rs:68-71`), which
  predates this entire window. Both sides read at the generator level. **`PROV-060`'s routing note —
  close catalog items by one regeneration, never by hand edits — governs every catalog-shaped lead
  above.**

### cyrup-side surfaces landed in this window with no ledger row

Read on the cyrup side only; the pi counterpart was not opened. Recorded so the next pass knows they
exist, not as defect claims.

- **Live pi.dev catalog fetch for `xai` + per-provider staleness floors** · M —
  `crates/cyrup-provider/src/providers/catalog_manifest.json` (the `xai` entry is now
  `source: https://pi.dev/api/models/providers/xai` with its own `fetchedAt`/`revision`, while 34
  catalogs stay at pi@`b0c2a90e`) and `providers/all.rs`'s new
  `builtin_model_data_generated_at_by_provider`. This is `XAI_1` per the code's own id. The open
  `PROV-071` is written as if `XAI_1` were a proposal; it now exists as machine state in a shipped
  manifest. A verifier should confirm `PROV-071`'s residual count (34 vs 35) and `PROV-018`/`PROV-039`'s
  "the value must be the LATEST extraction revision" rule against the new two-tier floor. The xtask
  generator was not opened and pi was not re-read.
- **Catalog-overlay single slot (`CatalogOverlaySlot`) + refresh coordinator** · M —
  `crates/cyrup-provider/src/catalog_refresh.rs:1-21`,`:30-75` (new file, 482 lines) and
  `crates/cyrup/src/provider.rs` (+146). A port of pi's `ModelCatalogRefreshCoordinator`. Its own doc
  records the defect it fixes: the overlay had two independent homes — a `static` the refresh wrote and
  a by-value field on `AgentSessionServices` captured at build time that `/model` read — **so a
  completed refresh never reached the model picker**. Closed with no ledger row found; `PROV-S05` is
  the layer below and is already CLOSED. A verifier should confirm `DRIFT-007`'s closure still
  describes the code. pi's `model-catalog-refresh.ts` not opened.
- **Workspace-wide `serde_json/preserve_order` pinned by a cyrup-core test** · S —
  `crates/cyrup-core/src/lib.rs` (`preserve_order_is_declared_workspace_wide`) + the root
  `Cargo.toml` declaration. The feature is declared at the workspace rather than inherited from
  `agent-client-protocol`'s edge, and the test fails if it is dropped; its doc says two cyrup-mcp units
  compute a byte count that must match `JSON.stringify`'s and that a BTreeMap-backed Map would change
  it. A build-graph invariant two area-13 units depend on, recorded nowhere in the ledger. Also
  relevant to `ICOM-054`, whose test was flipped in this window (`71fefe3`).

### Leads against existing item ids (no row changed)

- **`PROV-054` (grok-4.5 on the wrong wire api, CLOSED) — supersession flag.** Its closure rests on
  `v0.83.0`'s generator hardcoding Responses routing for that one model id. At v0.85.1 the rule is no
  longer model-scoped: `providers/xai.ts:7` declares `Provider<"openai-responses">` with no completions
  route, and the generator applies `XAI_RESPONSES_COMPAT` unconditionally. **A re-audit of that closure
  is owed.**
- **`PROV-040` — item growth.** `Models.streamDeferred` is a third method to port. Amend the item
  rather than filing a new id.
- **`PROV-060` — its "not statically auditable is REFUTED" finding must not be generalized forward.**
  The item is correct about `b0c2a90e`, where `*.models.ts` are full data literals. For the window
  measured here the premise is TRUE again: `packages/ai/src/providers/data/` is **gitignored at every
  tag from v0.83.0 through v0.85.1** (`git ls-tree -r <tag> --name-only` returns 0 files at all seven
  tags), and `git diff --stat v0.84.1..v0.85.1 -- packages/ai/src/providers/` touches only 4
  hand-written files. Every catalog-shaped claim above therefore comes from `scripts/generate-models.ts`
  or the `*-models.test.ts` fixtures, never from catalog JSON. A refresh past `b0c2a90e` must be driven
  from that script, not from a `git show` of `*.models.ts`.
- **Adjacency map for scheduling** (same code block — do not split across agents): `PROV-019` +
  `supportsMaxOutputTokens` (`openai_responses/params.rs:130`); `PROV-023` + `prompt_cache_options.ttl`
  (`openai_responses/params.rs:112-126`); `DRIFT-013` + the DeepSeek `useMaxTokens` omission
  (`compat.rs:631-637`); `DRIFT-014` + the eighth retry literal (`retry.rs:72`); `PROV-016`/`PROV-046`
  + `normalizeOptionalNulls`; `DRIFT-048` + the Google finish-reason fix
  (`google_generative_ai/parts.rs:65-70`).

### Cleared in this window — read and deliberately not filed

- `constrained-sampling.ts` strict-schema conversion — **already absorbed**: cyrup's
  `utils/constrained_sampling.rs` is a self-declared 1:1 port @v0.84.2 (pi commit `7915cdac`), called
  from all five adapters. Only `normalizeOptionalNulls` remains, filed above.
- `createAiBindingFetch()` / `api/cloudflare-ai-binding.ts` (v0.85.0 BREAKING, #8287) — **no port
  target**: a Cloudflare Workers `env.AI` binding concern; cyrup is a native binary.
- Narrow `api`/`providers`/`utils` subpath exports — packaging only, no runtime behaviour.
- `fix(ai): remove unnecessary Chord dependency` (`JsonValue` redefined locally, `types.ts:408`) and
  the opentelemetry/dependency-tree cleanups — type-identical, no behaviour.
- The four Chord commits — out of scope for `packages/ai`; the substance is in `packages/chord`.
- `utils/sleep.ts` + `abortableSleep` — mechanism only; cyrup expresses it with `tokio::time::sleep`
  under a `CancellationToken`. The behaviour that used it (Copilot 429 retry) is filed above.
- `providers/faux.ts` optional-key spreading and `providers/cloudflare-ai-gateway.ts`'s three-api type
  pin — serialization/type-inference niceties with no Rust analogue.
- Mistral SDK → native HTTP transport (~326 of that file's 393 changed lines) — **already the cyrup
  shape**; `api/mistral_conversations/` is a hand-written native transport. **Corrected 2026-10-10: wrong — cyrup's transport is native but kept the SDK's camelCase
  wire keys; see `PROV-152` (closed 2026-10-10: `api/mistral_conversations/wire.rs` now ports
  `toMistralWirePayload` and the decoder reads Mistral's snake_case).** The one real behaviour
  change inside it (the tool-call key) is filed above.
- `packages/ai/src/providers/data/*.json` — **not readable at any tag in the window** (gitignored); no
  field-level catalog diff was possible. See the `PROV-060` note above.
- `test/` (46 of 94 changed paths, ~3 100 added lines) and `README.md`/`CHANGELOG.md` — read for
  evidence, not themselves port surfaces.

### What this census did not cover

Only `packages/ai` was measured. `packages/agent` (+54 223) and `packages/coding-agent` (+17 908) in
the wider `v0.84.4..v0.85.1` window are unmeasured here; several leads above (the assistant-message
frame reducer, `streamDeferred`, durable retry) are the `ai`-side half of agent-side work living in
those packages, and none of the three can be sized honestly until a pass scoped to `packages/agent`
runs.

### 2026-09-24 census note — `v0.85.1..v0.87.1` (leads, not items)

Upstream-side reads only unless stated. **Cleared (read, not filed):** OpenCode's new
`providers/opencode-headers.ts` `withOpenCodeSessionHeader` (v0.86.0, #9326) moves `x-opencode-session`
into the ai layer; cyrup already sends it on every session request via
`crates/cyrup-session-svc/src/attribution.rs::session_headers`, and the one bare-agent path
(`crates/cyrup-ext-subagents/src/watchdog/agent_turn.rs::opencode_session_headers`) ports pi-subagents'
copy — no user-visible gap, though a non-session SDK caller of `cyrup-provider` would still omit it.
`PROV-071`'s upstream falsifier (a) was re-run: all **41** `*.models.ts` under `packages/ai/src/providers/`
at v0.87.1 are re-exports of gitignored `data/*.json` (0 files in `git ls-tree v0.87.1 packages/ai/src/providers/data`),
now including `meta.models.ts` and a new `radius.models.ts` — premise holds.
**Leads (no id) — all dispositioned by the second pass, see the paragraph after this one:** (1) **the v0.86.0 `TranscriptContext` break** — `ProviderStreams` take a
normalized transcript whose leading `role:"system"` message carries prompt + `toolsAdded`, with
mid-conversation `SystemMessage`s (`types.ts:491-505` @v0.87.1, `utils/transcript.ts`) replayed natively
where `supportsMidConvoSystemMessages`/`supportsMidConvoToolAdditions` hold and collapsed otherwise;
`git grep -n 'tools_added\|mid_convo_system' -- crates/` is empty. Strategic, crosses cyrup-core/agent/session;
sized as a lead for `DRIFT-040`'s owner, not filed. (2) `Model.promptCache` lifetime metadata and cache
warming (v0.86.0). (3) Codex `Off` reasoning effort sent rather than omitted (#9191). (4) OpenRouter
`x-session-id` on Chat Completions + Anthropic Messages (#9102); Baseten session-affinity headers
(#9629). (5) Mistral `reasoning_effort` for all `mistral-medium-*` and Mistral-hosted GLM-5.2
(#8700, #9375). (6) Anthropic-compatible relays reporting a different response model (#9188);
Vercel AI Gateway unsigned-thinking replay (#9676); Google thinking-level resolution when reasoning is
omitted (#9455). (7) `EventStream` quadratic drain (#9055) — cyrup's stream is a channel, likely N/A,
not read. (8) `ToolCall.arguments`/`ToolResultMessage.details` narrowed to JSON values — type-only in TS,
cyrup already uses `serde_json::Value`. (9) New catalog rows (Opus 5.5, GPT-6 Sol/Luna, Grok 4.7 as the
xai default) — `PROV-071`'s class; pi's data is not readable from git. (10) v0.86.0 public Radius baseline
catalog — recorded as growth on `PROV-014`.


> **2026-09-24, second pass — every numbered lead above is resolved:** (1) `TranscriptContext` → **`PROV-083`**
> (the ai-side half; the session-format half is area 03's). (2) `Model.promptCache` is catalog metadata
> (`ANTHROPIC_PROMPT_CACHE = {short:300, long:3600}`, direct Anthropic only) that cyrup's `ModelCompat`/`Model`
> ignore on the overlay; the warming behaviour it feeds lives in `packages/coding-agent` — recorded as
> growth on `PROV-071`, not an ai-layer item. (3) Codex Off → **`PROV-085`**. (4) OpenRouter `x-session-id`
> → **`PROV-086`**; Baseten is data-only and arrives via the overlay — struck. (5) Mistral → **`PROV-088`**.
> (6) #9188 relay model: **struck for cyrup** — cyrup never overwrote `model` from `message_start`, so the
> thinking-replay loss never existed; the `responseModel` record is folded into `PROV-090`. Vercel unsigned
> thinking (#9676) is the generator's `compat: { allowEmptySignature: true }` on all 190 Vercel rows,
> which cyrup's `allow_empty_signature` already honours when the overlay supplies it — catalog-floor growth on
> `PROV-071`. #9455 → **`PROV-087`**. (7) `EventStream` quadratic drain — **struck, N/A**: cyrup's
> `AssistantMessageEventStream` is `cyrup_core::finalizing_channel`, a tokio `mpsc::unbounded_channel`, O(1)
> per event. (8) JSON-typed `ToolCall.arguments`/`details` — **struck**, type-level only; cyrup already uses
> `serde_json` values. (9) new rows (Opus 5.5, GPT-6 Sol/Luna, `deepseek-flash`) → growth on `PROV-071`.
> (10) Radius baseline catalog — growth on `PROV-014`, as recorded. Also read and **cleared**: Cerebras
> strict exclusion (#9804, v0.86.1) is recorded as growth on `PROV-078` (same expression, same fix).

### Post-tag leads (pi v0.87.1..b45597504) — not items; README cites upstream only at tags

Read at the commit, one-sided where stated. **(a) `7fd564cbb`** "share model catalog protocol with pi.dev":
pi.dev selects a catalog revision by the client's `pi/<version>` User-Agent and **serves the DEFAULT (newest)
revision to any other agent**. cyrup's catalog fetch sends `cyrup/<version> (…)`
(`remote_catalog.rs::cyrup_user_agent`), so it is served catalogs shaped for the newest pi — e.g. OpenRouter
`anthropic/*` rows on `anthropic-messages` (`PROV-099`) and compat keys it ignores (`PROV-083`, `PROV-091`).
Not verified live. **(b) `a328aa89a`** unifies image and classifier models, bumps the generated-data schema
to v6 and deletes `image-models.generated.ts` — the deadline in `PROV-089`. **(c) `667fc3dd3`** Vercel AI
Gateway reports 1-hour cache writes in `message_delta`; cyrup's
`api/anthropic_messages/usage.rs::apply_message_delta_usage` reads only `cache_creation_input_tokens` there
(both sides read) — a `PROV-081`-class pricing gap. **(d) `002fc8385`** `onProviderStreamEvent` /
`provider_stream_event` extension event — area 06's surface. **(e) `fde38ed7c`**, **`4c2dfd936`** — generator
data only (Copilot Opus 5.5 effort map; image regeneration).

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

> **Next free id: `PROV-154`** (2026-10-10, the `PROV-104` closure filed `PROV-153`; before that `PROV-153`, 2026-10-10, `PROV-152` filed while reviewing `PROV-141`; before that `PROV-152`, 2026-10-10, the `PROV-123`/`PROV-146` closure filed `PROV-150` and `PROV-151`; before that `PROV-150`, 2026-10-09, the pi v1.1.0 drift triage filed `PROV-139`…`PROV-149`; before that `PROV-139`, 2026-10-08, the `PROV-120` follow-ups filed and closed `PROV-135`, `PROV-136`, in its review pass `PROV-137`, and in its second review `PROV-138`; `PROV-134` was taken on 2026-10-05 by the coordinator while recording `PROV-128`, which is why this line already read `PROV-135`; before that 2026-10-03, post-pin triage of pi v1.0.1: filed `PROV-130`…`PROV-133`; earlier on 2026-10-03 batch 6 filed `PROV-112`, an id the pi v1.0.0 pass left free, so the counter did not move then; before that 2026-10-02, after the pi v1.0.0 pass filed `PROV-113`…`PROV-129`).

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
> **`PROV-060` is the one to read first if you only read one.** It refutes the premise `PROV-004` and `PARITY-GAPS.md:956` (`OQ-5`) both rest on — that catalog accuracy is "not statically auditable" — by showing the `*.models.ts` files are full data literals at `b0c2a90e`, the very revision cyrup's manifest names as its provenance floor. That refutation is what makes `PROV-054` … `PROV-059` measurable at all, and it hands `PROV-018` its drift check. Nine sweeps inherited the "unverifiable" verdict; the data was one `git show` away.
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


> **RE-AUDITED 2026-09-16 at cyrup `cc7818b`** (the `#140` merge; branch `claude/subagents-next` is
> cut from it). **No row in this area moved, and the reason is mechanical rather than a re-reading of
> each row.** `git merge-base --is-ancestor 9aeba769 cc7818b` holds, and
> `git diff --name-only 9aeba769..cc7818b -- crates/` touches **exactly three crates** —
> `cyrup-ext-subagents` (142 files), `cyrup-tui` (15) and `cyrup-it` (13). Every other crate under
> `crates/` is **byte-identical** between the sha this file was last re-read at and `cc7818b`, so
> every citation here that names a file outside those three resolves to the same bytes it did on
> 2026-09-14 and its evidence is carried forward **by identity, not by assertion**. Rows whose
> evidence DOES reach into one of the three are re-run individually and marked in their own cells;
> so are rows carrying no `9aeba769` marker, which the 2026-09-14 pass did not cover.
>
> **Why this pass exists, and the trap it had to avoid.** Three PRs landed since the last
> reconciliation (`#137`/`#139`/`#140`, the SCOPE sequence, ~31k lines) and the ledger did not know.
> The failure mode found in area 09 — shipped work still recorded as open — **does not reproduce
> here**, because ~99% of the SCOPE diff lands in `cyrup-ext-subagents`, which no row in this area
> owns. The opposite error was live, though, and was caught in the act: `CFG-066`'s row-level grep
> (`TERMUX_VERSION`/`DISPLAY`/`WAYLAND_DISPLAY` → now non-zero in `crates/cyrup-tui/src/clipboard.rs`)
> reads like a closure and **is not one** — the row's claim is about whether the NATIVE backend is
> constructed, and `app/event_extract.rs:108` still calls `arboard::Clipboard::new()` ungated. A grep
> that goes from 0 hits to N hits is a prompt to re-read the row, not a closure.
| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~PROV-134~~ | ~~low~~ **CLOSED 2026-10-09** | not-ported | M | **`BaseModel.inputLimits` is unported, so every model's provider input limits and cache-safe preprocessing metadata are silently discarded** — `types.ts:1105` @v1.0.1 declares `inputLimits?` on `BaseModel`, i.e. on chat, image AND classifier models alike, described as "Provider input limits and cache-safe preprocessing metadata"; upstream's image preprocessing reads `inputLimits.images.resize`. cyrup has **no counterpart anywhere**: `grep -rn 'input_limits\|inputLimits' crates/ --include='*.rs'` is empty, including on the chat `Model`. Nothing is broken — serde ignores the unknown field, so rows still parse — but the metadata never reaches a consumer. **Measured on the live catalog 2026-10-05:** present on **270 of 400** openrouter chat rows and **57 of 59** image rows, so this is the common case rather than an edge. **FILED 2026-10-05** by the coordinator while recording `PROV-128`, which made it visible and more relevant: image models are now reachable, and `inputLimits.images.resize` is exactly the field an image request would need. Pre-existing and NOT introduced by `PROV-128`; the chat half has been discarded for as long as the field has existed upstream. **Fix** — model `inputLimits` on the shared base shape so all three model types carry it, deserialize it, and thread `images.resize` to the image request path. **Verify** — a catalog row carrying `inputLimits` round-trips it through parse, store and overlay; an image request against a model whose row sets `images.resize` honours it; a row without the field still parses. — **CLOSED 2026-10-09**, in four commits on this branch with `SEAM-128` and `CFG-085`'s last clause. The three types (`ModelImageResizeOptions`/`ModelImageInputLimits`/`ModelInputLimits`, `types.ts:1075-1096`) and `input_limits` on `Model`, `ImageModel` and `ClassifierModel` — in upstream's `BaseModel` slot between `input` and `cost` — plus both hand-written `*Wire` mirrors and both `to_auth_model` shims, which is where the silent drop lived; a generation-time `apply_image_input_metadata` port of `applyImageInputMetadata` (`generate-models.ts:994-1020`) at the four catalog parse sites; and the two other declaration surfaces `promptCache` reached (`FauxModelDefinition`, `cyrup_ext::ProviderModelConfig`). The consumers: `AgentSession::normalize_prompt_images` (`SEAM-128`) and the `read` tool, whose profile is now the LIVE `ModelResizeHandle` re-pushed on both model-switch paths, which is this row's read-tool clause. **The stamp question was settled the other way, with evidence rather than the `promptCache` analogy:** upstream DOES inject at generation time (`applyImageInputMetadata` called at `generate-models.ts:3487` and `:3505`, `DEFAULT_IMAGE_RESIZE` at `:424-430`, upstream's own assertions at `packages/ai/test/providers.test.ts:111-136`). **Two corrections to this row.** (a) "thread `images.resize` to the image request path" does NOT mean `generate_images`: at v1.0.4 `packages/ai/src/images.ts` and `api/openrouter-images.ts` contain no reference to `inputLimits`, `resize` or `processImage`, and the only three consumers are `agent-session.ts:694`, `:1933` and `tools/read.ts:138` — so it means the paths by which images ENTER a provider request, and a pre-resize inside `generate_images` would have been an invention. (b) the "270 of 400 / 57 of 59" figures are the LIVE pi.dev body (`src/tests/fixtures/pi-dev-openrouter-all-types.json`, which reproduces exactly); the FROZEN catalogs measure 265/393 chat and 0 of 54 image-capable rows — the embedded chat rows already carry the field (1011 of 1519, and 0 text-only rows do), so adding the serde field recovers real generated data, while the image catalog is what the stamp actually fixes. `maxRequestBytes`, `images.maxPerMessage` and `images.maxPerRequest` are modelled and round-tripped with NO enforcement, which is parity: `packages/coding-agent/docs/models.md:89` says outright that pi does not rewrite or reject history based on them. |
| ~~PROV-M01~~ | ~~medium~~ **FILED AND CLOSED 2026-08-14** | parity-bug | S | Two hand-written `impl Provider` decorators dropped the trait-defaulted half of the surface pi's object spread carries — `github-copilot`'s credential filter was discarded in the overlay configuration — **FILED AND CLOSED 2026-08-14**: sweep 8. See the body below. |
| ~~PROV-030~~ | ~~high~~ **CLOSED 2026-08-14** | not-ported | L | `google-vertex` is registered with 10 models and no wire API — every request dies with `NoApiImpl` — **CLOSED 2026-08-14**: sweep 2 — the area's headline `high`. `api/google_vertex.rs` + `auth/google_adc.rs` ported end to end (express-mode vs ADC split, `resolveProject`/`resolveLocation` with pi's two verbatim throw strings, `{location}` interpolation, ADC search order, refresh-token exchange, RS256 JWT-bearer assertion, google-auth-library's 5-minute eager refresh); body/decoder delegate to `api::google_generative_ai` because pi's two `buildParams` are line-for-line identical at v0.83.0. **Sweep 1's stated blocker was false**: `ring` 0.17 was already resolved in Cargo.lock via rustls, so RS256 needed no new crate. `KNOWN_DANGLING = ["google-vertex"]` deleted from `every_catalog_row_names_a_registered_api` — the Verify clause is now enforced with no carve-out. |
| ~~PROV-027~~ | ~~high~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug | S | Copilot's Claude models send `x-api-key`; pi sends `Authorization: Bearer` — **REFUTED, CLOSED 2026-08-14**: sweep 2 — **REFUTED at HEAD**, and not in sweep 1's fixedIds, so it had been stale for at least one pass. `api/anthropic_messages.rs:434` carries the `model.provider === "github-copilot"` branch documented as "the branch Pi tests FIRST inside createClient", with a fixture at :2399. |
| ~~PROV-029~~ | ~~high~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug | S | Copilot + Codex login flows written but unreachable; flow registry has no production caller — **REFUTED, CLOSED 2026-08-14**: sweep 2 — **REFUTED at HEAD.** `providers/github_copilot.rs:157` wires `GitHubCopilotLogin` (the flow WITH `login`) with an explanatory block at :141-146; `providers/openai_codex.rs:137` wires `OpenAiCodexOAuthFlow`. Both dead-ends are gone. |
| ~~PROV-028~~ | ~~high~~ **CLOSED 2026-08-14 — REFUTED** | not-ported | S | `github-copilot-headers.ts` unported — no `X-Initiator`/`Openai-Intent`/`Copilot-Vision-Request` — **REFUTED, CLOSED 2026-08-14**: sweep 2 — **REFUTED at HEAD.** `crates/cyrup-provider/src/api/github_copilot_headers.rs` exists (223 lines) and is consumed by `anthropic_messages.rs`, `openai_responses.rs` and `openai_completions.rs` — the same three impls pi applies `buildCopilotDynamicHeaders` in. The item's `rg -i 'X-Initiator\|Copilot-Vision\|Openai-Intent'` evidence is stale. |
| ~~PROV-047~~ | ~~high~~ **CLOSED 2026-08-15** | parity-bug | M | `httpProxy` reaches only the streaming wire APIs — OAuth, the agent proxy transport and extension HTTP bypass it — **PARTIALLY CLOSED 2026-08-14**: sweep 2 — provider half landed: `sse::configure_http_proxy`/`configured_http_proxy` consulted inside `node_http_proxy::get_proxy_env` for `http_proxy`/`https_proxy` only (pi's `??=` ambient-wins precedence preserved), `sse::build_client_for(target_url)`, and all five OAuth flows converted off the proxy-blind `build_client()`. **One of the item's three cyrup-side claims is REFUTED**: `wire.rs:472` is not production code — the only `build_client()` in `wire.rs` is at :529 inside `mod tests` (the `#[cfg(test)]` boundary is at :235). **RESIDUAL — three lines in three other crates, and until the first lands the fix is inert in production**: (1) `configure_http_proxy(...)` beside the existing `configure_http_idle_timeout(timeout_ms)` in `cyrup-session-svc/src/builder.rs`; (2) `cyrup-agent/src/proxy.rs:468`; (3) `cyrup-ext/src/caps/http.rs:599`. One corner is not reproduced and is stated in-source: pi's `??=` leaves an ambient `HTTPS_PROXY=""` in place and `getProxyEnv`'s `\|\|` then skips it, so upstream that means "no proxy"; here empty and unset are indistinguishable. — **CLOSED 2026-08-15**: sweep 11 — **all three residuals were VERIFIED LANDED at HEAD**, so the row was stale, not the code: (1) `cyrup-session-svc/src/builder.rs:286` calls `cyrup_provider::configure_http_proxy(proxy.clone())`; (2) `cyrup-agent/src/proxy.rs:478` builds its transport with `cyrup_provider::build_client_for(&url)` and carries the PROV-047 rationale at `:468-477`; (3) `cyrup-ext/src/caps/http.rs:710-723` `client_builder()` ends `.no_proxy()` with the reasoning inline, and `client_through` adds its proxy AFTER that call so the ported resolver is the single authority. The only `build_client()` left anywhere in production is none — every remaining hit is under `#[cfg(test)]`. The `??=`-empty-string corner stays as the one documented, in-source non-reproduction. |
| ~~PROV-048~~ | ~~high~~ **CLOSED 2026-08-14** | parity-bug | S | A lone-surrogate `\uXXXX` escape in a provider SSE frame kills the whole turn — **CLOSED 2026-08-14**: sweep 1 — one predicate over the SSE JSON repair path; lone-surrogate escape no longer kills the turn. |
| ~~PROV-003~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | M | `ApiKeyAuth` has no `login`; `Models` has no `login`/`logout` (OAuth flow half now closed) — **CLOSED 2026-08-14**: sweep 1 — closed in FULL. The `ApiKeyAuth::login` half was already done at HEAD by c8c86bc (the item was stale on that point); anthropic api-key login and `Models::login`/`logout` are now in. The status line still saying "partially-closed" is superseded. |
| ~~PROV-011~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | L | `constrainedSampling` / grammar-constrained tools not modeled — **four** affected sites (six at HEAD) — **CLOSED 2026-08-14**: sweep 6 — the ends were ported by sweeps 2-5; the two frames between them (`cyrup-agent/src/agent.rs:818` hard-coding `constrained_sampling: None`, and `cyrup-ext/src/wrapper.rs`'s hand-written `impl Tool for RegisteredTool` omitting the method) dropped the value, so no tool could opt in at all. Both closed; pinned by `agent_loop.rs::prov011_a_tools_constrained_sampling_declaration_reaches_the_provider`. **Do not add opt-ins to cyrup-tools' built-ins — no pi built-in declares `constrainedSampling`** (three grep hits at v0.83.0, all in `types.ts:463` / `tool-definition-wrapper.ts:14,:42`). |
| PROV-014 | ~~medium~~ low — **PARTIALLY CLOSED 2026-09-04** | parity-bug | M | radius + qwen-token-plan ×2 unregistered (pi-messages half closed) — **NOT STARTED 2026-08-15 (sweep 11), and the reason is a scheduling one, recorded so the next attempt does not re-derive it.** Half of the item is already done and this row did not say so: `env_api_keys.rs:53-54`,`:66` carries all three arms (`QWEN_TOKEN_PLAN_API_KEY`, `QWEN_TOKEN_PLAN_CN_API_KEY`, `RADIUS_API_KEY`) with the `env-api-keys.ts` citations. What remains splits into two pieces of very different size. **(a) qwen-token-plan ×2 — S, but it belongs to the CATALOG owner.** pi's providers are three-line `createProvider` calls (`providers/qwen-token-plan.ts` @v0.83.0: base URL `https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1`, `envApiKeyAuth("Qwen Token Plan API key")`, `openAICompletionsApi()`), so cyrup needs two `providers/fleet.rs` rows and two catalog JSON files sourced from `qwen-token-plan{,-cn}.models.ts`. **Hand-writing those two files is exactly the failure the Routing note above forbids**: they must come out of `xtask gen-catalogs` or `catalog_manifest.json` is invalidated on the next `--check`. Schedule with the generator, not against a provider agent. **(b) radius — M, and it is a NEW PROVIDER KIND, not a fleet row.** `providers/radius.ts` @v0.83.0 is hand-written, not `createProvider`: a `pi-messages` provider with a gateway option, `lazyOAuth`, and a `refreshModels` that reads a persisted store, imports a legacy pre-`ModelsStore` catalog when the stored entry is absent and the credential is OAuth, and refreshes dynamically — plus the whole of `providers/radius-config.ts` (`DEFAULT_RADIUS_GATEWAY`, `normalizeRadiusGatewayUrl`, `getRadiusModels`, `getRadiusModelsFromConfig`, `loadRadiusGatewayConfig`). The OAuth flow at `auth/oauth/radius.rs` is ported and ready; the provider around it is not. `builtin_oauth.rs:17`'s "radius … has no built-in provider in cyrup" is the accurate statement of that, and it is now the only radius claim in-tree that is not stale. — **PARTIALLY CLOSED 2026-09-04** (commit `1471a16f`; residual **low**): all three v0.83.0 built-ins — and v0.84.4's fourth, `qwen-token-plan-individual` — are now REGISTERED, resolve auth from their env vars, and stream against a faux origin. **qwen-token-plan ×3** are `providers/fleet.rs` members (`QWEN_TOKEN_PLAN`, `QWEN_TOKEN_PLAN_CN`, `QWEN_TOKEN_PLAN_INDIVIDUAL`; id/name/`baseUrl`/`envApiKeyAuth` label field-for-field against `ai/src/providers/qwen-token-plan{,-cn,-individual}.ts:6-15` @v0.84.4, registered `all.ts:118-120`) with `FleetCatalog::Dynamic` — a NEW two-variant enum on `FleetSpec.catalog` (`Embedded(&str)` | `Dynamic`) — and the provider-level `baseUrl` upstream's `createProvider` carries, now `FleetSpec.base_url` → `WireProvider::with_base_url`. **Why no catalog files, stated as evidence not convenience:** pi's rows for these are models.dev `alibaba-token-plan[-cn]` data (`ai/scripts/generate-models.ts:2303-2380` @v0.84.4) generated into gitignored `providers/data/*.json`; the providers were added at `bbb91fa8a` (v0.81.0~25) and `c03d78bdc`, both AFTER `b0c2a90e`, the last revision at which any `*.models.ts` was a data literal and the only revision `xtask gen-catalogs` can extract from — so the rows are in git at NO revision (`git -C tmp/pi log --all --diff-filter=A -- 'packages/ai/src/providers/data/*.json'` is empty), and both runtime sources (models.dev, pi.dev) are unreachable from this workspace (proxy 403). The previous row's own instruction — the files must come from the generator, never by hand — is therefore satisfied by shipping none: the runtime catalog arrives through the pi.dev overlay (`remote_catalog.rs` fetches `/api/models/providers/<id>` for every configured provider; `AuthStore::has_auth` counts the env key, so an exported `QWEN_TOKEN_PLAN_API_KEY` triggers it in rpc/interactive) and through `models.json`. `catalog_data.rs::DYNAMIC_ONLY_PROVIDERS` pins the four dynamic-only ids in BOTH directions so an accidentally empty catalog still fails. **radius** is a new provider kind, `crates/cyrup-provider/src/providers/radius.rs` (`RadiusProvider`, `RadiusProviderOptions`, `radius_provider_with`, `radius_auth`): a `pi-messages` provider over a `WireProvider` (every `Provider` method delegated by name, PROV-M01) with `envApiKeyAuth("Radius API key", [RADIUS_API_KEY])` + a gateway-bound `RadiusOAuth` (`radius.ts:30-33` @v0.84.4; `builtin_oauth.rs` gained the `"radius"` arm and its `:17` claim is gone), and the whole of `radius-config.ts` @v0.84.4 (byte-identical at v0.83.0): `DEFAULT_RADIUS_GATEWAY` (`:4`), `RadiusGatewayModel`/`RadiusGatewayConfig` (`:6-20`), `is_radius_gateway_model`/`sanitize_radius_gateway_config` (`:26-50`, rows FILTERED never fatal), `radius_credential_config`/`radius_models` (`:57-73`, read from `Credential::Oauth.ext["gatewayConfig"]`), `radius_models_from_config` (`:61-68`), `truncate_http_body` (`:75-78`), `load_radius_gateway_config` (`:80-96`, both error strings verbatim). `RadiusProvider::refresh_models` is `radius.ts:35-78` @v0.84.4 — legacy import, `allowNetwork`/abort gate, key via `resolve_provider_auth` (= `resolveRefreshCredential`, `models.ts:448-474`), fetch, publish to the attached `ModelsStore` — deduplicated with `RefreshDedup`; restore is the overlay loader's job (`RemoteCatalog::load_overlay` → `CatalogOverlay::apply`), the same split `remote_catalog.rs` documents, with ONE `[CYRUP-DELTA]`: the persisted entry stamps `last_modified = checked_at` so the overlay's pi #7016 staleness guard (vacuous for a provider with no embedded rows) does not discard it. Shell: `crates/cyrup/src/provider.rs::pi_dev_catalog_providers` excludes `radius` from the pi.dev fetch list, mirroring `model-runtime.ts:183-189` @v0.84.4 (`provider.id === "radius" ? provider : withRemoteCatalog(…)`). `env_api_keys.rs` gained the v0.84.4 `qwen-token-plan-individual` arm (`env-api-keys.ts:83`, same variable as `qwen-token-plan`). **Tests** (all red before — the ids/symbols did not exist, the registry asserted their ABSENCE, and the shell test counted two pi.dev requests where one is now issued): `fleet.rs::qwen_token_plan_members_match_upstream`, `fleet_has_nineteen_providers`; `all.rs::registry_contains_implemented_provider_ids` (three ids moved from the not-yet list to the expected list; `baseten` entered the not-yet list), `radius_registers_with_both_auth_strategies_and_takes_the_overlay`; `builtin_oauth.rs::only_the_five_built_ins_carry_oauth`, `subscription_split_matches_upstream`; `env_api_keys.rs::qwen_token_plan_and_radius_rows_match_upstream`; `radius.rs` ×16 (sanitize filter/reject, models-from-config stamping, legacy credential config, `truncate_http_body` 512-char cap, origin-resolved `/v1/config`, loopback `GET /v1/config` request shape with/without bearer, 503 and invalid-body messages verbatim, cancelled token opens no socket, static without a store, refresh publishes + restores through the real overlay loader with the built-in floor stamp, cache-only issues no request, legacy import, 500 persists nothing, a gateway row STREAMS over `POST /v1/messages` with `Authorization: Bearer` from `RADIUS_API_KEY`); `catalog_data.rs::every_registered_provider_has_a_non_empty_catalog` (two-directional exemption); `crates/cyrup/src/tests/catalog_refresh_modes.rs::update_models_never_fetches_radius_from_pi_dev`; `help.rs` env-block row for the Individual plan. `cargo nextest run -p cyrup-provider`: 1151 passed; `-p cyrup` green; clippy `-D warnings` and `RUSTDOCFLAGS='-D warnings' cargo doc` clean on both. **RESIDUAL (low), three pieces, none a registration gap:** (1) the three Qwen catalogs stay `Dynamic` until the models.dev/pi.dev data is reachable from a workspace — same constraint class as PROV-004's Coverage note; the git-derivable facts (ids, compat, thinking maps, `qwen-token-plan-models.test.ts` @v0.84.4) are recorded on each `fleet.rs` member for that day; (2) the shell never calls `Provider::refresh_models` — its refresh drives `RemoteCatalog` directly — so `RadiusProvider`'s gateway refresh runs only through `Models::refresh_with` on a store-attached instance (`all_providers()` constructs it static); wiring pi's `Models.refresh` trigger into `spawn_model_catalog_refresh`/`refresh_model_catalogs` with the `auth.json`-backed credential store is an S shell change; (3) `configureRadiusProviders` (`model-runtime.ts:219-233` @v0.84.4 — a `models.json` block with `"oauth": "radius"` becomes a `radiusProvider({ id, name, gateway })` instance) is not wired: `cyrup-config/src/model/compose.rs` composes the block's models but no `RadiusProvider` is constructed for it; `RadiusProviderOptions` is the ready-made shape. Separately noted, NOT this item: `baseten` (`all.ts:95` @v0.84.4) is the one v0.84.x built-in still unregistered — it is in `all.rs`'s not-yet guard with no ledger id. — **RE-READ 2026-09-14 at `9aeba769`, STILL OPEN (low); all three residual pieces survive verbatim, and the CLOSED half was re-audited for what its code DOES, not that it exists.** Registration is true against the CURRENT tag, not only v0.84.4: radius is constructed at `providers/all.rs:260` (`radius_provider_with` pushed), the guard array is now empty (`all.rs:419` `const NOT_YET: &[&str] = &[]`), and pi's `builtinProviders()` is unchanged at 40 entries (`packages/ai/src/providers/all.ts:89-129` @v0.85.1; `git diff v0.84.4 v0.85.1 -- all.ts` is empty, as are the diffs for `providers/radius.ts`, `radius-config.ts`, `qwen-token-plan.ts` and `env-api-keys.ts`). The three Qwen plans are real FLEET members with `base_url` + `envApiKeyAuth`. Residuals, each re-read on BOTH sides: **(1)** the three Qwen members are still `FleetCatalog::Dynamic` with zero embedded rows (`providers/fleet.rs:179,183,189`; `:209` `FleetCatalog::Dynamic => Vec::new()`) and upstream's data is still unobtainable — see `PROV-071`; **(2)** `git grep refresh_models 9aeba769 -- crates/cyrup crates/cyrup-config` is 0 hits — no crate outside `cyrup-provider` mentions `refresh_models` at all — so the shell still never drives `Provider::refresh_models` and `RadiusProvider`'s gateway refresh remains library-reachable only; **(3)** `configureRadiusProviders` (`packages/coding-agent/src/core/model-runtime.ts:219-233` @v0.85.1, byte-identical to v0.84.4, called at `:198` and `:700`) is still unported — `cyrup-config/src/model/compose.rs:134,163-166` handles only the `"oauth":"radius"` baseUrl rule and constructs no `RadiusProvider`, and `RadiusProviderOptions`/`radius_provider_with` have no caller outside `cyrup-provider` (only the `lib.rs`/`providers/mod.rs` re-exports and the doc mention at `crates/cyrup/src/provider.rs:98`). Severity stays **low**; Kind stays `parity-bug` (piece 3 is v0.83.0-era shell behaviour, not drift). **ONE STALE SUB-CLAIM, corrected here rather than rewritten above:** the trailing "`baseten` … is the one v0.84.x built-in still unregistered" is no longer true — `baseten` is a registered `Dynamic` fleet member (`providers/fleet.rs:151-156`, citing `DRIFT-009` in-source), which is precisely why `NOT_YET` is empty. **Falsification:** pieces (2)/(3) reopen-or-close by measurement — if any crate outside `cyrup-provider` gains a `refresh_models` call site, or `RadiusProviderOptions` gains a construction site driven by a `models.json` `"oauth":"radius"` block, re-audit those two pieces; piece (1) closes only when the three Qwen catalogs carry embedded rows and their ids leave `catalog_data.rs::DYNAMIC_ONLY_PROVIDERS`. **RE-CONFIRMED 2026-09-16 at `cc7818b` by identity:** `crates/cyrup-provider` and `crates/cyrup-core` are byte-identical across `9aeba769..cc7818b`, so the 2026-09-14 reading stands unchanged. The scheduling note still governs. **RE-VERIFIED STILL OPEN 2026-09-24 at `ea23ca2` / pi `v0.87.1`:** cyrup side byte-identical: `git diff --quiet 9aeba769 ea23ca2 -- crates/cyrup-provider crates/cyrup-core crates/cyrup-config xtask` exits 0, and `git grep 'refresh_models\|RadiusProviderOptions\|radius_provider_with' -- crates ':!crates/cyrup-provider'` is still empty, so pieces (2) and (3) stand. **Upstream GROWTH on piece (1), not a closure:** v0.86.0 gave Radius a generated public baseline catalog — `packages/ai/src/providers/radius.ts` @v0.87.1 seeds `baselineModels` from `RADIUS_MODELS` (`radius.models.ts`, a re-export of gitignored `data/radius.json`) when the gateway is the default one and overlays dynamic rows by id, so pi lists Radius models offline where cyrup lists none. The data is unreadable from git (`PROV-071`'s class); `qwen-token-plan*.ts`, `radius-config.ts` and `model-runtime.ts`'s `configureRadiusProviders` carry no behaviour change in `v0.85.1..v0.87.1`. |
| ~~PROV-016~~ | ~~medium~~ **CLOSED 2026-08-14** | stale-port | S | Tool-argument coercion ignores `allOf` — **CLOSED 2026-08-14**: sweep 1 — confirmed at HEAD exactly as filed; citations resolved clean at v0.83.0. |
| ~~PROV-018~~ | ~~medium~~ **CLOSED 2026-08-15** | tooling | M | No catalog generator, no drift check — **CLOSED 2026-08-15**: sweep 10 — `xtask` exists and generates all 35 catalogs plus `catalog_manifest.json` from one pinned revision. **The Fix's stated mechanism was replaced and the reason is recorded**: it said to run pi's `npm run generate-models` "because the tree can no longer simply be read", which PROV-060 refutes — at `b0c2a90e` the `*.models.ts` modules are data literals, so `git show` + a scanner needs no `npm install`, no generator run and no network. Drift check: `gen-catalogs --check` (byte-exact) and `--diff` (structural), plus the `#[ignore]`d `gen_catalogs_check_reports_no_drift_against_the_pinned_revision`. |
| ~~PROV-019~~ | ~~medium~~ **CLOSED 2026-08-14** | stale-port | S | `max_output_tokens` floor of 16 unported in both Responses APIs — **CLOSED 2026-08-14**: sweep 1 — landed together on both openai routes plus azure and codex, sharing one `SessionAffinityFormat`. PROV-034 additionally required CORRECTING a test that pinned `strict == false` with no compat (a new test-defect instance). |
| ~~PROV-021~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `ANTHROPIC_AUTH_TOKEN` bearer env unsupported — **CLOSED 2026-08-14**: sweep 1 — `ANTHROPIC_AUTH_TOKEN` bearer env supported. |
| ~~PROV-024~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `sessionAffinityFormat` unported on openai-completions — **CLOSED 2026-08-14**: sweep 1 — landed together on both openai routes plus azure and codex, sharing one `SessionAffinityFormat`. PROV-034 additionally required CORRECTING a test that pinned `strict == false` with no compat (a new test-defect instance). |
| ~~PROV-032~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | `Provider::filterModels` unported — Copilot filter has zero production callers — **CLOSED 2026-08-14**: sweep 1 — landed together, as PROV-032's own Fix predicted. Two deltas documented in-tree: `check_auth` resolves against the provider's first catalog row (cyrup's `ApiKeyAuth::resolve` takes a `&Model`), and pi's optional `ApiKeyAuth.check?` (auth/types.ts:173) has no cyrup counterpart and no upstream implementor. |
| ~~PROV-033~~ | ~~medium~~ **CLOSED 2026-08-14** | stale-port | S | openai-responses carries pi's deleted `sendSessionIdHeader`; `x-session-id` unreachable — **CLOSED 2026-08-14**: sweep 1 — landed together on both openai routes plus azure and codex, sharing one `SessionAffinityFormat`. PROV-034 additionally required CORRECTING a test that pinned `strict == false` with no compat (a new test-defect instance). |
| ~~PROV-035~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | M | `cache-stats.ts` unported — no cache-waste accounting, no cache-miss notices — **PARTIALLY CLOSED 2026-08-14**: sweep 2 — `crates/cyrup-provider/src/cache_stats.rs` is a full port of `cache-stats.ts` (CACHE_TTL_MS, NOISE_FLOOR_TOKENS, `detect_miss`, `as_previous_request`, `scan`, `compute_cache_waste`, `collect_cache_misses`, `detect_cache_miss`), including the arithmetic that looks like a bug upstream: the noise floor is `<=` so exactly 1024 is NOT a miss, `reportedCache` is sticky, a promptless turn does not clear `prev`, and a model switch is deliberately not a reset. **Two forced shape changes, both documented in the module header**: the scan takes `&[CacheScanEntry]` (`SessionEntry` lives in cyrup-session, which DEPENDS on cyrup-provider), and misses are keyed by INDEX because pi keys its result map on the `AssistantMessage` OBJECT REFERENCE. **RESIDUAL:** both render sites — the `/session` `Cache Re-billed:` line at `cyrup-tui/src/app.rs:4192` *(pre-split)* under pi's `stats.cost > 0 \|\| cacheWaste.missedTokens > 0` guard, and `collect_cache_misses` behind a `showCacheMissNotices` setting that does not exist. — **FIRST RENDER SITE CLOSED 2026-08-15**: sweep 11 — `state::cache_scan_entries` is the adapter the module header prescribed, `AgentSession::cache_waste()` runs `compute_cache_waste` over the session's full model registry as pi's `modelRuntime` argument does (`interactive-mode.ts:5660`), and the TUI `/session` block emits the `Cache Re-billed` row under pi's exact guard with its `missedCost >= 0.0001` split and its singular/plural `1 miss`/`N misses` (`:5704-5711`). The app.rs offset in this row was stale, and `40821ed` has since deleted the file: the renderer is `C::SessionInfo` at `app/execute_session.rs:147-213`, with the guard at `:181`, the `breakdown.len() > 1` gate at `:184` and the two `Cache Re-billed` formats at `:204`/`:208`. **RESIDUAL NARROWED to the second site only** — `collect_cache_misses` re-injecting per-message notices at render time (pi `interactive-mode.ts:3354-3355`, `:3456`, `:4166` — re-verified at `v0.83.0` 2026-08-19; the anchor is spelled out because the preceding citation is now a cyrup `app/` path) behind a `showCacheMissNotices` setting. That is a new `cyrup-config` settings key plus a transcript-render path, ~M, and was NOT started. — **SECOND SITE CLOSED 2026-09-04**: personally verified at HEAD `2571969`, landed sometime between this file's `4fb5e40` baseline and now (no commit names PROV-035). The setting exists — `crates/cyrup-config/src/settings/effective.rs::show_cache_miss_notices`, `getShowCacheMissNotices` default `false`, matching `interactive-mode.ts` @v0.83.0 — and is threaded live into `cyrup-tui/src/app/state.rs::AppState.show_cache_miss_notices`, re-read on settings apply (`app/run_arms.rs`) and on the `/settings` toggle (`app/execute_misc.rs::"showCacheMissNotices"`). The notice fires from `crates/cyrup-tui/src/app/events.rs`: the assistant-message-end handler sets `cache_miss_check_pending` under pi's exact non-terminal guard (`!matches!(stop_reason, Aborted | Error)`, mirroring `interactive-mode.ts:3752`'s `else` branch), and the pending flag is settled by an async call to `AgentSession::last_cache_miss()` (`cyrup-session-svc/src/session/stats.rs:74-80`, which itself calls `cyrup_provider::cache_stats::collect_cache_misses`) before pushing `transcript.push_cache_miss_notice(&miss)`. `push_cache_miss_notice` (`cyrup-tui/src/transcript/notices.rs:161-182`) reproduces `addCacheMissNotice` field-for-field, re-verified against `interactive-mode.ts:3455-3476` @v0.83.0: the `missedTokens < 20_000 && missedCost < 0.1` skip threshold, the `~$X.XX` cost suffix gated at `>= 0.01`, and all three labels ("Cache miss", "Cache miss after model switch", `"Cache miss after {n}m idle"`) byte-for-byte. Both render sites are now live; nothing left open under this id. |
| ~~PROV-042~~ | ~~medium~~ **CLOSED 2026-09-05** | not-ported | M | `transformHeaders` unported — `before_provider_headers` has no seam — **PARTIALLY CLOSED 2026-08-14**: sweep 1 — the Models-level seam is in (`StreamOptions.transform_headers`, applied at models.ts:480's position and stripped at :483's). **RESIDUAL:** the session-svc closure folding in `merge_provider_attribution_headers`, plus the `before_provider_headers` extension event and its WIT/event-catalog ABI bump (area 06). Nothing left inside cyrup-provider. — **RESIDUAL RE-MEASURED 2026-08-15 (sweep 11), and it is now ONE seam, not three.** Two of the three named residuals are done: the ABI half landed in full (`cyrup-ext/src/event.rs:59` `BeforeProviderHeaders = 31` with its name and `from_u8`, the `EventPatch::ProviderHeaders` reducer at `contract.rs:148-151`, `host/live.rs:2081-2082` and `:2303`, the WIT comment at `cyrup-ext{,-sdk}/wit/world.wit:326-327`, and the guest side at `cyrup-ext-sdk/src/api.rs:789-796` + `macros.rs:227-230`), and the attribution closure is installed — `AgentSession::into_shared` sets `agent.set_header_fn(...)` → `headers_for_model_ref` → `merge_provider_attribution_headers` (`session.rs:640-651`, `:3069-3084`, landed as AGENT-029). **What is genuinely left is the EMITTER and only the emitter**: `grep -rn 'HostEvent::BeforeProviderHeaders' crates/` finds the reducer, the enum, and one test constructing it — and no production site that emits it. So an extension can subscribe to `before_provider_headers` today and will never be called, which is worse than the documented refusal the item warned about. Also note the ordering half is still unreproduced for a second reason: cyrup's attribution rides the AGENT's `header_fn`, not `StreamOptions::transform_headers`, so pi's guarantee that the hook sees the attribution set already merged has no single point where both are present. Landing the emitter means moving the attribution closure onto `transform_headers` (the seam `collection.rs:556-560` already applies at pi's `models.ts:480` position) and dispatching the event from inside it. ~M, one crate. — **CLOSED 2026-09-05 at `bb355412` + `b9837e6c`.** The re-measurement was right that only the emitter was left, and wrong about why it was hard. **The seam was on a dead path.** `Models::apply_auth` is pi's literal position (`models.ts:657` @v0.84.4, stripped at `:660`) and is correct *for pi*, where every request goes through `Models.stream`/`streamSimple`. cyrup's agent streams `StreamFn` → `Provider::stream` → `WireProvider::stream` (`wire.rs:149`) → `ApiImpl::run`, and `rg '\.stream_simple\(|Models::stream' crates/` finds NO production caller of the collection — so a `transform_headers` closure was inert wherever it was installed, and the emitter would have been too. Fixed in two commits. (1) `bb355412` adds `crate::stream::apply_transform_headers` — the sibling of the existing `apply_on_payload` — and calls it from all TEN registered api impls right after `build_headers`, which is the position that reproduces pi's effect on cyrup's topology and the only one where the item's own Verify clause ("removing `x-api-key` inside it actually suppresses it") is reachable, since `x-api-key` is added by the api impl and not by `applyAuth`. Bedrock is the one documented exception: SigV4 signs the header set, so the hook runs at pi's pre-signing caller-header injection point (`bedrock-converse-stream.ts:224-227`) instead. (2) `b9837e6c` installs the producer in `SessionBuilder::build` beside the two sibling seams already there, via a new `AgentBuilder::transform_headers` → `GenerationConfig` → `StreamOptions`, plus `ExtensionHost::emit_before_provider_headers` (pi `runner.ts:1100-1125`). **The ordering claim is refuted, not carried:** attribution does NOT need to move. It rides `StreamOptions::headers` (AGENT-029) and the provider merges that overlay into the assembled set BEFORE running `transform_headers`, so the hook sees the attribution set already present and its return value wins — pi's guarantee, reached by a different route. Tests: `cyrup-provider/src/tests/transform_headers_on_the_wire.rs` drives the real `ApiImpl::run` for each of the ten apis against a loopback origin that records the request head (add reaches the wire, delete suppresses, the closure was handed the assembled auth header), with `every_registered_api_impl_has_a_case` over the new `ApiRegistry::ids` holding the "every api impl" bar as a property; `cyrup-session-svc/src/tests/before_provider_headers_emitter.rs` runs a real session turn and proves a subscribed native extension's add + `null`-delete reach the header bag, and that the unsubscribed path is an exact identity. **RESIDUAL:** none in this area. The `Models`-level seam keeps its own application (and its strip) so a `Models`-routed request still transforms exactly once; the two positions never stack. |
| ~~PROV-049~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `repair_json`'s invalid-`\u` arm doubles the backslash where pi emits `\u` unchanged — **CLOSED 2026-08-14**: sweep 1 — one predicate over the SSE JSON repair path; lone-surrogate escape no longer kills the turn. |
| ~~PROV-050~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `parse_partial` deletes astral characters written as surrogate pairs from recovered tool arguments — **CLOSED 2026-08-14**: sweep 1 — one predicate over the SSE JSON repair path; lone-surrogate escape no longer kills the turn. |
| ~~PROV-004~~ | ~~**tracker**~~ **CLOSED 2026-08-15** | tooling | M | The five newest catalogs were never field-diffed — **CLOSED 2026-08-15**: sweep 10 — its premise ("no longer checkable from this workspace") was false, and its coverage hole is gone: all 35 catalogs, including all five, are now generated from one revision and diffed field-by-field by `gen-catalogs`. |
| ~~PROV-015~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED** | not-ported | S | `ApiStreamOptions` has no `openai-completions` variant — **REFUTED, CLOSED 2026-08-14**: sweep 1 — **REFUTED, not fixed.** Its Impact is false at both tags: `OpenAICompletionsOptions`' only own members are `toolChoice`, `reasoningEffort` and (v0.84.1) `thinkingBudgets`, all three already on cyrup's `StreamOptions`. Reasoning recorded in-tree on the `ApiStreamOptions` enum. |
| ~~PROV-017~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | `Provider` trait exposes no `name`/`base_url`/`headers` — **CLOSED 2026-08-14**: sweep 1 — landed together, as PROV-032's own Fix predicted. Two deltas documented in-tree: `check_auth` resolves against the provider's first catalog row (cyrup's `ApiKeyAuth::resolve` takes a `&Model`), and pi's optional `ApiKeyAuth.check?` (auth/types.ts:173) has no cyrup counterpart and no upstream implementor. |
| ~~PROV-020~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `toolResult` JSONL key order diverges: `isError` emitted too early — **CLOSED 2026-08-14**: sweep 1 — `toolResult` JSONL key order corrected. |
| ~~PROV-023~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `prompt_cache_options` unported — one-shot requests implicitly cache-write — **CLOSED 2026-08-14**: sweep 1 — landed together on both openai routes plus azure and codex, sharing one `SessionAffinityFormat`. PROV-034 additionally required CORRECTING a test that pinned `strict == false` with no compat (a new test-defect instance). |
| ~~PROV-025~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED** | parity-bug | M | `deferredToolsMode: "kimi"` unported — **REFUTED, CLOSED 2026-08-15**: sweep 11 — **fully ported at HEAD and has been for at least one sweep; this row was stale, not the code.** The item's evidence (`rg --type rust 'deferred_tools_mode\|DeferredToolsMode' crates/` = 0 hits) now returns 12 production hits. `DeferredToolsMode::Kimi` is on `ModelCompat` (`api/compat.rs`), detected `None` and resolved with `.or(detected)` in `get_compat`, and BOTH halves of pi's rendering are in `api/openai_completions.rs`: `deferred_tool_names` is the separate `getDeferredToolNames` accessor (`openai-completions.ts:91-101`, insertion-ordered) filtering the top-level `tools` array (`:719-721`), and `convert_messages` pushes the two-key `{role:"system", tools:[…]}` message at upstream's exact position — after the image/`lastRole` handling, immediately before the `continue` (`:1266-1276`). Verified line-for-line against `git show v0.83.0:packages/ai/src/api/openai-completions.ts`. A body test already exists at `openai_completions.rs:2380` and states its own red-before condition. **NOTE 2026-09-24 (second pass), not reopened:** upstream DELETED `deferredToolsMode` in v0.86.0 (`9e05370b2`); Kimi K3's `{role:"system", tools}` item is now gated on `supportsMidConvoSystemMessages && supportsMidConvoToolAdditions`. The ported flag is a stale shape, tracked in `PROV-083`. |
| ~~PROV-031~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | M | `Models` has no `get_available`/`check_auth`/`login`/`logout` — **CLOSED 2026-08-14**: sweep 1 — landed together, as PROV-032's own Fix predicted. Two deltas documented in-tree: `check_auth` resolves against the provider's first catalog row (cyrup's `ApiKeyAuth::resolve` takes a `&Model`), and pi's optional `ApiKeyAuth.check?` (auth/types.ts:173) has no cyrup counterpart and no upstream implementor. |
| ~~PROV-034~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | openai-responses always emits `"strict": false` — **CLOSED 2026-08-14**: sweep 1 — landed together on both openai routes plus azure and codex, sharing one `SessionAffinityFormat`. PROV-034 additionally required CORRECTING a test that pinned `strict == false` with no compat (a new test-defect instance). |
| ~~PROV-036~~ | ~~low~~ **CLOSED 2026-08-15** | not-ported | S | `getUsageCostBreakdown` unported — one cost total, no per-model breakdown — **CLOSED 2026-08-15**: sweep 11 — ported as `cyrup-session-svc/src/state.rs::usage_cost_breakdown` (1:1 with `core/usage-totals.ts:37-70` @v0.83.0: the `provider/responseModel ?? model` key, the literal `Tools/summaries` bucket for toolResult + compaction + branch-summary usage, the `cost > 0 \|\| tokens > 0` filter and the cost-DESCENDING sort), surfaced as `AgentSession::usage_cost_breakdown()`, and rendered in `cyrup-tui/src/app/execute_session.rs:184-192`'s `/session` block under pi's `usageBreakdown.length > 1` guard (`interactive-mode.ts:5699`). Sweep 8's routing note was right that this needs `cyrup-session-svc` + `cyrup-tui` together; both landed in one pass. Pinned by `prov036_breakdown_keys_attribute_sort_and_reconcile`, which asserts pi's own stated invariant — the rows sum to `SessionStats::cost` exactly. |
| ~~PROV-037~~ | ~~low~~ **CLOSED 2026-08-15** | not-ported | S | Two `auth-guidance.ts` formatters and the OAuth-expiry preflight branch unported — **CLOSED 2026-08-15**: sweep 11 — sweep 8's correction was right (ONE formatter missing, not two, and the fix site is `cyrup-session-svc`). `format_no_api_key_found_message` is in `auth_guidance.rs` **with the `UNKNOWN_PROVIDER` → "the selected model" carve-out** (`auth-guidance.ts:23`), alongside a factored `format_oauth_reauthenticate_message` for the string pi builds inline and identically at `agent-session.ts:1188-1192` and `:432-436`. The preflight at `session.rs` step 3 now reproduces `agent-session.ts:1182-1195` in full: no model ⇒ `NoModelSelected`; not configured ⇒ a live re-check before refusing; on refusal ⇒ the OAuth-expiry message when the stored credential is `oauth` (pi `isUsingOAuth`, `model-runtime.ts:368-370`), else `formatNoApiKeyFoundMessage`. Carried by a new `SessionServiceError::AuthPreflightRefused` whose `Display` is `{0}` so pi's text reaches the user unprefixed. **One `[CYRUP-DELTA]` recorded in-source**: the second chance refreshes the `AuthStore` snapshot and re-asks `has_configured_auth` rather than composing a whole `Models` per refusal — same observable difference (a credential written after startup now proceeds), and the ambient-auth gap it does not cover is identical before and after. |
| ~~PROV-038~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | Catalog roster guard is a tautology; 5 catalogs get no per-field checks — **CLOSED 2026-08-14**: sweep 1 — closed together. The `tests/` path in both items was stale: the file is `crates/cyrup-provider/src/tests/catalog_data.rs` (moved in 63d729a/c3982b5). Blind spot 1's "the roster test currently passes" is confirmed — it did, tautologically. |
| ~~PROV-039~~ | ~~low~~ **CLOSED 2026-08-14** | stale-port | S | `catalog_manifest.json` staleness floor predates the newest embedded data — **CLOSED 2026-08-14**: sweep 1 — closed together. The `tests/` path in both items was stale: the file is `crates/cyrup-provider/src/tests/catalog_data.rs` (moved in 63d729a/c3982b5). Blind spot 1's "the roster test currently passes" is confirmed — it did, tautologically. |
| PROV-040 | low | upstream-drift | M | `fetchDeferred`/`cancelDeferred` unported — **RE-CONFIRMED AT HEAD, NOT STARTED 2026-08-15 (sweep 11).** `rg 'fetch_deferred\|cancel_deferred' crates/` is still exactly 0 hits, so the item is accurate as filed; the data half remains fully ported. Size, measured rather than estimated: four seams (`ApiImpl` in `api/mod.rs`, `Provider` in `provider.rs`, `Models` in `collection.rs`, and `faux.rs` mirroring `providers/faux.ts:567-660`) plus `DeferredFetchOptions.wait` on the request options — the same shape and roughly the same reach as `PROV-S05`'s `RefreshModelsContext` threading, which took one focused pass. Note for whoever takes it: this is v0.84.1-only drift (`git show v0.83.0:packages/ai/src/types.ts \| grep fetchDeferred` is empty), so it lands as a documented forward-port with the same `[CYRUP-DELTA]` treatment `PROV-063` just received, or it waits for the v0.84.1 rebase. — **RE-READ 2026-09-14 at `9aeba769`, STILL OPEN (low); claim holds exactly as filed.** The behaviour half is absent from the ENTIRE workspace, measured not sampled: `git grep -iE 'fetch_deferred\|cancel_deferred' 9aeba769 -- crates/` returns rc=1, ZERO hits; `cyrup-provider`'s `provider.rs`, `collection.rs` and `api/mod.rs` contain no `deferred` identifier at all (`api/mod.rs:275` is an unrelated "construction really is deferred" comment). The data half still round-trips. Kind settled by TAG PRESENCE rather than by date: `fetchDeferred` is absent at v0.83.0 and present at v0.84.1/v0.85.1, so this is genuine post-baseline `upstream-drift`. Severity stays **low** for the reason this row gives — `packages/ai/src/providers/faux.ts:567,633,659-660,694-695` @v0.85.1 is STILL the only first-party implementor. **Two amendments that do not change the verdict.** **(a)** The cyrup citations above are stale PATHS: `message.rs` was split into a module, so `StopReason::Deferred` is now `cyrup-core/src/message/stop_reason.rs:100` (with `:95` admitting `fetchDeferred`/`cancelDeferred` are unported) and the handle is `cyrup-core/src/message/assistant.rs:71` (`pub deferred: Option<Box<DeferredHandle>>`) / `:111` (`pub struct DeferredHandle`); `cyrup-test-support/src/tests/deferred_interop.rs` should be re-pinned by whoever takes the item. **(b) ITEM GROWTH at v0.85.1 — the surface is no longer four seams.** The shell now carries two more: `packages/coding-agent/src/core/model-runtime.ts:654-678` (a `Models`-shaped `fetchDeferred`/`cancelDeferred` pair with its own provider-missing throw) and `provider-composer.ts:522-528` (the decorator must forward BOTH or a composed provider silently loses the capability — the same trait-default trap `PROV-M01` recorded). A port that stops at `ApiImpl`/`Provider`/`Models`/`faux` leaves the composer seam unported. Upstream re-pins @v0.85.1: `packages/ai/src/types.ts:227` (`DeferredFetchOptions`), `:236` (`DeferredCancelOptions`), `:275-280` (on `ProviderStreams`); `models.ts:143-148,222-227`, `:718-744` (the `Provider ${model.provider} does not support deferred responses` dispatch) and `:848-864` (multi-api composition). **Falsification:** any hit for `fetch_deferred`/`cancel_deferred` under `crates/` reopens the measurement; the severity floor rises the moment a non-faux upstream provider implements `fetchDeferred` (re-grep `git grep -l fetchDeferred <tag> -- packages/ai/src/providers/` — at v0.85.1 it is `faux.ts` alone). **RE-CONFIRMED STILL OPEN 2026-09-16 at `cc7818b`, re-measured rather than carried forward:** `git grep -n 'fetch_deferred\|cancel_deferred' -- crates/` is still **0 hits** workspace-wide. **RE-VERIFIED STILL OPEN 2026-09-24 at `ea23ca2` / pi `v0.87.1`:** `git grep -n 'fetch_deferred\|cancel_deferred' -- crates/` is 0 hits. Falsifier re-run: `git grep -l fetchDeferred v0.87.1 -- packages/ai/src/providers/ packages/ai/src/api/` returns `providers/faux.ts` and `api/lazy.ts` — the latter is the lazy-loading forwarder, not an implementor — so faux is still the only first-party implementor and the severity floor does not rise. **GROWTH 2026-09-24 (second pass):** a third method, `streamDeferred(model, handle, options)`, returns the lazy stream and `fetchDeferred` becomes `.result()` on it (`models.ts`, v0.84.4; still present @v0.87.1); `grep -rn "fn fetch_deferred\|fn stream_deferred" crates/` is still 0. Port all three together. |
| ~~PROV-041~~ | ~~low~~ **CLOSED 2026-08-15** | stale-port | S | False in-tree provenance citations, incl. a wrong "1:1 port" claim — **PARTIALLY CLOSED 2026-08-14**: sweep 1 + 2 — down to ONE live instance: `crates/cyrup-ext-subagents/src/extension.rs` ("PROV-003 — cyrup ships no login flow at all"), plus the unbuilt CI citation lint. Instance (1) was corrected by sweep 1, instance (3) was already gone, and the fourth instance sweep 1 folded in (cyrup-core's stale `raw_stop_reason` comment) is fixed — see PROV-012 in the status table. — **CLOSED 2026-08-15**: sweep 11. Two corrections to the row itself: the `cyrup-ext-subagents/src/extension.rs` instance was ALREADY fixed at HEAD (`:12558-12566` now records the correction explicitly), and instance (3) was **not** gone — `providers/openai_codex.rs` still claimed the `openai-codex-responses` impl was "not registered today … cannot yet stream" while `api::register_builtins` registers it; corrected and pinned by `prov041_openai_codex_responses_is_registered`. **The item's Verify clause was then executed rather than deferred**: all 126 `*models.ts:NNN` citations in `cyrup-provider` were re-resolved against `git show v0.83.0:packages/ai/src/models.ts`, and **20 were wrong at the tag they name** — `provider.rs` ×4 (`refresh_models` cited `:63`, which is `ModelsApiStreamOptions`; `stream_simple` cited `:71`, a prose line; `baseUrl`/`headers` off by one), `collection.rs` ×12 (four section headers, `getAuth` ×3 at `:216` = `mergeHeaders`' closing brace, the refresh-dispatch header repeating the exact `:198-214` error this item was filed for, `getSupportedThinkingLevels` ×3 at `:670` = the `return true` beneath the branch), `catalog.rs` ×3 (the catch-and-skip contract cited the `refreshModels?` docblock), `wire.rs` ×2 (`applyAuth` at `:240-241`/`:252`, both inside other functions) and `simple_options.rs` ×1. All corrected with the construct named and `@v0.83.0` stamped. **RESIDUAL, stated plainly:** the CI citation lint is still not built; the sweep above was manual. Its natural home is now the `xtask` PROV-018 landed (a `git show`-based extractor already exists there), ~S. |
| ~~PROV-043~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | Bedrock has no request retry where pi inherits the AWS SDK's default — **CLOSED 2026-08-14**: sweep 1 — Bedrock request retry and `AWS_BEDROCK_FORCE_HTTP1` ported. |
| ~~PROV-044~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | `AWS_BEDROCK_FORCE_HTTP1` unported; client negotiates h2 with no override — **CLOSED 2026-08-14**: sweep 1 — Bedrock request retry and `AWS_BEDROCK_FORCE_HTTP1` ported. |
| ~~PROV-045~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | openai-responses `reasoning` branch drops the xAI `include` and the summary-only trigger — **CLOSED 2026-08-14**: sweep 1 — landed together on both openai routes plus azure and codex, sharing one `SessionAffinityFormat`. PROV-034 additionally required CORRECTING a test that pinned `strict == false` with no compat (a new test-defect instance). |
| ~~PROV-046~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Boolean tool-arg coercion accepts `"True"`/`" true "` where pi rejects the call — **CLOSED 2026-08-14**: sweep 1 — confirmed at HEAD exactly as filed; citations resolved clean at v0.83.0. |
| ~~PROV-051~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Codex header-phase timeout substituted with a whole-stream read timeout; pi's message and abort/timeout distinction lost — **CLOSED 2026-08-14**: sweep 1 — Codex header-phase timeout restored with pi's message and the abort/timeout distinction. |
| ~~PROV-S04~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | `estimateContextTokens`' message-anchored added-tool accounting unported — **CLOSED 2026-08-14**: sweep 1 — CLASSIFICATION CORRECTED: the item said the added-tool block was post-baseline (3d8f7435) and cyrup was "a faithful stale port". `git show v0.83.0:packages/ai/src/utils/estimate.ts` has it at :118-133, byte-identical to v0.84.1. It was a port omission. Citation-sweep miss of the same class as the nine already recorded. **NOTE 2026-09-24 (second pass), not reopened:** v0.86.0 deleted the `addedToolNames` accounting this closure ported; `estimate.ts` now counts system messages' `toolsAdded`/`toolsRemoved`. Tracked in `PROV-083`. |
| ~~PROV-S05~~ | ~~low~~ **CLOSED 2026-08-15** | not-ported | M | `Models::refresh` has no `force`, no abort signal, no per-provider error map — **CLOSED 2026-08-15**: sweep 11 — `Models::refresh_with(provider, ModelsRefreshOptions)` returns `ModelsRefreshResult { aborted, errors }` and reproduces every clause of `models.ts:276-328` @v0.83.0 in upstream's order, including the two a shorter port drops: an ABORTED provider records **no** error (`:305`'s `if (!signal?.aborted)` guard — cancellation is not a provider failure), and any failure is followed by the `allowNetwork:false` cache-restore re-invocation whose own failure is swallowed (`:313-322`), built WITHOUT `force` exactly as upstream builds it. `Provider::refresh_models` now takes a `RefreshModelsContext` (pi `models.ts:34-44`), forwarded unchanged by both delegating decorators. **The abort was treated as guarantee-sensitive and is tested as two separate properties**, because a signal that is accepted and ignored is the failure mode this project keeps finding: `prov_s05_cancel_actually_aborts_an_in_flight_refresh` parks a provider inside its fetch, asserts the refresh has NOT settled, cancels, and requires it to return within 5 s (it can only do so via the token it was handed) with `aborted == true` and an EMPTY error map; `prov_s05_a_pre_cancelled_refresh_calls_no_provider` pins `:286`'s pre-check. `refresh(Option<&str>)` survives as the compatibility shape and its doc now says outright that `refresh(None) == Ok(())` for a wholly failed refresh is the hole, and that callers who care must use `refresh_with`. Two `[CYRUP-DELTA]`s recorded: `provider: Option<&str>` has no upstream counterpart, and `credential`/`store` are not threaded because the persisting fetcher owns both. The false "1:1 port" doc above the method (PROV-041 instance 1) was rewritten in the same edit. |
| PROV-053 | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | **FILED AND CLOSED IN THE SAME PASS (sweep 2).** `EnvAuthContext` diverged from pi's `defaultProviderAuthContext()` (ai/src/auth/context.ts:22-40 @v0.83.0) in two ways: `file_exists` did not expand a leading `~` (pi does, at :29-33), and `env` returned `Some("")` for a blank variable where pi returns undefined unless `value.trim().length > 0` (:24-25). The first made the Vertex ADC arm unreachable **on every machine** — `ctx.fileExists(VERTEX_ADC_PATH)` was always false — and was PROV-030's real hidden blocker. The second is the more general hazard: every precedence chain ported from a JS `??` or truthiness test reads `Some("")` as CONFIGURED, which INVERTS the upstream semantics; the concrete instance is `GOOGLE_CLOUD_API_KEY=""` winning the coalesce at providers/google_vertex.rs:301-304 and suppressing the ADC fallback. Both fixed in `auth/types.rs` with the pi citations inline and pinned by two tests. **Worth a sweep: any other `ctx.env(...)`-fed `??` chain in this crate may have been written around the old `Some("")` behaviour.** — **THAT SWEEP WAS RUN 2026-08-15 (sweep 11) AND CAME BACK CLEAN.** All 42 `ctx.env(...)` call sites in `cyrup-provider` were read against their upstream operator: every JS-`\|\|`/truthiness site filters empty explicitly (`auth/helpers.rs`, `env_api_keys.rs::get_provider_env_value`, `providers/anthropic.rs`, `utils/node_http_proxy.rs`, `auth/google_adc.rs::provider_env_value`), the `OverlayEnvContext` at `auth/resolve.rs:124-129` correctly falls THROUGH an empty overlay value (pi `env[name] \|\| base.env(name)`, `auth/resolve.ts:73-78`), and the two nullish-`??` chains — `providers/amazon_bedrock.rs` and `providers/google_vertex.rs` — deliberately let a stored `""` win the coalesce and then fail the surrounding truthiness test, with the reasoning written out above each. No second instance of the PROV-053 class exists in this crate. Recorded here so nobody re-derives it. |
| ~~PROV-054~~ | ~~**high**~~ **CLOSED 2026-08-15** | stale-port | S | `xai/grok-4.5` routed over the WRONG WIRE API on the xai DEFAULT model — **CLOSED 2026-08-15**: sweep 10 — closed BY THE REGENERATION, not by hand, exactly as this item's Fix required. `xai.json` now comes from `b0c2a90e` via `cargo run -p xtask -- gen-catalogs`: `api: openai-responses`, `compat {supportsLongCacheRetention:false}`, `thinkingLevelMap {off:null,minimal:null}`, and the five retired rows gone in the same write. Confirmed at the PORTED TAG too — `XAI_RESPONSES_MODEL_ID = "grok-4.5"` (`ai/scripts/generate-models.ts:378` @v0.83.0), selected at `:1408`, compat at `:390-392`, map at `:386-389`. **RE-AUDITED 2026-09-24 (second pass) — closure holds:** the 2026-09-14 supersession flag asked whether the v0.85 all-xAI-on-Responses rule moved the premise; every row of the embedded `xai.json` (live pi.dev fetch, XAI_1) is `openai-responses`. |
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
| ~~PROV-065~~ | ~~low~~ **CLOSED 2026-08-15** | cyrup-original | S | `openrouter-images.json` has no `*.models.ts` counterpart — **CLOSED 2026-08-15**: sweep 10 — the asymmetry is now encoded rather than noted: the generator's roster names `packages/ai/src/image-models.generated.ts` as this file's source and takes the `openrouter` sub-record, the manifest records that per-provider source path, and a test asserts it. Regenerating it from that file produced **zero** row or field differences, confirming this item's "verified exact". The other half of the asymmetry — `together.models.ts` has no catalog file because `providers/together.rs` hand-ports its rows — is recorded in the same roster. **COUNT CORRECTED 2026-08-19: it is 21 rows, not 20** — pi's 20 plus the `moonshotai/Kimi-K3` addition (`PROV-070`), which `xtask/src/main.rs:65` still describes as "20 rows". *(2026-09-28: `xtask/src/main.rs:110-112` now says 21, and K3 is pi's own served row, not an addition; `PROV-070` closed.)* |
| ~~PROV-066~~ | ~~low~~ **CLOSED 2026-08-15** | not-ported | S | `open_router_routing` typed `serde_json::Value` where pi declares a structured `OpenRouterRouting` — **CLOSED 2026-08-15**: sweep 11 — ported as `api::compat::OpenRouterRouting` (1:1 with `types.ts:660-727` @v0.83.0) with `deny_unknown_fields` on it and on every nested object, plus the four union arms as untagged enums (`sort` string-or-spec, `max_price` number-or-string per field, and both percentile cutoffs). **One correction to this item:** upstream declares **13** fields, not 11 — `zdr` and `enforce_distillable_text` were missed by the count. Two details a naive port loses and this one does not: the field names are the WIRE names (snake_case), NOT the enclosing `ModelCompat`'s `rename_all = "camelCase"`; and `sort.partition` is `string \| null`, a three-state, so it is `Option<Option<String>>` behind a present-key-only deserializer — a plain `Option<String>` maps `null` to `None` and `skip_serializing_if` then DELETES the user's explicit null from the request. Three tests: a full-population round trip, the misspelled-key rejection at top level and one level down, and the explicit-null survival. **Key-order note recorded in-source**: cyrup's `serde_json` has no `preserve_order`, so the old `Value` path emitted alphabetically and this emits in pi's declaration order — neither matches pi's insertion order, and no catalog sets the key, so the only producer is a user's `models.json`. |
| ~~PROV-067~~ | ~~low~~ **CLOSED 2026-08-15** | cyrup-original | S | The wire-api registry is a fn-pointer factory table where pi's laziness is per-module dynamic `import()` — **CLOSED 2026-08-15**: sweep 11 — the sign-off this item asked for is now a `[CYRUP-DELTA, mechanism]` block in `api/mod.rs`'s header, naming `api/*.lazy.ts` and `lazyApi` (`api/lazy.ts:66-75` @v0.83.0, verified present at the tag), stating why the substitution is forced (Rust has no dynamic `import()`, so there is no module-load event to defer and no import cache to share — the only deferrable thing left is construction of the impl value) and why it is equivalent (nothing on either side depends on module-load timing). Pinned by `prov067_registry_constructs_nothing_until_the_first_get`, which asserts the id set equals pi's 10 `KnownApi` entries, that `builtin_registry()` constructs nothing, that `contains` stays free, and that the first `get` constructs exactly one impl. **Stated loudly: that test cannot go red before this change** — it goes red if a factory is replaced by an eager `register_impl`, or if the id set drifts. |
| ~~PROV-068~~ | ~~high~~ **CLOSED 2026-09-04 — REFUTED** | port-bug | S | An explicit `null` in `thinkingLevelMap` read as UNSUPPORTED, collapsing most reasoning models to two rungs — **REFUTED, CLOSED 2026-09-04**: landed in cyrup `24b6ffe` (between this file's `4fb5e40` baseline and HEAD `2571969`), and independently re-verified here against the ported tag rather than taken from the commit message. `null` really does mean unsupported; the "no provider-specific value" reading this row proposed is what ABSENCE already means, and the two are kept distinct on both ends. **Upstream, read at `v0.83.0` (the file's own ported baseline, not just the commit's later citations):** `if (mapped === null) return false;` (`packages/ai/src/models.ts:668`); the wire gate that proves absence and null are different cases, `else if (model.thinkingLevelMap?.off !== null)` (`packages/ai/src/api/openai-completions.ts:774`) — an absent `off` still emits the generic level name, a null `off` suppresses the parameter outright. Two upstream tests pin the exact three-way reading: `{off,minimal,low,medium: null, xhigh: "max"}` → `["high","xhigh"]` (`packages/coding-agent/test/model-registry.test.ts:1012-1019` @v0.83.0) and `{xhigh: null, max: "max"}` → `[off,minimal,low,medium,high,max]` (`packages/ai/test/max-thinking.test.ts:59-66` @v0.83.0). And the Kimi K2.6 row this item's own report was about is upstream's, verbatim: `thinkingLevelMap: { minimal: null, low: null, medium: null }` (`packages/ai/test/together-models.test.ts:24` @v0.83.0) — a two-rung ladder is pi's own catalog behaviour for that model, not a cyrup defect. **cyrup at HEAD**, `crates/cyrup-provider/src/collection.rs::get_supported_thinking_levels`, unchanged in logic (`Some(None) => false`, matching `mapped === null`) but now carries this citation trail in its doc comment. `crates/cyrup-provider/src/providers/together.rs`'s `moonshotai/Kimi-K3` row keeps `thinking_level_map: None` (full ladder) as a documented, deliberate choice for a model with no upstream row (`PROV-070`), not a hedge on this item's resolution — inverting the reading would have changed 159 catalog models and put `off` on models that cannot disable reasoning. |
| ~~PROV-069~~ | ~~critical~~ **CLOSED 2026-08-15** | port-bug | S | The model's `max_tokens` never reached the request, so the server applied its own ceiling — **CLOSED 2026-08-15** in `e677555`. **Root cause:** `GenConfig::max_tokens` has no production writer (`grep -rn '\.max_tokens(' crates/ | grep -v tests` is empty; the builder at `agent.rs:2294` is test-only), and the body emitted the key only when that option was `Some`, so it was never emitted at all. **The catalog's `max_tokens` — all 1087 rows, regenerated from pinned `b0c2a90e`, reconciled field-by-field, covered by tests — was decorative.** **MEASURED against the live provider, not inferred:** the same prompt to `moonshotai/Kimi-K3` with `max_tokens` omitted returns `finish_reason: length` at `completion_tokens: 2048` (**Together's default cap**), of which **1438 were reasoning tokens** — leaving ~610 for the visible answer; with `max_tokens: 131072` it returns `finish_reason: stop` at 3135 tokens. Fix sends the caller's ceiling when present and the model's otherwise, which is upstream's own rule at `anthropic-messages.ts:989` (`options?.maxTokens ?? model.maxTokens`) and the stated intent of `adjustMaxTokensForThinking`; recorded as a `CYRUP-DELTA` because `openai-completions.ts:716` guards on the caller value alone. **Test note:** every other wire test hand-supplies `max_tokens: Some(...)`, proving serialisation and hiding the only path that ships — 7112 tests passed while the product sent no ceiling. Verified RED by removing the fallback. **Two wrong diagnoses recorded so they are not retried:** the agent loop is a faithful port (`agent.rs:593` == `agent-loop.ts:196-200`) and was never implicated; and a port of pi's `isRecoverableLength` was drafted and REVERTED because it routes a truncated turn into `run_auto_compaction`, which at 3% of a 1M window trades truncation for compaction spam — it treats the symptom. **`PROV-068` compounds this** (reasoning pinned to `high` burns the ceiling) but is a separate defect and remains open. |
| ~~PROV-070~~ | ~~low~~ **CLOSED 2026-09-28** | ~~cyrup-original~~ upstream-drift *(reclassified 2026-09-28: converged, because the row's own falsifier fired)* | S | **CLOSED 2026-09-28** (on `claude/lows-next`): **the falsifier fired.** `https://pi.dev/api/models/providers/together`, re-fetched 2026-09-28 (22 rows), serves `moonshotai/Kimi-K3`, so K3 is pi's row, not a cyrup addition, and cyrup now carries pi's values field for field at `crates/cyrup-provider/src/providers/together.rs:264-274`: name `Kimi K3`, reasoning, text+image, `cost(3.0, 15.0, 0.3)` (cache-write 0), `1_048_576` context, `131_072` max_tokens, `Some(m())` (the three-null `{minimal, low, medium}` map, so the ladder is `off`/`high` like its K2.x siblings) and the same `together_compat(false, Some(ThinkingFormat::Together))` block. The `ADDITIONS` machinery and the 21-line provenance comment are gone. A six-line comment at `:258-263` now says K3 is pi's served row and the one row the `b0c2a90e` literal predates, and the function doc at `:94-98` says the same. The roster test is renamed back to `full_catalog_ported_from_pi` (`:428`) and asserts `models.len() == 21` (`:432`), so any extra row fails. `kimi_k3_is_pi_s_served_row` (`:490`) reads K3 through `together_provider().get_model` and pins name, reasoning, image input, context, max tokens, all four costs, the three explicit nulls, `get_supported_thinking_levels == [Off, High]` and the compat block. The lane reports it red without the fix. The one-line residual is closed too: `xtask/src/main.rs:110-112` now says the roster is 21 rows, the 20 that `together.models.ts` declares at `b0c2a90e` plus K3 from pi's served catalog. **LEDGER CORRECTION 2026-09-28:** reclassified from `cyrup-original` to converged `upstream-drift`, as the falsifier prescribes. The hand-measured row was wrong on two fields: pi's `contextWindow` is `1_048_576`, not the `1_000_000` measured from Together, and its `thinkingLevelMap` is the three-null map, not absent. pi.dev's Together roster also differs from `b0c2a90e` in other rows (it adds DeepSeek-V4-Flash-0731, V4-Pro-0813, V4.1-Flash, GLM-5.3, GLM-5.3-Flash and `thinkingmachines/Inkling`, and drops the Qwen3-235B, Qwen3.5-397B, Rnj-1, GLM-5 and GLM-5.1 rows). That drift is `PROV-071`'s, not this row's. **Not this row's either:** pi's K3 row also carries `inputLimits.images.resize` (2000x2000, 4718592 bytes, jpegQuality 80), and cyrup's `Model` has no `inputLimits` field at all; that is area 05 `CFG-085`. **Gate status:** lane-verified only. The combined integration pass ran `cargo fmt --all -- --check` (clean) but not clippy or nextest (`/` had 2.9 GB free). *Original filing follows.* — **`moonshotai/Kimi-K3` is a Together roster row cyrup ships and pi does not — the first deliberate ADDITION to a ported provider catalog, and it had no row in this file.** Landed `2add245` (2026-08-15). The model is `providers/together.rs:278-288` (`cost(3.0, 15.0, 0.3)`, `1_000_000` context, `131_072` max_tokens, `together_compat(false, Some(ThinkingFormat::Together))`), under a 21-line provenance comment at `:257-277` recording that every value except `max_tokens` was MEASURED from `GET https://api.together.xyz/v1/models` on 2026-08-15 and that `131_072` was verified with a live request that returned `finish_reason: stop`. Upstream's `together.models.ts` @`b0c2a90e` stops at K2.7-Code — K3 shipped 2026-07-26, after the pinned revision — so this is a signed-off divergence, not drift. **It is GUARDED, which is why the severity is `low`**: sweep 10's roster test was renamed and widened to `full_catalog_ported_from_pi_plus_recorded_additions` (`:442`), asserting `models.len() == 20 + ADDITIONS.len()` (`:448`) against `const ADDITIONS: &[&str] = &["moonshotai/Kimi-K3"]` (`:447`), so an ACCIDENTAL extra row still fails while a NAMED one does not. **NOT a regeneration hazard — checked, against the obvious assumption from `PROV-018`/`PROV-060`:** `together` has no entry in the generator's `CATALOGS` at all (`xtask/src/main.rs:65-66`: "cyrup hand-ports Together's rows as Rust literals … so this generator cannot own them") and the manifest note restates the exception (`xtask/src/main.rs:588`), so nothing regenerates this roster and the `DELTAS` table is the wrong instrument for it. **The live hook is `PROV-068`, which is still open at `high`:** this row's `thinking_level_map` is `None` (`providers/together.rs:286`) where both Kimi siblings — K2.6 (`:247`) and K2.7-Code (`:290`) — pass `Some(m())`, and `m` (`:100`) is `level_map(&[("minimal", None), ("low", None), ("medium", None)])`, i.e. the three explicit nulls `get_supported_thinking_levels` (`collection.rs:787-810`) reads as UNSUPPORTED — `Some(None) => false` at `:800`. `None` was chosen deliberately so K3 keeps the full ladder while `PROV-068` is open, and the reasoning is in-source at `providers/together.rs:272-277`. **`PROV-068` must revisit this row either way it resolves** — if explicit-null comes to mean "supported, send no provider value" the asymmetry is pointless; if it keeps meaning "unsupported" then the siblings are the bug and K3 is the template. **RESIDUAL, one line:** `xtask/src/main.rs:65` still says cyrup "hand-ports Together's **20** rows"; it is 21 (re-verified 2026-09-04: now at `:74`, text unchanged). — **`PROV-068` RESOLVED 2026-09-04, REFUTED — this row's "either way" is settled, not pointless.** `PROV-068` closed on the "unsupported" reading, and the siblings are NOT the bug: `together-models.test.ts:24` @v0.83.0 shows the K2.6 map `{minimal: null, low: null, medium: null}` is pi's own catalog data, not a cyrup invention, so K2.6/K2.7-Code's two-rung ladder is a correct port. K3's `thinking_level_map: None` therefore stays exactly what `together.rs`'s comment (updated in the same commit, `24b6ffe`) now says: a deliberate cyrup-original choice for a model with no upstream row to copy, not a hedge against an open question. This item's own severity and scope are otherwise unchanged — it is still the one-line `xtask` residual above. — **RE-READ 2026-09-14 at `9aeba769`, STILL OPEN (low); the item is exactly its one-line residual and nothing has been done to it.** The generator's doc comment still says Together is **20** rows where cyrup ships 21; the text is identical and only the offset moved again (`:65` → `:74` → now `xtask/src/main.rs:98`). The guarded-addition machinery this row credits for the low severity is intact, and what it ENFORCES was read rather than assumed: `crates/cyrup-provider/src/providers/together.rs:449-459` (`full_catalog_ported_from_pi_plus_recorded_additions`) asserts `models.len() == 20 + ADDITIONS.len()` with `ADDITIONS = ["moonshotai/Kimi-K3"]` and that each named addition is present, so an ACCIDENTAL extra row still fails; the file has 21 `model(` rows and the K3 row is at `:286` with `thinking_level_map: None`, under the 21-line provenance comment at `:257-284` that now carries `PROV-068`'s resolution. The `cyrup-original` classification also survives: `together.models.ts` @`b0c2a90e` has exactly 20 `id: "..."` rows and `grep -n Kimi-K3` there is empty, and `git grep -n Kimi-K3 v0.85.1 -- packages/` returns nothing. **IMPORTANT LIMIT on that last point — it is `PROV-071`'s, not a new defect:** at v0.85.1 `together.models.ts` is an 8-line re-export of gitignored `./data/together.json`, so pi's CURRENT Together roster is not measurable from git at all; "pi does not ship K3" is provable only against `b0c2a90e`. Whoever fixes the xtask line should write 21 **and** say which revision the 20 is counted from. **Falsification:** if pi's Together data ever becomes readable from this workspace (a `data/together.json` landing in git, or a reachable `https://pi.dev/api/models/providers/together`) and it contains `moonshotai/Kimi-K3`, this row reclassifies from `cyrup-original` to converged `upstream-drift`, and `together.rs`'s `ADDITIONS` entry plus its provenance comment must be retired in the same change. Closing the residual requires `xtask/src/main.rs:98` to read 21 (or to stop asserting a count). **RE-CONFIRMED STILL OPEN 2026-09-16 at `cc7818b`, re-measured:** `moonshotai/Kimi-K3` is still shipped in the Together roster — `crates/cyrup-provider/src/providers/together.rs:286`, and still declared as a deliberate addition at `:454` (`const ADDITIONS: &[&str] = &["moonshotai/Kimi-K3"]`). The `ADDITIONS` const is the mechanism this row asked for and it is intact; what stays open is the upstream-facing question, not the bookkeeping. **RE-VERIFIED STILL OPEN 2026-09-24 at `ea23ca2`:** cyrup side byte-identical: `git diff --quiet 9aeba769 ea23ca2 -- crates/cyrup-provider crates/cyrup-core crates/cyrup-config xtask` exits 0; `xtask/src/main.rs:98` still reads "Together's 20 rows" and `providers/together.rs:454` still declares `ADDITIONS = ["moonshotai/Kimi-K3"]`. pi's Together data remains unreadable at v0.87.1 (`together.models.ts` is a `data/*.json` re-export), so the falsifier cannot fire. |
| ~~PROV-071~~ | ~~medium~~ **CLOSED 2026-09-29** | tooling | L | **Every embedded catalog is frozen at `b0c2a90e` for the same structural reason — this was never an xai defect** — 39/39 of pi's `*.models.ts` modules at HEAD `71dca871b` are the post-`a9f6a3159` re-export of gitignored data, so `git show` recovers nothing for ANY provider at any revision after `b0c2a90e`. `XAI_1` unfroze ONE (`xai`) by fetching `https://pi.dev/api/models/providers/<id>`, which serves rows pre-shaped into cyrup's native `Model` JSON — a materially lower-effort route than porting `generate-models.ts`'s per-provider transform. The other 34 are NOT fixed by XAI_1..XAI_4 and are tracked here as unscheduled follow-up. Amends `DRIFT-009`, whose bolded prohibition on seeding from pi.dev was scoped to the blocked four (where `git show` WAS available) and does not reach a provider for which the pinned path is provably dead. See also `PROV-018`, `PROV-039`, `PROV-060`. — **RE-READ 2026-09-14 at `9aeba769`, STILL OPEN (medium); the claim holds unchanged on BOTH halves at the newer tag.** **Upstream, the structural cause has not reverted**, measured mechanically rather than sampled: all 39 `packages/ai/src/providers/*.models.ts` at v0.85.1 contain `with { type: "json" }` — 39/39 are the re-export shape, ZERO are data literals — while `.gitignore:11` @v0.85.1 is `packages/ai/src/providers/data/` and `git ls-tree -r v0.85.1 -- packages/ai/src/providers/data` is empty, so `git show` recovers nothing newer for ANY of the 39. By contrast `together.models.ts` @`b0c2a90e` is still a full 20-row literal, confirming `b0c2a90e` remains the only reachable data revision. **cyrup side:** `xtask/src/main.rs:70` still pins `DEFAULT_REV = "b0c2a90e"`, `:74` `DEFAULT_REV_TIMESTAMP = "2026-07-17T09:00:03Z"`, `:103-140` CATALOGS (34 pinned specs incl. the openrouter-images special case), and `:156-162` `LIVE_CATALOGS` still holds exactly ONE entry (`xai`) under its own comment stating the other 34 are affected by the same upstream change and are deliberately NOT there; `crates/cyrup-provider/src/providers/catalog/` holds 35 files and `catalog_manifest.json:1-3` still stamps `"generatedAt": "2026-07-17T09:00:03Z"` / `"source": "pi@b0c2a90e"`, 34-of-35 from the pinned revision. Nothing in `XAI_1..XAI_4` touched the other 34, precisely as this row says. Severity stays **medium**: the embedded floor is now ~2 months stale for 34 providers, and a stale row degrades to confidently-wrong rather than to missing. **FRESH MEASUREMENT, dated as this row demands and NOT inherited:** from this workspace on **2026-09-14**, `https://models.dev/api.json` and `https://pi.dev/api/models/providers/xai` BOTH fail — the agent proxy reports `connect_rejected`, HTTP code 000 — the opposite of the 200/200 recorded on 2026-09-13, and back to `DRIFT-009`'s 403. The demonstrated fix route is therefore not executable from here today, which STRENGTHENS rather than weakens the item. **Checked and NOT reportable as a new defect:** a blocked fetch does not break the build — `xtask/src/live_catalog.rs:189-221` maps a fetch error to `LiveOutcome::Skipped` and leaves the existing file, so `gen-catalogs` degrades to the pinned path rather than failing. **Falsification, two measurable conditions.** **(a) The upstream cause:** re-run the 39-module shape scan at the next tag — if any `*.models.ts` returns to a data literal, or `packages/ai/src/providers/data/*.json` appears in `git ls-tree`, the pinned path is alive again and this row's premise dies. **(b) The cyrup side:** the row closes only when `LIVE_CATALOGS` (or an equivalent) accounts for all 35 catalogs with per-provider `fetchedAt`/`revision` in `catalog_manifest.json` **and** `fleet.rs`'s `EXPECTED_COUNTS` assertions have been rewritten in the same change — a live-fetch generalization landing without that assertion rewrite is a regression, not a closure. Network reachability must be re-measured and re-dated on every pass: it flipped 200 → blocked in one day here. **RE-CONFIRMED 2026-09-16 at `cc7818b` by identity** (`crates/cyrup-provider` unchanged in `9aeba769..cc7818b`; `xtask/` likewise untouched, so the generator and its `--check` drift guard are as read on 2026-09-14). **This is area 01's only open medium** and the `b0c2a90e`-plus-unbounded-delta residue is unchanged. **RE-VERIFIED STILL OPEN 2026-09-24 at `ea23ca2` / pi `v0.87.1`:** cyrup side byte-identical: `git diff --quiet 9aeba769 ea23ca2 -- crates/cyrup-provider crates/cyrup-core crates/cyrup-config xtask` exits 0. Falsifier (a) re-run at v0.87.1: **41/41** `*.models.ts` (now incl. `meta.models.ts`, `radius.models.ts`) are re-exports and `git ls-tree -r v0.87.1 --name-only packages/ai/src/providers/data` is empty, so the pinned path is still dead. The rows this staleness now hides include v0.87.1's Opus 5.5 / GPT-6 Sol / GPT-6 Luna / Grok 4.7 (xai default) additions and v0.86.0's removal of GPT-5.4/-mini from Codex (per `packages/ai/CHANGELOG.md`, not a field diff). **GROWTH 2026-09-24 (second pass), read from `scripts/generate-models.ts` `v0.85.1..v0.87.1` (the script is in git; the data it writes is not):** hard-coded values the frozen floor lacks — `OPENAI_STANDARD_COSTS` now pins `gpt-5.6-sol` at 4/20/0.4/5 (cyrup `openai.json`/`openai-codex.json`: 5/30/0.5/6.25) and adds `gpt-6-sol`/`gpt-6-luna`/`gpt-6-astra`; DeepSeek's hand-written rows reprice `deepseek-v4-pro` to 1.32/3.96/0.044 (cyrup: 0.435/0.87/0.003625, about 3x under) and replace `deepseek-v4-flash` with `deepseek-flash` (V4.1, 0.3/1.2/0.006); `claude-opus-5-5` and Copilot `claude-opus-5.5`/`gpt-6-*` rows; Vercel `allowEmptySignature:true` on every row (#9676); Fireworks Messages `allowEmptySignature` + catalog-driven `forceAdaptiveThinking` (#9323); Copilot `gpt-*` to Responses; `supportsStrictMode` as explicit metadata (`PROV-078`); `supportsMidConvo*`, `promptCache`, `inputLimits` (`PROV-083`/`PROV-091`); `deferredToolsMode` removed. **Online these rows arrive through the pi.dev overlay, which cyrup applies per provider (`remote_catalog.rs`); the floor is what an offline or first-run user prices against** — why this stays `medium` and was not split into per-price items. — **CLOSED 2026-09-29**: the embedded catalogs are regenerated against live sources: `LIVE_CATALOGS` went 5 -> 38 and `CATALOGS` 34 -> 1 (only `openrouter-images`, which is not a provider module and whose live endpoint 404s), so 38 of the 39 embedded catalogs now come from `https://pi.dev/api/models/providers/<id>` instead of `pi@b0c2a90e`. `LiveCatalogSpec` collapsed to `{file,item}` with provider/url/module derived from the stem, so a mismatched endpoint is unspellable; `catalog_manifest.json` carries per-provider `fetchedAt`/`revision`; `fleet.rs::EXPECTED_COUNTS` was rewritten and is asserted TOTAL (the refresh moved openrouter 271->393, amazon-bedrock 109->174, moonshotai 10->4 -- 536 rows in, 204 out, 845 field values across 27 catalogs); and `FleetWire::api()` became `apis()` with a `MessagesAndCompletions` variant because `openrouter` is a two-API provider at v0.87.1 (`packages/ai/src/providers/openrouter.ts:8,21-25`) where it was single-API at `b0c2a90e`. Every signed-off divergence has CONVERGED -- 9 now pin a value upstream already carries, 3 name rows upstream retired -- so `DELTAS` is empty with its machinery intact and each entry moved to a `CONVERGED` table that inverts it (`Expect::Carries` / `Expect::Retired`), making a regression a hard error at generation time. `gen-catalogs --check` exits 0 over all 40 files compared and `--roster v0.87.1` accounts for 41 of 41 upstream modules. Verify: `xtask::tests::every_converged_divergence_still_holds_in_the_shipped_catalogs`, `xtask::tests::the_v0_87_1_provider_roster_is_fully_accounted_for`, `xtask::tests::the_catalog_roster_is_the_39_embedded_files`, `cyrup-provider providers::fleet::tests::every_catalog_parses_with_expected_count`, `cyrup-provider tests::catalog_data::the_catalog_manifest_names_one_revision_per_provider`, `cyrup-provider tests::catalog_data::every_model_the_regeneration_added_now_resolves`. |
| ~~PROV-072~~ | ~~low~~ **CLOSED 2026-09-28** | cyrup-original | S | **CLOSED 2026-09-28** (on `claude/lows-next`): **deleted, as the row's default says.** `crates/cyrup-provider/src/auth/oauth/load.rs` is gone (347 lines), and so are its `pub mod load` and `pub use load::{…}` block in `auth/oauth/mod.rs`, the `register_bundled_oauth_flow_loaders` re-export in `auth/mod.rs:16-20`, `OAuthError::FlowUnavailable`, and the registry-shaped `github_copilot_oauth_flow`/`kimi_coding_oauth_flow` factories with their tests (`auth/oauth/github_copilot.rs`, `auth/oauth/kimi_coding.rs`). `RadiusOptions` moves to `auth/oauth/radius.rs:87-93` as the port of `RadiusOAuthOptions` (`radius.ts:352-355` @v0.87.1, shape unchanged: `name`, `gateway`). The code now says why `load.ts` has no port: `providers/builtin_oauth.rs:25-37` explains that `bun-oauth.ts:12-23` registers every flow as a constant and a static Rust binary is always that fully bundled build, and `providers/github_copilot.rs:146-152` cites `githubCopilot: () => githubCopilotOAuth` at `bun-oauth.ts:16`. **The row's Verify, re-run:** `grep -rn` over `crates/` and `xtask/` for `register_bundled_oauth_flow_loaders`, `registered_oauth_flows`, `FlowUnavailable`, `OAuthFlowFactory`, `OAuthFlowId`, `OAuthFlowLoaders`, `RadiusFlowFactory`, every `load_*_oauth`, `github_copilot_oauth_flow` and `kimi_coding_oauth_flow` finds only the prose `registerBundledOAuthFlowLoaders` in `builtin_oauth.rs:28`, a citation of upstream. Tests: the `cyrup-provider` suite; `providers::builtin_oauth::tests::{only_the_five_built_ins_carry_oauth, subscription_split_matches_upstream}` still pin the direct-construction path. No new test, since this is a deletion. **Gate status:** lane-verified only (fmt clean; clippy/nextest not run on the combined tree). **LEDGER CORRECTION 2026-09-28:** the verifier corrected two docs as the lane first drafted them. `github_copilot.rs` cited `bun-oauth.ts:17` for `githubCopilot`, but `:17` is `openrouter`; `githubCopilot` is `:16`. `builtin_oauth.rs` said its match "IS `bun-oauth.ts`'s table"; it now says the match together with the two self-wired flows (`github-copilot`, `openai-codex`) is that table, less `meta`, which is `PROV-080`'s. **Remaining, not this row's:** `PROV-080` (meta OAuth) adds a `builtin_provider_oauth` arm; there is no registry to extend. `docs/PARITY-PLAN.md:595` still mentions `load.rs:111` (a planning doc outside the ledger). The `load.rs` citations in this file's `PROV-029` status row and body are history and are left as written. *Original filing follows.* — **The bundled OAuth flow-loader registry is dead — a complete, tested, documented subsystem with no production caller** — **FILED 2026-09-22 at `14e6c56`**, by the `PARITY-GAPS.md` `UW-` sweep, which `UW-11` had explicitly asked a later pass to run: *"`register_bundled_oauth_flow_loaders` (`auth/oauth/load.rs:111`) was not re-greped this pass. If it still has zero callers it is a live UW-class defect with no id."* It does, and it is worse than the registrar alone — the registry is dead on BOTH sides. **Write side:** `grep -rn 'register_bundled_oauth_flow_loaders' crates/ --include=*.rs` returns the definition (`crates/cyrup-provider/src/auth/oauth/load.rs:111`), two re-exports (`auth/oauth/mod.rs:62`, `auth/mod.rs:19`), four doc references (`load.rs:12`, `:64`, `auth/oauth/github_copilot.rs:838`, `auth/oauth/kimi_coding.rs:766`) and three CALLS — `load.rs:239`, `:277`, `:322` — **all three after the `#[cfg(test)]` that opens at `load.rs:198`** (`grep -n '#\[cfg(test)\]' crates/cyrup-provider/src/auth/oauth/load.rs` → `198`). Zero production callers. **Read side:** `grep -rn 'registered_oauth_flows' crates/ --include=*.rs \| grep -v 'auth/oauth/load.rs'` returns **nothing** — `registered_oauth_flows` (`load.rs:118`), whose own doc says it exists *"for status UIs that list which logins are actually available"*, has zero callers outside its module, and no status UI reads it. The backing `registry()` (`load.rs:104`) is therefore written by nobody and read only by its own two accessors. **Severity `low`, deliberately:** nothing user-visible breaks. `/login` resolves through `provider.provider_auth().oauth` (`cyrup-config/src/login.rs`), not through this registry, and `PROV-029` closed the flows-unreachable defect by wiring the provider side. This is dead weight plus a doc comment that promises a consumer that does not exist. **Fix:** discharge the second half of `UW-11`'s Fix as written — *either populate the flow registry or delete it*. Deleting is the smaller change and is the default unless a status UI is actually planned; if it is kept, the `"for status UIs"` doc must name the caller. **Verify:** after the fix, `grep -rn 'registered_oauth_flows' crates/ --include=*.rs` either has a production hit outside `load.rs`, or has none because the symbol is gone. A pass that leaves it hit-only-from-tests has not closed this. **RE-VERIFIED STILL OPEN 2026-09-24 at `ea23ca2`:** cyrup side byte-identical: `git diff --quiet 9aeba769 ea23ca2 -- crates/cyrup-provider crates/cyrup-core crates/cyrup-config xtask` exits 0; outside `auth/oauth/load.rs`, `register_bundled_oauth_flow_loaders` appears only in the two re-exports and two doc comments, and `registered_oauth_flows` has no hit at all. Upstream note: `auth/oauth/load.ts` @v0.87.1 added a `meta` loader to the bundled set — relevant only once `PROV-080` lands. |
| ~~PROV-073~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | **Mistral streaming tool calls are keyed `{callId}:{index}`, so a continuation chunk that omits the id opens a SECOND block and splits the arguments** — pi v0.84.4 (#8387) keys on `index ?? callId`. **FILED 2026-09-24** from the census lead; body below. — **CLOSED 2026-09-27**: the block key is `index:{i}` when the chunk carries an index, `call_id` otherwise (`api/mistral_conversations/blocks.rs:45-48`). Verify: `api::mistral_conversations::tests::decode::prov073_an_id_less_continuation_chunk_appends_to_the_indexed_block`. |
| ~~PROV-074~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | **DeepSeek requests send `max_completion_tokens`, which pi stopped sending at v0.84.2 in favour of `max_tokens`; DeepSeek detection is also case-sensitive** — the same expression and failure family as the closed `DRIFT-013`. **FILED 2026-09-24**; body below. — **CLOSED 2026-09-27**: `is_deepseek` is lower-cased and feeds `use_max_tokens` (`api/compat.rs:640,649,673`). Verify: `api::compat::tests::prov074_deepseek_uses_max_tokens_through_every_detection_route`. |
| ~~PROV-075~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | **Google / Vertex: any finish reason with a tool call present becomes `toolUse` and the error is cleared** — pi v0.84.2 overrides only when the mapped reason is `stop`, so a `MAX_TOKENS`/`SAFETY` turn keeps its `length`/`error`. **FILED 2026-09-24**; body below. — **CLOSED 2026-09-27**: the tool-call override is gated on the mapped reason being `Stop` (`api/google_generative_ai/parts.rs:76-81`) and `error_message` is no longer cleared. Verify: `api::google_generative_ai::tests::decode::a_non_stop_finish_reason_survives_a_tool_call`. |
| ~~PROV-076~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `parse_usage` now reads Kimi's top-level `usage.cached_tokens` as the third cache-read fallback, in pi's order: `prompt_tokens_details.cached_tokens`, then `prompt_cache_hit_tokens`, then `cached_tokens` (`api/openai_completions/finalize.rs:52-57`). A JSON `null` falls through each rung, as pi's `??` does. Upstream `parseChunkUsage`, `openai-completions.ts:1511-1550` @v0.87.1 (#8075, v0.84.3). Verify: `api::openai_completions::tests::decode::kimi_top_level_cached_tokens_count_as_cache_reads` (red without the fix). — *Original:* **Kimi's top-level `usage.cached_tokens` is not read as cache reads** — pi v0.84.3 (#8075) added it as the third fallback; cyrup mis-costs Kimi turns. **FILED 2026-09-24**; body below. |
| ~~PROV-077~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): Anthropic OAuth requests now send `user-agent: claude-cli/2.1.280`. `CLAUDE_CODE_VERSION` is `2.1.280` (`api/anthropic_messages/headers.rs:25`, used at `:138`), and its doc comment records the history checked against the tags: `2.1.75` at v0.84.4, `2.1.251` from v0.85.0 through v0.87.0, `2.1.280` at v0.87.1 (`anthropic-messages.ts:87`, `:952`). Verify: `api::anthropic_messages::tests::headers::oauth_headers_use_bearer_and_identity`, which now asserts the exact value (red without the fix). — *Original:* **Anthropic OAuth `user-agent` still claims `claude-cli/2.1.75`** — pi moved it to `2.1.251` (v0.85.0) and `2.1.280` (v0.87.1). **FILED 2026-09-24**; body below. |
| ~~PROV-078~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | S | **Unknown OpenAI-compatible endpoints are still detected as strict-capable, so every tool carries a `strict` key** — pi v0.87.0 (#9816) defaults runtime detection to `supportsStrictMode: false` and moves the built-in "capable" set into generated metadata. **FILED 2026-09-24**; body below. **GROWTH 2026-09-24 (second pass):** v0.86.1 `af7359b90` (#9804) also excludes Cerebras (`isCerebras` in detection) because mixed strict/non-strict tools 400 there; cyrup's `detect_compat` does not, and `cerebras.json` rows carry no override — so a BUILT-IN provider still receives the key. Same expression, same fix. — **CLOSED 2026-09-29**: `supportsStrictMode` is explicit upstream metadata rather than a runtime guess. The five endpoint predicates live once in `api/compat.rs`'s `mod endpoint` (:686-716) and feed both `detect_compat` -- whose runtime default is now a flat `false` at :860, matching pi's own "OpenAI compatibility alone does not imply strict JSON-schema tool support" in `packages/ai/src/api/openai-completions.ts` -- and `generated_supports_strict_mode` (:730-736), which `catalog.rs::load_catalog` applies at parse time in the generator's shape: `packages/ai/scripts/generate-models.ts`'s moved `supportsStrictMode: !isMoonshot && !isTogether && !isCloudflareAiGateway && !isNvidia && !isCerebras`, a key written only when it differs from the `false` default (pi's delta writer), and the `{...detected, ...model.compat}` merge order so a declared value is never overwritten. The resolved value was measured identical to pi's own published artifacts in 130/130 openai-completions rows, covering all five excluded endpoint families. Verify: `api::compat::tests::an_unknown_openai_compatible_endpoint_is_not_strict_capable`, `catalog::tests::builtin_openai_completions_rows_carry_strict_as_metadata`, `catalog::tests::cerebras_rows_are_excluded_like_pi_9804`. |
| ~~PROV-079~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `user_content` skips an empty `Content::Text` block when it builds a content array (`api/openai_completions/convert.rs:357`) and keeps the other parts in order, as pi's `.filter` does (`openai-completions.ts:1260-1281` @v0.87.1, `1b6ddca87`, #9797). An image sent with no prompt text no longer carries `{"type":"text","text":""}`. An array left empty after filtering is skipped by the existing caller (`convert.rs:55-58`), matching pi's `if (content.length === 0) continue`. Verify: `api::openai_completions::tests::params::empty_text_parts_are_dropped_from_a_user_content_array` (red without the fix). — *Original:* **Image-only user messages on openai-completions carry an empty `{"type":"text","text":""}` part** that some providers reject — pi v0.87.1 (#9797) filters empty text parts. **FILED 2026-09-24**; body below. |
| PROV-080 | low | upstream-drift | M | **The `meta` provider (Meta Model API key + Muse subscription device-code OAuth) is unported** — added to pi's `builtinProviders()` at v0.86.1. **FILED 2026-09-24**; body below. |
| ~~PROV-081~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `handle_metadata` sums the `inputTokens` of every `usage.cacheDetails[]` entry with `ttl: "1h"` into `cache_write_1h` before cost is computed (`api/bedrock_converse_stream/events.rs:395-407`), so one-hour writes are now priced at the one-hour rate. It stays `None` when `cacheDetails` is absent (pi's `?.reduce` yields `undefined`); an empty array gives `Some(0)`. Upstream `bedrock-converse-stream.ts:702-719` @v0.87.1 (#9457, v0.86.0). Verify: `api::bedrock_converse_stream::tests::decode::one_hour_cache_writes_are_read_from_cache_details`, which checks the cost against pi's `calculateCost` (`models.ts:900-920`): short writes at the `cacheWrite` rate plus long writes at twice the input rate (red without the fix). — *Original:* **Bedrock never populates `cache_write_1h`, so one-hour cache writes are priced at the five-minute rate** — pi v0.86.0 (#9457) sums `cacheDetails` with `ttl === 1h`. **FILED 2026-09-24**; body below. |
| ~~PROV-082~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | **Overflow classification: z.ai's `Prompt too long` is missed, and a bodyless `400`/`413` from ANY provider is classified as context overflow** — pi v0.86.0/v0.86.1 (#9482, #9805) widen the Anthropic pattern and gate the bodyless pattern on `provider === "cerebras"`. **FILED 2026-09-24**; body below. — **CLOSED 2026-09-27**: the Anthropic/z.ai pattern is widened to `prompt (?:is )?too long` and the bodyless `400`/`413` pattern is split out behind `provider == "cerebras"` (`utils/overflow.rs:16,46,103-107`). Verify: `utils::overflow::tests::zai_prompt_too_long_and_provider_gated_bodyless`. |
| ~~PROV-083~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | L | **pi v0.86.0 moved the system prompt and every tool change into the transcript (`TranscriptContext`, `role:"system"` messages with `toolsAdded`/`toolsRemoved`/`sections`); cyrup still carries v0.85's `addedToolNames` deferred-tool model, whose Anthropic `tool_reference` path, `deferredToolsMode:"kimi"` flag and `addedToolNames` estimate accounting upstream DELETED** — so the pre-v0.86 wire shapes cyrup emits (tool_reference in tool results, `tool_search_call` anchored at a tool result) are ones pi no longer produces, and pi's replacements (`tool_addition`/`tool_removal`, native mid-conversation system messages, fold-back for models that do not accept them) have no cyrup counterpart. **FILED 2026-09-24 (second pass)**; body below. — **CLOSED 2026-09-29**: the v0.86.0 mid-conversation shapes are ported. `SystemMessage` carries pi's declaration order (`role`, `content: string \| TextContent[]`, `sections`, `toolsAdded`, `toolsRemoved`, `timestamp`, `packages/ai/src/types.ts`), pinned by a hand-written serializer; `api/anthropic_messages/compat.rs:100-156` hand-rolls `generate-models.ts`'s two anchored model regexes and `getAnthropicMessagesCompat`'s provider gates (`anthropic` sets both `supportsMidConvoSystemMessages` and `supportsMidConvoToolChanges`; `opencode` and `github-copilot` only the former, under upstream's own comment about those proxies rejecting tool_addition/tool_removal blocks) as runtime predicates, because not one of the 39 embedded catalogs declares any `supportsMidConvo*` key and a constant `false` would have shipped the port as dead code; `cyrup-session`'s cut-point predicate became pi's explicit allow-list (`agent_message.rs:144-153` against `isCutPointMessage` in `packages/coding-agent/src/core/compaction/compaction.ts`), replacing an inverted `!matches!(self, ToolResult)` so a system message can no longer become a legal compaction cut point; and `api/openai_completions/transform.rs` holds a system message back past an open tool-call/tool-result pair instead of emitting it in place. Verify: `api::anthropic_messages::tests::mid_convo::mid_convo_system_messages_default_matches_upstreams_two_anchored_regexes`, `::provider_gates_split_system_messages_from_tool_changes`, `::declared_compat_overrides_the_runtime_default_in_both_directions`, `api::openai_completions::tests::transform::a_system_message_between_a_tool_call_and_its_results_is_held_back`. **CORRECTED 2026-10-03:** the closure covers the predicates, the `SystemMessage` shape and the cut-point allow-list, not the adapter half: no adapter emits `tool_addition`/`tool_removal` (`rg 'tool_addition|tool_removal' crates --type rust` finds no emitter; `api/anthropic_messages/messages.rs:58-60` skips `Message::System`), and code comments cite that remaining work as "PROV-083b", which had no ledger row. Filed 2026-10-03 as `PROV-133` (with pi v1.0.1's inline-tools change). |
| ~~PROV-084~~ | ~~medium~~ **CLOSED 2026-09-27** | parity-bug | S | **The shared SSE framer drops a final event that is not followed by a blank line**, so an Anthropic `message_stop` (or any last frame) cut at EOF becomes `Anthropic stream ended before message_stop` and the turn fails — pi's Anthropic decoder flushed the trailing event since before v0.83.0 (`flushSseEvent`), and v0.85.0 (#9047) made Codex do the same. **FILED 2026-09-24 (second pass)**; body below. — **CLOSED 2026-09-27**: `flush_at_eof: true` on both remaining live gates (`api/mistral_conversations/mod.rs:146`, `api/pi_messages.rs:203`), each pinned by a loopback `run()` test that is red without it. Verify: `api::mistral_conversations::tests::decode::prov084_the_live_run_path_flushes_a_reply_cut_after_the_finish_reason_chunk`, `api::pi_messages::tests::prov084_the_live_run_path_flushes_a_reply_cut_after_done`. |
| ~~PROV-085~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | **Codex with thinking `off` omits `reasoning`, so the server default effort applies and the model still reasons** — pi v0.86.0 (#9191) sends the model's Off effort (`"none"`, or the mapped value) unless the map says Off is unsupported. **FILED 2026-09-24 (second pass)**; body below. — **CLOSED 2026-09-27**: an `else` branch emits `{"effort": map.off ?? "none"}` at Off, and a mapped `off: null` suppresses `reasoning` entirely (`api/openai_codex_responses/request.rs:161-180`). Verify: `api::openai_codex_responses::tests::request::off_honours_a_mapped_off_and_a_null_off_suppresses`. |
| ~~PROV-086~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `detect_compat` now sets `send_session_affinity_headers: is_openrouter` (`api/compat.rs:681`, `:799`), with pi's own test: provider `openrouter` or a base URL containing `openrouter.ai` (`openai-completions.ts:1597`, `:1672` @v0.87.1). An OpenRouter completions request with a session id and caching on now sends `x-session-id` (`api/openai_completions/headers.rs:76-92`); `sendSessionAffinityHeaders:false` in `models.json` still suppresses it. The Anthropic half of the same upstream commit is also ported: `get_anthropic_compat` defaults `sendSessionAffinityHeaders` and `sessionAffinityFormat: openrouter` for OpenRouter (`api/anthropic_messages/compat.rs:32-46`), and on the API-key branch the header is `x-session-id` for the `openrouter` format and `x-session-affinity` otherwise, sent only when cache retention is not `none` (`api/anthropic_messages/headers.rs:150-165`; pi `anthropic-messages.ts:206-220`, `:563-564`, `:964-969`, `bbb61e34a`, #9102, v0.86.0). Verify: `api::openai_completions::tests::headers::openrouter_sends_session_affinity_by_default`, `api::anthropic_messages::tests::headers::openrouter_anthropic_sends_x_session_id_by_default` (both red without the fix). **Ledger correction:** the Fix below defers the Anthropic default until `PROV-099`. It does not depend on `PROV-099`, because it only affects an OpenRouter model routed over anthropic-messages (from `models.json` or the pi.dev overlay), so it was ported now. `PROV-099` is untouched. — *Original:* **OpenRouter Chat Completions requests never send `x-session-id`** — pi v0.86.0 (#9102) defaults `sendSessionAffinityHeaders` to true for OpenRouter endpoints, so cyrup loses OpenRouter's sticky routing and its prompt-cache hits. **FILED 2026-09-24 (second pass)**; body below. |
| ~~PROV-087~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | M | **Gemini thinking levels are chosen from a hard-coded model-id family table, not from the model's `thinkingLevelMap`** — pi v0.84.3 (`resolveGoogleThinkingLevel`) and v0.86.0 (#9455, `usesGoogleThinkingLevel` + `getDisabledGoogleThinkingConfig`) take the level and the disabled level from the map, so a Gemini 3.x / custom model whose supported levels differ gets a level the API rejects. **FILED 2026-09-24 (second pass)**; body below. — **CLOSED 2026-09-27**: `resolve_google_thinking_level` / `uses_google_thinking_level` / the Off fallback are ported and the per-family tables are gone (`api/google_generative_ai/thinking.rs:49,111,157-166`, `capabilities.rs:12`). Verify: `api::google_generative_ai::tests::thinking::{mapped_medium_survives_on_gemini_3_pro,mapped_xhigh_reaches_a_real_google_level,unmappable_thinking_level_fails_the_request,reasoning_off_uses_the_lowest_supported_level,token_budget_is_keyed_on_the_resolved_level}`. |
| ~~PROV-088~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | **Mistral reasoning is sent as `prompt_mode:"reasoning"` for every `mistral-medium-*` except 3.5 and for Mistral-hosted GLM-5.2** — pi v0.86.0 (#8700, #9375) sends `reasoning_effort` for them, since `prompt_mode` is ignored or unsupported there. **FILED 2026-09-24 (second pass)**; body below. — **CLOSED 2026-09-27**: `uses_reasoning_effort` gains the `mistral-medium-` prefix and `zai-glm-5-2` (`api/mistral_conversations/reasoning.rs:40-45`). Verify: `api::mistral_conversations::tests::reasoning::medium_family_and_glm_use_reasoning_effort_not_prompt_mode`. |
| ~~PROV-089~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): the generator reads this one catalog at its own pin, whatever `--rev` says. `xtask/src/main.rs:97` is `const IMAGES_REV: &str = "v0.87.1"`; `CatalogSpec.rev` (`:91`) is `Some(IMAGES_REV)` only on the `openrouter-images` entry (`:147`), and `CatalogSpec::rev` (`:199-201`) falls back to the run's `--rev` for every other entry. Generation (`:949`) and the manifest (`:1156`) both go through it, so `catalog_manifest.json`'s `catalogs."openrouter-images".source` is `pi@v0.87.1` and its note says why. `own_rev_summary` (`:736`) names the pinned revision in the run summaries. `providers/catalog/openrouter-images.json` now has 55 rows. It was compared independently with bun, importing the upstream TS module at v0.87.1: all 55 rows are JSON-identical to cyrup's file, in order, and every upstream key (`id`, `name`, `api`, `provider`, `baseUrl`, `input`, `output`, `cost.*`) is one `ImagesModel` reads. `gen-catalogs --check` reports all 35 files matching. Tests: `tests::catalog_data::the_images_catalog_is_image_models_at_v0_87_1` (`crates/cyrup-provider/src/tests/catalog_data.rs:205`, through `images::openrouter_image_models`, 55 rows and the 20 added ids; lane reports it red without the fix), `tests::catalog_data::the_catalog_manifest_names_one_revision_per_provider` (`:1128`), `images::tests::catalog_parses_verbatim_with_expected_count` (`images/mod.rs:694`, now 55) and xtask's `tests::module_paths_default_to_the_provider_models_module` (`xtask/src/main.rs:1472`, pins `img.rev(..) == "v0.87.1"` for any run revision and that `openrouter-images` is the only self-pinned catalog). **Gate status:** lane-verified only (fmt clean; clippy/nextest not run on the combined tree). **LEDGER CORRECTION 2026-09-28:** "Four MAI rows were also renamed" is wrong. Only ONE existing MAI row changed its display name (`Microsoft: MAI-Image-2.5` → `Microsoft AI: MAI-Image-2.5`); the other three "Microsoft AI:" names belong to added rows. The refresh also renames `x-ai/grok-imagine-image-quality` to `SpaceXAI: Grok Imagine Image Quality` and takes upstream's literal `0.0833333333333333` for `google/gemini-2.5-flash-image`'s `cacheWrite` (was `0.08333333333333334`). The 20 added ids match this row. **Deadline carried forward:** after the next pi tag (post-tag `a328aa89a` deletes `image-models.generated.ts`) this catalog becomes `PROV-071`'s class, and `IMAGES_REV` must stay `v0.87.1`. *Original filing follows.* — **`openrouter-images.json` is 20 rows behind a source that is still in git**: 35 rows (pi `b0c2a90e`) against 55 in `image-models.generated.ts` at v0.87.1 — the one embedded catalog `PROV-071`'s "no revision can yield newer rows" does not apply to, and it stops applying after the next pi tag (post-tag `a328aa89a` deletes the file). **FILED 2026-09-24 (second pass)**; body below. |
| ~~PROV-090~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | M | **Anthropic server-side refusal fallback is unported** — no `fallbacks` request field, no `server-side-fallback-2026-07-01` beta, no fallback-model repricing, no hard error on a mid-output `fallback` block. pi v0.84.3 (#8017, #8285) + v0.86.0; the generator gives `claude-fable-5` and `claude-opus-5` fallback targets. **FILED 2026-09-24 (second pass)**; body below. — **CLOSED 2026-09-27**: `allowed_fallback_models` on `ModelCompat` (`api/compat.rs:445`), the `fallbacks` param (`anthropic_messages/params.rs:255`), the beta (`headers.rs:20,65`), `message_start` repricing + `response_model` (`driver.rs:152-172`, `blocks.rs:58`) and the late-`fallback`-block terminal error. Verify: `api::anthropic_messages::tests::decode::prov090_server_side_fallback::*` (5) and `…::tests::params::prov090_allowed_fallback_models_emit_fallbacks_and_the_beta`. |
| ~~PROV-091~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | L | **Anthropic mid-conversation effort is unported** — no `supportsMidConvoEffort`, no `providerThinkingLevel` on `AssistantMessage`, no synthetic `system` effort messages, no `block_binding: drop_block`, no `mid-conversation-output-config`/`thinking-binding-controls` betas; also the interleaved-thinking beta is sent even when thinking is off. pi v0.85.0 says the managed path exists "so prefix mismatches can be dropped instead of surfacing as persistent 400 responses". **FILED 2026-09-24 (second pass)**; body below. — **CLOSED 2026-09-29**: Anthropic mid-conversation reasoning effort is ported. `cyrup_core::AssistantMessage` gains `providerThinkingLevel` in pi's own slot between `responseId` and `diagnostics` (`packages/ai/src/types.ts`), and `api/pi_messages.rs:644-648` copies it onto both the done and error frames under upstream's `!== undefined` guard; `insert_thinking_level_messages` reconstructs the exact historical marker prefix and appends the current marker at `api/anthropic_messages/params.rs:175`; the managed block is emitted for `supportsMidConvoEffort` rows as `thinking: {type: "adaptive", display: ..., block_binding: {prefix_mismatch_behavior: "drop_block"}}` with `output_config.effort` HARDCODED to `"high"` and hoisted out of the `model.reasoning` gate exactly as upstream hoists it (params.rs:256-273), with the temperature gate gaining the matching `supportsMidConvoEffort !== true` conjunct (:210-215); and the interleaved-thinking beta is now the full four-term gate `model.reasoning && thinkingEnabled && (interleavedThinking ?? true) && !forceAdaptiveThinking` (`headers.rs:97-98`), where cyrup carried only the last two terms and so shipped the beta on reasoning-off requests. Verify: `api::anthropic_messages::tests::headers::interleaved_beta_requires_thinking_enabled`, `::interleaved_beta_requires_a_reasoning_model`, `api::anthropic_messages::tests::mid_convo::reconstructs_an_exact_historical_marker_prefix_and_appends_the_current_marker`, `::defaults_omitted_effort_to_high_and_still_enables_drop_block`, `::preserves_native_effort_at_every_rung`. |
| ~~PROV-092~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | M | **openai-completions `reasoning_details` replay is the pre-v0.84.3 shape**: only `reasoning.encrypted` entries keyed to a tool-call id are kept; `reasoning.text`/`reasoning.summary` are dropped, deltas are not merged, and nothing is anchored on the thinking block — so OpenRouter reasoning replay loses signed reasoning text. **FILED 2026-09-24 (second pass)**; body below. — **CLOSED 2026-09-27**: the three-variant union, the consecutive-delta merge and the thinking-block anchoring are ported, with the legacy tool-call-signature parse kept as a fallback (`api/openai_completions/{blocks,convert}.rs`, new `reasoning_details.rs`). Verify: `api::openai_completions::tests::reasoning_details::*` (4). |
| ~~PROV-093~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): A new `supportsMaxOutputTokens` compat key (default `true`; `api/compat.rs:498`, resolved at `:563`) gates `max_output_tokens` (`api/openai_responses/params.rs:165-177`). The builder now uses ports of pi's `getPromptCacheRetention` (`params.rs:34-42`: `"24h"` only for a long-cache-capable model that is not explicit-mode) and `getPromptCacheOptions` (`:48-62`: `{mode:"explicit"}` on retention `none`, `{ttl:"30m"}` on long retention when long cache retention is supported). The key order matches pi's params literal (`openai-responses.ts:68-100`, `:309-323` @v0.87.1; #8941 v0.85.0, v0.85.1). Verify: `api::openai_responses::tests::params::{long_retention_picks_the_supported_cache_field, supports_max_output_tokens_false_omits_the_cap, explicit_prompt_cache_mode_only_on_opt_in_and_none_retention}` (red without the fix); the verifier extended the `supportsMaxOutputTokens` test to parse the key in its `models.json` spelling. **Ledger correction:** the row says neither compat key exists. `supportsExplicitPromptCacheMode` was already ported (`PROV-023`); only `supportsMaxOutputTokens` was missing. Setting the GPT-5.6+/GPT-6 catalog flags stays with `PROV-071`. — *Original:* **openai-responses has no `supportsMaxOutputTokens` gate and sends `prompt_cache_retention:"24h"` even to GPT-5.6+ explicit-cache models**, where pi v0.85.0/v0.85.1 suppress `max_output_tokens` on request and send `prompt_cache_options.ttl:"30m"` instead of `24h`. **FILED 2026-09-24 (second pass)**; body below. |
| ~~PROV-094~~ | ~~low~~ **CLOSED 2026-09-28** | parity-bug | S | **CLOSED 2026-09-28** (on `claude/lows-next`): Both Responses builders now send `StreamOptions.tool_choice` after `tools` and before `reasoning`, as pi does, whether or not any tools are sent (`api/openai_responses/params.rs:216-220`; `api/azure_openai_responses.rs:402-409`). `auto`, `none` and `required` are strings; a forced function uses the Responses API shape `{type:"function", name}`, not the Chat Completions nested shape, through the shared `responses_tool_choice` (`params.rs:69-74`). Upstream `openai-responses.ts:107`, `:340-341` and `azure-openai-responses.ts:60`, `:323-324` @v0.87.1 (v0.84.3). Verify: `api::openai_responses::tests::params::tool_choice_reaches_the_responses_body`, `api::azure_openai_responses::tests::build_params_emits_tool_choice` (red without the fix). — *Original:* **openai-responses and azure-openai-responses never emit `tool_choice`** — `StreamOptions.tool_choice` is read by completions, Anthropic, Google, Bedrock, Mistral and Codex but not by either Responses adapter; pi has sent it on openai-responses since before v0.83.0 and on Azure since v0.84.3. **FILED 2026-09-24 (second pass)**; body below. |
| ~~PROV-095~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): New module `utils/user_agent.rs` ports `getPiUserAgent` with Node's platform and arch tokens and `os.release()`, read from `/proc/sys/kernel/osrelease` on Linux and `uname -r` on other Unixes. Completions, Responses, Azure, Anthropic, Google, Vertex and Mistral now send `cyrup (<platform> <release>; <arch>)` beneath every header overlay (`insert_default_user_agent`, called at `openai_completions/headers.rs:101`, `openai_responses/headers.rs:88`, `azure_openai_responses.rs:461`, `anthropic_messages/headers.rs:195`, `google_generative_ai/endpoint.rs:52`, `google_vertex.rs:442`, `mistral_conversations/endpoint.rs:76`). A caller or model `User-Agent` in any casing overrides it, a caller `None` suppresses it, and Copilot's catalog `GitHubCopilotChat` UA and Anthropic OAuth's `claude-cli` identity still win. The seven call sites were checked against every `getPiUserAgent` use in `packages/ai/src` at v0.87.1 (#8305, v0.84.3). Codex keeps pi's own product token but now uses the same builder (`openai_codex_responses/headers.rs:120-123`). Verify: `default_user_agent_sits_under_the_overlays` in each of the seven adapters' tests, `utils::user_agent::tests`, and `api::openai_codex_responses::tests::headers::sse_headers_match_upstream_and_cannot_be_overridden`, which the verifier tightened from a `pi (` prefix check to the exact value (all red without the fix). **Ledger correction:** at v0.87.1 `openai-codex-responses.ts:1626` also uses `getPiUserAgent()`; cyrup's Codex UA previously dropped the release and used Rust's arch spelling and now matches. `remote_catalog.rs::cyrup_user_agent` ports coding-agent's `getPiUserAgent(VERSION)`, a different function, and was left alone. One platform divergence remains: on Windows the release is omitted, where Node reports e.g. `10.0.x`. — *Original:* **No default `User-Agent` on seven adapters** — pi v0.84.3 (#8305) sends `pi (<platform> <release>; <arch>)` on completions, Responses, Azure, Anthropic, Google, Vertex and Mistral unless the caller overrides it; cyrup sends none (reqwest has no default). **FILED 2026-09-24 (second pass)**; body below. |
| ~~PROV-096~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | **`NO_PROXY=internal.corp` does not exempt `api.internal.corp`**: cyrup carries pi's pre-v0.85.0 exact-host rule, with no lower-casing and no IPv6 bracket handling, so a corporate internal gateway is sent through the proxy. pi v0.85.0 (#8737). **FILED 2026-09-24 (second pass)**; body below. — **CLOSED 2026-09-27**: `parse_no_proxy_entry` lower-cases, strips brackets and parses IPv6, and the matcher exempts the exact host and `.{domain}` suffixes (`utils/node_http_proxy.rs:83-134,142-176`). Verify: `utils::node_http_proxy::tests::{no_proxy_matches_subdomains_wildcards_ipv6_and_ports,no_proxy_normalises_target_case}`. |
| ~~PROV-097~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | M | **Bedrock: redacted reasoning is dropped, empty-key tool arguments are replayed verbatim, and `onResponse` sees only `x-amzn-requestid`** — pi v0.84.2/v0.84.3 (#8314, #7882, #8234). **FILED 2026-09-24 (second pass)**; body below. — **CLOSED 2026-09-27**: all three ports landed: `redactedContent` buffered behind the `[Reasoning redacted]` placeholder and replayed as `redactedContent` (`events.rs:18,286`, `blocks.rs:29-32`, `convert.rs:131,234-243`), `sanitize_bedrock_document` over `toolUse.input` (`convert.rs:112-121,221`), and every response header reaching `on_response` (`driver.rs:230`). Verify: `api::bedrock_converse_stream::tests::{decode::prov097_redacted_reasoning::*,convert::prov097_replay::*,driver::prov097_on_response_receives_every_header}` (7). |
| ~~PROV-098~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | M | **GitHub Copilot login accepts the policy of every catalog model at once (`join_all`), with no 429 handling** — pi v0.84.2/v0.84.3 (#6187, #7850) enables only models whose policy is `unconfigured`, one at a time, stopping on rate limit. **FILED 2026-09-24 (second pass)**; body below. — **CLOSED 2026-09-29**: the retry budget is bounded by the deadline rather than documented away. `auth/oauth/github_copilot.rs:709-745` arms the budget only when both knobs are positive (pi's `packages/ai/src/auth/oauth/github-copilot.ts:141-146`), then per iteration clamps the request's own timeout to `min(policy.attempt_timeout, deadline - now_millis())` and takes pi's abort path with `COPILOT_ATTEMPT_ABORTED` when the remainder is already spent, racing the send against the caller's `CancelToken` -- upstream's `AbortSignal.any([signal, AbortSignal.timeout(maxElapsedMs)])` composed with a fresh per-request `AbortSignal.timeout(5000)`. So the in-flight request, not merely the sleep between attempts, is bounded; `fetch_model_catalog` lost its local `.timeout` and goes through the same central cap, giving the policy POST a deadline it never had. Error mapping follows upstream: a cap or budget abort becomes `OAuthError::Failed` so `enable_model` returns `Ok(false)` and the batch continues and the login still succeeds (pi's `catch { if (signal.aborted) throw; return false }`), a caller cancellation becomes `OAuthError::Cancelled` and propagates, and a surviving 429 is still the one failure that breaks the batch. The `[CYRUP-DELTA]` that had recorded the unbounded budget as acceptable is deleted, not reworded. Verify: `auth::oauth::github_copilot::tests::prov098::an_attempt_that_outlives_the_retry_budget_fails_like_pis_abort`, `::the_policy_post_is_capped_per_attempt`. |
| ~~PROV-099~~ | ~~low~~ **CLOSED 2026-10-02** | upstream-drift | M | **CLOSED 2026-10-02** (`claude/lows-batch5`) as STALE: the routing this row asks for is already in the catalog, and nothing guarded it. `openrouter.json` carries the 15 non-`:batch` `anthropic/*` rows on `anthropic-messages` at `https://openrouter.ai/api`; every other row, the `:batch` ones and the `~anthropic/*-latest` aliases included, stays `openai-completions` at `.../api/v1` (pi `scripts/openrouter-catalog.ts`, `useAnthropicMessages`). Two tests now pin it (`tests/openrouter_anthropic_route.rs`): `openrouter_anthropic_rows_use_messages_and_batch_rows_stay_on_completions` (through `Models::get_models(Some("openrouter"))`, both sides asserted non-empty) and `an_openrouter_anthropic_row_streams_to_api_v1_messages_with_a_messages_body` (the real `openrouter/anthropic/claude-sonnet-5` row, origin swapped for a loopback fake: request line exactly `POST /api/v1/messages HTTP/1.1`; body has `model`, `stream: true`, a numeric `max_tokens`, a top-level `system` and one user message, and none of `stream_options`, `max_completion_tokens`, `store`). Red by editing the catalog: flipping `anthropic/claude-opus-4.5` to `openai-completions` fails the catalog test only; flipping `anthropic/claude-sonnet-5` fails both (the wire test saw `POST /api/chat/completions`). The catalog file is unchanged against main. The row's premise was stale from the start of this batch; the body's cyrup paragraph below is kept as the record of what was filed. Original finding: ~~**OpenRouter `anthropic/*` models are routed over openai-completions; pi v0.85.0 registers OpenRouter with both apis and the generator sends every `anthropic/*` row over anthropic-messages** (base `https://openrouter.ai/api`). **FILED 2026-09-24 (second pass)**; body below.~~ |
| ~~PROV-100~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `ModelCompat` now carries `thinkingTokenBudgetField` (a `ThinkingTokenBudgetField` enum, `api/compat.rs:25-45`, `:403`), `supportsThinkingTokenBudget` (`:407`) and `vllmPriority` (`:434`). The completions builder sends `priority` after `tool_choice` when `vllmPriority` is set (`api/openai_completions/params.rs:190-194`). It computes pi's `resolveClampedThinkingBudget` (`params.rs:265-277`, with `utils/simple_options.rs:178-186`): the level's budget with custom `thinkingBudgets` over the defaults, `xhigh`/`max` clamped to `high`, capped so 1024 answer tokens remain under the output ceiling. That budget goes in the resolved top-level field after the reasoning chain (`params.rs:199-209`, field resolved at `:253-257`), and `{"$var":"thinking.budget"}` now resolves to it in both `chat_template_kwargs` and Baseten `chat_template_args` (`api/openai_completions/reasoning.rs:225-230`) instead of to an effort string. Upstream `types.ts:87-98`, `:718-727`, `:745-750`, `openai-completions.ts:862-871`, `:971-978`, `:1004-1066`, `simple-options.ts:55-77` @v0.87.1 (#8275 v0.84.3, #9004 v0.85.0). Verify: the 11 tests of `api::openai_completions::tests::params::prov100_thinking_token_budget` (the 9 cases of upstream `openai-completions-thinking-token-budget.test.ts`, plus `both_keys_parse_from_their_models_json_form`, added by the verifier, and the Baseten case), `vllm_priority_is_the_top_level_priority_field`, and `utils::simple_options::tests::adjust_shrinks_budget_to_keep_output` (red without the fix). **Ledger correction:** upstream has a fourth key, `supportsThinkingTokenBudget` (an alias for `thinking_token_budget`, v0.84.3), which the row did not list; it is ported. The budget helpers live in `packages/ai/src/api/simple-options.ts`. — *Original:* **openai-completions compat keys `thinkingTokenBudgetField`, `{"$var":"thinking.budget"}` and `vllmPriority` are unported** — the `$var` case is worse than absent: cyrup resolves `thinking.budget` as an effort string. pi v0.84.3 (#8275) and v0.85.0 (#9004); `models.json`-only surfaces. **FILED 2026-09-24 (second pass)**; body below. |
| PROV-101 | low | upstream-drift | M | — | **The openai-responses, azure-openai-responses and openai-codex-responses adapters have no grammar (`custom`) tool support: no `type:"custom"` tool, no `custom_tool_call`/`custom_tool_call_output` replay, no `response.custom_tool_call_input.*` decoding** — pi `openai-responses-shared.ts` @v0.87.1 (`:291-323`, `:335-338`, `:362-368`, `:504-523`, `:670-690`, `:726-738`). cyrup ported `constrained_sampling.rs` (PROV-011) and the `supports_openai_grammar_tools` compat flag, but `convert_responses_tools` (`api/openai_responses/tools.rs`) only ever emits `type:"function"`, so a tool with `constrainedSampling` grammar runs as a plain function on the Responses wire. Found while closing `DRIFT-058`, whose `custom_tool_call` namespace arms have nothing to attach to. **FILED 2026-09-30**; body below. |
| PROV-102 | low | not-ported | L | **NEW 2026-10-01** (filed by the `EXT-027` closure, `claude/llama-cpp-port`). `ImageModel` / `ModelType::Image` unification is not ported. pi 0.99 folds images into the one model surface: `ModelTypeMap { chat, image, classifier }`, `ModelType = keyof ModelTypeMap`, `AnyModel = ModelTypeMap[ModelType]` (`packages/ai/src/types.ts:1145-1168` @v0.99.2-17). cyrup keeps image models in their own stack, `crates/cyrup-provider/src/images`, and `ModelType` is `{Chat, Classifier}` only (`crates/cyrup-provider/src/classifier.rs:43`, `AnyModel` `:204`). Impact: an extension cannot list an image model beside chat and classifier models in one catalog, and `Models` has no single `getAllModels` over all three. **Fix** — add `ModelType::Image` and `AnyModel::Image` over the existing image model type, route `Models::generate_image` through the same typed lookup, and keep `crates/cyrup-provider/src/images` as the api layer. **Verify** — a provider listing one model of each type; each `Models` operation accepts only its own type. |
| PROV-103 | low | not-ported | M | **NEW 2026-10-01** (filed by the `EXT-027` closure, `claude/llama-cpp-port`). The array-based `models.all.json` / `providers/{id}.all.json` catalog variants and the catalog `classifier` model group are not ported: `ClassifierModelCatalog` and `flattenClassifierModelCatalog` (`packages/ai/src/model-catalog.ts:45`, `:80` @v0.99.2-17), and the generator output `models.all.json` (`packages/ai/scripts/generate-models.ts:3652`, `packages/ai/CHANGELOG.md:43`). cyrup's compiled and published catalogs list chat models only (the on-disk store file DOES carry classifier members, see `PROV-107`). Impact: a provider cannot ship classifier models in its static catalog; they come only from a live refresh. **Fix** — add the `classifier` group to the catalog types and the generator, and read the `.all.json` variants in `crates/cyrup-provider/src/remote_catalog.rs`. **Verify** — a catalog fixture with one classifier model reaches `Provider::get_all_models`. |
| ~~PROV-104~~ | ~~low~~ **CLOSED 2026-10-10 (NARROWED)** | not-ported | M | **CLOSED 2026-10-10, NARROWED** (on `claude/compassionate-cray-46f1tv`; checked against pi `f1b2e77f5` and llama.cpp `b11436`): both System One apis are ported and registered; the new `typesafe` PROVIDER entry (pi `providers/typesafe.ts`, one classifier-only catalog row) is narrowed out to a proposed row, because cyrup has no classifier-only provider shape and no catalog path for classifier rows. **Ported** (pi `api/typesafe-system-one.ts`, `cloudflare-workers-ai-system-one.ts`, `system-one-shared.ts`, `classifier-shared.ts`, all re-read at the pin): `crates/cyrup-provider/src/api/typesafe_system_one.rs` (`POST <base>/systemone`, `{model, ...request}`), `cloudflare_workers_ai_system_one.rs` (`POST <base>/run`, `{model, input}`, both Cloudflare envelopes, `success:false` messages joined by `; `, a non-`Completed` run refused), `system_one_shared.rs` (`wireRequest`: a public `bool` question goes out as `type:"noul"`; `parseAnswers`: a `noul` answer comes back as `{type:"bool", probability}`, each answer checked against its question's type; usage set BEFORE the answers are parsed, so a malformed reply keeps its billed usage) and `classifier_shared.rs` (`postClassifierRequest`: no api key fails first, bearer + content-type under the model's and the request's headers, hooks, per-attempt timeout, provider retries; `parseClassifierUsage` priced by the catalog). The transport half of `classifier_shared.rs` is the llama.cpp api's former private copy, moved and parametrized by the label pi hard-codes, so `llama-cpp-classify`'s error text is unchanged (all its tests pass unmodified). `KnownClassifierApi` gains `TypesafeSystemOne` and `CloudflareWorkersAiSystemOne` in pi's order (`types.ts:78-82`) and `implementation()`. **Registered:** `WireProvider` gains pi's `classifiers` leg (`with_classifier_models`, `with_classifiers`; `supports_classification` answers for the map, as `supports_image_generation` does), `openrouter` installs `{typesafe-system-one}` (`providers/openrouter.ts:34`) and `cloudflare-workers-ai` installs `{cloudflare-workers-ai-system-one}` (`providers/cloudflare-workers-ai.ts:21-23`); both providers' classifier rows arrive through the live catalog overlay, which forwards `classify`. pi's `cloudflareClassifier` wrapper is not needed: Workers AI auth already substitutes `{CLOUDFLARE_ACCOUNT_ID}` into the resolved base URL that `Models::classify` applies. **The real wire:** a REAL b11436 `llama-server` (router mode over `tinylaya-for-testing-Q8_0.gguf`, `output_modalities:["decisions"]`) answered pi's own three test questions on `POST /v1/systemone`; that answer is `cyrup_llama_cpp_wire::golden::SYSTEMONE_TINYLAYA_LIVE`, and the new `systemone` definitions (`choice_answer`, `score_answer` with llama.cpp's extra `legend`/`probabilities`, `noul_answer`, `response`, the 501 `not_a_decision_model` it gave for `stories260K`) reproduce it byte for byte (`systemone_definitions_reproduce_the_live_answer`). The provider's loopback fake answers `/v1/systemone` from those definitions, with llama.cpp's own request checks (a `type:"bool"` that reached the wire is refused with the live server's 400), and its drift guard `the_fake_writes_the_live_system_one_bytes` pins the live bytes on a real socket. **Verify, per api — MET:** `tests::system_one::typesafe_answers_one_choice_one_bool_and_one_score_question_over_loopback` and `cloudflare_answers_one_choice_one_bool_and_one_score_question_over_loopback` (one choice, one score, one bool each; request body, route, bearer and the bool-as-noul all asserted), plus 17 more: pi's test cases (catalog pricing 308 x 0.042/1e6, OpenRouter's extra members, wrong api refused before sending, header merge and suppression, retry, malformed answers keep usage, malformed usage ignored), no-key refusal, wrong answer types, llama.cpp's 501 rendering, Cloudflare's direct form and envelope failures, and both provider entries (`openrouter` through its registry; `cloudflare-workers-ai` end to end through `Models::classify` with the account id resolved). Measured base `8052de5` -> branch: `KnownClassifierApi::ALL` 1 -> 3; providers with a classifier map 0 -> 2; `cyrup-provider` +19 tests (18 in `tests/system_one.rs`, 1 drift guard), 1697 run, all passing; the wire crate 3 -> 4 tests. `[CYRUP-DELTA]`s, each documented in code: pi's image-input refusal has nothing to refuse (`ClassifierContext` has no `images`); a fractional token count truncates into `Usage`'s `u64`; `noRetryStatuses` is not carried (only `openai-decisions` uses it). **Red-proved**, each fix reverted alone, then restored and green: no `bool`->`noul` rename (the main test fails on the fake's live-shaped `400 questions.approved: "type" must be one of: choice, score, noul`, 11 of the 19 fail); usage parsed after the answers (malformed-answers test loses its usage); no trailing-slash collapse (`left: "/v1////systemone"`); either provider's `with_classifiers` removed (its entry test fails on `supports_classification`); no run-record unwrap (`Cloudflare Workers AI returned an unexpected response`); no api-key check; request/model header order swapped (`left: Some("Bearer model")`); retries off; no api check (3 tests); usage presence check off (`left: Some((0, 0, 0))`); `noul` read from `probability` (9 fail); choice type unchecked; Cloudflare messages joined by `, `; wrong label; `KnownClassifierApi::ALL` back to llama only; `legend` dropped from `score_answer` (the wire crate test and the fake's live-bytes guard both fail). **Remains (proposed row):** the `typesafe` provider (`TYPESAFE_API_KEY`, catalog `jev-latest`). — *Original:* **NEW 2026-10-01** (filed by the `EXT-027` closure, `claude/llama-cpp-port`). The System One classifier apis `typesafe-system-one` and `cloudflare-workers-ai-system-one` (`KnownClassifierApi`, `packages/ai/src/types.ts:35` @v0.99.2-17) are not ported: `KnownClassifierApi` has `LlamaCppClassify` only (`crates/cyrup-provider/src/classifier.rs`). Impact: a provider that registers a System One classifier model gets `classify` refused as an unknown api. **Fix** — port the two apis behind `ProviderClassifier` and register them in `ClassifierApiRegistry`, with their provider entries. **Verify** — a loopback fake per api answering one choice, one bool and one score question. |
| PROV-105 | low | not-ported | M | **NEW 2026-10-01** (filed by the `EXT-027` closure, `claude/llama-cpp-port`). `Provider.filterAllModels`, `Models.getAllAvailable` and `Models.getAvailableOfType` (`packages/ai/src/models.ts:196`, `:708-730` @v0.99.2-17), and `Models.refresh`'s handling of classifier models, are not ported. cyrup's classifier lane covers reads (`Provider::get_all_models`, `crates/cyrup-provider/src/provider.rs`), `classify` (`collection.rs:487`) and persistence only; availability filtering and refresh still see chat models. Impact: a classifier model of a provider whose chat models are filtered by credential is listed regardless, and a refresh through `Models` does not restore or publish classifier models (the extension engine in `cyrup-session-svc` does, for live providers). **Fix** — port the three methods and let `refresh_with` carry classifier models through the same restore/publish phases. **Verify** — filtered availability per type; a refresh round-trips a classifier model. **PARTIAL 2026-10-06 (`claude/codemode`):** the three read methods are ported — `Provider::filter_all_models` (`provider.rs:238`), `Models::get_all_available` (`collection.rs:1005`) and `Models::get_available_of_type` (`:1039`), because codemode's `models.*` needs them (`CODE-008`). **Still open:** `Models::refresh_with` (`collection.rs:794`) does not carry classifier models through the restore/publish phases; effort now S. |
| PROV-106 | low | not-ported | M | **NEW 2026-10-01** (filed by the `EXT-027` closure, `claude/llama-cpp-port`). `ProviderRequestOptions.fetch` and `.telemetryContext` (`packages/ai/src/types.ts:135`, `:142`) are not carried by `ClassifierOptions`, and `BaseModel.inputLimits` (`:1105`) is absent from `ClassifierModel`, because cyrup's `StreamOptions` / `ImagesOptions` do not carry them either and cyrup's `Model` has no `input_limits` (`crates/cyrup-provider/src/classifier.rs`, `stream.rs:204`). The classifier api is therefore tested through a loopback server with the base URL as the seam (`crates/cyrup-provider/src/tests/llama_cpp_classify_fake_server.rs`). Impact: an extension cannot substitute the transport or attach a telemetry context for a classify call. **Fix** — add `fetch` and `telemetry_context` to the shared request options (owned by `cyrup-provider`) and `input_limits` to `Model`, then to `ClassifierOptions` / `ClassifierModel`. **Verify** — a classify call through an injected transport. |
| PROV-107 | low | port-divergence | M | **NEW 2026-10-01** (filed by the `EXT-027` closure, `claude/llama-cpp-port`). Classifier models are persisted through the sibling methods `ModelsStore::read_classifier_models` / `write_classifier_models` / `write_with_classifiers` (`crates/cyrup-provider/src/models_store.rs:126-177`) instead of pi's one merged `ModelsStoreEntry.models: AnyModel[]` (`packages/ai/src/models-store.ts:3-5` @v0.99.2-17), because a fifth field on `ModelsStoreEntry` broke every struct-literal site outside the lane that added it. The brief asked for a serde-default field; that was not done, for that reason. **Corrected 2026-10-01:** the on-disk half is NOT open. `FileModelsStore` (`crates/cyrup-config/src/models_store.rs:461-545`) writes and reads the one merged `models` array, chat first, classifiers as `type:"classifier"` members, exactly as pi's file does (tests `the_file_holds_one_models_array_chat_first_then_classifiers`, `chat_and_classifier_models_survive_a_restart`, `crates/cyrup-config/src/tests/models_store_classifiers.rs`; `cyrup-it` `a_refresh_persists_chat_and_classifier_models_to_the_models_store_file`). What remains is the trait shape: a store that does not override the pair refuses a non-empty classifier write with `ModelSource`, and unknown member types are skipped by `is_classifier_member` (`models_store.rs:136`) rather than dropped by pi's `withKnownModelTypes` (`models.ts:120-127`). **Fix** — add `classifier_models` to `ModelsStoreEntry` with a custom serde that writes and reads pi's merged array (chat first, classifier tagged `type:"classifier"`, unknown types dropped), repair the literals with `..Default::default()`, and delete the sibling methods. **Verify** — the existing round-trip tests pass unchanged against the single field. **Open question:** is the second persisted shape worth a migration, given the file already holds pi's shape? |
| ~~PROV-108~~ | ~~low~~ **CLOSED 2026-10-10** | port-divergence | S | **NEW 2026-10-01** (filed by the `EXT-027` closure, `claude/llama-cpp-port`). `Models::classify` follows pi 0.99 `applyAuth` and returns `Provider is not configured: <id>` when a provider's auth resolves to nothing (`crates/cyrup-provider/src/collection.rs:550-554`; pi `packages/ai/src/models.ts:824-829` @v0.99.2-17), while cyrup's existing chat `Models::stream` path passes an unconfigured provider through (`AuthHelper::apply_auth`, `collection.rs:918-960`). Two entry points on one `Models` disagree about the same state. Chat behaviour was left unchanged because the offline `faux` provider relies on the pass-through. **Fix** — decide: either make `stream` refuse like `classify` and give `faux` a no-op auth strategy, or let `classify` pass through and record it. **Open question** (from the classifier lane): should `classify` pass through? **Verify** — one test per entry point for an unconfigured provider. — **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5). **Decision: `classify` was right; chat now refuses too.** pi has one `applyAuth` (`packages/ai/src/models.ts:843-875`) shared by `stream` (`:877-891`), `streamSimple` (`:901-908`), `streamDeferred`, `generateImages` and `classify` (`:972-989`), and it throws for every one of them: ``if (!resolution) { throw new ModelsError("auth", `Provider is not configured: ${model.provider}`); }`` (`:859-861`). pi's faux never hits it: its auth is `{ apiKey: { name: "Faux", resolve: async () => ({ auth: {} }) } }` (`providers/faux.ts:708`), always configured. **Fix:** `collection.rs` `AuthHelper::apply_auth` (behind `Models::stream`/`stream_simple`) returns new `ProviderError::NotConfigured` (`Display` `Provider is not configured: <id>`, code `auth`) when the strategy resolves to nothing, so the chat terminal carries the same text `classify` and `generate_images` report. The row's premise about `faux` was wrong: cyrup's `FauxProvider` has no strategy (`provider_auth()` is `None`), and that case still delegates on both paths, as `apply_classifier_auth` already did; pi has no strategy-less provider (`auth` is required), so this is a cyrup-only case, unchanged and recorded here. Verify: `tests/classifier_dispatch.rs` `prov108_models_stream_requires_a_configured_provider` and `prov108_models_stream_simple_requires_a_configured_provider` (error terminal `Provider is not configured: p`) beside the existing `models_classify_requires_a_configured_provider`, all three on one provider with an `UnconfiguredAuth` strategy. Red before: with the refusal reverted both chat tests fail `left: Some("stream ended without a terminal event")`; file restored byte-identical (sha256 checked). Full `cyrup-provider` suite green (1690); `cyrup-config models_json_provider` and `cyrup-mcp sampling` green. |
| ~~PROV-109~~ | ~~low~~ **CLOSED 2026-10-02** | parity-bug | S | **CLOSED 2026-10-02** (`claude/lows-batch5`): `AuthHelper::apply_auth` (`collection.rs`) now passes `options.env` into the auth resolution (`AuthOverrides.env`, was `None`, as `apply_classifier_auth` does) and sets `request_options.env = merge_env(resolution.env, options.env)` after the headers: the resolution's env first, the request's env winning per key (pi `models.ts:862` @v0.99.2-17; the early-return arms are untouched). Test `collection::tests::apply_auth_merges_the_resolution_env_into_the_request_options` (new stub `FixedEnvAuth`): with no request env the resolution env reaches `out.env`; with one, the merge is resolution-first and `SHARED` ends as the request's value while resolution-only and request-only keys both survive. Written first and RED: the first assertion failed with `left: None, right: Some({"LLAMA_BASE_URL": "http://resolved", ...})`. Not verified end to end through a real llama.cpp provider. Original finding: ~~**NEW 2026-10-01** (filed by the `EXT-027` closure, `claude/llama-cpp-port`). `Models::stream`'s `AuthHelper::apply_auth` (`crates/cyrup-provider/src/collection.rs:918-960`) does not merge the auth resolution's `env` into `request_options.env`, whereas pi's `applyAuth` does (`packages/ai/src/models.ts:481-487`, `:862` @v0.99.2-17) and cyrup's own `Models::classify` path does (`collection.rs:587`, `merge_env` `:988`). A provider whose `WireProvider` re-resolves with an `api_key` override but no `env` (llama.cpp needs `LLAMA_BASE_URL` there) is then "not configured" on that path. The production agent streams through `Provider::stream` directly and is unaffected; found while testing the llama provider and not worked around. **Fix** — set `request_options.env = merge_env(resolution_env, options.env.as_ref())` in `apply_auth` as `classify`'s twin does. **Verify** — a stream through `Models::stream` for a provider that needs a resolved env value.~~ |
| PROV-110 | low | port-divergence | S | **NEW 2026-10-01** (filed by the `EXT-027` closure, `claude/llama-cpp-port`). The classifier api differs from its TypeScript in four JS-semantics corners, none reachable from a normal `classify`. (1) `OrderedMap` (`crates/cyrup-provider/src/classifier.rs:316`) keeps insertion order but does not reproduce JS's hoisting of integer-like keys (`"1"`, `"2"`) ahead of the others, which changes choice-label assignment only for numeric-string criteria keys given out of ascending order; the state map is `serde_json` with `preserve_order`, which does not hoist either, so the state JSON key order differs for integer-like keys. (2) `JSON.stringify` of state keeps an exact `u64` / `i64` above 2^53 where JS rounds. (3) `answer_from_probabilities` (`crates/cyrup-provider/src/api/llama_cpp_classify.rs:609`) yields NaN or an empty key where pi's non-null assertions would yield `undefined` on a keys / probabilities length mismatch; unreachable, because both come from one label set. (4) The label-token cache is a `OnceCell` per key (`:971`), so a concurrent waiter does not share another caller's in-flight error or cancellation as pi's shared `Promise` does, and `classify_question`'s `try_join!` (`:1013`, `:1152`) drops the sibling request on the first error where pi leaves an orphaned request running. The last is recorded in the source as `[CYRUP-DELTA]` mechanism owned by `EXT-027`; the first three are not forced by Rust and stay on the list. **Fix** — (1) a key-ordering helper that hoists canonical integer keys in ascending order, applied to both maps; (2) a decision whether to emulate; (3) return an error; (4) none needed beyond the delta note. **Verify** — a numeric-keys fixture asserting pi's order. |
| ~~PROV-111~~ | ~~low~~ **CLOSED 2026-10-03** | port-divergence | M | **CLOSED 2026-10-03** (`claude/lows-batch6`): `RefreshModelsContext` (`cyrup-provider/src/provider.rs`) now carries `credential`, `stored`, `stored_classifiers` and `publisher: Option<Arc<dyn ModelsPublisher>>`, with a `publish()` that answers `Ok(false)` when there is no publisher. `ModelsPersist`, `ModelsPublication` and `ModelsPublisher` moved into `cyrup-provider` and are re-exported from `cyrup-ext/src/host/services.rs`, so the old import paths still resolve. The tokio task-local is gone (`ProviderRefreshContext` and `CURRENT_REFRESH` deleted); `GuestProviderRegistry::run_phase` builds the context directly and `cyrup-llama`'s `ContextRefreshHost` reads credential, stored and publisher from the argument (`LlamaRefreshHost`'s methods take `&RefreshModelsContext` and it is no longer a `CatalogPublisher` supertrait). `Debug` is hand-written so a credential never prints. `[CYRUP-DELTA]` on the type: the three host-supplied members are optional because two engines call `refresh_models`, the session's extension-provider registry (fills all of them) and `Models::refresh_with`, whose persisting fetcher owns its own store and auth context (there they are `None`); pi has one engine. Tests: `cyrup-session-svc` `a_refresh_models_that_spawns_a_task_still_sees_its_credential_and_can_publish` (the spawned task sees the credential and the stored catalog and its publication applies); three in `cyrup-ext/src/tests/provider_refresh.rs` (`a_clone_of_the_refresh_context_carries_everything_into_a_spawned_task`, `publishing_without_a_publisher_applies_nothing`, `the_refresh_contexts_debug_output_does_not_leak_the_credential`); the `cyrup-llama` `host_wiring` tests were adapted. The redaction test was proven by a gutting mutation (run by the lead): with `Debug` printing the credential field, `the_refresh_contexts_debug_output_does_not_leak_the_credential` failed; the line was restored. Red first: a scratch test whose provider spawns a task and reads `ProviderRefreshContext::current()` failed with `the spawned task saw None` (deleted after the red run). Original finding: ~~**NEW 2026-10-01** (filed by the `EXT-027` closure, `claude/llama-cpp-port`). `cyrup_provider::RefreshModelsContext` carries only `allow_network`, `force` and `cancel` (`crates/cyrup-provider/src/provider.rs:29-43`), so pi's `credential`, `stored` and `publish` (`packages/ai/src/models.ts:74-90` @v0.99.2-17) reach a provider through a tokio task-local, `ProviderRefreshContext::current()` (`crates/cyrup-ext/src/host/services.rs:343`, `:379`), set only for the duration of the `refresh_models` call by `GuestProviderRegistry` (`crates/cyrup-session-svc/src/guest_providers.rs:422`). It does not cross a `tokio::spawn` made inside `refresh_models`, and it makes the contract invisible to the type system. It was filed rather than tagged `[CYRUP-DELTA]`: the lane did not own `provider.rs`, and nothing about Rust forces it. **Fix** — add `stored` / `credential` / `publisher` fields to `RefreshModelsContext` itself (a handful of struct-literal sites: `collection.rs:635`, `:666`, `guest_providers.rs:416`), delete the task-local, and let `cyrup-llama`'s `ContextRefreshHost` read them from the argument. Only `run_phase` and `ProviderRefreshContext::current` / `scope` change in the engine. **Open question** from the engine lane: is the task-local acceptable until then? **Verify** — `refresh_models` that spawns a task still sees its credential.~~ |
| PROV-112 | low | not-ported | S | **NEW 2026-10-03** (filed while closing `SEAM-142` on `claude/lows-batch6`). `ImagesProvider::refresh_models` has no `has_refresh_models` or context-carrying equivalent. It is a separate trait from `Provider::refresh_models`, and no engine begins or supersedes refreshes for image providers, so nothing is wrong today. **Fix** — when an images refresh engine exists, add the same static-provider filter and the same context members. **Verify** — a static images provider's refresh does not supersede an in-flight one. |
| PROV-113 | low | not-ported | M | **`StreamOptions.onProviderStreamEvent` and its nine adapter call sites are unported** — pi `packages/ai/src/types.ts:198` (v1.0.0, `002fc8385`) plus `api/simple-options.ts:41` and one `await options?.onProviderStreamEvent?.(…)` per wire api; `grep -rn 'on_provider_stream_event' crates/` at HEAD is empty. Discharges the lead at `01-…:638`. **FILED 2026-10-02**; body below. |
| ~~PROV-114~~ | ~~medium~~ **CLOSED 2026-10-04** | stale-port | S | **CLOSED 2026-10-04** (`57ff4f5b`): `lower_reasoning` (`api/mistral_conversations/reasoning.rs`) now picks `reasoning_effort` vs `prompt_mode` by presence in `thinkingLevelMap` rather than by model id, reasoning-off sends the map's own `off` value and nothing when the map has none (the asymmetric `??` chain), and `MistralReasoningEffort` is v1.0.0's five-valued union (`options.rs`). `SimpleStreamOptions.reasoning` excludes `"off"` upstream, so cyrup's `Off`-default plus `is_on()` gate is an exact mirror. Verify: `a_mapped_model_outside_the_old_id_set_uses_reasoning_effort`, `reasoning_off_sends_the_maps_off_value_and_nothing_without_one`, `the_reasoning_effort_override_spans_all_five_upstream_values`, `mapped_models_use_reasoning_effort_not_prompt_mode`. — *Original:* **Mistral reasoning lowering is pinned to a hardcoded model-id set that pi v1.0.0 deleted** — `thinkingLevelMap`-presence now picks `reasoning_effort` vs `prompt_mode` (`api/mistral-conversations.ts:202-213` @v1.0.0), and reasoning-off sends `effortMap.off`. cyrup `api/mistral_conversations/reasoning.rs:40-50` still matches ids, so cyrup's own `zai-glm-5-3` catalog row gets `prompt_mode`. **FILED 2026-10-02**; body below. |
| ~~PROV-115~~ | ~~high~~ **CLOSED 2026-10-04** | upstream-drift | S | **CLOSED 2026-10-04** (`eddfa7ac`): `process_content` (`api/mistral_conversations/content.rs`) drops an empty text delta at all three text sites — string content, bare-string array item, `{type:"text"}` item — before `push_text` can open a block, so a GLM turn decodes as one assistant thinking entry and round-trips through `to_chat_messages` unchanged; a non-empty mid-text delta still splits, so the guard did not widen into dropping real text. Verify: `prov115_an_empty_text_delta_does_not_split_thinking_into_two_blocks`, `prov115_a_glm_turn_replays_as_one_assistant_thinking_entry`. — *Original:* **An empty Mistral text delta opens a text block and splits thinking into two blocks, which Mistral rejects when the transcript is replayed** — pi v1.0.0 guards both sites with `if (!textDelta) continue;` (`api/mistral-conversations.ts:636`, `:678`, `8930b9ec0`). cyrup's `push_text` (`api/mistral_conversations/content.rs`) has no guard, and `zai-glm-5-2`/`zai-glm-5-3` ship in cyrup's Mistral catalog. **FILED 2026-10-02**; body below. |
| ~~PROV-116~~ | ~~medium~~ **CLOSED 2026-10-04** | upstream-drift | S | **CLOSED 2026-10-04** (`57ff4f5b`): `api/openai_responses/decoder.rs` raises upstream's `OpenAI Responses stream completed with an unfinished tool call: {name} ({id})` when a terminal `toolUse` stream leaves a tool block unfinished, positioned before the `end_of_stream` arm so a stream cut before `response.completed` still reports truncation; the id is upstream's own `{call_id}\|{item_id}` block id, and the per-block `finished` flag (in place of upstream's `delete` of the `partialJson` scratch buffer) also catches a server that omits `output_index`. Verify: `prov116_a_completed_stream_with_an_unfinished_tool_call_is_an_error`, `prov116_a_server_omitting_output_index_cannot_smuggle_an_orphaned_call_through`, with `prov116_a_truncated_stream_reports_truncation_not_the_unfinished_call` and `prov116_the_guard_only_applies_to_a_tool_use_terminal` as negative controls. — *Original:* **`processResponsesStream` has no unfinished-tool-call guard, so a Responses stream that completes with a tool call whose `output_item.done` never arrived hands the agent truncated or duplicated arguments** — pi v1.0.0 throws instead (`api/openai-responses-shared.ts:764-775`, `1b2aa0ca0`). cyrup `api/openai_responses/decoder.rs:203-227` checks only `saw_terminal`. **FILED 2026-10-02**; body below. |
| ~~PROV-117~~ | ~~medium~~ **CLOSED 2026-10-04** | upstream-drift | S | **CLOSED 2026-10-04** (`57ff4f5b`): `AnthropicOAuth::run_login` ends `CallbackServer::start(..).await.ok()` (upstream's `.catch(() => undefined)`), holds an `Option<CallbackServer>`, skips the `select!` when it is `None`, and reaches the paste prompt through the same second chance upstream reaches with `value !== undefined`; the advertised `redirect_uri` falls back to the module constant `http://localhost:53692/callback`, which is what makes the unbound case workable. `openrouter.rs` and `radius.rs` keep upstream's deliberate hard failure. Verify: `login_degrades_to_manual_paste_when_the_callback_port_cannot_bind` (the test squats the port with a real `TcpListener` and asserts the bind fails first), `redirect_uri_matches_the_bound_listener`. **Updated 2026-10-08 by `PROV-136`:** a taken port now falls back to an OS-chosen one before the paste-only path, so that test became `login_falls_back_to_a_free_port_when_the_preferred_port_cannot_bind`, and the paste-only path is pinned by `login_degrades_to_manual_paste_when_no_callback_port_can_bind` (both binds fail on a TEST-NET-1 host; `redirect_uri` is then `REDIRECT_URI` verbatim). **Ledger correction:** the Codex flow needed no change — `start_local_oauth_server` already mapped `Err(Listen)` to `Ok(None)` and `manual_paste_completes_when_the_listener_is_unbound` already covered it. — *Original:* **A loopback OAuth callback that cannot bind aborts `/login` instead of degrading to the manual-paste prompt** — pi v1.0.0 wraps the listener in `.catch(() => undefined)` and `waitForCallbackOrManualInput` runs the paste prompt alone (`auth/oauth/anthropic.ts:148`, `auth/oauth/openai-codex.ts:370`, `auth/oauth/callback-server.ts:155-184`). cyrup's `run_login` propagates the start error with `?` and documents "there is no manual-paste fallback for it" (`auth/oauth/anthropic.rs`). **FILED 2026-10-02**; body below. |
| ~~PROV-118~~ | ~~medium~~ **CLOSED 2026-10-05** | not-ported | L | **"Sign in with ChatGPT" on the `openai` provider is unported** — pi v1.0.0 gives `openai` an `oauth` entry (`providers/openai.ts:12-19`) backed by the new 310-line `auth/oauth/openai-chatgpt.ts`, adds `LoginOptions.getDeviceId` to `OAuthAuth.login` (`auth/types.ts:202-226`), three `subscription_sharing_*` retry literals, and a ChatGPT-usage hint on the limit error. cyrup's `providers/openai.rs` carries an api key only. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** scope is understated, and v1.0.1 changed the file. (1) `isChatGPTSignIn` (`api/openai-responses.ts:40` @v1.0.1) is not only the usage hint: a ChatGPT sign-in token sent to `https://api.openai.com/v1` also makes the request omit `prompt_cache_retention`, `prompt_cache_options`, `max_output_tokens` and `temperature` (`:329-345`, `omitUnsupportedFields`); the row's text mentions none of that. (2) pi v1.0.1 (`eeac84ca9`) changes `auth/oauth/openai-chatgpt.ts` (309 lines at v1.0.1, 310 at v1.0.0): `startCallbackServer` now fails fast on `EADDRINUSE` with "Port 1455 is in use, probably by an unfinished login in another pi session or by the Codex CLI. Cancel that login and try again." (`:241-248`) and the paste fallback is dropped for this flow. The port is shared with legacy Codex login, so the two logins collide. This is the opposite of `PROV-117`'s degrade-to-paste. (3) "land (a) with `PROV-117`" is unsupported: `PROV-117` has no options-parameter work. cyrup has no `openai-chatgpt` code (verified: no hit under `crates/`). Severity and effort are roughly right. **PARTIAL 2026-10-05** (`209c239d`) — ported, proven, and **not usable yet**. Landed: the full ChatGPT login (`auth/oauth/openai_chatgpt.rs`, built on the existing `callback`/`pkce`/`interaction` substrate rather than a second listener), the `openai` oauth registration, the three `subscription_sharing_*` retry literals, the usage hint, and the four-field omission the row's own title omits — `ResponsesTokenKind{ApiKey,ChatGptSignIn}` paired with `ResponsesCredential`, whose single constructor hands out the key and its classification together so "get the key but forget to ask" is not expressible. **REMAINS:** nothing supplies a device id, so `/login` for `openai` fails at its FIRST statement with `Sign in with ChatGPT requires a device ID (UUID) for this installation` — `cyrup-config/src/login.rs:741` passes `LoginOptions::default()` and the only `with_device_id` caller in the tree is a test (`collection.rs:2746`). The supplier needs a persisted `deviceId` in `cyrup-config`'s settings schema, a `get_or_create_device_id()`, a signature change in `cyrup_config::login`, and a `cyrup-tui` caller — three crates and a persisted-schema change. **EVIDENCE CORRECTED, three ways.** (i) The `CORRECTED 2026-10-03` note's "the paste fallback is dropped for this flow" is **wrong**: the `v1.0.0..v1.0.1` diff shows the deleted hunk is the degrade-to-paste on BIND FAILURE; the `manual_code` prompt and its `Promise.race` against the callback are alive at v1.0.1 (`:271-282`). Read literally the row — and the lane brief that repeated it — would have deleted a live branch of upstream behaviour. Correct wording: *the degrade-to-paste on bind failure is dropped; the paste branch itself stays.* (ii) Fix (a) and Fix (e) were ALREADY DONE at HEAD: `LoginOptions`/`GetDeviceIdFn`/`device_id` exist in `auth/mod.rs` citing this very row, `login` takes `options: &LoginOptions` as a required parameter, and the Codex `"(legacy)"` rename is in place — so the row's "nowhere for a device id to arrive" is stale and the real gap was (b), (c), (d) and the literals. (iii) The usage hint is NOT gated on `isChatGPTSignIn`: upstream keys it on the error TEXT alone (`:227-229`, no call to the predicate), and pi's own test drives both arms with a plain api key. Cite drift: the four omission guards are `:335`, `:336`, `:340`, `:344`; `prompt_cache_key` (`:334`) and `service_tier` (`:349`) are the neighbours that are NOT omitted. **Fixed in passing:** `auth::types::Credential` derived `Debug` and printed `key`/`access`/`refresh` in the clear — the leak `cyrup-mcp/src/credentials.rs:96` already names as "the pattern NOT to copy" — now a redacting impl with a red-proved test. The same class remains on `ModelAuth`/`AuthResult`. 43 tests, each red-proved, including controls that catch "omit" degrading into "never send". — **CLOSED 2026-10-05** (`159bcfe1`, completing `209c239d`). The `REMAINS` clause is discharged: `/login openai` now reaches the ChatGPT flow. New `cyrup-config/src/settings/installation.rs` holds `InstallationId` (newtype over the lowercase UUID form, **no `Default`** — there is no valid default installation — and **no `Deserialize`**, because the value arrives as untyped settings JSON and serde is a construction path that would bypass the parse), `DeviceIdDecision{Reuse,KeepUnusable,Create}` as a pure total function of the stored value, and `InstallationIdSupplier` as the imperative shell. `SettingsManager::installation_id` reads `self.global.device_id()` — the GLOBAL document, with deliberately no merged getter — and `login()` takes `&SettingsManager` as a REQUIRED parameter for the same reason `LoginOptions` is required on `OAuthAuth::login`: a flow that needs an id must not be able to silently not receive one, which is the exact failure this row was open for. **EVIDENCE CORRECTED — and one of the errors was in the lane brief, not the row.** (i) The row's `PARTIAL` note said the only `with_device_id` caller was a test at `collection.rs:2746`; there were **seven** test call sites across three files (`auth/oauth/mod.rs:536`, `openai_chatgpt.rs:1299`, `:1339`, `:1475`, `:1536`, `:1616`, `collection.rs:2746`). The substantive point — no production caller — held, but the wording was copied into a brief verbatim and would be copied again. (ii) The row's "three crates and a persisted-schema change" over-counted: two crates of code, plus a doc-comment fix in `cyrup-provider` whose plumbing was already complete. (iii) Upstream states the GLOBAL requirement AND its reason in two places the row never cites — `settings-manager.ts:158` (*"global setting only"*) and the JSDoc at `:1170-1174`: *"Project settings are ignored so a committed project settings file cannot give every clone the same ID."* Verified at the pin. Record those, because the eight-line body alone does not say why the layer matters. **A malformed persisted id is kept, not reminted** — upstream's `if (!deviceId)` is false for any truthy value, so it returns a non-UUID verbatim and the flow then refuses the login with the same message a missing id produces; reminting would destroy a value cyrup did not write AND make a login succeed where pi's fails. **A mechanism difference is recorded WITHOUT a `[CYRUP-DELTA]`:** upstream schedules the settings write on a queue and can lose the id if the process exits before it drains, where cyrup awaits one write; the lane first tagged this a delta and then removed the tag because the marker is not for a timing shift, leaving the explanation as plain prose. Pinned by 12 tests that drive the REAL `OpenAiChatGptOAuth` through the REAL `cyrup_config::login::login` — not the generator in isolation, which would survive the pre-fix state — plus 8 pure tests; reverting to `LoginOptions::default()` reds 9 of the 12 with this row's own message. |
| ~~PROV-119~~ | ~~low~~ **CLOSED 2026-10-09** | not-ported | M | **CLOSED 2026-10-09**: the five env vars resolve LAST in `AnthropicApiKeyAuth::resolve` (`crates/cyrup-provider/src/providers/anthropic.rs`) and travel in `env` with an EMPTY `auth`, and `api::anthropic_messages::federation` performs the exchange cyrup-side: the RFC 7523 `jwt-bearer` body at `POST /v1/oauth/token`, a `[base_url, config]`-keyed token cache, re-exchange once within the refresh margin (the smaller of 60 s and a tenth of the token's lifetime, because a rule may mint 60-second tokens), and the identity-token file re-read on EVERY exchange (a projected token rotates on disk and a `jti`-bearing assertion may be exchanged only once, so a cached one is refused as a replay). The exchange goes through `build_client_for_target`, so it honours a proxy (PROV-047). The minted token is an `Authorization: Bearer` overlay and NEVER enters `ModelAuth::api_key` — load-bearing, because a federated token is `sk-ant-oat01-…` and `claude_code::is_oauth_token` is `api_key.contains("sk-ant-oat")`, so a token in the key slot would flip `is_oauth` and rewrite every tool name to its Claude Code alias on the wire. **TWO CYRUP-DELTAS, both evidenced against Anthropic's published contract rather than pi.** (1) The activation gate is the DOCUMENTED four variables, not pi's three: pi leaves `ANTHROPIC_SERVICE_ACCOUNT_ID` optional (`providers/anthropic.ts:54-63` requires rule id, org id and identity-token file only), but `service_account_id` is a REQUIRED field of the exchange body and the SDK's own activation set is those four (<https://platform.claude.com/docs/en/manage-claude/wif-reference>). pi can be loose because the SDK validates the `config` it is handed; cyrup builds the request itself, so pi's gate would activate federation and then fail with the deliberately opaque `401 Authentication failed`. (2) `ANTHROPIC_IDENTITY_TOKEN` is read as well as `ANTHROPIC_IDENTITY_TOKEN_FILE`; the docs make them alternatives of equal standing and pi reads only the file, so under pi a platform that injects the JWT as a variable cannot federate. `PiAnthropic._shouldResolveDefaultCredentials` stays not-applicable as the row predicted. Both federation env groups joined `CREDENTIAL_ENV_VARS`: the identity token is exchangeable for spend, and a spawned child inheriting the ids beside a projected token file would federate and bill. Docs: `docs/guide/getting-started/authenticate.md`, `docs/guide/reference/environment.md`. **Blockers cleared before starting:** `PROV-109` (closed 2026-10-02) and `PROV-021` (closed 2026-08-14). Verify: `api::anthropic_messages::federation::tests::{the_exchange_sends_the_documented_jwt_bearer_body, an_absent_workspace_id_is_omitted_from_the_body, a_token_is_reused_until_it_nears_expiry, an_expiring_token_is_re_exchanged, the_identity_token_file_is_re_read_on_every_exchange, a_denied_exchange_names_the_authentication_history, a_missing_identity_token_file_names_the_path, the_token_url_is_derived_from_the_api_base, the_four_documented_variables_activate_federation, a_missing_service_account_id_does_not_activate_federation, either_identity_token_source_satisfies_the_gate, request_auth_suppresses_federation, only_the_anthropic_provider_federates, the_minted_token_never_lands_in_the_api_key}` (the exchange tests run against a loopback listener, so the body field names, the URL and the header are asserted on the wire rather than through a mock of our own construction) and `providers::anthropic::tests::{federation_resolves_the_ids_into_env_with_no_credential, every_key_variable_wins_over_federation, a_partial_federation_configuration_resolves_to_nothing, the_inline_identity_token_also_activates_federation}`. The service-account gate and the `api_key` decision were RED-PROVED: reverting to pi's three-variable gate fails `a_missing_service_account_id_does_not_activate_federation`, and moving the token into `api_key` fails `the_minted_token_never_lands_in_the_api_key`. — *Original:* **Anthropic workload identity federation (OIDC) is unported** — pi v1.0.0 resolves five `ANTHROPIC_*` env vars into an `env`-carried federation config (`providers/anthropic.ts:48-69`) that `api/anthropic-messages.ts:351-379` turns into an SDK `config`, bypassing `assertRequestAuth` (`:614`, `:935`). `grep -rn 'ANTHROPIC_FEDERATION\|ANTHROPIC_ORGANIZATION_ID' crates/` at HEAD is empty. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the `assertRequestAuth` calls are at `api/anthropic-messages.ts:615` and `:936` @v1.0.0 (`:613` and `:934` @v1.0.1), and `getAnthropicFederation` at `:351` (`:349` @v1.0.1); the `:614`/`:935` cites are off by one. The row's own Fix names the SDK-less OIDC token exchange as "the real work" and depends on `PROV-109` and `PROV-021`. Recommended re-rating (not applied): effort M→L. |
| ~~PROV-120~~ | ~~low~~ **CLOSED 2026-10-08** | not-ported | M | **CLOSED 2026-10-08**: `AnthropicOAuth`'s `OAuthAuth::login` now opens with upstream's `Select` prompt (`Select Anthropic login method:`; `browser` / "Browser login (default)", then `copy_code` / "Copy code login (headless)") and dispatches `browser` to the unchanged `run_login`, `copy_code` to the new `login_copy_code`, and anything else to `Unknown Anthropic login method: {id}`. `login_copy_code` is a 1:1 port of `loginAnthropicCopyCode` (`anthropic.ts:200-235` @v1.1.0): the authorize URL against `COPY_CODE_REDIRECT_URI` (`https://platform.claude.com/oauth/code/callback`), one `manual_code` prompt placeholdered `code#state` carrying the login-wide cancel, `parse_authorization_input`, the `OAuth state mismatch` / `Missing authorization code` checks, the progress notify, and the exchange against the copy-code redirect with `state ?? verifier`. **No callback listener is started.** The `Missing OAuth state` check is not copied: upstream has none in either flow (dropped in `4df157433`, v0.99.0), so `code#` exchanges with `state: ""`; the browser flow's own leftover check was removed in the same pass (`PROV-135`), so the two flows agree on pasted input. The browser flow also gained upstream's free-port fallback, `8d8ae2fc2` (`PROV-136`), and, from the review pass, upstream's shared callback handler, so a denied consent ends the login instead of leaving it waiting (`PROV-137`); only then did both flows match upstream. Every login entry point (`/login`, `/login anthropic`, the provider picker, the API-key-or-subscription chooser) reaches the selector; ACP's terminal login relaunches the TUI. The TUI already rendered `Select` and `manual_code`; the headless audit fixed what still stood between a headless user and a pasted code: the sign-in URL had no copy key (`TUI-145`) and no OSC-8 link (`TUI-167`), the `code#state` placeholder was dropped (`TUI-168`, a recorded `[CYRUP-DELTA]`), and the browser launcher shared the TUI's terminal, so a console browser `xdg-open` falls back to could steal keystrokes or re-mode the tty; it now runs in a `setsid` session of its own, with no controlling terminal (`TUI-169`). Every existing login test now answers the selector with `browser` first. Verify (`auth/oauth/anthropic.rs` tests): `copy_code_login_exchanges_against_the_copy_code_redirect` (the callback server's test-only `bind_attempts` record, `auth/oauth/callback.rs:305-307`, `:508`, shows no start on the flow's TEST-NET-1 sentinel bind host `192.0.2.120`, and a control start on that host is recorded, so the empty record is not vacuous), `login_offers_browser_first_then_copy_code`, `login_rejects_an_unknown_method`, `copy_code_login_propagates_prompt_cancellation`, `copy_code_login_rejects_state_mismatch`, `copy_code_login_rejects_empty_paste_as_missing_code`, `copy_code_login_defaults_state_like_upstream`, and `login_completes_via_browser_redirect` (the `browser` choice still reaches the loopback redirect, and the exchange uses the advertised redirect URI). End to end through the registry: `crates/cyrup-provider/src/tests/builtin_oauth.rs` `the_anthropic_login_reaches_the_method_selector_and_copy_code_flow` (the strategy from `all_providers()`, the chain `/login` uses; no port bound, no network). Through the TUI: `crates/cyrup-tui/src/tests/login_flow.rs` `real_anthropic_login_offers_copy_code_and_shows_a_copyable_headless_url` (selector, copy-code URL, `code#state` field, ctrl+x copies the exact URL, cancel) and `copy_code_login_completes_from_the_tui_with_a_pasted_code` (the real Anthropic login with only its token endpoint faked: the pasted `code#state` is exchanged against the copy-code redirect and a credential is stored). Verified in the real `cyrup` binary in a headless tmux pty (no `DISPLAY`/`BROWSER`, scratch `HOME`): the selector lists browser first; copy-code shows the full authorize URL (`redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback`, `code_challenge`, `state`), the instructions and the `code#state` field, and ctrl+x puts the exact URL on the clipboard (OSC 52); a fake `abc#<state>` reaches `Exchanging authorization code for tokens...` and the live endpoint's `invalid_grant` is reported cleanly; a wrong state fails with `OAuth state mismatch`. With the token endpoint answered by a local TLS-intercepting proxy (`HTTPS_PROXY` plus a test CA in `SSL_CERT_FILE`; there is no token-URL override, as upstream has none), copy-code and browser login (with 53692 held by another process, so the URL advertised an OS-chosen port, completed both by the loopback redirect and by a pasted redirect URL) store an `oauth` credential in `auth.json` (mode 600). — *Original:* **The Anthropic copy-code (headless) login method and the login-method selector are unported** — pi v1.0.0 prompts `type:"select"` between browser and copy-code login and adds `loginAnthropicCopyCode` against `https://platform.claude.com/oauth/code/callback` (`auth/oauth/anthropic.ts:21`, `:191-226`, `:273-290`). cyrup's `OAuthAuth::login` for anthropic goes straight to the browser flow. **FILED 2026-10-02**; body below. |
| ~~PROV-121~~ | ~~low~~ **CLOSED 2026-10-08** | upstream-drift | S | **CLOSED 2026-10-08** (checked against pi ce950d78f): `utils/constrained_sampling.rs` now has pi's `UnsupportedStrictSchemaKeywordCheck` (`dyn Fn(&str, &Value) -> bool`, `constrained-sampling.ts:13`) as an optional third argument of `resolve_json_schema_strict_sampling` and second of `make_strict_json_schema`, checked over every key of every node after the fixed deny-list and passed down all three recursion edges (`:56`, `:65-71`, `:81`, `:89`, `:117`, `:228`); a rejected keyword is the same conversion error, rendered `${key}: ${JSON.stringify(value)} is unsupported`, so `prefer` degrades to non-strict and `require` fails naming it. `json_schema_tool_parameters` passes `None`, as `getJsonSchemaToolParameters` does (`:142-144`). `api/anthropic_messages/tools.rs` ports `isAnthropicStrictUnsupportedKeyword` (`anthropic-messages.ts:1541-1574`: eleven keywords, `minItems` only `0`/`1`, `format` only the ten listed strings) and passes it in `convert_tools` (`:1586`); the six other call sites (`openai_completions`, `openai_responses`, `bedrock_converse_stream`, `google_generative_ai` x2, `mistral_conversations`) and `cyrup-ext/src/wrapper.rs:705` pass `None`. Verify: `utils::constrained_sampling::tests::prov121_a_rejected_keyword_degrades_prefer_and_fails_require`, `prov121_min_items_and_format_have_allowlists`, `prov121_the_predicate_is_checked_at_every_depth` (red at HEAD: the argument did not exist), and `api::anthropic_messages::tests::tools::a_rejected_keyword_sends_the_tool_non_strict` (run RED with the Anthropic call site passing `None`: the body carried `strict: true`). — *Original:* **Anthropic strict tool use is sent for schemas Anthropic's strict mode rejects with a 400 for the whole request** — pi v1.0.0 threads an `UnsupportedStrictSchemaKeywordCheck` (`api/constrained-sampling.ts:13`, `:56`, `:228`) and supplies an Anthropic predicate over 11 keywords plus a `format`/`minItems` allowlist (`api/anthropic-messages.ts:1550-1583`, applied `:1593`). cyrup's `resolve_json_schema_strict_sampling` takes two arguments and has no hook. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** v1.0.1 line shift: the predicate is `api/anthropic-messages.ts:1569` and is applied at `:1586` (was `:1576`/`:1593` @v1.0.0); the wire change is otherwise identical. Production callers of `resolve_json_schema_strict_sampling` (`utils/constrained_sampling.rs:402`) are the seven provider converters plus `cyrup-ext/src/wrapper.rs:530`, so the signature change touches all of them. |
| ~~PROV-122~~ | ~~low~~ **CLOSED 2026-10-08** | upstream-drift | S | **CLOSED 2026-10-08** (on `claude/provider-drift-sweep`, checked against pi ce950d78f): `apply_message_delta_usage` (`api/anthropic_messages/usage.rs`) now reads `cache_creation.ephemeral_1h_input_tokens` into `cache_write_1h` under upstream's `!= null` guard (pi `api/anthropic-messages.ts:841-847` @ce950d78f, unchanged from v1.0.1), assigning only when the key is present so an absent or empty breakdown keeps the `message_start` value; no zero default on this path. Verify: `api::anthropic_messages::tests::usage::prov122_a_delta_only_breakdown_is_priced_at_the_one_hour_rate` (`cache_write_1h == Some(1000)` and `cost.cache_write` = 500 short at `cacheWrite` + 1000 long at 2x input; red at HEAD), `prov122_a_delta_without_a_breakdown_keeps_the_start_value`, `prov122_a_delta_with_an_empty_breakdown_keeps_the_start_value`. — *Original:* **`message_delta` does not read `cache_creation.ephemeral_1h_input_tokens`, so one-hour cache writes reported only in deltas are priced at the five-minute rate** — pi v1.0.0 reads it in both places (`api/anthropic-messages.ts:686` and `:843-849`, `667fc3dd3`). cyrup's `apply_message_delta_usage` (`api/anthropic_messages/usage.rs:31-53`) reads only `cache_creation_input_tokens`. Discharges lead (c) at `01-…:636`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** v1.0.1 line shift: `api/anthropic-messages.ts:684` (`message_start`) and `:843-846` (`message_delta`), was `:686` and `:845-849` @v1.0.0. cyrup's `apply_message_delta_usage` is at `api/anthropic_messages/usage.rs:31`, and `apply_message_start_usage` (`:7`) already reads `ephemeral_1h_input_tokens` (`:21-24`). |
| ~~PROV-123~~ | ~~low~~ **CLOSED 2026-10-10** | upstream-drift | S | **CLOSED 2026-10-10** (on `claude/zealous-bell-x0u1h0` off `main` @ `62502ff8`, checked against pi f1b2e77f5). **The production change landed in #215 (`52b1aa33`)** as the request half of area 05's `CFG-104`; this closure adds the Verify test and the doc rewrite the row asks for. **The Fix text is superseded upstream:** `c01f687e5`'s "pass `options?.samplingParams` through unmerged" was replaced by `76dfb88f6` (#9776, v1.0.2). At f1b2e77f5 BOTH sites call `resolveSamplingParams` (`api/simple-options.ts:24-34`): `buildBaseOptions` (`:42`) and the tail of each OpenAI-compatible `buildParams` (`api/openai-completions.ts:1004-1007`, `api/openai-responses.ts:383-386`, `api/azure-openai-responses.ts:243-246`). Resolving twice gives the same result, so cyrup mirrors both sites rather than "copy through unchanged". cyrup: `resolve_sampling_params` (`crates/cyrup-provider/src/utils/simple_options.rs:81`) is called by `build_base_options` (`:130`) and by the shared `apply_sampling_params` (`api/openai_completions/params.rs:286`), which resolves model defaults itself instead of applying only `opts.sampling_params`. The completions (`params.rs:247`), responses (`api/openai_responses/params.rs:313`) and Azure (`api/azure_openai_responses.rs:438`) builders call it. `merge_sampling_params` is gone (`rg merge_sampling_params crates/cyrup-provider` is empty), so the empty-map question is settled by pi's own guard (`simple-options.ts:31`), which `resolve_sampling_params` mirrors. **Verify, per clause** (all in `crates/cyrup-provider/src/tests/sampling_params.rs`): (1) *each adapter, `StreamOptions::default()`, model `{"top_p": 0.5}`, `top_p` in the body*: `prov123_each_openai_compatible_adapter_applies_model_level_params_on_a_direct_stream` (`:615`; `:549` before `PROV-150`'s test was inserted above it). It loops over the three apis through `direct_payload` (`:330`), which calls each adapter's body builder with no `build_base_options` in front, and asserts on the outgoing body. It also ports pi `test/sampling-options.test.ts:114-125` @f1b2e77f5 (model `{top_p: 0.95, min_p: 0.05}`, request `{top_p: 0.5}`: `top_p` 0.5, `min_p` 0.05). "Red at HEAD" cannot be shown literally, because #215 is already on HEAD; it is red with the fix undone instead. With `apply_sampling_params` reverted to the request-map-only shape, it fails at openai-completions (`left: None`, `right: Some(0.5)`), re-run by this closure. (2) *the `build_base_options` cases stay green*: all eight `agent026_*` cases pass (workspace nextest on the final tree: 15344 passed). (3) *the `:141` test's doc rewritten, not deleted*: `agent026_applies_model_level_sampling_params` (now `:175`, doc `:165-173`) keeps why the merge lived in `build_base_options` at v0.84.1, names `c01f687e5` as the reversal and points to the direct-path test. The module doc (`:1-19`) now describes both shapes. *Original:* ~~**The `model.sampling_params` merge moved out of `buildBaseOptions` into the three OpenAI-compatible `buildParams`, so a direct api-level `stream()` now keeps the model's defaults** — pi v1.0.0 `api/openai-completions.ts:999` / `openai-responses.ts:363` / `azure-openai-responses.ts:347` are `Object.assign(params, model.samplingParams, options?.samplingParams)` and `api/simple-options.ts:29` passes `options?.samplingParams` through unmerged (`c01f687e5`, #9506). cyrup keeps the v0.87.1 shape. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** citation fix: the three `Object.assign(params, model.samplingParams, options?.samplingParams)` lines are `api/openai-completions.ts:999`, `api/openai-responses.ts:382` and `api/azure-openai-responses.ts:348` (same at v1.0.0 and v1.0.1). `openai-responses.ts:363` is the v0.87.1 line and `azure-openai-responses.ts:347` is off by one. `api/simple-options.ts:29` is correct. cyrup's merge is `utils/simple_options.rs:57-105`.~~ |
| ~~PROV-124~~ | ~~low~~ **CLOSED 2026-10-08** | upstream-drift | S | **CLOSED 2026-10-08** (checked against pi ce950d78f): `retry_delay_ms` (`crates/cyrup-provider/src/utils/provider_retry.rs`) now ports pi's `Number.isFinite` gates on both header branches (`utils/provider-retry.ts:57`, `:64` @ce950d78f; filed as `:55`/`:62` at v1.0.0, moved down by the `noRetryStatuses` option added at `:7-8`). A non-finite `retry-after-ms` (NaN, `Infinity`, `1e999`, `-1e999`) defers to `retry-after`. An unparseable or non-finite `retry-after` falls through to the jittered exponential ladder; it no longer substitutes `0.0`. The comment that documented the old behaviour is replaced. `an_unparseable_retry_after_retries_immediately` is inverted into `an_unparseable_retry_after_falls_back_to_exponential_backoff`, with siblings `an_infinite_retry_after_falls_back_to_exponential_backoff` and `an_infinite_retry_after_ms_is_skipped`. All three were red against the pre-fix body. ~~**An unparseable `retry-after` retries immediately instead of falling back to exponential backoff** — pi v1.0.0 gates both header branches on `Number.isFinite` (`utils/provider-retry.ts:55`, `:62`, `2bbfcca43`). cyrup's `retry_delay_ms` substitutes `0.0` for the NaN case on purpose and pins it with `an_unparseable_retry_after_retries_immediately` (`utils/provider_retry.rs`). **FILED 2026-10-02**; body below.~~ |
| ~~PROV-125~~ | ~~low~~ **CLOSED 2026-10-08** | upstream-drift | S | **CLOSED 2026-10-08** (checked against pi ce950d78f): `OVERFLOW_PATTERNS` (`crates/cyrup-provider/src/utils/overflow.rs`) now carries `r"prompt exceeds max length"` as its second entry, in pi's position with pi's comment (`packages/ai/src/utils/overflow.ts:39` @ce950d78f), and the module doc names both z.ai bodies (`overflow.ts:30`). A z.ai CN-endpoint `Prompt exceeds max length` is now an overflow that auto-compaction can act on. Verify: `utils::overflow::tests::zai_prompt_too_long_and_provider_gated_bodyless` asserts `400 {"code":"1261","message":"Prompt exceeds max length"}` from `zai` is an overflow (pi `test/overflow.test.ts:44-48`); red before. `tests::overflow_estimate_parity::overflow_pattern_set_matches_pi_cardinality` now pins 25 entries (`overflow.ts:37-63`). Was: **The z.ai CN endpoint's overflow message `Prompt exceeds max length` is not classified as a context overflow**; body below. |
| ~~PROV-126~~ | ~~low~~ **CLOSED 2026-10-08** | upstream-drift | S | **CLOSED 2026-10-08** (on `claude/provider-drift-sweep`, checked against pi ce950d78f): pi's `providerHeadersToRecord` (`utils/headers.ts:11-23`) is unchanged, and its callers are now `google-generative-ai.ts:358`, `google-vertex.ts:399`, `openrouter-images.ts:130`, `llama-cpp-classify.ts:236`, `classifier-shared.ts:47` (the system-one helper moved there), `pi-messages.ts:398` and `bedrock-converse-stream.ts:258`; `anthropic-messages.ts:293-301` is still a case-sensitive `Object.assign`, so cyrup's Anthropic `build_headers` is left alone (the CORRECTED premise stands). New `utils/headers.rs` holds the shared helper: `provider_headers_to_record` (generalized out of `api/llama_cpp_classify.rs`, which now imports it) and `merge_provider_headers`, the same case-insensitive last-wins merge that keeps a `None` as a tombstone so cyrup's transport and the `User-Agent` default still read it as suppression. `api/google_generative_ai/endpoint.rs`, `api/google_vertex.rs` and `api/openrouter_images.rs` now merge their defaults and overlays through it; the merge order is unchanged. Not changed: `bedrock_converse_stream/headers.rs` already lower-cases its keys; `pi_messages.rs` collapses only `options.headers` upstream and then object-spreads it over its three defaults case-sensitively, which the existing overlay already mirrors for casing (it differs in one respect: a `None` in cyrup's overlay suppresses a default, which a `null` in pi's `options.headers` cannot). Verify: `utils::headers::tests::{a_later_source_overrides_an_earlier_one_case_insensitively, the_surviving_entry_keeps_the_last_writers_spelling, a_later_none_removes_the_earlier_header_in_any_casing, absent_sources_are_skipped_and_a_reset_header_moves_last}`; `google_generative_ai::tests::endpoint::differently_cased_overlays_replace_the_default_header`, `google_vertex::tests::differently_cased_overlays_replace_earlier_headers`, `openrouter_images::tests::request_headers_override_model_headers_case_insensitively`. All three call-site tests fail on the old code with two headers. The Anthropic test in the Verify is not added, because upstream Anthropic is case-sensitive. — *Original:* **Request-header merging is case-sensitive, so a `model.headers` or `options.headers` entry whose casing differs from the builtin default is emitted as a second header instead of overriding it** — pi v1.0.0 made `providerHeadersToRecord` variadic and case-insensitive last-wins with `null` removal (`utils/headers.ts:11-23`). cyrup's `HeaderMap` is `BTreeMap<String, Option<String>>` (`lib.rs:223`) and `build_headers` inserts each overlay under its own spelling (`api/anthropic_messages/headers.rs:189-211`). **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the premise about Anthropic is false. `api/anthropic-messages.ts` contains no use of `providerHeadersToRecord` at v1.0.0 or v1.0.1; pi's Anthropic path still merges with `mergeHeaders` (`:293-301`), a plain `Object.assign`, which is case-sensitive, so cyrup's Anthropic `build_headers` (`api/anthropic_messages/headers.rs:189-211`) already matches upstream. The real callers of the case-insensitive helper (`utils/headers.ts:11-23`) are `google-generative-ai.ts:358`, `google-vertex.ts:399`, `bedrock-converse-stream.ts:258`, `pi-messages.ts:398`, `system-one-shared.ts:166`, `openrouter-images.ts:130` and `llama-cpp-classify.ts:236`, so the gap exists only in cyrup's non-Anthropic ports of those. The owning upstream commit is `a328aa89a` ("unify image and classifier model infrastructure"), not the one cited. |
| ~~PROV-127~~ | ~~low~~ **CLOSED 2026-10-10** | upstream-drift | S | **`AssistantMessage.thinkingLevel` is unmodelled, so the pi level the loop requested is absent from the record and from the JSONL key order** — pi v1.0.0 declares it between `providerThinkingLevel` and `diagnostics` (`types.ts:553-558`). cyrup's `AssistantMessage` has `provider_thinking_level` only (`crates/cyrup-core/src/message/assistant.rs:65`), and its hand-written serializer's field list (`:152`, `:166-192`) has no slot for it. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the key-order claim is wrong. `types.ts:555-557` @v1.0.1 declares `thinkingLevel?` between `providerThinkingLevel` and `diagnostics` in the TYPE, but the only producer is `packages/agent/src/agent-loop.ts:409`, `Object.assign(await response.result(), { thinkingLevel: config.reasoning ?? "off" })`, which appends the key at runtime after the keys the adapter already set, on the final result only. There is therefore no fixed JSONL slot between `providerThinkingLevel` and `diagnostics`, and the Fix's "serialize it between ..." and byte-order Verify are unsupported; where the key lands in a persisted message depends on the loop, which is cross-area with area 03/agent loop. The field's existence, severity and effort stand. — **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5). The fix had already landed with the virtual-models port (`05bcff1`): `AssistantMessage::thinking_level: Option<ModelThinkingLevel>` (`crates/cyrup-core/src/message/assistant.rs`), serialized as `thinkingLevel` between `providerThinkingLevel` and `diagnostics`, counted in `len`, and stamped by the loop on the settled message before `message_end` (`crates/cyrup-agent/src/agent/run/stream.rs`, `Settled::with_thinking_level`, pi `agent-loop.ts:409` `Object.assign(..., { thinkingLevel: config.reasoning ?? "off" })`). This closure adds the tests the Verify names, each red-proved against the reverted non-test hunk. Verify mapping: (a) `None` absent and bytes identical — `assistant.rs::thinking_level_serializes_in_its_slot_and_is_absent_when_unset` pins the pre-field bytes literally; (b) `Some(Max)` as `"thinkingLevel":"max"` between `providerThinkingLevel` and `diagnostics` — same test; (c) interop round-trip as `PROV-012` — `cyrup-test-support` `deferred_interop.rs::thinking_level_survives_import_and_re_export` and `::a_turn_without_a_thinking_level_does_not_gain_one`. Beyond the Verify, the population half is pinned by `cyrup-agent` `tests/model_boundary.rs::settled_assistant_messages_record_the_requested_thinking_level` (per-request level incl. a `TurnUpdate` override, on `message_end`, and `off` when reasoning is off). **Clause not fully met, by design:** the 2026-10-03 correction stands — pi writes the key LAST (after `timestamp`/`durationMs`) because it is appended at runtime, so a pi-written line re-exports value-equal but not byte-equal (cyrup emits the type's declared slot). The interop fixture spells the pi line in pi's runtime order to prove the value survives. No `cyrup:ext` WIT change was needed: assistant messages cross that seam only as JSON strings. |
| ~~PROV-128~~ | ~~medium~~ **CLOSED 2026-10-05** | upstream-drift | L | **Image models are a second, parallel registry that pi v1.0.0 deleted — `ModelType`/`AnyModel` have no `Image` variant, so no image model can reach a `Provider` or the `Models` collection** — v1.0.0 folds images into the one provider surface (`createProvider({ images })`, `providers/openrouter.ts:33`; `ModelType = "chat" \| "image" \| "classifier"` and `AnyModel`, `types.ts:1158-1176`) and deletes `images-models.ts`, `providers/openrouter-images.ts` and `builtinImagesProviders`/`builtinImagesModels` (`providers/all.ts:188`). cyrup still ships the whole separate `images::{ImagesProvider, ImagesModels}` tree plus `providers/openrouter_images.rs`, and `AnyModel` is `Chat \| Classifier` (`classifier.rs:204-207`). Discharges post-tag lead (b) at `01-…:637`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the structure is confirmed (`types.ts:1158-1176` @v1.0.1, `providers/openrouter.ts:33`, `classifier.rs:204-207`), but the "silent per-row drop" impact is not occurring today. Pi's client asks the catalog endpoint for `?types=chat,image,classifier` (`packages/coding-agent/src/core/remote-catalog-provider.ts:22`, `:102`, new in v1.0.0); cyrup's `remote_catalog.rs:637-641` builds the URL with no `types` parameter. Checked live on 2026-10-03: `https://pi.dev/api/models/providers/openrouter` returns 397 rows, all chat, and with `?types=chat,image,classifier` returns 463 (397 chat, 57 image, 9 classifier). So image rows never reach cyrup and nothing is dropped; the missing `types` parameter is itself a prerequisite for step (1). The `providers/all.ts:188` cite for the deleted `builtinImages*` functions is wrong (those were at v0.87.1 `all.ts:145-157`). Recommended re-rating (not applied): medium→low (no behaviour is broken; effort L stands). — **CLOSED 2026-10-05** (`0032cc08`). **The row's recommended `medium`→`low` re-rating is DECLINED by owner decision: cyrup wants image generation, so the full port landed, not a deletion.** `?types=chat,image,classifier` on the catalog URL, `AnyModel::Image` now CONSTRUCTED in production (it previously existed with zero production constructors — the `SUBA-155` fail-open shape), `images` hung off `Provider` with `generate_images` on the collection, and the parallel `ImagesProvider`/`ImagesModels` tree retired. `split_by_type` converts pi's runtime `isModelType(…,"chat")` filters into three nominally-typed lists at the parse boundary, so `models()` cannot hand a chat caller an image row and `generate_images(&ImageModel)` makes pi's `assertImageModel` a compile-time fact. **EVIDENCE CORRECTED — the row's central deletion claim is half wrong, and following it costs six public items.** `images-models.ts` and `providers/openrouter-images.ts` ARE deleted at v1.0.1, but `images.ts` and `images-api-registry.ts` are **PRESENT and re-exported from `compat.ts:25-29`** — the global api-keyed registry and the standalone `generateImages()` sit BESIDE a provider's `images` map, not instead of it. The lane deleted cyrup's port of them and restored `ImagesApiImpl`, `ImagesApiFactory`, `ImagesApiProviderRegistry`, `images_builtin_registry`, `register_images_builtins` and the free `generate_images` after checking the pin. **Also uncorrected by the row's own note:** the two responses have DIFFERENT SHAPES — no `types` returns a model-id-keyed OBJECT (400 rows, all chat), `?types=` returns an ARRAY (469: 400 chat, 59 image, 10 classifier) — so adding the parameter without handling the array form breaks every refresh on day one. **Three defects found in scope and fixed, all red-proved:** (a) `RemoteCatalogProvider` forwarded neither `get_all_models` nor `classify`/`supports_classification`, latent at HEAD and live the moment `openrouter` gained image rows; (b) `cyrup-config`'s `FileModelsStore::members()` partitioned chat-vs-CLASSIFIER, so the first persisted image row would land in the chat half, fail the entry's typed conversion on `missing field reasoning`, and make `read()` return `None` — costing that provider its ENTIRE persisted overlay, chat rows included; (c) `write_classifier_models` rebuilt the array as `chat ++ classifiers`, deleting every other non-chat member. **One genuine loss averted:** retiring `ImagesModels` removed the only way to resolve auth for a non-chat model (`Models::get_auth` takes `&Model`), ported as `Models::get_any_auth`, which also closes the same hole for `ClassifierModel`. Stale cyrup cites, same `81200403` batch: `ModelType` is three-valued at `:56-60` not two at `:43-46`; `AnyModel`'s deserialize has an `image` arm at `:423` rather than rejecting it; `struct ImageModel` exists at `classifier.rs:225`. Fixtures are verbatim 2026-10-05 captures, so the tests do not rot as the endpoint drifts. **Residuals, each owned by another row:** `ModelsPersist` has no `images` arm (`EXT-027`), `get_available` stays chat-only (`EXT-027`), openrouter's classifiers cannot classify (`PROV-104`), and `xtask`'s live-catalog URL lacks `?types=` (`PROV-129`, whose stated dependency is now satisfied). |
| PROV-129 | low | tooling | M | **`PROV-089`'s named deadline has arrived: `image-models.generated.ts` is deleted at v1.0.0, so `openrouter-images.json` is now generated from a source that exists at no current or future tag** — v1.0.0 replaces it with `IMAGE_MODELS` in `models.generated.ts` (`providers/all.ts:1`) and moves the rows under the `openrouter` provider. cyrup's `xtask/src/main.rs:123` still pins `IMAGES_REV = "v0.87.1"` as the one non-live catalog, with a comment asserting there is no live endpoint. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the Fix's claim that the rows "arrive on the live endpoint cyrup already fetches" is wrong without a `types` parameter: `pi.dev/api/models/providers/openrouter` returns no image rows unless called with `?types=image` (checked 2026-10-03; see `PROV-128`), and neither `remote_catalog.rs:637` nor xtask's live URL shape (`https://pi.dev/api/models/providers/{file}`, asserted at `xtask/src/main.rs:1664-1667` and `:2042-2045`) carries it. v1.0.1 also adds a `?types=...` catalog revision flow (`packages/ai/scripts/hydrate-model-catalog.ts`, `scripts/update-model-catalog-pin.mjs`). The deletion itself is confirmed (`image-models.generated.ts` absent at v1.0.0 and v1.0.1; `xtask/src/main.rs:123` is `IMAGES_REV = "v0.87.1"`), and the interim comment and manifest-note correction stands; the real prerequisite for the full fix is the `types` parameter. |
| ~~PROV-130~~ | ~~medium~~ **CLOSED 2026-10-04** | upstream-drift | S | **CLOSED 2026-10-04** (`506f8e94`): `supports_thinking_block_binding` (`api/bedrock_converse_stream/capabilities.rs`) is upstream's five-needle predicate — `opus-4-7`, `opus-4-8`, `opus-5`, `sonnet-5`, `fable-5`, strictly narrower than the seven needles of `supports_adaptive_thinking` — and `build_additional_model_request_fields` (`params.rs`) sends `thinking.block_binding={prefix_mismatch_behavior:"drop_block"}` plus `anthropic_beta:["thinking-binding-controls-2026-08-01"]` behind `!is_gov_cloud && supports_thinking_block_binding(model)` on the adaptive branch only; `INTERLEAVED_THINKING_BETA` still rides the budget-based branch alone, so the two betas cannot collide, and GovCloud still omits `display`. Verify: `thinking_block_binding_rides_only_the_adaptive_models_that_accept_it` (red both when the needle list is emptied and when it is widened to the adaptive seven — Opus 4.6 must carry no `block_binding`), `the_budget_based_branch_never_carries_block_binding`, `adaptive_models_send_adaptive_thinking_and_an_effort`, `govcloud_omits_the_thinking_display_field`. **Ledger correction:** the new doc comments carry upstream line numbers from an earlier pi version — @v1.0.1 `buildAdditionalModelRequestFields` is `:1254` (comment: `:1039-1087`), `supportsAdaptiveThinking` is `:766` (comment: `:588-600`) and `useBlockBinding` is `:1270` (comment: `:1266`). — *Original:* **Bedrock Claude replays do not send `thinking.block_binding`, so a changed system prompt or tool list invalidates replayed signed thinking blocks and the request is rejected** — pi `69f0be6f0` adds `supportsThinkingBlockBinding` (`api/bedrock-converse-stream.ts:792-806` @v1.0.1: `opus-4-7`, `opus-4-8`, `opus-5`, `sonnet-5`, `fable-5` only; Opus 4.6 and Sonnet 4.6 reject the field) and, off GovCloud, sends `thinking.block_binding={prefix_mismatch_behavior:"drop_block"}` plus `anthropic_beta:["thinking-binding-controls-2026-08-01"]` on the adaptive branch (`:1262-1282`). cyrup's adaptive branch (`api/bedrock_converse_stream/params.rs:151-163`) sets only `type`, `display` and `effort`; `block_binding` appears only in the Anthropic adapter (`api/anthropic_messages/params.rs:270`). **FILED 2026-10-03**; body below. |
| ~~PROV-131~~ | ~~medium~~ **CLOSED 2026-10-04** | upstream-drift | S | **CLOSED 2026-10-04** (`26682ccb`): both embedded catalogs are regenerated at pi v1.0.1 — `providers/catalog/cloudflare-ai-gateway.json`'s nine dotted `claude-*` ids on the `/anthropic` passthrough are now dashed (54 rows in, 54 out; each new row byte-identical to its dotted predecessor apart from `id`), and `providers/catalog/amazon-bedrock.json` carries models.dev pricing tiers (23 rows gained `cost.tiers`, 9 rows added, none retired, and no pre-existing row moved a non-tier cost field, a context window or a compat flag). The dash rewrite stays scoped to the one upstream `scripts/generate-models.ts:1969` scopes it to, so dotted ids survive wherever they are legal (`workers-ai/@cf/zai-org/glm-5.3`, `openai.gpt-5.6-*`, github-copilot's `claude-sonnet-4.6`) — porting `replaceAll(".", "-")` into xtask would have been wrong. `gen-catalogs --only <stem>,…` refreshes a named subset and routes the rest through the existing `LiveOutcome::Skipped` path with provenance carried forward; `--check --only amazon-bedrock,cloudflare-ai-gateway` reproduces the committed data byte-for-byte against pi.dev and its summary says the other 36 were not checked. Verify: `bedrock_openai_rows_keep_their_long_context_pricing_tier` (23 tiered rows, 0 before), `providers::cloudflare::tests::ai_gateway_claude_ids_are_dashed_for_the_anthropic_passthrough`, and the two xtask `--only` tests. **Residual, not this row's defect:** the other 36 catalogs stay as stale as they were. — *Original:* **Embedded catalogs predate pi 1.0.1's generator fixes: dotted Cloudflare AI Gateway Claude ids that the `/anthropic` passthrough does not accept, and Bedrock rows stripped of models.dev pricing tiers** — `c10bfb0d7` applies `nativeId.replaceAll(".", "-")` for Cloudflare AI Gateway's anthropic upstream (`scripts/generate-models.ts:1969` @v1.0.1) and `4665fafb4` routes Bedrock cost through `getModelsDevCost` so tiers survive (`:1795`). cyrup `providers/catalog/cloudflare-ai-gateway.json` still holds nine dotted `claude-*` ids on `anthropic-messages` (e.g. `claude-opus-5.5` at `:309`), and `providers/catalog/amazon-bedrock.json` has no `tiers` object anywhere (`openai.gpt-5.5` at `:3639`). **FILED 2026-10-03**; body below. |
| ~~PROV-132~~ | ~~low~~ **CLOSED 2026-10-10** | stale-port | S | **Hand-ported Together DeepSeek V4 Pro uses the id `deepseek-ai/DeepSeek-V4-Pro`, which pi 1.0.1 renamed to `deepseek-ai/DeepSeek-V4-Pro-0813` to keep its thinking-level controls** — `28eaccb8e` changes `TOGETHER_TOGGLE_REASONING_EFFORT_MODELS` to `deepseek-ai/DeepSeek-V4-Pro-0813` (`scripts/generate-models.ts:201`; `test/together-models.test.ts:61`). cyrup `providers/together.rs:197-209` carries only the old id with the high-only level map and its tests look it up by the old id (`:449`, `:561`). **FILED 2026-10-03**; body below. — **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5): `providers/together.rs` `together_models()` now carries the row as `deepseek-ai/DeepSeek-V4-Pro-0813` ("DeepSeek V4 Pro 0813") with the high-only level map and `together_compat(true, Together)`, as `TOGETHER_TOGGLE_REASONING_EFFORT_MODELS` (`generate-models.ts:197` @f1b2e77f5) selects them; the old id is dropped, not aliased — pi has no alias or migration for it and `pi.dev/api/models/providers/together?types=chat` (fetched 2026-10-10) no longer lists it. Verify: `full_catalog_ported_from_pi` finds `-0813` with exactly pi's map, asserts the old id is absent; `catalog_models_encode_reasoning_per_pi` encodes it. Red before. |
| ~~PROV-133~~ | ~~low~~ **CLOSED 2026-10-09** | not-ported | L | **CLOSED 2026-10-09**, after `AGENT-039` unblocked it in the same pass. `compat::uses_native_tool_changes` is pi's gate verbatim (`anthropic-messages.ts:1139-1141` @v1.1.0) and is shared by the header and params builders, so the `inline-tools-2026-09-15` beta and the body cannot disagree about which shape was sent. Under it the request-level `tools` is FIXED at the initial system message's `toolsAdded` plus `__cyrup_deferred_placeholder__` (`defer_loading: true`), cache breakpoint on the last initial tool, and the deferred split is not consulted because the list is fixed by construction; later system messages become `{role:"system"}` carrying text, then `tool_removal` (`tool_reference`, SKIPPED for a name that is also re-added, since a definition replaces by name), then `tool_addition` (`tool_definition` by value), held and flushed immediately before the next assistant turn and once after the loop so a trailing one survives. `convert_messages` gained pi's optional `convertToolDefinitions`; `None` keeps the previous behaviour byte for byte, which is what every model that cannot express the blocks needs. The v1.0.0 `tool_reference` shape was NOT ported, as this row instructed. **CYRUP-DELTA on one name:** the placeholder is `__cyrup_deferred_placeholder__` where pi uses `__pi_deferred_placeholder__` — cyrup's own scaffolding, never callable and never user-visible, and the only property the wire depends on is byte-stability across requests, which a constant has. Verify: `api::anthropic_messages::tests::inline_tools::{a_mid_run_add_remove_and_redefine_emits_the_native_blocks, the_initial_system_message_is_not_emitted_as_blocks, no_initial_tool_means_no_native_shape, a_model_without_the_capabilities_keeps_the_old_shape}`. The first is this row's own Verify line. Both load-bearing rules were RED-PROVED: dropping pi's `initialTools.length > 0` term fails `no_initial_tool_means_no_native_shape`, and removing the replace-by-name rule fails the add/remove/redefine test. The other five adapters needed no change — they skip `Message::System` in their message loops, so restoring declarations to the transcript cannot make them double-declare. — *Earlier annotation, kept because its diagnosis was right and its conclusion was wrong:* **BLOCKED ON A CYRUP DESIGN DECISION, found 2026-10-09 while taking this row; effort re-rated M→L and the Fix below is incomplete as written.** The row says "wire `resolve_transcript` into the Anthropic adapter", but the adapter can never see a tool declaration: `crates/cyrup-agent/src/agent/run/stream.rs:65` calls `declare::without_tool_declarations(llm)` UNCONDITIONALLY on the request path, which clears `tools_added`/`tools_removed` on every system message, and `declare.rs:117-126` states the intent outright — cyrup carries the request's tools in `Context::tools` instead, because "a provider that replayed them would declare every tool twice, and a hidden one once". `the_request_projection_strips_declarations_only` pins it. Upstream's gate is `supportsMidConvoSystemMessages && supportsMidConvoToolChanges && initialTools.length > 0` with `initialTools = initialSystemMessage?.toolsAdded ?? []` (`anthropic-messages.ts:1139-1141` @v1.1.0), so under cyrup's projection that third term is ALWAYS false: the whole branch would be dead code in production while unit tests that build a `Context` by hand passed happily. **CORRECTED same day — this is not a cyrup design question, it is `AGENT-039`.** The first annotation framed the next step as an open choice; pi determines it. `agent-loop.ts:326-330` @v1.1.0 states the invariant: *"`context.tools` is what the runtime can execute; the transcript's system messages declare what the model may call … so replay always yields exactly `context.tools`"*, and `_installHiddenDeclarationsProjection` (`coding-agent/src/core/agent-session.ts:1767-1785`) projects out ONLY hidden names, leaving every other declaration in the transcript the provider receives. cyrup's blanket strip is therefore a cyrup-original divergence, and its justification ("a provider that replayed them would declare every tool twice") holds only because cyrup's adapters ALSO send `ctx.tools` at request level; pi's derive the request-level list from the transcript (`getCurrentTools(context.messages)`, `anthropic-messages.ts:578`, `:1221`), so there is one source and no double declaration. `normalizeContext` synthesises the leading system message from `systemPrompt`/`tools` when the transcript has none, so `initialTools` is populated on a first turn too — and cyrup already HAS `normalize_context` (`utils/transcript.rs:93`) and `get_current_tools` (`:137`) unused on this path. Ordered requirements, all from upstream: (1) replace the blanket strip at `agent/run/stream.rs:65` with pi's hidden-only projection; (2) have the adapters take their tool list from the transcript rather than `ctx.tools`; (3) then this row's native `tool_addition`/`tool_removal` branch has a non-empty `initialTools` and is reachable. Blocked on `AGENT-039` (open, area 02), which is (1)+(2); not blocked on a decision. — *Original:* **The Anthropic adapter emits no native mid-conversation tool changes (`tool_addition` / `tool_removal`), and pi 1.0.1 moved them to inline definitions behind a new beta — this is the adapter half of `PROV-083` that code comments call `PROV-083b`** — pi `b271b0a52` (`api/anthropic-messages.ts:195`, `:1137-1139`, `:1208-1210`, `:1344-1352` @v1.0.1): beta `inline-tools-2026-09-15`; the request-level tool list is fixed (initial tools plus the `__pi_deferred_placeholder__` deferred tool, `:205`); later tools go in `tool_addition` blocks as `{type:"tool_definition", definition}`; `hasToolRedefinitions` is deprecated. cyrup has only the compat predicates (`api/anthropic_messages/compat.rs:83`, `:195`): `messages.rs:60` skips `Message::System`, `params.rs:115` sends `ctx.tools` on every request, and `rg tool_addition crates` finds no emitter. `PROV-083b` appears only in code comments, never as a ledger row until now. **FILED 2026-10-03**; body below. **`api::emits_native_tool_additions` FLIPPED with this closure**, as the note main added on 2026-10-09 required: `crates/cyrup-provider/src/api/mod.rs` now returns `true` for `anthropic-messages`, and its pin test `tests::no_adapter_emits_native_tool_additions_yet` was rewritten to assert exactly that rather than "no adapter yet". That predicate is what lets pi-subagents' `toolActivation: "auto"` choose the `subagents_enable` loader (`cyrup-ext-subagents/src/extension/tool_activation.rs`, `SUBA-153`), so WITHOUT the flip this adapter work would have left `auto` on the eager `subagent` tool — the row would have read as closed while its only consumer saw no change. Caught by rebasing onto `main`, which had recorded the coupling while this work was in flight. |
| ~~PROV-135~~ | ~~low~~ **CLOSED 2026-10-08** | upstream-drift | S | **Filed 2026-10-08 and closed on arrival** (while closing `PROV-120`). **The Anthropic browser login rejected a pasted `code#` (or `?code=C&state=`) with `Missing OAuth state`; upstream exchanges it with `state: ""`.** pi `4df157433` (v0.99.0) dropped `if (!state) throw new Error("Missing OAuth state")` (was `anthropic.ts:304`); `loginAnthropic` now has `state = parsed.state ?? verifier` and `if (!code)` as its only post-input check (`anthropic.ts:189`, `:192` @v1.1.0). cyrup's `run_login` kept the check (`anthropic.rs:739-741` in the `PROV-120` first pass), so the two Anthropic flows disagreed after `PROV-120`. **CLOSED 2026-10-08:** the check is gone; `run_login` (`crates/cyrup-provider/src/auth/oauth/anthropic.rs:785`, `:790-795`) keeps `""` through `??` and checks the code only. Verify: `login_exchanges_a_paste_with_an_empty_state_like_upstream` (both spellings exchange `code: "C"`, `state: ""`; it replaces `login_rejects_paste_with_empty_state_as_missing_state`, which pinned the drift). Body below. |
| ~~PROV-136~~ | ~~low~~ **CLOSED 2026-10-08** | upstream-drift | S | **Filed 2026-10-08 and closed on arrival** (while closing `PROV-120`). **When port 53692 cannot be bound, the Anthropic browser login drops straight to paste-only; pi v1.1.0 first falls back to an OS-chosen free port.** pi `8d8ae2fc2` (#10571): `startCallbackServer(CALLBACK_PORT).catch(() => startCallbackServer(0)).catch(() => undefined)` and `redirectUri = callback?.redirectUri ?? REDIRECT_URI` (`anthropic.ts:154-157` @v1.1.0), used by the authorize URL (`:164`), the paste placeholder (`:179`) and the exchange (`:194`). cyrup tried the configured port once (`.ok()`, `anthropic.rs:630` in the `PROV-120` first pass). **CLOSED 2026-10-08:** `run_login` tries the configured port, then `0`, then none (`crates/cyrup-provider/src/auth/oauth/anthropic.rs:676-679`), and uses the bound URI everywhere, falling back to `Self::redirect_uri` (`REDIRECT_URI` in production) only with no listener (`:681-684`). Verify: `login_falls_back_to_a_free_port_when_the_preferred_port_cannot_bind`, `login_degrades_to_manual_paste_when_no_callback_port_can_bind`, `login_completes_via_browser_redirect`, `login_accepts_a_pasted_redirect_url_with_the_matching_state`, `redirect_uri_matches_the_bound_listener`. Body below. |
| ~~PROV-137~~ | ~~low~~ **CLOSED 2026-10-08** | upstream-drift | S | **Filed 2026-10-08 and closed on arrival** (review of the `PROV-120` pass). **When the user denies consent in the browser, the Anthropic browser login answered with an error page and kept waiting on the paste prompt; pi fails the login at once with `Anthropic authorization failed: <description>`.** pi `4df157433` (v0.99.0) moved `loginAnthropic` onto the shared `startOAuthCallbackServer` (`anthropic.ts:142-151` @v1.1.0, `state: verifier`), whose handler (`callback-server.ts:78-116`) checks the state first (`:85-88`, 400 `State mismatch.`, so a missing state too), then settles a truthy `error` with `finish({ error })` and a 400 `Anthropic authorization failed.` page carrying `error_description ?? error` (`:93-99`), then answers a missing code with 400 `Missing authorization code.` (`:100-104`), and on success shows `Signed in to Anthropic. You may now close this page.` (`:108`; pi `test/anthropic-oauth.test.ts:220`). `AnthropicCallbackHandler` was still the v0.83.0 hand-rolled handler (`anthropic.ts:114-148` @v0.83.0): an `?error=` was a non-settling `Continue` ("Anthropic authentication did not complete."), checked before the state, and the pages had the old wording; `bad_redirects_are_answered_without_ending_the_login` pinned that. **CLOSED 2026-10-08:** `AnthropicCallbackHandler::handle` (`crates/cyrup-provider/src/auth/oauth/anthropic.rs:383-445`) ports `:81-109` in upstream order — a non-GET 404 (the shared server owns the pathname half), state, `error` as `CallbackOutcome::Failed` with `OAuthError::Failed("Anthropic authorization failed: {description}")`, code, then the 200 page — all `no_store()` like `openrouter.rs`. The 502 branch (`:110-114`) is unreachable because `complete` is the identity. The redirect losing to an `Err` already ends `run_login` (the `settled?` arm propagates and the paste prompt is aborted on the way out, as a rejected `await callback?.wait()` throws out of `waitForCallbackOrManualInput` at `callback-server.ts:174` and its `finally` aborts the prompt; since `PROV-138` that abort is a drop guard covering every exit). The module header and provenance row now cite `callback-server.ts:78-116` @v1.1.0, and the header records the two remaining shared-server wording differences (the 404 and 409 pages), which no browser following a redirect can reach and which every flow on that server shares. Verify: `a_denied_redirect_fails_the_login` (`?error=access_denied&state=<verifier>` and the `error_description` variant: the login errs with `Anthropic authorization failed: access_denied` / `…: User denied access`, the page is 400 with the description, the token endpoint is never hit), `bad_redirects_are_answered_without_ending_the_login` (stateless denial, missing state and wrong state each get `State mismatch.`; `?error=&state=<verifier>` gets `Missing authorization code.`; a later good redirect completes), `login_completes_via_browser_redirect` (the `Signed in to Anthropic.` page). The redirect-driven tests now run under a 10s deadline (`within`, and a read timeout in `http_get`): with the free-port fallback mutated out, `login_falls_back_to_a_free_port_when_the_preferred_port_cannot_bind` fails in 10s where it used to hang. Body below. |
| ~~PROV-138~~ | ~~low~~ **CLOSED 2026-10-08** | parity-bug | S | **Filed 2026-10-08 and closed on arrival** (second review of the `PROV-120` pass). **When the paste won the Anthropic browser login's race, or the paste prompt failed, the prompt's own abort signal was never fired; pi aborts it on every exit.** Upstream threads `manualAbort.signal` into the `manual_code` prompt and calls `manualAbort.abort()` in a `finally` that runs whichever side settles the wait (`anthropic.ts:232`, `:261`, `:299-300` @v0.83.0; `waitForCallbackOrManualInput`, `callback-server.ts:160-182` @v1.1.0, the abort at `:180-182`), so a UI can dismiss a prompt it still shows; pi `test/anthropic-oauth.test.ts:177-211` @v1.1.0 ("resolves through the manual_code prompt and aborts it after settling") asserts `manualSignal.aborted` after a paste-won login. cyrup's `run_login` cancelled the token only in the `Winner::Redirect` arm (`anthropic.rs:648` at `d3789b1`, `:675` in the `PROV-120` first pass), so the paste-won path, the prompt-rejected path and the second-chance path all returned with the prompt's token live; the port was like this from the first v0.83.0 port. **CLOSED 2026-10-08:** the token's `drop_guard()` (`crates/cyrup-provider/src/auth/oauth/anthropic.rs:705`) is upstream's `finally`: every `?` return drops it, and the success path drops it once the wait has settled and before the exchange (`:788`), where `waitForCallbackOrManualInput` returns. The `Winner::Redirect` arm no longer cancels by hand (`:729`). The module header's *Cancellation* divergence note says so. Verify (`auth/oauth/anthropic.rs` tests): `login_resolves_through_the_manual_prompt_and_aborts_it_after_settling` (the port of pi's test: the paste wins, the login completes, the `manual_code` prompt's token is cancelled; red before the fix) and `a_failed_manual_prompt_is_aborted_too` (a rejected paste fails the login and its token is cancelled). The redirect-won path is still covered by `login_completes_via_browser_redirect`, whose blocking paste prompt only returns when its token fires. Body below. |
| ~~PROV-139~~ | ~~medium~~ **CLOSED 2026-10-10** | upstream-drift | S | **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5): `haiku-5` is now in all four needle lists in `api/bedrock_converse_stream/capabilities.rs`, in upstream's order (after `sonnet-5`): `supports_adaptive_thinking` (`bedrock-converse-stream.ts:766-779`), `supports_native_xhigh_effort` (`:781-792`), `supports_thinking_block_binding` (`:794-809`) and the Claude 5 arm of `supports_prompt_caching` (`:876-900`, needle `:889`). Every upstream citation in the module was refreshed to @f1b2e77f5. The ids are the six `*.anthropic.claude-haiku-5-5` rows pi.dev's `amazon-bedrock` catalog serves (bare, `global.`, `us.`, `eu.`, `jp.`, `au.`), and `haiku-5` matches each through the id alone. Verify, clause by clause, all in one test, `bedrock_converse_stream::tests::params::haiku_55_on_bedrock_caches_thinks_adaptively_and_binds_blocks_while_haiku_45_keeps_budget` (the six ids plus a nameless `global.` id, built without a `thinkingLevelMap` so the catalog map cannot supply `xhigh`): `supports_prompt_caching` is true and `system[1]` is `{cachePoint:{type:"default"}}`; `thinking == {type:"adaptive", display:"summarized", block_binding:{prefix_mismatch_behavior:"drop_block"}}` with `anthropic_beta == [THINKING_BINDING_CONTROLS_BETA]`; `map_thinking_level_to_effort(Xhigh) == "xhigh"` and `output_config.effort == "xhigh"` on the wire; and `anthropic.claude-haiku-4-5-20251001-v1:0` (bare and `global.`) keeps `type:"enabled"` with a budget, no `block_binding`, no `output_config`, the interleaved beta alone, `xhigh` clamped to `high`, and still caches through `-4-`. "Each is red before the needle is added" is met per needle: with the fix in place minus one needle the test fails at that needle's own assertion (adaptive: `left: {type:"enabled",budget_tokens:16384,...}`; binding: `left: {type:"adaptive",display:"summarized"}`; xhigh: `left: "high" right: "xhigh"`; caching: `anthropic.claude-haiku-5-5 must support prompt caching`), and with `haiku-5` widened to `haiku` the Haiku 4.5 half fails (`left: "xhigh" right: "high"`). Not run against live Bedrock, so the body's "budget thinking may be rejected outright" stays unverified. — *Original:* **Bedrock does not recognise Claude Haiku 5.5: no prompt caching, budget thinking instead of adaptive, and no native `xhigh` or block binding** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| ~~PROV-140~~ | ~~low~~ **CLOSED 2026-10-10** | upstream-drift | S | **Bedrock Converse sends no reasoning effort to OpenAI GPT models, so the configured thinking level never reaches them** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5). `build_additional_model_request_fields` (`crates/cyrup-provider/src/api/bedrock_converse_stream/params.rs:137-143`) now hands every non-Claude reasoning model to `build_openai_reasoning_fields` (`:217-237`), a port of pi `:1319-1330`: it tests `gpt-oss` before `gpt-` over the same `model_match_candidates` (made `pub(super)` in `capabilities.rs:13`, a one-word visibility change and the only edit there), sends gpt-oss a flat `reasoning_effort` from `OPENAI_GPT_OSS_EFFORT` (`:253-259`) without consulting the map, and sends other `gpt-` models a nested `reasoning.effort` where a string `thinkingLevelMap` entry wins over `OPENAI_GPT_EFFORT` (`:241-249`) through the shared `compat::mapped_effort_or` (absent and `null` both fall back, as `typeof mapped === "string"` does). Verify, in `bedrock_converse_stream/tests/params.rs`, all driven through `build_params` with the embedded catalog's rows: gpt-oss at `xhigh` sends `reasoning_effort:"high"` — `gpt_oss_sends_a_flat_reasoning_effort_clamped_to_high` (`openai.gpt-oss-120b-1:0`, also minimal→low, medium, max→high); GPT-6 at `minimal` sends `reasoning:{effort:"low"}` — `gpt_models_send_a_nested_reasoning_effort` (all six levels on `global.openai.gpt-6-sol`, `us.openai.gpt-6-luna`, `global.openai.gpt-5.6-sol`) plus `a_gpt_model_named_only_by_model_name_still_gets_the_effort` (pi's ARN case); a mapped level wins for `gpt-` and gpt-oss ignores the map — `a_mapped_level_wins_for_gpt_but_gpt_oss_ignores_the_map`; Claude unchanged — `claude_is_unchanged_while_gpt_gets_its_effort` (adaptive Opus 4.8 and budget-based Sonnet 4.5 at `minimal` carry no OpenAI field, and Nova still sends nothing); reasoning off sends no field — `reasoning_off_sends_no_openai_reasoning_field`. All six fail at HEAD; mutants that drop the map, let gpt-oss read it, swap the branch order or drop the reasoning-off gate each turn a named test red. Two notes: the map-wins and gpt-oss-ignores-map cases are not in pi's test file (the Verify line asks for them, so they are ported from the source); and the Claude half of `claude_is_unchanged_while_gpt_gets_its_effort` is a regression guard that cannot be red at HEAD by construction — the test bites through its GPT assertion. Every clause is met. Body below. |
| ~~PROV-141~~ | ~~low~~ **CLOSED 2026-10-10** | upstream-drift | S | **Mistral `finish_reason: "error"` is not retried, because its message lacks the `server error` marker upstream added** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. — **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5): `map_chat_stop_reason(Some("error"))` now returns `"Provider stopped with: error (server error)"` with upstream's comment, and the doc comment cites `mistral-conversations.ts:936-952` @f1b2e77f5. Verify, clause by clause: the new message and its retryability are asserted on the error terminal the decoder emits for a `finishReason: "error"` chunk, fed to `utils::retry::is_retryable_assistant_error` (`prov141_a_mistral_error_finish_reason_is_retryable_and_an_unknown_one_is_not`); `"unmapped_error"` stays non-retryable (same test); the table test's pin is updated (`an_unrecognized_finish_reason_is_an_error_not_a_clean_stop`). Red before: both fail with `left: Some("Provider stopped with: error")`. Caveat: the decoder this test drives reads the camelCase `finishReason`, so against Mistral's real snake_case stream the fixed message is not reached until `PROV-152` lands. **Resolved 2026-10-10 by `PROV-152`'s closure:** the decoder reads `finish_reason`, this test's fixtures are snake_case, and `wire_keys.rs::prov152_a_snake_case_error_finish_reason_reaches_the_retryable_message` drives pi's own fixture to the retryable message. |
| ~~PROV-142~~ | ~~low~~ **CLOSED 2026-10-10** | upstream-drift | S | **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5): `utils/estimate.rs` now divides by 3.5 as pi's `CHARS_PER_TOKEN = 3.5` (`estimate.ts:15`, `27075fe07`) does, held as the exact fraction 7/2 so `ceil_div` is `(chars * 2).div_ceil(7)` with no float rounding; the text, image, assistant, system and tools paths all go through it. cyrup-session's compaction estimator (`compaction/tokens.rs`) stays at `/ 4`, matching pi `coding-agent/src/core/compaction/compaction.ts:311-344` @f1b2e77f5. Verify, clause by clause: (1) pi's `test/context-estimate.test.ts` expectations are ported literally in `tests/context_estimate_chars_per_token.rs`, each asserting the `max_tokens` that `build_base_options` actually hands the request: `reserves_three_and_a_half_chars_per_token_when_limiting_output` (3000/2000/1000, `max_tokens` 2904), `stale_usage_falls_back_to_the_three_and_a_half_estimate` (1149, `max_tokens` 4755) and `fresh_usage_counts_the_trailing_text_at_three_and_a_half` (2002/2); (2) "20 chars give 6" is the flipped `"20 chars / 3.5"` assertion in `system_message_charges_prompt_text_and_both_tool_lists`, and `text_tokens_ceil_divide_by_three_and_a_half` replaces the old `/ 4` unit test; (3) "one image gives 1372" is `image_block_counts_estimated_chars`. Neither (2) nor (3) is a literal in pi's test file, which has no 20-char or image case; both are derived from `estimate.ts:15-16`. (4) The `compaction_tokens_after` tests were run, not assumed: both pass (`manual_compaction_reports_tokens_after_over_pi_s_raw_context` sums the provider estimate on its flattened side, which only grows, so its `pi_tokens < flattened_tokens` guard still holds), as do the four `estimator_prefix_timestamp_parity` tests that call `cyrup_provider::estimate_context_tokens`. Red with the old `/ 4` restored: all six tests failed (`max_tokens` `Some(3029)` vs `Some(2904)`, `Some(4899)` vs `Some(4755)`, trailing 1 vs 2, image 1200 vs 1372, `"abcd"` 1 vs 2, `20 chars / 3.5` 5 vs 6). The public `estimate_*` re-exports (`lib.rs:191-194`) change value for SDK users, as pi's do. Original finding: ~~**The request-context estimate still uses 4 characters per token; pi 1.1.0 uses 3.5, so cyrup's output-token clamp leaves less headroom and long prompts can overflow** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below.~~ |
| ~~PROV-143~~ | ~~low~~ **CLOSED 2026-10-10** | upstream-drift | S | **Codex Responses still pins `originator` and `User-Agent` after the caller's headers, so `model.headers` / `options.headers` cannot override them as pi 1.1.0 allows** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. — **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5): `build_sse_headers` seeds `originator` / `User-Agent` before all three overlay loops (`model.headers`, `auth.auth.headers`, `options.headers`) and sets only `Authorization` / `chatgpt-account-id` last; doc comment and citations rewritten to `:1640-1681` @f1b2e77f5. Verify, clause by clause: a model `originator` and a caller `user-agent` win (`prov143_model_and_caller_headers_override_originator_and_user_agent`, red before with `left: [Some("pi")]`); a caller `Authorization` / `chatgpt-account-id` is still replaced (same test, plus `sse_headers_match_upstream_and_auth_cannot_be_overridden`; passes at HEAD by design, and bites against a mutant that seeds the credential first: `left: [Some("Bearer ignored")]`). Extra: the credential overlay overrides and a `None` overlay deletes the default (`prov143_credential_overlay_overrides_and_a_none_overlay_deletes_the_default`, red before). |
| ~~PROV-144~~ | ~~low~~ **CLOSED 2026-10-10** | not-ported | S | **`LoginOptions.agentName` is unported, so an embedding app cannot name itself in the Sign in with ChatGPT or Codex browser login** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. — **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5): `auth::LoginOptions` gains `agent_name: Option<String>` (+ `with_agent_name`), pi `agentName?` (`auth/types.ts:209-213`); `openai_chatgpt.rs` `login` sends `options.agent_name` else `AGENT_NAME_HINT` (pi `openai-chatgpt.ts:253`, `options?.agentName ?? AGENT_NAME_HINT`); `openai_codex.rs` `login` threads the options into `login_browser` and `create_authorization_flow(originator: Option<&str>)` (pi `openai-codex.ts:289-306`, `:359-363`, `:430`). Verify: `prov144_uses_the_apps_agent_name_as_the_name_hint` and `prov144_uses_the_apps_agent_name_as_the_browser_login_originator` (ports of `9ad083102`'s two tests, driven through `login`; `my-app` sent, `Pi`/`pi` without it). Each red when its flow ignores the option. |
| PROV-145 | low | upstream-drift | M | **The Azure provider is still `azure-openai-responses` and serves only the Responses API; pi 1.0.3 renamed it to `azure` and added Foundry Chat Completions deployments (DeepSeek V4 Pro). Decision-gated: the rename needs an owner decision first** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| ~~PROV-146~~ | ~~low~~ **CLOSED 2026-10-10** | not-ported | M | **CLOSED 2026-10-10** (on `claude/zealous-bell-x0u1h0` off `main` @ `62502ff8`, checked against pi f1b2e77f5). **Landed in #215 (`52b1aa33`)** with area 05's `CFG-104`. This closure re-verified it, added one test and closed it together with `PROV-123`, as the Fix asks. **Model:** `Model.sampling_params` is `Option<ModelSamplingParams>` (`crates/cyrup-provider/src/model.rs:323`), holding the flat map and a `SamplingParamsByThinkingLevel` struct (`:161`, `:240`). It (de)serializes as pi's two sibling keys `samplingParams` / `samplingParamsByThinkingLevel` (pi `types.ts:852`, `:854`). **Divergence from the Fix:** it is a struct of `off`…`max` fields, not the suggested `BTreeMap<ModelThinkingLevel, Map>`, because `ModelThinkingLevel` is not `Ord`. The JSON is the same either way; the Rust shape is tagged `[CYRUP-DELTA]` at `model.rs:233`. **Resolve:** `resolve_sampling_params` (`utils/simple_options.rs:81`) is pi's `resolveSamplingParams` (`api/simple-options.ts:24-34`): flat, then the entry for `clamp_thinking_level(model, level)` (`collection.rs:1330`), then the request. It returns `None` when all three are absent (pi `:31`). It is called from `build_base_options` (`:130`, with the reasoning or `off`) and from the three adapters through `apply_sampling_params` (`api/openai_completions/params.rs:286`), as pi does at `openai-completions.ts:1004`, `openai-responses.ts:383` and `azure-openai-responses.ts:243`. The responses adapter uses pi's summary-only `medium` fallback (`api/openai_responses/params.rs:306-313`, pi `openai-responses.ts:363`). **Verify, per clause** (pi `test/sampling-options.test.ts:135-215` @f1b2e77f5; all in `crates/cyrup-provider/src/tests/sampling_params.rs`, all asserting on the outgoing request body): (1) *a level entry overrides the model default and is overridden by the request*: `cfg104_applies_the_effective_levels_params_over_model_defaults` (`:347`), `cfg104_request_keys_win_over_thinking_level_keys` (`:400`), and `cfg104_each_openai_compatible_adapter_applies_level_params_without_build_base_options` (`:425`) on all three adapters. Red: dropping the level layer fails all three; swapping the level and request layers fails `cfg104_request_keys_win_*`. (2) *an unsupported level clamps to the model's nearest*: `cfg104_applies_the_effective_levels_params_over_model_defaults` (pi `:135-152`, `low` with `low`/`medium` unmapped picks the `high` entry). Red: resolving at the requested level without clamping fails it. (3) *reasoning off selects the `off` entry*: `cfg104_applies_the_off_entry_when_reasoning_is_disabled` (`:384`). Red: dropping the level layer fails it. (4) *no params at all yields `None`*: `agent026_omits_sampling_params_when_neither_side_sets_them` (`:158`), `agent026_the_merge_yields_none_when_neither_side_sets_anything` (`:292`), and the new `prov146_resolve_yields_none_when_no_layer_applies` (`:659`; `:593` before `PROV-150`'s test was inserted above it). The new test covers a model that HAS a level map but no entry for its clamped level, and asserts both `None` and a body with no `temperature`/`top_p`. Red: it fails when the clamp is removed, and it is the only failing test when the `None` guard is changed to check for any level map. All four mutations were re-run by this closure (`cargo test -p cyrup-provider --lib sampling_params`, 17 tests) and reverted. **Not portable:** the Azure half of pi's summary-only case (`:198-215`), because cyrup's `AzureOpenAiResponsesOptions` has no `reasoning_summary`; the Azure builder resolves at the unified level (`azure_openai_responses.rs:433-438`). This is an unported option, not a deliberate divergence, filed as `PROV-150`: pi's Azure adapter has `reasoningSummary` (`azure-openai-responses.ts:29`) and the summary-only `medium` fallback (`:224`, `:232`) at f1b2e77f5, as it already did at v0.83.0 (`:58`, `:308`). `PROV-045` (CLOSED 2026-08-14) fixed the summary-only trigger on the openai-responses side, but cyrup's Azure options were never given the field, so the trigger cannot be reached there (the adapter's own comment at `azure_openai_responses.rs:411-413` says so). `PROV-150`'s Verify ports pi test `:198-215` for azure. **Update 2026-10-10:** `PROV-150` is closed on the same branch, and that case is now `prov150_summary_only_azure_request_uses_medium_effort_and_the_medium_entry` (`sampling_params.rs:495`). *Original:* ~~**`Model.samplingParamsByThinkingLevel` is unported, so per-thinking-level sampling overrides are ignored at request time (the request half; lands with `PROV-123`, and `CFG-104` is the models.json half)** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below.~~ |
| PROV-147 | low | not-ported | M | **The `openai-decisions` classifier API (OpenAI's Decisions API, `openai/gpt-6-luna` as a classifier) is unported** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| ~~PROV-148~~ | ~~low~~ **CLOSED 2026-10-10** | not-ported | S | **`ClassifierContext.images` is unported, so a classify request cannot carry images and no model-level image check exists** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. — **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5): `ClassifierContext` gains `images: Option<Vec<Content>>` (serde default, omitted when `None`, between `state` and `questions` as in `types.ts:682-690`); `classifier::assert_classifier_input_supported` ports `assertClassifierInputSupported` (`utils/model-operations.ts:46-53`) and `Models::classify` calls it first, before the provider lookup, as `models.ts:978-980` does; `api/llama_cpp_classify.rs` `run` ports the `:437` guard. Verify: `prov148_models_classify_rejects_images_on_a_text_only_classifier_model` (`Model p/c does not accept image input`, api not reached), `prov148_rejects_image_input` (`llama.cpp classification does not support image input`, no server request), `prov148_images_reach_an_image_capable_model_and_empty_or_absent_images_pass` (unchanged without images), plus ordering and wire tests. Each red under a targeted mutation. |
| ~~PROV-149~~ | ~~low~~ **CLOSED 2026-10-10** | stale-port | S | **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5): the nine catalogs this row measured are regenerated with the repo's own tool, `cargo run -p xtask -- gen-catalogs --only anthropic,amazon-bedrock,google,opencode,opencode-go,openrouter,vercel-ai-gateway,minimax,github-copilot` (pi.dev, fetchedAt 2026-10-09T12:41Z; `azure-openai-responses` left out per `PROV-145`; the other 29 live catalogs untouched). No price was hand-edited. Rows / tiered rows now equal the body's pi.dev column exactly: anthropic 17/1, amazon-bedrock 193/29, google 22/6, opencode 83/16, opencode-go 31/7, openrouter 403/81, vercel-ai-gateway 252/49, minimax 3/1, github-copilot 35/13. Verify, clause by clause: (1) `gen-catalogs --check --only <the nine>` reproduces all ten files compared (nine live plus the pinned `openrouter-images`), run twice, after the write and after the test pass. (2) The cost clause is asserted on the price the request uses: the shipped `anthropic_provider()` row is fed through the Anthropic SSE decoder and the `Done` message's `usage.cost` is checked, in `anthropic_messages::tests::usage::prov149_a_150k_input_haiku_55_turn_is_billed_at_the_long_context_tier` (150k input at $0.5/M, output at $2.5/M; MIRROR 90k at $0.1/M) and `prov149_sonnet_55_cache_reads_are_billed_at_ten_cents_per_million` (500k cache reads cost $0.05). `providers::amazon_bedrock::tests::haiku_55_profiles_ship_with_their_long_context_tier` does the same for the six Bedrock Haiku 5.5 rows through `compute_cost`, the function Bedrock's `blocks.rs` prices with (`global.` and bare at 0.5/2.5 above 100k, the four regional profiles at 0.55/2.75). (3) `tests::catalog_data::sonnet_4_5_resolves_at_a_200k_window_and_100_images_per_request` resolves `claude-sonnet-4-5` and `-20250929` through model selection with `context_window == 200000` and `input_limits.images.max_per_request == Some(100)`; it replaces `sonnet_4_5_offers_the_full_1m_context_window`, which pinned the stale value. Red-proved on the HEAD catalogs: the Haiku tests fail with "the embedded anthropic catalog has no claude-haiku-5-5" (Bedrock: "no global.anthropic.claude-haiku-5-5"), Sonnet 5.5 with "priced at 0.1, expected 0.05", Sonnet 4.5 with `left: 1000000 right: 200000`; the image clause alone, by putting 600 back on the regenerated rows, fails `left: Some(600) right: Some(100)`. **The tier threshold counts all input, as Anthropic's pricing page says:** `usage.rs` `select_rates` keys on `input + cacheRead + cacheWrite`, as pi `calculateCost` does (`models.ts:1200-1209` @f1b2e77f5); `prov149_cache_reads_count_toward_the_haiku_55_tier_threshold` (20k fresh + 90k cache read bills every component at the tier) goes red when the key is reduced to `input` alone. **The whole-catalog rewrites also moved data this row did not name**, and it is upstream's, not a choice here: 36 rows in and 10 retired across the seven stems other than `anthropic` and `amazon-bedrock`, 244 field values (openrouter 116 and vercel 52 cost changes, including base-price moves such as `deepseek/deepseek-v4-flash`; openrouter context/maxTokens churn; `opencode-go` `qwen3.7-plus`/`qwen3.8-max` moved from Completions to Messages; vercel `alibaba/qwen-3-235b` now non-reasoning), and on Bedrock Sonnet 5.5 `us.`/`eu.` and `zai.glm-5.3` `global.`/`us.`. The count and split pins in `cyrup-provider` that the new rows moved were updated to the new data, each with its provenance. Not touched: `usage.rs`'s doc still cites `models.ts:640-648`. — *Original:* **The embedded catalogs predate pi 1.1.0: no Claude Haiku 5.5 rows, Sonnet 5.5 cache reads priced at 0.2 instead of 0.1, Sonnet 4.5 embedded at a 1M context window, and no prompt-length tiers for Google, OpenCode, OpenCode Go, OpenRouter, Vercel or MiniMax** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| ~~PROV-150~~ | ~~low~~ **CLOSED 2026-10-10** | not-ported | S | **CLOSED 2026-10-10** (on `claude/zealous-bell-x0u1h0` off `main` @ `62502ff8`, checked against pi f1b2e77f5; `git -C tmp/pi log f1b2e77f5..42a3497d0 -- packages/ai/src/api/azure-openai-responses.ts packages/ai/test/sampling-options.test.ts` is empty). **upstream, re-read:** `AzureOpenAIResponsesOptions.reasoningSummary?: "auto" \| "detailed" \| "concise" \| null` (`api/azure-openai-responses.ts:29`); `buildParams` sets `reasoningEffort = options?.reasoningEffort ?? (options?.reasoningSummary ? "medium" : undefined)` (`:224`), opens the reasoning arm on it with the unmapped `"medium"` when no effort was given (`:227-229`), sends `summary: options?.reasoningSummary \|\| "auto"` and `include: ["reasoning.encrypted_content"]` (`:230-234`), and resolves sampling at `reasoningEffort ?? "off"` (`:243`). `streamSimple` (`:142-153`) never sets `reasoningSummary`, on Azure as on openai-responses, so the option travels only through the typed options. **Change:** `AzureOpenAiResponsesOptions` gains `reasoning_summary: Option<ReasoningSummary>` (`crates/cyrup-provider/src/api/azure_openai_responses.rs:78`), reusing the openai-responses type, whose `as_wire` became `pub(crate)` (`api/openai_responses/options.rs:19`). `build_params` mirrors `api/openai_responses/params.rs:256-313`: the truthy summary (`None` and `Some(Null)` are falsy, `azure_openai_responses.rs:420-423`) opens the reasoning arm with effort `"medium"` (`:431-434`), the chosen summary or `"auto"` is sent (`:439`), and sampling resolves at `Medium` for a summary-only request (`:458-463`). The plumbing is `StreamOptions::api_options` → `ApiStreamOptions::AzureOpenAiResponses` (`stream.rs:332`, accessor `:373`), the same path the openai-responses options take; no other caller builds these options. The module doc that called the field unreachable was rewritten. **Verify** (all asserting on the outgoing body, `crates/cyrup-provider/src/tests/sampling_params.rs`): `prov150_summary_only_azure_request_uses_medium_effort_and_the_medium_entry` (`:495`) ports pi `test/sampling-options.test.ts:198-215` for azure (summary `Auto`, reasoning off, distinct `off`/`medium` entries: `reasoning.effort == "medium"`, `reasoning.summary == "auto"`, `include == ["reasoning.encrypted_content"]`, `temperature` = the `medium` entry's 0.8); with no summary and with `Some(Null)` it sends the `off` entry (0.7), `reasoning == {"effort":"none"}` with no summary and no `include`; a typed `Detailed` reaches `reasoning.summary` on the summary-only path and `Concise` does with effort `high`. **Red** (each re-run with `cargo test -p cyrup-provider --lib -- prov150 sampling_params`, 18 tests, and reverted): (1) the reasoning arm keyed on effort only fails it (`effort` `"none"` vs `"medium"`, `:519`); (2) sampling resolved at the unified level fails it (`temperature` 0.7 vs 0.8, `:522`); (3) the summary hard-coded to `"auto"` fails it (`"auto"` vs `"detailed"`, `:539`). In each case it was the only failure. The openai-responses half (`cfg104_summary_only_responses_request_uses_the_medium_entry`, `:462`) is unchanged. *Original:* ~~**`AzureOpenAiResponsesOptions` has no `reasoning_summary`, so pi's Azure summary-only request (effort `medium`, sampling resolved at `medium`) cannot be made** **Filed 2026-10-10 from the `PROV-146` closure**; body below.~~ |
| ~~PROV-151~~ | ~~low~~ **CLOSED 2026-10-10** | stale-port | S | **CLOSED 2026-10-10** (on `claude/zealous-bell-x0u1h0` off `main` @ `62502ff8`, checked against pi f1b2e77f5; `git -C tmp/pi log f1b2e77f5..42a3497d0 -- packages/ai/scripts/generate-models.ts` is empty). **openai, by the generator:** `cargo run -p xtask -- gen-catalogs --diff --only openai` reported `+ gpt-6.1-sol` and one more change, `~ gpt-daybreak-blue-latest cost` (pi.dev now serves it a 272k tier: 8/30/0.8/10 above 272000). `git -C tmp/pi log -S daybreak f1b2e77f5 -- packages/ai` is empty, so that tier is models.dev data, not `12c416e1a`; the run was written and the daybreak hunk reverted by hand, which leaves it with `PROV-149` (recorded there). The manifest's `openai` entry moved to the fetch the row came from (`fetchedAt` `2026-10-09T12:42:07Z`, `revision` `sha256-3ff185d6…`, `catalog_manifest.json:139-140`). `gen-catalogs --check --only openai` therefore does not reproduce pi.dev: `--diff` lists exactly one difference, the daybreak tier, and no missing row. **Azure, by hand:** `--diff --only azure-openai-responses` skips the stem (`curl … azure-openai-responses failed: … 404`, `PROV-145`), so the row (`azure-openai-responses.json:1228`) was added by hand from the `openai` row, as the Fix allows. The manifest entry was left as it was, since no fetch happened; the `azure` rename stays with `PROV-145`. **Each field re-derived from pi `scripts/generate-models.ts` @f1b2e77f5:** `missingOpenAiModels` (`:2872-2883`, appended when absent `:2974-2978`): `openai-responses`, `https://api.openai.com/v1`, text+image, reasoning, `contextWindow` `OPENAI_LONG_CONTEXT_INPUT_THRESHOLD` = 272000 (`:391`), `maxTokens` 128000; cost `withOpenAiLongContextPricing(OPENAI_STANDARD_COSTS["gpt-6.1-sol"])` = `{2, 10, 0.1, 2.5}` (`:452`) plus one tier above 272000 at 4/15/0.2/5 (`:427-439`). `applyThinkingLevelMetadata` (`:1045-1064`): `off: null`, `minimal: null`, `low`…`max` identity; `OPENAI_RESPONSES_NONE_REASONING_MODELS` (`:455-468`) does not list it, so `:1068-1074` does not reset `off`. Compat on `openai`: `supportsStrictMode` (`:864-873`), `supportsOpenAIGrammarTools` (`:893-898`), `supportsToolSearch` + `supportsAdditionalTools` (`:367-379`, `:900-912`), `supportsMidConvoSystemMessages` (`:949-965`), `supportsExplicitPromptCacheMode` (`:970-977`); `inputLimits` 512 MiB / 1500 images plus resize (`:994-1016`). Azure (`:3308-3323`): the four scalar rates and no `tiers`, `baseUrl` `""`, `contextWindow` 272000 (no `AZURE_CONTEXT_WINDOW_OVERRIDES` entry, `:3301-3307`), compat only `supportsOpenAIGrammarTools` (`azure` is in `OPENAI_GRAMMAR_TOOL_PROVIDERS`, `:879-886`; every other compat pass is scoped to `openai`/Codex), resize-only `inputLimits`; `provider` stays cyrup's `azure-openai-responses`. Order: after `gpt-6-sol` in both, where pi.dev serves it on `openai`. **Verify** (`crates/cyrup-provider/src/`): `providers/openai.rs::gpt_6_1_sol_matches_the_upstream_rules` (`:251`) and `providers/azure_openai_responses.rs::the_gpt_6_1_sol_clone_matches_the_upstream_rules` (`:181`) resolve the rows from the embedded catalog with `context_window == 272000`, `max_tokens == 128000`, `thinking_level_map.off == null`, and the costs above; only the `openai` row has `supportsToolSearch` (the Azure test asserts `None`, and `tool_search_is_off_for_every_openai_responses_model_but_the_seven`, `api/openai_responses/tests/tools.rs:539`, keeps the flag on `openai` only). Pins updated with values re-derived from pi, not copied: openai count 43 → 44 (`openai.rs:111`), the long-context list (`:142`), Azure count 43 → 44 in two tests (`azure_openai_responses.rs:95`, `:233`), the exact tool-search lists (`api/anthropic_messages/tests/catalog.rs:193`, `tools.rs:539`, `ENABLED` 10 → 11). **Red:** with both catalogs restored to `HEAD`, 8 tests fail (`cargo test -p cyrup-provider --lib -- gpt_6_1_sol tool_search catalog_parses provider_identity long_context`, 38 run): the two new row tests, both openai/Azure count tests, Azure `provider_identity`, `long_context_models_carry_the_272k_pricing_tier` and both tool-search lists. With `off` set to `"none"` in both rows (and `supportsToolSearch` on the Azure row), the two row tests fail on `off` and only they do. *Original:* ~~**`openai.json` and `azure-openai-responses.json` lack pi's `gpt-6.1-sol` row (`12c416e1a`); only `openai-codex` was regenerated, by `CFG-102`** **Filed 2026-10-10 from the `PROV-123`/`PROV-146` closure**; body below.~~ |
| ~~PROV-152~~ | ~~high~~ **CLOSED 2026-10-10** | upstream-drift | M | **cyrup's Mistral transport puts the old SDK's camelCase keys on the wire: it sends `maxTokens`/`toolCalls`/`promptMode` and reads `finishReason`/`toolCalls`/`promptTokens`, where pi's native transport (since v1.0.1) sends and reads Mistral's snake_case; a finish reason, streamed tool calls and prompt usage are likely never seen** **Filed 2026-10-10 while reviewing `PROV-141`** (on `claude/provider-conformance-pi11`); body below. — **CLOSED 2026-10-10** (checked against pi f1b2e77f5 and Mistral's published OpenAPI): new `api/mistral_conversations/wire.rs` ports `toMistralWirePayload`/`toMistralWireMessage`/`toMistralWireContentChunk` (`mistral-conversations.ts:386-451`) and `mod.rs` applies it after the `before_provider_request` hook, as pi does after `onPayload` (`:148-151`, `:318`); `content.rs` reads `finish_reason`/`tool_calls` (`:633`, `:710`), `finish.rs` reads `prompt_tokens`/`completion_tokens`/`total_tokens` (`:617-625`); `messages.rs` adds pi's `prefix: false` / tool-call `index: 0` (`:864`, `:868`). Evidence upgraded the row: Mistral's OpenAPI (`docs.mistral.ai/openapi.yaml`) marks `ChatCompletionRequest`, `AssistantMessage`, `ToolMessage`, `ImageURLChunk` `additionalProperties: false` with a documented 422, so the request half was a rejected turn per spec (not merely unhonoured), and `UsageInfo`/`CompletionResponseStreamChoice` require the snake_case keys. Verify: `tests/wire_keys.rs::prov152_*` (four tests; request bytes captured off loopback after the hook carry `max_tokens`/`prompt_mode`/`reasoning_effort`/`tool_choice`/`prompt_cache_key`/`parallel_tool_calls`/… and assistant `tool_calls`, tool `tool_call_id`, chunk `image_url`, never camelCase; a snake_case `finish_reason: "error"` stream reaches `PROV-141`'s retryable message; a `tool_calls` delta yields a tool call; `usage.prompt_tokens` etc. counted). All four red at HEAD. No live smoke run: no Mistral key in this environment. |
| PROV-153 | low | not-ported | S | **The `typesafe` provider (pi providers/typesafe.ts) is not ported, so TypeSafe's own System One endpoint has no provider entry** **Filed 2026-10-10 by the `PROV-104` closure** (`claude/compassionate-cray-46f1tv`). Narrowed out of PROV-104 (closed 2026-10-10). pi f1b2e77f5 registers `typesafeProvider()` (providers/typesafe.ts, listed in providers/all.ts:171, KnownProvider `typesafe` in types.ts:97). It authenticates with an env API key, TYPESAFE_API_KEY. Its models are only the classifier rows of TYPESAFE_CLASSIFIER_MODELS (pi.dev /api/models/providers/typesafe?types=chat,image,classifier serves one row on 2026-10-10: jev-latest, api typesafe-system-one, baseUrl https://api.typesafe.ai/v1/, contextWindow 64000). It registers classifiers {"typesafe-system-one": typesafeSystemOneApi()}. cyrup has the api (crate::api::typesafe_system_one, KnownClassifierApi::TypesafeSystemOne) and the provider-side classifier leg (WireProvider::with_classifier_models / with_classifiers). What it lacks: a classifier-only provider (fleet entries assume a chat catalog parsed as Vec<Model>), an embedded catalog path for classifier rows (xtask gen-catalogs LIVE_CATALOGS plus a loader into Vec<ClassifierModel>), the env-key mapping, and registration in providers/all.rs. Impact: a user with a TypeSafe key cannot classify against api.typesafe.ai directly; OpenRouter's TypeSafe rows do work. Fix: add live("typesafe", ...) to xtask LIVE_CATALOGS and generate catalog/typesafe.json through gen-catalogs only; load it as ClassifierModel rows; build a WireProvider with no chat models, those classifier rows and a {typesafe-system-one} registry; map TYPESAFE_API_KEY; register it. Verify: the built-in typesafe provider lists jev-latest as AnyModel::Classifier, supports_classification is true, and Models::classify through it reaches a loopback fake's /v1/systemone with Bearer <TYPESAFE_API_KEY>. |

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
> reproduced today at any tag") and `PARITY-GAPS.md:956` (`OQ-5`) both rest on the observation that
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

## ~~PROV-042~~ — `ModelsStreamTransforms.transformHeaders` unported — `before_provider_headers` has no seam

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed ·
**CLOSED 2026-09-05** at `bb355412` (provider) + `b9837e6c` (agent/ext/session-svc)

> **Closure note (2026-09-05).** Two things were wrong with the state this item was left in, and
> only one of them was the one the 2026-08-15 re-measurement named.
>
> 1. **The 2026-08-14 seam was on a path no request takes.** `Models::apply_auth` reproduces pi's
>    literal position — `git -C tmp/pi show v0.84.4:packages/ai/src/models.ts` `:657`
>    (`if (options?.transformHeaders) headers = await options.transformHeaders(headers ?? {})`),
>    stripped at `:660` — and that is right *for pi*, where every request goes through
>    `Models.stream`/`streamSimple` (`:667-679`, `:688-694`). cyrup's agent loop does not: it streams
>    `StreamFn` → `Provider::stream` → `WireProvider::stream` (`crates/cyrup-provider/src/wire.rs:149`)
>    → `ApiImpl::run`, and `rg '\.stream_simple\(|Models::stream' crates/` finds no production
>    caller of the collection at all. So the field could be set and nothing would read it, and an
>    emitter landed on top of it would have been inert too.
>    `bb355412` adds `crate::stream::apply_transform_headers` — the sibling of the already-proven
>    `apply_on_payload` (pi `emitBeforeProviderRequest`) — and calls it from **all ten** registered
>    api impls immediately after `build_headers`. That position reproduces pi's *effect* on cyrup's
>    topology, and it is the only one at which this item's own **Verify** clause is reachable:
>    `x-api-key` is installed by the api impl, not by `applyAuth`, so "removing `x-api-key` inside it
>    actually suppresses it" is unachievable at pi's literal position. Bedrock is the single
>    documented exception — SigV4 signs the header set, so the hook runs at pi's own pre-signing
>    caller-header injection point (`bedrock-converse-stream.ts:224-227` @v0.84.4) rather than after
>    `authorize`. The `Models` seam keeps its own application and its strip, so a `Models`-routed
>    request still transforms exactly once and the two positions never stack.
> 2. **The emitter, as re-measured.** `b9837e6c` installs the producer in `SessionBuilder::build`
>    beside the two sibling provider seams already there, which is exactly pi's position
>    (`packages/coding-agent/src/core/sdk.ts:330-339` @v0.84.4 →
>    `ExtensionRunner.emitBeforeProviderHeaders`, `core/extensions/runner.ts:1100-1125`, gated on
>    `hasHandlers`). The path from the session to the provider field is new:
>    `AgentBuilder::transform_headers` → `GenerationConfig::transform_headers` →
>    `StreamOptions::transform_headers`. `ExtensionHost::emit_before_provider_headers` is modelled on
>    `emit_before_provider_request`; `ProviderHeaders` is `Record<string, string | null>` upstream and
>    `BTreeMap<String, Option<String>>` here, so the round-trip is total and a `null` survives as the
>    `None` that suppresses the header at `stream/sse.rs:359`.
>
> **The ordering half is REFUTED rather than carried.** The re-measurement said pi's guarantee "has
> no single point where both are present" and that closing it meant moving attribution onto
> `transform_headers`. It does not. Attribution rides `StreamOptions::headers` (AGENT-029,
> `session/model.rs:282` via `Agent::set_header_fn`), and the provider merges that overlay into the
> assembled set **before** running `transform_headers` — attribution first, hook last, hook's return
> value on the wire. Same two facts in pi's order, reached by a different route, so `set_header_fn`
> stays where AGENT-029 put it.
>
> **Verification.** `crates/cyrup-provider/src/tests/transform_headers_on_the_wire.rs` drives the
> real `ApiImpl::run` for each of the ten apis against a loopback origin that records the request
> head, asserting all three Verify clauses per impl (the closure is handed the assembled auth header;
> the header it adds reaches the wire; the header it deletes does not).
> `every_registered_api_impl_has_a_case`, over the new `ApiRegistry::ids`, fails if a new api is
> registered without a case, so "every api impl, not just one" is a property rather than a snapshot.
> `crates/cyrup-session-svc/src/tests/before_provider_headers_emitter.rs` builds a real session,
> runs a real turn, and proves a subscribed native extension's add + `null`-delete reach the header
> bag while an untouched header passes through — and that with no subscriber the transform is still
> installed and is an exact identity. Red before: with `apply_transform_headers` reduced to a
> pass-through the provider suite fails on the first impl; with the `builder.rs` installation removed
> both session-svc tests fail on "the StreamOptions the agent dispatched carried no
> `transform_headers`".
>
> **RESIDUAL:** none in this area.

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

**Impact** — The highest-severity item on this surface, and **not a flag — the protocol.** cyrup builds a Chat-Completions body and POSTs it to the Completions path for xAI's flagship model, where pi builds a Responses body (`input[]` not `messages[]`, a `reasoning` object, a different SSE event grammar). Every `grok-4.5` request diverges wholesale. Compounding: `thinkingLevelMap {off:null, minimal:null}` is absent so `off`/`minimal` are not suppressed, and `supportsLongCacheRetention:false` is absent so the resolver defaults it **true** (`detect_compat` gives `true` for xai) and will offer long cache retention xAI rejects. **This is on the default path** — `CFG-045` (`05-cyrup-config-and-resources.md:604`) makes `grok-4.5` the xai default model. No existing id covers it: `grep -rn 'grok-4.5' docs/gap-analysis/` hits only the model-resolver item, never the api mismatch.

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
2. **`PROV-004`'s and `PARITY-GAPS.md:956` (`OQ-5`)'s "not statically auditable" verdict is FALSE.** Both record, as settled, that "every `*.models.ts` at v0.84.1 is a two-line re-export, so no pricing, context-window, maxTokens or compat-flag claim about the 35 embedded catalogs can be checked by reading this workspace", and `PROV-004` is downgraded to "a verification task, not a fix task" on that basis. The re-export form begins at `a9f6a3159`; at its parent `b0c2a90e` — **precisely the revision cyrup's own manifest names as its provenance floor** — the files are full data literals. The entire catalog is checkable with `git show b0c2a90e:packages/ai/src/providers/<p>.models.ts` plus a ~12-line node script: no generator run, no `npm install`, no network. Nine sweeps read the backlog and inherited the "unverifiable" verdict; the data was one `git show` away.

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
owed to `PARITY-GAPS.md:956` (`OQ-5`), which still records it — **left open, outside this area's
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

**Impact** — A benign structural original. Filed for one reason only: so the **35-vs-35 catalog count is not mistaken for a clean one-to-one mapping.** The true mapping is 34 shared names, plus `together` (inline Rust in cyrup, `together.models.ts` in pi — **21/20 as of 2026-08-15: pi's 20 rows exact, plus the signed-off `moonshotai/Kimi-K3` addition, `PROV-070`**) *(2026-09-28: K3 turned out to be pi's own served row, so all 21 rows are pi's: 20 at `b0c2a90e` plus K3 from `pi.dev`; `PROV-070` closed.)*, plus `openrouter-images`, against pi's 37 `*.models.ts` files. A future sweep that compares directory counts and stops will conclude the catalog set is complete; it is not (`PROV-014` names three providers with no catalog at all).

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


## PROV-071 — The embedded catalog floor is frozen for every provider, not just the four that are empty

**Kind** tooling · **Severity** medium · **Effort** L · **Confidence** confirmed · **Filed** 2026-09-13

**upstream** — `a9f6a3159` (`feat(ai): separate generated model data (#6765)`, 2026-07-17) added
`packages/ai/src/providers/data/` to `.gitignore` and rewrote every `*.models.ts` into an 8-line
re-export of that now-gitignored, models.dev-fetched JSON. At HEAD `71dca871b` (2026-09-11) **all 39
provider modules are that shape** — verified by reading every one, not sampled. `b0c2a90e` is
`a9f6a3159`'s direct parent and the only revision at which any of them is a data literal.

**cyrup** — `xtask gen-catalogs` recovers catalogs with `git show` against `b0c2a90e`
(`xtask/src/main.rs`, `PROV-018`/`PROV-060`). That mechanism is therefore **permanently incapable of
producing anything newer, for any provider** — not only for the four with no rows at all
(`DRIFT-009`). The 31 "present" catalogs are frozen at 2026-07-17 exactly as hard as the four empty
ones; they simply fail silently, as stale data rather than as missing data.

**Impact** — Three things, and the third is why this is filed rather than left as a note.

1. **Scope was mis-stated across passes.** The catalog-floor discussion has been framed as
   "`DRIFT-009`: four catalogs short". The real shape is "35 catalogs frozen, four of them at zero
   rows". `PROV-054`…`PROV-059` were the visible symptoms for one provider; the same latent drift
   applies to the other 30 and is unmeasured.
2. **A stale catalog does not degrade to 'missing'.** As `01-cyrup-core-and-provider.md:561` already
   records, cyrup's resolvers invent a default wherever the catalog is silent, so a stale row
   degrades to *confidently wrong* — wrong price, wrong context window, wrong compat flag, no
   symptom.
3. **The fix now has a demonstrated route, and it is cheap.** `XAI_1` fetches
   `https://pi.dev/api/models/providers/xai` at generation time. pi serves that endpoint **already
   shaped into cyrup's native `Model` JSON** — exclusions applied, api unified — so no
   models.dev-shape transform and no port of `generate-models.ts` (3000+ lines, heavy per-provider
   hardcoding) is required. The remaining 34 are mechanically similar *once the plumbing exists*,
   which it now does (`xtask/src/live_catalog.rs`, `LIVE_CATALOGS`). **NOT assumed complete without
   doing it** — each provider needs its endpoint verified and its assertions rewritten (see the Fix).

**Amends `DRIFT-009`, and the amendment is load-bearing.** That row says seeding from the pi.dev
artifact is *"an explicit owner decision … which this item's own rewrite forbids in bold"*. The
prohibition was correct in its own scope: it was written against a proposal to seed the **blocked
four** from pi.dev *instead of* from git, before `PROV-060` showed `git show` at `b0c2a90e` was
available. It does not reach a provider for which the pinned path is provably dead — `xai.models.ts`
has been a re-export since `a9f6a3159`, so no revision yields newer rows and there is no `git show`
to prefer. **The decision recorded here is narrow: pi.dev is a permitted source when, and only when,
the pinned-revision path cannot reach the data for that provider.** Seeding from pi.dev to avoid the
work of reading git remains forbidden.

**Reachability is environment-specific and both measurements are real.** `DRIFT-009` and
`00-residual-ledger.md:195` record `curl https://models.dev/api.json` → `CONNECT tunnel failed,
response 403` (re-measured 2026-09-05) and build an escalation on it. From this workspace on
**2026-09-13**: `models.dev/api.json` → **200**, `pi.dev/api/models/providers/xai` → **200**. Neither
is authoritative; the block belongs to the sandbox's egress policy, not to the hosts. **Re-measure,
date the measurement, and do not inherit either value.**

**Fix** — Generalize `LIVE_CATALOGS` to the remaining 34, and do it as ONE piece of work with the
assertion change, not two:
(1) verify `https://pi.dev/api/models/providers/<id>` for each provider (a 404 there means that
    provider stays on the pinned path and must be said so, not silently skipped);
(2) **rewrite the count/roster assertions in the same change.** `providers/fleet.rs`'s
    `EXPECTED_COUNTS` pins an exact model count per provider. That is a valid fact about a commit
    while a catalog is git-pinned, and a scheduled failure the moment it is live-fetched — re-running
    `gen-catalogs` turns it red with no code change. `XAI_2` does this for xai (removes the count,
    asserts instead what pi's generator hardcodes: protocol, provider tag, `baseUrl`, the exclusion
    set, non-empty) and is the worked example to copy. Generalizing the fetch without this ships 34
    scheduled failures;
(3) keep the global `generatedAt`/`source` as the floor for whatever remains pinned — moving it to
    follow a live fetch discards every other provider's valid persisted overlay (pi #7016).

**Verify** — `gen-catalogs --roster <rev>` accounts for every upstream module across all three
buckets and exits 0; `catalog_manifest.json` names a per-provider source for each live catalog with
its own `fetchedAt`/`revision`.

**NOT closed by XAI_1..XAI_4.** Those four tasks fix xai's data, its stale assertions, and the
runtime refresh path. The other 34 catalogs are untouched and unscheduled. This row is the record
that they are stale, not merely un-audited.

## Findings filed 2026-09-24 (re-measure at cyrup `ea23ca2`, pi `v0.87.1`)

Every item here was read on both sides at the pins named: cyrup at `ea23ca2` (this area's crates are
byte-identical to `9aeba769`), and upstream only as `git -C tmp/pi show <tag>:<path>`. The tag where
each upstream change landed was settled by `git tag --contains` on the `git log -S` commit, not by date.
Static analysis: nothing was built or run, and every **Verify** line is a design. Severities rest on
the Impact prose and on upstream's own bug report; none is an observation of cyrup.

## PROV-073 — Mistral streaming tool calls are keyed `{callId}:{index}`; an id-less continuation chunk splits the call

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24 · **CLOSED 2026-09-27**: upstream `const key = toolCall.index ?? callId;` (`packages/ai/src/api/mistral-conversations.ts:695` @v0.87.1, read at the tag) is ported at `crates/cyrup-provider/src/api/mistral_conversations/blocks.rs:45-48` as `match index_opt { Some(i) => format!("index:{i}"), None => call_id.clone() }` — the composite `{call_id}:{index}` key is gone, and the `index:` prefix keeps the two key spaces disjoint so a provider id that looks like a bare integer cannot collide (a mechanism note, not a behavioural difference: pi's JS `Map` keys on a number, which a Rust `HashMap<String, _>` cannot express). Verify: `api::mistral_conversations::tests::decode::prov073_an_id_less_continuation_chunk_appends_to_the_indexed_block` — one tool call `abcdefghi` with `{"a":1}` from an id-bearing first chunk plus an id-less indexed continuation.

**cyrup** — `crates/cyrup-provider/src/api/mistral_conversations/blocks.rs::process_tool_call` derives
`call_id` from the chunk's `id`, or, when absent/empty/`"null"`, from
`derive_mistral_tool_call_id(&format!("toolcall:{index}"), 0)`, then keys the open-block map on
`format!("{call_id}:{index}")`. A first chunk carrying `id: "abc", index: 0` opens `abc:0`; a
continuation carrying only `index: 0` computes the DERIVED id and opens a second block.

**upstream** — `packages/ai/src/api/mistral-conversations.ts:691-695` @v0.87.1: `const key =
toolCall.index ?? callId;` — the index alone keys the block whenever it is present. At v0.84.1 the key
was `` `${callId}:${toolCall.index || 0}` `` (`:438`), exactly cyrup's shape; the change landed in
**v0.84.4** (#8387).

**Impact** — a Mistral stream that sends the id once and then streams argument fragments by index
produces two tool-call blocks: one with the name and a prefix of the JSON, one with a synthetic id and
the rest. The tool runs with truncated or unparseable arguments, or twice.

**Fix** — key `tool_blocks_by_key` on `index` when present, falling back to `call_id` only when the
chunk carries no index; keep the first-seen id on the block.

**Verify** — a decode test feeding `{id:"abc",index:0,function:{name,arguments:"{\"a\""}}` then
`{index:0,function:{arguments:":1}"}}` yields ONE tool call `abc` with `{"a":1}`.

## PROV-074 — DeepSeek requests send `max_completion_tokens`; detection is case-sensitive

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24 · **CLOSED 2026-09-27**: upstream `isDeepSeek = provider === "deepseek" || baseUrl.toLowerCase().includes("deepseek.com")` (`packages/ai/src/api/openai-completions.ts:1603` @v0.87.1) feeding both `isNonStandard` (`:1612`) and `useMaxTokens` (`:1623`) — all three read at the tag. Ported at `crates/cyrup-provider/src/api/compat.rs:640` (the lower-cased URL test, hoisted above `is_non_standard`), `:649` (the `is_non_standard` arm) and `:673` (the `use_max_tokens` arm), so `max_tokens_field` resolves to `max_tokens`. Verify: `api::compat::tests::prov074_deepseek_uses_max_tokens_through_every_detection_route`, which drives all three routes (`provider:"deepseek"`, a mixed-case `https://API.DEEPSEEK.COM/v1` base URL, and a DeepSeek-provider model on a proxy URL).

**cyrup** — `crates/cyrup-provider/src/api/compat.rs::detect_compat`: `use_max_tokens` is
`chutes.ai || is_moonshot || is_cloudflare_ai_gateway || is_together || is_nvidia || is_ant_ling ||
is_zai` — no DeepSeek arm — and `is_deepseek` is `provider == "deepseek" ||
base_url.contains("deepseek.com")` on the raw URL. `providers/catalog/deepseek.json`'s two rows carry no
`maxTokensField` override, so the detected `max_completion_tokens` is what ships.

**upstream** — `packages/ai/src/api/openai-completions.ts:1603` @v0.87.1 lowercases the base URL
(`baseUrl.toLowerCase().includes("deepseek.com")`), `:1612` puts `isDeepSeek` in `isNonStandard`, and
`:1621-1623` puts it in `useMaxTokens`. `git show v0.84.1:…` has no `isDeepSeek` in `useMaxTokens`;
`v0.84.2` does ("send max_tokens to DeepSeek APIs", #7933).

**Impact** — the completion cap never reaches DeepSeek in the field it reads, the same effectively
uncapped completion `DRIFT-013` fixed for Z.AI; a custom `models.json` entry pointing at
`https://API.DeepSeek.com` also loses the DeepSeek thinking format and reasoning-content replay.

**Fix** — add `|| is_deepseek` to `use_max_tokens`, hoist `is_deepseek` above `is_non_standard`, and
test the lowered base URL. Schedule with `DRIFT-013`'s expression (same code block).

**Verify** — `detect_compat` for `provider:"deepseek"` and for a custom model with base URL
`https://API.DEEPSEEK.COM/v1` both yield `max_tokens_field == "max_tokens"`.

## PROV-075 — Google / Vertex rewrite any finish reason to `toolUse` when a tool call is present, and clear the error

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24 · **CLOSED 2026-09-27**: upstream `if (output.content.some((b) => b.type === "toolCall") && output.stopReason === "stop")` (`packages/ai/src/api/google-generative-ai.ts:226` @v0.87.1, read at the tag; `api/google-vertex.ts:234` is byte-identical and cyrup's Vertex reuses the same `decode_stream`). Ported at `crates/cyrup-provider/src/api/google_generative_ai/parts.rs:76-81`: the override now fires only on `matches!(stop, StopReason::Stop)`, and the unconditional `dec.error_message = None` is deleted, so a `MAX_TOKENS` turn stays `Length` and a `SAFETY` turn stays `Error` with its message intact. `raw_stop_reason` is still recorded before the override (`:59`), as pi does. Verify: `api::google_generative_ai::tests::decode::a_non_stop_finish_reason_survives_a_tool_call` (MAX_TOKENS, SAFETY and the still-rewritten STOP case) and `api::google_generative_ai::tests::stop_reason::a_non_stop_finish_reason_names_itself_in_the_error`.

**cyrup** — `crates/cyrup-provider/src/api/google_generative_ai/parts.rs::process_chunk`: after
`map_stop_reason(reason)`, `if dec.blocks.iter().any(|b| matches!(b, Content::ToolCall(_)))` sets
`StopReason::ToolUse` and `dec.error_message = None` unconditionally. `api/google_vertex.rs` imports the
same `decode_stream`, so Vertex inherits it.

**upstream** — `packages/ai/src/api/google-generative-ai.ts:225-227` @v0.87.1 and
`api/google-vertex.ts:234-235`: `if (output.content.some((b) => b.type === "toolCall") &&
output.stopReason === "stop") output.stopReason = "toolUse";`. The `&& … === "stop"` guard landed in
**v0.84.2** (#8059/#8135); at v0.84.1 (`:217-219`) the override was unconditional, as in cyrup. pi never
cleared `errorMessage` at any tag — that half is cyrup-original.

**Impact** — a `MAX_TOKENS` turn whose tool call was cut mid-arguments, or a `SAFETY`/`RECITATION`
stop, is reported as a clean `toolUse`: the agent executes a truncated call and the diagnostic is
erased, where pi surfaces `length`/`error`.

**Fix** — gate the override on the mapped reason being `Stop` and stop clearing `error_message`.

**Verify** — a decode test with a tool-call part and `finishReason:"MAX_TOKENS"` yields
`StopReason::Length`; with `"SAFETY"` yields `Error` with its message intact; with `"STOP"` still
yields `ToolUse`.

## PROV-076 — Kimi's top-level `usage.cached_tokens` is not counted as cache reads

> **CLOSED 2026-09-28.** (on `claude/lows-next`) `parse_usage` reads `usage.cached_tokens` as the third cache-read fallback, after `prompt_tokens_details.cached_tokens` and `prompt_cache_hit_tokens` (`api/openai_completions/finalize.rs:52-57`). Test: `kimi_top_level_cached_tokens_count_as_cache_reads`. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — `crates/cyrup-provider/src/api/openai_completions/finalize.rs::parse_usage`: `cache_read` is
`prompt_tokens_details.cached_tokens`, else `prompt_cache_hit_tokens`, else 0.

**upstream** — `packages/ai/src/api/openai-completions.ts:1524` @v0.87.1: `…?.cached_tokens ??
rawUsage.prompt_cache_hit_tokens ?? rawUsage.cached_tokens ?? 0`, with the comment at `:1527-1530`
("Kimi documents top-level usage.cached_tokens on the final usage chunk"). Landed v0.84.3 (#8075).

**Impact** — Kimi cache hits are billed and displayed as uncached input; `/session` cost and the
cache-stats notices are wrong for that provider.

**Fix** — add `.or_else(|| raw.get("cached_tokens").and_then(Value::as_u64))` as the third fallback.

**Verify** — `parse_usage` on `{"prompt_tokens":100,"cached_tokens":60}` yields `cache_read == 60`.

## PROV-077 — Anthropic OAuth `user-agent` still claims `claude-cli/2.1.75`

> **CLOSED 2026-09-28.** (on `claude/lows-next`) `CLAUDE_CODE_VERSION` is `2.1.280` (`api/anthropic_messages/headers.rs:25`), and the OAuth header test asserts the exact `claude-cli/2.1.280`. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — `crates/cyrup-provider/src/api/anthropic_messages/headers.rs`: `const CLAUDE_CODE_VERSION:
&str = "2.1.75";`, sent as `claude-cli/{CLAUDE_CODE_VERSION}` on OAuth requests.

**upstream** — `packages/ai/src/api/anthropic-messages.ts:87` @v0.87.1 is `"2.1.280"`, used at `:952`.
It was `2.1.75` at v0.83.0 through v0.84.4, `2.1.251` at v0.85.0 through v0.87.0, and moved to
`2.1.280` at v0.87.1 ("Fixed Anthropic OAuth requests reporting an outdated Claude Code version").

**Impact** — every OAuth Anthropic request identifies as a client two upstream bumps out of date; the
upstream changelog treats that as a bug.

**Fix** — set the constant to `2.1.280` and cite the tag in the doc comment.

**Verify** — the existing OAuth header test asserts `claude-cli/2.1.280`.

## PROV-078 — Unknown OpenAI-compatible endpoints are detected as strict-capable, so every tool carries `strict`

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — `crates/cyrup-provider/src/api/compat.rs::detect_compat` sets `supports_strict_mode:
!is_moonshot && …` — i.e. `true` for any endpoint it does not recognise — and
`api/openai_completions/tools.rs::convert_tools` inserts `"strict"` on every function whenever
`compat.supports_strict_mode` holds.

**upstream** — `packages/ai/src/api/openai-completions.ts:1667` @v0.87.1: `detectCompat` returns
`supportsStrictMode: false` ("OpenAI compatibility alone does not imply strict JSON-schema tool
support"). The old expression moved into `scripts/generate-models.ts`'s
`detectOpenAICompletionsCompat` so BUILT-IN models keep strict as explicit catalog metadata, and the
generator's `OPENAI_COMPLETIONS_DEFAULT_COMPAT.supportsStrictMode` flipped to `false`. Landed
**v0.87.0** (`890f92088`, #9816). v0.86.1 had separately excluded Cerebras (#9804).

**Impact** — a `models.json` model on an unrecognised OpenAI-compatible server receives a `strict` key
(and strict-subset schemas where a tool requests strict sampling); upstream's report is HTTP 400 on
such servers. Predicted from that report, not observed here.

**Fix** — flip the detected default to `false` AND carry `supportsStrictMode: true` on the built-in
catalog rows that detection used to mark capable — the second half is generator work
(`PROV-018`/`PROV-071`), and flipping the default alone would drop strict from built-in OpenAI rows.

**Verify** — `detect_compat` for an unknown base URL yields `supports_strict_mode == false`; the
built-in `openai` rows still emit `"strict"`.

## PROV-079 — Image-only user messages on openai-completions carry an empty text part

> **CLOSED 2026-09-28.** (on `claude/lows-next`) `user_content` drops empty text parts from a content array (`api/openai_completions/convert.rs:357`); an array left empty is skipped by the caller (`:55-58`). Test: `empty_text_parts_are_dropped_from_a_user_content_array`. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — `crates/cyrup-provider/src/api/openai_completions/convert.rs::user_content` pushes
`{"type":"text","text":…}` for every `Content::Text` block, empty or not, whenever the content is not
text-only.

**upstream** — `packages/ai/src/api/openai-completions.ts:1261` @v0.87.1: `.filter((item) => item.type
!== "text" || item.text.length > 0)` before mapping (`1b6ddca87`, v0.87.1, #9797).

**Impact** — an attachment sent with no prompt text produces `[{"type":"text","text":""},
{"type":"image_url",…}]`, which upstream reports some OpenAI-compatible providers reject.

**Fix** — skip empty `Content::Text` blocks in the array branch of `user_content`.

**Verify** — `user_content(&[Text(""), Image{..}], true)` returns a one-element array.

## PROV-080 — The `meta` provider is unported

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — no provider id `meta`: `git grep -n 'api\.meta\.ai\|META_API_KEY' -- crates/` is empty, and
`crates/cyrup-provider/src/providers/all.rs`'s built-in list and `env_api_keys.rs` have no arm for it.

**upstream** — `packages/ai/src/providers/meta.ts` @v0.87.1 (absent at v0.85.1): `openai-responses`
at `https://api.meta.ai/v1`, `envApiKeyAuth("Meta Model API key", ["META_API_KEY"])` and a lazy
subscription OAuth (`loginLabel: "Sign in with Meta"`). `auth/oauth/meta.ts` is an RFC 8628 device
flow against `auth.meta.com` whose identity token is exchanged for a day-lived Model API key at
`https://api.meta.ai/muse-code/key`, stored as `refresh`/`access` so the ordinary refresh scheduler
re-mints it. Registered at `providers/all.ts:108`, `env-api-keys.ts` gains `meta: "META_API_KEY"`,
`auth/oauth/load.ts:57-59` adds `loadMetaOAuth`. Landed v0.86.1 (#9096).

**Impact** — `/login meta` and `META_API_KEY` do nothing in cyrup; Muse Spark models are unavailable.

**Fix** — a fleet-shaped provider row plus an `auth/oauth/meta.rs` device-code flow on the existing
`poll_oauth_device_code_flow`; catalog rows must come from the generator (`PROV-071`), not by hand.

**Verify** — `all_providers()` contains `meta` with both auth strategies; a device-flow test drives
authorize → token → key-mint against a stub.

## PROV-081 — Bedrock never populates `cache_write_1h`; one-hour writes are priced at the five-minute rate

> **CLOSED 2026-09-28.** (on `claude/lows-next`) `handle_metadata` fills `cache_write_1h` from `cacheDetails[]` entries with `ttl: "1h"` (`api/bedrock_converse_stream/events.rs:395-407`). Test: `one_hour_cache_writes_are_read_from_cache_details`. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — `crates/cyrup-provider/src/api/bedrock_converse_stream/` requests one-hour cache points
(`convert.rs` inserts `"ttl": "1h"`) but no file under that directory mentions `cache_write_1h` or
`cacheDetails`; `crates/cyrup-provider/src/usage.rs` already prices `cache_write_1h` separately when
it is set.

**upstream** — `packages/ai/src/api/bedrock-converse-stream.ts:712-717` @v0.87.1 sums
`event.usage.cacheDetails` entries with `ttl === CacheTTL.ONE_HOUR` into `usage.cacheWrite1h` before
`calculateCost`; `models.ts:912` prices it. Landed v0.86.0 (#9457).

**Impact** — Bedrock sessions with long cache retention under-report cost.

**Fix** — parse `usage.cacheDetails[]` in the metadata event and set `cache_write_1h`.

**Verify** — a decode test with `cacheDetails:[{ttl:"1h",inputTokens:1000}]` yields
`cache_write_1h == Some(1000)` and the 1h-rate cost.

## PROV-082 — z.ai's `Prompt too long` is not an overflow; any provider's bodyless `400`/`413` is

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24 · **CLOSED 2026-09-27**: upstream `/prompt (?:is )?too long/i` in `OVERFLOW_PATTERNS` (`packages/ai/src/utils/overflow.ts:38` @v0.87.1) plus `CEREBRAS_BODYLESS_OVERFLOW_PATTERN` (`:64`) tested only under `message.provider === "cerebras"` (`:145`) — both read at the tag. Ported at `crates/cyrup-provider/src/utils/overflow.rs:16` (the widened pattern), `:46` (the pattern lifted out of `OVERFLOW_PATTERNS` into its own constant) and `:103-107` (the provider gate). A z.ai `Prompt too long` is now an overflow that auto-compaction can act on, and a bodyless `400`/`413` from any other provider is not. Verify: `utils::overflow::tests::zai_prompt_too_long_and_provider_gated_bodyless`.

**cyrup** — `crates/cyrup-provider/src/utils/overflow.rs`: `OVERFLOW_PATTERNS` holds
`r"prompt is too long"` and, unconditionally, `r"^4(?:00|13)\s*(?:status code)?\s*\(no body\)"`;
`is_context_overflow` tests the list with no provider check.

**upstream** — `packages/ai/src/utils/overflow.ts` @v0.87.1: `:38` is `/prompt (?:is )?too long/i`
("Anthropic and z.ai"); the bodyless pattern moved out of the list into
`CEREBRAS_BODYLESS_OVERFLOW_PATTERN` (`:64`), tested only `if (message.provider === "cerebras")`
(`:145`). Landed v0.86.0 (#9482) and v0.86.1 (#9805).

**Impact** — a z.ai overflow is not recognised, so no auto-compaction and the turn just fails; and a
bodyless 400/413 from any other provider is treated as overflow, triggering a lossy compaction and
retry for an error that has nothing to do with context size.

**Fix** — widen the Anthropic pattern and split the bodyless pattern out behind
`message.provider == "cerebras"`.

**Verify** — `is_context_overflow` is true for a z.ai error `{"code":"1261","message":"Prompt too
long"}`, true for `"400 (no body)"` from `cerebras`, false for the same text from `openai`.

## Findings filed 2026-09-24, second pass (the `packages/ai` windows the first pass left unread)

Read in full this pass, upstream only as `git -C tmp/pi show <tag>:<path>` / `git diff v0.85.1..v0.87.1`:
`api/anthropic-messages.ts`, `api/openai-responses.ts`, `api/openai-responses-shared.ts`,
`api/azure-openai-responses.ts`, `api/openai-codex-responses.ts`, `types.ts`, `utils/transcript.ts`
(whole file), `utils/{text,estimate,event-stream}.ts`, `api/{transform-messages,simple-options,pi-messages}.ts`,
`models.ts`, `compat.ts`, `index.ts`, the `google-*`/`mistral-conversations.ts` commits (`16235fd93`,
`96617628e`, `4bd3f48df`), the two `openai-completions.ts` commits the first pass skipped (`bbb61e34a`,
`af7359b90`), `scripts/generate-models.ts` (the whole 705-line diff), `image-models.generated.ts` and
`models.generated.ts`. cyrup at `ea23ca2` (this area's crates byte-identical to `9aeba769`). Landing tags
by `git tag --contains`. Static analysis: nothing was built or run; **Verify** lines are designs.

## PROV-083 — The transcript-carried system prompt and tool changes (v0.86.0) are unported; cyrup still emits the deleted v0.85 deferred-tool shapes

**Kind** upstream-drift (with a `stale-port` component) · **Severity** medium · **Effort** L · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — the v0.85 model, end to end: `cyrup-core` `ToolResultMessage.added_tool_names` +
`Context.tools`; `crates/cyrup-provider/src/utils/deferred_tools.rs` (the `splitDeferredTools` port);
Anthropic `api/anthropic_messages/compat.rs::default_supports_tool_references` and
`tools.rs` (`defer_loading`) with `tool_reference` blocks in tool results (`convert.rs`, pinned by
`tests/deferred_tools.rs`); openai-responses `convert.rs` anchoring `tool_search_call`/`tool_search_output`
at the tool result; openai-completions `DeferredToolsMode::Kimi` (`api/compat.rs`, `convert.rs:118`,
`params.rs:149`); `utils/estimate.rs` added-tool accounting (`PROV-S04`). No `role:"system"` message
variant exists (`git grep -n 'tools_added\|mid_convo_system' -- crates/` is empty).

**upstream** — `9e05370b2` "Mid conversation system messages (#9548)", **v0.86.0**. `types.ts` @v0.87.1:
`SystemMessage { content, sections?, toolsAdded?, toolsRemoved?, timestamp }` joins `Message`;
`ToolResultMessage.addedToolNames` is deleted; `ProviderStreams.stream(model, TranscriptContext, …)` takes a
normalized transcript whose leading system message carries prompt + tools (`utils/transcript.ts`
`normalizeContext`, `resolveTranscript`, `collapseSystemMessages`, `resolveTranscriptTools`).
`utils/deferred-tools.ts` is deleted. Per adapter: Anthropic drops `supportsToolReferences` and
`tool_reference` results and instead, when `supportsMidConvoSystemMessages && supportsMidConvoToolChanges`
(generator: first-party `claude-opus-4-8`/`opus-5`/`opus-5-5`/`fable|mythos-5.x`), declares later tools
`defer_loading` behind a `__pi_deferred_placeholder__` and surfaces them with `tool_addition`/`tool_removal`
blocks in a mid-conversation `system` message plus the `mid-conversation-tool-changes-2026-07-01` beta;
otherwise it sends the CURRENT tool list. Responses anchors `additional_tools` / the tool-search pair at
the system message, not the tool result (`openai-responses-shared.ts` `appendSystemToolAdditions`).
Completions sends Kimi's `{role:"system", tools}` item under `supportsMidConvoSystemMessages &&
supportsMidConvoToolAdditions` (the flag `deferredToolsMode` is gone; generator: Kimi K3 only).
`estimate.ts` counts system messages. `transform-messages.ts` holds a system message that lands between a
tool call and its result.

**Impact** — (1) **Stale wire shapes**: on first-party Claude ≥4.5, a tool added mid-run reaches the model
as a `tool_reference` inside a tool result, a mechanism pi no longer sends; on models pi now serves
natively (Opus 4.8+/5.x) cyrup cannot use `tool_addition`, and on everything else pi sends the full
current list while cyrup still splits. (2) **Cache**: pi's placeholder exists because "the first real late
tool does not invalidate the cache (measured: full miss without it)"; cyrup has no equivalent. (3) **No
mid-conversation instruction or tool-removal channel**: prompt changes rewrite the head, so the cached
prefix breaks on every change where pi appends. (4) Closed `PROV-025` (Kimi mode ported) and `PROV-S04`
(estimate accounting ported) now describe shapes upstream deleted; neither is reopened, both are subsumed here.

**Fix** — a design decision first, because it crosses cyrup-core (a `System` message variant and its
JSONL form), cyrup-agent (who emits tool-change system messages) and cyrup-session (area 03, persisted).
Then per adapter: port `utils/transcript.ts`; replace `deferred_tools.rs` and `added_tool_names`
consumers with `resolve_transcript_tools`; add `supports_mid_convo_system_messages`,
`supports_mid_convo_tool_changes` (Anthropic) and `supports_mid_convo_tool_additions` (completions) to
`ModelCompat` and retire `deferred_tools_mode`. Catalog flags arrive by regeneration (`PROV-071`).

**Verify** — the upstream fixtures `test/transcript-tool-changes.test.ts` and
`test/system-message-replay.test.ts` @v0.87.1 translated to cyrup: a late tool on `claude-opus-4-8`
yields `tool_addition` + the placeholder + the beta; on `claude-haiku-4-5` it yields the full current
tool list and no `tool_reference`.

## PROV-084 — The SSE framer drops a final event not followed by a blank line; an Anthropic `message_stop` cut at EOF fails the turn

**Kind** parity-bug (Anthropic half, pre-v0.83.0) + upstream-drift (Codex half, v0.85.0) · **Severity** medium · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24 · **CLOSED 2026-09-27**: upstream's two hand-rolled reader loops, read at the tag: `readMistralEvents` is `body.getReader()`, a `\n\n` boundary drain, `if (done) break;`, then `if (buffer.trim()) { const event = parseMistralEvent(buffer); if (event !== MISTRAL_STREAM_DONE && event) yield event; }` (`packages/ai/src/api/mistral-conversations.ts:468`, `:471-474` @v0.87.1); `readPiMessagesEvents` is the same shape with `parsePiMessagesEvent` (`packages/ai/src/api/pi-messages.ts:297-299`, `:302-307`). Neither is an SDK helper and both flush the residual after the `done` break. cyrup: `flush_at_eof: true` at `crates/cyrup-provider/src/api/mistral_conversations/mod.rs:146` and `crates/cyrup-provider/src/api/pi_messages.rs:203`, with the two invented-upstream comments replaced by citations of those real loops. The pass's substantive finding is that the SHIPPED replay tests could not pin those gates: they read through `stream/sse.rs::decode_sse_bytes_flushing_at_eof`, whose flag is a test-harness constant (`api/mistral_conversations/tests/mod.rs:74`, `api/pi_messages.rs:1242`), so with both gates reverted to `false` the six other `prov084` tests stayed green. The two new tests drive `run()` over a loopback `tokio::net::TcpListener::bind("127.0.0.1:0")` with one canned `text/event-stream` body written verbatim so it can end without a terminating blank line, under an empty `ProviderEnv` overlay so a developer's shell cannot steer proxy resolution; reverting both gates fails exactly those two on the production terminals (`Mistral stream ended without a finish reason`, `radius stream ended without a terminal event`), and reverting the Mistral half alone fails only the Mistral test, so each half is independently load-bearing. The now-false doc on `decode_sse_bytes_flushing_at_eof` (it still said "the two adapters … Anthropic Messages and Codex Responses" after four adapters began flushing) is corrected at `stream/sse.rs:540-547` and now names the two tests that do the pinning. The pre-existing `CYRUP-DELTA` at `stream/framer.rs:122` is unchanged and remains sound: `SseFramer::flush` gates on non-empty DATA where pi's `flushSseEvent` (`anthropic-messages.ts:340-353` @v0.87.1) also fires on an `event:`-only residual — checked on the consumer side, such a residual reaches `iterateAnthropicEvents`, where a name in `ANTHROPIC_MESSAGE_EVENTS` hits `parseJsonWithRepair("")` and throws and a name outside it is `continue`d, so pi either errors or does nothing while cyrup drops the frame and errors with its own message or does nothing — a mechanism difference at parity, with no payload to lose. Verify: `api::mistral_conversations::tests::decode::prov084_the_live_run_path_flushes_a_reply_cut_after_the_finish_reason_chunk`, `api::pi_messages::tests::prov084_the_live_run_path_flushes_a_reply_cut_after_done`.

**cyrup** — `crates/cyrup-provider/src/stream/framer.rs::frame_bytes`: at EOF the unfold returns `None`
("An unterminated trailing line is dropped") without dispatching the pending event buffer. Every SSE
adapter reads through it (`wire.rs` `open_sse` for Anthropic and the rest; `openai_codex_responses/driver.rs`).
Anthropic's driver then reports `Anthropic stream ended before message_stop`
(`api/anthropic_messages/driver.rs`).

**upstream** — `api/anthropic-messages.ts` @v0.83.0 and @v0.87.1: `iterateSseMessages` decodes a residual
unterminated line and then `flushSseEvent(state)` (`:340`, called at `:461` @v0.87.1) yields the pending
event. `api/openai-codex-responses.ts` @v0.87.1 `:794` "Treat EOF as terminating the residual SSE frame",
with `if (done) break` moved after draining (`:822`) — v0.85.0 (#9047). The OpenAI SDK paths drop it, as
cyrup does.

**Impact** — a relay or proxy that closes the connection right after the last `data:` line (the #9047
report) turns a complete Anthropic or Codex response into an error and a lost turn.

**Fix** — flush the framer's pending data at EOF, gated to the two adapters upstream flushes for (or
unconditionally, with a note on the OpenAI paths).

**Verify** — `decode_sse_bytes("event: message_stop\ndata: {\"type\":\"message_stop\"}\n")` yields one
frame; an Anthropic fixture ending that way finishes with `stop`.

## PROV-085 — Codex with thinking `off` omits `reasoning`, so the server's default effort applies

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24 · **CLOSED 2026-09-27**: upstream `else if (model.reasoning && model.thinkingLevelMap?.off !== null) body.reasoning = { effort: model.thinkingLevelMap?.off ?? "none" };` (`packages/ai/src/api/openai-codex-responses.ts:595-596` @v0.87.1, read at the tag). Ported at `crates/cyrup-provider/src/api/openai_codex_responses/request.rs:161-180`: the Off branch reads `thinking_level_map["off"]`, falls back to upstream's literal `"none"` (not `thinking_level_key(Off)`), and an explicit `off: null` suppresses the object. The pre-existing `[CYRUP-DELTA]` at `:164-166` records that upstream's `else if` writes `{ effort }` with no `summary` key — a mechanism note at parity, since cyrup's requested-effort branch is the only one that carries `summary`. Verify: `api::openai_codex_responses::tests::request::off_honours_a_mapped_off_and_a_null_off_suppresses`, with `…::reasoning_effort_maps_and_null_suppresses` as the non-Off control.

**cyrup** — `api/openai_codex_responses/request.rs`: `if clamped != ModelThinkingLevel::Off { … }` — Off
emits nothing ("Codex has no … off branch").

**upstream** — `api/openai-codex-responses.ts:595-597` @v0.87.1: `else if (model.reasoning &&
model.thinkingLevelMap?.off !== null) body.reasoning = { effort: model.thinkingLevelMap?.off ?? "none" }`;
the explicit-`"none"` branch also honours a mapped Off. `e86102f18`, **v0.86.0** (#9191).

**Impact** — a user who turns thinking off on a Codex model still gets (and waits for) default-effort
reasoning.

**Fix** — mirror the `else if` in `request.rs`.

**Verify** — a reasoning Codex model at Off emits `"reasoning":{"effort":"none"}`; a model whose map sets
`off: null` emits no `reasoning`.

## PROV-086 — OpenRouter Chat Completions never sends `x-session-id`

> **CLOSED 2026-09-28.** (on `claude/lows-next`) OpenRouter is detected as session-affinity capable on both openai-completions (`api/compat.rs:799`) and anthropic-messages (`api/anthropic_messages/compat.rs:32-46`), so it sends `x-session-id`. **Ledger correction:** the Fix below waits for `PROV-099` for the Anthropic half; that half does not depend on it and was ported now. `PROV-099` is untouched. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — `api/compat.rs::detect_compat` returns `send_session_affinity_headers: false` for every
endpoint; `openrouter.json` rows carry no override; `api/openai_completions/headers.rs:81-85` only sends
`x-session-id` when the flag holds.

**upstream** — `api/openai-completions.ts:1672` @v0.87.1 `sendSessionAffinityHeaders: isOpenRouter`;
the same commit gives Anthropic Messages an OpenRouter default (`sessionAffinityFormat: "openrouter"` →
`x-session-id`). `bbb61e34a`, **v0.86.0** (#9102). The Baseten half of census lead (4) is data-only
(`sendSessionAffinityHeaders: true` in the generator) and reaches cyrup's dynamic Baseten rows through the
pi.dev overlay — not filed.

**Impact** — OpenRouter requests are not pinned to a replica, so prompt-cache hits are lost and turns cost
more.

**Fix** — `send_session_affinity_headers: is_openrouter`; add the Anthropic OpenRouter default when
`PROV-099` lands.

**Verify** — an OpenRouter completions request with a session id and caching on carries `x-session-id`;
`sendSessionAffinityHeaders:false` in `models.json` suppresses it.

## PROV-087 — Gemini thinking levels come from a model-id family table, not the model's `thinkingLevelMap`

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed · **Filed** 2026-09-24 · **CLOSED 2026-09-27**: upstream `resolveGoogleThinkingLevel` (`packages/ai/src/api/google-shared.ts:48` @v0.87.1), `usesGoogleThinkingLevel` (`:72`, which only selects the wire format) and `getDisabledGoogleThinkingConfig` (`:102`, disabling via `clampThinkingLevel(model, "off")`) — all three read at the tag. Ported at `crates/cyrup-provider/src/api/google_generative_ai/thinking.rs:49` (`resolve_google_thinking_level`, which reads `model.thinking_level_map` and fails the request on an unmappable value), `:111`/`:115` (the resolved level feeding the wire format), `:157-166` (`disabled_thinking_config` routing Off through the same resolver, with `thinkingBudget: 0` when the format does not apply) and `crates/cyrup-provider/src/api/google_generative_ai/capabilities.rs:12` (`uses_google_thinking_level`). The Gemini-3-Pro / Gemma-4 family tables are deleted: the rungs are now DATA on the model, so a `models.json` `thinkingLevelMap` is honoured and an unsupported level is refused instead of being guessed. Vertex inherits it through the shared `build_params`. Verify: `api::google_generative_ai::tests::thinking::{mapped_medium_survives_on_gemini_3_pro,mapped_xhigh_reaches_a_real_google_level,unmappable_thinking_level_fails_the_request,reasoning_off_uses_the_lowest_supported_level,token_budget_is_keyed_on_the_resolved_level}`.

**cyrup** — `api/google_generative_ai/thinking.rs`: `thinking_level` hard-codes Gemini-3-Pro →
LOW/HIGH, Gemma-4 → MINIMAL/HIGH, else the identity map; `disabled_thinking_config` hard-codes LOW for
Gemini-3-Pro and MINIMAL for Flash/Gemma-4. `grep -rn resolve_google_thinking_level crates/` is empty.
Vertex reuses the same `build_params`.

**upstream** — `api/google-shared.ts` @v0.87.1: `resolveGoogleThinkingLevel` (`:48`, v0.84.3, #8059/#8135)
maps through `model.thinkingLevelMap` and throws on an unmappable value; `usesGoogleThinkingLevel` (`:72`)
only picks the wire format; `getDisabledGoogleThinkingConfig` (`:102`) disables via
`clampThinkingLevel(model, "off")`. `google-generative-ai.ts:323` routes Off to `thinking.enabled:false`.
The per-family tables moved into the generator as data (`getGoogleThinkingLevelMap` from models.dev
`reasoning_options`). `16235fd93`, **v0.86.0** (#9455).

**Impact** — a Gemini 3.x or custom Google model whose supported levels differ from cyrup's family guess is
sent a level the API rejects (#9455's report); a `models.json` `thinkingLevelMap` on a Google model is
ignored.

**Fix** — port `resolveGoogleThinkingLevel`, `usesGoogleThinkingLevel` and
`getDisabledGoogleThinkingConfig`; delete the family tables once the catalog carries the maps (`PROV-071`).

**Verify** — a Google model with `thinkingLevelMap:{low:"HIGH"}` at `low` sends `HIGH`; Off on a model
whose map sets `off:null`, `minimal:"LOW"` sends `thinkingLevel: LOW`.

## PROV-088 — Mistral sends `prompt_mode:"reasoning"` where pi sends `reasoning_effort`

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24 · **CLOSED 2026-09-27**: upstream `usesReasoningEffort` is `id === "mistral-small-2603" || id === "mistral-small-latest" || id.startsWith("mistral-medium-") || id === "zai-glm-5-2"` (`packages/ai/src/api/mistral-conversations.ts:898-905` @v0.87.1, read at the tag; `usesPromptModeReasoning` at `:907-908` is `model.reasoning && !usesReasoningEffort`). Ported at `crates/cyrup-provider/src/api/mistral_conversations/reasoning.rs:40-45`, which matches upstream's shape exactly — the whole `mistral-medium-` family by prefix (so no exact `mistral-medium-3.5` arm is needed) plus `zai-glm-5-2` — with `uses_prompt_mode_reasoning` at `:49-51` unchanged as the complement. Those models now send `reasoning_effort` and no `prompt_mode`. Verify: `api::mistral_conversations::tests::reasoning::medium_family_and_glm_use_reasoning_effort_not_prompt_mode`, with `…::{reasoning_effort_models_emit_effort,prompt_mode_reasoning_for_other_reasoning_models}` as the controls on both sides of the predicate.

**cyrup** — `api/mistral_conversations/reasoning.rs::uses_reasoning_effort` matches exactly
`mistral-small-2603 | mistral-small-latest | mistral-medium-3.5`; every other reasoning model falls to
`prompt_mode`.

**upstream** — `api/mistral-conversations.ts:898` @v0.87.1 adds `id.startsWith("mistral-medium-")` and
`zai-glm-5-2`. `96617628e` (#8700) and `4bd3f48df` (#9375), both **v0.86.0**.

**Impact** — reasoning is silently not enabled (GLM-5.2 ignores `prompt_mode`) or rejected (#8700) on
those models.

**Fix** — widen the predicate.

**Verify** — `mistral-medium-3.6` and `zai-glm-5-2` at `high` send `reasoning_effort` and no `prompt_mode`.

## PROV-089 — `openrouter-images.json` is 20 rows behind a source still in git

> **CLOSED 2026-09-28.** On `claude/lows-next`, `xtask/src/main.rs:97` pins `IMAGES_REV = "v0.87.1"` for this one catalog whatever `--rev` says, and `openrouter-images.json` is regenerated to 55 rows, JSON-identical to `IMAGE_MODELS.openrouter` @v0.87.1; the manifest records `pi@v0.87.1` for it. Pinned by `tests::catalog_data::the_images_catalog_is_image_models_at_v0_87_1` and `the_catalog_manifest_names_one_revision_per_provider`. **Correction:** only one existing MAI row was renamed, not four (see the table row). The Verify below was run: `gen-catalogs --check` reports all 35 files matching.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — `crates/cyrup-provider/src/providers/catalog/openrouter-images.json`: 35 rows, generated from
`image-models.generated.ts`'s `openrouter` record at `DEFAULT_REV = "b0c2a90e"` (`xtask/src/main.rs`,
closed `PROV-065`).

**upstream** — `packages/ai/src/image-models.generated.ts` is a tracked data literal at every tag: 40 rows
@v0.83.0, 52 @v0.85.1, **55 @v0.87.1**. Missing from cyrup: the Seedream 5.0 pair, `inclusionai/ming-image-0.1-design`,
the three Krea 2 models, `meta/muse-image`, `microsoft/mai-image-2.5-pro`/`2.6`/`2.6-flash`,
`openai/gpt-image-2.5-flare`/`-sunburst`, `openrouter/auto-beta`, the Qwen Image 3 pair, the four Recraft V4
Styles models and `x-ai/grok-imagine-image-2.0`. Four MAI rows were also renamed ("Microsoft AI: …").

**Impact** — twenty image models pi offers are absent. Unlike the chat catalogs, nothing blocks the refresh
— **until the next pi tag**: post-tag `a328aa89a` deletes `image-models.generated.ts` and moves these rows
into the gitignored `openrouter.json`, after which this becomes `PROV-071`'s class.

**Fix** — pin the `openrouter-images` roster entry to v0.87.1 and regenerate, before bumping past it.

**Verify** — `gen-catalogs --check` against v0.87.1 reports 55 rows and no diff.

## PROV-090 — Anthropic server-side refusal fallback is unported

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed · **Filed** 2026-09-24 · **CLOSED 2026-09-27**: upstream `params.fallbacks` from `compat.allowedFallbackModels` (`packages/ai/src/api/anthropic-messages.ts:1199-1202` @v0.87.1), the `server-side-fallback-2026-07-01` beta, `message_start` repricing through the fallback's `cost` with `output.responseModel` (`:605-614`) and the hard error on a `fallback` content block after output started (`:627-629`). Ported as: `allowed_fallback_models: Option<Vec<AnthropicAllowedFallbackModel>>` on `ModelCompat` (`crates/cyrup-provider/src/api/compat.rs:445`, with `:443` recording that an explicitly empty array in a user's `models.json` is preserved); the `fallbacks` request param (`crates/cyrup-provider/src/api/anthropic_messages/params.rs:255`); `SERVER_SIDE_FALLBACK_BETA` pushed in pi's order, after the interleaved-thinking beta (`headers.rs:20`, `:52-65`, `:91`); and `message_start` setting `dec.response_model` and swapping in the matched fallback's `cost` (`driver.rs:152-172`, field at `blocks.rs:58`, defaulted at `:62` to the value any pre-fallback stream reaches so existing costs are byte-identical). Verify: `api::anthropic_messages::tests::decode::prov090_server_side_fallback::{a_matching_entry_sets_response_model_and_swaps_the_cost,an_echoed_model_id_leaves_response_model_none,b_no_matching_entry_keeps_the_requested_models_cost,c_a_mid_output_fallback_block_is_a_terminal_error,d_a_leading_fallback_block_is_ignored}` and `api::anthropic_messages::tests::params::prov090_allowed_fallback_models_emit_fallbacks_and_the_beta`.

**cyrup** — `grep -rn 'allowed_fallback\|fallbacks' crates/cyrup-provider/src` hits only OpenRouter's
unrelated `allow_fallbacks`; `api/anthropic_messages/driver.rs` `message_start` reads only `id` and usage.

**upstream** — `api/anthropic-messages.ts` @v0.87.1: `params.fallbacks` from
`compat.allowedFallbackModels` (`:1199-1202`), `server-side-fallback-2026-07-01` beta, `message_start`
repricing through the fallback's `cost` and `output.responseModel` (`:605-614`), and a hard error on a
`fallback` content block after output started (`:627-629`). Generator: `ANTHROPIC_ALLOWED_FALLBACK_MODELS =
{ "claude-fable-5": [opus-4-8, opus-5], "claude-opus-5": [opus-4-8] }`. v0.84.3 (#8017, #8285); the
`responseModel` split v0.86.0 (`1283afd0d`, #9188).

**Impact** — on `claude-fable-5` (in cyrup's catalog) a refusal ends the turn where pi is answered by the
fallback model; if a fallback ever does occur it is billed at the requested model's rate and not recorded.

**Fix** — `allowed_fallback_models` on `ModelCompat`; emit `fallbacks` + beta; reprice and set
`response_model` on `message_start`; error on a late `fallback` block.

**Verify** — upstream `test/anthropic-sse-parsing.test.ts` fallback cases translated.

## PROV-091 — Anthropic mid-conversation effort is unported

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** confirmed (mechanism); consequence predicted · **Filed** 2026-09-24

**cyrup** — no `supports_mid_convo_effort` on `ModelCompat`, no `provider_thinking_level` on
`crates/cyrup-core/src/message/assistant.rs`; `api/anthropic_messages/headers.rs` sends
`interleaved-thinking-2025-05-14` whenever `interleavedThinking` is not false and the model is not
adaptive — with no `model.reasoning` / thinking-enabled gate.

**upstream** — `api/anthropic-messages.ts` @v0.87.1: `providerThinkingLevel` recorded on output; for
`supportsMidConvoEffort` models `thinking:{type:"adaptive", block_binding:{prefix_mismatch_behavior:"drop_block"}}`
and `output_config.effort:"high"` (`:1150-1158`, with the in-source reason "so prefix mismatches can be
dropped instead of surfacing as persistent 400 responses"), `insertThinkingLevelMessages` adds a
`{role:"system", content:[], output_config:{effort}}` before each historical turn and one for the active
effort (`:1434-1447`), plus two betas; `getBetaFeatures` gates interleaved thinking on `model.reasoning &&
thinkingEnabled === true` (`:1019-1025`). Generator: Anthropic + OpenRouter, `claude-opus-5`, `opus-5-5`,
`fable|mythos-5.1`, except OpenRouter Opus 5. `4e69b0c28`, v0.85.0.

**Impact** — changing thinking level mid-session on those models is predicted, by upstream's own comment,
to produce persistent 400s. None of them is in cyrup's embedded catalog, but the pi.dev overlay serves them
(with the compat key cyrup ignores). The interleaved-beta gate is cosmetic on first-party Anthropic.

**Fix** — port the compat key, the persisted field (JSONL: area 03), the message insertion, the thinking
block and the betas; gate interleaved-thinking as upstream does.

**Verify** — `test/anthropic-mid-conversation-effort.test.ts` @v0.87.1 translated.

## PROV-092 — openai-completions `reasoning_details` replay is the pre-v0.84.3 shape

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed · **Filed** 2026-09-24 · **CLOSED 2026-09-27**: upstream's v0.84.3/v0.84.4 shape, read at the tag: the `reasoning.text`/`.summary`/`.encrypted` variant union and its guard (`packages/ai/src/api/openai-completions.ts:135-139`), `appendOpenAIReasoningDetail` merging consecutive text/summary deltas (`:252`), the array stored once on the THINKING block's `thinkingSignature` (`:664-674`) and replay preferring it with `parseLegacyEncryptedReasoningDetail` as the fallback (`:225`, `:1304-1312`). Ported as a new `crates/cyrup-provider/src/api/openai_completions/reasoning_details.rs` (`parse_openai_reasoning_details`, `parse_legacy_encrypted_reasoning_detail`, the append/merge), `blocks.rs:43-47` (`streamed_reasoning_details`, merged in arrival order) and `:134` (stored on the thinking block's `thinking_signature`), and `convert.rs:231-252` + `:319-320`, where the preserved array is the thinking block's own signature or, failing that, the legacy per-tool-call signatures, and `:281` suppresses the raw reasoning field only when details were preserved. Signed reasoning text from Anthropic/Gemini models behind OpenRouter is now replayed across tool turns, and reasoning with no tool call survives. Verify: `api::openai_completions::tests::reasoning_details::{preserves_reasoning_details_in_the_thinking_signature,falls_back_to_encrypted_tool_call_signatures,preserves_text_and_summary_details_in_sequence,merges_consecutive_text_and_summary_deltas}`.

**cyrup** — `api/openai_completions/decode.rs` (step 4) keeps only details that
`encrypted_reasoning_detail_id` accepts, storing each as the matching tool call's `thought_signature`;
`convert.rs` (≈`:267-289`) rebuilds `reasoning_details` by parsing tool-call signatures.

**upstream** — `api/openai-completions.ts` @v0.87.1: all three variants (`reasoning.text`/`.summary`/
`.encrypted`, `:135-139`), consecutive text/summary deltas merged by `appendOpenAIReasoningDetail`
(`:252`), the array stored once on the THINKING block's `thinkingSignature` (`:664-674`), replay preferring
it and falling back to `parseLegacyEncryptedReasoningDetail` (`:225`, `:1304-1312`), and a raw reasoning
field sent only when no details are preserved. v0.84.3/v0.84.4 (#7994, #8246, #8605).

**Impact** — on OpenRouter, signed reasoning text (Anthropic/Gemini models behind it) is never replayed,
so reasoning continuity is lost across tool turns; reasoning with no tool call is dropped entirely.

**Fix** — port the variant union, the merge and the thinking-block anchoring, keeping the legacy parse for
old sessions.

**Verify** — `test/openai-completions-reasoning-details.test.ts` @v0.87.1 translated.

## PROV-093 — openai-responses: no `supportsMaxOutputTokens`, and `24h` retention sent to GPT-5.6+

> **CLOSED 2026-09-28.** (on `claude/lows-next`) `supportsMaxOutputTokens` gates `max_output_tokens`. Explicit-mode models now get `prompt_cache_options.ttl:"30m"` on long retention instead of `prompt_cache_retention:"24h"` (`api/openai_responses/params.rs:34-62`, `:165-177`). **Ledger correction:** `supportsExplicitPromptCacheMode` was already ported (`PROV-023`); only `supportsMaxOutputTokens` was missing. The catalog flags stay with `PROV-071`. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — `api/openai_responses/params.rs`: `max_output_tokens` whenever `max_tokens > 0`; `"24h"` on
`Long` whenever `supports_long_cache_retention`, independent of `supports_explicit_prompt_cache_mode`; no
`ttl`. Neither compat key exists.

**upstream** — `api/openai-responses.ts` @v0.87.1 `:79` `supportsMaxOutputTokens ?? true`, `:321`;
`getPromptCacheRetention` returns `24h` only when `!supportsExplicitPromptCacheMode` and
`getPromptCacheOptions` returns `{ttl:"30m"}` for Long on explicit-mode models (`:83-99`). v0.85.0 (#8941),
v0.85.1.

**Impact** — a Codex-protocol gateway that rejects `max_output_tokens` cannot be configured to work; GPT-5.6+
long retention asks for a `24h` the model family does not use and never gets the `30m` TTL. Same code
block as closed `PROV-019`/`PROV-023`. cyrup's embedded GPT-5.6 rows also lack
`supportsExplicitPromptCacheMode` (catalog, `PROV-071`).

**Fix** — both compat keys and the two helpers.

**Verify** — `test/cache-retention.test.ts` and `openai-responses-compat.test.ts` @v0.87.1 translated.

## PROV-094 — openai-responses and azure-openai-responses never emit `tool_choice`

> **CLOSED 2026-09-28.** (on `claude/lows-next`) Both Responses builders now send `tool_choice` after `tools` (`api/openai_responses/params.rs:216-220`, `api/azure_openai_responses.rs:402-409`), with a forced function in the Responses shape `{type:"function", name}`. Evidence is in the row.

**Kind** parity-bug (openai-responses) + upstream-drift (Azure, v0.84.3) · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — `grep -rn tool_choice crates/cyrup-provider/src/api/openai_responses crates/cyrup-provider/src/api/azure_openai_responses.rs`
is empty outside tests; the completions, Anthropic, Google/Vertex, Bedrock, Mistral and Codex adapters
all read `StreamOptions.tool_choice`. (The census lead that `SimpleStreamOptions` lacks the field is
refuted: `utils/simple_options.rs:127` forwards `base.tool_choice`.)

**upstream** — `api/openai-responses.ts:308-309` @v0.83.0 and `:340-341` @v0.87.1; Azure `:323-324`
@v0.87.1 (fixed v0.84.3).

**Impact** — a forced or suppressed tool choice is silently ignored on the two Responses adapters. No
production caller in cyrup sets one today; extensions and SDK callers can.

**Fix** — emit `tool_choice` from `opts.tool_choice` in both `build_params`.

**Verify** — a Responses request with `ToolChoice::None` carries `"tool_choice":"none"`.

## PROV-095 — No default `User-Agent` on seven adapters

> **CLOSED 2026-09-28.** (on `claude/lows-next`) `utils/user_agent.rs` ports `getPiUserAgent`; the seven adapters send `cyrup (<platform> <release>; <arch>)` beneath every overlay, and Codex uses the same builder with pi's product token. **Ledger correction:** Codex was also a `getPiUserAgent` site at v0.87.1 and now matches it. One platform divergence remains (Windows omits the release). Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — only Codex (`api/openai_codex_responses/headers.rs:95`) and the catalog fetch
(`remote_catalog.rs::cyrup_user_agent`) set one; no `.user_agent(` on any client builder.

**upstream** — `utils/pi-user-agent.ts` @v0.87.1, applied in `openai-completions.ts:760`,
`openai-responses.ts:248`, `azure-openai-responses.ts:259`, `anthropic-messages.ts:295`,
`google-generative-ai.ts:357`, `google-vertex.ts:398`, `mistral-conversations.ts:338`. v0.84.3 (#8305).

**Impact** — gateways that reject or rate-limit requests without a User-Agent treat cyrup differently from pi.

**Fix** — a `cyrup (<os> <release>; <arch>)` default under caller headers on those seven.

**Verify** — each adapter's request carries the default and a caller `User-Agent` overrides it.

## PROV-096 — `NO_PROXY` bare-domain entries do not match subdomains

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24 · **CLOSED 2026-09-27**: upstream `parseNoProxyEntry` (`packages/ai/src/utils/node-http-proxy.ts:41` @v0.87.1, which trims and lower-cases before bracket/IPv6 handling) and `shouldProxyHostname` (`:74`), whose exemption is the exact host (`:106`) OR `normalizedTargetHost.endsWith("." + domain)` (`:110`) — read at the tag. Ported at `crates/cyrup-provider/src/utils/node_http_proxy.rs:83-93` (`strip_brackets`), `:93-134` (`parse_no_proxy_entry`, lower-casing at `:94` and handling a bracketed IPv6 host so `rsplit_once(':')` can no longer mis-parse `[::1]:8080`), and `:142-176`, where both the `no_proxy` list and the target host are lower-cased and the test is `normalized_target != domain && !normalized_target.ends_with(&format!(".{domain}"))`. `NO_PROXY=internal.corp` now exempts `api.internal.corp`. Verify: `utils::node_http_proxy::tests::{no_proxy_matches_subdomains_wildcards_ipv6_and_ports,no_proxy_normalises_target_case}`, with `…::{no_proxy_suffix_and_exact_match,no_proxy_port_qualified_only_blocks_matching_port}` as the pre-existing controls.

**cyrup** — `utils/node_http_proxy.rs` (`should_proxy_hostname` port): an entry not starting with `.`/`*`
is an exact-host test (`hostname != proxy_hostname`); no lower-casing; `rsplit_once(':')` mis-parses a
bracketed IPv6 entry.

**upstream** — `utils/node-http-proxy.ts` @v0.87.1: `parseNoProxyEntry` (`:41`, lower-cases, brackets,
IPv6) and `shouldProxyHostname` (`:74`), which exempts both the exact host and `endsWith("." + domain)`
(`:110`). v0.85.0 (#8737).

**Impact** — `NO_PROXY=internal.corp` still proxies `llm.internal.corp`; an internal gateway reached
through a corporate proxy typically fails.

**Fix** — port `parseNoProxyEntry` and the new matcher.

**Verify** — `NO_PROXY=internal.corp` exempts `api.internal.corp`; `NO_PROXY=[::1]:8080` exempts that port only.

## PROV-097 — Bedrock: redacted reasoning dropped, empty-key arguments replayed, response headers truncated

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed · **Filed** 2026-09-24 · **CLOSED 2026-09-27**: upstream, read at the tag: `redactedContent` buffered with the `[Reasoning redacted]` placeholder (`packages/ai/src/api/bedrock-converse-stream.ts:658-674` @v0.87.1) and replayed as `reasoningContent.redactedContent` rather than lowered to reasoning text (`:1010-1019`); `sanitizeBedrockDocument` (`:921`) applied to `toolUse.input` (`:1008`); and `addResponseHeadersMiddleware` (`:510`) passing every response header. Ported as: `REDACTED_THINKING_PLACEHOLDER` and the `redactedContent` read (`crates/cyrup-provider/src/api/bedrock_converse_stream/events.rs:18`, `:284-286`, with `:268` recording why two concurrent payloads are not concatenated), the `redacted` flag plus the decoded-bytes buffer on the thinking block (`blocks.rs:29-32`), `decode_redacted_content` and the `reasoningContent.redactedContent` replay arm (`convert.rs:131`, `:234-243` — a payload that will not decode is dropped, not degraded to text), `sanitize_bedrock_document` recursing over arrays and objects (`convert.rs:112-121`) applied at `:221`, and the full header map on `ProviderResponse::headers` (`driver.rs:230`). Verify: `api::bedrock_converse_stream::tests::decode::prov097_redacted_reasoning::{redacted_content_becomes_one_placeholder_thinking_block,a_signature_after_redacted_content_is_not_appended,the_plain_text_and_signature_path_is_unchanged}`, `…::tests::convert::prov097_replay::{a_redacted_thinking_block_replays_as_redacted_content,a_redacted_thinking_block_without_a_payload_is_dropped,empty_object_keys_are_stripped_from_tool_use_input_at_every_depth}` and `…::tests::driver::prov097_on_response_receives_every_header`.

**cyrup** — `api/bedrock_converse_stream/events.rs` reads `reasoningContent` text/signature only and
`blocks.rs:148` sets `redacted: false` unconditionally (`grep -rn redactedContent crates/` empty);
`convert.rs` replays `toolUse.input` as `tc.arguments` verbatim; `driver.rs` hands `on_response` a map
holding only `x-amzn-requestid`.

**upstream** — `api/bedrock-converse-stream.ts` @v0.87.1: `redactedContent` buffered with the
`[Reasoning redacted]` placeholder (`:658-674`) and replayed as `redactedContent` (`:1016`);
`sanitizeBedrockDocument` drops empty keys recursively (`:921`, used at `:1008`);
`addResponseHeadersMiddleware` passes every response header (`:510`). v0.84.2/v0.84.3 (#8314, #7882, #8234).

**Impact** — a Claude-on-Bedrock turn whose reasoning was redacted loses the block, so the tool-use
continuation that must replay it is rejected or loses context; a tool call with an empty-string key is
rejected on replay; `onResponse` consumers see one header.

**Fix** — the three ports, in `events.rs`, `convert.rs` and `driver.rs`.

**Verify** — `test/bedrock-redacted-reasoning.test.ts` and `bedrock-response-headers.test.ts` @v0.87.1 translated.

## PROV-098 — GitHub Copilot login accepts every catalog model's policy at once

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed · **Filed** 2026-09-24 · **STILL OPEN 2026-09-27**: the row's own claim (policy filter, retrying fetch, sequential batch, the `policyModelIds.length > 0` guard, the insertion-ordered union) is covered by five passing `prov098::` tests and nothing outside `crates/cyrup-provider` was touched, but THREE things remain. (1) BLOCKING — delta misuse at `auth/oauth/github_copilot.rs:636-639`: the parity claim must go and the code be made true to v0.87.1 instead. (a) `fetch_with_rate_limit_retry` (`:641-701`) must cap every attempt by the retry budget the way pi's composed `AbortSignal` does — the `deadline` already computed at `:652-654` should bound the send (e.g. `tokio::time::timeout_at`), with expiry taking the same error path as pi's `AbortError` (`OAuthError::Failed`, or `OAuthError::Cancelled` when the caller's token fired); today total elapsed is unbounded by `COPILOT_MAX_RATE_LIMIT_ELAPSED_MS` and an attempt straddling the deadline succeeds where pi fails. (b) `enable_model` (`:777-791`) is missing the per-request cap: upstream's policy POST is `AbortSignal.timeout(5000)` per attempt (`github-copilot.ts:149-151` via `:398`) and gives up at 5 s with `return false` (`:400-403`), where cyrup's POST sets no `.timeout(...)` and its only bound is the 5-minute global read-idle timeout (`stream/sse.rs:66`), so a stalling policy endpoint hangs login and the sequential batch for minutes. (c) Then rewrite the note as a true mechanism delta carrying no behavioural claim, or delete it once (a) and (b) land. Each change needs a test that fails without it — a loopback server that answers `/models` normally, then 429s the first `/models/<id>/policy` and stalls the retry past the budget, asserting the login returns (batch broken, policy paths recorded) within the budget rather than after the idle timeout — with the deadline/knobs INJECTED rather than sleeping 5 s, and nothing skipped or quarantined. (2) Provenance, the same class as the refusal's Defect 2, to be RE-DERIVED at the tag rather than copied from here: `providers/github_copilot.rs:666` and `auth/oauth/github_copilot.rs:707` cite `:171` for `const allowPolicyFallback` (actual `:177`, its comment `:175-176`); `providers/github_copilot.rs:680-683` cite `:136-139` (actual `:141-144`), `:157` (actual `:152`) and `:361` (actual `:360`), while `:150` is correct; and, stale from a pre-v0.87.1 read, `auth/oauth/github_copilot.rs:839` cites `loginGitHubCopilot` as `github-copilot.ts:329-359` (actual `:434-485`), `:807` cites `enableGitHubCopilotModels` as `:411-431` (actual `:414-432`), and `:373-407` for `enableGitHubCopilotModel` is `:373-412`. (3) Nothing else is missing: Defect 1 is closed with a proven red-without and Defect 3's new delta is sound.

**cyrup** — `auth/oauth/github_copilot.rs::enable_all_models` maps `enable_model` over
`github_copilot_models()` and `join_all`s them, ignoring every result; no 429 handling.

**upstream** — `auth/oauth/github-copilot.ts` @v0.87.1: `parseGitHubCopilotModelCatalog` returns
`policyModelIds` (only `policyState === "unconfigured"` models pi knows, `:93-132`),
`fetchWithRateLimitRetry` (`:135`, 429 + `retry-after` within a time budget), sequential enablement that
stops the batch on rate limit, run only when `policyModelIds.length > 0` (`:472`). v0.84.2/v0.84.3 (#6187, #7850).

**Impact** — each login fires one POST per catalog model concurrently; GitHub rate-limits the burst, so
some policies are silently not enabled, and policies already configured are re-posted. `PROV-029` made this
path reachable.

**Fix** — port the policy filter, the retrying fetch and the sequential loop.

**Verify** — against a mock returning 429 on the second call, login makes two calls and stops; an
`enabled` policy is never posted.

## PROV-099 — OpenRouter `anthropic/*` models are routed over openai-completions

> **CLOSED 2026-10-02** (stale; see the table row). The **cyrup** paragraph below describes the state when this was filed (2026-09-24) and is no longer true: the catalog routes the 15 non-`:batch` `anthropic/*` rows over `anthropic-messages`, now guarded by `tests/openrouter_anthropic_route.rs`.

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — `providers/fleet.rs` registers `openrouter` as `Completions`; the 15 `anthropic/*` rows in
`openrouter.json` carry `api: openai-completions`, base `https://openrouter.ai/api/v1`.

**upstream** — `providers/openrouter.ts` @v0.85.1 and @v0.87.1 registers both `anthropic-messages` and
`openai-completions`; the generator's `fetchOpenRouterModels` sends every `anthropic/*` (non-`:batch`) row
over `anthropic-messages` at `https://openrouter.ai/api`. v0.85.0. (The xAI half of this census lead is
resolved: all three `xai.json` rows are `openai-responses`.)

**Impact** — OpenRouter Claude turns lose native thinking-signature replay and cannot take the per-turn
effort path (`PROV-091`). Note the post-tag lead below: pi.dev may already serve cyrup the anthropic-messages
rows, which cyrup would dispatch to its Anthropic impl with OpenRouter's base URL — unverified.

**Fix** — register OpenRouter with both apis and take routing from the catalog.

**Verify** — an overlay row `anthropic/claude-sonnet-5` with `api: anthropic-messages` streams against a
mock OpenRouter `/api/v1/messages`.

## PROV-100 — `thinkingTokenBudgetField`, `{"$var":"thinking.budget"}` and `vllmPriority` unported

> **CLOSED 2026-09-28.** (on `claude/lows-next`) `thinkingTokenBudgetField`, `supportsThinkingTokenBudget` and `vllmPriority` are ported, and `{"$var":"thinking.budget"}` resolves to the clamped token budget (`api/openai_completions/params.rs:190-277`, `reasoning.rs:225-230`). **Ledger correction:** the row did not list `supportsThinkingTokenBudget`, a fourth upstream key; it is ported. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-09-24

**cyrup** — `ModelCompat` has none of the three; `api/openai_completions/reasoning.rs` resolves any
`$var` other than `thinking.enabled` through the effort map, so `thinking.budget` becomes an effort string.

**upstream** — `types.ts` @v0.87.1 `:93`, `:98`, `:725`, `:750`; `api/openai-completions.ts:866-867`
(`priority`), `:870`/`:976-977` (budget field). v0.84.3 (#8275) and v0.85.0 (#9004). Never set by the
generated catalog.

**Impact** — vLLM/SGLang/llama.cpp users configuring a thinking budget or request priority through
`models.json` get no budget, or a string where a number is expected.

**Fix** — port the three keys and the `simple-options.ts` budget table they share.

**Verify** — `test/openai-completions-vllm-priority.test.ts` and the budget-field cases @v0.87.1 translated.

## PROV-101 — Responses adapters have no grammar (`custom`) tool path

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed · **Filed** 2026-09-30 (found closing `DRIFT-058`)

**cyrup** — `api/openai_responses/tools.rs` `ConvertResponsesToolsOptions` carries `defer_loading`, `supports_strict_mode`, `default_strict` only ("only the members cyrup's function-tool branch consumes"); `convert_responses_tools` always emits `type:"function"`. `grep -rn custom_tool_call crates` is empty. `utils/constrained_sampling.rs` (`resolve_grammar_constrained_sampling`) and `ResolvedResponsesCompat::supports_openai_grammar_tools` exist but no Responses code calls them.

**upstream** — `packages/ai/src/api/openai-responses-shared.ts` @v0.87.1: `grammarToolInputProperties` option (`:111`, `:124`); `supportsOpenAIGrammarTools` (`:135`, `:362`); a grammar tool is sent as `{type:"custom", ...}` (`:365-368`); replay emits `custom_tool_call` (`:306-313`, namespace gated by `isSameModel`, `:315`) and `custom_tool_call_output` (`:335-338`); decoding opens a `custom_tool_call` slot (`:504-523`), accumulates `response.custom_tool_call_input.delta`/`.done` (`:670-690`) and closes it on `output_item.done` (`:726-738`). Test: `packages/ai/test/openai-responses-namespace.test.ts` "round-trips a custom-tool namespace received only on output_item.done".

**Impact** — a tool that opts into grammar-constrained sampling is sent to OpenAI as an ordinary JSON-schema function, so the grammar constraint is never applied on Responses routes; `ToolCall.namespace` on a `custom_tool_call` (modelled by `DRIFT-058`) cannot arise.

**Fix** — thread `supports_openai_grammar_tools` and a `grammar_tool_input_properties` map through `ConvertResponsesToolsOptions`/`convert_responses_messages`; add the `custom` tool arm, the `custom_tool_call(_output)` replay arms (with the `DRIFT-058` same-model namespace gate), and the `custom_tool_call` slot plus `response.custom_tool_call_input.*` decoding.

**Verify** — the custom-tool case of `openai-responses-namespace.test.ts` translated, plus a body-shape test that a grammar-constrained tool is sent as `type:"custom"` under `supportsOpenAIGrammarTools`.

## Findings filed 2026-10-02 — the `v0.87.1..v1.0.0` window in `packages/ai`

pi v1.0.0 (`2026-10-01`). Upstream read only at the tag, with `git -C tmp/pi show v1.0.0:<path>` and
`git -C tmp/pi diff v0.87.1..v1.0.0 -- packages/ai`; cyrup read at `fe875569`. The window's headline for
this area is the deletion of the parallel image-model registry (`PROV-128`) and the login surfaces pi
added beside it (`PROV-117`…`PROV-120`). Four follow-ups to `PROV-117`/`PROV-120` from outside this window, `PROV-135` and `PROV-137` (v0.99.0), `PROV-136` (v1.1.0) and `PROV-138` (a port bug since v0.83.0), were filed and closed on 2026-10-08; their sections follow `PROV-133`.

## PROV-113 — `onProviderStreamEvent` and its nine adapter call sites are unported

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed · **Filed** 2026-10-02

**upstream** — `002fc8385` ("expose provider stream events to extensions", #9901). `packages/ai/src/types.ts:198` @v1.0.0 adds to `StreamOptions`:

```ts
onProviderStreamEvent?: (data: unknown, model: Model<Api>) => void | Promise<void>;
```

with the contract in its doc comment: "each parsed provider stream event before Pi normalization. Event data is adapter-owned and must be treated as read-only. Adapter support is explicit; unsupported adapters do not invoke it." `api/simple-options.ts:41` threads it through `buildBaseOptions`. Nine adapters invoke it, each immediately after parsing one wire event and before any normalization: `api/anthropic-messages.ts:666`, `api/openai-completions.ts:554`, `api/google-generative-ai.ts:107`, `api/google-vertex.ts:116`, `api/bedrock-converse-stream.ts:297`, `api/pi-messages.ts:415`, `api/openai-responses-shared.ts:600` (reached from `openai-responses.ts:195` and `azure-openai-responses.ts:134` via the `onProviderStreamEvent` field on its options bag at `:111`), `api/openai-codex-responses.ts:751` (passed at `:669` and `:1549`), and `api/mistral-conversations.ts:597` (threaded as a parameter from `:153`, declared `:567`).

**cyrup** — `grep -rn 'on_provider_stream_event\|onProviderStreamEvent' crates/ --include='*.rs'` at HEAD returns zero. `StreamOptions` (`crates/cyrup-provider/src/stream.rs`) carries the sibling hooks `on_payload` and `on_response` (the plumbing at `stream.rs:71` and `:120-129`) but no third hook, and `build_base_options` (`utils/simple_options.rs:84-…`) has no field to thread.

**Impact** — nothing; cyrup has no consumer yet. This row exists because the surface is the ai-side half of a two-part feature and because the ledger recorded it as a lead and never as an item: `01-cyrup-core-and-provider.md:638` says of `002fc8385` "`onProviderStreamEvent` / `provider_stream_event` extension event — area 06's surface", and `grep -rn 'provider_stream_event' docs/gap-analysis/06-cyrup-ext.md` is empty, so neither half is filed. File this half here; the `provider_stream_event` extension event and its WIT surface remain area 06's and are still unfiled.

**Fix** — add `on_provider_stream_event: Option<Arc<dyn Fn(&serde_json::Value, &Model) -> BoxFuture<'_, ()> + Send + Sync>>` (or the crate's existing hook shape, matching `on_payload`) to `StreamOptions`; thread it in `build_base_options`; call it at the nine sites above, each on the raw parsed event before the decoder touches it. Match upstream's two invariants: the hook runs before normalization, and an adapter that does not support it simply never calls it.

**Verify** — one unit test per adapter that feeds a two-event canned stream through a loopback and asserts the hook saw both raw events, in order, before the corresponding normalized `StreamEvent`s were emitted. Red at HEAD because the field does not compile.

## PROV-114 — Mistral reasoning lowering is pinned to a hardcoded model-id set pi deleted

**Kind** stale-port · **Severity** medium · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-02

> **CORRECTED 2026-10-03:** impact is wider than stated. Every reasoning-effort row in cyrup's Mistral catalog (`crates/cyrup-provider/src/providers/catalog/mistral.json`: `mistral-medium-2604`, `mistral-medium-3.5`, `mistral-medium-latest`, `mistral-small-2603`, `mistral-small-latest`, `zai-glm-5-2`) carries `thinkingLevelMap.off = "none"`, and `lower_reasoning` returns `(None, None)` for any level that is not on (`api/mistral_conversations/reasoning.rs:15-17`), so cyrup never sends `reasoning_effort:"none"` for reasoning-off where pi sends `effortMap.off` (`mistral-conversations.ts:202-213` @v1.0.1, identical at v1.0.0). `zai-glm-5-3` (`mistral.json:840`, map `low:"low"`, `high:"high"`, `max:"max"`, `off:null`) is the only map-carrying row outside `uses_reasoning_effort`'s id set. `MistralReasoningEffort` (`options.rs:26`) has only `None`/`High`, so `low` and `max` need the enum widened, with `api/mistral_conversations/tests/reasoning.rs` updated. Recommended re-rating (not applied): effort S→M because of the enum widening and test updates.

**upstream** — `dc84c1ac0` ("send requested thinking level to Mistral reasoning models"). pi v1.0.0 deletes `usesReasoningEffort`, `usesPromptModeReasoning` and `mapReasoningEffort` outright and replaces the three with a presence test on the catalog's `thinkingLevelMap` (`api/mistral-conversations.ts:202-213`):

```ts
// Models with a thinking level map use `reasoning_effort`; other reasoning models use `prompt_mode`.
const effortMap = model.reasoning ? model.thinkingLevelMap : undefined;
const reasoningEffort = effortMap ? (reasoning ? (effortMap[reasoning] ?? "high") : (effortMap.off ?? undefined)) : undefined;
…
promptMode: model.reasoning && !effortMap && reasoning ? "reasoning" : undefined,
reasoningEffort: reasoningEffort as MistralReasoningEffort | undefined,
```

`MistralReasoningEffort` also widens from `"none" | "high"` to `"none" | "low" | "medium" | "high" | "max"` (`:34`).

**cyrup** — `crates/cyrup-provider/src/api/mistral_conversations/reasoning.rs:40-50` is the deleted v0.87.1 shape, closed as `PROV-088` on 2026-09-27:

```rust
fn uses_reasoning_effort(model: &Model) -> bool {
    matches!(model.id.as_str(), "mistral-small-2603" | "mistral-small-latest" | "zai-glm-5-2")
        || model.id.as_str().starts_with("mistral-medium-")
}
```

and `lower_reasoning` (`:12-34`) returns `(None, None)` whenever `!reasoning.is_on()`.

**Impact** — two divergences, both demonstrable against catalog data cyrup already ships. (1) `crates/cyrup-provider/src/providers/catalog/mistral.json` carries `zai-glm-5-3` with `reasoning: true` and a `thinkingLevelMap` of `{low, high, max}`, and that id is not in cyrup's `matches!` set, so cyrup sends `prompt_mode: "reasoning"` where pi v1.0.0 sends `reasoning_effort` — the field pi's own commit says `prompt_mode` is "ignored or unsupported" on. Any further Mistral-hosted model that arrives through the live catalog with a map has the same fate. (2) With reasoning off, pi now sends `reasoning_effort: effortMap.off` (`"none"` for every mapped Mistral row cyrup ships); cyrup sends nothing, leaving the server's default in place. The `low`/`medium`/`max` widening is type-level only — `map_reasoning_effort` already returns the map's own string.

**Fix** — replace `uses_reasoning_effort`/`uses_prompt_mode_reasoning` with the presence test, and move the reasoning-off early return so a model with a map still yields `reasoning_effort` from its `off` key. Keep `map_reasoning_effort`'s `?? "high"` fallback: it is unchanged upstream.

**Verify** — table test in `api::mistral_conversations::tests::reasoning` over cyrup's own catalog rows: `zai-glm-5-3` with `reasoning: high` sends `reasoning_effort` and no `prompt_mode`; `magistral-medium-latest` (reasoning, no map) still sends `prompt_mode`; `mistral-medium-latest` with reasoning off sends `reasoning_effort: "none"`. All three red at HEAD. Note that `PROV-088`'s existing test `medium_family_and_glm_use_reasoning_effort_not_prompt_mode` keeps passing under the new rule and should be re-expressed against the map rather than the id list.

## PROV-115 — An empty Mistral text delta splits thinking into two blocks, which Mistral rejects on replay

**Kind** upstream-drift · **Severity** high · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-02

> **CORRECTED 2026-10-03:** the guard sites are unchanged at v1.0.1 (`api/mistral-conversations.ts:636` string item, `:678` `type:"text"` item, each `if (!textDelta) continue;`, with the GLM comment at `:633-635`). On the cyrup side `push_text` is reached unguarded from three sites in `api/mistral_conversations/content.rs` (`:92` string content, `:99` string array item, `:126` `type:"text"` item); only the thinking path guards (`:117`, `delta.is_empty()`). `zai-glm-5-2` (`mistral.json:811`) and `zai-glm-5-3` (`:840`) are in the embedded catalog. No test covers an empty delta. Severity is defensible but generous: it affects two non-flagship models and needs an empty delta mid-thinking. No re-rating recommended; the S fix is two `is_empty` guards.

**upstream** — `8930b9ec0` ("ignore empty Mistral content deltas"). pi v1.0.0 adds the same guard at both text-delta sites in `consumeChatStream`, `api/mistral-conversations.ts:634-636` and `:676-678`, with the reason in the comment:

```ts
// GLM models on Mistral send empty content deltas around thinking and tool calls.
// Opening a block for them splits thinking into multiple blocks, which Mistral rejects on replay.
if (!textDelta) continue;
```

**cyrup** — `crates/cyrup-provider/src/api/mistral_conversations/content.rs` calls `push_text` unguarded at all three text sites (`:92` for string content, `:99` for a string array item, `:126` for a `{type:"text"}` item), and `push_text` opens a block for any delta, empty included: when `dec.current != Some(CurrentKind::Text)` it runs `close_current` — which emits `ThinkingEnd` for an open thinking block — then `dec.push_block(Content::text(""))` and `TextStart`. The thinking branch at `:116-120` *does* skip empty deltas (`if delta.is_empty() { continue; }`), so the splitting comes entirely from the text path. Nothing drops the empty text block later: `blocks.rs:104-131` `close_current` has no emptiness test, and `finalize_tool_blocks`/`finish.rs` do not prune.

**Impact** — the failure pi describes is reproducible on cyrup's own catalog. `crates/cyrup-provider/src/providers/catalog/mistral.json` ships `zai-glm-5-2` and `zai-glm-5-3`, both `reasoning: true`. On a turn where Mistral emits an empty content delta between two thinking deltas, cyrup's assistant message ends up with `thinking`, `text:""`, `thinking` instead of one thinking block. `api/mistral_conversations/messages.rs:65` then replays each thinking block as its own assistant content entry (`"thinking": [{ "type": "text", "text": … }]`), and Mistral rejects that body — so the turn after the split fails, and keeps failing while the message stays in context. That is a conversation-breaking failure on a model cyrup offers, which is why this is the one `high` in the batch.

**Fix** — add `if delta.is_empty() { continue; }` ahead of each of the three `push_text` call sites in `process_content` (pi guards two because its string-content path collapses differently; cyrup's third site needs the same treatment for the same reason). Do not put the guard inside `push_text`: a caller that legitimately wants an empty text block — there is none today — would be silenced invisibly.

**Verify** — `api::mistral_conversations::tests` decode test over a canned stream of `thinking "a"`, `content ""`, `thinking "b"`: the message has exactly one thinking block containing `"ab"` and no text block. Red at HEAD (three blocks). A second test should round-trip that message through `messages.rs` and assert one assistant thinking entry.

## PROV-116 — Responses streams hand the agent unfinished tool calls

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-02

> **CORRECTED 2026-10-03:** the upstream guard is at `api/openai-responses-shared.ts:763-777` (v1.0.0 and v1.0.1; throw text at `:772`), not `:764-775`. It is absent at v0.87.1. cyrup's `Decoder.slots` (`api/openai_responses/decoder.rs:33`) is documented as "removed on `output_item.done`", so the port is: when the settled stop reason is `ToolUse` and a Tool-kind slot remains, end the stream with an error. It needs a non-compliant server to trigger. Recommended re-rating (not applied): medium→low (reviewer judgement: the trigger is a non-compliant server; effort S unchanged).

**upstream** — `1b2aa0ca0` ("reject unfinished Responses tool calls instead of running them", fixes #9974). pi v1.0.0 adds, after the terminal-event check in `processResponsesStream` (`api/openai-responses-shared.ts:764-775`):

```ts
if (output.stopReason === "toolUse") {
    for (const block of output.content) {
        if (block.type !== "toolCall") continue;
        const toolCall = block as StreamingToolCall;
        if (toolCall.partialJson !== undefined || toolCall.customInput !== undefined) {
            throw new Error(`OpenAI Responses stream completed with an unfinished tool call: ${toolCall.name} (${toolCall.id})`);
        }
    }
}
```

The scratch buffers are the liveness marker — a finished call has had `partialJson`/`customInput` deleted. The commit message gives the concrete failure: with a server that omits `output_index`, two parallel calls became three (`echo a`, `echo a`, `echo b`), two sharing an id, and the agent ran all three.

**cyrup** — `crates/cyrup-provider/src/api/openai_responses/decoder.rs:203-227` ends the stream on `saw_terminal` alone and emits `StreamEvent::end_of_stream`. `grep -n 'unfinished' crates/cyrup-provider/src/api/openai_responses/` is empty. The `partial_json` scratch buffer exists and is the right marker: `blocks.rs:27`/`:55` declare it, `events.rs:109-130` accumulate into it, and `events.rs:206-223` is where a completed item replaces it.

**Impact** — the known trigger does not reach cyrup: `crates/cyrup-llama` streams over `openai-completions`, not `openai-responses` (`cyrup-llama/src/provider.rs:5-6`, `model.rs:148`). But the Responses decoder is shared by `openai`, `azure-openai-responses`, `openai-codex`, `opencode` and any OpenAI-compatible relay in the live catalog, and the consequence of the gap is that cyrup executes a tool call whose arguments were truncated mid-JSON or merged from two concurrent calls. For `bash`/`edit`/`write` that is a destructive action with wrong arguments, which is why this is `medium` rather than `low` despite no in-tree reproduction.

**Fix** — in the `end_of_stream` path at `decoder.rs:203-227`, when the resolved stop reason is `ToolUse`, scan the snapshot's tool blocks for a non-empty `partial_json` (and the grammar `custom_input` equivalent once `PROV-101` lands) and route that to the error path with pi's message verbatim. Note the ordering: upstream raises this *after* the terminal-event check, so a truncated stream still reports the truncation, not the unfinished call.

**Verify** — decode test feeding `response.output_item.added` for a function call plus `response.function_call_arguments.delta` and then a terminal `response.completed` with no `output_item.done`: the stream ends in an error whose text is `OpenAI Responses stream completed with an unfinished tool call: <name> (<id>)`, and no tool call reaches the message. Red at HEAD, where the call is emitted with its partial arguments.

## PROV-117 — A loopback bind failure aborts `/login` instead of falling back to manual paste

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-02

> **CORRECTED 2026-10-03:** the Codex half of this row is wrong. cyrup's Codex flow already degrades: `auth/oauth/openai_codex.rs:844-855` maps `OAuthError::Listen` to `Ok(None)`, and the module doc item 3 (`:63-67`) records it, matching `openai-codex.ts:361-370`. Only the Anthropic flow is a gap: `auth/oauth/anthropic.rs:570-576` starts the `CallbackServer` with `.await?` (`:570-576`; the comment at `:564-565` says there is no manual-paste fallback). So "Codex has the same shape", the impact claim that `/login openai-codex` fails outright, and "one test per flow" are false; the fix is Anthropic only, with one test. **v1.0.1 carve-out:** pi v1.0.1 (`eeac84ca9`) makes the ChatGPT flow (`auth/oauth/openai-chatgpt.ts:241-248`) hard-fail on `EADDRINUSE` ("Port 1455 is in use … Cancel that login and try again") and rethrow other bind errors; Anthropic (`anthropic.ts:140-148`) and Codex (`openai-codex.ts:362-370`) still degrade to paste. The degraded path therefore must NOT be applied to the ChatGPT flow (see `PROV-118`). Recommended re-rating (not applied): medium→low (an edge case, Anthropic only).

**upstream** — `4df157433` ("share OAuth callback server and sign-in page") extracts `auth/oauth/callback-server.ts` (183 lines) and converts four flows to it. Two of them now tolerate a listener that never comes up: `auth/oauth/anthropic.ts:140-148` and `auth/oauth/openai-codex.ts:362-370` both end the `startOAuthCallbackServer({…})` call with `.catch(() => undefined)`, and `waitForCallbackOrManualInput` (`auth/oauth/callback-server.ts:155-184`) takes `callback: OAuthCallbackServer<T> | undefined` and documents the degraded path: "Without a callback server only the manual prompt is used." `openrouter.ts:116` and `radius.ts:153` keep the hard failure.

**cyrup** — `crates/cyrup-provider/src/auth/oauth/anthropic.rs` `run_login` propagates the start error:

```rust
// `:231` — the verifier doubles as the OAuth state. The port is fixed and pre-registered,
// so a second concurrent login surfaces as `OAuthError::Listen`, which is upstream's
// `server.on("error", reject)` (`:150-152`); there is no manual-paste fallback for it.
let server = CallbackServer::start(config, AnthropicCallbackHandler { … }).await?;
```

That comment was a correct reading of v0.87.1 and is now stale. The Codex flow in `auth/oauth/openai_codex.rs` has the same shape.

**Impact** — on any host where port 53692 (Anthropic) or 1455 (Codex) is already bound, or where a sandbox refuses the loopback listen, `/login anthropic` and `/login openai-codex` fail outright in cyrup. pi 1.0 shows the authorize URL and the paste prompt, and the login completes. cyrup already has every piece needed for the degraded path — the manual-code prompt, the paste parser, and the redirect-URL validator are all present and tested (`login_completes_via_manual_paste`, `login_accepts_a_pasted_redirect_url_with_the_matching_state`) — they are simply unreachable when the listener does not start.

**Fix** — make the two flows tolerate `CallbackServer::start` failing: hold `Option<CallbackServer>`, skip the `select!` when it is `None` and await the paste prompt alone, and keep the `redirect_uri` the prompt advertises as the constant rather than reading it off the server. Leave openrouter and radius alone — upstream deliberately did. The shared seam is worth extracting at the same time (cyrup's `CallbackServer` already is one), but the behaviour change is the item.

**Verify** — a test that binds the fixed port itself, then runs `login` with a scripted interaction supplying a pasted redirect URL, and asserts the credential is returned. Red at HEAD with `OAuthError::Listen`. One test per flow.

## PROV-118 — "Sign in with ChatGPT" on the `openai` provider is unported

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed · **Filed** 2026-10-02

> **CORRECTED 2026-10-03:** scope is understated, and v1.0.1 changed the file. (1) `isChatGPTSignIn` (`api/openai-responses.ts:40` @v1.0.1) is not only the usage hint: a ChatGPT sign-in token sent to `https://api.openai.com/v1` also makes the request omit `prompt_cache_retention`, `prompt_cache_options`, `max_output_tokens` and `temperature` (`:329-345`, `omitUnsupportedFields`); the row's text mentions none of that. (2) pi v1.0.1 (`eeac84ca9`) changes `auth/oauth/openai-chatgpt.ts` (309 lines at v1.0.1, 310 at v1.0.0): `startCallbackServer` now fails fast on `EADDRINUSE` with "Port 1455 is in use, probably by an unfinished login in another pi session or by the Codex CLI. Cancel that login and try again." (`:241-248`) and the paste fallback is dropped for this flow. The port is shared with legacy Codex login, so the two logins collide. This is the opposite of `PROV-117`'s degrade-to-paste. (3) "land (a) with `PROV-117`" is unsupported: `PROV-117` has no options-parameter work. cyrup has no `openai-chatgpt` code (verified: no hit under `crates/`). Severity and effort are roughly right.

**upstream** — `02eed88fd` ("add alternative sign in for the openai provider"). pi v1.0.0 `providers/openai.ts:12-19` gives the plain `openai` provider an OAuth entry beside its api key:

```ts
oauth: lazyOAuth({ name: "OpenAI (ChatGPT subscription)", isSubscription: true, loginLabel: "Sign in with ChatGPT", load: loadOpenAIChatGPTOAuth }),
```

The flow is the new `auth/oauth/openai-chatgpt.ts` (310 lines): a public PKCE client with **dynamic client registration** — every login sends `client_id=dynamic_agent_client` and reads the issued client id back out of the callback's `client_id` parameter — against `https://auth.openai.com/api/accounts/authorize` and `…/oauth/token`, `resource=https://api.openai.com/v1`, scope `openid profile email offline_access resource.invoke chatgpt.tokens.use.direct`, a fixed callback on `127.0.0.1:1455/auth/callback`, a three-minute `EXPIRY_MARGIN_MS` on refresh, and a manual-paste branch that validates the pasted URL's origin and path. Four supporting changes land with it:

- `auth/types.ts:202-226` — a new `LoginOptions { getDeviceId?: () => string }` second parameter on `OAuthAuth.login`, for "the stable ID of this app installation, e.g. sent to OpenAI as its agent host ID", which the app creates on first use and must return identically thereafter.
- `utils/retry.ts:27` adds `subscription_sharing_usage_limit_exceeded` to the NON-retryable limit patterns, and `:99-100` add `subscription_sharing_usage_unavailable` / `subscription_sharing_user_unavailable` to the retryable set (filed separately as `DRIFT-060`, area 12, which owns that list).
- `api/openai-responses.ts:30-43`, `:222-229` — `isChatGPTSignIn` (provider `openai`, the real OpenAI base URL, and a credential that does not start with `sk-`) and a `Check your ChatGPT usage: https://chatgpt.com/settings/usage` line appended to the limit error.
- `providers/openai-codex.ts:10` renames the old Codex provider to `OpenAI Codex (legacy)`, which is how the `/login` picker distinguishes the two ChatGPT-backed options.

**cyrup** — `crates/cyrup-provider/src/providers/openai.rs` builds api-key auth only (`:17`, `:30` cite `envApiKeyAuth("OpenAI API key", …)`), `providers/builtin_oauth.rs:95` lists `"openai"` among the providers with no built-in OAuth, and `grep -rn 'openai-chatgpt\|openai_chatgpt\|subscription_sharing\|dynamic_agent_client' crates/` at HEAD is empty. `OAuthAuth::login` (`auth/mod.rs:118-131`) takes `&dyn AuthInteraction` and no options bag, so there is nowhere for a device id to arrive.

**Impact** — a ChatGPT Plus/Pro subscriber cannot reach `api.openai.com` through their subscription in cyrup; the only OpenAI subscription path is the legacy Codex provider against `chatgpt.com/backend-api`. No existing behaviour is wrong, which is why this is `medium` and not higher.

**Fix** — split it. (a) Widen `OAuthAuth::login` with an options parameter carrying `get_device_id`, and give `Models::login` a place to supply it; this is the cross-cutting half and should land first. (b) Port the flow as `auth/oauth/openai_chatgpt.rs` on cyrup's existing `CallbackServer` + `generate_pkce` + manual-paste primitives, including the dynamic-client-registration read-back and the refresh margin. (c) Register it on `providers/openai.rs` with `loginLabel`/`isSubscription`, and remove `"openai"` from `builtin_oauth.rs:95`'s no-OAuth list. (d) The `isChatGPTSignIn` usage hint on the Responses error path. (e) The Codex display-name rename. Land (a) with `PROV-117`, which also touches these flows.

**Verify** — per-part. (a) A `dyn OAuthAuth` test that the device-id closure reaches a flow that asks for one. (b) The full flow against a loopback stand-in for `auth.openai.com`: authorize URL contents, `client_id` read-back from the callback, token exchange, refresh with the margin applied, and the pasted-URL origin/path rejection. (c) `/login` lists "Sign in with ChatGPT" for `openai`. (d) A 429 body containing `subscription_sharing_usage_limit_exceeded` yields an error message ending in the usage URL.

## PROV-119 — Anthropic workload identity federation is unported

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed · **Filed** 2026-10-02

> **CORRECTED 2026-10-03:** the `assertRequestAuth` calls are at `api/anthropic-messages.ts:615` and `:936` @v1.0.0 (`:613` and `:934` @v1.0.1), and `getAnthropicFederation` at `:351` (`:349` @v1.0.1); the `:614`/`:935` cites are off by one. The row's own Fix names the SDK-less OIDC token exchange as "the real work" and depends on `PROV-109` and `PROV-021`. Recommended re-rating (not applied): effort M→L.

**upstream** — `a9424cd43` ("Anthropic workload identity federation", #10242). `env-api-keys.ts:32-36` @v1.0.0 adds `ANTHROPIC_FEDERATION_RULE_ID`, `ANTHROPIC_ORGANIZATION_ID`, `ANTHROPIC_SERVICE_ACCOUNT_ID`, `ANTHROPIC_IDENTITY_TOKEN_FILE`, `ANTHROPIC_WORKSPACE_ID`. `providers/anthropic.ts:48-69` resolves them **last**, after all three key env vars, and returns them as provider config rather than auth — `return { auth: {}, env: federation, source: "workload identity federation" }` — requiring the first three and treating the last two as optional. `api/anthropic-messages.ts:351-379` `getAnthropicFederation` turns that `env` into an SDK `config` with `authentication: { type: "oidc_federation", identity_token: { source: "file", path } }`, but only for `model.provider === "anthropic"` and only when `hasRequestAuth` is false; `:324-326` splits `hasRequestAuth` out of the old `assertRequestAuth` so the assertion can be skipped (`:614`, `:935`). Two further details: `PiAnthropic` (`:333-338`) overrides `_shouldResolveDefaultCredentials()` to `false` so the SDK's own credential chain never runs behind pi's resolver, and `:1054-1068` caches one client per `[baseUrl, federation]` key and clones it with `withOptions()` per request so the SDK's federated-token cache survives pi's per-request client construction.

**cyrup** — `grep -rn 'ANTHROPIC_FEDERATION\|ANTHROPIC_ORGANIZATION_ID\|ANTHROPIC_IDENTITY_TOKEN' crates/ --include='*.rs'` at HEAD is empty, and `grep -rn -i 'federation' docs/gap-analysis/` is empty. `env_api_keys.rs:39` maps anthropic to `["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]` (the `ANTHROPIC_AUTH_TOKEN` omission is `PROV-021`, still open), and `api/anthropic_messages/headers.rs:81-…` `build_headers` has no third auth branch.

**Impact** — a cyrup run in a CI or Kubernetes workload that authenticates to Anthropic by OIDC federation instead of a key has no path: the five env vars are ignored and the request fails `No API key for provider: anthropic`.

**Fix** — two halves, and the second is the real work. (a) Add the five env constants and the federation arm to anthropic's api-key auth, resolved last and returning the ids in `env` — note this depends on the auth resolution's `env` actually reaching `request_options.env`, which is `PROV-109`, still open; schedule after it. (b) cyrup does not use the Anthropic SDK, so there is no `config` to hand off: the OIDC token-file read, the exchange against Anthropic's federation endpoint, the short-lived access token and its refresh all have to be implemented, plus the per-`[base_url, config]` token cache that `:1049-1067` exists to provide. `PiAnthropic`'s `_shouldResolveDefaultCredentials` override is **not applicable** — cyrup has no SDK credential chain to suppress.

**Verify** — resolution order test: with a key present the federation arm is not taken; with only the three required ids it is, and they arrive in `env`. Wire test against a loopback federation endpoint: one exchange, the access token on the request, a second request inside the token's lifetime reusing the cached token, and a third after expiry re-exchanging.

## PROV-120 — The Anthropic copy-code login method and its selector are unported

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed · **Filed** 2026-10-02

> **CLOSED 2026-10-08.** The fix below landed as written, in `crates/cyrup-provider/src/auth/oauth/anthropic.rs`: the constants, the selector at the top of `OAuthAuth::login`, and `login_copy_code`, which starts no listener. One decision was recorded: the `Missing OAuth state` check is not carried over, because upstream has none in either flow (`parsed.state ?? verifier`; the browser-flow check was dropped in `4df157433`, v0.99.0). `copy_code_login_defaults_state_like_upstream` pins this. The browser flow's leftover copy of the check was removed in the same pass and is recorded as `PROV-135`; its leftover v0.83.0 callback handler, found in review, is `PROV-137`. **Cite drift:** pi is now v1.1.0+, and line numbers moved but the content has not changed since v1.0.0. The constants are now `anthropic.ts:23-25` (were `:21-23`), `loginAnthropicCopyCode` is `:200-235` (was `:191-226`), and the selector is `:282-299` (was `:273-290`). The only upstream change to this file since v1.0.0, `8d8ae2fc2` (v1.1.0, the browser callback falls back to a free port), touches `loginAnthropic` plus a two-line comment above `CALLBACK_PORT` (which is what shifted the constants by two lines); it was ported in the same pass and is recorded as `PROV-136`. The TUI side of a headless login was audited end to end at the same time; the defects it found are `TUI-145`, `TUI-167`, `TUI-168` and `TUI-169` (area 07). Tests are listed in the ledger row.

**upstream** — `7a11fe1c7` ("add copy code login method to Anthropic OAuth", #10194). `auth/oauth/anthropic.ts:273-290` @v1.0.0 turns `login` into a chooser:

```ts
const method = await interaction.prompt({ type: "select", message: "Select Anthropic login method:", options: [
    { id: "browser", label: "Browser login (default)" },
    { id: "copy_code", label: "Copy code login (headless)" },
] });
```

and `:191-226` adds `loginAnthropicCopyCode`: the same PKCE authorize URL but with `redirect_uri = COPY_CODE_REDIRECT_URI` (`:21`, `https://platform.claude.com/oauth/code/callback`), no listener at all, a `manual_code` prompt placeholdered `code#state`, and the exchange posted against that same redirect URI. An unknown method id throws.

**cyrup** — `crates/cyrup-provider/src/auth/oauth/anthropic.rs:705` `login` is `self.run_login(interaction).await` with no selector, and `run_login` is the single browser-redirect flow. There is no copy-code constant: `grep -n 'oauth/code/callback' crates/` is empty. The primitives are all present and used elsewhere — `AuthPromptKind::Select` with `AuthSelectOption` (`auth/oauth/interaction.rs:18`, `:45`, `:69`), the TUI arm that renders it (`cyrup-tui/src/login_dialog.rs:746`), and `parse_authorization_input`'s `code#state` handling inside this very module.

**Impact** — on a host with no browser and no reachable loopback (a container, a bare SSH session), pi 1.0 offers a flow that needs neither; cyrup's only Anthropic path needs one of the two. Note the overlap with `PROV-117`: that row makes the *existing* flow survive a bind failure, this one adds the flow upstream prefers for that case. Both are worth having, and they should be scheduled together since they edit the same function.

**Fix** — add the two method constants and the copy-code redirect URI; put the `Select` prompt at the top of `login` with upstream's ids, labels and order; implement `login_copy_code` as the authorize-URL notify plus one `manual_code` prompt plus the exchange against the copy-code redirect URI; reject an unknown method id with upstream's message. Reuse `parse_authorization_input` and the existing state-mismatch check unchanged.

**Verify** — `ScriptedInteraction` test choosing `copy_code`, pasting `code#state`, and asserting the exchange body's `redirect_uri` is the copy-code URL and no listener was started; a second choosing `browser` that still reaches the existing redirect flow; a third asserting an unknown id errors. All red at HEAD (no selector prompt is issued, so the scripted answer is never consumed).

## PROV-121 — Anthropic strict tool use has no provider rejected-keyword check

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-02

> **CLOSED 2026-10-08** (checked against pi ce950d78f; see the table row). The **cyrup** paragraph below describes the state when this was filed and is no longer true: the predicate is threaded through `make_strict_json_schema` and `resolve_json_schema_strict_sampling`, and `convert_tools` passes `is_anthropic_strict_unsupported_keyword`. At ce950d78f the upstream lines are `constrained-sampling.ts:13`, `:56`, `:65-71`, `:127-135`, `:225-235` and `anthropic-messages.ts:1541-1574` (applied `:1586`), unchanged from v1.0.1. The `cyrup-ext` call site is now `wrapper.rs:705` (was `:530`).

> **CORRECTED 2026-10-03:** v1.0.1 line shift: the predicate is `api/anthropic-messages.ts:1569` and is applied at `:1586` (was `:1576`/`:1593` @v1.0.0); the wire change is otherwise identical. Production callers of `resolve_json_schema_strict_sampling` (`utils/constrained_sampling.rs:402`) are the seven provider converters plus `cyrup-ext/src/wrapper.rs:530`, so the signature change touches all of them.

**upstream** — `295cc72b0` ("send Anthropic tools non-strict when schema has rejected keywords"). `api/constrained-sampling.ts:13` @v1.0.0 adds `export type UnsupportedStrictSchemaKeywordCheck = (key: string, value: unknown) => boolean;` and threads an optional predicate through `makeJsonSchemaNodeStrict` (`:56`, applied `:65-71`, recursed at `:81`, `:89`, `:117`), `makeStrictJsonSchema` (`:129`) and `resolveJsonSchemaStrictSampling` (`:228`). `api/anthropic-messages.ts:1550-1583` supplies the Anthropic predicate: eleven outright-rejected keywords (`minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf`, `maxItems`, `uniqueItems`, `minContains`, `maxContains`, `minProperties`, `maxProperties`), `minItems` rejected unless it is `0` or `1`, and `format` rejected unless its value is one of ten allowed string formats. `:1593` passes it in `convertTools`. The failure mode in the comment: these keywords make Anthropic "reject the whole request" with a 400.

**cyrup** — `crates/cyrup-provider/src/utils/constrained_sampling.rs:402` is `resolve_json_schema_strict_sampling(tool: &ToolDef, supports_strict_mode: bool)` — two arguments, no hook — and all eight call sites pass two (`api/anthropic_messages/tools.rs:32`, `openai_completions/tools.rs:70`, `openai_responses/tools.rs:45`, `bedrock_converse_stream/convert.rs:347`, `google_generative_ai/tools.rs:30` and `:77`, `mistral_conversations/tools.rs:38`, `cyrup-ext/src/wrapper.rs:530`). `grep -rn 'UnsupportedStrictSchemaKeyword' crates/` is empty.

**Impact** — nothing today, and the body should say so plainly rather than overstate it. The trigger needs a tool with `constrainedSampling.type == "json_schema"` *and* a model with `supports_strict_tools`. cyrup has sixteen Anthropic models with `supportsStrictTools: true` in `providers/catalog/anthropic.json`, but only four built-in tools opt into strict sampling at all — `read`, `write`, `edit` and the shell engine, each returning the single shared `cyrup_core::prefer_strict_tool_sampling()` static — and none of their parameter schemas carries any rejected keyword (`grep -n '"minimum"\|"maximum"\|"maxItems"\|"format"\|"uniqueItems"\|"multipleOf"' crates/cyrup-tools/src/tools/{read,write,edit,bash}.rs` is empty). MCP and extension tools, which can carry arbitrary schemas, do not declare constrained sampling (`grep -rn 'constrained_sampling' crates/cyrup-mcp/src/` is empty). So the hole is latent: it opens the first time a tool with a server-supplied schema opts into strict sampling, and then it is a hard 400 for the whole request rather than a degraded single tool — which is exactly why upstream's `prefer` fallback matters.

**Fix** — add the predicate parameter to `make_json_schema_node_strict`, `make_strict_json_schema` and `resolve_json_schema_strict_sampling` (take `Option<&dyn Fn(&str, &serde_json::Value) -> bool>`), thread `None` at the seven non-Anthropic sites, and supply the Anthropic predicate with the three rules above at `api/anthropic_messages/tools.rs:32`. Keep upstream's error-kind behaviour: a rejected keyword is an `UnsupportedStrictJsonSchema` error, so `prefer` degrades to non-strict and `require` fails the request with the reason.

**Verify** — in `utils::constrained_sampling::tests`, a `prefer` tool whose schema has `{"type":"integer","minimum":1}` resolves to `None` under the Anthropic predicate and to `Some(true)` under `None`; `minItems: 1` is accepted and `minItems: 2` is not; `format: "date"` is accepted and `format: "regex"` is not; the same tool under `require` errors with the keyword in the message. Red at HEAD because the parameter does not compile.

## PROV-122 — One-hour cache writes reported in `message_delta` are priced at the five-minute rate

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-02

> **CLOSED 2026-10-08.** (on `claude/provider-drift-sweep`, checked against pi ce950d78f) `apply_message_delta_usage` reads the delta's `cache_creation.ephemeral_1h_input_tokens` under upstream's `!= null` guard (pi `api/anthropic-messages.ts:841-847` @ce950d78f). Evidence is in the row.

> **CORRECTED 2026-10-03:** v1.0.1 line shift: `api/anthropic-messages.ts:684` (`message_start`) and `:843-846` (`message_delta`), was `:686` and `:845-849` @v1.0.0. cyrup's `apply_message_delta_usage` is at `api/anthropic_messages/usage.rs:31`, and `apply_message_start_usage` (`:7`) already reads `ephemeral_1h_input_tokens` (`:21-24`).

**upstream** — `667fc3dd3` ("price Vercel AI Gateway 1-hour cache writes correctly"). pi reads the TTL breakdown in both usage positions at v1.0.0: `api/anthropic-messages.ts:686` on `message_start` (`event.message.usage.cache_creation?.ephemeral_1h_input_tokens || 0`) and `:843-849` on `message_delta`, with the reason in the comment — "Vercel AI Gateway includes the TTL breakdown in deltas, though the SDK only types it on message_start" — cast through an inline type widening because the SDK type lacks the field. `calculateCost` runs immediately after, at `:857-858`.

**cyrup** — `crates/cyrup-provider/src/api/anthropic_messages/usage.rs:7-27` `apply_message_start_usage` reads it (`:21-26`, `cache_creation.ephemeral_1h_input_tokens`, defaulting to `Some(0)`), but `:31-53` `apply_message_delta_usage` updates `input`, `output`, `cache_read`, `cache_write` and `reasoning` and never touches `cache_write_1h` — so a one-hour write that only the delta reports stays at whatever `message_start` seeded, which for a relay that omits it there is `0`.

**Impact** — the one-hour portion is then billed through `cache_write` at the short-cache rate. `PROV-081` closed exactly this shape on the Bedrock decoder (`cacheDetails` with `ttl: "1h"`) and was `low`; this row is its Anthropic-decoder twin and the ledger already called it "a `PROV-081`-class pricing gap" as lead (c) at `01-cyrup-core-and-provider.md:636-638`, which this row discharges. Reach is narrow: cyrup has no `vercel-ai-gateway` provider module (`ls crates/cyrup-provider/src/providers/` has no entry for it), so today the delta path is only taken by Anthropic-compatible relays arriving through the live catalog.

**Fix** — in `apply_message_delta_usage`, add the `cache_creation.ephemeral_1h_input_tokens` read under upstream's `!= null` guard, i.e. assign only when the key is present, so an absent breakdown does not clobber the `message_start` value. Unlike the `message_start` path it must not default to zero.

**Verify** — `api::anthropic_messages::tests::usage` test: a `message_start` with no `cache_creation` followed by a `message_delta` carrying `cache_creation.ephemeral_1h_input_tokens: 1000` yields `cache_write_1h == Some(1000)` and the cost computed with the one-hour rate; a delta with no `cache_creation` leaves the seeded value intact. Red at HEAD on the first case.

## PROV-123 — The `model.sampling_params` merge moved into the adapters

> **CLOSED 2026-10-10** (checked against pi f1b2e77f5). The merge is resolved at both sites, as pi does since `76dfb88f6`; the production change landed in #215 (`52b1aa33`). The **cyrup** and **Fix** paragraphs below describe the state when this was filed and are no longer true. Evidence, per Verify clause, is in the table row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-02

> **CORRECTED 2026-10-03:** citation fix: the three `Object.assign(params, model.samplingParams, options?.samplingParams)` lines are `api/openai-completions.ts:999`, `api/openai-responses.ts:382` and `api/azure-openai-responses.ts:348` (same at v1.0.0 and v1.0.1). `openai-responses.ts:363` is the v0.87.1 line and `azure-openai-responses.ts:347` is off by one. `api/simple-options.ts:29` is correct. cyrup's merge is `utils/simple_options.rs:57-105`.

**upstream** — `c01f687e5` ("apply model samplingParams in direct stream()/complete() calls", closes #9506). The commit message states the defect: "Model-level samplingParams were only merged by streamSimple(). Direct stream()/complete() calls, such as extensions using modelRegistry.complete(), dropped them." The fix **moves** the merge, it does not duplicate it: `api/simple-options.ts:29` now passes `samplingParams: options?.samplingParams` straight through, and the three OpenAI-compatible `buildParams` tails become `Object.assign(params, model.samplingParams, options?.samplingParams)` (`api/openai-completions.ts:999`, `api/openai-responses.ts:363`, `api/azure-openai-responses.ts:347`), with per-request keys still last.

**cyrup** — the v0.87.1 arrangement, and documented as deliberate. `utils/simple_options.rs:64-78` `merge_sampling_params` does the merge and `:103-106` places it in `build_base_options`; `api/openai_completions/params.rs:240-246` applies only `opts.sampling_params` with the comment "The merge with `Model.sampling_params` already happened in `build_base_options`", and `api/openai_responses/params.rs:268-270` and `api/azure_openai_responses.rs:435-436` say the same. `crates/cyrup-provider/src/tests/sampling_params.rs:141` is a test whose doc names "the reason the merge lives in `build_base_options` rather than in each adapter" — upstream has now reversed that reason.

**Impact** — a call that reaches an `ApiImpl::stream` with a hand-built `StreamOptions`, bypassing `build_base_options`, drops the model's sampling defaults. `build_base_options` is public (`lib.rs:202`), so the bypass is reachable by construction; cyrup has no extension-facing `complete`/`stream` surface today (`grep -rn 'fn complete' crates/cyrup-ext/src/` is empty), which is why this is `low` rather than a live defect. It will become one the moment that surface lands, and it is cheaper to move the merge now than to discover it then.

**Fix** — mirror the move exactly: have `build_base_options` copy `options.base.sampling_params` through unchanged, and make `apply_sampling_params` take the model so it can apply `model.sampling_params` then the request's, per key, request last. `merge_sampling_params`' `None`-vs-empty-map distinction stops mattering once the merge is an `Object.assign` equivalent at the point of use, but check the three adapters' empty-map behaviour against upstream before deleting it.

**Verify** — add to `crates/cyrup-provider/src/tests/sampling_params.rs` a case that calls each of the three adapters' param builders with a `StreamOptions::default()` (no `sampling_params`) and a model carrying `{"top_p": 0.5}`, asserting `top_p` is in the body. Red at HEAD. The existing cases that go through `build_base_options` must stay green, and the test at `:141` needs its doc rewritten rather than deleted — it records why the old arrangement existed.

## PROV-124 — An unparseable `retry-after` retries immediately instead of backing off

> **CLOSED 2026-10-08** (checked against pi ce950d78f). Both header branches of `retry_delay_ms` are now gated on finiteness, as pi does at `utils/provider-retry.ts:57` and `:64`. The **cyrup** paragraph below describes the state when this was filed and is no longer true. The Verify tests are in `utils/provider_retry.rs`. Evidence is in the table row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-02

**upstream** — `2bbfcca43` ("use exponential backoff when Retry-After is unparseable"). `utils/provider-retry.ts` @v1.0.0 tightens both header branches of `getRetryDelayMs`: `:55` becomes `if (Number.isFinite(value))` for `retry-after-ms` (was `!Number.isNaN`, which let `Infinity` through), and `:62` wraps the `retry-after` branch in `if (Number.isFinite(delayMs))` so a value that is neither a number nor a parseable HTTP-date — `delayMs` is then `NaN` from `Date.parse(…) - Date.now()` — falls through to the exponential ladder below instead of being returned.

**cyrup** — `crates/cyrup-provider/src/utils/provider_retry.rs` `retry_delay_ms` reproduces the old behaviour on purpose, and says so:

```rust
// Pi's unparseable-date branch yields NaN, which `validateServerRetryDelayMs` passes
// through (`NaN > max` is false) and `abortableSleep`'s `Math.max(0, NaN)` then floors
// to an immediate retry. `0.0` reproduces that without importing NaN into the ladder.
None => parse_http_date_ms(raw).map(|at| (at - now_ms()) as f64).unwrap_or(0.0),
```

and pins it with `an_unparseable_retry_after_retries_immediately` (`:413`).

**Impact** — a provider that answers a 429 with a malformed `Retry-After` gets `max_retries` immediate retries from cyrup, burning the whole retry budget in microseconds and hammering an endpoint that just asked to be left alone. pi 1.0 waits `0.5·2^n` seconds, capped at 8. No cyrup-reachable provider is known to send a malformed value, which is why this stays `low`.

**Fix** — return the exponential delay when the computed `delay_ms` is not finite, for both headers. In Rust that is: keep `js_parse_float` for `retry-after-ms` but reject non-finite results; and in the `retry-after` branch, fall through to the exponential tail when `parse_http_date_ms` returns `None` rather than substituting `0.0`. The existing comment should be replaced, not amended — it documents behaviour upstream has abandoned.

**Verify** — invert `an_unparseable_retry_after_retries_immediately` into `an_unparseable_retry_after_falls_back_to_exponential_backoff` and add a case for `retry-after-ms: Infinity`. Both red at HEAD.

## PROV-125 — The z.ai CN overflow message is not classified as a context overflow

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-02 · **CLOSED 2026-10-08** (checked against pi ce950d78f): upstream `/prompt exceeds max length/i` is still the second `OVERFLOW_PATTERNS` entry (`packages/ai/src/utils/overflow.ts:39` @ce950d78f) and the doc line at `:30` still names both z.ai bodies. Ported at `crates/cyrup-provider/src/utils/overflow.rs` (pattern immediately after `prompt (?:is )?too long`, order preserved for `PROV-S03`'s comparison; module doc extended). Verify: `utils::overflow::tests::zai_prompt_too_long_and_provider_gated_bodyless`.

**upstream** — `3dd803d7e` ("detect Z.AI CN endpoint context overflow errors"). `utils/overflow.ts:39` @v1.0.0 adds `/prompt exceeds max length/i, // z.ai CN endpoint token overflow` as the second entry of `OVERFLOW_PATTERNS`, and the module doc at `:30` records the CN endpoint's second body shape alongside the first: `{"code":"1261","message":"Prompt exceeds max length"}`.

**cyrup** — `crates/cyrup-provider/src/utils/overflow.rs:15-…` `OVERFLOW_PATTERNS` opens with `r"prompt (?:is )?too long"` (the widened form that closed `PROV-082` on 2026-09-27) and has no `exceeds max length` entry; `grep -rn 'exceeds max length' crates/` at HEAD is empty.

**Impact** — a context overflow from z.ai's CN endpoint is classified as an ordinary provider error, so auto-compaction never fires on it and the turn fails where it would otherwise recover. Same mechanism and same blast radius as `PROV-082`, which was `medium` when the pattern it added was the only one covering z.ai at all; this is a second endpoint's wording, hence `low`.

**Fix** — add the pattern in pi's position (immediately after the Anthropic/z.ai one) with pi's comment, and extend the module doc's z.ai line to name both bodies. `PROV-S03` byte-compared this list against upstream once; preserving order keeps that comparison meaningful.

**Verify** — extend `utils::overflow::tests::zai_prompt_too_long_and_provider_gated_bodyless` (or add a sibling) with a message of `{"code":"1261","message":"Prompt exceeds max length"}` asserted as an overflow. Red at HEAD.

## PROV-126 — Header merging is case-sensitive, so a differently-cased override emits a second header

> **CLOSED 2026-10-08** (checked against pi ce950d78f). `utils/headers.rs` ports `providerHeadersToRecord`, and the Google, Vertex and OpenRouter-images header builders use it, as the classifier already did. Anthropic stays case-sensitive, as upstream is. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-02

> **CORRECTED 2026-10-03:** the premise about Anthropic is false. `api/anthropic-messages.ts` contains no use of `providerHeadersToRecord` at v1.0.0 or v1.0.1; pi's Anthropic path still merges with `mergeHeaders` (`:293-301`), a plain `Object.assign`, which is case-sensitive, so cyrup's Anthropic `build_headers` (`api/anthropic_messages/headers.rs:189-211`) already matches upstream. The real callers of the case-insensitive helper (`utils/headers.ts:11-23`) are `google-generative-ai.ts:358`, `google-vertex.ts:399`, `bedrock-converse-stream.ts:258`, `pi-messages.ts:398`, `system-one-shared.ts:166`, `openrouter-images.ts:130` and `llama-cpp-classify.ts:236`, so the gap exists only in cyrup's non-Anthropic ports of those. The owning upstream commit is `a328aa89a` ("unify image and classifier model infrastructure"), not the one cited.

**upstream** — `4df157433` and its neighbours rewrite `utils/headers.ts:11-23` @v1.0.0 from a single-map copy into a variadic case-insensitive merge:

```ts
export function providerHeadersToRecord(...headerSources: (ProviderHeaders | undefined)[]) {
    const merged = new Map<string, [string, string]>();
    for (const source of headerSources)
        for (const [name, value] of Object.entries(source ?? {})) {
            const normalizedName = name.toLowerCase();
            merged.delete(normalizedName);
            if (value !== null) merged.set(normalizedName, [name, value]);
        }
    return merged.size > 0 ? Object.fromEntries(merged.values()) : undefined;
}
```

Three behaviours fall out, none of which the old object-spread `mergeClientHeaders` had: a later source overrides an earlier one **regardless of casing**; the surviving entry keeps the *last* source's spelling; and a later `null` **removes** an earlier source's header instead of merely failing to set it. `api/anthropic-messages.ts:1045-1051` now calls it with four sources, and `api/llama-cpp-classify.ts:236` with its own list.

**cyrup** — `HeaderMap` is `pub type HeaderMap = std::collections::BTreeMap<String, Option<String>>` (`crates/cyrup-provider/src/lib.rs:223`) — case-sensitive keys — and `api/anthropic_messages/headers.rs:189-211` applies each overlay with `headers.insert(name.clone(), value.clone())`, i.e. under the source's own spelling, in the order auth overlay → `model.headers` → Copilot dynamic → `opts.headers`. The order is right (it is what `PROV-028` and `PROV-095` established); the key normalization is not. The one place cyrup already has the new shape is the classifier api `EXT-027`'s closure ported: `api/llama_cpp_classify.rs:663` `provider_headers_to_record(sources: &[Option<&HeaderMap>])`.

**Impact** — a `model.headers` entry from the live catalog, or an `options.headers` entry from settings, spelled `X-Api-Key` or `Anthropic-Beta` rather than lowercase, does not replace cyrup's lowercase default: both go on the wire. For `x-api-key` that means the default key is still sent and the override silently has no effect; for `anthropic-beta` it means two conflicting beta lists. A `null` intended to suppress a default also misses when the casing differs. All cyrup's own defaults are lowercase, so the trigger is a differently-cased value from catalog or config, which is why this is `low` — but it fails silently, which is the part worth fixing.

**Fix** — give the four wire apis that merge provider headers one shared helper with the semantics above: normalize to lowercase for identity, keep the last writer's spelling for output, and let `None` remove. Generalize `api/llama_cpp_classify.rs:663`'s function into `utils/headers.rs` rather than writing a second one. Leave the merge *order* alone.

**Verify** — unit test on the shared helper for each of the three behaviours; plus an `api::anthropic_messages::tests::headers` case where `model.headers` carries `X-Api-Key` over a lowercase default and exactly one `x-api-key`-identity header with the override's value reaches the request. Red at HEAD (two headers).

## PROV-127 — `AssistantMessage.thinkingLevel` is unmodelled

> **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5) — field, serializer slot and loop stamp were already at HEAD (`05bcff1`); closed by adding the Verify tests (serializer bytes in `assistant.rs`, interop round-trip in `cyrup-test-support`) plus a loop-population test in `cyrup-agent`, all red-proved. Byte order of a pi-written line is not reproduced (pi appends the key last; see the correction below and the row).

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-02

> **CORRECTED 2026-10-03:** the key-order claim is wrong. `types.ts:555-557` @v1.0.1 declares `thinkingLevel?` between `providerThinkingLevel` and `diagnostics` in the TYPE, but the only producer is `packages/agent/src/agent-loop.ts:409`, `Object.assign(await response.result(), { thinkingLevel: config.reasoning ?? "off" })`, which appends the key at runtime after the keys the adapter already set, on the final result only. There is therefore no fixed JSONL slot between `providerThinkingLevel` and `diagnostics`, and the Fix's "serialize it between ..." and byte-order Verify are unsupported; where the key lands in a persisted message depends on the loop, which is cross-area with area 03/agent loop. The field's existence, severity and effort stand.

**upstream** — `types.ts:553-558` @v1.0.0 adds a second thinking field to `AssistantMessage`, immediately after `providerThinkingLevel` and before `diagnostics`:

```ts
/** Pi thinking level the agent loop requested for this response. Absent outside the agent loop and for legacy responses. */
thinkingLevel?: ModelThinkingLevel;
```

The pair is deliberate: `providerThinkingLevel` records the provider-native effort string actually used, `thinkingLevel` the pi level the loop asked for.

**cyrup** — `crates/cyrup-core/src/message/assistant.rs:54-69` declares `provider_thinking_level: Option<String>` then `diagnostics`, with nothing between; `grep -n 'thinking_level' crates/cyrup-core/src/message/assistant.rs` finds only `provider_thinking_level` (`:65`, `:166`, `:186`, `:246`, `:280`). The hand-written serializer's documented key order (`:152`) is `provider, model, responseModel?, responseId?, providerThinkingLevel?, diagnostics?, usage, stopReason, deferred?, …`, and `len` (`:166-167`) counts the two optional fields it knows about.

**Impact** — the record cannot say what level was requested, only what the provider was told, so a transcript cannot distinguish "the user asked for `high` and the model mapped it to `high`" from "the user asked for `max` and the catalog clamped it". It also puts cyrup's JSONL one key short of pi's for any message pi would stamp — the same class of byte-order divergence `PROV-020` tracks for `toolResult`. No current cyrup behaviour is wrong; the field has no reader.

**Fix** — add `thinking_level: Option<ModelThinkingLevel>` in pi's slot, serialize it as `thinkingLevel` between `providerThinkingLevel` and `diagnostics`, skipped when unset, and widen `len` by one. The producer is the agent loop, not the provider — cyrup's loop already resolves a level per request (the ladder at `cyrup-session-svc/src/session/model.rs`, per `TUI-105`'s closure note), so stamping it is a one-line addition there, and area 03's owner should confirm nothing in its transcript reader assumes the old key set.

**Verify** — serializer test in `assistant.rs`: with `thinking_level: None` the key is absent and the JSON is byte-identical to today; with `Some(Max)` the key appears as `"thinkingLevel":"max"` between `providerThinkingLevel` and `diagnostics`. Round-trip through `cyrup-test-support`'s interop fixtures as `PROV-012` does for `rawStopReason`.

## PROV-128 — Image models are a parallel registry pi v1.0.0 deleted; `AnyModel` has no `Image` variant

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** confirmed · **Filed** 2026-10-02

> **CORRECTED 2026-10-03:** the structure is confirmed (`types.ts:1158-1176` @v1.0.1, `providers/openrouter.ts:33`, `classifier.rs:204-207`), but the "silent per-row drop" impact is not occurring today. Pi's client asks the catalog endpoint for `?types=chat,image,classifier` (`packages/coding-agent/src/core/remote-catalog-provider.ts:22`, `:102`, new in v1.0.0); cyrup's `remote_catalog.rs:637-641` builds the URL with no `types` parameter. Checked live on 2026-10-03: `https://pi.dev/api/models/providers/openrouter` returns 397 rows, all chat, and with `?types=chat,image,classifier` returns 463 (397 chat, 57 image, 9 classifier). So image rows never reach cyrup and nothing is dropped; the missing `types` parameter is itself a prerequisite for step (1). The `providers/all.ts:188` cite for the deleted `builtinImages*` functions is wrong (those were at v0.87.1 `all.ts:145-157`). Recommended re-rating (not applied): medium→low (no behaviour is broken; effort L stands).

**upstream** — `a328aa89a` ("unify image and classifier models"), the commit post-tag lead (b) at
`01-cyrup-core-and-provider.md:637` named. v1.0.0 collapses what were three registries into one:

- `types.ts:1096-1176` splits `Model` into a shared `BaseModel<TApi>` (`id`, `name`, `api`, `provider`,
  `baseUrl`, `input`, `inputLimits?`, `cost`, `headers?`) plus three siblings that carry a `type`
  discriminant: `Model<Api>` with `type?: "chat"`, `ImageModel<ImageApi>` with `type: "image"`, and
  `ClassifierModel<ClassifierApi>` with `type: "classifier"`. `ModelTypeMap`, `ModelType` and
  `AnyModel` (`:1163-1176`) are the union, and `KnownImagesApi`/`ImagesProviderId` are renamed
  `KnownImageApi`/dropped in favour of the ordinary `ProviderId` (`:31-35`, `:84`, `:622-623`).
- `models.ts` gives one `Provider` all three operations — `getAllModels()` beside `getModels()`, and
  `images` / `classifiers` dispatch maps on `createProvider` — with `KNOWN_MODEL_TYPES` and
  `withKnownModelTypes` (`:116-127`) dropping stored or remote entries whose type this build does not
  know. `utils/model-operations.ts` (new, 70 lines) holds `getModelType`, `isModelType`, the three
  `assert*Model` guards and `imageErrorResult`/`classifierErrorResult`.
- `providers/all.ts` adds `getBuiltinImageModel`, `getBuiltinClassifierModel`,
  `getBuiltinImageModels`, `getBuiltinClassifierModels` and `getAllBuiltinModels` over the generated
  `IMAGE_MODELS`/`CLASSIFIER_MODELS` records (`:55-132`), and **deletes** `builtinImagesProviders()`
  and `builtinImagesModels()` (`:188`, the last 14 lines of the file at v0.87.1).
- The whole separate images layer is gone: `git cat-file -e v1.0.0:packages/ai/src/images-models.ts`,
  `…/src/image-models.generated.ts` and `…/src/providers/openrouter-images.ts` all fail. The
  `openrouter-images` api survives only as a map entry on the ordinary openrouter provider:
  `providers/openrouter.ts:33` is `images: { "openrouter-images": openrouterImagesApi() }`, with its
  rows merged into that provider's one `models` array (`:23-27`).

**cyrup** — the classifier half of this unification is ported (`EXT-027`, the llama-cpp port) and the
image half is not, so cyrup sits exactly half-way:

- `crates/cyrup-provider/src/classifier.rs:43-46` is `enum ModelType { Chat, Classifier }` with
  `ModelType::ALL` a two-element array (`:50`), and `:204-207` is `enum AnyModel { Chat(Model), Classifier(ClassifierModel) }`.
  There is no `Image` variant in either, and no `ImageModel` type — `grep -rn 'struct ImageModel' crates/ --include='*.rs'`
  is empty; the only match is the old-shaped `images::ImagesModel` (`images/mod.rs:48`).
- `AnyModel`'s own `Deserialize` (`classifier.rs:282-299`) rejects any other type outright:
  `Some(other) => Err(D::Error::custom(format!("unknown model type: {other}")))`, so `"type":"image"`
  is an error, not a variant.
- Images keep their own parallel tree, which is what upstream deleted: `images/mod.rs` declares
  `ImagesProvider` (`:294`), `ImagesModels` (`:398`) and `create_images_models`, with its own
  `generate_images` collection method at `:508`; `providers/all.rs:377` `all_images_providers()` and
  `:383` `default_images_models()` are cited in-file to pi's now-deleted `builtinImagesProviders` /
  `builtinImagesModels` (`all.ts:125-131`); `providers/openrouter_images.rs:26` is the deleted
  `openrouterImagesProvider`.
- The unified `Provider`/collection surface has the classifier leg only: `provider.rs:118`
  `get_all_models()` defaults to wrapping the chat catalog as `AnyModel::Chat`, `:214` tests for
  `AnyModel::Classifier`, and `collection.rs:487` is `classify()`. `grep -n 'fn generate_images' crates/cyrup-provider/src/collection.rs`
  is empty — there is no images leg on the unified collection at all.

**Impact** — bounded today, and the bound is worth stating precisely rather than inflating. Images do
work in cyrup, through their own collection; nothing a user runs is broken. What is unreachable is the
unified surface: an image model cannot be registered on a `Provider`, cannot appear in
`get_all_models()`, cannot be named by `ModelType`, and cannot round-trip through the models store.
The reachable edge is the pi.dev overlay, which `PROV-099` and post-tag lead (a) establish is served
cyrup the **newest** catalog revision because of its `cyrup/<version>` User-Agent — so cyrup now
fetches a schema-v6 catalog in which openrouter's image and classifier rows sit in the same array as
its chat rows. That degrades gracefully rather than breaking, and by accident rather than by design:
`remote_catalog.rs:178-187` is a `filter_map` whose last step is `serde_json::from_value::<Model>(…).ok()`,
so a row missing `reasoning`/`contextWindow`/`maxTokens` is dropped per-row. Upstream's
`withKnownModelTypes` makes the same drop deliberately. So the cost is silent invisibility of the new
rows, not a failed parse — `medium`, not `high`.

**Fix** — `L`, and it should be split. Three separable steps, in order: (1) add `ModelType::Image` and
`AnyModel::Image(ImageModel)`, with `ImageModel` reshaped onto a shared base alongside `Model` and
`ClassifierModel`, and the `AnyModel` deserializer given the `"image"` arm; (2) hang `images` dispatch
on the unified `Provider` and add `generate_images` to the unified collection beside `classify`, then
re-express `providers/openrouter_images.rs` as an `images:` entry on the ordinary `openrouter`
provider; (3) retire `images::{ImagesProvider, ImagesModels, create_images_models}`,
`all_images_providers()` and `default_images_models()`, which have no upstream counterpart left.
Step (1) alone unblocks `PROV-129`. Note two adjacent open items this must not collide with: the
models-store shape (cyrup splits pi's single `models: AnyModel[]` into a chat `ModelsStoreEntry.models`
plus `read_classifier_models`/`write_classifier_models`, a divergence `models_store.rs:122-136`
documents and whose folding is already an **open `EXT-027` follow-up** — images would be a third
parallel channel on that same seam), and `PROV-104`, which owns the two System One classifier apis.

**Verify** — `AnyModel` deserializes a `"type":"image"` entry into `AnyModel::Image` and
`ModelType::ALL` has three members; an openrouter provider built from the embedded catalog lists its
image rows through `get_all_models()` and generates through the unified collection's `generate_images`;
and a remote catalog mixing chat, image and classifier rows for one provider yields all three rather
than only the chat rows — that last one is red at HEAD today and is the test that pins the real-world
edge. Keep a negative control for a type no build knows (`"type":"video"`), which must be dropped at
the store layer, not error.

## PROV-129 — `PROV-089`'s deadline has arrived: `openrouter-images.json`'s generation source no longer exists

**Kind** tooling · **Severity** low · **Effort** M · **Confidence** confirmed · **Filed** 2026-10-02

> **CORRECTED 2026-10-03:** the Fix's claim that the rows "arrive on the live endpoint cyrup already fetches" is wrong without a `types` parameter: `pi.dev/api/models/providers/openrouter` returns no image rows unless called with `?types=image` (checked 2026-10-03; see `PROV-128`), and neither `remote_catalog.rs:637` nor xtask's live URL shape (`https://pi.dev/api/models/providers/{file}`, asserted at `xtask/src/main.rs:1664-1667` and `:2042-2045`) carries it. v1.0.1 also adds a `?types=...` catalog revision flow (`packages/ai/scripts/hydrate-model-catalog.ts`, `scripts/update-model-catalog-pin.mjs`). The deletion itself is confirmed (`image-models.generated.ts` absent at v1.0.0 and v1.0.1; `xtask/src/main.rs:123` is `IMAGES_REV = "v0.87.1"`), and the interim comment and manifest-note correction stands; the real prerequisite for the full fix is the `types` parameter.

**upstream** — `PROV-089`'s closure (2026-09-28) wrote its own expiry condition into the ledger:
"**Deadline carried forward:** after the next pi tag (post-tag `a328aa89a` deletes
`image-models.generated.ts`) this catalog becomes `PROV-071`'s class, and `IMAGES_REV` must stay
`v0.87.1`." That tag is v1.0.0, and the condition is met:
`git cat-file -e v1.0.0:packages/ai/src/image-models.generated.ts` fails. The rows now live in
`IMAGE_MODELS` inside `models.generated.ts` (`providers/all.ts:1` imports
`CLASSIFIER_MODELS, IMAGE_MODELS, MODELS` from it), which is the gitignored generated-data file
`PROV-071` established is recoverable from no revision — `packages/ai/src/providers/data/` is empty in
`git ls-tree` at every tag. Upstream reaches them through `getBuiltinImageModels(provider)`
(`providers/all.ts:112-119`), keyed by the ordinary provider id `openrouter`, not by a separate
`openrouter-images` provider.

**cyrup** — `xtask/src/main.rs:123` is still `const IMAGES_REV: &str = "v0.87.1"`, and `:135-144`
keeps `openrouter-images` as the single remaining entry in `CATALOGS` (the pinned, non-live path)
after `PROV-071` moved the other 38 to `LIVE_CATALOGS`. Its comment states the standing rationale —
"its rows are the `openrouter` sub-record of `packages/ai/src/image-models.generated.ts`, which is
still a data literal in git, and `pi.dev/api/models/providers/openrouter-images` is a 404 — there is
no live endpoint to move it to (PROV-065)" — and the first clause of that is now false. The same
claim is baked into the generated manifest note (`:1292-1303`) and asserted by
`xtask::tests::module_paths_default_to_the_provider_models_module` (`:1472`), which pins
`img.rev(..) == "v0.87.1"`.

**Impact** — no user-visible break: the pin is what `PROV-089` deliberately installed, the 55 rows it
produced are correct as of v0.87.1, and `gen-catalogs` still reproduces them. What has changed is that
the pin is now permanent rather than provisional. `openrouter-images.json` is the one catalog with no
route to a newer row, and the in-tree comment and manifest note now assert a reason that a reader can
check and find wrong — which is the part most likely to cost someone an hour. `low`, and `tooling`
rather than `upstream-drift`, because the defect is in the generator's story about itself.

**Fix** — the honest fix is to stop treating `openrouter-images` as a catalog of its own, which is
`PROV-128` step (1)'s dependency: once an image row can be represented, these 55 rows are
`openrouter`'s image rows and arrive on the live endpoint cyrup already fetches for that provider
(`pi.dev/api/models/providers/openrouter`), where they are currently dropped by
`remote_catalog.rs:186`. That removes the last `CATALOGS` entry, lets `IMAGES_REV` and the
`CatalogSpec.rev` override delete, and collapses the manifest's pinned/live split. Until `PROV-128`
lands, the minimum is to correct the comment and manifest note to say the source file is **deleted
upstream as of v1.0.0** and that `v0.87.1` is now a terminal pin, and to restate the
`module_paths_default_to_the_provider_models_module` assertion against that fact.

**Verify** — once `PROV-128` step (1) lands: `gen-catalogs` accounts for all 39 catalogs through
`LIVE_CATALOGS` with `CATALOGS` empty, `openrouter`'s live fetch yields its image rows as
`AnyModel::Image`, and `gen-catalogs --check` exits 0 with no file read at a self-pinned revision.
Interim: a test asserting the generator's own note names v1.0.0 as the tag that deleted the source,
so the next reader is not sent to a file that is not there.

## PROV-130 — Bedrock Claude replays do not send `thinking.block_binding`

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (static read; not run against Bedrock) · **Filed** 2026-10-03

> Filed 2026-10-03 from the post-pin triage (pi v1.0.1, `v1.0.0..v1.0.1`).

**upstream** — `69f0be6f0` ("drop stale thinking blocks on Bedrock Claude replays", closes #10324). `packages/ai/src/api/bedrock-converse-stream.ts` @v1.0.1: `supportsThinkingBlockBinding(model)` (`:792-806`) matches `opus-4-7`, `opus-4-8`, `opus-5`, `sonnet-5`, `fable-5` and its doc comment says Opus 4.6 and Sonnet 4.6 reject the field with "thinking.adaptive.block_binding: Extra inputs are not permitted". At `:1262-1282`, for Anthropic Claude models off GovCloud (`useBlockBinding = !isGovCloud && supportsThinkingBlockBinding(model)`), the adaptive branch adds `block_binding: { prefix_mismatch_behavior: "drop_block" }` to `thinking` and `anthropic_beta: [THINKING_BINDING_CONTROLS_BETA]` (`"thinking-binding-controls-2026-08-01"`, `:122`) beside `output_config`. The in-source reason: replayed signed thinking blocks are bound to the system prompt and tools they were created with, and Bedrock 400s on replay after either changes unless stale blocks are dropped, "matching the Anthropic provider".

**cyrup** — `crates/cyrup-provider/src/api/bedrock_converse_stream/params.rs:151-163` (the adaptive branch of `build_additional_model_request_fields`) inserts `type`, optionally `display`, and `output_config.effort`, and nothing else. `rg block_binding crates` hits only the Anthropic adapter (`api/anthropic_messages/params.rs:270`, `headers.rs:27-29`, its tests); `rg thinking-binding` likewise. The GovCloud predicate already exists (`api/bedrock_converse_stream/capabilities.rs:127`, `is_gov_cloud_bedrock_target`), as does `supports_adaptive_thinking` (`:40`).

**Impact** — on a Bedrock Claude model that supports adaptive thinking and accepts the field, a session whose system prompt or tool list changes between turns (`/reload`, extension-provided tools) replays signed thinking blocks that no longer match their prefix; per pi's own comment the Bedrock request is then rejected with a 400. Not reproduced here.

**Fix** — add `supports_thinking_block_binding(model)` to `capabilities.rs` beside `supports_adaptive_thinking` with the five-substring match over id and name, and in the adaptive branch of `build_additional_model_request_fields` add `block_binding` to `thinking` and the beta to `anthropic_beta` when `!is_gov_cloud_bedrock_target(..)` and the predicate holds. Do not add it to the budget (`enabled`) branch; upstream does not.

**Verify** — payload tests: `opus-4-8` carries `thinking.block_binding.prefix_mismatch_behavior == "drop_block"` and the beta; `sonnet-4-6` and `opus-4-6` carry neither; a GovCloud target carries neither.

## PROV-131 — Embedded catalogs predate pi 1.0.1's Cloudflare dashed-id and Bedrock pricing-tier fixes

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed for the embedded files and the upstream generator; the 404 and the rate effect are taken from pi's commit text and were not reproduced · **Filed** 2026-10-03

> Filed 2026-10-03 from the post-pin triage (pi v1.0.1, `v1.0.0..v1.0.1`).

**upstream** — two generator commits in `packages/ai/scripts/generate-models.ts` @v1.0.1. `c10bfb0d7` (Cloudflare AI Gateway, anthropic upstream, `:1966-1969`): "The /anthropic passthrough forwards the model ID to Anthropic unchanged. models.dev lists dotted versions (claude-opus-5.5), but Anthropic only accepts dashed IDs (claude-opus-5-5)", then `id = nativeId.replaceAll(".", "-")`. `4665fafb4` (Amazon Bedrock, `:1794-1795`): `cost: getModelsDevCost(m.cost)` with the comment "Includes models.dev pricing tiers, e.g. the long-context tier for OpenAI models (#10326)", replacing a four-field literal that dropped tiers (`getModelsDevCost` at `:1254`).

**cyrup** — `crates/cyrup-provider/src/providers/catalog/cloudflare-ai-gateway.json` has nine `claude-*` ids containing a dot, all `api: "anthropic-messages"`: `claude-fable-5.1` (`:43`), `claude-haiku-4.5` (`:83`), `claude-opus-4.5` (`:117`), `claude-opus-4.6` (`:151`), `claude-opus-4.7` (`:189`), `claude-opus-4.8` (`:229`), `claude-opus-5.5` (`:309`), `claude-sonnet-4.5` (`:349`), `claude-sonnet-4.6` (`:383`). `providers/catalog/amazon-bedrock.json` contains zero `"tiers"` objects, e.g. `openai.gpt-5.5` (`:3639-3654`), whereas `openai.json` carries ten (e.g. `gpt-5.4`, tier above 272000 tokens at `:906-913`). The runtime overlay (`remote_catalog.rs`) can mask the embedded rows only once pi.dev serves regenerated data; checked live on 2026-10-03, `pi.dev/api/models/providers/cloudflare-ai-gateway?types=chat` still lists dotted ids (e.g. `claude-opus-5.5`) and `.../amazon-bedrock?types=chat` lists `openai.gpt-5.5` without `tiers`, so the live overlay does not fix this yet.

**Impact** — per pi's commit, a Cloudflare AI Gateway request for a dotted Claude id is rejected by Anthropic (a 404 on the passthrough); cyrup would send the dotted id on all nine rows. Bedrock OpenAI rows carry no long-context tier, so cost above the tier threshold is computed at the base rate. Neither was reproduced.

**Fix** — run `cargo run -p xtask -- gen-catalogs` once pi.dev serves v1.0.1-generated data (the generator is a live fetch per `PROV-071`, not a pinned revision), review the diff. Until then the two files cannot be fixed by hand without diverging from the generator; if a stopgap is wanted, edit only the nine CF ids, and record it here.

**Verify** — no id in `cloudflare-ai-gateway.json` of `api: "anthropic-messages"` contains a `.`; Bedrock OpenAI rows that models.dev tiers carry `cost.tiers`; `gen-catalogs --check` exits 0.

## PROV-132 — Together DeepSeek V4 Pro: the hand-ported row carries the pre-rename id

> **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5). **pi at the pin:** `packages/ai/scripts/generate-models.ts:197` is `const TOGETHER_TOGGLE_REASONING_EFFORT_MODELS = new Set(["deepseek-ai/DeepSeek-V4-Pro-0813"]);`, which (`:572-573`, `:562`) gives that id `TOGETHER_DEEPSEEK_V4_THINKING_LEVEL_MAP` (`{ minimal: null, low: null, medium: null, high: "high", xhigh: null }`) and `TOGETHER_TOGGLE_REASONING_EFFORT_COMPAT`; `test/together-models.test.ts:61` looks it up by `-0813`. pi's Together catalog data (`src/providers/data/`) is gitignored and generated from pi.dev, so the served catalog is the row's source: `pi.dev/api/models/providers/together?types=chat`, fetched 2026-10-10, lists `deepseek-ai/DeepSeek-V4-Pro-0813` (name "DeepSeek V4 Pro 0813", cost 1.32/3.96/0.13, contextWindow 1048576, maxTokens 384000) and **no longer lists** `deepseek-ai/DeepSeek-V4-Pro`. `git grep DeepSeek-V4 f1b2e77f5 -- 'packages/*/src'` is empty: pi has no alias or migration for the old id. **Fix:** `crates/cyrup-provider/src/providers/together.rs` `together_models()` — the row is renamed to `-0813` with that name, cost, window and max tokens, same level map and compat; the old row is dropped (the Fix's condition, "drop the old row only when pi.dev stops listing it", is met). No other cyrup reference to the Together id existed (the `huggingface.json`/`baseten.json` rows are other providers, untouched). **Verify:** `full_catalog_ported_from_pi` — `find("deepseek-ai/DeepSeek-V4-Pro-0813")` returns a model whose `thinking_level_map` equals pi's map exactly (only `high` maps), `supports_reasoning_effort: true`, `thinking_format: Together`, and the old id is absent; `catalog_models_encode_reasoning_per_pi` builds its body (`reasoning.enabled` + `reasoning_effort: "high"`). **Red before:** with the source row reverted to HEAD both fail (`together.rs:470` old id present; `:602` `-0813` not found); file restored byte-identical (sha256 checked).

**Kind** stale-port · **Severity** ~~low~~ **CLOSED 2026-10-10** · **Effort** S · **Confidence** confirmed (ids); the retirement of the old id is pi's changelog claim · **Filed** 2026-10-03

> Filed 2026-10-03 from the post-pin triage (pi v1.0.1, `v1.0.0..v1.0.1`).

**upstream** — `28eaccb8e` ("update Together DeepSeek V4 Pro model ID", #10336). `packages/ai/scripts/generate-models.ts:201` @v1.0.1 is `new Set(["deepseek-ai/DeepSeek-V4-Pro-0813"])` (was `…/DeepSeek-V4-Pro`), and `test/together-models.test.ts:61` does `getModel("together", "deepseek-ai/DeepSeek-V4-Pro-0813")`. Pi's changelog (`packages/ai/CHANGELOG.md:21`, v1.0.1): "Fixed Together DeepSeek V4 Pro losing its thinking level controls after Together renamed it to `deepseek-ai/DeepSeek-V4-Pro-0813`".

**cyrup** — `crates/cyrup-provider/src/providers/together.rs:197-209` is the hand-ported row `deepseek-ai/DeepSeek-V4-Pro` with the level map `high -> "high"` and the other levels null and `together_compat(true, Some(ThinkingFormat::Together))`; the tests at `:449` (`find("deepseek-ai/DeepSeek-V4-Pro")`) and `:561` look it up by that id. There is no `-0813` row (`rg DeepSeek-V4-Pro-0813 crates` is empty). Checked live on 2026-10-03, `pi.dev/api/models/providers/together?types=chat` lists both `deepseek-ai/DeepSeek-V4-Pro` and `deepseek-ai/DeepSeek-V4-Pro-0813`, so the old id is not yet gone from pi's catalog.

**Impact** — a user selecting the renamed model gets no thinking-level controls, since the level map is attached to the old id only; if Together has stopped serving the old id (pi's changelog implies a rename, not verified here) the old row also fails.

**Fix** — add a `deepseek-ai/DeepSeek-V4-Pro-0813` row with the same level map and compat, and update the two tests to cover it. Drop the old row only when pi.dev stops listing it.

**Verify** — `find("deepseek-ai/DeepSeek-V4-Pro-0813")` returns a model whose thinking-level map allows only `high`.

## PROV-133 — Anthropic native mid-conversation tool changes (`PROV-083b`) are unported; pi 1.0.1 moved them to inline definitions

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (static read; not run) · **Filed** 2026-10-03

> Filed 2026-10-03 from the post-pin triage (pi v1.0.1, `v1.0.0..v1.0.1`). `PROV-083` is CLOSED for its predicates and `SystemMessage` shape; the adapter half it deferred as "`PROV-083b`" in code comments had no ledger row.

**upstream** — `b271b0a52` ("define Anthropic mid-conversation tools inline"). `packages/ai/src/api/anthropic-messages.ts` @v1.0.1: `INLINE_TOOLS_BETA = "inline-tools-2026-09-15"` (`:195`); `DEFERRED_TOOL_PLACEHOLDER` named `__pi_deferred_placeholder__` with `defer_loading: true` (`:205`), declared whenever native tool changes are in use so the hidden prompt scaffolding stays in the cached prefix; with native tool changes the request-level tool list is fixed ("Native tool changes keep the request-level tool list fixed and define every later tool by value in a `tool_addition` block, which also expresses same-name redefinitions", `:1137-1139`; initial tools plus the placeholder, `:1208-1210`). In the message loop (`:1335-1352`) a later system message becomes a `{role:"system", content:[...]}` whose blocks are `tool_removal` (`{type:"tool_reference", name}`, skipped when the name is also re-added) then `tool_addition` (`{type:"tool_definition", definition}`), held back until the next assistant message. `hasToolRedefinitions()` (`utils/transcript.ts:186`) is deprecated ("no built-in transport needs it anymore", CHANGELOG `:12`).

**cyrup** — `crates/cyrup-provider/src/api/anthropic_messages/compat.rs:83` (`supports_mid_convo_tool_changes`) and `:195` (`default_supports_mid_convo_tool_changes`) exist as predicates only; `api/anthropic_messages/messages.rs:58-60` skips `Message::System` with the comment "PROV-083b wires each adapter's own `resolve_transcript`"; `params.rs:115` builds the tool list from `ctx.tools` each request via `split_deferred_tools`. `rg 'tool_addition|tool_removal' crates --type rust` finds no emitter. The same "PROV-083b" comment sits in `openai_completions/convert.rs:54`, `openai_responses/convert.rs:86`, `bedrock_converse_stream/convert.rs:183`, `google_generative_ai/convert.rs:52`, `mistral_conversations/messages.rs:21` and `utils/deferred_tools.rs:125` (which says 083b retires that module); those non-Anthropic adapters need only the collapse half and are not part of this row's v1.0.1 delta.

**Impact** — no failure: any tool change rebuilds the request-level tool list and loses the cached prefix on that turn (pi measures a full cache miss without the placeholder scaffolding). Cost, not correctness.

**Fix** — wire `resolve_transcript` into the Anthropic adapter and emit the v1.0.1 shapes: fixed request-level list plus the placeholder, `tool_addition` blocks with inline `tool_definition`, `tool_removal` blocks, the `inline-tools-2026-09-15` beta header. Do not port the v1.0.0 `tool_reference` shape. File separate rows for the other adapters if their collapse half is found missing.

**Verify** — a run in which a tool is added, removed and redefined mid-run: assert the blocks emitted, the unchanged request-level `tools`, and the beta header.

> **NOTE 2026-10-09:** the closure must also flip `cyrup_provider::api::emits_native_tool_additions` (`crates/cyrup-provider/src/api/mod.rs:254`, a `const fn` that returns `false` for every api) to `true` for `anthropic-messages`, and change its pin test `tests::no_adapter_emits_native_tool_additions_yet` (`:269`) with it, as the function's doc comment asks. Subagents' `toolActivation: "auto"` reads that predicate, not the compat flags, to decide whether a mid-conversation tool change is cache-safe and so whether to offer the `subagents_enable` loader (`crates/cyrup-ext-subagents/src/extension/tool_activation.rs:105-114`; the `SUBA-153` closure in `09b`). Emitting `tool_addition` blocks without the flip would leave `auto` on the eager `subagent` tool. Add to Verify: `emits_native_tool_additions("anthropic-messages")` is `true`, and an `auto` session on that api starts with the loader.

## Findings filed 2026-10-08 — follow-ups to `PROV-117` and `PROV-120` (Anthropic login)

Upstream read at `tmp/pi` HEAD `ce950d78f` (after v1.1.0), with `git -C tmp/pi show <commit>` for the two
commits; cyrup read before and after the commit that closed them (the `PROV-120` commit on `ef8f867`). All four rows were filed and closed
in the same pass as `PROV-120` (`PROV-137` by its review, `PROV-138` by its second review), so that a headless Anthropic login matches
upstream end to end.

## PROV-135 — The Anthropic browser login rejects a pasted code whose state is empty

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-08

> **CLOSED 2026-10-08.** `run_login` no longer has the `Missing OAuth state` check. A pasted `code#` or `?code=C&state=` skips the mismatch check (an empty state is falsy), keeps `""` through `parsed.state ?? verifier`, and is exchanged with `state: ""`, as upstream sends it. The only post-input check is `Missing authorization code` (`crates/cyrup-provider/src/auth/oauth/anthropic.rs:790-795`). The copy-code flow (`PROV-120`) never had the check, so the two flows now agree, and the "older, separate drift" wording in `login_copy_code`'s comment and the copy-code test's doc comment is gone. Test: `login_exchanges_a_paste_with_an_empty_state_like_upstream`.

**upstream** — `4df157433` ("share OAuth callback server and sign-in page", v0.99.0) removed `if (!state) throw new Error("Missing OAuth state");` from `loginAnthropic` (it was `auth/oauth/anthropic.ts:304`, right after the `Missing authorization code` check at `:303`). At v1.1.0 the browser flow sets `state = parsed.state ?? verifier` (`anthropic.ts:189`) and its only post-input check is `if (!code) throw new Error("Missing authorization code")` (`:192`). `loginAnthropicCopyCode` has the same shape (`:226-234`).

**cyrup** — in the `PROV-120` first pass, `run_login` in `crates/cyrup-provider/src/auth/oauth/anthropic.rs` still had `if !truthy(state.as_deref()) { return Err(OAuthError::Failed("Missing OAuth state".to_string())); }` (`:739-741`), pinned by `login_rejects_paste_with_empty_state_as_missing_state`. The `PROV-120` commit recorded this as "older, separate drift" and left it.

**Impact** — small. A user who pastes `CODE#` (Anthropic's hosted page shows `code#state`, and a truncated copy loses the state) got `Missing OAuth state` from the browser flow, while pi and cyrup's own copy-code flow send the exchange and let the server decide. The two Anthropic login methods behaved differently on the same paste.

**Fix** — *applied.* Delete the check. Keep `parsed.state.or_else(|| Some(verifier))` (`:775`), so an empty state stays empty and an absent one defaults to the verifier, exactly `??`.

**Verify** — `login_exchanges_a_paste_with_an_empty_state_like_upstream`: for both `C#` and `http://localhost:53692/callback?code=C&state=`, the login completes and the token request body has `code: "C"` and `state: ""`. It was red in the `PROV-120` first pass (`Missing OAuth state`). `copy_code_login_defaults_state_like_upstream` pins the same rule for the copy-code flow.

## PROV-136 — A taken Anthropic callback port goes straight to paste-only instead of falling back to a free port

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-08

> **CLOSED 2026-10-08.** `run_login` now tries the configured port (53692 in production), then port `0` (an OS-chosen free loopback port), then no listener. Each attempt binds `CALLBACK_HOST` and advertises `localhost` (`crates/cyrup-provider/src/auth/oauth/anthropic.rs:664-679`). The authorize URL, the paste placeholder and the token exchange all use the bound listener's URI, and use `Self::redirect_uri()` (`:622`, `REDIRECT_URI` verbatim in production) only when nothing bound (`:681-684`). The constant docs (`:98`, `:109`), `login`'s doc and the provenance table cite the new upstream lines. Testing "nothing bound" no longer works by squatting a port, since a squatted port now just triggers the fallback. So `CallbackServer::start` records every attempt as `host:port` in a test-only `bind_attempts` module (`crates/cyrup-provider/src/auth/oauth/callback.rs:305-307`, `:508`; compiled out of non-test builds), and each test that reads it binds a TEST-NET-1 host (RFC 5737) that no other test uses and on which every bind fails. Tests below.

**upstream** — `8d8ae2fc2` ("fall back to a free port for Anthropic OAuth callback", #10571, v1.1.0). `auth/oauth/anthropic.ts` @v1.1.0: a comment above `CALLBACK_PORT` says the port is preferred so it can be forwarded into containers or over SSH, and that Anthropic accepts any loopback port (`:18-20`). `loginAnthropic` wraps the start in `startCallbackServer(port)` (`:142-152`) and chains `startCallbackServer(CALLBACK_PORT).catch(() => startCallbackServer(0)).catch(() => undefined)` (`:154-156`). It then reads `const redirectUri = callback?.redirectUri ?? REDIRECT_URI` (`:157`) for the authorize URL (`:164`), the manual prompt's placeholder (`:179`) and `exchangeAuthorizationCode` (`:194`). The commit message names the trigger: Hyper-V/WSL port exclusions on Windows. Test: `test/anthropic-oauth.test.ts:224-240`, "falls back to a free callback port when the preferred port cannot be bound", which also asserts `exchangedRedirectUri` equals the advertised URI.

**cyrup** — in the `PROV-120` first pass, `run_login` started one `CallbackServer` on `CallbackServerConfig::fixed(self.callback_port, …)` and ended with `.ok()` (`anthropic.rs:630`), so a taken or reserved 53692 went straight to the `PROV-117` paste-only path. `login_degrades_to_manual_paste_when_the_callback_port_cannot_bind` pinned that.

**Impact** — on a host where 53692 is taken (a second concurrent login) or reserved (Windows' excluded port ranges), pi still gets the browser redirect on another loopback port and the login finishes with no paste. cyrup made the user copy the redirect URL out of the browser by hand. The login still worked, so the severity is low.

**Fix** — *applied.* Two `CallbackServer::start` calls, fixed port then `0`. `.ok()` on the second keeps upstream's "swallow every rejection" (including `Cancelled`). Use the bound URI everywhere and `REDIRECT_URI` only when nothing bound.

**Verify** — all in `auth/oauth/anthropic.rs` tests:
- `login_falls_back_to_a_free_port_when_the_preferred_port_cannot_bind` mirrors upstream's test.
  - It holds the preferred port for the whole login, and a control asserts that port really can't be bound.
  - The browser redirect is delivered to the advertised fallback port and gets a 200 page.
  - The advertised URI is `http://localhost:<port>/callback`, where the port is neither the preferred one nor `0`.
  - The exchanged `redirect_uri` and the paste placeholder both equal the advertised URI.
- `login_degrades_to_manual_paste_when_no_callback_port_can_bind` uses bind host `192.0.2.117`.
  - A control asserts that port 53692 and port 0 both fail there with `Listen`.
  - `bind_attempts` shows exactly `[host:53692, host:0]`.
  - The paste still completes the login.
  - The authorize URL, the placeholder and the exchange are all `REDIRECT_URI`.
- `login_completes_via_browser_redirect` and `login_accepts_a_pasted_redirect_url_with_the_matching_state` assert that the exchange uses the advertised URI.
  - The second builds its paste from the authorize URL's own `redirect_uri` and `state`, like upstream's "keeps the localhost redirect_uri" test (`test/anthropic-oauth.test.ts:43-79`).
- `redirect_uri_matches_the_bound_listener` takes its port from an ephemeral listener it is still holding, so there is no release-then-rebind race.

## PROV-137 — A denied Anthropic consent leaves the browser login waiting instead of failing it

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-08

> **CLOSED 2026-10-08.** `AnthropicCallbackHandler::handle` (`crates/cyrup-provider/src/auth/oauth/anthropic.rs:383-445`) is now a port of the shared handler `loginAnthropic` uses at v1.1.0, in upstream's order: method (404; the pathname half stays in the shared server), state (400 `State mismatch.`), a truthy `error` (400 `Anthropic authorization failed.` with `error_description ?? error`, and the wait settles with `Anthropic authorization failed: <description>`), code (400 `Missing authorization code.`), success (200 `Signed in to Anthropic. You may now close this page.`). Every reply is `no_store()`. The module header and provenance table cite the new upstream. Tests below.

**upstream** — `4df157433` ("share OAuth callback server and sign-in page", v0.99.0) deleted `loginAnthropic`'s own `createServer` handler (`auth/oauth/anthropic.ts:114-148` @v0.83.0) and starts `startOAuthCallbackServer({ providerName: "Anthropic", …, state: verifier, complete: async (code) => code })` (`anthropic.ts:142-151` @v1.1.0). Its request handler is `callback-server.ts:78-116`: 404 for a foreign method or path (`:81-84`), 400 `State mismatch.` when `state` is not exactly the expected one (`:85-88`), 409 when already claimed or settled (`:89-92`), then `if (error)` — `sendPage(400, oauthErrorHtml("Anthropic authorization failed.", description))` and `finish({ error: new Error("Anthropic authorization failed: " + description) })` with `description = error_description ?? error` (`:93-99`), 400 `Missing authorization code.` (`:100-104`), and 200 `Signed in to Anthropic. You may now close this page.` (`:105-109`). A rejected `callback.wait()` throws out of `waitForCallbackOrManualInput` (`:174`) and so out of `loginAnthropic`. Tests: `test/oauth-callback-server.test.ts:103-117` ("rejects the wait when the provider redirects with an error"), `test/anthropic-oauth.test.ts:220` (`toContain("Signed in to Anthropic.")`).

**cyrup** — in the `PROV-120` first pass and through the `PROV-117`/`PROV-135`/`PROV-136` work, `AnthropicCallbackHandler` was the v0.83.0 handler: `?error=` checked first and answered with `CallbackOutcome::Continue` ("Anthropic authentication did not complete." / `Error: {error}`), so the wait never settled; then "Missing code or state parameter."; then the state; success read "Anthropic authentication completed. You can close this window." `bad_redirects_are_answered_without_ending_the_login` pinned the non-settling denial. `openrouter.rs` had already ported the fail-on-error shape for its own flow.

**Impact** — a user who clicks *Deny* on claude.ai saw an error page while the TUI stayed on the paste prompt until they cancelled it; pi ends the login with `Anthropic authorization failed: access_denied`. The old page wording also disagreed with upstream's test. Low: the login could still be cancelled or completed by retrying.

**Fix** — *applied.* Reorder and re-word the handler as above, returning `CallbackOutcome::Failed` for a state-matching `error`. No change to `run_login` was needed: its `Winner::Redirect(Err)` arm already propagates, and the paste prompt is aborted on that exit (then by an explicit cancel in the arm; since `PROV-138`, by the drop guard that covers every exit).

**Verify** — in `auth/oauth/anthropic.rs` tests:
- `a_denied_redirect_fails_the_login`: for `error=access_denied` and for an added `error_description=User denied access`, with the flow's state, the login fails with `Anthropic authorization failed: access_denied` / `Anthropic authorization failed: User denied access`, the browser gets 400 `Anthropic authorization failed.` with the description, and the token endpoint sees no request. The paste prompt never answers, so only the redirect can end the login.
- `bad_redirects_are_answered_without_ending_the_login`: `?error=access_denied` with no state, `?code=only` and `?code=C&state=wrong` each get 400 `State mismatch.`; `?error=&state=<verifier>` (an empty `error` is falsy) gets 400 `Missing authorization code.`; the following good redirect still completes the login.
- `login_completes_via_browser_redirect`: the 200 page reads `Signed in to Anthropic. You may now close this page.`

The tests that drive the listener over HTTP while the paste prompt blocks now have a deadline, so a login that never settles fails the test instead of hanging the suite. The deadline is 10s, on the login and on the driver (`within`), plus a read timeout in `http_get`. Mutation check: with `Err(_) => start(0).await.ok()` replaced by `Err(_) => None`, `login_falls_back_to_a_free_port_when_the_preferred_port_cannot_bind` fails with "the login did not finish within 10s" where it used to hang.

## PROV-138 — The Anthropic browser login leaves the paste prompt's abort signal live when the paste wins

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-10-08

> **CLOSED 2026-10-08.** `run_login` holds a `drop_guard()` of the paste prompt's `CancelToken` (`crates/cyrup-provider/src/auth/oauth/anthropic.rs:705`), so the token fires on every exit from the wait, as upstream's `finally` does; the success path drops the guard as the wait settles, before the exchange (`:788`). Tests below.

**upstream** — `loginAnthropic` @v0.83.0 creates `manualAbort` (`anthropic.ts:232`), passes `manualAbort.signal` to the `manual_code` prompt (`:261`) and calls `manualAbort.abort()` in `finally` (`:299-300`). Since `4df157433` (v0.99.0) the same shape lives in `waitForCallbackOrManualInput` (`callback-server.ts:155-183` @v1.1.0): `manualAbort` at `:160`, the prompt's `signal` at `:163`, and `finally { manualAbort.abort(); }` at `:180-182`, which runs whether the callback won, the paste won, or either rejected. pi `test/anthropic-oauth.test.ts:177-211` @v1.1.0 answers the selector with `browser`, answers the paste, and asserts `manualSignal?.aborted` is `true` ("the prompt's signal is aborted once login settles, so UIs can dismiss it").

**cyrup** — `run_login` cancelled `manual_abort` only in the `Winner::Redirect` arm (`anthropic.rs:648` at `d3789b1`, `:675` in the `PROV-120` first pass). When the paste won (`Winner::Manual`), when the paste prompt rejected, and on the second-chance `manual.await`, the function returned with the token still live.

**Impact** — an interaction that keeps UI state keyed on the prompt's token (the contract `AuthPrompt::cancel` documents) is never told that the prompt is finished. The TUI dialog is replaced when the login settles, so nothing user-visible depended on it today. Low.

**Fix** — *applied.* Replace the hand-written cancel with `manual_abort.clone().drop_guard()`, held across the race and the second chance and dropped explicitly before the exchange.

**Verify** — in `auth/oauth/anthropic.rs` tests:
- `login_resolves_through_the_manual_prompt_and_aborts_it_after_settling`: the port of pi's test. The paste wins, the credential is stored from the fake token endpoint, an `auth_url` was emitted, and the `manual_code` prompt's token is cancelled. It fails against the previous code.
- `a_failed_manual_prompt_is_aborted_too`: a paste prompt that rejects fails the login with its own error, and its token is cancelled.

## Findings filed 2026-10-09 — the pi v1.1.0 drift triage (`v1.0.1..f1b2e77f5`)

Upstream read through `git -C tmp/pi show` only, at `f1b2e77f5` (= `v1.1.0-11-gf1b2e77f5`); cyrup read at `6b14575`. Nothing was run. The window record (what was read and not filed) is the PIN 2026-10-09 block at the top of this file.

## PROV-139 — Bedrock does not recognise Claude Haiku 5.5: no prompt caching, budget thinking instead of adaptive, and no native `xhigh` or block binding

> **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5; see the table row). The **cyrup** paragraph below describes the state when this was filed and is no longer true: all four predicates carry `haiku-5`, and the module's upstream citations are @f1b2e77f5.

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.1..f1b2e77f5` = v1.1.0+11, `packages/ai`, cyrup `6b14575`).

**upstream** — `f76c1db66` (Haiku 5.5, in v1.1.0) adds the `haiku-5` needle to all four Bedrock predicates in `packages/ai/src/api/bedrock-converse-stream.ts` @f1b2e77f5: `supportsAdaptiveThinking` `:766` (needle `:776`), `supportsNativeXhighEffort` `:781` (`:789`), `supportsThinkingBlockBinding` `:798` (`:806`) and `supportsPromptCaching` `:876` (the Claude 5 branch, `:886-889`).

**cyrup** — `crates/cyrup-provider/src/api/bedrock_converse_stream/capabilities.rs`: `supports_adaptive_thinking`'s `NEEDLES` (`:40-49`, seven entries, no `haiku-5`), `supports_thinking_block_binding` `:61`, `supports_native_xhigh_effort` `:70`, `supports_prompt_caching` `:126` (`any(&["fable-5", "opus-5", "sonnet-5"])`). `rg 'haiku-5' crates/cyrup-provider/src --glob '!**/tests/**'` finds nothing. The rows do reach cyrup: the live pi.dev `amazon-bedrock` catalog (fetched 2026-10-09) serves six `*.anthropic.claude-haiku-5-5` rows, which the runtime overlay merges in, and Haiku 4.x's `-4-` needle does not match `haiku-5-5`. The module's comments also still cite v1.0.1-era upstream lines (for example `:588-600` and `:679-698`).

**Impact** — A Bedrock Haiku 5.5 session sends no cache points, so every turn bills the whole prompt at the full input rate: real extra spend, not just a wrong display. With thinking on, cyrup sends `thinking:{type:"enabled",budget_tokens}` plus the interleaved beta where upstream sends adaptive thinking with `output_config.effort`; `xhigh` is downgraded to `high`; replayed thinking gets no `block_binding`. **The impact is probably understated:** pi's compat doc says `forceAdaptiveThinking` is for models whose upstream *requires* the adaptive format, and pi's Anthropic catalog sets it for Haiku 5.5, so the budget-thinking request cyrup builds may be rejected outright when thinking is on. That is plausible, not run-verified. Severity medium stands.

**Fix** — Add `"haiku-5"` to the four needle lists in `capabilities.rs` in upstream's order. Refresh every upstream line citation in the module's comments to @f1b2e77f5, not only the four needles' citations.

**Verify** — Unit tests in `bedrock_converse_stream/tests` for `global.anthropic.claude-haiku-5-5`: `supports_prompt_caching` is true, so a `cachePoint` is emitted; the request uses `thinking.type == "adaptive"` with `block_binding` and the binding beta; `map_thinking_level_to_effort(Xhigh) == "xhigh"`. Each is red before the needle is added. `anthropic.claude-haiku-4-5` keeps its current shape.

## PROV-140 — Bedrock Converse sends no reasoning effort to OpenAI GPT models, so the configured thinking level never reaches them

> **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5). `params.rs` gains `build_openai_reasoning_fields` after the Claude branch, with gpt-oss tested before `gpt-`, both effort tables, and the map consulted only on the nested `gpt-` form. Every Verify clause has a test in `bedrock_converse_stream/tests/params.rs` that fails at HEAD; the closure in the table row maps them. The only `capabilities.rs` edit is `model_match_candidates` becoming `pub(super)`.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.1..f1b2e77f5` = v1.1.0+11, `packages/ai`, cyrup `6b14575`).

**upstream** — `2989eb581` (#10142, closes #9331): `buildAdditionalModelRequestFields` (`packages/ai/src/api/bedrock-converse-stream.ts:1262` @f1b2e77f5) gains two non-Claude branches at `:1318-1330`. gpt-oss gets a flat `reasoning_effort` from `OPENAI_GPT_OSS_EFFORT`, clamped to low/medium/high. Other `gpt-` models get `reasoning:{effort}` from `thinkingLevelMap`, else from `OPENAI_GPT_EFFORT`, where minimal becomes low. The two tables are at `:1339-1356`. Only the nested `gpt-` branch consults `thinkingLevelMap`; the gpt-oss branch uses its table unconditionally.

**cyrup** — `crates/cyrup-provider/src/api/bedrock_converse_stream/params.rs:128-140`: `build_additional_model_request_fields` returns `None` unless `is_anthropic_claude_model(model)`. `rg -n 'gpt|reasoning_effort' crates/cyrup-provider/src/api/bedrock_converse_stream/` finds nothing. The embedded `amazon-bedrock.json` carries 33 `openai.gpt` rows, among them `global.openai.gpt-5.6-*` / `gpt-6-*` (e.g. `:2254`, `:2436`) and `openai.gpt-oss-120b` (`:4300`).

**Impact** — On Bedrock, GPT-5.x, GPT-6 and gpt-oss always run at Bedrock's default effort; the thinking-level selector has no effect for them.

**Fix** — Port both branches after the Claude branch, using the same `model_match_candidates` (`capabilities.rs:12`), and add the two effort tables. Test `gpt-oss` before `gpt-`, because `gpt-oss` also contains `gpt-`. In the nested form only, a `thinking_level_map` string entry overrides the table, as upstream does.

**Verify** — Port `test/bedrock-thinking-payload.test.ts`'s new cases: gpt-oss at `xhigh` sends `reasoning_effort:"high"`; GPT-6 at `minimal` sends `reasoning:{effort:"low"}`; a mapped level wins for `gpt-`; a gpt-oss model ignores the map; a Claude model is unchanged; reasoning off sends no field.

## PROV-141 — Mistral `finish_reason: "error"` is not retried, because its message lacks the `server error` marker upstream added

> **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5). Upstream's `"error"` arm (`packages/ai/src/api/mistral-conversations.ts:946-948` @f1b2e77f5) re-read and its string and comment copied byte for byte into `crates/cyrup-provider/src/api/mistral_conversations/finish.rs` (`map_chat_stop_reason`); the doc comment's text and citations now point at `:936-952` (function), `:946-948` / `:949-950` (arms) and `:633` (the `if (choice.finish_reason)` guard). No change to `utils/retry.rs`: `server.?error` was already in the list. Verify: `tests/stop_reason.rs::prov141_a_mistral_error_finish_reason_is_retryable_and_an_unknown_one_is_not` decodes a `finishReason: "error"` chunk, asserts the terminal's `error_message` is the new string and that `is_retryable_assistant_error` returns true on it, then does the same for `"unmapped_error"` and asserts false; `an_unrecognized_finish_reason_is_an_error_not_a_clean_stop` pins the mapped string. With the `finish.rs` hunk reverted both tests fail (`left: Some("Provider stopped with: error")`, `right: Some("Provider stopped with: error (server error)")`).

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.1..f1b2e77f5` = v1.1.0+11, `packages/ai`, cyrup `6b14575`).

**upstream** — `7fb59f995` (#10487): `mapChatStopReason`'s `"error"` arm returns `errorMessage: "Provider stopped with: error (server error)"` (`packages/ai/src/api/mistral-conversations.ts:946-948` @f1b2e77f5), so `server.?error` in the retry classifier matches. The test asserts `isRetryableAssistantError(message) === true` and that an unknown reason stays non-retryable.

**cyrup** — `crates/cyrup-provider/src/api/mistral_conversations/finish.rs:76-79` still returns `"Provider stopped with: error"`; its doc comment at `:68-70` quotes the old text and cites stale upstream lines (`:672-675`); `tests/stop_reason.rs:31` pins it. cyrup's retry list already has `server.?error` (`utils/retry.rs:84`), so the one-string change makes the error retryable.

**Impact** — Mistral reports transient server failures this way. In cyrup they end the turn with an error banner where pi auto-retries.

**Fix** — Change the string to `"Provider stopped with: error (server error)"` with upstream's comment; update the doc comment (text and upstream line citation) and the stop-reason test.

**Verify** — `map_chat_stop_reason(Some("error"))` yields the new message and `utils::retry::is_retryable_assistant_error` on the resulting message is true; `Some("unmapped_error")` stays non-retryable. Red before.

## PROV-142 — The request-context estimate still uses 4 characters per token; pi 1.1.0 uses 3.5, so cyrup's output-token clamp leaves less headroom and long prompts can overflow

> **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5): the request estimate divides by 3.5 (exact 7/2 integer ceiling) on every path, cyrup-session compaction stays at `/ 4`, pi's three `context-estimate.test.ts` cases are ported with their `max_tokens` values, the `"20 chars / 4"` assertion flipped to 6, the image case is 1372, and the `compaction_tokens_after` and `estimator_prefix_timestamp_parity` tests were run and pass. Clause-by-clause mapping and red-proof are in the table row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.1..f1b2e77f5` = v1.1.0+11, `packages/ai`, cyrup `6b14575`).

**upstream** — `27075fe07` (#10497), which touches only `estimate.ts` and its test: `CHARS_PER_TOKEN = 3.5` (`packages/ai/src/utils/estimate.ts:15` @f1b2e77f5). It drives `clampMaxTokensToContext` (`api/simple-options.ts:18`, used at `:46`). The coding-agent compaction estimator (`coding-agent/src/core/compaction/compaction.ts:311`, `:317`) deliberately stays at `/ 4`.

**cyrup** — `crates/cyrup-provider/src/utils/estimate.rs:27` is `const CHARS_PER_TOKEN: u64 = 4;`, with `ceil_div` (`:36-38`) using `div_ceil`. Its only production consumer is `utils/simple_options.rs:52` (`clamp_max_tokens_to_context`). cyrup-session's compaction has its own estimator (`compaction/tokens.rs`) and must not change. Two other consumers see the value: `estimate_*` is public API (`lib.rs:191-194`), so SDK users see the change; and the test `cyrup-session-svc/src/tests/compaction_tokens_after.rs:134-138` sums `cyrup_provider::estimate_message_tokens` directly.

**Impact** — For large inputs cyrup computes a higher `max_tokens` than pi. Providers that reject `input + max_tokens > context` (verified upstream on OpenRouter DeepSeek V4 Flash) fail the request where pi succeeds.

**Fix** — Use 3.5 with an integer-exact ceiling, `ceil(chars / 3.5) == (2*chars).div_ceil(7)`, on the text and image paths. Leave `cyrup-session` compaction at `/4`, as upstream does.

**Verify** — Port `test/context-estimate.test.ts`'s new expectations (20 chars give 6, not 5; one image gives `ceil(4800/3.5)` = 1372). The existing `"20 chars / 4"` assertion at `estimate.rs:524` flips. Check, rather than assume, that the `compaction_tokens_after` tests still pass, since one of them sums the provider estimate.

## PROV-143 — Codex Responses still pins `originator` and `User-Agent` after the caller's headers, so `model.headers` / `options.headers` cannot override them as pi 1.1.0 allows

> **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5). Upstream re-read: `buildBaseCodexHeaders` is `:1640-1661`, seeding `new Headers({ originator: "pi", "User-Agent": getPiUserAgent() })` at `:1647` (comment `:1646`), init headers `:1648-1650`, additional headers with `null` → `delete` `:1651-1657`, then `Authorization` / `chatgpt-account-id` `:1658-1659`; `buildSSEHeaders` is `:1663-1681`. `crates/cyrup-provider/src/api/openai_codex_responses/headers.rs::build_sse_headers` now follows that order, with cyrup's per-credential `auth.auth.headers` loop between `model.headers` and `options.headers` (all three after the seed). The doc comment no longer claims the four headers "cannot be overridden", and the citations in it, `codex_user_agent`'s doc and the `mod.rs` provenance table point at @f1b2e77f5. cyrup has no Codex WebSocket transport, so `buildWebSocketHeaders` has no counterpart to change. Verify: (1) "a caller `originator` / `User-Agent` wins" is `tests/headers.rs::prov143_model_and_caller_headers_override_originator_and_user_agent`, a port of `openai-codex-stream.test.ts:800-847` (model `originator: "my-app"`, caller `user-agent: "my-app/1.0"`, each resolved case-insensitively to exactly one entry); with the `headers.rs` hunk reverted it fails with `left: [Some("pi")]`, `right: [Some("my-app")]`. (2) "a caller `Authorization` / `chatgpt-account-id` is still replaced" is the same test's last two assertions (caller sends `Bearer ignored` / `acct_ignored`), plus the existing `sse_headers_match_upstream_and_auth_cannot_be_overridden`. These hold at HEAD too, since HEAD already set them last, so their bite was proven against a mutant that moves the two sets ahead of the overlays: both tests fail (`left: [Some("Bearer ignored")]`; `left: None`). `prov143_credential_overlay_overrides_and_a_none_overlay_deletes_the_default` also covers the Fix's `auth.auth.headers` and `None`-deletes clauses; red before with `left: Some(Some("pi"))`, `right: Some(Some("cred-app"))`.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.1..f1b2e77f5` = v1.1.0+11, `packages/ai`, cyrup `6b14575`).

**upstream** — `0cf65d2bf` (#10429): `buildBaseCodexHeaders` (`packages/ai/src/api/openai-codex-responses.ts:1640-1661` @f1b2e77f5) now seeds `{originator:"pi", "User-Agent": getPiUserAgent()}` first (`:1646-1647`), then applies init and additional headers. Only `Authorization` and `chatgpt-account-id` are set last (`:1658-1659`).

**cyrup** — `crates/cyrup-provider/src/api/openai_codex_responses/headers.rs:55-96`: the overlays come first (`:63-80`), then `Authorization`, `chatgpt-account-id`, `originator` (`:93`) and `User-Agent` (`:94`). The doc comment (`:44-48`) states that the four "cannot be overridden", and its upstream citation (`:1577-1617`) is stale. cyrup has three overlay loops, the third being the per-credential `auth.auth.headers`.

**Impact** — An extension or models.json entry that sets `originator` or `User-Agent` for openai-codex is silently overwritten, unlike every other adapter.

**Fix** — Seed `originator` and `User-Agent` before all three overlay loops (including `auth.auth.headers`), keep `Authorization` and `chatgpt-account-id` last, and rewrite the doc comment and its citation. A `None` overlay value on `originator` then deletes it, as `headers.delete` does upstream.

**Verify** — Port the two cases from `test/openai-codex-stream.test.ts`: a caller `originator` / `User-Agent` wins; a caller `Authorization` / `chatgpt-account-id` is still replaced. Red before.

## PROV-144 — `LoginOptions.agentName` is unported, so an embedding app cannot name itself in the Sign in with ChatGPT or Codex browser login

> **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5). **pi at the pin** (`9ad083102`, read in full): `auth/types.ts:209-213` `agentName?: string` ("Name this app introduces itself with during login, e.g. OpenAI's agent name hint and Codex originator. Defaults to pi's own name."); `auth/oauth/openai-chatgpt.ts:253` `agent_name_hint: options?.agentName ?? AGENT_NAME_HINT`; `auth/oauth/openai-codex.ts` `createAuthorizationFlow(originator: string = "pi")` (`:289-306`, `url.searchParams.set("originator", originator)` at `:305`), `loginOpenAICodex(interaction, options?: LoginOptions)` calls `createAuthorizationFlow(options?.agentName)` (`:359-363`), and `openaiCodexOAuth.login(interaction, options)` passes `options` to the browser method only (the device-code flow carries no originator). **Fix:** `crates/cyrup-provider/src/auth/mod.rs` `LoginOptions.agent_name: Option<String>` with upstream's doc quoted, `with_agent_name`, shown in `Debug`; `auth/oauth/openai_chatgpt.rs` `login` resolves `options.agent_name` else the flow's default (`AGENT_NAME_HINT`) and `authorization_url` takes it as a parameter; `auth/oauth/openai_codex.rs` `create_authorization_flow(originator: Option<&str>)` (`None` = upstream's `undefined` → default `ORIGINATOR`), `login_browser(interaction, options)`, `login` passes `options` through. `Some("")` sends an empty value, as `??` and a default parameter both do for `""`. Defaults and their divergence notes kept (the ChatGPT module note now says the hint is the default only). Embedders reach it through `Models::login_with_options`. **Verify:** `openai_chatgpt.rs::tests::prov144_uses_the_apps_agent_name_as_the_name_hint` (pi "uses the app's agent name as the name hint": full `login` against a loopback token endpoint with `getDeviceId` + `agentName: "my-app"` → authorize URL `agent_name_hint=my-app`; without it `Pi`); `openai_codex.rs::tests::prov144_uses_the_apps_agent_name_as_the_browser_login_originator` (pi "uses the app's agent name as the browser login originator": `login` with the `browser` method and a pasted code → `auth_url` event's `originator=my-app`; without it `pi`). **Red before:** with the ChatGPT `login` ignoring `options.agent_name` the first fails `left: Some("Pi") right: Some("my-app")`; with the Codex `login_browser` passing `None` the second fails `left: Some("pi") right: Some("my-app")`; files restored byte-identical (sha256 checked). At HEAD neither compiles (no `with_agent_name`).

**Kind** not-ported · **Severity** ~~low~~ **CLOSED 2026-10-10** · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.1..f1b2e77f5` = v1.1.0+11, `packages/ai`, cyrup `6b14575`).

**upstream** — `9ad083102` (#10433): `LoginOptions.agentName?` (`packages/ai/src/auth/types.ts:209-213`, the field at `:213` @f1b2e77f5) replaces `AGENT_NAME_HINT` in the ChatGPT authorize URL (`auth/oauth/openai-chatgpt.ts:253`) and the Codex browser-login `originator` (`auth/oauth/openai-codex.ts:363`, threaded from `login(interaction, options)`).

**cyrup** — `crates/cyrup-provider/src/auth/mod.rs:41-45`: `LoginOptions` has only `get_device_id`. The two flows hold private fields only the constants fill: `auth/oauth/openai_chatgpt.rs:527` / `:547` (`AGENT_NAME_HINT`, `:125`) and `auth/oauth/openai_codex.rs:520` / `:543` (`ORIGINATOR`, `:148`; the authorize parameter at `:588`).

**Impact** — SDK embedders (`cyrup-sdk`) always present as "Pi" / `pi` on OpenAI's consent screen. cyrup's own CLI is unaffected: it deliberately sends the upstream values.

**Fix** — Add `agent_name: Option<String>` to `LoginOptions` and use `options.agent_name.unwrap_or(AGENT_NAME_HINT / ORIGINATOR)` in both `login` paths. Keep the defaults and their divergence notes.

**Verify** — Port `openai-chatgpt-oauth.test.ts` / `openai-codex-oauth.test.ts`: with `agent_name: Some("Acme")` the authorize URL carries `agent_name_hint=Acme` / `originator=Acme`; without it the values stay `Pi` / `pi`.

## PROV-145 — The Azure provider is still `azure-openai-responses` and serves only the Responses API; pi 1.0.3 renamed it to `azure` and added Foundry Chat Completions deployments (DeepSeek V4 Pro). Decision-gated: the rename needs an owner decision first

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.1..f1b2e77f5` = v1.1.0+11, `packages/ai`, cyrup `6b14575`).

**upstream** — `a37306d43` (#9714, closes #9645; first contained in v1.0.3). `providers/azure.ts` (`id: "azure"` `:46`) dispatches `azure-openai-responses` and `openai-completions` (`:52`) through `azureStreams`, which resolves the endpoint onto the model; `withDeploymentName` (`:15`) swaps the `AZURE_OPENAI_DEPLOYMENT_NAME_MAP` deployment into the payload via `onPayload`. The shared helpers moved to `api/azure-openai-config.ts`; `env-api-keys.ts:90` and `all.ts:141` were renamed. The generator adds `azure/deepseek-v4-pro` with compat `supportsDeveloperRole:false`, `supportsMidConvoSystemMessages:true`, `thinkingFormat:"openai"`, `supportsLongCacheRetention:false`, Azure pricing and `AZURE_DEEPSEEK_V4_THINKING_LEVEL_MAP` (`scripts/generate-models.ts:290`, `:3330-3343`), all @f1b2e77f5. The rename is a recorded upstream breaking change for auth.json, models.json and settings.json keys; the coding-agent half is the one-line `defaultModelPerProvider` key in `model-resolver.ts:25`.

**cyrup** — `crates/cyrup-provider/src/providers/azure_openai_responses.rs:16`: `AZURE_OPENAI_RESPONSES_PROVIDER_ID = "azure-openai-responses"`, one api, models from `catalog/azure-openai-responses.json`; also `env_api_keys.rs:61` and `providers/all.rs:270-271`, and `crates/cyrup-config/src/model/defaults.rs:17` / `:77` / `:278` (the default-model key). Measured 2026-10-09: `https://pi.dev/api/models/providers/azure-openai-responses` returns **404** and `/azure` returns 200 with a `deepseek-v4-pro` row.

**Impact** — **The user-visible part:** the runtime overlay fetch for this stem now 404s, so Azure users silently stop receiving live catalog updates and their catalog is frozen at the embedded floor. `cargo run -p xtask -- gen-catalogs` also fails or skips the stem. Foundry Chat Completions deployments are unusable. Low, because the existing Responses deployments keep working.

**Fix** — **Owner decision first:** whether to follow the rename touches auth.json and models.json keys, so it is an area 05 migration question. Then: register `openai-completions` on the Azure provider behind an endpoint-resolving wrapper; apply the deployment-name map via `on_payload`; point the catalog and overlay id at `azure`; carry a read-side alias for stored `azure-openai-responses` credentials if the id changes; extend `CFG-102`'s default-model table work with the renamed key.

**Verify** — `azure/deepseek-v4-pro` streams through openai-completions against a loopback server: the body carries the mapped deployment as `model`, has `reasoning_effort` and no `thinking` / `prompt_cache_key`, and goes to the resolved Azure base URL. `gen-catalogs --check --only azure` reproduces pi.dev. A stored `azure-openai-responses` credential still resolves, if an alias is chosen.

## PROV-146 — `Model.samplingParamsByThinkingLevel` is unported, so per-thinking-level sampling overrides are ignored at request time (the request half; lands with `PROV-123`, and `CFG-104` is the models.json half)

> **CLOSED 2026-10-10** (checked against pi f1b2e77f5). Landed in #215 (`52b1aa33`) with `CFG-104`. The **cyrup** paragraph below describes the state when this was filed and is no longer true. Evidence, per Verify clause, is in the table row.

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.1..f1b2e77f5` = v1.1.0+11, `packages/ai`, cyrup `6b14575`).

**upstream** — `76dfb88f6` (#9776, v1.0.2). `samplingParamsByThinkingLevel?` on `Model` (`packages/ai/src/types.ts:854` @f1b2e77f5). `resolveSamplingParams(model, level, request)` (`api/simple-options.ts:24-34`) spreads `model.samplingParams`, then the entry for `clampThinkingLevel(model, level)`, then the request params. At f1b2e77f5 it is called from **both** sites: `buildBaseOptions` (`:41-42`) and the three OpenAI-compatible `buildParams` (`openai-completions.ts:1004`, `openai-responses.ts:383`, `azure-openai-responses.ts:243`). No pi.dev catalog row carries the field; it reaches users through models.json only (coding-agent `provider-composer.ts:72`, `:165-202`).

**cyrup** — `rg 'sampling_params_by_thinking_level|samplingParamsByThinkingLevel' crates` finds only a doc mention at `crates/cyrup-provider/src/virtual_models.rs:130`. `Model` has `sampling_params` only (`model.rs:197`). `utils/simple_options.rs:64-106` (`merge_sampling_params`, `build_base_options`) merges model and request params with no level layer, and the adapters apply `opts.sampling_params` (`api/openai_completions/params.rs:282`).

**Impact** — A model configured with, for example, `samplingParamsByThinkingLevel.high.temperature` gets the base sampling at every level. The key parses (serde ignores it) and has no effect.

**Fix** — **This is an amendment to the open `PROV-123`, not an independent change.** `PROV-123` says pi moved the `model.samplingParams` merge out of `buildBaseOptions` into the three `buildParams`; since `76dfb88f6` both sites call `resolveSamplingParams`, which changes `PROV-123`'s shape. Re-read `PROV-123` at f1b2e77f5 and land both together at `simple_options.rs:57-106` so the two fixes do not clash. Then: add `sampling_params_by_thinking_level: Option<BTreeMap<ModelThinkingLevel, Map>>` to `Model` (serde camelCase); port `resolve_sampling_params` with the existing thinking-level clamp; call it from `build_base_options` and the three adapters' param builders with the effort or `"off"`. The models.json and composer half is `CFG-104` (area 05).

**Verify** — Port `test/sampling-options.test.ts`'s new cases: a level entry overrides the model default and is overridden by the request; an unsupported level clamps to the model's nearest; reasoning off selects the `off` entry; no params at all yields `None`.

## PROV-147 — The `openai-decisions` classifier API (OpenAI's Decisions API, `openai/gpt-6-luna` as a classifier) is unported

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.1..f1b2e77f5` = v1.1.0+11, `packages/ai`, cyrup `6b14575`).

**upstream** — `ce8972a0e`. `api/openai-decisions.ts` maps `POST {baseUrl}/decisions`: bool becomes a predicate with the criteria appended to the instructions (`:51-71`); up to 128 images go as `input_image` data URLs (`:33`, `:73-91`); a refusal fails the result; 504 is not retried (`NO_RETRY_STATUSES` `:151`; `noRetryStatuses` in `utils/provider-retry.ts:8`, `:121`). `classify` is at `:161`; the shared HTTP code is in `api/classifier-shared.ts`. Registration is `providers/openai.ts:25-32`, with `filterAllModels` hiding classifiers under Sign in with ChatGPT OAuth (`:28`). The api id is at `types.ts:83`; the catalog row is `gpt-6-luna`, input `[text,image]`, `contextWindow` 922000, input-only cost with the long-context tier (`scripts/generate-models.ts`, `OPENAI_CLASSIFIER_MODELS`). All @f1b2e77f5.

**cyrup** — `crates/cyrup-provider/src/classifier.rs:84-110`: `KnownClassifierApi` has `LlamaCppClassify` only. `rg -i 'decisions' crates --type rust` finds nothing. `gpt-6-luna` exists only as a chat row (`providers/catalog/openai.json:1451`). `utils/provider_retry.rs` has no per-call no-retry status list. pi.dev already serves `gpt-6-luna` with `api:"openai-decisions", type:"classifier"` on `/providers/openai?types=chat,image,classifier` (fetched 2026-10-09), so the runtime overlay already hands cyrup a classifier row whose api it cannot dispatch. Related, already filed: `PROV-104` (System One apis), `PROV-105` (`filterAllModels`).

**Impact** — No hosted classifier is available on the `openai` provider; codemode's `models.classify()` (area 18) cannot use an OpenAI API key for classification.

**Fix** — Add `KnownClassifierApi::OpenAiDecisions`, a port of `openai-decisions.ts` on the llama-cpp classifier's HTTP/retry plumbing extended with a `no_retry_statuses` option. Register it on the openai provider with the API-key-only filter (with `PROV-105`) and add the classifier row to the openai catalog. Check what `Models::classify` does today with the overlay's `openai-decisions` row: an error is expected, and it should be the upstream-shaped `Unsupported classifier API`. Same commit and same struct as `PROV-148`; the two may land together.

**Verify** — Port `test/openai-decisions.test.ts` against a loopback server: the wire shape for choice / score / bool; images; more than 128 images is an error; a refusal gives an error result with usage kept; 504 is not retried while 500 is; under OAuth the model is absent from the available classifiers.

## PROV-148 — `ClassifierContext.images` is unported, so a classify request cannot carry images and no model-level image check exists

> **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5). **pi at the pin:** `packages/ai/src/types.ts:682-690` `interface ClassifierContext { state: JsonObject; images?: ImageContent[]; questions: … }` (doc: "Only models whose `input` includes `"image"` accept them; other models return an error result"); `utils/model-operations.ts:46-53` `assertClassifierInputSupported`: ``if (context.images?.length && !model.input.includes("image")) throw new ModelsError("provider", `Model ${model.provider}/${model.id} does not accept image input`)``; `models.ts:972-989` `classify` runs `assertClassifierModel`, then that assert, then `requireProvider`, `provider.classify` check, `applyAuth`, all inside a `try` whose `catch` is `classifierErrorResult`; `api/llama-cpp-classify.ts:437`, after the api check: ``if (context.images?.length) throw new Error(`${LABEL} classification does not support image input`)``, caught into an error result. **Fix:** `crates/cyrup-provider/src/classifier.rs` — `ClassifierContext.images: Option<Vec<Content>>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`; cyrup has no standalone `ImageContent`, so it carries `Content::Image` blocks, the same `{type:"image",data,mimeType}` wire shape — a `Content::Text` entry would also deserialize, which pi's type forbids; nothing consumes the images yet, so this is recorded rather than split), `ClassifierContext::has_images` (pi `context.images?.length`), `assert_classifier_input_supported`; `collection.rs` `Models::classify` calls it before the provider lookup (pi order; `assertClassifierModel` is structural in cyrup, the parameter is a `ClassifierModel`); `api/llama_cpp_classify.rs` `run` ports the `:437` guard after the api check. Construction sites updated with `images: None` (`cyrup-llama` and `cyrup-it` tests, the provider test fixtures). Side effect, no code change there: codemode's `check_classifier_context` builds the context with `serde_json::from_value`, so a script's `images` now reach `Models::classify` (and its check) instead of being dropped (`CODE-023`, area 18, is not re-verified here). **Verify:** `tests/classifier_dispatch.rs` `prov148_models_classify_rejects_images_on_a_text_only_classifier_model` (error result `Model p/c does not accept image input`, api calls 0), `prov148_the_image_check_runs_before_the_provider_lookup` (`Model ghost/c …`, not `Unknown provider`), `prov148_images_reach_an_image_capable_model_and_empty_or_absent_images_pass` (image model reaches the api; `images: Some([])` and `None` unchanged on a text-only model); `tests/llama_cpp_classify.rs::prov148_rejects_image_input` (exact message, no request to the fake server); `tests/classifier_types.rs::prov148_classifier_context_carries_images_on_the_wire`. **Red before:** at HEAD the tests do not compile (no `images` field); with the field kept, each test fails under a targeted mutation — `Models::classify` assert removed (the two rejection tests fail), llama guard removed (`prov148_rejects_image_input`), the assert ignoring `model.input` (the pass-through test), `skip_serializing_if` removed (the wire test); files restored byte-identical (sha256 checked).

**Kind** not-ported · **Severity** ~~low~~ **CLOSED 2026-10-10** · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.1..f1b2e77f5` = v1.1.0+11, `packages/ai`, cyrup `6b14575`).

**upstream** — `ce8972a0e`: `ClassifierContext.images?: ImageContent[]` (`packages/ai/src/types.ts:682-690`, the field at `:688` @f1b2e77f5). `assertClassifierInputSupported` (`utils/model-operations.ts:46-53`), called from `Models.classify` (`models.ts:979`), errors with `Model <p>/<id> does not accept image input` unless `model.input` includes `image`. `llama-cpp-classify` rejects images explicitly (`api/llama-cpp-classify.ts:437`: `${LABEL} classification does not support image input`, turned into an error result).

**cyrup** — `crates/cyrup-provider/src/classifier.rs:676-680`: `ClassifierContext { state, questions }`, with no images. `Models::classify` (`collection.rs:658`) has no input-modality check, and `api/llama_cpp_classify.rs` has no image guard.

**Impact** — An image-judging classify is not expressible; codemode's `models.classify({images})` (`CODE-023`, area 18) has nothing to pass them into, so a script's images are dropped and the classifier answers from `state` alone where pi returns an error result. Once a second classifier API lands (`PROV-147`), the missing guard would silently drop images there too.

**Fix** — Add `images: Option<Vec<ImageContent>>` (serde default) to `ClassifierContext`, the modality assert in `Models::classify`, and the llama-cpp guard returning an error result with upstream's message whenever `images` is non-empty.

**Verify** — `classify` with images on a text-only classifier model returns an error result naming the model; on llama-cpp it returns `llama.cpp classification does not support image input`; without images behaviour is unchanged.

## PROV-149 — The embedded catalogs predate pi 1.1.0: no Claude Haiku 5.5 rows, Sonnet 5.5 cache reads priced at 0.2 instead of 0.1, Sonnet 4.5 embedded at a 1M context window, and no prompt-length tiers for Google, OpenCode, OpenCode Go, OpenRouter, Vercel or MiniMax

> **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5; see the table row). The **cyrup** paragraph below describes the state when this was filed and is no longer true: the nine catalogs it measured were regenerated from pi.dev and match its pi.dev column row for row and tier for tier.

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.1..f1b2e77f5` = v1.1.0+11, `packages/ai`, cyrup `6b14575`).

**upstream** — `f76c1db66` adds Haiku 5.5 (a tier above 100k input at 5x). `ce950d78f` drops the hand-written 5.5 fallbacks, so models.dev's Sonnet 5.5 `cacheRead: 0.1` wins (`test/supports-xhigh.test.ts`). `943a10e74` keeps prompt-length tiers from every catalog (`scripts/ai-gateway-pricing.ts`, `scripts/openrouter-catalog.ts`, `getModelsDevCost` in `generate-models.ts`). `a2eef9eb6` pins Kimi K3 to cacheWrite 0. All @f1b2e77f5 and served at pi.dev. Separately, with no upstream commit behind it, models.dev now serves `claude-sonnet-4-5` and `claude-sonnet-4-5-20250929` at `contextWindow` 200000 and `inputLimits.images.maxPerRequest` 100.

**cyrup** — `crates/cyrup-provider/src/providers/catalog_manifest.json`: `anthropic` fetchedAt 2026-09-28. Measured 2026-10-09, embedded vs pi.dev (rows / tiered): anthropic 16/0 vs 17/1 (missing `claude-haiku-5-5`; `claude-sonnet-5-5` cacheRead 0.2 vs 0.1; `claude-sonnet-4-5` and `-20250929` embedded at `contextWindow` 1000000 and 600 images where pi.dev serves 200000 and 100); amazon-bedrock 183/23 vs 193/29 (six Haiku 5.5 rows missing); google 22/0 vs 22/6; opencode 76/0 vs 83/16; opencode-go 29/0 vs 31/7; openrouter 393/0 vs 403/81 (46 base-cost diffs); vercel-ai-gateway 248/0 vs 252/49; minimax 3/0 vs 3/1; github-copilot 32/11 vs 35/13. Kimi K3 already matches (cacheWrite 0 in `moonshotai.json`, `moonshotai-cn.json` and `kimi-coding.json`).

**Impact** — Offline or first-run sessions, and any provider whose overlay has not refreshed, undercount long-prompt cost on tiered models and overcount Sonnet 5.5 cache reads by 2x; Haiku 5.5 is unknown until the overlay arrives. More than a cost mismatch: such a session on Sonnet 4.5 can budget context to 1M and hit provider overflow errors. Online sessions are corrected by the runtime pi.dev overlay, which is why this stays low.

**Fix** — Run `cargo run -p xtask -- gen-catalogs` and review the diff, as `PROV-131` did. The `azure-openai-responses` stem 404s now (`PROV-145`), so do that row first or regenerate with `--only` excluding it.

> **UPDATE 2026-10-10** (from the `PROV-151` closure): `gen-catalogs --diff --only openai` also reports `openai/gpt-daybreak-blue-latest` gaining a 272k tier on pi.dev (`{inputTokensAbove: 272000, input: 8, output: 30, cacheRead: 0.8, cacheWrite: 10}` over the embedded flat 4/20/0.4/5). It is models.dev data, not in pi's generator at f1b2e77f5. `PROV-151` kept its write to `gpt-6.1-sol`, so this tier is still unembedded and belongs to this row's regeneration. Until then `gen-catalogs --check --only openai` reports that one field difference.

**Verify** — `gen-catalogs --check` reproduces pi.dev for every regenerated stem. A cost test prices a 150k-input Haiku 5.5 turn at the tier rate and Sonnet 5.5 cache reads at 0.1/M. `claude-sonnet-4-5` resolves with `context_window == 200000` and an image limit of 100.

## PROV-150 — `AzureOpenAiResponsesOptions` has no `reasoning_summary`, so pi's Azure summary-only request (effort `medium`, sampling resolved at `medium`) cannot be made

> **CLOSED 2026-10-10** (checked against pi f1b2e77f5). `AzureOpenAiResponsesOptions.reasoning_summary` and pi's summary-only `medium` fallback landed on `claude/zealous-bell-x0u1h0`. The **cyrup** paragraph below describes the state when this was filed and is no longer true. Evidence, per Verify clause, is in the table row.

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-10

> **Filed 2026-10-10 from the `PROV-146` closure** (on `claude/zealous-bell-x0u1h0` off `main` @ `62502ff8`, pi f1b2e77f5). It is the one case of pi `test/sampling-options.test.ts:135-215` that `PROV-146` could not port.

**upstream** — pi `api/azure-openai-responses.ts` @f1b2e77f5: `AzureOpenAIResponsesOptions` has `reasoningSummary?: "auto" | "detailed" | "concise" | null` (`:29`). `buildParams` computes `reasoningEffort = options?.reasoningEffort ?? (options?.reasoningSummary ? "medium" : undefined)` (`:224`). It sends `reasoning: {effort, summary: options?.reasoningSummary || "auto"}` plus `include: ["reasoning.encrypted_content"]` (`:226-234`), and resolves sampling at `reasoningEffort ?? "off"` (`:243-246`). A summary-only Azure request therefore gets effort `medium` and the model's `medium` sampling entry. pi `test/sampling-options.test.ts:198-215` asserts both (`effort` `medium`, the `medium` entry's `temperature`) for openai-responses and azure-openai-responses. The option and the fallback were already there at v0.83.0.

**cyrup** — `crates/cyrup-provider/src/api/azure_openai_responses.rs:63-72`: `AzureOpenAiResponsesOptions` has the four endpoint overrides and no `reasoning_summary`. The reasoning block (`:411-431`) keys on `clamp_thinking_level(model, opts.reasoning)` and always sends `summary: "auto"`. Sampling resolves at `opts.reasoning` (`:438`, through `openai_completions::apply_sampling_params`). The adapter's comments (`:15-18`, `:412-414`) call the field "typed-options-only (gap #11)", but no ledger row tracks it. The openai-responses adapter already has the field and both halves of the fallback (`api/openai_responses/options.rs:36`, `api/openai_responses/params.rs:263`, `:306-313`), from `PROV-045` and `CFG-104`.

**Impact** — A caller cannot pick the Azure reasoning summary (`detailed`/`concise`/`null`), and cannot ask for a summary with no effort. With reasoning off, a model with a `medium` sampling entry sends its `off` entry where pi would send `medium`. Low, because no cyrup caller sets a typed Azure summary today and the default `"auto"` matches pi's.

**Fix** — Add `reasoning_summary: Option<ReasoningSummary>` to `AzureOpenAiResponsesOptions` (reuse the openai-responses type). In `build_params`, mirror `openai_responses/params.rs:263-313`: open the reasoning arm on a summary-only request with effort `medium`, send the chosen summary (or `"auto"`), and pass `medium` to `apply_sampling_params` in that case.

**Verify** — Port pi `test/sampling-options.test.ts:198-215` for azure in `crates/cyrup-provider/src/tests/sampling_params.rs`. With reasoning off, `reasoning_summary: Some(Auto)` and a model with distinct `off`/`medium` sampling entries, the outgoing body carries the `medium` entry, `reasoning.effort == "medium"` and `include: ["reasoning.encrypted_content"]`. Without the summary, the `off` entry is sent and there is no `reasoning.summary`. A typed `Detailed` reaches `reasoning.summary`.

## PROV-151 — `openai.json` and `azure-openai-responses.json` lack pi's `gpt-6.1-sol` row (`12c416e1a`); only `openai-codex` was regenerated, by `CFG-102`

> **CLOSED 2026-10-10** (checked against pi f1b2e77f5). `openai/gpt-6.1-sol` came from `gen-catalogs --only openai`, with one unrelated tier hunk reverted; the Azure row was added by hand because pi.dev 404s that stem (`PROV-145`). The **cyrup** paragraph below describes the state when this was filed and is no longer true. Evidence, per Verify clause, is in the table row.

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-10

> **Filed 2026-10-10 from the `PROV-123`/`PROV-146` closure** (pi f1b2e77f5), which closed area 05's `CFG-102` by regenerating only the `openai-codex` catalog.

**upstream** — `12c416e1a` ("Add gpt-6.1-sol to the OpenAI, Azure OpenAI Responses, and OpenAI Codex providers"). In `scripts/generate-models.ts` @f1b2e77f5, `openai` gets it from `missingOpenAiModels` (row `:2872-2883`, api `openai-responses`, `contextWindow` 272000, `maxTokens` 128000, cost `:452` with the long-context tier), appended at `:2974-2978` when models.dev lacks it. `azure` copies every `openai`/`openai-responses` row (`:3308-3323`, no context override for this id). `applyThinkingLevelMetadata` gives both `off: null` (`:1045-1065`), and `openai` also gets `supportsToolSearch` (`OPENAI_TOOL_SEARCH_MODEL_IDS` `:367-379`, applied at `:900-912`). `test/supports-xhigh.test.ts:112-126` expects the model on `openai`, `azure` and `openai-codex`, with `off` unsupported.

**cyrup** — `grep -l '"gpt-6.1-sol"' crates/cyrup-provider/src/providers/catalog/*.json` matches only `openai-codex.json`. `openai.json` and `azure-openai-responses.json` have no such row, so `openai/gpt-6.1-sol` and `azure-openai-responses/gpt-6.1-sol` resolve only once the runtime overlay supplies them. The `openai` default stays `gpt-5.5` (pi `core/model-resolver.ts:24`), so no default depends on this.

**Impact** — Offline or first-run sessions cannot pick GPT-6.1 Sol on `openai` or Azure. Low: the runtime pi.dev overlay adds it for `openai`. **Azure is worse:** its overlay stem 404s since pi renamed the provider to `azure` (`PROV-145`), so the Azure catalog stays at the embedded floor until this or `PROV-145` lands.

**Fix** — `cargo run -p xtask -- gen-catalogs --only openai` and review the diff. It also adds `openai/gpt-6.1-sol` to the exact list in `tool_search_is_confined_to_the_openai_responses_catalog` (`crates/cyrup-provider/src/api/anthropic_messages/tests/catalog.rs:133`; its comment records the gap). For Azure, a scoped `--only azure-openai-responses` does not work: pi.dev serves that catalog as `azure` now. Land `PROV-145` first, or add the row by hand from the `openai` row as pi's `:3308-3323` does, and say so in the manifest.

**Verify** — `gen-catalogs --check --only openai` reproduces pi.dev. `openai/gpt-6.1-sol` and the Azure row resolve from the embedded catalog with `context_window == 272000`, `max_tokens == 128000` and `thinking_level_map.off == null`. Only the `openai` row has `supportsToolSearch`.

## PROV-152 — cyrup's Mistral transport speaks the old SDK's camelCase on the wire; pi's native transport speaks Mistral's snake_case

> **CLOSED 2026-10-10** (on `claude/provider-conformance-pi11`; checked against pi f1b2e77f5). **Investigation first, per key, from primary sources** (no Mistral key here, so no live run): Mistral's published OpenAPI (`https://docs.mistral.ai/openapi.yaml`, fetched 2026-10-10) and its official TS SDK (`mistralai/client-ts` `src/models/components/{chatcompletionstreamrequest,completionresponsestreamchoice,usageinfo,assistantmessage}.ts`, whose zod schemas `remap$` `maxTokens→max_tokens`, `finish_reason→finishReason`, `prompt_tokens→promptTokens`, `toolCalls↔tool_calls`) confirm the camelCase names were only ever the SDK's TypeScript surface. **Request:** `ChatCompletionRequest`, `AssistantMessage`, `ToolMessage`, `ImageURLChunk` and `ToolCall` are all `additionalProperties: false`, and `/v1/chat/completions` documents `422` (`HTTPValidationError`, pydantic shape) — so cyrup's `maxTokens`, `promptMode`, `reasoningEffort`, `toolChoice`, `promptCacheKey` (sent on every session turn), assistant `toolCalls`, tool `toolCallId` and chunk `imageUrl` (where `image_url` is *required*) were each a **422 per the published contract**, not a silently ignored key; this is the spec's statement, not an observation. `parallelToolCalls`/`topP`/`randomSeed`/`responseFormat`/`presencePenalty`/`frequencyPenalty`/`safePrompt` are never emitted by cyrup and reached the wire only via a payload hook. **Response:** `CompletionResponseStreamChoice` requires `finish_reason`, `DeltaMessage` carries `tool_calls`, `UsageInfo` requires `prompt_tokens`/`completion_tokens`/`total_tokens` — so cyrup read none of them: every turn would have ended "Mistral stream ended without a finish reason", streamed tool calls were dropped, usage (and, through the clamp, `cacheRead`) was 0. The cached-token probe (`getMistralCachedPromptTokens`, six spellings) matched pi already. Severity held at **high**, not raised to critical, because the 422 rests on the spec rather than an observed run; Confidence raised from inferred to confirmed-against-spec. Per-key table: session scratchpad `prov152-findings.md`.
>
> **Fix**, ported from pi @f1b2e77f5: new `crates/cyrup-provider/src/api/mistral_conversations/wire.rs` — `to_mistral_wire_payload` / `to_mistral_wire_message` / `to_mistral_wire_content_chunk` / `remap` port `toMistralWirePayload` / `toMistralWireMessage` / `toMistralWireContentChunk` / `remapMistralProperty` (`:386-451`) table for table, including `response_format.jsonSchema→json_schema` and `schemaDefinition→schema`; `mod.rs` applies it after `apply_on_payload`, so the hook still sees pi's SDK-style payload (`:148-151`, `:318`). `content.rs` reads `choice.finish_reason` (`:633`) and `delta.tool_calls` (`:710`); `finish.rs` `apply_usage` reads `prompt_tokens`/`completion_tokens`/`total_tokens` (`:617`, `:621`, `:625`). `messages.rs` adds pi's `prefix: false` on assistant messages (`:868`) and `index: 0` on request tool calls (`:864`), both added by the same `9dd90a497` and asserted by pi's replay fixture. The module doc that claimed the wire was camelCase is rewritten. Fixtures converted to the wire shape: `tests/{decode,stop_reason}.rs` streams, `truncation_parity.rs`'s Mistral streams, and `tests/{payload,reasoning,tools}.rs` (their `build_body` now returns the converted body, so the `is_none()` negatives check the snake_case key and are not vacuous).
>
> **Verify**, clause by clause: `tests/wire_keys.rs` — `prov152_request_body_is_mistral_snake_case_after_the_payload_hook` (pi "serializes SDK-style payloads…": request captured off a loopback socket from `ApiImpl::run`; the hook sees `maxTokens`/`promptMode`/`promptCacheKey`; the wire carries `max_tokens`, `prompt_mode`, `reasoning_effort`, `tool_choice`, `prompt_cache_key`, `top_p`, `random_seed`, `presence_penalty`, `frequency_penalty`, `parallel_tool_calls`, `safe_prompt`, `response_format.json_schema.schema`, `image_url`, and none of the camelCase keys at any remapped level); `prov152_replayed_tool_calls_and_tool_results_use_mistral_wire_keys` (pi's replay fixture verbatim: assistant `tool_calls` + `prefix:false` + `index:0`, tool `tool_call_id`); `prov152_a_snake_case_stream_yields_tool_calls_stop_reason_and_usage` (pi's "parses native thinking, text, fragmented tool calls, and cached-token usage": `ToolUse`, raw `tool_calls`, the tool call `{query:"pi"}`, usage input 7 / output 4 / cacheRead 3 / total 14); `prov152_a_snake_case_error_finish_reason_reaches_the_retryable_message` (pi `mistral-raw-stop-reason.test.ts`, all three cases: `finish_reason:"error"` → `"Provider stopped with: error (server error)"`, `is_retryable_assistant_error` true; `unmapped_error` false; `stop` clean). **Red before:** at HEAD all four fail — `max_tokens` is `Null`; `camelCase \`toolCalls\` on a message`; both stream tests get the error terminal "ended without a finish reason" with the tool call missing and usage 0. With the `content.rs`/`finish.rs`/`messages.rs`/`mod.rs` hunks reverted, 14 Mistral tests fail (the four above, `PROV-141`'s, seven converted `decode.rs` fixtures, `truncation_parity::complete_stream_still_reports_stop`); with `remap` made a no-op, 14 fail (both request tests plus the converted `payload`/`reasoning`/`tools` tests). Files restored byte-identical (sha256-checked) after each mutation. **Not met:** the optional live smoke run — no Mistral API key is available in this environment.

**Kind** upstream-drift · **Severity** ~~high~~ **CLOSED 2026-10-10** · **Effort** M · **Confidence** ~~inferred~~ confirmed against Mistral's published OpenAPI and official SDK, and pi read at the pin; not observed live · **Filed** 2026-10-10

> **Filed 2026-10-10 while reviewing `PROV-141`** (on `claude/provider-conformance-pi11`, cyrup base `99ebef0`, pi `f1b2e77f5`). It supersedes the v1.0.1 census note above that called the SDK → native switch "already the cyrup shape" (corrected in place).

**upstream** — `9dd90a497` ("fix(ai): replace Mistral SDK with native transport", in v1.0.1). At `f1b2e77f5`, `packages/ai/src/api/mistral-conversations.ts` reads the stream in Mistral's wire names — `chunk.usage.prompt_tokens` (`:617`), `choice.finish_reason` (`:633`, guarding `mapChatStopReason`), `delta.tool_calls` (`:710`) — and converts the request to them before sending: `toMistralWirePayload` / `toMistralWireMessage` (`:385-430`) remap `maxTokens→max_tokens`, `parallelToolCalls→parallel_tool_calls`, `promptMode→prompt_mode`, `toolCalls→tool_calls` and the rest. The camelCase names were the `@mistralai/mistralai` SDK's TypeScript surface, which the SDK translated on the wire; once pi dropped the SDK it had to translate itself. pi's `test/mistral-raw-stop-reason.test.ts` feeds `finish_reason`.

**cyrup** — `crates/cyrup-provider/src/api/mistral_conversations/` POSTs to `{baseUrl}/v1/chat/completions` directly (`mod.rs:3`) and its module doc states the wire JSON uses camelCase (`mod.rs:14`). It reads `"finishReason"` (`content.rs:44`), `"toolCalls"` (`content.rs:71`), `"promptTokens"` (`finish.rs:13`), and writes `"maxTokens"` (`payload.rs:64`) and `"toolCalls"` on assistant messages (`messages.rs:88`). `grep -rniE 'snake|camel|rename' crates/cyrup-provider/src/api/mistral_conversations/` finds no key conversion. The module's tests feed camelCase fixtures, so they pass.

**Impact** — Inferred, not observed: against Mistral's REST API a streamed `finish_reason` is never matched, so no stop reason is mapped (and `PROV-141`'s retryable `(server error)` message cannot be reached in production); streamed `tool_calls` deltas are dropped; prompt usage reads 0; `max_tokens`, `prompt_mode` and `parallel_tool_calls` are not honoured, and replayed assistant tool calls are not recognised by the API. Depending on how Mistral treats unknown request keys this ranges from silently degraded to every Mistral turn failing. Rated high because it would break the provider's user-facing flow; downgrade if a live run shows Mistral tolerates it.

**Fix** — Port `toMistralWirePayload`/`toMistralWireMessage` (or emit snake_case directly) for the request, and read `finish_reason`, `tool_calls`, `prompt_tokens` (and the other usage/delta keys pi reads) in `content.rs`/`finish.rs`. Convert the module's camelCase test fixtures to Mistral's wire shape, porting pi's fixtures from `test/mistral-*.test.ts` at the pin.

**Verify** — A recorded-shape stream with `finish_reason: "error"` reaches `PROV-141`'s retryable message through the decoder; a `tool_calls` delta yields a tool call; `usage.prompt_tokens` is counted; the request body carries `max_tokens` and assistant `tool_calls`, never the camelCase keys. One live smoke run against Mistral if a key is available. Red before.
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
- **`bun/cli.ts:13` `process.env.PI_CODING_AGENT = "true"`** — already filed and deliberately **not** re-filed here: `04-cyrup-tools.md:581-595` `TOOL-031` and `PARITY-GAPS.md:132-135` `PB-5` already cite `cli.ts:13` and `rpc-entry.ts:7`. The v0.84.1 `AI_AGENT` half is in the same item.

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
