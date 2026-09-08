//! `preview_simple_workflow_run` — pi `previewSimpleWorkflowRun` (`scripted-workflow.ts:1370-1386`):
//! a display-only preview for the exact simple `return runs.run(key, {...})` form. **Regex-driven,
//! not AST-based**, exactly as upstream — one anchored pattern plus one pattern per property.
//!
//! This is the second and last `fancy-regex` site in this crate (the first is
//! [`super::recovery`]); every pattern here is lookaround-free, so `fancy-regex` delegates to the
//! linear-time `regex` engine (`RegexImpl::Wrap`) and no backtracking budget applies.

use std::sync::LazyLock;

use fancy_regex::Regex;

/// pi `SimpleWorkflowRunPreview` (`scripted-workflow.ts:1365-1368`) — both fields optional, and an
/// absent field is ABSENT in JSON (upstream spreads conditionally), not `null`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct SimpleWorkflowRunPreview {
    /// The statically readable `agent` string literal, when present and decodable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The statically readable `task` string literal, when present and decodable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// The anchored whole-script pattern (`:1371`): `return` (optionally `await`) `runs.run(<string
/// literal>, { ... })` and nothing else. Group 1 is the `{...}` body.
static SIMPLE_RUN_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^\s*return\s+(?:await\s+)?runs\.run\s*\(\s*(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`[^`$\\]*`)\s*,\s*\{([\s\S]*)\}\s*\)\s*;?\s*$"#,
    )
    .unwrap_or_else(|error| unreachable!("SIMPLE_RUN_PATTERN is a fixed valid pattern: {error}"))
});

/// One per-property pattern — upstream builds `new RegExp(...)` per call (`:1374`); the two
/// possible names are known statically, so both are compiled once. Group 1 is the quoted literal.
fn property_pattern(name: &str) -> Regex {
    Regex::new(&format!(
        r#"(?:^|,)\s*(?:{name}|["']{name}["'])\s*:\s*("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`[^`$\\]*`)"#,
    ))
    .unwrap_or_else(|error| unreachable!("property pattern is fixed and valid: {error}"))
}

static AGENT_PATTERN: LazyLock<Regex> = LazyLock::new(|| property_pattern("agent"));
static TASK_PATTERN: LazyLock<Regex> = LazyLock::new(|| property_pattern("task"));

/// pi `readProperty` (`:1373-1383`): decode group 1 of the property match. `"`-literals decode via
/// JSON (`JSON.parse`; failure ⇒ `None`, `:1378`); `'`/`` ` ``-literals containing **any**
/// backslash ⇒ `None` (`:1380`), otherwise the raw inner slice.
fn read_property(body: &str, pattern: &Regex) -> Option<String> {
    let literal = pattern
        .captures(body)
        .ok()
        .flatten()?
        .get(1)
        .map(|m| m.as_str())?;
    if literal.is_empty() {
        return None;
    }
    if literal.starts_with('"') {
        return serde_json::from_str::<String>(literal).ok();
    }
    let inner = literal.get(1..literal.len().saturating_sub(1))?;
    if inner.contains('\\') {
        return None;
    }
    Some(inner.to_string())
}

/// pi `previewSimpleWorkflowRun` (`scripted-workflow.ts:1370-1386`). `None` when the script is not
/// the exact simple form; otherwise whichever of `agent`/`task` are statically readable string
/// literals.
#[must_use]
pub fn preview_simple_workflow_run(script: Option<&str>) -> Option<SimpleWorkflowRunPreview> {
    let script = script?;
    let captures = SIMPLE_RUN_PATTERN.captures(script).ok().flatten()?;
    let body = captures.get(1)?.as_str();
    Some(SimpleWorkflowRunPreview {
        agent: read_property(body, &AGENT_PATTERN),
        task: read_property(body, &TASK_PATTERN),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::preview_simple_workflow_run;

    #[test]
    fn reads_the_exact_simple_form() {
        let preview = preview_simple_workflow_run(Some(
            "return runs.run(\"key\", { agent: \"worker\", task: \"Do it\" });",
        ))
        .unwrap();
        assert_eq!(preview.agent.as_deref(), Some("worker"));
        assert_eq!(preview.task.as_deref(), Some("Do it"));
    }

    #[test]
    fn accepts_await_and_quoted_property_names() {
        let preview = preview_simple_workflow_run(Some(
            "  return await runs.run('k', { 'agent': 'a', \"task\": \"t\" })  ",
        ))
        .unwrap();
        assert_eq!(preview.agent.as_deref(), Some("a"));
        assert_eq!(preview.task.as_deref(), Some("t"));
    }

    #[test]
    fn double_quoted_literals_decode_via_json() {
        let preview =
            preview_simple_workflow_run(Some(r#"return runs.run("k", { task: "a\nb" });"#))
                .unwrap();
        assert_eq!(preview.task.as_deref(), Some("a\nb"));
    }

    #[test]
    fn single_quoted_with_backslash_is_none() {
        let preview =
            preview_simple_workflow_run(Some(r"return runs.run('k', { task: 'a\nb' });")).unwrap();
        assert_eq!(preview.task, None);
    }

    #[test]
    fn non_simple_scripts_are_none() {
        assert!(preview_simple_workflow_run(None).is_none());
        assert!(preview_simple_workflow_run(Some("const a = 1; return a;")).is_none());
        assert!(
            preview_simple_workflow_run(Some(
                "await runs.run('k', { agent: 'a' }); return runs.run('k2', { agent: 'b' });"
            ))
            .is_none()
        );
    }

    #[test]
    fn backtick_literals_reject_dollar_and_backslash() {
        let preview =
            preview_simple_workflow_run(Some("return runs.run(`k`, { task: `plain` });")).unwrap();
        assert_eq!(preview.task.as_deref(), Some("plain"));
        assert!(
            preview_simple_workflow_run(Some("return runs.run(`k${i}`, { task: `plain` });"))
                .is_none(),
            "an interpolated KEY literal fails the anchored pattern entirely: `[^`$\\\\]*` \
             cannot span the `$`, and neither quoted alternative matches a backtick literal"
        );
        // An interpolated PROPERTY is a different outcome, and the one upstream actually
        // produces: the anchored pattern still matches (the key is clean), so the preview EXISTS
        // and simply omits the property it could not read statically (`readProperty` ⇒
        // `undefined`, spread away at `scripted-workflow.ts:1384-1385`). Asserting `is_none()`
        // here would demand a divergence from upstream.
        let interpolated_task =
            preview_simple_workflow_run(Some("return runs.run(`k`, { task: `has ${x}` });"))
                .expect("a clean key keeps the anchored form matching");
        assert_eq!(interpolated_task.task, None, "an interpolated task is unreadable");
        assert_eq!(interpolated_task.agent, None);
    }
}
