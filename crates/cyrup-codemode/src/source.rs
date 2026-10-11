//! The `codemode` source format: JavaScript, optionally preceded by one options line (pi
//! `packages/codemode/src/source.ts` @v1.0.4, CODE-003).
//!
//! ```js
//! // @options: {"max_output_tokens": 2000, "timeout_ms": 30000}
//! const pkg = await tools.read({ path: "package.json" });
//! text(JSON.parse(pkg).name);
//! ```
//!
//! # Production call path
//!
//! * [`parse_codemode_source`] is the first step of a `codemode` tool call: it splits the model's
//!   `code` argument into the script and its [`CodemodeSourceOptions`] (pi `execute.ts:329`). The
//!   options carry `max_output_tokens`, which feeds [`crate::output::truncate_output`], and
//!   `timeout_ms`, which feeds the sandbox's deadline. The options line is the first non-blank line
//!   (one surrounding markdown fence is stripped first), and a later one is an error: both are
//!   [CYRUP-DELTA]s from upstream, which reads the very first line only.
//! * [`CODEMODE_SOURCE_GRAMMAR`] is what the `codemode` tool declares as its grammar-constrained
//!   input on providers that support it: `cyrup_core::constrained_sampling::GrammarVariants`'
//!   `openai_lark` field takes it as is (gap-analysis CODE-003).

use serde_json::Value;

use crate::js::{js_trim, js_trim_start, own_keys};

/// The first non-blank line, when it starts with this (after leading whitespace), is an options
/// line (`source.ts:11`).
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
    /// [CYRUP-DELTA] An options line after the first non-blank line. Upstream reads it as an
    /// ordinary comment, so its `timeout_ms` and `max_output_tokens` are dropped without a word and
    /// the script runs under limits the model did not ask for.
    #[error(
        "The `// @options:` line must be the first non-blank line of the script, but one is on line {line}. Move it to the top or delete it."
    )]
    LateOptions { line: usize },
    /// [CYRUP-DELTA] A markdown fence that opens and never closes.
    #[error(
        "The input starts with a markdown code fence that is never closed. Provide raw JavaScript only (no code fence)."
    )]
    UnclosedFence,
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

/// A markdown code fence.
const FENCE: &str = "```";

/// [CYRUP-DELTA] Strips exactly one surrounding markdown code fence (an opening line of three
/// backticks and an optional language word, a closing line of three backticks), which models wrap
/// scripts in although the description says not to. Without this the fence reaches the engine, which
/// reads "```js" as a tagged template and fails with `TypeError: "" is not a function`, far from the
/// cause. The opening fence line becomes an empty line, so line numbers are those of the input.
/// Input that does not start with a fence is returned as is.
fn strip_fence(input: &str) -> Result<std::borrow::Cow<'_, str>, CodemodeSourceError> {
    let body_start = input.len() - js_trim_start(input).len();
    let Some(rest) = input
        .get(body_start..)
        .and_then(|rest| rest.strip_prefix(FENCE))
    else {
        return Ok(std::borrow::Cow::Borrowed(input));
    };
    let (info, after) = rest.split_once('\n').unwrap_or((rest, ""));
    if info.contains('`') {
        // "```js text```" and the like are not a fence line.
        return Ok(std::borrow::Cow::Borrowed(input));
    }
    let Some(inner) = after.trim_end().strip_suffix(FENCE) else {
        return Err(CodemodeSourceError::UnclosedFence);
    };
    // The closing fence is a line of its own.
    if !(inner.is_empty() || inner.ends_with('\n')) {
        return Err(CodemodeSourceError::UnclosedFence);
    }
    let lead = input.get(..body_start).unwrap_or("");
    Ok(std::borrow::Cow::Owned(format!("{lead}\n{inner}")))
}

/// Split an optional `// @options: {...}` line from the script (`source.ts:100-115`).
///
/// [CYRUP-DELTA] Upstream reads the options from the very first line only, so a leading blank line
/// (or a fence the model wrapped the script in) silently drops `timeout_ms`. Here the options line
/// is the first non-blank line, one markdown fence around the whole script is stripped first (see
/// [`strip_fence`]), and a `// @options:` line anywhere later is an error rather than a comment.
/// When the options line is found it is replaced by an empty line rather than removed, so a stack
/// trace's line numbers still match the model's input.
///
/// # Errors
///
/// [`CodemodeSourceError`] for empty input, an unclosed fence, an options line that is invalid, has
/// no code after it, or is not the first non-blank line.
pub fn parse_codemode_source(input: &str) -> Result<ParsedCodemodeSource, CodemodeSourceError> {
    if js_trim(input).is_empty() {
        return Err(CodemodeSourceError::EmptySource);
    }
    let input = strip_fence(input)?;
    let input: &str = &input;
    // Byte offset and text of every line, without the `\r` of a CRLF.
    let mut lines = Vec::new();
    let mut offset = 0;
    for line in input.split('\n') {
        lines.push((offset, line.strip_suffix('\r').unwrap_or(line)));
        offset += line.len() + 1;
    }
    let first = lines.iter().position(|(_, line)| !js_trim(line).is_empty());
    let late_from = match first {
        Some(index) => index + 1,
        None => lines.len(),
    };
    let options_line = first.and_then(|index| {
        let (start, line) = *lines.get(index)?;
        let directive = js_trim_start(line).strip_prefix(CODEMODE_OPTIONS_PREFIX)?;
        Some((start, line.len(), directive))
    });
    if let Some((index, _)) = lines
        .iter()
        .enumerate()
        .skip(late_from)
        .find(|(_, (_, line))| js_trim_start(line).starts_with(CODEMODE_OPTIONS_PREFIX))
    {
        return Err(CodemodeSourceError::LateOptions { line: index + 1 });
    }
    let Some((start, _, directive)) = options_line else {
        return Ok(ParsedCodemodeSource {
            code: input.to_owned(),
            options: CodemodeSourceOptions::default(),
        });
    };
    let newline = input.get(start..).and_then(|rest| rest.find('\n'));
    let code_after = newline.map_or("", |at| input.get(start + at..).unwrap_or(""));
    if js_trim(code_after).is_empty() {
        return Err(CodemodeSourceError::OptionsWithoutCode);
    }
    let options = parse_options(js_trim(directive))?;
    let before = input.get(..start).unwrap_or("");
    Ok(ParsedCodemodeSource {
        code: format!("{before}{code_after}"),
        options,
    })
}

#[cfg(test)]
mod tests;
