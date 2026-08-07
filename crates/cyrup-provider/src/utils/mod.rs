//! Portable, dependency-free utilities ported 1:1 from Pi `packages/ai/src/utils/*` and
//! `api/simple-options.ts`.
//!
//! These are pure functions over the shared data model (no transport, no new external deps):
//! - [`deferred_tools`] — split the active tool list into an immediate prefix and transcript-
//!   anchored definitions (`utils/deferred-tools.ts`), DRIFT-001.
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
//! - [`error_body`] — the 4000-char cap on a provider HTTP error body before it reaches the
//!   transcript (`utils/error-body.ts`).
//!
//! Two members read HTTP response headers and so are not *quite* transport-free, matching the
//! upstream files they port 1:1:
//! - [`node_http_proxy`] — `HTTP(S)_PROXY`/`NO_PROXY` resolution (`utils/node-http-proxy.ts`).
//! - [`provider_retry`] — the server-directed request-retry policy (`utils/provider-retry.ts`).

pub mod deferred_tools;
pub mod error_body;
pub mod estimate;
pub mod hash;
pub mod http_date;
pub mod json_parse;
pub mod node_http_proxy;
pub mod overflow;
pub mod provider_retry;
pub mod refresh;
pub mod regexlite;
pub mod retry;
pub mod simple_options;
