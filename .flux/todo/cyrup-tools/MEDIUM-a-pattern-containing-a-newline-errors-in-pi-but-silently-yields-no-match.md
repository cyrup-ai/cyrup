---
title: A pattern containing a newline errors in pi but silently yields "No matches found" in cyrup
priority: MEDIUM
tool: grep
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: done
updated: 2026-08-27
---

# A pattern containing a newline errors in pi but silently yields "No matches found" in cyrup

## Core objective

A `grep` pattern that can match a literal line terminator must be **refused at matcher-build time**,
with the exact message pi's caller sees, instead of building successfully and then returning
`No matches found` from a line-oriented search that could never have matched it.

The whole gap lives in **one builder call**. `RegexMatcherBuilder::line_terminator` defaults to
`None`, and `None` is precisely the setting that switches off grep-regex's "the literal `\n` is not
allowed" guard. ripgrep — which pi spawns — sets `Some(b'\n')` on every non-`--multiline` search.
cyrup never sets it.

---

## What pi does — verified

[pi grep.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts) builds ripgrep's argv and
spawns it:

```ts
// grep.ts:220-226
const args: string[] = ["--json", "--line-number", "--color=never", "--hidden"];
if (ignoreCase) args.push("--ignore-case");
if (literal) args.push("--fixed-strings");
if (glob) args.push("--glob", glob);
args.push("--", pattern, searchPath);

const child = spawn(rgPath, args, { stdio: ["ignore", "pipe", "pipe"] });
```

There is **no** `-U` / `--multiline` anywhere in the file (`grep -n 'multiline' grep.ts` is empty),
so rg runs in its default line-oriented mode. The child's stderr is buffered whole
(`grep.ts:228`, `grep.ts:251-253`), and any exit code other than `0` or `1` becomes a rejection
carrying that stderr verbatim:

```ts
// grep.ts:309-312
if (!killedDueToLimit && code !== 0 && code !== 1) {
    const errorMsg = stderr.trim() || `ripgrep exited with code ${code}`;
    settle(() => reject(new Error(errorMsg)));
    return;
}
```

The rejection is never caught — `execute` returns that `Promise` directly (grep.ts:162), so the
model sees rg's own stderr text as the tool error. The `matchCount === 0` →
`No matches found` branch (grep.ts:314-318) is **below** it and is never reached for a rejected
pattern.

### The exact bytes pi surfaces

Verified against the `rg` on `PATH` (ripgrep 14.1.0), driving the same argv pi builds:

```console
$ rg --json --line-number --color=never --hidden -- $'a\nimport' file.txt
rg: the literal "\n" is not allowed in a regex

Consider enabling multiline mode with the --multiline flag (or -U for short).
When multiline mode is enabled, new line characters can be matched.
$ echo $?
2
```

Three layers make that text, all verified in ripgrep 14.1.0's sources:

1. `HiArgs::matcher_rust` takes the non-multiline branch and sets the line terminator —
   `crates/core/flags/hiargs.rs:482-484`:

   ```rust
   } else {
       builder.line_terminator(Some(b'\n')).dot_matches_new_line(false);
   ```

   (For completeness, the same builder is configured with
   `.multi_line(true).unicode(!self.no_unicode).octal(false).fixed_strings(self.fixed_strings)` at
   `hiargs.rs:461-465`, and `ban_byte(Some(b'\x00'))` at `hiargs.rs:502-504`. Neither is in scope
   here — see *Explicitly out of scope*.)
2. `suggest_multiline` appends the two-line hint, gated on the message text itself —
   `crates/core/flags/hiargs.rs:1437-1448`:

   ```rust
   fn suggest_multiline(msg: String) -> String {
       if msg.contains("the literal") && msg.contains("not allowed") {
           format!(
               "{msg}

   Consider enabling multiline mode with the --multiline flag (or -U for short).
   When multiline mode is enabled, new line characters can be matched.",
           )
       } else {
           msg
       }
   }
   ```
3. `main` prints the top-level error through `eprintln_locked!` (`crates/core/main.rs:62`), and that
   macro writes a fixed `rg: ` prefix first (`crates/core/messages.rs:50`). Exit code `2`
   (`main.rs:63`).

`stderr.trim()` therefore hands the model the whole four-line block, `rg: ` prefix included.

### Which spellings actually trip it

Observed with rg 14.1.0 against a file containing `a\nimport c\n`:

| `pattern` | `literal` | rg exit | rg behaviour |
| --- | --- | --- | --- |
| real newline `a`⏎`import` | `false` | 2 | `the literal "\n" is not allowed in a regex` |
| real newline `a`⏎`import` | `true` (`--fixed-strings`) | 2 | same error |
| two-char escape `a\nimport` | `false` | 2 | same error |
| two-char escape `a\nimport` | `true` (`--fixed-strings`) | 1 | **no error**, no match |
| `a\rb` | `false` | 1 | no error, no match |

The last two rows are load-bearing and are corrections to the original audit (see *Corrections*).

---

## What cyrup-tools does today — verified

[grep.rs:308-312](../../../crates/cyrup-tools/src/tools/grep.rs) — the sole regex construction
site in the workspace:

```rust
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(input.ignore_case.unwrap_or(false))
            .fixed_strings(input.literal.unwrap_or(false))
            .build(&input.pattern)
            .map_err(|e| error::invalid(format!("grep: invalid pattern: {e}")))?;
```

`grep -rn 'RegexMatcherBuilder\|RegexMatcher::new\|new_line_matcher' crates/` returns exactly two
lines, both in [grep.rs](../../../crates/cyrup-tools/src/tools/grep.rs) — the `use` at `:12` and the
builder at `:308`. `grep -rn 'line_terminator' crates/` returns nothing at all.

Because `line_terminator` is never set, the pattern compiles, and the searcher at
[grep.rs:120-123](../../../crates/cyrup-tools/src/tools/grep.rs) is line-oriented
(`SearcherBuilder` defaults to `multi_line: false`), so no match can ever span a line break. The
result is `No matches found` at [grep.rs:436-443](../../../crates/cyrup-tools/src/tools/grep.rs).

### Why `None` is exactly the disabling setting — verified in grep-regex 0.1.14

The workspace pins `grep-regex = "0.1.14"` ([Cargo.toml:152](../../../Cargo.toml)); source read at
`~/.cargo/registry/src/index.crates.io-*/grep-regex-0.1.14/`.

1. **The default is `None`.** `Config::default()` sets `line_terminator: None`
   (`src/config.rs:61`); `multi_line: false` is at `src/config.rs:50`.
2. **The guard is gated on it.** `ConfiguredHIR::new` only strips/refuses when it is `Some`
   (`src/config.rs:222-225`):

   ```rust
   hir = match config.line_terminator {
       None => hir,
       Some(line_term) => strip_from_match(hir, line_term)?,
   };
   ```
3. **The refusal.** `strip_from_match_ascii` walks the HIR; a `\n` inside a class with other
   members is silently removed (so `a\sb` keeps working), but a bare `\n` literal — or a class that
   would be emptied — returns `ErrorKind::NotAllowed` (`src/strip.rs:60`):

   ```rust
   let invalid = || Err(Error::new(ErrorKind::NotAllowed(ch.to_string())));
   ```
4. **The message.** `impl Display for Error` (`src/error.rs:75-77`):

   ```rust
   ErrorKind::NotAllowed(ref lit) => {
       write!(f, "the literal {:?} is not allowed in a regex", lit)
   }
   ```

   `{:?}` on the `String` `"\n"` renders `"\n"` — so the inner text is byte-identical to what
   ripgrep prints, because ripgrep is printing this very `Display`.
5. **`fixed_strings` is handled.** `Config::is_fixed_strings` bails out of the fast literal-alternation
   path when a pattern contains the line terminator (`src/config.rs:113-124`), routing it through
   the parse path — which is where step 3 runs. That is why `--fixed-strings` with a *real* newline
   still errors, while the two-char `\n` escape (which contains no `\n` byte) stays a plain literal
   and does not.
6. **`RegexMatcher::new_line_matcher` is the same thing.** `src/matcher.rs:402` is literally
   `RegexMatcherBuilder::new().line_terminator(Some(b'\n')).build(pattern)`, and its doc says it
   "will return an error if the given pattern contains a literal `\n`. Other uses of `\n` (such as
   in `\s`) are removed transparently."

### The change is searcher-safe

`Searcher::check_config` rejects a matcher whose line terminator differs from the searcher's
(`grep-searcher-0.1.16 src/searcher/mod.rs:805-821`, `ConfigError::MismatchedLineTerminators`).
The searcher's default is `LineTerminator::default()` (`mod.rs:190`), which is
`LineTerminator::byte(b'\n')` (`grep-matcher-0.1.8 src/lib.rs:268-273`). `SearcherBuilder` in
[grep.rs:120-123](../../../crates/cyrup-tools/src/tools/grep.rs) never overrides it, so
`Some(b'\n')` on the matcher is exactly consistent and no config error is possible.

The line-by-line strategy also does not change: `Searcher::multi_line()` is already `false`
(`SearcherBuilder` default), and `is_line_by_line_fast` (`src/searcher/core.rs:673-708`) already
returns `true` today via its `non_matching_bytes` branch for any pattern that cannot match `\n`.
Setting the terminator merely lets grep-regex build a `fast_line_regex`
(`grep-regex src/literal.rs:54-62` declines outright when `line_terminator.is_none()`), which is a
strictly-faster candidate-line prefilter over the same line ranges.

---

## User-visible impact

A model asking for a multi-line search gets a confident false negative — `No matches found` — where
pi hands back an actionable error naming the flag that would make the search legal. The likely
model conclusion is "that code does not exist", and the query is not retried.

---

## Corrections to the original audit

1. **`grep.ts:220-226` covered args *and* spawn.** The argv is `grep.ts:220-224`; the `spawn` is
   `grep.ts:226`.
2. **"errors … and also with `--fixed-strings`" was too broad.** With `--fixed-strings`, only the
   **real newline** errors. The two-char `\n` escape becomes a literal backslash-`n` and rg exits 1
   with no match — verified above, and explained by `Config::is_fixed_strings`
   (`grep-regex src/config.rs:113-124`). The implementation must reproduce that asymmetry, which it
   does for free by keeping `fixed_strings` wired to `input.literal`.
3. **`validate.rs:56`** is `pub fn validate_tool_call`, in
   [crates/cyrup-provider/src/validate.rs](../../../crates/cyrup-provider/src/validate.rs). The
   claim it supports — that the preflight coercer is type-only and rejects no control characters in
   `pattern` — holds.
4. **"surface … a message equivalent to ripgrep's" was under-specified.** pi surfaces rg's *stderr*,
   which carries a `rg: ` prefix and the two-line multiline hint. Reproducing only grep-regex's
   `Display` would still diverge. The prescription below reproduces all three layers.

---

## Required implementation

**One file changes: [crates/cyrup-tools/src/tools/grep.rs](../../../crates/cyrup-tools/src/tools/grep.rs).**
Two hunks.

### Hunk 1 — the error renderer

Insert this free function between the `GrepInput` struct (ends at
[grep.rs:31](../../../crates/cyrup-tools/src/tools/grep.rs)) and `pub struct GrepTool`
([grep.rs:33](../../../crates/cyrup-tools/src/tools/grep.rs)). That anchor is deliberate — it is
nowhere near the regions the cancellation task rewrites.

```rust
/// Render a `grep-regex` build failure as the bytes pi's caller actually receives.
///
/// Pi never builds a matcher itself. It spawns `rg`, buffers the child's stderr whole
/// (grep.ts:228, :251-253) and, for any exit code other than 0 or 1, rejects with `stderr.trim()`
/// (grep.ts:309-312) — a rejection nothing catches, so rg's stderr text IS the model-observed tool
/// error. That text has three layers, and dropping any one of them diverges:
///
/// 1. The inner message is `grep_regex::Error`'s own `Display`. cyrup links the same crate, so it
///    is already byte-identical: `the literal "\n" is not allowed in a regex` for
///    `ErrorKind::NotAllowed` (grep-regex-0.1.14 `src/error.rs:75-77`), and the same
///    `regex parse error: …` block — `(?:…)` wrapper included, since grep-regex wraps every
///    pattern that way (`src/config.rs:183-188`) — for a syntax error.
/// 2. ripgrep appends a two-line multiline hint, gated on the message text itself and on nothing
///    else (`suggest_multiline`, ripgrep 14.1.0 `crates/core/flags/hiargs.rs:1437-1448`). The
///    condition below is that predicate verbatim.
/// 3. ripgrep prefixes every top-level error with `rg: ` (`crates/core/messages.rs:50`, reached
///    from `eprintln_locked!("{:#}", err)` at `crates/core/main.rs:62`).
///
/// ripgrep's sibling `suggest_text` hint (`hiargs.rs:1451-1462`) is deliberately NOT reproduced:
/// it fires only on `ban_byte(Some(b'\x00'))`, which this matcher does not set.
fn rg_pattern_error(err: &grep_regex::Error) -> String {
    let msg = err.to_string();
    if msg.contains("the literal") && msg.contains("not allowed") {
        format!(
            "rg: {msg}\n\n\
             Consider enabling multiline mode with the --multiline flag (or -U for short).\n\
             When multiline mode is enabled, new line characters can be matched."
        )
    } else {
        format!("rg: {msg}")
    }
}
```

No new `use` line: `grep_regex` is already a direct dependency and the file already spells types
from it in full (`grep_regex::RegexMatcher` at
[grep.rs:77](../../../crates/cyrup-tools/src/tools/grep.rs)).

### Hunk 2 — set the line terminator and surface the refusal

Current ([grep.rs:308-312](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(input.ignore_case.unwrap_or(false))
            .fixed_strings(input.literal.unwrap_or(false))
            .build(&input.pattern)
            .map_err(|e| error::invalid(format!("grep: invalid pattern: {e}")))?;
```

Replacement:

```rust
        // `line_terminator(Some(b'\n'))` is ripgrep's non-multiline default, and Pi never passes
        // `-U/--multiline` (grep.ts:220-224 carries no such flag), so rg always takes the
        // else-branch of `matcher_rust`:
        //     builder.line_terminator(Some(b'\n')).dot_matches_new_line(false);
        // (ripgrep 14.1.0 `crates/core/flags/hiargs.rs:482-484`).
        //
        // Setting it makes grep-regex GUARANTEE the matcher can never produce a match containing
        // the terminator. `\n` is transparently subtracted from classes like `\s`, so `a\sb` keeps
        // building; a BARE `\n` literal cannot be removed without changing the pattern's intent, so
        // `build` fails with `ErrorKind::NotAllowed("\n")` (grep-regex-0.1.14 `src/config.rs:222-225`
        // → `src/strip.rs:60`). Leaving it at the builder default of `None` (`src/config.rs:61`)
        // skips that guard entirely — the pattern compiled fine and then matched nothing, because
        // the searcher below is line-oriented and no match can span a line break.
        //
        // `fixed_strings` stays wired to `input.literal` and needs no special casing: grep-regex
        // drops out of its literal-alternation fast path when a pattern contains the terminator
        // (`src/config.rs:113-124`), so a REAL newline still errors under `literal: true` while the
        // two-character `\n` escape stays an ordinary literal — exactly what rg does.
        //
        // Consistency with the searcher is required, not optional: `Searcher::check_config` errors
        // with `MismatchedLineTerminators` unless the two agree (grep-searcher-0.1.16
        // `src/searcher/mod.rs:805-821`). The searcher's default is `\n`
        // (`mod.rs:190` → grep-matcher-0.1.8 `src/lib.rs:268-273`) and is not overridden below.
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(input.ignore_case.unwrap_or(false))
            .fixed_strings(input.literal.unwrap_or(false))
            .line_terminator(Some(b'\n'))
            .build(&input.pattern)
            .map_err(|e| error::invalid(rg_pattern_error(&e)))?;
```

The `grep: invalid pattern: {e}` wrapper is **removed, not kept alongside**. It has no counterpart
in pi — pi emits rg's stderr and nothing else — and it is referenced nowhere else in the workspace
(`grep -rn 'invalid pattern' crates/` matches only this line).

### Resulting observable text

For `{"pattern": "a\nimport"}` (real newline or the two-character escape, `literal` false; or a
real newline with `literal` true), `execute` now returns `Err` whose message is byte-for-byte pi's:

```text
rg: the literal "\n" is not allowed in a regex

Consider enabling multiline mode with the --multiline flag (or -U for short).
When multiline mode is enabled, new line characters can be matched.
```

---

## Relationship to the finished cancellation task

[LOW — cancellation is only observed between candidate files](./LOW-cancellation-is-only-observed-between-candidate-files-not-during-a-file.md)
edits the same file. There is **no conflict**, and no ordering requirement:

- That task changes `search_one`'s signature
  ([grep.rs:72-83](../../../crates/cyrup-tools/src/tools/grep.rs)), its two `await` points
  (`:93-95`, `:162-163`), the `spawn_blocking` block (`:113-137`), `MatchSink` (`:243-259`), the two
  call sites (`:350-360`, `:415-425`), and a comment at `:388-393`. It inserts its new items
  immediately above the `MatchSink` doc comment at `:233`.
- This task changes only the `RegexMatcherBuilder` chain at `:308-312`, and inserts `rg_pattern_error`
  at `:32`. Neither region is touched by the other.

Two constraints that follow from it and must be honoured here:

- `search_one` takes `matcher: &grep_regex::RegexMatcher` and clones it into the blocking task. That
  is unchanged — the matcher is still built once in `execute` and shared. Do not move the builder
  into `search_one`.
- The cancellation task's `Cancelled` marker exists to distinguish an abort from a read failure
  *inside* the search. It is unrelated to a build failure: a pattern refusal happens in `execute`
  before any file is opened, and must remain a plain `Err` out of `execute`.

---

## Files that do NOT change

- [ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs),
  [ops/local/fs.rs](../../../crates/cyrup-tools/src/ops/local/fs.rs) — no filesystem seam is
  involved; the refusal happens before the first `metadata`/`walk`/`read_stream`.
- [error.rs](../../../crates/cyrup-tools/src/error.rs) — `error::invalid` already takes an
  `impl Into<String>` and produces a flat `ToolError { message }`. No new helper, no new variant.
- [find.rs](../../../crates/cyrup-tools/src/tools/find.rs) — `find` matches file *names* through
  fd's globbing, never file content, and has no `RegexMatcherBuilder`.
- [validate.rs](../../../crates/cyrup-provider/src/validate.rs) — the preflight coercer stays
  type-only. The refusal belongs at the matcher, exactly where rg puts it, so `pattern` keeps its
  bare `type: "string"` schema and cyrup's tool declaration stays byte-identical to pi's.
- [Cargo.toml](../../../Cargo.toml) — `grep-regex 0.1.14` already exposes
  `RegexMatcherBuilder::line_terminator` and `grep_regex::Error`. No dependency change.

## Explicitly out of scope

These are real divergences from ripgrep's `matcher_rust` observed during this research. They are
**separate findings** and must not be folded into this change:

- `.multi_line(true)` (`hiargs.rs:462`) — governs whether `^`/`$` anchor per line or per haystack.
  Different observable surface, different reasoning, different risk.
- `.ban_byte(Some(b'\x00'))` (`hiargs.rs:502-504`) — the sibling guard that makes
  `rg -- 'a\x00b'` print `pattern contains "\0" but it is impossible to match` plus the `--text`
  hint. Adding it would also make `suggest_text` live; it is deliberately not reproduced here.
- `.unicode(…)`, `.octal(false)`, `.size_limit(…)`, `.dfa_size_limit(…)` — none are set today and
  none affect the newline refusal.

---

## Definition of done

Stated as behaviour observable by driving the `grep` tool:

1. `{"pattern": "a<newline>import"}` — a real `\n` byte in the pattern — makes `execute` return
   `Err`, not a `ToolResult`, and the error message is exactly:

   ```text
   rg: the literal "\n" is not allowed in a regex

   Consider enabling multiline mode with the --multiline flag (or -U for short).
   When multiline mode is enabled, new line characters can be matched.
   ```

   including the `rg: ` prefix, the blank line, and both hint lines.
2. `{"pattern": "a\\nimport"}` — the two-character regex escape — produces that same error.
3. `{"pattern": "a<newline>import", "literal": true}` produces that same error.
4. `{"pattern": "a\\nimport", "literal": true}` produces a normal `No matches found`, **not** an
   error — the escape is an ordinary literal under fixed-strings, as it is for `rg --fixed-strings`.
5. `{"pattern": "a\\rb"}` produces a normal `No matches found`, not an error — only `\n` is banned,
   because `crlf` is not enabled.
6. `{"pattern": "a\\sb"}` still builds and still searches; it simply cannot match across a line
   break.
7. The refusal happens before any filesystem work: it is returned even when `path` names a directory
   that does not exist, and no file in the tree is opened.
8. A malformed pattern such as `{"pattern": "a("}` returns `rg: regex parse error:` followed by
   ripgrep's own caret block — the string `grep: invalid pattern:` appears nowhere in any output.
9. Every pattern that matched before still returns the identical rows, in the identical order, with
   the identical `[… limit reached …]` notice bracket and the identical `No matches found` string.
10. Nothing pi lacks is introduced: no new tool parameter, no multiline search mode, no `--text`
    style hint, no change to the tool's declared JSON schema.
