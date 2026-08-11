//! The repo change signature — a 1:1 port of `pi-subagents/src/watchdog/change-signature.ts` (220
//! lines @v0.43.0).
//!
//! The watchdog's default trigger is "the working tree changed since the last review", not "a turn
//! ended". [`compute_watchdog_repo_change_signature`] (`:186-197`) is that trigger: it asks git for
//! the porcelain status of the whole worktree, hashes the *content* of every changed path, and
//! returns a stable key. The runtime compares that key against the previous review's key
//! (`runtime.ts:373,424`), so a turn that produced no net change — an edit and its revert, a
//! read-only turn, a re-run of the same review — is skipped without ever calling a model.
//!
//! Content-hashing rather than status-parsing is deliberate: `git status` reports a path as
//! modified the moment its mtime moves, so a formatter that rewrites a file byte-identically would
//! otherwise trigger an unbounded loop of reviews. Hashing collapses that back to "no change".
//!
//! Three budgets bound the work, all read at CALL time so a test can move them after this module is
//! loaded (upstream says exactly that at `:16-17`):
//! [`MAX_HASH_FILE_BYTES_ENV`] (per-file, 64 MiB), [`MAX_HASH_TOTAL_BYTES_ENV`] (whole-signature,
//! 64 MiB) and [`MAX_HASH_ENTRIES_ENV`] (2 000 filesystem entries). Exceeding a byte budget degrades
//! ONE file to a `large:<size>:<mtime>` metadata marker rather than discarding the signature;
//! exceeding the entry budget emits a `skipped`/`skipped-children` marker so the truncation itself
//! is part of the hashed payload and a later, smaller tree does not collide with an earlier,
//! truncated one.
//!
//! The second export, [`event_indicates_repo_edit`] (`:199-220`), is the fallback trigger for when
//! git is unavailable (no repo, git not installed): a successful `edit`/`write` tool result observed
//! in the event stream. `runtime.ts:706-708` turns that boolean into a synthetic signature key so
//! the boundary still reviews.
//!
//! [CYRUP-DELTA] `spawnSync` is `std::process::Command::output` — blocking, exactly as upstream is.
//! The runtime calls this from an event handler that is `async` in both languages; upstream's own
//! call is synchronous inside that handler, so this port does not move it to a blocking pool and
//! change when the work happens relative to the rest of the boundary.
//!
//! [CYRUP-DELTA] the payload entries are sorted with byte-wise `Ord` where upstream uses
//! `String.prototype.localeCompare`. That ordering feeds ONLY the hash input, and the hash is only
//! ever compared against another hash produced by this same function, so a different total order is
//! not observable. `changed_paths`, which IS user-visible (the status line and the review input),
//! uses upstream's `Array.prototype.sort` — default UTF-16 code-unit order, which for the BMP is
//! exactly Rust's `str` ordering.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// `IGNORED_CHANGE_PREFIXES` (`change-signature.ts:6`), rebranded `.pi-subagents` -> `.cyrup-subagents`
/// to match this crate's own run-artifact directory.
const IGNORED_CHANGE_PREFIXES: &[&str] = &[".cyrup-subagents/", "tmp/", "node_modules/"];
/// `IGNORED_CHANGE_PATHS` (`change-signature.ts:7`).
const IGNORED_CHANGE_PATHS: &[&str] = &[".cyrup-subagents", "tmp", "node_modules"];
/// `IGNORED_CHANGE_SEGMENTS` (`change-signature.ts:8`).
const IGNORED_CHANGE_SEGMENTS: &[&str] = &[".git", ".cyrup-subagents", "node_modules"];

/// `DEFAULT_MAX_HASH_FILE_BYTES` (`change-signature.ts:10`).
pub const DEFAULT_MAX_HASH_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// `DEFAULT_MAX_HASH_TOTAL_BYTES` (`change-signature.ts:11`).
pub const DEFAULT_MAX_HASH_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
/// `DEFAULT_MAX_HASH_ENTRIES` (`change-signature.ts:12`).
pub const DEFAULT_MAX_HASH_ENTRIES: u64 = 2_000;

/// Per-file hash budget override (`PI_SUBAGENTS_MAX_HASH_FILE_BYTES`, `change-signature.ts:21`).
pub const MAX_HASH_FILE_BYTES_ENV: &str = "CYRUP_SUBAGENTS_MAX_HASH_FILE_BYTES";
/// Whole-signature hash budget override (`PI_SUBAGENTS_MAX_HASH_TOTAL_BYTES`, `:25`).
pub const MAX_HASH_TOTAL_BYTES_ENV: &str = "CYRUP_SUBAGENTS_MAX_HASH_TOTAL_BYTES";
/// Entry-count budget override (`PI_SUBAGENTS_MAX_HASH_ENTRIES`, `:29`).
pub const MAX_HASH_ENTRIES_ENV: &str = "CYRUP_SUBAGENTS_MAX_HASH_ENTRIES";

/// `positiveEnvNumber` (`change-signature.ts:14-17`): a finite, strictly positive value wins,
/// anything else falls back.
fn positive_env_number(name: &str, fallback: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|parsed| parsed.is_finite() && *parsed > 0.0)
        .map_or(fallback, |parsed| parsed as u64)
}

/// `WatchdogRepoChangeSignature` (`change-signature.ts:33-37`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogRepoChangeSignature {
    /// The repository top level `git rev-parse --show-toplevel` reported.
    pub root: String,
    /// The sha256 over the whole hashed payload.
    pub key: String,
    /// Every changed path, de-duplicated and sorted.
    pub changed_paths: Vec<String>,
}

/// `git` (`change-signature.ts:39-43`): run `git -C <cwd> <args>`, returning stdout only on a zero
/// exit. A missing `git` binary is the same "undefined" as a non-zero exit.
fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `normalizeRelPath` (`change-signature.ts:45-47`): platform separators to `/`, then drop a leading
/// `./`.
#[must_use]
pub fn normalize_rel_path(value: &str) -> String {
    let slashed = value.replace(std::path::MAIN_SEPARATOR, "/");
    slashed.strip_prefix("./").map_or(slashed.clone(), str::to_string)
}

/// `ignoredRelPath` (`change-signature.ts:49-54`): the exact-path set, the prefix list, or ANY path
/// segment in the segment set.
#[must_use]
pub fn ignored_rel_path(rel_path: &str) -> bool {
    let normalized = normalize_rel_path(rel_path);
    IGNORED_CHANGE_PATHS.contains(&normalized.as_str())
        || IGNORED_CHANGE_PREFIXES.iter().any(|prefix| normalized.starts_with(prefix))
        || normalized.split('/').any(|segment| IGNORED_CHANGE_SEGMENTS.contains(&segment))
}

/// `HashBudget` (`change-signature.ts:56-61`).
#[derive(Debug, Clone, Copy)]
struct HashBudget {
    entries: u64,
    bytes: u64,
    max_entries: u64,
    max_bytes: u64,
}

impl HashBudget {
    /// `useHashEntryBudget` (`change-signature.ts:63-67`): consume one entry slot, or report the
    /// budget exhausted.
    fn use_entry(&mut self) -> bool {
        if self.entries >= self.max_entries {
            return false;
        }
        self.entries = self.entries.saturating_add(1);
        true
    }
}

/// A `path`/`state` object, the base of every hashed entry.
fn entry(path: &str, state: &str) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("path".to_string(), Value::String(path.to_string()));
    map.insert("state".to_string(), Value::String(state.to_string()));
    map
}

/// `createHash("sha256").…digest("hex")` (`change-signature.ts:70-72`). `sha2` 0.11's digest output
/// does not implement `LowerHex`, so the hex encoding is explicit rather than a `{:x}` format.
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().fold(String::with_capacity(64), |mut out, byte| {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// `largeFileHash` (`change-signature.ts:73-75`): the metadata marker a file too big (or too far
/// over budget) to read degrades to.
fn large_file_hash(size: u64, mtime_ms: i64) -> String {
    format!("large:{size}:{mtime_ms}")
}

/// The file's mtime in whole milliseconds since the epoch (`stat.mtimeMs`, floored by upstream's
/// `Math.floor`). An unreadable or pre-epoch mtime reports `0` rather than failing the signature.
fn mtime_millis(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// The POSIX permission bits (`stat.mode & 0o777`).
#[cfg(unix)]
fn permission_bits(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777
}

/// Non-unix has no mode bits to mask; upstream's `stat.mode` is likewise synthetic there.
#[cfg(not(unix))]
fn permission_bits(metadata: &std::fs::Metadata) -> u32 {
    u32::from(!metadata.permissions().readonly()) * 0o666
}

/// `hashFileEntry` (`change-signature.ts:77-96`).
///
/// Both byte budgets are checked BEFORE the read, so an over-budget file is never read at all; a
/// read that fails anyway degrades to the same metadata marker (except `ENOENT`, which mirrors the
/// `lstat` `ENOENT` path and reports the file deleted) so one unreadable file never discards the
/// whole signature.
fn hash_file_entry(
    normalized: &str,
    full_path: &Path,
    metadata: &std::fs::Metadata,
    budget: &mut HashBudget,
) -> Value {
    let size = metadata.len();
    let hash = if size > positive_env_number(MAX_HASH_FILE_BYTES_ENV, DEFAULT_MAX_HASH_FILE_BYTES)
        || budget.bytes.saturating_add(size) > budget.max_bytes
    {
        large_file_hash(size, mtime_millis(metadata))
    } else {
        match std::fs::read(full_path) {
            Ok(bytes) => {
                budget.bytes = budget.bytes.saturating_add(size);
                sha256_hex(&bytes)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Value::Object(entry(normalized, "deleted"));
            }
            Err(err) => {
                tracing::warn!(
                    path = %normalized,
                    error = %err,
                    "[cyrup-subagents] watchdog hashFile fell back to metadata"
                );
                large_file_hash(size, mtime_millis(metadata))
            }
        }
    };
    let mut map = entry(normalized, "file");
    map.insert("mode".to_string(), Value::from(permission_bits(metadata)));
    map.insert("size".to_string(), Value::from(size));
    map.insert("hash".to_string(), Value::String(hash));
    Value::Object(map)
}

/// `gitWorktreeEntry` (`change-signature.ts:98-108`): a nested repository is summarized by its HEAD
/// and a hash of its own porcelain status rather than walked, so a submodule's whole object store is
/// never hashed.
fn git_worktree_entry(normalized: &str, full_path: &Path) -> Value {
    let status = git(full_path, &["status", "--porcelain=v1", "-z", "--untracked-files=no"]);
    let mut map = entry(normalized, "git-worktree");
    map.insert(
        "head".to_string(),
        git(full_path, &["rev-parse", "HEAD"])
            .map_or(Value::Null, |head| Value::String(head.trim().to_string())),
    );
    let dirty = status.as_ref().is_some_and(|s| !s.is_empty());
    map.insert("dirty".to_string(), Value::Bool(dirty));
    if let Some(status) = status.filter(|s| !s.is_empty()) {
        map.insert(
            "statusKey".to_string(),
            Value::String(sha256_hex(status.as_bytes())),
        );
    }
    Value::Object(map)
}

/// `hashPath` (`change-signature.ts:110-136`) — the recursive walk.
///
/// Returns `Err` only for an `lstat` failure that is not `ENOENT`, which upstream rethrows out of
/// `buildRepoChangeSignature` into its own catch.
fn hash_path(
    root: &Path,
    rel_path: &str,
    budget: &mut HashBudget,
) -> Result<Value, std::io::Error> {
    let normalized = normalize_rel_path(rel_path);
    if !budget.use_entry() {
        let mut map = entry(&normalized, "skipped");
        map.insert("reason".to_string(), Value::String("entry-limit".to_string()));
        return Ok(Value::Object(map));
    }
    let full_path = root.join(&normalized);
    let metadata = match std::fs::symlink_metadata(&full_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Value::Object(entry(&normalized, "deleted")));
        }
        Err(err) => return Err(err),
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let mut map = entry(&normalized, "symlink");
        map.insert(
            "target".to_string(),
            Value::String(
                std::fs::read_link(&full_path)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
        );
        return Ok(Value::Object(map));
    }
    if file_type.is_dir() {
        if full_path.join(".git").exists() {
            return Ok(git_worktree_entry(&normalized, &full_path));
        }
        let mut names: Vec<String> = std::fs::read_dir(&full_path)?
            .filter_map(Result::ok)
            .map(|dir_entry| {
                normalize_rel_path(&format!(
                    "{normalized}/{}",
                    dir_entry.file_name().to_string_lossy()
                ))
            })
            .filter(|child| !ignored_rel_path(child))
            .collect();
        names.sort();
        let remaining = budget.max_entries.saturating_sub(budget.entries);
        let take = usize::try_from(remaining).unwrap_or(usize::MAX).min(names.len());
        let skipped = names.len() - take;
        let mut child_entries: Vec<Value> = Vec::with_capacity(take + usize::from(skipped > 0));
        for child in names.iter().take(take) {
            child_entries.push(hash_path(root, child, budget)?);
        }
        if skipped > 0 {
            let mut map = entry(&normalized, "skipped-children");
            map.insert("reason".to_string(), Value::String("entry-limit".to_string()));
            map.insert("count".to_string(), Value::from(skipped));
            child_entries.push(Value::Object(map));
        }
        let mut map = entry(&normalized, "dir");
        map.insert("entries".to_string(), Value::Array(child_entries));
        return Ok(Value::Object(map));
    }
    if file_type.is_file() {
        return Ok(hash_file_entry(&normalized, &full_path, &metadata, budget));
    }
    let mut map = entry(&normalized, "other");
    map.insert("mode".to_string(), Value::from(permission_bits(&metadata)));
    Ok(Value::Object(map))
}

/// One `git status --porcelain=v1 -z` record.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PorcelainEntry {
    status: String,
    paths: Vec<String>,
}

/// `parsePorcelainZ` (`change-signature.ts:138-154`): NUL-separated records, two status characters,
/// a space, then the path — and for a rename or a copy (`R`/`C`) the ORIGINAL path follows as its
/// own NUL-separated token, which the loop consumes by advancing the index.
fn parse_porcelain_z(raw: &str) -> Vec<PorcelainEntry> {
    let tokens: Vec<&str> = raw.split('\0').filter(|token| !token.is_empty()).collect();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let Some(token) = tokens.get(index) else { break };
        if token.len() < 4 {
            index += 1;
            continue;
        }
        let status = token.get(..2).unwrap_or_default().to_string();
        let rel_path = token.get(3..).unwrap_or_default().to_string();
        let mut paths = vec![rel_path];
        if status.starts_with('R') || status.starts_with('C') {
            index += 1;
            if let Some(original) = tokens.get(index)
                && !original.is_empty()
            {
                paths.push((*original).to_string());
            }
        }
        entries.push(PorcelainEntry { status, paths });
        index += 1;
    }
    entries
}

/// `buildRepoChangeSignature` (`change-signature.ts:156-184`).
fn build_repo_change_signature(
    root: &Path,
    status_output: &str,
) -> Result<WatchdogRepoChangeSignature, std::io::Error> {
    build_repo_change_signature_with(
        root,
        status_output,
        HashBudget {
            entries: 0,
            bytes: 0,
            max_entries: positive_env_number(MAX_HASH_ENTRIES_ENV, DEFAULT_MAX_HASH_ENTRIES),
            max_bytes: positive_env_number(MAX_HASH_TOTAL_BYTES_ENV, DEFAULT_MAX_HASH_TOTAL_BYTES),
        },
    )
}

/// [`build_repo_change_signature`] with the budget supplied rather than read from the environment.
///
/// Upstream reads its three budgets at CALL time precisely so a test can move them
/// (`change-signature.ts:16-17`); this crate is `#![forbid(unsafe_code)]` and Rust 2024 makes
/// `std::env::set_var` unsafe, so the seam is a parameter instead of a process-global write — which
/// is also what keeps the budget test from racing every other test in the binary.
fn build_repo_change_signature_with(
    root: &Path,
    status_output: &str,
    mut budget: HashBudget,
) -> Result<WatchdogRepoChangeSignature, std::io::Error> {
    let mut entries: Vec<PorcelainEntry> = parse_porcelain_z(status_output)
        .into_iter()
        .map(|entry| PorcelainEntry {
            status: entry.status,
            paths: entry
                .paths
                .iter()
                .map(|p| normalize_rel_path(p))
                .filter(|p| !ignored_rel_path(p))
                .collect(),
        })
        .filter(|entry| !entry.paths.is_empty())
        .collect();
    entries.sort_by(|a, b| {
        format!("{} {}", a.status, a.paths.join("\0"))
            .cmp(&format!("{} {}", b.status, b.paths.join("\0")))
    });
    let changed_paths: Vec<String> = entries
        .iter()
        .flat_map(|entry| entry.paths.iter().cloned())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();
    let mut payload = Vec::with_capacity(entries.len());
    for entry in &entries {
        let mut map = Map::new();
        map.insert("status".to_string(), Value::String(entry.status.clone()));
        map.insert(
            "paths".to_string(),
            Value::Array(entry.paths.iter().cloned().map(Value::String).collect()),
        );
        let mut content = Vec::with_capacity(entry.paths.len());
        for rel_path in &entry.paths {
            content.push(hash_path(root, rel_path, &mut budget)?);
        }
        map.insert("content".to_string(), Value::Array(content));
        payload.push(Value::Object(map));
    }
    let serialized = serde_json::to_string(&Value::Array(payload))
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(WatchdogRepoChangeSignature {
        root: root.to_string_lossy().into_owned(),
        key: sha256_hex(serialized.as_bytes()),
        changed_paths,
    })
}

/// `computeWatchdogRepoChangeSignature` (`change-signature.ts:186-197`) — the entry point.
///
/// `None` means "no signature is available", which the runtime treats as "fall back to the observed
/// edit flag" (`runtime.ts:706-708`), never as "nothing changed". Three ways to get it: `cwd` is not
/// inside a git repository, the status call itself failed, or the walk threw.
#[must_use]
pub fn compute_watchdog_repo_change_signature(cwd: &Path) -> Option<WatchdogRepoChangeSignature> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"])?;
    let root = root.trim();
    if root.is_empty() {
        return None;
    }
    let root = Path::new(root);
    let status_output = git(root, &["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
    match build_repo_change_signature(root, &status_output) {
        Ok(signature) => Some(signature),
        Err(err) => {
            tracing::warn!(
                error = %err,
                "[cyrup-subagents] watchdog repo change signature failed"
            );
            None
        }
    }
}

/// `toolResultSucceeded` (`change-signature.ts:204-206`): neither `isError: true` nor a present
/// `error`. Note upstream tests `message.error === undefined`, so an explicit `error: null` counts
/// as a FAILURE — reproduced here with the same "key absent" test.
fn tool_result_succeeded(message: &Map<String, Value>) -> bool {
    message.get("isError") != Some(&Value::Bool(true)) && !message.contains_key("error")
}

/// `messageIndicatesRepoEdit` (`change-signature.ts:208-216`): a SUCCESSFUL `edit`/`write` tool
/// result, under either the `toolResult` or the `tool` role.
fn message_indicates_repo_edit(message: &Value) -> bool {
    let Some(input) = message.as_object() else {
        return false;
    };
    let role = input.get("role").and_then(Value::as_str);
    if role != Some("toolResult") && role != Some("tool") {
        return false;
    }
    let tool_name = input
        .get("toolName")
        .or_else(|| input.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    (tool_name == "edit" || tool_name == "write") && tool_result_succeeded(input)
}

/// `eventIndicatesRepoEdit` (`change-signature.ts:218-220` plus the `turn_end`/`tool_result` arms at
/// `:210-215`) — the git-free fallback trigger.
///
/// Three event shapes, in upstream's order: `turn_end` (its own message plus every tool result),
/// `tool_result` (the event's own fields re-labelled as a `toolResult` message), and
/// `tool_result_end` (its nested `message`). Anything else is not an edit.
#[must_use]
pub fn event_indicates_repo_edit(event: &Value) -> bool {
    let Some(input) = event.as_object() else {
        return false;
    };
    let is_type = |name: &str| {
        input.get("type").and_then(Value::as_str) == Some(name)
            || input.get("event").and_then(Value::as_str) == Some(name)
    };
    if is_type("turn_end") {
        if input.get("message").is_some_and(message_indicates_repo_edit) {
            return true;
        }
        return input
            .get("toolResults")
            .and_then(Value::as_array)
            .is_some_and(|results| results.iter().any(message_indicates_repo_edit));
    }
    if is_type("tool_result") {
        // `{ role: "toolResult", ...input }` — the spread comes AFTER the role, so an event that
        // carries its own `role` key overrides the synthesized one.
        let mut synthesized = Map::new();
        synthesized.insert("role".to_string(), Value::String("toolResult".to_string()));
        for (key, value) in input {
            synthesized.insert(key.clone(), value.clone());
        }
        return message_indicates_repo_edit(&Value::Object(synthesized));
    }
    if !is_type("tool_result_end") {
        return false;
    }
    input.get("message").is_some_and(message_indicates_repo_edit)
}

// -------------------------------------------------------------------------------------------
// The runtime seam (`runtime.ts:87,179` — the injectable `repoChangeSignature` option)
// -------------------------------------------------------------------------------------------

/// The REAL [`super::runtime::WatchdogRepoChangeSource`]: git porcelain plus content hashing.
///
/// This is what replaces the runtime's `NoRepoChangeSignatures` placeholder in production
/// ([`crate::extension::SubagentsExtension`]'s watchdog construction). Without it a
/// `review_changes_only` runtime never sees a signature at all and falls back to the observed-edit
/// trigger for every boundary — which reviews correctly but re-reviews an unchanged tree.
#[derive(Debug, Clone, Copy, Default)]
pub struct GitRepoChangeSource;

impl super::runtime::WatchdogRepoChangeSource for GitRepoChangeSource {
    fn compute(&self, cwd: &Path) -> Option<super::runtime::WatchdogRepoChangeSignature> {
        compute_watchdog_repo_change_signature(cwd).map(|signature| {
            super::runtime::WatchdogRepoChangeSignature {
                root: signature.root,
                key: signature.key,
                changed_paths: signature.changed_paths,
            }
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// A real git repository — the signature is defined by git's own porcelain output, so there is
    /// no meaningful way to test it against a fake.
    fn repo() -> Option<TempDir> {
        let tmp = TempDir::new().ok()?;
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(tmp.path())
                .args(args)
                .output()
                .ok()
                .filter(|o| o.status.success())
        };
        run(&["init", "-q"])?;
        run(&["config", "user.email", "t@example.com"])?;
        run(&["config", "user.name", "t"])?;
        Some(tmp)
    }

    #[test]
    fn ignored_paths_cover_exact_prefix_and_segment_matches() {
        assert!(ignored_rel_path("node_modules"));
        assert!(ignored_rel_path("node_modules/pkg/index.js"));
        assert!(ignored_rel_path("tmp/"));
        assert!(ignored_rel_path("a/.git/config"));
        assert!(ignored_rel_path(".cyrup-subagents/runs/x.json"));
        assert!(!ignored_rel_path("src/tmpfile.rs"));
        assert!(!ignored_rel_path("src/main.rs"));
    }

    #[test]
    fn normalize_strips_the_leading_dot_slash() {
        assert_eq!(normalize_rel_path("./a/b"), "a/b");
        assert_eq!(normalize_rel_path("a/b"), "a/b");
    }

    #[test]
    fn porcelain_z_consumes_the_original_path_of_a_rename() {
        let raw = " M src/a.rs\0R  src/new.rs\0src/old.rs\0?? src/c.rs\0";
        let entries = parse_porcelain_z(raw);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].paths, vec!["src/a.rs"]);
        assert_eq!(entries[1].status, "R ");
        assert_eq!(entries[1].paths, vec!["src/new.rs", "src/old.rs"]);
        assert_eq!(entries[2].paths, vec!["src/c.rs"]);
    }

    #[test]
    fn a_short_or_empty_token_is_skipped_without_desyncing_the_scan() {
        let entries = parse_porcelain_z("xx\0 M a.rs\0");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].paths, vec!["a.rs"]);
    }

    #[test]
    fn a_directory_outside_any_repo_has_no_signature() {
        let tmp = TempDir::new().unwrap();
        // `git rev-parse --show-toplevel` fails outside a work tree. If the temp dir happens to sit
        // inside one (a developer's /tmp is not usually a repo, but be exact), the signature is
        // still well-formed rather than absent.
        match compute_watchdog_repo_change_signature(tmp.path()) {
            None => {}
            Some(signature) => assert_eq!(signature.key.len(), 64),
        }
    }

    #[test]
    fn an_identical_rewrite_keeps_the_same_key_but_a_content_change_moves_it() {
        let Some(tmp) = repo() else {
            return; // no usable git binary in this environment
        };
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, "one").unwrap();
        let first = compute_watchdog_repo_change_signature(tmp.path()).unwrap();
        assert_eq!(first.changed_paths, vec!["a.txt".to_string()]);
        // Rewrite byte-identically: git reports it, the content hash does not.
        std::fs::write(&file, "one").unwrap();
        let second = compute_watchdog_repo_change_signature(tmp.path()).unwrap();
        assert_eq!(first.key, second.key, "identical content must not move the key");
        std::fs::write(&file, "two").unwrap();
        let third = compute_watchdog_repo_change_signature(tmp.path()).unwrap();
        assert_ne!(first.key, third.key, "changed content must move the key");
    }

    #[test]
    fn changed_paths_are_deduplicated_and_sorted() {
        let Some(tmp) = repo() else { return };
        for name in ["c.txt", "a.txt", "b.txt"] {
            std::fs::write(tmp.path().join(name), name).unwrap();
        }
        let signature = compute_watchdog_repo_change_signature(tmp.path()).unwrap();
        assert_eq!(
            signature.changed_paths,
            vec!["a.txt".to_string(), "b.txt".to_string(), "c.txt".to_string()]
        );
    }

    #[test]
    fn ignored_directories_do_not_appear_in_changed_paths() {
        let Some(tmp) = repo() else { return };
        std::fs::create_dir_all(tmp.path().join("node_modules/pkg")).unwrap();
        std::fs::write(tmp.path().join("node_modules/pkg/i.js"), "x").unwrap();
        std::fs::write(tmp.path().join("kept.txt"), "x").unwrap();
        let signature = compute_watchdog_repo_change_signature(tmp.path()).unwrap();
        assert_eq!(signature.changed_paths, vec!["kept.txt".to_string()]);
    }

    #[test]
    fn the_entry_budget_truncates_rather_than_failing() {
        let tmp = TempDir::new().unwrap();
        for i in 0..5 {
            std::fs::write(tmp.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        let status = (0..5)
            .map(|i| format!("?? f{i}.txt\0"))
            .collect::<String>();
        let budget = |max_entries: u64| HashBudget {
            entries: 0,
            bytes: 0,
            max_entries,
            max_bytes: DEFAULT_MAX_HASH_TOTAL_BYTES,
        };
        let full =
            build_repo_change_signature_with(tmp.path(), &status, budget(2_000)).unwrap();
        let truncated = build_repo_change_signature_with(tmp.path(), &status, budget(2)).unwrap();
        assert_eq!(truncated.changed_paths.len(), 5, "the path list is not budgeted");
        assert_ne!(full.key, truncated.key, "the truncation is part of the hashed payload");
    }

    #[test]
    fn a_byte_budget_degrades_one_file_to_its_metadata_marker() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("big.txt"), "x".repeat(64)).unwrap();
        let status = "?? big.txt\0";
        let generous = HashBudget {
            entries: 0,
            bytes: 0,
            max_entries: 100,
            max_bytes: DEFAULT_MAX_HASH_TOTAL_BYTES,
        };
        let starved = HashBudget {
            entries: 0,
            bytes: 0,
            max_entries: 100,
            max_bytes: 1,
        };
        let hashed = build_repo_change_signature_with(tmp.path(), status, generous).unwrap();
        let metadata = build_repo_change_signature_with(tmp.path(), status, starved).unwrap();
        assert_ne!(hashed.key, metadata.key);
        assert_eq!(metadata.changed_paths, vec!["big.txt".to_string()]);
    }

    #[test]
    fn only_a_successful_edit_or_write_result_indicates_a_repo_edit() {
        let edit = json!({
            "type": "tool_result", "role": "toolResult", "toolName": "edit", "content": "ok"
        });
        assert!(event_indicates_repo_edit(&edit));
        let failed = json!({
            "type": "tool_result", "role": "toolResult", "toolName": "edit", "isError": true
        });
        assert!(!event_indicates_repo_edit(&failed));
        let errored = json!({
            "type": "tool_result", "role": "toolResult", "toolName": "write", "error": "boom"
        });
        assert!(!event_indicates_repo_edit(&errored));
        let read = json!({ "type": "tool_result", "toolName": "read" });
        assert!(!event_indicates_repo_edit(&read));
    }

    #[test]
    fn turn_end_checks_its_message_and_every_tool_result() {
        let event = json!({
            "type": "turn_end",
            "message": { "role": "assistant", "content": "x" },
            "toolResults": [
                { "role": "toolResult", "toolName": "read", "content": "a" },
                { "role": "tool", "name": "write", "content": "b" },
            ],
        });
        assert!(event_indicates_repo_edit(&event));
        let clean = json!({
            "type": "turn_end",
            "toolResults": [{ "role": "toolResult", "toolName": "read" }],
        });
        assert!(!event_indicates_repo_edit(&clean));
    }

    #[test]
    fn tool_result_end_reads_its_nested_message() {
        let event = json!({
            "event": "tool_result_end",
            "message": { "role": "toolResult", "toolName": "write", "content": "ok" },
        });
        assert!(event_indicates_repo_edit(&event));
        assert!(!event_indicates_repo_edit(&json!({ "event": "tool_result_end" })));
        assert!(!event_indicates_repo_edit(&json!("not an object")));
    }
}
