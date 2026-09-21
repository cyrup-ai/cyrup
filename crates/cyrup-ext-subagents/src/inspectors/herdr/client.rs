//! The herdr seam, as a THIN ADAPTER over [`cyrup_herdr`] — the workspace's one herdr client.
//!
//! # What this module is, and what it deliberately is not
//!
//! Upstream's `herdr/client.ts` (130 lines @v0.68.0) *is* a client: it `spawn`s `herdr`, buffers
//! stdout/stderr, parses the last JSON line, and normalises the error code. **cyrup does none of
//! that here.** `crates/cyrup-herdr` already speaks herdr's newline-delimited JSON socket
//! protocol, typed, with its own framing bounds, timeouts and error vocabulary
//! (`crates/cyrup-herdr/src/lib.rs:1-144`). A second client would be a second thing to keep in
//! sync with herdr v0.9.1, and `inspectors/plugins.rs:50-51` names that outright: *"An
//! implementation that opens its own socket or spawns its own `herdr` process is a defect."*
//!
//! So this module is a **translator**: it takes the argv vector the frozen contract's
//! [`HerdrClient`] trait carries, dispatches it to the matching typed
//! [`cyrup_herdr::HerdrClient`] method, and renders the typed answer back into the JSON envelope
//! shape herdr's own CLI prints — `{"pane": {…}}` for a `PaneInfo`, `{}` for an acknowledgement.
//!
//! # Why the seam stays argv-shaped rather than becoming the typed surface
//!
//! The contract (`inspectors/plugins.rs:52-56`) declares
//! `async fn run(&self, args: &[&str]) -> Result<serde_json::Value, HerdrErrorCode>` and the
//! implementations do not edit the contract. That shape is also what makes the whole subtree
//! testable with no herdr anywhere: a fake records the argv it was handed, and the tests assert
//! the **exact sequence of herdr operations** a verb performs — which is the thing that breaks in
//! production when it drifts, because herdr rejects a flag it does not know. Pinning the typed
//! calls instead would pin Rust structs that cannot drift.
//!
//! # `[CYRUP-EXCEEDS-UPSTREAM]` — what this adapter translates, beside what pi drives
//!
//! **pi drives SIX herdr CLI verbs plus `herdr --version`.** Grepped, in full, at v0.68.0:
//! `pane get` (`herdr/actions.ts:85`, `focus.ts:33`, `project-panes.ts:402`), `pane split`
//! (`actions.ts:103-107`, `project-panes.ts:577-579`), `pane run` (`actions.ts:111`,
//! `project-panes.ts:585`), `pane close` (`actions.ts:113,149`,
//! `project-panes.ts:587,608,654`), `tab focus` (`focus.ts:40`) and `workspace focus`
//! (`focus.ts:44`). herdr v0.9.1 publishes 105 methods (`tmp/herdr/src/api/schema.rs:47-271`).
//!
//! This adapter translates **nine `pane …` argv forms plus `--version`**, which line up against
//! pi's six like this:
//!
//! | argv | herdr method | relation to pi |
//! |---|---|---|
//! | `pane get <id>` | `pane.get` | pi's |
//! | `pane run <id> <cmd>` | `pane.send_input` | pi's |
//! | `pane close <id>` | `pane.close` | pi's |
//! | `pane split … --env/--ratio` | `pane.split` | pi's, plus `--env`: env reaches the pane before its shell parses anything |
//! | `pane focus <id>` | `pane.focus` | **replaces** pi's `tab focus` and `workspace focus` with ONE call that works across both; see [`super::focus`] |
//! | `pane wait-output <id> --match …` | `pane.wait_for_output` | cyrup's: turns a blind `pane run` into a verified launch |
//! | `pane process-info <id>` | `pane.process_info` | cyrup's: a stronger ownership proof than the pane's label `cwd` |
//! | `pane report-agent <id> …` | `pane.report_agent` | cyrup's: the inspector's run shows up in herdr's own sidebar |
//! | `pane report-metadata <id> …` | `pane.report_metadata` | cyrup's: writes the `tokens.summary` key pi's `paneSummary` probes for (see that arm for who else writes it) |
//!
//! So cyrup sends neither of pi's two focus verbs, and the four rows marked "cyrup's" are the
//! whole of the surplus. Each of them carries its reason on the arm that handles it.
//!
//! # The version floor is **pi's**, not herdr's
//!
//! [`supports_raw_panes`] demands ≥ 0.7.5 (`client.ts:118-120`). Two things are greppable about
//! that. First, **herdr has no such capability**: `git grep -in "raw_panes\|rawPanes" d59d060` in
//! `tmp/herdr` is empty, and the only three hits for "raw pane" are test strings using "raw pane
//! id" for herdr's INTERNAL pane id as opposed to its public pane number
//! (`src/app/state.rs:1153,1252`, `src/workspace.rs:1281`) — a different sense of the word, and
//! not a feature, a flag or a version gate. Second, all four methods the floor guards —
//! `pane.split`, `pane.send_input`, `pane.get`, `pane.close` — are present at the pin
//! (`tmp/herdr/src/api/schema.rs:140,198,182,233`).
//!
//! **When they appeared is NOT asserted**, because nothing here can establish it: `tmp/herdr` is
//! one commit (`d59d060`, v0.9.1) with 50 commits of history and no release tags, so "predates
//! 0.7.5" would be a claim with no grep behind it. The floor is kept because its refusal sentence
//! is model-visible and is upstream's stated contract — not because this tree can show herdr
//! imposes it, and not because it can show herdr does not.

use std::collections::BTreeMap;
use std::time::Duration;

use cyrup_herdr::schema::{
    OutputMatch, PaneAgentState, PaneInfo, PaneProcessInfo, PaneProcessInfoParams,
    PaneReportAgentParams, PaneReportMetadataParams, PaneSendInputParams, PaneSplitParams,
    PaneWaitForOutputParams, ReadSource, SplitDirection,
};
use cyrup_herdr::{ApiErrorCode, EnvSource, HerdrError, HerdrPane, ProcessEnv};
use serde_json::{Map, Value, json};

use crate::inspectors::plugins::HerdrClient;
use crate::inspectors::types::HerdrErrorCode;

// =================================================================================================
// Upstream's model-visible sentences
// =================================================================================================

/// `client.ts:55` / `:85`, verbatim. Model-visible through
/// `Herdr inspector error (HERDR_UNAVAILABLE): {this}` and the project-pane equivalent.
pub const HERDR_NOT_INSTALLED: &str =
    "Herdr is not installed or is not on PATH. Install Herdr 0.7.5+ or set HERDR_BIN.";

/// `client.ts:92`'s fallback message, verbatim.
pub const HERDR_COMMAND_FAILED: &str = "Herdr command failed.";

/// The `source` every report this subtree makes is attributed to.
///
/// Distinct from the status bridge's `cyrup:subagents` (`crate::herdr::SOURCE`): herdr scopes
/// reporting authority per `source` (`tmp/herdr/src/app/api/panes.rs:1786-1790`), so an inspector
/// pane reporting under the bridge's name would contend with the bridge for the SAME authority on
/// a different pane's behalf. They are two reporters and they say so.
pub const INSPECTOR_SOURCE: &str = "cyrup:inspector";

// =================================================================================================
// The error pair the contract's `Result<_, HerdrErrorCode>` cannot carry
// =================================================================================================

/// A failed herdr operation: the contract's code, plus the sentence a model reads.
///
/// **This type exists because of a gap in the frozen contract**, and it is reported as one rather
/// than worked around silently. [`HerdrClient::run`] returns `Result<Value, HerdrErrorCode>` — a
/// bare code with no message — while every upstream failure sentence in this subtree is
/// `Herdr inspector error ({code}): {message}` (`herdr/actions.ts:70-72`) or
/// `Herdr project pane error ({code}): {message}` (`project-panes.ts:168-170`), where `{message}`
/// is **herdr's own**, forwarded verbatim by `client.ts:92`.
///
/// With only a code to go on, [`message_for`] reconstructs the sentence from the code and the argv
/// that produced it. Every message this subtree's tests assert is reconstructible that way (they
/// are pi's own literals, keyed on the code), but herdr's *specific* explanation — "pane w1:p2 was
/// closed by the user", say — is lost at the seam. Widening the trait to
/// `Result<Value, HerdrFailure>` is the orchestrator's call; see `contractGaps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrFailure {
    /// The contract's code.
    pub code: HerdrErrorCode,
    /// The sentence to show, reconstructed by [`message_for`].
    pub message: String,
}

impl HerdrFailure {
    /// A failure with an explicit message.
    #[must_use]
    pub fn new(code: HerdrErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Upstream's `Herdr inspector error ({code}): {message}` (`herdr/actions.ts:70-72`).
#[must_use]
pub fn inspector_error_text(failure: &HerdrFailure) -> String {
    format!(
        "Herdr inspector error ({}): {}",
        failure.code.as_str(),
        failure.message
    )
}

/// Reconstruct a failure sentence from a code and the argv that produced it.
///
/// Keyed on the code, because that is all the contract's seam carries. The `HERDR_UNAVAILABLE`
/// row is upstream's literal and is asserted by the not-installed tests; the rest name the
/// operation, which is the actionable half a bare code would lose too.
#[must_use]
pub fn message_for(code: HerdrErrorCode, args: &[&str]) -> String {
    let command = args.join(" ");
    match code {
        HerdrErrorCode::Unavailable => HERDR_NOT_INSTALLED.to_owned(),
        // `client.ts:75`'s literal, with the deadline this adapter actually applied.
        HerdrErrorCode::Timeout => format!(
            "Herdr command '{command}' timed out after {}ms.",
            timeout_for(args).as_millis()
        ),
        HerdrErrorCode::NotFound => {
            format!("Herdr reports no such pane for command '{command}'.")
        }
        HerdrErrorCode::PaneGone => format!("The Herdr pane for command '{command}' is gone."),
        HerdrErrorCode::UnsupportedVersion | HerdrErrorCode::ValidationError => {
            HERDR_COMMAND_FAILED.to_owned()
        }
    }
}

/// Run one herdr operation and attach the sentence its code implies.
///
/// Every caller in this subtree goes through here rather than through
/// [`HerdrClient::run`] directly, so the message reconstruction happens in exactly one place.
///
/// # Errors
/// Whatever the client reported, paired with [`message_for`]'s sentence.
pub async fn call(client: &dyn HerdrClient, args: &[&str]) -> Result<Value, HerdrFailure> {
    client
        .run(args)
        .await
        .map_err(|code| HerdrFailure::new(code, message_for(code, args)))
}

// =================================================================================================
// The version gate — pi's `detectHerdr`, over whatever the seam answers `--version` with
// =================================================================================================

/// A parsed `major.minor.patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HerdrVersion {
    /// Major.
    pub major: u64,
    /// Minor.
    pub minor: u64,
    /// Patch.
    pub patch: u64,
}

/// pi `parseHerdrVersion` (`client.ts:113-116`) — the FIRST `(\d+)\.(\d+)\.(\d+)` anywhere in the
/// text, so `herdr 0.9.1 (abcdef)` parses exactly as `0.9.1` does.
#[must_use]
pub fn parse_herdr_version(value: &str) -> Option<HerdrVersion> {
    // Scan forward over char boundaries and try to anchor the triple at each one. `char_indices`
    // gives exactly the boundaries `str::get` accepts, so no index can ever slice mid-codepoint.
    value
        .char_indices()
        .find_map(|(start, _)| value.get(start..).and_then(version_at))
}

/// Parse `major.minor.patch` anchored at the start of `value`, or `None`.
fn version_at(value: &str) -> Option<HerdrVersion> {
    let (major, rest) = leading_number(value)?;
    let (minor, rest) = leading_number(rest.strip_prefix('.')?)?;
    let (patch, _) = leading_number(rest.strip_prefix('.')?)?;
    Some(HerdrVersion {
        major,
        minor,
        patch,
    })
}

/// The leading run of ASCII digits, and what follows it.
fn leading_number(value: &str) -> Option<(u64, &str)> {
    let end = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    if end == 0 {
        return None;
    }
    let digits = value.get(..end)?;
    let rest = value.get(end..)?;
    digits.parse().ok().map(|number| (number, rest))
}

/// pi `supportsRawPanes` (`client.ts:118-120`), verbatim: `major > 0 || minor > 7 ||
/// (minor === 7 && patch >= 5)`.
///
/// Note the shape: it is a disjunction, not a lexicographic compare, so `0.8.0` passes on the
/// second term and `0.7.5` on the third. Porting it as `>= (0,7,5)` would agree on every input
/// herdr can produce and is still not what upstream wrote.
#[must_use]
pub const fn supports_raw_panes(version: HerdrVersion) -> bool {
    version.major > 0 || version.minor > 7 || (version.minor == 7 && version.patch >= 5)
}

/// What `detectHerdr` hands back on success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedHerdr {
    /// The parsed version.
    pub version: HerdrVersion,
    /// The text it was parsed from — the string that appears in the refusal sentence.
    pub version_text: String,
}

/// pi `detectHerdr` (`client.ts:122-129`).
///
/// # Errors
/// The seam's own failure; `VALIDATION_ERROR` for a version that will not parse
/// (`client.ts:127`); `HERDR_UNSUPPORTED_VERSION` below 0.7.5 (`:128`). Both sentences are
/// upstream's, verbatim.
pub async fn detect_herdr(client: &dyn HerdrClient) -> Result<DetectedHerdr, HerdrFailure> {
    let raw = call(client, &["--version"]).await?;
    // `textOk: true` (`client.ts:123`): a bare string comes back as one, anything else is
    // stringified rather than refused — `client.ts:125`.
    let version_text = match raw {
        Value::String(text) => text,
        other => other.to_string(),
    };
    let Some(version) = parse_herdr_version(&version_text) else {
        return Err(HerdrFailure::new(
            HerdrErrorCode::ValidationError,
            format!("Could not parse the Herdr version from '{version_text}'."),
        ));
    };
    if !supports_raw_panes(version) {
        return Err(HerdrFailure::new(
            HerdrErrorCode::UnsupportedVersion,
            format!(
                "Herdr {version_text} does not support raw inspector panes. Upgrade to Herdr \
                 0.7.5 or newer."
            ),
        ));
    }
    Ok(DetectedHerdr {
        version,
        version_text,
    })
}

// =================================================================================================
// argv helpers, shared with `actions` and `project_panes` so a flag is spelled once
// =================================================================================================

/// The value following `flag`, when the argv carries it.
#[must_use]
pub fn flag_value<'a>(args: &'a [&'a str], flag: &str) -> Option<&'a str> {
    let mut iter = args.iter();
    while let Some(entry) = iter.next() {
        if *entry == flag {
            return iter.next().copied();
        }
    }
    None
}

/// Every value following each occurrence of `flag` — herdr's `--env` is repeatable
/// (`tmp/herdr/src/cli/pane.rs:711-719`).
#[must_use]
pub fn flag_values<'a>(args: &'a [&'a str], flag: &str) -> Vec<&'a str> {
    let mut values = Vec::new();
    let mut iter = args.iter();
    while let Some(entry) = iter.next() {
        if *entry == flag
            && let Some(value) = iter.next()
        {
            values.push(*value);
        }
    }
    values
}

/// pi's per-call deadlines, keyed on the operation rather than passed per call.
///
/// The contract's `run(&self, args)` has no options parameter, so the table upstream spreads
/// across its call sites (`herdr/actions.ts:85,107,111,113,149`; `project-panes.ts:402,579,585,654`;
/// `client.ts:76,123`) lives here instead, in one place. One deliberate collapse: upstream uses
/// 5 s for the *cleanup* `pane close` (`actions.ts:113`) and 10 s for the *requested* one
/// (`:149`); at the argv layer those two are the same three tokens, so both get 10 s — the more
/// patient of the two, because a cleanup close that times out strands a pane in the user's
/// terminal.
#[must_use]
pub fn timeout_for(args: &[&str]) -> Duration {
    match args {
        ["--version"] => Duration::from_secs(3),
        ["pane", "get", ..] | ["pane", "process-info", ..] | ["pane", "focus", ..] => {
            Duration::from_secs(5)
        }
        ["pane", "close", ..] => Duration::from_secs(10),
        ["pane", "wait-output", ..] => flag_value(args, "--timeout")
            .and_then(|ms| ms.parse::<u64>().ok())
            .map_or(Duration::from_secs(15), |ms| {
                Duration::from_millis(ms) + cyrup_herdr::WAIT_GRACE
            }),
        _ => Duration::from_secs(15),
    }
}

// =================================================================================================
// The real adapter
// =================================================================================================

/// The production [`HerdrClient`]: every argv is translated to a typed [`cyrup_herdr`] call.
///
/// Holds a path and a deadline and nothing else — [`cyrup_herdr::HerdrClient`] opens a connection
/// per call (`crates/cyrup-herdr/src/lib.rs:22-26`), so constructing one is free and constructing
/// one eagerly would still not connect. That matters: `available()` must not construct a client
/// (`[AUG — verbs]` §D.1) and this type would not punish it if it did.
#[derive(Debug, Clone)]
pub struct SocketHerdrClient {
    inner: cyrup_herdr::HerdrClient,
}

impl SocketHerdrClient {
    /// A client for the socket of the herdr pane this process runs in.
    #[must_use]
    pub fn for_pane(pane: &HerdrPane) -> Self {
        Self {
            inner: cyrup_herdr::HerdrClient::for_pane(pane),
        }
    }

    /// `None` when this process is not inside a herdr pane — `HERDR_ENV != "1"` or
    /// `HERDR_PANE_ID` empty (`crates/cyrup-herdr/src/env.rs:133-147`), which is byte-for-byte
    /// the condition pi's `available()` checks (`herdr/plugin.ts:13-14`).
    #[must_use]
    pub fn discover(env: &impl EnvSource) -> Option<Self> {
        HerdrPane::discover(env).map(|pane| Self::for_pane(&pane))
    }

    /// [`Self::discover`] against the real process environment.
    #[must_use]
    pub fn from_process_env() -> Option<Self> {
        Self::discover(&ProcessEnv)
    }
}

/// `HerdrError` → the contract's code.
///
/// The `Api` arm goes through [`HerdrErrorCode::from_wire_lossy`] — the contract's own port of
/// `normalizeCode` (`client.ts:35-41`) — over herdr's real spellings, so `pane_not_found` lands on
/// `NOT_FOUND` through the `contains("not_found")` rung and everything else herdr produces
/// (`pane_split_failed`, `pane_send_failed`, `invalid_key`, …) collapses to `VALIDATION_ERROR`,
/// exactly as it does for pi. Every non-`Api` arm is a transport failure with no herdr code at
/// all: `Timeout` is its own, and the rest are `VALIDATION_ERROR` because upstream's own
/// non-zero-exit path is (`client.ts:104`).
#[must_use]
pub fn map_herdr_error(error: &HerdrError) -> HerdrErrorCode {
    match error {
        HerdrError::Unavailable(_) => HerdrErrorCode::Unavailable,
        HerdrError::Timeout { .. } => HerdrErrorCode::Timeout,
        HerdrError::Api { source, .. } => HerdrErrorCode::from_wire_lossy(source.code.as_str()),
        HerdrError::IdMismatch { .. }
        | HerdrError::Closed { .. }
        | HerdrError::Malformed { .. }
        | HerdrError::TooLarge { .. }
        | HerdrError::UnexpectedResult { .. }
        | HerdrError::Io(_) => HerdrErrorCode::ValidationError,
        // [`HerdrError`] is `#[non_exhaustive]` (`crates/cyrup-herdr/src/error.rs:284`), so a
        // future arm must land somewhere. `VALIDATION_ERROR` is the right somewhere: it is where
        // upstream's own catch-all puts an unrecognised failure (`client.ts:104`), and unlike
        // `NOT_FOUND` or `PANE_GONE` it is NOT tolerated by `closeHerdrInspector:150` — so a
        // failure this build cannot name is surfaced rather than silently treated as "the pane was
        // already gone".
        _ => HerdrErrorCode::ValidationError,
    }
}

/// herdr's own CLI envelope for a pane record: `{"pane": {…}}`.
///
/// `print_method_response` (`tmp/herdr/src/cli/runtime.rs:109-111`) prints the whole
/// `ResponseResult`, and pi's client strips `envelope.result` (`client.ts:97-98`), leaving exactly
/// this. Rebuilt field by field rather than `serde_json::to_value`d because
/// [`cyrup_herdr::schema::PaneInfo`] is `Deserialize`-only (`schema/panes.rs:427`) — it is a
/// record herdr sends, not one any client sends.
#[must_use]
pub fn pane_info_json(pane: &PaneInfo) -> Value {
    let mut record = Map::new();
    record.insert("pane_id".into(), json!(pane.pane_id));
    record.insert("terminal_id".into(), json!(pane.terminal_id));
    record.insert("workspace_id".into(), json!(pane.workspace_id));
    record.insert("tab_id".into(), json!(pane.tab_id));
    record.insert("focused".into(), json!(pane.focused));
    insert_opt(&mut record, "cwd", pane.cwd.as_deref());
    insert_opt(
        &mut record,
        "foreground_cwd",
        pane.foreground_cwd.as_deref(),
    );
    insert_opt(&mut record, "label", pane.label.as_deref());
    insert_opt(&mut record, "agent", pane.agent.as_deref());
    insert_opt(&mut record, "title", pane.title.as_deref());
    insert_opt(
        &mut record,
        "terminal_title",
        pane.terminal_title.as_deref(),
    );
    insert_opt(
        &mut record,
        "terminal_title_stripped",
        pane.terminal_title_stripped.as_deref(),
    );
    insert_opt(&mut record, "display_agent", pane.display_agent.as_deref());
    record.insert(
        "agent_status".into(),
        serde_json::to_value(&pane.agent_status).unwrap_or(Value::Null),
    );
    record.insert("state_labels".into(), json!(pane.state_labels));
    record.insert("tokens".into(), json!(pane.tokens));
    record.insert("revision".into(), json!(pane.revision));
    json!({ "pane": Value::Object(record) })
}

/// `{"process": {…}}` — the shape `pane process-info` answers with.
#[must_use]
fn process_info_json(info: &PaneProcessInfo) -> Value {
    let processes: Vec<Value> = info
        .foreground_processes
        .iter()
        .map(|process| {
            json!({
                "pid": process.pid,
                "name": process.name,
                "argv0": process.argv0,
                "cwd": process.cwd,
            })
        })
        .collect();
    json!({
        "process": {
            "pane_id": info.pane_id,
            "shell_pid": info.shell_pid,
            "foreground_process_group_id": info.foreground_process_group_id,
            "tty": info.tty,
            "foreground_processes": processes,
        }
    })
}

/// Insert `key` only when the value is present — herdr omits absent optionals rather than sending
/// `null`, and a consumer probing `typeof record[key] === "string"` must see the same thing.
fn insert_opt(record: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        record.insert(key.to_owned(), json!(value));
    }
}

/// herdr's `AgentStatus` spelling → the `PaneAgentState` a report carries
/// (`crates/cyrup-herdr/src/schema/common.rs:127-135`). Anything else is `Unknown`, which is
/// herdr's own "cannot tell" rather than a guess.
#[must_use]
fn agent_state_from_wire(raw: &str) -> PaneAgentState {
    match raw {
        "idle" => PaneAgentState::Idle,
        "working" => PaneAgentState::Working,
        "blocked" => PaneAgentState::Blocked,
        _ => PaneAgentState::Unknown,
    }
}

#[async_trait::async_trait]
impl HerdrClient for SocketHerdrClient {
    async fn run(&self, args: &[&str]) -> Result<Value, HerdrErrorCode> {
        let client = self.inner.clone().with_timeout(timeout_for(args));
        dispatch(&client, args).await.map_err(|error| {
            let code = map_herdr_error(&error);
            tracing::debug!(?code, argv = ?args, %error, "herdr operation failed");
            code
        })
    }
}

/// argv → one typed [`cyrup_herdr::HerdrClient`] call.
///
/// An argv this adapter does not know is `invalid_request` shaped: herdr itself answers an unknown
/// method that way (`tmp/herdr/src/api/server.rs:177-204`), and an adapter that silently answered
/// `{}` would be the fabricated success `cyrup-herdr` refuses everywhere else.
async fn dispatch(client: &cyrup_herdr::HerdrClient, args: &[&str]) -> Result<Value, HerdrError> {
    match args {
        // `herdr --version`. The socket's `ping` is strictly better than shelling out for it:
        // `tmp/herdr/src/api/server.rs:355-368` answers it without touching the app, so it stays
        // answerable while the UI is busy — which is exactly when an inspector is opened.
        ["--version"] => Ok(Value::String(client.ping().await?.version().to_owned())),

        ["pane", "split", rest @ ..] => {
            let mut params = PaneSplitParams::new(SplitDirection::Right);
            params.cwd = flag_value(args, "--cwd").map(str::to_owned);
            params.focus = rest.contains(&"--focus");
            params.ratio = flag_value(args, "--ratio").and_then(|value| value.parse().ok());
            params.env = env_pairs(args);
            // `--current` means "split whichever pane herdr considers current", which is
            // `PaneSplitParams`' own default (`target_pane_id: None`,
            // `crates/cyrup-herdr/src/schema/panes.rs:66-78`) — so it needs no translation, and
            // the flag's real force is upstream's: it is why `available()` gates on
            // `HERDR_PANE_ID` (`tmp/herdr/src/cli/pane.rs:660-667`).
            Ok(pane_info_json(&client.pane_split(params).await?))
        }

        // `pane run` is `pane.send_input`, not a spawn: herdr TYPES the command into the pane's
        // shell and presses Enter (`tmp/herdr/src/cli/pane.rs:1046-1052`). An `Ok` here means the
        // keys reached a PTY and nothing more — which is precisely why `pane wait-output` below
        // exists.
        ["pane", "run", pane_id, command] => {
            client
                .pane_send_input(PaneSendInputParams::run(*pane_id, *command))
                .await?;
            Ok(json!({}))
        }

        ["pane", "get", pane_id] => Ok(pane_info_json(&client.pane_get(*pane_id).await?)),

        ["pane", "close", pane_id] => {
            client.pane_close(*pane_id).await?;
            Ok(json!({}))
        }

        // `[CYRUP-EXCEEDS-UPSTREAM]` — ONE call, across tabs and workspaces.
        // `handle_pane_focus` (`tmp/herdr/src/app/api/panes.rs:484-500`) delegates to
        // `focus_pane_in_workspace` (`tmp/herdr/src/app/actions.rs:295-322`), whose
        // `switch_workspace_tab` is what makes it cross BOTH; herdr's own test is
        // `api_pane_focus_focuses_direct_target_across_tabs_and_workspaces` (`panes.rs:4152`).
        // See `super::focus` for the two pi refusal sentences this deletes.
        ["pane", "focus", pane_id] => Ok(pane_info_json(&client.pane_focus(*pane_id).await?)),

        // `[CYRUP-EXCEEDS-UPSTREAM]` — a verified launch. `--source recent` is what herdr's own
        // CLI defaults to for this verb (`tmp/herdr/src/cli/pane.rs:1054-1130`).
        ["pane", "wait-output", pane_id, ..] => {
            let text = flag_value(args, "--match").unwrap_or_default();
            let mut params = PaneWaitForOutputParams::new(
                *pane_id,
                ReadSource::Recent,
                OutputMatch::substring(text),
            );
            params.timeout_ms = flag_value(args, "--timeout").and_then(|ms| ms.parse().ok());
            let matched = client.pane_wait_for_output(params).await?;
            Ok(json!({
                "pane_id": matched.pane_id,
                "matched_line": matched.matched_line,
            }))
        }

        // `[CYRUP-EXCEEDS-UPSTREAM]` — the foreground process's real cwd, which `PaneInfo::cwd` is
        // NOT (`socket-api.mdx:750-752` documents it as the pane/workspace cwd used for labels).
        ["pane", "process-info", pane_id] => {
            let params = PaneProcessInfoParams {
                pane_id: Some((*pane_id).to_owned()),
            };
            Ok(process_info_json(&client.pane_process_info(params).await?))
        }

        // `[CYRUP-EXCEEDS-UPSTREAM]` — the inspector's run appears in herdr's sidebar, its rollups
        // and `agent.wait`. `state` "affects waits, notifications, and rollups"
        // (`socket-api.mdx:717-718`). **pi never sends this verb anywhere**:
        // `git grep -in "report_agent\|report-agent" v0.68.0` in `tmp/pi-subagents` is empty, so
        // it is absent from `integrations/herdr-status.ts` (which sends only `report-metadata`,
        // `:206,221`) as well as from `inspectors/`. The only `reportAgent` in that tree is
        // `agents.ts`'s agent-DEFINITION directory report, which has nothing to do with herdr.
        ["pane", "report-agent", pane_id, ..] => {
            let params = PaneReportAgentParams::new(
                *pane_id,
                flag_value(args, "--source").unwrap_or(INSPECTOR_SOURCE),
                flag_value(args, "--agent").unwrap_or_default(),
                agent_state_from_wire(flag_value(args, "--state").unwrap_or("unknown")),
            );
            client.report_agent(params).await?;
            Ok(json!({}))
        }

        // `[CYRUP-EXCEEDS-UPSTREAM]`, and only in WHERE it is sent from.
        //
        // `paneSummary` probes seven keys (`project-panes.ts:323-331`) and `PaneInfo` carries
        // exactly one of them, `tokens` — so `tokens.summary` is the only rung that can ever hit,
        // and only if something called `pane.report_metadata --token summary=…`. That is the
        // careful version `super::project_panes`' module doc §2 states, and this comment used to
        // contradict it by claiming "nothing was ever writing it". **Something is**: pi's own
        // status bridge writes `--token summary=${text}` on every publish
        // (`integrations/herdr-status.ts:221-229` @v0.68.0). What pi does NOT do is write it from
        // `inspectors/` — `project-panes.ts` reads the key and never sends the verb — so a pi
        // project pane running something other than pi renders a dash. cyrup sends it from this
        // subtree as well, which is the delta.
        ["pane", "report-metadata", pane_id, ..] => {
            let mut params = PaneReportMetadataParams::new(
                *pane_id,
                flag_value(args, "--source").unwrap_or(INSPECTOR_SOURCE),
            );
            for token in flag_values(args, "--token") {
                if let Some((key, value)) = token.split_once('=') {
                    params.tokens.insert(key.to_owned(), Some(value.to_owned()));
                }
            }
            client.report_metadata(params).await?;
            Ok(json!({}))
        }

        // There is no `["pane", "read", ..]` arm. `cyrup_herdr::HerdrClient::pane_read` exists
        // and `pane wait-output` uses the same read underneath, but NOTHING in this subtree sends
        // that argv — `grep -rn '"pane", "read"' crates/` finds only this comment — so an arm for
        // it would be an operation the adapter advertises and no verb performs. Add it with the
        // first caller, not before.
        unknown => Err(HerdrError::Api {
            method: "pane",
            source: cyrup_herdr::ApiError {
                code: ApiErrorCode::InvalidRequest,
                message: format!("no herdr operation for argv {unknown:?}"),
            },
        }),
    }
}

/// `--env KEY=VALUE`, repeatable (`tmp/herdr/src/cli/pane.rs:711-719`).
fn env_pairs(args: &[&str]) -> BTreeMap<String, String> {
    flag_values(args, "--env")
        .into_iter()
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::sync::Mutex;

    use super::*;

    /// A seam fake that records argv and replays a script. Every test in this subtree that needs a
    /// herdr uses one of these; none of them needs a herdr.
    #[derive(Default)]
    pub struct FakeHerdrClient {
        calls: Mutex<Vec<Vec<String>>>,
        script: Mutex<Vec<Result<Value, HerdrErrorCode>>>,
    }

    impl FakeHerdrClient {
        pub fn scripted(script: Vec<Result<Value, HerdrErrorCode>>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                script: Mutex::new(script),
            }
        }

        pub fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
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

    /// GUT `parse_herdr_version`'s scan loop (return `None`) and every row goes red.
    #[test]
    fn the_version_parser_finds_the_first_triple_anywhere_in_the_text() {
        assert_eq!(
            parse_herdr_version("0.9.1"),
            Some(HerdrVersion {
                major: 0,
                minor: 9,
                patch: 1
            })
        );
        assert_eq!(
            parse_herdr_version("herdr 0.7.5 (d59d060)"),
            Some(HerdrVersion {
                major: 0,
                minor: 7,
                patch: 5
            })
        );
        assert_eq!(
            parse_herdr_version("v12.0.34-rc1"),
            Some(HerdrVersion {
                major: 12,
                minor: 0,
                patch: 34
            })
        );
        assert_eq!(parse_herdr_version("herdr"), None);
        assert_eq!(parse_herdr_version("0.9"), None);
        // Non-ASCII ahead of the triple must not panic the byte scan.
        assert_eq!(
            parse_herdr_version("herdr — 1.2.3"),
            Some(HerdrVersion {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
    }

    /// GUT `supports_raw_panes` to `true` and the 0.7.4 row goes red; GUT it to
    /// `version.major > 0` and the 0.7.5 row goes red.
    #[test]
    fn the_version_floor_is_pis_three_term_disjunction() {
        let v = |major, minor, patch| HerdrVersion {
            major,
            minor,
            patch,
        };
        assert!(!supports_raw_panes(v(0, 7, 4)));
        assert!(supports_raw_panes(v(0, 7, 5)));
        assert!(supports_raw_panes(v(0, 8, 0)));
        assert!(supports_raw_panes(v(1, 0, 0)));
        assert!(!supports_raw_panes(v(0, 6, 99)));
    }

    /// T-PROJ-4. GUT the `supports_raw_panes` check out of `detect_herdr` and the refusal row goes
    /// red; GUT the `parse_herdr_version` check and the unparseable row goes red.
    #[tokio::test]
    async fn detect_herdr_carries_upstreams_two_refusal_sentences() {
        let low = FakeHerdrClient::scripted(vec![Ok(Value::String("0.7.4".into()))]);
        let failure = detect_herdr(&low).await.unwrap_err();
        assert_eq!(failure.code, HerdrErrorCode::UnsupportedVersion);
        assert_eq!(
            failure.message,
            "Herdr 0.7.4 does not support raw inspector panes. Upgrade to Herdr 0.7.5 or newer."
        );

        let junk = FakeHerdrClient::scripted(vec![Ok(Value::String("herdr".into()))]);
        let failure = detect_herdr(&junk).await.unwrap_err();
        assert_eq!(failure.code, HerdrErrorCode::ValidationError);
        assert_eq!(
            failure.message,
            "Could not parse the Herdr version from 'herdr'."
        );

        for accepted in ["0.7.5", "1.0.0"] {
            let ok = FakeHerdrClient::scripted(vec![Ok(Value::String(accepted.into()))]);
            assert_eq!(detect_herdr(&ok).await.unwrap().version_text, accepted);
            // One call, and it is `--version` (`client.ts:123`). A detect that probed anything
            // else would still produce the sentences above while costing a second round trip on
            // a box that has no herdr at all.
            assert_eq!(ok.calls(), vec![vec!["--version".to_owned()]]);
        }
    }

    /// T-PROJ-3's sentence, at the layer that produces it. GUT `message_for`'s `Unavailable` arm
    /// to `HERDR_COMMAND_FAILED` and this goes red.
    #[tokio::test]
    async fn an_unavailable_herdr_carries_upstreams_install_sentence() {
        let none = FakeHerdrClient::scripted(vec![Err(HerdrErrorCode::Unavailable)]);
        let failure = detect_herdr(&none).await.unwrap_err();
        assert_eq!(failure.code, HerdrErrorCode::Unavailable);
        assert_eq!(failure.message, HERDR_NOT_INSTALLED);
        assert_eq!(
            inspector_error_text(&failure),
            "Herdr inspector error (HERDR_UNAVAILABLE): Herdr is not installed or is not on PATH. \
             Install Herdr 0.7.5+ or set HERDR_BIN."
        );
    }

    /// GUT `timeout_for`'s `wait-output` arm to the default and this goes red — a wait whose
    /// deadline is shorter than herdr's own turns every successful long wait into a client
    /// timeout.
    #[test]
    fn the_deadline_table_is_pis_per_call_one() {
        assert_eq!(timeout_for(&["--version"]), Duration::from_secs(3));
        assert_eq!(timeout_for(&["pane", "get", "p1"]), Duration::from_secs(5));
        assert_eq!(
            timeout_for(&["pane", "close", "p1"]),
            Duration::from_secs(10)
        );
        assert_eq!(
            timeout_for(&["pane", "split", "--current"]),
            Duration::from_secs(15)
        );
        assert_eq!(
            timeout_for(&["pane", "wait-output", "p1", "--timeout", "5000"]),
            Duration::from_millis(5000) + cyrup_herdr::WAIT_GRACE
        );
    }

    /// GUT `flag_values` to return only the first match and the two-env row goes red — a split
    /// that silently drops the second `--env` is a pane missing half its environment.
    #[test]
    fn the_argv_helpers_read_repeated_flags() {
        let args = [
            "pane", "split", "--env", "A=1", "--env", "B=2", "--cwd", "/w",
        ];
        assert_eq!(flag_value(&args, "--cwd"), Some("/w"));
        assert_eq!(flag_value(&args, "--nope"), None);
        assert_eq!(flag_values(&args, "--env"), vec!["A=1", "B=2"]);
        let pairs = env_pairs(&args);
        assert_eq!(pairs.get("A").map(String::as_str), Some("1"));
        assert_eq!(pairs.get("B").map(String::as_str), Some("2"));
        // A malformed pair is dropped, not sent: herdr answers `invalid_env` for one
        // (`crates/cyrup-herdr/src/client.rs`'s `pane_split` doc).
        assert!(env_pairs(&["--env", "NOEQUALS"]).is_empty());
    }

    /// The contract's own `from_wire_lossy` over herdr's REAL codes
    /// (`tmp/herdr/src/app/api/panes.rs:1987-1990`, `:103`, `:1830`). GUT `map_herdr_error`'s
    /// `Api` arm to `ValidationError` and the `pane_not_found` row goes red, which is the one row
    /// that decides whether a closed pane is tolerated or reported as a failure.
    #[test]
    fn herdr_error_codes_normalise_the_way_pi_normalises_them() {
        let api = |code: ApiErrorCode| HerdrError::Api {
            method: "pane.get",
            source: cyrup_herdr::ApiError {
                code,
                message: String::new(),
            },
        };
        assert_eq!(
            map_herdr_error(&api(ApiErrorCode::PaneNotFound)),
            HerdrErrorCode::NotFound
        );
        assert_eq!(
            map_herdr_error(&api(ApiErrorCode::PaneSplitFailed)),
            HerdrErrorCode::ValidationError
        );
        assert_eq!(
            map_herdr_error(&api(ApiErrorCode::Timeout)),
            HerdrErrorCode::Timeout
        );
        assert_eq!(
            map_herdr_error(&HerdrError::Timeout {
                method: "pane.get",
                timeout: Duration::from_secs(5)
            }),
            HerdrErrorCode::Timeout
        );
        assert_eq!(
            map_herdr_error(&HerdrError::Closed { method: "pane.get" }),
            HerdrErrorCode::ValidationError
        );
    }

    /// `SocketHerdrClient::discover` is the gate, and it is pi's gate. GUT it to
    /// `Some(Self::…)` unconditionally and this goes red — which matters because a client
    /// constructed outside a pane would try to connect to a socket that is not there, on a box
    /// where the honest answer is "no inspector host".
    #[test]
    fn no_client_is_constructed_outside_a_herdr_pane() {
        let mut env = std::collections::HashMap::new();
        assert!(SocketHerdrClient::discover(&env).is_none());
        env.insert("HERDR_ENV".to_owned(), "1".to_owned());
        assert!(SocketHerdrClient::discover(&env).is_none());
        env.insert("HERDR_PANE_ID".to_owned(), String::new());
        assert!(SocketHerdrClient::discover(&env).is_none());
        env.insert("HERDR_PANE_ID".to_owned(), "w1:p1".to_owned());
        assert!(SocketHerdrClient::discover(&env).is_some());
    }
}
