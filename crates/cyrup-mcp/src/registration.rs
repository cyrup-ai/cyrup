//! Everything `init()` registers, from disk caches only — `index.ts`'s `installMcpAdapter` body
//! (13a §1, 13e §5/§6/§8; MCP-002, MCP-003, MCP-036, MCP-043, MCP-212, MCP-213, MCP-216, MCP-218,
//! MCP-219, MCP-247).
//!
//! # The invariant this module exists to protect
//!
//! `installMcpAdapter` runs **synchronously and completely**, with no `await` in its body and no
//! MCP server contacted. From `mcp.json` and `<agent_dir>/mcp-cache.json` alone it registers the
//! entire model-visible surface: one direct tool per cached MCP tool and resource, one slash
//! command per cached MCP prompt, the [`PROXY_TOOL_NAME`] gateway tool, [`MCP_COMMAND`],
//! [`MCP_AUTH_COMMAND`], and the [`MCP_CONFIG_FLAG`].
//!
//! A session therefore opens instantly with the full MCP tool surface visible to the model,
//! identical to the previous session's, with no subprocess spawned — and **the system prompt does
//! not change shape between a cold and a warm start**, which is what keeps the provider's
//! prompt-cache prefix valid.
//!
//! Two consequences for anyone editing this module:
//!
//! * **Read with `std::fs`, not `tokio::fs`.** Upstream uses `readFileSync`; the point is that
//!   nothing blocks the session build on the reactor.
//! * **Never return `Err`, never panic.** A native extension's failing `init()` is a fatal startup
//!   diagnostic that every mode arm turns into `dispose(); exit 1`. Upstream's `installMcpAdapter`
//!   *cannot* fail — every disk read it performs is defensive — so a malformed `mcp.json` or
//!   `mcp-cache.json` must degrade to an empty surface. Otherwise a stray `{{{` in a user's config
//!   crashes cyrup on a normal path (MCP-003, one of the port's criticals). Every function here
//!   that touches disk returns `Option`/a default, and [`register_surface`] has no fallible call.
//!
//! # Name formatting is a security boundary, not cosmetics
//!
//! [`BUILTIN_NAMES`] is the drop list `resolveDirectTools` applies to every formatted name, and in
//! cyrup it matters **more** than upstream: `ExtensionRegistry::active_tools`
//! (`crates/cyrup-ext/src/registry.rs`) walks the base tool list and substitutes the extension
//! registry's tool wherever the names match, so an MCP server shipping a tool named `read` would
//! replace cyrup's filesystem read tool for the whole session — every subsequent `read` call
//! routed to the remote server, silently, with the model's file paths as arguments. That is
//! MCP-212's permission-bypass clause and the reason it is `critical`.
//!
//! # The renderer seam
//!
//! Upstream passes `renderCall` / `renderResult` as **per-tool arguments**. cyrup splits them into
//! a declaration ([`InitApi::register_tool_renderer`]) plus a name-keyed serve
//! (`NativeExtension::{render_call, render_result}`). [`register_surface`] returns every registered
//! tool name in [`RegisteredSurface::tool_names`] and [`McpExtension`][crate::extension::McpExtension]'s `init`
//! declares a renderer for each; a name missing from that list has an unreachable renderer.
//!
//! # The execute seam
//!
//! Upstream's `registerDirectTool` passes `execute: createDirectToolExecutor(() => state, () =>
//! initPromise, spec)` — a closure over slots that are still empty at registration time, because
//! `initializeMcp` has not run yet. cyrup registers `Arc<dyn Tool>` values instead of object
//! literals, so the same late binding is a [`ToolDispatch`] slot every registered tool shares: the
//! runtime installs an [`McpToolDispatch`] into it once [`crate::state::McpState`] exists (MCP-214
//! for direct tools, 13d for the nine proxy modes). Until it is installed a call answers upstream's
//! own step-3 result — `MCP not initialized` with `details.error = "not_initialized"` — which is
//! exactly what upstream returns for a call that lands before init resolves.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use cyrup_core::{
    CancelToken, Content, Tool, ToolCallId, ToolError, ToolRenderKind, ToolResult, ToolUpdateSink,
};
use cyrup_ext::native::InitApi;
use cyrup_ext::{CommandDescriptor, EventKind};
use indexmap::{IndexMap, IndexSet};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::{
    BoolOrList, McpConfig, McpSettings, ServerEntry, ToolPrefix, ToolResultRendering,
};
use crate::dirs::McpDirs;
use crate::proxy::ToolMetadata;
use crate::proxy::constants::{REGEX_DFA_SIZE_LIMIT, REGEX_SIZE_LIMIT};

// ---------------------------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------------------------

/// The `--mcp-config` flag. Registered for `--help` even though its value is read from argv:
/// `ExtensionHost::apply_extension_flag_values` runs *after* the native-load loop, so `init` cannot
/// see the flag store — but an unreconciled `--flag` is itself a startup diagnostic, so registering
/// it is not optional (MCP-002).
pub const MCP_CONFIG_FLAG: &str = "mcp-config";

/// The gateway tool. On a cold cache this is the **only** model-facing MCP surface, which is why
/// `settings.disableProxyTool` must be treated as unsupported unless late tool registration
/// (HA-1 / MCP-037) exists — see [`should_register_proxy_tool`].
pub const PROXY_TOOL_NAME: &str = "mcp";

/// `/mcp` — the eight-way switch (status, setup, connect, reconnect, disconnect, tools, …).
pub const MCP_COMMAND: &str = "mcp";

/// `/mcp-auth` — the OAuth flow's interactive entry point.
pub const MCP_AUTH_COMMAND: &str = "mcp-auth";

/// The four seams the extension subscribes to.
///
/// * [`EventKind::SessionStart`] — the generation bump and the runtime build (MCP-008).
/// * [`EventKind::Input`] — `index.ts:489-511`'s pre-turn keep-alive convergence: the *turn* is
///   what must not start against a keep-alive server whose catalog went stale, so the gate hangs
///   off the submission, not off a timer.
/// * [`EventKind::SessionShutdown`] — **the only teardown point**, and where the metadata flush
///   lives (MCP-009, MCP-014).
/// * [`EventKind::ToolResult`] — `error-signal.ts`'s `toolErrorOverride`, which re-flags a returned
///   MCP failure as an error (MCP-045).
///
/// The `input` seam is **not** in 13a: the plan was written against v2.25.0, whose `index.ts` had
/// only the other three `pi.on` registrations. Upstream `48799fa` ("converge stale keep-alive tool
/// catalogs", #374) added the fourth, and it is load-bearing rather than an optimisation — without
/// the subscription [`McpExtension::on_input`][crate::extension::McpExtension::on_input] is dead
/// code and `ensureConverged` is reachable only from the 30-second timer, which is exactly the
/// window the fix closes.
pub const SUBSCRIBED_EVENTS: &[EventKind] = &[
    EventKind::SessionStart,
    EventKind::Input,
    EventKind::SessionShutdown,
    EventKind::ToolResult,
];

/// `direct-tools.ts`'s `BUILTIN_NAMES` — the eight names a formatted MCP tool name may never take
/// (MCP-212). Note `cyrup_tools::registry`'s own built-in list has **seven** entries and omits
/// `mcp`; this is the adapter's list, and `cyrup_ext_subagents::exec::mcp_direct_tools`'
/// `BUILTIN_TOOL_NAMES` already carries these same eight verbatim.
pub const BUILTIN_NAMES: [&str; 8] = ["read", "bash", "edit", "write", "grep", "find", "ls", "mcp"];

/// `direct-tools.ts`'s `DIRECT_TOOLS_ADVISORY_THRESHOLD`. At or above this many resolved specs the
/// resolver warns once: every direct tool costs prompt context on every turn (MCP-246). The warning
/// is suppressible with `settings.warnOnLargeDirectTools: false`; the threshold itself is not a cap
/// and never drops a spec.
pub const DIRECT_TOOLS_ADVISORY_THRESHOLD: usize = 75;

/// `direct-tools.ts`'s `INSTRUCTIONS_SNIPPET_LENGTH` — how much of a server's `instructions` the
/// proxy description quotes before `mcp({ instructions: "name" })` is needed for the rest.
const INSTRUCTIONS_SNIPPET_LENGTH: usize = 150;

/// `prompts.ts`'s `buildCommandDescription` budget.
const PROMPT_COMMAND_DESCRIPTION_LENGTH: usize = 120;

/// `index.ts`'s `registerDirectTool` `promptSnippet` budget.
const DIRECT_TOOL_PROMPT_SNIPPET_LENGTH: usize = 100;

/// `metadata-cache.ts`'s `CACHE_VERSION`. A cache written under any other version is ignored
/// wholesale — **and is not renumbered by the port** (MCP-209): `mcp-cache.json` is a cross-crate,
/// cross-*product* contract shared with a co-installed `pi-mcp-adapter` and read by
/// `cyrup_ext_subagents::exec::mcp_direct_tools`.
pub const METADATA_CACHE_VERSION: u32 = 1;

/// `metadata-cache.ts`'s `CACHE_MAX_AGE_MS` — seven days.
pub const METADATA_CACHE_MAX_AGE_MS: f64 = 7.0 * 24.0 * 60.0 * 60.0 * 1000.0;

/// The env override subagents and CI use to pin a minimal MCP tool surface (MCP-219).
pub const DIRECT_TOOLS_ENV_VAR: &str = "MCP_DIRECT_TOOLS";

/// The guideline cyrup's system-prompt sanitizer keeps **only** when a tool named `mcp` is in the
/// allowed set (MCP-236). The matcher is `cyrup_permission_system::sanitize::tools`, and it runs on
/// `split_whitespace().join(" ").to_lowercase()` — so this literal must stay lowercase and
/// whitespace-normalizable to exactly the string that matcher compares against, or the sanitizer
/// silently drops it and the model loses the discovery instruction. A tree-wide grep finds exactly
/// two occurrences: the matcher, and this line.
pub const PROXY_TOOL_PROMPT_GUIDELINE: &str = "use mcp for mcp discovery first: search by capability, describe one exact tool name, then call it.";

/// `index.ts`'s proxy-tool `promptSnippet`.
const PROXY_TOOL_PROMPT_SNIPPET: &str =
    "MCP gateway — status, search, describe, auth, and single MCP tool calls";

// ---------------------------------------------------------------------------------------------
// 1. Naming — `types.ts`'s name-formatting block (MCP-200, MCP-201, MCP-202, MCP-203, MCP-206)
// ---------------------------------------------------------------------------------------------

/// `types.ts` `sanitizeServerPrefix`. Iterates by **code point** (`Array.from`), keeps the valid
/// class verbatim and escapes everything else as `_<hex codepoint>_`.
///
/// `preserve_provider_valid` is upstream's default-`true` argument: the current grammar keeps `-`
/// and `_`, the legacy grammar (used only to build alias candidates) escapes them. So `github-mcp`
/// stays `github-mcp` today and was `github_2d_mcp` before; `naïve` is `na_ef_ve` in both.
///
/// **MCP-205, unresolved:** `cyrup_ext_subagents::exec::mcp_direct_tools`' private copy of this
/// grammar does `server_name.replace('-', "_")` instead, has no `mcp` prefix mode and no escape
/// form. Until that reconciliation lands, a subagent `mcp:server/tool` selector against a server
/// whose name contains a `-` resolves to a name this crate never registers.
#[must_use]
pub fn sanitize_server_prefix(server_name: &str, preserve_provider_valid: bool) -> String {
    let mut out = String::with_capacity(server_name.len());
    for ch in server_name.chars() {
        let valid = if preserve_provider_valid {
            ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
        } else {
            ch.is_ascii_alphanumeric()
        };
        if valid {
            out.push(ch);
        } else {
            out.push('_');
            out.push_str(&format!("{:x}", ch as u32));
            out.push('_');
        }
    }
    out
}

/// `serverName.replace(/-?mcp$/i, "")` — strip a trailing case-insensitive `mcp` plus one optional
/// preceding `-`. Byte arithmetic is safe because the matched suffix is ASCII.
fn strip_mcp_suffix(server_name: &str) -> &str {
    let lower = server_name.to_ascii_lowercase();
    let Some(without_mcp) = lower.strip_suffix("mcp") else {
        return server_name;
    };
    let cut = without_mcp.strip_suffix('-').unwrap_or(without_mcp).len();
    server_name.get(..cut).unwrap_or(server_name)
}

/// `types.ts` `getServerPrefix` (MCP-200).
#[must_use]
pub fn server_prefix(server_name: &str, mode: ToolPrefix) -> String {
    match mode {
        ToolPrefix::None => String::new(),
        ToolPrefix::Short => {
            let short = sanitize_server_prefix(strip_mcp_suffix(server_name), true);
            if short.is_empty() { "mcp".to_string() } else { short }
        }
        ToolPrefix::Mcp => format!("mcp__{}", sanitize_server_prefix(server_name, true)),
        ToolPrefix::Server => sanitize_server_prefix(server_name, true),
    }
}

/// `types.ts` `formatToolName` (MCP-200). **Dots only** — a hyphen inside the *tool* name survives,
/// which is why the legacy candidate set below exists at all.
///
/// `mcp__server__tool` is **not** a tool name: in [`ToolPrefix::Mcp`] mode a tool is
/// `mcp__<sanitizedServer>_<tool>`, one underscore between server and tool. The double-underscore
/// form belongs to prompt slash commands ([`format_prompt_command_name`]).
#[must_use]
pub fn format_tool_name(tool_name: &str, server_name: &str, prefix: ToolPrefix) -> String {
    let p = server_prefix(server_name, prefix);
    let sanitized = tool_name.replace('.', "_");
    if p.is_empty() { sanitized } else { format!("{p}_{sanitized}") }
}

/// `types.ts` `resolveToolPrefix` — `definition.toolPrefix ?? globalPrefix ?? "server"`.
///
/// The definition is **optional**, as upstream's `definition?: Pick<ServerEntry, "toolPrefix">`
/// is: a tool name may be resolved for a server that has no `mcpServers` entry, and that case
/// falls through to the global mode rather than being unrepresentable.
#[must_use]
pub fn resolve_tool_prefix(definition: Option<&ServerEntry>, global: ToolPrefix) -> ToolPrefix {
    definition.and_then(|entry| entry.tool_prefix).unwrap_or(global)
}

/// `types.ts` `resolveServerFromToolName(toolName, serverNames, prefix)` — the inverse of
/// [`server_prefix`] (MCP-073).
///
/// Returns the configured server whose prefix the tool name starts with, longest first, and
/// **`None` when two different server names produce that same longest prefix**. That fail-safe is
/// the point of the function: `short` mode deliberately maps `foo` and `foo-mcp` onto the same
/// prefix, and a downstream permission gate must fall back to its wildcard path rather than
/// enforce a server-scoped rule against the wrong server.
///
/// Borrowed, not owned: the answer is always one of the inputs, so a caller that needs a `String`
/// writes `.map(str::to_owned)` and one that does not pays nothing.
///
/// # The consumer is not wired, deliberately
///
/// `cyrup_permission_system::manager::add_derived_mcp_server_targets` derives MCP server targets
/// today with a **suffix** test (`tool_name.ends_with(&format!("_{server}"))`), which is a
/// different rule from this prefix one. Reconciling the two is a cross-crate decision (MCP-191)
/// and is not this unit's; upstream ships `resolveServerFromToolName` with no production caller
/// either.
#[must_use]
pub fn resolve_server_from_tool_name<'a, I>(
    tool_name: &str,
    server_names: I,
    prefix: ToolPrefix,
) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    if matches!(prefix, ToolPrefix::None) {
        return None;
    }
    // One pass instead of upstream's collect-sort-scan. `sort((a, b) => b.len - a.len)` is stable,
    // so upstream keeps the FIRST name at the winning length; taking a strictly-longer prefix only
    // reproduces that, and resets the ambiguity flag because the fail-safe is evaluated against
    // the new best.
    let mut best: Option<(&'a str, String)> = None;
    let mut ambiguous = false;
    for name in server_names {
        let candidate = server_prefix(name, prefix);
        // `if (p && toolName.startsWith(p + "_"))`.
        if candidate.is_empty() {
            continue;
        }
        let mut boundary = String::with_capacity(candidate.len() + 1);
        boundary.push_str(&candidate);
        boundary.push('_');
        if !tool_name.starts_with(&boundary) {
            continue;
        }
        match &best {
            Some((best_name, best_prefix)) if best_prefix.len() >= candidate.len() => {
                // `candidates.some(c => c.prefix === best.prefix && c.name !== best.name)`.
                if *best_prefix == candidate && *best_name != name {
                    ambiguous = true;
                }
            }
            _ => {
                best = Some((name, candidate));
                ambiguous = false;
            }
        }
    }
    if ambiguous {
        return None;
    }
    best.map(|(name, _)| name)
}

/// `types.ts` `getLegacyServerPrefix` — the same four modes over the pre-`-`/`_` grammar.
fn legacy_server_prefix(server_name: &str, mode: ToolPrefix) -> String {
    match mode {
        ToolPrefix::None => String::new(),
        ToolPrefix::Short => {
            let short = sanitize_server_prefix(strip_mcp_suffix(server_name), false);
            if short.is_empty() { "mcp".to_string() } else { short }
        }
        ToolPrefix::Mcp => format!("mcp__{}", sanitize_server_prefix(server_name, false)),
        ToolPrefix::Server => sanitize_server_prefix(server_name, false),
    }
}

/// `types.ts` `formatLegacyToolName` — note the tool name loses **hyphens as well as dots** here.
fn format_legacy_tool_name(tool_name: &str, server_name: &str, prefix: ToolPrefix) -> String {
    let p = legacy_server_prefix(server_name, prefix);
    let sanitized: String =
        tool_name.chars().map(|c| if c == '.' || c == '-' { '_' } else { c }).collect();
    if p.is_empty() { sanitized } else { format!("{p}_{sanitized}") }
}

/// `types.ts` `getToolNameCandidates` (MCP-201) — every name a user might plausibly have written in
/// an `includeTools` / `excludeTools` / `approveTools` entry.
///
/// Five expressions when `include_legacy` is false; **thirteen more** when it is true (5 + 4 + 4).
/// The resulting set is far smaller than 18 because of heavy overlap — for
/// `("list-sims", "xcodebuild-mcp", Short)` the current set has 4 members and the full set 12.
///
/// An [`IndexSet`], not a `HashSet`, because upstream returns a JS `Set` and one caller reads its
/// order: [`crate::proxy::is_tool_call_approval_required`]'s legacy arm takes the *first* current
/// candidate that is not the tool's own original name, which is upstream's
/// `[...currentCandidates].find(c => c !== toolMeta.originalName)`. The three legacy families are
/// therefore emitted as three complete groups, exactly as `types.ts` emits them, rather than
/// interleaved.
#[must_use]
pub fn tool_name_candidates(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    include_legacy: bool,
) -> IndexSet<String> {
    const MODES: [ToolPrefix; 3] = [ToolPrefix::Server, ToolPrefix::Short, ToolPrefix::Mcp];
    let mut out = IndexSet::new();
    out.insert(tool_name.to_string());
    out.insert(format_tool_name(tool_name, server_name, prefix));
    for mode in MODES {
        out.insert(format_tool_name(tool_name, server_name, mode));
    }
    if !include_legacy {
        return out;
    }
    // `types.ts` group 1: the `-`→`_` tool name under every prefix.
    let legacy_tool_name = tool_name.replace('-', "_");
    out.insert(legacy_tool_name.clone());
    out.insert(format_tool_name(&legacy_tool_name, server_name, prefix));
    for mode in MODES {
        out.insert(format_tool_name(&legacy_tool_name, server_name, mode));
    }
    // `types.ts` group 2: the pre-2.x server-prefix grammar.
    out.insert(format_legacy_tool_name(tool_name, server_name, prefix));
    for mode in MODES {
        out.insert(format_legacy_tool_name(tool_name, server_name, mode));
    }
    // `types.ts` group 3: the current spellings, post-normalised.
    out.insert(format_tool_name(tool_name, server_name, prefix).replace('-', "_"));
    for mode in MODES {
        out.insert(format_tool_name(tool_name, server_name, mode).replace('-', "_"));
    }
    out
}

/// `types.ts` `globToRegExp`: escape `[.+^${}()|[\]\\]`, then `*` → `.*` and `?` → `.`, anchored.
///
/// JS escapes first and then substitutes; `*` and `?` are not in the escape set, so the single pass
/// below is equivalent. A pattern Rust's parser rejects yields `None` and is treated as "matches
/// nothing" rather than propagating — this runs inside an infallible `init`.
///
/// **Ceilinged (MCP-076).** These patterns are config-supplied — `includeTools`, `excludeTools`,
/// `approveTools`, `searchKeywords` — so the compiled program gets the same explicit
/// [`REGEX_SIZE_LIMIT`] / [`REGEX_DFA_SIZE_LIMIT`] the model-supplied search query gets
/// (MCP-159). A pattern that exceeds either ceiling fails to build and therefore matches nothing,
/// which is the same outcome as a pattern the parser rejects.
fn glob_to_regex(pattern: &str) -> Option<Regex> {
    let mut out = String::with_capacity(pattern.len() + 2);
    out.push('^');
    for ch in pattern.chars() {
        match ch {
            '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            _ => out.push(ch),
        }
    }
    out.push('$');
    RegexBuilder::new(&out)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
        .build()
        .ok()
}

fn is_glob(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

/// `types.ts` `matchesToolPattern` (MCP-202). An exact `Set.has` for a literal pattern, a regex
/// test over every candidate for a glob. Upstream recompiles the regex inside the loop; so does
/// this, deliberately — the memoised path lives in [`CandidateIndex`], where upstream put it.
#[must_use]
pub fn matches_tool_pattern(candidates: &IndexSet<String>, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }
    for pattern in patterns {
        if !is_glob(pattern) {
            if candidates.contains(pattern) {
                return true;
            }
            continue;
        }
        if let Some(re) = glob_to_regex(pattern)
            && candidates.iter().any(|candidate| re.is_match(candidate))
        {
            return true;
        }
    }
    false
}

/// `types.ts` `ToolSelectorCandidateIndex` — the *current* candidate names of every **other**
/// enabled server with a valid cache entry, plus the two memo tables upstream carries
/// (`matcherByPattern`, `matchingCountByPattern`).
///
/// This is the disambiguation rule's other half and the subtlest thing in `types.ts`: a legacy
/// alias may only exclude a tool if it does not *also* name some other tool's current name. Note
/// the index is built over **all** servers including the one being filtered — upstream does not
/// subtract the current tool's own candidates when building it; the subtraction happens in
/// `has_other_current_match`, by comparing match counts.
///
/// `additionalCurrentCandidatesByToolName` (`types.ts:813`) is the index's second table and
/// belongs to [`build_tool_metadata`]'s speculative arm alone (MCP-207): when a configured server
/// has no metadata yet, upstream guesses the names it *would* mint for the tools being evaluated
/// and files them under the tool they were guessed for, so a legacy alias is suppressed by a
/// collision that has not happened yet. `direct-tools.ts` never supplies it, so it stays empty for
/// every index [`build_candidate_index`] mints and no existing behaviour changes.
#[derive(Debug, Default)]
pub struct CandidateIndex {
    all_current: IndexSet<String>,
    /// `additionalCurrentCandidatesByToolName` — keyed by the *evaluated* tool name, which is why
    /// [`CandidateIndex::has_other_current_match`] needs a `tool_name` it never used to.
    additional_by_tool: HashMap<String, IndexSet<String>>,
    matcher: HashMap<String, Option<Regex>>,
    matching_count: HashMap<String, usize>,
}

impl CandidateIndex {
    /// `types.ts` `createToolSelectorCandidateIndex(candidates)` — upstream's second argument
    /// omitted, i.e. no `additionalCurrentCandidatesByToolName`.
    #[must_use]
    pub fn new(all_current: IndexSet<String>) -> Self {
        Self {
            all_current,
            additional_by_tool: HashMap::new(),
            matcher: HashMap::new(),
            matching_count: HashMap::new(),
        }
    }

    /// `types.ts` `createToolSelectorCandidateIndex(candidates, additionalCandidatesByToolName)` —
    /// the two-argument form, which only [`build_tool_metadata`] uses.
    #[must_use]
    pub fn with_additional(
        all_current: IndexSet<String>,
        additional_by_tool: HashMap<String, IndexSet<String>>,
    ) -> Self {
        Self {
            all_current,
            additional_by_tool,
            matcher: HashMap::new(),
            matching_count: HashMap::new(),
        }
    }

    /// `types.ts` `ToolSelectorCandidateIndex.allCurrentCandidates` — the readonly view of the set
    /// the index was built over. The two memo tables stay private; this one is upstream's public
    /// field and is what lets a caller assert *what* was indexed without being able to mutate it.
    ///
    /// **No caller yet.** The caller is the `list_changed` refresh path, which reads back the index
    /// it has just rebuilt — **MCP-120**, unported. Init builds the index and consumes it in the
    /// same breath, so it never needs to ask what was indexed.
    #[must_use]
    pub fn all_current(&self) -> &IndexSet<String> {
        &self.all_current
    }

    /// `types.ts` `indexHasOtherCurrentMatch` — does `pattern` name a *different* tool's current
    /// name?
    ///
    /// Membership is upstream's `hasCandidate` (`types.ts:853-854`): the whole-index set **or**
    /// the additional set filed under `tool_name`. It reaches three places, all of them
    /// load-bearing — the literal arm's test, the glob arm's total (the additional candidates that
    /// are not already in `all_current` count too, `types.ts:875-879`), and the glob arm's
    /// self-match subtraction (`types.ts:883-885`).
    fn has_other_current_match(
        &mut self,
        tool_name: &str,
        current_candidates: &IndexSet<String>,
        pattern: &str,
    ) -> bool {
        // Destructured so the memo tables can be borrowed mutably while `all_current` and the
        // additional set are borrowed shared — they are disjoint fields, which a `self.` path
        // cannot express.
        let Self { all_current, additional_by_tool, matcher, matching_count } = self;
        let additional = additional_by_tool.get(tool_name);
        let has_candidate = |candidate: &str| {
            all_current.contains(candidate)
                || additional.is_some_and(|extra| extra.contains(candidate))
        };

        if !is_glob(pattern) {
            return has_candidate(pattern) && !current_candidates.contains(pattern);
        }

        let matcher =
            matcher.entry(pattern.to_string()).or_insert_with(|| glob_to_regex(pattern)).as_ref();
        let Some(matcher) = matcher else {
            return false;
        };
        // Only the whole-index count is memoised, exactly as upstream memoises it: the additional
        // set is per-tool, so folding it into `matchingCountByPattern` would poison the memo for
        // the next tool that reuses the same pattern.
        let mut total = *matching_count.entry(pattern.to_string()).or_insert_with(|| {
            all_current.iter().filter(|candidate| matcher.is_match(candidate)).count()
        });
        if let Some(additional) = additional {
            total = total.saturating_add(
                additional
                    .iter()
                    .filter(|candidate| {
                        !all_current.contains(*candidate) && matcher.is_match(candidate)
                    })
                    .count(),
            );
        }
        if total == 0 {
            return false;
        }
        let current = current_candidates
            .iter()
            .filter(|candidate| has_candidate(candidate) && matcher.is_match(candidate))
            .count();
        total > current
    }
}

/// `types.ts` `matchesToolSelector` (MCP-202), the three-step disambiguation rule:
///
/// 1. any **current** candidate matches → `true`;
/// 2. no index supplied → fall back to matching the **full legacy** candidate set;
/// 3. otherwise a pattern wins only if it matches a legacy-only candidate **and** does not name any
///    other tool's current candidate.
///
/// Step 3 is what stops a legacy alias from silently excluding the wrong tool once two servers
/// exist whose sanitized prefixes collide.
fn matches_tool_selector(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    patterns: &[String],
    index: Option<&mut CandidateIndex>,
) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let current = tool_name_candidates(tool_name, server_name, prefix, false);
    if matches_tool_pattern(&current, patterns) {
        return true;
    }
    let Some(index) = index else {
        return matches_tool_pattern(
            &tool_name_candidates(tool_name, server_name, prefix, true),
            patterns,
        );
    };
    let mut legacy = tool_name_candidates(tool_name, server_name, prefix, true);
    for candidate in &current {
        legacy.shift_remove(candidate);
    }
    patterns.iter().any(|pattern| {
        matches_tool_pattern(&legacy, std::slice::from_ref(pattern))
            && !index.has_other_current_match(tool_name, &current, pattern)
    })
}

/// `types.ts` `isToolAllowed` = `isToolIncluded && !isToolExcluded` (MCP-202). An absent or empty
/// `includeTools` means "everything allowed"; `excludeTools` is a plain selector match.
#[must_use]
pub fn is_tool_allowed(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    include_tools: Option<&[String]>,
    exclude_tools: Option<&[String]>,
    mut index: Option<&mut CandidateIndex>,
) -> bool {
    let included = match include_tools.filter(|p| !p.is_empty()) {
        None => true,
        Some(patterns) => {
            matches_tool_selector(tool_name, server_name, prefix, patterns, index.as_deref_mut())
        }
    };
    if !included {
        return false;
    }
    match exclude_tools.filter(|p| !p.is_empty()) {
        None => true,
        // The last use, so the reborrow the include arm needed is not needed here.
        Some(patterns) => !matches_tool_selector(tool_name, server_name, prefix, patterns, index),
    }
}

/// `resource-tools.ts` `resourceNameToToolName` (MCP-203): non-alphanumerics → `_`, collapse runs,
/// trim the edges, lowercase; an empty or digit-leading result is prefixed with `resource`.
#[must_use]
pub fn resource_name_to_tool_name(name: &str) -> String {
    let mut collapsed = String::with_capacity(name.len());
    let mut prev_underscore = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            collapsed.push(ch);
            prev_underscore = false;
        } else if !prev_underscore {
            collapsed.push('_');
            prev_underscore = true;
        }
    }
    let result = collapsed.trim_matches('_').to_ascii_lowercase();
    let leading_digit = result.chars().next().is_some_and(|c| c.is_ascii_digit());
    if result.is_empty() {
        "resource".to_string()
    } else if leading_digit {
        format!("resource_{result}")
    } else {
        result
    }
}

/// A resource's *base* tool name — `read_${resourceNameToToolName(name)}` — before
/// [`format_tool_name`] applies the server prefix (MCP-203).
#[must_use]
pub fn resource_base_tool_name(resource_name: &str) -> String {
    format!("read_{}", resource_name_to_tool_name(resource_name))
}

/// `types.ts` `sanitizePromptName` (MCP-206).
#[must_use]
pub fn sanitize_prompt_name(name: &str) -> String {
    let mut replaced = String::with_capacity(name.len());
    let mut prev_replacement = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            replaced.push(ch);
            prev_replacement = false;
        } else if !prev_replacement {
            // `[^A-Za-z0-9_-]+` is a `+` run replaced by ONE underscore.
            replaced.push('_');
            prev_replacement = true;
        }
    }
    let cleaned = replaced.trim_matches(|c| c == '_' || c == '-');
    if cleaned.is_empty() {
        return "prompt".to_string();
    }
    if cleaned.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("_{cleaned}")
    } else {
        cleaned.to_string()
    }
}

/// `types.ts` `formatPromptCommandName` (MCP-206) — `mcp__<serverPart>__<prompt>`.
///
/// The `||` chain is load-bearing: `getServerPrefix(...) || sanitizeServerPrefix(name) || "server"`,
/// so even [`ToolPrefix::None`] yields a server segment. This is the **double**-underscore form,
/// unlike [`format_tool_name`].
#[must_use]
pub fn format_prompt_command_name(
    prompt_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
) -> String {
    let mut server_part = server_prefix(server_name, prefix);
    if server_part.is_empty() {
        server_part = sanitize_server_prefix(server_name, true);
    }
    if server_part.is_empty() {
        server_part = "server".to_string();
    }
    format!("mcp__{server_part}__{}", sanitize_prompt_name(prompt_name))
}

/// `utils.ts` `truncateAtWord`: cut at `target`, back up to the last space when that space sits past
/// 60 % of the budget, and append `...` (three ASCII dots, not `…`).
///
/// JS measures and slices in **UTF-16 code units**, so the budget is counted with
/// [`str::encode_utf16`] — matching the in-tree precedent in `cyrup_tools::truncate`. The one
/// deliberate divergence: a cut that would land inside an astral character stops before it rather
/// than emitting the lone surrogate JS would, because there is no such thing as an invalid `String`
/// here.
#[must_use]
pub fn truncate_at_word(text: &str, target: usize) -> String {
    if text.is_empty() || text.encode_utf16().count() <= target {
        return text.to_string();
    }
    let mut cut = text.len();
    let mut used = 0usize;
    for (index, ch) in text.char_indices() {
        let width = ch.len_utf16();
        if used + width > target {
            cut = index;
            break;
        }
        used += width;
    }
    let truncated = text.get(..cut).unwrap_or(text);
    if let Some(space) = truncated.rfind(' ') {
        let head = truncated.get(..space).unwrap_or("");
        #[allow(clippy::cast_precision_loss)]
        let space_units = head.encode_utf16().count() as f64;
        #[allow(clippy::cast_precision_loss)]
        let threshold = target as f64 * 0.6;
        if space_units > threshold {
            return format!("{head}...");
        }
    }
    format!("{truncated}...")
}

// ---------------------------------------------------------------------------------------------
// 2. The metadata cache — the READER half of `metadata-cache.ts`
// ---------------------------------------------------------------------------------------------

/// `<agent_dir>/mcp-cache.json`, the whole file.
///
/// **Reader only.** The writer (`computeServerHash`, `stableStringify`, the serialisers) is
/// `cache.rs`'s job in 13c; this module holds only what `installMcpAdapter` reads synchronously at
/// load. Every field is lenient (`Option`, `#[serde(default)]`) because upstream casts the parsed
/// JSON without validating it, and because a foreign writer — a co-installed `pi-mcp-adapter` — owns
/// this file just as much as cyrup does. Unknown keys (`uiResourceUri`, `uiStreamMode`, cut with
/// Cut 2) are ignored, never rejected, and never renumber [`METADATA_CACHE_VERSION`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MetadataCache {
    #[serde(default)]
    pub version: u32,
    /// Server order is the **cache file's** order, which is why this is an `IndexMap`: the prompt
    /// loop iterates it directly (`Object.entries(cache.servers)`), so a `BTreeMap` here would
    /// reorder `/mcp__*` command registration and change which of two colliding prompt commands
    /// wins.
    #[serde(default)]
    pub servers: IndexMap<String, ServerCacheEntry>,
}

/// One server's cached discovery result.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCacheEntry {
    /// The 64-hex config-identity digest — see [`install_server_hasher`].
    #[serde(default)]
    pub config_hash: Option<String>,
    #[serde(default)]
    pub tools: Option<Vec<CachedTool>>,
    #[serde(default)]
    pub resources: Option<Vec<CachedResource>>,
    #[serde(default)]
    pub prompts: Option<Vec<CachedPrompt>>,
    #[serde(default)]
    pub instructions: Option<String>,
    /// Epoch milliseconds. Upstream rejects a non-number outright
    /// (`!entry.cachedAt || typeof entry.cachedAt !== "number"`), and so does
    /// [`is_server_cache_valid`].
    ///
    /// **The custom deserialiser is the whole point (MCP-145).** A plain `Option<f64>` does not
    /// turn `"cachedAt": "1760000000000"` into `None` — serde *errors*, the error propagates out of
    /// `serde_json::from_str::<MetadataCache>`, and [`load_metadata_cache`] answers `None` for the
    /// **entire file**. Upstream casts the parsed JSON without validating it and rejects only the
    /// one bad entry, so a foreign writer's malformed `cachedAt` would have cost every other
    /// server's cached tools here and none upstream. [`lenient_epoch_ms`] restores that: anything
    /// that is not a finite JSON number becomes `None`, which this entry — and only this entry —
    /// fails on.
    #[serde(default, deserialize_with = "lenient_epoch_ms")]
    pub cached_at: Option<f64>,
}

/// `Option<f64>` that answers `None` for **any** non-number instead of failing the parse — see
/// [`ServerCacheEntry::cached_at`].
///
/// A JSON `null` and an absent key both arrive as `None` (the `#[serde(default)]` covers absence);
/// a non-finite number cannot survive `serde_json`'s own number grammar, but `as_f64().filter(…)`
/// keeps the predicate total anyway.
fn lenient_epoch_ms<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Value>::deserialize(deserializer)?
        .and_then(|value| value.as_f64())
        .filter(|ms| ms.is_finite()))
}

impl ServerCacheEntry {
    /// `serverCache.tools ?? []`.
    #[must_use]
    pub fn tools(&self) -> &[CachedTool] {
        self.tools.as_deref().unwrap_or(&[])
    }
    /// `serverCache.resources ?? []`.
    #[must_use]
    pub fn resources(&self) -> &[CachedResource] {
        self.resources.as_deref().unwrap_or(&[])
    }
    /// `entry?.prompts ?? []`.
    #[must_use]
    pub fn prompts(&self) -> &[CachedPrompt] {
        self.prompts.as_deref().unwrap_or(&[])
    }
}

/// `types.ts` `CachedTool`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedTool {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub input_schema: Option<Value>,
    /// Kept as a raw [`Value`], not `Option<Vec<String>>`: [`is_ui_tool_visible_to_model`]'s
    /// fail-closed semantics depend on telling "absent" from "present but malformed", which a
    /// lenient derive flattens (MCP-208).
    #[serde(default)]
    pub ui_visibility: Option<Value>,
}

/// `types.ts` `CachedResource`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CachedResource {
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// `types.ts` `CachedPrompt`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CachedPrompt {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub arguments: Option<Vec<CachedPromptArgument>>,
}

/// One `prompts/list` argument, as cached.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct CachedPromptArgument {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// `ui-tool-visibility.ts` `isUiToolVisibleToModel` over an already-extracted cached value
/// (MCP-208). Absent → visible; an array containing `"model"` → visible; **anything else present →
/// hidden**, which is the fail-closed direction: a server that marked a tool app-only still means
/// "not for the model" even though cyrup has no app surface (Cut 2).
#[must_use]
pub fn is_ui_tool_visible_to_model(ui_visibility: Option<&Value>) -> bool {
    match ui_visibility {
        None | Some(Value::Null) => true,
        Some(Value::Array(entries)) => {
            entries.iter().any(|entry| entry.as_str() == Some("model"))
        }
        Some(_) => false,
    }
}

/// The `computeServerHash` seam (13c / `cache.rs`), installed once per process.
pub type ServerHasher = fn(&ServerEntry) -> Option<String>;

static SERVER_HASHER: OnceLock<ServerHasher> = OnceLock::new();

/// Install the config-identity hasher [`is_server_cache_valid`] compares `configHash` against.
///
/// **Why this is a seam and not a call.** `computeServerHash` is a 64-hex SHA-256 over
/// `stableStringify` of fourteen *interpolated* fields — it needs `interpolateEnvRecord`,
/// `resolveServerUrl`, `resolveBearerToken` and `resolveConfigPath`, all of which belong to
/// `util.rs`/`cache.rs`, and it must be **byte-identical** to the digest
/// `cyrup_ext_subagents::exec::mcp_direct_tools` computes when it reads the same file, or every
/// `mcp:` subagent tool selector silently resolves to nothing. Writing a second copy here is
/// precisely the failure the port plan names as its hardest external constraint, so this module
/// takes the one implementation by reference instead of minting a rival.
///
/// Returns `false` when a hasher was already installed (set-once, like the upstream module-scope
/// import it stands in for).
///
/// **The seam is now an override, not a prerequisite (MCP-145).** It shipped with no production
/// installer, and the consequence was documented here and unfixed: the hash comparison was
/// *skipped entirely*, so validity rested on `cachedAt` alone and a server whose definition had
/// changed since the cache was written still registered its stale direct tools — upstream would
/// have skipped the server and shown the proxy tool instead. [`default_server_hasher`] closes that:
/// nothing has to be installed for the comparison to happen, and installing something only
/// *replaces* the default (a test double, or a future caller that wants a different `home`).
///
/// This is deliberately not "call `dirs::compute_server_hash` inline". The digest has to be
/// byte-identical to the one `cyrup_ext_subagents::exec::mcp_direct_tools` computes over the same
/// file, so there is exactly one implementation of it in the crate and this module reaches it
/// through one named function rather than minting a rival.
pub fn install_server_hasher(hasher: ServerHasher) -> bool {
    SERVER_HASHER.set(hasher).is_ok()
}

/// `computeServerHash(definition)` as [`is_server_cache_valid`] uses it when nothing was installed
/// — the crate's real digest, with upstream's throw expressed as `None`.
///
/// `None` has exactly one source: [`crate::credentials::resolve_server_url`] rejected the `url`
/// (a placeholder naming an unset variable, or a string `new URL()` cannot parse). Upstream wraps
/// `computeServerHash` in a `try` and answers "not valid" on a throw, which is the sole mechanism
/// keeping a `url: "https://x/${MISSING}"` server out of the cold-start direct-tool surface.
///
/// # `home`
///
/// `resolveConfigPath(definition.cwd)` expands a leading `~`, so the digest depends on a home
/// directory. This uses [`crate::dirs::home_dir`], which is now the workspace's one ladder
/// ([`cyrup_config::paths::cyrup_home_dir_from`]) — the same one the in-tree **reader**
/// (`mcp_direct_tools::home_dir`) resolves through. They agreed only when `CYRUP_HOME` was unset
/// before; they agree unconditionally now. That was the narrow tail of MCP-139's agent-dir axis 3,
/// and the shared resolver it asked for is what closes it.
#[must_use]
pub fn default_server_hasher(definition: &ServerEntry) -> Option<String> {
    crate::dirs::try_compute_server_hash(
        definition,
        &crate::secrets::PROCESS_ENV,
        &crate::dirs::home_dir(),
    )
    .ok()
}

/// The installed hasher, or [`default_server_hasher`].
fn server_hasher() -> ServerHasher {
    SERVER_HASHER.get().copied().unwrap_or(default_server_hasher as ServerHasher)
}

fn now_ms() -> f64 {
    #[allow(clippy::cast_precision_loss)]
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

/// `metadata-cache.ts` `loadMetadataCache` — **never fails**: a missing file, unreadable bytes,
/// malformed JSON or a version mismatch all yield `None`, which downstream means "no direct tools,
/// no prompt commands, proxy tool only".
#[must_use]
pub fn load_metadata_cache(dirs: &McpDirs) -> Option<MetadataCache> {
    let raw = std::fs::read_to_string(dirs.metadata_cache()).ok()?;
    let cache: MetadataCache = serde_json::from_str(&raw).ok()?;
    if cache.version != METADATA_CACHE_VERSION {
        return None;
    }
    Some(cache)
}

/// `isServerCacheValid(entry, definition, maxAgeMs)` (`metadata-cache.ts:114`) — MCP-145, all four
/// rejections, in upstream's order.
///
/// ```text
/// let configHash; try { configHash = computeServerHash(definition) } catch { return false }
/// if (!entry || entry.configHash !== configHash) return false;
/// if (!entry.cachedAt || typeof entry.cachedAt !== "number") return false;
/// if (maxAgeMs > 0 && Date.now() - entry.cachedAt > maxAgeMs) return false;
/// return true;
/// ```
///
/// 1. **The throw arm.** `computeServerHash` runs inside a `try` and **any** throw means invalid.
///    [`server_hasher`] answers `None` for it. This is not a defensive nicety: it is the only
///    thing that keeps a `url: "https://x/${MISSING}"` server out of the cold-start direct-tool
///    surface, where it would advertise tools no call could ever reach.
/// 2. A `configHash` that does not match. Absent counts as not matching.
/// 3. A **falsy or non-numeric** `cachedAt` — `!entry.cachedAt` rejects `0` as well as absent, and
///    the `typeof` test rejects a JSON string, which [`lenient_epoch_ms`] turns into `None`.
/// 4. An age over `max_age_ms`, checked **only when that limit is positive** — so `0` disables the
///    age check entirely and a year-old entry is accepted.
#[must_use]
pub fn is_server_cache_valid(
    entry: &ServerCacheEntry,
    definition: &ServerEntry,
    max_age_ms: f64,
) -> bool {
    // Upstream wraps `computeServerHash` in a try/catch and answers `false` on a throw —
    // `resolveServerUrl` throws on an uninterpolatable URL. `None` is that throw.
    let Some(hash) = server_hasher()(definition) else {
        return false;
    };
    if entry.config_hash.as_deref() != Some(hash.as_str()) {
        return false;
    }
    // `!entry.cachedAt` is falsy-testing a number, so `0` is rejected alongside absent.
    let Some(cached_at) = entry.cached_at.filter(|value| value.is_finite() && *value != 0.0) else {
        return false;
    };
    if max_age_ms > 0.0 && now_ms() - cached_at > max_age_ms {
        return false;
    }
    true
}

/// The valid cache entry for `server_name`, or `None` — the exact guard every loop below opens with.
///
/// `pub(crate)` for MCP-021: `live::rehydrate_from_cache` opens with the identical
/// `cachedEntry && isServerCacheValid(cachedEntry, definition)` test (`init.ts:256`), and
/// re-deriving it there — in particular by reaching for `dirs::try_compute_server_hash` instead of
/// the [`server_hasher`] seam [`is_server_cache_valid`] already goes through — is the reader/writer
/// hash drift this module exists to prevent.
pub(crate) fn valid_entry<'a>(
    cache: Option<&'a MetadataCache>,
    server_name: &str,
    definition: &ServerEntry,
) -> Option<&'a ServerCacheEntry> {
    let entry = cache?.servers.get(server_name)?;
    is_server_cache_valid(entry, definition, METADATA_CACHE_MAX_AGE_MS).then_some(entry)
}

// ---------------------------------------------------------------------------------------------
// 3. `MCP_DIRECT_TOOLS` — the env override (MCP-219)
// ---------------------------------------------------------------------------------------------

/// A parsed `MCP_DIRECT_TOOLS` selection: whole servers, and per-server tool sets.
#[derive(Debug, Clone, Default)]
pub struct DirectToolSelection {
    pub servers: HashSet<String>,
    pub tools: IndexMap<String, HashSet<String>>,
}

/// `metadata-cache.ts` `parseDirectToolSelectors` (MCP-219).
///
/// The subtlety is `selector.split("/", 2)`: **JS `split` with a limit discards the third segment**
/// rather than folding it into the second, so `a/b/c` selects tool `b` on server `a` and `c` is
/// dropped. Trailing slashes are stripped first, so `d/` is the whole-server selector `d`.
#[must_use]
pub fn parse_direct_tool_selectors(selectors: &[String]) -> DirectToolSelection {
    let mut out = DirectToolSelection::default();
    for raw in selectors {
        let selector = raw.trim_end_matches('/');
        if selector.contains('/') {
            let mut parts = selector.split('/');
            let server = parts.next().unwrap_or("");
            let tool = parts.next().unwrap_or("");
            if !server.is_empty() && !tool.is_empty() {
                out.tools.entry(server.to_string()).or_default().insert(tool.to_string());
            } else if !server.is_empty() {
                out.servers.insert(server.to_string());
            }
        } else if !selector.is_empty() {
            out.servers.insert(selector.to_string());
        }
    }
    out
}

/// Which tools of one server the direct surface may take.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolFilter {
    /// `true` — every allowed tool.
    All,
    /// `string[]` — only these **unprefixed** names.
    Named(Vec<String>),
    /// `false` — the server contributes no direct tools at all.
    Off,
}

impl ToolFilter {
    fn admits(&self, bare_name: &str) -> bool {
        match self {
            Self::All => true,
            Self::Named(names) => names.iter().any(|name| name == bare_name),
            Self::Off => false,
        }
    }
}

/// `resolveDirectTools` step 3: the env selection wins outright, then the per-server `directTools`
/// *if present at all* (so an explicit `false` beats a global `true`), then the global.
fn resolve_tool_filter(
    server_name: &str,
    definition: &ServerEntry,
    settings: Option<&McpSettings>,
    env_selection: Option<&DirectToolSelection>,
) -> ToolFilter {
    if let Some(selection) = env_selection {
        if selection.servers.contains(server_name) {
            return ToolFilter::All;
        }
        return match selection.tools.get(server_name) {
            Some(tools) => ToolFilter::Named(tools.iter().cloned().collect()),
            None => ToolFilter::Off,
        };
    }
    match &definition.direct_tools {
        Some(BoolOrList::All(true)) => ToolFilter::All,
        Some(BoolOrList::All(false)) => ToolFilter::Off,
        Some(BoolOrList::Named(names)) => ToolFilter::Named(names.clone()),
        None => {
            if settings.and_then(|s| s.direct_tools) == Some(true) {
                ToolFilter::All
            } else {
                ToolFilter::Off
            }
        }
    }
}

/// `metadata-cache.ts` `getMissingConfiguredDirectToolServers` — every enabled server that *wants*
/// direct tools but has no valid cache entry. Feeds [`should_register_proxy_tool`] (MCP-218).
#[must_use]
pub fn missing_configured_direct_tool_servers(
    config: &McpConfig,
    cache: Option<&MetadataCache>,
    env_override: Option<&[String]>,
) -> Vec<String> {
    let env_selection = env_override.map(parse_direct_tool_selectors);
    let settings = config.settings.as_ref();
    let mut missing = Vec::new();
    for (server_name, definition) in config.enabled_servers() {
        let wants = resolve_tool_filter(server_name, definition, settings, env_selection.as_ref())
            != ToolFilter::Off;
        if !wants {
            continue;
        }
        if valid_entry(cache, server_name, definition).is_none() {
            missing.push(server_name.clone());
        }
    }
    missing
}

// ---------------------------------------------------------------------------------------------
// 4. `resolveDirectTools` — the spec list (MCP-212, `critical`)
// ---------------------------------------------------------------------------------------------

/// `types.ts` `DirectToolSpec` — one registered direct tool, minus the two Cut-2 UI fields.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectToolSpec {
    pub server_name: String,
    /// The MCP-side name (`list_sims`), or the `read_*` base name for a resource.
    pub original_name: String,
    /// The registered, model-visible name (`xcodebuild_list_sims`).
    pub prefixed_name: String,
    pub description: String,
    pub input_schema: Option<Value>,
    /// Set for a resource tool — the URI `resources/read` is called with.
    pub resource_uri: Option<String>,
}

/// The `directToolFingerprint` pre-image (13e §8). Field order is the literal's order, and
/// `Option::is_none` reproduces `JSON.stringify`'s dropping of `undefined`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectToolFingerprint<'a> {
    server_name: &'a str,
    original_name: &'a str,
    prefixed_name: &'a str,
    description: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_schema: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource_uri: Option<&'a str>,
}

/// `index.ts` `directToolFingerprint` — the change detector `syncDirectTools` diffs against
/// `registeredDirectTools` so an unchanged tool is never re-registered (which would invalidate the
/// provider's prompt-cache prefix for nothing).
///
/// The two UI keys are dropped with Cut 2, which 13e §8 explicitly sanctions: the fingerprint is
/// in-process state, not an on-disk contract. Nested `inputSchema` objects serialise in
/// `serde_json`'s key order rather than the wire order, which is *stable* — the only property a
/// fingerprint needs.
#[must_use]
pub fn direct_tool_fingerprint(spec: &DirectToolSpec) -> String {
    serde_json::to_string(&DirectToolFingerprint {
        server_name: &spec.server_name,
        original_name: &spec.original_name,
        prefixed_name: &spec.prefixed_name,
        description: &spec.description,
        input_schema: spec.input_schema.as_ref(),
        resource_uri: spec.resource_uri.as_deref(),
    })
    .unwrap_or_else(|_| spec.prefixed_name.clone())
}

/// Whether this server needs the expensive cross-server candidate index at all — upstream builds it
/// lazily, and only when the server actually carries `includeTools`/`excludeTools`.
fn has_tool_filters(definition: &ServerEntry) -> bool {
    definition.include_tools.as_ref().is_some_and(|v| !v.is_empty())
        || definition.exclude_tools.as_ref().is_some_and(|v| !v.is_empty())
}

/// The `getOtherCurrentCandidates` builder shared by `resolveDirectTools` and
/// `buildProxyDescription`: every **current** candidate name of every enabled server with a valid
/// cache entry — including this one, whose own candidates are subtracted by match count inside
/// [`CandidateIndex::has_other_current_match`].
fn build_candidate_index(
    config: &McpConfig,
    cache: Option<&MetadataCache>,
    global_prefix: ToolPrefix,
) -> CandidateIndex {
    let mut candidates = IndexSet::new();
    for (other_name, other_definition) in config.enabled_servers() {
        let Some(entry) = valid_entry(cache, other_name, other_definition) else {
            continue;
        };
        let other_prefix = resolve_tool_prefix(Some(other_definition), global_prefix);
        for tool in entry.tools() {
            if !is_ui_tool_visible_to_model(tool.ui_visibility.as_ref()) {
                continue;
            }
            candidates.extend(tool_name_candidates(&tool.name, other_name, other_prefix, false));
        }
        if other_definition.expose_resources != Some(false) {
            for resource in entry.resources() {
                let base = resource_base_tool_name(&resource.name);
                candidates.extend(tool_name_candidates(&base, other_name, other_prefix, false));
            }
        }
    }
    CandidateIndex::new(candidates)
}

/// `direct-tools.ts` `resolveDirectTools` (MCP-212) — the set of tools the model sees on turn 1.
///
/// `cache == None` → **empty**. There are no direct tools until a cache file exists; that is the
/// design, not a degradation, and it is why the proxy tool exists.
///
/// Iteration follows `mcpServers` **file order** (`Object.entries`), which is what decides which of
/// two colliding tools wins — hence `IndexMap` in [`McpConfig`], and hence the crate-wide ban on
/// round-tripping `mcp.json` through `serde_json::Value`.
///
/// The five warnings are `tracing::warn!` with byte-identical text (MCP-246): upstream's
/// `console.warn` goes to the log, not the transcript, and 75 direct tools must not become 75
/// toasts.
#[must_use]
pub fn resolve_direct_tools(
    config: &McpConfig,
    cache: Option<&MetadataCache>,
    global_prefix: ToolPrefix,
    env_override: Option<&[String]>,
) -> Vec<DirectToolSpec> {
    let mut specs: Vec<DirectToolSpec> = Vec::new();
    let Some(cache) = cache else {
        return specs;
    };

    let mut seen_names: HashSet<String> = HashSet::new();
    let env_selection = env_override.map(parse_direct_tool_selectors);
    let settings = config.settings.as_ref();

    for (server_name, definition) in config.mcp_servers.iter() {
        if definition.is_disabled() {
            continue;
        }
        let Some(entry) = valid_entry(Some(cache), server_name, definition) else {
            continue;
        };
        let filter =
            resolve_tool_filter(server_name, definition, settings, env_selection.as_ref());
        if filter == ToolFilter::Off {
            continue;
        }

        let effective_prefix = resolve_tool_prefix(Some(definition), global_prefix);
        let mut index = has_tool_filters(definition)
            .then(|| build_candidate_index(config, Some(cache), global_prefix));
        let include = definition.include_tools.as_deref();
        let exclude = definition.exclude_tools.as_deref();

        for tool in entry.tools() {
            if tool.name.is_empty() {
                // Upstream reads `tool.name` unguarded here and would throw on a nameless cached
                // tool, taking the whole extension load with it. `init` may not fail, and 13e's
                // MCP-207 note is explicit: the type system removes the hazard, so do not
                // reproduce the panic — skip the entry instead.
                continue;
            }
            if !is_ui_tool_visible_to_model(tool.ui_visibility.as_ref()) {
                continue;
            }
            if !filter.admits(&tool.name) {
                continue;
            }
            if !is_tool_allowed(
                &tool.name,
                server_name,
                effective_prefix,
                include,
                exclude,
                index.as_mut(),
            ) {
                continue;
            }
            let prefixed_name = format_tool_name(&tool.name, server_name, effective_prefix);
            if BUILTIN_NAMES.contains(&prefixed_name.as_str()) {
                tracing::warn!("MCP: skipping direct tool \"{prefixed_name}\" (collides with builtin)");
                continue;
            }
            if seen_names.contains(&prefixed_name) {
                tracing::warn!(
                    "MCP: skipping duplicate direct tool \"{prefixed_name}\" from \"{server_name}\""
                );
                continue;
            }
            seen_names.insert(prefixed_name.clone());
            specs.push(DirectToolSpec {
                server_name: server_name.clone(),
                original_name: tool.name.clone(),
                prefixed_name,
                description: tool.description.clone().unwrap_or_default(),
                input_schema: tool.input_schema.clone(),
                resource_uri: None,
            });
        }

        if definition.expose_resources == Some(false) {
            continue;
        }
        for resource in entry.resources() {
            // Resources carry no `uiVisibility` and are deliberately NOT visibility-filtered.
            let base_name = resource_base_tool_name(&resource.name);
            if !filter.admits(&base_name) {
                continue;
            }
            if !is_tool_allowed(
                &base_name,
                server_name,
                effective_prefix,
                include,
                exclude,
                index.as_mut(),
            ) {
                continue;
            }
            let prefixed_name = format_tool_name(&base_name, server_name, effective_prefix);
            if BUILTIN_NAMES.contains(&prefixed_name.as_str()) {
                tracing::warn!(
                    "MCP: skipping direct resource tool \"{prefixed_name}\" (collides with builtin)"
                );
                continue;
            }
            if seen_names.contains(&prefixed_name) {
                tracing::warn!(
                    "MCP: skipping duplicate direct resource tool \"{prefixed_name}\" from \"{server_name}\""
                );
                continue;
            }
            seen_names.insert(prefixed_name.clone());
            let description = resource
                .description
                .clone()
                .unwrap_or_else(|| format!("Read resource: {}", resource.uri));
            specs.push(DirectToolSpec {
                server_name: server_name.clone(),
                original_name: base_name,
                prefixed_name,
                description,
                input_schema: None,
                resource_uri: Some(resource.uri.clone()),
            });
        }
    }

    // `direct-tools.ts:227` (upstream `76a4ea3`, issue #358): the settings test runs FIRST and is
    // `!== false`, so an absent block still warns. This gates the *message* only — the advisory has
    // never been a cap, so suppressing it changes nothing about which specs register. That is the
    // whole point: the person who hit it meant to register 75 tools and wants the line to stop.
    if config.settings_or_default().warn_on_large_direct_tools()
        && specs.len() >= DIRECT_TOOLS_ADVISORY_THRESHOLD
    {
        tracing::warn!(
            "MCP: {} direct tools resolved. Each direct tool adds prompt context; README guidance recommends targeted sets of 5-20 tools and using the proxy or an explicit string[] when 75+ direct tools would be registered.",
            specs.len()
        );
    }

    specs
}

// ---------------------------------------------------------------------------------------------
// 4b. `buildToolMetadata` / `reconstructToolMetadata` — `state.toolMetadata`'s two writers
//     (MCP-207)
// ---------------------------------------------------------------------------------------------

/// `ui-app-bridge-helpers.ts:5` `RESOURCE_URI_META_KEY`.
const RESOURCE_URI_META_KEY: &str = "ui/resourceUri";

/// `ui-tool-visibility.ts:3` `extractUiToolVisibility(meta)` — the **live** half of MCP-208, over a
/// server's raw `_meta` map instead of a cache entry's already-extracted array.
///
/// Every malformed arm answers `Some(vec![])` rather than `None`, and that is the fail-closed
/// direction: an empty list does not contain `"model"`, so a tool whose `ui.visibility` is present
/// but unreadable is hidden from the model rather than shown to it. `None` means the server said
/// nothing at all, which is the only value that reads as "visible".
fn extract_ui_tool_visibility(
    meta: Option<&serde_json::Map<String, Value>>,
) -> Option<Vec<String>> {
    // `!ui || typeof ui !== "object" || Array.isArray(ui)` — an array `ui` is rejected *here* even
    // though `getNestedResourceUri` accepts one.
    let Some(Value::Object(ui)) = meta?.get("ui") else {
        return None;
    };
    // `visibility === undefined` — an explicit `null` is not `undefined` and falls through to `[]`.
    let visibility = ui.get("visibility")?;
    let Value::Array(entries) = visibility else {
        return Some(Vec::new());
    };
    let mut values: Vec<String> = Vec::new();
    for entry in entries {
        // `if (entry !== "model" && entry !== "app") return []` — ONE bad entry voids the list.
        let Some(text @ ("model" | "app")) = entry.as_str() else {
            return Some(Vec::new());
        };
        if !values.iter().any(|value| value == text) {
            values.push(text.to_string());
        }
    }
    Some(values)
}

/// `ui-app-bridge-helpers.ts:7-23` `getToolUiResourceUri(tool)`, reduced to the one observable that
/// survives Cut 2: whether it **throws**.
///
/// [`ToolMetadata`] carries no `uiResourceUri` — MCP Apps are cut — so the extracted value has
/// nowhere to go. The throw does: `tool-metadata.ts:100-104` catches it and pushes the tool's name
/// onto `failedTools`, and the tool is still registered without its URI. Answering `bool` rather
/// than `Result<Option<String>, _>` keeps that the only thing this computes.
fn ui_resource_uri_is_invalid(meta: Option<&serde_json::Map<String, Value>>) -> bool {
    let Some(meta) = meta else {
        return false;
    };
    // `getNestedResourceUri`: `ui.resourceUri` when `ui` is an object. An array `ui` reaches this
    // upstream too (`typeof [] === "object"`), where it simply has no `resourceUri` key.
    let nested = match meta.get("ui") {
        Some(Value::Object(ui)) => ui.get("resourceUri"),
        _ => None,
    };
    // `if (resourceUri === undefined) resourceUri = meta?.[RESOURCE_URI_META_KEY]` — a nested
    // `null` is not `undefined`, so it suppresses the fallback and then throws.
    let Some(resource_uri) = nested.or_else(|| meta.get(RESOURCE_URI_META_KEY)) else {
        return false;
    };
    !resource_uri.as_str().is_some_and(|text| text.starts_with("ui://"))
}

/// A cached `uiVisibility` in the shape [`ToolMetadata`] stores.
///
/// The cache holds whatever the writer put there — [`CachedTool::ui_visibility`] is a raw [`Value`]
/// precisely so [`is_ui_tool_visible_to_model`] can tell "absent" from "present but malformed" —
/// while [`ToolMetadata::ui_visibility`] is `Option<Vec<String>>`. The three arms agree with that
/// predicate exactly: absent stays absent (visible), an array keeps its string entries, and
/// anything else becomes the empty list, which contains no `"model"` and is therefore hidden.
fn cached_ui_visibility(value: Option<&Value>) -> Option<Vec<String>> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::Array(entries)) => {
            Some(entries.iter().filter_map(|entry| entry.as_str().map(str::to_string)).collect())
        }
        Some(_) => Some(Vec::new()),
    }
}

/// `tool-metadata.ts:18` `{ metadata, failedTools }`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BuiltToolMetadata {
    /// `state.toolMetadata.get(serverName)`'s new value.
    pub metadata: Vec<ToolMetadata>,
    /// Names whose `_meta.ui.resourceUri` extraction threw (`tool-metadata.ts:100-104`), plus the
    /// literal `"(unnamed)"` for a nameless tool (`:81`). A non-empty list becomes
    /// `MCP: {server} - {n} tools skipped` on the startup pass (`init.ts:356-361`).
    ///
    /// Only the nameless entry is actually skipped: a tool whose URI extraction threw is still
    /// registered, minus the URI there is nowhere to put.
    pub failed_tools: Vec<String>,
}

/// `tool-metadata.ts:9` `buildToolMetadata(...)` (MCP-207) — one connected server's live tool and
/// resource lists, resolved into the model-visible metadata `state.toolMetadata` holds.
///
/// `known_metadata` is the collision universe: `state.toolMetadata` from `updateServerMetadata`
/// (`init.ts:488`), or the startup snapshot from §12 pass two (`init.ts:340`, which also passes
/// `include_missing_configured_candidates = true`). The two are **not** interchangeable — the
/// startup snapshot exists precisely so pass two sees every server that connected, including ones
/// later in the map that `state.toolMetadata` does not carry yet.
///
/// Three details a paraphrase loses:
///
/// * **Gate order.** `isToolAllowed` → `formatToolName` → the `seenNames` **test** →
///   `uiVisibility` → the `seenNames` **reservation** (`:84-97`). The test at `:89` and the
///   `add` at `:97` are on either side of the visibility check, which is the whole subtlety: a
///   hidden tool does **not** consume its name, so the next tool that formats to the same name
///   still registers. Moving the `add` up one line changes which server wins a collision.
/// * **The resource arm applies no visibility filter and no [`BUILTIN_NAMES`] check** (`:117-136`).
///   The builtin drop list belongs to [`resolve_direct_tools`] alone — it guards what the model
///   can *call by name*, and `state.toolMetadata` is not that surface.
/// * **`input_schema` is carried through unnormalised.**
///   [`normalize_direct_tool_input_schema`] belongs to registration, not to metadata.
///
/// Unlike [`resolve_direct_tools`], which silently `continue`s for a nameless tool because it has
/// no channel to report one, a nameless tool here pushes `"(unnamed)"` onto
/// [`BuiltToolMetadata::failed_tools`] (`:80-83`).
#[allow(
    clippy::too_many_arguments,
    reason = "upstream's eight-parameter signature; every call site is a port of a `buildToolMetadata(...)` line and reordering or grouping them would break the correspondence"
)]
#[must_use]
pub fn build_tool_metadata(
    tools: &[rmcp::model::Tool],
    resources: &[rmcp::model::Resource],
    definition: &ServerEntry,
    server_name: &str,
    prefix: ToolPrefix,
    configured_servers: Option<&IndexMap<String, ServerEntry>>,
    known_metadata: Option<&IndexMap<String, Vec<ToolMetadata>>>,
    include_missing_configured_candidates: bool,
) -> BuiltToolMetadata {
    let mut metadata: Vec<ToolMetadata> = Vec::new();
    let mut failed_tools: Vec<String> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let effective_prefix = resolve_tool_prefix(Some(definition), prefix);
    let include = definition.include_tools.as_deref();
    let exclude = definition.exclude_tools.as_deref();

    // `hasToolFilters && configuredServers ? (() => { … })() : undefined` (`:26-77`).
    let mut index = match (has_tool_filters(definition), configured_servers) {
        (true, Some(configured_servers)) => {
            let mut candidates: IndexSet<String> = IndexSet::new();
            let mut additional_by_tool: HashMap<String, IndexSet<String>> = HashMap::new();
            let mut evaluated_tool_names: IndexSet<String> = IndexSet::new();

            for tool in tools {
                if tool.name.is_empty() {
                    continue;
                }
                evaluated_tool_names.insert(tool.name.to_string());
                candidates.extend(tool_name_candidates(
                    &tool.name,
                    server_name,
                    effective_prefix,
                    false,
                ));
            }
            if definition.expose_resources != Some(false) {
                for resource in resources {
                    let base_name = resource_base_tool_name(&resource.name);
                    // `evaluatedToolNames.add` runs BEFORE the name/uri guard (`:47-48`), so a
                    // resource missing either still contributes a speculative key.
                    evaluated_tool_names.insert(base_name.clone());
                    if !resource.name.is_empty() && !resource.uri.is_empty() {
                        candidates.extend(tool_name_candidates(
                            &base_name,
                            server_name,
                            effective_prefix,
                            false,
                        ));
                    }
                }
            }

            for (other_name, other_definition) in configured_servers {
                if other_name.as_str() == server_name {
                    continue;
                }
                let other_prefix = resolve_tool_prefix(Some(other_definition), prefix);
                match known_metadata.and_then(|known| known.get(other_name.as_str())) {
                    // An EMPTY vec is truthy in JS (`if (knownTools)`), so a server known to carry
                    // no tools takes this arm and contributes nothing — it does not fall through
                    // to the speculative one.
                    Some(known_tools) => {
                        for tool in known_tools {
                            candidates.insert(tool.name.clone());
                            candidates.extend(tool_name_candidates(
                                &tool.original_name,
                                other_name,
                                other_prefix,
                                false,
                            ));
                        }
                    }
                    // `else if (!knownMetadata || includeMissingConfiguredCandidates)` (`:60`) —
                    // a server with no metadata yet contributes only into the per-tool table.
                    None if known_metadata.is_none() || include_missing_configured_candidates => {
                        for tool_name in &evaluated_tool_names {
                            let additional =
                                additional_by_tool.entry(tool_name.clone()).or_default();
                            let names =
                                tool_name_candidates(tool_name, other_name, other_prefix, false);
                            // The `-`→`_` spellings are added ONLY under the startup pass's flag
                            // (`:68-72`), and AFTER the raw ones — the order the set keeps.
                            let normalized: Option<Vec<String>> =
                                include_missing_configured_candidates.then(|| {
                                    names.iter().map(|name| name.replace('-', "_")).collect()
                                });
                            additional.extend(names);
                            if let Some(normalized) = normalized {
                                additional.extend(normalized);
                            }
                        }
                    }
                    None => {}
                }
            }
            Some(CandidateIndex::with_additional(candidates, additional_by_tool))
        }
        _ => None,
    };

    for tool in tools {
        if tool.name.is_empty() {
            failed_tools.push("(unnamed)".to_string());
            continue;
        }
        if !is_tool_allowed(
            &tool.name,
            server_name,
            effective_prefix,
            include,
            exclude,
            index.as_mut(),
        ) {
            continue;
        }
        let name = format_tool_name(&tool.name, server_name, effective_prefix);
        if seen_names.contains(&name) {
            continue;
        }
        let meta = tool.meta.as_ref().map(|meta| &meta.0);
        let ui_visibility = extract_ui_tool_visibility(meta);
        if !crate::proxy::is_ui_tool_visible_to_model(ui_visibility.as_deref()) {
            continue;
        }
        seen_names.insert(name.clone());
        if ui_resource_uri_is_invalid(meta) {
            failed_tools.push(tool.name.to_string());
        }
        metadata.push(ToolMetadata {
            name,
            original_name: tool.name.to_string(),
            description: tool.description.as_deref().unwrap_or_default().to_string(),
            resource_uri: None,
            ui_visibility,
            // `rmcp` makes `inputSchema` required where the wire type has it optional, so the
            // `...(tool.inputSchema !== undefined ? … : {})` spread is always taken.
            input_schema: Some(Value::Object((*tool.input_schema).clone())),
        });
    }

    if definition.expose_resources != Some(false) {
        for resource in resources {
            let base_name = resource_base_tool_name(&resource.name);
            if !is_tool_allowed(
                &base_name,
                server_name,
                effective_prefix,
                include,
                exclude,
                index.as_mut(),
            ) {
                continue;
            }
            let name = format_tool_name(&base_name, server_name, effective_prefix);
            if seen_names.contains(&name) {
                continue;
            }
            seen_names.insert(name.clone());
            metadata.push(ToolMetadata {
                name,
                original_name: base_name,
                description: resource
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("Read resource: {}", resource.uri)),
                resource_uri: Some(resource.uri.clone()),
                ui_visibility: None,
                input_schema: None,
            });
        }
    }

    BuiltToolMetadata { metadata, failed_tools }
}

/// `metadata-cache.ts:185` `reconstructToolMetadata(...)` (MCP-207) — [`build_tool_metadata`]'s
/// walk over a cache entry instead of a live connection, for the servers §10 rehydrates before
/// anything connects.
///
/// This is **not** [`resolve_direct_tools`]. That one additionally applies the `directTools`
/// selector and the [`BUILTIN_NAMES`] collision check, neither of which belongs to
/// `state.toolMetadata`: the cached direct-tool surface is what the model may call, and this is
/// what the proxy may resolve.
///
/// Four differences from [`build_tool_metadata`], all of them upstream's:
///
/// * there is no `failedTools` channel — a nameless cached tool is simply skipped;
/// * visibility is checked **before** `isToolAllowed` and `formatToolName` (`:221-232`), where the
///   live walk checks it between the `seenNames` test and the `seenNames` reservation; neither
///   reserves a name for a hidden tool, so the reordering is observable only in how much work is
///   done before the drop;
/// * a resource with no `name` **or** no `uri` is skipped (`:247`), where the live walk keeps it;
/// * the index's other-server arm reads each server's own cache entry through
///   [`is_server_cache_valid`] (`:203`) rather than a `known_metadata` map, and therefore also
///   drops disabled servers.
#[must_use]
pub fn reconstruct_tool_metadata(
    server_name: &str,
    entry: &ServerCacheEntry,
    prefix: ToolPrefix,
    definition: &ServerEntry,
    configured_servers: Option<&IndexMap<String, ServerEntry>>,
    cache: Option<&MetadataCache>,
) -> Vec<ToolMetadata> {
    let mut metadata: Vec<ToolMetadata> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let effective_prefix = resolve_tool_prefix(Some(definition), prefix);
    let include = definition.include_tools.as_deref();
    let exclude = definition.exclude_tools.as_deref();

    // `hasToolFilters && configuredServers && cache ? (() => { … })() : undefined` (`:199-217`).
    let mut index = match (has_tool_filters(definition), configured_servers, cache) {
        (true, Some(configured_servers), Some(cache)) => {
            let mut candidates: IndexSet<String> = IndexSet::new();
            for (other_name, other_definition) in configured_servers {
                // `!otherEntry || !isServerCacheValid(...) || isServerDisabled(...)`, in upstream's
                // order. Note this arm does NOT skip `serverName` itself: the index deliberately
                // spans every server, and the tool's own candidates are subtracted by match count
                // inside `CandidateIndex::has_other_current_match`.
                let Some(other_entry) = valid_entry(Some(cache), other_name, other_definition)
                else {
                    continue;
                };
                if other_definition.is_disabled() {
                    continue;
                }
                let other_prefix = resolve_tool_prefix(Some(other_definition), prefix);
                for other_tool in other_entry.tools() {
                    if !is_ui_tool_visible_to_model(other_tool.ui_visibility.as_ref()) {
                        continue;
                    }
                    candidates.extend(tool_name_candidates(
                        &other_tool.name,
                        other_name,
                        other_prefix,
                        false,
                    ));
                }
                if other_definition.expose_resources != Some(false) {
                    for resource in other_entry.resources() {
                        let base_name = resource_base_tool_name(&resource.name);
                        candidates.extend(tool_name_candidates(
                            &base_name,
                            other_name,
                            other_prefix,
                            false,
                        ));
                    }
                }
            }
            Some(CandidateIndex::new(candidates))
        }
        _ => None,
    };

    for tool in entry.tools() {
        if tool.name.is_empty() {
            continue;
        }
        if !is_ui_tool_visible_to_model(tool.ui_visibility.as_ref()) {
            continue;
        }
        if !is_tool_allowed(
            &tool.name,
            server_name,
            effective_prefix,
            include,
            exclude,
            index.as_mut(),
        ) {
            continue;
        }
        let name = format_tool_name(&tool.name, server_name, effective_prefix);
        if seen_names.contains(&name) {
            continue;
        }
        seen_names.insert(name.clone());
        metadata.push(ToolMetadata {
            name,
            original_name: tool.name.clone(),
            description: tool.description.clone().unwrap_or_default(),
            resource_uri: None,
            ui_visibility: cached_ui_visibility(tool.ui_visibility.as_ref()),
            input_schema: tool.input_schema.clone(),
        });
    }

    if definition.expose_resources != Some(false) {
        for resource in entry.resources() {
            // `if (!resource?.name || !resource?.uri) continue` (`:247`) — the live walk has no
            // such guard, because a live `resources/list` cannot omit either.
            if resource.name.is_empty() || resource.uri.is_empty() {
                continue;
            }
            let base_name = resource_base_tool_name(&resource.name);
            if !is_tool_allowed(
                &base_name,
                server_name,
                effective_prefix,
                include,
                exclude,
                index.as_mut(),
            ) {
                continue;
            }
            let name = format_tool_name(&base_name, server_name, effective_prefix);
            if seen_names.contains(&name) {
                continue;
            }
            seen_names.insert(name.clone());
            metadata.push(ToolMetadata {
                name,
                original_name: base_name,
                description: resource
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("Read resource: {}", resource.uri)),
                resource_uri: Some(resource.uri.clone()),
                ui_visibility: None,
                input_schema: None,
            });
        }
    }

    metadata
}

// ---------------------------------------------------------------------------------------------
// 5. `buildProxyDescription` (MCP-213)
// ---------------------------------------------------------------------------------------------

/// `direct-tools.ts` `buildProxyDescription` (MCP-213) — the model's entire map of what MCP can do,
/// and the **prompt-cache key**: `syncProxyTool` re-registers the proxy tool only when this string
/// differs, so a non-deterministic rebuild invalidates the provider's prompt cache on every
/// metadata refresh. Everything it reads is order-preserving for that reason.
///
/// Two literal edits from the cuts, both sanctioned by 13e §6:
/// * the header's `use mcpScript` sentence is **dropped** (Cut 4 — there is no JS runtime, and
///   advertising a tool that does not exist is worse than the text delta);
/// * the `mcp({ action: "ui-messages" })` usage line is **dropped** (Cut 2).
///
/// Everything else is byte-identical, column alignment included.
#[must_use]
pub fn build_proxy_description(
    config: &McpConfig,
    cache: Option<&MetadataCache>,
    direct_specs: &[DirectToolSpec],
) -> String {
    // **The head literal must stay byte-identical to `crate::proxy::build_proxy_description`'s.**
    // `direct-tools.ts:240` is `"…Non-MCP Pi tools should be called directly…"`; the one rebrand is
    // `Pi` → `cyrup`, and this copy had dropped the word entirely. Because
    // `McpExtension::proxy_tool_description` re-registers the gateway tool only when the
    // description *changed*, a one-word difference between the cache-built description (here) and
    // the live-metadata one (there) meant the guard could never fire — so every reconnect
    // re-registered the tool and invalidated the provider's prompt-cache prefix, which is the exact
    // cost `settings.freezeDirectTools` exists to avoid.
    let global_prefix = config.tool_prefix();
    let mut desc = String::from(
        "MCP gateway — server status, tool search/describe, auth, and single MCP tool calls. Non-MCP cyrup tools should be called directly, not through mcp.\n",
    );

    // 2. Direct tools, grouped by server in first-appearance order (a JS `Map`).
    let mut direct_by_server: IndexMap<&str, usize> = IndexMap::new();
    for spec in direct_specs {
        *direct_by_server.entry(spec.server_name.as_str()).or_insert(0) += 1;
    }
    if !direct_by_server.is_empty() {
        let parts: Vec<String> = direct_by_server
            .iter()
            .map(|(server, count)| format!("{server} ({count})"))
            .collect();
        desc.push_str(&format!(
            "\nDirect tools available (call as normal tools): {}\n",
            parts.join(", ")
        ));
    }

    // 3. Per-server proxy-reachable counts.
    let mut server_summaries: Vec<String> = Vec::new();
    for (server_name, definition) in config.mcp_servers.iter() {
        if definition.is_disabled() {
            continue;
        }
        let entry = valid_entry(cache, server_name, definition);
        let effective_prefix = resolve_tool_prefix(Some(definition), global_prefix);
        let mut index = (has_tool_filters(definition) && cache.is_some())
            .then(|| build_candidate_index(config, cache, global_prefix));
        let include = definition.include_tools.as_deref();
        let exclude = definition.exclude_tools.as_deref();

        let tool_count = entry.map_or(0, |entry| {
            entry
                .tools()
                .iter()
                .filter(|tool| {
                    is_ui_tool_visible_to_model(tool.ui_visibility.as_ref())
                        && is_tool_allowed(
                            &tool.name,
                            server_name,
                            effective_prefix,
                            include,
                            exclude,
                            index.as_mut(),
                        )
                })
                .count()
        });
        let resource_count = if definition.expose_resources == Some(false) {
            0
        } else {
            entry.map_or(0, |entry| {
                entry
                    .resources()
                    .iter()
                    .filter(|resource| {
                        is_tool_allowed(
                            &resource_base_tool_name(&resource.name),
                            server_name,
                            effective_prefix,
                            include,
                            exclude,
                            index.as_mut(),
                        )
                    })
                    .count()
            })
        };

        let total_items = tool_count + resource_count;
        if total_items == 0 {
            continue;
        }
        let direct_count = direct_by_server.get(server_name.as_str()).copied().unwrap_or(0);
        let proxy_count = total_items.saturating_sub(direct_count);
        if proxy_count > 0 {
            server_summaries.push(format!("{server_name} ({proxy_count} tools)"));
        }
    }
    if !server_summaries.is_empty() {
        desc.push_str(&format!("\nServers: {}\n", server_summaries.join(", ")));
    }

    // 4. Disabled servers.
    let disabled: Vec<&str> = config
        .mcp_servers
        .iter()
        .filter(|(_, definition)| definition.is_disabled())
        .map(|(name, _)| name.as_str())
        .collect();
    if !disabled.is_empty() {
        desc.push_str(&format!(
            "\nDisabled servers (enable with /mcp enable <server> and /reload): {}\n",
            disabled.join(", ")
        ));
    }

    // 5. Server instructions, whitespace-collapsed and word-truncated at 150.
    let mut instruction_summaries: Vec<String> = Vec::new();
    for (server_name, definition) in config.mcp_servers.iter() {
        if definition.is_disabled() {
            continue;
        }
        let Some(instructions) =
            valid_entry(cache, server_name, definition).and_then(|entry| entry.instructions.as_ref())
        else {
            continue;
        };
        if instructions.is_empty() {
            continue;
        }
        let collapsed = instructions.split_whitespace().collect::<Vec<_>>().join(" ");
        let snippet = truncate_at_word(&collapsed, INSTRUCTIONS_SNIPPET_LENGTH);
        instruction_summaries.push(format!("  {server_name}: {snippet}"));
    }
    if !instruction_summaries.is_empty() {
        desc.push_str(&format!(
            "\nServer instructions (truncated - full text via mcp({{ instructions: \"name\" }})):\n{}\n",
            instruction_summaries.join("\n")
        ));
    }

    // 6. The fixed usage block. Byte-identical, column alignment included, minus the `ui-messages`
    // line (Cut 2).
    desc.push_str("\nUsage:\n");
    desc.push_str("  mcp({ })                              → Show server status\n");
    desc.push_str("  mcp({ server: \"name\" })               → List tools from server\n");
    desc.push_str("  mcp({ search: \"query\" })              → Search MCP tools by name/description\n");
    desc.push_str("  mcp({ describe: \"tool_name\" })        → Show tool details and parameters\n");
    desc.push_str("  mcp({ instructions: \"name\" })         → Show full server usage instructions\n");
    desc.push_str("  mcp({ connect: \"server-name\" })       → Connect to a server and refresh metadata\n");
    desc.push_str("  mcp({ tool: \"name\", args: { key: \"value\" } })         → Call a tool (object args; JSON string also accepted)\n");
    desc.push_str("  mcp({ action: \"auth-start\", server: \"name\" })      → Start manual OAuth and get a browser URL\n");
    desc.push_str("  mcp({ action: \"auth-complete\", server: \"name\", args: { redirectUrl: \"...\" } }) → Complete manual OAuth\n");
    desc.push_str(
        "\nMode: action > tool (call) > connect > describe > instructions > search > server (list) > nothing (status)",
    );

    desc
}

/// `index.ts` `syncProxyTool`'s three-way predicate (MCP-218): the `mcp` tool is registered when the
/// setting does not disable it, **or** there are no direct specs, **or** some configured
/// direct-tool server has no valid cache entry.
///
/// That is: disabling the proxy only takes effect once the direct surface is genuinely complete —
/// otherwise a cold cache would leave the user with no MCP access at all. With HA-1 (MCP-217)
/// unbuilt this predicate runs **only** at init, which is where it matters most.
#[must_use]
pub fn should_register_proxy_tool(
    config: &McpConfig,
    cache: Option<&MetadataCache>,
    direct_specs: &[DirectToolSpec],
    env_override: Option<&[String]>,
) -> bool {
    config.settings_or_default().proxy_tool_enabled()
        || direct_specs.is_empty()
        || !missing_configured_direct_tool_servers(config, cache, env_override).is_empty()
}

// ---------------------------------------------------------------------------------------------
// 6. The registered tools (MCP-216, MCP-236, MCP-238, MCP-247)
// ---------------------------------------------------------------------------------------------

/// The late-bound executor behind every tool this module registers — the port of upstream's
/// `() => state` / `() => initPromise` closures (MCP-214 for [`Self::call_direct`], 13d's nine
/// modes for [`Self::call_proxy`]).
#[async_trait::async_trait]
pub trait McpToolDispatch: Send + Sync + 'static {
    /// One direct tool call: `tools/call`, or `resources/read` when `spec.resource_uri` is set.
    async fn call_direct(
        &self,
        spec: &DirectToolSpec,
        call_id: ToolCallId,
        params: Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError>;

    /// One `mcp({...})` gateway call.
    async fn call_proxy(
        &self,
        call_id: ToolCallId,
        params: Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError>;
}

/// The slot every tool registered in one `init()` pass shares.
///
/// Set-once per GENERATION — not per pass. cyrup re-runs `init` on the *same*
/// `Arc<dyn NativeExtension>` for each session generation (see the crate docs' ordering
/// inversion), and each of those mints a fresh executor because it mints fresh tool objects.
/// A LIVE re-sync (`McpExtension::sync_tool_surface`) is a second pass INSIDE one generation and
/// must reuse the generation's existing `Arc`: `install` is a `OnceLock::set`, so a tool bound to a
/// freshly-defaulted slot nobody can install answers `MCP not initialized` for the rest of the
/// session. That is why [`register_surface`] takes the executor as an explicit parameter rather
/// than defaulting one — the choice of executor is the caller's, and making it invisible is
/// precisely how it went wrong.
///
/// [`RegisteredSurface::dispatch`] echoes the same `Arc` back, and whoever owns the generation
/// installs the real dispatch into it once [`crate::state::McpState`] exists.
#[derive(Default)]
pub struct ToolDispatch {
    slot: OnceLock<Arc<dyn McpToolDispatch>>,
}

impl std::fmt::Debug for ToolDispatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolDispatch").field("installed", &self.is_installed()).finish()
    }
}

impl ToolDispatch {
    /// Install the executor. `false` when one was already installed (the call is a no-op then).
    pub fn install(&self, dispatch: Arc<dyn McpToolDispatch>) -> bool {
        self.slot.set(dispatch).is_ok()
    }

    #[must_use]
    pub fn get(&self) -> Option<&Arc<dyn McpToolDispatch>> {
        self.slot.get()
    }

    #[must_use]
    pub fn is_installed(&self) -> bool {
        self.slot.get().is_some()
    }
}

/// `direct-tools.ts`'s step-3 return: a **successful** tool result carrying `details.error`, never
/// an `Err`. `cyrup-core`'s own module doc says tools signal failure with `Err(ToolError)`; an MCP
/// tool error is a deliberate divergence, because it is a successful execution *reporting* a remote
/// or pre-flight failure, and `Err` would lose `details` — which is what `error-signal.ts`'s
/// `{isError:true}` override reads (MCP-249).
fn not_initialized_result() -> ToolResult {
    ToolResult {
        content: vec![Content::text("MCP not initialized")],
        details: Some(json!({ "error": "not_initialized" })),
        ..ToolResult::default()
    }
}

/// `utils.ts` `normalizeDirectToolInputSchema` (MCP-216): a non-object schema becomes
/// `{type:"object", properties:{}}`, and `$schema` **and `additionalProperties`** are stripped.
///
/// The `additionalProperties` strip is not cosmetic — providers that reject an open schema reject
/// the whole tool list, taking every other tool down with it. Upstream then wraps the result in
/// TypeBox's `Type.Unsafe`; `cyrup_core::Tool::parameters` is raw JSON Schema, so that shim
/// disappears.
#[must_use]
pub fn normalize_direct_tool_input_schema(schema: Option<&Value>) -> Value {
    match schema {
        Some(Value::Object(map)) => {
            let mut map = map.clone();
            map.remove("$schema");
            map.remove("additionalProperties");
            Value::Object(map)
        }
        _ => json!({ "type": "object", "properties": {} }),
    }
}

/// `index.ts`'s `toolRenderShell` (MCP-238): `"self"` in compact mode, `"default"` otherwise, with
/// `toolResultRendering` defaulting to `compact`.
#[must_use]
pub fn tool_render_kind(settings: Option<&McpSettings>) -> ToolRenderKind {
    match settings.and_then(|s| s.tool_result_rendering) {
        Some(ToolResultRendering::Boxed) => ToolRenderKind::Default,
        _ => ToolRenderKind::SelfRendered,
    }
}

/// One registered direct tool — `index.ts` `registerDirectTool`'s shape (MCP-216).
///
/// Every string is **owned and computed once at construction**, because `cyrup_core::Tool` returns
/// `&str`/`Option<&str>` and a per-call `format!` would allocate on every system-prompt rebuild.
pub struct DirectTool {
    spec: DirectToolSpec,
    label: String,
    description: String,
    prompt_snippet: String,
    parameters: Value,
    render_kind: ToolRenderKind,
    dispatch: Arc<ToolDispatch>,
}

impl DirectTool {
    #[must_use]
    pub fn new(
        spec: DirectToolSpec,
        render_kind: ToolRenderKind,
        dispatch: Arc<ToolDispatch>,
    ) -> Self {
        let label = format!("MCP: {}", spec.original_name);
        let description = if spec.description.is_empty() {
            "(no description)".to_string()
        } else {
            spec.description.clone()
        };
        let snippet = truncate_at_word(&spec.description, DIRECT_TOOL_PROMPT_SNIPPET_LENGTH);
        let prompt_snippet = if snippet.is_empty() {
            format!("MCP tool from {}", spec.server_name)
        } else {
            snippet
        };
        let parameters = normalize_direct_tool_input_schema(spec.input_schema.as_ref());
        Self { spec, label, description, prompt_snippet, parameters, render_kind, dispatch }
    }

    /// The spec this tool was built from — the executor's entire input besides the call arguments.
    #[must_use]
    pub fn spec(&self) -> &DirectToolSpec {
        &self.spec
    }
}

#[async_trait::async_trait]
impl Tool for DirectTool {
    fn name(&self) -> &str {
        &self.spec.prefixed_name
    }
    fn parameters(&self) -> &Value {
        &self.parameters
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn label(&self) -> Option<&str> {
        Some(&self.label)
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some(&self.prompt_snippet)
    }
    fn render_kind(&self) -> ToolRenderKind {
        self.render_kind
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        params: Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        match self.dispatch.get() {
            Some(dispatch) => {
                dispatch.call_direct(&self.spec, call_id, params, cancel, on_update).await
            }
            None => Ok(not_initialized_result()),
        }
    }
}

/// The `mcp` gateway tool — `index.ts` `registerProxyTool`'s shape (MCP-247).
pub struct ProxyTool {
    description: String,
    parameters: Value,
    render_kind: ToolRenderKind,
    dispatch: Arc<ToolDispatch>,
}

impl ProxyTool {
    #[must_use]
    pub fn new(
        description: String,
        render_kind: ToolRenderKind,
        dispatch: Arc<ToolDispatch>,
    ) -> Self {
        Self { description, parameters: proxy_tool_parameters(), render_kind, dispatch }
    }
}

/// The proxy tool's parameter schema — twelve optional properties (MCP-247).
///
/// **Five of these names are a cross-crate contract**, not documentation:
/// `cyrup_permission_system::manager`'s `create_mcp_permission_targets` reads `tool`, `server`,
/// `connect`, `describe` and `search`, in that precedence, with `mcp_status` as the fallthrough.
/// Renaming or omitting any of the five silently disables the corresponding permission targets —
/// the gate stops seeing the call it was written to gate.
///
/// `action`'s description loses its `'ui-messages'` mention (Cut 2); the property itself stays,
/// because `auth-start` / `auth-complete` still use it. Upstream's TypeBox shims (`Type.Optional`,
/// `optionalNumber`) disappear: `Tool::parameters` is raw JSON Schema, and "optional" is simply the
/// absence of a `required` array.
#[must_use]
pub fn proxy_tool_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "tool": {
                "type": "string",
                "description": "Tool name to call (e.g., 'xcodebuild_list_sims')"
            },
            "args": {
                "anyOf": [
                    {
                        "type": "string",
                        "description": "Arguments as a JSON string (e.g., '{\"key\": \"value\"}')"
                    },
                    {
                        "type": "object",
                        "additionalProperties": true,
                        "description": "Arguments as a JSON object (e.g., { \"key\": \"value\" })"
                    }
                ],
                "description": "Tool arguments as a JSON object, or as a JSON string encoding one"
            },
            "connect": {
                "type": "string",
                "description": "Server name to connect (lazy connect + metadata refresh)"
            },
            "describe": {
                "type": "string",
                "description": "Tool name to describe (shows parameters)"
            },
            "instructions": {
                "type": "string",
                "description": "Server name to show that server's usage instructions"
            },
            "search": { "type": "string", "description": "Search tools by name/description" },
            "regex": {
                "type": "boolean",
                "description": "Treat search as regex (default: substring match)"
            },
            "includeSchemas": {
                "type": "boolean",
                "description": "Include parameter schemas in search results (default: true)"
            },
            "limit": {
                "type": "number",
                "minimum": 1,
                "description": "Maximum search results to return (default: 12)"
            },
            "offset": {
                "type": "number",
                "minimum": 0,
                "description": "Search result offset (default: 0)"
            },
            "server": {
                "type": "string",
                "description": "Filter to specific server (also disambiguates tool calls)"
            },
            "action": {
                "type": "string",
                "description": "Action: 'auth-start' or 'auth-complete'"
            }
        }
    })
}

#[async_trait::async_trait]
impl Tool for ProxyTool {
    fn name(&self) -> &str {
        PROXY_TOOL_NAME
    }
    fn parameters(&self) -> &Value {
        &self.parameters
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn label(&self) -> Option<&str> {
        Some("MCP")
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some(PROXY_TOOL_PROMPT_SNIPPET)
    }
    /// MCP-236 — the one guideline cyrup's prompt sanitizer keeps for a tool named `mcp`.
    fn prompt_guidelines(&self) -> Vec<&str> {
        vec![PROXY_TOOL_PROMPT_GUIDELINE]
    }
    fn render_kind(&self) -> ToolRenderKind {
        self.render_kind
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        params: Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        match self.dispatch.get() {
            Some(dispatch) => dispatch.call_proxy(call_id, params, cancel, on_update).await,
            None => Ok(not_initialized_result()),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 7. Prompt slash commands (MCP-206)
// ---------------------------------------------------------------------------------------------

/// One MCP prompt, resolved to the slash command it registers — upstream `PromptMetadata`
/// (`types.ts:584-591`), field for field.
///
/// **This is `crate::state::PromptMetadata`**: D0 discharged that forward declaration as a
/// `pub use` of this type, because upstream feeds `state.promptMetadata.values()` and
/// `resolveCachedPrompts(...)` into the same `registerPromptCommands(specs)` (`index.ts:280` and
/// `:283`). The cache path and the live path mint it through one function,
/// [`reconstruct_prompt_metadata`]; what still differs between them is *provenance*, which
/// `McpState::prompt_metadata_live` records separately, and which is why `prompts.ts` re-resolves
/// against the live map at invocation time (`findLivePromptMetadata`).
#[derive(Debug, Clone, PartialEq)]
pub struct PromptCommandSpec {
    pub server_name: String,
    pub original_name: String,
    pub command_name: String,
    pub title: Option<String>,
    pub description: String,
    pub arguments: Vec<CachedPromptArgument>,
}

/// The `McpPrompt | CachedPrompt` union `reconstructPromptMetadata` takes
/// (`metadata-cache.ts:320`), which Rust has to spell as a trait.
///
/// The two arms are the live `prompts/list` result and the cache entry; they carry the same four
/// fields under different types, and the argument lists differ only in element type. Bridging them
/// here rather than at the call sites is the point of the unification: every prompt command name
/// in the process is minted by one function over one shape.
pub trait PromptSource {
    /// `prompt.name` — the `filter(prompt => prompt?.name)` gate reads it before anything else.
    fn prompt_name(&self) -> &str;
    /// `prompt.title`.
    fn prompt_title(&self) -> Option<&str>;
    /// `prompt.description`.
    fn prompt_description(&self) -> Option<&str>;
    /// `Array.isArray(prompt.arguments) ? prompt.arguments.filter(a => a?.name).map(…) : []`
    /// (`metadata-cache.ts:326-332`) — absent and non-array both flatten to the empty list, and a
    /// nameless argument is dropped.
    fn prompt_arguments(&self) -> Vec<CachedPromptArgument>;
}

impl PromptSource for CachedPrompt {
    fn prompt_name(&self) -> &str {
        &self.name
    }
    fn prompt_title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    fn prompt_description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    fn prompt_arguments(&self) -> Vec<CachedPromptArgument> {
        self.arguments
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|argument| !argument.name.is_empty())
            .cloned()
            .collect()
    }
}

impl PromptSource for rmcp::model::Prompt {
    fn prompt_name(&self) -> &str {
        &self.name
    }
    fn prompt_title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    fn prompt_description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    fn prompt_arguments(&self) -> Vec<CachedPromptArgument> {
        self.arguments
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|argument| !argument.name.is_empty())
            .map(|argument| CachedPromptArgument {
                name: argument.name.clone(),
                description: argument.description.clone(),
                required: argument.required,
            })
            .collect()
    }
}

/// `metadata-cache.ts:318` `reconstructPromptMetadata(serverName, prompts, prefix, definition?)`
/// (MCP-207) — one server's prompt list, live or cached, as the slash commands it registers.
///
/// `definition` is optional upstream (`Pick<ServerEntry, "toolPrefix">`) and stays optional here,
/// which is exactly what [`resolve_tool_prefix`] now takes: a prompt resolved for a server with no
/// `mcpServers` entry falls through to the global mode instead of being unrepresentable.
#[must_use]
pub fn reconstruct_prompt_metadata<P: PromptSource>(
    server_name: &str,
    prompts: &[P],
    prefix: ToolPrefix,
    definition: Option<&ServerEntry>,
) -> Vec<PromptCommandSpec> {
    let effective_prefix = resolve_tool_prefix(definition, prefix);
    prompts
        .iter()
        .filter(|prompt| !prompt.prompt_name().is_empty())
        .map(|prompt| {
            let original_name = prompt.prompt_name();
            PromptCommandSpec {
                server_name: server_name.to_string(),
                original_name: original_name.to_string(),
                command_name: format_prompt_command_name(
                    original_name,
                    server_name,
                    effective_prefix,
                ),
                title: prompt.prompt_title().map(str::to_string),
                description: prompt.prompt_description().unwrap_or_default().to_string(),
                arguments: prompt.prompt_arguments(),
            }
        })
        .collect()
}

/// `prompts.ts:19` `resolveCachedPrompts`: walk the **cache's** server order, keep servers that are
/// configured, enabled, cache-valid and actually carry prompts, and hand each survivor's prompt
/// list to [`reconstruct_prompt_metadata`] — the same function the live path uses, so a command
/// name cannot depend on which path minted it.
#[must_use]
pub fn resolve_cached_prompts(
    config: &McpConfig,
    cache: Option<&MetadataCache>,
) -> Vec<PromptCommandSpec> {
    let mut specs = Vec::new();
    let Some(cache) = cache else {
        return specs;
    };
    let global_prefix = config.tool_prefix();

    for (server_name, entry) in &cache.servers {
        let Some(definition) = config.mcp_servers.get(server_name) else {
            continue;
        };
        if definition.is_disabled() || entry.prompts().is_empty() {
            continue;
        }
        if !is_server_cache_valid(entry, definition, METADATA_CACHE_MAX_AGE_MS) {
            continue;
        }
        specs.extend(reconstruct_prompt_metadata(
            server_name,
            entry.prompts(),
            global_prefix,
            Some(definition),
        ));
    }
    specs
}

/// `prompts.ts` `buildCommandDescription` — `MCP: <description|title|fallback>` truncated at 120,
/// with the fallback repeated when truncation empties the string.
#[must_use]
pub fn prompt_command_description(spec: &PromptCommandSpec) -> String {
    let fallback = format!("MCP prompt from {}", spec.server_name);
    let base = if !spec.description.is_empty() {
        spec.description.as_str()
    } else {
        match spec.title.as_deref() {
            Some(title) if !title.is_empty() => title,
            _ => fallback.as_str(),
        }
    };
    let described =
        truncate_at_word(&format!("MCP: {base}"), PROMPT_COMMAND_DESCRIPTION_LENGTH);
    if described.is_empty() { fallback } else { described }
}

/// `/mcp`'s descriptor. The description is `index.ts`'s literal.
///
/// `completions` carries the eight static subcommands — the first branch of upstream's
/// `getArgumentCompletions`, whose values are knowable at init. Its *second* branch (server names
/// for `reconnect`/`enable`/`disable`/`logout`) is only knowable at runtime and is served by the
/// dynamic argument-completion seam (HA-2 / MCP-041).
///
/// There is deliberately no `token` entry. Upstream declares eight subcommands and none of them is
/// `token` (`index.ts:476-485`); 13h's switch table gives it no arm. A ninth completion that falls
/// through to the default arm would open the browser panel — a completion that lies about what it
/// does.
#[must_use]
pub fn mcp_command_descriptor() -> CommandDescriptor {
    CommandDescriptor {
        description: "Show MCP server status".to_string(),
        completions: [
            "reconnect", "tools", "prompts", "setup", "logout", "disable", "enable", "status",
        ]
        .iter()
        .map(|value| (*value).to_string())
        .collect(),
    }
}

/// `/mcp-auth`'s descriptor.
#[must_use]
pub fn mcp_auth_command_descriptor() -> CommandDescriptor {
    CommandDescriptor {
        description: "Authenticate with an MCP server (OAuth)".to_string(),
        completions: Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// 8. `register_surface` — `installMcpAdapter`'s synchronous body (MCP-003)
// ---------------------------------------------------------------------------------------------

/// What one `init()` pass registered. Returned so [`crate::extension::McpExtension`] can seed the
/// cross-`init` fingerprint maps that make a session replacement re-register only what changed
/// (MCP-014, MCP-036) and can install the executor (MCP-214).
#[derive(Debug, Default)]
pub struct RegisteredSurface {
    /// Every tool name registered this pass, in registration order — the direct tools in config
    /// order first, then [`PROXY_TOOL_NAME`], exactly as `syncDirectTools` then `syncProxyTool`.
    /// `McpExtension::init` declares one renderer per entry.
    pub tool_names: Vec<String>,
    /// Every slash command registered this pass, in registration order: the prompt commands first
    /// (upstream registers them before anything else), then [`MCP_COMMAND`] and
    /// [`MCP_AUTH_COMMAND`].
    pub command_names: Vec<String>,
    /// The resolved direct-tool specs — the executor's input, and `/mcp tools`' cache-only listing.
    pub direct_tools: Vec<DirectToolSpec>,
    /// `prefixedName -> directToolFingerprint`, ready to seed
    /// `McpExtension::registered_direct_tools` so the next pass's diff is meaningful.
    pub direct_tool_fingerprints: IndexMap<String, String>,
    /// The proxy description as registered, or `None` when the proxy tool was not registered.
    /// Seeds `McpExtension::proxy_tool_description`, whose identity check is what preserves the
    /// provider's prompt-cache prefix.
    pub proxy_description: Option<String>,
    /// The prompt commands registered this pass, keyed in `command_names` order — the dispatcher
    /// (`NativeExtension::execute_command`) needs the server/prompt pair behind each name.
    pub prompt_commands: Vec<PromptCommandSpec>,
    /// The shared executor slot every registered tool reads. Install into it once
    /// [`crate::state::McpState`] exists; until then every MCP tool call answers
    /// `MCP not initialized`.
    pub dispatch: Arc<ToolDispatch>,
}

/// Where a registration pass writes its surface (HA-1 / MCP-037).
///
/// [`register_surface`] is upstream's `syncToolSurface` + `syncPromptCommands`: it resolves the
/// config and cache into a tool/command set. Upstream runs that ONE function from `init` and from
/// every live path (`onToolMetadataUpdated`, `mcp({connect})`, `/mcp reconnect`) because
/// `pi.registerTool` is legal from anywhere. cyrup has two different write targets for the same
/// resolution — [`InitApi`] during `init`, [`cyrup_ext::LateRegistrar`] after — so the resolution
/// is made generic over the target instead of being written twice.
///
/// Forking it was the alternative and is the wrong one: two resolution paths drift, and
/// reader/writer drift is exactly the failure class that produced this crate's `socket`,
/// `null`/`undefined` and `get_`/`read_` divergences. One path, two sinks.
pub trait SurfaceSink {
    fn register_tool(&mut self, tool: Arc<dyn cyrup_core::Tool>);
    fn register_command(&mut self, name: String, desc: cyrup_ext::CommandDescriptor);
    /// MCP-041's opt-in — `InitApi::add_autocomplete`. Defaulted to a no-op because the LATE sink
    /// has no such verb: a re-sync never re-registers `/mcp`, so it never needs to re-opt-in.
    fn add_autocomplete(&mut self, _command: String) {}
    fn register_tool_renderer(&mut self, tool_name: String);

    /// Declare the event subscription bitset. Load-time only: the bitset is read once when the
    /// extension is wired into the dispatcher, so a late pass has nothing to change and the late
    /// sink no-ops it. Upstream has the same asymmetry — `api.on(...)` mutates a handler map the
    /// runner already holds, while the subscription itself was fixed at load.
    fn subscribe(&mut self, _kinds: &[cyrup_ext::EventKind]) {}

    /// Declare a CLI flag. Load-time only for the same reason and with the same upstream
    /// precedent: flags are parsed before any handler can run, so registering one late could not
    /// affect the parse.
    fn register_flag(&mut self, _name: String, _spec: serde_json::Value) {}

    /// Whether a direct tool whose fingerprint is `fingerprint` needs (re-)registering.
    ///
    /// This is upstream's `if (previous !== fingerprint)` in `syncDirectTools`, and it is the
    /// SINK's business rather than the resolver's: `init` builds a fresh surface and registers
    /// everything, while a live re-sync must register only what changed, because re-registering
    /// identical bytes still rewrites the agent's tool array and the base system prompt and so
    /// invalidates the provider's prompt-cache prefix. One resolution pass, two policies.
    fn should_register_tool(&self, _name: &str, _fingerprint: &str) -> bool {
        true
    }

    /// Whether the proxy tool's description differs from the one it is currently registered with
    /// (`syncProxyTool`'s own comparison, MCP-043).
    fn should_register_proxy(&self, _description: &str) -> bool {
        true
    }

    /// Whether a prompt command still needs registering (`registeredPromptCommands`' dedup set).
    fn should_register_prompt_command(&self, _name: &str) -> bool {
        true
    }

    /// Whether this pass should register the FIXED commands — `/mcp` and `/mcp-auth`.
    ///
    /// True for `init`, false for a live re-sync. They neither appear nor change with the
    /// discovered surface, so re-registering them on every reconnect is churn plus a spurious
    /// `/` menu rebuild.
    ///
    /// A DISTINCT gate from [`Self::should_register_prompt_command`], not a reuse of it: that one
    /// consults `registeredPromptCommands`, which is seeded only from the per-server prompt specs
    /// and so never contains either fixed name — the gate could not have held them back however it
    /// was seeded, because they are not prompt commands.
    fn register_fixed_commands(&self) -> bool {
        true
    }
}

impl SurfaceSink for InitApi {
    fn add_autocomplete(&mut self, command: String) {
        InitApi::add_autocomplete(self, command);
    }

    fn register_tool(&mut self, tool: Arc<dyn cyrup_core::Tool>) {
        InitApi::register_tool(self, tool);
    }
    fn register_command(&mut self, name: String, desc: cyrup_ext::CommandDescriptor) {
        InitApi::register_command(self, name, desc);
    }
    fn register_tool_renderer(&mut self, tool_name: String) {
        InitApi::register_tool_renderer(self, tool_name);
    }
    fn subscribe(&mut self, kinds: &[cyrup_ext::EventKind]) {
        InitApi::subscribe(self, kinds);
    }
    fn register_flag(&mut self, name: String, spec: serde_json::Value) {
        InitApi::register_flag(self, name, spec);
    }
}

/// The post-`init` sink. Writes straight through to the registry; a failure is logged and the pass
/// continues, because one tool that will not register must not cost the other seven their
/// registration — upstream's `syncDirectTools` loop has no failure mode at all
/// (`pi.registerTool` returns `void`), so aborting the pass would be a cyrup-only behaviour.
pub struct LateSink {
    pub registrar: Arc<dyn cyrup_ext::LateRegistrar>,
    /// `registeredDirectTools` as of the last pass — `prefixedName -> fingerprint`.
    pub known_tools: indexmap::IndexMap<String, String>,
    /// `proxyToolDescription` as of the last pass.
    pub known_proxy: Option<String>,
    /// `registeredPromptCommands` as of the last pass.
    pub known_commands: indexmap::IndexMap<String, String>,
}

impl SurfaceSink for LateSink {
    fn should_register_tool(&self, name: &str, fingerprint: &str) -> bool {
        self.known_tools.get(name).map(String::as_str) != Some(fingerprint)
    }

    fn should_register_proxy(&self, description: &str) -> bool {
        self.known_proxy.as_deref() != Some(description)
    }

    fn should_register_prompt_command(&self, name: &str) -> bool {
        !self.known_commands.contains_key(name)
    }

    fn register_fixed_commands(&self) -> bool {
        false
    }

    fn register_tool(&mut self, tool: Arc<dyn cyrup_core::Tool>) {
        if let Err(e) = self.registrar.register_tool(tool) {
            tracing::warn!(error = %e, "MCP: late tool registration failed");
        }
    }
    fn register_command(&mut self, name: String, desc: cyrup_ext::CommandDescriptor) {
        if let Err(e) = self.registrar.register_command(name, desc) {
            tracing::warn!(error = %e, "MCP: late command registration failed");
        }
    }
    fn register_tool_renderer(&mut self, tool_name: String) {
        if let Err(e) = self.registrar.register_tool_renderer(tool_name) {
            tracing::warn!(error = %e, "MCP: late tool-renderer declaration failed");
        }
    }
}

/// Register the whole surface from disk caches. **Infallible by construction** — see the module
/// docs. There is no `?`, no `unwrap`, and no `Err` path in this function or anything it calls.
///
/// Order follows `installMcpAdapter` exactly, because registration order is user-visible: prompt
/// commands (upstream line-order first, so a prompt command named `mcp` could never displace the
/// real `/mcp`), then the `--mcp-config` flag, then `/mcp` and `/mcp-auth`, then
/// `syncDirectTools` and finally `syncProxyTool`.
pub fn register_surface<S: SurfaceSink + ?Sized>(
    api: &mut S,
    dirs: &McpDirs,
    config: &McpConfig,
    // The executor every tool this pass registers will read. `init` mints one per GENERATION; a
    // live re-sync passes the generation's EXISTING one, or its tools can never execute. See
    // [`ToolDispatch`] — this used to be defaulted here, which silently bound a re-sync's tools to
    // an empty slot nothing could install.
    dispatch: Arc<ToolDispatch>,
) -> RegisteredSurface {
    let mut surface =
        RegisteredSurface { dispatch: Arc::clone(&dispatch), ..RegisteredSurface::default() };
    api.subscribe(SUBSCRIBED_EVENTS);

    // `loadMetadataCache()` — `readFileSync`, defensive, `null` on anything unexpected.
    let cache = load_metadata_cache(dirs);

    // `const envRaw = process.env.MCP_DIRECT_TOOLS` — read ONCE, here, exactly as upstream reads it
    // once at install time. cyrup forbids unsafe env mutation, so tests drive the pure functions
    // ([`resolve_direct_tools`], [`should_register_proxy_tool`]) with an explicit override instead.
    let env_raw = std::env::var(DIRECT_TOOLS_ENV_VAR).ok();
    // The sentinel is compared against the RAW value upstream, not the trimmed one.
    let env_is_none_sentinel =
        env_raw.as_deref() == Some(crate::runtime::DIRECT_TOOLS_NONE_SENTINEL);
    let env_selectors = crate::runtime::direct_tools_override(env_raw.as_deref());
    // `getMissingConfiguredDirectToolServers` is passed `undefined` for BOTH "unset" and the
    // sentinel — that is what makes `__none__` suppress direct tools while still keeping the proxy
    // tool honest about servers that wanted them.
    let missing_override: Option<&[String]> =
        if env_raw.is_none() || env_is_none_sentinel { None } else { env_selectors.as_deref() };

    let render_kind = tool_render_kind(config.settings.as_ref());

    // --- prompt commands, one per cached prompt (MCP-206) --------------------------------------
    let mut seen_commands: HashSet<String> = HashSet::new();
    for spec in resolve_cached_prompts(config, cache.as_ref()) {
        if !seen_commands.insert(spec.command_name.clone()) {
            tracing::debug!(
                "MCP: prompt \"{}\" on {} skipped; /{} is already registered",
                spec.original_name,
                spec.server_name,
                spec.command_name
            );
            continue;
        }
        // `registeredPromptCommands` is a dedup SET upstream, not a fingerprint diff: a prompt
        // command's descriptor does not vary once its name is known, so an already-registered name
        // is skipped outright rather than compared.
        if api.should_register_prompt_command(&spec.command_name) {
            api.register_command(spec.command_name.clone(), prompt_command_descriptor(&spec));
            surface.command_names.push(spec.command_name.clone());
        }
        surface.prompt_commands.push(spec);
    }

    // --- the flag (MCP-002) --------------------------------------------------------------------
    api.register_flag(
        MCP_CONFIG_FLAG.to_string(),
        json!({ "description": "Path to MCP config file", "type": "string" }),
    );

    // --- /mcp and /mcp-auth --------------------------------------------------------------------
    // Registered once per generation, by `init`. A live re-sync skips them: they do not vary with
    // the discovered surface, so re-registering would be churn and a spurious `/` menu rebuild.
    if api.register_fixed_commands() {
        api.register_command(MCP_COMMAND.to_string(), mcp_command_descriptor());
        // MCP-041 — the opt-in that carries `/mcp` into the front-end's dynamic-completion table.
        // `mcp_command_descriptor`'s eight static rows are the completer's FIRST branch; the second
        // (server names for `reconnect`/`logout`/`disable`/`enable`) is only knowable at runtime and
        // reaches the popup through this table plus
        // `NativeExtension::labelled_argument_completions`. `/mcp-auth` deliberately does NOT opt in
        // — upstream declares no completer for it.
        api.add_autocomplete(MCP_COMMAND.to_string());
        surface.command_names.push(MCP_COMMAND.to_string());
        api.register_command(MCP_AUTH_COMMAND.to_string(), mcp_auth_command_descriptor());
        surface.command_names.push(MCP_AUTH_COMMAND.to_string());
    }

    // --- syncDirectTools (MCP-212 / MCP-216) ----------------------------------------------------
    let direct_specs = if env_is_none_sentinel {
        Vec::new()
    } else {
        resolve_direct_tools(config, cache.as_ref(), config.tool_prefix(), env_selectors.as_deref())
    };
    for spec in &direct_specs {
        let fingerprint = direct_tool_fingerprint(spec);
        // The fingerprint is recorded for EVERY resolved spec, registered or not: it is the
        // extension's memory of what the model is currently shown, and a spec skipped because it
        // is unchanged is still shown. Recording only the registered ones would make the next
        // pass believe every skipped tool had gone away.
        surface
            .direct_tool_fingerprints
            .insert(spec.prefixed_name.clone(), fingerprint.clone());
        if !api.should_register_tool(&spec.prefixed_name, &fingerprint) {
            continue;
        }
        surface.tool_names.push(spec.prefixed_name.clone());
        api.register_tool(Arc::new(DirectTool::new(
            spec.clone(),
            render_kind,
            Arc::clone(&dispatch),
        )));
        // MCP-036: declared HERE, beside the registration, rather than in a second loop the caller
        // runs. A tool registered on the LATE path has no caller loop — the same pass is the whole
        // registration — so a renderer declared anywhere else would silently not exist for it.
        api.register_tool_renderer(spec.prefixed_name.clone());
    }

    // --- syncProxyTool (MCP-213 / MCP-218 / MCP-247) --------------------------------------------
    if should_register_proxy_tool(config, cache.as_ref(), &direct_specs, missing_override) {
        let description = build_proxy_description(config, cache.as_ref(), &direct_specs);
        // Recorded before the gate, for the same reason the fingerprints are.
        surface.proxy_description = Some(description.clone());
        if api.should_register_proxy(&description) {
            api.register_tool(Arc::new(ProxyTool::new(
                description.clone(),
                render_kind,
                Arc::clone(&dispatch),
            )));
            api.register_tool_renderer(PROXY_TOOL_NAME.to_string());
            surface.tool_names.push(PROXY_TOOL_NAME.to_string());
        }
    }

    surface.direct_tools = direct_specs;
    surface
}

/// The descriptor for one cached prompt's slash command.
///
/// Upstream's `createPromptCommand` also carries the handler; cyrup routes execution by name
/// through `NativeExtension::execute_command`, so only the description crosses here. Argument
/// completions for a prompt's own arguments would need HA-2 (MCP-041).
fn prompt_command_descriptor(spec: &PromptCommandSpec) -> CommandDescriptor {
    CommandDescriptor {
        description: prompt_command_description(spec),
        completions: Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn entry(direct: bool) -> ServerEntry {
        ServerEntry { direct_tools: Some(BoolOrList::All(direct)), ..ServerEntry::default() }
    }

    fn cached_tool(name: &str) -> CachedTool {
        CachedTool { name: name.to_string(), ..CachedTool::default() }
    }

    /// A cache entry with a **placeholder** `configHash`; [`cache_of`] overwrites it with the real
    /// digest of the definition it is paired with.
    ///
    /// It used to carry the literal `"hash"` and be described as "always valid: no hasher is
    /// installed in tests". That was true and it was the bug: with no installed hasher
    /// `is_server_cache_valid` skipped the comparison entirely, so these fixtures never exercised
    /// it. MCP-145 gave the predicate a default hasher, and the fixtures now have to agree with the
    /// digest the production path computes.
    fn cache_entry(tools: Vec<CachedTool>) -> ServerCacheEntry {
        ServerCacheEntry {
            config_hash: None,
            tools: Some(tools),
            cached_at: Some(now_ms()),
            ..ServerCacheEntry::default()
        }
    }

    /// Build a cache whose entries carry the REAL `configHash` of the matching definition in
    /// `config` — the digest [`default_server_hasher`] computes, not a stand-in.
    ///
    /// An entry whose `config_hash` a test already set (a deliberate mismatch) is left alone, so
    /// "this entry is stale" stays expressible.
    fn cache_of(config: &McpConfig, servers: &[(&str, ServerCacheEntry)]) -> MetadataCache {
        let mut cache = MetadataCache { version: METADATA_CACHE_VERSION, ..Default::default() };
        for (name, entry) in servers {
            let mut entry = entry.clone();
            if entry.config_hash.is_none() {
                entry.config_hash = config
                    .mcp_servers
                    .get(*name)
                    .and_then(default_server_hasher);
            }
            cache.servers.insert((*name).to_string(), entry);
        }
        cache
    }

    fn config_of(servers: &[(&str, ServerEntry)]) -> McpConfig {
        let mut config = McpConfig::default();
        for (name, definition) in servers {
            config.mcp_servers.insert((*name).to_string(), definition.clone());
        }
        config
    }

    // --- naming (MCP-200 / MCP-203 / MCP-206) --------------------------------------------------

    #[test]
    fn sanitize_keeps_the_current_class_and_escapes_by_code_point() {
        assert_eq!(sanitize_server_prefix("github-mcp", true), "github-mcp");
        assert_eq!(sanitize_server_prefix("github-mcp", false), "github_2d_mcp");
        // `ï` is U+00EF in both grammars.
        assert_eq!(sanitize_server_prefix("naïve", true), "na_ef_ve");
        assert_eq!(sanitize_server_prefix("naïve", false), "na_ef_ve");
    }

    #[test]
    fn the_four_prefix_modes() {
        assert_eq!(server_prefix("github-mcp", ToolPrefix::None), "");
        assert_eq!(server_prefix("github-mcp", ToolPrefix::Short), "github");
        assert_eq!(server_prefix("github-mcp", ToolPrefix::Server), "github-mcp");
        assert_eq!(server_prefix("github-mcp", ToolPrefix::Mcp), "mcp__github-mcp");
        // `-?mcp$` eats the whole name, and the empty short prefix falls back to the literal.
        assert_eq!(server_prefix("mcp", ToolPrefix::Short), "mcp");
    }

    #[test]
    fn format_tool_name_replaces_dots_only() {
        assert_eq!(format_tool_name("list-sims", "x-mcp", ToolPrefix::Short), "x_list-sims");
        assert_eq!(format_tool_name("a.b", "s", ToolPrefix::Server), "s_a_b");
        assert_eq!(format_tool_name("a.b", "s", ToolPrefix::None), "a_b");
        // mcp mode is ONE underscore between server and tool.
        assert_eq!(format_tool_name("t", "s", ToolPrefix::Mcp), "mcp__s_t");
    }

    #[test]
    fn prompt_command_names_always_carry_a_server_segment() {
        assert_eq!(
            format_prompt_command_name("summarize", "gh-mcp", ToolPrefix::None),
            "mcp__gh-mcp__summarize"
        );
        assert_eq!(sanitize_prompt_name("9a b"), "_9a_b");
        assert_eq!(sanitize_prompt_name("__"), "prompt");
    }

    #[test]
    fn resource_names_collapse_and_never_lead_with_a_digit() {
        assert_eq!(resource_name_to_tool_name("My File//name"), "my_file_name");
        assert_eq!(resource_name_to_tool_name("///"), "resource");
        assert_eq!(resource_name_to_tool_name("2024 log"), "resource_2024_log");
        assert_eq!(resource_base_tool_name("My File"), "read_my_file");
    }

    #[test]
    fn truncate_at_word_backs_up_to_a_space_past_sixty_percent() {
        assert_eq!(truncate_at_word("short", 10), "short");
        // The last space sits at 8 of a 10 budget (> 6), so the cut lands on the word boundary.
        assert_eq!(truncate_at_word("abcd efg hijklmnop", 10), "abcd efg...");
        // The only space is at 1 of 10 (< 6), so the raw slice is kept.
        assert_eq!(truncate_at_word("a bcdefghijklmno", 10), "a bcdefghi...");
    }

    #[test]
    fn candidate_sets_dedupe_hard() {
        let current = tool_name_candidates("list-sims", "xcodebuild-mcp", ToolPrefix::Short, false);
        assert_eq!(current.len(), 4, "{current:?}");
        let full = tool_name_candidates("list-sims", "xcodebuild-mcp", ToolPrefix::Short, true);
        assert_eq!(full.len(), 12, "{full:?}");
    }

    #[test]
    fn glob_and_exact_selectors() {
        let candidates = tool_name_candidates("list_sims", "x", ToolPrefix::Server, false);
        assert!(matches_tool_pattern(&candidates, &["x_list_sims".to_string()]));
        assert!(matches_tool_pattern(&candidates, &["x_*".to_string()]));
        assert!(!matches_tool_pattern(&candidates, &["y_*".to_string()]));
        assert!(!matches_tool_pattern(&candidates, &[]));
    }

    /// `types.ts:848` `indexHasOtherCurrentMatch`, mirroring the three cases upstream added to
    /// `__tests__/resolve-server-from-tool-name.test.ts` in `14c0e6c` ("share filtered selector
    /// candidate scans", issue #354).
    ///
    /// The rule [`CandidateIndex`] exists to enforce: a **legacy-only** alias selects a tool only
    /// when it does not also name some *other* tool's current name. For
    /// `("do-thing", "my-server", Server)` the current set keeps the hyphen
    /// (`my-server_do-thing`), so `my_server_do_thing` is legacy-only — it excludes when nothing
    /// else answers to it, and must not once another server's tool does.
    ///
    /// The glob arm is why `14c0e6c` compares **counts** instead of asking `any(matches)`: the
    /// index spans every server *including this one*, because upstream stopped subtracting the
    /// tool's own candidates when building it. A pattern that reaches only my own candidates is
    /// therefore not a collision, which is what `current_only` pins — a naive `any` would read the
    /// self-match as someone else and silently stop excluding.
    #[test]
    fn the_candidate_index_separates_other_tools_from_my_own() {
        let no_patterns: Option<&[String]> = None;
        let empty_patterns: &[String] = &[];
        let exclude = ["my_server_do_thing".to_string()];

        // Legacy-only alias, nobody else answers to it → the exclusion lands.
        let mut lonely = CandidateIndex::new(IndexSet::new());
        assert!(!is_tool_allowed(
            "do-thing",
            "my-server",
            ToolPrefix::Server,
            no_patterns,
            Some(&exclude),
            Some(&mut lonely),
        ));
        // Same alias, but it is another tool's *current* name → the exclusion is suppressed.
        let mut collision = CandidateIndex::new(IndexSet::from(["my_server_do_thing".to_string()]));
        assert!(is_tool_allowed(
            "do-thing",
            "my-server",
            ToolPrefix::Server,
            no_patterns,
            Some(&exclude),
            Some(&mut collision),
        ));

        // An absent or empty selector must never touch the memo tables: `is_tool_allowed`
        // short-circuits before `matches_tool_selector` builds a candidate set at all.
        let mut untouched = CandidateIndex::new(IndexSet::from(["my_server_do_other".to_string()]));
        for patterns in [no_patterns, Some(empty_patterns)] {
            assert!(is_tool_allowed(
                "do-thing",
                "my-server",
                ToolPrefix::Server,
                no_patterns,
                patterns,
                Some(&mut untouched),
            ));
        }
        assert!(untouched.matcher.is_empty());
        assert!(untouched.matching_count.is_empty());

        // Glob: one *other* candidate matches and none of mine do → collision. Both the compiled
        // matcher and the whole-index match count are memoised once and reused by the second call.
        let glob = ["my_server_do_*".to_string()];
        let mut globbed = CandidateIndex::new(IndexSet::from([
            "my_server_do_other".to_string(),
            "unrelated".to_string(),
        ]));
        for _ in 0..2 {
            assert!(is_tool_allowed(
                "do-thing",
                "my-server",
                ToolPrefix::Server,
                no_patterns,
                Some(&glob),
                Some(&mut globbed),
            ));
        }
        assert_eq!(globbed.matching_count.get("my_server_do_*"), Some(&1));
        assert_eq!(globbed.matcher.len(), 1);

        // The same glob over an index holding only my own current candidates: `total > mine` is
        // false, so it is not a collision and the legacy alias excludes.
        let mut current_only = CandidateIndex::new(tool_name_candidates(
            "do-thing",
            "my-server",
            ToolPrefix::Server,
            false,
        ));
        assert!(!is_tool_allowed(
            "do-thing",
            "my-server",
            ToolPrefix::Server,
            no_patterns,
            Some(&glob),
            Some(&mut current_only),
        ));
        assert_eq!(current_only.matcher.len(), 1);
    }

    // --- MCP-212, the critical -----------------------------------------------------------------

    #[test]
    fn a_tool_that_formats_to_a_builtin_name_is_dropped() {
        let mut definition = entry(true);
        definition.tool_prefix = Some(ToolPrefix::None);
        let config = config_of(&[("s", definition)]);
        let cache = cache_of(&config, &[("s", cache_entry(vec![cached_tool("read"), cached_tool("ok")]))]);

        let specs = resolve_direct_tools(&config, Some(&cache), ToolPrefix::None, None);
        let names: Vec<&str> = specs.iter().map(|s| s.prefixed_name.as_str()).collect();
        assert_eq!(names, vec!["ok"], "the builtin-colliding name must never be registered");
    }

    #[test]
    fn every_builtin_name_is_dropped_in_none_mode() {
        let mut definition = entry(true);
        definition.tool_prefix = Some(ToolPrefix::None);
        let config = config_of(&[("s", definition)]);
        let tools: Vec<CachedTool> = BUILTIN_NAMES.iter().map(|n| cached_tool(n)).collect();
        let cache = cache_of(&config, &[("s", cache_entry(tools))]);

        let specs = resolve_direct_tools(&config, Some(&cache), ToolPrefix::None, None);
        assert!(specs.is_empty(), "resolved {specs:?}");
    }

    #[test]
    fn the_second_of_two_colliding_tools_is_dropped_in_config_order() {
        let mut a = entry(true);
        a.tool_prefix = Some(ToolPrefix::None);
        let mut b = entry(true);
        b.tool_prefix = Some(ToolPrefix::None);
        let config = config_of(&[("a", a), ("b", b)]);
        let cache = cache_of(&config, &[
            ("a", cache_entry(vec![cached_tool("dup")])),
            ("b", cache_entry(vec![cached_tool("dup")])),
        ]);

        let specs = resolve_direct_tools(&config, Some(&cache), ToolPrefix::None, None);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].server_name, "a", "file order decides the winner");
    }

    #[test]
    fn no_cache_means_no_direct_tools() {
        let config = config_of(&[("s", entry(true))]);
        assert!(resolve_direct_tools(&config, None, ToolPrefix::Server, None).is_empty());
    }

    #[test]
    fn direct_tools_false_skips_the_server_entirely() {
        let config = config_of(&[("s", entry(false))]);
        let cache = cache_of(&config, &[("s", cache_entry(vec![cached_tool("t")]))]);
        assert!(resolve_direct_tools(&config, Some(&cache), ToolPrefix::Server, None).is_empty());
    }

    #[test]
    fn a_hidden_tool_is_not_registered() {
        let config = config_of(&[("s", entry(true))]);
        let mut hidden = cached_tool("hidden");
        hidden.ui_visibility = Some(json!(["app"]));
        let mut malformed = cached_tool("malformed");
        malformed.ui_visibility = Some(json!("model"));
        let cache =
            cache_of(&config, &[("s", cache_entry(vec![hidden, malformed, cached_tool("visible")]))]);

        let specs = resolve_direct_tools(&config, Some(&cache), ToolPrefix::Server, None);
        let names: Vec<&str> = specs.iter().map(|s| s.prefixed_name.as_str()).collect();
        assert_eq!(names, vec!["s_visible"]);
    }

    #[test]
    fn resources_register_as_read_tools_with_the_uri_description() {
        let config = config_of(&[("s", entry(true))]);
        let mut entry_with_resource = cache_entry(vec![]);
        entry_with_resource.resources = Some(vec![CachedResource {
            uri: "file:///a".to_string(),
            name: "My File".to_string(),
            description: None,
        }]);
        let cache = cache_of(&config, &[("s", entry_with_resource)]);

        let specs = resolve_direct_tools(&config, Some(&cache), ToolPrefix::Server, None);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].prefixed_name, "s_read_my_file");
        assert_eq!(specs[0].description, "Read resource: file:///a");
        assert_eq!(specs[0].resource_uri.as_deref(), Some("file:///a"));
    }

    #[test]
    fn exclude_tools_filters_before_naming() {
        let mut definition = entry(true);
        definition.exclude_tools = Some(vec!["s_gone".to_string()]);
        let config = config_of(&[("s", definition)]);
        let cache = cache_of(&config, &[("s", cache_entry(vec![cached_tool("gone"), cached_tool("kept")]))]);

        let specs = resolve_direct_tools(&config, Some(&cache), ToolPrefix::Server, None);
        let names: Vec<&str> = specs.iter().map(|s| s.prefixed_name.as_str()).collect();
        assert_eq!(names, vec!["s_kept"]);
    }

    #[test]
    fn an_expired_cache_entry_is_invalid() {
        let mut stale = cache_entry(vec![cached_tool("t")]);
        stale.cached_at = Some(now_ms() - METADATA_CACHE_MAX_AGE_MS - 1000.0);
        let config = config_of(&[("s", entry(true))]);
        let cache = cache_of(&config, &[("s", stale)]);
        assert!(resolve_direct_tools(&config, Some(&cache), ToolPrefix::Server, None).is_empty());
    }


    // --- MCP-145: the hash comparison, the throw arm, and the two `cachedAt` rules -------------

    /// The seam had no production installer, so `is_server_cache_valid` **skipped the hash
    /// comparison entirely** and a server whose definition had changed since the cache was written
    /// still registered its stale direct tools. This is that hole, closed: with nothing installed,
    /// a foreign `configHash` is now rejected and the real one is accepted.
    #[test]
    fn the_config_hash_is_compared_without_anything_being_installed() {
        let definition = entry(true);
        let mut stale = cache_entry(vec![cached_tool("t")]);
        stale.config_hash = Some("0".repeat(64));
        assert!(
            !is_server_cache_valid(&stale, &definition, METADATA_CACHE_MAX_AGE_MS),
            "a mismatched digest must invalidate even with no installed hasher"
        );

        let mut fresh = cache_entry(vec![cached_tool("t")]);
        fresh.config_hash = default_server_hasher(&definition);
        assert!(is_server_cache_valid(&fresh, &definition, METADATA_CACHE_MAX_AGE_MS));

        // …and it tracks the definition: adding an identity field evicts the entry.
        let mut edited = definition.clone();
        edited.include_tools = Some(vec!["a".to_string()]);
        assert!(!is_server_cache_valid(&fresh, &edited, METADATA_CACHE_MAX_AGE_MS));
        // …while a runtime-only field does not.
        let mut noisy = definition.clone();
        noisy.debug = Some(true);
        assert!(is_server_cache_valid(&fresh, &noisy, METADATA_CACHE_MAX_AGE_MS));
    }

    /// The throw arm — upstream's `try { computeServerHash } catch { return false }`, and the sole
    /// mechanism keeping a URL server with a missing environment variable out of the cold-start
    /// direct-tool surface.
    ///
    /// The variable name is deliberately one nothing sets; the assertion is not that the hash is
    /// wrong but that there **is** no hash, so no `configHash` — not even one copied out of the
    /// entry itself — can make the entry valid.
    #[test]
    fn a_url_naming_a_missing_variable_is_never_cache_valid() {
        let definition = ServerEntry {
            url: Some("https://x.example/${CYRUP_MCP_145_DEFINITELY_UNSET}/mcp".to_string()),
            direct_tools: Some(BoolOrList::All(true)),
            ..ServerEntry::default()
        };
        assert!(default_server_hasher(&definition).is_none(), "the hash must throw");

        let mut anything = cache_entry(vec![cached_tool("t")]);
        anything.config_hash = Some("0".repeat(64));
        assert!(!is_server_cache_valid(&anything, &definition, METADATA_CACHE_MAX_AGE_MS));
        // Even `maxAgeMs = 0`, which disables the age check, cannot rescue it: the throw is first.
        assert!(!is_server_cache_valid(&anything, &definition, 0.0));

        // And the server therefore reports as MISSING a cache entry, which is what keeps the proxy
        // tool registered for it (MCP-218).
        let config = config_of(&[("u", definition)]);
        let mut cache = MetadataCache { version: METADATA_CACHE_VERSION, ..Default::default() };
        cache.servers.insert("u".to_string(), anything);
        assert_eq!(
            missing_configured_direct_tool_servers(&config, Some(&cache), None),
            vec!["u".to_string()]
        );
        assert!(resolve_direct_tools(&config, Some(&cache), ToolPrefix::Server, None).is_empty());
    }

    /// `!entry.cachedAt` is a falsy test on a number, and `maxAgeMs > 0` gates the age check —
    /// upstream's two remaining rejections, neither of which this predicate applied.
    #[test]
    fn cached_at_zero_is_rejected_and_max_age_zero_disables_the_age_check() {
        let definition = entry(true);
        let hash = default_server_hasher(&definition);

        let mut zero = cache_entry(vec![cached_tool("t")]);
        zero.config_hash = hash.clone();
        zero.cached_at = Some(0.0);
        assert!(
            !is_server_cache_valid(&zero, &definition, METADATA_CACHE_MAX_AGE_MS),
            "`cachedAt: 0` is falsy upstream, so it is absent"
        );
        // …and it is rejected by the FALSY test, not by the age check: with `maxAgeMs = 0` there is
        // no age check left to hide behind, and an `is_finite()`-only guard would accept it.
        assert!(!is_server_cache_valid(&zero, &definition, 0.0));

        let mut ancient = cache_entry(vec![cached_tool("t")]);
        ancient.config_hash = hash;
        ancient.cached_at = Some(now_ms() - METADATA_CACHE_MAX_AGE_MS * 52.0);
        assert!(!is_server_cache_valid(&ancient, &definition, METADATA_CACHE_MAX_AGE_MS));
        assert!(
            is_server_cache_valid(&ancient, &definition, 0.0),
            "`maxAgeMs = 0` disables the age check entirely"
        );
    }

    /// A malformed `cachedAt` from a foreign writer must cost that entry and nothing else.
    ///
    /// With a plain `Option<f64>` serde *errors* on a JSON string, the error propagates out of
    /// `from_str::<MetadataCache>`, and `load_metadata_cache` answers `None` for the whole file —
    /// so every other server in it silently loses its cached tools. Upstream casts without
    /// validating and rejects only the bad entry.
    #[test]
    fn a_string_cached_at_invalidates_one_entry_not_the_whole_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let agent_dir = temp.path().join("agent");
        std::fs::create_dir_all(&agent_dir).expect("mkdir");
        let definition = entry(true);
        let hash = default_server_hasher(&definition).expect("no url, so no throw");
        std::fs::write(
            agent_dir.join("mcp-cache.json"),
            serde_json::to_string(&json!({
                "version": METADATA_CACHE_VERSION,
                "servers": {
                    "bad":  { "configHash": hash, "cachedAt": "1760000000000",
                              "tools": [{ "name": "t" }], "resources": [] },
                    "good": { "configHash": hash, "cachedAt": now_ms(),
                              "tools": [{ "name": "t" }], "resources": [] }
                }
            }))
            .expect("serialize"),
        )
        .expect("write");

        let dirs = McpDirs::new(agent_dir, temp.path().to_path_buf());
        let cache = load_metadata_cache(&dirs).expect("the file still loads");
        assert_eq!(cache.servers.len(), 2);
        assert!(cache.servers["bad"].cached_at.is_none(), "a JSON string is not a number");

        let config = config_of(&[("bad", definition.clone()), ("good", definition)]);
        let specs = resolve_direct_tools(&config, Some(&cache), ToolPrefix::Server, None);
        let names: Vec<&str> = specs.iter().map(|s| s.prefixed_name.as_str()).collect();
        assert_eq!(names, vec!["good_t"], "only the malformed entry is lost");
    }

    // --- MCP-219 -------------------------------------------------------------------------------

    #[test]
    fn selector_parsing_drops_the_third_segment() {
        // The env value is split, trimmed and emptied-out FIRST (`crate::runtime`, MCP-013); only
        // then does the selector grammar run, so the blank element never reaches it.
        let selectors = crate::runtime::direct_tools_override(Some("a/b/c, ,d/")).unwrap();
        assert_eq!(selectors, vec!["a/b/c".to_string(), "d/".to_string()]);

        let selection = parse_direct_tool_selectors(&selectors);
        assert!(selection.servers.contains("d"));
        assert_eq!(selection.servers.len(), 1);
        assert_eq!(selection.tools.get("a").map(HashSet::len), Some(1));
        assert!(selection.tools.get("a").is_some_and(|t| t.contains("b")));
    }

    #[test]
    fn the_env_override_outranks_the_config() {
        let config = config_of(&[("a", entry(false)), ("b", entry(false))]);
        let cache = cache_of(&config, &[
            ("a", cache_entry(vec![cached_tool("t")])),
            ("b", cache_entry(vec![cached_tool("t")])),
        ]);
        let override_selectors = vec!["a".to_string()];
        let specs = resolve_direct_tools(
            &config,
            Some(&cache),
            ToolPrefix::Server,
            Some(&override_selectors),
        );
        let names: Vec<&str> = specs.iter().map(|s| s.prefixed_name.as_str()).collect();
        assert_eq!(names, vec!["a_t"]);
    }

    // --- MCP-246 -------------------------------------------------------------------------------

    /// `__tests__/direct-tools.test.ts` — "warns by default without capping when resolved direct
    /// tools exceed the README threshold" and "suppresses the large direct-tools advisory when
    /// configured" (upstream `76a4ea3`, issue #358).
    ///
    /// The advisory itself is a `tracing::warn!` and this crate installs no subscriber in tests, so
    /// upstream's `expect(warn).not.toHaveBeenCalled()` has no direct analogue. What is asserted is
    /// both halves that are observable: the threshold is **not** a cap — all 75 specs resolve with
    /// the advisory on *or* off — and the `!== false` three-way that decides whether it is emitted,
    /// read at the same call site the resolver reads it from.
    #[test]
    fn the_large_direct_tools_advisory_is_suppressible_and_never_caps() {
        let mut config = config_of(&[("huge", entry(true))]);
        let tools: Vec<CachedTool> = (0..DIRECT_TOOLS_ADVISORY_THRESHOLD)
            .map(|index| cached_tool(&format!("tool_{index}")))
            .collect();
        let cache = cache_of(&config, &[("huge", cache_entry(tools))]);

        // Default: loud, and every configured tool still registers.
        let specs = resolve_direct_tools(&config, Some(&cache), ToolPrefix::Server, None);
        assert_eq!(specs.len(), DIRECT_TOOLS_ADVISORY_THRESHOLD);
        assert!(config.settings_or_default().warn_on_large_direct_tools());

        // Suppressed: byte-identical spec list, no advisory.
        config.settings = Some(McpSettings {
            warn_on_large_direct_tools: Some(false),
            ..McpSettings::default()
        });
        let suppressed = resolve_direct_tools(&config, Some(&cache), ToolPrefix::Server, None);
        assert_eq!(suppressed, specs);
        assert!(!config.settings_or_default().warn_on_large_direct_tools());

        // `!== false`, not `=== true`: a present-and-`true` key is still loud.
        config.settings =
            Some(McpSettings { warn_on_large_direct_tools: Some(true), ..McpSettings::default() });
        assert!(config.settings_or_default().warn_on_large_direct_tools());
    }

    // --- MCP-213 -------------------------------------------------------------------------------

    #[test]
    fn proxy_description_golden_for_two_servers() {
        let disabled = ServerEntry { disabled: Some(true), ..ServerEntry::default() };
        let config = config_of(&[("alpha", entry(true)), ("beta", entry(false)), ("off", disabled)]);
        let mut alpha = cache_entry(vec![cached_tool("one"), cached_tool("two")]);
        alpha.instructions = Some("  Use   alpha   carefully.  ".to_string());
        let cache = cache_of(&config, &[
            ("alpha", alpha),
            ("beta", cache_entry(vec![cached_tool("b1"), cached_tool("b2"), cached_tool("b3")])),
        ]);
        let specs = resolve_direct_tools(&config, Some(&cache), ToolPrefix::Server, None);
        assert_eq!(specs.len(), 2, "{specs:?}");

        let description = build_proxy_description(&config, Some(&cache), &specs);
        let expected = concat!(
            "MCP gateway — server status, tool search/describe, auth, and single MCP tool calls. Non-MCP cyrup tools should be called directly, not through mcp.\n",
            "\nDirect tools available (call as normal tools): alpha (2)\n",
            "\nServers: beta (3 tools)\n",
            "\nDisabled servers (enable with /mcp enable <server> and /reload): off\n",
            "\nServer instructions (truncated - full text via mcp({ instructions: \"name\" })):\n",
            "  alpha: Use alpha carefully.\n",
            "\nUsage:\n",
            "  mcp({ })                              → Show server status\n",
            "  mcp({ server: \"name\" })               → List tools from server\n",
            "  mcp({ search: \"query\" })              → Search MCP tools by name/description\n",
            "  mcp({ describe: \"tool_name\" })        → Show tool details and parameters\n",
            "  mcp({ instructions: \"name\" })         → Show full server usage instructions\n",
            "  mcp({ connect: \"server-name\" })       → Connect to a server and refresh metadata\n",
            "  mcp({ tool: \"name\", args: { key: \"value\" } })         → Call a tool (object args; JSON string also accepted)\n",
            "  mcp({ action: \"auth-start\", server: \"name\" })      → Start manual OAuth and get a browser URL\n",
            "  mcp({ action: \"auth-complete\", server: \"name\", args: { redirectUrl: \"...\" } }) → Complete manual OAuth\n",
            "\nMode: action > tool (call) > connect > describe > instructions > search > server (list) > nothing (status)",
        );
        assert_eq!(description, expected);
        assert!(!description.contains("mcpScript"), "Cut 4");
        assert!(!description.contains("ui-messages"), "Cut 2");
    }

    #[test]
    fn proxy_description_is_deterministic() {
        let config = config_of(&[("a", entry(true)), ("b", entry(true)), ("c", entry(true))]);
        let cache = cache_of(&config, &[
            ("a", cache_entry(vec![cached_tool("t1"), cached_tool("t2")])),
            ("b", cache_entry(vec![cached_tool("t3")])),
            ("c", cache_entry(vec![cached_tool("t4")])),
        ]);
        let specs = resolve_direct_tools(&config, Some(&cache), ToolPrefix::Server, None);
        let distinct: HashSet<String> = (0..100)
            .map(|_| build_proxy_description(&config, Some(&cache), &specs))
            .collect();
        assert_eq!(distinct.len(), 1, "the prompt-cache key must not wobble");
    }

    // --- MCP-218 -------------------------------------------------------------------------------

    #[test]
    fn the_proxy_survives_a_cold_cache_even_when_disabled() {
        let mut config = config_of(&[("s", entry(true))]);
        config.settings =
            Some(McpSettings { disable_proxy_tool: Some(true), ..McpSettings::default() });

        // Cold cache: no direct specs at all.
        assert!(should_register_proxy_tool(&config, None, &[], None));

        // Warm cache with a complete direct surface: the setting is finally honoured.
        let cache = cache_of(&config, &[("s", cache_entry(vec![cached_tool("t")]))]);
        let specs = resolve_direct_tools(&config, Some(&cache), ToolPrefix::Server, None);
        assert_eq!(specs.len(), 1);
        assert!(!should_register_proxy_tool(&config, Some(&cache), &specs, None));
    }

    #[test]
    fn a_server_wanting_direct_tools_without_a_cache_entry_is_missing() {
        let config = config_of(&[("warm", entry(true)), ("cold", entry(true))]);
        let cache = cache_of(&config, &[("warm", cache_entry(vec![cached_tool("t")]))]);
        assert_eq!(
            missing_configured_direct_tool_servers(&config, Some(&cache), None),
            vec!["cold".to_string()]
        );
    }

    // --- MCP-216 / MCP-236 / MCP-238 / MCP-247 -------------------------------------------------

    #[test]
    fn the_direct_tool_registration_shape() {
        let spec = DirectToolSpec {
            server_name: "srv".to_string(),
            original_name: "orig".to_string(),
            prefixed_name: "srv_orig".to_string(),
            description: String::new(),
            input_schema: None,
            resource_uri: None,
        };
        let tool =
            DirectTool::new(spec, ToolRenderKind::SelfRendered, Arc::new(ToolDispatch::default()));
        assert_eq!(tool.name(), "srv_orig");
        assert_eq!(tool.label(), Some("MCP: orig"));
        assert_eq!(tool.description(), "(no description)");
        assert_eq!(tool.prompt_snippet(), Some("MCP tool from srv"));
        assert_eq!(tool.parameters(), &json!({ "type": "object", "properties": {} }));
    }

    #[test]
    fn the_input_schema_normalizer_strips_both_keys() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "additionalProperties": false,
            "properties": { "a": { "type": "string" } }
        });
        let normalized = normalize_direct_tool_input_schema(Some(&schema));
        assert!(normalized.get("$schema").is_none());
        assert!(normalized.get("additionalProperties").is_none());
        assert!(normalized.get("properties").is_some());
        assert_eq!(
            normalize_direct_tool_input_schema(Some(&json!([1, 2]))),
            json!({ "type": "object", "properties": {} })
        );
    }

    #[test]
    fn the_proxy_schema_keeps_the_five_permission_relevant_names() {
        let params = proxy_tool_parameters();
        let properties = params.get("properties").and_then(Value::as_object).unwrap();
        for name in ["tool", "server", "connect", "describe", "search"] {
            assert!(properties.contains_key(name), "{name} drives a permission target");
        }
        assert_eq!(properties.len(), 12);
        assert!(params.get("required").is_none(), "all twelve are optional");
        let action = properties["action"]["description"].as_str().unwrap();
        assert!(!action.contains("ui-messages"), "Cut 2");
        assert!(action.contains("auth-start") && action.contains("auth-complete"));
    }

    #[test]
    fn the_proxy_tool_carries_the_sanitizer_guideline() {
        let tool = ProxyTool::new(
            "desc".to_string(),
            ToolRenderKind::SelfRendered,
            Arc::new(ToolDispatch::default()),
        );
        assert_eq!(tool.name(), PROXY_TOOL_NAME);
        assert_eq!(tool.label(), Some("MCP"));
        assert_eq!(tool.prompt_guidelines(), vec![PROXY_TOOL_PROMPT_GUIDELINE]);
        // The sanitizer normalises with `split_whitespace().join(" ").to_lowercase()`.
        let normalized =
            PROXY_TOOL_PROMPT_GUIDELINE.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(normalized.to_lowercase(), PROXY_TOOL_PROMPT_GUIDELINE);
    }

    #[test]
    fn render_kind_follows_the_result_rendering_mode() {
        assert_eq!(tool_render_kind(None), ToolRenderKind::SelfRendered);
        let boxed = McpSettings {
            tool_result_rendering: Some(ToolResultRendering::Boxed),
            ..McpSettings::default()
        };
        assert_eq!(tool_render_kind(Some(&boxed)), ToolRenderKind::Default);
    }

    #[tokio::test]
    async fn an_uninstalled_dispatch_answers_not_initialized() {
        let tool = ProxyTool::new(
            "desc".to_string(),
            ToolRenderKind::SelfRendered,
            Arc::new(ToolDispatch::default()),
        );
        let result = tool
            .execute(
                ToolCallId::from("call-1"),
                json!({}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("a pre-init MCP call is a successful result, not an Err");
        assert_eq!(result.details, Some(json!({ "error": "not_initialized" })));
    }

    // --- MCP-206 / prompts ---------------------------------------------------------------------

    #[test]
    fn cached_prompts_become_one_command_each() {
        let config = config_of(&[("gh-mcp", entry(false))]);
        let mut server = cache_entry(vec![]);
        server.prompts = Some(vec![
            CachedPrompt {
                name: "summarize".to_string(),
                description: Some("Summarise a PR".to_string()),
                ..CachedPrompt::default()
            },
            CachedPrompt { name: String::new(), ..CachedPrompt::default() },
        ]);
        let cache = cache_of(&config, &[("gh-mcp", server)]);

        let prompts = resolve_cached_prompts(&config, Some(&cache));
        assert_eq!(prompts.len(), 1, "a nameless prompt is skipped, not registered");
        assert_eq!(prompts[0].command_name, "mcp__gh-mcp__summarize");
        assert_eq!(prompt_command_description(&prompts[0]), "MCP: Summarise a PR");
    }

    #[test]
    fn a_prompt_with_no_description_falls_back_through_title_then_server() {
        let spec = PromptCommandSpec {
            server_name: "srv".to_string(),
            original_name: "p".to_string(),
            command_name: "mcp__srv__p".to_string(),
            title: None,
            description: String::new(),
            arguments: Vec::new(),
        };
        assert_eq!(prompt_command_description(&spec), "MCP: MCP prompt from srv");
        let titled = PromptCommandSpec { title: Some("Title".to_string()), ..spec };
        assert_eq!(prompt_command_description(&titled), "MCP: Title");
    }

    // --- MCP-003, the whole surface -------------------------------------------------------------

    #[test]
    fn register_surface_survives_a_garbage_cache_and_still_registers_the_surface() {
        let temp = tempfile::tempdir().expect("tempdir");
        let agent_dir = temp.path().join("agent");
        std::fs::create_dir_all(&agent_dir).expect("mkdir");
        std::fs::write(agent_dir.join("mcp-cache.json"), "{{{ not json").expect("write");
        let dirs = McpDirs::new(agent_dir, temp.path().to_path_buf());

        let mut api = InitApi::new();
        let surface = register_surface(
            &mut api,
            &dirs,
            &config_of(&[("s", entry(true))]),
            Arc::new(ToolDispatch::default()),
        );

        assert!(surface.direct_tools.is_empty(), "a corrupt cache means no direct tools");
        assert_eq!(surface.tool_names, vec![PROXY_TOOL_NAME.to_string()]);
        assert!(surface.proxy_description.is_some());
        assert_eq!(
            surface.command_names,
            vec![MCP_COMMAND.to_string(), MCP_AUTH_COMMAND.to_string()]
        );
        assert!(!surface.dispatch.is_installed());
    }

    #[test]
    fn register_surface_registers_direct_tools_then_the_proxy() {
        let temp = tempfile::tempdir().expect("tempdir");
        let agent_dir = temp.path().join("agent");
        std::fs::create_dir_all(&agent_dir).expect("mkdir");
        let config = config_of(&[("srv", entry(true))]);
        // The on-disk `configHash` has to be the digest the loader will recompute — with a
        // placeholder it validated only because no hasher was installed (MCP-145).
        let config_hash = default_server_hasher(&entry(true)).expect("no url, so no throw");
        std::fs::write(
            agent_dir.join("mcp-cache.json"),
            serde_json::to_string(&json!({
                "version": METADATA_CACHE_VERSION,
                "servers": {
                    "srv": {
                        "configHash": config_hash,
                        "cachedAt": now_ms(),
                        "tools": [{ "name": "one" }, { "name": "two" }],
                        "resources": [],
                        "prompts": [{ "name": "brief" }]
                    }
                }
            }))
            .expect("serialize"),
        )
        .expect("write");
        let dirs = McpDirs::new(agent_dir, temp.path().to_path_buf());

        let mut api = InitApi::new();
        let surface =
            register_surface(&mut api, &dirs, &config, Arc::new(ToolDispatch::default()));

        assert_eq!(
            surface.tool_names,
            vec!["srv_one".to_string(), "srv_two".to_string(), PROXY_TOOL_NAME.to_string()],
            "direct tools in config order, then the gateway"
        );
        assert_eq!(surface.direct_tool_fingerprints.len(), 2);
        assert_eq!(
            surface.command_names,
            vec![
                "mcp__srv__brief".to_string(),
                MCP_COMMAND.to_string(),
                MCP_AUTH_COMMAND.to_string(),
            ],
            "prompt commands are registered before /mcp, exactly as upstream orders them"
        );
        assert_eq!(surface.prompt_commands.len(), 1);
    }

    #[test]
    fn a_fingerprint_changes_with_the_description_and_not_with_the_call() {
        let spec = DirectToolSpec {
            server_name: "s".to_string(),
            original_name: "t".to_string(),
            prefixed_name: "s_t".to_string(),
            description: "one".to_string(),
            input_schema: Some(json!({ "type": "object" })),
            resource_uri: None,
        };
        let first = direct_tool_fingerprint(&spec);
        assert_eq!(first, direct_tool_fingerprint(&spec));
        assert!(!first.contains("resourceUri"), "undefined keys are dropped, as JSON.stringify does");
        let changed = DirectToolSpec { description: "two".to_string(), ..spec };
        assert_ne!(first, direct_tool_fingerprint(&changed));
    }
    /// The gateway tool's description is built twice — from the disk cache here, and from live
    /// metadata in [`crate::proxy::build_proxy_description`]. `McpExtension::proxy_tool_description`
    /// re-registers the tool only when the text *changed*, so if the two heads ever differ the
    /// guard never fires and every reconnect invalidates the provider's prompt-cache prefix.
    #[test]
    fn both_proxy_descriptions_share_one_head_line() {
        let from_cache = build_proxy_description(&McpConfig::default(), None, &[]);
        let from_live = crate::proxy::build_proxy_description(
            &McpConfig::default(),
            &indexmap::IndexMap::new(),
            &[],
        );
        let head = |text: &str| text.lines().next().unwrap_or_default().to_string();
        assert_eq!(head(&from_cache), head(&from_live));
        // …and it is `direct-tools.ts:240` with the single `Pi` → `cyrup` rebrand.
        assert_eq!(
            head(&from_cache),
            "MCP gateway — server status, tool search/describe, auth, and single MCP tool calls. \
             Non-MCP cyrup tools should be called directly, not through mcp."
        );
    }

    // --- MCP-207, `state.toolMetadata`'s two writers --------------------------------------------

    fn live_meta(value: Value) -> rmcp::model::MetaObject {
        match value {
            Value::Object(map) => rmcp::model::MetaObject(map),
            _ => rmcp::model::MetaObject::new(),
        }
    }

    fn live_tool(name: &str, description: Option<&str>) -> rmcp::model::Tool {
        rmcp::model::Tool::new_with_raw(
            name.to_string(),
            description.map(|text| std::borrow::Cow::Owned(text.to_string())),
            Arc::new(serde_json::Map::new()),
        )
    }

    /// The four gates of `tool-metadata.ts:79-115`, in the order that decides which tool wins a
    /// name — and the two things that reach `failedTools`.
    ///
    /// The reservation is `seenNames.add` at `:97`, **after** the visibility test at `:94`, not
    /// before it: a hidden tool is dropped without consuming its name, so the next tool that
    /// formats to the same name still registers. (The `seenNames.has` *test* at `:89` is what runs
    /// before visibility, which is a different statement and has no observable effect.)
    #[test]
    fn the_live_walk_reserves_names_only_for_tools_the_model_can_see() {
        let definition = entry(true);
        let config = config_of(&[("srv", definition.clone())]);
        let tools = vec![
            // `!tool?.name` → `"(unnamed)"` and skip (`:80-83`).
            live_tool("", None),
            // Hidden: dropped, and its name stays free.
            live_tool("a.b", Some("hidden"))
                .with_meta(live_meta(json!({"ui": {"visibility": ["app"]}}))),
            // Formats to the same `srv_a_b` the hidden one would have taken.
            live_tool("a_b", Some("visible")),
            // …and now that the name IS taken, the next one on it is dropped (`:89-91`).
            live_tool("a.b", Some("dropped")),
            // `getToolUiResourceUri` throws on a non-`ui://` URI: the NAME is reported and the tool
            // is still registered (`:99-104`).
            live_tool("bad", None).with_meta(live_meta(json!({"ui": {"resourceUri": "http://x"}}))),
        ];

        let built = build_tool_metadata(
            &tools,
            &[],
            &definition,
            "srv",
            ToolPrefix::Server,
            Some(&config.mcp_servers),
            None,
            false,
        );

        let names: Vec<&str> = built.metadata.iter().map(|meta| meta.name.as_str()).collect();
        assert_eq!(names, vec!["srv_a_b", "srv_bad"]);
        assert_eq!(built.metadata[0].original_name, "a_b");
        assert_eq!(built.metadata[0].description, "visible");
        // `tool.description ?? ""`.
        assert_eq!(built.metadata[1].description, "");
        assert_eq!(built.failed_tools, vec!["(unnamed)".to_string(), "bad".to_string()]);
    }

    /// `state.toolMetadata` is not the direct-tool surface: the live walk applies **no**
    /// [`BUILTIN_NAMES`] check (`tool-metadata.ts` has none), and its resource arm falls back to
    /// `Read resource: ${uri}` (`:133`). `exposeResources: false` skips that arm entirely.
    #[test]
    fn the_live_walk_keeps_builtin_names_and_falls_back_to_the_uri_description() {
        let mut definition = entry(true);
        definition.tool_prefix = Some(ToolPrefix::None);
        let config = config_of(&[("srv", definition.clone())]);
        // Unprefixed, so it formats to the literal builtin name `resolve_direct_tools` drops.
        let tools = vec![live_tool("read", None)];
        let resources = vec![
            rmcp::model::Resource::new("file:///a", "notes"),
            rmcp::model::Resource::new("file:///b", "described").with_description("mine"),
        ];

        let built = build_tool_metadata(
            &tools,
            &resources,
            &definition,
            "srv",
            ToolPrefix::Server,
            Some(&config.mcp_servers),
            None,
            false,
        );
        assert_eq!(
            built.metadata.iter().map(|meta| meta.name.clone()).collect::<Vec<_>>(),
            vec!["read".to_string(), "read_notes".to_string(), "read_described".to_string()]
        );
        assert_eq!(built.metadata[1].description, "Read resource: file:///a");
        assert_eq!(built.metadata[1].resource_uri.as_deref(), Some("file:///a"));
        assert_eq!(built.metadata[2].description, "mine");

        let mut hidden_resources = definition.clone();
        hidden_resources.expose_resources = Some(false);
        let built = build_tool_metadata(
            &tools,
            &resources,
            &hidden_resources,
            "srv",
            ToolPrefix::Server,
            Some(&config.mcp_servers),
            None,
            false,
        );
        assert_eq!(
            built.metadata.iter().map(|meta| meta.name.clone()).collect::<Vec<_>>(),
            vec!["read".to_string()]
        );
    }

    /// `additionalCurrentCandidatesByToolName` (`tool-metadata.ts:60-73`), the arm that only
    /// exists here.
    ///
    /// `do_thing` is a legacy-only alias of `mine`'s `do-thing`, so on its own it excludes the
    /// tool. A second configured server with no metadata yet *would* mint `do_thing` as a current
    /// name once it connects — but only the `-`→`_` pass under
    /// `include_missing_configured_candidates` knows that, which is why the same call answers
    /// differently for the two flags. Getting this backwards drops a tool on the startup pass that
    /// every later refresh keeps.
    #[test]
    fn a_speculative_collision_suppresses_a_legacy_exclusion_only_on_the_startup_pass() {
        let mut mine = entry(true);
        mine.exclude_tools = Some(vec!["do_thing".to_string()]);
        let config = config_of(&[("mine", mine.clone()), ("other", entry(true))]);
        let tools = vec![live_tool("do-thing", None)];

        let registered = |include_missing: bool| {
            build_tool_metadata(
                &tools,
                &[],
                &mine,
                "mine",
                ToolPrefix::Server,
                Some(&config.mcp_servers),
                None,
                include_missing,
            )
            .metadata
            .len()
        };

        // Raw candidates only: `do_thing` names nobody, so the legacy alias excludes.
        assert_eq!(registered(false), 0);
        // …plus the normalised spellings: `other` would answer to `do_thing`, so the exclusion is
        // ambiguous and upstream keeps the tool.
        assert_eq!(registered(true), 1);
    }

    /// A server already in `known_metadata` contributes its **resolved** names, so it never lands
    /// in the speculative table — even when it is known to carry nothing at all
    /// (`if (knownTools)` is truthy for `[]`, `tool-metadata.ts:55`).
    #[test]
    fn a_known_server_takes_the_resolved_arm_even_when_it_knows_of_no_tools() {
        let mut mine = entry(true);
        mine.exclude_tools = Some(vec!["do_thing".to_string()]);
        let config = config_of(&[("mine", mine.clone()), ("other", entry(true))]);
        let tools = vec![live_tool("do-thing", None)];
        let mut known: IndexMap<String, Vec<ToolMetadata>> = IndexMap::new();
        known.insert("other".to_string(), Vec::new());

        // With `other` known-empty the speculative arm never runs, so the exclusion lands again
        // even under the startup flag.
        let built = build_tool_metadata(
            &tools,
            &[],
            &mine,
            "mine",
            ToolPrefix::Server,
            Some(&config.mcp_servers),
            Some(&known),
            true,
        );
        assert!(built.metadata.is_empty());

        // And a server whose resolved name IS the alias collides for real.
        known.insert(
            "other".to_string(),
            vec![ToolMetadata::new("do_thing", "do_thing", "")],
        );
        let built = build_tool_metadata(
            &tools,
            &[],
            &mine,
            "mine",
            ToolPrefix::Server,
            Some(&config.mcp_servers),
            Some(&known),
            true,
        );
        assert_eq!(built.metadata.len(), 1);
    }

    /// `reconstructToolMetadata` is not `resolveDirectTools`: the [`BUILTIN_NAMES`] drop list and
    /// the `directTools` selector are absent, a hidden cached tool is skipped, and a resource
    /// missing either half of `(name, uri)` is skipped (`metadata-cache.ts:247`).
    #[test]
    fn the_cache_walk_keeps_builtins_and_drops_half_written_resources() {
        let mut definition = entry(true);
        definition.tool_prefix = Some(ToolPrefix::None);
        let config = config_of(&[("srv", definition.clone())]);
        let mut entry_for_cache = cache_entry(vec![
            CachedTool {
                name: "read".to_string(),
                input_schema: Some(json!({"type": "object"})),
                ..CachedTool::default()
            },
            CachedTool {
                name: "hidden".to_string(),
                ui_visibility: Some(json!(["app"])),
                ..CachedTool::default()
            },
            cached_tool(""),
        ]);
        entry_for_cache.resources = Some(vec![
            CachedResource { uri: "file:///a".to_string(), name: "notes".to_string(), description: None },
            CachedResource { uri: String::new(), name: "nouri".to_string(), description: None },
            CachedResource { uri: "file:///c".to_string(), name: String::new(), description: None },
        ]);
        let cache = cache_of(&config, &[("srv", entry_for_cache)]);
        let stored = cache.servers.get("srv").expect("entry");

        let metadata = reconstruct_tool_metadata(
            "srv",
            stored,
            ToolPrefix::Server,
            &definition,
            Some(&config.mcp_servers),
            Some(&cache),
        );
        assert_eq!(
            metadata.iter().map(|meta| meta.name.clone()).collect::<Vec<_>>(),
            // `read` survives: the builtin collision is `resolve_direct_tools`' rule, not this one.
            vec!["read".to_string(), "read_notes".to_string()]
        );
        // Carried through UNNORMALISED — `normalize_direct_tool_input_schema` is registration's.
        assert_eq!(metadata[0].input_schema, Some(json!({"type": "object"})));
        assert_eq!(metadata[1].resource_uri.as_deref(), Some("file:///a"));

        // The same entry through `resolve_direct_tools` drops `read`, which is the difference the
        // two functions exist to keep: that one guards what the model may call by name.
        let specs = resolve_direct_tools(&config, Some(&cache), ToolPrefix::Server, None);
        assert!(specs.iter().all(|spec| spec.prefixed_name != "read"));
        assert!(specs.iter().any(|spec| spec.prefixed_name == "read_notes"));
    }

    /// `reconstructPromptMetadata` is now the ONE minter of prompt command names: the cache path
    /// reaches it through [`resolve_cached_prompts`] and the live path passes `rmcp` prompts
    /// straight in. Both must answer with the same `commandName` for the same prompt.
    #[test]
    fn one_prompt_walk_serves_the_cache_and_the_live_list() {
        let definition = entry(false);
        let live = vec![
            rmcp::model::Prompt::new(
                "brief",
                Some("Daily brief"),
                Some(vec![
                    rmcp::model::PromptArgument::new("day"),
                    rmcp::model::PromptArgument::new(""),
                ]),
            ),
            rmcp::model::Prompt::new("", None::<String>, None),
        ];
        let specs =
            reconstruct_prompt_metadata("demo", &live, ToolPrefix::Server, Some(&definition));
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].command_name, "mcp__demo__brief");
        assert_eq!(specs[0].description, "Daily brief");
        // The nameless argument is dropped, the named one survives.
        assert_eq!(
            specs[0].arguments,
            vec![CachedPromptArgument { name: "day".to_string(), description: None, required: None }]
        );

        let cached = vec![CachedPrompt {
            name: "brief".to_string(),
            description: Some("Daily brief".to_string()),
            arguments: Some(vec![CachedPromptArgument {
                name: "day".to_string(),
                description: None,
                required: None,
            }]),
            ..CachedPrompt::default()
        }];
        let from_cache =
            reconstruct_prompt_metadata("demo", &cached, ToolPrefix::Server, Some(&definition));
        assert_eq!(from_cache, specs);

        // `definition?` is optional upstream and stays optional here: no entry falls through to
        // the global mode rather than being unrepresentable.
        let global = reconstruct_prompt_metadata("demo", &cached, ToolPrefix::Mcp, None);
        assert_eq!(global[0].command_name, "mcp__mcp__demo__brief");
    }
}
