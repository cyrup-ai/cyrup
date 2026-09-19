//! LANES_2 — the five convergence verbs, as ONE type.
//!
//! `worktree.cleanup`, `worktree.discard`, `lane.status`, `lane.recordMerge` and
//! `lane.recordSupersession` are pi `runs/foreground/subagent-executor.ts:6213-6293` @v0.68.0,
//! which dispatches them as three separate `if` blocks over raw strings and decides three
//! orthogonal properties — *is this mutating*, *does it consult the authority policy*, *does it
//! require `handoffPath`* — with independent comparisons that can drift apart.
//!
//! Here they are a five-variant enum and three `match`es the compiler checks for exhaustiveness.
//! That is not cosmetic: [`LaneAction::is_mutating`] IS pi's child-safe split (`:6271`'s
//! `action !== "lane.status" && allowMutatingManagementActions === false`), and `lane.status`
//! being its one `false` is the whole reason a delegated child can read its own lane graph. A
//! boolean field on a struct can be set wrong; a match arm on a five-variant enum cannot be
//! forgotten.
//!
//! # Why this file lives under `src/extension/`
//!
//! `schema::tests::every_advertised_schema_property_is_read_outside_provided_keys` walks **only**
//! the `src/extension/` tree, excises every `provided_keys()` body, and fails for any advertised
//! schema property with no surviving `.field` read. The `p.handoff_path` / `p.lane_id` /
//! `p.merge` / `p.supersession` reads therefore have to be here to count as wired — a read living
//! in `handoff/` or `spawn/` would leave all four properties reported as advertised-but-unread.
//! `params.rs:277` records the same placement rule for the `schedule.*` projection.

use std::path::{Path, PathBuf};

use crate::registration::authority::AuthorityAction;

/// The five convergence verbs — pi `subagent-executor.ts:6213`, `:6242`, `:6270` @v0.68.0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaneAction {
    /// Plan which fan-out worktrees are safe to remove. Plan-only; removes nothing.
    WorktreeCleanup,
    /// Remove the worktrees a fan-out deliberately preserved.
    WorktreeDiscard,
    /// Render one manifest's cleanup eligibility. **The one read-only verb.**
    LaneStatus,
    /// Attest that a lane merged.
    LaneRecordMerge,
    /// Attest that a lane was superseded by another.
    LaneRecordSupersession,
}

impl LaneAction {
    /// The wire name, or `None` for anything else — the same `from_wire` shape
    /// [`crate::background::scheduled_runs::ScheduledRunAction`] uses, so `route_action` needs
    /// ONE guard arm for the whole family instead of five `|`-joined literals.
    pub(crate) fn from_wire(action: &str) -> Option<Self> {
        match action {
            "worktree.cleanup" => Some(Self::WorktreeCleanup),
            "worktree.discard" => Some(Self::WorktreeDiscard),
            "lane.status" => Some(Self::LaneStatus),
            "lane.recordMerge" => Some(Self::LaneRecordMerge),
            "lane.recordSupersession" => Some(Self::LaneRecordSupersession),
            _ => None,
        }
    }

    /// Round-trips [`Self::from_wire`]. Used wherever a refusal interpolates the verb, so the
    /// sentence a caller reads always names the verb it actually invoked.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::WorktreeCleanup => "worktree.cleanup",
            Self::WorktreeDiscard => "worktree.discard",
            Self::LaneStatus => "lane.status",
            Self::LaneRecordMerge => "lane.recordMerge",
            Self::LaneRecordSupersession => "lane.recordSupersession",
        }
    }

    /// pi's child-safe gate (`:6214`, `:6243`, `:6271`).
    ///
    /// `lane.status` is a pure read and is DELIBERATELY available to a child-safe fanout tool;
    /// every other verb writes a manifest, a plan file or the filesystem. **This must never
    /// become `true` for [`Self::LaneStatus`]** — routing all five through a predicate that
    /// answers `true` everywhere would silently cost a delegated child the ability to read its
    /// own lane graph, which is the point of the feature for delegated work.
    pub(crate) fn is_mutating(self) -> bool {
        !matches!(self, Self::LaneStatus)
    }

    /// The authority action to consult, for the ONE verb that has one.
    ///
    /// pi consults `discardWorktree` for `worktree.discard` (`:6249`) and consults NOTHING for
    /// the other four: `worktree.cleanup` is plan-only and removes nothing (`:6217`), and the
    /// three `lane.*` verbs edit a JSON file.
    ///
    /// **Delegates to [`AuthorityAction::for_tool_action`] rather than re-deciding**, so the
    /// crate has ONE table mapping a tool verb to a policy action. A second `matches!` here
    /// would compile, pass its own test, and still let the two disagree the day somebody edits
    /// only one of them — and the half that the dispatch does not read is the half that silently
    /// stops gating anything.
    ///
    /// `worktree.cleanup` is deliberately absent from that table:
    /// `git grep -n destructiveCleanup v0.68.0 -- src` returns exactly two hits, both inside
    /// `policy/authority.ts` (`:3` the action list, `:18` the default) — upstream declares
    /// `destructiveCleanup` and never consults it anywhere. A confirm prompt for a deletion that
    /// cannot occur teaches operators to click through prompts.
    pub(crate) fn authority_action(self) -> Option<AuthorityAction> {
        AuthorityAction::for_tool_action(self.as_str())
    }

    /// Whether `handoffPath` is REQUIRED.
    ///
    /// `worktree.cleanup` takes it optionally — pi `:6228` is a spread, so an absent path means
    /// "discover every manifest under `repo`". The other four require it (`:6246`, `:6277`).
    pub(crate) fn requires_handoff_path(self) -> bool {
        !matches!(self, Self::WorktreeCleanup)
    }
}

/// Every refusal these five verbs can answer with, carrying pi's wording verbatim.
///
/// A typed enum rather than seven `format!`s at seven call sites, because `text.rs`'s
/// parity-of-wording convention means these strings ARE the contract and three of them are shared
/// across verbs — one `Display` impl is one place to get them right.
///
/// Two sentences from these dispatch blocks are deliberately NOT here:
/// * `Lane '{lane}' does not match manifest run '{run}'.` (`:6283`) is
///   [`crate::handoff::HandoffError::LaneRunMismatch`], owned by the manifest model. Declaring it
///   twice would give the crate two spellings of one contract.
/// * *"Worktree discard canceled; preserved worktrees were not changed."* (`:6258`) has no
///   `isError` upstream, so it is an `Ok` result rather than a refusal variant. A user declining
///   a confirm is a CHOICE; returning `Err` would make a deliberate "no" look like a failure to
///   an orchestrator retry loop, which would then re-prompt.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum LaneActionRefusal {
    /// pi `:6215`, `:6244`, `:6272` — shared with every other child-safe refusal in this crate.
    #[error("Action '{0}' is not available from child-safe subagent fanout mode.")]
    ChildSafe(&'static str),
    /// pi `:6218`.
    #[error(
        "worktree.cleanup currently supports mode='plan' only; apply/removal is not available yet."
    )]
    CleanupPlanOnly,
    /// pi `:6221`.
    #[error("worktree.cleanup plan mode does not accept planId; apply is not available yet.")]
    CleanupNoPlanId,
    /// pi `:6247`.
    #[error("worktree.discard requires handoffPath from parallelHandoff.path or async status.")]
    DiscardRequiresHandoffPath,
    /// pi `:6275`.
    #[error("{0} requires laneId.")]
    LaneRequiresLaneId(&'static str),
    /// pi `:6277`.
    #[error("{0} requires handoffPath for the existing parallel handoff manifest.")]
    LaneRequiresHandoffPath(&'static str),
    /// pi `:6251`. Verb-SPECIFIC prose, not
    /// [`crate::registration::authority::forbidden_message`]'s generic
    /// `Authority policy forbids action '<x>'.` — upstream writes these three discard sentences
    /// by hand and a model reading them should learn what was and was not changed.
    #[error("Authority policy forbids worktree discard.")]
    DiscardForbidden,
    /// pi `:6255`. A `Confirm` with no UI is a REFUSAL, never an implicit yes.
    #[error(
        "Authority policy requires user confirmation for worktree discard, but this session has \
         no interactive UI. Preserved worktrees were not changed."
    )]
    DiscardNoUi,
    /// The typed `merge`/`supersession` payload did not parse. pi's recorder owns ~12 rejections
    /// (`parallel-handoff.ts:206-292`); in Rust they are the evidence newtypes' own fallible
    /// constructors, reached through `serde`, so the sentence a caller reads is still
    /// [`crate::handoff::HandoffError`]'s.
    #[error("{field} is invalid: {detail}")]
    InvalidEvidence {
        /// `merge` or `supersession`.
        field: &'static str,
        /// The constructor's own sentence.
        detail: String,
    },
}

/// pi's `"Discard preserved subagent worktrees?"` (`:6256`) — the confirm TITLE.
pub(crate) const DISCARD_CONFIRM_PROMPT: &str = "Discard preserved subagent worktrees?";

/// pi `:6258`, and deliberately not an error. See [`LaneActionRefusal`].
pub(crate) const DISCARD_DECLINED: &str =
    "Worktree discard canceled; preserved worktrees were not changed.";

/// pi's confirm BODY (`:6256`) — it names the manifest, because "discard preserved worktrees" is
/// not a question anyone can answer without knowing which ones.
pub(crate) fn discard_confirm_message(handoff_path: &Path) -> String {
    format!(
        "This permanently removes preserved worktrees and temporary branches recorded in:\n{}",
        handoff_path.display()
    )
}

/// The `lane.*` argument projection — `handoffPath`, `laneId`, `merge`, `supersession`.
///
/// Lives beside [`CleanupPlanParams`] in `params.rs` for the placement reason this module's own
/// doc records: the guard scans `src/extension/` only.
#[derive(Clone, Debug, Default)]
pub(crate) struct LaneActionParams {
    /// Raw, untrimmed — the blank-vs-absent distinction is what selects pi's message.
    pub(crate) handoff_path: Option<String>,
    /// Raw; compared by plain string equality against `manifest.runId` (pi `:6283`).
    pub(crate) lane_id: Option<String>,
    /// The merge attestation, **unmodified**. No trimming, no key filtering, no round-trip
    /// through an intermediate struct: `manifestDigest` is stamped by the recorder and every
    /// other field is evidence, so a helpful normalization here would change what an attestation
    /// attests to.
    pub(crate) merge: Option<serde_json::Value>,
    /// The supersession attestation, unmodified, for the same reason.
    pub(crate) supersession: Option<serde_json::Value>,
}

/// The `worktree.cleanup`-only argument projection — `repo`, `planId`, `mode`.
///
/// `handoffPath` is NOT here: it is shared with the other four verbs and is resolved once, in the
/// guard arm, through [`LaneAction::requires_handoff_path`].
#[derive(Clone, Debug, Default)]
pub(crate) struct CleanupPlanParams {
    /// Which repository to plan for; absent means the request cwd (pi `:6225-6227`).
    pub(crate) repo: Option<String>,
    /// Reserved. Any value is refused (pi `:6220-6222`).
    pub(crate) plan_id: Option<String>,
    /// Must be `plan` (pi `:6217-6219`).
    pub(crate) mode: Option<String>,
}

/// The missing-`handoffPath` refusal for a verb that requires one.
///
/// The two sentences differ by verb (pi `:6247` vs `:6277`) and live beside
/// [`LaneAction::requires_handoff_path`] so the predicate and the message cannot drift apart.
/// Only reached when that predicate is `true`.
pub(crate) fn missing_handoff_path_refusal(verb: LaneAction) -> LaneActionRefusal {
    match verb {
        LaneAction::WorktreeDiscard => LaneActionRefusal::DiscardRequiresHandoffPath,
        other => LaneActionRefusal::LaneRequiresHandoffPath(other.as_str()),
    }
}

/// Whether this verb is one of the three that take `laneId` (pi `:6270`'s shared block).
pub(crate) fn is_lane_evidence_verb(verb: LaneAction) -> bool {
    matches!(
        verb,
        LaneAction::LaneStatus | LaneAction::LaneRecordMerge | LaneAction::LaneRecordSupersession
    )
}

/// pi's `path.isAbsolute(x) ? x : path.resolve(requestCwd, x)` (`:6226`, `:6260`, `:6278`).
///
/// Against the REQUEST cwd, never [`std::env::current_dir`]: a verb that resolved against the
/// process cwd could be pointed at a manifest outside the project. `route_action` already
/// receives the resolved request cwd, so this is the whole of it.
///
/// Returns `None` for absent AND for blank-after-trim, which is the distinction the two
/// missing-`handoffPath` sentences depend on.
pub(crate) fn resolve_against_request_cwd(raw: Option<&str>, cwd: &Path) -> Option<PathBuf> {
    let trimmed = raw.map(str::trim).filter(|value| !value.is_empty())?;
    let candidate = Path::new(trimmed);
    Some(if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    })
}

/// pi's `params.laneId?.trim()` (`:6274`) — `None` for absent and for blank.
pub(crate) fn trimmed_lane_id(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim).filter(|value| !value.is_empty())
}

/// Parse one attestation object into its typed form.
///
/// The object reaches here byte-identical to what the caller sent; the ~12 rejections pi spends
/// `normalizeMergeEvidence` on are this crate's evidence newtypes
/// ([`crate::handoff::CommitSha`], [`crate::handoff::PrNumber`],
/// [`crate::handoff::AttestationTimestamp`], …), each of which deserializes THROUGH its fallible
/// constructor, so the sentence is still upstream's.
///
/// # Errors
///
/// [`LaneActionRefusal::InvalidEvidence`] carrying that constructor's sentence.
pub(crate) fn parse_evidence<T: serde::de::DeserializeOwned>(
    field: &'static str,
    value: Option<serde_json::Value>,
) -> Result<T, LaneActionRefusal> {
    let value = value.unwrap_or(serde_json::Value::Null);
    serde_json::from_value(value).map_err(|error| LaneActionRefusal::InvalidEvidence {
        field,
        detail: strip_serde_position(&error.to_string()),
    })
}

/// `serde_json::from_value` appends `" at line N column M"` to a custom error even though a
/// `Value` has no position. Stripping it keeps the verbatim sentence verbatim.
fn strip_serde_position(message: &str) -> String {
    match message.rfind(" at line ") {
        Some(index) if message[index..].ends_with(|c: char| c.is_ascii_digit()) => {
            message[..index].to_string()
        }
        _ => message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    const ALL: [LaneAction; 5] = [
        LaneAction::WorktreeCleanup,
        LaneAction::WorktreeDiscard,
        LaneAction::LaneStatus,
        LaneAction::LaneRecordMerge,
        LaneAction::LaneRecordSupersession,
    ];

    #[test]
    fn from_wire_round_trips_as_str_for_every_verb() {
        for action in ALL {
            assert_eq!(LaneAction::from_wire(action.as_str()), Some(action));
        }
        assert_eq!(LaneAction::from_wire("lane.record-merge"), None);
        assert_eq!(LaneAction::from_wire("worktree"), None);
    }

    /// **The safety invariant this feature turns on.** `lane.status` is a pure read and stays
    /// reachable from a child-safe fanout tool; the other four do not.
    #[test]
    fn only_lane_status_is_non_mutating() {
        assert!(!LaneAction::LaneStatus.is_mutating());
        for action in ALL {
            if action != LaneAction::LaneStatus {
                assert!(action.is_mutating(), "{} must be mutating", action.as_str());
            }
        }
    }

    /// Exactly one verb consults the authority policy, and it is the one that deletes things.
    #[test]
    fn only_worktree_discard_consults_the_authority_policy() {
        assert_eq!(
            LaneAction::WorktreeDiscard.authority_action(),
            Some(AuthorityAction::DiscardWorktree)
        );
        for action in ALL {
            if action != LaneAction::WorktreeDiscard {
                assert_eq!(
                    action.authority_action(),
                    None,
                    "{} must not consult the authority policy",
                    action.as_str()
                );
            }
        }
    }

    #[test]
    fn only_worktree_cleanup_takes_handoff_path_optionally() {
        assert!(!LaneAction::WorktreeCleanup.requires_handoff_path());
        for action in ALL {
            if action != LaneAction::WorktreeCleanup {
                assert!(action.requires_handoff_path(), "{}", action.as_str());
            }
        }
    }

    /// The wire names this family hands to `AuthorityAction::for_tool_action` must be the ones
    /// that table actually keys on. `authority_action()` delegates, so a typo in `as_str()` would
    /// silently turn the gate off rather than fail to compile — this is what catches that.
    #[test]
    fn the_authority_table_keys_on_this_familys_wire_names() {
        assert_eq!(
            AuthorityAction::for_tool_action("worktree.discard"),
            Some(AuthorityAction::DiscardWorktree),
            "the table must key on the exact wire name `as_str()` produces"
        );
        for action in ALL {
            if action != LaneAction::WorktreeDiscard {
                assert_eq!(
                    AuthorityAction::for_tool_action(action.as_str()),
                    None,
                    "{} must not be in the authority table",
                    action.as_str()
                );
            }
        }
    }

    #[test]
    fn a_blank_handoff_path_resolves_to_none_not_to_the_cwd() {
        let cwd = Path::new("/repo");
        assert_eq!(resolve_against_request_cwd(Some("   "), cwd), None);
        assert_eq!(resolve_against_request_cwd(None, cwd), None);
        assert_eq!(
            resolve_against_request_cwd(Some("handoff.json"), cwd),
            Some(PathBuf::from("/repo/handoff.json"))
        );
        assert_eq!(
            resolve_against_request_cwd(Some("/elsewhere/handoff.json"), cwd),
            Some(PathBuf::from("/elsewhere/handoff.json"))
        );
    }

    /// The evidence newtypes' sentences survive the trip through serde — this is what keeps a
    /// model's rejection actionable instead of a bare "invalid type" from the deserializer.
    #[test]
    fn invalid_merge_evidence_carries_the_constructors_own_sentence() {
        let error = parse_evidence::<crate::handoff::MergeEvidence>(
            "merge",
            Some(serde_json::json!({
                "prNumber": 0,
                "reviewedHead": "a".repeat(40),
                "mergeCommit": "b".repeat(40),
                "treeEquivalent": true,
                "postMergeChecks": "recorded",
                "attestedBy": "tester",
                "attestedAt": "2026-09-18T00:00:00Z"
            })),
        )
        .expect_err("prNumber 0 is refused");
        let text = error.to_string();
        assert!(
            text.contains("merge.prNumber must be a positive integer."),
            "the refusal must carry pi's own sentence, got: {text}"
        );
        assert!(
            !text.contains(" at line "),
            "serde's synthetic position must not leak into a verbatim sentence: {text}"
        );
    }
}
