//! `defaultTools` — the baseline, the `+name`/`-name` modifiers, the layer merge and the
//! resolution (Pi `core/settings-manager.ts:215-254` @v1.0.0).
//!
//! CFG-097. The key first shipped as a plain selection that REPLACED pi's four built-ins
//! (`4d9aa837c`, narrowed by `541045ae0`). Upstream `30a1d1849` ("support +name/-name in
//! defaultTools") plus `db6cc71dc` turned it into a small language:
//!
//! * plain names still replace the baseline;
//! * `+name` adds and `-name` removes, applied **in list order** against that result;
//! * a settings layer whose list is **only** modifiers is APPENDED to the inherited list rather
//!   than replacing it, so a project file can adjust the global selection.
//!
//! The v1.0.0 CHANGELOG entry is the user-facing contract: *"Added `+name` and `-name` entries to
//! the `defaultTools` setting to add or remove tools without repeating the defaults, for example
//! `"defaultTools": ["+codemode"]`. Project entries of this form apply on top of the user
//! setting."*
//!
//! `docs/settings.md:40` is the user-facing form and `:50` the worked example
//! `"defaultTools": ["+codemode"]`, which before this module selected a tool literally named
//! `+codemode` and so started the session with no built-ins at all.
//!
//! The `/reload` activation rule at `docs/settings.md:56` ("`/reload` enables tools newly added to
//! `defaultTools`. It does not disable tools removed from it or re-enable unchanged tools you
//! turned off") is NOT this module's concern: upstream carries it with a `usesDefaultTools` flag on
//! the live session (`core/sdk.ts:448`, `core/agent-session.ts:267`), whereas cyrup's reload
//! rebuilds the session from settings. That is `cyrup-session-svc`'s reload contract and predates
//! v1.0.0.

use serde_json::Value;

/// Tools enabled at startup when `defaultTools` does not change them (Pi `DEFAULT_TOOL_NAMES`,
/// `core/settings-manager.ts:215` @v1.0.0 — `["read", "bash", "edit", "write"]`).
///
/// Upstream made this an exported constant in this window and deleted `core/sdk.ts`'s local
/// `defaultActiveToolNames` copy in favour of importing it (`sdk.ts:267`); `cyrup-session-svc`'s
/// `DEFAULT_BUILTIN_TOOLS` is an alias of this one for the same reason, so the baseline the
/// resolver starts from and the baseline the tool selector falls back to cannot drift apart.
pub const DEFAULT_TOOL_NAMES: [&str; 4] = ["read", "bash", "edit", "write"];

/// `isToolModifier` (Pi `core/settings-manager.ts:217-219`): an entry starting with `+` or `-`.
///
/// Note what this does NOT check: `"+"` and `"-"` on their own are modifiers here, and are dropped
/// later by [`resolve_default_tools`]'s empty-name guard rather than being treated as tool names.
pub fn is_tool_modifier(entry: &str) -> bool {
    entry.starts_with('+') || entry.starts_with('-')
}

/// `mergeDefaultTools(base, overrides)` (Pi `core/settings-manager.ts:225-230`), the one special
/// case `deepMergeSettings` applies over the generic deep merge.
///
/// A list containing any plain name REPLACES the inherited one; a list of **only** modifiers is
/// APPENDED. Upstream's comment is the rationale for the fall-through: *"Settings files are not
/// validated; a malformed value replaces instead of throwing here"* — so a non-array on either
/// side replaces, exactly as the generic merge would have done.
///
/// `None` for `over` is upstream's `overrides === undefined`, i.e. the override layer does not set
/// the key at all, which leaves the base untouched.
pub fn merge_default_tools(base: Option<&Value>, over: Option<&Value>) -> Option<Value> {
    let Some(over) = over else {
        return base.cloned();
    };
    let (Some(base_arr), Some(over_arr)) = (base.and_then(Value::as_array), over.as_array()) else {
        return Some(over.clone());
    };
    // `overrides.every(isToolModifier)` — and `every` is true for an empty list, so an explicit
    // `[]` in the override layer APPENDS nothing rather than clearing the inherited selection.
    // A non-string entry is not a modifier, so a malformed list replaces.
    if !over_arr
        .iter()
        .all(|e| e.as_str().is_some_and(is_tool_modifier))
    {
        return Some(over.clone());
    }
    let mut out = base_arr.clone();
    out.extend(over_arr.iter().cloned());
    Some(Value::Array(out))
}

/// `resolveDefaultTools(entries)` (Pi `core/settings-manager.ts:236-249`): plain names replace
/// [`DEFAULT_TOOL_NAMES`], then each `+name`/`-name` is applied in list order against that result.
///
/// The edge condition at `:238` is load-bearing and is ported verbatim:
/// `plain.length > 0 || entries.length === 0 ? plain : [...DEFAULT_TOOL_NAMES]`. So an explicit
/// `[]` stays empty — "no built-ins at all", which is a configured value and not "unset" — while a
/// modifier-only list starts from the four defaults.
///
/// `+name` appends only when the name is absent AND non-empty; `-name` removes the first match.
pub fn resolve_default_tools(entries: &[String]) -> Vec<String> {
    let plain: Vec<String> = entries
        .iter()
        .filter(|e| !is_tool_modifier(e))
        .cloned()
        .collect();
    let tools = if !plain.is_empty() || entries.is_empty() {
        plain
    } else {
        DEFAULT_TOOL_NAMES
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    };
    apply_tool_modifiers(&tools, entries)
}

/// `getToolListError(entries)` (Pi `core/settings-manager.ts:95-102` @f1b2e77f5, v1.1.0
/// `ddaa0a034`): a tool list from `--tools` or the SDK `tools` option is either an allowlist of
/// plain names and patterns, or a list of ONLY `+name`/`-name` entries with exact names. Returns
/// the problem (pi's text verbatim), or `None` when the list is valid.
///
/// A plain-name allowlist containing `*` is legal here; matching it is `MCP-616`'s business.
pub fn get_tool_list_error(entries: &[String]) -> Option<String> {
    let modifiers: Vec<&String> = entries.iter().filter(|e| is_tool_modifier(e)).collect();
    if modifiers.is_empty() {
        return None;
    }
    if modifiers.len() < entries.len() {
        return Some("tool names cannot be mixed with +name or -name entries".to_string());
    }
    modifiers.iter().find(|e| e.contains('*')).map(|pattern| {
        format!("+name and -name entries take exact tool names, not patterns: {pattern}")
    })
}

/// `applyToolModifiers(base, entries)` (Pi `core/settings-manager.ts:108-119` @f1b2e77f5): apply
/// the `+name` and `-name` entries of `entries` to `base` IN ORDER. `+name` appends when the name
/// is absent and non-empty; `-name` removes the first match. Other entries are ignored.
pub fn apply_tool_modifiers(base: &[String], entries: &[String]) -> Vec<String> {
    let mut tools = base.to_vec();
    for entry in entries {
        if !is_tool_modifier(entry) {
            continue;
        }
        // `entry.slice(1)`, without raw indexing: `is_tool_modifier` guarantees one of the two
        // prefixes, and the `unwrap_or` leg is unreachable rather than a silent default.
        let name = entry
            .strip_prefix('+')
            .or_else(|| entry.strip_prefix('-'))
            .unwrap_or(entry.as_str());
        let index = tools.iter().position(|t| t == name);
        match (entry.starts_with('+'), index) {
            (true, None) if !name.is_empty() => tools.push(name.to_string()),
            (false, Some(i)) => {
                tools.remove(i);
            }
            _ => {}
        }
    }
    tools
}
