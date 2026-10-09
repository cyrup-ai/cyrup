//! The cases themselves.
//!
//! Each one names the line of the specification it tests, and each is written against
//! `&mut dyn Storage` with no `Session` anywhere — see this module's parent documentation for why that
//! is the point rather than a convenience.

use std::sync::Arc;

use cyrup_pico_doc::{DocMap, DocRoot, DocValue, Op, OpBatch, Path};

use crate::batch::{BatchBuilder, DocumentBase, DocumentContent, Retire};
use crate::error::{CommitError, RejectedReason};
use crate::records::{
    ConversationRecord, ConversationSemantics, DocumentCreate, DocumentScope, EntryRecord,
    LatestFork,
};
use crate::{
    Batch, Conversation, ConversationId, Cx, DefVersion, Document, DocumentId, DocumentPoint,
    Entry, EntryId, Id, IdKind, Kind, PageLimit, ROOT_CONVERSATION_ID, Seq, Storage, StorageExt,
    Task,
};

use super::{Case, Failure};

/// Every case, in the order the report prints them.
///
/// A `const` table rather than an inventory macro or a registry, so that adding a case is one visible
/// edit and the suite a backend runs is exactly the suite in the source.
pub const CASES: &[Case] = &[
    Case {
        name: "read_your_commit",
        about: "spec.md:4281 — once commit() resolves, later reads through that Storage observe it",
        run: |store| Box::pin(read_your_commit(store)),
    },
    Case {
        name: "sequence_strictly_increases",
        about: "spec.md:99-100 — commit sequences strictly increase; gaps are permitted",
        run: |store| Box::pin(sequence_strictly_increases(store)),
    },
    Case {
        name: "mint_is_monotone_and_never_reissues",
        about: "spec.md:4282 — ids come from one durable monotone global namespace",
        run: |store| Box::pin(mint_is_monotone_and_never_reissues(store)),
    },
    Case {
        name: "a_committed_value_is_detached",
        about: "spec.md:4375-4378 — a committed value cannot be observed to change",
        run: |store| Box::pin(a_committed_value_is_detached(store)),
    },
    Case {
        name: "content_is_applied_before_retirement",
        about: "spec.md:4363 — storage applies content before retirement, whatever the assembly order",
        run: |store| Box::pin(content_is_applied_before_retirement(store)),
    },
    Case {
        name: "a_second_live_incarnation_at_one_address_is_rejected",
        about: "spec.md:4362-4365 — the cross-batch half of the keyed batch (ADR-0030 F3)",
        run: |store| Box::pin(a_second_live_incarnation_at_one_address_is_rejected(store)),
    },
    Case {
        name: "one_number_is_owned_by_one_record_type",
        about: "spec.md:4274-4275 — global id ownership against already-committed records",
        run: |store| Box::pin(one_number_is_owned_by_one_record_type(store)),
    },
    Case {
        name: "a_numeric_read_of_a_current_only_incarnation_is_refused",
        about: "spec.md:4356-4357 — it rejects rather than depending on reclaimed content",
        run: |store| {
            Box::pin(a_numeric_read_of_a_current_only_incarnation_is_refused(
                store,
            ))
        },
    },
];

// --- helpers -------------------------------------------------------------------------------------
//
// Every helper returns a `Result` rather than unwrapping, because the workspace denies `unwrap`,
// `expect` and `panic!` and because a case that fails to set itself up should say so in the report
// rather than abort the run.

/// Parse a kind, reporting a setup failure.
fn kind(s: &str) -> Result<Kind, Failure> {
    Kind::parse(s).map_err(|e| Failure::setup(format!("kind {s:?}: {e}")))
}

/// A page limit, reporting a setup failure.
fn limit(n: u32) -> Result<PageLimit, Failure> {
    PageLimit::parse(n).map_err(|e| Failure::setup(format!("page limit {n}: {e}")))
}

/// A one-key document value.
fn value(key: &str, n: i64) -> DocRoot {
    let mut map = DocMap::new();
    map.insert(Arc::from(key), DocValue::integer(n));
    DocRoot::from_map(map)
}

/// Mint one id, reporting a setup failure.
async fn mint<K: IdKind>(store: &mut dyn Storage, cx: &Cx) -> Result<Id<K>, Failure> {
    store
        .mint::<K>(cx)
        .await
        .map_err(|e| Failure::setup(format!("minting a {} id: {e}", K::KIND)))
}

/// Commit one batch, reporting an unexpected failure.
async fn commit(store: &mut dyn Storage, batch: Batch, cx: &Cx) -> Result<Seq, Failure> {
    store.commit(batch, cx).await.map_err(Failure::Commit)
}

/// The root conversation, so a document has a conversation scope to live in.
async fn seed_root(store: &mut dyn Storage, cx: &Cx) -> Result<Seq, Failure> {
    let mut builder = BatchBuilder::new();
    builder
        .conversation(ConversationRecord {
            id: ROOT_CONVERSATION_ID,
            parent: None,
            owner: None,
        })
        .map_err(|e| Failure::setup(e.to_string()))?;
    let batch = builder
        .build()
        .ok_or_else(|| Failure::setup("the seed batch was empty"))?;
    commit(store, batch, cx).await
}

/// A `history: "latest"` conversation document creation at one address.
fn latest_doc(id: DocumentId, kind: Kind, conversation_id: ConversationId) -> DocumentCreate {
    DocumentCreate {
        id,
        kind,
        key: None,
        scope: DocumentScope::Conversation {
            conversation_id,
            semantics: ConversationSemantics::Latest {
                fork: LatestFork::Current,
            },
        },
    }
}

/// A `history: "rewindable"` conversation document creation at one address.
fn rewindable_doc(id: DocumentId, kind: Kind, conversation_id: ConversationId) -> DocumentCreate {
    DocumentCreate {
        id,
        kind,
        key: None,
        scope: DocumentScope::Conversation {
            conversation_id,
            semantics: ConversationSemantics::Rewindable {
                fork: crate::RewindableFork::Current,
            },
        },
    }
}

/// Create one document with a complete base, returning the commit sequence.
async fn create_document(
    store: &mut dyn Storage,
    record: DocumentCreate,
    base: DocRoot,
    then: Retire,
    cx: &Cx,
) -> Result<Seq, Failure> {
    let mut builder = BatchBuilder::new();
    builder
        .create_document(
            record,
            DocumentBase {
                version: DefVersion::FIRST,
                value: base,
            },
            then,
        )
        .map_err(|e| Failure::setup(e.to_string()))?;
    let batch = builder
        .build()
        .ok_or_else(|| Failure::setup("the create batch was empty"))?;
    commit(store, batch, cx).await
}

// --- cases ---------------------------------------------------------------------------------------

/// `spec.md:4281`: *"Once `commit()` resolves, later reads through that Storage observe it."*
async fn read_your_commit(store: &mut dyn Storage) -> Result<(), Failure> {
    let cx = Cx::detached();
    seed_root(store, &cx).await?;
    let entry_id: EntryId = mint::<Entry>(store, &cx).await?;
    let kind = kind("cyrup.message")?;

    let mut builder = BatchBuilder::new();
    builder
        .entry(EntryRecord {
            id: entry_id,
            conversation_id: ROOT_CONVERSATION_ID,
            kind,
            model: None,
            data: Some(DocValue::string("hello")),
            head: None,
            edits: None,
            by_task_id: None,
        })
        .map_err(|e| Failure::setup(e.to_string()))?;
    let batch = builder
        .build()
        .ok_or_else(|| Failure::setup("the batch was empty"))?;
    let seq = commit(store, batch, &cx).await?;

    let read = store.entry(entry_id, &cx).await?;
    let Some(committed) = read else {
        return Err(Failure::contract(
            "an entry committed through this handle was not visible to a later read",
        ));
    };
    if committed.commit_seq != seq {
        return Err(Failure::contract(format!(
            "the entry reports commit sequence {} but commit() returned {seq}",
            committed.commit_seq
        )));
    }
    if committed.entry.id != entry_id {
        return Err(Failure::contract("the read returned a different entry"));
    }

    // And through the scan path, which is a different index.
    let page = store
        .scan_entries(
            &crate::EntryQuery::all(ROOT_CONVERSATION_ID),
            limit(10)?,
            None,
            &cx,
        )
        .await?;
    if !page.items.iter().any(|e| e.id == entry_id) {
        return Err(Failure::contract(
            "the committed entry was visible to entry() but not to scan_entries()",
        ));
    }
    Ok(())
}

/// `spec.md:99-100`: *"Strictly increases between commits; gaps are permitted."*
async fn sequence_strictly_increases(store: &mut dyn Storage) -> Result<(), Failure> {
    let cx = Cx::detached();
    let mut previous: Option<Seq> = None;
    for n in 0..4_u32 {
        let id: ConversationId = mint::<Conversation>(store, &cx).await?;
        let mut builder = BatchBuilder::new();
        builder
            .conversation(ConversationRecord {
                id,
                parent: None,
                owner: None,
            })
            .map_err(|e| Failure::setup(e.to_string()))?;
        let batch = builder
            .build()
            .ok_or_else(|| Failure::setup("the batch was empty"))?;
        let seq = commit(store, batch, &cx).await?;
        if let Some(previous) = previous
            && seq <= previous
        {
            return Err(Failure::contract(format!(
                "commit {n} allocated sequence {seq}, which does not follow {previous}"
            )));
        }
        previous = Some(seq);
    }
    Ok(())
}

/// `spec.md:4282`: *"Allocate from the one global numeric namespace."* One namespace, monotone, and
/// never reissuing — including across kinds, which is what *one* namespace means.
async fn mint_is_monotone_and_never_reissues(store: &mut dyn Storage) -> Result<(), Failure> {
    let cx = Cx::detached();
    let mut seen: Vec<u64> = Vec::new();
    for _ in 0..3_u32 {
        seen.push(
            crate::RawId::from(mint::<Entry>(store, &cx).await?)
                .get()
                .get(),
        );
        seen.push(
            crate::RawId::from(mint::<Task>(store, &cx).await?)
                .get()
                .get(),
        );
        seen.push(
            crate::RawId::from(mint::<Document>(store, &cx).await?)
                .get()
                .get(),
        );
    }
    let mut sorted = seen.clone();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() != seen.len() {
        return Err(Failure::contract(
            "mint reissued a number: ids come from one global namespace and are never reused",
        ));
    }
    if seen.windows(2).any(|w| w.first() >= w.last()) {
        return Err(Failure::contract(format!(
            "mint is not monotone across kinds: {seen:?}"
        )));
    }
    Ok(())
}

/// `spec.md:4375-4378`: every retained write value and every read result is detached, so a committed
/// value cannot be observed to change.
///
/// In cyrup this is a property of the types rather than of a copy pass (ADR-0030 F4), which is exactly
/// why the case is worth keeping: it is the assertion that a *future* backend has not reintroduced
/// aliasing by handing out a handle into its own index.
async fn a_committed_value_is_detached(store: &mut dyn Storage) -> Result<(), Failure> {
    let cx = Cx::detached();
    seed_root(store, &cx).await?;
    let doc: DocumentId = mint::<Document>(store, &cx).await?;
    let kind = kind("cyrup.state")?;
    let first = value("n", 1);
    create_document(
        store,
        rewindable_doc(doc, kind, ROOT_CONVERSATION_ID),
        first.clone(),
        Retire::Keep,
        &cx,
    )
    .await?;

    let Some(before) = store.document(doc, DocumentPoint::Current, &cx).await? else {
        return Err(Failure::contract("a created document was not readable"));
    };
    if before.value != first {
        return Err(Failure::contract(
            "the read value is not the value committed",
        ));
    }

    // Change it. `continues` is the witness the read above produced, which is the only way to build a
    // delta at all (ADR-0030 F6 §C).
    let ops = OpBatch::new(vec![Op::Set {
        path: Path::keys(["n"]).map_err(|e| Failure::setup(format!("path: {e}")))?,
        value: DocValue::integer(2),
    }]);
    let mut builder = BatchBuilder::new();
    builder
        .change_document(
            doc,
            DocumentContent::Delta {
                continues: before.version,
                ops,
            },
            Retire::Keep,
        )
        .map_err(|e| Failure::setup(e.to_string()))?;
    let batch = builder
        .build()
        .ok_or_else(|| Failure::setup("the change batch was empty"))?;
    commit(store, batch, &cx).await?;

    // The value read before the change must not have moved, and neither must the value handed to the
    // creating commit.
    if before.value != first {
        return Err(Failure::contract(
            "a value returned by an earlier read changed when a later commit landed",
        ));
    }
    let Some(after) = store.document(doc, DocumentPoint::Current, &cx).await? else {
        return Err(Failure::contract("the document disappeared after a change"));
    };
    if after.value == before.value {
        return Err(Failure::contract("the change did not take effect"));
    }
    Ok(())
}

/// `spec.md:4363`: *"Storage applies content before retirement independent of write-array order."*
///
/// Under ADR-0030 F3's keyed batch the two assembly orders are not even two different batches, so what
/// a *case* can still catch is the half that is about application rather than assembly — and there are
/// two halves worth catching.
///
/// 1. **A backend that retires first rejects its own batch.** The content command and the retirement are
///    one value, so a backend that stamped the lifetime bound before writing the content would find
///    itself writing to a retired incarnation and reject the commit it was given. That the commit is
///    *accepted* is therefore the assertion, and it is the one `spec.md:1246-1247`'s
///    *"retiring an acquired draft persists its final content before retirement"* actually depends on.
/// 2. **Content committed before the retirement survives it.** A delta committed at a sequence inside the
///    incarnation's lifetime is still readable there after the incarnation is retired.
///
/// **A note on what is deliberately *not* asserted**, because it looks like an omission and is not.
/// Content committed in the *same* commit as the retirement is stamped with that commit's sequence, and
/// `spec.md:1111-1114`'s interval is half-open — `created <= at < retired` — so that sequence is the
/// first one at which the incarnation is no longer a member. The final content is therefore **persisted
/// and not readable through a numeric point**: it reaches observers through the publication of that
/// commit (PICO5-PLAN S6), and a reopen suite (S7) is what can show the record survived. Asserting a
/// numeric read of it here would be asserting against `spec.md:4353-4356`.
async fn content_is_applied_before_retirement(store: &mut dyn Storage) -> Result<(), Failure> {
    let cx = Cx::detached();
    seed_root(store, &cx).await?;
    let doc: DocumentId = mint::<Document>(store, &cx).await?;
    let kind = kind("cyrup.final")?;
    let created = create_document(
        store,
        rewindable_doc(doc, kind, ROOT_CONVERSATION_ID),
        value("n", 1),
        Retire::Keep,
        &cx,
    )
    .await?;

    let Some(before) = store.document(doc, DocumentPoint::Current, &cx).await? else {
        return Err(Failure::contract("a created document was not readable"));
    };

    // A change of its own, inside the lifetime.
    let mut builder = BatchBuilder::new();
    builder
        .change_document(
            doc,
            DocumentContent::Delta {
                continues: before.version,
                ops: OpBatch::new(vec![Op::Set {
                    path: Path::keys(["n"]).map_err(|e| Failure::setup(format!("path: {e}")))?,
                    value: DocValue::integer(2),
                }]),
            },
            Retire::Keep,
        )
        .map_err(|e| Failure::setup(e.to_string()))?;
    let batch = builder
        .build()
        .ok_or_else(|| Failure::setup("the change batch was empty"))?;
    let changed = commit(store, batch, &cx).await?;

    // Final content and retirement as one command.
    let final_value = value("n", 99);
    let mut builder = BatchBuilder::new();
    builder
        .change_document(
            doc,
            DocumentContent::Base(DocumentBase {
                version: before.def_version(),
                value: final_value,
            }),
            Retire::Retire,
        )
        .map_err(|e| Failure::setup(e.to_string()))?;
    let batch = builder
        .build()
        .ok_or_else(|| Failure::setup("the retire batch was empty"))?;
    let retired = match store.commit(batch, &cx).await {
        Ok(seq) => seq,
        Err(CommitError::Rejected(RejectedReason::DocumentRetired { .. })) => {
            return Err(Failure::contract(
                "the backend rejected its own batch's content as a write to a retired incarnation: \
                 the retirement was applied before the content it rides with",
            ));
        }
        Err(e) => return Err(Failure::Commit(e)),
    };

    // At `Current` a retired incarnation is absent (`spec.md:4353`).
    if store
        .document(doc, DocumentPoint::Current, &cx)
        .await?
        .is_some()
    {
        return Err(Failure::contract(
            "a retired incarnation is still readable at Current",
        ));
    }
    // Content committed inside the lifetime is still there afterwards.
    let Some(inside) = store.document(doc, DocumentPoint::At(changed), &cx).await? else {
        return Err(Failure::contract(
            "the retired incarnation is not readable inside its lifetime",
        ));
    };
    if inside.value != value("n", 2) {
        return Err(Failure::contract(
            "retirement discarded content committed inside the lifetime",
        ));
    }
    // And the creation sequence still answers with the created value.
    let Some(at_creation) = store.document(doc, DocumentPoint::At(created), &cx).await? else {
        return Err(Failure::contract("the creation sequence is not readable"));
    };
    if at_creation.value != value("n", 1) {
        return Err(Failure::contract(
            "a historical read answered with a later value",
        ));
    }
    // The retirement sequence is the exclusive bound (`spec.md:1111-1114`).
    if store
        .document(doc, DocumentPoint::At(retired), &cx)
        .await?
        .is_some()
    {
        return Err(Failure::contract(
            "the retirement sequence is inside the half-open lifetime",
        ));
    }
    Ok(())
}

/// `spec.md:4362-4365`: one logical address holds at most one live incarnation.
///
/// ADR-0030 F3 keeps this one as a **runtime** check on purpose — it is a fact about already-committed
/// records, so the keyed batch cannot lift it. The case also pins the legal shape beside the illegal
/// one: retire-then-create at one address in a *single* batch is allowed.
async fn a_second_live_incarnation_at_one_address_is_rejected(
    store: &mut dyn Storage,
) -> Result<(), Failure> {
    let cx = Cx::detached();
    seed_root(store, &cx).await?;
    let kind = kind("cyrup.singleton")?;
    let first: DocumentId = mint::<Document>(store, &cx).await?;
    create_document(
        store,
        latest_doc(first, kind.clone(), ROOT_CONVERSATION_ID),
        value("n", 1),
        Retire::Keep,
        &cx,
    )
    .await?;

    let second: DocumentId = mint::<Document>(store, &cx).await?;
    let mut builder = BatchBuilder::new();
    builder
        .create_document(
            latest_doc(second, kind.clone(), ROOT_CONVERSATION_ID),
            DocumentBase {
                version: DefVersion::FIRST,
                value: value("n", 2),
            },
            Retire::Keep,
        )
        .map_err(|e| Failure::setup(e.to_string()))?;
    let batch = builder
        .build()
        .ok_or_else(|| Failure::setup("the batch was empty"))?;
    match store.commit(batch, &cx).await {
        Err(CommitError::Rejected(RejectedReason::AddressOccupied { occupant, .. })) => {
            if occupant != first {
                return Err(Failure::contract(format!(
                    "the rejection named {occupant} as the occupant, not {first}"
                )));
            }
        }
        Err(CommitError::Rejected(other)) => {
            return Err(Failure::contract(format!(
                "a second live incarnation was rejected for the wrong reason: {other}"
            )));
        }
        Err(CommitError::Uncertain(e)) => {
            return Err(Failure::contract(format!(
                "a deterministic consistency failure was classified uncertain: {e}"
            )));
        }
        Ok(seq) => {
            return Err(Failure::contract(format!(
                "a second live incarnation at one address was committed at sequence {seq}"
            )));
        }
    }

    // The rejection must have left nothing behind: that is what `Rejected` claims.
    if store
        .document(second, DocumentPoint::Current, &cx)
        .await?
        .is_some()
    {
        return Err(Failure::contract(
            "a rejected creation left a durable record: Rejected claims nothing happened",
        ));
    }

    // Retire the occupant and create in the same batch: legal (`spec.md:4364-4365`).
    let third: DocumentId = mint::<Document>(store, &cx).await?;
    let mut builder = BatchBuilder::new();
    builder
        .retire_document(first)
        .map_err(|e| Failure::setup(e.to_string()))?;
    builder
        .create_document(
            latest_doc(third, kind, ROOT_CONVERSATION_ID),
            DocumentBase {
                version: DefVersion::FIRST,
                value: value("n", 3),
            },
            Retire::Keep,
        )
        .map_err(|e| Failure::setup(e.to_string()))?;
    let batch = builder
        .build()
        .ok_or_else(|| Failure::setup("the batch was empty"))?;
    commit(store, batch, &cx).await?;
    if store
        .document(third, DocumentPoint::Current, &cx)
        .await?
        .is_none()
    {
        return Err(Failure::contract(
            "retire-plus-create at one address did not make the new incarnation current",
        ));
    }
    Ok(())
}

/// `spec.md:4274-4275`: *"one number is owned by one record of one type"*, against already-committed
/// records.
///
/// The batch cannot express this one (ADR-0030 F6 §A: in-process, a number minted as an entry cannot
/// *become* a task id without crossing a decode boundary), so the case has to cross that boundary
/// deliberately — which is also the shape of the real failure: a damaged or hand-edited store.
async fn one_number_is_owned_by_one_record_type(store: &mut dyn Storage) -> Result<(), Failure> {
    let cx = Cx::detached();
    seed_root(store, &cx).await?;
    let entry_id: EntryId = mint::<Entry>(store, &cx).await?;
    let message = kind("cyrup.message")?;

    let mut builder = BatchBuilder::new();
    builder
        .entry(EntryRecord {
            id: entry_id,
            conversation_id: ROOT_CONVERSATION_ID,
            kind: message,
            model: None,
            data: None,
            head: None,
            edits: None,
            by_task_id: None,
        })
        .map_err(|e| Failure::setup(e.to_string()))?;
    let batch = builder
        .build()
        .ok_or_else(|| Failure::setup("the batch was empty"))?;
    commit(store, batch, &cx).await?;

    // The same number, decoded as a task id. This is the only construction path there is, and it is
    // the path a corrupted record takes.
    let raw = crate::RawId::from(entry_id).get().get();
    let as_task: crate::TaskId = serde_json::from_str(&raw.to_string())
        .map_err(|e| Failure::setup(format!("decoding {raw} as a task id: {e}")))?;

    let mut builder = BatchBuilder::new();
    builder
        .task(crate::TaskRecord {
            id: as_task,
            conversation_id: ROOT_CONVERSATION_ID,
            kind: kind("cyrup.generation")?,
            version: DefVersion::FIRST,
            input: DocValue::Null,
            owner: None,
            background: false,
            abort_requested: false,
            started_at: None,
            ended_at: None,
            state: crate::TaskState::Pending {
                checkpoint: DocValue::Null,
                memos: None,
            },
        })
        .map_err(|e| Failure::setup(e.to_string()))?;
    let batch = builder
        .build()
        .ok_or_else(|| Failure::setup("the batch was empty"))?;
    match store.commit(batch, &cx).await {
        Err(CommitError::Rejected(RejectedReason::IdAlreadyOwned { owner, .. })) => {
            if owner != crate::IdKindTag::Entry {
                return Err(Failure::contract(format!(
                    "the rejection named the owner as {owner}, not entry"
                )));
            }
            Ok(())
        }
        Err(CommitError::Rejected(other)) => Err(Failure::contract(format!(
            "writing one number as two record types was rejected for the wrong reason: {other}"
        ))),
        Err(CommitError::Uncertain(e)) => Err(Failure::contract(format!(
            "a deterministic ownership failure was classified uncertain: {e}"
        ))),
        Ok(seq) => Err(Failure::contract(format!(
            "one number was written as two record types, at sequence {seq}"
        ))),
    }
}

/// `spec.md:4356-4357`: *"A numeric lookup of a known current-only incarnation rejects rather than
/// depending on reclaimed content."*
///
/// The arm that makes this a rejection and not an `Ok(None)` is
/// [`StorageFailure::HistoryNotRetained`](crate::StorageFailure::HistoryNotRetained): answering `None`
/// would say the incarnation was never there, and answering from whatever survived reclamation would
/// guess. Metadata membership stays queryable historically, which the case also checks.
async fn a_numeric_read_of_a_current_only_incarnation_is_refused(
    store: &mut dyn Storage,
) -> Result<(), Failure> {
    let cx = Cx::detached();
    seed_root(store, &cx).await?;
    let doc: DocumentId = mint::<Document>(store, &cx).await?;
    let kind = kind("cyrup.latest")?;
    let created = create_document(
        store,
        latest_doc(doc, kind.clone(), ROOT_CONVERSATION_ID),
        value("n", 1),
        Retire::Keep,
        &cx,
    )
    .await?;

    match store.document(doc, DocumentPoint::At(created), &cx).await {
        Err(crate::StorageFailure::HistoryNotRetained { document, .. }) => {
            if document != doc {
                return Err(Failure::contract(
                    "the refusal named a different incarnation",
                ));
            }
        }
        Err(other) => {
            return Err(Failure::contract(format!(
                "a numeric read of a current-only incarnation failed for the wrong reason: {other}"
            )));
        }
        Ok(None) => {
            return Err(Failure::contract(
                "a numeric read of a current-only incarnation answered absence, which says it was never there",
            ));
        }
        Ok(Some(_)) => {
            return Err(Failure::contract(
                "a numeric read of a current-only incarnation answered from content it does not retain",
            ));
        }
    }

    // Membership metadata is still answerable historically (`spec.md:4358`).
    let address = crate::DocumentAddress {
        kind,
        scope: crate::ScopeRef::Conversation(ROOT_CONVERSATION_ID),
        key: None,
    };
    if store
        .find_document(&address, DocumentPoint::At(created), &cx)
        .await?
        .is_none()
    {
        return Err(Failure::contract(
            "historical membership metadata was not queryable for a current-only incarnation",
        ));
    }
    Ok(())
}
