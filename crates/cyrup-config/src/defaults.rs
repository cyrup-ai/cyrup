//! Port of Pi `packages/coding-agent/src/core/defaults.ts` @v0.83.0.
//!
//! Upstream that file has exactly one export, and this module mirrors it one-for-one rather than
//! folding the constant into [`crate::settings`]: the value is the fallback that *every* consumer
//! of `getDefaultThinkingLevel()` must name explicitly, and keeping it in its own module is what
//! makes an unnamed fallback visible in review.

use cyrup_core::ModelThinkingLevel;

/// `export const DEFAULT_THINKING_LEVEL: ThinkingLevel = "medium";`
/// (Pi `packages/coding-agent/src/core/defaults.ts:3` @v0.83.0).
///
/// This is the level a session starts at when the user has never written `defaultThinkingLevel`
/// into `settings.json`. Upstream names it at six sites — `core/sdk.ts:230` and `:235`
/// (`settingsManager.getDefaultThinkingLevel() ?? DEFAULT_THINKING_LEVEL`),
/// `core/agent-session.ts:1738` (the same expression) and `core/model-resolver.ts:594`, `:608`,
/// `:616`, `:642`, `:647`, `:651` — precisely because
/// `SettingsManager.getDefaultThinkingLevel()` returns `ThinkingLevel | undefined`
/// (`settings-manager.ts:740-742`) and refuses to choose for them.
///
/// **Do NOT confuse this with `ModelThinkingLevel::default()`.** `Off` is correct as the *type's*
/// zero — Pi forces `"off"` for a modelless session (`sdk.ts:238-240`) and cyrup relies on that at
/// `cyrup-session-svc/src/builder.rs`. `Off` is *not* correct as the unset-setting fallback; using
/// it there starts every default session with reasoning disabled. CFG-056.
pub const DEFAULT_THINKING_LEVEL: ModelThinkingLevel = ModelThinkingLevel::Medium;
