//! EXT-048 — the dialog-timeout wire key is pi's `timeout`, not `timeoutMs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::descriptor::DialogOptions;

/// BEFORE: `DialogOptions.timeout_ms` serialized as `timeoutMs` with no alias, and two in-tree
/// comments (`world.wit:312-313` in both copies, `cyrup-session-svc/src/host_services.rs:104`)
/// cited `types.ts:89` as the source of that name. Upstream is
/// `export interface ExtensionUIDialogOptions { signal?: AbortSignal; timeout?: number; }` at
/// `pi/packages/coding-agent/src/core/extensions/types.ts:95-100` @v0.83.0, with `timeout?: number`
/// on `:100` documented "Timeout in milliseconds. Dialog auto-dismisses with live countdown
/// display" — and `:89` is a blank line before the UI Context banner.
/// `git grep -n timeoutMs v0.83.0 -- packages/coding-agent/src` returns only unrelated startup-ui /
/// http-dispatcher / package-manager hits, so no wire variant spelled `timeoutMs` exists upstream.
///
/// This mattered the moment anything other than cyrup's own SDK wrote the bag — a hand-written
/// guest, a `custom()` overlay spec, or an RPC-side dialog forwarder — because `{"timeout": 5000}`
/// was silently ignored and the dialog got the fallback ceiling instead of the author's bound.
#[test]
fn a_dialog_options_bag_accepts_pis_timeout_key_and_the_legacy_timeout_ms_alias() {
    let upstream: DialogOptions = serde_json::from_str(r#"{"timeout": 5000}"#).unwrap();
    assert_eq!(
        upstream.timeout_ms,
        Some(5000),
        "pi's own key must be honoured"
    );

    let legacy: DialogOptions = serde_json::from_str(r#"{"timeoutMs": 5000}"#).unwrap();
    assert_eq!(
        legacy.timeout_ms,
        Some(5000),
        "bags cyrup's SDK already wrote keep working"
    );

    // The CANONICAL name on the way out is pi's, so a bag cyrup produces is one pi would accept.
    let wire = serde_json::to_value(DialogOptions::timeout(5000)).unwrap();
    assert_eq!(wire.get("timeout"), Some(&serde_json::json!(5000)));
    assert!(
        wire.get("timeoutMs").is_none(),
        "we no longer EMIT the invented key: {wire}"
    );

    // `signal` is the other member of upstream's two-field bag and is unaffected.
    let both: DialogOptions =
        serde_json::from_str(r#"{"timeout": 10, "signalId": "abc"}"#).unwrap();
    assert_eq!(both.timeout_ms, Some(10));
    assert_eq!(both.signal_id.as_deref(), Some("abc"));
}
