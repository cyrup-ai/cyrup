//! `models.*` for scripts: the model registry methods documented in `docs/codemode.md` (pi
//! `createModelGlobals`, `extensions/codemode/execute.ts:524-630` and the argument checks above it
//! @v1.0.1, CODE-008).
//!
//! Classifier and image calls appear as nested call rows so the renderer shows them, and their
//! usage goes to the [`Recorder`]. Rows show only the model, never prompts or image data. A model
//! is resolved by provider and id alone, so a baseUrl or headers a script supplies can never
//! receive the credentials, and catalog entries handed to the script carry no `headers`.
//!
//! # What backs each function
//!
//! [`CodemodeModels`] is upstream's `Pick<ModelRegistry, "getModelsOfType" | "getAvailableOfType" |
//! "getModelOfType" | "classify" | "generateImages">` (`tool.ts:70-74`). [`cyrup_provider::Models`]
//! implements it, method for method:
//!
//! | script function | backing |
//! |---|---|
//! | `getModelsOfType` | `Models::get_models_of_type` |
//! | `getAvailableOfType` | `Models::get_available_of_type` (PROV-105) |
//! | `getModelOfType` | `Models::get_model_of_type` |
//! | `classify` | `Models::classify` |
//! | `generateImages` | `Models::generate_images` |
//!
//! # Production call path
//!
//! [`super::execute::execute_codemode`] passes [`models_globals`] to the sandbox when the host
//! reports model access ([`CodemodeHost::models`](super::host::CodemodeHost::models)); the session
//! host returns a `Models` composed from its own registry, credentials and providers.

use std::sync::Arc;

use cyrup_codemode::js::{json_stringify, own_keys};
use cyrup_core::{CancelToken, Content};
use cyrup_provider::{
    AnyModel, AssistantImages, ClassifierContext, ClassifierModel, ClassifierOptions,
    ClassifierResult, ClassifierStopReason, ImageModel, ImagesContext, ImagesOptions,
    ImagesStopReason, ModelType, Models,
};
use serde_json::{Map, Value};
use tokio::sync::Semaphore;

use super::globals::{describe_value, spread_global};
use super::recorder::{ERROR_PREVIEW_CHARS, Recorder, truncate_text};
use super::{CodemodeNestedCall, CodemodeNestedCallStatus};
use crate::types::CodemodeTool;

/// `models.classify()` and `models.generateImages()` calls one script may have in flight;
/// `Promise.all` over many items queues the rest (`MAX_CONCURRENT_MODEL_CALLS`, `execute.ts:51`).
pub const MAX_CONCURRENT_MODEL_CALLS: usize = 4;

/// The model registry as scripts reach it. See the module docs for what backs each method.
#[async_trait::async_trait]
pub trait CodemodeModels: Send + Sync {
    /// Every known model of a type, optionally for one provider.
    fn get_models_of_type(&self, model_type: ModelType, provider: Option<&str>) -> Vec<AnyModel>;

    /// Models of a type whose provider has working credentials. `cancel` abandons the wait.
    ///
    /// # Errors
    ///
    /// The message of an auth-check failure, or of the abort.
    async fn get_available_of_type(
        &self,
        model_type: ModelType,
        provider: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<Vec<AnyModel>, String>;

    /// One catalog entry.
    fn get_model_of_type(
        &self,
        model_type: ModelType,
        provider: &str,
        id: &str,
    ) -> Option<AnyModel>;

    /// Never fails as a call: failures are in the result's `stop_reason` and `error_message`.
    async fn classify(
        &self,
        model: &ClassifierModel,
        context: &ClassifierContext,
        cancel: &CancelToken,
    ) -> ClassifierResult;

    /// Never fails as a call: failures are in the result's `stop_reason` and `error_message`.
    async fn generate_images(
        &self,
        model: &ImageModel,
        context: &ImagesContext,
        cancel: &CancelToken,
    ) -> AssistantImages;
}

#[async_trait::async_trait]
impl CodemodeModels for Models {
    fn get_models_of_type(&self, model_type: ModelType, provider: Option<&str>) -> Vec<AnyModel> {
        Models::get_models_of_type(self, model_type, provider)
    }

    async fn get_available_of_type(
        &self,
        model_type: ModelType,
        provider: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<Vec<AnyModel>, String> {
        // `raceWithAbortSignal(available, signal)` (`models.ts:697`, `:709`).
        tokio::select! {
            available = Models::get_available_of_type(self, model_type, provider) => {
                available.map_err(|error| error.to_string())
            }
            () = cancel.cancelled() => Err("This operation was aborted".to_owned()),
        }
    }

    fn get_model_of_type(
        &self,
        model_type: ModelType,
        provider: &str,
        id: &str,
    ) -> Option<AnyModel> {
        Models::get_model_of_type(self, model_type, provider, id)
    }

    async fn classify(
        &self,
        model: &ClassifierModel,
        context: &ClassifierContext,
        cancel: &CancelToken,
    ) -> ClassifierResult {
        let options = ClassifierOptions {
            cancel: Some(cancel.clone()),
            ..ClassifierOptions::default()
        };
        Models::classify(self, model, context, &options).await
    }

    async fn generate_images(
        &self,
        model: &ImageModel,
        context: &ImagesContext,
        cancel: &CancelToken,
    ) -> AssistantImages {
        let options = ImagesOptions {
            cancel: Some(cancel.clone()),
            ..ImagesOptions::default()
        };
        Models::generate_images(self, model, context, &options).await
    }
}

// ----------------------------------------------------------------------------- argument checks --

/// `an image`, `a classifier` (`withArticle`, `execute.ts:84-86`).
fn with_article(word: &str) -> String {
    let article = if word.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    };
    format!("{article} {word}")
}

fn is_record(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

/// `toModelType` (`execute.ts:64-67`).
fn to_model_type(value: Option<&Value>) -> Result<ModelType, String> {
    match value.and_then(Value::as_str) {
        Some("chat") => Ok(ModelType::Chat),
        Some("image") => Ok(ModelType::Image),
        Some("classifier") => Ok(ModelType::Classifier),
        _ => Err(format!(
            "Unknown model type {}. Use \"chat\", \"image\", or \"classifier\".",
            value.map_or_else(|| "undefined".to_owned(), json_stringify)
        )),
    }
}

/// `toProvider` (`execute.ts:69-73`): an absent or null provider is "all providers".
fn to_provider(value: Option<&Value>) -> Result<Option<&str>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(provider)) => Ok(Some(provider.as_str())),
        Some(_) => Err("provider must be a string".to_owned()),
    }
}

/// A catalog entry for scripts, without `headers`, which models.json headers can carry credentials
/// in (`toModelInfo`, `execute.ts:76-82`).
fn to_model_info(model: &AnyModel) -> Result<Value, String> {
    let mut info = serde_json::to_value(model).map_err(|error| error.to_string())?;
    if let Value::Object(map) = &mut info {
        map.remove("headers");
    }
    Ok(info)
}

const CLASSIFIER_CONTEXT_SHAPE: &str = r#"{ state: { ... }, questions: { <id>: { type: "choice", instructions, criteria: { <label>: <meaning> } } | { type: "score", instructions, criteria: [<lowest level>, ..., <highest level>] } | { type: "bool", instructions, criteria: { true: <meaning>, false: <meaning> } } } }"#;

/// Check a script's classifier context, so mistakes fail with the expected shape instead of a
/// provider error (`checkClassifierContext`, `execute.ts:102-142`).
fn check_classifier_context(
    context: Option<&Value>,
    docs_path: &str,
) -> Result<ClassifierContext, String> {
    let fail = |problem: String| {
        format!(
            "models.classify() {problem}. Expected context: {CLASSIFIER_CONTEXT_SHAPE}. See \"Classify\" in {docs_path}."
        )
    };
    let Some(context_object) = is_record(context) else {
        return Err(fail(format!(
            "expects a context object as its second argument, got {}",
            describe_value(context)
        )));
    };
    let state = context_object.get("state");
    if is_record(state).is_none() {
        return Err(fail(format!(
            "context.state must be an object, got {}",
            describe_value(state)
        )));
    }
    let questions = context_object.get("questions");
    let Some(questions_object) = is_record(questions).filter(|map| !map.is_empty()) else {
        return Err(fail(format!(
            "context.questions must map question IDs to questions, got {}",
            describe_value(questions)
        )));
    };
    let is_strings = |values: &mut dyn Iterator<Item = &Value>| {
        let mut any = false;
        for value in values {
            any = true;
            if !value.is_string() {
                return false;
            }
        }
        any
    };
    for id in own_keys(questions_object) {
        let at = format!("context.questions.{id}");
        let question = questions_object.get(id);
        let Some(question_object) = is_record(question) else {
            return Err(fail(format!(
                "{at} must be a question object, got {}",
                describe_value(question)
            )));
        };
        if !question_object
            .get("instructions")
            .is_some_and(Value::is_string)
        {
            return Err(fail(format!("{at}.instructions must be a string")));
        }
        let criteria = question_object.get("criteria");
        match question_object.get("type").and_then(Value::as_str) {
            Some("choice") => {
                if !is_record(criteria).is_some_and(|map| is_strings(&mut map.values())) {
                    return Err(fail(format!(
                        "{at} is a \"choice\" question, so criteria must map each label to its meaning"
                    )));
                }
            }
            Some("score") => {
                if !criteria
                    .and_then(Value::as_array)
                    .is_some_and(|items| is_strings(&mut items.iter()))
                {
                    return Err(fail(format!(
                        "{at} is a \"score\" question, so criteria must list the levels as strings, lowest first"
                    )));
                }
            }
            Some("bool") => {
                let both = is_record(criteria).is_some_and(|map| {
                    map.get("true").is_some_and(Value::is_string)
                        && map.get("false").is_some_and(Value::is_string)
                });
                if !both {
                    return Err(fail(format!(
                        "{at} is a \"bool\" question, so criteria must be {{ true: string, false: string }}"
                    )));
                }
            }
            _ => {
                return Err(fail(format!(
                    "{at}.type must be \"choice\", \"score\", or \"bool\", got {}",
                    question_object
                        .get("type")
                        .map_or_else(|| "undefined".to_owned(), json_stringify)
                )));
            }
        }
    }
    serde_json::from_value(Value::Object(context_object.clone()))
        .map_err(|error| fail(format!("could not be read: {error}")))
}

/// Check a script's image context, so mistakes such as `{ prompt }` fail with the expected shape
/// (`checkImagesContext`, `execute.ts:144-170`).
fn check_images_context(context: Option<&Value>, docs_path: &str) -> Result<ImagesContext, String> {
    let fail = |problem: String| {
        format!(
            "models.generateImages() {problem}. Expected context: {{ input: [{{ type: \"text\", text: <prompt> }}, ...optional {{ type: \"image\", data: <base64>, mimeType }} references] }}. See \"Generate images\" in {docs_path}."
        )
    };
    let Some(context_object) = is_record(context) else {
        return Err(fail(format!(
            "expects a context object as its second argument, got {}",
            describe_value(context)
        )));
    };
    let input = context_object.get("input");
    let Some(blocks) = input
        .and_then(Value::as_array)
        .filter(|blocks| !blocks.is_empty())
    else {
        return Err(fail(format!(
            "context.input must be a non-empty array of blocks, got {}",
            describe_value(input)
        )));
    };
    for (index, block) in blocks.iter().enumerate() {
        let valid = is_record(Some(block)).is_some_and(|map| {
            let string = |key: &str| map.get(key).is_some_and(Value::is_string);
            match map.get("type").and_then(Value::as_str) {
                Some("text") => string("text"),
                Some("image") => string("data") && string("mimeType"),
                _ => false,
            }
        });
        if !valid {
            return Err(fail(format!(
                "context.input[{index}] must be a text or image block, got {}",
                describe_value(Some(block))
            )));
        }
    }
    serde_json::from_value(Value::Object(context_object.clone()))
        .map_err(|error| fail(format!("could not be read: {error}")))
}

// ------------------------------------------------------------------------------------ globals --

/// What `models.*` shares across the script's calls.
struct ModelsState {
    models: Arc<dyn CodemodeModels>,
    recorder: Arc<Recorder>,
    limiter: Semaphore,
    docs_path: String,
    call_count: std::sync::atomic::AtomicUsize,
}

/// `models.getModelsOfType`, `getAvailableOfType`, `getModelOfType`, `classify` and
/// `generateImages` over `models` (`createModelGlobals`, `execute.ts:524-630`).
#[must_use]
pub fn models_globals(
    models: Arc<dyn CodemodeModels>,
    recorder: Arc<Recorder>,
    docs_path: &str,
) -> Vec<CodemodeTool> {
    let state = Arc::new(ModelsState {
        models,
        recorder,
        limiter: Semaphore::new(MAX_CONCURRENT_MODEL_CALLS),
        docs_path: docs_path.to_owned(),
        call_count: std::sync::atomic::AtomicUsize::new(0),
    });
    let mut globals = Vec::with_capacity(5);

    let s = Arc::clone(&state);
    globals.push(spread_global("models.getModelsOfType", move |args, _| {
        let s = Arc::clone(&s);
        async move {
            let model_type = to_model_type(args.first())?;
            let provider = to_provider(args.get(1))?;
            let listed = s.models.get_models_of_type(model_type, provider);
            let infos = listed
                .iter()
                .map(to_model_info)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some(Value::Array(infos)))
        }
    }));

    let s = Arc::clone(&state);
    globals.push(spread_global(
        "models.getAvailableOfType",
        move |args, context| {
            let s = Arc::clone(&s);
            async move {
                let model_type = to_model_type(args.first())?;
                let provider = to_provider(args.get(1))?;
                let available = s
                    .models
                    .get_available_of_type(model_type, provider, &context.cancel)
                    .await?;
                let infos = available
                    .iter()
                    .map(to_model_info)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Some(Value::Array(infos)))
            }
        },
    ));

    let s = Arc::clone(&state);
    globals.push(spread_global("models.getModelOfType", move |args, _| {
        let s = Arc::clone(&s);
        async move {
            let (Some(Value::String(provider)), Some(Value::String(id))) =
                (args.get(1), args.get(2))
            else {
                let described: Vec<String> =
                    args.iter().map(|arg| describe_value(Some(arg))).collect();
                return Err(format!(
                    "models.getModelOfType(type, provider, id) expects three strings, got ({}). The provider and the id are separate arguments, for example models.getModelOfType(\"classifier\", \"typesafe\", \"jev-latest\").",
                    described.join(", ")
                ));
            };
            let model_type = to_model_type(args.first())?;
            s.models
                .get_model_of_type(model_type, provider, id)
                .map(|model| to_model_info(&model))
                .transpose()
        }
    }));

    let s = Arc::clone(&state);
    globals.push(spread_global("models.classify", move |args, context| {
        let s = Arc::clone(&s);
        async move { s.classify(&args, &context.cancel).await }
    }));

    let s = Arc::clone(&state);
    globals.push(spread_global(
        "models.generateImages",
        move |args, context| {
            let s = Arc::clone(&s);
            async move { s.generate_images(&args, &context.cancel).await }
        },
    ));
    globals
}

impl ModelsState {
    /// Resolve the script's model by provider and id only (`runModelCall`, `execute.ts:538-585`):
    /// a script-supplied baseUrl or headers must never receive the credentials.
    fn resolve(
        &self,
        name: &str,
        model_type: ModelType,
        model: Option<&Value>,
    ) -> Result<AnyModel, String> {
        let type_name = model_type.as_str();
        let list_hint = format!(
            "List the {type_name} models you can use with models.getAvailableOfType(\"{type_name}\")."
        );
        let identity =
            is_record(model).and_then(|map| match (map.get("provider"), map.get("id")) {
                (Some(Value::String(provider)), Some(Value::String(id))) => {
                    Some((provider.as_str(), id.as_str()))
                }
                _ => None,
            });
        let Some((provider, id)) = identity else {
            // undefined arrives as null: spread arguments cross the sandbox as a JSON array.
            let undefined_hint = if matches!(model, None | Some(Value::Null)) {
                " models.getModelOfType() returns undefined for an unknown provider or id."
            } else {
                ""
            };
            return Err(format!(
                "{name}() expects {} model as its first argument, got {}.{undefined_hint} {list_hint}",
                with_article(type_name),
                describe_value(model)
            ));
        };
        let reference = format!("{provider}/{id}");
        if let Some(resolved) = self.models.get_model_of_type(model_type, provider, id) {
            return Ok(resolved);
        }
        let actual = [ModelType::Chat, ModelType::Image, ModelType::Classifier]
            .into_iter()
            .find(|other| {
                *other != model_type
                    && self
                        .models
                        .get_model_of_type(*other, provider, id)
                        .is_some()
            });
        Err(match actual {
            Some(actual) => format!(
                "\"{reference}\" is {} model, not {} model. {list_hint}",
                with_article(actual.as_str()),
                with_article(type_name)
            ),
            None => format!("Unknown {type_name} model \"{reference}\". {list_hint}"),
        })
    }

    fn begin_row(&self, name: &str, model: &str) -> super::recorder::RowId {
        let number = self
            .call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        self.recorder.begin(CodemodeNestedCall {
            id: format!("{}/{name}/{number}", self.recorder.tool_call_id()),
            name: name.to_owned(),
            args: model.to_owned(),
            status: CodemodeNestedCallStatus::Running,
            duration_ms: None,
            error: None,
            cost: None,
        })
    }

    async fn classify(&self, args: &[Value], cancel: &CancelToken) -> crate::types::ToolResult {
        let name = "models.classify";
        let resolved = self.resolve(name, ModelType::Classifier, args.first())?;
        let AnyModel::Classifier(model) = resolved else {
            return Err(format!(
                "{name}() resolved a model that is not a classifier"
            ));
        };
        let context = check_classifier_context(args.get(1), &self.docs_path)?;

        let row = self.begin_row(name, &format!("{}/{}", model.provider, model.id));
        let started = std::time::Instant::now();
        let result = {
            let _permit = self.limiter.acquire().await.map_err(|e| e.to_string())?;
            self.models.classify(&model, &context, cancel).await
        };
        let status = match result.stop_reason {
            ClassifierStopReason::Stop => CodemodeNestedCallStatus::Ok,
            ClassifierStopReason::Aborted => CodemodeNestedCallStatus::Cancelled,
            ClassifierStopReason::Error => CodemodeNestedCallStatus::Error,
        };
        self.finish_row(
            row,
            started,
            status,
            result.error_message.as_deref(),
            result.usage.as_ref(),
        );
        serde_json::to_value(&result)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    async fn generate_images(
        &self,
        args: &[Value],
        cancel: &CancelToken,
    ) -> crate::types::ToolResult {
        let name = "models.generateImages";
        let resolved = self.resolve(name, ModelType::Image, args.first())?;
        let AnyModel::Image(model) = resolved else {
            return Err(format!(
                "{name}() resolved a model that is not an image model"
            ));
        };
        let context = check_images_context(args.get(1), &self.docs_path)?;

        let row = self.begin_row(name, &format!("{}/{}", model.provider, model.id));
        let started = std::time::Instant::now();
        let result = {
            let _permit = self.limiter.acquire().await.map_err(|e| e.to_string())?;
            let result = self.models.generate_images(&model, &context, cancel).await;
            self.recorder.add_generated_images(
                result
                    .output
                    .iter()
                    .filter(|block| matches!(block, Content::Image { .. }))
                    .count(),
            );
            result
        };
        let status = match result.stop_reason {
            ImagesStopReason::Stop => CodemodeNestedCallStatus::Ok,
            ImagesStopReason::Aborted => CodemodeNestedCallStatus::Cancelled,
            ImagesStopReason::Error => CodemodeNestedCallStatus::Error,
        };
        self.finish_row(
            row,
            started,
            status,
            result.error_message.as_deref(),
            result.usage.as_ref(),
        );
        serde_json::to_value(&result)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    fn finish_row(
        &self,
        row: super::recorder::RowId,
        started: std::time::Instant,
        status: CodemodeNestedCallStatus,
        error: Option<&str>,
        usage: Option<&cyrup_core::Usage>,
    ) {
        if let Some(usage) = usage {
            self.recorder.add_model_usage(usage);
        }
        self.recorder.update(row, |call| {
            call.duration_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
            call.status = status;
            if let Some(error) = error {
                call.error = Some(truncate_text(error, ERROR_PREVIEW_CHARS));
            }
            if let Some(usage) = usage {
                call.cost = Some(usage.cost.total);
            }
        });
    }
}

#[cfg(test)]
mod tests;
