//! SEAM-S01 — `applyExtensionFlagValues` must REPORT, not swallow.
//!
//! Pi's `applyExtensionFlagValues` (`coding-agent/src/core/agent-session-services.ts:84-125`) has
//! three arms, and two of them produce a `{type: "error"}` diagnostic:
//!
//! * a captured `--flag` no loaded extension registered ⇒ collected and emitted ONCE as
//!   `Unknown option: --foo` / `Unknown options: --foo, --bar` (`:100-103`, `:118-124` — note the
//!   count-driven plural and the `", "` join);
//! * a bare `--flag` against a **string**-typed registered flag ⇒
//!   `Extension flag "--foo" requires a value` (`:113-116`), and the registered default stands.
//!
//! Those diagnostics are merged into `services.diagnostics` (`:182`), become
//! `runtime.diagnostics`, and are reported + `process.exit(1)`-ed at `main.ts:843-848`.
//!
//! cyrup's port had both arms as a bare `continue` (facade.rs, "Unregistered flag: no extension owns
//! it — ignored"), so a mistyped `--flag` produced **no message and exit 0** — the user's typo was
//! silently a no-op. These tests assert the returned diagnostic strings, which is the observable
//! the bin's exit-1 checkpoint keys off.
//!
//! No wasm needed: `ExtensionRegistry::set_flag` is the same store a guest's `registerFlag` writes,
//! so the registered-flag arms can be driven directly against the assembled `ExtensionHost`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_ext::{ExtensionFlagOverride, ExtensionHost, HostConfig};
use serde_json::json;

fn host() -> ExtensionHost {
    ExtensionHost::new(HostConfig::default())
}

/// The headline: a flag nobody registered is an ERROR, not silence.
#[test]
fn an_unregistered_flag_produces_pis_unknown_option_error() {
    let host = host();
    let diags = host
        .apply_extension_flag_values(&[(
            "dangerously-skip".to_string(),
            ExtensionFlagOverride::Bool(true),
        )])
        .expect("apply");
    // Pi `:120-123`: singular form, `--` re-prefixed.
    assert_eq!(diags, vec!["Unknown option: --dangerously-skip".to_string()]);
}

/// Pi aggregates EVERY unknown into ONE pluralized message, joined with `", "`, in capture order —
/// not one diagnostic per flag.
#[test]
fn multiple_unregistered_flags_aggregate_into_one_pluralized_message() {
    let host = host();
    let diags = host
        .apply_extension_flag_values(&[
            ("alpha".to_string(), ExtensionFlagOverride::Str("1".into())),
            ("beta".to_string(), ExtensionFlagOverride::Bool(true)),
        ])
        .expect("apply");
    assert_eq!(diags, vec!["Unknown options: --alpha, --beta".to_string()]);
}

/// A bare `--flag` on a STRING-typed registered flag: Pi's second error class. The registered
/// default must still stand (no value is stored), which is what makes this a diagnostic rather than
/// a silent `true`.
#[test]
fn a_valueless_string_flag_reports_requires_a_value_and_stores_nothing() {
    let host = host();
    host.registry()
        .set_flag("persona", json!({"type": "string", "default": "reviewer"}))
        .expect("register");

    let diags = host
        .apply_extension_flag_values(&[("persona".to_string(), ExtensionFlagOverride::Bool(true))])
        .expect("apply");

    assert_eq!(diags, vec!["Extension flag \"--persona\" requires a value".to_string()]);
    assert_eq!(
        host.registry().flag_value("persona").expect("read"),
        None,
        "the registered default must stand — no CLI value may be stored"
    );
}

/// Pi's ORDER: every per-flag "requires a value" first, in iteration order, then the single
/// aggregated "Unknown option(s)" last (it is pushed after the loop, `:118`).
#[test]
fn requires_a_value_errors_precede_the_aggregated_unknown_option_error() {
    let host = host();
    host.registry().set_flag("persona", json!({"type": "string"})).expect("register");

    let diags = host
        .apply_extension_flag_values(&[
            ("typo".to_string(), ExtensionFlagOverride::Bool(true)),
            ("persona".to_string(), ExtensionFlagOverride::Bool(true)),
        ])
        .expect("apply");

    assert_eq!(
        diags,
        vec![
            "Extension flag \"--persona\" requires a value".to_string(),
            "Unknown option: --typo".to_string(),
        ]
    );
}

/// The success arms are untouched: a registered flag still resolves, and produces NO diagnostic.
/// (Guards against "fix the reporting, break the feature".)
#[test]
fn registered_flags_still_resolve_with_no_diagnostics() {
    let host = host();
    host.registry().set_flag("verbose-ext", json!({"type": "boolean"})).expect("register");
    host.registry().set_flag("persona", json!({"type": "string"})).expect("register");

    let diags = host
        .apply_extension_flag_values(&[
            // Pi `:105-108`: a boolean flag stores `true` REGARDLESS of the captured token value.
            ("verbose-ext".to_string(), ExtensionFlagOverride::Str("whatever".into())),
            ("persona".to_string(), ExtensionFlagOverride::Str("critic".into())),
        ])
        .expect("apply");

    assert!(diags.is_empty(), "clean apply must not diagnose: {diags:?}");
    assert_eq!(host.registry().flag_value("verbose-ext").expect("read"), Some(json!(true)));
    assert_eq!(host.registry().flag_value("persona").expect("read"), Some(json!("critic")));
}
