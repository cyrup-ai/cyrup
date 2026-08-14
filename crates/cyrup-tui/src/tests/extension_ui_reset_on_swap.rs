//! A session swap must tear down EXTENSION-owned UI surfaces, not just session-owned ones.
//!
//! pi does this via `resetExtensionUI` (`interactive-mode.ts:1974-2003`), registered on the runtime
//! as `setBeforeSessionInvalidate` (`:452`). cyrup's `rebind_session` reset the session-owned
//! surfaces — transcript, selector, overlays, status flags — and left the extension header, footer,
//! widget, status rows and shortcut bindings in place, so surfaces owned by the OUTGOING session's
//! extensions kept rendering under the new session, attached to a host that no longer exists.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_session_svc::UiEffect;
use crate::{App, UiTheme};
use ratatui::backend::TestBackend;

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::default()).expect("app")
}

/// Every extension-owned surface is cleared by a swap.
#[tokio::test]
async fn a_session_swap_clears_extension_owned_surfaces() {
    let mut app = app();

    // Drive the REAL effect path an extension uses, not the fields directly.
    app.apply_ui_effect(UiEffect::SetHeader { content: "ext header".to_string() });
    app.apply_ui_effect(UiEffect::SetFooter { content: "ext footer".to_string() });
    app.apply_ui_effect(UiEffect::SetWidget { widget: serde_json::json!({"kind": "widget"}) });
    app.apply_ui_effect(UiEffect::SetStatus { key: "ext".to_string(), text: Some("busy".to_string()) });
    app.set_extension_shortcuts(["ctrl+g".to_string()]);

    app.rebind_session();

    let st = app.state();
    assert_eq!(st.extension_header, None, "header");
    assert_eq!(st.extension_footer, None, "footer");
    assert_eq!(st.extension_widget, None, "widget");
    assert!(st.extension_shortcuts.is_empty(), "shortcuts");
    assert!(st.status.extension_statuses.is_empty(), "status rows");
}

/// MIRROR: the session-owned resets `rebind_session` already did must keep working — this stays
/// green through a revert of the extension reset, so a failure above is the missing teardown and
/// not a broken fixture.
#[tokio::test]
async fn a_session_swap_still_resets_the_session_owned_surfaces() {
    let mut app = app();
    app.state_mut().status.set_streaming(true);
    app.state_mut().status.set_queued(3);

    app.rebind_session();

    assert!(!app.state().status.streaming, "streaming flag cleared");
    assert_eq!(app.state().status.queued, 0, "queue depth cleared");
}
