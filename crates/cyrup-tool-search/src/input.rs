//! The `tool_search` arguments: the schema the model sees and the validation `execute` applies to
//! what arrives (`extensions/tool-search/tool.ts:159-163,234-236` @v1.0.1).
//!
//! The arguments are parsed once into [`ToolSearchInput`]; a value that violates a rule is a
//! [`ToolSearchRefusal`], never a half-valid input.

use std::num::NonZeroUsize;
use std::sync::LazyLock;

use cyrup_codemode::js::js_trim;
use cyrup_codemode::rank::DEFAULT_TOOL_SEARCH_LIMIT;
use serde_json::{Value, json};

/// `toolSearchSchema` (`tool.ts:159-164`): `query` (required) and `limit?`, whose description
/// interpolates the default.
#[must_use]
pub fn tool_search_schema() -> &'static Value {
    static SCHEMA: LazyLock<Value> = LazyLock::new(|| {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query for deferred tools." },
                "limit": {
                    "type": "number",
                    "description": format!(
                        "Maximum number of tools to return. Defaults to {DEFAULT_TOOL_SEARCH_LIMIT}."
                    )
                }
            },
            "required": ["query"]
        })
    });
    &SCHEMA
}

/// A refusal of `tool_search`'s arguments. `Display` is the text upstream throws (`tool.ts:234-236`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ToolSearchRefusal {
    /// `query` is absent or not a string. Upstream's argument validation rejects this before
    /// `execute` runs, so its text is the pipeline's; this text is for a caller that reaches
    /// `execute` without that validation.
    #[error("tool_search expects {{ query: string }}")]
    QueryNotAString,
    /// `query.trim() === ""`.
    #[error("query must not be empty")]
    EmptyQuery,
    /// `limit` is not a positive integer (`!Number.isInteger(max) || max <= 0`).
    #[error("limit must be a positive integer")]
    LimitNotAPositiveInteger,
}

/// How many tools one search may load. A positive integer, so a zero limit cannot be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchLimit(NonZeroUsize);

impl SearchLimit {
    /// `DEFAULT_TOOL_SEARCH_LIMIT` (`tool.ts:21`).
    #[must_use]
    pub fn default_limit() -> Self {
        Self(NonZeroUsize::MIN.saturating_add(DEFAULT_TOOL_SEARCH_LIMIT - 1))
    }

    #[must_use]
    pub fn get(self) -> usize {
        self.0.get()
    }

    /// `Number.isInteger(max) && max > 0` over a JSON value; a number beyond `usize` saturates.
    fn from_json(value: &Value) -> Result<Self, ToolSearchRefusal> {
        let number = value
            .as_f64()
            .ok_or(ToolSearchRefusal::LimitNotAPositiveInteger)?;
        if number.fract() != 0.0 || number <= 0.0 {
            return Err(ToolSearchRefusal::LimitNotAPositiveInteger);
        }
        // Saturating float-to-integer conversion; `number >= 1`, so the result is non-zero.
        NonZeroUsize::new(number as usize)
            .map(Self)
            .ok_or(ToolSearchRefusal::LimitNotAPositiveInteger)
    }
}

/// Valid `tool_search` arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSearchInput {
    /// Not empty after JavaScript's `trim`. Kept as the model wrote it.
    pub query: String,
    pub limit: SearchLimit,
}

impl ToolSearchInput {
    /// Validates `params` in upstream's order: the query first, then the limit. An absent or `null`
    /// `limit` is the default (`limit ?? DEFAULT_TOOL_SEARCH_LIMIT`).
    ///
    /// # Errors
    ///
    /// The [`ToolSearchRefusal`] naming the first rule `params` breaks.
    pub fn parse(params: &Value) -> Result<Self, ToolSearchRefusal> {
        let query = params
            .get("query")
            .and_then(Value::as_str)
            .ok_or(ToolSearchRefusal::QueryNotAString)?;
        if js_trim(query).is_empty() {
            return Err(ToolSearchRefusal::EmptyQuery);
        }
        let limit = match params.get("limit").filter(|value| !value.is_null()) {
            None => SearchLimit::default_limit(),
            Some(value) => SearchLimit::from_json(value)?,
        };
        Ok(Self {
            query: query.to_owned(),
            limit,
        })
    }
}
