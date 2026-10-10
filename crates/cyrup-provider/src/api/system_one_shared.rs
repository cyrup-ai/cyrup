//! What the two System One classifier apis share (1:1 port of pi
//! `packages/ai/src/api/system-one-shared.ts` @f1b2e77f5, PROV-104).
//!
//! TypeSafe's System One protocol answers every question of a request in one call: the body is
//! `{state, questions}` wrapped in the service's envelope, the reply is `{answers, usage}` wrapped in
//! the service's envelope, and each answer is already a probability — nothing is computed here but
//! the shape. Two services serve it: TypeSafe itself, OpenRouter and llama.cpp's `llama-server`
//! (`POST <base>/systemone`, [`super::typesafe_system_one`]), and Cloudflare Workers AI
//! (`POST <base>/run`, [`super::cloudflare_workers_ai_system_one`]). A [`SystemOneTransport`] is
//! the difference between them.
//!
//! **`bool` is `noul` on the wire.** pi's public question and answer type is `bool`; System One
//! calls it `noul`. [`wire_request`] renames a bool question's `type` to `noul` and keeps every
//! other member; [`parse_answers`] reads a `noul` answer's `noul` member as the bool's
//! `probability` (pi `wireRequest`, `parseAnswers`). llama.cpp's server accepts exactly
//! `choice`, `score` and `noul` (`parse_questions`, `tools/server/server-decision.cpp:164-198`
//! @b11436) and answers a noul with `{"type": "noul", "noul": <p(true)>}` (`format_answer`,
//! `:764-779`).
//!
//! `[CYRUP-DELTA, shape]` pi rejects a context carrying images before sending
//! (`if (context.images?.length) throw "<label> does not support image input"`). cyrup's
//! [`ClassifierContext`] has no `images` member yet, so there is nothing to reject.

use serde_json::{Map, Value};

use super::classifier_shared::{
    ClassifyError, format_error, parse_classifier_usage, post_classifier_request, required_number,
};
use crate::classifier::{
    ClassifierAnswer, ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierQuestion,
    ClassifierResult, ClassifierStopReason, KnownClassifierApi, OrderedMap,
};

/// Differences between services that serve System One models (pi `SystemOneTransport`).
#[derive(Clone, Copy)]
pub(crate) struct SystemOneTransport {
    /// Classifier API implemented by this transport.
    pub api: KnownClassifierApi,
    /// Service name used in error messages.
    pub label: &'static str,
    /// Absolute request URL.
    pub url: fn(&ClassifierModel) -> Result<String, ClassifyError>,
    /// Wraps the System One request (`{state, questions}`) in the service's request envelope.
    pub payload: fn(&ClassifierModel, Map<String, Value>) -> Value,
    /// Extracts the System One output (`{answers, usage}`) from the service's response envelope.
    pub output: fn(Value) -> Result<Map<String, Value>, ClassifyError>,
}

/// `new URL(path, `${model.baseUrl.replace(/\/+$/u, "")}/`)`: the base URL with its trailing
/// slashes collapsed to one, and `path` resolved against it as a WHATWG URL does. An unparsable
/// base fails the request as pi's `new URL` throws (`TypeError: Invalid URL`).
pub(crate) fn endpoint_url(base_url: &str, path: &str) -> Result<String, ClassifyError> {
    let base = format!("{}/", base_url.trim_end_matches('/'));
    reqwest::Url::parse(&base)
        .and_then(|base| base.join(path))
        .map(String::from)
        .map_err(|_| ClassifyError::plain("Invalid URL"))
}

/// pi `wireRequest`: `{state, questions}`, with every public `bool` question's `type` renamed to
/// the wire-level `noul` and every other member kept. Questions keep their order.
pub(crate) fn wire_request(context: &ClassifierContext) -> Map<String, Value> {
    let questions: Map<String, Value> = context
        .questions
        .iter()
        .map(|(id, question)| {
            let mut wire = serde_json::to_value(question).unwrap_or(Value::Null);
            if matches!(question, ClassifierQuestion::Bool { .. })
                && let Some(object) = wire.as_object_mut()
            {
                object.insert("type".to_string(), Value::from("noul"));
            }
            (id.to_string(), wire)
        })
        .collect();
    let mut request = Map::new();
    request.insert("state".to_string(), Value::Object(context.state.clone()));
    request.insert("questions".to_string(), Value::Object(questions));
    request
}

/// pi `probabilities`: an object of finite numbers, in the object's own key order.
fn probabilities(
    label: &str,
    value: Option<&Value>,
    id: &str,
) -> Result<OrderedMap<f64>, ClassifyError> {
    let Some(object) = value.and_then(Value::as_object) else {
        return Err(ClassifyError::plain(format!(
            "{label} returned invalid probabilities for {id}"
        )));
    };
    let mut probabilities = OrderedMap::new();
    for (key, probability) in object {
        let probability = required_number(
            label,
            Some(probability),
            &format!("probability for {id}.{key}"),
        )?;
        probabilities.insert(key.clone(), probability);
    }
    Ok(probabilities)
}

/// pi `parseAnswers`: one answer per question of the REQUEST, in the request's order, each checked
/// against its question's type. A `noul` answer becomes the public `{type: "bool", probability}`.
/// Extra answers the service sends are ignored, and so are the extra members of an answer
/// (llama.cpp's score answers carry `legend` and `probabilities`).
pub(crate) fn parse_answers(
    label: &str,
    value: Option<&Value>,
    context: &ClassifierContext,
) -> Result<OrderedMap<ClassifierAnswer>, ClassifyError> {
    let Some(value) = value.and_then(Value::as_object) else {
        return Err(ClassifyError::plain(format!(
            "{label} returned an unexpected response"
        )));
    };
    let mut answers = OrderedMap::new();
    for (id, question) in context.questions.iter() {
        let Some(answer) = value.get(id).and_then(Value::as_object) else {
            return Err(ClassifyError::plain(format!(
                "{label} did not return an answer for {id}"
            )));
        };
        let kind = answer.get("type").and_then(Value::as_str);
        let parsed = match question {
            ClassifierQuestion::Choice { .. } => {
                let choice = answer.get("choice").and_then(Value::as_str);
                let (Some("choice"), Some(choice)) = (kind, choice) else {
                    return Err(ClassifyError::plain(format!(
                        "{label} did not return a choice answer for {id}"
                    )));
                };
                ClassifierAnswer::Choice {
                    choice: choice.to_string(),
                    probabilities: probabilities(label, answer.get("probabilities"), id)?,
                    confidence: required_number(
                        label,
                        answer.get("confidence"),
                        &format!("confidence for {id}"),
                    )?,
                }
            }
            ClassifierQuestion::Score { .. } => {
                if kind != Some("score") {
                    return Err(ClassifyError::plain(format!(
                        "{label} did not return a score answer for {id}"
                    )));
                }
                ClassifierAnswer::Score {
                    score: required_number(label, answer.get("score"), &format!("score for {id}"))?,
                    confidence: required_number(
                        label,
                        answer.get("confidence"),
                        &format!("confidence for {id}"),
                    )?,
                }
            }
            ClassifierQuestion::Bool { .. } => {
                if kind != Some("noul") {
                    return Err(ClassifyError::plain(format!(
                        "{label} did not return a bool answer for {id}"
                    )));
                }
                ClassifierAnswer::Bool {
                    probability: required_number(
                        label,
                        answer.get("noul"),
                        &format!("probability for {id}"),
                    )?,
                }
            }
        };
        answers.insert(id, parsed);
    }
    Ok(answers)
}

/// pi `classifySystemOne`: runs one System One classification over `transport`. Never fails as a
/// call; every failure is an `error` (or, once cancelled, `aborted`) result.
///
/// Usage is set BEFORE the answers are parsed, because a request whose answers are malformed was
/// still billed: such a result is an error that keeps its usage.
pub(crate) async fn classify_system_one(
    transport: SystemOneTransport,
    model: &ClassifierModel,
    context: &ClassifierContext,
    options: &ClassifierOptions,
) -> ClassifierResult {
    let mut output = ClassifierResult::new(model);
    let outcome: Result<OrderedMap<ClassifierAnswer>, ClassifyError> = async {
        if model.api.as_str() != transport.api.as_str() {
            return Err(ClassifyError::plain(format!(
                "Unsupported classifier API: {}",
                model.api
            )));
        }
        let url = (transport.url)(model)?;
        let payload = (transport.payload)(model, wire_request(context));
        let body = post_classifier_request(transport.label, &url, model, payload, options).await?;
        let result = (transport.output)(body)?;
        if let Some(usage) = parse_classifier_usage(result.get("usage"), model) {
            output.usage = Some(usage);
        }
        parse_answers(transport.label, result.get("answers"), context)
    }
    .await;
    match outcome {
        Ok(answers) => output.answers = answers,
        Err(error) => {
            output.stop_reason = if options.is_aborted() {
                ClassifierStopReason::Aborted
            } else {
                ClassifierStopReason::Error
            };
            output.error_message = Some(format_error(transport.label, &error));
        }
    }
    output
}
