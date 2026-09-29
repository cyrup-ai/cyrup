//! The demo extension's host WIRING that is neither an event handler nor a plain command surface:
//! the CLI-overridable `demo-flag` (with the `/flagdemo` command that reads it back) and the
//! inter-extension event bus (the `demo:bus` subscription with the `/buspub` command that emits on
//! it).

use crate::{CommandDescriptor, ExtensionApi, FlagSpec};
use serde_json::{Value, json};

pub(super) fn install(api: &mut ExtensionApi) {
    // A CLI-overridable flag (Pi `registerFlag`, loader.ts:257-266) + a command that READS it back
    // via `getFlag` (loader.ts:280-284). Registered with a static default; a `--demo-flag=<value>`
    // token captured on the command line (Pi `unknownFlags` → `applyExtensionFlagValues`,
    // agent-session-services.ts:84-113) must override that default so the value the guest reads here
    // is the CLI-supplied one, not the registered default (gap-08 §5.6).
    api.register_flag(
        "demo-flag",
        FlagSpec {
            r#type: "string".into(),
            default: Some(json!("off")),
            description: String::new(),
        },
    );
    api.register_command(
        "flagdemo",
        CommandDescriptor::new(
            "Report the resolved value of --demo-flag (demo, getFlag override).",
        ),
        |_args: &str, ctx: &crate::CommandCtx| {
            let value = ctx
                .ctx()
                .flag("demo-flag")
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            ctx.ui().notify(&format!("flag demo-flag = {value}"));
            Ok(Some(format!("flag demo-flag = {value}")))
        },
    );

    // Inter-extension event bus (Pi `pi.events.emit`/`pi.events.on`, event-bus.ts:12-32; gap-08 §5.3).
    // `/buspub <msg>` EMITS on the shared bus; the `on_bus` handler below (registered by EVERY loaded
    // instance of this extension) RECEIVES a topic another instance emitted and surfaces it via
    // `notify` — the observable proof a published event reached a subscribed handler cross-extension.
    api.on_bus(
        "demo:bus",
        |topic: &str, payload: Value, ctx: &crate::Ctx| {
            let msg = payload.get("msg").and_then(|v| v.as_str()).unwrap_or("");
            ctx.notify(&format!("bus recv {topic}: {msg}"));
        },
    );
    // EXT-087 — arm the `agent_settled` handler's one-shot `send_user_message`. See that handler
    // in [`super::hooks`] for why it is armed rather than unconditional: this component is the
    // shared fixture behind the whole `cyrup-it` suite.
    api.register_command(
        "armsend",
        CommandDescriptor::new(
            "Arm a one-shot `send_user_message` from the next `agent_settled` handler (demo).",
        ),
        |_args: &str, _ctx: &crate::CommandCtx| {
            ARMED_SETTLE_SEND.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(Some("armed".to_string()))
        },
    );
    api.register_command(
        "buspub",
        CommandDescriptor::new(
            "Emit a message on the inter-extension event bus (demo). args: <msg>",
        ),
        |args: &str, ctx: &crate::CommandCtx| {
            let msg = args.trim();
            ctx.ctx().emit("demo:bus", json!({ "msg": msg }));
            Ok(Some(format!("emitted demo:bus: {msg}")))
        },
    );
}

/// EXT-087 — the one-shot latch behind `/armsend`, read and cleared by the `agent_settled` handler
/// in [`super::hooks`].
///
/// A plain static because the guest is single-threaded inside its component instance and the
/// handler closures are `fn` pointers with no place to hang state; the surrounding SDK keeps its
/// other guest-side registries (`LATE_TOOLS`) the same way.
static ARMED_SETTLE_SEND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Take the `/armsend` latch, clearing it — so an armed send fires on the NEXT settle and only
/// that one.
pub(super) fn take_armed_settle_send() -> bool {
    ARMED_SETTLE_SEND.swap(false, std::sync::atomic::Ordering::SeqCst)
}
