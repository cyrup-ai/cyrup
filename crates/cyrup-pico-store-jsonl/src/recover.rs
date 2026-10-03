//! Recovery: `spec.md:4428-4434`'s five rules, and the offset validation ADR-0030 F6 §D's mechanism
//! owes the store.
//!
//! > - Remove torn final lines.
//! > - Ignore and remove unconfirmed sidecar tails.
//! > - Apply confirmed records only.
//! > - Missing required confirmed data is corruption and opening fails.
//! > - A later committed latest base or retirement may prove an earlier physical record unnecessary.
//!
//! # How "confirmed" is decided, in one sentence
//!
//! A commit's record lines precede its marker in `main.jsonl`, so a line is confirmed exactly when a
//! later marker for its sequence was read. Nothing is applied before its marker, which makes a crash
//! mid-commit leave bytes and no state — and makes *"exactly the set of commits whose markers
//! survived"* a property of this loop rather than of a cleanup pass.
//!
//! # The offset validation, which is ADR-0030 §14 open question 3
//!
//! The question asks whether validating every marker-carried sidecar offset is *"cheap enough to keep
//! the open-time saving it exists to buy"*. The answer is in [`check_sidecars`] and it is **one
//! `metadata()` per incarnation that has content**, with no payload byte read: a confirmed end past the
//! file's actual length is corruption (`spec.md:4433`), a file longer than its confirmed end has an
//! unconfirmed tail to remove, and equal is the ordinary case. So the validation is `stat`-bounded while
//! the saving it protects is the whole of the sidecar payload — and the saving grows with document size
//! while the cost does not.
//!
//! # Repair, and why a read-only open does not
//!
//! `repair` is `true` only under the store lock. A read-only open *ignores* an unconfirmed tail — it
//! must, to answer `spec.md:4429` — but removes nothing, because a second process truncating a file the
//! writer is appending to is precisely the corruption the lock exists to prevent
//! (ADR-0030 F6 §D). This is also why a read-only open of a store with a torn tail succeeds and does
//! not see it: *"ignore"* and *"remove"* are two rules, and only one of them needs the lock.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;

use cyrup_pico_store::{Corruption, RawId, StorageFailure};

use crate::identity::StoreIdentity;
use crate::read::Reader;
use crate::state::Committed;
use crate::wire::{MainLine, SIDECAR_PREFIX, SIDECAR_SUFFIX, main_path, sidecar_path};

/// A recovered store: its state, and where its log ends.
pub(crate) struct Recovered {
    /// Every read path over the recovered state.
    pub reader: Reader,
    /// `main.jsonl`'s length after recovery, which is the next append's offset.
    pub main_len: u64,
}

/// Replay `dir`'s log, validate its sidecars, and (under the lock) remove what no marker confirms.
///
/// # Errors
///
/// [`StorageFailure::Corrupt`] for every damage shape `spec.md:4428-4434` calls corruption: an
/// undecodable line that is not the torn final one, a marker that does not increase, a marker whose
/// records are not all there, a confirmed sidecar offset past end of file, an id filed under two kinds.
/// [`StorageFailure::Io`] from the medium.
pub(crate) fn open(
    dir: &Path,
    identity: StoreIdentity,
    repair: bool,
) -> Result<Recovered, StorageFailure> {
    let mut state = Committed::new();
    let path = main_path(dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(StorageFailure::Io(e)),
    };

    let mut offset: u64 = 0;
    // The end of the last line whose effect is confirmed: a marker, a mint, or a reclamation. Record
    // lines after it belong to a commit whose marker never arrived.
    let mut confirmed_end: u64 = 0;
    let mut pending: Vec<MainLine> = Vec::new();

    for chunk in bytes.split_inclusive(|b| *b == b'\n') {
        let start = offset;
        offset = offset.saturating_add(chunk.len() as u64);
        if !chunk.ends_with(b"\n") {
            // `spec.md:4429`: remove torn final lines. A chunk without its newline can only be the
            // last one, and can only be a write this process did not finish.
            break;
        }
        let line = decode(chunk, start, &path)?;
        match line {
            MainLine::Marker { seq, main, .. } => {
                if pending.len() != main as usize {
                    return Err(StorageFailure::Corrupt(Corruption::MissingConfirmedData {
                        what: format!(
                            "commit {seq} confirms {main} main record(s) in {}, {} present",
                            path.display(),
                            pending.len()
                        ),
                    }));
                }
                if let Some(wrong) = pending.iter().find(|l| line_seq(l) != Some(seq)) {
                    return Err(StorageFailure::Corrupt(Corruption::RecordMalformed {
                        what: "main log",
                        detail: format!(
                            "a record from commit {} precedes marker {seq}",
                            line_seq(wrong).map_or_else(|| "?".to_owned(), |s| s.to_string())
                        ),
                    }));
                }
                for record in pending.drain(..) {
                    state.apply_line(&record).map_err(StorageFailure::Corrupt)?;
                }
                state.apply_line(&line).map_err(StorageFailure::Corrupt)?;
                confirmed_end = offset;
            }
            MainLine::Mint { .. } | MainLine::Reclaim { .. } => {
                // Both are written outside a commit and are confirmed by themselves, so neither can
                // appear between a commit's records and its marker.
                if !pending.is_empty() {
                    return Err(StorageFailure::Corrupt(Corruption::RecordMalformed {
                        what: "main log",
                        detail: format!(
                            "{} holds a standalone line inside an unconfirmed commit at byte {start}",
                            path.display()
                        ),
                    }));
                }
                state.apply_line(&line).map_err(StorageFailure::Corrupt)?;
                confirmed_end = offset;
            }
            record => pending.push(record),
        }
    }

    // `spec.md:4430`: apply confirmed records only. Whatever is still pending belongs to a commit whose
    // marker never reached the log, so it is dropped — and, under the lock, removed, so the next append
    // starts from a byte the next marker can describe.
    // A read-only open leaves the file alone and simply *ignores* the tail; only a holder of the lock
    // removes it.
    if repair && (!pending.is_empty() || offset > confirmed_end) {
        truncate(&path, confirmed_end)?;
    }

    check_sidecars(dir, &state, repair)?;
    if repair {
        sweep_orphans(dir, &state);
    }

    Ok(Recovered {
        reader: Reader {
            dir: dir.to_path_buf(),
            identity,
            state,
        },
        main_len: confirmed_end,
    })
}

/// Validate every marker-carried offset against its sidecar's actual length, and remove unconfirmed
/// tails.
///
/// This is ADR-0030 §14 open question 3's cost, in full: one `metadata()` per incarnation with content,
/// and one `set_len` per incarnation that has an unconfirmed tail.
fn check_sidecars(dir: &Path, state: &Committed, repair: bool) -> Result<(), StorageFailure> {
    for (id, doc) in &state.documents {
        let confirmed = doc.end();
        let path = sidecar_path(dir, *id, doc.generation);
        if confirmed == 0 {
            // A record with no content record at all. `spec.md:4433`: missing required confirmed data
            // is corruption and opening fails — this is not an absence, because a creation always
            // writes a complete base (`spec.md:1374`).
            return Err(StorageFailure::Corrupt(Corruption::MissingConfirmedData {
                what: format!("document {id}'s content: no confirmed record"),
            }));
        }
        let actual = match std::fs::metadata(&path) {
            Ok(meta) => meta.len(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(StorageFailure::Corrupt(Corruption::MissingConfirmedData {
                    what: format!("document {id}'s sidecar {}", path.display()),
                }));
            }
            Err(e) => return Err(StorageFailure::Io(e)),
        };
        if actual < confirmed {
            // The failure mode the offsets design has to answer for: an offset past end of file. It
            // means the sidecar lost bytes a surviving marker promised, which is corruption and not a
            // shorter answer.
            return Err(StorageFailure::Corrupt(Corruption::MissingConfirmedData {
                what: format!(
                    "document {id}'s sidecar {} is {actual} bytes, and marker offsets confirm {confirmed}",
                    path.display()
                ),
            }));
        }
        if actual > confirmed && repair {
            // `spec.md:4429`: ignore **and remove** unconfirmed sidecar tails.
            truncate(&path, confirmed)?;
        }
    }
    Ok(())
}

/// Remove sidecar generations no document's authoritative layout names, and leftover temp files.
///
/// Both are the trace of a crash during reclamation: a replacement generation whose authorising
/// [`MainLine::Reclaim`] never reached the log, or a temp file whose rename never happened. Neither is
/// read by anything — the generation in the *name* is what makes that true — so the sweep is tidiness,
/// not correctness, and every failure in it is ignored for exactly that reason.
fn sweep_orphans(dir: &Path, state: &Committed) {
    let live: BTreeSet<(u64, u32)> = state
        .documents
        .iter()
        .map(|(id, doc)| (RawId::from(*id).get().get(), doc.generation))
        .collect();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.ends_with(".tmp") {
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        if let Some(parsed) = parse_sidecar_name(name)
            && !live.contains(&parsed)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// `doc-<id>-g<gen>.jsonl` -> `(id, gen)`.
fn parse_sidecar_name(name: &str) -> Option<(u64, u32)> {
    let middle = name
        .strip_prefix(SIDECAR_PREFIX)?
        .strip_suffix(SIDECAR_SUFFIX)?;
    let (id, generation) = middle.rsplit_once("-g")?;
    Some((id.parse().ok()?, generation.parse().ok()?))
}

/// Decode one complete line, reporting an undecodable one as corruption.
///
/// An unknown line kind lands here too, deliberately: see [`crate::wire`] for why this format does not
/// skip what it cannot read.
fn decode(chunk: &[u8], at: u64, path: &Path) -> Result<MainLine, StorageFailure> {
    let text = std::str::from_utf8(chunk).map_err(|e| malformed(path, at, &e))?;
    serde_json::from_str(text.trim_end_matches('\n')).map_err(|e| malformed(path, at, &e))
}

fn malformed(path: &Path, at: u64, e: &dyn core::fmt::Display) -> StorageFailure {
    StorageFailure::Corrupt(Corruption::RecordMalformed {
        what: "main log",
        detail: format!("{} at byte {at}: {e}", path.display()),
    })
}

/// Cut a file back to `len` and make the cut stick.
///
/// The `sync_data` is cheap here because this runs at most once per file per open, and it matters: a
/// truncation that stays in the page cache would be redone on the next open, and a crash between the
/// truncation and the first append would leave bytes a marker already disclaimed.
fn truncate(path: &Path, len: u64) -> Result<(), StorageFailure> {
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(StorageFailure::Io)?;
    f.set_len(len).map_err(StorageFailure::Io)?;
    f.sync_data().map_err(StorageFailure::Io)?;
    Ok(())
}

/// The sequence a record line belongs to, or `None` for the lines that belong to no commit.
fn line_seq(line: &MainLine) -> Option<cyrup_pico_store::Seq> {
    match line {
        MainLine::Conversation { seq, .. }
        | MainLine::Entry { seq, .. }
        | MainLine::Task { seq, .. }
        | MainLine::Submission { seq, .. }
        | MainLine::Document { seq, .. }
        | MainLine::Retire { seq, .. }
        | MainLine::Marker { seq, .. }
        | MainLine::Reclaim { seq, .. } => Some(*seq),
        MainLine::Mint { .. } => None,
    }
}
