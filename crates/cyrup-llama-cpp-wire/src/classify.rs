//! The CLASSIFIER wire: the three endpoints `cyrup_provider::api::llama_cpp_classify` drives,
//! `POST /tokenize`, `POST /apply-template` and `POST /completion` with `n_probs` (registered at
//! `tools/server/server.cpp:290`, `:292` and `:270` respectively @b11436; in router mode all three
//! are `proxy_post` to the child named by the body's `model`, `:242`, `:244` and `:229`, so the
//! child's answer reaches the client verbatim).
//!
//! What the api reads, and what these shapes therefore have to get right: `tokens[]` as bare ids
//! or `{id}` objects (`llama_cpp_classify.rs` `token_ids`), `prompt` (`render_prompt`), and
//! `completion_probabilities[0].top_logprobs[].{id, logprob}` (`next_token_logprobs`), having sent
//! `post_sampling_probs: false` itself, which is the switch between `logprob`/`top_logprobs` and
//! `prob`/`top_probs`. Everything else below is sent by a real server and read by nobody; it is
//! here so a fake answering from this module sends what llama.cpp sends.

use serde_json::{Value, json};

/// `POST /tokenize` with the default `with_pieces: false`: `{"tokens": [<int>, ...]}`
/// (`post_tokenize`, `server-context.cpp:5440-5479` @b11436 — `tokens_response = tokens` at
/// `:5473`, `res->ok(json{{"tokens", ...}})` at `:5477`). The api sends `add_special: false` and
/// `parse_special: false`; the server's defaults are `false` and `true` (`:5445-5446`).
pub fn tokenize(tokens: &[i64]) -> Value {
    json!({ "tokens": tokens })
}

/// `POST /tokenize` with `with_pieces: true`: each token is `{"id", "piece"}`, the piece a string
/// when it is valid UTF-8 and otherwise its bytes as an array of ints (`server-context.cpp:5451-
/// 5470` @b11436). The api never asks for pieces but accepts this form (`id` is read off an
/// object token), so it is the one object-token shape a real server can send.
pub fn tokenize_with_pieces(pieces: &[(i64, &str)]) -> Value {
    let tokens: Vec<Value> = pieces
        .iter()
        .map(|(id, piece)| json!({ "id": id, "piece": piece }))
        .collect();
    json!({ "tokens": tokens })
}

/// `POST /apply-template`: `{"prompt": <the rendered chat>}` and nothing else
/// (`post_apply_template`, `server-context.cpp:5416-5425` @b11436 — `res->ok({{"prompt",
/// data.at("prompt")}})` at `:5424`). The rendering is the model's chat template, which a fake
/// stands in for.
pub fn apply_template(prompt: &str) -> Value {
    json!({ "prompt": prompt })
}

/// One ranked candidate for the next token.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TokenLogprob<'a> {
    /// The vocabulary id.
    pub id: i64,
    /// The token's text.
    pub token: &'a str,
    /// Its natural-log probability before sampling.
    pub logprob: f64,
}

fn token_bytes(token: &str) -> Vec<u8> {
    token.as_bytes().to_vec()
}

/// One `top_logprobs` entry, `completion_token_output::to_json(post_sampling_probs = false)`
/// (`server-task.cpp:264-280` @b11436): `{"id", "token", "bytes", "logprob"}`, in that order.
/// `bytes` is the token text's raw bytes (`str_to_bytes`, `:309-315`); `token` is the same text cut
/// back to valid UTF-8 (`validate_utf8`, `:267-268`), which for the whole-character tokens used
/// here is the text itself.
pub fn top_logprob(candidate: TokenLogprob<'_>) -> Value {
    json!({
        "id": candidate.id,
        "token": candidate.token,
        "bytes": token_bytes(candidate.token),
        "logprob": candidate.logprob,
    })
}

/// `completion_probabilities` for a ONE-token prediction,
/// `completion_token_output::probs_vector_to_json(post_sampling_probs = false)`
/// (`server-task.cpp:282-301` @b11436): one entry per predicted token, the SAMPLED token's own
/// `{id, token, bytes, logprob}` followed by its `top_logprobs`, the `n_probs` best candidates in
/// rank order.
///
/// The fakes this replaced omitted the outer entry's `bytes` and `logprob` and sent `bytes: []`
/// for every candidate; nothing reads either, but a real server always sends both, filled.
pub fn completion_probabilities(sampled: TokenLogprob<'_>, top: &[TokenLogprob<'_>]) -> Value {
    let top: Vec<Value> = top.iter().copied().map(top_logprob).collect();
    json!([{
        "id": sampled.id,
        "token": sampled.token,
        "bytes": token_bytes(sampled.token),
        "logprob": sampled.logprob,
        "top_logprobs": top,
    }])
}

/// The non-streamed `POST /completion` answer for `n_predict: 1`,
/// `server_task_result_cmpl_final::to_json_non_oaicompat` (`server-task.cpp:340-363` @b11436),
/// with `completion_probabilities` attached because the request asked for `n_probs` and did not
/// stream (`:359-361`). Every top-level key a real server sends is present, in its order.
/// `generation_settings` and `timings` are objects whose contents nothing in this workspace reads,
/// so they are left empty rather than invented.
///
/// At `temperature: 0` (which the api sends) the sampled token is the top candidate, so `content`
/// is `sampled.token`; one predicted token stops on the `n_predict` limit, so `stop_type` is
/// `"limit"` (`stop_type_to_str`, `:251-258`). `tokens` is `generated_tokens`, empty unless the
/// request set `return_tokens` (`server-context.cpp:2238-2241`). `tokens_evaluated` is the
/// prompt's token count (`n_prompt_tokens = slot.task->n_tokens()`, `:2249`) and `tokens_cached`
/// is the slot's cached prompt (`slot.prompt.n_tokens()`, `:2251`), which after a one-token
/// prediction holds exactly the prompt, so the two are equal. `prompt` is the prompt's tokens
/// detokenized with special tokens (`:2244`): a fake has no tokenizer and echoes the text, which
/// differs only by a BOS the model adds. A live b11436 child (stories260K, `n_predict: 1`,
/// `n_probs: 2`, `post_sampling_probs: false`) answered these keys in this order, with
/// `tokens: []`, `stop: true`, `has_new_line: false`, `truncated: false`, `stop_type: "limit"`,
/// `stopping_word: ""` and `tokens_cached == tokens_evaluated`. Pinned in full by
/// [`crate::golden::COMPLETION_QWEN_P_B_OVER_A`].
pub fn completion(
    model: &str,
    prompt: &str,
    prompt_tokens: u64,
    sampled: TokenLogprob<'_>,
    top: &[TokenLogprob<'_>],
) -> Value {
    json!({
        "index": 0,
        "content": sampled.token,
        "tokens": [],
        "id_slot": 0,
        "stop": true,
        "model": model,
        "tokens_predicted": 1,
        "tokens_evaluated": prompt_tokens,
        "generation_settings": {},
        "prompt": prompt,
        "has_new_line": false,
        "truncated": false,
        "stop_type": "limit",
        "stopping_word": "",
        "tokens_cached": prompt_tokens,
        "timings": {},
        "completion_probabilities": completion_probabilities(sampled, top),
    })
}
