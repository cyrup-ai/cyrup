//! Slash-command execution and the command catalog.
//!
//! Pi `_tryExecuteExtensionCommand` (agent-session.ts:1148-1172) and `getCommands()`
//! (agent-session.ts:2332-2354): native + wasm extension command dispatch, the outcome surfacing
//! shared by both, and the merged extension-commands + prompt-templates + skills catalog.

use cyrup_core::CancelToken;

use super::AgentSession;

impl AgentSession {
    /// Try to execute a registered extension slash command (Pi `_tryExecuteExtensionCommand`,
    /// agent-session.ts:1148-1172). Parses `/<name> <args>`, routes to the owning NATIVE extension
    /// (R-08-016), and runs its command-tier handler. Returns `true` when a command was serviced
    /// (the submission is fully handled — Pi returns `true` even when the handler errors, after
    /// surfacing the error), `false` when no command matched (fall through to normal handling).
    pub(super) async fn try_execute_extension_command(&self, text: &str) -> bool {
        let body = text.strip_prefix('/').unwrap_or(text);
        let (name, args) = body.split_once(' ').unwrap_or((body, ""));
        if name.is_empty() {
            return false;
        }
        let cancel = self.session_cancel.child_token();
        // NATIVE built-ins first (R-08-016): route to the owning native extension.
        match self.services.ext_host.execute_native_command(name, args, &cancel).await {
            // A native extension owned + serviced the command (Pi short-circuits regardless of the
            // handler's own Ok/Err — the command was "handled").
            Ok(Some(payload)) => {
                // SURFACE THE HANDLER'S OUTPUT. This arm used to bind the payload to `_`.
                //
                // Pi's handler signature is `Promise<void>` and it talks to the user through
                // `ctx.ui.*` (`agent-session.ts:1278-1301` — the return value genuinely is
                // discarded there). cyrup's `NativeExtension::execute_command` instead returns
                // `Result<Option<String>>`, a cyrup-original channel, and its built-ins populate it:
                // `cyrup-ext-subagents` alone answers all 15 of its slash commands this way and
                // contains ZERO `notify` calls. Discarding the payload therefore made every one of
                // those commands silent — `/prompt-workflow list` ran, spawned, and printed nothing;
                // `/permission-system yoloMode on` wrote the config and said nothing. The seam was
                // advertised and unread, which is the same defect class as a mechanism wired to no
                // caller.
                //
                // Routing it to `notify` reproduces pi's OBSERVABLE behaviour (the user sees the
                // command's response) using the one UI channel that is already live end-to-end:
                // `LiveHostServices::notify` -> `UiEffect::Notify` -> the TUI's `showExtensionNotify`.
                // An empty payload stays silent, so a handler that deliberately says nothing still
                // says nothing.
                // The bind is the HANDLER's own `Result` — `execute_native_command` returns
                // `Result<Option<Result<Option<String>, ExtError>>, _>`: outer = routing, `Option` =
                // did a native extension own the name, inner = what the handler itself returned.
                self.surface_command_outcome(name, &payload);
                // SEAM-003: drain the control ops the native handler queued. This route used to
                // `return true` with NO drain at all, so a native built-in's `control(...)` sat in
                // the queue until some later WASM command happened to run. Pi keeps native + wasm
                // commands in one map and runs `commandContextActions` inline for both
                // (agent-session.ts:1183-1200), so both routes must drain identically. Boxed for the
                // same reason the wasm route is: a `SendUserMessage` op re-enters the prompt path.
                Box::pin(self.apply_pending_control()).await;
                return true;
            }
            // No NATIVE owner: the name may still belong to a LIVE wasm guest command. Pi keeps
            // native + wasm commands in ONE map (`getCommand`, agent-session.ts:1183), so both
            // routes are tried before falling through to normal prompt handling.
            Ok(None) => {}
            // Routing failure (e.g. poisoned lock): degrade to "not handled" (never panic).
            Err(_) => return false,
        }
        self.try_execute_wasm_command(name, args, &cancel).await
    }

    /// Surface a slash-command handler's OWN outcome to the user — the one behaviour the native and
    /// the wasm tier must share, factored out so they cannot drift again.
    ///
    /// pi keeps native and wasm commands in ONE map and runs them through a single
    /// `_tryExecuteExtensionCommand` (`core/agent-session.ts:1277-1301` @v0.83.0), so upstream there
    /// is exactly one behaviour and it is the SURFACING one: a thrown handler goes to
    /// `this._extensionRunner.emitError({extensionPath: `command:${commandName}`, event: "command",
    /// error})` (`:1294-1299`) and the command still reports handled. Routing it to `notify`
    /// reproduces that observable behaviour over the one UI channel that is live end-to-end
    /// (`LiveHostServices::notify` → `UiEffect::Notify` → the TUI's `show_extension_notify`, and the
    /// RPC drain's `extension_error` line).
    ///
    /// `Ok(Some(text))` has no pi counterpart — pi's handler signature is `Promise<void>` and it
    /// talks to the user through `ctx.ui.*` — but it is a real cyrup-side channel that the SDK's
    /// `CommandExec` populates (`cyrup-ext-sdk/src/guest.rs`'s `run_command` → `api.execute_command`),
    /// so a guest command that answers with text must speak exactly as the identical native command
    /// already does. An empty payload stays silent.
    pub(crate) fn surface_command_outcome(
        &self,
        name: &str,
        outcome: &Result<Option<String>, cyrup_ext::ExtError>,
    ) {
        match outcome {
            Ok(Some(text)) if !text.trim().is_empty() => {
                cyrup_ext::HostServices::notify(
                    &*self.services.host_services,
                    text,
                    cyrup_ext::NotifyKind::Info,
                );
            }
            // A handler that deliberately returns nothing stays silent.
            Ok(_) => {}
            Err(e) => {
                cyrup_ext::HostServices::notify(
                    &*self.services.host_services,
                    &format!("command:{name}: {e}"),
                    cyrup_ext::NotifyKind::Error,
                );
            }
        }
    }

    /// Execute a LIVE wasm-guest-registered slash command through the real run path (R-08-016; Pi
    /// `command.handler(args, ctx)`, agent-session.ts:1189-1200). Runs the guest's `execute-command`
    /// export at command tier, then drains + applies the session-tier control ops the guest queued
    /// via its `control` capability — Pi runs those inline in the handler's `createCommandContext`
    /// (agent-session.ts:1158); cyrup bridges the SYNC guest `control()` call to the ASYNC session
    /// effect here (arch-08 §6.3, mirrors [`Self::apply_pending_control`]). Returns `true` whenever a
    /// registered guest command was serviced — Pi returns `true` even when the handler throws
    /// (:1192-1200) — and `false` when no guest owns the name (fall through to a normal prompt).
    #[cfg(feature = "wasm-host")]
    async fn try_execute_wasm_command(&self, name: &str, args: &str, cancel: &CancelToken) -> bool {
        // Only a REGISTERED command routes here; an unknown `/name` falls through (Pi `getCommand`
        // returns `undefined` ⇒ `false`, agent-session.ts:1184).
        // SEAM-048 — resolve through the DISAMBIGUATED name, so the `name:2` spelling
        // `slash_command_catalog` advertises is a spelling the dispatcher accepts. pi's `getCommand`
        // matches on `invocationName` (`core/extensions/runner.ts:648`); the bare last-wins
        // `command_owner` did not, so an advertised `check:2` was unreachable.
        if !matches!(
            self.services.ext_host.registry().resolved_command_owner(name),
            Ok(Some(_))
        ) {
            return false;
        }
        // Run the guest handler and SURFACE its outcome, exactly as the native arm above does —
        // this used to be `let _ = …`, discarding both channels. pi treats a thrown handler as still
        // "handled" (`core/agent-session.ts:1292-1300`) but emits the error first; discarding it
        // here meant a trapping guest, an epoch-deadline interrupt, an `ExtError::Cancelled` or the
        // guest's own `execute-command` error return produced NOTHING — no transcript line, no
        // toast, no RPC `extension_error`, no log — while `return true` below still swallowed the
        // input, so the user pressed Enter on `/deploy` and the UI did nothing at all. The `true`
        // stays on every branch: a faulted command must not fall through to being treated as a
        // prompt.
        let outcome = self.services.ext_host.run_command(name, args, cancel).await;
        self.surface_command_outcome(name, &outcome);
        // Apply every control op the guest queued — session-tier (compact / set-model /
        // send-message / set-thinking-level / navigate / wait-idle) AND runtime-tier (new-session /
        // switch / fork / reload), the latter through the installed [`crate::RuntimeActions`] sink
        // (SEAM-003). This used to bind the runtime-tier ops to `_deferred` and drop them. Boxed: a
        // `send_user_message` op re-enters the prompt path (Pi `pi.sendMessage` from a command
        // handler), so the async future must introduce indirection to stay finitely sized.
        Box::pin(self.apply_pending_control()).await;
        true
    }

    /// Native-host fallback (no `wasm-host` feature): no live guest can own a command, so an
    /// unmatched slash falls through to normal prompt handling.
    #[cfg(not(feature = "wasm-host"))]
    async fn try_execute_wasm_command(
        &self,
        _name: &str,
        _args: &str,
        _cancel: &CancelToken,
    ) -> bool {
        false
    }

    /// The invocable slash commands a front-end can offer (Pi `get_commands`, rpc-mode.ts:653-683):
    /// registered extension commands (`source:"extension"`), prompt templates (`source:"prompt"`),
    /// and skills (`skill:<name>`, `source:"skill"`), each with a `name`/`description`/`source`/
    /// `sourceInfo` (rpc-types.ts `RpcSlashCommand`). `sourceInfo` is the full Pi `SourceInfo`
    /// (`{path, source, scope, origin, baseDir?}`, source-info.ts:6-12), wired from the
    /// `scope`/`origin` provenance the prompt/skill structs already carry
    /// ([`cyrup_resources::ResourceOrigin::source_info_json`]).
    pub fn slash_command_catalog(&self) -> Vec<serde_json::Value> {
        let mut out: Vec<serde_json::Value> = Vec::new();
        // EXT-062 / TUI-012 — the opt-in table behind the `argumentCompletions` key emitted below.
        // `(owner, registered name)` pairs, because that is the shape both tiers record
        // (`cyrup-ext/src/facade.rs:492` for a native, `cyrup-ext/src/host/live.rs:249` for a guest's
        // `registration.add-autocomplete`) and because two extensions may register the same raw
        // name — matching on the name alone would light the key up on the wrong row.
        let autocomplete_opt_in: std::collections::HashSet<(String, String)> = self
            .services
            .ext_host
            .registry()
            .command_autocomplete()
            .unwrap_or_default()
            .into_iter()
            .map(|(owner, command)| (owner.as_str().to_string(), command))
            .collect();
        // Registered extension commands.
        //
        // SEAM-048 — sourced from `resolved_commands()`, NOT `command_descriptions()`. pi builds
        // each `RpcSlashCommand` from `command.invocationName` (`rpc-mode.ts:680-687`), which comes
        // from `getRegisteredCommands()` → `resolveRegisteredCommands()`
        // (`core/extensions/runner.ts:598-641`): duplicates get `${name}:${occurrence}` and the
        // list is in EXTENSION LOAD ORDER (`for (const ext of this.extensions) for (const command of
        // ext.commands.values())`, `:602-607`). `command_descriptions()` enumerated the last-wins
        // `HashMap`, so a duplicate command name made the second extension's command invisible AND
        // the list order shuffled between runs.
        //
        // SEAM-055 — `sourceInfo.path` is the OWNING EXTENSION's id, not `""`. pi's `SourceInfo.path`
        // is a non-optional `string` (`core/source-info.ts:6-12`) that `createSyntheticSourceInfo`
        // (`:24-40`) takes as its first positional argument precisely so a synthetic entry still
        // names something; an empty path collapses every extension command into one bucket for a
        // client grouping or filtering by source. `ResolvedCommand` carries the owner, which is why
        // the two fixes are one change.
        //
        // SEAM-084 — the remaining three fields of that `sourceInfo`, which SEAM-055 did not reach.
        // pi derives the whole object ONCE per extension in `createExtension`
        // (`core/extensions/loader.ts:433-444` @v0.83.0) and `registerCommand` copies it onto every
        // `RegisteredCommand`; `rpc-mode.ts:681-686` then passes `command.sourceInfo` straight
        // through. cyrup hard-coded a literal instead, so:
        //
        // * `source` was `"extension"` — a value that exists NOWHERE upstream. pi emits `"local"` for
        //   a filesystem-loaded extension and the `<prefix:…>` segment for a synthetic one, so a
        //   client grouping by `sourceInfo.source` could not tell the two apart. (The sibling
        //   TOP-LEVEL `"source": "extension"` below IS correct — that is `SlashCommandSource`,
        //   `core/slash-commands.ts:4`, a different field, and `rpc-mode.ts:684` really does emit it.)
        // * `baseDir` was absent entirely, so a client resolving a command's assets relative to its
        //   extension directory had nothing to resolve against.
        //
        // Both now come from the provenance the loader recorded ([`cyrup_ext::ExtensionProvenance`]).
        // `scope`/`origin` stay `"temporary"`/`"top-level"`: those are `createSyntheticSourceInfo`'s
        // defaults (`core/source-info.ts:36-37`), which `createExtension` never overrides.
        if let Ok(cmds) = self.services.ext_host.registry().resolved_commands() {
            for cmd in cmds {
                let prov = self
                    .services
                    .ext_host
                    .registry()
                    .extension_provenance(&cmd.owner)
                    .ok()
                    .flatten()
                    // No loader recorded one: the extension came in through neither the discovery
                    // nor the native path (a test harness registering straight into the registry is
                    // the only in-tree case). "Loaded by the host with no path" is upstream's
                    // `<inline>` case, so answer as that rather than inventing a fourth value.
                    .unwrap_or_else(cyrup_ext::ExtensionProvenance::inline);
                let mut source_info = serde_json::Map::new();
                source_info.insert("path".into(), serde_json::Value::from(cmd.owner.as_str()));
                source_info.insert("source".into(), serde_json::Value::from(prov.source));
                source_info.insert("scope".into(), serde_json::Value::from("temporary"));
                source_info.insert("origin".into(), serde_json::Value::from("top-level"));
                // `baseDir?: string` (`core/source-info.ts:11`) — the key is ABSENT for a synthetic
                // extension, exactly as `JSON.stringify` drops `baseDir: undefined`.
                if let Some(dir) = prov.base_dir {
                    source_info.insert("baseDir".into(), serde_json::Value::from(dir));
                }
                let mut entry = serde_json::Map::new();
                entry.insert("name".into(), serde_json::Value::from(cmd.invocation_name));
                // `RegisteredCommand.description?: string` (`core/extensions/types.ts:1163-1168`) —
                // an undescribed command OMITS the key rather than sending `""`. cyrup's
                // `CommandDescriptor.description` is a non-optional `String` whose empty value is
                // this port's representation of that absent field, so empty is omitted here.
                if !cmd.descriptor.description.is_empty() {
                    entry.insert("description".into(), serde_json::Value::from(cmd.descriptor.description));
                }
                entry.insert("source".into(), serde_json::Value::from("extension"));
                // EXT-062 / TUI-012 — cyrup's analog of pi carrying the CALLBACK itself onto the
                // autocomplete row: `getArgumentCompletions: cmd.getArgumentCompletions`
                // (`modes/interactive/interactive-mode.ts:753` @v0.84.3). A closure cannot cross
                // this boundary (it is JSON, and one tier below it is a WIT world), so what crosses
                // is the PRESENCE bit, and the front-end calls back through
                // `ExtensionHost::command_completions(invocation_name, prefix)` when it needs the
                // items. Emitted only when true, matching the `description` key above and pi's own
                // `JSON.stringify` dropping an `undefined` field — the consumer reads it as
                // absent ⇒ false.
                //
                // Extension rows ONLY. pi wires the callback in the extension-command arm alone;
                // prompt templates (`:739-743`) and skills (`:758-766`) never get one, so those two
                // arms below deliberately omit the key.
                if autocomplete_opt_in
                    .contains(&(cmd.owner.as_str().to_string(), cmd.name.clone()))
                {
                    entry.insert("argumentCompletions".into(), serde_json::Value::Bool(true));
                }
                entry.insert("sourceInfo".into(), serde_json::Value::Object(source_info));
                out.push(serde_json::Value::Object(entry));
            }
        }
        // Prompt templates.
        for t in self.services.resources.prompts.winners() {
            let mut row = serde_json::json!({
                "name": t.name,
                "description": t.description,
                "source": "prompt",
                "sourceInfo": t.origin.source_info_json(&t.path),
            });
            // CMDHINT_01 — pi's INTERACTIVE registry carries `argumentHint` from the template itself
            // (`interactive-mode.ts:685-689`, spread-if-truthy; the sibling builtin carrier is `:640-644`).
            // pi's `get_commands` RPC omits it entirely (zero `argumentHint` in `rpc-mode.ts`) because
            // interactive mode never reads that RPC — but cyrup's TUI builds its registry from THIS catalog
            // (`app/run_arms.rs:31-38`), so without the key the hint is unreachable in the one mode pi shows
            // it in. Additive: the key is ABSENT when `None`, exactly as `JSON.stringify` drops the spread.
            // `PromptTemplate::argument_hint` is already empty-filtered at parse (`prompt.rs:112-113`).
            if let Some(hint) = t.argument_hint.as_deref()
                && let Some(obj) = row.as_object_mut()
            {
                obj.insert("argumentHint".into(), serde_json::Value::from(hint));
            }
            out.push(row);
        }
        // Skills (`/skill:<name>`).
        for s in self.services.resources.skills.winners() {
            out.push(serde_json::json!({
                "name": format!("skill:{}", s.name),
                "description": s.front.description.clone().unwrap_or_default(),
                "source": "skill",
                "sourceInfo": s.origin.source_info_json(&s.skill_md),
            }));
        }
        out
    }
}
