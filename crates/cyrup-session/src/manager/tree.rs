//! Read-only tree queries (R-04-010/025): leaf/entry/children/label lookups, the root→leaf walk,
//! and the defensive [`TreeNode`] copy handed to UIs. Nothing here mutates the manager.

use cyrup_core::EntryId;

use crate::entry::Entry;

use super::SessionManager;

/// A node of the defensive tree copy returned to UIs (R-04-025).
#[derive(Clone, Debug)]
pub struct TreeNode {
    pub entry: Entry,
    pub children: Vec<TreeNode>,
    pub label: Option<String>,
    /// Timestamp of the latest label change for this entry, if any — Pi
    /// `SessionTreeNode.labelTimestamp` (`session-manager.ts:159-167`), the value the `/tree`
    /// selector's `t` toggle renders beside the label. The manager has always held it
    /// (`labels: id → (label, timestamp)`); `getTree` simply never handed it out, so the TUI's
    /// timestamp column had no producer.
    pub label_timestamp: Option<String>,
}

impl SessionManager {
    pub fn leaf_id(&self) -> Option<&EntryId> {
        self.leaf.as_ref()
    }

    pub fn leaf_entry(&self) -> Option<&Entry> {
        self.leaf.as_ref().and_then(|id| self.entry(id))
    }

    pub fn entry(&self, id: &EntryId) -> Option<&Entry> {
        self.by_id.get(id).and_then(|&i| self.entries.get(i))
    }

    pub fn children(&self, id: &EntryId) -> Vec<&Entry> {
        self.children
            .get(id)
            .map(|kids| kids.iter().filter_map(|k| self.entry(k)).collect())
            .unwrap_or_default()
    }

    pub fn label(&self, id: &EntryId) -> Option<&str> {
        self.labels.get(id).map(|(l, _)| l.as_str())
    }

    /// Timestamp of the latest label change for `id` — Pi's `labelTimestampsById`
    /// (`session-manager.ts:865`), surfaced on [`TreeNode::label_timestamp`].
    pub fn label_timestamp(&self, id: &EntryId) -> Option<&str> {
        self.labels.get(id).map(|(_, ts)| ts.as_str())
    }

    /// Walk to root from `from` (default: the current leaf), returned root→leaf (R-04-010).
    pub fn branch_path(&self, from: Option<&EntryId>) -> Vec<&Entry> {
        let mut out = Vec::new();
        let mut cur = from.cloned().or_else(|| self.leaf.clone());
        while let Some(id) = cur {
            let e = match self.entry(&id) {
                Some(e) => e,
                None => break,
            };
            out.push(e);
            cur = e.parent_id();
        }
        out.reverse();
        out
    }

    /// Defensive tree copy for UIs. Children are sorted by timestamp, but roots are left in
    /// insertion order: Pi `getTree` sorts only each node's `children` and pushes roots in entry
    /// order (`session-manager.ts:1210-1234`). Observable only with multiple roots (orphan /
    /// self-parent entries); a well-formed session has exactly one root.
    pub fn tree(&self) -> Vec<TreeNode> {
        let mut visited = std::collections::HashSet::new();
        self.roots
            .iter()
            .filter_map(|id| self.build_node(id, &mut visited))
            .collect()
    }

    fn build_node(
        &self,
        id: &EntryId,
        visited: &mut std::collections::HashSet<EntryId>,
    ) -> Option<TreeNode> {
        // Cycle guard: a malformed file could still form a non-self loop; never revisit a node.
        if !visited.insert(id.clone()) {
            return None;
        }
        let entry = self.entry(id)?.clone();
        let mut kids = self.children.get(id).cloned().unwrap_or_default();
        kids.sort_by_key(|k| {
            self.entry(k)
                .and_then(|e| e.base())
                .map(|b| b.timestamp.clone())
        });
        let children = kids
            .iter()
            .filter_map(|k| self.build_node(k, visited))
            .collect();
        Some(TreeNode {
            entry,
            children,
            label: self.label(id).map(str::to_string),
            label_timestamp: self.label_timestamp(id).map(str::to_string),
        })
    }
}
