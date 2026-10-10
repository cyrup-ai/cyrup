//! Published JSON Schemas for cyrup's user-edited configuration files — `settings.json`,
//! `models.json` and `keybindings.json` — so an editor pointed at them (`"$schema": "<url>"`) gets
//! completion, hover documentation and validation. CFG-106.
//!
//! Port of pi's `packages/coding-agent/scripts/generate-schemas.ts` @f1b2e77f5 (`7f9e1198f`,
//! #9880). pi builds each document from TypeBox source (`core/settings-schema.ts`,
//! `core/model-config.ts`, `core/keybindings-schema.ts`, `theme-schema.ts`), compacts repeated
//! sub-schemas into `$defs` (`addDefinitions`, `:84-127`), stamps `$schema` / `$id` / `$comment`
//! (`serializeSchema`, `:129-141`) and commits the output under `schemas/`; its test
//! `matches the four committed artifacts` (`test/config-schemas.test.ts:33-47`) fails when a
//! committed file is stale. cyrup does the same: [`render_config_schemas`] builds the documents
//! and `src/tests/config_schemas.rs` compares them byte-for-byte with
//! `crates/cyrup-config/schemas/*.schema.json`, rewriting them when
//! `CYRUP_REGENERATE_SCHEMAS=1` is set.
//!
//! **[CYRUP-DELTA] The theme schema is not emitted.** pi's fourth artifact,
//! `schemas/theme.schema.json`, encodes `theme-schema.ts`'s `additionalProperties: false` at the
//! top level, in `colors` and in `export`. cyrup's theme loader (`cyrup-resources/src/theme.rs`)
//! still accepts unknown keys (TUI-178), so a strict published schema would flag files cyrup loads
//! without complaint. It is tracked as its own row until TUI-178 lands.
//!
//! The documents are hand-ported from pi's TypeBox source rather than derived from cyrup's Rust
//! types: `Settings` is a raw JSON map with no typed struct to derive from, and pi's constraints
//! (`exclusiveMinimum`, `minLength`, the key-id pattern) and its editor descriptions would need
//! attributes on every field anyway. Every value a schema states as a `default` is read from the
//! cyrup getter that applies it ([`crate::EffectiveSettings`] over an empty document), so the
//! published default cannot drift from the runtime one.

mod keybindings;
mod models;
mod settings;
mod typebox;

pub use keybindings::{KEYBINDING_SCHEMA_DESCRIPTIONS, key_id_pattern};

use serde_json::{Map, Value, json};

/// Where the committed documents are served from — the same mechanism as pi's `schemaBaseUrl`
/// (`generate-schemas.ts:21`, a `raw.githubusercontent.com` path into the package), pointed at
/// this crate's `schemas/` directory.
pub const SCHEMA_BASE_URL: &str =
    "https://raw.githubusercontent.com/cyrup-ai/cyrup/main/crates/cyrup-config";

/// pi's `schemaDraft` (`generate-schemas.ts:22`).
const SCHEMA_DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

/// The `$comment` stamped on every document (pi's `generatedComment`, `generate-schemas.ts:23-24`,
/// with cyrup's regeneration command).
const GENERATED_COMMENT: &str = "This file is generated from Rust source \
     (crates/cyrup-config/src/config_schemas). Do not edit it manually; regenerate with \
     CYRUP_REGENERATE_SCHEMAS=1 cargo test -p cyrup-config config_schemas.";

/// Paths of the generated documents, relative to `crates/cyrup-config`, in pi's artifact order
/// (`generate-schemas.ts:32-59`, less the theme schema — see the module doc).
pub const CONFIG_SCHEMA_PATHS: [&str; 3] = [
    "schemas/models.schema.json",
    "schemas/settings.schema.json",
    "schemas/keybindings.schema.json",
];

/// A generator failure. Each one is a programming error in this module, surfaced as a value
/// because the crate denies `panic`/`expect`.
#[derive(Debug, thiserror::Error)]
pub enum ConfigSchemaError {
    /// pi's `Invalid schema definition name` (`generate-schemas.ts:87-89`).
    #[error("invalid schema definition name: {0}")]
    InvalidDefinitionName(String),
    /// pi's `Unused schema definitions` (`generate-schemas.ts:120-125`): a declared `$defs` entry
    /// that no sub-schema matched.
    #[error("unused schema definitions: {}", .0.join(", "))]
    UnusedDefinitions(Vec<String>),
    /// pi's `mergeCompatProperties` errors (`compat-schema.ts:605-623`).
    #[error("{0}")]
    CompatMerge(String),
    #[error("could not serialize schema: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// One artifact: its path and root schema, plus the named sub-schemas to compact into `$defs`
/// (pi's `SchemaArtifact`, `generate-schemas.ts:26-30`).
struct SchemaArtifact {
    path: &'static str,
    schema: Value,
    definitions: Vec<(&'static str, Value)>,
}

fn artifacts() -> [SchemaArtifact; 3] {
    let [models_path, settings_path, keybindings_path] = CONFIG_SCHEMA_PATHS;
    [
        SchemaArtifact {
            path: models_path,
            schema: models::models_config_schema(),
            definitions: models::definitions(),
        },
        SchemaArtifact {
            path: settings_path,
            schema: settings::settings_schema(),
            definitions: settings::definitions(),
        },
        SchemaArtifact {
            path: keybindings_path,
            schema: keybindings::keybindings_schema(),
            definitions: keybindings::definitions(),
        },
    ]
}

/// The annotation keywords `addDefinitions` keeps at each use site when it swaps a sub-schema for
/// a `$ref` (pi's `schemaAnnotationKeys`, `generate-schemas.ts:61-69`).
const ANNOTATION_KEYS: [&str; 7] = [
    "default",
    "deprecated",
    "description",
    "examples",
    "readOnly",
    "title",
    "writeOnly",
];

fn is_annotation(key: &str) -> bool {
    ANNOTATION_KEYS.contains(&key)
}

/// pi's `splitAnnotations` (`generate-schemas.ts:71-81`).
fn split_annotations(schema: &Map<String, Value>) -> (Map<String, Value>, Map<String, Value>) {
    let mut shape = Map::new();
    let mut annotations = Map::new();
    for (key, value) in schema {
        let side = if is_annotation(key) {
            &mut annotations
        } else {
            &mut shape
        };
        side.insert(key.clone(), value.clone());
    }
    (shape, annotations)
}

struct PreparedDefinition {
    name: &'static str,
    shape: Value,
}

/// pi's `replaceDefinitions` closure (`generate-schemas.ts:94-112`): a sub-schema whose
/// annotation-free shape equals a definition's becomes `{ $ref, ...annotations }`; annotation
/// values themselves are never rewritten.
fn replace_definitions(
    value: &Value,
    definitions: &[PreparedDefinition],
    excluded: Option<&str>,
    referenced: &mut Vec<&'static str>,
) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| replace_definitions(item, definitions, excluded, referenced))
                .collect(),
        ),
        Value::Object(obj) => {
            let (shape, annotations) = split_annotations(obj);
            let shape = Value::Object(shape);
            if let Some(found) = definitions
                .iter()
                .find(|d| Some(d.name) != excluded && d.shape == shape)
            {
                if !referenced.contains(&found.name) {
                    referenced.push(found.name);
                }
                let mut out = Map::new();
                out.insert("$ref".into(), format!("#/$defs/{}", found.name).into());
                out.extend(annotations);
                return Value::Object(out);
            }
            Value::Object(
                obj.iter()
                    .map(|(key, child)| {
                        let child = if is_annotation(key) {
                            child.clone()
                        } else {
                            replace_definitions(child, definitions, excluded, referenced)
                        };
                        (key.clone(), child)
                    })
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// pi's `addDefinitions` (`generate-schemas.ts:84-127`).
fn add_definitions(
    schema: &Value,
    definitions: &[(&'static str, Value)],
) -> Result<Value, ConfigSchemaError> {
    let mut prepared = Vec::with_capacity(definitions.len());
    for (name, definition) in definitions {
        let valid_name = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !valid_name {
            return Err(ConfigSchemaError::InvalidDefinitionName(
                (*name).to_string(),
            ));
        }
        let shape = match definition {
            Value::Object(obj) => Value::Object(split_annotations(obj).0),
            other => other.clone(),
        };
        prepared.push(PreparedDefinition { name, shape });
    }

    let mut referenced = Vec::new();
    let compact = replace_definitions(schema, &prepared, None, &mut referenced);
    let mut compact_definitions = Map::new();
    for definition in &prepared {
        let body = replace_definitions(
            &definition.shape,
            &prepared,
            Some(definition.name),
            &mut referenced,
        );
        compact_definitions.insert(definition.name.to_string(), body);
    }
    let unused: Vec<String> = prepared
        .iter()
        .filter(|d| !referenced.contains(&d.name))
        .map(|d| d.name.to_string())
        .collect();
    if !unused.is_empty() {
        return Err(ConfigSchemaError::UnusedDefinitions(unused));
    }
    let mut out = match compact {
        Value::Object(obj) => obj,
        other => return Ok(other),
    };
    out.insert("$defs".into(), Value::Object(compact_definitions));
    Ok(Value::Object(out))
}

/// pi's `serializeSchema` (`generate-schemas.ts:129-141`): `$schema`, `$id`, `$comment`, then the
/// compacted document, pretty-printed with two-space indentation and a trailing newline.
fn serialize_schema(artifact: &SchemaArtifact) -> Result<String, ConfigSchemaError> {
    let schema = if artifact.definitions.is_empty() {
        artifact.schema.clone()
    } else {
        add_definitions(&artifact.schema, &artifact.definitions)?
    };
    let mut document = Map::new();
    document.insert("$schema".into(), SCHEMA_DRAFT.into());
    document.insert(
        "$id".into(),
        format!("{SCHEMA_BASE_URL}/{}", artifact.path).into(),
    );
    document.insert("$comment".into(), GENERATED_COMMENT.into());
    if let Value::Object(body) = schema {
        document.extend(body);
    }
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(document))?
    ))
}

/// Render every published configuration schema as `(path, text)`, in [`CONFIG_SCHEMA_PATHS`]
/// order (pi's `renderConfigSchemas`, `generate-schemas.ts:143-150`). `path` is relative to
/// `crates/cyrup-config`.
pub fn render_config_schemas() -> Result<Vec<(&'static str, String)>, ConfigSchemaError> {
    models::check_compat_merge()?;
    artifacts()
        .iter()
        .map(|artifact| Ok((artifact.path, serialize_schema(artifact)?)))
        .collect()
}

/// The full URL a document is published at — the value a config file's `"$schema"` key should
/// carry.
pub fn config_schema_url(path: &str) -> String {
    format!("{SCHEMA_BASE_URL}/{path}")
}

/// `{ "$schema": Type.Optional(Type.String(...)) }`, the key every pi document accepts so a file
/// can name its own schema.
fn schema_reference_property(description: Option<&str>) -> Value {
    match description {
        Some(description) => json!({ "type": "string", "description": description }),
        None => typebox::string(),
    }
}
