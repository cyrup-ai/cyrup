//! What every test in this crate needs: a real directory, a real store, and small batches.
//!
//! There is no filesystem double anywhere in this suite, deliberately. The slice's whole subject is what
//! the medium does — a torn line, an unconfirmed tail, a `flock` another process holds, a directory
//! entry that has to be flushed — and a double would be a model of the medium whose agreement with the
//! medium is the thing in doubt.

use std::path::Path;
use std::sync::Arc;

use cyrup_pico_doc::{DefVersion, DocMap, DocRoot, DocValue, Op, OpBatch, Path as DocPath};
use cyrup_pico_store::{
    Batch, BatchBuilder, CommitError, ConversationRecord, ConversationSemantics, Cx, DocumentBase,
    DocumentContent, DocumentCreate, DocumentId, DocumentPoint, DocumentScope, Entry, EntryId,
    EntryRecord, Id, IdKind, Kind, LatestFork, ROOT_CONVERSATION_ID, Retire, RewindableFork, Seq,
    Storage, StorageExt, StoredDocument,
};
use tempfile::TempDir;

use crate::{Durability, JsonlOptions, JsonlStore, StoreLock};

/// A store directory that is deleted when the test ends.
pub(crate) fn dir() -> TempDir {
    tempfile::tempdir().expect("a temp directory")
}

/// Open `dir` for writing at one tier.
pub(crate) fn open(dir: &Path, durability: Durability) -> JsonlStore {
    let lock = StoreLock::acquire(dir).expect("the store lock");
    JsonlStore::open_for_write(dir, lock, JsonlOptions::new(durability)).expect("the store")
}

/// Open `dir` for writing at the strong tier, which is the default this suite asserts against.
pub(crate) fn open_strong(dir: &Path) -> JsonlStore {
    open(dir, Durability::PowerLoss)
}

/// A detached context: nothing in this backend reads one, and `MemoryStore` does not either.
pub(crate) fn cx() -> Cx {
    Cx::detached()
}

/// Parse a kind.
pub(crate) fn kind(s: &str) -> Kind {
    Kind::parse(s).expect("a valid kind")
}

/// A one-key document value.
pub(crate) fn value(key: &str, n: i64) -> DocRoot {
    let mut map = DocMap::new();
    map.insert(Arc::from(key), DocValue::integer(n));
    DocRoot::from_map(map)
}

/// Commit the root conversation, so documents have a conversation scope to live in.
pub(crate) async fn seed_root(store: &mut JsonlStore) -> Seq {
    let mut builder = BatchBuilder::new();
    builder
        .conversation(ConversationRecord {
            id: ROOT_CONVERSATION_ID,
            parent: None,
            owner: None,
        })
        .expect("staging the root conversation");
    commit(store, builder.build().expect("a non-empty batch")).await
}

/// Commit one batch, failing the test on a rejection.
pub(crate) async fn commit(store: &mut JsonlStore, batch: Batch) -> Seq {
    store.commit(batch, &cx()).await.expect("the commit")
}

/// Mint one id.
pub(crate) async fn mint<K: IdKind>(store: &mut JsonlStore) -> Id<K> {
    store.mint::<K>(&cx()).await.expect("a minted id")
}

/// Commit one entry in the root conversation, returning its id and the commit sequence.
pub(crate) async fn commit_entry(store: &mut JsonlStore, text: &str) -> (EntryId, Seq) {
    let id: EntryId = mint::<Entry>(store).await;
    let mut builder = BatchBuilder::new();
    builder
        .entry(EntryRecord {
            id,
            conversation_id: ROOT_CONVERSATION_ID,
            kind: kind("cyrup.message"),
            model: None,
            data: Some(DocValue::string(text)),
            head: None,
            edits: None,
            by_task_id: None,
        })
        .expect("staging an entry");
    let seq = commit(store, builder.build().expect("a non-empty batch")).await;
    (id, seq)
}

/// A `history: "latest"` conversation document creation.
pub(crate) fn latest_doc(id: DocumentId, kind: Kind) -> DocumentCreate {
    DocumentCreate {
        id,
        kind,
        key: None,
        scope: DocumentScope::Conversation {
            conversation_id: ROOT_CONVERSATION_ID,
            semantics: ConversationSemantics::Latest {
                fork: LatestFork::Current,
            },
        },
    }
}

/// A `history: "rewindable"` conversation document creation.
pub(crate) fn rewindable_doc(id: DocumentId, kind: Kind) -> DocumentCreate {
    DocumentCreate {
        id,
        kind,
        key: None,
        scope: DocumentScope::Conversation {
            conversation_id: ROOT_CONVERSATION_ID,
            semantics: ConversationSemantics::Rewindable {
                fork: RewindableFork::Current,
            },
        },
    }
}

/// Create one document with a complete base.
pub(crate) async fn create_document(
    store: &mut JsonlStore,
    record: DocumentCreate,
    base: DocRoot,
) -> Seq {
    let mut builder = BatchBuilder::new();
    builder
        .create_document(
            record,
            DocumentBase {
                version: DefVersion::FIRST,
                value: base,
            },
            Retire::Keep,
        )
        .expect("staging a creation");
    commit(store, builder.build().expect("a non-empty batch")).await
}

/// Append one delta setting `key` to `n`, continuing the version the store reports.
pub(crate) async fn append_delta(store: &mut JsonlStore, id: DocumentId, key: &str, n: i64) -> Seq {
    let before = read(store, id).await;
    let mut builder = BatchBuilder::new();
    builder
        .change_document(
            id,
            DocumentContent::Delta {
                continues: before.version,
                ops: OpBatch::new(vec![Op::Set {
                    path: DocPath::keys([key]).expect("a one-key path"),
                    value: DocValue::integer(n),
                }]),
            },
            Retire::Keep,
        )
        .expect("staging a delta");
    commit(store, builder.build().expect("a non-empty batch")).await
}

/// Write a complete base over an existing document.
pub(crate) async fn write_base(store: &mut JsonlStore, id: DocumentId, base: DocRoot) -> Seq {
    let mut builder = BatchBuilder::new();
    builder
        .change_document(
            id,
            DocumentContent::Base(DocumentBase {
                version: DefVersion::FIRST,
                value: base,
            }),
            Retire::Keep,
        )
        .expect("staging a base");
    commit(store, builder.build().expect("a non-empty batch")).await
}

/// Read one document at `current`, failing the test if it is absent.
pub(crate) async fn read(store: &JsonlStore, id: DocumentId) -> StoredDocument {
    store
        .document(id, DocumentPoint::Current, &cx())
        .await
        .expect("the read")
        .expect("the document")
}

/// Assert a commit failed in the uncertain class, returning its message.
pub(crate) fn uncertain(e: CommitError) -> String {
    match e {
        CommitError::Uncertain(u) => u.to_string(),
        CommitError::Rejected(r) => panic!("expected an uncertain commit, got a rejection: {r}"),
    }
}

/// Assert a commit failed in the rejected class.
pub(crate) fn rejected(e: CommitError) -> String {
    match e {
        CommitError::Rejected(r) => r.to_string(),
        CommitError::Uncertain(u) => panic!("expected a rejection, got an uncertain commit: {u}"),
    }
}

/// Read the lines of `main.jsonl`.
pub(crate) fn main_lines(dir: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(crate::wire::main_path(dir)).expect("main.jsonl");
    text.lines().map(ToOwned::to_owned).collect()
}

/// Replace `main.jsonl` with these lines, each newline-terminated.
pub(crate) fn write_main_lines(dir: &Path, lines: &[String]) {
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    std::fs::write(crate::wire::main_path(dir), text).expect("rewriting main.jsonl");
}

/// Append raw bytes to `main.jsonl`, with no newline of their own: a torn write.
pub(crate) fn append_raw(dir: &Path, bytes: &[u8]) {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(crate::wire::main_path(dir))
        .expect("main.jsonl");
    f.write_all(bytes).expect("appending raw bytes");
}

/// One document's sidecar path at a generation.
pub(crate) fn sidecar(dir: &Path, id: DocumentId, generation: u32) -> std::path::PathBuf {
    crate::wire::sidecar_path(dir, id, generation)
}

/// Cut a file to `len`, as a power cut might.
pub(crate) fn truncate(path: &Path, len: u64) {
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("the file");
    f.set_len(len).expect("truncating");
}
