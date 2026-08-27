//! The demo extension's `demo-oauth` PROVIDER — its OAuth flow, its custom `streamSimple`, and its
//! model config — plus the global autocomplete provider that stacks a `demo:run` item on top of
//! whatever the wrapped provider produced.

use crate::{
    AutocompleteItem, AutocompleteSuggestions, ExtensionApi, OAuthProvider, ProviderConfig,
    ProviderHandlers,
};
use serde_json::{json, Value};

pub(super) fn install(api: &mut ExtensionApi) {
    // A custom provider with OAuth + a custom `streamSimple` (Pi `registerProvider({oauth, streamSimple})`,
    // the `custom-provider-*` examples). The `login` flow drives the host `oauth` callbacks; the
    // stream pushes assistant-message events back across the `provider-stream` import.
    let oauth = OAuthProvider::new(
        "Demo SSO",
        |callbacks: &crate::OAuthCallbacks| {
            // Drive the interactive login (Pi onAuth + onPrompt).
            callbacks.on_auth("https://demo.example/oauth/authorize?x=1", None);
            let code = callbacks.on_prompt("Paste the callback code:", None, false)?;
            Ok(json!({ "refresh": "r-demo", "access": format!("a-{code}"), "expires": 0 }))
        },
        |creds: Value| {
            // Refresh: rotate the access token (Pi refreshToken).
            let refresh = creds.get("refresh").and_then(|v| v.as_str()).unwrap_or("r-demo");
            Ok(json!({ "refresh": refresh, "access": "a-refreshed", "expires": 0 }))
        },
        |creds: &Value| {
            // getApiKey: derive the key string from the credentials.
            Ok(creds.get("access").and_then(|v| v.as_str()).unwrap_or_default().to_string())
        },
    )
    .with_modify_models(|models: Value, _creds: &Value| Ok(models));

    let stream = |model: Value, _ctx: Value, _opts: Value, out: &crate::ProviderStream| {
        // Push two assistant-message events then end (Pi createAssistantMessageEventStream).
        let id = model.get("id").and_then(|v| v.as_str()).unwrap_or("demo-model").to_string();
        out.emit(json!({ "type": "text", "text": format!("stream from {id}") }));
        out.emit(json!({ "type": "done" }));
        Ok(())
    };

    api.register_provider_with_handlers(
        "demo-oauth",
        ProviderConfig {
            name: "demo-oauth".into(),
            base_url: Some("https://demo.example".into()),
            api: Some("anthropic".into()),
            api_key: None,
            auth_header: None,
            headers: Default::default(),
            models: vec![crate::ProviderModelConfig {
                id: "demo-model".into(),
                name: Some("Demo Model".into()),
                // Full Pi model shape (sdk gap #26): reasoning/input modalities/cost/contextWindow/maxTokens.
                reasoning: true,
                input: vec!["text".into(), "image".into()],
                cost: crate::ModelCost {
                    input: 3.0,
                    output: 15.0,
                    cache_read: 0.3,
                    cache_write: 3.75,
                    tiers: None,
                },
                context_window: Some(200000),
                max_tokens: Some(8192),
                ..Default::default()
            }],
            oauth: None,
            has_stream_simple: false,
        },
        ProviderHandlers::new().with_oauth(oauth).with_stream_simple(stream),
    );

    // A global autocomplete provider (Pi `addAutocompleteProvider`): stack a "demo:" item on top of
    // whatever the wrapped ("current") provider produced.
    api.add_autocomplete_provider(
        |query: &crate::AutocompleteQuery, current: Option<&AutocompleteSuggestions>| {
            let mut items = current.map(|c| c.items.clone()).unwrap_or_default();
            items.push(AutocompleteItem::labelled("demo:run", "demo:run (extension)"));
            Some(AutocompleteSuggestions { items, prefix: query.current_line().to_string() })
        },
    );
}
