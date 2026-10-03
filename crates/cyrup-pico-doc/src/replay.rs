//! Stored content, the replay plan, and `materialize` (ADR-0030 F6 §C).
//!
//! # The one rule this module exists for
//!
//! `spec.md:4357-4359`: *"A missing required base, a version change inside a delta tail, or an
//! operation that cannot be applied inside an addressable lifetime is **storage corruption, not
//! absence**."*
//!
//! Upstream carries that distinction entirely in **which code path is taken** — a legitimately
//! absent record returns `undefined` and a damaged one throws — so the two channels are one
//! `if` away from being confused, and reporting corruption as absence loses a user's work quietly:
//! the session opens, the document is empty, and the agent proceeds from a blank slate.
//!
//! Here [`ReplayPlan::parse`] is the single home of all three rules, so the channels are
//! `Result<Option<T>, StorageFailure>`'s two arms in S3 and cannot be reached by accident
//! (`G-CORRUPTION-NOT-ABSENCE`).
//!
//! # Why `parse` replays and `materialize` only hands the value back
//!
//! ADR-0030 §10 requires `materialize` to be **pure and total** — no `Result`. The third corruption
//! rule is about an operation failing to apply, which cannot be known without applying it, so the
//! replay happens inside `parse`, where failing is the correct answer. What is left for
//! `materialize` is an `Arc` clone. That is not a hollowed-out function: it is the honest shape of
//! "all the ways this can fail have already happened", and it is what lets every caller downstream
//! of the parser be total.

use crate::apply::apply_one;
use crate::op::OpBatch;
use crate::path::PathError;
use crate::value::{DocRoot, DocValue};
use crate::version::{DefVersion, StoredVersion};

use serde::{Deserialize, Serialize};

/// One stored document content record (`spec.md:4237-4240`'s `DocumentContent`).
///
/// The version is on **each record**, not on the incarnation, *"because one incarnation may contain
/// records written by multiple definition versions"* (`spec.md:1119-1120`).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StoredContent {
    /// A complete value. Creation always stores one (`spec.md:1374`), as does every required version
    /// transition.
    Base {
        /// The definition version that wrote it.
        version: DefVersion,
        /// The complete document object.
        value: DocRoot,
    },
    /// An ordered Chord operation batch, continuing the newest applicable base.
    Delta {
        /// The definition version that wrote it. It must equal the base's, or the tail straddles a
        /// schema and is corruption.
        version: DefVersion,
        /// The operations.
        ops: OpBatch,
    },
}

impl StoredContent {
    /// The version this record was written by.
    #[must_use]
    pub const fn version(&self) -> DefVersion {
        match self {
            Self::Base { version, .. } | Self::Delta { version, .. } => *version,
        }
    }

    /// Whether this record is a base.
    #[must_use]
    pub const fn is_base(&self) -> bool {
        matches!(self, Self::Base { .. })
    }
}

/// Why a set of stored content records cannot be replayed.
///
/// `Serialize` only, per ADR-0030 §10: a corruption report must be loggable and surfaceable in a
/// diagnostic without becoming a construction path. A deserialisable `Corruption` would let a
/// damaged log line reintroduce a classification nobody made.
#[derive(Clone, PartialEq, Debug, Serialize, thiserror::Error)]
pub enum Corruption {
    /// A delta tail with no base before it.
    ///
    /// `spec.md:4357`. The records are not merely incomplete: a tail without its base cannot be
    /// replayed into any value, so answering `None` would report a document the user wrote as one
    /// that was never created.
    #[error("the stored records begin with a delta: there is no base to replay onto")]
    MissingRequiredBase,
    /// A version change inside a delta tail.
    ///
    /// `spec.md:4366-4367`: *"Deltas cannot cross a stored version boundary; a version transition
    /// must be a base."* Replaying it anyway applies paths against the wrong shape and yields
    /// plausible-looking corrupt state rather than an error — which is why this is the arm ADR-0030
    /// §2.3 calls the subtle one.
    #[error(
        "a delta at version {found} follows a base at version {base}: a delta cannot cross a version boundary"
    )]
    VersionChangeInDeltaTail {
        /// The version of the selected base.
        base: DefVersion,
        /// The version the offending delta claims.
        found: DefVersion,
        /// Its position in the record set, counted from the selected base.
        at_delta: usize,
    },
    /// An operation that cannot be applied to the value the records before it produced.
    #[error("delta {at_delta} operation {at_op} cannot be applied: {reason}")]
    OperationNotApplicable {
        /// Which delta, counted from the selected base.
        at_delta: usize,
        /// Which operation inside it.
        at_op: usize,
        /// Why, rendered: [`PathError`] is not `Serialize`, and a corruption report needs to be.
        reason: String,
    },
}

/// A replayed incarnation: the materialized value, its stored version, and the delta count.
///
/// Obtainable only from [`ReplayPlan::parse`], which is what makes [`ReplayPlan::version`] a
/// trustworthy minting site for [`StoredVersion`].
#[derive(Clone, Debug)]
pub struct ReplayPlan {
    value: DocRoot,
    version: StoredVersion,
    deltas_since_base: u32,
}

impl ReplayPlan {
    /// Select the newest applicable base, replay the ordered tail after it, and classify every way
    /// the records can be damaged.
    ///
    /// `records` is one incarnation's content records **in stored order** — which is the order
    /// `spec.md:4350` keeps meaningful. The newest base wins: *"It selects the newest applicable
    /// base, applies its ordered Chord delta tail"* (`spec.md:4348-4349`), so records before that
    /// base are not read and a legitimately reclaimed prefix is not corruption.
    ///
    /// # The absence channel is not this function's
    ///
    /// An **empty** record set is [`Corruption::MissingRequiredBase`], not absence. That is not a
    /// confusion of the two channels but the division between them: an incarnation that does not
    /// exist is `Ok(None)` from the backend's *record* lookup, which happens before this call, and
    /// `spec.md:1374`'s *"creation always stores a complete base"* means an incarnation that does
    /// exist has one. A known incarnation with no content records is therefore damaged, and saying
    /// so is the whole point (`G-CORRUPTION-NOT-ABSENCE`).
    ///
    /// # Errors
    ///
    /// [`Corruption`], in the three arms `spec.md:4357-4359` names.
    pub fn parse(records: &[StoredContent]) -> Result<Self, Corruption> {
        let Some(base_at) = records.iter().rposition(StoredContent::is_base) else {
            return Err(Corruption::MissingRequiredBase);
        };

        let (base_version, mut owner) = match records.get(base_at) {
            Some(StoredContent::Base { version, value }) => (*version, value.clone().into_value()),
            // Unreachable: `rposition` returned this index for `is_base`.
            _ => return Err(Corruption::MissingRequiredBase),
        };

        let tail = records.get(base_at.saturating_add(1)..).unwrap_or_default();
        let mut deltas_since_base = 0_u32;
        for (at_delta, record) in tail.iter().enumerate() {
            let ops = match record {
                StoredContent::Delta { version, ops } => {
                    if *version != base_version {
                        return Err(Corruption::VersionChangeInDeltaTail {
                            base: base_version,
                            found: *version,
                            at_delta,
                        });
                    }
                    ops
                }
                // Unreachable: `base_at` is the LAST base, so the tail holds no base.
                StoredContent::Base { version, .. } => {
                    return Err(Corruption::VersionChangeInDeltaTail {
                        base: base_version,
                        found: *version,
                        at_delta,
                    });
                }
            };
            for (at_op, op) in ops.ops().iter().enumerate() {
                apply_one(&mut owner, op).map_err(|reason: PathError| {
                    Corruption::OperationNotApplicable {
                        at_delta,
                        at_op,
                        reason: reason.to_string(),
                    }
                })?;
            }
            deltas_since_base = deltas_since_base.saturating_add(1);
        }

        let value = match owner {
            DocValue::Map(map) => DocRoot::from_arc(map),
            // Unreachable: `Op::ReplaceRoot` carries a `DocRoot`, and no other operation can change
            // the root's variant.
            other => {
                return Err(Corruption::OperationNotApplicable {
                    at_delta: usize::try_from(deltas_since_base).unwrap_or(usize::MAX),
                    at_op: 0,
                    reason: format!(
                        "replay produced {} rather than an object",
                        other.type_name()
                    ),
                });
            }
        };

        Ok(Self {
            value,
            version: StoredVersion::mint(base_version),
            deltas_since_base,
        })
    }

    /// The stored version, as a witness.
    ///
    /// This is the only minting site for [`StoredVersion`] in the workspace, and it is reachable only
    /// with records in hand — so a delta written later cannot claim a version nobody read
    /// (`G-VERSION-PER-RECORD`).
    #[must_use]
    pub const fn version(&self) -> StoredVersion {
        self.version
    }

    /// How many deltas were replayed after the selected base.
    ///
    /// `spec.md:1392-1394`: *"`deltasSinceBase` counts the deltas already stored after the
    /// incarnation's newest base, excluding the change being evaluated. Storage reports it when it
    /// materializes the current value."* This is that number, and it is why the checkpoint predicate
    /// needs no storage read.
    #[must_use]
    pub const fn deltas_since_base(&self) -> u32 {
        self.deltas_since_base
    }

    /// The materialized value, without the `Arc` clone [`materialize`] makes.
    #[must_use]
    pub const fn value(&self) -> &DocRoot {
        &self.value
    }
}

/// The materialized current value of a replay plan. **Pure and total**, as ADR-0030 §10 requires.
///
/// O(1): a `DocRoot` is an `Arc`, so this hands out a share of an immutable value rather than a copy
/// of it. That is the signature half of ADR-0030 F4's ownership rule — *"a read NEVER returns a
/// reference into the backend's decoded index or cache"* — and it is what lets `memory.ts:92-100`'s
/// recursive clone be deleted rather than ported (`G-OWNERSHIP-BOUNDARY`).
#[must_use]
pub fn materialize(plan: &ReplayPlan) -> DocRoot {
    plan.value.clone()
}
