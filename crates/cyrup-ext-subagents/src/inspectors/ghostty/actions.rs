//! ghostty's one verb — `open` — over `osascript`.
//!
//! Upstream: `src/inspectors/ghostty/actions.ts` (74 lines @v0.68.0). Ghostty 1.3's AppleScript
//! surface splits the front window's focused terminal, points the new surface at a working
//! directory, and runs one command in it. There is no binding file and no pane registry, so this
//! backend has **no `status` and no `close`** — and its own success sentence says so
//! (`ghostty/actions.ts:69`).
//!
//! # Why this compiles, and is tested, on Linux
//!
//! Upstream injects both the platform and the runner (`ghostty/plugin.ts:5-6,10`), so its own
//! tests drive the AppleScript path off macOS. cyrup does the same through
//! [`crate::inspectors::plugins::GhosttyRunner`]: [`GHOSTTY_APPLESCRIPT`], the argv builder and
//! [`open_ghostty_inspector`] are platform-independent, and only [`OsascriptRunner`] ever touches
//! `/usr/bin/osascript`. Nothing here is `#[cfg]`-ed out — the platform gate lives in
//! [`super::plugin::GhosttyInspectorPlugin::available`], exactly where upstream puts it.
//!
//! # `[CYRUP-DELTA: the runner's error channel carries a CODE, not upstream's `cause.message`]`
//!
//! Upstream's catch renders `Ghostty inspector error: ${cause.message}. ${hint}`
//! (`ghostty/actions.ts:70-72`) where `cause` is Node's `execFile` rejection — which fires for a
//! spawn failure, a timeout **and** a non-zero exit, and whose `message` carries the real text.
//! The FROZEN [`GhosttyRunner`](crate::inspectors::plugins::GhosttyRunner) returns
//! `Result<CommandOutput, HerdrErrorCode>`: a bare code with no message. So this module recovers
//! upstream's text where the information still exists and renders the code where it does not:
//!
//! * **Non-zero exit** — the realistic AppleScript failure (no Automation permission, no front
//!   window, Ghostty below 1.3). [`OsascriptRunner`] returns `Ok` with the captured stderr, and
//!   [`process_failure_message`] renders Node's own `Command failed: …` shape over it, so the
//!   user reads osascript's actual complaint.
//! * **Spawn failure / timeout** — no text survives the frozen signature, so
//!   [`runner_failure_message`] renders the code's own spelling.
//!
//! Reported in this batch's `contractGaps`: the fix is one message field on the seam, and it is
//! the orchestrator's call, not a local redefinition.

use std::time::Duration;

use cyrup_core::{Content, ToolError, ToolResult};

use crate::inspectors::plugins::GhosttyRunner;
use crate::inspectors::types::{
    CommandOutput, HerdrErrorCode, InspectorContext, InspectorLaunch, InspectorParams,
};

/// Static Ghostty 1.3 AppleScript; dynamic values arrive as `on run argv` arguments
/// (`ghostty/actions.ts:7-26`, verbatim — a reflowed script is a different script).
pub const GHOSTTY_APPLESCRIPT: &str = r#"on run argv
  set launchCommand to item 1 of argv
  set launchCwd to item 2 of argv
  set shouldFocus to item 3 of argv
  tell application "Ghostty"
    set sourceWindow to front window
    set sourceTab to selected tab of sourceWindow
    set sourceTerminal to focused terminal of sourceTab
    set surfaceConfiguration to new surface configuration
    set initial working directory of surfaceConfiguration to launchCwd
    set command of surfaceConfiguration to launchCommand
    set newTerminal to split sourceTerminal direction right with configuration surfaceConfiguration
    if shouldFocus is "true" then
      focus newTerminal
    else
      focus sourceTerminal
    end if
    return id of newTerminal
  end tell
end run"#;

/// pi's `failureHint` (`ghostty/actions.ts:52`) — appended to BOTH failure sentences, and to
/// neither success sentence.
pub const GHOSTTY_FAILURE_HINT: &str =
    "Ghostty inspector requires Ghostty 1.3+ and Automation permission for osascript.";

/// pi's `execFile("/usr/bin/osascript", …)` target (`ghostty/actions.ts:32`) — the absolute path,
/// never a `PATH` lookup.
pub const OSASCRIPT_BIN: &str = "/usr/bin/osascript";

/// pi `timeout: 15_000` (`ghostty/actions.ts:63`).
pub const OSASCRIPT_TIMEOUT: Duration = Duration::from_millis(15_000);

/// pi `maxBuffer: 64 * 1024` (`ghostty/actions.ts:65`). Node REJECTS the call when either stream
/// exceeds it; this port truncates instead, because the value it is protecting is a terminal id of
/// a dozen bytes and a truncated id still fails the empty/invalid check honestly.
pub const OSASCRIPT_MAX_BUFFER: usize = 64 * 1024;

/// The real `osascript` seam — pi's `defaultRunner` (`ghostty/actions.ts:30-41`).
///
/// `tokio::process::Command` is this crate's child-process idiom
/// (`exec/external_cli/preflight.rs:269`, `spawn/spawn_detached.rs:213`), plus
/// `tokio::time::timeout` for `:63`'s deadline and `kill_on_drop` so an abandoned `osascript`
/// never outlives the verb.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsascriptRunner;

impl OsascriptRunner {
    /// Construct the real runner.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl GhosttyRunner for OsascriptRunner {
    async fn run(&self, args: &[&str]) -> Result<CommandOutput, HerdrErrorCode> {
        let mut command = tokio::process::Command::new(OSASCRIPT_BIN);
        command.args(args);
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command.kill_on_drop(true);

        let output = match tokio::time::timeout(OSASCRIPT_TIMEOUT, command.output()).await {
            // Node's `spawn … ENOENT` rejection: the binary is absent, which on a non-macOS host
            // is the normal state. `HERDR_UNAVAILABLE` is the frozen vocabulary's "the host is not
            // here" code.
            Ok(Err(_)) => return Err(HerdrErrorCode::Unavailable),
            Ok(Ok(output)) => output,
            Err(_) => return Err(HerdrErrorCode::Timeout),
        };

        Ok(CommandOutput {
            stdout: truncate_to_buffer(&String::from_utf8_lossy(&output.stdout)),
            stderr: truncate_to_buffer(&String::from_utf8_lossy(&output.stderr)),
            exit_code: output.status.code(),
        })
    }
}

/// pi `maxBuffer` (`:65`), applied by truncation — see [`OSASCRIPT_MAX_BUFFER`].
fn truncate_to_buffer(text: &str) -> String {
    if text.len() <= OSASCRIPT_MAX_BUFFER {
        return text.to_owned();
    }
    let mut end = OSASCRIPT_MAX_BUFFER;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default().to_owned()
}

/// The message for a runner that never produced a process — see this module's delta.
fn runner_failure_message(code: HerdrErrorCode) -> String {
    code.as_str().to_owned()
}

/// The message for a process that ran and failed, in Node's own `execFile` rejection shape
/// (`Command failed: <cmd>` followed by the captured stderr), so the user reads osascript's real
/// complaint rather than an exit number.
fn process_failure_message(output: &CommandOutput) -> String {
    let stderr = output.stderr.trim();
    if stderr.is_empty() {
        format!("Command failed: {OSASCRIPT_BIN}")
    } else {
        format!("Command failed: {OSASCRIPT_BIN}\n{stderr}")
    }
}

/// pi's `result(text)` with no second argument (`ghostty/actions.ts:43-50`) — `isError` absent,
/// which in cyrup is `Ok` (see [`crate::inspectors::types`]'s module doc for the whole mapping).
fn ok_text(text: String) -> ToolResult {
    ToolResult {
        content: vec![Content::text(text)],
        ..Default::default()
    }
}

/// The argv pi hands `osascript` (`ghostty/actions.ts:61`), built once so the plugin, the tests
/// and any future caller agree by construction.
///
/// `cwd` is upstream's `context.target.status.cwd ?? context.cwd`, which the dispatcher has
/// already resolved onto [`InspectorContext::trusted_dir`].
#[must_use]
pub fn ghostty_argv(launch: &InspectorLaunch, cwd: &str, focus: bool) -> Vec<String> {
    vec![
        "-e".to_owned(),
        GHOSTTY_APPLESCRIPT.to_owned(),
        "--".to_owned(),
        launch.display_command.clone(),
        cwd.to_owned(),
        // pi `params.focus === true ? "true" : "false"` — a STRICT comparison, so an absent
        // `focus` is `"false"` and the AppleScript refocuses the source terminal.
        if focus { "true" } else { "false" }.to_owned(),
    ]
}

/// pi `openGhosttyInspector(context, launch, params, runner)` (`ghostty/actions.ts:54-74`).
///
/// # Errors
///
/// `Err(ToolError)` — upstream's `isError: true` — for a runner failure, a non-zero exit, and an
/// empty terminal id. All three carry [`GHOSTTY_FAILURE_HINT`]; none of them writes anything to
/// disk, because this backend has no binding to write.
pub async fn open_ghostty_inspector(
    ctx: &InspectorContext,
    launch: &InspectorLaunch,
    params: &InspectorParams,
    runner: &dyn GhosttyRunner,
) -> Result<ToolResult, ToolError> {
    let cwd = ctx.trusted_dir.to_string_lossy().into_owned();
    let argv = ghostty_argv(launch, &cwd, params.focus == Some(true));
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();

    let output = match runner.run(&borrowed).await {
        Ok(output) => output,
        Err(code) => {
            return Err(ToolError::new(format!(
                "Ghostty inspector error: {}. {GHOSTTY_FAILURE_HINT}",
                runner_failure_message(code)
            )));
        }
    };

    // Node's `execFile` rejects a non-zero exit, so upstream never reaches the id check with a
    // failed process. `None` is a signalled process, which Node also rejects.
    if output.exit_code != Some(0) {
        return Err(ToolError::new(format!(
            "Ghostty inspector error: {}. {GHOSTTY_FAILURE_HINT}",
            process_failure_message(&output)
        )));
    }

    let id = output.stdout.trim();
    if id.is_empty() {
        return Err(ToolError::new(format!(
            "Ghostty inspector error: Ghostty returned an empty terminal id. {GHOSTTY_FAILURE_HINT}"
        )));
    }

    Ok(ok_text(format!(
        "Opened read-only Ghostty inspector terminal {id} for async run {}. Status and close are \
         unavailable because this plugin writes no binding.",
        ctx.target.run_id
    )))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::*;
    use crate::inspectors::types::InspectorTarget;

    /// A scripted `osascript`, recording every argv it is handed.
    #[derive(Default)]
    struct FakeRunner {
        calls: Mutex<Vec<Vec<String>>>,
        answer: Mutex<Option<Result<CommandOutput, HerdrErrorCode>>>,
    }

    impl FakeRunner {
        fn stdout(text: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                answer: Mutex::new(Some(Ok(CommandOutput {
                    stdout: text.to_owned(),
                    stderr: String::new(),
                    exit_code: Some(0),
                }))),
            }
        }

        fn failing(output: CommandOutput) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                answer: Mutex::new(Some(Ok(output))),
            }
        }

        fn refusing(code: HerdrErrorCode) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                answer: Mutex::new(Some(Err(code))),
            }
        }

        fn argv(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl GhosttyRunner for FakeRunner {
        async fn run(&self, args: &[&str]) -> Result<CommandOutput, HerdrErrorCode> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|a| (*a).to_owned()).collect());
            self.answer
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(Ok(CommandOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: Some(0),
                }))
        }
    }

    fn context(cwd: &Path) -> InspectorContext {
        InspectorContext {
            target: InspectorTarget {
                run_id: "run-9".to_owned(),
                async_dir: cwd.join("async"),
                child_index: None,
            },
            trusted_dir: cwd.to_path_buf(),
            mission_id: None,
            mission_path: None,
            // These tests drive `open_ghostty_inspector` directly, below the `available()` gate
            // that is the only reader of `env`.
            env: std::collections::BTreeMap::new(),
        }
    }

    fn launch() -> InspectorLaunch {
        InspectorLaunch {
            exe: "cyrup".to_owned(),
            args: vec!["__subagent-inspector".to_owned()],
            display_command: "cyrup __subagent-inspector --run-id run-9".to_owned(),
        }
    }

    fn text_of(result: &ToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// T-GHOST-1 — the exact argv and the exact success sentence, on THIS Linux box.
    ///
    /// GUT: drop `"--"` from [`ghostty_argv`], or reorder command/cwd/focus. osascript reads
    /// `on run argv` positionally, so a reorder opens a terminal in the wrong directory running
    /// the wrong string, and nothing but this assertion sees it.
    #[tokio::test]
    async fn ghostty_opens_through_an_injected_runner_on_any_platform() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::stdout("term-7\n");
        let params = InspectorParams {
            pane_id: None,
            focus: Some(false),
        };
        let reply = open_ghostty_inspector(&context(dir.path()), &launch(), &params, &runner)
            .await
            .unwrap();

        assert_eq!(
            runner.argv(),
            vec![vec![
                "-e".to_owned(),
                GHOSTTY_APPLESCRIPT.to_owned(),
                "--".to_owned(),
                "cyrup __subagent-inspector --run-id run-9".to_owned(),
                dir.path().to_string_lossy().into_owned(),
                "false".to_owned(),
            ]]
        );
        assert_eq!(
            text_of(&reply),
            "Opened read-only Ghostty inspector terminal term-7 for async run run-9. Status and \
             close are unavailable because this plugin writes no binding."
        );
    }

    /// pi `params.focus === true` (`:61`) — a STRICT comparison. GUT it to `!= Some(false)` and an
    /// unfocused open steals the user's keyboard mid-task.
    #[tokio::test]
    async fn focus_is_a_strict_true_comparison() {
        let dir = tempfile::tempdir().unwrap();

        for (params, expected) in [
            (InspectorParams::default(), "false"),
            (
                InspectorParams {
                    pane_id: None,
                    focus: Some(false),
                },
                "false",
            ),
            (
                InspectorParams {
                    pane_id: None,
                    focus: Some(true),
                },
                "true",
            ),
        ] {
            let runner = FakeRunner::stdout("t1");
            open_ghostty_inspector(&context(dir.path()), &launch(), &params, &runner)
                .await
                .unwrap();
            assert_eq!(runner.argv()[0][5], expected, "{params:?}");
        }
    }

    /// T-GHOST-2 — every failure carries the hint, and every failure is `Err` (upstream's
    /// `isError: true`).
    ///
    /// GUT: drop [`GHOSTTY_FAILURE_HINT`] from any arm and the user is told the inspector failed
    /// with no way to learn that Automation permission is the usual cause — which is the entire
    /// reason `ghostty/actions.ts:52` exists.
    #[tokio::test]
    async fn an_empty_terminal_id_and_a_runner_error_both_carry_the_hint() {
        let dir = tempfile::tempdir().unwrap();

        let empty = open_ghostty_inspector(
            &context(dir.path()),
            &launch(),
            &InspectorParams::default(),
            &FakeRunner::stdout("   \n"),
        )
        .await
        .unwrap_err();
        assert_eq!(
            empty.to_string(),
            format!(
                "Ghostty inspector error: Ghostty returned an empty terminal id. \
                 {GHOSTTY_FAILURE_HINT}"
            )
        );

        let refused = open_ghostty_inspector(
            &context(dir.path()),
            &launch(),
            &InspectorParams::default(),
            &FakeRunner::refusing(HerdrErrorCode::Unavailable),
        )
        .await
        .unwrap_err();
        assert_eq!(
            refused.to_string(),
            format!("Ghostty inspector error: HERDR_UNAVAILABLE. {GHOSTTY_FAILURE_HINT}")
        );

        let timed_out = open_ghostty_inspector(
            &context(dir.path()),
            &launch(),
            &InspectorParams::default(),
            &FakeRunner::refusing(HerdrErrorCode::Timeout),
        )
        .await
        .unwrap_err();
        assert_eq!(
            timed_out.to_string(),
            format!("Ghostty inspector error: TIMEOUT. {GHOSTTY_FAILURE_HINT}")
        );
    }

    /// Node's `execFile` REJECTS a non-zero exit, so upstream never reads a terminal id off a
    /// failed osascript. GUT the exit-code check and a Ghostty that printed its AppleScript error
    /// to stderr and nothing to stdout is reported as *"Opened read-only Ghostty inspector
    /// terminal  for async run …"* — a success sentence for a terminal that does not exist.
    #[tokio::test]
    async fn a_non_zero_exit_is_a_failure_carrying_osascripts_own_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let reply = open_ghostty_inspector(
            &context(dir.path()),
            &launch(),
            &InspectorParams::default(),
            &FakeRunner::failing(CommandOutput {
                stdout: "term-3".to_owned(),
                stderr: "execution error: Not authorized to send Apple events. (-1743)\n"
                    .to_owned(),
                exit_code: Some(1),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(
            reply.to_string(),
            format!(
                "Ghostty inspector error: Command failed: {OSASCRIPT_BIN}\nexecution error: Not \
                 authorized to send Apple events. (-1743). {GHOSTTY_FAILURE_HINT}"
            )
        );

        let signalled = open_ghostty_inspector(
            &context(dir.path()),
            &launch(),
            &InspectorParams::default(),
            &FakeRunner::failing(CommandOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(
            signalled.to_string(),
            format!(
                "Ghostty inspector error: Command failed: {OSASCRIPT_BIN}. {GHOSTTY_FAILURE_HINT}"
            )
        );
    }

    /// The AppleScript is a CONTRACT with Ghostty 1.3, not prose. GUT any line and the script
    /// stops compiling inside osascript, which fails at run time on the user's machine and on no
    /// test but this one.
    #[test]
    fn the_applescript_is_upstreams_byte_for_byte() {
        assert!(GHOSTTY_APPLESCRIPT.starts_with("on run argv\n"));
        assert!(GHOSTTY_APPLESCRIPT.ends_with("end run"));
        assert!(GHOSTTY_APPLESCRIPT.contains("tell application \"Ghostty\""));
        assert!(GHOSTTY_APPLESCRIPT.contains("set launchCommand to item 1 of argv"));
        assert!(GHOSTTY_APPLESCRIPT.contains("set launchCwd to item 2 of argv"));
        assert!(GHOSTTY_APPLESCRIPT.contains("set shouldFocus to item 3 of argv"));
        assert!(GHOSTTY_APPLESCRIPT.contains(
            "split sourceTerminal direction right with configuration surfaceConfiguration"
        ));
        assert!(GHOSTTY_APPLESCRIPT.contains("if shouldFocus is \"true\" then"));
        assert!(GHOSTTY_APPLESCRIPT.contains("return id of newTerminal"));
        assert_eq!(GHOSTTY_APPLESCRIPT.lines().count(), 20);
    }

    /// `maxBuffer` (`:65`). GUT the truncation and a runaway osascript can pin an arbitrary amount
    /// of the extension's memory into a tool result.
    #[test]
    fn captured_output_is_capped_at_upstreams_buffer() {
        let big = "x".repeat(OSASCRIPT_MAX_BUFFER + 500);
        assert_eq!(truncate_to_buffer(&big).len(), OSASCRIPT_MAX_BUFFER);
        assert_eq!(truncate_to_buffer("short"), "short");
        // A multi-byte character straddling the cut is dropped whole, never split.
        let wide = format!("{}é", "x".repeat(OSASCRIPT_MAX_BUFFER - 1));
        assert_eq!(truncate_to_buffer(&wide).len(), OSASCRIPT_MAX_BUFFER - 1);
    }

    /// The real runner is the only code in this module that names a host binary, and it names the
    /// ABSOLUTE path upstream names (`:32`) — a `PATH` lookup would let a shadowed `osascript`
    /// run the launch command.
    #[test]
    fn the_real_runner_targets_the_absolute_osascript_path() {
        assert_eq!(OSASCRIPT_BIN, "/usr/bin/osascript");
        assert_eq!(OSASCRIPT_TIMEOUT, Duration::from_millis(15_000));
        assert_eq!(OSASCRIPT_MAX_BUFFER, 64 * 1024);
    }

    /// The cwd handed to the AppleScript is the dispatcher's already-validated directory, never
    /// the async dir. GUT it to `ctx.target.async_dir` and every Ghostty inspector opens in the
    /// run's artifact directory instead of the project the run is about.
    #[tokio::test]
    async fn the_launch_directory_is_the_trusted_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = context(dir.path());
        ctx.trusted_dir = PathBuf::from("/workspace/project");
        let runner = FakeRunner::stdout("t9");
        open_ghostty_inspector(&ctx, &launch(), &InspectorParams::default(), &runner)
            .await
            .unwrap();
        assert_eq!(runner.argv()[0][4], "/workspace/project");
    }
}
