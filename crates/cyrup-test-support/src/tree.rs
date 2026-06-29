//! Branched session-tree fixture builder (Pi `buildTestTree`, utilities.ts:291-315).
//!
//! Builds a session tree from a flat message list with optional `branch_from` markers (a message
//! whose `branch_from` is set first navigates the leaf back to the referenced entry, then appends —
//! producing a branch), returning a text→[`EntryId`] map for assertions.

use std::collections::HashMap;

use cyrup_core::EntryId;
use cyrup_session::error::SessionError;
use cyrup_session::manager::SessionManager;

use crate::messages::{assistant_msg, user_msg};

/// Failure building a test tree (Pi `buildTestTree` throws on an unknown branch ref,
/// utilities.ts:303).
#[derive(Debug, thiserror::Error)]
pub enum TreeError {
    #[error("Cannot branch from unknown entry: {0}")]
    UnknownBranch(String),
    #[error(transparent)]
    Session(#[from] SessionError),
}

/// One node in a [`TreeStructure`] (Pi `{ role, text, branchFrom? }`, utilities.ts:293-294).
#[derive(Clone, Debug)]
pub struct TreeMessage {
    pub role: TreeRole,
    pub text: String,
    /// The `text` of a previously-appended message to branch from before appending this one.
    pub branch_from: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeRole {
    User,
    Assistant,
}

impl TreeMessage {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: TreeRole::User, text: text.into(), branch_from: None }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self { role: TreeRole::Assistant, text: text.into(), branch_from: None }
    }

    /// Mark this message as branching from a previously-appended message (by its text).
    #[must_use]
    pub fn branch_from(mut self, from: impl Into<String>) -> Self {
        self.branch_from = Some(from.into());
        self
    }
}

/// The flat structure consumed by [`build_test_tree`] (Pi `structure.messages`, utilities.ts:293).
#[derive(Clone, Debug, Default)]
pub struct TreeStructure {
    pub messages: Vec<TreeMessage>,
}

impl TreeStructure {
    pub fn new(messages: Vec<TreeMessage>) -> Self {
        Self { messages }
    }
}

/// Build a branched session tree (Pi `buildTestTree`, utilities.ts:291-315). Returns a map from
/// each message's `text` to the [`EntryId`] it produced. Errors if a `branch_from` references an
/// unknown message (Pi throws `Cannot branch from unknown entry`).
pub fn build_test_tree(
    session: &mut SessionManager,
    structure: &TreeStructure,
) -> Result<HashMap<String, EntryId>, TreeError> {
    let mut ids: HashMap<String, EntryId> = HashMap::new();

    for msg in &structure.messages {
        if let Some(from) = &msg.branch_from {
            let branch_from_id = ids
                .get(from)
                .cloned()
                .ok_or_else(|| TreeError::UnknownBranch(from.clone()))?;
            session.branch(&branch_from_id)?;
        }

        let message = match msg.role {
            TreeRole::User => user_msg(&msg.text),
            TreeRole::Assistant => assistant_msg(&msg.text),
        };
        let id = session.append_message(message)?;
        ids.insert(msg.text.clone(), id);
    }

    Ok(ids)
}
