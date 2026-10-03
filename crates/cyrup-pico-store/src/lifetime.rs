//! Incarnation membership as a half-open interval (ADR-0030 §2.2 row 3, F6 §C;
//! `spec.md:1112-1114`).

use core::fmt;

use serde::de::{Error as DeError, IgnoredAny, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Seq;

/// A document incarnation's membership interval, `created_at <= at < retired_at`
/// (`spec.md:1112-1114`).
///
/// Upstream carries two `Seq` fields on the record and writes the interval test at each read site,
/// of which there are two. The failure that permits is a retire-then-create at one logical address
/// overlapping or gapping at the boundary sequence, which is exactly where the `<=`/`<` asymmetry
/// lives. Here the fields are private, [`Lifetime::contains`] is the only membership accessor, and
/// the asymmetry has one implementation (ADR-0030 §2.2: `guarded`).
///
/// # The empty lifetime is legal; the inverted one is not
///
/// *"A creation retired in the same commit has an empty lifetime"* (`spec.md:1114`), so
/// `retired_at == created_at` is a real, reachable state that [`Lifetime::new`] must accept and
/// that [`Lifetime::contains`] answers `false` to for every sequence. `retired_at < created_at` is
/// not reachable from any commit and is rejected, including on the way in from storage — a decoded
/// inverted interval is corruption, not a value to normalise.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Lifetime {
    created_at: Seq,
    retired_at: Option<Seq>,
}

/// `retired_at` was before `created_at`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, thiserror::Error)]
#[error("lifetime is inverted: retired at {retired_at} before created at {created_at}")]
pub struct InvertedLifetime {
    /// The interval's lower bound.
    pub created_at: Seq,
    /// The upper bound, which was below it.
    pub retired_at: Seq,
}

impl Lifetime {
    /// An unretired incarnation: no upper bound (`spec.md:1113`).
    #[must_use]
    pub const fn open(created_at: Seq) -> Self {
        Self {
            created_at,
            retired_at: None,
        }
    }

    /// A creation retired by the commit that created it: the empty lifetime (`spec.md:1114`).
    ///
    /// Infallible, which is why it exists beside [`Lifetime::new`]. `spec.md:4364-4365` stamps both
    /// bounds of a create-plus-retire with *the same* batch sequence, and that one pair can never be
    /// inverted — so the commit path that needs it should not have to handle an error arm it cannot
    /// reach, and `DocumentCreate::stamp` can stay infallible with no `expect`.
    #[must_use]
    pub const fn retired_at_creation(at: Seq) -> Self {
        Self {
            created_at: at,
            retired_at: Some(at),
        }
    }

    /// The validating constructor.
    ///
    /// Accepts `retired_at == created_at` (the empty lifetime of a creation retired in its own
    /// commit) and rejects `retired_at < created_at`.
    pub fn new(created_at: Seq, retired_at: Option<Seq>) -> Result<Self, InvertedLifetime> {
        if let Some(r) = retired_at
            && r < created_at
        {
            return Err(InvertedLifetime {
                created_at,
                retired_at: r,
            });
        }
        Ok(Self {
            created_at,
            retired_at,
        })
    }

    /// Retire an incarnation at `at`.
    ///
    /// Rejects a retirement before the creation, and accepts one at the creation sequence, which
    /// yields the empty lifetime. Takes `self` by value: retiring is a transition to a new value,
    /// not a mutation of a record a reader may be holding.
    pub fn retire(self, at: Seq) -> Result<Self, InvertedLifetime> {
        Self::new(self.created_at, Some(at))
    }

    /// Whether this incarnation is a member of the store at sequence `at`.
    ///
    /// The **only** membership accessor, and the only implementation of the half-open test:
    /// `created_at <= at` and, when retired, `at < retired_at`. An unretired incarnation has no
    /// upper bound.
    #[must_use]
    pub fn contains(&self, at: Seq) -> bool {
        self.created_at <= at && self.retired_at.is_none_or(|retired| at < retired)
    }

    /// Whether this incarnation was retired in the commit that created it, so no sequence is ever
    /// a member.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.retired_at == Some(self.created_at)
    }

    /// Whether this incarnation has no upper bound.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.retired_at.is_none()
    }

    /// When this incarnation was retired, if it was.
    ///
    /// **Not** a membership accessor — [`Lifetime::contains`] remains the only one, and remains the
    /// only place the `<=`/`<` asymmetry is written. This returns the bound for a *diagnostic*: a
    /// commit that writes to an already-retired incarnation is rejected, and
    /// `RejectedReason::DocumentRetired` names the sequence it was retired at. Reading a `Seq` forges
    /// nothing, because a `Seq` is only ever allocated by a store.
    #[must_use]
    pub const fn retired_at(&self) -> Option<Seq> {
        self.retired_at
    }

    /// When this incarnation was created.
    ///
    /// Same reasoning as [`Lifetime::retired_at`]: a diagnostic and an ordering key for a backend's
    /// content index, not a second implementation of membership.
    #[must_use]
    pub const fn created_at(&self) -> Seq {
        self.created_at
    }
}

const CREATED_AT: &str = "createdAt";
const RETIRED_AT: &str = "retiredAt";

/// Upstream's persisted shape: two sibling fields on the document record (`spec.md:4218-4221`),
/// with `retiredAt` absent while the incarnation is live. Hand-written so the pair is written as
/// the two fields S3's record will `#[serde(flatten)]`, rather than as a nested object.
impl Serialize for Lifetime {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let len = 1 + usize::from(self.retired_at.is_some());
        let mut map = s.serialize_map(Some(len))?;
        map.serialize_entry(CREATED_AT, &self.created_at)?;
        if let Some(retired) = self.retired_at {
            map.serialize_entry(RETIRED_AT, &retired)?;
        }
        map.end()
    }
}

struct LifetimeVisitor;

impl<'de> Visitor<'de> for LifetimeVisitor {
    type Value = Lifetime;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object with a `createdAt` commit sequence and an optional `retiredAt`")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut created_at: Option<Seq> = None;
        let mut retired_at: Option<Option<Seq>> = None;
        while let Some(key) = map.next_key::<field::Key>()? {
            match key {
                field::Key::CreatedAt => {
                    if created_at.is_some() {
                        return Err(A::Error::duplicate_field(CREATED_AT));
                    }
                    created_at = Some(map.next_value()?);
                }
                field::Key::RetiredAt => {
                    if retired_at.is_some() {
                        return Err(A::Error::duplicate_field(RETIRED_AT));
                    }
                    retired_at = Some(map.next_value()?);
                }
                // Unknown keys are ignored rather than rejected, because S3's `DocumentRecord`
                // reaches this impl through `#[serde(flatten)]`, which hands the visitor the whole
                // record's map. Rejecting here would reject every other field of the record.
                field::Key::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let created_at = created_at.ok_or_else(|| A::Error::missing_field(CREATED_AT))?;
        Lifetime::new(created_at, retired_at.flatten()).map_err(A::Error::custom)
    }
}

mod field {
    //! A three-way key discriminator, so the visitor never allocates a `String` for a field name
    //! and an unknown key costs nothing.

    use core::fmt;

    use serde::de::{Error as DeError, Visitor};
    use serde::{Deserialize, Deserializer};

    pub(super) enum Key {
        CreatedAt,
        RetiredAt,
        Other,
    }

    struct KeyVisitor;

    impl<'de> Visitor<'de> for KeyVisitor {
        type Value = Key;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a field name")
        }

        fn visit_str<E: DeError>(self, v: &str) -> Result<Self::Value, E> {
            Ok(match v {
                super::CREATED_AT => Key::CreatedAt,
                super::RETIRED_AT => Key::RetiredAt,
                _ => Key::Other,
            })
        }
    }

    impl<'de> Deserialize<'de> for Key {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            d.deserialize_identifier(KeyVisitor)
        }
    }
}

/// Hand-written, and this is the one impl in the crate where deriving would admit a value the
/// constructor forbids outright rather than merely a sloppier one: a derived impl over the two
/// private fields reconstructs an **inverted** interval from a damaged file without complaint, and
/// an inverted interval makes `contains` answer `false` everywhere, so a live incarnation silently
/// disappears from every historical read.
impl<'de> Deserialize<'de> for Lifetime {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_map(LifetimeVisitor)
    }
}
