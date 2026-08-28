---
stage: aug
status: done
updated: 2026-08-27 06:00
---

# The `/mcp` Command Surface: One Handler, Its Arms, And The Three Host Verbs It Needs

## Description

Every surface a human drives by hand in `cyrup-mcp` is unreachable at **one seam**.

[`McpExtension`](../../crates/cyrup-mcp/src/extension.rs) implements `id` ([:593](../../crates/cyrup-mcp/src/extension.rs)),
`is_ambient` ([:606](../../crates/cyrup-mcp/src/extension.rs)), `init` ([:625](../../crates/cyrup-mcp/src/extension.rs)),
`on_event` ([:713](../../crates/cyrup-mcp/src/extension.rs)), `render_call` ([:754](../../crates/cyrup-mcp/src/extension.rs)),
`render_result` ([:764](../../crates/cyrup-mcp/src/extension.rs)), `set_host_services` ([:776](../../crates/cyrup-mcp/src/extension.rs))
and `set_late_registrar` ([:783](../../crates/cyrup-mcp/src/extension.rs)) — and does **not** override
`NativeExtension::execute_command`. A grep for `execute_command` across `crates/cyrup-mcp/src`
returns three doc-comment mentions and no `impl`
([registration.rs:1926](../../crates/cyrup-mcp/src/registration.rs),
[registration.rs:2196](../../crates/cyrup-mcp/src/registration.rs),
[oauth.rs:3781](../../crates/cyrup-mcp/src/oauth.rs)). **Verified 2026-08-27: still true.**

Both commands *are* registered — at
[registration.rs:2136 and :2138](../../crates/cyrup-mcp/src/registration.rs), behind
`api.register_fixed_commands()` — so `/mcp` and `/mcp-auth` appear in the `/` menu, route correctly
through `ExtensionHost::execute_native_command`
([facade.rs:533-573](../../crates/cyrup-ext/src/facade.rs)), and land on the trait's default body:

```rust
Err(ExtError::Component(format!("native extension has no handler for command `{name}`")))
```

[native.rs:580-587](../../crates/cyrup-ext/src/native.rs) (the `Err` is line 586). That surfaces as an
`ExtError`-prefixed error notification (`command:mcp: …`). So today **both commands answer with the
same error string**, and every listing, every panel entry point, every write-back and every prompt
invocation is dead code behind it.

Eighteen units. Thirteen are that one handler and its arms — an arm has nowhere to attach until the
owner-fenced prologue exists, so they are five files' worth of one job. Three more (MCP-041,
MCP-362, MCP-368) are the only host additions these surfaces need, and nothing else in the port
touches `cyrup-tui`; MCP-377 is the `cyrup-mcp` half of MCP-368's painting problem and cannot be
split from it. MCP-049 is the same surface out of session.

---

## Premise audit — what this augmentation changed

The task as written carried **eleven** claims that no longer hold or never held. Each is corrected
in place below; they are listed here first because several change what gets built.

| # | Claim as written | Verified reality |
|---|---|---|
| 1 | `proxy.rs:1921` / `:1436-1495` / `:1490-1492` | **`crates/cyrup-mcp/src/proxy.rs` does not exist.** It was split into `proxy/` (14 files). `execute_status` is [proxy/discovery.rs:34](../../crates/cyrup-mcp/src/proxy/discovery.rs); `ProxyEnv::failure_age_seconds` is [proxy/env.rs:299](../../crates/cyrup-mcp/src/proxy/env.rs); `ConnectionStatus` is [proxy/env.rs:30](../../crates/cyrup-mcp/src/proxy/env.rs). Every `proxy.rs:NNNN` citation in the old text was dead. |
| 2 | "The three headless listings" | **They are not headless.** `showStatus`, `showTools` and `showPrompts` each open with `if (!ctx.hasUI) return;` ([commands.ts:45](../../tmp/pi-mcp-adapter/commands.ts), [:91](../../tmp/pi-mcp-adapter/commands.ts), [:128](../../tmp/pi-mcp-adapter/commands.ts)). The `default` arm's no-UI branch calls `showStatus`, which then does nothing — so a genuinely headless `/mcp` prints **nothing at all**. Port the guard; do not invent output. |
| 3 | `show_status` reads `McpState` accessors "the spine group introduces behind `ProxyEnv`" | **No `ProxyEnv` is needed and most accessors already exist.** `state.manager.get_connection(name)` ([server_manager.rs:1467](../../crates/cyrup-mcp/src/server_manager.rs)) returns `Option<Arc<ServerConnection>>`; `ServerConnection::status()` ([:866](../../crates/cyrup-mcp/src/server_manager.rs)) and `prompt_discovery_failed()` ([:978](../../crates/cyrup-mcp/src/server_manager.rs)) are public; `get_all_connections()` ([:1475](../../crates/cyrup-mcp/src/server_manager.rs)) is the snapshot `showPrompts` needs. Only `failure_age_seconds` / `failure_message` are missing, and the spine already prescribes the first ([MCP_RUNTIME_INIT_SPINE.md:734](MCP_RUNTIME_INIT_SPINE.md)). The second is **not** in the spine and is this task's. |
| 4 | `showTools` names come straight from `tool_metadata` | **It filters disabled servers first** ([commands.ts:129-131](../../tmp/pi-mcp-adapter/commands.ts)): `.filter(([serverName]) => !isServerDisabled(state.config.mcpServers[serverName]))`. Omitting that lists a disabled server's cached tools. |
| 5 | `read_raw_config_object` / `write_raw_config_object` "round-trip the JSONC **preserving comments**" — and MCP-049's divergence is "cyrup preserves comments; that is better behaviour" | **False, and there is a shipped test asserting the opposite.** [config.rs:4771](../../crates/cyrup-mcp/src/config.rs) is `assert!(!preview.before_text.contains("//"), "comments are stripped by the parse")`. [`RawJson`](../../crates/cyrup-mcp/src/config.rs) ([:254-267](../../crates/cyrup-mcp/src/config.rs)) has no comment variant and `serialize_raw_object` ([:3097](../../crates/cyrup-mcp/src/config.rs)) is `serde_json::to_string_pretty`. The real divergence is different (see §7). The acceptance criterion "comments survive the write" is **deleted** — it was unachievable. |
| 6 | `writeProjectServerDisabledOverride(earlyConfigPath, cwd, name, disabled)` | The Rust is a **`ConfigContext` method** taking only `(server_name, disabled)`: [config.rs:3484](../../crates/cyrup-mcp/src/config.rs). The path and cwd live on the context. Building that context is a 7-line idiom already duplicated at [extension.rs:196-204](../../crates/cyrup-mcp/src/extension.rs) and [extension.rs:626-634](../../crates/cyrup-mcp/src/extension.rs) — this arm is the third caller, so factor it. |
| 7 | `main.rs:183-190` pre-dispatches subcommands | `first_subcommand` has **no caller in `main.rs`**. It is called from [predispatch.rs:67](../../crates/cyrup/src/predispatch.rs), inside `run_predispatch`, which also resolves the `ConfigDirs`. |
| 8 | `session.rs:2623-2634` (`slash_command_catalog`) | `crates/cyrup-session-svc/src/session.rs` **does not exist**; it is `session/`. The function is [session/commands.rs:174](../../crates/cyrup-session-svc/src/session/commands.rs) and the extension-row builder ends at [:245](../../crates/cyrup-session-svc/src/session/commands.rs). |
| 9 | `editor.rs:96-120`, `:1610`, `:1650` | `crates/cyrup-tui/src/editor.rs` **does not exist**; it is `editor/`. `InputEditor` is [editor/mod.rs:115](../../crates/cyrup-tui/src/editor/mod.rs), `set_registry` is [editor/config.rs:118](../../crates/cyrup-tui/src/editor/config.rs), and the two `Autocomplete::compute` call sites are [editor/completion.rs:79](../../crates/cyrup-tui/src/editor/completion.rs) and [:119](../../crates/cyrup-tui/src/editor/completion.rs). |
| 10 | MCP-391: "the panel is met, only the entry point is absent" | **More is met than that.** [`open_mcp_panel`](../../crates/cyrup-mcp/src/ui.rs) ([ui.rs:4805](../../crates/cyrup-mcp/src/ui.rs)) and [`open_mcp_setup_panel`](../../crates/cyrup-mcp/src/ui.rs) ([ui.rs:4826](../../crates/cyrup-mcp/src/ui.rs)) exist and already solve the result-out-of-a-`bool` problem; [`AUTH_PANEL_NOTICE`](../../crates/cyrup-mcp/src/ui.rs) ([ui.rs:4757](../../crates/cyrup-mcp/src/ui.rs)) is the notice line; [`panel_unavailable_message`](../../crates/cyrup-mcp/src/ui.rs) ([ui.rs:4767](../../crates/cyrup-mcp/src/ui.rs)) is the `/mcp setup` twin the old text never mentioned. What is genuinely absent is four guards and one `Warning`. |
| 11 | MCP-334 is "half met, and the met half is the expensive one" (the eleven strings) | True, **and the other expensive half is also met**: [`oauth::authenticate`](../../crates/cyrup-mcp/src/oauth.rs) ([oauth.rs:3333](../../crates/cyrup-mcp/src/oauth.rs)) and [`AuthenticateOptions`](../../crates/cyrup-mcp/src/oauth.rs) ([oauth.rs:2425-2445](../../crates/cyrup-mcp/src/oauth.rs)) already carry `on_authorization_url` and `on_authorization_input` hooks with exactly upstream's shapes. MCP-334's dispatch is therefore writable **end to end** in this task. Two literals are still missing: `msg_not_oauth`'s **two-line notify variant** (the returned message and the notified text differ upstream, [commands.ts:268-274](../../tmp/pi-mcp-adapter/commands.ts)) and `terminalHyperlink`'s OSC-8 emitter. |

Two smaller corrections folded into the body: `ctx.has_ui` is a **public field** at
[native.rs:92](../../crates/cyrup-ext/src/native.rs), not a method; and `ExtMode`
([native.rs:28-34](../../crates/cyrup-ext/src/native.rs)) has **no** `Display`/`as_str`, so the two
panel-unavailable messages — which take `mode: &str` — need a small mapper.

---

### Landing order

**This task must land AFTER [MCP_RUNTIME_INIT_SPINE.md](MCP_RUNTIME_INIT_SPINE.md).** The prologue
awaits `initPromise` and every arm reads live `McpState`; the spine is what builds it and what
supplies `crate::env::failure_age_seconds`
([MCP_RUNTIME_INIT_SPINE.md:734](MCP_RUNTIME_INIT_SPINE.md)), which MCP-383 reads. `failure_messages`
exists only as an `McpState` **field** today
([state.rs:117](../../crates/cyrup-mcp/src/state.rs)) with a writer in the spine and **no reader
anywhere** — the reader is §6 of this task.

**MCP-381 and MCP-398 belong to Wave 7 of
[MCP_HIGH_SEVERITY_BACKLOG.md](MCP_HIGH_SEVERITY_BACKLOG.md)
([:431-460](MCP_HIGH_SEVERITY_BACKLOG.md)).** MCP-381 is the switch this task's arms hang off, and
MCP-040 / MCP-042 / MCP-334 *are* that switch's `/mcp` and `/mcp-auth` halves — both commands are
answered by the same default error today, and splitting the prologue from its two registrations
produces two handlers that duplicate the fence. **Ship the prologue and the whole switch here**, in
one commit, with every arm's call site written. Five arm *bodies* belong to Wave 7 units and are
**not** written here:

| arm | owning unit | what this task writes |
|---|---|---|
| `reconnect` | MCP-386 | the call site + `fn arm_reconnect` with a `TODO(MCP-386)` body |
| `setup` | MCP-387 | the call site + the `programmaticConfig` refusal + the reload-on-`configChanged` shape |
| `logout` | MCP-388 | the call site + the `Usage:` guard + the early `return` |
| `status`/`""`/default | MCP-394 | the whole arm except `openMcpPanel`'s orchestration, which is `TODO(MCP-394)` at [ui.rs:4788-4793](../../crates/cyrup-mcp/src/ui.rs) |
| the panel callbacks | MCP-392 | nothing — MCP-391's entry point takes `Arc<dyn McpPanelCallbacks>` as a **parameter** |

Every such stub follows the convention the crate already uses — a named `TODO(MCP-NNN)` comment
beside an inert body, exactly as [ui.rs:3260](../../crates/cyrup-mcp/src/ui.rs) and
[ui.rs:4788](../../crates/cyrup-mcp/src/ui.rs) do today. Do **not** write a second copy of any Wave 7
body "temporarily".

`register_late_command` and `LateRegistrar` **already exist**
([facade.rs:724](../../crates/cyrup-ext/src/facade.rs),
[native.rs:768-787](../../crates/cyrup-ext/src/native.rs)) — HA-1's command leg landed with
MCP-037/037a. Nothing in this group needs to build it.

---

## Four traps

**1. `execute_status` is not `showStatus`.**
[`proxy::discovery::execute_status`](../../crates/cyrup-mcp/src/proxy/discovery.rs) at
[proxy/discovery.rs:34-110](../../crates/cyrup-mcp/src/proxy/discovery.rs) is MCP-154 — the *gateway
tool's* `status` mode. Reuse its **six-rung ladder shape** (`disabled → connected → needs-auth →
failed → cached → not connected`, first match wins, with `metadata`/`connection` forced absent for a
disabled server) and port `showStatus`'s text fresh. The two texts are different artefacts:

| | `execute_status` (model-facing) | `showStatus` (human-facing) |
|---|---|---|
| header | `MCP: 1/2 servers, 7 tools` (+ ` (1 disabled)`) | `MCP Server Status:` then a blank line |
| disabled | `⊘ name (disabled)` | `⊘ name: disabled (run /mcp enable name, then /reload)` |
| connected | `✓ name (7 tools)` | `✓ name: connected (7 tools)` |
| failed | `✗ name (failed 12s ago)` — **no reason** | `✗ name: failed 12s ago — {sanitized reason}` |
| tail | `mcp({ server: "name" }) to list tools, …` | nothing |

Calling `execute_status` from `/mcp status` ships model-facing text to the human.

**2. `transform_mcp_content` is not `formatPromptResult`.**
[renderers.rs:616-670](../../crates/cyrup-mcp/src/renderers.rs) is `tool-registrar.ts`'s
**tool-result** shaping (MCP-220) over `&[serde_json::Value]`: `[Resource: {uri}]`,
`[Resource Link: {name}]\nURI: {uri}`, `[Audio content: {mime}]`, mime defaults of `image/png` /
`audio/*`, and an unknown arm that **re-serializes the original JSON**. MCP-399's §5.6 operates on
`rmcp::model::ContentBlock` and produces `[resource {uri}]`, `[resource_link {uri} — {name}]`,
`[audio {mime|unknown}]`, `[image {mime|unknown} (embedded)]`, with an unknown kind contributing the
**empty string**. Different input type, different casing, different bracket text, opposite unknown
behaviour. Two functions.

**3. The spec is wrong about `/mcp`'s completion list, and so is the tree.**
[`mcp_command_descriptor`](../../crates/cyrup-mcp/src/registration.rs) at
[registration.rs:1877-1888](../../crates/cyrup-mcp/src/registration.rs) declares **nine**
completions:

```rust
["reconnect", "tools", "prompts", "setup", "logout", "token", "disable", "enable", "status"]
```

`"token"` is at [registration.rs:1881](../../crates/cyrup-mcp/src/registration.rs). Upstream declares
**eight** and there is no `token` among them
([index.ts:476-485](../../tmp/pi-mcp-adapter/index.ts),
[13a §22 :826-831](../../docs/gap-analysis/13a-mcp-activation.md),
[13h §4.1 :1580-1590](../../docs/gap-analysis/13h-mcp-tui.md)). 13h's eight-way switch table
([:1613-1628](../../docs/gap-analysis/13h-mcp-tui.md)) gives `token` no arm, and a grep for `"token"`
as a subcommand across `crates/cyrup-mcp/src` finds only that one line. `token` is an invention.
Left in place, `/mcp token<TAB>` completes to a subcommand that falls through to the `default` arm
and **opens the browser panel** — a completion that lies. **Delete `"token"` from that array, and
fix the doc comment above it** ([:1872-1876](../../crates/cyrup-mcp/src/registration.rs)), which says
"nine static subcommands" and names `token` in the runtime-branch list. Do not add a `token` arm.

**4. `ResourceContents` and `ContentBlock` are both `#[non_exhaustive]`.**
`ContentBlock` is at [`rmcp-3.1.4` `model/content.rs:250-265`], `ResourceContents` at
[`model/resource.rs:167-190`]. rmcp's own `impl` blocks match them exhaustively because
`#[non_exhaustive]` only binds *foreign* crates — `cyrup-mcp` is foreign, so **both** matches need a
wildcard arm or they will not compile. The prescribed MCP-399 body in §5 has both.

---

## Per-unit breakdown

### The handler and its two registrations

#### MCP-040 — the `/mcp` command handler · medium · `host-verb`

**Unmet, re-verified.** No `execute_command` override exists; `/mcp` reaches
[native.rs:586](../../crates/cyrup-ext/src/native.rs). Absent: the owner-fenced `commandCtx`, the
**un-timed** `await initPromise` preamble with `MCP initialization failed: {message}` /
`MCP not initialized`, the `split(/\s+/)` split with `subcommand = parts[0] ?? ""`,
`targetServer = parts[1]`, `rest = parts[1..].join(" ")`, and the eight-way switch
([index.ts:501-618](../../tmp/pi-mcp-adapter/index.ts),
[13a §22 :838-857](../../docs/gap-analysis/13a-mcp-activation.md),
[13h §4.1-4.2 :1557-1628](../../docs/gap-analysis/13h-mcp-tui.md)).

The half that already exists and must be reused, not re-derived:
[`OwnedServices`](../../crates/cyrup-mcp/src/owner.rs) ([owner.rs:312-337](../../crates/cyrup-mcp/src/owner.rs))
is `createOwnedUi`, and its `fenced!` list ([owner.rs:373-465](../../crates/cyrup-mcp/src/owner.rs))
already covers `notify`, `set_status`, `confirm`, `input`, `open_overlay`, `control` and
`is_run_cancelled` — every verb this handler touches.
[`McpExtension::on_input`](../../crates/cyrup-mcp/src/extension.rs)
([extension.rs:517-554](../../crates/cyrup-mcp/src/extension.rs)) is the shipped template for
capture-owner-then-await-`init_task`; the command prologue is the same shape **without**
`INIT_WAIT_TIMEOUT_MS`.

**Correction to the ctx shape.** Upstream is
`ui: commandHasUI ? (commandOwner ? createOwnedUi(ctx.ui, commandOwner) : ctx.ui) : undefined`
([index.ts:505-512](../../tmp/pi-mcp-adapter/index.ts)) — three states, not two. A `has_ui == false`
context has **no** `ui` even when services are bound, and an ownerless context uses the **raw**
services rather than nothing. The `match (services, owner)` in the task's original sketch collapsed
both. It also dropped `commandReload` entirely; in cyrup that is
`HostServices::control(ControlOp::Reload)` ([services.rs:107](../../crates/cyrup-ext/src/host/services.rs))
through the same fenced handle, so the fence *is* the binding — but the arms that call it must go
through `cmd.ui`, never through `self.host_services()`.

#### MCP-042 — the `/mcp-auth` command handler · medium · `host-verb`

**Unmet**, same seam. The ordering detail that must survive: `if (!serverName && !ctx.hasUI)
return;` fires **silently, before the init-await**
([index.ts:637-639](../../tmp/pi-mcp-adapter/index.ts),
[13h §4.3 :1629-1638](../../docs/gap-analysis/13h-mcp-tui.md)) — a headless `/mcp-auth` with no
argument must produce no output, no error and **no initialization wait**. Writing the prologue first
and the bail second is the natural Rust order and is wrong.

#### MCP-334 — the `/mcp-auth` command surface and its eleven messages · medium · `host-verb`

**Half met, and both halves of the expensive part are already there**
([13g :1294-1305](../../docs/gap-analysis/13g-mcp-oauth.md)).

All eleven message strings are ported as functions in
[oauth.rs:3800-3872](../../crates/cyrup-mcp/src/oauth.rs):
[`MSG_REQUIRES_INTERACTIVE`](../../crates/cyrup-mcp/src/oauth.rs) (:3800),
`msg_server_not_found` (:3804), `msg_server_disabled` (:3810), `msg_not_oauth` (:3817),
`msg_no_url` (:3825), `msg_authenticating` (:3831), `msg_auth_success` (:3837),
`msg_auth_failed` (:3843), `msg_auth_threw` (:3851), `msg_auth_required_proxy` (:3858),
`msg_auth_required_direct_tools` (:3867). A grep for consumers of any of them across
`crates/cyrup-mcp/src` returns **exactly one hit, and it is a doc comment**
([oauth.rs:3784](../../crates/cyrup-mcp/src/oauth.rs)).

The *flow* is also built: [`oauth::authenticate`](../../crates/cyrup-mcp/src/oauth.rs)
([:3333-3338](../../crates/cyrup-mcp/src/oauth.rs)) plus
[`AuthenticateOptions`](../../crates/cyrup-mcp/src/oauth.rs) ([:2425-2445](../../crates/cyrup-mcp/src/oauth.rs))
with `on_authorization_url: Option<AuthorizationUrlHook>` ([:2413](../../crates/cyrup-mcp/src/oauth.rs))
and `on_authorization_input: Option<AuthorizationInputHook>` ([:2420](../../crates/cyrup-mcp/src/oauth.rs)).
`McpState::auth_storage_options` is field 12 ([state.rs:122](../../crates/cyrup-mcp/src/state.rs)) and
[`supports_oauth`](../../crates/cyrup-mcp/src/oauth.rs) is [oauth.rs:349](../../crates/cyrup-mcp/src/oauth.rs).

What is unmet is the dispatch, verbatim from
[commands.ts:244-333](../../tmp/pi-mcp-adapter/commands.ts):

* the guard order **no UI → unknown server → disabled → not OAuth → no URL**;
* `msg_server_disabled` notified at **`Warning`**, not `Error` ([commands.ts:263](../../tmp/pi-mcp-adapter/commands.ts));
* the `not OAuth` case notifying a **two-line** text while returning the one-line message — the
  doc comment at [oauth.rs:3815-3816](../../crates/cyrup-mcp/src/oauth.rs) records this and the
  second literal does not exist yet;
* `set_status("mcp-auth", Some(&msg_authenticating(name)))` and the `finally` clear
  **unless the signal aborted** — `HostServices::set_status(&self, key, Option<&str>)`
  ([owner.rs:377](../../crates/cyrup-mcp/src/owner.rs)) maps 1:1 including its `None`-clears semantics;
* `terminal_hyperlink` (OSC 8). `grep -rn "terminal_hyperlink\|u{1b}]8" crates/cyrup-mcp/src`
  returns only two **test** strings ([ui.rs:4858](../../crates/cyrup-mcp/src/ui.rs),
  [ui.rs:4869](../../crates/cyrup-mcp/src/ui.rs)). It must sanitize both halves and then emit the raw
  escape, because [`sanitize_terminal_text`](../../crates/cyrup-mcp/src/ui.rs)
  ([ui.rs:376](../../crates/cyrup-mcp/src/ui.rs)) strips OSC-8 — that is the whole reason upstream
  builds the sequence *after* sanitizing ([commands.ts:27-29](../../tmp/pi-mcp-adapter/commands.ts)).

### The three UI-gated listings

> **They are UI-gated, not headless.** Each begins `if (!ctx.hasUI) return;`. This is not a detail to
> "improve": the default arm's no-UI branch calls `showStatus`, so `/mcp` in print/json mode is
> deliberately silent. Port the guard as the first line of each.

#### MCP-383 — port `showStatus` · medium · `hand-written`

**Unmet.** `grep -rn "MCP Server Status" crates/` returns nothing. Needs
[§4.4 (13h :1639-1664)](../../docs/gap-analysis/13h-mcp-tui.md) /
[commands.ts:44-88](../../tmp/pi-mcp-adapter/commands.ts) in full: the `["MCP Server Status:", ""]`
header, rows in `config.mcp_servers` insertion order, the disabled row with `continue` and **no tool
suffix**, the five-rung first-match ladder, `toolSuffix = failed ? "" : " ({n} tools{, cached})"`,
`tools` **never singularised**, and the two-line empty-config message. The failure arm is tested
**before** the metadata arm, so a failed server never reports `cached` even with metadata present.

`ctx.hasUI` is the public field `HostCtx::has_ui` ([native.rs:92](../../crates/cyrup-ext/src/native.rs))
— the guard selects on that, **not** on `open_overlay` returning `false`.

#### MCP-384 — port `showTools` · low · `hand-written`

**Unmet.** `grep -rn "MCP Tools:" crates/` returns nothing. Upstream is
[commands.ts:127-148](../../tmp/pi-mcp-adapter/commands.ts). The names come from
`McpState::tool_metadata` ([state.rs:88-90](../../crates/cyrup-mcp/src/state.rs)), whose
`ServerToolMetadata::tool_names` ([state.rs:365-371](../../crates/cyrup-mcp/src/state.rs)) is already
documented as "the resolved, model-visible tool names … in server order" — i.e. the **prefixed,
registered** names §4.5 requires. **Disabled servers are filtered out first.** `Total: 1 tools` is
correct output; do not singularise. The empty case is one `No MCP tools available` Info notify and
an early return — *not* the header block with a zero total.

#### MCP-385 / MCP-385a — port `showPrompts` and its per-group header · medium / low

**Unmet.** `grep -rn "MCP Prompts:" crates/` returns nothing.
[§4.6 (13h :1672-1695)](../../docs/gap-analysis/13h-mcp-tui.md) /
[commands.ts:90-125](../../tmp/pi-mcp-adapter/commands.ts) need the group header `{serverName}:`
(unindented, no icon, plain colon, **unsanitized** — the name is the user's own config), servers
ordered by `localeCompare`, prompts sorted **in place** by `command_name`, `<required>` /
`[optional]` usage rendering, the two-space `/{command_name}` row, the **six**-space description row,
the per-group blank line, `Total: {N} prompt{s}` (singular only at 1), and the two distinct
`promptDiscoveryFailed` notes.

`failedPromptServers` is `get_all_connections()` filtered to `status == Connected &&
prompt_discovery_failed()`, then sorted — both accessors exist
([server_manager.rs:1475](../../crates/cyrup-mcp/src/server_manager.rs), [:978](../../crates/cyrup-mcp/src/server_manager.rs)).

MCP-385a is filed separately because a golden-text test written from a spec that omits the header row
cannot catch its absence. Write the header first.

**The declared divergence:** `String.localeCompare` with no locale is ICU root collation; Rust's
`str::cmp` is byte order. They agree on ASCII-lowercase names and disagree on mixed case (`Foo` vs
`bar`). Use `str::cmp` and **say so in the ported comment** — do not leave it unstated and do not
pull in a collation crate for one sort.

**Blocker this task absorbs:** `McpState::prompt_metadata`
([state.rs:94](../../crates/cyrup-mcp/src/state.rs)) holds `Vec<PromptMetadata>`, and
[`PromptMetadata`](../../crates/cyrup-mcp/src/state.rs) is still a one-field forward declaration
carrying only `name` ([state.rs:373-382](../../crates/cyrup-mcp/src/state.rs)). MCP-385 needs
`command_name`, `description` and `arguments`. Do not invent a second type: replace the stub's body
with the same fields [`PromptCommandSpec`](../../crates/cyrup-mcp/src/registration.rs) already carries
([registration.rs:1789-1797](../../crates/cyrup-mcp/src/registration.rs)) — `server_name`,
`original_name`, `command_name`, `title`, `description`, `arguments: Vec<CachedPromptArgument>` — so
the cache path (`resolve_cached_prompts`, [registration.rs:1803](../../crates/cyrup-mcp/src/registration.rs))
and the live path produce one shape. Upstream's own `PromptMetadata`
([types.ts:584-591](../../tmp/pi-mcp-adapter/types.ts)) is exactly those six fields, which is why
they match. MCP-039/MCP-395a own **populating** it; this group owns the fields it reads.

### The two remaining switch arms

#### MCP-389 — port `/mcp disable` and `/mcp enable` · medium · `hand-written`

**Unmet at the arm; met at the writer.** `grep -rn "not found in effective config\|run /reload to
apply" crates/` returns nothing in `cyrup-mcp`. But
[`ConfigContext::write_project_server_disabled_override`](../../crates/cyrup-mcp/src/config.rs)
([config.rs:3484-3570](../../crates/cyrup-mcp/src/config.rs)) is fully built, returns
[`ServerDisabledOverrideResult { path, changed }`](../../crates/cyrup-mcp/src/config.rs)
([config.rs:3333-3339](../../crates/cyrup-mcp/src/config.rs)), and already carries the four exact
error messages and the never-copy-a-definition property. The arm is four notifies and a call — plus
building the `ConfigContext`, which is the duplicated idiom of premise #6.

This is the **only** subcommand that tells the user to run `/reload` themselves; `setup` and the
panel path call `commandReload` for them.

#### MCP-391 — port `openMcpAuthPanel` · medium · `host-verb`

**Entry point unmet; panel met, and more of it than the task claimed.**
[`PanelOptions::auth_only`](../../crates/cyrup-mcp/src/ui.rs) exists
([ui.rs:1442-1457](../../crates/cyrup-mcp/src/ui.rs)) and `McpPanelModel::new`
([ui.rs:1594-1600](../../crates/cyrup-mcp/src/ui.rs)) already threads it through construction, the
visible-item rebuild ([ui.rs:1633](../../crates/cyrup-mcp/src/ui.rs)) and the empty message
([ui.rs:2669-2672](../../crates/cyrup-mcp/src/ui.rs)) — **one type, not two**, as the unit requires.
[`AUTH_PANEL_NOTICE`](../../crates/cyrup-mcp/src/ui.rs) ([ui.rs:4757](../../crates/cyrup-mcp/src/ui.rs)),
[`auth_panel_unavailable_message`](../../crates/cyrup-mcp/src/ui.rs) ([ui.rs:4780](../../crates/cyrup-mcp/src/ui.rs))
and [`open_mcp_panel`](../../crates/cyrup-mcp/src/ui.rs) ([ui.rs:4805](../../crates/cyrup-mcp/src/ui.rs))
are all ported.

What is absent is the entry point's four guards, in this exact order
([commands.ts:605-653](../../tmp/pi-mcp-adapter/commands.ts)):

1. `!ctx.hasUI` ⇒ `{ configChanged: false }`, **silently**;
2. `!canRenderPanel(ctx)` ⇒ notify `auth_panel_unavailable_message(mode)` at Info;
3. `programmaticConfig` ⇒ notify `Use /mcp-auth <server> to authenticate a server from the in-memory
   SDK config.` at Info (`grep -rn "unavailable when config is supplied" crates/` returns nothing);
4. zero non-disabled OAuth-capable servers ⇒ notify at **`NotifyKind::Warning`** and do not open.

**Do not reuse the string that already exists.** [ui.rs:2670](../../crates/cyrup-mcp/src/ui.rs) holds
`"No OAuth-capable MCP servers configured."` — that is the panel's **empty-body row**, rendered
inside an already-open panel. MCP-391's guard text is `"No OAuth-capable MCP servers are
configured."` — with `are`. Two strings, two surfaces; collapsing them loses the guard.

Always returns `configChanged: false`, even after a successful authentication.

**The callbacks are MCP-392's, not this task's.** `grep -rn "impl McpPanelCallbacks" crates/` finds
only three **test** stubs ([ui.rs:5028](../../crates/cyrup-mcp/src/ui.rs),
[:5385](../../crates/cyrup-mcp/src/ui.rs), [:5525](../../crates/cyrup-mcp/src/ui.rs)). The entry
point therefore takes `callbacks: Arc<dyn McpPanelCallbacks>` as a parameter rather than building
one — which is also what makes it testable against the existing stubs.

### The prompt-command grammar and renderer (MCP-398's feed)

```
grep -rn "parse_prompt_args\|resolve_prompt_args\|tokenize_args\|strip_quotes\|\
find_unquoted_equals\|format_prompt_result\|extract_message_text\|build_usage_message" \
  crates/cyrup-mcp/src
```

returns **nothing**. All four units are unwritten. Upstream is
[prompts.ts:44-222](../../tmp/pi-mcp-adapter/prompts.ts).

#### MCP-396 — `parsePromptArgs`' bash-style tokenizer · medium

Three details a reasonable rewrite gets wrong
([prompts.ts:65-124](../../tmp/pi-mcp-adapter/prompts.ts),
[13h §5.3 :1896-1916](../../docs/gap-analysis/13h-mcp-tui.md)): (i) both the opening **and** closing
quote go into the token, which is the only reason `stripQuotes` exists; (ii) `eq > 0` is **strict**,
so a token beginning with `=` is positional, not a named argument with an empty key; (iii) `escaped`
is carried across iterations and never flushed, so a trailing lone backslash is **dropped**.

**Not substitutable by `cyrup-tools`' shell-word handling** — that is a POSIX grammar, which strips
quotes during splitting. This one retains them and strips later.

#### MCP-397 — `resolvePromptArgs` and the usage message · medium

`positionalIndex` advances **only when the named lookup missed** — a JS `??` short-circuit
([prompts.ts:149](../../tmp/pi-mcp-adapter/prompts.ts)). Written as
`named.get(name).or_else(|| positional.get(i++))` in Rust it advances unconditionally and shifts
every subsequent positional by one: silent wrong output, not an error. Write it as an explicit
if/else. The undeclared-named passthrough
([prompts.ts:155-160](../../tmp/pi-mcp-adapter/prompts.ts)) is deliberate and cited upstream to the
MCP spec; **do not** filter it to the declared set.

#### MCP-397a — an explicit empty named value survives for a declared optional argument · low

Loop 1 refuses to bind `""` ([prompts.ts:150](../../tmp/pi-mcp-adapter/prompts.ts)); loop 2 re-adds
**any** named key not already in `args` ([:159](../../tmp/pi-mcp-adapter/prompts.ts)); a key loop 1
rejected is by definition not in `args`. So `topic=` puts `args["topic"] = ""` on the wire for a
declared **optional** `topic`, while a declared **required** `topic` still fails the `missing` filter
([:162](../../tmp/pi-mcp-adapter/prompts.ts)) and produces the usage error. Write the two loops in
upstream's order with **no `is_empty()` guard on loop 2**. The natural Rust rewrite — filter empties
in loop 1, extend only unknown non-empty keys — silently drops the argument and is exactly the shape
a reviewer will suggest. This unit exists to stop that review comment being accepted.

#### MCP-399 — `formatPromptResult` and `extractMessageText` · medium

See Traps 2 and 4. Corrections to the spec's `??` operators, all verified against
`rmcp-3.1.4` in this workspace's registry:

* `ImageContent.mime_type` and `AudioContent.mime_type` are **non-optional `String`**
  (`model/content.rs:62-73`, `:101-112`), so `mimeType ?? "unknown"` can only fire on the **empty
  string** — port it as `if s.is_empty() { "unknown" } else { s }`, not as an `Option` fallback.
* `ImageContent.data` is likewise a non-optional `String`, so `data ? " (embedded)" : ""` is
  `!data.is_empty()`.
* `ContentBlock::ResourceLink` wraps `rmcp::model::Resource` (`model/resource.rs:12-38`), whose
  `name` is a non-optional `String` — so `content.name ? …` is `!r.name.is_empty()`, and `uri` is
  never absent, which removes upstream's `content.uri ?? ""`.
* `ContentBlock` is `#[non_exhaustive]` (`model/content.rs:250-265`), so the `_ => String::new()`
  arm is required by the compiler **and** is upstream's `default:` behaviour. That coincidence is
  worth a comment so a later reader does not "improve" it into a stringify.
* `ResourceContents` is **also** `#[non_exhaustive]` (`model/resource.rs:167-190`) and has no `uri()`
  accessor, so its match needs a wildcard too — and the only faithful answer there is the empty
  string, matching upstream's `if (!resource) return "";`.
* `Role` (`model.rs:2527-2536`) is exhaustive (`User` / `Assistant`), so `role_str` needs no
  wildcard, and `GetPromptResult.messages: Vec<PromptMessage>` (`model.rs:4180-4198`) with
  `PromptMessage { role, content }` (`model/prompt.rs:125-128`) is the input.

### The three host additions

#### MCP-041 — HA-2: dynamic argument completions · medium · `host-addition`

(13h calls this unit **MCP-382**, [:2042](../../docs/gap-analysis/13h-mcp-tui.md); the STATUS table
calls it MCP-041, [:536](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md). Same unit.)

**All three legs unmet, re-verified.** `grep -rn "argument_completions\|command_completions"
crates/cyrup-tui/src crates/cyrup-session-svc/src crates/cyrup-mcp/src` returns **nothing**.

**(a)** There is no `NativeExtension::argument_completions` — the trait runs `init`, `on_event`,
`execute_command` ([native.rs:580](../../crates/cyrup-ext/src/native.rs)), `execute_shortcut`
([:605](../../crates/cyrup-ext/src/native.rs)), `render_call` ([:617](../../crates/cyrup-ext/src/native.rs)),
`transform_markdown` ([:638](../../crates/cyrup-ext/src/native.rs)), … with no such member.

**(b)** `ExtensionHost::command_completions`
([facade.rs:1861-1868](../../crates/cyrup-ext/src/facade.rs)) is `#[cfg(feature = "wasm-host")]`,
returns `Result<Vec<String>, ExtError>` — **label-less**, so it cannot carry the `(value, label)`
pairs upstream returns — and resolves through `live_for_command`
([facade.rs:1910-1923](../../crates/cyrup-ext/src/facade.rs)), a lookup in the **live-WASM** map, so
a native built-in errors ``command `mcp` has no live owner``. It is not extendable; it needs a
sibling.

**(c)** `slash_context` ([autocomplete.rs:137-140](../../crates/cyrup-tui/src/autocomplete.rs))
returns `None` the moment the buffer contains any whitespace:

```rust
if !before.starts_with('/') || before.contains(char::is_whitespace) {
    return None;
}
```

so there is no argument-completion context to hook. `SlashCommand::has_arg_completion` exists
([commands.rs:59](../../crates/cyrup-tui/src/commands.rs)) and
`dynamic_commands_from_catalog_gated` hardcodes it `false` for **every** dynamic row
([commands.rs:487](../../crates/cyrup-tui/src/commands.rs)) with a comment saying the catalog carries
no such key. The declaration half exists and does nothing: `InitApi::add_autocomplete`
([native.rs:413](../../crates/cyrup-ext/src/native.rs)),
`ExtensionRegistry::{add_command_autocomplete, command_autocomplete}`
([registry.rs:1003](../../crates/cyrup-ext/src/registry.rs), [:1013](../../crates/cyrup-ext/src/registry.rs)).

Also in scope: deleting `"token"` from `mcp_command_descriptor`
([registration.rs:1881](../../crates/cyrup-mcp/src/registration.rs)) and correcting its doc comment
— see Trap 3.

#### MCP-362 — the 60 s inactivity auto-cancel · medium · `host-verb`

**Half met.** Both overlays hold an `Instant` deadline re-armed on every keystroke and compared in
`tick`, with `INACTIVITY_MS = 60_000` ([ui.rs:88](../../crates/cyrup-mcp/src/ui.rs)) and
`REFRESH_MS = 250` ([ui.rs:94](../../crates/cyrup-mcp/src/ui.rs)). What is unmet is the **close**:
`tick` returns `bool` ([host/overlay.rs:290-292](../../crates/cyrup-ext/src/host/overlay.rs)), so the
panel cannot ask the host to tear it down. The code says so —
[ui.rs:3260-3270](../../crates/cyrup-mcp/src/ui.rs) is an explicit `TODO(MCP-362)` and only sets
`expired`, publishes the cancelled result, and closes on the **next keystroke**
([ui.rs:3236-3239](../../crates/cyrup-mcp/src/ui.rs)); the setup panel repeats it at
[ui.rs:4595-4600](../../crates/cyrup-mcp/src/ui.rs). An untouched panel stays painted forever, which
is the opposite of what the timer is for.

#### MCP-368 + MCP-377 — overlay geometry (HA-3) and the height clip · low + low

**MCP-368 (a) unmet:** `grep -rn "OverlayOptions" crates/` returns **nothing**.
`HostServices::open_overlay` takes no options bag
([services.rs:254-256](../../crates/cyrup-ext/src/host/services.rs)) and `ExtensionOverlay`
hardcodes `OVERLAY_WIDTH_PCT = 95`, `OVERLAY_MIN_WIDTH = 60`, `OVERLAY_MAX_HEIGHT_PCT = 85`,
`OVERLAY_MARGIN = 1` ([overlay.rs:78-86](../../crates/cyrup-tui/src/overlay.rs)), resolved in
`box_rect` — which is an **associated function**, `pub(crate) fn box_rect(area, content_rows)`
([overlay.rs:112-132](../../crates/cyrup-tui/src/overlay.rs)), not a method. So the 82-column browser
panel and the 92-column setup panel — whose `MAX_VISIBLE`, description budget and hint wrap are all
designed around those numbers — paint at 95% of the terminal. Upstream's numbers are literal:
`overlayOptions: { anchor: "center", width: 82 }` for both `openMcpPanel`
([commands.ts:592](../../tmp/pi-mcp-adapter/commands.ts)) and `openMcpAuthPanel`
([commands.ts:648](../../tmp/pi-mcp-adapter/commands.ts)), and `width: 92` for `openMcpSetup`
([commands.ts:478](../../tmp/pi-mcp-adapter/commands.ts)).

**MCP-377's compact branch is already met** and correctly: `visible_action_range`
([ui.rs:4010-4020](../../crates/cyrup-mcp/src/ui.rs)) with `COMPACT_ACTION_ROWS = 7`
([ui.rs:101](../../crates/cyrup-mcp/src/ui.rs)) / `half = 3`, the `… N more above` / `… N more below`
rows, the "Add a known server" heading emitted when the preset is the first **visible** row
([ui.rs:4048-4056](../../crates/cyrup-mcp/src/ui.rs)), and the `Enter select · Esc back` hint switch
([ui.rs:4086-4093](../../crates/cyrup-mcp/src/ui.rs)).

**What is unmet is the height half**, marked `TODO(MCP-368, MCP-377)` at
[ui.rs:4028-4033](../../crates/cyrup-mcp/src/ui.rs): above `inner_w >= COMPACT_WIDTH` (60,
[ui.rs:99](../../crates/cyrup-mcp/src/ui.rs)) the action list is not windowed at all, and
`action_preview`'s output is appended unbounded
([ui.rs:4079-4083](../../crates/cyrup-mcp/src/ui.rs)). Both overlays then discard the frame height
they were handed — `fn render(&mut self, width: usize, _height: usize)` at
[ui.rs:3231](../../crates/cyrup-mcp/src/ui.rs) and [ui.rs:4571](../../crates/cyrup-mcp/src/ui.rs) —
and `ExtensionOverlay::render` clips with `.take(rect.height)`
([overlay.rs:164-168](../../crates/cyrup-tui/src/overlay.rs)), whose own contract
([host/overlay.rs:266-273](../../crates/cyrup-ext/src/host/overlay.rs)) calls that
"lossless-by-design". So on a short terminal the setup panel's hint row and bottom border simply
vanish.

These two are one change: the panel can only bound itself against the **same** geometry the host will
paint at, so the height budget must come out of the options type MCP-368 introduces.

### Out of session

#### MCP-049 — `cyrup mcp init` · medium · `hand-written`

**CLI arm unmet; most of the library is built.** `SUBCOMMANDS` is a six-element array with no `"mcp"`
([subcommands.rs:34](../../crates/cyrup/src/subcommands.rs)), `first_subcommand` matches against it
([subcommands.rs:37-44](../../crates/cyrup/src/subcommands.rs)), and **`predispatch.rs:67`** — not
`main.rs` — is what calls it, having already resolved a `ConfigDirs`
([predispatch.rs:66-73](../../crates/cyrup/src/predispatch.rs)). `ConfigDirs` carries `agent_dir`,
`cwd` and `home` ([env.rs:154-172](../../crates/cyrup-config/src/env.rs)), which is everything
`McpDirs::new` ([dirs.rs:147](../../crates/cyrup-mcp/src/dirs.rs)) and `ConfigContext::with_home`
([config.rs:2765](../../crates/cyrup-mcp/src/config.rs)) need. `crates/cyrup` already depends on
`cyrup-mcp` ([Cargo.toml:82](../../crates/cyrup/Cargo.toml)) and `cyrup-permission-system`
([Cargo.toml:68](../../crates/cyrup/Cargo.toml)).

Already built and to be reused, not rewritten:

* [`ConfigContext::find_available_import_configs`](../../crates/cyrup-mcp/src/config.rs)
  ([config.rs:4108-4121](../../crates/cyrup-mcp/src/config.rs)) — `findAvailableImports` over the
  seven `ImportKind::ALL` families ([config.rs:177-186](../../crates/cyrup-mcp/src/config.rs)),
  first existing candidate per family, with the `Failed to discover imported MCP config from {kind}:`
  warning. **It takes `&mut Vec<ConfigDiagnostic>`** — the CLI must drain and print them.
* [`ConfigContext::config_discovery_paths`](../../crates/cyrup-mcp/src/config.rs)
  ([config.rs:4090-4101](../../crates/cyrup-mcp/src/config.rs)) — `{label, exists, path}` per rung,
  nothing parsed, which is `printDiscovery`'s table.
* [`ConfigContext::ensure_compatibility_imports`](../../crates/cyrup-mcp/src/config.rs)
  ([config.rs:3750-3783](../../crates/cyrup-mcp/src/config.rs)) — idempotent, returns
  `added: Vec<ImportKind>`, and **does not write** when nothing is added.
* `set_servers_object` ([config.rs:3189-3192](../../crates/cyrup-mcp/src/config.rs)) already
  `shift_remove`s the legacy `mcp-servers` key, which is `loadPiConfig`'s normalisation
  ([cli.js:93-94](../../tmp/pi-mcp-adapter/cli.js)) — and `ensure_compatibility_imports` already
  calls it ([config.rs:3781](../../crates/cyrup-mcp/src/config.rs)).
* `read_raw_config_object` / `write_raw_config_object`
  ([config.rs:3123](../../crates/cyrup-mcp/src/config.rs),
  [:3148](../../crates/cyrup-mcp/src/config.rs)) — key-order-preserving, atomic (tmp + rename).

**The divergence is NOT what the task said.** Upstream's `writePiConfig`
([cli.js:145-148](../../tmp/pi-mcp-adapter/cli.js)) is a whole-document `JSON.stringify` overwrite.
`ensure_compatibility_imports` **merges** into the existing raw object, preserving key order and
every key it does not touch, and writes atomically. **Comments are destroyed by both** — see premise
#5. State the merge/atomicity divergence in the arm's comment; do not claim comment preservation.

**A second divergence to record:** `cli.js`'s `printDiscovery` prints a **fixed six-row** table with
capitalised labels ([cli.js:120-132](../../tmp/pi-mcp-adapter/cli.js)).
[`ConfigContext::sources`](../../crates/cyrup-mcp/src/config.rs)
([config.rs:2840-2926](../../crates/cyrup-mcp/src/config.rs)) dedupes rungs whose read path collides
(`generic != user_path`, the two `.agents` guards, `project_path != user_path`,
`project_override != …`), so it yields **four to six** rows, and its labels are lowercase except
`"Pi global override"`. Print what it returns, verbatim: do not pad to six with synthetic rows (that
would print the same path twice under two labels) and do not re-case the labels (they are the same
strings the setup panel renders).

**Genuinely missing library code:** a writer for `settings.hostConfigDiscovery = "on"`.
`grep -n "fn enable_host" crates/cyrup-mcp/src/config.rs` returns nothing. The enum exists —
[`HostConfigDiscovery`](../../crates/cyrup-mcp/src/config.rs) ([config.rs:1635-1645](../../crates/cyrup-mcp/src/config.rs)),
`#[serde(rename_all = "lowercase")]`, so the wire value is `"on"`. One small sibling of
`ensure_compatibility_imports`.

**The `install` verb has no cyrup analog and is not ported** — it prints the two retirement errors
([cli.js:205-209](../../tmp/pi-mcp-adapter/cli.js)) and exits 1.

---

## Implementation

### 1 · `crates/cyrup-mcp/src/extension.rs` — three small additions

**(a) An `init_task` accessor.** `init_task` is a **private field**
([extension.rs:94](../../crates/cyrup-mcp/src/extension.rs)) and `commands.rs` is a sibling module,
so the prologue cannot reach it. Add beside `owner()` at
[extension.rs:342-345](../../crates/cyrup-mcp/src/extension.rs):

```rust
/// `initPromise` — the in-flight build, if any. `pub(crate)` because the command prologue
/// (`crate::commands`) awaits it un-timed, unlike `on_input`, which is in this module and reads
/// the field directly.
#[must_use]
pub(crate) fn init_task(&self) -> Option<Arc<InitTask>> {
    self.init_task.lock().ok().and_then(|slot| slot.clone())
}
```

**(b) The duplicated `ConfigContext` idiom, factored.** The seven lines at
[extension.rs:196-204](../../crates/cyrup-mcp/src/extension.rs) and
[extension.rs:626-634](../../crates/cyrup-mcp/src/extension.rs) are byte-identical; `/mcp disable`
and `cyrup mcp init` want a third. Extract, and rewrite both existing sites to call it:

```rust
/// The config context this generation reads and writes through — `--mcp-config` from argv, the
/// resolved `McpDirs`, and the pinned test home. Built per call because `config_path_from_argv`
/// reads the process arguments and `load()` re-reads both files from disk, which is the property
/// `install_surface_sync` depends on.
#[must_use]
pub(crate) fn config_context(&self) -> crate::config::ConfigContext {
    let explicit = crate::config::config_path_from_argv(std::env::args()).map(PathBuf::from);
    let mut ctx = crate::config::ConfigContext::new(self.dirs.clone(), explicit.as_deref());
    if let Some(home) = self.home.clone() {
        ctx = ctx.with_home(home);
    }
    ctx
}
```

**(c) The `NativeExtension` arm**, appended to the impl that ends at
[extension.rs:789](../../crates/cyrup-mcp/src/extension.rs):

```rust
/// `index.ts`'s two `pi.registerCommand` handlers plus every prompt command, routed by name.
///
/// `ExtensionHost::execute_native_command` already builds a COMMAND-tier `HostCtx`
/// (facade.rs:556), so this call is an assertion rather than a gate — but it is the assertion
/// that documents why `ControlOp::Reload` and `SendUserMessage` are legal from here, and it
/// costs one comparison.
async fn execute_command(
    &self,
    name: &str,
    args: &str,
    ctx: &HostCtx,
) -> Result<Option<String>, ExtError> {
    ctx.require_command_tier()?;
    match name {
        crate::registration::MCP_COMMAND => self.run_mcp_command(args, ctx).await,
        crate::registration::MCP_AUTH_COMMAND => self.run_mcp_auth_command(args, ctx).await,
        // Every other registered name is a prompt command (MCP-398, Wave 7).
        other => self.run_prompt_command(other, args, ctx).await,
    }
}

/// HA-2's native leg (MCP-041). SYNC, on the TUI's keystroke path — see `crate::commands`.
fn argument_completions(&self, command: &str, prefix: &str) -> Vec<(String, String)> {
    crate::commands::argument_completions(self, command, prefix)
}
```

**Every arm returns `Ok(None)` and notifies at its own level.** The `String` channel is Info-only and
returning both prints the message twice — the contract is spelled out at
[native.rs:551-579](../../crates/cyrup-ext/src/native.rs).

### 2 · `crates/cyrup-mcp/src/commands.rs` — the prologue, the split, the switch

New module; add `pub mod commands;` to [lib.rs:132-151](../../crates/cyrup-mcp/src/lib.rs) in
alphabetical position (between `config` and `credentials`).

```rust
/// The synthetic `commandCtx` (`index.ts:505-512`), snapshotted **before the first await**.
pub struct CommandCtx {
    /// `commandHasUI` — `HostCtx::has_ui`, captured, never re-read.
    pub has_ui: bool,
    /// `ctx.mode`. `canRenderPanel` is `has_ui && mode == Tui`.
    pub mode: cyrup_ext::ExtMode,
    pub cwd: std::path::PathBuf,
    /// `ui: hasUI ? (owner ? createOwnedUi(ctx.ui, owner) : ctx.ui) : undefined`.
    ///
    /// THREE states, not two. `None` when `!has_ui` (upstream's `undefined`) even if services are
    /// bound; the RAW handle when there is no owner; the fenced handle otherwise. Collapsing the
    /// middle state makes a headless-but-ownerless build silently mute.
    pub ui: Option<Arc<dyn cyrup_ext::HostServices>>,
    /// `commandOwner`. Re-checked before EVERY side effect, not once at entry.
    pub owner: Option<Arc<McpRuntimeOwner>>,
}

impl CommandCtx {
    /// `commandOwner?.throwIfInactive()`. `false` ⇒ the arm returns without doing its work.
    fn alive(&self) -> bool {
        self.owner.as_ref().is_none_or(|owner| owner.is_active())
    }

    fn notify(&self, message: &str, kind: cyrup_ext::NotifyKind) {
        if let Some(ui) = &self.ui {
            ui.notify(message, kind);
        }
    }

    /// `commandReload` — bound at construction upstream, here the fenced `control` verb. A stopped
    /// owner's `OwnedServices::control` answers `Err(inert_reason)`, which is the fence doing its
    /// job, so the error is logged and swallowed exactly as upstream's inert proxy would.
    fn reload(&self) {
        if let Some(ui) = &self.ui
            && let Err(reason) = ui.control(cyrup_ext::ControlOp::Reload)
        {
            tracing::debug!("MCP: /reload after a config change was refused: {reason}");
        }
    }

    /// `isTuiMode(ctx)` / `canRenderPanel(ctx)` — `hasUI && mode === "tui"`
    /// (`commands.ts:40-42`). The same predicate `ContextSnapshot::is_tui_mode` spells
    /// (runtime.rs:86).
    fn can_render_panel(&self) -> bool {
        self.has_ui && self.mode == cyrup_ext::ExtMode::Tui
    }
}

/// `ctx.mode` as the string `panel_unavailable_message` / `auth_panel_unavailable_message`
/// interpolate. `ExtMode` has no `Display` (native.rs:28-34) and these two messages are the only
/// consumers, so the mapping lives here rather than widening a foreign enum.
fn mode_str(mode: cyrup_ext::ExtMode) -> &'static str {
    match mode {
        cyrup_ext::ExtMode::Tui => "tui",
        cyrup_ext::ExtMode::Rpc => "rpc",
        cyrup_ext::ExtMode::Json => "json",
        cyrup_ext::ExtMode::Print => "print",
    }
}
```

The prologue, on `McpExtension`, shared by both commands:

```rust
impl McpExtension {
    /// `index.ts:501-527`'s fenced prologue. Un-timed — unlike the tool bodies, which use
    /// `INIT_WAIT_TIMEOUT_MS` (see `on_input`, extension.rs:524-535). Returns `None` once it has
    /// already notified the user.
    async fn command_prologue(&self, ctx: &HostCtx) -> Option<(Arc<McpState>, CommandCtx)> {
        // 1-2 — snapshot everything BEFORE the first await.
        let owner = self.owner();
        let services = self.host_services();
        let cmd = CommandCtx {
            has_ui: ctx.has_ui,
            mode: ctx.mode,
            cwd: ctx.cwd.clone(),
            ui: if ctx.has_ui {
                match (services, owner.clone()) {
                    (Some(services), Some(owner)) => {
                        Some(Arc::new(OwnedServices::new(services, owner)) as Arc<dyn cyrup_ext::HostServices>)
                    }
                    // `commandOwner ? createOwnedUi(ctx.ui, commandOwner) : ctx.ui` — the RAW
                    // handle, not `None`. An ownerless extension is a test harness or a
                    // pre-`SessionStart` build; it still has a user to talk to.
                    (Some(services), None) => Some(services),
                    (None, _) => None,
                }
            } else {
                None
            },
            owner,
        };

        // 3 — `if (!state && initPromise) await initPromise` (NO timeout).
        let mut state = self.state();
        if state.is_none()
            && let Some(task) = self.init_task()
        {
            match (*task).clone().await {
                Ok(built) => {
                    // `commandOwner?.throwIfInactive()` — the post-await fence.
                    if !cmd.alive() {
                        return None;
                    }
                    state = Some(built);
                }
                Err(error) => {
                    cmd.notify(
                        &format!("MCP initialization failed: {error}"),
                        cyrup_ext::NotifyKind::Error,
                    );
                    return None;
                }
            }
        }

        // 4 — still nothing.
        let Some(state) = state else {
            cmd.notify("MCP not initialized", cyrup_ext::NotifyKind::Error);
            return None;
        };
        Some((state, cmd))
    }
}
```

The argument split, exactly ([index.ts:528-531](../../tmp/pi-mcp-adapter/index.ts)):

```rust
// `parts = args?.trim()?.split(/\s+/) ?? []` — `"".split(/\s+/)` yields `[""]`, so the
// no-argument case is `subcommand == ""`, not an empty vec.
let trimmed = args.trim();
let parts: Vec<&str> =
    if trimmed.is_empty() { vec![""] } else { trimmed.split_whitespace().collect() };
let subcommand = parts.first().copied().unwrap_or("");
let target_server = parts.get(1).copied();                           // `reconnect` uses THIS
let rest = parts.get(1..).map(|r| r.join(" ")).unwrap_or_default();   // `logout`/`disable`/`enable`
```

`/mcp logout my server` targets `"my server"`; `/mcp reconnect a b` targets `"a"`. The two are not
interchangeable.

The switch follows [index.ts:533-617](../../tmp/pi-mcp-adapter/index.ts) and
[13h §4.2 :1613-1628](../../docs/gap-analysis/13h-mcp-tui.md) exactly:

```rust
match subcommand {
    "reconnect" => {
        if !cmd.alive() { return Ok(None); }
        self.arm_reconnect(&state, &cmd, target_server).await;      // TODO(MCP-386)
        if self.direct_tools_frozen() {
            self.sync_tool_surface();
        }
    }
    "tools" => cmd.notify_multiline(show_tools(&state), NotifyKind::Info),
    "prompts" => cmd.notify_multiline(show_prompts(&state), NotifyKind::Info),
    "setup" => {
        if !cmd.alive() { return Ok(None); }
        if state.programmatic_config.is_some() {
            cmd.notify(
                "MCP setup is unavailable when config is supplied by createMcpAdapter().",
                NotifyKind::Info,
            );
        } else if self.arm_setup(&state, &cmd).await {               // TODO(MCP-387)
            if !cmd.alive() { return Ok(None); }
            cmd.reload();
            return Ok(None);                                          // an early RETURN, not a break
        }
    }
    "logout" => {
        if rest.is_empty() {
            // `hasUI`-gated upstream (index.ts:564) and an early RETURN, not a break.
            if cmd.has_ui {
                cmd.notify("Usage: /mcp logout <server>", NotifyKind::Error);
            }
            return Ok(None);
        }
        if !cmd.alive() { return Ok(None); }
        self.arm_logout(&state, &cmd, &rest).await;                   // TODO(MCP-388)
    }
    sub @ ("disable" | "enable") => self.arm_set_disabled(&state, &cmd, sub, &rest),
    // `status`, `""` and ANYTHING UNRECOGNISED share one arm: `/mcp wibble` opens the panel.
    _ => {
        if cmd.has_ui {
            if !cmd.alive() { return Ok(None); }
            if state.programmatic_config.is_some() {
                cmd.notify(
                    "MCP status is shown from the in-memory SDK config; configuration discovery is unavailable.",
                    NotifyKind::Info,
                );
                cmd.notify_multiline(show_status(&state), NotifyKind::Info);
            } else if self.arm_browser_panel(&state, &cmd).await {    // TODO(MCP-394)
                if !cmd.alive() { return Ok(None); }
                cmd.reload();
                return Ok(None);
            }
        } else {
            // `showStatus` returns immediately when `!hasUI` — so this is silent, on purpose.
            cmd.notify_multiline(show_status(&state), NotifyKind::Info);
        }
    }
}
Ok(None)
```

`notify_multiline` is `fn notify_multiline(&self, body: String, kind: NotifyKind)` on `CommandCtx`,
which skips the call when `body.is_empty()` — that is how the `!has_ui` guard inside each listing is
expressed without three duplicate `if cmd.has_ui` blocks at the call sites.

**The blocking-overlay invariant.** `arm_setup` and `arm_browser_panel` end in
`HostServices::open_overlay`, which **blocks the calling task** via `block_in_place` until the modal
tears down ([host_services.rs:1043-1090](../../crates/cyrup-session-svc/src/host_services.rs)). That
is legal here and only here because `AppAction::Command` reaches `execute_command` through a
channel-back rather than inline on the TUI run-loop task — the invariant that file's comment states
and enforces at the call sites. Do not "helpfully" move any of these arms onto the run loop.

### 3 · `crates/cyrup-mcp/src/commands.rs` — MCP-383/384/385/385a, pure renderers

Put the three listing bodies in `commands.rs` as **pure functions returning `String`** (empty string
= "nothing to say"), so the arm is one `notify_multiline` and the text is exercisable without a host.

```rust
/// `commands.ts:44-88` `showStatus` (13h §4.4). One multi-line Info notification.
///
/// UI-GATED, not headless: upstream's first line is `if (!ctx.hasUI) return;`, and the `/mcp`
/// default arm's no-UI branch calls this anyway — so a print/json `/mcp` prints nothing. The empty
/// return expresses that here.
///
/// The ladder is `proxy::discovery::execute_status`'s six rungs (proxy/discovery.rs:34) with
/// `showStatus`'s OWN text: upstream keeps two renderers over one state machine and so does this.
#[must_use]
pub fn show_status(state: &McpState, has_ui: bool) -> String {
    if !has_ui {
        return String::new();
    }
    let mut lines = vec!["MCP Server Status:".to_string(), String::new()];
    for (name, definition) in &state.config.mcp_servers {
        if definition.is_disabled() {
            lines.push(format!("\u{2298} {name}: disabled (run /mcp enable {name}, then /reload)"));
            continue; // no tool suffix, ever
        }
        let connection = state.manager.get_connection(name);
        let status_of = connection.as_ref().map(|c| c.status());
        let metadata_len = state
            .tool_metadata
            .lock()
            .ok()
            .and_then(|m| m.get(name).map(|entry| entry.tool_names.len()));
        let failed_ago = crate::env::failure_age_seconds(state, name);

        // FIRST MATCH WINS, and `failed` is tested BEFORE `cached` — a failed server must never
        // report `cached` even when its metadata is present.
        let (icon, status, failed) = if status_of == Some(ConnectionStatus::Connected) {
            ("\u{2713}", "connected".to_string(), false)
        } else if status_of == Some(ConnectionStatus::NeedsAuth) {
            ("\u{26a0}", "needs auth".to_string(), false)
        } else if let Some(secs) = failed_ago {
            let reason = crate::ui::sanitize_terminal_text(
                &crate::env::failure_message(state, name).unwrap_or_default(),
            );
            let text = if reason.is_empty() {
                format!("failed {secs}s ago")
            } else {
                format!("failed {secs}s ago \u{2014} {reason}")
            };
            ("\u{2717}", text, true)
        } else if metadata_len.is_some() {
            ("\u{25cb}", "cached".to_string(), false)
        } else {
            ("\u{25cb}", "not connected".to_string(), false)
        };

        // `tools` is NEVER singularised.
        let suffix = if failed {
            String::new()
        } else {
            let cached = if status == "cached" { ", cached" } else { "" };
            format!(" ({} tools{cached})", metadata_len.unwrap_or(0))
        };
        lines.push(format!("{icon} {name}: {status}{suffix}"));
    }
    if state.config.mcp_servers.is_empty() {
        lines.push("No MCP servers configured".to_string());
        lines.push("Run /mcp setup to adopt imports or scaffold a starter .mcp.json".to_string());
    }
    lines.join("\n")
}
```

`ConnectionStatus` here is [`crate::lifecycle::ConnectionStatus`](../../crates/cyrup-mcp/src/lifecycle.rs)
([lifecycle.rs:148-155](../../crates/cyrup-mcp/src/lifecycle.rs)) — the three-variant one
`ServerConnection::status()` returns — **not** `crate::ui::ConnectionStatus`
([ui.rs:1167-1180](../../crates/cyrup-mcp/src/ui.rs)), which is the panel's six-variant view, and not
`crate::proxy::env::ConnectionStatus`. Three types with the same name; import the right one and say
which in a `use` comment.

Read these through `McpState` directly, **not** through a `ProxyCtx`: the listing is not a proxy mode
and building a `ProxyCtx` for it would drag the whole 30-method `ProxyEnv` trait
([proxy/env.rs:248-320](../../crates/cyrup-mcp/src/proxy/env.rs)) into the command path.

`show_tools` ([commands.ts:127-148](../../tmp/pi-mcp-adapter/commands.ts)) is the same shape, with
the disabled filter that premise #4 restored:

```rust
let all_tools: Vec<String> = state
    .tool_metadata
    .lock()
    .ok()
    .map(|metadata| {
        metadata
            .iter()
            // `!isServerDisabled(state.config.mcpServers[serverName])` — a server absent from the
            // config is NOT disabled (`isServerDisabled(undefined)` is falsy), so a stale metadata
            // entry still lists. Keep that: `is_none_or`, not `is_some_and`.
            .filter(|(server, _)| {
                state.config.mcp_servers.get(*server).is_none_or(|d| !d.is_disabled())
            })
            .flat_map(|(_, entry)| entry.tool_names.iter().cloned())
            .collect()
    })
    .unwrap_or_default();
if all_tools.is_empty() {
    return "No MCP tools available".to_string();   // NOT the header block with a zero total
}
```

then `["MCP Tools:", "", ...names.map(|n| format!("  {n}")), "", format!("Total: {} tools", n)]`.

For `show_prompts`, group with an `IndexMap<String, Vec<PromptMetadata>>` clone (grouping is rebuilt
per call, so the in-place sort is harmless — **do not** "optimise" it into a shared cache), then:

```rust
let mut servers: Vec<&String> = grouped.keys().collect();
// CYRUP-DELTA: `String.localeCompare` with no locale is ICU root collation; this is byte order.
// They agree for ASCII-lowercase names and disagree on mixed case (`Foo` vs `bar`). Accepted
// rather than pulling a collation crate in for one sort.
servers.sort_by(|a, b| a.as_str().cmp(b.as_str()));
for server in servers {
    lines.push(format!("{server}:"));           // MCP-385a: unindented, no icon, UNSANITIZED
    let Some(group) = grouped.get(server) else { continue };
    let mut prompts = group.clone();
    prompts.sort_by(|a, b| a.command_name.cmp(&b.command_name));
    for p in &prompts {
        let usage: String = p
            .arguments
            .iter()
            .map(|a| {
                if a.required.unwrap_or(false) {
                    format!("<{}>", a.name)
                } else {
                    format!("[{}]", a.name)
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(if usage.is_empty() {
            format!("  /{}", p.command_name)
        } else {
            format!("  /{} {usage}", p.command_name)
        });
        if !p.description.is_empty() {
            lines.push(format!("      {}", p.description)); // SIX spaces
        }
    }
    lines.push(String::new());                   // per-group blank line
}
```

`Total: {N} prompt{s}` is singular **only at 1**; `Total: {N} tools` is never singularised. The two
rules are different on purpose. The failed-discovery servers come from
`state.manager.get_all_connections()` filtered to `status() == Connected &&
prompt_discovery_failed()`, sorted; the empty-list note is appended to the **same sentence** with a
leading space, and the non-empty one is its own final line.

### 4 · `crates/cyrup-mcp/src/state.rs` — `PromptMetadata`'s real body

Replace the stub at [state.rs:373-382](../../crates/cyrup-mcp/src/state.rs). Keep
`#[non_exhaustive]`; drop `Default` only if a construction site needs it (nothing in this task
constructs one).

```rust
/// `prompts.ts`'s per-prompt metadata: the prompt's name, its arguments and the slash command it
/// becomes — upstream `PromptMetadata` (`types.ts:584-591`), field for field.
///
/// Deliberately the SAME six fields as
/// [`crate::registration::PromptCommandSpec`](crate::registration::PromptCommandSpec): the cache
/// path (`resolve_cached_prompts`) and the live `prompts/list` path must produce one shape, because
/// `findLivePromptMetadata` re-resolves one against the other at invocation time (§5.5).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PromptMetadata {
    pub server_name: String,
    /// The MCP prompt name, as the server reported it.
    pub original_name: String,
    /// The slash command this prompt registers, without the leading `/`.
    pub command_name: String,
    pub title: Option<String>,
    /// Empty, not `None`, for an undescribed prompt — upstream's `description: string`.
    pub description: String,
    pub arguments: Vec<crate::registration::CachedPromptArgument>,
}
```

`CachedPromptArgument` is [registration.rs:728-737](../../crates/cyrup-mcp/src/registration.rs)
(`name`, `description: Option<String>`, `required: Option<bool>`) and is already `Clone + Default +
PartialEq + Eq + Serialize + Deserialize`. MCP-039/MCP-395a populate the map; this task only reads it.

### 5 · `crates/cyrup-mcp/src/prompts.rs` — MCP-396/397/397a/399

New module (`pub mod prompts;` in `lib.rs`, between `owner` and `proxy`). Pure functions over
`&[CachedPromptArgument]` and `rmcp::model::GetPromptResult` — **no `McpState`, no host** — so
MCP-398's handler is a caller and these stand alone.

```rust
/// `prompts.ts:65-103` `tokenizeArgs` (13h §5.3). The quote characters STAY in the token;
/// `strip_quotes` removes them later. That is the whole reason `strip_quotes` exists.
fn tokenize_args(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        // Backslash is LITERAL inside single quotes.
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            current.push(ch);
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            current.push(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    // `escaped` is consumed across iterations and never flushed, so a TRAILING LONE BACKSLASH is
    // dropped. Upstream behaviour (prompts.ts:101); do not "fix" it.
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// `findUnquotedEquals` (prompts.ts:105-117) — the BYTE index of the first `=` outside quotes.
/// Byte, not char: the caller slices with it, and every quote/`=` this scans is ASCII, so the two
/// agree wherever it returns `Some`.
fn find_unquoted_equals(token: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (index, ch) in token.char_indices() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if ch == '=' {
            return Some(index);
        }
    }
    None
}

/// `stripQuotes` (prompts.ts:119-124) — strip exactly one layer when the value is at least two
/// chars and its first and last CHARACTERS are the same quote.
fn strip_quotes(value: &str) -> &str {
    let mut chars = value.chars();
    let (Some(first), Some(last)) = (chars.next(), chars.next_back()) else {
        return value; // fewer than two characters
    };
    if (first == '"' || first == '\'') && first == last {
        return chars.as_str();
    }
    value
}

/// `parsePromptArgs`'s return (prompts.ts:44). `IndexMap` because loop 2 of `resolve_prompt_args`
/// iterates it and JS object key order is insertion order.
pub struct ParsedPromptArgs {
    pub named: IndexMap<String, String>,
    pub positional: Vec<String>,
}

#[must_use]
pub fn parse_prompt_args(input: &str) -> ParsedPromptArgs {
    let mut out = ParsedPromptArgs { named: IndexMap::new(), positional: Vec::new() };
    for token in tokenize_args(input) {
        // `eq > 0` is STRICT: a token starting with `=` is POSITIONAL, not a named arg with an
        // empty key. And a whitespace-only key falls THROUGH to positional (upstream's `if (key)`
        // guard sits inside the `eq > 0` branch and does not `continue` when it fails).
        let named = find_unquoted_equals(&token)
            .filter(|eq| *eq > 0)
            .and_then(|eq| {
                let key = token.get(..eq)?.trim();
                if key.is_empty() {
                    return None;
                }
                let value = strip_quotes(token.get(eq + 1..)?.trim());
                Some((key.to_string(), value.to_string()))
            });
        match named {
            Some((key, value)) => {
                out.named.insert(key, value);
            }
            None => out.positional.push(strip_quotes(&token).to_string()),
        }
    }
    out
}
```

Binding — the two loops, in order, **with no guard added to loop 2** (MCP-397a):

```rust
/// `resolvePromptArgs` (prompts.ts:140-168). `Err` is `buildUsageMessage`'s text, which the caller
/// notifies at `Error`.
pub fn resolve_prompt_args(
    declared: &[CachedPromptArgument],
    command_name: &str,
    parsed: &ParsedPromptArgs,
) -> Result<IndexMap<String, String>, String> {
    let mut args: IndexMap<String, String> = IndexMap::new();
    let mut positional_index = 0usize;

    // LOOP 1 — declaration order. The positional cursor advances ONLY on a named MISS: upstream's
    // `??` short-circuits before evaluating `positional[positionalIndex++]`. Written as
    // `.or_else(|| positional.get(i))` with an unconditional bump, every later positional shifts
    // by one — silent wrong output, not an error.
    for arg in declared {
        let value = match parsed.named.get(&arg.name) {
            Some(value) => Some(value.clone()),
            None => {
                let value = parsed.positional.get(positional_index).cloned();
                positional_index += 1;
                value
            }
        };
        if let Some(value) = value
            && !value.is_empty()
        {
            args.insert(arg.name.clone(), value);
        }
    }

    // LOOP 2 — undeclared named args are forwarded UNFILTERED (cited upstream to the MCP spec's
    // allowance of arbitrary string key/values in `prompts/get` params).
    //
    // MCP-397a: NO `is_empty()` guard here. `topic=` is rejected by loop 1 and is therefore not in
    // `args`, so it lands as `args["topic"] = ""` for a declared OPTIONAL argument, while a
    // declared REQUIRED one still fails the `missing` filter below.
    for (key, value) in &parsed.named {
        if !args.contains_key(key) {
            args.insert(key.clone(), value.clone());
        }
    }

    let missing: Vec<&str> = declared
        .iter()
        .filter(|a| a.required.unwrap_or(false))
        .filter(|a| args.get(&a.name).is_none_or(String::is_empty))
        .map(|a| a.name.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(build_usage_message(declared, command_name, &missing));
    }
    Ok(args)
}

/// `buildUsageMessage` (prompts.ts:170-176):
/// `Missing required argument{s}: {names}.\nUsage: /{command_name} {usage}`, trimmed, with `{s}`
/// only when more than one is missing. The trim is what removes the trailing space when the prompt
/// declares no arguments at all.
fn build_usage_message(
    declared: &[CachedPromptArgument],
    command_name: &str,
    missing: &[&str],
) -> String {
    let usage = declared
        .iter()
        .map(|a| {
            if a.required.unwrap_or(false) {
                format!("<{}>", a.name)
            } else {
                format!("[{}]", a.name)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let plural = if missing.len() > 1 { "s" } else { "" };
    format!(
        "Missing required argument{plural}: {}.\nUsage: /{command_name} {usage}",
        missing.join(", ")
    )
    .trim()
    .to_string()
}
```

Flattening (MCP-399):

```rust
/// `prompts.ts:185-197` `formatPromptResult` (13h §5.6). NOT
/// [`crate::renderers::transform_mcp_content`] (renderers.rs:616) — that is tool-result shaping
/// over `serde_json::Value`, with different casing, different bracket text and an unknown arm that
/// re-serializes JSON instead of contributing nothing.
#[must_use]
pub fn format_prompt_result(result: &GetPromptResult) -> String {
    let single = result.messages.len() == 1;
    let mut lines: Vec<String> = Vec::new();
    for message in &result.messages {
        let text = extract_message_text(&message.content);
        if text.is_empty() {
            continue;
        }
        // A lone USER message is emitted bare; everything else — including a lone ASSISTANT
        // message — keeps its `[role] ` prefix.
        if single && message.role == Role::User {
            lines.push(text);
        } else {
            lines.push(format!("[{}] {text}", role_str(&message.role)));
        }
    }
    lines.join("\n\n").trim().to_string()
}

/// `Role` is exhaustive in rmcp (`model.rs:2527-2536`), so no wildcard — and the two spellings are
/// its `serde(rename_all = "camelCase")` wire values, which is what upstream interpolates.
fn role_str(role: &Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// `extractMessageText` (prompts.ts:199-222).
fn extract_message_text(content: &ContentBlock) -> String {
    match content {
        ContentBlock::Text(t) => t.text.clone(),
        ContentBlock::Resource(r) => match &r.resource {
            ResourceContents::TextResourceContents { uri, text, .. } => {
                format!("[resource {uri}]\n{text}")
            }
            ResourceContents::BlobResourceContents { uri, .. } => format!("[resource {uri}]"),
            // `ResourceContents` is `#[non_exhaustive]` (rmcp `model/resource.rs:167-171`) and has
            // no `uri()` accessor, so this arm is REQUIRED by the compiler even though 3.1.4 has
            // exactly the two above. Upstream answers `""` for a resource shape it cannot read
            // (`if (!resource) return ""`), so this is the faithful value, not a placeholder.
            _ => String::new(),
        },
        // Em dash, not a hyphen. rmcp's `Resource.name`/`.uri` are non-optional `String`
        // (`model/resource.rs:12-38`), so upstream's `uri ?? ""` cannot fire and `name ?` is an
        // emptiness test.
        ContentBlock::ResourceLink(r) => {
            if r.name.is_empty() {
                format!("[resource_link {}]", r.uri)
            } else {
                format!("[resource_link {} \u{2014} {}]", r.uri, r.name)
            }
        }
        // rmcp's `mime_type`/`data` are non-optional `String` (`model/content.rs:62-73`,
        // `:101-112`), so upstream's `?? "unknown"` and `data ? …` become empty-string tests, not
        // `Option` fallbacks.
        ContentBlock::Image(i) => {
            let mime = if i.mime_type.is_empty() { "unknown" } else { i.mime_type.as_str() };
            let embedded = if i.data.is_empty() { "" } else { " (embedded)" };
            format!("[image {mime}{embedded}]")
        }
        ContentBlock::Audio(a) => {
            let mime = if a.mime_type.is_empty() { "unknown" } else { a.mime_type.as_str() };
            format!("[audio {mime}]")
        }
        // `ContentBlock` is `#[non_exhaustive]` (`model/content.rs:250-258`), so this arm is
        // required by the compiler AND is upstream's `default: return ""`. Do not turn it into a
        // stringify — that is `transform_mcp_content`'s rule, not this one's.
        _ => String::new(),
    }
}
```

### 6 · `crates/cyrup-mcp/src/env.rs` — the missing reader

The spine introduces `crate::env` with `clear_failure`, `record_failure` and `failure_age_seconds`
([MCP_RUNTIME_INIT_SPINE.md:649-739](MCP_RUNTIME_INIT_SPINE.md)). It writes `failure_messages`
([:682-683](MCP_RUNTIME_INIT_SPINE.md)) and never reads it. Add the reader beside
`failure_age_seconds`:

```rust
/// `getFailureMessage(state, serverName)` (`init.ts:612-615`).
///
/// Gated on `failure_age_seconds` returning `Some`, NOT on the map being populated: a message whose
/// failure has aged out of the 60 s window must not be reported, and the expiry task that clears the
/// map is best-effort. Dropping the gate makes `/mcp status` print a stale reason next to a server
/// that is no longer in backoff.
#[must_use]
pub fn failure_message(state: &McpState, server: &str) -> Option<String> {
    failure_age_seconds(state, server)?;
    state.failure_messages.lock().ok()?.get(server).cloned()
}
```

### 7 · `crates/cyrup-mcp/src/commands.rs` — MCP-042/334/389/391

**`/mcp-auth`'s handler**, in upstream's order
([index.ts:622-671](../../tmp/pi-mcp-adapter/index.ts)):

```rust
async fn run_mcp_auth_command(
    &self,
    args: &str,
    ctx: &HostCtx,
) -> Result<Option<String>, ExtError> {
    let server_name = args.trim();
    // THE ORDERING DETAIL: this fires BEFORE the prologue, so a headless `/mcp-auth` with no
    // argument never awaits `init_task`. Writing the prologue first and the bail second is the
    // natural Rust order and is wrong (index.ts:637-639).
    if server_name.is_empty() && !ctx.has_ui {
        return Ok(None);
    }
    let Some((state, cmd)) = self.command_prologue(ctx).await else { return Ok(None) };

    if server_name.is_empty() {
        if state.programmatic_config.is_some() {
            cmd.notify(
                "Use /mcp-auth <server> to authenticate a server from the in-memory SDK config.",
                NotifyKind::Info,
            );
            return Ok(None);
        }
        // MCP-391. Callbacks are MCP-392's; this call site fixes their type.
        open_mcp_auth_panel(&state, &cmd, self.arm_panel_callbacks(&state, &cmd));
        return Ok(None);
    }

    if authenticate_server(server_name, &state, &cmd).await.is_ok() {
        if !cmd.alive() {
            return Ok(None);
        }
        self.arm_reconnect_one(&state, &cmd, server_name).await;    // TODO(MCP-386)
    }
    Ok(None)
}
```

**MCP-334's dispatch**, the five guards then the flow
([commands.ts:244-333](../../tmp/pi-mcp-adapter/commands.ts)). Every literal comes from
`crate::oauth`; only the two-line notify variant and the hyperlink are new:

```rust
/// `authenticateServer` (commands.ts:244-333) — MCP-334's guard order and status key.
///
/// `Err(message)` carries upstream's `{ok:false, message}`: the guards notify and then return the
/// SAME text, because two callers (`/mcp-auth <server>` and the panel's `authenticate` callback)
/// need it as a value, not just on screen.
pub async fn authenticate_server(
    server_name: &str,
    state: &McpState,
    cmd: &CommandCtx,
) -> Result<String, String> {
    // 1 — no interactive UI. Returns the message WITHOUT notifying: there is nowhere to notify.
    if cmd.ui.is_none() {
        return Err(crate::oauth::MSG_REQUIRES_INTERACTIVE.to_string());
    }
    // 2 — unknown server.
    let Some(definition) = state.config.mcp_servers.get(server_name) else {
        let message = crate::oauth::msg_server_not_found(server_name);
        cmd.notify(&message, NotifyKind::Error);
        return Err(message);
    };
    // 3 — disabled. WARNING, not Error (commands.ts:263).
    if definition.is_disabled() {
        let message = crate::oauth::msg_server_disabled(server_name);
        cmd.notify(&message, NotifyKind::Warning);
        return Err(message);
    }
    // 4 — not an OAuth server. The NOTIFIED text is two lines; the RETURNED message is one
    // (commands.ts:268-274). Two literals, deliberately — see the doc note at oauth.rs:3815.
    if !crate::oauth::supports_oauth(definition) {
        let message = crate::oauth::msg_not_oauth(server_name);
        cmd.notify(&crate::oauth::msg_not_oauth_notify(server_name), NotifyKind::Error);
        return Err(message);
    }
    // 5 — no URL. `resolve_server_url` can THROW; upstream's `try` wraps from here, so a throw
    // lands on the `msg_auth_threw` arm rather than the `msg_no_url` one.
    let url = match crate::credentials::resolve_server_url(definition.url.as_deref(), &env_fn) {
        Ok(Some(url)) => url,
        Ok(None) => {
            let message = crate::oauth::msg_no_url(server_name);
            cmd.notify(&message, NotifyKind::Error);
            return Err(message);
        }
        Err(error) => {
            let message = crate::oauth::msg_auth_threw(server_name, &error.to_string());
            cmd.notify(&message, NotifyKind::Error);
            return Err(message);
        }
    };

    // The status key, set before the flow and cleared in the `finally` UNLESS the signal aborted.
    let signal = cmd.owner.as_ref().map(|owner| owner.token());
    if let Some(ui) = &cmd.ui {
        ui.set_status("mcp-auth", Some(&crate::oauth::msg_authenticating(server_name)));
    }

    let options = crate::oauth::AuthenticateOptions {
        runtime: Some(Arc::clone(&state.oauth_runtime)),
        signal: signal.clone(),
        on_authorization_url: Some(url_hook(cmd, server_name)),
        on_authorization_input: Some(input_hook(cmd, server_name)),
        ..crate::oauth::AuthenticateOptions::new(auth_storage(state))
    };
    let outcome = crate::oauth::authenticate(server_name, &url, Some(definition), &options).await;

    let aborted = signal.as_ref().is_some_and(cyrup_core::CancelToken::is_cancelled);
    // `finally { if (!signal?.aborted) ui.setStatus("mcp-auth", undefined) }` — an aborted flow
    // LEAVES the status set, which is what the 13g verification asserts.
    if !aborted && let Some(ui) = &cmd.ui {
        ui.set_status("mcp-auth", None);
    }
    // …then the three outcome arms: `Authenticated` ⇒ `msg_auth_success` at Info and `Ok`;
    // any other status ⇒ `msg_auth_failed` at Error; an `Err` with a live signal ⇒ rethrow,
    // otherwise `msg_auth_threw` at Error.
}
```

The two hooks are where MCP-390's OSC-8 lands; write `terminal_hyperlink` here because both use it:

```rust
/// `terminalHyperlink(label, url)` (commands.ts:27-29) — OSC 8 with both halves sanitized FIRST.
///
/// The order matters: `sanitize_terminal_text` (ui.rs:376) strips OSC-8, so sanitizing the finished
/// sequence would erase the link. Sanitize the parts, then build the escape.
fn terminal_hyperlink(label: &str, url: &str) -> String {
    let url = crate::ui::sanitize_terminal_text(url);
    let label = crate::ui::sanitize_terminal_text(label);
    format!("\u{1b}]8;;{url}\u{1b}\\{label}\u{1b}]8;;\u{1b}\\")
}
```

`msg_not_oauth_notify` is the one new literal in `oauth.rs`, beside `msg_not_oauth` at
[oauth.rs:3817](../../crates/cyrup-mcp/src/oauth.rs):

```rust
/// [`msg_not_oauth`]'s `notify` variant — the SAME sentences, broken after the first with `\n`
/// (`commands.ts:269-273`). Two functions because the returned `message` and the notified text are
/// genuinely different strings upstream, and the panel surfaces the former.
#[must_use]
pub fn msg_not_oauth_notify(server_name: &str) -> String {
    format!(
        "Server \"{server_name}\" does not use OAuth authentication.\nSet \"auth\": \"oauth\" or omit auth for auto-detection."
    )
}
```

**MCP-389's arm** — four notifies and a call:

```rust
/// `/mcp disable` / `/mcp enable` (index.ts:569-597). Both refusals and both outcomes `break`;
/// none of them returns early.
fn arm_set_disabled(&self, state: &McpState, cmd: &CommandCtx, sub: &str, name: &str) {
    if state.programmatic_config.is_some() {
        cmd.notify(
            &format!("/mcp {sub} is unavailable when config is supplied by createMcpAdapter()."),
            NotifyKind::Info,
        );
        return;
    }
    if name.is_empty() {
        cmd.notify(&format!("Usage: /mcp {sub} <server>"), NotifyKind::Error);
        return;
    }
    if !state.config.mcp_servers.contains_key(name) {
        cmd.notify(&format!("Server \"{name}\" not found in effective config"), NotifyKind::Error);
        return;
    }
    if !cmd.alive() {
        return;
    }
    let disabled = sub == "disable";
    // The writer owns the file, the key spelling, the no-op detection and its four error strings
    // (config.rs:3484-3570). Never re-derive any of them here.
    match self.config_context().write_project_server_disabled_override(name, disabled) {
        Ok(result) if result.changed => cmd.notify(
            &format!(
                "{} server \"{name}\" in {} \u{2014} run /reload to apply",
                if disabled { "Disabled" } else { "Enabled" },
                result.path.display()
            ),
            NotifyKind::Info,
        ),
        Ok(_) => cmd.notify(
            &format!(
                "Server \"{name}\" is already {}",
                if disabled { "disabled" } else { "enabled" }
            ),
            NotifyKind::Info,
        ),
        // Upstream lets the throw escape into pi's command-error path. cyrup's equivalent is the
        // handler's own Error notify, which keeps the writer's exact message intact instead of
        // wrapping it in `command:mcp: `.
        Err(error) => cmd.notify(&error.to_string(), NotifyKind::Error),
    }
}
```

This is the **only** subcommand that tells the user to run `/reload` themselves.

**MCP-391's entry point** — four guards, then the shipped opener:

```rust
/// `openMcpAuthPanel` (commands.ts:605-653). Always reports `config_changed == false`, even after a
/// successful authentication — the panel writes no config.
///
/// `callbacks` is a parameter, not a construction: `build_mcp_panel_callbacks` is MCP-392's and
/// there is no production implementor of `McpPanelCallbacks` yet (only the three test stubs at
/// ui.rs:5028/5385/5525).
pub fn open_mcp_auth_panel(
    state: &McpState,
    cmd: &CommandCtx,
    callbacks: Arc<dyn crate::ui::McpPanelCallbacks>,
) {
    let Some(ui) = &cmd.ui else { return };            // 1 — `!hasUI`, silently
    if !cmd.can_render_panel() {                        // 2 — a UI, but not a terminal one
        ui.notify(&crate::ui::auth_panel_unavailable_message(mode_str(cmd.mode)), NotifyKind::Info);
        return;
    }
    if state.programmatic_config.is_some() {            // 3
        ui.notify(
            "Use /mcp-auth <server> to authenticate a server from the in-memory SDK config.",
            NotifyKind::Info,
        );
        return;
    }
    // 4 — zero OAuth-capable servers. `are configured`, WARNING, and NO panel. Distinct from the
    // panel's own empty-body row `No OAuth-capable MCP servers configured.` (ui.rs:2670), which is
    // rendered INSIDE an open panel. Two strings, two surfaces.
    let any_oauth = state
        .config
        .mcp_servers
        .values()
        .any(|d| !d.is_disabled() && crate::oauth::supports_oauth(d));
    if !any_oauth {
        ui.notify("No OAuth-capable MCP servers are configured.", NotifyKind::Warning);
        return;
    }

    let model = crate::ui::McpPanelModel::new(
        &state.config,
        crate::dirs::load_metadata_cache(self.dirs()),
        &provenance,
        callbacks,
        crate::ui::PanelOptions {
            auth_only: true,
            notice_lines: vec![crate::ui::AUTH_PANEL_NOTICE.to_string()],
            ..crate::ui::PanelOptions::default()
        },
    );
    // The shipped opener (ui.rs:4805) already solves getting the result back out of a `bool`.
    let _ = crate::ui::open_mcp_panel(ui.as_ref(), model, callbacks, handle);
}
```

### 8 · MCP-041 — HA-2, three legs

**(a) `cyrup-ext`.** A defaulted, **synchronous** method on `NativeExtension`, beside
`execute_shortcut` at [native.rs:605](../../crates/cyrup-ext/src/native.rs):

```rust
/// Dynamic argument completions for a command this extension registered (pi
/// `getArgumentCompletions(argumentPrefix): AutocompleteItem[] | null`,
/// `extensions/types.ts:1166` @v0.83.0). `(value, label)` — the label is what the popup shows,
/// the value is what is inserted. Empty = "no completions", which is pi's `null`.
///
/// SYNC on purpose, like `render_call`: it runs on the TUI's input path and an `async`
/// completer would stall the keystroke.
fn argument_completions(&self, _command: &str, _prefix: &str) -> Vec<(String, String)> {
    Vec::new()
}
```

and a non-`cfg`-gated facade verb beside `execute_native_command`
([facade.rs:533](../../crates/cyrup-ext/src/facade.rs)), routing through the **native** map exactly
as that function does:

```rust
/// The native arm of pi's `getArgumentCompletions`. [`Self::command_completions`] (`:1861`) is the
/// live-WASM arm and cannot serve a native for two reasons: `live_for_command` looks in the
/// live-WASM map, so a built-in errors ``command `X` has no live owner``; and its return type is
/// `Vec<String>`, which cannot carry pi's `{value, label}` pair at all.
pub fn native_command_completions(&self, name: &str, prefix: &str) -> Vec<(String, String)> {
    let Ok(Some((owner, registered))) = self.command_route(name) else { return Vec::new() };
    let Some(ext) = self.native.read().ok().and_then(|g| g.get(&owner).cloned()) else {
        return Vec::new();
    };
    ext.argument_completions(&registered, prefix)
}
```

`command_route` is already private-but-present at
[facade.rs:1899-1907](../../crates/cyrup-ext/src/facade.rs) and already carries the SEAM-048
registered name.

**(b) `cyrup-mcp`.** Implement it in `commands.rs`, reading **live** config, and delete `"token"`:

```rust
/// The eight rows of `getArgumentCompletions`' first branch (index.ts:476-485). Em dashes are
/// literal; the order is user-visible.
const MCP_SUBCOMMANDS: [(&str, &str); 8] = [
    ("reconnect", "reconnect \u{2014} Reconnect servers"),
    ("tools",     "tools \u{2014} List all tools"),
    ("prompts",   "prompts \u{2014} List all MCP prompts"),
    ("setup",     "setup \u{2014} Configure MCP servers"),
    ("logout",    "logout \u{2014} Clear server credentials"),
    ("disable",   "disable \u{2014} Disable a server"),
    ("enable",    "enable \u{2014} Enable a server"),
    ("status",    "status \u{2014} Show server status"),
];

pub(crate) fn argument_completions(
    ext: &McpExtension,
    command: &str,
    prefix: &str,
) -> Vec<(String, String)> {
    // `/mcp-auth` deliberately declares NO completer upstream — an asymmetry with `/mcp`, kept
    // (13h §4.3, and `mcp_auth_command_descriptor`'s empty `completions` at registration.rs:1892).
    if command != crate::registration::MCP_COMMAND {
        return Vec::new();
    }
    let normalized = prefix.trim_start();
    // `normalized.match(/^(\S+)\s+(.*)$/)` — a non-space run, whitespace, then the REST (possibly
    // empty), which is why `split_once` on whitespace is the exact equivalent.
    let Some((sub, arg_prefix)) = normalized.split_once(char::is_whitespace) else {
        return MCP_SUBCOMMANDS
            .iter()
            .filter(|(value, _)| value.starts_with(normalized))
            .map(|(value, label)| ((*value).to_string(), (*label).to_string()))
            .collect();
    };
    if !matches!(sub, "reconnect" | "logout" | "disable" | "enable") {
        return Vec::new();
    }
    // `|| !state` — a completer that fires before initialization offers nothing rather than
    // falling back to a config re-read, because upstream reads LIVE state here.
    let Some(state) = ext.state() else { return Vec::new() };
    let arg_prefix = arg_prefix.trim_start();
    state
        .config
        .mcp_servers
        .keys()
        .filter(|name| name.starts_with(arg_prefix))
        .map(|name| (format!("{sub} {name}"), name.clone()))
        .collect()
}
```

Filtering is literal `starts_with`, **not** `fuzzy::filter` — upstream uses `String.startsWith` on
both branches. Upstream's `null`-vs-`[]` distinction collapses harmlessly into "empty": the TUI
already treats an empty candidate set as "no popup"
([autocomplete.rs:142-144](../../crates/cyrup-tui/src/autocomplete.rs)).

Then **delete `"token"`** from [registration.rs:1881](../../crates/cyrup-mcp/src/registration.rs),
reduce the array to the eight values above, and rewrite the doc comment at
[registration.rs:1870-1876](../../crates/cyrup-mcp/src/registration.rs) — it currently says "nine
static subcommands" and lists `token` among the runtime-resolved names.

**(c) `cyrup-tui`.** A synchronous completer handle plus a fourth completion context.

In [commands.rs](../../crates/cyrup-tui/src/commands.rs):

```rust
/// A live argument completer for a registered slash command (pi's per-command
/// `getArgumentCompletions` closure, which cannot cross an FFI boundary as a closure).
/// `(value, label)`: the label is displayed, the value is what replaces the argument text.
pub trait ArgumentCompleter: Send + Sync + std::fmt::Debug {
    fn complete(&self, command: &str, prefix: &str) -> Vec<(String, String)>;
}
```

In [autocomplete.rs](../../crates/cyrup-tui/src/autocomplete.rs): add
`CompletionContext::Argument` beside the three at
[:26-36](../../crates/cyrup-tui/src/autocomplete.rs); give `apply`
([:97-133](../../crates/cyrup-tui/src/autocomplete.rs)) the arm
`CompletionContext::Argument => (completion.value.clone(), " ")` (the value already carries its
leading `/`, so unlike `Slash` nothing is re-prefixed); and add:

```rust
/// `/name arg…` — the argument-completion context `slash_context` bails out of (`:138`).
fn argument_context(
    registry: &CommandRegistry,
    completer: Option<&dyn ArgumentCompleter>,
    before: &str,
) -> Option<Autocomplete> {
    let completer = completer?;
    let body = before.strip_prefix('/')?;
    let (name, arg_prefix) = body.split_once(char::is_whitespace)?;
    // Only a command that DECLARED a completer is asked (pi's `&& cmd.getArgumentCompletions`).
    if !registry.get(name).is_some_and(|c| c.has_arg_completion) {
        return None;
    }
    let rows = completer.complete(name, &format!("{name} {arg_prefix}"));
    if rows.is_empty() {
        return None; // pi's `null` — the popup does not open
    }
    let mut items = Vec::with_capacity(rows.len());
    let mut completions = Vec::with_capacity(rows.len());
    for (value, label) in rows {
        // The LABEL is the display text (pi renders `item.label`); it already carries the
        // em-dash description for the subcommand rows.
        items.push(SelectItem::new(label, None));
        completions.push(Completion { value: format!("/{value}"), is_dir: false });
    }
    let list = SelectList::new(items, ColumnLayout::SLASH).with_no_match("No matching arguments");
    Some(Autocomplete {
        context: CompletionContext::Argument,
        prefix: before.to_string(), // the WHOLE `/mcp reconnect li` span is replaced
        completions,
        list,
    })
}
```

`Autocomplete::compute` ([:77-92](../../crates/cyrup-tui/src/autocomplete.rs)) takes one more
parameter and tries `slash_context` first, then `argument_context`, then `path_context`.
`InputEditor` ([editor/mod.rs:115](../../crates/cyrup-tui/src/editor/mod.rs) — a plain struct with no
derives, so this is free) gains `arg_completer: Option<Arc<dyn ArgumentCompleter>>` and a
`set_arg_completer` beside `set_registry`
([editor/config.rs:118](../../crates/cyrup-tui/src/editor/config.rs)), threaded at both `compute`
call sites ([editor/completion.rs:79](../../crates/cyrup-tui/src/editor/completion.rs),
[:119](../../crates/cyrup-tui/src/editor/completion.rs)).

Wiring: an adapter over `Arc<cyrup_ext::ExtensionHost>` implementing `ArgumentCompleter` by calling
`native_command_completions`, installed alongside `set_registry` in `rebuild_command_registry`
([execute_misc.rs:26-39](../../crates/cyrup-tui/src/app/execute_misc.rs)) — the one method that
covers boot, session swap, the skill-commands toggle and HA-1's late registration, so the completer
never outlives its session.

Last leg: `has_arg_completion` must stop being hardcoded `false`
([commands.rs:487](../../crates/cyrup-tui/src/commands.rs)). Emit `"hasArgCompletion": true` on the
extension rows of `slash_command_catalog`
([session/commands.rs:212-245](../../crates/cyrup-session-svc/src/session/commands.rs)) from
`ExtensionRegistry::command_autocomplete()`
([registry.rs:1013](../../crates/cyrup-ext/src/registry.rs)) — the producer that has existed with no
consumer — and read it in `dynamic_commands_from_catalog_gated`. `McpExtension::init` then calls
`api.add_autocomplete(MCP_COMMAND)` ([native.rs:413](../../crates/cyrup-ext/src/native.rs)) beside
its `register_command` at [registration.rs:2136](../../crates/cyrup-mcp/src/registration.rs), which
makes it the **first** production caller of a surface that has been write-only since it was added.

### 9 · MCP-362 + MCP-368 + MCP-377 — the overlay seam

**MCP-362.** Additive, defaulted, breaks no existing overlay. On `InteractiveOverlay`
([host/overlay.rs:262-292](../../crates/cyrup-ext/src/host/overlay.rs)):

```rust
/// Whether the overlay has decided to close itself without a keystroke — pi's
/// `setTimeout(() => done(...), INACTIVITY_MS)`. [`Self::tick`] returns `bool` and cannot express
/// this; the host consults this after every tick and tears the overlay down when it is `true`.
fn should_close(&self) -> bool {
    false
}
```

Mirror it on `cyrup_tui::overlay::Overlay` ([overlay.rs:52-69](../../crates/cyrup-tui/src/overlay.rs))
with `ExtensionOverlay` delegating ([overlay.rs:186-192](../../crates/cyrup-tui/src/overlay.rs)), and
in `on_overlay_ticked` ([run_arms.rs:388-401](../../crates/cyrup-tui/src/app/run_arms.rs)):

```rust
let mut changed = false;
for overlay in self.state.overlays.iter_mut() {
    changed |= overlay.tick();
}
let before = self.state.overlays.len();
// Dropping the `ExtensionOverlay` fires its one-shot, releasing the blocked extension task —
// the same teardown path `handle_overlay_key`'s `pop()` takes (app/input.rs:382), because the
// release is in `Drop` (overlay.rs:138-145), not in the pop.
self.state.overlays.retain(|o| !o.should_close());
changed |= self.state.overlays.len() != before;
```

`McpPanelOverlay::should_close` / `McpSetupOverlay::should_close` then return `self.expired`, and the
`TODO(MCP-362)` blocks at [ui.rs:3260-3270](../../crates/cyrup-mcp/src/ui.rs) and
[ui.rs:4595-4600](../../crates/cyrup-mcp/src/ui.rs) reduce to setting `expired`, calling
`model.expire()`, publishing, and returning `true`. The `if self.expired` early-close in each
`handle_key` ([ui.rs:3236-3239](../../crates/cyrup-mcp/src/ui.rs),
[ui.rs:4577-4579](../../crates/cyrup-mcp/src/ui.rs)) stays: a keystroke that races the tick must
still not resurrect the panel. Keep the resolved-cadence residue in the comment: a polled deadline
fires within one `REFRESH_MS`, so the panel lives up to 250 ms longer than upstream's `setTimeout`,
and at `refresh_ms() == 0` it would never auto-cancel at all.

**MCP-368 (HA-3), and the one place this task diverges from its spec.** MCP-368 recommends option
(a): an `OverlayOptions` bag threaded through `open_overlay` and `OverlayRequest`
([13h :1326-1352](../../docs/gap-analysis/13h-mcp-tui.md)). **Do not do that.** `open_overlay` is a
`HostServices` trait method with a default body
([services.rs:254-256](../../crates/cyrup-ext/src/host/services.rs)), a `LiveHostServices` impl
([host_services.rs:1043](../../crates/cyrup-session-svc/src/host_services.rs)) and — critically — a
`fenced!` macro arm in `OwnedServices` ([owner.rs:415-418](../../crates/cyrup-mcp/src/owner.rs))
whose arity the macro fixes. Widening it edits five sites across three crates to carry a value the
component already knows.

Instead put the bag on the trait the component already implements, which is also where upstream puts
it (`ctx.ui.custom(factory, { overlay: true, overlayOptions })` travels **with the factory**,
[commands.ts:592](../../tmp/pi-mcp-adapter/commands.ts)). In
[host/overlay.rs](../../crates/cyrup-ext/src/host/overlay.rs):

```rust
/// pi's `overlayOptions` (`interactive-mode.ts:2719`). Defaults are today's four `cyrup-tui`
/// constants, so `FleetOverlay` and `PermissionSystemSettingsOverlay` are untouched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayOptions {
    /// Fixed column count (pi `width: 82 | 92`), or `None` for the percentage default.
    pub width: Option<u16>,
    pub width_pct: u16,      // 95
    pub min_width: u16,      // 60
    pub max_height_pct: u16, // 85
    pub margin: u16,         // 1
}

impl OverlayOptions {
    /// The row budget the host will paint into, given the FRAME height. Called by
    /// `ExtensionOverlay::box_rect` AND by an overlay that windows its own body (MCP-377), so the
    /// two cannot disagree — which is the whole reason the height half and the width half are one
    /// change.
    #[must_use]
    pub fn max_rows(&self, frame_height: u16) -> u16 { /* margin*2, then max_height_pct, min 1 */ }
}
```

plus `fn options(&self) -> OverlayOptions { OverlayOptions::default() }` on `InteractiveOverlay`.
`ExtensionOverlay::box_rect` ([overlay.rs:112-132](../../crates/cyrup-tui/src/overlay.rs)) — an
**associated function**, so this is a third parameter, not a `&self` read — takes the options
instead of the four `const`s (keep the consts as the `Default` impl's source), and
`ExtensionOverlay::render` ([overlay.rs:146-172](../../crates/cyrup-tui/src/overlay.rs)) reads
`self.inner.options()` once per frame and passes it to both `box_rect` calls.
`McpPanelOverlay::options` returns `width: Some(82)`; `McpSetupOverlay::options` returns
`width: Some(92)`.

**MCP-377 (the height half).** Do **not** change `McpSetupPanelModel::render(&self, width)`
([ui.rs:3954](../../crates/cyrup-mcp/src/ui.rs)) — it has thirteen call sites in `ui.rs`'s own test
module. Add a sibling and delegate:

```rust
/// [`Self::render`] with a row budget (MCP-377). `render(width)` is
/// `render_bounded(width, usize::MAX)`, which is exactly today's behaviour.
pub fn render_bounded(&self, width: usize, max_rows: usize) -> Vec<OverlayLine> { … }
```

Inside, window the action list at **every** width (not only `inner_w < COMPACT_WIDTH`) by reusing
`visible_action_range` ([ui.rs:4010-4020](../../crates/cyrup-mcp/src/ui.rs)) with a row count derived
from `max_rows` rather than the fixed `COMPACT_ACTION_ROWS`, and truncate the `action_preview` block
([ui.rs:4079-4083](../../crates/cyrup-mcp/src/ui.rs)) so the trailing blank, hint row and bottom
border always fit. The "Add a known server" heading rule
([ui.rs:4048-4056](../../crates/cyrup-mcp/src/ui.rs)) already keys off *first visible row* and needs
no change — that is why it was written that way.

`McpSetupOverlay::render` ([ui.rs:4571-4573](../../crates/cyrup-mcp/src/ui.rs)) stops discarding its
`height` argument:

```rust
fn render(&mut self, width: usize, height: usize) -> Vec<OverlayLine> {
    let budget = self.options().max_rows(u16::try_from(height).unwrap_or(u16::MAX));
    self.model.render_bounded(width, budget as usize)
}
```

### 10 · MCP-049 — `cyrup mcp init`

In [subcommands.rs](../../crates/cyrup/src/subcommands.rs): extend the table at
[:34](../../crates/cyrup/src/subcommands.rs) to `[&str; 7]` with `"mcp"`, and add an arm at the top
of `dispatch` ([:450-456](../../crates/cyrup/src/subcommands.rs)) beside the existing special-cased
`config` arm:

```rust
// `mcp` is dispatched specially, like `config` below: its verbs are not package verbs and it
// takes none of `PackageCommand`'s flag grammar.
if argv.first().map(String::as_str) == Some("mcp") {
    return Ok(Some(run_mcp_subcommand(argv.get(1..).unwrap_or_default(), dirs)));
}
```

`run_mcp_subcommand` recognises `init` only; a missing verb, `help`, `--help` or `-h` prints the help
and returns 0; `install` prints the two retirement errors and returns 1; anything else prints
`Unknown command: {command}` plus the help and returns 1
([cli.js:197-218](../../tmp/pi-mcp-adapter/cli.js)). The code becomes the process exit code through
`run_predispatch`'s existing `return Ok(Some(code))`
([predispatch.rs:72](../../crates/cyrup/src/predispatch.rs)).

`run_init` builds the context from the `ConfigDirs` the pre-dispatch already resolved:

```rust
let mcp_dirs = cyrup_mcp::dirs::McpDirs::new(dirs.agent_dir.clone(), dirs.cwd.clone());
let ctx = cyrup_mcp::config::ConfigContext::new(mcp_dirs, None).with_home(dirs.home.clone());
```

then, in **upstream's exact order** ([cli.js:150-195](../../tmp/pi-mcp-adapter/cli.js)):

1. `find_available_import_configs(&mut diagnostics)` — print each diagnostic to stderr;
2. read the existing `imports` set, compute `imports_to_add`;
3. `printDiscovery`: `Config discovery:` + blank, then one `✓`/`-` row per
   `config_discovery_paths()` entry, then blank + `Compatibility imports:` + blank, then either
   `- No host-specific MCP configs detected` **and return from the printer**, or one
   `✓ {kind}: {path}` per found import;
4. `discovery_setting_changed = discover_host_configs && settings.hostConfigDiscovery != On`;
5. nothing to add **and** no setting change ⇒ the two-line `No Pi config changes needed.` message
   and exit 0 — before any writer;
6. `Detected host configs to import into Pi: {kinds}` when there is anything to add;
7. `Opting in to host-specific fallback discovery (standard and Pi-owned configs still take
   precedence).` when the setting changed;
8. `--dry-run` ⇒ `Dry run: would update {path}` and exit 0. **Tested here, before either writer** —
   not by rolling back after;
9. `ensure_compatibility_imports(&to_add)`, then the new `hostConfigDiscovery` writer;
10. `Updated {path}`, the **one unconditional** explanatory line, and the **one conditional** line
    gated on `discovery_setting_changed`.

The new writer, beside `ensure_compatibility_imports`
([config.rs:3750](../../crates/cyrup-mcp/src/config.rs)):

```rust
/// Set `settings.hostConfigDiscovery = "on"` in the adapter-owned global file
/// ([`Self::user_path`]). Returns `false` and writes nothing when it is already `"on"`, so a second
/// `cyrup mcp init` does not touch the mtime — the same idempotence contract
/// [`Self::ensure_compatibility_imports`] has, and for the same reason.
///
/// CYRUP-DELTA: upstream's `writePiConfig` (`cli.js:145-148`) rewrites the WHOLE document from a
/// spread, so it can only be idempotent by luck. This merges into the raw object, preserving key
/// order and every key it does not touch. Neither preserves comments — `RawJson` has no comment
/// variant and `parse_json_config` strips them (there is a test: config.rs:4771).
pub fn enable_host_config_discovery(&self) -> McpResult<bool> {
    let target = self.user_path();
    let mut raw = read_raw_config_object(&target);
    let mut settings = match raw.get("settings") {
        Some(RawJson::Object(existing)) => existing.clone(),
        // A non-object `settings` is replaced, matching upstream's spread of a non-object into an
        // object literal. `read_validated_config` is equally lenient about it.
        _ => RawObject::new(),
    };
    if settings.get("hostConfigDiscovery") == Some(&RawJson::String("on".to_string())) {
        return Ok(false);
    }
    settings.insert("hostConfigDiscovery".to_string(), RawJson::String("on".to_string()));
    raw.insert("settings".to_string(), RawJson::Object(settings));
    // `mcpServers: existingConfig.mcpServers ?? {}` — and it normalises the legacy key, which is
    // `loadPiConfig`'s `delete normalized["mcp-servers"]` (cli.js:94).
    let servers = get_servers_object(&raw);
    set_servers_object(&mut raw, servers);
    write_raw_config_object(&target, &raw)?;
    Ok(true)
}
```

JSONC reading for anything this cannot express goes through `cyrup_permission_system::jsonc`
(`crates/cyrup-permission-system/src/jsonc.rs`), which the `cyrup` bin already depends on
([Cargo.toml:68](../../crates/cyrup/Cargo.toml)) — the same parser `parse_json_config` uses
([config.rs:1784-1786](../../crates/cyrup-mcp/src/config.rs)), so both read that file identically by
construction.

---

## Definition of Done

Each item is checkable by reading the diff or by one `grep`. Nothing here requires a test suite.

### The seam

- [ ] `McpExtension` implements `NativeExtension::execute_command`; `grep -n "async fn execute_command" crates/cyrup-mcp/src/extension.rs` matches.
- [ ] `McpExtension::init_task()` and `McpExtension::config_context()` exist as `pub(crate)`, and both former inline `ConfigContext` builders ([extension.rs:196-204](../../crates/cyrup-mcp/src/extension.rs), [:626-634](../../crates/cyrup-mcp/src/extension.rs)) now call the latter — the idiom appears **once**.
- [ ] `command_prologue` snapshots `has_ui`, `mode`, `cwd`, the `ui` handle and the owner **before** the first `.await`; the `ui` field has all three upstream states (`None` when `!has_ui`, raw services when there is no owner, `OwnedServices` otherwise); the await carries **no** timeout; the two failure notices are byte-exact; and `cmd.alive()` is called after the await and before every side effect in the switch.
- [ ] `/mcp-auth`'s empty-argument + `!has_ui` bail is textually **above** the `command_prologue` call.
- [ ] The split produces `target_server = parts[1]` and `rest = parts[1..].join(" ")` and `reconnect` uses the former while `logout`/`disable`/`enable` use the latter.
- [ ] `status`, `""` and the `_` wildcard share one arm; `setup` and the panel arm `return` after `cmd.reload()` while every other arm falls through to the single `Ok(None)`.
- [ ] `logout`'s empty-name notify is gated on `cmd.has_ui` and returns early; `disable`/`enable`'s three refusals are ungated and fall through.
- [ ] Every arm returns `Ok(None)`; `grep -n "Ok(Some(" crates/cyrup-mcp/src/commands.rs` finds nothing.
- [ ] The five Wave 7 arms are single functions carrying a `TODO(MCP-386)` / `TODO(MCP-387)` / `TODO(MCP-388)` / `TODO(MCP-390)` / `TODO(MCP-394)` comment; no Wave 7 body is duplicated here.

### The listings

- [ ] `show_status`, `show_tools` and `show_prompts` each return an empty `String` when `has_ui` is false, and the call sites skip the notify on empty.
- [ ] `show_status` emits `MCP Server Status:` + a blank line, one row per server in `config.mcp_servers` insertion order, and reads connection state through `state.manager.get_connection(...).status()` — **not** through a `ProxyCtx` or `ProxyEnv`.
- [ ] A disabled server renders `⊘ {name}: disabled (run /mcp enable {name}, then /reload)` with **no** tool suffix, via `continue`.
- [ ] The ladder tests `failed` **before** `cached`, and the `failed` arm sets a suffix of `""`.
- [ ] `crate::env::failure_message` exists, is gated on `failure_age_seconds` being `Some`, and is `show_status`'s only reader of `failure_messages`.
- [ ] `show_tools` filters servers by `config.mcp_servers.get(server).is_none_or(|d| !d.is_disabled())`, returns the bare `No MCP tools available` string when empty, and otherwise ends `Total: {N} tools` — never singularised.
- [ ] `show_prompts` emits an unindented, icon-free, unsanitized `{server}:` header per group, two-space `/{command}` rows, six-space description rows, a blank line after each group, and `Total: {N} prompt{s}` singular only at 1.
- [ ] The `promptDiscoveryFailed` names come from `state.manager.get_all_connections()` filtered to connected **and** `prompt_discovery_failed()`, sorted; the empty-list note is appended to the same sentence with a leading space and the non-empty one is its own trailing line.
- [ ] A `CYRUP-DELTA` comment at the server sort site states the `localeCompare`-vs-`str::cmp` divergence.
- [ ] `state::PromptMetadata` carries the six `PromptCommandSpec` fields and a doc comment saying why the two types match.

### The arms

- [ ] `/mcp disable` calls `ConfigContext::write_project_server_disabled_override(name, disabled)` and re-derives none of its four error strings; the two outcome notices are built from `result.changed` and `result.path`.
- [ ] The three refusals are `/mcp {sub} is unavailable when config is supplied by createMcpAdapter().` (Info), `Usage: /mcp {sub} <server>` (Error) and `Server "{name}" not found in effective config` (Error).
- [ ] `authenticate_server` emits the five guards in the order no-UI → unknown → disabled → not-OAuth → no-URL, notifies `msg_server_disabled` at **`Warning`**, and uses a **second** literal `msg_not_oauth_notify` for the two-line notified text.
- [ ] `set_status("mcp-auth", Some(msg_authenticating(name)))` is set before the flow and cleared afterwards **only when the signal did not abort**.
- [ ] `terminal_hyperlink` sanitizes both halves **before** building the OSC-8 sequence; `grep -rn "u{1b}]8" crates/cyrup-mcp/src/commands.rs` matches exactly one function.
- [ ] `open_mcp_auth_panel` applies four guards in order and takes `callbacks: Arc<dyn McpPanelCallbacks>` as a parameter; it never constructs one.
- [ ] The zero-OAuth warning is `No OAuth-capable MCP servers are configured.` at `NotifyKind::Warning`, and [ui.rs:2670](../../crates/cyrup-mcp/src/ui.rs)'s `No OAuth-capable MCP servers configured.` is **unchanged**.
- [ ] `open_mcp_auth_panel` passes `PanelOptions { auth_only: true, notice_lines: vec![AUTH_PANEL_NOTICE.into()], .. }` and returns `config_changed == false` on every path.
- [ ] `mode_str` exists and is the only producer of the `&str` handed to `panel_unavailable_message` / `auth_panel_unavailable_message`.

### The prompt grammar

- [ ] `tokenize_args` keeps both quote characters in the token, treats `\` as literal inside `'…'`, carries `escaped` across iterations (dropping a trailing lone backslash), and runs an unterminated quote to end of input.
- [ ] `find_unquoted_equals` is filtered by `eq > 0` **strictly**, and a token whose key trims to empty falls through to `positional`.
- [ ] `resolve_prompt_args`' loop 1 advances `positional_index` **only** in the `None` branch of an explicit `match` — no `.or_else(…)` with an unconditional bump.
- [ ] Loop 2 has **no** `is_empty()` guard, and a comment naming MCP-397a says why.
- [ ] `build_usage_message` emits `Missing required argument{s}: {names}.\nUsage: /{command} {usage}` and is `.trim()`ed.
- [ ] `format_prompt_result` emits a lone `Role::User` message bare and prefixes everything else — including a lone assistant message — with `[{role}] `.
- [ ] `extract_message_text` has wildcard arms on **both** `ContentBlock` and `ResourceContents`, each with a comment naming `#[non_exhaustive]` and the upstream behaviour it coincides with.
- [ ] `mime_type` / `data` / `Resource.name` are handled with `is_empty()` tests, not `Option` fallbacks, and the `resource_link` separator is `\u{2014}`.
- [ ] `renderers::transform_mcp_content` ([renderers.rs:616](../../crates/cyrup-mcp/src/renderers.rs)) is unchanged and `grep -rn "transform_mcp_content" crates/cyrup-mcp/src/prompts.rs` finds nothing.

### The host additions

- [ ] `NativeExtension::argument_completions` exists with a defaulted empty body and is **sync**; `ExtensionHost::native_command_completions` exists, is **not** `#[cfg(feature = "wasm-host")]`, and routes through `command_route` + the native map.
- [ ] `mcp_command_descriptor` ([registration.rs:1877](../../crates/cyrup-mcp/src/registration.rs)) declares exactly eight subcommands; `grep -n '"token"' crates/cyrup-mcp/src/registration.rs` finds nothing, and its doc comment no longer says "nine".
- [ ] `MCP_SUBCOMMANDS` carries the eight em-dash labels verbatim from [index.ts:477-484](../../tmp/pi-mcp-adapter/index.ts), and `argument_completions` returns `Vec::new()` for `MCP_AUTH_COMMAND`.
- [ ] `CompletionContext::Argument` exists with an `apply` arm that does **not** re-prefix `/`; `argument_context` refuses a command whose `has_arg_completion` is false and returns `None` on an empty row set.
- [ ] `slash_command_catalog` emits `hasArgCompletion` from `ExtensionRegistry::command_autocomplete()`, and `dynamic_commands_from_catalog_gated` ([commands.rs:487](../../crates/cyrup-tui/src/commands.rs)) reads it instead of hardcoding `false`.
- [ ] `McpExtension::init` calls `api.add_autocomplete(MCP_COMMAND)`.
- [ ] `InteractiveOverlay::should_close` exists with a `false` default; `Overlay` mirrors it; `ExtensionOverlay` delegates; `on_overlay_ticked` `retain`s on it and repaints when the length changed.
- [ ] Both `TODO(MCP-362)` comments ([ui.rs:3260](../../crates/cyrup-mcp/src/ui.rs), [ui.rs:4595](../../crates/cyrup-mcp/src/ui.rs)) are gone, both overlays return `self.expired` from `should_close`, and both `handle_key` early-close guards remain.
- [ ] `OverlayOptions` exists in `cyrup-ext` with today's four constants as `Default` and a `max_rows(frame_height)`; `InteractiveOverlay::options` is defaulted; `ExtensionOverlay::box_rect` takes it as a parameter; `McpPanelOverlay` reports `width: Some(82)` and `McpSetupOverlay` `Some(92)`.
- [ ] `McpSetupPanelModel::render(&self, width)` still exists with the same signature and delegates to `render_bounded(width, usize::MAX)`; the `TODO(MCP-368, MCP-377)` comment ([ui.rs:4028](../../crates/cyrup-mcp/src/ui.rs)) is gone; the action list windows at every width and the preview block is truncated so the hint row and bottom border always fit.

### Out of session

- [ ] `"mcp"` is in `SUBCOMMANDS` ([subcommands.rs:34](../../crates/cyrup/src/subcommands.rs)) and `dispatch` special-cases it beside `config`.
- [ ] `run_init` performs its steps in cli.js's order, and the `--dry-run` test is textually **above** both writers.
- [ ] The discovery table prints one row per `config_discovery_paths()` entry with a `✓`/`-` prefix — no padding to six rows, no re-cased labels — and a comment records both divergences.
- [ ] `ConfigContext::enable_host_config_discovery` exists, returns `false` without writing when the setting is already `"on"`, and calls `set_servers_object` so the legacy key is normalised.
- [ ] The comment at the writer states the real divergence — merge + atomic rename versus upstream's whole-document overwrite — and explicitly notes that **neither** preserves comments, citing [config.rs:4771](../../crates/cyrup-mcp/src/config.rs).
- [ ] `cyrup mcp install` prints the two retirement errors and returns 1; `cyrup mcp wibble` prints `Unknown command: wibble` plus the help and returns 1; a bare `cyrup mcp`, `cyrup mcp help`, `--help` and `-h` all print the help and return 0.
