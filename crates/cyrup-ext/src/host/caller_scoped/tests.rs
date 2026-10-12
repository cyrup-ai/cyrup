//! EXT-105 — a native's services handle answers `flag_value` only for the caller's own flags.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use cyrup_core::ExtensionId;
use serde_json::{Value, json};

use super::CallerScopedServices;
use crate::host::services::{DenyServices, HostServices};
use crate::registry::ExtensionRegistry;

/// A backend that answers every flag name, as the session's does for any registered flag: what a
/// native could read before the gate.
struct AnswersEverything;

impl HostServices for AnswersEverything {
    fn flag_value(&self, name: &str) -> Option<Value> {
        Some(json!(format!("value of {name}")))
    }
}

fn registry() -> Arc<ExtensionRegistry> {
    let registry = Arc::new(ExtensionRegistry::new());
    registry
        .register_flag(
            ExtensionId::from("alpha"),
            "alpha-flag",
            json!({ "type": "string", "default": "a" }),
        )
        .unwrap();
    registry
        .register_flag(
            ExtensionId::from("beta"),
            "beta-flag",
            json!({ "type": "string", "default": "b" }),
        )
        .unwrap();
    registry
}

fn seen_by(caller: &str, inner: Arc<dyn HostServices>) -> CallerScopedServices {
    CallerScopedServices::new(inner, ExtensionId::from(caller), registry())
}

/// Both directions: each extension reads its own flag and not the other's, over a backend that
/// answers every name as the session's does. RED with the gate removed from
/// [`CallerScopedServices::flag_value`]; the production-chain red at the base is
/// `cyrup-session-svc`'s `a_native_does_not_read_a_flag_only_another_extension_declared`.
#[test]
fn each_caller_reads_its_own_flag_and_not_the_others() {
    let alpha = seen_by("alpha", Arc::new(AnswersEverything));
    let beta = seen_by("beta", Arc::new(AnswersEverything));

    // Positive controls: without these, a gate that refused everything would pass.
    assert_eq!(
        alpha.flag_value("alpha-flag"),
        Some(json!("value of alpha-flag"))
    );
    assert_eq!(
        beta.flag_value("beta-flag"),
        Some(json!("value of beta-flag"))
    );

    assert_eq!(alpha.flag_value("beta-flag"), None);
    assert_eq!(beta.flag_value("alpha-flag"), None);
    // A flag nobody declared, and an extension that declared nothing.
    assert_eq!(alpha.flag_value("nobody"), None);
    assert_eq!(
        seen_by("gamma", Arc::new(AnswersEverything)).flag_value("alpha-flag"),
        None
    );
    assert_eq!(alpha.caller().as_str(), "alpha");
}

/// NON-REGRESSION GUARD: the default backend answers `None` for every flag, and the gate changes
/// nothing about that — a caller's own flag over a backend with no registry is still `None`.
#[test]
fn the_default_backend_still_answers_none() {
    assert_eq!(DenyServices.flag_value("alpha-flag"), None);
    assert_eq!(
        seen_by("alpha", Arc::new(DenyServices)).flag_value("alpha-flag"),
        None
    );
}

/// Every method of [`HostServices`] is forwarded by [`CallerScopedServices`]. The trait gives each
/// method a deny default, so a method this impl does not name compiles and silently answers the
/// deny default to every native — the failure this test exists to catch when a verb is added to the
/// trait. GUARD: green as written; red when any trait method is missing from the impl.
#[test]
fn every_trait_method_is_forwarded() {
    fn method_names(block: &str) -> Vec<&str> {
        block
            .lines()
            .filter_map(|line| {
                let line = line.strip_prefix("    ")?;
                let line = line.strip_prefix("async ").unwrap_or(line);
                let rest = line.strip_prefix("fn ")?;
                let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))?;
                rest.get(..end)
            })
            .collect()
    }
    fn block_after<'a>(source: &'a str, opening: &str) -> &'a str {
        let start = source.find(opening).expect(opening);
        let body = source.get(start..).unwrap();
        let end = body.find("\n}\n").expect("the block's closing brace");
        body.get(..end).unwrap()
    }

    let trait_source = include_str!("../services.rs");
    let impl_source = include_str!("../caller_scoped.rs");
    let declared = method_names(block_after(
        trait_source,
        "pub trait HostServices: Send + Sync {",
    ));
    let forwarded = method_names(block_after(
        impl_source,
        "impl HostServices for CallerScopedServices {",
    ));

    assert!(
        declared.len() > 80,
        "the scan found the trait: {}",
        declared.len()
    );
    let missing: Vec<_> = declared
        .iter()
        .filter(|name| !forwarded.contains(name))
        .collect();
    assert!(missing.is_empty(), "not forwarded: {missing:?}");
    assert_eq!(declared.len(), forwarded.len(), "{forwarded:?}");
}
