---
stage: new
status: done
updated: 2026-08-22 16:35
---

# The `/mcp` Command Surface: One Handler, Its Arms, And The Three Host Verbs It Needs

## Description

Every surface a human drives by hand in `cyrup-mcp` is unreachable at **one seam**.

[`McpExtension`](../../crates/cyrup-mcp/src/extension.rs) implements `id`, `is_ambient`, `init`,
`on_event`, `render_call`, `render_result`, `set_host_services` and `set_late_registrar`
([extension.rs:592-789](../../crates/cyrup-mcp/src/extension.rs)) and does **not** override
`NativeExtension::execute_command`. A grep for `execute_command` across
`crates/cyrup-mcp/src` returns three doc-comment mentions and no `impl`
([registration.rs:1926](../../crates/cyrup-mcp/src/registration.rs),
[registration.rs:2196](../../crates/cyrup-mcp/src/registration.rs),
[oauth.rs:3781](../../crates/cyrup-mcp/src/oauth.rs)). Both commands *are* registered — at
[registration.rs:2136 and :2138](../../crates/cyrup-mcp/src/registration.rs), behind
`api.register_fixed_commands()` — so `/mcp` and `/mcp-auth` appear in the `/` menu, route correctly
through `ExtensionHost::execute_native_command`
([facade.rs:533-573](../../crates/cyrup-ext/src/facade.rs)), and land on the trait's default body:

```rust
Err(ExtError::Component(format!("native extension has no handler for command `{name}`")))
```

[native.rs:580-587](../../crates/cyrup-ext/src/native.rs). That `Err` surfaces as an
`ExtError`-prefixed error notification (`command:mcp: …`). So today **both commands answer with the
same error string**, and every listing, every panel entry point, every write-back and every prompt
invocation is dead code behind it.

Thirteen of the eighteen units below are that one handler and its arms. An arm has nowhere to attach
until the owner-fenced prologue exists, so they are five files' worth of one job, not thirteen tasks.
Three more (MCP-041, MCP-362, MCP-368) are the only host additions these surfaces need, and nothing
else in the port touches `cyrup-tui`; MCP-377 is the `cyrup-mcp` half of MCP-368's painting problem
and cannot be split from it. MCP-049 is the same surface out of session.

### Landing order

**This task must land AFTER [MCP_RUNTIME_INIT_SPINE.md](MCP_RUNTIME_INIT_SPINE.md).** The prologue
awaits `initPromise` and every arm reads live `McpState`; the spine is what builds it and what
supplies the one production `ProxyEnv`. Specifically, MCP-383 reads `failure_age_seconds` /
`failure_messages`, which exist only as `McpState` **fields**
([state.rs:115-117](../../crates/cyrup-mcp/src/state.rs)) and as `ProxyEnv` **declarations**
([proxy.rs:1490-1492](../../crates/cyrup-mcp/src/proxy.rs)) with no production implementor.

**MCP-381 and MCP-398 belong to Wave 7 of
[MCP_HIGH_SEVERITY_BACKLOG.md](MCP_HIGH_SEVERITY_BACKLOG.md) and are not this task's to write** —
MCP-381 is the switch this task's arms hang off, and MCP-398 is the caller of the grammar this task
builds. MCP-040, MCP-042 and MCP-334 *are* that
switch's `/mcp` and `/mcp-auth` halves and must be one change with it — both commands are answered by
the same default error today, and splitting the prologue from its two registrations produces two
handlers that duplicate the fence. Ship MCP-381 + MCP-040 + MCP-042 + MCP-334 as one commit; the
listing/entry-point/grammar units then land arm by arm against it.

`register_late_command` and `LateRegistrar` **already exist**
([facade.rs:724-732](../../crates/cyrup-ext/src/facade.rs),
[native.rs:768-787](../../crates/cyrup-ext/src/native.rs)) — HA-1's command leg landed with
MCP-037/037a. Nothing in this group needs to build it.

---

## Three traps, carried in from the audit

**1. `execute_status` is not `showStatus`.**
[`proxy::execute_status`](../../crates/cyrup-mcp/src/proxy.rs) at
[proxy.rs:1921-2010](../../crates/cyrup-mcp/src/proxy.rs) is MCP-154 — the *gateway tool's* `status`
mode. Reuse its **six-rung ladder shape** (`disabled → connected → needs-auth → failed → cached →
not connected`, first match wins, with `metadata`/`connection` forced absent for a disabled server)
and port the text fresh. Its text is a different artefact: `MCP: 1/2 servers, 7 tools`, `⊘ name
(disabled)`, `✓ name (7 tools)`, `✗ name (failed 12s ago)` with **no reason**, and a trailing
`mcp({ server: "name" })` hint. MCP-383's is `MCP Server Status:`, `⊘ {name}: disabled (run /mcp
enable {name}, then /reload)`, `✗ {name}: failed 12s ago — {reason} (…)`. Calling `execute_status`
from `/mcp status` would ship the model-facing text to the human.

**2. `transform_mcp_content` is not `formatPromptResult`.**
[renderers.rs:610-670](../../crates/cyrup-mcp/src/renderers.rs) is `tool-registrar.ts`'s
**tool-result** shaping (MCP-220): `[Resource: {uri}]`, `[Resource Link: {name}]`, `[Audio content:
{mime}]`, mime defaults of `image/png` / `audio/*`, and an unknown arm that **re-serializes the
original JSON**. MCP-399's §5.6 is `[resource {uri}]`, `[resource_link {uri} — {name}]`, `[audio
{mime|unknown}]`, `[image {mime|unknown} (embedded)]`, and an unknown kind contributing the **empty
string**. Different casing, different bracket text, opposite unknown-type behaviour. Two functions.

**3. The spec is wrong about `/mcp`'s completion list, and so is the tree.**
[`mcp_command_descriptor`](../../crates/cyrup-mcp/src/registration.rs) at
[registration.rs:1871-1888](../../crates/cyrup-mcp/src/registration.rs) declares **nine**
completions:

```rust
["reconnect", "tools", "prompts", "setup", "logout", "token", "disable", "enable", "status"]
```

Upstream declares **eight** and there is no `token` among them
([13a §22 :829-831](../../docs/gap-analysis/13a-mcp-activation.md),
[13h §4.1 :1580-1590](../../docs/gap-analysis/13h-mcp-tui.md)). 13h's eight-way switch table
([:1613-1628](../../docs/gap-analysis/13h-mcp-tui.md)) gives `token` no arm, and a grep for
`"token"` as a subcommand across `crates/cyrup-mcp/src` finds only that one line. `token` is an
invention. Left in place, `/mcp token<TAB>` completes to a subcommand that falls through to the
`default` arm and **opens the browser panel** — a completion that lies. **Delete `"token"` from that
array** as part of MCP-041. Do not add a `token` arm to the switch.

---

## Per-unit breakdown

### The handler and its two registrations

#### MCP-040 — the `/mcp` command handler · medium · `host-verb`

**Unmet.** No `execute_command` override exists ([extension.rs:592-789](../../crates/cyrup-mcp/src/extension.rs));
`/mcp` reaches [native.rs:586](../../crates/cyrup-ext/src/native.rs). Absent: the owner-fenced
`commandCtx`, the **un-timed** `await initPromise` preamble with `MCP initialization failed:
{message}` / `MCP not initialized`, the `split(/\s+/)` split with `subcommand = parts[0] ?? ""`,
`targetServer = parts[1]`, `rest = parts[1..].join(" ")`, and the eight-way switch
([13a §22 :845-857](../../docs/gap-analysis/13a-mcp-activation.md)).

The half that already exists and must be reused, not re-derived: `OwnedServices`
([owner.rs:314-345](../../crates/cyrup-mcp/src/owner.rs)) is `createOwnedUi`, and its `fenced!` list
([owner.rs:374-465](../../crates/cyrup-mcp/src/owner.rs)) already covers `notify`, `set_status`,
`confirm`, `input`, `open_overlay`, `control` and `is_run_cancelled` — every verb this handler
touches. `on_input` ([extension.rs:517-540](../../crates/cyrup-mcp/src/extension.rs)) is the shipped
template for capture-owner-then-await-`init_task`; the command prologue is the same shape **without**
`INIT_WAIT_TIMEOUT_MS`.

#### MCP-042 — the `/mcp-auth` command handler · medium · `host-verb`

**Unmet**, same seam. The ordering detail that must survive: `if (!serverName && !ctx.hasUI)
return;` fires **silently, before the init-await**
([13h §4.3 :1629-1638](../../docs/gap-analysis/13h-mcp-tui.md)) — a headless `/mcp-auth` with no
argument must produce no output, no error and **no initialization wait**. Writing the prologue first
and the bail second is the natural Rust order and is wrong.

#### MCP-334 — the `/mcp-auth` command surface and its eleven messages · medium · `host-verb`

**Half met, and the met half is the expensive one**
([13g :1294-1305](../../docs/gap-analysis/13g-mcp-oauth.md)). All eleven message strings are already ported
as functions in [oauth.rs:3800-3880](../../crates/cyrup-mcp/src/oauth.rs): `MSG_REQUIRES_INTERACTIVE`,
`msg_server_not_found`, `msg_server_disabled`, `msg_not_oauth`, `msg_no_url`, `msg_authenticating`,
`msg_auth_success`, `msg_auth_failed`, `msg_auth_threw`, `msg_auth_required_proxy`,
`msg_auth_required_direct_tools`. A grep for consumers of any of them across `crates/cyrup-mcp/src`
returns **exactly one hit, and it is a doc comment**
([oauth.rs:3784](../../crates/cyrup-mcp/src/oauth.rs)). What is unmet is the dispatch that emits them
in guard order (no UI → unknown server → disabled → not OAuth → no URL) and the `mcp-auth` status key
set to `msg_authenticating(name)` and cleared in a `finally` **unless the signal aborted**.
`HostServices::set_status(&self, key, Option<&str>)` maps 1:1 including its `None`-clears semantics.

### The three headless listings

#### MCP-383 — port `showStatus` · medium · `hand-written`

**Unmet.** `grep -rn "MCP Server Status" crates/` returns nothing. Needs §4.4 in full
([13h :1639-1664](../../docs/gap-analysis/13h-mcp-tui.md)): the `["MCP Server Status:", ""]` header,
rows in `config.mcp_servers` insertion order, the disabled row with `continue` and **no tool
suffix**, the five-rung first-match ladder, `toolSuffix = failed ? "" : " ({n} tools{, cached})"`,
`tools` **never singularised**, and the two-line empty-config message. The failure arm is tested
**before** the metadata arm, so a failed server never reports `cached` even with metadata present.
`ctx.hasUI` is `HostCtx::has_ui` ([native.rs:88-92](../../crates/cyrup-ext/src/native.rs)) — the
headless branch selects on that, **not** on `open_overlay` returning `false`.

#### MCP-384 — port `showTools` · low · `hand-written`

**Unmet.** `grep -rn "MCP Tools:" crates/` returns nothing. The names come from
`McpState::tool_metadata` ([state.rs:88-90](../../crates/cyrup-mcp/src/state.rs)), whose
`ServerToolMetadata::tool_names` ([state.rs:365-371](../../crates/cyrup-mcp/src/state.rs)) is already
documented as "the resolved, model-visible tool names … in server order" — i.e. the **prefixed,
registered** names §4.5 requires. `Total: 1 tools` is correct output; do not singularise.

#### MCP-385 / MCP-385a — port `showPrompts` and its per-group header · medium / low

**Unmet.** `grep -rn "MCP Prompts:" crates/` returns nothing. §4.6
([13h :1672-1695](../../docs/gap-analysis/13h-mcp-tui.md)) needs the group header `{serverName}:`
(unindented, no icon, plain colon, **unsanitized** — the name is the user's own config), servers
ordered by `localeCompare`, prompts sorted **in place** by `command_name`, `<required>` / `[optional]`
usage rendering, the two-space `/{command_name}` row, the **six**-space description row, the
per-group blank line, `Total: {N} prompt{s}` (singular only at 1), and the two distinct
`promptDiscoveryFailed` notes.

MCP-385a is filed separately because a golden-text test written from a spec that omits the header row
cannot catch its absence. Write the header first.

**The declared divergence:** `String.localeCompare` with no locale is ICU root collation; Rust's
`str::cmp` is byte order. They agree on ASCII-lowercase names and disagree on mixed case (`Foo` vs
`bar`). Use `str::cmp` and **say so in the ported comment** — do not leave it unstated and do not
pull in a collation crate for one sort.

**Blocker to name, not to absorb:** `McpState::prompt_metadata`
([state.rs:94](../../crates/cyrup-mcp/src/state.rs)) holds `Vec<PromptMetadata>`, and
`PromptMetadata` is still a one-field forward declaration carrying only `name`
([state.rs:377-382](../../crates/cyrup-mcp/src/state.rs)). MCP-385 needs `command_name`,
`description` and `arguments`. Do not invent a second type: replace the stub's body with the same
fields `PromptCommandSpec` already carries
([registration.rs:1789-1797](../../crates/cyrup-mcp/src/registration.rs)) —
`server_name`, `original_name`, `command_name`, `title`, `description`,
`arguments: Vec<CachedPromptArgument>` — so the cache path
(`resolve_cached_prompts`, [registration.rs:1803](../../crates/cyrup-mcp/src/registration.rs)) and
the live path produce one shape. MCP-039/MCP-395a own **populating** it; this group owns the fields
it reads.

### The two remaining switch arms

#### MCP-389 — port `/mcp disable` and `/mcp enable` · medium · `hand-written`

**Unmet at the arm; met at the writer.** `grep -rn "not found in effective config\|run /reload to
apply" crates/` returns nothing in `cyrup-mcp`. But
`ConfigContext::write_project_server_disabled_override`
([config.rs:3484-3572](../../crates/cyrup-mcp/src/config.rs)) is fully built, returns
`ServerDisabledOverrideResult { path, changed }`
([config.rs:3332-3339](../../crates/cyrup-mcp/src/config.rs)), and already carries the four exact
error messages and the never-copy-a-definition property. The arm is four notifies and a call.

This is the **only** subcommand that tells the user to run `/reload` themselves; `setup` and the
panel path call `commandReload` for them.

#### MCP-391 — port `openMcpAuthPanel` · medium · `host-verb`

**Entry point unmet; panel met.** `PanelOptions::auth_only` exists
([ui.rs:1443-1457](../../crates/cyrup-mcp/src/ui.rs)) and `McpPanelModel::new` already threads it
through construction, the visible-item rebuild and the empty message
([ui.rs:1578-1600](../../crates/cyrup-mcp/src/ui.rs), [ui.rs:2663-2672](../../crates/cyrup-mcp/src/ui.rs))
— **one type, not two**, as the unit requires. `auth_panel_unavailable_message`
([ui.rs:4780-4784](../../crates/cyrup-mcp/src/ui.rs)) is the v2.26.1 `!hasUI` refusal and is already
ported.

What is absent is the entry point itself: the `programmaticConfig` refusal `Use /mcp-auth <server> to
authenticate a server from the in-memory SDK config.` (`grep -rn "unavailable when config is
supplied" crates/` returns nothing) and the zero-OAuth-capable-servers **warning**.

**Do not reuse the string that already exists.** [ui.rs:2670](../../crates/cyrup-mcp/src/ui.rs) holds
`"No OAuth-capable MCP servers configured."` — that is the panel's **empty-body row**, rendered
inside an already-open panel. MCP-391's guard text is `"No OAuth-capable MCP servers are
configured."` — with `are` — emitted as a `NotifyKind::Warning` **instead of** opening the panel.
Two strings, two surfaces; collapsing them loses the guard.

Always returns `configChanged: false`, even after a successful authentication.

### The prompt-command grammar and renderer (MCP-398's feed)

`grep -rn "parse_prompt_args\|resolve_prompt_args\|tokenize_args\|strip_quotes\|
find_unquoted_equals\|format_prompt_result\|extract_message_text\|build_usage_message"
crates/cyrup-mcp/src` returns **nothing**. All four units are unwritten.

#### MCP-396 — `parsePromptArgs`' bash-style tokenizer · medium

Three details a reasonable rewrite gets wrong
([13h §5.3 :1896-1916](../../docs/gap-analysis/13h-mcp-tui.md)): (i) both the opening **and** closing
quote go into the token, which is the only reason `stripQuotes` exists; (ii) `eq > 0` is **strict**,
so a token beginning with `=` is positional, not a named argument with an empty key; (iii) `escaped`
is carried across iterations, so a trailing lone backslash is **dropped**.

**Not substitutable by `cyrup-tools`' shell-word handling** — that is a POSIX grammar, which strips
quotes during splitting. This one retains them and strips later.

#### MCP-397 — `resolvePromptArgs` and the usage message · medium

`positionalIndex` advances **only when the named lookup missed** — a JS `??` short-circuit. Written
as `named.get(name).or_else(|| positional.get(i++))` in Rust it advances unconditionally and shifts
every subsequent positional by one: silent wrong output, not an error. Write it as an explicit
if/else. The undeclared-named passthrough is deliberate and cited upstream to the MCP spec; **do not**
filter it to the declared set.

#### MCP-397a — an explicit empty named value survives for a declared optional argument · low

Loop 1 refuses to bind `""`; loop 2 re-adds **any** named key not already in `args`; a key loop 1
rejected is by definition not in `args`. So `topic=` puts `args["topic"] = ""` on the wire for a
declared **optional** `topic`, while a declared **required** `topic` still fails the `missing` filter
and produces the usage error. Write the two loops in upstream's order with **no `is_empty()` guard on
loop 2**. The natural Rust rewrite — filter empties in loop 1, extend only unknown non-empty keys —
silently drops the argument and is exactly the shape a reviewer will suggest. This unit exists to
stop that review comment being accepted.

#### MCP-399 — `formatPromptResult` and `extractMessageText` · medium

See Trap 2. Two rmcp-specific corrections to the spec's `??` operators, both verified against
`rmcp-3.1.4`:

* `ImageContent.mime_type` and `AudioContent.mime_type` are **non-optional `String`**
  (`model/content.rs:62-73`, `:101-112`), so `mimeType ?? "unknown"` can only fire on the **empty
  string** — port it as `if s.is_empty() { "unknown" } else { s }`, not as an `Option` fallback.
* `ImageContent.data` is likewise a non-optional `String`, so `data ? " (embedded)" : ""` is
  `!data.is_empty()`.
* `ContentBlock` is `#[non_exhaustive]` (`model/content.rs:255-265`), so the `_ => String::new()`
  arm is required by the compiler **and** is upstream's "anything else" behaviour. That coincidence
  is worth a comment so a later reader does not "improve" it into a stringify.

### The three host additions

#### MCP-041 — HA-2: dynamic argument completions · medium · `host-addition`

**All three legs unmet.** `grep -rn "argument_completions\|command_completions"
crates/cyrup-tui/src crates/cyrup-session-svc/src crates/cyrup-mcp/src` returns **nothing**.

(a) There is no `NativeExtension::argument_completions` — the trait's method list runs `init`,
`on_event`, `execute_command`, `execute_shortcut`, `render_call`, `render_result`,
`transform_markdown`, … ([native.rs:520-640](../../crates/cyrup-ext/src/native.rs)) with no such
member. (b) `ExtensionHost::command_completions`
([facade.rs:1858-1868](../../crates/cyrup-ext/src/facade.rs)) is `#[cfg(feature = "wasm-host")]` and
resolves through `live_for_command` ([facade.rs:1902-1920](../../crates/cyrup-ext/src/facade.rs)), a
lookup in the **live-WASM** map — a native built-in is absent from it, so the call errors ``command
`mcp` has no live owner``. (c) `slash_context`
([autocomplete.rs:140-142](../../crates/cyrup-tui/src/autocomplete.rs)) returns `None` the moment the
buffer contains any whitespace:

```rust
if !before.starts_with('/') || before.contains(char::is_whitespace) {
    return None;
}
```

so there is no argument-completion context to hook. `SlashCommand::has_arg_completion` exists
([commands.rs:60](../../crates/cyrup-tui/src/commands.rs)) and
`dynamic_commands_from_catalog_gated` hardcodes it `false` for **every** dynamic row
([commands.rs:487](../../crates/cyrup-tui/src/commands.rs)) with a comment saying the catalog carries
no such key. The declaration half exists and does nothing: `InitApi::add_autocomplete`
([native.rs:413-415](../../crates/cyrup-ext/src/native.rs)),
`ExtensionRegistry::{add_command_autocomplete, command_autocomplete}`
([registry.rs:1003](../../crates/cyrup-ext/src/registry.rs), [:1013](../../crates/cyrup-ext/src/registry.rs)).

Also in scope: deleting `"token"` from `mcp_command_descriptor`
([registration.rs:1881](../../crates/cyrup-mcp/src/registration.rs)) — see Trap 3.

#### MCP-362 — the 60 s inactivity auto-cancel · medium · `host-verb`

**Half met.** Both overlays hold an `Instant` deadline re-armed on every keystroke and compared in
`tick`, with `INACTIVITY_MS = 60_000` and `REFRESH_MS = 250`
([ui.rs:87-94](../../crates/cyrup-mcp/src/ui.rs)). What is unmet is the **close**: `tick` returns
`bool`, so the panel cannot ask the host to tear it down. The code says so —
[ui.rs:3263-3270](../../crates/cyrup-mcp/src/ui.rs) is an explicit `TODO(MCP-362)` and only sets
`expired`, publishes the cancelled result, and closes on the **next keystroke**
([ui.rs:3235-3239](../../crates/cyrup-mcp/src/ui.rs)); the setup panel repeats it at
[ui.rs:4597-4599](../../crates/cyrup-mcp/src/ui.rs). An untouched panel stays painted forever, which
is the opposite of what the timer is for.

#### MCP-368 + MCP-377 — overlay geometry (HA-3) and the height clip · low + low

**MCP-368 (a) unmet:** `grep -rn "OverlayOptions" crates/` returns **nothing**.
`HostServices::open_overlay` takes no options bag
([services.rs:254-256](../../crates/cyrup-ext/src/host/services.rs)) and `ExtensionOverlay`
hardcodes `OVERLAY_WIDTH_PCT = 95`, `OVERLAY_MIN_WIDTH = 60`, `OVERLAY_MAX_HEIGHT_PCT = 85`,
`OVERLAY_MARGIN = 1` ([overlay.rs:78-86](../../crates/cyrup-tui/src/overlay.rs)), resolved in
`box_rect` ([overlay.rs:110-132](../../crates/cyrup-tui/src/overlay.rs)). So the 82-column browser
panel and the 92-column setup panel — whose `MAX_VISIBLE = 12`, `prefixLen + 8` description budget
and `innerW - 2` hint wrap are all designed around 82 — paint at 95% of the terminal.

**MCP-377's compact branch is already met** and correctly: `visible_action_range`
([ui.rs:4005-4021](../../crates/cyrup-mcp/src/ui.rs)) with `COMPACT_ACTION_ROWS = 7` / `half = 3`, the
`… N more above` / `… N more below` rows, the "Add a known server" heading emitted when the preset is
the first **visible** row ([ui.rs:4048-4055](../../crates/cyrup-mcp/src/ui.rs)), and the
`Enter select · Esc back` hint switch ([ui.rs:4085-4092](../../crates/cyrup-mcp/src/ui.rs)).

**What is unmet is the height half**, marked `TODO(MCP-368, MCP-377)` at
[ui.rs:4028-4033](../../crates/cyrup-mcp/src/ui.rs): above `inner_w >= 60` the action list is not
windowed at all, and `action_preview`'s output is appended unbounded
([ui.rs:4076-4082](../../crates/cyrup-mcp/src/ui.rs)). Both overlays then discard the frame height
they were handed — `fn render(&mut self, width: usize, _height: usize)` at
[ui.rs:3231](../../crates/cyrup-mcp/src/ui.rs) and [ui.rs:4571](../../crates/cyrup-mcp/src/ui.rs) —
and `ExtensionOverlay::render` clips with `.take(rect.height)`
([overlay.rs:161-167](../../crates/cyrup-tui/src/overlay.rs)), whose own contract calls that
"lossless-by-design". So on a short terminal the setup panel's hint row and bottom border simply
vanish.

These two are one change: the panel can only bound itself against the **same** geometry the host will
paint at, so the height budget must come out of the options type MCP-368 introduces.

### Out of session

#### MCP-049 — `cyrup mcp init` · medium · `hand-written`

**CLI arm unmet; most of the library is built.** `SUBCOMMANDS` is a six-element array with no `"mcp"`
([subcommands.rs:35](../../crates/cyrup/src/subcommands.rs)), `first_subcommand` matches against it
([subcommands.rs:38-44](../../crates/cyrup/src/subcommands.rs)), and `main.rs` pre-dispatches through
both ([main.rs:183-190](../../crates/cyrup/src/main.rs)).

Already built and to be reused, not rewritten:

* `ConfigContext::find_available_import_configs`
  ([config.rs:4104-4121](../../crates/cyrup-mcp/src/config.rs)) — `findAvailableImports` over the
  seven `ImportKind::ALL` families ([config.rs:178-186](../../crates/cyrup-mcp/src/config.rs)),
  first existing candidate per family, with the `Failed to discover imported MCP config from {kind}:`
  warning.
* `ConfigContext::config_discovery_paths`
  ([config.rs:4085-4100](../../crates/cyrup-mcp/src/config.rs)) — `{label, exists, path}` per rung,
  nothing parsed, which is step 5's table.
* `ConfigContext::ensure_compatibility_imports`
  ([config.rs:3749-3784](../../crates/cyrup-mcp/src/config.rs)) — idempotent, returns
  `added: Vec<ImportKind>`, and **does not write** when nothing is added.
* `set_servers_object` ([config.rs:3190-3193](../../crates/cyrup-mcp/src/config.rs)) already
  `shift_remove`s the legacy `mcp-servers` key, which is step 3's normalisation.
* `read_raw_config_object` / `write_raw_config_object`
  ([config.rs:3123-3130](../../crates/cyrup-mcp/src/config.rs),
  [:3148](../../crates/cyrup-mcp/src/config.rs)) round-trip the JSONC **preserving comments**.

**The divergence is already decided by the tree and must be stated in the arm's comment, not
re-litigated:** upstream's `writePiConfig` is a plain `JSON.stringify` overwrite that destroys every
comment. `ensure_compatibility_imports` merges. cyrup preserves comments; that is better behaviour
and a visible divergence either way.

**A second divergence to record:** `cli.js`'s `printDiscovery` prints a **fixed six-row** table.
`ConfigContext::sources` ([config.rs:2840-2926](../../crates/cyrup-mcp/src/config.rs)) dedupes rungs
whose read path collides (`generic != user_path`, the two `.agents` guards, `project_path !=
user_path`, `project_override != …`), so it yields **four to six** rows. Print what it returns. Do
not pad to six with synthetic rows — that would print the same path twice under two labels.

**Genuinely missing library code:** a writer for `settings.hostConfigDiscovery = "on"`. `grep -n
"fn enable_host" config.rs` returns nothing. One small sibling of `ensure_compatibility_imports`.

**The `install` verb has no cyrup analog and is not ported.**

---

## Implementation

### 1 · `crates/cyrup-mcp/src/commands.rs` — the prologue and the switch

New module; add `pub mod commands;` to [lib.rs:132-151](../../crates/cyrup-mcp/src/lib.rs) in
alphabetical position.

```rust
/// The synthetic `commandCtx` (`index.ts` §22), snapshotted **before the first await**.
pub struct CommandCtx {
    /// `commandHasUI` — `HostCtx::has_ui`, captured, never re-read.
    pub has_ui: bool,
    pub mode: cyrup_ext::ExtMode,
    pub cwd: std::path::PathBuf,
    /// `ui: hasUI ? createOwnedUi(ctx.ui, owner) : undefined` — the fenced handle.
    /// `None` in a headless build, which is upstream's `undefined`.
    pub ui: Option<Arc<OwnedServices>>,
    /// `commandOwner`. Re-checked before EVERY side effect, not once at entry.
    pub owner: Option<Arc<McpRuntimeOwner>>,
}

impl CommandCtx {
    /// `commandOwner?.throwIfInactive()`. `Err(())` ⇒ the arm returns without doing its work.
    fn alive(&self) -> bool {
        self.owner.as_ref().is_none_or(|o| o.is_active())
    }
    fn notify(&self, message: &str, kind: cyrup_ext::NotifyKind) {
        if let Some(ui) = &self.ui {
            cyrup_ext::HostServices::notify(ui.as_ref(), message, kind);
        }
    }
}
```

The prologue, on `McpExtension`, shared by both commands:

```rust
/// `index.ts` §22's fenced prologue. Un-timed — unlike the tool bodies, which use
/// `INIT_WAIT_TIMEOUT_MS`. Returns `None` once it has already notified the user.
async fn command_prologue(&self, ctx: &HostCtx) -> Option<(Arc<McpState>, CommandCtx)> {
    // 1-2 — snapshot everything BEFORE the first await.
    let owner = self.owner();
    let services = self.host_services();
    let cmd = CommandCtx {
        has_ui: ctx.has_ui,
        mode: ctx.mode,
        cwd: ctx.cwd.clone(),
        ui: match (services, owner.clone()) {
            (Some(s), Some(o)) => Some(Arc::new(OwnedServices::new(s, o))),
            _ => None,
        },
        owner,
    };
    // 3 — `if (!state && initPromise) await initPromise` (NO timeout).
    let mut state = self.state();
    if state.is_none()
        && let Some(task) = self.init_task.lock().ok().and_then(|slot| slot.clone())
    {
        match (*task).clone().await {
            Ok(built) => {
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
```

The `NativeExtension` arm, appended to the impl at
[extension.rs:592-789](../../crates/cyrup-mcp/src/extension.rs):

```rust
async fn execute_command(
    &self,
    name: &str,
    args: &str,
    ctx: &HostCtx,
) -> Result<Option<String>, ExtError> {
    // A native applies the deadlock guard itself: the WASM path applies it, this one does not
    // (`13h` U-7). `ControlOp::Reload` and `SendUserMessage` are both command-tier.
    ctx.require_command_tier()?;
    match name {
        crate::registration::MCP_COMMAND => self.run_mcp_command(args, ctx).await,
        crate::registration::MCP_AUTH_COMMAND => self.run_mcp_auth_command(args, ctx).await,
        // Every other registered name is a prompt command (MCP-398).
        other => self.run_prompt_command(other, args, ctx).await,
    }
}
```

**Every arm returns `Ok(None)` and notifies at its own level.** The `String` channel is Info-only and
returning both prints the message twice — the contract is spelled out at
[native.rs:560-580](../../crates/cyrup-ext/src/native.rs).

The argument split, exactly:

```rust
// `parts = args?.trim()?.split(/\s+/) ?? []` — `"".split(/\s+/)` yields `[""]`, so the
// no-argument case is `subcommand == ""`, not an empty vec.
let trimmed = args.trim();
let parts: Vec<&str> =
    if trimmed.is_empty() { vec![""] } else { trimmed.split_whitespace().collect() };
let subcommand = parts.first().copied().unwrap_or("");
let target_server = parts.get(1).copied();          // `reconnect` uses THIS
let rest = parts.get(1..).map(|r| r.join(" ")).unwrap_or_default(); // `logout`/`disable`/`enable` use THIS
```

`/mcp logout my server` targets `"my server"`; `/mcp reconnect a b` targets `"a"`. The two are not
interchangeable.

The switch follows [13h §4.2](../../docs/gap-analysis/13h-mcp-tui.md) exactly, including that
`status`, `""` and **anything unrecognised** share one arm — `/mcp wibble` opens the panel — and that
`setup` and the panel path `return` after `commandReload()` while every other arm `break`s.

### 2 · MCP-383/384/385/385a — one module of pure renderers

Put the three listing bodies in `commands.rs` as **pure functions returning `String`**, so the arm is
`cmd.notify(&show_status(&state), NotifyKind::Info)` and the text is unit-testable without a host.

```rust
/// `commands.ts` `showStatus` (13h §4.4). One multi-line Info notify.
///
/// The ladder is `execute_status`'s six rungs (proxy.rs:1921) with `showStatus`'s OWN text:
/// upstream keeps two renderers over one state machine and so does this.
pub fn show_status(state: &McpState) -> String {
    let mut lines = vec!["MCP Server Status:".to_string(), String::new()];
    for (name, definition) in &state.config.mcp_servers {
        if definition.is_disabled() {
            lines.push(format!(
                "\u{2298} {name}: disabled (run /mcp enable {name}, then /reload)"
            ));
            continue; // no tool suffix, ever
        }
        let connection = state.manager.connection_status(name);
        let failed_ago = state.failure_age_seconds(name);
        let tool_count = state.tool_metadata_len(name);
        let has_metadata = state.has_tool_metadata(name);

        // FIRST MATCH WINS, and `failed` is tested BEFORE `cached` — a failed server must never
        // report `cached` even when its metadata is present.
        let (icon, status, failed) = if connection == Some(ConnectionStatus::Connected) {
            ("\u{2713}", "connected".to_string(), false)
        } else if connection == Some(ConnectionStatus::NeedsAuth) {
            ("\u{26a0}", "needs auth".to_string(), false)
        } else if let Some(secs) = failed_ago {
            let reason = state.failure_message(name).unwrap_or_default();
            let reason = crate::ui::sanitize_terminal_text(&reason);
            let text = if reason.is_empty() {
                format!("failed {secs}s ago")
            } else {
                format!("failed {secs}s ago \u{2014} {reason}")
            };
            ("\u{2717}", text, true)
        } else if has_metadata {
            ("\u{25cb}", "cached".to_string(), false)
        } else {
            ("\u{25cb}", "not connected".to_string(), false)
        };

        // `tools` is NEVER singularised.
        let suffix = if failed {
            String::new()
        } else {
            let cached = if status == "cached" { ", cached" } else { "" };
            format!(" ({tool_count} tools{cached})")
        };
        lines.push(format!("{icon} {name}: {status}{suffix}"));
    }
    if state.config.mcp_servers.is_empty() {
        lines.push("No MCP servers configured".to_string());
        lines.push(
            "Run /mcp setup to adopt imports or scaffold a starter .mcp.json".to_string(),
        );
    }
    lines.join("\n")
}
```

`connection_status` / `failure_age_seconds` / `failure_message` / `tool_metadata_len` /
`has_tool_metadata` are the `McpState` accessors the spine group introduces behind `ProxyEnv`
([proxy.rs:1436-1495](../../crates/cyrup-mcp/src/proxy.rs)). Read them through `McpState`, not
through a `ProxyCtx` — the listing is not a proxy mode and building a `ProxyCtx` for it would drag
the whole 30-method trait into the command path.

`show_tools` and `show_prompts` follow the same shape. For `show_prompts`, group with an
`IndexMap<String, Vec<PromptMetadata>>` clone (grouping is rebuilt per call, so the in-place sort is
harmless — **do not** "optimise" it into a shared cache), then:

```rust
let mut servers: Vec<&String> = grouped.keys().collect();
// CYRUP-DELTA: `String.localeCompare` with no locale is ICU root collation; this is byte order.
// They agree for ASCII-lowercase names and disagree on mixed case (`Foo` vs `bar`).
servers.sort_by(|a, b| a.as_str().cmp(b.as_str()));
for server in servers {
    lines.push(format!("{server}:"));           // MCP-385a: unindented, no icon, UNSANITIZED
    let mut prompts = grouped[server].clone();
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
rules are different on purpose.

### 3 · `crates/cyrup-mcp/src/prompts.rs` — MCP-396/397/397a/399

New module (`pub mod prompts;` in `lib.rs`). Pure functions over
`&[CachedPromptArgument]` ([registration.rs:729-737](../../crates/cyrup-mcp/src/registration.rs)) and
`rmcp::model::GetPromptResult` — **no `McpState`, no host** — so MCP-398's handler is a caller and
these are testable alone.

```rust
/// `prompts.ts` `tokenizeArgs` (13h §5.3). The quote characters STAY in the token;
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
    // `escaped` is consumed across iterations and never flushed, so a TRAILING LONE BACKSLASH
    // is dropped. Upstream behaviour; do not "fix" it.
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// The index of the first `=` outside quotes, or `None`.
fn find_unquoted_equals(token: &str) -> Option<usize> { /* … */ }

/// Strip exactly one layer when `len >= 2` and the first and last chars are the SAME quote.
fn strip_quotes(value: &str) -> &str { /* … */ }

pub struct ParsedPromptArgs {
    pub named: IndexMap<String, String>,
    pub positional: Vec<String>,
}

pub fn parse_prompt_args(input: &str) -> ParsedPromptArgs {
    let mut out = ParsedPromptArgs { named: IndexMap::new(), positional: Vec::new() };
    for token in tokenize_args(input) {
        // `eq > 0` is STRICT: a token starting with `=` is POSITIONAL, not a named arg with an
        // empty key.
        match find_unquoted_equals(&token).filter(|eq| *eq > 0) {
            Some(eq) => {
                let key = token[..eq].trim().to_string();
                let value = strip_quotes(token[eq + 1..].trim()).to_string();
                if key.is_empty() {
                    out.positional.push(strip_quotes(&token).to_string());
                } else {
                    out.named.insert(key, value);
                }
            }
            None => out.positional.push(strip_quotes(&token).to_string()),
        }
    }
    out
}
```

Binding — the two loops, in order, **with no guard added to loop 2** (MCP-397a):

```rust
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
            Some(v) => Some(v.clone()),
            None => {
                let v = parsed.positional.get(positional_index).cloned();
                positional_index += 1;
                v
            }
        };
        if let Some(v) = value
            && !v.is_empty()
        {
            args.insert(arg.name.clone(), v);
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
```

`build_usage_message` is `Missing required argument{s}: {names}.\nUsage: /{command_name} {usage}`,
trimmed, with `{s}` only when more than one is missing.

Flattening (MCP-399):

```rust
/// `prompts.ts` `formatPromptResult` (13h §5.6). NOT `renderers::transform_mcp_content`
/// (renderers.rs:610) — that is tool-result shaping, with different casing, different bracket
/// text and an unknown arm that re-serializes JSON instead of contributing nothing.
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
            lines.push(format!("[{}] {text}", role_str(message.role)));
        }
    }
    lines.join("\n\n").trim().to_string()
}

fn extract_message_text(content: &ContentBlock) -> String {
    match content {
        ContentBlock::Text(t) => t.text.clone(),
        ContentBlock::Resource(r) => match &r.resource {
            ResourceContents::TextResourceContents { uri, text, .. } => {
                format!("[resource {uri}]\n{text}")
            }
            ResourceContents::BlobResourceContents { uri, .. } => format!("[resource {uri}]"),
        },
        // Em dash, not a hyphen.
        ContentBlock::ResourceLink(r) => {
            if r.name.is_empty() {
                format!("[resource_link {}]", r.uri)
            } else {
                format!("[resource_link {} \u{2014} {}]", r.uri, r.name)
            }
        }
        // rmcp's `mime_type`/`data` are non-optional `String`, so upstream's `?? "unknown"` and
        // `data ? …` become empty-string tests, not `Option` fallbacks.
        ContentBlock::Image(i) => {
            let mime = if i.mime_type.is_empty() { "unknown" } else { &i.mime_type };
            let embedded = if i.data.is_empty() { "" } else { " (embedded)" };
            format!("[image {mime}{embedded}]")
        }
        ContentBlock::Audio(a) => {
            let mime = if a.mime_type.is_empty() { "unknown" } else { &a.mime_type };
            format!("[audio {mime}]")
        }
        // `ContentBlock` is `#[non_exhaustive]`, so this arm is required by the compiler AND is
        // upstream's "anything else ⇒ ''". Do not turn it into a stringify.
        _ => String::new(),
    }
}
```

### 4 · MCP-041 — HA-2, three legs

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
/// The native arm of pi's `getArgumentCompletions`. `command_completions` (`:1861`) is the
/// live-WASM arm and cannot serve a native: `live_for_command` looks in the live-WASM map, so a
/// built-in errors ``command `X` has no live owner``.
pub fn native_command_completions(&self, name: &str, prefix: &str) -> Vec<(String, String)> {
    let Ok(Some((owner, registered))) = self.command_route(name) else { return Vec::new() };
    let Some(ext) = self.native.read().ok().and_then(|g| g.get(&owner).cloned()) else {
        return Vec::new();
    };
    ext.argument_completions(&registered, prefix)
}
```

`command_route` is already private-but-present at
[facade.rs:1897-1901](../../crates/cyrup-ext/src/facade.rs) and already carries the SEAM-048
registered name.

**(b) `cyrup-mcp`.** Implement it on `McpExtension`, reading **live** config:

```rust
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

fn argument_completions(&self, command: &str, prefix: &str) -> Vec<(String, String)> {
    // `/mcp-auth` deliberately declares NO completer upstream — an asymmetry with `/mcp`, kept.
    if command != crate::registration::MCP_COMMAND {
        return Vec::new();
    }
    let normalized = prefix.trim_start();
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
    let Some(state) = self.state() else { return Vec::new() };
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
already treats an empty candidate set as "no popup" ([autocomplete.rs:145-147](../../crates/cyrup-tui/src/autocomplete.rs)).

Then **delete `"token"`** from [registration.rs:1881](../../crates/cyrup-mcp/src/registration.rs) and
reduce the array to the eight values above, so the static declaration and the dynamic completer agree.

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
`CompletionContext::Argument` beside the three at [:26-36](../../crates/cyrup-tui/src/autocomplete.rs);
give `apply` the arm `CompletionContext::Argument => (completion.value.clone(), " ")` (the value
already carries its leading `/`, so unlike `Slash` nothing is re-prefixed); and add:

```rust
/// `/name arg…` — the argument-completion context `slash_context` bails out of (`:141`).
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

`Autocomplete::compute` ([:76-93](../../crates/cyrup-tui/src/autocomplete.rs)) takes one more
parameter and tries `slash_context` first, then `argument_context`, then `path_context`.
`InputEditor` ([editor.rs:96-120](../../crates/cyrup-tui/src/editor.rs) — a plain struct with no
derives, so this is free) gains `arg_completer: Option<Arc<dyn ArgumentCompleter>>` and a
`set_arg_completer`, threaded at both `compute` call sites
([editor.rs:1610](../../crates/cyrup-tui/src/editor.rs), [:1650](../../crates/cyrup-tui/src/editor.rs)).

Wiring: an adapter over `Arc<cyrup_ext::ExtensionHost>` implementing `ArgumentCompleter` by calling
`native_command_completions`, installed alongside `set_registry` in `rebuild_command_registry`
([execute_misc.rs:25-38](../../crates/cyrup-tui/src/app/execute_misc.rs)) — the one method with four
callers (boot, session swap, the `enableSkillCommands` toggle, and HA-1's late registration), so the
completer never outlives its session.

Last leg: `has_arg_completion` must stop being hardcoded `false`
([commands.rs:487](../../crates/cyrup-tui/src/commands.rs)). Emit
`"hasArgCompletion": true` on the extension rows of `slash_command_catalog`
([session.rs:2623-2634](../../crates/cyrup-session-svc/src/session.rs)) from
`ExtensionRegistry::command_autocomplete()`
([registry.rs:1013](../../crates/cyrup-ext/src/registry.rs)) — the producer that has existed with no
consumer — and read it in `dynamic_commands_from_catalog_gated`. `McpExtension::init` then calls
`api.add_autocomplete(MCP_COMMAND)` ([native.rs:413](../../crates/cyrup-ext/src/native.rs)) beside
its `register_command` at [registration.rs:2136](../../crates/cyrup-mcp/src/registration.rs), which
makes it the **first** production caller of a surface that has been write-only since it was added.

### 5 · MCP-362 + MCP-368 + MCP-377 — the overlay seam

**MCP-362.** Additive, defaulted, breaks no existing overlay. On `InteractiveOverlay`
([host/overlay.rs:289-292](../../crates/cyrup-ext/src/host/overlay.rs)):

```rust
/// Whether the overlay has decided to close itself without a keystroke — pi's
/// `setTimeout(() => done(...), INACTIVITY_MS)`. `tick` returns `bool` and cannot express this;
/// the host consults this after every tick and tears the overlay down when it is `true`.
fn should_close(&self) -> bool {
    false
}
```

Mirror it on `cyrup_tui::overlay::Overlay` ([overlay.rs:53-68](../../crates/cyrup-tui/src/overlay.rs))
with `ExtensionOverlay` delegating ([overlay.rs:184-190](../../crates/cyrup-tui/src/overlay.rs)), and
in `on_overlay_ticked` ([run_arms.rs:388-401](../../crates/cyrup-tui/src/app/run_arms.rs)):

```rust
let mut changed = false;
for overlay in self.state.overlays.iter_mut() {
    changed |= overlay.tick();
}
let before = self.state.overlays.len();
// Dropping the `ExtensionOverlay` fires its one-shot, releasing the blocked extension task —
// the same teardown path `handle_overlay_key`'s `pop()` takes (app/input.rs:382).
self.state.overlays.retain(|o| !o.should_close());
changed |= self.state.overlays.len() != before;
```

`McpPanelOverlay::should_close` / `McpSetupOverlay::should_close` then return `self.expired`, and the
`TODO(MCP-362)` blocks at [ui.rs:3263-3270](../../crates/cyrup-mcp/src/ui.rs) and
[ui.rs:4597-4599](../../crates/cyrup-mcp/src/ui.rs) reduce to setting `expired`, calling
`model.expire()`, publishing, and returning `true`. Keep the resolved-cadence residue in the comment:
a polled deadline fires within one `REFRESH_MS`, so the panel lives up to 250 ms longer than
upstream's `setTimeout`, and at `refresh_ms() == 0` it would never auto-cancel at all.

**MCP-368 (HA-3), and the one place this task diverges from its spec.** MCP-368 recommends option
(a): an `OverlayOptions` bag threaded through `open_overlay` and `OverlayRequest`
([13h :1326-1352](../../docs/gap-analysis/13h-mcp-tui.md)). **Do not do that.** `open_overlay` is a
`HostServices` trait method with a default body
([services.rs:254](../../crates/cyrup-ext/src/host/services.rs)), a `LiveHostServices` impl
([host_services.rs:1043](../../crates/cyrup-session-svc/src/host_services.rs)) and — critically — a
`fenced!` macro arm in `OwnedServices` ([owner.rs:415-418](../../crates/cyrup-mcp/src/owner.rs))
whose arity the macro fixes. Widening it edits five sites across three crates to carry a value the
component already knows.

Instead put the bag on the trait the component already implements, which is also where upstream puts
it (`ctx.ui.custom(factory, { overlay: true, overlayOptions })` travels **with the factory**). In
[host/overlay.rs](../../crates/cyrup-ext/src/host/overlay.rs):

```rust
/// pi's `overlayOptions` (`interactive-mode.ts:2719`). Defaults are today's constants, so
/// `FleetOverlay` and `PermissionSystemSettingsOverlay` are untouched.
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
    /// `ExtensionOverlay::box_rect` AND by an overlay that windows its own body (MCP-377), so
    /// the two cannot disagree.
    #[must_use]
    pub fn max_rows(&self, frame_height: u16) -> u16 { /* margin, then max_height_pct */ }
}
```

plus `fn options(&self) -> OverlayOptions { OverlayOptions::default() }` on `InteractiveOverlay`.
`ExtensionOverlay::box_rect` ([overlay.rs:110-132](../../crates/cyrup-tui/src/overlay.rs)) takes the
options instead of reading the four `const`s (keep the consts as `Default`), and
`ExtensionOverlay::render` ([overlay.rs:146-171](../../crates/cyrup-tui/src/overlay.rs)) reads
`self.inner.options()` once per frame. `McpPanelOverlay` returns `width: Some(82)`,
`McpSetupOverlay` returns `width: Some(92)`.

**MCP-377 (the height half).** Do **not** change `McpSetupPanelModel::render(&self, width)` — it has
thirteen call sites in `ui.rs`'s own test module
([ui.rs:4887](../../crates/cyrup-mcp/src/ui.rs) through [ui.rs:5932](../../crates/cyrup-mcp/src/ui.rs)).
Add a sibling and delegate:

```rust
/// `render` with a row budget (MCP-377). `render(width)` is `render_bounded(width, usize::MAX)`.
pub fn render_bounded(&self, width: usize, max_rows: usize) -> Vec<OverlayLine> { … }
```

Inside, window the action list at **every** width (not only `inner_w < COMPACT_WIDTH`) by reusing
`visible_action_range` ([ui.rs:4005-4021](../../crates/cyrup-mcp/src/ui.rs)) with a row count derived
from `max_rows` rather than the fixed `COMPACT_ACTION_ROWS`, and truncate the `action_preview` block
([ui.rs:4076-4082](../../crates/cyrup-mcp/src/ui.rs)) so the trailing blank, hint row and bottom
border always fit. The "Add a known server" heading rule
([ui.rs:4048-4055](../../crates/cyrup-mcp/src/ui.rs)) already keys off *first visible row* and needs
no change — that is why it was written that way.

`McpSetupOverlay::render` ([ui.rs:4571](../../crates/cyrup-mcp/src/ui.rs)) stops discarding its
`height` argument:

```rust
fn render(&mut self, width: usize, height: usize) -> Vec<OverlayLine> {
    let budget = self.options().max_rows(u16::try_from(height).unwrap_or(u16::MAX));
    self.model.render_bounded(width, budget as usize)
}
```

### 6 · MCP-049 — `cyrup mcp init`

In [subcommands.rs](../../crates/cyrup/src/subcommands.rs): extend the table at
[:35](../../crates/cyrup/src/subcommands.rs) to `[&str; 7]` with `"mcp"`, and add an arm at the top
of `dispatch` ([:450](../../crates/cyrup/src/subcommands.rs)) beside the existing special-cased
`config` arm:

```rust
if argv.first().map(String::as_str) == Some("mcp") {
    return Ok(Some(run_mcp_subcommand(argv.get(1..).unwrap_or_default(), dirs).await?));
}
```

`run_mcp_subcommand` recognises `init` only; `install` prints the two retirement errors and returns
1; anything else prints `Unknown command: {command}` plus the help and returns 1. The code becomes
the process exit code through `main.rs`'s existing `return Ok(code)`
([main.rs:188-190](../../crates/cyrup/src/main.rs)).

`run_init` builds the context the extension builds
([extension.rs:806-815](../../crates/cyrup-mcp/src/extension.rs) is the shape):

```rust
let mcp_dirs = cyrup_mcp::dirs::McpDirs::new(dirs.agent_dir.clone(), dirs.cwd.clone());
let ctx = cyrup_mcp::config::ConfigContext::new(mcp_dirs, None).with_home(dirs.home.clone());
```

then, in upstream's step order: `find_available_import_configs`, the discovery table from
`config_discovery_paths` with the `✓`/`-` prefix, the `Compatibility imports:` block, the
`discoverySettingChanged` test, the no-op message pair, `ensure_compatibility_imports(&to_add)`, the
new `settings.hostConfigDiscovery` writer, `Updated {path}`, the **one unconditional** explanatory
line and the **one conditional** line gated on `discoverySettingChanged`. `--dry-run` prints
`Dry run: would update {path}` and writes nothing — so it must be tested **before** either writer,
not by rolling back after.

The new writer, beside `ensure_compatibility_imports`
([config.rs:3749](../../crates/cyrup-mcp/src/config.rs)):

```rust
/// Set `settings.hostConfigDiscovery = "on"` in the adapter-owned global file, preserving
/// comments (`read_raw_config_object` / `write_raw_config_object`). Returns `false` and writes
/// nothing when it is already `"on"`, so a second `cyrup mcp init` does not touch the mtime.
pub fn enable_host_config_discovery(&self) -> McpResult<bool> { … }
```

JSONC reading for anything this cannot express goes through `cyrup_permission_system::jsonc`
(`crates/cyrup-permission-system/src/jsonc.rs`), which the `cyrup` bin already depends on
([Cargo.toml:68](../../crates/cyrup/Cargo.toml)) — the same parser `cyrup-permission-system` uses on
`mcp.json`, so both read that file identically by construction.

---

## Acceptance Criteria

### The seam

- [ ] `McpExtension` implements `NativeExtension::execute_command`; invoking `/mcp` no longer
      produces ``native extension has no handler for command `mcp` ``
      ([native.rs:586](../../crates/cyrup-ext/src/native.rs)).
- [ ] The handler calls `HostCtx::require_command_tier()` itself before any side effect, and every
      arm returns `Ok(None)` after notifying at its own level — no arm returns `Ok(Some(_))`
      alongside a `notify`.
- [ ] The prologue snapshots `has_ui`, `mode`, `cwd`, the `OwnedServices` handle and the owner
      **before** the first `.await`, awaits `init_task` with **no** timeout, emits
      `MCP initialization failed: {message}` then `MCP not initialized`, and re-checks
      `owner.is_active()` before **every** side effect, not once at entry.
- [ ] `/mcp-auth` with an empty argument and `has_ui == false` returns silently — no output, no
      error, and **no** initialization wait — and this is observable as the init task never being
      awaited on that path.
- [ ] `/mcp logout my server` targets `"my server"`; `/mcp reconnect a b` targets `"a"`;
      `/mcp wibble` opens the browser panel rather than erroring; `/mcp logout` with no argument
      emits `Usage: /mcp logout <server>` and performs no further work.

### The listings

- [ ] `/mcp status` emits one Info notification whose body starts `MCP Server Status:` + a blank
      line, with one row per server in `config.mcp_servers` insertion order.
- [ ] A disabled server renders `⊘ {name}: disabled (run /mcp enable {name}, then /reload)` with
      **no** tool suffix.
- [ ] A server with a recorded failure **and** cached metadata renders `✗ … failed {N}s ago — …`
      with **no** tool suffix and never reports `cached`.
- [ ] A single tool renders `Total: 1 tools`; a single prompt renders `Total: 1 prompt`.
- [ ] `/mcp prompts` over two servers emits an unindented, icon-free `{server}:` header per group,
      two-space `/{command}` rows, six-space description rows, and a blank line after each group.
- [ ] A connected server carrying `promptDiscoveryFailed` adds the trailing
      `Prompt discovery failed for: …. Cached prompt metadata may be stale.` line, and the
      empty-list case instead appends the leading-space variant to the single sentence.
- [ ] The `localeCompare`-vs-`str::cmp` divergence is stated in a comment at the sort site.

### The arms

- [ ] `/mcp disable srv` writes through `write_project_server_disabled_override`
      ([config.rs:3484](../../crates/cyrup-mcp/src/config.rs)) and emits
      `Disabled server "srv" in {path} — run /reload to apply`; a no-op re-disable emits
      `Server "srv" is already disabled` and does not touch the file's mtime.
- [ ] An unknown name emits `Server "{name}" not found in effective config`; an empty name emits
      `Usage: /mcp disable <server>`; a programmatic config emits
      `/mcp disable is unavailable when config is supplied by createMcpAdapter().` at Info.
- [ ] Bare `/mcp-auth` with a UI opens the panel built from `PanelOptions { auth_only: true, .. }`
      ([ui.rs:1443](../../crates/cyrup-mcp/src/ui.rs)) with the notice line
      `Select an OAuth MCP server and press Enter or ctrl+a to authenticate.`, and the entry point
      reports `config_changed == false` even after a successful authentication.
- [ ] With zero non-disabled OAuth-capable servers, bare `/mcp-auth` emits
      `No OAuth-capable MCP servers are configured.` at `NotifyKind::Warning` and does **not** open
      the panel; the panel's own `No OAuth-capable MCP servers configured.` row
      ([ui.rs:2670](../../crates/cyrup-mcp/src/ui.rs)) is left unchanged.
- [ ] `/mcp-auth <server>` emits each of the five guard messages from
      [oauth.rs:3800-3831](../../crates/cyrup-mcp/src/oauth.rs) in the order no-UI → unknown →
      disabled → not-OAuth → no-URL, sets the `mcp-auth` status key to
      `msg_authenticating(name)`, and clears it on completion but **not** when the signal aborted.

### The prompt grammar

- [ ] `parse_prompt_args` over `a b`, `a="x y"`, `'a=b'`, `=x`, `a\ b`, `"unterminated`, a trailing
      lone backslash and empty input each produce upstream's tokens: `=x` is **positional**,
      `'a=b'` is **positional**, the trailing backslash is **dropped**, and an unterminated quote
      runs to end of input.
- [ ] With two required and one optional declared argument, `topic=x today` binds `topic` by name
      and `today` to the **first unbound** declared argument — the positional cursor does not
      advance on the named hit.
- [ ] An undeclared named argument survives into the resolved map unfiltered.
- [ ] `topic=` against a declared **optional** `topic` yields `{"topic": ""}`; the same input
      against a declared **required** `topic` yields `Missing required argument: topic.` plus the
      usage line. Two missing arguments produce `Missing required arguments: a, b.`
- [ ] `format_prompt_result` over a single `user` text message returns it verbatim; over two
      messages returns `[user] …\n\n[assistant] …`; over a single **assistant** message keeps the
      `[assistant] ` prefix.
- [ ] Each of the five `ContentBlock` variants produces its exact placeholder (`[resource {uri}]`,
      `[resource_link {uri} — {name}]` with an em dash, `[image {mime} (embedded)]`,
      `[audio {mime}]`), an empty extraction is skipped, and an empty `mime_type` renders
      `unknown`.
- [ ] `renderers::transform_mcp_content` ([renderers.rs:616](../../crates/cyrup-mcp/src/renderers.rs))
      is unchanged and is not called from the prompt path.

### The host additions

- [ ] `NativeExtension::argument_completions` exists with a defaulted empty body, and
      `ExtensionHost::native_command_completions` resolves a **native** owner (no
      ``command `mcp` has no live owner``).
- [ ] `mcp_command_descriptor` ([registration.rs:1871](../../crates/cyrup-mcp/src/registration.rs))
      declares exactly the eight upstream subcommands; `"token"` is gone.
- [ ] `/mcp ` + Tab in a live pty offers the eight subcommands with their em-dash labels; with two
      configured servers, `/mcp reconnect ` + Tab offers both server names and accepting one
      rewrites the buffer to `/mcp reconnect <name> ` with a trailing space.
- [ ] `/mcp reconnect linear` typed in full still works — the completer is additive and no
      keystroke path regresses.
- [ ] `slash_command_catalog` emits `hasArgCompletion` for extension rows that declared a
      completer, and `dynamic_commands_from_catalog_gated`
      ([commands.rs:487](../../crates/cyrup-tui/src/commands.rs)) reads it instead of hardcoding
      `false`.
- [ ] Both panels close **on their own** after 60 s with no keystroke, and a key at 59 s re-arms
      the deadline; the `TODO(MCP-362)` comments at
      [ui.rs:3263](../../crates/cyrup-mcp/src/ui.rs) and
      [ui.rs:4597](../../crates/cyrup-mcp/src/ui.rs) are gone.
- [ ] Closing by timer releases the blocked extension task (the `ExtensionOverlay` one-shot fires),
      exactly as a `Close` keystroke does.
- [ ] `OverlayOptions` exists with today's four constants as `Default`; the browser panel paints at
      82 columns and the setup panel at 92 at both a 100- and a 200-column terminal, while
      `FleetOverlay` and `PermissionSystemSettingsOverlay` paint unchanged.
- [ ] A setup-panel frame with a 40-line preview at a 20-row terminal still emits its hint row and
      bottom border **inside** the first 20 lines; the action list windows at `inner_w >= 60` as
      well as below it, and the compact-width behaviour at `inner_w = 58` is unchanged (a 13-action
      list with the cursor at index 9 still emits `… 6 more above`, seven rows, and no
      `… more below`).
- [ ] `McpSetupPanelModel::render(&self, width)` still exists and still compiles every existing
      call site in `ui.rs`.

### Out of session

- [ ] `cyrup mcp init` is reachable: `"mcp"` is in `SUBCOMMANDS`
      ([subcommands.rs:35](../../crates/cyrup/src/subcommands.rs)) and `first_subcommand` matches it.
- [ ] With `~/.cursor/mcp.json` and `~/.codex/config.toml` present in a temp HOME, a first run adds
      `imports: ["cursor", "codex"]` to the adapter-owned global file and a second run prints
      `No Pi config changes needed.` and writes nothing.
- [ ] `--dry-run` prints `Dry run: would update {path}` and leaves both the imports and the
      `hostConfigDiscovery` setting untouched.
- [ ] A `--discover-host-configs` run that only changes the setting emits exactly two trailing
      lines; a plain run emits one.
- [ ] Comments in an existing JSONC `mcp.json` survive the write, and the divergence from
      upstream's comment-destroying `writePiConfig` is stated in a comment at the call site.
- [ ] An existing `mcp-servers` key is normalised to `mcpServers` in the written file.
- [ ] `cyrup mcp install` prints the two retirement errors and exits 1; `cyrup mcp wibble` prints
      `Unknown command: wibble` plus the help and exits 1.
