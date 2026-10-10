//! The SYSTEM ONE wire: `POST /v1/systemone`, llama.cpp's native endpoint for typed decision
//! models (Laya, Kev, lev, OpenJev, Nimble, Clef), which `cyrup_provider::api::typesafe_system_one`
//! drives (PROV-104, EXT-110).
//!
//! Registered at `tools/server/server.cpp:289` @b11436 (`ctx_http.post("/v1/systemone", ...)`); in
//! router mode it is `proxy_post` to the child the body's `model` names (`:241`), so the child's
//! answer reaches the client verbatim. The handler is `post_systemone`
//! (`server-context.cpp:5583-5667`), the question parsing and the answer shapes are
//! `server_decision_context::parse_questions` and `format_answer` (`server-decision.cpp:137-205`,
//! `:731-804`).
//!
//! The request is TypeSafe's System One body, `{"model", "state", "questions"}`, where a question's
//! `type` is `choice`, `score` or `noul` — the wire spelling of pi's public `bool`
//! (`system-one-shared.ts` `wireRequest` @f1b2e77f5). The answer is
//! `{"model", "answers": {<id>: <answer>, ...}, "usage": {"input_tokens", "output_tokens": 0}}`,
//! answers in the order of the request's questions.
//!
//! What the client reads, and what these shapes therefore have to get right:
//! `answers.<id>.type`, `choice` / `probabilities` / `confidence` for a choice, `score` /
//! `confidence` for a score, `noul` for a noul, and `usage.input_tokens` / `output_tokens`. A score
//! answer also carries `legend` and `probabilities`, which pi ignores; they are here because a real
//! server sends them.
//!
//! A LIVE b11436 router over `tinylaya-for-testing-Q8_0.gguf` (`output_modalities: ["decisions"]`)
//! answered one choice, one score and one noul question with exactly these keys in exactly this
//! order; [`crate::golden::SYSTEMONE_TINYLAYA_LIVE`] is that answer, byte for byte.

use serde_json::{Map, Value, json};

/// A `choice` answer (`format_answer`, `server-decision.cpp:762`, `:786-790` @b11436):
/// `{"type": "choice", "choice", "probabilities", "confidence"}`. `choice` is the key with the
/// highest probability; `probabilities` holds every option's key in the order the request listed
/// them (`parse_questions`, `:169-171`, unless the model sorts them, `:172-176`).
pub fn choice_answer(choice: &str, probabilities: &[(&str, f64)], confidence: f64) -> Value {
    let probabilities: Map<String, Value> = probabilities
        .iter()
        .map(|(key, probability)| ((*key).to_string(), json!(probability)))
        .collect();
    json!({
        "type": "choice",
        "choice": choice,
        "probabilities": probabilities,
        "confidence": confidence,
    })
}

/// A `score` answer (`format_answer`, `server-decision.cpp:762`, `:791-801` @b11436):
/// `{"type": "score", "score", "legend", "probabilities", "confidence"}`. `score` is the expected
/// level, `legend` maps each level index (`"0"`, `"1"`, ...) to the request's criterion text, and
/// `probabilities` maps each level index to its probability.
pub fn score_answer(score: f64, levels: &[&str], probabilities: &[f64], confidence: f64) -> Value {
    let legend: Map<String, Value> = levels
        .iter()
        .enumerate()
        .map(|(index, level)| (index.to_string(), json!(level)))
        .collect();
    let probabilities: Map<String, Value> = probabilities
        .iter()
        .enumerate()
        .map(|(index, probability)| (index.to_string(), json!(probability)))
        .collect();
    json!({
        "type": "score",
        "score": score,
        "legend": legend,
        "probabilities": probabilities,
        "confidence": confidence,
    })
}

/// A `noul` answer (`format_answer`, `server-decision.cpp:762`, `:764-779` @b11436):
/// `{"type": "noul", "noul": <probability of "true">}`. pi maps it to its public
/// `{"type": "bool", "probability"}` (`system-one-shared.ts` `parseAnswers`).
pub fn noul_answer(noul: f64) -> Value {
    json!({ "type": "noul", "noul": noul })
}

/// The whole `POST /v1/systemone` answer (`post_systemone`, `server-context.cpp:5658-5665`
/// @b11436): `{"model", "answers", "usage": {"input_tokens", "output_tokens": 0}}`. No token is
/// generated, so `output_tokens` is always `0`; `input_tokens` is the prompt tokens of every task
/// the questions took.
pub fn response(model: &str, answers: &[(&str, Value)], input_tokens: u64) -> Value {
    let answers: Map<String, Value> = answers
        .iter()
        .map(|(id, answer)| ((*id).to_string(), answer.clone()))
        .collect();
    json!({
        "model": model,
        "answers": answers,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": 0,
        },
    })
}

/// `POST /v1/systemone` to a model that is not a decision model: `501 not_supported_error`,
/// "This model is not a decision model" (`server-context.cpp:5586-5588`,
/// `ERROR_TYPE_NOT_SUPPORTED`, `server-common.cpp:60-63` @b11436). The live b11436 router answered
/// exactly this for `stories260K`.
pub fn not_a_decision_model() -> (u16, Value) {
    (
        501,
        crate::router::error_body(
            501,
            "This model is not a decision model",
            "not_supported_error",
        ),
    )
}
