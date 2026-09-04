//! Small shared helpers used across `discovery/management/`'s other leaf modules: package-identifier
//! validation, source/context rendering, scope parsing, name sanitization, and scope-directory
//! resolution. Split out of `discovery/management.rs`'s own "Package-identifier validation" and
//! "Small shared helpers" sections — the two are merged into one file here because the traced call
//! graph shows every item in both is `pub(crate)`-shared across at least two other leaf files
//! (unlike `parse_csv`/`parse_tools`/`default_system_prompt_mode`/`default_inherit_project_context`,
//! which the original banner also grouped here but which turned out to have exactly one caller
//! each — those moved to `config_parse.rs`/`handlers.rs` respectively instead, co-located with
//! their sole caller).

use std::path::PathBuf;

use super::super::AgentDiscoveryConfig;
use super::super::package_name::{collapse_repeated_char, is_valid_package_identifier};
use super::super::types::{AgentSource, OverrideScope};
use crate::fork_context::ContextMode;

/// Normalize + validate a caller-supplied package identifier exactly per R-SA-006's grammar:
/// lowercase, whitespace runs -> `-`, strip anything outside `[a-z0-9.-]`, collapse repeated
/// `-`/`.` runs, trim leading/trailing `-`/`.`, then require
/// `^[a-z0-9][a-z0-9-]*(?:\.[a-z0-9][a-z0-9-]*)*$`.
///
/// Returns `Ok(None)` for an absent/empty/whitespace-only input (not a validation failure).
/// Returns `Ok(None)` — **not** `Err` — for a non-empty input that fails to normalize to a valid
/// identifier: per this module's own "invalid package identifier -> silent skip, not an error"
/// contract (R-SA-004/011 taxonomy note: discovery's per-file skip behavior for this exact
/// condition, R-SA-006, is mirrored here rather than promoted to a hard management-layer error),
/// callers that receive `Ok(None)` from a caller-supplied non-empty package value MUST treat the
/// whole create/update call as skipped (a no-op returning `Ok(None)` at the call-site level, see
/// [`crate::discovery::management::agent_crud::create_agent`]/[`crate::discovery::management::agent_crud::update_agent`])
/// rather than surfacing a `SubagentError`.
///
/// The two shared primitives (`collapse_repeated_char`, `is_valid_package_identifier`) are
/// imported from `crate::discovery::package_name`, the crate's single port; only the outer
/// normalize/collapse-whitespace sequencing below is written out locally, to preserve this
/// function's own `Option`-shaped error handling (see `package_name.rs`'s module doc for why
/// three differently-shaped callers exist instead of one shared function).
pub(crate) fn normalize_package_identifier(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lowered = trimmed.to_lowercase();
    let mut collapsed_ws = String::with_capacity(lowered.len());
    let mut last_was_ws = false;
    for ch in lowered.chars() {
        if ch.is_whitespace() {
            if !last_was_ws {
                collapsed_ws.push('-');
            }
            last_was_ws = true;
        } else {
            collapsed_ws.push(ch);
            last_was_ws = false;
        }
    }
    let filtered: String = collapsed_ws
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    let collapsed_hyphen = collapse_repeated_char(&filtered, '-');
    let collapsed_dot = collapse_repeated_char(&collapsed_hyphen, '.');
    let final_name = collapsed_dot
        .trim_start_matches(['-', '.'])
        .trim_end_matches(['-', '.'])
        .to_string();

    if final_name.is_empty() || !is_valid_package_identifier(&final_name) {
        return None;
    }
    Some(final_name)
}

/// The camelCase source label pi renders (`AgentSource` serde `rename_all = "camelCase"`).
pub(crate) fn source_str(source: AgentSource) -> &'static str {
    match source {
        AgentSource::Builtin => "builtin",
        AgentSource::Package => "package",
        AgentSource::User => "user",
        AgentSource::Project => "project",
        // SUBA-084 — the `- <name> (runtime, …)` list label (`agent-management.ts:849` @v0.64.0
        // `["runtime", "Runtime agents"]`; `runtime-agent-registration.test.ts:229`).
        AgentSource::Runtime => "runtime",
    }
}

pub(crate) fn context_str(mode: ContextMode) -> &'static str {
    match mode {
        ContextMode::Fresh => "fresh",
        ContextMode::Fork => "fork",
    }
}

pub(crate) fn override_scope_str(scope: OverrideScope) -> &'static str {
    match scope {
        OverrideScope::User => "user",
        OverrideScope::Project => "project",
    }
}

/// pi `asDisambiguationScope` (`agent-management.ts:79-82`): `"user"`/`"project"` pass through,
/// anything else (incl. absent / `"both"`) is `None`.
pub(crate) fn disambiguation_scope(scope: Option<&str>) -> Option<AgentSource> {
    match scope {
        Some("user") => Some(AgentSource::User),
        Some("project") => Some(AgentSource::Project),
        _ => None,
    }
}

/// pi `normalizeListScope` (`agent-management.ts:90-94`): absent -> both; `"user"`/`"project"`/
/// `"both"` pass through; any other value falls back to both. `None` here means "both".
pub(crate) fn normalize_list_scope(scope: Option<&str>) -> Option<AgentSource> {
    match scope {
        Some("user") => Some(AgentSource::User),
        Some("project") => Some(AgentSource::Project),
        _ => None,
    }
}

/// pi `sanitizeName` (`agent-management.ts:96-98`): `lowercase`, `trim`, `\s+`->`-`, strip
/// `[^a-z0-9-]`, `-+`->`-`, trim leading/trailing `-`.
pub(crate) fn sanitize_name(name: &str) -> String {
    let lowered = name.to_lowercase();
    let trimmed = lowered.trim();
    let mut ws_collapsed = String::with_capacity(trimmed.len());
    let mut last_ws = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if !last_ws {
                ws_collapsed.push('-');
            }
            last_ws = true;
        } else {
            ws_collapsed.push(ch);
            last_ws = false;
        }
    }
    let filtered: String = ws_collapsed
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect();
    let collapsed = collapse_repeated_char(&filtered, '-');
    collapsed.trim_matches('-').to_string()
}

/// The writable scope directory for a create, derived from the [`AgentDiscoveryConfig`] the same way
/// discovery scans it (so create and the next discovery pass agree on where the file lives).
///
/// The per-scope directory lists are ordered lowest-precedence-first (legacy `.agents` / extra dirs
/// early, the preferred `.cyrup/agents` — or the user's second `~/.agents` once it exists — last),
/// so the write target is the **last** (highest-precedence) entry: pi's `d.projectDir` = preferred
/// `<root>/.cyrup/agents` for a project create, and `d.userDir` = new-if-exists-else-old for a user
/// create (agent-management.ts:697-699, agents.ts:1420) — both the last entry under the topology
/// helpers' ordering. (For a single-entry list `first`/`last` coincide; only the multi-dir topology
/// distinguishes them.)
pub(crate) fn pick_scope_dir(
    cfg: &AgentDiscoveryConfig,
    scope: AgentSource,
    is_chain: bool,
) -> Option<PathBuf> {
    let dirs = match (scope, is_chain) {
        (AgentSource::User, false) => &cfg.user_agent_dirs,
        (AgentSource::User, true) => &cfg.user_chain_dirs,
        (AgentSource::Project, false) => &cfg.project_agent_dirs,
        (AgentSource::Project, true) => &cfg.project_chain_dirs,
        _ => return None,
    };
    dirs.last().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_package_identifier_matches_frontmatter_rs_validation_fixtures() {
        // Same fixture set `frontmatter.rs`'s own tests pin, to guard this module's outer
        // normalize sequencing against drift from `frontmatter.rs::parse_package_name`'s (the
        // two primitives both call into are shared via `super::super::package_name`; see this
        // function's own doc for why the validator is imported rather than duplicated).
        assert_eq!(normalize_package_identifier(None), None);
        assert_eq!(normalize_package_identifier(Some("")), None);
        assert_eq!(normalize_package_identifier(Some("   ---   ")), None);
        assert_eq!(normalize_package_identifier(Some("!!!")), None);
        assert_eq!(
            normalize_package_identifier(Some("Code Analysis!")),
            Some("code-analysis".to_string())
        );
        assert_eq!(
            normalize_package_identifier(Some("acme")),
            Some("acme".to_string())
        );
        assert_eq!(
            normalize_package_identifier(Some("acme.tools")),
            Some("acme.tools".to_string())
        );
    }
}
