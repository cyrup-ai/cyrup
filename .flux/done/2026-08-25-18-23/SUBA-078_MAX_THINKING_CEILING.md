---
stage: qa
status: completed
updated: 2026-08-28 16:45
severity: high
effort: small
subsystem: discovery settings / thinking
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-078
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-078 — QA rework: `intersect_thinking_ceilings` is fail-OPEN on an unrecognized level

**QA verdict 9/10.** The port is otherwise complete and correct: the module, the settings→merge→result
pipeline, the threading, both enforcement sites, and the env propagation all landed and are
mutation-proven. 2561 tests pass, clippy is back to its 5 pre-existing warnings, `cargo doc` is
clean, and the workspace check is clean. Everything below is the ONE outstanding item.

---

## The defect

[`exec/thinking_ceiling.rs`](../../../crates/cyrup-ext-subagents/src/exec/thinking_ceiling.rs)
silently DISCARDS an unrecognized level instead of erroring:

```rust
pub fn intersect_thinking_ceilings(ceilings: &[Option<&str>]) -> Option<String> {
    ceilings
        .iter()
        .flatten()
        .filter_map(|level| thinking_level_rank(level).map(|rank| (rank, *level)))   // <- drops garbage
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, level)| level.to_string())
}
```

Upstream does not. `intersectThinkingCeilings` reduces with `compareThinkingLevels`, which **throws**
for a level it cannot rank (`shared/thinking-ceiling.ts:16-21` @v0.57.0):

```js
if (leftRank === undefined || rightRank === undefined) throw new Error(`Invalid thinking level comparison; expected one of ${THINKING_LEVELS.join(", ")}.`);
```

**Why it matters, concretely.** `assert_thinking_within_ceiling` IS fail-closed on a garbage ceiling
— it already returns exactly that `Invalid thinking level comparison; …` message. But `intersect`
runs FIRST at every call site, so garbage never reaches the assert: it is dropped, the fold yields
`None`, the assert no-ops, and **the run proceeds with no ceiling at all**. A bound that was asked
for silently vanishes. The same drop also suppresses the env write, so the child inherits nothing
either.

That is fail-OPEN, in a module whose own header lists fail-closed as principle 3:

> 3. **It is fail-CLOSED.** A malformed inherited ceiling is an error, never "unbounded" — a bound
>    that degrades to nothing inverts the guarantee exactly when it matters.

**Reachability.** Every in-tree production path validates first (settings parse through
`parse_thinking_level`, the env through `decode_thinking_ceiling`), so this cannot fire today from
config. But `lib.rs:31` is `pub mod exec;` and `RunOptions::thinking_ceiling` is a `pub
Option<String>`, so any embedder — or any future in-crate caller — can set an unvalidated string and
get an unbounded run where they asked for a bound. Low probability; wrong direction.

**The comment argues against the wrong alternative.** It reads:

> *"treating garbage as the tightest bound would refuse runs for a typo"*

Upstream's alternative is not "treat as tightest" — it is **error**. That reasoning should not
survive the fix.

## Required change

Make the function fallible, matching its two siblings in the same module (`decode_thinking_ceiling`
and `assert_thinking_within_ceiling` both already return `Result`):

```rust
/// # Errors
///
/// pi `compareThinkingLevels` (`shared/thinking-ceiling.ts:16-21` @v0.57.0) THROWS for a level it
/// cannot rank, and so does this. Dropping the entry instead would silently erase a bound the
/// caller asked for — `assert_thinking_within_ceiling` never sees it, and the run proceeds
/// unbounded — which is the one outcome this module exists to prevent.
pub fn intersect_thinking_ceilings(ceilings: &[Option<&str>]) -> Result<Option<String>, String> {
    let mut lowest: Option<(usize, &str)> = Option::None;
    for level in ceilings.iter().flatten() {
        let rank = thinking_level_rank(level).ok_or_else(|| {
            format!("Invalid thinking level comparison; expected one of {}.", expected_levels())
        })?;
        if lowest.is_none_or(|(current, _)| rank < current) {
            lowest = Some((rank, level));
        }
    }
    Ok(lowest.map(|(_, level)| level.to_string()))
}
```

Three call sites, each already in a fallible context:

- [`exec/mod.rs`](../../../crates/cyrup-ext-subagents/src/exec/mod.rs) `run_sync`'s Step 2b — the
  fold is already inside a `match … { Err(error) => return pre_spawn_failure(…) }`; extend that arm
  to cover the intersect too.
- [`exec/spawn_plan.rs`](../../../crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) ×2 (the
  last-moment assert and the env write) — both already
  `.map_err(SubagentError::ThinkingCeilingViolation)?` on the neighbouring `inherited_thinking_ceiling()`
  call, so the same `?` applies.

> If you would rather make the state unrepresentable than validate it, a `ThinkingLevel` newtype
> whose only constructor is `parse_thinking_level` is the stronger fix — it is what upstream's
> `ThinkingLevel` union type gives it for free, and it would also tighten
> `RunOptions::thinking_ceiling`. That is a larger change than this item needs; the `Result` above
> is the minimum that closes the hole.

## Definition of done

1. `intersect_thinking_ceilings(&[Some("garbage")])` is an `Err`, not a silently-dropped `Ok(None)`.
2. A `RunOptions::thinking_ceiling` carrying an unrecognized level REFUSES the run rather than
   running it unbounded — the behaviour the module's fail-closed principle already claims.
3. The valid-input behaviour is unchanged: lowest-wins, absent entries skipped, all-absent → `None`.
4. The comment no longer argues against "treating garbage as the tightest bound".
5. `cargo test -p cyrup-ext-subagents`, `cargo clippy -p cyrup-ext-subagents --all-targets` and
   `cargo doc -p cyrup-ext-subagents --no-deps --lib` stay as clean as they are now (2561 passing,
   no new clippy finding, no doc warning). Reverting the fix must fail (1).

---

## Settled — do NOT reopen

Verified this pass; recorded so the next round is one round.

- **The bypass guarantee holds structurally.** `max_thinking` appears nowhere in
  `discovery/frontmatter.rs` (so not in `KNOWN_FIELDS`, never parsed), nowhere on `AgentDefinition`,
  and nowhere in `discovery/management/` (so the serializer cannot write it into an agent file). It
  lives only on `SubagentSettings` and the `LayeredOverrideSettings` accessor. A test pins
  `!is_known_field("maxThinking")`.
- **The mid-task corruption was fully recovered.** `discovery/mod.rs` is genuinely itself — its own
  module header, 16 hits for its own functions, and ZERO `exec/mod.rs` symbols — with all four
  SUBA-078 edits present at `:709`, `:894`, `:1216`, `:1314`.
- **`assert_thinking_within_ceiling` is faithful**: both early returns present (no ceiling → no
  check; no resolved level → no check), the `<=` boundary is correct, the optional subject clause
  collapses cleanly, and an unrankable ceiling reaching it errors with upstream's
  `compareThinkingLevels` message.
- **Refusal, not clamping**, and no warn tier for fallback rungs — correctly unlike `model_scope`.
- **The settings error drops the file path.** `parse_subagent_settings` has no path in scope and its
  sibling `validate_default_thinking` drops it for the same reason; the Oxford `or max` that
  distinguishes upstream's outer message is preserved.
- **Reusing `watchdog::review::resolve_effective_thinking`** rather than writing a third copy, and
  aliasing `spawn_plan::THINKING_LEVELS` rather than adding a fifth list.
- **Carrying the ceiling on the discovery result** instead of stamping it onto `AgentDefinition` —
  a deliberate divergence from upstream's mechanism that reaches the same enforcement points while
  making the bypass unrepresentable.
