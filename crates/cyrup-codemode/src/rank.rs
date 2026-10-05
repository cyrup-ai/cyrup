//! Tool discovery: a BM25 ranker over tool metadata (pi
//! `packages/coding-agent/src/extensions/tool-search/tool.ts` @v1.0.1, CODE-010).
//!
//! Shared by `searchTools()` in `codemode` scripts ([`crate::discovery::search_tools`]) and the
//! `tool_search` tool, which each build [`ToolSearchDocument`]s with
//! [`create_tool_search_document`] and rank them with [`Bm25Ranker`]. Scoring is bit-for-bit
//! upstream's: same tokeniser, same `k1`/`b`, same summation order, and `ln` taken from [`math_log`], the
//! fdlibm port that V8's `Math.log` is, so scores are the doubles pi computes.

use std::collections::{HashMap, HashSet};

use cyrup_core::ToolNamespace;
use serde_json::{Map, Value};

use crate::js::{js_trim, math_log, own_keys};

/// `DEFAULT_TOOL_SEARCH_LIMIT` (`tool-search/tool.ts:21`).
pub const DEFAULT_TOOL_SEARCH_LIMIT: usize = 8;

/// A tool as the ranker sees it: its name and the text built by [`create_tool_search_document`]
/// (`tool.ts:24`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSearchDocument {
    pub name: String,
    pub text: String,
}

/// One ranked hit (`tool.ts:29`).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSearchMatch {
    pub name: String,
    pub score: f64,
}

/// Ranks tools for a query. BM25 today; a hybrid ranker with embeddings can replace it
/// (`tool.ts:35`).
pub trait ToolRanker {
    fn rank(
        &self,
        query: &str,
        documents: &[ToolSearchDocument],
        limit: usize,
    ) -> Vec<ToolSearchMatch>;
}

/// `STOP_WORDS` (`tool.ts:39-61`), sorted.
const STOP_WORDS: [&str; 21] = [
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "is", "it", "of", "on",
    "or", "that", "the", "this", "to", "with",
];

/// Naive singular form, so `issues` matches `issue` and `searches` matches `search`
/// (`tool.ts:64-69`). Terms are `[a-z0-9]+`, so byte length is character length.
fn stem(term: &str) -> String {
    let len = term.len();
    if len > 4 {
        if let Some(base) = term.strip_suffix("ies") {
            return format!("{base}y");
        }
        if ["ches", "shes", "sses", "xes", "zes"]
            .iter()
            .any(|suffix| term.ends_with(suffix))
        {
            return term.get(..len - 2).unwrap_or(term).to_owned();
        }
    }
    if len > 3 && term.ends_with('s') && !term.ends_with("ss") {
        return term.get(..len - 1).unwrap_or(term).to_owned();
    }
    term.to_owned()
}

/// `text.replace(/([a-z0-9])([A-Z])/g, "$1 $2")`: a space between a lowercase letter or digit and
/// an uppercase letter that follows it. Matches do not overlap, as in a global `replace`.
fn split_lower_upper(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    let mut index = 0;
    while let Some(&current) = chars.get(index) {
        let next = chars.get(index + 1).copied();
        if (current.is_ascii_lowercase() || current.is_ascii_digit())
            && next.is_some_and(|c| c.is_ascii_uppercase())
        {
            out.push(current);
            out.push(' ');
            index += 1;
            out.extend(next);
            index += 1;
        } else {
            out.push(current);
            index += 1;
        }
    }
    out
}

/// `text.replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2")`: inside a run of two or more capitals that is
/// followed by a lowercase letter, a space before the last capital (`HTTPServer` -> `HTTP Server`).
/// A run of capitals can only match at its start, and a match consumes through the lowercase
/// letter.
fn split_acronym(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    let mut index = 0;
    while let Some(&current) = chars.get(index) {
        if !current.is_ascii_uppercase() {
            out.push(current);
            index += 1;
            continue;
        }
        let mut end = index;
        while chars.get(end).is_some_and(char::is_ascii_uppercase) {
            end += 1;
        }
        let followed_by_lower = chars.get(end).is_some_and(char::is_ascii_lowercase);
        if end - index >= 2 && followed_by_lower {
            out.extend(chars.get(index..end - 1).unwrap_or_default());
            out.push(' ');
            out.extend(chars.get(end - 1).copied());
            out.extend(chars.get(end).copied());
            index = end + 1;
        } else {
            out.extend(chars.get(index..end).unwrap_or_default());
            index = end;
        }
    }
    out
}

/// Lowercase terms, split at camelCase boundaries and non-alphanumerics, without stop words
/// (`tool.ts:72-80`).
#[must_use]
pub fn tokenize(text: &str) -> Vec<String> {
    split_acronym(&split_lower_upper(text))
        .to_lowercase()
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
        .filter(|term| !term.is_empty() && !STOP_WORDS.contains(term))
        .map(stem)
        .collect()
}

/// Schema descriptions and property names, recursively (`tool.ts:87-101`). Property names come in
/// `Object.keys` order; items that are arrays (tuples) are not followed, as upstream.
fn schema_text(schema: &Value, parts: &mut Vec<String>) {
    let Value::Object(map) = schema else {
        return;
    };
    if let Some(Value::String(description)) = map.get("description") {
        parts.push(description.clone());
    }
    if let Some(Value::Object(properties)) = map.get("properties") {
        push_properties(properties, parts);
    }
    if let Some(items) = map.get("items") {
        schema_text(items, parts);
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(variants)) = map.get(key) {
            for variant in variants {
                schema_text(variant, parts);
            }
        }
    }
}

fn push_properties(properties: &Map<String, Value>, parts: &mut Vec<String>) {
    for name in own_keys(properties) {
        parts.push(name.to_owned());
        if let Some(property) = properties.get(name) {
            schema_text(property, parts);
        }
    }
}

/// Search text of a tool: the name, the name with `_` as spaces, the description, schema
/// descriptions and property names, and the namespace with its description and instructions
/// (`tool.ts:108-116`).
#[must_use]
pub fn create_tool_search_document(
    name: &str,
    description: &str,
    parameters: &Value,
    namespace: Option<&ToolNamespace>,
) -> ToolSearchDocument {
    let mut parts: Vec<String> = vec![
        name.to_owned(),
        name.replace('_', " "),
        description.to_owned(),
    ];
    schema_text(parameters, &mut parts);
    if let Some(namespace) = namespace {
        parts.push(namespace.name.clone());
        parts.push(namespace.description.clone().unwrap_or_default());
        parts.push(namespace.instructions.clone().unwrap_or_default());
    }
    let kept: Vec<&str> = parts
        .iter()
        .map(String::as_str)
        .filter(|part| !js_trim(part).is_empty())
        .collect();
    ToolSearchDocument {
        name: name.to_owned(),
        text: kept.join(" "),
    }
}

/// Okapi BM25 with the usual parameters. Ties keep document order (`tool.ts:119-157`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bm25Ranker {
    k1: f64,
    b: f64,
}

impl Bm25Ranker {
    /// `k1 = 1.2`, `b = 0.75` (`tool.ts:124-125`).
    pub const DEFAULT_K1: f64 = 1.2;
    pub const DEFAULT_B: f64 = 0.75;

    #[must_use]
    pub fn new(k1: f64, b: f64) -> Self {
        Self { k1, b }
    }
}

impl Default for Bm25Ranker {
    fn default() -> Self {
        Self::new(Self::DEFAULT_K1, Self::DEFAULT_B)
    }
}

impl ToolRanker for Bm25Ranker {
    fn rank(
        &self,
        query: &str,
        documents: &[ToolSearchDocument],
        limit: usize,
    ) -> Vec<ToolSearchMatch> {
        let mut seen: HashSet<String> = HashSet::new();
        let query_terms: Vec<String> = tokenize(query)
            .into_iter()
            .filter(|term| seen.insert(term.clone()))
            .collect();
        if query_terms.is_empty() || documents.is_empty() || limit == 0 {
            return Vec::new();
        }
        let tokenized: Vec<Vec<String>> = documents
            .iter()
            .map(|document| tokenize(&document.text))
            .collect();
        let term_counts: Vec<HashMap<&str, usize>> = tokenized
            .iter()
            .map(|terms| {
                let mut counts: HashMap<&str, usize> = HashMap::new();
                for term in terms {
                    *counts.entry(term.as_str()).or_insert(0) += 1;
                }
                counts
            })
            .collect();
        // A document's length is the sum of its term counts, which is its token count.
        let lengths: Vec<f64> = tokenized.iter().map(|terms| terms.len() as f64).collect();
        let document_count = documents.len() as f64;
        let mean = lengths.iter().sum::<f64>() / document_count;
        // `|| 1`: a zero (or NaN) average falls back to 1.
        let average_length = if mean == 0.0 || mean.is_nan() {
            1.0
        } else {
            mean
        };
        let idf: Vec<f64> = query_terms
            .iter()
            .map(|term| {
                let frequency = term_counts
                    .iter()
                    .filter(|counts| counts.contains_key(term.as_str()))
                    .count() as f64;
                math_log(1.0 + (document_count - frequency + 0.5) / (frequency + 0.5))
            })
            .collect();
        let mut matches: Vec<ToolSearchMatch> = Vec::new();
        for (index, document) in documents.iter().enumerate() {
            let Some(counts) = term_counts.get(index) else {
                continue;
            };
            let length = lengths.get(index).copied().unwrap_or(0.0);
            let mut score = 0.0_f64;
            for (term, idf) in query_terms.iter().zip(&idf) {
                let count = counts.get(term.as_str()).copied().unwrap_or(0);
                if count == 0 {
                    continue;
                }
                let count = count as f64;
                let norm = self.k1 * (1.0 - self.b + (self.b * length) / average_length);
                score += idf * ((count * (self.k1 + 1.0)) / (count + norm));
            }
            if score > 0.0 {
                matches.push(ToolSearchMatch {
                    name: document.name.clone(),
                    score,
                });
            }
        }
        // `Array.prototype.sort` is stable, so equal scores keep document order.
        matches.sort_by(|a, b| b.score.total_cmp(&a.score));
        matches.truncate(limit);
        matches
    }
}

#[cfg(test)]
mod tests;
