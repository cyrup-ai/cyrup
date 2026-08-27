---
stage: qa
status: completed
updated: 2026-08-27 09:45
---

# MCP-370: Port `includeTools` And Glob `excludeTools` Into The In-Tree Reader


## COMPLETED — QA 10/10

The reader applies `includeTools`. It previously read `excludeTools` alone, so a server
configured to expose a named subset exposed everything.

Verified against the writer rather than in isolation, since reader/writer agreement is the
point of the unit: `is_tool_allowed` is byte-identical to `cyrup_mcp::registration`'s modulo
one comment; `matches_tool_selector` differs only by `shift_remove` -> `remove`, the documented
`HashSet` consequence; `matches_tool_pattern` is condensed but semantically equal, since
`patterns.iter().any(..)` on an empty slice is the writer's explicit early `false`.

One real divergence exists and is documented at the call site: the writer's glob arm compiles
to a regex and answers `None` when the size ceiling is hit, treating that pattern as matching
nothing, while this matcher compiles nothing and has no such failure mode. The note reasons
correctly that with this escape set the produced pattern is always syntactically valid, so the
ceiling is the only path that reaches `None`.

The glob matcher uses `chars()`/`Chars::as_str()` with no indexing, no `unwrap`/`expect` and no
`regex` dependency, following the existing lint-clean port in `exec/model_scope.rs` rather than
adding a third implementation.

The catch that mattered most: deleting `is_tool_excluded` would have orphaned a doc link under
`broken_intra_doc_links = deny` — a build break, not a stale sentence.

Two asserts beyond the prescribed edits drive the writer's `is_tool_allowed` over the same
fixture. Kept: that test's purpose is reader/writer agreement and nothing else pinned the new
filter against its twin.

The task's own "which writer to transcribe from" table was stale on arrival — the previous
commit deleted one of the three copies it named, two hours after augmentation. Transcribing
from `registration.rs` as it now stands surfaced its grouped legacy emission, which the task
had prescribed as interleaved.

Gates: check 0 warnings, doc exit 0, 7870/7870.

---

## The finding that reframes this task

**The writer already implements all of it.** This is a one-sided port into the reader, not a design
job — every semantic below is transcribed from working code in
[registration.rs](../../crates/cyrup-mcp/src/registration.rs), which is itself a verified
line-for-line port of [types.ts](../../tmp/pi-mcp-adapter/types.ts) and
[direct-tools.ts](../../tmp/pi-mcp-adapter/direct-tools.ts) at `v2.26.1` (`fafae21`).

**And the reader's rule is not a *subset* of the writer's — it is a *different* rule.** It
over-approximates on `includeTools` (unapplied entirely), but on `excludeTools` it also
**under-approximates**, because `is_tool_excluded` (`mcp_direct_tools.rs:1155-1181`) normalises
`-` -> `_` on **both** the candidates *and the user's pattern*, via `normalize_tool_name`
(`mcp_direct_tools.rs:1183-1185`). The writer never touches the pattern: it compares patterns
against an explicit *legacy candidate set* (`registration.rs:277-300`) guarded by a cross-server
disambiguation index (`registration.rs:369-423`). Concretely, for tool `click` on server
`browser-mcp` at the default `server` prefix — every row below hand-evaluated through both
implementations:

| `excludeTools` entry | reader today | writer | why |
| --- | --- | --- | --- |
| `browser_click` | excluded | excluded | current `short`-mode candidate — the two agree |
| `browser-click` | **excluded** | **kept** | reader normalises the *pattern*; the writer has no such candidate |
| `browser_mcp_click` | **kept** | **excluded** | legacy candidate `format_tool_name(...).replace('-', "_")` |
| `browser_2d_mcp_click` | **kept** | **excluded** | legacy candidate `format_legacy_tool_name` — `-` hex-escaped out of the *server* prefix |
| `browser*` | **kept** | **excluded** | glob, unsupported by the reader |

So `normalize_tool_name` must be **deleted**, not kept alongside the new code. Porting the filter
without removing it leaves a third rule that is neither side's.

## Which writer to transcribe from

`cyrup-mcp` carries **three** implementations of this filter. Only one is this reader's twin:

| Site | Ports | Use it? |
| --- | --- | --- |
| `registration.rs:277-481`, applied at `registration.rs:1111-1230` | `direct-tools.ts` `resolveDirectTools` | **yes — the reader's exact twin** |
| `proxy/tool_metadata.rs:232-420` | `tool-metadata.ts`, with the `additionalCurrentCandidatesByToolName` arms `direct-tools.ts` never supplies; uses `IndexSet` | no |
| `ui.rs:1491-1530` | the `Set<string>` branch of `ToolSelectorCandidateContext` (`types.ts:844`, `:904-905`) | no |

Both this reader and `registration.rs` resolve **direct tools**. Transcribe `registration.rs`.

## What each side does now, exactly

### Writer — `cyrup-mcp` (complete)

| Concern | Symbol | Site | Upstream |
| --- | --- | --- | --- |
| glob -> regex | `glob_to_regex` | `registration.rs:310-326` | `types.ts:804-807` |
| is this a glob? | `is_glob` | `registration.rs:328-330` | `types.ts:836`, `:855` |
| pattern vs candidate set | `matches_tool_pattern` | `registration.rs:336-360` | `types.ts:828-841` |
| candidate names (current + legacy) | `tool_name_candidates` | `registration.rs:277-300` | `types.ts:777-802` |
| legacy naming grammar | `legacy_server_prefix` / `format_legacy_tool_name` | `registration.rs:248-268` | `types.ts:764-775` |
| cross-server disambiguation | `CandidateIndex` / `has_other_current_match` | `registration.rs:369-423` | `types.ts:809-887` |
| the three-step selector rule | `matches_tool_selector` | `registration.rs:425-453` | `types.ts:889-909` |
| `include && !exclude` | `is_tool_allowed` | `registration.rs:458-481` | `types.ts:911-942` |
| index built lazily, only when filtered | `has_tool_filters` / `build_candidate_index` | `registration.rs:1062-1069` / `1071-1095` | `direct-tools.ts:153-174` |
| applied to tools and to resources | `resolve_direct_tools` | `registration.rs:1159-1166`, `1200-1207` | `direct-tools.ts:179`, `:205` |

The config fields are typed and lenient in [config.rs](../../crates/cyrup-mcp/src/config.rs):
`tool_prefix` `config.rs:845-847`, `include_tools` `config.rs:848-850`, `exclude_tools`
`config.rs:851-853` — all three `#[serde(default, deserialize_with = "lenient")]`. `ToolPrefix` is
`config.rs:1313-1325`, `#[serde(rename_all = "lowercase")]` over `server|none|short|mcp`.

### Reader — `cyrup-ext-subagents` (filter half missing)

In [mcp_direct_tools.rs](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs),
`include_tools` / `exclude_tools` are deserialised (`mcp_direct_tools.rs:267-275`) and hashed into
the 15-key identity pre-image (`mcp_direct_tools.rs:840-841`) — but only `exclude_tools` is ever
*read*, through the 5-candidate exact matcher at `mcp_direct_tools.rs:1155`, called from
`mcp_direct_tools.rs:621` (tools) and `mcp_direct_tools.rs:654` (resources). `includeTools` has no
read site at all — `grep -n "include_tools" mcp_direct_tools.rs` returns exactly three lines: the
field at `:273`, the hash key at `:840`, and its own doc comment.

The reader's own doc comments already name this as the open half (`mcp_direct_tools.rs:267-272`,
`:1087-1089`, `:1164-1168`, and the module-header bullet at `:43-44`); this change makes them false
and must rewrite them.

## The writer's exact semantics, to be reproduced rather than approximated

**1 — glob grammar** (`registration.rs:310-330`, `types.ts:804-807`). A pattern is a glob iff it
contains `*` or `?`. A glob becomes the anchored regex `^…$` with `. + ^ $ { } ( ) | [ ] \` escaped,
`*` -> `.*` and `?` -> `.`; everything else is literal. A non-glob pattern is an **exact
set-membership test** — it is never regex-compiled, so a candidate holding a literal `*` can never
be matched by a `*`-bearing pattern.

**2 — candidate names** (`registration.rs:277-300`, `types.ts:777-802`).
`tool_name_candidates(tool, server, prefix, include_legacy)`:

* current (`include_legacy = false`), 5 expressions: the bare `tool`; `format_tool_name` at the
  effective prefix; and `format_tool_name` at each of `Server`, `Short`, `Mcp`.
* legacy (`include_legacy = true`) adds 13 more, built from `tool.replace('-', "_")`,
  `format_legacy_tool_name` (which folds `.` **and** `-` in the tool name and escapes `-` out of the
  server prefix via `sanitize_server_prefix(_, false)`), and
  `format_tool_name(...).replace('-', "_")` — each at the effective prefix plus the same three
  modes. Heavy overlap: the resulting set is far smaller than 18.

**3 — the selector rule** (`registration.rs:425-453`, `types.ts:889-909`), three steps in order:

1. any **current** candidate matches a pattern -> `true`;
2. no index supplied -> fall back to matching the **full legacy** set;
3. otherwise subtract the current set from the legacy set (`types.ts:901`), and a pattern wins only
   if it matches a **legacy-only** candidate **and** does not name some *other* tool's current
   candidate (`has_other_current_match`, `registration.rs:384-423`).

**4 — `isToolAllowed`** (`registration.rs:458-481`, `types.ts:932-942`): an absent-or-empty
`includeTools` includes everything, otherwise the selector rule decides; then `excludeTools`, same
rule, negated. `include` is evaluated **first** and short-circuits — JS `&&` short-circuits too, so
this is faithful, not an optimisation.

**5 — the index** (`registration.rs:1071-1095`, `direct-tools.ts:156-174`): the *current* candidate
names of **every** enabled server with a valid cache entry — tools, and (unless
`exposeResources: false`) resources. The filtered server's own candidates are **not** subtracted at
build time; the subtraction happens by match count inside `has_other_current_match`. Built
**lazily**, only when the server carries a non-empty `includeTools` or `excludeTools`
(`registration.rs:1062-1069`, `registration.rs:1140-1141`).

## Prescription

All edits land in
[mcp_direct_tools.rs](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs).

Two hard constraints on the code below, both verified:

* **No `regex` dependency.** [Cargo.toml](../../crates/cyrup-ext-subagents/Cargo.toml) names none —
  `[dependencies]` opens at `Cargo.toml:23` and contains no `regex` line — and the crate states the
  ban as a rule in its own modules (`exec/task_intent.rs:31-39`, `exec/output.rs:442-444`). Do not
  add one.
* **`clippy::indexing_slicing = "deny"`** (workspace `Cargo.toml:101`, alongside `unwrap_used`,
  `expect_used`, `panic` at `:98-100`; `crates/cyrup-ext-subagents/Cargo.toml:11-12` inherits with
  `[lints] workspace = true`). A `hay[h]` / `pat[p..]` backtracking matcher will not compile here.
  Every matcher below walks `str::chars()` and `Chars::as_str()` instead.

`HashMap`/`HashSet` are imported at `mcp_direct_tools.rs:104`, `serde_json::Value` at `:108`.

### Step 1 — prerequisite: per-server `toolPrefix`

The reader resolves one global prefix (`mcp_direct_tools.rs:427`) and has no per-server override;
the writer's `resolve_tool_prefix` (`registration.rs:243-245`, `types.ts:702`) does. This matters
twice: the emitted name is `format_tool_name(tool, server, effective_prefix)`, and the
`ToolPrefix::None` corner of the candidate set differs (`format_tool_name(tool, server, None)` is
`tool.replace('.', "_")`, which the three-mode arms never produce). `toolPrefix` is **not** one of
the fifteen identity keys (`mcp_direct_tools.rs:805-842`), so adding it moves no digest.

Add to `ServerEntry` (`mcp_direct_tools.rs:229-276`):

```rust
    /// `resolveToolPrefix` (`cyrup_mcp::registration::resolve_tool_prefix`, `types.ts:702`) — a
    /// per-server override of `settings.toolPrefix`. Held as a raw string and parsed by
    /// [`parse_tool_prefix`] so an unrecognised value degrades to "inherit the global", which is
    /// what the writer's `lenient` `Option<ToolPrefix>` does (`config.rs:846-847`) — and is NOT
    /// what [`get_tool_prefix`]'s catch-all does.
    ///
    /// Not one of the fifteen identity keys: it renames tools, it does not redefine the server.
    #[serde(default, rename = "toolPrefix", deserialize_with = "lenient_string")]
    pub tool_prefix: Option<String>,
```

and make the two filter fields lenient the same way, so a wrong-typed `excludeTools` degrades the
*field* instead of dropping the whole server at `extract_server_map`
(`mcp_direct_tools.rs:506-518`) while the writer keeps it:

```rust
    #[serde(default, rename = "includeTools", deserialize_with = "lenient_string_list")]
    pub include_tools: Option<Vec<String>>,
    #[serde(default, rename = "excludeTools", deserialize_with = "lenient_string_list")]
    pub exclude_tools: Option<Vec<String>>,
```

The two deserialisers mirror `cyrup_mcp::config::lenient` (`config.rs:479-486`) exactly: buffer the
field into a `Value`, try the typed shape, answer `None` when it does not fit. Place them beside
`lenient_epoch_ms` (`mcp_direct_tools.rs:355`), which already has this shape.

```rust
/// `cyrup_mcp::config::lenient` (`config.rs:479-486`) for a string field: a wrong-typed value
/// becomes `None` rather than failing the whole `ServerEntry`. `extract_server_map`
/// (`mcp_direct_tools.rs:506-518`) drops an entry that fails to deserialize; the writer never drops
/// one for this reason.
fn lenient_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Value::deserialize(deserializer)?;
    Ok(raw.as_str().map(str::to_string))
}

/// [`lenient_string`] for `string[]`. A non-array, an array with a non-string member, and an
/// explicit `null` are all `None`.
fn lenient_string_list<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value::<Vec<String>>(raw).ok())
}

/// A `toolPrefix` value parsed **strictly**: an unrecognised value is `None`. The four spellings are
/// `cyrup_mcp::config::ToolPrefix`'s `#[serde(rename_all = "lowercase")]` (`config.rs:1313-1325`).
///
/// [`get_tool_prefix`] (`mcp_direct_tools.rs:1066`) keeps its catch-all-to-`Server` behaviour for
/// the *global* settings key, because that is `getServerPrefix`'s final `return` (`types.ts:686`).
/// A **per-server** value must instead fall through to the global, which is what the writer's
/// `lenient` `Option<ToolPrefix>` produces.
fn parse_tool_prefix(value: &str) -> Option<ToolPrefix> {
    match value {
        "server" => Some(ToolPrefix::Server),
        "none" => Some(ToolPrefix::None),
        "short" => Some(ToolPrefix::Short),
        "mcp" => Some(ToolPrefix::Mcp),
        _ => None,
    }
}
```

### Step 2 — the legacy naming grammar

`sanitize_server_prefix` (`mcp_direct_tools.rs:1090-1102`) is hard-wired to
`preserve_provider_valid = true`, and its doc comment (`mcp_direct_tools.rs:1087-1089`) says the
legacy grammar "is not ported". Port it — one signature change plus two functions, transcribed from
`registration.rs:184-201` and `registration.rs:248-268`:

```rust
/// Port of `sanitizeServerPrefix` (`types.ts:668`) — the twin of
/// `cyrup_mcp::registration::sanitize_server_prefix` (`registration.rs:184`).
///
/// `preserve_provider_valid = true` (every naming call site) keeps `-`; `false` is the **legacy**
/// grammar, which escapes it — `github-mcp` becomes `github_2d_mcp`. The legacy form exists only to
/// build the alias candidates of `getToolNameCandidates` (`types.ts:777`).
fn sanitize_server_prefix(server_name: &str, preserve_provider_valid: bool) -> String {
    let mut out = String::with_capacity(server_name.len());
    for ch in server_name.chars() {
        let valid = if preserve_provider_valid {
            ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
        } else {
            ch.is_ascii_alphanumeric()
        };
        if valid {
            out.push(ch);
        } else {
            out.push('_');
            out.push_str(&format!("{:x}", ch as u32));
            out.push('_');
        }
    }
    out
}

/// `getLegacyServerPrefix` (`types.ts:764`, `registration.rs:248`) — the same four modes over the
/// pre-`-`/`_` grammar.
fn legacy_server_prefix(server_name: &str, mode: ToolPrefix) -> String {
    match mode {
        ToolPrefix::None => String::new(),
        ToolPrefix::Short => {
            let short = sanitize_server_prefix(strip_mcp_suffix(server_name), false);
            if short.is_empty() { "mcp".to_string() } else { short }
        }
        ToolPrefix::Mcp => format!("mcp__{}", sanitize_server_prefix(server_name, false)),
        ToolPrefix::Server => sanitize_server_prefix(server_name, false),
    }
}

/// `formatLegacyToolName` (`types.ts:771`, `registration.rs:261`) — here the tool name loses
/// **hyphens as well as dots**, which is the one place the two grammars differ on the tool half of
/// the name.
fn format_legacy_tool_name(tool_name: &str, server_name: &str, prefix: ToolPrefix) -> String {
    let server_prefix = legacy_server_prefix(server_name, prefix);
    let sanitized: String =
        tool_name.chars().map(|c| if c == '.' || c == '-' { '_' } else { c }).collect();
    if server_prefix.is_empty() { sanitized } else { format!("{server_prefix}_{sanitized}") }
}
```

Pass `true` at the three existing `sanitize_server_prefix(...)` call sites inside `get_server_prefix`
(`mcp_direct_tools.rs:1106-1122`). `strip_mcp_suffix` (`mcp_direct_tools.rs:1126-1136`) and
`format_tool_name` (`mcp_direct_tools.rs:1145-1153`) already match `registration.rs:205-213` and
`registration.rs:235-239`; they need no change.

### Step 3 — the glob matcher

The crate already ports one `globToRegExp` without a regex engine:
[model_scope.rs](../../crates/cyrup-ext-subagents/src/exec/model_scope.rs)'s `glob_matches`
(`model_scope.rs:164-192`) splits the pattern on `*` into literal segments and walks them. **Follow
that shape** — it is lint-clean, allocation-light and already reviewed — but it is `*`-only, so it
cannot be reused as-is: `types.ts:806` also maps `?` -> `.`.

Two facts make an exact regex-free equivalent short:

* regex `.` matches no newline in either JS or Rust, and neither `^` nor `$` is line-anchored (JS
  without `/m`, Rust with the default `multi_line(false)`). So a `\n` in the candidate can only ever
  be matched by a **literal** `\n` in the pattern. Splitting both sides on `\n` and matching
  line-for-line is therefore exact, and removes every newline special case from the inner matcher.
* with `\n` gone, each `*`-separated segment has a **fixed character length** (`?` consumes exactly
  one), so leftmost-first matching of each interior segment is optimal — the same argument
  `model_scope.rs:159-163` already records.

`glob_to_regex` answers `None` — "matches nothing" — when `Regex::new` rejects the pattern. With
that escape set the produced pattern is always syntactically valid, so the only reachable `None` is
the regex crate's size limit on a pathologically long pattern; this matcher has no failure mode and
needs none.

```rust
/// `isGlob` (`types.ts:836`, `registration.rs:328`).
fn is_glob(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

/// `globToRegExp(pattern).test(candidate)` (`types.ts:804-807`, `registration.rs:310-326`) with no
/// regex engine — this crate has no `regex` dependency and does not acquire one for a
/// two-metacharacter grammar.
///
/// `^…$` is not line-anchored and `.` matches no newline, so a `\n` in the candidate must be met by
/// a literal `\n` in the pattern: matching line-for-line is exact, and lets [`glob_line_matches`]
/// ignore newlines entirely.
fn glob_matches(pattern: &str, candidate: &str) -> bool {
    let mut candidate_lines = candidate.split('\n');
    for pattern_line in pattern.split('\n') {
        let Some(candidate_line) = candidate_lines.next() else {
            return false;
        };
        if !glob_line_matches(pattern_line, candidate_line) {
            return false;
        }
    }
    candidate_lines.next().is_none()
}

/// Anchored `*`/`?` glob match over newline-free inputs, in the segment-split shape of
/// `exec::model_scope::glob_matches` (`model_scope.rs:164`) widened for `?`.
fn glob_line_matches(pattern: &str, text: &str) -> bool {
    let mut segments = pattern.split('*');
    // `split` on a non-empty separator always yields at least one element, so this arm is
    // unreachable in practice; it keeps the function panic-free without an `expect`.
    let Some(first) = segments.next() else {
        return text.is_empty();
    };
    let Some(mut rest) = match_segment(text, first) else {
        return false;
    };
    let tail: Vec<&str> = segments.collect();
    let Some((last, middle)) = tail.split_last() else {
        // No `*` at all: the single literal segment had to consume the whole text.
        return rest.is_empty();
    };
    for segment in middle {
        let Some(next) = find_segment(rest, segment) else {
            return false;
        };
        rest = next;
    }
    match_segment_at_end(rest, last)
}

/// Match `segment` — literals plus `?`, never `*` and never `\n` — against the front of `text`,
/// answering the unconsumed tail. `?` is regex `.`, so it consumes exactly one character.
fn match_segment<'a>(text: &'a str, segment: &str) -> Option<&'a str> {
    let mut rest = text;
    for pattern_char in segment.chars() {
        let mut chars = rest.chars();
        let text_char = chars.next()?;
        if pattern_char != '?' && pattern_char != text_char {
            return None;
        }
        rest = chars.as_str();
    }
    Some(rest)
}

/// The leftmost position at which `segment` matches, answering the tail after it. An empty segment
/// (from `**`, or a `*` at either end) is vacuously satisfied where it stands.
fn find_segment<'a>(text: &'a str, segment: &str) -> Option<&'a str> {
    let mut cursor = text;
    loop {
        if let Some(rest) = match_segment(cursor, segment) {
            return Some(rest);
        }
        let mut chars = cursor.chars();
        chars.next()?;
        cursor = chars.as_str();
    }
}

/// `segment` must end exactly at the end of `text` (the `$` anchor) without reaching back into what
/// the interior segments already consumed — taking it from the remainder gives that for free.
fn match_segment_at_end(text: &str, segment: &str) -> bool {
    let Some(skip) = text.chars().count().checked_sub(segment.chars().count()) else {
        return false;
    };
    let mut chars = text.chars();
    for _ in 0..skip {
        if chars.next().is_none() {
            return false;
        }
    }
    match_segment(chars.as_str(), segment).is_some_and(str::is_empty)
}
```

### Step 4 — candidates, index, `isToolAllowed`

Replace `is_tool_excluded` (`mcp_direct_tools.rs:1155-1181`) and delete `normalize_tool_name`
(`mcp_direct_tools.rs:1183-1185`) outright — after this change `normalize_tool_name` has no caller
left in the crate (its only other mention is the stale comment at `mcp_direct_tools.rs:1756`,
Step 6).

```rust
/// `getToolNameCandidates` (`types.ts:777`) — the twin of
/// `cyrup_mcp::registration::tool_name_candidates` (`registration.rs:277`). Every name a user might
/// plausibly have written in an `includeTools`/`excludeTools` entry: 5 expressions for the current
/// grammar, 13 more for the legacy one. Set iteration order is never observed — every consumer asks
/// a membership or an "any matches" question.
fn tool_name_candidates(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    include_legacy: bool,
) -> HashSet<String> {
    const MODES: [ToolPrefix; 3] = [ToolPrefix::Server, ToolPrefix::Short, ToolPrefix::Mcp];
    let mut out = HashSet::new();
    out.insert(tool_name.to_string());
    out.insert(format_tool_name(tool_name, server_name, prefix));
    for mode in MODES {
        out.insert(format_tool_name(tool_name, server_name, mode));
    }
    if include_legacy {
        let legacy_tool_name = tool_name.replace('-', "_");
        out.insert(legacy_tool_name.clone());
        out.insert(format_tool_name(&legacy_tool_name, server_name, prefix));
        out.insert(format_legacy_tool_name(tool_name, server_name, prefix));
        out.insert(format_tool_name(tool_name, server_name, prefix).replace('-', "_"));
        for mode in MODES {
            out.insert(format_tool_name(&legacy_tool_name, server_name, mode));
            out.insert(format_legacy_tool_name(tool_name, server_name, mode));
            out.insert(format_tool_name(tool_name, server_name, mode).replace('-', "_"));
        }
    }
    out
}

/// `matchesToolPattern` (`types.ts:828`, `registration.rs:336`): an exact membership test for a
/// literal pattern, a glob test over every candidate otherwise. A literal pattern is **never**
/// glob-matched, and a glob pattern is never compared literally. Upstream's leading
/// `patterns.length === 0` guard is folded into `any`, which answers `false` on an empty slice.
fn matches_tool_pattern(candidates: &HashSet<String>, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| {
        if is_glob(pattern) {
            candidates.iter().any(|candidate| glob_matches(pattern, candidate))
        } else {
            candidates.contains(pattern)
        }
    })
}

/// `ToolSelectorCandidateIndex` (`types.ts:809`, `registration.rs:369`) — the *current* candidate
/// names of every server with a valid cache entry, plus upstream's match-count memo. The writer
/// also memoises the compiled regex (`matcherByPattern`); nothing is compiled here, so only the
/// count table is carried.
///
/// The index is built over **all** servers, the one being filtered included — the subtraction is by
/// match count in [`CandidateIndex::has_other_current_match`], not at build time.
/// `additionalCurrentCandidatesByToolName` is deliberately absent: it exists only for
/// `tool-metadata.ts`'s speculative arms, and `direct-tools.ts` never supplies it.
#[derive(Debug, Default)]
struct CandidateIndex {
    all_current: HashSet<String>,
    matching_count: HashMap<String, usize>,
}

impl CandidateIndex {
    /// `indexHasOtherCurrentMatch` (`types.ts:846`, `registration.rs:384`) — does `pattern` name a
    /// *different* tool's current name?
    fn has_other_current_match(
        &mut self,
        current_candidates: &HashSet<String>,
        pattern: &str,
    ) -> bool {
        if !is_glob(pattern) {
            return self.all_current.contains(pattern) && !current_candidates.contains(pattern);
        }
        // Disjoint field borrows: the closure reads `all_current` while `matching_count` is
        // borrowed mutably, exactly as `registration.rs:400-408` does.
        let all_current = &self.all_current;
        let total = *self.matching_count.entry(pattern.to_string()).or_insert_with(|| {
            all_current.iter().filter(|candidate| glob_matches(pattern, candidate)).count()
        });
        if total == 0 {
            return false;
        }
        let mine = current_candidates
            .iter()
            .filter(|candidate| {
                self.all_current.contains(*candidate) && glob_matches(pattern, candidate)
            })
            .count();
        total > mine
    }
}

/// `matchesToolSelector` (`types.ts:889`, `registration.rs:425`), the three-step disambiguation
/// rule. Step 3 is what stops a legacy alias from silently filtering the wrong tool once two servers
/// exist whose sanitized prefixes collide.
fn matches_tool_selector(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    patterns: &[String],
    index: Option<&mut CandidateIndex>,
) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let current = tool_name_candidates(tool_name, server_name, prefix, false);
    if matches_tool_pattern(&current, patterns) {
        return true;
    }
    let Some(index) = index else {
        return matches_tool_pattern(
            &tool_name_candidates(tool_name, server_name, prefix, true),
            patterns,
        );
    };
    let mut legacy = tool_name_candidates(tool_name, server_name, prefix, true);
    for candidate in &current {
        legacy.remove(candidate);
    }
    patterns.iter().any(|pattern| {
        matches_tool_pattern(&legacy, std::slice::from_ref(pattern))
            && !index.has_other_current_match(&current, pattern)
    })
}

/// `isToolAllowed` = `isToolIncluded && !isToolExcluded` (`types.ts:932`, `registration.rs:458`). An
/// absent or empty `includeTools` means "everything allowed"; `excludeTools` is the same selector
/// rule, negated. JS `&&` short-circuits, so a tool the allowlist rejects never touches the memo
/// table — and neither does it here.
fn is_tool_allowed(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    include_tools: Option<&[String]>,
    exclude_tools: Option<&[String]>,
    mut index: Option<&mut CandidateIndex>,
) -> bool {
    let included = match include_tools.filter(|p| !p.is_empty()) {
        None => true,
        Some(patterns) => {
            matches_tool_selector(tool_name, server_name, prefix, patterns, index.as_deref_mut())
        }
    };
    if !included {
        return false;
    }
    match exclude_tools.filter(|p| !p.is_empty()) {
        None => true,
        Some(patterns) => !matches_tool_selector(tool_name, server_name, prefix, patterns, index),
    }
}

/// `resolveToolPrefix` (`types.ts:702`, `registration.rs:243`) — the per-server override if it
/// parses, else the global.
fn effective_tool_prefix(definition: &ServerEntry, global: ToolPrefix) -> ToolPrefix {
    definition.tool_prefix.as_deref().and_then(parse_tool_prefix).unwrap_or(global)
}

/// `hasToolFilters` (`direct-tools.ts:153`, `registration.rs:1062`) — whether this server needs the
/// cross-server index at all. Upstream builds it lazily and so does this: the answer is identical
/// either way, but the index is O(servers x tools) and most servers carry no filters.
fn has_tool_filters(definition: &ServerEntry) -> bool {
    definition.include_tools.as_ref().is_some_and(|v| !v.is_empty())
        || definition.exclude_tools.as_ref().is_some_and(|v| !v.is_empty())
}

/// The `selectorCandidateIndex` builder (`direct-tools.ts:156-174`,
/// `cyrup_mcp::registration::build_candidate_index` at `registration.rs:1071`): every **current**
/// candidate name of every server with a valid cache entry — tools, plus resources unless
/// `exposeResources: false`.
///
/// The two `filter(|n| !n.is_empty())` guards, and the resource `uri` guard, are this reader's own
/// emission guards from [`resolve_direct_tool_names`], repeated here so the index holds exactly the
/// names this reader can emit. Upstream and the writer read `tool.name` / `resource.name` unguarded
/// and check no `uri` at all; see divergence 5 below.
fn build_candidate_index(
    config: &McpConfig,
    cache: &MetadataCache,
    global_prefix: ToolPrefix,
) -> CandidateIndex {
    let mut all_current = HashSet::new();
    for (other_name, other_definition) in &config.mcp_servers {
        let Some(entry) = cache.servers.get(other_name) else {
            continue;
        };
        if !is_server_cache_valid(entry, other_definition) {
            continue;
        }
        let other_prefix = effective_tool_prefix(other_definition, global_prefix);
        for tool in entry.tools.iter().flatten() {
            let Some(name) = tool.name.as_deref().filter(|n| !n.is_empty()) else {
                continue;
            };
            all_current.extend(tool_name_candidates(name, other_name, other_prefix, false));
        }
        if other_definition.expose_resources == Some(false) {
            continue;
        }
        for resource in entry.resources.iter().flatten() {
            let (Some(name), Some(_uri)) = (
                resource.name.as_deref().filter(|n| !n.is_empty()),
                resource.uri.as_deref().filter(|u| !u.is_empty()),
            ) else {
                continue;
            };
            let base = format!("read_{}", resource_name_to_tool_name(name));
            all_current.extend(tool_name_candidates(&base, other_name, other_prefix, false));
        }
    }
    CandidateIndex { all_current, matching_count: HashMap::new() }
}
```

### Step 5 — wire it into `resolve_direct_tool_names`

In `resolve_direct_tool_names` (`mcp_direct_tools.rs:589-668`), inside the per-server loop and after
the cache-validity and selection guards (i.e. after `mcp_direct_tools.rs:611-613`), mirror
`registration.rs:1139-1143`:

```rust
        let effective_prefix = effective_tool_prefix(definition, prefix);
        let mut index =
            has_tool_filters(definition).then(|| build_candidate_index(config, cache, prefix));
        let include = definition.include_tools.as_deref();
        let exclude = definition.exclude_tools.as_deref();
```

`build_candidate_index` takes the **global** prefix and resolves each server's effective prefix
internally, exactly as `registration.rs:1141` passes `global_prefix`.

Replace the `is_tool_excluded` call at `mcp_direct_tools.rs:621` with:

```rust
            if !is_tool_allowed(
                tool_name,
                server_name,
                effective_prefix,
                include,
                exclude,
                index.as_mut(),
            ) {
                continue;
            }
```

and the one at `mcp_direct_tools.rs:654-655` with the same call over `&base_name`. Both emission
sites — `mcp_direct_tools.rs:624` (tools) and `mcp_direct_tools.rs:658` (resources) — switch from
`prefix` to `effective_prefix` in their `format_tool_name` call. The `is_builtin_name` /
`seen_names` guards that follow are already in the writer's order (`registration.rs:1169-1183`) and
do not move.

### Step 6 — the assertions that this change makes false

**Doc comments.** Four state that this half is unported: `mcp_direct_tools.rs:267-272`
(`include_tools` "deliberately **not** applied by this resolver"), `:1087-1089`
(`sanitize_server_prefix`, "the legacy grammar … is not ported", and its now-dangling
``[`is_tool_excluded`]`` intra-doc link — `rustdoc::broken_intra_doc_links` is `deny` at workspace
`Cargo.toml:107`, so this one is a **build break**, not a stale sentence), `:1164-1168`
(`is_tool_excluded`'s "stay unported"), and the module-header MCP-370 bullet at `:43-44`. While
editing `:1075` and `:1164`, fix two upstream citations that are off by a couple of lines:
`sanitizeServerPrefix` is `types.ts:668` (not `:667`) and `getToolNameCandidates` is `types.ts:777`
(not `:775`).

**One test expectation genuinely changes.**
`a_cache_written_by_cyrup_mcp_resolves_through_this_reader` (`mcp_direct_tools.rs:2475-2545`) builds
its fixture with `"includeTools": ["click", "navigate"]` (`:2483`) and asserts **both**
`browser-mcp_click` and `browser-mcp_read_console_logs` resolve (`:2528-2543`). Once `includeTools`
is applied, the resource tool's current candidate set is
`{read_console_logs, browser-mcp_read_console_logs, browser_read_console_logs,
mcp__browser-mcp_read_console_logs}`, and its legacy set adds only `_`-folded variants — none of
which is `click` or `navigate` — so it is now correctly filtered out. The assertions must drop the
resource row, and the comment at `:2477-2478` ("this resolver does not yet FILTER on it") must say
the opposite. **This is the visible proof the change works**; do not weaken the fixture to keep the
old expectation.

`includes_resource_tools_and_respects_exclude_tools` (`mcp_direct_tools.rs:1735-1763`) still passes
unchanged: its `excludeTools: ["browser_click"]` matches the `short`-mode **current** candidate
`browser_click` under the new rule as it did under the old one. Only its comment at `:1755-1756`
needs rewording — it names `normalize_tool_name`, which no longer exists.

The nine other `includeTools`/`excludeTools` occurrences in the test module (`:1887`, `:1964`,
`:1998`, `:2203`, `:2239-2240`, `:2372`, `:2685-2686`, and their expected pre-image strings) are all
`configHash` pre-image vectors — they never call `resolve`, and the leniency added in Step 1 does not
change how a well-typed value hashes, so none of them moves.

## Why the reader duplicates the writer instead of calling it

[Cargo.toml](../../crates/cyrup-ext-subagents/Cargo.toml) carries `cyrup-mcp = { workspace = true }`
at `Cargo.toml:142`, inside `[dev-dependencies]` (which opens at `Cargo.toml:128`) and **not** in
`[dependencies]` (`Cargo.toml:23`). The comment at `Cargo.toml:129-141` states the reason and is the
constraint on this task: resolving a subagent's `mcp:` selectors must not drag rmcp, reqwest and
oauth2 into a spawn. So the two implementations of this contract stay separate, and the dev-only
seam — already used at `mcp_direct_tools.rs:1847-1848` — is what keeps them from drifting. Do not
promote `cyrup-mcp` to a real dependency, and do not add `regex`.

## Divergences found while reading, deliberately NOT closed here

Each is a real reader/writer disagreement of the same class, and each is outside MCP-370's filter
half.

1. **`disabled`.** The writer skips a disabled server (`config.rs:899`, `config.rs:906-908`,
   `registration.rs:1127-1129`, and inside the index at `registration.rs:1076`); the reader's
   `ServerEntry` has no such field, so an `mcp:` selector naming a disabled server still resolves
   its tools — and its candidates still enter the index built above.
2. **`uiVisibility`.** The writer drops a tool not visible to the model (`registration.rs:744-752`,
   `registration.rs:1153-1155`, and in the index at `registration.rs:1083`); the reader's
   `CachedTool` (`mcp_direct_tools.rs:315-318`) carries only `name`.
3. **Iteration order.** The reader's `mcp_servers` is a `BTreeMap` (`mcp_direct_tools.rs:308`,
   alphabetical); the writer's is an `IndexMap` in file order (`config.rs:629-638`), and that order
   decides which of two colliding tools wins the `seen_names` race. Equal *sets*, potentially
   different *first-wins*.
4. **Entry-level leniency.** `extract_server_map` (`mcp_direct_tools.rs:506-518`) drops a whole
   server when *any* field is wrong-typed, and `validate_config` (`mcp_direct_tools.rs:497-501`)
   drops the whole `settings` block for one wrong-typed key; the writer's `lenient` degrades the
   field (`config.rs:479-486`). Step 1 fixes this for the three fields this task touches, and no
   others.
5. **The resource `uri` guard.** The reader requires a non-empty `uri` before emitting a resource
   tool (`mcp_direct_tools.rs:640-647`); upstream (`direct-tools.ts:203`) and the writer
   (`registration.rs:1195-1197`) check only the name. `build_candidate_index` above repeats the
   reader's guard so index and emission agree with each other; closing the gap against the writer is
   a separate change.

## Definition of done

* `includeTools` has a read site in
  [mcp_direct_tools.rs](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs): absent or
  empty admits everything, a non-empty value admits only what `matches_tool_selector` accepts, and
  it is evaluated before `excludeTools` and short-circuits on rejection.
* Both filters accept `*` and `?` under `globToRegExp`'s grammar, match the current **and** legacy
  candidate sets, and apply the `CandidateIndex` disambiguation — so for `click` on `browser-mcp`,
  `browser_mcp_click`, `browser_2d_mcp_click` and `browser*` now exclude where they did not, and
  `browser-click` no longer excludes where the writer keeps.
* `ServerEntry` carries `toolPrefix`, parsed strictly, and both emission sites format with the
  per-server effective prefix rather than the global one. No identity-hash key changed.
* `normalize_tool_name` and `is_tool_excluded` no longer exist in the crate —
  `grep -rn "normalize_tool_name\|is_tool_excluded" crates/cyrup-ext-subagents/` is empty.
* No `regex` dependency was added, and `cyrup-mcp` is still absent from that crate's
  `[dependencies]`.
* `cargo clippy --workspace --all-targets` is clean — in particular no `indexing_slicing`,
  `unwrap_used`, `expect_used` or `panic` in the new matchers, and no `broken_intra_doc_links` from
  the removed `is_tool_excluded`.
* `a_cache_written_by_cyrup_mcp_resolves_through_this_reader` asserts the **filtered** result
  (`browser-mcp_click` alone), `includes_resource_tools_and_respects_exclude_tools` still passes
  unchanged, and `cargo nextest run --workspace` shows no regression against the 7862-passing
  baseline.

Bookkeeping afterwards, not a deliverable of this task: the `MCP-370` rows in
[13-cyrup-mcp-STATUS.md](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) (`:48`, `:103-104`, `:400`,
`:897`) and the unit's spec entry in
[13h-mcp-tui.md](../../docs/gap-analysis/13h-mcp-tui.md) (`:1374-1400`) still describe the filter
half as open, with divergences 1-5 above as what remains.
