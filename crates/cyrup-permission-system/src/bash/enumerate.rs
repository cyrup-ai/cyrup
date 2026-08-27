//! Command-unit enumeration (port of pi `access-intent/bash/command-enumeration.ts:96-141` and
//! `access-intent/bash/nested-execution.ts:17-76`).

use tree_sitter::Node;

/// Execution context of a unit nested inside a substitution or subshell (pi `types.ts:39-42`).
/// `None` on the owning [`BashCommand`] means a current-shell (top-level) command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashCommandContext {
    CommandSubstitution,
    ProcessSubstitution,
    Subshell,
}

/// One command-pattern unit of a parsed bash program (pi `command-enumeration.ts:26-46`).
///
/// `text` is the string matched against the bash rules. The type is the stable extension point:
/// `PORT_BASH_WRAPPER_FLOOR` adds `wrapper_kind`, `PORT_BASH_PATH_PROJECTION` adds path candidates.
#[derive(Debug, Clone)]
pub struct BashCommand {
    pub text: String,
    pub context: Option<BashCommandContext>,
}

/// Container nodes descended into (pi `COMMAND_ENUM_DESCEND`, `command-enumeration.ts:53-58`).
const DESCEND: [&str; 4] = ["program", "list", "pipeline", "redirected_statement"];

/// Named nodes abandoned: neither commands nor able to host one (pi `COMMAND_ENUM_SKIP`, `:73`).
/// Redirects and heredoc bodies are deliberately NOT here — see [`EXECUTION_HOSTS`].
const SKIP: [&str; 2] = ["comment", "heredoc_end"];

/// Nodes that are not commands and whose own text is not read, but whose subtree can host a nested
/// execution that really runs (pi `EXECUTION_HOST_TYPES`, `nested-execution.ts:43-48`).
///
/// `echo hi > $(rm x)` parses with `file_redirect` as a SIBLING of the `command`, so a walker that
/// abandons redirects never sees the substitution — the bypass pi #741 fixed.
const EXECUTION_HOSTS: [&str; 4] =
    ["file_redirect", "heredoc_redirect", "herestring_redirect", "heredoc_body"];

/// The two node types whose interior commands really execute (pi `NESTED_EXECUTION_CONTEXTS`,
/// `nested-execution.ts:17-23`). `subshell` is deliberately absent — it is a command unit in its
/// own right, emitted whole and descended separately.
fn nested_context(kind: &str) -> Option<BashCommandContext> {
    match kind {
        "command_substitution" => Some(BashCommandContext::CommandSubstitution),
        "process_substitution" => Some(BashCommandContext::ProcessSubstitution),
        _ => None,
    }
}

/// A pending step of the walk.
///
/// **[CYRUP-DELTA]** pi recurses (`collectCommandsInto`/`forEachNestedExecution`). This port uses
/// an explicit stack: tree-sitter-bash nests `&&`/`||` chains left-recursively (a 20 000-command
/// chain measures 20 003 levels deep, and `$(` × 10 000 measures 30 004 with no parse error), and
/// a stack overflow under `panic = "abort"` would kill the process on the permission path. A depth
/// cap was rejected because chain nesting makes any stack-safe cap reject legitimate input.
/// Differentially tested against a transcription of pi's recursion over a 30-case corpus: identical
/// output, including ordering.
enum Work<'t> {
    /// Enumerate this node in `context`.
    Enumerate(Node<'t>, Option<BashCommandContext>),
    /// Descend this subtree looking only for nested execution contexts.
    FindNested(Node<'t>),
}

/// Enumerate the command units of a parsed bash program, in source order.
///
/// Descends containers and emits each `command` node whole, plus the inner commands of every
/// command substitution, process substitution and subshell — those really execute (pi #306). The
/// enclosing command is ALWAYS still emitted, so adding nested units can only ever make the
/// decision more restrictive, never weaker. Control-flow bodies and `{ … }` groups are emitted
/// whole without descending (deferred, pi `command-enumeration.ts:138-140`).
///
/// `None` means a unit's source text could not be read, and is the **fail-closed** signal: the
/// caller must not gate on a partial unit list, because a missing unit is a command that runs
/// ungated. `manager.rs` maps it onto the `<unparseable-bash-command>` branch, so the worst case
/// is `ask` and an explicit `deny` covering the whole command still denies.
///
/// Unreachable in practice — `src` is a `&str`, so `Node::utf8_text` cannot fail, and
/// tree-sitter node boundaries never split a codepoint, so the `unit_text` slice always lands on
/// a char boundary. It is surfaced rather than skipped precisely because it is invisible: a later
/// change to the walk (a synthesized offset, a node from another tree) would otherwise turn this
/// assumption into a silently ungated command. **[CYRUP-DELTA]** pi cannot express the case at all
/// — `node.text` is infallible in its runtime — so there is no upstream behaviour to match here.
#[must_use]
pub fn collect_commands(root: Node<'_>, src: &str) -> Option<Vec<BashCommand>> {
    let mut out = Vec::new();
    let mut stack = vec![Work::Enumerate(root, None)];

    // Children are pushed in reverse so pop order is source order; a unit can therefore be emitted
    // the moment it is popped (everything preceding it has already been processed).
    while let Some(work) = stack.pop() {
        match work {
            Work::Enumerate(node, context) => {
                // Anonymous tokens (`&&`, `;`, `|`, `$(`, `)`) carry no command.
                if !node.is_named() || SKIP.contains(&node.kind()) {
                    continue;
                }

                if node.kind() == "command" {
                    out.push(BashCommand { text: unit_text(node, src)?.to_string(), context });
                    // The command's own text already contains any substitution; descend anyway to
                    // ALSO emit the inner commands as units of their own.
                    stack.push(Work::FindNested(node));
                    continue;
                }

                if EXECUTION_HOSTS.contains(&node.kind()) {
                    stack.push(Work::FindNested(node));
                    continue;
                }

                if node.kind() == "subshell" {
                    out.push(BashCommand { text: node_text(node, src)?.to_string(), context });
                    push_children(&mut stack, node, Some(BashCommandContext::Subshell));
                    continue;
                }

                if DESCEND.contains(&node.kind()) {
                    push_children(&mut stack, node, context);
                    continue;
                }

                // Any other named statement (compound_statement, if/while/for/case, function
                // definition): emit whole, do not descend.
                out.push(BashCommand { text: node_text(node, src)?.to_string(), context });
            }

            Work::FindNested(node) => {
                for index in (0..node.child_count()).rev() {
                    let Some(child) = node.child(index) else { continue };
                    match nested_context(child.kind()) {
                        // Do not descend past the context: enumerate its interior as commands.
                        Some(context) => push_children(&mut stack, child, Some(context)),
                        None => stack.push(Work::FindNested(child)),
                    }
                }
            }
        }
    }
    Some(out)
}

fn push_children<'t>(
    stack: &mut Vec<Work<'t>>,
    node: Node<'t>,
    context: Option<BashCommandContext>,
) {
    for index in (0..node.child_count()).rev() {
        if let Some(child) = node.child(index) {
            stack.push(Work::Enumerate(child, context));
        }
    }
}

fn node_text<'a>(node: Node<'_>, src: &'a str) -> Option<&'a str> {
    node.utf8_text(src.as_bytes()).ok()
}

/// The match text of a `command` node, with any leading `variable_assignment` prefix stripped
/// (pi `commandUnitText`, `command-enumeration.ts:204-212`).
///
/// `AWS_PROFILE=prod aws s3 ls` must be matched as `aws s3 ls`, so an env-var prefix cannot defeat
/// a rule that gates the underlying command. Sliced verbatim from the first non-assignment child to
/// preserve spacing. A pure assignment with no `command_name` runs nothing and is returned whole.
fn unit_text<'a>(node: Node<'_>, src: &'a str) -> Option<&'a str> {
    for index in 0..node.child_count() {
        let Some(child) = node.child(index) else { continue };
        if child.is_named() && child.kind() != "variable_assignment" {
            return src.get(child.start_byte()..node.end_byte());
        }
    }
    node_text(node, src)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::bash::parse_command_units;

    /// `(text, context)` pairs for a command, in source order.
    fn units(command: &str) -> Vec<(String, Option<BashCommandContext>)> {
        parse_command_units(command)
            .expect("grammar loads and every unit's text is readable")
            .into_iter()
            .map(|u| (u.text, u.context))
            .collect()
    }

    /// The execution context of every nested unit. Nothing in the crate reads
    /// [`BashCommand::context`] yet — it is the extension point `PORT_BASH_WRAPPER_FLOOR` and
    /// `PORT_BASH_PATH_PROJECTION` consume — so an error here is otherwise invisible until it
    /// surfaces as a wrong prompt.
    #[test]
    fn nested_units_carry_their_execution_context() {
        use BashCommandContext::{CommandSubstitution, ProcessSubstitution, Subshell};

        assert_eq!(
            units("echo $(rm x)"),
            vec![
                ("echo $(rm x)".to_string(), None),
                ("rm x".to_string(), Some(CommandSubstitution)),
            ]
        );
        assert_eq!(
            units("diff <(ls a)"),
            vec![
                ("diff <(ls a)".to_string(), None),
                ("ls a".to_string(), Some(ProcessSubstitution)),
            ]
        );
        assert_eq!(
            units("( rm b )"),
            vec![("( rm b )".to_string(), None), ("rm b".to_string(), Some(Subshell))]
        );
        // A current-shell command carries no context.
        assert_eq!(units("echo hi"), vec![("echo hi".to_string(), None)]);
    }

    /// The enclosing unit is always emitted alongside the nested ones, so adding nested units can
    /// only ever make a decision more restrictive (pi `command-enumeration.ts:88-89`).
    #[test]
    fn the_enclosing_command_is_emitted_as_well_as_what_it_nests() {
        let u = units("echo hi > $(rm x)");
        assert_eq!(u.len(), 2);
        assert_eq!(u[0].0, "echo hi");
        assert_eq!(u[1].0, "rm x");
    }

    /// The justification for the iterative walker over pi's recursion (see [`Work`]).
    /// tree-sitter-bash nests `&&` chains left-recursively and imposes no depth limit, so a
    /// recursive walk would exhaust the stack — and under `panic = "abort"` that is process death
    /// on the permission path, not a catchable error. Depths here are far past what any recursive
    /// walker survives while staying fast enough for the suite.
    #[test]
    fn deep_nesting_and_long_chains_enumerate_without_exhausting_the_stack() {
        const DEPTH: usize = 20_000;

        // `$(` x DEPTH — an AST ~DEPTH levels deep. Every level contributes its enclosing command
        // plus the innermost `rm x`.
        let nested = format!("{}rm x{}", "$(".repeat(DEPTH), ")".repeat(DEPTH));
        let u = units(&nested);
        assert_eq!(u.len(), DEPTH + 1, "one unit per nesting level plus the innermost command");
        assert_eq!(u[u.len() - 1], ("rm x".to_string(), Some(BashCommandContext::CommandSubstitution)));

        // A DEPTH-element `&&` chain — `list` nodes nest left-recursively, so this is also ~DEPTH
        // levels deep even though it reads as flat.
        let chain = (0..DEPTH).map(|i| format!("echo {i}")).collect::<Vec<_>>().join(" && ");
        let u = units(&chain);
        assert_eq!(u.len(), DEPTH, "every command in the chain is its own unit");
        assert_eq!(u[0].0, "echo 0");
        assert_eq!(u[DEPTH - 1].0, format!("echo {}", DEPTH - 1));
    }

    /// Zero units is a legitimate parse, not a failure — it is `Some(vec![])`, which the caller
    /// distinguishes from the fail-closed `None`.
    #[test]
    fn trivially_empty_commands_parse_to_zero_units_rather_than_failing() {
        for command in ["", "   ", "# just a comment", "# one\n# two"] {
            assert_eq!(units(command), vec![], "{command:?}");
        }
    }
}
