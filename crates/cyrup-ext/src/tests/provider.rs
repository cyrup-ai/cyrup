//! Custom-provider registration fidelity (arch-08 §5.6; A-08-7). Exercises API-key resolution
//! (literal / `$ENV`/`${ENV}` / `!command`) and the [`ProviderHub`] defer→bind→flush lifecycle (Pi
//! `registerProvider`/`bindCore`), including post-bind immediate upsert and `unregisterProvider`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::provider::{ModelRegistrySink, ProviderHub, ProviderRegistration, resolve_api_key};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct FakeSink {
    upserts: Mutex<Vec<String>>,
    removes: Mutex<Vec<String>>,
}
impl ModelRegistrySink for FakeSink {
    fn upsert_provider(&self, reg: &ProviderRegistration) {
        self.upserts.lock().unwrap().push(reg.id.clone());
    }
    fn remove_provider(&self, id: &str) {
        self.removes.lock().unwrap().push(id.to_string());
    }
}

#[test]
fn api_key_resolution_literal_env_command() {
    // literal
    assert_eq!(
        resolve_api_key(Some("sk-literal")).unwrap(),
        Some("sk-literal".to_string())
    );
    // absent
    assert_eq!(resolve_api_key(None).unwrap(), None);
    assert_eq!(resolve_api_key(Some("")).unwrap(), None);
    // env interpolation against a var that is reliably present in the test environment.
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
        assert_eq!(resolve_api_key(Some("$HOME")).unwrap(), Some(home.clone()));
        assert_eq!(
            resolve_api_key(Some("pre-${HOME}-post")).unwrap(),
            Some(format!("pre-{home}-post"))
        );
    }
    // unknown var expands to empty (Pi behavior).
    assert_eq!(
        resolve_api_key(Some("$CYRUP_DEFINITELY_UNSET_VAR_XZ")).unwrap(),
        Some(String::new())
    );
    // `!command`: stdout, trimmed.
    assert_eq!(
        resolve_api_key(Some("!printf secret123")).unwrap(),
        Some("secret123".to_string())
    );
}

/// PROV-001, extension surface: Pi's `ProviderModelConfig.cost` is a full `ModelCost`, tiers
/// included (coding-agent/src/core/extensions/types.ts:1493). A registered long-context model whose
/// tiers were dropped at the seam gets billed at half the real rate above the threshold.
#[test]
fn registered_model_carries_long_context_pricing_tiers_across_the_seam() {
    let mut hub = ProviderHub::new();
    let cfg = json!({
        "name": "Acme",
        "apiKey": "sk-x",
        "api": "openai-completions",
        "baseUrl": "https://acme.example/v1",
        "models": [{
            "id": "acme-long",
            "name": "Acme Long",
            "contextWindow": 1_000_000,
            "maxTokens": 64_000,
            "cost": {
                "input": 2.5,
                "output": 15.0,
                "cacheRead": 0.25,
                "cacheWrite": 0.0,
                "tiers": [{
                    "inputTokensAbove": 272_000,
                    "input": 5.0,
                    "output": 22.5,
                    "cacheRead": 0.5,
                    "cacheWrite": 0.0
                }]
            }
        }]
    });
    hub.register("acme".into(), &cfg).unwrap();
    let reg = hub.get("acme").expect("registration stored");

    let models = reg.build_models();
    assert_eq!(models.len(), 1);
    let cost = &models[0].cost;
    let tiers = cost.tiers.as_ref().expect("tiers survived the seam");
    assert_eq!(tiers.len(), 1);
    assert_eq!(tiers[0].input_tokens_above, 272_000);

    // Observable consequence: a 300k-token request bills at the tier rate, not the base rate.
    let mut usage = cyrup_core::Usage {
        input: 300_000,
        ..Default::default()
    };
    cyrup_provider::apply_cost(cost, &mut usage);
    assert!(
        (usage.cost.input - 1.5).abs() < 1e-9,
        "long-context input cost was {} (base-rate billing is 0.75)",
        usage.cost.input
    );
}

#[test]
fn provider_hub_defers_until_bind_then_flushes() {
    let mut hub = ProviderHub::new();
    let cfg = json!({
        "name": "Acme",
        "apiKey": "sk-x",
        "api": "openai",
        "models": [{ "id": "m1", "name": "M1", "contextWindow": 1000, "maxOutputTokens": 100 }]
    });
    hub.register("acme".into(), &cfg).unwrap();

    // Before bind: queued, not flushed; the typed config + resolved key are stored.
    assert!(!hub.is_bound());
    assert_eq!(hub.pending_ids(), ["acme".to_string()]);
    let reg = hub.get("acme").expect("registration stored");
    assert_eq!(reg.resolved_api_key.as_deref(), Some("sk-x"));
    assert_eq!(reg.config.api.as_deref(), Some("openai"));
    assert_eq!(reg.config.models.len(), 1);
    assert_eq!(reg.config.models[0].id, "m1");

    // Bind: the pending registration flushes into the sink (Pi bindCore).
    let sink = Arc::new(FakeSink::default());
    hub.bind(sink.clone());
    assert!(hub.is_bound());
    assert!(hub.pending_ids().is_empty());
    assert_eq!(
        sink.upserts.lock().unwrap().clone(),
        vec!["acme".to_string()]
    );

    // Post-bind registration upserts immediately (no queue).
    hub.register("beta".into(), &json!({ "name": "Beta" }))
        .unwrap();
    assert_eq!(sink.upserts.lock().unwrap().len(), 2);
    assert!(hub.pending_ids().is_empty());

    // unregister notifies the sink + drops the registration (Pi unregisterProvider).
    assert!(hub.unregister("acme"));
    assert_eq!(
        sink.removes.lock().unwrap().clone(),
        vec!["acme".to_string()]
    );
    assert!(hub.get("acme").is_none());
    assert!(!hub.unregister("acme"), "second unregister is a no-op");
}

/// EXT-051 — pi's `oauth` block gained `isSubscription?: boolean` at
/// `pi/packages/coding-agent/src/core/extensions/types.ts:1475` @v0.84.1 ("Whether access through
/// this auth method is backed by a provider subscription"); it is ABSENT at the v0.83.0 baseline.
/// The value already crossed the seam inside the untyped `oauth` blob — what was missing was the
/// typed read, so an extension-supplied subscription provider was indistinguishable from a metered
/// API-key one on the host side.
#[test]
fn ext051_oauth_is_subscription_is_readable_on_a_guest_provider() {
    let mut hub = ProviderHub::new();
    hub.register(
        "sub".into(),
        &json!({ "name": "Sub", "oauth": { "name": "Sub Login", "isSubscription": true } }),
    )
    .unwrap();
    hub.register(
        "metered".into(),
        &json!({ "name": "Metered", "oauth": { "name": "Metered Login" } }),
    )
    .unwrap();
    hub.register(
        "keyed".into(),
        &json!({ "name": "Keyed", "apiKey": "sk-x" }),
    )
    .unwrap();

    let sub = hub.get("sub").expect("registered");
    assert!(sub.has_oauth());
    assert!(
        sub.oauth_is_subscription(),
        "a declared isSubscription must reach the host typed"
    );

    let metered = hub.get("metered").expect("registered");
    assert!(metered.has_oauth());
    assert!(
        !metered.oauth_is_subscription(),
        "an OMITTED optional reads false — upstream's `isSubscription?`, not a tri-state"
    );

    let keyed = hub.get("keyed").expect("registered");
    assert!(!keyed.has_oauth());
    assert!(
        !keyed.oauth_is_subscription(),
        "no oauth block at all is not a subscription"
    );
}
