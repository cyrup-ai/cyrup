//! SUBA-025 — the `subagent` tool's description is RESOLVED, not a hard-coded constant: the Rust
//! port of `pi-subagents/src/extension/tool-description.ts` (in-baseline at v0.34.0 and present at
//! every later tag through v0.47.1).
//!
//! # The three surfaces this closes
//!
//! 1. **`toolDescriptionMode`** (pi `resolveToolDescriptionMode`, `tool-description.ts:104`
//!    @v0.34.0 / `:68` @v0.43.0) — `"full"` | `"compact"` | `"custom"`, defaulting to `full`. An
//!    invalid value WARNS and falls back to `full`; it never fails config load, which is why
//!    [`crate::registration::SubagentExtensionConfig::tool_description_mode`] carries the raw JSON
//!    rather than a parsed enum.
//! 2. **The file override** — a `subagent-tool-description.md` in the project config dir or the
//!    agent dir, capped at [`CUSTOM_TOOL_DESCRIPTION_MAX_BYTES`], with `{{placeholder}}`
//!    interpolation (pi `loadCustomToolDescription`/`renderCustomTemplate`, `:143`/`:121`).
//! 3. **The mandatory safety-guidance appender** (pi `withMandatorySafetyGuidance`, `:180`) — the
//!    reason this item's severity was raised. A deployment may replace the entire description; it
//!    may NOT drop [`SUBAGENT_SAFETY_GUIDANCE`], and if the custom text embeds the block anywhere
//!    the appender lifts it back out and re-appends it exactly once at the end.
//!
//! Before this module, `SubagentTool::description` was a `&'static str` chosen only by
//! registration mode, so all three surfaces were absent: a deployment could neither trim the (long)
//! description to save context nor steer the orchestrator with project-specific text, and there was
//! no safety-guidance guarantee because there was no custom-description path at all.
//!
//! # Why this landed against v0.34.0's constants
//!
//! The gap-analysis row declined this item TWICE on the premise that landing it "requires AUTHORING
//! cyrup-specific compact and safety-guidance blocks — inventing model-facing text". **That premise
//! is refuted, and the refutation is mechanical**: `tool-description.ts` is present at v0.34.0
//! (`git cat-file -e v0.34.0:src/extension/tool-description.ts`), and its
//! `FULL_SUBAGENT_TOOL_DESCRIPTION` (`:17-66`) is the text `extension.rs`'s
//! `SUBAGENT_TOOL_DESCRIPTION` was already ported from. Its COMPACT and SAFETY siblings are written
//! around the same SINGLE/PARALLEL/CHAIN surface, so both come across as upstream constants with no
//! text authored here. Only v0.43.0's rewrite of those constants is `workflowScript`-shaped, and
//! v0.43.0 is not the revision cyrup's own full description comes from.
//!
//! The `full` arm therefore stays `extension.rs`'s existing constant — it is passed IN
//! ([`build_subagent_tool_description`]'s `full` argument) rather than duplicated here, so there is
//! exactly one full description in the crate and this module cannot drift from it.
//!
//! # Warnings
//!
//! pi's `warn` is an injectable callback defaulting to `console.warn("[pi-subagents] " + message)`
//! (`:94`). Here every function takes a `&mut Vec<String>` sink and the caller decides what to do
//! with it — [`crate::extension`] emits `tracing::warn!` under this crate's `[cyrup-subagents]`
//! prefix (the convention `watchdog/change_signature.rs` already uses). A sink rather than a
//! callback because it is what makes the warning texts directly assertable in-crate, which is how
//! all eight of upstream's are pinned below.

use std::path::{Path, PathBuf};

/// pi `SUBAGENT_SAFETY_GUIDANCE` (`src/extension/tool-description.ts:9-15` @v0.34.0), BYTE-IDENTICAL.
///
/// This is the block [`with_mandatory_safety_guidance`] guarantees survives a custom description:
/// a deployment may replace every other word the orchestrator reads about delegation, but not
/// these six bullets.
///
/// **Why the v0.34.0 tag and not v0.43.0** — and why this needed no authored text, contrary to the
/// gap-analysis row that declined it twice. v0.43.0 rewrote this constant around `workflowScript`
/// (`tool-description.ts:9-15` @v0.43.0: *"omit action for workflowScript execution"*), a
/// `node:vm` JS sandbox this crate does not implement (`extension.rs`'s own note, and SUBA-016).
/// v0.34.0's text is written around SINGLE/PARALLEL/CHAIN and names
/// `list/get/models/create/update/delete/status/interrupt/resume/append-step/doctor` — every one of
/// which is in [`crate::extension`]'s advertised action list today — so it describes cyrup's actual
/// surface exactly. v0.34.0 is also the tag cyrup's own `SUBAGENT_TOOL_DESCRIPTION` was ported
/// from (`FULL_SUBAGENT_TOOL_DESCRIPTION`, `tool-description.ts:17-66` @v0.34.0), so the two
/// constants come from ONE upstream revision rather than two.
pub const SUBAGENT_SAFETY_GUIDANCE: &str = r#"SAFETY-CRITICAL SUBAGENT GUIDANCE:
• Use { action: "list" } before execution and only run executable/non-disabled agents or chains.
• Keep execution and management separate: omit action for SINGLE/PARALLEL/CHAIN execution; use action only for list/get/models/create/update/delete/status/interrupt/resume/append-step/doctor.
• Async/background runs: launch with async:true only when work can proceed independently. Do not sleep or poll status just to wait; if this turn must block, use the wait tool. Otherwise continue useful work or respond and let completion notifications arrive.
• Child-safety boundary: ordinary child subagents are not orchestrators and must not run subagents. Only explicitly configured fanout children may use the child-safe subagent tool, still bounded by depth/session limits.
• Writing/review safety: keep one writer for the same cwd/worktree. Use fresh-context read-only reviewers/validators for independent review, then have the parent synthesize and apply fixes as the sole writer unless an isolated worktree was intentionally requested.
• Artifacts/status essentials: chain outputs live under {chain_dir}; async runs expose asyncId/asyncDir with status.json, events.jsonl, output logs, and status via { action: "status", id }. Include output paths and residual risks when reporting results."#;

/// pi `COMPACT_SUBAGENT_TOOL_DESCRIPTION` (`src/extension/tool-description.ts:68-88` @v0.34.0),
/// verbatim except for ONE deleted line.
///
/// `[CYRUP-DELTA]` — upstream `COMPACT_SUBAGENT_TOOL_DESCRIPTION` (`tool-description.ts:80`
/// @v0.34.0) carries the bullet *"• Opt-in schedule actions: schedule, schedule-list,
/// schedule-status, schedule-cancel. Schedule only explicit delayed runs the user asked for."*.
/// It is dropped here because `scheduledRuns` is unported (SUBA-016, blocked on `workflowScript`),
/// so advertising those four verbs would reproduce exactly the advertise-vs-refuse defect SUBA-046
/// was filed for: a model that reads the description and calls `schedule` lands on the
/// unknown-action arm. This is a DELETION of an upstream line naming an unported subsystem, not
/// authored text — the same convention `extension.rs`'s `SUBAGENT_ACTIONS` already documents
/// ("This is cyrup's CURRENT surface, not upstream's full 53"), and
/// [`the_compact_description_advertises_no_verb_cyrup_cannot_dispatch`] enforces it mechanically
/// rather than by assertion.
///
/// Every other verb this text names — `list`, `get`, `models`, `create`, `update`, `delete`,
/// `eject`, `disable`, `enable`, `reset`, `doctor`, `status`, `interrupt`, `resume`, `steer`,
/// `append-step` — plus `status view:"fleet"` / `view:"transcript"` and the `wait` tool, is live in
/// cyrup today.
pub const COMPACT_SUBAGENT_TOOL_DESCRIPTION: &str = r#"Delegate to subagents or manage definitions. Use exactly one mode per call.

EXECUTE:
• Before execution, call { action: "list" }; run only executable/non-disabled configured agents/chains.
• SINGLE {agent, task?}; PARALLEL {tasks:[{agent,task,count?,output?,reads?,progress?}], concurrency?, worktree?}; CHAIN {chain:[{agent,task?},{parallel:[...]}]}.
• context can be "fresh" or "fork"; omitted uses each agent defaultContext, otherwise fresh. timeoutMs/maxRuntimeMs apply to foreground and async/background runs.
• Chain templates may use {task}, {previous}, {chain_dir}, and named outputs. Parallel worktree isolation requires a clean git repo.
• If list shows proactive skill subagent suggestions, use a small fresh-context fanout only when the task is broad enough.

MANAGE / CONTROL:
• Use action without execution fields: list, get, models, create, update, delete, eject, disable, enable, reset, doctor.
• Async control actions: status, interrupt, resume, steer, append-step. Use status view:"fleet" for active-run overview, view:"transcript" to tail child output, and steer for non-terminal live guidance. Use id/runId prefixes carefully; use index for a specific child.

ASYNC / WAIT:
• async:true detaches background work. Do not sleep or poll just to wait; use the wait tool only when this turn must block. Otherwise continue useful work or respond and let completion notifications arrive.
• Status and artifacts live under asyncId/asyncDir with status.json, events.jsonl, output logs, session files, and { action:"status", id:"..." }.

SAFETY:
• Ordinary child subagents are not orchestrators and must not run subagents. Only explicit fanout children may use child-safe subagent, still bounded by depth/session limits.
• Keep one writer per cwd/worktree. Use fresh read-only review/validation fanout, then synthesize and apply fixes from the parent unless isolated worktrees were intentionally requested."#;

/// pi `CUSTOM_TOOL_DESCRIPTION_FILE` (`tool-description.ts:6`).
pub const CUSTOM_TOOL_DESCRIPTION_FILE: &str = "subagent-tool-description.md";

/// pi `CUSTOM_TOOL_DESCRIPTION_MAX_BYTES = 50 * 1024` (`tool-description.ts:7`) — 51200. The cap is
/// on the file's SIZE ON DISK, checked before it is read, so an enormous override cannot be pulled
/// into memory at all.
pub const CUSTOM_TOOL_DESCRIPTION_MAX_BYTES: u64 = 50 * 1024;

/// pi `ToolDescriptionMode` (`shared/types.ts`) — the three accepted values of
/// `subagents.toolDescriptionMode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolDescriptionMode {
    /// pi `"full"` — the complete multi-section description. Upstream's default.
    #[default]
    Full,
    /// pi `"compact"` — [`COMPACT_SUBAGENT_TOOL_DESCRIPTION`], for deployments trading discovery
    /// detail for context budget.
    Compact,
    /// pi `"custom"` — load [`CUSTOM_TOOL_DESCRIPTION_FILE`], falling back to `full` (with a
    /// warning) when it is missing or unusable.
    Custom,
}

impl ToolDescriptionMode {
    /// pi `isToolDescriptionMode` (`tool-description.ts:90`).
    #[must_use]
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "full" => Some(Self::Full),
            "compact" => Some(Self::Compact),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// pi `ToolDescriptionOptions` (`tool-description.ts:98-102`) minus the `warn` member, which is a
/// sink argument here (see the module doc).
///
/// Both directories are RESOLVED at construction rather than read lazily from the environment on
/// every call, because upstream resolves them once per `buildSubagentToolDescription` too and
/// because a description is built exactly once, at tool registration.
#[derive(Clone, Debug)]
pub struct ToolDescriptionOptions {
    /// pi `options.cwd ?? process.cwd()` — the project root whose `.cyrup/` is searched first.
    pub cwd: PathBuf,
    /// pi `options.agentDir ?? getAgentDir()` — the user-scope directory searched second.
    pub agent_dir: PathBuf,
}

impl ToolDescriptionOptions {
    /// Upstream's defaults: this session's cwd plus `getAgentDir()`.
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            agent_dir: agent_dir(),
        }
    }
}

/// `getAgentDir()` (`shared/utils.ts:72-77`) — `$CYRUP_AGENT_DIR`/`$PI_CODING_AGENT_DIR` with `~`
/// expansion, else `<home>/.cyrup/agent`. Byte-identical to `watchdog/settings.rs`'s `agent_dir`
/// and `exec/mcp_direct_tools.rs`'s `resolve_agent_dir`, this crate's two existing ports of the
/// same upstream function.
fn agent_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let configured = std::env::var("CYRUP_AGENT_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            std::env::var("PI_CODING_AGENT_DIR")
                .ok()
                .filter(|v| !v.is_empty())
        });
    match configured {
        Some(v) if v == "~" => home,
        Some(v) if v.starts_with("~/") => home.join(v.get(2..).unwrap_or("")),
        Some(v) => PathBuf::from(v),
        None => home.join(".cyrup").join("agent"),
    }
}

/// `getProjectConfigDir(projectRoot)` (`shared/utils.ts:68-70`) — `<root>/.cyrup` (upstream
/// `<root>/.pi`).
fn project_config_dir(project_root: &Path) -> PathBuf {
    project_root.join(".cyrup")
}

/// pi `resolveToolDescriptionMode` (`tool-description.ts:104-110` @v0.34.0).
///
/// `configured` is the RAW `subagents.toolDescriptionMode` JSON value: omitted (`None`) is `full`,
/// a recognised string is itself, and anything else warns with upstream's verbatim text — including
/// its `JSON.stringify` rendering of the offending value — and degrades to `full`. It deliberately
/// cannot fail: an unreadable knob must not take the whole extension down at load.
#[must_use]
pub fn resolve_tool_description_mode(
    configured: Option<&serde_json::Value>,
    warnings: &mut Vec<String>,
) -> ToolDescriptionMode {
    let Some(value) = configured.filter(|v| !v.is_null()) else {
        return ToolDescriptionMode::Full;
    };
    if let Some(mode) = value.as_str().and_then(ToolDescriptionMode::from_str) {
        return mode;
    }
    warnings.push(format!(
        "Ignoring invalid toolDescriptionMode {}; expected \"full\", \"compact\", or \"custom\".",
        serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
    ));
    ToolDescriptionMode::Full
}

/// pi `customDescriptionPaths` (`tool-description.ts:112-119`) — project scope FIRST, then user
/// scope, so a repository's own override wins over the operator's personal one.
#[must_use]
fn custom_description_paths(options: &ToolDescriptionOptions) -> [PathBuf; 2] {
    [
        project_config_dir(&options.cwd).join(CUSTOM_TOOL_DESCRIPTION_FILE),
        options.agent_dir.join(CUSTOM_TOOL_DESCRIPTION_FILE),
    ]
}

/// pi `renderCustomTemplate` (`tool-description.ts:121-141`): substitute `{{name}}` for the eight
/// supported variables, warning on (and preserving) an unknown one.
///
/// pi's `template.replace(/\{\{(\w+)\}\}/g, …)` is hand-scanned here rather than pulled through a
/// regex dependency; `\w` is `[A-Za-z0-9_]`, and a `{{…}}` whose body is not entirely `\w` is not a
/// match at all upstream, so it is left alone WITHOUT a warning — the same as here.
#[must_use]
fn render_custom_template(
    template: &str,
    full: &str,
    options: &ToolDescriptionOptions,
    warnings: &mut Vec<String>,
) -> String {
    let project_config_dir = project_config_dir(&options.cwd);
    let variable = |name: &str| -> Option<String> {
        match name {
            "fullDescription" | "full" => Some(full.to_string()),
            "compactDescription" | "compact" => {
                Some(COMPACT_SUBAGENT_TOOL_DESCRIPTION.to_string())
            }
            "safetyGuidance" | "safety" => Some(SUBAGENT_SAFETY_GUIDANCE.to_string()),
            "agentDir" => Some(options.agent_dir.display().to_string()),
            "projectConfigDir" => Some(project_config_dir.display().to_string()),
            _ => None,
        }
    };

    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        // A placeholder is `{{`, one-or-more `\w`, `}}`. Anything else copies through verbatim.
        let Some(open) = template.get(cursor..).and_then(|rest| rest.find("{{")) else {
            out.push_str(template.get(cursor..).unwrap_or_default());
            break;
        };
        let open = cursor + open;
        out.push_str(template.get(cursor..open).unwrap_or_default());
        let name_start = open + 2;
        let name_len = template
            .get(name_start..)
            .unwrap_or_default()
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        let name_end = name_start + name_len;
        let closes = template
            .get(name_end..)
            .is_some_and(|rest| rest.starts_with("}}"));
        if name_len == 0 || !closes {
            // Not a `\w+` placeholder — upstream's regex does not match, so the `{{` is literal.
            out.push_str("{{");
            cursor = name_start;
            continue;
        }
        let name = template.get(name_start..name_end).unwrap_or_default();
        match variable(name) {
            Some(replacement) => out.push_str(&replacement),
            None => {
                warnings.push(format!(
                    "{CUSTOM_TOOL_DESCRIPTION_FILE}: unknown placeholder {{{{{name}}}}} left unchanged."
                ));
                out.push_str(template.get(open..name_end + 2).unwrap_or_default());
            }
        }
        cursor = name_end + 2;
    }
    out
}

/// pi `loadCustomToolDescription` (`tool-description.ts:143-178`) — walk both candidate paths and
/// return the first usable one, warning (never failing) on each rejected candidate.
///
/// Every rejection continues to the NEXT path rather than aborting, exactly as upstream's `continue`
/// does: an unreadable project override must not mask a perfectly good user-scope one.
#[must_use]
fn load_custom_tool_description(
    full: &str,
    options: &ToolDescriptionOptions,
    warnings: &mut Vec<String>,
) -> Option<String> {
    for file_path in custom_description_paths(options) {
        let display = file_path.display().to_string();
        let metadata = match std::fs::metadata(&file_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                warnings.push(format!(
                    "Failed to inspect custom tool description '{display}': {error}"
                ));
                continue;
            }
        };
        if !metadata.is_file() {
            warnings.push(format!(
                "Ignoring custom tool description '{display}' because it is not a file."
            ));
            continue;
        }
        if metadata.len() > CUSTOM_TOOL_DESCRIPTION_MAX_BYTES {
            warnings.push(format!(
                "Ignoring custom tool description '{display}' because it is larger than \
                 {CUSTOM_TOOL_DESCRIPTION_MAX_BYTES} bytes."
            ));
            continue;
        }
        let template = match std::fs::read_to_string(&file_path) {
            Ok(template) => template,
            Err(error) => {
                warnings.push(format!(
                    "Failed to read custom tool description '{display}': {error}"
                ));
                continue;
            }
        };
        if template.trim().is_empty() {
            warnings.push(format!(
                "Ignoring empty custom tool description '{display}'."
            ));
            continue;
        }
        let rendered = render_custom_template(template.trim(), full, options, warnings);
        if rendered.trim().is_empty() {
            warnings.push(format!(
                "Ignoring custom tool description '{display}' because it rendered empty."
            ));
            continue;
        }
        return Some(rendered.trim().to_string());
    }
    None
}

/// pi `withMandatorySafetyGuidance` (`tool-description.ts:180-189`) — THE load-bearing half of
/// SUBA-025.
///
/// Splitting on the guidance before re-appending it is not redundant: it is what makes the result
/// idempotent and canonical. A custom description that already embeds the block (say, via
/// `{{safety}}` in the middle of the file) has it lifted out and re-appended ONCE, at the end,
/// rather than ending up with the block twice or buried mid-document where it is easiest to ignore.
/// A description that is *nothing but* the guidance collapses to the guidance alone.
#[must_use]
fn with_mandatory_safety_guidance(description: &str) -> String {
    let custom_description = description
        .split(SUBAGENT_SAFETY_GUIDANCE)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if custom_description.is_empty() {
        SUBAGENT_SAFETY_GUIDANCE.to_string()
    } else {
        format!("{custom_description}\n\n{SUBAGENT_SAFETY_GUIDANCE}")
    }
}

/// pi `buildSubagentToolDescription` (`tool-description.ts:191-200`) — the whole resolution, and
/// the only entry point `extension.rs` calls.
///
/// `full` is `extension.rs`'s `SUBAGENT_TOOL_DESCRIPTION`, passed in rather than duplicated here
/// (see the module doc): upstream owns both constants in one file, cyrup owns the full one next to
/// the tool it describes, and threading it keeps a single source of truth either way.
#[must_use]
pub fn build_subagent_tool_description(
    configured: Option<&serde_json::Value>,
    full: &str,
    options: &ToolDescriptionOptions,
    warnings: &mut Vec<String>,
) -> String {
    let mode = resolve_tool_description_mode(configured, warnings);
    if mode == ToolDescriptionMode::Compact {
        return COMPACT_SUBAGENT_TOOL_DESCRIPTION.to_string();
    }
    if mode == ToolDescriptionMode::Custom {
        if let Some(custom) = load_custom_tool_description(full, options, warnings) {
            return with_mandatory_safety_guidance(&custom);
        }
        warnings.push(format!(
            "{CUSTOM_TOOL_DESCRIPTION_FILE} was not found or valid for toolDescriptionMode \
             \"custom\"; using full description."
        ));
    }
    full.to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// A hermetic options pair: both search roots inside one tempdir, so no test can read the
    /// developer's real `~/.cyrup/agent/subagent-tool-description.md`.
    fn options(dir: &std::path::Path) -> ToolDescriptionOptions {
        let cwd = dir.join("project");
        let agent_dir = dir.join("agent");
        std::fs::create_dir_all(project_config_dir(&cwd)).expect("project config dir");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        ToolDescriptionOptions { cwd, agent_dir }
    }

    const FULL: &str = "FULL DESCRIPTION BODY";

    /// pi `resolveToolDescriptionMode` (`tool-description.ts:104-110`), all four arms including the
    /// verbatim warning — which renders the offending value through `JSON.stringify`, so a string
    /// is quoted and a number is not.
    #[test]
    fn the_mode_resolves_and_an_invalid_one_warns_with_pis_verbatim_text() {
        let mut warnings = Vec::new();
        assert_eq!(
            resolve_tool_description_mode(None, &mut warnings),
            ToolDescriptionMode::Full
        );
        for (raw, expected) in [
            ("full", ToolDescriptionMode::Full),
            ("compact", ToolDescriptionMode::Compact),
            ("custom", ToolDescriptionMode::Custom),
        ] {
            assert_eq!(
                resolve_tool_description_mode(
                    Some(&serde_json::Value::String(raw.to_string())),
                    &mut warnings
                ),
                expected
            );
        }
        assert!(warnings.is_empty(), "no valid value warns: {warnings:?}");

        assert_eq!(
            resolve_tool_description_mode(Some(&serde_json::json!("brief")), &mut warnings),
            ToolDescriptionMode::Full
        );
        assert_eq!(
            warnings,
            vec![
                "Ignoring invalid toolDescriptionMode \"brief\"; expected \"full\", \"compact\", \
                 or \"custom\"."
                    .to_string()
            ]
        );

        warnings.clear();
        assert_eq!(
            resolve_tool_description_mode(Some(&serde_json::json!(3)), &mut warnings),
            ToolDescriptionMode::Full
        );
        assert_eq!(
            warnings,
            vec![
                "Ignoring invalid toolDescriptionMode 3; expected \"full\", \"compact\", or \
                 \"custom\"."
                    .to_string()
            ]
        );
    }

    /// pi `buildSubagentToolDescription`'s `compact` arm (`tool-description.ts:193`).
    #[test]
    fn compact_mode_returns_the_short_form_and_full_mode_returns_the_full_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = options(dir.path());
        let mut warnings = Vec::new();
        assert_eq!(
            build_subagent_tool_description(
                Some(&serde_json::json!("compact")),
                FULL,
                &opts,
                &mut warnings
            ),
            COMPACT_SUBAGENT_TOOL_DESCRIPTION
        );
        assert_eq!(
            build_subagent_tool_description(None, FULL, &opts, &mut warnings),
            FULL
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(
            COMPACT_SUBAGENT_TOOL_DESCRIPTION.len() < 2_048,
            "the compact form exists to save context"
        );
    }

    /// THE reason SUBA-025's severity was raised: a deployment may replace the whole description
    /// and cannot drop the safety guidance. pi `withMandatorySafetyGuidance` (`:180`), applied on
    /// the `custom` branch (`:196`).
    #[test]
    fn a_custom_description_file_is_used_and_the_safety_guidance_is_appended() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = options(dir.path());
        std::fs::write(
            project_config_dir(&opts.cwd).join(CUSTOM_TOOL_DESCRIPTION_FILE),
            "Only delegate through the platform team's reviewer agent.\n",
        )
        .expect("write override");

        let mut warnings = Vec::new();
        let built = build_subagent_tool_description(
            Some(&serde_json::json!("custom")),
            FULL,
            &opts,
            &mut warnings,
        );
        assert_eq!(
            built,
            format!(
                "Only delegate through the platform team's reviewer agent.\n\n\
                 {SUBAGENT_SAFETY_GUIDANCE}"
            )
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!built.contains(FULL), "the override REPLACES the full text");
    }

    /// The appender is idempotent and canonicalizing: an override that already embeds the block
    /// (here via `{{safety}}` in the middle) ends with it exactly once, at the end.
    #[test]
    fn an_embedded_safety_block_is_lifted_out_and_re_appended_exactly_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = options(dir.path());
        std::fs::write(
            project_config_dir(&opts.cwd).join(CUSTOM_TOOL_DESCRIPTION_FILE),
            "Header.\n\n{{safety}}\n\nFooter.\n",
        )
        .expect("write override");

        let mut warnings = Vec::new();
        let built = build_subagent_tool_description(
            Some(&serde_json::json!("custom")),
            FULL,
            &opts,
            &mut warnings,
        );
        assert_eq!(
            built.matches(SUBAGENT_SAFETY_GUIDANCE).count(),
            1,
            "exactly one copy: {built}"
        );
        assert!(built.ends_with(SUBAGENT_SAFETY_GUIDANCE), "and it is last");
        assert_eq!(built, format!("Header.\n\nFooter.\n\n{SUBAGENT_SAFETY_GUIDANCE}"));

        // A file that is NOTHING but the guidance collapses to the guidance alone.
        assert_eq!(
            with_mandatory_safety_guidance(SUBAGENT_SAFETY_GUIDANCE),
            SUBAGENT_SAFETY_GUIDANCE
        );
    }

    /// pi `renderCustomTemplate` (`:121-141`): the eight variables, and the verbatim warning that
    /// leaves an unknown placeholder untouched rather than blanking it.
    #[test]
    fn template_placeholders_render_and_an_unknown_one_warns_and_survives() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = options(dir.path());
        let mut warnings = Vec::new();
        let rendered = render_custom_template(
            "{{full}}|{{fullDescription}}|{{compact}}|{{agentDir}}|{{projectConfigDir}}|{{nope}}|{{ spaced }}",
            FULL,
            &opts,
            &mut warnings,
        );
        assert!(rendered.starts_with(&format!("{FULL}|{FULL}|")));
        assert!(rendered.contains(COMPACT_SUBAGENT_TOOL_DESCRIPTION));
        assert!(rendered.contains(&opts.agent_dir.display().to_string()));
        assert!(rendered.contains(&project_config_dir(&opts.cwd).display().to_string()));
        assert!(rendered.ends_with("|{{nope}}|{{ spaced }}"), "{rendered}");
        assert_eq!(
            warnings,
            vec![format!(
                "{CUSTOM_TOOL_DESCRIPTION_FILE}: unknown placeholder {{{{nope}}}} left unchanged."
            )],
            "`{{{{ spaced }}}}` is not a `\\w+` match upstream, so it warns nothing"
        );
    }

    /// pi's size gate (`:157-159`) and its fall-through: an over-cap PROJECT override is skipped
    /// with the verbatim warning and the USER-scope file is used instead.
    #[test]
    fn an_over_cap_override_is_refused_with_pis_text_and_the_next_path_still_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = options(dir.path());
        let oversized = project_config_dir(&opts.cwd).join(CUSTOM_TOOL_DESCRIPTION_FILE);
        std::fs::write(
            &oversized,
            "x".repeat(usize::try_from(CUSTOM_TOOL_DESCRIPTION_MAX_BYTES).unwrap() + 1),
        )
        .expect("write oversized");
        std::fs::write(
            opts.agent_dir.join(CUSTOM_TOOL_DESCRIPTION_FILE),
            "User-scope override.",
        )
        .expect("write user override");

        let mut warnings = Vec::new();
        let built = build_subagent_tool_description(
            Some(&serde_json::json!("custom")),
            FULL,
            &opts,
            &mut warnings,
        );
        assert_eq!(
            built,
            format!("User-scope override.\n\n{SUBAGENT_SAFETY_GUIDANCE}")
        );
        assert_eq!(
            warnings,
            vec![format!(
                "Ignoring custom tool description '{}' because it is larger than 51200 bytes.",
                oversized.display()
            )]
        );
    }

    /// pi's `custom`-with-no-file fallback (`:197`), verbatim — the description degrades to `full`
    /// rather than to nothing.
    #[test]
    fn custom_mode_with_no_usable_file_falls_back_to_full_with_pis_warning() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = options(dir.path());
        std::fs::write(
            project_config_dir(&opts.cwd).join(CUSTOM_TOOL_DESCRIPTION_FILE),
            "   \n\t\n",
        )
        .expect("write blank override");

        let mut warnings = Vec::new();
        assert_eq!(
            build_subagent_tool_description(
                Some(&serde_json::json!("custom")),
                FULL,
                &opts,
                &mut warnings
            ),
            FULL
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].starts_with("Ignoring empty custom tool description '"));
        assert_eq!(
            warnings[1],
            "subagent-tool-description.md was not found or valid for toolDescriptionMode \
             \"custom\"; using full description."
        );
    }

    /// pi's not-a-file gate (`:153-155`): a DIRECTORY named `subagent-tool-description.md` is
    /// refused with its own sentence rather than crashing the read.
    #[test]
    fn a_directory_named_like_the_override_is_refused_with_pis_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = options(dir.path());
        let as_dir = project_config_dir(&opts.cwd).join(CUSTOM_TOOL_DESCRIPTION_FILE);
        std::fs::create_dir_all(&as_dir).expect("mkdir");

        let mut warnings = Vec::new();
        assert_eq!(
            build_subagent_tool_description(
                Some(&serde_json::json!("custom")),
                FULL,
                &opts,
                &mut warnings
            ),
            FULL
        );
        assert_eq!(
            warnings[0],
            format!(
                "Ignoring custom tool description '{}' because it is not a file.",
                as_dir.display()
            )
        );
    }

    /// The `[CYRUP-DELTA]` on [`COMPACT_SUBAGENT_TOOL_DESCRIPTION`], enforced MECHANICALLY rather
    /// than asserted in prose: the compact text may not name a management/control verb this crate
    /// cannot dispatch, because a model that reads the description and calls one lands on the
    /// unknown-action arm — the exact advertise-vs-refuse defect SUBA-046 was filed for.
    ///
    /// The four `schedule*` verbs upstream's line 80 carries are the only ones the deletion covers;
    /// if a later sweep lands `scheduledRuns` (SUBA-016), restoring that line is what makes this
    /// test keep passing with it back in.
    #[test]
    fn the_compact_description_advertises_no_verb_cyrup_cannot_dispatch() {
        for verb in ["schedule", "schedule-list", "schedule-status", "schedule-cancel"] {
            assert!(
                !COMPACT_SUBAGENT_TOOL_DESCRIPTION.contains(verb),
                "compact text advertises unported verb '{verb}'"
            );
        }
        // …while every verb it DOES name is one this crate answers.
        for verb in [
            "list", "get", "models", "create", "update", "delete", "eject", "disable", "enable",
            "reset", "doctor", "status", "interrupt", "resume", "steer", "append-step",
        ] {
            assert!(
                COMPACT_SUBAGENT_TOOL_DESCRIPTION.contains(verb),
                "compact text lost live verb '{verb}'"
            );
        }
    }

    /// The safety guidance is a PINNED constant: it is what every custom description is forced to
    /// carry, so an edit to it is a change to what every orchestrator is told it may do.
    #[test]
    fn the_safety_guidance_is_pinned_to_pis_v0_34_0_text() {
        assert!(SUBAGENT_SAFETY_GUIDANCE.starts_with("SAFETY-CRITICAL SUBAGENT GUIDANCE:\n"));
        assert_eq!(
            SUBAGENT_SAFETY_GUIDANCE.lines().count(),
            7,
            "one header plus six bullets"
        );
        assert_eq!(SUBAGENT_SAFETY_GUIDANCE.len(), 1333, "byte length is pinned");
        assert!(SUBAGENT_SAFETY_GUIDANCE
            .contains("ordinary child subagents are not orchestrators and must not run subagents"));
        assert!(SUBAGENT_SAFETY_GUIDANCE.contains("keep one writer for the same cwd/worktree"));
        assert!(!SUBAGENT_SAFETY_GUIDANCE.contains("workflowScript"), "v0.34.0, not v0.43.0");
    }
}
