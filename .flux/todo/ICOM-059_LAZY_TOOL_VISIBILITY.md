---
stage: aug
status: done
updated: 2026-08-27 22:33
---

# Add the toolVisibility config key and lazy intercom tool visibility

> **Upstream parity gap.** `cyrup-intercom` is a port of `pi-intercom` **v0.9.2**; upstream is now
> **v0.12.0** (`ef95f19`, 2026-08-22) at [`nicobailon/pi-intercom`](https://github.com/nicobailon/pi-intercom).
> Reference checkout: `./tmp/pi-intercom`. Gap analysis: `docs/gap-analysis/11-cyrup-intercom.md` — **ICOM-059**.

## 1. What upstream does

`12f4b6c` (issue #113, v0.12.0) adds a ninth config key, `toolVisibility`, and uses it to keep the
generic `intercom` tool — **its JSON schema AND its prompt snippet** — out of the model's active tool
set until the session actually touches intercom.

### 1.1 The config key — [`../../tmp/pi-intercom/config.ts`](../../tmp/pi-intercom/config.ts)

```ts
export type InboundTriggerPolicy = "always" | "replies" | "never";
export type IntercomToolVisibility = "always" | "after-first-use";   // :27
```

```ts
  /** Controls when the intercom tool enters the active model tool set */
  toolVisibility: IntercomToolVisibility;                            // :42-43
```

```ts
const defaults: IntercomConfig = {
  brokerCommand: "npx",
  brokerArgs: ["--no-install", "tsx"],
  confirmSend: false,
  inboundTrigger: "always",
  toolVisibility: "always",                                          // :67
  enabled: true,
  replyHint: true,
};
```

Validation is a verbatim clone of the `inboundTrigger` block immediately above it (`config.ts:138-146`):

```ts
    if (Object.hasOwn(parsedConfig, "toolVisibility")) {
      if (
        parsedConfig.toolVisibility !== "always"
        && parsedConfig.toolVisibility !== "after-first-use"
      ) {
        throw new Error(`"toolVisibility" must be "always" or "after-first-use"`);
      }
      config.toolVisibility = parsedConfig.toolVisibility;
    }
```

The throw is caught by `loadConfig`'s single `catch` and re-thrown as
`` `Failed to load intercom config at ${configPath}: ${message}` `` (`config.ts:174-177`). **There is
no fallback arm.** An unknown value fails the load, exactly as a corrupt JSON file does.

### 1.2 The gating — [`../../tmp/pi-intercom/index.ts`](../../tmp/pi-intercom/index.ts)

Two closure helpers plus one boolean (`index.ts:563-579`):

```ts
  let intercomToolHiddenByPolicy = false;
  function hideIntercomTool(): void {
    if (config.toolVisibility !== "after-first-use") return;
    const activeToolNames = pi.getActiveTools();
    if (!activeToolNames.includes(INTERCOM_TOOL_NAME)) return;
    pi.setActiveTools(activeToolNames.filter((name) => name !== INTERCOM_TOOL_NAME));
    intercomToolHiddenByPolicy = true;
  }
  function activateIntercomTool(): void {
    if (!intercomToolHiddenByPolicy) return;
    const activeToolNames = pi.getActiveTools();
    if (!activeToolNames.includes(INTERCOM_TOOL_NAME)) {
      pi.setActiveTools([...activeToolNames, INTERCOM_TOOL_NAME]);
    }
    intercomToolHiddenByPolicy = false;
  }
```

**Registration is untouched.** `pi.registerTool(intercomTool)` still runs unconditionally; the tool is
removed from the ACTIVE set by `hideIntercomTool()`, whose only call site is the first statement of
`startSessionRuntime` (`index.ts:1310`) — i.e. every `session_start`.

The four reveal sites, and the one deliberate non-site:

| # | Site | upstream |
| - | ---- | -------- |
| 1 | inbound **broker** message, immediately before injection | `sendIncomingBrokerMessage` wraps `sendIncomingMessage` (`index.ts:958-961`), swapped into BOTH the `"steer"` and `"trigger"` arms of `handleIncomingMessage` (`:1019`, `:1023`) |
| 2 | any `intercom` tool call | `activateIntercomTool()` is the FIRST statement of `execute` (`index.ts:1914`), before `ensureConnected` |
| 3 | a successful overlay send | inside `if (result?.sent && result.messageId && result.text && getLiveContext(...))` (`index.ts:2493-2494`) |
| 4 | loading the bundled skill | `pi.on("input")` matching `/^\/skill:pi-intercom(?:\s|$)/u` on `event.text.trimStart()` (`index.ts:1552-1556`), and `pi.on("tool_result")` for a NON-error `read` whose `path` realpath-equals the packaged `SKILL.md` (`isIntercomSkillRead`, `index.ts:95-105`, dispatched at `:1558-1562`) |
| — | **local in-process subagent relay** | deliberately NOT a reveal — `subagent:control-intercom` traffic still goes through the un-wrapped `sendIncomingMessage`, asserted by `intercom.integration.test.ts:1400-1405` ("Local relay traffic should not reveal the generic tool") |

Also not a reveal: the non-interactive busy auto-reply arm (it `return`s before `sendIncomingBrokerMessage`),
and `contact_supervisor` — the CHANGELOG is explicit: *"Broker reception and the child-only
`contact_supervisor` tool remain available while it is hidden."*

### 1.3 "First use", pinned

**The event that flips the flag is the FIRST of the four rows above to occur in this session**, and each
reveal is a `setActiveTools` write, not a marker anywhere else. Precisely:

- Row 1 flips it when a broker message reaches the **injection** call (steer or trigger), *before*
  `inject_message` runs, so the very turn that message drives already carries the tool
  (`intercom.integration.test.ts:1480` asserts `sentMessages[0].activeTools.includes("intercom")`).
  A message that is only surfaced/acknowledged, or answered with the busy auto-reply, does **not** flip it.
- Row 2 flips it on tool ENTRY, not on success — an `intercom` call that fails to connect still reveals.
- Row 3 flips it only on a **delivered** send.
- Row 4 flips it on the skill being *loaded* (the `/skill:pi-intercom` submission, or a successful `read`
  of the packaged `SKILL.md`); an ERRORED read does not (`intercom.integration.test.ts:1412-1417`).

**The flip does NOT persist across a restart, and is not written anywhere.** It is a closure `let`
inside `piIntercomExtension`, and `hideIntercomTool()` runs at the head of `startSessionRuntime`, so a
new session (or a runtime restart) re-hides. Upstream asserts exactly this — after a reveal, the test
re-emits `session_start` and requires the tool to be gone again
(`intercom.integration.test.ts:1426-1427`):

```ts
      await harness.emitLifecycle("session_start");
      assert.equal(harness.getActiveTools().includes("intercom"), false);
```

Port it as **session-scoped, reset on every `SessionStart`**. Nothing lands in `~/.cyrup/agent/intercom/`.

## 2. What already exists in the port — reuse it, do not duplicate

| Need | Already in the port |
| ---- | ------------------- |
| The enum + serde idiom | [`config.rs:19-29`](../../crates/cyrup-intercom/src/config.rs) — `InboundTrigger` derives `Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize` with `#[serde(rename_all = "lowercase")]` and `#[default]` on `Always` |
| The parse/validate idiom | [`config.rs:146-153`](../../crates/cyrup-intercom/src/config.rs) — `parse_config` hand-matches `v.as_str()` and returns upstream's message verbatim; it does NOT go through serde |
| The hard-error-with-path wrapper | [`config.rs:98-109`](../../crates/cyrup-intercom/src/config.rs) — `load_config` already maps every `parse_config` `Err` to `Failed to load intercom config at {path}: {message}`. **A new key needs zero new error plumbing**; returning the message from `parse_config` is sufficient, and the existing proof is `load_config_errors_with_the_path_on_a_corrupt_file` at [`config.rs:241-252`](../../crates/cyrup-intercom/src/config.rs) |
| `getActiveTools` / `setActiveTools` | [`cyrup-ext/src/host/services.rs:641`](../../crates/cyrup-ext/src/host/services.rs) `HostServices::active_tools() -> Option<Vec<String>>` and `:667` `set_active_tools(&[String])`. Live-backed by `cyrup-session-svc/src/host_services.rs:1764-1791`: `set_active_tools` mutates `DynamicToolState` **synchronously** and queues the rebuilt `(tools, prompt)` — so the tool's schema *and* its `prompt_snippet` leave the next turn's request, which is exactly what upstream's "keeping its model schema and prompt out of unused sessions" means |
| Access to those from anywhere | [`session_state.rs:299-301`](../../crates/cyrup-intercom/src/session_state.rs) `SharedIntercomState::host_services()`; `inbound.rs`'s background loop only ever holds `&SharedIntercomState`, so the flag and both helpers belong there |
| The per-session boolean idiom | [`session_state.rs:114`](../../crates/cyrup-intercom/src/session_state.rs) `has_ui: AtomicBool` + `set_has_ui`/`has_ui` (`:306-316`) — same shape, same `Ordering::SeqCst` |
| The bundled skill path | [`resources.rs:56-66`](../../crates/cyrup-intercom/src/resources.rs) `bundled_skill_files()` already resolves `resources/skills/pi-intercom/SKILL.md` (honouring `CYRUP_INTERCOM_RESOURCES_DIR`). The skill's frontmatter `name: pi-intercom` makes cyrup's command `/skill:pi-intercom`, byte-identical to upstream's regex |
| The raw input text | `HostEvent::Input { text, .. }` ([`cyrup-ext/src/event.rs:361`](../../crates/cyrup-ext/src/event.rs)) is dispatched by `cyrup-session-svc/src/session/run.rs:329` **before** `prepare_and_assemble` expands `/skill:` at `:412-414` — so the handler sees the un-expanded `/skill:pi-intercom`, as upstream's does |
| The read-tool result | `HostEvent::ToolResult { name, input, is_error, .. }` ([`event.rs:284-297`](../../crates/cyrup-ext/src/event.rs)), fired for built-ins too via `cyrup-ext/src/hooks.rs:85` `after_tool_call`. cyrup's read tool is named `"read"` with a `"path"` param ([`cyrup-tools/src/tools/read.rs:41-60`](../../crates/cyrup-tools/src/tools/read.rs)) — same two strings upstream keys on |

`toolVisibility` is the **only** one of upstream's nine config keys with no counterpart in
[`config.rs`](../../crates/cyrup-intercom/src/config.rs): `brokerArgs`, `brokerCommand`, `confirmSend`,
`enabled`, `inboundTrigger`, `replyHint`, `stableId` and `status` are all present and all parsed by the
same block. The idiom to follow is three lines above the insertion point.

## 3. Required implementation

### 3.1 `config.rs` — the key

Add the enum next to `InboundTrigger`, in its exact shape (kebab-case gives `"after-first-use"`):

```rust
/// `IntercomToolVisibility` (`v0.12.0 config.ts:27`): when the generic `intercom` tool enters the
/// ACTIVE model tool set. Default `Always` (`config.ts:67`). Does not affect `contact_supervisor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IntercomToolVisibility {
    /// The tool is in the active set from session start, as it always was.
    #[default]
    Always,
    /// The tool is withheld until this session's first intercom use (`crate::session_state`).
    AfterFirstUse,
}
```

Add the field to `IntercomConfig` directly after `inbound_trigger` (upstream's position), and to
`impl Default for IntercomConfig`:

```rust
    /// Inbound auto-trigger policy (`config.ts:53`, `Always`).
    pub inbound_trigger: InboundTrigger,
    /// When the `intercom` tool enters the active tool set (`v0.12.0 config.ts:42-43`, `Always`).
    pub tool_visibility: IntercomToolVisibility,
```

Validate in `parse_config`, immediately after the `inboundTrigger` block, hand-matched exactly as that
one is — **not** via `serde_json::from_value`, whose error text would not be upstream's:

```rust
    if let Some(v) = obj.get("toolVisibility") {
        config.tool_visibility = match v.as_str() {
            Some("always") => IntercomToolVisibility::Always,
            Some("after-first-use") => IntercomToolVisibility::AfterFirstUse,
            _ => return Err("\"toolVisibility\" must be \"always\" or \"after-first-use\"".to_string()),
        };
    }
```

That `Err` is a **hard config error**: `load_config` already prefixes it with
`Failed to load intercom config at {path}: `, so `{"toolVisibility":"lazy"}` fails the whole load and
names both the key and the file. Do not add a fallback, a `warn!`, or a `unwrap_or_default()` anywhere
on this path — a silent fall back to `"always"` is indistinguishable from a deliberate default and is
the precise defect v0.10.0 removed from this crate (see the doc comment at `config.rs:83-94`).

### 3.2 `session_state.rs` — the flag and the two helpers

Add one field beside `has_ui`, and the pair of methods. This is the ONLY home: the inbound loop reaches
state but not the extension.

```rust
    /// `intercomToolHiddenByPolicy` (`v0.12.0 index.ts:563`) — whether THIS session currently has the
    /// `intercom` tool withheld under `toolVisibility: "after-first-use"`. Session-scoped and never
    /// persisted: re-armed by [`Self::hide_intercom_tool`] on every `SessionStart`, exactly as
    /// upstream's closure `let` is re-armed at the head of `startSessionRuntime` (`index.ts:1310`).
    intercom_tool_hidden_by_policy: AtomicBool,
```

```rust
    /// `hideIntercomTool()` (`v0.12.0 index.ts:564-570`) — called FIRST in the `SessionStart` arm.
    /// A no-op under `Always`, under a degraded host (no `HostServices`, or no live dynamic-tool
    /// view), and when the tool is not in the active set to begin with.
    pub fn hide_intercom_tool(&self) {
        if self.config.tool_visibility != IntercomToolVisibility::AfterFirstUse {
            return;
        }
        let Some(services) = self.host_services() else { return };
        let Some(active) = services.active_tools() else { return };
        if !active.iter().any(|n| n == crate::tools::intercom::INTERCOM_TOOL_NAME) {
            return;
        }
        let kept: Vec<String> =
            active.into_iter().filter(|n| n != crate::tools::intercom::INTERCOM_TOOL_NAME).collect();
        services.set_active_tools(&kept);
        self.intercom_tool_hidden_by_policy.store(true, Ordering::SeqCst);
    }

    /// `activateIntercomTool()` (`v0.12.0 index.ts:571-578`) — the reveal. Idempotent and cheap: a
    /// relaxed-path early return under `Always` and after the first reveal, so the four call sites
    /// may call it unconditionally.
    pub fn reveal_intercom_tool(&self) {
        if !self.intercom_tool_hidden_by_policy.load(Ordering::SeqCst) {
            return;
        }
        let Some(services) = self.host_services() else { return };
        // CYRUP-DELTA (strictly safer, unreachable upstream): the flag is cleared only once the write
        // actually lands. `pi.getActiveTools()` cannot fail; `HostServices::active_tools` answers
        // `None` when no dynamic-tool view is attached, and clearing the flag on that answer would
        // strand the tool hidden with no second chance. Leaving it set retries at the next reveal.
        let Some(mut active) = services.active_tools() else { return };
        if !active.iter().any(|n| n == crate::tools::intercom::INTERCOM_TOOL_NAME) {
            active.push(crate::tools::intercom::INTERCOM_TOOL_NAME.to_string());
            services.set_active_tools(&active);
        }
        self.intercom_tool_hidden_by_policy.store(false, Ordering::SeqCst);
    }
```

Initialise the field to `AtomicBool::new(false)` in `SharedIntercomState::new` (`session_state.rs:170-186`).

### 3.3 `tools/intercom/mod.rs` — one shared name constant, and the reveal on call

Upstream hoists `const INTERCOM_TOOL_NAME = "intercom"` (`index.ts:31`) precisely because the string is
now load-bearing in two places. Do the same, and make `Tool::name` return it so the gate and the
registration can never drift:

```rust
/// `INTERCOM_TOOL_NAME` (`v0.12.0 index.ts:31`). The registered name AND the name the
/// `toolVisibility` gate adds/removes from the active set — one constant, so they cannot drift.
pub const INTERCOM_TOOL_NAME: &str = "intercom";
```

```rust
    fn name(&self) -> &str {
        INTERCOM_TOOL_NAME
    }
```

Reveal as the FIRST statement of `execute` (`tools/intercom/mod.rs:341-352`), ahead of the params
parse, matching `index.ts:1914`:

```rust
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        // `activateIntercomTool()` (`v0.12.0 index.ts:1914`) — on ENTRY, before validation and before
        // `ensureConnected`, so a call that fails to parse or connect still counts as first use.
        self.state.reveal_intercom_tool();
        let parsed: IntercomParams = serde_json::from_value(params)
            .map_err(|e| ToolError::new(format!("invalid intercom tool call: {e}")))?;
        self.dispatch(parsed, &cancel).await
    }
```

### 3.4 `resources.rs` — `isIntercomSkillRead`

Port upstream's helper here, over the EXISTING `bundled_skill_files()`; do not hardcode a second path:

```rust
/// The canonicalized bundled `SKILL.md` (`INTERCOM_SKILL_PATH`, `v0.12.0 index.ts:32`). Resolved once
/// — upstream's `realpathSync(fileURLToPath(...))` runs at module load.
fn bundled_skill_realpath() -> Option<&'static PathBuf> {
    static PATH: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| bundled_skill_files().first().and_then(|p| std::fs::canonicalize(p).ok()))
        .as_ref()
}

/// `isIntercomSkillRead(input, cwd)` (`v0.12.0 index.ts:95-105`): does this `read` call's `path` name
/// the bundled intercom skill? A leading `@` is stripped (pi's file-mention form), the rest is
/// resolved against `cwd` and canonicalized; an unresolvable path is `false`, never an error.
#[must_use]
pub fn is_bundled_skill_read(input: &serde_json::Value, cwd: &Path) -> bool {
    let Some(raw) = input.get("path").and_then(serde_json::Value::as_str) else { return false };
    let Some(target) = bundled_skill_realpath() else { return false };
    let normalized = raw.strip_prefix('@').unwrap_or(raw);
    std::fs::canonicalize(cwd.join(normalized)).is_ok_and(|p| &p == target)
}
```

### 3.5 `extension.rs` — subscriptions, the hide, and the two new event arms

**Registration stays exactly as it is.** `api.register_tool(Arc::new(IntercomTool::new(...)))` at
`extension.rs:447` remains unconditional, and the `contact_supervisor` gate at `:453-457` is untouched.
This is not incidental: `HostServices::set_active_tools` can only re-activate names already in
`DynamicToolState`'s registry (`cyrup-session-svc/src/tools.rs:149-152`, "unknown names are ignored"),
so a tool that was never registered could never be revealed mid-session. Hiding is an ACTIVE-SET
operation, never a registration one.

In `init`, extend the `api.subscribe(&[...])` list — **conditionally**, so the default `Always` config
pays nothing. `ToolResult` is a blocking mutate dispatch on every tool call in the session; subscribing
it for a session that can never use it is a real cost upstream's closures had no way to avoid:

```rust
        // `v0.12.0 index.ts:1552-1562` — the bundled-skill reveal needs the raw submitted text
        // (`pi.on("input")`) and the `read` tool's result (`pi.on("tool_result")`). Subscribed ONLY
        // under `after-first-use`: `ToolResult` is a blocking mutate dispatch on EVERY tool call, and
        // under `always` there is nothing to reveal.
        if self.state.config.tool_visibility == IntercomToolVisibility::AfterFirstUse {
            api.subscribe(&[EventKind::Input, EventKind::ToolResult]);
        }
```

In `on_event`, make `hide_intercom_tool()` the FIRST statement of the `SessionStart` arm
(`extension.rs:546`), ahead of `set_has_ui` — upstream's position at the head of `startSessionRuntime`:

```rust
            HostEvent::SessionStart { .. } => {
                // `hideIntercomTool()` (`v0.12.0 index.ts:1310`), the first statement of
                // `startSessionRuntime`: under `after-first-use` the tool leaves the active set at the
                // head of EVERY session, so a reveal never survives a restart.
                self.state.hide_intercom_tool();
                self.state.set_has_ui(ctx.has_ui);
                // … unchanged …
```

Add the two arms before the `_ => HookOutcome::Noop` catch-all:

```rust
            // `pi.on("input")` (`v0.12.0 index.ts:1552-1556`) — the bundled skill invoked as a slash
            // command. cyrup's `Input` event carries the RAW submission (`session/run.rs:329` runs
            // before `/skill:` expansion at `:412`), so this sees the same text upstream's regex does.
            HostEvent::Input { text, .. } => {
                let head = text.trim_start();
                if let Some(rest) = head.strip_prefix("/skill:pi-intercom")
                    && (rest.is_empty() || rest.starts_with(char::is_whitespace))
                {
                    self.state.reveal_intercom_tool();
                }
                HookOutcome::Noop
            }
            // `pi.on("tool_result")` (`v0.12.0 index.ts:1558-1562`) — a SUCCESSFUL `read` of the
            // bundled `SKILL.md` counts as loading the skill. An errored read does not.
            HostEvent::ToolResult { name, input, is_error, .. } => {
                if name == "read"
                    && !*is_error
                    && crate::resources::is_bundled_skill_read(input, &ctx.cwd)
                {
                    self.state.reveal_intercom_tool();
                }
                HookOutcome::Noop
            }
```

Finally, the overlay reveal in `run_intercom_command`, immediately after the send succeeds
(`extension.rs:377`) and before the `intercom_sent` audit append. `compose_send` returns `Err` unless the
broker confirmed delivery (`ui/compose.rs:229-235`), so reaching this line is exactly upstream's
`result?.sent && result.messageId && result.text` guard:

```rust
        let sent = compose_send(client, &target_id, &message).await?;
        // `activateIntercomTool()` (`v0.12.0 index.ts:2494`) — a DELIVERED overlay send is first use.
        self.state.reveal_intercom_tool();
```

### 3.6 `inbound.rs` — the broker-only reveal

Port `sendIncomingBrokerMessage` (`index.ts:958-961`) as a thin wrapper, in the file's existing
`fn` / `fn _at` pair idiom, and route the loop's two delivery arms through it:

```rust
/// `sendIncomingBrokerMessage` (`v0.12.0 index.ts:958-961`) — [`send_incoming_message`] preceded by the
/// `toolVisibility` reveal. The BROKER delivery path only: an inbound peer message is first use, so the
/// tool is in the active set before `inject_message` runs and the turn that message drives already
/// carries it. Deliberately NOT used by [`trigger_turn_over_inbound`] — local in-process subagent relay
/// traffic must not reveal the generic tool (`intercom.integration.test.ts:1400-1405`).
pub fn send_incoming_broker_message(
    state: &SharedIntercomState,
    from: &SessionInfo,
    message: &Message,
    delivery: InboundDelivery,
) -> bool {
    state.reveal_intercom_tool();
    send_incoming_message(state, from, message, delivery)
}

/// [`send_incoming_broker_message`] with the caller's captured runtime generation (`:963`).
pub fn send_incoming_broker_message_at(
    state: &SharedIntercomState,
    from: &SessionInfo,
    message: &Message,
    delivery: InboundDelivery,
    generation: u64,
) -> bool {
    state.reveal_intercom_tool();
    send_incoming_message_at(state, from, message, delivery, generation)
}
```

In `spawn_inbound_loop`, `InboundPolicy::Deliver` (`inbound.rs:458`) calls
`send_incoming_broker_message_at`, and `InboundPolicy::Steer` (`:471`) calls
`send_incoming_broker_message`. `InboundPolicy::AutoReply` and `InboundPolicy::SurfaceOnly` are
**unchanged** — a busy non-interactive session that only bounces the busy notice back has not used
intercom, and upstream's `return` before `sendIncomingBrokerMessage` says so. `send_incoming_message`,
`send_incoming_message_at` and `trigger_turn_over_inbound` keep their current bodies.

### 3.7 Explicitly out of scope

- `contact_supervisor`: no reveal call, no hide, and its registration gate at `extension.rs:453-457` is
  untouched. Under `after-first-use` a child session still has `contact_supervisor` from turn one.
- `seams.rs:181` (`trigger_turn_over_inbound`, the local subagent relay) and `crate::relay`: no reveal.
- Broker reception, presence, receipts, the `/intercom` and `/intercom-id` commands, and the durable
  `intercom_message` entry surface all keep working while the tool is hidden — hiding removes the tool
  from the MODEL's surface, nothing else.

## 4. Definition of Done

Observable behavior, with a `config.json` at `<agent-dir>/intercom/config.json`:

1. No `toolVisibility` key, or `{"toolVisibility":"always"}` → the session behaves exactly as it does
   today: `intercom` is in the active tool set from the first turn, its schema and its prompt snippet
   are in every request, and no `Input`/`ToolResult` subscription is added.
2. `{"toolVisibility":"lazy"}` (or `7`, or `null`) → the session fails to start intercom with
   `Failed to load intercom config at <…>/intercom/config.json: "toolVisibility" must be "always" or "after-first-use"`.
   The value never silently becomes `"always"`.
3. `{"toolVisibility":"after-first-use"}` → from `SessionStart`, `intercom` is absent from the active
   tool set: absent from the provider request's tool array and its prompt snippet absent from the
   "Available tools" section of the system prompt.
4. In that same session, the tool becomes present — and stays present for the rest of the session —
   after ANY of: (a) an inbound broker message being injected (steer or trigger), with the tool already
   present on the turn that message drives; (b) an `intercom` tool call; (c) a delivered `/intercom`
   overlay send; (d) submitting `/skill:pi-intercom`; (e) a successful `read` of the bundled
   `resources/skills/pi-intercom/SKILL.md`.
5. Under `after-first-use`, the tool remains absent after: an errored `read` of that skill file, a `read`
   of any other file, a local in-process subagent relay message, a busy non-interactive auto-reply, and
   a `contact_supervisor` call.
6. A restart re-hides: a new session started with the same config has `intercom` absent again,
   regardless of what the previous session did. Nothing about the reveal is written to disk.
7. `contact_supervisor` is registered, present and callable under both `toolVisibility` values, on
   exactly the same child-orchestrator/native-channel conditions as today.
8. Every other config key (`brokerArgs`, `brokerCommand`, `confirmSend`, `enabled`, `inboundTrigger`,
   `replyHint`, `stableId`, `status`) parses and errors exactly as before.
