//! The query-plan suite (PICO5-PLAN S10): every named access path is answered from an index.
//!
//! # What this suite asserts, and why that is the right assertion
//!
//! `spec.md:4325-4351` gives storage a closed set of named access paths, and `spec.md:4337-4339` adds
//! the sentence that makes them an obligation: *"application-maintained registries are not a substitute
//! for these kernel indexes."* ADR-0030 §2.2 classifies the row **guarded** on the caller's side — the
//! trait has no general predicate, so a caller *cannot* answer these by scanning — and **checked** on
//! the backend's, with ADR-0030 §6 naming what checks it: *"nothing forces a backend to use an index
//! (query-plan and benchmark suites do that)."* This is that suite.
//!
//! Naming the index is not enough on its own, because a name is a label and a label can lie. So every
//! case here makes the same two-part assertion:
//!
//! 1. **the named index drove the read** — [`Plan::index`], which [`crate::query::Planned`] produces
//!    from the walk itself rather than alongside it, so it cannot describe a path that did not run; and
//! 2. **the walk did not grow** when history that cannot match the query was committed —
//!    [`Plan::visited`] before and after, identical. This is the property ADR-0030 §2.2 actually prices:
//!    *"if they degrade to scans, cost grows with total history: a long session gets slower until it is
//!    unusable."*
//!
//! The second half is what a scan cannot fake. A filtered walk over the whole table would return the
//! same records and could carry the same label; what it cannot do is visit the same number of index
//! entries after a hundred unrelated records land.
//!
//! # And the guarantee the indexes put at risk
//!
//! `G-COMMIT-VISIBLE-TO-LATER-READS` (`spec.md:4281`: *"once `commit()` resolves, later reads through
//! that Storage observe it"*) held trivially while every read walked a primary table. An index is a
//! second place the same fact lives, so it is a second place the fact can be missing — and the two ways
//! it goes wrong are a creation that is not filed and a **replacement** that is filed without its
//! predecessor being withdrawn (`spec.md:4383`: *"live task transitions replace one row"*). Both have
//! cases here, and so does the third: that the state a reopen rebuilds answers every path exactly as the
//! session that wrote it did.

use cyrup_pico_doc::{DefVersion, DocValue};
use cyrup_pico_store::{
    Conversation, ConversationId, ConversationOwner, ConversationParent, ConversationQuery,
    ConversationRecord, DocumentAddress, DocumentPoint, DocumentQuery, Entry, EntryId, EntryQuery,
    EntryRecord, InputSubmission, Kind, PageLimit, ROOT_CONVERSATION_ID, RawId, ScopeRef, Seq,
    Submission, SubmissionId, SubmissionQuery, SubmissionRecord, SubmissionState, SubmissionStatus,
    SubmissionType, Task, TaskId, TaskOutcome, TaskQuery, TaskRecord, TaskState, TaskStatus,
    WriteSubmission,
};

use super::fixture::{self, open_strong};
use crate::{Index, JsonlStore, Plan};

// ---------------------------------------------------------------- the two assertions ------------

/// A page limit, for a test that does not care which.
fn limit(n: u32) -> PageLimit {
    PageLimit::parse(n).expect("a page limit")
}

/// The index that drove a read.
fn drove(plan: Plan, index: Index) -> Plan {
    assert_eq!(
        plan.index, index,
        "this path must be answered from {index}, and was answered from {}",
        plan.index
    );
    plan
}

/// The assertion a scan cannot pass: the walk did not grow when unmatchable history landed.
///
/// Both halves matter. An index that changed between the two reads would mean the planner's choice
/// depends on history rather than on the query, and a `visited` that grew means the walk is reading
/// records it cannot return.
fn unmoved(before: Plan, after: Plan) {
    assert_eq!(
        before.index, after.index,
        "the driving index changed when unrelated history was committed: {} became {}",
        before.index, after.index
    );
    assert_eq!(
        before.visited, after.visited,
        "the walk grew from {} to {} index entries when history that cannot match was committed — \
         that is the degradation to a scan ADR-0030 §2.2 prices",
        before.visited, after.visited
    );
}

// ---------------------------------------------------------------- fixtures ----------------------

/// Commit one conversation, optionally forked from a parent at an entry, optionally task-owned.
async fn commit_conversation(
    store: &mut JsonlStore,
    parent: Option<(ConversationId, EntryId)>,
    owner: Option<(ConversationId, TaskId)>,
) -> ConversationId {
    let id: ConversationId = fixture::mint::<Conversation>(store).await;
    let mut builder = cyrup_pico_store::BatchBuilder::new();
    builder
        .conversation(ConversationRecord {
            id,
            parent: parent.map(|(conversation_id, at)| ConversationParent {
                conversation_id,
                at,
            }),
            owner: owner.map(|(conversation_id, task_id)| ConversationOwner {
                conversation_id,
                task_id,
            }),
        })
        .expect("staging a conversation");
    fixture::commit(store, builder.build().expect("a non-empty batch")).await;
    id
}

/// Commit one entry in a named conversation, with an optional head marker.
async fn commit_entry_in(
    store: &mut JsonlStore,
    conversation_id: ConversationId,
    head: Option<EntryId>,
) -> EntryId {
    let id: EntryId = fixture::mint::<Entry>(store).await;
    let mut builder = cyrup_pico_store::BatchBuilder::new();
    builder
        .entry(EntryRecord {
            id,
            conversation_id,
            kind: fixture::kind("cyrup.message"),
            model: None,
            data: None,
            head,
            edits: None,
            by_task_id: None,
        })
        .expect("staging an entry");
    fixture::commit(store, builder.build().expect("a non-empty batch")).await;
    id
}

/// Commit one task record, reusing `id` to exercise `spec.md:4383`'s row replacement.
async fn put_task(
    store: &mut JsonlStore,
    id: TaskId,
    conversation_id: ConversationId,
    kind: &str,
    state: TaskState,
    abort_requested: bool,
    background: bool,
) {
    let mut builder = cyrup_pico_store::BatchBuilder::new();
    builder
        .task(TaskRecord {
            id,
            conversation_id,
            kind: fixture::kind(kind),
            version: DefVersion::FIRST,
            input: DocValue::Null,
            owner: None,
            background,
            abort_requested,
            started_at: None,
            ended_at: None,
            state,
        })
        .expect("staging a task");
    fixture::commit(store, builder.build().expect("a non-empty batch")).await;
}

/// A live task state.
fn running() -> TaskState {
    TaskState::Running {
        checkpoint: DocValue::Null,
        memos: None,
    }
}

/// A terminal task state.
fn terminal() -> TaskState {
    TaskState::Terminal {
        outcome: TaskOutcome::Completed {
            result: DocValue::Null,
        },
    }
}

/// Commit one task of the default shape, returning its id.
async fn commit_task(store: &mut JsonlStore, conversation_id: ConversationId) -> TaskId {
    let id: TaskId = fixture::mint::<Task>(store).await;
    put_task(
        store,
        id,
        conversation_id,
        "cyrup.turn",
        running(),
        false,
        false,
    )
    .await;
    id
}

/// Commit one submission record, reusing `id` to exercise row replacement.
async fn put_submission(
    store: &mut JsonlStore,
    id: SubmissionId,
    conversation_id: ConversationId,
    request_id: Option<&str>,
    state: SubmissionState,
) {
    let mut builder = cyrup_pico_store::BatchBuilder::new();
    builder
        .submission(SubmissionRecord {
            id,
            conversation_id,
            request_id: request_id.map(ToOwned::to_owned),
            state,
        })
        .expect("staging a submission");
    fixture::commit(store, builder.build().expect("a non-empty batch")).await;
}

/// Commit one queued input submission, returning its id.
async fn commit_submission(
    store: &mut JsonlStore,
    conversation_id: ConversationId,
    request_id: Option<&str>,
) -> SubmissionId {
    let id: SubmissionId = fixture::mint::<Submission>(store).await;
    put_submission(
        store,
        id,
        conversation_id,
        request_id,
        SubmissionState::Input(InputSubmission::Queued),
    )
    .await;
    id
}

/// Create one document at an address in the root conversation, returning its id and that address.
async fn commit_document(
    store: &mut JsonlStore,
    kind: &str,
) -> (cyrup_pico_store::DocumentId, DocumentAddress) {
    let id = fixture::mint::<cyrup_pico_store::Document>(store).await;
    let create = fixture::latest_doc(id, fixture::kind(kind));
    let address = create.address();
    fixture::create_document(store, create, fixture::value("v", 1)).await;
    (id, address)
}

// ---------------------------------------------------------------- scan_entries ------------------

/// `spec.md:4334-4336`: *"`scanEntries()` pages the inclusive ID range in newest-first order while
/// applying every conversation ancestry cap."* One page must therefore cost one page, not one history.
#[tokio::test]
async fn an_entry_page_costs_one_page_whatever_the_history_behind_it() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    for _ in 0..20 {
        commit_entry_in(&mut store, ROOT_CONVERSATION_ID, None).await;
    }

    let query = EntryQuery::all(ROOT_CONVERSATION_ID);
    let (short, before) = store
        .reader()
        .plans()
        .scan_entries(&query, limit(5), None)
        .expect("the scan");
    let before = drove(before, Index::EntriesByConversation);
    assert_eq!(short.items.len(), 5, "a page is the limit");
    assert!(!short.is_last(), "there is more history behind it");

    // Eighty more entries, none of which the first page can contain.
    for _ in 0..80 {
        commit_entry_in(&mut store, ROOT_CONVERSATION_ID, None).await;
    }
    let (_, after) = store
        .reader()
        .plans()
        .scan_entries(&query, limit(5), None)
        .expect("the scan");
    unmoved(before, after);
    assert!(
        before.visited <= 8,
        "a five-entry page visited {} index entries: one ancestry level plus the page and its \
         look-ahead is the budget",
        before.visited
    );
}

/// The ordering and the bounds, which are contract rather than performance: newest-first, both bounds
/// inclusive (`spec.md:4334-4336`).
#[tokio::test]
async fn an_entry_scan_is_newest_first_over_an_inclusive_range() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let mut ids = Vec::new();
    for _ in 0..6 {
        ids.push(commit_entry_in(&mut store, ROOT_CONVERSATION_ID, None).await);
    }

    let (lo, hi) = (ids[1], ids[4]);
    let (page, plan) = store
        .reader()
        .plans()
        .scan_entries(
            &EntryQuery {
                conversation_id: ROOT_CONVERSATION_ID,
                min_entry_id: Some(lo),
                max_entry_id: Some(hi),
            },
            limit(10),
            None,
        )
        .expect("the scan");
    drove(plan, Index::EntriesByConversation);
    let got: Vec<EntryId> = page.items.iter().map(|e| e.id).collect();
    assert_eq!(
        got,
        vec![ids[4], ids[3], ids[2], ids[1]],
        "both bounds are inclusive and the order is newest first"
    );
    assert!(page.is_last(), "the range is exhausted");
    assert!(
        plan.visited <= 5,
        "a bounded range must not visit the entries outside it; visited {}",
        plan.visited
    );
}

/// The cap is applied **inside** (`spec.md:4334-4336`), which means a parent that grows *above* the fork
/// point costs the child's reads nothing — the case a filtered walk would get wrong while still
/// returning the right records.
#[tokio::test]
async fn a_parents_later_history_costs_a_forks_reads_nothing() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let mut parent_entries = Vec::new();
    for _ in 0..5 {
        parent_entries.push(commit_entry_in(&mut store, ROOT_CONVERSATION_ID, None).await);
    }
    let cap = parent_entries[2];
    let child = commit_conversation(&mut store, Some((ROOT_CONVERSATION_ID, cap)), None).await;
    let own = commit_entry_in(&mut store, child, None).await;

    let query = EntryQuery::all(child);
    let (page, before) = store
        .reader()
        .plans()
        .scan_entries(&query, limit(50), None)
        .expect("the scan");
    let before = drove(before, Index::EntriesByConversation);
    let got: Vec<EntryId> = page.items.iter().map(|e| e.id).collect();
    assert_eq!(
        got,
        vec![own, parent_entries[2], parent_entries[1], parent_entries[0]],
        "the child sees its own entries and the parent's up to the cap, newest first"
    );

    for _ in 0..60 {
        commit_entry_in(&mut store, ROOT_CONVERSATION_ID, None).await;
    }
    let (again, after) = store
        .reader()
        .plans()
        .scan_entries(&query, limit(50), None)
        .expect("the scan");
    unmoved(before, after);
    assert_eq!(
        again.items.iter().map(|e| e.id).collect::<Vec<_>>(),
        got,
        "and the answer is unchanged, because the cap only tightens going up"
    );
}

// ---------------------------------------------------- find_latest_head_marker -------------------

/// `spec.md:4331-4334`. The marker is *"the newest visible entry carrying `head` at or below its
/// optional inclusive cutoff"*, and a head index is what makes that a descent rather than a backwards
/// walk through every entry since the last head.
#[tokio::test]
async fn the_newest_head_marker_is_found_without_walking_the_entries_after_it() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let first = commit_entry_in(&mut store, ROOT_CONVERSATION_ID, None).await;
    let early_head = commit_entry_in(&mut store, ROOT_CONVERSATION_ID, Some(first)).await;
    let late_head = commit_entry_in(&mut store, ROOT_CONVERSATION_ID, Some(early_head)).await;

    let (found, before) = store
        .reader()
        .plans()
        .find_latest_head_marker(ROOT_CONVERSATION_ID, None)
        .expect("the lookup");
    let before = drove(before, Index::HeadsByConversation);
    let marker = found.expect("a head marker");
    assert_eq!(marker.entry.id, late_head, "the newest head wins");
    assert_eq!(marker.head, early_head, "and its bound comes with it");

    // A hundred entries that carry no head: not one of them is a candidate.
    for _ in 0..100 {
        commit_entry_in(&mut store, ROOT_CONVERSATION_ID, None).await;
    }
    let (again, after) = store
        .reader()
        .plans()
        .find_latest_head_marker(ROOT_CONVERSATION_ID, None)
        .expect("the lookup");
    unmoved(before, after);
    assert_eq!(
        again.expect("a head marker").entry.id,
        late_head,
        "and the answer is unchanged"
    );
    assert!(
        before.visited <= 2,
        "one ancestry level and one candidate is the budget; visited {}",
        before.visited
    );
}

/// The cutoff half, with the same invariance: heads committed *above* the cutoff are not candidates and
/// must not be visited.
#[tokio::test]
async fn a_head_lookup_below_a_cutoff_ignores_the_heads_above_it() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let first = commit_entry_in(&mut store, ROOT_CONVERSATION_ID, None).await;
    let head = commit_entry_in(&mut store, ROOT_CONVERSATION_ID, Some(first)).await;
    let cutoff = commit_entry_in(&mut store, ROOT_CONVERSATION_ID, None).await;

    let (found, before) = store
        .reader()
        .plans()
        .find_latest_head_marker(ROOT_CONVERSATION_ID, Some(cutoff))
        .expect("the lookup");
    let before = drove(before, Index::HeadsByConversation);
    assert_eq!(found.expect("a marker").entry.id, head);

    for _ in 0..40 {
        let previous = commit_entry_in(&mut store, ROOT_CONVERSATION_ID, None).await;
        commit_entry_in(&mut store, ROOT_CONVERSATION_ID, Some(previous)).await;
    }
    let (again, after) = store
        .reader()
        .plans()
        .find_latest_head_marker(ROOT_CONVERSATION_ID, Some(cutoff))
        .expect("the lookup");
    unmoved(before, after);
    assert_eq!(
        again.expect("a marker").entry.id,
        head,
        "forty newer heads are all above the cutoff"
    );
}

// ---------------------------------------------------- scan_conversations ------------------------

/// `spec.md:4337-4339`: *"Conversation owner filters are indexed and conjunctive. They support ownership
/// traversal without an all-conversation scan."*
#[tokio::test]
async fn an_owner_filtered_conversation_scan_visits_only_that_owners_conversations() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let owner: TaskId = fixture::mint::<Task>(&mut store).await;
    let mut owned = Vec::new();
    for _ in 0..3 {
        owned
            .push(commit_conversation(&mut store, None, Some((ROOT_CONVERSATION_ID, owner))).await);
    }

    let query = ConversationQuery {
        owner_conversation_id: None,
        owner_task_id: Some(owner),
    };
    let (page, before) = store
        .reader()
        .plans()
        .scan_conversations(&query, limit(50), None)
        .expect("the scan");
    let before = drove(before, Index::ConversationsByOwnerTask);
    assert_eq!(
        page.items.iter().map(|c| c.id).collect::<Vec<_>>(),
        owned,
        "ascending by id"
    );
    assert_eq!(
        before.visited, 3,
        "three owned conversations, three index entries visited"
    );

    for _ in 0..50 {
        commit_conversation(&mut store, None, None).await;
    }
    let (_, after) = store
        .reader()
        .plans()
        .scan_conversations(&query, limit(50), None)
        .expect("the scan");
    unmoved(before, after);
}

/// Conjunctive, and driven by the **smaller** of the two indexes — which is a choice made from the sets'
/// own lengths, so it depends on the session's shape rather than on an ordering baked into the code.
#[tokio::test]
async fn both_owner_filters_narrow_and_the_smaller_index_drives() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let one: TaskId = fixture::mint::<Task>(&mut store).await;
    let two: TaskId = fixture::mint::<Task>(&mut store).await;
    let wanted = commit_conversation(&mut store, None, Some((ROOT_CONVERSATION_ID, one))).await;
    for _ in 0..8 {
        commit_conversation(&mut store, None, Some((ROOT_CONVERSATION_ID, two))).await;
    }

    let (page, plan) = store
        .reader()
        .plans()
        .scan_conversations(
            &ConversationQuery {
                owner_conversation_id: Some(ROOT_CONVERSATION_ID),
                owner_task_id: Some(one),
            },
            limit(50),
            None,
        )
        .expect("the scan");
    // Nine conversations share the owning conversation; one shares the owning task, so the task index
    // is the selective one and is the one that drives.
    drove(plan, Index::ConversationsByOwnerTask);
    assert_eq!(
        page.items.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![wanted],
        "a conversation must match both filters"
    );
    assert_eq!(plan.visited, 1, "and only the selective index is walked");
}

/// A named owner nothing is filed under is the **empty** answer, and producing it visits nothing.
/// Answering it by walking the table would be the scan this suite exists to rule out.
#[tokio::test]
async fn an_owner_with_no_conversations_is_answered_without_a_walk() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    for _ in 0..20 {
        commit_conversation(&mut store, None, None).await;
    }
    let nobody: TaskId = fixture::mint::<Task>(&mut store).await;

    let (page, plan) = store
        .reader()
        .plans()
        .scan_conversations(
            &ConversationQuery {
                owner_conversation_id: None,
                owner_task_id: Some(nobody),
            },
            limit(50),
            None,
        )
        .expect("the scan");
    drove(plan, Index::ConversationsByOwnerTask);
    assert!(page.items.is_empty(), "nothing is owned by that task");
    assert_eq!(plan.visited, 0, "and nothing was visited to find out");
}

// ---------------------------------------------------- scan_tasks -------------------------------

/// `spec.md:4348`: *"Task queries support conversation, kind, live/terminal status, abort mark, and
/// background status."* All five are indexed, so each of the five drives on its own.
#[tokio::test]
async fn each_of_the_five_task_fields_drives_an_index_of_its_own() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let elsewhere = commit_conversation(&mut store, None, None).await;
    let wanted: TaskId = fixture::mint::<Task>(&mut store).await;
    put_task(
        &mut store,
        wanted,
        ROOT_CONVERSATION_ID,
        "cyrup.compact",
        terminal(),
        true,
        true,
    )
    .await;
    // Forty tasks that differ from it in every one of the five fields.
    for _ in 0..40 {
        commit_task(&mut store, elsewhere).await;
    }

    let probes: [(TaskQuery, Index); 5] = [
        (
            TaskQuery {
                conversation_id: Some(ROOT_CONVERSATION_ID),
                ..TaskQuery::default()
            },
            Index::TasksByConversation,
        ),
        (
            TaskQuery {
                kind: Some(fixture::kind("cyrup.compact")),
                ..TaskQuery::default()
            },
            Index::TasksByKind,
        ),
        (
            TaskQuery {
                status: Some(TaskStatus::Terminal),
                ..TaskQuery::default()
            },
            Index::TasksByStatus,
        ),
        (
            TaskQuery {
                abort_requested: Some(true),
                ..TaskQuery::default()
            },
            Index::TasksByAbortMark,
        ),
        (
            TaskQuery {
                background: Some(true),
                ..TaskQuery::default()
            },
            Index::TasksByBackground,
        ),
    ];
    for (query, index) in probes {
        let (page, plan) = store
            .reader()
            .plans()
            .scan_tasks(&query, limit(50), None)
            .expect("the scan");
        drove(plan, index);
        assert_eq!(
            page.items.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![wanted],
            "{index} must select exactly the one task that matches"
        );
        assert_eq!(
            plan.visited, 1,
            "{index} must visit one entry, not the forty that cannot match"
        );
    }
}

/// The polarity that a *"tasks with an abort mark"* index would get wrong: `Some(false)` has to narrow
/// too, which is why the two booleans are indexed by partition.
#[tokio::test]
async fn the_false_half_of_a_boolean_task_filter_narrows_as_well_as_the_true_half() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let foreground: TaskId = fixture::mint::<Task>(&mut store).await;
    put_task(
        &mut store,
        foreground,
        ROOT_CONVERSATION_ID,
        "cyrup.turn",
        running(),
        false,
        false,
    )
    .await;
    for _ in 0..30 {
        let id: TaskId = fixture::mint::<Task>(&mut store).await;
        put_task(
            &mut store,
            id,
            ROOT_CONVERSATION_ID,
            "cyrup.turn",
            running(),
            false,
            true,
        )
        .await;
    }

    let (page, plan) = store
        .reader()
        .plans()
        .scan_tasks(
            &TaskQuery {
                background: Some(false),
                ..TaskQuery::default()
            },
            limit(50),
            None,
        )
        .expect("the scan");
    drove(plan, Index::TasksByBackground);
    assert_eq!(
        page.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![foreground]
    );
    assert_eq!(
        plan.visited, 1,
        "the thirty background tasks are not candidates"
    );
}

/// `spec.md:4383`: *"live task transitions replace one row."* The index has to withdraw the old row's
/// memberships in the same statement that files the new ones, or a transitioned task answers two
/// contradictory queries — the quietest way an index can be wrong.
#[tokio::test]
async fn a_transitioned_task_leaves_its_old_status_index_behind() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let id: TaskId = fixture::mint::<Task>(&mut store).await;
    put_task(
        &mut store,
        id,
        ROOT_CONVERSATION_ID,
        "cyrup.turn",
        running(),
        false,
        false,
    )
    .await;

    let live = TaskQuery {
        status: Some(TaskStatus::Running),
        ..TaskQuery::default()
    };
    let done = TaskQuery {
        status: Some(TaskStatus::Terminal),
        ..TaskQuery::default()
    };
    let (page, _) = store
        .reader()
        .plans()
        .scan_tasks(&live, limit(10), None)
        .expect("the scan");
    assert_eq!(page.items.len(), 1, "it starts running");

    // The same id, now terminal, and with its abort mark set.
    put_task(
        &mut store,
        id,
        ROOT_CONVERSATION_ID,
        "cyrup.turn",
        terminal(),
        true,
        false,
    )
    .await;

    let (still_live, plan) = store
        .reader()
        .plans()
        .scan_tasks(&live, limit(10), None)
        .expect("the scan");
    assert!(
        still_live.items.is_empty(),
        "a terminal task must not still answer a live query"
    );
    assert_eq!(
        plan.visited, 0,
        "and the withdrawn membership must be gone from the index, not merely filtered out of the \
         answer"
    );
    let (now_done, _) = store
        .reader()
        .plans()
        .scan_tasks(&done, limit(10), None)
        .expect("the scan");
    assert_eq!(
        now_done.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![id],
        "it answers exactly one status query, and this is it"
    );
    let (aborting, _) = store
        .reader()
        .plans()
        .scan_tasks(
            &TaskQuery {
                abort_requested: Some(false),
                ..TaskQuery::default()
            },
            limit(10),
            None,
        )
        .expect("the scan");
    assert!(
        aborting.items.is_empty(),
        "and the same holds for the boolean partitions"
    );
}

// ---------------------------------------------------- submissions ------------------------------

/// `spec.md:4314`: `submissionByRequest` is an exact lookup, so it costs one probe whatever the
/// conversation holds — and it is scoped to the conversation, which a single global map of request ids
/// would get wrong.
#[tokio::test]
async fn a_submission_request_lookup_is_one_probe_scoped_to_its_conversation() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let other = commit_conversation(&mut store, None, None).await;
    let wanted = commit_submission(&mut store, ROOT_CONVERSATION_ID, Some("req-7")).await;
    let mut elsewhere_with_same_key = None;
    for n in 0..30 {
        let id = commit_submission(&mut store, other, Some(&format!("req-{n}"))).await;
        if n == 7 {
            elsewhere_with_same_key = Some(id);
        }
    }

    let (found, plan) = store
        .reader()
        .plans()
        .submission_by_request(ROOT_CONVERSATION_ID, "req-7")
        .expect("the lookup");
    drove(plan, Index::SubmissionsByRequest);
    assert_eq!(found.expect("the submission").id, wanted);
    assert_eq!(plan.visited, 1, "one probe");

    // The same request id in another conversation is another submission. An index keyed by the request
    // id alone would answer this with the first one it filed, which is the bug the nesting prevents.
    let (elsewhere, _) = store
        .reader()
        .plans()
        .submission_by_request(other, "req-7")
        .expect("the lookup");
    assert_eq!(
        elsewhere.map(|s| s.id),
        elsewhere_with_same_key,
        "the lookup is scoped to its conversation"
    );
}

/// The three submission filters, each indexed, with the type filter being the one upstream's
/// `SubmissionQuery` cannot express without naming a status.
#[tokio::test]
async fn a_submission_scan_is_driven_by_whichever_of_its_three_filters_is_selective() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let writer: SubmissionId = fixture::mint::<Submission>(&mut store).await;
    put_submission(
        &mut store,
        writer,
        ROOT_CONVERSATION_ID,
        None,
        SubmissionState::Write(WriteSubmission::Queued),
    )
    .await;
    for _ in 0..25 {
        commit_submission(&mut store, ROOT_CONVERSATION_ID, None).await;
    }

    let (page, plan) = store
        .reader()
        .plans()
        .scan_submissions(
            &SubmissionQuery {
                conversation_id: Some(ROOT_CONVERSATION_ID),
                status: Some(SubmissionStatus::Queued),
                submission_type: Some(SubmissionType::Write),
            },
            limit(50),
            None,
        )
        .expect("the scan");
    // Twenty-six share the conversation and the status; one is a write, so the type index drives.
    drove(plan, Index::SubmissionsByType);
    assert_eq!(
        page.items.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![writer]
    );
    assert_eq!(plan.visited, 1);
}

// ---------------------------------------------------- documents --------------------------------

/// `spec.md:4340-4342`: *"`findDocument()` resolves one exact logical kind/scope/key address."* An
/// address index is what makes *"exact"* a probe; without one, resolving an address is a walk of every
/// incarnation the store has ever held.
#[tokio::test]
async fn an_address_resolves_without_visiting_the_documents_at_other_addresses() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let (wanted, address) = commit_document(&mut store, "cyrup.plan").await;

    let (found, before) = store
        .reader()
        .plans()
        .find_document(&address, DocumentPoint::Current)
        .expect("the lookup");
    let before = drove(before, Index::DocumentsByAddress);
    assert_eq!(found.expect("the record").id, wanted);
    assert_eq!(
        before.visited, 1,
        "one incarnation has ever had that address"
    );

    for n in 0..40 {
        commit_document(&mut store, &format!("cyrup.other{n}")).await;
    }
    let (again, after) = store
        .reader()
        .plans()
        .find_document(&address, DocumentPoint::Current)
        .expect("the lookup");
    unmoved(before, after);
    assert_eq!(again.expect("the record").id, wanted);
}

/// `spec.md:4342-4345`: *"`scanDocuments()` enumerates only the incarnations alive in one exact scope at
/// its selected point."* The scope is the index; another scope's documents are not candidates.
#[tokio::test]
async fn a_document_scan_is_bounded_by_its_scope() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let (first, _) = commit_document(&mut store, "cyrup.plan").await;
    let (second, _) = commit_document(&mut store, "cyrup.notes").await;

    let query = DocumentQuery {
        scope: ScopeRef::Conversation(ROOT_CONVERSATION_ID),
        at: DocumentPoint::Current,
        kind: None,
    };
    let (page, before) = store
        .reader()
        .plans()
        .scan_documents(&query, limit(50), None)
        .expect("the scan");
    let before = drove(before, Index::DocumentsByScope);
    assert_eq!(
        page.items.iter().map(|d| d.id).collect::<Vec<_>>(),
        vec![first, second],
        "ascending incarnation ids (`spec.md:4344`)"
    );

    // Thirty session-scoped documents, in a different scope entirely.
    for n in 0..30 {
        let id = fixture::mint::<cyrup_pico_store::Document>(&mut store).await;
        let create = cyrup_pico_store::DocumentCreate {
            id,
            kind: fixture::kind(&format!("cyrup.session{n}")),
            key: None,
            scope: cyrup_pico_store::DocumentScope::Session,
        };
        fixture::create_document(&mut store, create, fixture::value("v", 1)).await;
    }
    let (_, after) = store
        .reader()
        .plans()
        .scan_documents(&query, limit(50), None)
        .expect("the scan");
    unmoved(before, after);
}

/// `spec.md:4350`: *"The lookup never scans unrelated documents."* `document(id, at)` is a keyed probe
/// followed by one byte span, so the plan is the primary table and the number is one — and that is a
/// statement about there being no iteration in the function at all.
#[tokio::test]
async fn materializing_one_incarnation_is_a_keyed_probe() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let (wanted, _) = commit_document(&mut store, "cyrup.plan").await;
    for n in 0..20 {
        commit_document(&mut store, &format!("cyrup.other{n}")).await;
    }

    let (found, plan) = store
        .reader()
        .plans()
        .document(wanted, DocumentPoint::Current)
        .expect("the read");
    drove(plan, Index::Primary);
    assert_eq!(plan.visited, 1);
    assert_eq!(found.expect("the document").record.id, wanted);
}

// ------------------------------------------- read-your-commit, and the reopen -------------------

/// `spec.md:4281`: *"Once `commit()` resolves, later reads through that Storage observe it."* Trivial
/// while every read walked a primary table; with indexes it is a claim about every index being written
/// by the commit that establishes the fact. So: commit one of each record, then read it back through
/// each of the paths an index now answers.
#[tokio::test]
async fn a_commit_is_visible_through_every_indexed_path_immediately() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let owner: TaskId = fixture::mint::<Task>(&mut store).await;

    let conversation =
        commit_conversation(&mut store, None, Some((ROOT_CONVERSATION_ID, owner))).await;
    let (owned, _) = store
        .reader()
        .plans()
        .scan_conversations(
            &ConversationQuery {
                owner_conversation_id: None,
                owner_task_id: Some(owner),
            },
            limit(10),
            None,
        )
        .expect("the scan");
    assert_eq!(
        owned.items.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![conversation],
        "the owner index saw the commit that resolved"
    );

    let first = commit_entry_in(&mut store, conversation, None).await;
    let head = commit_entry_in(&mut store, conversation, Some(first)).await;
    let (entries, _) = store
        .reader()
        .plans()
        .scan_entries(&EntryQuery::all(conversation), limit(10), None)
        .expect("the scan");
    assert_eq!(
        entries.items.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![head, first]
    );
    assert_eq!(
        store
            .reader()
            .plans()
            .find_latest_head_marker(conversation, None)
            .expect("the lookup")
            .0
            .expect("a marker")
            .entry
            .id,
        head,
        "the head index saw it too"
    );

    let task = commit_task(&mut store, conversation).await;
    let (tasks, _) = store
        .reader()
        .plans()
        .scan_tasks(
            &TaskQuery {
                conversation_id: Some(conversation),
                ..TaskQuery::default()
            },
            limit(10),
            None,
        )
        .expect("the scan");
    assert_eq!(
        tasks.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![task]
    );

    let submission = commit_submission(&mut store, conversation, Some("r")).await;
    assert_eq!(
        store
            .reader()
            .plans()
            .submission_by_request(conversation, "r")
            .expect("the lookup")
            .0
            .expect("the submission")
            .id,
        submission
    );

    let (document, address) = commit_document(&mut store, "cyrup.plan").await;
    assert_eq!(
        store
            .reader()
            .plans()
            .find_document(&address, DocumentPoint::Current)
            .expect("the lookup")
            .0
            .expect("the record")
            .id,
        document
    );
}

/// The drift case. [`crate::state`]'s one application path means a reopen and a live commit build the
/// indexes through the same function; this is the test that would go red the day a second path appeared,
/// because it compares what the writing session answers with what the reopened store answers, through
/// every index.
#[tokio::test]
async fn the_indexes_a_reopen_builds_answer_exactly_as_the_session_that_wrote_them() {
    let dir = fixture::dir();
    let owner;
    let conversation;
    let before;
    {
        let mut store = open_strong(dir.path());
        fixture::seed_root(&mut store).await;
        owner = fixture::mint::<Task>(&mut store).await;
        conversation =
            commit_conversation(&mut store, None, Some((ROOT_CONVERSATION_ID, owner))).await;
        let first = commit_entry_in(&mut store, conversation, None).await;
        commit_entry_in(&mut store, conversation, Some(first)).await;
        let task: TaskId = fixture::mint::<Task>(&mut store).await;
        put_task(
            &mut store,
            task,
            conversation,
            "cyrup.turn",
            running(),
            false,
            true,
        )
        .await;
        // Replaced, so the reopen has to rebuild the withdrawal as well as the membership.
        put_task(
            &mut store,
            task,
            conversation,
            "cyrup.turn",
            terminal(),
            false,
            true,
        )
        .await;
        commit_submission(&mut store, conversation, Some("r")).await;
        commit_document(&mut store, "cyrup.plan").await;
        before = snapshot(&store, conversation, owner).await;
    }

    let store = open_strong(dir.path());
    let after = snapshot(&store, conversation, owner).await;
    assert_eq!(
        before, after,
        "a reopened store must answer every indexed path exactly as the session that wrote the \
         commits did — including the status a replaced task is no longer in"
    );
}

/// Every indexed path's answer and plan, as one comparable value.
async fn snapshot(
    store: &JsonlStore,
    conversation: ConversationId,
    owner: TaskId,
) -> Vec<(String, Plan)> {
    let plans = store.reader().plans();
    let mut out = Vec::new();
    let (conversations, plan) = plans
        .scan_conversations(
            &ConversationQuery {
                owner_conversation_id: None,
                owner_task_id: Some(owner),
            },
            limit(50),
            None,
        )
        .expect("the scan");
    out.push((
        format!(
            "conversations by owner: {:?}",
            conversations
                .items
                .iter()
                .map(|c| RawId::from(c.id))
                .collect::<Vec<_>>()
        ),
        plan,
    ));
    let (entries, plan) = plans
        .scan_entries(&EntryQuery::all(conversation), limit(50), None)
        .expect("the scan");
    out.push((
        format!(
            "entries: {:?}",
            entries
                .items
                .iter()
                .map(|e| RawId::from(e.id))
                .collect::<Vec<_>>()
        ),
        plan,
    ));
    let (marker, plan) = plans
        .find_latest_head_marker(conversation, None)
        .expect("the lookup");
    out.push((
        format!("head marker: {:?}", marker.map(|m| RawId::from(m.entry.id))),
        plan,
    ));
    for status in [TaskStatus::Running, TaskStatus::Terminal] {
        let (tasks, plan) = plans
            .scan_tasks(
                &TaskQuery {
                    status: Some(status),
                    ..TaskQuery::default()
                },
                limit(50),
                None,
            )
            .expect("the scan");
        out.push((
            format!(
                "tasks {status:?}: {:?}",
                tasks
                    .items
                    .iter()
                    .map(|t| RawId::from(t.id))
                    .collect::<Vec<_>>()
            ),
            plan,
        ));
    }
    let (submission, plan) = plans
        .submission_by_request(conversation, "r")
        .expect("the lookup");
    out.push((
        format!(
            "submission by request: {:?}",
            submission.map(|s| RawId::from(s.id))
        ),
        plan,
    ));
    let (documents, plan) = plans
        .scan_documents(
            &DocumentQuery {
                scope: ScopeRef::Conversation(ROOT_CONVERSATION_ID),
                at: DocumentPoint::Current,
                kind: None,
            },
            limit(50),
            None,
        )
        .expect("the scan");
    out.push((
        format!(
            "documents in scope: {:?}",
            documents
                .items
                .iter()
                .map(|d| RawId::from(d.id))
                .collect::<Vec<_>>()
        ),
        plan,
    ));
    out
}

// ---------------------------------------------------- paging -----------------------------------

/// A cursor resumes **strictly** below the page it ended, and the resumed page costs one page as well —
/// so paging deep into a history is not quadratic in it. The bound is what a cursor decoded to a bare
/// number buys through `Id<K>`'s `Borrow<NonZeroU64>`; without it, resuming is a walk from the top.
#[tokio::test]
async fn a_resumed_entry_page_costs_one_page_not_its_offset() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let mut ids = Vec::new();
    for _ in 0..60 {
        ids.push(commit_entry_in(&mut store, ROOT_CONVERSATION_ID, None).await);
    }
    ids.reverse();

    let query = EntryQuery::all(ROOT_CONVERSATION_ID);
    let (first, first_plan) = store
        .reader()
        .plans()
        .scan_entries(&query, limit(5), None)
        .expect("the scan");
    let cursor = first.next.clone().expect("more history");
    let (second, second_plan) = store
        .reader()
        .plans()
        .scan_entries(&query, limit(5), Some(&cursor))
        .expect("the scan");

    assert_eq!(
        first.items.iter().map(|e| e.id).collect::<Vec<_>>(),
        ids[..5].to_vec()
    );
    assert_eq!(
        second.items.iter().map(|e| e.id).collect::<Vec<_>>(),
        ids[5..10].to_vec(),
        "the second page resumes strictly below the first"
    );
    assert_eq!(
        first_plan.visited, second_plan.visited,
        "and it costs the same, fifty-five entries deeper in"
    );
}

/// The same for an ascending scan, where the cursor is a lower bound.
#[tokio::test]
async fn a_resumed_document_page_costs_one_page_not_its_offset() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let mut ids = Vec::new();
    for n in 0..20 {
        let (id, _) = commit_document(&mut store, &format!("cyrup.d{n}")).await;
        ids.push(id);
    }

    let query = DocumentQuery {
        scope: ScopeRef::Conversation(ROOT_CONVERSATION_ID),
        at: DocumentPoint::Current,
        kind: None,
    };
    let (first, first_plan) = store
        .reader()
        .plans()
        .scan_documents(&query, limit(4), None)
        .expect("the scan");
    let cursor = first.next.clone().expect("more documents");
    let (second, second_plan) = store
        .reader()
        .plans()
        .scan_documents(&query, limit(4), Some(&cursor))
        .expect("the scan");
    assert_eq!(
        first.items.iter().map(|d| d.id).collect::<Vec<_>>(),
        ids[..4].to_vec()
    );
    assert_eq!(
        second.items.iter().map(|d| d.id).collect::<Vec<_>>(),
        ids[4..8].to_vec()
    );
    assert_eq!(first_plan.visited, second_plan.visited);
}

// ---------------------------------------------------- the measurement surface -------------------

/// The footprint accounting is a measurement, so this pins what it measures rather than a number: both
/// halves grow with the store, and the record half is the larger one — which is the finding ADR-0030 §9's
/// *"tens of bytes per record"* model does not predict, because that model describes the index half only.
#[tokio::test]
async fn the_footprint_reports_both_halves_and_the_records_are_the_larger_one() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let empty = store.reader().plans().footprint();

    for n in 0..50 {
        let id: EntryId = fixture::mint::<Entry>(&mut store).await;
        let mut builder = cyrup_pico_store::BatchBuilder::new();
        builder
            .entry(EntryRecord {
                id,
                conversation_id: ROOT_CONVERSATION_ID,
                kind: fixture::kind("cyrup.message"),
                model: None,
                // A realistic entry carries a payload, which is what makes the record half dominate.
                data: Some(DocValue::string(&format!("{n}: {}", "x".repeat(400)))),
                head: None,
                edits: None,
                by_task_id: None,
            })
            .expect("staging an entry");
        fixture::commit(&mut store, builder.build().expect("a batch")).await;
    }
    let full = store.reader().plans().footprint();

    assert!(
        full.index_bytes > empty.index_bytes,
        "the index half grows with the store"
    );
    assert!(
        full.record_bytes > full.index_bytes,
        "the resident records are the larger half ({} record bytes against {} index bytes), which is \
         the half ADR-0030 §9's model leaves out",
        full.record_bytes,
        full.index_bytes
    );
    assert_eq!(
        full.records, 51,
        "the root conversation and fifty entries, with no documents"
    );
    assert_eq!(
        full.commits,
        store.reader().commit_count(),
        "the footprint's commit count is the store's"
    );
    assert_eq!(full.total_bytes(), full.index_bytes + full.record_bytes);
    assert!(
        store.reader().last_commit() > Some(Seq::FIRST),
        "and the store did commit"
    );
}

/// A `Kind` the fixture builds is the same `Kind` an address carries: the address index is keyed by the
/// whole address, so a family key is part of the key and a keyless singleton is a different address from
/// a keyed member (`spec.md:4341-4342`).
#[tokio::test]
async fn a_singleton_and_a_family_member_are_different_addresses() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let kind: Kind = fixture::kind("cyrup.plan");
    let singleton = fixture::mint::<cyrup_pico_store::Document>(&mut store).await;
    fixture::create_document(
        &mut store,
        fixture::latest_doc(singleton, kind.clone()),
        fixture::value("v", 1),
    )
    .await;
    let keyed = fixture::mint::<cyrup_pico_store::Document>(&mut store).await;
    let mut create = fixture::latest_doc(keyed, kind.clone());
    create.key = Some("a".to_owned());
    fixture::create_document(&mut store, create, fixture::value("v", 2)).await;

    let address = DocumentAddress {
        kind,
        scope: ScopeRef::Conversation(ROOT_CONVERSATION_ID),
        key: None,
    };
    let (found, plan) = store
        .reader()
        .plans()
        .find_document(&address, DocumentPoint::Current)
        .expect("the lookup");
    drove(plan, Index::DocumentsByAddress);
    assert_eq!(
        found.expect("the singleton").id,
        singleton,
        "a missing key means the singleton, not every family member"
    );
    assert_eq!(
        plan.visited, 1,
        "and the keyed member is not even a candidate"
    );
}
