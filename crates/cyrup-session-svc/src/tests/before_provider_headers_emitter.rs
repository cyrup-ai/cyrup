//! PROV-042 — `before_provider_headers` must have a PRODUCER.
//!
//! Every other piece of this hook already existed at HEAD: the `EventKind`
//! (`cyrup-ext/src/event.rs:60`), the WIT export and its guest side, the SDK's
//! `on_before_provider_headers` registration, and the in-place / `null`-deletes reducer
//! (`cyrup-ext/src/contract.rs:187-196`). What did not exist was any production site that
//! CONSTRUCTED the event: `rg 'HostEvent::BeforeProviderHeaders' crates/` found the enum, the
//! reducer and one test, and nothing else. An extension could subscribe to
//! `before_provider_headers` and would simply never be called — worse than a documented refusal,
//! because it is invisible from the extension author's side.
//!
//! Upstream, `git -C tmp/pi show v0.84.4:packages/coding-agent/src/core/sdk.ts` `:330-339`:
//!
//! ```text
//! transformHeaders: async (requestHeaders) => {
//!     const headers = mergeProviderAttributionHeaders(model, settingsManager, options?.sessionId, requestHeaders);
//!     return headerRunner?.hasHandlers("before_provider_headers")
//!         ? headerRunner.emitBeforeProviderHeaders(headers ?? {})
//!         : (headers ?? {});
//! },
//! ```
//!
//! — the session's `streamFn` installs the transform, and the extension dispatch happens inside it,
//! gated on a live handler. `builder.rs` now occupies exactly that position via
//! `AgentBuilder::transform_headers`.
//!
//! These tests assert the END-TO-END wiring, not the facade: a real `SessionBuilder`-built session
//! runs a real turn, and the transport the agent streams through reads `opts.transform_headers` off
//! the `StreamOptions` the loop dispatched and invokes it. Red before the emitter landed: the field
//! was `None` on every request (`saw_transform == false`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{InputSource, SessionBuilder, SessionConfig, UserInput};
use cyrup_agent::{Context, ProviderStreamFn, StreamEvent, StreamFn, StreamOptions};
use cyrup_core::{EventStream, ExtensionId, ModelRef, StopReason};
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::HeaderMap;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use serde_json::json;
use tempfile::TempDir;

// ------------------------------------------------------------------------------- the extension --

/// A native built-in standing in for the corporate-proxy extension the item names: it ADDS a header
/// and DELETES one, which is the whole of pi's documented `before_provider_headers` contract
/// ("Handlers mutate `headers` in place … A `null` value deletes that header",
/// `extensions/types.ts:681-685`).
struct HeaderStamp;

#[async_trait::async_trait]
impl NativeExtension for HeaderStamp {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("header-stamp")
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::BeforeProviderHeaders]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if matches!(ev, HostEvent::BeforeProviderHeaders { .. }) {
            return HookOutcome::Mutate(EventPatch::ProviderHeaders(json!({
                "x-corp-proxy": "on",
                "x-api-key": null,
            })));
        }
        HookOutcome::Noop
    }
}

// -------------------------------------------------------------------------------- the transport --

/// The header bag the transport hands to the transform, standing in for the set an api impl has
/// just assembled (`x-api-key` is the header the item's Verify names as the suppression probe).
fn assembled() -> HeaderMap {
    HeaderMap::from([
        ("x-api-key".to_string(), Some("secret".to_string())),
        (
            "content-type".to_string(),
            Some("application/json".to_string()),
        ),
    ])
}

/// A [`StreamFn`] that reads `opts.transform_headers` off the `StreamOptions` the agent loop
/// dispatched — i.e. the very field the provider applies — runs it over [`assembled`], records the
/// result, then serves the turn from a scripted provider.
struct TransformProbe {
    inner: ProviderStreamFn,
    /// `None` until a turn ran. `Some(None)` = a turn ran and the field was absent (the defect).
    seen: Arc<Mutex<Option<Option<HeaderMap>>>>,
}

impl StreamFn for TransformProbe {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        let transform = opts.transform_headers.clone();
        let seen = self.seen.clone();
        // The transform is async (it dispatches into the extension host); resolve it on a detached
        // task and record the outcome before the turn settles.
        tokio::spawn(async move {
            let out = match transform {
                Some(t) => Some(t(assembled()).await),
                None => None,
            };
            if let Ok(mut g) = seen.lock() {
                *g = Some(out);
            }
        });
        self.inner.stream(model, ctx, opts)
    }
}

// ------------------------------------------------------------------------------------ fixtures --

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

fn scripted() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    faux
}

/// Build a session whose transport is the probe, run one turn, and return what the transform did to
/// [`assembled`] — `None` when the `StreamOptions` carried no transform at all.
async fn run_turn(with_extension: bool) -> Option<HeaderMap> {
    let fx = fixture();
    let provider: Arc<dyn Provider> = scripted();
    let seen: Arc<Mutex<Option<Option<HeaderMap>>>> = Arc::new(Mutex::new(None));
    let probe: Arc<dyn StreamFn> = Arc::new(TransformProbe {
        inner: ProviderStreamFn::new(scripted() as Arc<dyn Provider>),
        seen: seen.clone(),
    });

    let mut builder = SessionBuilder::new(provider, base_config(&fx)).stream_fn(probe);
    if with_extension {
        builder = builder.with_native_extension(Arc::new(HeaderStamp) as Arc<dyn NativeExtension>);
    }
    let session = builder.build().await.expect("build").into_shared();

    let _stream = session
        .prompt(UserInput::text("hello", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    session.wait_for_idle().await;

    // The probe resolves the transform on a detached task; give it a bounded window to land.
    for _ in 0..200 {
        if seen.lock().ok().and_then(|g| g.clone()).is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let recorded = seen.lock().ok().and_then(|g| g.clone());
    let Some(outcome) = recorded else {
        panic!("the transport never streamed a turn, so this proves nothing");
    };
    outcome
}

// --------------------------------------------------------------------------------- the finding --

/// THE headline: a subscribed extension is actually invoked, and both halves of pi's in-place
/// contract reach the header bag — the header it set is present, the header it nulled is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subscribed_extension_sees_the_headers_and_its_mutation_wins() {
    let out = run_turn(true).await.expect(
        "the StreamOptions the agent dispatched carried no `transform_headers`, so \
         `before_provider_headers` still has no producer",
    );

    assert_eq!(
        out.get("x-corp-proxy"),
        Some(&Some("on".to_string())),
        "the extension's added header did not survive back to the provider: {out:?}"
    );
    assert!(
        !out.contains_key("x-api-key"),
        "a `null` value must DELETE the header (pi types.ts:681-685); got {out:?}"
    );
    assert_eq!(
        out.get("content-type"),
        Some(&Some("application/json".to_string())),
        "a header the extension did not touch must pass through unchanged: {out:?}"
    );
}

/// The gate: with no subscriber the transform is still installed (pi installs it unconditionally,
/// `sdk.ts:330`) and is an exact identity — the no-extension path pays no JSON round-trip and, more
/// importantly, cannot lose or reorder a header.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_no_subscriber_the_transform_is_installed_and_is_the_identity() {
    let out = run_turn(false)
        .await
        .expect("the transform must be installed whether or not an extension subscribes");
    assert_eq!(
        out,
        assembled(),
        "the unsubscribed path must not touch the header bag"
    );
}
