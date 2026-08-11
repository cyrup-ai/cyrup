//! The rolling scope artifact — a 1:1 port of `pi-subagents/src/watchdog/scope.ts` (62 lines
//! @v0.43.0).
//!
//! The watchdog reviews a turn against what the USER actually asked for, not against the agent's
//! own restatement of it. `WatchdogScopeArtifact` is that record: every real user prompt of the
//! session, newest last, bounded three ways (`:3-5`) — at most 8 entries, each at most 2 000
//! characters, and at most 16 000 characters in total. [`WatchdogScopeArtifact::render`] turns it
//! into the "Current scope:" block `runtime.ts:820` prepends to the review input, whose second
//! paragraph is the instruction that makes newer prompts SUPERSEDE older ones and tells the
//! reviewer to file anything serving no current scope item as `scope-drift`.
//!
//! Only REAL user prompts enter it. `runtime.ts:315-318` adds a prompt only when the turn is not an
//! auto-follow turn, so the watchdog's own synthetic follow-up text can never widen the scope it is
//! judging against — which would let one blocker warning license the very drift it flagged.
//!
//! ## The auto-follow prompt marker, in Rust
//!
//! Upstream also carries a second, independent channel for that decision:
//! `WATCHDOG_AUTO_FOLLOW_PROMPT_MARKER` (`:1`) is a JS `Symbol` stamped onto the event object with
//! `Object.defineProperty` (`:59-62`) and read back with `isWatchdogAutoFollowPromptEvent` (`:55-57`).
//! A symbol-keyed, non-enumerable property is invisible to `JSON.stringify`, to `Object.keys`, and
//! to any consumer that does not already hold the symbol — it rides along on the very object
//! identity that reaches `before_agent_start`.
//!
//! cyrup has no such channel and cannot grow one: a handler receives
//! [`cyrup_ext::HostEvent::BeforeAgentStart`], a serializable enum value reconstructed by the host
//! for each subscriber, so there is no object identity to stamp and no place to hide an extra key.
//! What the port keeps is upstream's OTHER, load-bearing arm of the same decision — the exact-text
//! match against `pendingAutoFollowPrompts` (`runtime.ts:309-311`), which is what actually fires in
//! upstream too whenever the prompt travels through `pi.sendUserMessage` rather than being
//! constructed in-process. [`WatchdogAutoFollowPromptLedger`] is that queue, extracted here so the
//! marker's whole contract — bounded depth, first-match-wins removal, "a real user prompt racing in
//! ahead of a queued one stays real" — lives beside the artifact it protects.

/// `MAX_SCOPE_ENTRIES` (`scope.ts:3`).
const MAX_SCOPE_ENTRIES: usize = 8;
/// `MAX_SCOPE_ENTRY_CHARS` (`scope.ts:4`).
const MAX_SCOPE_ENTRY_CHARS: usize = 2_000;
/// `MAX_SCOPE_TOTAL_CHARS` (`scope.ts:5`).
const MAX_SCOPE_TOTAL_CHARS: usize = 16_000;

/// The bound on `pendingAutoFollowPrompts` (`runtime.ts:676`).
const MAX_PENDING_AUTO_FOLLOW_PROMPTS: usize = 8;

/// `WatchdogScopeEntry` (`scope.ts:7-10`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogScopeEntry {
    /// The trimmed, length-capped prompt text.
    pub prompt: String,
    /// ISO-8601 instant the prompt was recorded.
    pub created_at: String,
}

/// `WatchdogScopeArtifact` (`scope.ts:12-53`).
#[derive(Debug, Clone, Default)]
pub struct WatchdogScopeArtifact {
    entries: Vec<WatchdogScopeEntry>,
}

impl WatchdogScopeArtifact {
    /// An empty artifact (`private entries: WatchdogScopeEntry[] = []`, `:13`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `addPrompt(prompt, { createdAt })` (`scope.ts:15-23`): trim, drop an empty prompt, cap the
    /// entry's length, stamp it, then re-trim the whole artifact.
    ///
    /// `created_at` of `None` is upstream's `new Date().toISOString()` default (`:20`).
    pub fn add_prompt(&mut self, prompt: &str, created_at: Option<String>) {
        let normalized = prompt.trim();
        if normalized.is_empty() {
            return;
        }
        self.entries.push(WatchdogScopeEntry {
            prompt: cap_chars(normalized, MAX_SCOPE_ENTRY_CHARS),
            created_at: created_at.unwrap_or_else(super::now_iso8601),
        });
        self.trim();
    }

    /// `reset()` (`scope.ts:25-27`).
    pub fn reset(&mut self) {
        self.entries.clear();
    }

    /// `snapshot()` (`scope.ts:29-31`) — a copy, so a caller cannot mutate the artifact through it.
    #[must_use]
    pub fn snapshot(&self) -> Vec<WatchdogScopeEntry> {
        self.entries.clone()
    }

    /// `render()` (`scope.ts:33-43`) — the empty string when there is no scope at all, else the
    /// header, the supersede/scope-drift instruction, and one numbered block per entry, all joined
    /// by a BLANK line (`join("\n\n")`) while the two lines WITHIN a block are joined by a single
    /// newline.
    #[must_use]
    pub fn render(&self) -> String {
        if self.entries.is_empty() {
            return String::new();
        }
        let mut blocks: Vec<String> = Vec::with_capacity(self.entries.len() + 2);
        blocks.push("Current scope:".to_string());
        blocks.push(
            "The following real user prompts are the current scope record, newest last. Newer prompts supersede and mutate older prompts: they may add, modify, or remove requirements. Flag work that serves no current scope item as category 'scope-drift'."
                .to_string(),
        );
        for (index, entry) in self.entries.iter().enumerate() {
            blocks.push(format!(
                "Scope prompt {} ({}):\n{}",
                index + 1,
                entry.created_at,
                entry.prompt
            ));
        }
        blocks.join("\n\n")
    }

    /// `private trim()` (`scope.ts:45-52`): first the entry-count bound, then the total-character
    /// bound — the latter never removing the LAST entry, so the newest prompt survives even when it
    /// alone exceeds the total budget.
    fn trim(&mut self) {
        while self.entries.len() > MAX_SCOPE_ENTRIES {
            self.entries.remove(0);
        }
        let mut total: usize = self.entries.iter().map(|e| char_len(&e.prompt)).sum();
        while self.entries.len() > 1 && total > MAX_SCOPE_TOTAL_CHARS {
            let removed = self.entries.remove(0);
            total -= char_len(&removed.prompt);
        }
    }
}

/// The queue behind `markWatchdogAutoFollowPromptEvent`/`isWatchdogAutoFollowPromptEvent`
/// (`scope.ts:55-62`) as this port realizes it: the exact prompt texts the runtime has queued as
/// auto-follow, matched by value at `before_agent_start` (`runtime.ts:309-311`).
///
/// Semantics reproduced from `runtime.ts:307-311,675-681`:
///
/// * `mark` appends and then drops the OLDEST when the queue exceeds 8, so a runaway auto-follow
///   loop cannot grow it without bound.
/// * `take_match` removes the FIRST occurrence only — two auto-follows with identical text are two
///   distinct queued prompts, and consuming one must leave the other queued.
/// * A prompt that is not in the queue is a REAL user prompt, even if an auto-follow is pending:
///   upstream's comment at `:307-308` is explicit that a user prompt racing in ahead of a queued
///   auto-follow "stays real and the pending matches survive for later".
#[derive(Debug, Clone, Default)]
pub struct WatchdogAutoFollowPromptLedger {
    pending: Vec<String>,
}

impl WatchdogAutoFollowPromptLedger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `markWatchdogAutoFollowPromptEvent` (`scope.ts:59-62`) — queue this exact prompt text as
    /// watchdog-authored (`runtime.ts:675-676`).
    pub fn mark(&mut self, prompt: impl Into<String>) {
        self.pending.push(prompt.into());
        if self.pending.len() > MAX_PENDING_AUTO_FOLLOW_PROMPTS {
            self.pending.remove(0);
        }
    }

    /// `isWatchdogAutoFollowPromptEvent` (`scope.ts:55-57`) fused with the consuming
    /// `splice(pendingIndex, 1)` of `runtime.ts:311`: `true` when this prompt was queued (and it is
    /// removed), `false` when it is a real user prompt (and the queue is untouched).
    pub fn take_match(&mut self, prompt: Option<&str>) -> bool {
        let Some(prompt) = prompt else {
            return false;
        };
        let Some(index) = self.pending.iter().position(|p| p == prompt) else {
            return false;
        };
        self.pending.remove(index);
        true
    }

    /// Un-queue a prompt whose delivery FAILED (`runtime.ts:679-680`), so the failed follow-up does
    /// not later disarm a genuine user prompt with the same text.
    pub fn unmark(&mut self, prompt: &str) {
        if let Some(index) = self.pending.iter().position(|p| p == prompt) {
            self.pending.remove(index);
        }
    }

    /// Drop every queued prompt (`runtime.ts:273` — the `clearScope` reset arm clears the artifact
    /// and this queue together).
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// How many prompts are queued — the bound's observable.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Whether nothing is queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// `value.length` — JS counts UTF-16 code units, not chars or bytes.
fn char_len(value: &str) -> usize {
    value.encode_utf16().count()
}

/// `value.length > max ? value.slice(0, max) : value` (`scope.ts:19`), on UTF-16 code-unit counts.
/// A cut that would land inside a surrogate pair or a multi-byte UTF-8 sequence is moved back to
/// the preceding character boundary — JS can produce a lone surrogate there, Rust cannot hold one,
/// and the difference is one replacement character at a 2 000-character truncation point.
fn cap_chars(value: &str, max: usize) -> String {
    if char_len(value) <= max {
        return value.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in value.chars() {
        let width = ch.len_utf16();
        if used + width > max {
            break;
        }
        out.push(ch);
        used += width;
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    fn at(n: u32) -> Option<String> {
        Some(format!("1970-01-01T00:00:{n:02}.000Z"))
    }

    #[test]
    fn an_empty_artifact_renders_nothing() {
        assert_eq!(WatchdogScopeArtifact::new().render(), "");
    }

    #[test]
    fn a_blank_prompt_is_not_recorded() {
        let mut scope = WatchdogScopeArtifact::new();
        scope.add_prompt("   \n\t ", at(0));
        assert!(scope.snapshot().is_empty());
        assert_eq!(scope.render(), "");
    }

    #[test]
    fn prompts_are_trimmed_and_rendered_newest_last_with_the_supersede_instruction() {
        let mut scope = WatchdogScopeArtifact::new();
        scope.add_prompt("  first  ", at(1));
        scope.add_prompt("second", at(2));
        let rendered = scope.render();
        let expected = [
            "Current scope:",
            "The following real user prompts are the current scope record, newest last. Newer prompts supersede and mutate older prompts: they may add, modify, or remove requirements. Flag work that serves no current scope item as category 'scope-drift'.",
            "Scope prompt 1 (1970-01-01T00:00:01.000Z):\nfirst",
            "Scope prompt 2 (1970-01-01T00:00:02.000Z):\nsecond",
        ]
        .join("\n\n");
        assert_eq!(rendered, expected);
    }

    #[test]
    fn the_entry_count_bound_drops_the_oldest() {
        let mut scope = WatchdogScopeArtifact::new();
        for n in 0..12u32 {
            scope.add_prompt(&format!("prompt-{n}"), at(n));
        }
        let entries = scope.snapshot();
        assert_eq!(entries.len(), MAX_SCOPE_ENTRIES);
        assert_eq!(entries[0].prompt, "prompt-4");
        assert_eq!(entries[MAX_SCOPE_ENTRIES - 1].prompt, "prompt-11");
    }

    #[test]
    fn a_single_over_long_prompt_is_capped_at_the_entry_bound() {
        let mut scope = WatchdogScopeArtifact::new();
        scope.add_prompt(&"x".repeat(MAX_SCOPE_ENTRY_CHARS + 500), at(0));
        assert_eq!(scope.snapshot()[0].prompt.len(), MAX_SCOPE_ENTRY_CHARS);
    }

    #[test]
    fn the_total_bound_never_removes_the_last_entry() {
        let mut scope = WatchdogScopeArtifact::new();
        // 8 entries x 2 000 chars = 16 000, exactly at the total bound; a ninth pushes over it and
        // the entry bound already dropped the first, so the total bound must drop more.
        for n in 0..9u32 {
            scope.add_prompt(&"y".repeat(MAX_SCOPE_ENTRY_CHARS), at(n));
        }
        let entries = scope.snapshot();
        assert!(!entries.is_empty(), "the newest prompt always survives");
        let total: usize = entries.iter().map(|e| e.prompt.len()).sum();
        assert!(total <= MAX_SCOPE_TOTAL_CHARS, "total {total}");
    }

    #[test]
    fn even_one_prompt_over_the_total_bound_survives_alone() {
        let mut scope = WatchdogScopeArtifact::new();
        for n in 0..40u32 {
            scope.add_prompt(&"z".repeat(MAX_SCOPE_ENTRY_CHARS), at(n));
        }
        assert!(!scope.snapshot().is_empty());
    }

    #[test]
    fn reset_clears_the_record() {
        let mut scope = WatchdogScopeArtifact::new();
        scope.add_prompt("something", at(0));
        scope.reset();
        assert!(scope.snapshot().is_empty());
        assert_eq!(scope.render(), "");
    }

    #[test]
    fn the_ledger_consumes_one_queued_prompt_at_a_time() {
        let mut ledger = WatchdogAutoFollowPromptLedger::new();
        ledger.mark("follow up");
        ledger.mark("follow up");
        assert!(ledger.take_match(Some("follow up")));
        assert_eq!(ledger.len(), 1, "the duplicate stays queued");
        assert!(ledger.take_match(Some("follow up")));
        assert!(ledger.is_empty());
        assert!(!ledger.take_match(Some("follow up")));
    }

    #[test]
    fn a_real_user_prompt_racing_a_queued_auto_follow_stays_real() {
        let mut ledger = WatchdogAutoFollowPromptLedger::new();
        ledger.mark("watchdog text");
        assert!(!ledger.take_match(Some("a real user prompt")));
        assert_eq!(ledger.len(), 1, "the pending match survives for later");
        assert!(ledger.take_match(Some("watchdog text")));
    }

    #[test]
    fn a_missing_prompt_is_never_an_auto_follow() {
        let mut ledger = WatchdogAutoFollowPromptLedger::new();
        ledger.mark("watchdog text");
        assert!(!ledger.take_match(None));
        assert_eq!(ledger.len(), 1);
    }

    #[test]
    fn the_ledger_is_bounded_at_eight() {
        let mut ledger = WatchdogAutoFollowPromptLedger::new();
        for n in 0..12 {
            ledger.mark(format!("p{n}"));
        }
        assert_eq!(ledger.len(), MAX_PENDING_AUTO_FOLLOW_PROMPTS);
        assert!(!ledger.take_match(Some("p0")), "the oldest was dropped");
        assert!(ledger.take_match(Some("p11")));
    }

    #[test]
    fn a_failed_delivery_unmarks_the_prompt() {
        let mut ledger = WatchdogAutoFollowPromptLedger::new();
        ledger.mark("queued");
        ledger.unmark("queued");
        assert!(ledger.is_empty());
        assert!(!ledger.take_match(Some("queued")));
    }

    #[test]
    fn utf16_capping_never_splits_a_character() {
        // An astral character is 2 UTF-16 code units, so an odd cap lands mid-pair.
        let value = "😀".repeat(1_500);
        let capped = cap_chars(&value, MAX_SCOPE_ENTRY_CHARS + 1);
        assert!(char_len(&capped) <= MAX_SCOPE_ENTRY_CHARS + 1);
        assert!(capped.chars().all(|c| c == '😀'));
    }
}
