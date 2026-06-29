//! Typed host aggregation for the discovery/startup events (gap-08 #4). The guest answers
//! `project_trust` / `resources_discover` via `hook-outcome::handled(json)`; the host folds those
//! raw [`crate::HandledValue`]s into the typed decisions Pi's runner consumes:
//! `project_trust` → a `{trusted, remember}` decision (Pi `ProjectTrustEvent` result, types.ts:503;
//! runner.ts:1046 — the FIRST extension that returns a decision wins), and `resources_discover` →
//! the UNION of skill/prompt/theme paths every extension contributes (Pi `ResourcesEvent`,
//! types.ts:528; runner.ts:197), attributed back to the contributing extension.

use crate::contract::HandledValue;
use cyrup_core::ExtensionId;

/// A project-trust decision folded from the `project_trust` handlers (Pi `{trusted, remember}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectTrustDecision {
    /// Whether the project is trusted (project-local extensions may load).
    pub trusted: bool,
    /// Whether to persist the decision (Pi `remember`).
    pub remember: bool,
    /// The extension whose decision was taken (the first to answer, Pi semantics).
    pub by: ExtensionId,
}

/// Fold the collected `project_trust` handled values into a decision: the FIRST extension that
/// returns a parseable `{trusted, ...}` object decides (Pi runner.ts:1046). `None` = no extension
/// decided (the host falls back to its own trust prompt).
pub fn fold_project_trust(
    handled: &[(ExtensionId, HandledValue)],
) -> Option<ProjectTrustDecision> {
    for (id, HandledValue(v)) in handled {
        if let Some(trusted) = v.get("trusted").and_then(|t| t.as_bool()) {
            let remember = v.get("remember").and_then(|r| r.as_bool()).unwrap_or(false);
            return Some(ProjectTrustDecision { trusted, remember, by: id.clone() });
        }
    }
    None
}

/// The aggregated resources every extension provides (Pi `resources_discover`). Each path list is the
/// UNION across extensions, de-duplicated, preserving first-seen order; `by_extension` attributes
/// each contributing extension's `(skill, prompt, theme)` counts for diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourcesAggregate {
    pub skill_paths: Vec<String>,
    pub prompt_paths: Vec<String>,
    pub theme_paths: Vec<String>,
    /// `(extension, skills, prompts, themes)` contributed — attribution (gap-08 #4).
    pub by_extension: Vec<(ExtensionId, usize, usize, usize)>,
}

/// Fold the collected `resources_discover` handled values into the typed [`ResourcesAggregate`].
pub fn fold_resources(handled: &[(ExtensionId, HandledValue)]) -> ResourcesAggregate {
    let mut agg = ResourcesAggregate::default();
    for (id, HandledValue(v)) in handled {
        let s = extend_unique(&mut agg.skill_paths, v.get("skillPaths"));
        let p = extend_unique(&mut agg.prompt_paths, v.get("promptPaths"));
        let t = extend_unique(&mut agg.theme_paths, v.get("themePaths"));
        if s + p + t > 0 {
            agg.by_extension.push((id.clone(), s, p, t));
        }
    }
    agg
}

/// Append the string entries of a JSON array `field` into `dst`, skipping duplicates. Returns the
/// number of NEW entries added (for attribution). A non-array / absent field contributes nothing.
fn extend_unique(dst: &mut Vec<String>, field: Option<&serde_json::Value>) -> usize {
    let mut added = 0;
    if let Some(arr) = field.and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(s) = item.as_str()
                && !dst.iter().any(|e| e == s)
            {
                dst.push(s.to_string());
                added += 1;
            }
        }
    }
    added
}
