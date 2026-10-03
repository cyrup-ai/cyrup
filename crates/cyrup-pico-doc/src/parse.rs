//! [`DocValue::parse`]: the sole construction path from host data, as a `serde::Serializer`.
//!
//! # Why a `Serializer` and not a validating `Deserialize`
//!
//! `docs/RUST-DESIGN-REVIEW.md:73` usually points at the decode boundary, and for every *other*
//! type in this kernel it is right. Here it points the other way, and ADR-0030 F4 says why: the
//! read direction is already safe, because JSON has no `NaN` literal and `serde_json::Number`'s own
//! `Deserialize` cannot yield a non-finite one. The *write* direction is where the loss happens:
//!
//! ```text
//! serde_json-1.0.x  src/value/ser.rs   fn serialize_f64(self, float: f64) -> Result<Value> { Ok(Value::from(float)) }
//! serde_json-1.0.x  src/value/from.rs  fn from(f: f64) -> Self { Number::from_f64(f).map_or(Value::Null, Value::Number) }
//! serde_json-1.0.x  src/number.rs      pub fn from_f64(f: f64) -> Option<Number> { if f.is_finite() { .. } else { None } }
//! ```
//!
//! So `serde_json::to_value(f64::NAN)` is **`Ok(Value::Null)`**. It does not error. The commit
//! succeeds, the publication carries `null`, and on reopen the field is permanently `null` — the
//! number is gone and nothing was raised. pi catches this because Chord's copy walk rejects
//! non-finite numbers at the offending assignment (`spec.md:1356-1362`); a straight Rust port
//! using `to_value` does not.
//!
//! **`serde_json::to_value` therefore must not appear on this crate's document path**, and
//! `tests/parse.rs::nan_is_rejected_rather_than_becoming_null` is the artefact that stops it coming
//! back. `serde_json::Value` is not constructed anywhere in this crate.
//!
//! # What this serializer is
//!
//! `serde_json`'s own `value::Serializer`, differing in exactly three ways: `serialize_f64` and
//! `serialize_f32` reject non-finite instead of substituting `null`; every nesting step checks
//! [`MAX_DEPTH`], because a host `Serialize` over a cyclic `Rc` graph recurses without bound and
//! `#![forbid(unsafe_code)]` does not stop a stack overflow; and the output type is [`DocValue`],
//! so there is no `serde_json::Value` on the path to forget to convert.

use std::sync::Arc;

use serde::Serialize;
use serde::ser::{
    SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant, Serializer,
};

use crate::value::{DocMap, DocRoot, DocValue, JsonNum, MAX_DEPTH, NotStrictJson};

impl DocValue {
    /// Parse host data into a document value, rejecting everything that is not strict JSON.
    ///
    /// This is the one real runtime check ADR-0030 F4 leaves standing, and it stands at exactly one
    /// boundary. There is no `From<serde_json::Value>`, no `From<f64>` and no other public
    /// constructor that takes an arbitrary number.
    ///
    /// # Errors
    ///
    /// [`NotStrictJson`] when the value contains a non-finite float, an out-of-range 128-bit
    /// integer, a non-string object key, or nesting past [`MAX_DEPTH`]; or when the host's own
    /// `Serialize` implementation fails.
    pub fn parse<T: Serialize + ?Sized>(value: &T) -> Result<Self, NotStrictJson> {
        value.serialize(DocSer { depth: 0 })
    }
}

impl DocRoot {
    /// Parse host data into a document root, additionally requiring a JSON object.
    ///
    /// This is `track()`'s and `prepareReplace()`'s entry gate (`spec.md:1340-1348`): *"the Session
    /// copies each one into exclusive kernel ownership, rejecting any value that is not strict
    /// JSON, before it becomes an immutable tracker revision."* After it, ownership is the
    /// mechanism and there is nothing left to copy.
    ///
    /// # Errors
    ///
    /// Everything [`DocValue::parse`] rejects, plus [`NotStrictJson::RootIsNotAnObject`].
    pub fn parse<T: Serialize + ?Sized>(value: &T) -> Result<Self, NotStrictJson> {
        Self::from_value(DocValue::parse(value)?)
    }
}

/// The serializer. `depth` is the number of containers already open.
#[derive(Clone, Copy)]
struct DocSer {
    depth: u32,
}

impl DocSer {
    /// Open one container, refusing to go past [`MAX_DEPTH`].
    fn descend(self) -> Result<Self, NotStrictJson> {
        let depth = self.depth.saturating_add(1);
        if depth > MAX_DEPTH {
            return Err(NotStrictJson::TooDeep);
        }
        Ok(Self { depth })
    }

    /// The one float rule. Everything this module exists for is these four lines.
    fn float(self, v: f64) -> Result<DocValue, NotStrictJson> {
        JsonNum::from_f64(v)
            .map(DocValue::Num)
            .ok_or(NotStrictJson::NonFinite(v))
    }
}

impl Serializer for DocSer {
    type Ok = DocValue;
    type Error = NotStrictJson;
    type SerializeSeq = SerList;
    type SerializeTuple = SerList;
    type SerializeTupleStruct = SerList;
    type SerializeTupleVariant = SerVariantList;
    type SerializeMap = SerMap;
    type SerializeStruct = SerMap;
    type SerializeStructVariant = SerVariantMap;

    fn serialize_bool(self, v: bool) -> Result<DocValue, NotStrictJson> {
        Ok(DocValue::Bool(v))
    }

    fn serialize_i8(self, v: i8) -> Result<DocValue, NotStrictJson> {
        self.serialize_i64(i64::from(v))
    }

    fn serialize_i16(self, v: i16) -> Result<DocValue, NotStrictJson> {
        self.serialize_i64(i64::from(v))
    }

    fn serialize_i32(self, v: i32) -> Result<DocValue, NotStrictJson> {
        self.serialize_i64(i64::from(v))
    }

    fn serialize_i64(self, v: i64) -> Result<DocValue, NotStrictJson> {
        Ok(DocValue::Num(JsonNum::from(v)))
    }

    fn serialize_i128(self, v: i128) -> Result<DocValue, NotStrictJson> {
        i64::try_from(v)
            .map(|n| DocValue::Num(JsonNum::from(n)))
            .map_err(|_| NotStrictJson::IntegerOutOfRange(v.to_string()))
    }

    fn serialize_u8(self, v: u8) -> Result<DocValue, NotStrictJson> {
        self.serialize_u64(u64::from(v))
    }

    fn serialize_u16(self, v: u16) -> Result<DocValue, NotStrictJson> {
        self.serialize_u64(u64::from(v))
    }

    fn serialize_u32(self, v: u32) -> Result<DocValue, NotStrictJson> {
        self.serialize_u64(u64::from(v))
    }

    fn serialize_u64(self, v: u64) -> Result<DocValue, NotStrictJson> {
        Ok(DocValue::Num(JsonNum::from(v)))
    }

    fn serialize_u128(self, v: u128) -> Result<DocValue, NotStrictJson> {
        u64::try_from(v)
            .map(|n| DocValue::Num(JsonNum::from(n)))
            .map_err(|_| NotStrictJson::IntegerOutOfRange(v.to_string()))
    }

    /// Widened to `f64` first, as `serde_json` does, so `1.5f32` and `1.5f64` agree in the bytes.
    fn serialize_f32(self, v: f32) -> Result<DocValue, NotStrictJson> {
        self.float(f64::from(v))
    }

    fn serialize_f64(self, v: f64) -> Result<DocValue, NotStrictJson> {
        self.float(v)
    }

    fn serialize_char(self, v: char) -> Result<DocValue, NotStrictJson> {
        Ok(DocValue::Str(Arc::from(v.to_string().as_str())))
    }

    fn serialize_str(self, v: &str) -> Result<DocValue, NotStrictJson> {
        Ok(DocValue::Str(Arc::from(v)))
    }

    /// Bytes become an array of numbers, as `serde_json` does: JSON has no byte type, and a
    /// silently different encoding here would make a round-trip asymmetric.
    fn serialize_bytes(self, v: &[u8]) -> Result<DocValue, NotStrictJson> {
        let items = v.iter().map(|b| DocValue::Num(JsonNum::from(*b))).collect();
        Ok(DocValue::List(Arc::new(items)))
    }

    fn serialize_none(self) -> Result<DocValue, NotStrictJson> {
        Ok(DocValue::Null)
    }

    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<DocValue, NotStrictJson> {
        v.serialize(self)
    }

    fn serialize_unit(self) -> Result<DocValue, NotStrictJson> {
        Ok(DocValue::Null)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<DocValue, NotStrictJson> {
        Ok(DocValue::Null)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<DocValue, NotStrictJson> {
        self.serialize_str(variant)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        v: &T,
    ) -> Result<DocValue, NotStrictJson> {
        v.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        v: &T,
    ) -> Result<DocValue, NotStrictJson> {
        let inner = self.descend()?;
        let mut map = DocMap::with_capacity(1);
        map.insert(Arc::from(variant), v.serialize(inner)?);
        Ok(DocValue::Map(Arc::new(map)))
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<SerList, NotStrictJson> {
        Ok(SerList {
            inner: self.descend()?,
            items: Vec::with_capacity(len.unwrap_or(0)),
        })
    }

    fn serialize_tuple(self, len: usize) -> Result<SerList, NotStrictJson> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<SerList, NotStrictJson> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<SerVariantList, NotStrictJson> {
        let inner = self.descend()?;
        Ok(SerVariantList {
            variant,
            list: SerList {
                inner: inner.descend()?,
                items: Vec::with_capacity(len),
            },
        })
    }

    fn serialize_map(self, len: Option<usize>) -> Result<SerMap, NotStrictJson> {
        Ok(SerMap {
            inner: self.descend()?,
            entries: DocMap::with_capacity(len.unwrap_or(0)),
            pending_key: None,
        })
    }

    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<SerMap, NotStrictJson> {
        self.serialize_map(Some(len))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<SerVariantMap, NotStrictJson> {
        let inner = self.descend()?;
        Ok(SerVariantMap {
            variant,
            map: SerMap {
                inner: inner.descend()?,
                entries: DocMap::with_capacity(len),
                pending_key: None,
            },
        })
    }
}

/// An array under construction.
struct SerList {
    inner: DocSer,
    items: Vec<DocValue>,
}

impl SerializeSeq for SerList {
    type Ok = DocValue;
    type Error = NotStrictJson;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), NotStrictJson> {
        self.items.push(v.serialize(self.inner)?);
        Ok(())
    }

    fn end(self) -> Result<DocValue, NotStrictJson> {
        Ok(DocValue::List(Arc::new(self.items)))
    }
}

impl SerializeTuple for SerList {
    type Ok = DocValue;
    type Error = NotStrictJson;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), NotStrictJson> {
        SerializeSeq::serialize_element(self, v)
    }

    fn end(self) -> Result<DocValue, NotStrictJson> {
        SerializeSeq::end(self)
    }
}

impl SerializeTupleStruct for SerList {
    type Ok = DocValue;
    type Error = NotStrictJson;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), NotStrictJson> {
        SerializeSeq::serialize_element(self, v)
    }

    fn end(self) -> Result<DocValue, NotStrictJson> {
        SerializeSeq::end(self)
    }
}

/// `{ "Variant": [ .. ] }`, as `serde_json` externally tags a tuple variant.
struct SerVariantList {
    variant: &'static str,
    list: SerList,
}

impl SerializeTupleVariant for SerVariantList {
    type Ok = DocValue;
    type Error = NotStrictJson;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), NotStrictJson> {
        SerializeSeq::serialize_element(&mut self.list, v)
    }

    fn end(self) -> Result<DocValue, NotStrictJson> {
        let mut map = DocMap::with_capacity(1);
        map.insert(Arc::from(self.variant), SerializeSeq::end(self.list)?);
        Ok(DocValue::Map(Arc::new(map)))
    }
}

/// An object under construction. JSON object keys are strings, so a key that serializes as anything
/// else is rejected here rather than coerced — a coerced key is a silently different document.
struct SerMap {
    inner: DocSer,
    entries: DocMap,
    pending_key: Option<Arc<str>>,
}

impl SerializeMap for SerMap {
    type Ok = DocValue;
    type Error = NotStrictJson;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), NotStrictJson> {
        // Keys are parsed at depth 0: a key is a scalar, and charging it the container's depth
        // would make the limit depend on whether a value sits in an object or an array.
        match key.serialize(DocSer { depth: 0 })? {
            DocValue::Str(s) => {
                self.pending_key = Some(s);
                Ok(())
            }
            other => Err(NotStrictJson::KeyIsNotAString {
                found: other.type_name(),
            }),
        }
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), NotStrictJson> {
        let key = self
            .pending_key
            .take()
            .ok_or(NotStrictJson::ValueBeforeKey)?;
        let value = v.serialize(self.inner)?;
        self.entries.insert(key, value);
        Ok(())
    }

    fn end(self) -> Result<DocValue, NotStrictJson> {
        Ok(DocValue::Map(Arc::new(self.entries)))
    }
}

impl SerializeStruct for SerMap {
    type Ok = DocValue;
    type Error = NotStrictJson;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        v: &T,
    ) -> Result<(), NotStrictJson> {
        let value = v.serialize(self.inner)?;
        self.entries.insert(Arc::from(key), value);
        Ok(())
    }

    fn end(self) -> Result<DocValue, NotStrictJson> {
        SerializeMap::end(self)
    }
}

/// `{ "Variant": { .. } }`, as `serde_json` externally tags a struct variant.
struct SerVariantMap {
    variant: &'static str,
    map: SerMap,
}

impl SerializeStructVariant for SerVariantMap {
    type Ok = DocValue;
    type Error = NotStrictJson;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        v: &T,
    ) -> Result<(), NotStrictJson> {
        SerializeStruct::serialize_field(&mut self.map, key, v)
    }

    fn end(self) -> Result<DocValue, NotStrictJson> {
        let mut map = DocMap::with_capacity(1);
        map.insert(Arc::from(self.variant), SerializeMap::end(self.map)?);
        Ok(DocValue::Map(Arc::new(map)))
    }
}
