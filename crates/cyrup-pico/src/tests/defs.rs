//! The definitions the behaviour suite speaks, and the parser tests for the typed address itself.
//!
//! Every definition here is a *type*, which is what [`DocDef`] is: there is no registry and nothing
//! is registered (`spec.md:1224-1225`: *"ordinary definitions are not registered"*). The tokens are built
//! where they are used.

use core::sync::atomic::{AtomicU32, Ordering};

use cyrup_pico_doc::{CheckpointInput, DocRoot};
use cyrup_pico_store::{
    ConversationId, ConversationSemantics, Kind, LatestFork, ROOT_CONVERSATION_ID, RewindableFork,
    ScopeRef, TaskId,
};
use serde::Serialize;

use crate::{
    ConversationScoped, DocDef, DocToken, Family, FamilyKey, Migration, MigrationFailed,
    NotADefinition, NotAFamilyKey, SessionScoped, Singleton, TaskScoped,
};

/// A value whose JSON form is the empty object.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub(crate) struct Empty {}

/// A value that records the seed it was created from.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub(crate) struct Seeded {
    seed: u32,
}

/// `test.live` at version 1: a Session-scoped singleton, no migration, no checkpoint.
pub(crate) struct Live;

impl DocDef for Live {
    type Value = Empty;
    type Place = SessionScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.live";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

/// `test.live` at version **2**, migrating by keeping the stored value **exactly**.
///
/// The identity migration is the point: `spec.md:1463-1465` requires a base *"even when the migrated
/// JSON is deeply equal"*, and a migration that cannot possibly change anything is the sharpest way
/// to ask whether the base is written for the version transition or for the diff.
pub(crate) struct LiveV2;

impl DocDef for LiveV2 {
    type Value = DocRoot;
    type Place = SessionScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.live";
    const VERSION: u32 = 2;
    const POLICY: () = ();
    const MIGRATE: Option<crate::MigrateFn<Self>> = Some(keep);
    fn initial((): ()) -> DocRoot {
        DocRoot::empty()
    }
}

/// The identity migration. A free `fn`, because [`DocDef::MIGRATE`] is a pointer.
fn keep(m: Migration<'_>) -> Result<DocRoot, MigrationFailed> {
    Ok(m.stored().clone())
}

/// `test.live` at version 2 with **no** migration: `spec.md:1444`'s fourth ladder arm.
pub(crate) struct LiveV2NoMigration;

impl DocDef for LiveV2NoMigration {
    type Value = Empty;
    type Place = SessionScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.live";
    const VERSION: u32 = 2;
    const POLICY: () = ();
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

/// `test.live` at version 2 whose migration refuses: `spec.md:1459-1460`'s *"persists nothing"*.
pub(crate) struct LiveV2Refusing;

impl DocDef for LiveV2Refusing {
    type Value = DocRoot;
    type Place = SessionScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.live";
    const VERSION: u32 = 2;
    const POLICY: () = ();
    const MIGRATE: Option<crate::MigrateFn<Self>> = Some(refuse);
    fn initial((): ()) -> DocRoot {
        DocRoot::empty()
    }
}

fn refuse(_: Migration<'_>) -> Result<DocRoot, MigrationFailed> {
    Err(MigrationFailed::new("this shape cannot be migrated"))
}

/// `test.counter`: a Session-scoped **family** seeded with a number.
pub(crate) struct Counter;

impl DocDef for Counter {
    type Value = Seeded;
    type Place = SessionScoped;
    type Shape = Family;
    type Seed = u32;
    const KIND: &'static str = "test.counter";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial(seed: u32) -> Seeded {
        Seeded { seed }
    }
}

/// `test.other`: a second Session-scoped singleton, so the reopen case has a document whose
/// definition is *unavailable* to the reopening Session.
pub(crate) struct Other;

impl DocDef for Other {
    type Value = Empty;
    type Place = SessionScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.other";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

/// How many times [`Checkpointed`]'s predicate has run. `spec.md:1395`: *"exactly once"*.
pub(crate) static CHECKPOINT_CALLS: AtomicU32 = AtomicU32::new(0);

/// The `deltas_since_base` the predicate last saw, so a test can check
/// `spec.md:1396-1399`'s *"excluding the change being evaluated"*.
pub(crate) static CHECKPOINT_SAW: AtomicU32 = AtomicU32::new(u32::MAX);

/// `test.checkpointed`: a Session-scoped singleton that checkpoints on the second delta.
pub(crate) struct Checkpointed;

impl DocDef for Checkpointed {
    type Value = Empty;
    type Place = SessionScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.checkpointed";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    const CHECKPOINT_WHEN: Option<fn(CheckpointInput<'_>) -> bool> = Some(on_second_delta);
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

fn on_second_delta(input: CheckpointInput<'_>) -> bool {
    CHECKPOINT_CALLS.fetch_add(1, Ordering::SeqCst);
    CHECKPOINT_SAW.store(input.deltas_since_base, Ordering::SeqCst);
    input.deltas_since_base >= 2
}

/// `test.chatter` as a `history: "latest"` conversation document.
pub(crate) struct Chatter;

impl DocDef for Chatter {
    type Value = Empty;
    type Place = ConversationScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.chatter";
    const VERSION: u32 = 1;
    const POLICY: ConversationSemantics = ConversationSemantics::Latest {
        fork: LatestFork::Current,
    };
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

/// The **same kind** as [`Chatter`], redeclared `history: "rewindable"`.
///
/// This is `spec.md:1063-1065`'s hazard in one type: extension code reloaded with a changed policy.
/// The record wins, which is what `a_token_that_redeclares_history_is_refused` asserts.
pub(crate) struct ChatterRewindable;

impl DocDef for ChatterRewindable {
    type Value = Empty;
    type Place = ConversationScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.chatter";
    const VERSION: u32 = 1;
    const POLICY: ConversationSemantics = ConversationSemantics::Rewindable {
        fork: RewindableFork::AsOf,
    };
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

/// `test.pinned`: a task-scoped family, used only by the address tests and the compile-fail cases.
pub(crate) struct Pinned;

impl DocDef for Pinned {
    type Value = Empty;
    type Place = TaskScoped;
    type Shape = Family;
    type Seed = ();
    const KIND: &'static str = "test.pinned";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

/// `test.chatter.keyed`: a conversation-scoped **family**, for the sixth constructor.
pub(crate) struct CounterInConversation;

impl DocDef for CounterInConversation {
    type Value = Empty;
    type Place = ConversationScoped;
    type Shape = Family;
    type Seed = ();
    const KIND: &'static str = "test.chatter.keyed";
    const VERSION: u32 = 1;
    const POLICY: ConversationSemantics = ConversationSemantics::Latest {
        fork: LatestFork::Current,
    };
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

/// `test.pinned.one`: a task-scoped singleton, for the fifth constructor.
pub(crate) struct PinnedSingleton;

impl DocDef for PinnedSingleton {
    type Value = Empty;
    type Place = TaskScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.pinned.one";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

/// A definition claiming the kernel's reserved namespace.
struct Builtin;

impl DocDef for Builtin {
    type Value = Empty;
    type Place = SessionScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "cyrup.live";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

/// A definition whose version is zero. `spec.md:1062`: *"versions are positive integers."*
struct Unversioned;

impl DocDef for Unversioned {
    type Value = Empty;
    type Place = SessionScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.unversioned";
    const VERSION: u32 = 0;
    const POLICY: () = ();
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

/// A definition whose kind is not a kind at all.
struct Nameless;

impl DocDef for Nameless {
    type Value = Empty;
    type Place = SessionScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

// ---------------------------------------------------------------------------------------------
// Token parsing: the one validation boundary.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_definition_is_parsed_once_into_a_kind_and_a_version() {
    let token = DocToken::<Live>::define().expect("test.live is a definition");
    assert_eq!(token.kind().as_str(), "test.live");
    assert_eq!(token.version().get(), 1);
}

#[test]
fn a_zero_version_is_not_a_definition() {
    assert!(matches!(
        DocToken::<Unversioned>::define(),
        Err(NotADefinition::VersionIsZero)
    ));
}

#[test]
fn a_kind_that_is_not_a_kind_is_not_a_definition() {
    assert!(matches!(
        DocToken::<Nameless>::define(),
        Err(NotADefinition::Kind(_))
    ));
}

/// ADR-0030 §2.2's last row: the reserved namespace is refused to third parties *"and built-ins
/// through a crate-private path"*. Both halves, in one case, because either alone would pass while
/// the row was half-implemented.
#[test]
fn the_reserved_namespace_is_refused_to_a_definition_and_open_to_the_kernel() {
    let refused = DocToken::<Builtin>::define();
    assert!(
        matches!(refused, Err(NotADefinition::ReservedKind { .. })),
        "an application definition cannot claim the reserved prefix"
    );
    let kernel = DocToken::<Builtin>::define_builtin().expect("the kernel's own path accepts it");
    assert_eq!(kernel.kind().as_str(), "cyrup.live");
    assert!(kernel.kind().is_reserved());
}

// ---------------------------------------------------------------------------------------------
// FamilyKey parsing.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_family_key_is_parsed_at_the_boundary() {
    assert_eq!(
        FamilyKey::parse("alpha").expect("a key").as_str(),
        "alpha",
        "an ordinary key survives unchanged"
    );
    assert!(matches!(FamilyKey::parse(""), Err(NotAFamilyKey::Empty)));
    assert!(matches!(
        FamilyKey::parse("one\ntwo"),
        Err(NotAFamilyKey::ControlCharacter { code: 0x0A })
    ));
    let long = "k".repeat(crate::MAX_FAMILY_KEY_LEN + 1);
    assert!(matches!(
        FamilyKey::parse(&long),
        Err(NotAFamilyKey::TooLong { .. })
    ));
}

// ---------------------------------------------------------------------------------------------
// The typed address: singleton versus keyless, and scope coherence.
// ---------------------------------------------------------------------------------------------

/// `spec.md:4341-4342`: *"a missing key means the singleton, not every family member."*
///
/// The two addresses carry the **same** kind and the **same** scope and are still different
/// addresses, which is the whole content of that sentence — and the reason [`DocumentAddress`]'s key
/// is an `Option<String>` rather than a `String` that happens to be empty for singletons.
///
/// [`DocumentAddress`]: cyrup_pico_store::DocumentAddress
#[test]
fn a_missing_key_is_the_singleton_and_not_a_family_member() {
    let live = DocToken::<Live>::define().expect("a definition");
    let counter = DocToken::<Counter>::define().expect("a definition");

    let singleton = live.at();
    assert_eq!(singleton.key(), None);

    let member = counter.at_key(FamilyKey::parse("alpha").expect("a key"));
    assert_eq!(member.key(), Some("alpha"));

    // Same scope, different kinds, so compare the keys at one kind by building a second member.
    let other_member = counter.at_key(FamilyKey::parse("beta").expect("a key"));
    assert_ne!(member.address(), other_member.address());
    assert_eq!(
        member.address().kind,
        other_member.address().kind,
        "two members of one family share its kind"
    );
    assert_eq!(member.address().scope, ScopeRef::Session);
}

/// The strengthening S5 makes over S4: an address and its policy are written by one constructor from
/// one argument, so S4's `TxError::ScopeDisagreesWithAddress` describes a state with no spelling.
///
/// This case is what replaces S4's `a_seed_whose_scope_disagrees_with_the_address_is_refused`. It
/// checks the property that check was checking — across **all six** constructors, which the runtime
/// check could only ever see one of at a time. It is `async` only because a `TaskId` has no public
/// constructor: `Tx::mint` is the one allocator (ADR-0030 F6 §A), which is itself the point.
#[tokio::test]
async fn every_constructor_writes_an_address_that_agrees_with_its_own_policy() {
    let (store, _switch) = super::store_double::FaultyStore::new();
    let (_session, handle) = crate::SessionMut::open(Box::new(store));
    let cx = cyrup_pico_store::Cx::detached();

    let crate::CommitOutcome::NothingToCommit { result: task, .. } = handle
        .commit(&cx, async |mut tx| {
            let task: TaskId = tx.mint().await?;
            Ok((task, tx.writing()))
        })
        .await
    else {
        panic!("minting alone stages nothing, so this commit is NothingToCommit");
    };

    let conversation: ConversationId = ROOT_CONVERSATION_ID;
    let alpha = || FamilyKey::parse("alpha").expect("a key");
    let chatter = DocToken::<Chatter>::define().expect("a definition");
    let pinned = DocToken::<Pinned>::define().expect("a definition");
    let counter = DocToken::<Counter>::define().expect("a definition");

    // Each `DocAddress<D>` is a different type, so the four checks are a macro-free repetition
    // rather than a loop over an array — which is itself the typing this case is about.
    let session_singleton = DocToken::<Live>::define().expect("a definition").at();
    let session_family = counter.at_key(alpha());
    assert_eq!(
        session_singleton.address().scope,
        session_singleton.scope().as_ref()
    );
    assert_eq!(session_singleton.address().scope, ScopeRef::Session);
    assert_eq!(
        session_family.address().scope,
        session_family.scope().as_ref()
    );
    assert_eq!(session_family.address().scope, ScopeRef::Session);

    let conversation_singleton = chatter.in_conversation(conversation);
    let conversation_family = DocToken::<CounterInConversation>::define()
        .expect("a definition")
        .in_conversation_keyed(conversation, alpha());
    assert_eq!(
        conversation_singleton.address().scope,
        conversation_singleton.scope().as_ref()
    );
    assert_eq!(
        conversation_singleton.address().scope,
        ScopeRef::Conversation(conversation)
    );
    assert_eq!(
        conversation_family.address().scope,
        conversation_family.scope().as_ref()
    );
    assert_eq!(
        conversation_family.address().scope,
        ScopeRef::Conversation(conversation)
    );

    let task_singleton = DocToken::<PinnedSingleton>::define()
        .expect("a definition")
        .on_task(task);
    let task_family = pinned.on_task_keyed(task, alpha());
    assert_eq!(
        task_singleton.address().scope,
        task_singleton.scope().as_ref()
    );
    assert_eq!(task_singleton.address().scope, ScopeRef::Task(task));
    assert_eq!(task_family.address().scope, task_family.scope().as_ref());
    assert_eq!(task_family.address().scope, ScopeRef::Task(task));

    // The policy travels with the scope, and only where the scope has one.
    assert_eq!(
        conversation_singleton.scope(),
        &cyrup_pico_store::DocumentScope::Conversation {
            conversation_id: conversation,
            semantics: ConversationSemantics::Latest {
                fork: LatestFork::Current
            },
        }
    );
    assert_eq!(
        task_singleton.scope(),
        &cyrup_pico_store::DocumentScope::Task { task_id: task }
    );
    assert_eq!(
        session_singleton.scope(),
        &cyrup_pico_store::DocumentScope::Session
    );
}

/// The kind on the address is the token's, parsed once.
#[test]
fn the_address_carries_the_definitions_parsed_kind() {
    let token = DocToken::<Chatter>::define().expect("a definition");
    let at = token.in_conversation(ROOT_CONVERSATION_ID);
    assert_eq!(
        at.address().kind,
        Kind::parse("test.chatter").expect("a kind")
    );
    assert_eq!(at.version(), token.version());
}
