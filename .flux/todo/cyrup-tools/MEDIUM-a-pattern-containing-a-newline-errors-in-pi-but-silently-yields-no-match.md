---
title: A pattern containing a newline errors in pi but silently yields "No matches found" in cyrup
priority: MEDIUM
tool: grep
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# A pattern containing a newline errors in pi but silently yields "No matches found" in cyrup

## What pi does

ripgrep is spawned without `-U/--multiline` (/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/grep.ts:220-226), so a pattern that can match `\n` is rejected at regex-build time; rg exits 2 and pi surfaces the stderr as a thrown error (`if (!killedDueToLimit && code !== 0 && code !== 1) { const errorMsg = stderr.trim() … reject(new Error(errorMsg)) }`, grep.ts:309-312). Verified with rg 14.1.0: `rg --json -- 'a\nimport' .` prints `rg: the literal "\n" is not allowed in a regex … Consider enabling multiline mode with the --multiline flag` and exits 2.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/tools/grep.rs:308-312 builds the matcher with only `case_insensitive` and `fixed_strings` — no `line_terminator(Some(b'\n'))`, so nothing bans `\n` — and the searcher at :120-123 is line-oriented, so such a pattern can never match. Verified by running the tool with `{"pattern":"a\nimport"}` over a file containing `a\nimport c\n`: result is `No matches found`.

## User-visible impact

A multi-line search request gets a confident but false negative instead of pi's actionable error telling the caller that newlines require multiline mode — the model is likely to conclude the code does not exist.

## Parity action

Set `line_terminator(Some(b'\n'))` on `RegexMatcherBuilder` (ripgrep's non-multiline default) so a `\n`-matching pattern fails to build, and surface that build error with a message equivalent to ripgrep's.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Could not refute. There is exactly one regex construction site in cyrup-tools (/home/user/cyrup/crates/cyrup-tools/src/tools/grep.rs:308-312) and it sets only case_insensitive and fixed_strings; RegexMatcherBuilder's line_terminator defaults to None, which is exactly the setting that disables ripgrep's "the literal \n is not allowed in a regex" guard. Searched crates/cyrup-tools/src and crates/cyrup-core/src for line_terminator, multi_line, multiline, dot_matches_new_line, crlf, "not allowed in a regex" — no matcher-configuration hits anywhere. The upstream preflight validator (crates/cyrup-provider/src/validate.rs:56) is type coercion only and treats pattern as a bare type:string, so nothing rejects control characters there either. No alternate search tool or rg wrapper implements the guard under another name. Verified empirically on both sides: rg 14.1.0 errors and exits 2 for both the real-newline pattern and the two-char regex-escape form (and also with --fixed-strings), while a throwaway example driving the real GrepTool over a file containing "a\nimport c\n" returned "No matches found" for both spellings. Severity medium rather than high: it is a genuine silent false negative reachable by an ordinary model emission, but nothing is mutated or corrupted, the answer is technically true for a line-oriented search, and the failure mode is a recoverable retry.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
