//! [`SharedStr`] — an append-only string shared by every snapshot taken from one stream (PERF-001).

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

/// The append target shared with every snapshot taken from it.
type Buf = Arc<RwLock<String>>;

// A poisoned buffer still holds a perfectly valid `String` — the panic that poisoned it happened
// somewhere else — so recovering it is correct, and it keeps the whole type infallible rather than
// pushing a `Result` into `Deref`, which cannot have one.
fn read(buf: &Buf) -> std::sync::RwLockReadGuard<'_, String> {
    buf.read().unwrap_or_else(PoisonError::into_inner)
}
fn write(buf: &Buf) -> std::sync::RwLockWriteGuard<'_, String> {
    buf.write().unwrap_or_else(PoisonError::into_inner)
}

/// A string that behaves exactly like `String` at every read site but is O(1) to clone.
///
/// A streaming decoder rebuilds the whole `AssistantMessage` for the `partial` on every delta, so
/// an owned `String` in a content block is copied once per delta — O(N²) over a turn. Every
/// snapshot taken from one block instead shares one growing buffer and remembers only how much of
/// it that snapshot may see, so taking a snapshot is a refcount bump and a `usize` (PERF-001).
///
/// Two properties make that safe to substitute for `String` everywhere:
///
/// * **Reads look identical.** [`Deref<Target = str>`](std::ops::Deref) covers the ~230 sites that
///   pattern-match a block and read its text, and `Serialize`/`Deserialize` are byte-for-byte
///   `String`'s, so nothing on the wire, in a session file or in an RPC frame moves.
/// * **Writes keep value semantics.** [`Self::push_str`] appends in place only while this handle
///   is still the buffer's tail; a handle that is not forks to its own buffer first. Two snapshots
///   of the same block can therefore be appended to independently and neither sees the other's
///   bytes, exactly as two `String`s would behave.
///
/// The flat `&str` that call sites expect is built on FIRST read and cached, so a snapshot nobody
/// reads — which is every in-flight `partial` in production, since the TUI fold and the json seam
/// both discard it — costs nothing at all. [`Self::is_materialised`] is the hook that lets that be
/// asserted rather than timed.
pub struct SharedStr {
    /// `None` for a string with no shared writer; the whole value then lives in `flat`.
    shared: Option<(Buf, usize)>,
    /// The flat form, materialised on FIRST read and cached. Refcounted so a clone can carry
    /// one that a reader already paid for without copying it.
    flat: OnceLock<Arc<str>>,
}

impl SharedStr {
    /// An empty string with no shared writer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: None,
            flat: OnceLock::new(),
        }
    }

    /// The flat `&str`, building and caching it on first call.
    ///
    /// This is the ONLY materialisation point: [`Deref`](std::ops::Deref), `Display`, `Debug`,
    /// `PartialEq`, `Hash`, `Ord` and `Serialize` all route through it.
    pub fn as_str(&self) -> &str {
        self.flat
            .get_or_init(|| match &self.shared {
                Some((buf, len)) => Arc::from(read(buf).get(..*len).unwrap_or_default()),
                None => Arc::from(""),
            })
            .as_ref()
    }

    /// `true` once a flat form exists for this handle.
    ///
    /// The point of this type is that a snapshot nobody reads costs nothing, which is a property of
    /// the streaming decoders that has to be ASSERTED rather than timed; this is the hook that makes
    /// it assertable.
    ///
    /// Read it as "a flat form exists", not "THIS handle paid for one": since [`Clone`] carries an
    /// already-built flat across (a refcount bump, not a copy), a clone of a handle something else
    /// read reports `true` without having built anything. That is what the flag needs to mean for
    /// the assertion to do its job — it fails exactly when a flat form was BUILT somewhere it should
    /// not have been, which is the regression worth catching, and a handle that merely shares one is
    /// not paying for it.
    pub fn is_materialised(&self) -> bool {
        self.flat.get().is_some()
    }

    /// Byte length — answered WITHOUT materialising.
    #[must_use]
    pub fn len(&self) -> usize {
        match (&self.shared, self.flat.get()) {
            (Some((_, len)), _) => *len,
            (None, Some(f)) => f.len(),
            (None, None) => 0,
        }
    }
    /// Whether the string is empty — answered WITHOUT materialising.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append, sharing the buffer with the writer while this handle is still its tail.
    ///
    /// The `guard.len() == len` test is what preserves `String`'s value semantics. A handle that is
    /// no longer the tail — because a sibling snapshot of the same block appended first — forks to
    /// its own buffer rather than appending onto bytes it cannot see. Two handles that are both at
    /// the tail self-heal: the first appends in place and thereby moves the tail past the second,
    /// whose own length check then fails and forks it.
    pub fn push_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        match self.shared.take() {
            Some((buf, len)) => {
                // Invalidate the cached flat form ONLY on this branch: the standalone branch below
                // still needs it, and taking it up front silently drops the existing value.
                let _ = self.flat.take();
                let mut guard = write(&buf);
                if guard.len() == len {
                    guard.push_str(s);
                    let new_len = guard.len();
                    drop(guard);
                    self.shared = Some((buf, new_len));
                } else {
                    let mut owned = String::with_capacity(len + s.len());
                    owned.push_str(guard.get(..len).unwrap_or_default());
                    owned.push_str(s);
                    drop(guard);
                    // `owned.len()`, not `len + s.len()`: the prefix read is written defensively
                    // (the invariant says `len` is always a char boundary within the buffer), and
                    // deriving the new length from the string that actually exists keeps the two
                    // in step even if that defence ever fires.
                    let new_len = owned.len();
                    self.shared = Some((Arc::new(RwLock::new(owned)), new_len));
                }
            }
            None => {
                // `Arc<str>` cannot hand its allocation back the way `Box::into_string` could, so
                // promoting a standalone handle costs one copy. It happens at most ONCE per value:
                // the first append moves it to `shared` and nothing ever moves it back.
                let mut owned = match self.flat.take() {
                    Some(f) => f.to_string(),
                    None => String::new(),
                };
                owned.push_str(s);
                let len = owned.len();
                self.shared = Some((Arc::new(RwLock::new(owned)), len));
            }
        }
    }

    /// Append one character, with [`Self::push_str`]'s semantics.
    pub fn push(&mut self, c: char) {
        self.push_str(c.encode_utf8(&mut [0u8; 4]));
    }
}

impl Clone for SharedStr {
    /// O(1) in every state: the shared buffer is a refcount bump, and the flat form — if some
    /// reader already paid for it — is an `Arc` bump too rather than a copy.
    ///
    /// Carrying the flat over is not a materialisation: it is only ever set when something already
    /// read THIS handle, and a clone sees the identical `len` prefix of the identical append-only
    /// buffer, so the cached value is exactly right for it. A clone of an UNREAD handle still
    /// carries nothing, which is what keeps "a snapshot nobody reads materialises nothing" true.
    ///
    /// The standalone case is why this matters beyond the streaming path: a block whose payload was
    /// REPLACED at block end (`text_end`, `arguments.done`) holds its value in `flat` alone, and
    /// before this it was memcpy'd into every later snapshot of the turn.
    fn clone(&self) -> Self {
        let flat = OnceLock::new();
        if let Some(f) = self.flat.get() {
            let _ = flat.set(Arc::clone(f));
        }
        Self {
            shared: self.shared.clone(),
            flat,
        }
    }
}

impl Default for SharedStr {
    fn default() -> Self {
        Self::new()
    }
}
impl std::ops::Deref for SharedStr {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}
impl AsRef<str> for SharedStr {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}
impl std::borrow::Borrow<str> for SharedStr {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}
impl std::fmt::Display for SharedStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self.as_str(), f)
    }
}
impl std::fmt::Debug for SharedStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_str(), f)
    }
}
impl std::fmt::Write for SharedStr {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.push_str(s);
        Ok(())
    }
}
impl PartialEq for SharedStr {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}
impl Eq for SharedStr {}
impl PartialEq<str> for SharedStr {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}
impl PartialEq<&str> for SharedStr {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}
impl PartialEq<String> for SharedStr {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}
impl PartialEq<SharedStr> for str {
    fn eq(&self, other: &SharedStr) -> bool {
        self == other.as_str()
    }
}
impl PartialEq<SharedStr> for &str {
    fn eq(&self, other: &SharedStr) -> bool {
        *self == other.as_str()
    }
}
impl PartialEq<SharedStr> for String {
    fn eq(&self, other: &SharedStr) -> bool {
        self.as_str() == other.as_str()
    }
}
impl PartialOrd for SharedStr {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for SharedStr {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}
impl Hash for SharedStr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}
impl From<String> for SharedStr {
    fn from(s: String) -> Self {
        let flat = OnceLock::new();
        let _ = flat.set(Arc::from(s));
        Self { shared: None, flat }
    }
}
impl From<&str> for SharedStr {
    fn from(s: &str) -> Self {
        Self::from(s.to_owned())
    }
}
impl From<&SharedStr> for SharedStr {
    /// Same as [`Clone`]: shares the buffer, materialises nothing.
    fn from(s: &SharedStr) -> Self {
        s.clone()
    }
}
impl From<&String> for SharedStr {
    fn from(s: &String) -> Self {
        Self::from(s.clone())
    }
}
impl From<Box<str>> for SharedStr {
    fn from(s: Box<str>) -> Self {
        let flat = OnceLock::new();
        let _ = flat.set(Arc::from(s));
        Self { shared: None, flat }
    }
}
impl From<Cow<'_, str>> for SharedStr {
    fn from(s: Cow<'_, str>) -> Self {
        Self::from(s.into_owned())
    }
}
impl From<SharedStr> for String {
    fn from(s: SharedStr) -> String {
        s.as_str().to_owned()
    }
}
impl std::str::FromStr for SharedStr {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(s))
    }
}
impl serde::Serialize for SharedStr {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}
impl<'de> serde::Deserialize<'de> for SharedStr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        <String as serde::Deserialize>::deserialize(d).map(SharedStr::from)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    /// The fork rule is what lets this stand in for `String`: a handle that is no longer the
    /// buffer's tail must not append onto bytes it cannot see.
    #[test]
    fn appending_keeps_string_value_semantics() {
        let mut a = SharedStr::from("abc");
        let b = a.clone();
        a.push_str("X");
        let mut c = b.clone();
        c.push_str("Y");
        let mut d = a.clone();
        d.push_str("Z");
        assert_eq!(a.as_str(), "abcX");
        assert_eq!(b.as_str(), "abc", "a snapshot never sees a later append");
        assert_eq!(c.as_str(), "abcY");
        assert_eq!(d.as_str(), "abcXZ");

        // Two handles that are BOTH the tail: the first appends in place, the second forks.
        let mut p = SharedStr::from("q");
        let mut q = p.clone();
        p.push_str("1");
        q.push_str("2");
        assert_eq!(p.as_str(), "q1");
        assert_eq!(q.as_str(), "q2");

        // Snapshots of a growing buffer freeze their own prefix.
        let mut w = SharedStr::new();
        let mut snaps = Vec::new();
        let mut expect = String::new();
        for i in 0..64u32 {
            w.push_str(&i.to_string());
            snaps.push(w.clone());
            expect.push_str(&i.to_string());
            assert_eq!(snaps[i as usize].as_str(), expect.as_str());
        }
    }

    /// The wire must not move: `SharedStr` is `String` in both directions, byte for byte.
    #[test]
    fn serde_is_byte_identical_to_string() {
        for s in [
            "",
            "plain",
            "quote\"back\\slash",
            "unicode ✓ 𝄞",
            "line\nbreak\ttab",
            "\u{1}ctl",
        ] {
            let as_string = serde_json::to_string(&s.to_string()).expect("serialize String");
            let as_shared =
                serde_json::to_string(&SharedStr::from(s)).expect("serialize SharedStr");
            assert_eq!(as_string, as_shared, "serialize {s:?}");
            let back: SharedStr = serde_json::from_str(&as_shared).expect("deserialize");
            assert_eq!(back.as_str(), s);
        }
    }

    /// A clone must be O(1) whatever state the handle is in — including a STANDALONE handle whose
    /// flat form an earlier reader already paid for.
    ///
    /// That is the state a block lands in when its payload is REPLACED at block end
    /// (`text_end`, `arguments.done`), and before the memo was refcounted it was memcpy'd into
    /// every later snapshot of the turn: the last O(bytes x deltas) term in the streamed `partial`.
    #[test]
    fn cloning_is_o1_for_a_standalone_handle_too() {
        let big: String = std::iter::repeat_n('x', 4 * 1024 * 1024).collect();
        let finished = SharedStr::from(big.clone());
        assert_eq!(
            finished.as_str().len(),
            big.len(),
            "read it, as a consumer would"
        );
        assert!(finished.is_materialised());

        // 2000 clones of a materialised 4 MB standalone. Each one is DROPPED before the next is
        // taken, so peak memory stays at the original plus one clone: a regression to a copying
        // `Clone` then fails the bound below instead of retaining 8 GB and taking the box with it.
        let t = std::time::Instant::now();
        let mut bytes_seen = 0usize;
        for _ in 0..2000 {
            let c = finished.clone();
            bytes_seen += c.len();
            std::hint::black_box(&c);
        }
        let elapsed = t.elapsed();
        assert_eq!(bytes_seen, big.len() * 2000);
        assert_eq!(
            finished.clone().as_str(),
            big.as_str(),
            "a clone reads what the original does"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(200),
            "2000 clones of a materialised 4 MB handle took {elapsed:?} — the flat form is being \
             copied, so a finished block is back to costing O(bytes) per later snapshot"
        );
    }

    /// A clone nobody reads must not inherit a flat form some other handle paid for.
    #[test]
    fn cloning_does_not_materialise() {
        let mut w = SharedStr::new();
        w.push_str("payload");
        let snap = w.clone();
        assert!(!snap.is_materialised());
        assert_eq!(snap.len(), 7, "length is answered without materialising");
        assert!(!snap.is_materialised());
        let read_once = snap.clone();
        assert!(
            !read_once.is_materialised(),
            "a clone of an unread handle carries nothing"
        );
        assert_eq!(read_once.as_str(), "payload");
        assert!(read_once.is_materialised());
        assert!(
            !snap.is_materialised(),
            "reading one handle does not materialise its siblings"
        );
        // A clone taken AFTER the read does inherit the flat — that is the O(1) path — and it must
        // still read the same bytes.
        let inheriting = read_once.clone();
        assert!(inheriting.is_materialised());
        assert_eq!(inheriting.as_str(), "payload");
    }
}
