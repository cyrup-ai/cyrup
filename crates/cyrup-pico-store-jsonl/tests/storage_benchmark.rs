//! The storage benchmark: the two numbers ADR-0030 §14 open question 2 asks for (PICO5-PLAN S10).
//!
//! > Nobody has measured real `~/.cyrup` session sizes — total commit count and table-record count are
//! > the two numbers that matter, because open is O(commits + table records). Measure before the
//! > threshold is treated as decided.
//!
//! # What *"a realistic commit count"* is, measured rather than guessed
//!
//! The corpus is 399 real agent-session transcripts over this repository — the only real sessions this
//! machine has, because `~/.cyrup` does not exist on it; see this slice's report for that and for why the
//! shape transfers. Their record counts are:
//!
//! | quantile | records per session | bytes per session |
//! |---|---|---|
//! | p50 | 116 | 0.46 MB |
//! | p90 | 339 | 1.13 MB |
//! | **p95** | **535** | **1.48 MB** |
//! | p99 | 1 292 | 2.59 MB |
//! | max | 5 276 | 11.9 MB |
//!
//! with a mean record size of **3 385 bytes**. So [`DEFAULT_COMMITS`] is 600 — just past the measured p95
//! — and [`RECORD_BYTES`] is that mean. A larger run is one environment variable away
//! (`CYRUP_PICO_BENCH_COMMITS`), and the figures for the sizes that matter are in the slice's report.
//!
//! # Why this is a test and not a `criterion` bench
//!
//! Two reasons, both about rot. A `#[bench]` target is not run by `cargo nextest run`, so a benchmark
//! that stopped compiling — or stopped measuring the thing it claims to — would go unnoticed until
//! somebody asked for the number; this runs on every green tree. And the assertion at the end **is** the
//! trigger evaluation: the day a p95 open at a realistic commit count crosses ADR-0030 §9's 200 ms, this
//! test says so, which is exactly the signal S12 is conditional on.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use cyrup_pico_doc::{DefVersion, DocMap, DocRoot, DocValue, Op, OpBatch, Path as DocPath};
use cyrup_pico_store::{
    BatchBuilder, ConversationRecord, ConversationSemantics, Cx, Document, DocumentBase,
    DocumentContent, DocumentCreate, DocumentId, DocumentPoint, DocumentScope, Entry, EntryId,
    EntryRecord, Kind, LatestFork, ROOT_CONVERSATION_ID, Retire, Storage, StorageExt,
};
use cyrup_pico_store_jsonl::benchmark::{self, TRIGGER_OPEN, TRIGGER_RESIDENT_BYTES};
use cyrup_pico_store_jsonl::{Durability, JsonlOptions, JsonlStore, StoreLock};

/// Just past the measured p95 real session (535 records).
const DEFAULT_COMMITS: usize = 600;

/// The measured mean record size across the corpus.
const RECORD_BYTES: usize = 3_385;

/// How many opens to time. Twenty gives a p95 that is an observation rather than an interpolation.
const DEFAULT_SAMPLES: usize = 20;

/// One document per this many commits, so the sidecar layout and the base-plus-tail read are exercised
/// at a realistic ratio rather than not at all.
const COMMITS_PER_DOCUMENT: usize = 50;

#[tokio::test]
async fn the_open_time_and_resident_size_at_a_realistic_commit_count() {
    let commits = env("CYRUP_PICO_BENCH_COMMITS", DEFAULT_COMMITS);
    let samples = env("CYRUP_PICO_BENCH_SAMPLES", DEFAULT_SAMPLES);
    let dir = tempfile::tempdir().expect("a temp directory");

    build(dir.path(), commits).await;
    let measured = benchmark::measure(dir.path(), samples).expect("the measurement");

    let bytes_on_disk = disk_bytes(dir.path());
    println!("\n  ---- cyrup-pico-store-jsonl: open time and resident size ----");
    println!("  commits                {}", measured.footprint.commits);
    println!("  table records          {}", measured.footprint.records);
    println!("  bytes on disk          {:.2} MB", mb(bytes_on_disk));
    println!("  open p50               {:?}", measured.p50);
    println!(
        "  open p95               {:?}   (trigger: {TRIGGER_OPEN:?})",
        measured.p95
    );
    println!("  open max               {:?}", measured.max);
    println!(
        "  resident index         {:.3} MB   (trigger: {:.0} MB)",
        mb(measured.footprint.index_bytes),
        mb(TRIGGER_RESIDENT_BYTES)
    );
    println!(
        "  resident records       {:.3} MB   (the half ADR-0030 §9's model omits)",
        mb(measured.footprint.record_bytes)
    );
    println!(
        "  resident total         {:.3} MB",
        mb(measured.footprint.total_bytes())
    );
    if let Some(rss) = measured.resident_process_bytes {
        println!(
            "  process RSS            {:.1} MB   (the process, not the store)",
            mb(rss)
        );
    }
    println!(
        "  SQLite trigger         {}",
        if measured.fires_the_sqlite_trigger() {
            "FIRES"
        } else {
            "does not fire"
        }
    );
    println!("  ------------------------------------------------------------\n");

    // The measurement measured something.
    assert_eq!(
        measured.footprint.commits as usize,
        commits + 1,
        "one commit per entry plus the root conversation"
    );
    assert_eq!(measured.samples, samples);
    assert!(
        measured.p50 > std::time::Duration::ZERO,
        "an open takes time"
    );
    assert!(measured.p95 >= measured.p50, "percentiles are ordered");
    assert!(measured.max >= measured.p95, "and the max bounds them");
    assert!(
        measured.footprint.index_bytes > 0 && measured.footprint.record_bytes > 0,
        "both halves of the footprint are accounted"
    );

    // And this is the trigger evaluation ADR-0030 §9 asks for, as an assertion rather than a report:
    // a realistic session's p95 open is far inside the 200 ms budget, and its resident index is far
    // inside the 64 MB one. If either stops being true, S12's condition has fired.
    assert!(
        !measured.fires_the_sqlite_trigger(),
        "ADR-0030 §9's SQLite trigger fired at {commits} commits: p95 open {:?} against {TRIGGER_OPEN:?}, \
         resident index {} bytes against {TRIGGER_RESIDENT_BYTES}. That is S12's condition, and it is \
         the author's call what to do about it — but it must not pass silently.",
        measured.p95,
        measured.footprint.index_bytes
    );
}

/// Read a `usize` from the environment, or take the default.
fn env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Megabytes, for a report a person reads.
fn mb(bytes: u64) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a report of megabytes to three places; the loss begins past 2^53 bytes"
    )]
    let bytes = bytes as f64;
    bytes / (1024.0 * 1024.0)
}

/// Every byte this store occupies, main log and sidecars together.
fn disk_bytes(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Commit one entry per commit, with a payload of the measured mean size, plus one document per
/// [`COMMITS_PER_DOCUMENT`] that then takes a delta on every later commit it is part of.
async fn build(dir: &std::path::Path, commits: usize) {
    let lock = StoreLock::acquire(dir).expect("the store lock");
    let mut store = JsonlStore::open_for_write(
        dir,
        lock,
        // The weak tier: a benchmark that measured the device's flush latency would be measuring the
        // device. The open path — which is what this times — is identical under both tiers.
        JsonlOptions::new(Durability::ProcessCrash),
    )
    .expect("the store");
    let cx = Cx::detached();

    let mut builder = BatchBuilder::new();
    builder
        .conversation(ConversationRecord {
            id: ROOT_CONVERSATION_ID,
            parent: None,
            owner: None,
        })
        .expect("staging the root conversation");
    store
        .commit(builder.build().expect("a batch"), &cx)
        .await
        .expect("the root commit");

    let payload = "x".repeat(RECORD_BYTES);
    let mut documents: Vec<DocumentId> = Vec::new();
    for n in 0..commits {
        let entry: EntryId = store.mint::<Entry>(&cx).await.expect("an entry id");
        let mut builder = BatchBuilder::new();
        builder
            .entry(EntryRecord {
                id: entry,
                conversation_id: ROOT_CONVERSATION_ID,
                kind: kind("cyrup.message"),
                model: None,
                data: Some(DocValue::string(&payload)),
                head: None,
                edits: None,
                by_task_id: None,
            })
            .expect("staging an entry");

        if n % COMMITS_PER_DOCUMENT == 0 {
            let id: DocumentId = store.mint::<Document>(&cx).await.expect("a document id");
            builder
                .create_document(
                    DocumentCreate {
                        id,
                        kind: kind(&format!("cyrup.doc{n}")),
                        key: None,
                        scope: DocumentScope::Conversation {
                            conversation_id: ROOT_CONVERSATION_ID,
                            semantics: ConversationSemantics::Latest {
                                fork: LatestFork::Current,
                            },
                        },
                    },
                    DocumentBase {
                        version: DefVersion::FIRST,
                        value: value("n", n as i64),
                    },
                    Retire::Keep,
                )
                .expect("staging a creation");
            documents.push(id);
        } else if let Some(id) = documents.last().copied() {
            // A delta on the newest document, so sidecars carry tails rather than single bases.
            let version = store
                .document(id, DocumentPoint::Current, &cx)
                .await
                .expect("the read")
                .expect("the document")
                .version;
            builder
                .change_document(
                    id,
                    DocumentContent::Delta {
                        continues: version,
                        ops: OpBatch::new(vec![Op::Set {
                            path: DocPath::keys(["n"]).expect("a one-key path"),
                            value: DocValue::integer(n as i64),
                        }]),
                    },
                    Retire::Keep,
                )
                .expect("staging a delta");
        }

        store
            .commit(builder.build().expect("a batch"), &cx)
            .await
            .expect("the commit");
    }
    store.close(&cx).await.expect("the close");
}

/// Parse a kind.
fn kind(s: &str) -> Kind {
    Kind::parse(s).expect("a valid kind")
}

/// A one-key document value.
fn value(key: &str, n: i64) -> DocRoot {
    let mut map = DocMap::new();
    map.insert(Arc::from(key), DocValue::integer(n));
    DocRoot::from_map(map)
}
