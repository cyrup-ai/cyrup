//! Open time and resident size, measured (PICO5-PLAN S10; ADR-0030 §14 open question 2).
//!
//! # What this exists to settle
//!
//! ADR-0030 §9 chooses the file backend over an embedded engine and writes the condition for revisiting
//! that choice as a number:
//!
//! > Write `cyrup-pico-store-sqlite` when a p95 real cyrup session's store exceeds **either** 200 ms to
//! > open **or** 64 MB resident index.
//!
//! And then says what the number is worth: *"That threshold is a **placeholder with no measurement
//! behind it** (§14, open question 2). It is stated as a number anyway, because a trigger without one is
//! not a trigger."* Open question 2 asks for the measurement:
//!
//! > Nobody has measured real `~/.cyrup` session sizes — total commit count and table-record count are
//! > the two numbers that matter, because open is O(commits + table records). Measure before the
//! > threshold is treated as decided.
//!
//! This module is the measuring instrument. It does not decide anything: whether the trigger fires is
//! read off [`Measurement::fires_the_sqlite_trigger`], and PICO5-PLAN makes acting on it S12's business
//! and conditional on that reading.
//!
//! # Why the open is timed through `open_read_only`
//!
//! It performs the same [`recover::open`](crate::recover) pass as the writable open — the same log
//! replay, the same index build, the same `stat` per sidecar — and takes no lock, so repeated samples do
//! not serialise against each other or against anything else on the machine. The one thing it does not
//! do is *repair*, and repair only ever removes bytes no marker confirmed, which a store this was
//! measured on does not have.
//!
//! # Why the resident size is modelled rather than read from the allocator
//!
//! Rust exposes no per-structure heap accounting, and process RSS measures the process, not the store:
//! it includes the binary, the runtime, the page cache mappings and whatever the measuring harness
//! itself allocated, and it does not shrink when a store is dropped. [`Footprint`] is therefore an
//! explicit model whose terms are stated where they are charged ([`crate::index`]), deliberately
//! pessimistic about the index half, and split so the half ADR-0030 §9's own model predicts can be
//! compared against the half it omits. A caller that wants the process figure as a sanity check can read
//! [`Measurement::resident_process_bytes`], which is `/proc/self/statm` where that exists and `None`
//! elsewhere — reported beside the model, never in place of it.

use std::path::Path;
use std::time::{Duration, Instant};

use cyrup_pico_store::StorageFailure;

use crate::index::Footprint;
use crate::store::JsonlStore;

/// ADR-0030 §9's open-time half of the SQLite trigger.
pub const TRIGGER_OPEN: Duration = Duration::from_millis(200);

/// ADR-0030 §9's resident-size half of the SQLite trigger.
pub const TRIGGER_RESIDENT_BYTES: u64 = 64 * 1024 * 1024;

/// What one store costs to open and to hold.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Measurement {
    /// How many opens were timed.
    pub samples: usize,
    /// The median open.
    pub p50: Duration,
    /// The 95th-percentile open, which is the quantity ADR-0030 §9's trigger names.
    pub p95: Duration,
    /// The slowest open seen.
    pub max: Duration,
    /// What the opened store holds in memory.
    pub footprint: Footprint,
    /// The whole process's resident size, where the platform reports one.
    ///
    /// A sanity check on the model and nothing more: it is the process, not the store. See this module's
    /// documentation.
    pub resident_process_bytes: Option<u64>,
}

impl Measurement {
    /// Which half of ADR-0030 §9's trigger this store trips, if either.
    ///
    /// The index half is compared against [`Footprint::index_bytes`], because that is the quantity §9's
    /// own model is about. [`Footprint::record_bytes`] is reported beside it rather than folded in, since
    /// folding it in would silently restate the threshold.
    #[must_use]
    pub const fn fires_the_sqlite_trigger(&self) -> bool {
        let slow = self.p95.as_nanos() > TRIGGER_OPEN.as_nanos();
        let large = self.footprint.index_bytes > TRIGGER_RESIDENT_BYTES;
        slow || large
    }
}

/// Open `dir` `samples` times, timing each, and report what the last open held.
///
/// # Panics
///
/// Never: `samples` is clamped to at least one, and the percentile indices are taken with checked
/// accessors.
///
/// # Errors
///
/// [`StorageFailure`] from the open itself. A store that cannot be opened has no open time.
pub fn measure(dir: &Path, samples: usize) -> Result<Measurement, StorageFailure> {
    let samples = samples.max(1);
    let mut timings = Vec::with_capacity(samples);
    let mut footprint = None;
    for _ in 0..samples {
        let started = Instant::now();
        let store = JsonlStore::open_read_only(dir)?;
        timings.push(started.elapsed());
        footprint = Some(store.plans().footprint());
    }
    timings.sort_unstable();
    let last = timings.len().saturating_sub(1);
    Ok(Measurement {
        samples,
        p50: at(&timings, percentile_index(timings.len(), 50)),
        p95: at(&timings, percentile_index(timings.len(), 95)),
        max: at(&timings, last),
        // `measure` always runs at least one sample, so this is always `Some`; the default is here
        // because a measurement must not be the thing that panics.
        footprint: footprint.unwrap_or(Footprint {
            index_bytes: 0,
            record_bytes: 0,
            commits: 0,
            records: 0,
        }),
        resident_process_bytes: resident_process_bytes(),
    })
}

/// The nearest-rank index of a percentile over `n` sorted samples.
///
/// Nearest-rank rather than interpolated, because an interpolated p95 of ten samples is a number no
/// sample produced, and a threshold should be compared against an observation.
fn percentile_index(n: usize, percentile: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let rank = n.saturating_mul(percentile).div_ceil(100).max(1);
    rank.saturating_sub(1).min(n.saturating_sub(1))
}

/// One sorted sample, or zero for an empty set.
fn at(sorted: &[Duration], index: usize) -> Duration {
    sorted.get(index).copied().unwrap_or_default()
}

/// The process's resident size, where the platform reports one in a file.
///
/// Linux only, and `None` everywhere else rather than a `cfg` fork with a second implementation to keep
/// correct: this figure is a sanity check beside the model, so a platform that does not offer it cheaply
/// simply does not offer it.
fn resident_process_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // `statm` counts pages; 4 KiB is the page size on every target this measurement runs on, and a wrong
    // page size would scale a sanity check rather than change a decision.
    Some(pages.saturating_mul(4096))
}
