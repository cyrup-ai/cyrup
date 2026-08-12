# 03 — cyrup-session (persistence, compaction, prompt)

This area covers `cyrup-session` (JSONL session store, entry model, branch/tree navigation, compaction and branch summarization, system-prompt assembly) plus the parts of `cyrup-core/src/message.rs`, `cyrup-session-svc` and `cyrup-tui` that own session state, stats and the `/tree` flow. It is measured against `pi/packages/coding-agent/src/core/{session-manager,system-prompt,skills,resource-loader,agent-session}.ts` and `pi/packages/coding-agent/src/core/compaction/*` at pi v0.83.0 — note bb301b6 re-aimed compaction at pi's **live coding-agent fork**, so `coding-agent/src/core/compaction/compaction.ts` (not the harness copy) is the oracle for everything below. Headline finding: the compaction engine itself is in genuinely good parity — ten pre-existing items close under adversarial re-verification — but the defects moved outward into the plumbing: token accounting reads the rendered basis where pi reads the raw one, branch summarization is unreachable from the shipped TUI, a duplicated `Compactor` branch-summary path carries three divergences from the live one, and a v1-migration edge silently re-admits an entire compacted history. Re-baselined against HEAD `1806375` on 2026-08-03; every closure below was confirmed by reading code at HEAD and the corresponding upstream function, not by reading a commit message.

## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| SESS-001 | closed | Null/absent `content` tolerated. `cyrup-core/src/message.rs:336/:473/:483` plus the three deserializers at `:581-641` match `sessionEntryToContextMessages` (session-manager.ts:382-394). Serializer unchanged, matching pi's plain `JSON.stringify`. |
| SESS-002 | closed | keep-recent walk no longer type-gates: `cutpoint.rs:133-150` is `estimate_raw_entry(e) == 0 → continue`, identical to compaction.ts:418-438 including the snap-forward loop. |
| SESS-003 | closed | `prompt/skills_inject.rs:26-30` filters `disable_model_invocation` and early-returns on empty, matching `formatSkillsForPrompt` (skills.ts:335-339). |
| SESS-004 | **open** | Unrecognized fields on *known* entry types still dropped on rewrite. |
| SESS-005 | closed | Back-scan breaks on compaction-entry / context-visible / `None` — same conditions in the same order as compaction.ts:439-446. |
| SESS-006 | closed | `compaction/branch.rs:130-134` implements `contextWindow \|\| 128000` then `− reserve`. Upstream is **branch-summarization.ts:312-313** (reserve default `:305`); the `:319-320`/`:313` refs in the old doc — and cyrup's own comments at `branch.rs:117`, `:121`, `mod.rs:360` — are off by seven lines. |
| SESS-007 | **open** | Leading-blank-line session file still fails to open; `load` / `read_header` / `scan_file` still disagree. |
| SESS-008 | closed | `compaction/summarize.rs:239` routes through `retry_assistant_call` — one choke point, as in compaction.ts:562-580. |
| SESS-009 | closed | `summarize.rs:230-231` sets `CacheRetention::None` plus a fresh session id, matching compaction.ts:569-573. Confirms DRIFT-005 is genuinely dead. |
| SESS-010 | closed | `summarize.rs:232` threads `reasoning`, gated at `:279-285` per compaction.ts:539-552. The branch path's `ModelThinkingLevel::Off` is **correct**: pi builds branch options inline at branch-summarization.ts:348 and never sets `reasoning`. |
| SESS-011 | closed | `compaction/prepare.rs:28-48` hands the hook raw `AgentMessage`s via `message_for_compaction`, matching `getMessageFromEntryForCompaction` (compaction.ts:80-85). |
| SESS-012 | **partially closed** | Recording landed (`entry.rs:71/:85`; `combine_usage` at `summarize.rs:145-167` matches compaction.ts:99-122 field-for-field). Accounting did not — twelve destructuring sites elide `usage` with `..`; zero readers. Remaining half tracked here plus SESS-021 and SESS-030. |
| SESS-013 | **open** | No `findShadowedContextFile`; AGENTS.md double-load in a nested linked worktree. |
| SESS-014 | **open**, reclassified | Whole-file reads for header discovery. Kind changed `stale-port` → `upstream-drift` and severity → low: pi's buffered reader landed in `f1c587dd` (2026-07-19), *after* cyrup's 2026-07-10 baseline. |
| SESS-015 | **open** | Unresolvable `firstKeptEntryIndex` demotes the compaction entry to `Unknown` and re-admits the whole compacted history. |
| SESS-016 | **open** | Explicitly-empty `selected_tools` still emits the skills section and the tool guidelines. |
| SESS-017 | **open** | `Compactor::run_branch_summary` records `fromId` as the old leaf. |
| SESS-018 | **open** | `custom_message` null content renders `"null"`; missing `display` drops the entry. |
| SESS-019 | **open, misdescribed** | Re-characterized. The `Current date` line is a **faithful port**, not a cyrup invention: pi removed it in `f4e9ca74` (2026-07-14), after cyrup's baseline. Kind is `upstream-drift`. Two real deltas survive: an extra leading `\n`, and the `project_context` wording. The `prompt/tests.rs:82`/`:113` assertions are **not** test-defects. |
| SESS-020 | closed | `TokenCache` keyed `(EntryId, EstimateKind)` at `compaction/tokens.rs:193-260`; both projections cached independently. Prerequisite for SESS-002, correctly landed. |
| SESS-021 | **open** | `SessionStats` misses abandoned branches, summarization usage, `toolCalls` and `cost`. |
| SESS-022 | **open** | `run_branch_summary` summarizes even when the user declined, because `skip_prompt` is consulted in the core. |
| SESS-023 | **open** | `/tree` always passes `summarize: false`; the whole branch-summary stack is dead in the shipped binary. |
| SESS-024 | **open** | Skills preamble drops pi's relative-path resolution instruction (skills.ts:345). |
| SESS-025 | **open** | `reserve_tokens` doc and two test names still assert the pre-SESS-006 rule. |
| SESS-026 | **open** | `serialize_conversation` drops `[Assistant]: ` when the joined text is empty; pi guards on block presence. |
| SESS-027 | **open** | Three `cyrup-core` content-deserializer docs promise per-role validation their bodies deliberately removed. |

Ten items closed. Seven are new this pass: SESS-028 … SESS-034.

## Open items

> **⚠ THIS TABLE IS NOT THE COMPLETE OPEN SET.** 6 further items from the 2026-08-03
> surface-driven sweep live in their own table under `## Surface-sweep findings` (line ~428), with
> `-S` ids — **including 0 rated critical/high**. Enumerating only this table undercounts the
> area by 6 items, which is exactly how `SEAM-S01` (high) escaped a full audit pass on
> 2026-08-07. Count BOTH tables. See structural defect A in `00-residual-ledger.md`.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| SESS-015 | high | parity-bug | M | Unresolvable `firstKeptEntryIndex` re-admits the whole compacted history |
| SESS-023 | high | not-ported | L | TUI `/tree` always navigates with `summarize: false`; branch-summary subsystem unreachable |
| SESS-007 | medium | parity-bug | S | Session file whose first physical line is blank fails to open |
| SESS-013 | medium | not-ported | M | AGENTS.md loaded twice in a nested git linked worktree |
| SESS-016 | medium | parity-bug | S | Explicitly EMPTY selected-tools list still emits skills and tool guidelines |
| SESS-017 | medium | parity-bug | M | `run_branch_summary` records `fromId` as the OLD leaf |
| SESS-018 | medium | parity-bug | S | `custom_message` null content renders `"null"`; missing `display` drops the entry |
| SESS-021 | medium | parity-bug | M | `SessionStats` ignores abandoned branches, summarization usage, toolCalls and cost |
| SESS-022 | medium | parity-bug | S | `run_branch_summary` summarizes even when the user declined |
| SESS-024 | medium | not-ported | S | Skills preamble drops pi's relative-path resolution instruction |
| SESS-028 | medium | parity-bug | S | Auto-compaction threshold fallback estimates the rendered context, not the raw one |
| SESS-032 | medium | test-defect | S | Two branch-summary tests assert the SESS-017 divergence as correct |
| SESS-004 | low | parity-bug | S | Unrecognized fields on known entry types dropped on rewrite |
| SESS-012 | low | not-ported | S | Summarization `usage` recorded but never accounted |
| SESS-014 | low | upstream-drift | S | Session-header discovery reads entire session files into memory |
| SESS-019 | low | upstream-drift | S | Footer `Current date` line plus extra newline; `project_context` wording drift |
| SESS-025 | low | stale-port | S | `reserve_tokens` doc and two test names assert the pre-SESS-006 rule |
| SESS-026 | low | parity-bug | S | `serialize_conversation` omits `[Assistant]: ` for an empty text block |
| SESS-027 | low | stale-port | S | Content-deserializer docs promise validation their bodies removed |
| SESS-029 | low | parity-bug | S | `estimate_tokens` applies one content-chars function to every role |
| SESS-030 | low | not-ported | S | `CompactionResult` omits pi's `usage` |
| SESS-031 | low | parity-bug | S | `estimatedTokensAfter` computed over the rendered context |
| SESS-033 | low | cyrup-original | S | `inputs_fingerprint` omits `disable_model_invocation`, has no caller, doc claims otherwise |
| SESS-034 | low | not-ported | M | Before-tree seam has no channel for customInstructions / replaceInstructions / label |

## SESS-015 — Unresolvable `firstKeptEntryIndex` demotes a compaction entry to `Unknown`, re-admitting the whole compacted history

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** high (failure path traced end to end at HEAD)

**cyrup** — `cyrup/crates/cyrup-session/src/entry.rs:61` declares `first_kept_entry_id: EntryId` with no `#[serde(default)]`. A v1 entry carries `firstKeptEntryIndex`, so `from_value::<KnownEntry>` fails at `entry.rs:257-259` and the entry lands as `Entry::Unknown` — the arm `migrate.rs:95-121` operates on. `convert_first_kept_index` (`migrate.rs:134-152`) removes `firstKeptEntryIndex` unconditionally at `:150` but inserts `firstKeptEntryId` only inside the `checked_sub(1) && assigned.get(pos)` guard at `:141-148`. When that guard fails, the re-type at `migrate.rs:116-119` no-ops and the entry stays `Entry::Unknown`; `latest_compaction` (`context.rs:99-101`) never matches, and both `build_context_messages` (`context.rs:107`) and `build_context_agent_messages` (`context.rs:240`) fall through to the `_ =>` arm that pushes the entire path.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:245-255` keeps `type:"compaction"` with `firstKeptEntryId` left undefined (`if (targetEntry && targetEntry.type !== "session") comp.firstKeptEntryId = targetEntry.id; delete comp.firstKeptEntryIndex;`). `buildContextEntries` (`session-manager.ts:418-453`) then never flips `foundFirstKept`, so its `0..compactionIdx` loop pushes nothing — pi **drops** the summarized history where cyrup **re-admits** it.

**Impact** — Opening a v1 session whose compaction target cannot be resolved silently re-sends the entire pre-compaction history to the model, on top of the summary. Cost and context-overflow, not a crash, so it stays invisible until a request blows the window.

**Fix** — `#[serde(default)] first_kept_entry_id: Option<EntryId>` at `entry.rs:61`, plus the three read sites (`context.rs:107-145`, `context.rs:240-278`, `compaction/prepare.rs:76-88`) treating `None` as "keep nothing before the compaction".

**Verify** — Add a fixture session with a v1 compaction entry whose `firstKeptEntryIndex` is 0 or out of range and assert `build_context_agent_messages` returns only the summary plus entries after it. No test covers the unresolvable case today.

## SESS-023 — TUI `/tree` always navigates with `summarize: false`, so the branch-summary subsystem is unreachable in the shipped binary

**Kind** not-ported · **Severity** high · **Effort** L · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:2051` handles `C::ConfirmSelection { kind: SelectorKind::Tree, value }` and at `:2056` calls `session.navigate_tree(entry, NavigateTreeOptions::default()).await`. `NavigateTreeOptions` derives `Default` (`cyrup-session-svc/src/session.rs:154-160`) with `summarize: bool` first, so it is always `false`. The summarize branch at `session.rs:1713-1750` never fires, `outcome.summary_entry` is always `None`, and `push_branch_summary` at `app.rs:2068` is dead. The arm also omits pi's pre-navigation abort of an in-flight response.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:4745-4812`: skip-prompt check `:4753`, three-option `showExtensionSelector` `:4755-4760`, custom-instructions editor `:4769`, stream abort `:4781-4785`, Escape → `abortBranchSummary()` `:4790-4799`, `navigateTree(entryId, { summarize, customInstructions })` `:4802-4806`, re-show tree on `result.aborted` `:4807-4812`.

**Impact** — A user navigating the tree in the shipped binary is never offered a branch summary and never gets one. Everything below — `compaction/branch.rs`, `generate_branch_summary_with_instructions`, `abort_branch_summary`, the retry observer, the `session_before_tree` hook, `BranchSummarySettings` — is unreachable, and `branchSummary.skipPrompt` in settings is a no-op. Navigating mid-stream also leaves the in-flight response running.

**Fix** — In `app.rs:2051-2068`, port the pi flow: abort any in-flight response, honor `skip_prompt`, present the three-option selector, open the custom-instructions editor on the third option, map Escape to `abort_branch_summary`, pass `NavigateTreeOptions { summarize, custom_instructions }`, and re-show the tree when `outcome.aborted`.

**Verify** — Drive the TUI event loop through a tree confirm in a session with a divergent branch and assert a `branch_summary` entry is appended; assert Escape during generation yields `aborted` with no entry.

## SESS-007 — A session file whose first physical line is blank fails to open

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high (line refs exact)

**cyrup** — `cyrup/crates/cyrup-session/src/manager.rs:851` `fn load`: blank-line skip at `:866-868`, but the header test is `if lineno == 0` (from `reader.lines().enumerate()`) at `:869`, so a leading blank line makes line 0 a non-header and the first real line is never treated as the header — `NotASession` at `:872`, and again at `:886` via `header.ok_or_else`. The listing side is self-inconsistent: `listing.rs:176-177` `read_header` takes the first non-blank line, while `listing.rs:189` `scan_file` takes `lines.next()?` unconditionally.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:502-510` `parseSessionEntryLine` returns null for blank *and* malformed lines; `loadEntriesFromFile` (`:513-554`) validates `entries[0]` — the first *parsed* entry — and returns `[]` rather than throwing (`:546-552`).

**Impact** — A session file with a stray leading newline (a truncated write, an editor, a merge) is unopenable, and the listing view disagrees with the loader about whether it exists at all.

**Fix** — In `manager.rs:851-886`, track "first parsed entry" rather than physical line index, and make `NotASession` a soft-empty return per pi. Share one rule across `load`, `listing.rs:176` and `listing.rs:189` — fix jointly with SESS-014.

**Verify** — Fixture session prefixed with `"\n"`; assert `load` succeeds, `read_header` and `scan_file` agree, and that a genuinely malformed first line yields empty rather than an error.

## SESS-013 — AGENTS.md loaded twice in a nested git linked worktree (no `findShadowedContextFile`)

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high (line refs exact both sides)

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/context_files.rs:89` `ContextFileLoader::load`; dedupe is `seen.insert(cf.path.clone())` at `:100` (global scan) and `:118` (ancestor walk) — exact-path only, no shadowing predicate. `grep -rn worktree crates/cyrup-session/` returns nothing.

**upstream** — `pi/packages/coding-agent/src/core/resource-loader.ts:100-116` `findShadowedContextFile`, both guards load-bearing at `:108` (`if (!worktreeRoot.startsWith(\`${mainRepoRoot}${sep}\`)) return undefined;`) and `:113` (canonical `.git` comparison); call site `:136`, `isShadowed` guard `:141-143`.

**Impact** — Inside a linked worktree nested under its main repo, the same AGENTS.md content is injected twice — wasted context and duplicated instructions the model may weight more heavily.

**Fix** — Port `findShadowedContextFile` into `context_files.rs`, call it before the `seen` insert at `:100`/`:118`, and skip shadowed files.

**Verify** — Create a linked worktree under the main repo with AGENTS.md at both roots and assert the built prompt contains the content once. Impact is still reasoned from code, not reproduced.

## SESS-016 — An explicitly EMPTY selected-tools list still emits the skills section and the tool guidelines

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high (every cited line verified exact)

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/builder.rs:272-274`: `fn is_selected(selected, name) -> bool { selected.is_empty() || selected.iter().any(...) }`. `read_available` at `:131-132`; skills gate `:152-154`; tool list loop `:168-188`; bash-fallback guideline `:202-205`; per-tool guidelines `:207-214` — all through that predicate. `PromptInputs.selected_tools` is `Vec<Arc<str>>` (`builder.rs:38`), so "unset" and "explicitly empty" are indistinguishable.

**upstream** — `pi/packages/coding-agent/src/core/system-prompt.ts:80` `const tools = selectedTools || ["read","bash","edit","write"]` — an empty array is truthy, so `hasBash`/`hasGrep`/`hasFind`/`hasLs`/`hasRead` (`:97-101`) are all false and the skills gate at `:155` skips. The customPrompt branch does the same via `:63` (`const customPromptHasRead = !selectedTools || selectedTools.includes("read")`).

**Impact** — A caller that deliberately restricts the agent to zero tools still gets skills advertised and tool-usage guidelines describing tools the model cannot call — instructions to use capabilities that do not exist.

**Fix** — Make `selected_tools` an `Option<Vec<Arc<str>>>` at `builder.rs:38`; `is_selected` becomes `match selected { None => DEFAULTS.contains(name), Some(v) => v.iter().any(...) }`. Update `read_available` and the three gates, plus the tool hash in the fingerprint at `:229-231`.

**Verify** — Add a `prompt/tests.rs` case with `selected_tools: Some(vec![])` asserting no `<available_skills>` and no tool guidelines. Nothing currently pins the wrong behavior — all eleven existing sites pass a non-empty vec.

## SESS-017 — `run_branch_summary` records `fromId` as the OLD leaf; pi records the navigation TARGET

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/compaction/mod.rs:383` `session.branch(&target_id)?;` then `:387` `let from_id = old_leaf.clone().unwrap_or_else(|| EntryId::from("root"));` then `:388` `session.append_branch_summary(from_id, ...)`. The sibling does it pi's way: `manager.rs:654` `let from_id = to.cloned().unwrap_or_else(|| EntryId::from("root"));` inside `branch_with_summary`. Live `/tree` uses the correct one (`cyrup-session-svc/src/session.rs:1753`); the divergent path is the embedder-facing `Compactor::run_branch_summary`.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:1381-1405` `branchWithSummary`: `this.leafId = branchFromId` (`:1391`), `parentId: branchFromId` (`:1395`), `fromId: branchFromId ?? "root"` (`:1397`) — one value for all three. Call site `agent-session.ts:3036-3042`, with the comment at `:3036` "Summary is attached at the navigation target position (newLeafId), not the old branch".

**Impact** — Summaries produced through the SDK/embedder path carry a `fromId` pointing at the abandoned leaf. Consumers reconstructing branch provenance from `fromId` get a different graph than pi's, and cyrup's two branch-summary implementations disagree with each other — the same session gets different metadata depending on which entry point produced it.

**Fix** — Collapse `Compactor::run_branch_summary` onto `session.branch_with_summary(Some(&target_id), ...)` (`manager.rs:640-670`), deleting the local `branch` + `append_branch_summary` pair at `mod.rs:383-388`. This also removes SESS-022 and SESS-034.

**Verify** — Assert `entry.from_id == entry.parent_id` (pi's invariant) for summaries from both entry points. Requires flipping the two assertions filed as SESS-032.

## SESS-018 — `custom_message` null content renders literal `"null"`; missing `display` drops the entry

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high (line refs verified by grep)

**cyrup** — `cyrup/crates/cyrup-session/src/entry.rs:101` `content: Value,` and `:102` `display: bool,` — neither carries `#[serde(default)]`, so an absent `display` fails `KnownEntry` deserialization and `entry.rs:257-259` demotes the entry to `Entry::Unknown`. `agent_message.rs:240-247` `custom_to_message` ends `other => vec![Content::text(other.to_string())]` at `:245`, so `Value::Null` injects the four characters `null` into a user turn.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:396-398` `createCustomMessage(entry.customType, entry.content ?? [], entry.display, entry.details, entry.timestamp)` — `?? []` normalizes null/undefined and a missing `display` is simply falsy.

**Impact** — Two ways a foreign or extension-written session degrades: a custom entry without `display` silently vanishes from context, and one with null content feeds the model the token `null`.

**Fix** — `#[serde(default)]` on `entry.rs:101-102`, and a `Value::Null => Vec::new()` arm before the catch-all at `agent_message.rs:241-246`.

**Verify** — Round-trip a custom entry with `content: null` and one with no `display`; assert the first contributes zero content blocks and the second survives as `KnownEntry::CustomMessage` with `display == false`.

## SESS-021 — `SessionStats` computed over post-compaction context of the current branch only; the persisted summarization `usage` has no reader

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high (upstream refs exact)

**cyrup** — `cyrup/crates/cyrup-session-svc/src/state.rs:28-53` `SessionStats::from_messages(&[Message])` matches only `Message::User` / `ToolResult` / `Assistant` — no compaction or branch-summary arm. The struct at `state.rs:11-24` has no `cost` and no tool-call counter. Both callers feed `self.messages()` (`session.rs:3063` and `:3082`); `messages()` at `session.rs:3011-3013` is `manager.build_context().messages`, documented at `:3007-3010` as the LLM-flattened, compaction-reduced view of the *current* branch.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:3112-3161` `getSessionStats`: loops `this.sessionManager.getEntries()` at `:3120`; compaction/branch-summary usage arm at `:3121-3123`; `toolCalls +=` at `:3138`, returned `:3149`; `cost` at `:3159`. `getEntries()` (`session-manager.ts:1301-1303`) is every entry in the file, abandoned branches included.

**Impact** — Reported session totals under-count: everything before a compaction, every abandoned branch, every summarization call's spend, plus `toolCalls` and `cost` are missing entirely. A user reading stats after a long compacted session sees a fraction of the real usage. This is also the missing half of SESS-012.

**Fix** — Add `SessionStats::from_entries(&[Entry])` in `state.rs` iterating `manager.entries()`, with the compaction/branch-summary `usage` arm first, plus `tool_calls` and `cost` fields. Repoint `session.rs:3063` and `:3082`. Leave `context_usage` on the rendered projection.

**Verify** — Build a session with a compaction (usage recorded), an abandoned branch, and tool calls on both branches; assert totals equal the sum over all entries, matching a pi capture.

## SESS-022 — `Compactor::run_branch_summary` generates and appends a summary even when the user declined one

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/compaction/mod.rs:352`: `if !user_wants_summary && settings.skip_prompt { (None, false) } else if collection.entries.is_empty() { ... } else { ...generate... }`. `skip_prompt` defaults false (`compaction/settings.rs:41`, `:46`, `BranchSummarySettings::default`), so `run_branch_summary(..., user_wants_summary = false, ...)` falls through to the generator at `mod.rs:362-378` and appends at `:388`.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:2983` gates strictly on the user's choice: `if (options.summarize && entriesToSummarize.length > 0 && !extensionSummary)`. `skipPrompt` is UI-only upstream: declared `settings-manager.ts:20` ("when true, skips \"Summarize branch?\" prompt and defaults to no summary"), read via `getBranchSummarySkipPrompt()` at `:801-803`, with exactly one consumer repo-wide — `interactive-mode.ts:4753`.

**Impact** — An embedder navigating with `summarize: false` still pays for a summarization LLM call and gets an unwanted `branch_summary` entry written into the session — the inverse of the setting's documented meaning. The live `/tree` path is unaffected (`cyrup-session-svc/src/session.rs:1713` has the correct gate), so the two paths disagree.

**Fix** — Replace the condition at `mod.rs:352` with a plain `if !user_wants_summary`, and demote `skip_prompt` to a front-end setting consumed only by the TUI (see SESS-023). Or fold into the SESS-017 collapse.

**Verify** — Call `run_branch_summary(..., user_wants_summary = false, ...)` with a non-empty collection and a recording summarizer; assert the summarizer was never invoked and no entry appended. Nothing pins the wrong behavior today — all five `skip_prompt` sites in tests pass `false`.

## SESS-024 — `<available_skills>` preamble drops pi's relative-path resolution instruction

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high (upstream line verified exact)

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/skills_inject.rs:17-18`: the entire preamble is the single const `SKILLS_PREAMBLE = "Available skills (open the SKILL.md with the read tool to use one):"`, pushed at `:32`.

**upstream** — `pi/packages/coding-agent/src/core/skills.ts:342-348` builds five preamble strings. The behavioral one is `skills.ts:345`: "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands." Lines `:343-344` are the cosmetic pair cyrup's DI-1 compaction legitimately folds.

**Impact** — Skills whose SKILL.md references sibling files by relative path (`references/palette.md`, `scripts/run.sh`) resolve against the agent's cwd instead of the skill directory, so the model issues reads that miss. Silent skill failure that looks like a badly written skill.

**Fix** — Add the resolution sentence as a second preamble line at `skills_inject.rs:17-18`. The section is already gated on `read` availability, so the instruction is only emitted when actionable.

**Verify** — `prompt/tests.rs` assertion that a prompt with at least one skill contains "resolve it against the skill directory".

## SESS-028 — Auto-compaction threshold fallback estimates the RENDERED context, not pi's raw AgentMessage context

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/session.rs:3664-3665` — the Case-2 threshold fallback is `let messages = self.messages().await; let estimate = estimate_context_tokens(&messages);`. `messages()` (`session.rs:3011-3013`) is the `convertToLlm`-flattened view. `estimate_context_tokens` (`cyrup-session/src/compaction/tokens.rs:127`) takes `&[Message]`; the raw twin `estimate_context_tokens_raw` (`tokens.rs:161`) takes `&[AgentMessage]` and exists for exactly this distinction. `session.rs:3665` is the only non-test production use of the rendered variant in the workspace, while `Compactor::should_compact` (`cyrup-session/src/compaction/mod.rs:114-128`) — which correctly uses `build_context_agent_messages` + `estimate_context_tokens_raw` at `:125-126` and cites the right pi lines in its own comment — has exactly one caller workspace-wide, a test at `cyrup-session/tests/compaction.rs:237`.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:2019-2021` `const messages = this.agent.state.messages; const estimate = estimateContextTokens(messages); if (estimate.lastUsageIndex === null) return false;`. `agent.state.messages` is `AgentMessage[]` (assigned at `agent-session.ts:1874` and `:2156`), and `estimateContextTokens` (`compaction.ts:202-229`) dispatches per-message on the raw roles `bashExecution` (`:284-287`), `custom` (`:279-283`), `branchSummary`/`compactionSummary` (`:288-292`).

**Impact** — On the persistent-API-error path (stopReason `error`, or all-zero usage) cyrup measures a different number than pi: the rendered projection drops `excludeFromContext` bash messages and pads summaries with LLM wrapper prose, so auto-compaction fires early or late. cyrup's own test proves the bases differ — `tests/compaction.rs:818-826` asserts the raw estimate is pi's captured 77 and `assert_ne!(rendered_tokens, 77)`. Secondary hazard: a correct raw-basis `should_compact` sits dead beside the live wrong-basis one, so a future editor may "fix" the dead copy and observe no behavior change.

**Fix** — At `session.rs:3665` estimate over the raw projection with `estimate_context_tokens_raw`, or route the whole Case-2 check through `Compactor::should_compact` so there is one implementation. Note pi reads the agent's *live* state (already including the just-received assistant message) where cyrup re-reads the persisted branch — reconcile or document that too.

**Verify** — Build a branch with an `excludeFromContext:true` bash entry plus a compaction summary, hand `check_compaction` an assistant turn with `stop_reason = Error`, and assert the trigger decision follows `estimate_context_tokens_raw` (the value `tests/compaction.rs:818` already pins) rather than the rendered one.

## SESS-032 — Two branch-summary tests assert `from_id` is the OLD leaf, pinning the SESS-017 divergence

**Kind** test-defect · **Severity** medium · **Effort** S · **Confidence** high (both line numbers exact)

**cyrup** — `cyrup/crates/cyrup-session/tests/compaction.rs:518-520`: `// Appended at the navigation point: from_id is the abandoned leaf, parent is the target.` / `assert_eq!(entry.from_id, l1, "from_id is the leaf navigated from (R-05-016)");` / `assert_eq!(entry.parent_id.as_ref(), Some(&l2), "appended at the navigation point");` — the two assertions contradict each other under pi's model, where `fromId` and `parentId` are one value. Same shape at `tests/compaction.rs:899-900` (`assert_eq!(entry.from_id, abandoned, ...)` / `assert_eq!(entry.parent_id.as_ref(), Some(&shared_a), "appended at the navigation target")`). Both drive `Compactor::run_branch_summary` (`compaction/mod.rs:316`), the path whose `from_id` (`mod.rs:387`) disagrees with the sibling `SessionManager::branch_with_summary` (`manager.rs:654`) the live `/tree` uses.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:1391-1398` sets `this.leafId = branchFromId`, `parentId: branchFromId` and `fromId: branchFromId ?? "root"` — one value for all three. `agent-session.ts:3036-3041` passes the navigation target.

**Impact** — The classic "test pins current-but-wrong behavior" shape. The suite makes SESS-017 look intentional: an auditor grepping for coverage finds two green assertions citing a spec id, while the contradicting sibling implementation has no assertion against it at all. Any SESS-017 fix reads as a regression until these flip.

**Fix** — With SESS-017, change both assertions to expect the navigation target (`l2` at `:519`, `shared_a` at `:899`) and drop the R-05-016 justification from the comments — `spec/` is not in this workspace and cannot be cited to defend behavior contradicting pi's code.

**Verify** — After the fix, `rg -n 'from_id is the .*navigated from' crates/cyrup-session/tests/` returns nothing and both tests assert `entry.from_id == entry.parent_id`.

## SESS-004 — Unrecognized fields on known entry types dropped on rewrite

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/entry.rs:15-26` `EntryBase` declares only `id`/`parent_id`/`timestamp` with no `#[serde(flatten)]` catch-all. `impl Deserialize for Entry` (`entry.rs:246-269`) preserves the original JSON verbatim only on the `Err(_) => Ok(Entry::Unknown(v))` arm at `:259`.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:502-510` `parseSessionEntryLine` is a bare `JSON.parse` with a blank/malformed→null guard and no schema check, so unknown keys survive any rewrite.

**Impact** — A session written by a newer cyrup, a fork, or an extension that annotates entries loses those annotations the first time this cyrup rewrites the file. Silent data loss, bounded to non-modelled keys.

**Fix** — Add `#[serde(flatten)] extra: Map<String, Value>` to `EntryBase` (`entry.rs:15-26`) and re-emit it on serialize.

**Verify** — Round-trip a session entry carrying an unknown top-level key and assert byte equality. Standing caveat: keys nested inside `AgentMessage`/`Content` have still not been enumerated.

## SESS-012 — Summarization `usage` recorded but never accounted

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — Recording is done and correct: `cyrup/crates/cyrup-session/src/entry.rs:71` (Compaction) and `:85` (BranchSummary) carry `usage: Option<Usage>`, `skip_serializing_if`-elided; `combine_usage` (`compaction/summarize.rs:145-167`) matches pi field-for-field, including the `(None,None) => None` rule for `cache_write_1h`/`reasoning`. Accounting is absent: every `KnownEntry::Compaction`/`BranchSummary` destructuring in the workspace (`session.rs:1300/3693/4432/4435/4514`, `entry.rs:140-155`, `manager.rs:495/655/677`, `context.rs:64/67/111/178/185/225/228/243`) elides `usage` with `..`. Zero readers.

**upstream** — `pi/packages/coding-agent/src/core/compaction/compaction.ts:99-122` `combineUsage`, consumed at `agent-session.ts:3121-3123` (stats) and `:1894-1901` (`compaction_end` payload).

**Impact** — Summarization spend is written to disk and never surfaced anywhere: not in stats, not on the event stream.

**Fix** — Both readers: SESS-021 (`SessionStats`) and SESS-030 (`CompactionResult`). Nothing further is owed by this item once those land.

**Verify** — Grep that at least one non-test site destructures `usage` off a compaction/branch-summary entry, and that its value reaches both stats and the event payload.

## SESS-014 — Session-header discovery reads entire session files into memory

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/listing.rs:176` `read_header` → `std::fs::read_to_string(path).ok()?`; `listing.rs:187` `scan_file` → the same. `read_header` is called per-candidate from `newest_session` (`listing.rs:105-111`).

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:490-494` defines `SESSION_READ_BUFFER_SIZE` (1 MiB), `SESSION_HEADER_READ_BUFFER_SIZE` (4096) and `MAX_SESSION_HEADER_SCAN_BYTES` (1 MiB), plus `SessionHeaderScanLimitError` at `:496-501` and the `readSync` loop in `loadEntriesFromFile` (`:513-554`).

**Impact** — Listing N sessions reads N whole files; on a directory of large sessions the picker stalls and peak memory tracks the largest file. This is **drift, not a stale port**: pi's buffered reader landed in `f1c587dd` ("avoid duplicate session reads", 2026-07-19), after cyrup's 2026-07-10 baseline, so cyrup never had it to port.

**Fix** — Replace both `read_to_string` calls with a bounded reader that stops at the first newline or a scan-byte cap. Do it with SESS-007 so `load`, `read_header` and `scan_file` share one first-entry rule.

**Verify** — Point `read_header` at a multi-megabyte session and assert bytes read stay under the cap.

## SESS-019 — Footer emits a `Current date` line pi has since REMOVED (plus an extra leading newline); `project_context` wording drifts

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high (git history checked on both sides)

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/builder.rs:333-339` writes `"\n\nCurrent date: {:04}-{:02}-{:02}"` — a blank line before the date. Wording deltas: `builder.rs:104` `project_context_open: "<project_context>\n\nProject-specific instructions follow.\n\n"` and `builder.rs:105` `project_context_close: "</project_context>"` (no trailing newline).

**upstream** — pi `f4e9ca74` (2026-07-14, "remove current date from system prompt", fixes #6621) deletes `prompt += \`\nCurrent date: ${date}\`` from both branches of `buildSystemPrompt` plus the now/year/month/day computation. Pre-removal pi wrote a single `\n`, not two. Current wording lives at `pi/packages/coding-agent/src/core/system-prompt.ts:55-56` (`"\n\n<project_context>\n\n"` + `"Project-specific instructions and guidelines:\n\n"`) and `:60` (`"</project_context>\n"`).

**Impact** — Byte-level prompt divergence from pi. The date line is a *faithful port* of the version cyrup ported — not a cyrup invention, and it owes no `[CYRUP-DELTA]` marker — but pi has since removed it, so keeping it is a deliberate drift decision. The extra newline and the `project_context` wording are genuine port deltas independent of that decision (`git log --since=2026-06-01 -- system-prompt.ts` shows only `f4e9ca74` and a docs-pointer tweak).

**Fix** — Two separable pieces. (a) Drop the date line at `builder.rs:333-339` to follow `f4e9ca74` — upstream drift, so decide it jointly with DRIFT-016/DRIFT-035 in `12-upstream-drift-pi-core.md`. (b) Independently of (a), fix the extra leading `\n` and align the `project_context` open/close strings at `builder.rs:104-105`.

**Verify** — Byte-compare a built prompt against a pi capture for the `project_context` region. Note `prompt/tests.rs:82` and `:113` (`out.contains("Current date: 2026-06-28")`) are **not** test-defects — they correctly pin the ported baseline and go stale only if (a) is taken.

## SESS-025 — `reserve_tokens` doc comment asserts the pre-SESS-006 budget rule; two test names repeat it

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/compaction/settings.rs:35-36` still reads "Message budget (newest-first) for the branch summary (default 16384). Per the corrected R-05-016 this IS the message budget, not `contextWindow − reserve`." — false since SESS-006 landed (`branch.rs:130-134`). `crates/cyrup-session/tests/compaction.rs:529 fn branch_budget_is_reserve_tokens_newest_first` repeats the stale claim in its name (and its comment at `:530`); `crates/cyrup-session/tests/parity.rs:122 fn gap5_cut_point_validity_excludes_settings_and_summaries` is named for excluding summaries while its own comment at `:123-125` says `branch_summary` *is* a valid cut point. Both test bodies assert correctly.

**upstream** — `pi/packages/coding-agent/src/core/compaction/branch-summarization.ts:312-313` (`const contextWindow = model.contextWindow || 128000;` / `const tokenBudget = contextWindow - reserveTokens;`), reserve default at `:305`. cyrup's own inline citations (`branch.rs:117`, `:121`, `mod.rs:360`) point at `:315-317` and are wrong by seven lines.

**Impact** — Documentation-only, but it is precisely the claim SESS-006 disproved; a reader trusting it will re-introduce the bug. The two misleading test names hide correct coverage.

**Fix** — Rewrite `settings.rs:35-36` to state the `contextWindow || 128000 − reserve` rule; rename `compaction.rs:529` and `parity.rs:122`; correct the three upstream line citations to `:312-313` / `:305`.

**Verify** — Grep that no doc under `compaction/` claims `reserve_tokens` is the budget.

## SESS-026 — `serialize_conversation` omits `[Assistant]: ` for a turn whose only text block is empty

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/compaction/serialize.rs:30-33`: `let text = join_text(&a.content, "\n"); if !text.is_empty() { parts.push(format!("[Assistant]: {text}")); }` — guards on the *joined* text, so a single empty text block joins to `""` and the line disappears.

**upstream** — `pi/packages/coding-agent/src/core/compaction/utils.ts:135-137` guards on *presence*: `if (msg.content.some((block) => block.type === "text")) { parts.push(\`[Assistant]: ${contentText(msg.content)}\`); }`. The asymmetry is deliberate upstream — the user arm (`utils.ts:113-115`) and toolResult arm (`:141-145`) both do guard on emptiness.

**Impact** — The transcript handed to the summarizer loses an assistant turn marker, shifting the interleaving pi's summary prompt sees. Rare and low-consequence, but it changes summary input bytes.

**Fix** — Change the guard at `serialize.rs:30-33` to `a.content.iter().any(|c| matches!(c, Content::Text { .. }))`. Leave the user and toolResult arms alone.

**Verify** — Serialize an assistant message whose only content is `Content::text("")` and assert the output contains `[Assistant]: `. Nothing currently pins the wrong behavior.

## SESS-027 — Three `cyrup-core` content deserializers' docs promise per-role validation their bodies removed

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** high (all six doc/body pairs read)

**cyrup** — `cyrup/crates/cyrup-core/src/message.rs:575-578` documents `de_user_content` as "validating the array to `Text|Image` only … a `Thinking`/`ToolCall` block is rejected" while its own body comment at `:590-592` says "with no role-union rejection". The same contradiction holds for `de_tool_result_content` (doc `:598-599` vs body `:607-608`) and `de_assistant_content` (doc `:610-612` vs body `:618-621`). The field docs at `:465-468` and `:481-482` repeat the false claim.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:502-510` `parseSessionEntryLine` is a bare `JSON.parse`; pi's per-role content unions in `ai/src/types.ts` are compile-time TypeScript only, never enforced at read time. The tolerant behavior is the correct port.

**Impact** — Documentation-only. An auditor reading these docs concludes cyrup validates content-block roles on read; it does not, by design. Someone may "restore" the validation and break SESS-001.

**Fix** — Rewrite the three fn docs and the two field docs (`message.rs:465-468`, `:481-482`, `:575-578`, `:598-599`, `:610-612`) to state the actual read-tolerant contract and cite `session-manager.ts:502-510`.

**Verify** — Grep that no doc in `message.rs` claims a block type is "rejected" on deserialize.

## SESS-029 — `cyrup-session`'s `estimate_tokens` applies one content-chars function to every role

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/compaction/tokens.rs:26-36` binds `blocks` uniformly (`Message::User { content, .. } | Message::ToolResult { content, .. } => content, Message::Assistant(a) => &a.content`) and sums `content_chars` over all three. `content_chars` (`tokens.rs:86-96`) charges `Content::Image => ESTIMATED_IMAGE_CHARS` (4800) and counts `Thinking`/`ToolCall` regardless of role. The workspace already holds a correct port of the same pi function: `cyrup/crates/cyrup-provider/src/utils/estimate.rs:80-101` gives the Assistant arm `Content::Image { .. } => 0`, and `text_and_image_chars` (`estimate.rs:56-68`) counts only Text and Image for user/toolResult.

**upstream** — `pi/packages/coding-agent/src/core/compaction/compaction.ts:277-288` — the `assistant` arm accumulates only `text`, `thinking` and `toolCall`; an image contributes nothing. `estimateTextAndImageContentChars` (`compaction.ts:246-260`), used by the `user` (`:272-276`) and `custom`/`toolResult` (`:289-293`) arms, counts only text and image.

**Impact** — An assistant message carrying an image block costs 1200 tokens in cyrup and 0 in pi; a user/toolResult message carrying a thinking or toolCall block (reachable through the SESS-001 read tolerance on a foreign or hand-edited session) is counted where pi counts 0. This estimate feeds the persisted `tokensBefore`, the keep-recent cut-point walk and the raw trigger, so cyrup can cut at a different entry than pi on the same file. Two ports inside this repo disagree, and the provider one matches pi.

**Fix** — Split `estimate_tokens` at `tokens.rs:26-36` into role-dispatched arms mirroring `cyrup_provider::estimate_message_tokens`: user/toolResult → text+image only; assistant → text/thinking/toolCall only. Or delegate to the provider function for the core-`Message` case and keep the AgentMessage arms local in `estimate_agent_message` (`tokens.rs:41-52`).

**Verify** — Beside the M1 byte-diff test in `crates/cyrup-session/tests/compaction.rs`, add an assistant message of one text block plus one image block and assert the estimate equals `ceil(text_len/4)`, citing `compaction.ts:277-288`. Severity is low because no cyrup code path currently places a `Content::Image` into an assistant message — the images provider lives under `cyrup-provider/src/images/` and does not feed session entries; reachable only via a foreign session or an extension.

## SESS-030 — `CompactionResult` omits pi's `usage`

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/state.rs:89-97` `pub struct CompactionResult { summary, first_kept_entry_id, tokens_before, estimated_tokens_after (:94), details (:96) }` — no `usage`. Constructed at `cyrup-session-svc/src/session.rs:1387-1393` (manual compact) and `:3804-3810` (auto), then emitted as `AgentSessionEvent::CompactionEnd` at `:1411` and `:3828`. The value is in hand: `usage` was persisted onto the compaction entry (`cyrup-session/src/entry.rs:71`) one statement earlier.

**upstream** — `pi/packages/coding-agent/src/core/compaction/compaction.ts:87-97` declares `CompactionResult<T>` with `/** Usage from the LLM call(s) that generated this summary, if available */ usage?: Usage;` at `:89-90`. `agent-session.ts:1894-1901` puts it on the object emitted as `compaction_end`, and the auto path repeats it at `:2174-2176`. Extensions consume it via `onComplete?: (result: CompactionResult) => void` and the `compaction?: CompactionResult` payload field in `coding-agent/src/core/extensions/types.ts`.

**Impact** — An extension or RPC client watching `compaction_end` cannot see what the compaction cost, even though cyrup wrote that usage into the session file one line earlier. Any host doing its own cost accounting off the event stream silently under-reports every compaction.

**Fix** — Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub usage: Option<Usage>` to `state.rs:89-97` and populate from the returned entry at `session.rs:1387` and `:3804`.

**Verify** — In `crates/cyrup-session-svc/tests`, run a compaction against a stub summarizer returning a known usage and assert the `CompactionEnd` payload's `usage` equals it and equals the usage on the appended compaction entry.

## SESS-031 — `estimatedTokensAfter` is computed over the LLM-rendered context instead of pi's raw AgentMessage context

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/session.rs:1372-1377` and `:3790-3795` both do `guard.build_context().messages.iter().map(cyrup_provider::estimate_message_tokens).sum()`. `build_context()` is the `convertToLlm`-flattened projection (`session.rs:3001-3003` / `:3011-3013`). `prepare_compaction` already does it right for `tokens_before`: `cyrup-session/src/compaction/prepare.rs:131-133` `let full_context = build_context_agent_messages(&refs); let tokens_before = estimate_context_tokens_raw(&full_context).tokens;`.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:1876` and `:2157` — `const estimatedTokensAfter = estimateMessagesTokens(sessionContext.messages);`, where `sessionContext = this.sessionManager.buildSessionContext()` yields `AgentMessage[]` (`session-manager.ts:462-469`) and `estimateMessagesTokens` (`agent-session.ts:284-290`) sums compaction.ts's raw-role `estimateTokens`.

**Impact** — The post-compaction size reported on `compaction_end` and over RPC differs from pi's for any branch containing bash executions, custom messages or summaries: wrapper prose gets counted and `excludeFromContext` bash is dropped. Reporting-only, but it is the number a UI shows as "compacted from X to Y", and it is inconsistent with `tokens_before` in the same payload.

**Fix** — Use the raw projection at `session.rs:1372` and `:3790`: `build_context_agent_messages` + `estimate_context_tokens_raw(...).tokens`, matching `prepare.rs:131-133`. Same root cause as SESS-028, different call site and different fix.

**Verify** — Extend the M1 byte-diff fixture (`tests/compaction.rs:795-836`) through a real compaction and assert `estimated_tokens_after` equals the raw estimate of the rebuilt context, not the rendered one.

## SESS-033 — `SystemPromptBuilder::inputs_fingerprint` omits `disable_model_invocation` and has no production caller, though its doc claims the agent caches on it

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/builder.rs:222-223`: "Cheap, non-cryptographic fingerprint of the output-affecting inputs (R-06-017). The agent caches `(fingerprint -> prompt)` for the session and only rebuilds when it changes." The skills loop at `builder.rs:248-252` hashes only `s.name`, `s.description`, `s.path` — not `disable_model_invocation`, which a104476 made output-affecting (`skills_inject.rs:26-30`). The caching claim is false at HEAD: `grep -rn inputs_fingerprint crates/` returns exactly three lines — the definition at `builder.rs:224` and two test call sites at `prompt/tests.rs:294-295`. The snapshot it would key on is swapped wholesale by `ContextStore` (`prompt/cache.rs:31-38` `ContextSnapshot`, held in an `ArcSwap`).

**upstream** — pi has no prompt fingerprint. `buildSystemPrompt` (`pi/packages/coding-agent/src/core/system-prompt.ts:36+`) is a pure function and agent-session rebuilds explicitly on tool-set change. There is no upstream contract to inherit; the divergence is that cyrup's doc promises a cache that does not exist and the hash it would use is already incomplete.

**Impact** — Latent today (no caller). If the fingerprint is ever wired up as documented, flipping a skill's `disable-model-invocation` frontmatter and reloading leaves the stale prompt in place, re-advertising a skill the author just hid — re-opening SESS-003. Meanwhile the doc misleads anyone auditing prompt-cache correctness.

**Fix** — Hash `s.disable_model_invocation` in the skills loop at `builder.rs:248-252`, then either wire the fingerprint into a real cache or correct the doc at `builder.rs:222-223` to say it is currently unused. Also reconsider `inp.today.hash(&mut h)` at `builder.rs:255`, which forces a daily rebuild only because of the SESS-019 date line.

**Verify** — In `prompt/tests.rs` beside `:294`, build two `PromptInputs` differing only in a skill's `disable_model_invocation` and assert their fingerprints differ — the same shape as the existing with_tool/without_tool assertion.

## SESS-034 — `Compactor::run_branch_summary`'s before-tree seam has no channel for the hook's customInstructions / replaceInstructions / label

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/compaction/hooks.rs:147-155`: `pub enum BeforeTreeDecision { Proceed, Cancel, CustomSummary { summary: String, details: Option<Value> } }` — three variants only. `compaction/mod.rs:345-380` consumes exactly those, and the generator it calls, `generate_branch_summary` (`compaction/branch.rs:221-236`), hardcodes `BRANCH_SUMMARY_PROMPT` at `:238` with no instruction slot. The live `/tree` path supports all three through a different seam: `cyrup-session-svc/src/session.rs:1725-1731` threads `eff_custom_instructions` / `eff_replace_instructions` into `generate_branch_summary_with_instructions`.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:2963-2975` reads `result.customInstructions`, `result.replaceInstructions` and `result.label` off the single `session_before_tree` hook result and passes the first two into `generateBranchSummary` at `:2988-2996`. `generateBranchSummary` honors them at `branch-summarization.ts:326-334` (`if (replaceInstructions && customInstructions) instructions = customInstructions; else if (customInstructions) instructions = \`${BRANCH_SUMMARY_PROMPT}\n\nAdditional focus: ${customInstructions}\`;`).

**Impact** — An extension that steers or replaces the branch-summary prompt, or labels the resulting entry, is silently ignored when the summary is produced through the embedder/SDK path — the same orphaned path that already carries SESS-017 (wrong `fromId`) and SESS-022 (summarizes when declined). Three defects on one duplicated code path.

**Fix** — Fix with SESS-017/SESS-022 by collapsing `Compactor::run_branch_summary` onto the session-svc flow; alternatively widen `BeforeTreeDecision` with `custom_instructions` / `replace_instructions` / `label` and call `generate_branch_summary_with_instructions`. Collapsing removes all three at once.

**Verify** — In `crates/cyrup-session/tests/compaction.rs`, script a before-tree hook returning custom instructions and assert the recording summarizer saw a prompt containing "Additional focus:"; with `replace_instructions`, assert the prompt is the custom text alone.

## Coverage

**Read at HEAD `1806375` (tree clean).** Nothing was executed — no cargo, no npm. Every closure is a read of the function body at HEAD plus the corresponding pi function opened in full, not a quoted excerpt. Where the previous document cited an upstream line number it was re-derived; four upstream citations were wrong and are corrected inline. Most consequential: SESS-006 and SESS-025 both pointed at `branch-summarization.ts:319-320` for a budget computation that lives at `:312-313` (reserve default `:305`) — cyrup's own in-code comments at `branch.rs:117`, `:121` and `mod.rs:360` share the same off-by-seven error.

**Closures.** Ten confirmed, one (SESS-012) confirmed as genuinely partial. Two were specifically attacked and held: SESS-010's branch path passing `ModelThinkingLevel::Off` looks like a shortcut, but pi builds branch options inline at `branch-summarization.ts:348` and never sets `reasoning`, so it is a correct port; SESS-005's back-scan was checked for the *order* of its two break conditions (a reversed order is semantically identical but would signal a guessed port) and matches `compaction.ts:441-445` exactly. SESS-012's "zero readers" claim was re-derived by grepping all twelve destructuring sites. One inherited supporting claim was found overstated and does not affect the verdict: `estimate_message_entry` is not a zero-hit grep — it survives at `tokens.rs:239` inside a doc comment — but the function itself is genuinely gone.

**History mined.** `git log c8bd2ab..HEAD` in cyrup plus pi's log since cyrup's baseline. Two pi commits change item characterizations and are folded in: `f4e9ca74` (2026-07-14) removed the `Current date` line, making SESS-019 drift rather than a cyrup invention; `f1c587dd` (2026-07-19) added the buffered header reader, making SESS-014 drift rather than a stale port. bb301b6's message defers "five live-fork commits since a6f720e6"; those were enumerated (`9b3a2059, 8e53e0e4, 2fd38684, f8b74a45, f1c587dd, f4e9ca74, bb3d7d39, 94373d81, d2f8dafb, 7303cbac`). Three were closed by this repo's own gap work (9b3a2059 = SESS-009, 8e53e0e4 = SESS-008, 2fd38684 = the SESS-012 recording half). `bb3d7d39` was checked and dismissed — its only system-prompt.ts change is a one-sentence docs pointer, and cyrup's docs section is a DI-4 rewrite anyway.

**Test-defect hunt, run independently rather than inherited.** Shape 1 (pins current-but-wrong behavior): two genuine instances, both filed as SESS-032, line numbers exact. The hunt was extended and found no further instances — `grep -rn skip_prompt crates/cyrup-session/tests/` shows all five sites pass `false` (nothing pins SESS-022); `grep -n selected_tools crates/cyrup-session/src/prompt/tests.rs` shows all eleven sites pass a non-empty vec (nothing pins SESS-016); `grep -rn NotASession crates/` finds no test (nothing pins SESS-007); `grep -rn '\[Assistant\]' crates/cyrup-session/` finds only the production format string (nothing pins SESS-026). The `prompt/tests.rs:82`/`:113` date assertions are explicitly *not* this shape — see SESS-019. Shape 2 (asserts an uncontrollable scheduling outcome): `grep -rn 'sleep|Instant::now|elapsed|yield_now|tokio::spawn|spawn_blocking' crates/cyrup-session/` returns four hits, all prose or a legitimate `spawn_blocking` for fs I/O (`prompt/cache.rs:86`). Nothing in `cyrup-session` is timing-dependent.

**One adjacent test cleared, with the previous rationale rejected.** `crates/cyrup-session-svc/tests/compact_refusals.rs:232` is safe, but *not* because "the asserted outcome holds wherever in the stream the cancel lands." It does not: `complete_summarization` ends in `cancel.run_until_cancelled(call)` (`summarize.rs:240-243`), which returns `Some(msg)` — success — if the call already completed, so a late cancel would yield `Ok` and fail the test's `expect_err`. What actually makes it safe is that the faux provider is configured with `tokens_per_second: Some(1.0)` and a deliberately long summary (`compact_refusals.rs:186-201`, comment: "long enough that at 1 token/s it streams for minutes"), so the 300 ms sleep cannot outrun the stream. The old rationale would license genuinely racy tests elsewhere and must not be reused.

**Blind spots and things taken on trust.**
- No test was run. A test cited as evidence is evidence of intent, not of passing; a confirmed-correct implementation could still be failing.
- `spec/` is absent from this workspace, so `R-05-016` / `R-05-018` / `R-06-010` / `R-06-017` are grep anchors only. This matters most for SESS-017 and SESS-022, where cyrup's own doc comments (`manager.rs:665-668`, `compaction/mod.rs:353`) invoke spec text to justify behavior contradicting pi. Those judgments rest on pi's code plus the fact that cyrup's two branch-summary paths contradict *each other* — a defect under any spec reading.
- cyrup's inline pi citations on the branch-summary path are stale by roughly 200 lines (`mod.rs:357` cites agent-session.ts:2787 for a gate now at `:2983`; `session.rs:1714`/`:1752` cite `:2787`/`:2845-2868` for code now at `:2983`/`:3036-3060`). Worth a low-severity citation-refresh sweep; not filed as an item.
- SESS-013's doubled-load impact is reasoned from code, not reproduced — it needs a real `git worktree add` inside a repo.
- SESS-004's caveat stands: pi's nine `SessionEntry` variants were re-checked against `KnownEntry` with no unmodelled *top-level* field found, but keys nested inside `AgentMessage`/`Content` were not enumerated.
- pi `f8b74a45` ("complete extension usage accounting") touches only the *harness* compaction fork plus docs. No session-area gap could be substantiated from it; it is the one commit in this window not run to ground.
- Not examined at all: session-file locking under concurrent writers, the fork/clone path beyond entry seeding, and TUI rendering of session entries outside the tree-confirm arm. `crates/cyrup-session-svc/tests/{round4.rs:297, round9_l5res.rs:347}` contain sleeps left to area 08 — but given that the `compact_refusals` rationale did not survive scrutiny, they deserve the `tokens_per_second`-style check rather than a glance.


---

## Surface-sweep findings (2026-08-03, HEAD `9219dcd`)

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at
all, rather than checking a list of known items. That inversion exists because the
item-driven method missed pi's stray-OSC-reply swallow (`pi/packages/tui/src/tui.ts:788-794`)
— a real, user-reported bug — and by construction cannot see behaviour nobody wrote an item
for. IDs use an `-SNN` suffix to mark their provenance.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| SESS-S01 | medium | not-ported | S | `AgentSession::list_sessions()` re-derives the DEFAULT cwd-encoded session dir instead of the manager's own dir, so the in-TUI `/resume` picker is blind under `--session-dir` (and under `--session <path elsewhere>`) |
| SESS-S02 | medium | not-ported | S | `/import` copies the JSONL into the sessions ROOT instead of the per-cwd session directory, so the imported session is invisible to every listing path |
| SESS-S03 | low | not-ported | S | Cross-project `listAll(sessionDir)` overload and the shared-dir `filterCwd` are unwired on the LISTING paths: under `--session-dir` the "all sessions" bucket is empty, and a shared dir lists other projects' sessions |
| SESS-S04 | low | not-ported | S | No resume-command hint on quit — pi prints the exact `--session-dir`/`--session` invocation needed to return; cyrup prints nothing |
| SESS-S05 | low | not-ported | S | `TreeNode` drops pi's `labelTimestamp`; the `t` toggle was ported anyway and renders the literal string `"current"` on the leaf row |
| SESS-S06 | low | not-ported | S | Session-listing `message_count` and search text skip any `message` entry whose payload fails to deserialize into cyrup's typed `AgentMessage`, where pi counts every `type:"message"` line |

## SESS-S01 — `AgentSession::list_sessions()` re-derives the DEFAULT cwd-encoded session dir instead of the manager's own dir, so the in-TUI `/resume` picker is blind under `--session-dir` (and under `--session <path elsewhere>`)

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/session-manager.ts:999-1001 `getSessionDir()`; field fixed at construction (`:953` uses it to place the file). `SessionManager.open` (:1526-1548) explicitly derives the dir from the FILE'S PARENT when no sessionDir is passed (`const dir = sessionDir ? normalizePath(sessionDir) : resolve(resolvedPath, "..")`, :1547). Consumer: pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:4867 `SessionManager.list(this.sessionManager.getCwd(), this.sessionManager.getSessionDir(), onProgress)`.

**cyrup** — ABSENT. `grep -rn "session_dir|uses_default_session_dir|getSessionDir" cyrup/crates/cyrup-session/src/` -> ONE hit, and it is a doc comment (layout.rs:36). `SessionManager` (crates/cyrup-session/src/manager.rs:36-49) has no session-dir field; the accessor block has none. The consumer is crates/cyrup-session-svc/src/session.rs:3092-3105 (claim cited :3052-3064 — line drift at HEAD 9219dcd): `sessions_root()` = `self.services.agent_dir.join("sessions")`, `list_sessions()` = `SessionLayout::new(self.sessions_root(), self.services.cwd.clone())` -> always the encoded default. `grep -n "session_dir" crates/cyrup-session-svc/src/services.rs` -> 0 hits. Sole consumer: crates/cyrup-tui/src/app.rs:2261.

**Impact** — Under `cyrup --session-dir ~/shared`, sessions are written to `~/shared/` but `/resume` scans `~/.cyrup/agent/sessions/--<enc-cwd>--/` — the user's sessions are unreachable from inside the TUI, or a stale unrelated set is offered and switched into. Broader than the claim states: it also fires for `cyrup --session /elsewhere/x.jsonl`, where pi's `open()` derives the dir from the file's parent and cyrup does not. The CLI `--resume` path IS correct (crates/cyrup/src/main.rs:952-958 `session_list_layout` honours `session_dir_explicit`), so the two entry points to the same feature disagree.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SESS-S02 — `/import` copies the JSONL into the sessions ROOT instead of the per-cwd session directory, so the imported session is invisible to every listing path

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/agent-session-runtime.ts:367-372 — `const sessionDir = this.session.sessionManager.getSessionDir(); ... const destinationPath = join(sessionDir, basename(resolvedPath));`. For a default-configured manager that is `<agentDir>/sessions/--<enc-cwd>--`, i.e. exactly the directory `SessionManager.list(cwd)` scans.

**cyrup** — ABSENT. crates/cyrup-session-svc/src/runtime.rs:511,517 — `let session_dir = self.factory.session_dir(); ... let destination = session_dir.join(file_name);`. crates/cyrup-session-svc/src/factory.rs:90-95 — `session_dir()` = `base_config.session_dir.clone().unwrap_or_else(|| base_config.agent_dir.join("sessions"))`. `grep -n "encode_cwd|SessionLayout" crates/cyrup-session-svc/src/factory.rs` -> 0 hits: no cwd encoding on this path. In the default case (`session_dir == None`, every run without `--session-dir`) the destination is the sessions ROOT.

**Impact** — `/import ~/backup/session.jsonl` succeeds and the runtime switches into it, so it looks correct. The file lands at `~/.cyrup/agent/sessions/session.jsonl` and is thereafter unreachable: `/resume` scans the `--<enc-cwd>--` subdir, and `--resume`'s cross-project `list_all` only descends into subdirectories. Additionally two imports sharing a basename silently overwrite each other in a directory shared by every project on the machine.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SESS-S03 — Cross-project `listAll(sessionDir)` overload and the shared-dir `filterCwd` are unwired on the LISTING paths: under `--session-dir` the "all sessions" bucket is empty, and a shared dir lists other projects' sessions

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/session-manager.ts:1655-1667 — `listAll` with a `customSessionDir` takes `listSessionsFromDir(customSessionDir)` and returns; only without one does it enumerate per-project subdirs of `getSessionsDir()`. :1638-1647 `SessionManager.list` computes `const filterCwd = sessionDir !== undefined && dir !== getDefaultSessionDirPath(cwd)` and filters with `sessionCwdMatches`. Both overloads are selected on `usesDefaultSessionDir()` at interactive-mode.ts:4869-4871; main.ts:372-373 passes `sessionDir` straight into `listAll`.

**cyrup** — ABSENT. `grep -rn "list_in_dir|list_all" crates --include=*.rs | grep -v listing.rs | grep -v tests` -> production call sites are crates/cyrup/src/main.rs:966,980 `list_in_dir(&layout.dir(), None, None)`, :967,984 `list_all(&SessionsRoot(root))` with `root = dirs.session_dir`, and crates/cyrup-session-svc/src/session.rs:3104 `listing::list(&layout)` = `list_in_dir(dir, None, None)` (listing.rs:42-45). Every one passes `None` for `cwd_filter`. `grep -rn "list_all_in_dir" crates --include=*.rs` -> listing.rs:92-99 (definition), lib.rs re-export, tests/parity.rs only — ZERO production callers. `uses_default_session_dir` does not exist anywhere.

**Impact** — Under `--session-dir X`: (1) `list_all(SessionsRoot(X))` scans X's subdirectories, which a custom dir does not have, so the cross-project half of the `--resume` picker (main.rs:967,984) returns nothing where pi lists X's own sessions; (2) with a session dir shared between projects, `--resume` and `/resume` list every project's sessions unfiltered where pi shows only the current project's.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SESS-S04 — No resume-command hint on quit — pi prints the exact `--session-dir`/`--session` invocation needed to return; cyrup prints nothing

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:238-251 `formatResumeCommand(sessionManager)` builds `pi [--session-dir <quoted>] --session <id>`, adding the flag exactly when `!sessionManager.usesDefaultSessionDir()` (:246-248); guarded to a TTY, a persisted manager, and an existing on-disk file (:239-243). Emitted on every interactive quit at :3663-3666 `process.stdout.write(`${chalk.dim("To resume this session:")} ${resumeCommand}\n`)`.

**cyrup** — ABSENT. `grep -rni "to resume this session|resume this session|format_resume|resume_command" crates` -> 0 hits. `grep -rni "resume" crates/cyrup-tui/src/*.rs crates/cyrup/src/main.rs` -> only selector plumbing and the `"no saved sessions to resume"` status (app.rs:2263). The interactive quit path, crates/cyrup/src/main.rs:476-484, writes nothing: `run_interactive(...)` -> `runtime.dispose().await` -> `return Ok(0)`. `grep -rn "println!|print!|emit_stray" crates/cyrup-tui/src/app.rs` -> 0 hits.

**Impact** — A user who quits has no printed route back. Compounds with findings 1 and 3: under `--session-dir` the in-TUI `/resume` cannot see the session either, so there is no surfaced way back at all — the user must know to re-pass `--session-dir X --session <id>` and find the id by hand.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SESS-S05 — `TreeNode` drops pi's `labelTimestamp`; the `t` toggle was ported anyway and renders the literal string `"current"` on the leaf row

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/session-manager.ts:159-167 `SessionTreeNode { entry; children; label?; labelTimestamp? }`. Rendered at pi/packages/coding-agent/src/modes/interactive/components/tree-selector.ts:741-744 — `this.showLabelTimestamps && flatNode.node.label && flatNode.node.labelTimestamp ? theme.fg("muted", `${this.formatLabelTimestamp(...)} `) : ""` — inline, right after the `[label]`; `formatLabelTimestamp` (:854-878) renders `HH:MM` today, `M/D HH:MM` this year, else with a 2-digit year. Default OFF (`private showLabelTimestamps = false`, :116).

**cyrup** — ABSENT. `grep -rn "label_timestamp|labelTimestamp" crates --include=*.rs` -> 4 hits, none a field: manager.rs:238,268 and tests/parity.rs:658 are comments about `createBranchedSession`; crates/cyrup-session-svc/src/session.rs:2212-2213 is a comment BLESSING the omission ("the optional Pi `labelTimestamp` is omitted … Pi marks `labelTimestamp?` optional"). Struct: crates/cyrup-session/src/manager.rs:29-34 `TreeNode { entry, children, label }`; built at :621. The data is present and discarded — `labels: HashMap<EntryId, (String, String)>` is `(label, label-change timestamp)` (manager.rs:43-44) and `label()` at :579-581 returns `.map(|(l, _)| l)`. Keybinding and renderer WERE ported: crates/cyrup-tui/src/keymap.rs:701,711,732 (`t` -> `ToggleLabelTimestamp`), tree_selector.rs:309-310,579 `toggle_time`, :483-486 renders `node.time_label`. `grep -rn "time_label" crates/cyrup-tui/src` -> sole producer crates/cyrup-tui/src/app.rs:3577 `time_label: n.is_leaf.then(|| "current".to_string())`.

**Impact** — In `/tree`, `t` toggles a right-aligned column showing the word `current` on the leaf row and nothing elsewhere — not a timestamp, not tied to labels. pi shows per-labelled-row when the label was applied, which is the point of the feature (locating a bookmark set twenty minutes ago in a large tree). cyrup also inverts the default: `show_time: true` (tree_selector.rs:174) vs pi's `false`. No data loss (the timestamp persists; `create_branched_session` re-emits labels with original timestamps, manager.rs:266-274) — it is unreachable through the read API.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SESS-S06 — Session-listing `message_count` and search text skip any `message` entry whose payload fails to deserialize into cyrup's typed `AgentMessage`, where pi counts every `type:"message"` line

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/session-manager.ts:717-718 — `if (entry.type !== "message") continue; messageCount++;` — increments on the TAG alone. Only the text extraction below (:726-733) narrows via `isMessageWithContent` + user/assistant. Lines are parsed with a bare `JSON.parse` (`parseSessionEntryLine`), so an unrecognised or legacy role still counts.

**cyrup** — ABSENT. crates/cyrup-session/src/listing.rs:214-220 — `let Ok(Entry::Known(KnownEntry::Message { message, base })) = serde_json::from_str::<Entry>(line) else { continue }; ... message_count += 1;`. The `else { continue }` fires before the increment. `Entry`'s hand-written `Deserialize` (crates/cyrup-session/src/entry.rs:264-286) demotes a known-tag-but-unparseable body to `Entry::Unknown` rather than erroring (`Err(_) => Ok(Entry::Unknown(v))`, :278), so such an entry is silently skipped. `grep -n "message_count" crates/cyrup-session/src/listing.rs` -> :196,:220,:266 only; no tag-level count.

**Impact** — In the `/resume` and `--resume` pickers, an unmigrated v2 session (or one carrying a role this build does not model) reports a lower `N msgs` than it has, and its text is missing from `all_messages_text` — the search corpus the picker's query DSL filters on (crates/cyrup-tui/src/app.rs:2276-2283). A legacy session can be under-counted and not findable by searching for text it demonstrably contains. Bounded to legacy/foreign files; current-format sessions unaffected.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

