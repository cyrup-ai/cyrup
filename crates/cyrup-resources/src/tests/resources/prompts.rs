//! Prompt template `/name` expansion, shell-style placeholders, frontmatter/CRLF handling (A-09-2).

use super::fixtures::{cfg, run_discover, write};
use crate::{ResourceScope, Skill, expand_prompt_template, parse_command_args, substitute_args};

// ===========================================================================
// A-09-2 — prompt template /name expansion, placeholders, disable
// ===========================================================================

#[tokio::test]
async fn a09_2_prompt_expansion_shell_args_and_disable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Pi shell-style positional substitution (prompt-templates.ts:69-101) + frontmatter fields.
    write(
        &root.join("global/prompts/Review.md"),
        "---\ndescription: Review a PR\nargument-hint: <pr> [focus]\n---\nReview $1 focusing on ${2:-everything}: $@",
    );

    let mut c = cfg(root);
    let report = run_discover(&c).await;
    let tmpl = report
        .registry
        .prompts
        .get_name("review")
        .expect("/review template discovered");
    // name case is preserved (prompt-templates.ts:108); registry key is normalized.
    assert_eq!(tmpl.name, "Review");
    assert_eq!(tmpl.description, "Review a PR");
    assert_eq!(tmpl.argument_hint.as_deref(), Some("<pr> [focus]"));

    // `$1`, `$@`, and `${2:-default}` substitution.
    assert_eq!(
        tmpl.expand("42 perf"),
        "Review 42 focusing on perf: 42 perf"
    );
    assert_eq!(tmpl.expand("42"), "Review 42 focusing on everything: 42");

    // `/name args` entry point matches case-sensitively (prompt-templates.ts:268-284).
    let all: Vec<_> = report.registry.prompts.winners().cloned().collect();
    assert_eq!(
        expand_prompt_template("/Review 42 perf", all.iter()),
        "Review 42 focusing on perf: 42 perf"
    );
    // Non-matching `/name` is returned unchanged.
    assert_eq!(
        expand_prompt_template("/unknown x", all.iter()),
        "/unknown x"
    );
    // A non-slash line is never expanded.
    assert_eq!(expand_prompt_template("hello $1", all.iter()), "hello $1");

    // --no-prompt-templates disables discovery (R-09-010).
    c.enable_prompts = false;
    let disabled = run_discover(&c).await;
    assert!(
        !disabled.registry.prompts.contains("review"),
        "--no-prompt-templates disables it"
    );
}

#[test]
fn prompt_substitute_args_and_quote_parsing() {
    // Quote-aware tokenizer (prompt-templates.ts:24-55).
    assert_eq!(
        parse_command_args(r#"one "two three" 'four five'"#),
        vec![
            "one".to_string(),
            "two three".to_string(),
            "four five".to_string()
        ]
    );

    let args = parse_command_args("a b c d");
    // `$ARGUMENTS` == all args; `${@:2}` slices from the 2nd; `${@:2:2}` takes 2 from the 2nd.
    assert_eq!(substitute_args("$ARGUMENTS", &args), "a b c d");
    assert_eq!(substitute_args("${@:2}", &args), "b c d");
    assert_eq!(substitute_args("${@:2:2}", &args), "b c");
    // Missing positional → empty; default kicks in only when missing/empty.
    assert_eq!(substitute_args("[$5]", &args), "[]");
    assert_eq!(substitute_args("${9:-fallback}", &args), "fallback");
    // Unrecognized `${...}` is left literal.
    assert_eq!(substitute_args("${foo}", &args), "${foo}");
}

/// CFG-016 + CFG-017: pi's `:-` alternative is `(\d+|ARGUMENTS|@):-([^}]*)`
/// (prompt-templates.ts:74 @v0.83.0), so the target may be `@` or `ARGUMENTS`, and index `0` is
/// `args[-1]` — `undefined`, therefore falsy, therefore the default (`:78-79`).
///
/// Red at HEAD before the fix: `${0:-fallback}` aborted the whole form on `checked_sub(1)?` and was
/// emitted verbatim as `${0:-fallback}`; `${@:-…}` / `${ARGUMENTS:-…}` failed the all-digits guard
/// and were likewise emitted verbatim.
#[test]
fn prompt_default_forms_accept_zero_and_the_all_args_targets() {
    let args = parse_command_args("a b");
    let none: Vec<String> = Vec::new();

    // CFG-016 — `${0:-…}` always takes the default; there is no positional 0.
    assert_eq!(substitute_args("${0:-fallback}", &none), "fallback");
    assert_eq!(substitute_args("${0:-fallback}", &args), "fallback");

    // CFG-017 — `@` / `ARGUMENTS` resolve to allArgs, and fall back only when it is empty.
    assert_eq!(substitute_args("${@:-fallback}", &args), "a b");
    assert_eq!(substitute_args("${ARGUMENTS:-fallback}", &args), "a b");
    assert_eq!(substitute_args("${@:-fallback}", &none), "fallback");
    assert_eq!(substitute_args("${ARGUMENTS:-fallback}", &none), "fallback");

    // An empty default is legal (`[^}]*`), and an unknown target is still not a placeholder.
    assert_eq!(substitute_args("${9:-}", &args), "");
    assert_eq!(substitute_args("${nope:-x}", &args), "${nope:-x}");

    // The slice family is unaffected (re-pinned: `${@:1:-2}` is not a placeholder).
    assert_eq!(substitute_args("${@:1}", &args), "a b");
    assert_eq!(substitute_args("${@:1:-2}", &args), "${@:1:-2}");
}

#[tokio::test]
async fn a09_2_frontmatter_body_trimmed_and_crlf_normalized() {
    use crate::{PromptTemplate, ResourceOrigin};

    let tmp = tempfile::tempdir().unwrap();

    // CRLF line endings throughout + leading/trailing blank lines around the body. Pi's
    // parseFrontmatter normalizes `\r\n`/`\r` → `\n` over the whole file (frontmatter.ts:8) and
    // returns `body.trim()` (frontmatter.ts:24).
    let p = tmp.path().join("prompts/Review.md");
    write(
        &p,
        "---\r\ndescription: Review a PR\r\n---\r\n\r\n  Review $1 then $@  \r\n\r\n",
    );
    let tmpl = PromptTemplate::load(&p, ResourceScope::Cli, ResourceOrigin::Builtin).unwrap();
    // Body is trimmed (no surrounding blank lines / spaces) and contains no CR.
    assert_eq!(tmpl.body, "Review $1 then $@");
    assert!(!tmpl.body.contains('\r'), "CRLF normalized to LF");
    // Frontmatter still parsed across CRLF.
    assert_eq!(tmpl.description, "Review a PR");
    // Expansion runs on the trimmed body (prompt-templates.ts:279-280).
    assert_eq!(tmpl.expand("42 x"), "Review 42 then 42 x");

    // Multi-line body keeps interior LF but loses only the surrounding whitespace, and interior
    // CRLF becomes LF.
    let p2 = tmp.path().join("prompts/Multi.md");
    write(&p2, "---\ndescription: d\n---\n\nline one\r\nline two\n");
    let t2 = PromptTemplate::load(&p2, ResourceScope::Cli, ResourceOrigin::Builtin).unwrap();
    assert_eq!(t2.body, "line one\nline two");

    // Skill bodies (Pi `stripFrontmatter`, frontmatter.ts:39) are trimmed + normalized too.
    let sp = tmp.path().join("skills/x/SKILL.md");
    write(
        &sp,
        "---\r\nname: x\r\ndescription: does x\r\n---\r\n\r\n# Heading\r\n\r\nBody.\r\n\r\n",
    );
    let skill = Skill::load(&sp, ResourceScope::Cli, ResourceOrigin::Builtin).unwrap();
    let body = skill.read_body().await.unwrap();
    assert_eq!(body, "# Heading\n\nBody.");

    // Loose fence: Pi closes at the first `\n---` substring even when it is not its own line
    // (frontmatter.ts:17,24); the body starts immediately after that `---`.
    let p3 = tmp.path().join("prompts/Loose.md");
    write(&p3, "---\ndescription: d\n---trailing\nrest\n");
    let t3 = PromptTemplate::load(&p3, ResourceScope::Cli, ResourceOrigin::Builtin).unwrap();
    assert_eq!(t3.body, "trailing\nrest");
    assert_eq!(t3.description, "d");

    // No fence → empty frontmatter + whole (normalized) content as body, untrimmed per Pi
    // (frontmatter.ts:14,33: the no-fence branch does not call `.trim()`).
    let p4 = tmp.path().join("prompts/Plain.md");
    write(&p4, "just text\r\nmore\n");
    let t4 = PromptTemplate::load(&p4, ResourceScope::Cli, ResourceOrigin::Builtin).unwrap();
    assert_eq!(t4.body, "just text\nmore\n");
}
