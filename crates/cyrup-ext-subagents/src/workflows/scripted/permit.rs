//! `WorkflowChildPermit` — the child half of pi `shared/workflow-child-permit.ts` (SCOPE_3d ported
//! the resource half into [`crate::workflows::permit`]; its `:7` doc records this split). The
//! permit admits exactly ONE distinct child launch for a `workflow` resource's first slice.
//!
//! # Why this is a runtime state enum and NOT typestate — decided, with evidence
//!
//! `claimWorkflowChildPermit` (`workflow-child-permit.ts:89-99`) reads:
//!
//! ```ts
//! record.state = record.childKey === childKey ? "claimed" : "consumed";   // :96
//! if (record.childKey !== childKey) return "Workflow child permit child key mismatch.";
//! ```
//!
//! **A mismatched claim burns the permit to `consumed` and *then* reports the error** — the
//! failure path performs a state transition. A typestate encoding
//! (`Permit<Available> -> Result<Permit<Claimed>, E>`) consumes the value and returns `Err`,
//! destroying the burnt permit, so a later `consume` could never observe *"is already consumed"* —
//! deleting exactly the anti-replay property the permit exists for. Additionally, `Clone` on a
//! one-use capability is SCOPE_3 §A.5's hazard, and a permit is stored and resumed across a run;
//! both point the same way. Shape: private fields, one fallible constructor, a runtime
//! [`PermitState`], `Debug` only — no `Clone`, no `Deserialize`, no `Serialize`.
//!
//! Precedence inside `claim`/`consume` is load-bearing (root-mismatch does NOT burn; child-key /
//! agent / runner / digest mismatches DO), so each is one pure decision function returning one
//! enum, applied by a two-line shell (SCOPE_3 §A.1's last row).

use crate::workflows::stable_json_digest;

/// The requested child context — pi's `"fresh" | "fork"` literal union on
/// `WorkflowChildPermitInput.context` (`workflow-child-permit.ts:11`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkflowChildContext {
    /// A fresh child session.
    Fresh,
    /// A fork of the parent session.
    Fork,
}

impl WorkflowChildContext {
    /// The projection-digest word (`workflow-child-permit.ts:47`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Fork => "fork",
        }
    }
}

/// Constructor input — pi `WorkflowChildPermitInput` (`workflow-child-permit.ts:4-11`).
#[derive(Debug)]
pub struct WorkflowChildPermitInput<'a> {
    /// The issuing extension package.
    pub issuer_package: &'a str,
    /// The workflow root run this permit belongs to.
    pub workflow_run_id: &'a str,
    /// The one child key the permit admits.
    pub child_key: &'a str,
    /// The agent the child must launch as.
    pub agent: &'a str,
    /// The launch-contract digest pinned at issue time.
    pub launch_contract_digest: &'a str,
    /// The requested child context.
    pub context: WorkflowChildContext,
}

/// The final launch projection checked at consume time — pi `WorkflowChildPermitLaunch`
/// (`workflow-child-permit.ts:13-20`).
///
/// `runner` stays a `String` rather than a unit enum: upstream's check `launch.runner !== "pi"`
/// (`:113`) is a REAL rejection with its own verbatim message, and the projection is assembled at
/// a boundary where the value is data; a unit enum would make the
/// [`WorkflowChildPermitError::NativeChildrenOnly`] arm unrepresentable rather than checked.
#[derive(Debug)]
pub struct WorkflowChildPermitLaunch<'a> {
    /// The workflow root run id of the launch.
    pub workflow_run_id: &'a str,
    /// The launched child key.
    pub child_key: &'a str,
    /// The launched agent.
    pub agent: &'a str,
    /// The launch-contract digest of the final launch.
    pub launch_contract_digest: &'a str,
    /// The launched child context.
    pub context: WorkflowChildContext,
    /// The runner — must be `"pi"`.
    pub runner: &'a str,
}

/// The permit lifecycle word — pi `WorkflowChildPermitRecord["state"]`
/// (`workflow-child-permit.ts:38`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PermitState {
    Available,
    Claimed,
    Consumed,
}

/// One variant per verbatim upstream message (`workflow-child-permit.ts:82-116`) — SCOPE_3 §A.1
/// row 3: never `Err(String)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WorkflowChildPermitError {
    /// `Workflow child permit is invalid.` — upstream's WeakMap-lookup failure. Unreachable
    /// through this type's private constructor (there is no way to hold a `WorkflowChildPermit`
    /// that was not built by [`WorkflowChildPermit::create`]), declared so the error vocabulary
    /// is complete and callers that transport the message never invent a variant.
    #[error("Workflow child permit is invalid.")]
    Invalid,
    /// `Workflow child permit is already consumed.`
    #[error("Workflow child permit is already consumed.")]
    AlreadyConsumed,
    /// `Workflow child permit does not match this workflow root.`
    #[error("Workflow child permit does not match this workflow root.")]
    RootMismatch,
    /// `Workflow child permit child key mismatch.`
    #[error("Workflow child permit child key mismatch.")]
    ChildKeyMismatch,
    /// `Workflow child permit launch was not claimed.`
    #[error("Workflow child permit launch was not claimed.")]
    LaunchNotClaimed,
    /// `Workflow child permit agent mismatch.`
    #[error("Workflow child permit agent mismatch.")]
    AgentMismatch,
    /// `Workflow child permit supports native Pi children only.`
    #[error("Workflow child permit supports native Pi children only.")]
    NativeChildrenOnly,
    /// `Workflow child permit does not match the final launch projection.`
    #[error("Workflow child permit does not match the final launch projection.")]
    ProjectionMismatch,
}

/// Constructor rejection — pi `required` (`workflow-child-permit.ts:53-56`), message verbatim:
/// `` `{label} must be a non-empty trimmed string.` ``.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{label} must be a non-empty trimmed string.")]
pub struct WorkflowChildPermitInputError {
    /// The offending input field's upstream label.
    pub label: &'static str,
}

/// pi `projectionDigest` (`workflow-child-permit.ts:42-51`): `stableJsonDigest` over the
/// `version: 1` projection. Key order is irrelevant — [`stable_json_digest`] sorts.
fn projection_digest(
    child_key: &str,
    agent: &str,
    launch_contract_digest: &str,
    context: WorkflowChildContext,
    runner: &str,
) -> String {
    stable_json_digest(&serde_json::json!({
        "version": 1,
        "childKey": child_key,
        "agent": agent,
        "launchContractDigest": launch_contract_digest,
        "context": context.as_str(),
        "runner": runner,
    }))
}

/// The pure claim decision (`workflow-child-permit.ts:89-99`): what the next state is and what the
/// caller is told, in one place, so the burn-on-mismatch order cannot drift from the reply.
fn claim_decision(
    state: PermitState,
    record_run: &str,
    record_child: &str,
    workflow_run_id: &str,
    child_key: &str,
) -> (PermitState, Result<(), WorkflowChildPermitError>) {
    if state != PermitState::Available {
        return (state, Err(WorkflowChildPermitError::AlreadyConsumed));
    }
    if record_run != workflow_run_id {
        // Root mismatch does NOT burn (`:95` precedes the `:96` transition).
        return (state, Err(WorkflowChildPermitError::RootMismatch));
    }
    if record_child == child_key {
        (PermitState::Claimed, Ok(()))
    } else {
        // The burn: a mismatched claim consumes the permit AND reports the mismatch (`:96-97`).
        (
            PermitState::Consumed,
            Err(WorkflowChildPermitError::ChildKeyMismatch),
        )
    }
}

/// The pure consume decision (`workflow-child-permit.ts:103-116`). Root mismatch is checked
/// BEFORE the transition (does not burn); child-key/agent/runner/digest mismatches are checked
/// AFTER (all burn).
#[allow(clippy::too_many_arguments)]
fn consume_decision(
    state: PermitState,
    record_run: &str,
    record_child: &str,
    record_agent: &str,
    expected_digest: &str,
    launch: &WorkflowChildPermitLaunch<'_>,
) -> (PermitState, Result<(), WorkflowChildPermitError>) {
    match state {
        PermitState::Available => {
            return (state, Err(WorkflowChildPermitError::LaunchNotClaimed));
        }
        PermitState::Consumed => {
            return (state, Err(WorkflowChildPermitError::AlreadyConsumed));
        }
        PermitState::Claimed => {}
    }
    if record_run != launch.workflow_run_id {
        return (state, Err(WorkflowChildPermitError::RootMismatch));
    }
    // `record.state = "consumed"` happens HERE (`:108`) — every later mismatch burns.
    let burnt = PermitState::Consumed;
    if record_child != launch.child_key {
        return (burnt, Err(WorkflowChildPermitError::ChildKeyMismatch));
    }
    if record_agent != launch.agent {
        return (burnt, Err(WorkflowChildPermitError::AgentMismatch));
    }
    if launch.runner != "pi" {
        return (burnt, Err(WorkflowChildPermitError::NativeChildrenOnly));
    }
    let launch_digest = projection_digest(
        launch.child_key,
        launch.agent,
        launch.launch_contract_digest,
        launch.context,
        launch.runner,
    );
    if expected_digest != launch_digest {
        return (burnt, Err(WorkflowChildPermitError::ProjectionMismatch));
    }
    (burnt, Ok(()))
}

/// The package-internal first-slice child permit — opaque, in-memory, and not serializable
/// (`workflow-child-permit.ts:59`). Private fields, one fallible constructor, no `Clone`, no
/// `Serialize`/`Deserialize` — the [`crate::workflows::WorkflowResourcePermit`] idiom, for the
/// module-doc reasons.
#[derive(Debug)]
pub struct WorkflowChildPermit {
    issuer_package: String,
    workflow_run_id: String,
    child_key: String,
    agent: String,
    expected_projection_digest: String,
    state: PermitState,
}

impl WorkflowChildPermit {
    /// pi `createWorkflowChildPermit` (`workflow-child-permit.ts:59-77`) — the ONLY constructor.
    /// `expectedProjectionDigest` is computed here, over the pinned `runner: "pi"` projection, and
    /// re-checked at [`Self::consume`].
    ///
    /// # Errors
    ///
    /// [`WorkflowChildPermitInputError`] with the verbatim `required` message when any input is
    /// empty or untrimmed.
    pub fn create(
        input: &WorkflowChildPermitInput<'_>,
    ) -> Result<Self, WorkflowChildPermitInputError> {
        fn required<'a>(
            value: &'a str,
            label: &'static str,
        ) -> Result<&'a str, WorkflowChildPermitInputError> {
            if value.trim().is_empty() || value != value.trim() {
                Err(WorkflowChildPermitInputError { label })
            } else {
                Ok(value)
            }
        }
        let issuer_package = required(input.issuer_package, "issuerPackage")?;
        let workflow_run_id = required(input.workflow_run_id, "workflowRunId")?;
        let child_key = required(input.child_key, "childKey")?;
        let agent = required(input.agent, "agent")?;
        let launch_contract_digest =
            required(input.launch_contract_digest, "launchContractDigest")?;
        Ok(Self {
            issuer_package: issuer_package.to_string(),
            workflow_run_id: workflow_run_id.to_string(),
            child_key: child_key.to_string(),
            agent: agent.to_string(),
            expected_projection_digest: projection_digest(
                child_key,
                agent,
                launch_contract_digest,
                input.context,
                "pi",
            ),
            state: PermitState::Available,
        })
    }

    /// The issuing package (recorded for auditability; upstream keeps it on the record).
    #[must_use]
    pub fn issuer_package(&self) -> &str {
        &self.issuer_package
    }

    /// pi `validateWorkflowChildPermitRoot` (`workflow-child-permit.ts:79-85`). Read-only: never
    /// transitions state.
    ///
    /// # Errors
    ///
    /// [`WorkflowChildPermitError::AlreadyConsumed`] / [`WorkflowChildPermitError::RootMismatch`].
    pub fn validate_root(&self, workflow_run_id: &str) -> Result<(), WorkflowChildPermitError> {
        if self.state != PermitState::Available {
            return Err(WorkflowChildPermitError::AlreadyConsumed);
        }
        if self.workflow_run_id != workflow_run_id {
            return Err(WorkflowChildPermitError::RootMismatch);
        }
        Ok(())
    }

    /// pi `claimWorkflowChildPermit` (`workflow-child-permit.ts:89-99`) — claim the first distinct
    /// launch attempt before validating its model-authored shape. **A mismatched child key burns
    /// the permit** (see the module doc).
    ///
    /// # Errors
    ///
    /// The claim-path subset of [`WorkflowChildPermitError`].
    pub fn claim(
        &mut self,
        workflow_run_id: &str,
        child_key: &str,
    ) -> Result<(), WorkflowChildPermitError> {
        let (next, outcome) = claim_decision(
            self.state,
            &self.workflow_run_id,
            &self.child_key,
            workflow_run_id,
            child_key,
        );
        self.state = next;
        outcome
    }

    /// pi `consumeWorkflowChildPermit` (`workflow-child-permit.ts:103-116`) — verify and
    /// permanently consume the permit before the one native process spawn.
    ///
    /// # Errors
    ///
    /// The consume-path subset of [`WorkflowChildPermitError`].
    pub fn consume(
        &mut self,
        launch: &WorkflowChildPermitLaunch<'_>,
    ) -> Result<(), WorkflowChildPermitError> {
        let (next, outcome) = consume_decision(
            self.state,
            &self.workflow_run_id,
            &self.child_key,
            &self.agent,
            &self.expected_projection_digest,
            launch,
        );
        self.state = next;
        outcome
    }

    /// pi `workflowChildPermitConsumed` (`workflow-child-permit.ts:118-121`): `claimed` counts as
    /// consumed for the "was it used" question.
    #[must_use]
    pub fn consumed(&self) -> bool {
        matches!(self.state, PermitState::Claimed | PermitState::Consumed)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn permit() -> WorkflowChildPermit {
        WorkflowChildPermit::create(&WorkflowChildPermitInput {
            issuer_package: "pkg",
            workflow_run_id: "root-1",
            child_key: "child",
            agent: "worker",
            launch_contract_digest: "digest-1",
            context: WorkflowChildContext::Fresh,
        })
        .unwrap()
    }

    fn launch<'a>() -> WorkflowChildPermitLaunch<'a> {
        WorkflowChildPermitLaunch {
            workflow_run_id: "root-1",
            child_key: "child",
            agent: "worker",
            launch_contract_digest: "digest-1",
            context: WorkflowChildContext::Fresh,
            runner: "pi",
        }
    }

    #[test]
    fn constructor_requires_trimmed_non_empty_inputs() {
        let err = WorkflowChildPermit::create(&WorkflowChildPermitInput {
            issuer_package: " pkg",
            workflow_run_id: "r",
            child_key: "c",
            agent: "a",
            launch_contract_digest: "d",
            context: WorkflowChildContext::Fork,
        })
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "issuerPackage must be a non-empty trimmed string."
        );
    }

    #[test]
    fn happy_path_claim_then_consume() {
        let mut p = permit();
        assert!(p.validate_root("root-1").is_ok());
        assert!(!p.consumed());
        p.claim("root-1", "child").unwrap();
        assert!(p.consumed(), "claimed counts as consumed");
        p.consume(&launch()).unwrap();
        assert!(p.consumed());
        assert_eq!(
            p.consume(&launch()).unwrap_err(),
            WorkflowChildPermitError::AlreadyConsumed
        );
    }

    #[test]
    fn mismatched_claim_burns_the_permit() {
        let mut p = permit();
        assert_eq!(
            p.claim("root-1", "other").unwrap_err(),
            WorkflowChildPermitError::ChildKeyMismatch
        );
        // The anti-replay property: the burnt permit still exists and reports consumed.
        assert_eq!(
            p.claim("root-1", "child").unwrap_err(),
            WorkflowChildPermitError::AlreadyConsumed
        );
        assert_eq!(
            p.validate_root("root-1").unwrap_err(),
            WorkflowChildPermitError::AlreadyConsumed
        );
        assert!(p.consumed());
    }

    #[test]
    fn root_mismatch_does_not_burn() {
        let mut p = permit();
        assert_eq!(
            p.claim("other-root", "child").unwrap_err(),
            WorkflowChildPermitError::RootMismatch
        );
        // Still available: the correct root can proceed.
        p.claim("root-1", "child").unwrap();
    }

    #[test]
    fn consume_before_claim_is_not_claimed() {
        let mut p = permit();
        assert_eq!(
            p.consume(&launch()).unwrap_err(),
            WorkflowChildPermitError::LaunchNotClaimed
        );
    }

    #[test]
    fn consume_agent_runner_and_digest_mismatches_burn() {
        for (mutate, expected) in [
            (
                Box::new(|l: &mut WorkflowChildPermitLaunch<'_>| l.agent = "other")
                    as Box<dyn Fn(&mut WorkflowChildPermitLaunch<'_>)>,
                WorkflowChildPermitError::AgentMismatch,
            ),
            (
                Box::new(|l: &mut WorkflowChildPermitLaunch<'_>| l.runner = "external"),
                WorkflowChildPermitError::NativeChildrenOnly,
            ),
            (
                Box::new(|l: &mut WorkflowChildPermitLaunch<'_>| {
                    l.launch_contract_digest = "digest-2";
                }),
                WorkflowChildPermitError::ProjectionMismatch,
            ),
            (
                Box::new(|l: &mut WorkflowChildPermitLaunch<'_>| {
                    l.context = WorkflowChildContext::Fork;
                }),
                WorkflowChildPermitError::ProjectionMismatch,
            ),
        ] {
            let mut p = permit();
            p.claim("root-1", "child").unwrap();
            let mut bad = launch();
            mutate(&mut bad);
            assert_eq!(p.consume(&bad).unwrap_err(), expected);
            assert_eq!(
                p.consume(&launch()).unwrap_err(),
                WorkflowChildPermitError::AlreadyConsumed,
                "the failed consume burnt the permit"
            );
        }
    }

    #[test]
    fn consume_root_mismatch_does_not_burn() {
        let mut p = permit();
        p.claim("root-1", "child").unwrap();
        let mut bad = launch();
        bad.workflow_run_id = "other";
        assert_eq!(
            p.consume(&bad).unwrap_err(),
            WorkflowChildPermitError::RootMismatch
        );
        p.consume(&launch()).unwrap();
    }

    #[test]
    fn all_eight_messages_are_verbatim() {
        let expected = [
            "Workflow child permit is invalid.",
            "Workflow child permit is already consumed.",
            "Workflow child permit does not match this workflow root.",
            "Workflow child permit child key mismatch.",
            "Workflow child permit launch was not claimed.",
            "Workflow child permit agent mismatch.",
            "Workflow child permit supports native Pi children only.",
            "Workflow child permit does not match the final launch projection.",
        ];
        let variants = [
            WorkflowChildPermitError::Invalid,
            WorkflowChildPermitError::AlreadyConsumed,
            WorkflowChildPermitError::RootMismatch,
            WorkflowChildPermitError::ChildKeyMismatch,
            WorkflowChildPermitError::LaunchNotClaimed,
            WorkflowChildPermitError::AgentMismatch,
            WorkflowChildPermitError::NativeChildrenOnly,
            WorkflowChildPermitError::ProjectionMismatch,
        ];
        for (variant, text) in variants.iter().zip(expected) {
            assert_eq!(variant.to_string(), text);
        }
    }
}
