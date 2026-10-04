//! SUBA-150 — the reply-fenced workflow script: `workflow: true` runs the single
//! ` ```js workflow ` fenced block in the SAME assistant reply that issued the `subagent` tool
//! call.
//!
//! A 1:1 port of pi-subagents `src/extension/reply-workflow-script.ts` @v0.74.0 (`0538e14d`,
//! #2588, `feat(workflows)!`), which replaced the tool's `workflowScript` / `workflowScriptPath`
//! parameters with one `workflow` field. Upstream's reason is an authoring tax, not a feature: a
//! script passed as a JSON string has every quote and newline escaped, which is a real source of
//! malformed scripts. A fenced block has none of that.
//!
//! # What lives here and what does not
//!
//! This module is the **pure** half — the fence grammar and the four refusals — exactly as
//! upstream splits it (`workflowBlocks` and `scriptFromReply` are module-private pure functions
//! there too, and `readReplyWorkflowScript` is the thin session-reading wrapper around them). The
//! grammar is the load-bearing part: it decides which block in a reply is the script, and getting
//! it wrong either runs the wrong code or refuses a correct reply.
//!
//! The session-reading half — walking `sessionManager.getBranch()` backwards for the assistant
//! message carrying this `toolCallId` (`reply-workflow-script.ts:14-26`) — is **not** here, and
//! neither is the `workflow` tool parameter itself. [`script_from_reply`] takes the already-joined
//! assistant text and the already-counted `workflow: true` call count, so both of its inputs are
//! data; a caller that has the branch supplies them. See the `SUBA-150` row for the split.
//!
//! # The grammar, precisely
//!
//! * A fence OPENS on a line starting with three or more backticks or three or more tildes
//!   (`OPEN_FENCE`), and the opener is TAGGED when the whole line is ` ```js workflow ` or
//!   ` ```javascript workflow ` with only trailing spaces/tabs after it (`WORKFLOW_FENCE`).
//! * A fence CLOSES on a line that is three or more of the SAME marker character, at least as
//!   long as the opener, with nothing after it but spaces/tabs (`CLOSE_FENCE`). A `~~~~` block
//!   therefore closes only on four or more tildes, and never on backticks.
//! * While a fence is open every line is skipped, so a ` ```js workflow ` written INSIDE another
//!   fence is not a script — that is what makes a reply able to show an example.
//! * Only tagged blocks are collected; untagged ones are consumed and discarded.

/// The tag an opening fence must carry for its body to be a workflow script — pi `WORKFLOW_FENCE`
/// (`reply-workflow-script.ts:4`): `/^```(?:js|javascript) workflow[ \t]*$/`.
///
/// The anchors are both load-bearing. A leading `~~~` can never be tagged (upstream's regexp
/// starts with literal backticks), and anything after `workflow` other than spaces/tabs — a
/// language attribute, a filename, a word — makes the block ordinary.
fn is_workflow_fence(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("```") else {
        return false;
    };
    let rest = if let Some(rest) = rest.strip_prefix("javascript") {
        rest
    } else if let Some(rest) = rest.strip_prefix("js") {
        rest
    } else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(" workflow") else {
        return false;
    };
    rest.chars().all(|ch| ch == ' ' || ch == '\t')
}

/// The run of fence characters a line opens with, if any — pi `OPEN_FENCE`
/// (`reply-workflow-script.ts:5`): `/^(`{3,}|~{3,})/`. Note there is NO trailing anchor: an opener
/// may carry an info string.
fn open_fence(line: &str) -> Option<&str> {
    let marker = line.chars().next().filter(|ch| *ch == '`' || *ch == '~')?;
    let len = line.chars().take_while(|ch| *ch == marker).count();
    if len < 3 {
        return None;
    }
    // Both candidate markers are 1-byte ASCII, so the char count is the byte length.
    line.get(..len)
}

/// The run of fence characters a line is ENTIRELY made of, if any — pi `CLOSE_FENCE`
/// (`reply-workflow-script.ts:6`): `/^(`{3,}|~{3,})[ \t]*$/`. The trailing anchor is what stops a
/// line like ` ```js ` from closing a block.
fn close_fence(line: &str) -> Option<&str> {
    let marker = open_fence(line)?;
    let rest = line.get(marker.len()..)?;
    rest.chars()
        .all(|ch| ch == ' ' || ch == '\t')
        .then_some(marker)
}

/// pi `workflowBlocks` (`reply-workflow-script.ts:41-58`): every ` ```js workflow ` /
/// ` ```javascript workflow ` block's body, skipping the bodies of other fences, plus whether a
/// TAGGED fence was left unclosed at the end of the text.
///
/// `unclosed` is upstream's `fence?.tagged === true`: an unclosed UNTAGGED fence is not an error,
/// it merely swallows the rest of the reply — which is exactly what a Markdown renderer does.
#[must_use]
pub fn workflow_blocks(text: &str) -> (Vec<String>, bool) {
    // pi splits on `\n` and strips a trailing `\r` per line, so a CRLF reply behaves as an LF one.
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let mut blocks: Vec<String> = Vec::new();
    // `(marker, tagged, start)` — the open fence, as upstream's `fence` object.
    let mut fence: Option<(&str, bool, usize)> = None;
    for (index, line) in lines.iter().enumerate() {
        let Some((marker, tagged, start)) = fence else {
            if let Some(open) = open_fence(line) {
                fence = Some((open, is_workflow_fence(line), index + 1));
            }
            continue;
        };
        let Some(close) = close_fence(line) else {
            continue;
        };
        // Same marker CHARACTER and at least as long as the opener (pi `:55`): a `~~~~` block is
        // not closed by `~~~`, and never by backticks.
        if close.as_bytes().first() != marker.as_bytes().first() || close.len() < marker.len() {
            continue;
        }
        if tagged {
            blocks.push(lines.get(start..index).unwrap_or_default().join("\n"));
        }
        fence = None;
    }
    let unclosed = matches!(fence, Some((_, true, _)));
    (blocks, unclosed)
}

/// pi's refusal for a caller that is not a model tool call carrying the block
/// (`readReplyWorkflowScript`'s fall-through, `reply-workflow-script.ts:25`), verbatim.
///
/// It is a `const` rather than a message built at the refusal site because it is the ONE refusal
/// whose text an RPC/CLI caller sees, and it names the alternative that does work.
pub const NOT_A_MODEL_TOOL_CALL_REFUSAL: &str = "workflow: true only works from a model subagent \
     tool call whose assistant message contains the ```js workflow block; other callers must pass \
     a script path such as workflow: \"./script.js\".";

/// pi `scriptFromReply` (`reply-workflow-script.ts:28-39`): the script this reply carries, or the
/// refusal that explains why it carries none.
///
/// `reply_text` is upstream's `content.flatMap(text blocks).join("\n")` — the assistant message's
/// text blocks joined with newlines — and `reply_workflow_calls` is its
/// `content.filter(toolCall && name === "subagent" && arguments.workflow === true).length`. Both
/// are passed in rather than derived here so this function is a pure function of data: the caller
/// owns the branch, this owns the grammar.
///
/// # Errors
///
/// Upstream's three content-shaped refusals, verbatim: more than one `workflow: true` call in one
/// reply, an unclosed tagged block, a block count that is not exactly one, and an empty block.
pub fn script_from_reply(reply_text: &str, reply_workflow_calls: usize) -> Result<String, String> {
    if reply_workflow_calls > 1 {
        return Err(format!(
            "This reply has {reply_workflow_calls} subagent calls with workflow: true; a reply can \
             carry only one. Pass other scripts as workflow file paths."
        ));
    }
    let (blocks, unclosed) = workflow_blocks(reply_text);
    if unclosed {
        return Err("The ```js workflow block in this reply is not closed.".to_string());
    }
    if blocks.len() != 1 {
        return Err(format!(
            "workflow: true requires exactly one ```js workflow fenced block in the same reply as \
             the tool call; found {}.",
            blocks.len()
        ));
    }
    let script = blocks.into_iter().next().unwrap_or_default();
    if script.trim().is_empty() {
        return Err("The ```js workflow block in this reply is empty.".to_string());
    }
    Ok(script)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// SUBA-150 — the happy path: one tagged block in the reply IS the script, and its body is
    /// handed over verbatim (no re-indentation, no trimming of interior blank lines).
    ///
    /// THE USER ACTION: the model writes a workflow inline in its reply and calls
    /// `subagent({ workflow: true })`. This is the whole point of `0538e14d` — a script written as
    /// a JSON string has every quote and newline escaped, and the model gets it wrong.
    #[test]
    fn one_tagged_block_is_the_script_verbatim() {
        let reply =
            "Here is the plan.\n\n```js workflow\nconst a = \"x\";\n\nruns.run(a);\n```\nDone.";
        assert_eq!(
            script_from_reply(reply, 1).expect("one block"),
            "const a = \"x\";\n\nruns.run(a);"
        );
        // `javascript` is the documented second spelling.
        let alt = "```javascript workflow\nruns.run(1);\n```";
        assert_eq!(
            script_from_reply(alt, 1).expect("one block"),
            "runs.run(1);"
        );
        // Trailing spaces/tabs on the opener and the closer are allowed.
        let padded = "```js workflow  \t\nruns.run(2);\n``` \t";
        assert_eq!(
            script_from_reply(padded, 1).expect("one block"),
            "runs.run(2);"
        );
    }

    /// SUBA-150 — an untagged ` ```js ` block is NOT a script, and a tagged block written inside
    /// another fence is not one either.
    ///
    /// THE USER ACTION: the model's reply SHOWS an example workflow inside a quoted fence while
    /// the real script sits in its own block, or it writes ordinary `js` code alongside. Treating
    /// either as the script would run code the model did not mean to run — which is why upstream's
    /// scanner skips the bodies of other fences rather than scanning the whole text for the tag.
    #[test]
    fn only_a_tagged_top_level_block_counts() {
        // A plain ` ```js ` block is invisible to the scanner.
        let plain = "```js\nnot_the_script();\n```\n```js workflow\nruns.run(1);\n```";
        assert_eq!(
            script_from_reply(plain, 1).expect("one block"),
            "runs.run(1);"
        );

        // A tagged fence INSIDE a four-backtick fence is part of that fence's body.
        let nested = "````md\n```js workflow\nexample();\n```\n````\n```js workflow\nreal();\n```";
        assert_eq!(script_from_reply(nested, 1).expect("one block"), "real();");

        // ...and with ONLY the quoted example, there is no script at all.
        let quoted_only = "````md\n```js workflow\nexample();\n```\n````";
        assert_eq!(
            script_from_reply(quoted_only, 1),
            Err(
                "workflow: true requires exactly one ```js workflow fenced block in the same \
                 reply as the tool call; found 0."
                    .to_string()
            )
        );
    }

    /// SUBA-150 — a `~~~~`-fenced block closes only on four or more tildes, never on backticks and
    /// never on a shorter tilde run (pi `:55`: same marker character, length at least the
    /// opener's).
    #[test]
    fn a_longer_tilde_fence_closes_only_on_an_equal_or_longer_tilde_run() {
        // `~~~` does not close `~~~~`, so the inner backtick fence stays inside the tilde block.
        let text = "~~~~\n~~~\n```js workflow\ninner();\n```\n~~~~\n```js workflow\nouter();\n```";
        assert_eq!(script_from_reply(text, 1).expect("one block"), "outer();");

        // A tilde fence is never TAGGED (upstream's `WORKFLOW_FENCE` starts with backticks), so an
        // unclosed one is not an error — it just swallows the rest of the reply.
        let (blocks, unclosed) = workflow_blocks("~~~js workflow\nruns.run(1);");
        assert!(blocks.is_empty(), "a tilde opener cannot be tagged");
        assert!(!unclosed, "an unclosed UNTAGGED fence is not a refusal");
    }

    /// SUBA-150 — all four of upstream's refusals, verbatim, because the model reads them and
    /// corrects itself from them.
    #[test]
    fn each_refusal_is_upstreams_own_sentence() {
        // (1) more than one `workflow: true` call in one reply.
        assert_eq!(
            script_from_reply("```js workflow\nruns.run(1);\n```", 2),
            Err(
                "This reply has 2 subagent calls with workflow: true; a reply can carry only one. \
                 Pass other scripts as workflow file paths."
                    .to_string()
            )
        );
        // (2) an unclosed TAGGED block.
        assert_eq!(
            script_from_reply("```js workflow\nruns.run(1);", 1),
            Err("The ```js workflow block in this reply is not closed.".to_string())
        );
        // (3) a block count that is not exactly one — in both directions.
        assert_eq!(
            script_from_reply("no blocks here", 1),
            Err(
                "workflow: true requires exactly one ```js workflow fenced block in the same \
                 reply as the tool call; found 0."
                    .to_string()
            )
        );
        assert_eq!(
            script_from_reply("```js workflow\na();\n```\n```js workflow\nb();\n```", 1),
            Err(
                "workflow: true requires exactly one ```js workflow fenced block in the same \
                 reply as the tool call; found 2."
                    .to_string()
            )
        );
        // (4) an empty block — present, closed, and carrying nothing.
        assert_eq!(
            script_from_reply("```js workflow\n\n   \n```", 1),
            Err("The ```js workflow block in this reply is empty.".to_string())
        );
        // And the non-model-caller refusal names the alternative that does work.
        assert!(
            NOT_A_MODEL_TOOL_CALL_REFUSAL.contains("workflow: \"./script.js\""),
            "{NOT_A_MODEL_TOOL_CALL_REFUSAL}"
        );
    }

    /// SUBA-150 — a CRLF reply behaves exactly as an LF one (pi strips a trailing `\r` per line),
    /// and the stripped `\r` does not survive into the script body.
    #[test]
    fn crlf_line_endings_are_normalized_before_the_grammar_runs() {
        let reply = "```js workflow\r\nruns.run(1);\r\n```\r\n";
        assert_eq!(
            script_from_reply(reply, 1).expect("one block"),
            "runs.run(1);"
        );
    }

    /// SUBA-150 — a tag that is not EXACTLY upstream's is not a tag: the regexp is anchored at
    /// both ends, so an info string with anything else after `workflow` makes the block ordinary.
    #[test]
    fn the_tag_is_anchored_at_both_ends() {
        for opener in [
            "```js workflows",      // a longer word
            "```js workflow extra", // a second token
            "```jsworkflow",        // no space
            "```ts workflow",       // another language
            "```js  workflow",      // two spaces
            " ```js workflow",      // indented: not an opener at all
        ] {
            let text = format!("{opener}\nruns.run(1);\n```");
            let (blocks, _) = workflow_blocks(&text);
            assert!(
                blocks.is_empty(),
                "`{opener}` must not open a workflow block"
            );
        }
    }
}
