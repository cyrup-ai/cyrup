//! Document definition versions and the `StoredVersion` witness (`spec.md:1119-1120, 4366-4367`).
//!
//! # Why `DefVersion` lives in this crate and not in `cyrup-pico-store`
//!
//! ADR-0030 §10 lists it under *identity* (F6 §A) with `Id<K>` and `Seq`, which reads as though it
//! belongs with them. ADR-0030 §8's dependency direction overrides that: `cyrup-pico-store`
//! depends on `cyrup-pico-doc` and never the reverse, because S3's `DocumentBase` takes [`DocRoot`]
//! and `DocumentContent::Delta` takes [`StoredVersion`]. [`StoredContent`] is in this crate and
//! carries a version in both arms, so the version must be here too or the two crates form a cycle.
//! `cyrup-pico-store` re-exports it, so `cyrup_pico_store::DefVersion` still resolves and S1's
//! tests are unchanged.
//!
//! [`DocRoot`]: crate::DocRoot
//! [`StoredContent`]: crate::StoredContent

use core::fmt;
use core::num::NonZeroU32;

use serde::{Serialize, Serializer};

/// The definition version a stored document base or delta was written by.
///
/// It belongs to each stored record rather than to the incarnation, *"because one incarnation may
/// contain records written by multiple definition versions"* (`spec.md:1119-1120`). A delta can
/// never cross a version boundary; a tail that straddles one is corruption, and
/// [`ReplayPlan::parse`] is the single place that says so.
///
/// `NonZeroU32` because version 0 names no definition: the first version a `DocDef` can declare is
/// 1, so "unversioned" and "version 0" cannot be the same bit pattern.
///
/// [`ReplayPlan::parse`]: crate::ReplayPlan::parse
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DefVersion(NonZeroU32);

impl DefVersion {
    /// The first version a definition can declare.
    pub const FIRST: Self = Self(NonZeroU32::MIN);

    /// A version from a nonzero number.
    #[must_use]
    pub const fn new(n: NonZeroU32) -> Self {
        Self(n)
    }

    /// The version number, for a diagnostic or a wire form.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl fmt::Display for DefVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for DefVersion {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(self.0.get())
    }
}

/// Proof that this version was **read from storage**, not asserted by a caller.
///
/// # What it is for
///
/// `spec.md:4366-4367`: *"Deltas cannot cross a stored version boundary; a version transition must
/// be a base."* The write side of that rule needs a delta to name the version of the records it
/// continues, and a plain [`DefVersion`] field would let the Session name any version at all —
/// including the current one, on a tail written by an older definition, which is exactly the
/// *"plausible-looking corrupt state"* ADR-0030 §2.3 names as the failure. A `StoredVersion` can
/// only be obtained from [`ReplayPlan::version`], and a `ReplayPlan` can only be obtained from
/// [`ReplayPlan::parse`] over records a backend read. So a delta cannot claim a version it did not
/// read.
///
/// # Why it has no serde in either direction
///
/// ADR-0030 §10's table: *"an in-process witness. A `Deserialize` would let a recovered record mint
/// a version claim."* `Serialize` is withheld too, because a serialized witness is one `Deserialize`
/// away from being a construction path and nothing needs it on the wire — the *number* travels as
/// the [`DefVersion`] field of [`StoredContent`], which is a different thing from the proof that it
/// was read.
///
/// The `_seal: ()` private field is what makes a struct literal outside this crate a compile error;
/// `tests/compile-fail/stored_version_cannot_be_forged.rs` is the proof.
///
/// [`ReplayPlan::parse`]: crate::ReplayPlan::parse
/// [`ReplayPlan::version`]: crate::ReplayPlan::version
/// [`StoredContent`]: crate::StoredContent
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StoredVersion {
    version: DefVersion,
    _seal: (),
}

impl StoredVersion {
    /// Mint a witness. Crate-private: [`ReplayPlan::parse`] is the only caller, and it has just
    /// read the records the version comes from.
    ///
    /// [`ReplayPlan::parse`]: crate::ReplayPlan::parse
    pub(crate) const fn mint(version: DefVersion) -> Self {
        Self { version, _seal: () }
    }

    /// The version this witness attests.
    #[must_use]
    pub const fn version(self) -> DefVersion {
        self.version
    }
}

impl fmt::Display for StoredVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.version.fmt(f)
    }
}

/// How a stored definition version fits the token a caller presented
/// (`spec.md:1441-1448`, ADR-0030 §2.3's migration row).
///
/// Four variants, because migration is **access-driven** and the four outcomes are different
/// actions, not degrees of one. Upstream writes the comparison out at each access site; here
/// [`classify_version`] is one pure function and the enum makes the "newer stored version" arm
/// impossible to forget.
///
/// [`classify_version`]: crate::classify_version
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VersionFit {
    /// `stored == token`: use the value as read.
    Current,
    /// `stored < token` and the definition supplied a migration: call it.
    ///
    /// `spec.md:1461-1463`: *"The first `tx.doc()` transaction after any stored-version migration
    /// writes a required current-version base, even when the migrated JSON is deeply equal."*
    Migrate {
        /// The version the stored records were written by, which the callback is told.
        from: DefVersion,
        /// The version to migrate to.
        to: DefVersion,
    },
    /// `stored > token`: reject typed access. Extension code older than its own data must not
    /// reinterpret it (`spec.md:1443`).
    StoredIsNewer {
        /// The stored version.
        stored: DefVersion,
        /// The token's version.
        token: DefVersion,
    },
    /// `stored < token` and the definition supplied no migration: reject
    /// (`spec.md:1444`, *"no migrate -> reject older stored version"*).
    NoMigration {
        /// The stored version.
        stored: DefVersion,
        /// The token's version.
        token: DefVersion,
    },
}

/// Classify a stored version against a token's version. Pure, total, and the only place the
/// four-way comparison is written.
///
/// `has_migration` is whether the definition declared a migration callback at all — not whether it
/// will succeed. A failing callback is a transaction that persists nothing
/// (`spec.md:1457-1459`), which is the Session's concern in S5, not this function's.
#[must_use]
pub fn classify_version(
    stored: StoredVersion,
    token: DefVersion,
    has_migration: bool,
) -> VersionFit {
    let stored = stored.version();
    match stored.cmp(&token) {
        core::cmp::Ordering::Equal => VersionFit::Current,
        core::cmp::Ordering::Greater => VersionFit::StoredIsNewer { stored, token },
        core::cmp::Ordering::Less if has_migration => VersionFit::Migrate {
            from: stored,
            to: token,
        },
        core::cmp::Ordering::Less => VersionFit::NoMigration { stored, token },
    }
}
