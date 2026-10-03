//! Document **definitions, tokens and typed addresses** (`spec.md:921-1065`).
//!
//! PICO5-PLAN S5's first deliverable. A definition is a Rust type implementing [`DocDef`]; a
//! [`DocToken`] is that definition with its kind and version **parsed once**; a [`DocAddress`] is one
//! exact logical document, built by the one constructor its definition's scope and shape allow.
//!
//! # The six constructors are `spec.md:1187-1192`'s six overloads
//!
//! Upstream declares `tx.doc()` six times — session/conversation/task × singleton/family — and relies
//! on TypeScript overload resolution to keep a `TaskDocToken` from being handed a `ConversationId`.
//! Here the six are six **named constructors**, each in an `impl` block bounded on
//! `D: DocDef<Place = .., Shape = ..>`:
//!
//! | `spec.md` overload | constructor | what the caller supplies |
//! |---|---|---|
//! | `doc(SessionDocToken)` | [`DocToken::at`] | nothing |
//! | `doc(SessionDocFamilyToken, key, seed)` | [`DocToken::at_key`] | a [`FamilyKey`] |
//! | `doc(ConversationDocToken, conversationId)` | [`DocToken::in_conversation`] | a `ConversationId` |
//! | `doc(ConversationDocFamilyToken, conversationId, key, seed)` | [`DocToken::in_conversation_keyed`] | both |
//! | `doc(TaskDocToken, taskId)` | [`DocToken::on_task`] | a `TaskId` |
//! | `doc(TaskDocFamilyToken, taskId, key, seed)` | [`DocToken::on_task_keyed`] | both |
//!
//! A definition's `Place` and `Shape` select exactly one of the six, so *"scope-preserving token
//! overloads require callers to supply only the concrete conversation/task ID and, for a family, its
//! key and creation seed"* (`spec.md:1222-1224`) is a property of the method set rather than a rule
//! overload resolution happens to enforce. Three compile-fail cases pin it.
//!
//! # Why the address and the scope are one value
//!
//! [`DocAddress`] carries **both** [`DocumentAddress`] (kind + `ScopeRef` + optional key) and
//! [`DocumentScope`] (the same scope *with* its conversation history and fork policy), and both are
//! written by the same constructor from the same argument. S4 needed a runtime check that the two
//! agreed — `TxError::ScopeDisagreesWithAddress`, now deleted — because a caller assembled them
//! separately. Here a disagreement has no spelling, which is the direction ADR-0030 §2.3's row asks
//! for: *"unrepresentable in the token"*.
//!
//! What stays **checked** is the other half of that row — whether the token agrees with the
//! *persisted record* — and `spec.md:1063-1065` is why it must stay a check:
//! [`TxError::TokenDisagreesWithRecord`] is raised against the record's own scope, because extension
//! code reloads independently of its data and unaccessed data must survive its code's absence.
//!
//! # `fork: "asOf"` without `history: "rewindable"`
//!
//! Not spelled here at all: [`Placement::Policy`] for [`ConversationScoped`] *is*
//! [`ConversationSemantics`], whose [`Rewindable`] arm is the only one carrying a `RewindableFork`,
//! and whose [`Latest`] arm carries a `LatestFork` that has no `AsOf` variant to name
//! (`cyrup-pico-store`, PICO5-PLAN S1). The token half of that guarantee is therefore the store's
//! enum reused, not a second encoding of the same rule.
//!
//! For [`SessionScoped`] and [`TaskScoped`] the policy is `()`: *"only conversation documents declare
//! history and fork behavior"* (`spec.md:923-925`), so *"a session document declared rewindable"* is
//! not a value either.
//!
//! [`Rewindable`]: ConversationSemantics::Rewindable
//! [`Latest`]: ConversationSemantics::Latest
//! [`TxError::TokenDisagreesWithRecord`]: crate::TxError::TokenDisagreesWithRecord

use core::fmt;
use core::marker::PhantomData;
use core::num::NonZeroU32;
use std::sync::Arc;

use cyrup_pico_doc::{CheckpointInput, DefVersion, DocRoot};
use cyrup_pico_store::{
    ConversationId, ConversationSemantics, DocumentAddress, DocumentScope, Kind, NotAKind, TaskId,
};
use serde::{Serialize, Serializer};

mod private {
    /// Seals [`super::Placement`] and [`super::Shape`]. Outside this crate neither trait can be
    /// implemented, so the three placements and the two shapes are closed sets — `spec.md:927-943`
    /// names exactly three scopes and `spec.md:1187-1192` exactly two shapes, and a fourth of either
    /// is a storage-format event rather than an additive change.
    pub trait Sealed {}
}

// ---------------------------------------------------------------------------------------------
// Placement: which scope a definition lives in, and what that scope alone may declare.
// ---------------------------------------------------------------------------------------------

/// Where a definition's documents live (`spec.md:927-943`).
///
/// Sealed, with exactly three implementors. The one associated type is [`Placement::Policy`], which
/// is the mechanism behind `spec.md:923-925`'s *"only conversation documents declare history and fork
/// behavior"*: it is `()` for two of the three.
pub trait Placement: private::Sealed + 'static {
    /// What a definition in this scope declares **beyond** the scope itself.
    ///
    /// [`ConversationSemantics`] for [`ConversationScoped`], `()` for the other two. A
    /// [`DocDef::POLICY`] is of this type, so a session-scoped definition has no history field to
    /// set and no fork field to disagree with it.
    type Policy: Copy + fmt::Debug;
}

/// Owned by the Session; current-only. Reopening does not retire it (`spec.md:1066-1067`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SessionScoped;

/// Owned by one conversation, with the history and fork policy that scope declares.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ConversationScoped;

/// Owned by one task; current-only, never copied by a fork (`spec.md:1070-1071`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TaskScoped;

impl private::Sealed for SessionScoped {}
impl Placement for SessionScoped {
    type Policy = ();
}

impl private::Sealed for ConversationScoped {}
impl Placement for ConversationScoped {
    type Policy = ConversationSemantics;
}

impl private::Sealed for TaskScoped {}
impl Placement for TaskScoped {
    type Policy = ();
}

// ---------------------------------------------------------------------------------------------
// Shape: singleton or family.
// ---------------------------------------------------------------------------------------------

/// Whether a definition names one document per scope or a keyed family (`spec.md:1116-1117`).
///
/// Sealed, with exactly two implementors, and it is the half that makes `spec.md:4341-4342`'s *"a
/// missing key means the singleton, not every family member"* a typing rule: a [`Family`]
/// definition's only address constructors take a [`FamilyKey`], so a keyless family address cannot
/// be written.
pub trait Shape: private::Sealed + 'static {}

/// One document per scope, addressed by kind and scope alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Singleton;

/// A keyed family: one document per (kind, scope, key).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Family;

impl private::Sealed for Singleton {}
impl Shape for Singleton {}

impl private::Sealed for Family {}
impl Shape for Family {}

// ---------------------------------------------------------------------------------------------
// FamilyKey
// ---------------------------------------------------------------------------------------------

/// The longest family key this kernel accepts, in bytes.
///
/// A limit exists because the key is persisted and indexed, and because a damaged record can claim
/// any length. The number is a budget, not a domain fact; it is [`cyrup_pico_store::MAX_KIND_LEN`]'s
/// twin for the same reason.
pub const MAX_FAMILY_KEY_LEN: usize = 128;

/// One family member's key (`spec.md:1079`, `:1115-1117`).
///
/// A newtype with a parse rather than a `String`, for exactly [`Kind`]'s reasons: the value is
/// persisted, and a bare `String` admits the empty key and a key containing `\n` — which a JSONL
/// backend writes as a record boundary (PICO5-PLAN S7). `Arc<str>` because a key is compared and
/// cloned into address keys far more often than it is built.
///
/// The guarantee is **guarded**, not unrepresentable, and the limit is worth stating:
/// [`DocumentAddress::key`] is an `Option<String>` the storage layer will accept anything in, so this
/// parse binds the typed acquisition path and not a backend reading a damaged file.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FamilyKey(Arc<str>);

/// Why a string is not a [`FamilyKey`].
///
/// `Serialize` only (ADR-0030 §10's rule for diagnostics): a rejection must be loggable without
/// becoming a construction path.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, thiserror::Error)]
pub enum NotAFamilyKey {
    /// The empty string. A key names one family member; nothing is named by nothing — and the empty
    /// key would be indistinguishable from the singleton in a human-readable address.
    #[error("a family key cannot be empty")]
    Empty,
    /// Longer than [`MAX_FAMILY_KEY_LEN`] bytes.
    #[error("a family key is at most {max} bytes, found {found}")]
    TooLong {
        /// How long the candidate was.
        found: usize,
        /// The limit.
        max: usize,
    },
    /// A control character. A newline is the one that matters; the whole range is rejected because
    /// no key legitimately contains one.
    #[error("a family key cannot contain a control character (found U+{code:04X})")]
    ControlCharacter {
        /// The offending code point.
        code: u32,
    },
}

impl FamilyKey {
    /// Parse a family key.
    ///
    /// # Errors
    /// [`NotAFamilyKey`] for the empty string, a key past [`MAX_FAMILY_KEY_LEN`], or a control
    /// character.
    pub fn parse(s: &str) -> Result<Self, NotAFamilyKey> {
        if s.is_empty() {
            return Err(NotAFamilyKey::Empty);
        }
        if s.len() > MAX_FAMILY_KEY_LEN {
            return Err(NotAFamilyKey::TooLong {
                found: s.len(),
                max: MAX_FAMILY_KEY_LEN,
            });
        }
        if let Some(c) = s.chars().find(|c| c.is_control()) {
            return Err(NotAFamilyKey::ControlCharacter { code: c as u32 });
        }
        Ok(Self(Arc::from(s)))
    }

    /// The key as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FamilyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for FamilyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl Serialize for FamilyKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

// ---------------------------------------------------------------------------------------------
// Migration
// ---------------------------------------------------------------------------------------------

/// What a migration callback is given (`spec.md:1441-1444`).
///
/// # Why there is no way to build one
///
/// The fields are private and [`Migration::new`] is crate-private, so a `Migration` exists only on
/// the acquisition path that read the stored value. That is how `spec.md:1447-1449`'s *"migration is
/// access-driven"* resists the shape pi had to warn against — a sweep at open — rather than being a
/// convention: there is no `Migration` to pass to a sweep.
///
/// # Why `stored` is a `DocRoot` and not `D::Value`
///
/// There is no older `Value` type to deserialise into: the stored shape is whatever the older
/// definition wrote, which is why `spec.md:954` types the parameter `JsonObject`. A [`DocRoot`] is
/// that object, immutable and already detached — so *"migration always starts from a detached stored
/// value"* and *"migration callbacks never receive a live tracker revision"* (`spec.md:1346-1347`,
/// `:1455`) are both consequences of the type rather than rules, which is ADR-0030 §2.3's
/// *"`Arc<DocValue>` makes 'a migration received a live revision' unrepresentable"*.
#[derive(Clone, Copy, Debug)]
pub struct Migration<'a> {
    stored: &'a DocRoot,
    from: DefVersion,
    to: DefVersion,
}

impl<'a> Migration<'a> {
    /// Crate-private: only the acquisition path, holding a value it has just read, can build one.
    pub(crate) const fn new(stored: &'a DocRoot, from: DefVersion, to: DefVersion) -> Self {
        Self { stored, from, to }
    }

    /// The stored value, as the older definition wrote it.
    #[must_use]
    pub const fn stored(&self) -> &'a DocRoot {
        self.stored
    }

    /// The version the stored value was written by.
    #[must_use]
    pub const fn from(&self) -> DefVersion {
        self.from
    }

    /// The version to produce. Always the token's own version.
    #[must_use]
    pub const fn to(&self) -> DefVersion {
        self.to
    }
}

/// A definition's migration callback.
///
/// A plain `fn` pointer rather than a boxed closure, because a definition is a *type*: there is
/// nothing per-instance to capture, and [`DocDef::MIGRATE`] can then be an associated `const` whose
/// `None` is the *same fact* as `spec.md:1444`'s *"no migrate -> reject older stored version"*. A
/// separate `HAS_MIGRATION` flag could disagree with the callback's presence; an `Option` cannot.
pub type MigrateFn<D> = fn(Migration<'_>) -> Result<<D as DocDef>::Value, MigrationFailed>;

/// A migration callback refused the stored value.
///
/// `spec.md:1459-1460`: *"callback failure persists nothing"*. The transaction rolls back before
/// storage admission, so this is one of `G-PRE-ADMISSION-ROLLBACK`'s arms and never a poison.
#[derive(Debug, thiserror::Error)]
#[error("a document migration callback refused the stored value")]
pub struct MigrationFailed(#[source] Box<dyn core::error::Error + Send + Sync>);

impl MigrationFailed {
    /// Wrap a callback's own error.
    #[must_use]
    pub fn new(cause: impl Into<Box<dyn core::error::Error + Send + Sync>>) -> Self {
        Self(cause.into())
    }
}

// ---------------------------------------------------------------------------------------------
// DocDef
// ---------------------------------------------------------------------------------------------

/// One document definition (`spec.md:950-957`).
///
/// A type rather than a value, because every field upstream marks `readonly` is a compile-time fact
/// here and the three callbacks capture nothing. What survives as a value is the [`DocToken`], which
/// is this definition with its [`KIND`](DocDef::KIND) and [`VERSION`](DocDef::VERSION) **parsed**.
///
/// # The one place validation happens
///
/// `spec.md:1062` and `:1064` list the definition validation rules. Two of them are shape rules on
/// strings and numbers — *"versions are positive integers"*, and a kind is a kind — and both are
/// checked exactly once, in [`DocToken::define`]. Downstream of that constructor the kind is a
/// [`Kind`] and the version is a [`DefVersion`], so nothing re-derives either test.
///
/// # What is deliberately *not* an associated type
///
/// `Self::Value` carries `Serialize` and **not** `DeserializeOwned`. The typed *write* path needs
/// `Serialize`, to turn [`initial`](DocDef::initial)'s and a migration's result into a [`DocRoot`];
/// the typed *read* path is PICO5-PLAN S6's `Revision::typed()`, and asking for the bound before the
/// method exists would force every definition to satisfy a requirement nothing uses.
pub trait DocDef: 'static {
    /// The typed value this definition's documents hold.
    ///
    /// `spec.md:1065`: *"`initial()` and `migrate()` return JSON objects"* — enforced where the value
    /// crosses into the kernel, by [`DocRoot::parse`], which rejects a non-object root.
    type Value: Serialize + 'static;

    /// Which scope these documents live in. One of [`SessionScoped`], [`ConversationScoped`],
    /// [`TaskScoped`].
    type Place: Placement;

    /// [`Singleton`] or [`Family`].
    type Shape: Shape;

    /// What a creation seed carries.
    ///
    /// `()` for a [`Singleton`]: `spec.md:1237` calls the token's `initial()` with no argument,
    /// because nothing names a member there, and `()` is the type that carries no information. A
    /// [`Family`]'s seed is `spec.md:1238`'s `initial(seed)` argument.
    type Seed;

    /// The stable definition kind. Parsed by [`DocToken::define`], which also refuses the reserved
    /// namespace (see [`cyrup_pico_store::RESERVED_PREFIX`]).
    const KIND: &'static str;

    /// The definition version.
    ///
    /// A `u32` here and a [`DefVersion`] on the token, because `spec.md:1062`'s *"versions are
    /// positive integers"* is a validation rule and [`DocToken::define`] is where validation runs.
    /// Writing `const VERSION: DefVersion` instead would push a `NonZeroU32` construction into every
    /// definition, where the only panic-free spelling is a `match` on an `Option`.
    const VERSION: u32;

    /// This definition's scope policy: a [`ConversationSemantics`] for a conversation document, `()`
    /// otherwise.
    const POLICY: <Self::Place as Placement>::Policy;

    /// The migration callback, or `None` for `spec.md:1444`'s *"no migrate"* arm.
    const MIGRATE: Option<MigrateFn<Self>> = None;

    /// The checkpoint predicate (`spec.md:1388-1390`).
    ///
    /// `None` and a predicate returning `false` are the *same* decision — `spec.md:1392` spells it
    /// `?? false` — so there is nothing to distinguish and no second flag to keep in step.
    /// [`CheckpointInput`] carries no storage handle, which is how *"evaluation performs no Storage
    /// read"* becomes a property of the type (ADR-0030 §2.3).
    const CHECKPOINT_WHEN: Option<fn(CheckpointInput<'_>) -> bool> = None;

    /// The value a missing document is created with (`spec.md:1230-1232`).
    ///
    /// Called **only** when the logical address is empty. The typed acquisition takes the seed by
    /// value and passes it here inside the absent branch, so *"the first call's detached seed wins
    /// and later seeds are ignored"* (`spec.md:1232-1234`) holds because a later seed is moved into a
    /// call that never happens.
    fn initial(seed: Self::Seed) -> Self::Value;
}

// ---------------------------------------------------------------------------------------------
// DocToken
// ---------------------------------------------------------------------------------------------

/// Why a [`DocDef`] is not a definition.
///
/// `Serialize` only, per ADR-0030 §10's diagnostics rule.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, thiserror::Error)]
pub enum NotADefinition {
    /// [`DocDef::KIND`] is not a [`Kind`].
    #[error("a document definition's kind is not a valid record kind")]
    Kind(#[from] NotAKind),
    /// [`DocDef::KIND`] is in the kernel's reserved namespace.
    ///
    /// ADR-0030 §2.2's last row: *"a `Kind` newtype parsed at registration that rejects the reserved
    /// namespace for third parties; built-ins through a crate-private path"*. This is that
    /// rejection, at the only boundary where it can be made — `cyrup-pico-store` deferred it here
    /// because deciding whether a definition is third-party needs the defining party, and the
    /// kernel's own built-ins must still be able to write reserved kinds.
    #[error("the kind {kind} is in the reserved namespace {prefix}")]
    ReservedKind {
        /// The offending kind.
        kind: Kind,
        /// The prefix it claimed.
        prefix: &'static str,
    },
    /// [`DocDef::VERSION`] is zero. `spec.md:1062`: *"versions are positive integers"*.
    #[error("a document definition's version must be a positive integer, found 0")]
    VersionIsZero,
}

/// A definition with its kind and version parsed: the value `spec.md:1219-1221`'s `tx.doc()` takes.
///
/// Built once per definition and then shared. Cheap to clone — a [`Kind`] is an `Arc<str>` — and
/// `Send + Sync` regardless of `D`, because `PhantomData<fn() -> D>` is.
pub struct DocToken<D: DocDef> {
    kind: Kind,
    version: DefVersion,
    _def: PhantomData<fn() -> D>,
}

impl<D: DocDef> Clone for DocToken<D> {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind.clone(),
            version: self.version,
            _def: PhantomData,
        }
    }
}

impl<D: DocDef> fmt::Debug for DocToken<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocToken")
            .field("kind", &self.kind)
            .field("version", &self.version)
            .finish()
    }
}

impl<D: DocDef> DocToken<D> {
    /// Parse a definition into a token.
    ///
    /// The one validation boundary: [`DocDef::KIND`] must be a [`Kind`] outside the reserved
    /// namespace, and [`DocDef::VERSION`] must be positive.
    ///
    /// # Errors
    /// [`NotADefinition`].
    pub fn define() -> Result<Self, NotADefinition> {
        let token = Self::parse()?;
        if token.kind.is_reserved() {
            return Err(NotADefinition::ReservedKind {
                kind: token.kind,
                prefix: cyrup_pico_store::RESERVED_PREFIX,
            });
        }
        Ok(token)
    }

    /// Parse a **kernel** definition, which may claim the reserved namespace.
    ///
    /// Crate-private, and that privacy is the whole of ADR-0030 §2.2's *"built-ins through a
    /// crate-private path"*: an application cannot reach this function, so it cannot define a kind
    /// the kernel's own replay logic would interpret.
    ///
    /// # Errors
    /// [`NotADefinition::Kind`], [`NotADefinition::VersionIsZero`].
    //
    // The first production caller is the built-in document definitions, which `spec.md:1079-1078`
    // defers — *"concrete built-in document grouping and semantics are declared when the built-in
    // definitions are implemented"* — and which ADR-0029 puts outside this build's scope. The path
    // ships now because the privacy boundary is the guarantee: adding it later means deciding it
    // later, and `src/tests/defs.rs` is what keeps it honest in the meantime.
    #[allow(dead_code)]
    pub(crate) fn define_builtin() -> Result<Self, NotADefinition> {
        Self::parse()
    }

    fn parse() -> Result<Self, NotADefinition> {
        let kind = Kind::parse(D::KIND)?;
        let Some(version) = NonZeroU32::new(D::VERSION) else {
            return Err(NotADefinition::VersionIsZero);
        };
        Ok(Self {
            kind,
            version: DefVersion::new(version),
            _def: PhantomData,
        })
    }

    /// This definition's kind.
    #[must_use]
    pub const fn kind(&self) -> &Kind {
        &self.kind
    }

    /// This definition's version, as the kernel's own type.
    #[must_use]
    pub const fn version(&self) -> DefVersion {
        self.version
    }

    fn address(&self, scope: DocumentScope, key: Option<FamilyKey>) -> DocAddress<D> {
        DocAddress {
            address: DocumentAddress {
                kind: self.kind.clone(),
                scope: scope.as_ref(),
                key: key.as_ref().map(|k| k.as_str().to_owned()),
            },
            scope,
            version: self.version,
            _def: PhantomData,
        }
    }
}

// The six constructors. Each one is `spec.md:1187-1192`'s overload of the same number, and each is
// reachable only for the definition shape that overload accepts.

impl<D: DocDef<Place = SessionScoped, Shape = Singleton>> DocToken<D> {
    /// The Session's singleton of this kind.
    #[must_use]
    pub fn at(&self) -> DocAddress<D> {
        self.address(DocumentScope::Session, None)
    }
}

impl<D: DocDef<Place = SessionScoped, Shape = Family>> DocToken<D> {
    /// One Session-scoped family member.
    #[must_use]
    pub fn at_key(&self, key: FamilyKey) -> DocAddress<D> {
        self.address(DocumentScope::Session, Some(key))
    }
}

impl<D: DocDef<Place = ConversationScoped, Shape = Singleton>> DocToken<D> {
    /// One conversation's singleton of this kind.
    #[must_use]
    pub fn in_conversation(&self, conversation_id: ConversationId) -> DocAddress<D> {
        self.address(
            DocumentScope::Conversation {
                conversation_id,
                semantics: D::POLICY,
            },
            None,
        )
    }
}

impl<D: DocDef<Place = ConversationScoped, Shape = Family>> DocToken<D> {
    /// One conversation-scoped family member.
    #[must_use]
    pub fn in_conversation_keyed(
        &self,
        conversation_id: ConversationId,
        key: FamilyKey,
    ) -> DocAddress<D> {
        self.address(
            DocumentScope::Conversation {
                conversation_id,
                semantics: D::POLICY,
            },
            Some(key),
        )
    }
}

impl<D: DocDef<Place = TaskScoped, Shape = Singleton>> DocToken<D> {
    /// One task's singleton of this kind.
    #[must_use]
    pub fn on_task(&self, task_id: TaskId) -> DocAddress<D> {
        self.address(DocumentScope::Task { task_id }, None)
    }
}

impl<D: DocDef<Place = TaskScoped, Shape = Family>> DocToken<D> {
    /// One task-scoped family member.
    #[must_use]
    pub fn on_task_keyed(&self, task_id: TaskId, key: FamilyKey) -> DocAddress<D> {
        self.address(DocumentScope::Task { task_id }, Some(key))
    }
}

// ---------------------------------------------------------------------------------------------
// DocAddress
// ---------------------------------------------------------------------------------------------

/// One exact logical document, with the policy its definition declared.
///
/// `spec.md:4340-4342`: *"`findDocument()` resolves one exact logical kind/scope/key address at
/// current or historical membership. A missing key means the singleton, not every family member."*
/// The `key` here is `None` for a [`Singleton`] and `Some` for a [`Family`], and which one it is was
/// decided by the constructor the definition's [`Shape`] admitted — not by an argument a caller could
/// forget.
pub struct DocAddress<D: DocDef> {
    address: DocumentAddress,
    scope: DocumentScope,
    version: DefVersion,
    _def: PhantomData<fn() -> D>,
}

impl<D: DocDef> Clone for DocAddress<D> {
    fn clone(&self) -> Self {
        Self {
            address: self.address.clone(),
            scope: self.scope.clone(),
            version: self.version,
            _def: PhantomData,
        }
    }
}

impl<D: DocDef> PartialEq for DocAddress<D> {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address && self.scope == other.scope && self.version == other.version
    }
}

impl<D: DocDef> Eq for DocAddress<D> {}

impl<D: DocDef> fmt::Debug for DocAddress<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocAddress")
            .field("address", &self.address)
            .field("scope", &self.scope)
            .field("version", &self.version)
            .finish()
    }
}

impl<D: DocDef> fmt::Display for DocAddress<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.address, f)
    }
}

impl<D: DocDef> DocAddress<D> {
    /// The storage-level address: kind, `ScopeRef`, optional key.
    #[must_use]
    pub const fn address(&self) -> &DocumentAddress {
        &self.address
    }

    /// The full scope, **with** the conversation history and fork policy where there is one.
    ///
    /// This is what [`Tx::doc`](crate::Tx::doc) compares against the persisted record, and what it
    /// persists on a creation.
    #[must_use]
    pub const fn scope(&self) -> &DocumentScope {
        &self.scope
    }

    /// The definition version the token speaks.
    #[must_use]
    pub const fn version(&self) -> DefVersion {
        self.version
    }

    /// The family key, or `None` for the singleton at this kind and scope.
    #[must_use]
    pub fn key(&self) -> Option<&str> {
        self.address.key.as_deref()
    }
}
