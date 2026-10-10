//! `defaultTools` for the tools that are not built in: `codemode`, `tool_search`, MCP tools, and
//! any other extension tool the setting names.
//!
//! Pi hands the session ONE list of initial active names — `options.tools ?? (options.noTools ? []
//! : (configuredDefaultToolNames ?? DEFAULT_TOOL_NAMES))` (`core/sdk.ts:274-276` @v1.0.4) — and the
//! session activates every registered tool whose name is in it (`_refreshToolRegistry`, which then
//! runs `_applyToolLoadout`, dropping only `hidden` tools, `core/agent-session.ts:3489-3578` and
//! `:1561-1566` @v1.0.4). A name that is in no registry is dropped without a word.
//!
//! cyrup splits that one list in two. [`crate::builder`]'s `select_active_tools` walks the
//! built-in registry, so it can only ever answer for `read`/`bash`/`edit`/`write` and the other
//! built-ins; an extension tool registers after it and was never offered the configured names, so
//! `"defaultTools": ["+codemode"]` — the documented way to switch `codemode` on — left it
//! registered and inactive. This module is the half that answers for everything else.
//!
//! # CYRUP-DELTA
//!
//! * A configured name no tool has registered by the time the session is built stays pending
//!   (pi's `_pendingToolNames`, `agent-session.ts:437` @v1.0.4) until a tool of that name
//!   registers, such as an MCP server's tool, or until the first run starts. Pi keeps pending
//!   names only for a transcript restore and a reload; it drops an unregistered default name.
//! * A configured name that matches nothing — not registered, or registered `hidden`, and not an
//!   MCP name — is reported once as a startup warning. Pi is silent.
//! * cyrup rebuilds the session on `/reload` and restores the recorded loadout, so a build has no
//!   previous settings object to compare with. The `/reload` rebuild therefore offers every
//!   configured name ([`DefaultToolInputs::started_with`] holds them all) and the reload activates
//!   the ones the setting newly adds afterwards, against the replaced session's own `defaultTools`
//!   (`AgentSession::activate_added_default_tools`, pi's `previousDefaultTools`,
//!   `agent-session.ts:3660-3685` @v1.1.0), so a name turned off, removed from the setting and
//!   later added again activates, as in pi. Everywhere else a session is restored (a resume,
//!   possibly in another process) there is no earlier load to ask, and `started_with` is the tools
//!   the session's first system message declared, which are what the defaults were when the
//!   session began. A name configured now that the session did not start with is newly added; a
//!   name the session started with and the user later turned off stays off, and a name removed
//!   from the setting stays on: pi's rule at `docs/settings.md`.

use std::collections::HashSet;
use std::sync::Arc;

use cyrup_core::Tool;

/// What [`plan`] reads.
pub(crate) struct DefaultToolInputs<'a> {
    /// Whether the configured names seed the session at all: the selection that remains after any
    /// `+name`/`-name` modifiers were applied to the defaults carries neither a `--tools` list nor
    /// `--no-tools` / `--no-builtin-tools`. That is pi's initial active names
    /// (`options.tools ?? (options.noTools ? [] : configuredDefaultToolNames ?? ...)`, then
    /// `applyToolModifiers`, `sdk.ts:274-294` @v1.1.0): `--no-builtin-tools` with `--tools
    /// +read,+codemode` still starts `codemode`, the modifiers filling the empty base, while
    /// `--no-builtin-tools` alone, or `--tools read`, applies none of the names. `false` applies
    /// nothing.
    pub names_apply: bool,
    /// The effective `defaultTools` after `+name`/`-name` resolution, or pi's four defaults when the
    /// setting is unset (`configuredDefaultToolNames ?? DEFAULT_TOOL_NAMES`).
    pub configured: &'a [String],
    /// `--exclude-tools`: pi filters them out of the initial names (`sdk.ts:276`).
    pub excluded: &'a HashSet<String>,
    /// The names the session already had a chance to start with: on a `/reload` every configured
    /// name (the reload activates the newly added ones itself), otherwise the tools the resumed
    /// session's first system message declared. `None` for a session with no transcript, where
    /// every configured name is new. See the module's CYRUP-DELTA note.
    pub started_with: Option<&'a [String]>,
}

/// What the builder does with the configured names.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct DefaultToolPlan {
    /// Registered tools to start the session with, in configured order.
    pub activate: Vec<String>,
    /// Names no tool has registered yet; they activate when one does.
    pub pending: Vec<String>,
    /// The names that match no tool, for the startup warning.
    pub unmatched: Vec<String>,
}

impl DefaultToolPlan {
    /// The one startup warning for [`Self::unmatched`], or `None` when everything matched.
    pub(crate) fn warning(&self) -> Option<String> {
        if self.unmatched.is_empty() {
            return None;
        }
        let quoted: Vec<String> = self.unmatched.iter().map(|n| format!("\"{n}\"")).collect();
        Some(format!(
            "defaultTools: no activatable tool is registered as {}",
            quoted.join(", ")
        ))
    }
}

/// Resolve the configured names against `registry`, every tool the session registered.
pub(crate) fn plan(inputs: &DefaultToolInputs<'_>, registry: &[Arc<dyn Tool>]) -> DefaultToolPlan {
    if !inputs.names_apply {
        return DefaultToolPlan::default();
    }
    let find = |name: &str| registry.iter().find(|t| t.name() == name);
    let mut out = DefaultToolPlan::default();
    let mut seen: HashSet<&str> = HashSet::new();
    for name in inputs.configured {
        if cyrup_core::tool_name_matches(inputs.excluded, name) || !seen.insert(name.as_str()) {
            continue;
        }
        let registered = find(name);
        let activatable = registered.is_some_and(|t| t.exposure().can_be_activated());
        if !activatable && !(registered.is_none() && cyrup_core::is_mcp_tool_name(name)) {
            out.unmatched.push(name.clone());
        }
        let wanted = inputs
            .started_with
            .is_none_or(|started| !started.contains(name));
        if !wanted {
            continue;
        }
        match registered {
            Some(_) if activatable => out.activate.push(name.clone()),
            // A `hidden` tool: "Activating it has no effect" (pi `_applyToolLoadout`).
            Some(_) => {}
            None => out.pending.push(name.clone()),
        }
    }
    out
}

/// The tools the first system message of a transcript declares (see
/// [`DefaultToolInputs::started_with`]). `None` when the transcript has no system message.
pub(crate) fn declared_at_start(messages: &[cyrup_session::AgentMessage]) -> Option<Vec<String>> {
    let first = messages.iter().find(|m| {
        matches!(
            m,
            cyrup_session::AgentMessage::Core(cyrup_core::Message::System(_))
        )
    })?;
    crate::tools::declared_tool_names(std::slice::from_ref(first))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use cyrup_core::{
        CancelToken, ToolCallId, ToolError, ToolExposure, ToolResult, ToolUpdateSink,
    };
    use serde_json::Value;

    struct Named {
        name: &'static str,
        exposure: ToolExposure,
        schema: Value,
    }

    #[async_trait::async_trait]
    impl Tool for Named {
        fn name(&self) -> &str {
            self.name
        }
        fn parameters(&self) -> &Value {
            &self.schema
        }
        fn description(&self) -> &str {
            "t"
        }
        fn exposure(&self) -> ToolExposure {
            self.exposure
        }
        fn default_active(&self) -> bool {
            false
        }
        async fn execute(
            &self,
            _call_id: ToolCallId,
            _params: Value,
            _cancel: CancelToken,
            _on_update: ToolUpdateSink,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::default())
        }
    }

    fn registry(tools: &[(&'static str, ToolExposure)]) -> Vec<Arc<dyn Tool>> {
        tools
            .iter()
            .map(|(name, exposure)| {
                Arc::new(Named {
                    name,
                    exposure: *exposure,
                    schema: serde_json::json!({}),
                }) as Arc<dyn Tool>
            })
            .collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn run(
        configured: &[&str],
        started_with: Option<&[String]>,
        registry: &[Arc<dyn Tool>],
    ) -> DefaultToolPlan {
        let configured = names(configured);
        let excluded = HashSet::new();
        plan(
            &DefaultToolInputs {
                names_apply: true,
                configured: &configured,
                excluded: &excluded,
                started_with,
            },
            registry,
        )
    }

    #[test]
    fn a_registered_inactive_tool_named_in_the_setting_is_activated_whatever_its_exposure() {
        let reg = registry(&[
            ("codemode", ToolExposure::ModelOnly),
            ("lookup", ToolExposure::Deferred),
            ("sandboxed", ToolExposure::Codemode),
        ]);
        let planned = run(&["codemode", "lookup", "sandboxed"], None, &reg);
        assert_eq!(planned.activate, ["codemode", "lookup", "sandboxed"]);
        assert!(planned.pending.is_empty() && planned.unmatched.is_empty());
    }

    #[test]
    fn an_unregistered_name_stays_pending_and_is_reported_unless_it_is_an_mcp_name() {
        let planned = run(&["nope", "mcp__docs__find", "read_mcp_resource"], None, &[]);
        assert_eq!(
            planned.pending,
            ["nope", "mcp__docs__find", "read_mcp_resource"]
        );
        assert_eq!(planned.unmatched, ["nope"]);
        assert_eq!(
            planned.warning().as_deref(),
            Some("defaultTools: no activatable tool is registered as \"nope\"")
        );
    }

    #[test]
    fn a_hidden_tool_matches_nothing_and_is_not_made_pending() {
        let reg = registry(&[("secret", ToolExposure::Hidden)]);
        let planned = run(&["secret"], None, &reg);
        assert!(planned.activate.is_empty() && planned.pending.is_empty());
        assert_eq!(planned.unmatched, ["secret"]);
    }

    #[test]
    fn a_session_that_started_with_a_name_does_not_get_it_back_but_a_new_name_is_added() {
        let reg = registry(&[
            ("codemode", ToolExposure::ModelOnly),
            ("tool_search", ToolExposure::Direct),
        ]);
        let started = names(&["read", "tool_search"]);
        let planned = run(&["tool_search", "codemode"], Some(&started), &reg);
        assert_eq!(planned.activate, ["codemode"]);
        assert!(planned.unmatched.is_empty());
    }

    #[test]
    fn exclusion_and_a_session_with_its_own_tool_selection_suppress_the_names() {
        let reg = registry(&[("codemode", ToolExposure::ModelOnly)]);
        let configured = names(&["codemode"]);
        let excluded: HashSet<String> = names(&["codemode"]).into_iter().collect();
        let base = |uses, excluded: &HashSet<String>| {
            plan(
                &DefaultToolInputs {
                    names_apply: uses,
                    configured: &configured,
                    excluded,
                    started_with: None,
                },
                &reg,
            )
        };
        assert_eq!(base(true, &excluded), DefaultToolPlan::default());
        assert_eq!(base(false, &HashSet::new()), DefaultToolPlan::default());
        assert_eq!(base(true, &HashSet::new()).activate, ["codemode"]);
    }

    /// `--exclude-tools` entries are patterns (`createToolNameMatcher`, `sdk.ts` @v1.0.4 filters
    /// the initial names by it), so a configured name a pattern matches is not activated.
    #[test]
    fn an_exclusion_pattern_suppresses_the_names_it_matches() {
        let reg = registry(&[
            ("codemode", ToolExposure::ModelOnly),
            ("tool_search", ToolExposure::ModelOnly),
        ]);
        let configured = names(&["codemode", "tool_search"]);
        let excluded: HashSet<String> = names(&["code*"]).into_iter().collect();
        let planned = plan(
            &DefaultToolInputs {
                names_apply: true,
                configured: &configured,
                excluded: &excluded,
                started_with: None,
            },
            &reg,
        );
        assert_eq!(planned.activate, ["tool_search"]);
    }
}
