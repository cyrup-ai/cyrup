//! Prompt-cache waste accounting — a port of pi `coding-agent/src/core/cache-stats.ts` @v0.83.0
//! (PROV-035).
//!
//! Prompt-cache misses are the largest avoidable cost in a long session, and they are invisible:
//! the money lands in the ordinary input/cache-write buckets, so the only symptom is that the total
//! is bigger than it should be. This module reconstructs, per turn, how many prompt tokens were in
//! the *previous* turn's prompt and were nevertheless re-billed, and what that cost.
//!
//! # The one shape change from upstream, and why
//!
//! pi's three entry points take `SessionEntry[]` (`session-manager.ts`) and key their result map on
//! the `AssistantMessage` **by object reference**. Neither is available here: `SessionEntry` lives
//! in `cyrup-session`, which depends on this crate, so consuming it would invert the dependency;
//! and Rust has no meaningful reference identity for a value type.
//!
//! So the scan takes a slice of [`CacheScanEntry`] — the only two facts the algorithm actually
//! reads out of a `SessionEntry` — and keys misses by **index into that slice**. The caller's
//! adapter is total and mechanical:
//!
//! ```ignore
//! entries.iter().map(|e| match e {
//!     SessionEntry::Compaction { .. } | SessionEntry::BranchSummary { .. } => CacheScanEntry::Reset,
//!     SessionEntry::Message { message: Message::Assistant(m), .. } => CacheScanEntry::Assistant(m),
//!     _ => CacheScanEntry::Other,
//! })
//! ```
//!
//! Every arithmetic decision below is upstream's, line for line, including the ones that look like
//! bugs and are not: the noise floor is `<=` (so exactly 1024 missed tokens is NOT counted), the
//! `reportedCache` flag is sticky across the whole scan segment, and a model switch is deliberately
//! NOT a reset even though a compaction is.
//!
//! # What is not here
//!
//! The two render sites. pi consumes these from `modes/interactive/interactive-mode.ts` @v0.83.0:
//! `:5660` feeds the `Cache Re-billed: $X (N tokens, M misses)` line at `:5705-5711`, and `:3354`
//! re-injects per-message miss notices into the transcript behind `getShowCacheMissNotices()`.
//! Those are `crates/cyrup-tui` and a new `showCacheMissNotices` setting; until they land, PROV-035
//! stays open on its wiring half.

use crate::model::Model;
use cyrup_core::AssistantMessage;
use std::collections::BTreeMap;

/// Prompt-cache TTL. Idle gaps longer than this are worth naming as the likely cause of a miss;
/// Anthropic's default cache TTL is five minutes (pi `CACHE_TTL_MS`, `cache-stats.ts:8`).
pub const CACHE_TTL_MS: i64 = 5 * 60 * 1000;

/// Per-turn misses at or below this are cache-breakpoint granularity noise (pi
/// `NOISE_FLOOR_TOKENS`, `cache-stats.ts:11`).
pub const NOISE_FLOOR_TOKENS: u64 = 1024;

/// A counted cache miss on a single assistant message (pi `interface CacheMiss`,
/// `cache-stats.ts:14-23`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CacheMiss {
    /// Prompt tokens that were in the previous turn's prompt but not read from cache.
    pub missed_tokens: u64,
    /// Extra dollars paid versus a full cache hit; `0.0` when pricing is unknown.
    pub missed_cost: f64,
    /// Milliseconds since the previous request, which is what last refreshed the cache.
    pub idle_ms: i64,
    /// `true` when the model changed relative to the previous request.
    pub model_changed: bool,
}

impl CacheMiss {
    /// `true` when the gap since the previous request exceeded the cache TTL — the "your cache
    /// simply expired" explanation, as opposed to a prompt-prefix change.
    pub fn exceeded_ttl(&self) -> bool {
        self.idle_ms > CACHE_TTL_MS
    }
}

/// Session-wide totals (pi `interface CacheWasteTotals`, `cache-stats.ts:25-30`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CacheWasteTotals {
    pub missed_tokens: u64,
    pub missed_cost: f64,
    /// Number of counted misses (turns above the noise floor).
    pub miss_count: usize,
}

/// Minimal pricing lookup (pi `interface ModelPriceSource`, `cache-stats.ts:33-35`), satisfied by
/// any model registry. `cache_read` is dollars per **million** tokens, as
/// [`crate::model::ModelCost`] stores it.
pub trait ModelPriceSource {
    /// The `cacheRead` rate for a `provider`/`model_id` pair, in $/1M tokens.
    fn cache_read_rate(&self, provider: &str, model_id: &str) -> Option<f64>;
}

/// A `ModelPriceSource` over any slice of catalog rows — the common case, and what makes
/// [`compute_cache_waste`] usable straight off `Models::get_models`.
impl ModelPriceSource for [Model] {
    fn cache_read_rate(&self, provider: &str, model_id: &str) -> Option<f64> {
        self.iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == model_id)
            .map(|m| m.cost.cache_read)
    }
}

/// The same, for an owned catalog. `[Model]` is unsized, so it can satisfy the trait but can never
/// be coerced to `&dyn ModelPriceSource`; a caller holding a `Vec<Model>` (every real one — the
/// session's model registry is built, not borrowed) needs this impl to pass it in at all.
impl ModelPriceSource for Vec<Model> {
    fn cache_read_rate(&self, provider: &str, model_id: &str) -> Option<f64> {
        self.as_slice().cache_read_rate(provider, model_id)
    }
}

/// A price source that knows nothing — pi's `models.getModel(...) ?? undefined` path, where the
/// cache-read rate falls back to `0`.
pub struct NoPrices;

impl ModelPriceSource for NoPrices {
    fn cache_read_rate(&self, _provider: &str, _model_id: &str) -> Option<f64> {
        None
    }
}

/// The only two facts the scan reads out of a session entry — see the module note on why this
/// exists instead of a `SessionEntry`.
#[derive(Clone, Copy, Debug)]
pub enum CacheScanEntry<'a> {
    /// A settled assistant turn (pi `entry.type === "message" && entry.message.role ===
    /// "assistant"`, `cache-stats.ts:117`).
    Assistant(&'a AssistantMessage),
    /// A compaction or branch summary. pi resets the scan here (`:110-115`): the context
    /// legitimately changed, so the next turn's prompt is new content rather than re-billed
    /// content.
    Reset,
    /// Anything else — user messages, tool results, settings entries. Ignored, and specifically NOT
    /// a reset.
    Other,
}

/// The last request seen by the scan; everything in its prompt should have been cached (pi
/// `interface PreviousRequest`, `cache-stats.ts:38-48`).
#[derive(Clone, Debug)]
struct PreviousRequest {
    prompt_tokens: u64,
    model_key: String,
    timestamp: i64,
    /// Sticky: some earlier request in this scan segment reported cache activity. Distinguishes a
    /// total miss on a cache-read-only provider (OpenAI-style, writes unreported) from a provider
    /// that never reports caching at all.
    reported_cache: bool,
}

fn model_key(message: &AssistantMessage) -> String {
    format!("{}/{}", message.provider, message.model)
}

/// 1:1 port of pi `detectMiss` (`cache-stats.ts:56-90`).
///
/// Returns `None` when nothing is counted: the first turn, a turn after a reset, a provider that
/// never reported cache activity, or a miss at or below the noise floor.
fn detect_miss(
    prev: Option<&PreviousRequest>,
    message: &AssistantMessage,
    models: &dyn ModelPriceSource,
) -> Option<CacheMiss> {
    let usage = &message.usage;
    let prompt_tokens = usage.input + usage.cache_read + usage.cache_write;

    // `:62-66`. A zero-cache turn only counts when cache activity was reported before: on
    // cache-read-only providers that is a total miss, while on providers that never report caching
    // it means nothing.
    let prev = prev?;
    if prompt_tokens == 0 || (usage.cache_read + usage.cache_write == 0 && !prev.reported_cache) {
        return None;
    }

    // `:69` — `Math.min(prev.promptTokens, promptTokens) - usage.cacheRead`. The subtraction can go
    // negative in JS (more read than the smaller of the two prompts); `saturating_sub` lands on 0,
    // which the noise-floor test below then rejects exactly as a negative would.
    let missed_tokens = prompt_tokens
        .min(prev.prompt_tokens)
        .saturating_sub(usage.cache_read);
    // `:70` — `<=`, so exactly NOISE_FLOOR_TOKENS is NOT a miss.
    if missed_tokens <= NOISE_FLOOR_TOKENS {
        return None;
    }

    // `:76-82`. Extra cost = missed tokens billed at the rate actually paid (input/cacheWrite,
    // including the write premium) instead of the cache-read rate. Missed tokens can only land in
    // the input or cacheWrite buckets, so the paid rate comes straight from this message's own cost
    // breakdown.
    let paid_tokens = usage.input + usage.cache_write;
    let paid_per_token = if paid_tokens > 0 {
        (usage.cost.input + usage.cost.cache_write) / paid_tokens as f64
    } else {
        0.0
    };
    let read_per_token = if usage.cache_read > 0 {
        usage.cost.cache_read / usage.cache_read as f64
    } else {
        models
            .cache_read_rate(message.provider.as_str(), message.model.as_str())
            .unwrap_or(0.0)
            / 1_000_000.0
    };

    Some(CacheMiss {
        missed_tokens,
        // `:85` — `Math.max(0, paidPerToken - readPerToken)`.
        missed_cost: missed_tokens as f64 * (paid_per_token - read_per_token).max(0.0),
        // `:86` — `Math.max(0, message.timestamp - prev.timestamp)`.
        idle_ms: (message.timestamp - prev.timestamp).max(0),
        model_changed: model_key(message) != prev.model_key,
    })
}

/// 1:1 port of pi `asPreviousRequest` (`cache-stats.ts:92-103`). `None` when the turn reported no
/// prompt at all, in which case the caller KEEPS the previous request rather than clearing it.
fn as_previous_request(
    message: &AssistantMessage,
    reported_cache: bool,
) -> Option<PreviousRequest> {
    let usage = &message.usage;
    let prompt_tokens = usage.input + usage.cache_read + usage.cache_write;
    if prompt_tokens == 0 {
        return None;
    }
    Some(PreviousRequest {
        prompt_tokens,
        model_key: model_key(message),
        timestamp: message.timestamp,
        reported_cache: reported_cache || usage.cache_read + usage.cache_write > 0,
    })
}

/// The result of one scan (pi's `scan` return, `cache-stats.ts:105-131`).
#[derive(Debug, Default)]
pub struct CacheScan {
    pub totals: CacheWasteTotals,
    /// Counted misses keyed by index into the scanned slice — pi keys by `AssistantMessage`
    /// reference, which Rust cannot reproduce; see the module note.
    pub misses: BTreeMap<usize, CacheMiss>,
    /// The trailing `PreviousRequest`, exposed only so [`detect_cache_miss`] can extend a scan by
    /// one not-yet-persisted message.
    prev: Option<PreviousRequest>,
}

/// 1:1 port of pi `scan` (`cache-stats.ts:105-131`).
pub fn scan(entries: &[CacheScanEntry<'_>], models: &dyn ModelPriceSource) -> CacheScan {
    let mut out = CacheScan::default();

    for (index, entry) in entries.iter().enumerate() {
        match entry {
            // `:110-115` — the context legitimately changed. Model switches are NOT exempt: they
            // re-bill the full prompt and SHOULD be counted.
            CacheScanEntry::Reset => {
                out.prev = None;
            }
            CacheScanEntry::Other => {}
            CacheScanEntry::Assistant(message) => {
                if let Some(miss) = detect_miss(out.prev.as_ref(), message, models) {
                    out.totals.missed_tokens += miss.missed_tokens;
                    out.totals.missed_cost += miss.missed_cost;
                    out.totals.miss_count += 1;
                    out.misses.insert(index, miss);
                }
                // `:126` — `prev = asPreviousRequest(...) ?? prev`: a promptless turn does not
                // clear the previous request.
                let reported = out.prev.as_ref().is_some_and(|p| p.reported_cache);
                if let Some(next) = as_previous_request(message, reported) {
                    out.prev = Some(next);
                }
            }
        }
    }

    out
}

/// Cumulative cache waste across a session: prompt tokens that should have been cache reads (they
/// were in the previous turn's prompt) but were re-billed (pi `computeCacheWaste`,
/// `cache-stats.ts:137-139`).
pub fn compute_cache_waste(
    entries: &[CacheScanEntry<'_>],
    models: &dyn ModelPriceSource,
) -> CacheWasteTotals {
    scan(entries, models).totals
}

/// All counted cache misses across a session, keyed by the index of the assistant entry that paid
/// for them (pi `collectCacheMisses`, `cache-stats.ts:146-151`). Used to re-derive transcript
/// notices when rebuilding the chat from entries (resume, post-compaction rebuild).
pub fn collect_cache_misses(
    entries: &[CacheScanEntry<'_>],
    models: &dyn ModelPriceSource,
) -> BTreeMap<usize, CacheMiss> {
    scan(entries, models).misses
}

/// Detect a cache miss on a just-completed assistant message (pi `detectCacheMiss`,
/// `cache-stats.ts:157-163`).
///
/// `entries` must NOT already contain `message` — upstream's `message_end` fires before
/// persistence, and passing the message twice would compare it against itself.
pub fn detect_cache_miss(
    entries: &[CacheScanEntry<'_>],
    message: &AssistantMessage,
    models: &dyn ModelPriceSource,
) -> Option<CacheMiss> {
    detect_miss(scan(entries, models).prev.as_ref(), message, models)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use cyrup_core::{Cost, StopReason, Usage};

    /// An assistant turn with only the fields the scan reads.
    fn turn(
        provider: &str,
        model: &str,
        timestamp: i64,
        input: u64,
        cache_read: u64,
        cache_write: u64,
    ) -> AssistantMessage {
        let mut message = AssistantMessage::errored(
            provider.into(),
            model,
            Some("anthropic-messages".into()),
            StopReason::Stop,
            "",
        );
        message.timestamp = timestamp;
        message.usage = Usage {
            input,
            output: 0,
            cache_read,
            cache_write,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: input + cache_read + cache_write,
            cost: Cost {
                // $3/1M input, $3.75/1M cache write, $0.30/1M cache read — Sonnet's real shape,
                // so the paid-vs-read delta below is a realistic number.
                input: input as f64 * 3.0 / 1_000_000.0,
                output: 0.0,
                cache_read: cache_read as f64 * 0.30 / 1_000_000.0,
                cache_write: cache_write as f64 * 3.75 / 1_000_000.0,
                total: 0.0,
            },
        };
        message
    }

    /// PROV-035's own Verify clause: a second turn with `cache_read == 0` after a first turn with a
    /// large `cache_write` yields exactly one miss, sized to the re-billed prefix.
    #[test]
    fn a_re_billed_prefix_is_one_miss_sized_to_the_prefix() {
        let first = turn("anthropic", "claude-sonnet-4.5", 0, 0, 0, 100_000);
        let second = turn("anthropic", "claude-sonnet-4.5", 1_000, 120_000, 0, 0);
        let entries = [
            CacheScanEntry::Assistant(&first),
            CacheScanEntry::Assistant(&second),
        ];

        let totals = compute_cache_waste(&entries, &NoPrices);
        assert_eq!(totals.miss_count, 1);
        assert_eq!(
            totals.missed_tokens, 100_000,
            "min(prev prompt, this prompt) - cacheRead = min(100k, 120k) - 0"
        );
        // paid rate is the pure $3/1M input rate; read rate falls back to 0 with no price source.
        assert!(
            (totals.missed_cost - 100_000.0 * 3.0 / 1_000_000.0).abs() < 1e-12,
            "got {}",
            totals.missed_cost
        );

        let misses = collect_cache_misses(&entries, &NoPrices);
        assert_eq!(misses.len(), 1);
        assert!(misses.contains_key(&1), "the SECOND turn paid for it");
        assert!(!misses[&1].model_changed);
        assert_eq!(misses[&1].idle_ms, 1_000);
        assert!(!misses[&1].exceeded_ttl());
    }

    /// pi `:62-66` — the sticky `reportedCache` flag. A provider that has never reported any cache
    /// activity is not accused of missing a cache it does not have.
    #[test]
    fn a_provider_that_never_reports_caching_is_never_counted() {
        let first = turn("openai", "gpt-5", 0, 100_000, 0, 0);
        let second = turn("openai", "gpt-5", 1_000, 120_000, 0, 0);
        let entries = [
            CacheScanEntry::Assistant(&first),
            CacheScanEntry::Assistant(&second),
        ];
        assert_eq!(compute_cache_waste(&entries, &NoPrices).miss_count, 0);
    }

    /// …but once ANY turn reports cache activity, a later zero-cache turn IS a total miss — the
    /// cache-read-only (OpenAI-style) case the flag exists for.
    #[test]
    fn a_zero_cache_turn_counts_once_caching_has_been_reported() {
        let first = turn("openai", "gpt-5", 0, 20_000, 80_000, 0);
        let second = turn("openai", "gpt-5", 1_000, 120_000, 0, 0);
        let entries = [
            CacheScanEntry::Assistant(&first),
            CacheScanEntry::Assistant(&second),
        ];
        let totals = compute_cache_waste(&entries, &NoPrices);
        assert_eq!(totals.miss_count, 1);
        assert_eq!(totals.missed_tokens, 100_000);
    }

    /// pi `:110-115` — a compaction resets the scan; a model switch deliberately does NOT.
    #[test]
    fn compaction_resets_the_scan_but_a_model_switch_does_not() {
        let first = turn("anthropic", "claude-sonnet-4.5", 0, 0, 0, 100_000);
        let second = turn("anthropic", "claude-sonnet-4.5", 1_000, 120_000, 0, 0);

        let compacted = [
            CacheScanEntry::Assistant(&first),
            CacheScanEntry::Reset,
            CacheScanEntry::Assistant(&second),
        ];
        assert_eq!(compute_cache_waste(&compacted, &NoPrices).miss_count, 0);

        let switched_first = turn("anthropic", "claude-opus-4.5", 0, 0, 0, 100_000);
        let switched = [
            CacheScanEntry::Assistant(&switched_first),
            CacheScanEntry::Assistant(&second),
        ];
        let misses = collect_cache_misses(&switched, &NoPrices);
        assert_eq!(misses.len(), 1, "a model switch re-bills and IS counted");
        assert!(misses[&1].model_changed);
    }

    /// pi `:70` uses `<=`, so a miss of exactly `NOISE_FLOOR_TOKENS` is discarded and
    /// `NOISE_FLOOR_TOKENS + 1` is kept. Off-by-one here silently changes every session's numbers.
    #[test]
    fn the_noise_floor_is_exclusive_at_exactly_1024() {
        let base = turn("anthropic", "m", 0, 0, 0, NOISE_FLOOR_TOKENS);
        let exact = turn("anthropic", "m", 1, NOISE_FLOOR_TOKENS, 0, 0);
        assert_eq!(
            compute_cache_waste(
                &[
                    CacheScanEntry::Assistant(&base),
                    CacheScanEntry::Assistant(&exact)
                ],
                &NoPrices
            )
            .miss_count,
            0
        );

        let base = turn("anthropic", "m", 0, 0, 0, NOISE_FLOOR_TOKENS + 1);
        let over = turn("anthropic", "m", 1, NOISE_FLOOR_TOKENS + 1, 0, 0);
        assert_eq!(
            compute_cache_waste(
                &[
                    CacheScanEntry::Assistant(&base),
                    CacheScanEntry::Assistant(&over)
                ],
                &NoPrices
            )
            .miss_count,
            1
        );
    }

    /// pi `:78-82` — with no cache read on this turn, the read rate comes from the price source, so
    /// the missed cost is the DELTA and not the gross paid amount.
    #[test]
    fn the_price_source_supplies_the_read_rate_when_this_turn_read_nothing() {
        let first = turn("anthropic", "claude-sonnet-4.5", 0, 0, 0, 100_000);
        let second = turn("anthropic", "claude-sonnet-4.5", 1_000, 120_000, 0, 0);
        let entries = [
            CacheScanEntry::Assistant(&first),
            CacheScanEntry::Assistant(&second),
        ];

        struct Sonnet;
        impl ModelPriceSource for Sonnet {
            fn cache_read_rate(&self, provider: &str, model_id: &str) -> Option<f64> {
                (provider == "anthropic" && model_id == "claude-sonnet-4.5").then_some(0.30)
            }
        }

        let priced = compute_cache_waste(&entries, &Sonnet).missed_cost;
        let unpriced = compute_cache_waste(&entries, &NoPrices).missed_cost;
        assert!(
            priced < unpriced,
            "a known cache-read rate must reduce the waste: {priced} vs {unpriced}"
        );
        assert!(
            (priced - 100_000.0 * (3.0 - 0.30) / 1_000_000.0).abs() < 1e-12,
            "got {priced}"
        );
    }

    /// pi `:126` — `prev = asPreviousRequest(...) ?? prev`. A promptless turn must not clear the
    /// previous request, or the turn after it escapes counting.
    #[test]
    fn a_promptless_turn_does_not_clear_the_previous_request() {
        let first = turn("anthropic", "m", 0, 0, 0, 100_000);
        let empty = turn("anthropic", "m", 500, 0, 0, 0);
        let third = turn("anthropic", "m", 1_000, 120_000, 0, 0);
        let entries = [
            CacheScanEntry::Assistant(&first),
            CacheScanEntry::Assistant(&empty),
            CacheScanEntry::Assistant(&third),
        ];
        let misses = collect_cache_misses(&entries, &NoPrices);
        assert_eq!(misses.len(), 1);
        assert!(misses.contains_key(&2), "the third turn is still measured");
    }

    /// `detect_cache_miss` extends a scan by one message that is NOT yet in `entries` — the
    /// `message_end`-before-persistence contract.
    #[test]
    fn detect_cache_miss_measures_a_message_not_yet_in_the_entries() {
        let first = turn("anthropic", "m", 0, 0, 0, 100_000);
        let pending = turn("anthropic", "m", 1_000, 120_000, 0, 0);
        let miss = detect_cache_miss(&[CacheScanEntry::Assistant(&first)], &pending, &NoPrices)
            .expect("counted");
        assert_eq!(miss.missed_tokens, 100_000);

        // Nothing before it ⇒ nothing to compare against.
        assert!(detect_cache_miss(&[], &pending, &NoPrices).is_none());
    }

    /// The idle gap drives pi's "your cache expired" explanation.
    #[test]
    fn a_gap_beyond_the_cache_ttl_is_flagged() {
        let first = turn("anthropic", "m", 0, 0, 0, 100_000);
        let late = turn("anthropic", "m", CACHE_TTL_MS + 1, 120_000, 0, 0);
        let miss = collect_cache_misses(
            &[
                CacheScanEntry::Assistant(&first),
                CacheScanEntry::Assistant(&late),
            ],
            &NoPrices,
        );
        assert!(miss[&1].exceeded_ttl());
    }

    /// The catalog-backed price source: `[Model]` is the ergonomic case the wiring half will use.
    #[test]
    fn a_catalog_slice_is_a_price_source() {
        let models = crate::providers::google_vertex::google_vertex_models();
        let first = models.first().expect("catalog is non-empty");
        assert_eq!(
            models
                .as_slice()
                .cache_read_rate("google-vertex", first.id.as_str()),
            Some(first.cost.cache_read)
        );
        assert_eq!(models.as_slice().cache_read_rate("nope", "nope"), None);
    }
}
