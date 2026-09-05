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
| DRIFT-004 | ~~**partially closed**~~ **CLOSED 2026-09-05** | `user_bash` from RPC, the `BashOperations` trait, the `BashOptions::operations` consumption half and — this pass — the guest tier (`register-bash-operations` + `bash-operations-exec` + `host-bash`, `cyrup:ext@0.10`) are all done. The 2026-08-15 note that all ten `BashOperations` hits were doc comments is long superseded. See the row and the detail section below. |
| DRIFT-005 | **closed** | Summarization isolation. `cyrup-session/src/compaction/summarize.rs:233-246` — `cache_retention: Some(CacheRetention::None)`, `session_id: Some(gen_session_id())`, routed through `retry_assistant_call`. Upstream `completeSummarization` at `pi/packages/coding-agent/src/core/compaction/compaction.ts:562-581` read at `v0.84.1`: clause-for-clause match. |
| DRIFT-006 | **closed** | Summarization retry observability. Four production `with_observer(retry_observer)` sites survive at `cyrup-session-svc/src/session.rs:1410, :1713, :2030, :4252` (line numbers moved from `1278/1547/1856/3722` — the file grew, the wiring is intact). Both halves re-verified: `cyrup-provider/src/utils/retry.rs:190-216` maps arm-for-arm onto `pi/packages/ai/src/utils/retry.ts:172-195` at `v0.84.1`, including the `if (lastRetry)` gate. |
| DRIFT-007 | **closed** | Runtime catalog overlay. `cyrup-provider/src/remote_catalog.rs:68` `https://pi.dev`, `:71` 4h interval, ETag lifecycle at `:520`/`:588`/`:614`/`:639`. Upstream `remote-catalog-provider.ts:6-7` at `v0.84.1`. Overlay only — does **not** close DRIFT-009's offline floor. |
| DRIFT-008 | **closed** | Thinking ladder. `cyrup-core/src/message.rs:30-39` `ThinkingLevel { Minimal, Low, Medium, High, Xhigh, Max }` with `Max` declared last, `:44-57` `ModelThinkingLevel`, round-trips `:59-90`, asserted `:1010-1012`. Upstream `pi/packages/ai/src/types.ts:82-83` — same ladder, same order, unchanged in the delta. *(Line cite corrected: the enum starts at `:30`, not `:38`.)* |
| DRIFT-009 | still-open · **duplicate-of: `PROV-018`** | Embedded catalog floor. The item's own count is now stale (31 → 35 shipped); upstream ships 39. **Rewritten this repair pass:** its "no in-tree regeneration source" claim is FALSE (`packages/ai/scripts/generate-models.ts` exists at both tags) and its pi.dev-seeding Fix was lossier than `PROV-018`'s. Now defers to `PROV-018` (tooling half) and `PROV-004`/`PROV-039` (provenance half); retains only the four-missing-catalog count, which is `PARITY-GAPS` VL-P25's data half. Restated below. **2026-09-05: the registration half of its four is CLOSED** (`ccd14981`) — `baseten` was the last unregistered pi built-in and is now a `Dynamic` fleet member like the three Qwen plans; what is left is the four catalog FILES (the offline floor), still models.dev-blocked. |
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
| DRIFT-039 | still-open · **duplicate-of: `AGENT-019`** | TEST DEFECT: parallel-tool test asserts wall-clock and completion order. All three assertions unchanged at HEAD. **Literally the same test** as `AGENT-019` (`02-…:677`) — `crates/cyrup-agent/src/tests/agent_loop.rs:327`. Fix it once; this item's body carries the better fix sketch (the `agent-loop.test.ts:589-612` rendezvous), so merge that into `AGENT-019` rather than working both. |
| DRIFT-040 | **TRACKER** (excluded from the severity count) · **LEAD — not yet evidenced** · **duplicate-of: `PARITY-GAPS` VL-P22** | pi harness-v2 rearchitecture. Absorbs DRIFT-037's residue (harness-v2 session-format interop). The three load-bearing claims — the `agent-harness.ts` rewrite (`+420/−996`), `docs/harness-v2.md` (`+2124/−367`) and the sqlite-node rebuild (`+12598/−3479`) — were **carried forward unverified** in the prior pass and are **still** unverified: this repair pass re-derived the six items whose *kind* was in doubt and did not spend the budget here. It proposes no work and says so, so it is a tracker in `## Leads — not yet evidenced`, outside the item count. |
| ~~DRIFT-041~~ | **CLOSED 2026-09-05** | Session HTML export is a 131-line text dump against pi's 5,021-line templated document. **Ported in full 2026-09-05** (batch-3, code `ce81dba9`): the base64 `SessionData` payload, the five byte-identical `v0.84.4` assets, the theme feed (first production consumer of `Theme::resolve_export`), and a `CssColor` parse boundary for the palette arithmetic. ~~One residual, low: `renderedTools` — which pi's own `exportFromFile` also omits.~~ **That residual list was WRONG and is corrected 2026-09-05 by the batch-3 ledger pass:** there is a second, larger one. `session_data()` emits only `{header, entries, leafId}`, so the exported document silently loses its **System Prompt** and **Available Tools** sections on all three live export paths, where pi always passes `this.state` (`agent-session.ts:3439` @v0.84.4). Filed as **`DRIFT-054`** (medium) — and **CLOSED 2026-09-05** by batch 4 (code `16b38c93`), which threads `AgentSession::export_state()` (the manager's leaf + `agent.state.systemPrompt` + `agent.state.tools`) through all three live export paths, exactly as pi's `exportSessionToHtml(sm, this.state, …)` does (`core/export-html/index.ts:263-270`, called from `agent-session.ts:3439`). `renderedTools` remains a genuine low residual. See the row and the detail section below. |
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

> ### AREA-12 PASS — 2026-08-15, cyrup HEAD `68bbd39` ("batch A — subagents and provider"), upstream re-measured
>
> **The tag table in this file's header is stale and is corrected here.** Re-measured with
> `git tag --sort=-v:refname` in each checkout on 2026-08-15: pi is at **`v0.84.2`** (not `v0.84.1`
> — `v0.84.2` adds `assertExactModelIds`, the DeepSeek `max_tokens` fix and case-insensitive
> DeepSeek base-URL detection to `packages/ai`), pi-subagents at **`v0.49.0`** (the header says
> `v0.47.1`), pi-permission-system at `v0.8.0`, pi-intercom at `v0.10.1`. Every citation added by
> this pass names `v0.84.2` and was re-derived, not carried.
>
> **Eight of the sixteen open rows were REFUTED at HEAD — the defect is gone.** Not one of them was
> closed by this pass: they were closed by the provider work in `68bbd39` and its predecessors,
> after this file was last reconciled. `DRIFT-014` (all seven retry literals present **plus** a
> documented `"dns error"` `[CYRUP-DELTA]` for hyper-util, which is the thing the row said could not
> be closed under the no-test-execution rule — it was closed by *reading the dependency's source*,
> not by running anything), `DRIFT-018` (`constrained_sampling` in `cyrup-core` **and**
> `cyrup-provider` with all four apply sites), `DRIFT-020` (`SessionAffinityFormat` three-valued,
> `send_session_id_header` deleted), `DRIFT-025` (`${@:-…}`/`${ARGUMENTS:-…}`), `DRIFT-027`
> (`DeferredToolsMode::Kimi` declared, resolved **and consumed** at two sites), `DRIFT-028` (the
> `"tool"` arm), `DRIFT-030` (`ANTHROPIC_AUTH_TOKEN` first, bearer header, display carve-out),
> `DRIFT-031` (`usage_cost_breakdown` in `cyrup-session-svc/src/state.rs` **and** rendered under
> pi's `len() > 1` gate at `cyrup-tui/src/app/execute_session.rs:184`). **The lesson for the next reconciliation is
> a scheduling one: this area is mostly a duplicate index, so its rows go stale from work done
> elsewhere, and a row here is not evidence of a live defect without a HEAD re-check.**
>
> **One item filed and closed: `DRIFT-052`** — the unrecorded `openAICompat` drift. See its row and
> body below. It is the **third** time this exact pair of Fireworks flags has moved (invented by an
> early sweep → removed by `PROV-061` as unsourced → reinstated here with pi `b9497c8c1` behind it),
> which is why the closure is carried by the generator's `DELTAS` table rather than by hand.
>
> **`DRIFT-015` takes a kind correction and a Fix correction, both from upstream.**
> `MessageRenderOptions { expanded; outputPad }` is declared at **`v0.83.0`**
> (`core/extensions/types.ts`), so the `outputPad` half is **`not-ported`**, not `upstream-drift` —
> the eleventh row in this file to take that correction. And its Fix ("a `render-options` record
> carrying `output-pad` and **width**") is wrong on both counts: upstream has no width field, and it
> has **three** distinct option records, not one — `MessageRenderOptions { expanded, outputPad }`
> for custom MESSAGE renderers (`custom-message.ts:73`), `EntryRenderOptions { expanded }` for
> custom ENTRY renderers (`custom-entry.ts:41`), and `ToolRenderResultOptions { expanded, isPartial }`
> for tool `renderResult` (`types.ts:411-416`, passed at `:494`). cyrup routes all three over ONE
> `render-call`/`render-result` pair, so threading options is a design decision about that
> collapse, not a record addition. Recorded so the next agent does not discover it mid-change.
>
> **Five rows are BLOCKED with measured sizes** (`DRIFT-004`, `DRIFT-009`, `DRIFT-015`, `DRIFT-019`,
> `DRIFT-041`, `DRIFT-047`) — see each row. The load-bearing measurement is `DRIFT-009`'s, because
> it also blocks `DRIFT-019`: **the four missing catalogs are not recoverable from git at any
> revision.** `xtask gen-catalogs` works by reading `packages/ai/src/providers/*.models.ts` at
> `b0c2a90e`, the last revision where those modules are data literals. `baseten`,
> `qwen-token-plan`, `qwen-token-plan-cn` and `qwen-token-plan-individual` were all introduced
> **after** it (`bbb91fa8a` 2026-07-20, `c1019d920` 2026-08-03, `c03d78bdc` 2026-08-06; `a9f6a3159`
> — the commit that gitignored `providers/data/` — is 2026-07-17), so every one of them has been a
> two-line re-export of a gitignored JSON file from its first commit. The generator's premise does
> not extend to them, and `DRIFT-009`'s own Fix ("the four missing catalogs must be in the
> generated set") is therefore not executable from a checkout.

> ### AREA-12 PASS — 2026-09-04, cyrup HEAD `2571969` (210 commits past this file's `4fb5e40`
> baseline), upstream re-measured at `v0.84.4`
>
> **One item closed, one filed, six re-verified unchanged.** `git log --oneline 4fb5e40..HEAD` over
> this area's eleven crates is 87 commits (112 since the 2026-08-15 pass's `68bbd39`); none of the
> intervening work (the core-loop CLTR_1..8 type refactor, the provider API module decompositions,
> the tools/grep/find parity sweep, the MCP port) touched the surfaces this file's six remaining
> `still-open` rows cite, and all six were re-read at HEAD and stand exactly as filed: `DRIFT-041`
> (`cyrup-session-svc/src/export.rs` is still 131 lines), `DRIFT-004` (`cyrup-modes/src/rpc/mod.rs:1111`
> still `operations: None`, `HOST_WORLD` still `cyrup:ext@0.8`, no `bash-operations` WIT member),
> `DRIFT-009`/`DRIFT-019` (`providers/all.rs:38-40,377` still self-documents `qwen-token-plan`,
> `qwen-token-plan-cn`, `radius` as `NOT REGISTERED`), `DRIFT-015` (`render-options`/`output-pad`
> still absent from `cyrup-ext/wit/world.wit`; `scoped-models`/`register-markdown-transformer` cited
> lines drifted to `:776`/`:517` from `:732`/`:476` — citation rot only, not re-fixed this pass),
> `DRIFT-047` (`TelemetryContext`/`telemetry_context` still zero hits in `crates/`). Two closures
> re-derived clause-for-clause and still hold after the refactors that touched their files —
> `DRIFT-001` (`cyrup-ext/src/wrapper.rs::additive_delta`, consumed at
> `cyrup-agent/src/agent/run/tools/finalized.rs:35` post-CLTR_4) and `DRIFT-048`
> (`cyrup-provider/src/api/google_generative_ai/convert.rs::assistant_parts` — the file split into a
> module directory since the 2026-08-15 pass, `include_id` threading is intact at the new paths).
>
> **`DRIFT-022` (TUI mode / alternate screen) is CLOSED.** Its own row already recorded the flag half
> as fixed via `SEAM-051`; the renderer half — what this tracker actually existed to track — landed
> in `dbcf59a` ("ship the fullscreen (alternate-screen) TUI mode, ADR-0005 B-1..B-14"), driven under
> a real pty per the commit's own transcript, not merely type-checked. `--tui-mode` → settings → the
> `Fullscreen` renderer is wired end to end (`crates/cyrup/src/interactive.rs:207-217`), and the
> `tuiMode`/`fullscreenScrollbar` settings keys this row's Fix asked for are live
> (`cyrup-config/src/settings/manager.rs:387,395`). Re-confirmed the upstream surface this row
> tracks is unchanged at `v0.84.4`. **Fine-grained fidelity against pi's 1378-line
> `tui-alt-screen.ts` was not diffed line-for-line — that residual belongs to `TUI-019` (area 07),
> which this closure explicitly does not speak for.**
>
> **One new item, `DRIFT-053`.** `packages/coding-agent/src/modes/interactive/session-share.ts` is
> new at `v0.84.3` (absent at `v0.84.1` and `v0.84.2` — confirmed with `git cat-file -e`) and
> restructures `/share` into a Radius-upload-first flow with the old gist flow demoted to fallback.
> cyrup's `/share` (`cyrup-tui/src/app/execute_misc.rs::share_session`) already cites the **new**
> file by line number for its `gh auth status` pre-check (`session-share.ts:59-68`) — meaning a prior
> pass read the restructured file — but ported only the fallback tail, not the Radius-first attempt
> that now precedes it. See the row and body below. **Blocked on `DRIFT-019`/`PROV-014`**: cyrup
> registers no `radius` provider at all, so even a direct port of `tryShareViaRadius` would have
> nothing to call `getProvider("radius")` against.
>
> **Not attempted this pass, and said so rather than left silent:** a full v0.84.2→v0.84.4 diff-stat
> skim of `packages/ai`, `packages/agent`, `packages/coding-agent`, `packages/tui` was run
> (248 files, +9822/−2179) and is far too large to read end-to-end inside one pass. Beyond
> `session-share.ts`, nothing else in that delta was verified confidently enough to file — in
> particular `packages/coding-agent/src/core/tools/powershell.ts` (new, 67 lines),
> `.../settings-submenu.ts` (new, 258 lines) and the `openai-completions.ts` / `bedrock-converse-
> stream.ts` / `github-copilot.ts` wire-API changes (+312/+155/+192 lines respectively) are
> **unread** and belong to areas 01/04/05/07 if anyone takes them, not filed here as leads to avoid
> inventing unverified rows. **DRIFT-023 and DRIFT-040 were left exactly as filed** (still
> `## Leads — not yet evidenced`) — neither side was re-read this pass; the two commands their rows
> already name are still the cheapest way to settle them, and this pass's budget went to the
> eleven-crate commit log and the six re-verifications instead.

| ID | Severity | Kind | Effort | Dedup | Title |
|---|---|---|---|---|---|
| ~~DRIFT-049~~ | ~~**high**~~ **CLOSED 2026-08-14** | **FIXED 2026-08-13** *(via `SEAM-047`)* | M | duplicate-of: `SEAM-047` | SIGTERM/SIGHUP never disposes the runtime, and in RPC mode is never observed at all — **CLOSED 2026-08-14**: closed pre-sweep (2026-08-13) via SEAM-047 — the row's kind cell already said so while its severity cell still read `high`, which is why two recounts carried a phantom high. Area 12 now has NO open high. |
| ~~DRIFT-041~~ | ~~medium~~ **CLOSED 2026-09-05** | not-ported | L | — | Session HTML export is a 131-line text dump against pi's 5,021-line templated document — **CLOSED 2026-09-05** (batch-3 pass; code `ce81dba9`). **The row's own "smallest honest slice" is exactly what landed, in one change**: the `SessionData` payload, the template assets, the theme feed — and the ANSI→HTML converter is the one piece deliberately NOT in it, for a reason the row could not have known (below). `crates/cyrup-session-svc/src/export.rs` is now `src/export/{mod.rs,color.rs}` + `src/export/assets/`. **(1) The document carries the TREE.** `session_data()` builds pi's `SessionData{header, entries, leafId}` (`core/export-html/index.ts:130-138`, `:263-270`, `:298-304` @v0.84.4) as raw `serde_json::Value`s — cyrup's on-disk entries are already pi-interoperable (`cyrup-session/src/entry.rs:1-3`), so `template.js` consumes them unmodified — base64-encoded into `<script id="session-data" type="application/json">` (`index.ts:160`, `template.html:42`). ~~`leafId` is the last non-`session` entry, which is both `_buildIndex`'s rule (`core/session-manager.ts:960-968`) and `SessionManager::leaf_id()`'s contract.~~ **WRONG in its second half — corrected 2026-09-05 at `824a539e`.** The last-entry rule is `_buildIndex`'s seeding rule (`core/session-manager.ts:959-977`) and therefore right for a FILE-only caller, which is all pi's `exportFromFile` has (`index.ts:288-305`). It is NOT `SessionManager::leaf_id()`'s contract: `SessionManager::branch` moves the leaf WITHOUT appending and `reset_leaf` clears it — as pi's own `branch`/`branchWithSummary` reassign `this.leafId` (`:1361-1365`, `:1393`) and `resetLeaf` nulls it (`:1373-1374`) — so after a `/tree` branch switch (or a resume onto an earlier branch) with no new message the last file entry belongs to the ABANDONED branch and `template.js:116-142` walked the wrong conversation. Fixed at `824a539e`: pi passes `sm.getLeafId()` into `generateHtml` (`index.ts:266`), and cyrup now threads the manager's leaf through the SHELL the way `AgentSession::export_theme()` is threaded — new `AgentSession::export_leaf_id()` (`session/transcript.rs`), a third `leaf_id: Option<&str>` parameter on `session_jsonl_to_html_with_theme`, with `None` retained as exactly pi's `exportFromFile` rule for `cyrup --export`. Tests: `export_html.rs::an_explicit_leaf_wins_over_the_last_file_entry` (red before by non-compilation), plus two source guards — `::export_to_html_passes_the_managers_leaf_to_the_renderer` and `cyrup-tui/src/tests/export.rs::both_tui_export_sites_pass_the_session_leaf` — each verified RED by replacing a call site's leaf argument with `None`. That single change is what restores tool-call **arguments**, tool-result `toolName`/`toolCallId`/`isError`/`details`, images, and the branch structure: the old `collect_text` harvested only values under a `"text"` key and ignored `parentId`/`leafId` entirely. **(2) Five assets, byte-identical.** `template.html` (55), `template.css` (1066), `template.js` (1864) and `vendor/{marked.min.js,highlight.min.js}` copied verbatim from `v0.84.4` under `src/export/assets/` and `include_str!`-ed, replacing pi's runtime `getExportTemplateDir()` (`index.ts:144-149`) because cyrup ships one binary with no sibling asset tree. `src/tests/export_html.rs::embedded_assets_are_byte_identical_to_pi_v0_84_4` pins each file's SHA-256, so a local "small fix" to `template.js` fails the suite instead of silently forking cyrup's export from upstream's; `assets/vendor/README.md` records provenance and both licences (marked 18.0.5 MIT, highlight.js 11.9.0 BSD-3-Clause), whose headers survive into every exported document. The nine placeholders are substituted in **upstream's order** with `replacen(_, _, 1)`, which is exactly `String.prototype.replace(string, string)`'s first-match-only rule (`index.ts:163-174`). **(3) The palette is the user's.** `ExportTheme::from_theme` ports `getResolvedThemeColors` + `getThemeExportColors` (`modes/interactive/theme/theme.ts:1064-1085`, `:1099-1125`) and `generateThemeVars`/`deriveExportColors` (`index.ts:81-128`, `:151-157`), including `withThemeColorFallbacks`' four aliases (`theme.ts:332-346`) and the `""`→`defaultText` rule keyed on the theme NAME, not its luminance (`theme.ts:1067`, `:1090-1093`). This gives `cyrup_resources::Theme::resolve_export` its **first production consumer** — `theme.rs:376-380`'s "the arch-12 HTML-export consumer is not yet in tree" note is now stale. `AgentSession::export_theme()` (`session/transcript.rs`) is the shell: live theme name off `ThemeAccess` (attached by the TUI, republished every frame, so a `/settings → theme` switch made a keystroke ago is carried) resolved against the session's discovered themes; `/export` and `/share` (`cyrup-tui/src/app/execute_session.rs`, `execute_misc.rs`) pass it too. **(4) DESIGN, recorded per the batch-3 rule** — one newtype and one FC/IS split, no more. `CssColor` (private `r`/`g`/`b`, `FromStr` = pi's `parseColor` grammar, `Display` = `#rrggbb`) is the parse boundary: upstream carries colours as bare `string`s and re-checks at every step, `adjustBrightness` opening `if (!parsed) return color` (`index.ts:74-75`) and handing an unparseable value straight back into the stylesheet. With the newtype that branch has nowhere to fire, and the ONE place absence is a real outcome — `deriveExportColors`' constant triple (`:82-89`) — is the single `Option` parameter of `derive_export_colors`. `session_jsonl_to_html_with_theme(&str, &ExportTheme) -> String` stays pure and total; theme DISCOVERY is the shell. Rejected: `String` + a `validate_color` helper (upstream's own dead re-check branch is the evidence that nothing then stops a later unvalidated string); a bare `(u8,u8,u8)` (no parse boundary, no `Display` contract); typestate (no invalid SEQUENCE exists — one call, no ordering); an outcome enum for the render (it is total — a corrupt line costs that entry, exactly as `parseSessionEntryLine` does, `session-manager.ts:503-511`). Migration cost was one line: `session_jsonl_to_html(&str)` keeps its signature so all four call sites compiled unchanged, `_with_theme` is additive, no serde on any new type. **Proof:** `cyrup-session-svc/src/tests/export_html.rs`, 12 tests, **7 measured RED** by splicing the previous 131-line renderer back in behind the new signature and re-running (`document_carries_the_template_and_both_vendored_libraries`, `embedded_session_data_reproduces_the_jsonl_exactly`, `tool_calls_keep_their_name_arguments_and_result_metadata`, `branching_history_is_exported_as_a_tree_with_a_leaf`, `palette_comes_from_the_active_theme_not_a_constant`, `transcript_text_cannot_break_out_of_the_script_element`, `malformed_and_empty_input_still_produce_a_document`) and all 12 green after; the other 5 pin the new type, the arithmetic and the asset digests, and pass either way by construction. `cyrup-tui/src/tests/export.rs` was rewritten off the old text-dump assertions (`class="entry user"`, `<title>My Session</title>`) onto the document + palette. `nextest -p cyrup-session-svc` 348/348, `-p cyrup-tui` 1411/1411. **THREE CYRUP-DELTAs**, each documented at its site: colours normalize to `#rrggbb` where pi round-trips the author's spelling (cyrup-resources has already decoded the document to a triple and the literal no longer exists); theme vars emit sorted rather than in document order (`ResolvedTheme::roles` is a `BTreeMap`, and CSS custom properties are order-independent); and the no-live-theme default is the compiled-in `dark` built-in where pi's `getDefaultTheme()` (`theme.ts:833-835`) probes the terminal background — this seam has no terminal (RPC mode, `cyrup --export`), and an interactive export carries the real theme anyway. **RESIDUAL — the list below said `renderedTools` ONLY, and that was FALSE; corrected 2026-09-05 by the batch-3 ledger pass, which re-read both sides.** ~~The payload is incomplete in a second, larger way~~ — **fixed 2026-09-05 at `16b38c93`; the description below is what was true between `ce81dba9` and that commit**: `session_data()` built `{header, entries, leafId}` and nothing supplied `systemPrompt` or `tools`, while the byte-identical `template.js` this item shipped renders a collapsible **System Prompt** block and an **Available Tools** list from exactly those two keys (`template.js:1405-1435`, destructured at `:15`). pi's `AgentSession.exportToHtml` ALWAYS passes `this.state` (`core/agent-session.ts:3439` @v0.84.4) into `exportSessionToHtml`, which sets `systemPrompt: state?.systemPrompt` and `tools: state?.tools?.map(...)` (`core/export-html/index.ts:263-269`) — and that is the only entry point `/export`, `/share` and RPC `export_html` have. The `exportFromFile` argument below does NOT cover it: `exportFromFile` is the path `cyrup --export` takes, not the path any front-end takes. Both inputs are already reachable at the seam — `/share` reads `session.current_system_prompt()` and `session.all_tools()` a few lines away in `cyrup-tui/src/app/execute_misc.rs` to build the `pi.share` entry — so this is a small fix, not an architectural one. **Filed as `DRIFT-054` (medium) — and CLOSED 2026-09-05 by batch 4 at `16b38c93`**, under the batch-4 rule that a defect in the CURRENT batch's own code is fixed rather than filed (`DRIFT-054` was this row's own output). The fix is `AgentSession::export_state()`: one `ExportState` carrying the manager's leaf, `agent.state.systemPrompt` (via `current_system_prompt()`, i.e. `override ?? base`) and `agent.state.tools` (the ACTIVE set, not `all_tools()`), passed by all three live paths — `export_to_html` (`session/transcript.rs`, the RPC path) and the TUI's `/export` and `/share`. `cyrup --export` keeps the file-only shape (`ExportState::from_file()` since `e8f93355`), which is pi's `exportFromFile` shape byte for byte. **Both in-source claims this row demanded be corrected were corrected in the same commit:** `session/transcript.rs`'s "Its one residual is `renderedTools`" now reads "the system prompt and the active tool list" among what the document carries, and `src/export/mod.rs`'s `SessionData` doc no longer excuses the absence as "what `JSON.stringify` produces for pi's own `exportFromFile`" — it now says which keys come from the transcript and which from the shell, and names pi's two call sites (`:263-270` vs `:298-304`) as the difference. **One citation in this row was ALSO wrong and is corrected here and in the source:** `exportSessionToHtml` is called from `agent-session.ts:3439`, not `:3438` (verified `git -C tmp/pi show v0.84.4:packages/coding-agent/src/core/agent-session.ts`). **The genuine low residual is `renderedTools`:** `ExportOptions.toolRenderer` / `preRenderCustomTools` (`index.ts:15-33`, `:177-230`), which pre-renders EXTENSION tool calls and results through their TUI renderers and converts the ANSI to HTML (`tool-renderer.ts` 172 lines, `ansi-to-html.ts` 258), is not ported. `template.js:1026` reads it as `renderedTools?.[call.id]` and falls back to its own rendering, so the document is complete for every built-in tool and degrades only for a custom-rendered extension tool. **CORRECTED at `e8f93355` (batch-4 review fix):** the clause that followed — that pi's OWN `exportFromFile` (`index.ts:288-316`) passes no renderer either, "so the document produced here is byte-shaped exactly like pi's file-mode export" — cited the FILE entry point to excuse a LIVE-path gap, the same false-comfort defence this row's `systemPrompt`/`tools` residual was corrected for. pi's LIVE path always builds a renderer (`agent-session.ts:3433-3437`, passed at `:3439-3443`, pre-rendered at `index.ts:254-261`, `:269`), and `/export`, `/share` and RPC `export_html` are all live paths. `cyrup --export`, the file path, does match upstream exactly. The row's "a partial port ships a document that looks MORE finished and is still wrong" applies to the payload/assets/theme trio, which landed together. **Two review notes recorded 2026-09-05 (`824a539e`), neither a defect:** `ExportTheme::role()` is `pub` with no production caller — a deliberate test accessor (the palette otherwise leaves the type only as CSS text), now said so at its doc; and `session_jsonl_to_html_with_theme` gained the `leaf_id` parameter described above, so its purity claim is now over three arguments, not two. — **2026-08-15, BLOCKED (measured), still open**: area-12 pass — re-measured at both ends and the sizes stand. cyrup `crates/cyrup-session-svc/src/export.rs` is **131 lines**, one file, five items. pi `packages/coding-agent/src/core/export-html/` @**v0.84.2** is **5,021 lines across 8 files** (`template.js` 1864, `vendor/highlight.min.js` 1212, `template.css` 1066, `index.ts` 316, `ansi-to-html.ts` 258, `tool-renderer.ts` 172, `vendor/marked.min.js` 78, `template.html` 55) — unchanged since v0.83.0, so it is owed debt and not lag. **Not attempted:** the smallest honest slice still lands a `SessionData` payload, four template assets, a theme feed and an ANSI→HTML converter together, because a partial port ships a document that looks MORE finished and is still wrong. Needs its own agent. Sweep 2 — not started. Effort L: pi's `core/export-html/` is 5,021 lines across eight files including vendored `marked.min.js` and `highlight.min.js`, and the port needs a base64 `SessionData` payload, four template assets, a theme-colour feed and an ANSI-to-HTML converter. |
| ~~DRIFT-048~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | — | Google converter picks the tool-call-id rule off the SOURCE message's model, not the target model — **CLOSED 2026-08-14**: sweep 2 — fixed exactly as the Fix specified: `include_id`, already computed from the TARGET model at `convert_messages:612` and already used for `functionResponse`, is now threaded into `assistant_parts`, which had been gating `functionCall.id` on `requires_tool_call_id(am.model)` — the model that PRODUCED the historical turn. On a mid-session `gemini-2.5-pro` → `gemini-3-pro` switch the two halves of a call/response pair therefore disagreed, silently. Verify implemented verbatim including the reverse-switch and both same-model control cases. **The item's observation that this is "a name-present, argument-wrong bug, invisible to an absence sweep" is worth promoting into the Coverage section as a recurring class — it is the second such finding in this file after DRIFT-026, in the same function.** |
| ~~DRIFT-004~~ | ~~medium~~ **CLOSED 2026-09-05** | upstream-drift | M | — | RPC `bash`: `UserBashEventResult.operations` backend seam unported — **CLOSED 2026-09-05** (batch-3 pass; code `8b688401`): the remainder this row measured — the WIT round-trip, FOUR world members plus a `HOST_WORLD` minor bump — is ported exactly as costed, and the costing was accurate. `registration.register-bash-operations()` declares that a guest HAS a backend; the keyed `events.bash-operations-exec(call-id, command, cwd, opts-json) -> result<option<s32>, string>` EXPORT runs one command through it; and pi's two closure-shaped `exec` options come back over the new `host-bash` interface — `emit-bash-output(call-id, chunk)` for `onData` (`core/tools/bash.ts:75` @v0.84.4) and `is-bash-cancelled(call-id)` for `signal?: AbortSignal` (`:76`), the interface itself at `:63-81` and `UserBashEventResult.operations` at `core/extensions/types.ts:1139`. `HOST_WORLD` moved **0.9 -> 0.10** with its history entry (`cyrup-ext/src/manifest.rs`); the row predicted 0.8 -> 0.9, which EXT-006's renderer batch had already consumed. **Review fix 2026-09-05 (`824a539e`): the bump's DOCUMENTATION half was missing** — this bump and EXT-006's left `docs/guide/extensions/authoring.md:104`/`:125-126`, `docs/guide/extensions/overview.md:27` and `README.md:68` telling authors to declare `cyrup:ext@0.8`, a world `check_world` REFUSES (`gmin >= hmin`). All are corrected and `cyrup-ext/src/tests/wit_world_sync.rs::the_authoring_docs_declare_the_current_host_world` now ties those three documents to `HOST_WORLD`. **Host:** `ExtensionHost::user_bash_operations` gained a second tier returning `cyrup_ext::host::GuestBashOperations`, an implementation of `cyrup_tools::ops::BashOperations` that forwards each `exec` to the owning guest's export; BOTH halves are required (declared in the registry AND live), so a declaration with no instance still falls through to `createLocalBashOperations` (`core/agent-session.ts:2993`'s `??` — **citation corrected 2026-09-05 from review**: the expression is at `:2993` at `v0.84.4`, not `:2782`, which is unrelated code. The same `:2782` is carried in ten code comments across `cyrup-ext`, `cyrup-ext-sdk`, `cyrup-tools`, `cyrup-session-svc` and `cyrup-it`; left unfixed in this review pass rather than pull five more crates through the check matrix under a 97%-full disk, and recorded here as the one place to sweep from). Output is replayed into `on_data` after the call settles and the cancelled arm replays too, the rule (and the `call-id` keying, and the drop guard) `execute_tool` already follows for `onUpdate` — EXT-M06. **Guest half landed in the same change**, as the row required: `cyrup-ext-sdk/src/{api,guest,macros}.rs` plus the new `ctx/bash_call.rs` (`BashCommand::{write,is_cancelled}`), and the bundled demo declares a backend, so every Tier-1 guest still builds (`cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2` clean). **Proof:** `cyrup-it/tests/ext/wasm_bash_operations.rs` drives a REAL component — the host resolves the guest backend, its output arrives over `emit-bash-output`, a guest `Err` stays pi's `throw` (`core/bash-executor.ts:154`) and an aborted command is `ExitStatus::Killed`; `cyrup-ext/src/tests/bash_operations_seam.rs` pins the registry table, the keyed back-channel and the two-halves rule; `cyrup-ext-sdk/src/tests/ergonomic.rs` pins the guest API and the `opts-json` decode. **RESIDUAL (new, low, recorded in the CYRUP-DELTA register at `crates/cyrup-ext/src/lib.rs`):** a guest backend runs under the wasm EPOCH budget (its dispatch budget plus the caller's declared `timeout`), where pi's backends are plain JS objects with none — and the `user_bash` seam declares no timeout (`cyrup-session-svc/src/bash.rs`'s `timeout: None`, itself a port of `bash-executor.ts:108-111`, which passes only `onData` and `signal`), so a guest command has the ordinary ~5s dispatch budget unless a caller supplies one. ~~Prior note, 2026-08-15 (BLOCKED, measured):~~ area-12 pass — **the trait and the consumption half both LANDED since this row was written** and the row understates the progress: `cyrup_tools::ops::BashOperations` exists (`ops/mod.rs:462`) with `LocalBashOperations` (`:489`), `BashOptions::operations` exists (`cyrup-session-svc/src/bash.rs:85`), `run_bash` takes it (`:127`) and `execute_bash` resolves pi's `options?.operations ?? createLocalBashOperations({ shellPath })`, pinned by three `..._operations_override_...` tests in `cyrup-session-svc/src/tests/round9_l5res.rs`. **What is left is exactly one thing and it is a WIT round-trip, not a Rust field:** a WASM guest cannot RETURN a callable (ADR-0002), so `cyrup-modes/src/rpc.rs:1367`'s `operations: None` is upstream's ABSENT `operations`, not a dropped one. The remaining design is written out in full in the CYRUP-DELTA register at `crates/cyrup-ext/src/lib.rs:76-121` and is FOUR world members plus a `HOST_WORLD` minor bump (0.8 → 0.9): a `registration.register-bash-operations()` import, a keyed `bash-operations-exec(call-id, command, cwd, env-json)` EXPORT, an `emit-bash-output(call-id, chunk)` import for pi's `onData` streaming, and an `is-bash-cancelled(call-id)` poll for its `AbortSignal` — plus the guest half in `cyrup-ext-sdk/src/{api,guest,macros}.rs` **in the same change or every Tier-1 guest fails to build**. That is a wasm-boundary streaming+cancellation protocol, not an M; it belongs with `SEAM-015` and `DRIFT-015` in one extension-surface agent. Prior note (still true): **2026-08-14**: sweep 1 + 2 — kept open with the mechanism correction recorded under SEAM-015: the seam is a WIT capability, not a `BashOptions` field. Sweep 2 re-confirmed rather than overrode that reasoning — the trait would live in `crates/cyrup-tools/src/ops/`, but its two consumers (`cyrup-session-svc/src/bash.rs`'s per-call override and `cyrup-modes/src/rpc.rs`'s population from the user-bash event result) are in other crates, so landing the trait alone produces a seam with no caller. |
| DRIFT-009 | medium | upstream-drift | M | duplicate-of: `PROV-018` | Embedded catalog floor is 4 catalogs short *(regeneration-source half struck — it was false)* — **2026-08-15, BLOCKED (measured, and the block is a DATA block, not an effort one)**: area-12 pass. `PROV-018`'s tooling half LANDED — `cargo run -p xtask -- gen-catalogs --check` reports *"all 36 files match pi@b0c2a90e"* and `xtask/src/main.rs` carries the `DELTAS` table this item's Fix wanted. **But the four catalogs it asks for cannot be produced by it.** The generator reads `packages/ai/src/providers/<p>.models.ts` at `b0c2a90e`, the last revision where those modules are data literals (`a9f6a3159`, 2026-07-17, gitignored `providers/data/`). All four providers postdate it — `qwen-token-plan` + `qwen-token-plan-cn` `bbb91fa8a` (2026-07-20), `baseten` `c1019d920` (2026-08-03), `qwen-token-plan-individual` `c03d78bdc` (2026-08-06) — so `git show <any-tag>:packages/ai/src/providers/qwen-token-plan.models.ts` is a two-line re-export of `./data/qwen-token-plan.json`, which is in git at **no** revision. The model IDs are recoverable from the `.models.ts` type literal at `bbb91fa8a`; the prices, context windows, `maxTokens`, thinking-level maps and compat blocks are not. **What it needs:** either a `node`/`npm install` toolchain to run pi's `generate-models` (which this workspace does not have and which `PROV-060` deliberately designed the extractor to avoid), or an explicit owner decision to seed from the pi.dev artifact — which this item's own rewrite forbids in bold. Escalate, do not re-attempt. — **2026-09-04, PARTIAL: contribution (2) CLOSED, contribution (1) converted from prose into a check** (`9e2bfda5`, `xtask/src/main.rs`). The Fix's second contribution — *"`scripts/diff-model-catalog.mjs` is the model for the drift check — port its comparison, do not invent one"* — is now ported: `canonicalize` is `canonicalizeJson`/`sortJsonKeys` (`v0.84.4 scripts/diff-model-catalog.mjs:96-114`, thinking-ladder key order at `:93`, array entries canonicalized with no parent key at `:106` — all three RE-DERIVED and confirmed EXACT by the 2026-09-05 ledger pass, which refutes the batch-3 second review's claim that they were each off by one; `sortJsonKeys` opens at `:96` and `canonicalizeJson` closes at `:114`) and `diff_catalog` is the single walk over the sorted union of both sides' ids (`:191-195`); `--diff` no longer calls a reordered `compat` block a data change. The first contribution stays data-blocked — **re-measured at `v0.84.4` and unchanged** — but is no longer a sentence: `gen-catalogs --roster <rev>` now audits the provider SET (`--roster v0.84.4` → *"39 provider module(s) upstream — 34 embedded, 5 accounted for as unported"*), fails when pi ships a module neither `CATALOGS` nor the new `UNPORTED` table names, and **fails the day the block lifts** by re-parsing each blocked module at that revision. Two refinements to the measurement: the block is per-FIELD (everything `generate-models.ts` hardcodes for these four is in git — `baseUrl`, `compat`, `thinkingFormat`, the ladders, the Individual allowlist at `:290-334`, `:1259-1330`, `:2303-2380` — only the models.dev half, `cost`/`contextWindow`/`maxTokens`/`name`/`reasoning`/modalities, is missing, `:2334`/`:1944`), and models.dev itself is unreachable from this workspace (`curl https://models.dev/api.json` → `CONNECT tunnel failed, response 403`), so the second of the two named paths is closed too. Severity and the escalation stand. — **2026-09-05, PARTIAL (batch-4), code `e4078e3a` + `ccd14981`: the registration half of contribution (1) is CLOSED and one of the block's two escalation paths is REFUTED as stated.** The row's Fix says the four catalogs must land *"in the same change as DRIFT-019 / `PROV-014`'s registrations"*. `PROV-014` landed three of the four on 2026-09-04 as `FleetCatalog::Dynamic` members and explicitly left the fourth out (*"Separately noted, NOT this item: `baseten` (`all.ts:95` @v0.84.4) is the one v0.84.x built-in still unregistered — it is in `all.rs`'s not-yet guard with no ledger id"*). It has an id now — this one — and it is registered: `providers/fleet.rs::BASETEN`, field for field against `ai/src/providers/baseten.ts:6-14` @v0.84.4 (`all.ts:95`, `env-api-keys.ts:106`, `baseUrl` from `generate-models.ts:1259`), `Dynamic` on the same evidence as its siblings, ordered between `ant-ling` and `cerebras` as `all.ts:92,95,96` has it. **It could not have been registered alone**: Baseten's TOGGLE-reasoning rows carry `thinkingFormat: "baseten"` (`toggleReasoningCompat` / `toggleReasoningEffortCompat`, `generate-models.ts:1274-1283`, selected at `:1310-1319`), a format cyrup did not have, so `ModelCompat` deserialization failed those rows entire. **CORRECTED 2026-09-05 (batch-4 review):** an earlier revision of this row said it was the only format Baseten's rows carry and that a registered baseten "would have offered zero models". That is false — `processBasetenModels` picks one of FOUR blocks per row, and an effort-only row takes `reasoningEffortCompat` (`thinkingFormat: "openai"`, `:1269-1273`) while a non-reasoning row takes `baseCompat` (no format, `:1260-1268`). The port is still necessary and still had to land in the same slice; what it prevents is the provider silently offering only its effort-only / non-reasoning subset, not offering nothing. So `ThinkingFormat::Baseten` (`types.ts:584`,`:578`), `ModelCompat::chatTemplateArgs` (`types.ts:593-594`, detected `{}` at `openai-completions.ts:1649`, resolved `:1695`) and the wire branch (`:888-904` — two independent halves, and the effort half has **no** `options.reasoningEffort` guard, falling back to `thinkingLevelMap.off` at `:899`) are ported in the same slice, with `buildChatTemplateValues` (`:1010-1026`) taking the map rather than the compat struct as upstream does. **cyrup now registers all forty of pi v0.84.4's built-ins**, and the hand-kept NOT-YET array is now EMPTY, backed by `all.rs::all_of_pis_v0_84_4_builtins_are_registered`, a set comparison against the whole of `all.ts:91-130`. **CORRECTED 2026-09-05 (batch-4 review):** the first draft of this sentence said that guard "fails in BOTH directions (a provider pi adds, a provider only cyrup has)" and the source comment beside it said an upstream addition fails there "whether or not anyone remembers this array". Both overstate it. `PI_BUILTINS` is a hand transcription of the PINNED target, so it catches a provider cyrup DROPS and a provider cyrup INVENTS against v0.84.4 — the second is genuinely the direction `PROV-062`'s prose table could not see — but a provider pi adds AFTER v0.84.4 is in neither list and the comparison stays green. It is as hand-kept as the array it sits beside and must be re-transcribed when ADR-0006 moves the target; `gen-catalogs --roster <rev>` is the check that reads upstream live. The two guards also contradicted each other as first written (`NOT_YET` asserts an id is absent, `PI_BUILTINS` asserted every id present), making the parking escape unusable; `NOT_YET` is now hoisted to module scope and subtracted from the expected set, so they compose. **REFUTED, and it matters because it was an escalation premise:** this row's *"what it needs: either a `node`/`npm install` toolchain … which this workspace does not have"* is FALSE at HEAD — `node v22.22.2`, `npm 10.9.7` and `bun 1.3.11` are installed (`/opt/node22/bin`, `/root/.bun/bin`). It changes nothing, and that is the point: `generate-models.ts:1431` fetches `https://models.dev/api.json`, which is an egress-POLICY denial here (`CONNECT tunnel failed, response 403` — re-measured 2026-09-05), so the block is the network one alone and no toolchain install lifts it. **What remains, and it is now only this:** the four catalog FILES — i.e. the embedded FLOOR. All four providers are live; what they lack is rows for an `--offline` run and for the window before the pi.dev overlay's first fetch. Re-measured unchanged at `v0.84.4`: `gen-catalogs --roster v0.84.4` → *"39 provider module(s) upstream — 34 embedded, 5 accounted for as unported"*, exit 0, and it still fails the day any of the four parses as a data literal. Severity holds at medium on the data half; the registration half is done. |
| ~~DRIFT-013~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | — | Z.AI sent `max_completion_tokens`, which it ignores — **CLOSED 2026-08-14**: sweep 2 — the missing `\|\| is_zai` term added to `use_max_tokens` in `api/compat.rs`, so Z.AI models resolve `max_tokens_field == MaxTokens` instead of the `max_completion_tokens` Z.AI ignores. **KIND CORRECTED: filed `upstream-drift` citing openai-completions.ts:1478-1485 @v0.84.1, but `git show v0.83.0:packages/ai/src/api/openai-completions.ts` has the identical disjunction at :1427-1435 with `isZai` at :1435 — INSIDE the ported baseline. It is `not-ported`, and the item's implicit "a rebase will sweep this up" framing was wrong.** Same class as the DRIFT-014/018/019/030/031/032 kind corrections the repair pass already made; this one was missed. |
| ~~DRIFT-014~~ | ~~medium~~ **REFUTED AT HEAD 2026-08-15** | **not-ported** | M | — | DNS/transport failures not classified retryable — **REFUTED 2026-08-15** (area-12 pass; closed by earlier provider work, not by this pass): all seven literals are present in `RETRYABLE_PROVIDER_PATTERNS` at `cyrup-provider/src/utils/retry.rs` in pi's own positions — `"524"`, `"getaddrinfo"`, `"ENOTFOUND"`, `"EAI_AGAIN"`, `"socket connection was closed"`, `"stream ended before a terminal response event"`, `"ResourceExhausted"`. **And the part this row said could not be closed IS closed, by a method the row did not consider.** The row warned that adding the seven Node-shaped literals alone would ship a test that passes while the real Rust failure stays unretried, and that settling it needed an observation the no-test-execution rule forbids. The fix instead READ THE DEPENDENCY: an eighth literal `"dns error"` is added and marked `[CYRUP-DELTA]` in-file, cited to `ConnectError::dns` at `hyper-util-0.1.20/src/client/legacy/connect/http.rs:668`, with the reasoning that only the hyper-util `&'static str` is stable across platforms while the trailing `std::io::Error` text is not (`"failed to lookup address information"` on macOS vs `"Name or service not known"` on glibc). **Generalise this: a claim about what a Rust dependency emits is a source-reading question, not a test-execution question.** Superseded prior note — **2026-08-14, still open**: sweep 2 — **keep open, and STRENGTHEN the warning rather than the item: this is the one backlog row whose Verify cannot be satisfied under the standing no-test-execution rule**, because it requires observing what string reqwest/hyper actually emits on a failed DNS lookup. Adding pi's seven Node-shaped literals (`getaddrinfo`, `ENOTFOUND`) alone would produce a test that passes while the real Rust failure mode stays unretried — exactly the false close the item's own Verify warns against. |
| DRIFT-015 | medium | **not-ported** *(was upstream-drift)* | L | duplicate-of: `EXT-019` | Extension context surface: ~~`scopedModels`~~, `outputPad`, ~~markdown transformers~~ still absent — **2026-08-15, TWO OF THREE HALVES CLOSED; the third is BLOCKED (measured)**: area-12 pass. `scoped-models: func() -> string` is declared at `crates/cyrup-ext/wit/world.wit:732` (EXT-045) and `register-markdown-transformer: func()` at `:476` with its `transform-markdown` export — both landed after this row was written, and the world is now `@0.8` (`HOST_WORLD` in `cyrup-ext/src/manifest.rs:219`), not the `@0.4` this row's Fix names. **KIND CORRECTED:** `MessageRenderOptions { expanded; outputPad }` is declared at **v0.83.0** `core/extensions/types.ts`, so the remaining half is `not-ported`, not `upstream-drift` — the eleventh row in this file to take that correction. **FIX CORRECTED, and this is why it is blocked rather than small:** the Fix asks for "a `render-options` record carrying `output-pad` and **width**", but upstream has no width field and has **three** option records, not one — `MessageRenderOptions { expanded, outputPad }` (custom messages, `custom-message.ts:73`), `EntryRenderOptions { expanded }` (custom entries, `custom-entry.ts:41`) and `ToolRenderResultOptions { expanded, isPartial }` (tool results, `types.ts:411-416`, passed at `:494`). cyrup collapses all three onto ONE `render-call`/`render-result` pair (`world.wit:259`/`:260`), so the work is a decision about that collapse plus a `HOST_WORLD` minor bump and a synchronized edit across `cyrup-ext/wit`, `cyrup-ext-sdk/wit`, `host/live.rs::render`, `facade.rs`, `cyrup-ext-sdk/src/{api,guest}.rs`, the four `MessageRenderer` impls in `example.rs`, `cyrup-it/tests/ext/ergonomic.rs:132` and the `cyrup-tui` caller. `wit_world_sync.rs` fails if only one WIT copy moves. Schedule with `DRIFT-004` — same world, same bump, one agent. |
| ~~DRIFT-028~~ | ~~medium~~ **REFUTED AT HEAD 2026-08-15** | **not-ported** *(was upstream-drift)* | S | — | OpenRouter Anthropic cache breakpoint skips tool results — **REFUTED 2026-08-15** (area-12 pass; closed by earlier provider work): `add_cache_control_to_last_conversation_message` at `cyrup-provider/src/api/openai_completions.rs:718-728` now accepts `role == Some("user") \|\| role == Some("assistant") \|\| role == Some("tool")`, with a guard test at `:3435` asserting the trailing tool result carries the breakpoint. Its doc comment records the kind correction this row never took: `git show v0.83.0:packages/ai/src/api/openai-completions.ts` already has `message.role === "tool"` at `:918` and `:947`, so it was `not-ported`, not lag. Superseded prior note — **2026-08-14, still open**: sweep 2 — half (a) (OpenRouter Anthropic cache breakpoint skipping tool results, in `openai_completions.rs`) is inside cyrup-provider and untouched; half (b) was already refuted upstream-side by the tracker. |
| ~~DRIFT-029~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | M | — | Concurrent user bash: single cancel slot makes abort miss and `is_bash_running` lie — **CLOSED 2026-08-14**: sweep 2 — pi's `_bashAbortControllers` SET ported. `AgentSession.bash_cancel: Mutex<Option<CancelToken>>` became `bash_cancels: Mutex<Vec<(u64, CancelToken)>>` plus a `next_bash_cancel_id: AtomicU64` standing in for the `AbortController` OBJECT IDENTITY pi's Set keys on — **not `options.id`, which is optional and may repeat, and which the item's proposed `HashMap<BashId, CancelToken>` would have keyed on**. `execute_bash` pushes one entry per call and installs a `BashCancelGuard` whose `Drop` is pi's `finally`; `abort_bash` cancels a snapshot of the whole set (pi's spread copy); `is_bash_running` is `!is_empty()`; the unconditional clears at the shell-resolution error path and the completion path are deleted. **KIND CORRECTED: filed `upstream-drift` against pi commit `2efa728d`, but `git grep -n _bashAbortController v0.83.0` shows the Set, its add, its delete-in-finally, the spread-copy abort and the `size > 0` getter all present at the ported tag — this was a port omission, not version lag.** |
| ~~DRIFT-030~~ | ~~medium~~ **REFUTED AT HEAD 2026-08-15** | **not-ported** | S | duplicate-of: `PROV-021` | `ANTHROPIC_AUTH_TOKEN` bearer-token auth unsupported — **REFUTED 2026-08-15** (area-12 pass; closed as `PROV-021`, exactly as this row directed): `cyrup-provider/src/env_api_keys.rs:44` lists `ANTHROPIC_AUTH_TOKEN` FIRST for anthropic; `:131` reproduces pi's display carve-out (`envKeys.find(key => key !== ANTHROPIC_AUTH_TOKEN_ENV)`, `env-api-keys.ts:147` @v0.83.0) so the token is discoverable for auth STATUS but never surfaced as an api key; `providers/anthropic.rs:38` defines `ANTHROPIC_AUTH_TOKEN_ENV` and `:114-126` resolves it into an `Authorization: Bearer` header with `source` reported, ahead of `ANTHROPIC_OAUTH_TOKEN` and `ANTHROPIC_API_KEY`. All four Verify cases are pinned by tests at `anthropic.rs:525-570`. |
| ~~DRIFT-033~~ | ~~medium~~ **CLOSED 2026-08-14** | port-divergence | M | — | A mid-run tool addition never reaches the system prompt — **CLOSED 2026-08-14**: sweep 2 — its refuter caveat ("this is not a one-line assignment; the prerequisite is modelling pi's two prompt slots") was CORRECT, and both parts landed together in cyrup-session-svc: `AgentSession.system_prompt_override: Mutex<Option<String>>` beside `base_system_prompt`, with `system_prompt_override()` / `effective_system_prompt()` (pi's `override ?? base`); `assemble_run_messages` writes the slot on all three branches; `drive_run` clears it at the head of its settle path (pi's `_runAgentPrompt` finally, agent-session.ts:1069); `push_active_tools` sets the agent prompt from `effective_system_prompt()` so a mid-run rebuild can no longer clobber a `before_agent_start` sanitization; and `PolicyHooks::prepare_next_turn` assigns `update.system_prompt` beside `update.tools`. **STRIKE the interim instruction to "promote the hooks.rs:158-161 comment into a documented [CYRUP-DELTA]" — the divergence it described no longer exists.** The one remaining delta is narrower and documented in-source: cyrup's `BeforeAgentStart` carries the prompt as a mutated-in-place `String`, so equality with the base stands in for pi's `!== undefined`. |
| ~~DRIFT-010~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | — | `get_available_thinking_levels` RPC dispatch arm missing (the method exists) — **CLOSED 2026-08-14**: sweep 1 — duplicate of SEAM-014, fixed in the same sweep. |
| ~~DRIFT-016~~ | ~~low~~ **CLOSED 2026-08-14** | **stale-port** *(was upstream-drift)* | S | duplicate-of: `SESS-019` | `Current date:` still injected into the system prompt — **CLOSED 2026-08-14**: sweep 1 — duplicate of SESS-019(a), now fixed. Its own re-derivation (removal predates the ported baseline ⇒ `stale-port`) was independently confirmed by re-running `git grep 'Current date' v0.83.0 -- packages/coding-agent/src`, which returns nothing. |
| ~~DRIFT-018~~ | ~~low~~ **REFUTED AT HEAD 2026-08-15** | **not-ported** | L | duplicate-of: `PROV-011` | Constrained sampling (strict JSON schema + Lark/regex grammars) absent — **REFUTED 2026-08-15** (area-12 pass; closed as `PROV-011`, exactly as this row directed): `crates/cyrup-core/src/constrained_sampling.rs` and `crates/cyrup-provider/src/utils/constrained_sampling.rs` both exist, `cyrup_core::Tool` carries the field (`tool.rs`), `ModelCompat` carries `supports_openai_grammar_tools`/`supports_strict_mode` (`api/compat.rs`), and it is APPLIED at every upstream site — `anthropic_messages.rs`, `google_generative_ai.rs`, `mistral_conversations.rs`, `azure_openai_responses.rs`, `openai_responses.rs`, `openai_completions.rs` — plus the WIT half in `cyrup-ext-sdk/src/{descriptor,guest}.rs` (`EXT-024`) and the tool-model half in `cyrup-agent`. |
| DRIFT-019 | low | **not-ported** *(was upstream-drift)* | M | duplicate-of: `PROV-014` | Radius / Qwen Token Plan providers unregistered (the wire API and OAuth landed) — **2026-08-15, BLOCKED on `DRIFT-009` for 3 of 4; the 4th is separable and NOT blocked**: area-12 pass, re-derived at `v0.84.2`. Still absent — `providers/all.rs:38-40` now says so in its own port-status table (`✗ NOT REGISTERED — PROV-014` for `qwen-token-plan`, `qwen-token-plan-cn`, `radius`) and `:377` asserts it, so the row is self-documenting at HEAD. **The three qwen token plans are blocked by `DRIFT-009`'s data block**: `qwenTokenPlanProvider` is `createProvider({ …, models: Object.values(QWEN_TOKEN_PLAN_MODELS) })` (`providers/qwen-token-plan.ts` @v0.84.2), i.e. a STATIC catalog, and that catalog is not in git at any revision. Registering them with an empty catalog is worse than absent — a provider that offers no models. **`radius` is different and was mis-scoped by this row:** `radiusProvider` (`providers/radius.ts` @v0.84.2) has **no static catalog at all** — it is `getModels: () => models` seeded empty plus a `refreshModels(context)` that restores from `context.stored`, imports the pre-ModelsStore legacy cache, then fetches `loadRadiusGatewayConfig(gateway, apiKey, signal)` and publishes. So radius needs the 96-line `radius-config.ts`, a `ModelsStore`-shaped refresh seam and a `builtin_oauth.rs` arm — a real but self-contained slice with no catalog dependency. **Split the item**: radius is schedulable now under `PROV-014`; the qwen half waits on `DRIFT-009`. |
| ~~DRIFT-020~~ | ~~low~~ **REFUTED AT HEAD 2026-08-15** | upstream-drift | S | duplicate-of: `PROV-024` | openai-responses affinity keys on removed `sendSessionIdHeader` — **REFUTED 2026-08-15** (area-12 pass; closed as `PROV-024`/`PROV-033`): `send_session_id_header` no longer exists as a field anywhere — its only surviving mention is the explanatory comment at `cyrup-provider/src/api/openai_responses.rs:547` recording that pi DELETED the flag (#6496). `SessionAffinityFormat` is the three-valued enum the row said a bool could not express: declared `api/compat.rs:345`, detected by `detect_session_affinity_format` (`:84`), resolved declared-over-detected (`:450-452`, `:743-745`), and consumed at `openai_responses.rs:552-555`, with `session_affinity_format_selects_the_header_set` at `:2112` and catalog coverage at `tests/catalog_data.rs:660-703`. |
| ~~DRIFT-024~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | duplicate-of: `SESS-013` | AGENTS.md loaded twice in nested git worktrees — **CLOSED 2026-08-14**: sweep 1 — duplicate of SESS-013, now fixed; both load-bearing guards (`:108`, `:113`) and the `isShadowed` gate (`:140-142`) are ported. |
| ~~DRIFT-025~~ | ~~low~~ **REFUTED AT HEAD 2026-08-15** | upstream-drift | S | duplicate-of: `CFG-017` | `${@:-default}` prompt-template defaults render literally — **REFUTED 2026-08-15** (area-12 pass; closed as `CFG-017`): `match_brace_form` in `cyrup-resources/src/prompt.rs` now accepts pi's full alternation `\d+\|ARGUMENTS\|@` — `target == "@" \|\| target == "ARGUMENTS"` substitutes `all_args`, a digits-only target indexes `args`, and anything else returns `None` because the regex alternative genuinely does not match. It also carries pi's JS truthiness (`""` takes the default) and `CFG-016`'s `${0:-…}` case (`args[-1]` is `undefined` ⇒ the default, where the port used to abort the whole form). |
| ~~DRIFT-027~~ | ~~low~~ **REFUTED AT HEAD 2026-08-15** | upstream-drift | S | duplicate-of: `PROV-025` | openai-completions has no `deferredToolsMode: "kimi"` — **REFUTED 2026-08-15** (area-12 pass; closed as `PROV-025`): `deferred_tools_mode: Option<DeferredToolsMode>` is declared on `ModelCompat` (`api/compat.rs:338`) and `ResolvedCompat` (`:499`), detected as `None` (`:669`) and resolved declared-over-detected (`:737`) — and it is **consumed**, not merely carried, at `openai_completions.rs:398` and `:1271`, the two sites matching pi's `:728`/`:1272`. The red-before note is recorded in-file at `:2386`. |
| ~~DRIFT-031~~ | ~~low~~ **REFUTED AT HEAD 2026-08-15** | **not-ported** | M | duplicate-of: `PROV-036` | No usage cost breakdown — per-model attribution and `Tools/summaries` unsurfaced — **REFUTED 2026-08-15** (area-12 pass; closed as `PROV-036`, both halves): `usage_cost_breakdown(entries)` and `UsageCostBreakdownEntry` are ported at `cyrup-session-svc/src/state.rs:159-230` citing `core/usage-totals.ts:37-70` @v0.83.0, exposed as `AgentSession::usage_cost_breakdown()` (`session.rs:3975`), **and rendered** at `cyrup-tui/src/app/execute_session.rs:155` under pi's own `breakdown.len() > 1` gate (`:184`) — the gate the row specifically asked for, so a single-model session still shows only the total. |
| ~~DRIFT-035~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | duplicate-of: `SESS-019` | Prompt tests assert the `Current date:` footer, pinning DRIFT-016 — **CLOSED 2026-08-14**: sweep 1 — both assertions at prompt/tests.rs are replaced, landed in the same change as the footer removal as the item requires. |
| ~~DRIFT-036~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | — | `settle()` uses a fixed 50 ms sleep as the only synchronization — **CLOSED 2026-08-14**: sweep 2 — `settle()` in `cyrup-session-svc/src/tests/summarization_retry_events.rs` no longer sleeps 50 ms. It takes the collector buffer and polls until the length is unchanged across 64 consecutive `yield_now`s — an observation of the state the assertions read — with an outer bound that turns a stuck pipeline into a NAMED PANIC rather than a hang. All ten call sites updated. The correct poll-until-observed pattern already existed in the same file at :420-425. |
| ~~DRIFT-039~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | duplicate-of: `AGENT-019` | `a_02_2_parallel_completion_vs_source_order` asserts a wall-clock bound and a completion ORDER — **CLOSED 2026-08-14**: sweep 1 — same test as AGENT-019 and closed by the same rewrite: the 115 ms bound is gone, a `Barrier(2)` plus a subscriber-driven oneshot make the completion order a fact, and the surviving 10 s `timeout` is documented in-source as a hang detector. **Sweep 2 additionally established that DRIFT-039 is the ONLY open DRIFT row whose fix lands in crates/cyrup-agent** — every row of this file's open table was read to confirm it, so no DRIFT work is owed by area 02. |
| ~~DRIFT-042~~ | ~~low~~ **HALF (a) CLOSED 2026-08-15** | not-ported | S | — | `/login` launches no browser and its "Cmd+click to open" hint is not a link — **CLOSED 2026-08-15 (area-12 pass) on half (a), which is the half this item owns.** New `crates/cyrup-tui/src/open_browser.rs` ports `utils/open-browser.ts:10-24` @v0.84.2 verbatim: `open` / `rundll32 url.dll,FileProtocolHandler` / `xdg-open`, argv vector with **no shell** (pi's stated security property at `:5-8` — an OAuth authorize URL is provider-supplied), stdio null, every error discarded. `LoginDialog::show_auth` calls it as its last act (`login-dialog.ts:111`) and `show_device_code` deliberately does not (`:118-131`). The launcher is an injectable field, not a global, so the call is asserted rather than assumed. **`[CYRUP-DELTA]`:** Node's `detached: true` + `.unref()` becomes a named reaper thread — a dropped `std::process::Child` is not reaped on Unix and the same function is reachable from an extension's `openUrl` (pi binds `openBrowser` onto the extension context at `interactive-mode.ts:353`), which is unbounded. **Half (b), the OSC-8 wrapping of BOTH the URL and the click hint (`:98-104`), stays open as `TUI-020`** — it needs a paint-time emitter, which is `07-cyrup-tui.md`'s work, and the module note in `login_dialog.rs` now says so. |
| ~~DRIFT-045~~ | ~~low~~ **CLOSED 2026-08-15** | not-ported | S | — | Ctrl+V with text on the clipboard inserts nothing — **CLOSED 2026-08-15** (area-12 pass). `read_clipboard_text()` added to `crates/cyrup-tui/src/clipboard.rs` beside the existing writer, porting `utils/clipboard.ts:35-69` @v0.84.2 including the Wayland branch pi added this delta (`bfc679d5e`): `clipboard_read_plan(os, env)` is a pure function gated on pi's literal three-way conjunction `platform() === "linux" && isWaylandSession() && WAYLAND_DISPLAY` (`:53`), then `wl-paste --no-newline --type text`, then `arboard`. **The three-state result is preserved and it is the subtle part**: pi's `ClipboardReadResult` (`:35`) distinguishes ok-with-null from failed, and `readClipboardText` returns `result.text` whenever `ok` — so an EMPTY Wayland clipboard must NOT fall through to the X11-oriented native backend, or it would paste a stale selection. Modelled as `Option<Option<String>>`. **TAG CORRECTION, adversarial review 2026-08-15 — only the WAYLAND branch is v0.84.x drift; the text-paste half is not-ported AT THE PORTED TAG, and every in-source citation for it says @v0.84.2.** Re-derived: `readClipboardText` already exists at v0.83.0 (`clipboard.ts:36-47`, native-only), and `handleClipboardPaste` at v0.83.0 is `interactive-mode.ts:2635-2658` — image-first, `return` at `:2647`, `readClipboardText()` at `:2650` — structurally identical to v0.84.2's `:2870-2893`. So cyrup was missing a v0.83.0 behaviour, not lagging a v0.84.x one. This is the **twelfth** row in this file to take the upstream-drift → not-ported correction; the Kind column already reads not-ported, but the citations should be re-tagged when this file is next touched. Minor off-by-ones in the same in-source block, listed so they are not re-derived: pi's `READ_CLIPBOARD_OPTIONS` is `:37-41` (cited `:36-40`), `readWaylandClipboardText` is `:43-50` (cited `:42-48`), the native read is `:65-70` (cited `:64-67`), `text || null` is `:67` (cited `:66`), and the text read/insert is `:2885-2889` (cited `:2884-2888`). `App::paste_from_clipboard` is pi's `handleClipboardPaste` (`interactive-mode.ts:2870-2892`): image first, text second, and the two reads are closures so the LAZINESS is testable — pi returns at `:2882` and never reads text when an image was found. **`[CYRUP-DELTA]`s, both recorded in-file:** the read gate stays pi's literal `"linux"` where the WRITE side's existing delta widened `p !== "linux"` to "macOS or Windows" (a failed read degrades to "no text"; a silently-succeeding write does not, which is what justified widening the other one) — asserted as a test so the two are not "made consistent" later; and the 5 s timeout comes from `recv_timeout` over a helper thread running `Command::output()`, because `output()` drains the pipe (a 50 MB clipboard cannot deadlock, which a `try_wait` poll over a piped stdout would) but has no timeout, and the helper still holds the `Child` so it is reaped. |
| ~~DRIFT-046~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | duplicate-of: `TOOL-036` | `normalizeWindowsShellPath` unported — Git-Bash/MSYS/Cygwin/WSL drive paths unconverted — **CLOSED 2026-08-14**: sweep 1 + 2 — **REOPENED AND RE-CLOSED, not a plain duplicate.** Sweep 1 closed it on TOOL-036's landing in `cyrup-tools/src/path.rs`, but a SECOND live instance existed in `crates/cyrup-config/src/paths.rs` — which is the 1:1 port of the very function upstream applies the rule inside (`utils/paths.ts` `normalizePath`) and was created AFTER this item was written, by CFG-025/CFG-036 — so cyrup ended sweep 1 with the rule in one copy of the normalizer and not the other. Now ported and applied at paths.ts:83-85's exact position (before the tilde expansion, inside the shared normalizer), with `test/paths.test.ts:133-150` ported verbatim INCLUDING the pass-through list, plus six extra grammar cases pinning where pi's regex backtracks and a hand parse does not. **The item's own Fix sentence — "Port the function into cyrup-resources (wherever `normalize_path` lives)" — names the wrong crate: `normalize_path` lives in cyrup-config, and cyrup-resources depends on it.** |
| DRIFT-047 | low | upstream-drift | L | duplicate-of: `VL-P5` | `packages/telemetry` and the `pi.ai.request` span contract absent — **2026-08-15, BLOCKED (measured), still open**: area-12 pass, re-derived at `v0.84.2`. Still absent — `grep -rnE 'TelemetryContext\|TelemetrySpan\|telemetry_context' crates --include='*.rs'` → **0**. Upstream sizes re-measured: `packages/telemetry/src` is **935 lines** (`index.ts` 357, `testing/conformance.ts` 315, `memory.ts` 219, `noop.ts` 20, `testing/{index,types}.ts` 24) and `packages/agent/src/harness/telemetry.ts` — where the `pi.ai.request` schema actually lives — is **615**. The threading surface in `packages/ai` is still just two lines (`types.ts:123` declares `telemetryContext?`, `api/simple-options.ts:36` forwards it), so the item's "SDK-surface parity today" note holds at v0.84.2 as well. **Why not attempted here rather than deferred silently:** the item's own refuter note says to resolve it by EXTENDING `PARITY-GAPS` VL-P5, not by opening a second L workstream from this ID, and landing only the trait pair would put a `telemetry_context` field on `StreamOptions` with no emitter and no conformance suite behind it — an invented surface, which is the `PROV-061` failure mode. Needs the VL-P5 owner. |
| ~~DRIFT-053~~ | ~~medium~~ **CLOSED 2026-09-05** | upstream-drift | M | — | `/share` gained a Radius-first upload path in `v0.84.3`; cyrup's `/share` still only knew the old gist-only flow — **CLOSED 2026-09-05** (batch-3 pass; code `9de4254b`). **The BLOCKER this row was filed under is gone at this HEAD:** `DRIFT-019`/`PROV-014` landed the real `radius` provider kind (`cyrup-provider/src/providers/radius.rs`, registered at `providers/all.rs:199-202`), so `get_provider("radius")` resolves and `tryShareViaRadius` has something to call. `crates/cyrup-tui/src/app/execute_misc.rs::share_session` now follows pi's restructured `shareSession` (`modes/interactive/session-share.ts:46-89` @v0.84.4): export the branch as JSONL once, try `try_share_via_radius` (`:57`), and only on `false` run the pre-existing `gh auth status` → `exportToHtml` → `gh gist create` chain. `try_share_via_radius` returns `false` for exactly the two conditions upstream does — no radius provider (`:93`), no resolvable credential (`:98`) — and `true` for **every** post-request outcome, so a failed Radius upload reports and does NOT republish the session as a gist, which is the divergence the row's Verify line specifically forbade "improving" into a fallback. The upload and its reply classification are a new `cyrup-provider/src/providers/radius_share.rs` (`artifacts_url` ← `:112-114`, `upload_share_artifact` ← `:110-149`, `classify_artifact_response` ← `:126-140`, `radius_share_token` ← `:95-98`), placed there for the reason upstream keeps `DEFAULT_RADIUS_GATEWAY` in `packages/ai` (`session-share.ts:6`). `exportSessionForShare`'s `pi.share` trailing entry (`:25-43` over `core/session-export.ts:21`,`:31-36`) IS ported — `cyrup-tui/src/app/share.rs::append_share_metadata` — so the row's "if the Radius viewer needs it parsed" caveat is settled in the affirmative. **Tag-to-tag:** the file is byte-identical at `v0.84.3` and `v0.84.4` (`git -C tmp/pi diff v0.84.3 v0.84.4 --` on that path is empty); this row's own line cites were off by a few (`shareSession` is `:46-89` not `:46-70`, `tryShareViaRadius` `:91-150` not `:72-101`, `shareViaGist` `:152-203` not `:103-140`) and are corrected here. The row also missed a third search param, `title` (`:114`). **Two supporting changes:** `impl cyrup_provider::CredentialStore for cyrup_config::AuthStore` — the impl that file's own header has always claimed and which did not exist, needed because `resolve_provider_auth` rotates an expiring OAuth token through `modify` on the store it is handed (`ai/src/auth/resolve.ts:117-149`) and a rotated refresh token written into a discarded in-memory map would leave `auth.json` invalid; and `auth_credential` (`cli/auth-command.ts:120-125`) in `cyrup-provider/src/auth/helpers.rs`. **Proof:** `cyrup-tui tests::share_radius_route` drives the row's two Verify lines against a real `AgentSession` with a seeded `auth.json` — with a radius credential `/share` reports the Radius upload's own failure sentence and never reaches `gh` (no "GitHub CLI", no "Gist:"), without one no Radius message appears at all — and was established RED behaviourally, not merely by symbol (short-circuiting the ported call site to `if false && self.try_share_via_radius(..)` fails the first test and leaves the second passing). Plus `tests::share_url::radius_share_tests` (the `pi.share` entry shape, the null-parent case, the three reported outcomes) and `providers::radius_share::tests` (URL, the 2xx-without-an-artifact arm, pi's three-tier failure detail with JS falsiness, an unparseable body, the credential gate both ways). **RESIDUALS (new, low):** (a) cyrup's `ApiKeyAuth::resolve` carries a `&Model` pi's does not (`ai/src/auth/types.ts:190-193`: "Resolution is provider-scoped") and radius's catalog is dynamic, so `radius_share_token` builds a provider-scoped `Model` describing the gateway — a divergence in the auth trait, not in `/share`; (b) the share URL is printed unwrapped, since pi's `hyperlink()` (`:139`) needs the paint-time OSC-8 emitter that is `TUI-020`'s; (c) a 15 s upload timeout upstream does not set, matched to `radius.rs`'s config-fetch ceiling.; **(d) recorded 2026-09-05 from review, undeclared until now, and given its own id `DRIFT-055` (low, open) by the batch-3 ledger pass as this row asked: the artifact uploaded to Radius is not the artifact pi uploads.** pi's `exportSessionToJsonl` writes `sessionManager.getBranch()` with `parentIds` REWRITTEN into a linear chain and a FRESH `new Date().toISOString()` header (`core/session-export.ts:21-38` @v0.84.4); cyrup feeds `append_share_metadata` the output of `AgentSession::export_to_jsonl`, which is `self.entries` — the WHOLE tree with original parents (`cyrup-session/src/manager/accessors.rs:20-30`) — and reuses the stored header timestamp. So a cyrup artifact carries abandoned branches pi would have stripped. This is a pre-existing property of `export_to_jsonl` that `/share` made newly user-visible; the commit body's description of the input as "the branch" is inaccurate for that reason. Not fixed here (it changes `export_to_jsonl` for every caller, `/export --jsonl` and RPC included); it wants its own id. |
| ~~DRIFT-054~~ | ~~medium~~ **CLOSED 2026-09-05** | parity-bug | S | — | ~~The templated HTML export drops `systemPrompt` and `tools`, so every exported document silently loses its System Prompt and Available Tools sections~~ — **CLOSED 2026-09-05** (batch-4 pass; code `16b38c93`). **The row was right on both sides and the Fix it named is what landed**, with one addition: rather than threading two more positional arguments, the shell now composes ONE value. New `AgentSession::export_state()` (`session/transcript.rs`) returns an `ExportState` carrying the manager's leaf, `current_system_prompt()` (`override ?? base` — pi's `agent.state.systemPrompt`, agent-session.ts:534) and `agent.state.tools` mapped to pi's three picked fields; `session_jsonl_to_html_with_theme`'s third parameter is that value instead of `Option<&str>`, and `session_data` emits `systemPrompt`/`tools` from it. All three live paths pass it — `AgentSession::export_to_html` (the RPC `export_html` path) and `cyrup-tui`'s `/export` (`app/execute_session.rs`) and `/share` (`app/execute_misc.rs`). `cyrup --export` passes `ExportState::from_file()`, which is pi's `exportFromFile` shape byte for byte (`index.ts:298-304`: both keys `undefined`, leaf from the file). **Tools are the ACTIVE set, not `all_tools()`** — pi maps `state.tools`, and `/share`'s own `pi.share` entry already draws that distinction two files away. **DESIGN (recorded per the batch rule):** the invariant encoded is pi's single `state?` gate — `systemPrompt` and `tools` are supplied together or not at all — plus "a live export carries all three shell-owned keys"; `ExportState`'s fields are private behind exactly two named constructors (`live`, `from_file`), so "passed the leaf, forgot the prompt" — literally how these keys went missing when the leaf was threaded alone — is unrepresentable. **CORRECTED at `e8f93355` (batch-4 review fix):** as first landed the type also derived `Default`, so `ExportState::default()` was a public, unnamed, lossy value a future LIVE caller could pass — the original defect, reproducible. The derive is gone and the file shape is the named `from_file()`; the doc now states plainly that a live caller passing `from_file` is still representable and that what holds the live paths is the source-grep test, not the compiler. What still needs runtime tests is that the values are the RIGHT ones, covered by the two source guards below. Rejected: two more positional parameters (five arguments, two of them `Option`, and nothing stopping the next caller repeating the defect); a nested `Option<ExportAgentState>` mirroring `state?` literally (the private constructors already enforce the gating without a second public type); reading the keys inside the renderer (it is pure and holds no session); a builder (one call-site shape). `Option<Vec<ExportTool>>` was kept rather than flattened because `undefined` and `[]` are different payloads upstream. Migration cost: three production call sites, six test call sites, one added `pub use`; no serde on either new type, and `session_jsonl_to_html(&str)` keeps its signature so `cyrup --export` and the `cyrup-tui` re-export were untouched. **Proof:** `src/tests/export_html.rs::a_live_export_carries_the_system_prompt_and_the_active_tools` (the base64 payload carries both keys; a tool entry has exactly `{description, name, parameters}` and nothing else), `::the_file_only_export_omits_both_agent_keys_the_way_pi_does`, `::an_empty_live_tool_set_is_an_empty_array_not_an_absent_key`, plus the two renamed source guards `::export_to_html_passes_the_live_session_state_to_the_renderer` and `cyrup-tui/src/tests/export.rs::both_tui_export_sites_pass_the_live_session_state`. **RED measured**, not asserted: stripping the two `map.insert` arms from `session_data` failed the first and third (`left: Null, right: "You are cyrup.\nBe brief."`); pointing `export_to_html` at the file-only shape failed the session-svc guard; pointing both TUI sites at it failed the TUI guard. `nextest -p cyrup-session-svc` 353/353, `-p cyrup-tui` 1415/1415 (1 skipped); clippy `-D warnings` and `RUSTDOCFLAGS='-D warnings' cargo doc` clean on both; `cargo check -p cyrup-modes` and `-p cyrup` for the downstream consumers. **Two corrections this closure carries:** `DRIFT-041`'s row asserted the export was complete when it was two keys short (corrected in that row above), and the `agent-session.ts:3438` citation this row and `DRIFT-041`'s both used is `:3439`. **Residual:** `renderedTools` only. **RESTATED at `e8f93355` (batch-4 review fix):** the first wording — "exactly what pi's own `exportFromFile` omits" — cited the FILE entry point to excuse a LIVE-path gap, which is the same false-comfort defence this row's own closure removed for `systemPrompt`/`tools`. pi's live path ALWAYS builds a renderer (`createToolHtmlRenderer(…)` unconditional at `agent-session.ts:3433-3437`, passed at `:3439-3443`, pre-rendered into the payload at `export-html/index.ts:254-261`, `:269`); only `exportFromFile` (`:288-316`) passes none. So this is a genuine live-path residual: a custom-rendered EXTENSION tool falls back to `template.js`'s built-in card (`:1026`) where pi would show the extension's own. Every built-in tool is unaffected, and `cyrup --export` matches upstream exactly. The `agent-session.ts:3022` citation on `export_to_html` was also stale (`exportToHtml` is at `:3427` @v0.84.4) and is corrected in the same commit. Original filing: — filed 2026-09-05 by the batch-3 ledger pass against the code `DRIFT-041` landed (`ce81dba9` + `824a539e`), after the batch-3 second review raised it and the batch ended without answering it. **upstream** `core/export-html/index.ts:263-269` @v0.84.4 — `exportSessionToHtml` builds `SessionData { header, entries, leafId, systemPrompt: state?.systemPrompt, tools: state?.tools?.map((t) => ({name, description, parameters})), renderedTools }`; `AgentSession.exportToHtml` ALWAYS passes `this.state` (`core/agent-session.ts:3439`), and it is the only entry point `/export` (`interactive-mode.ts:6023`), `/share` (`session-share.ts:72`) and RPC `export_html` (`rpc-mode.ts:601`) have. **cyrup** `crates/cyrup-session-svc/src/export/mod.rs::session_data` inserts `header`, `entries` and `leafId` and nothing else, and no call site supplies the other two. The shipped byte-identical `template.js` renders both blocks from those keys (`:1405-1435`, destructured at `:15`), so the sections are simply absent. **Impact** every cyrup HTML export is missing two visible sections a reader of the template would expect; silent, on all three paths. Not `exportFromFile`'s documented shape — that is `cyrup --export`'s path, not a front-end's. **Fix** thread the live system prompt and tool list through the same shell `AgentSession::export_theme()` and `export_leaf_id()` already use; `/share` reads `session.current_system_prompt()` and `session.all_tools()` a few lines away in `cyrup-tui/src/app/execute_misc.rs`, so both inputs are reachable. Correct the two in-source claims that assert the opposite (`session/transcript.rs`, `export/mod.rs`'s `SessionData` doc) in the same change. **Verify** a red-before test asserting the base64 payload carries both keys and that the rendered document contains the System Prompt header for a session that has one |
| DRIFT-055 | low | upstream-drift | S | — | **`/share` uploads the whole session tree with original `parentId`s and the stored header timestamp, where pi uploads the linearised current branch with a fresh one** — `DRIFT-053`'s residual (d), given its own id 2026-09-05 as that row asked. **upstream** `core/session-export.ts:21-38` @v0.84.4 — `exportSessionToJsonl` writes `getBranch()` with `parentId` rewritten into a linear chain and a fresh `new Date().toISOString()` header timestamp. **cyrup** `crates/cyrup-session/src/manager/accessors.rs::export_jsonl` writes the whole tree with parentIds intact and the stored header timestamp, and `/share` posts that. **Impact** a shared artifact carries abandoned branches the sharer did not mean to publish, and its header timestamp is the session's creation time rather than the share time. **Fix** give the share path a linearised export, leaving `cyrup --export`'s whole-tree dump alone. **Verify** share a session with two branches; assert only the current branch's entries are in the artifact and that the header timestamp is the share time |
| ~~DRIFT-050~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | — | `CYRUP_TELEMETRY=` empty is an explicit OFF upstream and a silent no-op here — **CLOSED 2026-08-14**: sweep 2 — `CYRUP_TELEMETRY=` / `PI_TELEMETRY=` (set but empty) is now an explicit OFF that beats the settings opt-in, restoring pi's tri-state (unset / set-empty / set-truthy): the telemetry field no longer goes through the `!v.is_empty()` filter and takes the first key that is SET AT ALL, while the two sibling flags (`offline`, `skip_version_chk`) keep the per-key empty filter, which is indistinguishable from pi for them. **MECHANISM NOTE the item could not anticipate: this could not be tested by mutating the process environment, because `std::env::set_var` is `unsafe` under Rust 2024 and `cyrup-config` is `#![forbid(unsafe_code)]`. `EnvVars::from_lookup(get)` was added as a pure seam and `from_process` reduced to `from_lookup(\|k\| std::env::var(k).ok())`; `first_env` no longer exists. Any area file citing `cyrup-config/src/env.rs:50-53 first_env` is now stale, and any future env-tier parity item in that crate needs the same seam.** |
| ~~DRIFT-051~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | — | `process.title`'s role suffix never set — RPC / runner / broker children are all bare `cyrup` in `ps` — **CLOSED 2026-08-14**: sweep 1 — duplicate of SEAM-070, fixed in the same sweep. **Sweep 2 carries SEAM-070's own caveat across: on macOS `pthread_setname_np` does not change what `ps -o comm=` prints, so this item's Verify line holds verbatim only on Linux.** |
| ~~DRIFT-052~~ | ~~medium~~ **FILED AND CLOSED 2026-08-15** | upstream-drift | S | — | Fireworks GLM 5.2 lost pi's `openAICompat` — no session-affinity header and a long cache retention Fireworks does not honour — **FILED AND CLOSED 2026-08-15** (area-12 pass; this is the unrecorded item the pass was asked to give an id). pi `b9497c8c1` ("fix(ai): correct Fireworks GLM prompt caching, closes #7676") replaced the inline `candidate.compat = { supportsStore: false, supportsDeveloperRole: false }` the GLM rows carried at the ported tag (`ai/scripts/generate-models.ts:2151-2155` @v0.83.0) with the shared `openAICompat` constant `processFireworksModels` builds (`:1239-1244` @v0.84.2), adding `sendSessionAffinityHeaders: true` and `supportsLongCacheRetention: false`. **The tag attribution in the hand-off was off by one: it first ships in `v0.84.0`, not `v0.84.1`, and is unchanged at `v0.84.2`.** See the body below. |

| ~~DRIFT-022~~ | ~~tracker~~ **CLOSED 2026-09-04** | **FIXED** *(via `SEAM-051`, `TUI-019`, `CFG-021`)* | L | duplicate-of: `SEAM-051` | TUI mode (`--tui-mode`, alternate screen) not ported — **CLOSED 2026-09-04**: area-12 re-audit at cyrup HEAD `2571969`. The renderer half this row was tracking has landed: `dbcf59a` ("ship the fullscreen (alternate-screen) TUI mode, ADR-0005 B-1..B-14") wires `--tui-mode` (`crates/cyrup/src/cli/args.rs:184-185`) through to `App::switch_tui_mode` at `crates/cyrup/src/interactive.rs:207-217`, precedence flag-then-setting-then-`regular` exactly as this row's Fix specified; the `tuiMode`/`fullscreenScrollbar` settings keys are live at `crates/cyrup-config/src/settings/manager.rs:387,395` and `effective.rs:461`. The commit records the binary driven under a real pty (alt-screen enter/leave escapes, wheel-report handling, tmux-aware mouse-capture divergence observed and matched) — an `observed`, not merely static, closure. Re-confirmed upstream still ships the surface unchanged at `v0.84.4` (`packages/coding-agent/src/cli/args.ts:203-209`, `--tui-mode`; `packages/tui/src/tui-alt-screen.ts`, 1378 lines at that tag). **Not re-verified**: line-for-line fidelity against pi's full 1378-line `tui-alt-screen.ts` was not diffed — any residual fine-grained parity gap in the renderer itself is `TUI-019`'s (area 07) to find, not this tracker's. |
| DRIFT-023 | **tracker** · *lead* | tracking | L | duplicate-of: `CFG-020` | Model registry → `ModelRuntime` refactor not absorbed — **evidence unverified** |
| DRIFT-032 | **tracker** | **not-ported** *(was upstream-drift)* | L | duplicate-of: `EXT-027` | llama.cpp router integration and Hugging Face model search entirely unported |
| DRIFT-040 | **tracker** · *lead* | tracking | L | duplicate-of: `VL-P22` | pi's agent-harness v2 rearchitecture entirely unabsorbed — **evidence unverified** |

**Count, re-derived 2026-09-04 (area-12 pass, cyrup HEAD `2571969`):** 36 IDs in this table (35
DRIFT-NNN rows + the new `DRIFT-053`) = **26 closed** + **10 open** — **7 severity-bearing (0
critical, 0 high, 5 medium, 2 low)** + **3 trackers** (`DRIFT-023`, `DRIFT-032`, `DRIFT-040`; the
fourth, `DRIFT-022`, closed this pass). The stale `34 IDs / 4 trackers` figure below predates the
2026-08-15 pass's eight REFUTED-AT-HEAD closures and this pass's `DRIFT-022` closure and `DRIFT-053`
filing; it is struck rather than deleted per the no-renumbering rule, and superseded by this line.
**Deduplicated, this area uniquely owns 15 IDs (the original 14 plus `DRIFT-053`) — 0 high, 9
medium, 6 low, of which ~~3~~ 2 are still open:** medium `DRIFT-004`\* *(closed 2026-09-05)*,
`DRIFT-013`\*, `DRIFT-014`\*, `DRIFT-028`\*, `DRIFT-029`\*, `DRIFT-033`\*, `DRIFT-041` (open),
`DRIFT-048`\*, `DRIFT-053` (open, new); low `DRIFT-010`\*, `DRIFT-036`\*, `DRIFT-042`\*,
`DRIFT-045`\*, `DRIFT-050`\*, `DRIFT-051`\*
(\* = closed, kept for the ID census). **Superseded 2026-09-05 (batch-3): `DRIFT-004` is CLOSED**,
so the table is 36 IDs = **27 closed + 9 open** — **6 severity-bearing (0 critical, 0 high, 4
medium, 2 low)** + 3 trackers, and the uniquely-owned open set is **two** mediums (`DRIFT-041`,
`DRIFT-053`). **Superseded again 2026-09-05 (same batch): `DRIFT-053` is CLOSED** (code
`9de4254b`), so the table is 36 IDs = **28 closed + 8 open** — **5 severity-bearing (0 critical, 0
high, 3 medium, 2 low)** + 3 trackers, and this area's uniquely-owned open set is **one** medium,
`DRIFT-041`. **Superseded a third time 2026-09-05 (same batch): `DRIFT-041` is CLOSED** (code
`ce81dba9`), so the table is 36 IDs = **29 closed + 7 open** — **4 severity-bearing (0 critical, 0
high, 2 medium, 2 low)** + 3 trackers, and **this area's uniquely-owned open set is now EMPTY**:
every remaining open row here (`DRIFT-009`, `DRIFT-015`, `DRIFT-019`, `DRIFT-047`, `DRIFT-035`,
`DRIFT-036`, `DRIFT-039`, and the three trackers) is a duplicate scheduled in its owning area or a
test defect owned elsewhere. ~~`DRIFT-041` leaves one NEW low residual behind it — the
`renderedTools` / `ToolHtmlRenderer` + ANSI-to-HTML seam — recorded in its row, not filed as a
separate ID.~~ **Superseded a FOURTH time 2026-09-05 by the batch-3 ledger pass, and this one moves
the count the other way.** That residual list was incomplete: `DRIFT-041`'s payload also drops
`systemPrompt` and `tools`, which is a user-visible loss of two document sections on all three
export paths and is now **`DRIFT-054` (medium, open)**; and `DRIFT-053`'s residual (d) — the
whole-tree, original-`parentId`, stored-timestamp `/share` artifact — has been given the id its own
row asked for, **`DRIFT-055` (low, open)**. So the table is **38 IDs = 29 closed + 9 open** —
**6 severity-bearing (0 critical, 0 high, 3 medium, 3 low)** + 3 trackers, and this area's
uniquely-owned open set is **`DRIFT-054` (medium) and `DRIFT-055` (low)**. `renderedTools` remains
a genuine low residual recorded inside `DRIFT-041`'s row rather than filed. **Superseded a FIFTH
time 2026-09-05 (batch 4): `DRIFT-054` is CLOSED** (code `16b38c93`) — the batch-4 rule is that a
defect found in the CURRENT batch's own code is fixed, not filed, and `DRIFT-054` was batch 3's own
output, so batch 4 opened by closing it. This area's uniquely-owned open set is now **`DRIFT-055`
(low)** alone. Derive the live figures with `scripts/count_open_items.py`, never from this
paragraph — it has now been superseded five times in two days, which is itself the argument for the
script.
~~This area's entire open-and-uniquely-owned set is three
mediums — `DRIFT-004`, `DRIFT-041`, `DRIFT-053`~~ — everything else open here (`DRIFT-009`,
`DRIFT-015`, `DRIFT-019`, `DRIFT-047`, and the three trackers) is scheduled in its owning area and
this file is read only for the extra evidence it carries.** ~~**Count:** 34 IDs =
**30 severity-bearing (0 critical, 1 high, 11 medium, 18 low)** + 4 trackers.
**Deduplicated: 14 IDs this area uniquely owns — 0 high, 8 medium, 6 low.** The single high is a
duplicate and is scheduled in area 08, so **this area contributes no high to a deduplicated plan.**~~

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

## DRIFT-052 — Fireworks GLM 5.2 lost pi's `openAICompat`: no session-affinity header, and a long cache retention Fireworks does not honour — **FILED AND CLOSED 2026-08-15**

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

> **Filed and closed in the same pass.** This is the drift an earlier sweep found and never gave an
> id: the hand-off called it "the v0.84.1 `openAICompat` change". Re-derived here — it first ships
> in **`v0.84.0`** (`git tag --contains b9497c8c1` → `v0.84.0`, `v0.84.1`, `v0.84.2`) and is
> unchanged at `v0.84.2`, so the tag in the hand-off was off by one.

**upstream** — pi `b9497c8c1`, "fix(ai): correct Fireworks GLM prompt caching, closes #7676". At the
ported tag `v0.83.0` the Fireworks GLM rows take an inline **assignment** in the post-ingest patch
loop (`packages/ai/scripts/generate-models.ts:2151-2155`):

```ts
if (candidate.provider === "fireworks" && candidate.id.includes("glm-5p2")) {
    candidate.api = "openai-completions";
    candidate.baseUrl = "https://api.fireworks.ai/inference/v1";
    candidate.compat = { supportsStore: false, supportsDeveloperRole: false };
}
```

It assigns rather than spreads, so it **discards** the four keys the models.dev Fireworks ingest had
just set (`:1560-1565` @v0.83.0). `b9497c8c1` moves Fireworks into a dedicated
`processFireworksModels` and gives the openai-completions rows a shared constant
(`:1239-1244` @v0.84.2):

```ts
const openAICompat: OpenAICompletionsCompat = {
    supportsStore: false,
    supportsDeveloperRole: false,
    sendSessionAffinityHeaders: true,
    supportsLongCacheRetention: false,
};
```

applied to every `glm-5p2` row at `:1274-1280` (and spread into `kimiK3Compat`, which cyrup's
catalog has no row for). Both new keys are declared on `OpenAICompletionsCompat` at **v0.83.0**
already (`packages/ai/src/types.ts:565`, `:571`) — this is a change to the *catalog data*, not to
the compat interface, which is why an interface-shaped sweep never saw it.

**cyrup** — `crates/cyrup-provider/src/providers/catalog/fireworks.json` carried the v0.83.0 shape on
both openai-completions rows, `accounts/fireworks/models/glm-5p2` and
`accounts/fireworks/routers/glm-5p2-fast`: `{"supportsStore": false, "supportsDeveloperRole": false}`.
The other fourteen rows are `anthropic-messages` and legitimately carry
`sendSessionAffinityHeaders: true`.

**Impact — neither key is auto-detected for Fireworks, so the catalog value alone decides both, and
they fail in opposite directions.**

* `send_session_affinity_headers` is hardcoded `false` in `detect_compat`
  (`api/compat.rs:670`, pi `openai-completions.ts:1471`), and the header gate is
  `if (sessionId && compat.sendSessionAffinityHeaders)` (pi `:647`). Absent ⇒ cyrup sends no
  `session_id` / `x-client-request-id` / `x-session-affinity` to Fireworks. Fireworks routes prompt
  cache by **replica affinity**, so every cache lookup misses — a continuous, silent cost.
* `supports_long_cache_retention` detects as
  `!(is_together || is_cloudflare_workers_ai || is_cloudflare_ai_gateway || is_nvidia || is_ant_ling)`
  (`api/compat.rs:678-682`, pi `:1474-1480`) — Fireworks is on none of those lists, so it detects
  **true**. Absent ⇒ cyrup requests a long retention Fireworks does not honour.

**This is the third time these two flags have moved on these two rows, which is the reason the fix
is shaped the way it is.** An early sweep *invented* them here (pattern-matched off the fourteen
anthropic rows); `PROV-061` removed them as unsourced, correctly — pi carries them at neither
provenance revision (`91585d9a`, `b0c2a90e`); this item restores them **with a citation**. A value
that keeps oscillating is a value with no provenance record, so it is not restored by hand.

**Fix (landed)** — Two `Delta` rows in the generator's `DELTAS` table
(`xtask/src/main.rs`, `FIREWORKS_OPENAI_COMPAT` + `WHY_FIREWORKS_GLM_COMPAT`), one per model id,
pinning `compat` to pi's four-key object in upstream's own key order; then
`cargo run -p xtask -- gen-catalogs`. The generator rewrote exactly two files —
`catalog/fireworks.json` (the two rows) and `catalog_manifest.json` (whose EXCEPTIONS prose is
derived from `DELTAS`, so the count went 10 → 12 and the new rows name themselves). This is the same
signed-off forward-port mechanism the six GPT-5.6 cost pins already use, and it means
`gen-catalogs --check` still reports *"all 36 files match pi@b0c2a90e"* while a future regeneration
cannot silently revert the decision — the failure mode `PROV-064` warns about.

**Verify (landed)** — Two tests, both RED before the catalog change:
`providers/fireworks.rs::anthropic_and_openai_models_route_per_api` (rewritten: it had *asserted the
bug*, pinning `send_session_affinity_headers == None` / `supports_long_cache_retention == None` and
naming the v0.84.x reinstatement as something that must not be encoded), and
`tests/catalog_data.rs::fireworks_openai_completions_rows_carry_pi_s_openai_compat` (renamed from
`…_carry_no_invented_affinity_or_cache_flags`, still stated over the whole provider so a third
openai-completions row appearing with the block copied onto it fails). Both assert the **resolved**
compat via `get_compat`, not only the declared `Option`s — the declared-value check alone would pass
against a catalog that had lost its compat block entirely.

**Bookkeeping** — `PROV-061` (`01-cyrup-core-and-provider.md:300`, `:1638`) is **superseded**, not
overturned: its analysis of the *provenance* was right and its removal was the right call at the
time. Its row and body should be annotated rather than reverted.

## ~~DRIFT-041~~ — ~~medium~~ **CLOSED 2026-09-05** — Session HTML export is a 131-line text dump against pi's 5,021-line templated document

**Kind** not-ported · **Severity** ~~medium~~ **CLOSED 2026-09-05** · **Effort** L · **Confidence** high

> **CLOSED 2026-09-05 (batch-3), code `ce81dba9`.** Everything below is the ORIGINAL filing and is
> kept unedited per the no-deletion rule. It held up: both measurements were re-derived at
> `v0.84.4` (the directory is still eight files, and still unchanged across the delta — the tag
> moved from `v0.84.1` to `v0.84.4` with no edit to `core/export-html/`), and all five numbered
> steps of its **Fix** were followed in order. Steps 1-4 landed together, for the reason the
> Open-items row gives; step 5 (`ToolHtmlRenderer` + ANSI-to-HTML) is the one residual, and the
> filing's own "it is the smallest slice, not the main one" is why that is a complete port rather
> than a partial one — pi's `exportFromFile` (`index.ts:288-316`) passes no renderer either, and
> `template.js:1026` reads `renderedTools?.[…]` with its own fallback.
>
> **The filing's Verify is satisfied point for point** by
> `crates/cyrup-session-svc/src/tests/export_html.rs`, whose fixture is the one it specifies —
> (a) an assistant message with a fenced code block, (b) a `bash` call **plus** its result,
> (c) an extension tool result, (d) two branches off one parent. The HTML carries the tool name
> **and the command string** (`tool_calls_keep_their_name_arguments_and_result_metadata`), ships
> the highlighter that produces `<code class="hljs">` in the browser and the tree/sidebar scaffold
> (`document_carries_the_template_and_both_vendored_libraries`), rebuilds exactly the JSONL entries
> from the embedded `session-data` (`embedded_session_data_reproduces_the_jsonl_exactly`), and
> matches the active theme's export colours rather than a constant
> (`palette_comes_from_the_active_theme_not_a_constant`). Seven of the twelve were measured RED
> against the previous renderer.
>
> **One correction to the filing.** Its cyrup-side line cites were written against a 131-line
> `export.rs`; that file no longer exists. The seam is `crates/cyrup-session-svc/src/export/`
> (`mod.rs`, `color.rs`, `assets/`), and the four consumers it lists are unchanged except that two
> of them (`execute_session.rs`, `execute_misc.rs`) and `session/transcript.rs` now pass a theme.
> The in-tree residual note at the old `export.rs:12-14` that the filing called "understating it by
> an order of magnitude" is gone with the file.

**cyrup** — `cyrup/crates/cyrup-session-svc/src/export.rs` — the **whole** renderer is 131 lines (`escape`, `collect_text`, `entry_role`, `session_jsonl_to_html`, `EXPORT_CSS`). `session_jsonl_to_html` (`:72-121`) splits `export_jsonl`, takes `name`/`title` and `cwd` off the header, and for each remaining line emits `<section class="entry {role}"><header>{role}</header>` plus one `<pre>` per string harvested by `collect_text` (`:35-56`), which recurses looking **only** for values under a `"text"` key. Styling is a hardcoded 8-line `EXPORT_CSS` const (`:124-131`) with a fixed `#1e1e2e` dark palette, not the user's theme. Four consumers: `cyrup-tui/src/app/execute_session.rs:91` (`/export`) and `app/execute_misc.rs:310` (`/share`), `cyrup-modes/src/rpc.rs:1257` (RPC `export_html`), `cyrup/src/main.rs:1321` (`--export`). The residual is acknowledged in-tree at `export.rs:12-14` but names only `tool-renderer.ts`, understating it by an order of magnitude.

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

## ~~DRIFT-004~~ — ~~medium~~ **CLOSED 2026-09-05** — RPC `bash`: `UserBashEventResult.operations` backend seam unported

**Kind** upstream-drift · **Severity** ~~medium~~ **closed** · **Effort** M · **Confidence** high

> **CLOSED 2026-09-05 (batch-3), code `8b688401`.** Everything below is the ORIGINAL filing and is kept unedited per the no-deletion rule; two of its three claims went stale long before this pass and the row above records when. What actually remained on 2026-09-05 was the GUEST tier alone, and it is now ported — see the Open-items row for the full closure, the world members and the tests. In summary: `cyrup:ext@0.9 -> @0.10` adds `registration.register-bash-operations`, the keyed `events.bash-operations-exec` export and the `host-bash` interface (`emit-bash-output` = pi's `onData`, `is-bash-cancelled` = pi's `signal`); `ExtensionHost::user_bash_operations` resolves a native object OR a `GuestBashOperations` forwarder; the guest half shipped in the same change so no Tier-1 guest broke; and `cyrup-it/tests/ext/wasm_bash_operations.rs` proves the round trip against a real component.
>
> **The filing's own Fix and Verify are both satisfied**, the Fix by earlier passes (the `cyrup-tools` trait, the `BashOptions` override, the `cyrup-modes` population) and the Verify by `cyrup-session-svc/src/tests/round9_l5res.rs`'s `..._operations_override_...` tests (a custom backend is used for that call and not the next; with no override the local backend still runs) plus this pass's guest-tier tests. **One residual, new and low:** a guest backend is bounded by the wasm epoch budget where pi's backends are unbounded — see the row.

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

> **2026-09-04 — PARTIAL CLOSURE (`9e2bfda5`, `xtask/src/main.rs`), batch-3 pass, upstream
> re-read at `v0.84.4`.** The item's Fix names two contributions. **(2) is DONE**: the drift
> check's comparison is now a port of `scripts/diff-model-catalog.mjs` rather than an invention —
> `canonicalize` is its `canonicalizeJson`/`sortJsonKeys` (`v0.84.4 scripts/diff-model-catalog.mjs:96-114`),
> including the thinking-ladder key order for `thinkingLevelMap`/`values` (`:93`) and the literal
> detail that array entries canonicalize with **no** parent key (`:106`), and `diff_catalog` is its
> single walk over the sorted union of both sides' model ids (`:191-195`) with equality decided on
> the canonical forms. That was a live defect and not only a fidelity point: `field_diff` compared
> `Val`s whose object equality is order-sensitive, so a pi revision that merely reordered a `compat`
> block would have reported every row in the catalog as changed
> (`xtask/src/main.rs::key_order_alone_is_not_a_field_difference` fails without the two
> `canonicalize` calls — verified by reverting them).
>
> **(1) stays data-blocked, re-measured at `v0.84.4`, but is no longer prose.** 39 `*.models.ts`
> upstream (`git -C tmp/pi ls-tree v0.84.4 --name-only packages/ai/src/providers/`), 35 catalogs in
> the tree (34 provider catalogs + `openrouter-images.json`), the same four missing. All four are
> `import values from "./data/<p>.json"` re-exports at `v0.84.4` (`baseten.models.ts:4`,
> `qwen-token-plan.models.ts:4`, `-cn:4`, `-individual:4`), `.gitignore:11` still ignores that
> directory, and `git log --all -- packages/ai/src/providers/data/<p>.json` is empty for all four.
> The new `UNPORTED` table in `xtask/src/main.rs` records all five upstream modules with no catalog
> (the four, plus hand-ported `together`) with a reason, an owning item and the upstream commit that
> explains it, and `gen-catalogs --roster <rev>` audits `CATALOGS ∪ UNPORTED` against pi's actual
> provider set. Measured: `--roster v0.84.4` → *"39 provider module(s) upstream — 34 embedded, 5
> accounted for as unported"*; `--roster b0c2a90e` → 35 upstream, 34 embedded, 1 unported, plus the
> four the floor is too old to ship. It exits non-zero when pi ships a module neither table names,
> when cyrup embeds a catalog pi has retired, and — the trigger this item needs — when a blocked
> module PARSES as a data literal at that revision, i.e. **the block has lifted**. That is the guard
> the item was filed for: the count that went stale (31 shipped vs 35 vs 39) can no longer disagree
> with the tree unnoticed.
>
> **Two refinements to the block itself, from reading `v0.84.4`:**
> 1. It is per-FIELD, not total. Everything `packages/ai/scripts/generate-models.ts` hardcodes for
>    these providers is in git — `baseUrl`, the `compat` blocks, `thinkingFormat`, the thinking
>    ladders, the reasoning-effort exclusion set and the Individual allowlist (`:290-334`,
>    `:1259-1330`, `:2303-2380`) — and `packages/ai/test/baseten-models.test.ts` /
>    `qwen-token-plan-models.test.ts` even carry full per-model expectations for a few rows. What is
>    unobtainable is exactly the models.dev half: `cost`, `contextWindow`, `maxTokens`, `name`,
>    `reasoning` and the input modalities (`:2334` `data[source]?.models`, `:1944`
>    `processBasetenModels(data.baseten)`).
> 2. The second of the two escalation paths is closed here too: models.dev is unreachable from this
>    workspace — `curl https://models.dev/api.json` returns `CONNECT tunnel failed, response 403`
>    through the agent proxy — so even a hand-written extractor could not fetch it.
>
> A partial catalog is not an acceptable half-measure: `DRIFT-019` makes the same call for the
> provider registrations ("registering them with an empty catalog is worse than absent"). **The
> escalation and the severity stand**; what changed is that a command now fails the day it lifts.

> **2026-09-05 — the registration half of contribution (1) is CLOSED, batch-4 pass, code
> `e4078e3a` (wire) + `ccd14981` (registration). Two corrections to the block's own statement.**
>
> **1. The Fix's *"landing in the same change as DRIFT-019 / `PROV-014`'s registrations"* is now
> satisfied for all four.** `PROV-014` landed three on 2026-09-04 — the Qwen Token Plan trio, as
> `FleetCatalog::Dynamic` members with no embedded rows — and left `baseten` out by name, recording
> it as *"the one v0.84.x built-in still unregistered … with no ledger id"*. This item is that id.
> `providers/fleet.rs::BASETEN` is `ai/src/providers/baseten.ts:6-14` @v0.84.4 field for field —
> id, name, `envApiKeyAuth("Baseten API key", ["BASETEN_API_KEY"])`, and the `baseUrl` its generator
> hardcodes (`ai/scripts/generate-models.ts:1259`) — registered between `ant-ling` and `cerebras`,
> which is `all.ts:92`,`:95`,`:96`'s own order; `env_api_keys.rs` gains `env-api-keys.ts:106`'s row
> and `--help`'s env block gains `args.ts:406`'s. **cyrup now registers all forty of pi v0.84.4's
> built-in providers.**
>
> **It could not be registered alone, and that is the substantive finding of this pass.** Baseten's
> rows are the only rows in pi that carry `thinkingFormat: "baseten"` — but not all of them do, and
> the first draft of this row (and of the source comment it justified) got that backwards.
> `processBasetenModels` selects one of FOUR compat blocks per row (`generate-models.ts:1310-1319`
> @v0.84.4): `toggleReasoningCompat` and `toggleReasoningEffortCompat` (`:1274-1283`) carry
> `thinkingFormat: "baseten"`, `reasoningEffortCompat` (`:1269-1273`, an `effort` option with no
> `toggle`) carries `"openai"`, and `baseCompat` (`:1260-1268`, a non-reasoning row) carries no
> format at all. cyrup's `ThinkingFormat` had no `baseten` variant, so `serde` failed the entire
> `ModelCompat` — hence the whole `Model` — for every TOGGLE-reasoning row. Since the four blocked
> providers have **no embedded rows at all**, every row they will ever have arrives by
> deserialization at runtime, so a registered baseten would have silently offered only its
> effort-only and non-reasoning subset, with its toggle-reasoning models (including the GLM-5.2 pair
> upstream pins by hand at `test/baseten-models.test.ts:19-54`) permanently missing. That is the
> `DRIFT-019` hazard ("registering them with an empty catalog is worse than absent") in its partial
> form, and it is why the wire port had to land in the same slice.
> **CORRECTED 2026-09-05 (batch-4 review):** the sentences replaced here claimed the format is on
> *every* Baseten reasoning row and that the provider would have offered *zero* models. Both were
> false against the cited lines; the correction changes no code and the port stands. Ported with it: `ThinkingFormat::Baseten`
> (`ai/src/types.ts:584`, doc line `:578`), `ModelCompat::chatTemplateArgs` (`types.ts:593-594`;
> detected `{}` at `api/openai-completions.ts:1649`, resolved `:1695`) and the wire branch
> (`:888-904`). The branch has a literal detail worth keeping: its two halves are independent, and
> the `reasoning_effort` half carries **no** `options.reasoningEffort` guard — unlike every sibling
> format — so with thinking off it still emits `thinkingLevelMap.off` (`:899`). `buildChatTemplateValues`
> (`:1010-1026`) now takes the values map rather than the compat struct, as upstream does, because
> the same resolution feeds two different request fields (`:884` vs `:893`).
>
> **2. One of the two escalation paths this item names is REFUTED as stated.** The Open-items row
> said the block needs *"a `node`/`npm install` toolchain to run pi's `generate-models` (which this
> workspace does not have)"*. That is FALSE at HEAD: `node v22.22.2`, `npm 10.9.7` and `bun 1.3.11`
> are installed (`/opt/node22/bin/node`, `/root/.bun/bin/bun`). It lifts nothing, and saying so
> precisely is the point — `generate-models.ts:1431` is `await fetch("https://models.dev/api.json")`,
> and models.dev is an egress-POLICY denial from this workspace (`CONNECT tunnel failed, response
> 403`, re-measured 2026-09-05, and per `/root/.ccr/README.md` a 403 is a policy denial not to be
> retried or routed around). So the block is **the network one alone**; a future attempt should not
> spend effort on a toolchain that is already here.
>
> **What is left is exactly the four catalog FILES — the offline FLOOR, nothing else.** All four
> providers are live: auth resolves, requests stream, and rows arrive through the pi.dev overlay
> (`remote_catalog.rs`) and `models.json`. What they lack is embedded rows for an `--offline` run
> and for the window before the overlay's first fetch. `xtask/src/main.rs::UNPORTED` now says so in
> those terms, and `gen-catalogs --roster v0.84.4` is unchanged and still green — *"39 provider
> module(s) upstream — 34 embedded, 5 accounted for as unported"* — and still hard-fails the day
> one of the four parses as a data literal again.
>
> **3. Adjacent, settled but NOT this item's to fix — `ThinkingFormat::Qwen` is missing upstream's
> `reasoning_effort` half, and it is a PORT OMISSION, not drift.** The 2026-09-05 pass left the
> classification open; the batch-4 review settled it and it is recorded here so the next pass does
> not re-derive it. `openai-completions.ts:753-760` @**v0.83.0** — the ported baseline — already
> carries `if (options?.reasoningEffort && compat.supportsReasoningEffort) { const effort =
> model.thinkingLevelMap?.[options.reasoningEffort] ?? options.reasoningEffort; if (typeof effort
> === "string") params.reasoning_effort = effort; }` inside the `qwen` arm, identical to `:872-877`
> @v0.84.4, while `api/openai_completions/reasoning.rs:50-52` inserts only `enable_thinking`. So
> nothing drifted upstream between the two tags — the half was never ported. Live-relevant: the
> qwen-token-plan rows set `reasoning_effort` on their `deepseek-v4-*` / `glm-5*` models
> (`generate-models.ts:306-316`). Pre-existing, outside this item's four-catalog scope, and it wants
> an area-01 compat item with an owner.

**cyrup** — `cyrup/crates/cyrup-provider/src/tests/catalog_data.rs:5-8` still declares the provenance as "31 embedded catalogs … byte-faithful snapshot of pi @ `5c1a2977`" and still names `packages/ai/src/providers/*.models.ts` as the source. The tree now ships **35** catalogs (`ls crates/cyrup-provider/src/providers/catalog/ | wc -l` → 35), so the doc's own count is stale by four. `cyrup-provider/src/catalog.rs:1-12` confirms the embedded set is still "the source of truth and the floor". *(The provenance-comment half of this is `PROV-039`; this item is scoped to the four missing catalogs.)*

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

*(Status of the two contributions, 2026-09-05: **(2) is done** — `xtask/src/main.rs::canonicalize`
/ `diff_catalog` port `diff-model-catalog.mjs`'s comparison and union walk. **(1)'s registration
half is done** — all four are registered providers (`baseten` on 2026-09-05, the Qwen trio under
`PROV-014`), declared in `UNPORTED` with their reasons and audited by `gen-catalogs --roster <rev>`,
which also re-tests the block. What remains is only the four catalog FILES, i.e. the offline floor.
Producing them needs a models.dev fetch, which is an egress-policy 403 from this workspace; pi's
generator would not help even though `node`/`npm`/`bun` ARE installed here, because
`generate-models.ts:1431` fetches that same host. Do not seed from the pi.dev artifact.)*

**Verify** — Per `PROV-018`: `cargo xtask gen-catalogs` against a named pi tag reproduces the current tree byte-for-byte, and the ignored drift test fails when pointed at a newer pi. This item adds: after the run, `ls crates/cyrup-provider/src/providers/catalog/ | wc -l` is 39 and each of the four new files is non-empty and registered in `providers/all.rs`. *(The second clause — "registered in `providers/all.rs`" — is satisfied ahead of the first as of 2026-09-05: all four are registered, `baseten` last. What the count clause waits on is the files.)* **Until then** (2026-09-04): `cargo run -p xtask -- gen-catalogs --pi tmp/pi --roster v0.84.4` exits 0 and reports 39 upstream / 34 embedded / 5 unported, and would exit 1 the moment pi ships a fortieth provider or re-commits any of the four modules as a data literal; `cargo nextest run -p xtask` covers the accounting (`the_v0_84_4_provider_roster_is_fully_accounted_for`, `an_unaccounted_upstream_module_is_a_hard_error`, `an_embedded_catalog_pi_no_longer_ships_is_a_hard_error`) and the ported comparison (`key_order_alone_is_not_a_field_difference`, `thinking_level_maps_canonicalize_in_ladder_order`, `array_entries_canonicalize_without_the_parent_key`, `diff_catalog_walks_the_sorted_union`).

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

## ~~DRIFT-022~~ — TUI mode (`--tui-mode`, alternate screen) not ported — **CLOSED 2026-09-04**

**Kind** tracking · **Severity** ~~**tracker**~~ **CLOSED 2026-09-04** · **Effort** L · **Confidence** high · **duplicate-of: `SEAM-051`**

> **CLOSED 2026-09-04 (area-12 pass, cyrup HEAD `2571969`).** The renderer half this tracker was
> tracking — everything below this line was still true as of the 2026-08-15 pass — has landed:
> `dbcf59a` ("ship the fullscreen (alternate-screen) TUI mode, ADR-0005 B-1..B-14") wires
> `--tui-mode` (`crates/cyrup/src/cli/args.rs:184-185`, replacing the `crates/cyrup/src/cli.rs` this
> body cites — the CLI moved into a `cli/` module tree) through `App::switch_tui_mode` at
> `crates/cyrup/src/interactive.rs:207-217`, with the precedence this row's own Fix specified
> (flag → `tuiMode` setting → `regular`). The `tuiMode`/`fullscreenScrollbar` settings keys land at
> `cyrup-config/src/settings/manager.rs:387,395`. The commit's own log records the binary driven
> under a real pty — alt-screen enter/leave escapes, SGR wheel reports, tmux-aware mouse-capture
> divergence observed and matched against upstream — so this is an `observed`, not merely static,
> closure. Upstream re-confirmed unchanged in shape at `v0.84.4`. **Not verified this pass: line-for-
> line fidelity against pi's now-1378-line `tui-alt-screen.ts`** (grown further since the `v0.84.1`
> figure below) — any residual fine-grained gap in the renderer's *behaviour* is `TUI-019`'s to find,
> not this tracker's; this closure speaks only to "the surface this row tracked now exists and is
> wired end to end", which is all a tracker of this shape ever asked.

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
> (`add_usage_totals`, `cyrup-tui/src/status.rs:168`, called from `app/events_fold.rs:96` and `status.rs:153`),
> so only the breakdown is missing, and it names pi's `usageBreakdown.length > 1` render gate. Work
> it as `PROV-036`.

**cyrup** — `grep -rnE 'cost_breakdown|CostBreakdown|usage_cost' crates --include='*.rs'` → 0. `crates/cyrup-tui/src/app/execute_session.rs:164`/`:177` render a single `| cost | ${:.3} |` row from `stats.cost`; there are no per-model rows and no `Tools/summaries` bucket. *(The prior filing's "No cost figure" is an overstatement — a single total is rendered; what is missing is the breakdown.)*

**upstream** — `pi/packages/coding-agent/src/core/usage-totals.ts:37` @**`v0.83.0`** `export function getUsageCostBreakdown(entries: SessionEntry[]): UsageCostBreakdownEntry[]`, with `interface UsageCostBreakdownEntry` at `:30-36`; imported at `modes/interactive/interactive-mode.ts:94` and consumed at `:5665` — **live at the ported baseline**, not dead code. At `v0.84.1` the declaration is at the same `:37`; only the consumer offsets shift (import `:101`, call `:6001`). Keyed `${provider}/${responseModel ?? model}`, with a bucket literally named `Tools/summaries` absorbing toolResult, branch-summary and compaction usage so the breakdown reconciles with the session total.

**Impact** — Users cannot see which model or which tool/summary traffic drove a session's cost — only the single total.

**Fix** — Work it as `PROV-036`. DRIFT-011 (closed) already supplies the data (`usage` on tool results, compaction and branch summaries). Port the breakdown half beside the existing totals code, keyed on `provider/response_model.unwrap_or(model)`, reproducing the `Tools/summaries` bucket **by name and by membership**, and render it in `cyrup-tui/src/app/execute_session.rs:147-213` under pi's `len() > 1` guard.

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

**cyrup** — `cyrup/crates/cyrup-session-svc/src/tests/summarization_retry_events.rs:98-104`: `async fn settle()` is verbatim `for _ in 0..10 { tokio::task::yield_now().await; }` then `tokio::time::sleep(Duration::from_millis(50)).await;`, while events are drained by a separately spawned collector task. The correct poll-until-observed pattern already exists in the same file.

**upstream** — pi's equivalent suites synchronize on observed state, not elapsed time — the rendezvous pattern at `pi/packages/agent/test/agent-loop.test.ts:589-612`.

**Impact** — Under load the drain may not have happened when assertions run. The **negative** assertion among the call sites is the worse half: it passes vacuously whenever the drain has not happened, so it proves nothing while looking green.

**Fix** — Replace `settle()` with the poll-until-observed helper already present in the file, bounded by a generous timeout; convert the negative assertion into "poll until the positive precondition holds, then assert the absence".

**Verify** — Both variants pass with the sleep removed entirely and under a single-worker runtime; the negative case must fail if the forbidden event is emitted. Fix in one pass with DRIFT-039 via a shared poll helper.

## DRIFT-039 — TEST DEFECT: `a_02_2_parallel_completion_vs_source_order` asserts a wall-clock bound and a completion ORDER it cannot control

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high · **duplicate-of: `AGENT-019`** — *literally the same test*, `crates/cyrup-agent/src/tests/agent_loop.rs:327`. Fix it once. This body carries the better fix sketch (the `agent-loop.test.ts:589-612` rendezvous); fold that into `AGENT-019` rather than working both, and deduplicate the pair before the test-defect sweep (`00-residual-ledger.md` cluster F3) so it is not booked twice.

**cyrup** — `cyrup/crates/cyrup-agent/src/tests/agent_loop.rs:327`: `assert!(elapsed < Duration::from_millis(115), …)`. `Instant::now()` is taken at `:277` **before** `agent.prompt` and read at `:281` **after** `wait_for_idle()`, so the budget also covers the faux stream and idle settling, not just the two tool sleeps (80 ms and 50 ms) — concurrent floor ≥ 80 ms, remaining margin under 35 ms, on a 4-worker multi-thread runtime in a suite `cargo test` runs many-at-once. `:301` additionally asserts `ends == vec!["fast", "slow"]` and `:302` `assert_ne!(ends, starts)`, requiring the 50 ms tool's `ToolExecutionEnd` to be observed before the 80 ms one — a 30 ms scheduling margin. All three unchanged at HEAD.

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

> **CLOSED 2026-08-15. Everything below is the filing text and is false at HEAD** — re-verified
> 2026-08-19 while re-pointing this section's `app.rs` citations. The text branch exists:
> `App::paste_from_clipboard` (`crates/cyrup-tui/src/app/shell.rs:330-349`) takes the image read and
> the text read as CLOSURES so pi's lazy ordering — text is never read when an image was found
> (`interactive-mode.ts:2882`/`:2884`) — is a unit-testable fact, inserts the text at `:344-346`, and
> names this item at `:341-343`. `App::try_paste_clipboard_image_path` (`:356-358`) supplies
> `crate::clipboard::read_clipboard_text` (`clipboard.rs:334`) as that second closure, so the
> "`grep read_clipboard_text` → 0" measurement below no longer holds.

**cyrup** — `cyrup/crates/cyrup-tui/src/app/input.rs:57-61`: `Action::ClipboardPasteImage` (bound to Ctrl+V) calls `try_paste_clipboard_image_path()` and, when it returns `false`, deliberately "falls through to the editor (text) handling below" (`:61`). The editor has no system-clipboard **read** — the only clipboard entry points in the workspace are `copy_to_clipboard` (`crates/cyrup-tui/src/clipboard.rs:212`, write-only) and `read_clipboard_image_to_temp` (`app/event_extract.rs:105`, images only); `grep -rnE 'read_clipboard_text|wl-paste|WAYLAND' crates --include='*.rs'` → 0. So a Ctrl+V whose clipboard holds text reaches the editor as a bare key event and inserts nothing. The comment at `app/input.rs:54-56` asserts the fall-through preserves "normal Ctrl+V behavior", which is true only for terminals that themselves map Ctrl+V to a bracketed paste — most do not. **cyrup's own help table advertises the behaviour**: `app/hotkeys.rs:56` describes `{paste_image}` as "Paste image or text from clipboard".

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2845-2868` `handleClipboardPaste` tries `readClipboardImage()` first (`:2847`) and, only when that yields nothing, does `const text = await readClipboardText(); if (text) this.editor.insertTextAtCursor?.(text)` (`:2860-2864`) — the text branch is the explicit fallback, not the terminal's job. `readClipboardText` is at `pi/packages/coding-agent/src/utils/clipboard.ts:53-80` and **gained a Wayland branch in this delta** (pi `bfc679d5e`): `:39-46` `readWaylandClipboardText` shells `wl-paste --no-newline --type text` with a 50 MB buffer and 5 s timeout, gated at `:54` on `platform() === "linux" && isWaylandSession() && process.env.WAYLAND_DISPLAY`, falling back to the native module otherwise. The same function also backs right-click paste at `interactive-mode.ts:2836`.

**Impact** — Ctrl+V is dead for text. Users who paste a URL, an error message or a code snippet get silence and no diagnostic — against a help table that says it should work. On Wayland specifically, even a terminal-mediated paste can fail because the native clipboard module is X11-oriented, which is exactly why upstream added the `wl-paste` branch in this release. *(Note cyrup already shells `wl-copy` in `copy_to_clipboard`, so the asymmetry is one-directional.)*

**Fix** — Add `read_clipboard_text()` beside `read_clipboard_image_to_temp` in `cyrup-tui` (or a small `clipboard.rs`), porting `clipboard.ts:39-46` + `:53-80`: on Linux with `WAYLAND_DISPLAY` set, shell `wl-paste --no-newline --type text` with the same 5 s timeout and treat any error as "fall through"; otherwise use the native backend cyrup already links for images. Then in `app/input.rs:57-61`, when `try_paste_clipboard_image_path()` returns `false`, call it and `self.state.editor.insert_str(&text)` before falling through — matching `interactive-mode.ts:2860-2864`'s ordering exactly (image first, text second).

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

## ~~DRIFT-053~~ — ~~medium~~ **CLOSED 2026-09-05** — `/share` gained a Radius-upload-first path in `v0.84.3`; cyrup still only knows the old gist-only flow

**Kind** upstream-drift · **Severity** ~~medium~~ **CLOSED** · **Effort** M · **Confidence** high

> **CLOSED 2026-09-05** (batch-3 pass; code `9de4254b`, `crates/cyrup-tui`, `crates/cyrup-provider`,
> `crates/cyrup-config`). Re-derived two-sided at `v0.84.4` before any code was written.
>
> **The blocker is gone, and that is the first thing this pass established.** The Fix below ends
> *"do not attempt this before `DRIFT-019`/`PROV-014` registers a `radius` provider"*. At this HEAD
> it is registered: `crates/cyrup-provider/src/providers/radius.rs` is the real provider **kind**
> (`pi-messages`, env-key **or** OAuth, dynamic catalog off `GET {gateway}/v1/config`) and
> `providers/all.rs:199-202` constructs it, so `get_provider("radius")` resolves and the
> credential the upload needs is resolvable through the ordinary request-auth path.
>
> **What landed.** `share_session` (`crates/cyrup-tui/src/app/execute_misc.rs`) is now pi's
> restructured `shareSession` (`session-share.ts:46-89`): one JSONL export, then
> `try_share_via_radius` (`:57`), then — only on `false` — the pre-existing `gh auth status`
> (`:59-68`) → `exportToHtml` (`:70-76`) → `shareViaGist` (`:152-203`) chain, unchanged.
> `try_share_via_radius` is `tryShareViaRadius` (`:91-150`) and its RETURN VALUE is the contract:
> `false` — go publish a gist — for exactly the two credential-shaped conditions upstream returns it
> for (`:93` no provider, `:98` no credential), and `true` for every outcome after the request is
> sent. A 403 from the gateway therefore ends `/share` with an error; it does **not** fall through
> and publish the gist the Radius configuration exists to avoid. The transport and the reply
> classification are a new `crates/cyrup-provider/src/providers/radius_share.rs`, beside
> `DEFAULT_RADIUS_GATEWAY` — the same placement upstream uses (`packages/ai`, imported at
> `session-share.ts:6`). `exportSessionForShare`'s `pi.share` trailing entry (`:25-43`, over
> `core/session-export.ts:21`/`:31-36`) is ported as `app/share.rs::append_share_metadata`, so this
> item's own "if the Radius viewer needs it parsed" caveat is settled in the affirmative: `parentId`
> is the last exported entry's id and `timestamp` is the **session header's**, both recovered from
> the serialized document so the appended entry cannot disagree with it.
>
> **Corrections to this row's own evidence,** found by reading the file rather than the row:
> `shareSession` is `:46-89` (not `:46-70`), `tryShareViaRadius` `:91-150` (not `:72-101`),
> `shareViaGist` `:152-203` (not `:103-140`), and the URL carries a THIRD search param the row does
> not mention — `title` (`:114`). The file is **byte-identical at `v0.84.3` and `v0.84.4`**
> (`git -C tmp/pi diff v0.84.3 v0.84.4 -- packages/coding-agent/src/modes/interactive/session-share.ts`
> is empty), so the ADR-0006 latest-tag target and the row's `v0.84.3` attribution agree.
>
> **Two supporting changes, each doing only what this item needs.**
> (1) `impl cyrup_provider::CredentialStore for cyrup_config::AuthStore` — the impl
> `cyrup-config/src/auth.rs`'s own header has claimed since it was written, and which **did not
> exist**: the trait's only implementation in the workspace was `InMemoryCredentialStore`, which is
> why `crates/cyrup/src/credential_print.rs` seeds one from `auth.json` and throws it away. Fine for
> a read-only print, wrong here: `resolve_provider_auth` rotates an expiring OAuth token by calling
> `modify` on the store it was handed (`ai/src/auth/resolve.ts:117-149`), and writing the rotated
> refresh token into a discarded map would leave `auth.json` holding a token the provider has
> already invalidated. (2) `auth_credential` — pi `getAuthCredential`
> (`cli/auth-command.ts:120-125`) — in `cyrup-provider/src/auth/helpers.rs`, with both JS falsiness
> rules preserved.
>
> **Design decisions** (batch-3 guidance). `RadiusShareOutcome { Shared, Failed }` and
> `ShareUpload { Gist, Radius }` are explicit domain enums rather than `Result<String, String>` /
> an `Output` plus a nullable field. The invariant: a non-2xx gateway reply is an **ordinary,
> reported** outcome of `/share`, not a technical failure to propagate — precisely the distinction
> the Verify line turns on. What becomes impossible: a `?` that silently swallows a reported error,
> and adding a third upload path without saying what the user is told (`apply_share_outcome` matches
> exhaustively). Still runtime-checked: which arm a given HTTP reply lands in — five tests.
> Rejected: `Result<String, String>` (invites `?`), a bool + message pair (makes "failed with no
> message" representable). Migration cost: `ShareMsg`'s two fields became one; `ShareMsg::new`
> keeps its public signature for existing callers and `ShareMsg::radius` was added. Functional
> core / imperative shell for the reply handling — `classify_artifact_response` is pure over
> (status, statusText, body), so every branch's wording is asserted without a socket. **Not**
> taken: typestate for the share lifecycle (its states are driven by external events and already
> live as `App` runtime state; typestate would buy nothing over the enum).
>
> **`[CYRUP-DELTA]`s, all recorded in-source.** No temp file on the Radius path (the export is
> POSTed from memory; pi writes one at `:47`/`:52` only because `exportSessionToJsonl` is a file
> writer, and its `finally` at `:79-86` already tolerates a null path — which is why
> `ShareInFlight::tmp` became an `Option`). A 200 whose `artifact` object carries no
> `canonical_url` is reported as a failure, where upstream's truthy `!json?.artifact` would render
> `Share URL: undefined`. A 15 s upload timeout upstream does not set, matched to `radius.rs`'s
> config-fetch ceiling. The artifact title is rebranded `"Cyrup session"` (pi: `"Pi session"`),
> while `customType: "pi.share"` and `DEFAULT_SHARE_VIEWER_URL` are deliberately **not** rebranded —
> both are matched by pi-operated services.
>
> **Proof.** `cyrup-tui tests::share_radius_route` drives the two Verify lines against a real
> `AgentSession` over a seeded `auth.json`: with a radius credential `/share` reports the Radius
> upload's own failure sentence and **never reaches `gh`** (no `GitHub CLI`, no `Gist:`); without
> one, no Radius message appears at all. No socket leaves the machine — the gateway is redirected to
> a closed local port through `App::set_radius_share_gateway`, the sibling of
> `set_login_provider_source` and mandated by the same "tests must never hit real provider APIs"
> convention. **Red established behaviourally, not merely by symbol:** short-circuiting the ported
> call site to `if false && self.try_share_via_radius(..)` fails the first test and leaves the
> second passing. Also `tests::share_url::radius_share_tests` (entry type / parentId / header
> timestamp / tool shape, the null-parent case, the three reported outcomes) and
> `providers::radius_share::tests` (URL with both params, the 2xx-without-an-artifact arm, pi's
> three-tier failure detail with JS falsiness, an unparseable body, the credential gate both ways).
>
> **RESIDUALS (new, low, none of them `/share`'s).** (a) cyrup's `ApiKeyAuth::resolve` carries a
> `&Model` pi's does not — `ai/src/auth/types.ts:190-193` states resolution is provider-scoped —
> and radius's catalog is dynamic, so `radius_share_token` builds a provider-scoped `Model`
> describing the gateway rather than a placeholder; the trait divergence is what forces it and
> belongs to whoever next touches `cyrup-provider`'s auth surface. (b) The share URL is printed
> unwrapped: pi's `hyperlink()` (`:139`) needs the paint-time OSC-8 emitter that is `TUI-020`'s
> (area 07), the same residual `login_dialog.rs` records for `DRIFT-042` half (b). (c) The
> `"Uploading to Radius..."` loader is mounted and cancellable, but no pty-driven observation of it
> was made this pass — the cancel path is asserted at the message level
> (`a_cancelled_radius_upload_prints_nothing`), not on screen.

*(Filed 2026-09-04, area-12 pass, from the v0.84.2→v0.84.4 diff-stat skim. New relative to this
file's own baseline: `packages/coding-agent/src/modes/interactive/session-share.ts` does not exist
at `v0.84.1` or `v0.84.2` — `git -C tmp/pi cat-file -e <tag>:packages/coding-agent/src/modes/
interactive/session-share.ts` fails at both — and first appears at `v0.84.3`, introduced by pi
`460191cfc` "feat(coding-agent): include context in Radius session shares". Present unchanged at
`v0.84.4`.)*

**cyrup** — `crates/cyrup-tui/src/app/execute_misc.rs::share_session` (`:898`) ports exactly the
**pre-`v0.84.3`** `/share`: render the session to HTML (`crate::export::session_jsonl_to_html`),
write it to a temp file, gate on `gh auth status` before mounting a loader
(`crates/cyrup-tui/src/app/execute_misc.rs`, citing `session-share.ts:59-68` in its own comment —
which is evidence a prior pass *did* open the new file, since that citation only makes sense against
the post-restructure line numbers), then shell `gh gist create --public=false <file>` and surface
the URL via `apply_share_outcome` (`:1016`) and `crate::app::share` (`ENV_SHARE_VIEWER_URL`,
`crates/cyrup-tui/src/app/share.rs:7`). There is no Radius attempt anywhere in the path:
`grep -rn '"radius"' crates/cyrup-tui/src --include='*.rs'` → 0, and cyrup has no `radius` provider
registered at all (`DRIFT-019`/`PROV-014`), so there is nothing for a ported `tryShareViaRadius` to
call `get_provider("radius")` against yet regardless.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/session-share.ts` @`v0.84.4`.
`shareSession` (`:46-70`) now tries Radius **first**: `exportSessionForShare` writes a JSONL
augmented with a `pi.share`-typed trailing entry carrying the system prompt and tool list
(`:25-44`), then `tryShareViaRadius` (`:72-101`) resolves the `radius` provider off
`session.modelRuntime.getProvider("radius")`, fetches an OAuth credential
(`getAuthCredential(...modelRuntime.getAuth("radius", …))`), and on success `POST`s the JSONL as
`application/x-ndjson` to `${DEFAULT_RADIUS_GATEWAY}/v1/artifacts?visibility=organization` — landing
the share as an **organization-visibility Radius artifact**, not a public link, and returning early
(`:70`, `return true` whether it succeeded or the upload failed — the caller does not fall through on
a failed Radius attempt with credentials present, only on "no provider" / "no credential"). Only when
`tryShareViaRadius` returns `false` (no radius provider or no stored credential) does `shareSession`
fall to the old `gh auth status` → `session.exportToHtml` → `shareViaGist` chain (`:56-58`,
`:103-140` for `shareViaGist`), unchanged from `v0.84.1`'s `handleShareCommand`.

**Impact** — Two distinct gaps, gated by whether radius auth exists. (1) **Structural, true today**:
cyrup's `/share` can never take the Radius path even in principle — it isn't threaded through the
command at all — so the code path pi added for exactly this feature has no analogue to fall short of
yet; this is closer to `not-ported` in spirit but the *trigger* (the restructuring) is genuine
post-baseline drift, which is why it is filed `upstream-drift` here rather than folded into
`DRIFT-019`. (2) **Behavioural, once `DRIFT-019`/`PROV-014` lands radius**: even after radius is
registered, cyrup's `/share` would still go straight to the gist fallback unless this item's own fix
also lands, meaning a user who deliberately configured Radius so their sessions stay inside their
organization would instead get every `/share` published as a GitHub gist — a materially different,
more-exposed sharing destination than the one they configured, and silently so (no error, no
indication the intended path was skipped).

**Fix** — Port `tryShareViaRadius` into `share_session` (or a sibling `try_share_via_radius`) ahead
of the existing gist path: resolve the `radius` provider and credential through the same
`AgentSession`/`ModelRuntime`-equivalent surface cyrup's other auth flows use, `POST` the JSONL
export (already available via `session.export_to_jsonl`) to `{gateway}/v1/artifacts?visibility=
organization` as `application/x-ndjson`, and only fall through to the existing `gh`/gist chain when
no radius provider or credential is present — matching `session-share.ts:56-58`'s early-return
shape. `exportSessionForShare`'s `pi.share`-typed trailing entry (system prompt + tool list) is a
small addition to `session_jsonl_to_html`'s existing serializer if the Radius viewer needs it
parsed; **do not attempt this before `DRIFT-019`/`PROV-014` registers a `radius` provider** — there
is nothing to resolve a credential against otherwise.

**Verify** — With a radius credential present, `/share` POSTs to the artifacts endpoint and surfaces
the returned `canonical_url`, never invoking `gh`. With no radius provider/credential, behaviour is
byte-identical to today (gist path, unchanged). A failed Radius upload (non-2xx or network error)
surfaces an error and does **not** fall through to gist — matching pi's `return true` on failure,
which this item's Fix must not silently "improve" into a fallback pi itself does not take.

---

## ~~DRIFT-054~~ — ~~medium~~ **CLOSED 2026-09-05** — The templated HTML export dropped `systemPrompt` and `tools`, losing two document sections on every live export

**Filed** 2026-09-05 by the batch-3 ledger pass, against code batch 3 had just landed
(`ce81dba9` + `824a539e`). **Closed** 2026-09-05 by batch 4 at `16b38c93`, under that batch's rule
that a defect found in the CURRENT batch's own code is a bug to fix, not a row to file — this row
is the worked example, since it was open only because batch 3 wrote it down and merged.

**Upstream, re-read at the tag (not taken from the row).** `packages/coding-agent/src/core/export-html/index.ts` @`v0.84.4`:

* `:130-138` — `interface SessionData { header; entries; leafId; systemPrompt?; tools?: Array<Pick<ToolDefinition, "name" | "description" | "parameters">>; renderedTools? }`.
* `:263-270` — `exportSessionToHtml(sm, state, options)` builds
  `{ header: sm.getHeader(), entries, leafId: sm.getLeafId(), systemPrompt: state?.systemPrompt, tools: state?.tools?.map((t) => ({ name: t.name, description: t.description, parameters: t.parameters })), renderedTools }`.
  Both agent keys are gated on the SAME `state?`.
* `:298-304` — `exportFromFile(inputPath, options)` sets `systemPrompt: undefined, tools: undefined`. This is the FILE path.
* `core/agent-session.ts:3439` — `return await exportSessionToHtml(this.sessionManager, this.state, {…})`. `exportToHtml` ALWAYS passes `this.state`, and it is the only entry point the three front-end paths have: `/export` `modes/interactive/interactive-mode.ts:6023`, `/share` `modes/interactive/session-share.ts:72`, RPC `export_html` `modes/rpc/rpc-mode.ts:601` (all three verified by `git show v0.84.4:<path> | grep -n exportToHtml`).
* `packages/agent/src/types.ts:334-345` — `AgentState.systemPrompt: string` and `tools` are both required, so `state?.tools?.map(...)` yields `[]`, never `undefined`, once `state` exists.
* The shipped `template.js` (byte-identical to upstream's, digest-pinned by
  `embedded_assets_are_byte_identical_to_pi_v0_84_4`) destructures both at `:15` and renders a
  collapsible **System Prompt** block at `:1403-1424` and an **Available Tools** list at
  `:1425-1452`, expanding each tool's `parameters.properties` with type, `required`/`optional` and
  description (`:1430`).

**cyrup, before the fix.** `crates/cyrup-session-svc/src/export/mod.rs::session_data` inserted
`header`, `entries` and `leafId` and nothing else; its doc defended the omission by citing
`exportFromFile`. Every cyrup caller is a LIVE path, so the two sections were missing from every
`/export`, `/share` and RPC `export_html` document — silently, since `template.js`'s readers are
`?.`-guarded and simply render nothing.

**What landed** (`16b38c93`, `cyrup-session-svc` + `cyrup-tui`):

1. `ExportTool { name, description, parameters }` — pi's `Pick<…>` exactly, three fields and no more.
2. `ExportState` — the three keys the JSONL cannot supply, private fields behind two constructors:
   `live(leaf_id, system_prompt, tools)` (pi's `exportSessionToHtml` shape) and `from_file()` (pi's
   `exportFromFile` shape). `Option<Vec<ExportTool>>` rather than `Vec` because `undefined` and
   `[]` are different payloads upstream, even though `template.js:1425`'s
   `tools && tools.length > 0` renders them the same.
3. `AgentSession::export_state()` — the imperative shell, beside the existing `export_theme()` and
   `export_leaf_id()`. Reads `current_system_prompt()` (`override ?? base`, pi's
   `agent.state.systemPrompt`) and `agent.state.tools` — the ACTIVE set the model was given this
   turn, NOT `all_tools()`, which also lists the toggled-off ones.
4. `session_jsonl_to_html_with_theme`'s third parameter becomes `&ExportState`; the three live call
   sites pass `session.export_state().await`. `session_jsonl_to_html(&str)` is unchanged, so
   `cyrup --export` and `cyrup-tui`'s re-export needed no edit.

**Design decision.** The invariant encoded is pi's single `state?` gate plus "a live export carries
all three shell-owned keys"; it becomes known in the shell, and `AgentSession::export_state()` is
the only way to build a live value. What becomes impossible: passing the leaf and forgetting the
prompt — which is literally how these keys went missing when the leaf was threaded alone as its own
`Option<&str>` — and a `systemPrompt` with no `tools`. What still needs runtime tests: that the
values are the RIGHT ones (effective prompt, active tool set, manager's leaf), covered by the two
source guards. Rejected: two more positional parameters (five arguments, two `Option`s, and the
same trap left open for the next caller); a nested `Option<ExportAgentState>` mirroring `state?`
literally (the private constructors already enforce the joint gating without a second public type);
reading the two keys inside the renderer (pure, holds no session); a builder (one call-site shape).
Migration cost: three production call sites, six test call sites, one added `pub use`, no serde.

**Tests, with the RED measured rather than asserted.**
`crates/cyrup-session-svc/src/tests/export_html.rs`:
`a_live_export_carries_the_system_prompt_and_the_active_tools`,
`the_file_only_export_omits_both_agent_keys_the_way_pi_does`,
`an_empty_live_tool_set_is_an_empty_array_not_an_absent_key`,
`export_to_html_passes_the_live_session_state_to_the_renderer` (renamed from the leaf-only guard);
`crates/cyrup-tui/src/tests/export.rs::both_tui_export_sites_pass_the_live_session_state` (likewise).
Stripping the two `map.insert` arms out of `session_data` failed the first and third
(`left: Null, right: "You are cyrup.\nBe brief."`); pointing `export_to_html` at
the file-only shape failed the session-svc guard; pointing both TUI sites at it failed the TUI
guard. Green after: `nextest -p cyrup-session-svc` 353/353, `-p cyrup-tui` 1415/1415 (1 skipped);
`clippy --all-targets -- -D warnings` and `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps` clean on
both crates; `cargo check -p cyrup-modes --all-targets` and `-p cyrup --all-targets` for the
downstream consumers of the changed surface.

**Corrections this closure carries.** `DRIFT-041`'s row asserted the export was complete when it
was two keys short — corrected in that row, which now also records that both in-source claims it
flagged (`session/transcript.rs::export_to_html`'s "Its one residual is `renderedTools`" and
`export/mod.rs`'s `SessionData` doc) were fixed in the same commit. And the citation
`agent-session.ts:3438`, used by this row and by `DRIFT-041`'s, is `:3439`. **That claim was
only half true when it was written (`bbce4652`), and the batch-4 regression gate caught it:** the five
source sites in `crates/` were genuinely corrected at `16b38c93` (`rg 'agent-session.ts:3438' crates/`
returns nothing), but three present-tense assertions in THIS file still carried `:3438` — the `DRIFT-041`
summary row, which cited both `:3438` and `:3439` for the same call in one sentence, the `DRIFT-041`
detail block, and this row's own detail. All three are corrected by the tenth-edition ledger pass
(2026-09-05). Verified once more at the tag: `git -C tmp/pi grep -n 'exportSessionToHtml(this.sessionManager'
v0.84.4` → `packages/coding-agent/src/core/agent-session.ts:3439`. The two remaining `:3438` strings in this
file are quotations of the superseded value inside correction sentences, this one included, and are meant
to stay.

**Residual.** `renderedTools` / `ToolHtmlRenderer` / `preRenderCustomTools` (`index.ts:15-33`,
`:177-230`, plus `tool-renderer.ts` and `ansi-to-html.ts`) remains unported and is unchanged by
this item — `template.js:1026` falls back to its own rendering. **RESTATED at `e8f93355` (batch-4
review fix):** the clause that used to follow, "exactly as it does for pi's own `exportFromFile`,
which passes no renderer either", excused a LIVE-path gap with the FILE entry point — the defence
this item exists to have removed. pi's live `exportToHtml` builds `createToolHtmlRenderer(…)`
unconditionally (`agent-session.ts:3433-3437`) and passes it (`:3439-3443`), and the map is
pre-rendered into the payload (`index.ts:254-261`, `:269`); only `exportFromFile` (`:288-316`)
passes none. cyrup's three front-end paths are all live, so a custom-rendered EXTENSION tool shows
`template.js`'s built-in card where pi shows the extension's own. Every built-in tool is
unaffected, and `cyrup --export` matches upstream. Low, recorded inside `DRIFT-041`'s row.

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

**Read first-hand at cyrup HEAD `a9000b1`** (working tree clean; last code commit `04c1ba2` — note this file was baselined at `1806375` and the ledger at `8d00f06`/`097bdde`, so cyrup has moved ~15 commits including the batch-1..10 parity/TUI series): `cyrup-core/src/message.rs`; `cyrup-provider/src/{api/{compat,openai_completions,openai_responses,anthropic_messages,google_generative_ai,pi_messages}.rs, utils/retry.rs, remote_catalog.rs, catalog.rs, providers/{all.rs,catalog/}, auth/{store.rs,oauth/}}` and `tests/catalog_data.rs`; `cyrup-session/src/{entry.rs, prompt/{builder,context_files,tests}.rs, compaction/summarize.rs}`; `cyrup-session-svc/src/{session.rs, hooks.rs, export.rs, services.rs, builder.rs, bash.rs}` and `tests/summarization_retry_events.rs`; `cyrup-agent/tests/agent_loop.rs`; `cyrup-config/src/{model.rs, env.rs, policy.rs, settings.rs}`; `cyrup-tools/src/{tools/{bash,grep,find}.rs, ops/local.rs}`; `cyrup-ext/{wit/world.wit, src/{wrapper.rs, facade.rs, host/services.rs}}` and `tests/wit_world_sync.rs`; `cyrup-ext-sdk/wit/world.wit`; `cyrup-modes/src/{rpc.rs, run.rs, json_event.rs}`; `cyrup-tui/src/{login_dialog.rs, status.rs, footer_data.rs, export.rs, app/ (then `app.rs`, split into the module tree by `40821ed`), panic_hook.rs, image.rs, model_selector.rs, markdown/latex.rs}`; `cyrup/src/{main.rs, cli.rs, signals.rs, update_check.rs}`.

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
