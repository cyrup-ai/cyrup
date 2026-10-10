//! The `cloudflare-workers-ai-system-one` classifier api (1:1 port of pi
//! `packages/ai/src/api/cloudflare-workers-ai-system-one.ts` and its `.lazy.ts` wrapper
//! @f1b2e77f5, PROV-104).
//!
//! System One models on the Workers AI REST endpoint: `POST <baseUrl>/run` with
//! `{"model": <id>, "input": {"state", "questions"}}`. The REST API wraps the model output in
//! Cloudflare's API envelope, in one of two forms:
//!
//! * third-party models such as `typesafe/jev` add a run record:
//!   `{success, result: {state: "Completed", result: {answers, usage}}}`;
//! * Cloudflare-hosted models such as `@cf/cloudflare/clef` return the output directly:
//!   `{success, result: {model, answers, usage}}`.
//!
//! `success: false` fails the request with the envelope's error messages joined by `; `.
//!
//! The catalog's base URL carries a `{CLOUDFLARE_ACCOUNT_ID}` placeholder
//! (`https://api.cloudflare.com/client/v4/accounts/{CLOUDFLARE_ACCOUNT_ID}/ai`). pi resolves it by
//! wrapping this api in `cloudflareClassifier` (`providers/cloudflare-stream.ts`); cyrup's
//! Workers AI auth already substitutes the account id into the resolved base URL, which
//! [`crate::collection::Models::classify`] applies to the model before dispatch, so the api itself
//! needs no wrapper (`crate::providers::cloudflare`).
//!
//! `[CYRUP-DELTA, mechanism]` — `lazy.ts`, as in [`super::typesafe_system_one`].

use std::sync::Arc;

use serde_json::{Map, Value};

use super::classifier_shared::ClassifyError;
use super::system_one_shared::{SystemOneTransport, classify_system_one, endpoint_url};
use crate::classifier::{
    ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult, KnownClassifierApi,
    ProviderClassifier,
};

/// Service name in error text (`const LABEL = "Cloudflare Workers AI"`).
const LABEL: &str = "Cloudflare Workers AI";

/// pi `cloudflareErrorMessage`: the `message` of every error object that has a string one, joined
/// by `; `, or `Cloudflare Workers AI request failed` when there is none.
fn cloudflare_error_message(errors: Option<&Value>) -> String {
    let messages: Vec<&str> = errors
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .filter_map(|error| error.as_object()?.get("message")?.as_str())
                .collect()
        })
        .unwrap_or_default();
    if messages.is_empty() {
        format!("{LABEL} request failed")
    } else {
        format!("{LABEL} error: {}", messages.join("; "))
    }
}

/// `String(value)` for the JSON values a run record's `state` can hold (ECMA-262 `ToString`).
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_string(),
    }
}

/// `payload: (model, request) => ({ model: model.id, input: request })`.
fn payload(model: &ClassifierModel, request: Map<String, Value>) -> Value {
    let mut body = Map::new();
    body.insert("model".to_string(), Value::from(model.id.as_str()));
    body.insert("input".to_string(), Value::Object(request));
    Value::Object(body)
}

/// `output`: unwrap Cloudflare's envelope, and the run record when there is one.
fn output(body: Value) -> Result<Map<String, Value>, ClassifyError> {
    let unexpected = || ClassifyError::plain(format!("{LABEL} returned an unexpected response"));
    let Value::Object(mut body) = body else {
        return Err(unexpected());
    };
    if body.get("success") == Some(&Value::Bool(false)) {
        return Err(ClassifyError::plain(cloudflare_error_message(
            body.get("errors"),
        )));
    }
    let Some(Value::Object(mut result)) = body.remove("result") else {
        return Err(unexpected());
    };
    if result.contains_key("answers") {
        return Ok(result);
    }
    if result.get("state").and_then(Value::as_str) != Some("Completed") {
        return Err(ClassifyError::plain(format!(
            "{LABEL} run did not complete (state: {})",
            js_string(result.get("state"))
        )));
    }
    match result.remove("result") {
        Some(Value::Object(inner)) => Ok(inner),
        _ => Err(unexpected()),
    }
}

/// The transport (pi `const transport: SystemOneTransport`).
const TRANSPORT: SystemOneTransport = SystemOneTransport {
    api: KnownClassifierApi::CloudflareWorkersAiSystemOne,
    label: LABEL,
    url: |model| endpoint_url(&model.base_url, "run"),
    payload,
    output,
};

/// Cloudflare Workers AI System One classification with public `bool` values mapped to wire-level
/// `noul` (pi `classify`).
struct CloudflareWorkersAiSystemOne;

#[async_trait::async_trait]
impl ProviderClassifier for CloudflareWorkersAiSystemOne {
    async fn classify(
        &self,
        model: &ClassifierModel,
        context: &ClassifierContext,
        options: &ClassifierOptions,
    ) -> ClassifierResult {
        classify_system_one(TRANSPORT, model, context, options).await
    }
}

/// The `cloudflare-workers-ai-system-one` implementation to register under
/// [`KnownClassifierApi::CloudflareWorkersAiSystemOne`] (pi `cloudflareWorkersAISystemOneApi`,
/// `cloudflare-workers-ai-system-one.lazy.ts`).
pub fn cloudflare_workers_ai_system_one_api() -> Arc<dyn ProviderClassifier> {
    Arc::new(CloudflareWorkersAiSystemOne)
}
