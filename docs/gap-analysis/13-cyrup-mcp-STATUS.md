# 13 · cyrup-mcp — implementation status against the port plan

> Part of **[13 — cyrup-mcp](13-cyrup-mcp.md)**, whose canonical table names every port unit. That
> table records what the port must BUILD; it has never recorded what is BUILT. This file is that
> second axis, and nothing else: one row per unit, what state it is actually in, and — where it is
> not done — the specific obligation that is unmet.

## Provenance

> ### CURRENT PIN — 2026-09-24. cyrup **`ea23ca2`** · `pi-mcp-adapter` **v2.37.0** (`28049de`)
>
> | | measured at | window read this pass | window still unread |
> |---|---|---|---|
> | `pi-mcp-adapter` | **v2.37.0** (clone at `86f3e20` = `v2.37.0-2-g86f3e20`) | `v2.33.0..v2.37.0` = **173 files, +15 550 / −4 169, 57 non-merge commits** (140 files / +13 027 / −2 677 excluding `dist/` and `package-lock.json`), triaged commit by commit — see *Re-measure — 2026-09-24* below. 11 units filed (`MCP-540`–`MCP-550`), each read on both sides | ~~`v2.32.1..v2.33.0` held only as leads; `v2.33.0..v2.37.0` leads read upstream-side only; `v2.37.0..HEAD` not measured~~ — **closed by the *Second pass* below** (same day): `v2.32.1..v2.33.0` read commit by commit, every lead resolved on both sides (`MCP-551`–`MCP-582` filed, the rest struck with evidence), `v2.37.0..86f3e20` recorded as post-tag leads. Still unread: only the non-theme hunks of the two panel files in `977577f` (sampled) |
> | `cyrup` | **`ea23ca2`** | `git diff 9aeba769..ea23ca2 -- crates/cyrup-mcp` is **one line** (`owner.rs`: the `editor_has_focus` arm in `OwnedServices`' `fenced!` block, from `838eac7`). `git diff 16edcde..ea23ca2 -- crates/cyrup-mcp` — i.e. since the squash that landed the crate, which the 2026-09-04 re-audit read — is 6 files, +71 / −82: `0aefd08` (two test assertions flipped by `serde_json/preserve_order`), `dd44b3c` (the `PI_MCP_ADAPTER_*` aliases dropped), `838eac7` (the line above) | ~~The 159 open rows the 2026-09-04 sample did not draw were **not re-derived from the TypeScript** this pass either~~ — **closed by the *Third pass* below** (same day): every one of the 157 rows open in *Every unit, by section* was re-checked on both sides (94 → `implemented`, 4 `missing` → `partial`, 7 severities corrected, 3 units filed). `git log ea23ca2..HEAD -- crates` is empty, so `ea23ca2` is still the code HEAD |
>
> Numbering resumes from **`MCP-586`** (table D, *Second pass*, took `MCP-551`–`MCP-582`; table E, *Third pass*, took `MCP-583`–`MCP-585`).

> ### PROVENANCE CORRECTION — 2026-09-14. The pins below are revised; **this file was not re-read.**
>
> Everything in this file is history and is correct as written. Its 2026-08-21 census was audited
> against **`pi-mcp-adapter` v2.26.1** (`fafae21`) and its 2026-09-04 re-audit against **v2.32.1**
> (`10a4536`); both stay. **Nothing in this block re-verifies any of it: no unit was re-read, no
> obligation re-derived, no count, severity or status changed, and no row moved between
> `implemented` / `partial` / `missing` / `not-applicable`.** This block states only how stale the
> file is.
>
> | | audited at (history — do not rewrite) | current pin (authoritative, per `README.md`'s baselines table) | window this file has never measured |
> |---|---|---|---|
> | `pi-mcp-adapter` | **`v2.26.1`** (2026-08-21 census) / **`v2.32.1`** (2026-09-04 re-audit) | **`v2.33.0`** *(was v2.32.1)* | `v2.25.0..v2.33.0` = **211 files, +27 961 / −2 129**, 113 non-merge commits. Two segments of that range are measured elsewhere and are NOT this file's blind spot: `v2.25.0..v2.26.1` by [`13-cyrup-mcp.md`](13-cyrup-mcp.md)'s *Retarget* section, `v2.26.1..v2.32.1` by this file's own 2026-09-04 re-audit. **`v2.32.1..v2.33.0` = 123 files, +9 455 / −1 352, 33 non-merge commits, is measured by nobody.** That is the unmeasured window, and the census below is its lead list |
> | `cyrup` | not recorded as a sha in either pass — the 2026-09-04 re-audit is the docs commit `11b9994a`, which **does not resolve in this repository** (`git cat-file -e` fails), so even that indirection is dead | code HEAD **`b28d3ff`**; the ledger's last recorded code baseline is `824a539e` | **Not expressible**, for the reason in the middle cell. The re-audit's own narration — `crates/cyrup-mcp` at **79 942 lines / 30 modules** across six landed waves — is the only cyrup-side anchor it left. At `b28d3ff` the crate is **43 `.rs` files / 79 930 lines** under `src` |
> | `pi` · `pi-subagents` · `pi-permission-system` · `pi-intercom` · `pi-acp` · `code_puppy_core_plugins` | — | `v0.85.1` · `v0.67.0` · `v0.8.0` · `v0.13.0` · `v0.0.33` · `v0.0.50` (ported surface byte-identical across all 39 tags) | out of this area's scope |

> **Superseded in part on 2026-09-04.** See *Re-audit — 2026-09-04, at `pi-mcp-adapter`
> v2.32.1* below: the clone is now at **v2.32.1**, the census here is stale by roughly 150 units, and
> 31 rows in the unit table have been re-ruled in place.

**Audited 2026-08-21** against upstream `pi-mcp-adapter` at tag **v2.26.1** (`fafae21`), the tag the
2026-08-20 retarget adopted — not the package's drifted HEAD. Upstream at that tag is 59 production
`.ts` files / 22,312 lines, checked out from `github.com/nicobailon/pi-mcp-adapter` (note: NOT under
the `earendil-works` org that hosts pi itself). cyrup side is `crates/cyrup-mcp`, 19 modules /
45,156 lines — a figure that includes the inline `#[cfg(test)]` modules and so overstates production
code substantially.

**Method, stated plainly so the numbers can be discounted correctly.** Nine independent readers, one
per section file, each holding that section's prose spec, the Rust, and the upstream TypeScript, and
ruling on every unit in its section. Every negative ruling — every `missing` and every `partial` —
was then handed to a second, adversarial reader whose instruction was to REFUTE it by finding the
implementation, on the assumption that the first reader had grepped TypeScript spellings against
Rust that names things differently. Where the two disagreed the skeptic won. That pass overturned
**15 rulings** — the honest measure of the first pass's false-positive rate, and a reason to treat
any single row below as a lead rather than a verdict.

**What this is not.** No row here was verified by building or running anything; every ruling is a
reading of source. `cut` and `open-decision` units are reported `not-applicable` and are NOT work.


## Re-measure — 2026-09-24, at cyrup `ea23ca2` / `pi-mcp-adapter` v2.37.0

**Method.** Upstream read only as `git -C tmp/pi-mcp-adapter show v2.37.0:<path>` (and `v2.34.0`,
`v2.35.0` where a change's landing tag mattered), plus `git diff v2.33.0..v2.37.0 -- <path>` and
`git show <commit>`; never the working tree. cyrup read at `ea23ca2`. No build, no test, no clippy —
this is a reading of source, like every pass before it. A unit below was filed only where **both**
sides were opened: the upstream function at the tag, and the cyrup function that does or does not
do the same thing. Everything read on one side only is in *Leads*, labelled so.

### Open rows re-verified — none moves, by observation, not by argument

The code under every open row is **byte-identical** to what the 2026-09-14 pass read at `9aeba769`,
except one line: `git diff 9aeba769..ea23ca2 -- crates/cyrup-mcp` is the single added
`fn editor_has_focus(&self) -> bool => false;` arm in `owner.rs`'s `OwnedServices` `fenced!` block
(`838eac7`), which keeps `MCP-006`'s "every `HostServices` method is fenced" obligation met as the
trait grew (`cyrup-ext/src/host/services.rs` gained the method in the same window). No other crate an
area-13 row cites moved in a way that touches a row: `cyrup-ext`'s `native.rs` change is the
`SanctionedWaitGate` rename (subagents' dispatch budget), and `register_late_tool` /
`take_tools_dirty` (`MCP-037a`) are unchanged in `cyrup-ext/src/facade.rs`. **So no `partial` or
`missing` row can have been closed by code since its last reading**, and none is closed here.
Falsification condition: any non-test hunk in `git diff 9aeba769..<sha> -- crates/cyrup-mcp` other
than that `owner.rs` line voids this paragraph.

The 13 `TODO(MCP-NNN)` marker ids are re-counted at `ea23ca2` and are the same 13 (`MCP-082`,
`MCP-084` — the latter a historical mention in `secrets.rs`' doc, the unit is `implemented` —
`MCP-235`, `MCP-269`, `MCP-278`, `MCP-283`, `MCP-287`, `MCP-309`, `MCP-312` ×3, `MCP-341`, `MCP-347`,
`MCP-368`, `MCP-377`).

**One `implemented` row's evidence is re-ruled in place: `MCP-282`.** Both sides read. Upstream at
v2.37.0 still spells every switch `PI_MCP_ADAPTER_*` and has **added two**:
`PI_MCP_ADAPTER_BEARER_COMMAND_TTL_MS` (`bearer-command-resolver.ts:4`) and
`PI_MCP_ADAPTER_OAUTH_FILE_KEY` (the encrypted-file store, `mcp-auth.ts`). cyrup's
`credentials.rs` declares the six as single-name `&str` constants (`TEST_AUTH_STORE_ENV` …
`TEST_LINUX_KEYRING_RECOVERY_ENV`) read through `env_lookup` — the dual-read the row's evidence
describes was deleted by `dd44b3c` as a workspace-wide owner decision ("Drop every PI_* env-var
alias in favor of CYRUP_-only"). Status stays **`implemented`** against the obligation *restated*
as "each surviving switch honoured under its `CYRUP_MCP_*` name"; the `PI_*` half is a recorded,
deliberate divergence, not open work. The two new switches belong to units that do not exist in
cyrup yet (`MCP-541`'s neighbours in *Leads*), not to `MCP-282`.

### The census, re-derived (counted, 488 units)

Re-parsed from the rows of *Every unit, by section* (437) plus tables A/B (40) plus table C below
(11) — one bold status cell per row, counted by script. It supersedes the 2026-09-04 counted column,
which predates the 2026-09-14 re-rulings.

| status | 2026-09-04 (437, counted) | 2026-09-24 — the 437 | + A/B `MCP-500`–`539` | + C `MCP-540`–`550` | **2026-09-24 total (488)** |
|---|---:|---:|---:|---:|---:|
| `implemented` | 244 | **252** | 0 | 0 | **252** |
| `partial` | 82 | **75** | 0 | 0 | **75** |
| `missing` | 84 | **83** | 35 | 10 | **128** |
| `not-applicable` | 27 | **27** | 5 | 1 | **33** |

By section, the 437: 13a 17/18/15/1 · 13b 33/9/5/4 · 13c 16/14/16/5 · 13d 32/3/1/0 ·
13e 39/4/5/5 · 13f 32/5/0/4 · 13g 38/7/1/3 · 13h 35/6/13/1 · 13i 10/9/27/4
(implemented / partial / missing / n-a). **This is still a floor on done-ness and a ceiling on
nothing** *(superseded — see *Third pass*, whose census is counted, not estimated)*: 159 open rows have not been re-derived from the TypeScript since 2026-08-21, the
2026-09-04 sample says roughly four in five of those are stale in the "actually done" direction, and
no pass has yet re-read the 252 `implemented` rows against v2.33.0+ upstream — this pass found three
whose *upstream* obligation moved under them (`MCP-061`, `MCP-072`, `MCP-222`; see table C).

### The upstream window `v2.33.0..v2.37.0`

Releases 2.34.0 (2026-09-14), 2.35.0 (2026-09-20), 2.36.0 (2026-09-21), 2.37.0 (2026-09-23).
Production TypeScript (`*.ts` excluding `__tests__/`, `dist/`, `conformance/`, `examples/`,
`*.test.ts`, `*.d.ts` — a narrower filter than the 2026-09-14 census used, so its 72/84 figures do
not compare) went **73 → 81 files, 28 109 → 31 379 lines**. New production modules:
`mcp-tasks.ts`, `bearer-command-resolver.ts`, `direct-tool-surface.ts`, `secure-keyring.ts`,
`semantic-search.ts`, `jev-client.ts`, `jev-contracts.ts`, `jev-key-store.ts`,
`agent-plugin-provenance.ts`. **Deleted**: `mcp-refresh-lock.ts` and `oauth-diagnostics.ts`
(`bdb89b7`, #563, "withdraw preview transaction dependencies"; the 2.34.0 changelog: "Cross-process
OAuth transaction serialization remains unavailable until the official SDK exposes the required
support").

**Two 2026-09-14 leads are withdrawn by upstream itself** and must not be filed: the
cross-process OAuth credential-transaction lock (13f's lead on `mcp-refresh-lock.ts`) and the
`PI_MCP_OAUTH_LOG` transaction diagnostics (13g's lead on `oauth-diagnostics.ts`). Both files exist at
v2.33.0 and at no later tag (`git cat-file -e v2.37.0:<file>` fails). Their lead text in 13f/13g is
left standing because it is true at v2.33.0; this note is the correction.

**Table C — both sides read.** Kind for every row: `upstream-drift`.

| id | sev | § | verdict | status | title | upstream | cyrup at `ea23ca2` |
|---|---|---|---|---|---|---|---|
| `MCP-540` | **high** | 13b | `hand-written` | **missing** | a higher-precedence source that switches a server's transport drops the other transport's fields | `v2.37.0:config.ts:688-712` `mergeServerMaps`: an override carrying `command` deletes `url headers requestHeadersCommand caFile auth bearerToken bearerTokenEnv bearerTokenStore oauth httpTransport socket` from the inherited entry; one carrying `url` deletes `command args env cwd pluginDataDir literalEnv inheritEnv socket`. Present since **v2.27.0** (`1ead3a6`, the package-manifest commit — so this is `v2.26.1..v2.32.1` drift the 2026-09-04 triage did not catch); `bearerTokenStore` joined the `command` arm at v2.34.0 (`6f4a8f8`, #552) | `config.rs` `merge_entry` handles only the URL-change strip, and its doc comment states the opposite of upstream: *"The `command` ⇄ `url` case is deliberately not handled: upstream v2.25.0 does not handle it either, so a base `{command}` overridden by `{url}` produces a two-transport entry … and fails at connect"* with `runtime.rs` `select_transport`'s `must configure exactly one of command or url`. So a project `.mcp.json` that repoints a user-global stdio server at an HTTP URL (or the reverse) — a layering upstream supports — connects upstream and fails in cyrup. The v2.25.0 premise was true; it stopped being true at v2.27.0. Land with `MCP-500` (same function, same exhaustive destructure) |
| `MCP-541` | medium | 13b/13e | `hand-written` | **missing** | self-namespaced tool names are not double-prefixed, and a formatted name produced by more than one tool or resource is **dropped and reported** rather than first-wins | `v2.37.0:types.ts:849-859` `formatToolName` returns the sanitised name unchanged when it already starts with `${prefix}_`; `:870 resolveUniqueNameOwnership` partitions entries into unique owners and collisions; `tool-metadata.ts:128-131` pushes every colliding `originalName` into `failedTools` and keeps only the unique ones (same helper at `metadata-cache.ts:255`, `mcp-references.ts:158`, `direct-tool-surface.ts:181`). v2.35.0, `1a5df72` (#614), `9bd91f4` (#600). At v2.33.0 `tool-metadata.ts` was first-wins via `seenNames` | `registration.rs` `format_tool_name` always emits `{p}_{sanitized}` (its test `format_tool_name_replaces_dots_only` pins that); `build_tool_metadata` is the v2.33.0 first-wins shape — `seen_names.contains(&name) ⇒ continue`, the loser neither registered nor reported. **Changes the upstream obligation under `MCP-072` and `MCP-207`, both recorded `implemented`**; neither row is edited — they were right at their tag |
| `MCP-542` | medium | 13e | `hand-written` | **missing** | `structuredContent` is appended alongside ordinary content, not only used as the empty-content fallback | `v2.37.0:tool-registrar.ts:236-246 resolveMcpResultContent` — when both exist, returns `[...blocks, {type:"text", text:"structuredContent:\n" + JSON.stringify(sc, null, 2)}]`. v2.35.0, `97435aa` (#605) | `renderers.rs` `resolve_mcp_result_content` returns the transformed blocks as soon as they are non-empty; `structuredContent` is consulted only when they are empty. A server that sends a text summary **and** structured data loses the structured half before the model sees it. **Changes the upstream obligation under `MCP-222` (`implemented`)** |
| `MCP-543` | medium | 13e | `hand-written` | **missing** | a fourth approval option, "Allow server for this session", bound to the server definition's hash | `v2.37.0:tool-approval.ts:151-194` — options `["Allow once","Allow for session","Allow server for this session","Deny"]` with a scope sentence in the dialog body; the grant is keyed to `session-approvals.ts:164 getServerApprovalIdentity` (the live definition object **and** `sha256(stableStringify(definition))`), re-checked before use and dropped on mismatch; `restoreSessionApprovalState` replaces `approvedServers` wholesale (`:182`) so a dialog opened on an old branch cannot grant. v2.35.0, `4becf97` (#628) | `proxy/constants.rs` `APPROVAL_OPTIONS` is `[&str; 3]`; `proxy/approval.rs` `ensure_tool_call_approved` has no server-scope grant. Sits beside the still-`partial` `MCP-232` (definition-hash cache key) — the two share the definition-hash computation and should land together |
| `MCP-544` | medium | 13b | `hand-written` | **missing** | config writes resolve a symlinked target and preserve its file mode | `v2.37.0:config.ts:1161 writeConfigText` — `realpathSync(writePath)` then `statSync(...).mode & 0o777`, tmp written with that mode, `chmodSync`, rename onto the **real** path; tmp removed on failure. v2.35.0, `e9f9366` (#601) | `config.rs` `write_raw_config_object` renames `<path>.<pid>.tmp` onto `path` itself, and every config writer goes through it (`write_shared_server_entry`, `write_direct_tools_config`, `write_project_server_disabled_override`, `ensure_compatibility_imports`, `enable_host_config_discovery`): a symlinked `mcp.json` (a dotfiles checkout) is **replaced by a regular file** on the first such write, and a `0600` file becomes umask-default. Silent — the write reports success. Changes the upstream obligation under `MCP-061` |
| `MCP-545` | medium | 13h | `host-verb` | **missing** | `/mcp edit [project\|global]` — open the shared config in the host editor, refuse non-object JSONC, reload after save | `v2.37.0:commands.ts:53 editSharedConfig` (`ctx.ui.editor`, default body `{"mcpServers": {}}`), `config.ts:1184 writeSharedConfigText` (top-level must be an object), `index.ts:1236` / `:1357` (the completion row and the dispatch arm). v2.35.0, `e9f9366` (#601) | `commands.rs` `MCP_SUBCOMMANDS` is the eight `reconnect tools prompts setup logout disable enable status`; no `edit`. Upstream's list is eleven at v2.37.0 (`jev` is `MCP-550`; `token` is the existing `MCP-504`). Depends on `MCP-544` for its write |
| `MCP-546` | medium | 13i | `hand-written` | **missing** | the MCP Tasks extension (`io.modelcontextprotocol/tasks`, SEP-2663): poll task-handle tool results to completion, route task-time elicitation/sampling through the normal handlers, cancel the remote task on abort; per-server `tasks: false` | `v2.37.0:mcp-tasks.ts` (467 lines, new) — `:51 TASKS_EXTENSION_ID`, `:95 RawRequestChannel`, `:270 serverAdvertisesTasks`, `:311 attachTaskSession`, `:425 callToolViaTaskSession`; attached only when the connected 2026-07-28 server advertises it (`server-manager.ts:1190`), consumed from `direct-tools.ts:316` and `proxy-modes.ts:1619`; `ServerEntry.tasks` at `types.ts:509`. v2.35.0, `67b5f02` (#627) | `live.rs` `call_tool` answers `ServerResult::CreateTaskResult` with the error *"deferred the call to a task; this client does not poll `tasks/get`"*, and `sampling.rs`' `SAMPLING_TASKS_UNSUPPORTED` doc states the client never declares the extension. **Reachability**: a conforming server only returns a task handle to a client that opted in, so today's user-visible cost is "task-only tools are unusable", not a wrong result — the refusal is honest. Needs an rmcp-support check before it is sized |
| `MCP-547` | medium | 13f | `hand-written` | **missing** | keychain payloads are chunked only on Windows; reassembly verifies the chunk digest; existing chunked records are compacted on an ordinary read | `v2.37.0:mcp-auth.ts:791 shouldChunkAuthPayload` — `store.kind !== 'encrypted-file' && length > AUTH_SECRET_CHUNK_SIZE && (process.platform === 'win32' \|\| TEST_AUTH_STORE_ENV === 'sizelimited')`; `:830` digest check on reassembly. v2.34.0, `49d9121` (#565), whose message names the user cost: every chunk is a separate macOS Keychain item that prompts separately, and "Always Allow" cannot stick across writes that mint new account names | `credentials.rs` `write_secure_auth_entry_to_store` chunks whenever `payload.len() > AUTH_SECRET_CHUNK_SIZE`, on every platform; `AUTH_SECRET_CHUNK_SIZE`'s doc calls it "both the chunk width **and** the chunking threshold" — the v2.33.0 shape. No compaction path |
| `MCP-548` | low | 13b | `hand-written` | **missing** | a blank, whitespace-only or comments-only optional config layer is absent, not a load failure | `v2.37.0:config.ts:842` — `if (stripJsonComments(text, {trailingCommas:true}).trim() === "") return null;` before parsing. v2.34.0, `cfbade4` (#568) | `config.rs` `read_validated_config` hands the text straight to `parse_json_config` → `cyrup_permission_system::jsonc::parse_into`, whose `serde_json::from_str` fails on empty input, so an empty `.mcp.json` produces a `Failed to load MCP config from …` diagnostic every start. Loads the same servers; the cost is a spurious warning |
| `MCP-549` | medium | 13b | `hand-written` | **missing** | `McpSettings` 26 → **33** keys, and the one of them with a merge-time default: `settings.exposeResources` | New at v2.34.0–v2.37.0 (`types.ts:590-682`): `allowInstall` (`:593`), `ancestorConfigRoots` (`:603`), `deferWithMissingMetadata` (`:609`), `namespaceProxyTools` (`:612`), `exposeResources` (`:628`), `jev` (`:630`), `oauthCredentialStore` (`:682`). `exposeResources` is the one this unit implements: `config.ts:382 applySettingDefaults` fills every entry whose own `exposeResources` is `undefined` from the setting (entry wins). `d417fb8` (#636) | `config.rs` `McpSettings` has 23 fields (upstream 26 minus Cut-4 `scriptMode` minus `MCP-503`'s two) and none of the seven; its doc still says "23 keys upstream". `ServerEntry::expose_resources` is per-entry only. `McpSettings` is `lenient`, so a v2.37.0 config parses — the new keys are silently ignored, which for `allowInstall: false` is the wrong direction to fail in once an install action exists. The other six keys' behaviour is in *Leads* |
| `MCP-550` | n/a | 13d | `open-decision` | **not-applicable** | Jev / "System One" semantic tool search and script evaluation (`/mcp jev setup`, `settings.jev`, `SYSTEMONE_*` / `TYPESAFE_*` keys) | `jev-client.ts`, `jev-key-store.ts`, `semantic-search.ts`, `commands.ts setupJevSemanticSearch`; v2.35.0 `199b4da` (#616), v2.36.0 `39c94db` (#630), v2.37.0 `6aafee0` (#647). A third-party hosted ranking service with its own credential store | Ruled `open-decision` exactly as `MCP-529` (the Parallel Search preset) and `MCP-048` were: a vendor integration, not a parity obligation. Recorded so the next pass does not re-derive it. `mcpScript` evaluation half is Cut 4 regardless |

**Totals after this pass: 488 units** — 477 + 11. Of the 11, 10 are open work, 1 is `open-decision`.

### Leads — upstream read, cyrup side grep-only (NOT units; no id)

> **RESOLVED 2026-09-24 (second pass).** Every bullet below was read on the cyrup side and is either
> promoted into table D or struck with its evidence — see *Leads resolved* in *Second pass* below.
> The text is kept as the record of what the first pass saw.

Each names its commit. A "grep = 0" is over `crates/cyrup-mcp/src` at `ea23ca2`.

- **`settings.deferWithMissingMetadata`** (13a) — `index.ts:672`, `:1115`; `e194276` (#643). Start
  without connecting even when cached metadata is missing; those servers show no tools until the first
  `mcp` call. grep = 0.
- **`settings.ancestorConfigRoots`** (13b) — `config.ts:599 getConfiguredAncestorRoot`,
  `:633 getAncestorProjectDirs`; `f0b83bc` (#556). A bounded ancestor-directory config source, read
  **only** from user-global/explicit layers (a project file cannot widen it), realpath-contained under
  `$HOME`, deepest root wins, farthest-first loading. A new rung shape for `ConfigContext::sources()`,
  so it collides with `MCP-502` the same way. grep = 0.
- **`settings.allowInstall: false`** (13a/13h) — `index.ts:1597`; `0e88e19` (#639). Gates the
  `mcp({action:"install"})` path from the still-unfiled v2.33.0 one-URL-install lead; nothing to gate
  in cyrup until that exists.
- **`settings.namespaceProxyTools: false`** (13e) — `namespace-tools.ts:80`, `mcp-references.ts:101`;
  `9561c80` (#599). An amendment to `MCP-513` (still `missing`), not a unit.
- **`oauth.clientMetadataUrl` — Client ID Metadata Documents, SEP-991** (13g) — `mcp-auth-flow.ts:211`,
  `config.ts:1039`; `b8fbc9c` (#571). cyrup's only mention is a comment in `oauth.rs` stating the
  adapter publishes no CIMD document — true at v2.33.0, not at v2.34.0.
- **`settings.oauthCredentialStore: "encrypted-file"`** (13f) — `mcp-auth.ts:44`, `mcp-auth-flow.ts:151`;
  `e4cd1c8` (#580). AES-256-GCM files keyed by `PI_MCP_ADAPTER_OAUTH_FILE_KEY`, explicit opt-in, no
  automatic fallback; Windows error 1312 points at it. grep = 0.
- **Command-backed bearer tokens behind a TTL cache, 401 reconnect** (13f/13c) —
  `bearer-command-resolver.ts` (new, 193 lines; `PI_MCP_ADAPTER_BEARER_COMMAND_TTL_MS`); `fbed2d6`
  (#615). Not read against `secrets.rs`' command resolution.
- **OAuth reconnect paths and expired-token rejection** (13g) — `234c6b7` (#629, "preserve OAuth state
  during atomic reconnect"), `d3adc40` (#646, `getMcpOAuthTokensForUrl` no longer returns an expired
  token with no refresh token). Not read against `oauth.rs`.
- **Linux revoked-session-keyring recovery for bearer and Jev stores** (13f) — `secure-keyring.ts`
  (new), `81f6731` (#626). cyrup already carries the keyctl recovery switches for OAuth
  (`KEYRING_RECOVERY_*`); whether they cover the bearer store is moot until `MCP-501` exists.
- **Direct tools** (13e) — `direct-tool-surface.ts` (new, 244 lines) extracted from `direct-tools.ts`;
  `b835b0f` (#642) keeps `directTools: "search"` tools inactive until a search selects them even if
  another extension re-enables them (amends the unfiled v2.33.0 search-mode lead); `db08047` (#612)
  recovers stringified arguments through type arrays and unions (amends `MCP-516`/`MCP-517`);
  `89c1b07` (#602) server-returned tool errors lose the input-schema guidance.
- **Search ranking** (13d) — `12461cf` (#613) CJK and mixed-script lexical matching in
  `search-ranking.ts`. Not read against `proxy/ranking.rs`.
- **Output guard / footer** (13e/13h) — `de0dc45` (#603) oversized `structuredContent` summaries
  self-identify as omitted; `7248ba0` (#604) footer status during deferred startup
  (`utils.ts formatMcpFooterStatus`); `60aa1c6` (#634) the large-direct-tools advisory goes through the
  UI.
- **Cut, recorded so the cuts stay deliberate** — MCP Apps sandbox navigation and CSP (`f618d4d`,
  `ui-server.ts`, `sandbox-proxy-template.ts`, `host-html-template.ts`) is **Cut 2**; `mcp-code.ts`,
  `mcp-script-worker.mjs` and the `mcp-scripting` skill gating (`33bdc38`) are **Cut 4**; `67b5f02`'s
  task-aware *MCP App* execution half is Cut 2 (its Tasks half is `MCP-546`).

### A cyrup-side fact this pass found that no row holds

`0aefd08` (`cyrup-acp`) turned `serde_json/preserve_order` on graph-wide, so `serde_json::Map` is now
an `IndexMap` in this crate. Six doc comments in `crates/cyrup-mcp/src` still state the opposite as
the premise of a design choice — `config.rs` (the `RawJson` type doc and `parse_json_config`'s
"Do not route this through `serde_json::Value`" note), `proxy/tool.rs`, `proxy/approval.rs` (`approval_argument_preview`'s
"recorded display divergence", which the flip in fact **resolves**: the preview now shows the
model's key order, as upstream's does), `renderers.rs`, and `schema.rs` (`get_or_compile`'s "canonical
key", which is now order-sensitive — a cache-efficiency cost only). The two places the order is
load-bearing for correctness — the approval cache key and the metadata-cache server hash — go through
`dirs::stable_stringify`, which sorts by construction, and are unaffected. ~~Ownerless; a doc-only fix.~~
**Filed as `MCP-582` (`tooling`, low) by the second pass**, which re-read all six at `ea23ca2`.

## Second pass — 2026-09-24, the unread windows closed (cyrup `ea23ca2`, `pi-mcp-adapter` v2.37.0)

**What this pass read.** (1) `v2.32.1..v2.33.0` in full — all 33 non-merge commits, each by
`git show <sha>` and each surviving hunk re-read at **v2.37.0** (the pin) with
`git show v2.37.0:<path>`; (2) the cyrup side of every *Leads* bullet above and of every lead in the
2026-09-14 section below and in 13a–13i's UNVERIFIED blocks; (3) `v2.37.0..HEAD` (2 untagged
commits) as post-tag leads. cyrup read at `ea23ca2` (no `crates/` commit since: `git log
ea23ca2..HEAD -- crates` is empty). No build, no test. Every row in table D had **both** functions
open. Upstream cites are `v2.37.0:<file>:<line>` unless a landing tag is named; the landing commit is
given so the row can be re-audited at the tag it entered.

**Numbering.** Table D is `MCP-551`–`MCP-582`. Numbering resumes from **`MCP-583`**.

**Status column meaning, restated for table D.** `partial` here means *the cyrup code exists and
carries the pre-change upstream shape*; `missing` means no counterpart. Area 13 keeps its open work in
tables A–D rather than in an `## Open items` table (see `README.md` §*Area 13*); table D is the
continuation of table C, not a second table of the same kind.

### Table D — both sides read

| id | sev | § | kind | status | title | upstream | cyrup at `ea23ca2` |
|---|---|---|---|---|---|---|---|
| `MCP-551` | medium | 13b | `upstream-drift` | **partial** | `--mcp-config` argv scan: equals form, last-wins, `--` terminator, `-`/`@` value guard | `utils.ts:107 getConfigPathFromArgv` — forward scan from argv[2], stops at `--`, accepts `--mcp-config=<path>`, rejects a following value starting `-`/`@` (and clears an earlier hit), later occurrence overwrites. `97253eb` (#515), v2.33.0 | `config.rs` `config_path_from_argv` takes the token after the **first** exact `--mcp-config`; its doc says "Reproduced including its limitation: `--mcp-config=path` is **not** supported and yields `None`", and the test at the `config_path_from_argv(["cyrup", "--mcp-config=/tmp/a.json"])` assertion pins that. Three callers (`extension.rs` ×2, `panel_host.rs`). `cyrup --mcp-config=~/x.json` silently loads the default ladder. Doc, code and test move together |
| `MCP-552` | medium | 13b | `upstream-drift` | **missing** | exclusive config mode (`*_MCP_CONFIG_MODE=exclusive`): one config file, nothing else | `config.ts:647 isExclusiveConfigMode`; `:472` `getConfigSources` returns only the pi-global file (**or `--mcp-config`'s target** since v2.33.0 `f7ce8d4` #498); `:212` no import discovery; `:274` host discovery forced off; `:279` no agent plugins; `:342` no package/agent-plugin merge — only the configured Claude plugins. Landed **v2.28.0** (`5088b4e`, #430), so this is `v2.26.1..v2.32.1` drift the 2026-09-04 triage did not catch | `grep -rn -i 'exclusive\|CONFIG_MODE' crates/cyrup-mcp/src` has no hit on this surface. A user who sets the switch to stop a repository's `.mcp.json` / host imports from being loaded gets them loaded anyway. Env name must follow `dd44b3c`'s single-name `CYRUP_MCP_*` rule |
| `MCP-553` | **high** | 13c | `upstream-drift` | **missing** | `ServerEntry.inheritEnv: false` — do not copy the parent environment into a stdio child | `server-manager.ts:2060 resolveEnv(env, serverName, literalEnv = false, inheritEnv = true)`, the `process.env` copy guarded by `if (inheritEnv)`; `:1076` passes `definition.inheritEnv !== false`; `types.ts:435`; `config.ts:699`/`:706` drop it on a url/socket switch. `7a7b01b` (#514), v2.33.0. The SDK's platform-default variables still apply | `secrets.rs` `resolve_env(overrides, server, literal_env, base)` always starts from `base.clone()` (the full `process_env_snapshot`), and `runtime.rs` `spawn_stdio_transport` does `env_clear()` then `envs(&spec.env)` — so the full parent environment always reaches the child. `ServerEntry` has no `inherit_env` field and is `lenient`, so the key is **silently ignored**: a server the user fenced off from `AWS_*`/tokens receives them. Rated on what it does (a stated isolation boundary not honoured), not on how often it is set. Fix must add the SDK default set (`HOME LOGNAME PATH SHELL TERM USER` on unix) when inheritance is off |
| `MCP-554` | low | 13c | `upstream-drift` | **missing** | `directToolCount` on the per-server status snapshot (last direct-surface sync, not recomputed) | `mcp-status.ts:30` `disabled ? 0 : state.directToolCounts?.get(name) ?? 0`; `types.ts` `McpServerStatusSnapshot.directToolCount`. `e32bb08` (#484), v2.33.0 | `state.rs` `McpServerStatusSnapshot` is the six-key v2.32.1 struct; `live.rs` `create_mcp_status_snapshot` has no count source. (The same struct also lacks `listenState`/`catalogStale` — owned by `MCP-508` — and the snapshot's backoff suppression — owned by `MCP-515`; not re-filed) |
| `MCP-555` | medium | 13h | `upstream-drift` | **partial** | `/mcp logout` closes the connection **before** clearing credentials | `commands.ts:443 logoutServer` — `manager.close` first; on failure `Failed to close OAuth server "X"; credentials were not cleared: …`; then `removeAuth`; on failure `Failed to clear OAuth credentials for "X": …`. Reordered at v2.33.0 (`5c1ea6b`) and kept through `bdb89b7`'s withdrawal (#563, "preserve token and logout ordering") | `commands.rs` `arm_logout` is the v2.32.1 order — `remove_auth` then `manager.close`, with the `OAuth credentials were cleared for "X", but its connection could not be closed` arm upstream deleted. A live transport can re-persist a refreshed token after the clear, so `/mcp logout` can report success and leave the user signed in |
| `MCP-556` | low | 13h | `upstream-drift` | **partial** | panel does not mark a reconnect as cached unless a cache entry was actually restored | `mcp-panel.ts:885` — `server.hasCachedData = true` moved **inside** the `if (entry)` arm. `34de8e3` (#501, issue #497), v2.33.0 | `ui.rs` reconnect-outcome arm sets `has_cached_data = true` unconditionally after the `if let Some(entry)`, and its comment says so ("Unconditional inside the connected branch — even when the cache re-read returned nothing"). The row then hides its `(not cached)` label falsely |
| `MCP-557` | low | 13i | `stale-port` | **partial** | sampling model resolution: first candidate wins; the per-candidate auth probe is deleted upstream | `sampling-handler.ts:124 resolveSamplingModel` — synchronous, returns `candidates[0]` or throws `No Pi model is available for MCP sampling`; provider auth moves to `ModelRegistry.complete`. `84e58f9` (#499), v2.33.0 | `sampling.rs` `resolve_sampling_model` still loops `options.models.get_auth(&model)` per candidate, skips unauthenticated ones and can emit the deleted `No configured auth for MCP sampling model. …` aggregate. Reverse lag against `MCP-452` (`implemented` at v2.32.1, row untouched). Consequence: cyrup samples with a *later* candidate where upstream fails on the first — different model, not an error. Needs a ruling (follow, or record a deliberate divergence) before it is worked |
| `MCP-558` | medium | 13e | `upstream-drift` | **partial** | output guard delegates to the host's truncation: host `truncateHead`, a classified notice, ten more `outputGuard` keys | `mcp-output-guard.ts:5-15` imports `DEFAULT_MAX_BYTES/LINES`, `truncateHead`, `formatSize` from pi; `:262 formatTruncationNotice` appends `First line exceeds …` / `Truncated: showing N of M lines (L line limit)` / `Truncated: N lines shown (… limit)`; details gain `truncatedBy totalLines totalBytes outputLines outputBytes lastLinePartial firstLineExceedsLimit maxLines maxBytes`. `4444b49` (#500), v2.33.0 | `renderers.rs` `guard_mcp_output` / private `truncate_head` / `format_truncation_notice` are the v2.32.1 adapter-owned shape: the notice has no reason clause and `outputGuard` carries five keys. The model-facing notice text differs on every truncation. `cyrup_tools::truncate::truncate_head` (with `truncated_by`, `last_line_partial`) is the host helper to delegate to. **Inverts `MCP-226`'s `hand-written` premise** (13e:378) — row untouched, verdict to re-rule when this lands |
| `MCP-559` | medium | 13e | `upstream-drift` | **missing** | session-branch-persisted approval grants (`mcp-approval-v1` custom entries), restored on start, resume and branch navigation | `session-approvals.ts:7` `MCP_APPROVAL_CUSTOM_TYPE`, `:120 createSessionApprovalWriter`, `:135 rememberToolApproval`, `:174 restoreSessionApprovalState` (clears and rebuilds `approvedToolCalls`, replaces `approvedServers`); `index.ts:322` restore, `:1156` `session_tree` handler. Only names and hashes persist. `928c30c` (#505), v2.33.0 | cyrup-mcp neither writes nor reads such entries (`grep -rn 'mcp-approval\|session_tree' crates/cyrup-mcp/src` = 0). The host seam **exists** — `HostServices::append_entry` / `branch()` (`cyrup-ext/src/host/services.rs`) and `HostEvent::SessionTree` — so this is `host-verb`. A resumed session re-prompts for every grant. The four-component key half is `MCP-232` (already re-ruled `partial`); the server-scope grant is `MCP-543`. Restore must skip `kind: "consent"` records (Cut 2 — see *Leads resolved*) |
| `MCP-560` | low | 13h | `stale-port` | **partial** | panels render through the host theme's semantic slots, with a plain fallback | `mcp-panel-theme.ts:86 createMcpPanelTheme(theme?)` maps `border→border`, `title/selected→accent`, `direct/confirm→success`, `needsAuth→warning`, `placeholder/hint→dim`, `description→muted`, `cancel→error`, plus bold/italic/inverse; `PLAIN_THEME` when no theme. `977577f` (#510), v2.33.0 | `ui.rs` `PanelTheme` / `SetupTheme` transcribe the deleted `DEFAULT_THEME` SGR tables (`2`, `36`, `32`, `33`, `2;3`) — the panels ignore the user's theme. `HostServices::theme()` exists. Shares the fallback obligation with `MCP-531` |
| `MCP-561` | medium | 13e | `upstream-drift` | **missing** | `directTools: "search"` — registered with real schemas, held inactive, activated additively by `mcp({ search })` | `types.ts:468`/`:610` (`boolean \| string[] \| "search"`), `:744 DirectToolSpec.lazy`; `index.ts:346 lazyDirectTools`, `:444 holdLazyToolsInactive`, `:458 activateSearchMatches`, the search arm's `Activated as direct tools: …` prefix and `details.activated`, `:1988 hasSearchModeSpecs` keeps the gateway registered even under `disableProxyTool`; `direct-tool-surface.ts:202 buildProxyDescription`'s `Search-mode servers (…)` paragraph; `b835b0f` (#642, v2.37.0) keeps them inactive even if another extension re-enables them. `c76993f` (#525), v2.33.0 | `config.rs` `direct_tools: Option<BoolOrList>` / `Option<bool>` under `lenient`: `"search"` fails the untagged enum and reads as **absent**, so a search-mode server is silently proxy-only. `proxy/description.rs` has no search paragraph. Cuts across `MCP-217`, `MCP-038` and the `disableProxyTool` decision — not a leaf |
| `MCP-562` | low | 13e | `upstream-drift` | **partial** | `mcp({ connect })` reports only **this** server's newly registered direct tools as `addedToolNames`, once each | `index.ts:1559 connectAndReport` — before/after snapshot keyed by a per-tool registration version, filtered to `registeredDirectToolServers.get(name) === serverName`, excluding lazy tools, consumed once through `reportedDirectToolNamesByServer`. `f430a9a` (#502), v2.33.0 | cyrup relies on the host wrapper (`cyrup-ext/src/wrapper.rs`, Pi `wrapRegisteredTool`) diffing the active set around the call: a concurrent connect or another server's refresh inside the window is attributed to this call, and a re-registered name (same name, new definition) is not reported. Confidence: **likely** — the attribution difference is read; whether the metadata listener's `sync_tool_surface` runs inside the wrapped call's window was not traced |
| `MCP-563` | medium | 13d | `upstream-drift` | **missing** | `mcp({ action: "install", url, server?, target? })` — one-URL install with a validating provisional connect and full rollback | `index.ts:1580 executeInstall`; `mcp-install.ts:28 normalizeMcpInstallRequest`, `:54 canonicalMcpServerUrl`; `state.provisionalInstalls` (`index.ts:1692` add, `:733` excluded from prompt commands, `init.ts:579` skips the metadata-cache write); gateway params `url`/`target` join `hasGatewayMode`; `settings.allowInstall: false` gates it (`0e88e19`, #639, v2.37.0). `fcd5d9d` (#532), v2.33.0 | No install mode (`grep '"install"\|provisional' crates/cyrup-mcp/src` = 0); `proxy/tool.rs` `has_gateway_mode` reads seven keys, not nine. The writer exists (`config.rs` `write_shared_server_entry`); the URL validator, the provisional transaction and a cross-process write serialiser (`withFileMutationQueue`, no counterpart) do not. Absorbs the 13a `provisionalInstalls` lead and the `allowInstall` lead |
| `MCP-564` | medium | 13d | `upstream-drift` | **missing** | `auth-start` opens the browser itself and a background watcher emits a turn-triggering `mcp-oauth-status` message | `proxy-modes.ts:220 emitAuthStatus` (`sendMessage({customType:"mcp-oauth-status",…},{triggerTurn:true})`), `:237 ensureBackgroundAuthWatcher`, `:253 openAuthorizationUrl: state.openBrowser`, `:323 formatManualAuthInstructions` rewritten; gateway line `Open OAuth and watch for completion`. v2.33.0 | `proxy/auth.rs` `execute_auth_start` returns the authorization URL and the copy-paste protocol only; nothing emits `mcp-oauth-status`; `proxy/description.rs` still says `Start manual OAuth and get a browser URL`. Hard prerequisite `MCP-027a` (`triggerTurn` convergence gate, `missing`) |
| `MCP-565` | medium | 13b | `upstream-drift` | **missing** | trusted local Claude plugin bundles: `claudePlugins` root key, the containment-checked loader, plugin skills via `resources_discover` | `types.ts:696`/`:710`; `config.ts:873 parseClaudePlugins`, `:654` whole-array replace in `mergeConfigs`, `:342` survives exclusive mode; `claude-plugin-loader.ts:15 loadClaudePluginBundles` (365 lines, realpath containment, `${CLAUDE_PLUGIN_ROOT}`, lowest precedence, shadowing by `formatServerNamespace`); `index.ts:1059` `resources_discover` returns `skillPaths`; `normalizeProgrammaticConfig` snapshots relative paths at factory time. `9d5db06` (#504), v2.33.0 | No counterpart (`grep -i 'claude_plugin\|CLAUDE_PLUGIN_ROOT' crates/cyrup-mcp/src` = 0; `agent_plugin.rs` is the *agent*-plugin loader, a different surface). The host event exists (`cyrup-ext/src/event.rs` `ResourcesDiscover`), so the skills half is `host-verb`. A v2.33.0 config listing plugins loads none of their servers, silently |
| `MCP-566` | medium | 13c | `upstream-drift` | **missing** | per-server `caFile`: PEM trust bundle, origin-scoped, fail-closed redirects | `http-ca.ts:9 validateCaFile` (HTTPS url only), `:21 createCaFetch` (every cert parsed, origin-scoped agent, `redirect: "error"`); `server-manager.ts:513` validates on connect/reconnect; `types.ts:440`; `URL_BOUND_AUTH_FIELDS` six with `caFile`. `f6cabbd` (#539), v2.33.0 | `grep -rn 'ca_file\|caFile\|add_root_certificate' crates/cyrup-mcp/src` = 0; the key is dropped by `lenient`, so a private-CA server fails TLS. Mechanism: reqwest `add_root_certificate` + `redirect::Policy::none()` on the rmcp HTTP client. `MCP-500` must be landed at six fields with it |
| `MCP-567` | low | 13c | `upstream-drift` | **missing** | macOS Local Network Privacy diagnosis on connect failures to literal private/link-local endpoints | `server-manager.ts:98 isLiteralLocalAddress`, `:1286 enrichHttpConnectionError` darwin arm (before the 503 arm). `2c861d7` (#544), v2.33.0 | No `enrichHttpConnectionError` at all (`MCP-507`). The SSE cause-capture half (`:1631`) is **Cut 1** and not owed. Schedule with `MCP-507`/`MCP-132` |
| `MCP-568` | medium | 13g | `upstream-drift` | **missing** | configured `headers` accompany OAuth discovery, registration, exchange and refresh — **at the MCP origin only**; protected requests fail closed on redirect | `mcp-auth-fetch.ts:52 createOAuthFetch` (`url.origin === origin ? getHeaders() : none`, SDK headers win, `redirect:"error"` when service headers were sent, errors collapsed to a value-free `TypeError`); `oauthHeaderResolver` caches per leg, never in storage. `0829964` (#535), v2.33.0 | `oauth.rs` `prepare_session` builds `AuthorizationManager::new(server_url)` with rmcp's own client; no configured header reaches any OAuth leg — a tenant-gated server cannot log in. rmcp 3.1.4 has `AuthorizationManager::with_client(reqwest::Client)`, but reqwest `default_headers` are **not** origin-scoped: a naive port leaks service headers to the discovered issuer. Reshapes `MCP-115`/`MCP-115a` |
| `MCP-569` | low | 13g | `upstream-drift` | **missing** | OAuth HTTP requests time out (30 s default, env-overridable) and honour the flow's abort | `mcp-auth-fetch.ts:38-43` `resolveOAuthRequestTimeoutMs` over `PI_MCP_OAUTH_REQUEST_TIMEOUT_MS`, combined into every OAuth fetch's signal. `6ba7d36` (#486), v2.33.0 | rmcp's default reqwest client has no request timeout; a stalled discovery, registration or token endpoint hangs that leg with no bound of its own (`oauth.rs`' `CALLBACK_TIMEOUT` covers only the wait for the browser callback). Same mechanism seam as `MCP-568` (`with_client`) |
| `MCP-570` | medium | 13g | `upstream-drift` | **missing** | `{port}` dynamic loopback redirect URIs (RFC 8252) | `mcp-auth-flow.ts:373 parseOAuthRedirectUri` — one `{port}` placeholder, authority port only, `http` + localhost/127.0.0.1/::1; `strictPort = !dynamicPort`; URI rewritten from the bound port. `cb8e316` (#483), v2.33.0 | `oauth.rs` `parse_oauth_redirect_uri` rejects anything without a numeric port (`OAuth redirectUri must include an explicit numeric port`) — the error upstream deleted — so a pre-registered `http://127.0.0.1:{port}/callback` client cannot log in. Read with `MCP-522`/`MCP-525` |
| `MCP-571` | medium | 13g | `upstream-drift` | **partial** | a redirect-URI mismatch no longer destroys refresh-capable credentials; a stale registration is invalidated after `invalid_grant` so the SDK re-registers | `mcp-auth-flow.ts:565` — clear client info **only** when `!redirectUriMatches && !tokens.refreshToken`; tokens are never cleared here; `mcp-oauth-provider.ts:265 staleRedirectClientId` → `invalidatedClientId`. `6dfeaa1` (#503), v2.33.0 | `oauth.rs` `start_auth` step 9 clears the client **and** `save_credentials(…, None)` on any mismatch, and its doc calls that "the destructive branch". Changing the callback port wipes a working refresh token before the new login has succeeded |
| `MCP-572` | low | 13a | `upstream-drift` | **missing** | `settings.deferWithMissingMetadata` — start without connecting even when cached metadata is missing | `index.ts:672`, `:1115` (prompt commands wait for live metadata). `e194276` (#643), v2.37.0 | `runtime.rs` §9 sets `bootstrap_all` when the cache file is absent and connects every enabled server at startup; no setting suppresses it (`McpSettings` lacks the key — `MCP-549`). Upstream's surrounding deferred-session path (`a462b30`, #578) is JS module-loading deferral and is **not** owed (see *Leads resolved*) |
| `MCP-573` | medium | 13b | `upstream-drift` | **missing** | `settings.ancestorConfigRoots` — bounded ancestor-directory project configs | `config.ts:599 getConfiguredAncestorRoot` (global/explicit layers only, absolute or `~/`, realpath under `$HOME` and containing cwd, deepest wins); `:633 getAncestorProjectDirs` (farthest first); new source ids `shared-project-ancestor` / `pi-project-ancestor`. `f0b83bc` (#556), v2.34.0 | `config.rs` `ConfigContext::sources` has no ancestor rung and `SourceId` no ancestor variant. A monorepo-root `.mcp.json` is not loaded from a subdirectory |
| `MCP-574` | low | 13g | `upstream-drift` | **missing** | `oauth.clientMetadataUrl` — Client ID Metadata Documents (SEP-991) | `mcp-auth-flow.ts:211-223` (string, env-interpolated, non-empty, `validateClientMetadataUrl`; `clientSecret` then requires `clientId`); `config.ts:1039` carries it through the OpenCode import. `b8fbc9c` (#571), v2.34.0 | `oauth.rs` supplies no `client_metadata_url` and says the adapter publishes none — true at v2.33.0 only. rmcp 3.1.4 exposes `with_client_metadata_url` (`transport/auth.rs`), so this is plumbing plus the validator |
| `MCP-575` | medium | 13f | `upstream-drift` | **missing** | `settings.oauthCredentialStore: "encrypted-file"` — AES-256-GCM files keyed by an env secret, explicit opt-in, no silent fallback | `mcp-auth.ts:43 OAUTH_FILE_KEY_ENV`, `:44` AAD context, `:343` key read; `types.ts:682`. `e4cd1c8` (#580), v2.34.0 | `credentials.rs` has keychain and test stores only (`grep -i 'encrypted\|FILE_KEY' credentials.rs` = 0). A host whose keychain cannot store the payload (upstream's motivating case is Windows error 1312) has no supported store |
| `MCP-576` | **high** | 13c | `upstream-drift` | **partial** | `bearerToken: "!command"` is re-resolved per request through a TTL cache, and a 401 on a keep-alive connection triggers reconnect | `bearer-command-resolver.ts:117 BearerCommandResolver` (5 min TTL, `PI_MCP_ADAPTER_BEARER_COMMAND_TTL_MS`, single-flight, last-good fallback, 10 s timeout, process-group kill); `server-manager.ts:246 createBearerCommandFetch`, `:1528` eager first resolve; `shouldReconnectAfterRefresh` treats 401 as a trigger. `fbed2d6` (#615), v2.35.0 | `secrets.rs` `resolve_http_secrets` runs the command **once per connect** and freezes the token into the transport; `lifecycle.rs` `ManagerSupervisor::should_reconnect_after_refresh` fails closed. A rotating token (the upstream report: a 24 h Cloudflare Access JWT) turns the server into permanent 401s until restart |
| `MCP-577` | medium | 13g | `upstream-drift` | **partial** | an expired access token with no refresh token is **not** returned | `mcp-auth-flow.ts:1120 getValidToken` — the fall-through returns `null` (`d3adc40`, #646, v2.37.0); tokens with no `expiresAt` still count as valid | `oauth.rs` `get_valid_token` step 4 returns the tokens "anyway" for expired-without-refresh, and documents that as the published contract. Callers send a dead token and get a 401 instead of a signed-out state |
| `MCP-578` | medium | 13c | `upstream-drift` | **partial** | reconnect builds the replacement privately and swaps it in; a failed candidate never removes the last good route | `server-manager.ts:929 doReconnect` — candidate created first, disposed if superseded (`reconnectAttempts` owner, `closeGenerations`), published only if the route is still the stale one; OAuth authority captured per attempt. `234c6b7` (#629), v2.36.0 | `server_manager.rs` `do_reconnect` is `close_inner(name)` **then** `connect_inner` — a transient failure during reconnect leaves the server with no connection |
| `MCP-579` | medium | 13d | `upstream-drift` | **partial** | proxy calls validate arguments against the advertised schema **before** approval and dispatch; server-returned errors no longer carry the schema | `proxy-modes.ts:35 proxyArgumentValidationError` (fails open on an unevaluable dialect), `:1489` → `Failed to call tool: <error>` + `Expected parameters:` suffix, `details.error: "call_failed"`; the two `tool_error` arms drop the suffix. `89c1b07` (#602), v2.36.0 | `proxy/call.rs` builds `schema_suffix` and appends it to server tool errors; no pre-dispatch validation, so an invalid call reaches the approval dialog and the server. `schema.rs` already has the validator |
| `MCP-580` | medium | 13d | `upstream-drift` | **partial** | lexical search tokenises non-ASCII runs as bounded bigrams (CJK) and restricts stem matching to ASCII | `search-ranking.ts:90 tokenize` (`SEARCH_RUN`, `MAX_UNICODE_BIGRAMS_PER_RUN = 64`, whole-run fallback), `matchesAsciiStem`. `12461cf` (#613), v2.36.0 | `proxy/ranking.rs` `tokenize` keeps only `[a-z0-9]` runs — a CJK query tokenises to nothing and matches no tool |
| `MCP-581` | low | 13e | `upstream-drift` | **partial** | the large-direct-tools advisory is a once-per-session UI warning counting eager tools only | `direct-tool-surface.ts:13 getLargeDirectToolsAdvisory` (excludes `lazy`, ends `Set settings.warnOnLargeDirectTools to false to hide this advisory.`); `index.ts:643 deliverLargeDirectToolsAdvisory` → `ctx.ui.notify(…, "warning")`. `60aa1c6` (#634), v2.36.0 | `registration.rs` emits it through `tracing::warn!` only — the user never sees it — without the trailing sentence |
| `MCP-582` | low | — | `tooling` | **missing** | six `cyrup-mcp` doc comments assert `serde_json::Map` is a `BTreeMap` | n/a (cyrup-side). `Cargo.toml` declares `serde_json = { features = ["preserve_order"] }` workspace-wide and `Cargo.lock`'s `serde_json` depends on `indexmap` | Wrong at `ea23ca2`: `config.rs` `RawJson` type doc and `parse_json_config`'s neighbouring "Do not route this through `serde_json::Value`" note; `proxy/tool.rs` `mcp_tool_schema` doc (**`MCP-194`'s "alphabetised" premise is false** — the row stays `implemented`, its evidence is stale); `proxy/approval.rs` `approval_argument_preview`'s "recorded display divergence" (now resolved: the preview keeps the model's order); `renderers.rs` `OrderedJson` doc; `schema.rs` `get_or_compile` ("canonical key" — now order-sensitive, a cache-hit cost only). `config.rs`'s `search_keywords` doc is conditional and correct. The two correctness-bearing orderings go through `dirs::stable_stringify` and are unaffected. Doc-only; owner: this area |

### One `missing` row re-ruled `implemented` — `MCP-137`

Both sides read. The row's evidence ("`createMcpStatusSnapshot` does not exist") is false at `ea23ca2`:
`live.rs` `create_mcp_status_snapshot` builds the six-key object with
`state.rs` `MCP_STATUS_SNAPSHOT_VERSION = 1`, `live.rs` `FAILURE_BACKOFF_MS` and `failure_age_seconds`
(strict `>`, rounded), and is published from `runtime.rs` twice. The recorded obligation is met.
What upstream has added since is filed elsewhere — `directToolCount` (`MCP-554`), `listenState` /
`catalogStale` (`MCP-508`), metadata suppression during backoff (`MCP-515`) — so the row closes on
its own obligation. Falsification: `grep -n 'fn create_mcp_status_snapshot' crates/cyrup-mcp/src/live.rs`
empty.

### Census after this pass (arithmetic, not a re-count)

> **Superseded by the *Third pass* census (counted, 523 units).**

Derived from *The census, re-derived* above plus table D and the `MCP-137` re-ruling; re-run the
census script before quoting it.

| status | 488 (above) | `MCP-137` | + D `MCP-551`–`582` | **2026-09-24 second pass (520)** |
|---|---:|---:|---:|---:|
| `implemented` | 252 | +1 | 0 | **253** |
| `partial` | 75 | 0 | 14 | **89** |
| `missing` | 128 | −1 | 18 | **145** |
| `not-applicable` | 33 | 0 | 0 | **33** |

### Leads resolved — every lead in this file and in 13a–13i

**Promoted** (lead → row): `--mcp-config` argv → `MCP-551`; exclusive override → `MCP-552` (the
whole mode is absent, not only the override); `inheritEnv` → `MCP-553`; `directToolCount` →
`MCP-554`; logout ordering → `MCP-555`; panel cached flag → `MCP-556`; sampling probe → `MCP-557`;
host truncation → `MCP-558`; session-branch grants (half b) → `MCP-559`; panel theme → `MCP-560`;
search mode (+ `b835b0f`, the 13h panel half, the 13b type half) → `MCP-561`; `connectAndReport` →
`MCP-562`; install (+ `provisionalInstalls`, `allowInstall`, `withFileMutationQueue`) → `MCP-563`;
auth watcher and the gateway-description lines → `MCP-564` / `MCP-561` / `MCP-563` (the description
is a pure function of those features and lands with each); Claude plugins (+ `resources_discover`,
`normalizeProgrammaticConfig`) → `MCP-565`; `caFile` / `http-ca.ts` → `MCP-566`; macOS LNP →
`MCP-567`; `mcp-auth-fetch.ts` → `MCP-568`; OAuth request timeout → `MCP-569`; `{port}` →
`MCP-570`; stale re-registration → `MCP-571`; `deferWithMissingMetadata` → `MCP-572`;
`ancestorConfigRoots` → `MCP-573`; CIMD → `MCP-574`; `encrypted-file` → `MCP-575`; bearer command
TTL → `MCP-576`; expired-token rejection → `MCP-577`; atomic reconnect (#629) → `MCP-578`; #602 →
`MCP-579`; CJK search → `MCP-580`; #634 advisory → `MCP-581`; the serde_json doc comments → `MCP-582`.

**Struck, with the evidence.**
- *Cross-process credential transactions* (13f) and *`PI_MCP_OAUTH_LOG` diagnostics* (13g) —
  **withdrawn upstream**: `bdb89b7` (#563) deletes `mcp-refresh-lock.ts`, `oauth-diagnostics.ts` and
  their tests; neither file exists at v2.34.0+. The `readAuthEntry` `cache` flag half of `6dfeaa1` is
  gone with them (`v2.37.0:mcp-auth.ts:947`/`:988` take `{ migrateLegacy }` only). `MCP-524`'s
  v2.33.0 "enlargement" is withdrawn with them; the row's own v2.32.1 obligation is unaffected.
- *Brokers consulted before session grants* (13e, `45757f5`) — **not applicable**: cyrup's broker is
  `MCP-233`'s cut, and its replacement, the `before_tool_call` gate, runs before `execute` is entered,
  hence before `proxy/approval.rs`' cache lookup, by construction. Reopen if cyrup ever adds an
  in-adapter broker.
- *`ConsentManager.restoreDecision`* (13f) — **not applicable, Cut 2 stands**: at v2.37.0 the
  consent manager's only *readers* are `ui-server.ts:361`/`:428`/`:568`; `session-approvals.ts:183`
  and `:199` only clear and write it. The cut's stated reason ("only consumers are the UI server")
  is imprecise, not wrong. Obligation carried into `MCP-559`: skip `kind:"consent"` records.
- *`outputSchema` end to end* (13e/13c) — **not applicable**: at v2.37.0 the field is written by
  `tool-metadata.ts:104` and `metadata-cache.ts:230`/`:293` and **read** only by `mcp-code.ts:346`
  (Cut 4). Reopen if any non-`mcp-code` reader appears.
- *`formatServerNamespace`* (13e), *`namespaceProxyTools`* (v2.35) and `6ee321f` (#523, the
  namespace tool's `args` description) — **already owned** by `MCP-513` (amended 2026-09-14); cyrup
  has no namespace-proxy surface to diverge.
- *Direct-tool argument recovery through type arrays/unions* (`db08047`, #612) — amends `MCP-516`
  (missing); *oversized `structuredContent` self-identifies as omitted* (`de0dc45`, #603) amends
  `MCP-516` too: `preservedFields` / `preservedCount` / `droppedCount` belong to #430's bounded
  details, which cyrup's `renderers.rs` structured summary predates entirely.
- *Linux keyring recovery for bearer and Jev stores* (`81f6731`) — moot until `MCP-501`
  (`bearerTokenStore`) exists; Jev is `MCP-550` (open-decision). Carried as an amendment to `MCP-501`.
- *Deferred session runtime* (`a462b30` #578, `7248ba0` #604) — **not applicable**: a JS
  lazy-module-loading deferral whose only observable effects are the footer text (already
  `MCP-032`'s `update_status_bar`) and the `deferWithMissingMetadata` knob (`MCP-572`). Reopen if a
  later tag makes the deferred path skip something user-visible.
- `56aa395` (Node 20.0 `AbortSignal.any`) — runtime-specific, no Rust counterpart. `8243eba`,
  `7d69db2`, `33bdc38` — `mcp-code` / scripting, **Cut 4**. `3ab9262` — refactor of in-window code.
  `a183681`, `a4b8082`, `8d68d19`, `a3f63ba`, `75e8016` — docs and lockfile.
- *Prose corrections* (13a/13c/13f: the `CYRUP_* → PI_* → default` ladder) — **confirmed** at
  `ea23ca2`: `cyrup-config/src/paths.rs` `ENV_AGENT_DIR_KEYS = ["CYRUP_AGENT_DIR",
  "CYRUP_CODING_AGENT_DIR"]` and `cyrup-provider`'s callback host reads `CYRUP_OAUTH_CALLBACK_HOST`
  only. Prose-only; a planned unit follows the single-name convention. Not a row.
- *`MCP-101` restate at four arguments* (13c) — superseded: `MCP-101` is already `implemented`
  (`secrets.rs` `resolve_env`), and `inheritEnv` is `MCP-553`.
- *`MCP-027a` as prerequisite* (13a) — stands, now recorded on `MCP-564`.
- *`MCP-522` / `MCP-525` / `MCP-531` "extended by this window"* — the extensions are `MCP-570` and
  `MCP-560`. Those three rows' own TypeScript (the v2.32.1 changelog-sourced table-B text) was **not**
  read by this pass; that is the rows' debt, not this window's.

### Post-tag leads (`pi-mcp-adapter` v2.37.0..86f3e20) — NOT rows; README cites upstream only at tags

- `8d43fd9` (#650) — OpenCode **v2** import: servers nested under `mcp.servers` (siblings `servers`
  and `timeout` excluded), `disabled: true` skipped alongside `enabled: false`, and OAuth
  `client_id` / `client_secret` / `auth_server_metadata_url` mapped to camelCase before the merge.
  cyrup `config.rs`' OpenCode extraction skips only `enabled == false` and reads servers straight off
  `mcp`. File when a tag contains it.
- `86f3e20` (#651) — schemars numeric formats (`uint64` …) registered as always-valid to silence Ajv
  warnings. **Expected not applicable**: cyrup's `jsonschema` 0.46.9 defaults to
  `ignore_unknown_formats: true` and emits no such warning; confirm when a tag contains it.

### Still unread after this pass

*(Corrected by the *Third pass*: two `v2.33.0..v2.37.0` commits this sentence covers were not in
fact triaged — `464337b` (#566) and `23c2852` (#572) — and each carries a behaviour change; they are
`MCP-583` and `MCP-585`. See *Third pass — window commits no pass had named*.)*

Nothing in `v2.32.1..v2.37.0` that touches a ported or planned surface. Explicitly not read line by
line: the non-theme hunks of `mcp-panel.ts` / `mcp-setup-panel.ts` in `977577f` (sampled; the diff is
almost wholly `fg(t.x, …)` → `this.theme.x(…)`), and `dist/`, `__tests__/`, `package-lock.json`
everywhere — no runtime effect, provenance recorded by the commit list above.

## Third pass — 2026-09-24, every open row re-checked (cyrup `ea23ca2`, `pi-mcp-adapter` v2.37.0)

**What this pass read.** Every row that was `partial` or `missing` in *Every unit, by section* when it
started — **157 rows**. That set is the 159 the 2026-09-04 sample did not draw, less the ten of them
the 2026-09-14 pass re-ruled `implemented` (`MCP-076`, `084`, `109`, `114`, `115a`, `208`, `214a`,
`224`, `225`) and `MCP-137` (second pass), plus the seven sample rows that stayed open on 2026-09-04
(`MCP-013`, `085`, `108`, `132`, `287`, `485`, `493`) and `MCP-232` (reopened 2026-09-14). **No open
row in that table was left unread.** Tables A–D (`MCP-500`–`582`) were not re-ruled: they were filed
at or after v2.32.1 and C/D were read on both sides today.

**Method.** For each row: (1) the row's recorded unmet obligation, and where the row text was
truncated the unit's body in 13a–13i; (2) the cyrup symbol that would discharge it at `ea23ca2`, and
a **production** call site for it (test-only callers do not count); (3) the upstream function at
**v2.37.0**, read from `git -C tmp/pi-mcp-adapter archive v2.37.0` (the tag, never the working tree),
with `git diff v2.32.1 v2.37.0 -- <file>` to see whether the obligation itself had moved. For
upstream files with an empty v2.32.1..v2.37.0 diff (`session-recovery.ts`, `npx-resolver.ts`,
`mcp-probe.ts`, `logger.ts`, `errors.ts`, `ts-shape.ts`, `json-schema-validator.ts`, `mcp-trace.ts`,
`abort.ts`, `failure-backoff.ts`) the function was read at v2.37.0 and the row's own v2.26.1/v2.32.1
citation was taken as still describing it. No build, no test, no clippy. **The bar is the
2026-09-04 re-audit's plus an upstream re-read**: a row moved to `implemented` means *its recorded
obligation is met at `ea23ca2` against v2.37.0's function*, not that the unit was re-derived end to
end; where upstream moved on after the obligation was written, the delta is filed or pointed at a
row that already owns it, and the closure names that row. Every re-ruled row carries a dated
*Third pass 2026-09-24* note in place, with its cyrup evidence and its upstream cite.

**Falsification.** Any row closed here reopens if the cited symbol is absent at the cited sha or has
no non-test caller: `git grep -n '<symbol>' ea23ca2 -- crates/cyrup-mcp/src` plus a caller outside
`#[cfg(test)]`.

### The result

| outcome | rows |
|---|---|
| `partial` / `missing` → **`implemented`** (94) | 13a: `MCP-008` `009` `010` `011` `015` `016` `017` `018` `020` `021` `022` `023` `024` `026` `027` `028` `029` `031` `032` `033` `036` `037` `040` `041` `042` `043` `046` `049` · 13b: `070` `073` `078` `087` `094` · 13c: `100` `101` `105` `115` `116` `122` `124` `125` `127` `128` `130` `135` `138` `140` `144` `145` · 13d: `193` · 13e: `204` `214` `217b` `231` · 13f: `260` · 13h: `362` `368` `377` `381` `383` `384` `385a` `386` `387` `388` `389` `391` `392` `395` `396` `397a` `399` · 13i: `450` `451` `453` `456` `461` `462` `463` `464` `465` `467` `468` `469` `470` `471` `473` `474` `475` `476` `477` `479` `480` `481` |
| `missing` → **`partial`** (4) | `MCP-129` (readResource accounted; no `getPrompt`), `MCP-134` (404 arm only), `MCP-454`, `MCP-458` |
| stays `partial`, obligation **restated** (8) | `MCP-014` (test-only now), `MCP-039` (frozen surface skips prompt sync), `MCP-131` (no `SIGTERM` leg), `MCP-143` (only `cyrup-ext`'s proc copy), `MCP-217a` (`/mcp` frozen line), `MCP-300` (only `MCP-347`'s suite left), `MCP-382` (label, early-config fallback), `MCP-490` (parity metric only) |
| stays `partial`, confirmed as recorded (30) | `MCP-012` `013` `068` `079` `085` `086` `089` `090` `108` `120` `123` `174` `191` `196` `215` `232` `249` `269` `278` `283` `287` `309` `313` `324` `326` `342` `347` `491` `492` `495` |
| stays `missing`, confirmed (21) | `MCP-027a` `091` `093` `098` `103` `104` `106` `107` `132` `133` `211` `341` `398` `483` `484` `485` `486` `487` `493` `496` `498` |

**Severity corrections** (plan severity, restated on user-visible consequence): `MCP-211`
medium → **high** (`mcp({describe})` shows the model the literal `(schema rendering is not wired —
MCP-211)` instead of any parameters, on every proxy-surface describe and every failed-call schema
suffix); `MCP-454` medium → **high** (sampling can see only built-in models over an empty credential
store and env keys — a user whose only credential is a stored login cannot be sampled for);
`MCP-014`, `MCP-131`, `MCP-143` high → medium; `MCP-039`, `MCP-174`, `MCP-217a`, `MCP-382` medium →
low. Each reason is in the row.

**The 2026-09-04 extrapolation, checked.** That pass predicted roughly four in five of the unread
open rows were stale in the "done" direction; the measured figure is **94 of 157 (60 %)**. The error
was in the direction it warned about but smaller than it guessed.

**Cyrup-side facts this pass found that no row holds** (doc-only, ownerless, recorded so nobody
re-derives them): `server_manager.rs`' `remember_url_elicitation` / `forget_url_elicitation` docs
still say "No production caller" (both are wired, `MCP-122`); `ui.rs:4335`'s `TODO(MCP-368,
MCP-377)` is stale (both landed); `abort.rs`' module doc is the only place that records that no
session `HostServices` implements `is_run_cancelled` — `MCP-458` now carries it.

### Table E — both sides read

Kind `upstream-drift` for all three. Numbering resumes from **`MCP-586`**.

| id | sev | § | kind | status | title | upstream | cyrup at `ea23ca2` |
|---|---|---|---|---|---|---|---|
| `MCP-583` | medium | 13a/13c | `upstream-drift` | **partial** | a failed `resources/list` is flagged and only a *failure* preserves cached resources; an empty list is authoritative; a disconnected server's tool metadata is retired; a remote close publishes a metadata update | `464337b` (#566), **v2.34.0**: `server-manager.ts` `fetchAllResources` returns `{resources, failed}` and `ServerConnection.resourceDiscoveryFailed`; `publishRemoteClose` fires the metadata listener with `remote-close` from the identity-guarded `onclose`; `init.ts:539-547` — `updateServerMetadata` deletes `toolMetadata`/`resourceCounts`/`directToolCounts` when the connection is gone or not connected; `init.ts:596-603` — `updateMetadataCache` keeps the previous resources only when `connection.resourceDiscoveryFailed === true && isServerCacheValid(existing, definition)`; the `preserveEmptyResources` option is deleted; `index.ts:593 loadToolSurfaceCache` overlays live catalogs for zero-TTL servers (matters once `MCP-505` lands) | `runtime.rs` `discover` swallows a failed `resources/list` to `Vec::new()` with no flag; `live.rs` `update_metadata_cache` preserves the old resources whenever the **new list is empty** and the hash matches — the v2.33.0 rule — so a server that genuinely removed its resources keeps advertising stale `read_*` tools from cache on the next start; `update_server_metadata` returns without retiring anything when the connection is not connected. The remote-close publication needs `MCP-131`'s `onclose`. **This commit was never named by the v2.33.0..v2.37.0 triage** |
| `MCP-584` | medium | 13d/13e | `upstream-drift` | **missing** | MCP 2026 multi-round `input_required` results are refused outright; upstream fulfils them through the registered elicitation/sampling handlers and reports the no-handler case as `input_required_needs_ui` | `5b31827` ("harden 2026 input-required flows", **v2.32.0** — so `v2.26.1..v2.32.1` drift the 2026-09-04 triage did not catch): `errors.ts:225-360` `getInputRequiredNeedsUiDetails` / `InputRequiredNeedsUiError` (bounded server/tool/resource/inputKey/inputMethod details, typed-code matching only); consumed at `proxy-modes.ts:1710` and `direct-tools.ts:398`. The rounds themselves are driven by `@modelcontextprotocol/client` 2.0.0 — `__tests__/mrtr-sdk-integration.test.ts` "fulfills one input_required result through the proxy executeCall path", "…through a direct MCP tool executor", "echoes requestState byte-for-byte on both retries" | `live.rs` `call_tool` and `read_resource` answer `ServerResult::InputRequiredResult` with an error (`…answered \`input_required\`; this client sends no in-call input responses`) even when `runtime.rs` has installed elicitation and sampling hooks; no `input_required_needs_ui` detail exists (`grep` = 0). rmcp models the result variant; whether it exposes the retry-with-responses round is unverified and sizes the fix |
| `MCP-585` | **high** | 13b/13c | `upstream-drift` | **missing** | Agent Plugin `args`, `cwd` and `headers` are literal — no `${VAR}` interpolation, no `!command` execution — and hashed raw | `23c2852` (#572, **v2.34.0**): `agent-plugin-provenance.ts` (`LITERAL_PLUGIN_FIELDS = args env cwd headers`, set by `agent-plugin-loader.ts:243`/`:271` on every plugin server, preserved through `cloneMcpConfig` and `mergeConfigs` — `config.ts:722`); `server-manager.ts:1048-1050` literal `args`/`cwd`, `:1075` env literal, `:1506` `literalHeaders` skips both `hasCommandHeader` and `resolveCommandSecretsRecord`; `metadata-cache.ts:92-95` raw hashing; `mcp-auth-flow.ts:77`, `:324` literal OAuth headers | `agent_plugin.rs` sets only `literal_env: Some(true)`. Plugin `headers` pass `translate_headers` (HTTP-syntax validation only) and reach `secrets.rs` `resolve_http_secrets` → `resolve_command_secret`, which **runs** any `!`-prefixed value through `/bin/sh -c` and interpolates `${VAR}` in every other one; `args` go through `interpolate_env_vars`. So a third-party plugin that declares only a remote `streamable-http` server can make cyrup execute a local shell command at connect time, or copy a user environment variable into a header sent to the plugin's URL — the boundary `agent_plugin.rs`' own module doc states ("must not be able to … read an environment variable through interpolation"). Rated high, not critical: a plugin can already ship a stdio server that runs its own code, so the escalation is confined to HTTP-only plugins. **Never named by the v2.33.0..v2.37.0 triage** (only as a clone-HEAD sha in `13-cyrup-mcp.md`) |

### Census, re-derived (counted, 523 units)

Counted by script: one bold status cell per row of *Every unit, by section* (437), plus the first
bold status cell of each `MCP-500`–`MCP-582` row in tables A–D (83; `MCP-500`'s authoritative row is
table A's), plus table E (3). Tallies are over those cells only; prose mentions are not counted.

| status | the 437 | A–D (83) | E (3) | **2026-09-24 third pass (523)** | second pass (520, arithmetic) |
|---|---:|---:|---:|---:|---:|
| `implemented` | 347 | 0 | 0 | **347** | 253 |
| `partial` | 42 | 14 | 1 | **57** | 89 |
| `missing` | 21 | 63 | 2 | **86** | 145 |
| `not-applicable` | 27 | 6 | 0 | **33** | 33 |

By section, the 437 (implemented / partial / missing / n-a): 13a 45/4/1/1 · 13b 38/6/3/4 ·
13c 33/7/6/5 · 13d 33/3/0/0 · 13e 43/4/1/5 · 13f 33/4/0/4 · 13g 38/7/1/3 · 13h 52/1/1/1 ·
13i 32/6/8/4. **Open work is now concentrated in the tables filed since v2.32.1** (80 of the 143
open units are `MCP-500`+), which were read on the upstream side in far more depth than the
original 437 ever were. The floor/ceiling caveat still applies in one direction: the 347
`implemented` rows have not been re-read for regression against v2.33.0+ as a set; the ones whose
upstream obligation this pass saw move are named in their closure notes.

### Third pass — window commits no pass had named

`git log --no-merges v2.33.0..v2.37.0` has 57 commits; twelve appear nowhere in area 13. Read at
the stat level (and in full where a runtime file moved):

- **`464337b` (#566)** — `MCP-583`. **`23c2852` (#572)** — `MCP-585`.
- `670bcfb` (#573), `cd81ba2` (#648) — refactors of *unreleased* in-window code (their own titles);
  the released behaviour is what the rows above cite at v2.37.0.
- `fd1c46b` (#557) — `http-ca.ts` bridges `Request` inputs for the CA fetch; a JS `fetch` input-shape
  fix inside `MCP-566`'s surface, no separate obligation.
- `3236e7b` (#625) — `request-headers-command.ts` keeps an abort signal alive across GC; JS-runtime
  only.
- `e8e3014`, `cacda95` (Pi 0.86/0.87 support), `beef601`, `a2aab87` (Windows CI/test runner),
  `ce2cb8c`, `619d7e3` (tests), `2459771` (README) — no runtime surface.

### Leads resolved by this pass

None were open in this file (the second pass resolved them). One amendment is recorded instead of a
row: the two *Shared MCP config: …* header lines `showStatus` prints (`commands.ts:144-150`) came in
with `c893a3d` (#478), the commit `MCP-528` already owns; they belong to that row's work.

### Still unread after this pass

Nothing in the 157-row window. Not re-read, by design: the 347 `implemented` rows as a regression
set, and tables A–D's rows (filed from v2.32.1 onward; C and D read on both sides today). The
v2.33.0..v2.37.0 commit census above now names every non-merge commit in that range.

## UNVERIFIED — 2026-09-14 census of the `v2.32.1..v2.33.0` window (leads, not status)

> **RESOLVED 2026-09-24 (second pass).** This window was read in full and every lead below is
> promoted (table D) or struck with evidence — see *Leads resolved*. Left standing as history.

**Nothing in this section changes a status.** No row below the fold moves between `implemented`,
`partial`, `missing` and `not-applicable`; no severity, no count and no census total is touched. No
`MCP-NNN` id is assigned to anything new — numbering resumes from **`MCP-539`** when a pass that
read both sides files it. This is a worklist.

Census method: upstream read only via `git -C tmp/pi-mcp-adapter show v2.33.0:<path>` and
`git diff v2.32.1..v2.33.0`; cyrup read at `b28d3ff`. cyrup's `credentials.rs`, `oauth.rs`,
`proxy/description.rs`, `commands.rs` (the logout path) and the consent surface were **not opened**.
Absence claims of the form "grep = 0 hits" are greps over `crates/cyrup-mcp/src` at `b28d3ff`, not
proof that a differently-named counterpart does not exist.

### Two surfaces this file owns

- **`cyrup-mcp` credential env switches collapsed to single-name `CYRUP_MCP_*`** · S · BOTH SIDES
  READ at `b28d3ff`. `crates/cyrup-mcp/src/credentials.rs:177` is
  `pub const TEST_AUTH_STORE_ENV: &str = "CYRUP_MCP_TEST_AUTH_STORE"`, and so are its five siblings:
  all six `[&str; 2]` dual-name constants became plain `&str`, `env_first` became `env_lookup`, and
  the six `PI_MCP_ADAPTER_*` spellings are no longer honoured. `request_headers_command.rs` lost
  `PI_MCP_ADAPTER_TEST_FAIL_PS` the same way. **Lead against `MCP-282`** (recorded `implemented` at
  `:1141` on the evidence "declares all six switches as `[&str; 2]` pairs with `CYRUP_MCP_*` first"):
  that evidence is false at `b28d3ff` and the behaviour it describes is gone. An implemented-unit's
  evidence invalidated, not a new gap. **The row is untouched.**
- **The in-tree dual-read precedent `13f` cites no longer exists** · S · cyrup side read at
  `b28d3ff`. `crates/cyrup-provider/src/auth/oauth/callback.rs:55-63` reads one key,
  `CYRUP_OAUTH_CALLBACK_HOST`, defaulting to `127.0.0.1`; the `PI_OAUTH_CALLBACK_HOST` fallback is
  gone, and per `dd44b3c`'s own message that is a behaviour flip — a `PI_`-only environment now
  resolves `127.0.0.1` where it used to resolve whatever that variable said (tests at
  `callback.rs:1129-1136` pin `0.0.0.0` as now inert). `crates/cyrup-config/src/paths.rs:335` is the
  same story for the agent dir. **Lead against every PLANNED unit that prescribes the convention**:
  `13f-mcp-credentials.md:297`, `:315`, `:873`, `:1432`, `:1439`, `:1713` and `13a:240`, `:1129`,
  `:2187`, `:2462`, `13c:1003` all state a `CYRUP_* → PI_* → default` ladder as the established
  in-tree precedent. The middle rung is deleted workspace-wide, so an unbuilt unit is currently
  specified against a precedent that no longer exists.

### Rows whose recorded obligation or evidence the window invalidates

Version lag, not regression: every one of these was true at the tag its pass read. **No status cell
is edited.** Each is a lead to restate the obligation *before* the unit is worked, because working
it against the recorded text ships the v2.32.1 shape and makes the delta a second pass.

| row | recorded | at v2.33.0 | filed in |
|---|---|---|---|
| `MCP-500` | `URL_BOUND_AUTH_FIELDS` "is five fields", cited `config.ts:525` | **six**, at `config.ts:553` (`caFile`). Schedule with `MCP-501` as one change, now three-way | 13b |
| `MCP-101` (`partial`) | `resolveEnv(env, serverName, literalEnv)` | four-argument: `resolveEnv(env, serverName, literalEnv = false, inheritEnv = true)`, `server-manager.ts:1742-1751` | 13c |
| `MCP-137` (`missing`) | "the per-server **six**-key object" | **seven** keys — `directToolCount` joins `McpServerStatusSnapshot` (`types.ts:42`) | 13c |
| `MCP-513` (`missing`) | "provider-safe sanitisation" | a named algorithm: `formatServerNamespace`, `types.ts:521-543`, 59-char cap + sha256-16 tail. Add `22682de` (#529) to its cite list; it has a second consumer now | 13e |
| `MCP-232` (`implemented`) | 3-component approval cache key, cache consulted before the broker | key gains a **definition hash**; broker now runs **before** the cache. `crates/cyrup-mcp/src/state.rs:392` is the 3-component form, `proxy/approval.rs:303` the old order | 13e |
| `MCP-226` (`implemented`, `hand-written`) | verdict justified at `13e:378` by cyrup's truncation differing from the adapter's own | upstream **deleted** its `truncateHead` and adopted the host's. The verdict's premise is inverted and the classification is an open question; `13b:543` / `13e:837`'s `50 * 1024` / `2000` are no longer upstream's source of truth | 13e |
| `MCP-452` (`implemented`) | cyrup's per-candidate auth probe matches upstream | **reverse lag** — upstream deleted the probe for `ModelRegistry.complete`. Needs a ruling, not a carry | 13i |
| `MCP-507` (`missing`) | the transient-503 arm of `enrichHttpConnectionError` | the function gained a macOS Local Network Privacy arm *before* the 503 one. Read and schedule the two together | 13c |
| `MCP-524`, `MCP-522`, `MCP-525`, `MCP-531` | table-B rows whose TypeScript has never been read | all four are extended by this window (credential transactions · `{port}` redirects and listener release · theme fallback). One read pass per cluster covers both tags | 13f/13g/13h |
| `MCP-027a` (`missing`) | `sendMessage`'s `triggerTurn` pre-turn convergence gate | now a **hard prerequisite**: the new `mcp-oauth-status` message is delivered through exactly that gate | 13a |

**One seam gap this window exposes that no row holds.** `withFileMutationQueue` — the host-supplied
cross-process config-write serialiser the new install path runs every persist through — has no cyrup
counterpart (`grep -rn 'file_mutation_queue' crates/cyrup-mcp/src` = 0). That is independent of the
install unit and is currently unfiled.

## Re-audit — 2026-09-04, at `pi-mcp-adapter` v2.32.1

> **Everything below the next four sections is dated 2026-08-21/22 and is superseded where it
> conflicts with this one.** It is kept, not rewritten, for the same reason the 2026-08-20 retarget
> kept its v2.25.0 citations: a rewritten history is worse than a dated one.

The clone moved from **v2.26.1** (`fafae21`) to **v2.32.1** (`10a4536`) and nobody had opened these
files since. Meanwhile `crates/cyrup-mcp` went from **45,156 lines / 19 modules** to **79,942 lines /
30 modules + a `proxy/` module tree** across six landed waves — `36184202` (a model can call a
server's tools), `9cf02a66` (OAuth-protected HTTP end to end), `35a0e51c` (sampling and elicitation),
`8e2c751c` (the wire tracer), `f4c85252` (the `/mcp` command surface and the live panels), `d4f7392c`
(two parity defects). The census below the fold predates **all six**.

### What this pass covered, and what it did not

**Covered.** (1) The upstream delta `v2.26.1..v2.32.1`, triaged file by file. (2) All eight units the
*Critical-severity open work* table lists. (3) A systematic 1-in-6 sample of the 196 open units,
drawn in id order off the *Every unit, by section* table (`sorted-open[::6]`), which lands
proportionally on every section: 13a 7, 13b 3, 13c 6, 13d 1, 13e 3, 13f 1, 13g 1, 13h 4, 13i 7.

**Not covered.** (a) The other 159 open units were **not** re-read; their rows below still say what
the 2026-08-21 audit said, and every one of them should be assumed stale in the same direction. (b)
The 214 `implemented` rows were not re-checked for regression. (c) This pass ran no build and no
test; like its predecessor it is a reading of source. (d) Its bar is **one level below** the
predecessor's: the 2026-08-21 audit ran two readers per ruling with an adversarial skeptic; this one
re-checks each sampled unit's own *named unmet obligation* — does the named symbol exist at HEAD, and
does it have a production call site — rather than re-deriving the whole unit from the TypeScript. A
row moved to `implemented` here means **its recorded obligation is met**, not that the unit was
re-verified end to end. (e) For the 40 new units filed below, table B's upstream side is
changelog-sourced and the TypeScript has not been read.

### The sample, and every ruling it produced

33 units, each re-checked against `crates/cyrup-mcp` at HEAD. **26 of 33 are now `implemented`;
4 remain `partial`; 3 remain `missing`.** Nothing here was inferred from a name: each cite is a
symbol plus a production call site.

| unit | § | was | now | evidence at HEAD |
|---|---|---|---|---|
| `MCP-006` | 13a | partial | **implemented** | `owner.rs:381`'s `fenced!` block covers all 66 `HostServices` methods (name-set diff, `36184202`); the row's "carries 31 methods" is the pre-wave count |
| `MCP-013` | 13a | partial | `partial` | `on_session_start` (`extension.rs:1125-1167`) now builds the owner and calls `start_initialization`, but still reads no `MCP_DIRECT_TOOLS` and awaits no initialization — `grep DIRECT_TOOLS_ENV_VAR crates/cyrup-mcp/src/extension.rs` is empty |
| `MCP-019` | 13a | missing | **implemented** | `runtime.rs:526-533`: `bootstrap_all`, the file-absent probe, and the "exists and does not parse ⇒ truncate but do NOT bootstrap" arm; consumed at `runtime.rs:582` |
| `MCP-025` | 13a | partial | **implemented** | all three byte-exact strings exist and are emitted: `MCP: Failed to connect to {name}: {display}` (`runtime.rs:728`), `MCP: {name} - {failed_tools} tools skipped` (`:803`), `MCP: {n}/{m} servers connected ({t} tools)` (`:830`) |
| `MCP-030` | 13a | partial | **implemented** | `state.rs:281` `catch_unwind(AssertUnwindSafe(…))` around the listener with the downcast-to-message arm; `set_tool_metadata_listener` at `state.rs:296` |
| `MCP-038` | 13a | missing | **implemented** | `extension.rs:355 fn deactivate_tools` over `fallback_deactivated_tools` (`:123`, read back at `:395`) |
| `MCP-045` | 13a | partial | **implemented** | the `on_event` arm exists — `extension.rs:2234` guards on `crate::proxy::error_vocab::tool_error_override(Some(details)) == Some(true)` |
| `MCP-075` | 13b | partial | **implemented** | `registration.rs:376 tool_name_candidates(name, server, prefix, legacy)`; the legacy arm is taken at `:636` and `:640` |
| `MCP-085` | 13b | partial | `partial` | `grep -rn 'fn format_terminal_error' crates/cyrup-mcp/src` is still empty; `ui.rs:404 sanitize_terminal_text` is the half that exists |
| `MCP-092` | 13b | missing | **implemented** | `schema.rs:50 compile` — the 2020-12 / draft-07 / unstamped ladder with `should_validate_formats(true)` and `unsupported_dialect_message`, cached at `schema.rs:85` |
| `MCP-102` | 13c | partial | **implemented** | `runtime.rs:1539/1561/1587` and two production call sites at `runtime.rs:3217` and `:4625` |
| `MCP-108` | 13c | missing | `partial` | `cache_entry_is_usable` (`cyrup-ext/src/caps/proc/npx_resolver.rs:490`) and `parse_package_spec` (`:528`) landed, but `entries` is a typed `HashMap<String, NpxCacheEntry>` (`:99`) rather than a per-entry drop-failures convert, and the Windows `npm.cmd`/`PATHEXT` half is absent (`grep -n 'npm.cmd\|PATHEXT\|cmd /c'` empty) |
| `MCP-119` | 13c | missing | **implemented** | `runtime.rs:4752 async fn discover` issues `list_all_tools` (`:4795`) with capability-gated `resources/list` / `prompts/list` and sets `prompt_discovery_failed` (`:4845`), read in production at `runtime.rs:763` and `commands.rs:269` |
| `MCP-126` | 13c | missing | **implemented** | `server_manager.rs:1383 close_generations`, per-name read at `:2105`, bump at `:1954`/`:2492`; `close_all` (`:2674`) with the late sweep documented and implemented at `:2665` |
| `MCP-132` | 13c | missing | `missing` | still nothing: no counterpart to `mcp-probe.ts`'s three-strategy ladder anywhere in `crates/cyrup-mcp/src` |
| `MCP-139` | 13c | partial | **implemented** | `registration.rs:150 METADATA_CACHE_VERSION`, `:1031 load_metadata_cache` with the version gate at `:1034` |
| `MCP-164` | 13d | partial | **implemented** | `live.rs:1093` really issues `Peer::send_request_with_option`; `call_tool_outcome` at `:1197`, `ProxyEnv::call_tool` at `:1467` |
| `MCP-207` | 13e | missing | **implemented** | `registration.rs:1612 build_tool_metadata` in the post-`14c0e6c` shape — `has_candidate` at `:563` with the current-candidate subtraction at `:569` and the matcher arm at `:605` |
| `MCP-217` | 13e | missing | **implemented** | `extension.rs:214 sync_tool_surface` (production) and `live.rs:1619` (the `ProxyEnv` half); the fingerprint diff is at `extension.rs:278` with the wholesale-replace rule at `:299-302` |
| `MCP-232` | 13e | partial | **implemented** | `proxy/approval.rs:286 ensure_tool_call_approved` with the `approved_tool_calls` lookup at `:306` and insert at `:348`; production implementor `live.rs:1727` → `:1741`; call site `proxy/call.rs:787` |
| `MCP-287` | 13f | partial | `partial` | `TODO(MCP-287)` is still live at `credentials.rs:81` — the three timeout/exit-code fixtures |
| `MCP-334` | 13g | partial | **implemented** | `McpExtension` now overrides `NativeExtension::execute_command` (`extension.rs:2280`) and `argument_completions` (`:2311`); `MCP_AUTH_COMMAND` registered at `registration.rs:98`/`:2971` |
| `MCP-370` | 13h | partial | **implemented** | the in-tree reader carries upstream's four-variant `ToolPrefix` (`mcp_direct_tools.rs:139-144`) **and** applies the filters — `is_tool_allowed(include, exclude, …)` at `:694-704` |
| `MCP-385` | 13h | missing | **implemented** | `commands.rs:199 pub fn show_prompts(state, has_ui)` |
| `MCP-390` | 13h | partial | **implemented** | `extension.rs:1624 authenticate_server`, `:1958 terminal_hyperlink` (OSC-8) and its production caller at `:1988`; the panel arm at `ui.rs:2410` |
| `MCP-397` | 13h | missing | **implemented** | `prompts.rs:140 resolve_prompt_args` and `:196 build_usage_message`, raised at `:187` |
| `MCP-452` | 13i | missing | **implemented** | `sampling.rs:445 resolve_sampling_model` |
| `MCP-460` | 13i | partial | **implemented** | the dispatch exists — `elicitation.rs:964` matches `ElicitRequestParams::FormElicitationParams` against the URL arm, into `handle_form_elicitation` (`:796`) / `handle_url_elicitation` (`:881`) |
| `MCP-466` | 13i | missing | **implemented** | `elicitation.rs:225 format_choice`, `:237 unique_labels`, `:270 humanize_name` |
| `MCP-472` | 13i | missing | **implemented** | the three `-32602`s are raised at `elicitation.rs:889` (unsupported), `:892` (unparseable) and `:895` (non-HTTP(S)), before any dialog |
| `MCP-478` | 13i | partial | **implemented** | `trace.rs:499 is_mcp_trace_enabled`, taking `McpTraceSettings` — the `??` combiner the row asked for (see the `8e2c751c` note: `McpSettings::trace_enabled()` was deleted as its duplicate) |
| `MCP-485` | 13i | missing | `missing` | `crates/cyrup-it/tests/mcp/` is `{main,activation,live_tool_call,http_oauth,protocol_13i}.rs` — no sequential runner with post-hoc log assertions |
| `MCP-493` | 13i | missing | `missing` | no test reads `crates/cyrup-mcp/Cargo.toml`; the asserted state still holds (`toml` is already a dependency, `Cargo.toml:124`), so this is still the section's one dependency-free net add |

### The eight `critical` open units are all closed

The *Critical-severity open work* table below lists eight. All eight now meet their recorded
obligation; two of them (`MCP-142`, `MCP-146`) were already `implemented` in the unit table and only
that summary table was stale.

| unit | recorded obligation | met at HEAD by |
|---|---|---|
| `MCP-083` | `resolveCommandSecretsRecord` "does not exist" | `secrets.rs:297`, applied to `env` (`:365`) and to `headers` (`:471`); `interpolate_env_record` at `secrets.rs:110` |
| `MCP-141` | the in-tree reader hashes 11 of 15 identity keys | `mcp_direct_tools.rs:895-985` emits all fifteen — `args auth bearerToken bearerTokenEnv command cwd env excludeTools exposeResources headers includeTools protocolVersion requestHeadersCommand socket url` — matching `v2.32.1:metadata-cache.ts:82-108` key for key |
| `MCP-142` | reader emits `null` for an absent field | already `implemented` (wave 1); the summary table was never updated |
| `MCP-146` | reader builds `get_<resource>` | already `implemented` (wave 1); same |
| `MCP-232` | the gate is a trait method with a test-only implementor | `proxy/approval.rs:286` + `live.rs:1727` (see the sample table) |
| `MCP-370` | the reader's `ToolPrefix` has 3 variants; filters unapplied | `mcp_direct_tools.rs:139-144` + `:694-704` (see the sample table) |
| `MCP-394` | `TODO(MCP-394)` at `ui.rs:4781` | the marker is gone; `commands.rs:426-437` is the `programmaticConfig` branch with the byte-exact `MCP status is shown from the in-memory SDK config; configuration discovery is unavailable.` followed by `show_status`, matching `v2.32.1:commands.ts:643-656`; the zero-servers delegation and the `!hasUI` arm are at `:439-444` |
| `MCP-455` | `confirmSampling`'s gate and its two formatters | `owner.rs:802 confirm_sampling`, `:839 format_request_approval`, `:888 format_response_approval`, all three called from `sampling.rs:221` and `:255` — matching `v2.32.1:sampling-handler.ts:62/87/95/111/185` |

### The census, re-derived

**Derived, not estimated** — **37 open units** were personally re-checked at HEAD: the 33-unit
sample plus the four open `critical` units outside it (`MCP-083`, `MCP-141`, `MCP-394`, `MCP-455`).
Of those 37, **30 moved to `implemented`**, 3 stayed `partial` (`MCP-013`, `MCP-085`, `MCP-287`),
3 stayed `missing` (`MCP-132`, `MCP-485`, `MCP-493`) and 1 moved `missing` → `partial`
(`MCP-108`). Two further `critical` rows (`MCP-142`, `MCP-146`) were confirmed already
`implemented`. **All 31 changed rows are rewritten in place in the unit table below**, each carrying
a dated `Re-ruled 2026-09-04` note over its prior ruling.

Two figures below the fold are wrong independently of any of this and are corrected here: the
*Census* table says 212/100/98/27, while the *Every unit, by section* table it summarises has always
said **214/98/98/27** — wave 1's `MCP-142`/`MCP-146` were applied to the rows and not to the total.
**214/98/98/27 of 437 is the correct pre-re-audit baseline.**

The first two columns are **counts**, obtained by re-parsing the unit table below. The third is an
**extrapolation** and is labelled as one.

| status | 2026-08-21 (corrected) | 2026-09-04 (counted, 437 units) | 2026-09-04 (estimated, 477 units) |
|---|---:|---:|---:|
| `implemented` | 214 | **244** | ~369 |
| `partial` | 98 | **82** | — |
| `missing` | 98 | **84** | — |
| open (`partial` + `missing`) | 196 | **166** | ~76 |
| `not-applicable` | 27 | **27** | 32 |
| **total** | **437** | **437** | **477** |

* **Counted** — 214 + the 30 rows this pass moved to `implemented`. Every one of the 244 carries a
  citation, in this block or in an earlier wave block. The 159 open rows this pass did not open are
  counted exactly as they stood; **they are the reason the counted column is a floor and not the
  answer.**
* **Estimated** — the systematic sample's closure rate applied to those 159. The four extra criticals
  were a census of a stratum, not a draw, so they are excluded from the rate: **26 of 33, p̂ = 0.788,
  Wilson 95% CI [0.622, 0.893]**. That puts roughly **125 (CI 99–142)** of the 159 already done and
  **~34 (CI 17–60)** genuinely open. With the 7 confirmed-open leftovers that is **~41 open existing
  units (CI ~24–67)**; adding the **35 open new units** filed below gives **~76 open of 477, with a
  95% interval of roughly 59 to 102**. It is an extrapolation from a 1-in-6 sample: **do not quote it
  as a count.**

**The 13 `TODO(MCP-NNN)` marker sites in `crates/cyrup-mcp/src` name 13 distinct ids** — `MCP-082`,
`MCP-084`, `MCP-235`, `MCP-269`, `MCP-278`, `MCP-283`, `MCP-287`, `MCP-309`, `MCP-312`, `MCP-341`,
`MCP-347`, `MCP-368`, `MCP-377` — and are a floor on the open set, not its size: `MCP-013`,
`MCP-085`, `MCP-108`, `MCP-132`, `MCP-485` and `MCP-493` are all open at HEAD and carry no marker.

### The upstream delta, triaged

`git diff --stat v2.26.1..v2.32.1` is **147 files, +16,014 / −1,001**; the log is **72 commits**
(68 with `--no-merges`) across releases 2.27.0 → 2.32.1. *(Corrected 2026-09-05, review pass: this
line and the two copies of it in `13-cyrup-mcp.md` and `MCP-PORT-METHODOLOGY.md` said 79.
`git -C tmp/pi-mcp-adapter rev-list --count v2.26.1..v2.32.1` = 72. The file and line counts on
either side of it re-derive exactly.)* Tests are 66 of those files. Production TypeScript changed in 38
files, six of which are new.

| upstream file(s) | what moved | triage |
|---|---|---|
| `metadata-cache.ts` | `computeServerHash` gains an injected `environment` (already covered — `dirs.rs::compute_mcp_server_hash_with`); `isServerCacheValid` gains an `entry.ttlMs` hint; `saveMetadataCache` drops the 2-space indent; `createCachedToolSelectorCandidateIndex` is extracted and shared | touches `MCP-139`/`MCP-140`/`MCP-145`; **three new units** (`MCP-505`, `MCP-506`, `MCP-539`). The **15 identity keys are unchanged** — `bearerTokenStore` is not hashed, `resolveBearerToken` (`v2.32.1:utils.ts:200-205`) is untouched |
| `config.ts`, `types.ts` | `ServerEntry` 29 → **30** (`bearerTokenStore`); `McpSettings` 24 → **26** (`strictDirectToolArguments`, `directToolResultDetails`); `URL_BOUND_AUTH_FIELDS` 4 → **5** | `MCP-069`/`MCP-070` amendments; **new units** `MCP-500` (critical), `MCP-501`, `MCP-503` |
| `package-mcp-loader.ts` *(new)* | `pi.mcp` package manifests as an MCP server source | a **source the six-rung ladder has no rung for** — `MCP-502` |
| `mcp-bearer-store.ts` *(new)* | OS credential store for static bearers, `token set\|status\|remove` CLI | 13f has no unit — `MCP-501`, `MCP-504` |
| `namespace-tools.ts` *(new)* | `mcp__<server>` namespace-proxy tools for proxy-only servers | 13e has no unit — `MCP-513` |
| `mcp-references.ts` *(new)* | pure `mcp:` reference resolver | 13e has no unit — `MCP-514` |
| `failure-backoff.ts` *(new)* | `getFailureAgeSeconds` / `getFailureMessage` / `isServerInActiveFailureBackoff` extracted from `index.ts` | **refactor only** — already `MCP-024`/`MCP-033`/`MCP-137`. No unit |
| `errors.ts` *(new)* | the `McpUiError` taxonomy extracted; consumed from `proxy-modes.ts`, `direct-tools.ts`, `error-signal.ts` as well as the UI files | already `MCP-089` ("Port the error taxonomy"). No unit |
| `server-manager.ts` (+452) | 2026-07-28 "modern listen" catalogs, streamable-HTTP session expiry reconnect, transient 503 policy, stderr capture, `cwd` diagnosis, sampling changes | `MCP-507`, `MCP-508`, `MCP-509`, `MCP-534` |
| `index.ts` (+474), `init.ts`, `lifecycle.ts`, `state.ts` | runtime registration API, init retry after a failed first init, status-after-tool-sync, backoff hiding | `MCP-510`, `MCP-511`, `MCP-512`, `MCP-515` |
| `proxy-modes.ts` (+246), `direct-tools.ts` (+232) | request-local `onprogress` bridge, nested-gateway-params refusal, strict advertised-schema validation, guarded result details, argument normalisation, description purity | `MCP-516`, `MCP-517`, `MCP-518`, `MCP-519`, `MCP-520` |
| `mcp-oauth-provider.ts`, `mcp-auth-flow.ts`, `mcp-callback-server.ts` | `authServerMetadataUrl`, HTTPS manual callback, implicit auth reusing stored credentials, cross-process invalidation, idle listener release | `MCP-521` … `MCP-525` |
| `mcp-panel.ts`, `mcp-setup-panel.ts`, `mcp-status.ts`, `cli.js`, `commands.ts` | `/pi-mcp` alias, ctrl+d enable/disable in the panel, setup write-target choice, theme fallback, per-server status notes | `MCP-526` … `MCP-531` |
| `mcp-probe.ts` | HTTP 202 and unauthenticated 401 report an ambiguous endpoint shape | extends the still-`missing` `MCP-132` — `MCP-532` |
| `request-headers-command.ts` | Windows `taskkill` exit 128; `ps` scan without overflowing `spawnSync`'s 1 MiB buffer | `MCP-533` |
| `search-ranking.ts`, `mcp-output-guard.ts`, `utils.ts`, `tool-result-renderer.ts`, `error-signal.ts` | per-catalog field/keyword reuse, guard and renderer evolution | inside existing units (`MCP-174`, `MCP-225`, `MCP-085`, 13e renderers). No new units |
| `host-html-template.ts`, `ui-server.ts`, `ui-session.ts`, `ui-resource-handler.ts`, `sandbox-proxy-template.ts` *(new)* | MCP Apps: second-origin sandbox proxy, UI capability advertising, streaming | **CUT 2** — `MCP-535`, `MCP-536`, recorded so the cut stays deliberate |
| `mcp-code.ts` | Node 24 FD warnings, bounded submitted code | **CUT 4** — `MCP-537` |
| `dist/**`, `tsconfig.public.json`, `package.json`, `test-runner.mjs`, `.github/**` | published public-export build, package subpaths, CI | npm packaging with no Rust analogue — `MCP-538` |

### New units — v2.26.1 → v2.32.1

Numbering continues from the plan's highest existing id (`MCP-499`); **no existing id is renumbered**,
and the letter-suffix convention (`MCP-027a`, `MCP-115a`) is reserved for insertions inside a section's
own sequence, which none of these are. Each row names the section file that owns it.

**Table A — upstream side personally read at v2.32.1.**

| id | sev | § | verdict | status | title | upstream at v2.32.1 | cyrup at HEAD |
|---|---|---|---|---|---|---|---|
| `MCP-500` | **critical** | 13b | `hand-written` | **missing** | `URL_BOUND_AUTH_FIELDS` is five fields, not four | `config.ts:525` — `["headers","bearerToken","bearerTokenEnv","bearerTokenStore","requestHeadersCommand"]`, deleted from the base entry at `:572-574` when a higher-precedence source repoints `url` | `config.rs:2274` carries the four-element array *(this cite read `:1702` until 2026-09-05; `:1702` is the `AuthMode` derive — corrected by the batch-3 ledger pass, which re-found the symbol)*. Not a live leak *today* only because `bearerTokenStore` is unported; it becomes one the moment `MCP-501` lands, which is exactly the shape the v2.26.1 retarget flagged for `requestHeadersCommand`. **Land this in the same change as `MCP-501`, never after it.** **Re-read 2026-09-14 at `9aeba769` — still missing, and the recorded obligation is now understated rather than met.** `URL_BOUND_AUTH_FIELDS` is **six** fields at v2.33.0, not five: `caFile` joins at `config.ts:553` (the cited `config.ts:525` is the five-element v2.32.1 form, verified), and the strip loop is `config.ts:600-602`. cyrup at 9aeba769 carries `pub const URL_BOUND_AUTH_FIELDS: [&str; 4]` (config.rs:2274) — headers, bearerToken, bearerTokenEnv, requestHeadersCommand — short **both** `bearerTokenStore` and `caFile`. Restate the scheduling note from two-way to **three-way**: MCP-500 + MCP-501 + an as-yet-unfiled `caFile` unit must land as one change. Not a live leak today only because neither `bearer_token_store` nor `ca_file` exists as a cyrup `ServerEntry` field; the forcing function is that `merge_entry` is deliberately written with no `..` rest pattern (documented at config.rs:2269-2272), so adding either field is a compile error there, with the module's exhaustiveness test as the second guard. |
| `MCP-501` | high | 13b/13f | `hand-written` | **missing** | `bearerTokenStore: true` and the URL-bound bearer credential store | `types.ts:446` (`ServerEntry` 29 → 30); `mcp-bearer-store.ts` (385 lines) — keyring `Entry(service, account)` over a hashed URL-bound account key, with a `StoredBearerTokenRecord` payload | absent: `grep -rn bearerTokenStore crates/cyrup-mcp/src` returns two hits, both in a `config.rs:22-24` header comment that asserts *"`git grep bearerTokenStore v2.26.1` is empty — the symbol appears at no upstream tag"*. That sentence is **false at v2.32.1** and is itself a residual to fix at the same time |
| `MCP-502` | high | 13b | `hand-written` | **missing** | `pi.mcp` package manifests as a server source | `package-mcp-loader.ts:16 loadPackageMcpConfigs(cwd)` — walks configured package roots, reads each `package.json`'s `pi.mcp`, prefixes the names | the ladder in `config.rs:3231 ConfigContext::sources()` has six rungs and no manifest rung *(this cite read `:2578` until 2026-09-05; `:2578` is `read_imported_config` — corrected by the batch-3 ledger pass)* |
| `MCP-503` | medium | 13b | `hand-written` | **missing** | `McpSettings` 24 → 26 | `types.ts` — `strictDirectToolArguments` and `directToolResultDetails` are the two added keys (diffed interface-to-interface against v2.26.1) | `config.rs`'s `McpSettings` has neither. The plan's "24 keys" annotation from the v2.26.1 retarget is superseded — **Re-read 2026-09-14 at `9aeba769` — still missing, and the count in the title is wrong in a way worth recording before the row is worked.** Re-deriving the `McpSettings` interface member-by-member at both tags gives **26 at v2.32.1 and 26 at v2.33.0** (toolPrefix showStatusIcon mcpFooterStatus notifyOnStartupConnect hostConfigDiscovery agentPluginPaths idleTimeout requestTimeoutMs directTools strictDirectToolArguments directToolResultDetails warnOnLargeDirectTools scriptMode toolResultRendering collapsedResultLines approveTools disableProxyTool freezeDirectTools autoAuth sampling samplingAutoApprove elicitation outputGuard trace authRequiredMessage oauthDir) — so the window does **not** move the count. Recorded as a negative result so it is not re-derived by hand next time. What the window *does* change is the type of one existing key: `directTools` is `boolean \| "search"` at v2.33.0 and `boolean` at v2.32.1. Restate the obligation as **26 keys, one of which is a three-state** before working it. cyrup's `McpSettings` (config.rs) still carries neither `strictDirectToolArguments` nor `directToolResultDetails` (grep at 9aeba769: zero hits). |
| `MCP-505` | medium | 13c | `hand-written` | **missing** | `isServerCacheValid` honours a per-entry `ttlMs` hint | `metadata-cache.ts:114-134` — a safe-integer `entry.ttlMs`, `0 ⇒ invalid`, otherwise `effectiveMaxAge = maxAgeMs > 0 ? min(maxAgeMs, ttlMs) : ttlMs`; sourced from `ListToolsResult["ttlMs"]` (`types.ts:705`) | `grep -rn 'ttl_ms\|ttlMs' crates/cyrup-mcp/src` = 0 |
| `MCP-506` | medium | 13c | `hand-written` | **missing** | metadata-cache writes are compact | `metadata-cache.ts:78` — `JSON.stringify(merged)`, where v2.26.1 wrote `JSON.stringify(merged, null, 2)`; atomic tmp+rename and the cross-process merge are unchanged | `dirs.rs::save_metadata_cache` — check and match the byte shape; a differential against a stock adapter reads the file, so indentation is observable |
| `MCP-539` | low | 13c | `hand-written` | **missing** | one shared selector-candidate index across `reconstructToolMetadata` calls | `metadata-cache.ts:266+ createCachedToolSelectorCandidateIndex(configuredServers, cache, prefix)` plus the `sharedSelectorCandidateIndex` parameter at `:200` | `registration.rs::CandidateIndex` is the per-call form; the startup rehydration builds it per server. Cost only, never behaviour |
| `MCP-535` | n/a | 13h | `cut` | **not-applicable** | MCP Apps second-origin sandbox proxy | `sandbox-proxy-template.ts` (new, 217 lines) — `SANDBOX_PROXY_SANDBOX`, `/sandbox`, `buildSandboxProxyHtml(parentOrigin)`; `ui-server.ts` +289 | **CUT 2**, honoured. Recorded so the next reader does not re-derive the cut |
| `MCP-536` | n/a | 13h | `cut` | **not-applicable** | `io.modelcontextprotocol/ui` capability advertising | v2.31.0, `#465` — the adapter advertises MCP Apps UI support so servers expose UI resources | **CUT 2**. It is a *capability* line, so note it in `build_client_capabilities`' doc rather than implementing it |
| `MCP-537` | n/a | 13d | `cut` | **not-applicable** | `mcpScript` FD warnings and bounded submitted code | `mcp-code.ts` +6; `#407`, `#413`, `#418` | **CUT 4** |
| `MCP-538` | n/a | — | `cut` | **not-applicable** | published public-export build and package subpaths | `tsconfig.public.json` (new), `dist/**` (+3,600), `package.json` +33 | npm packaging; a Rust crate has no analogue. `MCP-493`'s manifest-policy test is the nearest live obligation and is unrelated |

**Table B — filed from the v2.32.1 CHANGELOG and commit log; the TypeScript has NOT been read.**
Each row names its upstream commit or PR so the read pass has a starting point. **Do not work one of
these without first reading its upstream file at `v2.32.1` and rewriting the row's obligation.**

| id | sev | § | verdict | status | title | upstream |
|---|---|---|---|---|---|---|
| `MCP-504` | high | 13f | `host-verb` | **missing** | `token set\|status\|remove <server>` — stdin-only, never a token as an argument | v2.27.0, issue `#366`; `cli.js` |
| `MCP-507` | high | 13c | `hand-written` | **missing** | transient HTTP 503 as an availability error: one quiet startup warning, cached keep-alive catalogs preserved, bounded deferred recovery, no extra gateway retries, endpoint not misdiagnosed as non-MCP — **Re-read 2026-09-14 at `9aeba769` — still missing, and the obligation as recorded is understated.** At v2.33.0 `enrichHttpConnectionError` (`server-manager.ts:1019-1031`) runs three arms in this order: (1) `process.platform === 'darwin'` + a non-empty `localNetworkFailureCodes(error)` + `isLiteralLocalAddress(resolveServerUrl(definition)!)` → the long System Settings / Privacy & Security / Local Network message; (2) `isTransientHttpConnectError(error)` → `… — endpoint is temporarily unavailable (HTTP 503)`; (3) the `probeMcpEndpoint` fallback. The macOS arm **precedes** the 503 arm, so a 503 that also carries a local-network failure code against a literal-local URL takes arm 1, not arm 2 — the two cannot be ported independently without changing which message fires. cyrup has none of the three: grep for `Local Network Privacy` / `temporarily unavailable` / `probe_mcp_endpoint` / `enrich_http` over crates/cyrup-mcp/src at 9aeba769 is empty, and the nearest site, `runtime.rs:4265 connect_http_client`, has no enrichment path. Schedule MCP-507 + MCP-132 + MCP-532 as one read of this function. | `a3072f6` (#411), `#424`, `f176ef3` (#426) |
| `MCP-508` | high | 13c | `hand-written` | **missing** | MCP 2026-07-28 "modern listen": catalog listens recover from dropped listens, refresh quietly, expired Streamable HTTP sessions reconnect | `b2d795a`, `5b31827`, `#468`, `#369`/`#370` |
| `MCP-509` | medium | 13c | `hand-written` | **missing** | a missing or non-directory stdio `cwd` is named as such instead of blaming the executable | `a02a059` (#445), `#442` |
| `MCP-534` | low | 13c | `hand-written` | **missing** | a bounded keep-alive `tools/list` refresh timeout does not mark a healthy slow server failed | `c695664` (#402), `#400` |
| `MCP-510` | high | 13a | `host-addition` | **missing** | `registerMcpServer({pi, name, definition})` — session-scoped runtime registration, proxy-tool-only, never persisted, duplicate names fail closed; cross-extension through a versioned shared event contract; plus fail-closed runtime server snapshots | `f406cdf`, `00413a0` (#447), `6bde190` (#454), `#443` |
| `MCP-511` | medium | 13a | `hand-written` | **missing** | a failed first-time initialization keeps its reason and retries on the next `mcp(...)` call instead of latching `MCP not initialized` | `4755775` (#429), `#428` |
| `MCP-512` | medium | 13a | `hand-written` | **missing** | the first `connected` status snapshot waits for direct-tool synchronisation | `068d688` (#380) |
| `MCP-515` | high | 13e | `hand-written` | **missing** | servers in active failure backoff are hidden from cached direct tools, from gateway list/search/describe, and from status tool counts | `26527c5` (#434) |
| `MCP-513` | high | 13e | `hand-written` | **missing** | `mcp__<server>` namespace-proxy tools for proxy-only servers: valid cached metadata only, resource-only servers included, only prior namespace registrations cleaned up, `MCP_DIRECT_TOOLS` selection honoured, ambiguous normalized names skipped, provider-safe sanitisation, server-scoped raw name resolution — **Re-read 2026-09-14 at `9aeba769` — still missing.** The 'provider-safe sanitisation' clause is a named, specified algorithm upstream, not a loose requirement: `formatServerNamespace` at `types.ts:525-537` — hyphens to underscores, an injective `_<hex>_` escape behind `ENCODED_SERVER_NAMESPACE_MARKER = "_mcpns_"` (types.ts:521), a `MAX_SERVER_NAMESPACE_LENGTH = 59` cap (types.ts:523), and on overflow a `_mcpns__h_<head>_<sha256-16>` form that hashes the ASCII encoding rather than the raw name. cyrup has only `sanitize_server_prefix` (registration.rs:229, :343) — a different function, with no length cap, no marker and no hash tail. The function also gained a **second** consumer this window: `claude-plugin-loader.ts:3` / `:55` calls it to detect normalized-name collisions between plugin-supplied servers, so add that consumer to this row's cite list alongside 22682de (#529). | `8285d35` (#414), `2dafdc4` (#463), `34f4c2c` (#452) |
| `MCP-514` | medium | 13e | `hand-written` | **missing** | the pure `mcp:` reference resolver over explicit config and cache inputs | `23ceb45` (#425), `#420` |
| `MCP-516` | medium | 13e | `hand-written` | **missing** | opt-in strict advertised-schema validation with one-layer JSON recovery for object/array properties, and opt-in guarded raw MCP result details bounded by summarisation (`strictDirectToolArguments`, `directToolResultDetails` — see `MCP-503`) | `5088b4e` (#430) |
| `MCP-517` | medium | 13e | `hand-written` | **missing** | JSON-string tool-call arguments normalised before approval **and** transport, preserving embedded quotes | `dd64f97`, `#377` |
| `MCP-518` | medium | 13d | `hand-written` | **missing** | the gateway description is a pure function of config, stable across metadata-only refreshes, with live counts behind `mcp({})` | `5f07874` (#432) |
| `MCP-519` | medium | 13d | `hand-written` | **missing** | proxy tool-call progress bridged to the UI through a request-local `onprogress` | `cb3262b` (#440) |
| `MCP-520` | low | 13d | `hand-written` | **missing** | gateway parameters nested inside `args` fail with top-level guidance rather than dispatching inconsistently | `9f81a75` (#417) |
| `MCP-521` | medium | 13g | `hand-written` | **missing** | `oauth.authServerMetadataUrl` for providers whose AS metadata is not discoverable through protected-resource metadata | `2f944b3` (#461), `#458` |
| `MCP-522` | medium | 13g | `hand-written` | **missing** | HTTPS callback URLs completed through manual callback for pre-registered clients | `f319288` (#464) |
| `MCP-523` | medium | 13g | `hand-written` | **missing** | implicit auth reuses URL-bound stored credentials while preserving the anonymous fallback | `ff234b8` (#472) |
| `MCP-524` | medium | 13g | `hand-written` | **missing** | token invalidation preserves credentials replaced by another process instead of deleting a newly authorized shared credential | `f30c4e7` (#423), `#422` |
| `MCP-525` | low | 13g | `hand-written` | **missing** | the OAuth callback listener is released after an idle flow, and MCP pickers stay hidden while nested OAuth input is active | `3e974f3` (#403), `#404` |
| `MCP-526` | medium | 13h | `host-verb` | **missing** | `/pi-mcp` as an alias for `/mcp` when the host reserves `/mcp` | `c59698e` (#398), `#391` |
| `MCP-527` | medium | 13h | `hand-written` | **missing** | ctrl+d on a panel server row toggles enabled/disabled; saving persists `disabled` to the project layer and reloads, matching `/mcp disable` (cyrup already has `write_project_server_disabled` — this is the panel half) | `f94a3a7` (#479) |
| `MCP-528` | medium | 13h | `hand-written` | **missing** | `/mcp setup` offers project `.mcp.json` vs global `~/.config/mcp/mcp.json` as the write target, keeps Pi-owned files in the advanced flow, and says where a server will be saved | `c893a3d` (#478), `#477` |
| `MCP-529` | low | 13h | `open-decision` | **not-applicable** | the opt-in Parallel Search preset | `d936570` (#448) — a vendor preset, not a parity obligation. Ruled `open-decision` pending a product call, exactly as `MCP-048` was |
| `MCP-530` | low | 13h | `hand-written` | **missing** | per-server proxy lists distinguish cached lazy tools from servers needing auth, while preserving active failure backoff | `824b137` (#474) |
| `MCP-531` | low | 13h | `hand-written` | **missing** | status updates fall back to plain text when a host supplies a theme with no styling methods, and non-callable UI themes are guarded | `7736c84` (#451), `bb10169` (#450), `#449` |
| `MCP-532` | medium | 13c | `hand-written` | **missing** | HTTP 202 and unauthenticated 401 probes report an ambiguous endpoint shape instead of "not MCP" — an **extension of the still-`missing` `MCP-132`**, so schedule them together | `0b76154` (#419), `#415` |
| `MCP-533` | low | 13c | `hand-written` | **missing** | Windows `taskkill` exit 128 counts as successful cleanup; the `ps` cleanup scan does not overflow `spawnSync`'s 1 MiB buffer | `8100035` (#460), `93bcba8` (#401), `#399` |

**Totals after this pass: 477 units** — 437 + 40. Of the 40, **35 are open work**, 1 is
`open-decision` and 4 are `cut`.

### Recommended next batch for area 13, ranked

| # | work | effort | why here |
|---|---|---|---|
| 1 | `MCP-500` + `MCP-501` **together** | M | `MCP-500` alone is a one-line array; landing `MCP-501` without it ships the credential-leak class the v2.26.1 retarget already caught once. The `config.rs:22-24` header comment must be corrected in the same change — it currently asserts the symbol exists at no upstream tag |
| 2 | Re-read the 163 open rows this pass did not sample | L | The sample says ~4 in 5 are stale. Every consumer of this file — scheduling, the residual ledger, `PARITY-GAPS.md` — is reading a number that is wrong by roughly 150 units. This is the highest-value item and it is pure audit, no code |
| 3 | `MCP-502` (`pi.mcp` manifests) | M | A whole config **source** the six-rung ladder cannot express; it changes `ConfigContext::sources()`, which everything else in 13b is built on. Cheaper before more 13b work than after |
| 4 | `MCP-513` (namespace-proxy tools) + `MCP-503`/`MCP-516` | L | `mcp__<server>` is the largest new model-facing surface in the delta, and `d4f7392c` already fixed the sanitisation bug in that grammar — the code is warm |
| 5 | `MCP-507` + `MCP-508` + `MCP-534` (transient 503, modern listen, bounded refresh) | L | One cluster in `server_manager.rs`; all three are lifecycle robustness and share fixtures. Read the TypeScript first — all three are table-B rows |
| 6 | `MCP-510` (runtime registration API) | M | The only table-B row that adds a **host seam**, so it collides with `MCP-037`/`MCP-041`'s unbuilt `register_late_*` surface. Worth scheduling beside them rather than alone |
| 7 | `MCP-132` + `MCP-532` (the endpoint probe) | M | `MCP-132` was `missing` before the delta and is still `missing`; `MCP-532` only extends it. One unit's worth of work for two rows |
| 8 | `MCP-013`, `MCP-085`, `MCP-108`, `MCP-287`, `MCP-485`, `MCP-493` | S each | The six confirmed-open leftovers from the sample. `MCP-493` remains the one dependency-free net add and can be written today |
| 9 | `MCP-505`/`MCP-506`/`MCP-539` (cache TTL, compact writes, shared index) | S | Small, local to `dirs.rs`/`registration.rs`, and `MCP-506` is observable by any differential against a stock adapter |
| 10 | The 13g cluster `MCP-521`…`MCP-525` | M | OAuth is the crate's strongest surface (`9cf02a66` proved it against a real server) and these are five small additions on top of a working flow |

## Update — 2026-08-21, wave 1 (MCP-141 / 142 / 146 / 370)

The census below is **as of the audit** and is not rewritten by later work; this section records what
has moved since, so the two are never in conflict.

All four units concerned one file — `crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs`, the
in-tree READER of the metadata cache, which was never upgraded when `cyrup-mcp`'s writer was. Landed:
the identity pre-image is now the writer's key set in the same *resolved* forms, `stable_stringify`
renders an absent field as the bare `undefined` (a three-state `HashValue` mirroring the writer's,
replacing `serde_json::Value`), resource tools are `read_` not `get_`, `ToolPrefix` carries upstream's
four modes, and the server prefix preserves hyphens. `cyrup-mcp` was added to that crate's
**`[dev-dependencies]` only**, so the conformance tests assert against the writer itself rather than
against constants copied out of it — drift is now impossible rather than merely detectable.

| unit | was | now | what remains |
|---|---|---|---|
| `MCP-142` | partial | **implemented** | — |
| `MCP-146` | partial | **implemented** | — |
| `MCP-141` | partial | **partial** | `socket`, the vectors and the `lenient` cluster are CLOSED (below); `ResolvedIdentity::resolve` is the writer's production constructor |
| `MCP-370` | partial | **partial** | `includeTools` and glob `excludeTools` still unported in the reader, so it over-approximates what the adapter registers |

### The four hashing divergences are CLOSED (later wave)

Every cyrup digest now equals the digest a stock `pi-mcp-adapter` @ `v2.26.1` (`fafae21`) computes
for the same definition, measured by running upstream's own `stableStringify` + `computeServerHash`
on node 22 rather than by reasoning about them. This was free to do only because
`cyrup_mcp::dirs::save_metadata_cache` still has **no production call site** — its five callers are
all tests — so no deployed digest had to be invalidated.

1. **`socket` — the missing 15th key. CLOSED.** Upstream's `computeServerHash` builds a **15**-key
   identity whose third key is `socket` (`metadata-cache.ts:89`), and its `stableStringify` walks
   `Object.keys()`, so an absent socket is still emitted as `"socket":undefined`. Neither Rust side
   emitted the key, so *every* cyrup digest differed from pi's by exactly that member. Both now emit
   `"socket": undefined` unconditionally, which is complete as well as correct: `to_server_entries`
   rejects any entry that configures a socket (MCP-054), so the value can only ever be
   `resolveConfigPath(undefined)`. Upstream's digest for the stdio fixture is
   `2190558e470a75c0f992989bd1799b374e669deecb8093e4118a1a9419068cf4`; cyrup produced `4dd46c1f…`
   and now produces upstream's, pinned by
   `the_socket_key_is_no_longer_a_divergence_from_upstream`. 13c-mcp-servers.md:1851 ("Keep `socket`
   … in the pre-image despite Cut 3") was right and is now satisfied.

2. **The `lenient` cluster — three divergences, one root cause. CLOSED.** `config.rs` read `auth`,
   `protocolVersion`, `env` and `headers` behind `deserialize_with = "lenient"`, which silently drops
   any value the Rust type rejects. Upstream hashes all four verbatim and validates none of them at
   load. `AuthMode::Other` and `ProtocolVersionSetting::Other` now carry the raw value into the
   pre-image, and `cyrup_mcp::config::StringRecord` carries `env`/`headers` (and
   `requestHeadersCommand.env`) with their **throw** — upstream's `interpolateEnvRecord` raises a
   TypeError on a non-string member and `isServerCacheValid` catches it to `false`, which the reader
   always reproduced and the writer did not: it dropped the map, hashed `"env":undefined`, and called
   the entry VALID. That one was the two crates giving *opposite* answers, not merely different
   digests. **Connect-time validation is unchanged in kind and now actually happens**:
   `Invalid MCP protocolVersion` is raised by `runtime::version_negotiation` — which is where
   `resolveVersionNegotiation` raises it — and never by the deserialiser.

3. **A fifth divergence, found by measuring the four.** `mcp_direct_tools`'s `ServerEntry` holds
   `auth` and `protocolVersion` as `Option<Value>`, and serde's derived `Option<T>` reads a JSON
   `null` as `None` — so `auth: null` hashed as `"auth":undefined` where upstream hashes
   `"auth":null` (`d5e9d0fe71ad5cc5d6a82b93d537f69ee59809f7f10e1f5c1f26c1d0a97e28e4`, node 22).
   Here the **writer** was the correct side. A `present_or_absent` deserialiser closes it. It
   surfaced only because the new differential table asserts both implementations against
   *upstream's* digest rather than against each other.

4. **The golden vectors are upstream-faithful.** Every constant in `cyrup_mcp::dirs` and in
   `mcp_direct_tools` was regenerated by running upstream's own functions on node 22 and now
   **includes `socket`**, so the word "golden" no longer carries a caveat. Each reconstruction of the
   identity literal was proved faithful by asserting
   `sha256(preImage) === computeServerHash(definition)` against upstream's exported function on the
   same run.

**What wave 1 left open and is still open.**

* **The reader resolves; so does the writer now** (`ResolvedIdentity::resolve` replaced
  `::verbatim` at every production call site), so this one is closed too. `::verbatim` survives only
  as the fixture constructor, and its doc comment says what it cannot express.
* **MCP-370's filtering half** — `includeTools` and glob `excludeTools` are hashed by both sides but
  still not *applied* by the reader, so it over-approximates what the adapter registers.

Verified: `cargo nextest run --workspace` 7697/7698 (the one failure is the pre-existing
`cyrup-modes rpc_cycle_model_spans_the_full_auth_filtered_registry`, unrelated and documented
elsewhere), `cargo clippy --workspace --all-targets` exit 0 with no new warnings in the changed file.

## Update — 2026-08-22, wave 5 (the transport and connection units)

`createConnection` has a body. `McpServerManager`'s [`ConnectionFactory`] seam — the one wave 4 left
filled with `UnbuiltConnectionFactory` — is now `cyrup_mcp::runtime::ConnectionBuilder`, and
`initialize_mcp` is where it is installed (`runtime.rs:170-186`), so a configured server connects
for real against a real child and a real HTTP server for the first time. Landed in `runtime.rs`,
`errors.rs`, `request_headers_command.rs` and (for MCP-105) `cyrup-ext`'s `npx_resolver.rs`.

**How far that reaches, stated exactly.** `initialize_mcp` has no non-test caller:
`grep -rn 'initialize_mcp(' --include=*.rs` over the repo returns its definition
(`runtime.rs:125`) and one call, `runtime.rs:403`, which is inside the `#[cfg(test)]` module opening
at `runtime.rs:325`. The production entry point that would reach it,
`McpExtension::on_session_start` (`extension.rs:279-300`), is still MCP-008/MCP-011's empty body and
calls nothing. So this wave fills the seam and makes it reachable — it does **not** yet make a
configured server connect from a live session. An earlier draft of this section said "in production
for the first time"; that was wrong and is corrected here rather than quietly dropped, because the
next reader would otherwise have taken it as evidence that MCP-008/MCP-011 were no longer on the
critical path.

| unit | was | now | what remains |
|---|---|---|---|
| `MCP-101` | partial | **implemented** | — |
| `MCP-105` | missing | **implemented** | — |
| `MCP-109` | partial | **implemented** | its own verify line's `@modelcontextprotocol/conformance` client baseline was **not run** — the transport is proven against a hand-rolled loopback fixture instead |
| `MCP-113` | implemented | **implemented** | `select_transport` now has a caller — `ConnectionBuilder::create_connection` — but that caller is only installed at `initialize_mcp`, which is itself still test-only pending MCP-008/MCP-011 in `extension.rs::on_session_start`. Its census-row note ("no production caller yet") therefore still stands |
| `MCP-114` | partial | **implemented** | four of its five verify bullets are asserted on the wire here; the `bearerTokenEnv` fallback and the `HTTP bearer token` context string were already asserted in `secrets.rs`'s own tests and are not re-asserted |
| `MCP-115` | partial | **partial** | the ladder, its arm order and the 401 predicate are done — the predicate covering a **bare** 401 as well, which it did not until review-pass item 5; the *provider* is a seam (`HttpAuthProvider`) whose production binding is section 05's, and `skipIssuerMetadataValidation` is read and consumed by nothing because rmcp's streamable-HTTP config has no such field |
| `MCP-115a` | missing | **implemented** | — |
| `MCP-128` | partial | **partial** | its connect half now lands — `request_options.timeout` bounds the handshake on both arms (review-pass item 4), where before it was built by the manager and read by nothing. The manager-side `setDefaultRequestTimeoutMs` / `getRequestOptions` half is unchanged from its census note |
| `MCP-124` | partial | **implemented** | — (this row said `implemented \| —` before the `ManagerError` half landed, and was wrong: see review-pass item 2) |

**What each one actually did.**

* **MCP-101.** `ConnectionBuilder::connect_stdio` in upstream's order: `createClient` (so an invalid
  `protocolVersion` throws before a child exists), `args.map(interpolateEnvVars)`, the abort check,
  `mkdirSync(pluginDataDir)`, `resolveConfigPath(cwd) ?? defaultCwd`, `resolveEnv`, the stderr
  drain. Six tests spawn real children and read the environment back out of them.
* **MCP-105.** `parse_package_spec` + `EXACT_PACKAGE_VERSION_RE` replace `extract_package_name`,
  threaded into both the cache-hit predicate and `find_cached_package_dir`'s version filter. The
  29-row parse table was produced by running upstream's own `parsePackageSpec` on node 22.
* **MCP-109/114.** The transport is constructed for real — `reqwest` is now a declared dependency of
  `cyrup-mcp` (no new resolution surface; rmcp already resolves onto the workspace's `0.13.4`)
  because wrapping the client is what the header-command decorator needs. `resolve_server_url` and
  `resolve_http_secrets` were already written and are now wired.
* **MCP-115.** The four-arm ladder, in upstream's order, with `crate::oauth::on_unauthorized` as
  arms 4-5. **The 401 predicate is hand-written and the plan said it should not be**: rmcp's
  `ClientInitializeError::auth_challenge()` matches `AuthRequiredError` (401) *and*
  `InsufficientScopeError` (403), while upstream's `isUnauthorizedHttpError` is 401 only — using it
  as-is would turn every scope-denied 403 into a `needs-auth`.
* **MCP-115a.** `RequestHeadersCommandClient` is built once per connect (matching
  `server-manager.ts:868-870`, which is *outside* `attempt`) and used by every attempt. **Divergence
  3 of that module is closed**: `apply_derived` now clears rmcp's `auth_header` when the derived set
  carries an `authorization`, so a bearer-configured server with a signing command sends one
  `Authorization`, the derived one. Measured before the fix as
  `["Bearer static-bearer", "Signature derived"]`.
* **MCP-124.** The five aggregate variants, their byte-exact heads, and a structural
  `is_cleanup_failure` — **and, since the review pass below, the `ManagerError` half**: an
  `Aggregate` raised by `disposeConnection`/`closeAll` is now mapped onto the matching `McpError`
  variant by `From<&ManagerError>` instead of flattened to `Other`, and `ManagerError::Display`
  routes through `errors::render_aggregate_texts`. Without those two, the type that actually reaches
  a user through `closeAll` kept neither the class nor the rendering the unit was filed to fix.

**What this wave leaves open, stated so it is not mistaken for closed.**

* **`McpError::SetupFailed`'s only producer today is a narrow race.** The arm that raises it is
  `createConnection`'s catch after a *post-handshake* step fails and the cleanup after it also
  fails. `ConnectionBuilder::post_handshake`'s own abort check is such a step, so a `close` that
  races a settled handshake **and** whose `resource.close()` then fails does raise it against a real
  server — pinned by `an_abort_whose_own_cleanup_fails_is_a_setup_failure`. What is missing is
  *upstream's* producer, discovery (MCP-119), which cannot land through this seam at all:
  `NewConnection` has no field for tools/resources/prompts and `ServerConnection::new` hardcodes
  them empty. Widening that seam is `server_manager.rs`'s change. (An earlier draft of this bullet
  said the variant had "no producer" and "cannot fire against a real server". That was too strong;
  the reachable case is narrow, not empty.)
* **`McpError::AbortCleanupFailed` and `McpError::HttpCleanupFailed` have no producer either**, for
  a different reason: `serve_client_with_lifecycle_and_ct` closes the transport on every failure
  path and reports one error, so this port has no separate cleanup outcome to observe. That is
  MCP-123's residual verbatim.
* **`McpConnection`'s `Peer` is unreachable through `ConnectionResource`.** The trait exposes
  `close`/`has_session_id`/`child_pid`/`stderr_detail` and nothing else, so nothing outside
  `runtime.rs` can issue a request on a connection the builder made. Same seam, same owner.
* **MCP-103 is still unported**, so an `npx` server's tracked child is still the npm launcher. The
  call site is marked in `connect_stdio`.
* **A failed handshake SIGKILLs its child with no graceful window.** Nothing on that path calls
  `close()`; `serve_client_with_ct_inner` drops the transport and `ChildWithCleanup::drop` spawns a
  fire-and-forget `kill()`. Upstream's catch runs `client.close()`, and the TS SDK escalates
  close-stdin → 2 s → SIGTERM → 2 s → SIGKILL. Bounded to the failed-connect path — a successful
  connection tears down through `graceful_shutdown` — and pinned by
  `a_failed_handshake_leaves_no_child_behind`, which also guards against the child leaking outright.
* **`secrets::resolve_command_secret` takes no `EnvFn`**, so `ConnectionBuilder::with_environment`'s
  environment reaches `args`, `cwd`, the URL and the bearer ladder but not `env`/`headers` values.
  In production both are `process.env` and nothing diverges; in a test they are two seams.

Verified: `cargo check --workspace --all-targets` clean; `cargo nextest run --workspace` 7850/7851,
the one failure being the pre-existing `cyrup-modes
rpc_cycle_model_spans_the_full_auth_filtered_registry`. Clippy on `cyrup-mcp`/`cyrup-ext`: 2 warning
sites, both pre-existing and both measured against the tree with this wave's block sliced out.
rustdoc warnings 34 → 34 (`cyrup-mcp`) and 39 → 39 (`cyrup-ext`).

### Wave 5 review pass — six defects found in the wave's own output, and what changed

Every item below was **measured before the fix and re-measured after**, and each carries a test that
fails on the pre-fix tree. Two of them were false statements already written into the source and the
first draft of this section; those are corrected in place rather than appended to, because a wrong
comment outlives a wrong line of code — the next reader trusts it instead of checking.

| # | defect | where | now |
|---|---|---|---|
| 1 | "`initialize_mcp` installs it, so a configured server actually connects in production" — it has no non-test caller | this section's preamble, `runtime.rs:170-186`, the `MCP-113` row | corrected above; the builder is installed **at** `initialize_mcp`, which is test-only pending MCP-008/MCP-011 |
| 2 | `MCP-124` marked `implemented \| —` while the aggregates the manager actually raises had no typed variant, rendered head-prefixed, and lost their class at the public boundary | `server_manager.rs` `ManagerError` | `From<&ManagerError>` maps `Aggregate` (and a carried `Mcp(<aggregate>)`) onto the `McpError` variants; `Display` routes through `errors::render_aggregate_texts`; the five head constants are now **re-exported** from `errors.rs` instead of redefined, so the dispatch cannot drift |
| 3 | an OAuth token and a config-supplied `Authorization` both went on the wire | `runtime.rs::http_attempt` | the configured header wins and the token is dropped, matching `_commonHeaders`' spread order. Measured before: `["Bearer from-store", "Static abc123"]` |
| 4 | `requestTimeoutMs` never reached the handshake — a server that accepts and never answers `initialize` hung `connect` forever | `runtime.rs`, both arms | `connect_client_bounded` applies `request_options.timeout`; a lapse raises upstream's byte-exact `Request timed out`. Ablation: with the budget ignored, the two `wedged` tests do not terminate |
| 5 | a bare 401 (no `WWW-Authenticate`) was a hard error instead of `needs-auth`, and the doc at the site asserted the opposite | `runtime.rs::unauthorized_challenge` | widened with `bare_unauthorized`; the 403/`InsufficientScope` exclusion is unchanged and separately pinned |
| 6 | `has_session_id` was a hardcoded `true` under a comment claiming it was a live read | `runtime.rs::http_attempt` | a real read: `SessionIdProbe` wraps the HTTP client and records the `Mcp-Session-Id` the handshake response carried. A stateless server now reads `false` and stops tripping the session-recovery gate |

**Why 3 keeps recurring, written down because it is the transferable part.** rmcp carries the bearer
in a separate `auth_header` channel from the custom-header map, and *both* channels append —
`RequestBuilder::bearer_auth` and `builder.header(name, value)`. Upstream has one `Headers` object
with `set` semantics. So parity is not the default: **every path that can produce an `Authorization`
has to clear the other channel explicitly.** Wave 2 fixed instance one in
`secrets::resolve_http_secrets` (a resolved `bearerToken` strips a configured `Authorization`),
MCP-115a fixed instance two in `request_headers_command::apply_derived`, and this pass fixed the
third. There is no reason to think a fourth producer would be born correct.

**Also landed in this pass**, from the review's minor findings:

* **`StdioTransportSpec::resolve` now runs under `spawn_blocking`.** It reaches
  `secrets::resolve_command_secret`, a `std::process::Command` spawn polled with
  `std::thread::sleep` and bounded by a 10-second timeout. Run inline it held a tokio worker for up
  to ten seconds inside the manager's single-flight connect future, where `close`/`close_all`'s
  abort could not preempt it — a guarantee wave 4 measured against `UnbuiltConnectionFactory`, which
  returned instantly and so could not have caught this. Upstream's `spawnSync` blocks node's whole
  event loop, so leaving it inline was arguable parity; it was still the one way this
  `createConnection` body could weaken a guarantee the rest of the crate relies on. Measured on a
  one-worker runtime: a 200 ms timer over a connect carrying a 1-second `!command` env value fired
  at 1.019 s inline and at 0.2 s under `spawn_blocking`.
* **`SetupFailed`'s residual restated** (see the bullet above).
* **`close_inner`'s "Blocker, stated plainly" note** at `server_manager.rs` was stale in its first
  half (`is_cleanup_failure` does match all seven now) and true in its second; both halves are
  rewritten to what the code does.

### Still open — a 401 rmcp never turns into an error at all (found by the confirming pass)

**MCP-115 / F5 is incomplete, and the gap is invisible from this crate's own code.** The bare-401
fix works for a 401 with no body. It does NOT work for a 401 carrying
`Content-Type: application/json` and a parseable JSON-RPC error, because rmcp applies its
JSON-RPC-error shortcut to **every** non-success status, not just 400:
`rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:278-293` returns
`Ok(StreamableHttpPostResponse::Json(..))` for that case, so the
`Err(UnexpectedServerResponse("HTTP {status}: {body}"))` at `:296` — which `runtime.rs:2063`
prefix-matches — is never constructed. `bare_unauthorized` cannot fix this: it is never called.

MEASURED through the real `ConnectionBuilder::connect_http_client` against a loopback fixture
answering `initialize` with `401` + a JSON-RPC error body: the connect ends as a hard failure and
the OAuth ladder is never reached. A server that answers this way — which is legal, and which the
MCP spec's own error shape encourages — can never authenticate.

Fix shape: catch the status before rmcp collapses it, in the client-decorator seam this crate
already occupies (`SessionIdProbe` / `RequestHeadersCommandClient`), raising the unauthorized shape
whenever the response was HTTP 401 regardless of body; or carry the status out of the decorator into
the ladder. The ladder tests need a `json_rpc_body` mode on `FixtureOptions` alongside
`challenge: false` — the fixture's inability to produce this shape is exactly why it went unseen.

Not fixed here because it is a second, distinct mechanism from the one F5 addressed and wants its
own measured pass. It fails SAFE (a hard connect error, never a wrongly-authenticated request).

### Still open after the review pass — items outside this unit's files

Each was measured and is recorded here so it is not lost; none is fixed, because each lives in a
file this unit does not own.

* **`config.rs:618-621`'s cross-reference is dangling.** `StringRecord`'s `Deserialize` doc defers a
  residual to "`13c-mcp-servers.md`'s MCP-144 notes"; that block records only that
  `interpolate_env_record` drops non-string values, and says nothing about the non-object-`env`
  case. Measured on node 22 @ v2.26.1: `computeServerHash({command:"x",env:"abc"})` =
  `01ed7340…`, the writer produces `f0211144…` (upstream's digest for the same definition with `env`
  **absent**). Same family, also unrecorded: `env: []`, `env: 5`, `env: true` all hash as `{}`
  upstream (`1d224401…`) and as absent here. The same doc calls this "a fifth" divergence while this
  file and the wave report call `auth: null` the fifth and this the sixth. Fix: record it in 13c's
  MCP-144 block, or repoint `config.rs:621`.
* **That residual is described as writer-only and is not.** The **writer** degrades to `None`; the
  **reader** drops the whole server from the direct-tool surface —
  `mcp_direct_tools.rs::extract_server_map` skips any entry `serde_json::from_value::<ServerEntry>`
  rejects, and `env: Option<BTreeMap<String, Value>>` rejects a string, array, number or bool.
  Measured over six definitions, the reader keeps three where upstream keeps six. `args: [1,"b"]`
  and `command: 5` behave identically — one root cause (typed reader fields with no `lenient`
  equivalent), not three items.
* **`StringRecord` opened an unnamed connect-path divergence.** `secrets.rs:386` passes
  `entry.env.as_deref()`, which `Deref`s to the string members only, so
  `env: {"GOOD":"1","BAD":5}` now spawns the child with `GOOD=1`; before the retype `lenient`
  dropped the whole block and it spawned with none. Upstream does neither — measured,
  `resolveCommandSecretsRecord({GOOD:"1",BAD:5}, …)` throws `value.startsWith is not a function`
  and refuses the connect. The hash side is correct on both crates. Fix: route
  `resolve_stdio_env`/`resolve_http_secrets` through `StringRecord::unhashable()`, or name the
  divergence in `StringRecord`'s doc.
* **`registration.rs:792` and `:865-866` are stale.** Both say the hasher's `None` "has exactly one
  source" (`resolve_server_url`); `ResolvedIdentity::resolve` has had a second `Err` arm since the
  hashing wave, and `dirs.rs:1082-1085` already says "**two**". Behaviour is correct — `Option::ok()`
  swallows both — so this is docs only, in a crate whose house style is that comments carry the
  specification. `registration.rs` is untouched by wave 5.
* **The same class loss still applies to `McpError::CredentialStore` across `ManagerError`.**
  `From<&ManagerError>` now rebuilds the aggregates and keeps `Aborted`/`Config`/`Server`, but a
  credential-store failure raised inside the factory (`ConnectionBuilder::connect_http_client`'s
  `self.auth.authorize(..)?`) still arrives as `McpError::Other` and
  `is_credential_store_failure()` answers `false` for it. It cannot be fixed the same way:
  `AuthStoreError` is `#[non_exhaustive]` and not `Clone`, so the one-way `&ManagerError ->
  McpError` door cannot reconstruct it, and the type lives in `credentials.rs`. The class matters
  for the same reason the doc on the variant gives — section 07's refresh driver rethrows a store
  failure and swallows everything else — but no consumer of `is_credential_store_failure` currently
  sits downstream of this conversion, so it is a latent hazard rather than a live bug. Fix shape:
  make `AuthStoreError` `Clone`, or give `ManagerError` a `CredentialStore` arm.
* **`13c-mcp-servers.md:1208-1211`'s MCP-100 attribution is wrong.** It says
  `MCP connection for <name> was closed while connecting` is reachable "when the generation advanced
  **without** the attempt being aborted (what `reconnect`/`closeAll` can produce)". Upstream writes
  `closeGenerations` at exactly two places, `server-manager.ts:1098` and `:1146`, and **both** abort
  the attempt controller on the next line; `reconnect` never touches it (`doReconnect` delegates to
  `this.close(name)`, which aborts). The only window is between `connect`'s generation read at
  `:279` and its `connectAttempts.set` at `:286`. The measured half of that bullet is sound.

Verified after the review pass: `cargo check --workspace --all-targets` clean;
`cargo nextest run --workspace --no-fail-fast` 7858/7859, the one failure still the pre-existing
`cyrup-modes rpc_cycle_model_spans_the_full_auth_filtered_registry`; `cyrup-mcp` alone 612/612.
Clippy on `cyrup-mcp`: 2 diagnostics, both pre-existing (`dirs.rs:1863`'s empty line after a doc
comment, and `result_large_err` on `connect_client`, whose `ClientInitializeError` is returned
unflattened on purpose — see its doc). rustdoc warnings for `cyrup-mcp` 34 → 33.

Every fix in this pass is pinned by a test that fails on the pre-fix tree. The ablations, run one at
a time:

| fix | ablation | result |
|---|---|---|
| duplicate `Authorization` | restore `if config.auth_header.is_none()` | `an_oauth_token_never_joins_a_configured_authorization_header` fails with `left: ["Bearer from-store", "Static abc123"]` |
| handshake timeout | ignore the budget in `connect_client_bounded` | both `wedged` tests never terminate (`timeout 90` kills the run, exit 124) |
| bare 401 | drop the `bare_unauthorized` arm | `a_bare_401_with_no_challenge_still_reaches_the_oauth_ladder` fails |
| `has_session_id` | restore the hardcoded `true` | `a_stateless_http_server_reports_no_session_id` fails |
| MCP-124 rendering | restore head-prefixed `ManagerError::Display` | `close_all_aggregates_only_cleanup_failures` fails with `"MCP manager cleanup failed: MCP connection cleanup failed: client close failed"` |
| MCP-124 class | drop the `Aggregate` / `Mcp(<aggregate>)` arms of `From<&ManagerError>` | `close_rethrows_a_pending_connects_setup_failure_and_swallows_everything_else` fails with `Other("connect ECONNREFUSED: transport close failed")` |
| `spawn_blocking` | inline `StdioTransportSpec::resolve` | `a_slow_env_command_does_not_hold_the_worker_carrying_the_connect` fails: the 200 ms timer fires at 1.019 s |

## Census

> **STALE — dated 2026-08-21, and internally inconsistent.** Its 212/100 disagrees with the unit
> table it summarises (214/98), which had wave 1's `MCP-142`/`MCP-146` applied to the rows and not to
> the total. Use *The census, re-derived* in the 2026-09-04 re-audit block above.

| status | units | meaning |
|---|---:|---|
| `implemented` | 212 | the unit's obligations are met in the Rust |
| `partial` | 100 | lands, but a named obligation is unmet |
| `missing` | 98 | no implementation found |
| `not-applicable` | 27 | `cut` or `open-decision` — not work |
| **total** | **437** | |

**198 units carry open work** (98 missing + 100 partial). By the plan's own severity:

| severity | open | of total | 
|---|---:|---:|
| critical | 8 | 22 |
| high | 73 | 147 |
| medium | 91 | 172 |
| low | 24 | 60 |
| n/a | 2 | 35 |

### By section

> **STALE — dated 2026-08-21**, for the same reason as the census above it.

| § | missing | partial | open | units | critical+high open |
|---|---:|---:|---:|---:|---:|
| [`13a-mcp-activation.md`](13a-mcp-activation.md) | 17 | 22 | 39 | 51 | 10 |
| [`13b-mcp-config.md`](13b-mcp-config.md) | 6 | 13 | 19 | 51 | 9 |
| [`13c-mcp-servers.md`](13c-mcp-servers.md) | 20 | 20 | 40 | 51 | 23 |
| [`13d-mcp-proxy-modes.md`](13d-mcp-proxy-modes.md) | 1 | 4 | 5 | 36 | 3 |
| [`13e-mcp-tools.md`](13e-mcp-tools.md) | 7 | 8 | 15 | 53 | 7 |
| [`13f-mcp-credentials.md`](13f-mcp-credentials.md) | 0 | 5 | 5 | 41 | 1 |
| [`13g-mcp-oauth.md`](13g-mcp-oauth.md) | 1 | 8 | 9 | 49 | 2 |
| [`13h-mcp-tui.md`](13h-mcp-tui.md) | 15 | 9 | 24 | 55 | 10 |
| [`13i-mcp-protocol-and-verification.md`](13i-mcp-protocol-and-verification.md) | 31 | 11 | 42 | 50 | 16 |

The shape of that table is the finding. **`13i` (protocol and verification) is the weakest surface** —
31 of its 50 units have no implementation at all — and **`13c` (servers, transports, metadata cache)
carries the most critical-or-high open work (23)**. `13f` (credentials) is the strongest: nothing
missing, five partials, one of them high.

## Critical-severity open work

> **STALE — all eight are closed at HEAD.** See *The eight `critical` open units are all closed* in
> the 2026-09-04 re-audit block above for the citation on each. `MCP-500` is the one `critical` unit
> open in area 13 today, and it is new.

Eight of the plan's 22 `critical` units are open. None is a clean greenfield gap; every one is a
divergence inside something that already exists, which is why they read as `partial`.

| id | status | § | the unmet obligation |
|---|---|---|---|
| `MCP-083` | partial | 13b | Two obligations unmet. (1) `resolveCommandSecretsRecord` — the per-record form applied to `env` and `headers` — does not exist; grep over crates/cyrup-mcp/src for `resolve_command_secrets_record` / `interpolate_env_record` returns nothing, and … |
| `MCP-141` | partial | 13c | Three gaps, each independently fatal to the contract. (1) **The reader was not upgraded.** `cyrup_ext_subagents::exec::mcp_direct_tools::compute_mcp_server_hash` (/home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:531-584) still hashes … |
| `MCP-142` | partial | 13c | The **reader still emits `null`**: `mcp_direct_tools::stable_stringify` (/home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:796-825) maps `Value::Null => "null"`, and every absent field is materialised as `Value::Null` by `opt_str_value` … |
| `MCP-146` | partial | 13c | The reader was **not** changed: `cyrup_ext_subagents::exec::mcp_direct_tools::resolve_direct_tool_names` still builds `format!("get_{}", resource_name_to_tool_name(name))` at /home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:466, and … |
| `MCP-232` | partial | 13e | The gate itself is unimplemented — `ensure_tool_call_approved` exists only as a `ProxyEnv` trait method (proxy.rs:1488) with a test-only implementor. Missing: the cache lookup/insert against `approved_tool_calls`, the headless check performed **before** … |
| `MCP-370` | partial | 13h | The other half of option (a) — upgrading the in-tree consumer in the same change — has NOT been done. /home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs still has a 3-variant `enum ToolPrefix { Server, None, Short }` (line 45-49), … |
| `MCP-394` | partial | 13h | The orchestration is explicitly outstanding — `TODO(MCP-394)` at ui.rs:4781-4786. Absent: the `programmaticConfig` branch (notify `MCP status is shown from the in-memory SDK config; configuration discovery is unavailable.` + `showStatus`), the … |
| `MCP-455` | missing | 13i | Missing: `confirmSampling`'s three-branch gate (auto-approve short-circuit; explicit `has_ui: bool` sourced from the host config producing the distinct "MCP sampling requires interactive approval. Set settings.samplingAutoApprove to true to allow it without … |

## High-severity open work

> **STALE — dated 2026-08-21.** The 2026-09-04 sample re-ruled **eleven** `high` units to
> `implemented` — `MCP-025`, `MCP-075`, `MCP-092`, `MCP-119`, `MCP-126`, `MCP-139`, `MCP-164`,
> `MCP-207`, `MCP-217`, `MCP-390`, `MCP-452` — and did not open the other 62. No `high` unit it
> re-checked stayed open; the seven that did are `MCP-085` and `MCP-132`, `MCP-287`, `MCP-485`
> (medium) and `MCP-013`, `MCP-108`, `MCP-493` (low). The new
> `MCP-501`, `MCP-502`, `MCP-504`, `MCP-507`, `MCP-508`, `MCP-510`, `MCP-513` and `MCP-515` are
> `high` and are not counted in the 73.

73 of 147 `high` units are open.

| id | status | § | the unmet obligation |
|---|---|---|---|
| `MCP-008` | partial | 13a | Three obligations unmet in the handler itself. (1) It never calls `shutdown_previous_generation`/`shutdown_state` — `_previous_state` is bound to `_` and dropped, so the previous generation's status … |
| `MCP-009` | partial | 13a | The snapshotted state is discarded (`let _state = …`) instead of being passed to `lifecycle::shutdown_state`, so the shutdown-time status snapshot, the metadata flush and the flush-error-wins rule … |
| `MCP-010` | partial | 13a | Not wired: `shutdown_state` has no production caller (grep across `crates/cyrup-mcp/src` finds it only in lifecycle.rs's own definition, `shutdown_previous_generation`, and tests). And the only … |
| `MCP-011` | missing | 13a | Everything in this unit: the triple staleness check (`owner.is_active()` && `generation == my_gen` && `Arc::ptr_eq(init_task, promise)`), the stale-state teardown with `MCP: failed to clean stale … |
| `MCP-014` | partial | 13a | Two halves unproven/unbuilt. (1) The `SessionStart` rebuild half is empty (see MCP-008/MCP-011), so nothing that a replacement is supposed to preserve across generations is exercised. (2) The unit's … |
| `MCP-023` | missing | 13a | §12 in full: pass one building `startupKnownMetadata: Map<server, ToolMetadata[]>` over every successful connection (tools plus, when `exposeResources !== false`, `read_<resource>` entries with their … |
| `MCP-025` | partial | 13a | The notification half is entirely absent. Grep for `servers connected`, `tools skipped`, `Failed to connect to {name}` (the startup form) across `crates/cyrup-mcp/src` returns nothing: the … |
| `MCP-029` | partial | 13a | `updateMetadataCache(state, serverName, {preserveEmptyResources})` itself does not exist — grep for `preserve_empty`, `preserveEmptyResources`, `prompt_discovery_failed`, `serialize_tools`, … |
| `MCP-037` | missing | 13a | Build one of the two shapes: (i) a defaulted `NativeExtension::set_ext_host(&self, host: Weak<ExtensionHost>)` called from `ExtensionHost::load_native_with_services` beside `set_host_services`, plus … |
| `MCP-043` | partial | 13a | The two are disjoint types and the model reaches only the inert one. `proxy::McpTool::new` is constructed **only in tests** (all hits proxy.rs:5497-5584 are inside `#[cfg(test)]`); the tool actually … |
| `MCP-068` | partial | 13b | Three obligations unmet. (1) `MCP_UI_DEBUG` has no reader — the string does not appear anywhere in crates/cyrup-mcp/src (grep over src/*.rs: zero hits), so the logger level bootstrap does not exist. … |
| `MCP-070` | partial | 13b | Three things stop this from being a working contract. (1) Every production caller hashes UNRESOLVED values: src/ui.rs:1758 uses `ResolvedIdentity::verbatim(definition)`, whose own doc at … |
| `MCP-073` | missing | 13b | A `pub fn resolve_server_from_tool_name(tool_name, server_names, prefix) -> Option<String>` on `cyrup-mcp`: `None` for `ToolPrefix::None`; collect every configured server whose non-empty … |
| `MCP-075` | partial | 13b | The second copy is wrong. src/proxy.rs:486 `format_legacy_tool_name` derives the legacy prefix by taking `get_server_prefix(server_name, prefix)` — which has ALREADY applied the … |
| `MCP-076` | partial | 13b | src/registration.rs:334 compiles with bare `Regex::new(&out).ok()`, without the `RegexBuilder::size_limit` / `dfa_size_limit` ceilings the unit explicitly requires. The proxy copy does set them … |
| `MCP-084` | partial | 13b | `resolveServerUrl` is not implemented anywhere. Grep over crates/cyrup-mcp/src for each of its three exact strings — `MCP server URL must be a string`, `Missing environment variable`, `Invalid MCP … |
| `MCP-092` | missing | 13b | The whole dialect gate: `schemaDialect(schema)` (no string `$schema` ⇒ unstamped; else strip ONE trailing `#`), routing unstamped and `https://json-schema.org/draft/2020-12/schema` to a 2020-12 … |
| `MCP-094` | missing | 13b | One scheduled change to crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs covering all nine indexed divergences above, plus the shared conformance suite the unit names: one golden `mcp.json` + … |
| `MCP-100` | missing | 13c | The entire manager is unbuilt. Absent: `ServerConnection` (client/transport/definition/tools/resources/prompts/promptDiscoveryFailed/instructions/lastUsedAt/inFlight/status/credentialsInvalidated); … |
| `MCP-101` | partial | 13c | No `resolveEnv` and no connection builder. Specifically absent: (a) `args = (definition.args ?? []).map(interpolateEnvVars)` — grep for any arg-interpolation call site returns nothing; (b) … |
| `MCP-105` | missing | 13c | Add `ParsedPackageSpec { package_name, exact_version: Option<String> }` and `parse_package_spec` (the `@scope/name` `rfind('@') > find('/')` rule, `^=` then case-insensitive `^v` strip, validated … |
| `MCP-109` | partial | 13c | Nothing constructs the transport in production: `build_http_transport_config` and `http_transport_with_client` have only test callers (runtime.rs:1733/1756) and a doc reference from … |
| `MCP-114` | partial | 13c | The `connectHttpClient` pre-flight (§3.4 steps 1-7) does not exist. Specifically absent, verified by grepping all 19 files: (a) `resolveServerUrl` — no `resolve_server_url` symbol, and neither of its … |
| `MCP-115` | partial | 13c | The attempt loop itself is absent — there is no `connect_http_client` anywhere in the crate. Missing: the per-attempt fresh client; the transport-options assembly including … |
| `MCP-115a` | missing | 13c | Two concrete work items. (1) The wiring: build `RequestHeadersCommandClient::new(client, cfg, ct)?` **inside** MCP-115's retry closure and pass it to `http_transport_with_client(..., config)`, so it … |
| `MCP-116` | missing | 13c | No connection record carries `credentials_invalidated`, so `connect`'s step-7 carry-forward (`existing?.status == "needs-auth" && existing.credentialsInvalidated === true`) does not exist, nor does … |
| `MCP-119` | missing | 13c | No discovery at all. Needed: the unconditional `list_all_tools` with errors propagating; the `resources`/`prompts` capability gate read from `RunningService::peer_info() -> … |
| `MCP-124` | partial | 13c | Add the five variants (`AbortCleanupFailed`, `SetupFailed`, `HttpCleanupFailed`, `ConnectionCleanupFailed`, `ManagerCleanupFailed`) with `Display` rendering the byte-exact heads `MCP connection abort … |
| `MCP-125` | missing | 13c | Missing entirely: the disabled and stopped guards firing **before** the single-flight map is consulted (with `MCP server "<n>" is disabled` / `MCP server manager is closed` — neither string exists in … |
| `MCP-126` | missing | 13c | Nothing of §3.12 exists: the generation bump, the `connect_attempts[name].cancel()` with the `MCP connection <n> was closed` reason, removal from the map **before** awaiting cleanup, the … |
| `MCP-131` | partial | 13c | Nothing in production ever owns or closes a `TokioChildProcess`: `spawn_stdio_transport` has only test callers, `ManagerSupervisor::close`/`close_all` are no-ops (lifecycle.rs:307-322), and grep … |
| `MCP-134` | missing | 13c | The predicate does not exist. Needed: the absolute `hadSessionId` gate captured **before** the call; the 404 arm (which `StreamableHttpError::SessionExpired` supplies once an HTTP transport exists); … |
| `MCP-135` | missing | 13c | The whole wrapper is absent: the disabled/not-connected preconditions, `hadSessionId` captured before the call, the **live** config re-read after the failure, the 401 credential-cache invalidation … |
| `MCP-139` | partial | 13c | Three real gaps. (1) **The agent-dir consolidation did not happen.** `npx_resolver::agent_dir` (/home/user/cyrup/crates/cyrup-ext/src/caps/proc/npx_resolver.rs, anchored on … |
| `MCP-140` | partial | 13c | The **serialisers are absent**. Nothing converts a live MCP tool/resource/prompt list into `CachedTool`/`CachedResource`/`CachedPrompt`: grepping all 19 files for a `ServerCacheEntry {` construction … |
| `MCP-143` | partial | 13c | Both in-tree copies the unit names are unchanged. (1) `cyrup_ext::caps::proc::interpolate_env_vars_with` (/home/user/cyrup/crates/cyrup-ext/src/caps/proc.rs:148-156) is still `interpolate_braces` … |
| `MCP-144` | partial | 13c | Two call sites still bypass it. (1) `mcp_direct_tools::interpolate_env_record` (/home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:696-712) calls plain `interpolate_env_vars` … |
| `MCP-145` | partial | 13c | The throw arm is unreachable in practice: no fallible hasher is ever installed (`install_server_hasher` at registration.rs:754 has no production caller, and registration.rs:746-752 documents that … |
| `MCP-164` | partial | 13d | The rmcp invocation itself does not exist. `ProxyEnv::call_tool` (proxy.rs:1397) and `ProxyEnv::read_resource` (:1410) are declared with doc comments naming `Peer::send_request_with_option(...)` → … |
| `MCP-191` | partial | 13d | Neither of the unit's deliverables exists. (1) The hazard is undocumented: a grep for `MCP-191` across crates/cyrup-mcp/src returns zero hits — the only unit id in the section with no reference in … |
| `MCP-196` | partial | 13d | Not the named 46-ported + 1-re-specified target, and the expensive third of the suite is absent. Concretely missing, by upstream case name: proxy-modes-auto-auth's "runs URL elicitations returned by … |
| `MCP-207` | missing | 13e | The whole live `tools`+`resources` → `Vec<ToolMetadata>` pipeline is absent: no `failedTools` accumulation for unnamed tools, no post-visibility `seenNames` reservation, no `description ?? ""`, no … |
| `MCP-214` | missing | 13e | None of §7's ordered state machine exists: no owned-signal composition, no `lazyConnect`, no auto-auth-on-`needs-auth`, no connection assertion, no approval call, no request options, no `tools/call` … |
| `MCP-214a` | partial | 13e | Nothing wires either into a direct-tool call, because the executor does not exist (MCP-214). `ProxyEnv::call_tool` / `read_resource` (proxy.rs:1397, 1410) take a recovery callback but have no … |
| `MCP-217` | missing | 13e | No fingerprint-diff `syncDirectTools`, no `deactivateTools` fallback pass, no `syncProxyTool` description refresh at runtime and no `syncToolSurface` entry point. The state slots exist unused on … |
| `MCP-231` | missing | 13e | The whole predicate is unwritten: server-level `approveTools` overriding the global on presence, `true` → always required, non-array/empty → not required, the legacy-alias disambiguation reusing … |
| `MCP-249` | partial | 13e | Two gaps. (1) `server_unavailable` — emitted by upstream direct-tools.ts step 7 (`details: { error: "server_unavailable", server }`, verified at … |
| `MCP-260` | partial | 13f | Two items. (1) `crates/cyrup/src/mcp_keyring_helper_cmd.rs` does not exist: no `SUBCOMMAND`/`is_selected(argv)`/`dispatch()` triple, no `pub mod` in crates/cyrup/src/lib.rs, and no pre-dispatch in … |
| `MCP-324` | partial | 13g | The credential-store rethrow is a STRING-PREFIX test, not a structural one: oauth.rs:3706 does `error.to_string().starts_with(CREDENTIAL_STORE_PREFIX)` where `CREDENTIAL_STORE_PREFIX = "credential … |
| `MCP-326` | partial | 13g | An external abort does NOT reject with the identical reason value. `AuthenticateOptions::combined_signal` (oauth.rs:2618) builds a bare `CancelToken` via `crate::abort::combine`, which carries no … |
| `MCP-381` | missing | 13h | Everything in §4.1–§4.2 is absent: the owner-fenced prologue (capture `currentOwner` + a bound reload, build the synthetic `commandCtx` before the first await, await `initPromise` with the two … |
| `MCP-386` | missing | 13h | All ten steps of §4.7 are absent: the unknown-target guard, the sequential (`for … await`, not a join) all-servers loop, `manager.close` → `connect` with the two `throwIfInactive` checks, the … |
| `MCP-387` | missing | 13h | Absent: the `programmaticConfig` refusal (`MCP setup is unavailable when config is supplied by createMcpAdapter().`), the once-only computation of `discovery = getMcpDiscoverySummary(...)` and … |
| `MCP-388` | missing | 13h | All four steps of §4.9 are absent, including the load-bearing string `OAuth credentials were cleared for "{name}", but its connection could not be closed: {msg}` that distinguishes "credentials gone, … |
| `MCP-390` | partial | 13h | The command-level flow is explicitly outstanding — `TODO(MCP-334)` at oauth.rs:3922-3929 says so. Absent: the `/mcp-auth` handler itself (no `execute_command`, MCP-381); `terminalHyperlink`'s OSC-8 … |
| `MCP-392` | missing | 13h | The whole of §4.11's `buildMcpPanelCallbacks` is absent: the per-open `authStatusFailures: Map<String,String>` (deliberately NOT session state), the eight-rung `getConnectionStatus` ladder … |
| `MCP-395` | partial | 13h | The live half has nothing to land on and none of the three additions have been made: grep across /home/user/cyrup/crates/cyrup-ext/src, /home/user/cyrup/crates/cyrup-session-svc/src and … |
| `MCP-398` | missing | 13h | All nine steps of §5.5 are absent: the `MCP not initialized` guard, the `promptMetadataLive`-guarded staleness check BEFORE `lazyConnect` (the guard that stops a cache-only command being refused … |
| `MCP-450` | missing | 13i | The whole 12-step `handleSamplingRequest` free function is absent: no `SamplingOptions` bag, no `handle_sampling_request(&SamplingOptions, CreateMessageRequestParams) -> Result<CreateMessageResult, … |
| `MCP-452` | missing | 13i | Missing entirely: `fn sampling_candidates(available, hints, current) -> Vec<Model>` with hint-order-major / registry-order-minor appending, lowercase substring `.contains()` matching over … |
| `MCP-453` | missing | 13i | Missing: the direct `cyrup_provider` completion call with `{systemPrompt?, messages}` and `{apiKey?, headers?, maxTokens, temperature?, metadata?, cancel}`; `max_tokens` passed through … |
| `MCP-458` | missing | 13i | Missing: a sampling options bag holding two live closures over the stashed `Arc<dyn HostServices>` — `current_model()` read live, and a cancellation source composed as a child `CancellationToken` — … |
| `MCP-461` | missing | 13i | Missing in full: the `MCP Input Request\nServer: …` gate dialog with `["Continue","Decline"]` and `None`→cancel; the `properties.len() == 0` → `{action:"accept", content:{}}` short-circuit before any … |
| `MCP-464` | missing | 13i | Missing: the whole coercion core — 13 distinct message templates across 15 throw sites over `PrimitiveSchemaDefinition`'s typed limit fields (`StringSchema::{min_length,max_length}`, … |
| `MCP-465` | missing | 13i | Missing: compiling the original `requested_schema` with `jsonschema` + `.should_validate_formats(true)`, running it over the coerced `output`, and throwing `Invalid elicitation response: {err}`. Also … |
| `MCP-467` | missing | 13i | Missing the whole handler: the `!allow_url` gate, `url::Url::parse` failure and the http/https scheme allowlist — all three as `ErrorData::invalid_params` (-32602); the exact 9-line confirmation … |
| `MCP-471` | missing | 13i | Missing: taking a `#[must_use]` `HostCtx::begin_human_wait()` guard and the session-scoped `HostServices::human_interaction_lock` across every `select`/`input`/`confirm` in `cyrup-mcp` — the two … |
| `MCP-474` | missing | 13i | Missing: the keyword guard `\b(?:token\|secret\|password\|passwd\|api[_-]?key\|authorization\|cookie)\b` (case-insensitive) returning `"[REDACTED]"`; the three replacements (URL scheme, … |
| `MCP-483` | missing | 13i | Missing: adopting `@modelcontextprotocol/conformance` (pinned to rmcp's `0.2.0-alpha.10`, per the docket, not upstream's 0.1.16) as the port's protocol gate, run for both `--spec-version` values, … |
| `MCP-484` | missing | 13i | Missing the whole driver: scenario allowlist with non-zero exit on an unknown scenario; the scripted elicitation UI with preference order `["Use default","Submit","Continue"]` then `options[0]`; … |
| `MCP-490` | partial | 13i | Section 13i's own share is entirely absent — zero tests for sampling, elicitation or tracing, because none of that code exists (MCP-450..MCP-481). Also missing: the case-count parity metric the unit … |
| `MCP-492` | partial | 13i | REFUTED on its central assertion. The claim says 'three of the four surviving upstream files have no port'; all three do have substantial ports, they are just not named after the .ts files. (a) … |
| `MCP-496` | missing | 13i | Missing: pty infrastructure (none in the workspace) plus a driven run of the full elicitation sequence — gate → one dialog per widget kind → 20-option multi-select with `✓ ` toggle state → review → … |

## Every unit, by section

`implemented` rows carry the evidence that settled them; open rows carry the unmet obligation.

> **Re-audit 2026-09-14 (verified rulings, adversarially refuted where they did not survive).**
> 25 rows below carry a dated re-read at `9aeba769` against `pi-mcp-adapter` v2.32.1/v2.33.0. Nine
> moved status — `MCP-076`, `MCP-084`, `MCP-109`, `MCP-114`, `MCP-115a`, `MCP-208`, `MCP-214a`,
> `MCP-224`, `MCP-225` to **implemented** on refuted obligations, and `MCP-232` **implemented →
> partial** (severity `critical` → `high`) because the v2.33.0 approval cache key grew a
> `definitionHash` component cyrup does not compute. `MCP-079` stays open at `medium` → `high`.
> The rest are re-reads that still hold, several of which restate the obligation at the current tag.
>
> **The per-section `— N units, N missing, N partial` lines below are derived and are STALE — they
> were already stale before this pass** (13a stated 17 missing / 22 partial against 15 / 18 actual;
> 13e stated 53 units against 51), and nine of them are now staler still. Derived from the rows at
> 2026-09-14: 13a 51/15/18, 13b 51/5/9, 13c 51/16/14, 13d 36/1/3, 13e 51/5/4, 13f 41/0/5,
> 13g 49/1/7, 13h 55/13/6, 13i 50/27/9 (units / missing / partial). **The rows, not these header
> lines, are the authority**; `docs/gap-analysis/scripts/count_open_items.py` does not parse this
> file, so nothing derives them automatically yet.

### 13a · Activation, lifecycle and the host seam

[`13a-mcp-activation.md`](13a-mcp-activation.md) — 51 units, 17 missing, 22 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-001` | n/a | `hand-written` | **implemented** | Stand up `crates/cyrup-mcp` and attach it at the session-build arms | None for this unit. Noted only: three of `McpState`'s collaborator types are still forward declarations owned by other sections — … |
| `MCP-002` | low | `host-verb` | **implemented** | Read `--mcp-config` from argv directly, and register the flag for `--help` | `config::config_path_from_argv` (config.rs:1506) scans for the exact token `--mcp-config` and returns the *next* element only; … |
| `MCP-003` | critical | `host-verb` | **implemented** | Register the entire tool/command surface from disk caches inside `init()`, and … | `registration::register_surface(api, dirs, config) -> RegisteredSurface` (registration.rs:1858) is `pub fn`, has no `?` and no `Err` path: it … |
| `MCP-004` | high | `hand-written` | **implemented** | Port `McpRuntimeOwner` | `owner::McpRuntimeOwner` (owner.rs:68) over a `CancelToken` with `token()`, `is_active()`, `add_cleanup`, `begin_stop`/`stop` (memoised through a … |
| `MCP-005` | medium | `hand-written` | **implemented** | Reverse-order cleanup, the aggregate error, and the late-cleanup path | `McpRuntimeOwner::begin_stop` drains `cleanups.lock().drain(..).rev()`, invokes every closure at call time, joins with `futures::future::join_all`, … |
| `MCP-006` | medium | `extension-owned` | **implemented** | Port `createOwnedUi` as a fenced services handle | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: Two obligations unmet. (1) Coverage: the `fenced!` list carries 31 methods (notify, set_status, set_widget, set_working_message, confirm, input, select, oauth_prompt, oauth_select, editor, custom, open_overlay, theme, session_id, session_file, current_model, models, context_usage, is_idle, … |
| `MCP-007` | medium | `hand-written` | **implemented** | Port the abort helpers (combineAbortSignals, isAbortError, throwIfAborted, … | `abort::combine(owner, other)` returns `owner.clone()` for `None` and otherwise spawns one `tokio::select!` joiner over a fresh child token … |
| `MCP-008` | high | `hand-written` | **implemented** | The `session_start` generation protocol, abort-before-await | Three obligations unmet in the handler itself. (1) It never calls `shutdown_previous_generation`/`shutdown_state` — `_previous_state` is bound to `_` and dropped, so the previous generation's status snapshot and metadata flush never run. (2) It never builds the new owner or the new OAuth runtime … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `extension.rs` `on_session_start`: generation bump, previous slots taken, new owner/OAuth runtime published before the drain, `lifecycle.rs` `shutdown_previous_generation` (sync `begin_stop` + `shutdown_state` + `shutdown_oauth`), post-await re-check, then `start_initialization`. Upstream: `index.ts:1073-1152`. |
| `MCP-009` | high | `hand-written` | **implemented** | The `session_shutdown` handler | The snapshotted state is discarded (`let _state = …`) instead of being passed to `lifecycle::shutdown_state`, so the shutdown-time status snapshot, the metadata flush and the flush-error-wins rule never execute; and the OAuth runtime is not explicitly shut down (it is only reached indirectly if … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `extension.rs` `on_session_shutdown` → `shutdown_previous_generation(…, live::metadata_flush(dirs))`. Upstream: `index.ts:1195-1220`. |
| `MCP-010` | high | `hand-written` | **implemented** | `shutdownState`, preserving the metadata-flush error | Not wired: `shutdown_state` has no production caller (grep across `crates/cyrup-mcp/src` finds it only in lifecycle.rs's own definition, `shutdown_previous_generation`, and tests). And the only `MetadataFlush` implementation in the tree is `lifecycle::no_metadata_flush()` (lifecycle.rs:386), which … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `lifecycle.rs` `shutdown_state` (empty snapshot, flush captured, `begin_stop`, flush error wins); production callers `shutdown_previous_generation` and `commit_initialization`'s stale arm; `live.rs` `metadata_flush` replaces `no_metadata_flush`. Upstream: `index.ts:271-309`. |
| `MCP-011` | high | `hand-written` | **implemented** | `startInitialization`'s triple staleness check and metadata-update hook install | Everything in this unit: the triple staleness check (`owner.is_active()` && `generation == my_gen` && `Arc::ptr_eq(init_task, promise)`), the stale-state teardown with `MCP: failed to clean stale initialization state: …`, the commit into the `state` slot, the `onToolMetadataUpdated` hook install … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `extension.rs` `commit_initialization`: triple check (`owner.is_active` / generation / `init_task_is`), stale teardown logging `MCP: failed to clean stale initialization state`, state commit, `install_runtime_env`, dispatch install, `install_surface_sync`, `sync_tool_surface`, `update_status_bar`, freeze latch; `fail_initialization` is the `.catch` tail. Upstream: `index.ts:849-1025`. Upstream's additions since — approval restore (`MCP-559`), runtime servers (`MCP-510`), retained init failure (`MCP-511`), the `a462b30` finalization guard (JS re-entrancy, not owed) — are owned elsewhere. |
| `MCP-012` | medium | `extension-owned` | **partial** | `startLoadTimeInitialization` — the eager/keep-alive pre-warm | The pre-warm itself is absent. `extension::init` calls the gate and then only emits `tracing::debug!("MCP: eager/keep-alive servers configured — pre-warm pending")` (extension.rs:527-530): there is no `tokio::spawn`, no generation re-check at the top of the task, no synthetic … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `extension.rs` `init` still ends in `tracing::debug!("MCP: eager/keep-alive servers configured — pre-warm pending")`; no spawned load-time build. Upstream: `index.ts:1040-1057`, called at `:2027`. |
| `MCP-013` | low | `hand-written` | **partial** | The `MCP_DIRECT_TOOLS` blocking wait at session start | The blocking wait is not implemented. `extension::McpExtension::on_session_start` (extension.rs:282-305) contains no `MCP_DIRECT_TOOLS` read and no `await initialization` — there is no initialization to await (MCP-008/MCP-011). A `MCP_DIRECT_TOOLS`-pinned run therefore does not delay `SessionStart`. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `on_session_start` spawns and returns; nothing awaits the build when `MCP_DIRECT_TOOLS` names servers missing from the cache. Upstream: `index.ts:1136-1150` (`await initialization` when `getMissingConfiguredDirectToolServers` is non-empty). |
| `MCP-014` | medium | `hand-written` | **partial** | Re-`init` per session, and the build-before-dispose inversion | Two halves unproven/unbuilt. (1) The `SessionStart` rebuild half is empty (see MCP-008/MCP-011), so nothing that a replacement is supposed to preserve across generations is exercised. (2) The unit's four cyrup-it assertions are absent — `crates/cyrup-it/tests/mcp/activation.rs` contains only the … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`; severity `high` → `medium`.** the rebuild half has landed (MCP-008/011 above); what is left is test-only: `crates/cyrup-it/tests/mcp/activation.rs` has three tests, none forcing a session replacement, so the unit's four assertions (i)–(iv) are absent. Severity lowered because the remaining obligation is verification, not behaviour. Upstream: persistence of the closure variables across `session_start`: `index.ts:170-360`. |
| `MCP-015` | medium | `extension-owned` | **implemented** | Snapshot every context value before the first await in `initialize` | The two live-closure replacements are not built: nothing constructs a sampling config, so `getCurrentModel -> HostServices::current_model()` and `getSignal -> combine(owner, ctx)` have no call site, and `runtime.rs:139` binds the combined token to `_runtime_signal` (unused). No production code … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `extension.rs` `context_snapshot` is built synchronously before the first await and is the only ctx-derived value crossing into `initialize_mcp`; the two live closures now exist (`sampling.rs` `SamplingOptions::{current_model, signal}`). Their two defects are recorded on `MCP-458`, which owns them. Upstream: `init.ts:113-140`, `:162-165`. |
| `MCP-016` | medium | `hand-written` | **implemented** | The sampling and elicitation wiring gates | No call site applies the gates. `runtime::initialize_mcp` (runtime.rs:125-247) never reads `settings.sampling(...)`/`settings.elicitation(...)` and never wires a sampling or elicitation hook; `McpClientHandlerParts` (runtime.rs:1076) derives capabilities from whether hooks are present, but nothing … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `runtime.rs` `initialize_mcp` steps 5 and 6: `settings.sampling(has_ui)` → `set_sampling_config`; `settings.elicitation(has_ui)` + `ui` → `set_elicitation_config` with `allow_url = is_tui_mode()`. Upstream: `init.ts:154-171`. |
| `MCP-017` | medium | `hand-written` | **implemented** | Register owner cleanups in the exact LIFO order, plus the list-changed listener | Two of the unit's three parts are missing. (1) `cleanupMaterializedBinaryResources` is never registered as the first cleanup (so it would not run last): `renderers::MaterializedResources::cleanup` exists (renderers.rs, see the doc at :79) but has no `owner.add_cleanup` call site — grep for … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `extension.rs` `start_initialization` registers the materialized-resource cleanup first; `runtime.rs` registers `shutdown_oauth` (when owned), the list-changed listener, then `lifecycle.graceful_shutdown`. Upstream: `init.ts:245-266`; the listen-state listener is `MCP-508`, the `approvedServers` reset is `MCP-543`. |
| `MCP-018` | low | `hand-written` | **implemented** | The zero-enabled-servers early return | The user-facing half is absent: the `MCP: All {n} server(s) are disabled` info notification, gated on `allServerEntries.length > 0 && hasUI`, is not emitted — grep for `are disabled` across `crates/cyrup-mcp/src` returns only an unrelated proxy.rs comment at :4977. There is also no unit test … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `runtime.rs` zero-enabled arm: `MCP: All {n} server(s) are disabled` gated on `all > 0` and a UI, snapshot published. Upstream: `init.ts:268-276`. |
| `MCP-019` | medium | `hand-written` | **implemented** | Metadata-cache bootstrap: file-absent means connect everything once | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: The whole §9 block: the `existsSync(cachePath)` probe distinguished from `loadMetadataCache() == None`; `!cacheFileExists ⇒ bootstrapAll = true` plus `saveMetadataCache({version:1, servers:{}})`; `file present but unparseable ⇒ rewrite empty WITHOUT bootstrapping`; and the `bootstrap_all` flag … |
| `MCP-020` | medium | `hand-written` | **implemented** | Per-server lifecycle registration and idle-override derivation | The per-server registration loop in `initializeMcp` is absent — `runtime::initialize_mcp` never iterates `serverEntries` and never calls `register_server`/`mark_keep_alive`. Critically, the `idleOverride = definition.idleTimeout ?? (persistsAfterFirstSpawn ? 0 : undefined)` derivation exists … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `runtime.rs` §10 loop: `persists = Eager | LazyKeepAlive`, `idle_timeout.or(persists → 0)`, `register_server`, `mark_keep_alive` for `keep-alive` only. Upstream: `init.ts:296-307`. |
| `MCP-021` | medium | `hand-written` | **implemented** | Rehydrate tool/resource/prompt/instruction metadata from a hash-valid cache … | §10 step 6 in full: for each hash-valid `cachedEntry`, populate `toolMetadata` via `reconstructToolMetadata(name, entry, prefix, definition, config.mcpServers, cache)`, `resourceCounts` from `entry.resources.length`, `promptMetadata` via `reconstructPromptMetadata` **without** adding to … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `live.rs` `rehydrate_from_cache` behind `registration::valid_entry`, called from the §10 loop. Upstream: `init.ts:309-325` (the shared selector index is `MCP-539`). |
| `MCP-022` | medium | `hand-written` | **implemented** | The bounded startup connect pass | The whole §11: the `bootstrapAll ? all : keep-alive\|eager` selection, the `connecting to {n} servers...` status write through `formatMcpStatus`, an index-preserving `parallel_limit` worker pool at concurrency 10, the `needs-auth ⇒ "OAuth authentication required. Run /mcp-auth {name}."` byte-exact … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `runtime.rs` §11: `bootstrap_all ? all : is_prewarmed`, `connecting to {n} servers...`, `live::parallel_limit(…, STARTUP_CONNECT_CONCURRENCY)`, byte-exact needs-auth line. Upstream: `init.ts:327-360` (the transient-503 arm is `MCP-507`). |
| `MCP-023` | high | `hand-written` | **implemented** | The two-pass startup metadata build | §12 in full: pass one building `startupKnownMetadata: Map<server, ToolMetadata[]>` over every successful connection (tools plus, when `exposeResources !== false`, `read_<resource>` entries with their `resourceUri` and `Read resource: {uri}` default description) before any per-server build; pass two … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `runtime.rs` §12 pass one (`startup_known`) and pass two (`build_tool_metadata(…, Some(&startup_known), true)`). Upstream: `init.ts:365-430`. |
| `MCP-024` | medium | `hand-written` | **implemented** | Failure tracking with a 60-second backoff | All of §13: the `FAILURE_BACKOFF_MS = 60_000` and `MAX_FAILURE_MESSAGE_CHARS = 8*1024` constants, `clearFailure`, `recordFailure` (clear-first, `failedAt` stamp, message truncation, the expiry task holding a `Weak<McpState>` and selecting on the owner token, and the `== failedAt` generation check … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `live.rs` `clear_failure` / `record_failure` (clear-first, 8 KiB cap, owner-select expiry task, `failed_at` identity check) / `failure_age_seconds`. Upstream: `init.ts:59-100`; the `restoredReason` metadata notification and backoff hiding are `MCP-515`. |
| `MCP-025` | high | `hand-written` | **implemented** | Startup connect notifications, terminal sanitising, and skipped-tool warnings | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: The notification half is entirely absent. Grep for `servers connected`, `tools skipped`, `Failed to connect to {name}` (the startup form) across `crates/cyrup-mcp/src` returns nothing: the per-failure `MCP: Failed to connect to {name}: {sanitized}` double report (`ui.notify(Error)` **and** always … |
| `MCP-026` | low | `hand-written` | **implemented** | The `MCP_DIRECT_TOOLS` cache-bootstrap pass inside `initialize` | All of §14's second half: the `__none__` skip, the deliberate re-read of `process.env.MCP_DIRECT_TOOLS` and of the cache from inside `initializeMcp` (not the factory's closure value), the exclusion of servers already connected in the startup pass, the concurrency-10 pass, the `MCP server "{name}" … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `runtime.rs` §14: `__none__` raw test, cache re-read, exclusion of already-connected servers, concurrency-10 bootstrap, checkpoint 3, `will be available after restart` notice. Upstream: `init.ts:442-483`. |
| `MCP-027` | medium | `hand-written` | **implemented** | Lifecycle callbacks (reconnect, reconnect-failure, idle shutdown) | No callback is ever installed. `runtime::initialize_mcp` calls none of the three setters (grep for `set_reconnect_callback`/`set_idle_shutdown_callback` outside lifecycle.rs and its tests returns nothing), so the owner-guarded bodies the unit specifies — `updateServerMetadata → updateMetadataCache … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `runtime.rs` §15 installs all five callbacks, each owner-guarded. Upstream: `init.ts:486-521`. |
| `MCP-027a` | medium | `hand-written` | **missing** | `sendMessage`'s `triggerTurn` pre-turn convergence gate **(v2.26.1 retarget, … | Grow `SendMessage` to carry the `triggerTurn` flag (`Fn(String, bool)` or a small options struct), update both `state.rs` structs that hold it and the builder at runtime.rs:188, then implement the gate: with the flag unset deliver synchronously; with it set await … — **Re-read 2026-09-14 at `9aeba769` — still missing, and now a hard prerequisite rather than a nicety.** cyrup still types `pub type SendMessage = Arc<dyn Fn(String) + Send + Sync>` (state.rs:68) with no options parameter, so the `triggerTurn` branch is inexpressible — exactly as this row's own doc at state.rs:60-67 says. The v2.33.0 window promotes it: `proxy-modes.ts:134-146 emitAuthStatus` delivers the new `mcp-oauth-status` custom message through `state.sendMessage?.(…, { triggerTurn: true })`, so the OAuth status surface cannot be ported at all until this unit lands. One correction to the recorded work plan: the **host** seam already exists — `HostServices::inject_message(…, trigger_turn: bool)` is in the `fenced!` block at owner.rs:456-465, and `lifecycle::ensure_converged` exists at lifecycle.rs:946 — so the work is confined to the `SendMessage` alias, the two `state.rs` structs that hold it, and the builder. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: `state.rs:69` `pub type SendMessage = Arc<dyn Fn(String) + Send + Sync>`; `runtime.rs`'s builder logs `send_message not yet wired` and drops the message. Upstream: `init.ts:226-240` (`triggerTurn` → `lifecycle.ensureConverged` then deliver). |
| `MCP-028` | medium | `hand-written` | **implemented** | `updateServerMetadata` | The whole function: the connection-exists-and-connected guard, the definition-exists guard, the **disabled ⇒ delete from all five maps and return** arm, and otherwise `buildToolMetadata(..., state.toolMetadata)` (the *current* map as collision universe, not the startup snapshot), setting … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `live.rs` `update_server_metadata`: connected guard, definition guard, disabled ⇒ five-map delete, `build_tool_metadata` against the current map, prompt/instruction rules. The v2.34.0 not-connected retirement arm is filed as `MCP-583`. Upstream: `init.ts:539-573`. |
| `MCP-029` | high | `hand-written` | **implemented** | `updateMetadataCache` write rules | `updateMetadataCache(state, serverName, {preserveEmptyResources})` itself does not exist — grep for `preserve_empty`, `preserveEmptyResources`, `prompt_discovery_failed`, `serialize_tools`, `serialize_resources`, `serialize_prompts` across `crates/cyrup-mcp/src` returns nothing outside the … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `live.rs` `update_metadata_cache` (hash-keyed prompt fallback, `exposeResources`, presence-tested `instructions`, merge write). Upstream replaced the `preserveEmptyResources` rule at v2.34.0 — filed as `MCP-583`. Upstream: `init.ts:575-617`. |
| `MCP-030` | low | `hand-written` | **implemented** | `notifyToolMetadataUpdated` must never let a hook break a connect | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: Two obligations unmet. (1) No panic containment: the unit requires `std::panic::catch_unwind(AssertUnwindSafe(...))` around the call as the closest analogue of a thrown JS exception — grep for `catch_unwind` across `crates/cyrup-mcp/src` returns nothing, so a panicking hook propagates out of … |
| `MCP-031` | medium | `hand-written` | **implemented** | `flushMetadataCache` on shutdown | The real flush: iterate `manager.getAllConnections()` and, for every connection whose status is `connected`, call `updateMetadataCache(state, name)` synchronously (or awaited), then wire it as the `MetadataFlush` passed to `shutdown_state` from `on_session_shutdown`. Depends on MCP-029 and on 13c's … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `live.rs` `metadata_flush` — every `Connected` connection through `update_metadata_cache` — passed to `shutdown_state` by both session handlers. Upstream: `init.ts:634-640`. |
| `MCP-032` | low | `host-verb` | **implemented** | `updateStatusBar` — the three footer verbosities | The stateful wrapper `updateStatusBar(state)` does not exist. Missing: step 1's unconditional `publishMcpStatusSnapshot(state)` **before** the `!ui` early return, step 2's `if (!state.ui) return`, step 5's `connectedCount` derivation (connections `connected` **and** whose definition exists and is … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `live.rs` `update_status_bar`: publish first, `!ui` return, footer counts, `set_status`. Upstream: `init.ts:642-663` (accent styling is `MCP-560`/`MCP-531`). |
| `MCP-033` | medium | `hand-written` | **implemented** | `lazyConnect` | The eight-step algorithm in full: the combined `ownedSignal` + `throwIfAborted`; the four `false` guards in order (`needs-auth`; already-`connected` returning `true` after `updateServerMetadata` + `markKeepAliveAfterConnect`; inside the 60 s failure backoff via `getFailureAgeSeconds`; … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `live.rs` `lazy_connect`: owned signal, needs-auth / connected / backoff / disabled guards, status write, connect, commit sequence, abort-vs-failure split. Upstream: `init.ts:665-708` (`ensureListen` is `MCP-508`). |
| `MCP-034` | medium | `hand-written` | **implemented** | `McpLifecycleManager` — the health-check state machine | None for the state machine. Two downstream notes, owned by other units: `start()` has no production caller (`initializeMcp`'s … |
| `MCP-035` | high | `hand-written` | **implemented** | `gracefulShutdown` — memoised, and it waits for the in-flight check | None for this unit. `ManagerSupervisor::close_all` is still the deliberate `Ok(())` no-op pending 13c/MCP-126 (lifecycle.rs:320-327), matching … |
| `MCP-036` | medium | `hand-written` | **implemented** | `syncDirectTools`: the fingerprint diff, the re-activation path, and the … | `syncDirectTools`/`syncToolSurface` as a *within-session* operation does not exist. `register_surface` registers everything unconditionally (correct for `init` per MCP-014) but there is no diff: no `added`/`updated`/`deactivated` computation against `registered_direct_tools`, no re-activation path … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `extension.rs` `sync_tool_surface` (fingerprint diff via `LateSink`, per-tool `reactivate_tool`, `deactivate_tools` fallback, wholesale adoption). Upstream: `index.ts:500-583`. |
| `MCP-037` | high | `host-addition` | **implemented** | HA-1: a native extension has no handle to `ExtensionHost::register_late_tool` | Build one of the two shapes: (i) a defaulted `NativeExtension::set_ext_host(&self, host: Weak<ExtensionHost>)` called from `ExtensionHost::load_native_with_services` beside `set_host_services`, plus a new `ExtensionHost::register_late_command`; or (ii) defaulted `HostServices::{register_late_tool, … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `cyrup-ext/src/facade.rs` `HostLateRegistrar` (tool + command + renderer) handed over by `NativeExtension::set_late_registrar`; `extension.rs:2348`. Upstream: `index.ts:402` (`pi.registerTool` after load). |
| `MCP-037a` | critical | `host-addition` | **implemented** | HA-1b: `refresh_tools` drops the native tier's dirty flag in the `wasm-host` … | Minor residual against the verify paragraph: the tests assert `refresh_tools()` reports the change, not the full chain through … |
| `MCP-038` | medium | `host-verb` | **implemented** | `deactivateTools`: the optional `unregisterTool` primary path and the … | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: Implement `deactivate_tools(names)`: empty ⇒ `[]`; cyrup lands on upstream's `unregisterTool === undefined` branch, so go straight to the fallback — `remove = set(names)`, `active = services.active_tools()`; when `None` or empty, add every name to `fallback_deactivated_tools` and return; otherwise … |
| `MCP-039` | low | `host-addition` | **partial** | MCP prompts as slash commands registered after `init` | The after-`init` half is missing because its host seam is missing (same seam as HA-1): `ExtensionHost` has no `register_late_command` sibling to `register_late_tool` (grep over `crates/cyrup-ext/src` finds only `register_late_tool` at facade.rs:645), and `InitApi` is `&mut` only during `init`. … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`; severity `medium` → `low`.** the host leg exists (`facade.rs` `HostLateRegistrar::register_command` + `on_commands_changed`) and `sync_tool_surface` registers new prompt commands through `LateSink` — but `sync_tool_surface` returns before doing anything once `direct_tools_frozen()`, so under `settings.freezeDirectTools` a prompt discovered after the initial sync never becomes a slash command. Upstream: `index.ts:936-944`: `syncPromptCommands()` runs **before** the `directToolsFrozen` early return. |
| `MCP-040` | medium | `host-verb` | **implemented** | The `/mcp` command handler | The whole handler: the fenced `commandCtx` (owner-fenced services, `commandReload`, `commandHasUI`, the owner's token as signal), the un-timed `await initPromise` preamble with `MCP initialization failed: {message}` / `MCP not initialized`, the arg split (`parts[0]` / `parts[1]` / `rest`), and the … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `commands.rs` `on_mcp_command` via `command_prologue` (owner fence, init join, the two notices) and the subcommand switch. Upstream: `index.ts:1224-1452`. |
| `MCP-041` | medium | `host-addition` | **implemented** | HA-2: `/mcp`'s dynamic argument completions have no native path and no TUI … | (a) A defaulted `NativeExtension::argument_completions(&self, command, prefix) -> Vec<(String, String)>` plus a non-`wasm-host` arm on `ExtensionHost::command_completions` routing through the native map the way `execute_native_command` already does, preserving the value/label pair; (b) … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `NativeExtension::argument_completions` overridden at `extension.rs:2311` → `commands.rs` `argument_completions`. Upstream: `index.ts:1226-1275` (the label half is `MCP-382`). |
| `MCP-042` | medium | `host-verb` | **implemented** | The `/mcp-auth` command handler | The handler: the same fenced `commandCtx`; **`if (!serverName && !commandCtx.hasUI) return;` silently, before the init-await** (the ordering detail the unit says must survive); the shared init-await/`MCP not initialized` preamble; no-name-with-UI ⇒ the `programmaticConfig` notice or the auth … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `extension.rs` `on_mcp_auth_command` (bare+headless silent return before the init join, picker, `authenticate_server`, reconnect). Upstream: `index.ts:1453-1506`. |
| `MCP-043` | high | `hand-written` | **implemented** | The `mcp` gateway tool: registration, the init wait, and the dispatch order | The two are disjoint types and the model reaches only the inert one. `proxy::McpTool::new` is constructed **only in tests** (all hits proxy.rs:5497-5584 are inside `#[cfg(test)]`); the tool actually handed to `api.register_tool` is `registration::ProxyTool`, whose `execute` … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `registration.rs` `ProxyTool::execute` → `dispatch.rs` `McpDispatch::call_proxy`, which drives `proxy::McpTool`'s router through the installed dispatcher. Upstream: `index.ts:1780-1978`. |
| `MCP-044` | n/a | `cut` | **not-applicable** | The `mcpScript` tool | Cut 4, and honoured. `mcpScript` is never registered: `registration::register_surface` registers only the prompt commands, the flag, `/mcp`, … |
| `MCP-045` | medium | `host-verb` | **implemented** | The `tool_result` `isError` override | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: The handler arm is not written. `McpExtension::on_event` (extension.rs:535-559) matches `SessionStart`, `Input`, `SessionShutdown` and falls through everything else to `HookOutcome::Noop`, with the comment `// MCP-045 fills the isError override.` at extension.rs:557. Missing: a … |
| `MCP-046` | medium | `hand-written` | **implemented** | The abort call-site discipline inside the runtime | The audit this unit *is* cannot pass, because the guarded sites do not exist yet. None of the four `owner.throwIfInactive()` checkpoints inside `initializeMcp` are present — after the startup connect pass, at the top of every pass-two iteration, after the `MCP_DIRECT_TOOLS` bootstrap, and before … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `runtime.rs` `owner.throw_if_inactive()` checkpoints 1–4 (after the connect pass, top of each pass-two iteration, inside the bootstrap arm, before health checks). Upstream: `init.ts:363`, `:389`, `:479`, `:523`. |
| `MCP-047` | critical | `hand-written` | **implemented** | Port `agent-plugin-loader.ts` | `crates/cyrup-mcp/src/agent_plugin.rs` (2115 lines) ports the ruleset. Entry points … |
| `MCP-048` | high | `open-decision` | **implemented** | Agent-directory resolution, and whether `~/.pi/agent` is a migration source | Residual, small: the unit's cyrup-it assertion that `cyrup-mcp` and `cyrup-permission-system` resolve the **same** `mcp.json` path for a given … |
| `MCP-049` | medium | `hand-written` | **implemented** | Port `cli.js init` as a `cyrup mcp init` subcommand | Add `"mcp"` to the visible `SUBCOMMANDS` table and an `mcp init [--dry-run] [--discover-host-configs]` arm to `dispatch`. Port `runInit`: `findAvailableImports` over the seven families (first existing candidate per family); `loadPiConfig` via `cyrup_permission_system::jsonc` accepting `mcpServers` … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `crates/cyrup/src/mcp_cmd.rs` `run` / `run_init` (`--dry-run`, `--discover-host-configs`, retired `install`), dispatched from `subcommands.rs:468`. Upstream: `cli.js:160-205` `runInit`; the `token` / `key` verbs are `MCP-504` / `MCP-550`. |

### 13b · The six-source config ladder

[`13b-mcp-config.md`](13b-mcp-config.md) — 51 units, 6 missing, 13 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-050` | n/a | `extension-owned` | **implemented** | Create `cyrup-mcp` and its config module skeleton | crates/cyrup-mcp/Cargo.toml exists as a workspace member; src/lib.rs carries `#![forbid(unsafe_code)]` and `#![deny(clippy::{unwrap_used, … |
| `MCP-051` | high | `extension-owned` | **implemented** | Read `mcp.json` as JSONC, not JSON | src/config.rs:1527 `parse_json_config(raw, path)` = `cyrup_permission_system::jsonc::parse_config_into::<RawJson>(raw, path, "MCP config")`; … |
| `MCP-052` | high | `hand-written` | **implemented** | Port the six-source precedence ladder | src/config.rs:3231 `ConfigContext::sources()` emits all six rungs with the exact dedup guards (`generic != user_path`; each `.agents` path `!= … |
| `MCP-053` | critical | `hand-written` | **implemented** | Port `mergeServerMaps`, including URL-bound credential stripping | src/config.rs:2274 `URL_BOUND_AUTH_FIELDS: [&str; 4] = ["headers","bearerToken","bearerTokenEnv","requestHeadersCommand"]`; src/config.rs:1738 … |
| `MCP-054` | n/a | `cut` | **not-applicable** | socket ⇄ command/url transport-swap stripping | Cut. `ServerEntry` (src/config.rs:630-751) has no `socket` field, so neither transport-swap rule has an input. The Cut-3 propagation the plan … |
| `MCP-055` | medium | `hand-written` | **implemented** | Port `expandImports` / `mergeImports` | src/config.rs:1841 `merge_imports(left,right)` is concat + first-seen dedup; src/config.rs:2322 `expand_imports(config, home, cwd, diagnostics)` … |
| `MCP-056` | medium | `hand-written` | **implemented** | Port the 7 host-config import families | src/config.rs:157 `ImportKind` has all 7 variants in `IMPORT_PATHS` declaration order with `ALL` (src/config.rs:177) fixing iteration order; … |
| `MCP-057` | medium | `hand-written` | **implemented** | Port the `opencode` multi-file merge and entry translation | src/config.rs:1957 `resolve_opencode_project_candidate` does the two-phase walk (up for `.git` to find gitRoot; then down-to-up from cwd for the … |
| `MCP-058` | medium | `hand-written` | **implemented** | Port `hostConfigDiscovery` and `loadDiscoveredHostConfigs` | src/config.rs:2694 `ConfigContext::merged_settings` walks the ladder and one-level-merges each source's `settings` only; src/config.rs:2713 … |
| `MCP-059` | medium | `hand-written` | **implemented** | Port `getMcpDiscoverySummary`, conflicts and the fingerprint | src/config.rs:3866 `config_source_summaries`, src/config.rs:3891 `mcp_standard_config_summary` (its own narrower `{sources}` fingerprint), … |
| `MCP-060` | low | `hand-written` | **implemented** | Port RepoPrompt detection and `KNOWN_SERVER_PRESETS` | src/config.rs:4196 `known_server_presets()` returns all five with `id`/`name`/`summary`/`entry` byte-matching §15's table (including … |
| `MCP-061` | high | `extension-owned` | **implemented** | Port the atomic raw-config writer | src/config.rs:2891 `write_raw_config_object` does `create_dir_all(parent)`, writes `<path>.<std::process::id()>.tmp`, then `std::fs::rename` — no … |
| `MCP-062` | low | `hand-written` | **implemented** | Port `buildUnifiedDiff` (LCS) and `ConfigWritePreview` | src/config.rs:2975 `build_unified_diff` is the literal bottom-up `(rows+1)×(cols+1)` LCS DP with the addition-preferring tie-break spelled `right >= … |
| `MCP-063` | high | `hand-written` | **implemented** | Port `writeProjectServerDisabledOverride` | src/config.rs:3226 `ConfigContext::write_project_server_disabled_override` writes only the `disabled` key into `<cwd>/.cyrup/mcp.json`, preserves … |
| `MCP-064` | medium | `hand-written` | **implemented** | Port `getServerProvenance` and `writeDirectToolsConfig` | src/config.rs:3402 `ConfigContext::server_provenance` runs the three passes: host families (only when discovery is `On`, in `ImportKind::ALL` order, … |
| `MCP-065` | low | `hand-written` | **implemented** | Port `ensureCompatibilityImports`, starter config and shared-entry writers | src/config.rs:3510 `ensure_compatibility_imports` returns `added: []` and does not write when nothing is added; src/config.rs:3487 … |
| `MCP-066` | high | `hand-written` | **implemented** | Port `McpSettings` as a permissive struct with per-site defaults | src/config.rs:804 `McpSettings` carries 23 `Option<T>` fields (§5's 22 live keys after the `scriptMode` cut, plus v2.26's `warnOnLargeDirectTools`), … |
| `MCP-067` | medium | `hand-written` | **implemented** | Port the settings merge as a one-level key merge | Nothing behavioural. (Consequence of rule 4, already documented: a wrong-typed `settings.trace` in a higher-precedence file becomes `None` and … |
| `MCP-068` | high | `hand-written` | **partial** | Port env-var overrides, including the `__none__` sentinel | Three obligations unmet. (1) `MCP_UI_DEBUG` has no reader — the string does not appear anywhere in crates/cyrup-mcp/src (grep over src/*.rs: zero hits), so the logger level bootstrap does not exist. (2) `BROWSER` has no reader — it appears only in doc comments at src/oauth.rs:2534 and … — **Re-read 2026-09-14 at `9aeba769` — still `partial`.** Two of the three recorded obligations re-confirmed absent by direct grep: `MCP_UI_DEBUG` has no reader (zero hits, so the logger-level bootstrap does not exist) and `BROWSER` has no reader (zero hits outside the prose at oauth.rs:2534 and ui.rs:3062). The third obligation was **not** re-checked, so this line closes nothing and moves nothing. `logger.ts` is absent from the v2.32.1..v2.33.0 diff, so the upstream side is unchanged by the window. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `MCP_UI_DEBUG` and `BROWSER` have no reader (`grep` = 0 outside prose). Upstream: `logger.ts:167`; `init.ts:222` (`openUrl(pi, url, process.env.BROWSER, …)`). |
| `MCP-069` | high | `hand-written` | **implemented** | Port `ServerEntry` as a typed struct | REFUTED. MCP-069's only obligation about this message is 'The exactly-one-transport message loses `, or socket`'. Upstream is `Server ${name} must … |
| `MCP-069a` | critical | `hand-written` + `open-decision` | **not-applicable** | Fail **closed** on a malformed `requestHeadersCommand` **(v2.26.1 retarget, … | NOT-APPLICABLE by verdict class. The canonical table gives MCP-069a the verdict `hand-written` + `open-decision`, and the plan text itself says … |
| `MCP-070` | high | `hand-written` | **implemented** | Enforce the absent-vs-null hash pre-image contract | Three things stop this from being a working contract. (1) Every production caller hashes UNRESOLVED values: src/ui.rs:1758 uses `ResolvedIdentity::verbatim(definition)`, whose own doc at src/dirs.rs:687-694 says it is a placeholder "until MCP-082 and MCP-084 land" and is "wrong, silently" for any … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** production hashing is `dirs.rs` `try_compute_server_hash` → `ResolvedIdentity::resolve`; `ResolvedIdentity::verbatim` now appears only inside `dirs.rs`' `#[cfg(test)]` module (from `:1428`). Upstream: `metadata-cache.ts:84-114`. |
| `MCP-071` | high | `hand-written` | **implemented** | Port `ToolPrefix` with all four modes and `sanitizeServerPrefix` | src/registration.rs:184 `sanitize_server_prefix(server_name, preserve_provider_valid)` iterates `chars()` (code points), keeps `[A-Za-z0-9_-]` when … |
| `MCP-072` | high | `hand-written` | **implemented** | Port `formatToolName` / `resolveToolPrefix` | src/registration.rs:235 `format_tool_name` does `tool_name.replace('.', "_")` only — hyphens survive — and joins with `{prefix}_{sanitized}` or … |
| `MCP-073` | high | `hand-written` | **implemented** | Port `resolveServerFromToolName` with its ambiguity fail-safe | A `pub fn resolve_server_from_tool_name(tool_name, server_names, prefix) -> Option<String>` on `cyrup-mcp`: `None` for `ToolPrefix::None`; collect every configured server whose non-empty `server_prefix` satisfies `tool_name.starts_with(prefix + "_")`; sort by prefix length descending; return `None` … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `registration.rs:283` `resolve_server_from_tool_name` (`None` for `ToolPrefix::None`, longest prefix, ambiguity fail-safe), re-exported at `lib.rs:193`. Upstream: `types.ts:904-925`. |
| `MCP-074` | medium | `hand-written` | **implemented** | Port `sanitizePromptName` / `formatPromptCommandName` | src/registration.rs:517 `sanitize_prompt_name` replaces each `[^A-Za-z0-9_-]+` run with one `_`, trims leading/trailing `[_-]`, returns `"prompt"` … |
| `MCP-075` | high | `hand-written` | **implemented** | Port `getToolNameCandidates` (the legacy candidate set) | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: The second copy is wrong. src/proxy.rs:486 `format_legacy_tool_name` derives the legacy prefix by taking `get_server_prefix(server_name, prefix)` — which has ALREADY applied the preserve-provider-valid grammar — and then re-escaping every non-alphanumeric character of that result, instead of … |
| `MCP-076` | high | `hand-written` | **implemented** | Port glob matching and `isToolIncluded`/`isToolExcluded`/`isToolAllowed` | src/registration.rs:334 compiles with bare `Regex::new(&out).ok()`, without the `RegexBuilder::size_limit` / `dfa_size_limit` ceilings the unit explicitly requires. The proxy copy does set them (src/proxy.rs:583-587 via `REGEX_SIZE_LIMIT`/`REGEX_DFA_SIZE_LIMIT`), so the two glob compilers differ in … — **Re-ruled 2026-09-14 at `9aeba769`: the recorded obligation is closed, `partial` → `implemented`.** The single named residual was that registration.rs compiles with a bare `Regex::new(&out).ok()` without the `RegexBuilder::size_limit` / `dfa_size_limit` ceilings the unit requires, so the two glob compilers differ. At 9aeba769 the registration-side compiler ends `RegexBuilder::new(&out).size_limit(REGEX_SIZE_LIMIT).dfa_size_limit(REGEX_DFA_SIZE_LIMIT).build().ok()` (registration.rs:438-442), and the proxy copy sets the same two ceilings (proxy/discovery.rs:538-539); both compilers now agree. Upstream's `matchesToolPattern` / glob grammar is unchanged across v2.32.1..v2.33.0 — the types.ts glob helpers are absent from the window diff. **Scope of this ruling, stated so it is not over-read:** it settles the row's *recorded* obligation only; the unit was not re-derived end to end. Reopen if any clause outside the two `RegexBuilder` ceilings is shown unmet. |
| `MCP-077` | high | `hand-written` | **implemented** | Port the metadata/cache type model | src/dirs.rs:385-537 carries the writer-side model: `CACHE_VERSION = 1`, `CACHE_MAX_AGE_MS = 7 days`, `CachedTool` (with `ui_resource_uri` / … |
| `MCP-078` | medium | `extension-owned` | **implemented** | Port the status-snapshot types | The snapshot's SHAPE was not ported. Present: four `Vec<String>` fields (`connected`, `idle`, `failed`, `pending_auth`). Absent: `MCP_STATUS_SNAPSHOT_VERSION = 1` as a constant (grep for `SNAPSHOT_VERSION` over src/*.rs: zero hits); the closed 6-variant `McpServerRuntimeStatus` … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `state.rs:487` `MCP_STATUS_SNAPSHOT_VERSION = 1`, the six-variant `McpServerRuntimeStatus`, optional fields `skip_serializing_if`. Upstream: `types.ts:20-60` (`directToolCount` / listen fields are `MCP-554` / `MCP-508`). |
| `MCP-079` | high | `hand-written` | **partial** | Port the tool-approval decision and origin types | Two obligations unmet. (1) `McpToolApprovalDecision` — the four-arm `allow_once \| allow_for_session \| deny \| abstain` enum — does not exist; grep for those spellings over crates/cyrup-mcp/src returns nothing. The nearest type, `ApprovalOutcome { Approved, Denied, NoInteractiveSession }` … — **Re-read 2026-09-14 at `9aeba769` — still `partial`; severity `medium` → `high`.** `McpToolApprovalDecision` — the four-arm `allow_once` / `allow_for_session` / `deny` / `abstain` enum — does not exist at 9aeba769 (zero hits for the type name or for an `AllowForSession` variant). The nearest type is still `ApprovalOutcome { Approved, Denied, NoInteractiveSession }`, which is three-arm and collapses `allow_once` and `allow_for_session` into one. **Why the severity moved:** the v2.32.1..v2.33.0 window makes the decision a *persisted* record — `session-approvals.ts:9-37` @v2.33.0 introduces `SessionApprovalEntry` (version 1) with `decision: "allow_for_session"` as a wire value, plus `TOOL_APPROVAL_KEYS` and a `SessionApprovalWriter` — so the missing enum is no longer an internal modelling gap, it is interop. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `proxy/env.rs:186` `ApprovalOutcome { Approved, Denied, NoInteractiveSession }` — no four-arm decision. Upstream: `types.ts:574` `McpToolApprovalDecision`. |
| `MCP-080` | n/a | `cut` | **not-applicable** | MCP-UI type surface in `types.ts` | Cut 2. No bridge-protocol or `ui://`-envelope type is exported from the crate: grep over crates/cyrup-mcp/src for `UiResourceMeta`, … |
| `MCP-081` | medium | `hand-written` | **implemented** | Port `McpAdapterOptions` / programmatic config mode | src/extension.rs:144 `McpExtension::with_config(dirs, programmatic_config)`; src/extension.rs:453 `init` short-circuits the whole ladder when a … |
| `MCP-082` | high | `hand-written` | **implemented** | Port `interpolateEnvVars` including the `{env:VAR}` form | src/credentials.rs:3322 `interpolate_env_vars_with(value, lookup)` runs three CHAINED `replace_all` passes in upstream's order over `\$\{(\w+)\}`, … |
| `MCP-083` | critical | `extension-owned` | **implemented** | Port `!` / `!!` command-secret resolution | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: Two obligations unmet. (1) `resolveCommandSecretsRecord` — the per-record form applied to `env` and `headers` — does not exist; grep over crates/cyrup-mcp/src for `resolve_command_secrets_record` / `interpolate_env_record` returns nothing, and src/runtime.rs:608-612 and :701-703 explicitly defer it … |
| `MCP-084` | high | `hand-written` | **implemented** | Port `resolveServerUrl` / `resolveConfigPath` / `resolveBearerToken` | `resolveServerUrl` is not implemented anywhere. Grep over crates/cyrup-mcp/src for each of its three exact strings — `MCP server URL must be a string`, `Missing environment variable`, `Invalid MCP server URL after` — returns zero hits, and `getMissingEnvVars`'s combined-alternation scan has no … — **Re-ruled 2026-09-14 at `9aeba769`: the recorded detail is refuted in full, `partial` → `implemented`.** Both bodies read line for line. `credentials.rs:3499 resolve_server_url(url, env)` reproduces `utils.ts:200-218` arm for arm, including the singular/plural `Missing environment variable{s} in MCP server URL: {names}` (credentials.rs:3505-3509) and `Invalid MCP server URL after environment interpolation: {resolved}` (credentials.rs:3512-3515); the third throw, `MCP server URL must be a string`, is absorbed by the type system and that absorption is **stated** at credentials.rs:3473 rather than silently dropped. `missing_env_vars` (credentials.rs:3445) is the single combined-alternation scan with insertion-order dedupe matching `utils.ts:114-123`, spelled `[A-Za-z0-9_]` deliberately because Rust's `\w` is Unicode-aware and JavaScript's is not. `resolve_bearer_token` (credentials.rs:3404) matches `utils.ts:231-236` including the truthy-`bearerTokenEnv` test, and `resolve_config_path` is in dirs.rs over `node_path_join` (dirs.rs:355). All four have production call sites: runtime.rs:4277, panel_host.rs:119, live.rs:1634 (the `ProxyEnv` implementor, consumed at proxy/auth.rs:106 and :274), secrets.rs:492, runtime.rs:4103. None of the four functions changed in the v2.32.1..v2.33.0 window. |
| `MCP-085` | medium | `hand-written` | **partial** | Port terminal sanitisation and error flattening | `formatTerminalError` is not ported. No function walks an error's children/`source()` chain with a cycle guard, falls back to the aggregate's own message when the nested walk yielded nothing, de-duplicates and joins with `": "`, then sanitises. `CleanupErrors`'s `Display` (src/errors.rs:250-262) … — **Re-read 2026-09-14 at `9aeba769` — still `partial`, re-confirmed at the current tag rather than carried.** `grep -rn 'fn format_terminal_error' crates/cyrup-mcp/src` is zero hits: nothing walks an error's `source()` chain with a cycle guard, falls back to the aggregate's own message, de-duplicates and joins with `': '` before sanitising. `ui.rs:404 sanitize_terminal_text` is still the half that exists — it is the half MCP-025's startup notifications consume, and MCP-232's approval title at proxy/approval.rs:334-338. Upstream's `utils.ts` moved +41 in the v2.32.1..v2.33.0 window but the terminal-error flattener is not what moved; `stripOscSequences` (utils.ts:239) and its neighbours are intact. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: no `format_terminal_error`; only `errors.rs`' aggregate `Display` reproduces its output for `CleanupErrors`. Upstream: `utils.ts:298-322`. |
| `MCP-086` | medium | `extension-owned` | **partial** | Port the browser/path open dispatch | `openUrl`/`execOpen`'s browser arm is missing. `BROWSER` is never read — grep over crates/cyrup-mcp/src finds it only in prose at src/oauth.rs:2534 and src/ui.rs:3062, both of which state the dispatch belongs elsewhere. So the macOS `.app`-vs-executable distinction (`exec(browser,[target])` when … — **Re-read 2026-09-14 at `9aeba769` — still `partial`, re-confirmed at the current tag.** `grep -rn '"BROWSER"' crates/cyrup-mcp/src` is zero hits; the only occurrences are prose at oauth.rs:2534 and ui.rs:3062, both of which say the dispatch belongs elsewhere. So `openUrl` / `execOpen`'s browser arm — including the macOS `.app`-vs-executable distinction — is still unported. Upstream still has the surface: `__tests__/utils-exec-open.test.ts` is *modified*, not deleted, in the v2.32.1..v2.33.0 diff. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: no `BROWSER` read, no `execOpen` browser arm. Upstream: `utils.ts:48-90`. |
| `MCP-087` | medium | `hand-written` | **implemented** | Port `parallelLimit`, argv scan, `toStringRecord`, … | `parallelLimit` is not ported. No order-preserving bounded-concurrency helper exists: grep over crates/cyrup-mcp/src for `parallel_limit`, `buffered(`, `buffer_unordered` and `JoinSet` returns nothing. The only bounded fan-out is src/lifecycle.rs:947 … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `live.rs:83` `parallel_limit` (order-preserving), `config.rs:320` `to_string_record`, `registration.rs:2246` `normalize_direct_tool_input_schema`, `config.rs` `config_path_from_argv`. Upstream: `utils.ts:85`, `:152`, `:337`; the argv scan's v2.33.0 drift is `MCP-551`. |
| `MCP-088` | medium | `host-verb` | **implemented** | Port `formatMcpStatus` and `formatAuthRequiredMessage` | src/ui.rs:4636 `format_mcp_status(config, message)` returns `None` when `mcp_footer_status() == FooterStatus::Off` and otherwise prefixes `"\u{1f50c} … |
| `MCP-089` | medium | `hand-written` | **partial** | Port the error taxonomy | The unit's named obligations are all unmet. There is no `fn code(&self) -> &'static str` and no `fn recovery_hint(&self) -> &'static str` on `McpError` (grep over src/*.rs for `fn code(` / `recovery_hint`: only the errors.rs doc comment). There is no `ConsentError` arm — neither the … — **Re-read 2026-09-14 at `9aeba769` — still `partial`, and the window makes this row larger rather than smaller.** `fn code(&self) -> &'static str` and `fn recovery_hint(&self) -> &'static str` still do not exist on `McpError`; the only hits are in the errors.rs header comment at errors.rs:14, which describes the unmet obligation. The v2.33.0 census block at the head of this file already records that `errors.ts` extracted the `McpUiError` taxonomy and that it is now consumed from `proxy-modes.ts`, `direct-tools.ts` and `error-signal.ts` as well as the UI files — so the taxonomy has **more** consumers to satisfy than when this row was written. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `McpError` has no `code()` / `recovery_hint()`. Upstream: `errors.ts:17-60`; its v2.32.0 `InputRequiredNeedsUiError` class is in `MCP-584`. |
| `MCP-090` | low | `extension-owned` | **partial** | Port the logger as a `tracing` adapter | The user-facing contract the unit says to keep is absent. `MCP_UI_DEBUG` is never read — grep over crates/cyrup-mcp/src returns zero hits — so the level bootstrap (`"1"`/`"true"` ⇒ debug) does not exist. There is no `[MCP-UI…]` prefix or stable `tracing` target (grep `[MCP-UI`: zero hits), no … — **Re-read 2026-09-14 at `9aeba769` — still `partial`.** `MCP_UI_DEBUG` is never read (zero hits), so the `"1"` / `"true"` ⇒ debug-level bootstrap does not exist; and `grep -rn '\[MCP-UI' crates/cyrup-mcp/src` is zero, so there is no `[MCP-UI…]` prefix and no stable tracing target. `logger.ts` is not in the v2.32.1..v2.33.0 diff, so the obligation is unchanged by the window. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: no `MCP_UI_DEBUG` bootstrap, no `[MCP-UI…]` prefix/target. Upstream: `logger.ts:167`. |
| `MCP-091` | medium | `hand-written` | **missing** | Port `renderTsShape` | The whole of ts-shape.ts: the `try/catch → None` envelope, `UNSUPPORTED_KEYWORDS` with the `additionalProperties: false` exemption re-tested at every node, the `$defs`/`definitions` collection with `~1`/`~0` pointer-token decoding, `aliasFor`'s bare-name reuse and `Definition{n}` fallback, the … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: `live.rs` `RuntimeEnv::render_ts_shape` returns `None`. Upstream: `ts-shape.ts:6` (file unchanged v2.32.1..v2.37.0). |
| `MCP-092` | high | `hand-written` | **implemented** | Port the dual-dialect JSON Schema validator | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: The whole dialect gate: `schemaDialect(schema)` (no string `$schema` ⇒ unstamped; else strip ONE trailing `#`), routing unstamped and `https://json-schema.org/draft/2020-12/schema` to a 2020-12 validator and the two draft-07 URIs to a draft-07 validator, the exact `Unsupported JSON Schema dialect: … |
| `MCP-093` | medium | `hand-written` | **missing** | Register the `ajv-formats` formats `jsonschema` does not ship | `ValidationOptions::with_format(name, fn)` for each format `ajv-formats` supplies beyond `jsonschema`'s built-ins (`url, int32, int64, float, double, byte, binary, password, iso-time, iso-date-time, json-pointer-uri-fragment`), registered on BOTH builders, plus the test that enumerates … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: `schema.rs` registers no extra formats. Upstream: `json-schema-validator.ts:47`, `:63` `addFormats(ajv)`. |
| `MCP-094` | high | `hand-written` | **implemented** | Reconcile `mcp_direct_tools` with this section's contract | One scheduled change to crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs covering all nine indexed divergences above, plus the shared conformance suite the unit names: one golden `mcp.json` + `mcp-cache.json` pair asserted identically by `cyrup-mcp` and by `mcp_direct_tools`'s resolver. … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** the reciprocal change landed: `cyrup-ext-subagents`' `mcp_direct_tools` takes `cyrup-mcp` as a dev-dependency and asserts its pre-image, digest and tool names against `cyrup_mcp::dirs` (golden vector at `mcp_direct_tools.rs` `pre_image_matches_the_upstream_generated_golden_vector`; `dirs.rs:62-68`). Upstream: `metadata-cache.ts:84-114`. |
| `MCP-095` | n/a | `extension-owned` | **implemented** | JSONC parser home | Settled as the plan directs. crates/cyrup-mcp/Cargo.toml declares `cyrup-permission-system = { workspace = true }` with the reasoning verbatim; … |
| `MCP-096` | high | `open-decision` | **not-applicable** | Project trust and the two project-scoped config sources | REFUTED / NOT-APPLICABLE. The canonical table gives MCP-096 the verdict `open-decision`, and the plan's own words are 'This is the only genuine open … |
| `MCP-097` | low | `hand-written` | **implemented** | Port `getConfigDiscoveryPaths` and `findAvailableImportConfigs` | src/config.rs:3835 `ConfigContext::config_discovery_paths` maps `sources()` to `{label, path, exists}` using only `read_path.exists()` — it never … |
| `MCP-098` | medium | `hand-written` | **missing** | Preserve `renderTsShape`'s re-entrant alias emission | When MCP-091 is written, its output loop must be an index-based `while i < aliases.len() { … i += 1 }` over a growing `Vec<(String,String)>` (or `IndexMap`) so a `$ref` registered by `render(definition)` inside the loop is itself visited and emitted, and insertion order is preserved. The golden … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged — depends on `MCP-091`. Upstream: `ts-shape.ts`. |
| `MCP-099` | low | `hand-written` | **implemented** | Reproduce `buildConfigWritePreview`'s reserialised "before" text | src/config.rs:3056 `build_config_write_preview` computes `before_text` as `serialize_raw_object(&read_raw_config_object(path))` when the file exists … |

### 13c · Server manager, transports, metadata cache

[`13c-mcp-servers.md`](13c-mcp-servers.md) — 51 units, 20 missing, 20 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-100` | high | `hand-written` | **implemented** | McpServerManager: the five race guards and the full public API | The entire manager is unbuilt. Absent: `ServerConnection` (client/transport/definition/tools/resources/prompts/promptDiscoveryFailed/instructions/lastUsedAt/inFlight/status/credentialsInvalidated); the seven manager-owned maps (`connections`, `connectPromises`, `reconnectPromises`, `closePromises`, … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `server_manager.rs` `McpServerManager` + `ServerConnection` (connect / reconnect / close / close_all / touch / in-flight / request options / URL-elicitation registry), constructed in production by `runtime.rs` `initialize_mcp`. Upstream: `server-manager.ts:264` onward. |
| `MCP-101` | high | `rmcp` | **implemented** | stdio transport: spawn, env resolution, cwd, plugin data dir | No `resolveEnv` and no connection builder. Specifically absent: (a) `args = (definition.args ?? []).map(interpolateEnvVars)` — grep for any arg-interpolation call site returns nothing; (b) `resolveEnv(env, serverName, literalEnv)` — the full-process-env copy, the `literalEnv === true` verbatim arm, … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `runtime.rs` `ConnectionBuilder` stdio arm: `args` through `credentials::interpolate_env_vars`, `cwd` through `resolve_config_path` or the default, `secrets::resolve_env` (full base copy, `literal_env` verbatim arm, `!`-secret layering), `plugin_data_dir` mkdir. `inheritEnv` is `MCP-553`; the v2.34.0 built-in-plugin literal fields are `MCP-585`. Upstream: `server-manager.ts:1046-1080`, `:2060-2078`. |
| `MCP-102` | medium | `rmcp` | **implemented** | stderr tail capture and failure-message enrichment | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: The pure functions are done; the live wiring is not. No production task reads the returned `ChildStderr` into a tail — the only readers are the unit tests (runtime.rs:1670-1700). And the enrichment call site (`createConnection`'s catch appending `(detail)` to the connect failure) does not exist … |
| `MCP-103` | medium | `extension-owned` | **missing** | Wire npx/npm resolution into the connection builder | `resolve_npx_binary` is never called from cyrup-mcp. Needed: the `pub` promotion + re-export in `cyrup_ext::caps::proc`, and the call in the (not-yet-existing) connection builder applying `command = resolved.is_js ? "node" : resolved.bin_path` / `args = is_js ? [bin_path, ...extra_args] : … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: `runtime.rs`' builder carries the comment `Step 3 — MCP-103, NOT PORTED`; `resolve_npx_binary` is `pub(super)` in `cyrup-ext` and called only by the WASM `proc.spawn`. Upstream: `server-manager.ts:1059-1068`. |
| `MCP-104` | medium | `hand-written` | **missing** | npx cache: bump to CACHE_VERSION = 2 and port clearLegacyCache | `CACHE_VERSION` must become 2, and `clear_legacy_cache() -> bool` must be added (unlink, falling back to `write("")`), invoked once at module load via `std::sync::Once` inside `load_cache` **and** on every `load_cache()`, returning `None` when it evicted. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: `npx_resolver.rs:38` `CACHE_VERSION: u32 = 1`, no legacy-cache clear. Upstream: `npx-resolver.ts:8`, `:485`. |
| `MCP-105` | high | `hand-written` | **implemented** | npx resolver: exact package-version pinning is missing | Add `ParsedPackageSpec { package_name, exact_version: Option<String> }` and `parse_package_spec` (the `@scope/name` `rfind('@') > find('/')` rule, `^=` then case-insensitive `^v` strip, validated against upstream's exact-semver regex); thread `exact_version` into the cache-hit predicate (`!exact … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `npx_resolver.rs:528` `parse_package_spec`, `exact_version` threaded into `cache_entry_is_usable` and `find_cached_package_dir`. Inert for MCP until `MCP-103`. Upstream: `npx-resolver.ts:304`. |
| `MCP-106` | low | `hand-written` | **missing** | npx resolver: cache key must be [command, packageSpec, binName] | Move the computation after the parse and serialise `[command, &parsed.package_spec, parsed.bin_name.as_deref().unwrap_or("")]`. Today two invocations differing only in trailing args, and `npx pkg bin` vs `npx --package pkg bin`, occupy different cache slots. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: `npx_resolver.rs` keys the cache on `cache_key(command, args)`. Upstream: `npx-resolver.ts:56` `[command, parsed.packageSpec, parsed.binName ?? ""]`. |
| `MCP-107` | medium | `hand-written` | **missing** | npx resolver: no cancellation path | Add a `cancel: &cyrup_core::CancelToken` parameter with `throw_if_aborted`-equivalent checks on entry, on `force_npx_cache` entry and on its exit, and a `cancel.is_cancelled()` check in the 50 ms poll loop that kills and reaps the child. A session shutdown currently cannot interrupt up to 30 s of … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: `resolve_npx_binary(command, args)` takes no cancellation. Upstream: `npx-resolver.ts:41-46`, `:251-285`. |
| `MCP-108` | low | `hand-written` | **partial** | npx resolver: entry-level cache validation and Windows npm resolution | **Re-ruled 2026-09-04 (v2.32.1 re-audit): partial** — citation in the re-audit block at the head of this file. Prior ruling: Deserialise `entries` as `HashMap<String, serde_json::Value>` and convert per entry (dropping failures, including a non-finite/absent `resolvedAt` and a non-string `packageVersion`). For Windows, resolve `npm.cmd`/`npm.exe` via a PATH+PATHEXT walk or invoke through `cmd /c npm` at both call sites. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `npx_resolver.rs:99` `entries: HashMap<String, NpxCacheEntry>`; no `npm.cmd`/`PATHEXT`. Upstream: `npx-resolver.ts:473-482`. |
| `MCP-109` | high | `rmcp` | **implemented** | Streamable HTTP client transport | Nothing constructs the transport in production: `build_http_transport_config` and `http_transport_with_client` have only test callers (runtime.rs:1733/1756) and a doc reference from request_headers_command.rs:51. No `HttpTransportSpec` is ever populated because `resolveServerUrl`/header resolution … — **Re-ruled 2026-09-14 at `9aeba769`: the recorded detail is refuted, `partial` → `implemented`** — and note this row was *already* moved by the 2026-08-22 wave-5 table at the head of this file; only this section table was left stale, so the section's own count line includes it wrongly. The recorded claim 'Nothing constructs the transport in production: `build_http_transport_config` and `http_transport_with_client` have only test callers' is false: `build_http_transport_config` is called at runtime.rs:4425 inside `connect_http_client` (runtime.rs:4265), and `http_transport_with_client` at runtime.rs:4511 and :4531. `connect_http_client` has a production caller — `ConnectionBuilder::create_connection` (runtime.rs:4927) at runtime.rs:4938, on the `TransportKind::StreamableHttp` arm, with the needs-auth early return that skips the try block exactly as `server-manager.ts` does (transport construction at server-manager.ts:1300-1330 @v2.33.0). **Wave 5's caveat still stands and is not re-closed here:** the `@modelcontextprotocol/conformance` client baseline was not run, and the transport is proven against a hand-rolled loopback fixture. |
| `MCP-110` | n/a | `cut` | **not-applicable** | Legacy HTTP+SSE transport and the shouldFallbackToSse ladder | The load-time diagnostic string in config.rs:1615-1618 is a *paraphrase* ("...requests `httpTransport: \"sse\"`; rmcp ships no SSE client transport … |
| `MCP-111` | n/a | `cut` | **not-applicable** | Unix-domain-socket transport | Cut 3 by owner decision. `ServerEntry` (/home/user/cyrup/crates/cyrup-mcp/src/config.rs:630+) has no `socket` field — grepping config.rs for `pub … |
| `MCP-112` | n/a | `rmcp` | **implemented** | MCP NDJSON framing | Nothing to write, per the plan. /home/user/cyrup/crates/cyrup-mcp/src/runtime.rs:410-411 imports `rmcp::transport::TokioChildProcess` and … |
| `MCP-113` | medium | `hand-written` | **implemented** | Transport selection and mutual exclusion | `select_transport` has no production caller yet (only runtime.rs tests) because the connection builder (MCP-100) does not exist. The selection logic … |
| `MCP-114` | high | `extension-owned` | **implemented** | HTTP header, bearer and command-secret resolution | The `connectHttpClient` pre-flight (§3.4 steps 1-7) does not exist. Specifically absent, verified by grepping all 19 files: (a) `resolveServerUrl` — no `resolve_server_url` symbol, and neither of its two user-visible throws (`Missing environment variable{s} in MCP server URL: ...` with … — **Re-ruled 2026-09-14 at `9aeba769`: the recorded detail is refuted, `partial` → `implemented`** (as with MCP-109, the 2026-08-22 wave-5 table already moved this row and only this section table was stale). The recorded claim 'The `connectHttpClient` pre-flight (§3.4 steps 1-7) does not exist … (a) `resolveServerUrl` — no `resolve_server_url` symbol' is false. `runtime.rs:4265 connect_http_client` implements the pre-flight in upstream's order with upstream's step comments: step 1 is `credentials::resolve_server_url` at :4277, **before any secret command is spawned**; steps 2-6 are `HttpTransportSpec::resolve` at :4288 (`crate::secrets` — hasCommandHeader, resolveCommandSecretsRecord, commandBearer, the bearer ladder, the Headers injection guard); the `requestHeadersCommand` decorator is built at :4297-4306 against `server-manager.ts:1314-1315` @v2.33.0. Production caller as for MCP-109. **Wave 5's caveat carries unchanged:** four of five verify bullets are asserted on the wire; the `bearerTokenEnv` fallback and the `HTTP bearer token` context string are asserted in secrets.rs's own tests instead. |
| `MCP-115` | high | `hand-written` | **implemented** | Implicit-vs-explicit OAuth provider state machine and the attempt loop | The attempt loop itself is absent — there is no `connect_http_client` anywhere in the crate. Missing: the per-attempt fresh client; the transport-options assembly including `skipIssuerMetadataValidation` (the config field exists at config.rs:1311 but has no consumer); the once-only per-attempt … — **Re-read 2026-09-14 at `9aeba769` — stays `partial`, and the 2026-08-22 wave-5 note is the accurate one, not this recorded detail.** The implicit-vs-explicit state machine and the attempt loop **do** exist: `runtime.rs:4308 let mut auth_state = crate::oauth::initial_http_auth_state(entry)`, then a `loop` at :4311 whose head matches on `HttpAuthProviderState::{Explicit, ImplicitChallenged, Disabled, ImplicitDeferred}` (runtime.rs:4317-4329) — Explicit reads the store on the first attempt, ImplicitChallenged only after a 401 has proven it needed to, exactly as the unit specifies. What is still unmet is what wave 5 recorded: `skipIssuerMetadataValidation` is read (the per-server name set is built at runtime.rs:236-246 and stored at :3439) and **consumed by nothing**, because rmcp's streamable-HTTP config has no such field. Ruled on the recorded residual; the unit was not re-derived end to end. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** the recorded residual is gone: `skipIssuerMetadataValidation` is consumed — `runtime.rs` `StoredCredentialAuth` carries the per-server set into `oauth.rs` `get_valid_token` (`:3042`) and the flow (`:2640`). Upstream: `mcp-auth-flow.ts:1171`. |
| `MCP-115a` | high | `hand-written` | **implemented** | Wire the per-request header command into `connectHttpClient` **(v2.26.1 … | Two concrete work items. (1) The wiring: build `RequestHeadersCommandClient::new(client, cfg, ct)?` **inside** MCP-115's retry closure and pass it to `http_transport_with_client(..., config)`, so it survives the implicit-OAuth retry the way upstream's `requestFetch` does. Blocked by … — **Re-ruled 2026-09-14 at `9aeba769`: built, `missing` → `implemented`** (again already carried by the 2026-08-22 wave-5 table; only this section table was stale). The recorded work item — build `RequestHeadersCommandClient::new(client, cfg, ct)?` **inside** MCP-115's retry closure and pass it to `http_transport_with_client(..., config)` so it survives the implicit-OAuth retry — is built at runtime.rs:4297-4306, and built **outside** the loop, deliberately, with the deviation stated at runtime.rs:4290-4296: *'The plan's cyrup note says inside the retry closure; the .ts says outside, and outside is what makes the eager `resolvedCommand(config)` validation fail the CONNECT exactly once rather than once per attempt.'* That was checked against the TypeScript rather than taken: `server-manager.ts:1314-1315` @v2.33.0 builds `commandFetch` once, above the attempt, and composes it with the new `caFetch?.fetch`. **The plan text is wrong and the code is right** — the decorator is reused across attempts and the MCP client is what is fresh per attempt (runtime.rs:4295; the attempt loop is at :4311). |
| `MCP-116` | high | `hand-written` | **implemented** | needs-auth connection state and one-shot credential invalidation | No connection record carries `credentials_invalidated`, so `connect`'s step-7 carry-forward (`existing?.status == "needs-auth" && existing.credentialsInvalidated === true`) does not exist, nor does either needs-auth exit (the HTTP ladder's own and `createConnection`'s catch-path downgrade), nor the … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `server_manager.rs:2103` carries `credentials_invalidated` forward from a needs-auth record; `ServerConnection::credentials_invalidated`. Upstream: `server-manager.ts:455-465`. |
| `MCP-117` | medium | `rmcp` | **implemented** | Protocol-revision negotiation | The open decision was resolved as option **(a)**: rmcp's `ClientLifecycleMode` is adopted as-is and the disposable-sibling mechanism is *not* … |
| `MCP-118` | medium | `rmcp` | **implemented** | Client capability advertisement (sampling / elicitation form+url) | One recorded, unavoidable divergence documented at runtime.rs:885-891: rmcp's `InitializeRequestParams::capabilities` is not an `Option`, so the port … |
| `MCP-119` | high | `rmcp` | **implemented** | Paginated discovery with capability gating and per-list failure policy | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: No discovery at all. Needed: the unconditional `list_all_tools` with errors propagating; the `resources`/`prompts` capability gate read from `RunningService::peer_info() -> InitializeResult.capabilities`; the per-list failure policy (abort and 401 re-throw; resources → silent `[]`; prompts → … |
| `MCP-120` | medium | `rmcp` | **partial** | list_changed refresh with identity guards | Only the notification plumbing exists. The `ListChangedHook` (runtime.rs:1051) is a type alias with no production implementation, so the actual §3.10 body is absent: the identity check against the live connection map, the `status == connected` check, the re-call of `list_all_*`, the wholesale field … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged in effect: `server_manager.rs` `manager_handler_factory` builds every production handler with `list_changed: None`, so `runtime.rs` `dispatch_list_changed` logs `list_changed with no refresh hook wired` and drops the notification. Upstream: `server-manager.ts:694-696` (SDK `listChanged` handlers). |
| `MCP-121` | n/a | `cut` | **not-applicable** | Adapter-private UI stream-patch notification handler | Cut 2 by owner decision, and the cut is honoured with the exact behaviour the unit's verify line asks for: … |
| `MCP-122` | medium | `hand-written` | **implemented** | URL-elicitation acceptance tracking and completion notice | Everything the manager owns is missing. Verified by grep across all 19 files: no `acceptedUrlElicitations` registry (`Mutex<HashMap<String, HashSet<String>>>`), no `remember_url_elicitation` (and therefore no `runtimeSignal.aborted` no-op rule), no `Set.delete`-returned-true gate, and the notice … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `server_manager.rs` `remember_url_elicitation` (runtime-abort no-op), `forget_url_elicitation` (delete-gated notice), wired from `runtime.rs`' `ElicitationOptions::on_url_accepted` and the handler factory's completion sink. (The "No production caller" doc comments on both methods are stale.). Upstream: `server-manager.ts:287`, `:1395`. |
| `MCP-123` | medium | `rmcp` | **partial** | Connect-time abort and once-only transport cleanup | The residual adapter policy is not written. Absent: the once-only cleanup handle for the HTTP retry ladder — grepped for `futures::future::Shared`, `BoxFuture<'static, Result<(), Arc<`, `abort_cleanup` across all 19 files, no hits — and the cleanup-failure-vs-connect-failure *distinction*, since … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `runtime.rs` `connect_http_client`'s own doc records that `HttpCleanupFailed` / `AbortCleanupFailed` have no producer on the HTTP ladder. Upstream: `server-manager.ts` `connectHttpClient` attempt catch. |
| `MCP-124` | high | `hand-written` | **implemented** | Error taxonomy and containsCleanupFailure | Add the five variants (`AbortCleanupFailed`, `SetupFailed`, `HttpCleanupFailed`, `ConnectionCleanupFailed`, `ManagerCleanupFailed`) with `Display` rendering the byte-exact heads `MCP connection abort cleanup failed`, `MCP connection setup failed`, `MCP HTTP connection cleanup failed`, `MCP … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `errors.rs:255-281` the five variants with byte-exact heads; `is_cleanup_failure`. Upstream: `errors.ts`/`server-manager.ts` heads. |
| `MCP-125` | high | `hand-written` | **implemented** | reconnect: guards, single-flight, identity, in-flight preservation | Missing entirely: the disabled and stopped guards firing **before** the single-flight map is consulted (with `MCP server "<n>" is disabled` / `MCP server manager is closed` — neither string exists in the crate); `reconnect_promises: Mutex<HashMap<String, Shared<BoxFuture<..>>>>` with … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `server_manager.rs` `reconnect` (disabled / `MANAGER_CLOSED` guards before `reconnect_promises`). Upstream: `server-manager.ts:507`. |
| `MCP-126` | high | `hand-written` | **implemented** | close / closeAll: generations, attempt aborts, late-name sweep | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: Nothing of §3.12 exists: the generation bump, the `connect_attempts[name].cancel()` with the `MCP connection <n> was closed` reason, removal from the map **before** awaiting cleanup, the no-connection path that awaits a pending close or re-throws only cleanup failures from a pending connect, … |
| `MCP-127` | medium | `hand-written` | **implemented** | Idle and in-flight accounting | No connection carries `last_used_at` or `in_flight`, so none of `touch` / `increment_in_flight` / `decrement_in_flight` (floor at zero) / `is_idle` (connected AND zero in-flight AND strict `now - last_used_at > timeout`) exists, and the RAII guard the plan specifies is unwritten. `lifecycle.rs`'s … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `ServerConnection::{touch, increment_in_flight, decrement_in_flight}`, `McpServerManager::is_idle`, `InFlightGuard`. Upstream: `server-manager.ts:2049`. |
| `MCP-128` | medium | `rmcp` | **implemented** | Request options: timeout normalisation and owned signal | The manager-side half is missing: `setDefaultRequestTimeoutMs` (which normalises on the way in) and the public `getRequestOptions(name, signal?)` that resolves the definition **by name from the live connection map** — neither exists, because the manager does not. The cancellation half is also … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `McpServerManager::{set_default_request_timeout_ms, get_request_options}`. Upstream: `server-manager.ts:355`. |
| `MCP-129` | medium | `rmcp` | **partial** | getPrompt / readResource accounting and disabled re-check | The manager's `getPrompt` and `readResource` do not exist. Missing: the `status == connected` precondition with the exact `Server "<n>" is not connected` message; `touch → incrementInFlight → … → finally { decrementInFlight; touch }` (touch **twice**); `readResource`'s live-definition … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `partial`.** was `missing`. `readResource`'s accounting exists (`proxy/call.rs:845-860` wraps `invoke`, which reads through `live.rs` `read_resource` → `with_session_recovery`). `getPrompt` has no counterpart: `grep -rn 'prompts/get\|get_prompt' crates/cyrup-mcp/src` finds only comments. Upstream: `server-manager.ts:1857` `getPrompt`, `:1881` `readResource`. |
| `MCP-130` | medium | `hand-written` | **implemented** | Startup connect concurrency limit | There is no startup connect path at all, so the concurrency bound has nothing to bound. When the startup connect lands it must be `futures::stream::iter(..).map(..).buffered(10).collect()` (limit 10, output in config order). — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `live.rs` `parallel_limit` at `STARTUP_CONNECT_CONCURRENCY` in `runtime.rs` §11 and §14. Upstream: `init.ts:344`. |
| `MCP-131` | medium | `rmcp` | **partial** | Child-process cleanup and orphan avoidance | Nothing in production ever owns or closes a `TokioChildProcess`: `spawn_stdio_transport` has only test callers, `ManagerSupervisor::close`/`close_all` are no-ops (lifecycle.rs:307-322), and grep finds no `graceful_shutdown()` call on a transport anywhere in cyrup-mcp. The non-orphan property … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`; severity `high` → `medium`.** restated: production now owns and closes the child (`StdioChildConnection` drain task + `graceful_shutdown`, driven by `close_inner`'s detached driver). Residual, recorded in `server_manager.rs`' own doc: rmcp closes stdin, waits 3 s, then `SIGKILL`s — no `SIGTERM` leg, so a server that would have cleaned up on `SIGTERM` is hard-killed. Upstream: SDK `StdioClientTransport.close` (stdin → `SIGTERM` → `SIGKILL`). |
| `MCP-132` | medium | `extension-owned` | **missing** | MCP endpoint probe (three-strategy ladder) | The entire three-strategy ladder is unwritten: the seven constants, the `modern` / `legacy-post` / `legacy-sse` request shapes (exact headers, bodies, 5 s timeout, unauthenticated, all three against the same URL), `classifyResponse`'s five rungs, `jsonRpcEnvelopeInfo`, `isBearerChallenge` … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: no endpoint probe in the crate. Upstream: `mcp-probe.ts:185`. |
| `MCP-133` | medium | `hand-written` | **missing** | Probe-enriched HTTP connect failures | Missing: the URL-only wrapping of the connect future, the exact ` — probe: ` separator (space, em-dash, space), preserving the original error as `cause`, and the swallow-all rule (any probe failure — including a `resolveServerUrl` throw on the re-resolve — returns the original error unchanged). … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: no `enrichHttpConnectionError` counterpart. Upstream: `server-manager.ts:470`. |
| `MCP-134` | high | `rmcp` | **partial** | isTerminatedSession predicate | The predicate does not exist. Needed: the absolute `hadSessionId` gate captured **before** the call; the 404 arm (which `StreamableHttpError::SessionExpired` supplies once an HTTP transport exists); the 400 arm requiring both `"code"\s*:\s*-32000` and `"message"\s*:\s*"Bad Request: Server not … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `partial`.** was `missing`. `live.rs` `session_expired_status` maps rmcp's `SessionExpired` to 404 and gates the retry; the 400-with-both-markers arm is unreachable because rmcp raises no typed 400 (documented there, fails closed). Upstream: `session-recovery.ts:47` (file unchanged since v2.32.1). |
| `MCP-135` | high | `hand-written` | **implemented** | withSessionRecovery retry wrapper | The whole wrapper is absent: the disabled/not-connected preconditions, `hadSessionId` captured before the call, the **live** config re-read after the failure, the 401 credential-cache invalidation running **before** the `isTerminatedSession` gate, the exactly-one retry, the `onNeedsAuth` hook, … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `live.rs:1276` `with_session_recovery` (disabled / not-connected preconditions, recovery, single retry). Upstream: `session-recovery.ts:93`. |
| `MCP-136` | n/a | `hand-written` | **not-applicable** | Tracker: what survives a restart | The one live issue this tracker names — agent-dir resolution — is still open and is owned by MCP-139: `cyrup-mcp` takes an already-resolved … |
| `MCP-137` | medium | `hand-written` | **implemented** | Status snapshot construction | **Re-ruled 2026-09-24 (second pass, both sides read): `missing` → `implemented`** — `live.rs` `create_mcp_status_snapshot` exists and meets the obligation below; later upstream additions are `MCP-554`/`MCP-508`/`MCP-515`. See *Second pass*. Prior ruling: `createMcpStatusSnapshot` does not exist. Needed: `MCP_STATUS_SNAPSHOT_VERSION = 1`, `FAILURE_BACKOFF_MS = 60_000`, `getActiveFailureAgeSeconds` (falsy `failedAt` → absent; `ageMs > 60_000` → absent; else `round(ageMs/1000)`), the per-server six-key object with `resourceCount`/`failedAgoSeconds` … |
| `MCP-138` | low | `extension-owned` | **implemented** | Publish the status snapshot | The channel only ever carries a default value: the sole production publishers are `runtime.rs:234` and `lifecycle.rs:1559`, both `McpStatusSnapshot::default()`. The payload type is also the wrong shape (see MCP-137), so a consumer reading it gets four empty vectors. No shutdown snapshot equivalent … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `live.rs` `create_mcp_status_snapshot` is published by `update_status_bar`, the `record_failure` expiry task, `runtime.rs`' tail and zero-enabled arm; `shutdown_state` publishes the empty snapshot. Upstream: `mcp-status.ts:75`. |
| `MCP-139` | high | `hand-written` | **implemented** | Metadata cache: path, schema, version, load and merge-save | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: Three real gaps. (1) **The agent-dir consolidation did not happen.** `npx_resolver::agent_dir` (/home/user/cyrup/crates/cyrup-ext/src/caps/proc/npx_resolver.rs, anchored on `caps::proc::host_home_dir`) and `mcp_direct_tools::resolve_agent_dir`/`home_dir` … |
| `MCP-140` | high | `hand-written` | **implemented** | Metadata cache: serialisers and reconstructors | The **serialisers are absent**. Nothing converts a live MCP tool/resource/prompt list into `CachedTool`/`CachedResource`/`CachedPrompt`: grepping all 19 files for a `ServerCacheEntry {` construction outside tests returns nothing (only dirs.rs:1254/1342, registration.rs:1981, ui.rs:5049 — all … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `dirs.rs` `serialize_tools` / `serialize_resources` / `serialize_prompts`, used by `live.rs` `update_metadata_cache` to build `ServerCacheEntry`. Upstream: `metadata-cache.ts:282-330`. |
| `MCP-141` | critical | `hand-written` | **implemented** | computeServerHash must hash all 14 fields; the in-tree reader hashes 11 | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: Three gaps, each independently fatal to the contract. (1) **The reader was not upgraded.** `cyrup_ext_subagents::exec::mcp_direct_tools::compute_mcp_server_hash` (/home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:531-584) still hashes **11** keys — `protocolVersion`, … |
| `MCP-142` | critical | `hand-written` | **implemented** (wave 1) | stableStringify emits the bare token `undefined`, not `null` | The **reader still emits `null`**: `mcp_direct_tools::stable_stringify` (/home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:796-825) maps `Value::Null => "null"`, and every absent field is materialised as `Value::Null` by `opt_str_value` (:587-589), by … |
| `MCP-143` | medium | `hand-written` | **partial** | interpolateEnvVars is missing its third pattern {env:NAME} | Both in-tree copies the unit names are unchanged. (1) `cyrup_ext::caps::proc::interpolate_env_vars_with` (/home/user/cyrup/crates/cyrup-ext/src/caps/proc.rs:148-156) is still `interpolate_braces` (:157) + `interpolate_dollar_env` (:180) — two patterns; grepping proc.rs for `{env:` returns nothing. … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`; severity `high` → `medium`.** restated: `credentials.rs:3341` and `mcp_direct_tools.rs:1805` carry all three patterns; `cyrup-ext/src/caps/proc.rs:139` (the WASM guest `proc.spawn` env path) still has two. Severity lowered: the MCP adapter's own paths are correct. Upstream: `utils.ts:134-139`. |
| `MCP-144` | high | `hand-written` | **implemented** | !/!! secret-expression semantics in hashed values | Two call sites still bypass it. (1) `mcp_direct_tools::interpolate_env_record` (/home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:696-712) calls plain `interpolate_env_vars` and additionally **drops non-string values** (`if let Some(text) = value.as_str()` at :706) where … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `mcp_direct_tools.rs:1744` `interpolate_env_record` routes every value through `interpolate_secret_expression` and fails on a non-string instead of dropping it. Upstream: `metadata-cache.ts:84-114`, `utils.ts` `interpolateSecretExpression`. |
| `MCP-145` | high | `hand-written` | **implemented** | isServerCacheValid including the throw-to-false rule | The throw arm is unreachable in practice: no fallible hasher is ever installed (`install_server_hasher` at registration.rs:754 has no production caller, and registration.rs:746-752 documents that without one the hash comparison is **skipped entirely**), and `compute_server_hash` cannot fail because … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `registration.rs:1003` `default_server_hasher` is fallible (`try_compute_server_hash(...).ok()`) and `is_server_cache_valid` answers `false` on `None`. Upstream: `metadata-cache.ts:116`. |
| `MCP-146` | critical | `hand-written` | **implemented** (wave 1) | Resource tool naming: read_ upstream vs get_ in the in-tree reader | The reader was **not** changed: `cyrup_ext_subagents::exec::mcp_direct_tools::resolve_direct_tool_names` still builds `format!("get_{}", resource_name_to_tool_name(name))` at /home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:466, and the test at :1044 still asserts … |
| `MCP-147` | medium | `hand-written` | **implemented** | Direct-tool selector parsing and the missing-server gate | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs:835-853 `parse_direct_tool_selectors` strips trailing `/`, then for a slash-bearing selector … |
| `MCP-148` | n/a | `rmcp` | **implemented** | The protocol layer is rmcp, client-only | /home/user/cyrup/crates/cyrup-mcp/Cargo.toml declares exactly the settled set: `rmcp = { version = "3.1.2", default-features = false, features = … |
| `MCP-149` | n/a | `hand-written` | **not-applicable** | Tracker: section 03 index and cross-section edges | No work item of its own; the three out-of-crate changes it indexes are all still outstanding and are filed under MCP-103, MCP-139 and … |

### 13d · Proxy modes

[`13d-mcp-proxy-modes.md`](13d-mcp-proxy-modes.md) — 36 units, 1 missing, 4 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-151` | high | `host-verb` | **implemented** | Register the `mcp` tool with the exact JSON Schema | None for the unit's obligation. Code-health note only: the schema literal exists twice (`proxy::mcp_tool_schema` and … |
| `MCP-152` | high | `hand-written` | **implemented** | Port `buildProxyDescription` and re-register on change | The hand-written half of MCP-152 is complete and tested. The description is built with `write!`-style assembly in TWO byte-identical copies … |
| `MCP-153` | high | `hand-written` | **implemented** | Port mode dispatch: precedence, args coercion, init gate | Port complete, but the ported dispatcher is not reachable in production: the tool actually registered is `registration::ProxyTool`, whose `execute` … |
| `MCP-154` | medium | `hand-written` | **implemented** | Port `executeStatus` | proxy.rs:1811 `execute_status`: the six-rung ladder (disabled → connected → needs-auth → failed → cached → not connected) over … |
| `MCP-155` | medium | `hand-written` | **implemented** | Port `executeList` | proxy.rs:1912 `execute_list`: `not_found` with empty `tools`/`count`, `disabled_result("list", …)`, the 300-char preview via … |
| `MCP-156` | low | `hand-written` | **implemented** | Port `executeInstructions` | proxy.rs:2013 `execute_instructions` checks `not_found` → `server_disabled` → cached instructions (`<s> instructions:\n\n<text>` with … |
| `MCP-157` | medium | `hand-written` | **implemented** | Port `executeDescribe` | proxy.rs:2060 `execute_describe`. Verified line-by-line against upstream /home/user/cyrup/tmp/pi-mcp-adapter/proxy-modes.ts `executeDescribe`: … |
| `MCP-158` | high | `hand-written` | **implemented** | Port `executeSearch` match selection | proxy.rs:2171 `execute_search` selection half, diffed against upstream proxy-modes.ts `executeSearch`. Disabled-`server` short-circuit first (:2192); … |
| `MCP-159` | medium | `hand-written` | **implemented** | Port the regex search path onto a linear-time engine | Every item in MCP-159's **verify** list exists. (1) The re-specified catastrophic-backtracking case: proxy.rs:5471-5481 compiles `(a+)+$`, runs it, … |
| `MCP-160` | medium | `hand-written` | **implemented** | Port `executeSearch` rendering, pagination footer and connecting hint | proxy.rs:2276-2420 rendering half, diffed byte-for-byte against upstream proxy-modes.ts. Zero results: `connecting` computed as `[server]` iff … |
| `MCP-161` | high | `hand-written` | **implemented** | Port `executeConnect` | Runs only against `ProxyEnv`'s `connect`/`reconnect`; the trait has no production implementor (see MCP-164), so this mode is exercised solely by … |
| `MCP-162` | high | `hand-written` | **implemented** | Port `attemptAutoAuth` and the single-shot latch | proxy.rs:2611 `attempt_auto_auth` reproduces the ladder: `settings.auto_auth()` opt-in, missing/disabled/non-OAuth ⇒ Skipped, `resolve_server_url` … |
| `MCP-163` | critical | `hand-written` | **implemented** | Port `executeCall`'s resolution state machine (phases 1-5) | proxy.rs:2949-3283 `execute_call` phases 1-5. Fail-closed resolution is present and is the sentinel form, not first-match: `SingleMatch` … |
| `MCP-164` | high | `hand-written` | **implemented** | Port `executeCall`'s invocation paths and result shaping | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: The rmcp invocation itself does not exist. `ProxyEnv::call_tool` (proxy.rs:1397) and `ProxyEnv::read_resource` (:1410) are declared with doc comments naming `Peer::send_request_with_option(...)` → `RequestHandle::cancel(reason)` and `Peer::read_resource`, but a workspace grep for … |
| `MCP-165` | medium | `hand-written` | **implemented** | Port `executeCall`'s error taxonomy | No behavioural test exercises any of the three arms. `grep -n 'url_elicitation\|SessionRecoveryAuthRequired\|ProxyCallError'` over the test module … |
| `MCP-167` | medium | `hand-written` | **implemented** | Port `executeAuthStart` and `formatManualAuthInstructions` | proxy.rs:2443 `format_manual_auth_instructions` builds the literal vector and applies `.filter(Boolean)` semantics via `lines.retain(\|l\| … |
| `MCP-168` | medium | `hand-written` | **implemented** | Port `executeAuthComplete` | The unit's two named unit tests are absent: nothing asserts all three input keys are accepted, and nothing asserts a non-`"authenticated"` status … |
| `MCP-169` | high | `hand-written` | **implemented** | Freeze the `details.error` vocabulary as a conformance table | proxy.rs:180 `#[non_exhaustive] #[serde(rename_all = "snake_case")] enum McpErrorCode` with all 32 variants, `McpErrorCode::ALL` (:249) as a … |
| `MCP-170` | high | `extension-owned` | **implemented** | Use insertion-ordered maps for servers and metadata | Integration note (not this unit's gap): `ProxyCtx` holds its own `tool_metadata` separate from `McpState::tool_metadata`, documented at … |
| `MCP-171` | low | `open-decision` | **implemented** | Decide the `localeCompare` tie-break | None functionally. Performance note worth a follow-up: `locale_compare` constructs a fresh `feruca::Collator` on every comparison (config.rs:3791 … |
| `MCP-172` | high | `hand-written` | **implemented** | Port `normalizeSearchText` and `tokenize` | proxy.rs:767 `normalize_search_text` is a hand-written char scanner doing the three steps in order — camelCase split before lowercasing, advancing … |
| `MCP-173` | high | `hand-written` | **implemented** | Port `scoreToolMatch` field scoring | proxy.rs:902 `score_tool_match` with `WEIGHT_NAME/ORIGINAL_NAME/SERVER/DESCRIPTION = 12/10/8/5` (proxy.rs:118-124) and `MIN_STEM_LENGTH = 4` (:115). … |
| `MCP-174` | low | `hand-written` | **partial** | Port keyword scoring and `resolveSearchKeywords` | One malformed-config divergence remains. Upstream's `resolveSearchKeywords` skips only the offending key (`if (!Array.isArray(values)) continue;`), but cyrup deserialises `search_keywords` with `#[serde(deserialize_with = "lenient")]` (config.rs:715), and `lenient` (config.rs:479) drops the … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`; severity `medium` → `low`.** unchanged: `config.rs` `search_keywords` is `IndexMap<String, Vec<String>>` under `lenient`, so one non-array value or non-string element drops the whole map. Severity lowered: malformed config only. Upstream: `search-ranking.ts:69-74` skips only the bad key/element. |
| `MCP-175` | high | `hand-written` | **implemented** | Port the coverage gate and final bonuses | proxy.rs:962-987. `full_coverage` is the integer comparison `matched == total`, never a float equality; the gate is `if !phrase_matched && (if total … |
| `MCP-176` | high | `hand-written` | **implemented** | Port `rankToolMatches` and `paginate` | proxy.rs:1073 `rank_tool_matches` walks the IndexMap in insertion order, skips disabled and non-matching servers, computes `has_keywords = … |
| `MCP-177` | low | `hand-written` | **implemented** | Port keyword resolution inside the regex search path | No test asserts a keyword-only regex match returns with `score: 0` (upstream's "matches keywords in regex search mode"). Folded into MCP-196. |
| `MCP-178` | high | `open-decision` | **implemented** | Port `rankSuggestions`, and settle the `getServerPrefix` conflict | MCP-178 is verdict **open-decision**, and the Rust has already picked a side — option (a): cyrup-mcp implements the adapter's FOUR-mode, … |
| `MCP-191` | high | `open-decision` | **partial** | `auth-start` / `auth-complete` derive no distinct permission targets | Neither of the unit's deliverables exists. (1) The hazard is undocumented: a grep for `MCP-191` across crates/cyrup-mcp/src returns zero hits — the only unit id in the section with no reference in the code — and nothing in cyrup-permission-system records that `auth-start`/`auth-complete` fall … — **Re-read 2026-09-14 at `9aeba769` — still `partial`.** `grep -rn 'MCP-191' crates/cyrup-mcp/src` is zero hits, so this is still the only unit id in its section with no in-code reference and deliverable (1), documenting the hazard, is unmet; neither deliverable has moved. **Scheduling note:** the v2.33.0 window makes the auth surface this row is about *more* permission-relevant, not less — `proxy-modes.ts:134-146` now emits an `mcp-oauth-status` message with `nextAction: { connect: serverName }`, a new action the `auth-start` / `auth-complete` target question would have to cover. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: an open decision; `MCP-191` is now cited in code (`registration.rs:279`, `lib.rs:191`) as the suffix-vs-prefix reconciliation, but no decision on auth-start/auth-complete targets is recorded. Upstream: —. |
| `MCP-192` | medium | `host-verb` | **implemented** | Satisfy the permission system's contracts on the `mcp` tool | The cyrup-it half of "verify" (with `mcp` denied, assert the guideline disappears from the system prompt — the assertion that actually catches a … |
| `MCP-193` | medium | `host-addition` | **implemented** | Reach `register_late_tool` from a native extension | HA-1 is unbuilt. Needed: either `NativeExtension::set_ext_host(Weak<ExtensionHost>)` called beside the existing `set_host_services`, or a defaulted `HostServices::register_late_tool` backed by a late-attached sink (the `set_overlay_sink` / `attach_dynamic_tools` precedent in … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** HA-1 built — `NativeExtension::set_late_registrar` / `HostLateRegistrar` (as `MCP-037`). Upstream: —. |
| `MCP-194` | low | `open-decision` | **implemented** | Tool-schema property order is alphabetised by `serde_json` | **Evidence stale 2026-09-24 (second pass):** `serde_json/preserve_order` is on workspace-wide (`Cargo.toml`, since `0aefd08`), so the schema now serialises in declaration order and the alphabetisation this row accepts no longer happens; status unchanged, doc fix is `MCP-582`. Decision (c) accepted and made visible. proxy.rs:3965-3971 documents that the workspace builds `serde_json` without `preserve_order` so … |
| `MCP-195` | medium | `hand-written` | **implemented** | Port the ranking conformance suite (11 cases) | 11/11. Upstream __tests__/search-ranking.test.ts has exactly 11 `it(` cases (8 in `describe("search ranking")` including the two paginate assertions, … |
| `MCP-196` | high | `hand-written` | **partial** | Port the proxy-mode conformance suites (47 cases) | Not the named 46-ported + 1-re-specified target, and the expensive third of the suite is absent. Concretely missing, by upstream case name: proxy-modes-auto-auth's "runs URL elicitations returned by proxy tool calls", "rethrows proxy auto-auth cancellation", "surfaces aborted proxy tool calls via … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `proxy/testsupport.rs` `FakeEnv` exists; the named auto-auth cases (URL elicitation from proxy calls, cancellation rethrow, aborted calls) have no tests. Upstream: `__tests__/proxy-modes-auto-auth.test.ts`. |
| `MCP-197` | medium | `host-verb` | **implemented** | Port the render binding, including the `toolResultRendering` fork | `tool_render_kind(settings)` (registration.rs:1472) returns `ToolRenderKind::Default` only for `Some(ToolResultRendering::Boxed)` and `SelfRendered` … |
| `MCP-198` | medium | `hand-written` | **implemented** | Port the cross-server candidate-collision set behind the description's counts | proxy.rs:3737 `collision_candidates` builds the cross-server candidate set once per `build_proxy_description` call: it iterates every configured, … |
| `MCP-199` | low | `host-verb` | **implemented** | Wire native-tool detection to `all_tool_names` | REFUTED — the claim rests on a misreading. owner.rs:408 is NOT 'a stale-generation no-op that always returns None': `OwnedServices` is … |

### 13e · Tool registration, naming, approval, output guard

[`13e-mcp-tools.md`](13e-mcp-tools.md) — 53 units, 7 missing, 8 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-200` | high | `hand-written` | **implemented** | The four-mode server-prefix / tool-name formatter | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs: `sanitize_server_prefix` (code-point walk, `_{:x}_` escape, `preserve_provider_valid` flag), … |
| `MCP-201` | high | `hand-written` | **implemented** | getToolNameCandidates, including the legacy arm | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `tool_name_candidates(tool, server, prefix, include_legacy) -> HashSet<String>` plus … |
| `MCP-202` | high | `hand-written` | **implemented** | matchesToolPattern / matchesToolSelector / isToolAllowed | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs: `glob_to_regex` (escape set `[.+^${}()\|[]\\]`, `*`→`.*`, `?`→`.`, anchored), `is_glob`, … |
| `MCP-203` | medium | `hand-written` | **implemented** | resourceNameToToolName and the read_ resource base name | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `resource_name_to_tool_name` (non-alphanumeric → `_`, run-collapse, trim both edges, lowercase, … |
| `MCP-204` | medium | `hand-written` | **implemented** | resolveServerFromToolName with its ambiguity fail-safe | No pure `resolve_server_from_tool_name(tool_name, server_names, prefix)` exists: no `None` for `ToolPrefix::None`, no longest-prefix winner, and no ambiguity fail-safe returning `None` when two different servers produce the same winning prefix. The two existing prefix scans pick a first match … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `registration.rs:283` (same function as `MCP-073`). Upstream: `types.ts:904`. |
| `MCP-205` | high | `open-decision` | **not-applicable** | Reconcile mcp_direct_tools.rs with pi-mcp-adapter naming | Verdict is **open-decision**, and no ruling has been recorded: registration.rs:179 says verbatim "MCP-205, unresolved" and proxy.rs:419 says … |
| `MCP-206` | low | `hand-written` | **implemented** | sanitizePromptName / formatPromptCommandName | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `sanitize_prompt_name` (`[^A-Za-z0-9_-]+` run → one `_`, trim `[_-]`, `"prompt"` when empty, … |
| `MCP-207` | high | `hand-written` | **implemented** | buildToolMetadata | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: The whole live `tools`+`resources` → `Vec<ToolMetadata>` pipeline is absent: no `failedTools` accumulation for unnamed tools, no post-visibility `seenNames` reservation, no `description ?? ""`, no resource arm with `read_` + `Read resource: <uri>` + `resourceUri`, and none of the v2.26.1 shape the … |
| `MCP-208` | medium | `hand-written` | **implemented** | extractUiToolVisibility / isUiToolVisibleToModel (kept half) | `extractUiToolVisibility(tool._meta)` is absent — nothing walks a live `_meta.ui.visibility` (grep for `_meta` / `"visibility"` / `extract_ui` across crates/cyrup-mcp/src finds no reader). The fail-closed extraction cases (`_meta.ui` non-object or an array → visible; `visibility` present but … — **Re-ruled 2026-09-14 at `9aeba769`: the recorded detail is refuted, `partial` → `implemented`.** Both bodies read arm for arm. The recorded claim '`extractUiToolVisibility(tool._meta)` is absent — nothing walks a live `_meta.ui.visibility`' is false: `registration.rs:1495 fn extract_ui_tool_visibility` reproduces `ui-tool-visibility.ts:3-18` exactly — absent or non-object meta and a non-object-or-array `ui` → None (visible); `visibility === undefined` → None; a non-array → `Some(vec![])`; one entry that is neither `"model"` nor `"app"` voids the whole list to `Some(vec![])`; dedupe preserving first-occurrence order. `is_ui_tool_visible_to_model` (registration.rs:943-946) is `visibility === undefined \|\| includes("model")`. Production readers at registration.rs:1302 and :1376, plus the cache-side reader at dirs.rs:764; the fail-closed direction is stated and correct at registration.rs:1491-1494. Upstream's file is unchanged in the v2.32.1..v2.33.0 window (consumed at tool-metadata.ts:93-96 and metadata-cache.ts:297). |
| `MCP-209` | n/a | `cut` | **not-applicable** | getToolUiResourceUri / extractToolUiStreamMode and the UI spec fields | Cut 2, correctly honoured: /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `DirectToolSpec` has no `uiResourceUri`/`uiStreamMode` and … |
| `MCP-210` | medium | `hand-written` | **implemented** | findToolByName, getToolNames, totalToolCount | /home/user/cyrup/crates/cyrup-mcp/src/proxy.rs `find_tool_by_name` (exact `name` match first, then `-`→`_` normalised on both sides) at proxy.rs:735, … |
| `MCP-211` | high | `hand-written` | **missing** | formatSchema and its four helpers | The whole pretty-printer is unwritten: `formatSchema`'s five-way dispatch, `formatProperty` (non-object early return, parts joined by one space, recursion at `indent + " "`), `formatType`'s six ordered rules including `Object.hasOwn(schema,"const")` (must be `Map::contains_key`), … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`; severity `medium` → `high`.** unchanged, and worse than its severity said: `live.rs` `RuntimeEnv::format_schema` returns the literal `(schema rendering is not wired — MCP-211)`, and with `render_ts_shape` = `None` (`MCP-091`) every `mcp({describe})` answers `Parameters:` followed by that marker, and every failed call's `Expected parameters:` suffix carries it too. A model on the proxy surface is never shown a tool's parameters. Upstream: `tool-metadata.ts:168`; used at `proxy-modes.ts:766`, `:826`, `:1491`, `:1721`. |
| `MCP-212` | critical | `hand-written` | **implemented** | resolveDirectTools, including the builtin-collision drop | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `resolve_direct_tools`: `cache == None` → empty; iterates `config.mcp_servers` in file order … |
| `MCP-213` | high | `hand-written` | **implemented** | buildProxyDescription | One text delta against the plan's stated port literal: the header reads `Non-MCP cyrup tools should be called directly` where 13e §6 specifies … |
| `MCP-214` | high | `hand-written` | **implemented** | The direct-tool execute state machine | None of §7's ordered state machine exists: no owned-signal composition, no `lazyConnect`, no auto-auth-on-`needs-auth`, no connection assertion, no approval call, no request options, no `tools/call` / `resources/read`, no content transform → guard hand-off, no error/abort mapping and no in-flight … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `dispatch.rs` `McpDispatch::call_direct` → `proxy::execute_call` with the server pinned and `ApprovalOrigin::for_direct_tool`. Upstream: `direct-tools.ts:125`. |
| `MCP-214a` | high | `hand-written` | **implemented** | recoverAuthConnection and the per-server request options | Nothing wires either into a direct-tool call, because the executor does not exist (MCP-214). `ProxyEnv::call_tool` / `read_resource` (proxy.rs:1397, 1410) take a recovery callback but have no production implementor, and `build_request_options` has no production call site (grep shows only … — **Re-ruled 2026-09-14 at `9aeba769`: the recorded cause is refuted, `partial` → `implemented`; severity stays `high`.** The recorded detail rested on one stated cause — 'Nothing wires either into a direct-tool call, because the executor does not exist (MCP-214). `ProxyEnv::call_tool` / `read_resource` … take a recovery callback but have no production implementor, and `build_request_options` has no production call site.' All three clauses are false. The direct-tool executor exists as `dispatch.rs::McpDispatch::call_direct` (dispatch.rs:309), documented phase-by-phase against `direct-tools.ts:380-545` and routing into `proxy::execute_call`. `live.rs` is the production `ProxyEnv` implementor: `call_tool` at live.rs:1467 and `read_resource` at live.rs:1528 both take `recovery: &AuthRecovery<'_>` and both call `self.request_options(server)` (live.rs:1479, :1534; helper at :1377-1381), which is `server_manager::get_request_options` (server_manager.rs:1734) → `runtime::build_request_options` (runtime.rs:3279). The `ProxyEnv` verb's call site is proxy/call.rs:930. **Falsification condition, because this ruling rests on the executor-and-implementor chain rather than on a line-for-line read of `recoverAuthConnection`'s five arms:** reopen if `AuthRecovery::recover` is shown NOT to fire on the needs-auth arm of a direct-tool `tools/call` — i.e. if a direct tool against a server whose token expired mid-call returns a needs-auth envelope without attempting re-auth, where `direct-tools.ts:474` / `:508` / `:539` / `:548`'s `onNeedsAuth: recoverAuthConnection` would have. |
| `MCP-215` | medium | `hand-written` | **partial** | attemptDirectAutoAuth and the auth message templates | The direct-tool flavour has no caller. /home/user/cyrup/crates/cyrup-mcp/src/oauth.rs defines `msg_auth_required_direct_tools` (the `MCP server "x" requires OAuth…` literal, distinct from the proxy's `Server "x" …`) and `msg_auto_auth_failed`, and a grep shows both are referenced only from a doc … — **Re-read 2026-09-14 at `9aeba769` — still `partial`, and slightly worse than recorded.** The row says cyrup defines `msg_auth_required_direct_tools` and `msg_auto_auth_failed`, both referenced only from a doc comment. At 9aeba769 `msg_auto_auth_failed` does not exist at all (zero hits) and `msg_auth_required_direct_tools` (oauth.rs:3952) is still referenced only from the doc comment at oauth.rs:3926 — so `attemptDirectAutoAuth` has no caller **and** one of its two message templates has regressed out of the tree. Upstream still has the direct-tool auto-auth path: `direct-tools.ts:474`, `:508`, `:539`, `:548` wire `recoverAuthConnection` as `onNeedsAuth`. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `oauth.rs:3952` `msg_auth_required_direct_tools` has no caller; direct calls surface the proxy's `Server "x" requires OAuth…` text. Upstream: `direct-tools.ts:40-111`. |
| `MCP-216` | medium | `host-verb` | **implemented** | The direct-tool registration shape | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `DirectTool::new` computes and owns `label = "MCP: {original}"`, `description` falling back to … |
| `MCP-217` | high | `host-addition` | **implemented** | Post-init dynamic tool (and command) registration | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: No fingerprint-diff `syncDirectTools`, no `deactivateTools` fallback pass, no `syncProxyTool` description refresh at runtime and no `syncToolSurface` entry point. The state slots exist unused on `McpExtension` (`registered_direct_tools`, `fallback_deactivated_tools`, `proxy_tool_description` in … |
| `MCP-217a` | low | `hand-written` | **partial** | freezeDirectTools and the frozen-surface escape hatches | Neither accessor has a production caller (grep for `freeze_direct_tools()` / `direct_tools_frozen()` across crates/cyrup-mcp/src returns only the definitions). Missing: setting the latch immediately after the initial post-init sync; the freeze log line `MCP: direct tools frozen after initial sync — … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`; severity `medium` → `low`.** restated: the latch and log exist (`extension.rs` commit step 9, `freeze_direct_tools`), but `commands.rs` `show_status` lacks upstream's `Direct tools frozen; active registrations may differ from current metadata.` line (present since v2.32.1), and the log keeps the pre-v2.32.1 wording. Upstream: `commands.ts:195-197`; `index.ts:963`. |
| `MCP-217b` | low | `host-verb` | **implemented** | The tool-surface refresh notification | No `MCP: direct tools refreshed (+{added}, ~{updated}, -{deactivated})` toast, no added/updated/deactivated counting, and no `ctx.hasUI`-equivalent guard (i.e. emit only when an interactive surface is present). Depends on MCP-217 producing the counts. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `extension.rs` `sync_tool_surface` notifies `MCP: direct tools refreshed (+a, ~u, -d)` when the count is non-zero. Upstream: `index.ts:611-641`. |
| `MCP-218` | medium | `hand-written` | **implemented** | syncProxyTool's registration/deactivation predicate | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `should_register_proxy_tool` implements the three-way OR (`disableProxyTool !== true` \|\| no … |
| `MCP-219` | medium | `hand-written` | **implemented** | MCP_DIRECT_TOOLS, __none__ and parseDirectToolSelectors | /home/user/cyrup/crates/cyrup-mcp/src/runtime.rs `DIRECT_TOOLS_NONE_SENTINEL = "__none__"` and `direct_tools_override` (comma split, trim, … |
| `MCP-220` | high | `hand-written` | **implemented** | transformMcpContent for every standard MCP content type | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `transform_mcp_content`: text, image (`image/png` default — the only non-text output), `resource` … |
| `MCP-221` | medium | `hand-written` | **implemented** | transformMcpResourceContents | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `transform_mcp_resource_contents`: string `text` wins, then string `blob` materialises, else … |
| `MCP-222` | high | `hand-written` | **implemented** | resolveMcpResultContent and the structured-content fallback | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `resolve_mcp_result_content`: transforms `result.content` when it is an array, and only when the … |
| `MCP-223` | high | `hand-written` | **implemented** | Binary-resource materialization with its four limits | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `MaterializedResources::materialize`: cancelled scope → `runtime stopped`; `base64_decoded_len` … |
| `MCP-224` | medium | `hand-written` | **implemented** | The materialized-resource cleanup drain and retry | No pending-cleanup set, no per-directory attempt counters capped at `MAX_CLEANUP_RETRY_ATTEMPTS = 3`, no `CLEANUP_RETRY_DELAY_MS = 30_000` timer guarded by "already pending or nothing retryable", no timer-clear when the set empties, and no aggregate error carrying `Vec<io::Error>` with the message … — **Re-ruled 2026-09-14 at `9aeba769`: the recorded detail is refuted in full, `partial` → `implemented`.** Both bodies read. Every clause — 'No pending-cleanup set, no per-directory attempt counters capped at `MAX_CLEANUP_RETRY_ATTEMPTS = 3`, no `CLEANUP_RETRY_DELAY_MS = 30_000` timer guarded by "already pending or nothing retryable", no timer-clear when the set empties' — is false. renderers.rs:126 and :129 declare both constants; `PendingCleanup` carries `directories` (the per-path attempt counters) and `retry: Option<JoinHandle>`; `has_retryable_cleanup_directory` (renderers.rs:983-989) is `tool-registrar.ts:61-66` verbatim; `schedule_pending_cleanup_retry` (renderers.rs:1002-1040) is `tool-registrar.ts:68-82` including the exact guard `if pending.retry.is_some() \|\| !has_retryable_cleanup_directory(&pending) { return }` and the decision to spend the attempt counters at **schedule** time rather than drain time — which is what upstream's loop at tool-registrar.ts:69-72 does. The `Handle::try_current()` guard is a forced Rust-side addition (`tokio::spawn` panics off-runtime and the crate denies `clippy::panic`); it is stated rather than hidden, and it costs no attempt. `tool-registrar.ts` is untouched in the v2.32.1..v2.33.0 window. |
| `MCP-225` | medium | `hand-written` | **implemented** | resolveMcpOutputGuardOptions and the MCP_OUTPUT_GUARD kill switch | The env variable is never actually read in production: grep for `MCP_OUTPUT_GUARD` across crates/cyrup-mcp/src finds only doc comments, and grep for `output_guard(` finds no production call site at all (the guard is reached through the `ProxyEnv::guard_mcp_output` seam, which has no production … — **Re-ruled 2026-09-14 at `9aeba769`: the recorded cause is refuted, `partial` → `implemented`.** The recorded detail rested on one stated cause — 'The env variable is never actually read in production: grep for `MCP_OUTPUT_GUARD` … finds only doc comments, and grep for `output_guard(` finds no production call site at all (the guard is reached through the `ProxyEnv::guard_mcp_output` seam, which has no production implementor).' That is false: `live.rs:1767` — inside the production `ProxyEnv` implementor, the same one the 2026-09-04 pass itself identified as production for MCP-164 / MCP-217 / MCP-232 — reads `std::env::var("MCP_OUTPUT_GUARD")` and passes it to `config::McpSettings::output_guard(env)` (config.rs:1457), which is the `resolveMcpOutputGuardOptions` port. Upstream's shape is unchanged this window: `mcp-output-guard.ts:85-94` is still `enabled: envKillSwitch("MCP_OUTPUT_GUARD") ?? configured !== false` over the same three `positiveInt` tunings. Noted and deliberately left with its owner: the two defaults this resolves against are the ones MCP-226 shows are no longer literals upstream — that is MCP-226's row, not this one. |
| `MCP-226` | high | `hand-written` | **implemented** | guardMcpOutput's normalize / affix / passthrough path | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `guard_mcp_output` steps 1–2 plus `sanitize_content` (image mime trimmed then `take_utf16(.., … |
| `MCP-227` | high | `hand-written` | **implemented** | The truncation arithmetic and notice format | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs: `text_stats` (0 lines for empty), `reserve_budget` (charges `"\n\n" + notice` against both caps, … |
| `MCP-228` | high | `hand-written` | **implemented** | saveArtifact's private-directory spill | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `save_artifact(kind, text)`: `make_private_temp_dir("pi-mcp-output-")` (`DirBuilder::mode(0o700)`, … |
| `MCP-229` | medium | `hand-written` | **implemented** | boundMcpResult and the result-summary schema | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `bound_mcp_result` (raw kept under the threshold), `summarize_mcp_result` (spill via … |
| `MCP-230` | medium | `hand-written` | **implemented** | Record the output guard's actual security contract | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs module docs, section "The output guard's security contract, stated exactly (MCP-230)": states it … |
| `MCP-231` | high | `hand-written` | **implemented** | isToolCallApprovalRequired | The whole predicate is unwritten: server-level `approveTools` overriding the global on presence, `true` → always required, non-array/empty → not required, the legacy-alias disambiguation reusing MCP-201/MCP-202, the explicit injection of the first non-bare current candidate with `-`→`_` into the … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `proxy/approval.rs:75` `is_tool_call_approval_required` (presence override, `true`, list, legacy disambiguation). Upstream: `tool-approval.ts:24-60`. |
| `MCP-232` | high | `host-verb` | **partial** | ensureToolCallApproved and the approval dialog | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: The gate itself is unimplemented — `ensure_tool_call_approved` exists only as a `ProxyEnv` trait method (proxy.rs:1488) with a test-only implementor. Missing: the cache lookup/insert against `approved_tool_calls`, the headless check performed **before** calling `select` (so a cancelled dialog and a … — **Re-ruled 2026-09-14 at `9aeba769`: `implemented` → `partial`, severity `critical` → `high`.** The 2026-09-04 re-ruling is invalidated on a security-relevant axis by the v2.32.1..v2.33.0 window; both bodies were read rather than the changelog. At v2.33.0 the approval cache key is **four** components — `${serverName}\0${originalToolName}\0${definitionHash}\0${argsHash}` (`session-approvals.ts:55-61`), where `definitionHash = sha256(stableStringify({originalName, inputSchema, resourceUri, uiResourceUri}))` (`session-approvals.ts:43-52`). cyrup's `state.rs:392 approval_cache_key(server, original_tool, args)` is the three-component v2.32.1 form and hashes only the arguments; it is consumed at `proxy/approval.rs:303`. The census's other half — `tool-approval.ts:137-152` now calling `requestBrokerApproval` **before** consulting the cache — is moot for cyrup, whose broker is MCP-233's cut, and `proxy/approval.rs:296-301` says so explicitly. The `definitionHash` half is **not** moot. **Falsification / reopen-as-closed condition:** close the moment `state.rs::approval_cache_key` takes the tool metadata and folds a sha256 over `{originalName, inputSchema, resourceUri, uiResourceUri}` into the key between the tool name and the args hash. Concretely: a session where the user picks *Allow for session* for tool T with args A, the server then re-advertises T with a changed `inputSchema` (a new destructive parameter), and the same args A are sent again — upstream v2.33.0 re-prompts, cyrup at 9aeba769 does not. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `state.rs:392` `approval_cache_key(server, original_tool, args)` — three components, no definition hash. Upstream: `session-approvals.ts` definition-hash key. |
| `MCP-233` | medium | `host-verb` | **implemented** | Drop the approval broker; before_tool_call is the broker | The broker is deliberately absent and the decision is recorded in code: /home/user/cyrup/crates/cyrup-mcp/src/lib.rs:80 ("The fifth cut field is … |
| `MCP-234` | high | `open-decision` | **not-applicable** | Direct MCP tools do not reach the mcp permission category | Verdict is **open-decision** with no ruling recorded, so by the class rule this is not-applicable rather than outstanding work. Nothing behavioural … |
| `MCP-235` | high | `hand-written` | **implemented** | sanitizeTerminalText / stripOscSequences | /home/user/cyrup/crates/cyrup-mcp/src/ui.rs `strip_osc_sequences` (hand-written scanner over both `ESC ]` and C1 `U+009D` introducers, terminated by … |
| `MCP-236` | medium | `hand-written` | **implemented** | Give the mcp tool its prompt guideline | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `PROXY_TOOL_PROMPT_GUIDELINE` holds the lowercase literal and `impl Tool for ProxyTool { fn … |
| `MCP-237` | medium | `hand-written` | **implemented** | The call-row formatters | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `format_mcp_proxy_tool_call_lines` (all seven surviving branches in order, with `@ server`, … |
| `MCP-238` | low | `host-verb` | **implemented** | resolveMcpToolRenderOptions and the renderShell selection | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `resolve_mcp_tool_render_options` over /home/user/cyrup/crates/cyrup-mcp/src/config.rs … |
| `MCP-239` | medium | `hand-written` | **implemented** | collectCollapsedResultLines / formatMcpToolResultLines / blockToLines | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `collect_collapsed_result_lines` (UTF-16 `utf16_len` budget, `indexOf`-style split without … |
| `MCP-240` | low | `hand-written` | **implemented** | formatMcpToolResultIdentity | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `format_mcp_tool_result_identity`: `None` unless `details.mode == "call"`, server from `server` … |
| `MCP-241` | low | `hand-written` | **implemented** | The compact result row without a render width | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `compact_result_widget` — drops the trailing `"…"` when truncated, prefixes `"{title} → "` on the … |
| `MCP-242` | low | `host-verb` | **implemented** | Expanded rendering without a per-row expansion flag | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `render_mcp_tool_result` computes `expanded = tools_expanded \|\| is_truthy(details.error)`; … |
| `MCP-243` | low | `hand-written` | **implemented** | The compact call-row suppression has no cyrup equivalent | The recommended "drop the stash entirely" was taken: /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `render_call` always returns a drawn call row … |
| `MCP-244` | low | `hand-written` | **implemented** | The renderer contract carries no theme | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs emits only `text` / `truncated-text` widget nodes (`text_widget`, `truncated_text_widget`); … |
| `MCP-245` | low | `extension-owned` | **not-applicable** | Width-aware truncation is not needed | Dissolved by the plan ("nothing to build"), and the tree matches: no width crosses `NativeExtension::render_result`, … |
| `MCP-246` | low | `extension-owned` | **implemented** | Route the five collision/advisory warnings | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `resolve_direct_tools` emits all five as `tracing::warn!`: `MCP: skipping direct tool "…" … |
| `MCP-247` | high | `hand-written` | **implemented** | The mcp proxy tool's parameter schema | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `proxy_tool_parameters()` — twelve optional properties, no `required` array; `args` is the … |
| `MCP-248` | n/a | `hand-written` | **not-applicable** | Tracker: registration, approval, guard and rendering | Tracker unit. State of the critical path it indexes, from this audit: MCP-200 ✓, MCP-201 ✓, MCP-202 ✓, MCP-203 ✓, MCP-207 ✗, MCP-212 ✓, MCP-216 ✓, … |
| `MCP-249` | high | `hand-written` | **partial** | Freeze the details schema this subsystem emits | Two gaps. (1) `server_unavailable` — emitted by upstream direct-tools.ts step 7 (`details: { error: "server_unavailable", server }`, verified at /home/user/cyrup/tmp/pi-mcp-adapter/direct-tools.ts:420) — has no enum variant and no producer; a grep for `server_unavailable` across … — **Re-read 2026-09-14 at `9aeba769` — still `partial`.** The first recorded gap is re-confirmed at the current tag: `grep -rn 'server_unavailable' crates/cyrup-mcp/src` is zero hits — no enum variant and no producer — while upstream still emits `details: { error: "server_unavailable", server }` from `direct-tools.ts` step 7. **Scheduling note:** `direct-tools.ts` moved +37 in the v2.32.1..v2.33.0 window, so re-read the step-7 site at v2.33.0 before working the row. The error code survives the window, but the full details schema was **not** re-derived, so this row's second gap is untouched by this re-read. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `server_unavailable` has 0 hits. Upstream: `direct-tools.ts:192-196`. |

### 13f · Credentials and keychain storage

[`13f-mcp-credentials.md`](13f-mcp-credentials.md) — 41 units, 0 missing, 5 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-250` | high | `hand-written` | **implemented** | The `AuthEntry` record and its strict normalization | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs — `AuthEntry` (line 596) and `StoredClientInfo` (line 448) are … |
| `MCP-251` | high | `hand-written` | **implemented** | Derive the keychain account and legacy directory from `sha256-<hex>` of the … | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `hex_sha256` (732), `auth_entry_account` (761) = `format!("sha256-{}", … |
| `MCP-252` | high | `extension-owned` | **implemented** | Add the OS keyring backend and map its error taxonomy | Two secondary points, neither behaviour-changing today: (a) `keyring::Entry::store_status()` is named in a doc comment (line 963) but never called — … |
| `MCP-253` | high | `hand-written` | **implemented** | The chunking manifest write path | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AuthEntryChunkManifest` (784, declaration order … |
| `MCP-254` | high | `hand-written` | **implemented** | The chunked read path and the `AuthStoreError` taxonomy | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `StoreOp` (254) with `verb()`/`preposition()`, `AuthStoreError` (336) with … |
| `MCP-255` | medium | `hand-written` | **implemented** | Stale-chunk cleanup ordering and its error-swallowing | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `existing_chunk_manifest` (2258, swallow-all `.ok()??`), `try_remove_chunk_payloads` (2282, `let … |
| `MCP-256` | high | `hand-written` | **implemented** | The legacy plaintext import-and-delete path (and the record translator) | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AuthStorageOptions::from_settings` (1802), `McpAuthStore::auth_base_dir` (2126) delegating to … |
| `MCP-257` | high | `hand-written` | **implemented** | The process-lifetime auth-entry cache and its three external invalidation points | None for this unit's own obligation (the eviction primitive). Informational: `invalidate_cache` has no production caller anywhere in … |
| `MCP-258` | medium | `extension-owned` | **implemented** | Fault-injection backends behind an explicit selector | Mechanism divergence from the plan text, behaviourally equivalent: the four backends are a hand-rolled `MemorySecretStore` + injected … |
| `MCP-259` | low | `hand-written` | **implemented** | Honour the auth-cache disable switch | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AUTH_CACHE_DISABLED_ENV` (176) dual-read, `env_is_one` (236, strict `== "1"`), … |
| `MCP-260` | high | `hand-written` | **implemented** | Re-exec under `keyctl session -` via a hidden `__mcp-keyring-helper` subcommand | Two items. (1) `crates/cyrup/src/mcp_keyring_helper_cmd.rs` does not exist: no `SUBCOMMAND`/`is_selected(argv)`/`dispatch()` triple, no `pub mod` in crates/cyrup/src/lib.rs, and no pre-dispatch in main.rs before clap parsing. Consequence: the default `current_exe() __mcp-keyring-helper` path … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `crates/cyrup/src/mcp_keyring_helper_cmd.rs` pre-dispatched from `main.rs:254`; `credentials.rs:1393` re-execs `current_exe()` + `__mcp-keyring-helper` under `keyctl session -`. The fixture tests are `MCP-278`'s. Upstream: `mcp-auth.ts` `runLinuxKeyringRecoveryOperation`. |
| `MCP-261` | medium | `hand-written` | **implemented** | The helper's one-shot JSON stdio protocol | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `KeyringHelperRequest` (1320) with `#[serde(skip_serializing_if = "Option::is_none")]` on … |
| `MCP-262` | medium | `hand-written` | **implemented** | The revoked-keyring cause-chain predicate | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `KEY_REVOKED_PATTERN` (1261, `(?i)key\s*(?:has been\s*)?revoked\|keyrevoked` in a … |
| `MCP-263` | low | `hand-written` | **implemented** | Emit the two credential-store-unavailable messages verbatim | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `format_oauth_credential_store_unavailable` (1981): Linux + predicate ⇒ `OAuth credential store … |
| `MCP-264` | critical | `hand-written` | **implemented** | URL binding and the mutators' sibling-purge rule | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AuthEntry::matches_url` (640, exact string equality, empty stored URL treated as absent), … |
| `MCP-265` | high | `hand-written` | **implemented** | `inspectAuthForUrl`'s three-state status and its fail-open/fail-closed split | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `OAuthCredentialStatus` (1963: `Present(AuthEntry)`/`Absent`/`Unavailable{message}`) and … |
| `MCP-266` | medium | `hand-written` | **implemented** | The accessor surface section 07 consumes | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `McpAuthStore` (2048) exposes inherent methods rather than free functions over globals: … |
| `MCP-267` | medium | `rmcp` | **implemented** | Expiry arithmetic | Live path is rmcp's: /home/user/cyrup/crates/cyrup-mcp/src/oauth.rs:2740 calls `manager.get_access_token()`. The one surviving hand-written predicate … |
| `MCP-268` | high | `hand-written` | **implemented** | Serialize read-modify-write per server | Narrow residual: the synchronous inherent methods (`update_credentials` 2890, `update_client_info` 2902, `update_state` 2916, `save_auth_entry` 2765, … |
| `MCP-269` | medium | `hand-written` | **partial** | MCP credentials never reach `auth.json` | The standing guard the unit asks for does not exist: there is no repo-level test asserting that no MCP credential material can reach `cyrup_config::env`'s auth path and no `Serialize` route that could send an `AuthEntry` there. The crate flags this itself as `TODO(MCP-269)` at … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `TODO(MCP-269)` at `credentials.rs:85`. Upstream: —. |
| `MCP-270` | low | `extension-owned` | **implemented** | The embedder facade (`oauth.ts`) | /home/user/cyrup/crates/cyrup-mcp/src/oauth.rs `get_mcp_oauth_tokens_for_url` (3876, delegates to `get_valid_token`), … |
| `MCP-271` | n/a | `rmcp` | **implemented** | The MCP-SDK `OAuthTokens` conversion | Dissolved as the plan requires: /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AuthEntry::credentials` is … |
| `MCP-272` | n/a | `cut` | **not-applicable** | `ConsentManager` | Cut 2, correctly not ported. `grep -rni "consent" /home/user/cyrup/crates/cyrup-mcp/src/*.rs` finds only OAuth-consent-screen prose (config.rs:1306, … |
| `MCP-273` | n/a | `cut` | **not-applicable** | `ConsentError` | Cut with MCP-272. `grep -rn "ConsentError\|CONSENT_DENIED\|CONSENT_REQUIRED" /home/user/cyrup/crates/cyrup-mcp/src/` returns nothing; … |
| `MCP-274` | n/a | `cut` | **not-applicable** | Consent state is process-scoped and must not be persisted | Cut with MCP-272. No consent state of any kind exists in /home/user/cyrup/crates/cyrup-mcp/src/state.rs (`McpState`, 451 lines) — the module doc at … |
| `MCP-275` | medium | `hand-written` | **implemented** | Compact JSON serialization | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `write_secure_auth_entry_to_store` (2313) uses `serde_json::to_string` (never … |
| `MCP-276` | n/a | `extension-owned` | **not-applicable** | The non-string server-name guards do not port | Correctly not ported, and filed rather than dropped: /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs:757-759 records the reasoning on … |
| `MCP-277` | critical | `hand-written` | **implemented** | Prove the absence of secret leakage through `Debug`, logs and errors | One verify item unported: there is no grep-based standing test asserting that no error string anywhere in the crate interpolates a payload or token, … |
| `MCP-278` | medium | `hand-written` | **partial** | The storage acceptance suite (17 tests) | Definitively missing: the two subprocess cases — `routes revoked Linux keyring operations through the recovery helper` and `does not use the recovery helper for generic secure-store failures` (the fake `keyctl` exiting 99 and the assertion that the fake store file was never created). … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `TODO(MCP-278)` at `credentials.rs:75` (the two subprocess cases). Upstream: `mcp-auth.test.ts`. |
| `MCP-280` | high | `hand-written` | **implemented** | The keychain service name, and what happens to a co-installed pi-mcp-adapter | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AUTH_SECRET_SERVICE = "cyrup.mcp.oauth"` (125) and `LEGACY_AUTH_SECRET_SERVICE = … |
| `MCP-281` | medium | `hand-written` | **implemented** | Adopt the keychain-mandatory posture | The unit's verify is only half covered: `the_store_unavailable_sentence_is_verbatim` asserts the sentence on the error type, but there is no … |
| `MCP-282` | low | `hand-written` | **implemented** | Env-var namespace for the surviving switches | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs declares all six switches as `[&str; 2]` pairs with `CYRUP_MCP_*` first: `TEST_AUTH_STORE_ENV` … — **Re-ruled 2026-09-24 at `ea23ca2` (evidence, not status): the dual-read described above no longer exists.** `dd44b3c` collapsed all six to single-name `&str` `CYRUP_MCP_*` constants read through `env_lookup`, as a workspace-wide owner decision. Stays `implemented` against the obligation restated as "each surviving switch honoured under its `CYRUP_MCP_*` name"; the `PI_MCP_ADAPTER_*` half is a recorded divergence. See *Re-measure — 2026-09-24*. |
| `MCP-283` | medium | `hand-written` | **partial** | The cache acceptance suite (13 tests) | Not ported, each a distinct upstream case: (a) `normalizes publication exactly as a later store reload does` — no test asserting an unknown key is dropped identically on the publish (hit) path and the store-reload (miss) path; only the generic `unknown_keys_are_dropped_not_rejected` exists; (b) the … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `TODO(MCP-283)` at `credentials.rs:75`. Upstream: `mcp-auth.test.ts`. |
| `MCP-284` | medium | `hand-written` | **implemented** | The parse-error wrapping asymmetry between read and remove | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `read_auth_entry_from_store` (2549) wraps **only** the backend `store.read` in … |
| `MCP-285` | medium | `hand-written` | **implemented** | Remove-path chunk cleanup is fatal, not best-effort | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `remove_chunk_payloads` (2269, `?` on every chunk removal) is used only by … |
| `MCP-286` | low | `hand-written` | **implemented** | Bound `chunkCount` on read | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AUTH_CHUNK_COUNT_LIMIT: usize = 64` (154) with the cyrup-addition rationale in the doc comment, … |
| `MCP-287` | medium | `hand-written` | **partial** | The subprocess timeout path and the unreachable ladder rung | The three fixtures the unit names are absent: a helper that sleeps 30 s (⇒ rung-1 message within ~10 s and no zombie), a helper printing `{"ok":false,"error":"boom"}` and exiting 1 (⇒ rung-2 message), and the same helper exiting 0 (⇒ rung-5 message `boom`). No such test exists in credentials.rs's … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `TODO(MCP-287)` at `credentials.rs:81`. Upstream: —. |
| `MCP-288` | low | `rmcp` | **implemented** | The three `expiresAt` predicates | Two of the three sites are gone as specified: no SDK-shape conversion exists (MCP-271) and the live predicate is rmcp's `get_access_token` … |
| `MCP-289` | n/a | `extension-owned` | **implemented** | Create the `cyrup-mcp` crate | Layout divergence only, no behavioural consequence: this section landed as one 4796-line `src/credentials.rs` rather than the planned … |
| `MCP-290` | medium | `hand-written` | **implemented** | Persist the DCR client record rmcp's `StoredCredentials` drops | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `StoredClientInfo` (448) persists `client_id`, `client_secret`, `client_id_issued_at`, … |
| `MCP-291` | high | `hand-written` | **implemented** | Implement `rmcp::transport::auth::{CredentialStore, StateStore}` over the … | REFUTED on its central point. (1) Both traits ARE implemented over the keychain — `McpCredentialStore` (credentials.rs:3103/3128) and `McpStateStore` … |

### 13g · OAuth 2.1 acquisition

[`13g-mcp-oauth.md`](13g-mcp-oauth.md) — 49 units, 1 missing, 8 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-300` | n/a | `hand-written` | **partial** | The OAuth subsystem as one shippable unit | The unit's `verify` is MCP-347's suite green end to end against a stub authorization server; that stub does not exist and `crates/cyrup-mcp/` has no `tests/` directory. Separately, the subsystem is not reachable from a running session: `McpExtension` (extension.rs:419) does not override … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** restated: the reachability half is closed (`execute_command` → `on_mcp_auth_command`, `StoredCredentialAuth` on every HTTP connect); the remaining obligation is `MCP-347`'s suite. Upstream: —. |
| `MCP-301` | high | `hand-written` | **implemented** | Flow ownership: runtime, generation counter, four maps | oauth.rs: `McpOAuthRuntime` (line 1967) holds `token`/`controller` `CancelToken`s, an explicit `generation: AtomicU64`, a `stop_reason: … |
| `MCP-302` | medium | `hand-written` | **implemented** | extractOAuthConfig and its twelve validation messages | oauth.rs `extract_oauth_config` (line 322) applies the value-shaped rules in source order — `clientSecret` `!`-prefixed values preserved … |
| `MCP-303` | medium | `hand-written` | **implemented** | parseOAuthRedirectUri's loopback-only validation | oauth.rs `parse_oauth_redirect_uri` (line 542) with `RedirectEndpoint { port, callback_host, callback_path }` (line 521). Checks run in upstream's … |
| `MCP-304` | high | `hand-written` | **implemented** | Callback endpoint configuration and MCP_OAUTH_CALLBACK_PORT | oauth.rs: `DEFAULT_OAUTH_CALLBACK_PORT = 19876` (838), `DEFAULT_OAUTH_CALLBACK_PATH = "/callback"` (840), `DEFAULT_OAUTH_CALLBACK_HOST = "localhost"` … |
| `MCP-305` | high | `hand-written` | **implemented** | The bind / rebind / strict-port state machine | oauth.rs `ensure_callback_server` (1171) is the serializing wrapper: refuses with `OAuth callback server stopped` while a stop future is present, … |
| `MCP-306` | critical | `hand-written` | **implemented** | The callback request handler's eight branches | oauth.rs `CallbackMultiplexer::handle` (line 981) implements `cyrup_provider::auth::oauth::callback::CallbackHandler`, never calls … |
| `MCP-307` | medium | `hand-written` | **implemented** | The three callback pages, including host branding | oauth.rs `PAGE_STYLE` (676), `CHECK_ICON`/`CROSS_ICON` (735/738, both `xmlns`-free), `page()` (741) reproducing the template byte for byte including … |
| `MCP-308` | high | `hand-written` | **implemented** | Listener lifetime: reserve, wait, cancel, stop, restart, process exit | oauth.rs `reserve_callback_server` (1335), `release_callback_server` (1340), `wait_for_callback` (1355, deletes the reservation first, inserts the … |
| `MCP-309` | medium | `hand-written` | **partial** | The discovery trigger: proactive probe or reactive challenge | Nothing in production ever supplies `AuthenticateOptions::challenge`: grepping `challenge` across crates/cyrup-mcp/src finds only oauth.rs's own declaration and use, and the only `ProxyEnv` implementor is the test `FakeEnv` (proxy.rs:4500), so no connect failure is wired into it. With no challenge … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `TODO(MCP-309)` at `oauth.rs:2647`; `StoredCredentialAuth::authorize` passes no challenge. Upstream: `mcp-auth-flow.ts:1165` (refresh uses `probeAuthDiscovery`). |
| `MCP-310` | n/a | `rmcp` | **implemented** | RFC 9728 protected-resource metadata discovery | Supplied by rmcp and actually called: oauth.rs:2694 `manager.resolve_metadata_from_challenge(challenge)` and oauth.rs:3774 … |
| `MCP-311` | n/a | `rmcp` | **implemented** | RFC 8414 + OIDC discovery and the issuer echo check | rmcp-owned and reached through the same `resolve_metadata_from_challenge`/`resolve_metadata` calls; the port's only lever is … |
| `MCP-312` | medium | `rmcp` | **implemented** | RFC 7591 dynamic client registration | rmcp-owned: `AuthorizationSession::new(manager, request)` at oauth.rs:2782 drives the MCP client-registration priority order. The port supplies the … |
| `MCP-313` | medium | `hand-written` | **partial** | Client metadata and the host-branding defaults | Neither `client_uri` nor `logo_uri` nor a confidential `token_endpoint_auth_method` reaches the registration body — rmcp's `ClientRegistrationRequest` is fixed and the port does not perform its own registration POST; recorded as `TODO(MCP-312)` at oauth.rs:2771-2779. `default_client_uri()` … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `TODO(MCP-312)` at `oauth.rs:2732` — no `client_uri`/`logo_uri` in registration. Upstream: `mcp-oauth-provider.ts` client metadata. |
| `MCP-314` | high | `hand-written` | **implemented** | Restore the full client configuration after initialize_from_store | REFUTED — the claim inverts the requirement. MCP-314's cyrup column asks for exactly three things and all three are present: persist the registration … |
| `MCP-315` | high | `hand-written` | **implemented** | The keychain-backed CredentialStore, and the expiry arithmetic | Two `rmcp::transport::auth::CredentialStore` impls, both per-server and URL-bound: `oauth::ServerCredentialStore` (oauth.rs:1696, generic over the … |
| `MCP-316` | high | `hand-written` | **implemented** | authorizationParams' reserved-key guard and the no-browser-mid-turn fence | oauth.rs `RESERVED_AUTHORIZATION_PARAMS` (2359) has all eight members including `code_challenge_method`, and `add_authorization_params` (2379) … |
| `MCP-317` | n/a | `rmcp` | **implemented** | PKCE and the authorization URL | rmcp-owned and reached: the authorization URL comes from `AuthorizationSession::get_authorization_url()` (oauth.rs:2932 in `start_auth_inner`), which … |
| `MCP-318` | high | `rmcp` | **implemented** | Token endpoint, client authentication, and the retry policy | REFUTED as port work. The canonical table's verdict is `rmcp`, and the ONLY hand-written obligation the plan names is the client-auth lever — 'the … |
| `MCP-319` | n/a | `rmcp` | **implemented** | RFC 8707 resource binding | rmcp-owned; the port adds nothing and correctly adds no `validateResourceURL` override — grepping crates/cyrup-mcp/src for `resource`-parameter … |
| `MCP-320` | n/a | `rmcp` | **implemented** | Flow-state custody across the browser hop | oauth.rs:2680 `manager.set_state_store(InMemoryStateStore::new())` in `prepare_session`, and again at oauth.rs:3770 in `refresh_tokens` — the … |
| `MCP-321` | high | `hand-written` | **implemented** | The storage read/write surface this flow consumes | The seam is `oauth::McpOAuthStorage` (oauth.rs:1523) with `load`, `save_credentials`, `save_client`, `clear_all`, `oauth_state`, `clear_oauth_state`, … |
| `MCP-322` | low | `rmcp` | **implemented** | Issuer binding of stored credentials | oauth.rs `issuers_match` (2467) is equality with exactly one trailing slash tolerated on either side. `prepare_session` (2704-2717) computes … |
| `MCP-323` | medium | `rmcp` | **implemented** | The RFC 9207 gate in completeAuth, including keepPendingForRetry | oauth.rs `complete_auth` (3373): step 3 fires when `expected_issuer.is_some() && iss.is_none() && requires_issuer`, sets the explicit … |
| `MCP-324` | high | `rmcp` | **partial** | getValidToken's refresh path and its fall-through | The credential-store rethrow is a STRING-PREFIX test, not a structural one: oauth.rs:3706 does `error.to_string().starts_with(CREDENTIAL_STORE_PREFIX)` where `CREDENTIAL_STORE_PREFIX = "credential store"` (3742) and only `map_auth_error` (3745) applies that prefix, to `AuthError::InternalError`. … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `oauth.rs:3702` `error.to_string().starts_with(CREDENTIAL_STORE_PREFIX)`. Upstream: `mcp-auth-flow.ts:1186` `error instanceof OAuthCredentialStoreError`. |
| `MCP-325` | medium | `rmcp` | **implemented** | The client_credentials grant | oauth.rs `authenticate_client_credentials` (2806), short-circuited from `start_auth` at step 3 (before the redirect parse, the state generation and … |
| `MCP-326` | high | `hand-written` | **partial** | The manual/headless leg: parsing and the callback-versus-paste race | An external abort does NOT reject with the identical reason value. `AuthenticateOptions::combined_signal` (oauth.rs:2618) builds a bare `CancelToken` via `crate::abort::combine`, which carries no payload, and both the abort arm of `wait_for_authorization_response`'s `tokio::select!` and the … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `oauth.rs:2581` `combined_signal` is a bare `combine` with no reason payload. Upstream: `mcp-auth-flow.ts`. |
| `MCP-327` | low | `extension-owned` | **implemented** | Browser launch | oauth.rs `BrowserLauncher` trait (2524) with `OpenerLauncher` calling `opener::open` directly (2538; `opener = "0.8.5"` at Cargo.toml:159) and … |
| `MCP-328` | high | `hand-written` | **implemented** | startAuth's ordering, stale-registration checks and aggregate cleanup | oauth.rs `start_auth` (2899) + `start_auth_inner` (3018) implement all fourteen steps in order: disabled check (`MCP server "{s}" is disabled`); … |
| `MCP-329` | medium | `hand-written` | **implemented** | The 5-minute abandoned-flow timer and its state guard | oauth.rs `MANUAL_AUTH_TIMEOUT` = 5 min (1869); `set_pending_auth` (2208) arms a detached `tokio::spawn` racing `timer.cancelled()` against … |
| `MCP-330` | high | `hand-written` | **implemented** | authenticate's in-flight dedup and its cleanup boundary | oauth.rs `authenticate` (3475) builds the dedup key `{server}\|{url}\|{base_dir}`, and the lookup-or-insert of the … |
| `MCP-331` | high | `hand-written` | **implemented** | completeAuth and completeAuthFromInput | oauth.rs `complete_auth` (3373) requires a pending flow (`No pending OAuth flow for server: {s}`), reads the stored session/issuer/base_dir with the … |
| `MCP-332` | medium | `hand-written` | **implemented** | supportsOAuth, getAuthStatus, removeAuth | oauth.rs `supports_oauth` (491) reproduces the truth table branch for branch in the observable order, with `auth === "oauth"` beating the … |
| `MCP-333` | high | `rmcp` | **implemented** | The connect-path 401 classification | REFUTED — the auditor judged MCP-333 against a scope the plan explicitly excludes. 13g's Coverage/Excluded section says: '`server-manager.ts` beyond … |
| `MCP-334` | medium | `host-verb` | **implemented** | The /mcp-auth command surface and its eleven messages | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: There is no command handler: `McpExtension` (extension.rs:419) implements `id`, `is_ambient`, `init`, `render_call`, `render_result` and `set_host_services` but does NOT override `NativeExtension::execute_command`, so invoking `/mcp-auth <server>` hits cyrup-ext's default body at … |
| `MCP-335` | medium | `hand-written` | **implemented** | auth-start / auth-complete and auto-auth | proxy.rs `format_manual_auth_instructions` (2443) — re-exported from oauth.rs so there is one copy — builds the ten-element array, drops every empty … |
| `MCP-336` | n/a | `extension-owned` | **implemented** | Callback-listener ownership: settled as reuse | The reuse is real, not a rebuild. oauth.rs imports `cyrup_provider::auth::oauth::callback::{CallbackServer, CallbackServerConfig, CallbackHandler, … |
| `MCP-337` | n/a | `rmcp` | **implemented** | The rmcp split: verified, settled | crates/cyrup-mcp/Cargo.toml:70-76 declares exactly the settled set: `rmcp = { version = "3.1.2", default-features = false, features = ["client", … |
| `MCP-338` | n/a | `extension-owned` | **implemented** | Browser-open mechanism: settled on opener | `opener = "0.8.5"` at crates/cyrup-mcp/Cargo.toml:159, and `OpenerLauncher::open` (oauth.rs:2538) calls `opener::open(url)` directly — no … |
| `MCP-339` | medium | `open-decision` | **not-applicable** | Bind localhost or 127.0.0.1 | Open decision, and the Rust has picked option (c): bind `127.0.0.1`, advertise `localhost`. oauth.rs:850 declares `DEFAULT_OAUTH_CALLBACK_HOST = … |
| `MCP-340` | low | `open-decision` | **not-applicable** | The stale hardcoded client version in the discovery probe | Open decision, rendered moot by the side the Rust picked for MCP-309: recommendation (a), the reactive path, means there is no `probeAuthDiscovery` … |
| `MCP-341` | medium | `hand-written` | **missing** | Ship a corrected OAuth document | The whole unit: ship a ported OAuth document with §14's eight divergences corrected inline (undocumented `oauth.logoUri`; the rebranding defaults for `clientName`/`client_uri`; discovery order stated backwards — `WWW-Authenticate` is primary and `.well-known` the fallback; the absent RFC 9207 … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: `TODO(MCP-341)` at `oauth.rs:3932`. Upstream: `OAUTH.md`. |
| `MCP-342` | medium | `hand-written` | **partial** | A reachable, three-form interpolate_env_vars | The consolidation the unit asks for did not happen — a THIRD implementation was added instead of one shared implementation, and the two pre-existing copies still carry the two-form parity defect. `cyrup_ext::caps::proc::interpolate_env_vars` (crates/cyrup-ext/src/caps/proc.rs:139) is still … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: three implementations (`credentials.rs:3324`, `mcp_direct_tools.rs:1805`, `cyrup-ext` `proc.rs:139`, the last still two-form). Upstream: `utils.ts:134`. |
| `MCP-343` | n/a | `rmcp` | **not-applicable** | Non-unix entropy: dissolved | Dissolved, and the refutation holds against the current tree. `cyrup_provider::auth::oauth::random` (crates/cyrup-provider/src/auth/oauth/random.rs) … |
| `MCP-344` | medium | `hand-written` | **implemented** | The process-shared listener refcount | oauth.rs `live_runtimes()` (2029) is a process-global `OnceLock<StdMutex<HashSet<u64>>>` keyed by runtime id — a set, not a counter, so the repeated … |
| `MCP-345` | medium | `hand-written` | **implemented** | Preserve both errors when cleanup fails | `McpError::OAuthAggregate { phase: &'static str, errors: CleanupErrors }` (errors.rs:105-113) renders as `#[error("{phase}: {errors}")]` and … |
| `MCP-346` | low | `extension-owned` | **implemented** | The public token API | All three functions exist in oauth.rs: `get_mcp_oauth_tokens_for_url` (3876) delegating to `get_valid_token` so it may refresh and propagating a … |
| `MCP-347` | n/a | `hand-written` | **partial** | The executable spec as the acceptance suite | The file's own `TODO(MCP-347)` at oauth.rs:4111-4118 enumerates what is outstanding, and it is accurate: the rmcp conformance suites named in MCP-337 (MCP-310, MCP-311, MCP-317, MCP-318, MCP-319, MCP-320), `start_auth`'s five stale-registration variants (MCP-328), `authenticate`'s dedup and browser … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `TODO(MCP-347)` at `oauth.rs:4035`. Upstream: —. |
| `MCP-349` | high | `extension-owned` | **implemented** | resolveCommandSecret's subprocess mechanism | oauth.rs `resolve_command_secret` (170) with `COMMAND_SECRET_TIMEOUT = 10 s` (142) and `COMMAND_SECRET_MAX_OUTPUT_BYTES = 1 MiB` (144). All three … |

### 13h · The two panels and the slash commands

[`13h-mcp-tui.md`](13h-mcp-tui.md) — 55 units, 15 missing, 9 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-350` | — | `tracker` | **not-applicable** | Section-08 tracker: poll-repaint replaces push-repaint; the overlay pair is the … | Tracker row; the plan (docs/gap-analysis/13h-mcp-tui.md, "MCP-350 — Tracker") says it proposes no schedulable work and is excluded from every count. … |
| `MCP-350a` | high | `extension-owned` | **implemented** | Stash the `HostServices` handle so panels and commands can reach the host — … | Only the stash is proven. The plan's verify ("drive a panel action that notifies, assert it reached the injected double") cannot run because no … |
| `MCP-351` | high | `hand-written` | **implemented** | `McpPanel`'s construction from config plus validated cache | `McpPanelModel::new` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:1596-1751) walks `config.mcp_servers` as an `IndexMap`, skips non-authenticatable … |
| `MCP-352` | high | `hand-written` | **implemented** | `getOtherCurrentCandidates` and the include/exclude engine it feeds | `McpPanelModel::other_current_candidates` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:1773-1812) iterates every enabled server INCLUDING the current … |
| `MCP-353` | high | `hand-written` | **implemented** | `rebuildVisibleItems`: the flattened list plus the filter state machine | `McpPanelModel::rebuild_visible_items` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:1822-1870) reproduces all three behaviours: a non-empty query … |
| `MCP-354` | medium | `hand-written` | **implemented** | `fuzzyScore` | `fuzzy_score` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:483-511) is the literal formula: substring → `100.0 + (lq.len()/lt.len())*50.0`, otherwise … |
| `MCP-355` | critical | `hand-written` | **implemented** | The panel's top-level key dispatch, in order | Only unit-level evidence. The plan's "not done until the same sequence has been typed into a real terminal" cannot be confirmed from source. |
| `MCP-356` | medium | `hand-written` | **implemented** | The description-search modal | `McpPanelModel::handle_desc_search_key` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:2186-2226) handles only escape/confirm (exit + clear … |
| `MCP-357` | high | `hand-written` | **implemented** | The discard-confirmation modal | `McpPanelModel::handle_discard_key` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:2228-2268) with `discard_selected` initialised to 1 in `new` … |
| `MCP-358` | critical | `hand-written` | **implemented** | Toggling, dirty tracking and the tri-state `buildResult` | `toggle_item` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:1982-2017) with the `!tools.iter().all(is_direct)` rule and the import-notice at both … |
| `MCP-359` | high | `hand-written` | **implemented** | In-panel OAuth (`authenticateServer`) on the sync overlay seam | The panel-side half only. No production `McpPanelCallbacks::authenticate` exists (all impls are `#[cfg(test)]`, ui.rs:5023/5374/5514) — that is … |
| `MCP-360` | high | `hand-written` | **implemented** | In-panel reconnect and `rebuildServerTools` | `start_reconnect` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:2296-2306) and the `Reconnected` arm of `finish_job` (ui.rs:2385-2432): status … |
| `MCP-361` | medium | `extension-owned` | **implemented** | `ctrl+y` copies a server's failure message | `copy_to_clipboard` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:3019-3057) over `CLIPBOARD_COMMANDS` (ui.rs:2995-3012): `pbcopy`/`wl-copy`/`xclip … |
| `MCP-362` | medium | `host-verb` | **implemented** | The 60 s inactivity auto-cancel | The panel does not actually close itself. `InteractiveOverlay::tick` returns `bool` (/home/user/cyrup/crates/cyrup-ext/src/host/overlay.rs:289) with no way to request a close, so the code (ui.rs:3258-3266, an explicit `TODO(MCP-362)`) only sets `expired`, publishes the cancelled result, and closes … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `InteractiveOverlay::should_close` (`cyrup-ext/src/host/overlay.rs`), implemented by both panels (`ui.rs:3517`, `:5031`) off the `expired` latch. Upstream: `mcp-panel.ts:456`, `mcp-setup-panel.ts:434`. |
| `MCP-363` | high | `extension-owned` | **implemented** | `panel-keys.ts`: resolve the three canonical ids and `mcp.panel.save` | `PanelKeys` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:980-1117): `from_user_bindings` (ui.rs:1029) reproduces the three-way `mcp.panel.save` … |
| `MCP-363a` | medium | `open-decision` | **implemented** | Where the canonical select-key defaults live | The guard test is not actually cross-crate: it asserts the literals `"up"/"down"/"return"` against themselves rather than against `cyrup-tui`'s … |
| `MCP-364` | critical | `hand-written` | **implemented** | The terminal-injection sanitizers | `strip_osc_sequences` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:309-338) is a hand-written scanner that consumes to BEL/ST/`ESC \` or to end of … |
| `MCP-365` | low | `hand-written` | **implemented** | `estimateTokens` and the footer statistics | `estimate_tokens` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:534-545) is `ceil((utf16_len(name)+utf16_len(desc)+utf16_len(stringify(schema ?? … |
| `MCP-366` | medium | `hand-written` | **implemented** | The panel frame layout | Two residues, both stated in the plan and neither closeable from source: (a) the plan's `visibleWidth` \t→three-spaces normalisation is not … |
| `MCP-367` | medium | `hand-written` | **implemented** | The row renderers, status labels and word wrap | `render_server_row` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:2865-2928) with the `(not cached)` branch (no toggle icon) and the two distinct … |
| `MCP-368` | low | `host-addition` | **implemented** | Overlay geometry: the requested column counts, and the silent height clip (HA-3) | Two open pieces. (1) HA-3 itself has not landed: no `OverlayOptions { anchor, width, min_width, max_height, margin }` on `open_overlay`, no `OverlayRequest` plumbing, so the 82-column browser panel and the 92-column setup panel are painted at 95% of the terminal. (2) The height half for the SETUP … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `OverlayOptions` landed; the panels ask for `width: Some(82)` / `Some(92)` (`ui.rs:3510`, `:5024`). Upstream: `commands.ts:653`, `:794`, `:854`. |
| `MCP-369` | critical | `host-verb` | **implemented** | `McpPanelResult` escaping an `open_overlay` that returns only `bool` | No test drives it: there is no stub `HostServices` exercising the Close path or the `false` branch, and no production caller of `open_mcp_panel` … |
| `MCP-370` | critical | `open-decision` | **implemented** | Tool/resource/prompt name formatting versus the in-tree consumer | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: The other half of option (a) — upgrading the in-tree consumer in the same change — has NOT been done. /home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs still has a 3-variant `enum ToolPrefix { Server, None, Short }` (line 45-49), `get_tool_prefix` folds every unknown value … |
| `MCP-371` | medium | `hand-written` | **implemented** | `McpSetupPanel`'s screen model and dynamic action list | `McpSetupPanelModel` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:3449-3467) with `SetupScreen` (ui.rs:3283) initialised from the caller's mode in … |
| `MCP-372` | medium | `hand-written` | **implemented** | The imports and paths sub-screens | `handle_imports_key` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:3710-3735) with clamped cursor, `space` toggling membership in `selected_imports`, … |
| `MCP-374` | medium | `hand-written` | **implemented** | `runAction`, the busy latch and the notice model | `run_action` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:3785-3822) is the eight-way dispatch including the muted `Review the details below. Press … |
| `MCP-375` | medium | `hand-written` | **implemented** | The per-action preview builders | `McpSetupPanelModel::action_preview` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:4312-4455) covers all nine bodies: the `run-setup` sentence, the … |
| `MCP-376` | medium | `hand-written` | **implemented** | `formatWritePreview` and `formatPreview` | `format_preview` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:4256-4258) and `format_write_preview` (ui.rs:4267-4305): intro lines, a blank line when … |
| `MCP-377` | low | `hand-written` | **implemented** | The compact-width action window | The unit also owns the setup panel's half of MCP-368's height problem and that half is untouched, marked `TODO(MCP-368, MCP-377)` at ui.rs:4023-4028: above `inner_w >= 60` the action list is not windowed at all and `action_preview`'s output is appended unbounded (ui.rs:4120-4123), so a long action … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `ui.rs` `render_actions` windows the action list at every width against a budget net of preview, trailer and markers. (The `TODO(MCP-368, MCP-377)` comment above it at `ui.rs:4335` is stale.). Upstream: `mcp-setup-panel.ts` `renderActions`. |
| `MCP-378` | low | `hand-written` | **implemented** | The two summary lines | `discovery_summary_line` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:4167-4209) has all three branches with the first varying on … |
| `MCP-379` | medium | `hand-written` | **implemented** | `KNOWN_SERVER_PRESETS` | `known_server_presets()` (/home/user/cyrup/crates/cyrup-mcp/src/config.rs:4196-4238) returns the five presets in order — deepwiki, context7, notion, … |
| `MCP-380` | low | `hand-written` | **implemented** | The onboarding-state file | /home/user/cyrup/crates/cyrup-mcp/src/onboarding.rs in full: `OnboardingState` (line 32-43), `load_onboarding_state` (line 62-79) hand-normalises … |
| `MCP-381` | high | `hand-written` | **implemented** | `/mcp`: registration, the owner-fenced prologue and the eight-way switch | Everything in §4.1–§4.2 is absent: the owner-fenced prologue (capture `currentOwner` + a bound reload, build the synthetic `commandCtx` before the first await, await `initPromise` with the two failure notices), the `split(/\s+/)` argument split with `targetServer = parts[1]` for `reconnect` versus … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `commands.rs:347` `on_mcp_command` (prologue, split, switch). Upstream: `index.ts:1277-1452`. |
| `MCP-382` | low | `host-addition` | **partial** | HA-2: `/mcp`'s dynamic argument completions have no native path, no label and … | HA-2 itself has not landed anywhere. `NativeExtension` still has no `argument_completions` method (trait method list, /home/user/cyrup/crates/cyrup-ext/src/native.rs:458-683). `ExtensionHost::command_completions` (facade.rs:1737-1743) still delegates to `LiveExtension::argument_completions` … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`; severity `medium` → `low`.** restated: the native path exists (`argument_completions` override); the `{value, label}` label is dropped by design (`commands.rs` doc: the TUI inserts the string), and before the state commits it returns nothing where upstream falls back to the early config. Upstream: `index.ts:1226-1275` (`completionConfig = state?.config ?? earlyConfig`, `a462b30`). |
| `MCP-383` | medium | `hand-written` | **implemented** | Port `showStatus` | The whole of §4.4 is absent: the `["MCP Server Status:", ""]` header, the per-server rows in `Object.keys` order, the disabled row `⊘ {name}: disabled (run /mcp enable {name}, then /reload)` with `continue`, the five-way first-match ladder (`connected` ✓ / `needs auth` ⚠ / `failed {N}s ago — … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `commands.rs:96` `show_status` (header, disabled row, first-match ladder, suffix rule, empty case). Upstream: `commands.ts:140-205`. Drift since: the two shared-config header lines (`c893a3d`, #478 — an amendment to `MCP-528`), the listen vocabulary (`MCP-508`), the frozen line (`MCP-217a`). |
| `MCP-384` | low | `hand-written` | **implemented** | Port `showTools` | §4.5 is absent: flat-mapping `toolMetadata` over non-disabled servers to the PREFIXED registered names in map-iteration order, the `No MCP tools available` empty case, and the `MCP Tools:` / blank / two-space-indented names / blank / `Total: {N} tools` block (never singularised) as one Info notify. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `commands.rs:159` `show_tools`. Upstream: `commands.ts:244`. |
| `MCP-385` | medium | `hand-written` | **implemented** | Port `showPrompts` | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: §4.6 is absent in full: the grouped-by-server listing with servers sorted by `localeCompare` and prompts sorted in place by `commandName`, the `<required>`/`[optional]` usage rendering, the two-space `/{commandName}` row, the six-space description row, the per-group blank line, `Total: {N} … |
| `MCP-385a` | low | `hand-written` | **implemented** | `/mcp prompts` opens each group with a `{serverName}:` header row | The per-group `{serverName}:` header row — unindented, no icon, plain colon, unsanitized — is absent along with its parent function. Without it the eventual listing would be one flat run of `/mcp__a__x` rows separated by unexplained blank lines. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `commands.rs:224` pushes the unindented `{server}:` header. Upstream: `commands.ts:207`. |
| `MCP-386` | high | `hand-written` | **implemented** | Port `reconnectServer` / `reconnectServers` | All ten steps of §4.7 are absent: the unknown-target guard, the sequential (`for … await`, not a join) all-servers loop, `manager.close` → `connect` with the two `throwIfInactive` checks, the `needs-auth` early return with its exact warning, `buildToolMetadata` + `state.toolMetadata.set`, the … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `commands.rs:518` `arm_reconnect`. Upstream: `commands.ts:342`. |
| `MCP-387` | high | `hand-written` | **implemented** | Port `/mcp setup` and the reload-after-write flow | Absent: the `programmaticConfig` refusal (`MCP setup is unavailable when config is supplied by createMcpAdapter().`), the once-only computation of `discovery = getMcpDiscoverySummary(...)` and `loadOnboardingState()` at OPEN time with `markSetupCompleted` persisting that pre-write fingerprint, the … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `commands.rs:583` `arm_setup` and the `programmaticConfig` refusal at `:392`. Upstream: `commands.ts:586-657`. |
| `MCP-388` | high | `hand-written` | **implemented** | Port `logoutServer` | All four steps of §4.9 are absent, including the load-bearing string `OAuth credentials were cleared for "{name}", but its connection could not be closed: {msg}` that distinguishes "credentials gone, connection alive" from a total failure, plus the usage error (`Usage: /mcp logout <server>`) and … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `commands.rs:650` `arm_logout` meets the recorded (v2.32.1) obligation; the v2.33.0 reordering is `MCP-555`. Upstream: `commands.ts:443`. |
| `MCP-389` | medium | `hand-written` | **implemented** | Port `/mcp disable` and `/mcp enable` | The shared `disable`/`enable` arm is absent: the `programmaticConfig` refusal `"/mcp {sub} is unavailable when config is supplied by createMcpAdapter()."`, the `Usage: /mcp {sub} <server>` error, the `Server "{name}" not found in effective config` error, and the two result notices … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `commands.rs` `arm_set_disabled` (`:419`, `:477`). Upstream: `index.ts` disable/enable arm. |
| `MCP-390` | high | `host-verb` | **implemented** | Port `authenticateServer` and `/mcp-auth` | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: The command-level flow is explicitly outstanding — `TODO(MCP-334)` at oauth.rs:3922-3929 says so. Absent: the `/mcp-auth` handler itself (no `execute_command`, MCP-381); `terminalHyperlink`'s OSC-8 emission (grep for `terminal_hyperlink` and for a `\u{1b}]8` producer across the crate finds only the … |
| `MCP-391` | medium | `host-verb` | **implemented** | Port `openMcpAuthPanel` | The entry point `openMcpAuthPanel` itself does not exist. Absent: the `!hasUI` guard, the `programmaticConfig` refusal `Use /mcp-auth <server> to authenticate a server from the in-memory SDK config.`, the zero-OAuth-capable-servers warning `No OAuth-capable MCP servers are configured.` (grep finds … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `extension.rs` `pick_oauth_server` with the `from the in-memory SDK config` refusal and `No OAuth-capable MCP servers are configured.` (`:1875-1878`). Upstream: `commands.ts:807`. |
| `MCP-392` | high | `hand-written` | **implemented** | Port `buildMcpPanelCallbacks`'s connection-status derivation | The whole of §4.11's `buildMcpPanelCallbacks` is absent: the per-open `authStatusFailures: Map<String,String>` (deliberately NOT session state), the eight-rung `getConnectionStatus` ladder (delete-from-map → disabled → `resolveServerUrl` throws ⇒ `failed` → the four-condition OAuth guard calling … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `panel_host.rs` `auth_status_failures` + `connection_status` ladder. Upstream: `commands.ts:658-806`. |
| `MCP-393` | low | `hand-written` | **implemented** | Port the shared-config notice and its one-shot state | REFUTED. This unit names two things — `buildSharedConfigNoticeLines` and the one-shot state — and both are ported, byte-for-byte on the strings: … |
| `MCP-394` | critical | `hand-written` | **implemented** | Port `openMcpPanel`'s orchestration and the direct-tools write-back | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: The orchestration is explicitly outstanding — `TODO(MCP-394)` at ui.rs:4781-4786. Absent: the `programmaticConfig` branch (notify `MCP status is shown from the in-memory SDK config; configuration discovery is unavailable.` + `showStatus`), the ZERO-SERVERS-delegates-to-`openMcpSetup(…, "empty", … |
| `MCP-394a` | medium | `hand-written` | **implemented** | A change for a server with no provenance entry is silently dropped | Two soft residues rather than behavioural gaps: the skip is an unannotated `let-else continue` with no comment naming it as upstream's deliberate … |
| `MCP-395` | high | `host-addition` | **implemented** | HA-1's command leg: MCP prompts are slash commands, and there is no late … | The live half has nothing to land on and none of the three additions have been made: grep across /home/user/cyrup/crates/cyrup-ext/src, /home/user/cyrup/crates/cyrup-session-svc/src and /home/user/cyrup/crates/cyrup-tui/src for `register_late_command`, `mark_commands_dirty`, `take_commands_dirty`, … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `facade.rs` `HostLateRegistrar::register_command` + `commands_listeners` rebuild of the TUI `/` menu. Upstream: `index.ts:713-735`. |
| `MCP-395a` | medium | `hand-written` | **implemented** | Cache-time prompt resolution and command naming | `resolve_cached_prompts` (/home/user/cyrup/crates/cyrup-mcp/src/registration.rs:1720-1765) walks the cache's server order, skips servers absent from … |
| `MCP-396` | medium | `hand-written` | **implemented** | Port `parsePromptArgs`'s bash-style tokenizer | §5.3 is absent in full: the character-by-character tokenizer with `escaped` carried across iterations (so a trailing lone backslash is dropped), the backslash-is-literal-inside-single-quotes rule, quote characters RETAINED in the token, unterminated quotes running to end of input, … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `prompts.rs:107` `parse_prompt_args`. Upstream: `prompts.ts:43`. |
| `MCP-397` | medium | `hand-written` | **implemented** | Port `resolvePromptArgs` and the usage message | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: §5.4 is absent: loop 1 over the declared arguments in declaration order with the named-lookup-first / positional-cursor-advances-only-on-a-miss rule, loop 2 forwarding undeclared named arguments unfiltered, the `missing` filter over required-and-empty, and `buildUsageMessage`'s `Missing required … |
| `MCP-397a` | low | `hand-written` | **implemented** | An explicit empty named value for a declared optional argument is still sent | The two-loop ordering that makes an explicit empty named value survive for a declared OPTIONAL argument (`args["topic"] = ""` on the wire) while a declared REQUIRED one still fails the `missing` filter must be written in upstream's order with no `is_empty()` guard on loop 2. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `prompts.rs:140` `resolve_prompt_args` — loop 2 has no `is_empty` guard. Upstream: `prompts.ts` `resolvePromptArgs`. |
| `MCP-398` | high | `host-verb` | **missing** | Port the prompt command handler | All nine steps of §5.5 are absent: the `MCP not initialized` guard, the `promptMetadataLive`-guarded staleness check BEFORE `lazyConnect` (the guard that stops a cache-only command being refused before its server has been contacted), argument parse/resolve, the un-configured-server check, the two … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged and user-visible: prompt commands are registered, but `McpExtension::execute_command` handles only `mcp` / `mcp-auth` and nothing in the crate sends `prompts/get`, so invoking a registered `/server:prompt` command returns `native extension has no handler for command`. Upstream: `prompts.ts:242-360` `createPromptCommand`. |
| `MCP-399` | medium | `hand-written` | **implemented** | Port `formatPromptResult` and `extractMessageText` | §5.6 is absent: the `lines.join("\n\n").trim()` flattening, the single-`user`-message-emitted-bare special case with `[{role}] ` prefixes otherwise (including a lone ASSISTANT message keeping its prefix), the skip of empty extractions, and the five content-kind placeholders `[resource {uri}]` … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `prompts.rs:227` `format_prompt_result`, `:256` `extract_message_text`. Upstream: `prompts.ts:184`. |

### 13i · Protocol tracer, conformance, verification

[`13i-mcp-protocol-and-verification.md`](13i-mcp-protocol-and-verification.md) — 50 units, 31 missing, 11 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-450` | high | `hand-written` | **implemented** | handleSamplingRequest as a pure function of an options bag | The whole 12-step `handleSamplingRequest` free function is absent: no `SamplingOptions` bag, no `handle_sampling_request(&SamplingOptions, CreateMessageRequestParams) -> Result<CreateMessageResult, ErrorData>`, no producer of `SamplingHook`, and none of the 11 mirrored unit cases from … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `sampling.rs:181` `handle_sampling_request` over `SamplingOptions`, installed by `runtime.rs` step 5. Upstream: `sampling-handler.ts:34` (the v2.33.0 model-resolution change is `MCP-557`). |
| `MCP-451` | medium | `hand-written` | **implemented** | The six unsupported-sampling-feature rejections, in order (task becomes … | Missing: the ordered `match` over `CreateMessageRequestParams::{include_context, tools, tool_choice, stop_sequences}` producing the four byte-exact messages; the per-content-block guard (`MCP sampling ${type} content is not supported` / `MCP sampling assistant ${type} content is not supported`) … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `sampling.rs` guard constants `:60-75` in order; content guard inside `convert_sampling_message`. Upstream: `sampling-handler.ts:34-66`. |
| `MCP-452` | high | `extension-owned` | **implemented** | resolveSamplingModel candidate ordering and the sequential auth probe | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: Missing entirely: `fn sampling_candidates(available, hints, current) -> Vec<Model>` with hint-order-major / registry-order-minor appending, lowercase substring `.contains()` matching over `provider/id` \| `id` \| `name`, first-wins dedupe on `(provider, id)`, then current model, then the whole … |
| `MCP-453` | high | `extension-owned` | **implemented** | Run the nested completion via cyrup-provider directly | Missing: the direct `cyrup_provider` completion call with `{systemPrompt?, messages}` and `{apiKey?, headers?, maxTokens, temperature?, metadata?, cancel}`; `max_tokens` passed through unmodified/unclamped; the composed child `tokio_util::sync::CancellationToken` cancelled by either the run … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `sampling.rs` `options.models.complete(&resolved, &context, &StreamOptions{cancel, max_tokens unclamped, temperature})`. Upstream: `sampling-handler.ts:68`. |
| `MCP-454` | high | `extension-owned` | **partial** | Source the candidate set from the whole configured catalogue | Missing: reading `cyrup_provider::catalog::{builtin_catalog, load_catalog}` directly to build the candidate set, plus the `HostServices::{models, scoped_models, current_model}` session view. Nothing consumes the fenced `models()`/`current_model()` delegations that already exist in `owner.rs`. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `partial`; severity `medium` → `high`.** restated: the candidate set is `runtime.rs`' `cyrup_provider::default_models(CreateModelsOptions::default())` — built-in providers only, over an **empty** `InMemoryCredentialStore` and env-only auth. Custom `models.json` providers and stored logins are invisible, so a user whose only credential is a stored login gets `No cyrup model is available for MCP sampling` where upstream samples. The session seam exists (`HostServices::registered_provider`, `models`). Upstream: `init.ts:158` `modelRegistry: ctx.modelRegistry`; `sampling-handler.ts:129`. |
| `MCP-455` | critical | `host-verb` | **implemented** | The two sampling approval gates and their formatters | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: Missing: `confirmSampling`'s three-branch gate (auto-approve short-circuit; explicit `has_ui: bool` sourced from the host config producing the distinct "MCP sampling requires interactive approval. Set settings.samplingAutoApprove to true to allow it without UI." message; `HostServices::confirm` … |
| `MCP-456` | medium | `hand-written` | **implemented** | convertSamplingMessage, convertAssistantResult, mapStopReason | Missing: the `SamplingContent::{Single, Multiple}` normalisation; the synthetic assistant record with the literal sentinels `api: "mcp-sampling"`, `provider: "mcp"`, `model: "sampling-request"`, all-zero usage and `stopReason: "stop"`; `convertAssistantResult`'s error/aborted rethrows, … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `sampling.rs:270` `convert_sampling_message`, `:329` `convert_assistant_result`. Upstream: `sampling-handler.ts:173-230`. |
| `MCP-457` | low | `rmcp` | **implemented** | Sampling capability advertisement and handler-before-connect | None for this unit's obligations. Note for context only: because no `SamplingHook` producer exists (MCP-450), the sampling capability is never … |
| `MCP-458` | high | `host-verb` | **partial** | Bind sampling's model and cancellation to the live runtime owner | Missing: a sampling options bag holding two live closures over the stashed `Arc<dyn HostServices>` — `current_model()` read live, and a cancellation source composed as a child `CancellationToken` — plus the required *two independent* signal-accessor reads (once at handler entry, once inside the … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `partial`.** restated: `SamplingOptions::{current_model, signal}` exist and re-read live, but (1) `SessionSlot::current_model` reads through `state.ui`, which is `None` without a UI, so a headless `samplingAutoApprove` run never prefers the session's model; (2) `signal()` combines the owner with `None` — `HostServices::is_run_cancelled` is not implemented by the session host (`abort.rs` doc) — so Esc does not cancel an in-flight sampling call. Upstream: `init.ts:162-165` (`ctx.model`, `combineAbortSignals(owner.signal, ctx.signal)`; pi `runner.ts:858` `get signal()` is the live turn signal). |
| `MCP-459` | low | `hand-written` | **implemented** | truncateAtWord with UTF-16 length semantics | `crates/cyrup-mcp/src/registration.rs:571 pub fn truncate_at_word(text: &str, target: usize) -> String` implements the exact five-branch algorithm … |
| `MCP-460` | low | `rmcp` | **implemented** | Elicitation dispatch; absent/unknown mode falls to form | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: There is no form-vs-url dispatch and no form handler, so the "absent/unknown mode → form" behaviour cannot be exercised or verified. Work item: implement the `match ElicitRequestParams { FormElicitationParams => handle_form_elicitation, UrlElicitationParams => handle_url_elicitation }` split plus … |
| `MCP-461` | high | `hand-written` | **implemented** | handleFormElicitation's gate, review loop and edit picker | Missing in full: the `MCP Input Request\nServer: …` gate dialog with `["Continue","Decline"]` and `None`→cancel; the `properties.len() == 0` → `{action:"accept", content:{}}` short-circuit before any review screen; the per-field collect pass; the `while(true)` review loop with its deliberately … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `elicitation.rs:796` `handle_form_elicitation`. Upstream: `elicitation-handler.ts:47`. |
| `MCP-462` | low | `rmcp` | **implemented** | Iterate requestedSchema.properties in document order | No iteration site exists yet. When the form handler lands it must zip `ElicitationSchema::property_order` against `ElicitationSchema::properties`; iterating the `BTreeMap` directly is the silent bug the plan names. Verify with a `z`/`a`/`m` key-order test. — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `elicitation.rs:310` `ordered_properties`. Upstream: `elicitation-handler.ts:52`. |
| `MCP-463` | medium | `hand-written` | **implemented** | collectValidField's per-field re-prompt loop | Missing: the unbounded per-field re-prompt loop; the single-property synthetic schema built by copying `params` and replacing only `requested_schema` (so sibling fields like `message` survive), carrying `required` only when the field is required; `HostServices::notify(msg, NotifyKind::Error)` on … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `elicitation.rs:725` `collect_valid_field`. Upstream: `elicitation-handler.ts:90`. |
| `MCP-464` | high | `hand-written` | **implemented** | coerceAndValidateFormValues, including JS Number() semantics | Missing: the whole coercion core — 13 distinct message templates across 15 throw sites over `PrimitiveSchemaDefinition`'s typed limit fields (`StringSchema::{min_length,max_length}`, `NumberSchema`/`IntegerSchema::{minimum,maximum}`, `*MultiSelectEnumSchema::{min_items,max_items}`); an explicit JS … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `elicitation.rs:594` `coerce_and_validate`, `js_number`, the message builders `:106-160`. Upstream: `elicitation-handler.ts:198`. |
| `MCP-465` | high | `hand-written` | **implemented** | Final schema assertion with format as an assertion, not an annotation | Missing: compiling the original `requested_schema` with `jsonschema` + `.should_validate_formats(true)`, running it over the coerced `output`, and throwing `Invalid elicitation response: {err}`. Also missing the compiled-schema cache (the validator runs twice per field), and the decision to … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `coerce_and_validate` runs the `ValidatorCache` (`should_validate_formats(true)`) and raises `Invalid elicitation response: …`. Upstream: `elicitation-handler.ts:263`. |
| `MCP-466` | medium | `hand-written` | **implemented** | The label-uniquifying and humanising helpers | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: Missing: `formatChoice(value, title)`; `uniqueLabels`/`uniqueAction` appending `…` in a `while` loop against an accumulating `used` set (required because `HostServices::select` returns the chosen *string*); `extractMultiSelectOptions` as a `match` on `MultiSelectEnumSchema::{Untitled(items.enum_), … |
| `MCP-467` | high | `hand-written` | **implemented** | handleUrlElicitation, including the three -32602 rejections | Missing the whole handler: the `!allow_url` gate, `url::Url::parse` failure and the http/https scheme allowlist — all three as `ErrorData::invalid_params` (-32602); the exact 9-line confirmation dialog with `Host:` = host+port (`Url::host_str` + port) and `Full URL:` = the **raw** input string (no … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `elicitation.rs:881` `handle_url_elicitation` (three `-32602`s — the 2026-09-04 `MCP-472` citation). Upstream: `elicitation-handler.ts:311`. |
| `MCP-468` | medium | `rmcp` | **implemented** | Advertise elicitation {form, url?} with allowUrl == (mode == tui) | Nothing wires them in production: grep shows `.elicitation(` (config.rs:1090) has no caller, `ElicitationMode` is constructed only in tests, and `McpClientHandler::new` is called only at runtime.rs:1615 (a test). `runtime.rs:118` still lists "the **elicitation gate**" as work inside … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `runtime.rs` step 6 `set_elicitation_config` with `allow_url = is_tui_mode()`; the manager's handler factory advertises from it. Upstream: `init.ts:167-171`. |
| `MCP-469` | medium | `rmcp` | **implemented** | The notifications/elicitation/complete dedupe and its notice | The hand-written half is absent. `crates/cyrup-mcp/src/state.rs` (read in full, fields listed at l.77-128) has no accepted-elicitation registry; grep of the crate for `HashSet<String>` keyed by server, `remember_url_elicitation`, and the notice text `MCP browser interaction for … completed. You can … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `server_manager.rs` `forget_url_elicitation`-gated notice in `manager_handler_factory`. Upstream: `server-manager.ts:1395`. |
| `MCP-470` | medium | `hand-written` | **implemented** | handleUrlElicitationRequired for the -32042 elicitation array | Missing: decoding `ErrorData { code: ErrorCode(-32042), data }` into the elicitation array (rmcp models neither), and the sequential loop — cancel immediately if the runtime is aborted or `allow_url` is false; otherwise iterate `error.elicitations` in order, short-circuit on the first non-`accept`, … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `server_manager.rs:2971` `handle_url_elicitation_required` (abort / `allow_url` cancel, sequential short-circuit). Upstream: `server-manager.ts:1464`. |
| `MCP-471` | high | `host-verb` | **implemented** | Hold the dispatcher budget and the interaction lock across every dialog | Missing: taking a `#[must_use]` `HostCtx::begin_human_wait()` guard and the session-scoped `HostServices::human_interaction_lock` across every `select`/`input`/`confirm` in `cyrup-mcp` — the two sampling approval dialogs, the elicitation gate/field/review/edit dialogs (with the guard wrapping the … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `owner.rs` `McpDialog::with_human_wait` takes `human_interaction_lock` and `begin_human_wait`; the ctx is recorded at commit (`extension.rs` step 2b). Upstream: —. |
| `MCP-472` | low | `rmcp` | **implemented** | The three URL rejections carry JSON-RPC -32602 | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: Missing: the three `ErrorData::invalid_params(msg, None)` returns for "URL elicitation is not supported", "URL elicitation supplied an invalid URL" and "URL elicitation only supports HTTP and HTTPS URLs", plus the discipline that every *other* throw in the two handlers stays `-32603` … |
| `MCP-473` | medium | `hand-written` | **implemented** | The McpTraceEvent schema v1, exact key set and insertion order | Missing: the `#[derive(Serialize)]` event struct with the 13 fields in `createMcpTraceEvent`'s **insertion** order (`version, timestamp, direction, server, transport, kind, status, bytes, method, id, relatedRequestId, errorCode, durationMs`) — explicitly NOT the interface declaration order where … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `trace.rs:88` `McpTraceEvent`. Upstream: `mcp-trace.ts`. |
| `MCP-474` | high | `hand-written` | **implemented** | redactTraceText, dead third branch and all | Missing: the keyword guard `\b(?:token\|secret\|password\|passwd\|api[_-]?key\|authorization\|cookie)\b` (case-insensitive) returning `"[REDACTED]"`; the three replacements (URL scheme, `bearer\|basic`, and the third — port it verbatim including the literal `"$1=[REDACTED]"` replacement string … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `trace.rs:151` `redact_trace_text`. Upstream: `mcp-trace.ts:57`. |
| `MCP-475` | low | `hand-written` | **implemented** | traceId, messageKind, messageBytes | Missing: `messageKind` (`"method" in msg` → `"id" in msg ? request : notification`, else `response`); `traceId` as a two-arm match on rmcp's `NumberOrString::{Number(i64), String(Arc<str>)}` producing the number or the literal `"[REDACTED_ID]"`; and `messageBytes` as the serialised UTF-8 length … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `trace.rs:283` `trace_id`, `trace_event`. Upstream: `mcp-trace.ts`. |
| `MCP-476` | medium | `hand-written` | **implemented** | McpTraceWriter: latching caps, injectable fs, serialized append queue | Missing the writer itself: injectable `append_file`/`write_file`/`mkdir` (the seam without which the `["reset","append"]` ordering test and the `maxBytes: 20` latch test cannot be unit tests); truncate-on-open with the `mkdir → writeFile("")` init chain latching `disabled` on failure; a sync, … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `trace.rs:373` `TraceWriter` with `RealTraceFs`. Upstream: `mcp-trace.ts:122`. |
| `MCP-477` | low | `open-decision` | **implemented** | Trace file path derivation, and .pi to .cyrup | The side picked is (a) `.cyrup/mcp-traces/`. Still missing the rest of `createMcpTraceWriter`'s path derivation: `settings.file` used verbatim when absolute and resolved against the session cwd when relative, else `mcp-<ISO timestamp with `:`/`.` → `-`>-<≤8 base36 chars>.jsonl` inside … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `trace.rs:513` `trace_file_path` (absolute / cwd-relative `settings.file`, else `.cyrup/mcp-traces/mcp-<stamp>-<suffix>.jsonl`). Upstream: `mcp-trace.ts:202-217`. |
| `MCP-478` | low | `hand-written` | **implemented** | isMcpTraceEnabled and the reduced transport-kind enum | **Re-ruled 2026-09-04 (v2.32.1 re-audit): implemented** — citation in the re-audit block at the head of this file. Prior ruling, kept for the record: Missing: the combining function `is_mcp_trace_enabled(entry, settings) = entry.trace.unwrap_or(settings.trace_enabled())` — the `??` semantics where a per-server `false` beats a global `true`; `\|\|` would be wrong. Missing the five-case test (including `{debug:true}` → false and `{trace:false}` + … |
| `MCP-479` | medium | `hand-written` | **implemented** | TracingTransport<T> over rmcp::transport::Transport | Missing: a `TracingTransport<T>` newtype implementing `rmcp::transport::Transport<RoleClient>` — `send` timing the inner send and emitting one event per JSON-RPC batch member *after* it resolves (status `sent`, or `error` + rethrow); `receive` emitting an `inbound` event before returning; `close` … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `trace.rs:569` `TracingTransport`; `runtime.rs` `maybe_traced` at the stdio and both HTTP sites. Upstream: `mcp-trace.ts`. |
| `MCP-480` | medium | `hand-written` | **implemented** | Wire the trace writer lifecycle into the server manager | Missing: a lazily-created `OnceCell<Arc<TraceWriter>>` on the manager shared across all traced servers (session-global byte/event budgets); instrumenting the transport at construction with the kind carried as an enum (post-Cut-1/Cut-3 there is one instrumentation point per kind, so … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `missing` → `implemented`.** `server_manager.rs` `set_trace_config` / `trace_writer_for`, set in `runtime.rs` step 4b; flushed in `dispose_connection`. Upstream: `server-manager.ts:343`. |
| `MCP-481` | low | `hand-written` | **implemented** | The trace settings surface (settings.trace object, per-server trace bool) | Nothing consumes it — grep shows `TraceSettings` has no reference outside its declaration and `McpSettings::trace`, and `trace_enabled()` has no caller. The unit's own verify ("assert per-server `false` beats global `true`") cannot be written until MCP-478's combining function exists; `config.rs` … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): `partial` → `implemented`.** `trace.rs:499` `is_mcp_trace_enabled(settings, per-server)`. Upstream: `mcp-trace.ts`. |
| `MCP-482` | n/a | `hand-written` | **implemented** | Tracker: the upstream verification surface, with the cut census | None as an index. It remains a document-only unit; nothing in `crates/` tracks the case-count parity metric it defines (see MCP-490). |
| `MCP-483` | high | `hand-written` | **missing** | Adopt the MCP conformance harness as the port's protocol gate | Missing: adopting `@modelcontextprotocol/conformance` (pinned to rmcp's `0.2.0-alpha.10`, per the docket, not upstream's 0.1.16) as the port's protocol gate, run for both `--spec-version` values, with results archived. The client contract to implement is `argv[1]` = server URL, … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: no conformance harness in the workspace. Upstream: `conformance/`. |
| `MCP-484` | high | `hand-written` | **missing** | A hidden cyrup mcp conformance-driver subcommand | Missing the whole driver: scenario allowlist with non-zero exit on an unknown scenario; the scripted elicitation UI with preference order `["Use default","Submit","Continue"]` then `options[0]`; `CONFORMANCE_DRIVER_DEBUG`; the definition builder; the headless OAuth round trip; `connectWithAuth` … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: no driver subcommand in `crates/cyrup/src`. Upstream: `conformance/`. |
| `MCP-485` | medium | `hand-written` | **missing** | A sequential runner with post-hoc log assertions | Missing: the sequential-or-parallel runner with an env-overridable results dir and timeout, the `is_baselined` literal `grep -Fqx " - $1"` check on the YAML text, the `allows_client_error` cases, live scenario discovery, the three-way per-scenario outcome, the closing summary line, and — most … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged. Upstream: `conformance/`. |
| `MCP-486` | medium | `hand-written` | **missing** | Re-derive the expected-failures baseline; do not copy it | Missing: an empty `expected-failures` file, one observed run, and a file written from the observed failures with a mechanism-level rationale per entry, preserving the exact two-space ` - scenario` indentation the runner greps for. Copying upstream's five entries would be actively unsafe (a listed … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged. Upstream: `conformance/expected-failures`. |
| `MCP-487` | low | `hand-written` | **missing** | Allocate the ephemeral callback port in Rust | Missing: `std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port()` then drop, per driver process, plus the per-scenario tempdir OAuth store. Note this unit may be **dissolved rather than implemented**: `docs/gap-analysis/MCP-PORT-METHODOLOGY.md` ADR-0022 recommends "runner (b) — binding … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged. Upstream: `conformance/`. |
| `MCP-488` | n/a | `hand-written` | **implemented** | Record what conformance does not cover | Three of the plan's named non-coverage bullets are not enumerated explicitly and are only implied by "the entire adapter layer": **sampling** (no … |
| `MCP-489` | medium | `open-decision` | **not-applicable** | The fate of the eight surviving fixture MCP servers | Unresolved. Ratifying it needs two sub-rulings the docket names: whether `node` may appear in the test environment at all, and whether `rmcp/server` … |
| `MCP-490` | high | `hand-written` | **partial** | Port the unit-testable share of the vitest suite | Section 13i's own share is entirely absent — zero tests for sampling, elicitation or tracing, because none of that code exists (MCP-450..MCP-481). Also missing: the case-count parity metric the unit names as its tracking measure is recorded nowhere in the tree, so "how much of the 84 in-scope … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** restated: sampling, elicitation and trace now carry in-crate tests; the case-count parity metric the unit tracks by is still recorded nowhere. Upstream: `__tests__/`. |
| `MCP-491` | medium | `open-decision` | **partial** | A home for the MCP seam tests without breaking the 7-target cap | Three concrete work items: (1) reconcile the conflict — either fold `tests/mcp/` into `bin`/`session_svc` per ADR-0021(b), or write the justification into `docs/TEST-ARCHITECTURE.md` §9.1 and raise G2's threshold; (2) the G2 count is in fact already `11`, not `8` — `crates/cyrup-tui/tests/` holds … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged open decision; `crates/cyrup-it/tests/mcp/` is still its own target. Upstream: —. |
| `MCP-492` | high | `hand-written` | **partial** | Port the node:test OAuth suite as a serialised group | REFUTED on its central assertion. The claim says 'three of the four surviving upstream files have no port'; all three do have substantial ports, they are just not named after the .ts files. (a) mcp-callback-server.test.ts → `the_callback_listener_end_to_end` (oauth.rs:4797, ~170 lines over a real … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: the ports exist in-crate (`oauth.rs` tests); the serialised cross-process group has no `cyrup-it` home. Upstream: `mcp-auth.test.ts`, `mcp-callback-server.test.ts`, `mcp-auth-flow.test.ts`, `oauth-public-api.test.ts`. |
| `MCP-493` | low | `hand-written` | **missing** | A Cargo/manifest policy test pinning the rmcp feature set | Missing: a `#[test]` in `cyrup-mcp` that parses its own `Cargo.toml` (the `toml` crate is already a dependency) and asserts the rmcp feature set is exactly the five named features with `default-features = false`, that `server` and `elicitation` are absent, that the pinned version matches, and that … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: no test reads `crates/cyrup-mcp/Cargo.toml`. Upstream: —. |
| `MCP-494` | medium | `open-decision` | **not-applicable** | The CI gate's shape, including the conformance step | Unresolved, and there is nothing to build on: no workflow file exists, so the clippy/typecheck step, the chosen gate, the `cyrup-it --features it` … |
| `MCP-495` | medium | `hand-written` | **partial** | Reconcile the test-time environment contract with cyrup's isolation rules | Two obligations unmet. (1) The doc reconciliation the unit explicitly asks for has not happened: `docs/TEST-ARCHITECTURE.md:613-614` still tells readers to "use `cyrup_test_support::env::scoped`" and l.650-657 references `cyrup_test_support::env::PROVIDER_KEYS`, but `crates/cyrup-test-support/src/` … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `partial`.** unchanged: `docs/TEST-ARCHITECTURE.md` still cites `cyrup_test_support::env::scoped`; `crates/cyrup-test-support/src/` has no `env.rs`. Upstream: —. |
| `MCP-496` | high | `hand-written` | **missing** | Live-pty verification for the elicitation dialogs and sampling gates | Missing: pty infrastructure (none in the workspace) plus a driven run of the full elicitation sequence — gate → one dialog per widget kind → 20-option multi-select with `✓ ` toggle state → review → edit → submit — and both sampling approval dialogs, screenshotting each step, checking scrollability … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged: no pty harness in the workspace. Upstream: —. |
| `MCP-497` | n/a | `cut` | **not-applicable** | Coverage tracking | Verdict is `cut` — coverage tracking is out of scope by owner decision (the vitest v8 coverage config gates nothing upstream and is not run in … |
| `MCP-498` | medium | `hand-written` | **missing** | The two child-process host harnesses | Missing both: (1) a child-process startup harness asserting a direct MCP tool is registered **before** `agent_start` from a cold cache — and the unit's own note applies, that on a cold cache cyrup exposes only the `mcp` proxy tool unless HA-1 is built, so the test must state whether it is testing … — **Third pass 2026-09-24 (both sides; cyrup `ea23ca2`, upstream v2.37.0): re-checked, stays `missing`.** unchanged. Upstream: —. |
| `MCP-499` | medium | `open-decision` | **not-applicable** | A trace-JSONL differential harness against the TS adapter | Unresolved, and doubly blocked: it needs the ADR-0027 ruling *and* the tracer (MCP-473..MCP-480) before the oracle can be bootstrapped on … |

#### Triage — 2026-08-22 (scoping pass)

The census row above is **as of the 2026-08-21 audit**. This subsection is the scoping pass the
audit's own shape asked for: 13i carries the most `missing` units of any section (31; next is 13c
at 20) and the highest missing:partial ratio (31:11 against 13h's 15:9) — both read off the by-section
table above — so picking units off the list in id order would start by building things whose harnesses
do not exist and whose call sites are not reachable. It re-checks every open row against the tree as
it stands today, orders what is left, and proposes waves. **No production code was changed by this
pass**; `git status --porcelain` was empty before it and the only files it writes are this document
and its own `.flux` task frontmatter.

Baseline recorded before writing (so "unchanged" means something): `cargo clippy -p cyrup-mcp
--all-targets` exit 0, **2 diagnostics**, both pre-existing and both already named at the head of
this file — `empty_line_after_doc_comments` at `dirs.rs:1861-1863` and `result_large_err` at
`runtime.rs:1696`. `cargo nextest run -p cyrup-mcp` was run for the same reason. No `.rs` file is
touched by this pass, so both figures are unchanged by construction.

**The open set, enumerated.** 50 units, 4 `implemented` (`MCP-457`, `MCP-459`, `MCP-482`,
`MCP-488`), 4 `not-applicable` (`MCP-489`, `MCP-494`, `MCP-497`, `MCP-499`), leaving **42 open**:
31 `missing` — `MCP-450`, `451`, `452`, `453`, `454`, `455`, `456`, `458`, `461`, `462`, `463`,
`464`, `465`, `466`, `467`, `471`, `472`, `473`, `474`, `475`, `476`, `479`, `480`, `483`, `484`,
`485`, `486`, `487`, `493`, `496`, `498` — and 11 `partial` — `MCP-460`, `468`, `469`, `470`, `477`,
`478`, `481`, `490`, `491`, `492`, `495`.

##### The gate that sits above all 42, and why it is not a blocker

Every 13i handler reaches a live server through one seam: the manager-supplied
[`HandlerFactory`](../../crates/cyrup-mcp/src/runtime.rs) (`runtime.rs:1927`). What production
installs today is `bare_handler_factory()` (`runtime.rs:1931-1942`), whose `sampling`,
`elicitation`, `list_changed` and `elicitation_complete` are all literal `None`; the only other
producer is `ConnectionBuilder::with_handler_factory` (`runtime.rs:2289`), which has no caller —
`grep -rn 'with_handler_factory\|handler_factory\|bare_handler_factory' crates/ --include=*.rs`
returns 6 lines, all inside `runtime.rs`. One level up, `initialize_mcp` still has no non-test
caller (`grep -rn 'initialize_mcp(' crates/ --include=*.rs` → the definition at `runtime.rs:125`
and one call at `runtime.rs:413`, inside this file's `#[cfg(test)]` module), which is the same
finding the wave-5 note above records and is unchanged.

The consequence, stated so nobody re-derives it: **no 13i unit can be verified end-to-end in
production today, and none of them is blocked by that.** Every unit below is unit-testable at its
own seam — a free function, a `Transport` newtype, a writer with injectable fs, a `ClientHandler`
constructed directly. The activation chain (§13a) and the manager's factory (§13c) are what turn
the finished handlers on; they are a **sequencing** constraint on the final `cyrup-it` proofs, not
a prerequisite for writing any of the code. Schedule 13i's waves against unit seams and let the
activation work land in parallel.

##### Confirmed-missing vs actually-present

The audit's skeptic pass overturned 15 rulings workspace-wide, so each `missing` was treated as a
lead. Re-checking the 42 against the tree found **six rows whose remaining obligation is materially
smaller than the census says**, including the section's only `critical`. Each is evidenced by a
command; nothing below is inferred from a name.

| unit | census | what is actually in the tree | what genuinely remains |
|---|---|---|---|
| `MCP-455` | missing | **The three-branch gate is built and unit-tested.** `owner.rs:663 pub async fn confirm_sampling` is upstream's order verbatim (auto-approve → no-UI → declined); `SamplingApproval` (`owner.rs:629`) carries the explicit `has_ui: bool` the unit singles out as un-inferrable; both message constants (`owner.rs:614`, `owner.rs:618`) and both titles (`owner.rs:604`, `owner.rs:607`) are present, with tests at `owner.rs:1092-1160` covering all three branches plus the `has_ui && dialog.is_none()` wiring case. | The three formatters only — `formatRequestApproval`, `formatResponseApproval`, `messageText` (`grep -rn 'format_request_approval\|format_response_approval' crates/ --include=*.rs` returns one hit, a doc reference at `owner.rs:58`, and no definition) — plus the production call site. **Reclassify `missing` → `partial`.** |
| `MCP-471` | missing | **Both guards are taken, at a single chokepoint.** `McpDialog::enter` (`owner.rs:560-571`) acquires `HostServices::human_interaction_lock` and then `HostCtx::begin_human_wait`, and both `McpDialog::confirm` (`owner.rs:575`) and `McpDialog::select` (`owner.rs:582`) bind the pair to a named `_guards` local. The type's doc already records the honest limit of the P-3 guard at today's call sites. | An `input` method on `McpDialog` (none exists), and the loop-scoping the unit requires — `MCP-463`'s unbounded re-prompt loop must hold the guard across the **loop**, which cannot be written before that loop exists. Plus the `cyrup-it` verify. **Reclassify `missing` → `partial`.** |
| `MCP-469` | partial | **The registry and the dispatch both landed since the audit.** `accepted_url_elicitations: HashMap<String, HashSet<String>>` (`server_manager.rs:1224`) with `remember_url_elicitation` (`:2582`, no-op once the runtime signal fired), `forget_url_elicitation` (`:2601`, returning `bool` with `Set.delete` semantics), per-server clear (`:2265`) and wholesale clear (`:2470`), tested at `server_manager.rs:3877-3899`. `on_custom_notification` (`runtime.rs:1609-1655`) routes `ELICITATION_COMPLETE_METHOD`, gated on `aborted \|\| !allow_url`, and hands off to the hook. | Only the hook producer and its notice text. `elicitation_complete` is `None` at the one production construction (`runtime.rs:1942`), and `grep -rn 'completed. You can retry the tool now' crates/` returns **0 hits**. |
| `MCP-477` | partial | The chosen side is already written into the code, not just the plan: `config.rs:1031` and `config.rs:1624` document the default as `<cwd>/.cyrup/mcp-traces/mcp-<ts>-<rand>.jsonl`. | The derivation function itself. The decision half is effectively taken; do not re-open it. |
| `MCP-481` | partial | More than "nothing consumes it": `TraceSettings` (`config.rs:1620`) has all four fields, the per-server `trace: Option<bool>` is at `config.rs:876`, and `boundedPositiveInteger` is already ported twice — `trace_max_bytes`/`trace_max_events` (`config.rs:1271-1280`) over `DEFAULT_MCP_TRACE_MAX_BYTES = 256 * 1024` and `DEFAULT_MCP_TRACE_MAX_EVENTS = 10_000` (`config.rs:1051-1054`). | Consumers, and `MCP-478`'s combining function. The settings surface is done. |
| `MCP-491` | partial | **The premise moved.** `grep -c '^\[\[test\]\]' crates/cyrup-it/Cargo.toml` returns **8**, not 7, and one of them is `name = "mcp"`, `path = "tests/mcp/main.rs"` (`cyrup-it/Cargo.toml:198-204`), populated with `tests/mcp/{main.rs,activation.rs}` (272 lines). | The plan's recommendation — "(b) fold into `bin` and `session_svc`" — is retrospective advice about a target that already exists. The live question shrinks to two placements: where `MCP-498`'s child-process harness goes (`bin` or `mcp`) and where `MCP-492`'s port-binding OAuth suite goes. The doc reconciliation and the guardrail-threshold question survive unchanged. |

Three more rows are confirmed-missing but are **cheaper than their neighbours** and should not be
scheduled by severity order:

* **`MCP-493`** (`low`, manifest policy test) has **zero** dependencies and can be written today.
  The manifest it must assert is already in the asserted state: `crates/cyrup-mcp/Cargo.toml`
  declares `rmcp = { version = "3.1.2", default-features = false, features = ["client",
  "transport-child-process", "transport-streamable-http-client-reqwest", "reqwest", "auth"] }` —
  exactly the five, `server` and `elicitation` both absent — and `toml = "1.1.2"` is already a
  dependency, so the test needs no manifest change of its own. It is the one 13i unit that is a
  pure net add with no seam.
* **`MCP-465`** needs no new dependency: `jsonschema` is already declared for `cyrup-mcp`
  (workspace pin `0.46.9`, `default-features = false`) — and note that
  `grep -rn 'jsonschema' crates/cyrup-mcp/src/` returns **0 hits**, so today it is a declared-but-unused
  edge shared with the still-missing `MCP-092`. Whoever lands either unit should confirm the version
  bump the unit mentions against `should_validate_formats`.
* **`MCP-467`** needs no new dependency either: `url` and `opener = "0.8.5"` are both already
  declared in `crates/cyrup-mcp/Cargo.toml`.

Two rows are **blocked on another 13i unit rather than missing in their own right**:

* **`MCP-462`** — the unit is a *rule about an iteration site* (`zip property_order against
  properties`, never iterate the `BTreeMap`). No such site exists, and none can exist before
  `MCP-461` writes the form loop. It is not independently schedulable; it is an acceptance
  criterion on `MCP-461`.
* **`MCP-460`** — the dispatch is `rmcp`'s already (the `LegacyForm` untagged arm), so the unit's
  behaviour is free; what is missing is the two handlers to dispatch *to*. Same relationship:
  an acceptance criterion on `MCP-461` + `MCP-467`, not a unit of its own.

That accounts for 11 of the 42. The remaining **31 stand as the census filed them**: 25
confirmed-missing — `MCP-450`, `451`, `452`, `453`, `454`, `456`, `458`, `461`, `463`, `464`, `466`,
`472`, `473`, `474`, `475`, `476`, `479`, `480`, `483`, `484`, `485`, `486`, `487`, `496`, `498` —
and 6 still-`partial` — `MCP-468`, `470`, `478`, `490`, `492`, `495`. (6 re-checked + 2
blocked-on-another-unit + 3 annotated-but-missing + 25 + 6 = 42.) Spot-checks behind that: `grep -rIn`
over `crates/` returns 0 hits for each of `handle_sampling_request`, `SamplingOptions`,
`sampling_candidates`, `handle_form_elicitation`, `coerce_and_validate`, `TracingTransport`,
`McpTraceEvent`, `redact_trace`, and 0 of the workspace's 23 `Cargo.toml`
files (`find . -name Cargo.toml -not -path './target/*'`) name `portable-pty`, `expectrl` or
`rexpect` — `MCP-496`'s "no pty infrastructure" claim, re-confirmed.

##### Dependency order

Read as a partial order; anything not named as a predecessor is independent.

1. **Nothing in 13i gates the tracer.** `MCP-473 → 474 → 475 → 476 → 477 → 478 → 481` are pure
   functions and a writer with an injectable fs seam. `MCP-479` needs the event and the writer;
   `MCP-480` needs `479` plus `478`'s enablement combiner. The tracer is the section's only chain
   with no host, no dialog and no provider in it.
2. **Elicitation is two layers.** The value pipeline (`MCP-464`, `465`, `466`) is pure and comes
   first; the dialog sequence (`MCP-461`, `463`, `467`, and the `MCP-471` residue) consumes it.
   `MCP-462` and `MCP-460` are acceptance criteria on that second layer, not separate work.
   `MCP-472` is three error constructors inside `MCP-467`'s handler and lands with it.
3. **Sampling is three layers.** Model resolution (`452`, `453`, `454`) and wire translation
   (`451`, `456`) are independent of each other and of everything else; the handler (`450`) is the
   assembly point and needs both, plus `455`'s formatters and `458`'s live closures.
   `MCP-459` (`truncate_at_word`) is already implemented and is `455`'s only prerequisite.
4. **Advertisement follows behaviour.** `MCP-468` (`elicitation:{form,url?}`) and the `MCP-469`
   hook producer must not land before the handlers exist, or the client advertises a capability it
   cannot service — the one ordering error in this section that is visible to a remote server.
   `MCP-470` needs `467` (it drives URL elicitations) and a production implementor of
   `ProxyEnv::handle_url_elicitation_required` (`proxy.rs:1486`; the call site is `proxy.rs:3731`
   and the only impl is at `proxy.rs:4973`, inside `#[cfg(test)]`).
5. **Conformance is gated on elicitation, not on sampling.** `MCP-484`'s scripted UI exists to make
   `elicitation-sep1034-client-defaults` traverse the *real* form handler, so the driver is worth
   little before layer 2 lands. `MCP-483 → 484 → 485 → 486` is strictly sequential (the baseline is
   written from one observed run of the runner). `MCP-487` may **dissolve** rather than land: it
   only exists if `MCP-485` chooses a fixed callback port, and `docs/TEST-ARCHITECTURE.md`'s R4
   pushes the other way.
6. **Verification trails everything.** `MCP-490`'s 13i share is zero until 13i's code exists — that
   is the census row's own wording. `MCP-496` needs both consent gates and the whole elicitation
   sequence, *plus* pty infrastructure that does not exist in the workspace. `MCP-498` needs a
   `cyrup-it` target and is discussed under HA-1 below. `MCP-492` needs `MCP-491`'s placement
   ruling.
7. **Gated on other sections, not on 13i.** The `HandlerFactory` installation (§13c manager) and
   `initialize_mcp`'s production caller (§13a activation) gate only the *end-to-end* proofs —
   `MCP-457`'s and `MCP-468`'s `cyrup-it` capability tests, `MCP-479`'s tracing-on/off differential,
   `MCP-480`'s two-server file test, `MCP-471`'s budget test. None of them gates writing the code.

##### The `host-addition` neighbours — recommendation: schedule none of them for 13i

**Zero of 13i's 50 units carry the `host-addition` verdict** — the verdict column of the table above
holds only `hand-written`, `rmcp`, `host-verb`, `extension-owned`, `open-decision` and `cut`. The
section file says so directly: *"The section's three `host-addition` neighbours (`HA-1` late tool
registration, `HA-2` argument completions, `HA-3` overlay geometry) are owned elsewhere and none of
them gates sampling, elicitation or tracing"* (`13i-mcp-protocol-and-verification.md:1776`). `HA-2`
and `HA-3` have **no** contact surface in 13i at all; they appear on 13h units (`MCP-382`, and
`MCP-395` for `HA-1`'s command leg).

The single point of contact is `MCP-498`'s note
(`13i-mcp-protocol-and-verification.md:1706-1709`): on a cold cache cyrup exposes only the `mcp`
proxy tool unless `HA-1` is built, so the upstream test's "a direct MCP tool is registered before
`agent_start`" assertion is about the **warm** path here.

**Recommendation.** Land `MCP-498` against the warm path, and say so in the test name and its module
doc rather than in a comment — the unit's own instruction is "state which you are testing". File the
cold-cache assertion as a follow-up owned by whoever lands `HA-1`, so it is a known deferred
assertion rather than a silently weaker test. Do **not** pull `HA-1` into a 13i wave: it is a
host-surface change whose consumers are 13a and 13h units, and adding it here would make the
section's largest wave depend on a crate this section otherwise only reads. `HA-2` and `HA-3` need
no 13i decision at all.

##### Proposed waves

Grouped by **shared obligation**, per PR #30. The failure mode being avoided is the one that put
`runtime.rs` in a different agent's set than the unit whose obligation needed it: each wave below is
one sentence of contract, and every file that sentence touches is inside it.

| wave | obligation, in one sentence | units | size | predecessors |
|---|---|---|---:|---|
| **0** | The manifest cannot drift from the settled rmcp feature set without failing a test. | `493` | 1×S | none |
| **A** | A bounded, redacted, ordered JSONL file exists and can be written without a transport. | `473`, `474`, `475`, `476`, `477`, `478`, `481` | 6×S + 1×M | none |
| **B** | Turning tracing on changes the file and nothing on the wire. | `479`, `480` | 1×M + 1×S | A |
| **C** | A server's declared constraints are enforced on the values before anything is returned. | `464`, `465`, `466` | 2×M + 1×S | none |
| **D** | Every question this crate asks a human is guarded, ordered, cancellable, and never partially submitted. | `461`, `463`, `467`, `472`, `471`-residue, and `460`/`462` as acceptance criteria | 2×M + 5×S | C |
| **E1** | A nested completion runs on the right model with the session's own credentials. | `452`, `453`, `454` | 2×M + 1×S | none |
| **E2** | What crosses the MCP↔provider boundary is byte-for-byte what upstream sends, including its refusals. | `451`, `456` | 1×M + 1×S | none |
| **E3** | No server-directed completion happens without consent, and none outlives its turn. | `450`, `455`-residue, `458` | 2×M + 1×S | E1, E2 |
| **F** | The client advertises exactly the capabilities it can service, and services them. | `468`, `469`-residue, `470` | 3×S | D, E3; §13c's manager factory |
| **G** | An external referee grades the real client stack, and the baseline is observed rather than copied. | `483`, `484`, `485`, `486`, (`487`, may dissolve) | 1×M + 4×S | D |
| **H** | Every algorithm 13i owns is pinned by a test that fails on the pre-fix tree. | `490` (13i share), `492`, `495`, `498` | 1×L + 3×M | D, E3, A; `491` ruling for placement |
| **spike** | Decide whether a pty harness enters the workspace at all, before `496` is scheduled as work. | `496` | 1×M | D, E3 |

Notes on the sizing:

* **A is the wave to start with**, not wave 0 and not the `critical` unit. It is seven units with
  one shared obligation, no host surface, no dialogs, no provider, and no dependency on any other
  section — the same shape as the waves that worked in PR #30. Wave 0 is a single test and can ride
  along with anything.
* **C and D must not be split by file.** `MCP-464`'s coercion, `MCP-465`'s final assertion and
  `MCP-463`'s re-prompt loop are one user-visible behaviour (a bad value is rejected with the
  server's own text and the user's typing is not lost); an agent holding only one of them cannot
  tell whether a failing case is its own bug.
* **D carries the `MCP-471` residue deliberately.** The guard is already at one chokepoint
  (`McpDialog::enter`); the remaining work is an `input` method and *where the guard is taken*
  around `MCP-463`'s unbounded loop. That is a property of the loop, so it belongs to the agent
  writing the loop.
* **E1, E2, C and A are mutually independent** and can run concurrently — four agents, no shared
  file among them beyond `config.rs` (A only) and new modules.
* **F is the first wave that needs another section.** Do not schedule it until §13c's manager owns
  the `HandlerFactory`; scheduling it early produces a wave that can write code but cannot prove it.
* **H is trailing by construction**, and `MCP-490`'s `L` is the mocking rewrite, not the assertion
  translation — budget it as such.

##### Open decisions this pass did not settle

Recorded so they are not mistaken for work: `MCP-489` (fixture strategy), `MCP-491` (seam-test
placement — narrowed above, not closed), `MCP-494` (the CI gate's shape) and `MCP-499` (the trace
differential harness) remain `not-applicable`/`open-decision` and need a ruling from the owner, not
an agent. `MCP-477`'s `.cyrup` side is taken in the code and should be treated as settled.

## Rulings the skeptic overturned

Recorded because they are the measure of how much to trust the rest, and because each is a place
where the Rust implements the contract under a name the plan does not use.

| id | first pass | corrected to | why |
|---|---|---|---|
| `MCP-069` | partial | **implemented** | REFUTED. MCP-069's only obligation about this message is 'The exactly-one-transport message loses `, or socket`'. Upstream is `Server ${name} must configure exactly one of command, url, or socket` … |
| `MCP-069a` | missing | **not-applicable** | NOT-APPLICABLE by verdict class. The canonical table gives MCP-069a the verdict `hand-written` + `open-decision`, and the plan text itself says '*Filed 2026-08-20 by the v2.25.0 → v2.26.1 retarget. NOT implemented.*' … |
| `MCP-096` | partial | **not-applicable** | REFUTED / NOT-APPLICABLE. The canonical table gives MCP-096 the verdict `open-decision`, and the plan's own words are 'This is the only genuine open decision in the section, and it is a policy choice, not a missing … |
| `MCP-152` | partial | **implemented** | The hand-written half of MCP-152 is complete and tested. The description is built with `write!`-style assembly in TWO byte-identical copies (registration.rs:1196 from the cold cache, proxy.rs:3797 from live metadata) … |
| `MCP-159` | partial | **implemented** | Every item in MCP-159's **verify** list exists. (1) The re-specified catastrophic-backtracking case: proxy.rs:5471-5481 compiles `(a+)+$`, runs it, and asserts completion under a 250 ms wall-clock bound, with the … |
| `MCP-178` | partial | **implemented** | MCP-178 is verdict **open-decision**, and the Rust has already picked a side — option (a): cyrup-mcp implements the adapter's FOUR-mode, hyphen-preserving grammar (`ToolPrefix::{Server, None, Short, Mcp}`, … |
| `MCP-199` | partial | **implemented** | REFUTED — the claim rests on a misreading. owner.rs:408 is NOT 'a stale-generation no-op that always returns None': `OwnedServices` is `createOwnedUi`'s fence (owner.rs:279-340), and the `fenced!` macro (:317-337) … |
| `MCP-205` | partial | **not-applicable** | Verdict is **open-decision**, and no ruling has been recorded: registration.rs:179 says verbatim "MCP-205, unresolved" and proxy.rs:419 says "MCP-178/MCP-205 open decision". Per the verdict class this is not-applicable, … |
| `MCP-234` | partial | **not-applicable** | Verdict is **open-decision** with no ruling recorded, so by the class rule this is not-applicable rather than outstanding work. Nothing behavioural is missing: the plan's recommended (c) is 'document the split and match … |
| `MCP-291` | partial | **implemented** | REFUTED on its central point. (1) Both traits ARE implemented over the keychain — `McpCredentialStore` (credentials.rs:3103/3128) and `McpStateStore` (3173/3196) — which is what the unit's **cyrup** paragraph asks for. … |
| `MCP-314` | partial | **implemented** | REFUTED — the claim inverts the requirement. MCP-314's cyrup column asks for exactly three things and all three are present: persist the registration fields as a second keychain record (oauth.rs:3095-3111 writes … |
| `MCP-318` | partial | **implemented** | REFUTED as port work. The canonical table's verdict is `rmcp`, and the ONLY hand-written obligation the plan names is the client-auth lever — 'the lever that exists is AuthorizationManager::set_metadata: fetch or accept … |
| `MCP-333` | partial | **implemented** | REFUTED — the auditor judged MCP-333 against a scope the plan explicitly excludes. 13g's Coverage/Excluded section says: '`server-manager.ts` beyond the auth-provider seam — transport construction and the connection … |
| `MCP-393` | partial | **implemented** | REFUTED. This unit names two things — `buildSharedConfigNoticeLines` and the one-shot state — and both are ported, byte-for-byte on the strings: `shared_config_notice_lines` (ui.rs:4694-4716) reproduces the … |
| `MCP-492` | partial | **partial** | REFUTED on its central assertion. The claim says 'three of the four surviving upstream files have no port'; all three do have substantial ports, they are just not named after the .ts files. (a) … |

