//! [`WorkflowResourcePermit`] — the resource half of pi `shared/workflow-child-permit.ts`
//! (`:119-212` @ `df26ebc8`): the host-grant authority validator
//! (`cloneWorkflowResourceAuthority`, `:152-168`), permit issue/consume
//! (`createWorkflowResourcePermit` `:170-193`, `consumeWorkflowResourcePermit` `:195-203`) and
//! the host-call authorization gate (`authorizeWorkflowResourceHost`, `:205-212`).
//!
//! The `WorkflowChildPermit` family (`:60-117`) is deliberately NOT here — it belongs to the
//! workflow launch path (SCOPE_3e/3f).

use super::key::WorkflowKey;
use super::stable_json::stable_json_digest;

/// One host-command grant attached to a resolved workflow resource — pi
/// `WorkflowResourceHostAuthority` (`workflow-child-permit.ts:119-122`).
///
/// `key` is a [`WorkflowKey`] by construction — pi's inline grammar literal in
/// `cloneWorkflowResourceAuthority` (SCOPE_3d §0.14's seventh check) dissolves into the type.
/// The grant-shape guards ("must be an object", "only key and command") are structurally absent
/// for the same reason. `command` stays raw here; the authority validator at permit issue is what
/// bounds and TRIMS it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowResourceHostAuthority {
    /// The `runs.host` key this grant authorizes.
    pub key: WorkflowKey,
    /// The exact command text the grant authorizes (compared trimmed on both sides).
    pub command: String,
}

/// The authority attached to a resolved workflow resource — pi `WorkflowResourceAuthority`
/// (`workflow-child-permit.ts:124-126`).
///
/// `host` stays an `Option`: an ABSENT grant list refuses `runs.host` outright ("runs.host is not
/// allowed for this workflow resource.") where an EMPTY one falls through to the per-key refusal —
/// the two are observably different upstream (`:207-211`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkflowResourceAuthority {
    /// The host-command grants, when the resource declared any.
    pub host: Option<Vec<WorkflowResourceHostAuthority>>,
}

/// pi `cloneWorkflowResourceAuthority` (`workflow-child-permit.ts:152-168`): the validated deep
/// clone a permit stores — never the caller's value. ≤ 32 grants; grant keys unique; commands
/// non-empty after trim, ≤ 16 KiB **after trim**, NUL-free; and the **trimmed** command is what
/// is stored (observable downstream: `authorize_host` compares against `command.trim()`, so both
/// sides normalize and a caller may pass a padded command).
fn clone_workflow_resource_authority(
    authority: &WorkflowResourceAuthority,
) -> Result<WorkflowResourceAuthority, String> {
    let Some(host) = &authority.host else {
        return Ok(WorkflowResourceAuthority { host: None });
    };
    if host.len() > 32 {
        return Err(
            "Workflow resource host authority must be an array of at most 32 grants.".to_string(),
        );
    }
    let mut keys: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut cloned: Vec<WorkflowResourceHostAuthority> = Vec::with_capacity(host.len());
    for grant in host {
        // The grammar half of pi's "unique safe key" check lives in the type; uniqueness is the
        // half that remains.
        if !keys.insert(grant.key.as_str()) {
            return Err("Workflow resource host grant requires a unique safe key.".to_string());
        }
        let trimmed = grant.command.trim();
        if trimmed.is_empty() || trimmed.len() > 16 * 1024 || grant.command.contains('\0') {
            return Err(
                "Workflow resource host grant requires a non-empty command of at most 16384 bytes without NUL."
                    .to_string(),
            );
        }
        cloned.push(WorkflowResourceHostAuthority {
            key: grant.key.clone(),
            command: trimmed.to_string(),
        });
    }
    Ok(WorkflowResourceAuthority { host: Some(cloned) })
}

/// The literal `kind: "workflow"` on a [`WorkflowResourceProvenance`] — a single-variant enum so
/// the wire shape is fixed rather than a free `String` (pi `shared/types.ts:154`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowResourceProvenanceKind {
    /// The only value: a workflow resource.
    Workflow,
}

/// The literal `invocation: "named"` (pi `shared/types.ts:157`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowResourceInvocation {
    /// The only value: the resource was invoked by name.
    Named,
}

/// The literal `expansion: "resolved"` (pi `shared/types.ts:158`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowResourceExpansionState {
    /// The only value: the script came from a registered resolver, not caller text.
    Resolved,
}

/// Audit provenance for a resolved workflow resource — pi `WorkflowResourceProvenance`
/// (`shared/types.ts:152-159`): three literal-typed fields, the resource identity, and a
/// **hyphenated** UUID (`randomUUID()`, SCOPE_3d §0.22 — not `RunId`'s `as_simple` form).
///
/// Built ONCE, inside [`WorkflowResourcePermit::issue`] — upstream constructs it twice (permit
/// `:184-191`, resolution `:216-223`); here the permit owns it and the resolver reads it back, so
/// the two cannot drift.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowResourceProvenance {
    /// Always `"workflow"`.
    pub kind: WorkflowResourceProvenanceKind,
    /// The resource's registered name.
    pub name: WorkflowKey,
    /// The resource's registered version.
    pub version: u32,
    /// Always `"named"`.
    pub invocation: WorkflowResourceInvocation,
    /// Always `"resolved"`.
    pub expansion: WorkflowResourceExpansionState,
    /// The resolution's unique id — hyphenated UUID v4.
    pub id: String,
}

/// The two-state resource permit lifecycle (pi `WorkflowResourcePermitRecord.state`,
/// `workflow-child-permit.ts:147`) — `available | consumed`; the three-state
/// `available|claimed|consumed` belongs to the CHILD permit family, not here (SCOPE_3d §0.15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkflowResourcePermitState {
    /// Issued and not yet consumed.
    Available,
    /// Consumed exactly once.
    Consumed,
}

/// The input to [`WorkflowResourcePermit::issue`] — pi `WorkflowResourcePermitInput`
/// (`workflow-child-permit.ts:128-134`).
#[derive(Clone, Debug)]
pub struct WorkflowResourcePermitInput {
    /// The resource's registered name. pi's `required(resourceName)` non-empty-trimmed check is
    /// subsumed by the type: the key grammar admits no whitespace and no empty string.
    pub resource_name: WorkflowKey,
    /// The resource's registered version (positive).
    pub resource_version: u32,
    /// The resolution's unique id (hyphenated UUID).
    pub resource_id: String,
    /// The [`stable_json_digest`] of the resolved script text.
    pub script_digest: String,
    /// The raw authority the resolver declared; validated and clone-stored by `issue`.
    pub authority: WorkflowResourceAuthority,
}

/// pi `required` (`workflow-child-permit.ts:54-57`): non-empty AND already trimmed.
fn required(value: &str, label: &str) -> Result<String, String> {
    if value.trim().is_empty() || value.trim() != value {
        Err(format!("{label} must be a non-empty trimmed string."))
    } else {
        Ok(value.to_string())
    }
}

/// An unforgeable, single-consumption capability proving a workflow script came out of a
/// registered resource resolver rather than from caller-supplied text — pi
/// `WorkflowResourcePermit` (`workflow-child-permit.ts:136-138`).
///
/// [CYRUP-DELTA, strictly stronger] Upstream brands this as an empty frozen object and keeps its
/// real record in a module-private `WeakMap` (`workflow-child-permit.ts:150`), because JS has no
/// way to make a value unforgeable. Rust does: the fields are private, the only constructor is
/// [`WorkflowResourcePermit::issue`] in this module, and the type is deliberately not
/// `Clone`/`Copy`/`Default`/`Deserialize` (nor `Serialize` — it is in-memory only). The `WeakMap`
/// indirection therefore has no analogue here and is deliberately not reproduced — it was never
/// the security property, only the mechanism available. The "permit is invalid" refusal for a
/// record-less permit is structurally absent for the same reason.
#[derive(Debug)]
pub struct WorkflowResourcePermit {
    resource_name: WorkflowKey,
    script_digest: String,
    authority: WorkflowResourceAuthority,
    provenance: WorkflowResourceProvenance,
    state: WorkflowResourcePermitState,
}

/// What a successful [`WorkflowResourcePermit::consume`] hands back — upstream's
/// `{ provenance, authority }` record (`workflow-child-permit.ts:195,202`).
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowResourceConsumption {
    /// The permit's provenance, for the launch record.
    pub provenance: WorkflowResourceProvenance,
    /// The validated (trimmed) authority, for `runs.host` gating.
    pub authority: WorkflowResourceAuthority,
}

impl WorkflowResourcePermit {
    /// pi `createWorkflowResourcePermit` (`workflow-child-permit.ts:170-193`): validates the
    /// identity fields and the authority (storing the validated clone, never the caller's value),
    /// and builds the provenance once.
    ///
    /// # Errors
    ///
    /// Upstream's messages verbatim: the `required` message for a blank/untrimmed
    /// `resource_id`/`script_digest`, `"resourceVersion must be a positive integer."`, or one of
    /// [`clone_workflow_resource_authority`]'s three authority rejections.
    pub fn issue(input: WorkflowResourcePermitInput) -> Result<Self, String> {
        let resource_id = required(&input.resource_id, "resourceId")?;
        let script_digest = required(&input.script_digest, "scriptDigest")?;
        if input.resource_version < 1 {
            return Err("resourceVersion must be a positive integer.".to_string());
        }
        let authority = clone_workflow_resource_authority(&input.authority)?;
        let provenance = WorkflowResourceProvenance {
            kind: WorkflowResourceProvenanceKind::Workflow,
            name: input.resource_name.clone(),
            version: input.resource_version,
            invocation: WorkflowResourceInvocation::Named,
            expansion: WorkflowResourceExpansionState::Resolved,
            id: resource_id,
        };
        Ok(Self {
            resource_name: input.resource_name,
            script_digest,
            authority,
            provenance,
            state: WorkflowResourcePermitState::Available,
        })
    }

    /// The provenance this permit carries — read back by the resolver so the resolution and the
    /// permit can never disagree (see [`WorkflowResourceProvenance`]).
    #[must_use]
    pub fn provenance(&self) -> &WorkflowResourceProvenance {
        &self.provenance
    }

    /// pi `consumeWorkflowResourcePermit` (`workflow-child-permit.ts:195-203`): verify the script
    /// against the issued digest and permanently consume the permit — `&mut self`, exactly once.
    ///
    /// # Errors
    ///
    /// `"Workflow resource permit is already consumed."` on a second consumption, or
    /// `"Workflow resource permit does not match the resolved workflow script."` when `script`'s
    /// digest differs from the issued one.
    pub fn consume(&mut self, script: &str) -> Result<WorkflowResourceConsumption, String> {
        if self.state != WorkflowResourcePermitState::Available {
            return Err("Workflow resource permit is already consumed.".to_string());
        }
        if stable_json_digest(&serde_json::Value::String(script.to_string())) != self.script_digest
        {
            return Err(
                "Workflow resource permit does not match the resolved workflow script.".to_string(),
            );
        }
        self.state = WorkflowResourcePermitState::Consumed;
        Ok(WorkflowResourceConsumption {
            provenance: self.provenance.clone(),
            authority: self.authority.clone(),
        })
    }

    /// pi `authorizeWorkflowResourceHost` (`workflow-child-permit.ts:205-212`): validate one
    /// `runs.host(key, command)` call against the consumed permit's authority.
    ///
    /// `key` is a raw `&str` deliberately: it is untrusted script-author input whose failure mode
    /// is THIS function's per-key refusal (upstream produces the same sentence for an unknown and
    /// a malformed key), never a parse error.
    ///
    /// # Errors
    ///
    /// `"Workflow resource authority is unavailable."` before consumption, `"runs.host is not
    /// allowed for this workflow resource."` when no grant list was declared, or the per-key
    /// refusal naming the key and the resource.
    pub fn authorize_host(&self, key: &str, command: &str) -> Result<(), String> {
        if self.state != WorkflowResourcePermitState::Consumed {
            return Err("Workflow resource authority is unavailable.".to_string());
        }
        let Some(host) = &self.authority.host else {
            return Err("runs.host is not allowed for this workflow resource.".to_string());
        };
        let trimmed = command.trim();
        if host
            .iter()
            .any(|grant| grant.key.as_str() == key && grant.command == trimmed)
        {
            Ok(())
        } else {
            Err(format!(
                "The command for runs.host('{key}') is not allowed for workflow resource '{}'.",
                self.resource_name.as_str()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn key(raw: &str) -> WorkflowKey {
        WorkflowKey::parse(raw).expect("valid key")
    }

    fn input(script: &str, authority: WorkflowResourceAuthority) -> WorkflowResourcePermitInput {
        WorkflowResourcePermitInput {
            resource_name: key("run-ci"),
            resource_version: 1,
            resource_id: "11111111-2222-4333-8444-555555555555".to_string(),
            script_digest: stable_json_digest(&serde_json::Value::String(script.to_string())),
            authority,
        }
    }

    /// The authority validator: ≤ 32 grants, unique keys, bounded NUL-free commands stored
    /// TRIMMED — and `authorize_host` trims its side too, so a padded caller command matches.
    #[test]
    fn authority_is_validated_and_stored_trimmed() {
        let authority = WorkflowResourceAuthority {
            host: Some(vec![WorkflowResourceHostAuthority {
                key: key("ci"),
                command: "  npm test  ".to_string(),
            }]),
        };
        let mut permit = WorkflowResourcePermit::issue(input("s", authority)).expect("issues");
        assert_eq!(
            permit.authorize_host("ci", "npm test"),
            Err("Workflow resource authority is unavailable.".to_string()),
            "authority is unavailable before consumption"
        );
        let consumption = permit.consume("s").expect("consumes");
        assert_eq!(
            consumption
                .authority
                .host
                .as_deref()
                .and_then(<[_]>::first)
                .map(|grant| grant.command.as_str()),
            Some("npm test"),
            "the TRIMMED command is what is stored"
        );
        assert_eq!(permit.authorize_host("ci", "  npm test "), Ok(()));
        assert_eq!(
            permit.authorize_host("ci", "rm -rf /"),
            Err("The command for runs.host('ci') is not allowed for workflow resource 'run-ci'.".to_string())
        );
        assert_eq!(
            permit.authorize_host("other", "npm test"),
            Err("The command for runs.host('other') is not allowed for workflow resource 'run-ci'.".to_string())
        );
    }

    /// The three authority rejections and the two identity rejections, upstream's wording.
    #[test]
    fn issue_rejects_bad_inputs_with_upstreams_messages() {
        let dup = WorkflowResourceAuthority {
            host: Some(vec![
                WorkflowResourceHostAuthority { key: key("ci"), command: "a".to_string() },
                WorkflowResourceHostAuthority { key: key("ci"), command: "b".to_string() },
            ]),
        };
        assert_eq!(
            WorkflowResourcePermit::issue(input("s", dup)).err(),
            Some("Workflow resource host grant requires a unique safe key.".to_string())
        );
        let over = WorkflowResourceAuthority {
            host: Some(
                (0..33)
                    .map(|index| WorkflowResourceHostAuthority {
                        key: key(&format!("k{index}")),
                        command: "c".to_string(),
                    })
                    .collect(),
            ),
        };
        assert_eq!(
            WorkflowResourcePermit::issue(input("s", over)).err(),
            Some("Workflow resource host authority must be an array of at most 32 grants.".to_string())
        );
        let nul = WorkflowResourceAuthority {
            host: Some(vec![WorkflowResourceHostAuthority {
                key: key("ci"),
                command: "a\0b".to_string(),
            }]),
        };
        assert_eq!(
            WorkflowResourcePermit::issue(input("s", nul)).err(),
            Some("Workflow resource host grant requires a non-empty command of at most 16384 bytes without NUL.".to_string())
        );
        let mut blank_id = input("s", WorkflowResourceAuthority::default());
        blank_id.resource_id = " ".to_string();
        assert_eq!(
            WorkflowResourcePermit::issue(blank_id).err(),
            Some("resourceId must be a non-empty trimmed string.".to_string())
        );
        let mut zero_version = input("s", WorkflowResourceAuthority::default());
        zero_version.resource_version = 0;
        assert_eq!(
            WorkflowResourcePermit::issue(zero_version).err(),
            Some("resourceVersion must be a positive integer.".to_string())
        );
    }

    /// Two-state lifecycle: a digest mismatch refuses without consuming; consumption is exactly
    /// once; an absent grant list refuses `runs.host` outright (distinct from an empty one).
    #[test]
    fn consume_is_single_shot_and_digest_checked() {
        let mut permit =
            WorkflowResourcePermit::issue(input("script", WorkflowResourceAuthority::default()))
                .expect("issues");
        assert_eq!(
            permit.consume("tampered").err(),
            Some("Workflow resource permit does not match the resolved workflow script.".to_string())
        );
        let consumption = permit.consume("script").expect("still available");
        assert_eq!(consumption.authority.host, None);
        assert_eq!(
            permit.consume("script").err(),
            Some("Workflow resource permit is already consumed.".to_string())
        );
        assert_eq!(
            permit.authorize_host("ci", "npm test"),
            Err("runs.host is not allowed for this workflow resource.".to_string()),
            "no grant list at all refuses runs.host outright"
        );
        // The provenance carries the three literals and the hyphenated id.
        assert_eq!(
            serde_json::to_value(permit.provenance()).expect("serializes"),
            serde_json::json!({
                "kind": "workflow",
                "name": "run-ci",
                "version": 1,
                "invocation": "named",
                "expansion": "resolved",
                "id": "11111111-2222-4333-8444-555555555555",
            })
        );
    }
}
