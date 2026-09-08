//! SCOPE_17 — the result-summary contract: a system-prompt fold that asks every child for a
//! bounded, tail-delimited summary of its answer, and the extractor that reads it back with a
//! fallback that cannot fail. Sibling of [`crate::exec::turn_budget`], composed at the same seam
//! in `spawn_plan.rs`'s persona composition so a foreground child and a detached fan-out member
//! get the identical contract with no per-path wiring.

/// The tail marker [`append_result_summary_system_prompt`] asks every child to close its response
/// with.
const RESULT_MARKER: &str = "<<<RESULT>>>";

/// The bounded summary of one child's output: the contract block when the child honoured it, the
/// output's TAIL when it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultSummary {
    /// The summary text, bounded to `max_bytes` on a UTF-8 boundary.
    pub text: String,
    /// `true` when the child honoured the contract — diagnostic only; both sources are usable and
    /// neither is rendered differently.
    pub from_contract: bool,
    /// `true` when text was dropped to fit, so a renderer can say so and point at the full output.
    pub truncated: bool,
}

/// Fold the result-summary contract onto the child's system prompt, so its answer can cross into
/// another agent's context bounded rather than whole.
///
/// Sibling of [`crate::exec::turn_budget::append_turn_budget_system_prompt`] and composed at the
/// same point in `spawn_plan`, which is the ONE seam every child argv is built through — so a
/// foreground child and a detached fan-out member get the identical contract with no per-path
/// wiring.
///
/// # Not applied when the child is already structured
///
/// An agent with an `output_schema` is contractually producing a machine-readable value; asking it
/// to ALSO emit a prose block invites it to satisfy one contract and break the other, and
/// `structured_output` is already a better summary than any prose tail. Returns the persona
/// untouched in that case, exactly as the turn-budget appender does with no budget.
#[must_use]
pub fn append_result_summary_system_prompt(system_prompt: &str, applies: bool) -> String {
    if !applies {
        return system_prompt.to_string();
    }
    let block = [
        "## Result summary".to_string(),
        "End your final response with a summary block, exactly:".to_string(),
        String::new(),
        "<<<RESULT>>>".to_string(),
        "<your result here>".to_string(),
        String::new(),
        "Put the ANSWER in that block — the value, decision, or outcome asked for — not a description of".to_string(),
        "what you did. Keep it under 400 characters. Everything above the marker is kept in full for anyone".to_string(),
        "who needs the detail; the block is what other agents see first.".to_string(),
    ]
    .join("\n");
    let trimmed = system_prompt.trim();
    if trimmed.is_empty() {
        block
    } else {
        format!("{trimmed}\n\n{block}")
    }
}

/// # The fallback is the load-bearing half
///
/// The marker is the fast path, not the contract. A child that ignores the instruction, a model
/// that reformats it, an agent definition predating it, a `Replace`-mode persona that dropped it —
/// all still yield a usable summary, because a conclusion sits at the END of a response. That is
/// why this degrades instead of breaking, and why no agent definition needs migrating.
///
/// TAIL, not head: [`crate::exec::child_protocol::BoundedByteTail`] is this crate's existing
/// UTF-8-boundary-safe tail ring (`.push(bytes)` then `.text()`) — the exact same primitive
/// `child-protocol.ts`'s stderr capture uses. Head truncation would return the preamble and drop
/// the answer — the exact failure this task exists to fix.
///
/// The LAST marker occurrence wins ([`str::rfind`], never `find`), so a child that quotes the
/// format while reasoning about it cannot spoof an earlier block. An empty block after the last
/// marker (the child emitted the delimiter but nothing after it) falls through to the tail exactly
/// as if no marker had been emitted at all.
#[must_use]
pub fn extract_result_summary(final_output: &str, max_bytes: usize) -> ResultSummary {
    let (source, from_contract) = match final_output.rfind(RESULT_MARKER) {
        Some(marker_at) => {
            let after = final_output
                .get(marker_at + RESULT_MARKER.len()..)
                .unwrap_or("")
                .trim();
            if after.is_empty() {
                (final_output, false)
            } else {
                (after, true)
            }
        }
        None => (final_output, false),
    };
    let source = source.trim();
    let cap = max_bytes.max(1);
    let mut tail = crate::exec::child_protocol::BoundedByteTail::new(cap);
    tail.push(source.as_bytes());
    ResultSummary {
        text: tail.text(),
        from_contract,
        truncated: source.len() > cap,
    }
}
