---
stage: qa
status: completed
updated: 2026-08-30 15:00
---

# `/mcp` Command Surface — Wave 7: make the four inert arms live

The previous rounds built the `/mcp` **dispatcher**: the fenced prologue, the argument split, the
eight-way switch, the three listings, `disable`/`enable`, and the completion source. That work is
done and is not revisited here.

Four of the switch's arms still notify a placeholder and return. This pass replaces all four with
their real bodies.

```
crates/cyrup-mcp/src/commands.rs:503   arm_reconnect       -> "not available yet (MCP-386)."
crates/cyrup-mcp/src/commands.rs:513   arm_setup           -> "not available yet (MCP-387)."
crates/cyrup-mcp/src/commands.rs:520   arm_logout          -> "not available yet (MCP-388)."
crates/cyrup-mcp/src/commands.rs:530   arm_browser_panel   -> falls back to show_status (MCP-394)
```

---

## The finding that sets this pass's scope

**Every data-layer function these four arms need is already written, tested, and has no production
caller.** The port built the panel, the config writers, the discovery summary, the onboarding
store and the OAuth remove verb, and then stopped one layer short of the thing that calls them.
Forty-odd doc comments in the crate say so in the same words — *"No production caller yet … the
dispatcher that would call it is `TODO(MCP-394)`"*.

So this is **not** a porting task. It is a wiring task, and the single most important constraint is:

> **Call what exists. Do not re-derive a status ladder, a config writer, a connect sequence or a
> credential path that is already in the tree.** Every prescription below names the exact function
> to call. If a step below seems to need new logic, the function for it is almost certainly already
> written — find it before writing one.

What is genuinely missing is exactly **two** trait implementations, and everything else falls out of
them:

| Trait | Declared | Implementors today | Blocks |
|---|---|---|---|
| [`McpPanelCallbacks`](../../crates/cyrup-mcp/src/ui.rs) (`ui.rs:1433`) | ✅ | 3, all `#[cfg(test)]` (`ui.rs:5228`, `:5585`, `:5725`) | MCP-394, MCP-391 |
| [`SetupPanelCallbacks`](../../crates/cyrup-mcp/src/ui.rs) (`ui.rs:3455`) | ✅ | 1, `#[cfg(test)]` | MCP-387 |

### Dependency order — why all six tickets are one unit

```
MCP-386 reconnect ──┐
                    ├──> MCP-392 McpPanelCallbacks ──┬──> MCP-394 /mcp panel
MCP-387 setup ──────┘   (its `reconnect` member IS   └──> MCP-391 /mcp-auth picker
     │                   MCP-386's body)
     └──> MCP-394's zero-servers branch delegates to the setup panel

MCP-388 logout — independent, and the only one that can be built alone.
```

None of the six can be deferred without leaving a placeholder *inside* a live path. Build all six.

---

## 1 · MCP-386 — `/mcp reconnect [server]`

### Do not port `reconnectServer`'s body. It is already in the tree, twice over.

Upstream's `reconnectServer` (`commands.ts:150-221`) closes, connects, rebuilds tool metadata,
republishes prompt metadata, updates server instructions, flushes the metadata cache, fires the
metadata notification, marks keep-alive, clears the failure record and updates the status bar.

All ten steps are [`crate::proxy::execute_connect`](../../crates/cyrup-mcp/src/proxy/auth.rs)
(`proxy/auth.rs:284`), whose tail is labelled *"The eight-step commit, in order"* and which is the
same function `/mcp-auth`'s post-login reconnect already drives.

And the *reporting* half — the three outcome messages — is
[`McpExtension::reconnect_after_auth`](../../crates/cyrup-mcp/src/extension.rs)
(`extension.rs:1695`), which already produces upstream's exact literals:

* `MCP: Reconnected to {name} ({t} tools, {r} resources)`
* `MCP: {name} requires OAuth. Run /mcp-auth {name} first.`
* the `connect_failure_message` fallback.

**One divergence, and it is benign:** upstream closes unconditionally before connecting;
`execute_connect` calls `env.reconnect` only when the server is currently `Connected` and plain
`connect` otherwise (`proxy/auth.rs:303-307`). For a failed or idle server upstream's `close` is a
no-op, so the two sequences agree. Do not "fix" this.

### Required change: extract, then loop

`reconnect_after_auth` is `guard + body`. Split it so `/mcp reconnect` can reuse the body without
inheriting the auth-specific guard message.

In [`extension.rs`](../../crates/cyrup-mcp/src/extension.rs), split at the `proxy_ctx()` guard:

```rust
/// `commands.ts:169-221` — `reconnectServer`'s try/catch, as one connect-and-report step.
///
/// Extracted from [`Self::reconnect_after_auth`] so `/mcp reconnect` reuses it rather than
/// standing up a second copy of the eight-step commit. The commit itself is
/// [`crate::proxy::execute_connect`]; this function owns only the three outcome messages.
async fn reconnect_one(
    &self,
    ctx: &Arc<crate::proxy::ProxyCtx>,
    server_name: &str,
) -> AuthCommandOutcome {
    // ...verbatim from reconnect_after_auth's current body, below its `let Some(ctx)` guard.
}

async fn reconnect_after_auth(&self, server_name: &str) -> AuthCommandOutcome {
    let Some(ctx) = self.proxy_ctx() else {
        return AuthCommandOutcome::failed(
            format!(
                "OAuth credentials were stored for \"{server_name}\", but MCP is not initialized; the connection was not retried."
            ),
            cyrup_ext::NotifyKind::Warning,
        );
    };
    self.reconnect_one(&ctx, server_name).await
}
```

Then `arm_reconnect` in [`commands.rs`](../../crates/cyrup-mcp/src/commands.rs) becomes
`reconnectServers` (`commands.ts:224-242`):

```rust
async fn arm_reconnect(&self, state: &McpState, cmd: &CommandCtx, server: Option<&str>) {
    // `if (targetServer && !state.config.mcpServers[targetServer])` — the named-but-absent
    // refusal, BEFORE the loop, so `/mcp reconnect nope` says so once instead of iterating.
    if let Some(name) = server
        && !state.config.mcp_servers.contains_key(name)
    {
        cmd.notify(&format!("Server \"{name}\" not found in config"), NotifyKind::Error);
        return;
    }
    let Some(ctx) = self.proxy_ctx() else {
        cmd.notify("MCP is not initialized; nothing was reconnected.", NotifyKind::Warning);
        return;
    };
    // `targetServer ? [targetServer] : Object.keys(state.config.mcpServers)` — file order, which
    // is why `mcp_servers` is an `IndexMap`.
    let names: Vec<String> = match server {
        Some(name) => vec![name.to_string()],
        None => state.config.mcp_servers.keys().cloned().collect(),
    };
    for name in names {
        // `reconnectServer`'s own two pre-guards (`commands.ts:158-168`), which
        // `execute_connect` reports through a ToolResult rather than a notice.
        let Some(definition) = state.config.mcp_servers.get(&name) else { continue };
        if definition.is_disabled() {
            cmd.notify(
                &format!("MCP: {name} is disabled. Run /mcp enable {name}, then /reload."),
                NotifyKind::Warning,
            );
            continue;
        }
        // The owner fence is re-checked per server, not once: a `/reload` landing mid-loop must
        // stop the remaining reconnects.
        if !cmd.alive() {
            return;
        }
        let outcome = self.reconnect_one(&ctx, &name).await;
        cmd.notify(&outcome.message, outcome.kind);
    }
    crate::live::update_status_bar(state);
}
```

`arm_reconnect`'s signature loses both underscores; the `direct_tools_frozen` /
`sync_tool_surface` call after it in the switch (`commands.rs:376-378`) is already correct and does
not change.

---

## 2 · MCP-388 — `/mcp logout <server>`

`logoutServer` (`commands.ts:336-381`) is four steps, and step 1 is
[`crate::oauth::remove_auth`](../../crates/cyrup-mcp/src/oauth.rs) (`oauth.rs:3802`) — already
written, with its four interleaved abort checks, and explicitly waiting for this caller.

```rust
/// `logoutServer(serverName, state, ctx)` (`commands.ts:336-381`).
///
/// Both failure arms return **after notifying**, and their messages are not interchangeable: the
/// first means nothing was cleared, the second means the credentials ARE gone but a live
/// connection still holds the old token — the user needs to know which.
async fn arm_logout(&self, state: &Arc<McpState>, cmd: &CommandCtx, server: &str) {
    if !state.config.mcp_servers.contains_key(server) {
        cmd.notify(&format!("Server \"{server}\" not found in config"), NotifyKind::Error);
        return;
    }
    let cancel = cmd.owner.as_ref().map_or_else(cyrup_core::CancelToken::new, |o| o.token());
    // The GENERATION's vault — `state.auth_options` (`state.rs:325`) resolves it, and the comment
    // there explains why minting a second store breaks the login path. Do not build one here.
    let options = state.auth_options(&self.dirs, &cancel);
    if let Err(error) = crate::oauth::remove_auth(server, &options).await {
        // `if (isAbortError(error, signal)) throw error` — a cancellation is not a failure to
        // report; the user asked for it. The token is the SECOND argument and must be passed:
        // `is_abort_error(error, None)` recognises only errors that spell abort in their text.
        if crate::abort::is_abort_error(&error, Some(&cancel)) {
            return;
        }
        cmd.notify(
            &format!(
                "Failed to clear OAuth credentials for \"{server}\": {}",
                crate::ui::sanitize_terminal_text(&error.to_string())
            ),
            NotifyKind::Error,
        );
        return;
    }
    if !cmd.alive() {
        return;
    }
    if let Err(error) = state.manager.close(server).await {
        if crate::abort::is_abort_error(&error, Some(&cancel)) {
            return;
        }
        cmd.notify(
            &format!(
                "OAuth credentials were cleared for \"{server}\", but its connection could not be closed: {}",
                crate::ui::sanitize_terminal_text(&error.to_string())
            ),
            NotifyKind::Error,
        );
        return;
    }
    if !cmd.alive() {
        return;
    }
    crate::live::update_status_bar(state);
    cmd.notify(
        &format!(
            "OAuth credentials cleared for \"{server}\". Run /mcp-auth {server} to authenticate again."
        ),
        NotifyKind::Info,
    );
}
```

`state.manager.close` is `pub async fn close(self: &Arc<Self>, name: &str)`
(`server_manager.rs:2413`) — `state.manager` is already an `Arc`, so it calls directly.

The switch arm at `commands.rs:395-407` already computes `rest`, guards the empty case and fences;
only the `self.arm_logout(&state, &cmd, &rest).await` call site's first argument changes to
`&state` (an `&Arc<McpState>`).

---

## 3 · MCP-392 — the production `McpPanelCallbacks`

### Where it goes

A new module, **`crates/cyrup-mcp/src/panel_host.rs`**, declared in
[`lib.rs`](../../crates/cyrup-mcp/src/lib.rs) beside `mod commands;`. Both production callback
implementors live there. `commands.rs` is the dispatcher; this is the adapter the dispatcher hands
to the panel, and putting ~300 lines of it in `commands.rs` would bury the switch.

### The lifetime problem, and the pattern that already solves it in this crate

`McpPanelCallbacks::authenticate` and `::reconnect` return
`futures::future::BoxFuture<'static, …>`, so the struct cannot borrow. It needs:

* `Arc<McpState>` — cloned, trivial;
* `HostCtx` — **`#[derive(Clone)]`** (`cyrup-ext/src/native.rs:126`), so clone it;
* a handle back to `McpExtension`, for `authenticate_server` and `reconnect_one`.

The last one must be **`Weak`**, and the crate already has the seam:
`McpExtension::self_weak` (`extension.rs:150`), bound by `into_arc` (`:452`), which
[`install_surface_sync`](../../crates/cyrup-mcp/src/extension.rs) (`:707`) and
`install_runtime_env` (`:729`) both read with this exact idiom — including the debug line for a
unit test that holds the extension by value rather than through `into_arc`:

```rust
let Some(weak) = self.self_weak.get().cloned() else {
    tracing::debug!("MCP: no self handle bound; /mcp panel not opened");
    return false;
};
```

Add a small `pub(crate) fn self_handle(&self) -> Option<std::sync::Weak<McpExtension>>` next to
`config_context` (`extension.rs:526`) rather than reaching into the field from another module, and
have the three existing readers keep their current bodies — this is a new accessor, not a refactor
of them.

### The eight-rung status derivation

`getConnectionStatus` (`commands.ts:499-529`) is the only member with real logic, and the ladder's
order is load-bearing. Port it rung for rung:

```rust
fn connection_status(&self, server: &str) -> ConnectionStatus {
    // Rung 0: the per-open diagnostics map is CLEARED for this server first, so a store that
    // recovered between two repaints stops reporting yesterday's failure.
    self.auth_status_failures.lock().ok().map(|mut map| map.shift_remove(server));

    let Some(definition) = self.state.config.mcp_servers.get(server) else {
        // `isServerDisabled(undefined)` is falsy upstream, so an unknown name falls THROUGH to
        // the connection lookup rather than reporting `disabled`.
        return ConnectionStatus::Idle;
    };
    if definition.is_disabled() {
        return ConnectionStatus::Disabled;
    }
    let connection = self.state.manager.get_connection(server);

    // `resolveServerUrl` CAN THROW, and a throw is `failed` — not `idle`, and not a panic.
    let server_url = match crate::credentials::resolve_server_url(
        definition.url.as_deref(),
        &crate::credentials::process_env(),
    ) {
        Ok(url) => url,
        Err(_) => return ConnectionStatus::Failed,
    };

    // The four-condition OAuth guard, all four required:
    //   auth === "oauth" && serverUrl && oauth !== false && oauth?.grantType !== "client_credentials"
    // `client_credentials` is excluded because it needs no stored user token — reporting
    // `needs-auth` for one would send the user to a browser flow it does not use.
    if let Some(url) = server_url.as_deref()
        && definition.uses_oauth_authorization_code()
    {
        match auth_store.inspect_auth_for_url(server, url) {
            // "unavailable" = the credential STORE could not be read. It is a diagnostic, not a
            // connection failure: it is recorded here and surfaced through `failure_message`,
            // and it deliberately does NOT touch `state.failure_messages` — a status inspection
            // must not poison the 60-second connect backoff.
            Ok(OAuthCredentialStatus::Unavailable { message }) => {
                if let Ok(mut map) = self.auth_status_failures.lock() {
                    map.insert(server.to_string(), message);
                }
                return ConnectionStatus::Failed;
            }
            Ok(OAuthCredentialStatus::Absent) => return ConnectionStatus::NeedsAuth,
            // `!authStatus.entry.tokens`. `AuthEntry` has no `tokens` field — the tokens are
            // PROJECTED out of `credentials` by `crate::oauth::project_tokens` (`oauth.rs:1671`),
            // which is the same projection `get_auth_status` (`oauth.rs:3775`) uses. Reading
            // `credentials.is_none()` instead would call a half-written entry authenticated.
            Ok(OAuthCredentialStatus::Present(entry))
                if entry.credentials.as_ref().and_then(crate::oauth::project_tokens).is_none() =>
            {
                return ConnectionStatus::NeedsAuth;
            }
            // A store error that is NOT "unavailable" has no upstream arm — `inspectAuthForUrl`
            // throws and the caller has no catch. Record it like the unavailable case rather than
            // discarding it: the panel showing a reason beats a silent `idle`.
            Err(error) => {
                if let Ok(mut map) = self.auth_status_failures.lock() {
                    map.insert(server.to_string(), error.to_string());
                }
                return ConnectionStatus::Failed;
            }
            Ok(OAuthCredentialStatus::Present(_)) => {}
        }
    }

    // Only now the live connection, then the failure window, then idle.
    match connection.as_ref().map(|c| c.status()) {
        Some(crate::lifecycle::ConnectionStatus::NeedsAuth) => ConnectionStatus::NeedsAuth,
        Some(crate::lifecycle::ConnectionStatus::Connected) => ConnectionStatus::Connected,
        _ if crate::live::failure_age_seconds(&self.state, server).is_some() => {
            ConnectionStatus::Failed
        }
        _ => ConnectionStatus::Idle,
    }
}
```

Three notes that are easy to get wrong:

1. **`ConnectionStatus` is ambiguous in this crate.** `commands.rs:83` already carries the warning:
   three types share the name — `crate::lifecycle`'s three-variant one, `crate::proxy::env`'s, and
   `crate::ui`'s six-variant panel view. This trait returns **`crate::ui::ConnectionStatus`**.
   Importing the wrong one compiles and lies.
2. **`supports_oauth` is not this predicate and must not be substituted for it.**
   `supports_oauth` (`oauth.rs:350`) answers "could this server ever do OAuth", and its last line
   is `definition.auth.is_none()` — a header-less URL server with no `auth` key says **true**. The
   panel's guard is the strict one: `auth` is *explicitly* `oauth`. No predicate spells it today,
   so add one to `ServerEntry` beside `is_disabled` (`config.rs:872`), named for what it means:

   ```rust
   /// `definition.auth === "oauth" && definition.oauth !== false
   ///  && definition.oauth?.grantType !== "client_credentials"` (`commands.ts:511-516`).
   ///
   /// Strictly narrower than [`crate::oauth::supports_oauth`], which also answers `true` for a
   /// URL server that declares no `auth` at all. Only a server matching THIS predicate has a
   /// stored user token worth inspecting.
   #[must_use]
   pub fn uses_oauth_authorization_code(&self) -> bool {
       self.auth == Some(AuthMode::Named(AuthKind::Oauth))
           && !matches!(self.oauth, Some(OAuthSetting::Disabled(false)))
           && !matches!(
               &self.oauth,
               Some(OAuthSetting::Config(config))
                   if config.grant_type == Some(OAuthGrantType::ClientCredentials)
           )
   }
   ```

   The resolved-URL condition stays at the call site, because the URL is already in hand there and
   resolving it twice would re-run a resolver that can fail.
3. **`inspect_auth_for_url`** is `credentials.rs:2787`, a method on `McpAuthStore` returning
   `Result<OAuthCredentialStatus, AuthStoreError>` — **not** a bare enum, so all four arms above are
   required. The `auth_store` binding is resolved once per `connection_status` call through
   `state.manager.auth_store()`, with the same `unwrap_or_else` reconstruction
   [`McpState::auth_options`](../../crates/cyrup-mcp/src/state.rs) (`state.rs:325`) uses — read
   that function's comment first; it explains why minting a second store breaks the login path.

### The remaining five members

```rust
fn failure_message(&self, server: &str) -> Option<String> {
    // The panel-only diagnostic WINS over the connect-failure text: it is the more specific of
    // the two and the reason the status is `failed` at all.
    self.auth_status_failures
        .lock().ok().and_then(|map| map.get(server).cloned())
        .or_else(|| crate::live::failure_message(&self.state, server))   // live.rs:214
}

fn can_authenticate(&self, server: &str) -> bool {
    self.state.config.mcp_servers.get(server)
        .is_some_and(|d| !d.is_disabled() && crate::oauth::supports_oauth(d))  // oauth.rs:350
}

fn refresh_cache_after_reconnect(&self, server: &str) -> Option<ServerCacheEntry> {
    // Re-reads the WHOLE cache file every call, deliberately: that is how the panel observes what
    // `update_metadata_cache` just flushed. Do not cache it — the trait's doc says so.
    crate::registration::load_metadata_cache(&self.dirs)?      // registration.rs:993
        .servers                                              // IndexMap<String, ServerCacheEntry>
        .get(server)
        .cloned()
}

fn authenticate(&self, server: String) -> BoxFuture<'static, Result<McpAuthResult, String>> {
    let (weak, state, ctx) = (self.ext.clone(), Arc::clone(&self.state), self.ctx.clone());
    Box::pin(async move {
        let Some(ext) = weak.upgrade() else {
            return Err("MCP: the extension was dropped while the panel was open.".to_string());
        };
        // `authenticate_server` (extension.rs:1532) already owns every guard, message and level.
        let outcome = ext.authenticate_server(&state, &server, &ctx).await;
        // `AuthCommandOutcome` -> `McpAuthResult`: the panel reads `ok` and `message` only, and an
        // EMPTY message is the abort arm's "say nothing", which must map to `None` rather than
        // to a blank line in the panel's notice row.
        Ok(McpAuthResult {
            ok: outcome.ok,
            message: (!outcome.message.is_empty()).then(|| outcome.message.clone()),
        })
    })
}

fn reconnect(&self, server: String) -> BoxFuture<'static, Result<bool, String>> {
    let weak = self.ext.clone();
    Box::pin(async move {
        let Some(ext) = weak.upgrade() else { return Ok(false) };
        let Some(ctx) = ext.proxy_ctx() else { return Ok(false) };   // extension.rs:753
        Ok(ext.reconnect_one(&ctx, &server).await.ok)                 // §1
    })
}
```

`authenticate_server`, `reconnect_one` and `proxy_ctx` must be visible from `panel_host.rs` —
widen the first two to `pub(crate)`, exactly as `command_services`, `await_committed_state` and
`mode_str` were widened for `commands.rs`.

`auth_status_failures` is `Mutex<IndexMap<String, String>>` — **per open**, constructed with the
struct, never shared between two panel openings. Upstream's is a `Map` created inside
`buildMcpPanelCallbacks`, which runs once per open, and that scoping is the whole reason it is safe
to keep diagnostics out of `state.failure_messages`.

---

## 4 · MCP-387 — `/mcp setup`

### The ten callbacks are ten existing functions

`openMcpSetup`'s callback object (`commands.ts:440-478`) maps one-to-one onto functions that are
already written and, in eight of ten cases, carry a *"No production caller yet"* note naming this
task:

| `SetupPanelCallbacks` | Existing implementation |
|---|---|
| `preview_imports` | `ConfigContext::preview_compatibility_imports` — `config.rs:3696` |
| `preview_starter_project` | build via `build_starter_project_config` — `config.rs:3316` |
| `preview_repo_prompt` | `preview_shared_server_entry` — `config.rs:3332` |
| `preview_known_server` | `preview_shared_server_entry` at `ConfigContext::project_path()` — `config.rs:2767` |
| `adopt_imports` | `ConfigContext::ensure_compatibility_imports` — `config.rs:3717` |
| `scaffold_project_config` | `ConfigContext::write_starter_project_config` — `config.rs:3800` |
| `add_repo_prompt` | `write_shared_server_entry` — `config.rs:3352` |
| `add_known_server` | `write_shared_server_entry` — `config.rs:3352` |
| `open_path` | `crate::ui::open_path` — `ui.rs:3119` |
| `mark_setup_completed` | `crate::onboarding::mark_setup_completed` — `onboarding.rs:154` |

Three shape rules:

* **`config_changed` is an `AtomicBool` field on the struct**, not a return value. Upstream closes
  over a `let configChanged` and mutates it from inside the callbacks; the Rust equivalent is a
  field the caller reads after `open_mcp_setup_panel` returns. `&self` methods make it an atomic,
  not a `bool`.
* **`adopt_imports` sets it only when `result.added` is non-empty** — an all-already-present adopt
  wrote nothing and must not trigger a `/reload`. `CompatibilityImportsResult.added`
  (`config.rs:3310`) documents exactly that. The other three writers set it unconditionally.
* **`preview_repo_prompt` and `add_repo_prompt` re-run the discovery summary on every call**
  (upstream calls `getMcpDiscoverySummary(...)` inside both). Keep that: the target path is
  recomputed from disk, so a `.mcp.json` created since the panel opened is honoured. Both return
  the "not available" arm when any of `entry` / `target_path` / `server_name` on
  [`RepoPromptDiscovery`](../../crates/cyrup-mcp/src/config.rs) (`config.rs:3970`) is `None`;
  `add_repo_prompt`'s is an `Err`, `preview_repo_prompt`'s is `None`.

Hold `Weak<McpExtension>` and call `ext.config_context()` (`extension.rs:526`) per call rather than
storing a `ConfigContext` — `config_context()` re-resolves the `--mcp-config` argv override each
time, which is upstream's `pi.getFlag("mcp-config")` and must not be frozen at open.

### The arm

```rust
/// `openMcpSetup(state, pi, ctx, configOverridePath, mode, options)` (`commands.ts:406-481`).
///
/// Returns whether anything was written, which is what decides the `cmd.reload()` in the switch.
async fn arm_setup(&self, state: &McpState, cmd: &CommandCtx) -> bool {
    // `canRenderPanel(ctx)` — already spelled once, on CommandCtx (`commands.rs:77`).
    // The programmatic-config refusal is upstream's THIRD guard but the switch already raised it
    // at `commands.rs:381-386`, so it is not repeated here.
    if !cmd.can_render_panel() {
        cmd.notify(&crate::ui::panel_unavailable_message(mode_str(cmd.mode)), NotifyKind::Info);
        return false;
    }
    let (Some(ui), Some(weak)) = (cmd.ui.as_ref(), self.self_handle()) else { return false };

    let mut diagnostics = Vec::new();
    // `mode === "setup"` passes no `includeHostConfigs`, so it defaults ON; the "empty" delegation
    // from §5 passes `false`. Both go through this one function.
    let discovery = self.config_context().mcp_discovery_summary(true, &mut diagnostics);
    let onboarding = crate::onboarding::load_onboarding_state(&self.dirs.onboarding_state());

    let callbacks = Arc::new(crate::panel_host::SetupCallbacks::new(
        weak, self.dirs.clone(), discovery.fingerprint.clone(), true,
    ));
    let model = crate::ui::McpSetupPanelModel::new(
        discovery,
        onboarding,
        Arc::clone(&callbacks) as Arc<dyn crate::ui::SetupPanelCallbacks>,
        crate::ui::SetupScreen::Setup,
        crate::ui::PanelKeys::from_agent_dir(self.dirs.agent_dir()),   // ui.rs:1102
    );
    // `false` = no host took the overlay. Not an error — the same signal `open_mcp_panel`'s
    // `None` carries, and the caller's cue to fall back.
    if !crate::ui::open_mcp_setup_panel(ui.as_ref(), model, callbacks.clone(), Handle::current()) {
        cmd.notify(&crate::ui::panel_unavailable_message(mode_str(cmd.mode)), NotifyKind::Info);
        return false;
    }
    callbacks.config_changed()
}
```

`state` becomes unused in `arm_setup`; drop the parameter and its argument at the call site rather
than keeping a leading underscore.

`crate::ui::SetupScreen` and `McpSetupPanelModel` are already exported from `lib.rs:207`.

---

## 5 · MCP-394 — `/mcp` and `/mcp status` open the browser panel

`openMcpPanel` (`commands.ts:539-603`), with its guards in upstream's order. Guard 1
(programmatic config) is already at the call site (`commands.rs:381-393`); guard 2 is what
`arm_browser_panel` does **today**, so it survives unchanged as the fallback branch.

```rust
/// `openMcpPanel(state, pi, ctx, configOverridePath, onDirectToolsConfigChanged)`
/// (`commands.ts:539-603`). Returns whether the config changed.
///
/// Takes `&Arc<McpState>` and the raw `&HostCtx`, not `&McpState` and `&CommandCtx` alone:
/// `McpPanelCallbacks`' two async members return `BoxFuture<'static, _>`, so the callbacks
/// struct must OWN a state handle and a `HostCtx` (`#[derive(Clone)]`,
/// `cyrup-ext/src/native.rs:126`). `CommandCtx` deliberately carries neither — it holds the
/// snapshotted `has_ui`/`mode`/`cwd` and the fenced services handle, which is the right shape for
/// every other arm. Widen this one signature; do not add fields to `CommandCtx` for it.
async fn arm_browser_panel(
    &self,
    state: &Arc<McpState>,
    cmd: &CommandCtx,
    ctx: &HostCtx,
) -> bool {
    // GUARD 2 — a UI with no terminal overlay. Upstream re-renders showStatus as TEXT here
    // (`commands.ts:557`) rather than refusing, because every fact the panel shows is available
    // as a listing. This is the branch this function already implemented.
    if !cmd.can_render_panel() {
        cmd.notify_multiline(show_status(state, cmd.has_ui), NotifyKind::Info);
        return false;
    }
    // GUARD 3 — nothing configured yet. `/mcp` on a fresh machine is a SETUP prompt, not an empty
    // table, and it opens on the Empty screen with host-config import discovery OFF.
    if state.config.mcp_servers.is_empty() {
        return self.arm_setup_empty(cmd).await;
    }
    let (Some(ui), Some(weak)) = (cmd.ui.as_ref(), self.self_handle()) else { return false };

    let mut diagnostics = Vec::new();
    let config_ctx = self.config_context();
    let provenance = config_ctx.server_provenance(&mut diagnostics);          // config.rs:3602
    let cache = crate::registration::load_metadata_cache(&self.dirs);         // registration.rs:993

    // `buildSharedConfigNoticeLines` (`commands.ts:388-405`) — two lines, shown ONCE ever, and
    // only when a shared source actually declares servers.
    let summary = config_ctx.mcp_standard_config_summary(&mut diagnostics);   // config.rs:4164
    let onboarding = crate::onboarding::load_onboarding_state(&self.dirs.onboarding_state());
    let (notice_lines, fingerprint) = shared_config_notice(&summary, &onboarding);

    let callbacks: Arc<dyn crate::ui::McpPanelCallbacks> =
        Arc::new(crate::panel_host::PanelCallbacks::new(
            weak, Arc::clone(state), ctx.clone(), self.dirs.clone(),
        ));
    let model = crate::ui::McpPanelModel::new(
        &state.config,
        cache,
        &provenance,
        Arc::clone(&callbacks),
        crate::ui::PanelOptions {
            notice_lines,
            auth_only: false,
            keys: crate::ui::PanelKeys::from_agent_dir(self.dirs.agent_dir()),
            // `None` is `default_server_hasher` — the real digest, resolvers and all. Injecting
            // one here would be a second spelling of the same hash (MCP-141, MCP-145).
            server_hash: None,
        },
    );
    let Some(result) = crate::ui::open_mcp_panel(ui.as_ref(), model, callbacks, Handle::current())
    else {
        // `None` is pi's `!ctx.hasUI` branch, NOT an error — fall back to the listing.
        cmd.notify_multiline(show_status(state, cmd.has_ui), NotifyKind::Info);
        return false;
    };

    // The hint is stamped as shown once the panel has CLOSED, and only if it was actually
    // rendered — `markSharedConfigHintShown(fingerprint)` at `commands.ts:600`.
    if let Some(fingerprint) = fingerprint {
        let _ = crate::onboarding::mark_shared_config_hint_shown(
            &self.dirs.onboarding_state(), &fingerprint,                      // onboarding.rs:140
        );
    }

    let changes = result.to_config_changes();                                 // ui.rs:1337
    if result.cancelled || changes.is_empty() {
        return false;
    }
    match crate::config::write_direct_tools_config(&changes, &provenance, &state.config) {
        Ok(()) => {
            // `onDirectToolsConfigChanged?.(changes)` — see the note below on why this is ONE
            // call and not two.
            self.sync_tool_surface();
            cmd.notify("Direct tools updated for this session.", NotifyKind::Info);
            // NOT `true`. See "the arm that looks like a bug" below.
            false
        }
        Err(error) => {
            cmd.notify(
                &format!("Direct tools updated, but live refresh failed: {error}"),
                NotifyKind::Error,
            );
            true
        }
    }
}
```

### Three things here that will be got wrong without reading this

**(a) `applyDirectToolConfigChanges` has no counterpart, and needs none.**
Upstream's write-back is two calls — `applyDirectToolConfigChanges(changes)`, which mutates
`state.config.mcpServers[name].directTools` in memory, then `syncToolSurface(ctx)`, which resolves
the surface from `state?.config ?? earlyConfig`. In cyrup, `McpState::config` is a plain
`McpConfig` behind an `Arc` (`state.rs:103`) and is **immutable by construction** — and it does not
need to be mutable, because
[`McpExtension::sync_tool_surface`](../../crates/cyrup-mcp/src/extension.rs) (`extension.rs:214`)
**re-reads config and cache from disk** rather than reusing the in-memory copy. Its own comment at
`:241` says why: *"Reusing the captured early config would resolve the surface the session started
with, which is the bug this method exists to fix."* Since `write_direct_tools_config` has already
flushed the changes to disk on the line above, the disk re-read observes exactly what the in-memory
mutation would have produced. **Do not add interior mutability to `McpState::config` to port a step
this design already made unnecessary.**

The one residue, and it is upstream's too in the half that matters: `write_direct_tools_config`
skips a server with no provenance entry (`config.rs:3385`, `let Some(prov) … else { continue }`),
matching `config.ts:1189`. Upstream's in-memory arm would still apply that server's change for the
rest of the session; cyrup's disk re-read cannot. Every configured server gets a provenance entry
from `server_provenance`, so the divergence is unreachable from the panel — record it as a
`CYRUP-DELTA` comment on the call and move on. Do not build a mechanism for it.

**(b) The arm that looks like a bug is upstream's, and is load-bearing.**
`configChanged` is initialised `false`, and the **success** path never sets it — only the `.catch`
does (`commands.ts:594`). So a successful direct-tools edit does **not** trigger `commandReload()`,
and a *failed* live refresh does. That is deliberate: the success path has already re-synced the
surface in-process, so a reload would be a redundant full restart; the failure path could not, so a
reload is the recovery. Returning `true` on success here would restart the session on every panel
edit. The `TODO(MCP-394)` at `ui.rs:4978` flags this exact arm. Port it as written.

**(c) `open_mcp_panel` blocks, and that is the established shape.**
`HostServices::open_overlay` blocks the extension's task until the overlay closes
(`ui.rs:5001`'s doc). Two shipped production callers already do this from inside an async
`execute_command`: `PermissionSystemSettingsOverlay`
(`cyrup-permission-system/src/extension/command.rs:207`) and `LiveHostServices::custom`
(`cyrup-session-svc/src/host_services.rs:1054`). Follow them exactly — pass
`tokio::runtime::Handle::current()`, which the overlay uses only for `handle.spawn`
(`ui.rs:3223`, `:4670`), never `block_on`. Do **not** introduce `spawn_blocking` or any second
concurrency model for this call.

One consequence to accept rather than fix: upstream notifies *"Direct tools updated for this
session."* from inside the panel's `done` callback, while the overlay is still on screen; here it
lands after the overlay tears down. `open_settings_overlay` records the same shape difference for
the same reason and it is the right trade.

### The call site

The `_` arm of the switch (`commands.rs:407-433`) passes the extra argument:
`self.arm_browser_panel(&state, &cmd, ctx).await`. `state` is already the `Arc<McpState>`
`command_prologue` returned and `ctx` is `on_mcp_command`'s own parameter, so nothing new has to be
threaded through the dispatcher.

### `arm_setup_empty` and the notice helper

The zero-servers delegation is `openMcpSetup(..., "empty", { includeHostConfigs: false })` — the
same body as §4 with two arguments changed. Give `arm_setup` those two as parameters
(`screen: SetupScreen`, `include_host_configs: bool`) and let `arm_setup_empty` be the one-line
caller, rather than a second copy.

`shared_config_notice` is `buildSharedConfigNoticeLines`, in `panel_host.rs`:

```rust
/// `buildSharedConfigNoticeLines` (`commands.ts:388-405`). Empty lines AND a `None` fingerprint
/// mean "say nothing and stamp nothing" — the two travel together, which is why this returns a
/// pair rather than two independent values.
fn shared_config_notice(
    summary: &McpStandardConfigSummary,
    onboarding: &OnboardingState,
) -> (Vec<String>, Option<String>) {
    if !summary.has_shared_servers || onboarding.shared_config_hint_shown {
        return (Vec::new(), None);
    }
    let sources = summary.sources.iter()
        .filter(|s| s.kind == DiscoveryKind::Shared && s.server_count > 0)
        .map(|s| s.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    (
        vec![
            format!("Using standard MCP config from {sources}."),
            "Cyrup only writes compatibility imports and adapter-specific overrides into \
             Cyrup-owned files when needed.".to_string(),
        ],
        Some(summary.fingerprint.clone()),
    )
}
```

The second line is upstream's with `Pi` → `Cyrup`; it names the product to the user, and the crate
already re-words upstream's user-facing product references. Mark it `CYRUP-DELTA`.

---

## 6 · MCP-391 — `/mcp-auth` with no argument opens the picker panel

[`pick_oauth_server`](../../crates/cyrup-mcp/src/extension.rs) (`extension.rs:1458`) currently
runs a `select` dialog. Its doc block says the swap *"is blocked on **MCP-392**, not on
preference"* — §3 removes that block, so make the swap.

Everything before the final gesture already matches `openMcpAuthPanel` (`commands.ts:605-652`) —
the programmatic-config refusal, the OAuth-capable filter, the zero-candidates warning and the
`auth_panel_unavailable_message` arm. **Change only the gesture**, keeping all four guards where
they are:

```rust
// Replaces the `state.dialog()` + `dialog.select(...)` pair.
let Some(ui) = ctx.has_ui.then(|| self.command_services(ctx, self.owner().as_ref())).flatten()
else {
    return Picked::Refused(AuthCommandOutcome::failed(
        crate::ui::auth_panel_unavailable_message(mode_str(ctx.mode)),
        cyrup_ext::NotifyKind::Info,
    ));
};
// ... build cache / provenance / callbacks exactly as §5 does, then:
let model = crate::ui::McpPanelModel::new(
    &state.config, cache, &provenance, Arc::clone(&callbacks),
    crate::ui::PanelOptions {
        // The panel's OWN notice — it names the keystrokes the panel actually has, which is why
        // `AUTH_PICKER_PROMPT` (a select-dialog question) does not carry over.
        notice_lines: vec![crate::ui::AUTH_PANEL_NOTICE.to_string()],   // ui.rs:4937
        auth_only: true,                                                // ui.rs:1478
        keys: crate::ui::PanelKeys::from_agent_dir(self.dirs.agent_dir()),
        server_hash: None,
    },
);
```

Under `auth_only` the panel performs the authentication itself, through
`McpPanelCallbacks::authenticate` — it does not hand a name back for the caller to authenticate.
So the result maps to **`Picked::Dismissed`** whether the user authenticated or closed: the work is
already done and the caller must not run a second flow. `openMcpAuthPanel` returns
`{ configChanged: false }` unconditionally for the same reason.

`AUTH_PICKER_PROMPT` (`extension.rs:1774`) loses its only reader — **delete the constant** rather
than leaving it dead, and delete `state.dialog()`'s use here if this was its last one (check first;
`McpDialog` has other callers from MCP-471's dialog arms). `AUTH_PANEL_NOTICE`'s doc, which says it
has no production reader, gains one — update it.

---

## 7 · The stale-note sweep

Roughly forty doc comments assert *"No production caller yet … `TODO(MCP-394)`"*. After this pass
most are false, and a false note is worse than none: the next reader deletes live code on its word.
Correct them in the same pass — this is not optional cleanup, it is part of the change.

```bash
rg -n 'TODO\(MCP-39[124]\)|No production (caller|implementor|consumer|reader)|not ported' crates/cyrup-mcp/src/
```

Each hit is one of three cases: **now called** (rewrite the note to name the caller), **still
uncalled** (leave it, and it should now be a very short list), or **already stale before this pass**
— of which at least one is known:

* `config.rs:3438` `write_project_server_disabled_override` carries the boilerplate note, but
  `arm_set_disabled` (`commands.rs:446`) has called it since MCP-389. Fix it.

Also revisit, because they name this dispatcher as the blocker:
`lifecycle.rs:611` (`unregister_server` — still uncalled; the config-change path it belongs to is
not part of this task, so keep its note but drop the "not ported" clause about `/mcp`),
`oauth.rs:3768` (`get_auth_status` — still uncalled; `remove_auth` beside it is now live, so the
shared note must be split), `onboarding.rs:21`, `state.rs:246`, `state.rs:518`, `ui.rs:993`,
`ui.rs:1099`, `ui.rs:1333`, `ui.rs:1416`, `ui.rs:1986`-`2025`, `ui.rs:3421`-`3455`,
`ui.rs:3713`-`3737`, `ui.rs:4878`, `ui.rs:4940`, `ui.rs:4955`, `ui.rs:4978`, `ui.rs:4997`,
`ui.rs:5023`, `config.rs:1734`, `config.rs:2717`, `config.rs:2741`, `config.rs:3970`.

---

## Definition of done

1. `/mcp reconnect` and `/mcp reconnect <server>` connect through
   [`crate::proxy::execute_connect`](../../crates/cyrup-mcp/src/proxy/auth.rs) and report
   upstream's three outcome messages; `reconnect_after_auth` and the new `/mcp reconnect` path
   share **one** body, and the eight-step commit is not duplicated anywhere.
2. `/mcp logout <server>` calls [`crate::oauth::remove_auth`](../../crates/cyrup-mcp/src/oauth.rs),
   then closes the connection, and its two failure arms carry the two distinct messages.
3. `crates/cyrup-mcp/src/panel_host.rs` exists and holds production implementors of **both**
   `McpPanelCallbacks` and `SetupPanelCallbacks`; `connection_status` reproduces the eight rungs in
   order, including the `resolve_server_url`-throws arm and the four-condition OAuth guard, and its
   `authStatusFailures` map is per-open and never writes to `state.failure_messages`.
4. `/mcp setup` opens the real setup panel and returns whether anything was written, with
   `adopt_imports` setting that flag only for a non-empty `added`.
5. `/mcp` and `/mcp status` open the browser panel; zero configured servers delegate to the setup
   panel's Empty screen with `include_host_configs: false`; the direct-tools write-back calls
   `write_direct_tools_config` then `sync_tool_surface`, and returns `true` **only** from the
   failure arm.
6. `/mcp-auth` with no argument opens the `auth_only` panel; `AUTH_PICKER_PROMPT` is deleted
   rather than left dead.
7. No new interior mutability on `McpState::config`, and no second copy of any function named in
   the tables in §1, §3 and §4 — each is called, not re-derived.
8. `rg -n 'not available yet \(MCP-38[678]\)|not available yet \(MCP-394\)' crates/` returns
   nothing.
9. The §7 sweep is done: every surviving "No production caller" note is true, and
   `config.rs:3438`'s is corrected.
10. `cargo check --workspace --all-targets` and `cargo clippy --workspace --all-targets` are clean,
    `cargo doc --workspace --no-deps --bins` exits 0, and no existing test is deleted or weakened.

---

## Complete — do not redo

The `/mcp` prologue, the argument split and the eight-way switch; the three listings and their
`!has_ui` gating; `arm_set_disabled` and its four write-back messages; `argument_completions` and
`MCP_SUBCOMMANDS`; `prompts.rs` in full; `terminal_hyperlink` and its call site; `render_bounded`'s
budget arithmetic and `SETUP_CHROME_ROWS = 1`; the deleted `!ctx.has_ui` guard in
`pick_oauth_server` and the doc block recording why it stays deleted — **§6 changes that function's
final gesture only, and must not reintroduce that guard.**
