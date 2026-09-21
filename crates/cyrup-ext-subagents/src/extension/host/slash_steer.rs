//! `/subagents-steer` — steer a live async run, or one of its children (pi
//! `slash-commands.ts:1057-1119`).
//!
//! **Signature frozen by the orchestrator; the body is this batch's work.** The capability is
//! already live and dispatched: the `steer` action is advertised at `extension/tool/text.rs:307`
//! and routed at `extension/tool/routing.rs:2543`, behind the `allowSteer` authority gate. This
//! command is a slash REGISTRATION over that, not a new capability.
//!
//! # The call chain, end to end
//!
//! `dispatch_slash(SlashCommandName::SubagentsSteer, …)` → [`SubagentsExtension::slash_subagents_steer`]
//! → [`parse_steer_args`] → (optional `--child` resolution over the run's own
//! [`crate::background::RunStatus`]) →
//! [`crate::extension::executor::SubagentExecutor::control_steer`] — the SAME entry point
//! `routing.rs`'s `"steer" => { … }` arm calls, so the tool surface and the slash surface cannot
//! diverge (R-SA-130). No second authority gate is added here: refusals, the session gate and the
//! run-state guard are all `control_steer`'s, exactly as they are for the tool.
//!
//! # Upstream flag drift, verified at v0.68.0
//!
//! * There is NO selector branch. Upstream's `/subagents-stop` opens `ctx.ui.custom(…)` on its
//!   no-id branch (`:1044-1047`); `/subagents-steer` does not — a missing (or `--`-leading) run id
//!   is `sendSlashText(pi, usage)` and nothing else (`:1065-1068`). `has_ui` is therefore threaded
//!   in and deliberately unused; steering an unnamed run is a guess, and a guess speaks into a
//!   live child's prompt.
//! * `steeringRecovery: false` (`:1116`) has no cyrup counterpart to set. Upstream's
//!   steering-RECOVERY subsystem (`async-steering-action.ts:88-250`) is not ported at all —
//!   [`crate::extension::executor::foreground_actions::steer`]'s own module doc says so — so the
//!   guarantee that flag buys (never swap the addressed child for a pause-and-revive replacement)
//!   holds here by construction rather than by a parameter.

use std::path::Path;

use crate::background::child_identity::{
    AsyncStatusChildResolution, async_status_child_identity_candidates, resolve_by_candidates,
};
use crate::background::{RunState, StepStatus, run_status};
use crate::error::SubagentError;
use crate::extension::executor::paths::{default_async_root_in, default_results_dir_in};
use crate::extension::host::SubagentsExtension;

/// The outcome of [`parse_steer_args`] — upstream's two exits from `:1062-1080`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SteerArgs {
    /// Both of upstream's `sendSlashText(pi, usage)` returns: no run id / a `--`-leading run id
    /// (`:1066-1068`), and an empty message after the selector (`:1077-1080`).
    Usage,
    /// A run id, an optional `--child` selector, and the free-form remainder as the message.
    Parsed {
        /// The first token — upstream's `const [id, ...rest] = tokens`.
        id: String,
        /// Upstream's `childId`, set ONLY by a `--child <value>` pair in first position.
        child_id: Option<String>,
        /// `rest.slice(messageStart).join(" ").trim()`.
        message: String,
    },
}

/// pi `slash-commands.ts:1062-1080`, ported line for line.
///
/// Two details are deliberate upstream decisions rather than oversights, and both are load-bearing:
///
/// 1. **`--child` is the ONLY flag.** It is consumed only when it is the first token after the run
///    id AND its value does not itself start with `--` (`:1073`). A `--child` anywhere else is
///    message text.
/// 2. **Everything after the run id (or after the consumed pair) is free-form message text,
///    INCLUDING a leading `--token`.** Upstream says so in its own comment at `:1070-1072`: *"Only
///    the optional child selector is parsed as a flag. Everything after the run id (or selector) is
///    free-form message text, including a leading token such as `--verbose`."* A message is prose
///    aimed at a running model, so refusing it for looking like a flag would make whole sentences
///    unsendable.
///
/// The `id.startsWith("--")` guard (`:1066`) is what keeps that permissiveness honest: a caller who
/// forgot the run id gets the usage line instead of having their first flag silently addressed as
/// a run.
pub(crate) fn parse_steer_args(args: &str) -> SteerArgs {
    // pi `args.trim().split(/\s+/).filter(Boolean)`.
    let tokens: Vec<&str> = args.split_whitespace().filter(|t| !t.is_empty()).collect();
    // pi `const [id, ...rest] = tokens; if (!id || id.startsWith("--"))`.
    let Some((id, rest)) = tokens.split_first() else {
        return SteerArgs::Usage;
    };
    if id.starts_with("--") {
        return SteerArgs::Usage;
    }

    // pi `:1073-1076` — first position, non-`--` value, both required.
    let mut child_id: Option<String> = None;
    let mut message_start = 0usize;
    if rest.first().copied() == Some("--child")
        && let Some(value) = rest.get(1)
        && !value.starts_with("--")
    {
        child_id = Some((*value).to_string());
        message_start = 2;
    }

    // pi `rest.slice(messageStart).join(" ").trim()`.
    let message = rest
        .get(message_start..)
        .unwrap_or(&[])
        .join(" ")
        .trim()
        .to_string();
    if message.is_empty() {
        return SteerArgs::Usage;
    }
    SteerArgs::Parsed {
        id: (*id).to_string(),
        child_id,
        message,
    }
}

/// [`async_status_child_identity_candidates`] plus this step's nested run ids — pi's
/// `resolveAsyncStatusChild(…, { includeNested: true })` (`child-identity.ts:34-42`), which the
/// SLASH path is upstream's only caller of (`slash-commands.ts:1099`).
///
/// \[CYRUP-DELTA] Two halves of upstream's nested walk are unrepresentable and are recorded at
/// [`crate::background::child_identity`]'s own module doc as well:
///
/// * cyrup's per-step nested tracking is [`StepStatus::nested_run_ids`] — bare
///   [`crate::background::RunId`]s, not pi's `NestedRunSummary { id, children, … }` — so the walk
///   is ONE level deep, not `findNested`'s recursion into `nested.children`.
/// * upstream pushes a SECOND match when a step matches both by its own identity and by a nested
///   id, which makes that step ambiguous; folding both into one candidate list here yields one
///   match instead. Unreachable in practice (a nested run id is a fresh token and cannot equal its
///   own parent step's identity), and recorded rather than left to inference.
fn candidates_including_nested(step: &StepStatus, index: usize) -> Vec<String> {
    let mut candidates = async_status_child_identity_candidates(step, index);
    for nested in &step.nested_run_ids {
        let token = nested.as_str();
        if !token.is_empty() && !candidates.iter().any(|seen| seen == token) {
            candidates.push(token.to_string());
        }
    }
    candidates
}

impl SubagentsExtension {
    /// Upstream's usage line, verbatim (`slash-commands.ts:1061`) — rendered on a parse failure.
    pub(crate) const STEER_USAGE: &'static str =
        "Usage: /subagents-steer <run-id> [--child <child-id>] <message>";

    /// pi `slash-commands.ts:1059-1118`. Only `--child` is parsed as a flag; everything after the
    /// run id (or the selector) is free-form message text, INCLUDING a leading `--token`
    /// (`:1065-1067`).
    ///
    /// With no `--child`, the run id is handed straight to
    /// [`SubagentExecutor::control_steer`](crate::extension::executor::SubagentExecutor::control_steer)
    /// (`:1112-1117`). With one, upstream first resolves the child against the run's OWN status
    /// (`:1082-1110`) so it can decide between addressing a nested run by its id and addressing a
    /// step by its index — the two shapes `control_steer` distinguishes as `target` vs `index`.
    ///
    /// `has_ui` is unused: upstream opens no selector on this command (see the module doc).
    pub(crate) async fn slash_subagents_steer(
        &self,
        args: &str,
        cwd: &Path,
        has_ui: bool,
    ) -> Result<String, SubagentError> {
        // pi `:1057-1119` registers no `ctx.ui.custom` branch for this command — see the module
        // doc's flag-drift note. Threaded by the frozen signature; deliberately not consulted.
        let _ = has_ui;

        let (id, child_id, message) = match parse_steer_args(args) {
            SteerArgs::Usage => return Ok(Self::STEER_USAGE.to_string()),
            SteerArgs::Parsed {
                id,
                child_id,
                message,
            } => (id, child_id, message),
        };

        // pi `let targetId = id; let childIndex: number | undefined;` (`:1082-1083`).
        let mut target_id = id.clone();
        let mut child_index: Option<usize> = None;

        if let Some(child_id) = child_id.as_deref() {
            // pi `:1085-1092` — the run must be a CURRENT-SESSION run in one of the three live
            // states before a child selector means anything. Upstream expresses that as
            // `listAsyncRuns(DIRS.async, { runId, states: ["queued","running","paused"], sessionId })[0]`
            // plus `readStatus(run.asyncDir)`; cyrup's equivalent single read is
            // `reconcile_by_id`, which performs the SAME R-SA-079 reconciliation the tool path's
            // `control_steer` will perform a moment later, followed by the two filters.
            let roots = self.executor.config_snapshot().await.roots;
            let async_root = default_async_root_in(&roots, cwd);
            let results_dir = default_results_dir_in(&roots, cwd);
            let resolved = run_status::reconcile_by_id(&async_root, &results_dir, &id).await?;

            // pi `if (!run || !status)` (`:1093-1096`), its sentence verbatim.
            let Some((status, _paths)) = resolved else {
                return Ok(format!("No current-session async run found for '{id}'."));
            };
            // The `states` filter and the `sessionId` filter of upstream's `listAsyncRuns` call,
            // which collapse into the same "not found" sentence because upstream reads `[0]` of a
            // filtered list. The session gate is `control_steer`'s own
            // (`foreground_actions/steer.rs`, pi `async-steering-action.ts:48`, PERMISSIVE), so
            // this pre-check can never refuse a run the action itself would have accepted.
            let live = matches!(
                status.state,
                RunState::Queued | RunState::Running | RunState::Paused
            );
            let admitted = crate::background::delivery::SessionGate::Permissive.admits(
                crate::identity::SessionId::parse_opt(
                    self.executor.current_session_id().as_deref(),
                )
                .as_ref(),
                status.session_id.as_ref(),
            );
            if !live || !admitted {
                return Ok(format!("No current-session async run found for '{id}'."));
            }

            // pi `:1097-1103` — `resolveAsyncStatusChild(resolutionStatus, childId,
            // { includeNested: true })`. Upstream's `resolutionStatus` splice
            // (`:1098-1101`, re-attaching each step's `children` from the listing) has nothing to
            // do here: cyrup's `nested_run_ids` already live ON the reconciled status, so the
            // status read above IS upstream's merged view.
            let child = match resolve_by_candidates(&status, child_id, candidates_including_nested)
            {
                AsyncStatusChildResolution::Resolved(child) => child,
                // pi `if (!resolution.ok) sendSlashText(pi, resolution.message)` (`:1104-1107`) —
                // both codes render their own sentence, which is already upstream's verbatim.
                failure => {
                    return Ok(failure.failure_message().unwrap_or_default().to_string());
                }
            };

            // pi `:1108-1110`, in upstream's order:
            //   nested → address the nested RUN by its own id;
            //   step.runId → address that child run by id;
            //   otherwise → address the step POSITIONALLY.
            let step = status.steps.get(child.index);
            let nested_match = step.is_some_and(|step| {
                step.nested_run_ids
                    .iter()
                    .any(|nested| nested.as_str() == child_id)
            });
            if nested_match {
                target_id = child_id.to_string();
            } else if let Some(run_id) = step.and_then(|step| step.run_id.as_ref()) {
                target_id = run_id.as_str().to_string();
            } else {
                child_index = Some(child.index);
            }
        }

        // pi `:1112-1117` — `runCommand(ctx, { action: "steer", id, message, index?, … })`. The
        // SAME executor entry point `extension/tool/routing.rs`'s `"steer"` arm calls, with
        // upstream's argument set: no `dir`, no `task`, no `mode`.
        self.executor
            .control_steer(
                cwd,
                Some(target_id.as_str()),
                None,
                Some(message.as_str()),
                None,
                child_index,
                None,
            )
            .await
            .map_err(SubagentError::Management)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// MUTATION: dropping the `!id` / `id.startsWith("--")` guard (`slash-commands.ts:1066`) —
    /// a bare command, or one whose first token is a flag, must render the usage line rather than
    /// addressing a run named `--child`.
    #[test]
    fn a_missing_or_flag_leading_run_id_is_the_usage_line() {
        for args in ["", "   ", "\t\n", "--child c1 go", "--verbose steer this"] {
            assert_eq!(
                parse_steer_args(args),
                SteerArgs::Usage,
                "{args:?} must not parse as a steer request"
            );
        }
    }

    /// MUTATION: dropping the empty-message guard (`:1077-1080`) — a run id with nothing after it
    /// (or with only a consumed `--child` pair) is a usage error, not a steer with an empty
    /// message.
    #[test]
    fn a_run_id_with_no_message_is_the_usage_line() {
        assert_eq!(parse_steer_args("run0001"), SteerArgs::Usage);
        assert_eq!(parse_steer_args("run0001 --child c1"), SteerArgs::Usage);
    }

    /// MUTATION: consuming `--child` anywhere but first position, or accepting a `--`-leading
    /// value (`:1073`). Both must leave the tokens as MESSAGE text.
    #[test]
    fn child_is_consumed_only_in_first_position_with_a_non_flag_value() {
        assert_eq!(
            parse_steer_args("run0001 --child c1 tighten the diff"),
            SteerArgs::Parsed {
                id: "run0001".to_string(),
                child_id: Some("c1".to_string()),
                message: "tighten the diff".to_string(),
            },
            "first position with a plain value: consumed"
        );
        assert_eq!(
            parse_steer_args("run0001 --child --c1 tighten the diff"),
            SteerArgs::Parsed {
                id: "run0001".to_string(),
                child_id: None,
                message: "--child --c1 tighten the diff".to_string(),
            },
            "a `--`-leading value is NOT a child selector, and the pair stays in the message"
        );
        // A DANGLING `--child` IS NOT A USAGE ERROR. Upstream's guard is
        // `rest[0] === "--child" && rest[1] && !rest[1].startsWith("--")` (`:1072`): with
        // `rest = ["--child"]`, `rest[1]` is `undefined`, so the selector is NOT consumed,
        // `messageStart` stays 0, and `message` is the non-empty string `"--child"` — which sails
        // past the empty-message guard on `:1077`. So the token becomes MESSAGE TEXT, exactly as
        // it does past first position. This assertion originally demanded `Usage`; the production
        // parser disagreed with it and the production parser is the one that matches upstream.
        assert_eq!(
            parse_steer_args("run0001 --child"),
            SteerArgs::Parsed {
                id: "run0001".to_string(),
                child_id: None,
                message: "--child".to_string(),
            },
            "a dangling `--child` is message text, not a usage error (pi `:1072`, `:1077`)"
        );
        assert_eq!(
            parse_steer_args("run0001 please --child c1 now"),
            SteerArgs::Parsed {
                id: "run0001".to_string(),
                child_id: None,
                message: "please --child c1 now".to_string(),
            },
            "`--child` past first position is message text"
        );
    }

    /// MUTATION: adding a general flag parser, or rejecting a `--`-leading message. Upstream's own
    /// comment (`:1070-1072`) names `--verbose` as the example that must survive VERBATIM as the
    /// first word of the message.
    #[test]
    fn a_message_beginning_with_a_flag_token_is_preserved_verbatim() {
        assert_eq!(
            parse_steer_args("run0001 --verbose and keep going"),
            SteerArgs::Parsed {
                id: "run0001".to_string(),
                child_id: None,
                message: "--verbose and keep going".to_string(),
            }
        );
        assert_eq!(
            parse_steer_args("run0001 --child c1 --verbose and keep going"),
            SteerArgs::Parsed {
                id: "run0001".to_string(),
                child_id: Some("c1".to_string()),
                message: "--verbose and keep going".to_string(),
            },
            "the selector is consumed and the flag-looking message still survives"
        );
    }

    /// MUTATION: collapsing runs of whitespace differently from `split(/\s+/)`, or forgetting the
    /// final `.trim()` (`:1076`).
    #[test]
    fn interior_whitespace_collapses_exactly_as_upstream_joins_it() {
        assert_eq!(
            parse_steer_args("  run0001   do   the\tthing  "),
            SteerArgs::Parsed {
                id: "run0001".to_string(),
                child_id: None,
                message: "do the thing".to_string(),
            }
        );
    }

    /// MUTATION: dropping the nested rung from [`candidates_including_nested`] — pi's
    /// `includeNested: true` (`slash-commands.ts:1099`) is what lets a caller name a nested
    /// descendant by its own run id. Without it the slash path can only reach top-level steps.
    #[test]
    fn nested_run_ids_are_candidates_and_the_ordinary_rungs_survive() {
        use crate::background::RunId;

        let mut step = StepStatus::pending("scout".to_string());
        step.run_id = Some(RunId::from_token("childrun00001"));
        step.nested_run_ids = vec![RunId::from_token("nestedrun0001")];

        let candidates = candidates_including_nested(&step, 2);
        assert!(
            candidates.iter().any(|c| c == "childrun00001"),
            "the step's own run id stays a candidate: {candidates:?}"
        );
        assert!(
            candidates.iter().any(|c| c == "step:2"),
            "the positional rung stays a candidate: {candidates:?}"
        );
        assert!(
            candidates.iter().any(|c| c == "nestedrun0001"),
            "the nested run id is a candidate (`includeNested`): {candidates:?}"
        );
        assert_eq!(
            candidates_including_nested(&StepStatus::pending("scout".to_string()), 0),
            vec!["step:0".to_string()],
            "a step with neither rung is still exactly upstream's positional identity"
        );
    }

    /// MUTATION: rewording the usage constant, or returning it as an `Err` (which the host would
    /// prefix with `subagent command failed:`). Upstream renders it through `sendSlashText`, i.e.
    /// as ordinary command output, and the same sentence is the command's registered `usage`.
    #[test]
    fn the_usage_constant_is_upstreams_and_matches_the_registered_descriptor() {
        assert_eq!(
            SubagentsExtension::STEER_USAGE,
            "Usage: /subagents-steer <run-id> [--child <child-id>] <message>"
        );
        let descriptor = crate::registration::slash_commands::SLASH_COMMANDS
            .iter()
            .find(|c| {
                c.name == crate::registration::slash_commands::SlashCommandName::SubagentsSteer
            })
            .expect("/subagents-steer is registered");
        assert_eq!(
            descriptor.usage,
            SubagentsExtension::STEER_USAGE,
            "the rendered usage and the advertised usage are one sentence"
        );
    }

    /// MUTATION: routing the no-id branch to a selector or to a "most recent run" guess. Upstream
    /// registers NO `ctx.ui.custom` branch on this command (`:1057-1119`), and a guess here speaks
    /// into a live child's prompt — so the empty-argument call must render the usage line whether
    /// or not a UI is attached.
    #[tokio::test]
    async fn an_empty_argument_renders_usage_with_and_without_a_ui() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            crate::registration::SubagentExtensionConfig::default(),
            dir.path().to_path_buf(),
        );
        for has_ui in [true, false] {
            assert_eq!(
                ext.slash_subagents_steer("", dir.path(), has_ui)
                    .await
                    .expect("the usage line is output, not an error"),
                SubagentsExtension::STEER_USAGE,
                "has_ui={has_ui}"
            );
        }
    }

    /// MUTATION: implementing a second steer path instead of calling
    /// [`crate::extension::executor::SubagentExecutor::control_steer`]. A run id that resolves to
    /// nothing must come back with THAT function's own refusal — the identical sentence the
    /// `subagent({ action: "steer" })` tool call produces — rather than one invented here.
    #[tokio::test]
    async fn an_unknown_run_id_returns_the_live_actions_own_refusal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            crate::registration::SubagentExtensionConfig::default(),
            dir.path().to_path_buf(),
        );
        let slash = ext
            .slash_subagents_steer("nosuchrun0001 tighten the diff", dir.path(), false)
            .await
            .expect_err("an unresolvable id is a refusal");
        let action = ext
            .executor()
            .control_steer(
                dir.path(),
                Some("nosuchrun0001"),
                None,
                Some("tighten the diff"),
                None,
                None,
                None,
            )
            .await
            .expect_err("the tool surface refuses identically");
        assert_eq!(
            slash.to_string(),
            SubagentError::Management(action).to_string(),
            "the slash surface and the tool surface must be one implementation"
        );
    }
}
