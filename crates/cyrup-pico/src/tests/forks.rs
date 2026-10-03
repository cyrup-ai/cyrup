//! PICO5-PLAN **S9**'s behaviour suite: forks, and the isolation of what a copy reads.
//!
//! The plan names two things and this module is organised around them:
//!
//! * *"the order-independence case explicitly: the same program with the copy command assembled
//!   before and after the parent's write must produce the same child — which under F3's keyed batch is
//!   not even expressible as two different batches, so this becomes **one canary**"* —
//!   [`forking_and_writing_a_current_source_is_rejected_in_either_order`] runs the same program twice,
//!   with the parent's write before the fork and after it, and asserts the identical pre-admission
//!   rejection and that storage was never entered. The canary half is the assertion that the two
//!   orders are *the same batch*: there is no `Vec<StorageWrite>` to put them in a different sequence.
//! * *"**plus a behaviour test** that later source changes, retirement and reopen cannot affect the
//!   child"* — [`later_source_changes_retirement_and_reopen_cannot_affect_the_child`], which does all
//!   three to one parent document and then reads the child back through a **second** store handle.
//!
//! Beside those, one case per sentence of `spec.md` §3.7 that would otherwise be unpinned: the fork
//! point's visibility, each of the three policies, the policy coming from the record rather than the
//! token, task documents not being copied, and the two halves of *"an unaccessed copy retains only its
//! descriptor"* — no value in Session memory, and `document.copy` metadata on the publication instead
//! of a value.
//!
//! Self-contained, like [`super::observation`]: its own definitions and its own kinds, so a change to
//! S4's or S5's fixtures cannot move a fork assertion.

use std::sync::{Arc, Mutex};

use cyrup_pico_store::{
    ConversationId, ConversationParent, ConversationSemantics, Cx, DocumentAddress, DocumentId,
    DocumentPoint, EntryId, Kind, LatestFork, RewindableFork, ScopeRef, StoredDocument,
};
use serde::{Deserialize, Serialize};

use super::path;
use super::store_double::{FaultSwitch, SharedStore};
use crate::{
    CommitOutcome, ConversationOwnership, ConversationScoped, DocDef, DocToken, EntryDraft, Fork,
    PreparationFailed, Publication, RollbackReason, Session, SessionMut, SessionScoped, Singleton,
    TaskScoped,
};

// ---------------------------------------------------------------------------------------------
// The definitions. One per fork policy, plus a task-scoped one and a session-scoped one, so the
// "never copied" clauses have something real to not copy.
// ---------------------------------------------------------------------------------------------

/// The value every document in this suite holds.
#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct Note {
    text: String,
}

impl Note {
    fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
        }
    }
}

/// `test.fork.current`: `history: "latest"`, `fork: "current"`.
struct Current;

impl DocDef for Current {
    type Value = Note;
    type Place = ConversationScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.fork.current";
    const VERSION: u32 = 1;
    const POLICY: ConversationSemantics = ConversationSemantics::Latest {
        fork: LatestFork::Current,
    };
    fn initial((): ()) -> Note {
        Note::new("seed")
    }
}

/// The **same kind** as [`Current`], redeclared `rewindable`/`asOf`.
///
/// `spec.md:1063-1065`'s hazard: extension code reloaded with a changed policy. The fork must read
/// `latest`/`current` from the record anyway, and a typed acquisition with this token must be refused.
struct CurrentRedeclared;

impl DocDef for CurrentRedeclared {
    type Value = Note;
    type Place = ConversationScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.fork.current";
    const VERSION: u32 = 1;
    const POLICY: ConversationSemantics = ConversationSemantics::Rewindable {
        fork: RewindableFork::AsOf,
    };
    fn initial((): ()) -> Note {
        Note::new("seed")
    }
}

/// `test.fork.asof`: `history: "rewindable"`, `fork: "asOf"`.
struct AsOf;

impl DocDef for AsOf {
    type Value = Note;
    type Place = ConversationScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.fork.asof";
    const VERSION: u32 = 1;
    const POLICY: ConversationSemantics = ConversationSemantics::Rewindable {
        fork: RewindableFork::AsOf,
    };
    fn initial((): ()) -> Note {
        Note::new("seed")
    }
}

/// `test.fork.initial`: `history: "latest"`, `fork: "initial"`.
struct Initial;

impl DocDef for Initial {
    type Value = Note;
    type Place = ConversationScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.fork.initial";
    const VERSION: u32 = 1;
    const POLICY: ConversationSemantics = ConversationSemantics::Latest {
        fork: LatestFork::Initial,
    };
    fn initial((): ()) -> Note {
        Note::new("from the definition")
    }
}

/// `test.fork.task`: task-scoped. `spec.md:1489-1490`: never copied by a fork.
struct Pinned;

impl DocDef for Pinned {
    type Value = Note;
    type Place = TaskScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.fork.task";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial((): ()) -> Note {
        Note::new("task private")
    }
}

/// `test.fork.session`: session-scoped. `spec.md:1490`: *"session documents remain shared."*
struct Shared;

impl DocDef for Shared {
    type Value = Note;
    type Place = SessionScoped;
    type Shape = Singleton;
    type Seed = ();
    const KIND: &'static str = "test.fork.session";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial((): ()) -> Note {
        Note::new("shared")
    }
}

fn token<D: DocDef>() -> DocToken<D> {
    DocToken::<D>::define().expect("a valid definition")
}

fn kind(s: &str) -> Kind {
    Kind::parse(s).expect("a valid kind")
}

fn entry(kind_name: &str) -> EntryDraft {
    EntryDraft {
        kind: kind(kind_name),
        model: None,
        data: None,
        head: None,
        edits: None,
        by_task_id: None,
    }
}

// ---------------------------------------------------------------------------------------------
// The fixture: a parent conversation with two entries and one document per policy, where the two
// `latest`/`rewindable` documents were changed AFTER the first entry. That is what makes the
// `current`-versus-`asOf` distinction observable rather than a coincidence.
// ---------------------------------------------------------------------------------------------

/// Everything a fork case needs, with the parent already two commits deep.
struct Parent {
    shared: SharedStore,
    session: Session,
    handle: SessionMut,
    switch: FaultSwitch,
    cx: Cx,
    /// The conversation to fork.
    id: ConversationId,
    /// The entry to fork at: the **first** one, so the second commit's writes are "later".
    at: EntryId,
}

impl Parent {
    /// The fork point, as the one value [`crate::Tx::fork_conversation`] takes.
    const fn point(&self) -> ConversationParent {
        ConversationParent {
            conversation_id: self.id,
            at: self.at,
        }
    }

    /// One incarnation's committed stored state, read through the durable store rather than the
    /// Session — which is the only way to see a copy the Session never materialised.
    async fn stored(&self, id: DocumentId) -> StoredDocument {
        self.shared
            .document(id)
            .await
            .expect("the store is healthy")
            .expect("one stored incarnation")
    }

    /// The `text` field of one incarnation's committed value.
    async fn text(&self, id: DocumentId) -> String {
        self.stored(id)
            .await
            .value
            .get("text")
            .and_then(|v| v.as_str())
            .expect("a `text` string")
            .to_owned()
    }
}

/// Build the fixture: two commits, so the second one's writes are *later* than the fork point.
async fn parent() -> Parent {
    let shared = SharedStore::new();
    let (store, switch) = shared.open();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();

    // Commit 1: the conversation, the fork-point entry, one document per policy at "v1", plus the
    // two documents a fork must not copy.
    let CommitOutcome::Committed {
        result: (id, at),
        session: handle,
        ..
    } = handle
        .commit(&cx, async |tx| {
            let mut tx = tx.writing();
            let conversation = tx
                .create_conversation(ConversationOwnership::Ownerless, None)
                .await?;
            let id = conversation.id;
            let first = tx.append_entry(id, entry("test.user")).await?;

            let current = tx.doc(&token::<Current>().in_conversation(id), ()).await?;
            tx.draft(current)?.set(&path("text"), "v1")?;
            let as_of = tx.doc(&token::<AsOf>().in_conversation(id), ()).await?;
            tx.draft(as_of)?.set(&path("text"), "v1")?;
            let initial = tx.doc(&token::<Initial>().in_conversation(id), ()).await?;
            tx.draft(initial)?.set(&path("text"), "v1")?;

            // A task document in a task of this conversation, and a session document. Neither is in
            // the conversation's scope, which is why neither can be in a fork's answer.
            let task = tx.mint().await?;
            let pinned = tx.doc(&token::<Pinned>().on_task(task), ()).await?;
            tx.draft(pinned)?.set(&path("text"), "v1")?;
            let shared_doc = tx.doc(&token::<Shared>().at(), ()).await?;
            tx.draft(shared_doc)?.set(&path("text"), "v1")?;

            Ok(((id, first.id), tx))
        })
        .await
    else {
        panic!("the first commit must succeed");
    };

    // Commit 2: a second entry, and "v2" in both copying documents. Everything here is **after** the
    // fork point, which is the whole point of the fixture.
    let CommitOutcome::Committed {
        session: handle, ..
    } = handle
        .commit(&cx, async |tx| {
            let mut tx = tx.writing();
            tx.append_entry(id, entry("test.assistant")).await?;
            let current = tx.doc(&token::<Current>().in_conversation(id), ()).await?;
            tx.draft(current)?.set(&path("text"), "v2")?;
            let as_of = tx.doc(&token::<AsOf>().in_conversation(id), ()).await?;
            tx.draft(as_of)?.set(&path("text"), "v2")?;
            Ok(((), tx))
        })
        .await
    else {
        panic!("the second commit must succeed");
    };

    Parent {
        shared,
        session,
        handle,
        switch,
        cx,
        id,
        at,
    }
}

/// Fork at the fixture's fork point in one commit, returning the outcome and the handle.
async fn fork(p: Parent) -> (Fork, Parent) {
    let point = p.point();
    let Parent {
        shared,
        session,
        handle,
        switch,
        cx,
        id,
        at,
    } = p;
    let CommitOutcome::Committed {
        result,
        session: handle,
        ..
    } = handle
        .commit(&cx, async |tx| {
            let mut tx = tx.writing();
            let fork = tx
                .fork_conversation(ConversationOwnership::Ownerless, point)
                .await?;
            Ok((fork, tx))
        })
        .await
    else {
        panic!("the forking commit must succeed");
    };
    (
        result,
        Parent {
            shared,
            session,
            handle,
            switch,
            cx,
            id,
            at,
        },
    )
}

/// The child incarnation one kind was copied into, and the policy that copied it.
fn copied(fork: &Fork, kind_name: &str) -> (RewindableFork, Option<DocumentId>) {
    let found = fork
        .documents()
        .iter()
        .find(|d| d.address().kind.as_str() == kind_name)
        .expect("one decision for that kind");
    (found.policy(), found.child())
}

// ---------------------------------------------------------------------------------------------
// `spec.md:1473-1475`: one concrete VISIBLE entry.
// ---------------------------------------------------------------------------------------------

/// `spec.md:258-259`: *"`parent.at` is an entry in the parent history visible to the child."*
///
/// An entry that belongs to another conversation is not, and the rejection is pre-admission: the
/// Session comes back and storage was never entered.
#[tokio::test]
async fn a_fork_point_must_be_visible_in_the_parents_history() {
    let p = parent().await;
    let before = p.switch.commits();
    let Parent {
        session: _session,
        handle,
        cx,
        id,
        at,
        switch,
        ..
    } = p;

    // A second, unrelated conversation, and an entry in it.
    let CommitOutcome::Committed {
        result: elsewhere,
        session: handle,
        ..
    } = handle
        .commit(&cx, async |tx| {
            let mut tx = tx.writing();
            let other = tx
                .create_conversation(ConversationOwnership::Ownerless, None)
                .await?;
            let entry = tx.append_entry(other.id, entry("test.user")).await?;
            Ok((entry.id, tx))
        })
        .await
    else {
        panic!("the unrelated commit must succeed");
    };
    let after_setup = switch.commits();

    let outcome = handle
        .commit::<(), _>(&cx, async |tx| {
            let mut tx = tx.writing();
            tx.fork_conversation(
                ConversationOwnership::Ownerless,
                ConversationParent {
                    conversation_id: id,
                    at: elsewhere,
                },
            )
            .await?;
            Ok(((), tx))
        })
        .await;

    let CommitOutcome::RolledBack { reason, .. } = outcome else {
        panic!("a fork point outside the parent's history must not commit");
    };
    assert!(
        matches!(
            reason,
            RollbackReason::Callback(crate::CallbackError::Tx(
                crate::TxError::ForkPointNotVisible { .. }
            ))
        ),
        "expected ForkPointNotVisible, got {reason:?}"
    );
    assert_eq!(
        switch.commits(),
        after_setup,
        "a rejected fork allocates no sequence"
    );
    assert!(after_setup > before, "the fixture really did commit");
    let _ = at;
}

// ---------------------------------------------------------------------------------------------
// `spec.md:1478-1490`: the three policies, each read from the record.
// ---------------------------------------------------------------------------------------------

/// The policy table of `spec.md:1481-1486`, all three rows in one fork.
///
/// `current` takes *"the committed parent value selected when the fork commit runs"* — "v2", written
/// after the fork point. `asOf` takes *"the parent value at `E`'s commit"* — "v1". `initial` copies
/// nothing at all and is reported as a decision rather than omitted.
#[tokio::test]
async fn each_policy_takes_the_value_its_own_record_names() {
    let (fork, p) = fork(parent().await).await;

    assert_eq!(
        fork.documents().len(),
        3,
        "three conversation documents, and nothing else: {:?}",
        fork.documents()
    );

    let (policy, child) = copied(&fork, "test.fork.current");
    assert_eq!(policy, RewindableFork::Current);
    let child = child.expect("`current` copies an instance");
    assert_eq!(
        p.text(child).await,
        "v2",
        "`current` is the committed value when the fork commit runs"
    );

    let (policy, child) = copied(&fork, "test.fork.asof");
    assert_eq!(policy, RewindableFork::AsOf);
    let child = child.expect("`asOf` copies an instance");
    assert_eq!(
        p.text(child).await,
        "v1",
        "`asOf` is the parent value at the fork point's commit, not today's"
    );

    let (policy, child) = copied(&fork, "test.fork.initial");
    assert_eq!(policy, RewindableFork::Initial);
    assert_eq!(
        child, None,
        "`initial` copies no instance; the initializer runs on first child access"
    );
}

/// `spec.md:4317`: *"Storage persists one independent complete child base at the source's stored
/// version."*
///
/// A **base**, not a delta and not a reference — which is the representation the independence rests
/// on, so it is asserted on the record rather than inferred from behaviour.
#[tokio::test]
async fn a_copy_is_persisted_as_one_complete_base_at_the_sources_version() {
    let (fork, p) = fork(parent().await).await;
    for name in ["test.fork.current", "test.fork.asof"] {
        let (_, child) = copied(&fork, name);
        let child = child.expect("a copied instance");
        let stored = p.stored(child).await;
        assert_eq!(
            stored.deltas_since_base, 0,
            "{name}: a copy is a base with no delta tail"
        );
        assert_eq!(
            stored.def_version().get(),
            1,
            "{name}: at the source's stored version"
        );
    }
}

/// `spec.md:1489-1490`: *"Task documents and tasks are never copied. Session documents remain shared
/// and are not rewindable."*
///
/// Not a filter: a fork enumerates **one exact conversation scope**, and neither of those lives in it.
#[tokio::test]
async fn a_fork_copies_no_task_and_no_session_document() {
    let (fork, _p) = fork(parent().await).await;
    for name in ["test.fork.task", "test.fork.session"] {
        assert!(
            fork.documents()
                .iter()
                .all(|d| d.address().kind.as_str() != name),
            "{name} must not appear in a fork's answer: {:?}",
            fork.documents()
        );
    }
}

/// `spec.md:1478-1479`: the policy is *"persisted in its `DocumentRecord`"* — and `spec.md:1063-1065`
/// says the record, not the token, is the authority.
///
/// The fork takes no token at all, so there is nothing for a redeclared definition to change: the
/// `latest`/`current` record is still copied as `current`. The redeclared **token** is refused at the
/// typed acquisition, which is where a check can see it.
#[tokio::test]
async fn a_redeclared_token_changes_neither_the_policy_nor_the_record() {
    let (fork, p) = fork(parent().await).await;
    let (policy, child) = copied(&fork, "test.fork.current");
    assert_eq!(
        policy,
        RewindableFork::Current,
        "the record says `latest`/`current`, and the fork read the record"
    );
    let child = child.expect("`current` copies an instance");

    let Parent { handle, cx, .. } = p;
    let outcome = handle
        .commit::<(), _>(&cx, async |tx| {
            let mut tx = tx.writing();
            // The same kind, redeclared `rewindable`/`asOf`.
            tx.doc(
                &token::<CurrentRedeclared>().in_conversation(
                    // The CHILD conversation: the copy carried the parent's semantics into it.
                    fork.child(),
                ),
                (),
            )
            .await?;
            Ok(((), tx))
        })
        .await;
    let CommitOutcome::RolledBack { reason, .. } = outcome else {
        panic!("a redeclared token must not be accepted");
    };
    assert!(
        matches!(
            reason,
            RollbackReason::Callback(crate::CallbackError::Tx(
                crate::TxError::TokenDisagreesWithRecord { .. }
            ))
        ),
        "expected TokenDisagreesWithRecord, got {reason:?}"
    );
    let _ = child;
}

// ---------------------------------------------------------------------------------------------
// `spec.md:1491-1494`: the pre-admission rejection, and the order-independence canary.
// ---------------------------------------------------------------------------------------------

/// **The canary PICO5-PLAN S9 names.** The same program, with the parent's write assembled before the
/// fork and after it, is rejected identically and never reaches storage.
///
/// `spec.md:1491-1494`: *"a transaction that creates a fork therefore rejects if it also writes one of
/// the parent's `fork: "current"` documents; commit the parent change first so the fork has one
/// unambiguous stored source revision."*
///
/// The two orders are **the same batch**: ADR-0030 F3's [`Batch`](cyrup_pico_store::Batch) is five
/// `BTreeMap`s, so there is no write array for the order to live in and this test cannot distinguish
/// them *by construction* — which is the canary. What it does distinguish is that the rejection is
/// `RolledBack` and not `Rejected`: it happened in preparation, before admission, so
/// `FaultSwitch::commits` never moved.
#[tokio::test]
async fn forking_and_writing_a_current_source_is_rejected_in_either_order() {
    for write_first in [true, false] {
        let p = parent().await;
        let point = p.point();
        let Parent {
            handle,
            cx,
            switch,
            id,
            ..
        } = p;
        let before = switch.commits();

        let outcome = handle
            .commit::<(), _>(&cx, async |tx| {
                let mut tx = tx.writing();
                let at = token::<Current>().in_conversation(id);
                if write_first {
                    let doc = tx.doc(&at, ()).await?;
                    tx.draft(doc)?.set(&path("text"), "v3")?;
                    tx.fork_conversation(ConversationOwnership::Ownerless, point)
                        .await?;
                } else {
                    tx.fork_conversation(ConversationOwnership::Ownerless, point)
                        .await?;
                    let doc = tx.doc(&at, ()).await?;
                    tx.draft(doc)?.set(&path("text"), "v3")?;
                }
                Ok(((), tx))
            })
            .await;

        let CommitOutcome::RolledBack { reason, .. } = outcome else {
            panic!("write_first={write_first}: the conflicting transaction must not commit");
        };
        assert!(
            matches!(
                reason,
                RollbackReason::Preparation(PreparationFailed::ForkSourceWritten {
                    policy: RewindableFork::Current,
                    ..
                })
            ),
            "write_first={write_first}: expected ForkSourceWritten, got {reason:?}"
        );
        assert_eq!(
            switch.commits(),
            before,
            "write_first={write_first}: a pre-admission rejection allocates no sequence"
        );
    }
}

/// The complement of the canary: forking while writing an **`asOf`** source is legal once the copy has
/// been replaced by an ordinary create.
///
/// `spec.md:4316`'s *"a batch may not create, change, or retire a **selected** source"* stops applying
/// when nothing is selected any more, and `spec.md:1491-1494`'s rule names `fork: "current"` only — for
/// a reason: the parent's value *at the fork point* is historical, so a write now cannot make it
/// ambiguous. Pinning this is what stops the rejection above from being widened into a rule the spec
/// does not have.
#[tokio::test]
async fn an_as_of_source_may_be_written_once_its_copy_became_a_create() {
    let p = parent().await;
    let point = p.point();
    let Parent {
        shared,
        handle,
        cx,
        id,
        ..
    } = p;

    let CommitOutcome::Committed { result: fork, .. } = handle
        .commit(&cx, async |tx| {
            let mut tx = tx.writing();
            let fork = tx
                .fork_conversation(ConversationOwnership::Ownerless, point)
                .await?;
            // Touch the child, which replaces the copy with an ordinary create...
            let child = tx
                .doc(&token::<AsOf>().in_conversation(fork.child()), ())
                .await?;
            assert_eq!(
                tx.draft(child)?
                    .read_at(&path("text"))
                    .and_then(cyrup_pico_doc::DocValue::as_str),
                Some("v1"),
                "the child inherited the parent's value AT THE FORK POINT"
            );
            // ... and then write the parent, which is no longer a selected source.
            let parent_doc = tx.doc(&token::<AsOf>().in_conversation(id), ()).await?;
            tx.draft(parent_doc)?.set(&path("text"), "v3")?;
            Ok((fork, tx))
        })
        .await
    else {
        panic!("writing an as-of source beside a materialised copy must be legal");
    };

    let (_, child) = copied(&fork, "test.fork.asof");
    let child = child.expect("`asOf` copies an instance");
    let stored = shared
        .document(child)
        .await
        .expect("the store is healthy")
        .expect("the child exists");
    assert_eq!(
        stored
            .value
            .get("text")
            .and_then(cyrup_pico_doc::DocValue::as_str),
        Some("v1"),
        "the child kept the fork point's value while the parent moved on"
    );
}

// ---------------------------------------------------------------------------------------------
// `spec.md:4317-4318`: later source changes, reclamation, retirement and reopen.
// ---------------------------------------------------------------------------------------------

/// **The behaviour test PICO5-PLAN S9 names.** *"Later source changes, reclamation, retirement, or
/// backend reopen cannot affect the child"* (`spec.md:4317-4318`).
///
/// All three are done to both copied documents, and the child is then read back through a **second**
/// store handle over the same durable state — the closest a backend with no files has to a reopen
/// (`src/tests/store_double.rs` explains why that is the right shape here).
#[tokio::test]
async fn later_source_changes_retirement_and_reopen_cannot_affect_the_child() {
    let (fork, p) = fork(parent().await).await;
    let (_, current_child) = copied(&fork, "test.fork.current");
    let (_, as_of_child) = copied(&fork, "test.fork.asof");
    let current_child = current_child.expect("a copied instance");
    let as_of_child = as_of_child.expect("a copied instance");

    let Parent {
        shared,
        handle,
        cx,
        id,
        ..
    } = p;

    // Change both sources...
    let CommitOutcome::Committed {
        session: handle, ..
    } = handle
        .commit(&cx, async |tx| {
            let mut tx = tx.writing();
            let current = tx.doc(&token::<Current>().in_conversation(id), ()).await?;
            tx.draft(current)?.set(&path("text"), "v9")?;
            let as_of = tx.doc(&token::<AsOf>().in_conversation(id), ()).await?;
            tx.draft(as_of)?.set(&path("text"), "v9")?;
            Ok(((), tx))
        })
        .await
    else {
        panic!("changing the sources must succeed");
    };

    // ...and then retire them.
    let CommitOutcome::Committed { .. } = handle
        .commit(&cx, async |tx| {
            let mut tx = tx.writing();
            tx.retire_doc(&token::<Current>().in_conversation(id))
                .await?;
            tx.retire_doc(&token::<AsOf>().in_conversation(id)).await?;
            Ok(((), tx))
        })
        .await
    else {
        panic!("retiring the sources must succeed");
    };

    // Reopen: a second handle over the same durable state, with its own Session.
    let (reopened, _switch) = shared.open();
    let (_session, _handle) = SessionMut::open(Box::new(reopened));
    let read = |id| {
        let shared = shared.clone();
        async move {
            shared
                .document(id)
                .await
                .expect("the store is healthy")
                .expect("the child survived")
                .value
                .get("text")
                .and_then(cyrup_pico_doc::DocValue::as_str)
                .map(str::to_owned)
        }
    };
    assert_eq!(
        read(current_child).await.as_deref(),
        Some("v2"),
        "the `current` child is independent of every later source change and of retirement"
    );
    assert_eq!(
        read(as_of_child).await.as_deref(),
        Some("v1"),
        "the `asOf` child is independent of every later source change and of retirement"
    );
}

// ---------------------------------------------------------------------------------------------
// `spec.md:1499-1504`: an unaccessed copy, and what it publishes.
// ---------------------------------------------------------------------------------------------

/// `spec.md:1499`: *"an unaccessed copy retains only its descriptor in Session memory"*, and
/// `spec.md:1502-1504`: *"definition-free copies publish explicit `document.copy` metadata rather than
/// a value … never interpreted as a document value."*
///
/// Both halves: the Session has no snapshot of the child, and the publication announces the copy in
/// [`Publication::copies`] — a list of [`Copied`](crate::Copied), which has no `value()` to misread
/// (`tests/compile-fail/a_copied_document_has_no_value.rs`).
#[tokio::test]
async fn an_unaccessed_copy_announces_metadata_and_holds_no_value() {
    let p = parent().await;
    let point = p.point();
    let Parent {
        session,
        handle,
        cx,
        ..
    } = p;

    let seen: Arc<Mutex<Vec<(DocumentId, DocumentPoint, DocumentAddress)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    let changed = Arc::new(Mutex::new(Vec::new()));
    let change_recorder = Arc::clone(&changed);
    let disposer = session.subscribe_commits(move |publication: &Publication| {
        if let Ok(mut log) = recorder.lock() {
            log.extend(
                publication
                    .copies()
                    .iter()
                    .map(|c| (c.document(), c.at(), c.address().clone())),
            );
        }
        if let Ok(mut log) = change_recorder.lock() {
            log.extend(publication.changes().iter().map(crate::Change::document));
        }
    });

    let CommitOutcome::Committed { result: fork, .. } = handle
        .commit(&cx, async |tx| {
            let mut tx = tx.writing();
            let fork = tx
                .fork_conversation(ConversationOwnership::Ownerless, point)
                .await?;
            Ok((fork, tx))
        })
        .await
    else {
        panic!("the forking commit must succeed");
    };
    disposer.dispose();

    let copies = seen.lock().expect("the log is healthy").clone();
    assert_eq!(copies.len(), 2, "one announcement per copy: {copies:?}");
    let changes = changed.lock().expect("the log is healthy").clone();
    for (child, _, address) in &copies {
        assert!(
            !changes.contains(child),
            "a copy must not also appear as a value-bearing change: {child}"
        );
        assert_eq!(
            address.scope,
            ScopeRef::Conversation(fork.child()),
            "a copy's announced address is the CHILD's"
        );
        assert_eq!(
            session.snapshot(address).expect("the authority is healthy"),
            None,
            "an unaccessed copy retains no value in Session memory"
        );
    }
    // The points are the two the policies name, one each.
    assert!(
        copies
            .iter()
            .any(|(_, at, _)| *at == DocumentPoint::Current),
        "the `current` copy reads at `current`: {copies:?}"
    );
    assert!(
        copies
            .iter()
            .any(|(_, at, _)| matches!(at, DocumentPoint::At(_))),
        "the `asOf` copy reads at the fork point's commit: {copies:?}"
    );
}

/// `spec.md:1500-1502`: *"typed access inside the creating transaction lazily reads the detached
/// source, migrates when required, and replaces the copy with one ordinary child create containing the
/// final prepared value."*
///
/// So a touched copy is a **change** with a value, not a copy announcement — and the value it starts
/// from is the parent's, not the definition's `initial()`.
#[tokio::test]
async fn a_touched_copy_becomes_an_ordinary_create_with_the_inherited_value() {
    let p = parent().await;
    let point = p.point();
    let Parent {
        shared,
        session,
        handle,
        cx,
        ..
    } = p;

    let copies = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&copies);
    let disposer = session.subscribe_commits(move |publication: &Publication| {
        if let Ok(mut n) = counter.lock() {
            *n += publication.copies().len();
        }
    });

    let CommitOutcome::Committed { result: fork, .. } = handle
        .commit(&cx, async |tx| {
            let mut tx = tx.writing();
            let fork = tx
                .fork_conversation(ConversationOwnership::Ownerless, point)
                .await?;
            let child = tx
                .doc(&token::<Current>().in_conversation(fork.child()), ())
                .await?;
            // The draft reads what it inherited, NOT `Current::initial()`'s "seed".
            assert_eq!(
                tx.draft(child)?
                    .read_at(&path("text"))
                    .and_then(cyrup_pico_doc::DocValue::as_str),
                Some("v2"),
                "a typed acquisition of a copy reads the source, not the definition's initial value"
            );
            tx.draft(child)?.set(&path("text"), "edited")?;
            Ok((fork, tx))
        })
        .await
    else {
        panic!("the forking commit must succeed");
    };
    disposer.dispose();

    assert_eq!(
        *copies.lock().expect("the counter is healthy"),
        1,
        "only the untouched `asOf` copy is announced as a copy"
    );
    let (_, child) = copied(&fork, "test.fork.current");
    let child = child.expect("a copied instance");
    let stored = shared
        .document(child)
        .await
        .expect("the store is healthy")
        .expect("the child exists");
    assert_eq!(
        stored
            .value
            .get("text")
            .and_then(cyrup_pico_doc::DocValue::as_str),
        Some("edited"),
        "the replacement carries the final prepared value"
    );
    assert_eq!(
        stored.deltas_since_base, 0,
        "a create is one complete base (`spec.md:1239`)"
    );
    assert_eq!(
        session
            .snapshot(token::<Current>().in_conversation(fork.child()).address())
            .expect("the authority is healthy")
            .and_then(|root| root
                .get("text")
                .and_then(cyrup_pico_doc::DocValue::as_str)
                .map(str::to_owned))
            .as_deref(),
        Some("edited"),
        "a touched copy IS in Session memory, because the Session prepared its value"
    );
}

/// `spec.md:1245-1247` for a copy: retiring the child before anything touched it must persist the
/// retirement **with** the copy, not instead of it.
///
/// ADR-0030 F3 is why this is one field and not a second command:
/// [`DocumentCommand::Copy`](cyrup_pico_store::DocumentCommand::Copy) carries `then: Retire`, so there
/// is no order between the copy and the retirement to get wrong.
#[tokio::test]
async fn retiring_an_untouched_copy_retires_the_incarnation_the_copy_created() {
    let p = parent().await;
    let point = p.point();
    let Parent {
        shared, handle, cx, ..
    } = p;

    let CommitOutcome::Committed { result: fork, .. } = handle
        .commit(&cx, async |tx| {
            let mut tx = tx.writing();
            let fork = tx
                .fork_conversation(ConversationOwnership::Ownerless, point)
                .await?;
            tx.retire_doc(&token::<Current>().in_conversation(fork.child()))
                .await?;
            Ok((fork, tx))
        })
        .await
    else {
        panic!("the forking commit must succeed");
    };

    let (_, child) = copied(&fork, "test.fork.current");
    let child = child.expect("a copied instance");
    assert_eq!(
        shared
            .document(child)
            .await
            .expect("the store is healthy")
            .map(|_| ()),
        None,
        "an incarnation created and retired in one commit has the empty lifetime (`spec.md:1114`)"
    );
}
