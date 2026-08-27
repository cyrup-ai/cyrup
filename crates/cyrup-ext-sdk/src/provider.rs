//! Provider OAuth + custom-stream guest closures (Pi `ProviderConfig.oauth`/`streamSimple`, on the
//! config `registerProvider(name, config)` takes, types.ts:1401; arch-08 §5.6). The static
//! [`crate::ProviderConfig`] (baseUrl/apiKey/models) crosses the seam as serializable JSON and
//! registers models host-side; the DYNAMIC callbacks
//! (`login`/`refreshToken`/`getApiKey`/`modifyModels` and `streamSimple`) cannot serialize, so they
//! live guest-side in [`ProviderHandlers`] keyed by provider id and are invoked across the boundary
//! via the `provider-*` WIT exports (host gap #1 / sdk gap #1).
//!
//! During `login(callbacks)` the guest drives the interactive flow through [`OAuthCallbacks`] — the
//! safe-Rust front for the `oauth` host imports (onAuth/onPrompt/onSelect/…). `streamSimple` pushes
//! assistant-message events through a [`ProviderStream`] (the `provider-stream` host import).

use serde::Serialize;
use serde_json::Value;

/// OAuth credentials persisted between sessions (Pi `OAuthCredentials`, pi-ai oauth/types.ts):
/// `{refresh, access, expires}` plus an open extras bag. Carried across the seam as JSON.
pub type OAuthCredentials = Value;

/// The interactive callbacks a guest `login` flow invokes (Pi `OAuthLoginCallbacks`). On `wasm32`
/// each method calls the matching `oauth` host import; on the host target it returns inert defaults
/// so the login closure is unit-testable.
#[derive(Clone, Copy, Debug, Default)]
pub struct OAuthCallbacks;

impl OAuthCallbacks {
    /// A callbacks handle. [`OAuthCallbacks`] is a unit struct — every method reaches the `oauth`
    /// host import directly — so this binds nothing.
    pub fn new() -> Self {
        OAuthCallbacks
    }

    /// Show an auth URL the user opens in a browser (Pi `onAuth`).
    pub fn on_auth(&self, url: &str, instructions: Option<&str>) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::oauth::on_auth(url, instructions);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (url, instructions);
    }

    /// Show a device-code flow prompt (Pi `onDeviceCode`).
    pub fn on_device_code(
        &self,
        user_code: &str,
        verification_uri: &str,
        interval_seconds: Option<u32>,
        expires_in_seconds: Option<u32>,
    ) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::oauth::on_device_code(
            user_code,
            verification_uri,
            interval_seconds,
            expires_in_seconds,
        );
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (user_code, verification_uri, interval_seconds, expires_in_seconds);
    }

    /// Prompt for a value (e.g. "Paste the callback URL"); `Err` = the user cancelled (Pi `onPrompt`).
    pub fn on_prompt(
        &self,
        message: &str,
        placeholder: Option<&str>,
        allow_empty: bool,
    ) -> Result<String, String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::oauth::on_prompt(
                message,
                placeholder,
                allow_empty,
            );
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (message, placeholder, allow_empty);
            Err("oauth prompt unavailable on host target".into())
        }
    }

    /// Report progress during a long login (Pi `onProgress`).
    pub fn on_progress(&self, message: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::oauth::on_progress(message);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = message;
    }

    /// Show an interactive selector over `[(id, label)]`; returns the chosen id (Pi `onSelect`).
    pub fn on_select(&self, message: &str, options: &[(&str, &str)]) -> Option<String> {
        let options_json = serde_json::to_string(
            &options.iter().map(|(id, label)| serde_json::json!({"id": id, "label": label})).collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::oauth::on_select(message, &options_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (message, options_json);
            None
        }
    }
}

/// The streaming sink a `streamSimple` handler pushes assistant-message events into (Pi
/// `createAssistantMessageEventStream`). Each [`Self::emit`] forwards an event across the
/// `provider-stream` host import; the host relays them to the provider pipeline.
#[derive(Clone, Debug)]
pub struct ProviderStream {
    stream_id: String,
}

impl ProviderStream {
    /// Bind a sink to the host-assigned `stream_id` every [`Self::emit`] is addressed to. The
    /// guest glue builds this for a `provider-stream-simple` call; a `streamSimple` handler
    /// receives it rather than constructing one.
    pub fn new(stream_id: impl Into<String>) -> Self {
        Self { stream_id: stream_id.into() }
    }
    /// The host-assigned id of this stream.
    pub fn id(&self) -> &str {
        &self.stream_id
    }
    /// Push one assistant-message stream event (Pi `stream.push(event)`).
    ///
    /// **On an encode failure NO event is pushed.** `event` is author-supplied and its `serde_json`
    /// encoding is fallible; rather than pushing a `null` event into the provider pipeline, the
    /// push is skipped and the error is surfaced as an error-severity [`Ui::notify_with`]
    /// notification. The signature stays `()` — Pi's `stream.push` has no return value to fold an
    /// `Err` into.
    ///
    /// [`Ui::notify_with`]: crate::Ui::notify_with
    pub fn emit(&self, event: impl Serialize) {
        let event_json = match serde_json::to_string(&event) {
            Ok(s) => s,
            Err(e) => {
                crate::ctx::Ui.notify_with(
                    &format!(
                        "ProviderStream::emit({}): event dropped, failed to encode: {e}",
                        self.stream_id
                    ),
                    crate::ctx::NotifyKind::Error,
                );
                return;
            }
        };
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::provider_stream::emit_event(&self.stream_id, &event_json);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = event_json;
    }

    /// **Call this BEFORE sending the provider request**, with the request payload, and send the
    /// REPLACEMENT this returns (`Ok(None)` = unchanged).
    ///
    /// Half of pi's must-invoke `streamSimple` contract, quoted verbatim from
    /// `ProviderConfig.streamSimple` (`extensions/types.ts:1452-1457` @v0.84.1): "Implementations
    /// must invoke `options.onPayload` before sending the provider request and use any returned
    /// replacement payload. They must invoke `options.onResponse` after receiving the response and
    /// before consuming its body, matching built-in providers."
    ///
    /// EXT-M05 — EXT-052 landed this contract on the HOST side only: `provider-stream.on-payload`
    /// is declared in `world.wit` and implemented in `cyrup-ext/src/host/live.rs`, but no SDK
    /// surface existed, so a guest provider written with this crate could not invoke it no matter
    /// how carefully its author read the world. `ProviderStream::emit` was the type's only method.
    /// The failure is exactly the one the world comment names: every request an extension-supplied
    /// provider issues stays invisible to `before_provider_request`, so a redaction or audit
    /// extension silently stops working the moment the user switches to a guest provider.
    ///
    /// (Upstream these are fields of the `options` bag rather than of the stream. They hang off
    /// `ProviderStream` here because the world keys BOTH on the same `stream-id` this type already
    /// owns — a [CYRUP-DELTA] of shape, not of semantics, against `types.ts:1452-1457`.)
    ///
    /// **On an encode failure the host is NOT consulted and this returns `Ok(None)`.** `payload` is
    /// author-supplied and its `serde_json` encoding is fallible; handing the host a `null` payload
    /// would show `before_provider_request` subscribers a request that was never made, so the call
    /// is skipped and the error is surfaced as an error-severity [`Ui::notify_with`] notification.
    /// `Ok(None)` already means "no replacement, send yours unchanged", so the outbound request is
    /// unaffected.
    ///
    /// **`Err` = the host answered with a replacement this SDK could not decode.** The `Ok(None)`
    /// arm above is reserved for "the host had no replacement", so a `serde_json::from_str` failure
    /// on the returned string cannot share it: folding the two together would send the ORIGINAL,
    /// un-redacted payload while `before_provider_request` believed it had rewritten the request,
    /// with no error raised anywhere. The host serializes a `serde_json::Value`
    /// (`crates/cyrup-ext/src/host/live.rs:985`, `.map(|v| v.to_string())`), so this arm is
    /// defensive against a non-cyrup host rather than reachable today.
    ///
    /// [`Ui::notify_with`]: crate::Ui::notify_with
    pub fn on_payload(&self, payload: impl Serialize) -> Result<Option<Value>, String> {
        let id = &self.stream_id;
        let payload_json = match serde_json::to_string(&payload) {
            Ok(s) => s,
            Err(e) => {
                let m = format!("ProviderStream::on_payload({id}): host not consulted, encode failed: {e}");
                crate::ctx::Ui.notify_with(&m, crate::ctx::NotifyKind::Error);
                return Ok(None);
            }
        };
        #[cfg(target_arch = "wasm32")]
        return crate::guest::bindings::cyrup::ext::provider_stream::on_payload(id, &payload_json)
            .map(|s| serde_json::from_str(&s).map_err(|e| {
                format!("ProviderStream::on_payload({id}): host replacement is not valid JSON: {e}")
            }))
            .transpose();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = payload_json;
            Ok(None)
        }
    }

    /// **Call this AFTER receiving the provider response and BEFORE consuming its body** — the
    /// notify-only half of the same must-invoke contract (EXT-M05; see [`Self::on_payload`]). This
    /// is how `after_provider_response` (`extensions/types.ts:692-696`) reaches a guest provider's
    /// responses; `headers` is serialized as the response header map.
    ///
    /// **On a `headers` encode failure the host is NOT notified.** `headers` is author-supplied and
    /// its `serde_json` encoding is fallible; reporting the response with an empty `{}` header map
    /// would show `after_provider_response` subscribers headers the provider never returned, so the
    /// call is skipped and the error is surfaced as an error-severity [`Ui::notify_with`]
    /// notification. The signature stays `()` — this half of the contract is notify-only.
    ///
    /// [`Ui::notify_with`]: crate::Ui::notify_with
    pub fn on_response(&self, status: u16, headers: impl Serialize) {
        let headers_json = match serde_json::to_string(&headers) {
            Ok(s) => s,
            Err(e) => {
                crate::ctx::Ui.notify_with(
                    &format!(
                        "ProviderStream::on_response({}): host not notified, headers failed to encode: {e}",
                        self.stream_id
                    ),
                    crate::ctx::NotifyKind::Error,
                );
                return;
            }
        };
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::provider_stream::on_response(
            &self.stream_id,
            status,
            &headers_json,
        );
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (status, headers_json);
    }
}

/// The guest's dynamic OAuth provider (Pi `ProviderConfig.oauth`, types.ts:1380-1392). `login`
/// runs the interactive flow and returns the credentials to persist; `refresh_token` renews them;
/// `get_api_key` derives the API key string; `modify_models` optionally rewrites the model list.
pub struct OAuthProvider {
    /// Display name in the login UI.
    pub name: String,
    /// Runs the interactive flow through [`OAuthCallbacks`] and returns the credentials to
    /// persist. Required; set by [`Self::new`].
    #[allow(clippy::type_complexity)]
    pub login: Box<dyn Fn(&OAuthCallbacks) -> Result<OAuthCredentials, String> + 'static>,
    /// Renews stored credentials. Required; set by [`Self::new`].
    #[allow(clippy::type_complexity)]
    pub refresh_token: Box<dyn Fn(OAuthCredentials) -> Result<OAuthCredentials, String> + 'static>,
    /// Derives the API key string from the credentials. Required; set by [`Self::new`].
    #[allow(clippy::type_complexity)]
    pub get_api_key: Box<dyn Fn(&OAuthCredentials) -> Result<String, String> + 'static>,
    /// Optional: rewrite the provider's models given the credentials (e.g. update baseUrl).
    #[allow(clippy::type_complexity)]
    pub modify_models:
        Option<Box<dyn Fn(Value, &OAuthCredentials) -> Result<Value, String> + 'static>>,
}

impl OAuthProvider {
    /// Build an OAuth provider from its three required closures.
    pub fn new(
        name: impl Into<String>,
        login: impl Fn(&OAuthCallbacks) -> Result<OAuthCredentials, String> + 'static,
        refresh_token: impl Fn(OAuthCredentials) -> Result<OAuthCredentials, String> + 'static,
        get_api_key: impl Fn(&OAuthCredentials) -> Result<String, String> + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            login: Box::new(login),
            refresh_token: Box::new(refresh_token),
            get_api_key: Box::new(get_api_key),
            modify_models: None,
        }
    }

    /// Attach an optional `modifyModels` closure.
    #[must_use]
    pub fn with_modify_models(
        mut self,
        f: impl Fn(Value, &OAuthCredentials) -> Result<Value, String> + 'static,
    ) -> Self {
        self.modify_models = Some(Box::new(f));
        self
    }
}

/// A guest `streamSimple` handler (Pi `ProviderConfig.streamSimple`, on the config
/// `registerProvider(name, config)` takes, types.ts:1401): given the model / context / options it
/// pushes assistant-message events into the [`ProviderStream`] then returns.
pub trait StreamSimple: 'static {
    /// Push the response for one call into `out`, then return. An `Err` here is the
    /// provider-level failure the host sees; a single event that fails to encode never reaches
    /// this return value — [`ProviderStream::emit`] drops it and reports it as a notification.
    fn stream(
        &self,
        model: Value,
        context: Value,
        options: Value,
        out: &ProviderStream,
    ) -> Result<(), String>;
}

impl<F> StreamSimple for F
where
    F: Fn(Value, Value, Value, &ProviderStream) -> Result<(), String> + 'static,
{
    fn stream(
        &self,
        model: Value,
        context: Value,
        options: Value,
        out: &ProviderStream,
    ) -> Result<(), String> {
        (self)(model, context, options, out)
    }
}

/// The dynamic provider callbacks paired with a static [`crate::ProviderConfig`] at registration
/// (the non-serializable half of Pi's `registerProvider(config)`).
#[derive(Default)]
pub struct ProviderHandlers {
    /// The dynamic OAuth half, or `None` for a provider that needs no login flow. Set by
    /// [`Self::with_oauth`]; [`Self::has_oauth`] tests it.
    pub oauth: Option<OAuthProvider>,
    /// The custom streaming half, or `None` to leave streaming to the host. Set by
    /// [`Self::with_stream_simple`]; [`Self::has_stream_simple`] tests it.
    pub stream_simple: Option<Box<dyn StreamSimple>>,
}

impl ProviderHandlers {
    /// Empty handlers — neither half set. The head of the builder chain
    /// [`Self::with_oauth`]/[`Self::with_stream_simple`].
    pub fn new() -> Self {
        Self::default()
    }
    /// Attach the OAuth half (builder-style).
    #[must_use]
    pub fn with_oauth(mut self, oauth: OAuthProvider) -> Self {
        self.oauth = Some(oauth);
        self
    }
    /// Attach the `streamSimple` half (builder-style).
    #[must_use]
    pub fn with_stream_simple(mut self, stream: impl StreamSimple) -> Self {
        self.stream_simple = Some(Box::new(stream));
        self
    }
    /// Whether an [`OAuthProvider`] is attached.
    pub fn has_oauth(&self) -> bool {
        self.oauth.is_some()
    }
    /// Whether a [`StreamSimple`] handler is attached.
    pub fn has_stream_simple(&self) -> bool {
        self.stream_simple.is_some()
    }
}
