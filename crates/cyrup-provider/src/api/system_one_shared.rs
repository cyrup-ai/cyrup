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
//! calls it `noul`. [`wire_request`] sends each question as `{type, instructions, criteria}` with a
//! bool's `type` spelled `noul` (`wire_question`, where the PROV-156 delta on extra members is
//! recorded); [`parse_answers`] reads a `noul` answer's `noul` member as the bool's
//! `probability` (pi `wireRequest`, `parseAnswers`). llama.cpp's server accepts exactly
//! `choice`, `score` and `noul` (`parse_questions`, `tools/server/server-decision.cpp:137-207`
//! @b11436) and answers a noul with `{"type": "noul", "noul": <p(true)>}` (`format_answer`,
//! `:764-779`).
//!
//! A context carrying images is rejected before anything is sent, as pi does
//! (`if (context.images?.length) throw "<label> does not support image input"`,
//! `system-one-shared.ts:116` @f1b2e77f5): neither System One transport takes image input.

use serde_json::{Map, Value};

use super::classifier_shared::{
    ClassifyError, format_error, js_key_order_map, parse_classifier_usage, post_classifier_request,
    required_number,
};
use crate::classifier::{
    BoolCriteria, ClassifierAnswer, ClassifierContext, ClassifierModel, ClassifierOptions,
    ClassifierQuestion, ClassifierResult, ClassifierStopReason, KnownClassifierApi, OrderedMap,
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

/// pi `wireRequest` (`system-one-shared.ts:85-96` @f1b2e77f5): `{state, questions}`, every
/// question in [`wire_question`]'s shape. Questions keep their order.
pub(crate) fn wire_request(context: &ClassifierContext) -> Map<String, Value> {
    let questions: Map<String, Value> = context
        .questions
        .iter()
        .map(|(id, question)| (id.to_string(), wire_question(question)))
        .collect();
    let mut request = Map::new();
    // `state: context.state` is a JS object, so it is already in own-key order when the payload is
    // handed to `onPayload` (PROV-110 corner (1)); the body is serialized in that order too.
    request.insert(
        "state".to_string(),
        Value::Object(js_key_order_map(&context.state)),
    );
    request.insert("questions".to_string(), Value::Object(questions));
    request
}

/// One question as System One reads it: `{type, instructions, criteria}` — pi's declared member
/// order for every variant (`types.ts:662-678` @f1b2e77f5) — with `bool` spelled `noul` and a
/// bool's criteria as `{true, false}`. Every field is named (no `..`), so a field added to a
/// variant does not compile here until its wire spelling is decided.
///
/// `[CYRUP-DELTA, intentional]` PROV-156: pi sends the question OBJECT it was handed (`question`,
/// or for a bool `{ ...question, type: "noul" }`), so a JSON caller's member order and any member
/// the type does not name reach the server. cyrup's question is typed at the parse boundary, which
/// is where the delta is recorded (`cyrup-codemode-runtime`'s `check_classifier_context`). On this
/// wire it is the observable half: the Workers AI `typesafe/jev` input schema closes every question
/// object and a noul's criteria (`additionalProperties: false`), so a service validating against it
/// would refuse pi's request with an extra member and answers cyrup's; llama.cpp reads the three
/// members by name (`parse_questions`, `server-decision.cpp:137-207` @b11436) and Clef's schema
/// leaves them open, so there an extra changes nothing.
fn wire_question(question: &ClassifierQuestion) -> Value {
    let (kind, instructions, criteria) = match question {
        ClassifierQuestion::Choice {
            instructions,
            criteria,
        } => (
            "choice",
            instructions,
            Value::Object(
                criteria
                    .iter()
                    .map(|(label, meaning)| (label.to_owned(), Value::from(meaning.as_str())))
                    .collect(),
            ),
        ),
        ClassifierQuestion::Score {
            instructions,
            criteria,
        } => ("score", instructions, Value::from(criteria.clone())),
        ClassifierQuestion::Bool {
            instructions,
            criteria:
                BoolCriteria {
                    when_true,
                    when_false,
                },
        } => (
            "noul",
            instructions,
            Value::Object(Map::from_iter([
                ("true".to_owned(), Value::from(when_true.as_str())),
                ("false".to_owned(), Value::from(when_false.as_str())),
            ])),
        ),
    };
    Value::Object(Map::from_iter([
        ("type".to_owned(), Value::from(kind)),
        (
            "instructions".to_owned(),
            Value::from(instructions.as_str()),
        ),
        ("criteria".to_owned(), criteria),
    ]))
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
        if context.has_images() {
            return Err(ClassifyError::plain(format!(
                "{} does not support image input",
                transport.label
            )));
        }
        let url = (transport.url)(model)?;
        let payload = (transport.payload)(model, wire_request(context));
        let body =
            post_classifier_request(transport.label, &url, model, payload, options, &[]).await?;
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
