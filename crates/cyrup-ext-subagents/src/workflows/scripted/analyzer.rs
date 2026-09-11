//! The structural analyzer behind `action:"validate"` and `run`'s pre-flight (SCOPE_3f §3.6).
//!
//! # Two owners, split by authority
//!
//! **V8 owns syntax**, because V8 is the executor and is therefore authoritative about what parses.
//! **This module owns structure**, and it is the *only* structural parser: one analyzer, used by
//! both `validate` and `run`, where upstream runs acorn twice (in-worker `scripted-workflow.ts:802`
//! and host-side `:1573`) and the two can drift.
//!
//! Ordering matters and is enforced by the callers: V8 compiles first, so this analyzer only ever
//! sees source already known to parse, and the two cannot disagree about syntax.
//!
//! # Why this is Rust and not a JS pass in the prelude
//!
//! An earlier design proposed "a small JS pass over the parse produced by the same isolate". **V8
//! exposes no AST to JavaScript**, so that is not implementable — and it could not support the
//! globals check below in any case, which needs real scope resolution to tell a reference to a
//! missing global apart from a local binding that shadows one.
//!
//! # The two checks
//!
//! 1. **Nested async** (`scripted-workflow.ts:800-831`) — reject any `async` function, arrow, or
//!    method that is not the top-level wrapper. Portability: upstream's promise bookkeeping differs
//!    across Node and Bun, so the surface is narrowed to top-level `await` plus explicit chains.
//!
//! 2. **Enumerated globals** — SCOPE_3f §0.3's capability contract is a CLOSED set, so every free
//!    identifier is resolved against it and the remainder is rejected. Upstream cannot do this: its
//!    validator (`:1568-1626`) checks syntax, nested-async, `runs.run` key grammar, static `baseRef`
//!    and JSON-ness of params, and performs no identifier resolution at all — so a script that
//!    reaches for `setTimeout` passes `validate` with `ok: true` and then dies as a runtime
//!    `ReferenceError`, mid-script, after children have already launched and been paid for.
//!
//!    That is the failure mode this whole port is organised against (§0.1.2): the agent wrote
//!    something reasonable, the runtime said no, and the contract never let it predict that.
//!    Catching it here means the propose→validate→repair loop fires **before a child is spent**.

use std::collections::BTreeSet;

use deno_ast::SourceRanged;
use deno_ast::view::NodeTrait;
use deno_ast::{MediaType, ModuleSpecifier, ParseParams};

use super::types::WorkflowScriptValidationError;

/// pi `NESTED_ASYNC_WORKFLOW_ERROR` (`scripted-workflow.ts:797`), byte-verbatim — 700 characters
/// carrying a worked rewrite, because an error here is a re-prompt the model acts on, not a
/// diagnostic a human reads (SCOPE_3f §0.1.3).
pub const NESTED_ASYNC_WORKFLOW_ERROR: &str = "workflowScript validation failed before child launch; no children launched. workflowScript does not support nested async functions. Use top-level await, plain helper functions that return runs.run(...), or explicit Promise chains so workflows stay portable across Node and Bun. Parallel plus sequential rewrite: const a = runs.run(\"a\", { agent: \"worker\", task: \"A\" }); const writer = await runs.run(\"writer\", { agent: \"worker\", task: \"Write\" }); const review = await runs.run(\"review\", { agent: \"reviewer\", task: writer.output }); const [aResult] = await Promise.all([a]); return { a: aResult.output, issue: { writerRunId: writer.runId, reviewRunId: review.runId } };";

/// The shorter in-validator wording upstream uses when reporting the same defect as a *finding*
/// rather than as a thrown error (`scripted-workflow.ts:1097`).
pub const NESTED_ASYNC_VALIDATION_ERROR: &str = "workflowScript does not support nested async functions. Use top-level await, plain helper functions that return runs.run(...), or explicit Promise chains.";

/// The workflow sandbox's capability surface — SCOPE_3f §0.3, which is upstream's
/// `vm.createContext({ runs, Promise, emit, console })` measured rather than assumed.
///
/// `state` is admitted only when the run has a mission; see [`AnalyzerOptions::state_enabled`].
const WORKFLOW_GLOBALS: &[&str] = &["runs", "emit", "console"];

/// Every ECMAScript built-in a bare V8 context exposes. This is the whole of "standard JavaScript"
/// as the contract means it — deliberately NOT including `setTimeout`, `fetch`, `TextEncoder`,
/// `URL`, `crypto`, `structuredClone`, `queueMicrotask`, `require`, `Buffer` or `process`, none of
/// which exist in the sandbox on any engine (§0.3).
const ECMASCRIPT_GLOBALS: &[&str] = &[
    // Value properties of the global object.
    "globalThis",
    "Infinity",
    "NaN",
    "undefined",
    // Function properties.
    "eval",
    "isFinite",
    "isNaN",
    "parseFloat",
    "parseInt",
    "decodeURI",
    "decodeURIComponent",
    "encodeURI",
    "encodeURIComponent",
    // Fundamental objects.
    "Object",
    "Function",
    "Boolean",
    "Symbol",
    // Error objects.
    "Error",
    "AggregateError",
    "EvalError",
    "RangeError",
    "ReferenceError",
    "SyntaxError",
    "TypeError",
    "URIError",
    // Numbers and dates.
    "Number",
    "BigInt",
    "Math",
    "Date",
    // Text processing.
    "String",
    "RegExp",
    // Indexed collections.
    "Array",
    "Int8Array",
    "Uint8Array",
    "Uint8ClampedArray",
    "Int16Array",
    "Uint16Array",
    "Int32Array",
    "Uint32Array",
    "Float16Array",
    "Float32Array",
    "Float64Array",
    "BigInt64Array",
    "BigUint64Array",
    // Keyed collections.
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    // Structured data.
    "ArrayBuffer",
    "SharedArrayBuffer",
    "DataView",
    "Atomics",
    "JSON",
    // Managing memory.
    "WeakRef",
    "FinalizationRegistry",
    // Control abstraction.
    "Promise",
    "GeneratorFunction",
    "AsyncGeneratorFunction",
    "Generator",
    "AsyncGenerator",
    "AsyncFunction",
    // Reflection.
    "Reflect",
    "Proxy",
    // Internationalization (ECMA-402; V8 ships it).
    "Intl",
];

/// A global the sandbox does not have, paired with the re-prompt that teaches the model *why*.
///
/// A bare "is not defined" reports an absence. These report the design, because the exclusions are
/// deliberate and the model can only avoid them next time if it learns the rule (§0.1.3).
fn unavailable_global_message(name: &str, state_enabled: bool) -> String {
    let reason = match name {
        "setTimeout" | "setInterval" | "setImmediate" | "clearTimeout" | "clearInterval"
        | "queueMicrotask" => {
            " Workflows have no timer: every wait is a wait on real work, so that it appears in the trace and can be cancelled. Await a runs.* call instead."
        }
        "fetch" | "XMLHttpRequest" | "WebSocket" | "Request" | "Response" | "Headers" => {
            " Reach the network through an extension-owned workflow resource that grants runs.host."
        }
        "require" | "module" | "exports" | "process" | "Buffer" | "__dirname" | "__filename" => {
            " workflowScript is not a Node module; it is a plain statement body evaluated in a sandbox with no module system."
        }
        "Deno" => " The host op bridge is not part of the workflow surface.",
        "state" if !state_enabled => {
            " state.get/state.set require a mission; this run was started with mission:false."
        }
        _ => "",
    };
    let mut globals = String::from("runs, emit, console");
    if state_enabled {
        globals.push_str(", state");
    }
    format!(
        "workflowScript referenced an unavailable global '{name}'. Available globals are {globals}, and standard ECMAScript built-ins only.{reason}"
    )
}

/// What the analyzer is allowed to admit for this particular run.
#[derive(Clone, Copy, Debug, Default)]
pub struct AnalyzerOptions {
    /// Whether the run has a mission, which is what makes `state` a legal global
    /// (`scripted-workflow.ts:915`; the tool schema says *"mission state when enabled"*).
    pub state_enabled: bool,
}

/// The analyzer's verdict: every structural finding, in source order.
#[derive(Clone, Debug, Default)]
pub struct AnalysisReport {
    /// Findings, each carrying `line`/`column` like every other validator result.
    pub errors: Vec<WorkflowScriptValidationError>,
}

impl AnalysisReport {
    /// Whether the script is structurally acceptable.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// The wrapper the script is evaluated inside, both here and at run time
/// (`scripted-workflow.ts:916`). Keeping the two identical is what makes a reported line number
/// mean the same thing in a validation finding and in a runtime stack.
const WRAPPER_PREFIX: &str = "(async () => {\n";
const WRAPPER_SUFFIX: &str = "\n})()";

/// Analyze a workflowScript body.
///
/// # Errors
///
/// Returns `Err` only when the source does not parse. Callers compile with V8 first, so in the
/// normal path this cannot fire; it is still surfaced rather than swallowed because a disagreement
/// between V8 and the analyzer is a real defect worth seeing.
pub fn analyze_workflow_script(
    script: &str,
    options: AnalyzerOptions,
) -> Result<AnalysisReport, WorkflowScriptValidationError> {
    let wrapped = format!("{WRAPPER_PREFIX}{script}{WRAPPER_SUFFIX}");
    let specifier = ModuleSpecifier::parse("file:///workflow-script.js").map_err(|error| {
        WorkflowScriptValidationError {
            message: format!("workflowScript analyzer could not build a specifier: {error}"),
            line: None,
            column: None,
        }
    })?;
    let parsed = deno_ast::parse_program(ParseParams {
        specifier,
        text: wrapped.clone().into(),
        media_type: MediaType::JavaScript,
        // `with_view` requires tokens whenever comments are present, and a workflowScript is
        // ordinary source that will contain them.
        capture_tokens: true,
        scope_analysis: true,
        maybe_syntax: None,
    })
    .map_err(|error| parse_error(&error))?;

    let mut errors = Vec::new();
    parsed.with_view(|program| {
        let scope = deno_ast::Scope::analyze(program);
        let mut visitor = WorkflowVisitor {
            options,
            scope: &scope,
            errors: &mut errors,
            reported: BTreeSet::new(),
        };
        visitor.visit(program.as_node(), true);
    });

    errors.sort_by_key(|error| (error.line.unwrap_or(0), error.column.unwrap_or(0)));
    Ok(AnalysisReport { errors })
}

struct WorkflowVisitor<'a> {
    options: AnalyzerOptions,
    scope: &'a deno_ast::Scope,
    errors: &'a mut Vec<WorkflowScriptValidationError>,
    /// One finding per distinct global name — a script that calls `setTimeout` five times gets one
    /// re-prompt, not five copies of it.
    reported: BTreeSet<String>,
}

impl WorkflowVisitor<'_> {
    /// `at_wrapper` is true only while descending toward the `(async () => { … })()` wrapper this
    /// analyzer (and the runtime, identically) puts around the body. The FIRST async arrow reached
    /// that way is the wrapper and is exempt; every other async function anywhere is a finding.
    fn visit(&mut self, node: deno_ast::view::Node<'_>, at_wrapper: bool) {
        // NOT `at_wrapper && self.check_nested_async(node)` — `&&` short-circuits, which would skip
        // the check for every node inside the wrapper body: i.e. for every nested async function
        // there is. The exemption is an ARGUMENT to the check, never a guard on calling it.
        let is_wrapper = self.check_nested_async(node, at_wrapper);
        self.check_global_reference(node);
        // Only the spine down to the wrapper keeps the exemption; everything inside its body is
        // ordinary script and is checked normally.
        let descend_at_wrapper = at_wrapper && !is_wrapper;
        for child in node.children() {
            self.visit(child, descend_at_wrapper);
        }
    }

    /// Returns `true` when this node WAS the exempt wrapper arrow.
    fn check_nested_async(&mut self, node: deno_ast::view::Node<'_>, at_wrapper: bool) -> bool {
        use deno_ast::view::Node;
        let is_async = match node {
            Node::FnDecl(decl) => decl.function.inner.is_async,
            Node::FnExpr(expr) => expr.function.inner.is_async,
            Node::ArrowExpr(arrow) => arrow.inner.is_async,
            Node::ClassMethod(method) => method.function.inner.is_async,
            Node::PrivateMethod(method) => method.function.inner.is_async,
            Node::MethodProp(prop) => prop.function.inner.is_async,
            _ => return false,
        };
        if !is_async {
            return false;
        }
        if at_wrapper && matches!(node, Node::ArrowExpr(_)) {
            // The wrapper: exempt, and it ends the exemption for everything below it.
            return true;
        }
        let (line, column) = position(node);
        self.errors.push(WorkflowScriptValidationError {
            message: NESTED_ASYNC_VALIDATION_ERROR.to_string(),
            line,
            column,
        });
        false
    }

    fn check_global_reference(&mut self, node: deno_ast::view::Node<'_>) {
        use deno_ast::view::Node;
        let Node::Ident(ident) = node else {
            return;
        };
        // Only *references* matter. A binding site (`const fetch = …`, a parameter), an object key,
        // or a member property (`a.fetch`) is not a reference to a global.
        if !is_reference_position(ident) {
            return;
        }
        // Scope-aware: a local binding that shadows a global resolves locally and is fine. This is
        // the whole reason the analyzer needs real scope analysis rather than a flat walk.
        if self.scope.var_by_ident(ident).is_some() {
            return;
        }
        let name = ident.inner.sym.to_string();
        if self.is_allowed(&name) {
            return;
        }
        if !self.reported.insert(name.clone()) {
            return;
        }
        let (line, column) = position(node);
        self.errors.push(WorkflowScriptValidationError {
            message: unavailable_global_message(&name, self.options.state_enabled),
            line,
            column,
        });
    }

    fn is_allowed(&self, name: &str) -> bool {
        if WORKFLOW_GLOBALS.contains(&name) {
            return true;
        }
        if name == "state" {
            return self.options.state_enabled;
        }
        // `arguments` is bound by the wrapper's function context wherever it is legal, and V8
        // rejects it where it is not — so it is never this analyzer's finding to report.
        if name == "arguments" {
            return true;
        }
        ECMASCRIPT_GLOBALS.contains(&name)
    }
}

/// Report positions in the AGENT's coordinate space, not the wrapper's.
///
/// `start_line()`/`start_column()` are 0-indexed (dprint-swc-ext `common/mod.rs:15`). The wrapper
/// prepends exactly one line, so a 0-indexed wrapped line N is 1-based script line N — the +1 and
/// the −1 cancel. Columns just need the 0→1 shift. Upstream performs the same adjustment
/// (`scripted-workflow.ts:1579`).
fn position(node: deno_ast::view::Node<'_>) -> (Option<u32>, Option<u32>) {
    let line = u32::try_from(node.start_line()).unwrap_or(1).max(1);
    let column = u32::try_from(node.start_column())
        .unwrap_or(0)
        .saturating_add(1);
    (Some(line), Some(column))
}

/// Whether this identifier is a *reference* to a binding rather than a binding site or a name that
/// merely looks like one (an object key, a non-computed member property, a label).
fn is_reference_position(ident: &deno_ast::view::Ident<'_>) -> bool {
    use deno_ast::view::{MemberProp, Node};
    match ident.parent() {
        // `a.fetch` — `fetch` is a property, not a global.
        // A non-computed `a.fetch` names a property. Compare by source range: the view's member
        // property is an `IdentName`, a different node type from the `Ident` we were handed.
        Node::MemberExpr(member) => !matches!(
            member.prop,
            MemberProp::Ident(prop) if prop.range() == ident.range()
        ),
        // `{ fetch: 1 }` — a key, not a reference. Shorthand `{ fetch }` is a Shorthand prop and
        // does not land here, so it stays a reference, correctly.
        Node::KeyValueProp(_) | Node::KeyValuePatProp(_) => false,
        // Binding sites and non-reference name positions.
        Node::BindingIdent(_)
        | Node::FnDecl(_)
        | Node::ClassDecl(_)
        | Node::Param(_)
        | Node::ImportNamedSpecifier(_)
        | Node::ImportDefaultSpecifier(_)
        | Node::ImportStarAsSpecifier(_)
        | Node::ExportNamedSpecifier(_)
        | Node::LabeledStmt(_)
        | Node::BreakStmt(_)
        | Node::ContinueStmt(_) => false,
        _ => true,
    }
}

fn parse_error(error: &deno_ast::ParseDiagnostic) -> WorkflowScriptValidationError {
    // Strip the acorn-style trailing `(line:col)` upstream strips (`:1578`), so the wording reads
    // the same on both runtimes, and carry the position in the structured fields instead.
    let raw = error.to_string();
    let trimmed = raw
        .trim_end_matches(|c: char| c == ')' || c.is_ascii_digit() || c == ':' || c == '(')
        .trim()
        .to_string();
    WorkflowScriptValidationError {
        message: if trimmed.is_empty() { raw } else { trimmed },
        line: None,
        column: None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn analyze(script: &str) -> AnalysisReport {
        analyze_workflow_script(
            script,
            AnalyzerOptions {
                state_enabled: false,
            },
        )
        .unwrap()
    }

    fn analyze_with_state(script: &str) -> AnalysisReport {
        analyze_workflow_script(
            script,
            AnalyzerOptions {
                state_enabled: true,
            },
        )
        .unwrap()
    }

    #[test]
    fn the_documented_surface_is_accepted() {
        let report = analyze(
            r#"
const [a, b] = await runs.all([
  { key: "a", agent: "reviewer", task: "Review the diff" },
  { key: "b", agent: "oracle", task: "Check the design" },
]);
if (!a.ok || /blocker/i.test(a.output)) {
  const fix = await runs.run("fix", { agent: "worker", task: `Fix: ${a.output}` });
  emit({ fixed: fix.runId });
  return { fixed: true };
}
console.log("shipping", JSON.stringify({ n: [1, 2, 3].map((x) => x * 2) }));
return { shipped: true, at: Date.now(), refs: runs.refs([a, b]) };
"#,
        );
        assert!(report.ok(), "unexpected findings: {:?}", report.errors);
    }

    #[test]
    fn nested_async_is_rejected_but_the_wrapper_is_not() {
        assert!(analyze("return await runs.run(\"a\", { agent: \"w\", task: \"t\" });").ok());

        let report = analyze("const f = async () => 1; return await f();");
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].message, NESTED_ASYNC_VALIDATION_ERROR);

        assert_eq!(analyze("async function g() {} return 1;").errors.len(), 1);
        assert_eq!(
            analyze("const o = { async m() {} }; return 1;")
                .errors
                .len(),
            1
        );
        // A NON-async helper is the documented alternative and must stay legal.
        assert!(analyze("function g() { return runs.run(\"a\", { agent: \"w\", task: \"t\" }); } return await g();").ok());
    }

    #[test]
    fn unavailable_globals_are_caught_before_a_child_is_spent() {
        let report = analyze("await new Promise((r) => setTimeout(r, 1000)); return 1;");
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        let message = &report.errors[0].message;
        assert!(message.contains("'setTimeout'"), "{message}");
        assert!(
            message.contains("Workflows have no timer"),
            "the message must teach the rule, not just report an absence: {message}"
        );
        assert!(
            message.contains("Await a runs.* call instead."),
            "{message}"
        );

        for (script, needle) in [
            ("const r = await fetch(\"http://x\"); return r;", "'fetch'"),
            ("const fs = require(\"fs\"); return 1;", "'require'"),
            ("return Buffer.from(\"x\").length;", "'Buffer'"),
            ("return process.env.HOME;", "'process'"),
            ("return structuredClone({});", "'structuredClone'"),
            (
                "return new TextEncoder().encode(\"x\").length;",
                "'TextEncoder'",
            ),
            ("return Deno.core.ops;", "'Deno'"),
        ] {
            let report = analyze(script);
            assert!(
                report
                    .errors
                    .iter()
                    .any(|error| error.message.contains(needle)),
                "{needle} not reported for `{script}`: {:?}",
                report.errors
            );
        }
    }

    #[test]
    fn a_local_binding_that_shadows_a_global_is_not_reported() {
        // The whole reason this needs scope analysis rather than a scope-blind walk.
        assert!(
            analyze("const fetch = (u) => runs.run(\"f\", { agent: \"w\", task: u }); return await fetch(\"x\");").ok(),
            "a local `fetch` must not be reported as a missing global"
        );
        assert!(analyze("function setTimeout(a) { return a; } return setTimeout(1);").ok());
        assert!(analyze("const [Buffer] = [1]; return Buffer;").ok());
    }

    #[test]
    fn property_and_key_positions_are_not_global_references() {
        assert!(analyze("const o = { fetch: 1 }; return o.fetch;").ok());
        assert!(analyze("return runs.status;").ok());
        assert!(analyze("const o = {}; o.setTimeout = 1; return o.setTimeout;").ok());
    }

    #[test]
    fn state_is_admitted_only_with_a_mission() {
        let script = "await state.set(\"k\", 1); return await state.get(\"k\");";
        assert!(analyze_with_state(script).ok());

        let report = analyze(script);
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].message.contains("'state'"));
        assert!(
            report.errors[0].message.contains("require a mission"),
            "{:?}",
            report.errors[0]
        );
    }

    #[test]
    fn one_finding_per_global_and_positions_are_in_the_agents_coordinates() {
        let report = analyze("const a = setTimeout;\nconst b = setTimeout;\nreturn 1;");
        assert_eq!(report.errors.len(), 1, "duplicates must collapse");
        assert_eq!(
            report.errors[0].line,
            Some(1),
            "line 1 of the SCRIPT, not of the wrapper"
        );
    }
}
