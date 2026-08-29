//! [`LazyArgs`] — tool-call arguments parsed on first read (PERF-001).

use crate::json::parse_streaming_json_object;
use crate::shared_str::SharedStr;
use serde_json::{Map, Value};
use std::sync::{Arc, OnceLock};

/// Tool-call `arguments` that present as `Map<String, Value>` but cost O(1) to snapshot.
///
/// `serde_json::Value` is foreign and has no shared-string variant, so the tool path cannot share
/// its payload the way [`SharedStr`] does — a `Map` holds its strings by value. It can defer
/// BUILDING it: a snapshot carries only the raw accumulated buffer, and the `Map` is recovered the
/// first time something actually reads it (PERF-001). A decoder that rebuilds the `partial` on
/// every delta therefore stops paying for a parse per delta, and in production pays for none at
/// all — the TUI fold and the json seam both discard `partial` unread.
///
/// The recovery is [`parse_streaming_json_object`] over the whole buffer, which is exactly what
/// the decoders called per delta before, so nothing about the recovered value changes — including
/// the salvage of a truncated buffer, whose semantics that function owns. Reads look identical via
/// [`Deref<Target = Map<String, Value>>`](std::ops::Deref), and `Serialize`/`Deserialize` are the
/// `Map`'s own, so a non-object is still rejected on the wire and the wire bytes do not move.
pub struct LazyArgs {
    /// The raw accumulated tool-argument buffer, when this value came from a stream.
    raw: Option<SharedStr>,
    /// Refcounted so a clone can carry a map that a reader already paid for without copying it.
    parsed: OnceLock<Arc<Map<String, Value>>>,
}

impl LazyArgs {
    /// Arguments deferred behind the raw streaming buffer they are recovered from.
    #[must_use]
    pub fn streaming(raw: SharedStr) -> Self {
        Self { raw: Some(raw), parsed: OnceLock::new() }
    }

    /// The materialised map, parsing on first call.
    ///
    /// Deliberately NOT named `get`: an inherent method wins over [`Deref`](std::ops::Deref), so a
    /// `get` here would shadow `Map::get` and silently break every `arguments.get("key")` call
    /// site.
    pub fn as_map(&self) -> &Map<String, Value> {
        self.parsed
            .get_or_init(|| Arc::new(parse_streaming_json_object(self.raw.as_deref())))
            .as_ref()
    }

    /// `true` once a parsed map exists for this value.
    ///
    /// The point of this type is that a snapshot nobody reads costs nothing, which is a property of
    /// the streaming decoders that has to be ASSERTED rather than timed; this is the hook that makes
    /// it assertable. Read it the way [`SharedStr::is_materialised`] is read: "a map exists", not
    /// "THIS value built one" — a clone shares an already-parsed map rather than re-parsing.
    pub fn is_materialised(&self) -> bool {
        self.parsed.get().is_some()
    }
}

impl Clone for LazyArgs {
    /// O(1) in every state, for the reasons given on [`SharedStr::clone`]: the raw buffer is a
    /// refcount bump, and a map some reader already paid for is shared rather than deep-copied.
    fn clone(&self) -> Self {
        let parsed = OnceLock::new();
        if let Some(m) = self.parsed.get() {
            let _ = parsed.set(Arc::clone(m));
        }
        Self { raw: self.raw.clone(), parsed }
    }
}

impl Default for LazyArgs {
    fn default() -> Self {
        Self::from(Map::new())
    }
}
impl std::ops::Deref for LazyArgs {
    type Target = Map<String, Value>;
    fn deref(&self) -> &Map<String, Value> {
        self.as_map()
    }
}
impl std::fmt::Debug for LazyArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_map(), f)
    }
}
impl PartialEq for LazyArgs {
    fn eq(&self, other: &Self) -> bool {
        self.as_map() == other.as_map()
    }
}
impl Eq for LazyArgs {}
impl From<Map<String, Value>> for LazyArgs {
    fn from(m: Map<String, Value>) -> Self {
        let parsed = OnceLock::new();
        let _ = parsed.set(Arc::new(m));
        Self { raw: None, parsed }
    }
}
impl From<LazyArgs> for Map<String, Value> {
    /// Reclaims the map without copying when this is the last handle on it, and clones only when a
    /// sibling snapshot still shares it.
    fn from(a: LazyArgs) -> Self {
        let _ = a.as_map();
        match a.parsed.into_inner() {
            Some(arc) => Arc::try_unwrap(arc).unwrap_or_else(|shared| (*shared).clone()),
            None => Map::new(),
        }
    }
}
impl<'a> IntoIterator for &'a LazyArgs {
    type Item = (&'a String, &'a Value);
    type IntoIter = serde_json::map::Iter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.as_map().iter()
    }
}
impl serde::Serialize for LazyArgs {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.as_map().serialize(s)
    }
}
impl<'de> serde::Deserialize<'de> for LazyArgs {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        <Map<String, Value> as serde::Deserialize>::deserialize(d).map(LazyArgs::from)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::json::StreamingArgs;

    /// Recovering the map from the whole buffer on demand must agree, byte for byte, with the
    /// incremental scanner fed the same bytes in arbitrary pieces — including on the truncated and
    /// malformed buffers a cut-off stream leaves behind.
    #[test]
    fn deferred_recovery_matches_the_incremental_scanner() {
        let buffers = [
            r#"{"a":1,"b":"hello","c":true,"d":null,"e":-2.5e3}"#,
            r#"{"path":"/tmp/x","content":"line1\nline2 é 😀 end"#,
            r#"{"k":"v","nested":{"x":1},"after":2}"#,
            r#"{"arr":[1,2,3],"t":"z"#,
            r#"{"trunc":"abc\u00"#,
            r#"{"trunc":"abc\ud83d"#,
            r#"{"bad":tru"#,
            r#"{  "spaced"  :  "  v  "  }"#,
            "{",
            "",
            "not json at all",
        ];
        for buf in buffers {
            let lazy = LazyArgs::streaming(SharedStr::from(buf));
            for chunk in [1usize, 2, 3, 5, 7, 40, 1000] {
                let mut inc = StreamingArgs::default();
                let mut at = 0;
                while at < buf.len() {
                    let mut end = (at + chunk).min(buf.len());
                    while !buf.is_char_boundary(end) {
                        end += 1;
                    }
                    inc.feed(&buf[at..end]);
                    at = end;
                }
                assert_eq!(&*lazy, &inc.object(buf), "buffer {buf:?}, {chunk}-byte chunks");
            }
        }
    }

    /// The wire must not move, and a non-object must still be rejected.
    #[test]
    fn serde_is_byte_identical_to_map() {
        let m: Map<String, Value> =
            serde_json::from_str(r#"{"b":2,"a":"x","c":[1,{"d":null}]}"#).expect("map");
        assert_eq!(
            serde_json::to_string(&m).expect("map"),
            serde_json::to_string(&LazyArgs::from(m.clone())).expect("lazy")
        );
        let back: LazyArgs = serde_json::from_str(&serde_json::to_string(&m).expect("map"))
            .expect("deserialize");
        assert_eq!(&*back, &m);
        assert!(serde_json::from_str::<LazyArgs>("[1,2]").is_err());
        assert!(serde_json::from_str::<LazyArgs>("\"s\"").is_err());
    }

    /// A clone must be O(1) whatever state the value is in — including one built from an owned
    /// `Map` with no raw buffer behind it, which is what the terminal and non-streaming paths hold.
    #[test]
    fn cloning_a_parsed_lazyargs_is_o1() {
        let mut m = Map::new();
        for i in 0..2000 {
            m.insert(format!("k{i}"), Value::String("v".repeat(64)));
        }
        let args = LazyArgs::from(m);
        assert_eq!(args.len(), 2000);

        // Each clone is DROPPED before the next is taken — see [`SharedStr`]'s twin of this test for
        // why retaining them would turn a regression into an out-of-memory rather than a failure.
        let t = std::time::Instant::now();
        let mut keys_seen = 0usize;
        for _ in 0..1000 {
            let c = args.clone();
            keys_seen += c.len();
            std::hint::black_box(&c);
        }
        let elapsed = t.elapsed();
        assert_eq!(keys_seen, 2000 * 1000);
        assert_eq!(*args.clone(), *args, "a clone reads what the original does");
        assert!(
            elapsed < std::time::Duration::from_millis(200),
            "1000 clones of a parsed 2000-key map took {elapsed:?} — the map is being deep-copied"
        );
    }

    /// `From<LazyArgs> for Map` must hand back the right map whether or not a sibling still shares
    /// it: it reclaims the allocation when sole, and clones only when it cannot.
    #[test]
    fn into_map_is_correct_shared_or_sole() {
        let src = r#"{"a":1,"b":"two"}"#;
        let want = parse_streaming_json_object(Some(src));

        let sole = LazyArgs::streaming(SharedStr::from(src));
        assert_eq!(Map::from(sole), want, "sole handle reclaims the map");

        let shared = LazyArgs::streaming(SharedStr::from(src));
        let sibling = shared.clone();
        let _ = sibling.len(); // force the parse so both handles hold the same Arc
        let sibling2 = sibling.clone();
        assert_eq!(Map::from(sibling), want, "shared handle clones the map");
        assert_eq!(*sibling2, want, "the sibling is undisturbed");
        assert_eq!(Map::from(shared), want);
    }

    /// A snapshot nobody reads parses nothing, and a clone does not inherit a sibling's parse.
    #[test]
    fn cloning_does_not_parse() {
        let mut w = SharedStr::new();
        w.push_str(r#"{"k":"v"}"#);
        let snap = LazyArgs::streaming(w.clone());
        assert!(!snap.is_materialised());
        let other = snap.clone();
        assert!(!other.is_materialised(), "a clone of an unparsed handle carries nothing");
        assert_eq!(other.len(), 1);
        assert!(other.is_materialised());
        assert!(!snap.is_materialised(), "reading one handle does not parse its siblings");
        // A clone taken AFTER the parse inherits it — the O(1) path — and must still read the same.
        let inheriting = other.clone();
        assert!(inheriting.is_materialised());
        assert_eq!(inheriting.len(), 1);
    }
}
