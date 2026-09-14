//! Why a failed model stays failed — the cached model-exclusion registry.
//!
//! Ports pi `runs/shared/model-exclusions.ts` (385 LOC @v0.64.0) and the `model-fallback.ts` half
//! that reads and writes it.
//!
//! # The problem this solves
//!
//! The fallback ladder's entire purpose is to spend attempts where they can succeed. Without this
//! registry it cannot: a model that failed with `429`/`quota`/`unauthorized` on the last run is
//! attempted again on the next one, and on every run after that, until the underlying condition
//! clears on its own. Every one of those attempts costs a full child spawn and a full task
//! dispatch to learn something the previous run already knew.
//!
//! So a retryable model failure is RECORDED — keyed by model, or by provider when the model was
//! never the problem — with an expiry, and the ladder filters against it before it spawns anything.
//!
//! # Layout
//!
//! ```text
//! model_exclusions/
//!   mod.rs      this facade: the narrative and the re-exports; no logic
//!   entry.rs    ModelExclusion, ModelExclusionTarget, matching, parse_model_key
//!   store.rs    the owned store + TTL + dedupe + the 200-entry cap
//!   persist.rs  the on-disk format, the env override, the validating read, the atomic write
//!   auth.rs     invalidate_auth_exclusions — the auth-store mtime rule
//!   filter.rs   filter_fallback_candidates + the bounded operator diagnostic
//! ```
//!
//! # The four rules that are not obvious
//!
//! 1. **A context overflow never records.** The input was too large; the model is fine. Excluding
//!    it for 24 h would take a working model out of every later ladder because one task was too
//!    big. This is [`crate::exec::fallback::is_context_overflow`]'s third call site.
//! 2. **An exclusion can die before its TTL.** Touching the auth store invalidates every
//!    auth-flavoured exclusion recorded before it, so an operator who fixes their credentials does
//!    not wait out a 24-hour 401 (`auth.rs`).
//! 3. **A provider entry and a model entry are asymmetric.** `openai/gpt-4` does not exclude
//!    `github-copilot/gpt-4`, but a provider-wide entry excludes every model of that provider
//!    ([`entry::ModelExclusion::matches`]).
//! 4. **A provisioning failure is not a model failure**, even when its text reads like one — it
//!    gets a 15-minute window instead of the default TTL, and never re-stamps
//!    ([`crate::exec::fallback::record_retryable_model_failure`]).
//!
//! # What is deliberately NOT ported
//!
//! pi `throwForExplicitModelExclusion` (`model-fallback.ts:326-334`) refuses a run outright when
//! the PRIMARY model's origin is `explicit` and that model is excluded, rather than rotating to a
//! fallback. cyrup's counterpart rung is [`crate::exec::fallback::resolve_model_inheritance`] /
//! [`crate::exec::fallback::ModelOverride`], which is a different function on a different call path
//! from everything this module touches, and porting the arm means threading the store into the
//! inheritance resolver as well. It is not ported here, and the operator is not left without an
//! answer: an explicit model that is excluded is dropped by
//! [`filter::filter_fallback_candidates`] like any other candidate, and when it is the only rung
//! the run is refused with [`filter::ZERO_USABLE_MODEL_CANDIDATES_ERROR`] plus the same evidence
//! upstream's message carries — the difference is the sentence, not whether the run is refused.

pub mod auth;
pub mod entry;
pub mod filter;
pub mod persist;
pub mod store;

pub use entry::{
    MAX_DATE_TIMESTAMP_MS, ModelExclusion, ModelExclusionTarget, ModelKey, parse_model_key,
};
pub use filter::{
    ExcludedCandidate, ExclusionObserver, ExclusionOverride, FilterOptions, ModelExclusionEvidence,
    ZERO_USABLE_MODEL_CANDIDATES_ERROR, filter_fallback_candidates,
    format_excluded_candidate_evidence, format_model_exclusion_expiry,
    ignore_stale_model_unavailable_exclusion, sanitize_model_exclusion_diagnostic,
};
pub use persist::{MODEL_EXCLUSIONS_PATH_ENV, exclusions_file_path};
pub use store::{
    DEFAULT_EXCLUSION_REASON, DEFAULT_MODEL_EXCLUSION_TTL_MS, MAX_MODEL_EXCLUSION_TTL_MS,
    ModelExclusionStore, RecordModelFailure, apply_model_exclusions_config,
    resolve_model_exclusion_ttl, validate_model_exclusion_ttl, validate_model_exclusions_config,
};
