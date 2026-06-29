//! Custom-provider registration host-side (arch-08 §5.6; Pi `registerProvider`, types.ts:1337/1363;
//! `runner.ts:344` bindCore). A guest sends a typed [`ProviderConfig`] across the seam; the host
//! parses it, resolves the API key (literal / `$ENV`/`${ENV}` interpolation / leading `!command`,
//! Pi's resolution rules), and routes it to the [`ModelRegistrySink`] the session injects. Until the
//! sink is bound, registrations QUEUE; [`ProviderHub::bind`] flushes the pending set (Pi's
//! defer→`bindCore` lifecycle, A-08-7). OAuth + `streamSimple` cross as opaque blocks (executing
//! those guest callbacks needs guest exports — see gap-08 #5: a documented partial).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A custom LLM provider configuration (host mirror of the SDK `ProviderConfig`, Pi types.ts:1363).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    /// Literal, `$ENV`/`${ENV}` interpolation, or leading `!command` (resolved by [`resolve_api_key`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub models: Vec<ProviderModelConfig>,
    /// OAuth metadata (`{name}`): a static marker that this provider authenticates via OAuth. The
    /// dynamic callbacks (login/refreshToken/getApiKey/modifyModels) are guest closures invoked via
    /// the `provider-*` exports ([`crate::host::LiveExtension::provider_login`], etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<Value>,
    /// Whether the guest supplied a custom `streamSimple` handler (drives the host to invoke the
    /// `provider-stream-simple` export rather than a built-in API stream).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_stream_simple: bool,
}

/// Per-model config inside a [`ProviderConfig`] (Pi `ProviderModelConfig`, types.ts:1396).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

/// A resolved provider registration: the parsed config, the resolved API key (if any), and whether
/// it carries an OAuth block (whose dynamic callbacks need guest exports — gap-08 #5).
#[derive(Clone, Debug)]
pub struct ProviderRegistration {
    pub id: String,
    pub config: ProviderConfig,
    pub resolved_api_key: Option<String>,
}

impl ProviderRegistration {
    /// Whether this provider authenticates via OAuth (vs. a static/resolved API key). The dynamic
    /// `login`/`refreshToken`/`getApiKey`/`modifyModels` callbacks are invoked across the
    /// `provider-*` exports (the host drives `/login` through [`crate::host::LiveExtension`]).
    pub fn has_oauth(&self) -> bool {
        self.config.oauth.is_some()
    }

    /// The OAuth display name (Pi `oauth.name`), if this provider carries an OAuth block.
    pub fn oauth_name(&self) -> Option<String> {
        self.config
            .oauth
            .as_ref()
            .and_then(|v| v.get("name"))
            .and_then(|n| n.as_str())
            .map(str::to_string)
    }

    /// Whether the guest supplied a custom `streamSimple` handler (drives `provider-stream-simple`).
    pub fn has_stream_simple(&self) -> bool {
        self.config.has_stream_simple
    }
}

/// Resolve an API-key spec per Pi's rules (types.ts apiKey doc): a leading `!` runs a shell command
/// and uses its trimmed stdout; `$VAR`/`${VAR}` interpolate the environment; otherwise it is literal.
/// Returns `Ok(None)` for an absent spec. A failed `!command` surfaces an error (never a panic).
pub fn resolve_api_key(spec: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = spec else { return Ok(None) };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    if let Some(cmd) = raw.strip_prefix('!') {
        // Run via the platform shell; trim the trailing newline (Pi `!command`).
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .map_err(|e| format!("apiKey command failed to spawn: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("apiKey command exited non-zero: {}", stderr.trim()));
        }
        let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Ok(Some(key));
    }
    Ok(Some(interpolate_env(raw)))
}

/// Interpolate `$VAR` and `${VAR}` from the environment; unknown vars expand to empty (Pi behavior).
fn interpolate_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        // `${VAR}` form.
        if chars.peek() == Some(&'{') {
            chars.next();
            let mut name = String::new();
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
                name.push(c);
            }
            out.push_str(&std::env::var(&name).unwrap_or_default());
            continue;
        }
        // `$VAR` form (alnum + underscore).
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_alphanumeric() || c == '_' {
                name.push(c);
                chars.next();
            } else {
                break;
            }
        }
        if name.is_empty() {
            out.push('$');
        } else {
            out.push_str(&std::env::var(&name).unwrap_or_default());
        }
    }
    out
}

/// The model-registry the session injects (Pi `bindCore`). The hub upserts/removes providers here;
/// the concrete impl lives in cyrup-provider/cyrup-session and is wired at runtime (arch-08 §5.6).
pub trait ModelRegistrySink: Send + Sync {
    /// Register (or replace) a provider's models (Pi `registerProvider` → ModelRegistry).
    fn upsert_provider(&self, reg: &ProviderRegistration);
    /// Remove a provider's models, restoring any built-ins it overrode (Pi `unregisterProvider`).
    fn remove_provider(&self, id: &str);
}

/// The provider registration hub (arch-08 §5.6). Holds resolved registrations and an optional
/// injected [`ModelRegistrySink`]; registrations made before `bind` are queued and flushed at bind.
#[derive(Default)]
pub struct ProviderHub {
    registrations: Vec<ProviderRegistration>,
    /// Ids registered before a sink was bound — flushed in order at [`Self::bind`].
    pending: Vec<String>,
    sink: Option<Arc<dyn ModelRegistrySink>>,
}

impl ProviderHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a provider from the guest's JSON config: parse → resolve api key → store, and either
    /// upsert into the bound sink or queue for the next [`Self::bind`] (Pi defer→bindCore).
    pub fn register(&mut self, id: String, config_json: &Value) -> Result<(), String> {
        let mut config: ProviderConfig =
            serde_json::from_value(config_json.clone()).map_err(|e| e.to_string())?;
        if config.name.is_empty() {
            config.name = id.clone();
        }
        let resolved_api_key = resolve_api_key(config.api_key.as_deref())?;
        let reg = ProviderRegistration { id: id.clone(), config, resolved_api_key };

        // Replace an existing registration with the same id (Pi "replaces all models").
        self.registrations.retain(|r| r.id != id);
        self.registrations.push(reg.clone());

        match &self.sink {
            Some(sink) => sink.upsert_provider(&reg),
            None => {
                self.pending.retain(|p| p != &id);
                self.pending.push(id);
            }
        }
        Ok(())
    }

    /// Unregister a provider (Pi `unregisterProvider`): drop it + notify the sink. Returns whether it
    /// was present.
    pub fn unregister(&mut self, id: &str) -> bool {
        let had = self.registrations.iter().any(|r| r.id == id);
        self.registrations.retain(|r| r.id != id);
        self.pending.retain(|p| p != id);
        if had && let Some(sink) = &self.sink {
            sink.remove_provider(id);
        }
        had
    }

    /// Bind the model-registry sink and FLUSH the pending registrations into it (Pi `bindCore`).
    pub fn bind(&mut self, sink: Arc<dyn ModelRegistrySink>) {
        for id in std::mem::take(&mut self.pending) {
            if let Some(reg) = self.registrations.iter().find(|r| r.id == id) {
                sink.upsert_provider(reg);
            }
        }
        self.sink = Some(sink);
    }

    /// Whether the registry sink has been bound (Pi post-`bindCore`).
    pub fn is_bound(&self) -> bool {
        self.sink.is_some()
    }

    /// Ids still queued for the next bind (not yet flushed).
    pub fn pending_ids(&self) -> &[String] {
        &self.pending
    }

    /// All registered provider ids (Pi `getRegisteredProviders`).
    pub fn ids(&self) -> Vec<String> {
        self.registrations.iter().map(|r| r.id.clone()).collect()
    }

    /// Look up a resolved registration by id.
    pub fn get(&self, id: &str) -> Option<&ProviderRegistration> {
        self.registrations.iter().find(|r| r.id == id)
    }
}
