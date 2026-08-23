//! Reusable permission-policy decision logic for the `tool_call` gate (R-12-005/006).
//!
//! This is the *pure, unit-testable* core of an opt-in permission gate. The gate **wiring** lives in
//! `cyrup-agent` (`before_tool_call`); this module provides only the decision function it consults,
//! mirroring Pi's gate contract (`{ block, reason } | { input } | undefined`). A
//! [`PermissionPolicy`] is an ordered list of [`Rule`]s over a tool's name + parsed JSON input; the
//! first matching rule decides. With **no** rules (the default), every call returns
//! [`PolicyDecision::Proceed`] — the YOLO default (R-12-001/002): nothing is gated unless a policy is
//! explicitly built and consulted.
//!
//! [`PolicyDecision::Proceed`] / [`PolicyDecision::Mutate`] / [`PolicyDecision::Block`] are the exact
//! Pi-contract triple. [`PolicyDecision::Confirm`] is the buildable confirm hook (R-12-009): a pure
//! function cannot prompt, so it yields `Confirm` for the agent gate to resolve via `ctx.ui.confirm`
//! (and block-by-default when `has_ui == false`).

use crate::isolation::ProtectedPaths;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

/// The decision an agent `tool_call` gate acts on. `Proceed`/`Mutate`/`Block` mirror Pi's contract;
/// `Confirm` defers to host UI (resolved by the gate, not here).
#[derive(Clone, Debug, PartialEq)]
pub enum PolicyDecision {
    /// No rule intervened — run the tool with its original input.
    Proceed,
    /// Run the tool, but with this rewritten input (argument rewriting, R-12-005).
    Mutate { input: Value },
    /// Do not run; emit an `isError` tool result carrying `reason` (R-12-005/006).
    Block { reason: String },
    /// Ask the user (gate resolves via `UiConfirm`; block-by-default with no UI, R-12-009).
    Confirm { reason: String },
}

/// Predicate over `(tool_name, input)` — `true` selects the rule.
type ArgPred = Arc<dyn Fn(&str, &Value) -> bool + Send + Sync>;
/// Input rewriter for a [`RuleAction::Mutate`] rule.
type Rewriter = Arc<dyn Fn(&str, &Value) -> Value + Send + Sync>;

#[derive(Clone)]
enum RuleAction {
    Allow,
    Deny(String),
    Confirm(String),
    Mutate(Rewriter),
}

/// One policy rule: a matcher plus the action to take when it matches.
#[derive(Clone)]
pub struct Rule {
    matches: ArgPred,
    action: RuleAction,
}

impl Rule {
    /// Start building a rule from a `(tool, input) -> bool` matcher.
    pub fn when<F>(pred: F) -> RuleBuilder
    where
        F: Fn(&str, &Value) -> bool + Send + Sync + 'static,
    {
        RuleBuilder {
            matches: Arc::new(pred),
        }
    }
}

/// Builder produced by [`Rule::when`]; pick a terminal action.
pub struct RuleBuilder {
    matches: ArgPred,
}

impl RuleBuilder {
    /// Allow matching calls (`Proceed`). Useful as an early allow-list entry before broader denies.
    pub fn allow(self) -> Rule {
        Rule {
            matches: self.matches,
            action: RuleAction::Allow,
        }
    }

    /// Block matching calls with `reason`.
    pub fn deny(self, reason: impl Into<String>) -> Rule {
        Rule {
            matches: self.matches,
            action: RuleAction::Deny(reason.into()),
        }
    }

    /// Require confirmation for matching calls.
    pub fn confirm(self, prompt: impl Into<String>) -> Rule {
        Rule {
            matches: self.matches,
            action: RuleAction::Confirm(prompt.into()),
        }
    }

    /// Rewrite the input of matching calls.
    pub fn mutate<F>(self, rewrite: F) -> Rule
    where
        F: Fn(&str, &Value) -> Value + Send + Sync + 'static,
    {
        Rule {
            matches: self.matches,
            action: RuleAction::Mutate(Arc::new(rewrite)),
        }
    }
}

/// An ordered set of [`Rule`]s consulted by an agent `tool_call` gate. First match wins; no match
/// (including the empty policy) is [`PolicyDecision::Proceed`].
#[derive(Clone, Default)]
pub struct PermissionPolicy {
    rules: Vec<Rule>,
}

impl PermissionPolicy {
    /// An empty policy — every call proceeds (the YOLO default, R-12-001).
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Append a rule (builder style).
    #[must_use]
    pub fn with_rule(mut self, rule: Rule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Append a rule in place.
    pub fn push(&mut self, rule: Rule) {
        self.rules.push(rule);
    }

    /// Number of rules (0 == YOLO default).
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// True when no rules are configured.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Decide what to do with a pending tool call. Pure: no I/O, deterministic in its rules.
    pub fn evaluate(&self, tool: &str, input: &Value) -> PolicyDecision {
        for rule in &self.rules {
            if (rule.matches)(tool, input) {
                return match &rule.action {
                    RuleAction::Allow => PolicyDecision::Proceed,
                    RuleAction::Deny(reason) => PolicyDecision::Block {
                        reason: reason.clone(),
                    },
                    RuleAction::Confirm(reason) => PolicyDecision::Confirm {
                        reason: reason.clone(),
                    },
                    RuleAction::Mutate(f) => PolicyDecision::Mutate {
                        input: f(tool, input),
                    },
                };
            }
        }
        PolicyDecision::Proceed
    }
}

// ----------------------------------------------------------------- matcher helpers

/// Matcher: the call targets tool `name`.
pub fn is_tool(name: &'static str) -> impl Fn(&str, &Value) -> bool {
    move |tool, _| tool == name
}

/// Matcher: a `bash` call whose `command` contains `needle`.
pub fn bash_command_contains(needle: impl Into<String>) -> impl Fn(&str, &Value) -> bool {
    let needle = needle.into();
    move |tool, input| {
        tool == "bash"
            && input
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|c| c.contains(&needle))
    }
}

/// True for a few obviously destructive shell patterns. Heuristic only (a real policy composes its
/// own matchers); used by [`dangerous_bash_rule`].
pub fn is_dangerous_command(command: &str) -> bool {
    const NEEDLES: [&str; 4] = ["rm -rf", "rm -fr", "mkfs", ":(){ :|:& };:"];
    NEEDLES.iter().any(|n| command.contains(n))
}

// ----------------------------------------------------------------- rule helpers

/// Path-protection rule (R-12-006): deny `write`/`edit` whose `path` argument is a protected path.
/// This is the policy-seam sibling of [`crate::isolation::ProtectedFs`] (the backend-seam version).
pub fn protected_path_rule(paths: ProtectedPaths) -> Rule {
    Rule::when(move |tool, input| {
        matches!(tool, "write" | "edit")
            && input
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(|p| paths.is_protected(Path::new(p)))
    })
    .deny("write to protected path denied")
}

/// Confirm-before-dangerous-`bash` rule (R-12-005): yields [`PolicyDecision::Confirm`] for obviously
/// destructive commands so the gate can prompt (and block-by-default with no UI).
pub fn dangerous_bash_rule() -> Rule {
    Rule::when(|tool, input| {
        tool == "bash"
            && input
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(is_dangerous_command)
    })
    .confirm("potentially destructive command")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_policy_proceeds() {
        let p = PermissionPolicy::new();
        assert!(p.is_empty());
        assert_eq!(
            p.evaluate("bash", &json!({"command": "rm -rf /"})),
            PolicyDecision::Proceed
        );
    }

    #[test]
    fn deny_dangerous_bash() {
        let p = PermissionPolicy::new()
            .with_rule(Rule::when(bash_command_contains("rm -rf")).deny("no rm -rf"));
        assert_eq!(
            p.evaluate("bash", &json!({"command": "rm -rf build"})),
            PolicyDecision::Block {
                reason: "no rm -rf".into()
            }
        );
        // A different bash command is unaffected.
        assert_eq!(
            p.evaluate("bash", &json!({"command": "ls"})),
            PolicyDecision::Proceed
        );
    }

    #[test]
    fn confirm_rule() {
        let p = PermissionPolicy::new().with_rule(dangerous_bash_rule());
        assert_eq!(
            p.evaluate("bash", &json!({"command": "mkfs /dev/sda"})),
            PolicyDecision::Confirm {
                reason: "potentially destructive command".into()
            }
        );
    }

    #[test]
    fn mutate_rewrites_input() {
        let p =
            PermissionPolicy::new().with_rule(Rule::when(is_tool("bash")).mutate(|_t, input| {
                let mut v = input.clone();
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("command".into(), json!("echo safe"));
                }
                v
            }));
        let decision = p.evaluate("bash", &json!({"command": "rm -rf /"}));
        assert_eq!(
            decision,
            PolicyDecision::Mutate {
                input: json!({"command": "echo safe"})
            }
        );
    }

    #[test]
    fn protected_path_rule_blocks_write_passes_others() {
        let p = PermissionPolicy::new().with_rule(protected_path_rule(ProtectedPaths::defaults()));
        // write to .env -> Block
        assert!(matches!(
            p.evaluate("write", &json!({"path": ".env", "content": "x"})),
            PolicyDecision::Block { .. }
        ));
        // edit to .git/config -> Block
        assert!(matches!(
            p.evaluate("edit", &json!({"path": ".git/config"})),
            PolicyDecision::Block { .. }
        ));
        // write to a normal file -> Proceed
        assert_eq!(
            p.evaluate("write", &json!({"path": "src/main.rs", "content": "x"})),
            PolicyDecision::Proceed
        );
        // read is never gated by this rule
        assert_eq!(
            p.evaluate("read", &json!({"path": ".env"})),
            PolicyDecision::Proceed
        );
    }

    #[test]
    fn first_matching_rule_wins() {
        let p = PermissionPolicy::new()
            .with_rule(Rule::when(bash_command_contains("git push")).allow())
            .with_rule(Rule::when(is_tool("bash")).deny("all bash blocked"));
        // The allow rule precedes the broad deny.
        assert_eq!(
            p.evaluate("bash", &json!({"command": "git push origin"})),
            PolicyDecision::Proceed
        );
        assert_eq!(
            p.evaluate("bash", &json!({"command": "ls"})),
            PolicyDecision::Block {
                reason: "all bash blocked".into()
            }
        );
    }
}
