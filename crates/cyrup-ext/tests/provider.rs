//! Custom-provider registration fidelity (arch-08 §5.6; A-08-7). Exercises API-key resolution
//! (literal / `$ENV`/`${ENV}` / `!command`) and the [`ProviderHub`] defer→bind→flush lifecycle (Pi
//! `registerProvider`/`bindCore`), including post-bind immediate upsert and `unregisterProvider`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_ext::provider::{resolve_api_key, ModelRegistrySink, ProviderHub, ProviderRegistration};
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
    assert_eq!(resolve_api_key(Some("sk-literal")).unwrap(), Some("sk-literal".to_string()));
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
    assert_eq!(resolve_api_key(Some("$CYRUP_DEFINITELY_UNSET_VAR_XZ")).unwrap(), Some(String::new()));
    // `!command`: stdout, trimmed.
    assert_eq!(resolve_api_key(Some("!printf secret123")).unwrap(), Some("secret123".to_string()));
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
    assert_eq!(sink.upserts.lock().unwrap().clone(), vec!["acme".to_string()]);

    // Post-bind registration upserts immediately (no queue).
    hub.register("beta".into(), &json!({ "name": "Beta" })).unwrap();
    assert_eq!(sink.upserts.lock().unwrap().len(), 2);
    assert!(hub.pending_ids().is_empty());

    // unregister notifies the sink + drops the registration (Pi unregisterProvider).
    assert!(hub.unregister("acme"));
    assert_eq!(sink.removes.lock().unwrap().clone(), vec!["acme".to_string()]);
    assert!(hub.get("acme").is_none());
    assert!(!hub.unregister("acme"), "second unregister is a no-op");
}
