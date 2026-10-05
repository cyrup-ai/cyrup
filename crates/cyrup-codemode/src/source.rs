//! The `codemode` source format: JavaScript, optionally preceded by one options line (pi
//! `packages/codemode/src/source.ts` @v1.0.1, CODE-003).
//!
//! ```js
//! // @options: {"max_output_tokens": 2000, "timeout_ms": 30000}
//! const text = await tools.read({ path: "package.json" });
//! text(JSON.parse(text).name);
//! ```
//!
//! # Production call path
//!
//! * [`parse_codemode_source`] is the first step of a `codemode` tool call: it splits the model's
//!   `code` argument into the script and its [`CodemodeSourceOptions`] (pi `execute.ts:329`). The
//!   options carry `max_output_tokens`, which feeds [`crate::output::truncate_output`], and
//!   `timeout_ms`, which feeds the sandbox's deadline.
//! * [`CODEMODE_SOURCE_GRAMMAR`] is what the `codemode` tool declares as its grammar-constrained
//!   input on providers that support it: `cyrup_core::constrained_sampling::GrammarVariants`'
//!   `openai_lark` field takes it as is (gap-analysis CODE-003).

use serde_json::Value;

use crate::js::{js_trim, js_trim_start, own_keys};

/// A first line that starts with this (after leading whitespace) is an options line
/// (`source.ts:11`).
pub const CODEMODE_OPTIONS_PREFIX: &str = "// @options:";

/// `source.ts:13`. Upstream's `SUPPORTED_FIELDS_TEXT` (`:14`) is spelled out in the
/// [`CodemodeSourceError`] messages.
const SUPPORTED_FIELDS: [&str; 2] = ["max_output_tokens", "timeout_ms"];
/// Largest delay `setTimeout` supports, which bounds `timeout_ms` (`source.ts:16`).
pub const MAX_TIMEOUT_MS: u64 = 2_147_483_647;
/// `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Lark grammar for providers with grammar-constrained tool input. It only fixes the shape of the
/// options line; the options JSON and the code are checked by [`parse_codemode_source`]. Byte
/// identical to upstream's `String.raw` template (`source.ts:22-30`), including the leading and
/// trailing newline.
pub const CODEMODE_SOURCE_GRAMMAR: &str = r#"
start: options_source | plain_source
options_source: OPTIONS_LINE NEWLINE SOURCE
plain_source: SOURCE

OPTIONS_LINE: /[ \t]*\/\/ @options:[^\r\n]*/
NEWLINE: /\r?\n/
SOURCE: /[\s\S]+/
"#;

/// Options an options line can carry (pi `CodemodeSourceOptions`, `source.ts:32`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CodemodeSourceOptions {
    /// Token budget for the script's output.
    pub max_output_tokens: Option<u64>,
    /// Hard deadline for the whole script in milliseconds, including tool calls.
    pub timeout_ms: Option<u64>,
}

/// A parsed `codemode` source (pi `ParsedCodemodeSource`, `source.ts:39`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedCodemodeSource {
    /// The script with the options line replaced by an empty line, so line numbers are unchanged.
    pub code: String,
    pub options: CodemodeSourceOptions,
}

/// Why a source was refused (pi `CodemodeSourceError`, `source.ts:45`). The `Display` text of each
/// variant is upstream's message verbatim, which is what the model reads back.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CodemodeSourceError {
    /// `source.ts:102-104`.
    #[error(
        "Expected JavaScript source text (non-empty). Provide JS only, optionally with a first line `// @options: {{\"max_output_tokens\": 1000}}`."
    )]
    EmptySource,
    /// `source.ts:112`.
    #[error("The @options line must be followed by JavaScript source on subsequent lines")]
    OptionsWithoutCode,
    /// An empty directive (`source.ts:58`) or one that is not a JSON object (`source.ts:69`).
    #[error(
        "@options must be a JSON object with supported fields `max_output_tokens` and `timeout_ms`"
    )]
    OptionsNotAnObject,
    /// `source.ts:64-66`. `reason` is the JSON parser's message; upstream appends V8's `SyntaxError`
    /// text there, which is engine specific and not reproducible byte for byte.
    #[error(
        "@options must be valid JSON with supported fields `max_output_tokens` and `timeout_ms`: {reason}"
    )]
    OptionsInvalidJson { reason: String },
    /// `source.ts:74`. Names the first unsupported key in `Object.keys` order.
    #[error("@options only supports `max_output_tokens` and `timeout_ms`; got `{field}`")]
    UnsupportedOption { field: String },
    /// `source.ts:81`.
    #[error("@options field `max_output_tokens` must be a non-negative safe integer")]
    InvalidMaxOutputTokens,
    /// `source.ts:87-89`.
    #[error("@options field `timeout_ms` must be a positive integer up to 2147483647")]
    InvalidTimeoutMs,
}

/// `isSafeInteger` (`source.ts:52`): a JSON number that is an integer in `0..=2^53 - 1`. JSON has
/// one number type, so `1.0` and `1e3` are integers here exactly as in JavaScript.
fn safe_integer(value: &Value) -> Option<u64> {
    let number = value.as_f64()?;
    if number >= 0.0 && number.fract() == 0.0 && number <= MAX_SAFE_INTEGER {
        // Exact: integral and below 2^53.
        Some(number as u64)
    } else {
        None
    }
}

fn parse_options(directive: &str) -> Result<CodemodeSourceOptions, CodemodeSourceError> {
    if directive.is_empty() {
        return Err(CodemodeSourceError::OptionsNotAnObject);
    }
    let value: Value = serde_json::from_str(directive).map_err(|error| {
        CodemodeSourceError::OptionsInvalidJson {
            reason: error.to_string(),
        }
    })?;
    let Value::Object(fields) = value else {
        return Err(CodemodeSourceError::OptionsNotAnObject);
    };
    for key in own_keys(&fields) {
        if !SUPPORTED_FIELDS.contains(&key) {
            return Err(CodemodeSourceError::UnsupportedOption {
                field: key.to_owned(),
            });
        }
    }
    let mut options = CodemodeSourceOptions::default();
    if let Some(value) = fields.get("max_output_tokens") {
        options.max_output_tokens =
            Some(safe_integer(value).ok_or(CodemodeSourceError::InvalidMaxOutputTokens)?);
    }
    if let Some(value) = fields.get("timeout_ms") {
        let timeout = safe_integer(value)
            .filter(|ms| *ms != 0 && *ms <= MAX_TIMEOUT_MS)
            .ok_or(CodemodeSourceError::InvalidTimeoutMs)?;
        options.timeout_ms = Some(timeout);
    }
    Ok(options)
}

/// Split an optional first-line `// @options: {...}` from the script (`source.ts:100-115`).
///
/// Only the first line is ever an options line. When it is one, it is replaced by an empty line
/// rather than removed, so a stack trace's line numbers still match the model's input.
///
/// # Errors
///
/// [`CodemodeSourceError`] for empty input and for an options line that is invalid or has no code
/// after it.
pub fn parse_codemode_source(input: &str) -> Result<ParsedCodemodeSource, CodemodeSourceError> {
    if js_trim(input).is_empty() {
        return Err(CodemodeSourceError::EmptySource);
    }
    let newline = input.find('\n');
    let first_line = newline.map_or(input, |at| input.get(..at).unwrap_or(input));
    let first_line = first_line.strip_suffix('\r').unwrap_or(first_line);
    let trimmed = js_trim_start(first_line);
    let Some(directive) = trimmed.strip_prefix(CODEMODE_OPTIONS_PREFIX) else {
        return Ok(ParsedCodemodeSource {
            code: input.to_owned(),
            options: CodemodeSourceOptions::default(),
        });
    };
    let code = newline.map_or("", |at| input.get(at..).unwrap_or(""));
    if js_trim(code).is_empty() {
        return Err(CodemodeSourceError::OptionsWithoutCode);
    }
    let options = parse_options(js_trim(directive))?;
    Ok(ParsedCodemodeSource {
        code: code.to_owned(),
        options,
    })
}

#[cfg(test)]
mod tests;
