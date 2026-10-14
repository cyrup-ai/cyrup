//! The demo's LIVE widget (SUBA-210, SUBA-211): a two-line card that folds to one line on a plain
//! click on its header row and shows the terminal size it was last drawn for — the guest-tier
//! twin of `pi-subagents`' async-jobs card (`buildWidgetComponent`, `src/tui/render.ts:2911`
//! @ad11b7ab), whose `handleMouse` (`:2938-2944`) folds on a left click at `y === 0` with no
//! shift, alt or ctrl, and whose render reads `process.stdout.rows`/`columns`.
//!
//! `/demo-widget` publishes it; from then on the host routes the widget's events to the handler
//! registered here, which republishes through `ui.set-widget`. The folded state lives in the
//! guest, as pi's lives in its component's closure.

use std::cell::Cell;

use crate::widget::{TerminalSize, WidgetEvent, WidgetEventOutcome};
use crate::{CommandDescriptor, ExtensionApi, Ui};

/// The widget key the demo publishes under.
pub const WIDGET_KEY: &str = "demo-card";
/// The command that publishes it.
pub const WIDGET_COMMAND: &str = "demo-widget";
/// The header row's text, unfolded.
pub const WIDGET_HEADER: &str = "demo card";
/// The one line the folded card shows.
pub const WIDGET_FOLDED: &str = "demo card (folded)";

thread_local! {
    /// pi's `let collapsed = initiallyCollapsed` closure variable.
    static COLLAPSED: Cell<bool> = const { Cell::new(false) };
}

/// The card's lines for `size` — the terminal size the guest was told, or read.
pub fn card_lines(collapsed: bool, size: Option<TerminalSize>) -> Vec<String> {
    if collapsed {
        return vec![WIDGET_FOLDED.to_string()];
    }
    let size = match size {
        Some(size) => format!("terminal {}x{}", size.columns, size.rows),
        None => "terminal unknown".to_string(),
    };
    vec![WIDGET_HEADER.to_string(), size]
}

fn publish(ui: &Ui, size: Option<TerminalSize>) {
    let lines = card_lines(COLLAPSED.with(Cell::get), size);
    ui.set_widget(WIDGET_KEY, &lines, None);
}

pub(super) fn install(api: &mut ExtensionApi) {
    api.register_command(
        WIDGET_COMMAND,
        CommandDescriptor::new("Publish the demo's live widget (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let ui = ctx.ui();
            publish(&ui, ui.terminal_size());
            Ok(None)
        },
    );
    api.on_widget_event(WIDGET_KEY, |event: &WidgetEvent, ctx: &crate::Ctx| {
        let ui = ctx.ui();
        match event {
            // pi `if (event.type !== "click" || event.button !== "left" || event.y !== 0) return
            // undefined; if (event.shift || event.alt || event.ctrl) return undefined;`
            WidgetEvent::Mouse(mouse) if mouse.is_plain_click_on_row(0) => {
                COLLAPSED.with(|c| c.set(!c.get()));
                publish(&ui, ui.terminal_size());
                WidgetEventOutcome::Handled
            }
            WidgetEvent::Mouse(_) => WidgetEventOutcome::Unhandled,
            WidgetEvent::Resize(size) => {
                publish(&ui, Some(*size));
                WidgetEventOutcome::Handled
            }
        }
    });
}
