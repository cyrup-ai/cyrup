//! Memo of a decoder's per-block [`Content`](cyrup_core::Content) projection (PERF-001).
//!
//! Every streaming decoder builds a fresh `AssistantMessage` for the `partial` on every event, and
//! its `content` used to be re-projected from scratch each time — re-cloning every accumulated
//! `String` and re-parsing every open tool call's whole argument buffer, on a delta that touched
//! one block. This caches the projection per block so a delta re-projects only the block it
//! changed.
//!
//! The invariant is structural rather than a revision counter: `entries` runs parallel to the
//! decoder's `blocks`, a decoder pushes to both together, and the ONLY way to obtain a
//! `&mut Block` is through an accessor that invalidates that block's slot first. Handing out a
//! mutable block IS the statement "this block may change", so there is no separate bookkeeping to
//! forget to update.
//!
//! The memo covers the PROJECTION only. `project_indexed` still clones every cached
//! [`Content`](cyrup_core::Content) into the returned vector on every snapshot, so the memo alone
//! would still be O(bytes accumulated) per event. What makes it O(blocks) is that a `Content`'s
//! payload is now shared rather than owned — [`SharedStr`](cyrup_core::SharedStr) for text and
//! thinking, [`LazyArgs`](cyrup_core::LazyArgs) for tool arguments — so each of those clones is a
//! refcount bump. The two changes are complementary: the memo stops the re-projection, the shared
//! payloads stop the re-copy.

use cyrup_core::Content;

/// Per-block memo of the projection from a decoder's block to its [`Content`].
#[derive(Default)]
pub(crate) struct ContentCache {
    entries: Vec<Option<Content>>,
}

impl ContentCache {
    /// Add a slot for a newly pushed block. Call in lockstep with the decoder's own `blocks.push`.
    pub(crate) fn push(&mut self) {
        self.entries.push(None);
    }

    /// Drop the memo for `pos`, which is about to be handed out mutably.
    pub(crate) fn invalidate(&mut self, pos: usize) {
        if let Some(slot) = self.entries.get_mut(pos) {
            *slot = None;
        }
    }

    /// The content projection, recomputing only the invalidated slots.
    ///
    /// The length repair is a safety net, not the mechanism: a decoder that pushes through its own
    /// accessor keeps the two vectors in step, so it can only fire if a future edit pushes to
    /// `blocks` directly — in which case the memo degrades to "recompute everything", which is
    /// slow, never wrong.
    pub(crate) fn project<B, F>(&mut self, blocks: &[B], project: F) -> Vec<Content>
    where
        F: Fn(&B) -> Content,
    {
        self.project_indexed(blocks, |_, block| project(block))
    }

    /// [`Self::project`] for a decoder whose projection also depends on something keyed by the
    /// block's POSITION — a side scratch map, say — so the closure is handed the index too.
    pub(crate) fn project_indexed<B, F>(&mut self, blocks: &[B], project: F) -> Vec<Content>
    where
        F: Fn(usize, &B) -> Content,
    {
        if self.entries.len() != blocks.len() {
            self.entries.resize_with(blocks.len(), || None);
        }
        for (i, slot) in self.entries.iter_mut().enumerate() {
            if slot.is_none()
                && let Some(block) = blocks.get(i)
            {
                *slot = Some(project(i, block));
            }
        }
        self.entries.iter().flatten().cloned().collect()
    }
}
