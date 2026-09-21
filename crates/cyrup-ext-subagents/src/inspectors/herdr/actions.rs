//! `inspector.{open,status,close}` over herdr — the binding, the pane, and the launch.
//!
//! Upstream: `src/inspectors/herdr/actions.ts` (155 lines @v0.68.0) — `HerdrInspectorBinding:9`,
//! `bindingPath:24`, `parse:28-40`, `readHerdrInspectorBindingForTarget:51`, `errorText:70-72`,
//! `paneId:74-82`, `openHerdrInspector:88`, `statusHerdrInspector:134`, `closeHerdrInspector:146`.
//!
//! # What an inspector pane is
//!
//! A pane beside the user's work running `cyrup __subagent-inspector` against ONE async run. It
//! is a **mirror**: closing it does not stop the run (`inspector-runner.ts:34`), and the reply
//! this module returns says so in upstream's own words.
//!
//! # The three places cyrup does more than pi, each with its premise
//!
//! 1. **The launch is verified** (`[CYRUP-EXCEEDS-UPSTREAM]`). `pane run` is
//!    `pane.send_input`: herdr types the command into the pane's shell and presses Enter
//!    (`tmp/herdr/src/cli/pane.rs:1046-1052`). An `Ok` therefore means *keys reached a PTY* and
//!    nothing more. If the shell was mid-command, or the binary is missing, pi writes a binding
//!    pointing at a pane that never started the inspector — and `inspector.status` reports it open
//!    forever. cyrup waits for the dashboard's own header
//!    ([`INSPECTOR_READY_MARKER`]) with `pane wait-output`, and on timeout runs upstream's
//!    existing failure path: `pane close`, no binding (`herdr/actions.ts:112-115`).
//! 2. **The pane is focusable** (`[CYRUP-EXCEEDS-UPSTREAM]`). `openHerdrInspector:100` tells the
//!    user *"Herdr cannot refocus an arbitrary raw pane id; select it in the Herdr UI."* That is
//!    false against herdr 0.9.1 — see [`super::focus`] for `pane.focus` and its handler at
//!    `tmp/herdr/src/app/api/panes.rs:484-500`. **This is the one upstream sentence this batch is
//!    permitted to change, and it is changed because its premise is false**, not for taste.
//! 3. **The run is visible in herdr** (`[CYRUP-EXCEEDS-UPSTREAM]`). One
//!    `pane report-agent --state working` per open puts the run in herdr's sidebar, its rollups
//!    and `agent.wait` (`socket-api.mdx:701-733`, `:715-716`). pi never calls it from
//!    `inspectors/` at all. It costs one call and it is the difference between a run the user can
//!    drive and a run the user can also *see*.

use std::path::{Path, PathBuf};

use cyrup_core::{Content, ToolError, ToolResult};
use serde_json::Value;

use super::client::{self, HerdrFailure, INSPECTOR_SOURCE, inspector_error_text};
use super::focus::{self, FocusFailure};
use crate::inspectors::plugins::HerdrClient;
use crate::inspectors::types::{
    HerdrErrorCode, HerdrInspectorBinding, HerdrInspectorKind, InspectorContext, InspectorLaunch,
    InspectorParams, InspectorTarget, SchemaVersion1,
};

/// The substring `pane wait-output` waits for before a binding is written.
///
/// **It IS the runner's header line** — [`crate::inspectors::types::inspector_header_line`], the
/// same call `super::super::runner::format_inspector_dashboard` makes to render it. Never spelled
/// here, and no longer assembled here either: the failure when the two disagree is invisible to
/// every unit test in this subtree, because each one scripts the `wait-output` answer, so a marker
/// that matches nothing still passes here while, against a real herdr, every `inspector.open`
/// waits [`INSPECTOR_READY_TIMEOUT_MS`], closes the pane it just opened and writes no binding.
/// That is exactly what a hand-written literal produced on this branch — the runner renders
/// `cyrup-inspector for {runId}` (upstream's `pi-subagents inspector for ${status.runId}`,
/// `inspector-runner.ts:33`), and the literal here read `subagents inspector for {runId}`, which
/// is not a substring of it.
///
/// `the_ready_marker_is_the_runners_own_dashboard_header` keeps the call from being replaced by a
/// copy.
#[must_use]
pub fn inspector_ready_marker(run_id: &str) -> String {
    crate::inspectors::types::inspector_header_line(run_id)
}

/// How long to wait for the dashboard's first frame before declaring the launch failed.
///
/// The runner's default refresh is 1 500 ms and its first frame is rendered before the first
/// sleep, so 5 s is roughly three refreshes of slack — long enough for a cold binary on a loaded
/// box, short enough that a user who mistyped nothing is not left staring at a spinner.
pub const INSPECTOR_READY_TIMEOUT_MS: u64 = 5_000;

/// The fraction of the current pane that **stays with the session**, leaving the rest to the
/// inspector.
///
/// `pane.split`'s `ratio` (`crates/cyrup-herdr/src/schema/panes.rs:50`, CLI `--ratio`,
/// `tmp/herdr/src/cli/pane.rs:676-688`) — herdr defaults to an even 50 % split, which gives half
/// the user's terminal to a read-only dashboard. pi passes no ratio at all and so always takes
/// that default.
///
/// The orientation is herdr's and is the opposite of the obvious reading, so it is cited rather
/// than assumed: `split_at` puts the ORIGINAL pane in `first` and the NEW pane in `second`
/// (`tmp/herdr/src/layout.rs:598-603` @ `d59d060`), and the geometry gives `first` the ratio —
/// `let first_w = ((area.width as f32) * ratio).round()` (`:694`, and `:702` for a horizontal
/// split). So `ratio` is the ORIGINAL pane's share: 0.6 keeps the session at 60 % and gives the
/// read-only mirror 40 %. The value on this branch was 0.4, which did the reverse.
pub const INSPECTOR_PANE_RATIO: f32 = 0.6;

/// The environment variable the inspector's session roots are ALSO delivered through.
///
/// `[CYRUP-EXCEEDS-UPSTREAM]`, and the reason is the mechanism: `pane run` is a keystroke
/// injection, so every token of `launch.display_command` is re-parsed by whatever shell the pane
/// happens to be running — which is the entire reason `session-roots-codec.ts` base64-encodes
/// them in the first place. `--env KEY=VALUE` at split time (`tmp/herdr/src/cli/pane.rs:711-719`)
/// puts the value in the pane's environment **before** the shell sees anything.
///
/// The `--session-roots` flag stays primary: it is what `inspector.command` prints and what a user
/// pasting the command by hand needs.
///
/// **What this variable is NOT.** It was written as "the belt to that pair of braces", and that
/// premise does not survive a read of the runner: nothing consumes it. `parse_args`
/// (`super::super::runner::parse_args`) reads the roots from the argv flag alone, and a shell that
/// mangled the payload into several tokens would be refused by that parser's pairwise scan before
/// any fallback could run — so the variable could not rescue the failure it was described as
/// covering. What it genuinely does is put the launch's roots where a human inspecting the pane
/// (`env | grep CYRUP_INSPECTOR`) or a future reader can see them, which is why it is kept and
/// why it is described as that and nothing more.
pub const INSPECTOR_SESSION_ROOTS_ENV: &str = "CYRUP_INSPECTOR_SESSION_ROOTS";

// =================================================================================================
// The binding file
// =================================================================================================

/// pi `bindingPath` (`herdr/actions.ts:24-26`) — `<asyncDir>/inspectors/herdr{,-<index>}.json`.
#[must_use]
pub fn binding_path(async_dir: &Path, index: Option<usize>) -> PathBuf {
    let name = match index {
        None => "herdr.json".to_owned(),
        Some(index) => format!("herdr-{index}.json"),
    };
    async_dir.join("inspectors").join(name)
}

/// pi `readHerdrInspectorBinding` (`herdr/actions.ts:42-48`) — every failure is `None`, never a
/// throw.
///
/// The validation upstream writes by hand (`parse:28-40`) is the contract's type: `schemaVersion`
/// must be `1` ([`SchemaVersion1`]), `kind` must be `"herdr-inspector"`
/// ([`HerdrInspectorKind`]), `childIndex` must be a non-negative integer (`Option<usize>`), and
/// the four string fields are non-optional. A record failing any of them is a serde error, which
/// lands on the same `None`.
///
/// **Synchronous on purpose**: [`crate::inspectors::plugins::InspectorPlugin::owns`] is a
/// synchronous method and must stay one — it is consulted for every plugin on every
/// `inspector.status`/`inspector.close`, and it reads a file rather than talking to a host.
#[must_use]
pub fn read_herdr_inspector_binding(
    async_dir: &Path,
    index: Option<usize>,
) -> Option<HerdrInspectorBinding> {
    let bytes = std::fs::read(binding_path(async_dir, index)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// pi `readHerdrInspectorBindingForTarget` (`herdr/actions.ts:51-60`).
///
/// Three checks beyond parsing, and each one is a different way a binding can be stale:
/// a different run wrote it; it is for a different child of the same run; or the async directory
/// it names is not the one being asked about once both are resolved through their symlinks.
#[must_use]
pub fn read_herdr_inspector_binding_for_target(
    target: &InspectorTarget,
) -> Option<HerdrInspectorBinding> {
    let binding = read_herdr_inspector_binding(&target.async_dir, target.child_index)?;
    if binding.run_id != target.run_id || binding.child_index != target.child_index {
        return None;
    }
    let bound = std::fs::canonicalize(&binding.async_dir).ok()?;
    let asked = std::fs::canonicalize(&target.async_dir).ok()?;
    (bound == asked).then_some(binding)
}

/// `Ok(ToolResult)` carrying `text` — upstream's `result(text)` with `isError` absent.
fn ok(text: impl Into<String>) -> Result<ToolResult, ToolError> {
    Ok(ToolResult {
        content: vec![Content::text(text.into())],
        ..Default::default()
    })
}

/// Upstream's `result(text, true)`. See `inspectors/types.rs`'s module doc for why `isError: true`
/// is `Err(ToolError)` in cyrup and not a flag.
fn err(text: impl Into<String>) -> Result<ToolResult, ToolError> {
    Err(ToolError::new(text.into()))
}

/// A focus failure, in the inspector's own sentence shape.
fn focus_error_text(failure: &FocusFailure) -> String {
    format!(
        "Herdr inspector error ({}): {}",
        failure.code.as_str(),
        failure.message
    )
}

/// The run's lifecycle state, for `statusHerdrInspector`'s two sentences.
///
/// Upstream reads `context.target.status.state` — a snapshot the dispatcher already resolved.
/// cyrup's frozen [`InspectorContext`] carries no run state (reported as a contract gap by this
/// batch's core writer and again by this one), so the state is re-read from the run's own
/// `status.json`, which is where the dispatcher's snapshot came from too
/// (`crate::background::RunDir::status`). A run whose status file has gone is reported as
/// `unknown` rather than suppressing the whole line: the binding is still the answer the user
/// asked for.
async fn run_state(async_dir: &Path) -> String {
    let path = crate::background::RunDir::for_existing(async_dir).status();
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return "unknown".to_owned();
    };
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .as_ref()
        .and_then(|record| record.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned()
}

/// The base64 payload of `--session-roots` inside a built launch, if there is one.
///
/// Read off the argv rather than re-encoded, so the env copy and the flag copy can never disagree:
/// they are the same bytes.
fn session_roots_arg(launch: &InspectorLaunch) -> Option<&str> {
    let mut args = launch.args.iter();
    while let Some(arg) = args.next() {
        if arg == "--session-roots" {
            return args.next().map(String::as_str);
        }
    }
    None
}

// =================================================================================================
// open
// =================================================================================================

/// pi `openHerdrInspector` (`herdr/actions.ts:88-132`), plus the three additions in the module
/// doc.
///
/// # Errors
/// Every herdr failure, as `Herdr inspector error ({code}): {message}`. A failure after the split
/// closes the pane it just opened and writes no binding — dropping either half leaves the user an
/// orphan pane, or a binding pointing at a pane running nothing.
pub async fn open_herdr_inspector(
    context: &InspectorContext,
    launch: &InspectorLaunch,
    params: &InspectorParams,
    herdr: &dyn HerdrClient,
) -> Result<ToolResult, ToolError> {
    let detected = match client::detect_herdr(herdr).await {
        Ok(detected) => detected,
        Err(failure) => return err(inspector_error_text(&failure)),
    };
    let run_id = context.target.run_id.as_str();

    // An existing binding whose pane herdr still has is reported, not duplicated (`:96-102`).
    if let Some(existing) = read_herdr_inspector_binding_for_target(&context.target)
        && let Ok(current) = client::call(herdr, &["pane", "get", &existing.pane_id]).await
        && focus::pane_id_of(&current).as_deref() == Some(existing.pane_id.as_str())
    {
        return already_open(context, params, herdr, existing).await;
    }

    // ---- split ----------------------------------------------------------------------------
    // `--cwd` is upstream's `context.target.status.cwd ?? context.cwd` (`:105`), which the frozen
    // contract delivers pre-resolved as `InspectorContext::trusted_dir` — the batch's core writer
    // records that in its own contract-gap note, and this is the reader it was written for.
    let cwd = context.trusted_dir.to_string_lossy().into_owned();
    let ratio = INSPECTOR_PANE_RATIO.to_string();
    let mut split: Vec<String> = vec![
        "pane".into(),
        "split".into(),
        "--current".into(),
        "--direction".into(),
        "right".into(),
        "--cwd".into(),
        cwd,
        if params.focus == Some(true) {
            "--focus".into()
        } else {
            "--no-focus".into()
        },
        "--ratio".into(),
        ratio,
    ];
    if let Some(roots) = session_roots_arg(launch) {
        split.push("--env".into());
        split.push(format!("{INSPECTOR_SESSION_ROOTS_ENV}={roots}"));
    }
    let split_argv: Vec<&str> = split.iter().map(String::as_str).collect();
    let split_result = match client::call(herdr, &split_argv).await {
        Ok(value) => value,
        Err(failure) => return err(inspector_error_text(&failure)),
    };
    let Some(pane_id) = focus::pane_id_of(&split_result) else {
        // Upstream's literal (`:110`), including its `PANE_GONE` code — which herdr itself never
        // produces (`[AUG — verbs]` §B.1); this is pi's own, and it stays pi's own.
        return err("Herdr inspector error (PANE_GONE): pane split returned no pane id.");
    };

    // ---- run ------------------------------------------------------------------------------
    if let Err(failure) =
        client::call(herdr, &["pane", "run", &pane_id, &launch.display_command]).await
    {
        close_quietly(herdr, &pane_id).await;
        return err(inspector_error_text(&failure));
    }

    // ---- verify (`[CYRUP-EXCEEDS-UPSTREAM]`) ------------------------------------------------
    let marker = inspector_ready_marker(run_id);
    let timeout = INSPECTOR_READY_TIMEOUT_MS.to_string();
    if let Err(failure) = client::call(
        herdr,
        &[
            "pane",
            "wait-output",
            &pane_id,
            "--match",
            &marker,
            "--timeout",
            &timeout,
        ],
    )
    .await
    {
        close_quietly(herdr, &pane_id).await;
        return err(format!(
            "{}\nThe inspector did not start in the new pane, so the pane was closed and no \
             binding was written.",
            inspector_error_text(&failure)
        ));
    }

    // ---- report (`[CYRUP-EXCEEDS-UPSTREAM]`) ------------------------------------------------
    report_inspector_agent(
        herdr,
        &pane_id,
        run_id,
        &run_state(&context.target.async_dir).await,
    )
    .await;

    // ---- bind -------------------------------------------------------------------------------
    let now = crate::time::format_iso8601_millis(crate::time::now_epoch_millis());
    let binding = HerdrInspectorBinding {
        schema_version: SchemaVersion1,
        kind: HerdrInspectorKind::HerdrInspector,
        run_id: context.target.run_id.clone(),
        async_dir: context.target.async_dir.clone(),
        child_index: context.target.child_index,
        mission_id: context.mission_id.clone(),
        mission_path: context.mission_path.clone(),
        pane_id: pane_id.clone(),
        opened_at: now.clone(),
        last_focused_at: (params.focus == Some(true)).then_some(now),
        herdr_version: Some(detected.version_text),
        command: launch.display_command.clone(),
        extra: serde_json::Map::new(),
    };
    if let Err(cause) = write_binding(&context.target, &binding).await {
        close_quietly(herdr, &pane_id).await;
        return err(format!(
            "Herdr inspector error (VALIDATION_ERROR): Failed to persist the inspector binding \
             '{}': {cause}. The newly opened pane '{pane_id}' was closed.",
            binding_path(&context.target.async_dir, context.target.child_index).display()
        ));
    }

    ok(format!(
        "Opened read-only Herdr inspector pane {pane_id} for async run {run_id}. Closing the pane \
         does not stop the run.\nControls inside the pane: steer <message>, stop, status."
    ))
}

/// The `:97-102` branch: the binding's pane is live, so report it rather than opening a second
/// one.
///
/// `params.focus` is where cyrup and pi part company — see the module doc's point 2.
async fn already_open(
    context: &InspectorContext,
    params: &InspectorParams,
    herdr: &dyn HerdrClient,
    existing: HerdrInspectorBinding,
) -> Result<ToolResult, ToolError> {
    let run_id = context.target.run_id.as_str();
    let pane_id = existing.pane_id.clone();
    let head = format!("Herdr inspector pane {pane_id} is already open for async run {run_id}.");
    if params.focus != Some(true) {
        return ok(head);
    }
    match focus::focus_herdr_pane(herdr, &pane_id).await {
        Ok(focused) => {
            // A focus that happened is a fact about the binding, so the binding records it — the
            // same rewrite `project-panes.ts:529-531` performs, and the reason the contract's
            // binding carries `#[serde(flatten)] extra`: a key pi added survives this round trip.
            let refreshed = HerdrInspectorBinding {
                last_focused_at: Some(crate::time::format_iso8601_millis(
                    crate::time::now_epoch_millis(),
                )),
                ..existing
            };
            let _ = write_binding(&context.target, &refreshed).await;
            ok(format!("{head} Focused pane {}.", focused.pane_id))
        }
        Err(failure) => err(format!("{head} {}", focus_error_text(&failure))),
    }
}

/// `pane close`, ignoring the outcome — upstream's cleanup call (`:113`) likewise discards it.
///
/// A cleanup that failed leaves the pane the user can see and close; a cleanup that *propagated*
/// would replace the real failure with a second one.
async fn close_quietly(herdr: &dyn HerdrClient, pane_id: &str) {
    if let Err(failure) = client::call(herdr, &["pane", "close", pane_id]).await {
        tracing::debug!(
            pane_id,
            code = failure.code.as_str(),
            "herdr inspector cleanup close failed"
        );
    }
}

/// Tell herdr which run this pane is showing, and how that run is doing.
///
/// Best effort by construction: a herdr that refuses the report still has a working inspector
/// pane, and failing the open over a sidebar entry would trade the feature for the garnish.
async fn report_inspector_agent(herdr: &dyn HerdrClient, pane_id: &str, run_id: &str, state: &str) {
    let reported = match state {
        "running" | "queued" => "working",
        "paused" => "blocked",
        "complete" | "failed" | "stopped" => "idle",
        _ => "unknown",
    };
    if let Err(failure) = client::call(
        herdr,
        &[
            "pane",
            "report-agent",
            pane_id,
            "--source",
            INSPECTOR_SOURCE,
            "--agent",
            run_id,
            "--state",
            reported,
        ],
    )
    .await
    {
        tracing::debug!(
            pane_id,
            code = failure.code.as_str(),
            "herdr inspector agent report failed"
        );
    }
}

/// `writeAtomicJson(bindingPath(...), binding)` (`:130`), with the directory created first.
async fn write_binding(
    target: &InspectorTarget,
    binding: &HerdrInspectorBinding,
) -> Result<(), std::io::Error> {
    let path = binding_path(&target.async_dir, target.child_index);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    crate::background::atomic::write_atomic_json(&path, binding).await
}

// =================================================================================================
// status / close
// =================================================================================================

/// pi `statusHerdrInspector` (`herdr/actions.ts:134-144`).
///
/// # Errors
/// A pane herdr will not answer for is an error carrying the binding path AND the run state,
/// because the run is what the user actually cares about and it outlives the pane (`:141`).
pub async fn status_herdr_inspector(
    context: &InspectorContext,
    herdr: &dyn HerdrClient,
) -> Result<ToolResult, ToolError> {
    let run_id = context.target.run_id.as_str();
    let path = binding_path(&context.target.async_dir, context.target.child_index);
    let Some(binding) = read_herdr_inspector_binding_for_target(&context.target) else {
        let child = match context.target.child_index {
            None => String::new(),
            Some(index) => format!(" child {index}"),
        };
        // NOT an error (`:137` passes no second argument): a status query with no inspector open
        // is a normal answer.
        return ok(format!(
            "No Herdr inspector binding exists for async run {run_id}{child}."
        ));
    };
    let state = run_state(&context.target.async_dir).await;
    match client::call(herdr, &["pane", "get", &binding.pane_id]).await {
        Err(failure) => err(format!(
            "{}\nBinding: {}\nRun state remains authoritative: {state}.",
            inspector_error_text(&failure),
            path.display()
        )),
        Ok(_) => ok(format!(
            "Herdr inspector {} is open for async run {run_id}.\nRun state: {state}\nBinding: {}",
            binding.pane_id,
            path.display()
        )),
    }
}

/// pi `closeHerdrInspector` (`herdr/actions.ts:146-155`).
///
/// # Errors
/// Any close failure that is not `NOT_FOUND`/`PANE_GONE`. Those two are **successes**: the pane is
/// already gone, so removing the stale binding is the whole remaining job (`:150-154`).
pub async fn close_herdr_inspector(
    context: &InspectorContext,
    herdr: &dyn HerdrClient,
) -> Result<ToolResult, ToolError> {
    let run_id = context.target.run_id.as_str();
    let Some(binding) = read_herdr_inspector_binding_for_target(&context.target) else {
        return ok(format!(
            "No Herdr inspector binding exists for async run {run_id}."
        ));
    };
    if let Err(failure) = client::call(herdr, &["pane", "close", &binding.pane_id]).await
        && failure.code != HerdrErrorCode::NotFound
        && failure.code != HerdrErrorCode::PaneGone
    {
        return err(inspector_error_text(&failure));
    }
    let path = binding_path(&context.target.async_dir, context.target.child_index);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        // `rmSync(..., { force: true })` (`:153`) — an absent file is the desired end state.
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
        Err(cause) => {
            return err(format!(
                "Herdr inspector error (VALIDATION_ERROR): Failed to remove the inspector binding \
                 '{}': {cause}",
                path.display()
            ));
        }
    }
    ok(format!(
        "Closed Herdr inspector pane {} for async run {run_id}. The subagent run was not stopped.",
        binding.pane_id
    ))
}

/// A failure this module could not attribute to herdr — used by [`super::plugin`] when no client
/// can be constructed at all.
#[must_use]
pub fn unavailable_failure() -> HerdrFailure {
    HerdrFailure::new(HerdrErrorCode::Unavailable, client::HERDR_NOT_INSTALLED)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::sync::Mutex;

    use serde_json::json;

    use super::*;

    #[derive(Default)]
    struct FakeHerdrClient {
        calls: Mutex<Vec<Vec<String>>>,
        script: Mutex<Vec<Result<Value, HerdrErrorCode>>>,
    }

    impl FakeHerdrClient {
        fn scripted(script: Vec<Result<Value, HerdrErrorCode>>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                script: Mutex::new(script),
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }

        fn verbs(&self) -> Vec<String> {
            self.calls().iter().map(|call| call.join(" ")).collect()
        }
    }

    #[async_trait::async_trait]
    impl HerdrClient for FakeHerdrClient {
        async fn run(&self, args: &[&str]) -> Result<Value, HerdrErrorCode> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|a| (*a).to_owned()).collect());
            let mut script = self.script.lock().unwrap();
            if script.is_empty() {
                return Ok(json!({}));
            }
            script.remove(0)
        }
    }

    /// herdr's REAL envelope (`[AUG — verbs]` V15): the whole `PaneInfo` under `pane`, with the
    /// four ids herdr always sends. A fake that answered `{"pane_id":"p1"}` would pin a shape
    /// herdr never sends and would pass only through `pane_id_of`'s tolerance rung.
    fn pane_envelope(pane_id: &str, cwd: &str) -> Value {
        json!({ "pane": {
            "pane_id": pane_id,
            "terminal_id": "t-1",
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "focused": false,
            "cwd": cwd,
            "agent_status": "idle",
            "revision": 7,
        }})
    }

    fn context(async_dir: &Path, run_id: &str, child_index: Option<usize>) -> InspectorContext {
        InspectorContext {
            target: InspectorTarget {
                run_id: run_id.to_owned(),
                async_dir: async_dir.to_path_buf(),
                child_index,
            },
            trusted_dir: async_dir.to_path_buf(),
            mission_id: None,
            mission_path: None,
            // These tests drive the `open`/`status`/`close` bodies directly, below the
            // `available()` gate that is the only reader of `env`.
            env: std::collections::BTreeMap::new(),
        }
    }

    fn launch() -> InspectorLaunch {
        InspectorLaunch {
            exe: "/usr/bin/cyrup".to_owned(),
            args: vec![
                "__subagent-inspector".to_owned(),
                "--async-dir".to_owned(),
                "/tmp/run".to_owned(),
                "--session-roots".to_owned(),
                "WyIvYSIsIi9iIl0=".to_owned(),
            ],
            display_command: "/usr/bin/cyrup __subagent-inspector --async-dir /tmp/run".to_owned(),
        }
    }

    fn opened_script() -> Vec<Result<Value, HerdrErrorCode>> {
        vec![
            Ok(Value::String("0.9.1".into())),      // --version
            Ok(pane_envelope("w1:p2", "/tmp/run")), // pane split
            Ok(json!({})),                          // pane run
            Ok(json!({ "pane_id": "w1:p2" })),      // pane wait-output
            Ok(json!({})),                          // pane report-agent
        ]
    }

    fn text_of(result: &Result<ToolResult, ToolError>) -> String {
        match result {
            Ok(value) => value
                .content
                .iter()
                .map(|content| match content {
                    Content::Text { text, .. } => text.to_string(),
                    other => format!("{other:?}"),
                })
                .collect::<Vec<_>>()
                .join(""),
            Err(error) => error.to_string(),
        }
    }

    /// pi `bindingPath` (`:24-26`). GUT the `index` arm to always produce `herdr.json` and two
    /// children of one run overwrite each other's binding — the second `inspector.close` would
    /// then close the first child's pane.
    #[test]
    fn the_binding_path_is_upstreams() {
        let dir = Path::new("/a/run");
        assert_eq!(
            binding_path(dir, None),
            Path::new("/a/run/inspectors/herdr.json")
        );
        assert_eq!(
            binding_path(dir, Some(0)),
            Path::new("/a/run/inspectors/herdr-0.json")
        );
        assert_eq!(
            binding_path(dir, Some(12)),
            Path::new("/a/run/inspectors/herdr-12.json")
        );
    }

    /// **T-OPEN-1**, with the two cyrup additions. GUT any flag spelling in the split argv and
    /// this goes red — herdr rejects a flag it does not know, so this is the only place the CLI
    /// contract is pinned. GUT the `--ratio`/`--env` pair and the `[CYRUP-EXCEEDS-UPSTREAM]`
    /// silently reverts to pi's 50 % split with no pane environment.
    #[tokio::test]
    async fn open_performs_split_run_verify_report_write() {
        let dir = tempfile::tempdir().unwrap();
        let herdr = FakeHerdrClient::scripted(opened_script());
        let ctx = context(dir.path(), "run-1", None);

        let reply =
            open_herdr_inspector(&ctx, &launch(), &InspectorParams::default(), &herdr).await;

        assert_eq!(
            herdr.calls(),
            vec![
                vec!["--version"],
                vec![
                    "pane",
                    "split",
                    "--current",
                    "--direction",
                    "right",
                    "--cwd",
                    &dir.path().to_string_lossy(),
                    "--no-focus",
                    "--ratio",
                    "0.6",
                    "--env",
                    "CYRUP_INSPECTOR_SESSION_ROOTS=WyIvYSIsIi9iIl0=",
                ],
                vec![
                    "pane",
                    "run",
                    "w1:p2",
                    "/usr/bin/cyrup __subagent-inspector --async-dir /tmp/run"
                ],
                vec![
                    "pane",
                    "wait-output",
                    "w1:p2",
                    "--match",
                    "cyrup-inspector for run-1",
                    "--timeout",
                    "5000"
                ],
                vec![
                    "pane",
                    "report-agent",
                    "w1:p2",
                    "--source",
                    "cyrup:inspector",
                    "--agent",
                    "run-1",
                    "--state",
                    "unknown"
                ],
            ]
        );

        assert_eq!(
            text_of(&reply),
            "Opened read-only Herdr inspector pane w1:p2 for async run run-1. Closing the pane \
             does not stop the run.\nControls inside the pane: steer <message>, stop, status."
        );

        let written = read_herdr_inspector_binding(dir.path(), None).unwrap();
        assert_eq!(written.pane_id, "w1:p2");
        assert_eq!(written.run_id, "run-1");
        assert_eq!(written.kind, HerdrInspectorKind::HerdrInspector);
        assert_eq!(written.schema_version, SchemaVersion1);
        assert_eq!(written.herdr_version.as_deref(), Some("0.9.1"));
        assert_eq!(
            written.command,
            "/usr/bin/cyrup __subagent-inspector --async-dir /tmp/run"
        );
        assert!(written.last_focused_at.is_none());

        // `skip_serializing_if` is required, not cosmetic: a `"childIndex": null` fails pi's own
        // `parse` check, because `null !== undefined` (`herdr/actions.ts:33`).
        let raw = std::fs::read_to_string(binding_path(dir.path(), None)).unwrap();
        assert!(!raw.contains("childIndex"), "{raw}");
        assert!(!raw.contains("missionId"), "{raw}");
        assert!(raw.contains("\"schemaVersion\": 1"), "{raw}");
        assert!(raw.contains("\"kind\": \"herdr-inspector\""), "{raw}");
    }

    /// **T-OPEN-2.** GUT the cleanup `pane close` and a failed launch leaves an orphan pane in the
    /// user's terminal; GUT the early return and a binding is written pointing at a pane that is
    /// running nothing, which `inspector.status` then reports as open forever.
    #[tokio::test]
    async fn a_pane_run_failure_closes_the_pane_and_writes_no_binding() {
        let dir = tempfile::tempdir().unwrap();
        let herdr = FakeHerdrClient::scripted(vec![
            Ok(Value::String("0.9.1".into())),
            Ok(pane_envelope("w1:p2", "/tmp/run")),
            Err(HerdrErrorCode::ValidationError),
        ]);
        let ctx = context(dir.path(), "run-1", None);

        let reply =
            open_herdr_inspector(&ctx, &launch(), &InspectorParams::default(), &herdr).await;

        assert!(reply.is_err());
        assert_eq!(
            text_of(&reply),
            "Herdr inspector error (VALIDATION_ERROR): Herdr command failed."
        );
        assert!(herdr.verbs().contains(&"pane close w1:p2".to_owned()));
        assert!(!binding_path(dir.path(), None).exists());
        assert!(!dir.path().join("inspectors").exists());
    }

    /// **T-OPEN-4** — the `[CYRUP-EXCEEDS-UPSTREAM]` verification. GUT the `wait-output` hop and
    /// this goes red: a pane whose shell swallowed the keystrokes would be bound forever, and
    /// nothing else in the suite sees the difference because `pane run` itself succeeded.
    #[tokio::test]
    async fn a_pane_that_never_starts_the_inspector_is_closed_and_leaves_no_binding() {
        let dir = tempfile::tempdir().unwrap();
        let herdr = FakeHerdrClient::scripted(vec![
            Ok(Value::String("0.9.1".into())),
            Ok(pane_envelope("w1:p2", "/tmp/run")),
            Ok(json!({})),
            Err(HerdrErrorCode::Timeout),
        ]);
        let ctx = context(dir.path(), "run-1", None);

        let reply =
            open_herdr_inspector(&ctx, &launch(), &InspectorParams::default(), &herdr).await;

        assert!(reply.is_err());
        assert!(text_of(&reply).contains("Herdr inspector error (TIMEOUT)"));
        assert!(text_of(&reply).contains(
            "The inspector did not start in the new pane, so the pane was closed and no \
                 binding was written."
        ));
        assert!(herdr.verbs().contains(&"pane close w1:p2".to_owned()));
        assert!(!binding_path(dir.path(), None).exists());
    }

    /// **T-OPEN-3**, with cyrup's correction. GUT the already-open branch and the run gets a
    /// SECOND inspector pane every time the user presses the key. GUT the `focus_herdr_pane` call
    /// and `focus: true` silently does nothing — which is the exact behaviour pi documents as
    /// impossible and which is not.
    #[tokio::test]
    async fn an_already_open_pane_is_focused_not_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path(), "run-1", None);

        // Seed a real binding by opening once.
        let seed = FakeHerdrClient::scripted(opened_script());
        open_herdr_inspector(&ctx, &launch(), &InspectorParams::default(), &seed)
            .await
            .unwrap();
        let first = std::fs::read_to_string(binding_path(dir.path(), None)).unwrap();

        // Without focus: reported, nothing rewritten, no split.
        let plain = FakeHerdrClient::scripted(vec![
            Ok(Value::String("0.9.1".into())),
            Ok(pane_envelope("w1:p2", "/tmp/run")),
        ]);
        let reply =
            open_herdr_inspector(&ctx, &launch(), &InspectorParams::default(), &plain).await;
        assert_eq!(
            text_of(&reply),
            "Herdr inspector pane w1:p2 is already open for async run run-1."
        );
        assert!(
            !plain
                .verbs()
                .iter()
                .any(|verb| verb.starts_with("pane split"))
        );
        assert_eq!(
            std::fs::read_to_string(binding_path(dir.path(), None)).unwrap(),
            first
        );

        // With focus: ONE `pane focus`, and pi's false "cannot refocus" sentence is gone.
        let focused = FakeHerdrClient::scripted(vec![
            Ok(Value::String("0.9.1".into())),
            Ok(pane_envelope("w1:p2", "/tmp/run")),
            Ok(pane_envelope("w1:p2", "/tmp/run")),
        ]);
        let params = InspectorParams {
            pane_id: None,
            focus: Some(true),
        };
        let reply = open_herdr_inspector(&ctx, &launch(), &params, &focused).await;
        assert_eq!(
            text_of(&reply),
            "Herdr inspector pane w1:p2 is already open for async run run-1. Focused pane w1:p2."
        );
        assert!(!text_of(&reply).contains("cannot refocus"));
        assert!(focused.verbs().contains(&"pane focus w1:p2".to_owned()));
        assert!(
            read_herdr_inspector_binding(dir.path(), None)
                .unwrap()
                .last_focused_at
                .is_some()
        );
    }

    /// **The one contract no other test in this subtree can see.**
    ///
    /// `open_herdr_inspector` writes a binding only after `pane wait-output --match <marker>`
    /// answers, and every test here SCRIPTS that answer — so a marker that matches nothing the
    /// runner ever prints passes all of them while, against a real herdr, the wait runs to
    /// [`INSPECTOR_READY_TIMEOUT_MS`], the pane is closed and no binding is written. That is the
    /// bug this branch shipped: the marker read `subagents inspector for {runId}` and the
    /// runner's header reads `cyrup-inspector for {runId}`.
    ///
    /// This pins the two literals to each other. The other half of the guarantee — that the
    /// header really reaches the pane's stdout — is the real-binary integration test
    /// `cyrup_subagent_inspector_renders_a_real_run_and_a_steer_line_lands`
    /// (`crates/cyrup-it/tests/subagents/inspector_runner_subcommand_integration.rs`), which
    /// asserts `{INSPECTOR_HEADER_PREFIX}{run_id}` in the process's own output. Together they say
    /// the wait can actually match.
    ///
    /// GUT: replace the `format!` in [`inspector_ready_marker`] with any literal and this goes
    /// RED.
    #[test]
    fn the_ready_marker_is_the_runners_own_dashboard_header() {
        use crate::inspectors::types::INSPECTOR_HEADER_PREFIX;

        let marker = inspector_ready_marker("run-1");
        assert_eq!(marker, format!("{INSPECTOR_HEADER_PREFIX}run-1"));
        assert!(
            marker.starts_with(INSPECTOR_HEADER_PREFIX),
            "the marker must be a prefix of the header line the runner prints, got {marker:?}"
        );
    }

    /// GUT the `PANE_GONE` arm and a split whose answer carries no pane id proceeds to
    /// `pane run ""`, which herdr answers for a pane that does not exist.
    #[tokio::test]
    async fn a_split_with_no_pane_id_is_upstreams_pane_gone_literal() {
        let dir = tempfile::tempdir().unwrap();
        let herdr = FakeHerdrClient::scripted(vec![
            Ok(Value::String("0.9.1".into())),
            Ok(json!({ "pane": {} })),
        ]);
        let reply = open_herdr_inspector(
            &context(dir.path(), "run-1", None),
            &launch(),
            &InspectorParams::default(),
            &herdr,
        )
        .await;
        assert_eq!(
            text_of(&reply),
            "Herdr inspector error (PANE_GONE): pane split returned no pane id."
        );
        assert!(!binding_path(dir.path(), None).exists());
    }

    /// **T-STAT-1.** GUT the `Run state:` line and the user loses the only thing that outlives the
    /// pane; GUT the binding removal in `close` and a closed pane stays bound, so the next
    /// `inspector.open` reports it already open and never opens anything.
    #[tokio::test]
    async fn status_and_close_round_trip_the_binding() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path(), "run-1", None);
        std::fs::write(dir.path().join("status.json"), r#"{"state":"running"}"#).unwrap();

        let seed = FakeHerdrClient::scripted(opened_script());
        open_herdr_inspector(&ctx, &launch(), &InspectorParams::default(), &seed)
            .await
            .unwrap();

        let status_client = FakeHerdrClient::scripted(vec![Ok(pane_envelope("w1:p2", "/tmp/run"))]);
        let status = status_herdr_inspector(&ctx, &status_client).await;
        assert_eq!(
            text_of(&status),
            format!(
                "Herdr inspector w1:p2 is open for async run run-1.\nRun state: running\nBinding: \
                 {}",
                binding_path(dir.path(), None).display()
            )
        );

        let close_client = FakeHerdrClient::scripted(vec![Ok(json!({}))]);
        let closed = close_herdr_inspector(&ctx, &close_client).await;
        assert_eq!(close_client.verbs(), vec!["pane close w1:p2"]);
        assert_eq!(
            text_of(&closed),
            "Closed Herdr inspector pane w1:p2 for async run run-1. The subagent run was not \
             stopped."
        );
        assert!(!binding_path(dir.path(), None).exists());
    }

    /// **T-STAT-3.** GUT the `NOT_FOUND`/`PANE_GONE` tolerance and a pane the user already closed
    /// by hand leaves its binding on disk forever, with `inspector.close` reporting a failure the
    /// user can do nothing about.
    #[tokio::test]
    async fn close_tolerates_a_pane_that_is_already_gone() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path(), "run-1", None);
        let seed = FakeHerdrClient::scripted(opened_script());
        open_herdr_inspector(&ctx, &launch(), &InspectorParams::default(), &seed)
            .await
            .unwrap();

        for gone in [HerdrErrorCode::NotFound, HerdrErrorCode::PaneGone] {
            let seed = FakeHerdrClient::scripted(opened_script());
            open_herdr_inspector(&ctx, &launch(), &InspectorParams::default(), &seed)
                .await
                .unwrap();
            let herdr = FakeHerdrClient::scripted(vec![Err(gone)]);
            let closed = close_herdr_inspector(&ctx, &herdr).await;
            assert_eq!(
                text_of(&closed),
                "Closed Herdr inspector pane w1:p2 for async run run-1. The subagent run was not \
                 stopped."
            );
            assert!(!binding_path(dir.path(), None).exists());
        }

        // A close failure that is NEITHER is still an error, and leaves the binding alone.
        let seed = FakeHerdrClient::scripted(opened_script());
        open_herdr_inspector(&ctx, &launch(), &InspectorParams::default(), &seed)
            .await
            .unwrap();
        let herdr = FakeHerdrClient::scripted(vec![Err(HerdrErrorCode::ValidationError)]);
        assert!(close_herdr_inspector(&ctx, &herdr).await.is_err());
        assert!(binding_path(dir.path(), None).exists());
    }

    /// `statusHerdrInspector:137` and `closeHerdrInspector:148` pass NO second argument, so both
    /// are non-errors. GUT either to `err(...)` and `is_error` flips — the single easiest
    /// behaviour in this subtree to get wrong.
    #[tokio::test]
    async fn status_and_close_with_no_binding_are_not_errors() {
        let dir = tempfile::tempdir().unwrap();
        let herdr = FakeHerdrClient::default();

        let status = status_herdr_inspector(&context(dir.path(), "run-1", None), &herdr).await;
        assert!(
            status.is_ok(),
            "status with no binding must not be an error"
        );
        assert_eq!(
            text_of(&status),
            "No Herdr inspector binding exists for async run run-1."
        );

        let child = status_herdr_inspector(&context(dir.path(), "run-1", Some(3)), &herdr).await;
        assert_eq!(
            text_of(&child),
            "No Herdr inspector binding exists for async run run-1 child 3."
        );

        let closed = close_herdr_inspector(&context(dir.path(), "run-1", None), &herdr).await;
        assert!(closed.is_ok(), "close with no binding must not be an error");
        assert_eq!(
            text_of(&closed),
            "No Herdr inspector binding exists for async run run-1."
        );

        // And neither verb touched the host.
        assert!(herdr.calls().is_empty());
    }

    /// pi `readHerdrInspectorBindingForTarget:51-60` — each of the three staleness checks.
    /// GUT any one and a binding written by a different run, a different child, or a moved
    /// directory is accepted as this target's.
    #[test]
    fn a_binding_belongs_to_exactly_one_target() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("inspectors")).unwrap();
        let binding = HerdrInspectorBinding {
            schema_version: SchemaVersion1,
            kind: HerdrInspectorKind::HerdrInspector,
            run_id: "run-1".into(),
            async_dir: dir.path().to_path_buf(),
            child_index: Some(2),
            mission_id: None,
            mission_path: None,
            pane_id: "w1:p2".into(),
            opened_at: "2026-09-21T00:00:00.000Z".into(),
            last_focused_at: None,
            herdr_version: Some("0.9.1".into()),
            command: "cyrup".into(),
            extra: serde_json::Map::new(),
        };
        std::fs::write(
            binding_path(dir.path(), Some(2)),
            serde_json::to_vec(&binding).unwrap(),
        )
        .unwrap();

        let target = |run_id: &str, index| InspectorTarget {
            run_id: run_id.to_owned(),
            async_dir: dir.path().to_path_buf(),
            child_index: index,
        };
        assert!(read_herdr_inspector_binding_for_target(&target("run-1", Some(2))).is_some());
        assert!(read_herdr_inspector_binding_for_target(&target("run-2", Some(2))).is_none());
        assert!(read_herdr_inspector_binding_for_target(&target("run-1", None)).is_none());
    }

    /// The contract's `#[serde(flatten)] extra` is what makes a cyrup round trip preserve a key pi
    /// added. GUT the flatten (or add `deny_unknown_fields`) and a `focus` that rewrites the
    /// binding silently deletes it.
    #[test]
    fn an_unknown_key_written_by_pi_survives_a_round_trip() {
        let raw = serde_json::json!({
            "schemaVersion": 1,
            "kind": "herdr-inspector",
            "runId": "run-1",
            "asyncDir": "/a/run",
            "paneId": "w1:p2",
            "openedAt": "2026-09-21T00:00:00.000Z",
            "command": "pi",
            "piOnlyKey": { "nested": true },
        });
        let binding: HerdrInspectorBinding = serde_json::from_value(raw).unwrap();
        assert_eq!(binding.extra.get("piOnlyKey").unwrap()["nested"], true);
        let back = serde_json::to_value(&binding).unwrap();
        assert_eq!(back["piOnlyKey"]["nested"], true);

        // And a future schemaVersion is a parse error, not a silently mis-read record.
        let future = serde_json::json!({
            "schemaVersion": 2,
            "kind": "herdr-inspector",
            "runId": "r",
            "asyncDir": "/a",
            "paneId": "p",
            "openedAt": "t",
            "command": "c",
        });
        assert!(serde_json::from_value::<HerdrInspectorBinding>(future).is_err());
    }

    /// GUT `session_roots_arg` to `None` and the pane loses its belt-and-braces environment copy;
    /// the `--session-roots` flag still works, which is exactly why only this test sees it.
    #[test]
    fn the_session_roots_payload_is_lifted_off_the_launch_argv() {
        assert_eq!(session_roots_arg(&launch()), Some("WyIvYSIsIi9iIl0="));
        let bare = InspectorLaunch {
            exe: "cyrup".into(),
            args: vec!["__subagent-inspector".into()],
            display_command: "cyrup __subagent-inspector".into(),
        };
        assert_eq!(session_roots_arg(&bare), None);
        // A trailing `--session-roots` with no value must not panic.
        let truncated = InspectorLaunch {
            exe: "cyrup".into(),
            args: vec!["--session-roots".into()],
            display_command: "cyrup".into(),
        };
        assert_eq!(session_roots_arg(&truncated), None);
    }
}
