//! The operation applier: [`apply`] and [`apply_batches`].
//!
//! Both are pure functions from a root and some operations to a new root. **Structural sharing is
//! not implemented here** — it falls out of `Arc::make_mut` on the way down ([`at_mut`]), which
//! clones exactly the containers on the mutation spine and leaves every untouched subtree pointing
//! at the same allocation. The unit tests assert it with `Arc::ptr_eq`, which is the only honest way
//! to assert it.
//!
//! [`apply_batches`] exists for the reason upstream gives: *"When only the final result of an
//! ordered backlog is needed, replay its batches without concatenating their operations. One
//! copy-on-write scope is shared across the complete call, so no intermediate revision is exposed or
//! safe to retain."* Here the copy-on-write scope is the single owned root threaded through the loop,
//! and "not exposed" is not a warning but the absence of a binding to expose.

use std::sync::Arc;

use crate::op::{Op, OpBatch, Permutation, TrimLen};
use crate::path::{Path, PathError, Seg, at_mut, list_mut, map_mut, split_last};
use crate::value::{DocRoot, DocValue};

/// Apply one ordered batch to a root.
///
/// # Errors
///
/// [`PathError`] when an operation does not address anything it can be applied to. Upstream calls
/// this *"an operation that cannot be applied inside an addressable lifetime"* and classifies it as
/// storage corruption (`spec.md:4358-4359`); [`ReplayPlan::parse`] is where that classification is
/// made, so this function only has to be honest about failing.
///
/// [`ReplayPlan::parse`]: crate::ReplayPlan::parse
pub fn apply(base: &DocRoot, ops: &[Op]) -> Result<DocRoot, PathError> {
    // Cloning the root's `Arc` is what puts the top container's refcount at >= 2, which is what
    // makes the first `make_mut` below copy rather than mutate. The base is untouched for all time.
    let mut owner = base.clone().into_value();
    for op in ops {
        apply_one(&mut owner, op)?;
    }
    reroot(owner)
}

/// Apply several ordered batches, exposing no intermediate revision.
///
/// # Errors
///
/// [`PathError`] from the first operation that cannot be applied. Earlier batches' effects are
/// discarded with the partially built root, so the caller's `base` is still the last good value.
pub fn apply_batches<'a>(
    base: &DocRoot,
    batches: impl IntoIterator<Item = &'a OpBatch>,
) -> Result<DocRoot, PathError> {
    let mut owner = base.clone().into_value();
    for batch in batches {
        for op in batch.ops() {
            apply_one(&mut owner, op)?;
        }
    }
    reroot(owner)
}

/// Apply one operation in place on an owned root **value**, which is always a [`DocValue::Map`].
///
/// Crate-private and taking a value rather than a [`DocRoot`] so the change recorder can share this
/// code while building its candidate incrementally — the property `spec.md:1305-1310` needs, where
/// adoption *"performs no diffing, application, allocation, or callback"* because the candidate
/// already **is** the value.
pub(crate) fn apply_one(root: &mut DocValue, op: &Op) -> Result<(), PathError> {
    match op {
        Op::ReplaceRoot(next) => {
            *root = next.clone().into_value();
            Ok(())
        }
        Op::Set { path, value } => set(root, path, value.clone()),
        Op::Delete { path } => delete(root, path),
        Op::AppendStr { path, text } => append_str(root, path, text),
        Op::TrimStrFront { path, count } => trim_str_front(root, path, *count),
        Op::Splice {
            path,
            at,
            remove,
            items,
        } => splice(root, path, *at, *remove, items),
        Op::Permute { path, perm } => permute(root, path, perm),
    }
}

/// `["s", path, value]`.
fn set(root: &mut DocValue, path: &Path, value: DocValue) -> Result<(), PathError> {
    let (parent, last) = split_last(path)?;
    let slot = at_mut(root, &parent)?;
    match last {
        Seg::Key(key) => {
            map_mut(slot, key)?.insert(Arc::clone(key), value);
        }
        Seg::Index(index) => {
            let list = list_mut(slot)?;
            let len = list.len();
            let at = usize::try_from(*index).unwrap_or(usize::MAX);
            match list.get_mut(at) {
                Some(element) => *element = value,
                // "Growing length inserts null; writing past the next index throws."
                None if at == len => list.push(value),
                None => return Err(PathError::SetWouldLeaveAHole { index: *index, len }),
            }
        }
    }
    Ok(())
}

/// `["d", path]`. Objects only: *"Arrays stay dense. Writing past the next index and deleting an
/// element throw."*
fn delete(root: &mut DocValue, path: &Path) -> Result<(), PathError> {
    let (parent, last) = split_last(path)?;
    let slot = at_mut(root, &parent)?;
    match last {
        Seg::Key(key) => {
            // `shift_remove`, not `swap_remove`: insertion order is the document's order in the
            // bytes, and a deletion must not reorder the surviving members.
            if map_mut(slot, key)?.shift_remove(&**key).is_none() {
                return Err(PathError::NoSuchKey {
                    key: key.to_string(),
                });
            }
            Ok(())
        }
        Seg::Index(_) => Err(PathError::DeleteOfArrayElement),
    }
}

/// `["a", path, text]`.
fn append_str(root: &mut DocValue, path: &Path, text: &str) -> Result<(), PathError> {
    with_string(root, path, |s| {
        let mut next = String::with_capacity(s.len().saturating_add(text.len()));
        next.push_str(s);
        next.push_str(text);
        Ok(next)
    })
}

/// `["t", path, count]` — remove `count` **bytes** from a string's front.
///
/// # ADR-0030 §14 open question 7, settled here
///
/// *"Chord's string-trim operation counts UTF-16 code units. `["t", path, count]` is a
/// persisted-format decision, and Rust strings are UTF-8. Whether cyrup stores UTF-16 units (wire
/// compatible, awkward), byte offsets (natural, incompatible) or char counts is unsettled, and it is
/// settled once because it is persisted."*
///
/// **Settled as byte offsets**, which is §14's own provisional answer, for the reason it gives and one
/// it does not:
///
/// * *"Only matters if cyrup ever exchanges operation batches with a chord peer; ADR-0029's scope says
///   it does not."* This crate's [`Op`] wire form is cyrup's own persisted format, written and read by
///   cyrup alone, so UTF-16 would buy compatibility with nothing that exists.
/// * **The producer is Rust.** Every trim op in this design is created by [`OpenChange`], called from
///   cyrup or from an extension across ADR-0002's serde boundary — both of which count bytes
///   naturally. UTF-16 would turn an O(1) `str` slice into a walk in order to serve a peer that is out
///   of scope, and would put a lone-surrogate case into a type that cannot hold one.
///
/// **The incompatibility, recorded as §14 asks it to be:** a batch produced by a chord peer and
/// replayed here would mis-trim any string containing a non-ASCII character, and a batch produced here
/// would mis-trim on such a peer. Nothing in ADR-0029's scope exchanges batches with one. If that ever
/// changes, this is the single operation whose wire meaning differs, and it changes by migration rather
/// than in place, because the number is persisted.
///
/// A count that lands inside a multi-byte character is [`PathError::TrimNotACharBoundary`]. It is a
/// real arm rather than a hygiene arm: the count is persisted, so a damaged record can name any offset.
///
/// [`OpenChange`]: crate::OpenChange
fn trim_str_front(root: &mut DocValue, path: &Path, count: TrimLen) -> Result<(), PathError> {
    let count = count.get();
    let at = usize::try_from(count).unwrap_or(usize::MAX);
    with_string(root, path, |s| {
        if at > s.len() {
            return Err(PathError::TrimPastEnd {
                count,
                len: s.len(),
            });
        }
        s.get(at..)
            .map(str::to_owned)
            .ok_or(PathError::TrimNotACharBoundary { count })
    })
}

/// `["p", path, index, remove, items]`.
fn splice(
    root: &mut DocValue,
    path: &Path,
    at: u32,
    remove: u32,
    items: &[DocValue],
) -> Result<(), PathError> {
    let slot = at_mut(root, path)?;
    let list = list_mut(slot)?;
    let len = list.len();
    let start = usize::try_from(at).unwrap_or(usize::MAX);
    if start > len {
        return Err(PathError::SpliceStartOutOfRange { at, len });
    }
    let remove_n = usize::try_from(remove).unwrap_or(usize::MAX);
    let end = start
        .checked_add(remove_n)
        .filter(|end| *end <= len)
        .ok_or(PathError::SpliceRemoveOutOfRange { at, remove, len })?;
    let _removed: Vec<DocValue> = list.splice(start..end, items.iter().cloned()).collect();
    Ok(())
}

/// `["m", path, permutation]` — `new[i] = old[permutation[i]]`.
fn permute(root: &mut DocValue, path: &Path, perm: &Permutation) -> Result<(), PathError> {
    let slot = at_mut(root, path)?;
    let list = list_mut(slot)?;
    let len = list.len();
    if perm.len() != len {
        return Err(PathError::PermutationLengthMismatch {
            perm_len: perm.len(),
            len,
        });
    }
    let mut next = Vec::with_capacity(len);
    for &source in perm.indices() {
        let at = usize::try_from(source).unwrap_or(usize::MAX);
        match list.get(at) {
            // Cloning a `DocValue` clones an `Arc`, not a tree: permuting a 100k array moves 100k
            // pointers, which is exactly upstream's *"copies that array's pointer storage"*.
            Some(value) => next.push(value.clone()),
            // Unreachable: `Permutation::parse` proved every index is below the permutation's own
            // length, and the check above proved that equals `len`. A branch, not an index, so the
            // workspace's `indexing_slicing = deny` has nothing to forgive.
            None => {
                return Err(PathError::PermutationLengthMismatch {
                    perm_len: perm.len(),
                    len,
                });
            }
        }
    }
    *list = next;
    Ok(())
}

/// Replace the string a path addresses with a function of it.
fn with_string<F>(root: &mut DocValue, path: &Path, f: F) -> Result<(), PathError>
where
    F: FnOnce(&str) -> Result<String, PathError>,
{
    let slot = at_mut(root, path)?;
    let next = match slot {
        DocValue::Str(s) => f(s)?,
        other => {
            return Err(PathError::NotAString {
                found: other.type_name(),
            });
        }
    };
    *slot = DocValue::Str(Arc::from(next.as_str()));
    Ok(())
}

/// Put a mutated root value back as a [`DocRoot`].
///
/// Infallible in practice: the value came from a root and the only operation that can change its
/// variant is [`Op::ReplaceRoot`], whose payload is itself a [`DocRoot`]. It is written as a
/// `Result` rather than an `unwrap` for the reason that makes that unobjectionable — the day the
/// premise changes, a `Result` is a compile-checked reminder and an `unwrap` is a crash.
pub(crate) fn reroot(owner: DocValue) -> Result<DocRoot, PathError> {
    match owner {
        DocValue::Map(map) => Ok(DocRoot::from_arc(map)),
        other => Err(PathError::NotAnObject {
            key: "<root>".to_owned(),
            found: other.type_name(),
        }),
    }
}
