//! `commands.ts` + `index.ts:501-617` — the `/mcp` command surface.
//!
//! [`McpExtension::execute_command`](crate::McpExtension) routes `/mcp` here. Everything a human
//! drives by hand in this crate hangs off that one arm: the three listings, the enable/disable
//! write-backs, and the two panel entry points.
//!
//! # The three listings are UI-gated, not headless
//!
//! `showStatus`, `showTools` and `showPrompts` each open with `if (!ctx.hasUI) return;`
//! (`commands.ts:45`, `:91`, `:128`), and the default arm's no-UI branch calls `showStatus` anyway —
//! so a genuinely headless `/mcp` prints **nothing at all**. Each function here returns an empty
//! `String` for that case and [`CommandCtx::notify_multiline`] skips the notification, which is how
//! the guard is expressed once instead of at three call sites.

use std::sync::Arc;

use cyrup_ext::host::{ControlOp, HostServices, NotifyKind};
use cyrup_ext::native::ExtMode;

use crate::owner::McpRuntimeOwner;
use crate::state::McpState;

/// The synthetic `commandCtx` (`index.ts:505-512`), snapshotted **before the first await**.
pub struct CommandCtx {
    /// `commandHasUI` — [`cyrup_ext::native::HostCtx::has_ui`], captured, never re-read.
    pub has_ui: bool,
    /// `ctx.mode`. `canRenderPanel` is `has_ui && mode == Tui`.
    pub mode: ExtMode,
    pub cwd: std::path::PathBuf,
    /// `ui: hasUI ? (owner ? createOwnedUi(ctx.ui, owner) : ctx.ui) : undefined` — three states,
    /// resolved by [`McpExtension::command_services`](crate::McpExtension), which is the same
    /// resolution `/mcp-auth` already goes through.
    pub ui: Option<Arc<dyn HostServices>>,
    /// `commandOwner`. Re-checked before EVERY side effect, not once at entry.
    pub owner: Option<Arc<McpRuntimeOwner>>,
}

impl CommandCtx {
    /// `commandOwner?.throwIfInactive()`. `false` ⇒ the arm returns without doing its work.
    #[must_use]
    pub fn alive(&self) -> bool {
        self.owner.as_ref().is_none_or(|owner| owner.is_active())
    }

    pub fn notify(&self, message: &str, kind: NotifyKind) {
        if let Some(ui) = &self.ui {
            ui.notify(message, kind);
        }
    }

    /// A listing's whole body as one notification, skipped when the body is empty.
    ///
    /// The empty string is each listing's `if (!ctx.hasUI) return;`, so this is where that guard is
    /// honoured — one place rather than three `if cmd.has_ui` blocks at the call sites.
    pub fn notify_multiline(&self, body: String, kind: NotifyKind) {
        if body.is_empty() {
            return;
        }
        self.notify(&body, kind);
    }

    /// `commandReload` — bound at construction upstream, here the fenced `control` verb.
    ///
    /// A stopped owner's `OwnedServices::control` answers `Err(inert_reason)`, which is the fence
    /// doing its job, so the error is logged and swallowed exactly as upstream's inert proxy would.
    pub fn reload(&self) {
        if let Some(ui) = &self.ui
            && let Err(reason) = ui.control(ControlOp::Reload)
        {
            tracing::debug!("MCP: /reload after a config change was refused: {reason}");
        }
    }

    /// `isTuiMode(ctx)` / `canRenderPanel(ctx)` — `hasUI && mode === "tui"` (`commands.ts:40-42`).
    /// The same predicate [`crate::runtime::ContextSnapshot::is_tui_mode`] spells.
    #[must_use]
    pub fn can_render_panel(&self) -> bool {
        self.has_ui && self.mode == ExtMode::Tui
    }
}

// `ConnectionStatus` is deliberately `crate::lifecycle`'s three-variant one — what
// `ServerConnection::status()` returns. Two other types share the name: `crate::ui`'s six-variant
// panel view and `crate::proxy::env`'s. Importing the wrong one compiles and lies.
use crate::lifecycle::ConnectionStatus;

/// `commands.ts:44-88` `showStatus` (13h §4.4). One multi-line Info notification.
///
/// The ladder is [`crate::proxy::discovery::execute_status`]'s six rungs with `showStatus`'s **own**
/// text: upstream keeps two renderers over one state machine and so does this. The model-facing
/// text has a header count, no failure reason and a `mcp({...})` tail; none of that belongs here.
///
/// Read through [`McpState`] directly, never a `ProxyCtx`: a listing is not a proxy mode, and
/// building one would drag the 30-method `ProxyEnv` trait into the command path.
#[must_use]
pub fn show_status(state: &McpState, has_ui: bool) -> String {
    if !has_ui {
        return String::new();
    }
    let mut lines = vec!["MCP Server Status:".to_string(), String::new()];
    for (name, definition) in &state.config.mcp_servers {
        if definition.is_disabled() {
            lines.push(format!(
                "\u{2298} {name}: disabled (run /mcp enable {name}, then /reload)"
            ));
            continue; // no tool suffix, ever
        }
        let status_of = state.manager.get_connection(name).map(|c| c.status());
        // `tool_metadata` is `IndexMap<String, Vec<ToolMetadata>>` — the per-server count is the
        // vec's length. `ToolMetadata` carries one tool each; there is no `tool_names` field.
        let metadata_len = state
            .tool_metadata
            .lock()
            .ok()
            .and_then(|metadata| metadata.get(name).map(Vec::len));
        let failed_ago = crate::live::failure_age_seconds(state, name);

        // FIRST MATCH WINS, and `failed` is tested BEFORE `cached` — a failed server must never
        // report `cached` even when its metadata is still present.
        let (icon, status, failed) = if status_of == Some(ConnectionStatus::Connected) {
            ("\u{2713}", "connected".to_string(), false)
        } else if status_of == Some(ConnectionStatus::NeedsAuth) {
            ("\u{26a0}", "needs auth".to_string(), false)
        } else if let Some(secs) = failed_ago {
            let reason = crate::ui::sanitize_terminal_text(
                &crate::live::failure_message(state, name).unwrap_or_default(),
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

        // `tools` is NEVER singularised here — `show_prompts` singularises and this does not, on
        // purpose.
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

/// `commands.ts:127-148` `showTools`.
#[must_use]
pub fn show_tools(state: &McpState, has_ui: bool) -> String {
    if !has_ui {
        return String::new();
    }
    let all_tools: Vec<String> = state
        .tool_metadata
        .lock()
        .ok()
        .map(|metadata| {
            metadata
                .iter()
                // `!isServerDisabled(state.config.mcpServers[serverName])` — a server absent from
                // the config is NOT disabled (`isServerDisabled(undefined)` is falsy), so a stale
                // metadata entry still lists. `is_none_or`, not `is_some_and`.
                .filter(|(server, _)| {
                    state
                        .config
                        .mcp_servers
                        .get(server.as_str())
                        .is_none_or(|definition| !definition.is_disabled())
                })
                .flat_map(|(_, entries)| entries.iter().map(|tool| tool.name.clone()))
                .collect()
        })
        .unwrap_or_default();

    if all_tools.is_empty() {
        // NOT the header block with a zero total.
        return "No MCP tools available".to_string();
    }
    let total = all_tools.len();
    let mut lines = vec!["MCP Tools:".to_string(), String::new()];
    lines.extend(all_tools.into_iter().map(|name| format!("  {name}")));
    lines.push(String::new());
    lines.push(format!("Total: {total} tools"));
    lines.join("\n")
}

/// `commands.ts:90-125` `showPrompts` + MCP-385a's per-group header.
#[must_use]
pub fn show_prompts(state: &McpState, has_ui: bool) -> String {
    if !has_ui {
        return String::new();
    }
    let grouped = state
        .prompt_metadata
        .lock()
        .ok()
        .map(|metadata| metadata.clone())
        .unwrap_or_default();
    let total: usize = grouped.values().map(Vec::len).sum();

    let mut lines: Vec<String> = Vec::new();
    if total == 0 {
        lines.push("No MCP prompts available".to_string());
    } else {
        lines.push("MCP Prompts:".to_string());
        lines.push(String::new());
        let mut servers: Vec<&String> = grouped.keys().collect();
        // CYRUP-DELTA: `String.localeCompare` with no locale is ICU root collation; this is byte
        // order. They agree for ASCII-lowercase names and disagree on mixed case (`Foo` vs `bar`).
        // Accepted rather than pulling a collation crate in for one sort.
        servers.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        for server in servers {
            // MCP-385a: unindented, no icon, UNSANITIZED.
            lines.push(format!("{server}:"));
            let Some(group) = grouped.get(server) else {
                continue;
            };
            // Grouping is rebuilt per call, so the in-place sort is harmless. Do NOT "optimise"
            // this into a shared cache: the map is live and the sort would race a re-registration.
            let mut prompts = group.clone();
            prompts.sort_by(|a, b| a.command_name.cmp(&b.command_name));
            for prompt in &prompts {
                let usage: String = prompt
                    .arguments
                    .iter()
                    .map(|argument| {
                        if argument.required.unwrap_or(false) {
                            format!("<{}>", argument.name)
                        } else {
                            format!("[{}]", argument.name)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                lines.push(if usage.is_empty() {
                    format!("  /{}", prompt.command_name)
                } else {
                    format!("  /{} {usage}", prompt.command_name)
                });
                if !prompt.description.is_empty() {
                    // SIX spaces, not four.
                    lines.push(format!("      {}", prompt.description));
                }
            }
            lines.push(String::new()); // per-group blank line
        }
        // Singular ONLY at 1 — unlike `show_tools`, which never singularises.
        let plural = if total == 1 { "" } else { "s" };
        lines.push(format!("Total: {total} prompt{plural}"));
    }

    // Servers that connected but whose `prompts/list` failed.
    let mut failed: Vec<String> = state
        .manager
        .get_all_connections()
        .into_iter()
        .filter(|(_, connection)| {
            connection.status() == ConnectionStatus::Connected
                && connection.prompt_discovery_failed()
        })
        .map(|(name, _)| name)
        .collect();
    failed.sort();
    if !failed.is_empty() {
        let note = format!(
            "Prompt discovery failed for: {} (run /mcp reconnect <server> to retry)",
            failed.join(", ")
        );
        if total == 0 {
            // Appended to the SAME sentence, with a leading space.
            if let Some(last) = lines.last_mut() {
                last.push(' ');
                last.push_str(&note);
            }
        } else {
            // Its own final line.
            lines.push(note);
        }
    }
    lines.join("\n")
}

// =================================================================================================
// The prologue, the split and the switch (`index.ts:501-617`)
// =================================================================================================

use cyrup_ext::ExtError;
use cyrup_ext::native::HostCtx;

use crate::McpExtension;

impl McpExtension {
    /// `index.ts:501-527`'s fenced prologue. Returns `None` once it has already notified the user.
    ///
    /// Everything is snapshotted **before the first await**: an owner captured afterwards would
    /// fence against the generation that replaced it, which is the bug `on_input` documents.
    ///
    /// The await itself is [`McpExtension::await_committed_state`] — un-timed, unlike the tool
    /// bodies which use `INIT_WAIT_TIMEOUT_MS`. It is not re-implemented here: it already carries
    /// both of upstream's failure literals ("MCP not initialized", "MCP initialization failed: …")
    /// and `/mcp-auth` awaits through it, so a second copy would be two spellings of one wait.
    pub(crate) async fn command_prologue(
        &self,
        ctx: &HostCtx,
    ) -> Option<(Arc<McpState>, CommandCtx)> {
        let owner = self.owner();
        let cmd = CommandCtx {
            has_ui: ctx.has_ui,
            mode: ctx.mode,
            cwd: ctx.cwd.clone(),
            // `ui: hasUI ? (owner ? createOwnedUi(ctx.ui, owner) : ctx.ui) : undefined`, resolved
            // by the same helper `/mcp-auth` uses. Do not re-derive the three states here.
            ui: self.command_services(ctx, owner.as_ref()),
            owner,
        };

        match self.await_committed_state().await {
            Ok(state) => {
                // `commandOwner?.throwIfInactive()` — the post-await fence.
                if !cmd.alive() {
                    return None;
                }
                Some((state, cmd))
            }
            Err(message) => {
                cmd.notify(&message, NotifyKind::Error);
                None
            }
        }
    }

    /// `/mcp`'s handler — `index.ts:528-617`'s split and switch.
    ///
    /// Every arm returns `Ok(None)`: `/mcp` speaks through notifications, never through the
    /// command's return channel, because a returned `Some` would be echoed into the transcript as
    /// model-visible text.
    pub(crate) async fn on_mcp_command(
        &self,
        args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        let Some((state, cmd)) = self.command_prologue(ctx).await else {
            return Ok(None);
        };

        // `parts = args?.trim()?.split(/\s+/) ?? []` — `"".split(/\s+/)` yields `[""]`, so the
        // no-argument case is `subcommand == ""`, not an empty vec.
        let trimmed = args.trim();
        let parts: Vec<&str> = if trimmed.is_empty() {
            vec![""]
        } else {
            trimmed.split_whitespace().collect()
        };
        let subcommand = parts.first().copied().unwrap_or("");
        // `reconnect` takes the FIRST word; `logout`/`disable`/`enable` take the whole rest. They
        // are not interchangeable: `/mcp logout my server` targets `"my server"` and
        // `/mcp reconnect a b` targets `"a"`.
        let target_server = parts.get(1).copied();
        let rest = parts
            .get(1..)
            .map(|rest| rest.join(" "))
            .unwrap_or_default();

        match subcommand {
            "reconnect" => {
                if !cmd.alive() {
                    return Ok(None);
                }
                self.arm_reconnect(&state, &cmd, target_server).await;
                if self.direct_tools_frozen() {
                    self.sync_tool_surface();
                }
            }
            "tools" => cmd.notify_multiline(show_tools(&state, cmd.has_ui), NotifyKind::Info),
            "prompts" => cmd.notify_multiline(show_prompts(&state, cmd.has_ui), NotifyKind::Info),
            "setup" => {
                if !cmd.alive() {
                    return Ok(None);
                }
                if state.programmatic_config.is_some() {
                    cmd.notify(
                        "MCP setup is unavailable when config is supplied by createMcpAdapter().",
                        NotifyKind::Info,
                    );
                } else if self
                    .arm_setup(&cmd, crate::ui::SetupScreen::Setup, true)
                    .await
                {
                    if !cmd.alive() {
                        return Ok(None);
                    }
                    cmd.reload();
                    return Ok(None); // an early RETURN, not a break
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
                if !cmd.alive() {
                    return Ok(None);
                }
                self.arm_logout(&state, &cmd, &rest).await;
            }
            sub @ ("disable" | "enable") => self.arm_set_disabled(&state, &cmd, sub, &rest),
            // `status`, `""` and ANYTHING UNRECOGNISED share one arm: `/mcp wibble` opens the panel.
            _ => {
                if cmd.has_ui {
                    if !cmd.alive() {
                        return Ok(None);
                    }
                    if state.programmatic_config.is_some() {
                        cmd.notify(
                            "MCP status is shown from the in-memory SDK config; configuration discovery is unavailable.",
                            NotifyKind::Info,
                        );
                        cmd.notify_multiline(show_status(&state, cmd.has_ui), NotifyKind::Info);
                    } else if self.arm_browser_panel(&state, &cmd, ctx).await {
                        if !cmd.alive() {
                            return Ok(None);
                        }
                        cmd.reload();
                        return Ok(None);
                    }
                } else {
                    // `showStatus` returns immediately when `!hasUI` — so this is silent, on
                    // purpose, and `notify_multiline` skips the empty body.
                    cmd.notify_multiline(show_status(&state, cmd.has_ui), NotifyKind::Info);
                }
            }
        }
        Ok(None)
    }

    /// `/mcp disable` / `/mcp enable` (`index.ts:569-597`) — MCP-389.
    ///
    /// Both refusals and both outcomes fall through; none of them returns early. This is the **only**
    /// subcommand that tells the user to run `/reload` themselves.
    fn arm_set_disabled(&self, state: &McpState, cmd: &CommandCtx, sub: &str, name: &str) {
        if state.programmatic_config.is_some() {
            cmd.notify(
                &format!(
                    "/mcp {sub} is unavailable when config is supplied by createMcpAdapter()."
                ),
                NotifyKind::Info,
            );
            return;
        }
        if name.is_empty() {
            cmd.notify(&format!("Usage: /mcp {sub} <server>"), NotifyKind::Error);
            return;
        }
        if !state.config.mcp_servers.contains_key(name) {
            cmd.notify(
                &format!("Server \"{name}\" not found in effective config"),
                NotifyKind::Error,
            );
            return;
        }
        if !cmd.alive() {
            return;
        }
        let disabled = sub == "disable";
        // The writer owns the file, the key spelling, the no-op detection and its four error
        // strings. Never re-derive any of them here.
        match self
            .config_context()
            .write_project_server_disabled_override(name, disabled)
        {
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
            // Upstream lets the throw escape into pi's command-error path. cyrup's equivalent is
            // this Error notify, which keeps the writer's exact message intact instead of wrapping
            // it in `command:mcp: `.
            Err(error) => cmd.notify(&error.to_string(), NotifyKind::Error),
        }
    }

    /// `reconnectServers(state, ctx, targetServer)` (`commands.ts:224-242`) — `/mcp reconnect` and
    /// `/mcp reconnect <server>`.
    ///
    /// **The connect itself is not written here.** Each server goes through
    /// [`McpExtension::reconnect_one`], which is `reconnectServer`'s try/catch over
    /// [`crate::proxy::execute_connect`] — the eight-step metadata commit in upstream's order. A
    /// second hand-written reconnect would fork that order; this arm owns only the iteration and
    /// the two pre-guards upstream applies per server.
    ///
    /// Upstream closes unconditionally before connecting, while `execute_connect` calls
    /// `env.reconnect` only for a currently-`Connected` server and plain `connect` otherwise. For a
    /// failed or idle server upstream's `close` is a no-op, so the two sequences agree.
    async fn arm_reconnect(&self, state: &McpState, cmd: &CommandCtx, server: Option<&str>) {
        // `if (targetServer && !state.config.mcpServers[targetServer])` — the named-but-absent
        // refusal happens BEFORE the loop, so `/mcp reconnect nope` says so once.
        if let Some(name) = server
            && !state.config.mcp_servers.contains_key(name)
        {
            cmd.notify(
                &format!("Server \"{name}\" not found in config"),
                NotifyKind::Error,
            );
            return;
        }
        let Some(ctx) = self.proxy_ctx() else {
            // Upstream has no analog: its `state.manager` exists as soon as the extension does, so
            // `reconnectServer` always has something to call. Here the proxy context is installed
            // by the commit tail, and a `/mcp reconnect` racing initialization would otherwise
            // iterate every server printing nothing.
            cmd.notify(
                "MCP is not initialized; nothing was reconnected.",
                NotifyKind::Warning,
            );
            return;
        };
        // `targetServer ? [targetServer] : Object.keys(state.config.mcpServers)` — file order,
        // which is why `mcp_servers` is an `IndexMap`. Collected before the loop because
        // `reconnect_one` borrows nothing from the map but the loop awaits across it.
        let names: Vec<String> = match server {
            Some(name) => vec![name.to_string()],
            None => state.config.mcp_servers.keys().cloned().collect(),
        };
        for name in names {
            // `reconnectServer`'s own two pre-guards (`commands.ts:158-168`). `execute_connect`
            // reports both through a `ToolResult` aimed at the model, so the human-facing forms
            // are raised here instead.
            let Some(definition) = state.config.mcp_servers.get(&name) else {
                continue;
            };
            if definition.is_disabled() {
                cmd.notify(
                    &format!("MCP: {name} is disabled. Run /mcp enable {name}, then /reload."),
                    NotifyKind::Warning,
                );
                continue;
            }
            // Re-checked per server, not once at entry: a `/reload` landing mid-loop must stop the
            // remaining reconnects rather than finish converging against a dead generation.
            if !cmd.alive() {
                return;
            }
            let outcome = self.reconnect_one(&ctx, &name).await;
            cmd.notify(&outcome.message, outcome.kind);
        }
        crate::live::update_status_bar(state);
    }

    /// `openMcpSetup(state, pi, ctx, configOverridePath, mode, options)` (`commands.ts:406-481`).
    ///
    /// Returns whether anything was written, which is what decides the `cmd.reload()` in the switch.
    ///
    /// Two parameters rather than two functions: `/mcp setup` opens the Setup screen with host
    /// config discovery on, and `/mcp` with nothing configured opens the Empty screen with it off
    /// (`commands.ts:562`). Upstream passes both as arguments to one function and so does this.
    ///
    /// The `programmatic_config` refusal is upstream's third guard but the switch already raises it
    /// before calling here, so it is not repeated.
    async fn arm_setup(
        &self,
        cmd: &CommandCtx,
        screen: crate::ui::SetupScreen,
        include_host_configs: bool,
    ) -> bool {
        // `canRenderPanel(ctx)` — `hasUI && mode === "tui"`, spelled once on `CommandCtx`.
        if !cmd.can_render_panel() {
            cmd.notify(
                &crate::ui::panel_unavailable_message(crate::extension::mode_str(cmd.mode)),
                NotifyKind::Info,
            );
            return false;
        }
        let Some(ui) = cmd.ui.as_ref() else {
            return false;
        };

        let mut diagnostics = Vec::new();
        let discovery = self
            .config_context()
            .mcp_discovery_summary(include_host_configs, &mut diagnostics);
        let onboarding = crate::onboarding::load_onboarding_state(&self.dirs().onboarding_state());

        // The callbacks own the flag `openMcpSetup` closes over, so the caller reads it back off
        // the same object after the panel resolves.
        let callbacks = Arc::new(crate::panel_host::SetupCallbacks::new(
            self.dirs().clone(),
            self.home().cloned(),
            discovery.fingerprint.clone(),
            include_host_configs,
        ));
        let model = crate::ui::McpSetupPanelModel::new(
            discovery,
            onboarding,
            Arc::clone(&callbacks) as Arc<dyn crate::ui::SetupPanelCallbacks>,
            screen,
            crate::ui::PanelKeys::from_agent_dir(self.dirs().agent_dir()),
        );
        // `false` is "no host took the overlay" — pi's `!ctx.hasUI` branch, not an error. The
        // refusal text is the same one the mode guard above raises, because from the user's side
        // the two are one situation: there is no terminal panel to show.
        if !crate::ui::open_mcp_setup_panel(
            ui.as_ref(),
            model,
            Arc::clone(&callbacks) as Arc<dyn crate::ui::SetupPanelCallbacks>,
            tokio::runtime::Handle::current(),
        ) {
            cmd.notify(
                &crate::ui::panel_unavailable_message(crate::extension::mode_str(cmd.mode)),
                NotifyKind::Info,
            );
            return false;
        }
        callbacks.config_changed()
    }

    /// `logoutServer(serverName, state, ctx)` (`commands.ts:336-381`) — clear the credentials,
    /// then drop the connection that is still holding the old token.
    ///
    /// **The two failure messages are not interchangeable, which is why both arms exist.** The
    /// first means nothing was cleared; the second means the credentials ARE gone but a live
    /// connection survived them, so the session keeps working until it drops and the user needs to
    /// know which of the two happened.
    ///
    /// Step one is [`crate::oauth::remove_auth`], which owns the pending-callback cancellation, the
    /// persisted-state clear and the four interleaved abort checks. None of that is re-derived here.
    async fn arm_logout(&self, state: &Arc<McpState>, cmd: &CommandCtx, server: &str) {
        if !state.config.mcp_servers.contains_key(server) {
            cmd.notify(
                &format!("Server \"{server}\" not found in config"),
                NotifyKind::Error,
            );
            return;
        }
        let cancel = cmd
            .owner
            .as_ref()
            .map_or_else(cyrup_core::CancelToken::new, |owner| owner.token());
        // The GENERATION's vault. `McpState::auth_options` explains why minting a second store
        // here would leave the removal talking to its own in-process cache.
        let options = state.auth_options(self.dirs(), &cancel);
        if let Err(error) = crate::oauth::remove_auth(server, &options).await {
            // `if (isAbortError(error, signal)) throw error` — a cancellation is not a failure to
            // report; the user asked for it, and upstream rethrows rather than notifying.
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

    /// `openMcpPanel(state, pi, ctx, configOverridePath, onDirectToolsConfigChanged)`
    /// (`commands.ts:539-603`). Returns whether the config changed.
    ///
    /// Takes `&Arc<McpState>` and the raw `&HostCtx` rather than `&McpState` and `CommandCtx`
    /// alone: [`crate::ui::McpPanelCallbacks`]' two async members return `BoxFuture<'static, _>`,
    /// so the callbacks object must OWN a state handle and a context. `CommandCtx` carries the
    /// snapshotted `has_ui`/`mode`/`cwd` and the fenced services handle, which is the right shape
    /// for every other arm; this one arm needs more, so it asks for more.
    async fn arm_browser_panel(
        &self,
        state: &Arc<McpState>,
        cmd: &CommandCtx,
        ctx: &HostCtx,
    ) -> bool {
        // GUARD 2 — a UI with no terminal overlay. Upstream re-renders `showStatus` as TEXT here
        // (`commands.ts:557`) rather than refusing, because every fact the panel shows is also
        // available as a listing. (Guard 1, the programmatic-config refusal, is at the call site.)
        if !cmd.can_render_panel() {
            cmd.notify_multiline(show_status(state, cmd.has_ui), NotifyKind::Info);
            return false;
        }
        // GUARD 3 — nothing configured yet. `/mcp` on a fresh machine is a SETUP prompt, not an
        // empty table, and it opens on the Empty screen with import discovery OFF.
        if state.config.mcp_servers.is_empty() {
            return self
                .arm_setup(cmd, crate::ui::SetupScreen::Empty, false)
                .await;
        }
        let (Some(ui), Some(weak)) = (cmd.ui.as_ref(), self.self_handle()) else {
            // No fenced handle, or an extension not built through `into_arc` (the in-crate unit
            // tests). Either way the panel cannot be wired, so fall back to the listing rather
            // than opening a half-built one.
            cmd.notify_multiline(show_status(state, cmd.has_ui), NotifyKind::Info);
            return false;
        };

        let mut diagnostics = Vec::new();
        let config_context = self.config_context();
        let provenance = config_context.server_provenance(&mut diagnostics);
        // `crate::dirs::load_metadata_cache` — the panel is typed against `crate::dirs`'
        // `MetadataCache`, not `crate::registration`'s same-named one.
        let cache = crate::dirs::load_metadata_cache(&self.dirs().metadata_cache());

        let summary = config_context.mcp_standard_config_summary(&mut diagnostics);
        let onboarding = crate::onboarding::load_onboarding_state(&self.dirs().onboarding_state());
        let (notice_lines, fingerprint) =
            crate::panel_host::shared_config_notice(&summary, &onboarding);

        let callbacks: Arc<dyn crate::ui::McpPanelCallbacks> =
            Arc::new(crate::panel_host::PanelCallbacks::new(
                weak,
                Arc::clone(state),
                ctx.clone(),
                self.dirs().clone(),
            ));
        let model = crate::ui::McpPanelModel::new(
            &state.config,
            cache,
            &provenance,
            Arc::clone(&callbacks),
            crate::ui::PanelOptions {
                notice_lines,
                auth_only: false,
                keys: crate::ui::PanelKeys::from_agent_dir(self.dirs().agent_dir()),
                // `None` is `default_server_hasher` — the crate's real digest, resolvers and all.
                // Injecting one here would be a second spelling of the same hash (MCP-141).
                server_hash: None,
            },
        );
        let Some(result) = crate::ui::open_mcp_panel(
            ui.as_ref(),
            model,
            callbacks,
            tokio::runtime::Handle::current(),
        ) else {
            // `None` is pi's `!ctx.hasUI` branch, NOT an error — fall back to the listing.
            cmd.notify_multiline(show_status(state, cmd.has_ui), NotifyKind::Info);
            return false;
        };

        // Stamped once the panel has CLOSED, and only when the notice was actually rendered:
        // `markSharedConfigHintShown(fingerprint)` (`commands.ts:600`). Stamping for a notice that
        // never appeared would retire it forever.
        if let Some(fingerprint) = fingerprint
            && let Err(error) = crate::onboarding::mark_shared_config_hint_shown(
                &self.dirs().onboarding_state(),
                Some(fingerprint.as_str()),
            )
        {
            tracing::debug!("MCP: could not record the shared-config hint: {error}");
        }

        let changes = result.to_config_changes();
        if result.cancelled || changes.is_empty() {
            return false;
        }
        match crate::config::write_direct_tools_config(&changes, &provenance, &state.config) {
            Ok(()) => {
                // `onDirectToolsConfigChanged?.(changes)` is TWO calls upstream —
                // `applyDirectToolConfigChanges` then `syncToolSurface`. Only the second has a
                // counterpart here, and that is not an omission: the first mutates
                // `state.config.mcpServers[name].directTools` in memory so that
                // `syncToolSurface`'s `state?.config` read sees it, while
                // `McpExtension::sync_tool_surface` deliberately RE-READS config and cache from
                // disk (see its own comment on why reusing the captured config is the bug it
                // exists to fix). `write_direct_tools_config` flushed those same changes one line
                // above, so the disk re-read observes exactly what the in-memory mutation would
                // have produced. `McpState::config` therefore needs no interior mutability.
                //
                // CYRUP-DELTA: a server with no provenance entry is skipped by the writer
                // (upstream skips it too), and upstream's in-memory arm would still apply it for
                // the rest of the session where the disk re-read cannot. `server_provenance`
                // returns an entry for every configured server, so the panel cannot reach it.
                self.sync_tool_surface();
                cmd.notify("Direct tools updated for this session.", NotifyKind::Info);
                // NOT `true`. Upstream initialises `configChanged` to `false` and sets it ONLY in
                // the failure arm below (`commands.ts:594`): the success path has already
                // re-synced the surface in-process, so a `/reload` would be a redundant restart.
                // Returning `true` here would restart the session on every panel edit.
                false
            }
            Err(error) => {
                // CYRUP-DELTA on the TEXT, not the outcome. Upstream wraps the write, the refresh
                // and the success notice in one `try` and reports every failure as "Direct tools
                // updated, but live refresh failed" (`commands.ts:592`) — accurate only when it was
                // the refresh that threw. Here `sync_tool_surface` returns a `bool` and cannot
                // fail, so the write is the *only* thing this arm can be reporting, and upstream's
                // sentence would tell the user their change was saved when it was not.
                //
                // "fully" is load-bearing: `write_direct_tools_config` groups the changes by target
                // file and `?`s on the first failure, so a change spanning two files can leave the
                // first one written. That is also why this returns `true` and triggers a reload —
                // disk may have moved and the in-process surface was never synced, which is exactly
                // the state a reload resolves.
                cmd.notify(
                    &format!("Direct tools could not be fully saved: {error}"),
                    NotifyKind::Error,
                );
                true
            }
        }
    }
}

// =================================================================================================
// MCP-041 — HA-2's dynamic argument completions
// =================================================================================================

/// The eight subcommands of `getArgumentCompletions`' first branch (`index.ts:476-485`), in
/// upstream's user-visible order.
///
/// There is no `token` row: upstream declares eight subcommands and none of them is `token`. The
/// same eight are `mcp_command_descriptor`'s static `completions`, which is the list the `/` menu
/// shows before any argument is typed; this is the list shown *after* `/mcp `.
const MCP_SUBCOMMANDS: [&str; 8] = [
    "reconnect",
    "tools",
    "prompts",
    "setup",
    "logout",
    "disable",
    "enable",
    "status",
];

/// `getArgumentCompletions` (`index.ts:470-497`) — both branches.
///
/// Filtering is a literal `starts_with`, **not** fuzzy matching: upstream uses `String.startsWith`
/// on both branches. Upstream's `null`-vs-`[]` distinction collapses harmlessly into "empty" — the
/// TUI already treats an empty candidate set as "no popup".
///
/// # Why the rows are bare values, not `value — description` pairs
///
/// The front-end's extension-completion path (`ExtensionCompletions.items: Vec<String>`) uses each
/// string as **both** the popup label and the inserted text, and `apply`'s `SlashArgument` arm
/// replaces the argument span with it. A row carrying an em-dash description would therefore be
/// typed into the prompt verbatim. Upstream's `{value, label}` pair has no counterpart on this
/// path, so the label half is dropped rather than smuggled into the value.
#[must_use]
pub(crate) fn argument_completions(ext: &McpExtension, command: &str, prefix: &str) -> Vec<String> {
    // `/mcp-auth` deliberately declares NO completer upstream — an asymmetry with `/mcp`, kept.
    if command != crate::registration::MCP_COMMAND {
        return Vec::new();
    }
    let normalized = prefix.trim_start();
    // `normalized.match(/^(\S+)\s+(.*)$/)` — a non-space run, whitespace, then the REST (possibly
    // empty), which is why `split_once` on whitespace is the exact equivalent.
    let Some((sub, arg_prefix)) = normalized.split_once(char::is_whitespace) else {
        return MCP_SUBCOMMANDS
            .iter()
            .filter(|value| value.starts_with(normalized))
            .map(|value| (*value).to_string())
            .collect();
    };
    if !matches!(sub, "reconnect" | "logout" | "disable" | "enable") {
        return Vec::new();
    }
    // `|| !state` — a completer that fires before initialization offers nothing rather than falling
    // back to a config re-read, because upstream reads LIVE state here.
    let Some(state) = ext.state() else {
        return Vec::new();
    };
    let arg_prefix = arg_prefix.trim_start();
    state
        .config
        .mcp_servers
        .keys()
        .filter(|name| name.starts_with(arg_prefix))
        // The bare server name: `apply`'s `SlashArgument` arm replaces the ARGUMENT span, not the
        // whole line, so re-prefixing with `{sub} ` would produce `/mcp reconnect reconnect srv`.
        .map(|name| (*name).clone())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use crate::config::{McpConfig, ServerEntry};
    use crate::state::{McpState, McpStateParts};

    /// An `McpState` over `config` with an empty manager — the two things every listing reads.
    fn state_with(config: McpConfig) -> Arc<McpState> {
        use futures::FutureExt;

        let owner = Arc::new(crate::owner::McpRuntimeOwner::new());
        let manager = Arc::new(crate::state::McpServerManager::default());
        let lifecycle = Arc::new(crate::lifecycle::McpLifecycleManager::new(
            Arc::clone(&manager),
            Arc::new(|_| false),
        ));
        Arc::new(McpState::new(McpStateParts {
            owner,
            manager,
            lifecycle,
            config,
            programmatic_config: None,
            oauth_runtime: crate::oauth::create_oauth_runtime(None),
            auth_storage_options: crate::credentials::AuthStorageOptions::default(),
            ui: None,
            open_browser: Arc::new(|_| async { Ok(()) }.boxed()),
            send_message: Arc::new(|_| {}),
        }))
    }

    fn config_with(servers: &[(&str, ServerEntry)]) -> McpConfig {
        let mut config = McpConfig::default();
        for (name, entry) in servers {
            config
                .mcp_servers
                .insert((*name).to_string(), entry.clone());
        }
        config
    }

    fn tool(name: &str) -> crate::proxy::ToolMetadata {
        crate::proxy::ToolMetadata {
            name: name.to_string(),
            original_name: name.to_string(),
            description: String::new(),
            resource_uri: None,
            ui_visibility: None,
            input_schema: None,
        }
    }

    fn ext() -> crate::McpExtension {
        crate::McpExtension::new(crate::dirs::McpDirs::new(
            std::path::PathBuf::from("/nonexistent/agent"),
            std::path::PathBuf::from("/w"),
        ))
    }

    /// The first branch: no whitespace yet, so the eight subcommands filtered by `starts_with`.
    #[test]
    fn the_subcommand_branch_filters_by_prefix_not_fuzzily() {
        let ext = ext();
        let rows = argument_completions(&ext, crate::registration::MCP_COMMAND, "");
        assert_eq!(
            rows.len(),
            8,
            "all eight, and only eight — `token` is not one of them"
        );
        assert!(!rows.iter().any(|row| row == "token"));

        let rows = argument_completions(&ext, crate::registration::MCP_COMMAND, "re");
        assert_eq!(rows, vec!["reconnect".to_string()]);

        // `starts_with`, not fuzzy: `ts` would fuzzy-match `tools`/`status` and must not.
        assert!(argument_completions(&ext, crate::registration::MCP_COMMAND, "ts").is_empty());
    }

    /// `/mcp-auth` deliberately declares no completer upstream — the asymmetry is kept.
    #[test]
    fn mcp_auth_offers_nothing() {
        let ext = ext();
        assert!(argument_completions(&ext, crate::registration::MCP_AUTH_COMMAND, "").is_empty());
    }

    /// The second branch needs live state; with none it offers nothing rather than falling back to
    /// a config re-read, because upstream reads LIVE state here.
    #[test]
    fn the_server_branch_is_silent_before_initialization() {
        let ext = ext();
        for sub in ["reconnect ", "logout ", "disable ", "enable "] {
            assert!(
                argument_completions(&ext, crate::registration::MCP_COMMAND, sub).is_empty(),
                "{sub} should offer nothing with no state"
            );
        }
    }

    /// A subcommand that takes no server argument offers nothing, even with a trailing space.
    #[test]
    fn subcommands_without_a_server_argument_offer_nothing() {
        let ext = ext();
        for sub in ["tools ", "prompts ", "status ", "setup "] {
            assert!(
                argument_completions(&ext, crate::registration::MCP_COMMAND, sub).is_empty(),
                "{sub} takes no server argument"
            );
        }
    }

    // ---- the three listings ----

    /// **The rung-order assertion.** A server that is BOTH inside the 60 s failure window AND holds
    /// cached metadata must render `failed`, never `cached`. Testing `cached` first passes every
    /// other case in this file and only breaks here.
    #[test]
    fn a_failed_server_reports_failed_even_when_metadata_is_cached() {
        let state = state_with(config_with(&[("srv", ServerEntry::default())]));
        // Metadata present — the `cached` rung's whole condition.
        state
            .tool_metadata
            .lock()
            .unwrap()
            .insert("srv".to_string(), vec![tool("a"), tool("b")]);
        crate::live::record_failure(&state, "srv", "boom");

        let out = show_status(&state, true);
        assert!(out.contains("srv: failed"), "got {out}");
        assert!(
            !out.contains("cached"),
            "the failed rung must win; got {out}"
        );
        // The failed arm's suffix is empty — no tool count beside a failure.
        assert!(!out.contains("2 tools"), "got {out}");
        assert!(
            out.contains("boom"),
            "the sanitized reason rides the row; got {out}"
        );
    }

    /// With no failure recorded, the same metadata takes the `cached` rung and carries the count.
    #[test]
    fn cached_metadata_reports_cached_with_its_count() {
        let state = state_with(config_with(&[("srv", ServerEntry::default())]));
        state
            .tool_metadata
            .lock()
            .unwrap()
            .insert("srv".to_string(), vec![tool("a"), tool("b")]);
        let out = show_status(&state, true);
        assert!(out.contains("srv: cached (2 tools, cached)"), "got {out}");
    }

    /// A disabled server renders its own row via `continue` — **no tool suffix**, ever.
    #[test]
    fn a_disabled_server_has_no_tool_suffix() {
        let disabled = ServerEntry {
            disabled: Some(true),
            ..Default::default()
        };
        let state = state_with(config_with(&[("srv", disabled)]));
        state
            .tool_metadata
            .lock()
            .unwrap()
            .insert("srv".to_string(), vec![tool("a")]);
        let out = show_status(&state, true);
        assert!(
            out.contains("srv: disabled (run /mcp enable srv, then /reload)"),
            "got {out}"
        );
        assert!(
            !out.contains("tools"),
            "a disabled row carries no count; got {out}"
        );
    }

    /// All three listings are UI-GATED, not headless: each returns `""` so `notify_multiline` skips
    /// the notification. That pair is what makes a print/json `/mcp` silent.
    #[test]
    fn every_listing_is_empty_without_a_ui() {
        let state = state_with(config_with(&[("srv", ServerEntry::default())]));
        assert!(show_status(&state, false).is_empty());
        assert!(show_tools(&state, false).is_empty());
        assert!(show_prompts(&state, false).is_empty());
    }

    /// `tools` is NEVER singularised and `prompt{s}` IS singular at exactly one. The two rules
    /// differ on purpose, so they are pinned together where the asymmetry is visible.
    #[test]
    fn tools_never_singularise_but_prompts_do() {
        let state = state_with(config_with(&[("srv", ServerEntry::default())]));
        state
            .tool_metadata
            .lock()
            .unwrap()
            .insert("srv".to_string(), vec![tool("only")]);
        assert!(
            show_tools(&state, true).ends_with("Total: 1 tools"),
            "never singularised"
        );

        state.prompt_metadata.lock().unwrap().insert(
            "srv".to_string(),
            vec![crate::state::PromptMetadata {
                server_name: "srv".to_string(),
                original_name: "p".to_string(),
                command_name: "srv:p".to_string(),
                title: None,
                description: String::new(),
                arguments: Vec::new(),
            }],
        );
        assert!(
            show_prompts(&state, true).contains("Total: 1 prompt"),
            "singular at one"
        );
        assert!(!show_prompts(&state, true).contains("1 prompts"));
    }

    /// `show_tools`' filter is `is_none_or`: a server holding metadata but ABSENT from the config is
    /// not disabled (`isServerDisabled(undefined)` is falsy upstream), so its tools still list. A
    /// server explicitly disabled in the config does not.
    #[test]
    fn an_unconfigured_server_still_lists_but_a_disabled_one_does_not() {
        let disabled = ServerEntry {
            disabled: Some(true),
            ..Default::default()
        };
        let state = state_with(config_with(&[("off", disabled)]));
        {
            let mut metadata = state.tool_metadata.lock().unwrap();
            metadata.insert("off".to_string(), vec![tool("hidden")]);
            metadata.insert("ghost".to_string(), vec![tool("orphan")]);
        }
        let out = show_tools(&state, true);
        assert!(
            out.contains("orphan"),
            "an unconfigured server lists; got {out}"
        );
        assert!(
            !out.contains("hidden"),
            "a disabled server does not; got {out}"
        );
    }

    /// The empty-config message is two lines and replaces the rows entirely.
    #[test]
    fn an_empty_config_explains_itself() {
        let state = state_with(McpConfig::default());
        let out = show_status(&state, true);
        assert!(out.contains("No MCP servers configured"), "got {out}");
        assert!(out.contains("Run /mcp setup"), "got {out}");
    }

    /// With nothing cached, `show_tools` returns the BARE string — not the header block with a zero
    /// total.
    #[test]
    fn no_tools_is_a_bare_sentence() {
        let state = state_with(config_with(&[("srv", ServerEntry::default())]));
        assert_eq!(show_tools(&state, true), "No MCP tools available");
    }

    // ---- MCP-389 ----

    /// The writer owns the file, the key spelling and the no-op detection; this arm owns only the
    /// four notices. Both refusals and both outcomes fall through — none returns early.
    #[test]
    fn set_disabled_writes_once_and_reports_the_no_op_second_time() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dirs = crate::dirs::McpDirs::new(tmp.path().join("agent"), tmp.path().join("project"));
        std::fs::create_dir_all(tmp.path().join("project")).expect("cwd");
        let ctx = crate::config::ConfigContext::new(dirs, None).with_home(tmp.path().to_path_buf());

        let first = ctx
            .write_project_server_disabled_override("srv", true)
            .expect("first write");
        assert!(first.changed, "the first write changes the document");
        let second = ctx
            .write_project_server_disabled_override("srv", true)
            .expect("second write");
        assert!(
            !second.changed,
            "a repeat must report no change so the arm says `is already disabled`"
        );
        assert_eq!(
            first.path, second.path,
            "both name the same project override file"
        );
    }
}
