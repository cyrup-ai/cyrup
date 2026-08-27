---
title: Find glob matching is always case-sensitive; pi (fd) applies smart-case by default
priority: MEDIUM
tool: find
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: in-progress
updated: 2026-08-27
---

# Find glob matching is always case-sensitive; pi (fd) applies smart-case by default

## What pi does

pi's `find` shells out to the real `fd` binary. [pi find.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/find.ts):235 builds the argv as `["--glob", "--color=never", "--hidden"]` and the rest of the construction (find.ts:235-267) adds only `--no-require-git`, `--max-results N`, `--full-path`, then `args.push("--", effectivePattern, searchPath)`. It never passes `--case-sensitive`/`-s` or `--ignore-case`/`-i`. The tool schema (find.ts:29-35) exposes only `pattern`, `path`, `limit` — there is no case parameter to pass. So fd's default governs, and fd's default is **smart case**.

The binary is whatever `sharkdp/fd` release is current at download time, with `10.3.0` pinned for darwin-x64 ([pi tools-manager.ts](../../../tmp/pi/packages/coding-agent/src/utils/tools-manager.ts):30-52, :250-253). The rule below is quoted from **fd v10.3.0** and was re-checked against `master`; it is byte-identical in both.

### fd's exact smart-case rule

1. Every pattern is turned into a **regex string first** — <https://github.com/sharkdp/fd/blob/v10.3.0/src/main.rs#L93-L99>:

   ```rust
   let pattern_regexps = exprs
       .as_ref().unwrap_or(&empty).iter().chain([pattern])
       .map(|pat| build_pattern_regex(pat, &opts))
       .collect::<Result<Vec<String>>>()?;
   ```

2. In `--glob` mode that conversion is globset's own regex emission — main.rs:169-172:

   ```rust
   fn build_pattern_regex(pattern: &str, opts: &Opts) -> Result<String> {
       Ok(if opts.glob && !pattern.is_empty() {
           let glob = GlobBuilder::new(pattern).literal_separator(true).build()?;
           glob.regex().to_owned()
   ```

3. The case decision is taken on that **regex string**, not on the raw glob — main.rs:195-202:

   ```rust
   // The search will be case-sensitive if the command line flag is set or
   // if any of the patterns has an uppercase character (smart case).
   let case_sensitive = !opts.ignore_case
       && (opts.case_sensitive
           || pattern_regexps
               .iter()
               .any(|pat| pattern_has_uppercase_char(pat)));
   ```

4. `pattern_has_uppercase_char` (`src/regex_helper.rs`) parses the regex with `regex_syntax` and returns true only for an uppercase char in a **literal** or in a **character-class range endpoint** — `char::is_uppercase` (Unicode), not `is_ascii_uppercase`. Regex metacharacters and escapes do not count; upstream's own worked examples are `pattern_has_uppercase_char("foo.[a-zA-Z]") == true`, `pattern_has_uppercase_char(r"\Acargo") == false`, `pattern_has_uppercase_char(r"carg\x6F") == false`.

5. The verdict is applied by recompiling that regex — main.rs:478-481:

   ```rust
   RegexBuilder::new(&pattern_regex)
       .case_insensitive(!config.case_sensitive)
   ```

**Flag interaction** (fd `src/cli.rs`:125-146): `-s/--case-sensitive` and `-i/--ignore-case` each `overrides_with` the other, and the expression above makes `--ignore-case` win outright (`!opts.ignore_case && (…)`). pi passes neither, so both are `false` and the rule collapses to: **case-sensitive iff the pattern contains an uppercase character; case-insensitive otherwise.**

So upstream `find(pattern: "*.md")` returns `README.MD`, `CHANGELOG.Md` and `notes.md`; `find(pattern: "*.MD")` returns only `README.MD`.

### Why a scan of the raw glob string reproduces step 3 exactly

fd inspects the glob-derived regex, but for a glob the two are provably equivalent, so cyrup does not need to materialise a regex:

* globset's emitter (`globset-0.4.18` `src/glob.rs`:673-762) can only produce `(?-u)`, `(?i)`, `^`, `$`, `.`, `.*`, `[^/]`, `[^/]*`, `(?:/?|.*/)`, `/.*`, `(?:/|/.*/)`, `(?:…|…)`, `[…]` and escaped literals. Literals go through `char_to_escaped_literal`/`bytes_to_escaped_literal` (glob.rs:765-790), which escape ASCII via `regex_syntax::escape_into` and encode non-ASCII bytes as **lowercase** `\xNN`. **No uppercase letter is ever introduced by the translation.**
* Conversely, every uppercase character present in the glob survives into an HIR literal or a class range endpoint, because no glob metacharacter (`* ? [ ] { } ! - , / \`) is an uppercase letter. `{Foo,bar}` recurses into the alternation; `[aBc]` becomes single-char ranges whose start and end are both `B`. The only tokens globset ever drops are empty alternates, which contain nothing.
* `literal_separator` (fd always `true`; cyrup uses `full_path`) only chooses between `.*` and `[^/]*` — it cannot change the case verdict.

Therefore `effective.chars().any(char::is_uppercase)` is exact, and it must be `char::is_uppercase` (Unicode) to match fd on non-ASCII.

**Which string the rule reads.** fd sees the pattern pi actually hands it — `effectivePattern`, after the `**/` prefix is prepended (find.ts:257-262). cyrup's equivalent is `effective` in `PatternMatcher::build`, so the predicate reads `effective`. The verdict is identical either way, since `**/` contains no uppercase character (the same is true of pi's Windows-only `[/\\]` rewrite at find.ts:264-265).

**Case folding is ASCII-only on both sides.** globset writes `(?-u)` and then `(?i)` (glob.rs:675-678); fd applies `RegexBuilder::case_insensitive` to a pattern that already begins with `(?-u)`. So `*.md` matches `README.MD` in both, and neither folds `É`/`é`.

## What cyrup-tools does

[find.rs](../../../crates/cyrup-tools/src/tools/find.rs):131 calls `PatternMatcher::build(&input.pattern)`, and find.rs:207 `matcher.is_match(&abs_posix, &basename)` is the sole match site. [globmatch.rs](../../../crates/cyrup-tools/src/tools/globmatch.rs):37-40 compiles the glob as:

```rust
let glob: Glob = GlobBuilder::new(&effective)
    .literal_separator(full_path)
    .build()
```

`.case_insensitive(...)` is never called, so `GlobOptions::default()` applies (`globset-0.4.18` glob.rs:240-248: `case_insensitive: false`) and matching is case-SENSITIVE unconditionally. Neither the pattern nor the basename is lowercased anywhere on the path. A crate-wide search for `case_insensitive|smart.case|to_lowercase|eq_ignore_ascii|ignore_case` over `crates/cyrup-tools/src` returns only [grep.rs](../../../crates/cyrup-tools/src/tools/grep.rs):23/51/309 (`grep`'s explicit `ignoreCase` parameter), [ls.rs](../../../crates/cyrup-tools/src/tools/ls.rs):129/229 (collation sort) and `ops/shell.rs`:266 (PATH lookup) — no smart-case path exists for `find` under any name.

## User-visible impact

`find` silently returns fewer results than upstream on any case-varying tree: `*.md` misses `README.MD` and `CHANGELOG.Md`; `makefile` misses `Makefile`; `src/**/*.ts` misses `SRC/app.TS`. The model receives an empty or short result set and concludes the files do not exist. There is no parameter to opt back in, because pi exposes none either — the behaviour must simply be correct by default.

## Required change

One required path. Two edits, both in [globmatch.rs](../../../crates/cyrup-tools/src/tools/globmatch.rs), inside/adjacent to `PatternMatcher::build` only.

### 1. Add the smart-case predicate

Insert this private free function immediately **above** `impl PatternMatcher` (i.e. after the `PatternMatcher` struct definition, currently ending at globmatch.rs:23):

```rust
/// fd's smart case, reproduced for `find` (fd v10.3.0 `src/main.rs:195-202`): the search is
/// case-sensitive **iff** the pattern carries an uppercase character, and case-INSENSITIVE
/// otherwise. pi passes neither `-s/--case-sensitive` nor `-i/--ignore-case` (find.ts:235-267)
/// and its schema has no case parameter (find.ts:29-35), so fd's default is the whole rule.
///
/// fd runs this over the *regex* globset emits for the glob (main.rs:169-172, then
/// `regex_helper::pattern_has_uppercase_char`, which counts uppercase only in HIR literals and
/// class range endpoints). Scanning the glob string itself is equivalent: globset's emitter
/// (`globset-0.4.18` glob.rs:673-790) introduces no uppercase letter — non-ASCII bytes become
/// lowercase `\xNN` escapes — and no glob metacharacter is an uppercase letter, so the uppercase
/// characters of the glob and of its regex are the same set.
///
/// `char::is_uppercase` (Unicode), NOT `is_ascii_uppercase`: fd's check is Unicode-aware.
fn pattern_has_uppercase_char(pattern: &str) -> bool {
    pattern.chars().any(char::is_uppercase)
}
```

### 2. Apply it in the builder

CURRENT — globmatch.rs:37-40:

```rust
        let glob: Glob = GlobBuilder::new(&effective)
            .literal_separator(full_path)
            .build()
            .map_err(|e| error::invalid(format!("invalid glob '{pattern}': {e}")))?;
```

REPLACEMENT:

```rust
        // fd's smart case. The verdict is taken on `effective` — the string fd itself receives,
        // after pi prepends `**/` (find.ts:257-262) — not on the raw `pattern`. `**/` holds no
        // uppercase character, so the two agree on every input; `effective` is simply the literal
        // equivalent of fd's own input.
        let glob: Glob = GlobBuilder::new(&effective)
            .literal_separator(full_path)
            .case_insensitive(!pattern_has_uppercase_char(&effective))
            .build()
            .map_err(|e| error::invalid(format!("invalid glob '{pattern}': {e}")))?;
```

Nothing else changes: `full_path`, the `**/` prefixing arms, the error text, and `PatternMatcher::is_match` all stay exactly as they are.

### Do NOT touch `RgGlob`

`RgGlob` reproduces ripgrep's `--glob` override rule for `grep`, which pi feeds verbatim to `rg` ([pi grep.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts):223 `if (glob) args.push("--glob", glob);`). ripgrep turns override globs case-insensitive only for `--iglob`/`--glob-case-insensitive`, which pi never passes, so `RgGlob` is correctly case-sensitive and must stay so. `grep`'s own `ignoreCase` parameter (grep.rs:309) is a separate, explicit switch and is untouched. Smart case is a `find`-only rule.

## Files that change

* [crates/cyrup-tools/src/tools/globmatch.rs](../../../crates/cyrup-tools/src/tools/globmatch.rs) — the only file. New private fn `pattern_has_uppercase_char`; one added builder call inside `PatternMatcher::build`.

No other file changes. [find.rs](../../../crates/cyrup-tools/src/tools/find.rs) needs no edit: it already routes every candidate through `PatternMatcher`, so the new verdict reaches the match site at find.rs:207 unchanged.

## Sibling-task collision note

The queued task *"glob dir prunes the whole directory in pi but only filters files in cyrup"* also edits [globmatch.rs](../../../crates/cyrup-tools/src/tools/globmatch.rs). The two do not overlap in code:

* **This task** touches only the `PatternMatcher` region (globmatch.rs:20-45): a new free fn between the struct and its `impl`, plus one line inside `PatternMatcher::build`. It does not touch `RgGlob`, `keeps_file`, `to_posix`, or [grep.rs](../../../crates/cyrup-tools/src/tools/grep.rs).
* **The sibling** touches only `RgGlob` (globmatch.rs:85-167) — adding a directory-matching entry point beside `keeps_file` — plus its call sites in `grep.rs`.

Whichever lands second must re-anchor by symbol name, not by line number: inserting `pattern_has_uppercase_char` shifts every line below globmatch.rs:23 by the length of the new function, so the sibling's `RgGlob` line citations move.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Confirmed absent after an exhaustive search. globmatch.rs:36-41 `PatternMatcher::build` compiles the glob with `GlobBuilder::new(&effective).literal_separator(full_path).build()` and never calls `.case_insensitive(...)`, so globset's case-SENSITIVE default applies unconditionally. find.rs:131 is the sole call site and find.rs:207 `matcher.is_match(&abs_posix, &basename)` the sole match site; neither the pattern nor the basename is lowercased anywhere on the path. Crate-wide `rg -i 'case_insensitive|smart.?case|to_lowercase|eq_ignore_ascii|ignore_case'` across cyrup-tools/src AND cyrup-core/src yields only grep.rs:23/51/309 (grep's explicit `ignoreCase` parameter) and ls.rs:117-129/229 (collation sort) — no smart-case path exists for `find`, under any name. There is also no fd shell-out fallback: find.rs:1-2 states the tool uses `ignore::WalkBuilder` + `globset` in place of the fd binary, and every other `fd` hit in the crate is a comment or unrelated file-descriptor text. Upstream side verified independently: `rg 'case_sensitive|case_insensitive|smart'` over pi's find.ts returns zero hits and the argv construction at find.ts:235-267 adds only --glob/--color=never/--hidden/--no-require-git/--max-results/--full-path, so fd's documented smart-case default governs (all-lowercase pattern matches case-insensitively, including in --glob mode). I also checked pi's alternate `customOps.glob` branch (find.ts:167-176) to be sure cyrup was not porting that one instead — it is not; it ports the default fd branch. This is a genuinely missing capability, not a different implementation of it.

Re-verified during augmentation against pi 0.84.3 and fd v10.3.0: every line citation above holds, except that pi's `rg` argv line has moved to grep.ts:223 and the `find` schema sits at find.ts:29-35.

## Definition of done

Observable behaviour of the `find` tool, in a tree containing `README.MD`, `CHANGELOG.Md`, `notes.md`, `Makefile`, `src/app.ts` and `SRC/app.TS`:

1. `find(pattern: "*.md")` returns `README.MD`, `CHANGELOG.Md` and `notes.md` — an all-lowercase pattern matches case-insensitively.
2. `find(pattern: "*.MD")` returns `README.MD` only — one uppercase character anywhere in the pattern makes the whole match case-sensitive.
3. `find(pattern: "*.Md")` returns `CHANGELOG.Md` only.
4. `find(pattern: "makefile")` returns `Makefile`; `find(pattern: "Makefile")` returns `Makefile`; `find(pattern: "MAKEFILE")` returns nothing.
5. `find(pattern: "src/**/*.ts")` returns both `src/app.ts` and `SRC/app.TS` — smart case applies identically in full-path mode, where the pattern compiled is `**/src/**/*.ts`.
6. `find(pattern: "src/**/*.TS")` returns `SRC/app.TS` only.
7. A non-ASCII pattern folds nowhere: `find(pattern: "café.txt")` does not match `CAFÉ.TXT`, matching fd's ASCII-only folding.
8. `grep` is unaffected: `grep(pattern: "NEEDLE", glob: "*.md")` still does not match `README.MD`, and `grep`'s `ignoreCase` parameter behaves exactly as before.
9. `find` with an invalid glob still fails with the same `invalid glob '<pattern>': …` message.
10. No new parameter appears on the `find` schema — it stays `pattern`, `path`, `limit`, byte-for-byte pi's (find.ts:29-35). This is a parity fix, not a redesign.
