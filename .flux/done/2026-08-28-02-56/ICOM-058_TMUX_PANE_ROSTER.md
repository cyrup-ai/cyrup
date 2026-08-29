---
stage: qa
status: completed
updated: 2026-08-29 01:15
---

# Surface tmux pane ids in the session roster

> **Upstream parity gap.** `cyrup-intercom` is a port of `pi-intercom` **v0.9.2**; upstream is now
> **v0.12.0** (`ef95f19`, 2026-08-22) at [`nicobailon/pi-intercom`](https://github.com/nicobailon/pi-intercom).
> Reference checkout: [`./tmp/pi-intercom`](../../tmp/pi-intercom). Gap analysis:
> `docs/gap-analysis/11-cyrup-intercom.md` — **ICOM-058** (line 311).

## 0. Re-verified 2026-08-29 against `af1a63b`

Re-checked after ICOM-056 and ICOM-016 merged (PR #98). **The upstream research is perfect and needs
no correction; the cyrup-side line numbers had drifted badly, and the exhaustive-literal inventory in
Step 8 was missing a site.**

**Upstream, all exact — do not re-derive:**

| Claim | Verified |
| --- | --- |
| `tmuxPane?: string` on `SessionInfo` | `types.ts:42` |
| The SAME guard line twice | `broker/protocol.ts:168` and `:203` |
| `currentTmuxPane()` reading `process.env.TMUX_PANE?.trim()` | `index.ts:531-532` |
| The render fragment ``` `session.tmuxPane ? ` · tmux ${…}` : ""` ``` | `index.ts:551` |
| The registration spread `...(tmuxPane ? { tmuxPane } : {})` | `index.ts:891,900` |
| The broker's whitelist copy | `broker/broker.ts:475` |
| The overlay does NOT render it | `ui/session-list.ts` — **0** hits for `tmuxPane` |

**Cyrup-side citations corrected this pass:**

| Was | Now | Symbol |
| --- | --- | --- |
| `protocol.rs:237-300` | `protocol.rs:239-302` | `SessionInfo` |
| `protocol.rs:297-299` | `protocol.rs:299-301` | the `extra` capture |
| `protocol.rs:632-666` | `protocol.rs:673-707` | `SessionRegistration` |
| `identity.rs:62-75` | `identity.rs:67-80` | the env-inventory precedent |
| `connect.rs:562-582` | `connect.rs:581-601` | `build_registration` |
| `tools/intercom/mod.rs:210-258` | `:225-273` | `format_session_list_row` |
| `format_context.rs:68-81` | `:70-83` | `format_context_usage` |

**The Step 8 inventory was wrong, and would have broken the build.** It listed FIVE
`SessionRegistration` literals; there are **six**. The missing one is
`transport/protocol.rs:1022`, an exhaustive literal inside
`client_register_serializes_with_pi_field_names`. Step 8 below now lists all six, and every
`SessionInfo` line number is re-derived. **Re-derive both lists again at exec time** — this is the
third brief this session whose enumerated literal list had drifted, and it is the single most likely
cause of a failed build on a task of this shape.

**Still true, so the plan stands unchanged:**

- `SessionInfo` still ends `context_pct` → `context_tokens` → `context_window` → `extra`, so Step 1's
  insertion point (after `context_window`, before `extra`) is still upstream's declaration order.
- The `#[serde(flatten)] extra` capture is intact, so the interop claim in the Objective holds:
  `tmuxPane` round-trips today as an opaque key and this task promotes it to a modelled one.
- `list.rs:44,57` and `list_cwd.rs:73,82` both still call `format_session_list_row`, so one edit to
  one function still surfaces the pane in both actions.
- ICOM-056 and ICOM-016 added no field to `SessionInfo` and no new literal of either type; they
  touched `session_state.rs`, `transport/client.rs`, `seams.rs`, `extension.rs`, `inbound.rs` and
  `tools/intercom/*`, which is why so many line numbers moved while the SET did not.

---

## Objective

Port upstream `4af53db` (v0.11.0, "feat: surface tmux pane ids in roster") in full: model `tmuxPane`
on both wire types, read `$TMUX_PANE` at registration, copy it onto the broker's stored
`SessionInfo`, and render it inside the roster row that `intercom{action:"list"}` and
`intercom{action:"list-cwd"}` print — omitting it entirely when the session is not in a tmux pane.

**Interop is not at risk today.** `SessionInfo`'s `#[serde(flatten)] extra` capture
([`transport/protocol.rs:299-301`](../../crates/cyrup-intercom/src/transport/protocol.rs)) already
round-trips `tmuxPane` verbatim from a v0.12.0 peer through a cyrup broker. This task promotes it
from opaque passthrough to a **modelled, produced and rendered** field. After the change the key is
emitted explicitly (`#[serde(rename_all = "camelCase")]` → `tmuxPane`) instead of via `extra`, so the
relay stays byte-identical.

---

## 1. What upstream does

### 1.1 The type — [`tmp/pi-intercom/types.ts:36-42`](../../tmp/pi-intercom/types.ts)

```ts
  /** tmux pane id (e.g. "%212") of the session's terminal, read from
   *  $TMUX_PANE at registration. Present only when the session runs inside a
   *  tmux pane; absent for cloud, headless, IDE-embedded, or Herdr sessions.
   *  The pane id is immutable for the process lifetime — unlike the window
   *  name, which is mutable — so a peer can live-resolve the current window
   *  from it via tmux when it needs to introspect or drive that pane. */
  tmuxPane?: string;
```

It is declared **last** on `SessionInfo`, after the `contextPct`/`contextTokens`/`contextWindow`
trio. `SessionRegistration = Omit<SessionInfo, "id" | "endpointEpoch" | "peerUid" | "trustedLocal">`
([`types.ts:102-104`](../../tmp/pi-intercom/types.ts)), so the registration inherits it.

### 1.2 The guards — [`tmp/pi-intercom/broker/protocol.ts:168`](../../tmp/pi-intercom/broker/protocol.ts) and `:203`

```ts
  if (value.tmuxPane !== undefined && typeof value.tmuxPane !== "string") {
    return false;
  }
```

The **same** line appears twice: once in `isSessionInfo` (`:168`, just before the `trustedLocal`
return) and once in `isSessionRegistration` (`:203`, just before the `status` return). This is the
`[NON-NULL]` shape the port already has a helper for — `undefined` passes, a `string` passes, and an
explicit `null` **fails** (`typeof null === "object"`), i.e. `socket.destroy()`.

Note the asymmetry with `runtimeFallbackAlias`, which upstream does *not* guard on the registration:
`tmuxPane` **is** guarded on both, so the port must model it on both.

### 1.3 The producer — [`tmp/pi-intercom/index.ts:527-534`](../../tmp/pi-intercom/index.ts)

```ts
// The tmux pane id (e.g. "%212") the session was launched in. $TMUX_PANE is
// inherited at process start and immutable for the lifetime — moving the pane
// between windows keeps its id — so it is a stable join key a peer can use to
// live-resolve the current window via tmux. Absent outside tmux.
function currentTmuxPane(): string | undefined {
  const pane = process.env.TMUX_PANE?.trim();
  return pane ? pane : undefined;
}
```

Trimmed at the source, and a blank value is `undefined` — so a conforming peer never puts
whitespace-only on the wire. It is spread into the registration in `buildRegistration`
([`index.ts:886-908`](../../tmp/pi-intercom/index.ts)):

```ts
    const identity = buildPresenceIdentity(pi, currentIntercomSessionId ?? currentSessionId);
    const tmuxPane = currentTmuxPane();
    return {
      ...identity,
      cwd: liveContext.cwd,
      model: currentModel,
      pid: process.pid,
      startedAt: sessionStartedAt,
      lastActivity: Date.now(),
      status: currentStatus(),
      ...(tmuxPane ? { tmuxPane } : {}),
```

`buildRegistration` is re-run on every reconnect rung, so `currentTmuxPane()` is re-read every time —
harmless, because `$TMUX_PANE` is immutable for the process lifetime.

### 1.4 The broker copy — [`tmp/pi-intercom/broker/broker.ts:465-477`](../../tmp/pi-intercom/broker/broker.ts)

```ts
        const info: SessionInfo = {
          id,
          endpointEpoch: randomUUID(),
          ...
          ...(session.status !== undefined ? { status: session.status } : {}),
          ...(session.tmuxPane !== undefined ? { tmuxPane: session.tmuxPane } : {}),
          trustedLocal: typeof LISTEN_TARGET === "string" && process.platform !== "win32",
        };
```

The stored `SessionInfo` is a **whitelist** built from the registration; `tmuxPane` had to be added
to it explicitly. It is copied once, at register. Presence updates never carry it (`ClientMessage`'s
`presence` tag has no such field — [`types.ts:115`](../../tmp/pi-intercom/types.ts)), and the broker
mutates the stored `info` in place on presence, so the register-time value survives every update.

### 1.5 The render — [`tmp/pi-intercom/index.ts:546-553`](../../tmp/pi-intercom/index.ts)

```ts
function formatSessionListRow(session: SessionInfo, currentCwd: string, isSelf: boolean, idPrefix: string): string {
  const name = session.name || "Unnamed session";
  const tags = [isSelf ? "self" : session.cwd === currentCwd ? "same cwd" : undefined, session.status]
    .filter((tag): tag is string => Boolean(tag));
  const suffix = tags.length ? ` [${tags.join(", ")}]` : "";
  const pane = session.tmuxPane ? ` · tmux ${session.tmuxPane}` : "";
  return `• ${name} (${idPrefix}) — ${session.cwd} (${session.model}${formatContextUsage(session)}${pane})${suffix}`;
}
```

Three facts to port exactly:

1. The pane term sits **inside the model parentheses**, immediately **after** `formatContextUsage`.
2. It is a `" · tmux %212"` fragment with a leading `·` separator, or the **empty string** — the same
   empty-string-means-omitted contract `formatContextUsage` already uses. There is no column, no
   placeholder and no dangling `·`.
3. `session.tmuxPane ? …` is JS-falsy: an empty string renders nothing.

**Upstream's TUI overlay does not render it.**
[`tmp/pi-intercom/ui/session-list.ts:36-42`](../../tmp/pi-intercom/ui/session-list.ts) is unchanged
by `4af53db`:

```ts
function sessionTitle(session: SessionInfo, options?: { self?: boolean; sameCwd?: boolean }): string {
  const name = session.name || "Unnamed session";
  const tags = [options?.self ? "self" : undefined, options?.sameCwd ? "same cwd" : undefined]
    .filter((tag): tag is string => Boolean(tag));
  const suffix = tags.length ? ` [${tags.join(", ")}]` : "";
  return `${name} (${shortSessionId(session.id)})${suffix}`;
}
```

That is the **same asymmetry the port already records for context usage**
([`format_context.rs:9-10`](../../crates/cyrup-intercom/src/format_context.rs): "note upstream's own
`ui/session-list.ts` overlay does NOT render it, and neither does cyrup's"). The overlay stays
unchanged here for the same reason.

---

## 2. What already exists in the port and MUST be reused

Nothing new is to be invented. Every mechanism this change needs is already in the crate:

| Need | Existing thing to reuse | Location |
| --- | --- | --- |
| `x !== undefined && typeof x !== "string"` on the wire | `#[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]` — the `[NON-NULL]` idiom | [`transport/protocol.rs:102-118`](../../crates/cyrup-intercom/src/transport/protocol.rs) |
| Reading an env var as identity, with a pure testable core | `native_supervisor_channel_available_from(env: impl Fn(&str) -> Option<String>)` + a process-env wrapper; the module is "the crate's single env inventory" | [`identity.rs:67-80`](../../crates/cyrup-intercom/src/identity.rs) |
| The roster row `intercom{list}` prints | `format_session_list_row` — the 1:1 port of `formatSessionListRow`, already carrying the `idPrefix` 4th arg and the `tags`/`suffix` ladder | [`tools/intercom/mod.rs:225-273`](../../crates/cyrup-intercom/src/tools/intercom/mod.rs) |
| "absent optional renders the empty string, concatenated inline" | `format_context_usage` — returns `String::new()` when `context_pct` is `None`, so the row is byte-for-byte the pre-feature row | [`format_context.rs:70-83`](../../crates/cyrup-intercom/src/format_context.rs) |
| JS `||` falsy on a string (`""` falls through) | `.as_deref().filter(\|n\| !n.is_empty())` — used by `display_name` and by `session_title` | [`tools/intercom/mod.rs:221-223`](../../crates/cyrup-intercom/src/tools/intercom/mod.rs), [`ui/session_list.rs:22-33`](../../crates/cyrup-intercom/src/ui/session_list.rs) |
| Optional-tag suppression with no stray separator | `session_title`'s `let suffix = if tags.is_empty() { String::new() } else { … }` | [`ui/session_list.rs:22-33`](../../crates/cyrup-intercom/src/ui/session_list.rs) |
| The registration builder that runs on every reconnect | `build_registration` | [`connect.rs:581-601`](../../crates/cyrup-intercom/src/connect.rs) |
| The broker's stored-`SessionInfo` whitelist | `BrokerState::handle_register`'s `let info = SessionInfo { … }` | [`broker/session.rs:88-107`](../../crates/cyrup-intercom/src/broker/session.rs) |

**`intercom{list}` and `intercom{list-cwd}` need no edit at all.** Both already call
`format_session_list_row` ([`tools/intercom/list.rs:44,57`](../../crates/cyrup-intercom/src/tools/intercom/list.rs),
[`tools/intercom/list_cwd.rs:73,82`](../../crates/cyrup-intercom/src/tools/intercom/list_cwd.rs)),
so one edit to that one function surfaces the pane in both actions. Do not add a second renderer.

**Disambiguation.** `session_title` at
[`ui/session_list.rs:22`](../../crates/cyrup-intercom/src/ui/session_list.rs) is the *overlay* title
renderer (the port of `ui/session-list.ts` `sessionTitle`), not the `intercom{list}` roster row. The
roster row this task must change is `format_session_list_row`. `session_title` is reused here as the
**pattern** for optional-tag suppression, and is itself left untouched (§1.5).

---

## 3. Prescriptive implementation plan

### Step 1 — `SessionInfo.tmux_pane` — [`transport/protocol.rs`](../../crates/cyrup-intercom/src/transport/protocol.rs)

Add the field to `struct SessionInfo` (`:239-302`) **after `context_window` and before `extra`**,
matching upstream's declaration order:

```rust
    /// `tmuxPane` (`v0.12.0 types.ts:36-42`): the tmux pane id (e.g. `"%212"`) of the session's
    /// terminal, read from `$TMUX_PANE` at registration and copied onto the stored `SessionInfo`
    /// by the broker (`v0.12.0 broker/broker.ts:475`).
    ///
    /// Additive at v0.11.0 (`4af53db`). Present only when the session runs inside a tmux pane;
    /// absent for cloud, headless and IDE-embedded sessions. Upstream's own note is the reason it
    /// is worth relaying: the pane id is **immutable for the process lifetime** — unlike the
    /// window name — so a peer can live-resolve the current window from it via tmux.
    ///
    /// `[NON-NULL]` — `isSessionInfo` guards it with
    /// `value.tmuxPane !== undefined && typeof value.tmuxPane !== "string"`
    /// (`v0.12.0 broker/protocol.ts:168`), so an explicit `null` is fatal, exactly as for `status`.
    #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
    pub tmux_pane: Option<String>,
```

`#[serde(rename_all = "camelCase")]` on the struct already maps `tmux_pane` ↔ `tmuxPane`, and
`skip_serializing_if` keeps an absent pane off the wire — so the emitted frame is identical to the
one the `extra` capture used to reproduce.

### Step 2 — `SessionRegistration.tmux_pane` — same file

Add the same field to `struct SessionRegistration` (`:673-707`) **after `status`, before `extra`**:

```rust
    /// `tmuxPane` (`v0.12.0 types.ts:36-42`), carried on the registration by `buildRegistration`'s
    /// `...(tmuxPane ? { tmuxPane } : {})` spread (`v0.12.0 index.ts:900`) and copied onto the
    /// stored `SessionInfo` at `v0.12.0 broker/broker.ts:475`.
    ///
    /// `[NON-NULL]`, and unlike `runtimeFallbackAlias` this one **is** validated by
    /// `isSessionRegistration` upstream (`v0.12.0 broker/protocol.ts:203`) — so modelling it with
    /// `present_non_null` puts cyrup neither looser nor stricter than pi: a non-string is a
    /// destroyed connection on both sides.
    #[serde(default, deserialize_with = "present_non_null", skip_serializing_if = "Option::is_none")]
    pub tmux_pane: Option<String>,
```

### Step 3 — the environment read — [`identity.rs`](../../crates/cyrup-intercom/src/identity.rs)

`identity.rs` is the crate's single env inventory; the port of `currentTmuxPane` belongs there, next
to `native_supervisor_channel_available_from` (`:67-80`), in the same `*_from(env)` + wrapper shape.

**The variable name is `TMUX_PANE`, with no `CYRUP_` prefix.** The crate's prefix rule renames pi's
*own* variables (`PI_INTERCOM_*` → `CYRUP_INTERCOM_*`); `TMUX_PANE` is exported by **tmux** into
every pane's process environment. Renaming it would read a variable nothing ever sets, and the
feature would be silently dead. State this in the doc comment so a later prefix sweep does not
"fix" it.

```rust
/// `TMUX_PANE` — **not** `CYRUP_TMUX_PANE`. The `CYRUP_` prefix rule applies to pi's OWN variables
/// (`PI_INTERCOM_*`); this one is exported by **tmux** itself into every pane's environment, so
/// renaming it would read a variable nothing sets and the pane column would never populate.
pub const ENV_TMUX_PANE: &str = "TMUX_PANE";

/// `currentTmuxPane()` (`v0.12.0 index.ts:527-534`, 8 lines):
///
/// ```text
/// const pane = process.env.TMUX_PANE?.trim();
/// return pane ? pane : undefined;
/// ```
///
/// Upstream's own comment is the rationale: "$TMUX_PANE is inherited at process start and immutable
/// for the lifetime — moving the pane between windows keeps its id — so it is a stable join key a
/// peer can use to live-resolve the current window via tmux. Absent outside tmux."
///
/// Trimmed AND blank-rejected here, at the producer, so a cyrup session never puts a whitespace-only
/// pane id on the wire — which is what lets the renderer stay a plain presence check.
#[must_use]
pub fn current_tmux_pane_from(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    env(ENV_TMUX_PANE).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// [`current_tmux_pane_from`] over the process environment.
#[must_use]
pub fn current_tmux_pane() -> Option<String> {
    current_tmux_pane_from(|k| std::env::var(k).ok())
}
```

### Step 4 — populate the registration — [`connect.rs:581-601`](../../crates/cyrup-intercom/src/connect.rs)

In `build_registration`, add one field to the `SessionRegistration { … }` literal, after `status`:

```rust
        status: Some(state.current_status()),
        // `...(tmuxPane ? { tmuxPane } : {})` (`v0.12.0 index.ts:893,900`). Read from the LIVE
        // environment on every reconnect rung exactly as upstream re-runs `currentTmuxPane()`
        // inside `buildRegistration`; `$TMUX_PANE` is immutable for the process lifetime, so every
        // rung produces the same value and a re-register never changes the peer's pane column.
        tmux_pane: crate::identity::current_tmux_pane(),
        extra: Default::default(),
```

### Step 5 — copy it onto the stored `SessionInfo` — [`broker/session.rs:88-107`](../../crates/cyrup-intercom/src/broker/session.rs)

In `BrokerState::handle_register`'s `let info = SessionInfo { … }`, add after `status`:

```rust
            status: registration.status,
            // `...(session.tmuxPane !== undefined ? { tmuxPane: session.tmuxPane } : {})`
            // (`v0.12.0 broker/broker.ts:475`). The stored `SessionInfo` is a whitelist, so an
            // un-copied field would be dropped by a cyrup broker even though the registration
            // carried it — the roster is built from THIS value, not from the registration.
            tmux_pane: registration.tmux_pane,
            peer_uid: None,
```

`broker/presence.rs` needs **no** change: `ClientMessage::Presence` carries no `tmuxPane` upstream,
and `handle_presence` mutates the stored `session.info` field-by-field, so the register-time pane
survives every presence update. Record that as a deliberate no-op if a note is warranted.

### Step 6 — render it — [`tools/intercom/mod.rs:225-273`](../../crates/cyrup-intercom/src/tools/intercom/mod.rs)

Extend `format_session_list_row`, inline, exactly where upstream put it — the term goes **inside** the
model parentheses, **after** `format_context_usage`:

```rust
    let suffix = if tags.is_empty() { String::new() } else { format!(" [{}]", tags.join(", ")) };
    // `const pane = session.tmuxPane ? ` · tmux ${session.tmuxPane}` : ""` (`v0.12.0 index.ts:551`).
    // Same empty-string-means-omitted contract as `format_context_usage`: a session outside tmux
    // renders byte-for-byte the pre-v0.11.0 row — no column, no placeholder, no dangling `·`.
    //
    // Upstream's check is JS-falsy, so `""` already renders nothing. The `trim()` here is a
    // display-only strengthening: a conforming peer cannot send whitespace-only (the producer
    // trims — `identity::current_tmux_pane`), but a hostile one can, and ` · tmux    ` is exactly
    // the stray separator this must never print. It changes nothing on the wire.
    let pane = session
        .tmux_pane
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .map_or_else(String::new, |p| format!(" · tmux {p}"));
    format!(
        "• {} ({}) — {} ({}{}{}){}",
        name,
        id_prefix,
        session.cwd,
        session.model,
        format_context_usage(session),
        pane,
        suffix
    )
```

Update the function's doc comment to quote the v0.12.0 line and cite `v0.12.0 index.ts:546-553`.

### Step 7 — the child fixture — [`bin/cyrup_intercom_child_fixture.rs:100-115`](../../crates/cyrup-intercom/src/bin/cyrup_intercom_child_fixture.rs)

That binary registers as a genuine broker participant, so it reports its pane like any other
session — a hard-coded `None` would be a silent divergence:

```rust
        status: None,
        tmux_pane: cyrup_intercom::identity::current_tmux_pane(),
        extra: Default::default(),
```

### Step 8 — exhaustive struct literals

`SessionInfo` and `SessionRegistration` are plain structs with no `Default`, so **every** literal in
the crate must gain the new field for it to compile. The complete list:

`SessionRegistration { … }` — SIX sites, re-derived against this branch's base:
[`connect.rs:583`](../../crates/cyrup-intercom/src/connect.rs) (Step 4),
[`bin/cyrup_intercom_child_fixture.rs:100`](../../crates/cyrup-intercom/src/bin/cyrup_intercom_child_fixture.rs) (Step 7),
`session_state.rs:1305`, `transport/client.rs:985`, `tools/intercom/mod.rs:617`, and
**`transport/protocol.rs:1022`** — the last of these is NOT in the original list and is exhaustive; it
is the `client_register_serializes_with_pi_field_names` test, which asserts the emitted field names.
Adding `tmux_pane: None` there leaves its assertions unchanged, because `skip_serializing_if` keeps an
absent pane off the wire — which is itself the proof that Step 1's serde attributes are right.

`SessionInfo { … }` — SEVENTEEN sites:
[`broker/session.rs:88`](../../crates/cyrup-intercom/src/broker/session.rs) (Step 5),
[`seams.rs:152`](../../crates/cyrup-intercom/src/seams.rs) (the synthetic `subagent-result` relay
sender — `tmux_pane: None`, since a synthetic sender has no terminal; note it inline next to the
existing `runtime_fallback_alias: None` note), `seams.rs:375`, `seams.rs:419`, `ui/compose.rs:251`,
`ui/inline_message.rs:453`, `ui/session_list.rs:191`, `session_state.rs:1063`, `session_state.rs:1354`,
`project_target.rs:183`, `reply_tracker.rs:392`, `transport/client.rs:1372`, `transport/client.rs:1425`,
`tools/intercom/mod.rs:762`, `extension.rs:1215`, `extension.rs:1252`, `inbound.rs:686`.

All of those but the three named above take `tmux_pane: None`. Sites that build a `SessionInfo` via
`serde_json::from_value!` ([`format_context.rs:90`](../../crates/cyrup-intercom/src/format_context.rs))
or by struct-update from a helper (`tools/intercom/mod.rs:550,679-681`) need nothing.

### Step 9 — leave these alone, deliberately

- [`ui/session_list.rs`](../../crates/cyrup-intercom/src/ui/session_list.rs) — `session_title` and
  the overlay's `path_line`. Upstream's `sessionTitle` is untouched by `4af53db`, and the crate
  already documents this exact overlay/roster asymmetry for context usage
  ([`format_context.rs:9-10`](../../crates/cyrup-intercom/src/format_context.rs)). Adding a pane to
  the overlay would break `session_title`'s documented 1:1 correspondence with
  `session-list.ts:36-42` for a column upstream chose not to show there.
- [`format_context.rs`](../../crates/cyrup-intercom/src/format_context.rs) — the pane fragment is
  inlined in `format_session_list_row` upstream and stays inlined here. It is the *precedent* for
  the empty-string contract, not a home for the new code.
- [`tools/intercom/list.rs`](../../crates/cyrup-intercom/src/tools/intercom/list.rs) and
  [`list_cwd.rs`](../../crates/cyrup-intercom/src/tools/intercom/list_cwd.rs) — both already route
  through `format_session_list_row`; a second render path here would be duplication.

---

## Definition of Done

Observable behavior, end to end:

1. A `cyrup-intercom` session started inside a tmux pane sends `"tmuxPane": "%212"` (the trimmed
   `$TMUX_PANE` value) in its `register` frame's `session` object, on the first connect and on every
   reconnect rung.
2. A session started outside tmux, or with `TMUX_PANE` set to an empty or whitespace-only value,
   omits the key entirely — the frame carries no `tmuxPane` at all, not `null` and not `""`.
3. A cyrup broker that accepted such a registration reports the pane back in `sessions[]`,
   `session_joined`, `presence_update` and `message.from`, and it **keeps** reporting it after any
   number of presence updates.
4. `intercom({action:"list"})` renders a peer in a pane as
   `• worker (a1b2c3d4) — /w/proj (opus-5 · 72% ctx (144k/200k) · tmux %212) [same cwd, idle]` —
   the pane term inside the model parentheses, immediately after the context usage, separated by
   ` · `.
5. A peer with no pane renders `• worker (a1b2c3d4) — /w/proj (opus-5) [same cwd, idle]` — byte-for-byte
   the row printed before this change. No empty column, no placeholder, no trailing or doubled `·`.
   The same holds for a peer whose `tmuxPane` is present but whitespace-only.
6. `intercom({action:"list-cwd"})` shows the identical column, from the same single renderer.
7. A cyrup broker relaying between two pi v0.12.0 peers still round-trips `tmuxPane` unchanged; a
   frame carrying `"tmuxPane": null` or `"tmuxPane": 7` is a protocol error that closes the
   connection, on both `register` and any `SessionInfo`-bearing frame — matching
   `broker/protocol.ts:168,203`.
8. The `/intercom` overlay (`SessionListOverlay`) is visually unchanged.
9. `grep -rn "tmux" crates/cyrup-intercom/src` returns the new field, the env constant, the producer
   and the one render site — and nothing in `ui/session_list.rs`.
