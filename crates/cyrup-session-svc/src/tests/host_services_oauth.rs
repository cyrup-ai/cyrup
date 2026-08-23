//! The provider OAuth login callbacks (pi `onPrompt`/`onSelect`) round-tripping through the live
//! dialog renderer, `allowEmpty` included — plus `getCommands()` passing the live catalog through
//! unchanged.
//!
//! One of the five files the inline `mod tests` in `host_services.rs` became when that file was
//! split into `src/host_services/`; this is the section its `provider OAuth login callbacks (pi)`
//! banner opened. Shares [`super::host_services_core::svc_with`] with its siblings.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};

use cyrup_ext::host::HostServices;
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use serde_json::{json, Value};

use crate::host_services::{UiKind, UiReply, UiRequest};

use super::host_services_core::svc_with;
use super::host_services_introspection::FakeCatalog;

/// `oauth_prompt`/`oauth_select` must reach the live dialog renderer.
///
/// pi wires them to the real interaction — `onPrompt: (prompt) => callbacks.prompt({type:
/// "text", ...prompt})` and `onSelect: (prompt) => callbacks.prompt({type: "select", ...prompt})`
/// (`core/provider-composer.ts:245,248`) against `AuthInteraction.prompt(): Promise<string>`,
/// "returns the entered/selected string (`select` returns the option id)"
/// (`packages/ai/src/auth/types.ts:152-161`). cyrup's production backend overrode neither, so a
/// guest-authored provider's interactive `login` could never obtain a value: every prompt came
/// back "oauth prompt capability not granted" from a capability nothing in the workspace grants,
/// and every select came back `None` (which a guest reads as the user cancelling).
///
/// RED before the fix on both round-trip assertions. The headless assertions pass either way and
/// pin that an unattached renderer still yields pi's `noOpUIContext` denial WITHOUT blocking.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_prompt_and_select_round_trip_through_the_ui_sink() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = Arc::new(svc_with(provider));

    // Headless (no renderer): the deny defaults, without blocking — pi's `noOpUIContext`.
    assert!(svc.oauth_prompt("paste the callback url", None, false).is_err());
    assert_eq!(svc.oauth_select("pick an account", &json!([])), None);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    svc.set_ui_sink(tx);
    // The scripted renderer answers exactly as the TUI's `UiKind::Input`/`UiKind::Select` arms
    // do: a typed string, or the chosen option STRING out of `options`.
    let seen: Arc<Mutex<Vec<(UiKind, String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            crate::sync::lock(&seen2)
                .push((req.kind, req.prompt.clone(), req.options.clone()));
            let reply = match req.kind {
                UiKind::Input => UiReply::Text(Some("pasted-code".to_string())),
                // Pick the SECOND row, so the id mapped back cannot be an accident of ordering.
                UiKind::Select => UiReply::Text(
                    req.options.as_array().and_then(|a| a.get(1)).and_then(Value::as_str).map(str::to_string),
                ),
                _ => UiReply::Confirm(false),
            };
            let _ = req.reply.send(reply);
        }
    });

    let s1 = svc.clone();
    let prompted = tokio::task::spawn_blocking(move || {
        s1.oauth_prompt("paste the callback url", Some("https://…"), false)
    })
    .await
    .expect("oauth prompt task");
    assert_eq!(
        prompted.as_deref(),
        Ok("pasted-code"),
        "pi's `prompt()` resolves with the entered string, not a capability denial"
    );

    let s2 = svc.clone();
    let picked = tokio::task::spawn_blocking(move || {
        s2.oauth_select(
            "pick an account",
            &json!([
                {"id": "acct-1", "label": "Personal"},
                {"id": "acct-2", "label": "Work"},
            ]),
        )
    })
    .await
    .expect("oauth select task");
    assert_eq!(
        picked.as_deref(),
        Some("acct-2"),
        "`select` returns the option ID (auth/types.ts:157), mapped back from the label the \
         renderer displayed"
    );

    // The renderer saw the OAuth selector's LABELS, not raw `{id,label}` objects it cannot
    // render (the `UiRequest.options` contract is a flat array of option strings).
    let seen = crate::sync::lock(&seen).clone();
    let select = seen.iter().find(|(k, _, _)| *k == UiKind::Select).expect("a select request");
    assert_eq!(select.2, json!(["Personal", "Work"]), "labels are what reach the renderer");
    assert!(
        seen.iter().any(|(k, p, _)| *k == UiKind::Input && p == "paste the callback url"),
        "the prompt message rides the dialog title, like every other kind: {seen:?}"
    );
}

/// `allow_empty: false` is the guest declaring the value mandatory (`world.wit:871`), so an
/// empty submission is not an answer — pi's prompt rejects rather than resolving with `""`.
/// With `allow_empty: true` the same submission IS the answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_prompt_honours_allow_empty() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = Arc::new(svc_with(provider));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    svc.set_ui_sink(tx);
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let _ = req.reply.send(UiReply::Text(Some(String::new())));
        }
    });

    let s1 = svc.clone();
    let strict = tokio::task::spawn_blocking(move || s1.oauth_prompt("token?", None, false))
        .await
        .expect("task");
    assert!(strict.is_err(), "a mandatory prompt does not resolve with an empty value");

    let s2 = svc.clone();
    let lenient = tokio::task::spawn_blocking(move || s2.oauth_prompt("token?", None, true))
        .await
        .expect("task");
    assert_eq!(lenient.as_deref(), Ok(""), "allow-empty accepts the empty submission");
}

/// EXT-037 — the override must (a) answer `None` when no catalog is attached, so the cyrup-ext
/// binding's registry fallback stays reachable, and (b) pass the live catalog's rows through
/// UNCHANGED — same order, same `name:N` invocation spelling, same descriptions.
///
/// Scope note: that pass-through is the whole contract of this override. That the rows THEMSELVES
/// are pi's `[...extensionCommands, ...templates, ...skills]` is `slash_command_catalog`'s
/// contract, asserted against a real session in `crate::tests::post_run_loop` and
/// `crate::tests::install_noop`; the double here stands in for it because a `LiveHostServices`
/// unit test cannot build an `AgentSession`.
#[test]
fn commands_passes_the_live_catalog_through_unchanged() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider);

    // Unattached: `None` ⇒ the cyrup-ext binding falls back to the registry's resolved commands.
    assert!(svc.commands().is_none(), "no catalog attached ⇒ no live answer");

    svc.attach_session_catalog(Arc::new(FakeCatalog));
    let rows = svc.commands().expect("an attached catalog answers");
    let names: Vec<&str> = rows.iter().filter_map(|r| r["name"].as_str()).collect();
    assert_eq!(
        names,
        ["deploy", "deploy:2", "review", "skill:pdf"],
        "extension commands (with the `name:N` collision spelling), then templates, then skills"
    );
    let sources: Vec<&str> = rows.iter().filter_map(|r| r["source"].as_str()).collect();
    assert_eq!(sources, ["extension", "extension", "prompt", "skill"]);
    assert!(
        rows.iter().all(|r| !r["description"].as_str().unwrap_or_default().is_empty()),
        "every row carries a description — the bare-name walk carried none"
    );
}

