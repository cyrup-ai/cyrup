//! Prompt templates — markdown expanded by `/name` with `{{placeholder}}` substitution
//! (arch-09 §3.4, R-09-007..010).

use std::path::{Path, PathBuf};

use crate::discovery::Named;
use crate::error::ResourceError;
use crate::key::ResourceKey;
use crate::scope::{ResourceOrigin, ResourceScope};

/// A markdown prompt template. Bodies are small and eagerly cached (R-09-025).
#[derive(Clone, Debug)]
pub struct PromptTemplate {
    /// Expanded by `/name` (R-09-007).
    pub key: ResourceKey,
    pub path: PathBuf,
    pub body: String,
    /// Discovered `{{names}}` (for argument prompting).
    pub placeholders: Vec<String>,
    pub scope: ResourceScope,
    pub origin: ResourceOrigin,
}

/// Arguments supplied at expansion time, keyed by placeholder name.
#[derive(Default, Debug, Clone)]
pub struct PlaceholderArgs(std::collections::BTreeMap<String, String>);

impl PlaceholderArgs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.0.insert(name.into(), value.into());
        self
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for PlaceholderArgs {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self(iter.into_iter().map(|(k, v)| (k.into(), v.into())).collect())
    }
}

/// The result of expanding a template (R-09-009). Unknown placeholders are left literal and
/// reported in `unresolved` — deterministic, no panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    pub unresolved: Vec<String>,
}

impl PromptTemplate {
    /// Load a template from a markdown file. `key` defaults to the file stem.
    pub fn load(
        path: &Path,
        scope: ResourceScope,
        origin: ResourceOrigin,
    ) -> Result<PromptTemplate, ResourceError> {
        let body = std::fs::read_to_string(path)?;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let key = ResourceKey::normalize(stem);
        if key.is_empty() {
            return Err(ResourceError::Manifest(format!(
                "prompt template has no usable name: {}",
                path.display()
            )));
        }
        let placeholders = scan_placeholders(&body);
        Ok(PromptTemplate { key, path: path.to_path_buf(), body, placeholders, scope, origin })
    }

    /// Expand at input-pipeline time (R-09-009). Single linear scan, no regex.
    pub fn expand(&self, args: &PlaceholderArgs) -> Expansion {
        expand_str(&self.body, args)
    }
}

impl Named for PromptTemplate {
    fn key(&self) -> &ResourceKey {
        &self.key
    }
    fn scope(&self) -> ResourceScope {
        self.scope
    }
}

/// Discover every `{{name}}` placeholder, de-duplicated, in first-seen order.
pub(crate) fn scan_placeholders(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (name, _, _) in placeholder_iter(body) {
        if !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// Linear-scan expansion. Unknown placeholders are preserved literally and recorded.
fn expand_str(body: &str, args: &PlaceholderArgs) -> Expansion {
    let mut text = String::with_capacity(body.len());
    let mut unresolved: Vec<String> = Vec::new();
    let mut cursor = 0usize;
    for (name, start, end) in placeholder_iter(body) {
        // Copy the literal segment before this placeholder.
        if let Some(seg) = body.get(cursor..start) {
            text.push_str(seg);
        }
        match args.get(name) {
            Some(val) => text.push_str(val),
            None => {
                if let Some(raw) = body.get(start..end) {
                    text.push_str(raw);
                }
                if !unresolved.iter().any(|n| n == name) {
                    unresolved.push(name.to_string());
                }
            }
        }
        cursor = end;
    }
    if let Some(rest) = body.get(cursor..) {
        text.push_str(rest);
    }
    Expansion { text, unresolved }
}

/// Iterate `{{name}}` occurrences, yielding `(name, start_byte, end_byte)` of the whole `{{…}}`
/// span. Names are trimmed; empty or whitespace-only braces are ignored.
fn placeholder_iter(body: &str) -> impl Iterator<Item = (&str, usize, usize)> {
    let bytes = body.as_bytes();
    let mut i = 0usize;
    std::iter::from_fn(move || {
        while i + 1 < bytes.len() {
            if bytes.get(i) == Some(&b'{') && bytes.get(i + 1) == Some(&b'{') {
                let inner_start = i + 2;
                // Find closing `}}`.
                let mut j = inner_start;
                let mut found = None;
                while j + 1 < bytes.len() {
                    if bytes.get(j) == Some(&b'}') && bytes.get(j + 1) == Some(&b'}') {
                        found = Some(j);
                        break;
                    }
                    j += 1;
                }
                match found {
                    Some(close) => {
                        let end = close + 2;
                        let inner = body.get(inner_start..close).unwrap_or("").trim();
                        let start = i;
                        i = end;
                        if !inner.is_empty() && inner.chars().all(is_placeholder_char) {
                            // Re-locate the trimmed name span is unnecessary; expose trimmed name.
                            return Some((inner, start, end));
                        }
                        // Not a valid placeholder; continue scanning past it.
                        continue;
                    }
                    None => {
                        i = bytes.len();
                        return None;
                    }
                }
            }
            i += 1;
        }
        None
    })
}

fn is_placeholder_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
}
