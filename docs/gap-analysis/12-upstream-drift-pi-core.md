# 12 — pi core drift since cyrup HEAD

This area tracks behavior that `pi/` core (packages `ai/`, `agent/`, `coding-agent/`, `tui/`) has changed or added since cyrup's port was written, plus stale ports and defects the port carries against it — provider/API wire behavior, session and compaction entry formats, the extension context surface, the model resolver, retry classification, process lifecycle, session export, and the tests that pin any of it. Items own the surfaces that areas 01–08 do not.

> **Re-audited 2026-08-12, cyrup HEAD `a9000b1`** (docs-only; last **code** commit `04c1ba2`, working tree clean — ~15 commits past this file's `1806375` baseline), **measured against pi `v0.84.1`** (the ported baseline is `v0.83.0`; delta 627 files, +52291/−17556). Sibling upstreams are pinned elsewhere and are not this area's: pi-subagents `v0.47.1`, pi-permission-system `v0.8.0`, pi-intercom `v0.10.1`.
>
> **What changed in this pass.** **Five items closed** by code that landed after this file's baseline — `DRIFT-012` (`StopReason::Pending`/`Deferred` + `raw_stop_reason`, verified across all six producer sites), `DRIFT-017` (bash session env scrub + repopulate), `DRIFT-021` (`supports_finish_reason`, verified through to its **consumer**), `DRIFT-034` (WIT world `@0.4.0` with a real byte-identity test), and `DRIFT-026`, which this file already recorded as closed in its status table while its headline sentence and its Open-items row still called it the area's only `high`. **That contradiction is now struck: this area has no `critical` and no `high`.** All thirteen prior closures were re-derived from both sides with **zero overturns**.
>
> **One item withdrawn and one merged away.** `DRIFT-037` is **refuted on both sides** — `first_kept_entry_id` is already `Option` with elision at `entry.rs:75-79`, and `retainedTail` belongs to the harness-v2 entry shape cyrup does not port, and predates `v0.83.0` anyway; its residue (harness-v2 session interop) is folded into `DRIFT-040`. `DRIFT-038` is a **duplicate of `CFG-018`** (same function, same defect, same fix) and is merged into it; `CFG-018` keeps the ID. Both keep their numbers in the status table so the calls can be re-audited.
>
> **Eight new items filed** — `DRIFT-041` (session HTML export is a 131-line text dump against pi's 5,021-line templated document), `DRIFT-042` (`/login` opens no browser and its "Cmd+click" hint is not a link), `DRIFT-045` (Ctrl+V never reads clipboard **text**), `DRIFT-046` (`normalizeWindowsShellPath` unported), `DRIFT-047` (`packages/telemetry` and the `pi.ai.request` span contract), `DRIFT-048` (Google converter reads the id-inclusion rule off the **source** message's model, not the target model), `DRIFT-049` (SIGTERM/SIGHUP never disposes the runtime, and in RPC mode never exits), `DRIFT-050` (`CYRUP_TELEMETRY=` empty is an explicit OFF upstream and a silent no-op here). Two further candidates were **rejected as duplicates** — a detached-child registry (`SEAM-S03`) and `AI_AGENT` (`PB-5`) — and are recorded in `## Coverage` so nobody re-derives them.
>
> Open in this area: **33** — 0 critical, 0 high, 12 medium, 21 low.

> ### REPAIR PASS — 2026-08-12 (second pass, same day), applying the completeness critique
>
> Read-only static repair. No Rust or TypeScript touched; upstream re-read at both tags with
> `git -C pi show <tag>:<path>` and `git ls-tree`/`git grep <tag>`.
>
> **1. `DRIFT-009` was factually wrong about upstream and produced a lossier Fix than the item that
> already owns the work.** It asserted "There is still **no in-tree regeneration source**" and
> proposed seeding the embedded floor from the pi.dev artifact as "the only source still available".
> Both halves are false. `packages/ai/scripts/generate-models.ts` (2 733 lines) exists at **both**
> tags, alongside `model-data.ts`, `models-dev-reasoning-options.ts` and `check-model-data.ts`; pi's
> root `package.json:24-30` exposes `generate:models`, `hydrate:model-data`, `generate:model-catalog`,
> `diff:model-catalog` and `check:model-catalog`; `scripts/diff-model-catalog.mjs` is a purpose-built
> catalog drift-differ and `scripts/publish-model-catalog.mjs` documents the artifact schema. Only the
> **output** (`packages/ai/src/providers/data/`) is gitignored (`.gitignore:11`). `PROV-018`'s Fix
> ("runs pi's `npm run generate-models`") is the correct one; `DRIFT-009` now **defers to it** and
> keeps only the four-catalog count. See `## Coverage` for the reusable lesson about what a sweep is
> allowed to skip — this file's own dismissal of root `scripts/` is how the generator got lost.
>
> **2. Nine items rested on a commit hash rather than a two-sided read, inside a count where the
> other 417 are held to a two-sided standard.** Seven were re-derived this pass with two-sided
> file:line evidence at the ported tag (`DRIFT-016`, `DRIFT-018`, `DRIFT-019`, `DRIFT-030`,
> `DRIFT-031`, `DRIFT-032`, `DRIFT-035`) — and **six of the seven turned out to be misclassified**:
> the upstream code they cite already existed at **v0.83.0**, so they are port omissions, not
> version lag, and will **not** be swept up by a rebase. Kinds corrected accordingly. The two that
> could not be re-derived cheaply (`DRIFT-023`, `DRIFT-040`) are now flagged **lead — not yet
> evidenced** and are excluded from the item count as trackers.
>
> **3. Four items propose no work and say so** — `DRIFT-022`, `DRIFT-023`, `DRIFT-032`, `DRIFT-040`.
> They are marked **tracker**, keep their IDs and bodies, and are **excluded from the severity
> counts**: an item that proposes no work is bookkeeping, not backlog.
>
> **4. This area is partly a duplicate index, and the duplication is worse than the ledger records.**
> A full cross-file ID census this pass finds **20 of 33** items duplicating an item owned by areas
> 01–08 or by a `PARITY-GAPS` entry (the ledger's cluster F4 lists 12). Every duplicate now carries an explicit
> `duplicate-of: <ID>` marker in its row so a recount can produce a deduplicated figure alongside the
> raw one. **No ID was renumbered, merged or deleted.** Two duplicates the ledger's F4 table misses
> are recorded here for the first time: `DRIFT-046` → `TOOL-036` and `DRIFT-009` → `PROV-018`.
>
> **5. One severity corrected.** `DRIFT-049` medium → **high**: it is the same defect as `SEAM-047`,
> which area 08 and `PARITY-GAPS` **PB-30** both rate high. A defect cannot carry two severities, and
> a high that reads as medium falls off a planner's high list.
>
> **6. One item filed** from the `packages/coding-agent/src/bun/` sweep this area never ran —
> `DRIFT-051` (`process.title`'s role suffix). The rest of `bun/` is adjudicated N/A in `## Coverage`,
> which closes the "zero mentions anywhere in the directory" hole the critique named.
>
> **Open after repair: 34 IDs = 30 severity-bearing (0 critical, 1 high, 11 medium, 18 low) + 4
> trackers.** Prior figure was 33 IDs / 33 severity-bearing (0/0/12/21); the delta is +1 filed
> (`DRIFT-051`, low), 1 medium→high (`DRIFT-049`), and 4 rows moved out of the severity count as
> trackers (all four were low).
>
> **Deduplicated, this area uniquely owns 14 IDs — 0 high, 8 medium, 6 low:** medium `DRIFT-004`,
> `DRIFT-013`, `DRIFT-014`, `DRIFT-028`, `DRIFT-029`, `DRIFT-033`, `DRIFT-041`, `DRIFT-048`; low
> `DRIFT-010`, `DRIFT-036`, `DRIFT-042`, `DRIFT-045`, `DRIFT-050`, `DRIFT-051`. **The one high
> (`DRIFT-049`) is a duplicate of `SEAM-047`, so it must be scheduled once, in area 08.** The other
> 20 IDs restate work an owning area is already tracking. Use the deduplicated figure for planning
> and the raw figure only for ID bookkeeping — see `## Coverage → Duplication census`.

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
> **Area 12 — recount: 34 rows → 16 open (0 critical · 0 high · 7 medium · 9 low) + 4 trackers.** The
> area's only high, `DRIFT-049`, was already FIXED via `SEAM-047` — its *kind* cell said so while its
> *severity* cell still read `high`, which is why two recounts carried a phantom high. **Area 12 now
> has no open high.**
>
> **FOUR MORE KIND CORRECTIONS, all in the same direction: items filed as `upstream-drift` that are
> actually `not-ported` and that no rebase would ever have swept up.**
>
> - **`DRIFT-013`** — filed against openai-completions.ts:1478-1485 @v0.84.1, but
>   `git show v0.83.0:packages/ai/src/api/openai-completions.ts` has the identical disjunction at
>   `:1427-1435` with `isZai` at `:1435` — **inside the ported baseline**.
> - **`DRIFT-029`** — filed against pi commit `2efa728d`, but `git grep -n _bashAbortController v0.83.0`
>   shows the `Set`, its add, its delete-in-finally, the spread-copy abort and the `size > 0` getter
>   **all present at the ported tag**. A port omission, not version lag.
> - **`DRIFT-046`** — not a plain duplicate of `TOOL-036` after all: a SECOND live instance existed in
>   `crates/cyrup-config/src/paths.rs`, the 1:1 port of the very function upstream applies the rule
>   inside, created AFTER this item was written by `CFG-025`/`CFG-036`. cyrup ended sweep 1 with the
>   rule in one copy of the normalizer and not the other. **Its own Fix sentence names the wrong
>   crate** — `normalize_path` lives in cyrup-config, not cyrup-resources.
> - **`DRIFT-033`** — its refuter caveat was CORRECT: not a one-line assignment, the prerequisite was
>   modelling pi's two prompt slots, and both parts landed together.
>
> These join the `DRIFT-014`/`018`/`019`/`030`/`031`/`032` corrections the repair pass already made.
> **The pattern is now measured, not suspected: this file's default assumption that a missing feature
> is version lag is wrong more often than it is right, and every remaining `upstream-drift` row should
> be re-derived at `v0.83.0` before it is scheduled.**
>
> **`DRIFT-001`'s status row still points at `DRIFT-033` as an open residual.** `DRIFT-033` is closed,
> so `DRIFT-001` reads as fully closed. **`DRIFT-049`'s Bookkeeping note** (SEAM-S02 is stale and
> should be re-audited as closed) is already reflected in area 08's status table and can be struck.
> **`DRIFT-051`** carries `SEAM-070`'s caveat across: on macOS `pthread_setname_np` does not change
> what `ps -o comm=` prints, so its Verify line holds verbatim only on Linux.
>
> **`DRIFT-014` is the one backlog row whose Verify cannot be satisfied under the standing
> no-test-execution rule** — it requires observing what string reqwest/hyper actually emits on a
> failed DNS lookup. Adding pi's seven Node-shaped literals alone would ship a test that passes while
> the real Rust failure mode stays unretried. That warning is now in the row itself so the next agent
> neither rediscovers it nor closes on the literals.
>
> **`DRIFT-048`'s own observation deserves promoting into Coverage as a recurring class**: "a
> name-present, argument-wrong bug, invisible to an absence sweep". It is the second such finding in
> this file after `DRIFT-026`, **in the same function**.


## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| DRIFT-001 | **closed** | Message-anchored tool loading. Re-verified at HEAD: `cyrup-ext/src/wrapper.rs:130-145` computes `additive_delta(before, after)` then assigns `result.added_tool_names = union_in_order(...)` (`:143`); the producer-only contract is restated at `cyrup-ext/src/facade.rs:381` ("a tool never sets the field"), and the consumer model still carries it (`cyrup-agent/tests/tool_result_model.rs:151`). Upstream `pi/packages/ai/src/utils/deferred-tools.ts` is **byte-unchanged** across `v0.83.0..v0.84.1`, so the closure cannot have gone stale. Residual system-prompt half is DRIFT-033. |
| DRIFT-002 | **closed** | `normalize_tool_call_id`. `cyrup-provider/src/api/openai_completions.rs:738-762` re-diffed clause-for-clause (pipe split, per-part sanitize, empty-`item_id` collapse, `<= 40` early return, 8-char hash, `40 - hash.len() - 1` floor with `.max(1)`, openai 40-char truncation), parity test at `:2257-2272`. **Citation repointed:** at `v0.84.1` the upstream function is `pi/packages/ai/src/api/openai-completions.ts:1054-1078`, not `:1006-1030` (that is the `v0.83.0` span). The file changed `+77` in the delta; this function did not. |
| DRIFT-003 | **closed** | `content_block_start` payload. `cyrup-provider/src/api/anthropic_messages.rs:1529` dispatches to `process_block_start` (`:1564-1670`), whose four arms seed text from `content_block.text`, thinking from **both** `thinking` and `signature`, and redacted-thinking `data`→signature. Guard test at `:2709-2749`. Upstream `anthropic-messages.ts:588-604`; the git diff shows this is exactly the change that landed in the delta (`text: ""` → `event.content_block.text ?? ""`). |
| DRIFT-004 | **partially closed** | `user_bash` from RPC is done; the `UserBashEventResult.operations` backend seam is not — all ten `BashOperations` hits in cyrup are doc comments. Remainder open below. |
| DRIFT-005 | **closed** | Summarization isolation. `cyrup-session/src/compaction/summarize.rs:233-246` — `cache_retention: Some(CacheRetention::None)`, `session_id: Some(gen_session_id())`, routed through `retry_assistant_call`. Upstream `completeSummarization` at `pi/packages/coding-agent/src/core/compaction/compaction.ts:562-581` read at `v0.84.1`: clause-for-clause match. |
| DRIFT-006 | **closed** | Summarization retry observability. Four production `with_observer(retry_observer)` sites survive at `cyrup-session-svc/src/session.rs:1410, :1713, :2030, :4252` (line numbers moved from `1278/1547/1856/3722` — the file grew, the wiring is intact). Both halves re-verified: `cyrup-provider/src/utils/retry.rs:190-216` maps arm-for-arm onto `pi/packages/ai/src/utils/retry.ts:172-195` at `v0.84.1`, including the `if (lastRetry)` gate. |
| DRIFT-007 | **closed** | Runtime catalog overlay. `cyrup-provider/src/remote_catalog.rs:68` `https://pi.dev`, `:71` 4h interval, ETag lifecycle at `:520`/`:588`/`:614`/`:639`. Upstream `remote-catalog-provider.ts:6-7` at `v0.84.1`. Overlay only — does **not** close DRIFT-009's offline floor. |
| DRIFT-008 | **closed** | Thinking ladder. `cyrup-core/src/message.rs:30-39` `ThinkingLevel { Minimal, Low, Medium, High, Xhigh, Max }` with `Max` declared last, `:44-57` `ModelThinkingLevel`, round-trips `:59-90`, asserted `:1010-1012`. Upstream `pi/packages/ai/src/types.ts:82-83` — same ladder, same order, unchanged in the delta. *(Line cite corrected: the enum starts at `:30`, not `:38`.)* |
| DRIFT-009 | still-open · **duplicate-of: `PROV-018`** | Embedded catalog floor. The item's own count is now stale (31 → 35 shipped); upstream ships 39. **Rewritten this repair pass:** its "no in-tree regeneration source" claim is FALSE (`packages/ai/scripts/generate-models.ts` exists at both tags) and its pi.dev-seeding Fix was lossier than `PROV-018`'s. Now defers to `PROV-018` (tooling half) and `PROV-004`/`PROV-039` (provenance half); retains only the four-missing-catalog count, which is `PARITY-GAPS` VL-P25's data half. Restated below. |
| DRIFT-010 | still-open | `get_available_thinking_levels` RPC verb. **Evidence corrected** — the underlying method exists; only the RPC arm is missing. |
| DRIFT-011 | **closed** | `usage` on tool results / compaction / branch summaries. `cyrup-core/src/message.rs:460`, `cyrup-session/src/entry.rs:87-89` and `:101-103`, both elided. **Citation repointed:** `packages/agent/src/harness/types.ts` no longer exists at `v0.84.1`; the coding-agent shape cyrup ports is `session-manager.ts:76-77` / `:90`, and the harness-v2 shape is `harness/session/types.ts:44-58` — both carry `usage?`. Missing consumer is DRIFT-031. |
| DRIFT-012 | **closed** `04c1ba2`-era | `StopReason` now carries `Pending` (`cyrup-core/src/message.rs:166`) **and** `Deferred` (`:188`, absorbing `v0.84.0`'s new `"deferred"` with provenance at `:173-174`); `raw_stop_reason: Option<String>` at `:492`. Verified by **producer sweep**, not declaration: assigned in all six wire APIs (`anthropic_messages.rs:1545`, `bedrock_converse_stream.rs:1895`, `mistral_conversations.rs:882`, `google_generative_ai.rs:1066`, `openai_completions.rs:1641`, `openai_responses.rs:1262`/`:1440`) — exactly the upstream producer set (`git grep rawStopReason v0.84.1`). Upstream `types.ts:391`, `:527-531`. **The ledger's promoted `PROV-010/AGENT-014/DRIFT-012` high row is stale and must be struck.** |
| DRIFT-013 | still-open | Z.AI `max_completion_tokens`. One-token fix; upstream's trailing `\|\| isZai` confirmed at `openai-completions.ts:1478-1485`. Uniquely owned by this area — no duplicate elsewhere. *(Table-cell pipes escaped this repair pass; the raw `\|\|` was splitting this row into five columns in every Markdown renderer.)* |
| DRIFT-014 | still-open | DNS/transport literals. **Kind corrected to `not-ported`** — `git diff v0.83.0..v0.84.1 -- packages/ai/src/utils/retry.ts` is empty, so all seven literals existed at the ported baseline. This will not be swept up by a rebase. |
| DRIFT-015 | **partially closed** · **duplicate-of: `EXT-019`** (markdown-transformer half; `TUI-034` carries the renderer half) | The `019e4ad6` native-provider half landed: `cyrup-ext/wit/world.wit:277-278` declares `register-provider`/`unregister-provider`. `scoped-models`, `render-options`/`output-pad` and markdown transformers are still absent from the world. Remainder open below. Overlaps `PARITY-GAPS` VL-P21. |
| DRIFT-016 | still-open · **kind corrected** · **duplicate-of: `SESS-019`** | `Current date:` in the system prompt. **Re-derived two-sided this repair pass:** `git grep 'Current date' v0.83.0 -- packages/coding-agent/src` returns **nothing** — the removal predates the *ported* baseline, so this is `stale-port` (cyrup carries behaviour upstream deleted), not `upstream-drift`. `SESS-019` (`03-cyrup-session.md:382`, medium) owns the same footer plus the extra leading newline and the `project_context` wording. |
| DRIFT-017 | **closed** | Bash session env. `cyrup-tools/src/tools/bash.rs:153` runs the unconditional scrub (`session_env_scrub_keys()`, the load-bearing half) **ahead of** the `expose_session_environment` gate at `:154`; `:156-176` repopulate the five vars; `spawn_hook` applied **last** at `:179-183`. Upstream `resolveSpawnContext` at `v0.84.1` `bash.ts:163-189` is the same delete→repopulate→hook order, guideline gate at `:334`. cyrup's scrub list is a deliberate, documented **superset** (`CYRUP_*` **and** `PI_*`, `cyrup-tools/src/config.rs:41-48`). |
| DRIFT-018 | still-open · **kind corrected** · **duplicate-of: `PROV-011`** | Constrained sampling. **Re-derived two-sided this repair pass:** `packages/ai/src/api/constrained-sampling.ts` (148 lines, 7 exports) exists at **`v0.83.0`** and is already imported there by `anthropic-messages.ts:40`, `azure-openai-responses.ts:18`, `bedrock-converse-stream.ts:57`, `google-shared.ts:8` and `mistral-conversations.ts:28` — so this is `not-ported`, not `upstream-drift`, and a rebase will never sweep it up. `PROV-011` (`01-…:154`, medium, four affected sites) is strictly broader and owns the fix; `TOOL-016` and `EXT-024` own the tool-model and WIT halves. |
| DRIFT-019 | **partially closed** · **kind corrected** · **duplicate-of: `PROV-014`** | The `pi-messages` wire API and Radius OAuth landed (`cyrup-provider/src/api/pi_messages.rs`, `auth/oauth/radius.rs`, id registered at `auth/oauth/load.rs:59`). The **provider registrations** are still absent. **Re-derived two-sided this repair pass:** `git ls-tree v0.83.0 packages/ai/src/providers/` already lists `radius.ts`, `radius-config.ts`, `qwen-token-plan.ts` and `qwen-token-plan-cn.ts` — three of the four missing registrations are `not-ported` at the ported baseline; only `qwen-token-plan-individual.ts` is genuine `v0.84.1` drift (`PARITY-GAPS` VL-P2). The critic's independent re-derivation is recorded so it is **not re-litigated**: `grep -rn "radius\|qwen" crates/cyrup-provider/src/providers/all.rs` is empty across the **whole file**, not merely `:145-243`, so the `fleet!`-macro grep trap does not rescue this — the gap is real. `PROV-014` (`01-…:168`, medium) owns it. |
| DRIFT-020 | still-open · **duplicate-of: `PROV-024`** (`PROV-033` owns the openai-responses half) | `sendSessionIdHeader` vs `sessionAffinityFormat`. **Strengthened:** upstream's union is now three-valued (`"openai" \| "openai-nosession" \| "openrouter"`, `types.ts:112`), which a bool cannot express. |
| DRIFT-021 | **closed** | Streams without `finish_reason`. Whole chain present *including the consumer*: declared `compat.rs:83-86`, resolved `:216`, detected `:362`, declared-over-detected `:407-409`, **consumed** at `openai_completions.rs:1547-1560`, tests `:2633`/`:2691`. Upstream `openai-completions.ts:578-585`; the git diff shows this is the change that landed in the delta. |
| DRIFT-022 | **TRACKER** (excluded from the severity count) · **duplicate-of: `SEAM-051`** (`TUI-019` + `CFG-021` carry the renderer and settings halves) | Alternate-screen mode. Its Fix is "Do **not** implement yet" and its Verify is "n/a while tracking", so it proposes no work and is now a tracker. **Item text corrected** — upstream renamed the concept to **TUI mode**, the flag is `--tui-mode` (independently confirmed as the *only* `args.ts` flag added in the whole delta), `--alt` was deleted, and the surface grew by 1047 lines. Same gap as `PARITY-GAPS` VL-P19. |
| DRIFT-023 | **TRACKER** (excluded from the severity count) · **LEAD — not yet evidenced** · **duplicate-of: `CFG-020`** | `ModelRegistry` → `ModelRuntime`. Still **not re-derived**: neither side was re-read in the 2026-08-12 pass, and the repair pass did not re-derive it either (the cheap re-derivations were spent on the six items whose *kind* was in doubt). It proposes no work and says so, so it is now a tracker rather than a backlog row, and it sits in `## Leads — not yet evidenced` outside the item count. `CFG-020` (`05-…:335`, "No `ModelRuntime` type and no availability snapshot") owns the cyrup-side question. |
| DRIFT-024 | still-open · **duplicate-of: `SESS-013`** | AGENTS.md double-load in nested worktrees. Upstream `findShadowedContextFile` confirmed declared at `resource-loader.ts:100`, called at `:136`. `SESS-013` (`03-…:172`, medium) is strictly better evidenced — it names both load-bearing guards (`:108` worktree-prefix, `:113` canonical `.git`) and the `isShadowed` gate at `:140-142`. |
| DRIFT-025 | still-open · **duplicate-of: `CFG-017`** | `${@:-default}` prompt-template defaults. Upstream regex alternation `\$\{(\d+\|ARGUMENTS\|@):-([^}]*)\}` confirmed at `prompt-templates.ts:74-80`. `CFG-017` (`05-…:419`) owns `cyrup-resources`. |
| DRIFT-026 | **closed** `513e45a` | Google converter signed-empty-block skip. Re-derived hardest of the set: `cyrup-provider/src/api/google_generative_ai.rs:745` resolves the signature **before** the skip at `:751`; the thinking arm mirrors it (`:769`/`:772`); the cross-provider `else` keeps the unconditional skip at `:785-787`. `git diff v0.83.0..v0.84.1 -- packages/ai/src/api/google-shared.ts` shows exactly this hoist landing upstream. **This file's headline sentence and its Open-items row both claimed it was still the area's only `high`; both are now struck.** The residual defect in the *same function* is the new DRIFT-048. |
| DRIFT-027 | still-open · **duplicate-of: `PROV-025`** | `deferredToolsMode: "kimi"`. **Strengthened:** upstream actually *consumes* it at `openai-completions.ts:728` and `:1272`, not merely declares it. `PROV-025` (`01-…:338`) records the same kind correction (`types.ts:567` @v0.83.0 → port bug, not lag) and a fuller fix. |
| DRIFT-028 | still-open, **halved** | OpenRouter cache breakpoints. Half (a) — tool-result role — confirmed. Half (b) — `~anthropic/*` aliases — **refuted and dropped**: pi's own runtime detector at `openai-completions.ts:1497` is byte-equivalent to cyrup's, so pi misses those ids identically. A shared upstream bug is not a cyrup divergence. |
| DRIFT-029 | still-open | Single bash cancel slot. Upstream's `Set` confirmed at `agent-session.ts:339` (add `:2781`, delete-in-finally `:2805`, iterate-a-copy `:2843`, `size > 0` `:2850`). |
| DRIFT-030 | still-open · **kind corrected** · **duplicate-of: `PROV-021`** | `ANTHROPIC_AUTH_TOKEN`. **Re-derived two-sided this repair pass:** the constant and both carve-outs exist at **`v0.83.0`** — `env-api-keys.ts:29` (`ANTHROPIC_AUTH_TOKEN_ENV`), `:76` (returned first for anthropic), `:147` (excluded from api-key display, `:149` at v0.84.1), `providers/anthropic.ts:5`/`:21`. So `not-ported`, not `upstream-drift`. `PROV-021` (`01-…:226`, medium) already carries exactly this correction and the full fix sketch. |
| DRIFT-031 | still-open · **kind corrected** · **duplicate-of: `PROV-036`** | Usage cost breakdown. **Re-derived two-sided this repair pass:** `getUsageCostBreakdown` is declared at `usage-totals.ts:37` at **`v0.83.0`** with a live consumer at `interactive-mode.ts:5665` (`:6001` at v0.84.1, import `:94`→`:101`) — `not-ported`, not `upstream-drift`. `PROV-036` (`01-…:550`, low) owns it and its evidence is better: it also records that the *totals* half **is** ported at `cyrup-tui/src/status.rs:170`. |
| DRIFT-032 | **TRACKER** (excluded from the severity count) · **kind corrected** · **duplicate-of: `EXT-027`** | llama.cpp router / HF model search. **Re-derived two-sided this repair pass** (the six commit hashes are replaced by file evidence): `packages/coding-agent/src/extensions/llama/{client,huggingface,index,provider,ui}.ts`, `docs/llama-cpp.md` and `test/llama-extension.test.ts` all exist at **`v0.83.0`** and are byte-present at `v0.84.1` — `not-ported`, not `upstream-drift`. Its Fix is "defer until DRIFT-019 and DRIFT-009 are settled" and its Verify is "n/a until scoped", so it proposes no work and is now a tracker. `EXT-027` (`06-…:601`) owns it. The grep trap stands: cyrup's `huggingface` hits are a provider **catalog** registered via the fleet macro, not an integration. |
| DRIFT-033 | still-open | Mid-run tool addition never reaches the system prompt. **Fix caveat added** — modelling pi's two prompt slots comes first; this is not a one-line assignment. |
| DRIFT-034 | **closed** | WIT world version. Both copies read `package cyrup:ext@0.4.0;` at line 18 and `diff` reports them byte-identical, and the invariant is now **enforced by a real test** — `cyrup-ext/tests/wit_world_sync.rs:20-24` ties `HOST_WORLD` to the `package` line of *both* paths, with the build-identity guard at `:137-149`. Closed on both halves the item asked for. |
| DRIFT-035 | still-open · **duplicate-of: `SESS-019`** | TEST DEFECT: prompt tests pin DRIFT-016. Both assertions survive unannotated at `prompt/tests.rs:82` and `:113`. **Re-derived this repair pass:** the footer is absent from `packages/coding-agent/src` at **both** tags (only historical `test/fixtures/*.jsonl` transcripts contain the string), so the assertions pin a pre-baseline stale port, not lag. Same footer as `SESS-019`; must land in the same change as `DRIFT-016`. |
| DRIFT-036 | still-open | TEST DEFECT: `settle()` fixed 50 ms sleep, verbatim unchanged at `summarization_retry_events.rs:98-104`. |
| ~~DRIFT-037~~ | **refuted / withdrawn** | Compaction `retainedTail` / required `firstKeptEntryId`. **Refuted on both sides.** (1) cyrup already declares `#[serde(default, skip_serializing_if = "Option::is_none")] first_kept_entry_id: Option<EntryId>` at `cyrup-session/src/entry.rs:75-79`, four lines above the `usage` sibling the filing reasoned from — the pattern *is* applied. (2) `retainedTail` lives on the **harness-v2** `CompactionEntry` (`harness/session/types.ts:44-51`), which has no `firstKeptEntryId` at all, while the coding-agent format cyrup actually ports still declares `firstKeptEntryId: string` **required** (`session-manager.ts:69`, `compaction/compaction.ts:90`). (3) It is not delta drift: `git grep -l retainedTail v0.83.0` already lists it. Residue — interop with harness-v2-written sessions — folded into DRIFT-040. ID retained so this call can be re-audited. |
| ~~DRIFT-038~~ | **superseded by `CFG-018`** | `resolve_scope` exact-reference short-circuit. The defect is **real and confirmed on both sides** (`cyrup-config/src/model.rs:256-280` vs `model-resolver.ts:290-307`), and the auditor's evidence correction stands — `find_exact_model_reference_match` **does** exist in the workspace, at `cyrup-tui/src/model_selector.rs:380` with the ambiguity rule tested at `:651`, and `resolve_scope` still does not call it. But this is the same function, defect, severity, effort and fix as `CFG-018` (`05-cyrup-config-and-resources.md:253-268`), filed in the area that owns `cyrup-config`. Carrying it twice inflates the backlog and risks two people fixing one line. **Work it as `CFG-018`**; fold the `model_selector.rs:380` pointer into that item. |
| DRIFT-039 | still-open · **duplicate-of: `AGENT-019`** | TEST DEFECT: parallel-tool test asserts wall-clock and completion order. All three assertions unchanged at HEAD. **Literally the same test** as `AGENT-019` (`02-…:677`) — `crates/cyrup-agent/tests/agent_loop.rs:327`. Fix it once; this item's body carries the better fix sketch (the `agent-loop.test.ts:589-612` rendezvous), so merge that into `AGENT-019` rather than working both. |
| DRIFT-040 | **TRACKER** (excluded from the severity count) · **LEAD — not yet evidenced** · **duplicate-of: `PARITY-GAPS` VL-P22** | pi harness-v2 rearchitecture. Absorbs DRIFT-037's residue (harness-v2 session-format interop). The three load-bearing claims — the `agent-harness.ts` rewrite (`+420/−996`), `docs/harness-v2.md` (`+2124/−367`) and the sqlite-node rebuild (`+12598/−3479`) — were **carried forward unverified** in the prior pass and are **still** unverified: this repair pass re-derived the six items whose *kind* was in doubt and did not spend the budget here. It proposes no work and says so, so it is a tracker in `## Leads — not yet evidenced`, outside the item count. |
| DRIFT-041 | **new** | Session HTML export is a 131-line text dump against pi's 5,021-line templated document. |
| DRIFT-042 | **new** | `/login` opens no browser and its click hint is not an OSC-8 link. |
| ~~DRIFT-043~~ | **rejected — not filed** | Detached-child registry. Duplicate of `SEAM-S03`, and its lead mechanism claim is wrong. See `## Coverage`. |
| ~~DRIFT-044~~ | **rejected — not filed** | `AI_AGENT` never stamped. Duplicate of `PB-5`, which is strictly broader. See `## Coverage`. |
| DRIFT-045 | **new** | Ctrl+V never reads clipboard **text**, only images. |
| DRIFT-046 | **new** · **duplicate-of: `TOOL-036`** | `normalizeWindowsShellPath` unported. **Duplicate found this repair pass and missing from the ledger's F4 table:** `TOOL-036` (`04-…:462`, low) covers the same `paths.ts:67-73` / `:83-85` port *and* the `os.homedir()` half `DRIFT-046` omits, in the crate (`cyrup-tools/src/path.rs`) that actually owns the normalizer. `PARITY-GAPS.md:652` already couples the two under the same Windows-is-a-target question. |
| DRIFT-047 | **new** · **duplicate-of: `PARITY-GAPS` VL-P5** | `packages/telemetry` and the `pi.ai.request` span contract absent. The item's own refuter note already says to resolve by **extending VL-P5**, not opening a second L-sized workstream; the marker makes that machine-readable. Only the span schema is incremental. |
| DRIFT-048 | **new** | Google converter reads the tool-call-id rule off the **source** message's model, not the target model. Uniquely owned here. |
| DRIFT-049 | **new** · **severity medium → high** · **duplicate-of: `SEAM-047`** | SIGTERM/SIGHUP never disposes the runtime; in RPC mode it never exits at all. **Raised to high this repair pass:** area 08 rates `SEAM-047` (`08-…:105`) high and `PARITY-GAPS` **PB-30** names both IDs at high — a defect cannot carry two severities, and a high that reads as medium falls off a planner's high list. `SEAM-008` and `SEAM-059` are the sibling signal-path items. Schedule once, in area 08. |
| DRIFT-050 | **new** | `CYRUP_TELEMETRY=` (set but empty) is an explicit OFF upstream and a silent no-op here. Uniquely owned here. |
| DRIFT-051 | **new** (repair pass) | `process.title`'s role suffix never set, so RPC-mode, subagent-runner and intercom-broker children are indistinguishable from an interactive session in `ps`. Filed from the `packages/coding-agent/src/bun/` sweep this area had never run. |

Closed: **13** (5 this pass). Partially closed: **3** (DRIFT-004, DRIFT-015, DRIFT-019 — remainders open below). Withdrawn: 1 (DRIFT-037). Superseded: 1 (DRIFT-038 → `CFG-018`).

## Open items

One table, deliberately — a second table is what cost `SEAM-S01` an audit pass (README blind spot,
mechanical fix note). **Rows whose Severity cell reads `tracker` are excluded from the severity
counts**: they propose no work and say so, so they are bookkeeping rather than backlog. The
**Dedup** column names the item in another area that owns the same defect; see
`## Coverage → Duplication census`. **No ID was renumbered, merged or deleted to produce it.**

> **RE-DERIVED 2026-08-14 (sweeps 7-8 reconciliation, third edition) — counted set UNCHANGED at 0 critical, 0 high, 7 medium, 9 low = 16** (34 rows: 14 closed, 16 open, plus the four `tracker` rows). **No sweep since 3 has owned this area — it is now the LEAST-worked file in the directory by closure rate (14 of 34 = 41%, against a directory average of 76%), and that is a scheduling fact, not a difficulty fact.** One routing note added: **`DRIFT-004` needs the same `BashOperations` trait in `crates/cyrup-tools` that `SEAM-015` needs, so the two should go to ONE agent** (re-confirmed by sweep 8; see `08-cyrup-session-svc-and-modes.md`).

> **RECOUNTED 2026-08-14 — counted set: 0 critical, 0 high, 7 medium, 9 low = 16**, plus the four `tracker` rows. 14 rows are now marked CLOSED, including the area's only high.

| ID | Severity | Kind | Effort | Dedup | Title |
|---|---|---|---|---|---|
| ~~DRIFT-049~~ | ~~**high**~~ **CLOSED 2026-08-14** | **FIXED 2026-08-13** *(via `SEAM-047`)* | M | duplicate-of: `SEAM-047` | SIGTERM/SIGHUP never disposes the runtime, and in RPC mode is never observed at all — **CLOSED 2026-08-14**: closed pre-sweep (2026-08-13) via SEAM-047 — the row's kind cell already said so while its severity cell still read `high`, which is why two recounts carried a phantom high. Area 12 now has NO open high. |
| DRIFT-041 | medium | not-ported | L | — | Session HTML export is a 131-line text dump against pi's 5,021-line templated document — **2026-08-14, still open**: sweep 2 — not started. Effort L: pi's `core/export-html/` is 5,021 lines across eight files including vendored `marked.min.js` and `highlight.min.js`, and the port needs a base64 `SessionData` payload, four template assets, a theme-colour feed and an ANSI-to-HTML converter. |
| ~~DRIFT-048~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | — | Google converter picks the tool-call-id rule off the SOURCE message's model, not the target model — **CLOSED 2026-08-14**: sweep 2 — fixed exactly as the Fix specified: `include_id`, already computed from the TARGET model at `convert_messages:612` and already used for `functionResponse`, is now threaded into `assistant_parts`, which had been gating `functionCall.id` on `requires_tool_call_id(am.model)` — the model that PRODUCED the historical turn. On a mid-session `gemini-2.5-pro` → `gemini-3-pro` switch the two halves of a call/response pair therefore disagreed, silently. Verify implemented verbatim including the reverse-switch and both same-model control cases. **The item's observation that this is "a name-present, argument-wrong bug, invisible to an absence sweep" is worth promoting into the Coverage section as a recurring class — it is the second such finding in this file after DRIFT-026, in the same function.** |
| DRIFT-004 | medium | upstream-drift | M | — | RPC `bash`: `UserBashEventResult.operations` backend seam unported — **2026-08-14, still open**: sweep 1 + 2 — kept open with the mechanism correction recorded under SEAM-015: the seam is a WIT capability, not a `BashOptions` field. Sweep 2 re-confirmed rather than overrode that reasoning — the trait would live in `crates/cyrup-tools/src/ops/`, but its two consumers (`cyrup-session-svc/src/bash.rs`'s per-call override and `cyrup-modes/src/rpc.rs`'s population from the user-bash event result) are in other crates, so landing the trait alone produces a seam with no caller. |
| DRIFT-009 | medium | upstream-drift | M | duplicate-of: `PROV-018` | Embedded catalog floor is 4 catalogs short *(regeneration-source half struck — it was false)* |
| ~~DRIFT-013~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | — | Z.AI sent `max_completion_tokens`, which it ignores — **CLOSED 2026-08-14**: sweep 2 — the missing `\|\| is_zai` term added to `use_max_tokens` in `api/compat.rs`, so Z.AI models resolve `max_tokens_field == MaxTokens` instead of the `max_completion_tokens` Z.AI ignores. **KIND CORRECTED: filed `upstream-drift` citing openai-completions.ts:1478-1485 @v0.84.1, but `git show v0.83.0:packages/ai/src/api/openai-completions.ts` has the identical disjunction at :1427-1435 with `isZai` at :1435 — INSIDE the ported baseline. It is `not-ported`, and the item's implicit "a rebase will sweep this up" framing was wrong.** Same class as the DRIFT-014/018/019/030/031/032 kind corrections the repair pass already made; this one was missed. |
| DRIFT-014 | medium | **not-ported** | M | — | DNS/transport failures not classified retryable (seven literals, all predating the baseline) — **2026-08-14, still open**: sweep 2 — **keep open, and STRENGTHEN the warning rather than the item: this is the one backlog row whose Verify cannot be satisfied under the standing no-test-execution rule**, because it requires observing what string reqwest/hyper actually emits on a failed DNS lookup. Adding pi's seven Node-shaped literals (`getaddrinfo`, `ENOTFOUND`) alone would produce a test that passes while the real Rust failure mode stays unretried — exactly the false close the item's own Verify warns against. |
| DRIFT-015 | medium | upstream-drift | L | duplicate-of: `EXT-019` | Extension context surface: `scopedModels`, `outputPad`, markdown transformers still absent |
| DRIFT-028 | medium | upstream-drift | S | — | OpenRouter Anthropic cache breakpoint skips tool results — **2026-08-14, still open**: sweep 2 — half (a) (OpenRouter Anthropic cache breakpoint skipping tool results, in `openai_completions.rs`) is inside cyrup-provider and untouched; half (b) was already refuted upstream-side by the tracker. |
| ~~DRIFT-029~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | M | — | Concurrent user bash: single cancel slot makes abort miss and `is_bash_running` lie — **CLOSED 2026-08-14**: sweep 2 — pi's `_bashAbortControllers` SET ported. `AgentSession.bash_cancel: Mutex<Option<CancelToken>>` became `bash_cancels: Mutex<Vec<(u64, CancelToken)>>` plus a `next_bash_cancel_id: AtomicU64` standing in for the `AbortController` OBJECT IDENTITY pi's Set keys on — **not `options.id`, which is optional and may repeat, and which the item's proposed `HashMap<BashId, CancelToken>` would have keyed on**. `execute_bash` pushes one entry per call and installs a `BashCancelGuard` whose `Drop` is pi's `finally`; `abort_bash` cancels a snapshot of the whole set (pi's spread copy); `is_bash_running` is `!is_empty()`; the unconditional clears at the shell-resolution error path and the completion path are deleted. **KIND CORRECTED: filed `upstream-drift` against pi commit `2efa728d`, but `git grep -n _bashAbortController v0.83.0` shows the Set, its add, its delete-in-finally, the spread-copy abort and the `size > 0` getter all present at the ported tag — this was a port omission, not version lag.** |
| DRIFT-030 | medium | **not-ported** *(was upstream-drift)* | S | duplicate-of: `PROV-021` | `ANTHROPIC_AUTH_TOKEN` bearer-token auth unsupported |
| ~~DRIFT-033~~ | ~~medium~~ **CLOSED 2026-08-14** | port-divergence | M | — | A mid-run tool addition never reaches the system prompt — **CLOSED 2026-08-14**: sweep 2 — its refuter caveat ("this is not a one-line assignment; the prerequisite is modelling pi's two prompt slots") was CORRECT, and both parts landed together in cyrup-session-svc: `AgentSession.system_prompt_override: Mutex<Option<String>>` beside `base_system_prompt`, with `system_prompt_override()` / `effective_system_prompt()` (pi's `override ?? base`); `assemble_run_messages` writes the slot on all three branches; `drive_run` clears it at the head of its settle path (pi's `_runAgentPrompt` finally, agent-session.ts:1069); `push_active_tools` sets the agent prompt from `effective_system_prompt()` so a mid-run rebuild can no longer clobber a `before_agent_start` sanitization; and `PolicyHooks::prepare_next_turn` assigns `update.system_prompt` beside `update.tools`. **STRIKE the interim instruction to "promote the hooks.rs:158-161 comment into a documented [CYRUP-DELTA]" — the divergence it described no longer exists.** The one remaining delta is narrower and documented in-source: cyrup's `BeforeAgentStart` carries the prompt as a mutated-in-place `String`, so equality with the base stands in for pi's `!== undefined`. |
| ~~DRIFT-010~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | — | `get_available_thinking_levels` RPC dispatch arm missing (the method exists) — **CLOSED 2026-08-14**: sweep 1 — duplicate of SEAM-014, fixed in the same sweep. |
| ~~DRIFT-016~~ | ~~low~~ **CLOSED 2026-08-14** | **stale-port** *(was upstream-drift)* | S | duplicate-of: `SESS-019` | `Current date:` still injected into the system prompt — **CLOSED 2026-08-14**: sweep 1 — duplicate of SESS-019(a), now fixed. Its own re-derivation (removal predates the ported baseline ⇒ `stale-port`) was independently confirmed by re-running `git grep 'Current date' v0.83.0 -- packages/coding-agent/src`, which returns nothing. |
| DRIFT-018 | low | **not-ported** *(was upstream-drift)* | L | duplicate-of: `PROV-011` | Constrained sampling (strict JSON schema + Lark/regex grammars) absent |
| DRIFT-019 | low | **not-ported** *(was upstream-drift)* | M | duplicate-of: `PROV-014` | Radius / Qwen Token Plan providers unregistered (the wire API and OAuth landed) |
| DRIFT-020 | low | upstream-drift | S | duplicate-of: `PROV-024` | openai-responses affinity keys on removed `sendSessionIdHeader` |
| ~~DRIFT-024~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | duplicate-of: `SESS-013` | AGENTS.md loaded twice in nested git worktrees — **CLOSED 2026-08-14**: sweep 1 — duplicate of SESS-013, now fixed; both load-bearing guards (`:108`, `:113`) and the `isShadowed` gate (`:140-142`) are ported. |
| DRIFT-025 | low | upstream-drift | S | duplicate-of: `CFG-017` | `${@:-default}` prompt-template defaults render literally |
| DRIFT-027 | low | upstream-drift | S | duplicate-of: `PROV-025` | openai-completions has no `deferredToolsMode: "kimi"` |
| DRIFT-031 | low | **not-ported** *(was upstream-drift)* | M | duplicate-of: `PROV-036` | No usage cost breakdown — per-model attribution and `Tools/summaries` unsurfaced |
| ~~DRIFT-035~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | duplicate-of: `SESS-019` | Prompt tests assert the `Current date:` footer, pinning DRIFT-016 — **CLOSED 2026-08-14**: sweep 1 — both assertions at prompt/tests.rs are replaced, landed in the same change as the footer removal as the item requires. |
| ~~DRIFT-036~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | — | `settle()` uses a fixed 50 ms sleep as the only synchronization — **CLOSED 2026-08-14**: sweep 2 — `settle()` in `cyrup-session-svc/src/tests/summarization_retry_events.rs` no longer sleeps 50 ms. It takes the collector buffer and polls until the length is unchanged across 64 consecutive `yield_now`s — an observation of the state the assertions read — with an outer bound that turns a stuck pipeline into a NAMED PANIC rather than a hang. All ten call sites updated. The correct poll-until-observed pattern already existed in the same file at :420-425. |
| ~~DRIFT-039~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | duplicate-of: `AGENT-019` | `a_02_2_parallel_completion_vs_source_order` asserts a wall-clock bound and a completion ORDER — **CLOSED 2026-08-14**: sweep 1 — same test as AGENT-019 and closed by the same rewrite: the 115 ms bound is gone, a `Barrier(2)` plus a subscriber-driven oneshot make the completion order a fact, and the surviving 10 s `timeout` is documented in-source as a hang detector. **Sweep 2 additionally established that DRIFT-039 is the ONLY open DRIFT row whose fix lands in crates/cyrup-agent** — every row of this file's open table was read to confirm it, so no DRIFT work is owed by area 02. |
| DRIFT-042 | low | not-ported | S | — | `/login` launches no browser and its "Cmd+click to open" hint is not a link |
| DRIFT-045 | low | not-ported | S | — | Ctrl+V with text on the clipboard inserts nothing |
| ~~DRIFT-046~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | duplicate-of: `TOOL-036` | `normalizeWindowsShellPath` unported — Git-Bash/MSYS/Cygwin/WSL drive paths unconverted — **CLOSED 2026-08-14**: sweep 1 + 2 — **REOPENED AND RE-CLOSED, not a plain duplicate.** Sweep 1 closed it on TOOL-036's landing in `cyrup-tools/src/path.rs`, but a SECOND live instance existed in `crates/cyrup-config/src/paths.rs` — which is the 1:1 port of the very function upstream applies the rule inside (`utils/paths.ts` `normalizePath`) and was created AFTER this item was written, by CFG-025/CFG-036 — so cyrup ended sweep 1 with the rule in one copy of the normalizer and not the other. Now ported and applied at paths.ts:83-85's exact position (before the tilde expansion, inside the shared normalizer), with `test/paths.test.ts:133-150` ported verbatim INCLUDING the pass-through list, plus six extra grammar cases pinning where pi's regex backtracks and a hand parse does not. **The item's own Fix sentence — "Port the function into cyrup-resources (wherever `normalize_path` lives)" — names the wrong crate: `normalize_path` lives in cyrup-config, and cyrup-resources depends on it.** |
| DRIFT-047 | low | upstream-drift | L | duplicate-of: `VL-P5` | `packages/telemetry` and the `pi.ai.request` span contract absent |
| ~~DRIFT-050~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | — | `CYRUP_TELEMETRY=` empty is an explicit OFF upstream and a silent no-op here — **CLOSED 2026-08-14**: sweep 2 — `CYRUP_TELEMETRY=` / `PI_TELEMETRY=` (set but empty) is now an explicit OFF that beats the settings opt-in, restoring pi's tri-state (unset / set-empty / set-truthy): the telemetry field no longer goes through the `!v.is_empty()` filter and takes the first key that is SET AT ALL, while the two sibling flags (`offline`, `skip_version_chk`) keep the per-key empty filter, which is indistinguishable from pi for them. **MECHANISM NOTE the item could not anticipate: this could not be tested by mutating the process environment, because `std::env::set_var` is `unsafe` under Rust 2024 and `cyrup-config` is `#![forbid(unsafe_code)]`. `EnvVars::from_lookup(get)` was added as a pure seam and `from_process` reduced to `from_lookup(\|k\| std::env::var(k).ok())`; `first_env` no longer exists. Any area file citing `cyrup-config/src/env.rs:50-53 first_env` is now stale, and any future env-tier parity item in that crate needs the same seam.** |
| ~~DRIFT-051~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | — | `process.title`'s role suffix never set — RPC / runner / broker children are all bare `cyrup` in `ps` — **CLOSED 2026-08-14**: sweep 1 — duplicate of SEAM-070, fixed in the same sweep. **Sweep 2 carries SEAM-070's own caveat across: on macOS `pthread_setname_np` does not change what `ps -o comm=` prints, so this item's Verify line holds verbatim only on Linux.** |
| DRIFT-022 | **tracker** | **flag half FIXED 2026-08-13** *(via `SEAM-051`)*; renderer half still tracking | L | duplicate-of: `SEAM-051` | TUI mode (`--tui-mode`, alternate screen) not ported |
| DRIFT-023 | **tracker** · *lead* | tracking | L | duplicate-of: `CFG-020` | Model registry → `ModelRuntime` refactor not absorbed — **evidence unverified** |
| DRIFT-032 | **tracker** | **not-ported** *(was upstream-drift)* | L | duplicate-of: `EXT-027` | llama.cpp router integration and Hugging Face model search entirely unported |
| DRIFT-040 | **tracker** · *lead* | tracking | L | duplicate-of: `VL-P22` | pi's agent-harness v2 rearchitecture entirely unabsorbed — **evidence unverified** |

**Count:** 34 IDs = **30 severity-bearing (0 critical, 1 high, 11 medium, 18 low)** + 4 trackers.
**Deduplicated: 14 IDs this area uniquely owns — 0 high, 8 medium, 6 low.** The single high is a
duplicate and is scheduled in area 08, so **this area contributes no high to a deduplicated plan.**

## Leads — not yet evidenced (outside the item count)

Two tracker items rest on inherited citations that no pass has re-derived from both sides. They are
recorded here rather than in the backlog because the hard rule says a commit is a hypothesis, and an
item at a lower evidence standard than its neighbours corrupts the count it sits inside. They keep
their IDs and their bodies below.

| ID | What is unverified | Cheapest way to settle it |
|---|---|---|
| `DRIFT-023` | Neither side re-read since the `1806375` filing. The cyrup-side "doc citations only" claim and the `model-runtime.ts` `+274/−82` diffstat are both inherited. | `git -C pi diff --stat v0.83.0..v0.84.1 -- packages/coding-agent/src/core/model-runtime.ts` plus `rg -n 'ModelRuntime' crates/` — two commands, then fold the result into `CFG-020`. |
| `DRIFT-040` | The `agent-harness.ts` rewrite (`+420/−996`), `docs/harness-v2.md` (`+2124/−367`) and the sqlite-node rebuild (`+12598/−3479`) were never read. The package-level rename (`packages/storage` → `packages/session-backends`) and `harness/session/types.ts:44-58` **were** confirmed first-hand and are not in doubt. | Read `packages/agent/src/agent-harness.ts` at both tags and diff `harness/session/types.ts` against `cyrup-session/src/entry.rs`. Sizing only — the item stays a tracker either way until pi's harness log goes quiet. |

---

## DRIFT-041 — Session HTML export is a 131-line text dump against pi's 5,021-line templated document

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/export.rs` — the **whole** renderer is 131 lines (`escape`, `collect_text`, `entry_role`, `session_jsonl_to_html`, `EXPORT_CSS`). `session_jsonl_to_html` (`:72-121`) splits `export_jsonl`, takes `name`/`title` and `cwd` off the header, and for each remaining line emits `<section class="entry {role}"><header>{role}</header>` plus one `<pre>` per string harvested by `collect_text` (`:35-56`), which recurses looking **only** for values under a `"text"` key. Styling is a hardcoded 8-line `EXPORT_CSS` const (`:124-131`) with a fixed `#1e1e2e` dark palette, not the user's theme. Four consumers: `cyrup-tui/src/app.rs:4160` (`/export`) and `:4273` (`/share`), `cyrup-modes/src/rpc.rs:1257` (RPC `export_html`), `cyrup/src/main.rs:1321` (`--export`). The residual is acknowledged in-tree at `export.rs:12-14` but names only `tool-renderer.ts`, understating it by an order of magnitude.

**upstream** — pi `v0.84.1` `packages/coding-agent/src/core/export-html/` totals **5,021 lines across eight files**: `template.js` 1864, `vendor/highlight.min.js` 1212, `template.css` 1066, `index.ts` 316, `ansi-to-html.ts` 258, `tool-renderer.ts` 172, `vendor/marked.min.js` 78, `template.html` 55. `index.ts:263-270` builds `SessionData{header, entries, leafId, systemPrompt, tools, renderedTools}`; `:151-158` resolves the **active theme** (`getResolvedThemeColors`/`getThemeExportColors`) and injects `{{THEME_VARS}}`/`{{BODY_BG}}`/`{{CONTAINER_BG}}`/`{{INFO_BG}}`; `:160` base64-encodes the session data; `:169-174` substitutes `{{CSS}}`/`{{JS}}`/`{{SESSION_DATA}}`/`{{MARKED_JS}}`/`{{HIGHLIGHT_JS}}`. `template.html:12-40` ships a sidebar with a search box (`:18`), five filter buttons (`:21-25`), a resizable tree pane (`:29-32`) and an image modal (`:37-39`). `template.js:73-95` `buildTree()` walks `parentId`; `:122-142` walks to root; `:708-754` drives leaf/target navigation; `:857`/`:866` call `hljs.highlight`; `:1557` configures `marked`; `:1094-1107` encodes a deep link. `index.ts:178-230` `preRenderCustomTools` renders **extension** tool calls/results through their TUI renderers — `ansi-to-html.ts` is what converts that ANSI into HTML, which is why it is load-bearing rather than incidental. Directory **unchanged** `v0.83.0..v0.84.1`, so this is owed debt, not lag.

**Impact** — `/export`, `/share`, `cyrup --export` and RPC `export_html` all produce a document that looks superficially plausible and is materially wrong. Markdown renders as literal source; code blocks are unhighlighted; **tool-call arguments vanish** (bash's `command`, edit's diff — they sit under `arguments`, not `text`); tool **results** keep their text but lose their tool name, call id, `isError` and `details`, so a result appears as an unattributed `<pre>`; abandoned branches are interleaved with the active path because `parentId`/`leafId` are ignored; images are dropped; the export ignores the user's theme. Exported transcripts are the artifact users hand to other people, so the loss is silent and travels — and `/share` publishes it. *(Refuter caveat, stated so nobody re-scopes off the wrong claim: the original filing's "every tool call and tool result vanishes" is overstated — result **text** does survive.)*

**Fix** — Rewrite `export.rs` around pi's contract rather than around `collect_text`. (1) Serialize a `SessionData` payload — header, full entries, `leaf_id`, system prompt, tool list — and embed it base64 in a `<script id="session-data" type="application/json">`, mirroring `index.ts:160`/`template.html:42`, so the document carries the **tree** instead of a flattened text list. (2) Port `template.html`/`template.css`/`template.js` as `include_str!` assets under `crates/cyrup-session-svc/src/export/`, substituting the same five placeholders. (3) Feed real theme colours from `cyrup-resources::theme`, which already models pi's `export` block (`crates/cyrup-resources/src/theme.rs:140, :598, :685`), instead of `EXPORT_CSS`. (4) Vendor a markdown + highlight pair, or reuse `cyrup-tui`'s markdown renderer to emit HTML host-side — `template.js`'s marked/hljs use is the only part with no Rust analogue. (5) Add the `ToolHtmlRenderer` + ANSI-to-HTML seam last; it is the smallest slice, not the main one.

**Verify** — Golden-file test in `cyrup-session-svc`: export a fixture session containing (a) an assistant message with a fenced code block, (b) a `bash` tool call **plus** its result, (c) an extension tool result, (d) two branches off one parent. Assert the HTML contains the tool name **and the command string**, contains a highlighted `<code class="hljs">`, contains the tree/sidebar scaffold, and that a rebuild from the embedded `session-data` yields exactly the entries in the JSONL. Assert the palette matches the active theme's export colours, not a constant.

## DRIFT-048 — Google converter picks the tool-call-id rule off the SOURCE message's model, not the target model

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-provider/src/api/google_generative_ai.rs:797`, inside `assistant_parts`: the toolCall arm gates on `if requires_tool_call_id(am.model.as_str())` — the model that **produced** the assistant message. cyrup already computes the correct value one function up: `convert_messages` sets `let include_id = requires_tool_call_id(&model_id);` at `:612` from the **target** model and uses it for `functionResponse`s — but `assistant_parts(am, same)` (called at `:627` and `:737`) is never handed it, so the two halves of a call/response pair can disagree.

**upstream** — `pi/packages/ai/src/api/google-shared.ts:177` gates the same field on `requiresToolCallId(model.id)` — the target model of the current request, for both halves. The predicate itself was **widened in this delta** to return true for every Gemini major version ≥ 3 (`google-shared.ts:73-79`); cyrup ported the predicate correctly at `:529-533`, which is what makes the wrong argument bite now.

**Impact** — On a session that switches model mid-flight — `gemini-2.5-pro` → `gemini-3-pro`, exactly the case this delta made common — older assistant turns serialize `functionCall` **without** `id` while their matching `functionResponse` carries one (from `include_id`), so Gemini 3 sees unpaired call/response pairs. The reverse switch injects an `id` into a `gemini-2.5` request that upstream would never send. Silent: the request is well-formed, the pairing is not. Note this sits in the same function whose *other* defect (DRIFT-026) was just closed, so the file has been read recently and this was still missed — a name-present, argument-wrong bug is invisible to an absence sweep.

**Fix** — Thread the already-computed `include_id` from `convert_messages:612` into `assistant_parts` as a parameter and use it at `:797` in place of `requires_tool_call_id(am.model.as_str())`, matching `google-shared.ts:177`. Both call sites (`:627`, `:737`) are in the same function that computes it.

**Verify** — Unit test in `google_generative_ai.rs`: build a history whose assistant tool-call turn carries `model = "gemini-2.5-pro"`, convert it for target `gemini-3-pro`, and assert the emitted `functionCall` carries an `id` **and** that it matches the `functionResponse`'s. Repeat with the models swapped and assert **neither** carries an `id`. A same-model case must be unchanged, so the existing fixtures stay green.

## DRIFT-049 — SIGTERM/SIGHUP never disposes the runtime, and in RPC mode is never observed at all

**Kind** not-ported · **Severity** **high** *(raised from medium, repair pass)* · **Effort** M · **Confidence** high · **duplicate-of: `SEAM-047`**

> **FIXED 2026-08-13 via `SEAM-047`** (`crates/cyrup/src/signals.rs`, rewritten as a per-host port;
> three call sites in `main.rs`). Closed here for bookkeeping only — the work, its pi citations and
> its two stated `CYRUP-DELTA`s live in area 08's `SEAM-047` entry, per the "schedule it once" note
> below. The first SIGTERM/SIGHUP in print/json/rpc now aborts the session, fires `cancel`, awaits
> `runtime.dispose()` and exits 143/129 (pi `print-mode.ts:48-64`, `rpc-mode.ts:365-379`,`:723-740`
> @v0.83.0); the RPC half is pinned by `crates/cyrup/tests/signal_shutdown.rs`, which drives the real
> binary and was measured RED before the change ("still alive 15s after the FIRST -TERM") and GREEN
> after. `SEAM-059` (the watcher held a stale startup `Arc<AgentSession>`) landed in the same rewrite.
> **`SEAM-008` is NOT closed by this** and was re-scoped rather than inherited — see area 08.
>
> **Severity and ownership corrected 2026-08-12 (repair pass).** This is the same defect area 08
> files as **`SEAM-047`** (`08-cyrup-session-svc-and-modes.md:105`), which rates it **high**, and
> `PARITY-GAPS` **PB-30** names both IDs at high. A defect cannot carry two severities, and a high
> filed as a medium drops off a planner's high list — so it is raised here to match. **Schedule it
> once, in area 08**, together with the sibling signal-path items `SEAM-008` (the 143/129 codes are
> computed but used only on the second delivery) and `SEAM-059` (the watcher holds a disposed
> session Arc). The body below is retained because it carries the RPC-mode analysis and the
> `SEAM-S02` bookkeeping note that area 08's item does not.

**cyrup** — `cyrup/crates/cyrup/src/signals.rs:88-101` `spawn_abort_on_signal` does exactly `session.abort(); cancel.cancel();` on the first signal, then blocks awaiting a **second** signal before `std::process::exit(repeat.exit_code())`. `session.abort()` is `abort_retry() + agent.abort()` (`cyrup-session-svc/src/session.rs:1348-1351`) and touches neither `session_cancel` nor the runtime; the `cancel` it fires is `main.rs:367`'s `CancelToken::new()`, a TUI input/watcher token, **not** `session_cancel`. In interactive/print/json that is survivable because the mode loop returns and `main.rs:575` / `run.rs:39` / `run.rs:60` then call `runtime.dispose()`. **RPC mode has no such route**: `run_rpc_dispatch` (`cyrup-modes` `run.rs:101-116`) only disposes after `run_rpc` returns, and `run_rpc` is parked on a stdin read that a signal does not disturb — nothing else observes `cancel`.

**upstream** — pi `v0.84.1` does all of it on the **first** delivery. `packages/coding-agent/src/modes/rpc/rpc-mode.ts:366-383` registers SIGTERM (plus SIGHUP off-win32) and the handler calls `killTrackedDetachedChildren(); void shutdown(signal === "SIGHUP" ? 129 : 143, signal);`; `shutdown` at `:724-741` runs `runtimeHost.dispose()` then `process.exit(exitCode)`. `packages/coding-agent/src/modes/print-mode.ts:50-66` is the same shape.

**Impact** — A supervisor's SIGTERM to an RPC-mode cyrup leaves it running: no `session_shutdown` event reaches extensions (so nothing flushes), `session_cancel` is never cancelled (so in-flight bash trees survive — see the `SEAM-S03` overlap), and the process never exits, so the supervisor escalates to SIGKILL after its grace period and everything unflushed is lost. In the other modes the runtime *is* disposed, but only as a side effect of the mode loop returning, and only after a second signal for the exit code — pi exits `143`/`129` on the first.

**Fix** — Move disposal into the signal path rather than relying on the mode loop. In `signals.rs`, on the **first** signal: cancel `session_cancel` (not just the TUI token), await `runtime.dispose()` with a bounded grace, then exit `143`/`129` per signal, mirroring `rpc-mode.ts:366-383` + `:724-741`. Register the handler for RPC mode explicitly — today it is the one mode with no path from a signal to `run_rpc`. Keep the existing repeat-signal force-exit at `:97-100` as the escalation.

**Verify** — Integration test per mode: send SIGTERM to a running cyrup and assert (a) the process exits `143` without a second signal, (b) a registered extension observed `session_shutdown`, (c) the session file is flushed. An RPC-mode case is mandatory — it is the one that fails today. **Bookkeeping:** `SEAM-S02` (`08-cyrup-session-svc-and-modes.md:520`) is now **stale** — it says the second signal is swallowed and cyrup is "ABSENT", but `signals.rs:97-100` implements the repeat force-exit with pi's exact `130`/`143`/`129` codes. Re-audit `SEAM-S02` as closed and let this item replace it.

## DRIFT-004 — RPC `bash`: `UserBashEventResult.operations` backend seam unported

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — The `user_bash` half is closed. The backend seam is not: there is **no Rust trait or type named `BashOperations`** — all ten hits are doc comments citing pi (`cyrup-tools/src/ops/local.rs:6, :8, :487, :811`; `cyrup-session-svc/src/bash.rs:73`; `cyrup-session-svc/src/session.rs:4496`; `cyrup-tools/src/tools/bash.rs:220`; `cyrup-tools/src/ops/shell.rs:88`; `cyrup-tools/tests/round3.rs:196, :300`). `BashOptions` carries no per-call backend override.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:576` passes `operations: eventResult?.operations`; the option is `BashToolOptions.operations?: BashOperations` at `pi/packages/coding-agent/src/core/tools/bash.ts:186-187`. Unchanged at `v0.84.1`.

**Impact** — An extension can block or replace a bash **result** but cannot supply the backend the command executes **through**, so sandboxed, remote or recorded bash backends supplied per call are impossible.

**Fix** — Introduce a `BashOperations` trait in `cyrup-tools/src/ops/` matching pi's `createLocalBashOperations` surface (the doc comments already name it), make the local implementation the default, add an optional per-call override to `BashOptions` in `cyrup-session-svc/src/bash.rs`, and populate it from the user-bash event result in `cyrup-modes/src/rpc.rs`.

**Verify** — Test that an extension returning a custom `operations` from the user-bash event has its backend used for that call and not for the next; and that with no override the local backend still runs.

## DRIFT-009 — Embedded catalog floor is four catalogs short

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high ·
**duplicate-of: `PROV-018`** (tooling half) · see also `PROV-004`, `PROV-039`, `PARITY-GAPS` VL-P25

> **REWRITTEN 2026-08-12 (repair pass). The prior edition of this item was factually wrong about
> upstream and its Fix was worse than the one already on the backlog.** It stated "There is still
> **no in-tree regeneration source**" and proposed seeding the embedded floor from the pi.dev
> artifact as "the only source still available". Both halves are false — see **upstream** below.
> The regeneration source is committed, at both tags, and `PROV-018` already proposes driving it.
> **Do not seed from the pi.dev artifact.** That path is strictly lossier: the artifact is the
> *published* catalog, so it cannot reproduce whatever the generator computes from
> `models-dev-reasoning-options.ts` (the effort→thinking-level map) or from the compat overrides
> `generate-models.ts` applies per provider, and it gives no reproducible build step — the exact
> property `PROV-018`'s ignored drift test needs.

**cyrup** — `cyrup/crates/cyrup-provider/tests/catalog_data.rs:5-8` still declares the provenance as "31 embedded catalogs … byte-faithful snapshot of pi @ `5c1a2977`" and still names `packages/ai/src/providers/*.models.ts` as the source. The tree now ships **35** catalogs (`ls crates/cyrup-provider/src/providers/catalog/ | wc -l` → 35), so the doc's own count is stale by four. `cyrup-provider/src/catalog.rs:1-12` confirms the embedded set is still "the source of truth and the floor". *(The provenance-comment half of this is `PROV-039`; this item is scoped to the four missing catalogs.)*

**upstream** — pi `v0.84.1` ships **39** `*.models.ts` (`git ls-tree v0.84.1 --name-only packages/ai/src/providers/ | grep -c models.ts` → 39), each a two-line re-export of `packages/ai/src/providers/data/*.json`. **Only the output is gitignored** (`pi/.gitignore:11` — `packages/ai/src/providers/data/`); the generator is committed and present at **both** tags, verified with `git cat-file -e <tag>:<path>`:

| path | v0.83.0 | v0.84.1 | role |
|---|---|---|---|
| `packages/ai/scripts/generate-models.ts` | ✔ (2 733 lines) | ✔ | the generator itself |
| `packages/ai/scripts/model-data.ts` | ✔ | ✔ | `createModelDataManifest`, `validateGeneratedModelData`, `validateModelDataDirectory` |
| `packages/ai/scripts/models-dev-reasoning-options.ts` | ✔ | ✔ | `getEffortThinkingLevelMap` — the effort→thinking-level ladder |
| `packages/ai/scripts/check-model-data.ts` | ✔ | ✔ | the committed data-integrity check |
| `scripts/diff-model-catalog.mjs` | ✔ | ✔ | a purpose-built **catalog drift differ** |
| `scripts/publish-model-catalog.mjs` | ✔ | ✔ | documents the published-artifact schema |

Driven from pi's root `package.json:24-30` — `generate:models`, `hydrate:model-data`, `generate:model-catalog`, `diff:model-catalog`, `check:model-catalog` — and from `packages/ai/package.json:52-54` (`generate-models` = `node scripts/generate-models.ts --strict`; `hydrate-model-data` = `--strict --data-only`; `generate-model-catalog` = `--strict --json-only --json-output ../../.artifacts/model-catalog`).

Missing four catalogs: `baseten`, `qwen-token-plan`, `qwen-token-plan-cn`, `qwen-token-plan-individual` — matching `PARITY-GAPS` VL-P25 and overlapping DRIFT-019's provider half. Three of the four (`qwen-token-plan`, `qwen-token-plan-cn`, and `baseten`'s provider entry) are **not v0.84.1 drift**: `qwen-token-plan.models.ts` and `qwen-token-plan-cn.models.ts` both exist at `v0.83.0`.

**Impact** — Continuing catalog drift. Wrong prices and context windows on `--offline` runs, on first launch before the DRIFT-007 overlay fetches, and whenever pi.dev is unreachable. Silent — a wrong price is never surfaced as an error. Four providers have no floor at all.

**Fix** — **Defer the mechanism to `PROV-018`**, whose Fix is already correct: `cyrup/xtask gen-catalogs` runs pi's `npm run generate-models` at a named tag, consumes the produced `packages/ai/src/providers/data/*.json` plus the image models, emits every `providers/catalog/*.json`, and rewrites `catalog_manifest.json`; plus an `#[test] #[ignore]` drift check. This item contributes exactly two things to that work: (1) the four missing catalogs must be in the generated set, landing in the same change as DRIFT-019 / `PROV-014`'s registrations; (2) `scripts/diff-model-catalog.mjs` is the model for the drift check — port its comparison, do not invent one. The provenance-comment rewrite at `catalog_data.rs:5-8` belongs to `PROV-039`.

**Verify** — Per `PROV-018`: `cargo xtask gen-catalogs` against a named pi tag reproduces the current tree byte-for-byte, and the ignored drift test fails when pointed at a newer pi. This item adds: after the run, `ls crates/cyrup-provider/src/providers/catalog/ | wc -l` is 39 and each of the four new files is non-empty and registered in `providers/all.rs`.

## DRIFT-013 — Z.AI sent `max_completion_tokens`, which it ignores

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-provider/src/api/compat.rs:319-324`: `use_max_tokens = base_url.contains("chutes.ai") || is_moonshot || is_cloudflare_ai_gateway || is_together || is_nvidia || is_ant_ling`. No `is_zai` — even though `is_zai` is defined at `:285` and already used at `:311`, `:338` and `:355`. Consumed at `:363-367` (`max_tokens_field`).

**upstream** — `pi/packages/ai/src/api/openai-completions.ts:1478-1485` at `v0.84.1`: `const useMaxTokens = baseUrl.includes("chutes.ai") || isMoonshot || isCloudflareAiGateway || isTogether || isNvidia || isAntLing || isZai;` — the trailing `|| isZai` is the missing term. pi `2fe21b40`.

**Impact** — Requests to Z.AI / `api.z.ai` / `open.bigmodel.cn` carry a token cap the provider ignores, so output length is effectively unbounded — overlong completions and unexpected cost.

**Fix** — Add `|| is_zai` to the `use_max_tokens` disjunction at `compat.rs:319-324`.

**Verify** — Unit test: a `zai` / `zai-coding-cn` model resolves `max_tokens_field == "max_tokens"`; an `openai` model still resolves `max_completion_tokens`. The existing assertions in `openai_completions.rs` are on an `openai` model and do not pin the bug.

## DRIFT-014 — DNS/transport failures not classified retryable

**Kind** **not-ported** · **Severity** medium · **Effort** M · **Confidence** medium

*(Kind corrected this pass. The prior filing called this `upstream-drift`; `git diff v0.83.0..v0.84.1 -- packages/ai/src/utils/retry.ts` is **empty**, so every missing literal already existed at the ported baseline. This is a port omission and will **not** be swept up by a routine rebase.)*

**cyrup** — `cyrup/crates/cyrup-provider/src/utils/retry.rs:34-75` `RETRYABLE_PROVIDER_PATTERNS`. Re-derived line by line against upstream at `v0.84.1`: exactly seven literals are absent — `"524"` (ts:36), `"getaddrinfo"` (ts:54), `"ENOTFOUND"` (ts:55), `"EAI_AGAIN"` (ts:56), `"socket connection was closed"` (ts:60), `"stream ended before a terminal response event"` (ts:74), `"ResourceExhausted"` (ts:88).

**upstream** — `pi/packages/ai/src/utils/retry.ts:26-89` `RETRYABLE_PROVIDER_ERROR_PATTERN`.

**Impact** — A transient DNS failure or an upstream 524 aborts the turn instead of retrying, surfacing as a hard error mid-run.

**Fix** — **Touch only `retry.rs:34-75`.** Do **not** follow the original filing's instruction to add these to `cyrup-ext-subagents/src/exec/fallback.rs`: that list is `RETRYABLE_MODEL_FAILURE_PATTERNS`, a structural port of a *different* upstream (`pi-subagents/src/runs/shared/model-fallback.ts`), whose entries do not overlap pi-core's; injecting pi-core literals there would be an unprompted divergence from pi-subagents. Separately, the literals alone are **not sufficient**: pi's are Node-shaped (`getaddrinfo`, `ENOTFOUND`), and `grep -rnE 'getaddrinfo|dns error|failed to lookup' crates --include='*.rs'` → 0 — what string reqwest/hyper actually emits on a failed lookup is undetermined. Determine it first, then add the Rust-side literal alongside pi's.

**Verify** — Unit test over `RETRYABLE_PROVIDER_PATTERNS` asserting each of the seven upstream literals classifies retryable, **plus** a test using the actual reqwest/hyper DNS error string. **Do not close on the literals alone** — see blind spot 1 in `## Coverage`.

## DRIFT-015 — Extension context surface: `scopedModels`, `outputPad`, markdown transformers still absent

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** high · **duplicate-of: `EXT-019`** (markdown-transformer half; `TUI-034` owns the renderer half, `PARITY-GAPS` VL-P21 the cross-cutting entry). The `scopedModels` / `outputPad` halves are uniquely owned here.

**cyrup** — Partially closed. `cyrup/crates/cyrup-ext/wit/world.wit:277-278` now declares `register-provider: func(id: string, config-json: string)` and `unregister-provider: func(id: string)` — the `019e4ad6` half is done. The rest is still absent from the world: `grep -nE 'scoped-models|render-options|output-pad|markdown-transformer' crates/cyrup-ext/wit/world.wit` → 0.

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts` at `v0.84.1`: `scopedModels` `:326`, `outputPad` `:1144`, `MarkdownTransformer` `:1153`, `registerMarkdownTransformer` `:1292`, `markdownTransformer?` `:1703`.

**Impact** — Extensions written against current pi cannot be ported: they cannot read scoped models, cannot control output padding, and cannot transform markdown.

**Fix** — Widen `cyrup-ext/wit/world.wit` (and the byte-identical `cyrup-ext-sdk/wit/world.wit`) with a `render-options` record carrying `output-pad` and width, a `scoped-models` accessor on the context, and a `register-markdown-transformer` host call plus its guest export. Bump the world package version once for the whole batch — DRIFT-034 closed at `@0.4.0`, and `cyrup-ext/tests/wit_world_sync.rs` will now fail if only one copy is edited. Overlaps `PARITY-GAPS` VL-P21 (the markdown-transformer half is area 06's).

**Verify** — A fixture guest component exercising each new call must instantiate and round-trip; `wit_world_sync.rs` must stay green, which requires editing both copies and bumping the version.

## DRIFT-028 — OpenRouter Anthropic cache breakpoint skips tool results

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

*(Halved this pass. The original filing's half (b) — `~anthropic/*` alias ids — is **refuted and dropped**: pi's own runtime detector at `openai-completions.ts:1497` is `provider === "openrouter" && model.id.startsWith("anthropic/")`, byte-equivalent to cyrup's, and `git grep 'startsWith("~' v0.84.1 -- packages/ai/src` → 0. The four `~anthropic/*` ids in cyrup's `openrouter.json` came from pi's own catalog, so **pi misses them identically**. A shared upstream bug is not a cyrup divergence and must not be carried in an equivalence-scoped item.)*

**cyrup** — `cyrup/crates/cyrup-provider/src/api/openai_completions.rs:628-638` `add_cache_control_to_last_conversation_message` accepts only `role == Some("user") || role == Some("assistant")` at `:631`.

**upstream** — `pi/packages/ai/src/api/openai-completions.ts:964-975` (`addCacheControlToLastConversationMessage`) and its sibling `addCacheControlToMessage` at `:999-1004` both accept `"user" || "assistant" || "tool"`.

**Impact** — Cost-only but continuous: in an agent loop the last message is almost always a tool result, so the cache breakpoint lands one message too early on every turn.

**Fix** — Add `|| role == Some("tool")` at `openai_completions.rs:631`. Straight runtime parity; no `[CYRUP-DELTA]` note is needed now that half (b) is gone.

**Verify** — Test that a conversation ending in a tool result gets `cache_control` on that message, and that one ending in an assistant message is unchanged.

## DRIFT-029 — Concurrent user bash: single cancel slot makes abort miss and `is_bash_running` lie

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/session.rs:406` `bash_cancel: Mutex<Option<CancelToken>>` — **one** slot, initialized `:520`, overwritten unconditionally in `execute_bash` at `:4480-4481`, cleared unconditionally at `:4504` and `:4541`, read by `abort_bash` at `:4632-4636` and `is_bash_running` at `:4640-4642` (which answers on `is_some()`, i.e. reports "running" for whatever was last stored). The RPC dispatcher calls in with no serialization, so two concurrent user bash calls need no exotic setup.

**upstream** — pi `2efa728d`; `pi/packages/coding-agent/src/core/agent-session.ts:339` `private readonly _bashAbortControllers = new Set<AbortController>()`, add `:2781`, delete-in-finally `:2805`, iterate a copy `:2843`, `size > 0` `:2850`.

**Impact** — With two bash commands in flight, abort cancels only the most recent — the first is orphaned and keeps running — and the first to finish clears the slot, so `is_bash_running` reports false while a command is still executing. Compounds DRIFT-049 and `SEAM-S03`: the orphan is also the thing no signal path reaches.

**Fix** — Replace `bash_cancel` with a keyed set (`Mutex<HashMap<BashId, CancelToken>>`): insert at `:4480`, remove only the **own** key in the two completion paths `:4504`/`:4541`, iterate a snapshot in `abort_bash`, and make `is_bash_running` a non-empty check.

**Verify** — Test starting two long-running bash calls then `abort_bash`: both must terminate; and after the shorter one completes, `is_bash_running` must still be true while the longer one runs.

## DRIFT-030 — `ANTHROPIC_AUTH_TOKEN` bearer-token auth unsupported

**Kind** **not-ported** *(corrected from `upstream-drift`, repair pass)* · **Severity** medium · **Effort** S · **Confidence** high · **duplicate-of: `PROV-021`**

> **Re-derived two-sided 2026-08-12 (repair pass), replacing a commit hash with evidence at the
> ported tag.** Every cited line already exists at **`v0.83.0`**, so `24e5cc04` (#6148) is
> pre-baseline: this is a **port omission**, not version lag. `PROV-021`
> (`01-cyrup-core-and-provider.md:226`, medium) already records that correction verbatim
> ("Kind corrected: filed `upstream-drift`, it is a **port bug** against v0.83.0"), names the exact
> cyrup line to change (`env_api_keys.rs:39`), and gives an in-tree template for the bespoke
> `ApiKeyAuth` (`providers/cloudflare.rs:71-113`). Work it as `PROV-021`.

**cyrup** — `grep -rnE 'ANTHROPIC_AUTH_TOKEN' crates --include='*.rs'` → 0. `cyrup-provider/src/env_api_keys.rs:39` is `"anthropic" => Some(&["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"])`, and `providers/anthropic.rs` resolves only into `ModelAuth.api_key` (→ `x-api-key`); no env path produces an `Authorization: Bearer` header.

**upstream** — `pi/packages/ai/src/env-api-keys.ts` @**`v0.83.0`**: `:29` defines `ANTHROPIC_AUTH_TOKEN_ENV`, `:73` carries the inline carve-out comment, `:76` returns it **first** for anthropic (`[ANTHROPIC_AUTH_TOKEN_ENV, ANTHROPIC_OAUTH_TOKEN_ENV, ANTHROPIC_API_KEY_ENV]`), `:147` implements the display exclusion (`envKeys.find(key => key !== ANTHROPIC_AUTH_TOKEN_ENV)`); `providers/anthropic.ts:5` imports it and `:21` reads it, resolving to an `Authorization: Bearer` header. At `v0.84.1` the same file carries the same content with two offsets shifted by the added radius entry — `:149` for the display exclusion, `providers/anthropic.ts:24` for the read. The prior filing's "the file moved to `packages/ai/src/env-api-keys.ts`" is misleading: it is at that path at **both** tags.

**Impact** — Users behind an Anthropic-compatible gateway that issues bearer tokens — the common enterprise proxy setup — cannot authenticate with cyrup at all. It also affects auth **status** reporting, since `env_api_keys.rs` is what the login/status pickers consult (`PROV-021`).

**Fix** — Work it as `PROV-021`: add `ANTHROPIC_AUTH_TOKEN` first at `env_api_keys.rs:39`, reproduce pi's `getEnvApiKey` carve-out so it is never turned into a literal api key, and give `providers/anthropic.rs` a bespoke `ApiKeyAuth` that probes it first and returns `Authorization: Bearer <token>` in `ModelAuth.headers`.

**Verify** — Per `PROV-021`: with only `ANTHROPIC_AUTH_TOKEN` set the request carries `Authorization: Bearer …` and no `x-api-key`; with both set the bearer wins; with only `ANTHROPIC_API_KEY` behaviour is unchanged; `api_key_env_vars("anthropic")` reports all three.

## DRIFT-033 — A mid-run tool addition never reaches the system prompt

**Kind** port-divergence · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/hooks.rs:170-181` `prepare_next_turn` spreads `previous`, sets `update.tools = Some(session.next_turn_tools().await)` at `:179` and returns — `update.system_prompt` is **never** assigned on any path. The consumer side is already wired: `cyrup-agent/src/agent.rs` reads `if let Some(prompt) = u.system_prompt { … }`. The reason is stated in place at `hooks.rs:158-161`: cyrup has one prompt slot instead of pi's override/base pair, so re-pushing would undo a `before_agent_start` sanitization mid-run.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:534-554` `_installAgentNextTurnRefresh` returns `{...previousSnapshot, context: {...previousContext, systemPrompt: this._systemPromptOverride ?? this._baseSystemPrompt, tools: this.agent.state.tools.slice()}, …}` — the system prompt is re-pushed **unconditionally, every turn**.

**Impact** — A tool registered mid-run becomes callable (DRIFT-001 closed that) but is never described to the model, so the model has no reason to call it until the next run.

**Fix** — **Refuter caveat: this is not a one-line assignment.** The prerequisite is modelling pi's **two** prompt slots. (a) Split cyrup's single prompt slot into `base` + `override` in `cyrup-session-svc` so a re-push cannot undo a mid-run sanitization; (b) only then set `update.system_prompt` at `hooks.rs:179` to `override ?? base`, mirroring `agent-session.ts:534-554`. The work is entirely in `cyrup-session-svc`, **not** `cyrup-agent`. Until it lands, promote the `hooks.rs:158-161` comment into a documented `[CYRUP-DELTA]` in `cyrup-session-svc/src/lib.rs` — the divergence currently lives only in a private doc comment on a non-public method.

**Verify** — Test registering a tool mid-run and asserting the next turn's system prompt names it, **while** a `before_agent_start` sanitization applied earlier in the same run survives that turn boundary. The second half is what proves the two-slot model, not just the assignment.

---

## DRIFT-010 — `get_available_thinking_levels` RPC dispatch arm missing

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — **Evidence corrected this pass.** The prior filing reported "exactly one hit, a doc comment" — that grep was written with a literal `|`, which plain `grep` does not treat as alternation. Case-insensitively, cyrup **already implements the underlying method**: `AgentSession::available_thinking_levels` at `cyrup/crates/cyrup-session-svc/src/session.rs:3076`, consumed at `:3152`, covered by `cyrup-session-svc/tests/round2.rs:104, :117`. What is missing is only the RPC surface: `SessionCommand` in `cyrup/crates/cyrup-modes/src/rpc.rs` declares `SetThinkingLevel` (`:132`) and `CycleThinkingLevel` (`:134`) and **no** `GetAvailableThinkingLevels`, and there is no dispatch arm.

**upstream** — pi `c1793952` (#6865); `pi/packages/coding-agent/src/modes/rpc/rpc-types.ts:39, :163`, dispatched at `rpc-mode.ts:507-510` (`session.getAvailableThinkingLevels()`), documented at `pi/docs/rpc.md:316-328`. Unchanged at `v0.84.1`.

**Impact** — An RPC embedder cannot discover which thinking levels the current model supports, so it must guess and let the server clamp silently.

**Fix** — Pure wiring, no new logic: add the `GetAvailableThinkingLevels` variant to `SessionCommand` in `cyrup-modes/src/rpc.rs` beside `SetThinkingLevel`/`CycleThinkingLevel`, and a dispatch arm returning `session.available_thinking_levels()` (`session.rs:3076`).

**Verify** — RPC round-trip test asserting the response equals `available_thinking_levels()` for a model with a restricted ladder and for one with the full set.

## DRIFT-016 — `Current date:` still injected into the system prompt

**Kind** **stale-port** *(corrected from `upstream-drift`, repair pass)* · **Severity** low · **Effort** S · **Confidence** high · **duplicate-of: `SESS-019`**

> **Re-derived two-sided 2026-08-12 (repair pass), replacing a commit hash with evidence at the
> ported tag.** The footer is absent from `packages/coding-agent/src` at **`v0.83.0`**, not merely at
> `v0.84.1` — so `f4e9ca74` landed *before* cyrup's baseline and this is a **stale port** (cyrup
> carries behaviour upstream deleted), not version lag. It will not be swept up by a rebase.
> `SESS-019` (`03-cyrup-session.md:382`, medium) owns the same footer plus the extra leading newline
> and the `project_context` wording drift; work it there.

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/builder.rs:330-340` `emit_footer` writes `"\n\nCurrent date: {:04}-{:02}-{:02}"` at `:335`, unconditionally.

**upstream** — pi `f4e9ca74` removed it. `git grep -n 'Current date' <tag> -- packages/coding-agent/src` returns **nothing at all** at **both** `v0.83.0` **and** `v0.84.1` — not merely absent from `system-prompt.ts`. The only occurrences anywhere under `packages/` at either tag are inside `packages/coding-agent/test/fixtures/*.jsonl`, i.e. historical transcript data captured before the removal, which is not a source of truth for the prompt builder.

**Impact** — The system prompt differs from pi's byte-for-byte, and the date changes daily, so prompt-cache prefixes break across midnight.

**Fix** — Delete the write at `builder.rs:330-340`, and the two assertions in DRIFT-035 with it — they will fail otherwise, so the two must land in one change.

**Verify** — Prompt golden snapshots contain no `Current date:`; the same session rendered on two simulated dates produces identical prompts.

## DRIFT-018 — Constrained sampling (strict JSON schema + Lark/regex grammars) absent

**Kind** **not-ported** *(corrected from `upstream-drift`, repair pass)* · **Severity** low · **Effort** L · **Confidence** high · **duplicate-of: `PROV-011`**

> **Re-derived two-sided 2026-08-12 (repair pass), replacing a commit hash with evidence at the
> ported tag.** `constrained-sampling.ts` exists at **`v0.83.0`** and is already consumed there by
> five APIs, so this predates cyrup's baseline: it is a **port omission**, not version lag, and a
> rebase will never sweep it up. **`PROV-011`** (`01-cyrup-core-and-provider.md:154`, medium, effort
> L) is strictly broader — it enumerates all four consuming sites, distinguishes the resolver from
> the already-ported `supports_strict_mode` flag, and names two sites that are actively *wrong*
> rather than merely absent. `TOOL-016` owns the tool-model half and `EXT-024` the WIT half. Work it
> as `PROV-011`.

**cyrup** — `grep -rnE 'constrained_sampling|ConstrainedSampling|GrammarFormat' crates --include='*.rs'` → 0. The only `grammar` hits are unrelated comments (`cyrup-session-svc/src/bash.rs:320`, `cyrup-tools/src/tools/jsnum.rs:23`, `cyrup-provider/src/utils/regexlite.rs:6`).

**upstream** — `pi/packages/ai/src/api/constrained-sampling.ts` @**`v0.83.0`** — 148 lines, seven exports (`GrammarConstrainedSampling` `:9`, `GrammarToolInputJsonBuffer` `:15`, `getGrammarToolInput` `:21`, `appendGrammarToolInputJsonDelta` `:33`, `resolveJsonSchemaStrictSampling` `:84`, `resolveGrammarConstrainedSampling` `:101`, `createGrammarToolInputProperties` `:136`). Already imported at that tag by five APIs: `anthropic-messages.ts:40` (applied `:1298`), `azure-openai-responses.ts:18`, `bedrock-converse-stream.ts:57` (applied `:934`), `google-shared.ts:8` (applied `:316`), `mistral-conversations.ts:28` (applied `:497`). Byte-present at `v0.84.1` with the same export offsets; only the call-site line numbers shift (`bedrock-converse-stream.ts:58`/`:991`, `google-shared.ts:9`/`:328`). pi `24bace27` (#6341) plus follow-up `34239180` are the originating commits and are **pre-baseline**.

**Impact** — Callers cannot force schema- or grammar-constrained output; structured-output use cases fall back to prompt-and-parse. The Google leg additionally never asks Gemini to validate function calls against the declared schema (`PROV-011`'s fourth site), which is the one route where upstream gets a server-side guarantee rather than a hint.

**Fix** — Work it as `PROV-011`: port `constrained-sampling.ts` to `cyrup-provider/src/utils/constrained_sampling.rs`, add `constrained_sampling` to `cyrup_core::Tool`, add `supports_strict_tools` / `supports_openai_grammar_tools` to `ModelCompat`/`ResolvedCompat`/`ResolvedResponsesCompat`, and apply at all four sites. Do not open a second workstream from this ID.

**Verify** — Per `PROV-011`. Per route: a tool declaring `constrainedSampling` on a strict-capable model serializes the merged strict schema byte-equal to pi's; on a non-capable model the schema is unchanged; the Google route emits `functionCallingConfig.mode: "VALIDATED"` exactly when `resolveGoogleFunctionCallingMode` would.

## DRIFT-019 — Radius / Qwen Token Plan providers unregistered

**Kind** **not-ported** *(corrected from `upstream-drift`, repair pass — three of the four predate the baseline)* · **Severity** low · **Effort** M · **Confidence** high · **duplicate-of: `PROV-014`**

> **Re-derived two-sided 2026-08-12 (repair pass), replacing four commit hashes with evidence at the
> ported tag.** `git ls-tree v0.83.0 --name-only packages/ai/src/providers/` already lists
> `radius.ts`, `radius-config.ts`, `qwen-token-plan.ts`, `qwen-token-plan.models.ts`,
> `qwen-token-plan-cn.ts` and `qwen-token-plan-cn.models.ts`. Only `qwen-token-plan-individual.ts`
> (+ `.models.ts`) is new at `v0.84.1`. So **three of the four registrations are port omissions at
> the ported baseline**, not version lag; only the fourth is drift (`PARITY-GAPS` VL-P2).
> `PROV-014` (`01-cyrup-core-and-provider.md:168`, medium) already carries exactly this correction
> in its title ("a v0.83.0 port bug, not lag") and owns `cyrup-provider`.
>
> **The critic's independent re-derivation of the cyrup half, recorded so it is not re-litigated:**
> `grep -rn "radius\|qwen" crates/cyrup-provider/src/providers/all.rs` is empty across the **whole
> file**, not merely the `:145-243` range this item cited. The known `fleet!`-macro grep trap
> therefore does **not** rescue this item — the gap is real.

**cyrup** — Partially closed. The wire API and OAuth landed: `cyrup/crates/cyrup-provider/src/api/pi_messages.rs` exists (its own tests at `:1012`/`:1426` exercise `provider: "radius"`), and `cyrup-provider/src/auth/oauth/radius.rs` is registered at `auth/oauth/load.rs:59`. The **provider registrations** are still absent: `grep -nE 'radius|qwen'` over the whole of `cyrup-provider/src/providers/all.rs` returns 0 hits (the registration block is `:145-243`). `PROV-014` adds two consequences this item omits: `env_api_keys.rs:34-73` has no arm for any of them, and `providers/builtin_oauth.rs:17` states outright that radius has no built-in provider.

**upstream** — @**`v0.83.0`**: `packages/ai/src/providers/{radius.ts, radius-config.ts, qwen-token-plan.ts, qwen-token-plan-cn.ts}` plus their `*.models.ts` re-exports. @**`v0.84.1`**: the same six files, plus `qwen-token-plan-individual.ts` and `qwen-token-plan-individual.models.ts`. Originating commits `bbb91fa8`, `961fa6c1`, `a9f5b1c1`, `4c1a0b92` — recorded as provenance only; the file inventory above is the evidence.

**Impact** — Those providers remain unusable from cyrup even though the hard parts — the `pi-messages` wire format and the Radius OAuth flow — are already built. This is a registration-and-catalog gap, not a feature gap.

**Fix** — Work it as `PROV-014`. Add the four fleet registrations in `providers/all.rs`, the four `env_api_keys.rs` arms, and their catalogs (the same four DRIFT-009 counts); radius additionally needs a `builtin_oauth.rs` arm plus a provider constructor, the same shape as `PROV-029`'s fix. Effort is M, not L, now that `pi_messages.rs` and `oauth/radius.rs` exist.

**Verify** — `providers/all.rs`'s port-status table updated (it is stale for four other providers too — `PROV-030`); a catalog present for each new provider; a faux round-trip through `pi_messages.rs` for radius and through openai-completions for the qwen token plans.

## DRIFT-020 — openai-responses affinity keys on removed `sendSessionIdHeader`

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high · **duplicate-of: `PROV-024`** (`PROV-033` owns the openai-**responses** half, which needs a field *deletion* and is the worse of the two). Both `PROV` items record the kind correction this one does not: `types.ts:566-570` declares `sessionAffinityFormat` at **v0.83.0**, so it is a port bug, not lag.

**cyrup** — `cyrup/crates/cyrup-provider/src/api/compat.rs:157` declares `send_session_id_header: Option<bool>` on `ModelCompat`, `:180` the resolved bool, `:193` the `unwrap_or(true)`; sole consumer `cyrup-provider/src/api/openai_responses.rs:441`. `grep -rnE 'session_affinity_format|sessionAffinityFormat' crates --include='*.rs'` → 0.

**upstream** — pi `298665cf` (#6496). At `v0.84.1` the migration is complete: `sessionAffinityFormat` at `openai-completions.ts:656, :659, :1527` (`isOpenRouter ? "openrouter" : "openai"`) and `:1572`, plus `openai-responses.ts:70, :233`, with the union `SessionAffinityFormat = "openai" | "openai-nosession" | "openrouter"` at `types.ts:112`.

**Impact** — OpenRouter session affinity cannot be expressed — cyrup's boolean **cannot represent the third state** at all. And with DRIFT-007 now ingesting pi-shaped catalog JSON at runtime, an unknown `sessionAffinityFormat` key is dropped silently, with no regeneration step where a human would notice.

**Fix** — Replace `send_session_id_header` with `session_affinity_format` through the declare/resolve/consume chain (`compat.rs:157, :180, :193` → `openai_responses.rs:441`), keeping the boolean as a deprecated alias mapping onto the enum. No test pins the current shape, so nothing must be unwound.

**Verify** — Catalog entries carrying `sessionAffinityFormat` resolve all three values; a legacy `sendSessionIdHeader: false` still suppresses the header.

## DRIFT-022 — TUI mode (`--tui-mode`, alternate screen) not ported

**Kind** tracking · **Severity** **tracker** *(excluded from the severity count, repair pass — was low)* · **Effort** L · **Confidence** high · **duplicate-of: `SEAM-051`**

> **Flag half FIXED 2026-08-13 via `SEAM-051`**; the RENDERER half is still tracking, so this row
> stays open. `--tui-mode regular|fullscreen` now parses (`crates/cyrup/src/cli.rs`), `regular` is a
> working no-op and `fullscreen` is accepted and declined at startup with ADR-0005's interim string
> — so the binary no longer exits 1 claiming pi's own default is an unknown option. Both of pi's
> diagnostics are ported verbatim (`args.ts:180-192` @v0.84.1). What remains here is what this
> tracker was always about: the alternate-screen RENDERER (`TUI-019`) and the `tuiMode` /
> `fullscreenScrollbar` settings keys (`CFG-021`). Evidence in area 08's `SEAM-051` entry.
>
> **Marked tracker 2026-08-12 (repair pass).** Its Fix is "Do **not** implement yet" and its Verify
> is "n/a while tracking", so it proposes no work — bookkeeping, not backlog. The **behavioural**
> cost of the absence is filed and is concrete: `SEAM-051` (`--tui-mode regular`, pi's own default,
> makes the binary exit 1 with a message claiming the option is unknown), `CFG-021` (the `tuiMode` /
> `fullscreenScrollbar` settings keys), `TUI-019` (the renderer mode itself). Those three carry the
> work; this ID tracks the upstream surface so the port is not attempted while pi is still moving it.

**cyrup** — `grep -rnE 'AlternateScreen' crates --include='*.rs'` returns four hits, all in `cyrup/crates/cyrup-tui/src/startup_selector.rs` (`:20` import, `:44` enter, `:52`/`:62` leave) — the one-shot startup session picker, **not** a UI mode. Recorded so nobody mistakes it for the feature. No `tui_mode`/`ui_mode` in `crates/cyrup/src/cli.rs`.

**upstream** — **Item text corrected this pass.** Upstream renamed the concept from "ui mode" to **TUI mode** (pi `5446cd754`); the flag is now `--tui-mode` — independently confirmed as the **only** CLI flag added anywhere in `v0.83.0..v0.84.1` (`git diff v0.83.0..v0.84.1 -- packages/coding-agent/src/cli/args.ts | grep '^+' | grep -oE '"--[a-z-]+"'` → exactly `"--tui-mode"`). `--alt` was **deleted** (pi `c72728bc1`). The surface grew by 1047 lines: `packages/tui/src/tui-alt-screen.ts` is new at `v0.84.1`, plus `components/{scroll-view,alt-screen-flash}.ts`. Earlier commits: `f074efd9`, `c13ffe18`, `ea1e77e2`, `3c717842`, `8ac92f83`, `6129a353`, `b3ed27b3`.

**Impact** — cyrup has only the inline viewport; no fullscreen transcript navigation. Feature absence, not a defect.

**Fix** — Do not implement yet — the upstream surface moved again inside this very delta. When taken, it is a `cyrup-tui` renderer mode plus a `--tui-mode` flag in `crates/cyrup/src/cli.rs`. Same gap as `PARITY-GAPS` VL-P19, which owns the `packages/tui` half (`layout.ts`, `layout-node.ts`, the stack components); `latex.ts` from that same set **is** already ported at `cyrup-tui/src/markdown/latex.rs` and was verified this pass.

**Verify** — n/a while tracking.

## DRIFT-023 — Model registry → `ModelRuntime` refactor not absorbed

**Kind** tracking · **Severity** **tracker** *(excluded from the severity count, repair pass — was low)* · **Effort** L · **Confidence** low · **duplicate-of: `CFG-020`** · **LEAD — not yet evidenced**

> **Carried forward UNVERIFIED, for the second consecutive pass.** The 2026-08-12 re-audit was
> **refuted on process** — neither side was re-read — and the repair pass did not re-derive it
> either: the re-derivation budget went to the six items whose *kind* was in doubt, where the payoff
> was a reclassification. So the evidence below is still inherited from the `1806375` filing plus an
> unconfirmed claim that `model-runtime.ts` gained `+274/-82` in the delta. **Do not act on these
> citations without re-deriving both sides.** It is listed in `## Leads — not yet evidenced` with
> the two commands that would settle it, and it is **excluded from the item count** because it
> proposes no work. `CFG-020` (`05-cyrup-config-and-resources.md:335`) owns the cyrup-side question
> and is in the area that owns `cyrup-config`.

**cyrup** — There is no cyrup `ModelRuntime` type; the reported hits are doc citations in `cyrup-session-svc/src/services.rs` and `cyrup-session-svc/tests/settings_resolve.rs`. *(Unverified this pass.)*

**upstream** — pi `9993c969`. Both `model-registry.ts` and `model-runtime.ts` still coexist in `pi/packages/coding-agent/src/core/` at `v0.84.1`, so the migration is in progress, not finished. Upstream keeps building on it (`ab366ebe` custom compaction through the provider, `bd9e09db` dynamic provider refresh; credential serialization is `PARITY-GAPS` VL-P24, area 05).

**Impact** — Divergence compounds: DRIFT-007 and settings-declared models each bolted a payload onto the old registry, so a later port has more to unwind.

**Fix** — Deferred by design; re-scope once pi finishes the migration and deletes `model-registry.ts`. Related in kind to DRIFT-040 — both are "pi is mid-refactor, track don't port".

**Verify** — n/a while tracking. First action next pass: re-derive both sides so this item's confidence can go back up.

## DRIFT-024 — AGENTS.md loaded twice in nested git worktrees

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high · **duplicate-of: `SESS-013`** (`03-cyrup-session.md:172`, medium) — better evidenced there: it names both load-bearing guards (`resource-loader.ts:108` worktree-prefix, `:113` canonical `.git`) and the `isShadowed` gate at `:140-142`, and routes the realpath helper to `SESS-036`.

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/context_files.rs` (173 lines) de-duplicates only by exact path — `seen.insert(cf.path.clone())` at `:118` and `:136` and nowhere else — and the file contains no worktree/shadow logic at all.

**upstream** — pi `cced6a21` (#7221) added `findShadowedContextFile`, declared at `pi/packages/coding-agent/src/core/resource-loader.ts:100` and called at `:136`.

**Impact** — In a nested worktree the same guidance is injected twice, wasting context and doubling any instruction weight.

**Fix** — Port `findShadowedContextFile` into `context_files.rs`: before the `seen` insert at `:118`/`:136`, drop a candidate whose path is shadowed by a nearer worktree root.

**Verify** — Test with a worktree nested inside its main repo, both carrying AGENTS.md: exactly one is loaded, the nearer one.

## DRIFT-025 — `${@:-default}` prompt-template defaults render literally

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high · **duplicate-of: `CFG-017`** (`05-cyrup-config-and-resources.md:419`) — the area that owns `cyrup-resources`, where the fix lands.

**cyrup** — `cyrup/crates/cyrup-resources/src/prompt.rs:243-246`: the `:-` branch requires `!num.is_empty() && num.bytes().all(|b| b.is_ascii_digit())`, so `${@:-none}` and `${ARGUMENTS:-none}` fall through; the `${@:N}` branch at `:258-264` then rejects them on the digits-only `start_str` guard, returning `None`.

**upstream** — pi `64f83c85` (closes #6695); `pi/packages/coding-agent/src/core/prompt-templates.ts:74-80` — the regex alternation is `\$\{(\d+|ARGUMENTS|@):-([^}]*)\}`, i.e. `@` and `ARGUMENTS` **are** valid `:-` targets and resolve to `allArgs`, documented at `:62-63`.

**Impact** — A template written for pi renders `${@:-none}` literally into the prompt instead of substituting the default.

**Fix** — Allow `num == "@"` and `num == "ARGUMENTS"` in the `:-` branch at `prompt.rs:243-246`, substituting all arguments when present and the default when absent.

**Verify** — Unit tests: `${@:-none}` with no args yields `none`, with args yields the joined args; `${ARGUMENTS:-none}` behaves identically; `${1:-x}` unchanged.

## DRIFT-027 — openai-completions has no `deferredToolsMode: "kimi"`

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high · **duplicate-of: `PROV-025`** (`01-cyrup-core-and-provider.md:338`) — which records the kind correction this one does not (`types.ts:567` declares `deferredToolsMode?: "kimi"` at **v0.83.0**, so it is a port bug, not lag) and warns that `getDeferredToolNames` is a different accessor from `splitDeferredTools`.

**cyrup** — `grep -rnE 'deferred_tools_mode|deferredToolsMode' crates --include='*.rs'` → 0. Neither `ModelCompat` (`cyrup/crates/cyrup-provider/src/api/compat.rs:73-168`) nor `ResolvedCompat` (`:205-230`) carries the field, so a catalog entry declaring it is silently dropped.

**upstream** — pi `f16b4e0c` + `70c57632`. At `v0.84.1`, `pi/packages/ai/src/api/openai-completions.ts:160` lists `deferredToolsMode` in the detected-compat key union and `:163` in the declared compat — and it is **actually consumed** at `:728` (`compat.deferredToolsMode === "kimi" ? getDeferredToolNames(...)`) and `:1272`, so this is live behaviour, not a dormant field.

**Impact** — Kimi models get the default deferred-tools rendering, so tool-heavy prompts are shaped wrongly for that provider.

**Fix** — Add `deferred_tools_mode` to `ModelCompat`/`ResolvedCompat` following the `cache_control_format` pattern exactly (declare, resolve, detect, precedence), and branch on it in `cyrup-provider/src/utils/deferred_tools.rs` at the site corresponding to `openai-completions.ts:728`.

**Verify** — Test that a catalog entry declaring `deferredToolsMode: "kimi"` survives resolution and changes the rendered tool payload; default mode unchanged.

## DRIFT-031 — No usage cost breakdown — per-model attribution and `Tools/summaries` unsurfaced

**Kind** **not-ported** *(corrected from `upstream-drift`, repair pass)* · **Severity** low · **Effort** M · **Confidence** high · **duplicate-of: `PROV-036`**

> **Re-derived two-sided 2026-08-12 (repair pass), replacing a commit hash with evidence at the
> ported tag.** `getUsageCostBreakdown` is declared at `usage-totals.ts:37` at **`v0.83.0`** with a
> live consumer already wired there, so `2fd38684` (#6671) is pre-baseline: a **port omission**, not
> version lag. **`PROV-036`** (`01-cyrup-core-and-provider.md:550`, low) owns it and is better
> evidenced — it records that the *totals* half **is** already ported
> (`add_usage_totals`, `cyrup-tui/src/status.rs:170`, called from `app.rs:4523` and `status.rs:155`),
> so only the breakdown is missing, and it names pi's `usageBreakdown.length > 1` render gate. Work
> it as `PROV-036`.

**cyrup** — `grep -rnE 'cost_breakdown|CostBreakdown|usage_cost' crates --include='*.rs'` → 0. `crates/cyrup-tui/src/app.rs:4203`/`:4216` render a single `| cost | ${:.3} |` row from `stats.cost`; there are no per-model rows and no `Tools/summaries` bucket. *(The prior filing's "No cost figure" is an overstatement — a single total is rendered; what is missing is the breakdown.)*

**upstream** — `pi/packages/coding-agent/src/core/usage-totals.ts:37` @**`v0.83.0`** `export function getUsageCostBreakdown(entries: SessionEntry[]): UsageCostBreakdownEntry[]`, with `interface UsageCostBreakdownEntry` at `:30-36`; imported at `modes/interactive/interactive-mode.ts:94` and consumed at `:5665` — **live at the ported baseline**, not dead code. At `v0.84.1` the declaration is at the same `:37`; only the consumer offsets shift (import `:101`, call `:6001`). Keyed `${provider}/${responseModel ?? model}`, with a bucket literally named `Tools/summaries` absorbing toolResult, branch-summary and compaction usage so the breakdown reconciles with the session total.

**Impact** — Users cannot see which model or which tool/summary traffic drove a session's cost — only the single total.

**Fix** — Work it as `PROV-036`. DRIFT-011 (closed) already supplies the data (`usage` on tool results, compaction and branch summaries). Port the breakdown half beside the existing totals code, keyed on `provider/response_model.unwrap_or(model)`, reproducing the `Tools/summaries` bucket **by name and by membership**, and render it in `cyrup-tui/src/app.rs:4192-4218` under pi's `len() > 1` guard.

**Verify** — Per `PROV-036`. Test a session using two models and containing a compaction: the breakdown attributes tokens per model, the compaction's usage lands in the summaries bucket, and totals match `session_stats()`.

## DRIFT-032 — llama.cpp router integration and Hugging Face model search entirely unported

**Kind** **not-ported** *(corrected from `upstream-drift`, repair pass)* · **Severity** **tracker** *(excluded from the severity count, repair pass — was low)* · **Effort** L · **Confidence** high *(was medium)* · **duplicate-of: `EXT-027`**

> **Re-derived two-sided 2026-08-12 (repair pass), replacing six commit hashes with a file
> inventory at both tags.** The prior edition's upstream evidence was "pi `f1a466b1` plus follow-ons
> `3da591ab`, `864b35c4`, `6abb4b06`, `2a2b0a39`, `0c32e83a`; all still present at `v0.84.1`" — six
> hashes and a presence assertion with no path and no line. `git ls-tree -r <tag> --name-only
> packages/` settles it in one command, and settles the classification with it: the whole surface
> exists at **`v0.83.0`**, so it is a **port omission**, not version lag. Confidence rises from
> medium to high on that evidence. **Marked tracker**: its Fix is "Defer until DRIFT-019 and
> DRIFT-009 are settled" and its Verify is "n/a until scoped" — it proposes no work.

**cyrup** — The only `llama` hits are incidental comments and overflow patterns (`cyrup-provider/src/auth/helpers.rs:23`, `api/openai_completions.rs:1221`, `utils/overflow.rs:28` and `:173`). **Grep trap, recorded deliberately:** cyrup *does* ship a `huggingface` provider **catalog** (registered via the fleet macro at `providers/all.rs:25`, with model ids in `tests/catalog_data.rs:212, :306`) — a name-grep "succeeds" here for an unrelated reason. There is no router integration and no HF model **search**.

**upstream** — present and byte-identical in inventory at **both** `v0.83.0` and `v0.84.1`:
`packages/coding-agent/src/extensions/llama/index.ts`, `client.ts`, `provider.ts`, `ui.ts`,
`huggingface.ts`; `packages/coding-agent/docs/llama-cpp.md`; `packages/coding-agent/test/llama-extension.test.ts`. (The `packages/ai/src/providers/huggingface{,.models}.ts` pair is the unrelated provider catalog that produces cyrup's grep hit, and is **not** part of this surface.) Commits `f1a466b1` (2026-07-17), `3da591ab`, `864b35c4`, `6abb4b06`, `2a2b0a39` (#7072), `0c32e83a` (#7258) are recorded as provenance only; the inventory above is the evidence.

**Impact** — No local-model workflow: cyrup cannot drive a llama.cpp router or discover models on Hugging Face.

**Fix** — Large, self-contained feature port. **Note the crate has changed with the classification:** upstream ships this as a *bundled extension* under `packages/coding-agent/src/extensions/`, not as a provider, which is why **`EXT-027`** (`06-cyrup-ext.md:601`, "pi's bundled llama.cpp router extension has no counterpart") is the owning item and area 06 the owning area — not `cyrup-provider` as this item's prior Fix assumed. Defer until DRIFT-019 / `PROV-014` and DRIFT-009 / `PROV-018` are settled.

**Verify** — n/a while tracking.

## DRIFT-035 — TEST DEFECT: prompt tests assert the `Current date:` footer, pinning DRIFT-016

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high · **duplicate-of: `SESS-019`** (same footer; `SESS-019` owns the production write, this ID owns the two assertions that defend it)

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/tests.rs:82` — `assert!(out.contains("Current date: 2026-06-28"), "date footer")` — and `:113` — `assert!(out.contains("Current date: 2026-06-28"), "footer kept")`. Both present, both **unannotated**, both defending the write site at `prompt/builder.rs:335` (DRIFT-016), confirmed still emitting.

**upstream** — pi removed the footer in `f4e9ca74`. **Re-derived two-sided this repair pass:** `git grep 'Current date' <tag> -- packages/coding-agent/src` returns nothing at **`v0.83.0`** as well as at `v0.84.1`, so these assertions pin a **pre-baseline stale port**, not a lag the next rebase will resolve. The only occurrences under `packages/` at either tag are historical transcripts in `packages/coding-agent/test/fixtures/*.jsonl`.

**Impact** — Two green assertions actively defend known drift. Neither message signals whether it encodes parity or pinned debt, so the next reader will assume parity and leave it.

**Fix** — Annotate both with `// DRIFT-016: pins known drift — delete with builder.rs:335` **now**, even though DRIFT-016 is deferred; delete both when DRIFT-016 lands. The two changes cannot be separated — DRIFT-016 cannot go green without this.

**Verify** — Deleting the footer in `builder.rs` fails exactly these two assertions and nothing else.

## DRIFT-036 — TEST DEFECT: `settle()` uses a fixed 50 ms sleep as the only synchronization

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/tests/summarization_retry_events.rs:98-104`: `async fn settle()` is verbatim `for _ in 0..10 { tokio::task::yield_now().await; }` then `tokio::time::sleep(Duration::from_millis(50)).await;`, while events are drained by a separately spawned collector task. The correct poll-until-observed pattern already exists in the same file.

**upstream** — pi's equivalent suites synchronize on observed state, not elapsed time — the rendezvous pattern at `pi/packages/agent/test/agent-loop.test.ts:589-612`.

**Impact** — Under load the drain may not have happened when assertions run. The **negative** assertion among the call sites is the worse half: it passes vacuously whenever the drain has not happened, so it proves nothing while looking green.

**Fix** — Replace `settle()` with the poll-until-observed helper already present in the file, bounded by a generous timeout; convert the negative assertion into "poll until the positive precondition holds, then assert the absence".

**Verify** — Both variants pass with the sleep removed entirely and under a single-worker runtime; the negative case must fail if the forbidden event is emitted. Fix in one pass with DRIFT-039 via a shared poll helper.

## DRIFT-039 — TEST DEFECT: `a_02_2_parallel_completion_vs_source_order` asserts a wall-clock bound and a completion ORDER it cannot control

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high · **duplicate-of: `AGENT-019`** — *literally the same test*, `crates/cyrup-agent/tests/agent_loop.rs:327`. Fix it once. This body carries the better fix sketch (the `agent-loop.test.ts:589-612` rendezvous); fold that into `AGENT-019` rather than working both, and deduplicate the pair before the test-defect sweep (`00-residual-ledger.md` cluster F3) so it is not booked twice.

**cyrup** — `cyrup/crates/cyrup-agent/tests/agent_loop.rs:327`: `assert!(elapsed < Duration::from_millis(115), …)`. `Instant::now()` is taken at `:277` **before** `agent.prompt` and read at `:281` **after** `wait_for_idle()`, so the budget also covers the faux stream and idle settling, not just the two tool sleeps (80 ms and 50 ms) — concurrent floor ≥ 80 ms, remaining margin under 35 ms, on a 4-worker multi-thread runtime in a suite `cargo test` runs many-at-once. `:301` additionally asserts `ends == vec!["fast", "slow"]` and `:302` `assert_ne!(ends, starts)`, requiring the 50 ms tool's `ToolExecutionEnd` to be observed before the 80 ms one — a 30 ms scheduling margin. All three unchanged at HEAD.

**upstream** — pi proves the same property **causally**, with no timer: `pi/packages/agent/test/agent-loop.test.ts:589-612` uses a `firstDone` promise plus a `releaseFirst` resolver and asserts `parallelObserved`; the mirror test asserts `false` for the forced-sequential case. No elapsed-time assertion exists anywhere in pi's parallel-tool coverage.

**Impact** — A phantom red on an unrelated change whenever the machine is loaded. Worse, the diagnostic reads "parallel ran concurrently", so a contributor who hits it goes hunting a concurrency regression in `cyrup-agent` that is not there.

**Fix** — Replace the timing proof with a rendezvous mirroring `agent-loop.test.ts:589-612`. Give `SpanTool` an optional `Arc<Notify>`/oneshot pair: the slow tool waits on a gate; the fast tool sets an `AtomicBool overlap_observed` if it enters while the slow tool has not exited, then releases the gate. Assert `overlap_observed`, delete the `elapsed` bound at `:327` and the `Instant` at `:277`/`:281`, and derive the completion-order assertion at `:301` from release order rather than racing sleeps. Keep the `spans` vector for the sequential test, where non-overlap is a genuine invariant.

**Verify** — The rewritten test passes with both tool sleeps set to 0 ms, still **fails** when both tools are forced to `ExecMode::Sequential`, and stays green under `taskset -c 0` / a single-worker runtime — which the current version cannot.

## DRIFT-040 — pi's agent-harness v2 rearchitecture entirely unabsorbed

**Kind** tracking · **Severity** **tracker** *(excluded from the severity count, repair pass — was low)* · **Effort** L · **Confidence** medium · **duplicate-of: `PARITY-GAPS` VL-P22** · **LEAD — not yet evidenced**

> **Marked tracker and flagged as a lead 2026-08-12 (repair pass).** Its Fix is "Do **not** port
> now" and its Verify is "n/a while tracking", so it proposes no work. Separately, three of its
> load-bearing claims are still inherited rather than read — see `## Leads — not yet evidenced` for
> exactly which, and which were confirmed first-hand and are **not** in doubt. `PARITY-GAPS` VL-P22
> carries the `jsonl/storage.ts` half that routes to area 03; README blind spot 4 records that the
> harness-v2 half of VL-P22 was orphaned, which this ID now explicitly owns.

**cyrup** — cyrup ports `pi/packages/agent/src/harness/{session,compaction,system-prompt}` into `cyrup-session`. None of the v2 surface exists: no repository/factory layer over sessions, no shutdown lifecycle, no swappable search index, no per-session store queue. `cyrup/crates/cyrup-session/src/entry.rs` is still the pre-v2 shape.

**upstream** — The surface **accelerated inside this delta** rather than settling. Confirmed first-hand at package level (`git ls-tree v0.83.0 packages/` vs `v0.84.1`): `packages/storage` was **renamed to `packages/session-backends`**, and `packages/{telemetry,protocol,client}` are new. `packages/agent/src/harness/session/types.ts` exists at `v0.84.1` with a reshaped `CompactionEntry` (`:44-58`) that carries `retainedTail` and **no** `firstKeptEntryId`, while the coding-agent format cyrup ports still declares `firstKeptEntryId: string` required (`session-manager.ts:69`). *Carried forward unverified:* the `agent-harness.ts` rewrite (`+420/−996`), `docs/harness-v2.md` (`+2124/−367`) and the sqlite-node rebuild (`+12598/−3479`) were **not** re-read this pass. Earlier representative commits: `9e7582aa` (#6594 sqlite session storage + `getPathToRootOrCompaction`), `82c48598`, `bc031ae4`, `871a9904`, `62f3c61b`, `4e81b796`, `2700511e`, `a1bb7e44`, `3c2b71b5`, `5e48d41e`, `a80008b96` (the storage rename).

**Impact** — Structural rather than behavioural today — cyrup's session layer works — but this is the largest single unabsorbed surface in pi core and where every future harness fix will land. **This item now also owns the harness-v2 session-format interop question** that DRIFT-037 mis-filed as a standalone medium: cyrup can read the coding-agent compaction shape it ports, and a harness-v2-written compaction (summary + `retainedTail`, no `firstKeptEntryId`) is a *different format*, not a missing field. If pi's coding-agent path ever adopts the v2 entry shape, that becomes a concrete interop defect and should be filed **then**, with the coding-agent-side evidence.

**Fix** — Do **not** port now; the surface is still moving. Track as with DRIFT-022/DRIFT-023. When taken, the entry points are the repository/factory seam and the Storage-vs-Search split — cyrup's JSONL store becomes one `Storage` impl rather than the only one. `packages/session-backends/sqlite-node` stays out of scope per the existing no-counterpart rule, **but see blind spot 4**: that rule was written before harness-v2 made `Storage` a swappable interface.

**Verify** — n/a while tracking. Re-scope once `git -C pi log --oneline -- packages/agent/src/harness` goes quiet for a release cycle, then diff `harness/session/types.ts` against `cyrup-session/src/entry.rs` and `agent_message.rs` to size the real delta.

## DRIFT-042 — `/login` launches no browser and its "Cmd+click to open" hint is not a link

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

*(Severity corrected from medium this pass: the URL is still printed and the flow still completes, which puts this below same-file mediums like DRIFT-030, where an entire auth mode is unreachable.)*

**cyrup** — `cyrup/crates/cyrup-tui/src/login_dialog.rs:213-222` `show_auth` pushes the raw URL as `LoginLineKind::Accent` (`:216`) and `click_hint()` as `LoginLineKind::Dim` (`:217`); `show_device_code` at `:226-234` does the same (`:229`/`:230`). `click_hint()` at `:69-76` returns the literal string "Cmd+click to open" / "Ctrl+click to open". `LoginLineKind::style` (`:92-101`) maps kinds to **colours only**, so no OSC-8 sequence is ever emitted for these lines. And nothing anywhere spawns a browser: `grep -rnE 'xdg-open|rundll32|open_browser|FileProtocolHandler' crates --include='*.rs'` → 0. The gap is self-documented at `login_dialog.rs:212` ("pi additionally calls `openBrowser(url)`") — disclosed, never closed.

**upstream** — `pi/packages/coding-agent/src/utils/open-browser.ts:10-24` `openBrowser`: darwin `open`, win32 `rundll32 url.dll,FileProtocolHandler`, else `xdg-open`; spawned **detached and never through a shell** (the comment at `:5-8` is a security note), launcher failure swallowed. `pi/packages/coding-agent/src/modes/interactive/components/login-dialog.ts:99-100` wraps the URL as `\x1b]8;;${url}\x07${url}\x1b]8;;\x07`, `:103-104` wraps the **click hint in the same OSC-8 sequence** so the hint itself is the clickable target, and `:111` calls `openBrowser(url)`. `showDeviceCode` at `:118-131` applies the same OSC-8 wrapping but deliberately does **not** open a browser.

**Impact** — On every `/login` OAuth flow — the first thing a new user does — cyrup prints a URL and a hint telling the user to click it. The hint is not a link, so clicking does nothing, and no browser opens; the user must select and copy the URL out of a raw-mode TUI by hand. pi lands the user in the browser with zero interaction.

**Fix** — Two halves, and **only the first is new here.** (a) Add `crates/cyrup-tui/src/open_browser.rs` porting `open-browser.ts:10-24` verbatim — `std::process::Command` with `open` / `rundll32 url.dll,FileProtocolHandler` / `xdg-open`, `.spawn()` detached, errors ignored, explicitly **not** through a shell — and call it at the end of `login_dialog.rs::show_auth` (`:221`). Leave `show_device_code` alone: upstream deliberately does not open a browser there. (b) The OSC-8 half is an instance of an already-filed class — **`TUI-020` — OSC-8 hyperlink capability detected and tested but never emitted** (`07-cyrup-tui.md:398`). Cross-reference it rather than duplicating the emitter work; the substrate cyrup needs already exists (`cyrup-tui/src/image.rs:430-444` `HYPERLINKS`/`hyperlinks_supported`/`seed_hyperlink_support`, and `render_with_hyperlink_support` re-exported at `cyrup-tui/src/lib.rs:147`). The escape must be emitted at **paint** time, not stored in the cell text (see the note at `cyrup-tui/src/markdown.rs:136`).

**Verify** — Inject a spawn hook so a test asserts `open_browser` is invoked exactly once with the auth URL on `show_auth` and **zero** times on `show_device_code`, matching `login-dialog.ts:111` vs `:118-131`. The link-target assertion belongs to `TUI-020`'s test.

## DRIFT-045 — Ctrl+V with text on the clipboard inserts nothing

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:1667-1671`: `Action::ClipboardPasteImage` (bound to Ctrl+V) calls `try_paste_clipboard_image_path()` and, when it returns `false`, deliberately "falls through to the editor (text) handling below" (`:1670`). The editor has no system-clipboard **read** — the only clipboard entry points in the workspace are `copy_to_clipboard` (`app.rs:4996`, write-only) and `read_clipboard_image_to_temp` (`app.rs:5033`, images only); `grep -rnE 'read_clipboard_text|wl-paste|WAYLAND' crates --include='*.rs'` → 0. So a Ctrl+V whose clipboard holds text reaches the editor as a bare key event and inserts nothing. The comment at `:1664-1666` asserts the fall-through preserves "normal Ctrl+V behavior", which is true only for terminals that themselves map Ctrl+V to a bracketed paste — most do not. **cyrup's own help table advertises the behaviour**: `app.rs:2101` describes `{paste_image}` as "Paste image or text from clipboard".

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2845-2868` `handleClipboardPaste` tries `readClipboardImage()` first (`:2847`) and, only when that yields nothing, does `const text = await readClipboardText(); if (text) this.editor.insertTextAtCursor?.(text)` (`:2860-2864`) — the text branch is the explicit fallback, not the terminal's job. `readClipboardText` is at `pi/packages/coding-agent/src/utils/clipboard.ts:53-80` and **gained a Wayland branch in this delta** (pi `bfc679d5e`): `:39-46` `readWaylandClipboardText` shells `wl-paste --no-newline --type text` with a 50 MB buffer and 5 s timeout, gated at `:54` on `platform() === "linux" && isWaylandSession() && process.env.WAYLAND_DISPLAY`, falling back to the native module otherwise. The same function also backs right-click paste at `interactive-mode.ts:2836`.

**Impact** — Ctrl+V is dead for text. Users who paste a URL, an error message or a code snippet get silence and no diagnostic — against a help table that says it should work. On Wayland specifically, even a terminal-mediated paste can fail because the native clipboard module is X11-oriented, which is exactly why upstream added the `wl-paste` branch in this release. *(Note cyrup already shells `wl-copy` in `copy_to_clipboard`, so the asymmetry is one-directional.)*

**Fix** — Add `read_clipboard_text()` beside `read_clipboard_image_to_temp` in `cyrup-tui` (or a small `clipboard.rs`), porting `clipboard.ts:39-46` + `:53-80`: on Linux with `WAYLAND_DISPLAY` set, shell `wl-paste --no-newline --type text` with the same 5 s timeout and treat any error as "fall through"; otherwise use the native backend cyrup already links for images. Then in `app.rs:1667-1671`, when `try_paste_clipboard_image_path()` returns `false`, call it and `self.state.editor.insert_str(&text)` before falling through — matching `interactive-mode.ts:2860-2864`'s ordering exactly (image first, text second).

**Verify** — Unit test the ordering with an injectable clipboard backend: image present → path inserted, text ignored; image absent + text present → text inserted; both absent → editor unchanged and the key still falls through. Add a Wayland-branch test asserting `wl-paste` is selected only when `WAYLAND_DISPLAY` is set, and that a non-zero exit falls back rather than surfacing an error.

## DRIFT-046 — `normalizeWindowsShellPath` unported: Git-Bash / MSYS / Cygwin / WSL drive paths unconverted

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high · **duplicate-of: `TOOL-036`** — *found this repair pass and missing from the ledger's F4 table*. `TOOL-036` (`04-cyrup-tools.md:462`, low) covers the same `paths.ts:67-73` body and the same `:83-85` placement, **plus** the `os.homedir()` half this item omits (`cyrup-tools/src/path.rs:91-93` resolves `$HOME` only, so `~` is broken on Windows independently of the drive-path rule), and it lands in the crate that actually owns the normalizer. `PARITY-GAPS.md:652` already couples the two under one Windows-is-a-declared-target question — answer that once for `PB-19`, `DRIFT-046`, `TOOL-036` and `TOOL-038`.

**cyrup** — `grep -rnE 'normalize_windows_shell_path|cygdrive' crates --include='*.rs'` → 0. cyrup's path normalization has no Windows drive-form branch at all. **Scope caveat, stated so this is not overrated:** the workspace carries only six `cfg(windows)`/`target_os = "windows"` sites, so Windows is barely a target today — it is worth filing precisely because it will be invisible until someone tries. See blind spot 5.

**upstream** — `pi/packages/coding-agent/src/utils/paths.ts:66-73` `normalizeWindowsShellPath` — returns the input unless it starts with a single `/` and contains no backslash, then matches `/^\/(?:mnt\/|cygdrive\/)?([a-z])(?:\/(.*))?$/i` and rebuilds `C:\…` with `/` → `\`. Applied **unconditionally inside `normalizePath`** at `:83-85` (`if (process.platform === "win32") …`), i.e. on every normalized path, not at one call site. **New in this delta** (pi `9524d3a58`; `git show v0.83.0:…/paths.ts | grep normalizeWindowsShellPath` → nothing). Pinned by `pi/packages/coding-agent/test/paths.test.ts:133-150`: `/c/Users/example/project` → `C:\Users\example\project`, `/cygdrive/d/work` → `D:\work`, `/mnt/e/source` → `E:\source`, `/c` → `C:\`, plus a list of forms that must pass through untouched.

**Impact** — On Windows, a path typed in the shell form every Git-Bash/WSL user actually uses — `/c/Users/me/proj`, `/mnt/d/src` — is passed to the Win32 API verbatim and fails to resolve. It reaches every surface that normalizes a path: `@file` mentions, `--session-dir`, tool arguments, package installs. The failure is a confusing "not found" on a path the user can see exists.

**Fix** — Port the function into `cyrup-resources` (wherever `normalize_path` lives) as a `#[cfg(windows)]`-gated step applied **inside the shared normalizer**, mirroring `paths.ts:83-85`'s placement — one site, not per-caller. Keep the guard clauses exactly: bail on `//` (UNC), bail if a backslash is already present, uppercase the drive letter, replace `/` with `\` in the suffix, emit `C:\` for a bare `/c`.

**Verify** — Port `pi/packages/coding-agent/test/paths.test.ts:133-150` verbatim as a Rust unit test, **including the pass-through cases** — those are what stop the rule from mangling legitimate POSIX paths on non-Windows hosts.

## DRIFT-047 — `packages/telemetry` and the `pi.ai.request` span contract absent

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** high · **duplicate-of: `PARITY-GAPS` VL-P5**

> **Duplicate note (refuter):** this is substantially `PARITY-GAPS` **VL-P5** (`telemetryContext` on request options), which already cites `types.ts:122-123` and `simple-options.ts:36` and already records that `packages/telemetry` entered the dependency closure. The **only incremental content here is the span schema**. Resolve this by **extending VL-P5**, not by opening a second L-sized workstream.

**cyrup** — No telemetry seam: `grep -rnE 'TelemetryContext|TelemetrySpan|telemetry_context|start_span' crates --include='*.rs'` → 0, and `StreamOptions` in `cyrup-provider/src/stream.rs` carries no telemetry field. What cyrup has under the name is **unrelated**: `cyrup-config/src/env.rs:77` (`CYRUP_TELEMETRY`/`PI_TELEMETRY`) and `cyrup-config/src/policy.rs:25-27` are the *install*-telemetry opt-in — the port of `coding-agent/src/core/telemetry.ts` (13 lines, one export), which **is** ported (verified this pass, with one edge defect: DRIFT-050).

**upstream** — `packages/telemetry` is a **new top-level package** at `v0.84.1` (confirmed by `git ls-tree v0.83.0 packages/` vs `v0.84.1`; `packages/protocol` and `packages/client` are new in the same delta, and `packages/storage` became `packages/session-backends`). `src/index.ts:14-16` `TelemetryContext.startSpan`, `:18-22` `TelemetrySpan` (`addEvent`/`setAttributes`/`setStatus`), `:47-69` the schema vocabulary, `:72-74` `defineTelemetrySchema`; plus `src/memory.ts` (219 lines), `src/noop.ts`, `src/testing/conformance.ts` (315). `packages/agent/src/harness/telemetry.ts` (615 lines, also new) declares the concrete contract at `:42-118`: one span `pi.ai.request` with six start attributes (`pi.ai.operation` restricted to `stream|fetch_deferred|cancel_deferred|generate_images`, provider, model, api, streaming, deferred) and twelve end attributes (response model/id/stop_reason, `http.status_code`, six usage counters, `usage.cost`, `stream.chunk_count`, `stream.time_to_first_chunk_ms`, `error.type`), with `startAiSpan` at `:138-144`. It reaches the port's dependency closure through `packages/ai`: `types.ts:120-123` declares `telemetryContext?: TelemetryContext` on request options and `api/simple-options.ts:36` threads it. **Honest scope note:** pi has no internal emitter yet — `simple-options.ts:36` is the only forwarding site — so this is SDK-surface parity today, and the schema is the durable part.

**Impact** — An embedder cannot attach a parent span to a cyrup request, so provider calls emit no correlated traces and there is no way to answer "which model call in which turn cost the 40 s". More consequentially for the port, the span schema is upstream's declared observability contract: every future harness/provider instrumentation will be phrased against `pi.ai.request`'s attribute names, and each will need manual re-derivation until the vocabulary exists here.

**Fix** — Do not chase pi's TypeScript generics. Take the two durable pieces. (1) A `TelemetryContext`/`TelemetrySpan` trait pair in `cyrup-core`, with a no-op default so nothing else changes. **API correction (refuter): `startSpan` is callback-SCOPED** — `startSpan<T>(options: SpanOptions, callback: (span) => T | Promise<T>): Promise<T>` (`packages/telemetry/src/index.ts:14-16`) — so the Rust signature must take a closure (`fn in_span<T>(&self, opts, f: impl FnOnce(&dyn TelemetrySpan) -> T) -> T`), **not** return a guard; a guard-returning port will not match pi's nesting semantics. (2) A `telemetry_context: Option<Arc<dyn TelemetryContext>>` on `StreamOptions` in `cyrup-provider/src/stream.rs`, threaded exactly as pi threads it at `api/simple-options.ts:36`. Encode the `pi.ai.request` attribute names as consts so an adapter can map them; a `tracing`-backed adapter is the natural Rust implementation and is a mechanism difference, not a behaviour one.

**Verify** — A test telemetry context records spans in memory (`packages/telemetry/src/memory.ts` is the reference): assert one `pi.ai.request` span per logical provider call, that the six required start attributes are present with pi's exact names, that `pi.ai.operation` is one of the four allowed values, and that a failed call sets status `error` with the error-type attribute. Assert the no-op default allocates nothing when no context is supplied.

## DRIFT-050 — `CYRUP_TELEMETRY=` (set but empty) is an explicit OFF upstream and a silent no-op here

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-config/src/env.rs:77-79`: `EnvVars::from_process` reads `telemetry: first_env(&["CYRUP_TELEMETRY", "PI_TELEMETRY"]).as_deref().map(truthy)`, and `first_env` at `:50-53` is `std::env::var(k).ok().filter(|v| !v.is_empty())` — an **empty value is filtered to `None`**, collapsing pi's tri-state (unset / set-empty / set-truthy) into two. `NetworkPolicy::resolve` (`cyrup-config/src/policy.rs:25-27`) then falls through to `s.enable_install_telemetry()` and telemetry stays **on**.

**upstream** — `pi/packages/coding-agent/src/core/telemetry.ts:8-12` `isInstallTelemetryEnabled` branches on `telemetryEnv !== undefined`, so an empty string takes the **env** branch and `isTruthyEnvFlag("")` returns false at `:3-5` — telemetry is force-disabled regardless of `settingsManager.getEnableInstallTelemetry()`.

**Impact** — `PI_TELEMETRY= cyrup …` is the ordinary shell idiom for neutralising an inherited variable, and it is the one form that **silently fails**: a wrapper script that empties the variable still ships install telemetry. The sibling flags are unaffected — `offline` and `skip_version_chk` use `is_some_and(truthy)`, which matches pi's plain truthiness test — so this is specific to the one tri-state field.

**Fix** — In `cyrup-config/src/env.rs`, read the telemetry variable **without** the `is_empty` filter: `std::env::var("CYRUP_TELEMETRY").or_else(|_| std::env::var("PI_TELEMETRY")).ok().map(|v| truthy(&v))`, so a set-but-empty value yields `Some(false)` and wins over settings at `policy.rs:25-27`, mirroring `telemetry.ts:8-12`. Do **not** change `first_env` itself — the other callers rely on the empty-filter and match pi there.

**Verify** — Three-case unit test on `NetworkPolicy::resolve` with `enable_install_telemetry = true` in settings: variable unset → telemetry on; variable set to `""` → telemetry **off**; variable set to `"1"` → on. Repeat for `PI_TELEMETRY` to prove the alias carries the same tri-state, and assert `offline`/`skip_version_chk` are unchanged.

## DRIFT-051 — `process.title`'s role suffix is never set, so RPC-mode, subagent-runner and intercom-broker children are all indistinguishable from an interactive session in `ps`

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

*(Filed 2026-08-12 in the repair pass, from the `packages/coding-agent/src/bun/` sweep this area had never run — the surface the completeness critique recorded as having **zero mentions anywhere in `docs/gap-analysis/`**. The rest of `bun/` is adjudicated N/A in `## Coverage → The `bun/` sweep`.)*

**cyrup** — `cyrup/crates/cyrup/src/main.rs:53-57` declines the whole of pi's process-identity block in a comment: "Process identity (Pi `cli.ts:12-13`: `process.title = APP_NAME` + `PI_CODING_AGENT=true`) is **NOT** replicated here: `process.title` has no std API, and `std::env::set_var` is `unsafe` under edition 2024 … The bin is `unsafe`-free by policy, so this cosmetic identity marker is gated as a hard-language limit". Nothing anywhere sets a process name: `grep -rnE 'proctitle|prctl|set_process_title|setprogname|PR_SET_NAME' crates/ --include='*.rs'` → 0. `crates/cyrup-tui/src/terminal_title.rs` sets the *terminal's* OSC title, a different surface. The two hidden re-exec subcommands — `crates/cyrup/src/subagent_runner_cmd.rs:12` (`__subagent-runner`) and `crates/cyrup/src/intercom_broker_cmd.rs` (`__intercom-broker`) — re-exec `current_exe()` and therefore inherit the bare `cyrup` name too.

**upstream** — `pi/packages/coding-agent/src/bun/cli.ts:5` @**both** tags: `process.title = APP_NAME;`, and the non-Bun entry `packages/coding-agent/src/cli.ts:12` does the same, so this is not a Bun-only concern. `packages/coding-agent/src/rpc-entry.ts:6` sets a **different** value: `` process.title = `${APP_NAME}-rpc`; ``. Documented behaviour since `packages/coding-agent/CHANGELOG.md:3249` ("CLI process title is now set to `pi` for easier process identification") and named as a rebrandable extension point at `CHANGELOG.md:1349`. All three files are byte-identical at `v0.83.0` and `v0.84.1`.

**Impact** — **The base title is already satisfied for cyrup by accident**, and saying so is the point of filing this narrowly: a Rust binary's `argv[0]` is already `cyrup`, whereas Node's is `node`, which is the entire reason pi needs the assignment. What is actually lost is the **role suffix**. pi advertises an RPC-mode process as `pi-rpc`, so an operator can `pkill pi-rpc`, spot a stuck RPC child in `ps`, or pick it out in Activity Monitor without touching a user's interactive session. In cyrup an `--mode rpc` process, a `__subagent-runner` child and an `__intercom-broker` child all appear as plain `cyrup`, so recovering from a hung background process means reading `ps -f` command lines and killing by PID. This compounds `DRIFT-049` / `SEAM-047`: the RPC process a supervisor cannot stop with SIGTERM is also the one it cannot identify.

**Fix** — Set the per-process **name** (not the environment) after mode resolution in `crates/cyrup/src/main.rs::run`: `prctl(PR_SET_NAME)` on Linux and the macOS equivalent — a two-platform `cfg` block or a small crate such as `proctitle`. Use `cyrup` for interactive, `cyrup-rpc` when `--mode rpc` resolves (mirroring `rpc-entry.ts:6`), and distinct names in `subagent_runner_cmd::dispatch` and `intercom_broker_cmd::dispatch`. **Correct the comment at `main.rs:53-57` in the same change:** its `unsafe`/edition-2024 rationale covers only the `std::env::set_var` half — process naming is a syscall against the current process, not a mutation of the shared environment, so it does not carry that hazard, and the comment currently reads as if the whole block were unimplementable. The `PI_CODING_AGENT` / `AI_AGENT` half of that same comment is **already filed** as `TOOL-031` (`04-cyrup-tools.md:416-430`) and `PARITY-GAPS` **PB-5** — do not re-file it here; PB-5 is where the "add to the per-child env vector rather than `unsafe set_var`" placement decision gets made.

**Verify** — `cyrup --mode rpc`, then `ps -o comm= -p <pid>` returns `cyrup-rpc`; an interactive session returns `cyrup`; a live `__subagent-runner` child returns its own name. All three return `cyrup` today.

---

## Coverage

### Repair pass 2026-08-12 — what was re-read, corrected and newly swept

**Re-derived two-sided at the ported tag, replacing commit hashes with file:line.** Nine items were
carrying upstream evidence that led with a commit hash inside a count where every other item is held
to a two-sided standard. Seven were settled this pass with `git cat-file -e <tag>:<path>`,
`git ls-tree <tag>`, `git grep <tag>` and `git show <tag>:<path>` — and **six of the seven were
misclassified**, because the code they cite already exists at **`v0.83.0`**:

| item | prior kind | corrected kind | the evidence that settled it |
|---|---|---|---|
| `DRIFT-016` | upstream-drift | **stale-port** | `git grep 'Current date' v0.83.0 -- packages/coding-agent/src` → nothing |
| `DRIFT-018` | upstream-drift | **not-ported** | `constrained-sampling.ts` @v0.83.0, 148 L, 7 exports, imported by 5 APIs |
| `DRIFT-019` | upstream-drift | **not-ported** (3 of 4) | `radius.ts`, `radius-config.ts`, `qwen-token-plan{,-cn}.ts` @v0.83.0 |
| `DRIFT-030` | upstream-drift | **not-ported** | `env-api-keys.ts:29`/`:76`/`:147` + `providers/anthropic.ts:5`/`:21` @v0.83.0 |
| `DRIFT-031` | upstream-drift | **not-ported** | `usage-totals.ts:37` @v0.83.0, consumed `interactive-mode.ts:5665` |
| `DRIFT-032` | upstream-drift | **not-ported** | `src/extensions/llama/{index,client,provider,ui,huggingface}.ts` @v0.83.0 |
| `DRIFT-035` | test-defect *(unchanged)* | — | footer absent from `packages/coding-agent/src` at **both** tags |

**That is a systematic, not incidental, result.** A commit hash tells you *when* something landed
upstream; it tells you nothing about whether it landed **before or after the tag cyrup was ported
from**, which is the only question that separates `upstream-drift` (expected lag, a rebase resolves
it) from `not-ported` (a port omission a rebase will never touch). Six of seven hash-led items were
on the wrong side of that line. **The countermeasure is one command per item** —
`git cat-file -e v0.83.0:<path>` — and it should run before any `upstream-drift` kind is assigned.
This is the same failure README structural blind spot 3 records for baselines, one level down.

The two that could not be settled cheaply — `DRIFT-023` and `DRIFT-040` — are now in
`## Leads — not yet evidenced`, outside the item count, with the exact commands that would settle
them. `DRIFT-019`'s cyrup half was independently re-derived by the completeness critic
(`grep -rn "radius|qwen" crates/cyrup-provider/src/providers/all.rs` is empty across the **whole**
file, not merely `:145-243`, so the `fleet!` grep trap does not rescue it); that is recorded in the
item so it is not re-litigated.

### The lesson `DRIFT-009` taught: what a sweep is allowed to skip

`DRIFT-009` asserted that pi has "**no in-tree regeneration source**" for its model catalogs and
proposed, on that basis, seeding cyrup's embedded floor from the pi.dev artifact as "the only source
still available". `packages/ai/scripts/generate-models.ts` — 2 733 lines — has been committed at
every tag in the window, together with `model-data.ts`, `models-dev-reasoning-options.ts`,
`check-model-data.ts`, `scripts/diff-model-catalog.mjs` (a purpose-built **catalog drift differ**)
and `scripts/publish-model-catalog.mjs`. Only the generator's **output**
(`packages/ai/src/providers/data/`) is gitignored, at `.gitignore:11`. `PROV-018` had the right Fix
the whole time; three documents disagreed and the wrong one produced a plan.

**The mechanism of the miss is in this file's own Coverage, and it is reusable.** The prior pass's
rejected-with-reason list dismisses "**`packages/evals` and root `scripts/`** — dev/release tooling
with no runtime behaviour". That is true of *runtime* behaviour and false of everything else: build
and release tooling is where a **generated artifact's provenance** lives, and the item this file was
writing was precisely about a generated artifact's provenance. The `.gitignore` line that hid the
data was read; the `package.json` scripts that produce it were not.

> **Rule for the next sweep: "no runtime effect" licenses skipping a directory's *behaviour*, never
> its *provenance*.** Before writing "there is no source for X", grep the root and per-package
> `package.json` `scripts` blocks for something that generates X, and settle presence with
> `git cat-file -e <tag>:<path>` rather than by inference from a `.gitignore` entry. A gitignored
> path is evidence that an artifact is generated — which implies a generator — not evidence that
> the generator is absent.

### Duplication census — this area is partly a duplicate index

A full cross-file ID census this pass: **20 of the 33 prior open IDs restate a defect an area file or
`PARITY-GAPS` entry already owns.** The ledger's cluster F4 lists 12 of them; two are recorded here
for the first time (`DRIFT-046` → `TOOL-036`, `DRIFT-009` → `PROV-018`) and six more were implicit in
individual items but not machine-readable. Every one now carries `duplicate-of:` in its row and in
its Kind line. **No ID was renumbered, merged or deleted** — the hard rule stands, and a closed or
duplicated ID must remain re-auditable.

| this ID | owned by | owning area |
|---|---|---|
| `DRIFT-009` | `PROV-018` (+ `PROV-004`, `PROV-039`, VL-P25) | 01 |
| `DRIFT-015` | `EXT-019` (+ `TUI-034`, VL-P21) — markdown half only | 06 |
| `DRIFT-016` | `SESS-019` | 03 |
| `DRIFT-018` | `PROV-011` (+ `TOOL-016`, `EXT-024`) | 01 |
| `DRIFT-019` | `PROV-014` (+ VL-P2 for the `-individual` variant) | 01 |
| `DRIFT-020` | `PROV-024` (+ `PROV-033`) | 01 |
| `DRIFT-022` | `SEAM-051` (+ `TUI-019`, `CFG-021`) | 08 |
| `DRIFT-023` | `CFG-020` | 05 |
| `DRIFT-024` | `SESS-013` | 03 |
| `DRIFT-025` | `CFG-017` | 05 |
| `DRIFT-027` | `PROV-025` | 01 |
| `DRIFT-030` | `PROV-021` | 01 |
| `DRIFT-031` | `PROV-036` | 01 |
| `DRIFT-032` | `EXT-027` | 06 |
| `DRIFT-035` | `SESS-019` | 03 |
| `DRIFT-039` | `AGENT-019` — *literally the same test* | 02 |
| `DRIFT-046` | `TOOL-036` — **new pairing** | 04 |
| `DRIFT-047` | `PARITY-GAPS` VL-P5 | cross-cutting |
| `DRIFT-049` | `SEAM-047` (+ `SEAM-008`, `SEAM-059`, PB-30) | 08 |
| `DRIFT-040` | `PARITY-GAPS` VL-P22 (harness half) | cross-cutting |

**The 14 IDs this area uniquely owns:** `DRIFT-004`, `DRIFT-010`, `DRIFT-013`, `DRIFT-014`,
`DRIFT-028`, `DRIFT-029`, `DRIFT-033`, `DRIFT-036`, `DRIFT-041`, `DRIFT-042`, `DRIFT-045`,
`DRIFT-048`, `DRIFT-050`, `DRIFT-051`. Those are the rows a planner should schedule from here; the
other twenty should be scheduled in their owning area, and this file read only for the extra evidence
several of them carry.

**A pattern worth stating, because it distorts the severity histogram.** This area consistently rates
its duplicates *lower* than the owning area does — `DRIFT-018` low vs `PROV-011` medium; `DRIFT-019`
low vs `PROV-014` medium; `DRIFT-020` low vs `PROV-024` medium; `DRIFT-024` low vs `SESS-013` medium;
`DRIFT-030` low vs `PROV-021` medium; `DRIFT-049` medium vs `SEAM-047` high. **The severity
distribution of this area is therefore an artifact of duplication, not an independent judgement**, and
"area 12 has no criticals and no highs" was partly a bookkeeping effect. Only `DRIFT-049` was re-rated
here, because it is the one where the mismatch changes scheduling — a high filed as a medium falls off
a planner's high list. The others are left at their filed severity with the owner's rating recorded in
the marker, so the recount can prefer the owner's number without this file inflating the medium count.

### The `bun/` sweep — `packages/coding-agent/src/bun/`, run for the first time

Three files, read in full at **both** tags (byte-identical at both). One item filed (`DRIFT-051`);
the rest is adjudicated, which closes the "zero mentions anywhere in the directory" hole.

- **`bun/cli.ts:5` `process.title = APP_NAME`** → **`DRIFT-051`**, together with
  `rpc-entry.ts:6`'s `` `${APP_NAME}-rpc` `` role suffix, which is the half that actually carries
  behaviour for cyrup.
- **`bun/cli.ts:13` `process.env.PI_CODING_AGENT = "true"`** (and `rpc-entry.ts:7`, plus
  `v0.84.1`'s `AI_AGENT` at `rpc-entry.ts:8`) → **already filed, not re-filed**: `TOOL-031`
  (`04-cyrup-tools.md:416-430`) and `PARITY-GAPS` **PB-5**, both of which already cite `cli.ts:13`
  and `rpc-entry.ts:7` and already note that `main.rs:53-57`'s `unsafe` rationale covers only the
  process-global-mutation half. The prior pass had already rejected a duplicate of this as
  `DRIFT-044`; that rejection stands.
- **`bun/cli.ts:2`,`:8` `registerBunOAuthFlows()`** → **covered.** It resolves to
  `packages/ai/src/bun-oauth.ts:11-21`, registering seven loaders; cyrup compiles all seven in
  statically as `crates/cyrup-provider/src/auth/oauth/{anthropic,openai_codex,github_copilot,openrouter,kimi_coding,xai,radius}.rs`,
  exported from `auth/oauth/mod.rs:60-61`. A 1:1 set with no bundler seam needed. *(The separate
  question of which flows are **wired** into `provider_auth()` is `PROV-029`, not this.)*
- **`bun/restore-sandbox-env.ts` (whole file)** → **N/A, mechanism.** A workaround for
  oven-sh/bun#27802, stated in its own header: a Bun-**compiled** binary inside a sandbox sees an
  empty `process.env`, so it re-reads `/proc/self/environ`. It self-disables on any non-Bun runtime
  (`if (!process.versions?.bun) return;`, `:20`). Rust's `std::env::vars()` reads the `environ` block
  the kernel handed the process, independent of any runtime's snapshotting — there is no defect to
  work around. Same adjudication for its deliberate duplicate,
  `packages/ai/src/utils/provider-env.ts:15-39 getBunSandboxEnvValue`; that file's portable half
  (`getProviderEnvValue`, `:45-52`) **is** covered by cyrup's `ProviderEnv` overlay + `AuthContext::env`
  precedence.
- **`bun/register-bedrock.ts` (whole file)** + `packages/ai/src/api/bedrock-converse-stream.lazy.ts:15-30`
  (`setBedrockProviderModule` / `bedrockModuleOverride`) → **N/A, mechanism.** An override seam that
  exists only because pi loads bedrock through a variable specifier a bundler cannot resolve, so the
  Bun single-file build must inject a statically imported module (stated verbatim at
  `bedrock-converse-stream.lazy.ts:17-21`). cyrup's `ApiRegistry`
  (`crates/cyrup-provider/src/api/mod.rs:80-118`) is already a lazy get-or-init factory registry and
  registers bedrock unconditionally at `api/mod.rs:152-155`; there is no second registration path to
  override, and `ApiRegistry::register_impl` (`:96-98`) already offers the same substitution
  capability to an embedder. **Recorded specifically because the ledger notes bedrock has had no
  read-against-upstream pass — this closes the registration-seam question, not the wire-format one,
  which remains open and belongs to area 01.**
- **`bun/cli.ts:6` `process.emitWarning = (() => {})`** (also `cli.ts:14`, `rpc-entry.ts:8`) →
  **N/A, mechanism.** Silences Node's deprecation / `ExperimentalWarning` writes to stderr, which
  would corrupt the alternate screen. A Rust binary emits no runtime warnings and has no
  `emitWarning` channel to override. *(The analogous hazard — a library writing to stderr under the
  TUI — is a different concern with different mechanics, and cyrup's answer to it is
  `output_guard`.)*
- **`bun/cli.ts:10`,`:14`,`:15` — the `await import(...)` ordering** → **N/A, mechanism.** It is
  staged so a bundler cannot statically follow the chain into the Node-only AWS SDK. Rust has no
  bundler and no module-evaluation order to stage; `cyrup-provider` links
  `bedrock_converse_stream` unconditionally.

### Un-run surfaces adjacent to this sweep, stated rather than implied

`packages/ai/src/utils/` was named by the completeness critique as a directory no file claimed to have
read. **It is not this area's** — `01-cyrup-core-and-provider.md` owns "wire APIs, providers,
streaming" — and the 2026-08-12 sweep routed it there. Recorded here only so the boundary is explicit
and the next auditor does not assume this file covered it: `sanitize-unicode.ts`, `node-http-proxy.ts`,
`event-stream.ts`, `abort-signals.ts`, `hash.ts`, `json-parse.ts` and `typebox-helpers.ts` are area
01's, and `provider-env.ts` is adjudicated above only for its Bun-sandbox half.

**Read first-hand at cyrup HEAD `a9000b1`** (working tree clean; last code commit `04c1ba2` — note this file was baselined at `1806375` and the ledger at `8d00f06`/`097bdde`, so cyrup has moved ~15 commits including the batch-1..10 parity/TUI series): `cyrup-core/src/message.rs`; `cyrup-provider/src/{api/{compat,openai_completions,openai_responses,anthropic_messages,google_generative_ai,pi_messages}.rs, utils/retry.rs, remote_catalog.rs, catalog.rs, providers/{all.rs,catalog/}, auth/{store.rs,oauth/}}` and `tests/catalog_data.rs`; `cyrup-session/src/{entry.rs, prompt/{builder,context_files,tests}.rs, compaction/summarize.rs}`; `cyrup-session-svc/src/{session.rs, hooks.rs, export.rs, services.rs, builder.rs, bash.rs}` and `tests/summarization_retry_events.rs`; `cyrup-agent/tests/agent_loop.rs`; `cyrup-config/src/{model.rs, env.rs, policy.rs, settings.rs}`; `cyrup-tools/src/{tools/{bash,grep,find}.rs, ops/local.rs}`; `cyrup-ext/{wit/world.wit, src/{wrapper.rs, facade.rs, host/services.rs}}` and `tests/wit_world_sync.rs`; `cyrup-ext-sdk/wit/world.wit`; `cyrup-modes/src/{rpc.rs, run.rs, json_event.rs}`; `cyrup-tui/src/{login_dialog.rs, status.rs, footer_data.rs, export.rs, app.rs, panic_hook.rs, image.rs, model_selector.rs, markdown/latex.rs}`; `cyrup/src/{main.rs, cli.rs, signals.rs, update_check.rs}`.

**Read first-hand at pi `v0.84.1`** (and diffed against `v0.83.0`): `core/{telemetry,diagnostics,footer-data-provider}.ts`; `core/export-html/{index,template.html,template.js,template.css,ansi-to-html,tool-renderer}.ts` and `vendor/`; `src/utils/*` with every file's export surface enumerated (`abort.ts`, `clipboard.ts`, `paths.ts`, `open-browser.ts`, `pi-user-agent.ts`, `deprecation.ts`, `shell.ts`, `tools-manager.ts`, `version-check.ts`, `management-http.ts` read in full or in diff); `packages/telemetry/src/{index,memory,noop,testing}`; `packages/agent/src/harness/telemetry.ts` and `harness/session/types.ts`; `packages/evals` (diffstat + new-file list); `scripts/`; cross-checks in `packages/ai/src/{types.ts, utils/retry.ts, utils/deferred-tools.ts, api/{google-shared,openai-completions,openai-responses,anthropic-messages,constrained-sampling,simple-options}.ts, providers/radius*.ts, env-api-keys.ts}`; `coding-agent/src/{cli.ts, rpc-entry.ts, cli/args.ts, cli/experimental/*, server/create-harness.ts, core/{model-resolver,session-manager,resource-loader,usage-totals,prompt-templates,remote-catalog-provider,compaction/compaction,agent-session,tools/bash,extensions/types}.ts, modes/{print-mode.ts, rpc/{rpc-mode,rpc-types}.ts, interactive/{interactive-mode.ts, components/{footer,login-dialog}.ts}}}`; `packages/coding-agent/test/paths.test.ts`; `packages/agent/test/agent-loop.test.ts`; `package.json` dependency closure at **both** tags.

**Systematic sweeps run (not item-driven).** (1) **Env-var surface** diffed across both tags over `packages/` + `scripts/` — 7 added (`AI_AGENT`, `BASETEN_API_KEY`, `CC`, `PI_EVAL_ARTIFACT_DIR`, `PI_TUI_WIN32_TOOLCHAIN`, `STY`, `ZELLIJ`), 4 removed (all belonging to the deleted legacy server or moved inside radius; none is a real removal for the port). Routing: `AI_AGENT` → `PB-5`; `BASETEN_API_KEY` → VL-P1 (area 01); `CC`/`STY`/`ZELLIJ` are multiplexer detection for `fc3554e16 reduce mouse tracking in terminal multiplexers` → area 07. (2) **Settings-key surface** diffed — exactly 3 added (`fullscreenScrollbar`, `mermaid`, `tuiMode`), all covered by VL-P19/VL-P20 → area 07. (3) **CLI-flag surface** diffed over `cli/args.ts` — exactly 1 added (`--tui-mode`) → VL-P19, area 07. (4) **New-file enumeration** over the full 627-file delta (241 additions), triaged file by file. **(5) Added in the repair pass: `packages/coding-agent/src/bun/`** — all three files read in full at both tags, every statement dispositioned (one item, `DRIFT-051`; the rest N/A or already-filed), written up under `### The bun/ sweep` above. **Note what sweeps 1–4 have in common: all four are *delta*-scoped, so none of them can see a pre-`v0.83.0` omission** — which is exactly the class the repair pass's re-derivations turned up six times. See blind spot 11.

**Inferred, not observed** — every severity and effort rating; DRIFT-041's "materially wrong document" claim is derived from reading both renderers, not from rendering an export; DRIFT-048's mid-session-switch scenario is derived from the two call sites and the widened `requiresToolCallId` predicate, not from a captured request; DRIFT-049's RPC-mode consequence is derived from the absence of any path from `signals.rs` to `run_rpc`'s stdin park, not from sending a signal.

**Handoffs — found here, landing in another area's crate, deliberately NOT duplicated as items here:** `packages/tui/src/{tui-alt-screen,tui-main-screen,layout,layout-node,latex}.ts` + `components/{stack,h-stack,v-stack,scroll-view,alt-screen-flash}.ts` → area 07 (VL-P19; `latex.ts` is already ported at `cyrup-tui/src/markdown/latex.rs`, verified, so only the alt-screen half is open). `components/mermaid.ts` + the new `grok-mermaid` dep → area 07 (VL-P20). `components/markdown-transform.ts` + `extensions/types.ts:1153/:1292` → area 06 (VL-P21, overlaps DRIFT-015). `agent/src/harness/session/jsonl/storage.ts` atomic writes + torn-tail truncation → area 03 (VL-P22). `ai/src/utils/abort.ts` + auth cancellation → area 01 (VL-P6). `coding-agent/src/utils/{tool-result-images,management-http,pi-manifest}.ts` → areas 04/01/05 (VL-P12/16/15). `cli/{auth-command,auth-check}.ts` → area 08 (VL-P14). `modes/json-event.ts` + `a4475344f make JSON streaming output linear` → area 08 (cyrup's `cyrup-modes/src/json_event.rs` already cites `v0.84.1` explicitly at `:52`, so it is current). `core/model-runtime.ts` credential serialization → area 05 (VL-P24). DRIFT-041/042/045 land in `cyrup-session-svc`/`cyrup-tui` and DRIFT-049 in `cyrup`/`cyrup-modes` — filed **here** because they came out of this area's sweep of `core/export-html`, `utils/open-browser.ts`, `utils/clipboard.ts` and the mode signal handlers, and no area file or `PARITY-GAPS` entry mentioned any of them.

**Rejected with reason (do not re-derive).**

- **Detached-child registry** (proposed `DRIFT-043`, medium) — **rejected on two independent grounds.** (1) Duplicate: `SEAM-S03 — No detached-child registry: setsid-detached bash children are not killed from any signal/teardown path` already exists at `08-cyrup-session-svc-and-modes.md:537-547`, same kind, severity, effort and upstream citations (`shell.ts:175-194`, `bash.ts:108`/`:142`). (2) The lead **mechanism claim is wrong**: `execute_bash` takes its token as `self.session_cancel.child_token()` (`session.rs:4480`) and `runtime.dispose()` ends in `self.session_cancel.cancel()` (`session.rs:2424`), so in interactive mode the first signal → `App::run` returns → `main.rs:575` disposes → the child token **is** cancelled → `cyrup-tools/src/ops/local.rs:506-511` `send_sigkill_tree` killpg's the group. Print/JSON dispose the same way. The panic path is not "kills nothing" either: `ops/local.rs:461` sets `cmd.kill_on_drop(true)`, so unwinding SIGKILLs the direct shell (grandchildren do survive — single-pid kill, not killpg). What actually leaks is narrower — the `std::process::exit` at `signals.rs:99` (destructors do not run) and **RPC mode, where nothing observes the signal at all** — and that is filed as **DRIFT-049**, not as a registry item.
- **`AI_AGENT` never stamped** (proposed `DRIFT-044`, low) — **rejected as a duplicate.** `PB-5 · PI_CODING_AGENT is never stamped into the environment (and AI_AGENT is the v0.84.1 half)` already exists at `PARITY-GAPS.md:63-66`, cites the same two upstream sites (`cli.ts:14`, `rpc-entry.ts:8`) and is strictly **broader** (it also covers `PI_CODING_AGENT`). The facts are right (`git grep AI_AGENT v0.83.0 -- packages/` → 0; `v0.84.1` `cli.ts:14`, `rpc-entry.ts:8`, `README.md:669`; `grep -rnE 'AI_AGENT' crates` → 0), but filing twice splits one small change across two backlogs — and the two entries gave **contradictory fixes** (PB-5: add to the per-child env vector to avoid `unsafe set_var`; the proposal: explicitly do **not** do that). **Resolve inside PB-5**, and decide the placement question there.
- **`packages/server` rewrite** (`05e89b418` + the new `server.ts`/`sessions.ts`/`protocol.ts`/`connection.ts`/`listener.ts`/`snapshots.ts` + `transports/unix/`) — `pi-server` is **not** in coding-agent's dependency closure at either tag (checked `package.json` both sides), and the experimental CLI's `runServer`/`runClient` are context interfaces with no implementation in coding-agent src (`cli/experimental/commands/server.ts:19-21, :44`). Nothing behavioural to port yet. `pi-protocol` + `pi-client` **did** enter the closure and are already filed as VL-P23.
- ~~**`packages/evals` and root `scripts/`** — dev/release tooling with no runtime behaviour~~ — **PARTLY RETRACTED 2026-08-12 (repair pass).** The `packages/evals` half stands, and the `816237c10 target baseline x64 CPUs` check stands (cyrup has no `.cargo/config.toml` and no `target-cpu` in `[profile.release]`, `Cargo.toml:219-224`, so the SIGILL hazard does not exist here). **The root `scripts/` half was wrong and it cost `DRIFT-009` its Fix.** `scripts/diff-model-catalog.mjs` is a purpose-built catalog **drift differ** and `scripts/publish-model-catalog.mjs` documents the published-artifact schema; together with `packages/ai/scripts/generate-models.ts` they are the regeneration source `DRIFT-009` declared did not exist. "No runtime behaviour" was true and irrelevant: the question the item was answering was about a generated artifact's **provenance**, which is exactly what build tooling holds. See `## Coverage → The lesson DRIFT-009 taught` for the rule this produced.
- **`ensureTool("fd"|"rg")`** (`tools-manager.ts`, 371 lines, zero cyrup hits) — genuinely N/A: cyrup's grep and find are in-process ports over the `ignore`/`grep`/`globset` crates with the delta documented in-file (`cyrup-tools/src/tools/grep.rs:1-2`, `find.rs:1-2`). There is no external binary to download.
- **`6ca423447` event-bus leak fix** — cyrup is structurally immune: `cyrup-ext/src/host/services.rs:1002-1008` dedupes `(owner, topic)` on subscribe and `:1031-1039` `clear()` drops all subs on hot-reload, so upstream's duplicate-handler-per-reload defect cannot occur.
- **`warnDeprecation`** (`utils/deprecation.ts`) — zero consumers upstream at `v0.84.1`; dead code.
- **`getFileRevision`-based revalidation of `auth.json`/`models.json`** — cyrup's auth store has no read cache at all (no mtime/revision/cache logic in `cyrup-provider/src/auth/store.rs`), so it re-reads every time; upstream's change is a lock-contention optimization with no correctness delta.
- **`checkForNewPiVersion` / `https://pi.dev/api/latest-version`** — deliberately skipped with a written rationale at `cyrup/src/update_check.rs:16-23` (no cyrup release feed exists to point at). **Flagged for a human rather than filed:** by the project's own rule a *disclosed* deferral is not an *approved* one, and "invent a cyrup release endpoint" is a product decision, not an engineering one.
- **`DRIFT-028` half (b)**, `~anthropic/*` cache-control aliases — refuted and dropped; pi misses those ids identically (see the item).
- **`DRIFT-037`** in full — refuted on both sides; see the status table.

**Method** — Read-only throughout: no cargo, no npm, nothing run, no source modified. Every `closed` verdict was re-derived by opening the Rust at HEAD **and** the TypeScript at the tag and comparing clause by clause — **zero overturns** among the thirteen closures. Closures were checked at the **consumer**, not the declaration, wherever a half-fix was possible: DRIFT-012 by sweeping all six `raw_stop_reason` producer sites against `git grep rawStopReason v0.84.1`, DRIFT-021 by following `supports_finish_reason` through to `openai_completions.rs:1547-1560`, DRIFT-034 by confirming the identity test actually ties both paths to the `package` line rather than merely diffing the copies against each other. Two `still-open` claims were **corrected on evidence** (DRIFT-010's method exists; DRIFT-038's helper exists), one on **kind** (DRIFT-014 is `not-ported`, not drift), one on **scope** (DRIFT-028 halved), and one on **severity** (DRIFT-042 medium → low). Note for future passes: several prior greps were written with a literal `|` and silently returned 0 — **use `grep -E`**.

**Blind spots — this is the list that stops the next pass wasting effort.**

1. **DRIFT-014's load-bearing half is still unresolved.** The seven missing literals are certain (re-derived line by line against `retry.ts:26-89` at `v0.84.1`), but what string reqwest/hyper actually emits on a failed DNS lookup is undetermined — `grep -rnE 'getaddrinfo|dns error|failed to lookup' crates --include='*.rs'` → 0 — and determining it needs a running process, which the no-cargo rule forbids. **Do not close DRIFT-014 on the literals alone.**
2. **The size-mismatch class is almost certainly not exhausted.** DRIFT-041 was found by *opening the directory*, not by grepping: `grep -rniE 'export-html|exportSessionToHtml|ToolHtmlRenderer' docs/gap-analysis/*.md` returns **zero** across all sixteen files — no prior pass ever had an item about session export. Any other pi subsystem that cyrup answered with a small hand-rolled stand-in is equally invisible to an item-driven **or** grep-driven pass, because the Rust symbol exists and the grep succeeds. **The counter is to compare SIZE and FEATURE SET of the two implementations, not presence of a name.** DRIFT-048 is the sibling shape: a correctly-ported helper called with the **wrong argument** also survives every absence sweep.
3. **Catalog *content* was not audited** (DRIFT-009). The count and the provenance claim were checked; per-model pricing/context-window fidelity was not. **Corrected 2026-08-12 (repair pass): the prior wording — "**cannot** be audited from this workspace at all" — is too strong and is the same overreach that produced DRIFT-009's wrong Fix.** pi does not *commit* `packages/ai/src/providers/data/*.json`, so it cannot be audited by **reading** — but `packages/ai/scripts/generate-models.ts` is committed at both tags and `npm run generate-models` reproduces the data, so it can be audited by **running** the generator. That is out of scope for a static pass and forbidden by this workflow's no-build rule; it is **not** out of reach for the engineer who takes `PROV-018`, whose whole Fix is that generator run. Restate the limitation as "not auditable statically", not "not auditable". `PARITY-GAPS` OQ-5 needs the same correction.
4. **`packages/session-backends`** (renamed from `packages/storage`, `+12598/−3479` in the delta) and the sqlite-node rebuild were **not read**. They stay out of scope under the existing no-counterpart rule (cyrup's store is JSONL), **but that rule was written before harness-v2 made `Storage` a swappable interface** — if the *interface*, rather than the sqlite implementation, becomes the thing pi's harness fixes are phrased against, the rule needs revisiting. Recorded under DRIFT-040 rather than filed.
5. **Windows coverage is weak on both sides of DRIFT-046.** cyrup carries only six `cfg(windows)`/`target_os = "windows"` sites, but whether Windows is a **declared target** for the port was not established. If it is not, DRIFT-046 and pi's other win32 work in this delta (`c96bfaccd` right-click paste, `73dd066ee` Shift+Enter, `4254e0d93`/`cc4aa1a2e` native rebuild sources, `fa07e7bd9` truecolor detection, `beeca6ab7` bunfig autoload, `windows-self-update.ts`) are all moot; if it is, that is a cluster nobody has sized.
6. **The test-defect hunt was NOT re-run.** The three known instances (DRIFT-035/036/039) were re-verified unchanged at HEAD, but no sweep for new ones was run — in particular the shape the prior pass flagged as uncovered (assertions on event **counts** or on **log output** that pin current-but-wrong behaviour) remains unswept, and the codebase grew by roughly fifteen commits of new tests since that sweep ran.
7. **DRIFT-049's blast radius is bounded by static reading only.** It was established that `session.abort()` does not reach `session_cancel` and that `run_rpc` has no signal observer, but whether tokio's runtime teardown incidentally SIGKILLs anything on the way out was **not** observed — it should not, since bash children are `setsid` session leaders (`cyrup-tools/src/ops/local.rs:270-278`), but that is an inference, not an observation.
8. **DRIFT-023 was not re-derived at all** this pass and its citations are inherited. **Still true after the repair pass** — it is now formally a `## Leads — not yet evidenced` row with the two commands that would settle it, alongside `DRIFT-040`. It remains the first thing to re-read next pass, since a tracking item with stale evidence is indistinguishable from a closed one nobody checked.

9. **NEW (repair pass) — a commit hash is not a tag-relative claim, and this area filed nine items on one.** Seven were re-derived and six proved misclassified: the upstream code already existed at `v0.83.0`, so what was filed as expected version lag was in fact a port omission a rebase will never resolve. The defect is in the *shape* of the evidence, not in any one item — a hash answers "when did this land upstream?" and the classification question is "did it land before or after the tag cyrup was ported from?". **Before assigning `upstream-drift`, run `git cat-file -e v0.83.0:<path>`.** Two items (`DRIFT-023`, `DRIFT-040`) are still hash-only and are now outside the item count rather than inside it at a lower standard.

10. **NEW (repair pass) — this area's severity histogram is not an independent measurement.** Twenty of thirty-three IDs duplicate an owning area's item, and this area rates six of them *lower* than the owner does. "Area 12 has no criticals and no highs" was therefore partly a bookkeeping effect, not a property of the surface. Only `DRIFT-049` was re-rated (medium → high, to match `SEAM-047` / PB-30). **Any future count of this area should be taken deduplicated**; the raw figure exists only so no ID is lost.

11. **NEW (repair pass) — the `bun/` sweep was run, but no equivalent surface sweep has ever been run over `packages/coding-agent/src/` as a whole from this area.** The 2026-08-12 pass ran four systematic sweeps (env vars, settings keys, CLI flags, new-file enumeration over the delta) — all four are **delta-scoped**, i.e. they can only find things that changed between `v0.83.0` and `v0.84.1`. Every one of the six misclassifications above was a *pre-delta* miss, invisible to all four by construction. The missing axis is a **baseline-scoped** surface sweep: enumerate the exported symbols of `packages/coding-agent/src/core/` and `packages/ai/src/` **at `v0.83.0`** and ask what consumes each in `crates/`. That is a large job and is the single highest-yield unswept axis this area has.

**Confirmed clean** — DRIFT-013's `max_completion_tokens` assertions are on a correct `openai` model and do not pin the bug; DRIFT-020 has zero assertions on `send_session_id_header`; DRIFT-026's closure is not pinned by any test asserting the old skip. DRIFT-035, DRIFT-036 and DRIFT-039 remain the only known test defects in this area — see blind spot 6 for why that is a floor, not a total.
