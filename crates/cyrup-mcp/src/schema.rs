//! `json-schema-validator.ts` — the dual-dialect gate.
//!
//! rmcp validates NOTHING client-side (there is no `jsonSchemaValidator` option on
//! `Peer<RoleClient>`, unlike the TS SDK's), so this is not optional and not a duplicate.
//!
//! One validator, two consumers: [`crate::elicitation`]'s final assertion and 13b's `outputSchema`
//! gate. Two validators in one crate that disagree about whether `format` is an assertion is the
//! failure this module exists to prevent.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::Value;

use crate::errors::{McpError, McpResult};

/// `DRAFT_07_SCHEMA_URIS` (`json-schema-validator.ts:18-21`).
const DRAFT_07_URIS: [&str; 2] = [
    "http://json-schema.org/draft-07/schema",
    "https://json-schema.org/draft-07/schema",
];
/// `DRAFT_2020_12_SCHEMA_URIS` (`json-schema-validator.ts:22-24`).
const DRAFT_2020_12_URIS: [&str; 1] = ["https://json-schema.org/draft/2020-12/schema"];

/// `schemaDialect(schema)` (`json-schema-validator.ts:26-34`).
///
/// A NON-STRING `$schema` is `unstamped`, not an error — `typeof schema.$schema !== "string"`. And
/// exactly ONE trailing `#` is stripped, so `…/schema##` stays unrecognised.
fn dialect(schema: &Value) -> Option<&str> {
    let raw = schema.get("$schema")?.as_str()?;
    Some(raw.strip_suffix('#').unwrap_or(raw))
}

/// `Unsupported JSON Schema dialect: ${uri}` (`json-schema-validator.ts:53`).
#[must_use]
pub fn unsupported_dialect_message(uri: &str) -> String {
    format!("Unsupported JSON Schema dialect: {uri}")
}

/// `createJsonSchemaValidator().getValidator(schema)` — compile, or say which dialect was refused.
///
/// `should_validate_formats(true)` is the whole point: `jsonschema` treats `format` as an
/// ANNOTATION by default, which would silently disable `format: "email"` — the one constraint the
/// elicitation coercion pass cannot express.
///
/// # Errors
///
/// [`unsupported_dialect_message`] for a stamped-but-unknown dialect; the compiler's own message for
/// a schema that will not build.
pub fn compile(schema: &Value) -> McpResult<jsonschema::Validator> {
    match dialect(schema) {
        // Unstamped and 2020-12 both take the 2020-12 arm — upstream's `??=` order.
        None => {}
        Some(uri) if DRAFT_2020_12_URIS.contains(&uri) => {}
        Some(uri) if DRAFT_07_URIS.contains(&uri) => {}
        Some(uri) => return Err(McpError::Config(unsupported_dialect_message(uri))),
    }
    jsonschema::options()
        .should_validate_formats(true)
        .build(schema)
        .map_err(|error| McpError::Config(error.to_string()))
}

/// The per-schema compile cache. `coerceAndValidateFormValues` runs once per field AND once per
/// review pass over the same `requestedSchema`, so an uncached compile is O(fields²).
///
/// Keyed on the canonical JSON text rather than on a pointer: the single-property synthetic schemas
/// `collect_valid_field` builds are fresh values every iteration, and only a content key dedupes
/// them.
#[derive(Default)]
pub struct ValidatorCache {
    entries: Mutex<HashMap<String, Arc<jsonschema::Validator>>>,
}

impl ValidatorCache {
    /// Compile `schema`, or hand back the compilation this cache already holds for it.
    ///
    /// A poisoned lock is recovered with `PoisonError::into_inner`, the same policy
    /// `server_manager.rs` uses and for the same reason: `unwrap_used` is denied crate-wide and a
    /// half-written cache is not representable — the map is only ever inserted into.
    ///
    /// # Errors
    ///
    /// Whatever [`compile`] returns for a schema this cache has not seen.
    pub fn get_or_compile(&self, schema: &Value) -> McpResult<Arc<jsonschema::Validator>> {
        // `serde_json::to_string` on a `Value` is key-sorted for objects (serde_json's map is a
        // BTreeMap unless `preserve_order` is on), so this is a canonical key.
        let key = schema.to_string();
        {
            let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(hit) = entries.get(&key) {
                return Ok(Arc::clone(hit));
            }
        }
        // Compiled OUTSIDE the lock: `jsonschema::build` is not trivial and a second thread
        // compiling the same schema concurrently is cheaper than serialising every caller behind
        // one mutex. The insert below is last-write-wins over an identical value.
        let compiled = Arc::new(compile(schema)?);
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(Arc::clone(
            entries.entry(key).or_insert_with(|| Arc::clone(&compiled)),
        ))
    }
}

impl std::fmt::Debug for ValidatorCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = self
            .entries
            .lock()
            .map(|entries| entries.len())
            .unwrap_or_default();
        f.debug_struct("ValidatorCache")
            .field("entries", &len)
            .finish()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn both_dialects_and_an_unstamped_schema_compile() {
        for stamp in [
            None,
            Some("https://json-schema.org/draft/2020-12/schema"),
            Some("http://json-schema.org/draft-07/schema"),
            // Exactly one trailing `#` is stripped.
            Some("http://json-schema.org/draft-07/schema#"),
        ] {
            let mut schema = json!({"type": "object"});
            if let Some(uri) = stamp
                && let Some(object) = schema.as_object_mut()
            {
                object.insert("$schema".to_string(), json!(uri));
            }
            assert!(compile(&schema).is_ok(), "{stamp:?} should compile");
        }
    }

    #[test]
    fn a_stamped_but_unknown_dialect_names_the_uri() {
        let schema = json!({"$schema": "https://example.invalid/schema", "type": "object"});
        let error = compile(&schema).expect_err("an unknown dialect is refused");
        assert!(
            error.to_string().contains(&unsupported_dialect_message(
                "https://example.invalid/schema"
            )),
            "got {error}"
        );
    }

    /// `…/schema##` keeps one `#` after the strip and is therefore NOT draft-07.
    #[test]
    fn only_one_trailing_hash_is_stripped() {
        let schema = json!({"$schema": "http://json-schema.org/draft-07/schema##"});
        assert!(
            compile(&schema).is_err(),
            "a double hash stays unrecognised"
        );
    }

    /// A non-string `$schema` is `unstamped` to [`dialect`] — `typeof schema.$schema !== "string"`
    /// — so it never reaches the unsupported-dialect arm.
    ///
    /// It still does not compile, and that divergence belongs to the validator rather than to this
    /// dispatch: `jsonschema` enforces the spec's "`$schema` MUST be a string", where ajv's default
    /// configuration does not. Pinned so the distinction stays visible — the failure a user sees is
    /// the compiler's message, never [`unsupported_dialect_message`].
    #[test]
    fn a_non_string_schema_key_is_unstamped_but_still_will_not_compile() {
        let schema = json!({"$schema": 7, "type": "object"});
        assert_eq!(dialect(&schema), None, "a non-string stamp is unstamped");
        let error = compile(&schema).expect_err("the validator enforces the spec");
        assert!(
            !error
                .to_string()
                .contains("Unsupported JSON Schema dialect"),
            "this must be the compiler's refusal, not the dialect arm's; got {error}"
        );
    }

    /// The reason `should_validate_formats(true)` exists: without it `format` is an annotation and
    /// this assertion silently passes.
    #[test]
    fn format_is_an_assertion_not_an_annotation() {
        let schema = json!({"type": "string", "format": "email"});
        let validator = compile(&schema).expect("compiles");
        assert!(validator.validate(&json!("not-an-email")).is_err());
        assert!(validator.validate(&json!("someone@example.com")).is_ok());
    }

    #[test]
    fn the_cache_returns_the_same_compilation_for_an_equal_schema() {
        let cache = ValidatorCache::default();
        let first = cache
            .get_or_compile(&json!({"type": "object"}))
            .expect("compiles");
        // A DIFFERENT `Value` with the same content — the case `collect_valid_field` produces.
        let second = cache
            .get_or_compile(&json!({"type": "object"}))
            .expect("cached");
        assert!(
            Arc::ptr_eq(&first, &second),
            "an equal schema must hit the cache"
        );
    }
}
