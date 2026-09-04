//! The demo extension's commands that READ or MUTATE session / agent state: the base-context
//! accessors, the greeting command and its completer, the host-mode report, the tree/session
//! mutations, the thinking-level and model setters, the active-tool restriction, and the
//! `withSession` re-binding callback.

use crate::{CommandDescriptor, ExtensionApi, NewSessionOptions, ReplacedSessionContext};
use serde_json::json;

pub(super) fn install(api: &mut ExtensionApi) {
    // EXT-005: report the BASE-context state accessors (Pi types.ts:329-346) back as text so a test
    // can prove the guest reads the HOST's live answer rather than a hard-coded default.
    api.register_command(
        "ctxstate",
        CommandDescriptor::new("Report the base-context state accessors (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let base = ctx.ctx();
            Ok(Some(format!(
                "idle={} pending={} trusted={} prompt={}",
                base.is_idle(),
                base.has_pending_messages(),
                base.is_project_trusted(),
                base.system_prompt(),
            )))
        },
    );

    // A slash command (R-08-016) with a dynamic argument completer: `/greet <name>` -> a greeting.
    api.register_command_with_completions(
        "greet",
        CommandDescriptor::new("Greet someone by name (demo command)."),
        |args: &str, ctx: &crate::CommandCtx| {
            ctx.ui().notify("greet command ran");
            // Address a keyed status segment (Pi `setStatus(key, text)`, types.ts:148 @v0.83.0), then clear
            // it (Pi `setStatus(key, undefined)`) — proves keyed set + clear over the boundary.
            ctx.ui().set_status("greet", Some("greeting…"));
            ctx.ui().clear_status("greet");
            // A COMMAND-tier control op (R-08-008): legal here, recorded by the host backend.
            // Carries Pi's `CompactOptions.customInstructions` (types.ts:296-300) so the host-side
            // op is proved to arrive with its payload, not just to arrive.
            let _ = ctx.compact_with(&crate::CompactOptions {
                custom_instructions: Some("demo: keep the greeting".into()),
            });
            Ok(Some(format!("hello, {}!", args.trim())))
        },
        |prefix: &str| {
            ["world", "team", "everyone"]
                .iter()
                .filter(|c| c.starts_with(prefix))
                .map(|c| c.to_string())
                .collect()
        },
    );

    // Report the host's run mode + dialog capability back out as command output (Pi `ctx.mode` /
    // `ctx.hasUI`, extensions/types.ts:311,313). A guest guards terminal-only UI on these, so the
    // pair has to reflect the mode the HOST was actually configured with — which is what
    // `crates/cyrup-ext/tests/guest_host_mode.rs` reads this command's output to prove.
    api.register_command(
        "hostmode",
        CommandDescriptor::new("Report ctx.mode + ctx.hasUI (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let base = ctx.ctx();
            Ok(Some(format!(
                "mode={} has_ui={}",
                base.mode().as_str(),
                base.has_ui()
            )))
        },
    );

    // A command exercising the state-mutation seams (R-08-026; Pi `appendEntry`/`setSessionName`/
    // `setLabel`, agent-session.ts:2265-2279): append a custom entry to the live tree, rename the
    // session, then label the just-appended entry. Proves each no-op FIRES against the real session.
    api.register_command(
        "statedemo",
        CommandDescriptor::new(
            "Append a custom entry, rename the session, label the entry (demo).",
        ),
        |_args: &str, ctx: &crate::CommandCtx| {
            let id = ctx
                .ctx()
                .session()
                .append_entry("demoNote", json!({ "note": "from guest" }))?;
            ctx.ctx().session().set_session_name("renamed-by-guest");
            ctx.ctx().session().set_label(&id, Some("guest-label"));
            // EXT-046: `None` CLEARS (pi `setLabel(entryId, label: string | undefined)`,
            // extensions/types.ts:1314 @v0.83.0) — set then clear, so the demo exercises both
            // directions of the signature that used to be write-only.
            ctx.ctx().session().set_label(&id, None);
            ctx.ctx().session().set_label(&id, Some("guest-label"));
            Ok(Some(format!("appended {id}")))
        },
    );

    // A COMMAND-tier counterpart to the event-tier set_thinking_level call above (parity gap #12):
    // at command tier the deadlock rule permits it, so the host APPLIES it (recorded as a
    // `SetThinkingLevel` control op) and the guest observes `Ok(())`.
    api.register_command(
        "thinkdemo",
        CommandDescriptor::new("Set the thinking level from a command (demo, parity gap #12)."),
        |args: &str, ctx: &crate::CommandCtx| {
            let level = if args.trim().is_empty() {
                "high"
            } else {
                args.trim()
            };
            match ctx.models().set_thinking_level(level) {
                Ok(()) => {
                    ctx.ui().notify(&format!("thinking level set: {level}"));
                    Ok(Some(format!("thinking level set: {level}")))
                }
                Err(e) => {
                    ctx.ui().notify(&format!("thinking level rejected: {e}"));
                    Ok(Some(format!("thinking level rejected: {e}")))
                }
            }
        },
    );

    // GAP-11 command-tier proof: a guest COMMAND calling set_model must still apply after the
    // event-tier gate was removed. Command tier was always permitted (R-08-008); this pins that the
    // fix did not regress it.
    api.register_command(
        "gap11setmodel",
        CommandDescriptor::new("Set the model from a command (GAP-11 command-tier proof)."),
        |args: &str, ctx: &crate::CommandCtx| {
            let target = args.trim();
            match ctx.set_model(target) {
                Ok(()) => {
                    ctx.ui().notify(&format!("model set: {target}"));
                    Ok(Some(format!("model set: {target}")))
                }
                Err(e) => {
                    ctx.ui().notify(&format!("model rejected: {e}"));
                    Ok(Some(format!("model rejected: {e}")))
                }
            }
        },
    );

    // A command exercising the active-tool restriction (Pi `setActiveTools`) + typed fork options.
    api.register_command(
        "planmode",
        CommandDescriptor::new("Restrict the active tools to read-only (demo plan mode)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            ctx.ctx().set_active_tools(&["read"]);
            let active = ctx.ctx().active_tools();
            Ok(Some(format!("active tools: {}", active.join(","))))
        },
    );

    // A command exercising the `withSession` re-binding callback (Pi `ReplacedSessionContext`,
    // sdk gap #3): start a new session and move post-replacement work into the closure, which the host
    // invokes against the re-bound session after the command returns.
    api.register_command(
        "withsessiondemo",
        CommandDescriptor::new("Start a new session and notify on the re-bound session (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            ctx.new_session_with_callback(
                &NewSessionOptions::default(),
                |rsc: &ReplacedSessionContext| {
                    rsc.ui()
                        .notify("withSession ran on the replacement session");
                    Ok(())
                },
            )?;
            Ok(Some("new session scheduled".into()))
        },
    );
}
