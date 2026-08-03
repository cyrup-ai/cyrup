//! Portable, dependency-free utilities ported 1:1 from Pi `packages/ai/src/utils/*` and
//! `api/simple-options.ts`.
//!
//! These are pure functions over the shared data model (no transport, no new external deps):
//! - [`estimate`] — heuristic context-token estimation (`utils/estimate.ts`).
//! - [`overflow`] — context-overflow error classification (`utils/overflow.ts`).
//! - [`retry`] — transient provider/transport error classification (`utils/retry.ts`).
//! - [`json_parse`] — best-effort/partial JSON recovery for streamed tool-call args
//!   (`utils/json-parse.ts`).
//! - [`simple_options`] — the unified "simple" option surface + thinking-budget mapping
//!   (`api/simple-options.ts`).
//! - [`regexlite`] — the tiny case-insensitive matcher the overflow/retry classifiers run on
//!   (replaces the `regex` crate, which is outside this round's dependency budget).
//! - [`http_date`] — the two `Date.parse` shapes the remote model-catalog overlay needs
//!   (`Last-Modified` IMF-fixdate + the ISO-8601 catalog-manifest stamp), DRIFT-007.

pub mod estimate;
pub mod hash;
pub mod http_date;
pub mod json_parse;
pub mod node_http_proxy;
pub mod overflow;
pub mod refresh;
pub mod regexlite;
pub mod retry;
pub mod simple_options;
