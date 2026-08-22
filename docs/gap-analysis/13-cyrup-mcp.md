# 13 — cyrup-mcp

`pi-mcp-adapter` is the npm package that gives pi its Model Context Protocol client: server lifecycle
over stdio and HTTP, a metadata cache, a six-source config ladder, tool registration and naming, tool
approval, an output guard, result rendering, resources and prompts, sampling, elicitation, a full
OAuth 2.1 acquisition path with OS-keychain storage, two full-screen TUI panels, two slash commands
and a protocol tracer. At v2.25.0 it is **61 production files and 21,815 lines of TypeScript**, plus
107 test files and a conformance harness. pi loads it as an extension; pi's own core knows almost
nothing about MCP.

Porting it means building `crates/cyrup-mcp` — **a native built-in crate compiled into the cyrup
binary**, the same shape as `crates/cyrup-ext-subagents` — that reproduces that behaviour against
`rmcp`, the official Rust MCP SDK, and reaches cyrup only through the extension API that already
exists.

> **Provenance.** Upstream is **`pi-mcp-adapter` v2.25.0**, tagged one day before this document was
> written; drift from the tag to its HEAD is 17 files / +543 / −69 across 4 commits. cyrup is branch
> **`david/cyrup`** and is **deliberately not pinned**. This document carries **no line numbers and no
> commit shas** — cyrup references are by symbol and file, upstream references by file and symbol. The
> previous edition pinned both and measured **37% of its cyrup line citations sitting on already-drifted
> files within a day**; a line-anchored plan is stale on arrival. rmcp is 3.1.2, read from the local
> checkout (seven commits past the `rmcp-v3.1.2` tag), and every claim about it below was checked
> against that source rather than against docs.rs.
>
> **436 port units — 22 critical · 147 high · 172 medium · 60 low · 35 n/a**, plus one `tracker`
> row excluded from every count. By verdict: 301
> hand-written, 37 `rmcp`, 34 extension-owned, 27 host-verb, 17 open-decision, 11 cut, **9
> host-addition covering exactly three distinct host surfaces**. Thirty-two rulings are open.
> (**433 at v2.25.0**; the v2.26.1 retarget of 2026-08-20 added three — MCP-027a, MCP-069a, MCP-115a
> — and the open ruling MCP-069a carries.)
> **Four surfaces are cut by owner decision** — the legacy HTTP+SSE transport, MCP Apps, the raw
> unix-socket transport, and `mcpScript`/the JavaScript worker — which removes ~14% of the package and,
> as a consequence, **every line of hand-written protocol code and every JavaScript engine question**.
>
> Counts are computed from the nine section files, which are the source of truth; where a unit's
> header carries a compound verdict (`hand-written` + `host-verb`), the census counts the
> **first-listed** one, so the seven verdict buckets sum to exactly 436. Ten units carry a
> host-addition leg somewhere in a compound verdict; they are named in full below.
>
> **IMPLEMENTATION STATUS IS NOT IN THIS DOCUMENT — it is in
> [13-cyrup-mcp-STATUS.md](13-cyrup-mcp-STATUS.md).** The canonical table below records what the port
> must BUILD; it has never recorded what is BUILT, and the `status` column added to it on 2026-08-21
> is a summary of that file, not a second source of truth. As of that audit, against v2.26.1:
> **212 units implemented · 100 partial · 98 missing · 27 not-applicable** — so **198 of 437 units
> carry open work**, including 8 of the 22 `critical` ones. The weakest surface is `13i` (protocol and
> verification, 31 of 50 units with no implementation at all); the most critical-or-high open work sits
> in `13c` (servers and the metadata cache, 23). Read the STATUS file before acting on any unit here:
> it carries the evidence, and it is explicit that no ruling in it was verified by building or running
> anything.

> **RETARGETED 2026-08-20 — the port now targets `pi-mcp-adapter` v2.26.1, not v2.25.0.** Everything
> below this blockquote was authored against v2.25.0 and is **still cited that way on purpose**. Read
> *Retarget — v2.25.0 → v2.26.1* immediately below before acting on any unit.

---

## Retarget — v2.25.0 → v2.26.1

**Dated 2026-08-20.** Upstream tagged v2.26.0 and then v2.26.1 after this plan was written. The port
target moved to **v2.26.1**; the plan did not move with it, and this section — not a rewritten body —
is the record of the difference.

### Rule 1: the v2.25.0 citations in this plan are correct and must not be blind-rewritten

This document and its nine section files cite `v2.25.0` **51 times** and reference upstream by
`file:line` throughout. Those line numbers are correct **as of the v2.25.0 tag**, which is the tree
they were read from (`git show v2.25.0:<path>` — see *Coverage · Read*). A version-string
search-and-replace would leave every one of those citations pointing at a line that has since moved,
**silently invalidating all 51** while looking like an update. It is a strictly worse state than the
honest one: a citation that says v2.25.0 and means it.

So the rule for anyone working a unit is:

1. A `file:line` in a unit body resolves against **v2.25.0**. `git show v2.25.0:<path>` is how you
   read it, exactly as the author did.
2. Before implementing, check that file against **v2.26.1** as well —
   `git diff v2.25.0..v2.26.1 -- <path>`. If it is untouched — and it is for every production file
   except the seventeen named in the delta table below — the unit is unaffected and you are done.
3. If it *is* touched, the delta table below tells you what changed and who owns it.

Only the *delta table*, the *amendments* and the *new units* in this section are written at v2.26.1.
Where an inline count elsewhere in the plan would now mislead an implementer into building the wrong
thing, it carries a short `**v2.26.1:**` annotation beside the original rather than a replacement.

### The delta, measured

`git diff v2.25.0..v2.26.1` is **42 files, +2,879 / −163**. Twenty-one of those files are tests and
account for **+1,725 / −31** on their own. Real source change is **17 files, +1,118 / −124**, and the
single largest item is one new file (`request-headers-command.ts`, 336 lines — 30% of the source
delta by itself). Seventeen changed files out of ~60 production files means **the plan's other
forty-odd file citations are untouched between the two tags**. This is one new feature plus
evolution — not a rewrite, and not a re-plan.

Four numbers in `types.ts` / `config.ts` / `metadata-cache.ts` moved, all four because of that one
feature. Verified by counting both tags, not by reading a changelog:

| | v2.25.0 | v2.26.1 | where the plan carries the old number — **annotated in place, not replaced** |
|---|---:|---:|---|
| `McpSettings` keys | 23 | **24** (`warnOnLargeDirectTools`) | `13-cyrup-mcp.md` §*What the adapter actually is* table, and *Coverage · Read* |
| `ServerEntry` fields | 28 | **29** (`requestHeadersCommand`) | same table; `13b-mcp-config.md` MCP-069 |
| `URL_BOUND_AUTH_FIELDS` | 3 | **4** (`requestHeadersCommand`) | `13b-mcp-config.md` §*mergeServerMaps* step 3 |
| `computeServerHash` identity keys | 14 (13 post-Cut-3) | **15 (14 post-Cut-3)** | `13b-mcp-config.md` MCP-070 ("11 vs 13 post-cut") |

`URL_BOUND_AUTH_FIELDS` is the one that is not merely a count: an implementer who copies the plan's
three-element array ships a **credential-leak bug**, because a higher-precedence config source that
repoints `url` would carry the lower-precedence server's request-signing command to the new endpoint.
The crate already has the four-element array; the plan's step 3 is annotated in place.

### How this was reconciled, and why most of it was already done

Cuts 1 and 2 of the port were written by agents reading the upstream **working tree**
(`v2.26.0-12-gc5dbb81`), not the v2.25.0 tag. The port is therefore *ahead of its own plan* in
several places — `dirs.rs` cites `getConfigDirName()` (`agent-dir.ts:5`), a symbol that did not exist
at v2.25.0 and that the plan never mentions. Eleven upstream changes were reconciled one at a time
against `crates/cyrup-mcp` on the rule **determine whether the crate already has it; port only what
is genuinely missing**. Seven were already present in whole or in part.

| upstream commit | subject | verdict | where |
|---|---|---|---|
| `2a2db3c` + `91f9943` | per-request HTTP header commands; header-command result union | **NEWLY PORTED** | new `src/request_headers_command.rs` (1,284 lines, 13 tests); `config.rs` (`HttpRequestHeadersCommand`, `ServerEntry` field, `URL_BOUND_AUTH_FIELDS`, `merge_entry`); `dirs.rs` (15th hash key + golden vectors); `Cargo.toml` (`http`, `sse-stream`, `nix`) |
| `76a4ea3` | suppress the large-direct-tools advisory | **NEWLY PORTED** | `config.rs` `warn_on_large_direct_tools` + accessor + merge arm; `registration.rs:1168` gate, replacing a `RESIDUAL` comment that named the exact missing key |
| `48799fa` | converge stale keep-alive tool catalogs | **MIXED** — `lifecycle.ts` (+281/−35, the substance) was already ported arm-for-arm from the working tree; the fourth `pi.on("input")` registration was **missing and is newly ported** (`registration.rs` `SUBSCRIBED_EVENTS`, `extension.rs::on_input`). Its `sendMessage` half is **not** done — see MCP-027a |
| `5bcd6c5` | scope session tool approvals to arguments | **NEWLY PORTED** as the mechanism (`state.rs::approval_cache_key`, the `server\0tool\0sha256(stableStringify(args))` triple). Its caller, MCP-232, is still an open unit; MCP-232's spec **prescribed the pre-commit bug** and was corrected in `13e-mcp-tools.md` |
| `5787ecd` | do not hang MCP panels outside TUI mode | **MOSTLY ALREADY PORTED / INAPPLICABLE.** The hang is structurally impossible here — `HostServices::open_overlay` returns `false` instead of blocking, and `ContextSnapshot::is_tui_mode()` already *is* `canRenderPanel`. The `/mcp setup` refusal string was already present; the `/mcp-auth` one was missing and is newly ported (`ui.rs::auth_panel_unavailable_message`) |
| `1bf3671` | recover nested mcp proxy args | **ALREADY PORTED**, line for line (`proxy.rs:4242-4265`). Two end-to-end tests upstream shipped with it had no counterpart and were added. One fidelity gap in its blast radius **was** fixed here: `args: null` — see *Fixed in passing* |
| `6686b12` | compact MCP input previews | **ALREADY PORTED (2 of 3) + INAPPLICABLE (1 of 3).** The leading-blank skip and the `(leading blank output omitted)` placeholder are byte-exact in `renderers.rs`. `compactInputPreview` depends on the call-row→result-row stash cut as **MCP-243**, and the defect it fixes does not exist here: `render_call` already prints the full 1,500-char argument body upstream's compact row had lost |
| `4ab5a40` | honor rebranded Pi config directories | **ALREADY PORTED / INAPPLICABLE.** All three `getConfigDirName()` call sites map to `PROJECT_OVERRIDE_DIR` + `PROJECT_OVERRIDE_NAME`, already split the post-commit way. The `piConfig` manifest half is a host-rebranding facility; cyrup **is** the rebranded distribution and resolves it at compile time — a cut recorded in `dirs.rs:26-31` before v2.26 existed |
| `faf55f7` | skip the O(tools²) startup collision scan | **MIXED** — `registration.rs` had it at the *post*-commit shape for both call sites; `proxy.rs`'s second `build_proxy_description` (which upstream does not have) was only half-gated and is **newly fixed** (`collision_candidates`, `server_has_tool_filters`) |
| `14c0e6c` | share filtered selector candidate scans | **ALREADY PORTED** (`registration.rs::CandidateIndex` is `createToolSelectorCandidateIndex`, count-based self-subtraction and all). The tests upstream shipped with it had no counterpart and were added. `ui.rs` correctly still takes the `Set` branch — `mcp-panel.ts` does too |

**Net: 4 newly ported, 2 mixed (part ported here), 4 already ported or inapplicable.** No unit in the
plan was invalidated; four were amended and three new ones were filed.

### Fixed in passing — two defects the reconciliation surfaced

Both were found by reading v2.26.1 against the crate, neither belongs to any single upstream commit,
and both are closed:

1. **`renderers.rs` — a reachable `usize` underflow.** `tool-result-renderer.ts:383` lets
   `remainingChars` fall to `-1` and catches it next call with `remainingChars <= 0` (`:348`). `usize`
   has no `-1`: a pushed line exactly as long as the remaining budget wrapped to `usize::MAX`, which
   the port's `remaining == 0` guard never satisfies, so the char budget silently became unbounded in
   release and **panicked in debug** — reproduced at `renderers.rs:1703`, against a crate that denies
   `panic`. `saturating_sub` lands on `0`, which is the same guard upstream's `-1` trips. Pinned by
   `a_line_that_lands_exactly_on_the_char_budget_does_not_underflow`.
2. **`proxy.rs` — `mcp({ args: null })` answered with status instead of throwing.** Serde folds a
   present JSON `null` into `None`, erasing the `!== undefined` distinction that `parseArgs`
   (`index.ts:880-882`) and `1bf3671`'s own rescue arm (`index.ts:903`) both depend on. Closed with a
   `present_value` deserializer on the one field where presence is load-bearing. Pinned by
   `an_explicit_null_args_is_thrown_where_an_absent_args_is_status`.

### Three new units — filed, NOT done

These are the parts of the delta that are genuinely missing and are **not** inline fixes. They are
real, open work; nothing below is implemented.

| id | severity | file | why it is a unit and not a patch |
|---|---|---|---|
| **MCP-069a** | **critical** | `13b-mcp-config.md` | a malformed `requestHeadersCommand` **fails open** — the server connects unsigned where upstream refuses. Closing it reverses `config.rs`'s rule 4 for one field, so it needs the port owner's ruling, not a patch |
| **MCP-115a** | high | `13c-mcp-servers.md` | the `RequestHeadersCommandClient` decorator is built and tested but has **no caller**: `connectHttpClient` has no Rust counterpart yet. Blocked on an unported module, not deferred |
| **MCP-027a** | medium | `13a-mcp-activation.md` | `sendMessage`'s `triggerTurn` pre-turn convergence gate is **inexpressible** in today's `SendMessage` type alias, which takes no options at all |

### Four amendments to existing units

- **MCP-232** (`13e`) — its spec prescribed the pre-`5bcd6c5` two-part key and said the difference "is
  not observable". At v2.26.1 it is: the key is a triple including an args hash. Corrected in place.
- **MCP-207** (`13e`) — `buildToolMetadata` must be ported in its post-`14c0e6c` form, with
  `additionalCurrentCandidatesByToolName` and the `hasCandidate` / `totalMatchingCount` arms. Porting
  the v2.25.0 shape then optimising is a second port, not a first.
- **MCP-069 / MCP-070 / `mergeServerMaps` step 3** (`13b`) — annotated with the four moved counts.
- **§*What the adapter actually is*** and **§*Coverage · Read*** (this file) — annotated likewise.

### What is still outstanding, stated plainly

Beyond the three units above: `request_headers_command.rs` records **five deliberate divergences** in
its own module header (a per-connection `CancelToken` for a per-request `AbortSignal`; rmcp's refusal
of reserved header names; a derived `Authorization` that *appends* to `auth_header` rather than
replacing it; case-variant duplicate header names resolving last-wins rather than comma-joined; and
one error sentence with no upstream fixed text). The `Authorization` one is a real behavioural
difference — a bearer-configured server would send two `Authorization` headers — and it is folded into
**MCP-115a**, because there is no connect path to observe or test it through until that unit lands.

`proxy.rs::index_has_other_current_match` has `14c0e6c`'s semantics without its memo tables, so it
recompiles the glob per **(tool, pattern)** pair. Cost only, never behaviour; the planned close is
deletion in favour of `registration.rs::CandidateIndex` when MCP-207 collapses the two selector paths.

---

## The thesis: it is an extension, and the port changes nothing in core

This is the defining property of the thing being ported, and it survives the port.

**1. Upstream is an extension, and cyrup's extension tier is where it lands.** `pi-mcp-adapter` is an
npm package pi loads at startup. `crates/cyrup-mcp` is a native built-in crate attached in
`crates/cyrup/src/main.rs` at the **three session-build arms** through
`SessionFactory::with_native_extension` (`crates/cyrup-session-svc/src/factory.rs`;
`SessionBuilder` carries the same method), and loaded by the session builder through
`ExtensionHost::load_native_with_services`. That is exactly how `cyrup-ext-subagents` and
`cyrup-permission-system` attach today, both of them ports of pi npm packages.

**2. A native extension is not sandboxed. This is the load-bearing correction.** `HostServices` is
the capability surface a **WASM guest** is confined to. A native crate links `rmcp`, `tokio`,
`keyring`, `reqwest`, `opener` and the filesystem **directly**, and reaches for `HostServices` only
where it genuinely touches the host: drawing UI, notifying, reading session state, honouring
cancellation, registering tools and commands. The working precedent is unambiguous —
`cyrup-ext-subagents` spawns real OS subprocesses with `tokio::process`, escalates signals with
`nix`, validates JSON Schema with `jsonschema`, reads `cyrup_provider::catalog` directly and paints
with `ratatui`, none of it through `HostServices`. **"`HostServices` has no X" is therefore almost
never a blocker** — only when X is a host concern the extension cannot legitimately do itself:
mutating the agent's live tool array, drawing into the terminal, prompting the one human.

**3. The extension surface is already rich enough.** `InitApi` offers `register_tool`,
`register_command`, `register_tool_renderer`, `register_message_renderer`, `register_entry_renderer`,
`register_shortcut`, `register_flag`, `add_autocomplete`, `add_autocomplete_provider`, `subscribe`,
`subscribe_bus`, `subscribe_terminal_input`. `ExtensionHost` offers `register_late_tool`,
`refresh_tools`, `active_tools` — with the caveat that `refresh_tools` does not currently answer for
the native tier, which is Finding 1. `HostServices` offers the five dialog verbs, `oauth_prompt` /
`oauth_select`, `open_overlay`, `notify`, `set_status`, `set_widget`, `set_header` / `set_footer` /
`set_title`, `inject_message`, `append_entry`, the session/model/context accessors,
`human_interaction_lock`, `is_project_trusted`, `active_tools` / `all_tool_names` /
`set_active_tools` / `all_tools`, `commands`, `control`, `exec`, the `http_request` family and the
`proc_spawn` family. The seam table below maps every in-scope adapter capability onto one of them, or
onto the native crate's own dependencies.

**4. What actually survives as a core change: three surfaces, ten port units, all small.**

| id | surface | size | who else benefits | consequence if not built |
|---|---|---|---|---|
| **HA-1** | A native extension has no **handle** to `ExtensionHost::register_late_tool`; there is no `register_late_command` sibling; **and the path from that handle to the live agent is broken in the default build** (Finding 1 below) | **M, across two crates** — one defaulted `NativeExtension::set_ext_host(Weak<ExtensionHost>)` called beside the existing `set_host_services`, plus the command sibling, **plus a one-line fix to `ExtensionHost::refresh_tools`** | every native extension; closes a two-tier asymmetry with the WASM `registration` WIT import, and repairs a latent defect that today has no callers to expose it | on a cold cache the first session exposes only the `mcp` proxy tool; direct tools and prompt commands appear next session. `mcp({connect})` cannot surface tools mid-session; the proxy tool's description is frozen for a session; `settings.disableProxyTool` becomes unsupported. **Building only the handle changes none of that** — see Finding 1 |
| **HA-2** | Extension slash commands have no argument completions — no native dispatch arm and no TUI consumer | **M** — a defaulted `NativeExtension::argument_completions`, a non-`wasm-host` arm on `ExtensionHost::command_completions`, and one call from `cyrup-tui`'s `autocomplete::slash_context` | any extension command with arguments; `InitApi::add_autocomplete` is a write-only surface today | `/mcp reconnect <TAB>`, `/mcp logout <TAB>`, `/mcp disable <TAB>`, `/mcp enable <TAB>` do not complete server names and `/mcp <TAB>` does not list the eight subcommands. Every command still works typed in full |
| **HA-3** | `HostServices::open_overlay` carries no geometry options: `ExtensionOverlay::box_rect` hardcodes one rect, and the extension cannot request the panels' 82/92 columns or learn the height it was clipped to | **S** — an `OverlayOptions` argument whose `Default` is today's constants | any extension overlay with a fixed content width | **cosmetic.** The host draws no border — it `Clear`s a rect and paints the extension's lines — so the panels centre their own content inside the default 95% rect and look correct. The only visible difference is that the `Clear` erases the transcript across the full 95%. The *height*-clip half is not the host's problem and is assigned to the panels themselves |

Port units, in full. **HA-1**: `MCP-037` (the handle) and **`MCP-037a`** (the `refresh_tools` fix,
Finding 1), with `MCP-039` / `MCP-152` / `MCP-193` / `MCP-217` / `MCP-395` the same seam seen from
five subsystems. **HA-2**: `MCP-041`, and `MCP-382` is the same addition seen from the TUI — one
addition, filed twice, not two. **HA-3**: `MCP-368`, which owns the geometry half and explicitly
disowns the height-clip half to `MCP-366` and `MCP-377`. Nine of those ten count as `host-addition` in
the verdict census; `MCP-152` reads as `hand-written` first and carries an HA-1 leg.

**5. The previous edition's headline claim was half wrong — and so, it turns out, was the correction.**
The previous edition filed dozens of "cyrup-side prerequisites", rated several `critical`, and led
with **"there is no way for a native extension to register a tool after init"**. That overstated the
gap. The first correction to it — that the mechanism is complete and live, and *only* the handle is
missing — understated it, because it was assembled by reading each function on its own. Composed
across the `wasm-host` cfg boundary, the chain breaks one step past `register_late_tool`. The accurate
statement is Finding 1 below, and the accurate scope of HA-1 is **one defaulted method plus a one-line
fix**, in two crates.

What *is* true, and is the reason HA-1 is still small: everything downstream of `refresh_tools`
exists and is wired. `ExtensionHost::register_late_tool` (`crates/cyrup-ext/src/facade.rs`) inserts
the `Arc<dyn Tool>` into the registry's executable tool map and raises the tools-dirty flag;
`ExtensionHost::active_tools` re-materialises and wraps; `AgentSession::refresh_extension_tools`
(`crates/cyrup-session-svc/src/session.rs`) merges into `DynamicToolState`, auto-activates new names
and calls `AgentSession::push_active_tools`, which rewrites `Agent::set_tools` **and** the system
prompt — driven from `AgentSession::next_turn_tools`, i.e. **at every turn boundary inside a live
run**. The break is the one link in the middle, and `register_late_tool` has **zero callers anywhere,
production or test**, which is why nothing has ever noticed.

### Finding 1 — late tool registration does not reach the agent in the default build

**This is a defect in cyrup that this analysis found, not a gap in the port.** It is filed here rather
than buried in a unit because it invalidates a claim the previous edition of this document made in
bold, and because it changes the size and the shape of the port's one load-bearing host addition.

The chain from `ExtensionHost::register_late_tool` to the running agent, composed rather than read
function by function:

1. `register_late_tool` → `ExtensionRegistry::register_tool` → `register_tool_inner` inserts the tool
   into `g.tools` / `g.tool_order` and raises the tools-dirty flag. **This part is exactly as
   documented, for both tiers** — `register_tool_inner` is the common path, and the flag it raises
   says nothing about which tier raised it.
2. `ExtensionHost::refresh_tools` returns `Ok(false)` early unless `take_tools_dirty()`, and otherwise
   returns **the value of `materialize_guest_tools()`** — the *materializer's* verdict, not the flag's.
3. `materialize_guest_tools` has two cfg arms. The native-only arm
   (`#[cfg(not(feature = "wasm-host"))]`) is a no-op returning `Ok(true)`. The **`wasm-host` arm
   iterates `ExtensionRegistry::guest_tool_entries` only** — `g.guest_tools` / `g.guest_tool_order`,
   a **different map** from the `g.tools` a native tool was inserted into. With no new *guest*
   descriptor to wrap, it reports `Ok(false)`.
4. `crates/cyrup-ext`'s manifest sets `default = ["wasm-host"]`, so **the production build takes the
   wasm-host arm.**
5. `AgentSession::refresh_extension_tools` hard-gates on that bool — `Ok(false) => return`, with no
   diagnostic — and so never reaches `active_tools` / `merge_registered` / `push_active_tools`.
6. `take_tools_dirty()` is a `swap(false)`, so the signal is **consumed and destroyed**; a later turn
   has nothing to re-read.

The consequence is precise: with HA-1's handle built and nothing else, `register_late_tool` returns
`Ok(())`, the tool sits in the registry, the `/mcp` panel lists it with its toggle on, `refresh_tools`
answers "nothing changed", and **the model cannot call it for the rest of the session**. It reappears
at the next session build, where `ExtensionHost::active_tools` is called directly by the session
builder rather than through the gated refresh — which is exactly the cold-cache degradation this
document already describes for *not* building HA-1 at all. In other words, the handle alone buys
nothing.

**The fix, in `crates/cyrup-ext/src/facade.rs`:** run the guest materializer for its side effects but
return `true` on the strength of the flag that was actually taken, because the dirty flag is raised by
**both** tiers' `register_tool_inner` while the materializer reports only on guests. That is one line,
and it makes the native and `wasm-host` builds agree. It is `MCP-037a`, and its verify line is the
only one in the port that must run twice: register a native tool from a live handler and assert it
reaches `agent.tools()` on the next turn, **with `--features wasm-host` and without**.

**Why nobody caught it.** Every function in the chain is correct in isolation and carries a doc
comment that is true in isolation. `refresh_tools`'s comment describes re-materialising *guest*
descriptors; `register_late_tool`'s comment describes the *native* analog of the guest import. Only
composing them across the cfg boundary shows that one consumes what the other produces through the
wrong map. The reason it is latent rather than live is that `register_late_tool` has no callers —
`cyrup-mcp` would be the first.

**6. One wiring gap that is not a host addition, and that nothing else in the directory records.**
`LiveHostServices` (`crates/cyrup-session-svc/src/host_services.rs`) implements ~50 `HostServices`
methods and **`is_run_cancelled` is not among them**, so the documented substitute for pi's
`ctx.signal` returns the trait default `false` forever in production. The WASM bridge in
`crates/cyrup-ext/src/host/live.rs` forwards to the same trait method, so **both tiers are affected**.
`tools_expanded` has the identical shape. Size: one method body each. Dangerous precisely because it
compiles, returns a plausible value and looks correct — `MCP-007`, `MCP-033`, `MCP-040` and `MCP-046`
all read it.

**Everything else the previous edition filed as a prerequisite dissolved.** The full list is in
Coverage; the load-bearing ones are that a nested completion for sampling reaches `cyrup-provider`
directly (upstream bypasses pi's host API too — a host verb here would be the *divergence*), that the
tool-approval broker is `ExtHooks::before_tool_call` plus `cyrup-permission-system`'s existing MCP
target derivation, that `--mcp-config` is read from `std::env::args()` because upstream reads
`process.argv`, and that the MCP status snapshot needs no bus verb because cyrup has no consumer for
one.

---

## Scope

Four surfaces are **cut by decision of the project owner**. They are not phase 2, not open questions
and not future work. They are recorded here with their reasons so a later pass does not re-file them
as gaps.

### CUT 1 — the legacy HTTP+SSE transport

The 2024-11-05 two-endpoint shape (GET `/sse` → `endpoint` event → POST). **Supported transports are
exactly `stdio` and `streamable HTTP`.**

The reason is stronger than a preference: **rmcp 3.1.2 ships no SSE client transport at all.**
`crates/rmcp/src/transport.rs` exports `TokioChildProcess`, `StreamableHttpClientTransport` and
`UnixSocketHttpClient` on the client side and nothing else; there is no `SseClientTransport` type and
no `transport-sse-client` feature anywhere in `crates/rmcp/src/`. (`client-side-sse` is only the SSE
*frame parser* the streamable-HTTP client uses.) Supporting the legacy transport would mean
hand-writing a protocol transport — the exact thing the dependency decision exists to avoid.

Goes with it: `server-manager.ts`'s `shouldFallbackToSse` 404/405/406/415 downgrade probe, the
`SSEClientTransport` construction inside `connectHttpClient`, the `httpTransport === "sse"` branch,
and `traceTransportKind`'s `"sse"` variant. **Kept:** `ServerEntry.protocolVersion` is protocol-*era*
negotiation, not transport, and maps 1:1 onto rmcp's `ClientLifecycleMode`.

### CUT 2 — MCP Apps / the UI extension, entirely

`ui-server.ts`, `ui-session.ts`, `host-html-template.ts`, `ui-resource-handler.ts`,
`ui-stream-types.ts`, `ui-app-bridge-helpers.ts`, `app-bridge.bundle.js`, `glimpse-ui.ts`,
`consent-manager.ts`, every `ui://` resource path, the local HTTP host server, the iframe bridge
protocol, and `@modelcontextprotocol/ext-apps`. **2,487 lines across nine files.**

Consequences to propagate: **no `axum`**, no local HTTP *server* for apps, no app-initiated tool
calls, `McpToolApprovalOrigin` loses `"iframe"`, and the tool-result renderer handles the standard MCP
content types but not `ui://` resources. The OAuth loopback callback listener is a **different**
server and stays.

### CUT 3 — the raw unix-socket transport

`unix-socket-transport.ts` and `ServerEntry.socket`. rmcp ships `UnixSocketHttpClient`
(`transport/common/unix_socket.rs`), but that is **streamable HTTP over a UDS** — a different wire
shape from the adapter's raw framed socket, which targets `rmcp-mux`. rmcp does not ship the
adapter's shape, and stdio plus streamable HTTP cover the field.

### CUT 4 — `mcpScript` / the JavaScript worker

`mcp-code.ts`, `mcp-script-worker.mjs`, `skills/mcp-scripting/SKILL.md`, the `mcpScript` tool
registration, `McpSettings.scriptMode`, and `McpToolApprovalOrigin`'s `"script"` variant.

**This removes the only JavaScript-engine question in the entire port.** No `rquickjs`, no vendored
C, no `boa`, no JS-in-WASM. Combined with Cut 2, **`node` is not a production dependency of
`cyrup-mcp`** — the two places it looked unavoidable are both resolved: the keyring recovery path
re-execs `std::env::current_exe()` instead (the mechanism being ported is "re-run the keyring op
inside a fresh `keyctl` session over stdin/stdout JSON", not "run node"), and the npx force-cache argv
`npm exec --yes --package <spec> -- node -e 1` spawns a *third-party* Node MCP server's package
manager, which is what MCP is. Coverage is preserved: `mcp({search})` → `mcp({describe})` →
`mcp({tool, args})` is the same discover/inspect/call loop, one call per turn instead of batched.

### The two consequences that matter most

**(a) The port hand-writes no MCP protocol code at all.** rmcp's only two gaps relative to the
adapter are the legacy SSE transport and the raw framed unix socket — which are exactly two of the
four cuts. **The cut boundary and the SDK boundary coincide.**

**(b) There is no JavaScript engine and no JavaScript runtime dependency anywhere in the port.** Do
not raise one, for anything.

### What remains, and how much of the package that is

| | files | lines | share |
|---|---:|---:|---:|
| in scope | 50 | **18,759** | **86%** |
| Cut 2 — MCP Apps | 9 | 2,487 | 11% |
| Cut 4 — mcpScript | 2 | 484 | 2% |
| Cut 3 — unix socket | 1 | 85 | <1% |
| **total at v2.25.0** | **61** | **21,815** | |

In scope: server lifecycle and connection management over stdio and streamable HTTP, the metadata
cache, config, tool registration and naming, tool approval, the output guard, result rendering,
resources and resource templates, prompts, structured content and output schemas, progress and
cancellation, sampling, elicitation, the full OAuth 2.1 acquisition and storage path, the OS
keychain, the two TUI panels, slash commands, status and notifications, and tracing.

### Where a cut surface is entangled with an in-scope one

| file | the seam |
|---|---|
| `ui-tool-visibility.ts` | **split, not cut.** `extractUiToolVisibility` and `isUiToolVisibleToModel` are **kept** — `direct-tools.ts` uses the predicate in three places and `tool-metadata.ts` in one, and dropping it would **expose to the model tools the server explicitly marked app-only**. Only `isUiToolCallableByApp` is cut, having no caller |
| `proxy-modes.ts` | ten dispatch arms become nine. Only `executeUiMessages` is cut; `status`, `list`, `search`, `describe`, `instructions`, `connect`, `call`, `auth-start`, `auth-complete` all stay in relative order. An `action:"ui-messages"` call now falls through to `executeStatus`, the same fall-through an unrecognised action already had |
| `direct-tools.ts` | cut the UI-session interleave (`maybeStartUiSession`, `summarizeUiSessionResult`, `sendToolResult`, `sendToolCancelled`, the `reused` close, the `_meta: uiSession?.requestMeta` injection, result branch 10c). The remaining executor keeps its **full ordering**: disabled check → owned-signal composition → `lazyConnect` → auto-auth on `needs-auth` → connection assertion → `ensureToolCallApproved` → request options → `withSessionRecovery`-wrapped `tools/call` → content transform → output guard → error/abort mapping → in-flight decrement |
| `tool-result-renderer.ts` | **survives whole minus one branch.** `action === "ui-messages"` in `formatMcpProxyToolCallLines` is the file's only UI reference and it contains no `ui://` code at all — that lived in `ui-resource-handler.ts` |
| `buildProxyDescription` | **model-facing text that must be edited, not merely trimmed.** Two removals: the header sentence naming `mcpScript`, and the `mcp({ action: "ui-messages" })` usage line. Every other byte, including the usage block's column alignment, stays identical — the string is a prompt-cache key |
| the `mcp` tool schema | the `action` property's description must narrow from `Action: 'ui-messages', 'auth-start', or 'auth-complete'` to `Action: 'auth-start' or 'auth-complete'`. Leaving it advertises a mode that now silently falls through |
| `consent-manager.ts` | **cut with Cut 2**, correcting the seam map's `A-5` row and its file table. A grep at v2.25.0 shows `ConsentManager`'s only consumers are `ui-server.ts` and `ui-session.ts`. The surviving approval surface is a **different actor on a different path**: `tool-approval.ts`'s local gate — the `approveTools` config, the session `approvedToolCalls` cache, the three-way select — which ports whole |
| `errors.ts` | five of seven classes go with Cut 2, and `wrapError`'s only production caller with them. The surviving taxonomy is the base shape plus `McpServerError` |
| `logger.ts` | every production `warn` (2) and `error` (5) site lived in an Apps file, as did three of the four child loggers. The port still maps all four levels onto `tracing` — the level filter is the user-facing contract, not the current call distribution |
| `mcp-probe.ts` | **survives whole and gains an arm.** All three strategies hit the *same* URL (legacy-sse is a GET with `Accept: text/event-stream`, not a separate path), so the probe is unaffected — but under Cut 1 a legacy-SSE-only endpoint would be classified "responded with an MCP event stream" while the connect fails. The port adds one ladder arm naming the unsupported transport. A new string, recorded as a divergence |
| `computeServerHash` | the socket **transport** is cut, but the `socket` **key stays in the 14-key hash pre-image**. A cyrup config can never carry a socket value, so it always hashes as absent — as it does for the vast majority of upstream servers. Keeping it costs one always-absent field; dropping it changes the digest for **every** server and voids the golden-vector fixture. Same reasoning for `protocolVersion` |
| the cache schema | `uiResourceUri` and `uiStreamMode` become dead and are **not written**, but the names stay reserved and **`CACHE_VERSION` must not move** — `cyrup-ext-subagents` already reads this file. `uiVisibility` is **not** merely reserved: it is still written and still read by the `isUiToolVisibleToModel` filter |
| config load | `httpTransport: "sse"` and `socket` must each produce a **named load-time diagnostic**, never a silent drop. `agent-plugin-loader.ts` sets `httpTransport` straight from a manifest's `type: "sse"`, so a plugin declaring it is a live case that would otherwise appear configured and never connect |
| `glimpse-ui.ts` | **ruled on.** A macOS-only native-webview launcher that locates the `glimpseui` npm package's binary and opens a window containing an `<iframe src="{handle.url}">`. Its only caller is `ui-session.ts`. A pure MCP-Apps viewer — **cut entirely** |
| the conformance matrix | the `sse-retry` scenario is SSE stream resumption **inside** streamable HTTP — rmcp's own conformance client runs it with `StreamableHttpClientTransport`. It stays, and must not be dropped with the legacy transport |
| the test census | 12 of 96 `__tests__` files pin cut surfaces only; `ui-tool-visibility.test.ts` **splits at the function boundary** (2 cases port, 1 does not). 84 vitest files and 5 `node:test` files remain in scope, and the fixture inventory drops to **eight** servers |
---

## What the adapter actually is

Sizes are lines at v2.25.0. "Load-bearing" means a defect here is visible to the user or the model on
a normal path; "peripheral" means it degrades an affordance.

| subsystem | files | lines | weight | note |
|---|---|---:|---|---|
| **Proxy modes** | `proxy-modes.ts`, `search-ranking.ts` | 1,537 | load-bearing | the single largest file in the package. After the cuts this is the *whole* model-facing surface on a cold cache |
| **Config** | `config.ts`, `agent-plugin-loader.ts`, `agent-dir.ts`, `onboarding-state.ts` | 1,739 | load-bearing | six-source ladder, seven host-config import families, provenance, conflicts, six writers with preview twins |
| **Server manager & transports** | `server-manager.ts`, `session-recovery.ts`, `lifecycle.ts`, `runtime-owner.ts`, `abort.ts`, `mcp-probe.ts`, `mcp-trace.ts` | 2,000 | load-bearing | five race guards, generation fencing, four lifecycle modes, terminated-session recovery |
| **OAuth & keychain** | `mcp-auth.ts`, `mcp-auth-flow.ts`, `mcp-oauth-provider.ts`, `mcp-callback-server.ts`, `oauth.ts`, `oauth-handler.ts`, `mcp-keyring-helper.cjs` | 3,236 | load-bearing | the second-largest subsystem; collapses hardest onto rmcp, but the storage half does not collapse at all |
| **TUI panels** | `mcp-panel.ts`, `mcp-setup-panel.ts`, `panel-keys.ts` | 1,734 | peripheral | fully interactive: server list, tool tree, fuzzy filter, token estimates, direct-tool toggles, save |
| **Types & naming** | `types.ts` | 859 | load-bearing | 23 settings keys, 28 server-entry fields, four prefix modes, the 18-expression candidate set, glob filtering (**v2.26.1: 24 and 29** — `warnOnLargeDirectTools`, `requestHeadersCommand`) |
| **Activation** | `index.ts`, `init.ts`, `state.ts` | 1,609 | load-bearing | the extension entry, `initializeMcp`, the lifecycle generation counter |
| **Commands** | `commands.ts` | 627 | load-bearing | `/mcp` eight-way switch, `/mcp-auth`, the non-TUI text listings |
| **Direct tools & registration** | `direct-tools.ts`, `tool-registrar.ts`, `tool-metadata.ts`, `resource-tools.ts`, `ui-tool-visibility.ts` (kept half) | 1,175 | load-bearing | the executor state machine, binary-resource materialisation, `buildProxyDescription` |
| **Rendering & guard** | `tool-result-renderer.ts`, `mcp-output-guard.ts` | 831 | load-bearing | byte/line caps, spill-to-file, compact/boxed shells |
| **Metadata cache** | `metadata-cache.ts` | 353 | load-bearing | **the one file with an existing Rust reader in cyrup** |
| **Prompts** | `prompts.ts` | 353 | peripheral | prompt→command naming, positional + `key=value` args, result formatting |
| **Sampling & elicitation** | `sampling-handler.ts`, `elicitation-handler.ts` | 632 | load-bearing | the two server→client request paths; both hold a human dialog |
| **Approval** | `tool-approval.ts` | 182 | load-bearing | glob matching, the three-way select, the session cache |
| **Support** | `utils.ts`, `errors.ts`, `logger.ts`, `ts-shape.ts`, `json-schema-validator.ts`, `error-signal.ts`, `mcp-status.ts`, `npx-resolver.ts` | 1,595 | mixed | `npx-resolver.ts` is **already ported** into `cyrup-ext` |
| **CLI shim** | `cli.js` | 209 | peripheral | never loads pi; scaffolds compatibility imports. Becomes a visible `cyrup mcp init` verb |

Three subsystems the brief lists as in scope have **nothing to port**: a grep over the whole package
at v2.25.0 finds **zero** occurrences of `roots`, of `logging/setLevel` / `notifications/message`, and
of `completion/complete`. The adapter implements none of them. rmcp ships all three. Wiring them
would be **new functionality, not a port**, and is outside the 1:1 parity mandate. Resource
*subscriptions* are the same story. Recorded here so a later pass does not file them as gaps.

---

## The dependency decision: `rmcp` 3.1.2, client-only

```toml
rmcp = { version = "3.1.2", default-features = false, features = [
  "client",
  "transport-child-process",
  "transport-streamable-http-client-reqwest",
  "reqwest",
  "auth",
] }
```

Read from `crates/rmcp/Cargo.toml`. Apache-2.0, the official Rust MCP SDK. Five notes, each of which
changes something:

- **`default-features = false` is mandatory.** rmcp's `default = ["base64", "macros", "server"]`, and
  `server` pulls `transport-async-rw`, `schemars`, `pastey` and `uuid` for a role the adapter never
  plays — the adapter is a client and never runs an MCP *server*. `base64` returns transitively
  through `client-side-sse`, so nothing is lost.
- **`transport-child-process`** pulls `process-wrap` and `tokio/process`; `process-wrap` is **new to
  the lock file**. `transport-streamable-http-client-reqwest` pulls `sse-stream`, also **new**.
- **Name `reqwest` explicitly.** Both `auth` and the streamable-HTTP feature select only the private
  `__reqwest`, which turns the dependency on without choosing a TLS backend. cyrup's workspace
  `reqwest` already enables `rustls` and Cargo unifies features — but relying on another crate's
  feature selection for your TLS backend is not a contract, and naming it costs nothing.
- **`auth`** = `["dep:async-trait", "dep:oauth2", "__reqwest", "dep:url"]`. **`oauth2` 5.0.0 is
  already in cyrup's lock file** and rmcp requires `"5.0"` — no new resolution surface.
- **`elicitation` is NOT needed**, correcting a widely-held assumption. The feature is `["dep:url"]`
  and every `#[cfg(feature = "elicitation")]` in the tree is server-side. The whole client half —
  `ClientHandler::create_elicitation`, `ElicitRequestParams::{FormElicitationParams,
  UrlElicitationParams}`, the `ElicitationSchema` primitive family, `ElicitResult`,
  `ElicitationAction`, and `ElicitationCapability::{with_form, with_url}` — is **unconditional under
  `client`**. Enabling it would add `url` for nothing.

**The consequence of `default-features = false` that costs code, and that a reader will hit on their
first line.** `ClientCapabilities` in `crates/rmcp/src/model/capabilities.rs` is `#[non_exhaustive]`,
and its `builder()` is generated by a `builder!` macro guarded by
`#[cfg(any(feature = "server", feature = "macros"))]` — **neither of which this feature set selects.**
So both forms a reader reaches for first fail to compile: `ClientCapabilities::builder()` does not
exist in this build, and a struct literal is forbidden outside the defining crate for a
`#[non_exhaustive]` type. The working form is `Default` plus field assignment, because the type derives
`Default` and every field is `pub`:

```rust
let mut caps = rmcp::model::ClientCapabilities::default();
caps.sampling = Some(sampling);          // built the same way
caps.elicitation = Some(elicitation);
```

The same rule applies to **`SamplingCapability`** (also `#[non_exhaustive]`, also `Default`, fields
`tools` and `context`) and to **`ElicitationCapability`**. Two auth types are `#[non_exhaustive]`
*without* a `Default`: **`StoredCredentials`** and **`StoredAuthorizationState`** (both in
`crates/rmcp/src/transport/auth.rs`) must be built through their constructors —
`StoredCredentials::new(client_id, token_response, granted_scopes, token_received_at)` then
`.with_issuer(…)`, and `StoredAuthorizationState::new_with_expected_issuer(…)` then
`.with_requested_scopes(…)`. This affects `MCP-291`'s `CredentialStore` / `StateStore` impls and
`MCP-290`'s DCR record directly: those types cross the trait boundary and cannot be assembled
literally.

**Sampling is soft-deprecated in 3.1.2 and it does not change the port.** `SamplingCapability`'s doc
comment reads "Deprecated by SEP-2577; remains functional and will be removed in a future release",
and `ClientCapabilitiesBuilder::{enable_sampling_tools, enable_sampling_context}` carry
`#[deprecated(since = "1.8.0")]`. Neither is reachable from this feature set — the builder is behind
`server`/`macros` — so nothing in the port emits a deprecation warning, and the capability still
functions end to end: `ClientHandler::create_message` is not deprecated, and the sampling wire path is
unaffected. **The sampling units' severity is unaffected**, `MCP-455` included. This is the third such
notice in 3.1.2, after the logging (`Peer::set_level`) and roots deprecations already recorded above;
it is written down so that a future `rmcp` bump that finally removes it is recognised as the breaking
change it will be, rather than rediscovered.

`which-command` is a judgement call and stays **off**: it would give rmcp's PATH resolution, but
`cyrup_ext::caps::proc::npx_resolver` already does far more than `which` for the one command shape
that needs it. `DEPENDENCY_POLICY.md` constrains nothing downstream — it is rmcp's own
selection/Dependabot/MSRV policy; the one clause worth carrying is "anything not needed by every user
should sit behind a Cargo feature", which is why the list above is as narrow as it is.

### Why taking the SDK beats hand-rolling

The adapter's protocol surface is the initialize handshake and revision negotiation, the tools /
prompts / resources / resource-templates cursor loops, `tools/call` with `_meta` and progress tokens,
`notifications/cancelled`, the three list-changed notifications, `sampling/createMessage`,
`elicitation/create` in two modes, structured content and output schemas, and an OAuth 2.1 client
covering RFC 9728, RFC 8414/OIDC, RFC 7591, PKCE S256, RFC 8707 and RFC 9207. `crates/rmcp/src/transport/auth.rs`
alone is **8,235 lines**. Hand-rolling that is a second project, and every line of it is a conformance
liability the SDK already carries.

### `ClientHandler` — the exact trait `cyrup-mcp` implements

From `crates/rmcp/src/handler/client.rs`. Every method has a default, so `cyrup-mcp` overrides only
what it uses. The trait is `Sized + Send + Sync + 'static`, is blanket-implemented as
`Service<RoleClient>`, and has `impl ClientHandler for Box<T>/Arc<T>` — so `cyrup-mcp` holds one
handler type per server connection and shares state through `Arc`.

| method | kind | `cyrup-mcp` |
|---|---|---|
| `get_info() -> ClientInfo` | info | **override** — name, version, and a `ClientCapabilities` carrying `sampling` and `elicitation { form, url }`. Built with `default()` + field assignment, **not** a struct literal and **not** `builder()` — see the dependency note above |
| `ping` | request | default |
| `create_message` | request | **override** — sampling |
| `create_elicitation` | request | **override** — form + url |
| `list_roots` | request | default (empty) — upstream has no roots |
| `on_custom_request` | request | default (method-not-found) |
| `on_cancelled` | notification | **override** — trace / abort bookkeeping |
| `on_progress` | notification | **override** — tool-update stream |
| `on_logging_message` | notification | default — upstream has none |
| `on_resource_updated` | notification | default — upstream never subscribes |
| `on_resource_list_changed` | notification | **override** |
| `on_tool_list_changed` | notification | **override** |
| `on_prompt_list_changed` | notification | **override** |
| `on_subscriptions_acknowledged` | notification | default |
| `on_task_status` | notification | default — upstream rejects sampling tasks |
| `on_custom_notification` | notification | **override** — `notifications/elicitation/complete` |

One shape difference worth ~20 lines: upstream registers list-changed handling through the TS SDK's
`listChanged: { tools: { onChanged }, … }` client option, which **re-fetches and hands back the new
list**. rmcp's `on_*_list_changed` is a bare notification, and `Peer<RoleClient>` **invalidates its own
response cache** on it, so the handler re-calls `list_all_*` itself. A glue difference, not a
capability difference.

### What rmcp actually provides — verified against the checkout, with corrections

| claimed | verdict | evidence |
|---|---|---|
| client role | **yes** | `service/client.rs`: `RoleClient`, `Peer<RoleClient>`, `RunningService` |
| stdio child process with `env`, `cwd`, stderr capture | **yes** | `transport/child_process.rs`: `TokioChildProcess::new` / `TokioChildProcessBuilder`; `env`/`cwd` via `tokio::process::Command` + `ConfigureCommandExt`; `builder().stderr(Stdio::piped()).spawn()` returns `(TokioChildProcess, Option<ChildStderr>)`. **The builder's default is `Stdio::inherit()`** — upstream's `debug ? "inherit" : "pipe"` exactly, correcting a divergence the first pass had worked around |
| streamable HTTP | **yes** | `transport/streamable_http_client.rs`: `StreamableHttpClientTransportConfig { uri, auth_header, custom_headers, retry_config, allow_stateless, max_sse_event_size, reinit_on_expired_session, … }` |
| sampling | **yes** | `ClientHandler::create_message`; `CreateMessageRequestParams`, `ModelPreferences`, `ModelHint`, `SamplingMessage`, `CreateMessageResult`; `examples/clients/src/sampling_stdio.rs` |
| elicitation, both modes | **yes, without the `elicitation` feature** | `ElicitRequestParams::{FormElicitationParams, UrlElicitationParams}`, with an **`ElicitRequestParamsWire::LegacyForm` untagged fallback** so absent or unknown `mode` deserialises as form — upstream's `params.mode === "url" ? url : form`, exactly. `model/elicitation_schema.rs` types the primitives as a **closed enum**, which turns half of upstream's schema-sniffing into a `match` |
| elicitation property order | **yes** | `ElicitationSchema` deserialises through a wire type whose `properties` is an `IndexMap`, and the `From` impl fills `property_order: Option<Vec<String>>`. Iterate that; the `BTreeMap` is the trap |
| roots | **yes (unused)** | `ClientHandler::list_roots`, `Root`, `ListRootsResult`, `Peer::notify_roots_list_changed` |
| resources, templates, subscriptions | **yes (subscriptions unused)** | `Peer::{list_all_resources, list_all_resource_templates, read_resource}`; `subscribe`/`unsubscribe` (legacy, deprecated) and `Peer::listen(SubscriptionFilter)` |
| prompts | **yes** | `Peer::{list_all_prompts, get_prompt}` |
| completions | **yes (unused)** | `Peer::{complete, complete_prompt_argument, complete_resource_argument, complete_prompt_simple, complete_resource_simple}` |
| logging | **yes (unused, deprecated)** | `Peer::set_level` (`#[deprecated]` per SEP-2577), `ClientHandler::on_logging_message` |
| progress and cancellation | **yes** | `handler/client/progress.rs`'s `ProgressDispatcher`/`ProgressSubscriber`; `RequestHandle::cancel(reason)`; `PeerRequestOptions { timeout, meta, reset_timeout_on_progress, max_total_timeout }`; `serve_client_with_ct(CancellationToken)`; `RunningService::{cancel, cancellation_token, waiting}` |
| protocol-revision negotiation | **yes, and it maps 1:1 to the config field** | `ClientLifecycleMode::{Initialize, Discover{preferred_versions}, Auto{preferred_versions, legacy_version}}` + `ClientServiceExt::serve_with_lifecycle`; `ProtocolVersion::V_*`; `select_protocol_version` |
| OAuth 2.1 — PKCE S256, DCR (7591), PRM (9728), AS metadata (8414) | **yes, and more** | `transport/auth.rs`: `OAuthState`, `AuthorizationManager`, `AuthorizationSession`, `AuthClient<C>`, `CredentialStore`/`StateStore`, `WWWAuthenticateParams::parse`, `ScopeUpgradeConfig`, `ClientCredentialsConfig`. Also RFC 8707 resource binding, the RFC 9207 `iss` gate, CIMD (SEP-991), automatic refresh, and 403 `insufficient_scope` scope upgrade (SEP-835). PKCE S256 is always enforced; there is no `plain` fallback |
| 401 / terminated-session classification | **yes** | `ClientInitializeError::auth_challenge()` walks the `source()` chain and returns the raw `WWW-Authenticate` header, replacing `isUnauthorizedHttpError` and adding a 403 arm upstream lacks. `StreamableHttpError::SessionExpired` fires on exactly `NOT_FOUND && session_was_attached` — upstream's `isTerminatedSession` first arm, session gate included |
| **tool output schemas and structured content** | **types yes, validation NO** | `Tool::output_schema`, `CallToolResult::structured_content` exist. **rmcp does no client-side JSON-Schema validation** — there is no validator hook on `Peer<RoleClient>`, unlike the TS SDK's `jsonSchemaValidator` client option that `json-schema-validator.ts` supplies. **Correction: this one is hand-written** |
| legacy HTTP+SSE client transport | **NO — and this is the point** | no `SseClientTransport`, no `transport-sse-client` feature, nothing in `transport.rs`'s exports. Cut 1 is aligned with the SDK |
| raw framed unix socket | **NO** | `UnixSocketHttpClient` exists but is streamable-HTTP-over-UDS, a different shape. Cut 3 |

**Two rmcp deltas that cost real code and must not be glossed.**

1. **`initialize_from_store` restores only `client_id`.** `StoredCredentials`'s five fields —
   `client_id`, `token_response`, `granted_scopes`, `token_received_at`, `issuer` — carry no
   client-secret field, and
   `AuthorizationManager::initialize_from_store` calls `configure_client_id`. Upstream's
   `StoredClientInfo` persists `clientSecret`, `clientIdIssuedAt`, `clientSecretExpiresAt` and
   `redirectUris`. A *confidential* dynamically-registered client loses its secret across restarts.
   Fix, extension-owned and ~20 lines: persist the DCR response as a second keychain record and, after
   `initialize_from_store()`, call `AuthorizationManager::configure_client(OAuthClientConfig::new(…)
   .with_client_secret(…))`. rmcp's public API supports it (`MCP-290`, `MCP-314`).
2. **`CredentialStore` is per-manager and keyless.** `load`/`save`/`clear` take no key, so
   `cyrup-mcp` instantiates one keyring-backed store per server bound to that server's account key.
   That is the natural shape, not a workaround (`MCP-291`).

Three further deltas are named in the section files rather than here: rmcp's dynamic-registration body
is fixed and drops `client_uri` / `logo_uri` / confidential auth methods; its client-authentication
selection follows the TypeScript SDK's rule, not the adapter's, on an empty
`token_endpoint_auth_methods_supported`; and `set_allow_missing_issuer` covers only half of
`skipIssuerMetadataValidation`. All three have ~10–40-line fixes through rmcp's public API.

### The rest of the dependency set

| upstream npm | cyrup Rust | status |
|---|---|---|
| `@modelcontextprotocol/client` + `/core` 2.0.0 | **`rmcp` 3.1.2** | new; feature set above |
| `@napi-rs/keyring` ^1.3.0 | **`keyring` 4.1.6** | **new to the lock file.** `@napi-rs/keyring` is a napi binding of the same Rust library, so the store semantics port near 1:1 — confirmed by two `keyring_core::Error` `Display` strings being byte-identical to the strings upstream's fault-injection store fabricates. What does not come free: Windows blob chunking, and the Linux keyring-revoked recovery. **The Linux backend choice is an open ruling** — see OPEN-1 |
| `ajv` ^8 + `ajv-formats` ^3 | `jsonschema` | **already in tree, bump required.** Must cover draft-07 *and* 2020-12 with `should_validate_formats(true)` on both, matching `json-schema-validator.ts`'s dispatch on `$schema`. Keep `default-features = false` — the workspace comment on remote/file `$ref` resolution applies identically |
| `smol-toml` ^1.6 | `toml` | **already in tree** (`cyrup-resources`); promote to `[workspace.dependencies]`. One caller: the Codex `config.toml` import |
| `strip-json-comments` ^5 | `cyrup_permission_system::jsonc` | **already in tree and already `pub`.** It is the same parser `cyrup_permission_system::manager`'s `read_configured_mcp_server_names` uses on `mcp.json`, so both crates parse that file identically **by construction** |
| `open` ^10 | `opener` | **new to the lock file.** The OAuth authorization URL and `Url` elicitation. Upstream has *both* an exec dispatch (`$BROWSER`, in `utils.ts`) and a direct `open` import; the port has both |
| `recheck` ^4.5 (ReDoS) | `regex` | **none needed** — with a named residual, below. `regex` is already resolved in the lock file as a direct dependency of `cyrup-permission-system`; promote it to `[workspace.dependencies]` |
| `cross-spawn` ^7 | `tokio::process` | none; rmcp's `TokioChildProcess` owns the stdio spawn |
| `zod`, `typebox` | `serde` + `serde_json::Value` | none; tool parameters cross as JSON |
| `@modelcontextprotocol/ext-apps` ^1.2 | — | **CUT 2** |
| (`axum`) | — | **not used.** It appears only in rmcp's dev-dependencies and its OAuth *example*. Cut 2 removed the only reason to want an HTTP server, and the OAuth callback listener is `cyrup-provider`'s |

Reused rather than added: **`cyrup_provider::auth::oauth::callback`** for the OAuth loopback listener
(a real `TcpListener` accept loop with request-read timeout, head cap and settle-once — and a
`CallbackHandler` that returns `CallbackOutcome::Continue` never settles the server's one-shot, which
is precisely what makes it persistent and multi-tenant); **`cyrup_tools::truncate` /
`cyrup_tools::output`** for the output guard's dual byte/line truncation and temp-file spill; and
**`cyrup_ext::caps::proc::npx_resolver`**, already an 892-line port of this package's own
`npx-resolver.ts`.

**The `recheck` residual, stated precisely.** `recheck.checkSync` is called at exactly one place —
`executeSearch`, guarding a caller-supplied pattern from `mcp({ search, regex: true })` — and anything
but `safe` is rejected as `unsafe_pattern`. Rust's `regex` crate compiles to a finite automaton with a
**linear-time matching guarantee**, so the attack the check exists to stop **cannot occur**. What is
*not* reproduced, exactly:

1. **Compile-time and memory blowup remain possible.** Set `RegexBuilder::size_limit` and
   `dfa_size_limit` explicitly rather than relying on defaults, and surface the compile error as
   upstream's `invalid_pattern`.
2. **The `unsafe_pattern` *diagnostic* disappears.** Upstream tells the model "Regex query rejected as
   unsafe". The port has no such rejection — a nested-quantifier pattern simply runs, in linear time.
   A behaviour delta, recorded as one; the code enum keeps `unsafe_pattern` as a documented
   no-producer variant so 31 of 32 codes stay reachable.
3. `MAX_REGEX_SEARCH_QUERY_LENGTH` and the `"i"` flag are not `recheck` and port directly.
4. **Dialect differences.** JS `RegExp` accepts backreferences and lookaround that `regex` rejects;
   those become `invalid_pattern` where upstream compiled them. Name it in the `/mcp` help text rather
   than pretending the dialects match.
---

## The seam map

The core reference: every in-scope adapter capability, the cyrup mechanism that serves it, and a
verdict. Verdicts are **`rmcp`** (the SDK does it) · **`host-verb`** (a named existing cyrup API) ·
**`extension-owned`** (the native crate does it with its own dependencies; no host involvement, no
core change) · **`hand-written`** (new code in `cyrup-mcp`) · **`host-addition`** (needs a new host
surface) · **`cut`**.

### Transports

| # | capability | upstream | cyrup | verdict |
|---|---|---|---|---|
| T-1 | stdio child process with `env`, `cwd`, stderr tail | `server-manager.ts` `createConnection`, `StdioClientTransport` + `stderrTail` | `TokioChildProcess` / `TokioChildProcessBuilder::{stderr, spawn}`, `ConfigureCommandExt` | **`rmcp`** |
| T-2 | `npx`/`npm` → real binary pre-resolution | `npx-resolver.ts` `resolveNpxBinary`, before the transport is built | `cyrup_ext::caps::proc::npx_resolver::resolve_npx_binary` — **already a full port**, currently `pub(super)` | **`extension-owned` (reuse)** |
| T-3 | streamable HTTP | `StreamableHTTPClientTransport` in `connectHttpClient` | `StreamableHttpClientTransport` + its config | **`rmcp`** |
| T-4 | legacy HTTP+SSE + `shouldFallbackToSse` | `server-manager.ts` | — | **`cut` (1)** |
| T-5 | raw unix socket | `unix-socket-transport.ts`, `ServerEntry.socket` | — | **`cut` (3)** |
| T-6 | transport selection + mutual exclusion | `createConnection` | reduced to command/url; `socket` and `httpTransport:"sse"` become named load-time diagnostics | **`hand-written`** |
| T-7 | protocol-era negotiation | `resolveVersionNegotiation` | `ClientLifecycleMode` + `ClientServiceExt::serve_with_lifecycle`; `ProtocolVersion::V_*` | **`rmcp`** |
| T-8 | per-request timeout, per-server override | `buildRequestOptions` / `getResolvedRequestTimeoutMs` | `PeerRequestOptions { timeout, reset_timeout_on_progress, max_total_timeout }` | **`rmcp`** |
| T-9 | custom headers, bearer token, `bearerTokenEnv` | `connectHttpClient`, `resolveBearerToken` | `StreamableHttpClientTransportConfig::{auth_header, custom_headers}` | **`rmcp`** + **`hand-written`** (env/secret-command resolution) |
| T-10 | metadata-only JSONL protocol trace | `mcp-trace.ts` `wrapTransportWithMcpTrace`, `McpTraceWriter`, `redactTraceText` | a `TracingTransport<T>` newtype over `rmcp::transport::Transport<RoleClient>` | **`hand-written`** |

### Server lifecycle

| # | capability | upstream | cyrup | verdict |
|---|---|---|---|---|
| L-1 | connection map, connect/reconnect dedup, close, `closeAll` | `McpServerManager` (five race-guard maps) | — | **`hand-written`** |
| L-2 | `eager`/`lazy`/`keep-alive`/`lazy-keep-alive`, idle sweep, `touch`/`incrementInFlight`/`isIdle` | `lifecycle.ts`, `init.ts` | `tokio` timers on an extension-owned task | **`hand-written`** |
| L-3 | runtime owner, abort ownership, generation fencing | `runtime-owner.ts`, `abort.ts`, `index.ts` | a `tokio_util::sync::CancellationToken` tree (which is what `cyrup_core::CancelToken` *is*, and exactly the type `serve_client_with_ct` takes) | **`hand-written`** (thin) + **`host-verb` `is_run_cancelled`** |
| L-4 | 60 s failure backoff and messages | `init.ts` | — | **`hand-written`** |
| L-5 | terminated-session detection and retry | `session-recovery.ts` | `StreamableHttpError::SessionExpired` covers the 404 arm; the 400/`-32000` and ProtocolError arms and the retry-with-reauth wrapper are adapter policy | **`rmcp`** + **`hand-written`** |
| L-6 | tools/prompts/resources list-changed refresh | the TS SDK's `listChanged` client option | `ClientHandler::on_*_list_changed` (bare; `Peer` self-invalidates) then re-`list_all_*` | **`rmcp`** + glue |
| L-7 | server `instructions` | `client.getInstructions?.()` | `RunningService::peer_info()` → `InitializeResult.instructions` | **`rmcp`** |
| L-8 | init orchestration, staged connect, startup notifications | `init.ts` `initializeMcp` | — | **`hand-written`** |
| L-9 | eager/keep-alive pre-connect before `session_start` | `index.ts` `startLoadTimeInitialization` | a task spawned from `NativeExtension::init` using the `Arc<dyn HostServices>` stashed by `set_host_services` | **`extension-owned`** |
| L-10 | session restart / shutdown teardown | `pi.on("session_start"/"session_shutdown")` | `EventKind::SessionStart` / `EventKind::SessionShutdown` | **`host-verb` `InitApi::subscribe`** |

### Tools

| # | capability | upstream | cyrup | verdict |
|---|---|---|---|---|
| TL-1 | `tools/list` with pagination | `fetchAllTools` | `Peer::list_all_tools` | **`rmcp`** |
| TL-2 | naming: four prefix modes, `formatToolName`, `getToolNameCandidates`, `resolveServerFromToolName`, include/exclude globs, `searchKeywords` | `types.ts` | — (pure functions) | **`hand-written`** |
| TL-3 | direct-tool registration at init from `mcp-cache.json` | `index.ts` `syncDirectTools` | `InitApi::register_tool` | **`host-verb`** |
| TL-4 | direct-tool registration **after** init | `onToolMetadataUpdated` → `syncToolSurface` | `ExtensionHost::register_late_tool` exists; **a native has no handle, and `refresh_tools` drops the native tier's signal in the default build** (Finding 1) | **`host-addition` HA-1** (two units) |
| TL-5 | tool removal on disable / disappearance | `deactivateTools`: optional `pi.unregisterTool`, else `setActiveTools` + `fallbackDeactivatedTools` | `HostServices::{active_tools, set_active_tools}`. `ExtensionRegistry` has no `unregister_tool`, so cyrup lands on **upstream's own documented fallback branch** | **`host-verb`** (accepted delta: the name stays registered for the session) |
| TL-6 | direct-tool fingerprinting | `directToolFingerprint` | — | **`hand-written`** |
| TL-7 | `freezeDirectTools` | `index.ts` | — | **`hand-written`** |
| TL-8 | `mcp` proxy tool: registration, description, `disableProxyTool`, re-register on change | `registerProxyTool`/`syncProxyTool`, `buildProxyDescription` | `InitApi::register_tool` + HA-1 for the refresh | **`host-verb`** + **`host-addition`** |
| TL-9 | nine proxy modes | `proxy-modes.ts` | — | **`hand-written`** |
| TL-10 | proxy mode `ui-messages` | `executeUiMessages` | — | **`cut` (2)** |
| TL-11 | `mcpScript` tool | `mcp-code.ts`, `mcp-script-worker.mjs` | — | **`cut` (4)** |
| TL-12 | search ranking, pagination, suggestions | `search-ranking.ts` | — | **`hand-written`** (pure) |
| TL-13 | regex search ReDoS guard | `recheck.checkSync` | `regex`'s linear-time guarantee + explicit size limits | **`hand-written`** — residual named above |
| TL-14 | `mcp({tool})` disambiguation, prefix-driven lazy connect, auto-auth retry | `executeCall` | — | **`hand-written`** |
| TL-15 | direct-tool executor | `createDirectToolExecutor` | — | **`hand-written`** (UI interleave removed) |
| TL-16 | app-only tool hiding | `extractUiToolVisibility` + `isUiToolVisibleToModel` | — | **`hand-written`** (kept) |
| TL-17 | app-callable check | `isUiToolCallableByApp` | — | **`cut` (2)** |
| TL-18 | resources exposed as tools | `exposeResources`, `resource-tools.ts` | — | **`hand-written`** |
| TL-19 | tool-argument JSON-Schema validation | the TS SDK's `jsonSchemaValidator` option | **rmcp does not validate client-side** | **`hand-written`** with `jsonschema` |
| TL-20 | `renderShell: "self" \| "default"` | `toolRenderShell` | `cyrup_core::ToolRenderKind::{Default, SelfRendered}` | **`host-verb`** |

### Result rendering

| # | capability | upstream | cyrup | verdict |
|---|---|---|---|---|
| R-1 | content transform: text / image / audio / resource / resource_link / structured fallback | `transformMcpContent`, `resolveMcpResultContent` | over `rmcp::model::ContentBlock` and `ResourceContents` | **`hand-written`** |
| R-2 | binary-resource materialisation (10 MiB/resource, session byte+file caps, `0o600`, `wx`) and sweep | `materializeBinaryResource`, `cleanupMaterializedBinaryResources` | — | **`hand-written`** |
| R-3 | `ui://` resource rendering | `ui-resource-handler.ts` | — | **`cut` (2)** |
| R-4 | output guard: byte/line caps, spill, truncation notice, `details.mcpResult` cap, image pass-through | `mcp-output-guard.ts` | reuse `cyrup_tools::truncate::{truncate_head, TruncOpts, Truncated}` and `cyrup_tools::output::OutputAccumulator`'s spill pattern | **`hand-written`** (on cyrup primitives) |
| R-5 | proxy/direct call renderers, `compact`/`boxed`, `collapsedResultLines` | `tool-result-renderer.ts` | `InitApi::register_tool_renderer` + `NativeExtension::{render_call, render_result}` | **`host-verb`** + **`hand-written`** |
| R-6 | re-flag returned MCP failures as errors | `error-signal.ts` on `tool_result` | `EventKind::ToolResult` + the `HookOutcome` patch path | **`host-verb`** |

### Approval

| # | capability | upstream | cyrup | verdict |
|---|---|---|---|---|
| A-1 | `approveTools` matching (global + per-server globs, legacy-name compat) | `isToolCallApprovalRequired` | — | **`hand-written`** (pure) |
| A-2 | approval dialog + session cache | `ensureToolCallApproved` → three-way select | `HostServices::select` under `HostServices::human_interaction_lock` | **`host-verb`** |
| A-3 | headless refusal | `approval_required_headless` when `!state.ui` | `select` returns `None` with no UI sink attached | **`host-verb`** |
| A-4 | cross-extension approval broker | `MCP_TOOL_APPROVAL_REQUEST_EVENT` carrying a `claim(handler)` callback | subsumed by `ExtHooks::before_tool_call` + `cyrup-permission-system`; cyrup's `SharedBus` is JSON-only and deferred and **cannot carry a closure by construction** | **`host-verb`** (already wired) — the bus event does not port |
| A-5 | `ConsentManager` | `consent-manager.ts` | — | **`cut` (2)** — corrects the seam map's own earlier reading |

### Metadata cache

| # | capability | upstream | cyrup | verdict |
|---|---|---|---|---|
| M-1 | `<agent_dir>/mcp-cache.json`, `CACHE_VERSION = 1`, atomic rename | `metadata-cache.ts` | `cyrup-mcp` is the **writer**; `cyrup_ext_subagents::exec::mcp_direct_tools` is an existing **reader** | **`hand-written`** — schema is a fixed contract |
| M-2 | `computeServerHash` config identity | `metadata-cache.ts` | already ported as `compute_mcp_server_hash` in `cyrup-ext-subagents` — **must produce identical digests** | **`hand-written`** (parity-constrained) |
| M-3 | 7-day staleness, `isServerCacheValid` | `metadata-cache.ts` | `CACHE_MAX_AGE_MS` already mirrored in the reader | **`hand-written`** |
| M-4 | reconstructors and serialisers | `metadata-cache.ts` | — | **`hand-written`** |
| M-5 | `parseDirectToolSelectors`, missing-server gate | `metadata-cache.ts` | selector semantics already mirrored in the reader | **`hand-written`** |

### Config

| # | capability | upstream | cyrup | verdict |
|---|---|---|---|---|
| C-1 | six-source discovery and merge | `config.ts` | — | **`hand-written`** |
| C-2 | JSONC parsing | `strip-json-comments` | `cyrup_permission_system::jsonc` — the same parser that crate uses on `mcp.json` | **`extension-owned` (reuse)** |
| C-3 | `.toml` config import (Codex) | `smol-toml` | `toml` | **`extension-owned` (reuse)** |
| C-4 | `--mcp-config` flag | `registerFlag` for `--help`; the **value** read from `process.argv` by `getConfigPathFromArgv` | `InitApi::register_flag` + `std::env::args()` — **the literal upstream mechanism.** No flag-read-back gap exists | **`host-verb`** + **`extension-owned`** |
| C-5 | six config writers + their `preview*` twins | `config.ts` | direct filesystem writes | **`extension-owned`** |
| C-6 | env interpolation (`${X}`, `$env:X`, `{env:X}`), `literalEnv` | `interpolateEnvVars`, `resolveEnv` | — | **`hand-written`** |
| C-7 | secret-resolution commands for HTTP headers | `connectHttpClient` | `tokio::process` directly | **`extension-owned`** |
| C-8 | Agent Plugin config loading | `agent-plugin-loader.ts` | — | **`hand-written`** |
| C-9 | `KNOWN_SERVER_PRESETS`, provenance, conflicts | `config.ts` | — | **`hand-written`** |
| C-10 | onboarding state file | `onboarding-state.ts` | — | **`hand-written`** |
| C-11 | agent-dir resolution | `agent-dir.ts` | `cyrup-config`'s `ConfigDirs::agent_dir` **field** (`ConfigDirs::resolve` populates it) — the same `<agent_dir>` `cyrup-permission-system` resolves | **`extension-owned` (reuse)** |

### OAuth

| # | capability | upstream | cyrup | verdict |
|---|---|---|---|---|
| OA-1 | RFC 9728 + RFC 8414/OIDC discovery, `WWW-Authenticate` seeding | `mcp-oauth-provider.ts`, `mcp-auth-flow.ts` | `AuthorizationManager::{resolve_metadata, resolve_metadata_from_challenge}`, `WWWAuthenticateParams::parse`; `ClientInitializeError::auth_challenge()` is the reactive trigger | **`rmcp`** |
| OA-2 | RFC 7591 dynamic client registration | `McpOAuthProvider` | `AuthorizationManager::register_client`; CIMD via `AuthorizationRequest::with_client_metadata_url` | **`rmcp`** |
| OA-3 | PKCE S256, authorize URL, RFC 8707 `resource`, `authorizationParams` | `startAuth` | `OAuthState::start_authorization` + `get_authorization_url` | **`rmcp`** |
| OA-4 | code exchange, refresh, 403 scope upgrade | `completeAuth`, `getValidToken` | `OAuthState::handle_callback_with_issuer`, `AuthorizationManager::{exchange_code_for_token_with_issuer, refresh_token, request_scope_upgrade}` | **`rmcp`** |
| OA-5 | `client_credentials` grant | `OAuthConfig.grantType` | `ClientCredentialsConfig` | **`rmcp`** |
| OA-6 | loopback listener: fixed port + path, **multi-tenant** keyed by `state`, ref-counted, success/error HTML | `mcp-callback-server.ts` | build on `cyrup_provider::auth::oauth::callback::{CallbackServer, CallbackServerConfig::fixed, CallbackHandler, CallbackOutcome}`. **The multiplexer is adapter code**: one server whose handler always returns `Continue` and routes by `state` into a per-flow oneshot map, kept alive by a refcount | **`hand-written`** on a **reused** primitive |
| OA-7 | browser open | npm `open` | `opener` | **`extension-owned`** |
| OA-8 | manual paste fallback | `parseAuthorizationRedirectInput`, `mcp({action:"auth-complete"})` | `HostServices::input` / `oauth_prompt`; the parser is pure | **`host-verb`** + **`hand-written`** |
| OA-9 | server picker for `/mcp-auth` | `openMcpAuthPanel` | `HostServices::oauth_select`, or the panel overlay | **`host-verb`** |
| OA-10 | auto-auth on a `needs-auth` connection mid-call | `attemptAutoAuth` / `attemptDirectAutoAuth` | — | **`hand-written`** |
| OA-11 | `authRequiredMessage` templating | `getAuthRequiredMessage` | — | **`hand-written`** |
| OA-12 | confidential-client secret across restarts | `StoredClientInfo.clientSecret` | re-apply with `configure_client(…with_client_secret)` after `initialize_from_store()` | **`hand-written`** (~20 lines) |
| OA-13 | public OAuth API | `oauth.ts` | a `pub` module on `cyrup-mcp` | **`extension-owned`** |

### Keychain

| # | capability | upstream | cyrup | verdict |
|---|---|---|---|---|
| K-1 | OS credential store read/write/delete | `keyringAuthSecretStore` over `@napi-rs/keyring` | `keyring` 4.1.6 `Entry` — the same underlying library. Note 4.x is a registry over `keyring-core`: `Entry::new` returns `Result` and absence is `Err(NoEntry)`, not a null | **`extension-owned`** |
| K-2 | expose it to rmcp | — (upstream's SDK provider owns storage) | implement `rmcp::transport::auth::CredentialStore` per server + `StateStore` for PKCE/CSRF | **`hand-written`** (thin) |
| K-3 | chunking past the value limit with a manifest | `mcp-auth.ts` | — | **`hand-written`** |
| K-4 | Linux keyring-revoked recovery | `spawnSync("keyctl", ["session","-","node","mcp-keyring-helper.cjs"])`, JSON over stdio, 10 s timeout | same mechanism: `keyctl session - <current_exe()> __mcp-keyring-helper`, same protocol, same timeout, same trigger regex. **No `node`** | **`hand-written`** |
| K-5 | legacy plaintext `tokens.json` one-time import then delete | `mcp-auth.ts` | — | **`hand-written`** |
| K-6 | in-process auth-entry cache + kill switch | `authEntryCache` | — | **`hand-written`** |

### Sampling

| # | capability | upstream | cyrup | verdict |
|---|---|---|---|---|
| S-1 | receive `sampling/createMessage` | `registerSamplingHandler` | `ClientHandler::create_message` | **`rmcp`** |
| S-2 | advertise the capability | `buildClientCapabilities` | `ClientCapabilities.sampling` | **`rmcp`** |
| S-3 | resolve the model from `modelPreferences` | `resolveSamplingModel` over `ModelRegistry` | `cyrup_provider::catalog` directly + `HostServices::{models, scoped_models, current_model}` | **`extension-owned`** + **`host-verb`** |
| S-4 | run the completion | `complete()` from `pi-ai/compat`, **bypassing pi's host API** | `cyrup-provider`'s completion path directly — **no host verb exists and none is wanted** | **`extension-owned`** |
| S-5 | two approval dialogs + `samplingAutoApprove` | `confirmSampling` | `HostServices::confirm` under `human_interaction_lock` | **`host-verb`** |
| S-6 | message/content conversion both directions | `convertSamplingMessage`, `convertAssistantResult` | over `SamplingMessage`, `CreateMessageResult` | **`hand-written`** |
| S-7 | the hard rejections (tasks, includeContext, tools, toolChoice, stopSequences) | `handleSamplingRequest` | `task` becomes **structural** — `CreateMessageRequestParams` has no such field, and task augmentation is an extension `cyrup-mcp` never declares | **`hand-written`** |
| S-8 | abort checkpoints | `throwIfAborted(signal)` ×3 | `CancelToken` + `HostServices::is_run_cancelled` | **`host-verb`** |

### Elicitation

| # | capability | upstream | cyrup | verdict |
|---|---|---|---|---|
| E-1 | receive `elicitation/create`; advertise `{form, url?}` | `registerElicitationHandler` | `ClientHandler::create_elicitation`; `ElicitationCapability::{with_form, with_url}` | **`rmcp`** (no `elicitation` feature) |
| E-2 | Form: one widget per primitive | `handleFormElicitation` | `HostServices::{select, input, confirm}` over the typed `PrimitiveSchemaDefinition` family | **`host-verb`** |
| E-3 | coercion + validation + re-prompt | `coerceAndValidateFormValues` | — (rmcp validates nothing client-side) | **`hand-written`** — where the real work is |
| E-4 | Url: consent → open → accept | `handleUrlElicitation` | `HostServices::{confirm, oauth_prompt}` + `opener` | **`host-verb`** + **`extension-owned`** |
| E-5 | `notifications/elicitation/complete` + dedupe set | `setNotificationHandler` | `ClientHandler::on_custom_notification` (rmcp has no first-class variant) + `HostServices::notify` | **`rmcp`** + **`hand-written`** |
| E-6 | batch `UrlElicitationRequiredError` handling | `handleUrlElicitationRequired` | — | **`hand-written`** |
| E-7 | decline / cancel | `{action:"decline"\|"cancel"}` | `ElicitationAction::{Decline, Cancel}`; rmcp's **default handler declines**, so an unimplemented handler is fail-safe | **`rmcp`** |

### Prompts, resources, roots, logging, progress

| # | capability | upstream | cyrup | verdict |
|---|---|---|---|---|
| P-1 | `prompts/list` (paginated), `prompts/get` | `fetchAllPrompts`, `getPrompt` | `Peer::{list_all_prompts, get_prompt}` | **`rmcp`** |
| P-2 | prompt → slash-command name | `sanitizePromptName`, `formatPromptCommandName` | — | **`hand-written`** |
| P-3 | register prompt commands at init from the cache | `registerPromptCommands` | `InitApi::register_command` | **`host-verb`** |
| P-4 | register prompt commands **after** init | `syncPromptCommands` | no `register_late_command` on `ExtensionHost`, and the `/` registry is installed at exactly three points, none extension-driven | **`host-addition`** — folded into HA-1 (`MCP-395`) |
| P-5 | argument parsing (positional + `key=value`), result formatting | `prompts.ts` | — | **`hand-written`** |
| P-6 | prompt-argument completion | **not implemented upstream** | `Peer::complete_prompt_argument` exists unused | **`rmcp`** — nothing to port |
| RS-1 | `resources/list`, `templates/list`, `read` | `fetchAllResources`, `readResource` | `Peer::{list_all_resources, list_all_resource_templates, read_resource}` | **`rmcp`** |
| RS-2 | resource content → content blocks | `transformMcpResourceContents` | over `ResourceContents` | **`hand-written`** |
| RS-3 | resources as tools | `resource-tools.ts` | — | **`hand-written`** |
| RS-4 | resource subscriptions | **not implemented upstream** | `Peer::{subscribe, unsubscribe}` / `Peer::listen` exist unused | **`rmcp`** — nothing to port |
| RS-5 | `resources/list_changed` | `handleResourcesListChanged` | `ClientHandler::on_resource_list_changed` | **`rmcp`** + glue |
| RT-1 | roots | **not implemented upstream — zero occurrences** | `list_roots` defaults to empty | **`rmcp`** — nothing to port |
| LG-1 | MCP `logging/setLevel` + `notifications/message` | **not implemented upstream** | `Peer::set_level`, `on_logging_message` exist unused | **`rmcp`** — nothing to port |
| LG-2 | adapter-internal logging | `logger.ts` | `tracing` | **`extension-owned`** |
| LG-3 | protocol progress | `_meta.progressToken` through the SDK | `ClientHandler::on_progress` + `ProgressDispatcher` | **`rmcp`** |
| LG-4 | request cancellation | `abortable(client.callTool(...), signal)` | `RequestHandle::cancel(reason)`, `PeerRequestOptions`, `serve_client_with_ct` | **`rmcp`** |

### TUI, commands, status

| # | capability | upstream | cyrup | verdict |
|---|---|---|---|---|
| U-1 | `/mcp` status panel (server list, tool tree, fuzzy filter, token estimates, direct-tool toggles, save) | `mcp-panel.ts` + `openMcpPanel` | implement `cyrup_ext::InteractiveOverlay`, open with `HostServices::open_overlay`; precedents `cyrup_ext_subagents::tui::fleet_overlay::FleetOverlay` and `cyrup_permission_system::config_modal::PermissionSystemSettingsOverlay` | **`host-verb`** |
| U-2 | `/mcp setup` onboarding panel | `mcp-setup-panel.ts` | same | **`host-verb`** |
| U-3 | `/mcp-auth` picker panel | `openMcpAuthPanel` | same, or `HostServices::oauth_select` | **`host-verb`** |
| U-4 | panel geometry (fixed 82/92 columns, centre) | `overlayOptions` | `ExtensionOverlay::box_rect` hardcodes one geometry; the host draws **no border**, so content self-centres — only the `Clear` width differs | **`host-addition` HA-3** (cosmetic; owned by `MCP-368`) |
| U-5 | panel keybindings from user config | `createPanelKeys(keybindings)` | a native crate links `cyrup-config` and reads `<agent_dir>/keybindings.json` itself — the only way to see a user's `mcp.panel.save`; `OverlayKey` arrives pre-mapped for the three canonical ids | **`extension-owned`** (`MCP-363`) |
| U-6 | non-TUI text status/tools/prompts listings | `showStatus`/`showTools`/`showPrompts` | command return string + `HostServices::notify` | **`host-verb`** |
| X-1 | `/mcp` and `/mcp-auth` registration and dispatch | `pi.registerCommand` | `InitApi::register_command` + `NativeExtension::execute_command` at **command tier** (session mutation allowed) | **`host-verb`** |
| X-2 | `/mcp` dynamic argument completions | `getArgumentCompletions` | declaration half exists; native dispatch arm and TUI consumer both missing | **`host-addition` HA-2** |
| X-3 | `/reload` after a config change | `ctx.reload()` | `HostServices::control(ControlOp::Reload)` — legal from command tier | **`host-verb`** |
| X-4 | footer status segment, three verbosities | `ui.setStatus("mcp", …)` | `HostServices::set_status(key, Option<&str>)` — keyed, clearable with `None`. Colour is dropped: `LiveHostServices` does not override `theme`, so the branch collapses to upstream's own no-theme arm | **`host-verb`** |
| X-5 | notifications | `ctx.ui.notify(msg, kind)` | `HostServices::notify` + `NotifyKind::{Info, Warning, Error}` | **`host-verb`** |
| X-6 | status snapshot published on the event bus | `publishMcpStatusSnapshot` on `pi.events` | `SharedBus::emit` exists but a native has no route to it **and cyrup has no consumer** — keep the snapshot as an in-crate `tokio::sync::watch`. Building the route would be a dead primitive | **`extension-owned`** |
| X-7 | tracing | `logger.ts` | `tracing` | **`extension-owned`** |
| X-8 | error taxonomy | `errors.ts` | `thiserror` | **`hand-written`** |
| X-9 | endpoint probe for setup | `probeMcpEndpoint` | `reqwest` | **`extension-owned`** |
| X-10 | TypeScript-shape rendering of tool schemas | `ts-shape.ts` | — | **`hand-written`** (pure) |
| X-11 | terminal-text sanitising | `sanitizeTerminalText` | — | **`hand-written`** |
| X-12 | `--no-extensions` participation | installed-package tier upstream | `NativeExtension::is_ambient` ⇒ **`true`** (it is the sole `--no-extensions` gate) | **`host-verb`** |
| X-13 | pre-trust bootstrap participation | n/a | `NativeExtension::decides_project_trust` ⇒ **`false`** (the default; opting in runs `init` **twice on the same object**, and `init` is not idempotent) | **`host-verb`** |
---

## Architecture

### `crates/cyrup-mcp` — the module tree

Sizes are order-of-magnitude estimates from the upstream line counts, not measurements.

| module | owns | ~lines |
|---|---:|---:|
| `lib.rs` | crate root; `mcp_extension_for_env(...) -> Option<Arc<dyn NativeExtension>>`, mirroring `cyrup_ext_subagents::extension::subagent_extension_for_env` | 150 |
| `extension.rs` | the `NativeExtension` impl: `id`, `init`, `set_host_services`, `on_event`, `on_bus_event`, `execute_command`, `execute_shortcut`, `render_call`, `render_result`, `is_ambient` ⇒ true, `decides_project_trust` ⇒ false | 900 |
| `state.rs` | `McpExtensionState` (**20 fields after the cuts**, from 25), the `AtomicU64` lifecycle generation | 200 |
| `runtime.rs` | `McpRuntimeOwner`, the cancellation-token tree, the abort helpers, LIFO cleanup with an aggregate error | 350 |
| `init.rs` | `initializeMcp`: staged connect, per-server lifecycle registration, the two-pass metadata build, failure backoff, status bar, cache flush | 700 |
| `config/` | `mod.rs` (six-source ladder, merge, discovery summary), `imports.rs` (seven host-config families incl. opencode's git-root walk and codex's snake_case remaps), `entry.rs` (`ServerEntry`, the 14-key hash pre-image), `settings.rs` (23 keys, env overrides, the `__none__` sentinel), `write.rs` (six writers + preview twins + LCS diff), `plugin.rs` (`agent-plugin-loader`) | 2,200 |
| `manager/` | `mod.rs` (`McpServerManager`: connections, five race guards, generations), `connect.rs` (transport construction, the attempt loop, header/bearer/secret resolution), `recovery.rs` (`withSessionRecovery`), `lifecycle.rs` (four modes, idle sweep, `gracefulShutdown`), `probe.rs`, `trace.rs` (`TracingTransport<T>` + `McpTraceWriter`) | 2,400 |
| `handler.rs` | the `ClientHandler` impl — one type per connection, state shared through `Arc` | 300 |
| `cache.rs` | the `mcp-cache.json` **writer**: schema, `computeServerHash`, `stableStringify`, serialisers, reconstructors | 550 |
| `tools/` | `mod.rs` (registration, fingerprints, freeze, activation), `metadata.rs` (`buildToolMetadata`, `formatSchema`), `direct.rs` (the executor state machine), `proxy.rs` (the `mcp` tool + `buildProxyDescription`), `modes.rs` (nine modes), `ranking.rs`, `resources.rs`, `approval.rs`, `guard.rs`, `render.rs`, `validate.rs` | 3,400 |
| `sampling.rs` | `handleSamplingRequest`, model resolution, both approval gates, conversion, `mapStopReason` | 450 |
| `elicitation.rs` | form gate / review loop / edit picker, `coerceAndValidateFormValues`, the URL leg, the completion notice | 600 |
| `oauth/` | `mod.rs` (flow ownership, generation counter, four maps, `startAuth`/`completeAuth`/`authenticate`), `callback.rs` (the multi-tenant router over `cyrup-provider`'s listener), `config.rs` (`extractOAuthConfig`), `pages.rs` (the three HTML pages) | 1,400 |
| `auth/` | `store.rs` (`AuthEntry`, chunking manifest, the process-lifetime cache), `rmcp_store.rs` (`CredentialStore` + `StateStore`), `legacy.rs` (the plaintext import), `keyctl.rs` (the re-exec trigger and protocol) | 1,100 |
| `ui/` | `panel.rs` (`/mcp` status overlay), `setup.rs` (`/mcp setup` overlay), `keys.rs` | 1,500 |
| `commands.rs` | `/mcp`'s eight-way switch, `/mcp-auth`, the non-TUI text listings, argument completions | 700 |
| `prompts.rs` | prompt commands: naming, argument parsing, result formatting | 350 |
| `status.rs` | the status snapshot on a `tokio::sync::watch`, in-crate | 120 |
| `errors.rs`, `util.rs` | the taxonomy; env interpolation (three forms), `sanitizeTerminalText`, `parallelLimit`, argv scan | 500 |
| | **total** | **≈17,500** |

Outside the crate, three additive files in the binary crate, each precedented twice over by
`crates/cyrup/src/intercom_broker_cmd.rs` and `crates/cyrup/src/subagent_runner_cmd.rs` — both of
which expose `SUBCOMMAND` / `is_selected(argv)` / `dispatch` and are pre-dispatched from
`crates/cyrup/src/main.rs` *before* any clap parsing:

- `mcp_keyring_helper_cmd.rs` — the hidden `__mcp-keyring-helper` mode re-exec'd under `keyctl session -`.
- `mcp_conformance_cmd.rs` — the hidden conformance-driver client the MCP conformance harness points at.
- a **visible** `cyrup mcp init` verb in the existing `SUBCOMMANDS` table — `cli.js`'s replacement.
  Visible, not hidden: the two existing precedents are internal re-execs, and this one is a user-facing
  scaffolding command.

### Is a second crate justified? Yes — exactly one, and it is small

**`cyrup-mcp-naming`**, ~600 lines: `ToolPrefix` and its four modes, `sanitizeServerPrefix`,
`formatToolName`, `resolveToolPrefix`, `resolveServerFromToolName` with its ambiguity fail-safe,
`getToolNameCandidates`, `resourceNameToToolName`, the glob matcher and
`isToolIncluded`/`isToolExcluded`/`isToolAllowed`, and `sanitizePromptName`/`formatPromptCommandName`.
Dependencies: `regex`, `serde`. Nothing else.

The reason is not tidiness, it is that **this code has two owners and its divergence fails silently.**
`cyrup-mcp` writes MCP tool names; `cyrup_ext_subagents::exec::mcp_direct_tools` resolves `mcp:`
selectors against them and today carries a faithful port of *pi-subagents'* own drifted copy — which
differs in at least six ways (`-`→`_` instead of sanitize-preserving; no `mcp` prefix mode; no dot
replacement; `get_` instead of `read_` for resource tools; exact-match-only exclusion with no globs
and no `includeTools`; plus `BTreeMap` ordering against config insertion order, and no disabled check).
When the two grammars disagree, a subagent's tool allowlist resolves to **nothing**, with no error
anywhere. A shared crate makes that divergence a compile-time impossibility rather than a review
discipline.

The two rejected alternatives, so nobody re-derives them: making `cyrup-ext-subagents` depend on
`cyrup-mcp` is acyclic but drags `rmcp`, `keyring` and `oauth2` into a crate that sits on the
subagent-child hot path and needs 600 lines of string functions; and putting the module in
`cyrup-core` puts MCP-specific *policy* in the type substrate every crate compiles against, so a
naming change would rebuild the world. This is `MCP-205`'s recommendation, made concrete.

### How it attaches

```
crates/cyrup/src/main.rs
  └─ (three session-build arms, beside subagent_extension_for_env
      and permission_extension_for_env)
       └─ cyrup_mcp::mcp_extension_for_env(...)  -> Arc<dyn NativeExtension>
            └─ SessionFactory::with_native_extension (crates/cyrup-session-svc/src/factory.rs)
                 └─ ExtensionHost::load_native_with_services
                      ├─ NativeExtension::set_host_services(Arc<dyn HostServices>)   [BEFORE init]
                      └─ NativeExtension::init(&mut InitApi)
```

`set_host_services` runs **before** `init`, which is what makes an `init`-spawned background task
legitimate: it already holds the backend and observes the later manager / UI / inject attachments
through the `Arc`'s interior mutability. That is what the method exists for, and
`cyrup-ext-subagents` spawns detached OS processes from `init` and supervises them for the process
lifetime — an MCP stdio child living across turns is strictly less than that.

### What it does NOT touch

No change to `cyrup-agent`, `cyrup-session`, `cyrup-tools`, `cyrup-core`, `cyrup-modes`,
`cyrup-intercom`, `cyrup-resources` or `cyrup-sdk`. Inside `cyrup-ext` and `cyrup-tui`, only the three
host additions above — one of which, HA-1, is two edits rather than one because of Finding 1 — plus
one `pub` visibility promotion. Inside `cyrup-ext-subagents`, five
coordinated edits to one module (below) — a change to an extension crate, not to cyrup's core. Three
one-line Cargo chores: promote `regex` and `toml` to `[workspace.dependencies]` (both currently reach
the tree as single-crate direct dependencies) and bump `jsonschema`.

---

## Lifecycle and data flow

Eight sequences. These are what an implementer builds against.

### 1 · Cold start, warm `mcp-cache.json`

1. `load_native_with_services` → `set_host_services(Arc)` → `init(&mut InitApi)`.
2. `init` **must never return `Err`** — a native `init` returning `Err` is a fatal startup diagnostic
   that exits 1, so a malformed `mcp.json` would crash cyrup on a normal path. Every failure inside
   `init` degrades to a registered-but-empty surface plus a deferred notification (`MCP-003`).
3. Read `--mcp-config` from `std::env::args()`; load the six-source config ladder through
   `cyrup_permission_system::jsonc`; read `<agent_dir>/mcp-cache.json`.
4. `syncDirectTools(earlyConfig, earlyCache)` → one `InitApi::register_tool` per cached direct tool,
   filtered by `isUiToolVisibleToModel`, include/exclude globs, and the **builtin-collision drop**.
5. `registerProxyTool` → `register_tool("mcp")` with `buildProxyDescription` over the cached counts;
   `registerPromptCommands(resolveCachedPrompts)` → `InitApi::register_command` per prompt.
6. `register_tool_renderer`; `register_command("mcp")`, `register_command("mcp-auth")`;
   `register_flag("mcp-config")`; `add_autocomplete("mcp")`;
   `subscribe(SessionStart | SessionShutdown | ToolResult)`.
7. Spawn the load-time pre-warm task for `eager` / `keep-alive` servers off the stashed `Arc`.
8. `EventKind::SessionStart` → bump the generation counter → `initializeMcp`: per-server lifecycle
   registration and idle-override derivation, the bounded startup connect pass, the two-pass metadata
   build, `updateMetadataCache`, `updateStatusBar`, startup notifications with terminal sanitising.
9. Any tool that changed re-registers through HA-1 — **both halves of it**: the handle
   (`MCP-037`) puts the tool in the registry, and the `refresh_tools` fix (`MCP-037a`) is what lets
   `AgentSession::refresh_extension_tools` see it at the next turn boundary. With a warm cache that
   set is usually empty, which is why the warm path is **identical with or without HA-1**.

### 2 · Cold start, no cache (or stale / hash-mismatched)

Steps 1–3 and 6–7 as above. Then:

4. Zero direct tools are registered. The `mcp` proxy tool is registered with a description carrying no
   per-server counts.
5. `initializeMcp` sees the cache file **absent** and connects **everything once** (`MCP-019`) rather
   than honouring `lazy`; a hash-mismatched entry re-connects only that server.
6. Metadata is built and written; `updateStatusBar` reports the real counts.
7. **Without HA-1 the direct tools do not appear this session.** The `mcp` proxy tool is the whole
   model-facing surface — which is the documented single-tool path, and is what upstream itself falls
   back to. The next session is warm and identical to sequence 1. This is the entire user-visible cost
   of not building HA-1 — and, per Finding 1, it is also the cost of building **only** `MCP-037` and
   not `MCP-037a`, with the added hazard that the extension and the panel then believe the tools are
   live while the model cannot call them.

### 3 · Lazy connect on the first tool call

1. Model calls a direct tool, or `mcp({tool: "linear_create_issue", args})`.
2. `ExtHooks::before_tool_call` builds `HostEvent::ToolCall { call_id, name, input }` and dispatches
   block/mutate. `EventKind::ToolCall::fails_closed()` is `true` — the only kind that is — so a handler
   that traps, panics or blows its invocation budget **denies**.
3. `cyrup-permission-system` derives MCP targets and answers allow / ask / deny.
   `HostCtx::begin_human_wait` / `HumanWaitGate` suspend the dispatcher's invocation budget across a
   human answer so a slow approval cannot fail-**open**.
4. The executor resolves the server (prefix-driven, with the fail-closed ambiguity gate), then
   `lazyConnect`: transport construction → npx pre-resolution if the command is `npx`/`npm` →
   `serve_client_with_lifecycle` under the connection's `CancellationToken`.
5. If the connect returns a 401, `ClientInitializeError::auth_challenge()` yields the raw
   `WWW-Authenticate` header, the connection is marked `needs-auth`, and `attemptAutoAuth` runs once
   behind a single-shot latch (sequence 5) if `settings.autoAuth` allows it.

### 4 · A tool call end to end

```
before_tool_call gate  →  disabled-server check  →  owned-signal composition
  →  lazyConnect  →  auto-auth on needs-auth  →  connection assertion
  →  ensureToolCallApproved (approveTools globs / session cache / three-way select
                             under human_interaction_lock; headless ⇒ refuse)
  →  buildRequestOptions (per-server timeout, progress reset, meta)
  →  withSessionRecovery( Peer::send_request_with_option(CallToolRequest) )
  →  transformMcpContent  →  binary-resource materialisation (four limits, 0o600, wx)
  →  guardMcpOutput (byte/line caps, spill-to-file, truncation notice, details cap)
  →  error / abort mapping into the frozen details.error vocabulary
  →  finally: decrementInFlight + touch
  →  tool_result event  →  error-signal isError override
  →  render_call / render_result through the registered renderer
```

Every step is ordered, and the order is the specification. `callIdentity` is fixed once, before the
call, so a rename mid-flight cannot change what the renderer keys on.

### 5 · An OAuth acquisition

1. Trigger: either the reactive path (a connect 401 carrying a challenge) or the proactive one
   (`/mcp-auth <server>` on a disconnected server). **Which is the default is an open ruling** —
   reactive costs no extra round trip and is what rmcp's own client example does.
2. Reserve the loopback listener by refcount. It is **one** `TcpListener` for the whole process, at a
   fixed port and path, whose handler always returns `CallbackOutcome::Continue` — so it never settles
   and stays multi-tenant.
3. `AuthorizationManager::resolve_metadata_from_challenge` → RFC 9728 protected-resource metadata →
   RFC 8414 / OIDC authorization-server metadata, with the issuer echo check.
4. No stored `client_id` ⇒ `register_client` (RFC 7591). Re-apply the confidential half by hand
   afterwards, because `StoredCredentials` does not carry it.
5. `OAuthState::start_authorization` mints PKCE S256 + CSRF and writes `StoredAuthorizationState` to
   the `StateStore`; `get_authorization_url` adds RFC 8707 `resource` and the validated
   `authorizationParams`. `opener::open(url)`.
6. The browser hits `http://<host>:<port>/<path>?code=…&state=…`. The handler runs its eight branches
   and routes **by `state`** into that flow's oneshot. An unknown `state` must not settle another
   flow — this is the port's one genuine permission boundary in the OAuth path.
7. `handle_callback_with_issuer` validates the RFC 9207 `iss` **before** deleting the state — so a
   forged `iss` cannot destroy the legitimate callback's verifier, which is `keepPendingForRetry` for
   free — then exchanges the code.
8. `CredentialStore::save` writes `StoredCredentials` to the OS keychain under service
   `cyrup.mcp.oauth`, account `sha256-<hex>` of the server name, chunked past the value limit.
9. Release the refcount; `reconnectServer`. A parallel manual/headless leg accepts a pasted redirect
   URL or bare code and must race the callback safely.

### 6 · A `sampling/createMessage` arriving from a server

1. `ClientHandler::create_message`.
2. The rejections, in order, as byte-exact error strings: `includeContext !== "none"`,
   `params.tools?.length`, `params.toolChoice`, `params.stopSequences?.length`. The `task` guard is
   **structural** — the field does not exist on `CreateMessageRequestParams`, and task augmentation is
   an extension `cyrup-mcp` never declares.
3. `resolveSamplingModel` walks `modelPreferences` hints against `cyrup_provider::catalog` plus
   `HostServices::{models, scoped_models, current_model}`, first-wins on substring, with a sequential
   auth probe.
4. **Approval gate 1** — `HostServices::confirm` under `human_interaction_lock`, showing server name,
   resolved model, system prompt and message previews. Skipped when `samplingAutoApprove`.
5. `throwIfAborted` → the completion runs through **`cyrup-provider` directly**, exactly as upstream
   imports `complete` from `pi-ai/compat` and bypasses pi's host API → `throwIfAborted`.
6. **Approval gate 2** — the response preview. Then `CreateMessageResult` with the mapped stop reason.

Inverting either gate is a permission bypass, which is why `MCP-455` is one of the fourteen criticals.

### 7 · An `elicitation/create` (form)

1. `ClientHandler::create_elicitation`; rmcp's default returns `Decline`, so an unimplemented handler
   is fail-safe and matches upstream's behaviour when `settings.elicitation` is off.
2. Absent or unrecognised `mode` deserialises as form through the untagged `LegacyForm` arm.
3. Iterate **`ElicitationSchema::property_order`**, not the `BTreeMap`. Per primitive:
   enum single-select → `select`; enum multi-select → the toggle loop; boolean → `confirm`;
   string / number / integer → `input` with the placeholder. All under `human_interaction_lock`, and
   `HostCtx::begin_human_wait` held across every one.
4. `coerceAndValidateFormValues` with JS `Number()` semantics, `format` as an **assertion** not an
   annotation, `minimum`/`maximum`/`minLength`/`maxLength`/`required`, and a per-field re-prompt loop.
5. Review loop with an edit picker; Escape or empty ⇒ `Decline`; abort ⇒ `Cancel`; otherwise
   `ElicitResult::with_content`.

The URL leg is shorter: consent → `opener::open` → `Accept`, with the `elicitation_id` recorded so the
later `notifications/elicitation/complete` (which arrives at `on_custom_notification`, since rmcp does
not model it first-class) fires the retry notice exactly once.

### 8 · Shutdown

1. `EventKind::SessionShutdown` → `shutdownState`, four steps in this order: publish the shutdown
   snapshot → **flush the metadata cache, capturing its error** → stop the runtime owner → **rethrow
   the flush error in preference to the shutdown error**.
2. The owner's cleanups run **LIFO**: `lifecycle.gracefulShutdown()` → `shutdownOAuth` →
   `cleanupMaterializedBinaryResources`. That order matters: an in-flight OAuth callback must be
   refusable *after* the servers close.
3. `gracefulShutdown` is memoised and waits for the in-flight health check.
4. Per stdio child, `TokioChildProcess::graceful_shutdown` closes the transport, waits **3 s**, then
   hard-kills. **Named delta:** the TS SDK does close-stdin → 2 s → SIGTERM → 2 s → SIGKILL. rmcp has
   no SIGTERM leg and signals a single pid rather than a process group — which is correct *because*
   npx pre-resolution removed the npm launcher that made the group necessary.
5. A session restart re-runs the whole thing: `AgentSessionRuntime::new_session_with` builds the
   replacement — re-running `init()` — **before** the outgoing session is disposed, so registration
   and teardown do not interleave.

---

## How it interfaces with the rest of cyrup

### `cyrup-ext` — the host itself

`cyrup-mcp` implements `NativeExtension` and consumes `InitApi`, `HostServices`, `HostCtx`,
`EventKind` and `InteractiveOverlay`. Two flags are load-bearing and easy to get wrong:
`is_ambient` must be **`true`** (it is the sole `--no-extensions` gate, and `pi-mcp-adapter` is an
installed package upstream, so `--no-extensions` must switch it off), and `decides_project_trust` must
stay **`false`** (opting in runs `init` **twice on the same object** in the pre-trust bootstrap pass,
and this `init` is not idempotent). All three host additions land here — HA-1 as two edits, the
handle and the `refresh_tools` correction of Finding 1. One `pub` promotion is needed:
`cyrup_ext::caps::proc::npx_resolver`'s `resolve_npx_binary` and `NpxResolution` are `pub(super)`
inside a private `mod`, and the alternative to promoting them is copying 892 already-ported lines.

### `cyrup-permission-system` — MCP targets already exist

This crate **already ports the MCP half of pi's `permission-manager.ts`**, and that fixes three
cross-crate contracts:

- `create_mcp_permission_targets` reads exactly `{tool, server, connect, describe, search}` off the
  `mcp` tool's arguments, in that precedence, and derives targets in pi's order —
  `<server>_<tool>`, `<server>:<tool>`, `<server>`, `<tool>`, the raw reference, plus the mode
  baseline. **`MCP_BASELINE_TARGETS` is `["mcp_status", "mcp_list", "mcp_search", "mcp_describe",
  "mcp_connect"]`.** **Renaming a parameter silently changes which permission rules apply.** The extra
  parameters the port keeps (`args`, `regex`, `includeSchemas`, `limit`, `offset`, `instructions`,
  `action`) are not read by the derivation and are safe.
- The default arm matters: an `mcp({action: …})` or `mcp({instructions: …})` call falls through to
  `mcp_status`.
- `read_configured_mcp_server_names` reads `<agent_dir>/mcp.json` **directly**, through this crate's
  own `jsonc`, accepting either `mcpServers` or `mcp-servers`, sorted length-desc then
  lexicographic. The global MCP config path is a contract, not a choice.

The approval broker does not port and does not need to: `before_tool_call` **is** the broker,
structurally, and it is already wired for every origin that survives the cuts.

### `cyrup-ext-subagents` — the hardest external constraint in the port

`cyrup_ext_subagents::exec::mcp_direct_tools` **already reads** `<agent_dir>/mcp-cache.json` in Rust:
`CACHE_VERSION = 1`, `CACHE_MAX_AGE_MS` = 7 days, the
`{ version, servers: { <name>: { configHash, timestamp, tools[], resources[], prompts[] } } }` shape,
with `compute_mcp_server_hash` already ported. **`cyrup-mcp` is the writer of a file that already has
a reader**, and the digests must be byte-identical or every `mcp:` subagent tool selector silently
resolves to nothing.

The reader and the upstream writer **do not currently agree**, in five ways, each of which
independently changes the digest or the name for essentially every server:

| # | divergence | consequence | unit |
|---|---|---|---|
| 1 | the hash covers **11** keys, not 14 (`socket`, `protocolVersion`, `includeTools` absent), and hashes `url` **raw** rather than through `resolveServerUrl` | every digest differs | `MCP-141` |
| 2 | `stable_stringify` maps an absent field to `"null"`; upstream emits the bare 9-character token `undefined` | every digest differs, since the identity object always carries all keys | `MCP-142` |
| 3 | `interpolate_env_vars` implements two of upstream's three patterns — `{env:NAME}` is missing | values differ wherever that form is used | `MCP-143` |
| 4 | `!`/`!!` secret-expression semantics are skipped: `!!X` is not un-escaped and `!cmd` **is** interpolated | wrong hashed value, and a latent execution-timing hazard | `MCP-144` |
| 5 | resource tools are named `get_<name>`; upstream builds `read_<name>` | the name differs, so the selector misses | `MCP-146` |

**These must land as one change with a golden-vector fixture generated from the TypeScript.** They are
three of the port's fourteen criticals. Do **not** bump `CACHE_VERSION` to drop the now-dead
`uiResourceUri` / `uiStreamMode` fields — leave them absent and ignored.

### `cyrup-tui` — the panels

Through `HostServices::open_overlay(Box<dyn InteractiveOverlay>) -> bool` only. `InteractiveOverlay`
(`crates/cyrup-ext/src/host/overlay.rs`) is the direct counterpart of pi's
`ctx.ui.custom(factory, { overlay: true })` `Component`: `render(width, height) -> Vec<OverlayLine>`,
`handle_key(OverlayKey) -> OverlayOutcome`, `refresh_ms`, `tick`. `LiveHostServices::open_overlay`
forwards to the mode's overlay sink and blocks on a one-shot with `block_in_place` until teardown — no
timeout, matching `await ctx.ui.custom(...)` — and returns `false` **without blocking** when no
interactive surface is attached, which is pi's `if (!ctx.hasUI)` branch as a return value.
`cyrup-tui`'s `ExtensionOverlay` fires the one-shot **on drop**, so every teardown path releases the
blocked extension task. Two working precedents exist and they cover both shapes the panels need:
`cyrup_ext_subagents::tui::fleet_overlay::FleetOverlay` for async work spawned out of an overlay and
drained back into it, and `cyrup_permission_system::config_modal::PermissionSystemSettingsOverlay`
for reading a result off an `Arc`-shared object **after** `open_overlay` has returned — which is the
only way a `bool` return can carry `McpPanelResult` (`MCP-369`, `MCP-394`). One delta remains:
geometry, which is HA-3, cosmetic, and owned by `MCP-368`. Keybindings are **not** a delta — a native
crate links `cyrup-config` and reads `<agent_dir>/keybindings.json` directly, which is the only way to
honour a user's `mcp.panel.save` (`MCP-363`).

### `cyrup-config` — `mcp.json` and the dirs

`<agent_dir>` is the `ConfigDirs::agent_dir` **field** — a `PathBuf` populated by `ConfigDirs::resolve`
from the CLI flag, then `$CYRUP_AGENT_DIR` / `$PI_CODING_AGENT_DIR`, then the default; it is not a
method, so nothing calls it and `cyrup-mcp` takes the resolved `ConfigDirs` (or the path off it) as a
constructor argument the way `cyrup_ext_subagents::subagent_extension_for_env` takes it. **Three agent-dir resolvers exist in the
workspace today and two disagree on the home source**, so `mcp-cache.json` and `mcp-npx-cache.json`
can land in different directories under exactly the `CYRUP_AGENT_DIR`/`CYRUP_HOME` configurations CI
and subagent isolation use. Consolidating them is a one-resolver chore that benefits every crate that
reads the agent dir. `cyrup-config`'s `project_config_dir()` settles the `.cyrup/` question for
`.mcp.json` and the trace directory.

### `cyrup-provider` — sampling, and the callback server

Two direct uses, neither a layering inversion (`cyrup-ext` itself already depends on
`cyrup-provider`): `cyrup_provider::catalog::{builtin_catalog, load_catalog}` plus the completion path
for sampling, and `cyrup_provider::auth::oauth::callback` for the OAuth loopback listener —
**reused, not rebuilt**. `CallbackServerConfig::{fixed, ephemeral, with_host, advertising,
with_cancel}` already covers strict-vs-ephemeral port, path, bind host and advertise host, and
`OAuthError::Listen{source}` distinguishes `AddrInUse`. Also copy `CredentialStore::modify`'s
serialized per-id read-modify-write shape (the mutex half, not its `FileLock` half): it restores
exactly the atomicity single-threaded JavaScript supplied for free.

### `cyrup-tools` — naming and collisions against builtins

No dependency, one contract: **`resolveDirectTools` must drop an MCP tool whose name collides with a
cyrup builtin.** `ExtensionRegistry::active_tools` lets a registered tool override, so without the
drop an MCP server can silently replace `read`, `bash` or `edit`. That is `MCP-212`, and it is
critical for exactly that reason. The five collision/advisory warnings and the 75-tool advisory route
through `HostServices::notify`.

### `cyrup-ext`'s caps — `npx_resolver` is already this package's own code

`cyrup_ext::caps::proc::npx_resolver` is an 892-line direct port of `npx-resolver.ts` with the same
cache version, 24 h TTL and 30 s force-cache timeout. Reuse it; do not re-port it; do not let the two
copies drift. One rewiring is required: upstream resolves npx **in the connection builder**, while
cyrup's `apply_npx_resolution` sits inside `ProcCaps::spawn` — the *guest* path — so `cyrup-mcp` must
call `resolve_npx_binary` itself. Six confirmed gaps against v2.25.0 are filed as `MCP-104`…`MCP-108`;
the one that bites is that `package_version` is written but never read, so `npx -y srv@1.2.3` spawns
whatever binary has the newest mtime.
---

## What is genuinely hard

After the cuts, after `rmcp`, and after the prerequisites dissolved, six things are left that are
genuinely difficult rather than merely long.

**1 · The frozen metadata-cache contract.** This is the only place in the port where a mistake is
*silent*. `cyrup-mcp` writes a file another shipping crate already reads, the two sides disagree in
five ways today, and the failure mode of a mismatch is an empty subagent tool allowlist with no error
on any path. It needs a golden-vector fixture generated from the TypeScript, and the five edits to
`mcp_direct_tools` must land in the same change as the writer — `MCP-070`, `MCP-094`, `MCP-139`…
`MCP-146`, `MCP-205`.

**2 · Dynamic registration lifetime.** HA-1 is small to *build* — two edits, one of them a single line
(Finding 1). What is hard is the surrounding choreography: the fingerprint diff that decides whether
to re-register, `freezeDirectTools`, the re-activation path for a tool that was deactivated through
the `setActiveTools` fallback and comes back, and the fact that a removed tool's *name* stays in the
registry for the session (upstream's own `unregisterTool === undefined` branch). `MCP-036`, `MCP-037`,
`MCP-037a`, `MCP-217`, `MCP-217a`, `MCP-395`.

**3 · The two TUI panels — the hardest thing in the port.** 1,681 lines of fully interactive UI:
server list, tool tree with per-tool direct-tool toggles, fuzzy filter, token estimates, the
14-row key-dispatch table, two modals, in-panel OAuth and reconnect, the tri-state save, and a second
panel with a 13-row action table, five presets and nine previews — all reimplemented on
`InteractiveOverlay`, whose `open_overlay` returns only a `bool`, so every result has to escape
through caller-supplied shared state. Thirty port units cover them (`MCP-351`…`MCP-380`), six of them
`critical`. **None of it is verifiable by unit test.** ratatui's `TestBackend` passes while the
assembled application has layout and empty-state bugs; nothing here is done until it has been run in a
real terminal, and seven of those units carry that requirement in their `verify` line.

**4 · The credential path.** Chunking past the keychain value limit with a manifest, the
process-lifetime cache with three external invalidation points, the legacy plaintext import (which
must *not* delete its keychain source, unlike the file source, because a co-installed `pi-mcp-adapter`
still owns it), the two byte-exact credential-store-unavailable messages, and the `keyctl session -`
re-exec. Compounded by an unresolved ruling on which Linux backend `keyring` links — which determines
whether four of those units are live code or dead code.

**5 · OAuth error paths.** The happy path is `rmcp`. What is not: the callback handler's eight
branches with their conditional synchronous removals, `startAuth`'s ordering and its four-phase
aggregate cleanup, the five stale-registration branches including one deliberate `clearTokens`, the
5-minute abandoned-flow timer, `authenticate`'s in-flight dedup, and the manual-paste leg racing the
callback. `MCP-306` is the port's only genuine permission boundary outside the approval gates.

**6 · Verification has no cheap oracle.** The MCP conformance harness tests the **wire**, and the wire
is rmcp's — which rmcp already gates upstream, running `--suite all` against two spec versions with no
expected-failures file. Everything this port actually adds sits *above* the wire, and the only
available differential oracle is the TypeScript adapter itself, through a trace-JSONL harness that
requires keeping `pi-mcp-adapter` checked out and installable in CI. `MCP-483`…`MCP-499`.

### Open rulings

**Thirty-one rulings are open; seventeen carry their own `open-decision` port unit and fourteen ride
inside units whose verdict is something else.** These seven change the design and should be settled
before code is written; the rest are recorded in the section files with options and a recommendation.
The two added by the restored TUI section are `MCP-363a` (where the canonical select-key defaults
live) and `MCP-370` (tool/resource/prompt name formatting versus the in-tree consumer — the panel side
of the `MCP-205` naming reconciliation, and a `critical` in its own right).

| ruling | options | recommendation |
|---|---|---|
| **OPEN-1 · which Linux credential store `keyring` links** | (a) `features=["v1"]` — smallest tree, but v1 selects Secret Service over zbus, **not** kernel keyutils, so a revoked session keyring cannot occur and `MCP-260`/`261`/`262`/`287` become **dead code that must be cut in the same breath** — and a headless box with no D-Bus has no store at all, which is exactly the environment upstream's recovery path serves; (b) `features=["cli"]` + `keyring_core::Entry` — reproduces `@napi-rs/keyring`'s backend but pulls dbus/zbus/sqlite/sample; (c) link `keyring-core` plus the three platform stores directly and `set_default_store` per platform — what `keyring` 4.x's own docs prescribe for an application, upstream's exact backend set, smallest tree that keeps keyutils | **(c)**, falling back to (b) if the settled `keyring = 4.1.6` line must be preserved verbatim. (a) only as an explicit product decision that headless Linux without D-Bus is unsupported |
| **`MCP-117` · `Auto` negotiation against a stdio server that exits on `server/discover`** | rmcp's `serve_client_with_lifecycle` runs `discover_startup` and `legacy_startup` on the **same** `&mut transport` and returns `Legacy` only when the probe produced a correlated JSON-RPC error; a child that *exits* produces a transport error with no fallback. Upstream ships a fixture for exactly this. (a) adopt as-is and record the loss; (b) for stdio + `auto` only, spawn a disposable sibling child, run `Discover` on it, then open the real child pinned to the negotiated revision; (c) hand-roll — ruled out | **(b)** — the only option preserving upstream's observable behaviour, bounded to one config arm, needing nothing from cyrup's core. (a) only with explicit sign-off that discover-intolerant servers may break |
| **`MCP-205` · which naming grammar governs the process** | (a) leave both copies; (b) make `cyrup-ext-subagents` depend on `cyrup-mcp`; (c) a shared naming crate both depend on | **(c)** — decided above as `cyrup-mcp-naming`. Doing nothing is not defensible: the failure is a silently empty subagent allowlist |
| **`MCP-096` · do project-scoped config sources honour project trust** | upstream applies **no gate**; cyrup's `SettingsManager` skips the whole project layer for an untrusted project, and `HostServices::is_project_trusted` makes gating one call. A project-local `.mcp.json` can name an arbitrary stdio `command` and an `!`-prefixed `env` value that runs a shell command at connect | **gate**, reporting the sources as present-but-untrusted in `/mcp` status, with the divergence recorded. The security delta is real and one-sided |
| **`MCP-135` · `reinit_on_expired_session`** | on, it transparently replays `initialize` and retries the in-flight request — covering upstream's 404 arm and nothing else, while stacking a second silent one-shot retry under `withSessionRecovery` and hiding the reconnect the adapter wanted | **off**; port `withSessionRecovery` literally |
| **`MCP-048` · is `~/.pi/agent` a migration source** | `cyrup-permission-system` resolves `<agent_dir>/mcp.json` **independently**, so a fallback living only inside `cyrup-mcp` would make the permission gate enumerate a different (empty) server set than the extension runs — permissions too permissive or too strict, with no error. (a) `~/.cyrup/agent` only + a one-way migration; (b) dual-read from a **shared** resolver; (c) permanent extra source | **(a)** |
| **`MCP-118` · the client identity string** | upstream announces `pi-mcp-<server>`; `cyrup-mcp-<server>` is the obvious rename, but any MCP server that allow-lists or fingerprints the pi client name will not recognise cyrup | **rename** — misrepresenting the client to a remote server to inherit an allow-list is worse than being refused — and record it so a "why does this server reject us" report has an answer |

Three rulings are near-settled and are stated as instructions rather than questions: start the
conformance expected-failures baseline **empty** and write it from an observed run (all five of
upstream's entries are scenarios rmcp implements and does not baseline, and a *passing* baselined
entry fails the run); track rmcp's conformance-harness pin rather than upstream's; and bind
`127.0.0.1` while advertising `localhost` through the existing `CallbackServerConfig::advertising`,
with the IPv6-only-localhost residual named.

### One named residual risk, not a decision

`MCP-262`'s revoked-keyring predicate matches `Display` text. The outer layer is confirmed —
`keyring_core::Error::NoStorageAccess` renders byte-identically to the string upstream's
fault-injection store fabricates — but the **inner** platform error's rendering under a genuinely
revoked kernel session keyring has not been observed, and macOS never revokes a keyring, so
`MCP-260`/`261`/`262`/`287` are exercised on a dev machine only through a forced test variable and a
fake `keyctl`. Under OPEN-1 option (a) the question does not arise at all.

---

## Port units

433 units. `id · severity · effort · verdict · title · owning section file`. Bodies — upstream
behaviour, the cyrup mechanism, the verify line — live in the section files; this is the one complete
enumeration.

Severity is the house scale: **`critical` = data loss, silent wrong output, a permission bypass, or a
crash on a normal path.** Four clauses, no fifth. Blocking-ness is **not** severity — "without this
the subsystem is inert" is scheduling information and lives in the unit body. Effort: `S` under a day
· `M` a few days · `L` a week+ or needs design. `n/a` severity marks a unit that proposes no
user-visible behaviour of its own (crate scaffolding, trackers, and every `cut` record); `n/a` effort
marks the four units that are pure records with no work attached.

Where a section file's unit header carries a **compound** verdict — `hand-written` + `host-verb`, and
thirty-odd others — this table and the census show the **first-listed** one, so the seven verdict
buckets sum to the total. The one place that loses information is the host-addition legs, so they are
enumerated in full above rather than inferred from this column.

| id | sev | eff | verdict | title | § | status |
|---|---|---|---|---|---|---|
| `MCP-001` | n/a | M | `hand-written` | Stand up `crates/cyrup-mcp` and attach it at the session-build arms | `13a-mcp-activation.md` | done |
| `MCP-002` | low | S | `host-verb` | Read `--mcp-config` from argv directly, and register the flag for `--help` | `13a-mcp-activation.md` | done |
| `MCP-003` | critical | L | `host-verb` | Register the entire tool/command surface from disk caches inside `init()`, and never fail | `13a-mcp-activation.md` | done |
| `MCP-004` | high | M | `hand-written` | Port `McpRuntimeOwner` | `13a-mcp-activation.md` | done |
| `MCP-005` | medium | S | `hand-written` | Reverse-order cleanup, the aggregate error, and the late-cleanup path | `13a-mcp-activation.md` | done |
| `MCP-006` | medium | M | `extension-owned` | Port `createOwnedUi` as a fenced services handle | `13a-mcp-activation.md` | **partial** |
| `MCP-007` | medium | S | `hand-written` | Port the abort helpers (combineAbortSignals, isAbortError, throwIfAborted, abortable) | `13a-mcp-activation.md` | done |
| `MCP-008` | high | M | `hand-written` | The `session_start` generation protocol, abort-before-await | `13a-mcp-activation.md` | **partial** |
| `MCP-009` | high | S | `hand-written` | The `session_shutdown` handler | `13a-mcp-activation.md` | **partial** |
| `MCP-010` | high | S | `hand-written` | `shutdownState`, preserving the metadata-flush error | `13a-mcp-activation.md` | **partial** |
| `MCP-011` | high | M | `hand-written` | `startInitialization`'s triple staleness check and metadata-update hook install | `13a-mcp-activation.md` | **MISSING** |
| `MCP-012` | medium | S | `extension-owned` | `startLoadTimeInitialization` — the eager/keep-alive pre-warm | `13a-mcp-activation.md` | **partial** |
| `MCP-013` | low | S | `hand-written` | The `MCP_DIRECT_TOOLS` blocking wait at session start | `13a-mcp-activation.md` | **partial** |
| `MCP-014` | high | M | `hand-written` | Re-`init` per session, and the build-before-dispose inversion | `13a-mcp-activation.md` | **partial** |
| `MCP-015` | medium | S | `extension-owned` | Snapshot every context value before the first await in `initialize` | `13a-mcp-activation.md` | **partial** |
| `MCP-016` | medium | M | `hand-written` | The sampling and elicitation wiring gates | `13a-mcp-activation.md` | **partial** |
| `MCP-017` | medium | S | `hand-written` | Register owner cleanups in the exact LIFO order, plus the list-changed listener | `13a-mcp-activation.md` | **partial** |
| `MCP-018` | low | S | `hand-written` | The zero-enabled-servers early return | `13a-mcp-activation.md` | **partial** |
| `MCP-019` | medium | S | `hand-written` | Metadata-cache bootstrap: file-absent means connect everything once | `13a-mcp-activation.md` | **MISSING** |
| `MCP-020` | medium | S | `hand-written` | Per-server lifecycle registration and idle-override derivation | `13a-mcp-activation.md` | **partial** |
| `MCP-021` | medium | M | `hand-written` | Rehydrate tool/resource/prompt/instruction metadata from a hash-valid cache entry | `13a-mcp-activation.md` | **MISSING** |
| `MCP-022` | medium | M | `hand-written` | The bounded startup connect pass | `13a-mcp-activation.md` | **MISSING** |
| `MCP-023` | high | M | `hand-written` | The two-pass startup metadata build | `13a-mcp-activation.md` | **MISSING** |
| `MCP-024` | medium | S | `hand-written` | Failure tracking with a 60-second backoff | `13a-mcp-activation.md` | **MISSING** |
| `MCP-025` | high | S | `hand-written` | Startup connect notifications, terminal sanitising, and skipped-tool warnings | `13a-mcp-activation.md` | **partial** |
| `MCP-026` | low | S | `hand-written` | The `MCP_DIRECT_TOOLS` cache-bootstrap pass inside `initialize` | `13a-mcp-activation.md` | **MISSING** |
| `MCP-027` | medium | S | `hand-written` | Lifecycle callbacks (reconnect, reconnect-failure, idle shutdown) | `13a-mcp-activation.md` | **partial** |
| `MCP-027a` | medium | S | `hand-written` | `sendMessage`'s `triggerTurn` pre-turn convergence gate **(v2.26.1 retarget, 2026-08-20)** | `13a-mcp-activation.md` | **MISSING** |
| `MCP-028` | medium | S | `hand-written` | `updateServerMetadata` | `13a-mcp-activation.md` | **MISSING** |
| `MCP-029` | high | M | `hand-written` | `updateMetadataCache` write rules | `13a-mcp-activation.md` | **partial** |
| `MCP-030` | low | S | `hand-written` | `notifyToolMetadataUpdated` must never let a hook break a connect | `13a-mcp-activation.md` | **partial** |
| `MCP-031` | medium | S | `hand-written` | `flushMetadataCache` on shutdown | `13a-mcp-activation.md` | **MISSING** |
| `MCP-032` | low | S | `host-verb` | `updateStatusBar` — the three footer verbosities | `13a-mcp-activation.md` | **partial** |
| `MCP-033` | medium | M | `hand-written` | `lazyConnect` | `13a-mcp-activation.md` | **MISSING** |
| `MCP-034` | medium | M | `hand-written` | `McpLifecycleManager` — the health-check state machine | `13a-mcp-activation.md` | done |
| `MCP-035` | high | S | `hand-written` | `gracefulShutdown` — memoised, and it waits for the in-flight check | `13a-mcp-activation.md` | done |
| `MCP-036` | medium | M | `hand-written` | `syncDirectTools`: the fingerprint diff, the re-activation path, and the renderer declaration | `13a-mcp-activation.md` | **partial** |
| `MCP-037` | high | M | `host-addition` | HA-1: a native extension has no handle to `ExtensionHost::register_late_tool` | `13a-mcp-activation.md` | **MISSING** |
| `MCP-037a` | critical | S | `host-addition` | HA-1b: `refresh_tools` drops the native tier's dirty flag in the `wasm-host` build | `13a-mcp-activation.md` | done |
| `MCP-038` | medium | S | `host-verb` | `deactivateTools`: the optional `unregisterTool` primary path and the `setActiveTools` fallback | `13a-mcp-activation.md` | **MISSING** |
| `MCP-039` | medium | S | `host-addition` | MCP prompts as slash commands registered after `init` | `13a-mcp-activation.md` | **partial** |
| `MCP-040` | medium | L | `host-verb` | The `/mcp` command handler | `13a-mcp-activation.md` | **MISSING** |
| `MCP-041` | medium | M | `host-addition` | HA-2: `/mcp`'s dynamic argument completions have no native path and no TUI consumer | `13a-mcp-activation.md` | **MISSING** |
| `MCP-042` | medium | M | `host-verb` | The `/mcp-auth` command handler | `13a-mcp-activation.md` | **MISSING** |
| `MCP-043` | high | L | `hand-written` | The `mcp` gateway tool: registration, the init wait, and the dispatch order | `13a-mcp-activation.md` | **partial** |
| `MCP-044` | n/a | S | `cut` | The `mcpScript` tool | `13a-mcp-activation.md` | n/a |
| `MCP-045` | medium | S | `host-verb` | The `tool_result` `isError` override | `13a-mcp-activation.md` | **partial** |
| `MCP-046` | medium | S | `hand-written` | The abort call-site discipline inside the runtime | `13a-mcp-activation.md` | **partial** |
| `MCP-047` | critical | M | `hand-written` | Port `agent-plugin-loader.ts` | `13a-mcp-activation.md` | done |
| `MCP-048` | high | S | `open-decision` | Agent-directory resolution, and whether `~/.pi/agent` is a migration source | `13a-mcp-activation.md` | done |
| `MCP-049` | medium | M | `hand-written` | Port `cli.js init` as a `cyrup mcp init` subcommand | `13a-mcp-activation.md` | **MISSING** |
| `MCP-050` | n/a | M | `extension-owned` | Create `cyrup-mcp` and its config module skeleton | `13b-mcp-config.md` | done |
| `MCP-051` | high | S | `extension-owned` | Read `mcp.json` as JSONC, not JSON | `13b-mcp-config.md` | done |
| `MCP-052` | high | M | `hand-written` | Port the six-source precedence ladder | `13b-mcp-config.md` | done |
| `MCP-053` | critical | M | `hand-written` | Port `mergeServerMaps`, including URL-bound credential stripping | `13b-mcp-config.md` | done |
| `MCP-054` | n/a | S | `cut` | socket ⇄ command/url transport-swap stripping | `13b-mcp-config.md` | n/a |
| `MCP-055` | medium | S | `hand-written` | Port `expandImports` / `mergeImports` | `13b-mcp-config.md` | done |
| `MCP-056` | medium | M | `hand-written` | Port the 7 host-config import families | `13b-mcp-config.md` | done |
| `MCP-057` | medium | M | `hand-written` | Port the `opencode` multi-file merge and entry translation | `13b-mcp-config.md` | done |
| `MCP-058` | medium | S | `hand-written` | Port `hostConfigDiscovery` and `loadDiscoveredHostConfigs` | `13b-mcp-config.md` | done |
| `MCP-059` | medium | M | `hand-written` | Port `getMcpDiscoverySummary`, conflicts and the fingerprint | `13b-mcp-config.md` | done |
| `MCP-060` | low | S | `hand-written` | Port RepoPrompt detection and `KNOWN_SERVER_PRESETS` | `13b-mcp-config.md` | done |
| `MCP-061` | high | S | `extension-owned` | Port the atomic raw-config writer | `13b-mcp-config.md` | done |
| `MCP-062` | low | S | `hand-written` | Port `buildUnifiedDiff` (LCS) and `ConfigWritePreview` | `13b-mcp-config.md` | done |
| `MCP-063` | high | M | `hand-written` | Port `writeProjectServerDisabledOverride` | `13b-mcp-config.md` | done |
| `MCP-064` | medium | M | `hand-written` | Port `getServerProvenance` and `writeDirectToolsConfig` | `13b-mcp-config.md` | done |
| `MCP-065` | low | S | `hand-written` | Port `ensureCompatibilityImports`, starter config and shared-entry writers | `13b-mcp-config.md` | done |
| `MCP-066` | high | M | `hand-written` | Port `McpSettings` as a permissive struct with per-site defaults | `13b-mcp-config.md` | done |
| `MCP-067` | medium | S | `hand-written` | Port the settings merge as a one-level key merge | `13b-mcp-config.md` | done |
| `MCP-068` | high | S | `hand-written` | Port env-var overrides, including the `__none__` sentinel | `13b-mcp-config.md` | **partial** |
| `MCP-069` | high | M | `hand-written` | Port `ServerEntry` as a typed struct | `13b-mcp-config.md` | done |
| `MCP-069a` | critical | S | `hand-written` + `open-decision` | Fail **closed** on a malformed `requestHeadersCommand` **(v2.26.1 retarget, 2026-08-20)** | `13b-mcp-config.md` | n/a |
| `MCP-070` | high | M | `hand-written` | Enforce the absent-vs-null hash pre-image contract | `13b-mcp-config.md` | **partial** |
| `MCP-071` | high | S | `hand-written` | Port `ToolPrefix` with all four modes and `sanitizeServerPrefix` | `13b-mcp-config.md` | done |
| `MCP-072` | high | S | `hand-written` | Port `formatToolName` / `resolveToolPrefix` | `13b-mcp-config.md` | done |
| `MCP-073` | high | S | `hand-written` | Port `resolveServerFromToolName` with its ambiguity fail-safe | `13b-mcp-config.md` | **MISSING** |
| `MCP-074` | medium | S | `hand-written` | Port `sanitizePromptName` / `formatPromptCommandName` | `13b-mcp-config.md` | done |
| `MCP-075` | high | M | `hand-written` | Port `getToolNameCandidates` (the legacy candidate set) | `13b-mcp-config.md` | **partial** |
| `MCP-076` | high | M | `hand-written` | Port glob matching and `isToolIncluded`/`isToolExcluded`/`isToolAllowed` | `13b-mcp-config.md` | **partial** |
| `MCP-077` | high | S | `hand-written` | Port the metadata/cache type model | `13b-mcp-config.md` | done |
| `MCP-078` | medium | S | `extension-owned` | Port the status-snapshot types | `13b-mcp-config.md` | **partial** |
| `MCP-079` | medium | S | `hand-written` | Port the tool-approval decision and origin types | `13b-mcp-config.md` | **partial** |
| `MCP-080` | n/a | S | `cut` | MCP-UI type surface in `types.ts` | `13b-mcp-config.md` | n/a |
| `MCP-081` | medium | S | `hand-written` | Port `McpAdapterOptions` / programmatic config mode | `13b-mcp-config.md` | done |
| `MCP-082` | high | S | `hand-written` | Port `interpolateEnvVars` including the `{env:VAR}` form | `13b-mcp-config.md` | done |
| `MCP-083` | critical | M | `extension-owned` | Port `!` / `!!` command-secret resolution | `13b-mcp-config.md` | **partial** |
| `MCP-084` | high | S | `hand-written` | Port `resolveServerUrl` / `resolveConfigPath` / `resolveBearerToken` | `13b-mcp-config.md` | **partial** |
| `MCP-085` | medium | M | `hand-written` | Port terminal sanitisation and error flattening | `13b-mcp-config.md` | **partial** |
| `MCP-086` | medium | S | `extension-owned` | Port the browser/path open dispatch | `13b-mcp-config.md` | **partial** |
| `MCP-087` | medium | S | `hand-written` | Port `parallelLimit`, argv scan, `toStringRecord`, `normalizeDirectToolInputSchema` | `13b-mcp-config.md` | **partial** |
| `MCP-088` | medium | S | `host-verb` | Port `formatMcpStatus` and `formatAuthRequiredMessage` | `13b-mcp-config.md` | done |
| `MCP-089` | medium | S | `hand-written` | Port the error taxonomy | `13b-mcp-config.md` | **partial** |
| `MCP-090` | low | S | `extension-owned` | Port the logger as a `tracing` adapter | `13b-mcp-config.md` | **partial** |
| `MCP-091` | medium | M | `hand-written` | Port `renderTsShape` | `13b-mcp-config.md` | **MISSING** |
| `MCP-092` | high | S | `hand-written` | Port the dual-dialect JSON Schema validator | `13b-mcp-config.md` | **MISSING** |
| `MCP-093` | medium | S | `hand-written` | Register the `ajv-formats` formats `jsonschema` does not ship | `13b-mcp-config.md` | **MISSING** |
| `MCP-094` | high | L | `hand-written` | Reconcile `mcp_direct_tools` with this section's contract | `13b-mcp-config.md` | **MISSING** |
| `MCP-095` | n/a | S | `extension-owned` | JSONC parser home | `13b-mcp-config.md` | done |
| `MCP-096` | high | S | `open-decision` | Project trust and the two project-scoped config sources | `13b-mcp-config.md` | n/a |
| `MCP-097` | low | S | `hand-written` | Port `getConfigDiscoveryPaths` and `findAvailableImportConfigs` | `13b-mcp-config.md` | done |
| `MCP-098` | medium | S | `hand-written` | Preserve `renderTsShape`'s re-entrant alias emission | `13b-mcp-config.md` | **MISSING** |
| `MCP-099` | low | S | `hand-written` | Reproduce `buildConfigWritePreview`'s reserialised "before" text | `13b-mcp-config.md` | done |
| `MCP-100` | high | L | `hand-written` | McpServerManager: the five race guards and the full public API | `13c-mcp-servers.md` | **MISSING** |
| `MCP-101` | high | M | `rmcp` | stdio transport: spawn, env resolution, cwd, plugin data dir | `13c-mcp-servers.md` | **partial** |
| `MCP-102` | medium | S | `rmcp` | stderr tail capture and failure-message enrichment | `13c-mcp-servers.md` | **partial** |
| `MCP-103` | medium | S | `extension-owned` | Wire npx/npm resolution into the connection builder | `13c-mcp-servers.md` | **MISSING** |
| `MCP-104` | medium | S | `hand-written` | npx cache: bump to CACHE_VERSION = 2 and port clearLegacyCache | `13c-mcp-servers.md` | **MISSING** |
| `MCP-105` | high | M | `hand-written` | npx resolver: exact package-version pinning is missing | `13c-mcp-servers.md` | **MISSING** |
| `MCP-106` | low | S | `hand-written` | npx resolver: cache key must be [command, packageSpec, binName] | `13c-mcp-servers.md` | **MISSING** |
| `MCP-107` | medium | S | `hand-written` | npx resolver: no cancellation path | `13c-mcp-servers.md` | **MISSING** |
| `MCP-108` | low | S | `hand-written` | npx resolver: entry-level cache validation and Windows npm resolution | `13c-mcp-servers.md` | **MISSING** |
| `MCP-109` | high | S | `rmcp` | Streamable HTTP client transport | `13c-mcp-servers.md` | **partial** |
| `MCP-110` | n/a | n/a | `cut` | Legacy HTTP+SSE transport and the shouldFallbackToSse ladder | `13c-mcp-servers.md` | n/a |
| `MCP-111` | n/a | n/a | `cut` | Unix-domain-socket transport | `13c-mcp-servers.md` | n/a |
| `MCP-112` | n/a | S | `rmcp` | MCP NDJSON framing | `13c-mcp-servers.md` | done |
| `MCP-113` | medium | S | `hand-written` | Transport selection and mutual exclusion | `13c-mcp-servers.md` | done |
| `MCP-114` | high | M | `extension-owned` | HTTP header, bearer and command-secret resolution | `13c-mcp-servers.md` | **partial** |
| `MCP-115` | high | M | `hand-written` | Implicit-vs-explicit OAuth provider state machine and the attempt loop | `13c-mcp-servers.md` | **partial** |
| `MCP-115a` | high | S | `hand-written` | Wire the per-request header command into `connectHttpClient` **(v2.26.1 retarget, 2026-08-20)** | `13c-mcp-servers.md` | **MISSING** |
| `MCP-116` | high | S | `hand-written` | needs-auth connection state and one-shot credential invalidation | `13c-mcp-servers.md` | **MISSING** |
| `MCP-117` | medium | S | `rmcp` | Protocol-revision negotiation | `13c-mcp-servers.md` | done |
| `MCP-118` | medium | S | `rmcp` | Client capability advertisement (sampling / elicitation form+url) | `13c-mcp-servers.md` | done |
| `MCP-119` | high | M | `rmcp` | Paginated discovery with capability gating and per-list failure policy | `13c-mcp-servers.md` | **MISSING** |
| `MCP-120` | medium | S | `rmcp` | list_changed refresh with identity guards | `13c-mcp-servers.md` | **partial** |
| `MCP-121` | n/a | n/a | `cut` | Adapter-private UI stream-patch notification handler | `13c-mcp-servers.md` | n/a |
| `MCP-122` | medium | S | `hand-written` | URL-elicitation acceptance tracking and completion notice | `13c-mcp-servers.md` | **partial** |
| `MCP-123` | medium | S | `rmcp` | Connect-time abort and once-only transport cleanup | `13c-mcp-servers.md` | **partial** |
| `MCP-124` | high | S | `hand-written` | Error taxonomy and containsCleanupFailure | `13c-mcp-servers.md` | **partial** |
| `MCP-125` | high | S | `hand-written` | reconnect: guards, single-flight, identity, in-flight preservation | `13c-mcp-servers.md` | **MISSING** |
| `MCP-126` | high | M | `hand-written` | close / closeAll: generations, attempt aborts, late-name sweep | `13c-mcp-servers.md` | **MISSING** |
| `MCP-127` | medium | S | `hand-written` | Idle and in-flight accounting | `13c-mcp-servers.md` | **MISSING** |
| `MCP-128` | medium | S | `rmcp` | Request options: timeout normalisation and owned signal | `13c-mcp-servers.md` | **partial** |
| `MCP-129` | medium | S | `rmcp` | getPrompt / readResource accounting and disabled re-check | `13c-mcp-servers.md` | **MISSING** |
| `MCP-130` | medium | S | `hand-written` | Startup connect concurrency limit | `13c-mcp-servers.md` | **MISSING** |
| `MCP-131` | high | S | `rmcp` | Child-process cleanup and orphan avoidance | `13c-mcp-servers.md` | **partial** |
| `MCP-132` | medium | M | `extension-owned` | MCP endpoint probe (three-strategy ladder) | `13c-mcp-servers.md` | **MISSING** |
| `MCP-133` | medium | S | `hand-written` | Probe-enriched HTTP connect failures | `13c-mcp-servers.md` | **MISSING** |
| `MCP-134` | high | S | `rmcp` | isTerminatedSession predicate | `13c-mcp-servers.md` | **MISSING** |
| `MCP-135` | high | M | `hand-written` | withSessionRecovery retry wrapper | `13c-mcp-servers.md` | **MISSING** |
| `MCP-136` | n/a | S | `hand-written` | Tracker: what survives a restart | `13c-mcp-servers.md` | n/a |
| `MCP-137` | medium | S | `hand-written` | Status snapshot construction | `13c-mcp-servers.md` | **MISSING** |
| `MCP-138` | low | S | `extension-owned` | Publish the status snapshot | `13c-mcp-servers.md` | **partial** |
| `MCP-139` | high | M | `hand-written` | Metadata cache: path, schema, version, load and merge-save | `13c-mcp-servers.md` | **partial** |
| `MCP-140` | high | M | `hand-written` | Metadata cache: serialisers and reconstructors | `13c-mcp-servers.md` | **partial** |
| `MCP-141` | critical | M | `hand-written` | computeServerHash must hash all 14 fields; the in-tree reader hashes 11 | `13c-mcp-servers.md` | **partial** |
| `MCP-142` | critical | S | `hand-written` | stableStringify emits the bare token `undefined`, not `null` | `13c-mcp-servers.md` | done |
| `MCP-143` | high | S | `hand-written` | interpolateEnvVars is missing its third pattern {env:NAME} | `13c-mcp-servers.md` | **partial** |
| `MCP-144` | high | S | `hand-written` | !/!! secret-expression semantics in hashed values | `13c-mcp-servers.md` | **partial** |
| `MCP-145` | high | S | `hand-written` | isServerCacheValid including the throw-to-false rule | `13c-mcp-servers.md` | **partial** |
| `MCP-146` | critical | S | `hand-written` | Resource tool naming: read_ upstream vs get_ in the in-tree reader | `13c-mcp-servers.md` | done |
| `MCP-147` | medium | S | `hand-written` | Direct-tool selector parsing and the missing-server gate | `13c-mcp-servers.md` | done |
| `MCP-148` | n/a | n/a | `rmcp` | The protocol layer is rmcp, client-only | `13c-mcp-servers.md` | done |
| `MCP-149` | n/a | S | `hand-written` | Tracker: section 03 index and cross-section edges | `13c-mcp-servers.md` | n/a |
| `MCP-151` | high | M | `host-verb` | Register the `mcp` tool with the exact JSON Schema | `13d-mcp-proxy-modes.md` | done |
| `MCP-152` | high | M | `hand-written` | Port `buildProxyDescription` and re-register on change | `13d-mcp-proxy-modes.md` | done |
| `MCP-153` | high | M | `hand-written` | Port mode dispatch: precedence, args coercion, init gate | `13d-mcp-proxy-modes.md` | done |
| `MCP-154` | medium | S | `hand-written` | Port `executeStatus` | `13d-mcp-proxy-modes.md` | done |
| `MCP-155` | medium | S | `hand-written` | Port `executeList` | `13d-mcp-proxy-modes.md` | done |
| `MCP-156` | low | S | `hand-written` | Port `executeInstructions` | `13d-mcp-proxy-modes.md` | done |
| `MCP-157` | medium | M | `hand-written` | Port `executeDescribe` | `13d-mcp-proxy-modes.md` | done |
| `MCP-158` | high | M | `hand-written` | Port `executeSearch` match selection | `13d-mcp-proxy-modes.md` | done |
| `MCP-159` | medium | S | `hand-written` | Port the regex search path onto a linear-time engine | `13d-mcp-proxy-modes.md` | done |
| `MCP-160` | medium | M | `hand-written` | Port `executeSearch` rendering, pagination footer and connecting hint | `13d-mcp-proxy-modes.md` | done |
| `MCP-161` | high | M | `hand-written` | Port `executeConnect` | `13d-mcp-proxy-modes.md` | done |
| `MCP-162` | high | M | `hand-written` | Port `attemptAutoAuth` and the single-shot latch | `13d-mcp-proxy-modes.md` | done |
| `MCP-163` | critical | L | `hand-written` | Port `executeCall`'s resolution state machine (phases 1-5) | `13d-mcp-proxy-modes.md` | done |
| `MCP-164` | high | L | `hand-written` | Port `executeCall`'s invocation paths and result shaping | `13d-mcp-proxy-modes.md` | **partial** |
| `MCP-165` | medium | M | `hand-written` | Port `executeCall`'s error taxonomy | `13d-mcp-proxy-modes.md` | done |
| `MCP-167` | medium | M | `hand-written` | Port `executeAuthStart` and `formatManualAuthInstructions` | `13d-mcp-proxy-modes.md` | done |
| `MCP-168` | medium | S | `hand-written` | Port `executeAuthComplete` | `13d-mcp-proxy-modes.md` | done |
| `MCP-169` | high | S | `hand-written` | Freeze the `details.error` vocabulary as a conformance table | `13d-mcp-proxy-modes.md` | done |
| `MCP-170` | high | S | `extension-owned` | Use insertion-ordered maps for servers and metadata | `13d-mcp-proxy-modes.md` | done |
| `MCP-171` | low | M | `open-decision` | Decide the `localeCompare` tie-break | `13d-mcp-proxy-modes.md` | done |
| `MCP-172` | high | S | `hand-written` | Port `normalizeSearchText` and `tokenize` | `13d-mcp-proxy-modes.md` | done |
| `MCP-173` | high | M | `hand-written` | Port `scoreToolMatch` field scoring | `13d-mcp-proxy-modes.md` | done |
| `MCP-174` | medium | M | `hand-written` | Port keyword scoring and `resolveSearchKeywords` | `13d-mcp-proxy-modes.md` | **partial** |
| `MCP-175` | high | S | `hand-written` | Port the coverage gate and final bonuses | `13d-mcp-proxy-modes.md` | done |
| `MCP-176` | high | S | `hand-written` | Port `rankToolMatches` and `paginate` | `13d-mcp-proxy-modes.md` | done |
| `MCP-177` | low | S | `hand-written` | Port keyword resolution inside the regex search path | `13d-mcp-proxy-modes.md` | done |
| `MCP-178` | high | M | `open-decision` | Port `rankSuggestions`, and settle the `getServerPrefix` conflict | `13d-mcp-proxy-modes.md` | done |
| `MCP-191` | high | M | `open-decision` | `auth-start` / `auth-complete` derive no distinct permission targets | `13d-mcp-proxy-modes.md` | **partial** |
| `MCP-192` | medium | S | `host-verb` | Satisfy the permission system's contracts on the `mcp` tool | `13d-mcp-proxy-modes.md` | done |
| `MCP-193` | medium | M | `host-addition` | Reach `register_late_tool` from a native extension | `13d-mcp-proxy-modes.md` | **MISSING** |
| `MCP-194` | low | S | `open-decision` | Tool-schema property order is alphabetised by `serde_json` | `13d-mcp-proxy-modes.md` | done |
| `MCP-195` | medium | S | `hand-written` | Port the ranking conformance suite (11 cases) | `13d-mcp-proxy-modes.md` | done |
| `MCP-196` | high | L | `hand-written` | Port the proxy-mode conformance suites (47 cases) | `13d-mcp-proxy-modes.md` | **partial** |
| `MCP-197` | medium | S | `host-verb` | Port the render binding, including the `toolResultRendering` fork | `13d-mcp-proxy-modes.md` | done |
| `MCP-198` | medium | M | `hand-written` | Port the cross-server candidate-collision set behind the description's counts | `13d-mcp-proxy-modes.md` | done |
| `MCP-199` | low | S | `host-verb` | Wire native-tool detection to `all_tool_names` | `13d-mcp-proxy-modes.md` | done |
| `MCP-200` | high | M | `hand-written` | The four-mode server-prefix / tool-name formatter | `13e-mcp-tools.md` | done |
| `MCP-201` | high | M | `hand-written` | getToolNameCandidates, including the legacy arm | `13e-mcp-tools.md` | done |
| `MCP-202` | high | M | `hand-written` | matchesToolPattern / matchesToolSelector / isToolAllowed | `13e-mcp-tools.md` | done |
| `MCP-203` | medium | S | `hand-written` | resourceNameToToolName and the read_ resource base name | `13e-mcp-tools.md` | done |
| `MCP-204` | medium | S | `hand-written` | resolveServerFromToolName with its ambiguity fail-safe | `13e-mcp-tools.md` | **MISSING** |
| `MCP-205` | high | M | `open-decision` | Reconcile mcp_direct_tools.rs with pi-mcp-adapter naming | `13e-mcp-tools.md` | n/a |
| `MCP-206` | low | S | `hand-written` | sanitizePromptName / formatPromptCommandName | `13e-mcp-tools.md` | done |
| `MCP-207` | high | L | `hand-written` | buildToolMetadata | `13e-mcp-tools.md` | **MISSING** |
| `MCP-208` | medium | S | `hand-written` | extractUiToolVisibility / isUiToolVisibleToModel (kept half) | `13e-mcp-tools.md` | **partial** |
| `MCP-209` | n/a | S | `cut` | getToolUiResourceUri / extractToolUiStreamMode and the UI spec fields | `13e-mcp-tools.md` | n/a |
| `MCP-210` | medium | S | `hand-written` | findToolByName, getToolNames, totalToolCount | `13e-mcp-tools.md` | done |
| `MCP-211` | medium | M | `hand-written` | formatSchema and its four helpers | `13e-mcp-tools.md` | **MISSING** |
| `MCP-212` | critical | L | `hand-written` | resolveDirectTools, including the builtin-collision drop | `13e-mcp-tools.md` | done |
| `MCP-213` | high | M | `hand-written` | buildProxyDescription | `13e-mcp-tools.md` | done |
| `MCP-214` | high | L | `hand-written` | The direct-tool execute state machine | `13e-mcp-tools.md` | **MISSING** |
| `MCP-214a` | high | M | `hand-written` | recoverAuthConnection and the per-server request options | `13e-mcp-tools.md` | **partial** |
| `MCP-215` | medium | M | `hand-written` | attemptDirectAutoAuth and the auth message templates | `13e-mcp-tools.md` | **partial** |
| `MCP-216` | medium | M | `host-verb` | The direct-tool registration shape | `13e-mcp-tools.md` | done |
| `MCP-217` | high | L | `host-addition` | Post-init dynamic tool (and command) registration | `13e-mcp-tools.md` | **MISSING** |
| `MCP-217a` | medium | S | `hand-written` | freezeDirectTools and the frozen-surface escape hatches | `13e-mcp-tools.md` | **partial** |
| `MCP-217b` | low | S | `host-verb` | The tool-surface refresh notification | `13e-mcp-tools.md` | **MISSING** |
| `MCP-218` | medium | S | `hand-written` | syncProxyTool's registration/deactivation predicate | `13e-mcp-tools.md` | done |
| `MCP-219` | medium | S | `hand-written` | MCP_DIRECT_TOOLS, __none__ and parseDirectToolSelectors | `13e-mcp-tools.md` | done |
| `MCP-220` | high | M | `hand-written` | transformMcpContent for every standard MCP content type | `13e-mcp-tools.md` | done |
| `MCP-221` | medium | S | `hand-written` | transformMcpResourceContents | `13e-mcp-tools.md` | done |
| `MCP-222` | high | S | `hand-written` | resolveMcpResultContent and the structured-content fallback | `13e-mcp-tools.md` | done |
| `MCP-223` | high | M | `hand-written` | Binary-resource materialization with its four limits | `13e-mcp-tools.md` | done |
| `MCP-224` | medium | M | `hand-written` | The materialized-resource cleanup drain and retry | `13e-mcp-tools.md` | **partial** |
| `MCP-225` | medium | S | `hand-written` | resolveMcpOutputGuardOptions and the MCP_OUTPUT_GUARD kill switch | `13e-mcp-tools.md` | **partial** |
| `MCP-226` | high | M | `hand-written` | guardMcpOutput's normalize / affix / passthrough path | `13e-mcp-tools.md` | done |
| `MCP-227` | high | M | `hand-written` | The truncation arithmetic and notice format | `13e-mcp-tools.md` | done |
| `MCP-228` | high | S | `hand-written` | saveArtifact's private-directory spill | `13e-mcp-tools.md` | done |
| `MCP-229` | medium | M | `hand-written` | boundMcpResult and the result-summary schema | `13e-mcp-tools.md` | done |
| `MCP-230` | medium | S | `hand-written` | Record the output guard's actual security contract | `13e-mcp-tools.md` | done |
| `MCP-231` | high | M | `hand-written` | isToolCallApprovalRequired | `13e-mcp-tools.md` | **MISSING** |
| `MCP-232` | critical | M | `host-verb` | ensureToolCallApproved and the approval dialog | `13e-mcp-tools.md` | **partial** |
| `MCP-233` | medium | S | `host-verb` | Drop the approval broker; before_tool_call is the broker | `13e-mcp-tools.md` | done |
| `MCP-234` | high | M | `open-decision` | Direct MCP tools do not reach the mcp permission category | `13e-mcp-tools.md` | n/a |
| `MCP-235` | high | S | `hand-written` | sanitizeTerminalText / stripOscSequences | `13e-mcp-tools.md` | done |
| `MCP-236` | medium | S | `hand-written` | Give the mcp tool its prompt guideline | `13e-mcp-tools.md` | done |
| `MCP-237` | medium | S | `hand-written` | The call-row formatters | `13e-mcp-tools.md` | done |
| `MCP-238` | low | S | `host-verb` | resolveMcpToolRenderOptions and the renderShell selection | `13e-mcp-tools.md` | done |
| `MCP-239` | medium | M | `hand-written` | collectCollapsedResultLines / formatMcpToolResultLines / blockToLines | `13e-mcp-tools.md` | done |
| `MCP-240` | low | S | `hand-written` | formatMcpToolResultIdentity | `13e-mcp-tools.md` | done |
| `MCP-241` | low | M | `hand-written` | The compact result row without a render width | `13e-mcp-tools.md` | done |
| `MCP-242` | low | S | `host-verb` | Expanded rendering without a per-row expansion flag | `13e-mcp-tools.md` | done |
| `MCP-243` | low | S | `hand-written` | The compact call-row suppression has no cyrup equivalent | `13e-mcp-tools.md` | done |
| `MCP-244` | low | S | `hand-written` | The renderer contract carries no theme | `13e-mcp-tools.md` | done |
| `MCP-245` | low | S | `extension-owned` | Width-aware truncation is not needed | `13e-mcp-tools.md` | n/a |
| `MCP-246` | low | S | `extension-owned` | Route the five collision/advisory warnings | `13e-mcp-tools.md` | done |
| `MCP-247` | high | S | `hand-written` | The mcp proxy tool's parameter schema | `13e-mcp-tools.md` | done |
| `MCP-248` | n/a | S | `hand-written` | Tracker: registration, approval, guard and rendering | `13e-mcp-tools.md` | n/a |
| `MCP-249` | high | S | `hand-written` | Freeze the details schema this subsystem emits | `13e-mcp-tools.md` | **partial** |
| `MCP-250` | high | M | `hand-written` | The `AuthEntry` record and its strict normalization | `13f-mcp-credentials.md` | done |
| `MCP-251` | high | S | `hand-written` | Derive the keychain account and legacy directory from `sha256-<hex>` of the server name | `13f-mcp-credentials.md` | done |
| `MCP-252` | high | M | `extension-owned` | Add the OS keyring backend and map its error taxonomy | `13f-mcp-credentials.md` | done |
| `MCP-253` | high | M | `hand-written` | The chunking manifest write path | `13f-mcp-credentials.md` | done |
| `MCP-254` | high | S | `hand-written` | The chunked read path and the `AuthStoreError` taxonomy | `13f-mcp-credentials.md` | done |
| `MCP-255` | medium | S | `hand-written` | Stale-chunk cleanup ordering and its error-swallowing | `13f-mcp-credentials.md` | done |
| `MCP-256` | high | M | `hand-written` | The legacy plaintext import-and-delete path (and the record translator) | `13f-mcp-credentials.md` | done |
| `MCP-257` | high | M | `hand-written` | The process-lifetime auth-entry cache and its three external invalidation points | `13f-mcp-credentials.md` | done |
| `MCP-258` | medium | S | `extension-owned` | Fault-injection backends behind an explicit selector | `13f-mcp-credentials.md` | done |
| `MCP-259` | low | S | `hand-written` | Honour the auth-cache disable switch | `13f-mcp-credentials.md` | done |
| `MCP-260` | high | M | `hand-written` | Re-exec under `keyctl session -` via a hidden `__mcp-keyring-helper` subcommand | `13f-mcp-credentials.md` | **partial** |
| `MCP-261` | medium | S | `hand-written` | The helper's one-shot JSON stdio protocol | `13f-mcp-credentials.md` | done |
| `MCP-262` | medium | S | `hand-written` | The revoked-keyring cause-chain predicate | `13f-mcp-credentials.md` | done |
| `MCP-263` | low | S | `hand-written` | Emit the two credential-store-unavailable messages verbatim | `13f-mcp-credentials.md` | done |
| `MCP-264` | critical | M | `hand-written` | URL binding and the mutators' sibling-purge rule | `13f-mcp-credentials.md` | done |
| `MCP-265` | high | S | `hand-written` | `inspectAuthForUrl`'s three-state status and its fail-open/fail-closed split | `13f-mcp-credentials.md` | done |
| `MCP-266` | medium | S | `hand-written` | The accessor surface section 07 consumes | `13f-mcp-credentials.md` | done |
| `MCP-267` | medium | S | `rmcp` | Expiry arithmetic | `13f-mcp-credentials.md` | done |
| `MCP-268` | high | M | `hand-written` | Serialize read-modify-write per server | `13f-mcp-credentials.md` | done |
| `MCP-269` | medium | S | `hand-written` | MCP credentials never reach `auth.json` | `13f-mcp-credentials.md` | **partial** |
| `MCP-270` | low | S | `extension-owned` | The embedder facade (`oauth.ts`) | `13f-mcp-credentials.md` | done |
| `MCP-271` | n/a | S | `rmcp` | The MCP-SDK `OAuthTokens` conversion | `13f-mcp-credentials.md` | done |
| `MCP-272` | n/a | S | `cut` | `ConsentManager` | `13f-mcp-credentials.md` | n/a |
| `MCP-273` | n/a | S | `cut` | `ConsentError` | `13f-mcp-credentials.md` | n/a |
| `MCP-274` | n/a | S | `cut` | Consent state is process-scoped and must not be persisted | `13f-mcp-credentials.md` | n/a |
| `MCP-275` | medium | S | `hand-written` | Compact JSON serialization | `13f-mcp-credentials.md` | done |
| `MCP-276` | n/a | S | `extension-owned` | The non-string server-name guards do not port | `13f-mcp-credentials.md` | n/a |
| `MCP-277` | critical | S | `hand-written` | Prove the absence of secret leakage through `Debug`, logs and errors | `13f-mcp-credentials.md` | done |
| `MCP-278` | medium | M | `hand-written` | The storage acceptance suite (17 tests) | `13f-mcp-credentials.md` | **partial** |
| `MCP-280` | high | S | `hand-written` | The keychain service name, and what happens to a co-installed pi-mcp-adapter | `13f-mcp-credentials.md` | done |
| `MCP-281` | medium | M | `hand-written` | Adopt the keychain-mandatory posture | `13f-mcp-credentials.md` | done |
| `MCP-282` | low | S | `hand-written` | Env-var namespace for the surviving switches | `13f-mcp-credentials.md` | done |
| `MCP-283` | medium | M | `hand-written` | The cache acceptance suite (13 tests) | `13f-mcp-credentials.md` | **partial** |
| `MCP-284` | medium | S | `hand-written` | The parse-error wrapping asymmetry between read and remove | `13f-mcp-credentials.md` | done |
| `MCP-285` | medium | S | `hand-written` | Remove-path chunk cleanup is fatal, not best-effort | `13f-mcp-credentials.md` | done |
| `MCP-286` | low | S | `hand-written` | Bound `chunkCount` on read | `13f-mcp-credentials.md` | done |
| `MCP-287` | medium | S | `hand-written` | The subprocess timeout path and the unreachable ladder rung | `13f-mcp-credentials.md` | **partial** |
| `MCP-288` | low | S | `rmcp` | The three `expiresAt` predicates | `13f-mcp-credentials.md` | done |
| `MCP-289` | n/a | S | `extension-owned` | Create the `cyrup-mcp` crate | `13f-mcp-credentials.md` | done |
| `MCP-290` | medium | S | `hand-written` | Persist the DCR client record rmcp's `StoredCredentials` drops | `13f-mcp-credentials.md` | done |
| `MCP-291` | high | M | `hand-written` | Implement `rmcp::transport::auth::{CredentialStore, StateStore}` over the keychain | `13f-mcp-credentials.md` | done |
| `MCP-300` | n/a | S | `hand-written` | The OAuth subsystem as one shippable unit | `13g-mcp-oauth.md` | **partial** |
| `MCP-301` | high | M | `hand-written` | Flow ownership: runtime, generation counter, four maps | `13g-mcp-oauth.md` | done |
| `MCP-302` | medium | M | `hand-written` | extractOAuthConfig and its twelve validation messages | `13g-mcp-oauth.md` | done |
| `MCP-303` | medium | S | `hand-written` | parseOAuthRedirectUri's loopback-only validation | `13g-mcp-oauth.md` | done |
| `MCP-304` | high | S | `hand-written` | Callback endpoint configuration and MCP_OAUTH_CALLBACK_PORT | `13g-mcp-oauth.md` | done |
| `MCP-305` | high | M | `hand-written` | The bind / rebind / strict-port state machine | `13g-mcp-oauth.md` | done |
| `MCP-306` | critical | M | `hand-written` | The callback request handler's eight branches | `13g-mcp-oauth.md` | done |
| `MCP-307` | medium | M | `hand-written` | The three callback pages, including host branding | `13g-mcp-oauth.md` | done |
| `MCP-308` | high | M | `hand-written` | Listener lifetime: reserve, wait, cancel, stop, restart, process exit | `13g-mcp-oauth.md` | done |
| `MCP-309` | medium | S | `hand-written` | The discovery trigger: proactive probe or reactive challenge | `13g-mcp-oauth.md` | **partial** |
| `MCP-310` | n/a | S | `rmcp` | RFC 9728 protected-resource metadata discovery | `13g-mcp-oauth.md` | done |
| `MCP-311` | n/a | S | `rmcp` | RFC 8414 + OIDC discovery and the issuer echo check | `13g-mcp-oauth.md` | done |
| `MCP-312` | medium | S | `rmcp` | RFC 7591 dynamic client registration | `13g-mcp-oauth.md` | done |
| `MCP-313` | medium | S | `hand-written` | Client metadata and the host-branding defaults | `13g-mcp-oauth.md` | **partial** |
| `MCP-314` | high | S | `hand-written` | Restore the full client configuration after initialize_from_store | `13g-mcp-oauth.md` | done |
| `MCP-315` | high | M | `hand-written` | The keychain-backed CredentialStore, and the expiry arithmetic | `13g-mcp-oauth.md` | done |
| `MCP-316` | high | S | `hand-written` | authorizationParams' reserved-key guard and the no-browser-mid-turn fence | `13g-mcp-oauth.md` | done |
| `MCP-317` | n/a | S | `rmcp` | PKCE and the authorization URL | `13g-mcp-oauth.md` | done |
| `MCP-318` | high | M | `rmcp` | Token endpoint, client authentication, and the retry policy | `13g-mcp-oauth.md` | done |
| `MCP-319` | n/a | S | `rmcp` | RFC 8707 resource binding | `13g-mcp-oauth.md` | done |
| `MCP-320` | n/a | S | `rmcp` | Flow-state custody across the browser hop | `13g-mcp-oauth.md` | done |
| `MCP-321` | high | M | `hand-written` | The storage read/write surface this flow consumes | `13g-mcp-oauth.md` | done |
| `MCP-322` | low | S | `rmcp` | Issuer binding of stored credentials | `13g-mcp-oauth.md` | done |
| `MCP-323` | medium | S | `rmcp` | The RFC 9207 gate in completeAuth, including keepPendingForRetry | `13g-mcp-oauth.md` | done |
| `MCP-324` | high | M | `rmcp` | getValidToken's refresh path and its fall-through | `13g-mcp-oauth.md` | **partial** |
| `MCP-325` | medium | S | `rmcp` | The client_credentials grant | `13g-mcp-oauth.md` | done |
| `MCP-326` | high | M | `hand-written` | The manual/headless leg: parsing and the callback-versus-paste race | `13g-mcp-oauth.md` | **partial** |
| `MCP-327` | low | S | `extension-owned` | Browser launch | `13g-mcp-oauth.md` | done |
| `MCP-328` | high | L | `hand-written` | startAuth's ordering, stale-registration checks and aggregate cleanup | `13g-mcp-oauth.md` | done |
| `MCP-329` | medium | S | `hand-written` | The 5-minute abandoned-flow timer and its state guard | `13g-mcp-oauth.md` | done |
| `MCP-330` | high | M | `hand-written` | authenticate's in-flight dedup and its cleanup boundary | `13g-mcp-oauth.md` | done |
| `MCP-331` | high | M | `hand-written` | completeAuth and completeAuthFromInput | `13g-mcp-oauth.md` | done |
| `MCP-332` | medium | S | `hand-written` | supportsOAuth, getAuthStatus, removeAuth | `13g-mcp-oauth.md` | done |
| `MCP-333` | high | M | `rmcp` | The connect-path 401 classification | `13g-mcp-oauth.md` | done |
| `MCP-334` | medium | M | `host-verb` | The /mcp-auth command surface and its eleven messages | `13g-mcp-oauth.md` | **partial** |
| `MCP-335` | medium | M | `hand-written` | auth-start / auth-complete and auto-auth | `13g-mcp-oauth.md` | done |
| `MCP-336` | n/a | S | `extension-owned` | Callback-listener ownership: settled as reuse | `13g-mcp-oauth.md` | done |
| `MCP-337` | n/a | S | `rmcp` | The rmcp split: verified, settled | `13g-mcp-oauth.md` | done |
| `MCP-338` | n/a | S | `extension-owned` | Browser-open mechanism: settled on opener | `13g-mcp-oauth.md` | done |
| `MCP-339` | medium | S | `open-decision` | Bind localhost or 127.0.0.1 | `13g-mcp-oauth.md` | n/a |
| `MCP-340` | low | S | `open-decision` | The stale hardcoded client version in the discovery probe | `13g-mcp-oauth.md` | n/a |
| `MCP-341` | medium | S | `hand-written` | Ship a corrected OAuth document | `13g-mcp-oauth.md` | **MISSING** |
| `MCP-342` | medium | S | `hand-written` | A reachable, three-form interpolate_env_vars | `13g-mcp-oauth.md` | **partial** |
| `MCP-343` | n/a | S | `rmcp` | Non-unix entropy: dissolved | `13g-mcp-oauth.md` | n/a |
| `MCP-344` | medium | S | `hand-written` | The process-shared listener refcount | `13g-mcp-oauth.md` | done |
| `MCP-345` | medium | S | `hand-written` | Preserve both errors when cleanup fails | `13g-mcp-oauth.md` | done |
| `MCP-346` | low | S | `extension-owned` | The public token API | `13g-mcp-oauth.md` | done |
| `MCP-347` | n/a | L | `hand-written` | The executable spec as the acceptance suite | `13g-mcp-oauth.md` | **partial** |
| `MCP-349` | high | S | `extension-owned` | resolveCommandSecret's subprocess mechanism | `13g-mcp-oauth.md` | done |
| `MCP-350` | — | — | `tracker` | Section-08 tracker: poll-repaint replaces push-repaint; the overlay pair is the whole substrate — **excluded from every count** | `13h-mcp-tui.md` | n/a |
| `MCP-350a` | high | S | `extension-owned` | Stash the `HostServices` handle so panels and commands can reach the host — prerequisite for all of section 08 | `13h-mcp-tui.md` | done |
| `MCP-351` | high | M | `hand-written` | `McpPanel`'s construction from config plus validated cache | `13h-mcp-tui.md` | done |
| `MCP-352` | high | M | `hand-written` | `getOtherCurrentCandidates` and the include/exclude engine it feeds | `13h-mcp-tui.md` | done |
| `MCP-353` | high | M | `hand-written` | `rebuildVisibleItems`: the flattened list plus the filter state machine | `13h-mcp-tui.md` | done |
| `MCP-354` | medium | S | `hand-written` | `fuzzyScore` | `13h-mcp-tui.md` | done |
| `MCP-355` | critical | M | `hand-written` | The panel's top-level key dispatch, in order | `13h-mcp-tui.md` | done |
| `MCP-356` | medium | S | `hand-written` | The description-search modal | `13h-mcp-tui.md` | done |
| `MCP-357` | high | S | `hand-written` | The discard-confirmation modal | `13h-mcp-tui.md` | done |
| `MCP-358` | critical | S | `hand-written` | Toggling, dirty tracking and the tri-state `buildResult` | `13h-mcp-tui.md` | done |
| `MCP-359` | high | M | `hand-written` | In-panel OAuth (`authenticateServer`) on the sync overlay seam | `13h-mcp-tui.md` | done |
| `MCP-360` | high | M | `hand-written` | In-panel reconnect and `rebuildServerTools` | `13h-mcp-tui.md` | done |
| `MCP-361` | medium | S | `extension-owned` | `ctrl+y` copies a server's failure message | `13h-mcp-tui.md` | done |
| `MCP-362` | medium | S | `host-verb` | The 60 s inactivity auto-cancel | `13h-mcp-tui.md` | **partial** |
| `MCP-363` | high | M | `extension-owned` | `panel-keys.ts`: resolve the three canonical ids and `mcp.panel.save` | `13h-mcp-tui.md` | done |
| `MCP-363a` | medium | S | `open-decision` | Where the canonical select-key defaults live | `13h-mcp-tui.md` | done |
| `MCP-364` | critical | M | `hand-written` | The terminal-injection sanitizers | `13h-mcp-tui.md` | done |
| `MCP-365` | low | S | `hand-written` | `estimateTokens` and the footer statistics | `13h-mcp-tui.md` | done |
| `MCP-366` | medium | L | `hand-written` | The panel frame layout | `13h-mcp-tui.md` | done |
| `MCP-367` | medium | M | `hand-written` | The row renderers, status labels and word wrap | `13h-mcp-tui.md` | done |
| `MCP-368` | low | M | `host-addition` | Overlay geometry: the requested column counts, and the silent height clip (HA-3) | `13h-mcp-tui.md` | **partial** |
| `MCP-369` | critical | S | `host-verb` | `McpPanelResult` escaping an `open_overlay` that returns only `bool` | `13h-mcp-tui.md` | done |
| `MCP-370` | critical | M | `open-decision` | Tool/resource/prompt name formatting versus the in-tree consumer | `13h-mcp-tui.md` | **partial** |
| `MCP-371` | medium | M | `hand-written` | `McpSetupPanel`'s screen model and dynamic action list | `13h-mcp-tui.md` | done |
| `MCP-372` | medium | M | `hand-written` | The imports and paths sub-screens | `13h-mcp-tui.md` | done |
| `MCP-374` | medium | M | `hand-written` | `runAction`, the busy latch and the notice model | `13h-mcp-tui.md` | done |
| `MCP-375` | medium | M | `hand-written` | The per-action preview builders | `13h-mcp-tui.md` | done |
| `MCP-376` | medium | S | `hand-written` | `formatWritePreview` and `formatPreview` | `13h-mcp-tui.md` | done |
| `MCP-377` | low | S | `hand-written` | The compact-width action window | `13h-mcp-tui.md` | **partial** |
| `MCP-378` | low | S | `hand-written` | The two summary lines | `13h-mcp-tui.md` | done |
| `MCP-379` | medium | S | `hand-written` | `KNOWN_SERVER_PRESETS` | `13h-mcp-tui.md` | done |
| `MCP-380` | low | S | `hand-written` | The onboarding-state file | `13h-mcp-tui.md` | done |
| `MCP-381` | high | M | `hand-written` | `/mcp`: registration, the owner-fenced prologue and the eight-way switch | `13h-mcp-tui.md` | **MISSING** |
| `MCP-382` | medium | M | `host-addition` | HA-2: `/mcp`'s dynamic argument completions have no native path, no label and no consumer | `13h-mcp-tui.md` | **partial** |
| `MCP-383` | medium | S | `hand-written` | Port `showStatus` | `13h-mcp-tui.md` | **MISSING** |
| `MCP-384` | low | S | `hand-written` | Port `showTools` | `13h-mcp-tui.md` | **MISSING** |
| `MCP-385` | medium | S | `hand-written` | Port `showPrompts` | `13h-mcp-tui.md` | **MISSING** |
| `MCP-385a` | low | S | `hand-written` | `/mcp prompts` opens each group with a `{serverName}:` header row | `13h-mcp-tui.md` | **MISSING** |
| `MCP-386` | high | M | `hand-written` | Port `reconnectServer` / `reconnectServers` | `13h-mcp-tui.md` | **MISSING** |
| `MCP-387` | high | M | `hand-written` | Port `/mcp setup` and the reload-after-write flow | `13h-mcp-tui.md` | **MISSING** |
| `MCP-388` | high | S | `hand-written` | Port `logoutServer` | `13h-mcp-tui.md` | **MISSING** |
| `MCP-389` | medium | S | `hand-written` | Port `/mcp disable` and `/mcp enable` | `13h-mcp-tui.md` | **MISSING** |
| `MCP-390` | high | L | `host-verb` | Port `authenticateServer` and `/mcp-auth` | `13h-mcp-tui.md` | **partial** |
| `MCP-391` | medium | S | `host-verb` | Port `openMcpAuthPanel` | `13h-mcp-tui.md` | **partial** |
| `MCP-392` | high | M | `hand-written` | Port `buildMcpPanelCallbacks`'s connection-status derivation | `13h-mcp-tui.md` | **MISSING** |
| `MCP-393` | low | S | `hand-written` | Port the shared-config notice and its one-shot state | `13h-mcp-tui.md` | done |
| `MCP-394` | critical | M | `hand-written` | Port `openMcpPanel`'s orchestration and the direct-tools write-back | `13h-mcp-tui.md` | **partial** |
| `MCP-394a` | medium | S | `hand-written` | A change for a server with no provenance entry is silently dropped | `13h-mcp-tui.md` | done |
| `MCP-395` | high | L | `host-addition` | HA-1's command leg: MCP prompts are slash commands, and there is no late command registration | `13h-mcp-tui.md` | **partial** |
| `MCP-395a` | medium | S | `hand-written` | Cache-time prompt resolution and command naming | `13h-mcp-tui.md` | done |
| `MCP-396` | medium | S | `hand-written` | Port `parsePromptArgs`'s bash-style tokenizer | `13h-mcp-tui.md` | **MISSING** |
| `MCP-397` | medium | S | `hand-written` | Port `resolvePromptArgs` and the usage message | `13h-mcp-tui.md` | **MISSING** |
| `MCP-397a` | low | S | `hand-written` | An explicit empty named value for a declared optional argument is still sent | `13h-mcp-tui.md` | **MISSING** |
| `MCP-398` | high | M | `host-verb` | Port the prompt command handler | `13h-mcp-tui.md` | **MISSING** |
| `MCP-399` | medium | S | `hand-written` | Port `formatPromptResult` and `extractMessageText` | `13h-mcp-tui.md` | **MISSING** |
| `MCP-450` | high | M | `hand-written` | handleSamplingRequest as a pure function of an options bag | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-451` | medium | S | `hand-written` | The six unsupported-sampling-feature rejections, in order (task becomes structural) | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-452` | high | M | `extension-owned` | resolveSamplingModel candidate ordering and the sequential auth probe | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-453` | high | M | `extension-owned` | Run the nested completion via cyrup-provider directly | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-454` | medium | S | `extension-owned` | Source the candidate set from the whole configured catalogue | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-455` | critical | M | `host-verb` | The two sampling approval gates and their formatters | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-456` | medium | M | `hand-written` | convertSamplingMessage, convertAssistantResult, mapStopReason | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-457` | low | S | `rmcp` | Sampling capability advertisement and handler-before-connect | `13i-mcp-protocol-and-verification.md` | done |
| `MCP-458` | high | M | `host-verb` | Bind sampling's model and cancellation to the live runtime owner | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-459` | low | S | `hand-written` | truncateAtWord with UTF-16 length semantics | `13i-mcp-protocol-and-verification.md` | done |
| `MCP-460` | low | S | `rmcp` | Elicitation dispatch; absent/unknown mode falls to form | `13i-mcp-protocol-and-verification.md` | **partial** |
| `MCP-461` | high | M | `hand-written` | handleFormElicitation's gate, review loop and edit picker | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-462` | low | S | `rmcp` | Iterate requestedSchema.properties in document order | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-463` | medium | S | `hand-written` | collectValidField's per-field re-prompt loop | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-464` | high | M | `hand-written` | coerceAndValidateFormValues, including JS Number() semantics | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-465` | high | M | `hand-written` | Final schema assertion with format as an assertion, not an annotation | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-466` | medium | S | `hand-written` | The label-uniquifying and humanising helpers | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-467` | high | M | `hand-written` | handleUrlElicitation, including the three -32602 rejections | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-468` | medium | S | `rmcp` | Advertise elicitation {form, url?} with allowUrl == (mode == tui) | `13i-mcp-protocol-and-verification.md` | **partial** |
| `MCP-469` | medium | S | `rmcp` | The notifications/elicitation/complete dedupe and its notice | `13i-mcp-protocol-and-verification.md` | **partial** |
| `MCP-470` | medium | S | `hand-written` | handleUrlElicitationRequired for the -32042 elicitation array | `13i-mcp-protocol-and-verification.md` | **partial** |
| `MCP-471` | high | S | `host-verb` | Hold the dispatcher budget and the interaction lock across every dialog | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-472` | low | S | `rmcp` | The three URL rejections carry JSON-RPC -32602 | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-473` | medium | S | `hand-written` | The McpTraceEvent schema v1, exact key set and insertion order | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-474` | high | S | `hand-written` | redactTraceText, dead third branch and all | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-475` | low | S | `hand-written` | traceId, messageKind, messageBytes | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-476` | medium | M | `hand-written` | McpTraceWriter: latching caps, injectable fs, serialized append queue | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-477` | low | S | `open-decision` | Trace file path derivation, and .pi to .cyrup | `13i-mcp-protocol-and-verification.md` | **partial** |
| `MCP-478` | low | S | `hand-written` | isMcpTraceEnabled and the reduced transport-kind enum | `13i-mcp-protocol-and-verification.md` | **partial** |
| `MCP-479` | medium | M | `hand-written` | TracingTransport<T> over rmcp::transport::Transport | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-480` | medium | S | `hand-written` | Wire the trace writer lifecycle into the server manager | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-481` | low | S | `hand-written` | The trace settings surface (settings.trace object, per-server trace bool) | `13i-mcp-protocol-and-verification.md` | **partial** |
| `MCP-482` | n/a | S | `hand-written` | Tracker: the upstream verification surface, with the cut census | `13i-mcp-protocol-and-verification.md` | done |
| `MCP-483` | high | S | `hand-written` | Adopt the MCP conformance harness as the port's protocol gate | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-484` | high | M | `hand-written` | A hidden cyrup mcp conformance-driver subcommand | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-485` | medium | S | `hand-written` | A sequential runner with post-hoc log assertions | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-486` | medium | S | `hand-written` | Re-derive the expected-failures baseline; do not copy it | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-487` | low | S | `hand-written` | Allocate the ephemeral callback port in Rust | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-488` | n/a | S | `hand-written` | Record what conformance does not cover | `13i-mcp-protocol-and-verification.md` | done |
| `MCP-489` | medium | M | `open-decision` | The fate of the eight surviving fixture MCP servers | `13i-mcp-protocol-and-verification.md` | n/a |
| `MCP-490` | high | L | `hand-written` | Port the unit-testable share of the vitest suite | `13i-mcp-protocol-and-verification.md` | **partial** |
| `MCP-491` | medium | M | `open-decision` | A home for the MCP seam tests without breaking the 7-target cap | `13i-mcp-protocol-and-verification.md` | **partial** |
| `MCP-492` | high | M | `hand-written` | Port the node:test OAuth suite as a serialised group | `13i-mcp-protocol-and-verification.md` | **partial** |
| `MCP-493` | low | S | `hand-written` | A Cargo/manifest policy test pinning the rmcp feature set | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-494` | medium | S | `open-decision` | The CI gate's shape, including the conformance step | `13i-mcp-protocol-and-verification.md` | n/a |
| `MCP-495` | medium | S | `hand-written` | Reconcile the test-time environment contract with cyrup's isolation rules | `13i-mcp-protocol-and-verification.md` | **partial** |
| `MCP-496` | high | M | `hand-written` | Live-pty verification for the elicitation dialogs and sampling gates | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-497` | n/a | S | `cut` | Coverage tracking | `13i-mcp-protocol-and-verification.md` | n/a |
| `MCP-498` | medium | M | `hand-written` | The two child-process host harnesses | `13i-mcp-protocol-and-verification.md` | **MISSING** |
| `MCP-499` | medium | M | `open-decision` | A trace-JSONL differential harness against the TS adapter | `13i-mcp-protocol-and-verification.md` | n/a |

### Where the units live

| § | published as | subject | range | units | note |
|---|---|---|---|---:|---|
| 01 | `13a-mcp-activation.md` | Activation, lifecycle and the host seam | `MCP-001`…`MCP-049` | 51 | includes `MCP-037a` (Finding 1) and `MCP-027a` (v2.26.1 retarget) |
| 02 | `13b-mcp-config.md` | Configuration, the type model and errors | `MCP-050`…`MCP-099` | 51 | includes `MCP-069a` (v2.26.1 retarget) |
| 03 | `13c-mcp-servers.md` | Server manager, transports and the metadata cache | `MCP-100`…`MCP-149` | 51 | includes `MCP-115a` (v2.26.1 retarget) |
| 04 | `13d-mcp-proxy-modes.md` | Proxy modes and search ranking | `MCP-151`…`MCP-199` | 36 | **14 ids deleted, not cut** — see below |
| 05 | `13e-mcp-tools.md` | Tool registration, approval, output guard and rendering | `MCP-200`…`MCP-249` | 53 | includes `MCP-214a`, `MCP-217a`, `MCP-217b` |
| 06 | `13f-mcp-credentials.md` | Credential storage, keychain and consent | `MCP-250`…`MCP-291` | 41 | `MCP-279` was retired as dead scaffolding |
| 07 | `13g-mcp-oauth.md` | The OAuth 2.1 flow and the callback server | `MCP-300`…`MCP-349` | 49 | `MCP-348` was retired as dead scaffolding |
| 08 | `13h-mcp-tui.md` | The TUI panels, slash commands and prompts | `MCP-350`…`MCP-399` | 54 | includes `MCP-350a`, `MCP-363a`, `MCP-385a`, `MCP-394a`, `MCP-395a`, `MCP-397a`; `MCP-373` retired under Cut 2; `MCP-350` is a `tracker` and is excluded from the count (55 bodies, 54 counted) |
| — | — | **there is no section 09.** `MCP-400`…`MCP-449` is unallocated | — | 0 | the surface it would have held is MCP Apps, which is Cut 2 |
| 10 | `13i-mcp-protocol-and-verification.md` | Sampling, elicitation, tracing and verification | `MCP-450`…`MCP-499` | 50 | |
| | | **total** | | **436** | |

**Fourteen ids were deleted from section 04 and are not scheduled anywhere.** They are not `cut`
records and must not be re-filed as work: `MCP-150` (a tool-surface index tracker, dead scaffolding),
`MCP-166` (`executeUiMessages` — Cut 2, and the surviving nine-arm dispatch is `MCP-153`'s), and
`MCP-179`…`MCP-190` (the whole `mcpScript` worker — Cut 4). `MCP-180` in particular read "decide how
cyrup executes adapter-authored JavaScript", which is the exact ruling Cut 4 exists to eliminate; it
is deleted rather than kept as an auditable `cut` record precisely so that no phase plan can schedule
it. `MCP-373` (`glimpse-ui.ts`) was retired the same way inside section 08 and is recorded in that
file's *Out of scope*. Numbering gaps in this table are therefore **not** evidence of a deletion on
their own — this paragraph and the section files' own *Out of scope* blocks are the record.

**This is also why the v2.26.1 retarget filed `MCP-027a` / `MCP-069a` / `MCP-115a` with letter
suffixes rather than taking free ids.** Every unallocated number is either a deliberate deletion that
must never be re-filed as work (`MCP-150`, `MCP-166`, `MCP-179`…`MCP-190`, `MCP-292`…`MCP-299`) or the
reserved `MCP-400`…`MCP-449` block that would have been section 09 (MCP Apps, Cut 2). The letter
suffix is the established form for a unit discovered after its range was allocated — `MCP-037a`,
`MCP-214a`, `MCP-217a`, `MCP-217b`, `MCP-350a`, `MCP-363a` — and it keeps each new unit in the section
file that owns its subject.

---

## Coverage

### Read

**Upstream — `pi-mcp-adapter` v2.25.0**, read with `git show v2.25.0:<path>`, never from a working
tree. All 61 production files. All 107 `__tests__` files and 5 sibling `*.test.ts` files were
enumerated and censused; the ones carrying behavioural contracts the port must satisfy (the
proxy-mode suites, the ranking suite, the OAuth `node:test` suite, the storage and cache suites) were
read. `conformance/` — `driver.ts`, `run.sh`, `baseline-client.yml`, `README.md`. `README.md` and
`OAUTH.md` were read and cross-checked against the code, which produced two findings on their own:
`README` documents 22 of 23 settings keys and 25 of 28 server-entry fields (**v2.26.1: 23 of 24 and
26 of 29**), and `OAUTH.md` carries
eight divergences from the implementation.

**rmcp 3.1.2**, from the local checkout — `crates/rmcp/Cargo.toml` (the real feature graph),
`src/handler/client.rs` (the full `ClientHandler` method set), `src/service/client.rs`
(`Peer<RoleClient>`, `ClientLifecycleMode`, `serve_with_lifecycle`), `src/transport.rs` and every
client transport module, `src/model.rs` / `src/model/` (content blocks, elicitation schema,
`CreateMessageRequestParams`), `src/transport/auth.rs` (8,235 lines), `docs/OAUTH_SUPPORT.md`,
`DEPENDENCY_POLICY.md`, `examples/clients/`, `conformance/src/bin/{client,server}.rs` and
`.github/workflows/conformance.yml`. **Every claim about rmcp in this directory was checked against
that source. None came from docs.rs.**

**cyrup, branch `david/cyrup`** — `cyrup-ext`'s `facade.rs`, `native.rs`, `hooks.rs`, `event.rs`,
`bus.rs`, `host/services.rs`, `host/overlay.rs`, `host/live.rs`, `caps/proc*`;
`cyrup-session-svc`'s `factory.rs`, `builder.rs`, `session.rs`, `host_services.rs`;
`cyrup-permission-system`'s `manager.rs`, `jsonc.rs`, `gate.rs`, `stores.rs`, `config_modal.rs`;
`cyrup-ext-subagents`'s `extension.rs`, `exec/mcp_direct_tools.rs`, `tui/fleet_overlay.rs`;
`cyrup-provider`'s `auth/oauth/callback.rs`, `auth/store.rs`, `catalog`; `cyrup-config`'s dirs and
`auth`; `cyrup-tools`'s `truncate` and `output`; `cyrup-tui`'s `overlay.rs` and `autocomplete`;
`crates/cyrup/src/main.rs`, `intercom_broker_cmd.rs`, `subagent_runner_cmd.rs`; the workspace
`Cargo.toml` and `Cargo.lock`.

**Also read for the naming reconciliation:** `pi-subagents`' own MCP allowlist file, which is where
`mcp_direct_tools.rs`'s drifted grammar comes from.

### Excluded — one reason per entry

| excluded | reason |
|---|---|
| `ui-server.ts`, `ui-session.ts`, `host-html-template.ts`, `ui-resource-handler.ts`, `ui-stream-types.ts`, `ui-app-bridge-helpers.ts`, `app-bridge.bundle.js`, `glimpse-ui.ts`, `consent-manager.ts` | **Cut 2**, owner decision. Read far enough to rule on each and to find the seams, then excluded |
| `unix-socket-transport.ts` | **Cut 3**, owner decision. rmcp does not ship the shape |
| `mcp-code.ts`, `mcp-script-worker.mjs`, `skills/mcp-scripting/SKILL.md` | **Cut 4**, owner decision |
| `examples/interactive-visualizer/**` | an MCP Apps example server; goes with Cut 2 |
| `package-lock.json`, `.github/`, `tsconfig.json`, `vitest.config.ts`, `.npmignore`, `banner.png`, `pi-mcp.mp4` | packaging and CI metadata with no behaviour to port. `package.json`'s dependency block **was** read — it is the source of the dependency table |
| `CHANGELOG.md` | narrative; every claim it makes was checked against code instead |
| pi core itself | out of area. Where the adapter calls a pi host API, the API's *contract* was taken from the adapter's call site, not from pi's implementation |
| rmcp's server role, `transport-streamable-http-server*`, `tower`, `request-state`, `macros`, `schemars`, `auth-client-credentials-jwt`, `local`, `which-command` | the adapter never runs an MCP server, and each of the rest was checked and found unneeded — recorded so the feature list is auditable rather than inherited |
| `roots`, MCP `logging`, MCP `completions`, resource subscriptions | **read, nothing to port.** rmcp ships all four; upstream implements none. Wiring them is new functionality, outside the 1:1 mandate |

### Blind spots

**1 · This is a static analysis. Nothing was built, run, tested or reproduced.** No `cargo check`, no
binary, no live terminal, no MCP server contacted. Every `verify` line in every section file is a
**design, not an observation**. The directory's own measurement of what that costs is not reassuring:
of seventeen items driven through a real pty in an earlier pass, sixteen were confirmed to exist and
only **three survived unchanged** — the recurring failure being that an item recovers what the code
does and not what the user sees. Treat every unit's *mechanism* as a hypothesis even where its
*existence* is well evidenced.

**2 · The TUI panels are specified on paper and have never been drawn.** `13h-mcp-tui.md` covers
`mcp-panel.ts` (1,015), `mcp-setup-panel.ts` (666), `panel-keys.ts` (53), `commands.ts` (627) and
`prompts.ts` (353) in 53 units to reimplementation depth — construction order, `rebuildVisibleItems`,
`fuzzyScore`, the 14-row key table, both modals, both row renderers, both distinct `wrapText`s, the
13-row setup action table, five presets, nine previews. That closes the *coverage* hole. It does not
close the *evidence* hole, and this is the one place in the port where the two come apart furthest:
**a panel specification is a hypothesis about pixels.** Seven units say so in their own `verify`
lines, and blind spot 1 applies to them with full force. Any effort estimate for section 08 derived
from the unit table is a floor.

**3 · An item-driven analysis cannot see behaviour nobody wrote an item for.** Every pass starts from
a list and asks "is this item real?"; a function with no item is invisible to all of them. The
surface-driven sweeps that were run here were **file-driven** (walk every upstream file) and
**symbol-driven** for the cut census and the env-var census. The sweeps **not** run: a per-`_meta`-key
sweep, a per-error-string sweep across the whole package (done for the details vocabulary and the
OAuth messages only), and a sweep of what the MCP *specification* requires that the adapter itself
omits. Treat 433 as a floor. **Finding 1 is the worked example of what this blind spot costs**: every
function in that chain had an item, every item was checked and confirmed, and the defect lived in the
composition between them — which no item described.

**3a · Closed.** `MCP-350` (the section tracker, carrying the poll-repaint-replaces-push-repaint
decision) and `MCP-350a` (the `set_host_services` stash both halves of section 08 depend on) were
referenced in that file's prose without being defined, because the section was written in two halves
and each assumed the other owned them. Both now have bodies in `13h-mcp-tui.md`. `MCP-350a` counts;
`MCP-350` is a tracker and is excluded from every count, per this directory's convention.

**4 · Two rmcp behaviours are asserted from reading, not from running.** The `Auto`-negotiation
failure against a discover-intolerant stdio child (`MCP-117`) is inferred from
`serve_client_with_lifecycle`'s control flow and rmcp's own doc comment, not observed. And
`TokioChildProcess::graceful_shutdown`'s single-pid, no-SIGTERM teardown is read from the source;
whether it orphans a grandchild in practice depends on what the server itself spawns.

**5 · `keyring` 4.x was got wrong once already, in the safe-looking direction.** An earlier pass
"corrected" the version down and rewrote the feature names from a stale local index snapshot; the
correction was itself the error. The current figures come from the live registry and the published
crate source. The residual: **which Linux backend gets linked is still an open ruling**, and it
determines whether four units are live code or dead code.

**6 · The cache golden vectors do not exist yet.** The five divergences between the writer contract
and the in-tree reader were derived by reading both sides. They are stated as byte-level claims about
a digest, and **nobody has computed a digest on either side.** The fixture that would settle them is
`MCP-141`'s own verify line.

**7 · The panels aside, no upstream test was executed.** The conformance suite was read, not run,
including against rmcp's own driver. The claim that the harness can be pointed at a Rust binary is
**demonstrated** by rmcp's checked-in workflow rather than inferred — but cyrup's driver does not
exist, so nothing has been run end to end.

### Corrections to the first edition

| # | first edition | this edition |
|---|---|---|
| 1 | Line-anchored: cyrup cited as `file.rs:1234`, upstream as `file.ts:NNN`, with commit shas and "resolves at" provenance | **Zero line numbers, zero shas, zero working-tree provenance.** cyrup by symbol and file, upstream by file and symbol. The first edition measured **37% of its cyrup line citations sitting on already-drifted files within a day**; roughly **5,900 brittle anchors** were removed across the nine section files |
| 2 | **"There is no way for a native extension to register a tool after init"**, rated `critical` | **Overstated — but the first correction to it understated the gap, and Finding 1 is the accurate version.** Everything downstream of `refresh_tools` is complete and live and reaches the agent at every turn boundary. What is missing is the handle **and** one line inside `refresh_tools`, which in the default `wasm-host` build reports "nothing changed" for a native late registration and destroys the dirty flag. Re-filed as HA-1 across `MCP-037` and `MCP-037a`, sized `M` across two crates |
| 3 | Dozens of "cyrup-side prerequisites", several `critical` | **Three host additions across ten units, all small; all three now have owning units.** Roughly forty claimed prerequisites dissolved by naming the API that already serves them — the load-bearing ones listed below |
| 4 | ~20% of items rated `critical` | **21 of 433 = 4.8%**, each meeting one of the four clauses. Every prerequisite-shaped `critical` was demoted with its blocking-ness moved into the unit body; the rate rose only because the restored TUI section contributes six and Finding 1 contributes one |
| 5 | Four scope questions left open | **Four cuts, decided, with reasons and seams** — and the two consequences that follow: **no hand-written protocol code**, and **no JavaScript engine anywhere** |
| 6 | rmcp treated as an open adoption decision blocked on a spike | **Settled and verified against the checkout**, feature by feature, with three corrections *against* the SDK (no client-side schema validation; `initialize_from_store` drops the client secret; the fixed DCR body) and several *for* it (elicitation needs no feature; `property_order` exists; `LegacyForm` handles absent `mode`; `auth_challenge()` replaces the 401 classifier; `SessionExpired` replaces half of `isTerminatedSession`) |

**The prerequisites that dissolved, and what actually serves them** — recorded so nobody re-files
them:

| claimed prerequisite | what serves it |
|---|---|
| a nested-completion verb on `HostServices` for sampling (rated critical) | upstream imports `complete` from `pi-ai/compat` **directly**, bypassing pi's host API. `cyrup-mcp` linking `cyrup-provider` is the faithful port; a host verb would be the *divergence* |
| a bus-publish verb for `MCP_STATUS_EVENT` | **no consumer exists in cyrup.** Keep the snapshot in-crate on a `tokio::sync::watch`; building the route would be a dead primitive |
| a value-returning host request channel for the tool-approval broker's `claim()` callback | `ExtHooks::before_tool_call` + `EventKind::ToolCall::fails_closed()` + `create_mcp_permission_targets`. **That is the broker**, already wired |
| extension flag read-back for `--mcp-config` | upstream reads `process.argv` in `getConfigPathFromArgv`; `registerFlag` is only for `--help`. `std::env::args()` is the *literal* mechanism |
| `ExtensionRegistry::unregister_tool` | `pi.unregisterTool` is an **optional** upstream API probed at runtime with a documented `setActiveTools` fallback. cyrup lands on that branch — a supported upstream configuration |
| a browser-open capability decision for the workspace | a native crate is not sandboxed; `opener::open` directly, exactly as upstream depends on npm `open` |
| a JS engine (`rquickjs` / `boa` / JS-in-WASM), rated a from-zero critical decision | **Cut 4.** Not raised anywhere |
| `axum`, or a local HTTP server | **Cut 2.** The OAuth loopback listener is `cyrup-provider`'s `TcpListener` and is a different thing |
| hand-writing the streamable-HTTP transport on `HttpCaps`, and the NDJSON framing question | `StreamableHttpClientTransport`; and `AsyncRwTransport` inside `TokioChildProcess`. `HttpCaps`/`ProcCaps` are the **WASM-guest** capability grants, not a native crate's path |
| `~/.pi/agent` semantics | the `ConfigDirs::agent_dir` field. Only the *migration* question survives |
| no `CancelToken` reaches a handler | `cyrup_core::CancelToken` **is** `tokio_util::sync::CancellationToken`, the exact type `serve_client_with_ct` takes; the facade races the handler future against the dispatch token; `is_run_cancelled` is the documented run-scoped poll |
| no theme access | cosmetic. `HostServices::theme` exists but `LiveHostServices` does not override it, so the footer goes out uncoloured through upstream's own no-theme arm |
| `regex` has no Rust ReDoS analyser | `regex`'s linear-time guarantee makes the attack structurally impossible; the residual is named above |
| promoting the JSONC parser to `cyrup-core` | `cyrup_permission_system::jsonc` is already `pub` and is already the parser that crate uses on `mcp.json`. No cycle |
| re-porting `npx-resolver.ts` | already ported. A one-line `pub` promotion, not a host concern |
| a `pi-mcp-adapter-port.md` document that `proc.rs`/`http.rs` cite as the locked WIT shape | nothing in this port modifies `ProcSpawnSpec`, so the question does not arise |
| `MCP-014` as an open question | settled by ordering: `AgentSessionRuntime::new_session_with` builds the replacement, re-running `init()`, **before** the outgoing session is disposed |

**One correction this edition makes to its own seam map:** row `A-5` and the file table list
`consent-manager.ts` as in-scope `hand-written` work. It is **cut** with Cut 2 — its only production
consumers are `ui-server.ts` and `ui-session.ts`. Its behavioural contract is nonetheless recorded in
`MCP-272`…`MCP-274` so the denied-server asymmetry and the always-mode one-shot are not lost with the
code.

**And two findings this edition contributes that no section file records.** The first is Finding 1
above — `ExtensionHost::refresh_tools` cannot see a native tier's late registration in the default
build — which is a live defect in `crates/cyrup-ext`, latent only because `register_late_tool` has no
callers yet. The second is its close relative: `LiveHostServices` never overrides
`HostServices::is_run_cancelled` or `tools_expanded`, and the WASM host bridge forwards to the same
trait defaults — so both return a plausible constant for **every tier** in production. One method body
each. Both share a failure mode worth naming: **they compile, they return a plausible value, and they
look correct.** Neither is findable by reading one function; both were found by asking what a caller
actually receives.
