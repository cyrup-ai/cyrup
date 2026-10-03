//! Opaque per-scan cursors (ADR-0030 F6 §B; `spec.md:4196, 4320-4321`).
//!
//! # The failure these replace
//!
//! Upstream a cursor is `type Cursor = Readonly<Record<string, JsonValue>>` — *"one structural bag
//! for every scan and every backend"* (ADR-0030 §2.2). `spec.md:4320-4321` then states the contract
//! in prose: *"Callers only round-trip them to the same scan on the same storage; cross-storage or
//! cross-query use is unsupported."* Nothing enforces it, and the consequence ADR-0030 names is the
//! quiet one: *"the scan resumes from a position meaning something else; a history page has a hole,
//! with no error and no log line."*
//!
//! So there is one cursor **type** per scan. Handing an [`EntryCursor`] to a task scan is a type
//! error, not a hole in a transcript —
//! `tests/compile-fail/cursors_do_not_cross_scans.rs` is the proof.
//!
//! # The store stamp
//!
//! Cross-*store* reuse cannot be a type error, because two stores have the same types. Each cursor
//! therefore carries the [`StoreId`] of the store that produced it, and the only accessor a backend
//! has for the payload — [`CursorBytes::payload_for`] — takes the reading store's id and compares.
//! ADR-0030 F6 §B concedes the obvious objection and answers it: *"That compare is easy to dismiss
//! as ceremony — it is one `u128` comparison, and the failure it prevents is a silent hole in a
//! user's transcript."*
//!
//! # No `Deserialize`, and no `Serialize`
//!
//! Withheld, per ADR-0030 §10's serde table. *"Withholding `Deserialize` means a host cannot persist
//! a cursor across restarts, which is deliberate — §10 does not require it, and adding it would need
//! a validating parse plus pi's shape check (`memory.ts:123`)."* A derived impl would let any host
//! bytes become a cursor for any scan, undoing both the per-scan typing and the store stamp in one
//! step.
//!
//! # What a backend may put in the payload
//!
//! Anything, including nothing. The payload is **backend-private** (`spec.md:4320`: *"Cursors are
//! backend-owned"*). A backend that pages by id writes the id; one that pages by offset writes the
//! offset. Nothing here interprets the bytes.

use core::fmt;

use crate::StoreId;

/// A backend-private scan position, stamped with the store that produced it.
///
/// Not `Serialize`, not `Deserialize`, and the payload is unreadable without naming the store (see
/// this module's documentation).
#[derive(Clone, PartialEq, Eq)]
pub struct CursorBytes {
    store: StoreId,
    payload: Box<[u8]>,
}

/// A cursor presented to the wrong store.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
#[error("this cursor was issued by store {issued_by} and cannot be used with store {presented_to}")]
pub struct WrongStore {
    /// The store that issued the cursor.
    pub issued_by: StoreId,
    /// The store it was presented to.
    pub presented_to: StoreId,
}

impl CursorBytes {
    /// Stamp a scan position for `store`.
    ///
    /// Public because backends live in other crates — `cyrup-pico-store-jsonl` must be able to issue
    /// a cursor. That is not a bypass of anything: the guarantee here is that a cursor cannot be
    /// *decoded* by the wrong store or handed to the wrong scan, not that only this crate can make
    /// one.
    #[must_use]
    pub fn new(store: StoreId, payload: impl Into<Box<[u8]>>) -> Self {
        Self {
            store,
            payload: payload.into(),
        }
    }

    /// The payload, if this cursor belongs to `store`.
    ///
    /// The **only** way to read it, which is what makes the store check unskippable rather than
    /// customary.
    ///
    /// # Errors
    ///
    /// [`WrongStore`] when the cursor was issued by a different store.
    pub fn payload_for(&self, store: StoreId) -> Result<&[u8], WrongStore> {
        if self.store == store {
            Ok(&self.payload)
        } else {
            Err(WrongStore {
                issued_by: self.store,
                presented_to: store,
            })
        }
    }

    /// Which store issued this cursor. For a diagnostic; it does not give access to the payload.
    #[must_use]
    pub const fn store(&self) -> StoreId {
        self.store
    }
}

impl fmt::Debug for CursorBytes {
    /// Renders the store and the payload's length, never the payload: the bytes are backend-private
    /// and a `Debug` that printed them would be the serialisation the serde table withholds.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CursorBytes({}, {} bytes)",
            self.store,
            self.payload.len()
        )
    }
}

/// Declare one opaque cursor type per scan.
///
/// A macro because the five are identical in everything but name, and the names are the guarantee:
/// five newtypes over one representation is precisely ADR-0030 F6 §B.
macro_rules! scan_cursor {
    ($(#[$meta:meta])* $name:ident, $scan:literal) => {
        $(#[$meta])*
        ///
        /// Opaque, with no `Serialize` and no `Deserialize`. It round-trips into
        #[doc = concat!("`", $scan, "`")]
        /// on the store that issued it, and nowhere else.
        #[derive(Clone, PartialEq, Eq, Debug)]
        pub struct $name(CursorBytes);

        impl $name {
            /// Issue a cursor for this scan.
            #[must_use]
            pub fn new(bytes: CursorBytes) -> Self {
                Self(bytes)
            }

            /// The scan position, if this cursor belongs to `store`.
            ///
            /// # Errors
            ///
            /// [`WrongStore`] when the cursor was issued by a different store.
            pub fn payload_for(&self, store: StoreId) -> Result<&[u8], WrongStore> {
                self.0.payload_for(store)
            }

            /// Which store issued it.
            #[must_use]
            pub const fn store(&self) -> StoreId {
                self.0.store()
            }
        }
    };
}

scan_cursor!(
    /// A position in a conversation scan.
    ConversationCursor,
    "Storage::scan_conversations"
);
scan_cursor!(
    /// A position in an entry scan.
    EntryCursor,
    "Storage::scan_entries"
);
scan_cursor!(
    /// A position in a task scan.
    TaskCursor,
    "Storage::scan_tasks"
);
scan_cursor!(
    /// A position in a submission scan.
    SubmissionCursor,
    "Storage::scan_submissions"
);
scan_cursor!(
    /// A position in a document scan.
    DocumentCursor,
    "Storage::scan_documents"
);
