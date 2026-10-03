//! Base-or-delta selection: `choose_representation` (`spec.md:1374-1399`).
//!
//! # Why this is a pure function and not a Session method
//!
//! `spec.md:1388-1399` makes three demands at once, and they are easy to satisfy separately and hard
//! to satisfy together:
//!
//! 1. the predicate is evaluated **exactly once**, after preparation;
//! 2. its `deltasSinceBase` **excludes the change being evaluated**;
//! 3. evaluation **performs no storage read**.
//!
//! Upstream satisfies all three by discipline: the Session calls the predicate at one fixed point and
//! maintains the counter itself. The hazards that leaves are real — *"evaluating twice lets a
//! stateful predicate observe an inconsistent count; a read puts I/O on the held line"* — and the
//! line in question is the mutation line, held through storage settlement.
//!
//! Making it a pure function over `(candidate, ops, deltas_since_base)` discharges (3) structurally:
//! there is no storage handle in scope to read from, in this function or in the predicate it calls.
//! (1) becomes visible in one place instead of auditable across a method, and (2) is the caller's
//! number with the ADR's own note that it is a count of records **already stored**.
//!
//! ADR-0030 §13 (`spec.md:4594`) forbids a backend heuristic here: a definition that never
//! checkpoints may create an unbounded replay tail, and *"that is a definition bug, not a backend
//! heuristic."* So this function has no size threshold, no byte count and no comparison against
//! `initial()`, deliberately.

use crate::change::Prepared;
use crate::value::DocRoot;

/// Whether a base is required regardless of what the definition's predicate would say.
///
/// `spec.md:1374, 1398`: *"Creation always stores a complete base"* and *"Creation and version
/// transitions require bases and do not call it."* Two named variants rather than a `bool`, because
/// at the call site `BaseRequirement::Required` says which rule applies and `true` does not.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BaseRequirement {
    /// Creation, or a definition-version transition, or the first write after a migration
    /// (`spec.md:1461-1463`). A base is written even when the prepared batch is empty.
    Required,
    /// An ordinary later mutation. The definition's predicate decides.
    Optional,
}

/// What a prepared change should be stored as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Representation {
    /// Write nothing and publish nothing.
    ///
    /// `spec.md:1357`: *"An existing current-version document with an empty batch writes and
    /// publishes nothing."* The condition is emptiness of the **batch**, never equality of the
    /// values: *"A nonempty structural batch whose final value is deeply equal to its base remains a
    /// valid durable change and publication."* Both halves fall out of one `is_empty` here, which is
    /// why there is no second whole-value comparison anywhere in this crate.
    Nothing,
    /// Store the complete candidate value.
    Base,
    /// Store the prepared operation batch.
    ///
    /// *"A Chord root-replacement operation remains a delta unless the definition selected a
    /// checkpoint; it does not authorize reclamation"* (`spec.md:1407-1409`) — so this arm is
    /// returned for a batch that happens to be `[["r", value]]` too, and reclamation is not this
    /// function's business.
    Delta,
}

/// Everything a checkpoint predicate is allowed to see.
///
/// Exactly `(candidate, ops, deltas_since_base)`, as `spec.md:1390` passes
/// `(candidateValue, ops, { deltasSinceBase })`. There is no storage handle, no Session and no
/// context, so *"evaluation performs no Storage read"* is a property of the type rather than a rule
/// in prose.
#[derive(Clone, Copy, Debug)]
pub struct CheckpointInput<'a> {
    /// The prepared candidate value — the value that will be adopted and published.
    pub candidate: &'a DocRoot,
    /// The exact operations that produced it.
    pub ops: &'a crate::OpBatch,
    /// Deltas already stored after the incarnation's newest base, **excluding this change**.
    pub deltas_since_base: u32,
}

/// Choose how one prepared change is stored, evaluating the definition's predicate at most once.
///
/// The decision table, which is `spec.md:1374-1401` in full:
///
/// | `required` | batch | predicate | result |
/// |---|---|---|---|
/// | `Required` | any | **not called** | [`Representation::Base`] |
/// | `Optional` | empty | **not called** | [`Representation::Nothing`] |
/// | `Optional` | nonempty | `true` | [`Representation::Base`] |
/// | `Optional` | nonempty | `false` or absent | [`Representation::Delta`] |
///
/// `checkpoint_when` is `None` when the definition declared no predicate, which `spec.md:1390`'s
/// `?? false` makes equivalent to one returning `false`. It is taken as a `&dyn Fn` rather than a
/// generic so that "evaluated at most once" is checkable by reading this function, and so that the
/// absent case is a `None` rather than a defaulted closure the caller has to supply.
#[must_use]
pub fn choose_representation(
    required: BaseRequirement,
    prepared: &Prepared,
    deltas_since_base: u32,
    checkpoint_when: Option<&dyn Fn(CheckpointInput<'_>) -> bool>,
) -> Representation {
    if required == BaseRequirement::Required {
        // "Creation and version transitions require bases and do not call it." Returning before the
        // predicate is reachable is what makes that true rather than intended.
        return Representation::Base;
    }
    if prepared.is_empty() {
        return Representation::Nothing;
    }
    let checkpoint = checkpoint_when.is_some_and(|predicate| {
        predicate(CheckpointInput {
            candidate: prepared.value(),
            ops: prepared.ops(),
            deltas_since_base,
        })
    });
    if checkpoint {
        Representation::Base
    } else {
        Representation::Delta
    }
}
