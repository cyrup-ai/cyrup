//! The `typesafe-system-one` classifier api (1:1 port of pi
//! `packages/ai/src/api/typesafe-system-one.ts` and its `typesafe-system-one.lazy.ts` wrapper
//! @f1b2e77f5, PROV-104).
//!
//! TypeSafe's native System One protocol: `POST <baseUrl>/systemone` with
//! `{"model": <id>, "state", "questions"}`, answered with `{answers, usage}` at the top level.
//! Three services speak it with different base URLs, so all three use this api:
//!
//! * TypeSafe itself (`https://api.typesafe.ai/v1/`);
//! * OpenRouter, which "serves TypeSafe's System One protocol at /api/v1/systemone"
//!   (`providers/openrouter.ts:33-34`); and
//! * llama.cpp's `llama-server` for its decision models (`POST /v1/systemone`,
//!   `tools/server/server.cpp:289` @b11436), which pi's llama extension classifies through this
//!   api with the server's `/v1` base URL (`toPiClassifierModel`, `extensions/llama/provider.ts:
//!   102-117`; EXT-110). A real b11436 router over a decision GGUF answered this api's request with
//!   the shape `cyrup_llama_cpp_wire::systemone` pins.
//!
//! pi's `ClassifierOptions.temperature` is ignored: System One has no temperature field
//! (`test/typesafe-system-one.test.ts`, "the option is ignored").
//!
//! `[CYRUP-DELTA, mechanism]` — `lazy.ts` defers the module import to the first call; Rust has
//! nothing to defer, so [`typesafe_system_one_api`] returns the implementation directly, the same
//! substitution [`super::llama_cpp_classify`] documents.

use std::sync::Arc;

use serde_json::{Map, Value};

use super::classifier_shared::ClassifyError;
use super::system_one_shared::{SystemOneTransport, classify_system_one, endpoint_url};
use crate::classifier::{
    ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult, KnownClassifierApi,
    ProviderClassifier,
};

/// Service name in error text (`label: "System One API"`).
const LABEL: &str = "System One API";

/// `payload: (model, request) => ({ model: model.id, ...request })`.
fn payload(model: &ClassifierModel, request: Map<String, Value>) -> Value {
    let mut body = Map::new();
    body.insert("model".to_string(), Value::from(model.id.as_str()));
    body.extend(request);
    Value::Object(body)
}

/// `output`: the body itself, which must be an object.
fn output(body: Value) -> Result<Map<String, Value>, ClassifyError> {
    match body {
        Value::Object(object) => Ok(object),
        _ => Err(ClassifyError::plain(format!(
            "{LABEL} returned an unexpected response"
        ))),
    }
}

/// The transport (pi `const transport: SystemOneTransport`).
const TRANSPORT: SystemOneTransport = SystemOneTransport {
    api: KnownClassifierApi::TypesafeSystemOne,
    label: LABEL,
    url: |model| endpoint_url(&model.base_url, "systemone"),
    payload,
    output,
};

/// TypeSafe System One classification with public `bool` values mapped to wire-level `noul`
/// (pi `classify`).
struct TypesafeSystemOne;

#[async_trait::async_trait]
impl ProviderClassifier for TypesafeSystemOne {
    async fn classify(
        &self,
        model: &ClassifierModel,
        context: &ClassifierContext,
        options: &ClassifierOptions,
    ) -> ClassifierResult {
        classify_system_one(TRANSPORT, model, context, options).await
    }
}

/// The `typesafe-system-one` implementation to register under
/// [`KnownClassifierApi::TypesafeSystemOne`] (pi `typesafeSystemOneApi`,
/// `typesafe-system-one.lazy.ts`).
pub fn typesafe_system_one_api() -> Arc<dyn ProviderClassifier> {
    Arc::new(TypesafeSystemOne)
}
