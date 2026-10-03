//! `cyrup-pico-store-jsonl` — the Pico5 durability kernel's file backend (PICO5-PLAN S7).
//!
//! §11.3's publication protocol over the append-plus-deferred-`fdatasync` primitive `cyrup-session`
//! already owns, plus the two requirements ADR-0030 §9 adds that neither the specification nor pi
//! states: an **exclusive store lock at open**, and a **durable rename**.
//!
//! ```text
//! let lock = StoreLock::acquire(dir)?;                       // or: the store is in use, by name
//! let mut store = JsonlStore::open_for_write(
//!     dir, lock, JsonlOptions::new(Durability::PowerLoss),
//! )?;
//! let seq = store.commit(batch, &cx).await?;                 // sidecars, then one marker, then publish
//! ```
//!
//! # What this backend is for
//!
//! ADR-0030 §9 chooses it over an embedded engine for now, and is specific about why it is not a
//! placeholder: *"files are what cyrup already does, what keeps a session inspectable, and what pi ships
//! as a peer."* The cost is stated rather than hidden — open is **O(commits + table records)** and
//! `main.jsonl` is never compacted (`spec.md:4447-4448`) — and §9 writes the trigger for replacing it
//! with SQLite as a number: *"when a p95 real cyrup session's store exceeds either 200 ms to open or
//! 64 MB resident index."*
//!
//! # The four guarantees this crate carries
//!
//! * **`G-INV-1` in the medium.** One commit is atomic across every record table and every document
//!   command. The mechanism is that nothing is applied until its marker is appended, so a crash
//!   mid-commit leaves bytes and no state ([`crate::recover`]).
//! * **The marker protocol** (`spec.md:4415-4418`), with no standalone-sidecar fast path, and with the
//!   marker carrying sidecar **byte offsets** — ADR-0030 F6 §D's mechanism, sound only because the lock
//!   makes this process the only appender ([`crate::wire`]).
//! * **The two durability tiers**, named ([`Durability`]), with the asymmetry §11.3 turns on: a lost
//!   tail commit is recoverable, a marker without its data is not.
//! * **Recovery and poison** (`spec.md:4428-4434`): torn final lines removed, unconfirmed sidecar tails
//!   ignored and removed, confirmed records only, missing required confirmed data **fails the open**,
//!   and any uncertain append poisons the backend.
//! * **One writer per store** ([`StoreLock`]): `unrepresentable` in-process through
//!   `commit(&mut self, ..)`, **guarded** across processes here, and `checked` on a network filesystem
//!   — where `flock` is unreliable and the identity record's holder pid is what remains.
//!
//! # ADR-0030 §14 open question 3, answered
//!
//! > Does the marker-carries-sidecar-offsets design survive a second look? … Recovery must validate
//! > every offset against actual sidecar length and treat a past-EOF offset as corruption. Is that
//! > validation cheap enough to keep the open-time saving it exists to buy?
//!
//! **Yes, and by a wide margin, but it moves a cost rather than removing one.** The validation is one
//! `metadata()` per incarnation that has content and reads no payload byte ([`crate::recover`]), so it
//! is bounded by the number of documents while the saving it protects is the size of their content —
//! the gap widens as documents grow. What the design does cost is **reclamation**: offsets make
//! `spec.md:4443-4446`'s rename-over-the-live-sidecar unsound, because a crash between that rename and
//! the line authorising it would leave surviving markers describing a file that is now shorter. This
//! backend therefore renames to a new **generation** name and names the authoritative generation in the
//! log ([`JsonlStore`]'s reclamation). That is the real price of the mechanism, it is paid once in one
//! function, and it closes a crash window the in-place rename leaves open.
//!
//! # Objections to ADR-0030, implemented as written and recorded here
//!
//! * **`open_for_write(dir, lock, opts)` takes both a directory and a lock, and nothing in the
//!   signature makes them agree.** Taking the directory *from* the lock would make a mismatch
//!   unrepresentable and delete a runtime check; the signature is implemented as ADR-0030 F6 §D writes
//!   it, with the check ([`JsonlStore::open_for_write`]), because a signature in the design of record is
//!   not this slice's to change.
//! * **PICO5-PLAN S7 asks for the tiers to be *"wired to `cyrup-session`'s existing
//!   `SESSION_SYNCER`"`.*** That static is private to that crate, and ADR-0030 §8 both lists this
//!   crate's dependencies as *"one new dependency: fs4"* and rules that *"`cyrup-session` is not
//!   extended in place … what is reused is the primitive **beneath** that trait"*. The mechanism is
//!   therefore lifted ([`crate::syncer`]), with attribution, and the same holds for S11's
//!   `durable_rename` ([`crate::durable`]). A leaf crate holding both once is the right end state and is
//!   a refactor across two crate boundaries rather than part of this slice.
//! * **The strong tier does not cost ~1.5 µs on a commit that writes document content.** PICO5-PLAN S7
//!   quotes that figure from ADR-0030 §9, where it is the cost of a deferred flush — but
//!   `spec.md:4421-4424` requires the sidecar flush to have *happened* before the marker is appended, so
//!   a document-bearing commit under [`Durability::PowerLoss`] pays one coalesced flush round
//!   (~200 µs). The figure is exact for every [`Durability::ProcessCrash`] commit and for a
//!   main-only commit under either tier, which `spec.md:4426` singles out.
//!
//! # The reopen suite lives here, not in the shared conformance suite
//!
//! PICO5-PLAN S3's `cyrup_pico_store::conformance` module — behind that crate's `conformance`
//! feature, which is why it is named here rather than linked — anticipates S7 extending it
//! *"through `StoreFactory`"*. The cases this slice owes — exactly the commits whose markers survived, a
//! crash between a mint and a commit, the two tiers' asymmetry, a second process refused — are all
//! statements about a **medium**, and `MemoryStore` is §11.1's reference semantics with no medium at
//! all. Putting them in the shared table would mean either failing them for the reference backend or
//! gating them behind a capability flag that only one backend sets. They are therefore this crate's own
//! tests, and the shared suite is run here unchanged (`tests/conformance.rs`) so parity is still judged
//! by the suite that is the contract (`spec.md:4369`).

#![forbid(unsafe_code)]

mod appender;
pub mod benchmark;
mod copy;
mod durable;
mod identity;
mod index;
mod lock;
mod plan;
mod query;
mod read;
mod recover;
mod state;
mod store;
mod syncer;
mod wire;

pub use benchmark::Measurement;
pub use identity::{FORMAT_VERSION, StoreIdentity};
pub use index::Footprint;
pub use lock::{StoreBusy, StoreLock};
pub use plan::{Index, Plan};
pub use query::Planned;
pub use read::Reader;
pub use store::{Durability, JsonlOptions, JsonlStore, ReadOnlyStore};

#[cfg(test)]
mod tests;
