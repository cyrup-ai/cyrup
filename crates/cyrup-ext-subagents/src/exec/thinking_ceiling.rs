//! SUBA-078 — the `subagents.maxThinking` reasoning-level ceiling (pi
//! `src/shared/thinking-ceiling.ts` @v0.57.0, `547112ec feat: add max thinking ceiling for
//! subagents #1397`).
//!
//! An operator-configured UPPER BOUND on how much reasoning a subagent subtree may spend. It
//! behaves like this crate's other two bounds — [`crate::exec::model_scope`] and
//! [`crate::exec::capability_ceiling`] — in the three ways that matter:
//!
//! 1. **It refuses, it does not clamp.** Silently lowering an agent's level would run it shallower
//!    than it asked while reporting success, hiding the misconfiguration.
//! 2. **It is monotonic across the process boundary.** [`crate::exec::thinking_ceiling::intersect_thinking_ceilings`]
//!    takes the
//!    LOWEST of everything in play, and the resolved value is written to
//!    [`crate::exec::thinking_ceiling::THINKING_CEILING_ENV`] for the child, so a nested subtree
//!    can only ever tighten.
//! 3. **It is fail-CLOSED.** A malformed inherited ceiling is an error, never "unbounded" — a bound
//!    that degrades to nothing inverts the guarantee exactly when it matters.
//!
//! The ceiling is SETTINGS-ONLY. Upstream keeps `maxThinking` out of the agent frontmatter contract
//! entirely (it is absent from `agent-serializer.ts`'s `KNOWN_FIELDS`) and merely stamps the
//! resolved settings value onto each agent so the launch path can read it off one struct. This port
//! goes further and never puts it on [`crate::discovery::types::AgentDefinition`] at all, carrying
//! it on the discovery RESULT the way `modelScope` already travels: a value that cannot appear in
//! frontmatter cannot be authored by an agent raising its own ceiling, and cannot be round-tripped
//! into an agent file by the management serializer.

use crate::exec::spawn_plan::THINKING_LEVELS;

/// pi `SUBAGENT_THINKING_CEILING_ENV = "PI_SUBAGENT_THINKING_CEILING"`
/// (`shared/thinking-ceiling.ts:4` @v0.57.0), under this crate's own prefix exactly as
/// [`crate::exec::capability_ceiling::CAPABILITY_CEILING_ENV`] is.
pub const THINKING_CEILING_ENV: &str = "CYRUP_SUBAGENT_THINKING_CEILING";

/// The upstream spelling of [`THINKING_CEILING_ENV`], honoured on READ only, so a cyrup child
/// launched by a pi parent still inherits the bound. Same compatibility rule (and same read-only
/// direction) as [`crate::exec::capability_ceiling::CAPABILITY_CEILING_ENV_PI_ALIAS`].
pub const THINKING_CEILING_ENV_PI_ALIAS: &str = "PI_SUBAGENT_THINKING_CEILING";

/// Rank of `level` within [`THINKING_LEVELS`] — its index, so `off` is `0` and therefore the
/// TIGHTEST ceiling, and `max` is the loosest. `None` for an unrecognized level.
///
/// pi builds the same map at `shared/thinking-ceiling.ts:6`.
fn thinking_level_rank(level: &str) -> Option<usize> {
    THINKING_LEVELS.iter().position(|known| *known == level)
}

/// The `expected one of …` tail every message in this module shares (pi `THINKING_LEVELS.join(", ")`).
fn expected_levels() -> String {
    THINKING_LEVELS.join(", ")
}

/// pi `parseThinkingLevel` (`shared/thinking-ceiling.ts:8-14` @v0.57.0): a recognized level, TRIMMED,
/// or an error naming `field`.
///
/// # Errors
///
/// `Invalid {field}; expected one of off, minimal, low, medium, high, xhigh, max.` — upstream's
/// message verbatim, for any absent, non-string or unrecognized value.
pub fn parse_thinking_level(value: Option<&str>, field: &str) -> Result<String, String> {
    if let Some(trimmed) = value.map(str::trim)
        && thinking_level_rank(trimmed).is_some()
    {
        return Ok(trimmed.to_string());
    }
    Err(format!(
        "Invalid {field}; expected one of {}.",
        expected_levels()
    ))
}

/// pi `intersectThinkingCeilings` (`shared/thinking-ceiling.ts:23-27` @v0.57.0): the LOWEST of the
/// present ceilings, or `None` when none are present.
///
/// Taking the lowest is the whole mechanism — it is why a bound can only tighten as the tree
/// deepens, and why a child intersecting what it inherited with its own configuration can never
/// widen what its parent allowed.
///
/// # Errors
///
/// pi reduces with `compareThinkingLevels`, which THROWS for a level it cannot rank
/// (`shared/thinking-ceiling.ts:16-21` @v0.57.0), and so does this.
///
/// DROPPING an unrankable entry instead would be fail-OPEN, which is the one outcome this module
/// exists to prevent: this function runs BEFORE
/// [`assert_thinking_within_ceiling`] at every call site, so a dropped entry never reaches the
/// assert — the fold yields `None`, the assert no-ops, and the run proceeds with NO ceiling at all,
/// while the env write below it is suppressed so the child inherits nothing either. A bound the
/// caller asked for would silently vanish. Every in-tree path validates before calling here, but
/// `RunOptions::thinking_ceiling` is a public `Option<String>` on a public module, so the guarantee
/// has to hold at this seam rather than by convention.
pub fn intersect_thinking_ceilings(ceilings: &[Option<&str>]) -> Result<Option<String>, String> {
    let mut lowest: Option<(usize, &str)> = Option::None;
    for level in ceilings.iter().flatten() {
        let rank = thinking_level_rank(level).ok_or_else(|| {
            format!(
                "Invalid thinking level comparison; expected one of {}.",
                expected_levels()
            )
        })?;
        if lowest.is_none_or(|(current, _)| rank < current) {
            lowest = Some((rank, level));
        }
    }
    Ok(lowest.map(|(_, level)| level.to_string()))
}

/// pi `decodeThinkingCeiling` (`shared/thinking-ceiling.ts:29-32` @v0.57.0): an absent or EMPTY
/// value is simply no inherited ceiling; anything else must parse.
///
/// # Errors
///
/// Fail-CLOSED: a malformed inherited ceiling is an error, not "unbounded". Contrast
/// `subagents.timeoutMs` (SUBA-077), which degrades silently — a timeout falling back to a default
/// is still bounded, whereas a ceiling falling back to nothing is a bound that vanished.
pub fn decode_thinking_ceiling(value: Option<&str>) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some("") => Ok(None),
        Some(raw) => parse_thinking_level(Some(raw), "inherited thinking ceiling").map(Some),
    }
}

/// This process's own inherited ceiling, read from [`THINKING_CEILING_ENV`] with
/// [`THINKING_CEILING_ENV_PI_ALIAS`] as a fallback (pi reads `process.env[…]` directly at
/// `execution.ts:1714` and `pi-args.ts:877`).
///
/// # Errors
///
/// Propagates [`decode_thinking_ceiling`]'s fail-closed error.
pub fn inherited_thinking_ceiling() -> Result<Option<String>, String> {
    let raw = std::env::var(THINKING_CEILING_ENV)
        .or_else(|_| std::env::var(THINKING_CEILING_ENV_PI_ALIAS))
        .ok();
    decode_thinking_ceiling(raw.as_deref())
}

/// pi `assertThinkingWithinCeiling` (`shared/thinking-ceiling.ts:42-55` @v0.57.0).
///
/// Two early returns, both load-bearing. No ceiling means no check. And no RESOLVED level means no
/// check either — `resolveEffectiveThinking` yields nothing without a model, so a run whose model is
/// unresolved is never refused by this bound; there is simply nothing to compare.
///
/// `config_thinking` is the agent's own open `thinking:` string. It is consulted only when the model
/// id carries no `:<level>` suffix of its own, and only when it names a recognized level — so a
/// `thinking: false` (or any other non-level string) contributes nothing, exactly as upstream's
/// `THINKING_LEVELS.find((level) => level === configThinking)` does.
///
/// # Errors
///
/// `Thinking level '<requested>' exceeds configured maximum '<ceiling>' for agent '<a>' run '<r>'.`
/// — upstream's message, including its OPTIONAL subject clause: the `agent`/`run` parts are joined
/// by a single space, and the whole ` for …` clause is omitted when both are absent.
pub fn assert_thinking_within_ceiling(
    model: Option<&str>,
    config_thinking: Option<&str>,
    ceiling: Option<&str>,
    agent: Option<&str>,
    run_id: Option<&str>,
) -> Result<(), String> {
    let Some(ceiling) = ceiling else {
        return Ok(());
    };
    // Reuses the existing port of pi `resolveEffectiveThinking` (`shared/model-info.ts:35-40`)
    // rather than re-deriving it: a `:suffix` on the model wins outright, else the config value,
    // and only if it is a recognized level.
    let config = config_thinking
        .map(|level| crate::watchdog::types::ThinkingSetting::Level(level.to_string()));
    let Some(requested) =
        crate::watchdog::review::resolve_effective_thinking(model, config.as_ref())
    else {
        return Ok(());
    };
    let requested = parse_thinking_level(Some(&requested), "requested thinking level")?;
    let (Some(requested_rank), Some(ceiling_rank)) = (
        thinking_level_rank(&requested),
        thinking_level_rank(ceiling),
    ) else {
        return Err(format!(
            "Invalid thinking level comparison; expected one of {}.",
            expected_levels()
        ));
    };
    if requested_rank <= ceiling_rank {
        return Ok(());
    }
    let subject = [
        agent.map(|name| format!("agent '{name}'")),
        run_id.map(|id| format!("run '{id}'")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    let subject = if subject.is_empty() {
        String::new()
    } else {
        format!(" for {subject}")
    };
    Err(format!(
        "Thinking level '{requested}' exceeds configured maximum '{ceiling}'{subject}."
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    /// pi ranks by INDEX into `THINKING_LEVELS`, so `off` is the tightest ceiling and `max` the
    /// loosest. Getting this backwards would invert every bound in the module.
    #[test]
    fn levels_rank_by_index_so_off_is_the_tightest_ceiling() {
        assert_eq!(thinking_level_rank("off"), Some(0));
        assert_eq!(thinking_level_rank("max"), Some(THINKING_LEVELS.len() - 1));
        assert!(thinking_level_rank("low") < thinking_level_rank("high"));
        assert_eq!(thinking_level_rank("nonsense"), Option::None);
    }

    #[test]
    fn parse_trims_and_names_the_field_in_upstreams_message() {
        assert_eq!(
            parse_thinking_level(Some("  high  "), "x"),
            Ok("high".to_string())
        );
        assert_eq!(
            parse_thinking_level(Some("nope"), "requested thinking level"),
            Err(
                "Invalid requested thinking level; expected one of off, minimal, low, medium, \
                 high, xhigh, max."
                    .to_string()
            )
        );
        assert!(
            parse_thinking_level(Option::None, "x").is_err(),
            "absent is invalid too"
        );
    }

    /// The mechanism: the LOWEST wins, which is what makes a bound able only to tighten as the
    /// tree deepens. A child that inherits `low` and configures `high` stays bound by `low`.
    #[test]
    fn intersection_takes_the_lowest_so_a_child_can_only_tighten() {
        let lowest = |ceilings: &[Option<&str>]| {
            intersect_thinking_ceilings(ceilings).expect("all inputs here are valid levels")
        };
        assert_eq!(
            lowest(&[Some("high"), Some("low")]).as_deref(),
            Some("low"),
            "an inherited `low` must survive a locally-configured `high`"
        );
        assert_eq!(lowest(&[Some("low"), Some("off")]).as_deref(), Some("off"));
        assert_eq!(
            lowest(&[Option::None, Some("medium")]).as_deref(),
            Some("medium"),
            "absent entries are skipped, not treated as a bound"
        );
        assert_eq!(
            lowest(&[Option::None, Option::None]),
            Option::None,
            "nothing configured anywhere is NO ceiling, not the tightest one"
        );
    }

    /// pi reduces with `compareThinkingLevels`, which THROWS for a level it cannot rank
    /// (`shared/thinking-ceiling.ts:16-21` @v0.57.0).
    ///
    /// DROPPING the entry instead would be fail-OPEN, and silently so: this function runs BEFORE
    /// [`assert_thinking_within_ceiling`] at every call site, so a dropped entry never reaches the
    /// assert — the fold yields `None`, the assert no-ops, and the run proceeds with NO ceiling
    /// while the env write is suppressed too. The bound the caller asked for would just vanish.
    #[test]
    fn an_unrankable_entry_errors_rather_than_silently_erasing_the_bound() {
        let err = intersect_thinking_ceilings(&[Some("garbage")])
            .expect_err("an unrankable level must not be dropped");
        assert_eq!(
            err,
            "Invalid thinking level comparison; expected one of off, minimal, low, medium, high, \
             xhigh, max."
        );
        // ...and it errors even when a VALID entry sits beside it, rather than quietly using that
        // one: the caller supplied two bounds and only one was understood.
        assert!(intersect_thinking_ceilings(&[Some("low"), Some("garbage")]).is_err());
        assert!(intersect_thinking_ceilings(&[Some("garbage"), Some("low")]).is_err());
    }

    /// pi `decodeThinkingCeiling`: absent and EMPTY both mean "no inherited ceiling", but anything
    /// else must parse. Fail-CLOSED — a ceiling that degraded to "unbounded" would invert the
    /// guarantee exactly when it matters.
    #[test]
    fn a_malformed_inherited_ceiling_is_an_error_never_unbounded() {
        assert_eq!(decode_thinking_ceiling(Option::None), Ok(Option::None));
        assert_eq!(decode_thinking_ceiling(Some("")), Ok(Option::None));
        assert_eq!(
            decode_thinking_ceiling(Some("low")),
            Ok(Some("low".to_string()))
        );
        let err = decode_thinking_ceiling(Some("garbage")).expect_err("must not degrade");
        assert!(err.contains("Invalid inherited thinking ceiling;"), "{err}");
    }

    #[test]
    fn a_level_above_the_ceiling_is_refused_with_upstreams_message() {
        let err = assert_thinking_within_ceiling(
            Some("anthropic/claude-opus-4-6"),
            Some("xhigh"),
            Some("low"),
            Some("worker"),
            Some("r1"),
        )
        .expect_err("xhigh exceeds low");
        assert_eq!(
            err,
            "Thinking level 'xhigh' exceeds configured maximum 'low' for agent 'worker' run 'r1'."
        );

        // The subject clause is OPTIONAL upstream, and collapses entirely when neither part is
        // given — no dangling " for ".
        let err = assert_thinking_within_ceiling(
            Some("anthropic/claude-opus-4-6"),
            Some("xhigh"),
            Some("low"),
            Option::None,
            Option::None,
        )
        .expect_err("xhigh exceeds low");
        assert_eq!(
            err,
            "Thinking level 'xhigh' exceeds configured maximum 'low'."
        );
    }

    #[test]
    fn a_level_at_or_below_the_ceiling_passes() {
        for level in ["off", "minimal", "low"] {
            assert_eq!(
                assert_thinking_within_ceiling(
                    Some("m"),
                    Some(level),
                    Some("low"),
                    Option::None,
                    Option::None
                ),
                Ok(()),
                "{level} is within 'low'"
            );
        }
    }

    /// The two early returns. Losing either turns this bound into a source of spurious refusals:
    /// with no ceiling there is nothing to enforce, and with no RESOLVED level there is nothing to
    /// compare — `resolveEffectiveThinking` yields nothing without a model.
    #[test]
    fn the_two_no_op_arms_never_refuse() {
        assert_eq!(
            assert_thinking_within_ceiling(
                Some("m"),
                Some("xhigh"),
                Option::None,
                Option::None,
                Option::None
            ),
            Ok(()),
            "no ceiling configured => no check"
        );
        assert_eq!(
            assert_thinking_within_ceiling(
                Option::None,
                Some("xhigh"),
                Some("off"),
                Option::None,
                Option::None
            ),
            Ok(()),
            "no model => nothing resolves => no check, even against the tightest ceiling"
        );
        assert_eq!(
            assert_thinking_within_ceiling(
                Some("m"),
                Some("false"),
                Some("off"),
                Option::None,
                Option::None
            ),
            Ok(()),
            "a non-level `thinking:` contributes nothing, exactly as pi's `find(level === config)`"
        );
    }

    /// A `:suffix` on the model id wins over the agent's `thinking:`, so the ceiling is checked
    /// against what the child will REALLY run at.
    #[test]
    fn a_model_suffix_outranks_the_config_level_for_the_check() {
        let err = assert_thinking_within_ceiling(
            Some("anthropic/claude-opus-4-6:xhigh"),
            Some("off"),
            Some("low"),
            Option::None,
            Option::None,
        )
        .expect_err("the suffix is what the child would run at");
        assert!(
            err.contains("'xhigh' exceeds configured maximum 'low'"),
            "{err}"
        );
    }

    /// SUBA-078: the ceiling is SETTINGS-ONLY. If `maxThinking` ever became an authored frontmatter
    /// key, an agent file could raise its own bound to `max` and defeat the operator's ceiling —
    /// and the management serializer would start writing the key into agent files.
    #[test]
    fn max_thinking_is_not_an_authored_agent_frontmatter_key() {
        assert!(
            !crate::discovery::frontmatter::is_known_field("maxThinking"),
            "an agent must never be able to author its own thinking ceiling"
        );
    }
}
