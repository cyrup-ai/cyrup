#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

//! Tests of the sandbox. `port` is upstream's `test/sandbox.test.ts` case by case; `runtime`
//! holds what only this implementation has to prove: the caller's runtime, concurrency, dropping
//! and closing, the heap limit, the global surface.

mod port;
mod runtime;
mod support;
