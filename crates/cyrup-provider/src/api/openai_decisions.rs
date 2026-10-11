//! The `openai-decisions` classifier api (1:1 port of pi `packages/ai/src/api/openai-decisions.ts`
//! and its `openai-decisions.lazy.ts` wrapper @f1b2e77f5, PROV-147).
//!
//! OpenAI's Decisions API: `POST <baseUrl>/decisions` with `{ model, input, questions }`
//! (<https://developers.openai.com/api/docs/guides/decisions>). Upstream's header, verbatim:
//!
//! > The state is sent as JSON text. With images, the input becomes one user message with the
//! > state as `input_text` followed by `input_image` data URLs. Questions map to Decisions types:
//! > `choice` to `choice`, `score` to `score`, and `bool` to `predicate`. Predicates have no
//! > criteria field, so the meanings of true and false are appended to the instructions.
//! >
//! > Only OpenAI API keys work: Sign in with ChatGPT tokens are rejected on this route.
//!
//! That last line is why the `openai` provider hides its classifier rows from an OAuth credential
//! ([`crate::providers::openai::filter_openai_all_models`], pi `filterAllModels`,
//! `providers/openai.ts:27-29`).
//!
//! The state text is `JSON.stringify(context.state)` as JS prints it — own-key order and number
//! text included ([`super::classifier_shared::json_stringify`]; PROV-110 corners (1) and (2)),
//! because it is the text the model reads.
//!
//! pi's `ClassifierOptions.temperature` is ignored: the Decisions request has no temperature field
//! (pi's test passes `1.5` and asserts the body has none).
//!
//! `[CYRUP-DELTA, mechanism]` — `lazy.ts` defers the module import to the first call; Rust has
//! nothing to defer, so [`openai_decisions_api`] returns the implementation directly, the same
//! substitution [`super::typesafe_system_one`] documents.

use std::sync::Arc;

use cyrup_core::Content;
use serde_json::{Map, Value, json};

use super::classifier_shared::{
    ClassifyError, format_error, json_stringify, parse_classifier_usage, post_classifier_request,
    required_number,
};
use super::system_one_shared::endpoint_url;
use crate::classifier::{
    ClassifierAnswer, ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierQuestion,
    ClassifierResult, ClassifierStopReason, KnownClassifierApi, OrderedMap, ProviderClassifier,
};

/// Service name in error text (`const LABEL = "OpenAI Decisions"`, `:30`).
const LABEL: &str = "OpenAI Decisions";

/// "The endpoint accepts at most this many image parts per request." (`MAX_IMAGES`, `:32-33`).
pub const MAX_IMAGES: usize = 128;

/// Statuses that fail at once although the provider retry policy would retry them
/// (`NO_RETRY_STATUSES`, `:146-151`), with upstream's reason: "Cloudflare in front of
/// api.openai.com answers 504 with an HTML page when a request runs longer than about five seconds.
/// Large inputs, currently above roughly 600K tokens, hit this limit, and retrying the same input
/// runs into it again, so 504 is not retried."
pub const NO_RETRY_STATUSES: &[u16] = &[504];

/// `predicateInstructions` (`:43-49`): the bool's instructions with each non-empty meaning
/// appended, `True means: …` then `False means: …`.
fn predicate_instructions(instructions: &str, when_true: &str, when_false: &str) -> String {
    let meanings: Vec<String> = [
        (!when_true.is_empty()).then(|| format!("True means: {when_true}")),
        (!when_false.is_empty()).then(|| format!("False means: {when_false}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if meanings.is_empty() {
        instructions.to_string()
    } else {
        format!("{instructions}\n\n{}", meanings.join("\n"))
    }
}

/// `wireQuestion` (`:51-71`), in the object-literal key order pi writes (`type`, `name`,
/// `instructions`, then `choices` / `levels`). A choice option with an empty description is sent
/// as `{value}` alone (`description ? {value, description} : {value}`).
fn wire_question(name: &str, question: &ClassifierQuestion) -> Value {
    match question {
        ClassifierQuestion::Choice {
            instructions,
            criteria,
        } => {
            let choices: Vec<Value> = criteria
                .iter()
                .map(|(value, description)| {
                    if description.is_empty() {
                        json!({ "value": value })
                    } else {
                        json!({ "value": value, "description": description })
                    }
                })
                .collect();
            json!({
                "type": "choice",
                "name": name,
                "instructions": instructions,
                "choices": choices,
            })
        }
        ClassifierQuestion::Score {
            instructions,
            criteria,
        } => {
            let levels: Vec<Value> = criteria
                .iter()
                .map(|label| json!({ "label": label }))
                .collect();
            json!({
                "type": "score",
                "name": name,
                "instructions": instructions,
                "levels": levels,
            })
        }
        ClassifierQuestion::Bool {
            instructions,
            criteria,
        } => json!({
            "type": "predicate",
            "name": name,
            "instructions": predicate_instructions(
                instructions,
                &criteria.when_true,
                &criteria.when_false,
            ),
        }),
    }
}

/// `wireInput` (`:73-92`): the state as JSON text, or — with images — one user message holding
/// that text as `input_text` and each image as an `input_image` data URL, in order. More than
/// [`MAX_IMAGES`] images fail before anything is sent.
///
/// `[CYRUP-DELTA, type]` pi types `images` as `ImageContent[]`, so a non-image entry is
/// unrepresentable there (at runtime it would print `data:undefined;base64,undefined`). cyrup's
/// `images` carries [`Content`] blocks, which can be text (PROV-148), so a non-image entry is
/// refused with an error result here instead of being sent as a broken data URL.
fn wire_input(context: &ClassifierContext) -> Result<Value, ClassifyError> {
    let state = json_stringify(&Value::Object(context.state.clone()))?;
    let images = context.images.as_deref().unwrap_or_default();
    if images.is_empty() {
        return Ok(Value::String(state));
    }
    if images.len() > MAX_IMAGES {
        return Err(ClassifyError::plain(format!(
            "{LABEL} accepts at most {MAX_IMAGES} images, got {}",
            images.len()
        )));
    }
    let mut content = vec![json!({ "type": "input_text", "text": state })];
    for image in images {
        let Content::Image { data, mime_type } = image else {
            return Err(ClassifyError::plain(format!(
                "{LABEL} accepts only image content in images"
            )));
        };
        content.push(json!({
            "type": "input_image",
            "image_url": format!("data:{mime_type};base64,{data}"),
        }));
    }
    Ok(json!([{ "role": "user", "content": content }]))
}

/// `choiceProbabilities` (`:94-104`): an array of `{value, probability}` entries, read into an
/// object in entry order (`Object.fromEntries`: a repeated value keeps its first position and takes
/// the last probability, which [`OrderedMap::insert`] also does).
fn choice_probabilities(value: Option<&Value>, id: &str) -> Result<OrderedMap<f64>, ClassifyError> {
    let invalid =
        || ClassifyError::plain(format!("{LABEL} returned invalid probabilities for {id}"));
    let entries = value.and_then(Value::as_array).ok_or_else(invalid)?;
    let mut probabilities = OrderedMap::new();
    for entry in entries {
        let entry = entry.as_object().ok_or_else(invalid)?;
        let choice = entry
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        let probability = required_number(
            LABEL,
            entry.get("probability"),
            &format!("probability for {id}.{choice}"),
        )?;
        probabilities.insert(choice, probability);
    }
    Ok(probabilities)
}

/// `parseAnswer` (`:106-129`): a refusal fails the result for any question type; otherwise the
/// answer's type must match the question's (`predicate` for a bool) and its fields must be finite
/// numbers. A score answer's own `probabilities` are dropped, as pi drops them.
fn parse_answer(
    id: &str,
    question: &ClassifierQuestion,
    answer: &Map<String, Value>,
) -> Result<ClassifierAnswer, ClassifyError> {
    let kind = answer.get("type").and_then(Value::as_str);
    if kind == Some("refusal") {
        return Err(ClassifyError::plain(format!(
            "{LABEL} refused to answer {id}"
        )));
    }
    match question {
        ClassifierQuestion::Choice { .. } => {
            let choice = answer.get("choice").and_then(Value::as_str);
            let (Some("choice"), Some(choice)) = (kind, choice) else {
                return Err(ClassifyError::plain(format!(
                    "{LABEL} did not return a choice answer for {id}"
                )));
            };
            Ok(ClassifierAnswer::Choice {
                choice: choice.to_string(),
                probabilities: choice_probabilities(answer.get("probabilities"), id)?,
                confidence: required_number(
                    LABEL,
                    answer.get("confidence"),
                    &format!("confidence for {id}"),
                )?,
            })
        }
        ClassifierQuestion::Score { .. } => {
            if kind != Some("score") {
                return Err(ClassifyError::plain(format!(
                    "{LABEL} did not return a score answer for {id}"
                )));
            }
            Ok(ClassifierAnswer::Score {
                score: required_number(LABEL, answer.get("score"), &format!("score for {id}"))?,
                confidence: required_number(
                    LABEL,
                    answer.get("confidence"),
                    &format!("confidence for {id}"),
                )?,
            })
        }
        ClassifierQuestion::Bool { .. } => {
            if kind != Some("predicate") {
                return Err(ClassifyError::plain(format!(
                    "{LABEL} did not return a predicate answer for {id}"
                )));
            }
            Ok(ClassifierAnswer::Bool {
                probability: required_number(
                    LABEL,
                    answer.get("probability"),
                    &format!("probability for {id}"),
                )?,
            })
        }
    }
}

/// `parseAnswers` (`:131-144`): answers arrive as an ARRAY matched to the questions by `name`, in
/// any order; one answer per question of the REQUEST, in the request's order. Entries that are not
/// objects with a string `name` are ignored, and a later entry for the same name wins
/// (`byName.set`).
fn parse_answers(
    value: Option<&Value>,
    context: &ClassifierContext,
) -> Result<OrderedMap<ClassifierAnswer>, ClassifyError> {
    let Some(entries) = value.and_then(Value::as_array) else {
        return Err(ClassifyError::plain(format!(
            "{LABEL} returned an unexpected response"
        )));
    };
    let mut by_name: Vec<(&str, &Map<String, Value>)> = Vec::new();
    for entry in entries {
        let Some(answer) = entry.as_object() else {
            continue;
        };
        let Some(name) = answer.get("name").and_then(Value::as_str) else {
            continue;
        };
        match by_name.iter_mut().find(|(seen, _)| *seen == name) {
            Some(slot) => slot.1 = answer,
            None => by_name.push((name, answer)),
        }
    }
    let mut answers = OrderedMap::new();
    for (id, question) in context.questions.iter() {
        let Some((_, answer)) = by_name.iter().find(|(name, _)| *name == id) else {
            return Err(ClassifyError::plain(format!(
                "{LABEL} did not return an answer for {id}"
            )));
        };
        answers.insert(id, parse_answer(id, question, answer)?);
    }
    Ok(answers)
}

/// `errorMessage` (`:153-158`): a gateway `504` is explained instead of returning Cloudflare's
/// HTML page; every other failure renders as the other classifier apis render theirs.
fn error_message(error: &ClassifyError) -> String {
    if error.status() == Some(504) {
        return format!(
            "{LABEL} error (504): the request timed out at the gateway. Very large inputs (above \
             roughly 600K tokens) currently exceed its time limit."
        );
    }
    format_error(LABEL, error)
}

/// The body of `classify`'s `try` (`:172-190`), in pi's order: the api check, the URL, the request
/// body (whose image cap fails before the api-key check inside the post), the post, usage set
/// BEFORE the answers are parsed ("a request with malformed or refused answers was still billed"),
/// then the answers.
async fn run(
    model: &ClassifierModel,
    context: &ClassifierContext,
    options: &ClassifierOptions,
    output: &mut ClassifierResult,
) -> Result<OrderedMap<ClassifierAnswer>, ClassifyError> {
    if model.api.as_str() != KnownClassifierApi::OpenAiDecisions.as_str() {
        return Err(ClassifyError::plain(format!(
            "Unsupported classifier API: {}",
            model.api
        )));
    }
    let url = endpoint_url(&model.base_url, "decisions")?;
    let questions: Vec<Value> = context
        .questions
        .iter()
        .map(|(id, question)| wire_question(id, question))
        .collect();
    let body = json!({
        "model": model.id.as_str(),
        "input": wire_input(context)?,
        "questions": questions,
    });
    let body =
        post_classifier_request(LABEL, &url, model, body, options, NO_RETRY_STATUSES).await?;
    let Value::Object(body) = body else {
        return Err(ClassifyError::plain(format!(
            "{LABEL} returned an unexpected response"
        )));
    };
    if let Some(usage) = parse_classifier_usage(body.get("usage"), model) {
        output.usage = Some(usage);
    }
    parse_answers(body.get("answers"), context)
}

/// Classification through OpenAI's Decisions API (pi `classify`, `:160-197`). Never fails as a
/// call; every failure is an `error` (or, once cancelled, `aborted`) result with no answers.
struct OpenAiDecisions;

#[async_trait::async_trait]
impl ProviderClassifier for OpenAiDecisions {
    async fn classify(
        &self,
        model: &ClassifierModel,
        context: &ClassifierContext,
        options: &ClassifierOptions,
    ) -> ClassifierResult {
        let mut output = ClassifierResult::new(model);
        match run(model, context, options, &mut output).await {
            Ok(answers) => output.answers = answers,
            Err(error) => {
                output.answers = OrderedMap::new();
                output.stop_reason = if options.is_aborted() {
                    ClassifierStopReason::Aborted
                } else {
                    ClassifierStopReason::Error
                };
                output.error_message = Some(error_message(&error));
            }
        }
        output
    }
}

/// The `openai-decisions` implementation to register under
/// [`KnownClassifierApi::OpenAiDecisions`] (pi `openAIDecisionsApi`, `openai-decisions.lazy.ts`).
pub fn openai_decisions_api() -> Arc<dyn ProviderClassifier> {
    Arc::new(OpenAiDecisions)
}
