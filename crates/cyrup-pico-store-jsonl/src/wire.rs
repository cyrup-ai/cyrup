//! The on-disk shapes: `main.jsonl`'s line kinds, a sidecar's content line, and the file names.
//!
//! # The layout, and where it follows ADR-0030 rather than `spec.md`
//!
//! ```text
//! store.json              the durable identity (crate::identity)
//! store.lock              the exclusive lock (crate::lock)
//! main.jsonl              every table record, every document record, one marker per commit
//! doc-<id>-g<gen>.jsonl   one document incarnation's content records
//! ```
//!
//! `spec.md:4406-4410` draws three file kinds, the third being `task-<id>.jsonl` for *"live task
//! replacements"*. **ADR-0030 §9 draws two**, and this backend follows ADR-0030: *"`main.jsonl` holds
//! table writes, document records and one marker per commit; sidecars hold content"*, from which §9
//! derives the property that prices the whole design — *"the sidecars are never read at open … so open
//! is O(commits + table records)"*. A task sidecar would be read at open, because the five-field task
//! query (`spec.md:4348`) is answered from an in-memory index that has to exist before the first query.
//!
//! **[CYRUP-DELTA]** A task's replacements therefore live in `main.jsonl` rather than in a sidecar of
//! their own. Same guarantee — `spec.md:4383`'s *"live task transitions replace one row"*, with the
//! newest line winning — and the same bytes read at open either way, since a sidecar's tail cannot be
//! found without reading it. What differs is what can be reclaimed later: pi can rewrite a task
//! sidecar, and `main.jsonl` is never compacted (`spec.md:4447-4448`), so a task updated tens of
//! thousands of times costs this backend open time that pi could reclaim. That is the stated envelope
//! `spec.md:4393-4395` asks for rather than an emergent difference, and it is the cost ADR-0030 §9
//! already accepts for every other table record.
//!
//! # Serde
//!
//! Every type here derives both halves, which ADR-0030 §10's serde table permits for a persisted
//! record *"under the condition: no `#[serde(borrow)]` and no `Cow<'_, _>` anywhere in them"* — honoured
//! literally. None of these types carries an invariant a constructor establishes: the invariants of this
//! format (markers strictly increase, a confirmed offset lies inside its sidecar, only confirmed
//! records are applied) are facts about a *sequence* of lines, which no line's own decode could check,
//! and they live in [`crate::recover`]. The invariant-bearing values *inside* the lines — [`Seq`],
//! [`Id`](cyrup_pico_store::Id), [`DefVersion`], `Lifetime`, `Kind` — each keep their own hand-written
//! `Deserialize` from S1, so a damaged line is rejected by the field that knows what it means.
//!
//! An **unknown** line kind is a decode error and therefore corruption, deliberately: `cyrup-session`'s
//! reader skips what it does not understand because it has no cross-record invariants to break, and
//! this format has nothing but. A line this build cannot read is a line whose effect on the committed
//! state is unknown, which is exactly `spec.md:4433`'s *"missing required confirmed data is corruption
//! and opening fails"*.

use std::path::{Path, PathBuf};

use cyrup_pico_doc::{DefVersion, StoredContent};
use cyrup_pico_store::{
    ConversationRecord, DocumentId, DocumentRecord, EntryRecord, RawId, Seq, SubmissionRecord,
    TaskRecord,
};
use serde::{Deserialize, Serialize};

/// `main.jsonl`'s name.
pub(crate) const MAIN_FILE: &str = "main.jsonl";

/// One line of `main.jsonl`.
///
/// Externally keyed by `t`, with the payload inline, so a line is one flat object a human can read with
/// `jq` — which ADR-0030 §9 lists as a reason to keep a file backend at all: *"inspectable with `jq`,
/// which matters for support"*.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub(crate) enum MainLine {
    /// A conversation record, written once (`spec.md:4274`).
    Conversation {
        /// The commit that wrote it.
        seq: Seq,
        /// The record.
        r: ConversationRecord,
    },
    /// An entry record, written once (`spec.md:69`).
    Entry {
        /// The commit that wrote it.
        seq: Seq,
        /// The record.
        r: EntryRecord,
    },
    /// A task record. Replaced as the task advances, newest line winning (`spec.md:4383`).
    Task {
        /// The commit that wrote it.
        seq: Seq,
        /// The record.
        r: TaskRecord,
    },
    /// A submission record. Replaced as the submission advances.
    Submission {
        /// The commit that wrote it.
        seq: Seq,
        /// The record.
        r: SubmissionRecord,
    },
    /// A document incarnation's record, already stamped with its lifetime.
    Document {
        /// The commit that wrote it.
        seq: Seq,
        /// The record.
        r: DocumentRecord,
    },
    /// A retirement of an existing incarnation.
    ///
    /// A line of its own rather than a rewritten [`MainLine::Document`], because the log is append-only
    /// and the retirement's sequence *is* the upper lifetime bound: replaying `created_at` from the
    /// creation line and `retired_at` from this one reconstructs the half-open interval with no record
    /// ever being edited.
    Retire {
        /// The commit that retired it, which is the lifetime's upper bound.
        seq: Seq,
        /// The incarnation.
        id: DocumentId,
    },
    /// The one marker that publishes a commit (`spec.md:4415-4419`).
    ///
    /// Written **last**, after every record of the commit it confirms. Nothing in this file is applied
    /// at open until its marker is read, which is what makes a crash mid-commit leave no trace other
    /// than bytes recovery removes.
    Marker {
        /// This commit's sequence. Strictly greater than every earlier marker's.
        seq: Seq,
        /// How many `main.jsonl` record lines this marker confirms.
        ///
        /// `spec.md:4416` asks the marker to *list* the records it publishes. For the main log a count
        /// is the whole of that list, because the lines are the ones immediately preceding this marker
        /// — and it is not ceremony: it is the only thing that detects a main log whose records
        /// vanished under a surviving marker, which is the residue of `spec.md:4425`'s
        /// *"ordinary publication does not explicitly flush `main.jsonl`"*. Short, the open fails with
        /// [`Corruption::MissingConfirmedData`](cyrup_pico_store::Corruption::MissingConfirmedData)
        /// rather than publishing less than was committed.
        main: u32,
        /// One entry per document sidecar this commit appended to.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        docs: Vec<MarkerDoc>,
    },
    /// A durable id high-water mark (`spec.md:4282`).
    ///
    /// Outside every commit, and confirmed by itself: an allocation is durable the moment this line is
    /// written, which is what makes *"a crash between a mint and the commit that uses the minted id"*
    /// unable to reissue it. `high` is a **reservation ceiling**, not the last id handed out — ids are
    /// reserved in blocks ([`crate::store`]), so a crash wastes at most a block and gaps in the id
    /// space are permitted while reuse is not.
    Mint {
        /// Every id up to and including this number is spent.
        high: u64,
    },
    /// A document sidecar's new confirmed layout after reclamation (`spec.md:4443-4446`).
    ///
    /// Written **after** the replacement generation is durable, and confirmed by itself. It supersedes
    /// every earlier offset for that incarnation, which is what keeps the marker-carried offsets of
    /// ADR-0030 F6 §D true across a reclamation: the offsets in older markers describe an older
    /// generation, and this line names the generation they stop applying to.
    Reclaim {
        /// The commit whose base or retirement authorised it (`spec.md:4435-4436`).
        seq: Seq,
        /// The incarnation.
        id: DocumentId,
        /// The generation now authoritative.
        generation: u32,
        /// Its complete confirmed layout, newest last.
        keep: Vec<Slice>,
    },
}

/// One document sidecar's confirmed end, as one commit left it.
///
/// ADR-0030 F6 §D's mechanism: *"because this process is the only appender, it knows each sidecar's byte
/// offset before writing the marker, so the marker can carry those offsets and the open pass never reads
/// sidecar payloads at all."* The `base` and `version` flags are what complete it — with them, open
/// knows where the newest base is and what version the incarnation is stored at without reading a single
/// payload byte, so `document(id, at)` is *"one seek plus the tail"* and the delta-witness check at
/// commit time needs no replay.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub(crate) struct MarkerDoc {
    /// The incarnation whose sidecar was appended to.
    pub id: DocumentId,
    /// The sidecar's byte length after this commit's appends.
    pub end: u64,
    /// Whether the record this commit appended is a complete base.
    pub base: bool,
    /// The definition version it was written at.
    pub version: DefVersion,
}

/// One content record's place in its sidecar.
///
/// The in-memory form is the same shape and is built by [`crate::recover`] from the markers; this is
/// the persisted form, which only a [`MainLine::Reclaim`] writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub(crate) struct Slice {
    /// The commit that wrote the record.
    pub seq: Seq,
    /// Its end offset in the sidecar.
    pub end: u64,
    /// Whether it is a complete base.
    pub base: bool,
    /// The definition version it was written at.
    pub version: DefVersion,
}

/// One line of a document sidecar.
///
/// The sequence rides with the content because a historical read selects records *by sequence*
/// (`spec.md:4345-4350`) and because it is the cross-check against the marker-carried offsets: a line
/// whose sequence disagrees with the slice it was read from is corruption rather than a different
/// answer.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct ContentLine {
    /// The commit that wrote it.
    pub seq: Seq,
    /// The record.
    pub content: StoredContent,
}

/// `main.jsonl`'s path inside a store directory.
pub(crate) fn main_path(dir: &Path) -> PathBuf {
    dir.join(MAIN_FILE)
}

/// One incarnation's sidecar path at a generation.
///
/// The generation is in the **name** rather than in the file, so a replacement never overwrites the
/// generation a surviving marker still describes. See [`crate::store`]'s reclamation for why that is
/// load-bearing once offsets are carried.
pub(crate) fn sidecar_path(dir: &Path, id: DocumentId, generation: u32) -> PathBuf {
    dir.join(format!("doc-{}-g{generation}.jsonl", RawId::from(id).get()))
}

/// The prefix every document sidecar's name starts with, for the orphan sweep at open.
pub(crate) const SIDECAR_PREFIX: &str = "doc-";
/// The suffix every document sidecar's name ends with.
pub(crate) const SIDECAR_SUFFIX: &str = ".jsonl";
