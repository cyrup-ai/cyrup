---
stage: qa
status: completed
updated: 2026-08-27 08:00
---

# One Tool-Name Grammar: MCP-073 + MCP-075 + MCP-076

## COMPLETED — QA 10/10

MCP-075/076/073 landed. `proxy/tool_metadata.rs` went 420 to 103 lines: the duplicate
tool-name grammar is deleted and re-exported from `registration.rs`, so one definition
serves all six call sites. The double-sanitisation that mangled every `mcp__` server name
and then FAILED OPEN at the `excludeTools` arm is gone. `glob_to_regex` now builds through
`RegexBuilder` with size and DFA limits. `resolve_server_from_tool_name` matches all 13
upstream vectors. Candidate sets moved to `IndexSet` because `proxy/approval.rs` reads
insertion order.

QA found one defect, since fixed: the collapse dropped the 13e integration instruction
telling a future implementer to replace the remaining `ToolMetadata` items with
`pub use crate::renderers::{…}`. Restored in the module header.

Left undone deliberately: the task's "zero `Regex::new` hits" criterion is unattainable —
ten remain, all compile-time literals unreachable from untrusted input. Flagged, not fixed.
One accessor was added beyond the ten prescribed edits so an existing collision test could
keep asserting real content.

Gates: check/clippy/doc clean, 7870/7870 tests.

---

## Objective

`crates/cyrup-mcp` carries **four drifted copies** of `types.ts`'s tool-name grammar. Collapse them
onto one upstream-faithful implementation in
[`registration.rs`](../../crates/cyrup-mcp/src/registration.rs), which fixes the two divergences the
collapse exposes — **MCP-075**, a legacy server prefix that is escaped twice, and **MCP-076**, a glob
compiler with no resource ceiling — and add the one function of that grammar the port never wrote,
**MCP-073** `resolveServerFromToolName`.

This is the whole of the `high`-severity residue that no sibling task in `.flux/todo/` owns. The
other eighteen residual units are routed at the end, with the correction that makes each routing
true.

---

# Part 1 — The verification pass

The plan this file used to hold was written on 2026-08-22 15:11 and batched 40 units into nine
waves. Re-read against the tree today, **three of its premises are false** and one is now redundant.
The corrections are the substance of this rewrite.

## Correction 1 — `proxy.rs` does not exist

`ba75bbf refactor(mcp): decompose proxy.rs into a proxy/ module` (PR #69, landed 2026-08-27) split
the 7,594-line `proxy.rs` into
[`crates/cyrup-mcp/src/proxy/`](../../crates/cyrup-mcp/src/proxy/) — 14 files, with
[`proxy/mod.rs`](../../crates/cyrup-mcp/src/proxy/mod.rs) glob re-exporting every submodule so every
`crate::proxy::X` path still resolves.

**Every `proxy.rs:NNNN` citation in the old plan, in the census, and in five sibling task files is
now dead.** The relocations that matter to the residue:

| old citation | symbol | lives at |
|---|---|---|
| `proxy.rs:4591` | `is_tool_call_approval_required` | [proxy/approval.rs:77](../../crates/cyrup-mcp/src/proxy/approval.rs) |
| `proxy.rs:4787` | `ensure_tool_call_approved` (free fn) | [proxy/approval.rs:272](../../crates/cyrup-mcp/src/proxy/approval.rs) |
| `proxy.rs:509-530` | `format_legacy_tool_name` | [proxy/tool_metadata.rs:158](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) |
| `proxy.rs:483` | `strip_mcp_suffix` | [proxy/tool_metadata.rs:131](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) |
| `proxy.rs:583-587` | the ceilinged `glob_to_regex` | [proxy/tool_metadata.rs:232-252](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) |
| `proxy.rs:1465` / `:1478` | `ProxyEnv::call_tool` / `read_resource` | [proxy/env.rs](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `proxy.rs:4862` | the `#[cfg(test)]` opening | now **per-file**: `description.rs:288`, `ranking.rs:459`, `approval.rs:343`, `call.rs:911`, `discovery.rs:636` |
| `proxy.rs:4932` | `FakeEnv` | [proxy/testsupport.rs:199](../../crates/cyrup-mcp/src/proxy/testsupport.rs) |
| `proxy.rs:1232-1236` | `ConnectOutcome` | [proxy/env.rs](../../crates/cyrup-mcp/src/proxy/env.rs) |

**[`proxy/tool_metadata.rs`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) has no
`#[cfg(test)]` module at all** — all 420 lines are production. That matters below: the deletion this
task prescribes has no test module to relocate.

## Correction 2 — twelve sibling tasks now own 22 of the 40 batched units

`96591f8 chore(flux): split the queue into cyrup-mcp work and a parked backlog` moved
`todo/_backlog/` to [`backlog/`](../backlog/) and left exactly 13 `MCP_*` files in
[`todo/`](.). Six of them did not exist when the old plan was written. Verified by extracting every
`MCP-\d+[a-z]?` from each file:

| old wave | units | now owned by |
|---|---|---|
| 1 · request seam | `MCP-119` | [MCP_DISCOVERY_PAGINATION.md](MCP_DISCOVERY_PAGINATION.md) |
| 1 | `MCP-135` | [MCP_SESSION_RECOVERY.md](MCP_SESSION_RECOVERY.md) |
| 1 | `MCP-164` | [MCP_RUNTIME_INIT_SPINE.md](MCP_RUNTIME_INIT_SPINE.md) |
| 2 · session start | `MCP-008`, `MCP-011` | [MCP_CONNECT_AND_OAUTH_LAST_MILE.md](MCP_CONNECT_AND_OAUTH_LAST_MILE.md) |
| 2 | `MCP-009`, `MCP-010`, `MCP-011`, `MCP-023` | [MCP_RUNTIME_INIT_SPINE.md](MCP_RUNTIME_INIT_SPINE.md) |
| 3 · tool execution | `MCP-043`, `MCP-207`, `MCP-214`, `MCP-217` | [MCP_RUNTIME_INIT_SPINE.md](MCP_RUNTIME_INIT_SPINE.md) |
| 4 · patterns/schema | `MCP-092` | [MCP_SCHEMA_AND_ERROR_TEXT.md](MCP_SCHEMA_AND_ERROR_TEXT.md) |
| 6 · cache identity | `MCP-143` | [MCP_PROCESS_LAUNCH_RESOLUTION.md](MCP_PROCESS_LAUNCH_RESOLUTION.md) |
| 6 | `MCP-144` | [MCP_CONFIG_LENIENT_TYPES.md](MCP_CONFIG_LENIENT_TYPES.md) |
| 6 | `MCP-094` | [MCP_SCHEMA_AND_ERROR_TEXT.md](MCP_SCHEMA_AND_ERROR_TEXT.md) |
| 8 · oauth | `MCP-324` | [MCP_CONNECT_AND_OAUTH_LAST_MILE.md](MCP_CONNECT_AND_OAUTH_LAST_MILE.md) |
| 9 · standalones | `MCP-068` | [MCP_SCHEMA_AND_ERROR_TEXT.md](MCP_SCHEMA_AND_ERROR_TEXT.md) |
| 9 | `MCP-260` | [MCP_UNPINNED_BEHAVIOUR_TESTS.md](MCP_UNPINNED_BEHAVIOUR_TESTS.md) |

[MCP_COMMAND_SURFACE.md](MCP_COMMAND_SURFACE.md) mentions `MCP-381` and `MCP-398` and **explicitly
refuses them**: *"MCP-381 and MCP-398 belong to Wave 7 of MCP_HIGH_SEVERITY_BACKLOG.md and are not
this task's to write"* ([MCP_COMMAND_SURFACE.md:50-52](MCP_COMMAND_SURFACE.md)). They stay in this
file's routing table, not in its payload — they are blocked behind
[MCP_RUNTIME_INIT_SPINE.md](MCP_RUNTIME_INIT_SPINE.md) by that file's own landing order.

## Correction 3 — the old plan grouped the tool-name grammar into two waves. It is one obligation

Old Wave 4 held `MCP-076` (the glob compiler); old Wave 5 held `MCP-073` + `MCP-075` (the name
grammar). They are the **same mechanism**: `glob_to_regex` exists only to be run against the output
of `tool_name_candidates`, both live in the same duplicated block, and the fix for either is the
deduplication that fixes both. Splitting them across two agents means two agents editing
`registration.rs` and `proxy/tool_metadata.rs` — the exact PR #30 failure mode the old plan spent a
section warning about.

## Correction 4 — `get_tool_name_candidates` is not missing, and `MCP-139`/`MCP-145` are not what the census says

* `MCP-075`'s row calls the unit `partial`; the tree agrees, but the *reason* has moved file. The
  buggy copy is [proxy/tool_metadata.rs:158-180](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs),
  not `proxy.rs:486`. The **correct** copy exists at
  [registration.rs:248-263](../../crates/cyrup-mcp/src/registration.rs) — which is the whole reason
  this task is a deduplication rather than a port.
* `MCP-139`'s row says the cache is `partial`. The census does not record that **there are two
  independent metadata-cache implementations in this crate**:
  [`dirs.rs`](../../crates/cyrup-mcp/src/dirs.rs) (`MetadataCache` at `:624`, `ServerCacheEntry` at
  `:567`, `load_metadata_cache` at `:644`, `save_metadata_cache` at `:669`, `is_server_cache_valid`
  at `:843`, `compute_server_hash` at `:1275`) and
  [`registration.rs`](../../crates/cyrup-mcp/src/registration.rs) (`MetadataCache` at `:612`,
  `ServerCacheEntry` at `:626`, `load_metadata_cache` at `:830`, `is_server_cache_valid` at `:860`).
  [`ui.rs:71-73`](../../crates/cyrup-mcp/src/ui.rs) imports the `dirs.rs` set; `registration.rs`'s
  own `build_candidate_index` uses the `registration.rs` set. **That duplication, not the missing
  call site, is `MCP-139`'s real content.** Routed below; it is deliberately *not* in this task's
  payload, because unifying two cache schemas and unifying the name grammar are two different jobs
  in the same two files and must not overlap.
* `MCP-140`'s serialisers exist ([dirs.rs:761/786/808](../../crates/cyrup-mcp/src/dirs.rs)) and
  `MCP-145`'s validator exists ([dirs.rs:843/850](../../crates/cyrup-mcp/src/dirs.rs)). Both rows'
  real gap is a production caller, which is `MCP-029`'s, which is the spine's.

---

# Part 2 — The obligation

## The four copies

`types.ts`'s tool-name grammar is implemented four times in this workspace. Only one of them is
upstream-faithful.

| # | location | status |
|---|---|---|
| 1 | [`registration.rs:184-478`](../../crates/cyrup-mcp/src/registration.rs) — `sanitize_server_prefix`, `strip_mcp_suffix`, `server_prefix`, `format_tool_name`, `resolve_tool_prefix`, `legacy_server_prefix`, `format_legacy_tool_name`, `tool_name_candidates`, `glob_to_regex`, `matches_tool_pattern`, `CandidateIndex`, `matches_tool_selector`, `is_tool_allowed` | **the keeper.** Correct legacy prefix, memoising `CandidateIndex`. Two defects: `glob_to_regex` has no ceiling (MCP-076), and the candidate set is a `HashSet` so upstream's insertion order is lost |
| 2 | [`proxy/tool_metadata.rs:85-394`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) — the same thirteen, re-derived | **delete.** `format_legacy_tool_name` is wrong (MCP-075); `index_has_other_current_match` is the non-memoising form its own doc-comment says to delete. The module header already prescribes this: *"This module is temporary by design… this file is deleted and its `mod`/`pub use` lines in [`crate::proxy`] are replaced by `pub use` — a delete, not a rewrite"* ([:3-6](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs)), and the file has already done exactly that twice, at [`:402`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) and [`:420`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) |
| 3 | [`ui.rs:1490-1540`](../../crates/cyrup-mcp/src/ui.rs) — `matches_tool_selector_set` / `is_tool_allowed_set` | **keep, retype.** This is `mcp-panel.ts`'s genuinely different arm (a pre-subtracted `Set`, no count comparison) and its own doc says so at [`ui.rs:1485-1487`](../../crates/cyrup-mcp/src/ui.rs). It already calls copy #1, so it inherits the fix; it only needs its container type updated |
| 4 | [`cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:1106-1185`](../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs) | **out of scope.** A three-mode, hyphen-*replacing* grammar ported from `pi-subagents`. Reconciling it is `MCP-178`/`MCP-205`, an open decision, and is owned by [MCP_DIRECT_TOOLS_FILTERS.md](MCP_DIRECT_TOOLS_FILTERS.md) |

## Divergence 1 — MCP-075: the legacy server prefix is escaped twice

Upstream, [`tmp/pi-mcp-adapter/types.ts:664-670`](../../tmp/pi-mcp-adapter/types.ts) and
[`:764-775`](../../tmp/pi-mcp-adapter/types.ts):

```ts
function sanitizeServerPrefix(serverName: string, preserveProviderValid = true): string {
  const validCharacters = preserveProviderValid ? /^[A-Za-z0-9_-]$/ : /^[A-Za-z0-9]$/;
  return Array.from(serverName, char =>
    validCharacters.test(char) ? char : `_${char.codePointAt(0)!.toString(16)}_`,
  ).join("");
}

function getLegacyServerPrefix(serverName: string, mode: ToolPrefix): string {
  if (mode === "none") return "";
  if (mode === "short") return sanitizeServerPrefix(serverName.replace(/-?mcp$/i, ""), false) || "mcp";
  if (mode === "mcp") return `mcp__${sanitizeServerPrefix(serverName, false)}`;
  return sanitizeServerPrefix(serverName, false);
}
```

The legacy prefix sanitises the **raw server name** under the strict `[A-Za-z0-9]` grammar, exactly
once, and stamps the `mcp__` literal **after** that.

[`proxy/tool_metadata.rs:158-180`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) instead takes
the *already-sanitised* permissive prefix and escapes it a second time:

```rust
// crates/cyrup-mcp/src/proxy/tool_metadata.rs:158 — WRONG, delete
fn format_legacy_tool_name(tool_name: &str, server_name: &str, prefix: ToolPrefix) -> String {
    let server_prefix = match prefix {
        ToolPrefix::None => String::new(),
        _ => {
            let base = get_server_prefix(server_name, prefix);   // <- already sanitised, and for
                                                                 //    ToolPrefix::Mcp already carries
                                                                 //    the literal `mcp__`
            let mut out = String::with_capacity(base.len());
            for ch in base.chars() {                             // <- second pass over the OUTPUT
                if ch.is_ascii_alphanumeric() { out.push(ch); }
                else { out.push('_'); out.push_str(&format!("{:x}", ch as u32)); out.push('_'); }
            }
            out
        }
    };
    ...
}
```

**Worked divergences.** Every `_` the first pass emitted — whether from an escape or from the
`mcp__` literal — is escaped again by the second.

| server | mode | upstream `getLegacyServerPrefix` | `proxy/tool_metadata.rs` | |
|---|---|---|---|---|
| `github` | `mcp` | `mcp__github` | `mcp_5f__5f_github` | ✗ |
| `my-server` | `mcp` | `mcp__my_2d_server` | `mcp_5f__5f_my_2d_server` | ✗ |
| `my server` | `server` | `my_20_server` | `my_5f_20_5f_server` | ✗ |
| `my server` | `short` | `my_20_server` | `my_5f_20_5f_server` | ✗ |
| `my-server` | `server` | `my_2d_server` | `my_2d_server` | ✓ |
| `my_server` | `server` | `my_5f_server` | `my_5f_server` | ✓ |

The two accidental agreements are why this survived review: a raw `-` or `_` round-trips correctly
through the double pass. Every other case does not, and **`ToolPrefix::Mcp` is wrong for every
server name without exception** — because
[`get_tool_name_candidates`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) computes all four
modes' legacy spellings regardless of which mode is configured
([`:215-218`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs)).

**Blast radius, and it is not symmetric.** The corrupted candidate set reaches three call sites:

* [`proxy/description.rs:186`/`:204`](../../crates/cyrup-mcp/src/proxy/description.rs) — `is_tool_allowed`'s
  `excludeTools` arm. **Fails open**: an `excludeTools` entry written in a legacy spelling does not
  match, and a tool the user excluded stays advertised to the model.
* [`proxy/approval.rs:112`](../../crates/cyrup-mcp/src/proxy/approval.rs) — `approveTools`' legacy
  tier. Fails safe (an extra prompt), but it is a consent gate and must still be right.
* [`proxy/ranking.rs:292`](../../crates/cyrup-mcp/src/proxy/ranking.rs) — `searchKeywords` selectors.

Upstream pins the correct behaviour directly, at
[`__tests__/resolve-server-from-tool-name.test.ts:181-189`](../../tmp/pi-mcp-adapter/__tests__/resolve-server-from-tool-name.test.ts):

```ts
expect(matchesToolPattern(hyphenCandidates, ["my_2d_server_do_thing"])).toBe(true);
```

## Divergence 2 — MCP-076: one of the two glob compilers has no ceiling

[`registration.rs:310-326`](../../crates/cyrup-mcp/src/registration.rs) ends in a bare
`Regex::new(&out).ok()`. [`proxy/tool_metadata.rs:247-251`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs)
and [`proxy/discovery.rs:431-434`](../../crates/cyrup-mcp/src/proxy/discovery.rs) both build through
`RegexBuilder` with `REGEX_SIZE_LIMIT` / `REGEX_DFA_SIZE_LIMIT`
([`proxy/constants.rs:75`/`:77`](../../crates/cyrup-mcp/src/proxy/constants.rs), both `1 << 20`).

`registration.rs`'s copy is the one that compiles **config-supplied** `includeTools` /
`excludeTools` / `approveTools` / `searchKeywords` globs — the untrusted-pattern site with the
widest input — and it is the only one without a ceiling. The deduplication makes this a one-line
fix, because after it there is exactly one `glob_to_regex` left.

## Gap 3 — MCP-073: `resolveServerFromToolName` was never ported

Zero hits crate-wide, confirmed. Upstream is
[`types.ts:726-748`](../../tmp/pi-mcp-adapter/types.ts):

```ts
export function resolveServerFromToolName(
  toolName: string, serverNames: Iterable<string>, prefix: ToolPrefix,
): string | undefined {
  if (prefix === "none") return undefined;
  const candidates: { name: string; prefix: string }[] = [];
  for (const name of serverNames) {
    const p = getServerPrefix(name, prefix);
    if (p && toolName.startsWith(p + "_")) candidates.push({ name, prefix: p });
  }
  if (candidates.length === 0) return undefined;
  candidates.sort((a, b) => b.prefix.length - a.prefix.length);
  const best = candidates[0];
  // Fail safe: short mode can intentionally map names such as foo and foo-mcp
  // to the same prefix. Return undefined so a downstream permission gate uses
  // its existing wildcard path rather than enforcing a rule against the wrong server.
  if (candidates.some((c) => c.prefix === best!.prefix && c.name !== best!.name)) {
    return undefined;
  }
  return best?.name;
}
```

The fail-safe compares **prefix strings**, not lengths. Two matching prefixes of equal length must
be identical strings (both are prefixes of the same tool name), so the predicate reduces to *"more
than one configured server produces the longest matching prefix"*.

**It has no production caller upstream either** — the only references are its own test file. It is
an exported helper for a downstream policy gate. In this workspace that gate is
[`cyrup-permission-system/src/manager.rs`](../../crates/cyrup-permission-system/src/manager.rs):
`push_mcp_tool_permission_targets` at `:997` falls through to `add_derived_mcp_server_targets` at
`:971`, which matches on **`ends_with("_{server}")`** — a suffix rule, the inverse of this one.
Wiring that gate is a separate cross-crate decision and is **not** in scope; ship the function and
its `lib.rs` re-export, as `MCP-073`'s row asks
([13-cyrup-mcp-STATUS.md:576](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)), and record the
mismatch in its doc comment.

## The container-type decision the deduplication forces

`registration::tool_name_candidates` returns `HashSet<String>`; `proxy`'s returns
`IndexSet<String>`. **Adopt `IndexSet`, not `HashSet`** — this is not cosmetic:

* Upstream returns a JS `Set`, which preserves insertion order.
* [`proxy/approval.rs:168-170`](../../crates/cyrup-mcp/src/proxy/approval.rs) reads that order and
  says so: `current.iter().find(|candidate| *candidate != original_name)` is upstream's
  `[...currentCandidates].find(c => c !== toolMeta.originalName)` — *"`IndexSet` iterates in
  insertion order, which is what makes 'first' mean the same thing here as in a JS `Set`."*
  Retyping to `HashSet` would make that arm non-deterministic.
* `registration.rs` already depends on `indexmap` ([`:67`](../../crates/cyrup-mcp/src/registration.rs)).

Take the ordering fidelity while the file is open: `registration.rs`'s insertion order
([`:277-303`](../../crates/cyrup-mcp/src/registration.rs)) interleaves the three legacy families,
where upstream emits them in three complete groups.

---

# Part 3 — Implementation

Ten edits, in this order. Nothing here is optional and there is no alternative path: after edit 2
there is exactly one definition of each grammar function in the crate, and the remaining edits are
the call sites that follow from it.

## 1 · `crates/cyrup-mcp/src/registration.rs` — the single grammar

**1a. Imports.** [`:58`](../../crates/cyrup-mcp/src/registration.rs),
[`:67-68`](../../crates/cyrup-mcp/src/registration.rs). Keep `HashSet` (still used by
`DirectToolSelection` at [`:900-901`](../../crates/cyrup-mcp/src/registration.rs) and by three local
`seen` sets); add `IndexSet`, `RegexBuilder`, and the two ceilings.

```rust
use std::collections::{HashMap, HashSet};
use indexmap::{IndexMap, IndexSet};
use regex::{Regex, RegexBuilder};

use crate::proxy::constants::{REGEX_DFA_SIZE_LIMIT, REGEX_SIZE_LIMIT};
```

The `registration` → `proxy::constants` edge is fine: module cycles are legal within a crate, and
`proxy` already depends on `registration` ([`proxy/tool_metadata.rs:402`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs)).
Do **not** move the constants — [`proxy/discovery.rs:431-434`](../../crates/cyrup-mcp/src/proxy/discovery.rs)
is their other consumer and it is genuinely proxy-owned (MCP-159, the model-supplied search query).
Amend the doc on [`proxy/constants.rs:71-74`](../../crates/cyrup-mcp/src/proxy/constants.rs) to name
MCP-076 alongside MCP-159.

**1b. MCP-076 — ceiling the glob compiler.** Replace the tail of `glob_to_regex`
([`:310-326`](../../crates/cyrup-mcp/src/registration.rs)); the escape loop is already correct and
is unchanged.

```rust
    out.push('$');
    RegexBuilder::new(&out)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
        .build()
        .ok()
}
```

**1c. `tool_name_candidates` → `IndexSet`, in upstream's emission order.** Replace the body at
[`:277-303`](../../crates/cyrup-mcp/src/registration.rs). Three complete groups, matching
[`types.ts:777-803`](../../tmp/pi-mcp-adapter/types.ts) statement for statement.

```rust
#[must_use]
pub fn tool_name_candidates(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    include_legacy: bool,
) -> IndexSet<String> {
    const MODES: [ToolPrefix; 3] = [ToolPrefix::Server, ToolPrefix::Short, ToolPrefix::Mcp];
    let mut out = IndexSet::new();
    out.insert(tool_name.to_string());
    out.insert(format_tool_name(tool_name, server_name, prefix));
    for mode in MODES {
        out.insert(format_tool_name(tool_name, server_name, mode));
    }
    if !include_legacy {
        return out;
    }
    // `types.ts:786` — group 1: the `-`→`_` tool name under every prefix.
    let legacy_tool_name = tool_name.replace('-', "_");
    out.insert(legacy_tool_name.clone());
    out.insert(format_tool_name(&legacy_tool_name, server_name, prefix));
    for mode in MODES {
        out.insert(format_tool_name(&legacy_tool_name, server_name, mode));
    }
    // `types.ts:792` — group 2: the pre-2.x server-prefix grammar.
    out.insert(format_legacy_tool_name(tool_name, server_name, prefix));
    for mode in MODES {
        out.insert(format_legacy_tool_name(tool_name, server_name, mode));
    }
    // `types.ts:797` — group 3: the current spellings, post-normalised.
    out.insert(format_tool_name(tool_name, server_name, prefix).replace('-', "_"));
    for mode in MODES {
        out.insert(format_tool_name(tool_name, server_name, mode).replace('-', "_"));
    }
    out
}
```

`legacy_server_prefix` ([`:248-259`](../../crates/cyrup-mcp/src/registration.rs)) and
`format_legacy_tool_name` ([`:261-267`](../../crates/cyrup-mcp/src/registration.rs)) are **already
byte-faithful to upstream and must not be touched.** They are the reference the deleted copy is
being replaced by.

**1d. Retype the three consumers of the set.** [`:336`](../../crates/cyrup-mcp/src/registration.rs),
[`:369-390`](../../crates/cyrup-mcp/src/registration.rs),
[`:425-450`](../../crates/cyrup-mcp/src/registration.rs). `HashSet<String>` → `IndexSet<String>`
throughout, and one method rename — `IndexSet` spells removal `shift_remove`:

```rust
pub fn matches_tool_pattern(candidates: &IndexSet<String>, patterns: &[String]) -> bool { … }

pub struct CandidateIndex {
    all_current: IndexSet<String>,
    matcher: HashMap<String, Option<Regex>>,
    matching_count: HashMap<String, usize>,
}

impl CandidateIndex {
    #[must_use]
    pub fn new(all_current: IndexSet<String>) -> Self { … }

    fn has_other_current_match(
        &mut self,
        current_candidates: &IndexSet<String>,
        pattern: &str,
    ) -> bool { … }
}
```

In `matches_tool_selector` ([`:447`](../../crates/cyrup-mcp/src/registration.rs)):

```rust
    let mut legacy = tool_name_candidates(tool_name, server_name, prefix, true);
    for candidate in &current {
        legacy.shift_remove(candidate);   // was `legacy.remove(candidate)`
    }
```

`is_tool_allowed` ([`:458-478`](../../crates/cyrup-mcp/src/registration.rs)) needs **no change** —
its signature already speaks `Option<&mut CandidateIndex>` and its `index.as_deref_mut()` reborrow
is the pattern the new call sites copy.

**1e. `resolve_tool_prefix` takes an `Option`.** [`:243-246`](../../crates/cyrup-mcp/src/registration.rs).
Upstream's parameter is optional (`definition?: Pick<ServerEntry, "toolPrefix">`,
[`types.ts:701-706`](../../tmp/pi-mcp-adapter/types.ts)); the proxy copy has it right and this one
does not.

```rust
/// `types.ts:704` `resolveToolPrefix(definition, globalPrefix)` — the per-server override wins.
#[must_use]
pub fn resolve_tool_prefix(definition: Option<&ServerEntry>, global: ToolPrefix) -> ToolPrefix {
    definition.and_then(|entry| entry.tool_prefix).unwrap_or(global)
}
```

Four in-file call sites become `Some(…)`: [`:1081`](../../crates/cyrup-mcp/src/registration.rs),
[`:1139`](../../crates/cyrup-mcp/src/registration.rs),
[`:1312`](../../crates/cyrup-mcp/src/registration.rs),
[`:1823`](../../crates/cyrup-mcp/src/registration.rs). Two in `ui.rs`:
[`:1660`](../../crates/cyrup-mcp/src/ui.rs), [`:1798`](../../crates/cyrup-mcp/src/ui.rs).

**1f. `build_candidate_index`.** [`:1076`](../../crates/cyrup-mcp/src/registration.rs) —
`HashSet::new()` → `IndexSet::new()`.

**1g. MCP-073 — the new function.** Place it immediately after `resolve_tool_prefix`, so the
forward grammar and its inverse are adjacent.

```rust
/// `types.ts:726` `resolveServerFromToolName(toolName, serverNames, prefix)` — the inverse of
/// [`server_prefix`] (MCP-073).
///
/// Returns the configured server whose prefix the tool name starts with, longest first, and
/// **`None` when two different server names produce that same longest prefix**. That fail-safe is
/// the point of the function: `short` mode deliberately maps `foo` and `foo-mcp` onto the same
/// prefix, and a downstream permission gate must fall back to its wildcard path rather than
/// enforce a server-scoped rule against the wrong server.
///
/// Borrowed, not owned: the answer is always one of the inputs, so a caller that needs a `String`
/// writes `.map(str::to_owned)` and one that does not pays nothing.
///
/// # The consumer is not wired, deliberately
///
/// `cyrup_permission_system::manager::add_derived_mcp_server_targets` derives MCP server targets
/// today with a **suffix** test (`tool_name.ends_with(&format!("_{server}"))`), which is a
/// different rule from this prefix one. Reconciling the two is a cross-crate decision (MCP-191)
/// and is not this unit's; upstream ships `resolveServerFromToolName` with no production caller
/// either.
#[must_use]
pub fn resolve_server_from_tool_name<'a, I>(
    tool_name: &str,
    server_names: I,
    prefix: ToolPrefix,
) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    if matches!(prefix, ToolPrefix::None) {
        return None;
    }
    // One pass instead of upstream's collect-sort-scan. `sort((a, b) => b.len - a.len)` is stable,
    // so upstream keeps the FIRST name at the winning length; taking a strictly-longer prefix only
    // reproduces that, and resets the ambiguity flag because the fail-safe is evaluated against
    // the new best.
    let mut best: Option<(&'a str, String)> = None;
    let mut ambiguous = false;
    for name in server_names {
        let candidate = server_prefix(name, prefix);
        // `if (p && toolName.startsWith(p + "_"))`.
        if candidate.is_empty() {
            continue;
        }
        let mut boundary = String::with_capacity(candidate.len() + 1);
        boundary.push_str(&candidate);
        boundary.push('_');
        if !tool_name.starts_with(&boundary) {
            continue;
        }
        match &best {
            Some((best_name, best_prefix)) if best_prefix.len() >= candidate.len() => {
                // `candidates.some(c => c.prefix === best.prefix && c.name !== best.name)`.
                if *best_prefix == candidate && *best_name != name {
                    ambiguous = true;
                }
            }
            _ => {
                best = Some((name, candidate));
                ambiguous = false;
            }
        }
    }
    if ambiguous {
        return None;
    }
    best.map(|(name, _)| name)
}
```

The upstream test file
([`__tests__/resolve-server-from-tool-name.test.ts`](../../tmp/pi-mcp-adapter/__tests__/resolve-server-from-tool-name.test.ts))
is the behavioural spec. The five cases that discriminate a correct port from a plausible one:

| input | expected | why |
|---|---|---|
| `("searxng-extra_deep_search", ["searxng","searxng-extra"], server)` | `searxng-extra` | longest prefix wins ([`:66-71`](../../tmp/pi-mcp-adapter/__tests__/resolve-server-from-tool-name.test.ts)) |
| `("foo_query", ["foo","foo-mcp"], short)` | `None` | the fail-safe ([`:92-94`](../../tmp/pi-mcp-adapter/__tests__/resolve-server-from-tool-name.test.ts)) |
| `("mcp_query", ["-mcp"], short)` | `-mcp` | the empty-short `"mcp"` fallback ([`:86-90`](../../tmp/pi-mcp-adapter/__tests__/resolve-server-from-tool-name.test.ts)) |
| `("searxngweb_search", ["searxng"], server)` | `None` | the `_` boundary is required ([`:158-163`](../../tmp/pi-mcp-adapter/__tests__/resolve-server-from-tool-name.test.ts)) |
| `("a_20_b_run", ["a b","a-20-b"], server)` | `a b` | escaped and literal prefixes stay distinct ([`:232-238`](../../tmp/pi-mcp-adapter/__tests__/resolve-server-from-tool-name.test.ts)) |

## 2 · `crates/cyrup-mcp/src/proxy/tool_metadata.rs` — delete the copy

Delete [`:85-394`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) in full — `sanitize_server_prefix`,
`get_server_prefix`, `strip_mcp_suffix`, `format_tool_name`, `format_legacy_tool_name`,
`resolve_tool_prefix`, `get_tool_name_candidates`, `glob_to_regex`, `matches_tool_pattern`,
`index_has_other_current_match`, `matches_tool_selector`, `is_tool_included`, `is_tool_allowed`.

**Keep** three items, which are 13e types the proxy legitimately owns and which are *not* drifted:
`ToolMetadata` ([`:38-71`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs)),
`is_ui_tool_visible_to_model` ([`:78-83`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) — it
takes `Option<&[String]>` against the proxy's typed `CachedTool`, where
[`registration.rs:744`](../../crates/cyrup-mcp/src/registration.rs) takes `Option<&Value>` against a
different struct; two readers of two shapes, not a duplicate), and `find_tool_by_name`
([`:407-413`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs)).

In their place, extend the re-export the file already uses at
[`:402`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) and
[`:420`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs):

```rust
// **De-duplicated (MCP-073/075/076).** The tool-name grammar had two implementations in this
// crate and they had drifted: this copy's `format_legacy_tool_name` re-escaped an already-escaped
// prefix (`mcp__github` → `mcp_5f__5f_github`), so every legacy `excludeTools` / `approveTools` /
// `searchKeywords` selector under `ToolPrefix::Mcp` — and every selector at all for a server whose
// name carries a character outside `[A-Za-z0-9_-]` — silently failed to match. `registration.rs`
// is the surviving grammar; it is `types.ts` verbatim.
pub use crate::registration::{
    CandidateIndex, format_tool_name, is_tool_allowed, matches_tool_pattern,
    resolve_server_from_tool_name, resolve_tool_prefix, resource_name_to_tool_name,
    sanitize_server_prefix, server_prefix, tool_name_candidates, truncate_at_word,
};
```

Update the module header ([`:1-8`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs)) to say the
naming grammar has already made the move it predicted and that what remains is the three 13e types.
[`proxy/mod.rs`](../../crates/cyrup-mcp/src/proxy/mod.rs)'s `pub use tool_metadata::*;` carries the
re-exports onward, so no `crate::proxy::X` path outside this directory changes.

## 3 · `crates/cyrup-mcp/src/proxy/description.rs` — the collision set becomes an index

`collision_candidates` ([`:79-111`](../../crates/cyrup-mcp/src/proxy/description.rs)) returns a bare
`IndexSet` because the deleted `is_tool_allowed` took one. The surviving one takes a
`CandidateIndex`, so build one — and make "no server declares a selector" the `None` it already
means, instead of an empty set plus a second predicate.

```rust
/// The MCP-198 cross-server collision set, as the memoising index `is_tool_allowed` consumes.
///
/// `None` — never an empty index — when no server declares a selector: `direct-tools.ts:257-262`
/// (upstream `faf55f7`) short-circuits before it reads the set, so building one nothing consults
/// is pure startup cost. See the original note for the 2.6 s figure behind that commit.
fn collision_index(
    config: &McpConfig,
    cache: &IndexMap<String, CachedServerEntry>,
    prefix: ToolPrefix,
) -> Option<CandidateIndex> {
    if !config.mcp_servers.values().any(server_has_tool_filters) {
        return None;
    }
    let mut all_candidates: IndexSet<String> = IndexSet::new();
    for (other_server, other_definition) in &config.mcp_servers {
        let Some(other_entry) = cache.get(other_server) else { continue };
        if other_definition.is_disabled() {
            continue;
        }
        let other_prefix = resolve_tool_prefix(Some(other_definition), prefix);
        for tool in &other_entry.tools {
            // `isUiToolVisibleToModel` survives the MCP Apps cut — see the original note.
            if !is_ui_tool_visible_to_model(tool.ui_visibility.as_deref()) {
                continue;
            }
            all_candidates.extend(tool_name_candidates(&tool.name, other_server, other_prefix, false));
        }
        if other_definition.expose_resources() {
            for (name, _) in &other_entry.resources {
                let base = format!("read_{}", resource_name_to_tool_name(name));
                all_candidates.extend(tool_name_candidates(&base, other_server, other_prefix, false));
            }
        }
    }
    Some(CandidateIndex::new(all_candidates))
}
```

In `build_proxy_description`, [`:165`](../../crates/cyrup-mcp/src/proxy/description.rs) becomes
`let mut collision = collision_index(config, cache, prefix);`, and the two counting closures at
[`:180-215`](../../crates/cyrup-mcp/src/proxy/description.rs) become explicit loops — a `&mut`
borrow cannot cross an `.iter().filter(…).count()` closure:

```rust
        let mut index = server_has_tool_filters(definition).then(|| collision.as_mut()).flatten();

        let mut tool_count = 0_usize;
        if let Some(entry) = entry {
            for tool in &entry.tools {
                if !is_ui_tool_visible_to_model(tool.ui_visibility.as_deref()) {
                    continue;
                }
                if is_tool_allowed(
                    &tool.name,
                    server_name,
                    effective_prefix,
                    definition.include_tools.as_deref(),
                    definition.exclude_tools.as_deref(),
                    index.as_deref_mut(),
                ) {
                    tool_count += 1;
                }
            }
        }

        let mut resource_count = 0_usize;
        if definition.expose_resources()
            && let Some(entry) = entry
        {
            for (name, _) in &entry.resources {
                let base = format!("read_{}", resource_name_to_tool_name(name));
                if is_tool_allowed(
                    &base,
                    server_name,
                    effective_prefix,
                    definition.include_tools.as_deref(),
                    definition.exclude_tools.as_deref(),
                    index.as_deref_mut(),
                ) {
                    resource_count += 1;
                }
            }
        }
```

`index.as_deref_mut()` on an `Option<&mut CandidateIndex>` is the same reborrow
`registration::is_tool_allowed` already performs at
[`:469`](../../crates/cyrup-mcp/src/registration.rs).

## 4 · `crates/cyrup-mcp/src/proxy/approval.rs`

Import from the new path ([`:21`](../../crates/cyrup-mcp/src/proxy/approval.rs)); rename
`get_tool_name_candidates` → `tool_name_candidates` at
[`:101`](../../crates/cyrup-mcp/src/proxy/approval.rs), [`:112`](../../crates/cyrup-mcp/src/proxy/approval.rs),
[`:124`](../../crates/cyrup-mcp/src/proxy/approval.rs), [`:134`](../../crates/cyrup-mcp/src/proxy/approval.rs),
[`:164`](../../crates/cyrup-mcp/src/proxy/approval.rs); drop the `Option` wrapper on every
`matches_tool_pattern` argument ([`:103`](../../crates/cyrup-mcp/src/proxy/approval.rs),
[`:111`](../../crates/cyrup-mcp/src/proxy/approval.rs), [`:178-179`](../../crates/cyrup-mcp/src/proxy/approval.rs)):

```rust
    if matches_tool_pattern(&current, patterns) {          // was `Some(patterns)`
    …
        matches_tool_pattern(&legacy, std::slice::from_ref(pattern))
            && !matches_tool_pattern(&other_current, std::slice::from_ref(pattern))
```

`resolve_tool_prefix` calls here already pass `Option` and are unchanged. `approval_legacy_arm`'s
`shift_remove` calls ([`:172`](../../crates/cyrup-mcp/src/proxy/approval.rs),
[`:175`](../../crates/cyrup-mcp/src/proxy/approval.rs)) and its insertion-order `find`
([`:168`](../../crates/cyrup-mcp/src/proxy/approval.rs)) keep working unchanged — that is what the
`IndexSet` decision bought.

## 5 · `crates/cyrup-mcp/src/proxy/ranking.rs`

[`:15`](../../crates/cyrup-mcp/src/proxy/ranking.rs) import rewrite; `get_server_prefix` →
`server_prefix` at [`:438`](../../crates/cyrup-mcp/src/proxy/ranking.rs) and in the tests at
[`:690-701`](../../crates/cyrup-mcp/src/proxy/ranking.rs); `get_tool_name_candidates` →
`tool_name_candidates` at [`:292`](../../crates/cyrup-mcp/src/proxy/ranking.rs); and at
[`:301`](../../crates/cyrup-mcp/src/proxy/ranking.rs):

```rust
        if !matches_tool_pattern(&candidates, std::slice::from_ref(pattern)) {
```

## 6 · `crates/cyrup-mcp/src/proxy/call.rs`

[`:23`](../../crates/cyrup-mcp/src/proxy/call.rs) import rewrite; `get_server_prefix` →
`server_prefix` at [`:390`](../../crates/cyrup-mcp/src/proxy/call.rs). `find_tool_by_name` and
`ToolMetadata` still come from `tool_metadata`.

## 7 · `crates/cyrup-mcp/src/proxy/discovery.rs`

[`:18`](../../crates/cyrup-mcp/src/proxy/discovery.rs) imports `find_tool_by_name`,
`truncate_at_word` and `ToolMetadata` — all three survive. No change beyond confirming the path
still resolves.

## 8 · `crates/cyrup-mcp/src/ui.rs` — retype the panel arm

Add `IndexSet` to the imports; the two helper signatures at
[`:1496`](../../crates/cyrup-mcp/src/ui.rs) and [`:1522`](../../crates/cyrup-mcp/src/ui.rs) take
`&IndexSet<String>`; the builder at [`:1784-1785`](../../crates/cyrup-mcp/src/ui.rs) returns and
constructs an `IndexSet`; and two removals become `shift_remove`
([`:1507`](../../crates/cyrup-mcp/src/ui.rs), [`:1815`](../../crates/cyrup-mcp/src/ui.rs)). The two
`resolve_tool_prefix` calls ([`:1660`](../../crates/cyrup-mcp/src/ui.rs),
[`:1798`](../../crates/cyrup-mcp/src/ui.rs)) take `Some(…)`.

Do **not** collapse `matches_tool_selector_set` / `is_tool_allowed_set` into
`registration::is_tool_allowed`. Its own doc at [`:1485-1487`](../../crates/cyrup-mcp/src/ui.rs)
records why they differ — the panel supplies a pre-subtracted set and the two arms genuinely
disagree — and that is upstream's shape, not drift.

## 9 · `crates/cyrup-mcp/src/lib.rs`

Re-export the new function beside the crate's other public grammar, near
[`:181`](../../crates/cyrup-mcp/src/lib.rs):

```rust
pub use registration::resolve_server_from_tool_name;
```

## 10 · Test-module mechanics in `registration.rs`

Five `CandidateIndex::new(HashSet…)` constructions in the existing test module become `IndexSet`
([`:2363`](../../crates/cyrup-mcp/src/registration.rs), [`:2373`](../../crates/cyrup-mcp/src/registration.rs),
[`:2385`](../../crates/cyrup-mcp/src/registration.rs), [`:2402`](../../crates/cyrup-mcp/src/registration.rs)).
`IndexSet` implements `From<[T; N]>`, so `HashSet::from([…])` → `IndexSet::from([…])` is a
one-token change at each site. These are container renames in tests that already exist — **no new
tests are part of this task.**

## Lint constraints on every line above

`clippy::unwrap_used`, `expect_used`, `panic` and `indexing_slicing` are `deny` workspace-wide.
`resolve_server_from_tool_name` is written without `sort` + `[0]` for exactly that reason.
`rustdoc::broken_intra_doc_links` is `deny` and `.cargo/config.toml` passes
`--document-private-items`, so every `[`Item`]` in a moved or rewritten doc comment must resolve
from its **new** module — `proxy/tool_metadata.rs`'s surviving docs that referenced deleted local
items are the ones to check.

---

# Part 4 — The other eighteen high units, routed

Not this task's work. Each row names the file that owns it or the condition that unblocks it. No
unit is left unaccounted for.

| unit | § | disposition |
|---|---|---|
| `MCP-014` | 13a | Re-`init` per session. Blocked on `on_session_start` building anything → [MCP_RUNTIME_INIT_SPINE.md](MCP_RUNTIME_INIT_SPINE.md) |
| `MCP-025` | 13a | Startup connect notifications. Same block; needs the spine's notification surface |
| `MCP-029` | 13a | `updateMetadataCache` write rules. This is the missing production caller for `dirs.rs`'s serialisers (`MCP-140`) and for `save_metadata_cache` — one obligation, and it belongs to the spine |
| `MCP-140` | 13c | Serialisers exist at [dirs.rs:761/786/808](../../crates/cyrup-mcp/src/dirs.rs) with test-only callers. **Row restated: the gap is `MCP-029`'s call site.** Do not schedule separately |
| `MCP-070` | 13b | Absent-vs-null hash pre-image. The row's real content is that every production caller hashes **unresolved** values via `ResolvedIdentity::verbatim`, which depends on `MCP-082`/`MCP-084` → [MCP_CONNECT_AND_OAUTH_LAST_MILE.md](MCP_CONNECT_AND_OAUTH_LAST_MILE.md) |
| `MCP-139` | 13c | **Row restated by this pass.** The real content is the two independent cache implementations (`dirs.rs` and `registration.rs`, see Correction 4). Needs its own task; it touches the same two files as this one and must not run concurrently with it |
| `MCP-145` | 13c | `isServerCacheValid` exists at [dirs.rs:843/850](../../crates/cyrup-mcp/src/dirs.rs); the throw arm is unreachable because `install_server_hasher` ([registration.rs:785](../../crates/cyrup-mcp/src/registration.rs)) has no production caller. Rides with `MCP-139` |
| `MCP-214a` | 13e | `recoverAuthConnection` + per-server request options. Blocked on `MCP-214`, which the spine owns |
| `MCP-249` | 13e | The `details` schema. `server_unavailable` is absent from the 32-variant `McpErrorCode` ([proxy/error_vocab.rs:30](../../crates/cyrup-mcp/src/proxy/error_vocab.rs)); its producer is `direct-tools.ts` step 7, which is `MCP-214`'s. Ships with `MCP-214a` |
| `MCP-326` | 13g | The manual/headless OAuth leg. `abort::combine` ([abort.rs:60](../../crates/cyrup-mcp/src/abort.rs)) returns a bare `cyrup_core::CancelToken`, which carries **no reason payload at all** — so the fix is a cross-crate type change, not an `oauth.rs` edit. Group with `MCP-324` in [MCP_CONNECT_AND_OAUTH_LAST_MILE.md](MCP_CONNECT_AND_OAUTH_LAST_MILE.md) |
| `MCP-381` | 13h | `/mcp`'s owner-fenced prologue and eight-way switch. Explicitly disowned by [MCP_COMMAND_SURFACE.md:50-52](MCP_COMMAND_SURFACE.md), which requires it ship as one commit with `MCP-040` + `MCP-042` + `MCP-334`. Blocked on the spine |
| `MCP-398` | 13h | The prompt command handler. Same commit boundary; consumes the grammar `MCP_COMMAND_SURFACE` builds |
| `MCP-386` | 13h | `reconnectServer` / `reconnectServers`. An arm of `MCP-381`'s switch; additionally blocked on a real reconnect → [MCP_DISCOVERY_PAGINATION.md](MCP_DISCOVERY_PAGINATION.md) |
| `MCP-387` | 13h | `/mcp setup` + reload-after-write. Arm of `MCP-381` |
| `MCP-388` | 13h | `logoutServer`. Arm of `MCP-381` |
| `MCP-390` | 13h | `authenticateServer` / `/mcp-auth`. `TODO(MCP-334)` at [oauth.rs:3780](../../crates/cyrup-mcp/src/oauth.rs) marks the seam. Arm of `MCP-381` |
| `MCP-392` | 13h | `buildMcpPanelCallbacks`' eight-rung status ladder. Reads state the arms write; last within the switch |
| `MCP-395` | 13h | **Reduced.** HA-1's command leg landed — `register_late_command` at [facade.rs:724](../../crates/cyrup-ext/src/facade.rs), `LateRegistrar` at [native.rs:768](../../crates/cyrup-ext/src/native.rs), `sync_tool_surface` at [extension.rs:166](../../crates/cyrup-mcp/src/extension.rs). All that remains is a **live caller**, which the spine supplies |

**One correction to carry into 13i.** `MCP-471`'s row calls the human-interaction gate `missing`
([13-cyrup-mcp-STATUS.md:476](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)), but
`McpState::human_wait_ctx` exists at [state.rs:151](../../crates/cyrup-mcp/src/state.rs), is written
from `on_event` at [extension.rs:742](../../crates/cyrup-mcp/src/extension.rs) and consumed at
[owner.rs:552-570](../../crates/cyrup-mcp/src/owner.rs). Flag to
[MCP_13I_SCOPING.md](MCP_13I_SCOPING.md).

---

# Definition of done

Each line is a command whose output is the check.

- [ ] **One definition of each grammar function, all in `registration.rs`.**
      `rg -n 'fn (sanitize_server_prefix|server_prefix|get_server_prefix|strip_mcp_suffix|format_tool_name|legacy_server_prefix|format_legacy_tool_name|tool_name_candidates|get_tool_name_candidates|glob_to_regex|matches_tool_pattern|matches_tool_selector|is_tool_included|is_tool_allowed|resolve_tool_prefix|index_has_other_current_match)\(' crates/cyrup-mcp/src`
      returns hits **only** in `registration.rs`, plus `ui.rs`'s two `_set` variants.
- [ ] **No unbounded regex compile in the crate.**
      `rg -n 'Regex::new' crates/cyrup-mcp/src` returns zero hits; every construction goes through
      `RegexBuilder` with both ceilings.
- [ ] **The double-escape is gone.** `rg -n 'is_ascii_alphanumeric' crates/cyrup-mcp/src/proxy/`
      returns zero hits — the only strict-grammar sanitiser left is
      `registration::sanitize_server_prefix`'s `preserve_provider_valid == false` arm.
- [ ] **`proxy/tool_metadata.rs` is the three 13e types plus re-exports** — under 130 lines, and
      `rg -c '^(pub )?fn ' crates/cyrup-mcp/src/proxy/tool_metadata.rs` is `2`
      (`is_ui_tool_visible_to_model`, `find_tool_by_name`).
- [ ] **MCP-073 exists and is public.**
      `rg -n 'resolve_server_from_tool_name' crates/cyrup-mcp/src/lib.rs crates/cyrup-mcp/src/registration.rs`
      shows the definition and the `lib.rs` re-export.
- [ ] **`tool_name_candidates` returns `IndexSet<String>`** and emits upstream's three legacy groups
      in `types.ts:786` / `:792` / `:797` order.
- [ ] **`resolve_tool_prefix` takes `Option<&ServerEntry>`** and all six call sites compile.
- [ ] `cargo check --workspace --all-targets` exits 0.
- [ ] `cargo doc --workspace --no-deps --bins` exits 0 (the rustdoc lints are `deny`; moved doc
      comments are the risk).
- [ ] `cargo clippy -p cyrup-mcp --all-targets` reports no new findings.
- [ ] `cargo nextest run --workspace` — 7862 passing, no regressions. Existing tests are retyped,
      not added to.
- [ ] Every `proxy.rs:NNNN` citation this file used to carry has been replaced by a live
      `proxy/<file>.rs:NNNN` one, and every relative link above resolves from `.flux/todo/`.

Not part of this task: writing new tests or benchmarks; adding documentation beyond the doc comments
the edits above quote; editing anything under `docs/`; touching
`cyrup-ext-subagents`' grammar (copy #4); wiring `resolve_server_from_tool_name` into
`cyrup-permission-system`.
