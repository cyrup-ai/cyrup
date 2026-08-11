//! Path normalization, frontmatter extraction, the minimal YAML-map parser, and the small
//! `Value`-shaped helpers (port of pi `common.ts`). Pure; host-independent.

use serde_json::{Map, Value};

use crate::ordered::OrderedValue;

/// pi `common.ts:7-13` `toRecord` — an object's map, or an empty map for non-objects/arrays/null.
#[must_use]
pub fn to_record(value: &Value) -> &Map<String, Value> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    match value.as_object() {
        Some(map) => map,
        None => EMPTY.get_or_init(Map::new),
    }
}

/// pi `common.ts:15-22` `getNonEmptyString` — a trimmed non-empty string, else `None`.
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

/// pi `common.ts:57-74` `normalizePathForComparison`: strip surrounding quotes, drop a leading `@`,
/// expand `~`, resolve against `cwd`, lexically collapse `..`. Returns a `/`-separated absolute path
/// (non-win32; cyrup targets unix — win32 lowercasing is intentionally omitted, matching the doc's
/// unix scope).
#[must_use]
pub fn normalize_path_for_comparison(path_value: &str, cwd: &str) -> String {
    let trimmed = strip_edge_quotes(path_value.trim());
    if trimmed.is_empty() {
        return String::new();
    }

    let normalized_path = trimmed.strip_prefix('@').unwrap_or(trimmed);

    // `~` / `~/` expansion (pi `common.ts:65-69`).
    let expanded: String = if normalized_path == "~" {
        home_dir()
    } else if let Some(rest) = normalized_path.strip_prefix("~/") {
        join_paths(&home_dir(), rest)
    } else {
        normalized_path.to_string()
    };

    // `resolve(cwd, path)` — absolute stays; relative joins under cwd (pi `common.ts:71`).
    let absolute = if expanded.starts_with('/') {
        expanded
    } else {
        join_paths(cwd, &expanded)
    };

    lexical_normalize(&absolute)
}

/// pi `common.ts:76-94` `normalizePathResourceForPermission`: normalize for comparison, force `/`
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

/// pi `common.ts:96-107` `isPathWithinDirectory`: `path == dir`, or `path` starts with `dir/`.
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

/// pi `common.ts:167-179` `extractFrontmatter`: the YAML block between a leading `---\n` and the
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

/// pi `PERMISSION_SYSTEM_COMMAND_DESCRIPTION` (v0.8.0 `common.ts:181`), verbatim but for the
/// rebrand: the description the `/permission-system` slash command registers with
/// ([`crate::extension::PERMISSION_SYSTEM_COMMAND`], pi `index.ts:1502-1503`).
pub const PERMISSION_SYSTEM_COMMAND_DESCRIPTION: &str =
    "Configure cyrup-permission-system debug logging and yolo-mode behavior";

/// pi `createPermissionSystemCommandHandler`'s TUI guard (v0.8.0 `common.ts:188-198`): the
/// `/permission-system` handler refuses outright when there is no interactive UI, because its whole
/// body is "open a modal". Emitted as a `warning` notification, and ONLY as that — the handler
/// returns `Ok(None)` rather than this same sentence, because the session surfaces an
/// `Ok(Some(text))` as a second, Info-level notification.
pub const PERMISSION_SYSTEM_COMMAND_REQUIRES_UI: &str =
    "/permission-system requires interactive TUI mode.";

/// pi `isPrototypePollutionKey` (`common.ts:111-113`): the three key names
/// [`parse_simple_yaml_map`] refuses to store.
///
/// **This is NOT a security fix in Rust, and porting it buys no safety.** Upstream added the guard
/// at v0.8.0 as a JavaScript prototype-pollution defence: there, `record["__proto__"] = child`
/// mutates the object's prototype chain instead of adding an own property, and `constructor` /
/// `prototype` are likewise reachable via `Object.prototype`, so attacker-authored agent
/// frontmatter could corrupt objects it never touched. Rust has no prototype chain, and this
/// parser stores keys in an ordinary ordered `Vec<(String, OrderedValue)>` where `"__proto__"` is
/// just a string like any other — the hazard the guard defends against does not exist here.
///
/// It is ported because it is an **observable parity difference**: upstream yields a map WITHOUT
/// these three keys and cyrup yielded one WITH them, so a `constructor: allow` line under
/// `permission.bash` in an agent's markdown frontmatter became a live permission rule in cyrup
/// while pi ignored it outright.
pub(crate) fn is_prototype_pollution_key(key: &str) -> bool {
    matches!(key, "__proto__" | "constructor" | "prototype")
}

/// pi `common.ts:115-161` `parseSimpleYamlMap`: an indentation-nested map of scalar strings and
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
        let key = strip_edge_quotes(line.get(..sep).unwrap_or("").trim()).to_string();
        // pi `common.ts:133-135`. Placement is load-bearing and matches upstream exactly: the
        // `continue` lands BEFORE the stack pop below, so a dropped key neither opens a nesting
        // level nor closes the enclosing one. Any more-indented lines beneath it therefore
        // re-parent onto the map that was already open, rather than being dropped with it.
        if is_prototype_pollution_key(&key) {
            continue;
        }
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

/// Strip a single pair of matching surrounding single/double quotes (pi `common.ts:152-155`'s
/// scalar-value unquote: `startsWith('"') && endsWith('"')`, or the same for `'`). Strict/paired —
/// used only for the YAML scalar VALUE, which pi genuinely requires to match on both ends.
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

/// Strip a leading quote char and/or a trailing quote char INDEPENDENTLY (pi's
/// `.replace(/^["']|["']$/g, "")`, used for `normalizePathForComparison` (`common.ts:58`) and the
/// YAML map key (`common.ts:132`)). Unlike [`strip_surrounding_quotes`], the two ends need not be
/// present together nor match each other: `"abc` → `abc`, `abc'` → `abc`, `'abc"` → `abc`.
fn strip_edge_quotes(value: &str) -> &str {
    let mut s = value;
    if let Some(rest) = s.strip_prefix('"').or_else(|| s.strip_prefix('\'')) {
        s = rest;
    }
    if let Some(rest) = s.strip_suffix('"').or_else(|| s.strip_suffix('\'')) {
        s = rest;
    }
    s
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
    fn path_quote_stripping_is_independent_per_side_not_paired() {
        // pi: `pathValue.trim().replace(/^["']|["']$/g, "")` strips a leading and/or trailing
        // quote independently — no requirement that both be present or match. A strict
        // matched-pair implementation would leave the stray quote character behind, producing a
        // resource string that never matches a rule written for the unquoted path.
        assert_eq!(
            normalize_path_resource_for_permission("\"@/abs/path", "/work"),
            "/abs/path",
            "leading quote only (no closing quote) must still be stripped"
        );
        assert_eq!(
            normalize_path_resource_for_permission("@/abs/path'", "/work"),
            "/abs/path",
            "trailing quote only (no leading quote) must still be stripped"
        );
        assert_eq!(
            normalize_path_resource_for_permission("'@/abs/path\"", "/work"),
            "/abs/path",
            "mismatched quote types on each end must still both be stripped"
        );
    }

    #[test]
    fn yaml_key_quote_stripping_is_independent_per_side_not_paired() {
        // pi: `line.slice(0, separatorIndex).trim().replace(/^['"]|['"]$/g, "")` — same
        // independent-per-side stripping as the path case, applied to the YAML map key.
        let parsed = parse_simple_yaml_map("\"git *: allow");
        assert_eq!(parsed.get("git *").and_then(|v| v.as_str()), Some("allow"));

        let parsed = parse_simple_yaml_map("git *': allow");
        assert_eq!(parsed.get("git *").and_then(|v| v.as_str()), Some("allow"));

        let parsed = parse_simple_yaml_map("'git *\": allow");
        assert_eq!(parsed.get("git *").and_then(|v| v.as_str()), Some("allow"));
    }

    // pi `common.ts:111-113` + `:133-135`: `__proto__`, `constructor` and `prototype` are dropped
    // by the frontmatter parser at EVERY nesting level. Pre-fix cyrup stored them as ordinary
    // keys, so they reached `normalize_raw_permission` and became live rules.
    #[test]
    fn prototype_pollution_keys_are_dropped_at_every_nesting_level() {
        let parsed = parse_simple_yaml_map(concat!(
            "__proto__: allow\n",
            "constructor: allow\n",
            "prototype: allow\n",
            "permission:\n",
            "  bash:\n",
            "    __proto__: allow\n",
            "    constructor: allow\n",
            "    prototype: allow\n",
            "    echo *: allow\n",
        ));
        for k in ["__proto__", "constructor", "prototype"] {
            assert!(parsed.get(k).is_none(), "top-level key {k:?} must be dropped");
        }
        let bash = parsed.get("permission").unwrap().get("bash").unwrap();
        for k in ["__proto__", "constructor", "prototype"] {
            assert!(bash.get(k).is_none(), "nested key {k:?} must be dropped");
        }
        // MIRROR: dropping the three keys must not disturb their siblings on the same map.
        assert_eq!(bash.get("echo *").and_then(|v| v.as_str()), Some("allow"));
        assert_eq!(
            bash.as_object().unwrap().len(),
            1,
            "only the surviving sibling should remain: {:?}",
            bash.as_object().unwrap()
        );
    }

    // MIRROR: only the three exact names are dropped. A key that merely contains or resembles one
    // of them is an ordinary rule key and must survive — otherwise the port is over-broad and
    // silently deletes operator-authored rules. (pi's check is `===`, `common.ts:112`.)
    #[test]
    fn keys_resembling_prototype_pollution_keys_are_kept() {
        let parsed = parse_simple_yaml_map(concat!(
            "__proto__x: a\n",
            "x__proto__: b\n",
            "Constructor: c\n",
            "my-constructor: d\n",
            "prototypes: e\n",
            "proto: f\n",
        ));
        for (k, v) in [
            ("__proto__x", "a"),
            ("x__proto__", "b"),
            ("Constructor", "c"),
            ("my-constructor", "d"),
            ("prototypes", "e"),
            ("proto", "f"),
        ] {
            assert_eq!(parsed.get(k).and_then(|x| x.as_str()), Some(v), "key {k:?} must survive");
        }
    }

    // pi `common.ts:133-135` `continue`s BEFORE the stack pop/push at `:139-148`, so a dropped
    // key that would have opened a nested map opens nothing AND closes nothing: its would-be
    // children attach to the still-open enclosing map.
    #[test]
    fn children_of_a_dropped_key_reparent_onto_the_enclosing_map() {
        let parsed =
            parse_simple_yaml_map("permission:\n  constructor:\n    bash: allow\n  skills: ask\n");
        let permission = parsed.get("permission").unwrap();
        assert!(permission.get("constructor").is_none());
        // `bash: allow` (indent 4) lands directly on `permission`, not under a `constructor` map.
        assert_eq!(permission.get("bash").and_then(|v| v.as_str()), Some("allow"));
        assert_eq!(permission.get("skills").and_then(|v| v.as_str()), Some("ask"));
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
