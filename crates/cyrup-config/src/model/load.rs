//! Reading model files off disk: the JSONC pre-pass, the strict loader, and the reporting
//! loader that degrades every failure mode into a message instead of an error (R-07-023).

use std::path::Path;

use cyrup_provider::Model;

use crate::error::ConfigError;

use super::schema::ModelFile;
use super::validate::{render_schema_errors, validate_models_config};

/// Load custom OpenAI/Anthropic/Google-compatible model defs from a `models.json` (R-07-023).
pub fn load_custom_models(path: &Path) -> Result<Vec<Model>, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ConfigError::Io(e)),
    };
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let models: Vec<Model> = serde_json::from_str(&text)?;
    Ok(models)
}
/// Strip `//` line comments and trailing commas from JSON, leaving string literals untouched — a
/// 1:1 port of Pi's `stripJsonComments` (coding-agent/src/utils/json.ts), which every `models.json`
/// read goes through (`JSON.parse(stripJsonComments(content))`, model-config.ts:257).
///
/// Written as a single scanning pass rather than the two regex replaces, because Rust's `regex`
/// crate has no backreference-free equivalent of the alternation trick and a scanner is exact.
fn strip_json_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    // Byte offsets in `out` of pending `,` characters that may turn out to be trailing.
    let mut pending_comma: Option<usize> = None;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                pending_comma = None;
                out.push(c);
                let mut escaped = false;
                for sc in chars.by_ref() {
                    out.push(sc);
                    if escaped {
                        escaped = false;
                    } else if sc == '\\' {
                        escaped = true;
                    } else if sc == '"' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                for sc in chars.by_ref() {
                    if sc == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ',' => {
                pending_comma = Some(out.len());
                out.push(c);
            }
            '}' | ']' => {
                if let Some(at) = pending_comma.take() {
                    // Everything between the comma and here is whitespace (any other char cleared
                    // `pending_comma`), so the comma is trailing: drop it.
                    out.remove(at);
                }
                out.push(c);
            }
            c if c.is_whitespace() => out.push(c),
            c => {
                pending_comma = None;
                out.push(c);
            }
        }
    }
    out
}

/// Load a `models.json` provider-config file (Pi's `{ providers: {...} }` shape). A missing or
/// empty file yields an empty [`ModelFile`]. JSONC `//` comments and trailing commas are stripped
/// first, exactly as Pi does (model-config.ts:257). This is additive alongside
/// [`load_custom_models`] (which reads the legacy flat `Vec<Model>` shape).
pub fn load_models_file(path: &Path) -> Result<ModelFile, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ModelFile::default()),
        Err(e) => return Err(ConfigError::Io(e)),
    };
    if text.trim().is_empty() {
        return Ok(ModelFile::default());
    }
    let file: ModelFile = serde_json::from_str(&strip_json_comments(&text))?;
    Ok(file)
}
/// Load `<agent_dir>/models.json` into a composed [`ModelFile`], turning EVERY failure mode into a
/// human-readable message instead of an error the caller might treat as fatal.
///
/// Pi keeps a `ModelConfig` with an empty provider map plus one distinct error string per failure —
/// load / parse / schema (model-config.ts:251, :261, :271) — and the agent starts normally with the
/// built-in registry. This mirrors that contract: the returned `ModelFile` is empty on failure and
/// the `Option<String>` is the diagnostic the startup panel renders.
pub fn load_models_file_reporting(path: &Path) -> (ModelFile, Option<String>) {
    let empty = |msg: String| (ModelFile::default(), Some(msg));
    // Tier 1 — read (`ModelConfig.load`'s catch at model-config.ts:251-256 @v0.83.0). ENOENT is an
    // empty snapshot with NO message (`:250`).
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (ModelFile::default(), None),
        Err(e) => {
            return empty(format!(
                "Failed to load models.json: {e}\n\nFile: {}",
                path.display()
            ));
        }
    };
    if text.trim().is_empty() {
        return (ModelFile::default(), None);
    }
    // Tier 2 — JSON syntax (`JSON.parse(stripJsonComments(content))`, `:259-270`).
    let value: serde_json::Value = match serde_json::from_str(&strip_json_comments(&text)) {
        Ok(v) => v,
        Err(e) => {
            return empty(format!(
                "Failed to parse models.json: {e}\n\nFile: {}",
                path.display()
            ));
        }
    };
    // Tier 3 — schema (`validateModelsConfig.Check`, `:265-279`). EVERY failing field is reported,
    // by dotted key path, under a heading distinct from the syntax one.
    let schema_errors = validate_models_config(&value);
    if !schema_errors.is_empty() {
        return empty(format!(
            "Invalid models.json schema:\n{}\n\nFile: {}",
            render_schema_errors(&schema_errors),
            path.display()
        ));
    }
    match serde_json::from_value::<ModelFile>(value) {
        Ok(file) => (file, None),
        // A typing failure the hand-written validator above does not cover (today: only `compat`'s
        // three-arm union) is still a SCHEMA failure in Pi's model, not a syntax one.
        Err(e) => empty(format!(
            "Invalid models.json schema:\n  - {e}\n\nFile: {}",
            path.display()
        )),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::model::fixtures::model;

    #[test]
    fn load_custom_models_roundtrip() {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("models.json");
        let models = vec![model("custom", "my-model", "My Model")];
        std::fs::write(&path, serde_json::to_string(&models).unwrap()).unwrap();
        let loaded = load_custom_models(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.first().unwrap().id.as_str(), "my-model");
        // missing file → empty
        assert!(
            load_custom_models(&dir.join("nope.json"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn models_json_jsonc_comments_and_trailing_commas_are_stripped() {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("models.json");
        std::fs::write(
            &path,
            "{\n  // leading comment\n  \"providers\": {\n    \"acme\": {\n      \"baseUrl\": \"https://acme.test/v1\", // trailing note\n      \"models\": [{ \"id\": \"a1\" },]\n    },\n  }\n}\n",
        )
        .unwrap();
        let file = load_models_file(&path)
            .expect("JSONC models.json must parse like Pi's stripJsonComments");
        assert_eq!(file.providers.len(), 1);
        assert_eq!(file.providers["acme"].models.len(), 1);
        // A `//` sequence INSIDE a string literal survives.
        std::fs::write(
            &path,
            r#"{"providers":{"acme":{"baseUrl":"https://acme.test/v1"}}}"#,
        )
        .unwrap();
        let file = load_models_file(&path).unwrap();
        assert_eq!(
            file.providers["acme"].base_url.as_deref(),
            Some("https://acme.test/v1")
        );
    }

    /// CFG-046 + CFG-043: pi types `name`/`baseUrl`/`apiKey`/`api` as
    /// `Type.Optional(Type.String({ minLength: 1 }))` (model-config.ts:188-198 @v0.83.0), so an
    /// empty string FAILS `validateModelsConfig.Check` and `ModelConfig.load` returns an empty
    /// provider map plus `Invalid models.json schema:` with one `  - <dotted.path>: <message>` line
    /// per failure (`:272-279`) — a heading distinct from the JSON-syntax one.
    ///
    /// Red at HEAD: no length check anywhere, so `"baseUrl": ""` composed every model of that
    /// provider onto an empty endpoint while the file was reported as VALID; and a wrong-typed
    /// field surfaced as serde's byte-offset message under `Failed to parse models.json`.
    #[test]
    fn models_json_schema_failures_are_reported_per_field_not_as_a_parse_error() {
        let dir = crate::test_util::temp_dir();

        let path = dir.join("empty-base-url.json");
        std::fs::write(&path, r#"{"providers":{"x":{"baseUrl":""}}}"#).unwrap();
        let (file, err) = load_models_file_reporting(&path);
        assert!(file.providers.is_empty());
        let err = err.expect("an empty baseUrl must be a schema failure");
        assert!(err.starts_with("Invalid models.json schema:"), "{err}");
        assert!(
            err.contains("  - providers.x.baseUrl: Expected string length greater or equal to 1"),
            "{err}"
        );

        let path = dir.join("wrong-type.json");
        std::fs::write(
            &path,
            r#"{"providers":{"mycorp":{"models":[{"id":"m","contextWindow":"big"}]}}}"#,
        )
        .unwrap();
        let (_file, err) = load_models_file_reporting(&path);
        let err = err.expect("a wrong-typed field must be a schema failure");
        assert!(err.starts_with("Invalid models.json schema:"), "{err}");
        assert!(
            err.contains("providers.mycorp.models.0.contextWindow: Expected number"),
            "{err}"
        );

        // A JSON SYNTAX error keeps its own distinct heading (model-config.ts:265-270).
        let path = dir.join("syntax.json");
        std::fs::write(&path, "{ not json").unwrap();
        let (_file, err) = load_models_file_reporting(&path);
        assert!(
            err.unwrap().starts_with("Failed to parse models.json"),
            "syntax errors must not be relabelled as schema errors"
        );
    }

    #[test]
    fn malformed_models_json_reports_instead_of_erroring_out() {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("models.json");
        std::fs::write(&path, "{ not json").unwrap();
        let (file, err) = load_models_file_reporting(&path);
        assert!(file.providers.is_empty());
        let err = err.expect("a parse failure must be reported");
        assert!(err.contains("Failed to parse models.json"), "{err}");
        // A missing file is NOT an error (Pi returns an empty snapshot on ENOENT, model-config.ts:248).
        let (file, err) = load_models_file_reporting(&dir.join("absent.json"));
        assert!(file.providers.is_empty() && err.is_none());
    }

    /// Pi types `oauth` as `Type.Literal("radius")` (model-config.ts:194), so any other spelling is
    /// a SCHEMA failure that empties the whole file and reports one error (model-config.ts:265-272)
    /// — not a silently-ignored key. cyrup's serde loader reaches the same contract through
    /// `load_models_file_reporting`.
    #[test]
    fn models_json_rejects_an_unknown_oauth_mode_for_the_whole_file() {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("models.json");
        std::fs::write(
            &path,
            r#"{"providers":{"acme":{"oauth":"anthropic","baseUrl":"https://x.test/v1"}}}"#,
        )
        .unwrap();
        let (file, err) = load_models_file_reporting(&path);
        assert!(
            file.providers.is_empty(),
            "an invalid schema empties the file"
        );
        let err = err.expect("and reports why");
        assert!(
            err.contains("radius"),
            "the message names the legal value: {err}"
        );
    }
}
