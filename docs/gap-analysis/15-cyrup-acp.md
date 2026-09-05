# 15 — cyrup-acp

`cyrup-acp` is the **Agent Client Protocol adapter**: the surface that lets an editor — Zed is the
reference client — drive cyrup over ACP JSON-RPC 2.0 on stdio. The editor sends `initialize`,
`session/new`, `session/prompt`, `session/cancel`, `session/load`, `session/list`, `session/delete`,
`session/set_mode` and `session/set_config_option`; cyrup answers, and streams the turn back as
`session/update` notifications plus `session/request_permission` requests. It is not a new agent, a
new tool tier or a new provider — it is a **fourth front-end** beside the TUI, `--mode rpc` and
print/json, and everything below the front-end is the `AgentSession` those three already drive.

> **Provenance.** Upstream is **`svkozak/pi-acp` v0.0.33**, MIT © Sergii Kozak. Clone HEAD is
> `d1cffc0` = `v0.0.33-2-gd1cffc0`; **both commits past the tag are README-only** and the
> `v0.0.33..HEAD` window touches no `src/` path, so the tag and HEAD are the same port target.
> Reference checkout is `./tmp/pi-acp`, which is **gitignored** — read it with
> `git -C tmp/pi-acp show v0.0.33:<path>`, never from the working tree. Size at the tag: **17
> TypeScript files and 4 238 lines under `src/`**, plus **3 265 lines of `test/` across 33 files**
> and **10 `scripts/smoke-*.mjs`** harnesses beside it. (The 33 is measured from the tag —
> `git ls-tree -r --name-only v0.0.33 | grep -c '^test/'` — and corrects a "25 files" figure carried
> into this port's brief; the 3 265 lines is confirmed.)
>
> **The Rust binding is `agent-client-protocol = "2.1"`, default features, no feature flags.** Its
> schema crate is pinned transitively at `agent-client-protocol-schema =1.7.0`, whose default
> surface *is* protocol v1 and is enforced as such. **Do not enable `unstable_protocol_v2`** — it
> activates a real guard in v1 mode that hard-errors an unknown `protocolVersion` where pi-acp
> downgrades gracefully. Both crates are **Apache-2.0**, the first Apache-2.0 edge in an MIT
> workspace. pi-acp itself targets the **TypeScript** SDK `@agentclientprotocol/sdk ^0.26.0`;
> method-name parity between that SDK and the Rust crate is confirmed for every method this port
> touches.
>
> **150 port units — 7 critical · 25 high · 67 medium · 51 low.** Derived by counting the tables
> below, which are the source of truth: **132 units filed by the five area surveys, 18 struck by the
> adversary passes** (listed per area, not deleted), leaving 114, **plus 36 units the adversary
> passes filed that no survey had**. Every area had an adversary complete, so no area is
> wholly single-source; the 36 adversary-filed units are individually marked `single-source` in
> `## Open items` because only one pass has seen them.
>
> **Implementation status — CORRECTED 2026-09-05.** This paragraph previously read *"Nothing in this
> document is implemented. `crates/cyrup-acp` does not exist."* **That is no longer true and must not
> be relied on.** The crate landed at `0aefd08` and merged to `main` at `ef4448d` (PR #123): 12
> modules, 18 043 lines, 216 unit tests, plus `acp_session.rs` and `acp_transport.rs` in `cyrup-it`.
>
> What remains true from the original wording: the unit tables below were written as a **design**,
> so a `verify` line states what was intended, not what has been observed. Treat a `verify` line as a
> specification to check the code against — **not** as evidence the check was run. Section 6 is the
> authority for what is open; the merge did not close every unit, and the tool-call streaming and
> structured-diff paths in particular are unit-tested but were never exercised end to end (no
> credentials to complete a `session/prompt`).
>
> **The Rust type design is a companion document, not a duplicate.**
> [`../adr/ADR-0028-cyrup-acp-type-design.md`](../adr/ADR-0028-cyrup-acp-type-design.md) decides the
> shapes — the `Turn` typestate, `AcpSessionId`, `AppliedMode`, `TerminalAppender`, `Snapshot`,
> `DialogChoice` — and states what each does *not* guarantee. Units below name those types where
> their mechanism depends on one; they do not restate the argument.

---

## 1. The architecture decision

**`cyrup-acp` binds to `AgentSession` in-process, as a new `AppMode::Acp` beside
`Interactive`/`Print`/`Json`/`Rpc`, with a one-live-session `SessionManager`.** This is the
inversion of its upstream and it is the single fact a reader most needs before opening any unit.

### Why the question exists at all

pi-acp is an **out-of-process adapter by necessity**. It is a separate npm package that cannot link
into pi, so it spawns `pi --mode rpc` as a child and bridges two wires: ACP JSON-RPC on its own
stdio, and pi's newline-delimited JSON on the child's. Every fact it holds about the agent arrives
as an untyped `Record<string, unknown>` off that second wire, which is why roughly 40% of its code
is defensive key-probing — `translate/bash.ts` probes twelve key paths to recover one command
string, and `translate/pi-tools.ts` runs a four-deep fallback ladder to find stdout.

cyrup has no such constraint. `cyrup-acp` is a workspace crate in the same binary, and cyrup's own
RPC mode (`crates/cyrup-modes/src/rpc/mod.rs`) is a 1:1 port of pi's `rpc-mode.ts` — so an
out-of-process design would also work, and `cyrup_modes::RpcClient`
(`crates/cyrup-modes/src/rpc_client.rs`) is already a strictly better `PiRpcProcess` than the one
upstream hand-rolled: typed options, id correlation, a request timeout, a stderr accumulator, an
early-exit settle check pi-acp lacks, and a SIGTERM→SIGKILL escalation. **That is the strongest
argument for the design this document rejects, and it belongs on the record.**

### The two arguments that decided it

**The out-of-process design cannot see what ACP needs, by contract.** `cyrup_modes::is_upstream_wire_event`
deliberately keeps `SessionReplaced`, `ModelChanged`, `SessionStart` and `SessionShutdown` *off* the
RPC wire — they are cyrup super-set members pi's `session.subscribe` cannot deliver — and
`extension_ui_effect_json` returns `None` for seven `UiEffect` variants. An out-of-process
`cyrup-acp` would therefore have no source for ACP's `current_mode_update` or
`available_commands_update`, and no way to learn that the session was replaced under it. Everything
else it needed would arrive as JSONL to be re-parsed, which is the 40% of pi-acp that is key-probing,
reintroduced against a wire cyrup itself defines.

**The permission seam only closes in-process.** Over RPC, an ACP host sees an `extension_ui_request`
with a title and a method and cannot tell a permission ask from any other dialog — which is exactly
why pi-acp's `handleExtensionSelect` synthesizes `allow_once` options for *every* select. In-process
the sink receives the real `cyrup_session_svc::UiRequest` with its typed `UiKind` and its embedded
`oneshot::Sender<UiReply>`, and `cyrup-permission-system`'s own decision states are available for a
faithful mapping onto ACP's `PermissionOption.kind`. `session/request_permission` is the single
richest thing ACP offers that pi's RPC wire does not; giving it a degraded input is the wrong trade.

### What the decision costs

**One live session per connection, structurally rather than by policy.** pi-acp's `SessionManager`
is a genuine `Map<string, PiAcpSession>` of N live children and its `closeAllExcept` is leak
avoidance it could drop tomorrow. cyrup's `AgentSessionRuntime` (`crates/cyrup-session-svc/src/runtime.rs`)
holds one `Arc<AgentSession>` in one slot behind one generation watch — it is a replacer, not a
multiplexer — and N sessions in one process are unsafe today for reasons `cyrup-acp` does not own:
`NativeExtension::set_host_services` stashes the host-services `Arc` in **first-write-wins
`OnceLock` slots** inside `PermissionSystemExtension` (`crates/cyrup-permission-system/src/extension/mod.rs`),
`McpExtension` (`crates/cyrup-mcp/src/extension.rs`) and `FluxExtension` (`crates/cyrup-flux/src/extension.rs`),
so session B's permission dialog would open on session A's `UiSink`; and `RUNTIME_API`
(`crates/cyrup-permission-system/src/runtime_api.rs`) and `ROOT_PARENT_SESSION_ANCHOR`
(`crates/cyrup-ext-subagents/src/background/parent_anchor.rs`) are process-global last-writer-wins
slots on the permission path. In practice Zed opens one ACP connection per project window and this
is invisible; a client that opens two workspaces on one connection gets B evicting A.

**A second `session/new` with a different cwd needs a new `SessionFactory`, not a second `build()`.**
`crate::session_launch::attach_native_extensions` (`crates/cyrup/src/session_launch.rs`) bakes
`session_cwd` into each built-in at *factory construction* time, so `SessionFactory::build(target, Some(other_cwd))`'s
advertised cwd rebind does not reach the native extensions. This contradicts the naive reading of
the in-process premise and is a pre-existing defect independent of ACP —
`AgentSessionRuntime::switch_session` with a `cwd_override` has the same hole today.

**What would flip the decision.** Any one of: ACP must host sessions across different cyrup versions
or different agent binaries simultaneously; crash isolation becomes a stated requirement (a panic in
`cyrup-tools` or a provider must not take the editor's agent connection down); or N-session becomes
hard *and* the native-extension host-services slots stay `OnceLock`. The flip is cheap, because both
designs consume the identical `AgentSessionEvent` vocabulary — one through `AgentSession::prompt`'s
run-scoped stream, one through `RpcClient`'s `EventSubscription` — so the fork point is the sink
layer and nothing above it.

### The one finding that argues against in-process, and is not resolved here

**`serde_json/preserve_order`.** `agent-client-protocol` 2.1.0 **and** `agent-client-protocol-schema`
1.7.0 both declare `serde_json = { version = "1", features = ["preserve_order", "raw_value"] }`
**non-optionally**, with no cargo feature to turn it off. Cargo feature unification is graph-wide and
additive, so adding either as a normal dependency of any workspace member flips `serde_json::Map`
from `BTreeMap` to `IndexMap` for the entire shipped `cyrup` binary. **This workspace has already
litigated exactly that, twice in writing**: `/home/user/cyrup/Cargo.toml`'s `mermaid-text` block
rejects `mermansi` solely for it, recording that the flip "broke two pre-existing
`cyrup-ext-subagents` tests, and only under `cargo test --workspace`", and `xtask/Cargo.toml` is
deliberately dependency-free for the same reason. Measured state: `cargo tree -e features -i serde_json`
in the workspace resolves two roots and only the **build-dependency** root (via `tree-sitter`'s build
script) carries `preserve_order`; in `tmp/acp-probe/` the **normal** root does. Three options —
accept the flip and audit every site that depends on `BTreeMap` key ordering (config persistence,
provider request bodies, MCP payloads, session JSONL records); `[patch.crates-io]` a fork of the two
ACP crates with the feature dropped; or keep `cyrup-acp` out of the workspace binary, which
contradicts the decision. **This is `ACP-Q1` in `## Open questions` and it is the highest-leverage
unresolved item in the area.** It is not `critical` on the house scale — nothing is silently wrong
*yet* — and blocking-ness is not severity.

### What the decision deletes outright

| upstream surface | why it has no counterpart |
|---|---|
| `src/pi-rpc/process.ts`'s `PiRpcProcess.spawn`, the spawn/error race, `PiRpcSpawnError` and its three ENOENT/EACCES/other messages | there is no child process |
| `src/pi-rpc/command.ts` entire — `defaultPiCommand`, `getPiCommand`, `shouldUseShellForPiCommand`, and `PI_ACP_PI_COMMAND` | every line is an npm-installation assumption; a Rust binary has no `.cmd` shim to detect and the adapter *is* the agent |
| the UUID correlation map, the exit/error rejection of pendings, and the fourteen `pi <verb> failed:` strings | correlation is the call stack; "the peer died" has no analogue |
| the ANSI prelude buffer, `stripAnsi`, `consumePreludeLines` | recovers human text written to a shared fd; also already dead upstream — `rg 'consumePrelude'` over the tree matches only its own definition |
| `translate/bash.ts`'s twelve-path `bashCommand` probe and `translate/pi-tools.ts`'s stdout ladder | `ToolExecutionStart/Update/End` are typed and all three carry `tool_name` |
| `src/acp/slash-commands.ts` **entire** — 197 lines | its stated premise ("pi RPC mode disables slash command expansion, so we do it here") is **false for cyrup**: `AgentSession::prepare_and_assemble` (`crates/cyrup-session-svc/src/session/run.rs`) expands unconditionally when `UserInput::expand_templates` is set |
| `src/acp/pi-settings.ts` **entire** | `cyrup_config::EffectiveSettings` + `settings::merge::deep_merge` + `settings::migrate::migrate_settings` cover every getter, merge rule and back-compat key |
| `src/acp/session-store.ts` and `src/acp/paths.ts` — the `~/.pi/pi-acp/session-map.json` sidecar | the mapping is derivable; see `ACP-222` for the one place that derivation is currently unsound |
| the two `setTimeout(…, 0)` deferrals in `newSession` and `loadSession` | `Responder::respond` and `ConnectionTo::send_notification` both enqueue synchronously on the same outgoing channel, so respond-then-notify orders itself with no timer |


---

## 2. What cyrup already has

The table the ground truth opened with, corrected by the adversary passes. Every cyrup symbol below
was read; the corrections are marked, because three of the original rows were wrong in ways that
would have cost a porter a session each.

| pi-acp hand-rolls | cyrup symbol that already covers it | note |
|---|---|---|
| parses NDJSON `Record<string, unknown>` events | `cyrup_session_svc::AgentSessionEvent` (`crates/cyrup-session-svc/src/event.rs`) — a typed enum of **27 variants** | **understated in the ground truth**: it also carries `SummarizationRetryScheduled` / `…AttemptStart` / `…Finished` and `SessionReplaced` |
| `src/acp/pi-sessions.ts`, 333 lines of head/tail/whole-file byte scanning | `cyrup_session::listing::{list_all, list_in_dir, list_all_with_progress, scan_file, SessionInfo}` + `layout::{SessionLayout, SessionsRoot, encode_cwd}` | one streaming pass replaces five strategies, and `read_header`'s bounded chunked read fixes an upstream defect where a first line over 64 KiB makes a session vanish from every listing. **Caveat: nothing exposes any of it over a wire** — all 34 arms of `cyrup_modes::rpc`'s `handle` were enumerated and there is no `list_sessions` or `delete_session` verb |
| `src/acp/slash-commands.ts`, 197 lines re-implementing pi's template engine | **`AgentSession::slash_command_catalog`** (`crates/cyrup-session-svc/src/session/commands.rs`) for the catalog; `cyrup_resources::{parse_command_args, substitute_args, expand_prompt_template}` and `PromptTemplate` (`crates/cyrup-resources/src/prompt.rs`) for the engine | **the ground truth's row was wrong.** `cyrup_session_svc::SessionCommand` (`crates/cyrup-session-svc/src/command.rs`) is the *control-verb* enum — `Prompt`/`Steer`/`Abort`/`Compact`/… — and has nothing to do with slash commands. The Rust engine is a strict superset: `$ARGUMENTS`, `${N:-default}`, `${@:N}`, `${@:N:L}`, real YAML frontmatter, `argument-hint`, path-namespaced names |
| `export_html` over RPC | `cyrup_session_svc::export::session_jsonl_to_html` (`crates/cyrup-session-svc/src/export.rs`) | **a different shape, not just a superset**: it is `fn(&str) -> String` — pure, sync, taking the JSONL text — where pi's `export_html` takes an output path and writes. `AgentSession::export_to_html` (`crates/cyrup-session-svc/src/session/transcript.rs`) is the wrapper, and it is where `ACP-291` lives |
| `src/acp/pi-settings.ts`'s hand-rolled deep merge | `cyrup_config::{Settings, EffectiveSettings, SettingsScope, SettingsManager}` + `settings::merge::deep_merge` + `settings::migrate::migrate_settings` | byte-identical merge semantics, and **strictly safer**: `SettingsManager::load(store, project_trusted)` loads the project scope only when the project is trusted, where upstream merges `<cwd>/.pi/settings.json` unconditionally |
| `extension_ui_request` correlated by a wire `id` | `cyrup_session_svc::{UiKind, UiRequest, UiReply, UiEffect}` delivered through a `UiSink`, with the `oneshot::Sender<UiReply>` **inside** the request value | the correlation id cannot be lost because there is no id; `LiveHostServices::ui_roundtrip` (`crates/cyrup-session-svc/src/host_services.rs`) already races the reply against `DialogOptions.timeout` and fails closed |
| `PiRpcProcess.spawn` + spawn diagnostics + prelude scraping | nothing to port | — but if the architecture ever flips, `cyrup_modes::RpcClient` is the whole of it already built |
| `normalizePiMessageText` / `normalizePiAssistantText` (`src/acp/translate/pi-messages.ts`) | `extract_full_content` and `join_text` (`crates/cyrup-session-svc/src/session/transcript.rs`) | **found by the adversary, not the survey**: both are byte-for-byte ports of the same pi helper. Both are private `fn`; the residue is a `pub(crate)` promotion, not a port |

### Where cyrup is a superset, and what that buys

`AgentSessionEvent` carries four variants pi has no analogue for, and they are the reason the ACP
client can be told things pi-acp could never tell it.

- **`QueueUpdate { steering, follow_up }`**, with `AgentSession::{steer, follow_up, emit_queue_update}`
  (`crates/cyrup-session-svc/src/session/queue.rs`), replaces pi-acp's client-side `turnQueue` with a
  real server-side queue that also surfaces *steering*, which pi-acp cannot see at all. **It changes
  ACP semantics**, which is why `ACP-124` files it as a decision rather than an improvement: N
  follow-ups drain inside one run and one `AgentSettled`, so N `session/prompt` requests settle
  together instead of serially, and one cancel cancels all of them.
- **`SessionInfoChanged { name }`** is emitted by `AgentSession::set_session_name` for *every* cause,
  so a rename originating in an extension or another front-end reaches the ACP client. pi-acp only
  ever emits `session_info_update` from inside its own `/name` handler. Same story for
  `ModelChanged` / `ThinkingLevelChanged` and the config-option surface — that is `ACP-077`, which
  has **no upstream at all** and exists because upstream's dropdown desyncs silently.
- **`EntryAppended`** gives the ACP host a persistence signal pi-acp had to infer from message
  traffic. No unit consumes it yet; it is spare capacity, recorded so it is not re-discovered.
- **`BashExecutionUpdate { id, delta }`** is the one the ground truth got backwards, and the
  correction matters: it is the **out-of-loop `!`-command seam** (`crates/cyrup-session-svc/src/bash.rs`,
  pi's `executeBash`), **not** the agent-loop `bash` tool, and the ACP terminal protocol is driven
  entirely by tool events. It is not reachable from `ToolExecutionUpdate`, so it does not make the
  terminal delta better than upstream — see `ACP-140`, where cyrup is materially **worse**.

Three further things cyrup already holds that remove work outright: `cyrup::interactive::build_startup_report`
→ `cyrup_tui::{StartupReport, build_startup_lines}` is the entire sourcing half of `buildStartupInfo`,
already de-duplicated and conflict-resolved, and carries four diagnostic blocks pi-acp cannot see;
`cyrup_session_svc::auth_guidance` carries pi's own remedy text verbatim; and
`cyrup_session_svc::delete_session_file_at` (`crates/cyrup-session-svc/src/session/files.rs`) is
already idempotent on an absent file and already prefers the trash over an unrecoverable unlink.

---

## 3. Cuts

Surfaces removed by decision. Each carries the consequence a porter must propagate somewhere else,
because a cut that loses its consequence is how a defect ships.

| surface (upstream) | why | consequence to propagate | who owns it |
|---|---|---|---|
| `pi-rpc/process.ts` spawn + `PiRpcSpawnError` + the three diagnostic strings; `index.ts:15-18`'s npm install text | no child. All four strings name an npm global install that does not exist for cyrup, and describe a PATH lookup that no longer happens | `session/new` loses its spawn-diagnostic failure class and gains a different one — `SessionBuilder::build` errors including trust refusal, `SessionServiceError::MissingSessionCwd`, and extension-host load failures. `ACP-001` must **not** reintroduce a PATH lookup to keep an analogue alive: it re-enters the current executable, so ENOENT is structurally impossible | `ACP-058`, `ACP-221` |
| `pi-rpc/command.ts` entire + `PI_ACP_PI_COMMAND` + the `shell:` argument | npm shim assumptions | cyrup-acp has **no** configuration point for "which agent binary" and must not invent one. `test/unit/pi-command.test.ts` and `test/unit/new-session-pi-not-found.test.ts` have no port | — |
| request correlation: the `randomUUID` ids, the pending map, the exit/error rejection, the fourteen `pi <verb> failed:` strings | in-process, correlation is the call stack | (1) the fourteen strings are pi-acp's own inventions, not pi's — the replacements are typed `SessionServiceError` variants, several of which already carry pi's verbatim text, so no string table is owed. (2) `sendExtensionUiResponse`'s uncorrelated fire-and-forget shape is the one place the cut removes a **latent defect**, not just a mechanism: a dialog whose child died never settled. Its replacement must still copy the RPC host's pending-map pruning, `pending.retain(\|_, p\| !p.reply.is_closed())` (`crates/cyrup-modes/src/rpc/mod.rs`), because `ui_roundtrip` drops the sender on its own timeout | `ACP-144` |
| the ANSI prelude capture and `stripAnsi` | in-process there is no shared fd to scrape; and it is already dead upstream — no caller, no test | cyrup owns the stdout-discipline half properly: `output_guard::take_over_stdout()` reroutes incidental writes to stderr for every non-`Interactive` mode. **Do not port `stripAnsi`** — `cyrup_session_svc::bash::strip_ansi` is better (OSC and `ESC \` handling upstream's regex lacks) | `ACP-003` |
| `slash-commands.ts` entire, plus the `fileCommands` plumbing through `SessionManager` and `PiAcpSession.prompt` | the premise is false for cyrup (see §1), and every algorithm has an in-tree superset | cyrup-acp holds **no** template state — no `fileCommands` field, no reload on `session/new` or `session/load`, no cwd-keyed cache — and submits raw text. The advertised list comes from `slash_command_catalog`, so it cannot disagree with what expands. Two propagations: cyrup's project root is `.cyrup/prompts`, not `.pi/prompts`; and cyrup's command names are path-namespaced (`flux/new`), so an ACP client sees names containing `/` after the leading slash | `ACP-266`, `ACP-267` |
| `pi-commands.ts`'s defensive layer — the `commands`/`data.commands` shape tolerance, the four `typeof` guards, the `raw` return — and the `try/catch` fallback to file commands in `newSession`/`loadSession` | `slash_command_catalog` is an infallible in-process call over data this workspace emits | pi-acp's fallback meant a failed commands fetch still produced a usable menu. In-process nothing can fail, so no degraded mode is needed — **but if `slash_command_catalog` is ever made fallible this cut must be revisited rather than silently producing an empty menu** | `ACP-267` |
| `pi-settings.ts` entire | every piece has a canonical in-tree owner | **security-relevant, not merely mechanical**: `getMergedSettings` merges `<cwd>/.pi/settings.json` with no trust check, where `SettingsManager::load(store, project_trusted)` gates the project scope. A port that re-read the files directly "to match pi-acp" would reintroduce a trust bypass. Second propagation: settings are read from the **session**, so `initialize` cannot consult them | `ACP-268` |
| `pi-settings.ts`'s `quietStart` legacy alias | speculative back-compat for an unspecified older pi. `migrate_settings` is a 1:1 port of pi's own four migrations and contains no such entry; no file in `crates/` mentions the key | `quietStartup` is the only spelling. Adding the alias would create a settings key cyrup has never emitted | — |
| `buildUpdateNotice` + `isSemver` + `compareSemver` — the `spawnSync('npm','view',…)` check and its `npm i -g …` notice | an npm-distribution assumption. **The workspace already litigated this**: `crates/cyrup/src/update_check.rs`'s module doc records that pi's release-feed check was deliberately not ported because cyrup "has no such feed and no self-update channel to point at" | this removes the *only* surviving output of the `quietStartup` branch, so under `quietStartup` the prelude is empty — which is exactly what `test/unit/startup-info-env.test.ts` covers with its `timeouts.length === 1` leg. The surviving update surface is `cyrup::update_check::{check_for_available_updates, spawn_package_update_check}`, which an ACP host has no channel for; routing it into a `session/update` would be a **new** unit, not a port | `ACP-066` |
| `readNearestPackageJson`'s six-level directory walk | Rust bakes identity in at compile time — `cyrup::startup::PACKAGE_NAME` is `env!("CARGO_PKG_NAME")` | the two `??` fallbacks in the `agentInfo` literal become **unreachable and must not be written**; a `.unwrap_or("cyrup")` there is dead code asserting an impossible failure. Also removes the last filesystem read from `initialize`, which becomes a pure function of the request | `ACP-051` |
| `terminalAuthLaunchSpec`'s `argv[0].includes('node') && argv[1].endsWith('.js')` sniffing | `schema::v1::AuthMethodTerminal` carries `id`, `name`, `description`, `args`, `env` and `_meta` and deliberately **no `command`** — the client re-launches the invocation it already has | the typed `AuthMethod::Terminal` carries only the extra `args`, so the `_meta` shim becomes a legacy-client fallback rather than the primary path. The remaining decision is what `args` contains, and cyrup **has no `--terminal-login` flag today** | `ACP-011`, `ACP-013` |
| `buildStartupInfo`'s discovery half — the `AGENTS.md` probe, the three-root recursive skill walk, the two `readdirSync`s, the `packages` re-read | a blind re-derivation of state the session already holds resolved | three behaviour changes to record, not absorb: items become **names** not absolute paths (which is what pi's own `showLoadedResources` does, so this moves *toward* pi); duplicates are resolved rather than double-listed; and the four diagnostic blocks become available for the first time — pi shows them even under `quietStartup`, and pi-acp could not show them at all | `ACP-066` |
| `pickFallbackTitleFromHead`'s literal form — the whole-file `readFileSync` and its `if (lines.length > 2000) break` | the guard is a **no-op bug**: `lines.length` is loop-invariant, so any file over 2 000 lines breaks after examining line 1 (the header, never a user message) and returns `null` — after having already read the whole file | the **behaviour must not be preserved**. `SessionInfo.first_message` finds the first user message regardless of size, so cyrup produces a title where pi-acp produced `null`. A fixture ported from pi-acp asserting `title === null` for a large unnamed session will fail, correctly | `ACP-205` |
| `walkJsonlFiles`' unbounded recursion | the real layout is two levels, which `listing::list_all` walks directly | **a `*.jsonl` nested deeper than one level under the sessions root is listed by pi-acp and will not be listed by cyrup.** This is the one case where `session/list` returns *fewer* sessions than upstream. **The survey's stated rationale was wrong** — upstream's `readdirSync(…, {withFileTypes:true})` predicates are lstat-based, so it silently skips symlinks and cannot loop; cyrup's `collect_paths` applies no file-type test and `list_all_with_progress` filters with `p.is_dir()`, which *does* follow symlinks. Both directions are deltas | `ACP-223`, `ACP-230` |
| `crypto.randomUUID()` as the replay `toolCallId` fallback | `Message::ToolResult` carries the real persisted `ToolCallId` | cutting it is a **behavioural fix**: upstream mints a fresh uuid on every `session/load`, so the same historic tool call renders under a different id each reload and no client-side state survives. Assert stability across two successive loads | `ACP-215` |
| `formatAutoRetryMessage`'s `Retrying...` fallback | `AutoRetryStart { attempt: u32, max_attempts: u32, delay_ms: u64, … }` has no optional or stringly-typed fields | delete the arm; do **not** port `test/component/session-events.test.ts`'s fallback case as a passing-by-construction test. The `waiting 0s` → `waiting 1s` bump for sub-second delays is **not** part of this cut and must survive | `ACP-142` |
| the `notify` acknowledgement (`sendExtensionUiResponse({id, cancelled:true})` after the notify chunk) | `UiEffect::Notify` arrives on the fire-and-forget `UiEffectSink`, whose type carries no reply channel at all | one fewer exit path in the dialog handler; the "answer exactly once" invariant now covers only the four `UiKind`s. A notify emitted with no ACP client attached is dropped, matching `LiveHostServices` with no sink installed | `ACP-148` |
| `/changelog`'s `which pi` / `npm root -g` package-root walk and its 20 000-char truncation | an npm-installation assumption with no Rust analogue; spawning `which`/`npm` from an in-process agent to find its own docs would be a new subprocess dependency in a design whose premise is having none | **replace, do not delete silently.** cyrup has a `/changelog` builtin whose answer today is `What's New` / `No changelog entries found.` (`crates/cyrup-tui/src/app/submit.rs`). Either answer with that same string so both front-ends agree, or drop `changelog` from the advertised list — a command that always says "nothing" is worse than no command. Whichever is chosen, the string must not mention pi or npm | `ACP-272` |
| `/export`'s pre-flight guard — the `getState()` probe, the `existsSync`/empty-file tests and their three messages | it defends a *pi RPC defect* its own comment names: pi's `export_html` reads the session JSONL and, when absent, "RPC mode emits an uncorrelated parse error (no id), which would otherwise hang our request". cyrup cannot produce that shape — `export_to_html` never reads the session file (it serialises the in-memory tree via `manager.export_jsonl`), and even a real failure is correlated (`RpcResponse::err` echoes `raw_id`) | **`/export` on a zero-message session now succeeds** and writes an HTML document of an empty transcript, where pi-acp printed "Nothing to export yet". If that is judged unhelpful the right guard is a count check against `session_stats().total_messages` — a typed read — not a port of three strings whose parentheticals describe a mechanism cyrup does not use | `ACP-288` |
| `/name`'s version-skew hint (`/set_session_name/i.test(msg)` → "requires a newer pi version") | a diagnostic for an out-of-process pair; in-process the capability is a compile-time fact | **propagate the general rule**: any pi-acp diagnostic that inspects an error *string* to infer a capability of the other process is a cut. Two other sites in the same area have the shape — `/compact`'s `typeof res.tokensBefore === 'number'` and `/session`'s `JSON.stringify` fallback both probe for fields cyrup's typed `CompactionResult` and `SessionStats` always carry | `ACP-283`, `ACP-284`, `ACP-285` |
| `unstable_setSessionModel` | a three-line duplicate of `setSessionConfigOption`'s `model` branch, funnelling into the same `setSessionModel`. `agent-client-protocol-schema` 1.7.0 has **no** model-setting method at any feature flag, and the only remaining route (`ClientRequest::ExtMethodRequest`) is reachable only for method names beginning with `_` | its behaviour is fully covered by `ACP-073` + `ACP-075`. The wire name the TS SDK 0.26 binds it to is **not determinable from the pi-acp tree** — no `node_modules/`, and `rg 'unstable'` over `src/` and `test/` returns exactly one hit, the declaration itself. If a shipping Zed build is later observed sending a dedicated method, file a **new** unit against that evidence rather than inventing an ext handler | `ACP-Q6` |
| the literal user-visible strings containing "Pi" — `Pi ${method} UI request is not supported in ACP yet; cancelling it.`, `Pi notification`, `Pi ${method}` as a synthetic tool-call title | product copy, not protocol. Reproducing them puts another product's name in a cyrup user's transcript | rewrite and record a `CYRUP-DELTA` at each site so a later byte-parity audit does not "fix" them back. **The asymmetry is deliberate and worth stating**: `Retrying (attempt …)` and `Context nearing limit, running automatic compaction...` contain no product name and **must** be ported byte-for-byte. "Some strings are exact and some are not" is exactly the rule that erodes without a written reason | `ACP-142`, `ACP-143`, `ACP-147`, `ACP-149` |

---

## 4. The port units

Five areas, in the order a porter should meet them. Severity is the house scale — `critical` means
**data loss, silent wrong output, a permission bypass, or a crash on a normal path**, and nothing
else. Blocking-ness ("without this the crate is inert") is scheduling information and lives in the
body, never in the rating. Effort is `XS` under an hour · `S` under a day · `M` a few days · `L` a
week or needs design.

Every area's table is followed by the bodies of its `medium`-and-above units, and then by a
**Refuted** subsection naming the units an adversary pass struck and the cyrup symbol that already
covers each. **That record is as valuable as the units** — house rule 4 exists because filing
something cyrup already does under another name is this repository's most repeated error, and
eighteen such filings were caught here.

### 4a · Transport, process lifecycle and authentication

Upstream: `src/index.ts`, `src/pi-rpc/command.ts`, `src/pi-rpc/process.ts`, `src/acp/auth.ts`,
`src/acp/auth-required.ts`, `src/acp/paths.ts`.

This area collapses harder than any other: all of `command.ts` and everything in `process.ts` except
two residues has no counterpart. What survives is `index.ts` — as three existing cyrup mechanisms
rather than one file — and the whole of authentication, where the architecture changes the *answer*
and not merely the mechanism. pi-acp cannot see inside pi, so it guesses from error strings; cyrup
can, and the eleven-substring list becomes a typed classifier with one anchored string tail.

| id | title | upstream | sev | eff | verify |
|---|---|---|---|---|---|
| ACP-001 | `--terminal-login` argv gate, classified before clap | `index.ts` top-level `process.argv.includes` | high | S | unit on `is_selected` for argv positions 1..n and absence; `cyrup-it` asserts `cyrup --acp --terminal-login` writes zero JSON-RPC frames to stdout |
| ACP-002 | `AppMode::Acp` and the `--acp` / `--mode acp` surface | `index.ts` (implicit: the binary has one non-login role) | high | S | `resolve_app_mode` table test: `--acp` with `stdin_tty=false, stdout_tty=false` resolves `Acp`, not `Print` |
| ACP-003 | Stdio transport bootstrap and `run_acp_dispatch` | `index.ts` `ndJsonStream` + `new AgentSideConnection` | high | M | `cyrup-it`: pipe a hand-written `initialize` frame into `cyrup --acp`, assert a well-formed response frame and exit 0 on EOF |
| ACP-004 | A failed stdout write is a clean exit, not an error | `index.ts`'s `write` body + `stdout.on('error')` | medium | S | `cyrup-it`: send `initialize`, close the read end of stdout, send another request; assert exit 0 and empty stderr |
| ACP-005 | Stdin EOF and close terminate the process, and dispose first | `index.ts` `on('end'\|'close', shutdown)` + `shutdown()` | high | S | `cyrup-it`: drive one prompt, close stdin, assert a `session_shutdown` was emitted and no child process group survives |
| ACP-006 | SIGINT and SIGTERM shut the ACP host down | `index.ts` `process.on('SIGINT'\|'SIGTERM', shutdown)` | low | XS | add `AppMode::Acp` to the host array in `first_sigterm_and_sighup_exit_non_interactive_hosts` (`crates/cyrup/src/signals.rs`) |
| ACP-010 | The `cyrup_terminal_login` AuthMethod identity and its three strings | `auth.ts` `PI_SETUP_METHOD_ID` + the id/name/description block | medium | XS | unit asserting the three strings byte-for-byte and exactly one method — the analogue of `test/unit/auth-methods-terminal-auth-meta.test.ts` |
| ACP-011 | Registry `type`/`args`/`env` → typed `AuthMethod::Terminal` | `auth.ts`'s `type:'terminal'`, `args:['--terminal-login']`, `env:{}` | medium | S | serialization test: `"type":"terminal"`, `"args":["--terminal-login"]`, and the args string equals `acp_terminal_login_cmd::SUBCOMMAND` |
| ACP-012 | Zed's `_meta["terminal-auth"]` compat shape, gated on the client's probe | `auth.ts` `supportsTerminalAuthMeta`; `agent.ts` `clientCapabilities._meta['terminal-auth'] === true` | medium | S | two unit tests over serialized JSON: probe true → `_meta["terminal-auth"].label == "Launch cyrup"`; probe false or absent → no `terminal-auth` key anywhere |
| ACP-013 | The terminal-auth launch spec must name this executable | `auth.ts` `terminalAuthLaunchSpec` | medium | XS | unit: `command` equals `std::env::current_exe()` when it resolves; `args` is exactly `["--terminal-login"]` — one element, not two |
| ACP-014 | `authenticate` is a successful no-op | `agent.ts` `authenticate` | medium | XS | the handler-completeness test: enumerate registered handlers against `AGENT_METHOD_NAMES` and assert every name has one |
| ACP-015 | `maybeAuthRequiredError` rebuilt as a typed classifier | `auth-required.ts` entire | high | M | (a) every typed variant classifies as auth-required; (b) the regression: `maximum context length is 200000 tokens, however you requested 214031 tokens` must **not** classify — it contains `403` inside `214031` |
| ACP-016 | The `AUTH_REQUIRED` payload: data shape and message string | `auth-required.ts`, duplicated at two `agent.ts` sites | medium | S | serialization test pinning `code == -32000`, the message byte-identical, `data.authMethods[0].id == CYRUP_SETUP_METHOD_ID` |
| ACP-017 | Zero available models is treated as unauthenticated | `agent.ts` `newSession`'s `rawModelsCount === 0` ladder | medium | M | `cyrup-it` with no credentials: `session/new` answers `-32000` with the ACP-016 message and a later `session/list` shows no dangling session |
| ACP-018 | The ACP host disables theme discovery | `pi-rpc/process.ts`'s `--no-themes` and its justifying comment | low | XS | assert the constructed `SessionConfig` has `no_themes == true` **and** that extension/prompt discovery is untouched |
| ACP-021 | The ACP arm must not inherit `require_model: true` | no upstream — `index.ts` always builds the transport first | high | S | `cyrup-it` with no credentials: `cyrup --acp` answers `initialize` on stdout and exits 0 on EOF, rather than printing `No models available` to stderr and exiting 1 |
| ACP-022 | A mid-turn provider 401/403 is not an `Err` — classify at the settle boundary | `auth-required.ts` called from `session.ts`'s in-flight-turn site | high | M | unit: a terminal `AssistantMessage` with `StopReason::Error` and `error_message = "http 401: …"` classifies as auth-required at the settle boundary; the same text mid-message does not |
| ACP-023 | `spawn_abort_on_signal` needs a runtime the lazy build does not have yet | `index.ts` signal handlers, which reach a live `agent` | medium | S | `cyrup-it`: SIGTERM a `cyrup --acp` that has completed `initialize` but never received `session/new`; assert `kill_tracked_detached_children` ran and the exit code is the non-interactive one |
| ACP-024 | The stdin read-failure path is the unfiled half of ACP-004 | `index.ts` `stdin.on('error', …)` against the `end`/`close` pair | low | XS | assert the chosen split: clean EOF → 0, read error → non-zero (or all three → 0), pinned so a porter cannot flatten it by accident |
| ACP-025 | `ext_mode` telling extensions the ACP host is `rpc` is a wire-visible decision | no upstream — pi-acp's child genuinely *is* `pi --mode rpc` | low | XS | a `CYRUP-DELTA` at `ext_mode` plus an assertion that the guest ctx string for an ACP session is the chosen value |
| ACP-026 | `--terminal-login` must not bypass the TTY guard | `index.ts`'s `spawnSync(cmd, [], {stdio:'inherit'})`, which re-resolves in the child | low | S | run `cyrup --acp --terminal-login` with both ends piped; assert a diagnostic and a non-TUI exit rather than a TUI painted into a pipe |

**ACP-001 — `--terminal-login` argv gate, classified before clap.**
*Upstream* — before any other work, before the transport exists and before a byte reaches stdout,
scan argv for the literal `--terminal-login`. The test is membership **anywhere**
(`process.argv.includes`), not a positional check, because an ACP client appends `AuthMethod.args`
to the agent command it already has, so the token arrives last. When present the process is not an
ACP agent: it runs the agent interactively on inherited stdio and exits with that status.
*cyrup* — a new `crates/cyrup/src/acp_terminal_login_cmd.rs` beside `subagent_runner_cmd.rs`, with
`SUBCOMMAND` and `is_selected`, classified by a new `Internal::AcpTerminalLogin` arm in
`crate::predispatch::{Internal, classify_internal}` and dispatched from `main`. **The predicate
differs from its three siblings**: they are `argv.get(1) == Some(SUBCOMMAND)`, this one is
`argv.iter().skip(1).any(…)`. In-process the "run pi" step is not a spawn — see `ACP-026` for why it
must not simply force `AppMode::Interactive`.
*Verify* — as tabled. **Open question `ACP-Q2`**: upstream runs `pi` with no arguments on the
assumption the user types `/login` there; cyrup could land directly in `cyrup_config::login::resolve_login_command`.
That is a product decision, not a port fact.

**ACP-002 — `AppMode::Acp` and the `--acp` / `--mode acp` surface.**
*Upstream* — pi-acp encodes "this process serves ACP" in the identity of the binary, so there is no
flag to port. cyrup-acp is the same binary as every other mode.
*cyrup* — add `Acp` to `cyrup_config::AppMode` (`crates/cyrup-config/src/trust.rs`, four variants
today) and to `crate::cli::enums::Mode`; add an `--acp` bool following the `--rpc` precedent; make
`if cli.acp || cli.mode == Some(Mode::Acp) { AppMode::Acp }` the **first** branch of
`crate::cli::runtime_mode::resolve_app_mode`, because an ACP agent is launched with pipes on both
ends and would otherwise reach `!stdin_tty || !stdout_tty → Print` and silently become a one-shot
printer that eats the client's first JSON-RPC frame as a chat prompt. The new variant breaks exactly
two exhaustive matches — `ext_mode` (`crates/cyrup-session-svc/src/builder.rs`, see `ACP-025`) and
`main`'s terminal `match mode`. `should_take_over_stdout` needs no change. Also decide `config.persist`
(`crates/cyrup/src/cli/config_map.rs` **and** `crates/cyrup/src/prelaunch.rs`, duplicated verbatim) —
that is `ACP-213`, and adding `Acp` to only one of the two is a live foot-gun. **The right trust
behaviour is a split concept, not a flag flip**: keep `is_interactive` meaning "TUI host" and add
`AppMode::can_prompt()` (`Interactive | Acp`) for `decide_trust`'s step 5, because
`is_interactive` is also read by `should_take_over_stdout`, `config.persist` and the `!= Interactive`
guards in `bootstrap.rs` / `startup_ui.rs` / `prelaunch.rs`, all of which would flip to the TTY branch.
Add `set_process_name("cyrup-acp")` beside the existing `cyrup-rpc` line in the same arm.
*Verify* — as tabled, plus the existing `decide_trust` tests confirming the other four modes are
byte-identical.

**ACP-003 — Stdio transport bootstrap and `run_acp_dispatch`.**
*Upstream* — bind the JSON-RPC agent to process stdin/stdout with newline-delimited framing and hold
the connection open for the life of the process. Upstream constructs the byte streams by hand
because Node gives it no stdio transport.
*cyrup* — `agent_client_protocol::Stdio::new()` handed to `Agent.builder()…​.connect_to(transport).await`,
wrapped in a `run_acp_dispatch` in `crates/cyrup/src/run.rs` modelled on `run_rpc_dispatch`: run the
protocol, `runtime.dispose().await` unconditionally, then propagate. **One structural divergence
from its sibling**: `run_rpc` opens with `runtime.session().await.bind_extensions().await` (SEAM-033
— the host announces after `--name`/`--models`), whereas ACP must announce after `initialize`
settles, because `has_ui` and the client's advertised capabilities are what a `session_start`
handler should see. With one-live-session semantics the runtime is therefore built **lazily on
`session/new`**, not in `main` before the mode match where `session_launch::launch` builds it today
— which is the constraint `ACP-023` exists to reconcile. `CYRUP-DELTA` to record at the site: the
`blocking` pool thread parked in `read(2)` on stdin is not cancellable; clean EOF exits fine
(verified in the probe transcript), but a teardown while stdin is still open must simply return from
`main` rather than await the reader.
*Verify* — as tabled.

**ACP-004 — A failed stdout write is a clean exit, not an error.**
*Upstream* — three guards with one intent: an editor that closes the connection mid-write produces a
clean exit, never a diagnostic and never a non-zero status. (1) already-destroyed stdout resolves
immediately; (2) the write callback's `err` is explicitly discarded; (3) a synchronous
`ERR_STREAM_DESTROYED` throw is caught; and separately, any `error` event on stdout exits **0**.
*cyrup* — the guard is genuinely needed: `Stdio::connect_to` is `blocking::Unblock` over
`std::io::stdout()` feeding a sink around `write_line`, `transport_outgoing_lines_actor` surfaces the
`io::Error` verbatim, and `rg 'BrokenPipe'` over the ACP crate returns nothing — so a broken pipe
propagates out of `connect_to`. Wrap the write half so `ErrorKind::{BrokenPipe, NotConnected}`
terminates `run_acp_dispatch` with `Ok(())`. **`CYRUP-DELTA` to record**: cyrup's RPC sibling does the
opposite on purpose — `write_pump` propagates any write error to a non-zero exit, which is right for
RPC (a severed protocol stream is a real failure) and wrong for ACP, where the client closing the
pipe **is** the normal termination. Severity is `medium`, not `high`: `run_acp_dispatch` disposes
before propagating, so nothing is lost or corrupted; it is exit-code and diagnostic fidelity against
a supervising editor.
*Verify* — as tabled.

**ACP-005 — Stdin EOF and close terminate the process, and dispose first.**
*Upstream* — both `end` and `close` terminate, in a fixed order: a best-effort
`agent.agent.dispose()` inside a swallowing try/catch, then `process.exit(0)`. `shutdown` is
registered three times over and is idempotent only because `process.exit` does not return.
*cyrup* — `run_acp_dispatch` returns on transport EOF and `runtime.dispose().await` runs on every
exit path, the same contract `run_rpc_dispatch` holds and whose doc already cites pi's
`process.stdin.on("end") → shutdown()`. `AgentSessionRuntime::dispose` emits
`session_shutdown{reason:"quit"}`, fires the session cancel token (killing tracked bash process
groups) and drains the fsync queue.
**Severity corrected from `critical` to `high`, and the correction is load-bearing for the test.**
The survey invoked the data-loss clause; cyrup's own code refutes it in writing.
`flush_session_writes` (`crates/cyrup-session/src/store.rs`) documents: *"This is a power-loss
guarantee only — the bytes are already in the page cache, so no process-exit path can lose them —
which is why it is a courtesy at teardown rather than a correctness requirement"*, and
`AgentSession::dispose_with` repeats it at the call site. Skipping dispose therefore cannot lose the
tail of the transcript. The residual harm is real but is none of the four clauses: extensions never
see `HostEvent::SessionShutdown`, `ExtensionHost::invalidate_live` never runs (with `impl Drop for
AgentSession` as the documented backstop), and `session_cancel` never fires, orphaning tracked
detached bash process groups — a resource leak, covered independently on the signal path by
`kill_tracked_detached_children()`, so **only the stdin-EOF path is exposed**.
*Verify* — the survey's proposed assertion ("the session file contains the final assistant entry")
**passes without the fix**, for exactly the page-cache reason above. The verify line in the table is
the replacement: assert on the emitted `session_shutdown` and on no surviving child process group.

**ACP-010 — The `cyrup_terminal_login` AuthMethod identity and its three strings.**
*Upstream* — exactly one auth method is ever advertised: stable id `pi_terminal_login`, name
`Launch pi in the terminal`, description `Start pi in an interactive terminal to configure API keys
or login`. The id is exported as a named constant because it is what the client echoes back in
`authenticate`.
*cyrup* — a `pub const CYRUP_SETUP_METHOD_ID: &str` plus an `auth_methods(..) -> Vec<AuthMethod>`
builder in `crates/cyrup-acp/src/auth.rs`. All three strings are user-visible and must be rebranded
as a **deliberate, recorded** decision (`cyrup_terminal_login` / `Launch cyrup in the terminal` /
`Start cyrup in an interactive terminal to configure API keys or login`), not transliterated by
accident. Note `schema::v1::AuthMethod` is an **enum**, not a struct.
*Verify* — as tabled.

**ACP-011 — Registry `type`/`args`/`env` → typed `AuthMethod::Terminal`.**
*Upstream* — the method carries the registry-required terminal-auth triple, bolted onto a plain
object and cast, because the TS SDK 0.26 type does not carry them; the file's own comment concedes
the shape is what the registry requires rather than what the SDK models.
*cyrup* — `AuthMethod::Terminal(AuthMethodTerminal::new(..))` is first-class in 2.1.0 with typed
`args`/`env`, so the cast disappears. Keep the `--terminal-login` literal independent from
`ACP-001`'s `SUBCOMMAND` and cross-test the two rather than sharing a constant across crates,
following the precedent documented on `subagent_runner_cmd::SUBCOMMAND`.
*Verify* — as tabled. **Open question `ACP-Q3`**: cyrup has no `--terminal-login` flag and no `login`
subcommand today — only the TUI `/login` slash command and `auth_guidance::get_provider_login_help`
— so `args` is either `[]` (relaunch interactively, user types `/login`) or a new flag must exist
first. An `args` naming a flag that does not exist produces a terminal that exits with a usage error.

**ACP-012 — Zed's `_meta["terminal-auth"]` compat shape, gated on the client's probe.**
*Upstream* — Zed decides whether to render the Authenticate banner from a **non-standard** extension
field, so pi-acp emits both shapes. The `_meta` half is included only when the client advertised
`clientCapabilities._meta['terminal-auth'] === true` — a strict `=== true`, so a truthy non-boolean
does not qualify — and its payload is the launch spec spread flat plus `label: 'Launch pi'`. The
option's default when absent is `true`; the gate at the one real call site overrides that. When
suppressed, `_meta` is absent entirely and the registry half is still returned.
*cyrup* — `Meta` is `serde_json::Map`, and both `AuthMethodTerminal` and `AuthMethodAgent` carry
`#[serde(rename="_meta")] meta: Option<Meta>`, so the compat payload is `.meta(..)`. **`CYRUP-DELTA`:**
2.1.0 gives a *typed* negotiation, `ClientCapabilities.auth: AuthCapabilities { terminal: bool }`,
which is what this hack stood in for. Prefer the typed path and keep the `_meta` emission as a
fallback for older Zed builds, gated exactly as upstream gates it.
*Verify* — as tabled. **Open question `ACP-Q4`**: whether a current Zed sets the typed capability in
addition to (or instead of) the `_meta` probe is not determinable from the pi-acp tree.

**ACP-013 — The terminal-auth launch spec must name this executable.**
*Upstream* — take `argv[0] || 'node'` and `argv[1]`; if both exist **and** `argv[0]` contains `node`
**and** `argv[1]` ends `.js`, return `{command: argv[0], args: [argv[1], '--terminal-login']}`;
otherwise fall back to `{command: 'pi-acp', args: ['--terminal-login']}` and assume PATH.
*cyrup* — `std::env::current_exe()` — total, with an explicit `io::Result`, and already the
workspace's answer to this exact question in `crates/cyrup-intercom/src/transport/spawn.rs`,
`crates/cyrup/src/subcommands.rs` and `crates/cyrup-config/src/paths.rs`. On failure fall back to the
literal `"cyrup"` on PATH, matching intercom's `unwrap_or_else(|_| PathBuf::from("cyrup"))`. The
interpreter-sniffing branch is a cut, and does not survive the translation — which is the point:
upstream's heuristic falls through under its own `npm run dev` (`argv[1]` ends `.ts`) and produces a
spec naming a binary that is not installed in a dev checkout, with no diagnostic.
*Verify* — as tabled.

**ACP-014 — `authenticate` is a successful no-op.**
*Upstream* — the handler ignores its params entirely, including `methodId`, and returns success. The
reason is in the comment: terminal auth happens out of band, so by the time a client calls
`authenticate` there is nothing to do. **It must not error** — Zed calls it after the terminal flow,
and an error reads as a failed login.
*cyrup* — an `on_receive_request` handler for `AuthenticateRequest` returning `AuthenticateResponse::new()`,
answered inline (it is short and non-blocking, unlike `session/new` and `session/prompt`).
**Registering it is not optional**: `agent_client_protocol::Agent` is a role marker with no trait to
implement, so an unregistered handler falls through to `default_handle_dispatch_from`, which returns
`Handled::No { retry: message.has_session_id() }` — a session-scoped method is **retained and
retried**, so a forgotten handler is a hang, not a `method_not_found`.
*Verify* — the handler-completeness test in the table closes this unit and every other
missing-handler hang at once.

**ACP-015 — `maybeAuthRequiredError` rebuilt as a typed classifier.**
*Upstream* — stringify, lowercase, and return non-null if the result **contains** any of eleven
substrings in this order: `api key`, `apikey`, `missing key`, `no key`, `not configured`,
`unauthorized`, `authentication`, `permission denied`, `forbidden`, `401`, `403`. Any hit produces
`RequestError.authRequired({authMethods}, 'Configure an API key or log in with an OAuth provider.')`.
Two of the eleven are bare digit runs matched anywhere in the string, and the rest are common English
words. The three call sites differ in consequence: one also destroys the just-created session, one
converts a `get_state` failure, one rejects the in-flight turn.
*cyrup* — a pure `fn classify_auth(&SessionServiceError) -> Option<AuthRequired>` in
`crates/cyrup-acp/src/auth.rs`, typed first. **The reachable typed inputs are exactly three**, and
all three are pre-flight: `SessionServiceError::{NoConfiguredAuth, AuthPreflightRefused, NoModelSelected}`
(`crates/cyrup-session-svc/src/error.rs`). The mid-turn half belongs to `ACP-022` at a different
call site with a different signature — see that unit for why. Precedent for a string tail at this
seam exists (`cyrup_provider::utils::retry`'s pattern list classifies the same flattened field), but
**that precedent is unanchored** and contains bare `"429"`/`"500"`/`"502"`; it justifies a tail, not
an unanchored one.
*Verify* — as tabled; test (b) is the regression that names the upstream defect. **Open question
`ACP-Q5`**: upstream's `not configured` also fires on MCP-server and extension configuration errors,
which have nothing to do with provider credentials. The typed classifier's default answer is no.

**ACP-016 — The `AUTH_REQUIRED` payload: data shape and message string.**
*Upstream* — every refusal carries `data = { authMethods: getAuthMethods() }` — the full list, so a
client that has not called `initialize` can still render the button from the error alone — and the
message `Configure an API key or log in with an OAuth provider.` with its trailing period. The three
sites construct it independently, and `getAuthMethods()` is called with **no options** there, so
`supportsTerminalAuthMeta` defaults to `true` and the `_meta` half is included **even for a client
that did not advertise the probe** — an asymmetry with the `initialize` path.
*cyrup* — `agent_client_protocol::Error`. **Critical construction detail**: `From<ErrorCode> for Error`
sets `message` to strum's display string (`"Authentication required"`), so `Error::auth_required()`
alone produces the wrong message. Build it explicitly:
`Error::new(-32000, "Configure an API key or log in with an OAuth provider.").data(json!({"authMethods": methods}))`.
Resolve the `_meta` asymmetry deliberately — recommend gating the error payload on the same probe as
`initialize` and recording the divergence, since emitting an ungated `_meta` to a client that
declined it is the strictly worse of the two.
*Verify* — as tabled.

**ACP-017 — Zero available models is treated as unauthenticated.**
*Upstream* — after a session is created, three ordered checks each destroy it before throwing; the
third is an explicit inference recorded in the comment: *"If pi has no models available after
spawning, it's effectively unauthenticated."* The rule is stated on the **raw** count from pi, before
any filtering, and a non-array `models` counts as zero.
*cyrup* — in cyrup this is not an inference, it is a represented state: `cyrup_provider::unconfigured::UnconfiguredProvider`
**is** the empty catalog, installed when no credential names a real provider, and `resolve_model`
returns `model: None` with `format_no_models_available_message()` as the fallback. So the handler
checks `session.model().is_none()` (or the empty catalog) rather than counting a wire array. **But
the rule itself is a live decision, not a mechanical port** — cyrup deliberately supports a modelless
launch (SEAM-075, `crates/cyrup-session-svc/src/tests/modelless_launch.rs`), and pi's own hard stop
is mode-gated with interactive excluded precisely so a credential-less first run gets a UI to
authenticate from. The two candidates are (a) parity — refuse with `auth_required` and roll back —
and (b) cyrup-native — return the session, omit the `model` config option (which `ACP-064`'s builder
already does for an empty list) and deliver `model_fallback_message()` as the first
`agent_message_chunk`. This is `ACP-Q7`, the largest behavioural decision in the area, and it is
unanswerable from pi-acp because pi-acp had no modelless-session concept to weigh against. The
teardown half is `ACP-060`.
*Verify* — as tabled for (a); for (b), assert a `NewSessionResponse` whose `configOptions` contains
no `id == "model"` entry and a first `session/update` carrying the fallback text.

**ACP-021 — The ACP arm must not inherit `require_model: true`.**
*Upstream* — none: the ACP role in `index.ts` never pre-checks credentials, it always builds the
transport first and answers the modelless case **on the wire**, after `session/new`.
*cyrup* — `crates/cyrup/src/main.rs` builds both non-interactive hosts with
`PostBuild { require_model: true }` (the block commented *"The two non-interactive hosts. Both take
pi's modelless hard stop"*), and `session_launch::launch` then executes
`if post.require_model && session.model().is_none() { runtime.dispose().await;
output_guard::restore_stdout(); diagnostics::no_models_available(); return Ok(ControlFlow::Break(1)) }`
— a stderr print and exit 1 that happens **before** `main` reaches `match mode`. Put `AppMode::Acp`
on that leg and a credential-less launch dies before the transport exists: the editor sees a process
that exited 1 having written `No models available. Use /login …` to stderr and **zero JSON-RPC
frames**, and `ACP-010`, `ACP-012`, `ACP-016` and `ACP-017` are all structurally unreachable. ACP must
take Interactive's `require_model: false` (the SEAM-075 arm — modelless launch plus a banner) and
answer the modelless state over the wire per `ACP-017`.
*Verify* — as tabled. This is filed as a unit precisely because every survey cited the hard stop as
*supporting evidence* for `ACP-017` and none filed it as the thing that must be overridden.

**ACP-022 — A mid-turn provider 401/403 is not an `Err` — classify at the settle boundary.**
*Upstream* — `auth-required.ts`'s third call site rejects the **in-flight** ACP turn when the
provider fails mid-stream.
*cyrup* — the natural port is unbuildable. `SessionServiceError` (26 variants) carries no
`ProviderError`, and `cyrup_agent::AgentError` (ten variants —
`RunActive`/`NoMessages`/`ContinueFromAssistant`/`NoModelSelected`/`Cancelled`/`Hook`/`Core`) carries
none either. `ProviderError::into_error_message`'s own doc states the rule: request and stream
failures *are never thrown* — they are flattened into an `AssistantMessage` with `StopReason::Error`
and `error_message = "http 401: …"` (`#[error("http {status}: {message}")]`,
`crates/cyrup-provider/src/error.rs`), which is also why `run::exit_code` reads the terminal
assistant message rather than an error. **So `prompt` SUCCEEDS on a provider 401 and there is no
`Err` at the ACP prompt handler at all.** The mid-turn case must be classified at the **turn-settle
boundary**, by inspecting the terminal `AssistantMessage::error_message` before building
`PromptResponse`, with an **anchored** `^http (401|403):` match — never `contains`, which is what
lets `214031` read as a 403.
*Verify* — as tabled.

**ACP-023 — `spawn_abort_on_signal` needs a runtime the lazy build does not have yet.**
*Upstream* — pi-acp's signal handlers reach `agent.agent.dispose()`, and `agent` exists from the
moment the connection is constructed.
*cyrup* — `spawn_abort_on_signal(runtime: Arc<AgentSessionRuntime>, cancel, host)`
(`crates/cyrup/src/signals.rs`) takes the runtime **by value at spawn time**, and `ACP-006`
prescribes calling it in `main`'s ACP arm exactly as the `Rpc` arm does — but `ACP-003` requires the
runtime to be built lazily on `session/new`, because `initialize` must settle first. The two cannot
both hold. Either the watcher is armed only after the first `session/new`, leaving a startup window
in which SIGTERM triggers neither `kill_tracked_detached_children()` nor `runtime.dispose()` and the
process dies on tokio's default disposition, or the watcher needs a shape that can bind a runtime
later (a `watch`/`OnceLock` handoff, or arming on the first session). Note the watcher's **first
act**, `kill_tracked_detached_children()`, is documented as *"genuinely first: before the repeat
watcher, before the abort, before the dispose"*, so deferring the watcher defers that too.
*Verify* — as tabled.

#### Refuted — 4a

| id | struck because | cyrup symbol |
|---|---|---|
| ~~ACP-007~~ `process.stdin.resume()` keep-alive | in Rust nothing runs after `main` returns; there is no event loop to keep alive. The unit's own mechanism is the words "No code" and its verify delegates to `ACP-003`. One `CYRUP-DELTA` comment, not a unit | `main`'s awaited mode arm (`crates/cyrup/src/main.rs`) plus `Stdio`'s `blocking::Unblock::new(std::io::stdin())` reader |
| ~~ACP-008~~ `set_process_name("cyrup-acp")` | the capability, the `unsafe`-containment rationale, the call site and the SEAM-070 naming convention all exist; the delta is one string literal in the branch `ACP-002` already adds, and the unit itself concedes "no automated assertion is warranted" | `set_process_name` (`crates/cyrup/src/main.rs`), already carrying `cyrup-rpc`/`cyrup-subagent`/`cyrup-broker`/`cyrup-mcp-keyring` |
| ~~ACP-009~~ disconnect disposes every live session | double-filed with `ACP-005`: same mechanism, and its verify reads "Covered by ACP-005's dispose assertion". Under one-live-session the fan-out has no counterpart; the N-session note belongs in `ACP-Q8` | `AgentSessionRuntime::dispose` → `AgentSession::dispose_with` (`crates/cyrup-session-svc/src/session/lifecycle.rs`) |
| ~~ACP-019~~ adapter-owned storage directory | two reasons. The path capability is present and already correct — `ConfigDirs` resolves every directory with the CLI>env>default ladder applied, which is the property the unit says `homedir()` cannot hold. And the directory's only upstream occupant is the sessionId→file map, which is moot in-process: `listing::SessionInfo` carries `{path, id, cwd}` and `listing::resolve(&SessionSelector::Uuid(..), ..)` turns an id or unique prefix into a path. The unit's own conclusion is "Do not build the directory before something needs it" | `cyrup_config::ConfigDirs` (`crates/cyrup-config/src/env.rs`) + `cyrup_session::{listing, layout}` |
| ~~ACP-020~~ session-file parent directory created before first write | cyrup holds the invariant unconditionally at the write path rather than opportunistically at startup, so nothing implements it and nothing can regress it — house rule 3's automated-assertion clause can never be satisfied. Kept as a sentence in `## 3. Cuts`' spirit so the handshake is not re-implemented | `DiskStore`'s two write paths both call `std::fs::create_dir_all(parent)` (`crates/cyrup-session/src/store.rs`) |

### 4b · The request surface — initialize, session/new, modes, models, startup info

Upstream: `src/acp/agent.ts` (the startup half of `class PiAcpAgent`), with `auth.ts`,
`auth-required.ts` and `pi-settings.ts` as its helpers.

In-process collapses this area more than any other, because every fact `agent.ts` probes off NDJSON
is a typed read: `getState`/`getAvailableModels` stop being parallelisable RPC calls with `catch →
null` degradations, the `pre?` caching parameters threaded through three functions vanish, and the
six `try { … } catch { return null }` wrappers go with them — **and with them goes the class of bug
they enabled**, since `getModelState`'s `'default'` sentinel and `getThinkingState`'s silent fall
back to `'medium'` were both reachable only through a swallowed probe failure.

**Three of this area's originally-proposed golden-JSON assertions are unsatisfiable against schema
1.7.0 and are corrected in the tables below.** `AgentCapabilities.auth: AgentAuthCapabilities` is a
required non-`Option` field that always serializes an `"auth":{}` key pi-acp never emits, so
`ACP-052` cannot compare byte-for-byte; and `SessionMode` and `SessionConfigSelectOption` are
`#[skip_serializing_none]`, so `None` **omits** the key and there is no way to emit the explicit
`description: null` that `ACP-062` and `ACP-064` were written against. Every golden criterion here is
a **subset assertion over the keys the port controls**.

| id | title | upstream | sev | eff | verify |
|---|---|---|---|---|---|
| ACP-050 | `initialize` clamps the requested protocol version | `agent.ts` `initialize` | medium | XS | unit over the clamp: `[0,1,2,65535]` all map to `V1`; integration: `"protocolVersion": 2` answers `1` and does not error |
| ACP-051 | `agentInfo` name / title / version | `agent.ts` `initialize`'s `agentInfo` + `readNearestPackageJson` | low | XS | integration: `agentInfo.name == "cyrup"`, `agentInfo.version == env!("CARGO_PKG_VERSION")` |
| ACP-052 | The four advertised capability blocks | `agent.ts` `initialize`'s `agentCapabilities` | medium | S | **subset** assertion: `loadSession==true`, `promptCapabilities.image==true`, `mcpCapabilities=={http:false,sse:false}`, and `sessionCapabilities.list`/`.delete` both present as `{}` — dropping the `Some(..)` fails |
| ACP-053 | `promptCapabilities.embeddedContext` behind an env opt-in | `agent.ts` `initialize`; `test/unit/pi-enable-embed-context-flag.test.ts` | low | XS | unit on the **pure predicate** taking the value as an argument: only `Some("true")` is true |
| ACP-054 | `authMethods` and the conditional `_meta` shim, from `initialize` | `auth.ts` `getAuthMethods` at the `initialize` call site | medium | S | two assertions on serialized JSON: typed `auth.terminal` true → `"type":"terminal"` with no `_meta`; false plus the `_meta` probe → the legacy key |
| ACP-055 | `authenticate` answers success | `agent.ts` `authenticate` | low | XS | send `authenticate` with an arbitrary `methodId`; assert a `result`, not an `error`, and the connection stays live |
| ACP-056 | `session/new` rejects a non-absolute `cwd` | `agent.ts` `newSession`'s first statement | high | XS | `cwd: "relative/path"` → `-32602` with message exactly `cwd must be an absolute path: relative/path`, and no session file anywhere |
| ACP-057 | Build the session **off** the dispatch loop, and never propagate `Err` | `agent.ts` `newSession` + `session.ts` `SessionManager.create` | critical | M | issue `session/new`, then `session/list` while it is in flight; assert the `list` response arrives first. Second: force a build failure, assert an error response **and** that the connection answers a later request |
| ACP-058 | The auth-required / internal-error paths of `session/new` | `agent.ts` `newSession`'s four branches; `test/unit/new-session-runtime-startup-errors.test.ts` | medium | S | `NoConfiguredAuth` → `-32000` with non-empty `data.authMethods`; a non-auth build failure → `-32603` |
| ACP-059 | Zero available models means unauthenticated | `agent.ts` `newSession`; `test/unit/new-session-auth-required-when-no-models.test.ts` | high | S | with an empty catalog, assert the behaviour chosen in `ACP-Q7`, exactly |
| ACP-060 | The destructive rollback on a normal error path | `agent.ts` `cleanupFailedNewSession` | high | S | drive the chosen zero-models path; assert the file `session/new` created is gone and no other file under the sessions root was touched — or, if creation is deferred past the auth gate, that no file was ever created |
| ACP-061 | One live session per connection | `agent.ts` `newSession`'s `closeAllExcept`; `session.ts` `SessionManager.close` | high | M | `session/new` twice; assert a different `sessionId`, that a `session/prompt` against the first id errors rather than silently routing to the second, and that the generation watch fired exactly once |
| ACP-062 | The ACP mode list **is** the thinking-level ladder | `agent.ts` `getThinkingState` + `isThinkingLevel` | medium | S | a reasoning model yields 7 modes ending in `max` with names `Thinking: off`…`Thinking: max`; a non-reasoning model yields exactly `[off]`; `currentModeId == thinking_level_to_str(session.thinking_level().await)` |
| ACP-063 | The advertised model list and current selection | `agent.ts` `getModelState` | medium | S | with a two-model catalog and a current model, assert `currentValue` is a **member** of `options` — membership, not equality; with an empty catalog, no `id == "model"` option is emitted at all |
| ACP-064 | The two config options and their order | `agent.ts` `buildConfigOptions`; `test/unit/session-config-options.test.ts` | medium | S | **subset** golden: ids `model` then `thought_level`, categories, names, descriptions and `currentValue`; the `description: null` half of the upstream fixture is unrepresentable and is dropped |
| ACP-065 | `NewSessionResponse` has no `models` field | `agent.ts` `newSession`'s return | medium | XS | golden: top-level keys are exactly `sessionId`, `modes`, `configOptions`, `_meta` — reintroducing `models` fails |
| ACP-066 | The markdown startup prelude | `agent.ts` `buildStartupInfo`; `test/unit/startup-info-project-packages.test.ts` | medium | M | unit over the **pure renderer** with a fixture `StartupReport`: the exact `## Context` / `- item` structure, an empty section emits nothing, a report with only diagnostics still renders them, and `quiet_startup` renders diagnostics without the inventory |
| ACP-068 | The prelude is delivered **after** the `session/new` response | `agent.ts` `newSession`'s `setTimeout(…,0)`; `session.ts` `sendStartupInfoIfPending` | medium | S | assert on **raw NDJSON frame order**: the response line precedes the `session/update` line |
| ACP-069 | `available_commands_update` is also deferred past the response | `agent.ts` `newSession`'s second `setTimeout` | medium | S | frame order: the notification follows the response, and its array contains at least one catalog-sourced entry plus the builtins |
| ACP-070 | The eight headless built-ins | `agent.ts` `builtinAvailableCommands`; `test/unit/builtin-commands.test.ts` | medium | S | the list serializes to the fixture (names, descriptions, hints, order) **and** every name resolves to a dispatchable command — the second half is what fails today |
| ACP-071 | `mergeCommands` — first-wins, order preserved | `agent.ts` `mergeCommands`; `test/unit/merge-commands.test.ts` | low | XS | a user command named `compact` shadows the builtin; total length is `1 + builtins.len() - 1` |
| ACP-072 | `session/set_mode` sets the thinking level and echoes the **applied** one | `agent.ts` `setSessionMode` | high | S | against a model whose supported levels exclude `xhigh`: `{modeId:"xhigh"}` must not emit a `current_mode_update` claiming `xhigh` — it errors, or emits the clamped level. A test asserting only `{}` came back passes the broken version |
| ACP-073 | `session/set_config_option` routes `model` and `thought_level` | `agent.ts` `setSessionConfigOption`; `test/unit/session-config-options.test.ts` | medium | S | thought-level path emits **both** notifications in order (`current_mode_update` then `config_option_update`) and the returned `configOptions` carries the new `currentValue`; `configId: "nope"` → `-32602` with `Unknown config option: nope` |
| ACP-075 | `emitConfigOptionsUpdate` re-derives the whole option set | `agent.ts` `emitConfigOptionsUpdate` + `getSessionConfiguration` | medium | S | after a set the session clamps or redirects, the pushed `currentValue` equals the session's state read back independently — not the requested value |
| ACP-077 | Push config/mode updates on **session-originated** changes | no upstream — a latent defect upstream | medium | S | change the model through a non-ACP route (`cycle_model`, or an extension command) and assert a `config_option_update` reaches the client. The pi-acp-faithful implementation fails this test |
| ACP-078 | The `Unknown sessionId` gate every setter opens with | `agent.ts` `restoreSession`'s `invalidParams` arm, entered from three setters | medium | S | a stale id on `session/set_mode` / `session/set_config_option` returns `-32602` with `Unknown sessionId: <id>` byte-for-byte, and does **not** mutate the live session |
| ACP-079 | The setters do real blocking work and must leave the dispatch loop | `agent.ts` `setSessionMode` / `setSessionConfigOption`, which await inline | medium | S | issue `session/set_mode` and a `session/cancel` back to back; assert the cancel is observed before the set response |
| ACP-080 | An undescribed command's description is defined upstream | `pi-commands.ts` `describeFallback` | low | XS | a catalog row with no `description` key projects to `(source:location)`-shaped text, never `""` |
| ACP-081 | `buildStartupInfo` can never return an empty string | `agent.ts` `buildStartupInfo`'s terminal `join().trim() + '\n'` | low | XS | pin the decision: a project with nothing to report either emits the degenerate newline chunk (parity) or suppresses an all-whitespace prelude (better) |
| ACP-082 | `lastSessionCwd` is connection-scoped state `session/new` writes | `agent.ts` `newSession` / `loadSession` / `restoreSession`, read by `listSessions` | low | XS | covered by `ACP-207`'s default-filter test; this unit owns the **write** |

**ACP-050 — `initialize` clamps the requested protocol version.**
*Upstream* — `protocolVersion: requested === supportedVersion ? requested : supportedVersion`. Every
non-1 request is answered with 1; there is no error path. A client asking for a version the agent
does not serve gets a graceful downgrade and is expected to disconnect itself if it cannot live with
1.
*cyrup* — `schema::ProtocolVersion` is a `u16` newtype, **not an enum**, so the clamp is a total
function over it. Respond with `InitializeResponse::new(clamped)`. **Do not enable
`unstable_protocol_v2`**: with it on, `ProtocolCompat::incoming_initialize_request` hard-errors
`unsupported_protocol_version` on a mismatch, which is the opposite of this behaviour.
**`CYRUP-DELTA`:** `protocol_version` carries no `DefaultOnError`, so a value outside `u16` or of the
wrong JSON type fails deserialization and the request is rejected where pi-acp clamped to 1. That
divergence is imposed by the SDK and cannot be closed from the handler. The ternary reads as dead
code and simplifies to `protocolVersion: requested`; a maintainer making that simplification
introduces a silent protocol lie, so the clamp belongs in a named total function with its own test,
not inline in an object literal.
*Verify* — as tabled.

**ACP-052 — The four advertised capability blocks.**
*Upstream* — exactly `loadSession: true`; `mcpCapabilities: {http:false, sse:false}` (pi has no MCP
over ACP, and `params.mcpServers` is accepted and stored on `session/new` without ever being used);
`promptCapabilities: {image:true, audio:false, embeddedContext: <ACP-053>}`; and
`sessionCapabilities: {list:{}, delete:{}}` — both present as empty objects, which is the ACP
spelling of "supported". The `UNSTABLE` comment on `list`/`delete` is stale as of schema 1.7.0.
*cyrup* — the builder chain in `schema::v1`. `McpCapabilities::new()` already defaults both to
`false`. `SessionCapabilities.list`/`.delete` are `Option<…>` where `Some(T::new())` serializes to
`{}` and `None` is omitted — **that `Option` is the whole advertisement**, so passing `None` silently
un-advertises the picker. Two `CYRUP-DELTA` candidates the Rust schema offers and pi-acp cannot —
`SessionCapabilities.additional_directories` and `AgentAuthCapabilities.logout` — **must not** be
advertised without a working implementation. `promptCapabilities.image: true` is a *static* claim;
cyrup knows per-model vision (`cyrup_provider::Modality::Image`, `Model::supports_image_input`,
`cyrup_tools::config::ModelVisionHandle`), but `initialize` runs before any session exists, so the
static `true` is correct and the per-model truth belongs to the prompt path.
*Verify* — as tabled. The byte-for-byte fixture the survey proposed **cannot pass**:
`AgentCapabilities.auth` is a required field that always emits `"auth":{}`. **Open question `ACP-Q9`**:
`loadSession: true` and the two session capabilities are promises about surfaces other units own; if
`session/load`, `session/list` or `session/delete` ship later, the capability must be gated behind
the implementation rather than advertised aspirationally.

**ACP-054 — `authMethods` and the conditional `_meta` shim, from `initialize`.**
*Upstream* — the `initialize` call site is the one place `supportsTerminalAuthMeta` is computed
rather than defaulted, from `clientCapabilities._meta['terminal-auth'] === true`.
*cyrup* — same shapes as `ACP-010`/`ACP-011`/`ACP-012`; this unit is the `initialize` wiring and the
negotiation. Prefer the typed `req.client_capabilities.auth.terminal`, whose own doc says the client
sets it only when it can reproduce the configured agent invocation; fall back to the legacy `_meta`
emission only when the typed flag is false **and** `client_capabilities.meta` carries the
`terminal-auth` key, i.e. an older Zed.
*Verify* — as tabled.

**ACP-056 — `session/new` rejects a non-absolute `cwd`.**
*Upstream* — the first statement of the method:
`` throw RequestError.invalidParams(`cwd must be an absolute path: ${params.cwd}`) ``. Code −32602,
the offending value interpolated verbatim. It runs before any session is created, so there is
nothing to clean up.
*cyrup* — `NewSessionRequest.cwd` is a `PathBuf`, so the check is `Path::is_absolute`. Build the
`Error` by hand — `From<ErrorCode> for Error` stamps `"Invalid params"` and loses the specific text.
**Do not canonicalize instead of rejecting**: that accepts the input and changes the session root
under the client. `CYRUP-DELTA`: `cwd.display()` differs from JS interpolation for non-UTF-8 bytes.
The check must fire **before** `switch_session_with`, whose own `MissingSessionCwd` is an existence
test, not an absoluteness test, and produces a different error.
*Verify* — as tabled.

**ACP-057 — Build the session off the dispatch loop, and never propagate `Err`.**
*Upstream* — `sessions.create(...)` spawns the child, probes `getState()` for `sessionId`/`sessionFile`,
falls back to `crypto.randomUUID()`, and upserts the sidecar only when `state.sessionFile` is a
string.
*cyrup* — build through `cyrup::session_launch::build_factory` + `SessionFactory::build`, then
install into the `AgentSessionRuntime`. **Two hard constraints.** (1) The handler must not await the
build: `ConnectionTo`'s own doc says handler callbacks run on the event loop and the connection
cannot process new messages while a handler runs, and a build runs discovery, extension loading and
possibly an interactive trust prompt. So `cx.spawn(...)` immediately and move the
`Responder<NewSessionResponse>` into the task — it is `Send + 'static`. (2) Inside that task **never
propagate `Err` with `?`**: `ConnectionTo::spawn`'s doc is *"If the spawned task returns an error,
the entire server will shut down."* A build failure must become `responder.respond_with_error(..)`
with the task still returning `Ok(())`.
**Why `critical`** — crash on a normal path: an ordinary failed `session/new` (bad cwd permissions, an
extension that fails to load, trust refusal) kills the editor's agent connection rather than
returning an error. The severity is about the connection teardown, not about the build.
*cyrup, continued* — the session id is `AgentSession::session_id()` and the file
`AgentSession::session_file().await`. `CYRUP-DELTA` at the site: `SessionFactory::build(target, Some(other_cwd))`'s
cwd rebind does **not** reach the native extensions (§1), so a second `session/new` with a different
cwd requires a new factory via `build_factory`.
*Verify* — as tabled. **Open question `ACP-Q10`**: `NewSessionRequest.mcp_servers` is accepted and
ignored by pi-acp; cyrup has a real MCP tier, so ignoring it is a live choice rather than a forced
one.

**ACP-058 — The auth-required / internal-error paths of `session/new`.**
*Upstream* — `getState()` and `getAvailableModels()` run under `Promise.all` with per-promise
`.catch` capturing rather than rethrowing, then four ordered branches, each of which calls
`cleanupFailedNewSession` first. Branch 4 discards the specific error and rebuilds a fresh one, and a
`stateErr` that is *not* auth-shaped is swallowed entirely — the session proceeds with `state = null`.
*cyrup* — with no child there is no `getState`/`getAvailableModels` RPC that can fail, so branches 1,
2 and 4 collapse into "the build returned an error" and branch 3 becomes `ACP-059`. The classifier is
`ACP-015`; build the errors by hand per `ACP-016`. **Use `cyrup_session_svc::auth_guidance`** — the
whole family of provider-auth guidance strings, ported 1:1 from pi's `auth-guidance.ts` — rather than
pi-acp's single flat sentence where a more specific remedy is known. `CYRUP-DELTA`: keep pi-acp's
`{authMethods}` key and shape so a Zed that reads it still renders the banner.
*Verify* — as tabled. **Open question `ACP-Q11`**: byte-parity (`Configure an API key…`) and
useful-to-the-user (`format_no_api_key_found_message(provider)`, which names the provider and points
at `/login`) diverge here.

**ACP-059 — Zero available models means unauthenticated.**
*Upstream* — the rule is stated on the **raw** count from pi, before any filtering or `provider/id`
formatting, and a non-array `models` is treated as zero. The pinning test asserts code −32000, the
message, and that `sessions.close('s1')` was called.
*cyrup* — `AgentSession::available_model_catalog()` (`crates/cyrup-session-svc/src/session/model.rs`)
is the exact analogue of pi's `modelRegistry.getAvailable()` = `getAll().filter(hasConfiguredAuth)`,
and is already what the RPC `get_available_models` verb serves; `.is_empty()` is the predicate. See
`ACP-017` and `ACP-Q7` for the decision; this unit owns the mechanism and the wire shape.
*Verify* — as tabled.

**ACP-060 — The destructive rollback on a normal error path.**
*Upstream* — close the session; resolve the file as `state?.sessionFile` when a non-blank string else
`store.get(sessionId)?.sessionFile`; if that yields a path and `existsSync`, `unlinkSync` it inside a
try/catch that ignores failures; delete the store entry. **This permanently deletes a `.jsonl` on an
ordinary, expected error** — an unauthenticated first run. And `SessionManager.create` only upserts
the store entry when `state.sessionFile` was a string, so when `getState()` threw both lookups miss
and the freshly created file is **orphaned rather than deleted**, silently, forever.
*cyrup* — only needed if `ACP-Q7` lands on parity. Then `cyrup_session_svc::delete_session_file_at(path)`
is the seam and the path is `AgentSession::session_file().await` — a typed `Option<PathBuf>` read
from the live session, so upstream's two-source guessing and its orphan hole do not exist.
`CYRUP-DELTA`: `delete_session_file_at` moves to trash where pi-acp does an unrecoverable
`unlinkSync`; trash is safer and should be kept — **but** for an internal stub cleanup, putting
adapter garbage in the user's trash is arguably wrong and a direct `remove_file` may be right here
even though `ACP-218` uses the trash path. **Better still, avoid the rollback entirely**:
`SessionConfig.persist` already selects an ephemeral in-memory session, so a build that runs the auth
gate before committing to disk has nothing to delete. Never swallow the delete failure silently; log
at `tracing::warn!` with the path.
*cyrup, hazard* — the same hazard exists in-process for a different reason: `SessionBuilder::build`
appends `model_change` and `thinking_level_change` entries near the end of the build, which is the
first `DiskStore::append_line` and therefore the moment the file materialises. A `session/new` that
builds successfully and is then rejected by ACP-level validation leaves a header-plus-two-entries
stub on disk.
*Verify* — as tabled.

**ACP-061 — One live session per connection.**
*Upstream* — `closeAllExcept(session.sessionId)`, guarded with `?.` because tests stub the manager.
The comment is explicit that this is leak avoidance, not a protocol requirement — an evicted session
is still restorable from disk via `restoreSession`. The identical call is also made from `loadSession`.
*cyrup* — structural rather than policy: `AgentSessionRuntime` is a single-slot replacer, so
installing a new session **is** the eviction. The ACP driver must react the way `run_rpc`'s loop
does: read `AgentSessionRuntime::watch_generation()` both as a dedicated `select!` arm **and** as a
`has_changed()` check before servicing any request, and on a bump run the `rebind_session` sequence —
re-acquire `runtime.session().await`, re-`subscribe()`, re-install `set_ui_sink` / `set_ui_effect_sink`
/ `ext_host.add_error_listener`, clear in-flight state. The prior stream is terminated with
`AgentSessionEvent::SessionReplaced`, which the pump must recognise (`ACP-154`). **Do not infer
replacement from the request name**: the `Dispatched` doc in `rpc/mod.rs` records (SEAM-022) that an
extension's `ctx.newSession()` arrives as an ordinary prompt, which is exactly why the generation
watch exists. The Architecture phase recommends **lifting** `rebind_session` + `LoopSinks` into
`cyrup-modes` rather than letting `cyrup-acp` become its third copy — the second is
`crates/cyrup-tui/src/app/extension_ui.rs`.
*Verify* — as tabled. See `ACP-225` for the tension between this unit's forced rebuild and
`ACP-209`'s live-session short-circuit.

**ACP-062 — The ACP mode list is the thinking-level ladder.**
*Upstream* — `availableModes` is the **fixed** six-element array `off|minimal|low|medium|high|xhigh`,
each rendered `{id, name: "Thinking: ${id}", description: null}`; the list never varies with the
model; `currentModeId` falls back to `'medium'`.
*cyrup* — `AgentSession::{available_thinking_levels, thinking_level}`
(`crates/cyrup-session-svc/src/session/thinking.rs`), serialized through
`cyrup_session_svc::builder::thinking_level_to_str`. **Three divergences that are behaviour, not
mechanism, each needing a `CYRUP-DELTA`.** (i) cyrup's ladder has a seventh rung, `max`
(`ModelThinkingLevel::Max`, `crates/cyrup-core/src/message/thinking.rs`), which the six-value union
cannot express. (ii) The list is **model-dependent**: `available_thinking_levels` returns
`get_supported_thinking_levels(&model)` and the full 7-rung set only when no model is resolved, so a
non-reasoning model yields `[off]` alone. (iii) The default is not a hardcoded `medium` — the
session's actual level is authoritative and is read, not guessed. (iv) `SessionMode.description` is
`Option<String>` under `#[skip_serializing_none]`, so `None` **omits** the key where pi-acp emits an
explicit `null`; there is no way to emit the null, so this is a forced divergence, not a choice.
*Verify* — as tabled. **Open question `ACP-Q12`**: whether a one-entry mode list is acceptable to
Zed, or whether the whole `modes`/`thought_level` surface should be omitted when `supports_thinking()`
is false. pi-acp never faced this because its list was constant.

**ACP-063 — The advertised model list and current selection.**
*Upstream* — each entry becomes `{modelId: "provider/id", name: "provider/name", description: null}`
with both halves `String(...).trim()`-ed and an entry with an empty provider **or** id dropped; the
current model comes from `state.model` when it is an object with non-empty trimmed fields; the whole
thing returns `null` when the list is empty **and** there is no current model; otherwise an unknown
current model falls back to `availableModels[0]?.modelId ?? 'default'`.
*cyrup* — `available_model_catalog()` supplies `Vec<Model>` with typed `provider: ProviderId`,
`id: ModelId`, `name: String`, so the trim/empty/drop ladder is unreachable and is cut; the current
selection is `AgentSession::model() -> Option<ModelRef>`, a real `Option`, so **the `'default'`
sentinel must not be ported** — an absent selection is `None` and the whole option is omitted.
**Severity lowered from `high` to `medium`, and the reason matters for the test**: the sentinel is
unreachable even upstream. `getModelState` runs `if (!availableModels.length && !currentModelId)
return null` **before** the `?? 'default'` fallback, so past that guard a falsy `currentModelId`
implies a non-empty list, `availableModels[0]` exists, and its `modelId` is a template literal that
cannot be empty. Both `?? 'default'` sites are dead code. Keep the membership assertion — it is a
good test — but not on that justification. `CYRUP-DELTA` available: `SessionConfigSelectOptions::Grouped`
exists in 1.7.0, so models can be grouped by provider instead of relying on the `provider/` string
prefix; keep `Ungrouped` for parity unless the delta is recorded.
*Verify* — as tabled.

**ACP-064 — The two config options and their order.**
*Upstream* — always exactly one thinking option; the model option only when the model list is
non-empty, added with `unshift` so the order is `[model, thought_level]`. The pinning test uses
`deepEqual`, so every key, string and array position is load-bearing. Constants are
`MODEL_CONFIG_ID = 'model'` and `THOUGHT_LEVEL_CONFIG_ID = 'thought_level'`.
*cyrup* — `SessionConfigOption::select(id, name, current_value, options)` with `.description(..)` and
`.category(SessionConfigOptionCategory::{Model, ThoughtLevel})` — both categories exist ungated and
spell exactly `model` / `thought_level` under `rename_all="snake_case"`. `SessionConfigKind::Select`
is `#[serde(flatten)]`ed under `type`, so the bytes land at the same nesting depth. Keep the two ids
as `const`s so the setter and the builder cannot drift. **Order matters and is not enforced by the
type.** The `description: null` half of the upstream fixture is unrepresentable
(`#[skip_serializing_none]`), so the golden is a subset assertion.
*Verify* — as tabled. **Open question `ACP-Q13`**: cyrup has more selectable session state than pi —
`scoped_models`, auto-compaction, steering/follow-up modes — and 1.7.0 has `SessionConfigKind::Boolean`
for exactly the toggle shape. That is a superset enhancement, filed separately, not a port unit.

**ACP-065 — `NewSessionResponse` has no `models` field.**
*Upstream* — returns `{sessionId, configOptions, models, modes, _meta}`; TypeScript structural typing
lets the extra `models` key ride along on a response type that does not declare it.
*cyrup* — `schema::v1::NewSessionResponse` has exactly four fields — `session_id`, `modes`,
`config_options`, `meta` — and is `#[non_exhaustive]`, so `models` **cannot** be a top-level key.
Either drop it (the `model` config option already carries the same information in the spec-sanctioned
place) or move it under `_meta`. **The identical constraint holds for `LoadSessionResponse`**, which
has no `models` field either; decide once and apply to both. A `CYRUP-DELTA` is mandatory either way,
because a Zed build that reads `response.models` gets `undefined`.
*Verify* — as tabled. **Open question `ACP-Q14`**: whether any shipping Zed reads it. If none does,
dropping it is strictly better than an `_meta` shim nobody consumes.

**ACP-066 — The markdown startup prelude.**
*Upstream* — a markdown string joined with `\n`, trimmed and terminated with one `\n`: a `pi v<version>`
header line, `---`, then four sections emitted only if non-empty — **Context** (`cwd/AGENTS.md` as an
absolute path), **Skills** (every direct `*.md` plus every `SKILL.md` found by a recursive walk under
three roots, unsorted, undeduplicated), **Prompts** (`/<basename>` from the prompts dir), and
**Extensions** (`*.ts`/`*.js` from the extensions dir plus every entry of both `settings.json`
`packages` arrays, where an `npm:` entry renders as two lines). Every filesystem call is individually
try/caught. Themes are deliberately excluded.
*cyrup* — **the sourcing is already assembled and typed; only the rendering and the delivery are
new.** `cyrup::interactive::build_startup_report` builds a `cyrup_tui::StartupReport` whose
`context_files` (from `services.context.snapshot()`), `skills` (`services.resources.skills.all()`, by
name), `prompts`, `extensions` (`services.ext_host.loaded_ids()`) and `themes` are exactly pi's own
blocks — sourced from live registries, de-duplicated, conflict-resolved. `cyrup_tui::build_startup_lines`
renders them for the TUI; cyrup-acp needs a **markdown renderer over the same `StartupReport`**, not
a re-scan. `CYRUP-DELTA`s: items become names not absolute paths (which is what pi's own
`showLoadedResources` does, so this moves toward pi); themes are available and should be decided on
rather than silently dropped; and `StartupReport` carries four diagnostic blocks — `[Skill conflicts]`,
`[Prompt conflicts]`, `[Extension issues]`, `[Theme conflicts]` — which pi shows **even under
`quietStartup`** and which an ACP client has no way to see today. The `quietStartup` gate itself is
already implemented, not merely available: `StartupReport::show_listing()` is `verbose || !quiet_startup`
and `has_diagnostics()` is the keep-the-diagnostics-under-quiet half, pinned by
`quiet_startup_suppresses_the_listing_but_never_the_diagnostics` (`crates/cyrup-tui/src/startup.rs`),
so this unit consumes one `if report.show_listing()` rather than porting a branch.
*Verify* — as tabled. **Open question `ACP-Q15`**: `build_startup_report` lives in the `cyrup` **bin**
crate and takes `&AgentSession`; it must move (or be duplicated) to be reachable, and the natural home
beside `StartupReport` would make `cyrup-acp` depend on `cyrup-tui`.

**ACP-068 — The prelude is delivered after the `session/new` response.**
*Upstream* — `setTimeout(() => session.sendStartupInfoIfPending(), 0)`. The timer exists because the
notification must not reach the client before the response; the comment at the sibling deferral says
so. `sendStartupInfoIfPending` sets its sent flag **first** so a re-entrant call cannot double-send.
The same text is also carried in the response's `_meta.piAcp.startupInfo`.
*cyrup* — **the timer is cut, the ordering is not.** `Responder::respond` is a synchronous
`send_fn: Box<dyn FnOnce(..) + Send>` that serializes and enqueues on the connection's outgoing
channel, and `ConnectionTo::send_notification` enqueues on the same channel, so `responder.respond(resp)`
then `cx.send_notification(..)` **in the same spawned task** gives deterministic ordering with no
timer and no race. TypeScript could not express this because the response is emitted by returning.
Record the deletion of the timer as a `CYRUP-DELTA` **naming the ordering guarantee it relies on** —
that guarantee is the whole reason the code is correct, and a future refactor that responds from a
different task breaks it silently. Once-ness becomes `Option::take()`, so the `startupInfoSent` flag
is unrepresentable.
**Severity lowered from `high` to `medium`**: the correct implementation is the default one, the
deliverable is a doc comment plus a wire-order assertion, and the regression drops one informational
chunk whose text is also on the response. `high` there read as blocking-ness.
*Verify* — as tabled; the assertion must be on **wire order**, not on parsed values. **Open question
`ACP-Q16`**: carrying the prelude in both `_meta.piAcp.startupInfo` and a chunk means a client that
renders both shows it twice. pi-acp accepted that.

**ACP-069 — `available_commands_update` is also deferred past the response.**
*Upstream* — a second `setTimeout(…, 0)` whose comment is explicit: *"some clients (e.g. Zed) will
ignore notifications for an unknown sessionId. So we must send this after the session/new response
has been delivered."* On any throw it falls back, without re-raising, to the file-based path.
*cyrup* — same ordering discipline as `ACP-068`; no timer. The source is
`AgentSession::slash_command_catalog()`, the skill gate is
`session.services().settings.effective().enable_skill_commands()`, and the try/catch ladder is cut.
**The block is duplicated in `loadSession`** with a shortened comment, and a port that gets the
ordering right on `session/new` and reuses a plain `send_notification` on `session/load` produces a
session whose command menu is empty for reasons no log shows — factor one
`async fn advertise_commands(&self, cx, session)` called from both handlers.
`CYRUP-DELTA`: `AvailableCommand.description` is a required `String` in Rust; the fallback is
`ACP-080`, which is defined upstream and must not be re-decided.
*Verify* — as tabled. **Open question `ACP-Q17`**: pi-acp passes `includeExtensionCommands: false`,
deliberately hiding extension commands. `ACP-269` reverses that; the two units must agree.

**ACP-070 — The eight headless built-ins.**
*Upstream* — a fixed eight-element array in this order with these exact strings: `compact` /
"Manually compact the session context" / hint "optional custom instructions"; `autocompact` / "Toggle
automatic context compaction" / "on|off|toggle"; `export` / "Export session to an HTML file in the
session cwd"; `session` / "Show session stats (messages, tokens, cost, session file)"; `name` / "Set
session display name" / "<name>"; `steering` and `follow-up`, each with a two-clause description and
the hint "(no args to show) all | one-at-a-time"; and `changelog` / "Show pi changelog". It is a
headless-friendly **subset** plus three toggles, not pi's interactive builtin registry.
*cyrup* — a `const BUILTINS: &[AcpBuiltin]` where `AcpBuiltin` is the domain enum of `ACP-282`, so
the advertised list and the dispatcher cannot drift — the two upstream lists are ~450 lines apart in
one file with nothing relating them, and the ceremony is one `const fn wire_name` plus its inverse.
Descriptions need the two `pi` occurrences reworded, and `changelog` needs the cut's decision.
**Severity raised from `low` to `medium`, because the mechanism as filed makes the unit's own verify
unsatisfiable, and it walked into the same-name trap.** `cyrup_session_svc::SessionCommand` has **no
`ExportHtml` variant** — its export verb is `ExportJsonl { .. }` and the HTML render is the free
function `session_jsonl_to_html`; `ExportHtml { output_path }` is a variant of the **other**
`SessionCommand`, `cyrup_modes::rpc::types::SessionCommand`, which is the pi-compatible JSONL wire
enum and is not what an in-process host dispatches. Worse, **`changelog` has no backing verb anywhere
outside the TUI**: it is an in-crate effect in `crates/cyrup-tui/src/app/submit.rs`. So two of the
eight advertised rows have no dispatch path as specified, and advertising a command that silently
does nothing is a dead row in the client's palette. The other six check out: `Compact`,
`SetAutoCompaction`, `GetSessionStats`, `SetSessionName`, `SetSteeringMode`, `SetFollowUpMode`.
*Verify* — as tabled; the second half is the one that fails today. **Open question `ACP-Q18`**: whether
the list should be *derived* from `cyrup_tui::BUILTIN_SLASH_COMMANDS` filtered to the headless-safe
subset. Derivation prevents drift but changes the eight strings; hand-writing preserves them and
guarantees drift. Note upstream is internally inconsistent here: `mergeCommands(piCommands, builtins)`
lets a user command named `compact` shadow the builtin in the **advertised** list while `prompt()`'s
if-chain still intercepts it, so the two sets are already out of sync upstream.

**ACP-072 — `session/set_mode` sets the thinking level and echoes the applied one.**
*Upstream* — restore the session (`Unknown sessionId` if absent), reject a non-member `modeId` with
`` `Unknown modeId: ${mode}` `` at −32602, `await proc.setThinkingLevel(mode)`, then fire-and-forget
`current_mode_update` and await `emitConfigOptionsUpdate`. **The echoed `currentModeId` is the
REQUESTED value, not the applied one** — pi-acp has no clamping concept.
*cyrup* — `thinking_level_from_str` for the parse, membership against `available_thinking_levels()`
for validity. `AgentSession::set_thinking_level(level).await` **returns the effective level after
clamping** (`clamp_thinking_level(m, level)`, or `Off` for a modelless session), and **that return
value — never the request's `mode_id` — is what `CurrentModeUpdate` must carry**; ADR-0028's
`AppliedMode` newtype exists to make the wrong value unconstructible. `set_thinking_level` also
republishes `CYRUP_REASONING_LEVEL` for the next bash child and appends a `thinking_level_change`
entry, so it must not be bypassed. Errors hand-built so the message survives. Unlike pi-acp's `void`,
`send_notification` is synchronous, so both updates go out in program order.
*Verify* — as tabled: **a test asserting only that `{}` came back passes the broken version.**
**Open question `ACP-Q19`**: reject an unsupported-but-well-formed level with −32602, or accept and
clamp? Only clamping is safe if the mode list is model-derived, because then an unsupported level is
never advertised.

**ACP-073 — `session/set_config_option` routes `model` and `thought_level`.**
*Upstream* — restore; reject a non-string `value` with `` `Expected string value for config option:
${configId}` ``; route `model` → `setSessionModel`, `thought_level` → validate then set and emit
`current_mode_update`; anything else → `` `Unknown config option: ${configId}` ``. Then
`emitConfigOptionsUpdate` and return `{configOptions}`. **The pinned notification order for the
thought-level case is `current_mode_update` then `config_option_update`**; the model case emits
exactly one.
*cyrup* — `SetSessionConfigOptionRequest.value` is `SessionConfigOptionValue` — an enum,
`{Boolean{value}, #[serde(untagged)] ValueId{value}}` — not a bare string, so the `typeof` guard
becomes a `match` whose `Boolean` arm produces the equivalent error. Route on the two `const` ids from
`ACP-064`. The `model` branch is `AgentSession::set_model(pattern)`, which already resolves both
`provider/id` and a bare id through `cyrup_config::ModelResolver::{parse_pattern, match_reference}`
and then runs the `has_configured_auth` precheck via `set_model_resolved` — so upstream's
split-on-first-`/`, rejoin-the-rest, then scan-the-catalog ladder has no counterpart. Map
`ModelNotFound(pattern)` → −32602 `Unknown modelId: {pattern}` and `NoConfiguredAuth(..)` → −32000
with `data.authMethods`, which pi-acp had no way to distinguish. **One fact to carry**:
`match_reference`'s third step is a **substring** match, so cyrup's resolution is strictly more
lenient than pi-acp's exact bare-id scan — a `value` of `"4"` selects a model where pi-acp raised
−32602. If parity on the rejection path matters, add an explicit membership check against
`available_model_catalog()` before calling `set_model`.
*Verify* — as tabled.

**ACP-075 — `emitConfigOptionsUpdate` re-derives the whole option set.**
*Upstream* — called with **no** cached state, so it re-issues both probes and rebuilds both selectors
from scratch, which is what makes the returned `currentValue` reflect what pi actually applied rather
than what was requested. Called from all three setters.
*cyrup* — one function over `&AgentSession` returning `Vec<SessionConfigOption>`, built from four
in-memory reads, then `cx.send_notification(SessionUpdate::ConfigOptionUpdate(..))`. The
parallelisation and the `catch → null` degradations are cut. **The re-derive-rather-than-patch
discipline must survive**: it is what keeps a clamped thinking level and a provider-swapped model
honest in the client's dropdown.
*Verify* — as tabled.

**ACP-077 — Push config/mode updates on session-originated changes.**
*Upstream* — **none.** pi-acp emits these only from inside its own setters, so a model switched by an
extension's `pi.setModel`, by a queued command, or by any in-agent route leaves the client's dropdown
showing the old value indefinitely. pi's own RPC stream *does* emit `model_changed` and
`thinking_level_changed`; `agent.ts` never subscribes them. **This is a latent defect upstream, not a
behaviour to reproduce.**
*cyrup* — `AgentSessionEvent::{ModelChanged, ThinkingLevelChanged}` are already emitted by
`set_model_resolved`/`apply_model_change` and `set_thinking_level` for **every** cause. Subscribe them
in the ACP pump and push from there, which makes `ACP-072`/`ACP-073`/`ACP-075` plain calls whose
notifications arrive via the same path — one emitter, no double-push to suppress. Note
`cyrup_modes::is_upstream_wire_event` deliberately keeps `ModelChanged` off the RPC wire as a cyrup
super-set member; **that filter is for the pi-compatible JSONL stream and must not be applied to the
ACP pump.**
*Verify* — as tabled. **Open question `ACP-Q20`**: if the setters also push directly while the pump
pushes, every ACP-originated change emits two identical updates. The pump must be the single emitter,
which means the setters must not notify — a deliberate divergence from `ACP-072`/`ACP-073`'s pinned
notification counts.

**ACP-078 — The `Unknown sessionId` gate every setter opens with.**
*Upstream* — `setSessionMode`, `setSessionConfigOption` and `unstable_setSessionModel` all begin with
`await this.restoreSession(params.sessionId)`, and an id that is neither live nor recoverable throws
`` RequestError.invalidParams(`Unknown sessionId: ${sessionId}`) `` at −32602.
*cyrup* — the mechanism cannot be inherited: pi-acp's resolution is a three-source lookup that
transparently respawns an evicted session, whereas one-live-session has exactly one `Arc<AgentSession>`
and no respawn. **Every handler in this area must compare `req.session_id` against
`AgentSession::session_id()` itself and emit that exact error**, and the concurrent-request
de-duplication `restoringSessions` provides has to be re-decided (`ACP-209`). The survey mentioned
this parenthetically inside two other units and filed nothing, so nobody owned the string, the code or
the check — and the likely outcome is three handlers each inventing a different message, or none
checking at all and a stale id silently mutating the live session. Build the id through ADR-0028's
`AcpSessionId::try_from` as the handler's first statement.
*Verify* — as tabled.

**ACP-079 — The setters do real blocking work and must leave the dispatch loop.**
*Upstream* — awaits `proc.setThinkingLevel` / `proc.setModel` / `getSessionConfiguration` inline,
which in Node costs a JSON-RPC round trip and blocks nothing.
*cyrup* — it cannot be inline. `AgentSession::set_thinking_level` awaits an append to the session file
and then `ext_host.dispatcher().dispatch_notify(&HostEvent::ThinkingLevelSelect, ..)`, which runs
guest extension code; `set_model_resolved` runs `install_owning_provider` — provider rebuild and
credential resolution — and then `apply_model_change`. `ConnectionTo`'s doc is unambiguous that the
connection cannot process new messages while a handler runs, so an inline await in `session/set_mode`
blocks `session/cancel` and every other inbound message for the duration; and the
**`cx.spawn`-must-not-propagate-`Err`** trap applies to these three handlers identically. `ACP-057`
states the discipline for `session/new` only, which is how it gets forgotten on the other three:
**the rule is a property of the area.** `authenticate` (`ACP-055`) is the one handler explicitly
granted the inline path.
*Verify* — as tabled.

#### Refuted — 4b

| id | struck because | cyrup symbol |
|---|---|---|
| ~~ACP-067~~ `quietStartup` suppresses the prelude but not the update notice | nothing survives to port. Its two behaviours are "suppress the inventory" and "still emit the update notice"; the update notice is cut (`crates/cyrup/src/update_check.rs` already litigated pi's release-feed check), and the first is **already implemented**, not merely available. Residue is one `if` inside `ACP-066`'s renderer | `cyrup_tui::StartupReport::{quiet_startup, show_listing, has_diagnostics}` (`crates/cyrup-tui/src/startup.rs`), filled by `build_startup_report` from `EffectiveSettings::quiet_startup` |
| ~~ACP-074~~ `setSessionModel`'s two accepted model-id spellings | the entire subject is a cyrup capability under another name, verified in source: `set_model(pattern)` → `parse_pattern(pattern, true)` → `match_reference`, which does exact-canonical, then exact-bare-id, then partial matching with an alias-preferred tiebreak. Upstream's split/rejoin/scan ladder has no counterpart to write; the residual error mapping is two match arms of `ACP-073`, which already names the same target | `AgentSession::set_model` (`crates/cyrup-session-svc/src/session/model.rs`) over `cyrup_config::ModelResolver::{match_reference, parse_pattern}` |
| ~~ACP-076~~ `unstable_setSessionModel` | struck on both grounds: fully covered by `ACP-073` + `ACP-075`, and moot under the chosen binding — schema 1.7.0 has no model-setting method at any feature flag, and the only remaining route routes solely on a leading `_`, which nobody in this repo can confirm for the TS SDK. Recorded in `## 3. Cuts`; re-file against observed client evidence if one is ever seen | `AgentSession::set_model`, reached through `ACP-073`'s `model` branch |

### 4c · The turn loop and the event translation layer

Upstream: `src/acp/session.ts` (1 034 lines — pi-acp's engine) and
`src/acp/translate/{bash,pi-tools,pi-messages}.ts`.

`AgentSessionEvent` is typed, so the twelve-key `bashCommand` probe, the four-deep stdout ladder and
the `{partialArgs}` wrapper have no input to defend against, and `cleanupToolCall` drops from five
maps to two. **But one part of this area gets *worse* than upstream, and the ground truth had it
backwards.** The ACP terminal protocol is driven entirely by tool events; cyrup's `bash` tool
`ToolUpdate.content` is a **tail-truncated preview** (`build_stream_update` → `truncate_tail`,
`crates/cyrup-tools/src/tools/bash.rs`), so the append-delta's prefix assumption breaks by
construction once output exceeds the limit — and cyrup's bash result carries **no exit code at all**
(`BashDetails` is `{truncation?, fullOutputPath?}`; a non-zero exit is an `Err` whose text ends
`Command exited with code {code}`).

**The area's structural blind spot, inherited from upstream, is worth stating before the table.**
pi-acp models the turn with a nullable `pendingTurn` and a session-wide event handler because its
wire has no run correlation. cyrup already solved that: `AgentSession::prompt`
(`crates/cyrup-session-svc/src/session/run.rs`) registers `fanout.subscribe_run()` **before**
dispatching and returns that stream, and `Fanout::end_run` (`crates/cyrup-session-svc/src/subscriber.rs`)
clears the run-scoped senders **immediately after** `emit_agent_settled` — its own doc says *"so a
run-scoped consumer observes the settle as its last event."* Using session-wide `subscribe()` throws
that away and re-creates the correlation problem in a codebase that had solved it. That is `ACP-153`,
and `ACP-154`/`ACP-155` are the two failures it does not close on its own.

| id | title | upstream | sev | eff | verify |
|---|---|---|---|---|---|
| ACP-120 | SessionManager: registry, lookup error string, the one-live-session collapse | `session.ts` `SessionManager` | medium | S | `session/prompt` with an unregistered sessionId returns `-32602`, message exactly `Unknown sessionId: bogus` |
| ACP-121 | A prompt resolves **only** on `agent_settled` | `session.ts` `pendingTurn` / `startTurn` / the settle arms | critical | M | against a faux provider that fails once and auto-retries: assert the response is written **after** the settle fanout, and exactly one `PromptResponse` despite two `AgentEnd`s |
| ACP-122 | `lastEmit` ordering: the response never overtakes a notification | `session.ts` `emit` / `flushEmits` | high | S | transport-level: a final chunk immediately before settle appears **before** the response frame; a `send_notification` returning `Err` still yields a `PromptResponse` rather than closing the connection |
| ACP-123 | `cancelRequested` and the `StopReason` mapping | `session.ts` `cancel` / `wasCancelRequested`; `agent.ts`'s `'error'` collapse | high | S | `session/prompt` then `session/cancel` mid-stream: the cancel is dispatched **before** the response (proving the handler did not block) and the response carries `stopReason: "cancelled"` |
| ACP-124 | The turn queue and the `_meta` queue-depth publication | `session.ts` `turnQueue` / `QueuedTurn` | medium | M | two overlapping prompts: exactly one run starts, the queued chunk text is byte-exact `Queued message (position 1).`, and the second response arrives only after the second settle |
| ACP-125 | Startup-info deferral: set / send-if-pending | `session.ts` `setStartupInfo` / `sendStartupInfoIfPending` | low | XS | call the send twice; exactly one notification is enqueued |
| ACP-126 | The prompt-failure path: flush, auth detection, queue clearing | `session.ts` `startTurn`'s rejection handler | high | S | with a provider that fails preflight: the response is a JSON-RPC error (not a fabricated `end_turn`), the connection stays open, and any queued prompt receives a response rather than hanging |
| ACP-127 | `message_update`: text and thinking deltas to chunks | `session.ts`'s `message_update` arm | medium | XS | exactly one notification per `TextDelta`/`ThinkingDelta` and zero for `TextStart`/`TextEnd`/`Done` |
| ACP-128 | Early tool-call surfacing from streaming deltas | `session.ts`'s `toolcall_start/delta/end` branch | medium | M | one `tool_call` then `tool_call_update`s for one id, all `pending`; a truncated argument buffer produces a **partial object**, not a `{partialArgs}` wrapper |
| ACP-129 | Monotonic tool-call status and the first-vs-update decision | `session.ts` `currentToolCalls` | medium | S | advance to `InProgress` then replay a `Pending` advance: the emitted update carries `in_progress` and is a `tool_call_update`; a second `tool_call` for a known id is never produced |
| ACP-130 | `toToolCallLocations`: path probing and cwd resolution | `session.ts` `getToolPath` / `toToolCallLocations` | low | XS | relative resolves against the session cwd, absolute passes through, missing path → `None`, and `line: None` serialises with no `line` key |
| ACP-131 | `tool_execution_start` (non-bash): snapshot capture and the transition emit | `session.ts`'s `tool_execution_start` arm | medium | M | with a temp dir: `edit` on an existing file records `Content`, on a missing file `Absent`, on an unreadable file `Unreadable`; `tool_call` emitted once at `in_progress` when no stream delta preceded it |
| ACP-132 | `findUniqueLineNumber`: unique-oldText line inference | `session.ts` `findUniqueLineNumber` | low | XS | table: unique needle at line 3 → `Some(3)`; twice → `None`; empty → `None`; absent → `None` |
| ACP-133 | `getParsedEdits` / `getEditOldTexts`: current and legacy edit schemas | `session.ts` both functions | low | S | pin whichever needle order was chosen, so a later refactor cannot silently flip which line the location points at |
| ACP-134 | `tool_execution_update`: partial output and file-mutation suppression | `session.ts`'s `tool_execution_update` arm | low | S | an `edit` update produces neither `content` nor `rawOutput`; a `grep` one produces both |
| ACP-135 | `tool_execution_end`: the structured diff, and diff-suppresses-`rawOutput` | `session.ts`'s `tool_execution_end` arm | high | M | three cases: write-to-new-file → `old_text: None`; an edit whose pre-read failed → **no diff** (the delta from upstream, asserted explicitly); a diff-bearing update carries no `rawOutput` |
| ACP-136 | `toolResultToText`: the diff → content → stdout → JSON ladder | `translate/pi-tools.ts` `toolResultToText`; `test/unit/pi-tools.test.ts` | medium | S | content-block extraction, `details.diff` precedence, and the JSON fallback for a result with no text content |
| ACP-137 | `cleanupToolCall`: teardown at tool end | `session.ts` `cleanupToolCall` | low | XS | drive a tool start with no matching end, then `AgentSettled`; assert all per-call maps are empty |
| ACP-138 | `isBashTool` and `bashCommand`: the tool-call title | `translate/bash.ts` both | low | XS | `{command:"ls"}` → title `ls`; `{command:"   "}` and `{}` → title `bash`; tool name `Bash` recognised |
| ACP-139 | `emitBashToolCall` and the `terminal_info` `_meta` protocol | `session.ts` `emitBashToolCall`; `translate/bash.ts` | medium | S | golden: the first bash `tool_call` carries `content[0].terminalId` and `_meta.terminal_info.terminal_id` equal to the tool call id, snake_case; the second emission for the same id carries neither |
| ACP-140 | `bashOutputDelta`: the append-only terminal delta | `session.ts` `emitBashOutputUpdate`; `translate/bash.ts` | high | M | unit on the appender: a growing snapshot yields only the suffix; a head-dropping snapshot yields the chosen desync outcome, **not** a duplicate append. Component: a command exceeding the truncation limit, asserting the concatenated data contains no repeated segment |
| ACP-141 | `bashExitCode` and the `terminal_exit` `_meta` | `translate/bash.ts` `bashExitCode` | medium | S | run `sh -c 'exit 42'`; assert `terminal_exit.exit_code == 42`, not 1. **This test fails today under a faithful port, which is the point of writing it** |
| ACP-142 | `auto_retry_start` / `_end` status chunks and their exact strings | `session.ts`'s retry arms + `formatAutoRetryMessage` | low | XS | `(1,4,1500)` → `Retrying (attempt 1/4, waiting 2s)...`; `(1,3,400)` → `waiting 1s`; `(1,3,0)` → `waiting 0s`; and **no** emitted chunk contains the `error_message` text |
| ACP-143 | `auto_compaction_start` / `_end` status chunks | `session.ts`'s two compaction arms | medium | XS | `CompactionStart{Threshold}` → the byte-exact string; `{Manual}` → nothing; `CompactionEnd{aborted:true}` → no success string |
| ACP-144 | `extension_ui_request` dispatch and the catch that always answers | `session.ts`'s arm + `handleExtensionUiRequest` | high | M | with a client that errors on `session/request_permission`: the guest's `ui.confirm` still returns the deny default within the test timeout, and the connection stays open |
| ACP-145 | Select: option ids and the strict round-trip | `session.ts` `handleExtensionSelect` / `optionIndex` | critical | S | a guest `ui.select` with three options produces three `allow_once` options; selecting the second returns the second option's **string**; a cancelled outcome and a fabricated `option_id` both return the deny default |
| ACP-146 | Confirm: the two fixed options and the cancelled outcome | `session.ts` `handleExtensionConfirm` + `CONFIRM_PERMISSION_OPTIONS` | critical | S | `RequestPermissionOutcome::Cancelled` yields `UiReply::Confirm(false)` at the guest, **and** a variant reached only through the `#[non_exhaustive]` wildcard also yields `false` |
| ACP-147 | Input and editor: cancellation with a visible fallback message | `session.ts`'s `input`/`editor` branch | medium | M | a guest `ui.editor` yields `Text(None)` and exactly one chat chunk; with `ClientCapabilities.elicitation` present, `ui.input` produces an `elicitation/create` and the returned string reaches the guest |
| ACP-148 | Notify: chat chunk with a severity `_meta` | `session.ts`'s `notify` branch | low | S | `UiEffect::Notify{Warning}` → one chunk with `_meta.cyrupAcp.notify.level == "warning"`; a `SetStatus` effect → no notification at all |
| ACP-149 | The synthetic dialog tool call carrying the request | `session.ts` `extensionUiToolCall` + its five-key allowlist | low | XS | the synthesised `rawInput` contains exactly the fields present on the `UiRequest` and no others — no reply-channel state, no options bag |
| ACP-150 | `requestExtensionPermission`: the catch that cancels when the client rejects | `session.ts` `requestExtensionPermission` | high | S | a client returning a JSON-RPC error for `session/request_permission`: the guest's `ui.confirm` returns `false` promptly, the connection stays open, and a later prompt still works |
| ACP-151 | `toToolKind`: the tool-name → `ToolKind` map | `session.ts` `toToolKind` | low | XS | table over every tool name cyrup registers by default, asserting none falls through to `Other` unintentionally |
| ACP-153 | Use `prompt`'s run-scoped stream, not `subscribe()` | `session.ts` `pendingTurn` (the correlation pi-acp had to invent) | high | M | a run refused inside the driver must not settle the pending turn; a run started by an extension's `ctx.prompt` must not settle it either |
| ACP-154 | `SessionReplaced` ends the stream with no settle | no upstream — pi-acp's only non-settle end is a dead child | high | S | replace the session mid-turn; assert the pending `session/prompt` receives a response (not a hang) and the driver rebinds |
| ACP-155 | The fanout applies real backpressure: never await a client round trip on the pump | `session.ts`'s `void this.handleExtensionUiRequest(ev)` detachment | high | M | park a permission dialog unanswered and keep the agent producing events past the channel capacity; assert the agent still reaches `AgentSettled` and the dialog is still answerable |
| ACP-156 | The end-of-tool re-read has no `FsOps` handle, and `std::fs` bypasses the confined backend | `session.ts`'s `tool_execution_end` `readFileSync` | high | M | with `confine_to_cwd` set, an `edit` whose path is outside the root must not ship file contents to the client in a `Diff.new_text` |
| ACP-157 | `powershell` is a second built-in shell tool every bash unit excludes | `translate/bash.ts` `isBashTool`'s exact match | medium | S | a `powershell` tool call gets `ToolKind::Execute`, a title from its `command` arg, and the terminal `_meta` family — the same as `bash` |
| ACP-158 | Prompt images and non-text content blocks reach the queued turn | `session.ts` `prompt(message, images)` / `QueuedTurn.images` | medium | S | `[Text, Image]` reaches the agent as one `UserInput` with text first then the image; assert the chosen `InputSource` is persisted |
| ACP-159 | `cancel()` resolves queued turns **without** flushing | `session.ts` `cancel()` | low | XS | pin the chosen order for the cancel path specifically, distinct from `ACP-123`'s |
| ACP-160 | `inAgentLoop` is write-only dead state and must not be ported | `session.ts` `private inAgentLoop` | low | XS | none — the deliverable is that the field does not exist; its comment becomes the doc comment on the settle transition |

**ACP-120 — SessionManager: registry, lookup error string, the one-live-session collapse.**
*Upstream* — a `Map<string, PiAcpSession>`. `get(id)` throws `RequestError.invalidParams("Unknown
sessionId: " + id)` — user-visible in Zed and byte-identical; `maybeGet` returns undefined;
`close(id)` disposes inside a swallowing try/catch then deletes; `closeAllExcept(keep)` is called from
`session/new` and `session/load`.
*cyrup* — an `AcpSessions` holding `Option<(SessionId, Arc<AgentSession>)>` rather than a map, because
`AgentSessionRuntime` is a one-slot replacer and the native-extension host-services slots are
`OnceLock`s shared across a factory. A miss returns `Error::new(-32602, format!("Unknown sessionId:
{sid}"))` built by hand — `From<ErrorCode>` would stamp `"Invalid params"`. `close` becomes
`AgentSessionRuntime::dispose().await`. `SessionId(Arc<str>)` is cheap to clone as the key.
*Verify* — as tabled. **Open question `ACP-Q21`**: whether `session/new` on an already-live session
should evict (upstream) or error. With one live session eviction is structural, so the observable
difference is only whether the old session's in-flight `session/prompt` gets a response or a dangling
request — which `ACP-154` answers.

**ACP-121 — A prompt resolves only on `agent_settled`.**
*Upstream* — `startTurn` installs `pendingTurn` and fires `proc.prompt(...)`, **whose promise is
explicitly not the turn's completion**. `agent_start` sets a flag. `turn_end` does **nothing at all**
— its arm is an empty `break` with a comment that pi uses `turn_end` for sub-steps. `agent_end` only
clears the flag, because pi may still retry, compact, or process a queued continuation. Only
`agent_settled` awaits `flushEmits()` and then resolves. Upstream's own component test drives
`agent_start / auto_retry_start / agent_end{willRetry:true} / agent_start / turn_end /
agent_end{willRetry:false}` and asserts the promise is **still unresolved**, then resolves it with
`agent_settled`.
*cyrup* — a per-turn actor holding the `Responder<PromptResponse>` (moved into the `cx.spawn`ed task)
and the run-scoped stream, settling only on `AgentSessionEvent::AgentSettled`, which
`emit_agent_settled` emits exactly once per run after `flush_pending_bash_messages`. `TurnEnd` and
`AgentEnd` are consumed and dropped; `AgentEnd` carries `will_retry: bool`, so the retry can be logged
without ever being tempted to settle on it. ADR-0028 §3's `Turn<Running>` typestate exists to make
`AgentEnd` unable to reach the transition and a second settle a compile error.
**Why `critical`** — silent wrong output: settling on `AgentEnd` returns `stopReason: end_turn` to Zed
while the retried, compacted or continued run is still streaming, so Zed closes the turn and renders
the rest of the real answer as orphan chunks outside any turn. The user reads a truncated answer as
complete and nothing anywhere reports an error. `cyrup_modes`' `run_rpc` already keys its `in_flight`
latch off `AgentSettled`, which is independent confirmation the rule is right and evidence it belongs
in shared code rather than being written a third time.
*Verify* — as tabled.

**ACP-122 — `lastEmit` ordering: the response never overtakes a notification.**
*Upstream* — `lastEmit = lastEmit.then(() => conn.sessionUpdate(...)).catch(() => {})`. The catch is
unconditional and silent: a client that has gone away must not stop the turn from completing. Every
path that resolves or rejects a turn awaits `flushEmits()` first — except `cancel()`, which is
`ACP-159`.
*cyrup* — **delete the barrier rather than port it.** `send_notification` is synchronous and enqueues
on an mpsc, so notifications are ordered among themselves. The ordering guarantee for the *response*
must be re-established structurally: **the same task that owns the event pump must own the
`Responder`**, so `responder.respond(...)` is literally the last statement after the final
`send_notification`. Notification errors are swallowed with `let _ =`, never `?` — a propagated `Err`
out of `cx.spawn` tears down the whole connection. Splitting the pump and the responder across two
tasks still compiles and races silently, which is why ADR-0028 records this as a guarantee the type
does **not** enforce.
*Verify* — as tabled.

**ACP-123 — `cancelRequested` and the `StopReason` mapping.**
*Upstream* — `cancel()` sets the flag, drains and resolves every queued turn as `'cancelled'`, emits
`Cleared queued prompts.` plus a `session_info_update` (only when the queue was non-empty), then
awaits `proc.abort()`. `startTurn` clears the flag per turn. On settle the reason is
`cancelRequested ? 'cancelled' : 'end_turn'`; on rejection, `'cancelled' : 'error'`. pi-acp's own
`StopReason` union includes `'error'`, which ACP does not have, so `agent.ts` collapses it:
`result === 'error' ? (wasCancelRequested() ? 'cancelled' : 'end_turn') : result`. The upstream test
calls cancel before any event and still gets `'cancelled'`.
*cyrup* — `AgentSession::abort_and_settle()` (`crates/cyrup-session-svc/src/session/queue.rs`) is the
exact analogue of pi's `abort()`: it cancels the retry backoff first, then the run, then waits
bounded for idle. The flag becomes a per-turn `AtomicBool` cleared when the actor is constructed, not
when the turn settles. **Do not reproduce the `'error'` arm** — it exists only because pi's RPC prompt
could reject; an in-process failure is a `SessionServiceError` and belongs in `respond_with_error`,
not in a fabricated `end_turn`.
*Verify* — as tabled.

**ACP-124 — The turn queue and the `_meta` queue-depth publication.**
*Upstream* — if a turn is pending, the prompt is pushed and **two** updates are emitted: an
`agent_message_chunk` reading exactly `Queued message (position N).` (N = length after the push,
1-based) and a `session_info_update` whose only payload is `_meta: {piAcp: {queueDepth: N, running:
true}}`. On settle, shift; if a turn comes out, emit `Starting queued message. (N remaining)` and
start it, else publish `{queueDepth: 0, running: false}`. Both comments concede the payload is
invisible in Zed today.
*cyrup* — two options and the choice is load-bearing. **(a) Faithful**: a `VecDeque<QueuedTurn>` in
the host, one `prompt` at a time — N prompts, N runs, N settles, N responses, upstream semantics
preserved. **(b) Native**: `PromptOptions { streaming_behavior: Some(StreamingBehavior::FollowUp) }`
and mirror `AgentSessionEvent::QueueUpdate{steering, follow_up}` into the `_meta` — richer, and it
surfaces steering, which pi-acp cannot see — but **one run drains all queued messages**, so all N
`session/prompt` requests settle on the same `AgentSettled` with the same `end_turn`, and one cancel
cancels all of them. Recommend (a) for the port and (b) as a follow-on, because ACP's request/response
pairing is per prompt. Either way name the `_meta` namespace `cyrupAcp`, not `piAcp`, and record it.
`QueuedTurn` must carry the images of `ACP-158`, not just the text.
*Verify* — as tabled. **Open question `ACP-Q22`**: whether Zed 0.26-era clients render
`session_info_update` `_meta` at all. Upstream's own comments say no; if not, (b)'s richer payload is
free and the decision reduces to request-pairing semantics alone.

**ACP-126 — The prompt-failure path.**
*Upstream* — on rejection: `flushEmits()`, then the auth classifier (reject with AUTH_REQUIRED if it
hits, so Zed can offer terminal login), else resolve `'cancelled'`/`'error'`; clear the pending turn;
publish a final `session_info_update`. **Crucially the queue is not drained and the next turn is not
started** — the comment says pi may be unhealthy — so queued prompts are left hanging with
`queueDepth` still published.
*cyrup* — `AgentSession::prompt_with` returns `Result<PromptAccepted, SessionServiceError>`; the `Err`
arm is the analogue, classified per `ACP-015`. Failure reaches the client via `respond_with_error` and
the spawned task still returns `Ok(())`. **Do not port the leave-the-queue-hanging behaviour**: with
one live session and no child there is no "pi may be unhealthy" condition, and stranded queued
prompts are stranded ACP requests. Drain them as `Cancelled` and record a `CYRUP-DELTA`.
*Verify* — as tabled.

**ACP-127 — `message_update`: text and thinking deltas to chunks.**
*Upstream* — `text_delta` → `agent_message_chunk`, `thinking_delta` → `agent_thought_chunk`, both
guarded on `typeof delta === 'string'`. **Every other assistant-message event type produces nothing**
— `text_start`/`text_end`/`thinking_start`/`thinking_end`/`start`/`done`/`error` all fall through to a
bare `break`. No `_meta`, no `messageId`.
*cyrup* — a `match` over `StreamEvent::{TextDelta, ThinkingDelta}` on
`AgentSessionEvent::MessageUpdate { assistant_message_event: Box<StreamEvent>, .. }`; the typed enum
removes the `typeof` guard entirely. `ContentChunk` also has an optional `message_id` the TS SDK
lacked; leave it `None` for parity and record the option.
*Verify* — as tabled.

**ACP-128 — Early tool-call surfacing from streaming deltas.**
*Upstream* — the tool call is `ame.toolCall ?? ame.partial.content[ame.contentIndex ?? 0]` — pi
"sometimes" puts it on the event and "always" in the partial. An empty toolCallId emits nothing.
`rawInput` is `toolCall.arguments` when a non-null object, else `JSON.parse(partialArgs)` and, on
parse failure, the literal wrapper `{partialArgs: s}`. First sighting of a non-bash id emits
`tool_call`; a known id emits `tool_call_update` with the **existing** status, never recomputed —
which keeps `rawInput` fresh while args stream so Zed shows a spinner.
*cyrup* — `StreamEvent::{ToolCallStart, ToolCallDelta}` carry only `content_index` + `partial:
Arc<AssistantMessage>`, so the `ame.toolCall ??` half has no analogue; read
`partial.content[content_index]` and match `Content::ToolCall`. `ToolCall.arguments` is `LazyArgs`,
which materialises a partially-streamed buffer through `parse_streaming_json_object`
(`crates/cyrup-core/src/json.rs`), so a truncated `{"path": "/et` yields a real partial map and
**the `{partialArgs}` wrapper is unreachable and must not be written**.
*Verify* — as tabled. **Open question `ACP-Q23`**: whether to emit an update on **every** delta
(upstream does) or only on Start/End. Upstream's per-delta emission is cheap over a wire it does not
own; in-process it forces `LazyArgs` materialisation per delta, defeating the laziness. Not a parity
question — Zed sees the same final state either way.

**ACP-129 — Monotonic tool-call status and the first-vs-update decision.**
*Upstream* — `Map<string, 'pending'|'in_progress'>` read as `existingStatus ?? 'pending'`, with an
explicit comment that a late delta arriving after `tool_execution_start` must **not** downgrade an
`in_progress` call, because clients hide progress if it does. The same map also decides `tool_call`
vs `tool_call_update` and `includeTerminal` — it answers two different questions and neither is named.
*cyrup* — a `ToolCallTable` newtype over `HashMap<ToolCallId, ToolCallPhase>` where `ToolCallPhase:
Ord`, whose sole mutator is `advance(id, phase) -> Emission` taking `max(existing, requested)`
internally, so **no call site can express a downgrade** and the first-vs-update decision is returned
rather than re-derived.
**Severity lowered from `high` to `medium`, on the unit's own evidence.** The ordering hazard upstream
defends against is unreachable in cyrup: `crates/cyrup-agent/src/agent/run/turn.rs` awaits
`stream_assistant()` and only then `execute_tool_calls(..)` in the same loop body, so a
`ToolCallDelta` cannot follow a `ToolExecutionStart` for the same call. What remains is the
bookkeeping, whose failure is a duplicate tool row in Zed that never completes — a rendering defect,
none of the four clauses. The monotone table is still worth building, because that ordering guarantee
lives in another crate and is not enforced at this seam.
*Verify* — as tabled.

**ACP-131 — `tool_execution_start` (non-bash): snapshot capture and the transition emit.**
*Upstream* — `isFileMutation` is an **exact** match on `edit` or `write`. If so, and a path is
recoverable, the file is read with `readFileSync` and stored as `{path, oldText}`; for `edit` only,
the first `oldText` needle yielding a unique line sets the location line. **Any throw — missing file,
EACCES, non-UTF8 — stores `oldText: null`, conflating three states into two.**
*cyrup* — `ToolExecutionStart` carries a typed `ToolCallId` that is never absent, so the
`crypto.randomUUID()` fallback is dead; `fileMutationToolCallIds` is unnecessary because `tool_name`
is on the Update and End events too. Model the snapshot as ADR-0028's
`Snapshot::{Absent, Content, Unreadable}`, where the `Unreadable` arm has **no diff constructor**.
Read through `cyrup_tools::ops::FsOps` rather than `std::fs` so the snapshot honours the session's
configured backend — **but see `ACP-156`: `AgentSessionServices` exposes no `FsOps` handle today, so
this prescribed mechanism is unreachable until that unit lands.**
*Verify* — as tabled. **Open question `ACP-Q24`**: whether the snapshot should use ACP's
`fs/readTextFile` client capability when advertised — the only way to diff against a buffer the user
has edited but not saved. Upstream ignores the capability entirely.

**ACP-135 — `tool_execution_end`: the structured diff.**
*Upstream* — re-read the file and emit a diff when `snapshot.oldText === null || newText !==
snapshot.oldText` — **a failed pre-read is treated as "this is a new file"**. The diff item carries
`snapshot.path`, the **original, possibly relative** string. Any throw on the re-read falls through to
text only. `rawOutput` is included **only when there is no structured diff**. Upstream's tests pin
three properties: no diff is synthesised at tool *start* from the requested args; the diff reflects
the **realized** file contents even when a fuzzy match changed something other than what was
requested; and `rawOutput` is absent whenever a diff is present.
*cyrup* — `ToolCallContent::Diff(Diff { path: PathBuf, old_text: Option<String>, new_text, meta })`.
`path` is a `PathBuf`, so upstream's pass-the-relative-string-through becomes an explicit decision —
Zed resolves relative diff paths against the session cwd, and passing the resolved absolute path is
safer and is the recommended delta. `Snapshot`'s three states remove the `null` conflation: only
`Absent` may produce `old_text: None`, and `Unreadable` produces **no diff at all**, which is the
divergence from upstream and must be asserted rather than absorbed. `is_error` is authoritative —
cyrup's `edit` returns `Err` for a partial batch, so a half-applied edit correctly emits no diff.
*Verify* — as tabled.

**ACP-136 — `toolResultToText`: the ladder.**
*Upstream* — falsy result → `''`. (1) `details.diff` when a non-blank string — pi's edit tool returns
a terse success line in `content` and the full unified diff in `details.diff`, so the diff wins.
(2) join the `text` of every `{type:'text'}` block with **no separator**. (3) a stdout/stderr/exitCode
ladder assembled as `stdout`, `stderr:\n…`, `exit code: n`, joined with `\n\n` and `trimEnd`ed.
(4) `JSON.stringify(result, null, 2)`, falling back to `String(result)` on a circular-reference throw.
*cyrup* — deserialize into the typed `{content: Vec<Content>, details: Option<Value>}` rather than
probing — `result_value_of` (`crates/cyrup-agent/src/agent/message.rs`) is the one place that shape is
built, so it is a closed contract. Step 1 stays (`EditDetails.diff` is still a `String`); step 2
becomes a match on `Content::Text`; **step 3 is dead against every cyrup built-in** and is cut with
`ACP-141`'s consequence attached; step 4 stays and matters, because MCP and extension tools return
arbitrary `details` and may have no text content — `serde_json::to_string_pretty` cannot fail on a
`Value`, so the `String(result)` fallback is cut too.
*Verify* — as tabled.

**ACP-139 — `emitBashToolCall` and the `terminal_info` `_meta` protocol.**
*Upstream* — the bash `tool_call` carries `title: bashCommand(args) ?? toolName`, `kind:'execute'`,
plus — **only on the first emission for that id** — `content: [{type:'terminal', terminalId:
toolCallId}]` and `_meta: {terminal_info: {terminal_id, cwd}}`. The `_meta` keys are **snake_case**,
unlike everything else pi-acp emits, because that is Zed's display-only-terminal convention. **The
terminal id IS the tool call id** — one string in two protocol namespaces.
*cyrup* — `ToolCallContent::Terminal(Terminal::new(TerminalId::from(tool_call_id.clone())))` plus
`.meta(..)`. Because `SessionUpdate` is internally tagged, the inner struct's `_meta` flattens to the
same nesting depth pi-acp writes — verified in the Architecture phase's live probe. Track terminal
state in a `HashMap<ToolCallId, TerminalState>` whose entry exists iff `terminal_info` was sent, so a
`terminal_output` naming a terminal the client never heard of is unrepresentable. **This must cover
`powershell` too** (`ACP-157`).
*Verify* — as tabled. **Open question `ACP-Q25`**: whether Zed still honours the
`terminal_info`/`terminal_output`/`terminal_exit` `_meta` convention, or whether the typed
`terminal/*` client family (ungated in 1.7.0) is now the supported route. Upstream's own comment cites
the schema docs, not a Zed release. **If the typed family works, this whole `_meta` protocol is a
cut** — resolve before building ceremony on it.

**ACP-140 — `bashOutputDelta`: the append-only terminal delta.**
*Upstream* — `previous = snapshots[id] ?? ''`, `delta = next.startsWith(previous) ?
next.slice(previous.length) : next`, store `next`, and emit `_meta.terminal_output.data` only when the
delta is non-empty. The update carries **no** `content` and **no** `rawOutput`: for bash, everything
rides `_meta`.
*cyrup* — ADR-0028's `TerminalAppender` with `Push::{Nothing, Append, Desynced}`. **This is the one
place cyrup is materially worse than pi upstream and it must not be papered over**: `build_stream_update`
takes `acc.tail_string()` and then `truncate_tail(…, TruncOpts::new(max_lines, max_bytes))`, so once
output exceeds the limit the next preview has dropped its head and is not a prefix of the last — and
upstream's fallback re-appends the whole preview into a terminal that appends. Zed's terminal shows
the last N lines repeated once per update, tens of times over one command, with nothing reporting a
problem. **Decide the desync policy explicitly** — emitting nothing (accept a gap) or emitting a
marker is defensible; silently re-appending is not.
*Verify* — as tabled. **Open question `ACP-Q26`**: whether to bypass the tool-update preview entirely
and stream from the tool's `OutputAccumulator` (`crates/cyrup-tools/src/output.rs`), which holds the
untruncated tail. That would make the prefix invariant true by construction, at the cost of a new seam
between `cyrup-tools` and `cyrup-acp`.

**ACP-141 — `bashExitCode` and the `terminal_exit` `_meta`.**
*Upstream* — probe four keys for a number, else `isError ? 1 : 0`. On a terminal status, add
`terminal_exit: {terminal_id, exit_code, signal: null}` — `signal` is always the literal null, never
omitted.
*cyrup* — **every probe path is empty**: `BashDetails` is `{truncation?, fullOutputPath?}` and a
non-zero exit is `Err(error::invalid(append_status(&body, &format!("Command exited with code
{code}"))))`, so the numeric code exists only inside human-readable error text. Three options: (a)
faithful — every failing command reports exit 1; (b) parse the trailing status line, brittle but exact
and it also covers the timeout and kill cases upstream cannot express; (c) **add `exit_code:
Option<i32>` to `BashDetails`** and populate it in the tool's terminal arms — a small contained change
to `cyrup-tools` that makes the ACP layer typed and also benefits the TUI. **Recommend (c).**
**Severity lowered from `high` to `medium`**, because the code is not actually lost to the user, only
to the terminal chrome: `Executed::from` (`crates/cyrup-agent/src/agent/run/tools/finalize.rs`)
converts a `ToolError` into `ToolResult { content: vec![Content::text(e.to_string())], .. }` with
`is_error: true`, so the `Content::Text` join picks up the whole body including the trailing
`Command exited with code 42` and it reaches the client as `terminal_output.data`; and `is_error` has
already set the tool status to `failed`, so the pass/fail signal survives too. Degraded metadata,
none of the four clauses.
*Verify* — as tabled. **Open question `ACP-Q27`**: whether `Command aborted` and `Command timed out`
should map to an exit code at all, or to `signal` — cyrup can distinguish `ExitStatus::{Killed,
TimedOut, Signaled}` and knows more than pi did here.

**ACP-143 — `auto_compaction_start` / `_end` status chunks.**
*Upstream* — two fixed strings, byte-exact, reading no field off the event:
`Context nearing limit, running automatic compaction...` and
`Automatic compaction finished; context was summarized to continue the session.` pi-acp handles no
other compaction event, so a manual compaction produces nothing.
*cyrup* — cyrup renamed these: there is no `auto_compaction_*`. Map
`AgentSessionEvent::CompactionStart{reason}` with `CompactionReason::{Threshold, Overflow}`
(`crates/cyrup-session/src/compaction/hooks.rs`) to the start string, and `CompactionEnd{reason,
result, aborted, will_retry, error_message}` with the same two reasons to the end string.
`CompactionReason::Manual` emits nothing — exactly upstream's behaviour, arrived at by a different
route, and the match arm should say so. `CompactionEnd` also carries `aborted` and `error_message`,
which upstream cannot see; **emitting the success string for an aborted or failed compaction would be
actively misleading**, so gate on `!aborted && error_message.is_none()` and record the delta. See
`ACP-283` for the interaction with `/compact`'s own summary chunk.
*Verify* — as tabled.

**ACP-144 — `extension_ui_request` dispatch and the catch that always answers.**
*Upstream* — the arm fires the async handler and attaches a catch that, on **any** throw, answers
`{id, cancelled: true}` with its own error swallowed. `handleExtensionUiRequest` reads `id` and
`method` as string props; **a missing `id` returns immediately having answered nothing** — a silent
drop. Then dispatches by method, with a final default that cancels every unrecognised one. The
invariant the structure encodes: except for the missing-`id` case, every path answers exactly once.
*cyrup* — `UiRequest` arrives on an `mpsc::UnboundedSender<UiRequest>` installed via
`LiveHostServices::set_ui_sink`, carrying a typed `UiKind` and a `oneshot::Sender<UiReply>` — so
there is no wire id to be missing and the silent-drop case is unrepresentable, and the
unrecognised-method default is unrepresentable too (`UiKind` has four variants). What survives, and is
critical to get right: **the reply must be sent on every exit path including the error one**, because
`LiveHostServices::ui_roundtrip` with no `DialogOptions.timeout` does a bare `reply_rx.await` inside
`block_in_place` — a dropped sender there parks a runtime worker thread and the wasm guest forever, so
the turn never settles and the prompt never resolves. Mirror the RPC host's pending-map pruning for
the timeout case. **And see `ACP-155`: the dialog must not be awaited on the event-pump task.**
*Verify* — as tabled.

**ACP-145 — Select: option ids and the strict round-trip.**
*Upstream* — options are `String(...)`-mapped; an empty list answers cancelled immediately with no
permission request. Each option becomes `{optionId: 'choice-' + index, name, kind: 'allow_once'}` —
**every** option is `allow_once`, because pi-acp cannot tell a permission ask from any other select.
The reply is parsed back with a strict round-trip (`Number.isSafeInteger`, `>= 0`, `String(index) ===
rest`, so `choice-01` and `choice-1.0` are rejected), and the guest receives the option **string**,
not the index.
*cyrup* — `UiKind::Select` with `UiRequest.options`. Instead of parsing an index back out, keep a
per-dialog `HashMap<PermissionOptionId, String>` owned by the dialog task, so the strict parse has
nothing to validate and is cut; a miss lands on `UiReply::Text(None)`, bit-identical to
`default_ui_reply(UiKind::Select)`. ADR-0028 §3's `DialogChoice` newtype with its private constructor
is the shape: the only way to obtain one is a lookup in the table that minted the ids **for this
dialog**, so a stale or fabricated id has no path to a reply, and a table that outlives its dialog is a
lifetime error rather than a wrong answer.
**Severity raised from `medium` to `critical`, and this is the correction that matters most in this
area.** `LocalAskChannel::confirm` (`crates/cyrup-permission-system/src/ask.rs`) — the function
`PermissionSystemExtension`'s prompt path calls — reaches the human through **`HostServices::select`,
not `confirm`**. Its own doc says so verbatim: *"Maps to `HostServices::select` + `HostServices::input`
(NOT `confirm` — port doc §7.3)."* It builds a four-option list (`"Allow Once"`, `"Allow Always"`,
`"Reject"`, `"Reject with Reason"`) and decides the grant by an exact string `match selected.as_deref()`,
with `PermissionDecisionState::{Once, Always}` and `approved: true` on the two approve arms. **So
`UiKind::Select` is the tool-permission dialog in cyrup**, and an option round-trip that returns an
approve string the user did not pick — an off-by-one, a stale per-dialog map, or a non-`Selected`
outcome falling through to the selected-arm logic — is a real `Once`/`Always` grant the user never
gave. That is the permission-bypass clause. The `Cancelled` path is safe by construction, which is
why the rating rests on the **selection round-trip**, not the cancel.
*Verify* — as tabled. **Open question `ACP-Q28`**: whether `elicitation/create` with titled
`EnumOption`s is a better carrier for `Select` — it can carry per-option descriptions, which permission
options cannot, at the cost of losing the client's permission-prompt affordance.

**ACP-146 — Confirm: the two fixed options and the cancelled outcome.**
*Upstream* — a module-level constant, exactly two options in this order: `{optionId:'yes', name:'Yes',
kind:'allow_once'}` and `{optionId:'no', name:'No', kind:'reject_once'}`. The **cancelled check comes
first and is a separate branch**; the confirmed value is only ever computed on the selected arm.
*cyrup* — `UiKind::Confirm` → the two `PermissionOption`s. `RequestPermissionResponse.outcome` is a
`#[non_exhaustive]` schema enum, so the `match` **requires** a `_ =>` arm, **and that arm must land on
`UiReply::Confirm(false)`** together with the explicit `Cancelled` arm. There is no correct default
other than deny; `default_ui_reply(UiKind::Confirm)` is already `Confirm(false)` in the RPC front-end.
**Why `critical`** — permission bypass: a wildcard that falls through to the selected-arm logic
(`option_id == "yes"` on an outcome that has no option, or a bare `true`) converts a user's dismissal
of a confirmation dialog into approval. **The evidence is corrected from the survey's**: this path is
*not* `LocalAskChannel` (see `ACP-145`), but `HostServices::confirm` is reached by the MCP owner
fence (`crates/cyrup-mcp/src/owner.rs`), subagent authority routing
(`crates/cyrup-ext-subagents/src/extension/tool/routing.rs`) and any WASM guest calling `ui.confirm`,
so the clause holds on those consumers.
*Verify* — as tabled; the second assertion is the `#[non_exhaustive]` trap made testable.

**ACP-147 — Input and editor: cancellation with a visible fallback message.**
*Upstream* — emits an `agent_message_chunk` reading exactly `Pi {method} UI request is not supported
in ACP yet; cancelling it.` then answers cancelled. A dedicated test pins both the message and the
cancellation for both methods.
*cyrup* — **`UiKind::Input` has a real answer now**: `elicitation/create` (schema 1.7.0) with an
`ElicitationSchema` of `StringPropertySchema` — map `UiRequest.prompt` to the schema title and
`UiRequest.placeholder` to the property `description`. ACP has no placeholder; `default` is a
*prefill*, which is a different thing — record as a `CYRUP-DELTA`. Gate on
`ClientCapabilities.elicitation`; when absent, fall back to the cancel, which lands on
`UiReply::Text(None)` — identical to `default_ui_reply(UiKind::Input)`, so the fallback needs no
special case. **`UiKind::Editor` genuinely has no ACP home**: its contract is a prefilled buffer in the
user's editor and `StringPropertySchema` has no multiline hint (`format` is
`email|uri|date|date-time|Other`). Best available is an elicitation field with `default = req.message`
and no length cap, as a documented delta whose cost is the loss of the real editor; otherwise cancel.
The message must be rewritten — "Pi" must not appear in cyrup's user-visible copy.
*Verify* — as tabled. **Open question `ACP-Q29`**: whether Zed answers `elicitation/create` at all is
unverified; the Architecture phase places it in the schema, not in an observed client.

**ACP-150 — `requestExtensionPermission`: the catch that cancels when the client rejects.**
*Upstream* — wraps the permission request in a try/catch; on **any** throw — the client refusing, the
connection dropping, a timeout — it answers cancelled to pi and returns `null`, which every caller
reads as "already answered". **This is the single place that guarantees a dialog cannot strand the
extension when the ACP client misbehaves.**
*cyrup* — `ConnectionTo::send_request(...).await` returns `Result<_, Error>`; the `Err` arm must send
`default_ui_reply(kind)` on the `oneshot` and return, and the enclosing spawned task must still return
`Ok(())` — a propagated `Err` out of `cx.spawn` shuts down the whole connection. Returning
`Option<Outcome>` where `None` means "already replied" preserves upstream's caller contract, but the
better shape is to make the reply sender un-droppable: pass it by value into a helper that must consume
it, so "already answered" is proven by the type rather than by a sentinel.
*Verify* — as tabled.

**ACP-153 — Use `prompt`'s run-scoped stream, not `subscribe()`.**
*Upstream* — pi-acp had to invent client-side correlation because its wire has none.
*cyrup* — the correlated primitive already exists and the natural port discards it (see this section's
preamble). Two concrete consequences a session-wide `subscribe()` cannot handle. **(1) `AgentSettled`
is once per `drive_run` *invocation*, not once per ACP prompt**: `drive_run` does
`let started = self.agent.prompt(messages).await; if let Err(e) = &started { warn }` and then falls
through to `self.emit_agent_settled().await` **outside** the `if let Ok(handle)` — so a run refused as
`RunActive` inside the spawned driver emits a spurious settle while the real run is still streaming.
**(2)** Any run the ACP host did not start — an extension's `ctx.prompt`, an SDK caller on the same
`AgentSession` — produces a settle a session-wide driver would attribute to the pending ACP turn. Both
land on `ACP-121`'s truncated-answer symptom by a route `ACP-121`'s own fix does not close. Note the
interaction with `ACP-124` option (b): `steer`/`follow_up` fold into the same run, so one run-scoped
stream and one settle covers all folded messages — which is the same fact that collapses N ACP
requests onto one response.
*Verify* — as tabled.

**ACP-154 — `SessionReplaced` ends the stream with no settle.**
*Upstream* — a pi-acp turn has exactly two terminations: `agent_settled`, or the child's prompt
promise rejecting. cyrup has a third with no upstream analogue.
*cyrup* — `Fanout::invalidate` (`crates/cyrup-session-svc/src/subscriber.rs`) emits
`AgentSessionEvent::SessionReplaced { generation }` and then clears **both** the run-scoped and the
persistent senders, so every stream ends and `AgentSettled` never arrives. This fires on every runtime
replacement path, and the Architecture phase established that an extension's `ctx.newSession()` /
`ctx.fork()` / `ctx.switchSession()` / `ctx.reload()` arrives as an ordinary prompt (SEAM-022) — **so
it can happen during an ACP turn, triggered by the agent's own tool call.** The turn actor must treat
`SessionReplaced` and bare stream termination as terminals for the pending `Responder`, respond, and
then rebind; otherwise the JSON-RPC request hangs forever and Zed's turn never closes.
`AgentSessionRuntime::watch_generation` is the rebind signal (`ACP-061`); what is new here is binding
it to the **turn's** lifetime rather than only to the driver's.
*Verify* — as tabled.

**ACP-155 — Never await a client round trip on the event pump.**
*Upstream* — detaches the dialog with `void this.handleExtensionUiRequest(ev)` **precisely so** a
human sitting on a permission prompt cannot block the child's NDJSON reader. That constraint is
nowhere stated in the surveys and is far sharper in cyrup, because the channel is bounded **and
awaited**.
*cyrup* — `Fanout::emit` does `let _ = s.send(ev.clone()).await` over `mpsc::channel(CHANNEL_CAPACITY)`
with `CHANNEL_CAPACITY = 1024`, documented as *"backpressure → slows the agent, never drops."*
`ACP-144`/`ACP-150` put `ConnectionTo::send_request(...).await` — a full client round trip with
unbounded human latency — on the dialog path, and `ACP-122` argues in the opposite direction that one
task should own both the notification sends and the responder. **If that same task also awaits a
dialog reply, the agent stalls at 1 024 queued events and the run cannot reach `AgentSettled` — a
deadlock**, because the turn cannot settle while the agent is blocked and the dialog cannot resolve
while the task that would service it is blocked. `run_rpc` avoids this structurally: `rpc_driver`
enqueues onto an unbounded `mpsc<RpcOut>` while `write_pump` owns the writer, and a pending dialog is
parked in the `PendingUi` map rather than awaited inline. The ACP driver needs the same split, and
`ACP-122`'s respond-last rule must be reconciled with it rather than stated in isolation.
*Verify* — as tabled.

**ACP-156 — The end-of-tool re-read has no `FsOps` handle.**
*Upstream* — `readFileSync` at both ends of the tool.
*cyrup* — `ACP-131` prescribes `FsOps` for the **start** snapshot; nothing prescribes it for the
**end** re-read, and the end re-read is the one whose bytes are **shipped to the client** as
`Diff.new_text`. `TraversalFs::read` (`crates/cyrup-tools/src/isolation/traversal.rs`) hard-denies a
path outside the confinement root, with a canonicalize-based symlink-escape guard, and the session
builder installs it whenever `cfg.confine_to_cwd` is set (with `ProtectedFs::rooted` layered on when
`cfg.protect_paths` is). **So a `std::fs::read_to_string` at either end reads — and at the end
transmits — file contents the session's own backend refuses to open**, and for a non-local backend the
diff would be computed against the host filesystem while the tool wrote through the configured
`FsOps`, so the diff shipped to Zed is of a different file than the one that changed. **The blocking
fact: `AgentSessionServices` exposes `cwd`, `agent_dir`, `session_dir`, `home`, `settings`,
`project_trusted`, `auth`, `resources`, `startup_diagnostics`, `model_config`, `catalog_overlay`,
`context`, `ext_host`, `guest_providers` — and no `FsOps` handle at all.** The `Backend { fs, proc }`
assembled in `SessionBuilder::build` is handed to the tool registry and never retained on the session.
The port needs either a new accessor on `AgentSessionServices` or a decision to source the diff from
`EditDetails` (`crates/cyrup-tools/src/details.rs`, which already carries `diff`, `patch`,
`first_changed_line`) instead of a filesystem re-read. **Evaluate `EditDetails` first** — it sidesteps
the whole question — but note nobody has yet read what it contains for a `write`.
*Verify* — as tabled.

**ACP-157 — `powershell` is a second built-in shell tool.**
*Upstream* — `isBashTool` is `toolName.toLowerCase() === 'bash'`, exact, and `toToolKind`'s bash case
is case-**sensitive**.
*cyrup* — cyrup ships a second shell tool the exact match never sees. `POWERSHELL_CONFIG`
(`crates/cyrup-tools/src/tools/powershell.rs`) is `name: "powershell"`, built by `ShellTool::powershell`
from the **same engine** as bash, so its result shape, its `BashDetails`, its `build_stream_update`
truncation and its `Command exited with code {n}` text are all identical. It is a known built-in —
`crates/cyrup-session-svc/src/builder.rs` lists it in the known-builtins set with a note that it *must*
be — and `crates/cyrup-tui/src/transcript/tool_builtin.rs` already classifies the two as one class
(`Builtin::Shell("PS>")` vs `Builtin::Shell("$")`). Ported verbatim, a `powershell` call gets
`ToolKind::Other`, no terminal `_meta`, no title from its command, and falls through the generic text
ladder — a visibly worse rendering than bash for a tool that is byte-identical underneath. **Every one
of `ACP-138`/`139`/`140`/`141`/`151` needs the same two-name predicate**, and the natural shape is to
key on the shell tool's config rather than a hardcoded string.
*Verify* — as tabled.

**ACP-158 — Prompt images and non-text content blocks reach the queued turn.**
*Upstream* — the prompt signature is `prompt(message: string, images: unknown[] = [])`, `QueuedTurn`
records the images, and `startTurn` dispatches `this.proc.prompt(t.message, t.images)`.
*cyrup* — no filed unit mentioned the images: `ACP-124`'s `QueuedTurn` port describes a `VecDeque`
without saying what a queued turn holds, and `ACP-124`/`ACP-126`'s mechanisms only ever pass text.
ACP's `PromptRequest.prompt` is a `Vec<ContentBlock>` (Text/Image/Audio/ResourceLink/Resource), so the
image path is not hypothetical. The target is `UserInput { text, images: Vec<Content>, source:
InputSource, expand_templates }`, whose `into_agent_message` puts text first then images. **Two
decisions fall out that nobody recorded**: how the non-image variants map — pi-acp handles them in
`translate/prompt.ts`, outside this area's file list, so they can fall between the two surveys
(`ACP-276`…`ACP-281` own them) — and what `InputSource` an ACP submission carries, since the enum is
`Cli | Stdin | Rpc | Sdk | Tui` with **no `Acp` variant** and the value is persisted into the session
record.
*Verify* — as tabled.

#### Refuted — 4c

| id | struck because | cyrup symbol |
|---|---|---|
| ~~ACP-152~~ `normalizePiMessageText` / `normalizePiAssistantText` | both functions already exist in cyrup, byte-for-byte, from the same pi helper (`tree-selector.ts`'s `extractFullContent`) that pi-acp copied. `extract_full_content`'s own doc reads *"A JSON string is returned as-is; an array concatenates the `text` of every `{\"type\":\"text\"}` block; anything else (including `null`) is the empty string"* — the same three arms in the same order — and it names `join_text` as "exactly this over the parsed representation". The residue is a `pub(crate)` promotion, not a port; filing it risks a third divergent copy. Its genuine content — whether `session/load` replay should also emit `agent_thought_chunk` for `Content::Thinking` and `tool_call` for `Content::ToolCall` instead of pi-acp's text-only MVP — is an enhancement the unit itself labelled as such, and belongs with `ACP-214` | `extract_full_content` and `join_text` (`crates/cyrup-session-svc/src/session/transcript.rs`) |

### 4d · Session persistence, listing, loading, replay and deletion

Upstream: `src/acp/session-store.ts`, `src/acp/paths.ts`, `src/acp/pi-sessions.ts`, and `agent.ts`'s
`findStoredSession` / `restoreSession` / `listSessions` / `loadSession` / `deleteSession` /
`cleanupFailedNewSession` / `lastSessionCwd`.

This area shrinks more than any other, and the shrinkage follows from one fact the adapter never has:
**cyrup mints the session id and names the file after it.** `SessionLayout::new_file_path(ts, uuid)`
(`crates/cyrup-session/src/layout.rs`) is called with `id.as_str()` at all three creation sites, so the
on-disk name is `<sanitized-timestamp>_<session-uuid>.jsonl` and the sessionId→path map is a filename
match, not stored data. That is what deletes the sidecar. **`ACP-222` is the unit that says the
derivation is currently unsound**, and it must land before `ACP-202` becomes the sole restore path.

**Nothing in this area was refuted**, and that is a finding rather than a shrug: the adversary pass
searched `crates/` by concept for every unit and confirmed that the four claiming "none to write"
each name real symbols that really do the work *and* each still carry a residual behaviour difference,
while the ones claiming `already_present: none` are genuinely absent — there is no absolute-path
validator on any session path, no cleanup of a partially-built session file anywhere, and no
ACP-shaped error construction. One supporting claim in the corpus was measured and is **false** and
must not be carried downstream: "nothing in `crates/` paginates anything" — `crates/cyrup-mcp/src/runtime.rs`
consumes MCP `nextCursor` pagination with tests. Different direction, so `ACP-208` still stands.

**A scheduling constraint that gates four units at once.** `session_list_layout`,
`session_list_cwd_filter`, `gather_session_refs` and `list_global_sessions` are all `pub(crate)` in
the `cyrup` **bin** crate, which will depend on `cyrup-acp` — so a library `cyrup-acp` cannot reach
any of them without either a lift into `cyrup-session` or putting the ACP mode inside `crates/cyrup`.
That is `ACP-Q30`, and it gates `ACP-200`, `ACP-201`, `ACP-207` and `ACP-223`, not one of them.

| id | title | upstream | sev | eff | verify |
|---|---|---|---|---|---|
| ACP-200 | Sessions-directory resolution | `pi-sessions.ts` `getPiAgentDir` / `readSessionDirFromSettings` / `getPiSessionsDir` | low | XS | covered by `shell_path_is_tilde_expanded_like_session_dir` (`crates/cyrup-config/src/settings/tests/merge_and_scope.rs`), plus `ACP-229`'s new anchoring case |
| ACP-201 | sessionId → (file, cwd) resolution, local then cross-project | `agent.ts` `findStoredSession`; `pi-sessions.ts` `findPiSession` | low | S | existing `session_resolve` tests pin the resolution; add one ACP-level case that a `session/prompt` for a session in a **different** project directory resolves and loads |
| ACP-202 | Cross-project lookup that reads no session bodies | `pi-sessions.ts` `findPiSession` (its cost side) | medium | S | build a root with N project dirs plus one file whose name does not encode its header id; assert the right path for the N and `None` for the mismatch, **and** that no session body was parsed |
| ACP-203 | Listing scan: header, title, updatedAt, ordering | `pi-sessions.ts` `listPiSessions` and its five helpers | low | XS | port `test/component/session-updatedAt-message-only.test.ts` and `session-title-long-session.test.ts` as `cyrup-session` listing tests |
| ACP-204 | `updatedAt` is a JS-`toISOString`-compatible string | `pi-sessions.ts` `pickUpdatedAtFromTail` + the mtime fallback | low | XS | every `session/list` row's `updatedAt` matches `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$` |
| ACP-205 | Title fallback chain, and the `(no messages)` sentinel | `pi-sessions.ts` `pickTitleFromTail` → `scanSessionInfoNameFromFile` → `pickFallbackTitleFromHead` | medium | XS | a header-only file yields `"title": null`, **not** `"(no messages)"`; a 200-char first user message with no `session_info` yields exactly its first 80 characters |
| ACP-206 | ACP `SessionInfo.cwd` is a required absolute path | `pi-sessions.ts` `parseSessionHeader`; `agent.ts` `listSessions` | low | XS | a header with no `cwd` is present in `listing::list_all` (cyrup's tolerance unchanged) and absent from `session/list`, with no `"cwd": ""` row on the wire |
| ACP-207 | `listSessions` defaults its cwd filter to `lastSessionCwd` | `agent.ts` `listSessions` + `lastSessionCwd` | medium | XS | port `test/component/session-list-scoped.test.ts`: `{}` with a last cwd returns only that cwd's session; explicit `cwd` overrides; `{}` with no last cwd returns both |
| ACP-208 | Numeric-offset opaque cursor, page size 50 | `agent.ts` `listSessions`'s pagination block | medium | S | over 120 sessions: page 1 has 50 rows and `nextCursor == "50"`; page 3 has 20 and `null`; `"abc"`, `"-5"`, `"0"` all return page 1; the last page's JSON contains `"nextCursor":null` and `"_meta":{}` |
| ACP-209 | Single-flight restore | `agent.ts` `restoreSession` + `restoringSessions` | critical | M | two concurrent `session/prompt`s for one unloaded id with a counting factory: `build` called **exactly once**, both requests answered, and the JSONL contains exactly the two expected user entries |
| ACP-210 | Unknown sessionId is `-32602` with the exact text | `agent.ts` `restoreSession` / `loadSession`; `session.ts` `SessionManager.get` | low | XS | the raw error frame is `{"code":-32602,"message":"Unknown sessionId: <id>"}` byte-for-byte |
| ACP-211 | `cwd` must be absolute on `session/new` and `session/load` | `agent.ts` both guards | low | XS | `session/load` with `cwd:"relative/dir"` returns the exact message and performs no filesystem work |
| ACP-212 | `session/load` tears down the live session before restoring | `agent.ts` `loadSession`'s `close` + `closeAllExcept` | low | S | `session/load` on the already-live id still yields exactly one `factory.build` and one `SessionReplaced`, and re-emits `available_commands_update` |
| ACP-213 | `AppMode::Acp` must persist sessions | implicit — every feature in this area presupposes a JSONL | medium | XS | `to_session_config` with `AppMode::Acp` and no `--no-session` yields `persist == true`; with `--no-session`, false. Plus: a session created by `session/new` appears in a later `session/list` |
| ACP-214 | Replay: user and assistant text | `agent.ts` `loadSession`'s replay loop; `translate/pi-messages.ts` | low | M | `session/load` emits `user_message_chunk` / `agent_message_chunk` in transcript order with the expected texts, and nothing for a message with no text blocks |
| ACP-215 | Replay: synthetic completed `tool_call` + `tool_call_update` pairs | `agent.ts`'s `toolResult` branch; `translate/pi-tools.ts` | medium | M | port `test/component/session-load-toolresult.test.ts`: four notifications in order, `failed` on the errored one, and **both** updates in a pair carry the same `toolCallId`, equal to the persisted one and stable across two loads |
| ACP-216 | Replay: the bash terminal variant | `agent.ts`'s `isBash` branch; `translate/bash.ts` | low | S | the replayed `tool_call` carries `content[0].terminalId` and matching `_meta.terminal_info`; the update carries both `terminal_output` and `terminal_exit` with `signal == null`; empty output emits `terminal_exit` only |
| ACP-217 | Replay precedes the response; command advertisement follows it | `agent.ts` `loadSession`'s awaited loop vs its trailing `setTimeout` | medium | S | raw frame sequence: every replay notification precedes the response, `available_commands_update` follows it. Second: a `session/cancel` during a long replay is observed **before** the load response |
| ACP-218 | `session/delete` is idempotent and deletes the file | `agent.ts` `deleteSession` | medium | S | port `test/unit/session-delete.test.ts`'s four cases; the absent-file idempotence of the underlying call is already covered by `crates/cyrup-session-svc/src/tests/delete_session_file_trash.rs` |
| ACP-219 | `session/delete` of the session that is currently live | `agent.ts` `deleteSession` (which has no active-session guard) | critical | S | deleting the live id returns `{}` (not an error), the file is gone, **no headerless stub reappears** after a later write, and a following `session/prompt` for that id errors rather than resurrecting the file |
| ACP-220 | Remove the session file for a `session/new` that never returned an id | `agent.ts` `cleanupFailedNewSession` | medium | S | force a post-build `session/new` failure; the client sees the error and the sessions directory contains no new `*.jsonl` — specifically no header-only stub |
| ACP-221 | `restoreSession`'s failure mapping, and the `cx.spawn` trap | `agent.ts` `restoreSession`'s try/catch around the spawn | high | S | a `session/prompt` for a session whose recorded cwd was deleted returns a JSON-RPC error **and the connection still answers the next request** |
| ACP-222 | Filename-derived session ids are ambiguous | `pi-sessions.ts` `findPiSession` — which resolves by the **header** id, never the filename | high | M | `uuid_of` on `0000_delete_me.jsonl` and on `s.jsonl` must not silently mis-resolve; a `--session-id my_session` round-trips |
| ACP-223 | Recursion is load-bearing under a settings-derived `sessionDir` | `pi-sessions.ts` `walkJsonlFiles` + `getPiSessionsDir`; `test/component/session-list-custom-session-dir.test.ts` | medium | S | port that fixture: `settings.json {sessionDir}` with the session one cwd-encoded level below must be listed, or the flat-only behaviour is asserted as a recorded delta |
| ACP-224 | `deleteSession` leaves the ACP session live and usable | `agent.ts` `deleteSession` | medium | S | pin the chosen semantics: either a following `session/prompt` succeeds (parity) or it errors (`ACP-219`'s shape) — the two units must not disagree |
| ACP-225 | The live-session short-circuit and the forced rebuild are in tension | `agent.ts` `restoreSession`'s `maybeGet` vs `loadSession`'s preceding `close` | medium | S | one explicit rule, asserted both ways: `session/prompt` short-circuits on live; `session/load` bypasses the short-circuit |
| ACP-226 | `loadSession`'s teardown and `lastSessionCwd` write precede validation | `agent.ts` `loadSession`'s statement order | low | XS | a `session/load` for an unresolvable id must not dispose the live session and must not re-scope the default `session/list` filter |
| ACP-227 | A leading blank line makes a session invisible upstream | `pi-sessions.ts` `readFirstLine` + `if (!first) continue` | low | XS | a session file with a leading blank line is listed and loadable — a recorded delta, since upstream drops it |
| ACP-228 | An explicit `session_info` clear erases the title in cyrup | `pi-sessions.ts`'s two title scanners | low | XS | a session named and later cleared reports `null`, not the old name; pinned as a delta |
| ACP-229 | A relative `sessionDir` anchors to the agent dir upstream | `pi-sessions.ts` `readSessionDirFromSettings` | low | XS | `{"sessionDir": "sessions-alt"}` resolves to the chosen anchor, asserted — cyrup does not absolutize today |
| ACP-230 | Symlink handling is the reverse of the cut's stated rationale | `pi-sessions.ts` `walkJsonlFiles`'s lstat-based predicates | low | XS | a symlinked `*.jsonl` under the root: assert the chosen behaviour, since cyrup lists it and upstream does not |

**ACP-202 — Cross-project lookup that reads no session bodies.**
*Upstream* — resolves one sessionId by listing **every** session — first line, 256 KiB tail, sometimes
a whole-file rescan, sometimes a second whole-file read — and discarding all but one row. That cost is
why the sidecar exists: it is a cache in front of a scan, not a source of truth.
*cyrup* — a new `cyrup_session::listing::find_by_id(root, id) -> Option<PathBuf>` that walks
`<root>/*/` with `read_dir` and matches `uuid_of(path)` — the same private helper `listing::resolve`
already uses — opening **no** files, then a single `read_header` on the one hit to recover the cwd.
This is the fast path `session/prompt` takes for a session that is not currently live; `ACP-201`'s
`gather_session_refs` stays the fallback for prefix matches and for a file whose name does not encode
its id. **Do not implement `ACP-201` as the primary restore path**: `scan_file` reads every file end to
end (it accumulates `all_messages_text`), so `session/list` is *slower* per file than pi-acp's
64 KiB + 256 KiB happy path — acceptable for a user-initiated, paginated listing, and not acceptable
on the restore path.
*Verify* — as tabled. **Gated on `ACP-222`**: the derivation this unit rests on is unsound today.

**ACP-205 — Title fallback chain, and the `(no messages)` sentinel.**
*Upstream* — the newest non-blank trimmed `session_info.name`; failing that, the first user message's
text sliced to 80 UTF-16 code units; failing that, `null`, which ACP emits as `"title": null`.
*cyrup* — `SessionInfo.name` covers the first two rungs. The third maps to `SessionInfo.first_message`
with **two differences that must be handled at the projection site**: (a) `first_message` is
untruncated, so apply the 80-character clip — reuse the `chars().take(80)` semantics of `dag_display`
(`crates/cyrup-session-svc/src/session/transcript.rs`) or `truncate_summary`
(`crates/cyrup/src/startup_ui.rs`), both of which count chars, matching JS `slice` closely enough; and
(b) **`scan_file` substitutes the literal `"(no messages)"` when no user message was found, which must
map to `title: null` and must never reach the wire as a title.** `CYRUP-DELTA` to record: cyrup's
`first_message` joins a message's text blocks with `" "` where pi-acp takes only the first block.
*Verify* — as tabled.

**ACP-207 — `listSessions` defaults its cwd filter to `lastSessionCwd`.**
*Upstream* — `effectiveCwd = params.cwd ?? this.lastSessionCwd`, then a **strict string equality**
`s.cwd === effectiveCwd` on the header cwd — no normalization, no `realpath`, no trailing-slash
tolerance. When both are null, every session across every project is returned. The comment states the
reason: Zed currently sends `{}`, and the default emulates pi's project-scoped `/resume` picker.
*cyrup* — a `last_session_cwd: Mutex<Option<PathBuf>>` on the host, written on the three paths
`ACP-082` owns. Reuse `cyrup_session::listing::list_in_dir(dir, Some(cwd), None)` — its
`session_cwd_matches` is the same strict compare — rather than post-filtering `list_all`, since the
encoded layout already isolates by cwd. **Note the interaction with `ACP-206`**: filtering rows
*after* `list_all` changes the page arithmetic in `ACP-208`, whose `nextCursor` is computed against
the filtered length, so any filter must be applied **before** the slice or pages silently shrink.
*Verify* — as tabled. **Open question `ACP-Q31`**: under an explicit `--session-dir` the cwd-encoded
directory does not exist and `list_in_dir` returns empty; the host must branch the way
`list_global_sessions` does, and `AgentSession::list_sessions`
(`crates/cyrup-session-svc/src/session/files.rs`) already implements that branch.

**ACP-208 — Numeric-offset opaque cursor, page size 50.**
*Upstream* — `NaN`, a negative and `0` all mean "start at the beginning", never an error. `PAGE_SIZE
= 50`. `nextCursor` is a decimal integer string, and **`null`, not omitted**, on the last page. `_meta`
is `{}`, an empty object, not absent.
*cyrup* — new. `ListSessionsResponse::new(page).next_cursor(next).meta(Meta::new())`; `next_cursor` and
`title`/`updated_at` carry no `skip_serializing_if` in 1.7.0, so `None` serializes as `null`, matching
byte-for-byte, while `.meta(Meta::new())` is **required** to emit `"_meta":{}` rather than omitting the
key.
*Verify* — as tabled. **Open question `ACP-Q32`**: an offset cursor over a list re-scanned per request
is not stable — a session touched between two pages re-sorts to the front and the client silently skips
one row and sees another twice. pi-acp accepted that ("For MVP"). The ACP field is explicitly opaque,
so nothing on the wire forces the offset form; `(updated_at, session_id)` is available.

**ACP-209 — Single-flight restore.**
*Upstream* — three checks with **no intervening await**: live session, in-flight promise, else
construct the async IIFE, store it in the map, and only then await, with a `finally` that deletes.
Because the map insert happens before the first `await`, two prompts arriving in one tick share one
child. JavaScript's single-threaded turn boundary **is** the lock.
*cyrup* — **this does not survive translation and must be rebuilt.** `AgentSessionRuntime::switch_session_with`
takes `&self`, reads `self.session().await`, **drops the guard**, awaits
`self.factory.build(SessionTarget::Resume(path), Some(cwd))`, and only afterwards calls `self.install(...)`.
Nothing serializes that window: `RuntimeInner` sits behind an `RwLock` but the lock is taken and
released three separate times across the build, and `install_inner` is a last-writer-wins slot
replacement.
**Why `critical`** — silent wrong output. Zed sends two `session/prompt`s for the same not-currently-live
sessionId within one tick (window restore plus a queued prompt is the concrete case). Both pass the
live check, both run `factory.build(Resume(path))` against the **same JSONL**, and both call `install`.
The second replaces the slot, disposes the first session and terminates its subscriber stream with
`SessionReplaced` — **after** its user message has been appended and its prompt accepted. The first
request never gets a response, its turn is lost, and the transcript on disk carries an orphan user turn
with no assistant reply. Nothing errors. And beyond the hung turn: two `AgentSession`s built on the
same `Resume(path)` each hold their own append fd and their own in-memory entry tree, so the JSONL gets
two interleaved parent-id trees.
*cyrup, fix* — guard it in the ACP host: a single-flight keyed by session id, or — given one live
session — a `tokio::sync::Mutex<()>` held across the whole check-build-install sequence with the live
check re-taken inside the lock.
*Verify* — as tabled. **Open question `ACP-Q33`**: the right home is arguable. Fixing it in the ACP
host leaves the same race open for `cyrup-tui`'s `/resume` and for `switch_session` generally; fixing
it inside `switch_session_with` fixes every caller but changes a shared type's concurrency contract and
needs the TUI's re-entrancy checked.

**ACP-213 — `AppMode::Acp` must persist sessions.**
*Upstream* — pi-acp spawns `pi --mode rpc` with pi's default persistence, so every ACP session has a
JSONL. Every feature in this area — `session/list`, `session/load`, `session/delete`, and
`restoreSession`'s spawn against `stored.sessionFile` — is meaningless without it.
*cyrup* — `config.persist = !no_session && (explicit || mode == AppMode::Interactive)` appears in
`crates/cyrup/src/cli/config_map.rs` **and again** in `crates/cyrup/src/prelaunch.rs::resolve_session`.
Both must admit `AppMode::Acp`. Without the change an ACP `session/new` builds a `MemStore`-backed
session, `session_file()` returns `None`, the session never appears in `session/list`, and
`session/load` for it fails `Unknown sessionId` on the next connection. **Both computations are
duplicated verbatim; adding `Acp` to only one is a live foot-gun** — collapse them into one helper as
part of this unit.
*Verify* — as tabled.

**ACP-215 — Replay: synthetic completed tool-call pairs.**
*Upstream* — for each `toolResult` message, **two** updates. First a `tool_call` claiming
`status:'completed'` unconditionally with `rawInput: null`, then a `tool_call_update` that may
**downgrade** it to `failed` — a two-step the client sees as a flicker. `toolName` defaults to the
literal `'tool'`; `toolCallId` defaults to a **fresh `crypto.randomUUID()`**. `kind` maps only three
names: `read` → read, `write`/`edit` → edit, everything else → other.
*cyrup* — `Message::ToolResult { tool_call_id: ToolCallId, tool_name, content, is_error, details }`
(`crates/cyrup-core/src/message/conversation.rs`) supplies all of it typed, **including the real
persisted id**, so the randomUUID fallback is cut (see `## 3. Cuts` for why that is a behavioural fix,
not tidying). Recall the Rust `ToolCallUpdate` puts `status`/`content` inside `ToolCallUpdateFields`,
which `#[serde(flatten)]`s back out to pi-acp's flat shape. `toolResultToText`'s ladder collapses per
`ACP-136`.
*Verify* — as tabled. **Open question `ACP-Q34`**: `ToolKind` in 1.7.0 has ten variants including
`Search`, `Execute`, `Fetch`, `Delete` and `Move`. Whether replay uses the richer mapping (diverging
from pi-acp) or stays at three is a decision, and **it must match whatever the live tool-call
translation chooses (`ACP-151`)**, or a replayed call renders differently from the same call live.

**ACP-217 — Replay precedes the response; command advertisement follows it.**
*Upstream* — every replay update is `await`ed inside `loadSession`, so all of them are on the wire
before the response. The `available_commands_update` is deliberately the opposite, deferred with
`setTimeout(fn, 0)` for the stated reason that clients ignore notifications for an unknown sessionId.
*cyrup* — **both halves are load-bearing and both are traps.** The replay must not be awaited inside
the handler in the naive way, because handler callbacks run on the event loop and a long replay blocks
an inbound `session/cancel`. Emit the replay with `cx.send_notification` — synchronous, enqueues on an
mpsc — from inside the handler, which is cheap and preserves ordering, but move any *awaiting* work
into `cx.spawn` and carry the `Responder` with it. The post-response advertisement is a `cx.spawn`
after `responder.respond(..)`. **A `cx.spawn` task returning `Err` tears down the whole connection**,
so a replay failure must become `respond_with_error` with the task still returning `Ok(())`.
*Verify* — as tabled. **Open question `ACP-Q35`**: whether the replay should be emitted synchronously
from the handler at all — a 10 k-entry session produces 10 k `send_notification` calls onto an mpsc
before the handler returns, which is fast but unbounded in memory. Bounding it means moving the whole
replay into `cx.spawn`, which then needs its own ordering guarantee against the response.

**ACP-218 — `session/delete` is idempotent and deletes the file.**
*Upstream* — reads the sidecar and independently runs discovery; if **both** miss it returns `{}` —
success, no error, no write — citing the ACP semantics that deleting a non-existent session succeeds
idempotently. Otherwise it `unlinkSync`es inside a swallowing try/catch, deletes the store entry, and
returns `{}`. There is no confirmation, no trash, and no check that the path lies under the sessions
root; a failed unlink is indistinguishable from a successful one on the wire.
*cyrup* — resolve per `ACP-201`/`ACP-202`; on a miss return `DeleteSessionResponse::new()` immediately.
On a hit call `cyrup_session_svc::delete_session_file_at(&path)`, which is a strict superset: it tries
the `trash` CLI first with pi's leading-dash guard, treats exit-0 **or** the file having vanished as
success, falls back to `remove_file`, and already returns `Ok` for `ErrorKind::NotFound`. `CYRUP-DELTA`:
trash-first against pi-acp's permanent unlink; the cost is that a deleted ACP session is recoverable
from the user's trash, which a client cannot observe but a user can.
*Verify* — as tabled. **Open question `ACP-Q36`**: `delete_session_file_at` returns
`Ok(DeleteMethod::Trash)` whenever the file merely no longer exists, which is right for an ACP delete
and hides which mechanism ran for an audit trail. Whether to surface `DeleteMethod` in the response
`_meta` is a decision.

**ACP-219 — `session/delete` of the session that is currently live.**
*Upstream* — does not distinguish. It unlinks whether or not a child holds the file — safe on POSIX
because the child's fd keeps the inode alive and the file simply disappears from listings. The ACP
contract permits no error here.
*cyrup* — **cyrup's two mechanisms actively fight this.** `AgentSession::delete_session_file` returns
`SessionServiceError::Io("refusing to delete the active session")` — an error ACP has no place for.
And `DiskStore` deliberately holds an `O_APPEND` fd whose doc says the pre-existing reopen-per-append
behaviour *"silently recreated a session file deleted underneath a live manager — leaving a headerless
stub"* and that the held fd is what stops it: *"Do not add recreate-on-delete back."* So the correct
sequence is: if the target is the live session, **dispose the runtime first** (`AgentSessionRuntime::dispose`,
the same call `run_rpc_dispatch` makes, which emits `session_shutdown` and fires `session_cancel`),
then call the free function `delete_session_file_at` — never `AgentSession::delete_session_file`.
**Severity raised from `high` to `critical`.** The naive implementation is **silent data loss on a
normal path**. Call `delete_session_file_at` on the live session's path without disposing first and
the session keeps running, keeps accepting prompts, and **keeps appending every subsequent turn to an
unlinked inode** that no listing, no `session/load` and no user can ever reach again — nothing errors,
and the client sees a healthy session. The alternative naive call is the *better* of the two failures,
because a protocol violation is at least visible. Deleting the session you are currently in is an
ordinary client action — a session picker with a delete affordance, on the row that is open — so the
data-loss path is a normal path.
*Verify* — as tabled. **Open question `ACP-Q37`**: disposing the runtime as a side effect of a delete
is a stronger action than ACP's `session/delete` implies, and it kills tracked bash children via
`session_cancel`. If the client deletes while a turn is running, the turn dies. pi-acp had no such
coupling — and see `ACP-224`, which records that pi-acp leaves the session live and usable, so this
unit's verify is a **deliberate divergence, not parity**.

**ACP-220 — Remove the session file for a `session/new` that never returned an id.**
*Upstream* — called from all four failure paths; closes the session, resolves the file from either of
two sources, unlinks it inside a swallowing try/catch, deletes the store entry. The point is that pi
has already created and written a session file by the time the adapter decides the session is
unusable, and without this the file becomes a permanent ghost in every future `session/list`.
*cyrup* — the same hazard exists in-process for a different reason (see `ACP-060`): the build's own
`model_change` / `thinking_level_change` appends are the moment the file materialises. On any
post-build `session/new` error path, capture `AgentSession::session_file()`, dispose the runtime, and
call `delete_session_file_at`; swallow the delete failure and return the original protocol error.
*Verify* — as tabled. **Two open decisions**, both recorded rather than defaulted: cyrup may have no
counterpart to pi-acp's three auth-shaped call sites at all if `ACP-Q7` lands on the modelless side,
collapsing this to a single generic post-build path; and routing an internal stub cleanup through the
trash-first path puts adapter garbage in the user's trash, so a direct `remove_file` may be right here
even though `ACP-218` uses the trash path.

**ACP-221 — `restoreSession`'s failure mapping, and the `cx.spawn` trap.**
*Upstream* — `if (e?.name === 'PiRpcSpawnError') throw RequestError.internalError({code: e?.code},
String(e?.message ?? e)); throw e` — a **−32603** whose message is the spawn error's own text and
whose `data` is `{code}`, with every non-spawn error rethrown unchanged.
*cyrup* — no unit covered the failure side of restore: `ACP-209` covers concurrency, `ACP-210` only
not-found, `ACP-211` only the relative cwd. The in-process counterparts are `factory.build(SessionTarget::Resume(path))`
returning `SessionServiceError::MissingSessionCwd` (`switch_session_with` checks `!cwd.exists()` before
teardown), `SessionError::NotASession` from `SessionManager::open` on a file whose first parsed entry
is not a header, and plain IO. Two things must be decided and neither was: what wire error each maps to
(the `data:{code}` shape has no in-process analogue), and — **the sharp half** — that the mapping must
happen via `responder.respond_with_error(..)` inside the spawned task **with the task still returning
`Ok(())`**. A `session/prompt` for a session whose recorded cwd has since been deleted is an ordinary
input; mapping it with `?` kills the editor's agent connection.
*Verify* — as tabled.

**ACP-222 — Filename-derived session ids are ambiguous.**
*Upstream* — pi-acp never derives an id from a path: `findPiSession` matches `s.sessionId === sessionId`
where the id came from `parseSessionHeader`'s `obj.id`.
*cyrup* — the sidecar cut replaces that with a filename derivation, **and the derivation is unsound.**
Verified in source: `validate_session_id` (`crates/cyrup-session/src/ids.rs`) explicitly permits `.`,
`_` and `-` in the **interior** of an id; `SessionLayout::new_file_path` writes
`format!("{}_{}.jsonl", sanitize_ts(timestamp), uuid)`; and `listing::uuid_of` is
`stem.rsplit_once('_').map(|(_, u)| u.to_string()).or_else(|| Some(stem.to_string()))` — it splits on
the **last** underscore with a whole-stem fallback. So `--session-id my_session` derives `"session"`,
and a stem with no underscore at all is returned whole as the id. **Both failure shapes appear in
pi-acp's own fixtures**: `test/unit/session-delete.test.ts` writes `0000_delete_me.jsonl` (derives
`"me"`) and `test/component/session-list-custom-session-dir.test.ts` writes `s.jsonl` (derives `"s"`).
This is a pre-existing defect — `listing::resolve(SessionSelector::Uuid, ..)` already mis-resolves
`--session my_session` today — but the sidecar cut **promotes it from a CLI convenience path to the
load-bearing `session/prompt` restore path**, where a wrong answer means opening the wrong transcript.
Either fix `uuid_of` to `split_once` (and prove the sanitized timestamp can never contain `_`), or keep
`ACP-201`'s header-id resolution authoritative with the filename lookup as a pure fast-path hint
confirmed by `read_header`. **Do not let `ACP-202` land as the sole restore path until one is chosen.**
*Verify* — as tabled.

**ACP-223 — Recursion is load-bearing under a settings-derived `sessionDir`.**
*Upstream* — the cut of `walkJsonlFiles` asserts the real layout is two levels. **Upstream's own
fixture is a counterexample**: `session-list-custom-session-dir.test.ts` writes
`settings.json {sessionDir: "<root>/somewhere-else"}` and places the session at
`<root>/somewhere-else/--p--/s.jsonl` — one cwd-encoded level **below** the configured dir — and
asserts `listPiSessions()` finds it.
*cyrup* — cyrup cannot. `ConfigDirs::with_settings_session_dir` sets `session_dir_explicit = true` for
a settings-derived value, `session_list_layout` therefore picks `SessionLayout::literal` (whose `dir()`
returns the root verbatim, unencoded), and `list_global_sessions` under `session_dir_explicit` does a
**flat** `list_in_dir` whose `collect_paths` reads one directory and never descends. On that fixture
cyrup's `session/list` returns zero sessions. The reachable real-world case is a user whose
`settings.json` points `sessionDir` at the sessions **root**: new sessions are written flat and are
found, every pre-existing cwd-encoded session is invisible. **This also inverts `ACP-Q31`**, which
prescribes "flat scan plus cwd filter" for exactly this configuration — that prescription is what
loses the nested files. Decide explicitly: one level of descent under an explicit dir, or a documented
delta that a settings-derived `sessionDir` is flat-only.
*Verify* — as tabled.

**ACP-224 — `deleteSession` leaves the ACP session live and usable.**
*Upstream* — `deleteSession` touches only the sidecar and the file. It never closes, never disposes,
never removes the id from the manager. So after a successful delete the session is still in the map,
`restoreSession`'s first line still hits, and a following `session/prompt` succeeds — the child keeps
appending to the now-unlinked inode, which is why "safe on POSIX" is only half the story: safe from a
crash, lossy from a persistence standpoint.
*cyrup* — `ACP-219`'s verify requires the **opposite** ("a following `session/prompt` returns
`Unknown sessionId` rather than resurrecting the file"). That may well be the better semantics —
cyrup's held `O_APPEND` fd makes upstream's shape actively harmful — but it is a deliberate behaviour
change that `ACP-219` records as if it were parity. Label it a `CYRUP-DELTA` with its cost: **a client
that deletes a session it is mid-turn in loses the turn, where pi-acp would have completed it.**
*Verify* — as tabled; the two units must be implemented together and must not disagree.

**ACP-225 — The live-session short-circuit and the forced rebuild are in tension.**
*Upstream* — reconciles them explicitly: `restoreSession` returns the live session when one exists,
and `loadSession` defeats that by closing the id first so a fresh child is guaranteed and commands are
re-advertised (the comment says exactly this).
*cyrup* — `ACP-209` prescribes a single-flight whose critical section begins with "the live-session
check re-taken inside the lock", and `ACP-212` claims the close-before-restore is "implicit" because
`switch_session_with` always rebuilds. **Both cannot hold.** If the host short-circuits on a live
session, `session/load` on the already-live id never reaches `switch_session_with` and `ACP-212`'s own
verify (exactly one `factory.build`, one `SessionReplaced`) fails; if it does not short-circuit, every
`session/prompt` rebuilds the session from disk. The port needs one explicit rule — **`session/prompt`
short-circuits on live, `session/load` bypasses the short-circuit** — and implementing the two units
independently produces one of the two wrong behaviours.
*Verify* — as tabled.

#### Refuted — 4d

**Nothing in this area was refuted.** The adversary pass searched by concept for every one of the
twenty-one filed units and struck none; the reasoning is in the section preamble, and it is recorded
here rather than omitted because a negative result is worth as much as a strike.

### 4e · Slash commands, prompt translation and the built-in dispatcher

Upstream: `src/acp/slash-commands.ts`, `src/acp/pi-commands.ts`, `src/acp/pi-settings.ts`,
`src/acp/translate/prompt.ts`, and the eight-command dispatcher at the head of `agent.ts`'s `prompt()`.

**Two thirds of this area is cut, but not the two thirds the survey drew.** Nine units were struck —
every one had `cyrup_mechanism: "None to write"`, a verified `already_present` symbol, and a `verify`
reading "add nothing"; they are documentation of existing capability, which is precisely house rule
4's failure mode. Their real content survives as `CYRUP-DELTA` lines on `ACP-267` and `ACP-282`. What
genuinely remains is (a) the catalog → `AvailableCommand` projection with its two policy decisions,
(b) `promptToPiMessage`, which is pure translation with **no cyrup counterpart at all** and whose
every output string lands verbatim in the model's context, and (c) the eight built-ins, which must be
intercepted in the ACP host because they are not extension commands and the session core would send
them to the model as literal prompt text.

**The one unguarded destructive path in the whole port is here.** `/export` composes a filename from a
client-supplied `sessionId` and `AgentSession::export_to_html` ends in a bare `std::fs::write` with no
containment check — `ACP-291`, and it is the reason `AcpSessionId::export_path_in` exists in ADR-0028.

| id | title | upstream | sev | eff | verify |
|---|---|---|---|---|---|
| ACP-263 | Provenance in the advertised description | `slash-commands.ts`'s `sourceStr`; `pi-commands.ts` `describeFallback` | low | S | a row with `sourceInfo.scope=="user"` and an empty description projects to a **non-empty** description carrying exactly one provenance marker |
| ACP-266 | cyrup-acp must **not** expand prompt templates | `session.ts` `prompt()`'s `expandSlashCommand` call and its comment | low | XS | assert the **submitted** text reaching the core still starts with `/tpl`, i.e. that the host performed no lookup |
| ACP-267 | Project `slash_command_catalog()` rows into `AvailableCommand`s | `pi-commands.ts` `toAvailableCommandsFromPiGetCommands`; `slash-commands.ts` `toAvailableCommands` | medium | S | a fixture catalog with a duplicate name, a nameless row and a description-less row projects as expected: duplicate collapsed first-wins, nameless dropped, description-less carrying the `ACP-263` fallback |
| ACP-268 | `skill:` gating on `enableSkillCommands` | `pi-commands.ts`'s prefix filter, fed by `pi-settings.ts` | low | XS | with the setting false, only the prompt row is advertised — **and** `/skill:<name>` still expands when submitted, proving the gate is advertisement-only |
| ACP-269 | Reverse pi-acp's `source === 'extension'` exclusion | `pi-commands.ts`'s `includeExtensionCommands` default | medium | S | a session with a native extension registering `/deploy` advertises `deploy`, and `session/prompt` of `/deploy x` produces the handler's notify text as a chunk with **no model call** |
| ACP-271 | Carry `argumentHint` into `AvailableCommandInput` | `slash-commands.ts`'s `// input: omitted for now` comment | low | XS | a template with `argument-hint: <file>` projects to `Unstructured(hint)`; one without projects to `input: None` |
| ACP-272 | Built-in advertisement list and merge ordering | `agent.ts` `builtinAvailableCommands` + `mergeCommands` | medium | S | `BUILTINS.iter().map(name)` equals the dispatcher's accepted-name set **exactly, both directions**, so adding a variant to one without the other fails |
| ACP-276 | `promptToPiMessage`: text concatenation | `translate/prompt.ts`'s `text` case | medium | XS | reproduce `test/unit/prompt-to-pi-message.test.ts`'s three string assertions byte-for-byte |
| ACP-277 | `resource_link` → `\n[Context] <uri>` | `translate/prompt.ts`'s `resource_link` case | medium | XS | the exact golden `'Hello\n[Context] file:///tmp/foo.txt world'` |
| ACP-278 | `image` → a base64 content block with no data-url prefix | `translate/prompt.ts`'s `image` case | medium | XS | `[Text("see"), Image{…}]` yields text `"see"` and one `Content::Image` whose `data` is byte-identical to the input |
| ACP-279 | `resource` → `[Embedded Context]` in three shapes | `translate/prompt.ts`'s `resource` case | medium | S | both upstream goldens byte-for-byte, a bare-uri case, and a padded base64 string asserting the **decoded** byte count |
| ACP-280 | `audio` → an explicit not-supported marker | `translate/prompt.ts`'s `audio` case | medium | XS | `'\n[Audio] (audio/wav, 3 bytes) not supported by cyrup-acp'`, **plus** an assertion that the advertised `promptCapabilities.audio` is `false` |
| ACP-281 | Unknown content blocks: cyrup rejects the turn where pi-acp dropped the block | `translate/prompt.ts`'s `default: break` | medium | XS | pin the chosen behaviour: an unknown `type` either rejects the whole `PromptRequest` (the default) or is tolerated via a shim |
| ACP-282 | The built-in dispatch gate and argument split | `agent.ts` `prompt()`'s `images.length === 0 && startsWith('/')` guard | medium | S | `/compact` with an attached image is **not** intercepted; `/session` with trailing whitespace is; `/compactfoo` is not; a prompt template named `session` is shadowed |
| ACP-283 | `/compact` | `agent.ts`'s `compact` arm | medium | S | `/compact tighten it` emits exactly `"Compaction completed. (custom instructions applied)\nTokens before: <n>\n\n<summary>"` as one chunk; no args omits the parenthetical |
| ACP-284 | `/session` | `agent.ts`'s `session` arm | medium | S | the exact five-line shape including `Tokens: in N, out N, cache read N, cache write N, total N`; an in-memory session omits only the `Session file:` line |
| ACP-285 | `/name` | `agent.ts`'s `name` arm | medium | S | `/name` alone emits `Usage: /name <name>`; `/name my session` emits **exactly one** `session_info_update` with `title:"my session"` and a parseable ISO-8601 `updatedAt`, then `Session name set: my session` |
| ACP-286 | `/steering` | `agent.ts`'s `steering` arm | low | S | three branches, three exact strings, including `Steering mode: all` for the no-arg read |
| ACP-287 | `/follow-up` | `agent.ts`'s `follow-up` arm | low | XS | the same three branches with the follow-up strings, **plus** an assertion that the two commands write to two different modes |
| ACP-288 | `/export` | `agent.ts`'s `export` arm | medium | M | writes `cyrup-session-<safeSessionId>.html` under the session cwd and emits exactly two chunks — a text chunk equal to `"Session exported: "` (trailing space preserved) then a `resource_link` with `file://<abs>` and `text/html` |
| ACP-289 | `/autocompact` | `agent.ts`'s `autocompact` arm | low | S | `on`/`enabled`/`off`/`disabled`/no-arg/`toggle`/a typo, each pinned to the chosen boolean and the exact `Auto-compaction enabled.` / `disabled.` string |
| ACP-290 | The advertised list is in nondeterministic order | `slash-commands.ts`'s directory order, preserved by `mergeCommands` | medium | S | two consecutive projections of the same catalog produce the same order |
| ACP-291 | `/export` composes a path from client input and writes it unguarded | `agent.ts`'s `safeSessionId` regex | critical | S | a `session/load` with a hostile-shaped id is rejected at `-32602` with **no file written anywhere**, asserted by scanning the temp root; and an export path is always a direct child of the session cwd |
| ACP-292 | Built-ins bypass the turn queue, and `/compact` aborts an in-flight turn | `agent.ts`'s dispatcher position, above `session.prompt()`'s gate | medium | S | pipeline `/compact` behind a running prompt; assert the chosen behaviour — refuse, queue, or abort — rather than whichever one falls out |
| ACP-293 | `available_commands_update` is emitted from two call sites | `agent.ts`'s duplicated block in `newSession` and `loadSession` | low | XS | the `session/load` path emits the notification **after** its response, same as `session/new` |
| ACP-294 | `promptCapabilities.embeddedContext` has no cyrup env name | `agent.ts` `initialize`; `test/unit/pi-enable-embed-context-flag.test.ts` | low | XS | the name is declared in `cyrup_config::env_keys` rather than read ad hoc, and the predicate takes the value as an argument |
| ACP-295 | pi-acp's file-command precedence is inverted vs pi and vs cyrup | `slash-commands.ts` pushes user before project, then de-dupes first-wins | low | XS | none — the deliverable is that the upstream ordering is recorded as a defect the cut deletes, not as behaviour with parity value |
| ACP-296 | Legacy `skills.enableSkillCommands` resolves to a different layer | `pi-settings.ts`'s merge-then-fallback order | low | XS | global `{enableSkillCommands:true}` + project `{skills:{enableSkillCommands:false}}` resolves to the chosen value, pinned |

**ACP-267 — Project `slash_command_catalog()` rows into `AvailableCommand`s.**
*Upstream* — `name` must be a non-empty trimmed string or the row is dropped; `description` is the
trimmed string or `describeFallback`; output is `{name, description}` only, deliberately omitting
`input`. De-dupe by name, first wins.
*cyrup* — `AgentSession::slash_command_catalog()` returns `Vec<serde_json::Value>` rows already shaped
as pi's `RpcSlashCommand` — `name`, optional `description` (**key absent when empty**), `source`
(`"extension"`/`"prompt"`/`"skill"`), `sourceInfo`, optional `argumentHint`, optional
`argumentCompletions`. Project into `AvailableCommand::new(name, description)`, where `description` is
a **required `String`** in Rust — so the absent-key case must resolve through `ACP-263`, not to `""`.
De-dupe with a `HashSet<String>`, first wins. Emit via `SessionUpdate::AvailableCommandsUpdate` after
the response (`ACP-069`, `ACP-293`). **`CYRUP-DELTA`s to record here rather than as their own units**:
`PromptTemplate::description` truncates by `chars().take(60)` where pi-acp uses JS `String.length`
(UTF-16 code units), which differ for astral-plane characters; and `slash_command_catalog` returns
untyped `Value` rows, so **this one seam does require key-probing despite the in-process narrative**.
*Verify* — as tabled. **Open question `ACP-Q38`**: pi-acp drops nameless rows defensively and cyrup's
catalog cannot emit one; keep the guard as a cheap invariant or drop it.

**ACP-269 — Reverse pi-acp's `source === 'extension'` exclusion.**
*Upstream* — every `source === 'extension'` row is silently removed unless the caller opts in, which
`agent.ts` never does. The unit test pins it.
*cyrup* — **include them.** The exclusion is an out-of-process workaround: pi-acp cannot know whether
an extension command needs UI it cannot serve, and its `prompt()` hands text to a subprocess. In
cyrup the same submission reaches `AgentSession::prepare`, whose step 0 is
`try_execute_extension_command` — the command runs, its outcome is surfaced through
`surface_command_outcome` → `HostServices::notify` → `UiEffect::Notify`, and the ACP host renders that
as an `agent_message_chunk`. cyrup's TUI already advertises them
(`dynamic_commands_from_catalog_gated`, `crates/cyrup-tui/src/commands.rs`), so excluding them in ACP
would make one front-end show strictly less than another off the same session. Record the reversal as
a `CYRUP-DELTA` citing `pi-commands.ts`.
*Verify* — as tabled. **Open question `ACP-Q39`**: an extension command that opens a `UiKind::Editor`
dialog has no faithful ACP rendering; that degrades the command, it does not make advertising it
wrong — but if `Editor` cancels to `Text(None)`, a per-command capability filter may later be worth
adding.

**ACP-272 — Built-in advertisement list and merge ordering.**
*Upstream* — `mergeCommands(a, b)` concatenates and de-dupes first-wins, and `agent.ts` calls it as
`mergeCommands(piCommands, builtins)`, **so a pi-sourced command named `compact` shadows the builtin in
the advertised list while `prompt()`'s if-chain still intercepts it.** That inconsistency is upstream
behaviour, not a port target.
*cyrup* — a `const BUILTINS: &[AcpBuiltin]` derived from `ACP-282`'s domain enum, so the advertised
list and the dispatcher cannot drift — the two upstream lists are 450 lines apart with nothing relating
them, and a Rust port that normalises `follow-up` to `FollowUp` and derives `"follow_up"` on one side
while matching `"follow-up"` on the other produces a menu entry that silently becomes a literal user
message. Reword the two `pi` occurrences; `changelog` takes the cut's decision. **Fix the merge order
to builtins-first** so the advertised list matches what `prompt()` actually dispatches, and record the
upstream inconsistency in the delta.
*Verify* — as tabled. **Open question `ACP-Q40`**: cyrup's TUI has no `/steering`, `/follow-up` or
`/autocompact` builtin. Advertising them over ACP but not in the TUI is an asymmetry pi-acp introduced;
keeping it means the two front-ends disagree about what commands exist.

**ACP-276 / ACP-277 / ACP-278 / ACP-279 / ACP-280 — `promptToPiMessage`.**
These five are one 71-line pure function with no cyrup counterpart, and **every string they emit lands
verbatim in the model's context**, so each is pinned byte-for-byte against
`test/unit/prompt-to-pi-message.test.ts`.
*Upstream* — text blocks concatenate with **no separator, no trimming**; a `resource_link` appends
`\n[Context] <uri>` using only the raw `uri` (a link as the first block therefore makes the message
start with `\n`); an `image` contributes **nothing** to the text and pushes `{type:'image', mimeType,
data}` with `data` passed through verbatim — pi expects raw base64 with **no `data:<mime>;base64,`
prefix** — and drops any `uri`; a `resource` produces one of three shapes in a fixed order
(`text` → `\n[Embedded Context] {uri} ({mime})\n{text}` defaulting `text/plain`; `blob` →
`\n[Embedded Context] {uri} ({mime}, {bytes} bytes)` defaulting `application/octet-stream` with
**`bytes` the DECODED length**; neither → the bare uri line), with `uri` defaulting to the literal
`(unknown)`; and an `audio` block appends `\n[Audio] ({mime}, {bytes} bytes) not supported by pi-acp`
because, in the comment's words, *"Not supported by pi. Provide a marker so we don't silently drop
context."*
*cyrup* — a pure `fn prompt_to_user_input(&[ContentBlock]) -> (String, Vec<cyrup_core::Content>)`.
`ContentBlock::Image` maps to `cyrup_core::Content::Image { data, mime_type }`
(`crates/cyrup-core/src/message/content.rs`), which is exactly `PiImage` — base64 in `data`, no prefix
— so it is a one-line construction with no reshaping; collect into `UserInput::images` the way
`cyrup_modes::rpc::user_input` already does. The `resource` shapes become a typed `match` on the
schema's embedded-resource enum, and the bare-uri case becomes the `#[non_exhaustive]` catch-all rather
than a runtime probe; apply the two mime defaults explicitly. **The byte count must be the decoded
length** — `base64::decoded_len_estimate` is an *estimate* and disagrees on padded input, so compute it
exactly and pin it against the `3 bytes` golden. The `audio` marker is the **one string in this file
that must change** (`pi-acp` → `cyrup-acp`), and the change must be deliberate rather than incidental.
**Invariant to record**: the set of block types the translator handles meaningfully and the set
`initialize` advertises in `promptCapabilities` are two views of one fact declared ~180 lines apart in
two files; the `audio` arm exists specifically as a safety net for when they disagree, which is an
admission the code cannot keep them in agreement. Derive the capability response from the translator's
own table.
*Verify* — as tabled per unit.

**ACP-281 — Unknown content blocks.**
*Upstream* — `default: break`. Any unknown `type` contributes nothing and produces no error, no log
and no diagnostic.
*cyrup* — **the behaviour is the opposite, and it does not come free.** `PromptRequest.prompt` is a
bare `pub prompt: Vec<ContentBlock>` with neither `VecSkipError` nor `DefaultOnError`, and
`ContentBlock` is `#[serde(tag="type", rename_all="snake_case")]` with **no `#[serde(other)]` variant**
— only `#[non_exhaustive]`, which constrains Rust matches, not serde. An unknown `type` therefore fails
deserialization of the **entire `PromptRequest`**, and the client gets an invalid-params rejection of
the whole turn where pi-acp silently dropped one block and carried on. The `_ => {}` arm is mandatory
to compile but is **unreachable for wire-sourced blocks**.
**Severity raised from `low` to `medium`**, because the mechanism as filed was wrong in the direction
that matters: the survey read the crate's general leniency (`DefaultOnError` / `VecSkipError` on
*optional fields*) as covering the block list, and it does not. This is a real client-visible
divergence needing a decision — tolerate via a custom `Vec<serde_json::Value>` shim, or accept the
rejection and document it — not a free win.
*Verify* — as tabled. A `tracing::debug!` at the arm costs nothing and is the only observability this
path can have.

**ACP-282 — The built-in dispatch gate and argument split.**
*Upstream* — the dispatcher runs only when there are **zero images** and the trimmed message begins
with `/`. It then splits on the **first literal space only** — so a tab after the command name lands
inside `cmd` — and tokenises the remainder with the quote-aware parser. An unmatched `cmd` falls
through to the session unchanged, so a template or extension command named `deploy` is unaffected but
one named `compact` is permanently shadowed.
*cyrup* — an `AcpBuiltin` domain enum with `from_name` / `wire_name` / `ALL`, from which `ACP-272`'s
advertisement list is **derived**, so exactly one place spells each name and an added variant fails to
compile until both matches are extended. Split the args with `cyrup_resources::parse_command_args`,
but **keep pi-acp's literal-space name split** so `/compact\tfoo` behaves identically — record a
`CYRUP-DELTA` against `cyrup_resources::prompt::split_command`, which splits on any whitespace. Document
the shadowing order at the site: the ACP host dispatches **before** `AgentSession::prepare`, so it
shadows extension commands and prompt templates of the same name, exactly as pi's TUI builtins do.
*Verify* — as tabled. **Open question `ACP-Q41`**: whether shadowing is the right default — a user with
a `/session` prompt template loses it under ACP but keeps it in the TUI.

**ACP-283 — `/compact`.**
*Upstream* — `customInstructions = args.join(' ').trim() || undefined`; call compact; build
`` `Compaction completed.${custom ? ' (custom instructions applied)' : ''}` ``, then
`` `Tokens before: ${tokensBefore}` `` when it is a number; join with `\n`; append `\n\n${summary}`
when the summary is a truthy string. One chunk, `end_turn`. **A thrown compact rejects the whole
`prompt()` request** — there is no try/catch on this arm.
*cyrup* — `session.compact(custom).await -> Result<CompactionResult, SessionServiceError>`
(`crates/cyrup-session-svc/src/session/compaction.rs`). `CompactionResult` is typed: `summary: String`
and `tokens_before: u64` are **both non-optional** (`crates/cyrup-session-svc/src/state.rs`), so the two
`typeof` guards collapse — `Tokens before:` is always emitted, and the summary block iff `summary` is
non-empty (JS truthiness on `''`). Map `Err` to `respond_with_error(Error::new(-32603, e.to_string()))`
**from the spawned task**, not a propagated `Err`.
*Verify* — as tabled. **Open question `ACP-Q42`**: cyrup emits `CompactionStart`/`CompactionEnd` on the
same operation, so if the event translator also projects those (`ACP-143`) the client sees both the
events and this summary chunk. Decide once, here or there.

**ACP-284 — `/session`.**
*Upstream* — builds lines conditionally in a fixed order: `Session:`, `Session file:`, `Messages:`,
`Cost:` (raw JS number formatting, no currency, no fixed decimals), then `Tokens: ` plus a `, `-joined
list of the present sub-parts in order. If **no** line was produced, the entire output is instead
`` `Session stats:\n${JSON.stringify(stats, null, 2)}` ``.
*cyrup* — `session.session_stats().await -> SessionStats` (`crates/cyrup-session-svc/src/session/stats.rs`,
`crates/cyrup-session-svc/src/state.rs`) is typed and carries `session_id: String`,
`session_file: Option<String>`, `total_messages: usize`, `cost: f64`, and `tokens: StatsTokens { … }`
all `u64` and always present. So **only `session_file` is conditional**, every other guard collapses,
and **the `JSON.stringify` fallback is unreachable — drop it, do not port a dead branch**. `cost` is an
`f64`: pi-acp prints JS default formatting; pick a Rust rendering deliberately and record it, since
cyrup's TUI uses `${:.3}` in `execute_session.rs`, which is a different string.
*Verify* — as tabled. **Open question `ACP-Q43`**: `SessionStats` additionally carries
`user_messages`, `assistant_messages`, `tool_calls`, `tool_results` and `context_usage`, all of which
the TUI shows and pi-acp's `/session` does not. Extending is additive and cheap; decide rather than
default.

**ACP-285 — `/name`.**
*Upstream* — empty → one chunk `Usage: /name <name>`. Otherwise set, and on success emit **two**
updates in order: `session_info_update` with `title` and a fresh `updatedAt`, then an
`agent_message_chunk` reading `Session name set: ${name}`. Note `args.join(' ')` re-joins the
quote-stripped tokens, so `/name "a  b"` becomes `a b`.
*cyrup* — `session.set_session_name(&name).await` (`crates/cyrup-session-svc/src/session/transcript.rs`);
the version-skew hint is a cut. **The important delta**: `set_session_name` already fans out
`AgentSessionEvent::SessionInfoChanged { name }` to every subscriber **and** dispatches a
`HostEvent::SessionInfoChanged` to extensions — so if the event translator maps that to
`SessionUpdate::SessionInfoUpdate`, emitting one here too sends the client **two**. Decide one owner,
preferably the event translator, so a rename originating from an extension or another front-end reaches
the client identically. And note `SessionInfoUpdate.title` is `MaybeUndefined<String>`, **not
`Option`**: send `MaybeUndefined::Value(name)`, never `Null`, which **clears** the title. `updated_at`
is an ISO-8601 `String`. The RPC precedent (`SessionCommand::SetSessionName`) additionally refuses an
empty name with `"Session name cannot be empty"`.
*Verify* — as tabled: **exactly one** `session_info_update`. **Open question `ACP-Q44`**: cyrup's TUI
`/name` with no argument *shows* the current name rather than printing a usage line; matching the TUI
would be friendlier and diverges from pi-acp.

**ACP-288 — `/export`.**
*Upstream* — never accepts a user-supplied path. Computes `safeSessionId = sessionId.replace(/[^a-zA-Z0-9_-]/g,'_')`
and `outputPath = join(session.cwd, \`pi-session-${safeSessionId}.html\`)`, then emits **two** chunks:
first `{type:'text', text:'Session exported: '}` — trailing space, no newline, and the comment explains
this avoids a duplicate-looking link — then a `resource_link` with `name`, `uri: file://…`,
`mimeType:'text/html'`, `title:'Session exported'`.
*cyrup* — `session.export_to_html(Some(&path)).await -> Result<PathBuf, SessionServiceError>` returns
the resolved path and never an empty one, so the `no output path returned by pi` branch is unreachable
— drop it. Rename the artefact to `cyrup-session-<safeSessionId>.html` to match cyrup's own default,
and keep the sanitising filter as an explicit `is_ascii_alphanumeric() || '_' || '-'` — **see
`ACP-291` for why it is not cosmetic**. The `file://` URI must be built from the absolute path;
`PathBuf` is not UTF-8-guaranteed, so a non-UTF-8 cwd needs an explicit decision.
*Verify* — as tabled. **Open question `ACP-Q45`**: cyrup's TUI `/export` accepts a path argument (and
`.jsonl`); pi-acp refuses one on purpose. Accepting a client-supplied path is security-adjacent —
arbitrary write inside the session cwd versus anywhere — so default to the refusal until asked.

**ACP-290 — The advertised list is in nondeterministic order.**
*Upstream* — pi-acp's list is deterministic: user templates in directory order, then project, then the
builtins, de-duped first-wins.
*cyrup* — **it is not.** `slash_command_catalog` sources its prompt and skill rows from
`self.services.resources.prompts.winners()` / `.skills.winners()`, and `ResourceSet::winners` is
`self.by_key.values()` over a `std::collections::HashMap` (`crates/cyrup-resources/src/discovery/mod.rs`)
— its own doc says *"order unspecified"*. Only the extension arm is ordered (it iterates
`resolved_commands()`, extension-load order). So the `available_commands_update` payload **reorders its
prompt and skill rows on every process start**: the Zed command menu shuffles between launches for no
user-visible reason, and `ACP-267`'s golden test is flaky by construction. Either sort the projection
(by name, or by a stable provenance rank) inside `cyrup-acp` and record the delta, or fix `winners()` —
but the ACP host must not assume the order it is given.
*Verify* — as tabled.

**ACP-291 — `/export` composes a path from client input and writes it unguarded.**
*Upstream* — the `safeSessionId` regex.
*cyrup* — `ACP-288` says to keep the filter, but not what it defends, so it reads as cosmetic and is
exactly the line a Rust port drops as noise. **It is a security control.** `session.sessionId` is not
agent-minted on every path: `session/load` takes the id from the client, and every `session/prompt`
carries it, so an id containing `../` or an absolute-looking segment composes straight into
`join(cwd, …)` — and `PathBuf::join` with an absolute component **replaces** the base, so
`cwd.join("cyrup-session-/etc/x.html")` is `/etc/x.html`. On the cyrup side there is **no second line
of defence**: `AgentSession::export_to_html` (`crates/cyrup-session-svc/src/session/transcript.rs`)
takes the caller's path verbatim and ends in `std::fs::write(&out, html)` — no normalisation, no
containment check against `services.cwd`, no existence check, and no consultation of
`cyrup-permission-system`, unlike the file-write tools in `cyrup-tools`. Note the default-path branch
is exposed too: it falls back to `self.session_id().as_str()` when `session_file()` is `None`.
**Why `critical`** — data loss: a dropped sanitiser is an arbitrary file write driven by client input,
which silently destroys whatever was at that path. Even a *correct* sanitiser overwrites an existing
`cyrup-session-<id>.html` in the user's project directory — that half is parity (pi-acp overwrote too)
and must be recorded, not treated as the regression.
*cyrup, fix* — ADR-0028's `AcpSessionId::export_path_in(dir)` is the shape: the only constructor of an
export path, re-checking `p.parent() == Some(dir)`, so the containment cannot be simplified away into a
bare `format!`. Validate the wire id at the handler boundary with `cyrup_session::validate_session_id`
before it reaches any path composition.
*Verify* — as tabled; assert by **scanning the temp root**, not by inspecting a return value.
**Open question `ACP-Q46`**: whether a client may `session/load` an id it did not receive from this
connection decides whether this is a boundary check or defence in depth. The corpus does not settle it,
and the type is right either way.

**ACP-292 — Built-ins bypass the turn queue, and `/compact` aborts an in-flight turn.**
*Upstream* — pi-acp queues concurrent prompts, **but the built-in dispatcher sits upstream of that
gate**, so a built-in issued while a turn is streaming executes immediately.
*cyrup* — harmless for six of the eight. **Not for `/compact`**: `AgentSession::compact`
(`crates/cyrup-session-svc/src/session/compaction.rs`) opens with `self.abort_and_settle().await`
before installing its own cancel token, so a client that pipelines `/compact` behind a running prompt
**kills the running turn**, whose own `session/prompt` then resolves `cancelled`. `/autocompact` and
`/name` mutate live session state mid-turn on the same path. `ACP-283` files the strings and the
error discipline but says nothing about *when* the arm may run. Decide explicitly: gate the built-ins
on `AgentSession::is_run_active` and refuse-or-queue, or document the abort as intended. **This is the
same concurrent-`session/prompt` question `ACP-057`/`ACP-079` raise for the handler-must-not-block
rule, and it should be settled once.**
*Verify* — as tabled.

#### Refuted — 4e

| id | struck because | cyrup symbol |
|---|---|---|
| ~~ACP-260~~ prompt-template roots and recursive discovery | both roots and the recursive walk verified present; the unit's own mechanism is "None to write" and its verify is "No new assertion needed". The `.pi/prompts`-vs-`.cyrup/prompts` question survives on the `slash-commands.ts` cut | `cyrup_resources::discovery::blocking` (global `cfg.global_dir.join("prompts")`, project `base.join(".cyrup/prompts")`) + `ResourceSet::build` + `AgentSessionServices::resources` |
| ~~ACP-261~~ frontmatter parsing | present and a strict superset — `serde_yml`, YAML fault → empty map, which is what pi itself does. Nothing to write, nothing to assert | `cyrup_resources::prompt::parse_frontmatter` + `skill::split_front_matter` |
| ~~ACP-262~~ description derivation and the 60-char truncation | identical modulo the UTF-16-vs-scalar delta the unit itself names; that delta is now a `CYRUP-DELTA` line on `ACP-267` | `cyrup_resources::prompt::first_line_description` + `DESCRIPTION_TRUNCATE` |
| ~~ACP-264~~ quote-aware argument tokenizer | same algorithm, same quote semantics, no escapes, unterminated quote absorbs the remainder — verified line by line. The `is_whitespace()`-vs-space/tab delta is already in the Rust doc comment; the only live consumer is `ACP-282`, where it is one call | `cyrup_resources::parse_command_args`, re-exported at the crate root |
| ~~ACP-265~~ `$1..$N` / `$@` substitution | present and a strict superset. The unit's own mechanism reads "None to write, **and nothing to call from cyrup-acp**" — a unit whose entire content is that cyrup-acp must not use it is the invariant (`ACP-266`), not a port unit | `cyrup_resources::substitute_args` |
| ~~ACP-270~~ `describeFallback` | duplicate of `ACP-263` by the unit's own text (`cyrup_mechanism: "Part of ACP-263's helper"`, `verify: "Covered by ACP-263"`). One fact folded forward: `location` is not a key **pi's own `get_commands` emits either**, so upstream's `(prompt:project)` fallback fires only in its own unit test, never against a real pi | `cyrup_tui::commands::{autocomplete_source_tag, prefix_autocomplete_description}` — a different spelling, so `ACP-263` still owns the decision |
| ~~ACP-273~~ read the two settings through the session | both getters verified to carry pi's defaults verbatim (`unwrap_or(true)` / `unwrap_or(false)`), and `deep_merge` verified identical to pi-acp's `deepMerge`. The residue — read it off the session, never a free `fn(cwd)`, never cached on the connection — is already restated inside `ACP-268`, the one unit that consumes the flag; `quiet_startup` has no consumer in this area at all | `cyrup_config::EffectiveSettings::{enable_skill_commands, quiet_startup}` + `settings::merge::deep_merge` |
| ~~ACP-274~~ `skills.enableSkillCommands` back-compat | the migration does exactly what the unit says. **But strike the claim as well as the unit**: it is not "strictly stronger" than pi-acp's read-time fallback, it is differently *ordered* — see `ACP-296` | `cyrup_config::settings::migrate::migrate_settings` step 3 |
| ~~ACP-275~~ agent-directory resolution | present, covers the legacy `PI_CODING_AGENT_DIR` rung, and the unit itself concludes the ACP host does not need to call it at all because the agent dir arrives through `AgentSessionServices` | `cyrup_config::paths::cyrup_agent_dir_from` |

---

## 5. Open questions

Everything a survey or an adversary pass flagged as an inference or as unresolved, with the evidence
that would settle it. **These are not units.** A question that turns out to have a behavioural answer
becomes one; a question that turns out to be a decision is settled by whoever owns the surface and
recorded as a `CYRUP-DELTA` at the site. Nothing here may be guessed.

| id | question | what would settle it | blocks |
|---|---|---|---|
| **ACP-Q1** | `serde_json/preserve_order` is non-optional in both ACP crates and flips `serde_json::Map` to `IndexMap` for the whole binary. Accept the flip, `[patch.crates-io]` a fork, or keep `cyrup-acp` out of the binary? | run `cargo test --workspace` with `agent-client-protocol` as a normal dependency of one member and count the failures; audit the four named ordering-sensitive sites (config persistence, provider request bodies, MCP payloads, session JSONL) | **the crate's existence** |
| ACP-Q2 | Does `--terminal-login` land in the interactive TUI (upstream's assumption that the user types `/login`) or directly in `cyrup_config::login::resolve_login_command`? | a product decision | ACP-001 |
| ACP-Q3 | cyrup has no `--terminal-login` flag and no `login` subcommand — only the TUI `/login`. Is `AuthMethod.args` `[]` (relaunch interactively) or does a new flag land first? | read `crates/cyrup-tui/src/commands.rs`'s `BUILTIN_SLASH_COMMANDS` and decide; an `args` naming a flag that does not exist produces a terminal that exits with a usage error | ACP-011, ACP-013 |
| ACP-Q4 | Does a current Zed set `ClientCapabilities.auth.terminal` in addition to, or instead of, the `_meta["terminal-auth"]` probe? | capture an `initialize` request from a live Zed. Not determinable from the pi-acp tree — no `node_modules/`, no test asserts it | ACP-012, ACP-054 |
| ACP-Q5 | Upstream's `not configured` pattern fires on MCP-server and extension configuration errors. Should cyrup classify a failed MCP server as auth-required? | a product decision; the typed classifier's default answer is no | ACP-015 |
| ACP-Q6 | What wire method name does `@agentclientprotocol/sdk` 0.26 bind `unstable_setSessionModel` to? Only a leading `_` reaches `ExtMethodRequest`. | read the TS SDK, or observe a shipping Zed. `rg 'unstable'` over pi-acp's `src/` and `test/` returns exactly one hit, the declaration | the cut stands until refuted |
| **ACP-Q7** | Modelless `session/new`: refuse with `auth_required` and roll back (parity), or return the session with no `model` config option plus the fallback banner (cyrup-native)? | unanswerable from pi-acp, which had no modelless-session concept. Weigh SEAM-075 against upstream fidelity | ACP-017, ACP-059, ACP-060, ACP-220 |
| ACP-Q8 | Is N-session ever a requirement? It is gated on making the native-extension host-services slot per-session, which `cyrup-acp` does not own | a requirement, plus a change in `cyrup-ext` / `cyrup-permission-system` / `cyrup-mcp` / `cyrup-flux` | ACP-061, ACP-120, and the architecture decision itself |
| ACP-Q9 | `loadSession: true` and `sessionCapabilities.{list,delete}` are promises about surfaces other units own. Gate the capability behind the implementation, or advertise aspirationally? | sequencing — see §8 | ACP-052 |
| ACP-Q10 | `NewSessionRequest.mcp_servers` is accepted and ignored by pi-acp. cyrup has a real MCP tier | a decision with `cyrup-mcp`'s owner | ACP-057 |
| ACP-Q11 | Byte-parity (`Configure an API key or log in with an OAuth provider.`) versus useful (`format_no_api_key_found_message(provider)`, which names the provider and points at `/login`) | a product decision | ACP-058 |
| ACP-Q12 | Is a one-entry mode list (`[off]`) acceptable to Zed, or should the whole `modes`/`thought_level` surface be omitted when `supports_thinking()` is false? | test against a live Zed | ACP-062 |
| ACP-Q13 | cyrup has more selectable session state than pi (`scoped_models`, auto-compaction, steering/follow-up modes) and 1.7.0 has `SessionConfigKind::Boolean` | a decision; file as an enhancement, not a port unit | ACP-064 |
| ACP-Q14 | Does any shipping Zed read `NewSessionResponse.models`? If none does, dropping it beats an `_meta` shim nobody consumes | observe a client | ACP-065 |
| ACP-Q15 | `build_startup_report` lives in the `cyrup` **bin** crate and takes `&AgentSession`. Move it, duplicate it, or depend on `cyrup-tui`? | a crate-layout decision, coupled to ACP-Q30 | ACP-066 |
| ACP-Q16 | The prelude is carried in both `_meta.piAcp.startupInfo` and a chunk; a client rendering both shows it twice. pi-acp accepted that | a decision | ACP-068 |
| ACP-Q17 | ACP-069 ports upstream's `includeExtensionCommands: false` and ACP-269 reverses it. They must agree | settle ACP-269 first; ACP-069 then follows | ACP-069, ACP-269 |
| ACP-Q18 | Derive the eight built-ins from `cyrup_tui::BUILTIN_SLASH_COMMANDS` filtered to the headless-safe subset, or hand-write them? Derivation prevents drift and changes the strings; hand-writing preserves them and guarantees drift | a decision, with the `changelog`/`export` dispatch gap (ACP-070) resolved first | ACP-070, ACP-272 |
| ACP-Q19 | Reject an unsupported-but-well-formed thinking level with −32602, or accept and clamp? | clamping matches `set_thinking_level`'s own contract and is the only safe answer if the mode list is model-derived, because then an unsupported level is never advertised | ACP-062, ACP-072 |
| **ACP-Q20** | If the setters push directly *and* the event pump pushes, every ACP-originated change emits two identical `config_option_update`s. The pump must be the single emitter — which means the setters must not notify, diverging from ACP-072/073's pinned counts | a decision, taken once for `config_option_update`, `current_mode_update` **and** `session_info_update` (ACP-285 has the same shape) | ACP-072, ACP-073, ACP-075, ACP-077, ACP-285 |
| ACP-Q21 | `session/new` on an already-live session: evict (upstream) or error? | with one live session, eviction is structural; the observable difference is only whether the old session's in-flight prompt gets a response — which ACP-154 answers | ACP-061, ACP-120 |
| ACP-Q22 | Do Zed 0.26-era clients render `session_info_update` `_meta` at all? Upstream's own comments say no | observe a client. If not, ACP-124 option (b)'s richer payload is free | ACP-124 |
| ACP-Q23 | Emit a `tool_call_update` on **every** streaming delta (upstream) or only on Start/End? In-process, per-delta forces `LazyArgs` materialisation and defeats the laziness | a performance decision; Zed sees the same final state either way | ACP-128 |
| ACP-Q24 | Should the file snapshot use ACP's `fs/readTextFile` client capability when advertised — the only way to diff against a buffer the user edited but did not save? Upstream ignores the capability | a decision, after ACP-156 settles where the read happens at all | ACP-131, ACP-135, ACP-156 |
| **ACP-Q25** | Does Zed still honour the `terminal_info`/`terminal_output`/`terminal_exit` `_meta` convention, or is the typed `terminal/*` client family (ungated in 1.7.0) now the supported route? | observe a client. **If the typed family works, ACP-139/140/141 are largely a cut** — do not build ceremony on the `_meta` protocol before answering this | ACP-139, ACP-140, ACP-141, ACP-216 |
| ACP-Q26 | Stream the terminal delta from the tool's `OutputAccumulator` (untruncated tail) instead of the truncated `ToolUpdate.content` preview? That makes the prefix invariant true by construction | a new seam between `cyrup-tools` and `cyrup-acp`; weigh against ACP-140's desync policy | ACP-140 |
| ACP-Q27 | Should `Command aborted` / `Command timed out` map to an exit code at all, or to `terminal_exit.signal`? cyrup can distinguish `ExitStatus::{Killed, TimedOut, Signaled}` and knows more than pi did | a decision, alongside ACP-141 option (c) | ACP-141 |
| ACP-Q28 | Is `elicitation/create` with titled `EnumOption`s a better carrier for `UiKind::Select` than `session/request_permission`? It carries per-option descriptions, at the cost of the client's permission-prompt affordance | a decision — and note ACP-145 makes Select the tool-permission dialog, so this is a permission-UX decision, not a cosmetic one | ACP-145, ACP-147 |
| ACP-Q29 | Does Zed answer `elicitation/create`? The Architecture phase places it in the schema, not in an observed client | observe a client | ACP-147 |
| **ACP-Q30** | `session_list_layout`, `session_list_cwd_filter`, `gather_session_refs` and `list_global_sessions` are `pub(crate)` in the `cyrup` **bin** crate, which will depend on `cyrup-acp`. Lift them into `cyrup-session`, or put the ACP mode inside `crates/cyrup`? | a crate-layout decision, coupled to ACP-Q15 | ACP-200, ACP-201, ACP-207, ACP-223 |
| ACP-Q31 | Under an explicit `--session-dir` the cwd-encoded directory does not exist and `list_in_dir` returns empty. Flat scan plus cwd filter, or one level of descent? | **ACP-223 inverts this**: the flat prescription loses nested files that upstream's own fixture asserts are found. `AgentSession::list_sessions` already implements a branch | ACP-207, ACP-223 |
| ACP-Q32 | An offset cursor over a per-request re-scan is unstable — a session touched between pages re-sorts and the client skips one row and sees another twice. pi-acp accepted it ("For MVP"). Encode `(updated_at, session_id)` instead? | the ACP field is explicitly opaque, so nothing on the wire forces the offset form | ACP-208 |
| ACP-Q33 | Where does the single-flight guard live? In the ACP host it leaves the same race open for `cyrup-tui`'s `/resume` and for `switch_session` generally; inside `switch_session_with` it fixes every caller but changes a shared type's concurrency contract | confirm with whoever owns `crates/cyrup-session-svc/src/runtime.rs`, and check the TUI's re-entrancy | ACP-209 |
| ACP-Q34 | Replay `ToolKind`: pi-acp's three names, or 1.7.0's ten variants? | must match whatever the live translation (ACP-151) chooses, or a replayed call renders differently from the same call live | ACP-151, ACP-215 |
| ACP-Q35 | Should the `session/load` replay be emitted synchronously from the handler? 10 k entries means 10 k `send_notification`s onto an mpsc before the handler returns — fast but unbounded in memory | bounding it means moving the replay into `cx.spawn`, which then needs its own ordering guarantee against the response | ACP-217 |
| ACP-Q36 | `delete_session_file_at` returns `Ok(DeleteMethod::Trash)` whenever the file merely no longer exists. Surface `DeleteMethod` in the response `_meta`? | right for an ACP delete, opaque for an audit trail | ACP-218 |
| ACP-Q37 | Disposing the runtime as a side effect of `session/delete` is stronger than ACP implies, and `session_cancel` kills tracked bash children — deleting a background session mid-turn kills the turn. Intended? | a decision, taken together with ACP-224, which records that pi-acp leaves the session live | ACP-219, ACP-224 |
| ACP-Q38 | Keep pi-acp's nameless-row guard as a cheap invariant, or drop it? cyrup's catalog cannot emit one | a decision | ACP-267 |
| ACP-Q39 | An extension command opening a `UiKind::Editor` dialog has no faithful ACP rendering. Does that warrant a per-command capability filter? | after ACP-147 settles what `Editor` degrades to | ACP-269 |
| ACP-Q40 | cyrup's TUI has no `/steering`, `/follow-up` or `/autocompact` builtin. Advertising them over ACP but not in the TUI means the two front-ends disagree about what commands exist | a decision, coupled to ACP-Q18 | ACP-272 |
| ACP-Q41 | Should an ACP built-in shadow a user prompt template of the same name? A user with a `/session` template loses it under ACP and keeps it in the TUI | a decision; pi-acp shadows, and is internally inconsistent about it (ACP-272) | ACP-282 |
| ACP-Q42 | `/compact` emits its own summary chunk while `CompactionStart`/`CompactionEnd` also project to `session/update`. The client would see both | a decision, taken once with ACP-143 | ACP-143, ACP-283 |
| ACP-Q43 | `SessionStats` carries `user_messages`, `assistant_messages`, `tool_calls`, `tool_results` and `context_usage` that pi-acp's `/session` does not show. Extend? Also: how is `cost: f64` rendered — cyrup's TUI uses `${:.3}` | a decision; extending is additive and cheap | ACP-284 |
| ACP-Q44 | cyrup's TUI `/name` with no argument **shows** the current name; pi-acp prints a usage line | a decision | ACP-285 |
| ACP-Q45 | cyrup's TUI `/export` accepts a path argument; pi-acp refuses one on purpose. Accepting a client-supplied path is arbitrary-write-inside-cwd versus arbitrary-write-anywhere | security-adjacent; default to the refusal until asked, and note ACP-291 | ACP-288, ACP-291 |
| ACP-Q46 | May an ACP client `session/load` an id it did not receive from this connection? | decides whether ACP-291's containment is a boundary check or defence in depth. The type is right either way | ACP-291 |

---

## 6. Open items

**This table is the authority for what is open in area 15.** It carries every unit, ranked by
severity then id, and nothing else in this repository speaks for it. Struck units are **not** here —
they are in each area's `Refuted` subsection, which is where a re-file must start.

`verification` is `verified` where an adversary pass completed for that area **and** examined that
unit, and `single-source` where only one pass has seen it. All five areas had an adversary complete,
so **no area is wholly single-source**; the 36 `single-source` rows are the units the adversary passes
*filed*, which by construction the survey never checked. Treat those as leads with citations rather
than as confirmed findings, and re-read both sides before scheduling one.

**Severity corrections applied while writing this document, each named so the change is auditable:**

| id | from | to | why |
|---|---|---|---|
| ACP-005 | critical | high | the data-loss clause does not hold — `flush_session_writes`'s own doc says *"no process-exit path can lose them"*. The residual (missed `SessionShutdown`, orphaned bash process groups) is a resource leak |
| ACP-004 | high | medium | the guard is genuinely needed, but the harm is a non-zero exit and one stderr line at a moment when the client has already stopped reading; dispose runs first, so nothing is lost |
| ACP-063 | high | medium | the `'default'` sentinel the rating rested on is **unreachable** — upstream's null guard runs before the `?? 'default'` fallback, so both sites are dead code |
| ACP-068 | high | medium | the correct implementation is the default one; the deliverable is a doc comment plus a wire-order assertion, and the regression drops one informational chunk whose text is also on the response. `high` read as blocking-ness |
| ACP-070 | low | medium | two of the eight advertised built-ins have no dispatch path as specified (`ExportHtml` is a variant of the *other* `SessionCommand`; `changelog` has no verb outside the TUI), making the unit's own verify unsatisfiable and putting dead rows in the client's palette |
| ACP-129 | high | medium | the ordering hazard is unreachable in cyrup — `stream_assistant()` completes before `execute_tool_calls` in the same loop body. The residual is a duplicate tool row, a rendering defect |
| ACP-141 | high | medium | the exit code is not lost to the **user**: the error body including `Command exited with code 42` reaches the client as terminal output, and `is_error` already sets the tool status to `failed`. Degraded metadata |
| **ACP-145** | medium | **critical** | `LocalAskChannel::confirm` reaches the human through `HostServices::select`, not `confirm`, and decides the grant by exact string match on four option labels — so `UiKind::Select` **is** the tool-permission dialog, and a wrong option round-trip is a real `Once`/`Always` grant. Permission bypass |
| **ACP-219** | high | **critical** | calling `delete_session_file_at` on the live session without disposing first leaves the held `O_APPEND` fd appending every subsequent turn to an unlinked inode nothing can reach. Silent data loss on the ordinary act of deleting the session you are in |
| ACP-206 | medium | low | the prescribed cwd filter is a **divergence** from upstream, not parity — pi-acp emits `cwd:""` and relative cwds verbatim. No clause is met; the worst case is a row the client cannot open, which is already upstream's behaviour |
| ACP-266 | critical | low | the invoked mechanism is impossible: `expand_prompt_template` returns its input unchanged unless it starts with `/` **and** the leading token matches a template name, and `substitute_args` only ever runs on a matched body — so no second pass over expanded text exists. Its proposed test also passed with the bug present, and has been replaced |
| ACP-281 | low | medium | `PromptRequest.prompt` carries no `VecSkipError` and `ContentBlock` has no `#[serde(other)]`, so an unknown block **rejects the entire turn** where pi-acp dropped one and continued. A real client-visible divergence, not a free win |

**ACP-146 keeps `critical` on corrected evidence**, not on the survey's: its consumer is not
`LocalAskChannel` (see ACP-145) but the MCP owner fence (`crates/cyrup-mcp/src/owner.rs`), subagent
authority routing (`crates/cyrup-ext-subagents/src/extension/tool/routing.rs`) and any WASM guest
calling `ui.confirm`. **ACP-291 is filed at `critical` on the data-loss clause**, and it is the only
critical this document adds that no survey rated at all.

Every remaining `critical` was re-checked against the four clauses: ACP-057 (crash on a normal path —
an `Err` out of `cx.spawn` tears down the connection on an ordinary failed `session/new`), ACP-121
(silent wrong output — a truncated answer rendered as complete), ACP-209 (silent wrong output plus an
orphan user turn on disk). **7 of 150 is 4.7% critical**, which is where the house scale expects it.

| sev | id | title | eff | area | verification |
|---|---|---|---|---|---|
| critical | ACP-057 | Build the session off the dispatch loop, and never propagate `Err` | M | 4b | verified |
| critical | ACP-121 | A prompt resolves only on `agent_settled` | M | 4c | verified |
| critical | ACP-145 | Select: option ids and the strict round-trip | S | 4c | verified |
| critical | ACP-146 | Confirm: the two fixed options and the cancelled outcome | S | 4c | verified |
| critical | ACP-209 | Single-flight restore | M | 4d | verified |
| critical | ACP-219 | `session/delete` of the session that is currently live | S | 4d | verified |
| critical | ACP-291 | `/export` composes a path from client input and writes it unguarded | S | 4e | single-source |
| high | ACP-001 | `--terminal-login` argv gate, classified before clap | S | 4a | verified |
| high | ACP-002 | `AppMode::Acp` and the `--acp` / `--mode acp` surface | S | 4a | verified |
| high | ACP-003 | Stdio transport bootstrap and `run_acp_dispatch` | M | 4a | verified |
| high | ACP-005 | Stdin EOF and close terminate the process, and dispose first | S | 4a | verified |
| high | ACP-015 | `maybeAuthRequiredError` rebuilt as a typed classifier | M | 4a | verified |
| high | ACP-021 | The ACP arm must not inherit `require_model: true` | S | 4a | single-source |
| high | ACP-022 | A mid-turn provider 401/403 is not an `Err` — classify at the settle boundary | M | 4a | single-source |
| high | ACP-056 | `session/new` rejects a non-absolute `cwd` | XS | 4b | verified |
| high | ACP-059 | Zero available models means unauthenticated | S | 4b | verified |
| high | ACP-060 | The destructive rollback on a normal error path | S | 4b | verified |
| high | ACP-061 | One live session per connection | M | 4b | verified |
| high | ACP-072 | `session/set_mode` sets the thinking level and echoes the applied one | S | 4b | verified |
| high | ACP-122 | `lastEmit` ordering: the response never overtakes a notification | S | 4c | verified |
| high | ACP-123 | `cancelRequested` and the `StopReason` mapping | S | 4c | verified |
| high | ACP-126 | The prompt-failure path: flush, auth detection, queue clearing | S | 4c | verified |
| high | ACP-135 | `tool_execution_end`: the structured diff, and diff-suppresses-`rawOutput` | M | 4c | verified |
| high | ACP-140 | `bashOutputDelta`: the append-only terminal delta | M | 4c | verified |
| high | ACP-144 | `extension_ui_request` dispatch and the catch that always answers | M | 4c | verified |
| high | ACP-150 | `requestExtensionPermission`: the catch that cancels when the client rejects | S | 4c | verified |
| high | ACP-153 | Use `prompt`'s run-scoped stream, not `subscribe()` | M | 4c | single-source |
| high | ACP-154 | `SessionReplaced` ends the stream with no settle | S | 4c | single-source |
| high | ACP-155 | Never await a client round trip on the event pump | M | 4c | single-source |
| high | ACP-156 | The end-of-tool re-read has no `FsOps` handle | M | 4c | single-source |
| high | ACP-221 | `restoreSession`'s failure mapping, and the `cx.spawn` trap | S | 4d | single-source |
| high | ACP-222 | Filename-derived session ids are ambiguous | M | 4d | single-source |
| medium | ACP-004 | A failed stdout write is a clean exit, not an error | S | 4a | verified |
| medium | ACP-010 | The `cyrup_terminal_login` AuthMethod identity and its three strings | XS | 4a | verified |
| medium | ACP-011 | Registry `type`/`args`/`env` → typed `AuthMethod::Terminal` | S | 4a | verified |
| medium | ACP-012 | Zed's `_meta["terminal-auth"]` compat shape, gated on the client's probe | S | 4a | verified |
| medium | ACP-013 | The terminal-auth launch spec must name this executable | XS | 4a | verified |
| medium | ACP-014 | `authenticate` is a successful no-op | XS | 4a | verified |
| medium | ACP-016 | The `AUTH_REQUIRED` payload: data shape and message string | S | 4a | verified |
| medium | ACP-017 | Zero available models is treated as unauthenticated | M | 4a | verified |
| medium | ACP-023 | `spawn_abort_on_signal` needs a runtime the lazy build does not have yet | S | 4a | single-source |
| medium | ACP-050 | `initialize` clamps the requested protocol version | XS | 4b | verified |
| medium | ACP-052 | The four advertised capability blocks | S | 4b | verified |
| medium | ACP-054 | `authMethods` and the conditional `_meta` shim, from `initialize` | S | 4b | verified |
| medium | ACP-058 | The auth-required / internal-error paths of `session/new` | S | 4b | verified |
| medium | ACP-062 | The ACP mode list is the thinking-level ladder | S | 4b | verified |
| medium | ACP-063 | The advertised model list and current selection | S | 4b | verified |
| medium | ACP-064 | The two config options and their order | S | 4b | verified |
| medium | ACP-065 | `NewSessionResponse` has no `models` field | XS | 4b | verified |
| medium | ACP-066 | The markdown startup prelude | M | 4b | verified |
| medium | ACP-068 | The prelude is delivered after the `session/new` response | S | 4b | verified |
| medium | ACP-069 | `available_commands_update` is also deferred past the response | S | 4b | verified |
| medium | ACP-070 | The eight headless built-ins | S | 4b | verified |
| medium | ACP-073 | `session/set_config_option` routes `model` and `thought_level` | S | 4b | verified |
| medium | ACP-075 | `emitConfigOptionsUpdate` re-derives the whole option set | S | 4b | verified |
| medium | ACP-077 | Push config/mode updates on session-originated changes | S | 4b | verified |
| medium | ACP-078 | The `Unknown sessionId` gate every setter opens with | S | 4b | single-source |
| medium | ACP-079 | The setters do real blocking work and must leave the dispatch loop | S | 4b | single-source |
| medium | ACP-120 | SessionManager: registry, lookup error string, one-live-session collapse | S | 4c | verified |
| medium | ACP-124 | The turn queue and the `_meta` queue-depth publication | M | 4c | verified |
| medium | ACP-127 | `message_update`: text and thinking deltas to chunks | XS | 4c | verified |
| medium | ACP-128 | Early tool-call surfacing from streaming deltas | M | 4c | verified |
| medium | ACP-129 | Monotonic tool-call status and the first-vs-update decision | S | 4c | verified |
| medium | ACP-131 | `tool_execution_start` (non-bash): snapshot capture and the transition emit | M | 4c | verified |
| medium | ACP-136 | `toolResultToText`: the diff → content → stdout → JSON ladder | S | 4c | verified |
| medium | ACP-139 | `emitBashToolCall` and the `terminal_info` `_meta` protocol | S | 4c | verified |
| medium | ACP-141 | `bashExitCode` and the `terminal_exit` `_meta` | S | 4c | verified |
| medium | ACP-143 | `auto_compaction_start` / `_end` status chunks | XS | 4c | verified |
| medium | ACP-147 | Input and editor: cancellation with a visible fallback message | M | 4c | verified |
| medium | ACP-157 | `powershell` is a second built-in shell tool | S | 4c | single-source |
| medium | ACP-158 | Prompt images and non-text content blocks reach the queued turn | S | 4c | single-source |
| medium | ACP-202 | Cross-project lookup that reads no session bodies | S | 4d | verified |
| medium | ACP-205 | Title fallback chain, and the `(no messages)` sentinel | XS | 4d | verified |
| medium | ACP-207 | `listSessions` defaults its cwd filter to `lastSessionCwd` | XS | 4d | verified |
| medium | ACP-208 | Numeric-offset opaque cursor, page size 50 | S | 4d | verified |
| medium | ACP-213 | `AppMode::Acp` must persist sessions | XS | 4d | verified |
| medium | ACP-215 | Replay: synthetic completed tool-call pairs | M | 4d | verified |
| medium | ACP-217 | Replay precedes the response; command advertisement follows it | S | 4d | verified |
| medium | ACP-218 | `session/delete` is idempotent and deletes the file | S | 4d | verified |
| medium | ACP-220 | Remove the session file for a `session/new` that never returned an id | S | 4d | verified |
| medium | ACP-223 | Recursion is load-bearing under a settings-derived `sessionDir` | S | 4d | single-source |
| medium | ACP-224 | `deleteSession` leaves the ACP session live and usable | S | 4d | single-source |
| medium | ACP-225 | The live-session short-circuit and the forced rebuild are in tension | S | 4d | single-source |
| medium | ACP-267 | Project `slash_command_catalog()` rows into `AvailableCommand`s | S | 4e | verified |
| medium | ACP-269 | Reverse pi-acp's `source === 'extension'` exclusion | S | 4e | verified |
| medium | ACP-272 | Built-in advertisement list and merge ordering | S | 4e | verified |
| medium | ACP-276 | `promptToPiMessage`: text concatenation | XS | 4e | verified |
| medium | ACP-277 | `resource_link` → `\n[Context] <uri>` | XS | 4e | verified |
| medium | ACP-278 | `image` → a base64 content block with no data-url prefix | XS | 4e | verified |
| medium | ACP-279 | `resource` → `[Embedded Context]` in three shapes | S | 4e | verified |
| medium | ACP-280 | `audio` → an explicit not-supported marker | XS | 4e | verified |
| medium | ACP-281 | Unknown content blocks reject the turn where pi-acp dropped the block | XS | 4e | verified |
| medium | ACP-282 | The built-in dispatch gate and argument split | S | 4e | verified |
| medium | ACP-283 | `/compact` | S | 4e | verified |
| medium | ACP-284 | `/session` | S | 4e | verified |
| medium | ACP-285 | `/name` | S | 4e | verified |
| medium | ACP-288 | `/export` | M | 4e | verified |
| medium | ACP-290 | The advertised list is in nondeterministic order | S | 4e | single-source |
| medium | ACP-292 | Built-ins bypass the turn queue, and `/compact` aborts an in-flight turn | S | 4e | single-source |
| low | ACP-006 | SIGINT and SIGTERM shut the ACP host down | XS | 4a | verified |
| low | ACP-018 | The ACP host disables theme discovery | XS | 4a | verified |
| low | ACP-024 | The stdin read-failure path is the unfiled half of ACP-004 | XS | 4a | single-source |
| low | ACP-025 | `ext_mode` telling extensions the ACP host is `rpc` is a wire-visible decision | XS | 4a | single-source |
| low | ACP-026 | `--terminal-login` must not bypass the TTY guard | S | 4a | single-source |
| low | ACP-051 | `agentInfo` name / title / version | XS | 4b | verified |
| low | ACP-053 | `promptCapabilities.embeddedContext` behind an env opt-in | XS | 4b | verified |
| low | ACP-055 | `authenticate` answers success | XS | 4b | verified |
| low | ACP-071 | `mergeCommands` — first-wins, order preserved | XS | 4b | verified |
| low | ACP-080 | An undescribed command's description is defined upstream | XS | 4b | single-source |
| low | ACP-081 | `buildStartupInfo` can never return an empty string | XS | 4b | single-source |
| low | ACP-082 | `lastSessionCwd` is connection-scoped state `session/new` writes | XS | 4b | single-source |
| low | ACP-125 | Startup-info deferral: set / send-if-pending | XS | 4c | verified |
| low | ACP-130 | `toToolCallLocations`: path probing and cwd resolution | XS | 4c | verified |
| low | ACP-132 | `findUniqueLineNumber`: unique-oldText line inference | XS | 4c | verified |
| low | ACP-133 | `getParsedEdits` / `getEditOldTexts`: current and legacy edit schemas | S | 4c | verified |
| low | ACP-134 | `tool_execution_update`: partial output and file-mutation suppression | S | 4c | verified |
| low | ACP-137 | `cleanupToolCall`: teardown at tool end | XS | 4c | verified |
| low | ACP-138 | `isBashTool` and `bashCommand`: the tool-call title | XS | 4c | verified |
| low | ACP-142 | `auto_retry_start` / `_end` status chunks and their exact strings | XS | 4c | verified |
| low | ACP-148 | Notify: chat chunk with a severity `_meta` | S | 4c | verified |
| low | ACP-149 | The synthetic dialog tool call carrying the request | XS | 4c | verified |
| low | ACP-151 | `toToolKind`: the tool-name → `ToolKind` map | XS | 4c | verified |
| low | ACP-159 | `cancel()` resolves queued turns without flushing | XS | 4c | single-source |
| low | ACP-160 | `inAgentLoop` is write-only dead state and must not be ported | XS | 4c | single-source |
| low | ACP-200 | Sessions-directory resolution | XS | 4d | verified |
| low | ACP-201 | sessionId → (file, cwd) resolution, local then cross-project | S | 4d | verified |
| low | ACP-203 | Listing scan: header, title, updatedAt, ordering | XS | 4d | verified |
| low | ACP-204 | `updatedAt` is a JS-`toISOString`-compatible string | XS | 4d | verified |
| low | ACP-206 | ACP `SessionInfo.cwd` is a required absolute path | XS | 4d | verified |
| low | ACP-210 | Unknown sessionId is `-32602` with the exact text | XS | 4d | verified |
| low | ACP-211 | `cwd` must be absolute on `session/new` and `session/load` | XS | 4d | verified |
| low | ACP-212 | `session/load` tears down the live session before restoring | S | 4d | verified |
| low | ACP-214 | Replay: user and assistant text | M | 4d | verified |
| low | ACP-216 | Replay: the bash terminal variant | S | 4d | verified |
| low | ACP-226 | `loadSession`'s teardown and `lastSessionCwd` write precede validation | XS | 4d | single-source |
| low | ACP-227 | A leading blank line makes a session invisible upstream | XS | 4d | single-source |
| low | ACP-228 | An explicit `session_info` clear erases the title in cyrup | XS | 4d | single-source |
| low | ACP-229 | A relative `sessionDir` anchors to the agent dir upstream | XS | 4d | single-source |
| low | ACP-230 | Symlink handling is the reverse of the cut's stated rationale | XS | 4d | single-source |
| low | ACP-263 | Provenance in the advertised description | S | 4e | verified |
| low | ACP-266 | cyrup-acp must not expand prompt templates | XS | 4e | verified |
| low | ACP-268 | `skill:` gating on `enableSkillCommands` | XS | 4e | verified |
| low | ACP-271 | Carry `argumentHint` into `AvailableCommandInput` | XS | 4e | verified |
| low | ACP-286 | `/steering` | S | 4e | verified |
| low | ACP-287 | `/follow-up` | XS | 4e | verified |
| low | ACP-289 | `/autocompact` | S | 4e | verified |
| low | ACP-293 | `available_commands_update` is emitted from two call sites | XS | 4e | single-source |
| low | ACP-294 | `promptCapabilities.embeddedContext` has no cyrup env name | XS | 4e | single-source |
| low | ACP-295 | pi-acp's file-command precedence is inverted vs pi and vs cyrup | XS | 4e | single-source |
| low | ACP-296 | Legacy `skills.enableSkillCommands` resolves to a different layer | XS | 4e | single-source |

---

## 7. Test architecture

Read [`../TEST-ARCHITECTURE.md`](../TEST-ARCHITECTURE.md) first; this section only says where
`cyrup-acp`'s tests land under its rules, which are not re-litigated here.

### The split

**`crates/cyrup-acp/src/` under `#[cfg(test)]` holds the overwhelming majority**, and that is not an
accident of taste — it falls out of the port's own shape. The pure functions this document files are
the port: the protocol-version clamp (`ACP-050`), `prompt_to_user_input` and its five goldens
(`ACP-276`…`ACP-280`), the markdown startup renderer over a fixture `StartupReport` (`ACP-066`), the
capability and config-option builders (`ACP-052`, `ACP-062`, `ACP-064`), the `AvailableCommand`
projection (`ACP-267`), `TerminalAppender` (`ACP-140`), `FileSnapshot::into_diff` (`ACP-135`),
`Settle::observe` (`ACP-121`), `AcpSessionId::{try_from, export_path_in}` (`ACP-291`),
`DialogOptions::resolve` (`ACP-145`, `ACP-146`), `findUniqueLineNumber` (`ACP-132`),
`toolResultToText` (`ACP-136`), the auth classifier (`ACP-015`), and the `AcpBuiltin` name-set
round-trip (`ACP-272`). None of these needs a process, a socket, a guest or a binary. **Where a unit's
`verify` line names a golden string, that golden belongs in `src/` too** — a byte-for-byte assertion
on `'\n[Embedded Context] file:///tmp/a.txt (text/plain)\nhi'` is a unit test in every sense.

**`crates/cyrup-it/` takes only what crosses a seam**, and `cyrup-acp` crosses exactly two of the four
that earn a place there.

| seam | target | what lands there |
|---|---|---|
| the `cyrup` binary's argv, exit codes and stdio | **`cli`** (existing) | every raw-NDJSON test: `ACP-003`'s `initialize` round trip, `ACP-004`'s broken-pipe exit, `ACP-005`'s stdin-EOF dispose, `ACP-021`'s credential-less launch answering on stdout, `ACP-023`'s SIGTERM before any `session/new`, `ACP-026`'s TTY guard, `ACP-001`'s zero-frames assertion, and every **frame-order** assertion (`ACP-068`, `ACP-069`, `ACP-217`, `ACP-122`, `ACP-123`) |
| a live WASM guest | **`wasm`** (existing, `required-features = ["it", "wasm-host"]`) | the dialog round trips: `ACP-144`, `ACP-145`, `ACP-146`, `ACP-147`, `ACP-150`, `ACP-155`'s deadlock case, and `ACP-269`'s extension-command dispatch |
| multi-crate `AgentSession` assembly | **`harness`** (existing) | `ACP-057`'s dispatch-loop and connection-survival pair, `ACP-061`'s eviction, `ACP-209`'s counting-factory single-flight, `ACP-219`/`ACP-224`'s live-delete pair, `ACP-213`'s persist round trip, `ACP-215`'s replay stability across two loads |

**No eighth `[[test]]` target is needed, and this is worth stating because the temptation exists.** An
"ACP protocol" target looks natural and is not justified: §9.1 accepts exactly two justifications —
a crate-level `#![cfg(...)]` the rest of the suite must not get, or process isolation because the
target aborts, panics on unwind, installs a global handler, or mutates process-global state. Every ACP
protocol test either drives `cyrup --acp` as a child (the `cli` seam, which already spawns) or
assembles an `AgentSession` in process (the `harness` seam). `ACP-023`'s signal test is the closest
call — it sends a real signal — but `cli` already owns `signal_shutdown`, so it belongs there.

**The rule cuts both ways.** A `#[cfg(test)]` module inside `crates/cyrup-acp/src/` that spawns
`cyrup --acp` is just as misfiled as a `tests/` directory, and harder to see — `CARGO_BIN_EXE_<name>`
is never set for a library's own unit tests, so such a test can only find the binary by accident of a
sibling target having linked it. `crates/cyrup-acp/tests/` stays empty.

**Isolation rules that bite this crate specifically.** R2 (no `env::set_var`) is why `ACP-053` and
`ACP-294` must take the environment value as an **argument** to a pure predicate rather than reading
it inside the test — the crate should read it once at handler construction and thread it as data, and
the workspace's `clippy.toml` `disallowed-methods` guard fails the build otherwise. R1 (tempdir per
test) covers every session-file assertion in `ACP-060`, `ACP-209`, `ACP-218`, `ACP-219`, `ACP-220` and
`ACP-291` — and `ACP-291`'s assertion is specifically a **scan of the temp root**, not an inspection
of a return value, because the whole point is that a write may have landed somewhere the return value
does not mention. R4 (no fixed ports) does not apply: ACP is stdio, not a socket.

### pi-acp's own tests are a specification asset

The upstream suite is **33 files under `test/` (3 265 lines) plus 10 `scripts/smoke-*.mjs`**, and it
is the closest thing this port has to an executable specification. Treat it as a source, not as a
suite to translate: several cases pin behaviour that is cut, and at least three pin behaviour that is
**wrong** and must not be reproduced.

**Port as unit tests, case for case** — these are named in the unit bodies above and each has an
owning unit: `prompt-to-pi-message.test.ts` (`ACP-276`…`ACP-280`, byte-for-byte),
`pi-tools.test.ts` (`ACP-136`), `session-config-options.test.ts` (`ACP-064`, `ACP-073` — as a
**subset** golden, see below), `builtin-commands.test.ts` (`ACP-070`), `merge-commands.test.ts`
(`ACP-071`), `auth-methods-terminal-auth-meta.test.ts` (`ACP-010`, `ACP-012`),
`pi-enable-embed-context-flag.test.ts` (`ACP-053`), `pi-commands.test.ts` (`ACP-267`, `ACP-263`),
`startup-info-project-packages.test.ts` and `startup-info-env.test.ts` (`ACP-066`, and `ACP-081` owns
the `timeouts.length === 1` leg), `session-delete.test.ts`'s four cases (`ACP-218`).

**Port as `cyrup-it` cases**: `session-events.test.ts`'s retry sequence is `ACP-121`'s verify almost
verbatim and is the single most valuable file in the upstream suite — it drives
`agent_start / auto_retry_start / agent_end{willRetry:true} / agent_start / turn_end /
agent_end{willRetry:false}` and asserts the promise is **still unresolved**.
`session-queue-cancel.test.ts` is `ACP-123` + `ACP-124`; `session-load-toolresult.test.ts` is
`ACP-215`; `session-list-scoped.test.ts` is `ACP-207`; `session-list-custom-session-dir.test.ts` is
`ACP-223` and is the fixture that **refutes** the "two levels is enough" reasoning;
`session-updatedAt-message-only.test.ts` and `session-title-long-session.test.ts` belong in
`crates/cyrup-session`'s own `src/` as listing tests (`ACP-203`); `session-diff.test.ts` is
`ACP-135`'s three properties; `session-thinking-modes.test.ts` and
`agent-steering-followup-modes.test.ts` are `ACP-062`/`ACP-072` and `ACP-286`/`ACP-287`;
`session-list-and-load.test.ts` spans `ACP-201`/`ACP-208`/`ACP-214`.

**Do not port** — each for a stated reason: `pi-command.test.ts` and `new-session-pi-not-found.test.ts`
pin `command.ts` and the spawn diagnostics, both wholly cut; `slash-commands.test.ts` pins the 197-line
template engine that is cut; `pi-messages.test.ts` pins functions cyrup already has, so its cases
become coverage of `extract_full_content` / `join_text` in `crates/cyrup-session-svc` rather than new
tests here; the spawn half of `new-session-runtime-startup-errors.test.ts` is cut and only its
auth/internal-error half survives as `ACP-058`; `session-events.test.ts`'s `Retrying...` fallback case
must **not** be ported, because `AutoRetryStart`'s fields are non-optional and the assertion would pass
by construction. `stdout-destroyed-does-not-crash.test.ts` is the exception that inverts: its
*behaviour* is `ACP-004` and belongs in `cli`, but its *mechanism* (destroying a Node stream) has no
analogue — close the read end of the pipe instead.

**Three upstream fixtures are unsatisfiable as written and must be rewritten as subset assertions
before anyone schedules them**, or each will burn a session: `session-config-options.test.ts`'s
`deepEqual` cannot pass because `SessionMode` and `SessionConfigSelectOption` are
`#[skip_serializing_none]` and there is no way to emit the explicit `description: null` it asserts;
any byte-for-byte `agentCapabilities` fixture cannot pass because `AgentCapabilities.auth` is a
required field that always emits `"auth":{}`; and `pickFallbackTitleFromHead`'s `title === null` for a
large unnamed session is upstream's loop-invariant bug, so a cyrup port produces a title and the
ported assertion fails **correctly**.

**The 10 `scripts/smoke-*.mjs` are the shape of the `cli` target, not tests to port.** Each drives the
real binary over stdio with hand-written frames — `smoke-acp.mjs`, `smoke-acp-load.mjs`,
`smoke-queue.mjs`, `smoke-modes.mjs`, `smoke-session.mjs`, `smoke-compact.mjs`, `smoke-export.mjs`,
`smoke-startupinfo.mjs`, `smoke-newsession-intro.mjs`, `smoke-changelog.mjs` — which is exactly what
the `cli` target does with `cyrup --acp`. Read them for their frame sequences and their assertion
points; do not carry them as `.mjs`, since that would put a Node dependency into a port whose whole
premise is having none. `smoke-changelog.mjs` has no counterpart at all until `ACP-272`'s changelog
decision lands.

### What no test in this document can prove

Three of the guarantees this port leans on are **not** closed by any assertion here, and the ADR says
so at each site: `Turn`'s typestate does not order the `PromptResponse` after the final
`session/update` — that follows from one task owning both, which is control flow a type cannot
enforce; `Snapshot` does not choose the read backend, which is `ACP-156`; and `DialogChoice` does not
make the fail-closed path *timely*, only correct. Two compile-fail (`trybuild`) cases are worth their
maintenance and no more: that `Turn::settle` twice does not compile, and that `Turn::begin` cannot be
handed the result of `AgentSession::subscribe()`. A compile-fail test for "you cannot construct
`AcpSessionId`" adds maintenance without information — the private field is self-evident in review.

---

## 8. Sequencing

The dependency order, then the three units to pick up first.

### Phase 0 — answer two questions before writing code

**`ACP-Q1` (`serde_json/preserve_order`) gates the crate's existence**, not one unit. If the answer is
"keep `cyrup-acp` out of the workspace binary", the in-process architecture is void and §1's flip
conditions apply. Measure it: add `agent-client-protocol` as a normal dependency of one member, run
`cargo test --workspace`, and audit the four ordering-sensitive sites. **`ACP-Q30` (the `pub(crate)`
`session_resolve` helpers) gates four units and the crate layout**, and is coupled to `ACP-Q15`; it is
cheap to answer and expensive to defer, because it decides whether `cyrup-acp` is a library crate at
all. Neither needs a line of ACP code.

### Phase 1 — the process exists and speaks the protocol

`ACP-002` → `ACP-021` → `ACP-003`, in that order, with `ACP-023` resolved as part of `ACP-003` rather
than after it. **`ACP-021` must land with `ACP-002`, not after**: the mode variant and the
`require_model` override are the same edit in `crates/cyrup/src/main.rs`, and shipping the first
without the second means a credential-less launch dies before the transport exists, which makes the
entire authentication surface unreachable and untestable. `ACP-005`, `ACP-004` and `ACP-024` close the
lifecycle; `ACP-006`, `ACP-008`-as-a-line-of-`ACP-002`, `ACP-018` and `ACP-025` are trivia that ride
along. `ACP-213` belongs here too, because both `config.persist` sites are in the same two files and
adding `Acp` to one of them is the foot-gun.

### Phase 2 — the turn, correlated correctly from the first line

`ACP-153` **before** `ACP-121`. The temptation is to write the settle rule first because it is the
`critical`, but `ACP-121`'s fix does not close the two failures `ACP-153` names — a spurious settle
from a refused inner prompt, and a settle from a run the host did not start — and retrofitting the
run-scoped stream afterwards means rewriting the actor. Then `ACP-121`, `ACP-154` (the third
termination pi-acp does not have), `ACP-155` (the pump/dialog task split, which must be decided before
either `ACP-144` or `ACP-122` is written, not reconciled after), `ACP-122`, `ACP-123`, `ACP-126`. Only
then `ACP-057` and `ACP-061`, which need a turn to install into. `ACP-124` last in this phase, because
`ACP-Q22` may make its option (b) free.

### Phase 3 — the permission seam

`ACP-146` and `ACP-145` together, then `ACP-144` and `ACP-150`, then `ACP-147`. **These are the two
permission-bypass criticals and they must not be split across phases**, because the `UiSink` is
installed once and a half-installed sink that serves `Confirm` but not `Select` leaves
`LocalAskChannel` — the tool-permission dialog — reaching a sink that answers nothing. `ACP-155`'s
task split is a prerequisite, which is why it sits in phase 2.

### Phase 4 — events, tools and the terminal

`ACP-127` → `ACP-129` → `ACP-128` → `ACP-131` → `ACP-135`, with `ACP-156` resolved before `ACP-131`
commits to a read mechanism. The bash family (`ACP-138`…`ACP-141`, `ACP-157`) is gated on `ACP-Q25`:
if the typed `terminal/*` client family is the supported route, most of it is a cut, so **answer
`ACP-Q25` before building the `_meta` protocol**. `ACP-141` option (c) is a small `cyrup-tools` change
and should be raised with that crate's owner early, since it is the only unit in this document that
edits another crate's data shape.

### Phase 5 — sessions on disk

`ACP-222` **before** `ACP-202`, and `ACP-202` before anything treats filename derivation as
authoritative. Then `ACP-209` (with `ACP-Q33` answered), `ACP-225` (which reconciles `ACP-209` with
`ACP-212` and must be decided before either is implemented), `ACP-219` + `ACP-224` together,
`ACP-220`, and the listing family (`ACP-200`…`ACP-208`, `ACP-223`, `ACP-226`…`ACP-230`) once `ACP-Q30`
has said where the helpers live. `ACP-214`…`ACP-217` (replay) last, because `ACP-215`'s `ToolKind`
mapping must follow `ACP-151`'s.

### Phase 6 — the request surface and the commands

`ACP-050`…`ACP-056`, `ACP-058`…`ACP-060` (with `ACP-Q7` answered first — it changes what three of them
do), `ACP-062`…`ACP-065`, `ACP-072`…`ACP-079`, then the whole of 4e. `ACP-Q20` must be settled before
`ACP-077` and the three setters are written, or the client gets doubled updates. `ACP-291` lands with
`ACP-288` and not after it: the sanitiser is the security control and a `/export` shipped without it
is an arbitrary write.

### The first three units a porter should pick up

1. **`ACP-Q1`, as work rather than a question.** Not a unit, and deliberately first. It is a
   half-day measurement whose answer either validates the entire architecture or voids it, and every
   line written before it is written on an unverified premise. Nothing else in this document is
   cheaper per unit of risk removed.
2. **`ACP-002` + `ACP-021`, as one change.** The smallest edit that makes an ACP process exist and be
   reachable: a variant, a flag, a first branch in `resolve_app_mode`, two exhaustive matches, and the
   `require_model` override. Without the second half the whole auth surface is dead code, and the two
   halves live in the same file — which is exactly why they were filed separately by different passes
   and would have been implemented separately.
3. **`ACP-153`, before `ACP-121`.** The turn's correlation primitive. It is the one structural
   decision that is expensive to retrofit: every event-translation unit hangs off the stream the turn
   owns, and choosing `AgentSession::subscribe()` first means rewriting the actor and re-testing
   `ACP-121`, `ACP-154` and `ACP-155` afterwards. Picking it up first also forces an early read of
   `AgentSession::prompt` and `Fanout::{subscribe_run, end_run}`, which is the reading a porter needs
   before touching anything else in 4c.

**What deliberately is not in the first three**: the two permission criticals. They are more severe
than any of the above and they are correctly phase 3, because a `UiSink` has nothing to install into
until a session and a turn exist. Severity ranks the backlog; it does not order the build.
