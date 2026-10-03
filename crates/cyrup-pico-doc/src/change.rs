//! The tracker, the open change and the prepared result (`spec.md:1270-1310`).
//!
//! This is Chord's `Tracker`/`Change`/`Prepared` triple, with three of its four runtime mechanisms
//! gone. Upstream pays, per change, for a revocation `Proxy`, a `#sealed` flag, an `#assertOpen()`
//! on every method and a validating copy walk at every placement
//! (`packages/durable/src/transaction.ts:663, 673, 898-900`). Here:
//!
//! * **revocation is a borrow.** S4's `Draft<'d, T>` is `{ ch: &'d mut OpenChange, .. }`, so a draft
//!   that outlives its change does not compile and there is nothing to revoke (ADR-0030 F2).
//! * **the copy walk is a move** of an already-parsed [`DocValue`] (ADR-0030 F4).
//! * **staleness is ownership plus one number.** [`Tracker::adopt`] takes the [`Prepared`] by value
//!   and checks its base revision, so "adopting twice" and "adopting a competitor" are one check
//!   over a consumed value rather than four.
//!
//! What is *not* deleted is the `base_revision` check itself. Several changes may be open from one
//! revision, and adopting one must stale the others (`packages/chord/src/delta/README.md`,
//! *"Lifecycle"*). That is state about a value another owner holds, so it stays a check — and
//! `spec.md:1296` requires the Session line to hold at most one open change per tracker anyway,
//! which is S4's job and not expressible here.
//!
//! # The refcount-at-least-two invariant
//!
//! [`Tracker::begin_change`] clones the authority root's `Arc`. That single line is what makes every
//! `Arc::make_mut` in the applier copy-on-write instead of mutating the published revision in place,
//! and it is therefore the whole of `spec.md:4528-4530`'s *"trusted immutable revisions"*. The canary
//! for it is `authority_revision_is_untouched_by_a_draft_write`.

use crate::apply::{apply_one, reroot};
use crate::op::{Op, OpBatch, Permutation, TrimLen};
use crate::path::{Path, PathError};
use crate::value::{DocRoot, DocValue};

/// A revision number. Monotone per tracker, and meaningful only against that tracker.
///
/// Deliberately **not** a commit sequence: it is in-process bookkeeping about one tracker's adoption
/// order, and conflating the two is the confusion ADR-0030 §2.2's first row is about. This crate has
/// no edge to `cyrup-pico-store`, so `Seq` is not even in scope to reach for by mistake.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Revision(u64);

impl Revision {
    /// The revision a freshly tracked root is at.
    pub const INITIAL: Self = Self(0);

    /// The next revision.
    #[must_use]
    const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    /// The revision number, for a diagnostic.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One loaded document's authority revision.
#[derive(Clone, Debug)]
pub struct Tracker {
    value: DocRoot,
    revision: Revision,
}

impl Tracker {
    /// Take ownership of a root, in O(1) and with no traversal.
    ///
    /// Upstream has to say *"`initial` is transferred: never mutate it again"*, and
    /// `packages/chord/src/delta/README.md` has to add that *"roots must be alias-free"* because a
    /// container shared by two keys makes the authority value diverge from what replicas compute
    /// from the ops. Neither sentence has a Rust equivalent: the root is moved, and an `Arc` subtree
    /// at two keys is **correct** here because it is immutable, so the alias-free rule and its
    /// provenance bookkeeping are deleted rather than ported (ADR-0030 F4).
    #[must_use]
    pub fn track(value: DocRoot) -> Self {
        Self {
            value,
            revision: Revision::INITIAL,
        }
    }

    /// The current authority revision's value.
    #[must_use]
    pub fn value(&self) -> &DocRoot {
        &self.value
    }

    /// The current revision number.
    #[must_use]
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// Open a change over the current revision.
    ///
    /// The clone here is an `Arc` clone, which is the refcount-at-least-two step this module's
    /// documentation is about. It does not copy the document.
    #[must_use]
    pub fn begin_change(&self) -> OpenChange {
        OpenChange {
            base: self.value.clone(),
            base_revision: self.revision,
            candidate: self.value.clone().into_value(),
            ops: Vec::new(),
        }
    }

    /// Chord's `prepareReplace`: a whole-root operation, not a diff.
    ///
    /// *"If `value` is deeply equal to the current root, `ops` is empty and the current root is
    /// kept. Otherwise `ops` is `[["r", value]]` and `prepared.value === value`."* The equality is
    /// [`IndexMap`]'s, so it ignores key order — which is what makes the kept-root arm agree with
    /// upstream rather than merely resemble it.
    ///
    /// [`IndexMap`]: indexmap::IndexMap
    #[must_use]
    pub fn prepare_replace(&self, value: DocRoot) -> Prepared {
        if value == self.value {
            return Prepared {
                base: self.value.clone(),
                value: self.value.clone(),
                ops: OpBatch::empty(),
                base_revision: self.revision,
            };
        }
        Prepared {
            base: self.value.clone(),
            value: value.clone(),
            ops: OpBatch::new(vec![Op::ReplaceRoot(value)]),
            base_revision: self.revision,
        }
    }

    /// Adopt a prepared result: a synchronous pointer swap, nothing else.
    ///
    /// `spec.md:1305-1310`: *"`adopt()` accepts only a prepared result from that tracker at its
    /// current revision, then performs only a synchronous pointer swap to the already-computed
    /// immutable `value`. It performs no diffing, application, allocation, or callback."* That
    /// matters because adoption runs **after** storage committed, where failure is unrecoverable:
    /// anything that can fail here turns a committed write into a poisoned session.
    ///
    /// # Errors
    ///
    /// [`StalePreparation`] when another preparation was adopted first. By construction this cannot
    /// mean "already adopted this one": [`Prepared`] is consumed.
    pub fn adopt(&mut self, prepared: Prepared) -> Result<(), StalePreparation> {
        if prepared.base_revision != self.revision {
            return Err(StalePreparation {
                prepared_against: prepared.base_revision,
                tracker_at: self.revision,
            });
        }
        self.value = prepared.value;
        self.revision = self.revision.next();
        Ok(())
    }
}

/// A preparation adopted out of order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
#[error(
    "this preparation was made against revision {} but the tracker is at revision {}",
    prepared_against.get(),
    tracker_at.get()
)]
pub struct StalePreparation {
    /// The revision the preparation was made against.
    pub prepared_against: Revision,
    /// The revision the tracker is at now.
    pub tracker_at: Revision,
}

/// A change in progress: the candidate value and the exact operations that built it.
///
/// The candidate is built **incrementally, as each operation is recorded**, which is what leaves
/// [`OpenChange::prepare`] with nothing to compute and adoption with nothing to do but swap a
/// pointer. It is the same property, read from the other end, as ADR-0030 §2.3's *"the candidate
/// **is** the value"*.
///
/// S4's `Draft<'d, T>` wraps `&'d mut OpenChange` and adds the typed surface; this type is the
/// untyped recorder underneath, and it is pure.
#[derive(Clone, Debug)]
pub struct OpenChange {
    base: DocRoot,
    base_revision: Revision,
    /// Always a [`DocValue::Map`]: it was built from a [`DocRoot`] and only [`Op::ReplaceRoot`] can
    /// change its variant, whose payload is itself a [`DocRoot`].
    candidate: DocValue,
    ops: Vec<Op>,
}

impl OpenChange {
    /// The revision this change is open over.
    #[must_use]
    pub fn base_revision(&self) -> Revision {
        self.base_revision
    }

    /// Read the candidate: read-your-writes inside the change.
    #[must_use]
    pub fn read(&self) -> &DocValue {
        &self.candidate
    }

    /// The value at a path, if the path addresses one.
    #[must_use]
    pub fn read_at(&self, path: &Path) -> Option<&DocValue> {
        let mut cur = &self.candidate;
        for seg in path.segments() {
            cur = match (cur, seg) {
                (DocValue::Map(map), crate::Seg::Key(key)) => map.get(&**key)?,
                (DocValue::List(list), crate::Seg::Index(i)) => {
                    list.get(usize::try_from(*i).unwrap_or(usize::MAX))?
                }
                _ => return None,
            };
        }
        Some(cur)
    }

    /// `["s", path, value]`, with Chord's no-op normalisation.
    ///
    /// *"Writes restored to their original value and deeply equal container assignments usually
    /// normalize to empty."* The equality is [`IndexMap`]'s order-insensitive one, so assigning an
    /// object whose keys differ only in order records nothing — upstream's exact semantics, free from
    /// the container (ADR-0030 §2.3, §4).
    ///
    /// [`IndexMap`]: indexmap::IndexMap
    ///
    /// # Errors
    ///
    /// [`PathError`] when the path does not address an assignable slot.
    pub fn set(&mut self, path: &Path, value: DocValue) -> Result<(), PathError> {
        if self.read_at(path) == Some(&value) {
            return Ok(());
        }
        self.record(Op::Set {
            path: path.clone(),
            value,
        })
    }

    /// `["d", path]`.
    ///
    /// # Errors
    ///
    /// [`PathError`] when the path does not address an object member.
    pub fn delete(&mut self, path: &Path) -> Result<(), PathError> {
        self.record(Op::Delete { path: path.clone() })
    }

    /// `["a", path, text]`. Appending nothing records nothing.
    ///
    /// # Errors
    ///
    /// [`PathError`] when the path does not address a string.
    pub fn append_str(&mut self, path: &Path, text: &str) -> Result<(), PathError> {
        if text.is_empty() {
            return Ok(());
        }
        self.record(Op::AppendStr {
            path: path.clone(),
            text: text.into(),
        })
    }

    /// `["t", path, count]`.
    ///
    /// # Errors
    ///
    /// [`PathError`] when the path does not address a string, the count is longer than it in bytes, or
    /// the count lands inside a multi-byte character.
    pub fn trim_str_front(&mut self, path: &Path, count: TrimLen) -> Result<(), PathError> {
        self.record(Op::TrimStrFront {
            path: path.clone(),
            count,
        })
    }

    /// `["p", path, index, remove, items]`. A splice that removes nothing and inserts nothing
    /// records nothing.
    ///
    /// # Errors
    ///
    /// [`PathError`] when the path does not address an array, or the range is out of bounds.
    pub fn splice(
        &mut self,
        path: &Path,
        at: u32,
        remove: u32,
        items: Vec<DocValue>,
    ) -> Result<(), PathError> {
        if remove == 0 && items.is_empty() {
            return Ok(());
        }
        self.record(Op::Splice {
            path: path.clone(),
            at,
            remove,
            items: items.into(),
        })
    }

    /// `["m", path, permutation]`. The identity permutation records nothing.
    ///
    /// # Errors
    ///
    /// [`PathError`] when the path does not address an array of the permutation's length.
    pub fn permute(&mut self, path: &Path, perm: Permutation) -> Result<(), PathError> {
        if perm
            .indices()
            .iter()
            .enumerate()
            .all(|(i, &source)| u32::try_from(i).map(|i| i == source).unwrap_or(false))
        {
            return Ok(());
        }
        self.record(Op::Permute {
            path: path.clone(),
            perm,
        })
    }

    /// Chord's `prepareReplace`, inside an open change: an O(1) ownership move of a whole root.
    ///
    /// A replacement deeply equal to the candidate records nothing, matching
    /// `packages/chord/src/delta/README.md`'s *"the current root is kept"*.
    pub fn replace_root(&mut self, root: DocRoot) {
        if self.candidate == root.clone().into_value() {
            return;
        }
        // Infallible: `ReplaceRoot` cannot fail to apply.
        self.candidate = root.clone().into_value();
        self.ops.push(Op::ReplaceRoot(root));
    }

    /// Record an operation by applying it to the candidate first, so a rejected operation leaves
    /// neither the candidate nor the batch changed.
    ///
    /// This is upstream's *"throws at the offending assignment, before the draft changes"*
    /// (`spec.md:1362`) — except that here there is nothing to roll back, because the candidate is
    /// only touched once the operation is known to apply.
    fn record(&mut self, op: Op) -> Result<(), PathError> {
        let mut candidate = self.candidate.clone();
        apply_one(&mut candidate, &op)?;
        self.candidate = candidate;
        self.ops.push(op);
        Ok(())
    }

    /// Seal the change into a prepared result.
    ///
    /// Nothing is computed here: the candidate was built as the operations were recorded. That is
    /// the point, and it is why adoption cannot fail for any reason an allocation could cause.
    ///
    /// # Errors
    ///
    /// [`PathError`] only in the unreachable case that the candidate stopped being an object, which
    /// no operation can do — a `Result` rather than an `unwrap`, so the day that premise changes the
    /// compiler says so instead of the process dying after a durable commit.
    pub fn prepare(self) -> Result<Prepared, PathError> {
        Ok(Prepared {
            base: self.base,
            value: reroot(self.candidate)?,
            ops: OpBatch::new(self.ops),
            base_revision: self.base_revision,
        })
    }

    /// Discard the change. `abort` is idempotent upstream because the object survives it; here the
    /// change is consumed, so a second abort does not compile.
    pub fn abort(self) {}
}

/// A sealed change: the base it was computed from, the candidate, and the exact operations.
///
/// All three are immutable for all time, which upstream states as a mutation-rights table row
/// (*"`prepared.value`, `prepared.ops`, op tuples, paths, permutations, payloads — No."*) and which
/// here is the absence of any `&mut` accessor.
#[derive(Clone, Debug)]
pub struct Prepared {
    base: DocRoot,
    value: DocRoot,
    ops: OpBatch,
    base_revision: Revision,
}

impl Prepared {
    /// The revision this preparation was computed from.
    #[must_use]
    pub fn base(&self) -> &DocRoot {
        &self.base
    }

    /// The candidate: the value adoption will swap to, and publication will carry.
    #[must_use]
    pub fn value(&self) -> &DocRoot {
        &self.value
    }

    /// The exact operations, in order.
    #[must_use]
    pub fn ops(&self) -> &OpBatch {
        &self.ops
    }

    /// The revision this preparation was made against.
    #[must_use]
    pub fn base_revision(&self) -> Revision {
        self.base_revision
    }

    /// Whether this preparation changes nothing.
    ///
    /// `spec.md:1357-1358`: *"An existing current-version document with an empty batch writes and
    /// publishes nothing. A nonempty structural batch whose final value is deeply equal to its base
    /// remains a valid durable change and publication."* Both halves are emptiness of the **batch**,
    /// never equality of the values, and [`choose_representation`] is where that distinction is
    /// acted on.
    ///
    /// [`choose_representation`]: crate::choose_representation
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}
