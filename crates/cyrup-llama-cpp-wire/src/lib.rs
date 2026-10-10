//! The llama.cpp `llama-server` wire, ONE definition, grounded in llama.cpp's own server source
//! (EXT-100, EXT-108). A TEST-ONLY crate: nothing depends on it except through
//! `[dev-dependencies]`.
//!
//! # Why this crate exists
//!
//! Every llama.cpp test in this workspace runs against a FAKE server, and so does pi's
//! (`packages/coding-agent/test/llama-extension.test.ts`, an in-process `node:http`
//! `createServer`). A fake is written from a reading of the client, so a wire shape the fake and
//! the client agree on passes every test even when the real server does something else. This
//! crate is the fix available without a live server: the shapes llama.cpp's own handlers build,
//! transcribed with a file:line citation each, in ONE place, so the fakes answering from it cannot
//! drift apart and an auditor can re-derive every shape from the pin below.
//!
//! There are three llama-server fakes:
//!
//! * `cyrup-llama/src/tests/fake_server.rs` — the router management api. Answers from
//!   [`router`].
//! * `cyrup-it/tests/llama/fake.rs` — both halves behind one port, for the seam tests. Answers from
//!   [`router`] and [`classify`].
//! * `cyrup-provider/src/tests/llama_cpp_classify_fake_server.rs` — the classifier apis. Answers
//!   from [`classify`], [`systemone`] (`POST /v1/systemone`, PROV-104) and
//!   [`router::file_not_found`].
//!
//! Each fake carries a drift guard comparing what it puts on a real socket with [`golden`].
//!
//! # Why a crate of its own
//!
//! The definition has to be reachable from all three fakes. `cyrup-llama` depends on
//! `cyrup-provider`, so a definition in `cyrup-llama` could serve the provider's fake only through
//! a dev-dependency back-edge, which Cargo accepts but which builds a second copy of
//! `cyrup-provider` for the provider's tests. `cyrup-it` has no `[dependencies]` at all. EXT-108
//! first proposed `cyrup-provider` behind a test-only feature; that works too, but puts test data
//! and a feature switch into a shipping crate. A leaf crate with one dependency (`serde_json`),
//! taken only through `[dev-dependencies]`, is reachable from every fake with no back-edge and
//! keeps the data out of every shipping crate.
//!
//! # Upstream pin
//!
//! Every `@b11436` citation means llama.cpp tag `b11436`
//! (`b9a5a00b86fd285a445916086a0b1dc35bee6d66`, 2026-10-06), under `tools/server/`. That is the
//! version this repo targets: `docs/guide/llama-cpp.md` tells the operator to "use a current
//! llama.cpp build with router support", and cyrup pins no other llama.cpp version anywhere. Read
//! it from the tag, never from a moving working tree.
//!
//! **Floor.** The router's management API arrived in `b9688`, "server: (router) add model
//! management API (#23976)" (2026-06-17) — the first release whose `GET /models` carries the
//! `source`, `architecture` and merged child-`meta` fields `cyrup_llama::model` reads. `b9000`'s
//! router answered `id`/`aliases`/`tags`/`status` only, so no release before `b9688` can drive
//! that client.
//!
//! # What this is not
//!
//! A shape BOTH this definition and its citations read the same wrong way passes every guard. Only
//! a live `llama-server` can catch that, and `cyrup-it`'s opt-in `llama_live` target is the check
//! for it (EXT-100): it starts a real router (`LLAMA_SERVER_BIN`, `CYRUP_LLAMA_MODELS_DIR`) and
//! requires every key each definition here claims to be present in the real answer with the same
//! JSON type. Its first run against `b11436` found two definitions wrong that every guard had
//! passed: [`router::router_props`]' `params` (`null`, not `{}`) and [`router::not_found`] (a
//! handler's 404 body is replaced by the `File Not Found` literal).

pub mod classify;
pub mod golden;
pub mod router;
pub mod systemone;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde_json::Value;

    use crate::classify::{self, TokenLogprob};
    use crate::{golden, router, systemone};

    const B: TokenLogprob<'static> = TokenLogprob {
        id: 66,
        token: "B",
        logprob: -0.3,
    };
    const A: TokenLogprob<'static> = TokenLogprob {
        id: 65,
        token: "A",
        logprob: -1.5,
    };

    /// Every definition serializes to its golden bytes. A change to a definition fails here (and
    /// in every fake's drift guard) until the golden is changed with it, on purpose.
    #[test]
    fn definitions_serialize_to_their_golden_bytes() {
        assert_eq!(
            router::file_not_found().1.to_string(),
            golden::FILE_NOT_FOUND
        );
        assert_eq!(router::file_not_found().0, 404);
        assert_eq!(
            router::invalid_api_key().1.to_string(),
            golden::INVALID_API_KEY
        );
        assert_eq!(router::invalid_api_key().0, 401);
        let (status, body) = router::invalid_request("model is not running");
        assert_eq!(
            (status, body.to_string().as_str()),
            (400, golden::MODEL_IS_NOT_RUNNING)
        );
        assert_eq!(router::success().to_string(), golden::SUCCESS);
        assert_eq!(
            router::models_envelope(Vec::new()).to_string(),
            golden::EMPTY_MODELS
        );
        assert_eq!(
            router::models_envelope(vec![
                router::decision_entry("kev", "loaded"),
                router::decision_entry("laya", "unloaded"),
            ])
            .to_string(),
            golden::MODELS_DECISION_KEV_LOADED_LAYA_UNLOADED
        );
        assert_eq!(
            router::router_props(true).to_string(),
            golden::ROUTER_PROPS_AUTOLOAD
        );
        assert_eq!(
            router::child_props("{{ messages }}").to_string(),
            golden::CHILD_PROPS_PLAIN
        );
        assert_eq!(
            router::sse_frame(&router::sse_event("*", "models_reload", None)),
            golden::SSE_MODELS_RELOAD_FRAME
        );
        assert_eq!(
            classify::tokenize(&[65, 66]).to_string(),
            golden::TOKENIZE_65_66
        );
        assert_eq!(
            classify::apply_template("<|user|>\nhi\n<|assistant|>\n").to_string(),
            golden::APPLY_TEMPLATE_USER_HI
        );
        assert_eq!(
            classify::completion_probabilities(B, &[B, A]).to_string(),
            golden::COMPLETION_PROBABILITIES_B_OVER_A
        );
        assert_eq!(
            classify::completion("qwen", "p", 1, B, &[B, A]).to_string(),
            golden::COMPLETION_QWEN_P_B_OVER_A
        );
        assert_eq!(
            classify::tokenize_with_pieces(&[(65, "A")]).to_string(),
            golden::TOKENIZE_WITH_PIECES_65_A
        );
    }

    /// A handler's 404 reaches the wire as httplib's `File Not Found` literal, not as the
    /// `format_error_response` body the handler built (`server-http.cpp:199-212` @b11436). Found
    /// by the live EXT-100 run: a real b11436 router answered `POST /models/load` of an unknown
    /// model with exactly [`golden::FILE_NOT_FOUND`].
    #[test]
    fn a_handlers_not_found_is_the_file_not_found_literal() {
        let (status, body) = router::not_found("model is not found");
        assert_eq!(
            (status, body.to_string().as_str()),
            (404, golden::FILE_NOT_FOUND)
        );
    }

    /// The whole `/completion` answer carries llama.cpp's top-level keys in llama.cpp's order, and
    /// its `completion_probabilities` is exactly [`classify::completion_probabilities`].
    #[test]
    fn the_completion_answer_has_every_real_key_in_order() {
        let answer = classify::completion("qwen", "p", 3, B, &[B, A]);
        let keys: Vec<&str> = answer
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, golden::COMPLETION_KEYS);
        assert_eq!(
            answer
                .get("completion_probabilities")
                .map(Value::to_string)
                .as_deref(),
            Some(golden::COMPLETION_PROBABILITIES_B_OVER_A)
        );
        assert_eq!(answer.get("content"), Some(&Value::from("B")));
    }

    /// The System One definitions, fed the numbers the live b11436 router answered with, serialize
    /// to that live answer byte for byte; the 501 for a text model likewise.
    #[test]
    fn systemone_definitions_reproduce_the_live_answer() {
        let answer = systemone::response(
            "tinylaya-for-testing-Q8_0",
            &[
                (
                    "category",
                    systemone::choice_answer(
                        "failure",
                        &[
                            ("success", 0.4996767927733331),
                            ("failure", 0.5003232072266669),
                        ],
                        0.000646414453333799,
                    ),
                ),
                (
                    "satisfaction",
                    systemone::score_answer(
                        1.0014472175540505,
                        &["low", "neutral", "high"],
                        &[0.3335314403923897, 0.3314899016611701, 0.33497865794644016],
                        0.0,
                    ),
                ),
                ("approved", systemone::noul_answer(0.500289248081911)),
            ],
            103,
        );
        assert_eq!(answer.to_string(), golden::SYSTEMONE_TINYLAYA_LIVE);
        let (status, body) = systemone::not_a_decision_model();
        assert_eq!(
            (status, body.to_string().as_str()),
            (501, golden::SYSTEMONE_NOT_A_DECISION_MODEL)
        );
    }

    /// `with_pieces: true` yields `{id, piece}` objects, the one object-token form a real server
    /// sends.
    #[test]
    fn tokenize_with_pieces_is_id_and_piece() {
        assert_eq!(
            classify::tokenize_with_pieces(&[(65, "A")]).to_string(),
            golden::TOKENIZE_WITH_PIECES_65_A
        );
    }
}
