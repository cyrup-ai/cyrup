//! `cyrup-codemode-runtime` — the engine-bound half of pi's `codemode` (gap-analysis area 18,
//! `docs/gap-analysis/18-pi-codemode.md`, decision `CODE-001`, engine `deno_core` per ADR-0031).
//!
//! The engine-independent half (script source format, declaration renderer, BM25 ranker, output
//! budget) is `cyrup-codemode`. This crate holds what needs a JavaScript engine or a session:
//!
//! | module | upstream @v1.0.1 | ledger |
//! |---|---|---|
//! | [`types`] | `packages/codemode/src/types.ts` | CODE-002 |
//! | `sandbox` | `packages/codemode/src/runtime/*` | CODE-002 |
//! | [`tool`] | `packages/coding-agent/src/extensions/codemode/{tool,execute}.ts` | CODE-007/008/009/010/012 |
//! | [`renderer`] | `packages/coding-agent/src/extensions/codemode/renderer.ts` | CODE-011 |
//! | [`extension`] | `packages/coding-agent/src/extensions/codemode/index.ts` | — |
//! | `testkit` | — | a scripted sandbox and a recording host, behind the `testkit` feature |

#![forbid(unsafe_code)]

pub mod extension;
pub mod renderer;
pub mod sandbox;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
pub mod tool;
pub mod types;

pub use extension::{CodemodeExtension, EXTENSION_ID};
