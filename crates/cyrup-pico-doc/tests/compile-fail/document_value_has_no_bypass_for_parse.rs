//! `G-INV-6`, value half — ADR-0030 §2.1 invariant 6 and §2.3 (`spec.md:72, 1356-1369`): *"only
//! strict JSON enters a document, rejected at the offending assignment."*
//!
//! ADR-0030 F4's *"guarantee not gained"* item 2 states the obligation this case pins:
//! `DocValue::parse` *"must accept `impl Serialize` and must **not** also offer a path taking a
//! pre-built `serde_json::Value`; if it ever does, that path needs the equivalent walk."* The reason
//! is specific and read from the dependency's source: `serde_json::to_value(f64::NAN)` is
//! `Ok(Value::Null)`, so a `From<serde_json::Value>` here would be a silent data-loss path.

use std::sync::Arc;

use cyrup_pico_doc::{DocMap, DocRoot, DocValue};

fn main() {
    // No conversion from a pre-built `serde_json::Value`: that is the `to_value` path, and it loses
    // a `NaN` as a `null` before this crate ever sees it.
    let forged: DocValue = serde_json::Value::Null.into();

    // No `From<f64>` that skips the finite check either.
    let from_float: DocValue = f64::NAN.into();

    // And a root cannot be struct-literalled past `DocRoot::parse`'s object requirement.
    let literal = DocRoot(Arc::new(DocMap::new()));

    let _ = (forged, from_float, literal);
}
