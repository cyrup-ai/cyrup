//! The demo extension's UI commands — the `ui.*` dialog surface (`confirm`, `input`, `select`,
//! `editor`, and the programmatic dismiss signal) — plus the `ctrl+t` shortcut, whose handler opens
//! a dialog of its own.

use crate::{CommandDescriptor, DialogOptions, ExtensionApi};

pub(super) fn install(api: &mut ExtensionApi) {
    // A command exercising the programmatic dialog-dismiss signal (Pi `ExtensionUIDialogOptions.signal`,
    // sdk gap #2): abort the named signal, then a dialog bound to it returns cancelled (here `confirm`
    // -> false) even though the backend's canned answer is `true`.
    api.register_command(
        "signaldemo",
        CommandDescriptor::new("Dismiss a dialog via a named signal, then confirm (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            ctx.ui().abort_signal("demo-dialog");
            let ok = ctx.ui().confirm_with(
                "proceed?",
                "unreachable body",
                &DialogOptions::signal("demo-dialog"),
            );
            Ok(Some(format!("confirmed: {ok}")))
        },
    );

    // A command exercising `confirm`'s `message` body (Pi `confirm(title, message, opts)`,
    // rpc-types.ts:240 @v0.83.0; L4 review §2.6): threads live through `ctx/ui.rs` -> WIT `confirm` -> the host
    // backend, distinct from the prompt/title (not dismissed, so the backend actually sees it).
    api.register_command(
        "confirmdemo",
        CommandDescriptor::new("Open a confirm dialog with a message body (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let ok = ctx.ui().confirm_with(
                "proceed?",
                "this is the message body, distinct from the title",
                &DialogOptions::default(),
            );
            ctx.ui().notify(&format!("confirmed: {ok}"));
            // Visible LIVE proof the guest received the REAL answer, not just that the dialog closed:
            // a second dialog whose PROMPT embeds the value just received — no dependency on `notify`/
            // `append_entry`'s host-side-only recording (a command handler's plain return value is
            // itself discarded too, Pi-faithfully, `session.rs` `try_execute_wasm_command`).
            let _ = ctx.ui().confirm(&format!("you answered: {ok}"));
            Ok(Some(format!("confirmed: {ok}")))
        },
    );

    // A command exercising `input`'s `placeholder` (Pi `input(title, placeholder, opts)`,
    // rpc-types.ts:241-248 @v0.83.0; L4 review §2.7): threads live through `ctx/ui.rs` -> WIT `input` -> the host
    // backend, distinct from the prompt/title.
    api.register_command(
        "inputdemo",
        CommandDescriptor::new("Open an input dialog with a placeholder (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let answer = ctx.ui().input_with(
                "name?",
                Some("e.g. Ada Lovelace"),
                &DialogOptions::default(),
            );
            ctx.ui().notify(&format!("input: {answer:?}"));
            // Visible LIVE proof (see `confirmdemo`'s comment): echo the received text in a follow-up
            // dialog's prompt.
            let _ = ctx.ui().confirm(&format!("you typed: {answer:?}"));
            Ok(Some(format!("input: {answer:?}")))
        },
    );

    // A command exercising `select` (Pi `select(title, options, opts): Promise<string|undefined>`,
    // types.ts:133 @v0.83.0; L4 review §2.1 interactive-TUI wiring): offers three options and surfaces the
    // chosen STRING (or "none" if dismissed).
    api.register_command(
        "selectdemo",
        CommandDescriptor::new("Open a select dialog over three options (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let chosen = ctx.ui().select("pick one", &["alpha", "beta", "gamma"]);
            let text = format!("selected: {}", chosen.as_deref().unwrap_or("none"));
            ctx.ui().notify(&text);
            // Visible LIVE proof (see `confirmdemo`'s comment): echo the received choice in a follow-up
            // dialog's prompt.
            let _ = ctx.ui().confirm(&format!(
                "you picked: {}",
                chosen.as_deref().unwrap_or("none")
            ));
            Ok(Some(text))
        },
    );

    // A command exercising `editor` (Pi's external-editor dialog; L4 review §2.1): seeds the buffer
    // with fixed text and surfaces the edited result (or "none" if cancelled/non-zero exit).
    api.register_command(
        "editordemo",
        CommandDescriptor::new("Open an external-editor dialog seeded with fixed text (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let edited = ctx.ui().editor("edit demo", "seed text from the guest");
            let text = format!("edited: {}", edited.as_deref().unwrap_or("none"));
            ctx.ui().notify(&text);
            // Visible LIVE proof (see `confirmdemo`'s comment): echo the received text in a follow-up
            // dialog's prompt.
            let _ = ctx.ui().confirm(&text);
            Ok(Some(text))
        },
    );

    // A keyboard shortcut (R-08-017) whose handler itself opens a SYNCHRONOUS `ui.confirm` dialog (L4
    // review §2.1): proves the run loop's `AppAction::ExtensionShortcut` no longer self-deadlocks now
    // that it is spawned rather than awaited inline (a shortcut handler blocking on `ui_roundtrip`
    // while the SAME task also owns `ui_rx` would otherwise hang forever).
    api.register_shortcut(
        "ctrl+t",
        "Open a confirm dialog from a shortcut (demo)",
        |ctx: &crate::Ctx| {
            let ok = ctx.ui().confirm("shortcut confirm — proceed?");
            let text = format!("shortcut confirmed: {ok}");
            ctx.ui().notify(&text);
            // Visible LIVE proof (see `confirmdemo`'s comment) — also proves TWO SEQUENTIAL synchronous
            // `ui.*` round trips from the SAME spawned shortcut task both reach the run loop correctly.
            let _ = ctx.ui().confirm(&text);
            Ok(())
        },
    );
}
