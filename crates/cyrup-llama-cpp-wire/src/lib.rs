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
//! * `cyrup-provider/src/tests/llama_cpp_classify_fake_server.rs` — the classifier api. NOT yet
//!   moved onto this crate (EXT-108 is PARTIAL on it; see the ledger).
//!
//! Each fake that answers from here carries a drift guard comparing what it puts on a real socket
//! with [`golden`].
//!
//! # Why a crate of its own
//!
//! The definition has to be reachable from all three fakes. `cyrup-llama` depends on
//! `cyrup-provider` (so the definition cannot live in `cyrup-llama` and serve the provider's fake
//! without a dev-dependency cycle), and `cyrup-it` has no `[dependencies]` at all. A leaf crate with
//! one dependency (`serde_json`) is reachable from every one of them with no cycle and puts no test
//! data into any shipping crate, not even behind a feature.
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
//! a live `llama-server` can catch that; the opt-in `LLAMA_SERVER_BIN` conformance target EXT-108
//! names is the check for it.

pub mod classify;
pub mod golden;
pub mod router;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde_json::Value;

    use crate::classify::{self, TokenLogprob};
    use crate::{golden, router};

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

    /// `with_pieces: true` yields `{id, piece}` objects, the one object-token form a real server
    /// sends.
    #[test]
    fn tokenize_with_pieces_is_id_and_piece() {
        assert_eq!(
            classify::tokenize_with_pieces(&[(65, "A")]).to_string(),
            r#"{"tokens":[{"id":65,"piece":"A"}]}"#
        );
    }
}
