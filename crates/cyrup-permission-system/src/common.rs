//! Path normalization, frontmatter extraction, the minimal YAML-map parser, and the small
//! `Value`-shaped helpers (port of pi `common.ts`). Pure; host-independent.

use serde_json::{Map, Value};

use crate::ordered::OrderedValue;

/// pi `common.ts:6-12` `toRecord` — an object's map, or an empty map for non-objects/arrays/null.
#[must_use]
pub fn to_record(value: &Value) -> &Map<String, Value> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    match value.as_object() {
        Some(map) => map,
        None => EMPTY.get_or_init(Map::new),
    }
}

/// pi `common.ts:14-21` `getNonEmptyString` — a trimmed non-empty string, else `None`.
#[must_use]
pub fn get_non_empty_string(value: Option<&Value>) -> Option<String> {
    let s = value?.as_str()?;
    let trimmed = s.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

/// The user's home directory (pi `os.homedir()`), from `$HOME`/`$USERPROFILE`. Degrades to `""`
/// (never panics) when unset, which collapses `~` expansion to a relative resolve — matching pi's
/// behavior no worse.
fn home_dir() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default()
}

/// Lexically collapse `.`/`..` segments in a `/`-separated path WITHOUT touching the filesystem
/// (node `path.normalize`, which never `stat`s — important: the permission subject is often a path
/// that does not exist yet, so `std::fs::canonicalize` is wrong here). An absolute input keeps its
/// leading `/`; a `..` above an absolute root is dropped (node semantics).
pub(crate) fn lexical_normalize(input: &str) -> String {
    let is_abs = input.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in input.split('/') {
        match seg {
            "" | "." => {}
            ".." => match out.last() {
                Some(&last) if last != ".." => {
                    out.pop();
                }
                Some(_) => out.push(".."),
                None => {
                    if !is_abs {
                        out.push("..");
                    }
                }
            },
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    if is_abs {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

/// pi `common.ts:27-44` `normalizePathForComparison`: strip surrounding quotes, drop a leading `@`,
/// expand `~`, resolve against `cwd`, lexically collapse `..`. Returns a `/`-separated absolute path
/// (non-win32; cyrup targets unix — win32 lowercasing is intentionally omitted, matching the doc's
/// unix scope).
#[must_use]
pub fn normalize_path_for_comparison(path_value: &str, cwd: &str) -> String {
    let trimmed = strip_surrounding_quotes(path_value.trim());
    if trimmed.is_empty() {
        return String::new();
    }

    let normalized_path = trimmed.strip_prefix('@').unwrap_or(trimmed);

    // `~` / `~/` expansion (pi `common.ts:35-39`).
    let expanded: String = if normalized_path == "~" {
        home_dir()
    } else if let Some(rest) = normalized_path.strip_prefix("~/") {
        join_paths(&home_dir(), rest)
    } else {
        normalized_path.to_string()
    };

    // `resolve(cwd, path)` — absolute stays; relative joins under cwd (pi `common.ts:41`).
    let absolute = if expanded.starts_with('/') {
        expanded
    } else {
        join_paths(cwd, &expanded)
    };

    lexical_normalize(&absolute)
}

/// pi `common.ts:46-64` `normalizePathResourceForPermission`: normalize for comparison, force `/`
/// separators, collapse a bare root to `/`, then strip a trailing slash.
#[must_use]
pub fn normalize_path_resource_for_permission(path_value: &str, cwd: &str) -> String {
    let normalized = normalize_path_for_comparison(path_value, cwd).replace('\\', "/");
    if normalized.is_empty() {
        return String::new();
    }
    if normalized.chars().all(|c| c == '/') {
        return "/".to_string();
    }
    normalized.trim_end_matches('/').to_string()
}

/// pi `common.ts:66-77` `isPathWithinDirectory`: `path == dir`, or `path` starts with `dir/`.
#[must_use]
pub fn is_path_within_directory(path_value: &str, directory: &str) -> bool {
    if path_value.is_empty() || directory.is_empty() {
        return false;
    }
    if path_value == directory {
        return true;
    }
    let prefix =
        if directory.ends_with('/') { directory.to_string() } else { format!("{directory}/") };
    path_value.starts_with(&prefix)
}

/// pi `common.ts:125-137` `extractFrontmatter`: the YAML block between a leading `---\n` and the
/// next `\n---`, else `""`.
#[must_use]
pub fn extract_frontmatter(markdown: &str) -> String {
    let normalized = markdown.replace("\r\n", "\n");
    if !normalized.starts_with("---\n") {
        return String::new();
    }
    // Find the next `\n---` at or after index 4 (pi `indexOf("\n---", 4)`).
    match normalized.get(4..).and_then(|rest| rest.find("\n---")) {
        Some(rel) => normalized.get(4..4 + rel).unwrap_or("").to_string(),
        None => String::new(),
    }
}

/// pi `common.ts:81-123` `parseSimpleYamlMap`: an indentation-nested map of scalar strings and
/// nested maps. Faithful to pi's minimal parser — NOT a general YAML parser (no lists, no
/// multi-line scalars); the permission frontmatter never uses those. Returns an [`OrderedValue`] so
/// pattern-key insertion order is preserved (the engine's last-match-wins depends on it).
#[must_use]
pub fn parse_simple_yaml_map(input: &str) -> OrderedValue {
    let mut root: Vec<(String, OrderedValue)> = Vec::new();
    // Stack of (indent, path-into-root). We track the path as a Vec<String> of keys because Rust
    // cannot hold aliasing mutable references into a nested map like the JS `stack` of object refs.
    let mut stack: Vec<(i64, Vec<String>)> = vec![(-1, Vec::new())];

    for raw_line in input.split('\n') {
        let trimmed_full = raw_line.trim();
        if trimmed_full.is_empty() || trimmed_full.starts_with('#') {
            continue;
        }
        let indent = (raw_line.len() - raw_line.trim_start().len()) as i64;
        let line = trimmed_full;
        let Some(sep) = line.find(':') else { continue };
        if sep == 0 {
            continue;
        }
        let key = strip_surrounding_quotes(line.get(..sep).unwrap_or("").trim()).to_string();
        let raw_value = line.get(sep + 1..).unwrap_or("").trim().to_string();

        while stack.len() > 1 && stack.last().map(|(i, _)| indent <= *i).unwrap_or(false) {
            stack.pop();
        }
        let parent_path = stack.last().map(|(_, p)| p.clone()).unwrap_or_default();

        if raw_value.is_empty() {
            // Open a nested map at parent_path + key.
            let mut child_path = parent_path.clone();
            child_path.push(key.clone());
            insert_ordered_at_path(&mut root, &parent_path, &key, OrderedValue::empty_object());
            stack.push((indent, child_path));
        } else {
            let scalar = strip_surrounding_quotes(&raw_value).to_string();
            insert_ordered_at_path(&mut root, &parent_path, &key, OrderedValue::Str(scalar));
        }
    }

    OrderedValue::Object(root)
}

/// Insert `value` at `root[path...][key]`, creating intermediate objects as needed. A path segment
/// whose current value is not an object is overwritten with a fresh object (degrade, never panic).
/// A repeated `key` overwrites in place (JS object-key semantics), preserving first-seen position.
fn insert_ordered_at_path(
    root: &mut Vec<(String, OrderedValue)>,
    path: &[String],
    key: &str,
    value: OrderedValue,
) {
    let mut cursor = root;
    for seg in path {
        let idx = match cursor.iter().position(|(k, _)| k == seg) {
            Some(i) => i,
            None => {
                cursor.push((seg.clone(), OrderedValue::empty_object()));
                cursor.len() - 1
            }
        };
        let entry = match cursor.get_mut(idx) {
            Some(e) => e,
            None => return,
        };
        if !matches!(entry.1, OrderedValue::Object(_)) {
            entry.1 = OrderedValue::empty_object();
        }
        cursor = match &mut entry.1 {
            OrderedValue::Object(next) => next,
            _ => return,
        };
    }
    if let Some(slot) = cursor.iter_mut().find(|(k, _)| k == key) {
        slot.1 = value;
    } else {
        cursor.push((key.to_string(), value));
    }
}

/// Strip a single pair of matching surrounding single/double quotes (pi
/// `.replace(/^["']|["']$/g, "")` / the YAML scalar unquote).
fn strip_surrounding_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes.first().copied();
        let last = bytes.last().copied();
        if (first == Some(b'"') && last == Some(b'"'))
            || (first == Some(b'\'') && last == Some(b'\''))
        {
            return value.get(1..value.len() - 1).unwrap_or("");
        }
    }
    value
}

/// Join two path fragments with a single `/` (node `path.join`, minimal: no `..` collapsing here —
/// [`lexical_normalize`] does that later).
pub(crate) fn join_paths(base: &str, rest: &str) -> String {
    if base.is_empty() {
        return rest.to_string();
    }
    let base = base.trim_end_matches('/');
    let rest = rest.trim_start_matches('/');
    if rest.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{rest}")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn resource_collapses_dotdot_lexically_without_fs() {
        // `/dir/../etc/passwd` → `/etc/passwd` (canonicalize would fail: path does not exist).
        let r = normalize_path_resource_for_permission("/dir/../etc/passwd", "/work");
        assert_eq!(r, "/etc/passwd");
    }

    #[test]
    fn resource_resolves_relative_under_cwd_and_strips_trailing_slash() {
        let r = normalize_path_resource_for_permission("sub/dir/", "/work");
        assert_eq!(r, "/work/sub/dir");
    }

    #[test]
    fn resource_strips_at_and_quotes() {
        let r = normalize_path_resource_for_permission("\"@/abs/path\"", "/work");
        assert_eq!(r, "/abs/path");
    }

    #[test]
    fn sibling_dir_is_not_within() {
        assert!(!is_path_within_directory("/safe-evil/x", "/safe"));
        assert!(is_path_within_directory("/safe/x", "/safe"));
    }

    #[test]
    fn frontmatter_and_yaml_parse_nested_permission_in_order() {
        let md = "---\npermission:\n  bash:\n    \"git *\": allow\n    \"*\": ask\n---\nbody";
        let fm = extract_frontmatter(md);
        assert!(fm.contains("permission"));
        let parsed = parse_simple_yaml_map(&fm);
        let bash = parsed.get("permission").unwrap().get("bash").unwrap();
        let entries = bash.as_object().unwrap();
        // Insertion order preserved: "git *" before "*".
        assert_eq!(entries[0].0, "git *");
        assert_eq!(entries[0].1.as_str(), Some("allow"));
        assert_eq!(entries[1].0, "*");
        assert_eq!(entries[1].1.as_str(), Some("ask"));
    }
}
