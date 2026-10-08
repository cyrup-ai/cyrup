//! `ProviderStreamFn` must refuse a VIRTUAL model instead of silently streaming an arbitrary one.
//!
//! A virtual model is a catalog entry that routes each request to a physical model
//! (`cyrup_core::VIRTUAL_MODEL_API`). The session's routing step replaces it before the request is
//! built, so nothing should ever reach a provider carrying that api — but
//! [`crate::ProviderStreamFn::stream`] resolves the request's model id against the installed
//! provider's catalog and FALLS BACK to that catalog's first entry, so an unrouted virtual model
//! would otherwise be answered by whichever model happens to be listed first, with no error
//! anywhere.
//!
//! Upstream has the same shape of guard in the only place a virtual model can reach: the
//! `withVirtualModels` decorator's `unroutedStream`
//! (`packages/coding-agent/src/core/virtual-models.ts:189-194` @v1.0.4), whose own text this
//! reproduces. `ModelRuntime.stream` has no virtual arm at all, which is why the guard sits at the
//! transport.

use std::sync::Arc;

use cyrup_core::{ApiId, ModelRef, StopReason};
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::{Context, Provider, StreamOptions, collect_message};

use crate::{ProviderStreamFn, StreamFn};

fn virtual_ref() -> ModelRef {
    ModelRef {
        provider: "router".into(),
        api: Some(ApiId::from(cyrup_core::VIRTUAL_MODEL_API)),
        model: "auto".into(),
    }
}

/// RED-PROVE: replace the `pi-virtual` test in `ProviderStreamFn::stream` with `if false` — the
/// faux provider's catalog fallback answers instead, the terminal becomes a `Stop` from
/// `faux/faux-model`, and both assertions below fail.
#[tokio::test]
async fn an_unrouted_virtual_model_is_a_terminal_error_and_never_reaches_the_provider() {
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = Arc::clone(&faux) as Arc<dyn Provider>;
    let stream_fn = ProviderStreamFn::new(provider);

    let ctx = Context {
        system_prompt: None,
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let message =
        collect_message(stream_fn.stream(&virtual_ref(), &ctx, &StreamOptions::default())).await;

    assert_eq!(message.stop_reason, StopReason::Error);
    assert_eq!(
        message.error_message.as_deref(),
        Some("Virtual model router/auto must be routed before streaming"),
        "upstream's own `unroutedStream` text (virtual-models.ts:190-193)"
    );
    assert_eq!(
        message.provider.as_str(),
        "router",
        "the error names the VIRTUAL model, as pi's does"
    );
    assert_eq!(message.model, "auto");
    assert_eq!(message.api.as_str(), cyrup_core::VIRTUAL_MODEL_API);
    assert_eq!(
        faux.call_count(),
        0,
        "the provider must not be asked at all — the fallback this guards would have asked it \
         for `faux-model`"
    );
}

/// REGRESSION GUARD, not a red proof: a PHYSICAL model still reaches the provider unchanged.
#[tokio::test]
async fn a_physical_model_still_reaches_the_provider() {
    let faux = Arc::new(FauxProvider::new());
    let model = faux.model().clone();
    let provider: Arc<dyn Provider> = Arc::clone(&faux) as Arc<dyn Provider>;
    let stream_fn = ProviderStreamFn::new(provider);
    let ctx = Context {
        system_prompt: None,
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let model_ref = ModelRef {
        provider: model.provider.clone(),
        api: Some(model.api.clone()),
        model: model.id.clone(),
    };
    let _ = collect_message(stream_fn.stream(&model_ref, &ctx, &StreamOptions::default())).await;
    assert_eq!(faux.call_count(), 1);
}
