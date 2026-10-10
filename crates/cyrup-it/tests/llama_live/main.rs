//! **Live llama.cpp conformance (EXT-100).** `crates/cyrup-llama` and the two llama.cpp classifier
//! apis of `crates/cyrup-provider` against a REAL `llama-server`, not a fake.
//!
//! Every other llama.cpp test in this workspace runs against a fake (`cyrup-llama`'s
//! `src/tests/fake_server.rs`, this crate's `tests/llama/fake.rs`, `cyrup-provider`'s
//! `src/tests/llama_cpp_classify_fake_server.rs`), and every fake answers from ONE cited definition,
//! `crates/cyrup-llama-cpp-wire`. A shape the definition and its citations read the same wrong way
//! passes every one of those tests. This target is the check that can catch it: it starts a real
//! router, and
//!
//! * [`conformance`] compares each `cyrup-llama-cpp-wire` definition with the real server's answer
//!   to the same request: every key the definition claims is present in the real answer with the
//!   same JSON type (recursively), and the literal error bodies are equal byte for byte;
//! * [`live`] drives cyrup's own code paths, never raw HTTP: `LlamaClient` (list, the SSE-backed
//!   `load_and_wait`, props, `unload_and_wait`), `LlamaProvider::refresh` through a
//!   `LlamaController`, one streamed chat turn through `Models::stream`, one `llama-cpp-classify`
//!   classify against `stories260K`, and one `typesafe-system-one` classify against
//!   `tinylaya-for-testing-Q8_0`;
//! * [`session`] goes one layer up, to what a user runs: a REAL `AgentSession` assembled the way
//!   the `cyrup` binary assembles it (the llama.cpp built-in attached by `build_factory`, the
//!   credential in `auth.json`), refreshed against the real router, then a prompted turn.
//!
//! # Running it
//!
//! It is OPT-IN twice: the `it` feature arms the target (like every target here), and two
//! environment variables arm the tests. Without both variables each test writes
//! `SKIPPED (llama_live): ...` straight to the process's stderr (around libtest's capture, so
//! `cargo test` shows it) and returns; it never pretends to have checked anything. With only one
//! of them, or with a model missing from the directory, the test FAILS, so a half-configured run
//! cannot pass quietly either.
//!
//! * `LLAMA_SERVER_BIN` — a `llama-server` binary built from llama.cpp **`b11436`** (`b9a5a00`), the
//!   release `cyrup-llama-cpp-wire` pins. Any release from the floor `b9688` on has the router
//!   management api; the definitions are checked against the pin.
//! * `CYRUP_LLAMA_MODELS_DIR` — a directory holding exactly the two test models (the router lists
//!   every GGUF in it):
//!   * `stories260K.gguf` — a 1.2 MB Llama-architecture text model (`output_modalities:
//!     ["text"]`): Hugging Face `ggml-org/models`, file `tinyllamas/stories260K.gguf`, the default
//!     model of llama.cpp's own server tests (`tools/server/tests/utils.py:55-56` @b11436);
//!   * `tinylaya-for-testing-Q8_0.gguf` — a 97 MB random-weight Laya decision model
//!     (`output_modalities: ["decisions"]`) from Hugging Face `ggml-org/tinylaya-for-testing-gguf`,
//!     the model llama.cpp's own System One tests use (`utils.py:632-645`), served by the native
//!     `POST /v1/systemone`.
//!
//! ```text
//! env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY \
//!     LLAMA_SERVER_BIN=/path/to/llama.cpp/build/bin/llama-server \
//!     CYRUP_LLAMA_MODELS_DIR=/path/to/llama-models \
//!     cargo test -p cyrup-it --features it --test llama_live -- --nocapture
//! ```
//!
//! (`env -u` only because `support::env`'s guards refuse ambient provider credentials in any
//! target; `cargo run -p xtask -- it` clears the whole environment, which also clears these two
//! variables, so through xtask this target always skips.) The router is started by the test on a
//! free loopback port with `-t 1` and `--api-key`, and killed when the test ends, pass or fail; its
//! children exit on their own when their stdin closes (`server-models.cpp:1834-1853` @b11436).
//!
//! This repo has no CI, so there is no nightly job to put this in: the run above is the documented
//! manual opt-in, and the ledger (EXT-100) records each run.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../support/mod.rs"]
mod support;

mod conformance;
// The `llama` target's session fixture (a REAL `AgentSession` through
// `cyrup::session_launch::build_factory`), shared rather than copied; this target uses part of it
// (the file allows `dead_code` itself).
#[path = "../llama/fixture.rs"]
mod fixture;
mod live;
mod router;
mod session;
