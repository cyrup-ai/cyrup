//! `watchdog_diff` — a port of `pi-subagents/src/watchdog/diff-tool.ts` (@ad11b7ab, the tool added
//! by `1bb41617`/#1815, its second description string as `ff2e00bd` left it) (SUBA-233).
//!
//! The reviewer's only view of what a turn changed used to be the changed-path list in the review
//! input; to see a change it had to re-read whole files. This is the tool upstream gives it: the
//! repository diff since a Git `HEAD` captured when the session started, plus untracked file
//! paths. Its description is the one place the reviewer is told that **other sessions' edits in
//! the same checkout are in that diff too**, so the strings are upstream's, verbatim.
//!
//! The baseline is captured when the session binds ([`WatchdogDiffBaselineSlot::start`], upstream's
//! `startWatchdogDiffBaselineCapture` in both `register-main.ts:395` and `register-child.ts:101`)
//! and awaited by the review that needs it. With no baseline — no repository, or no `HEAD` yet —
//! the reviewer is not given the tool and its system prompt does not name it, as upstream's
//! `...(diffBaseline ? [createWatchdogDiffTool(diffBaseline)] : [])` (`review.ts:306`) decides.
//!
//! Git runs as a child process, as upstream's `spawnSync("git", …)` does; nothing here is
//! unix-only.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cyrup_core::{
    CancelToken, Content, ExecMode, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use serde_json::{Value, json};

/// `WATCHDOG_DIFF_TOOL_NAME` (`diff-tool.ts:6`).
pub const WATCHDOG_DIFF_TOOL_NAME: &str = "watchdog_diff";

/// `label: "Watchdog diff"` (`diff-tool.ts:77`).
pub const WATCHDOG_DIFF_TOOL_LABEL: &str = "Watchdog diff";

/// `WATCHDOG_DIFF_MAX_CHARS` (`diff-tool.ts:7`) — in JavaScript string units (UTF-16), as upstream
/// measures `text.length`.
pub const WATCHDOG_DIFF_MAX_CHARS: usize = 24_000;

/// `WATCHDOG_DIFF_UNAVAILABLE_BASELINE` (`diff-tool.ts:8`).
pub const WATCHDOG_DIFF_UNAVAILABLE_BASELINE: &str = "watchdog_diff unavailable: this cwd has no valid Git HEAD baseline. No diff can be shown; inspect files with independent read-only tools instead.";

/// `MAX_UNTRACKED_FILES` (`diff-tool.ts:9`).
const MAX_UNTRACKED_FILES: usize = 50;

/// The description with no baseline (`diff-tool.ts:79`).
pub const WATCHDOG_DIFF_DESCRIPTION_NO_BASELINE: &str =
    "Report that a Git HEAD diff baseline is unavailable in this cwd. No diff is generated.";

/// The description of the working-tree form, against the `HEAD` at reviewer launch
/// (`diff-tool.ts:80-81`, `workingTreeAtLaunch`).
pub const WATCHDOG_DIFF_DESCRIPTION_WORKING_TREE: &str = "Show the current staged and unstaged working-tree delta against reviewer-launch HEAD, plus untracked file paths, including edits by other sessions in this checkout. Committed ranges are not included. Optional path narrows it; stat:true returns per-file counts only.";

/// The description the watchdog reviewer is given: the diff since the session's baseline
/// (`diff-tool.ts:82`).
pub const WATCHDOG_DIFF_DESCRIPTION_SINCE_BASELINE: &str = "Show the repository diff since the review baseline, plus untracked file paths, including edits by other sessions in this checkout. Optional path narrows it; stat:true returns per-file counts only.";

/// `WatchdogDiffBaseline` (`diff-tool.ts:11-14`): the repository root and the `HEAD` it had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogDiffBaseline {
    /// `git rev-parse --show-toplevel`.
    pub root: String,
    /// `git rev-parse HEAD`.
    pub reference: String,
}

/// `runGit(root, args)` (`diff-tool.ts:23-26`).
struct GitOutput {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn run_git(root: &Path, args: &[&str]) -> GitOutput {
    match Command::new("git").arg("-C").arg(root).args(args).output() {
        Ok(output) => GitOutput {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        },
        Err(error) => GitOutput {
            ok: false,
            stdout: String::new(),
            stderr: error.to_string(),
        },
    }
}

/// `BASELINE_ARGS` (`diff-tool.ts:28`).
const BASELINE_ARGS: [&str; 3] = ["rev-parse", "--show-toplevel", "HEAD"];

/// `parseBaseline(stdout)` (`diff-tool.ts:30-36`): `HEAD` is the last line; the repository path
/// itself may contain newlines.
fn parse_baseline(stdout: &str) -> Option<WatchdogDiffBaseline> {
    let mut lines: Vec<&str> = stdout
        .trim()
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let reference = lines.pop()?.to_string();
    let root = lines.join("\n");
    (!root.is_empty() && !reference.is_empty()).then_some(WatchdogDiffBaseline { root, reference })
}

/// `captureWatchdogDiffBaseline(cwd)` (`diff-tool.ts:38-42`) — `HEAD` now, synchronously.
#[must_use]
pub fn capture_watchdog_diff_baseline(cwd: &Path) -> Option<WatchdogDiffBaseline> {
    let result = run_git(cwd, &BASELINE_ARGS);
    if result.ok {
        parse_baseline(&result.stdout)
    } else {
        None
    }
}

/// Where a session's baseline is kept: `let diffBaseline: Promise<…> | undefined`
/// (`register-main.ts:372`, `register-child.ts:60`), set at session start and awaited by the
/// review (`review.ts:293`).
///
/// Empty — never started — is upstream's `undefined`, and a review then has no diff tool.
#[derive(Clone, Default)]
pub struct WatchdogDiffBaselineSlot {
    current: Arc<Mutex<Option<Arc<BaselineCapture>>>>,
}

impl std::fmt::Debug for WatchdogDiffBaselineSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchdogDiffBaselineSlot")
            .finish_non_exhaustive()
    }
}

/// One capture: the `HEAD` of `cwd`, computed once.
struct BaselineCapture {
    cwd: PathBuf,
    result: tokio::sync::OnceCell<Option<WatchdogDiffBaseline>>,
}

impl BaselineCapture {
    async fn get(&self) -> Option<WatchdogDiffBaseline> {
        self.result
            .get_or_init(|| async {
                let cwd = self.cwd.clone();
                tokio::task::spawn_blocking(move || capture_watchdog_diff_baseline(&cwd))
                    .await
                    .ok()
                    .flatten()
            })
            .await
            .clone()
    }
}

impl WatchdogDiffBaselineSlot {
    /// `diffBaseline = startWatchdogDiffBaselineCapture(ctx.cwd)` (`register-main.ts:395`,
    /// `diff-tool.ts:44-51`): replace the session's baseline with `cwd`'s `HEAD` now, so commits
    /// made later — by this session's subagents or anyone else — still show in the diff.
    ///
    /// The capture runs in the background when there is a runtime to run it on, without blocking
    /// the caller ("Does not block the event loop", `:44`); a review that needs it first waits for
    /// it.
    pub fn start(&self, cwd: &Path) {
        let capture = Arc::new(BaselineCapture {
            cwd: cwd.to_path_buf(),
            result: tokio::sync::OnceCell::new(),
        });
        if let Ok(mut slot) = self.current.lock() {
            *slot = Some(Arc::clone(&capture));
        }
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = capture.get().await;
            });
        }
    }

    /// `await options.diffBaseline?.()` (`review.ts:293`) — the baseline, once captured; `None`
    /// when no capture was started, or when the cwd has no valid Git `HEAD`.
    pub async fn get(&self) -> Option<WatchdogDiffBaseline> {
        let capture = self.current.lock().ok().and_then(|slot| slot.clone())?;
        capture.get().await
    }
}

/// `WatchdogDiffParams` (`diff-tool.ts:16-19`).
#[must_use]
pub fn watchdog_diff_parameters_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Restrict the diff to one file or directory, relative to the repo root."
            },
            "stat": {
                "type": "boolean",
                "description": "Return per-file change counts instead of the full diff."
            }
        },
        "additionalProperties": false
    })
}

/// `description` (`diff-tool.ts:78-82`): which of the three strings a tool built over `baseline`
/// carries.
#[must_use]
pub fn watchdog_diff_description(
    baseline: Option<&WatchdogDiffBaseline>,
    working_tree_at_launch: bool,
) -> &'static str {
    if baseline.is_none() {
        WATCHDOG_DIFF_DESCRIPTION_NO_BASELINE
    } else if working_tree_at_launch {
        WATCHDOG_DIFF_DESCRIPTION_WORKING_TREE
    } else {
        WATCHDOG_DIFF_DESCRIPTION_SINCE_BASELINE
    }
}

/// `validatePath(value)` (`diff-tool.ts:53-60`).
fn validate_path(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(trimmed) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if trimmed.starts_with('-') {
        return Err("watchdog_diff path must not start with '-'.".to_string());
    }
    // `path.isAbsolute` — a leading separator, or a Windows drive or UNC form.
    if Path::new(trimmed).is_absolute() || trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err("watchdog_diff path must be relative to the repo root.".to_string());
    }
    if trimmed.split(['/', '\\']).any(|segment| segment == "..") {
        return Err("watchdog_diff path must not contain '..'.".to_string());
    }
    Ok(Some(trimmed.to_string()))
}

/// The UTF-16 length JavaScript's `text.length` measures.
fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The longest prefix of `text` at most `units` UTF-16 units long, on a character boundary.
fn js_prefix(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (index, ch) in text.char_indices() {
        used += ch.len_utf16();
        if used > units {
            return text.get(..index).unwrap_or_default();
        }
    }
    text
}

/// `bound(text)` (`diff-tool.ts:62-66`).
fn bound(text: &str) -> String {
    let length = js_len(text);
    if length <= WATCHDOG_DIFF_MAX_CHARS {
        return text.to_string();
    }
    let marker = format!(
        "\n\n[... {} characters omitted; call again with a narrower path ...]",
        length - WATCHDOG_DIFF_MAX_CHARS
    );
    let keep = WATCHDOG_DIFF_MAX_CHARS.saturating_sub(js_len(&marker));
    format!("{}{marker}", js_prefix(text, keep))
}

/// `execute(_toolCallId, params)` (`diff-tool.ts:86-115`) — the text the tool answers, or the
/// error it throws.
///
/// # Errors
///
/// A path that fails [`validate_path`], or `git diff` failing (`:91`).
pub fn run_watchdog_diff(
    baseline: Option<&WatchdogDiffBaseline>,
    working_tree_at_launch: bool,
    params: &Value,
) -> Result<String, String> {
    let Some(baseline) = baseline else {
        return Ok(WATCHDOG_DIFF_UNAVAILABLE_BASELINE.to_string());
    };
    let path_filter = validate_path(params.get("path").and_then(Value::as_str))?;
    let stat = params.get("stat").and_then(Value::as_bool) == Some(true);
    let root = Path::new(&baseline.root);

    let mut diff_args = vec!["diff", "--no-color", "--no-ext-diff"];
    if stat {
        diff_args.push("--stat");
    }
    diff_args.push(&baseline.reference);
    diff_args.push("--");
    if let Some(filter) = path_filter.as_deref() {
        diff_args.push(filter);
    }
    let diff = run_git(root, &diff_args);
    if !diff.ok {
        let reason = if diff.stderr.is_empty() {
            "unknown error"
        } else {
            &diff.stderr
        };
        return Err(format!("git diff failed: {reason}"));
    }

    let mut untracked_args = vec!["ls-files", "--others", "--exclude-standard", "-z", "--"];
    if let Some(filter) = path_filter.as_deref() {
        untracked_args.push(filter);
    }
    let untracked_result = run_git(root, &untracked_args);
    if working_tree_at_launch {
        let current_head = run_git(root, &["rev-parse", "HEAD"]);
        if !current_head.ok || current_head.stdout.trim() != baseline.reference {
            return Ok("Reviewer-launch HEAD changed or is unavailable. Committed changes are unsupported; relaunch the reviewer or supply a diff artifact.".to_string());
        }
    }
    let untracked: Vec<&str> = if untracked_result.ok {
        untracked_result
            .stdout
            .split('\0')
            .filter(|file| !file.is_empty())
            .collect()
    } else {
        Vec::new()
    };

    let mut sections = vec![diff.stdout.trim_end().to_string()];
    if !untracked.is_empty() {
        let shown: Vec<&str> = untracked
            .iter()
            .take(MAX_UNTRACKED_FILES)
            .copied()
            .collect();
        let mut listing = vec!["Untracked files (use read to inspect):".to_string()];
        listing.extend(shown.iter().map(|file| format!(" {file}")));
        sections.push(listing.join("\n"));
        if untracked.len() > shown.len() {
            sections.push(format!(
                "... {} more untracked files",
                untracked.len() - shown.len()
            ));
        }
    }
    let joined = sections
        .into_iter()
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let text = bound(&joined);
    if !text.is_empty() {
        return Ok(text);
    }
    let short: String = baseline.reference.chars().take(12).collect();
    Ok(if working_tree_at_launch {
        format!(
            "No working-tree changes against reviewer-launch HEAD {short}. Committed changes are not included."
        )
    } else {
        format!("No changes since baseline {short}.")
    })
}

/// `createWatchdogDiffTool(baseline, options)` (`diff-tool.ts:69-117`) as a runtime [`Tool`].
pub struct WatchdogDiffAgentTool {
    baseline: Option<WatchdogDiffBaseline>,
    working_tree_at_launch: bool,
    parameters: Value,
}

impl std::fmt::Debug for WatchdogDiffAgentTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchdogDiffAgentTool")
            .field("baseline", &self.baseline)
            .field("working_tree_at_launch", &self.working_tree_at_launch)
            .finish_non_exhaustive()
    }
}

impl WatchdogDiffAgentTool {
    /// Build the tool over `baseline`; `working_tree_at_launch` is upstream's
    /// `options.workingTreeAtLaunch` (`diff-tool.ts:71-73`).
    #[must_use]
    pub fn new(baseline: Option<WatchdogDiffBaseline>, working_tree_at_launch: bool) -> Self {
        Self {
            baseline,
            working_tree_at_launch,
            parameters: watchdog_diff_parameters_schema(),
        }
    }
}

#[async_trait]
impl Tool for WatchdogDiffAgentTool {
    fn name(&self) -> &str {
        WATCHDOG_DIFF_TOOL_NAME
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    /// `executionMode: "sequential"` (`diff-tool.ts:84`).
    fn execution_mode(&self) -> ExecMode {
        ExecMode::Sequential
    }

    fn description(&self) -> &str {
        watchdog_diff_description(self.baseline.as_ref(), self.working_tree_at_launch)
    }

    fn label(&self) -> Option<&str> {
        Some(WATCHDOG_DIFF_TOOL_LABEL)
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let baseline = self.baseline.clone();
        let working_tree_at_launch = self.working_tree_at_launch;
        let text = tokio::task::spawn_blocking(move || {
            run_watchdog_diff(baseline.as_ref(), working_tree_at_launch, &params)
        })
        .await
        .map_err(|error| ToolError::new(error.to_string()))?
        .map_err(ToolError::new)?;
        let chars = js_len(&text);
        Ok(ToolResult {
            content: vec![Content::text(text)],
            details: Some(json!({ "chars": chars })),
            ..ToolResult::default()
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// A repository with one committed file, `src/lib.rs`.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.invalid"]);
        git(&["config", "user.name", "t"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn the_descriptions_are_upstreams_verbatim() {
        let baseline = WatchdogDiffBaseline {
            root: "/r".into(),
            reference: "abc".into(),
        };
        assert_eq!(
            watchdog_diff_description(Some(&baseline), false),
            "Show the repository diff since the review baseline, plus untracked file paths, including edits by other sessions in this checkout. Optional path narrows it; stat:true returns per-file counts only."
        );
        assert_eq!(
            watchdog_diff_description(Some(&baseline), true),
            "Show the current staged and unstaged working-tree delta against reviewer-launch HEAD, plus untracked file paths, including edits by other sessions in this checkout. Committed ranges are not included. Optional path narrows it; stat:true returns per-file counts only."
        );
        assert_eq!(
            watchdog_diff_description(None, false),
            "Report that a Git HEAD diff baseline is unavailable in this cwd. No diff is generated."
        );
    }

    #[test]
    fn the_baseline_is_the_repository_root_and_head() {
        let dir = repo();
        let baseline = capture_watchdog_diff_baseline(&dir.path().join("src")).unwrap();
        assert_eq!(
            std::fs::canonicalize(&baseline.root).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
        assert_eq!(baseline.reference.len(), 40);
    }

    #[test]
    fn outside_a_repository_or_before_a_first_commit_there_is_no_baseline() {
        let plain = tempfile::tempdir().unwrap();
        // A tempdir inside no repository; `GIT_CEILING_DIRECTORIES` is not needed because the
        // system temp directory is not a checkout.
        assert_eq!(capture_watchdog_diff_baseline(plain.path()), None);
        let empty = tempfile::tempdir().unwrap();
        Command::new("git")
            .arg("-C")
            .arg(empty.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        assert_eq!(capture_watchdog_diff_baseline(empty.path()), None);
    }

    #[test]
    fn an_edit_and_an_untracked_file_are_shown_and_a_path_narrows_it() {
        let dir = repo();
        let baseline = capture_watchdog_diff_baseline(dir.path()).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn two() {}\n").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();

        let all = run_watchdog_diff(Some(&baseline), false, &json!({})).unwrap();
        assert!(all.contains("-pub fn one() {}\n+pub fn two() {}"), "{all}");
        assert!(
            all.contains("Untracked files (use read to inspect):\n notes.txt"),
            "{all}"
        );

        let narrowed =
            run_watchdog_diff(Some(&baseline), false, &json!({ "path": "src/lib.rs" })).unwrap();
        assert!(narrowed.contains("+pub fn two() {}"));
        assert!(!narrowed.contains("notes.txt"), "{narrowed}");

        let stat = run_watchdog_diff(Some(&baseline), false, &json!({ "stat": true })).unwrap();
        assert!(stat.contains("src/lib.rs | 2 +-"), "{stat}");
    }

    #[test]
    fn a_commit_made_after_the_baseline_still_shows() {
        let dir = repo();
        let baseline = capture_watchdog_diff_baseline(dir.path()).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn two() {}\n").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-q", "-m", "child"]] {
            Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(&args)
                .status()
                .unwrap();
        }
        let text = run_watchdog_diff(Some(&baseline), false, &json!({})).unwrap();
        assert!(text.contains("+pub fn two() {}"), "{text}");
    }

    #[test]
    fn a_clean_tree_says_so() {
        let dir = repo();
        let baseline = capture_watchdog_diff_baseline(dir.path()).unwrap();
        let text = run_watchdog_diff(Some(&baseline), false, &json!({})).unwrap();
        assert_eq!(
            text,
            format!(
                "No changes since baseline {}.",
                baseline.reference.get(..12).unwrap()
            )
        );
        let launch = run_watchdog_diff(Some(&baseline), true, &json!({})).unwrap();
        assert!(launch.starts_with("No working-tree changes against reviewer-launch HEAD "));
    }

    #[test]
    fn without_a_baseline_the_tool_answers_the_unavailable_line() {
        assert_eq!(
            run_watchdog_diff(None, false, &json!({ "path": "src/lib.rs" })).unwrap(),
            WATCHDOG_DIFF_UNAVAILABLE_BASELINE
        );
    }

    #[test]
    fn a_path_must_stay_inside_the_repository() {
        let baseline = WatchdogDiffBaseline {
            root: "/nonexistent".into(),
            reference: "abc".into(),
        };
        for (path, message) in [
            ("-p", "watchdog_diff path must not start with '-'."),
            (
                "/etc",
                "watchdog_diff path must be relative to the repo root.",
            ),
            ("src/../..", "watchdog_diff path must not contain '..'."),
        ] {
            assert_eq!(
                run_watchdog_diff(Some(&baseline), false, &json!({ "path": path })).unwrap_err(),
                message
            );
        }
    }

    #[test]
    fn a_long_diff_is_bounded_with_upstreams_marker() {
        let text = "a".repeat(WATCHDOG_DIFF_MAX_CHARS + 100);
        let bounded = bound(&text);
        assert_eq!(js_len(&bounded), WATCHDOG_DIFF_MAX_CHARS);
        assert!(
            bounded
                .ends_with("\n\n[... 100 characters omitted; call again with a narrower path ...]")
        );
    }

    #[tokio::test]
    async fn a_started_slot_answers_the_baseline_and_an_unstarted_one_none() {
        let dir = repo();
        let slot = WatchdogDiffBaselineSlot::default();
        assert_eq!(slot.get().await, None);
        slot.start(dir.path());
        assert_eq!(slot.get().await, capture_watchdog_diff_baseline(dir.path()));
    }
}
