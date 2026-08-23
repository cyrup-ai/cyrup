---
stage: aug
status: done
updated: 2026-08-22 15:09
---

# MCP-370: Port `includeTools` And Glob `excludeTools` Into The In-Tree Reader

## The finding that reframes this task

**The writer already implements all of it.** This is a one-sided port into the reader, not a design
job — every semantic below is transcribed from working code in
[registration.rs](../../crates/cyrup-mcp/src/registration.rs), not invented here.

**And the reader's rule is not a *subset* of the writer's — it is a *different* rule.** The task
said the reader "over-approximates". It does, on `includeTools` (unapplied entirely) — but on
`excludeTools` it also **under-approximates**, because `is_tool_excluded`
(`mcp_direct_tools.rs:1155-1181`) normalises `-` → `_` on **both** the candidates *and the user's
pattern*, via `normalize_tool_name` (`mcp_direct_tools.rs:1183-1185`). The writer never touches the
pattern: it compares patterns against an explicit *legacy candidate set*
(`registration.rs:277-300`) guarded by a cross-server disambiguation index
(`registration.rs:369-423`). Concretely, for tool `click` on server `browser-mcp`:

| `excludeTools` entry | reader today | writer | why |
| --- | --- | --- | --- |
| `browser_click` | excluded | excluded | current `short`-mode candidate — the two agree |
| `browser-click` | **excluded** | **kept** | reader normalises the *pattern*; the writer has no such candidate |
| `browser_mcp_click` | **kept** | **excluded** | legacy candidate (`format_legacy_tool_name`) the reader has no notion of |
| `browser*` | **kept** | **excluded** | glob, unsupported by the reader |

So `normalize_tool_name` must be **deleted**, not kept alongside the new code. Porting the filter
without removing it leaves a third rule that is neither side's.

## What each side does now, exactly

### Writer — `cyrup-mcp` (complete)

| Concern | Symbol | Site |
| --- | --- | --- |
| glob → regex | `glob_to_regex` | `registration.rs:310-326` |
| is this a glob? | `is_glob` | `registration.rs:328-330` |
| pattern vs candidate set | `matches_tool_pattern` | `registration.rs:336-360` |
| candidate names (current + legacy) | `tool_name_candidates` | `registration.rs:277-300` |
| legacy naming grammar | `legacy_server_prefix` / `format_legacy_tool_name` | `registration.rs:248-268` |
| cross-server disambiguation | `CandidateIndex` / `has_other_current_match` | `registration.rs:369-423` |
| the three-step selector rule | `matches_tool_selector` | `registration.rs:425-453` |
| `include && !exclude` | `is_tool_allowed` | `registration.rs:458-481` |
| index built lazily, only when filtered | `has_tool_filters` / `build_candidate_index` | `registration.rs:1062-1069` / `1071-1097` |
| applied to tools and to resources | `resolve_direct_tools` | `registration.rs:1159-1166`, `1200-1207` |

The config fields are typed and lenient in [config.rs](../../crates/cyrup-mcp/src/config.rs):
`include_tools` `config.rs:850`, `exclude_tools` `config.rs:853`, per-server `tool_prefix`
`config.rs:847`, `ToolPrefix` `config.rs:1316-1325`.

### Reader — `cyrup-ext-subagents` (filter half missing)

In [mcp_direct_tools.rs](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs),
`include_tools` / `exclude_tools` are deserialised (`mcp_direct_tools.rs:267-275`) and hashed into
the 15-key identity pre-image (`mcp_direct_tools.rs:840-841`) — but only `exclude_tools` is ever
*read*, through the 5-candidate exact matcher at `mcp_direct_tools.rs:1155`, called from
`mcp_direct_tools.rs:621` (tools) and `mcp_direct_tools.rs:654` (resources). `includeTools` has no
read site at all. The reader's own doc comments already name this as the open half
(`mcp_direct_tools.rs:267-272`, `mcp_direct_tools.rs:1087-1089`, `mcp_direct_tools.rs:1164-1168`);
this change makes them false and must rewrite them.

## The writer's exact semantics, to be reproduced rather than approximated

**1 — glob grammar** (`registration.rs:310-330`). A pattern is a glob iff it contains `*` or `?`.
A glob becomes the anchored regex `^…$` with `. + ^ $ { } ( ) | [ ] \` escaped, `*` → `.*` and
`?` → `.`; everything else is literal. A non-glob pattern is an **exact set-membership test** — it
is never regex-compiled, so a candidate holding a literal `*` can never be matched by a `*`-bearing
pattern.

**2 — candidate names** (`registration.rs:277-300`). `tool_name_candidates(tool, server, prefix,
include_legacy)`:

* current (`include_legacy = false`), 5 expressions: the bare `tool`; `format_tool_name` at the
  effective prefix; and `format_tool_name` at each of `Server`, `Short`, `Mcp`.
* legacy (`include_legacy = true`) adds 13 more, built from `tool.replace('-', "_")`,
  `format_legacy_tool_name` (which folds `.` **and** `-` in the tool name and escapes `-` out of the
  server prefix via `sanitize_server_prefix(_, false)`), and
  `format_tool_name(...).replace('-', "_")` — each at the effective prefix plus the same three
  modes. Heavy overlap: the resulting set is far smaller than 18.

**3 — the selector rule** (`registration.rs:425-453`), three steps in order:

1. any **current** candidate matches a pattern → `true`;
2. no index supplied → fall back to matching the **full legacy** set;
3. otherwise a pattern wins only if it matches a **legacy-only** candidate **and** does not name
   some *other* tool's current candidate (`has_other_current_match`, `registration.rs:384-423`).

**4 — `isToolAllowed`** (`registration.rs:458-481`): an absent-or-empty `includeTools` includes
everything, otherwise the selector rule decides; then `excludeTools`, same rule, negated. `include`
is evaluated **first** and short-circuits.

**5 — the index** (`registration.rs:1071-1097`): the *current* candidate names of **every** enabled
server with a valid cache entry — tools, and (unless `exposeResources: false`) resources. The
filtered server's own candidates are **not** subtracted at build time; the subtraction happens by
match count inside `has_other_current_match`. Built **lazily**, only when the server carries a
non-empty `includeTools` or `excludeTools` (`registration.rs:1062-1069`, `registration.rs:1140-1141`).

## Prescription

All edits land in
[mcp_direct_tools.rs](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs). The crate has
**no `regex` dependency** — deliberately, and stated as a rule in its own modules
(`exec/task_intent.rs:33-38`, `exec/output.rs:442-444`) — so do **not** add one; reproduce
`glob_to_regex` as a matcher. `HashMap`/`HashSet` are already imported at `mcp_direct_tools.rs:104`.

### Step 1 — prerequisite: per-server `toolPrefix`

The reader resolves one global prefix (`mcp_direct_tools.rs:427`) and has no per-server override;
the writer's `resolve_tool_prefix` (`registration.rs:243-245`) does. This matters twice: the emitted
name is `format_tool_name(tool, server, effective_prefix)`, and the `ToolPrefix::None` corner of the
candidate set differs (`format_tool_name(tool, server, None)` is `tool.replace('.', "_")`, which the
three-mode arms never produce). `toolPrefix` is **not** one of the fifteen identity keys
(`mcp_direct_tools.rs:815-842`), so adding it moves no digest.

Add to `ServerEntry` (`mcp_direct_tools.rs:229-276`):

```rust
    /// `resolveToolPrefix` (`cyrup_mcp::registration::resolve_tool_prefix`) — a per-server override
    /// of `settings.toolPrefix`. Held as a raw string and parsed by [`parse_tool_prefix`] so an
    /// unrecognised value degrades to "inherit the global", which is what the writer's `lenient`
    /// `Option<ToolPrefix>` does — and is NOT what [`get_tool_prefix`]'s catch-all does.
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

```rust
/// `cyrup_mcp::config::lenient` for a string field: a wrong-typed value becomes `None` rather than
/// failing the whole `ServerEntry`. `extract_server_map` drops an entry that fails to deserialize;
/// the writer never drops one for this reason.
fn lenient_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Value>::deserialize(deserializer)
        .ok()
        .flatten()
        .and_then(|value| value.as_str().map(str::to_string)))
}

/// [`lenient_string`] for `string[]`. A non-array, or an array with a non-string member, is `None`
/// — `cyrup_mcp::config::lenient` over `Option<Vec<String>>`.
fn lenient_string_list<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Value>::deserialize(deserializer)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value::<Vec<String>>(value).ok()))
}

/// A `toolPrefix` value parsed **strictly**: an unrecognised value is `None`.
///
/// [`get_tool_prefix`] (`mcp_direct_tools.rs:1066`) keeps its catch-all-to-`Server` behaviour for
/// the *global* settings key, because that is `getServerPrefix`'s final `return`. A **per-server**
/// value must instead fall through to the global, which is what the writer's `lenient`
/// `Option<ToolPrefix>` produces (`config.rs:847`).
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

`sanitize_server_prefix` (`mcp_direct_tools.rs:1090-1104`) is hard-wired to
`preserve_provider_valid = true`, and its doc comment (`mcp_direct_tools.rs:1087-1089`) says the
legacy grammar "is not ported". Port it — one signature change plus two functions, transcribed from
`registration.rs:184-201` and `registration.rs:248-268`:

```rust
/// Port of `sanitizeServerPrefix` (`types.ts:667`) — the twin of
/// `cyrup_mcp::registration::sanitize_server_prefix` (`registration.rs:184`).
///
/// `preserve_provider_valid = true` (every naming call site) keeps `-`; `false` is the **legacy**
/// grammar, which escapes it — `github-mcp` becomes `github_2d_mcp`. The legacy form exists only to
/// build the alias candidates of `getToolNameCandidates`.
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

/// `getLegacyServerPrefix` (`registration.rs:248`) — the same four modes over the pre-`-`/`_`
/// grammar.
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

/// `formatLegacyToolName` (`registration.rs:261`) — here the tool name loses **hyphens as well as
/// dots**, which is the one place the two grammars differ on the tool half of the name.
fn format_legacy_tool_name(tool_name: &str, server_name: &str, prefix: ToolPrefix) -> String {
    let server_prefix = legacy_server_prefix(server_name, prefix);
    let sanitized: String =
        tool_name.chars().map(|c| if c == '.' || c == '-' { '_' } else { c }).collect();
    if server_prefix.is_empty() { sanitized } else { format!("{server_prefix}_{sanitized}") }
}
```

Pass `true` at the three existing `sanitize_server_prefix(...)` call sites inside `get_server_prefix`
(`mcp_direct_tools.rs:1106-1119`). `strip_mcp_suffix` (`mcp_direct_tools.rs:1126-1135`) and
`format_tool_name` (`mcp_direct_tools.rs:1145-1153`) already match `registration.rs:205-213` and
`registration.rs:235-239`; they need no change.

### Step 3 — candidates, globs, index, `isToolAllowed`

Replace `is_tool_excluded` (`mcp_direct_tools.rs:1155-1181`) and delete `normalize_tool_name`
(`mcp_direct_tools.rs:1183-1185`) outright.

```rust
/// `getToolNameCandidates` (`types.ts:775`) — the twin of
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

/// `isGlob` (`registration.rs:328`).
fn is_glob(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

/// `globToRegExp(pattern).test(candidate)` (`registration.rs:310-326`) **without a regex engine** —
/// this crate has no `regex` dependency and does not acquire one for a two-metacharacter grammar.
///
/// That function builds `^…$` with `[.+^${}()|[\]\\]` escaped, `*` → `.*` and `?` → `.`; every other
/// character is literal. The equivalent matcher is the classic greedy wildcard walk with **one**
/// extra rule: regex `.` matches no newline in either JS or Rust, so neither `?` nor `*` may cross
/// one. (A tool name containing a newline is pathological; the arm is here so this is an equivalence
/// rather than an approximation.)
///
/// `glob_to_regex` answers `None` — "matches nothing" — when `Regex::new` rejects the pattern. With
/// that escape set the produced pattern is always syntactically valid, so the only reachable `None`
/// is the regex crate's size limit on a pathologically long pattern; this matcher has no failure
/// mode and needs none.
fn glob_matches(pattern: &str, candidate: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let hay: Vec<char> = candidate.chars().collect();
    let (mut p, mut h) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut consumed = 0usize;
    while h < hay.len() {
        match pat.get(p) {
            Some('*') => {
                star = Some(p);
                p += 1;
                consumed = h;
            }
            Some('?') if hay[h] != '\n' => {
                p += 1;
                h += 1;
            }
            Some(&c) if c != '?' && c == hay[h] => {
                p += 1;
                h += 1;
            }
            // Backtrack: let the last `*` swallow one more character. `.` cannot match `\n`, so
            // `.*` cannot cross one either — a newline ends every extension.
            _ => match star {
                Some(s) if hay[consumed] != '\n' => {
                    p = s + 1;
                    consumed += 1;
                    h = consumed;
                }
                _ => return false,
            },
        }
    }
    pat[p..].iter().all(|&c| c == '*')
}

/// `matchesToolPattern` (`registration.rs:336`): an exact membership test for a literal pattern, a
/// glob test over every candidate otherwise. A literal pattern is **never** glob-matched, and a glob
/// pattern is never compared literally.
fn matches_tool_pattern(candidates: &HashSet<String>, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| {
        if is_glob(pattern) {
            candidates.iter().any(|candidate| glob_matches(pattern, candidate))
        } else {
            candidates.contains(pattern)
        }
    })
}

/// `ToolSelectorCandidateIndex` (`registration.rs:369`) — the *current* candidate names of every
/// server with a valid cache entry, plus upstream's match-count memo. The writer also memoises the
/// compiled regex; nothing is compiled here, so only the count table is carried.
///
/// The index is built over **all** servers, the one being filtered included — the subtraction is by
/// match count in [`CandidateIndex::has_other_current_match`], not at build time.
#[derive(Debug, Default)]
struct CandidateIndex {
    all_current: HashSet<String>,
    matching_count: HashMap<String, usize>,
}

impl CandidateIndex {
    /// `indexHasOtherCurrentMatch` (`registration.rs:384`) — does `pattern` name a *different*
    /// tool's current name?
    fn has_other_current_match(
        &mut self,
        current_candidates: &HashSet<String>,
        pattern: &str,
    ) -> bool {
        if !is_glob(pattern) {
            return self.all_current.contains(pattern) && !current_candidates.contains(pattern);
        }
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

/// `matchesToolSelector` (`registration.rs:425`), the three-step disambiguation rule. Step 3 is what
/// stops a legacy alias from silently filtering the wrong tool once two servers exist whose
/// sanitized prefixes collide.
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

/// `isToolAllowed` = `isToolIncluded && !isToolExcluded` (`registration.rs:458`). An absent or empty
/// `includeTools` means "everything allowed"; `excludeTools` is the same selector rule, negated.
/// `include` short-circuits, so a tool the allowlist rejects never touches the memo table.
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

/// `getOtherCurrentCandidates` (`registration.rs:1071`): every **current** candidate name of every
/// server with a valid cache entry — tools, plus resources unless `exposeResources: false`.
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
            let Some(name) = resource.name.as_deref().filter(|n| !n.is_empty()) else {
                continue;
            };
            let base = format!("read_{}", resource_name_to_tool_name(name));
            all_current.extend(tool_name_candidates(&base, other_name, other_prefix, false));
        }
    }
    CandidateIndex { all_current, matching_count: HashMap::new() }
}

/// `resolveToolPrefix` (`registration.rs:243`) — the per-server override if it parses, else the
/// global.
fn effective_tool_prefix(definition: &ServerEntry, global: ToolPrefix) -> ToolPrefix {
    definition.tool_prefix.as_deref().and_then(parse_tool_prefix).unwrap_or(global)
}

/// `hasToolFilters` (`registration.rs:1062`) — whether this server needs the cross-server index at
/// all. Upstream builds it lazily and so does this: the answer is identical either way, but the
/// index is O(servers × tools) and most servers carry no filters.
fn has_tool_filters(definition: &ServerEntry) -> bool {
    definition.include_tools.as_ref().is_some_and(|v| !v.is_empty())
        || definition.exclude_tools.as_ref().is_some_and(|v| !v.is_empty())
}
```

### Step 4 — wire it into `resolve_direct_tool_names`

In `resolve_direct_tool_names` (`mcp_direct_tools.rs:589-667`), inside the per-server loop and after
the cache-validity and selection guards, mirror `registration.rs:1139-1143`:

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

and the one at `mcp_direct_tools.rs:654` with the same call over `&base_name`. Both emission sites —
`mcp_direct_tools.rs:624` (tools) and `mcp_direct_tools.rs:658` (resources) — switch from `prefix`
to `effective_prefix` in their `format_tool_name` call. The `is_builtin_name` / `seen_names` guards
that follow are already in the writer's order (`registration.rs:1169-1183`) and do not move.

### Step 5 — the comments that assert the gap

Three doc comments state that this half is unported and become false:
`mcp_direct_tools.rs:267-272` (`include_tools` "deliberately not applied by this resolver"),
`mcp_direct_tools.rs:1087-1089` (`sanitize_server_prefix`, "the legacy grammar is not ported"), and
`mcp_direct_tools.rs:1164-1168` (`is_tool_excluded`'s "stay unported"). The module header's MCP-370
bullet (`mcp_direct_tools.rs:43-44`) should state the filter half as closed.

## Checking parity against the writer rather than against a constant

[Cargo.toml](../../crates/cyrup-ext-subagents/Cargo.toml) carries `cyrup-mcp = { workspace = true }`
in **`[dev-dependencies]` only** — verified: `Cargo.toml:114` opens the section,
`Cargo.toml:128` is the entry, and `[dependencies]` (`Cargo.toml:23`) does not name it. The comment
above the entry states the seam's purpose, and `mod tests` already reaches through it
(`mcp_direct_tools.rs:1878-1881`). That makes the parity claim a direct equality instead of a
hand-maintained expectation. Both sides read one fixture directory:

* reader — `resolve_mcp_direct_tool_names_in(&selectors, cwd, &dirs)` (`mcp_direct_tools.rs:415`);
* writer — `cyrup_mcp::registration::resolve_direct_tools(&config, cache.as_ref(),
  config.tool_prefix(), Some(&selectors))` (`registration.rs:1111`), with `config` from
  `cyrup_mcp::config::load_mcp_config(&writer_dirs, None)` (`config.rs:3068`), `cache` from
  `cyrup_mcp::registration::load_metadata_cache(&writer_dirs)` (`registration.rs:830`), and
  `writer_dirs` from `cyrup_mcp::dirs::McpDirs::new(agent_dir, cwd)` (`dirs.rs:137`, `dirs.rs:147`).

The writer's `env_override` argument is the counterpart of the reader's `mcp:` selector list:
`parse_direct_tool_selectors` (`registration.rs:910`) and `parse_selections`
(`mcp_direct_tools.rs:676`) are the same function, and `resolve_tool_filter`
(`registration.rs:953-976`) makes an env selection outrank `directTools` entirely — so passing
`Some(&selectors)` puts both sides on the same selection. Compare the reader's `Vec<String>` against
`specs.iter().map(|s| &s.prefixed_name)` **sorted**, for the ordering reason immediately below.

## Divergences found while reading, deliberately NOT closed here

Each is a real reader/writer disagreement of the same class, and each is outside MCP-370's filter
half. A parity fixture must avoid them or it fails for the wrong reason.

1. **`disabled`.** The writer skips a disabled server (`config.rs:899`, `config.rs:906-908`,
   `registration.rs:1127-1129`); the reader's `ServerEntry` has no such field, so an `mcp:` selector
   naming a disabled server still resolves its tools.
2. **`uiVisibility`.** The writer drops a tool not visible to the model (`registration.rs:744-752`,
   `registration.rs:1153-1155`, and in the index at `registration.rs:1083`); the reader's
   `CachedTool` (`mcp_direct_tools.rs:315-320`) carries only `name`.
3. **Iteration order.** The reader's `mcp_servers` is a `BTreeMap` (`mcp_direct_tools.rs:308`,
   alphabetical); the writer's is an `IndexMap` in file order (`config.rs:628-637`), and that order
   decides which of two colliding tools wins the `seen_names` race. Equal *sets*, potentially
   different *first-wins*.
4. **Entry-level leniency.** `extract_server_map` (`mcp_direct_tools.rs:506-518`) drops a whole
   server when *any* field is wrong-typed; the writer's `lenient` degrades the field
   (`config.rs:771-780`). Step 1 fixes this for the three fields this task touches, and no others.

## Definition of done

* For any `mcp.json` + `mcp-cache.json` pair whose servers set neither `disabled` nor
  `uiVisibility`, the reader's resolved name list and the writer's `prefixed_name` list are equal as
  sets for the same selector list — including definitions using `includeTools`, `excludeTools`,
  globs in either, both together, and a per-server `toolPrefix`.
* `includeTools` has a read site in the reader: absent-or-empty admits everything, a non-empty value
  admits only what `matches_tool_selector` accepts, and it is evaluated before `excludeTools`.
* `excludeTools` accepts `*` and `?` under `glob_to_regex`'s grammar, matches the current **and**
  legacy candidate sets, and applies the `CandidateIndex` disambiguation — so `browser_mcp_click`
  and `browser*` now exclude where they did not, and `browser-click` no longer excludes where the
  writer keeps.
* `normalize_tool_name` and `is_tool_excluded` no longer exist in
  [mcp_direct_tools.rs](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs).
* `cyrup-mcp` stays out of that crate's `[dependencies]`, and no `regex` dependency is added.
* The `MCP-370` rows in
  [13-cyrup-mcp-STATUS.md](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) (`:48`, `:103-104`,
  `:400`, `:897`) read `implemented`, with the four divergences above recorded there as what stays
  open.
