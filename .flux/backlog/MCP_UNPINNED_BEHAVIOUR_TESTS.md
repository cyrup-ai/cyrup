---
stage: aug
status: done
updated: 2026-08-27 06:00
---

# Pin The Implemented cyrup-mcp Behaviour That Nothing Asserts

## Description

Ten units, one obligation: **behaviour that is fully implemented and has no assertion that would
catch its regression.** None of these is a port gap. Every one of them is a place where the Rust is
correct today and a one-character edit tomorrow would be silent.

They are grouped by the *harness* they need, not by the file they land in, because the harness is
the whole cost:

* **Harness A — a fallible `FakeEnv`.** `FakeEnv::call_tool`
  ([proxy/testsupport.rs:113-122](../../crates/cyrup-mcp/src/proxy/testsupport.rs)) and
  `FakeEnv::read_resource` (:123-131) return `Ok` unconditionally, so `catch_arm`
  ([proxy/call.rs:841-909](../../crates/cyrup-mcp/src/proxy/call.rs)) — the whole of MCP-165's error
  taxonomy — is **unreachable from any test in the crate**.
  `handle_url_elicitation_required` (testsupport.rs:132-138), `complete_auth_from_input` (:185-192)
  and `guard_mcp_output` (:212-218) are likewise hard-wired to one answer each, and the
  `recovery: &AuthRecovery<'_>` argument both call seams take is **ignored** (`_recovery`), so
  `AuthRecovery::recover` (call.rs:83-121) — the entire mid-request auto-auth ladder — is dead to
  the test suite too. MCP-165 and MCP-168 both die at that fence; making the fake scriptable
  unblocks both at once.
* **Harness B — the existing `cyrup-it` MCP target, plus one fixture `keyctl`/helper pair.**
  MCP-278 and MCP-287 need real spawned subprocesses. They share one new module and one pair of
  shell fixtures. MCP-269 does **not** belong there (see **F6**) and MCP-283's cache cases are
  in-crate.
* **Harness C — equality guards between duplicated literals.** MCP-151, MCP-152 and MCP-192 are the
  same defect three times: a string that exists in two places, where the copy under test is not the
  copy the registered tool carries. MCP-171 is the code-health residue that belongs with them.

Sources: [13d-mcp-proxy-modes.md](../../docs/gap-analysis/13d-mcp-proxy-modes.md) (MCP-151 :779,
MCP-152 :794, MCP-165 :994, MCP-168 :1021, MCP-171 :1061, MCP-192 :1202),
[13f-mcp-credentials.md](../../docs/gap-analysis/13f-mcp-credentials.md) (MCP-269 :1046,
MCP-278 :1181, MCP-283 :1196, MCP-287 :1262), and the census rows in
[13-cyrup-mcp-STATUS.md](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) (:668, :669, :682, :684,
:687, :696, :790, :799, :803, :807) — all ten verified present at those lines.

Upstream is checked out at [tmp/pi-mcp-adapter](../../tmp/pi-mcp-adapter) at tag **v2.26.1**
(`fafae21`); every `file:line` below is at that tag.

None of these units appears in [MCP_HIGH_SEVERITY_BACKLOG.md](MCP_HIGH_SEVERITY_BACKLOG.md), in
[MCP_13I_SCOPING.md](MCP_13I_SCOPING.md), or in the already-filed MCP task set.

---

## Findings that change the plan — read before implementing

Nine premises in the specs, the census and this task's own earlier draft are **wrong**. Each is
corrected below, and the correction is what the Implementation section prescribes.

### F0 · `proxy.rs` no longer exists — every `proxy.rs:NNNN` citation is stale

Commit `ba75bbf` (*"refactor(mcp): decompose proxy.rs into a proxy/ module"*) split the file into
fourteen. Every line reference in the specs, in the census and in this task's prior draft points at
a file that is gone. The mapping actually needed:

| behaviour | old | now |
|---|---|---|
| `FakeEnv`, `ctx_with`, `text_of`, `config_with`, `http`, `stdio` | proxy.rs:4881-… | [proxy/testsupport.rs:40-313](../../crates/cyrup-mcp/src/proxy/testsupport.rs) |
| `catch_arm`, `invoke`, `execute_call`, `AutoAuthLatch`, `AuthRecovery` | proxy.rs:3711 / :3618 | [proxy/call.rs:841](../../crates/cyrup-mcp/src/proxy/call.rs), :735, :202, :41, :70 |
| `execute_auth_complete`, `attempt_auto_auth` | proxy.rs:2669 | [proxy/auth.rs:143](../../crates/cyrup-mcp/src/proxy/auth.rs), :219 |
| `mcp_tool_schema`, `McpTool`, the dispatcher's `redirectUrl ?? code ?? input` | proxy.rs:4080 / :4474 | [proxy/tool.rs:53](../../crates/cyrup-mcp/src/proxy/tool.rs), :266, :447 |
| `MCP_TOOL_GUIDELINE`, `MCP_TOOL_NAME`, `INSTRUCTIONS_SNIPPET_LENGTH` | proxy.rs:181 / :108 | [proxy/constants.rs:101](../../crates/cyrup-mcp/src/proxy/constants.rs), :83, :29 |
| `proxy::build_proxy_description` | proxy.rs:3904 | [proxy/description.rs:139](../../crates/cyrup-mcp/src/proxy/description.rs) |
| `ProxyCallError`, `UrlElicitationAction`, `GuardedOutput`, `ProxyEnv` | proxy.rs | [proxy/env.rs:90](../../crates/cyrup-mcp/src/proxy/env.rs), :111, :219, :272 |
| `rank_collate` and its three sort sites | proxy.rs:1074 / :1134 / :2384 / :2413 | [proxy/ranking.rs:324](../../crates/cyrup-mcp/src/proxy/ranking.rs), :378, [proxy/discovery.rs:497](../../crates/cyrup-mcp/src/proxy/discovery.rs), :526 |

`proxy/testsupport.rs` already exists and already holds the shared fixtures; nothing new has to be
invented for Harness A's *location*.

### F1 · The `cyrup-it` MCP target already exists

MCP-278's **cyrup** paragraph and the crate header
([credentials.rs:70-75](../../crates/cyrup-mcp/src/credentials.rs)) both say *"`cyrup-it` sets
`autotests = false`, so the MCP target must be declared by hand"*. It is already declared:
`[[test]] name = "mcp"`, `path = "tests/mcp/main.rs"`, `required-features = ["it"]` at
[Cargo.toml:198-204](../../crates/cyrup-it/Cargo.toml), with
[tests/mcp/main.rs](../../crates/cyrup-it/tests/mcp/main.rs) already pulling in
`#[path = "../support/mod.rs"] mod support;` (:23-24) and `mod activation;` (:26). `cyrup-mcp` is
already a dev-dependency (Cargo.toml:102). **Do not create a target and do not add a dependency.
Add one module file and one `mod` line.**

### F2 · MCP-287's rung-1 message is NOT upstream's `ETIMEDOUT` text

MCP-287 (13f:1265-1268) describes upstream's `spawnSync` timeout as surfacing
`Linux keyring recovery helper could not start: <ETIMEDOUT message>`. cyrup does **not** produce
that string. The wait-with-timeout at
[credentials.rs:1487-1514](../../crates/cyrup-mcp/src/credentials.rs) kills the child and returns
its own sentence (:1504-1507):

```
Linux keyring recovery helper could not start: {keyctl} timed out after {ms} ms
```

with `{keyctl}` = `RecoveryInvocation::keyctl` (credentials.rs:1356, the **resolved program string
from the env override** — not the literal `"keyctl"`) and `{ms}` = `KEYRING_RECOVERY_TIMEOUT_MS` =
`10_000` (credentials.rs:158). **Assert cyrup's own string.** A test authored against upstream's
ETIMEDOUT text is authored to fail. `AuthSecretStoreError::Recovery` renders `{0}` verbatim
(credentials.rs:316-317), so `err.to_string()` on the value `LinuxKeyringRecoveryStore::read`
returns is exactly that sentence.

### F3 · MCP-269's "no `Serialize` derive" clause is unsatisfiable as written

MCP-269's **verify** (13f:1059-1060) asks for *"no `Serialize` derive that could route an
`AuthEntry`"* to `cyrup_config::env`'s auth path. `AuthEntry` **must** derive `Serialize` — it is
the keychain payload (credentials.rs:595, `#[derive(Clone, Default, Serialize, Deserialize)]`),
written with `serde_json::to_string` on the store path. Deleting the derive deletes the credential
store.

What the clause protects is *reachability*, not the derive. Prescribed replacement, both halves
source-observable: (a) a **source guard** asserting `cyrup_config::auth` appears nowhere in
`crates/cyrup-mcp/src/**` outside comments — the crate header already asserts the design property
in prose (credentials.rs:14, :80-82) with nothing enforcing it; and (b) a **behavioural guard**
running a full save/update/remove lifecycle against a temp agent dir and asserting
`<agent_dir>/auth.json` — `cyrup_config::env::Env::auth_path`'s exact construction
([env.rs:317-319](../../crates/cyrup-config/src/env.rs), `self.agent_dir.join("auth.json")`) —
never comes into existence and no file under the tree contains the token text.

Both mentions of `cyrup_config::auth` in the crate today are doc comments (credentials.rs:14 as
`//!`, credentials.rs:2299 as an indented `///`), so a comment-skipping guard is green on landing.

### F4 · `remove_evicts_even_when_the_cache_is_disabled` does not disable the cache

[credentials.rs:4249-4262](../../crates/cyrup-mcp/src/credentials.rs) builds its store with
`store_with_env(Arc::new(|_| None))`. `is_cache_enabled` is
`!env_is_one(&env, &AUTH_CACHE_DISABLED_ENV)` (credentials.rs:2155-2157) and `env_is_one` is
`== Some("1")` (:236-238), so an env that answers `None` leaves the cache **enabled**. The test
therefore asserts eviction on the gate-**on** path while its name claims the gate-off path, and
upstream's `evicts a removed credential even when the gate is turned off`
([__tests__/mcp-auth-cache.test.ts:268-279](../../tmp/pi-mcp-adapter/__tests__/mcp-auth-cache.test.ts))
is unported. Fix the fixture and the name; do not delete the gate-on assertion — keep both.

### F5 · The strong schema and guideline tests guard the copies that are not registered

`mcp_tool_schema()` ([proxy/tool.rs:53-81](../../crates/cyrup-mcp/src/proxy/tool.rs)) is guarded by
a twelve-name / exact-`action`-description / alphabetical-order assertion (proxy/tool.rs:520-549).
The tool `register_surface` actually registers is `registration::ProxyTool`
([registration.rs:2179](../../crates/cyrup-mcp/src/registration.rs)), whose `parameters` come from
the **second literal**, `proxy_tool_parameters()` (registration.rs:1675-1735), guarded only by a
five-name spot check (registration.rs:2875-2887). The two literals are byte-equivalent today and
**nothing compares them**, so either may drift alone.

Same shape for the guideline: `MCP_TOOL_GUIDELINE` (proxy/constants.rs:101-102) is the one under
test at proxy/constants.rs:111-126; `PROXY_TOOL_PROMPT_GUIDELINE` (registration.rs:162) is the one
`ProxyTool::prompt_guidelines` returns (registration.rs:1757-1759). registration.rs:160-161 claims
*"A tree-wide grep finds exactly two occurrences: the matcher, and this line"*. A case-insensitive
tree-wide grep for `discovery first` returns **five** hits across three files
(`sanitize/tools.rs:47`, `registration.rs:162`, `proxy/constants.rs:96`, `:102`, `:123`). The doc
claim is stale and must be corrected with the fix.

### F6 · MCP-269's two guards belong in `cyrup-mcp`, not in `cyrup-it`

`crates/cyrup-it/tests/mcp/main.rs:11-14` states the target's own curation bar: *"a real session, a
real process, or a real socket. A unit that can assert against `register_surface` and a `TempDir`
belongs in `crates/cyrup-mcp/src`."* MCP-269's source guard is a file walk and its behavioural
guard is a `TempDir` + `MemorySecretStore` lifecycle — neither needs a session, a process or a
socket. **Put both in `credentials.rs`'s existing `#[cfg(test)] mod tests`, beside `test_store`
(credentials.rs:3641-3654).** That deletes a whole file from the plan. MCP-278 and MCP-287 do need
real subprocesses and stay in `cyrup-it`.

### F7 · MCP-152's gap is narrower and sharper than "the copies are unguarded"

Both description builders already have goldens, and they do not overlap:

* `registration::build_proxy_description` (registration.rs:1271-1426) is pinned **whole-string**,
  every usage line included, by `proxy_description_golden_for_two_servers`
  (registration.rs:2756-2790).
* `proxy::build_proxy_description` ([description.rs:139-284](../../crates/cyrup-mcp/src/proxy/description.rs))
  is pinned by `proxy_description_renders_every_block_in_order` (description.rs:311-367), which
  asserts `starts_with(<head literal>)`, `contains(<Disabled servers sentence>)`,
  `contains(<Server instructions sentence>)`, `ends_with(<the Mode: line>)` and
  `matches('→').count() == 9` — **but not the text of the nine usage lines.**

So the real hole is exactly two things: (1) the proxy copy's nine usage lines can drift
individually (only their count and the final `Mode:` line are pinned), and (2) nothing compares the
two copies beyond `text.lines().next()` (`both_proxy_descriptions_share_one_head_line`,
registration.rs:3070-3086). registration.rs:1276-1283 records that these two heads **have already
diverged once**, and that the cost was a permanently-firing re-registration invalidating the
provider's prompt-cache prefix.

**Do not** prescribe a three-server whole-string equality between the two builders. They read
different cache types with different validity semantics — `registration` filters through
`valid_entry(cache, …)` (a staleness/fingerprint check, registration.rs:1311) while `proxy` takes
`cache.get(server_name)` straight (description.rs:171) — and building a `MetadataCache` that passes
`valid_entry` alongside an equivalent `IndexMap<String, CachedServerEntry>` would make the test red
for reasons unrelated to the two literals. The **empty config** is the exact right input: with no
servers, blocks 2-5 all render empty and the output *is* head + usage block, so whole-string
equality there compares precisely the two duplicated literals and nothing else.

### F8 · `feruca::Collator` does not memoise, and the cited precedent does not exist

MCP-171's residue is real — `locale_compare`
([config.rs:4047-4051](../../crates/cyrup-mcp/src/config.rs)) builds a fresh collator per
comparison and `rank_collate` (ranking.rs:324-326) delegates to it from three sorts
(ranking.rs:378, discovery.rs:497, discovery.rs:526), so an O(n log n) sort builds O(n log n)
collators. Two claims around it are wrong:

* The doc at config.rs:4042-4046 says `collate` takes `&mut self` because *"it memoises"*. It does
  not. `feruca-0.12.0` `src/collator.rs:108-123` shows the mutable state is four scratch buffers
  (`a_chars`, `b_chars`, `a_cea`, `b_cea`); `Collator::new` (:135-147) allocates `vec![0; 64]`
  twice. The per-call cost is two heap allocations, not a lost cache — and the correctness question
  a hoist raises is **buffer reuse**, not memoisation, which is what the new assertion must probe.
* `crates/cyrup-config/src/model.rs` **does not exist**. The real precedents are
  [model/resolver.rs:149-153](../../crates/cyrup-config/src/model/resolver.rs) and
  [cyrup-tools/src/tools/ls.rs:128-129](../../crates/cyrup-tools/src/tools/ls.rs), and both hoist
  one collator out of a *single local* `sort_by`. That lever is unavailable here: `locale_compare`
  is a free function in `config.rs` called from three sorts in two other modules, which is exactly
  why this needs a thread-local rather than a hoisted local.

### F9 · The registered `ProxyTool` has no dispatch implementor yet

`grep -rn "McpToolDispatch for"` over `crates/cyrup-mcp/src` returns **nothing**: no type
implements the trait (registration.rs:1456-1475), so `ProxyTool::execute` always takes the
`dispatch.get() == None` arm (registration.rs:1772-1774) and answers `not_initialized_result()`.
Meanwhile `McpTool` (proxy/tool.rs:266) is a fully-wired `Tool` that `register_surface` never
registers and that no other crate names. That wiring is MCP-214's, out of scope here — but it means
**neither** tool is "the live one" today, so the fix for MCP-151/MCP-192 must be *collapse to one
literal*, not *move the test to the other tool*. Collapsing is correct whichever tool MCP-214 lands
on. Say so in the doc comments you touch, so the next reader is not misled by "the registered tool".

---

## Per-unit breakdown

### MCP-165 · `executeCall`'s error taxonomy — medium, `hand-written`, implemented

**Obligation unmet.** The three arms of `catch_arm`
([proxy/call.rs:841-909](../../crates/cyrup-mcp/src/proxy/call.rs)) are exercised by nothing:
`SessionRecoveryAuthRequired` (:851), `UrlElicitationRequired` (:860) and `Other` (:873). Both call
sites (call.rs:768 for the resource path, :775 for the tool path) sit behind
`ProxyEnv::read_resource` / `call_tool`, and the only implementor in the tree returns `Ok`
unconditionally (testsupport.rs:113-131). Upstream is `proxy-modes.ts:1294-1322`.

**Sharpest single fact.** `details.autoAuthAttempted` is inserted at **exactly one place in the
crate** — call.rs:857 — and nothing asserts that key. The other eight `auth_required` producers
(call.rs:303, :359, :416, :534, :549, :578, :592 and auth.rs:336) all live in the resolution state
machine and carry no such key, so `autoAuthAttempted`'s presence is the only discriminator between
"the catch arm fired" and "resolution refused". Assert its presence, not just its value.

**Second sharpest.** `AuthRecovery::recover` (call.rs:83-121) — the whole mid-request ladder,
including the `Failed ⇒ Err(SessionRecoveryAuthRequired)` raise at :93-98 — is never called by any
test, because both seams take `_recovery` and drop it. That is also the only route by which
`latch.attempted()` is ever `true` at call.rs:857.

### MCP-168 · `executeAuthComplete` — medium, `hand-written`, implemented

**Obligation unmet.** MCP-168's verify (13d:1029-1031) names two unit tests. Neither exists. The one
test present (`auth_complete_closes_the_connection_and_clears_the_failure`,
[proxy/auth.rs:594-615](../../crates/cyrup-mcp/src/proxy/auth.rs)) covers only the success arm
(auth.rs:173-185).

1. *all three input keys accepted* — the `redirectUrl ?? code ?? input` selection lives in the tool
   dispatcher (proxy/tool.rs:447-455, upstream `index.ts:856`), not in `execute_auth_complete`, so
   it must be driven through `McpTool::execute`. `FakeEnv` currently cannot record which input
   arrived (`complete_auth_from_input` at testsupport.rs:185-192 ignores its argument).
2. *a non-`"authenticated"` status yields `not_authenticated` with the status echoed* — the arm at
   auth.rs:167-172 is unreachable while the fake always answers `"authenticated"`.

The `auth_complete_failed` arm (auth.rs:161-166) is likewise unreachable; it comes free once
`complete_auth_from_input` can fail.

### MCP-192 · The permission system's contracts on the `mcp` tool — medium, `host-verb`, implemented

**Obligation unmet.** `guideline_normalises_to_the_sanitizer_key`
([proxy/constants.rs:111-126](../../crates/cyrup-mcp/src/proxy/constants.rs)) **re-implements** the
sanitizer's normalisation inline and compares against a copy of the key literal — so a drift in
[sanitize/tools.rs:47](../../crates/cyrup-permission-system/src/sanitize/tools.rs) is invisible to
it, and it tests `MCP_TOOL_GUIDELINE`, which is not the constant the registered tool carries (F5).
The registration-side test (registration.rs:2890-2903) is weaker still: its "round trip" asserts
`PROXY_TOOL_PROMPT_GUIDELINE.split_whitespace().join(" ").to_lowercase() == PROXY_TOOL_PROMPT_GUIDELINE`,
which is true of *any* already-lowercase single-spaced string and proves nothing about the
sanitizer.

The failure mode is inverted from the obvious guess, and that is why the weak tests are not enough:
`should_keep_guideline` is `guideline_keep_rule(..).unwrap_or(true)` (sanitize/tools.rs:133-135), so
a bullet matching no rule is **always kept**. A one-character drift does not delete guidance — it
silently disables the gate, leaving `use mcp …` in the system prompt after the `mcp` tool has been
taken away.

### MCP-151 · The `mcp` tool's JSON Schema — high, `host-verb`, implemented

**Obligation unmet (code-health).** Two byte-equivalent schema literals: `mcp_tool_schema()`
(proxy/tool.rs:53-81, `OnceLock`) and `proxy_tool_parameters()` (registration.rs:1675-1735, rebuilt
per call). The registered tool takes the second (registration.rs:1658, :2179). Per F5 the strong
test guards the first and no test compares them. Five property names — `tool`, `server`, `connect`,
`describe`, `search` — are a cross-crate permission contract:
`cyrup_permission_system::manager::create_mcp_permission_targets`
([manager.rs:1021-1061](../../crates/cyrup-permission-system/src/manager.rs)) reads exactly those
five, in that precedence.

### MCP-152 · `buildProxyDescription` — high, `hand-written`, implemented

**Obligation unmet (code-health).** Two builders: `proxy::build_proxy_description`
(description.rs:139-284, live metadata, `&IndexMap<String, CachedServerEntry>`) and
`registration::build_proxy_description` (registration.rs:1271-1426, cold cache,
`Option<&MetadataCache>`). Production calls **only** the second (registration.rs:2177). The head
literal is duplicated verbatim at description.rs:145-148 and registration.rs:1285-1288; the
nine-line usage block at description.rs:273-284 and registration.rs:1409-1424. See **F7** for what
is and is not already guarded, and for why the fix is two shared constants plus an empty-config
whole-string equality — not a multi-server cross-builder golden.

### MCP-171 · The `localeCompare` tie-break — low, `open-decision`, implemented

**Obligation unmet (performance residue plus one missing stability assertion).** See **F8** for the
two corrections. The existing ordering assertions
([config.rs:5097-5117](../../crates/cyrup-mcp/src/config.rs)) all pass a *fresh* collator every
time, so they say nothing about a reused one — which is exactly what the hoist introduces.

### MCP-269 · MCP credentials never reach `auth.json` — medium, `hand-written`, partial

**Obligation unmet.** No guard exists; the crate flags it itself as `TODO(MCP-269)` at
credentials.rs:80-82. See **F3** for the corrected form of the two verify clauses and **F6** for
where they belong.

### MCP-278 · The storage acceptance suite — medium, `hand-written`, partial

**Obligation unmet.** The in-process cases are ported. The **two subprocess cases** are not:

1. *routes revoked Linux keyring operations through the recovery helper*
   ([__tests__/mcp-auth-storage.test.ts:243-293](../../tmp/pi-mcp-adapter/__tests__/mcp-auth-storage.test.ts))
   — the positive path through `should_attempt_recovery` (credentials.rs:1298-1305) into
   `LinuxKeyringRecoveryStore` (credentials.rs:2642, :2665, :2793).
2. *does not use the recovery helper for generic secure-store failures*
   (mcp-auth-storage.test.ts:295-308) — the negative twin, with a fake `keyctl` exiting 99 and an
   assertion that the fake keyring store was never created.

The fixture `keyctl` argv contract (`$1 == "session"`, `$2 == "-"`, exit 64 otherwise, `shift 2`,
`exec "$@"`) is documented at credentials.rs:1401-1406 and is *the only thing that pins MCP-260's
argv shape*. Nothing in the tree spawns it today. The one-line JSON wire format
(`KeyringHelperRequest`, credentials.rs:1318-1333; `KeyringHelperResponse`, :1336-1348) is likewise
unexercised end to end.

### MCP-283 · The cache acceptance suite — medium, `hand-written`, partial

**Obligation unmet.** Upstream's cache file
([__tests__/mcp-auth-cache.test.ts](../../tmp/pi-mcp-adapter/__tests__/mcp-auth-cache.test.ts)) has
five harness/selector cases (:55-104, all ported or N/A) and ten behavioural ones. Missing:

| # | upstream case | why the existing tests miss it |
|---|---|---|
| a | :204 *normalizes publication exactly as a later store reload does* | only the generic `unknown_keys_are_dropped_not_rejected` (credentials.rs:3719-3731) exists, and it never goes through `publish_to_cache` (:2377-2392), which **re-parses the payload it just wrote** |
| b | :186-192 *reconstructs chunked entries once* (the second half of the :177 case) | `a_large_credential_round_trips_through_chunks` (:3835-3863) reads exactly once, and that read is a **cache hit** (`save_auth_entry` published), so `read_chunked_auth_entry` never runs on the ordinary read path at all |
| c | :123 *…updates…* (`updateTokens` refreshes the published value) | nothing calls `update_credentials` (:2891-2900) and re-reads at zero backend cost |
| d | :229-243 *reloads externally changed credentials* | `invalidation_reloads_and_evicts_only_its_target` (:4174-4191) counts reads but never asserts the **value** changed, so an eviction that reloads and then still serves the stale clone would pass |
| e | :251-266 *…and is harmless while disabled* (second half) + :268 *evicts a removed credential even when the gate is turned off* | see **F4** — the test named for the latter builds a cache-**enabled** fixture, and nothing calls `invalidate_cache` with the gate off |

Additionally, `a_returned_entry_is_isolated_from_the_cached_one` (:4193-4213) covers only the
*hit-copy* direction of upstream's :137 case. Two directions are untested and both are guaranteed
only by `publish_to_cache`'s re-parse: the clone handed back from a **miss**-populated read, and the
caller's own `&mut AuthEntry` after `save_auth_entry` returns.

### MCP-287 · The subprocess timeout path and the unreachable ladder rung — medium, `hand-written`, partial

**Obligation unmet.** The six rungs are implemented and correctly ordered (rung 1 at
credentials.rs:1446-1451 and :1504-1507, rung 2 at :1531-1541, rung 3 at :1543-1550, rung 4 at
:1552-1567, rung 5 at :1568-1577, rung 6 at :1579-1586). None of the three fixtures MCP-287 names
exists. See **F2** for the string to assert.

**Additional hazard, found while reading, and it changes the fixture.** The timeout branch kills the
direct child and then **joins the stdout reader thread** (credentials.rs:1494-1502). The read end
only sees EOF when every holder of the write end is gone, and `child.kill()` kills only the direct
child. The process chain is `Command::new(keyctl)` → *pid X* → `exec "$@"` → *pid X is now the
helper*. If the **helper** then forks (`#!/bin/sh` + `sleep 30`, no `exec`), the `sleep` inherits
the stdout pipe, survives the kill, and the 10 s timeout silently becomes 30 s. **Both** scripts
must `exec`: the `keyctl` fixture does `exec "$@"` (which is also the argv contract) and the
hung-helper fixture must be `#!/bin/sh` + `exec sleep 30`. The test asserts an elapsed-time window
so a regression here fails loudly rather than hanging.

---

## Implementation

### 1 · Harness A — make `FakeEnv` scriptable ([`crates/cyrup-mcp/src/proxy/testsupport.rs`](../../crates/cyrup-mcp/src/proxy/testsupport.rs))

Add beside the `FakeEnv` struct (testsupport.rs:40-56). `ProxyCallError` (env.rs:88-106) is
`#[derive(Debug)]` only — not `Clone` — so script a clonable recipe and mint the error at the seam.

```rust
/// A scripted [`ProxyCallError`]. `ProxyCallError` is not `Clone`, so the fake holds a recipe and
/// mints one error per call — which is also what lets `read_resource` and `call_tool` share it.
#[derive(Debug, Clone)]
pub(crate) enum CallFault {
    /// `session-recovery.ts`'s `SessionRecoveryAuthRequiredError`, with `error.authMessage`
    /// present or absent — `catch_arm`'s first arm (call.rs:851).
    AuthRequired { auth_message: Option<String> },
    /// rmcp's `UrlElicitationRequiredError`, carrying the opaque detail (call.rs:860).
    UrlElicitation { detail: String },
    /// Anything else — `is_abort_error` says false ⇒ `call_failed` (call.rs:885-889).
    Other(String),
    /// `McpError::Aborted`, the arm that turns into `aborted` rather than `call_failed`.
    Aborted(String),
    /// **The production shape.** Flip the connection to `NeedsAuth` — which is what the manager
    /// does on a mid-request 401 — then delegate to [`AuthRecovery::recover`] and propagate its
    /// error. This is the ONLY route by which `latch.attempted()` is `true` at call.rs:857.
    ViaRecovery,
}

impl CallFault {
    fn into_error(self, server: &str) -> ProxyCallError {
        match self {
            CallFault::AuthRequired { auth_message } => {
                ProxyCallError::SessionRecoveryAuthRequired {
                    server: server.to_string(),
                    auth_message,
                }
            }
            CallFault::UrlElicitation { detail } => {
                ProxyCallError::UrlElicitationRequired { detail }
            }
            CallFault::Other(message) => ProxyCallError::Other(McpError::other(message)),
            CallFault::Aborted(reason) => ProxyCallError::Other(McpError::Aborted(reason)),
            // Handled at the seam, never here.
            CallFault::ViaRecovery => ProxyCallError::Other(McpError::other("unreachable")),
        }
    }
}
```

New `FakeEnv` fields. Every one is `Default`-constructible, so `#[derive(Default)]` at
testsupport.rs:40 keeps working and **no existing test changes**:

```rust
    /// When set, `call_tool` and `read_resource` fail with this instead of succeeding.
    pub(crate) call_fault: Mutex<Option<CallFault>>,
    /// `manager.handleUrlElicitationRequired`'s verdict; `None` keeps today's `Accept`.
    pub(crate) elicitation_action: Mutex<Option<UrlElicitationAction>>,
    /// When set, `guard_mcp_output` returns it as `GuardedOutput::output_guard` — the spill that
    /// makes `catch_arm` substitute the truncation message (call.rs:894-900).
    pub(crate) guard_spill: Mutex<Option<Value>>,
    /// `completeAuthFromInput`'s answer. `None` keeps today's `"authenticated"`.
    pub(crate) complete_auth_status: Mutex<Option<String>>,
    /// When set, `complete_auth_from_input` fails with this — auth.rs:161-166's arm.
    pub(crate) complete_auth_fails: Mutex<Option<String>>,
    /// Every `input` `complete_auth_from_input` was handed, in order — MCP-168's three-key proof.
    pub(crate) complete_auth_inputs: Mutex<Vec<String>>,
```

Builders beside the existing ones (testsupport.rs:58-88), same by-value `self` style:

```rust
    pub(crate) fn with_call_fault(self, fault: CallFault) -> Self {
        *self.call_fault.lock().unwrap() = Some(fault);
        self
    }
    pub(crate) fn with_elicitation_action(self, action: UrlElicitationAction) -> Self {
        *self.elicitation_action.lock().unwrap() = Some(action);
        self
    }
    pub(crate) fn with_guard_spill(self, guard: Value) -> Self {
        *self.guard_spill.lock().unwrap() = Some(guard);
        self
    }
    pub(crate) fn with_auth_complete_status(self, status: &str) -> Self {
        *self.complete_auth_status.lock().unwrap() = Some(status.to_string());
        self
    }
    pub(crate) fn with_auth_complete_failure(self, message: &str) -> Self {
        *self.complete_auth_fails.lock().unwrap() = Some(message.to_string());
        self
    }
```

Rewrite exactly five trait methods; leave the other twenty-eight alone.

```rust
    async fn call_tool(
        &self,
        server: &str,
        _tool: &str,
        _arguments: JsonMap<String, Value>,
        recovery: &AuthRecovery<'_>,
        _cancel: &CancelToken,
    ) -> Result<CallToolOutcome, ProxyCallError> {
        match self.call_fault.lock().unwrap().clone() {
            // The manager marks the connection `needs-auth` on a mid-request 401, THEN asks the
            // recovery ladder. Reproducing that order is what keeps `recover`'s `Connected`
            // short-circuit (call.rs:85-87) from swallowing the test.
            Some(CallFault::ViaRecovery) => {
                self.connections
                    .lock()
                    .unwrap()
                    .insert(server.to_string(), ConnectionStatus::NeedsAuth);
                recovery.recover().await?;
                Ok(CallToolOutcome::default())
            }
            Some(fault) => Err(fault.into_error(server)),
            None => Ok(CallToolOutcome::default()),
        }
    }
    async fn read_resource(
        &self,
        server: &str,
        _uri: &str,
        recovery: &AuthRecovery<'_>,
        _cancel: &CancelToken,
    ) -> Result<Vec<Content>, ProxyCallError> {
        match self.call_fault.lock().unwrap().clone() {
            Some(CallFault::ViaRecovery) => {
                self.connections
                    .lock()
                    .unwrap()
                    .insert(server.to_string(), ConnectionStatus::NeedsAuth);
                recovery.recover().await?;
                Ok(Vec::new())
            }
            Some(fault) => Err(fault.into_error(server)),
            None => Ok(Vec::new()),
        }
    }
    async fn handle_url_elicitation_required(
        &self,
        _server: &str,
        _detail: &str,
    ) -> UrlElicitationAction {
        self.elicitation_action.lock().unwrap().unwrap_or(UrlElicitationAction::Accept)
    }
    async fn complete_auth_from_input(
        &self,
        _server: &str,
        input: &str,
        _cancel: &CancelToken,
    ) -> McpResult<String> {
        self.complete_auth_inputs.lock().unwrap().push(input.to_string());
        if let Some(message) = self.complete_auth_fails.lock().unwrap().clone() {
            return Err(McpError::other(message));
        }
        Ok(self
            .complete_auth_status
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "authenticated".to_string()))
    }
    async fn guard_mcp_output(
        &self,
        content: Vec<Content>,
        _options: OutputGuardOptions,
    ) -> GuardedOutput {
        GuardedOutput {
            content,
            output_guard: self.guard_spill.lock().unwrap().clone(),
            ..GuardedOutput::default()
        }
    }
```

`UrlElicitationAction` is `Copy` (env.rs:109-117), so `Option<UrlElicitationAction>::unwrap_or`
moves out of the guard exactly the way `self.approval.lock().unwrap().unwrap_or(…)` already does at
testsupport.rs:210. The module's `#![allow(clippy::unwrap_used, …)]` at testsupport.rs:5 already
covers every `unwrap` above.

### 2 · MCP-165 — the three arms, in [`proxy/call.rs`](../../crates/cyrup-mcp/src/proxy/call.rs)'s test module (after :1068)

Six `#[tokio::test]`s. Every fixture is the shape already used by
`an_empty_success_falls_back_to_the_placeholder_and_keeps_the_identity` (call.rs:1057-1067):
`config_with(&[("srv", stdio("a"))])`, `FakeEnv::default().with_connection("srv",
ConnectionStatus::Connected)`, metadata `&[("srv", vec![ToolMetadata::new("srv_run", "run", "")])]`,
then `execute_call(&ctx, "srv_run", None, None, &CancelToken::new(), None)`. Import `CallFault` from
`crate::proxy::testsupport`.

* **`the_auth_required_arm_carries_the_message_and_the_latch_flag`** —
  `CallFault::AuthRequired { auth_message: Some("token expired".into()) }`; assert
  `details["error"] == "auth_required"`, `text_of(&result) == "token expired"`,
  `details["message"] == "token expired"`, and — the key nothing else in the crate asserts —
  `details["autoAuthAttempted"] == json!(false)`. Repeat with `auth_message: None` and assert the
  text is `crate::proxy::results::default_auth_required_message("srv")`, which is the
  `unwrap_or_else` at call.rs:852-853 routed through `get_auth_required_message`.
* **`a_mid_request_401_runs_the_recovery_ladder_and_latches`** — the production shape.
  `auto_auth_on(config_with(&[("linear", http("https://linear.example/mcp"))]))` (reuse the helper's
  shape from auth.rs:453-456), metadata
  `&[("linear", vec![ToolMetadata::new("linear_issues", "issues", "")])]`, env
  `FakeEnv::default().with_connection("linear", ConnectionStatus::Connected)
  .with_oauth("linear.example").with_call_fault(CallFault::ViaRecovery)`. `ctx_with` builds a ctx
  with `ui: None`, so `has_ui()` is false and `attempt_auto_auth` step 4 (auth.rs:254-256) returns
  `Failed(get_auth_required_message(..))` **without** calling `authenticate`. Assert:
  ```rust
  assert_eq!(details["error"], json!("auth_required"));
  assert_eq!(details["autoAuthAttempted"], json!(true), "the ladder ran and latched");
  assert_eq!(text_of(&result), default_auth_required_message("linear"));
  assert_eq!(env.authenticate_calls.load(Ordering::SeqCst), 0, "step 4 refuses before authenticate");
  ```
  This is the only test in the crate that reaches `AuthRecovery::recover` (call.rs:83-121) at all.
* **`the_url_elicitation_arm_renders_one_message_per_action`** — table-drive all three
  `UrlElicitationAction`s with
  `.with_call_fault(CallFault::UrlElicitation { detail: "open me".into() })` and
  `.with_elicitation_action(action)`:

  | action | exact message |
  |---|---|
  | `Accept` | `The original MCP tool did not run. Complete the opened browser interaction, then retry the tool.` |
  | `Decline` | `The URL interaction was declined.` |
  | `Cancel` | `The URL interaction was cancelled.` |

  Assert `details["error"] == "url_elicitation_required"` and `details["action"] == action.as_str()`
  in all three, and that `details.get("autoAuthAttempted").is_none()` — this arm carries no latch key.
* **`a_plain_failure_is_call_failed_and_an_abort_is_aborted`** — `CallFault::Other("boom")` ⇒
  `details["error"] == "call_failed"` and `details["message"] == "boom"`;
  `CallFault::Aborted("stop")` ⇒ `details["error"] == "aborted"` and `details["message"] == "stop"`.
  That is `is_abort_error` ([abort.rs:140-147](../../crates/cyrup-mcp/src/abort.rs)) driving
  call.rs:885-889.
* **`a_guard_spill_replaces_the_message_with_the_truncation_sentence`** — `CallFault::Other("boom")`
  **plus** `.with_guard_spill(json!({"fullOutputPath": "/tmp/x"}))`; assert
  `details["message"] == "output truncated; see outputGuard.fullOutputPath"` (call.rs:896) and that
  `details["outputGuard"] == json!({"fullOutputPath": "/tmp/x"})` (`GuardedOutput::write_details`,
  env.rs:230-239). Also assert `details["error"] == "call_failed"` — the spill must not change the
  code.
* **`the_resource_path_reaches_the_same_catch_arm`** — build the resource tool the way
  `approval_failures_report_tool_not_resource_uri` (call.rs:1013) does
  (`resource.resource_uri = Some("file:///notes.md".to_string())`), script
  `CallFault::AuthRequired { auth_message: Some("expired".into()) }`, and assert
  `details["error"] == "auth_required"` **and** `details["resourceUri"] == json!("file:///notes.md")`
  with no `tool` key — proving the second `catch_arm` call site (call.rs:768) is live and that
  `call_identity` (call.rs:125-140) is spread on it.

### 3 · MCP-168 — the two named tests

**In [`proxy/tool.rs`](../../crates/cyrup-mcp/src/proxy/tool.rs)'s test module**, beside the other
`McpTool::execute` drivers (tool.rs:617+). `execute_owner` is `None` when `set_owner` was never
called, so the generation fence (tool.rs:420-422) is skipped and `InitPhase::Ready(ctx)` is enough.

```rust
    #[tokio::test]
    async fn all_three_auth_complete_input_keys_reach_the_seam_in_precedence_order() {
        use crate::proxy::testsupport::{config_with, ctx_with, http};

        let config = config_with(&[("linear", http("https://linear.example/mcp"))]);
        let (ctx, env) = ctx_with(config, &[], &[], FakeEnv::default());
        let (_keep, rx) = tokio::sync::watch::channel(InitPhase::Ready(ctx));
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);

        // `index.ts:856` `parsedArgs?.redirectUrl ?? parsedArgs?.code ?? parsedArgs?.input`
        // (tool.rs:447-455). Each key ALONE reaches the seam carrying its own value.
        for (key, value) in [("redirectUrl", "http://cb?code=a"), ("code", "b"), ("input", "c")] {
            let result = tool
                .execute(
                    ToolCallId::from("call-1"),
                    json!({"action": "auth-complete", "server": "linear", "args": {key: value}}),
                    CancelToken::new(),
                    Box::new(|_| {}),
                )
                .await
                .expect("auth-complete returns an envelope, never an Err");
            assert_eq!(
                result.details.clone().unwrap()["authenticated"],
                json!(true),
                "key = {key}"
            );
        }
        // …and all three AT ONCE records only `redirectUrl`'s — the `or_else` chain, not a merge.
        let _ = tool
            .execute(
                ToolCallId::from("call-2"),
                json!({"action": "auth-complete", "server": "linear",
                       "args": {"input": "c", "code": "b", "redirectUrl": "http://cb?code=a"}}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("envelope");
        assert_eq!(
            *env.complete_auth_inputs.lock().unwrap(),
            vec![
                "http://cb?code=a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "http://cb?code=a".to_string(),
            ]
        );
    }
```

**In [`proxy/auth.rs`](../../crates/cyrup-mcp/src/proxy/auth.rs)'s test module**, directly after
`auth_complete_closes_the_connection_and_clears_the_failure` (auth.rs:615):

```rust
    /// `proxy-modes.ts:414-419` — a status other than `"authenticated"` is echoed, and NONE of the
    /// success side-effects (close, clearFailure, updateStatusBar — auth.rs:174-176) run.
    #[tokio::test]
    async fn a_non_authenticated_status_is_echoed_and_leaves_the_connection_open() {
        let config = config_with(&[("linear", http("https://linear.example/mcp"))]);
        let env = FakeEnv::default()
            .with_connection("linear", ConnectionStatus::Connected)
            .with_failure("linear", 5)
            .with_auth_complete_status("pending");
        let (ctx, env) = ctx_with(config, &[], &[], env);
        let result =
            execute_auth_complete(&ctx, "linear", "http://cb?code=x", &CancelToken::new())
                .await
                .unwrap();
        let details = result.details.clone().unwrap();
        assert_eq!(details["error"], json!("not_authenticated"));
        assert_eq!(details["status"], json!("pending"), "the status is echoed, not swallowed");
        assert_eq!(details["server"], json!("linear"));
        assert!(details.get("authenticated").is_none());
        assert_eq!(text_of(&result), "OAuth authentication did not complete for \"linear\".");
        // The success arm's three side-effects did NOT run.
        assert_eq!(env.get_connection("linear"), Some(ConnectionStatus::Connected));
        assert_eq!(env.failure_age_seconds("linear"), Some(5));
    }

    /// `proxy-modes.ts:427-432` — the catch arm, reachable for the first time now that the fake can
    /// fail.
    #[tokio::test]
    async fn a_throwing_complete_auth_reports_auth_complete_failed_with_the_message() {
        let config = config_with(&[("linear", http("https://linear.example/mcp"))]);
        let env = FakeEnv::default().with_auth_complete_failure("callback rejected");
        let (ctx, _) = ctx_with(config, &[], &[], env);
        let result =
            execute_auth_complete(&ctx, "linear", "http://cb?code=x", &CancelToken::new())
                .await
                .unwrap();
        let details = result.details.clone().unwrap();
        assert_eq!(details["error"], json!("auth_complete_failed"));
        assert_eq!(details["message"], json!("callback rejected"));
        assert_eq!(
            text_of(&result),
            "Failed to complete OAuth for \"linear\": callback rejected"
        );
    }
```

> Transcribe that last sentence from auth.rs:165 verbatim when you write it — the source is
> `format!("Failed to complete OAuth for \"{server_name}\": {message}")`. Assert what the source
> produces; if the two disagree, the source wins (this unit is `implemented`, so a difference is a
> transcription error, not a bug to fix).

### 4 · Harness C1 — MCP-151, collapse to one schema literal

In [`registration.rs`](../../crates/cyrup-mcp/src/registration.rs):

* Drop the `parameters: Value` field from `ProxyTool` (:1645) and its initialiser (:1658).
* `fn parameters(&self) -> &Value` (:1745-1747) becomes `crate::proxy::mcp_tool_schema()` — that
  returns `&'static Value`, which coerces to the `&Value` the trait wants.
* Replace the body of `proxy_tool_parameters()` (:1675-1735) with
  `crate::proxy::mcp_tool_schema().clone()`. **Keep the doc comment** — the five permission-relevant
  names are documented there and at proxy/tool.rs:43-45, and this is the copy that names
  `create_mcp_permission_targets` — and add a sentence saying the literal now lives in
  `proxy::mcp_tool_schema`, so the twelve-property assertion at proxy/tool.rs:520-549 guards the
  schema **both** tools serve. Note F9 there in one clause: neither tool is wired to a dispatch yet,
  which is exactly why collapsing beats moving the test.
* Keep `the_proxy_schema_keeps_the_five_permission_relevant_names` (:2875-2887) unchanged: it is now
  a cross-module reachability assertion rather than a second, weaker snapshot.

### 5 · Harness C2 — MCP-152, two shared literals plus a whole-string empty-config guard

Both builders keep their own shape (their cache and spec types genuinely differ — see **F7**); only
the two **fixed** literals are shared. Add them to
[`proxy/constants.rs`](../../crates/cyrup-mcp/src/proxy/constants.rs) beside
`INSTRUCTIONS_SNIPPET_LENGTH` (:29):

```rust
/// `direct-tools.ts:240`'s header, with the single `Pi` → `cyrup` rebrand and the `mcpScript`
/// sentence cut (Cut 4). ONE literal, because the description is built twice — from the cold cache
/// ([`crate::registration::build_proxy_description`]) and from live metadata
/// ([`crate::proxy::build_proxy_description`]) — and `McpExtension::proxy_tool_description`
/// re-registers only when the text CHANGED, so a one-word difference between the copies makes the
/// guard misfire on every reconnect and invalidates the provider's prompt-cache prefix. That has
/// happened once already (registration.rs:1276-1283).
pub const PROXY_DESCRIPTION_HEAD: &str =
    "MCP gateway — server status, tool search/describe, auth, and single MCP tool calls. Non-MCP cyrup tools should be called directly, not through mcp.\n";

/// `direct-tools.ts`'s fixed usage block, minus the `ui-messages` line (Cut 2). Byte-exact,
/// including the two-space indent, the `→` glyph and the ABSENCE of a trailing newline on the
/// final `Mode:` line. Shared for the same reason as [`PROXY_DESCRIPTION_HEAD`].
pub const PROXY_DESCRIPTION_USAGE: &str = concat!(
    "\nUsage:\n",
    // …the nine lines, transcribed verbatim from description.rs:275-283…
    "\nMode: action > tool (call) > connect > describe > instructions > search > server (list) > nothing (status)",
);
```

Then `desc.push_str(PROXY_DESCRIPTION_HEAD)` replaces description.rs:145-148 and
registration.rs:1285-1288, and `desc.push_str(PROXY_DESCRIPTION_USAGE)` replaces the ten `push_str`
calls at description.rs:274-283 and the eleven at registration.rs:1410-1424.

Upgrade `both_proxy_descriptions_share_one_head_line` (registration.rs:3070-3086) — rename it, and
compare the whole string:

```rust
    /// The gateway description is built twice — from the disk cache here, and from live metadata in
    /// [`crate::proxy::build_proxy_description`]. For an EMPTY config blocks 2-5 all render empty,
    /// so this compares exactly the two literals that used to be duplicated: the head and the
    /// nine-line usage block. A multi-server comparison would NOT be equivalent — the two builders
    /// filter their caches differently (`valid_entry` here, a bare `get` there) — and is covered
    /// per-builder by `proxy_description_golden_for_two_servers` (:2756) and
    /// `proxy_description_renders_every_block_in_order` (proxy/description.rs:311).
    #[test]
    fn both_proxy_descriptions_are_byte_identical_for_an_empty_config() {
        let from_cache = build_proxy_description(&McpConfig::default(), None, &[]);
        let from_live = crate::proxy::build_proxy_description(
            &McpConfig::default(),
            &indexmap::IndexMap::new(),
            &[],
        );
        assert_eq!(from_cache, from_live);
        assert_eq!(
            from_cache,
            format!(
                "{}{}",
                crate::proxy::PROXY_DESCRIPTION_HEAD,
                crate::proxy::PROXY_DESCRIPTION_USAGE
            ),
            "an empty config renders exactly the two shared literals and nothing else"
        );
    }
```

Then close F7's remaining hole in
[`proxy/description.rs`](../../crates/cyrup-mcp/src/proxy/description.rs)'s
`proxy_description_renders_every_block_in_order` (:311-367): replace the `ends_with(<Mode: line>)` +
`matches('→').count() == 9` pair at :364-367 with
`assert!(description.ends_with(PROXY_DESCRIPTION_USAGE))`, keeping the
`!description.contains("ui-messages")` assertion. That pins all nine lines on the proxy side for the
first time. Leave registration.rs:2756-2790's golden literal spelled out in full — a golden that
quotes the constant it is guarding guards nothing.

### 6 · Harness C3 — MCP-192, one guideline and a real sanitizer round trip

In [`registration.rs`](../../crates/cyrup-mcp/src/registration.rs), correct the stale doc claim at
:156-161 (it says two occurrences; a case-insensitive tree-wide grep for `discovery first` returns
five across three files) and re-point the constant at the single source of truth:

```rust
pub const PROXY_TOOL_PROMPT_GUIDELINE: &str = crate::proxy::MCP_TOOL_GUIDELINE;
```

This forces the two tools to advertise one string. `MCP_TOOL_GUIDELINE` is mixed-case
(`"Use mcp for MCP discovery first: …"`, proxy/constants.rs:101-102) and the sanitizer lowercases
before matching (`normalize_guideline_text`, sanitize/tools.rs:89-97), so the change is
behaviour-preserving — but the lowercase "round trip" at registration.rs:2899-2902 becomes false and
must go. Replace those four lines with the reachability assertion that is actually worth having:

```rust
        assert_eq!(
            PROXY_TOOL_PROMPT_GUIDELINE, crate::proxy::MCP_TOOL_GUIDELINE,
            "one literal; the sanitizer round trip is proxy/constants.rs's test"
        );
```

Then replace `guideline_normalises_to_the_sanitizer_key`
([proxy/constants.rs:111-126](../../crates/cyrup-mcp/src/proxy/constants.rs)) with a test that runs
the **actual** sanitizer. `cyrup-mcp` already depends on `cyrup-permission-system`
([Cargo.toml:45](../../crates/cyrup-mcp/Cargo.toml), a normal dependency already used in production
at config.rs:1785, and there is no reverse edge), and `sanitize_available_tools_section` is `pub` in
a `pub mod` ([sanitize/tools.rs:181](../../crates/cyrup-permission-system/src/sanitize/tools.rs),
`sanitize/mod.rs:15`, `lib.rs:89`):

```rust
    #[test]
    fn the_guideline_is_gated_by_the_real_sanitizer_when_mcp_is_denied() {
        use cyrup_permission_system::sanitize::tools::sanitize_available_tools_section;

        let prompt = format!(
            "Intro.\n\nGuidelines:\n- {MCP_TOOL_GUIDELINE}\n- use write only for new files or complete rewrites\n\nEnd:\nfin"
        );

        // `mcp` exposed ⇒ the bullet survives. NOTE: `guideline_keep_rule` returning `Some(true)`
        // and returning `None` are INDISTINGUISHABLE here, which is why the denied case below is
        // the assertion that actually catches a drift (13d MCP-192).
        let kept =
            sanitize_available_tools_section(&prompt, &["mcp".to_string(), "write".to_string()]);
        assert!(kept.prompt.contains(MCP_TOOL_GUIDELINE));

        // `mcp` denied ⇒ the bullet is GONE. If this literal ever drifts from
        // `cyrup-permission-system/src/sanitize/tools.rs:47`, `unwrap_or(true)` keeps the bullet
        // and this line fails — which is the whole point of the unit.
        let denied = sanitize_available_tools_section(&prompt, &["write".to_string()]);
        assert!(denied.removed);
        assert!(
            !denied.prompt.to_lowercase().contains("mcp discovery first"),
            "a denied `mcp` must take its guideline with it; got:\n{}",
            denied.prompt
        );
        // The unrelated bullet is untouched — proves the section was FILTERED, not deleted.
        assert!(denied.prompt.contains("use write only for new files or complete rewrites"));
        assert_eq!(MCP_TOOL_NAME, "mcp");
    }
```

Do **not** add a `cyrup-it` extension test for this. 13d MCP-192's verify asks for one, but
`sanitize::tools`' own module doc (sanitize/tools.rs:1-4) records that it is *"pure string logic
with ZERO host/policy dependency (it takes the already-computed exposed-tool set)"*, and
`crates/cyrup-it/tests/permission/context_hygiene.rs:122-146` already drives that wiring end to end
for `write`/`read`. Routing `mcp` through a live extension would re-assert the wiring, not the
literal, and the literal is the thing that drifts.

### 7 · MCP-171 — hoist the collator ([`crates/cyrup-mcp/src/config.rs`](../../crates/cyrup-mcp/src/config.rs))

Replace config.rs:4047-4051 and correct the doc paragraph at :4042-4046, which is wrong twice (F8):

```rust
thread_local! {
    /// One collator per thread. `feruca::Collator::collate` takes `&mut self` because it reuses
    /// four scratch buffers (`feruca-0.12.0` `src/collator.rs:108-123`), so this is a `RefCell`
    /// rather than a shared value — and thread-local rather than a global lock, because the callers
    /// are sort comparators and a mutex would serialise them.
    static COLLATOR: std::cell::RefCell<feruca::Collator> = std::cell::RefCell::new(new_collator());
}

/// The one collator configuration this workspace uses: CLDR-root tailoring, non-ignorable variable
/// weighting, byte-value tie-break — proven against Node in `cyrup-tools/src/tools/ls.rs:128` and
/// `cyrup-config/src/model/resolver.rs:149`.
fn new_collator() -> feruca::Collator {
    feruca::Collator::new(feruca::Tailoring::default(), false, true)
}

#[must_use]
pub fn locale_compare(left: &str, right: &str) -> Ordering {
    // `try_with` (TLS may already be destroyed at shutdown) and `try_borrow_mut` (`collate` cannot
    // re-enter this function, so the borrow can never actually conflict) both degrade to a fresh
    // collator rather than panicking — the workspace denies `panic`/`unwrap`/`expect`.
    COLLATOR
        .try_with(|cell| match cell.try_borrow_mut() {
            Ok(mut collator) => collator.collate(left, right),
            Err(_) => new_collator().collate(left, right),
        })
        .unwrap_or_else(|_| new_collator().collate(left, right))
}
```

Rewrite the "not on any hot path" sentence to the truth: `rank_collate`
([proxy/ranking.rs:324-326](../../crates/cyrup-mcp/src/proxy/ranking.rs)) is this function, and it
is the comparator of three sorts — `rank_tool_matches` (ranking.rs:378), the empty-query sort
([proxy/discovery.rs:497](../../crates/cyrup-mcp/src/proxy/discovery.rs)) and the connecting-server
hint list (discovery.rs:526) — so an O(n log n) sort used to build O(n log n) collators, two heap
allocations each. `config.rs:98` already imports `std::cmp::Ordering`.

`locale_compare_orders_case_the_way_icu_does` (config.rs:5097-5117) must pass **unchanged** — that
is the proof the hoist is behaviour-preserving. Then extend it with the assertion the hoist newly
needs, which is about **buffer reuse**, not memoisation:

```rust
        // The collator is now shared per thread and `collate` reuses four scratch buffers, one pair
        // of which starts at 64 entries and GROWS. A long comparison must not leave the buffers in
        // a state that changes a later short one, and repetition must be idempotent.
        let long = "a".repeat(4000);
        assert_eq!(locale_compare(&long, "z"), Ordering::Less);
        for _ in 0..3 {
            assert_eq!(locale_compare("a", "A"), Ordering::Less);
            assert_eq!(locale_compare("é", "z"), Ordering::Less);
            assert_eq!(locale_compare(&long, &long), Ordering::Equal);
        }
```

### 8 · Harness B — one new `cyrup-it` module and two fixture scripts

Add **one** `mod` line to [tests/mcp/main.rs](../../crates/cyrup-it/tests/mcp/main.rs) after :26 —
no `[[test]]` target, no new dependency (F1):

```rust
mod keyring_recovery;
```

Create `crates/cyrup-it/tests/mcp/keyring_recovery.rs`, gated `#![cfg(unix)]`. The fixture writer
follows the in-suite precedent at
[tests/subagents/run_state_signal_and_stop_parity.rs:109-124](../../crates/cyrup-it/tests/subagents/run_state_signal_and_stop_parity.rs)
(`std::fs::Permissions::from_mode(0o755)`), and the scratch tree is
[`support::scratch::Scratch`](../../crates/cyrup-it/tests/support/scratch.rs) (`write` at :68-77,
`dir` at :61-66).

**Nothing may touch `std::env`.** `KEYRING_RECOVERY_KEYCTL_ENV`, `KEYRING_RECOVERY_HELPER_ENV` and
`TEST_LINUX_KEYRING_RECOVERY_ENV` are `pub` (credentials.rs:190-210) and `credentials` is a
`pub mod` ([lib.rs:135](../../crates/cyrup-mcp/src/lib.rs)), so every switch is injected through
`EnvFn` (credentials.rs:220) — which is docs/TEST-ARCHITECTURE.md §4 R2 and the reason `EnvFn`
exists. The fixture store path is **baked into the script body**, not passed through the
environment, because the helper inherits the parent process's env and there is no sound way to set
one.

```rust
fn script(scratch: &Scratch, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = scratch.write(format!("fixtures/{name}"), body);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|e| panic!("chmod +x {}: {e}", path.display()));
    path
}

/// The fake `keyctl`. Upstream's own fixture, verbatim in shape
/// (`tmp/pi-mcp-adapter/__tests__/mcp-auth-storage.test.ts:249-255`): it PROVES the argv contract
/// documented at `crates/cyrup-mcp/src/credentials.rs:1401-1406`, and is the only thing in the tree
/// that does. `exec` matters twice: it is what upstream's fixture does, and it makes the helper the
/// DIRECT child, so the parent's `kill()` on timeout actually closes the stdout pipe.
const KEYCTL: &str = "#!/bin/sh\n\
[ \"$1\" = \"session\" ] || exit 64\n\
[ \"$2\" = \"-\" ] || exit 64\n\
shift 2\n\
exec \"$@\"\n";

/// An `EnvFn` answering only the three recovery switches. `CYRUP_*` names win the dual read
/// (credentials.rs:190-210), so answering index `[0]` is what production reads first and is
/// sufficient.
fn recovery_env(keyctl: &Path, helper: Option<&Path>) -> EnvFn {
    let keyctl = keyctl.to_string_lossy().into_owned();
    let helper = helper.map(|p| p.to_string_lossy().into_owned());
    Arc::new(move |key: &str| {
        if key == KEYRING_RECOVERY_KEYCTL_ENV[0] {
            Some(keyctl.clone())
        } else if key == TEST_LINUX_KEYRING_RECOVERY_ENV[0] {
            Some("1".to_string())
        } else if key == KEYRING_RECOVERY_HELPER_ENV[0] {
            helper.clone()
        } else {
            None
        }
    })
}
```

`…_KEYRING_RECOVERY_HELPER` names a **program, not a script token** (credentials.rs:1379-1386: an
override is exec'd directly with `args: Vec::new()`), so a shell script with a shebang is a legal
helper — which is what makes these tests possible while MCP-260's host half
(`crates/cyrup/src/mcp_keyring_helper_cmd.rs`) is still unbuilt. Confirmed unbuilt: that file does
not exist and nothing in `crates/cyrup/src` names `KEYRING_HELPER_SUBCOMMAND`.

#### 8a · The fake keyring helper, and why `sed` is exact here

The helper is a POSIX `sh` script backed by one file per account under a baked-in directory.
`serde_json` emits struct fields in declaration order, so `KeyringHelperRequest`
(credentials.rs:1318-1333) is always
`{"operation":"…","service":"…","account":"…"[,"payload":"…"]}` with `payload` **last** and omitted
entirely for `read`/`remove` (`skip_serializing_if = "Option::is_none"`). The stored value is the
JSON-**escaped** payload body, re-emitted verbatim inside `"value":"…"`, so the round trip is
lossless with no unescaping. Stored payloads never contain a newline — pinned at
credentials.rs:3856 — so one `read -r` line is the whole request. Account names are
`sha256-<64 hex>` and `sha256-<hex>.chunk.<digest>.<n>` (credentials.rs:761-770), all safe
filenames.

```rust
fn helper_body(store_dir: &Path) -> String {
    format!(
        "#!/bin/sh\n\
STORE='{}'\n\
mkdir -p \"$STORE\"\n\
read -r line\n\
op=$(printf '%s' \"$line\" | sed -n 's/^{{\"operation\":\"\\([a-z]*\\)\".*/\\1/p')\n\
acct=$(printf '%s' \"$line\" | sed -n 's/.*\"account\":\"\\([^\"]*\\)\".*/\\1/p')\n\
case \"$op\" in\n\
  read) if [ -f \"$STORE/$acct\" ]; then \
printf '{{\"ok\":true,\"found\":true,\"value\":\"%s\"}}\\n' \"$(cat \"$STORE/$acct\")\"; \
else printf '{{\"ok\":true,\"found\":false}}\\n'; fi ;;\n\
  write) printf '%s' \"$line\" | sed -n 's/.*\"payload\":\"\\(.*\\)\"}}$/\\1/p' > \"$STORE/$acct\"; \
printf '{{\"ok\":true}}\\n' ;;\n\
  remove) rm -f \"$STORE/$acct\"; printf '{{\"ok\":true}}\\n' ;;\n\
  *) printf '{{\"ok\":false,\"error\":\"bad op\"}}\\n'; exit 1 ;;\n\
esac\n",
        store_dir.display()
    )
}
```

The store directory must be a path the helper creates (`scratch.root().join("fake-keyring")`),
**not** one `Scratch::dir` pre-creates — its existence is the "did the helper run" signal.

#### 8b · MCP-278's two subprocess cases

* **`revoked_keyring_operations_are_routed_through_the_recovery_helper`** — build

  ```rust
  let store = McpAuthStore::with_backends(
      Arc::new(MemorySecretStore::with_fault(SimulatedFault::KeyRevoked)),
      Arc::new(MemorySecretStore::new()),
      McpDirs::new(scratch.agent_dir(), scratch.work()),
      AuthStorageOptions::default(),
      recovery_env(&keyctl, Some(&helper)),
  );
  ```

  (`with_backends` at credentials.rs:2086-2105; `SimulatedFault::KeyRevoked` raises a
  `NoStorageAccess("KeyRevoked")` chain the predicate matches, credentials.rs:1038-1041.) Then
  reproduce upstream's whole round trip (mcp-auth-storage.test.ts:283-292) with a **5000-character**
  access token, so the chunking layer is exercised across the hop:

  1. `store.save_auth_entry("recovered", &mut entry, Some("https://example.com/mcp"))` — the write
     fails at the backend and retries through `write_secure_auth_entry`'s recovery arm
     (credentials.rs:2652-2668).
  2. `store.reset_cache()`, then `store.auth_entry("recovered")` returns the same token — the read
     recovery arm (credentials.rs:2617-2624 / :2633-2649) reassembled the chunks through the helper.
  3. `store.remove_auth_entry("recovered")` (credentials.rs:2785-2798), then
     `store.auth_entry("recovered")` is `Ok(None)` and the fake keyring directory is **empty**.
  4. `assert!(fake_store_dir.exists())` — the helper ran; and assert no error anywhere mentions
     `exit code 64`, which is the argv assertion (`keyctl` exits 64 on any argv but `session -`).
* **`a_generic_store_failure_never_spawns_the_recovery_helper`** — the same fixture pair, but
  `SimulatedFault::Unavailable` (not revocation). Assert:

  ```rust
  let error = store.auth_entry("generic").expect_err("the backend is unavailable");
  assert_eq!(
      error.to_string(),
      "Failed to read OAuth credentials for generic from the OS secure credential store"
  );
  // The DISCRIMINATOR: had recovery run and failed, the source would be
  // `AuthSecretStoreError::Recovery(..)`. It ran not at all.
  let source = std::error::Error::source(&error).map(ToString::to_string).unwrap_or_default();
  assert!(
      !source.contains("Linux keyring recovery helper"),
      "should_attempt_recovery's AND must fail closed on a non-revocation error; got {source}"
  );
  assert!(!fake_store_dir.exists(), "the helper was never spawned");
  ```

  That is `should_attempt_recovery`'s conjunction (credentials.rs:1298-1305) proven behaviourally
  rather than by the in-crate predicate test at credentials.rs:4323-4353.

#### 8c · MCP-287's three ladder fixtures

Drive `LinuxKeyringRecoveryStore::new(AUTH_SECRET_SERVICE, recovery_env(..))`
(credentials.rs:1590-1610) directly through `AuthSecretStore::read` (credentials.rs:923), so the
assertion is on the rung message with no store wrapping in the way — `AuthSecretStoreError::Recovery`
renders `{0}` verbatim (credentials.rs:316-317).

* **`a_hung_helper_produces_the_rung_one_timeout_message_and_does_not_hang`** — helper body
  `"#!/bin/sh\nexec sleep 30\n"`. **The `exec` is load-bearing** (see MCP-287's hazard note): without
  it the `sleep` outlives `child.kill()` holding the stdout pipe, and the join at
  credentials.rs:1500-1502 blocks for the full 30 s. Record `Instant::now()`, call `read`, assert:

  ```rust
  assert_eq!(
      error.to_string(),
      format!(
          "Linux keyring recovery helper could not start: {} timed out after {} ms",
          keyctl.display(),
          KEYRING_RECOVERY_TIMEOUT_MS
      )
  );
  assert!(elapsed >= Duration::from_millis(KEYRING_RECOVERY_TIMEOUT_MS), "it must actually wait");
  assert!(elapsed < Duration::from_secs(20), "the stdout-reader join must not outlive the kill");
  ```

  The `{keyctl}` interpolation is `RecoveryInvocation::keyctl` — the resolved program **path string**
  from the env override, not the literal `"keyctl"` (credentials.rs:1356, :1504-1507). This case
  costs ~10 s of wall clock; the `it` target is off by default, so that is acceptable, but say so in
  the test's doc comment.
* **`an_error_reply_that_exits_one_reports_the_exit_code_not_the_helper_text`** — helper prints
  `{"ok":false,"error":"boom"}` and `exit 1`. Assert exactly
  `"Linux keyring recovery helper failed with exit code 1"` — rung 2 (credentials.rs:1531-1541)
  winning over rung 5, which is the whole point of the unit and the reason the real helper must
  exit 1.
* **`the_same_reply_at_exit_zero_reaches_rung_five`** — identical body, `exit 0`. Assert the message
  is exactly `"boom"` (credentials.rs:1568-1577). Add the empty-`error` variant
  (`{"ok":false,"error":""}`, exit 0) and assert the fallback
  `"Linux keyring recovery helper failed"`, which the `.filter(|m| !m.is_empty())` at :1573-1575
  produces.

### 9 · MCP-269's two guards — in [`credentials.rs`](../../crates/cyrup-mcp/src/credentials.rs)'s own test module (F6)

```rust
    /// (a) The SOURCE guard. `crates/cyrup-mcp` must never name `cyrup_config`'s credential store —
    /// the crate header states the design property in prose (credentials.rs:14, :80-82) and nothing
    /// enforced it. Precedent for the shape:
    /// `crates/cyrup-tui/src/tests/transcript_expand_wiring.rs:128-160`.
    ///
    /// The walk is RECURSIVE: `src/` holds `proxy/` since `ba75bbf`, and a flat `read_dir` guard
    /// would silently skip fourteen files.
    #[test]
    fn no_cyrup_mcp_source_file_reaches_cyrup_configs_auth_store() {
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let files = walk_files(&src);
        assert!(files.len() > 20, "the walk must reach src/proxy/; found {}", files.len());

        let mut offenders = Vec::new();
        for file in files.iter().filter(|p| p.extension().is_some_and(|x| x == "rs")) {
            let text = std::fs::read_to_string(file).expect("source file is readable");
            for (index, line) in text.lines().enumerate() {
                let code = line.trim();
                // The crate header DISCUSSES the decision at credentials.rs:14 and :2299.
                if code.starts_with("//") {
                    continue;
                }
                let names_auth_module = code.contains("cyrup_config::auth");
                let imports_auth = code.starts_with("use cyrup_config")
                    && code.split(&[':', '{', '}', ',', ' '][..]).any(|seg| seg == "auth");
                if names_auth_module || imports_auth {
                    offenders.push(format!("{}:{}: {code}", file.display(), index + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "MCP credentials must never reach `auth.json` (MCP-269): {offenders:#?}"
        );
    }
```

Match on `cyrup_config::auth` and on an `auth`-bearing `use cyrup_config…` **only**. A bare
`AuthStore::` predicate false-positives on this crate's own `McpAuthStore::`, `AuthStoreError::` and
`AuthStorageOptions` and must not be used.

```rust
    /// (b) The BEHAVIOURAL guard. `<agent_dir>/auth.json` is exactly what
    /// `cyrup_config::env::Env::auth_path` constructs
    /// (`crates/cyrup-config/src/env.rs:317-319`, `self.agent_dir.join("auth.json")`).
    #[test]
    fn a_full_mcp_credential_lifecycle_never_creates_auth_json() {
        const TOKEN: &str = "mcp-269-canary-access-token";

        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        let cwd = dir.path().join("work");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let store = McpAuthStore::with_backends(
            Arc::new(MemorySecretStore::new()),
            Arc::new(MemorySecretStore::new()),
            McpDirs::new(agent_dir.clone(), cwd.clone()),
            AuthStorageOptions::default(),
            Arc::new(|_| None),
        );

        let mut entry = AuthEntry {
            credentials: Some(credentials(TOKEN)),
            ..AuthEntry::default()
        };
        store.save_auth_entry("srv", &mut entry, Some("https://x.example/mcp")).unwrap();
        store.update_credentials("srv", credentials(TOKEN), None).unwrap();
        store.update_state("srv", state("csrf"), None).unwrap();
        store.remove_auth_entry("srv").unwrap();

        assert!(!agent_dir.join("auth.json").exists(), "cyrup_config::env::Env::auth_path");
        for root in [&agent_dir, &cwd] {
            for path in walk_files(root) {
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    !bytes.windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()),
                    "{} contains the token", path.display()
                );
            }
        }
    }
```

`credentials(..)` and `state(..)` are the module's existing helpers (credentials.rs:3656-3670).
Write **one** `fn walk_files(root: &Path) -> Vec<PathBuf>` (recursive, sorted for determinism) used
by both guards rather than writing the walk twice.

Then replace the `TODO(MCP-269)` bullet at credentials.rs:80-82 with a pointer to the two guards by
name.

### 10 · MCP-283's cache cases — [`credentials.rs`](../../crates/cyrup-mcp/src/credentials.rs), beside :4135-4273

All of these use the existing `test_store` (:3641-3654), `store_with_env` (:4122-4133),
`credentials` (:3656-3665) and `MemorySecretStore::{seed, read_count, entries}` (:1080-1103)
helpers. `AuthEntry` has no `PartialEq` and a hand-written `Debug` (credentials.rs:595), so compare
with `serde_json::to_value(&entry).unwrap()`.

* **(a)** `publication_normalizes_exactly_as_a_store_reload_does` — upstream
  mcp-auth-cache.test.ts:204-217. `backend.seed(&auth_entry_account("srv"),
  r#"{"serverUrl":"https://x.example/mcp","client":{"clientId":"c","redirectUris":["https://a.example"]},"futureKey":9}"#)`.
  Call `store.update_credentials("srv", credentials("t"), None)` — `mutate` (:2850-2858) reads,
  applies and calls `save_auth_entry`, which ends in `publish_to_cache` (:2377-2392), and **that
  re-parses the payload it just wrote**. Read once (cache hit), `store.reset_cache()`, read again
  (store reload), and assert the two `to_value`s are equal and that `backend.entries()` no longer
  carries `futureKey`. *Upstream's framing does not port literally* — `AuthEntry` has no unknown-key
  channel on the write side (:592-594) — so the porting-correct assertion is hit-path ≡ miss-path
  for the same entry, with `redirect_uris` present because it is the one field that degrades
  silently (:3753-3765) and therefore the one with something lossy to be right about.
* **(b)** `a_chunked_entry_is_reconstructed_once_then_served_from_the_cache` — upstream :186-192.
  Use the `SimulatedFault::SizeLimited` fixture from `:3836`, save a 5000-character credential,
  `store.reset_cache()`, read once, record `backend.read_count()`, read twice more and assert the
  count is **unchanged**. Assert the reassembled token still has length 5000. Note in the doc comment
  that `a_large_credential_round_trips_through_chunks` (:3835) reads only on a cache **hit**, so this
  is the first test in the crate that runs `read_chunked_auth_entry` on the ordinary read path.
* **(c)** `update_credentials_refreshes_the_published_value` — upstream :123-135 (the `updateTokens`
  half). Save with token `"old"`, read (warm), then
  `update_credentials("entry", credentials("new"), None)`, record `read_count`, read, and assert the
  served access token is `"new"` **with zero further backend reads**.
* **(d)** `an_invalidated_entry_serves_the_new_value_not_just_a_new_read` — upstream :229-243. Save
  `"old"` with the cache on; write `"new"` **behind the cache** by building a second `McpAuthStore`
  over the *same* `Arc<MemorySecretStore>` with a gate-off env
  (`Arc::new(|k| (k == AUTH_CACHE_DISABLED_ENV[0]).then(|| "1".to_string()))`); assert the first
  store still serves `"old"`; `invalidate_cache("rotated")`; assert it now serves `"new"`. Also cover
  the absent→appearing direction: read a missing name (cached as an explicit `None`, :4138-4140),
  write it behind the cache, assert it is still absent, invalidate, assert it appears. This is the
  case `invalidation_reloads_and_evicts_only_its_target` (:4174) cannot catch, because it counts
  reads and never checks a value.
* **(e)** The gate-off pair. Fix **F4**: rename `remove_evicts_even_when_the_cache_is_disabled`
  (:4250) to `remove_evicts_on_the_gate_on_path`, keeping its body verbatim, and add
  `eviction_and_invalidation_are_honoured_with_the_gate_off`, built with

  ```rust
  store_with_env(Arc::new(|k| (k == AUTH_CACHE_DISABLED_ENV[0]).then(|| "1".to_string())))
  ```

  asserting `!store.is_cache_enabled()` first, then (i) `invalidate_cache` is harmless while disabled
  (upstream :260-266) and (ii) `remove_auth_entry` still clears the slot so the next read reaches the
  backend and returns `None` (upstream :268-279, and the `remove_auth_entry` doc at :2781-2784 states
  this explicitly).
* **(f)** Extend `a_returned_entry_is_isolated_from_the_cached_one` (:4193-4213) rather than adding a
  near-duplicate. It covers only the hit-copy direction; add the two it misses, both guaranteed
  solely by `publish_to_cache`'s re-parse: prepend a `store.reset_cache()` so the first read is a
  **miss**-populated clone, and after `save_auth_entry` returns, mutate the caller's own
  `&mut AuthEntry` (`entry.client.as_mut().unwrap().redirect_uris.as_mut().unwrap().push(..)`) and
  assert a subsequent read is unaffected.

Finally, update the `TODO(MCP-278)` / `TODO(MCP-283)` / `TODO(MCP-287)` bullets at
credentials.rs:70-79 to reflect what now exists, including the correction that the `cyrup-it` MCP
target was already declared (F1).

---

## Definition of Done

Run `cargo nextest run -p cyrup-mcp` (baseline 612 passing) and
`cargo nextest run -p cyrup-it --features it --test mcp`. Every box below is a grep or a test name.

**Harness A — `FakeEnv` (`crates/cyrup-mcp/src/proxy/testsupport.rs`)**

- [ ] `FakeEnv` carries `call_fault`, `elicitation_action`, `guard_spill`, `complete_auth_status`,
      `complete_auth_fails` and `complete_auth_inputs`, all `Default`-constructible, plus a
      `CallFault` recipe enum that mints a `ProxyCallError` per call.
- [ ] `FakeEnv::{call_tool, read_resource}` return `Err` when a fault is scripted, and
      `CallFault::ViaRecovery` marks the connection `NeedsAuth` and delegates to
      `AuthRecovery::recover`.
- [ ] `FakeEnv::{handle_url_elicitation_required, complete_auth_from_input, guard_mcp_output}` honour
      their scripted values and default to today's behaviour when unset.
- [ ] Every pre-existing test in the crate is byte-unchanged and still passes.

**MCP-165 (`crates/cyrup-mcp/src/proxy/call.rs`)**

- [ ] A test asserts `auth_required` **with `details.autoAuthAttempted` present and `false`**, for
      both a carried `auth_message` and the `default_auth_required_message` fallback.
- [ ] A test reaches `catch_arm` through `AuthRecovery::recover` and asserts
      `details.autoAuthAttempted == true` — the first test in the crate to execute call.rs:83-121.
- [ ] A test table-drives all three `UrlElicitationAction`s, asserting the exact message and
      `details.action` for each, and that no `autoAuthAttempted` key appears.
- [ ] A test asserts `call_failed` for a plain error and `aborted` for an `McpError::Aborted`.
- [ ] A test asserts a spilled guard replaces `details.message` with
      `output truncated; see outputGuard.fullOutputPath`, that `details.outputGuard` is the spilled
      object, and that `details.error` is still `call_failed`.
- [ ] A test reaches `catch_arm` through the **resource** path and asserts `details.resourceUri` with
      no `tool` key.

**MCP-168**

- [ ] A test drives `McpTool::execute` with `redirectUrl`, `code` and `input` in turn and asserts all
      three reach `complete_auth_from_input` with their own value.
- [ ] The same test asserts precedence: all three keys at once records only `redirectUrl`'s value.
- [ ] A test asserts a non-`"authenticated"` status yields `details.error == "not_authenticated"`,
      echoes the status, carries no `authenticated` key, renders the exact sentence, and leaves both
      the connection and the failure record untouched.
- [ ] A test asserts a throwing `complete_auth_from_input` yields `auth_complete_failed` with the
      message interpolated.

**MCP-151 / MCP-152 / MCP-192 — one literal each**

- [ ] `grep -rn 'xcodebuild_list_sims' crates/cyrup-mcp/src` returns exactly one hit;
      `ProxyTool::parameters` and `proxy_tool_parameters` both resolve to `proxy::mcp_tool_schema()`
      and `ProxyTool` no longer carries a `parameters` field.
- [ ] `PROXY_DESCRIPTION_HEAD` and `PROXY_DESCRIPTION_USAGE` exist once in `proxy/constants.rs` and
      are the only source of those strings in both `build_proxy_description` functions.
- [ ] `both_proxy_descriptions_are_byte_identical_for_an_empty_config` asserts whole-string equality
      **and** that the empty-config output equals `HEAD + USAGE`. No multi-server cross-builder
      equality test is added (F7).
- [ ] `proxy_description_renders_every_block_in_order` asserts `ends_with(PROXY_DESCRIPTION_USAGE)`,
      pinning all nine usage lines on the proxy side; registration's golden still spells the string
      out in full.
- [ ] `registration::PROXY_TOOL_PROMPT_GUIDELINE` is defined as `crate::proxy::MCP_TOOL_GUIDELINE`;
      the false lowercase round-trip at registration.rs:2899-2902 is gone; the stale "exactly two
      occurrences" claim at registration.rs:160-161 is corrected to the real count.
- [ ] `the_guideline_is_gated_by_the_real_sanitizer_when_mcp_is_denied` calls
      `cyrup_permission_system::sanitize::tools::sanitize_available_tools_section` and asserts the
      bullet **survives** with `mcp` allowed, is **removed** with `mcp` denied, and that an unrelated
      bullet survives both. `grep -rn 'trim_start_matches' crates/cyrup-mcp/src/proxy/constants.rs`
      returns nothing — the re-implemented normaliser is gone.

**MCP-171 (`crates/cyrup-mcp/src/config.rs`)**

- [ ] `grep -n 'Collator::new' crates/cyrup-mcp/src/config.rs` shows it only inside `new_collator`,
      and `locale_compare` builds at most one collator per thread.
- [ ] `locale_compare` contains no `panic!`, `unwrap` or `expect`, and degrades to a fresh collator
      on both TLS-unavailable and already-borrowed.
- [ ] `locale_compare_orders_case_the_way_icu_does`'s seven original assertions pass **unchanged**,
      plus the long-string/repetition block that probes scratch-buffer reuse.
- [ ] The doc no longer says the collator "memoises" or that this is "not on any hot path"; it names
      `rank_collate` and the three sort sites, and the precedents it cites
      (`model/resolver.rs:149`, `ls.rs:128`) are paths that exist.

**MCP-278 / MCP-287 (`crates/cyrup-it/tests/mcp/keyring_recovery.rs`)**

- [ ] The module is registered by one `mod` line in `tests/mcp/main.rs`; **no `[[test]]` target and
      no dependency is added** — both already exist (F1).
- [ ] A fixture `keyctl` enforces `$1 == "session"`, `$2 == "-"`, exits 64 otherwise, and does
      `shift 2; exec "$@"`.
- [ ] A revoked-keyring **write → read → remove** round trip with a 5000-character token is served
      end to end through the recovery helper; the fake keyring directory exists and ends empty.
- [ ] A generic (non-revocation) store failure produces
      `Failed to read OAuth credentials for generic from the OS secure credential store`, its
      `source()` names no recovery helper, and the fake keyring directory is never created.
- [ ] A helper whose body is `exec sleep 30` yields exactly
      `Linux keyring recovery helper could not start: <resolved keyctl path> timed out after 10000 ms`,
      and the call returns between 10 s and 20 s.
- [ ] `{"ok":false,"error":"boom"}` + `exit 1` yields
      `Linux keyring recovery helper failed with exit code 1`; the same reply at `exit 0` yields
      `boom`; an empty `error` at `exit 0` yields `Linux keyring recovery helper failed`.
- [ ] `grep -n 'std::env::set_var\|std::env::remove_var' crates/cyrup-it/tests/mcp/keyring_recovery.rs`
      returns nothing; every switch is injected through `EnvFn` and the fixture store path is baked
      into the script body.

**MCP-269 (`crates/cyrup-mcp/src/credentials.rs`)**

- [ ] A source guard walks `src/**` **recursively** (asserting it found more than 20 files, so a
      future `proxy/`-style split cannot silently shrink it) and finds no non-comment line naming
      `cyrup_config::auth` or importing `auth` from `cyrup_config`, without false-positiving on
      `McpAuthStore` / `AuthStoreError` / `AuthStorageOptions`.
- [ ] A behavioural guard runs save → `update_credentials` → `update_state` → `remove_auth_entry` and
      asserts `<agent_dir>/auth.json` never exists and no file under either `McpDirs` root contains
      the canary token.
- [ ] `TODO(MCP-269)` at credentials.rs:80-82 is replaced by a pointer to the two guards by name.

**MCP-283 (`crates/cyrup-mcp/src/credentials.rs`)**

- [ ] Cache-hit and store-reload paths produce an identical `serde_json::Value` for an entry seeded
      with an unknown key and a `redirect_uris` list, and the stored payload no longer carries the
      unknown key.
- [ ] A chunked entry is reconstructed once on a cache **miss** and then served with zero further
      backend reads.
- [ ] `update_credentials` refreshes the published value and the next read costs zero backend reads.
- [ ] An invalidated entry serves the **new value** after a write behind the cache, in both the
      rotated and the absent→appearing directions.
- [ ] `remove_evicts_even_when_the_cache_is_disabled` is renamed to match the fixture it builds, and
      a new gate-off test asserts `!store.is_cache_enabled()`, that `invalidate_cache` is harmless,
      and that `remove_auth_entry` still clears the slot.
- [ ] `a_returned_entry_is_isolated_from_the_cached_one` additionally covers the miss-populated clone
      and the caller's `&mut AuthEntry` after `save_auth_entry`.
- [ ] `TODO(MCP-278)` / `TODO(MCP-283)` / `TODO(MCP-287)` at credentials.rs:70-79 are updated,
      including the correction that the `cyrup-it` target was already declared.

**Workspace**

- [ ] `cargo nextest run --workspace` passes with **at least** 7862 tests, none removed.
- [ ] `cargo clippy --workspace --all-targets` is clean: the workspace denies `unwrap_used`,
      `expect_used`, `panic` and `indexing_slicing`, so every new test module carries the existing
      `#[allow(...)]` header pattern (`proxy/call.rs:911`, `credentials.rs:3630-3635`,
      `cyrup-it/tests/mcp/main.rs:16-21`) and no production path added here uses any of the four.
- [ ] `cargo doc -p cyrup-mcp` is clean: `rustdoc::broken_intra_doc_links` is `deny`, and the new
      constants' doc comments link `crate::registration::build_proxy_description` and
      `crate::proxy::build_proxy_description` across modules.
