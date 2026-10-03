//! Document records, scopes and addresses (`spec.md:1066-1120`, `:4229-4234`).

use core::fmt;

use serde::{Deserialize, Serialize};

use crate::{ConversationId, DocumentId, Kind, Lifetime, Seq, TaskId};

/// One persisted document incarnation (`spec.md:1068-1086`).
///
/// `spec.md:1113-1116`: *"`id` is never reused. Retiring and recreating the same logical kind, scope
/// and family key creates a new incarnation."* So a record is created once and then only retired;
/// its content is a separate, backend-private sequence of base and delta records.
///
/// # Why the record, and not the caller's token, is the authority
///
/// `spec.md:1117-1120`: *"The record preserves scope and conversation history/fork semantics so
/// unavailable extension code does not make existing data disappear."* ADR-0030 §2.3 classifies the
/// token-versus-record agreement check **checked**, and calls it *"correct as a check"* for that
/// reason — extension code reloads independently of its data. That check belongs to PICO5-PLAN S5;
/// what belongs here is that the record carries the semantics at all.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DocumentRecord {
    /// This incarnation. Never reused.
    pub id: DocumentId,
    /// Its definition kind.
    pub kind: Kind,
    /// Its family key. Absent for a singleton.
    ///
    /// `spec.md:4231-4234` and `:4341-4342` make the distinction load-bearing: *"A missing key means
    /// the singleton, not every family member."*
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Its membership interval, stamped by the committing storage.
    ///
    /// `spec.md:1111-1114`: the half-open interval `created_at <= at < retired_at`, with an unretired
    /// incarnation having no upper bound and *"a creation retired in the same commit"* having an empty
    /// one. [`Lifetime`] is the one implementation of that asymmetry (ADR-0030 §2.2).
    pub lifetime: Lifetime,
    /// Its scope, and — for a conversation document only — its history and fork policy.
    pub scope: DocumentScope,
}

impl DocumentRecord {
    /// This incarnation's logical address.
    ///
    /// `spec.md:1116-1117`: *"A singleton is identified logically by kind and scope. A family is
    /// identified logically by kind, scope and `key`."* One function, so the address a write occupies
    /// and the address a read resolves cannot be computed two different ways.
    #[must_use]
    pub fn address(&self) -> DocumentAddress {
        DocumentAddress {
            kind: self.kind.clone(),
            scope: self.scope.as_ref(),
            key: self.key.clone(),
        }
    }

    /// Whether this incarnation retains history for a numeric read (`spec.md:4357-4358`).
    ///
    /// Only a rewindable conversation document does. For every other scope a numeric lookup is
    /// [`StorageFailure::HistoryNotRetained`] rather than a guess over whatever records survived
    /// reclamation — ADR-0030 §2.3's *"the only thing between `rewindable` being a promise and a best
    /// effort"*.
    ///
    /// [`StorageFailure::HistoryNotRetained`]: crate::StorageFailure::HistoryNotRetained
    #[must_use]
    pub const fn retains_history(&self) -> bool {
        matches!(
            self.scope,
            DocumentScope::Conversation {
                semantics: ConversationSemantics::Rewindable { .. },
                ..
            }
        )
    }

    /// Whether this incarnation is alive at `at`.
    #[must_use]
    pub fn is_alive_at(&self, at: Seq) -> bool {
        self.lifetime.contains(at)
    }
}

/// A document's scope, carrying the semantics only the scope that has them declares.
///
/// `spec.md:923-925`: *"Scope directly determines document ownership and lifetime. Only conversation
/// documents declare history and fork behavior."* That sentence is this enum: a session document has
/// no history field to set, so *"a session document declared rewindable"* is not a value.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "lowercase")]
pub enum DocumentScope {
    /// Owned by the Session. Current-only; reopening does not retire it (`spec.md:1066-1067`).
    Session,
    /// Owned by one conversation, with its history and fork policy.
    Conversation {
        /// The conversation.
        conversation_id: ConversationId,
        /// Its history and fork policy.
        semantics: ConversationSemantics,
    },
    /// Owned by one task. Current-only, never copied by a fork, retired when the task becomes
    /// terminal (`spec.md:1070-1071`).
    Task {
        /// The task.
        task_id: TaskId,
    },
}

impl DocumentScope {
    /// This scope without its semantics: the part an address carries.
    #[must_use]
    pub const fn as_ref(&self) -> ScopeRef {
        match self {
            Self::Session => ScopeRef::Session,
            Self::Conversation {
                conversation_id, ..
            } => ScopeRef::Conversation(*conversation_id),
            Self::Task { task_id } => ScopeRef::Task(*task_id),
        }
    }
}

/// A conversation document's history and fork policy (`spec.md:928-937`).
///
/// **This is the enum that makes `fork: "asOf"` without `history: "rewindable"` unspellable**, which
/// `spec.md:1068-1069` states as a validation rule and ADR-0030 §2.3 classifies `unrepresentable`.
/// The mechanism is that the two histories carry *different fork types*: [`LatestFork`] has no `AsOf`
/// variant to name. A derived `Deserialize` cannot forge it either — a stored
/// `{"history":"latest","fork":"asOf"}` is an unknown-variant error, which the backend reports as
/// [`Corruption::RecordMalformed`].
///
/// ADR-0030 §2.3 scopes the claim to the token and calls the record's half **checked**. The record's
/// half is lifted here too, and the difference is worth stating precisely: what remains checked is
/// that the record *agrees with the caller's token* (S5), not that the record is internally coherent.
///
/// [`Corruption::RecordMalformed`]: crate::Corruption::RecordMalformed
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "history", rename_all = "lowercase")]
pub enum ConversationSemantics {
    /// Current-only. Reclaimable after a committed base or retirement (`spec.md:4357-4358`).
    Latest {
        /// What a fork does with it.
        fork: LatestFork,
    },
    /// History retained, so a numeric read inside the lifetime reconstructs the selected value.
    Rewindable {
        /// What a fork does with it.
        fork: RewindableFork,
    },
}

impl ConversationSemantics {
    /// Whether a numeric read of this incarnation can be answered.
    #[must_use]
    pub const fn retains_history(self) -> bool {
        matches!(self, Self::Rewindable { .. })
    }

    /// The fork policy, as one value across both histories.
    ///
    /// Widening [`LatestFork`] into [`RewindableFork`] is sound in this one direction — every latest
    /// fork policy is also a rewindable one — and it exists so a fork implementation (PICO5-PLAN S9)
    /// matches on three arms once instead of on two enums.
    #[must_use]
    pub const fn fork(self) -> RewindableFork {
        match self {
            Self::Latest {
                fork: LatestFork::Current,
            }
            | Self::Rewindable {
                fork: RewindableFork::Current,
            } => RewindableFork::Current,
            Self::Latest {
                fork: LatestFork::Initial,
            }
            | Self::Rewindable {
                fork: RewindableFork::Initial,
            } => RewindableFork::Initial,
            Self::Rewindable {
                fork: RewindableFork::AsOf,
            } => RewindableFork::AsOf,
        }
    }
}

/// What a fork does with a `history: "latest"` conversation document (`spec.md:924-928`).
///
/// Two variants, and the absent third is the point: a current-only document has no history to fork
/// *as of* an entry, so [`RewindableFork::AsOf`] has no counterpart here.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LatestFork {
    /// The child starts from the parent's current value.
    Current,
    /// The child starts from the definition's initial value.
    Initial,
}

/// What a fork does with a `history: "rewindable"` conversation document (`spec.md:930-936`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RewindableFork {
    /// The child starts from the parent's value **at the fork point**. Needs retained history, which
    /// is why this variant exists only here.
    AsOf,
    /// The child starts from the parent's current value.
    Current,
    /// The child starts from the definition's initial value.
    Initial,
}

/// A scope without its semantics: where a document lives.
///
/// `spec.md:4229-4234`'s `DocumentAddress` carries `DocumentRecord["scope"]`, which in TypeScript is
/// the scope union *including* history and fork. It is a separate, smaller type here because an
/// address is a **location** and resolving one must not depend on the policy stored at it — otherwise
/// a caller presenting the wrong policy would silently fail to find a document that is there, which is
/// the disappearance `spec.md:1117-1120` exists to prevent.
///
/// The wire form is externally tagged — `"session"`, `{"conversation":7}`, `{"task":4}` — rather than
/// internally tagged like [`DocumentScope`], because serde cannot internally tag a newtype variant
/// whose payload is a number, and an address's scope *is* a bare id. The alternative, a struct variant
/// with one field, would buy nothing: this type is cyrup's own persisted shape, not pi's.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeRef {
    /// The Session's own scope.
    Session,
    /// One conversation's scope.
    Conversation(ConversationId),
    /// One task's scope.
    Task(TaskId),
}

impl fmt::Display for ScopeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session => f.write_str("session"),
            Self::Conversation(c) => write!(f, "conversation {c}"),
            Self::Task(t) => write!(f, "task {t}"),
        }
    }
}

/// One exact logical document address (`spec.md:4229-4234`).
///
/// `spec.md:4340-4342`: *"`findDocument()` resolves one exact logical kind/scope/key address at
/// current or historical membership. A missing key means the singleton, not every family member."*
/// The `Option<String>` key is that distinction, and it is why this type is not three strings.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct DocumentAddress {
    /// The definition kind.
    pub kind: Kind,
    /// Where it lives.
    pub scope: ScopeRef,
    /// The family key, or `None` for the singleton at this kind and scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl fmt::Display for DocumentAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} in {}", self.kind, self.scope)?;
        if let Some(key) = &self.key {
            write!(f, " keyed {key:?}")?;
        }
        Ok(())
    }
}

/// A document record without the fields storage stamps (`spec.md:1088-1092`).
///
/// *"`DocumentCreate` is not another persisted record; it is the same scoped union without
/// storage-assigned lifetime fields"* (`spec.md:1119-1120`). The absent field is [`Lifetime`], and its
/// absence is the guarantee: a caller cannot propose when its own document was created, so
/// `createdAt` cannot disagree with the commit that created it. [`DocumentCreate::stamp`] is the only
/// way to get a record, and it takes the sequence from the committing storage.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DocumentCreate {
    /// The incarnation to create. Minted by [`Storage::mint`](crate::StorageExt::mint).
    pub id: DocumentId,
    /// Its definition kind.
    pub kind: Kind,
    /// Its family key. Absent for a singleton.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Its scope, with the semantics the scope declares.
    pub scope: DocumentScope,
}

impl DocumentCreate {
    /// This creation's logical address, for the occupancy check.
    #[must_use]
    pub fn address(&self) -> DocumentAddress {
        DocumentAddress {
            kind: self.kind.clone(),
            scope: self.scope.as_ref(),
            key: self.key.clone(),
        }
    }

    /// Stamp this creation with the commit sequence, producing the persisted record.
    ///
    /// `retired` is `true` when the same batch retires the new incarnation, which
    /// `spec.md:4364-4365` stamps with the same sequence: *"Create plus retire stamps both lifetime
    /// bounds with the batch sequence"*, giving the empty lifetime `spec.md:1113` calls legal.
    ///
    /// Infallible by construction: a lifetime whose bounds are the same sequence is empty, not
    /// inverted, so there is no error arm to handle and no way for a caller to produce one.
    #[must_use]
    pub fn stamp(self, at: Seq, retired: bool) -> DocumentRecord {
        DocumentRecord {
            id: self.id,
            kind: self.kind,
            key: self.key,
            lifetime: if retired {
                Lifetime::retired_at_creation(at)
            } else {
                Lifetime::open(at)
            },
            scope: self.scope,
        }
    }
}
