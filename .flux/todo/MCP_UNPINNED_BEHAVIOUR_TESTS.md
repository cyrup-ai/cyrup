---
stage: new
status: done
updated: 2026-08-22 16:44
---

# Pin The Implemented cyrup-mcp Behaviour That Nothing Asserts

## Description

Ten units, one obligation: **behaviour that is fully implemented and has no assertion that would
catch its regression.** None of these is a port gap. Every one of them is a place where the Rust is
correct today and a one-character edit tomorrow would be silent.

They are grouped by the *harness* they need, not by the file they land in, because the harness is
the whole cost:

* **Harness A — a fallible `FakeEnv`.** `FakeEnv::call_tool` and `FakeEnv::read_resource` return
  `Ok` unconditionally ([proxy.rs:4954-4972](../../crates/cyrup-mcp/src/proxy.rs)), so
  [`catch_arm`](../../crates/cyrup-mcp/src/proxy.rs) (proxy.rs:3711) — the whole of MCP-165's error
  taxonomy — is **unreachable from any test in the crate**. `handle_url_elicitation_required`
  (proxy.rs:4973-4979), `complete_auth_from_input` (proxy.rs:5026-5032) and `guard_mcp_output`
  (proxy.rs:5053-5058) are likewise hard-wired to one answer each. MCP-165 and MCP-168 both die at
  that fence; making the fake scriptable unblocks both at once.
* **Harness B — the `cyrup-it` MCP target, plus one fixture `keyctl`/helper pair.** MCP-278, MCP-287
  and MCP-269 all need a real spawned subprocess or a real cross-crate path. They share one new
  module and one pair of shell fixtures. MCP-283's five missing cache cases are in-crate but
  exercise the same `McpAuthStore`, so they are scheduled with them.
* **Harness C — equality guards between duplicated literals.** MCP-151, MCP-152 and MCP-192 are the
  same defect three times: a string that exists in two or three places, where the copy under test is
  not the copy in production. MCP-171 is the code-health residue that belongs with them.

Sources: [13d-mcp-proxy-modes.md](../../docs/gap-analysis/13d-mcp-proxy-modes.md) (MCP-151 :779,
MCP-152 :794, MCP-165 :994, MCP-168 :1021, MCP-171 :1061, MCP-192 :1202),
[13f-mcp-credentials.md](../../docs/gap-analysis/13f-mcp-credentials.md) (MCP-269 :1046,
MCP-278 :1181, MCP-283 :1196, MCP-287 :1262), and the census rows in
[13-cyrup-mcp-STATUS.md](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) (:668, :669, :682, :684,
:687, :696, :790, :799, :803, :807).

None of these units appears in [MCP_HIGH_SEVERITY_BACKLOG.md](MCP_HIGH_SEVERITY_BACKLOG.md), in
[MCP_13I_SCOPING.md](MCP_13I_SCOPING.md), or in the already-filed MCP task set.

---

## Findings that change the plan — read before implementing

Five premises in the specs and in the census are **wrong**. Each is corrected below and the
correction is what the implementation section prescribes.

### F1 · The `cyrup-it` MCP target already exists

MCP-278's **cyrup** paragraph says *"`cyrup-it` sets `autotests = false`, so declare the `[[test]]`
target explicitly"*, and the crate header repeats it as `TODO(MCP-278)`
([credentials.rs:70-75](../../crates/cyrup-mcp/src/credentials.rs)). It is already declared:
`[[test]] name = "mcp"`, `path = "tests/mcp/main.rs"`, `required-features = ["it"]` at
[Cargo.toml:198-204](../../crates/cyrup-it/Cargo.toml), with
[tests/mcp/main.rs](../../crates/cyrup-it/tests/mcp/main.rs) already pulling in
`#[path = "../support/mod.rs"] mod support;` (:23-24) and `mod activation;` (:26). **Do not create a
target. Add modules to the existing one and one `mod` line to `main.rs`.**

### F2 · MCP-287's rung-1 message is NOT upstream's `ETIMEDOUT` text

MCP-287 describes upstream's `spawnSync` timeout as surfacing
`Linux keyring recovery helper could not start: <ETIMEDOUT message>`. cyrup does **not** produce
that string. The wait-with-timeout at
[credentials.rs:1490-1508](../../crates/cyrup-mcp/src/credentials.rs) kills the child and returns
its own sentence:

```
Linux keyring recovery helper could not start: {keyctl} timed out after {ms} ms
```

with `{keyctl}` = `RecoveryInvocation::keyctl` (credentials.rs:1367-1370, the resolved `keyctl`
program string) and `{ms}` = `KEYRING_RECOVERY_TIMEOUT_MS` = `10_000` (credentials.rs:158-159).
**Assert cyrup's own string.** A test authored against upstream's ETIMEDOUT text is authored to
fail. `AuthSecretStoreError::Recovery` renders `{0}` verbatim (credentials.rs:316-317), so
`err.to_string()` is exactly that sentence.

### F3 · MCP-269's "no `Serialize` derive" clause is unsatisfiable as written

MCP-269's **verify** asks for *"no `Serialize` derive that could route an `AuthEntry`"* to
`cyrup_config::env`'s auth path. `AuthEntry` **must** derive `Serialize` — it is the keychain
payload, written with `serde_json::to_string` on the store path
([credentials.rs:595-597](../../crates/cyrup-mcp/src/credentials.rs) and :2322). Deleting the derive
deletes the credential store.

What the clause is actually protecting is *reachability*, not the derive. Prescribed replacement,
both halves source-observable: (a) a **source guard** asserting `cyrup_config::auth` and `AuthStore`
appear nowhere in `crates/cyrup-mcp/src/**` outside comments — the crate header already asserts the
design property in prose (credentials.rs:14, :80-82) with nothing enforcing it; and (b) a
**behavioural guard** running a full save/update/remove lifecycle against a temp agent dir and
asserting `<agent_dir>/auth.json` — `cyrup_config::env::Env::auth_path`'s exact construction
([env.rs:313-315](../../crates/cyrup-config/src/env.rs)) — never comes into existence and no file
under the tree contains the token text.

### F4 · `remove_evicts_even_when_the_cache_is_disabled` does not disable the cache

[credentials.rs:4250-4263](../../crates/cyrup-mcp/src/credentials.rs) builds its store with
`store_with_env(Arc::new(|_| None))`. `is_cache_enabled` is
`!env_is_one(&env, &AUTH_CACHE_DISABLED_ENV)` (credentials.rs:2155-2157) and `env_is_one` is
`== Some("1")` (:237-239), so an env that answers `None` leaves the cache **enabled**. The test
therefore asserts eviction on the gate-**on** path while its name claims the gate-off path, and
MCP-283's *"`removeAuthEntry` evicting **even with the gate off**"* case is unported. Fix the fixture
and the name; do not delete the gate-on assertion — keep both.

### F5 · The strong schema test guards the copy that is not registered

`mcp_tool_schema()` ([proxy.rs:4080-4106](../../crates/cyrup-mcp/src/proxy.rs)) is guarded by a
twelve-name / exact-`action`-description / alphabetical-order assertion (proxy.rs:5537-5567). The
tool actually registered by `register_surface` is `registration::ProxyTool`
([registration.rs:2174-2186](../../crates/cyrup-mcp/src/registration.rs)), whose `parameters` come
from the **second literal**, `proxy_tool_parameters()` (registration.rs:1675-1737), guarded only by
a five-name spot check (registration.rs:2875-2887). A drift in the registered schema passes today.

The same shape holds for the description (two `build_proxy_description`, only the head line
compared) and the guideline (`MCP_TOOL_GUIDELINE` at proxy.rs:181-182 vs
`PROXY_TOOL_PROMPT_GUIDELINE` at registration.rs:162, only the *unregistered* one normalisation-
tested at proxy.rs:5652-5667). Note also that registration.rs:160-161 claims *"A tree-wide grep
finds exactly two occurrences: the matcher, and this line"* — a grep for
`mcp discovery first` returns **five** hits across three files today. The doc claim is stale and
must be corrected with the fix.

---

## Per-unit breakdown

### MCP-165 · `executeCall`'s error taxonomy — medium, `hand-written`, implemented

**Obligation unmet.** The three arms of
[`catch_arm`](../../crates/cyrup-mcp/src/proxy.rs) (proxy.rs:3711-3771) are exercised by nothing:
`SessionRecoveryAuthRequired` (:3721), `UrlElicitationRequired` (:3730) and `Other` (:3743). Both
call sites (proxy.rs:3638 and :3645) sit behind `ProxyEnv::call_tool` / `read_resource`, and the
only implementor in the test tree returns `Ok` unconditionally (proxy.rs:4954-4972). MCP-165's
verify names three unit assertions — *each arm's exact text and code*, and *a guard-spilled path
substitutes the truncation message* — and none is possible until the fake can fail.

### MCP-168 · `executeAuthComplete` — medium, `hand-written`, implemented

**Obligation unmet.** MCP-168's verify names two unit tests. Neither exists. The one test present
([proxy.rs:6840-6857](../../crates/cyrup-mcp/src/proxy.rs)) covers only the success arm.

1. *all three input keys accepted* — the `redirectUrl ?? code ?? input` selection lives in the tool
   dispatcher (proxy.rs:4474-4483), not in `execute_auth_complete`, so this must be driven through
   `McpTool::execute`. `FakeEnv` currently cannot record which input arrived
   (`complete_auth_from_input` at proxy.rs:5026-5032 ignores its argument).
2. *a non-`"authenticated"` status yields `not_authenticated` with the status echoed* — the arm at
   proxy.rs:2669-2674 is unreachable while the fake always answers `"authenticated"`.

### MCP-192 · The permission system's contracts on the `mcp` tool — medium, `host-verb`, implemented

**Obligation unmet.** MCP-192's verify has two halves; only the weaker one exists.
`guideline_normalises_to_the_sanitizer_key` (proxy.rs:5652-5667) **re-implements** the sanitizer's
normalisation inline and compares against a copy of the key literal — so a drift in
[sanitize/tools.rs:47](../../crates/cyrup-permission-system/src/sanitize/tools.rs) is invisible to
it, and it tests `MCP_TOOL_GUIDELINE` (proxy.rs:181-182), which is not the constant the registered
tool carries (F5).

The failure mode is inverted from the obvious guess and that is why the weak test is not enough:
`should_keep_guideline` is `guideline_keep_rule(..).unwrap_or(true)`
(sanitize/tools.rs:133-135), so a bullet matching no rule is **always kept**. A one-character drift
does not delete guidance — it silently disables the gate, leaving `use mcp …` in the system prompt
after the `mcp` tool has been taken away.

### MCP-151 · The `mcp` tool's JSON Schema — high, `host-verb`, implemented

**Obligation unmet (code-health).** Two byte-equivalent schema literals: `mcp_tool_schema()`
(proxy.rs:4080-4106, `OnceLock`) and `proxy_tool_parameters()` (registration.rs:1675-1737, rebuilt
per call). The registered tool takes the second (registration.rs:1658, :2179). Per F5 the strong
test guards the first. Five property names — `tool`, `server`, `connect`, `describe`, `search` — are
a cross-crate permission contract (registration.rs:1664-1668).

### MCP-152 · `buildProxyDescription` — high, `hand-written`, implemented

**Obligation unmet (code-health).** Two builders: `proxy::build_proxy_description`
(proxy.rs:3904-4051, live metadata, `&IndexMap<String, CachedServerEntry>`) and
`registration::build_proxy_description` (registration.rs:1271-1425, cold cache,
`Option<&MetadataCache>`). Production calls **only** the second (registration.rs:2175). The head
literal is duplicated verbatim at proxy.rs:3910-3912 and registration.rs:1284-1287; the nine-line
usage block at proxy.rs:4038-4050 and registration.rs:1410-1422.

The existing guard `both_proxy_descriptions_share_one_head_line` (registration.rs:3069-3085) compares
`text.lines().next()` and nothing else — the usage block, the `Disabled servers` sentence and the
`Server instructions` sentence are unguarded across the copies. registration.rs:1276-1284 records
that these two heads **have already diverged once**, and that the cost was a permanently-firing
re-registration invalidating the provider's prompt-cache prefix.

### MCP-171 · The `localeCompare` tie-break — low, `open-decision`, implemented

**Obligation unmet (performance residue).** `locale_compare`
([config.rs:4048-4051](../../crates/cyrup-mcp/src/config.rs)) constructs a fresh
`feruca::Collator` on **every comparison**. Its own doc (config.rs:4044-4046) justifies this with
*"conflict lists are a handful of server names read once per discovery-summary poll, so the
allocation is not on any hot path"* — which was true when it was written and is false now:
`rank_collate` (proxy.rs:1074-1082) delegates straight to it and is the comparator of three sorts —
`rank_tool_matches` (proxy.rs:1134), the empty-query sort (proxy.rs:2384) and the connecting-server
hint list (proxy.rs:2413) — so an O(n log n) sort builds O(n log n) collators. The workspace already
hoists this exact collator outside a `sort_by` at
[cyrup-config/src/model.rs:151-157](../../crates/cyrup-config/src/model.rs).

### MCP-269 · MCP credentials never reach `auth.json` — medium, `hand-written`, partial

**Obligation unmet.** No guard exists; the crate flags it itself as `TODO(MCP-269)` at
credentials.rs:80-82. See **F3** for the corrected form of the two verify clauses.

### MCP-278 · The storage acceptance suite — medium, `hand-written`, partial

**Obligation unmet.** The 15 in-process cases are ported. The **two subprocess cases** are not:

1. *routes revoked Linux keyring operations through the recovery helper* — the positive path through
   `should_attempt_recovery` (credentials.rs:1303-1305) into `LinuxKeyringRecoveryStore`
   (credentials.rs:2642, :2665, :2793).
2. *does not use the recovery helper for generic secure-store failures* — the negative twin, with a
   fake `keyctl` exiting 99 and an assertion that the helper's marker file was never created.

The fixture `keyctl` argv contract (`$1 == "session"`, `$2 == "-"`, exit 64 otherwise, `shift 2`,
`exec "$@"`) is documented at credentials.rs:1408-1413 and is *the only thing that pins MCP-260's
argv shape*. Nothing in the tree spawns it today.

### MCP-283 · The cache acceptance suite — medium, `hand-written`, partial

**Obligation unmet.** Eight of the thirteen upstream cases are ported (credentials.rs:4136-4290).
Missing, each a distinct upstream case:

| # | upstream case | why the existing tests miss it |
|---|---|---|
| a | publication normalizes exactly as a later store reload does | only the generic `unknown_keys_are_dropped_not_rejected` (:3720) exists, and it never goes through `publish_to_cache` (:2379-2392) |
| b | a chunked entry is reconstructed once, then served with zero further backend reads | `a_large_credential_round_trips_through_chunks` (:3836) never re-reads |
| c | clone isolation in the **write** direction | `a_returned_entry_is_isolated_from_the_cached_one` (:4194) mutates only the returned entry |
| d | `updateTokens` refreshes the published value | nothing calls `update_credentials` (:2890) and re-reads at zero cost |
| e | `removeAuthEntry` evicts **with the gate off**, and invalidation is harmless while disabled | see **F4** — the test named for this builds a cache-**enabled** fixture |

### MCP-287 · The subprocess timeout path and the unreachable ladder rung — medium, `hand-written`, partial

**Obligation unmet.** The six rungs are implemented and correctly ordered (rung 1 at
credentials.rs:1447-1451/:1505-1508, rung 2 at :1532-1543, rung 3 at :1544-1550, rung 4 at
:1552-1561, rung 5 at :1568-1577, rung 6 at :1579-1587). None of the three fixtures MCP-287 names
exists. See **F2** for the string to assert.

**Additional hazard worth pinning, found while reading:** the timeout branch kills the direct child
and then **joins the stdout reader thread** (credentials.rs:1496-1503). The read end only sees EOF
when every holder of the write end is gone; `child.kill()` kills only the direct child. A fixture
`keyctl` that runs `"$@"` without `exec` would leave the sleeper holding the pipe and the 10 s
timeout would silently become 30 s. The prescribed fixture uses `exec "$@"` (which is also the argv
contract), and the test asserts an elapsed-time upper bound so a regression here fails loudly rather
than hanging.

---

## Implementation

### 1 · Harness A — make `FakeEnv` fallible (`crates/cyrup-mcp/src/proxy.rs`)

Extend the fixture at proxy.rs:4882-4897 with five scripted slots. `ProxyCallError` is not `Clone`,
so script a small clonable fault and build the error at the seam.

```rust
/// A scripted `ProxyCallError`. `ProxyCallError` is not `Clone`, so the fake holds a recipe and
/// mints one error per call — which is also what lets `read_resource` and `call_tool` share it.
#[derive(Debug, Clone)]
enum CallFault {
    /// `SessionRecoveryAuthRequiredError`, with `error.authMessage` present or absent.
    AuthRequired { auth_message: Option<String> },
    /// rmcp's `UrlElicitationRequiredError`, carrying the opaque detail.
    UrlElicitation { detail: String },
    /// Anything else — `McpError::Other`, so `is_abort_error` says false ⇒ `call_failed`.
    Other(String),
    /// `McpError::Aborted`, the arm that turns into `aborted` rather than `call_failed`.
    Aborted(String),
}

impl CallFault {
    fn into_error(self, server: &str) -> ProxyCallError {
        match self {
            CallFault::AuthRequired { auth_message } => ProxyCallError::SessionRecoveryAuthRequired {
                server: server.to_string(),
                auth_message,
            },
            CallFault::UrlElicitation { detail } => ProxyCallError::UrlElicitationRequired { detail },
            CallFault::Other(message) => ProxyCallError::Other(McpError::other(message)),
            CallFault::Aborted(reason) => ProxyCallError::Other(McpError::Aborted(reason)),
        }
    }
}
```

New `FakeEnv` fields (all `Default`-constructible, so the existing `#[derive(Default)]` at
proxy.rs:4881 keeps working and no existing test changes):

```rust
    /// When set, `call_tool` and `read_resource` fail with this instead of succeeding.
    call_fault: Mutex<Option<CallFault>>,
    /// `manager.handleUrlElicitationRequired`'s verdict; `None` keeps today's `Accept`.
    elicitation_action: Mutex<Option<UrlElicitationAction>>,
    /// When set, `guard_mcp_output` returns it as `GuardedOutput::output_guard` — the spill that
    /// makes `catch_arm` substitute the truncation message.
    guard_spill: Mutex<Option<Value>>,
    /// `completeAuthFromInput`'s answer. `None` keeps today's `"authenticated"`.
    complete_auth_status: Mutex<Option<String>>,
    /// Every `input` `complete_auth_from_input` was handed, in order — MCP-168's three-key proof.
    complete_auth_inputs: Mutex<Vec<String>>,
```

Builders beside the existing ones (proxy.rs:4899-4928):

```rust
        fn with_call_fault(self, fault: CallFault) -> Self {
            *self.call_fault.lock().unwrap() = Some(fault);
            self
        }
        fn with_elicitation_action(self, action: UrlElicitationAction) -> Self {
            *self.elicitation_action.lock().unwrap() = Some(action);
            self
        }
        fn with_guard_spill(self, guard: Value) -> Self {
            *self.guard_spill.lock().unwrap() = Some(guard);
            self
        }
        fn with_auth_complete_status(self, status: &str) -> Self {
            *self.complete_auth_status.lock().unwrap() = Some(status.to_string());
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
            _recovery: &AuthRecovery<'_>,
            _cancel: &CancelToken,
        ) -> Result<CallToolOutcome, ProxyCallError> {
            match self.call_fault.lock().unwrap().clone() {
                Some(fault) => Err(fault.into_error(server)),
                None => Ok(CallToolOutcome::default()),
            }
        }
        async fn read_resource(
            &self,
            server: &str,
            _uri: &str,
            _recovery: &AuthRecovery<'_>,
            _cancel: &CancelToken,
        ) -> Result<Vec<Content>, ProxyCallError> {
            match self.call_fault.lock().unwrap().clone() {
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

### 2 · MCP-165 — the three arms, beside the existing `execute_call` tests (proxy.rs, after :6808)

Four `#[tokio::test]`s. Every fixture is the shape already used at proxy.rs:6786-6792.

* `auth_required_arm_carries_the_message_and_the_latch_flag` — `CallFault::AuthRequired
  { auth_message: Some("token expired".into()) }`; assert `details["error"] == "auth_required"`,
  `text_of(&result) == "token expired"`, and `details["autoAuthAttempted"] == false`. Repeat with
  `auth_message: None` and assert the text is `default_auth_required_message("srv")` — the
  `unwrap_or_else` at proxy.rs:3722-3723.
* `url_elicitation_arm_renders_one_message_per_action` — table-drive
  `(UrlElicitationAction::Accept, "The original MCP tool did not run. Complete the opened browser
  interaction, then retry the tool.")`, `(Decline, "The URL interaction was declined.")`,
  `(Cancel, "The URL interaction was cancelled.")`; assert `details["error"] ==
  "url_elicitation_required"` and `details["action"] == action.as_str()` in all three.
* `a_plain_failure_is_call_failed_and_an_abort_is_aborted` — `CallFault::Other("boom")` ⇒
  `details["error"] == "call_failed"` and `details["message"] == "boom"`; `CallFault::Aborted("stop")`
  ⇒ `details["error"] == "aborted"`.
* `a_guard_spill_replaces_the_message_with_the_truncation_sentence` — `CallFault::Other("boom")`
  **plus** `.with_guard_spill(json!({"fullOutputPath": "/tmp/x"}))`; assert `details["message"] ==
  "output truncated; see outputGuard.fullOutputPath"` and that `details["outputGuard"]` is the
  spilled object (`GuardedOutput::write_details`, proxy.rs:1421-1428). This is the arm at
  proxy.rs:3762-3768.

Add a fifth driving the **resource** path (`tool_meta.resource_uri` set, proxy.rs:3618-3641) with
`CallFault::AuthRequired`, so the second `catch_arm` call site (proxy.rs:3638) is covered too.

### 3 · MCP-168 — the two named tests (proxy.rs, beside :6839)

```rust
    #[tokio::test]
    async fn all_three_input_keys_reach_complete_auth_from_input() {
        let config = config_with(&[("linear", http("https://linear.example/mcp"))]);
        let (ctx, env) = ctx_with(config, &[], &[], FakeEnv::default());
        let (_keep, rx) = tokio::sync::watch::channel(InitPhase::Ready(ctx));
        let gate = Arc::new(ProxyInitGate::new(rx));
        let tool = McpTool::new(String::new(), &McpSettings::default(), gate);

        for (key, value) in
            [("redirectUrl", "http://cb?code=a"), ("code", "b"), ("input", "c")]
        {
            let result = tool
                .execute(
                    ToolCallId::from("call-1"),
                    json!({"action": "auth-complete", "server": "linear", "args": {key: value}}),
                    CancelToken::new(),
                    Box::new(|_| {}),
                )
                .await
                .expect("auth-complete returns an envelope, never an Err");
            assert_eq!(result.details.clone().unwrap()["authenticated"], json!(true), "key = {key}");
        }
        // `redirectUrl ?? code ?? input` — each key reached the seam with ITS value (proxy.rs:4474).
        assert_eq!(
            *env.complete_auth_inputs.lock().unwrap(),
            vec!["http://cb?code=a".to_string(), "b".to_string(), "c".to_string()]
        );
    }
```

Add the precedence assertion in the same test: one call passing all three keys at once must record
only `redirectUrl`'s value — that is what `or_else` chaining at proxy.rs:4477-4479 buys.

```rust
    #[tokio::test]
    async fn a_non_authenticated_status_is_echoed_as_not_authenticated() {
        let config = config_with(&[("linear", http("https://linear.example/mcp"))]);
        let env = FakeEnv::default().with_auth_complete_status("pending");
        let (ctx, env) = ctx_with(config, &[], &[], env);
        let result =
            execute_auth_complete(&ctx, "linear", "http://cb?code=x", &CancelToken::new())
                .await
                .unwrap();
        let details = result.details.clone().unwrap();
        assert_eq!(details["error"], json!("not_authenticated"));
        assert_eq!(details["status"], json!("pending"), "the status is echoed, not swallowed");
        assert_eq!(
            text_of(&result),
            "OAuth authentication did not complete for \"linear\"."
        );
        // The success side-effects must NOT have run (proxy.rs:2675-2686).
        assert!(env.get_connection("linear").is_none() || true);
    }
```

Drop the last line and instead seed `.with_connection("linear", ConnectionStatus::Connected)` and
assert the connection is **still** `Connected` — proving `close` was not called on the failure arm.

### 4 · Harness C1 — MCP-151, one schema literal

Delete the second literal. In [registration.rs](../../crates/cyrup-mcp/src/registration.rs):

* Drop the `parameters: Value` field from `ProxyTool` (:1644-1649) and the initialiser at :1658.
* `fn parameters(&self) -> &Value` (:1745-1747) becomes `crate::proxy::mcp_tool_schema()` — a
  `&'static Value` coerces to the `&Value` the trait wants.
* Replace the body of `proxy_tool_parameters()` (:1675-1737) with
  `crate::proxy::mcp_tool_schema().clone()`, keeping the doc comment (the five permission-relevant
  names are documented there and nowhere else) and adding a line saying the literal now lives in
  `proxy::mcp_tool_schema` so the twelve-property assertion at proxy.rs:5537 guards the **registered**
  schema.
* Keep `the_proxy_schema_keeps_the_five_permission_relevant_names` (:2875-2887) as-is: it is now a
  cross-module reachability assertion rather than a second, weaker snapshot.

### 5 · Harness C2 — MCP-152, the two shared literals plus a full-string guard

Both description builders keep their own shape (their cache and spec types genuinely differ); only
the two **fixed** literals are shared. Add to [proxy.rs](../../crates/cyrup-mcp/src/proxy.rs), beside
`INSTRUCTIONS_SNIPPET_LENGTH` (:108):

```rust
/// `direct-tools.ts:240`'s header, with the single `Pi` → `cyrup` rebrand and the `mcpScript`
/// sentence cut (Cut 4). ONE literal, because the description is built twice — from the cold cache
/// (`registration::build_proxy_description`) and from live metadata
/// ([`build_proxy_description`]) — and `McpExtension::proxy_tool_description` re-registers only
/// when the text CHANGED, so a one-word difference between the copies makes the guard misfire on
/// every reconnect and invalidates the provider's prompt-cache prefix. That has happened once
/// already (registration.rs:1276-1284).
pub(crate) const PROXY_DESCRIPTION_HEAD: &str =
    "MCP gateway — server status, tool search/describe, auth, and single MCP tool calls. Non-MCP cyrup tools should be called directly, not through mcp.\n";

/// `direct-tools.ts`'s fixed usage block, minus the `ui-messages` line (Cut 2). Byte-exact,
/// including the two-space indent, the `→` glyph and the ABSENCE of a trailing newline on the
/// final `Mode:` line. Shared for the same reason as [`PROXY_DESCRIPTION_HEAD`].
pub(crate) const PROXY_DESCRIPTION_USAGE: &str = concat!(
    "\nUsage:\n",
    "  mcp({ })                              → Show server status\n",
    // …the remaining eight lines verbatim from proxy.rs:4040-4049…
    "\nMode: action > tool (call) > connect > describe > instructions > search > server (list) > nothing (status)",
);
```

Then `desc.push_str(PROXY_DESCRIPTION_HEAD)` / `desc.push_str(PROXY_DESCRIPTION_USAGE)` replaces
proxy.rs:3909-3912 + :4038-4050 and registration.rs:1284-1287 + :1410-1422.

Upgrade the guard at registration.rs:3069-3085 from a head-line comparison to a **whole-string**
one, keeping the head assertion as a second statement so the literal itself stays pinned:

```rust
    #[test]
    fn both_proxy_descriptions_are_byte_identical_for_the_same_inputs() {
        let from_cache = build_proxy_description(&McpConfig::default(), None, &[]);
        let from_live = crate::proxy::build_proxy_description(
            &McpConfig::default(),
            &indexmap::IndexMap::new(),
            &[],
        );
        // Blocks 2-5 are all empty for an empty config, so this compares the head AND the nine-line
        // usage block in full — the two literals that used to be duplicated.
        assert_eq!(from_cache, from_live);
        assert!(from_cache.starts_with(crate::proxy::PROXY_DESCRIPTION_HEAD));
        assert!(from_cache.ends_with(
            "\nMode: action > tool (call) > connect > describe > instructions > search > server (list) > nothing (status)"
        ));
    }
```

Then add a second case with a **three-server** config (one disabled, one with a cached instructions
string, one with two cached tools), building the `MetadataCache` and the `IndexMap<String,
CachedServerEntry>` from the same source data, and assert whole-string equality again — that is what
covers blocks 3, 4 and 5.

### 6 · Harness C3 — MCP-192, one guideline and a real sanitizer round trip

In [registration.rs](../../crates/cyrup-mcp/src/registration.rs), correct the stale doc claim at
:156-161 (it says two occurrences; there are five) and re-point `PROXY_TOOL_PROMPT_GUIDELINE` at the
single source of truth:

```rust
pub const PROXY_TOOL_PROMPT_GUIDELINE: &str = crate::proxy::MCP_TOOL_GUIDELINE;
```

This forces the two tools to advertise one string. `MCP_TOOL_GUIDELINE` is mixed-case
(`"Use mcp for MCP discovery first: …"`, proxy.rs:181-182) and the sanitizer lowercases before
matching, so the change is behaviour-preserving — but delete the now-false lowercase round-trip
assertion at registration.rs:2899-2902 and replace both weak tests with the real thing.

Replace `guideline_normalises_to_the_sanitizer_key` (proxy.rs:5652-5667) with a test that runs the
**actual** sanitizer. `cyrup-mcp` already depends on `cyrup-permission-system`
([Cargo.toml:31](../../crates/cyrup-mcp/Cargo.toml)) and
`sanitize_available_tools_section` is `pub`
([sanitize/tools.rs:187](../../crates/cyrup-permission-system/src/sanitize/tools.rs)):

```rust
    #[test]
    fn the_guideline_is_gated_by_the_real_sanitizer_when_mcp_is_denied() {
        use cyrup_permission_system::sanitize::tools::sanitize_available_tools_section;

        let prompt = format!(
            "Intro.\n\nGuidelines:\n- {MCP_TOOL_GUIDELINE}\n- use write only for new files or complete rewrites\n\nEnd:\nfin"
        );

        // `mcp` exposed ⇒ the bullet survives. `guideline_keep_rule` returning `Some(true)` and
        // returning `None` are INDISTINGUISHABLE here, which is why the denied case below is the
        // assertion that actually catches a drift (13d MCP-192).
        let kept = sanitize_available_tools_section(&prompt, &["mcp".to_string(), "write".to_string()]);
        assert!(kept.prompt.contains(MCP_TOOL_GUIDELINE));

        // `mcp` denied ⇒ the bullet is GONE. If the literal ever drifts from sanitize/tools.rs:47,
        // `unwrap_or(true)` keeps it and this fails.
        let denied = sanitize_available_tools_section(&prompt, &["write".to_string()]);
        assert!(denied.removed);
        assert!(
            !denied.prompt.contains("mcp discovery first"),
            "a denied `mcp` must take its guideline with it; got:\n{}",
            denied.prompt
        );
        // The unrelated bullet is untouched — proves the section was filtered, not deleted.
        assert!(denied.prompt.contains("use write only for new files or complete rewrites"));
        assert_eq!(MCP_TOOL_NAME, "mcp");
    }
```

Add the same two assertions against `registration::PROXY_TOOL_PROMPT_GUIDELINE` in
registration.rs's test module, so the constant the **registered** tool carries is the one under test.

### 7 · MCP-171 — hoist the collator (`crates/cyrup-mcp/src/config.rs`)

Replace config.rs:4048-4051 and correct the doc paragraph at :4044-4046, which is now false:

```rust
thread_local! {
    /// One collator per thread. `feruca::Collator::collate` takes `&mut self` because it memoises,
    /// so this is a `RefCell` rather than a shared value — and thread-local rather than a global
    /// lock, because the callers are sort comparators and a mutex would serialise them.
    static COLLATOR: std::cell::RefCell<feruca::Collator> = std::cell::RefCell::new(new_collator());
}

/// The one collator configuration this workspace uses: CLDR-root tailoring, non-ignorable variable
/// weighting, byte-value tie-break — proven against Node in `cyrup-tools/src/tools/ls.rs` and
/// `cyrup-config/src/model.rs`.
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

Replace the "not on any hot path" sentence with the truth: `rank_collate` (proxy.rs:1074) is this
function, and it is the comparator of the sorts at proxy.rs:1134, :2384 and :2413. The existing
assertions at config.rs:5098-5116 (`apple < banana`, case-blind primary, lowercase-first tertiary,
`é` between `e` and `z`, digits before letters) must still pass unchanged — that is the proof the
hoist is behaviour-preserving. Add one assertion that a repeated comparison is stable across calls
(`locale_compare("a", "A")` twice) so a memoisation bug in the shared collator would surface.

### 8 · Harness B — the `cyrup-it` MCP modules and the two fixture scripts

Add three `mod` lines to [tests/mcp/main.rs](../../crates/cyrup-it/tests/mcp/main.rs) after :26:

```rust
mod credential_isolation;
mod keyring_recovery;
```

**Fixture helper, shared by both new modules.** Put it in `tests/mcp/keyring_recovery.rs` and let
`credential_isolation.rs` `use super::keyring_recovery::…` if it needs it. Use
[`support::scratch::Scratch::write`](../../crates/cyrup-it/tests/support/scratch.rs) (:68-77) and set
the executable bit; gate the whole module on `#[cfg(unix)]`.

```rust
/// The fake `keyctl`. Upstream's own fixture, verbatim in shape: it PROVES the argv
/// (`crates/cyrup-mcp/src/credentials.rs:1408-1413`) and is the only thing in the tree that does.
/// `exec` matters twice: it is what upstream's fixture does, and it makes the helper the DIRECT
/// child, so the parent's `kill()` on timeout actually closes the stdout pipe.
const KEYCTL: &str = r#"#!/bin/sh
[ "$1" = "session" ] || exit 64
[ "$2" = "-" ] || exit 64
shift 2
exec "$@"
"#;

fn script(scratch: &Scratch, name: &str, body: &str) -> PathBuf {
    let path = scratch.write(format!("fixtures/{name}"), body);
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// An `EnvFn` answering only the recovery switches — nothing touches `std::env`
/// (docs/TEST-ARCHITECTURE.md §4 R2; `EnvFn` exists for exactly this, credentials.rs:217-220).
fn recovery_env(keyctl: &Path, helper: &Path) -> EnvFn {
    let (keyctl, helper) = (keyctl.to_owned(), helper.to_owned());
    Arc::new(move |key: &str| match key {
        k if k == KEYRING_RECOVERY_KEYCTL_ENV[0] => Some(keyctl.to_string_lossy().into_owned()),
        k if k == KEYRING_RECOVERY_HELPER_ENV[0] => Some(helper.to_string_lossy().into_owned()),
        k if k == TEST_LINUX_KEYRING_RECOVERY_ENV[0] => Some("1".to_string()),
        _ => None,
    })
}
```

`KEYRING_RECOVERY_KEYCTL_ENV`, `KEYRING_RECOVERY_HELPER_ENV` and
`TEST_LINUX_KEYRING_RECOVERY_ENV` are `pub` (credentials.rs:190-210) and `credentials` is a
`pub mod` ([lib.rs:135](../../crates/cyrup-mcp/src/lib.rs)), so every type below is reachable.
`…_KEYRING_RECOVERY_HELPER` names a **program, not a script token** (credentials.rs:1376-1382), so a
shell script with a shebang is a legal helper — which is what makes these tests possible while
MCP-260's host half (`crates/cyrup/src/mcp_keyring_helper_cmd.rs`) is still unbuilt.

#### 8a · MCP-278's two subprocess cases (`tests/mcp/keyring_recovery.rs`)

```rust
/// A helper that answers every request from a JSON file, and TOUCHES a marker so the negative case
/// can assert it never ran.
const HELPER_OK: &str = r#"#!/bin/sh
touch "$MCP_IT_HELPER_MARKER"
read -r _line
printf '{"ok":true,"found":true,"value":%s}\n' "$MCP_IT_HELPER_VALUE"
"#;
```

* `revoked_keyring_reads_are_routed_through_the_recovery_helper` — build an `McpAuthStore` with
  `McpAuthStore::with_backends(Arc::new(MemorySecretStore::with_fault(SimulatedFault::KeyRevoked)),
  Arc::new(MemorySecretStore::new()), McpDirs::new(agent, cwd), AuthStorageOptions::default(),
  recovery_env(&keyctl, &helper))` (credentials.rs:2088-2107, :1070). Seed the helper's reply with a
  serialised `AuthEntry`. Call `store.auth_entry("srv")` and assert it returns the helper's entry —
  the recovery dispatch at credentials.rs:2620-2624/:2642-2647 fired. Assert the marker file exists,
  and assert the process did **not** exit 64, i.e. no `Recovery("… failed with exit code 64")` — that
  is the argv assertion.
* `a_generic_store_failure_never_spawns_the_recovery_helper` — same fixture but
  `SimulatedFault::Unavailable` (not revocation) and a `keyctl` whose body is `exit 99`. Assert
  `store.auth_entry("srv")` is `Err`, that the rendered error is the store's own
  `Unavailable { operation: Read, .. }` text (`"Failed to read OAuth credentials for srv from the OS
  secure credential store"`, credentials.rs:3804-3822) and **not** any `Linux keyring recovery
  helper …` sentence, and that the marker file does **not** exist. That is
  `should_attempt_recovery`'s AND (credentials.rs:1303-1305) proven behaviourally rather than by the
  in-crate predicate test at credentials.rs:4324.

#### 8b · MCP-287's three ladder fixtures (same module)

Drive `LinuxKeyringRecoveryStore::new(AUTH_SECRET_SERVICE, recovery_env(..))` (credentials.rs:1596-
1610) directly through `AuthSecretStore::read`, so the assertion is on the rung message with no
store logic in the way.

* `a_hung_helper_produces_the_rung_one_timeout_message_and_does_not_hang` — helper body
  `#!/bin/sh\nsleep 30\n`. Record `Instant::now()`, call `read`, assert:
  ```rust
  assert_eq!(
      error.to_string(),
      format!("Linux keyring recovery helper could not start: {} timed out after 10000 ms",
              keyctl.display())
  );
  assert!(elapsed < Duration::from_secs(20), "the stdout-reader join must not outlive the kill");
  ```
  The `{keyctl}` interpolation is `RecoveryInvocation::keyctl` — the resolved program **path string**
  from the env override, not the literal `"keyctl"` (credentials.rs:1367-1370, :1505-1508). The
  elapsed bound is the no-zombie/no-hang assertion (see the MCP-287 hazard note above).
* `an_error_reply_that_exits_one_reports_the_exit_code_not_the_helper_text` — helper prints
  `{"ok":false,"error":"boom"}` and `exit 1`. Assert
  `"Linux keyring recovery helper failed with exit code 1"` — rung 2 winning over rung 5
  (credentials.rs:1532-1543), which is the whole point of the unit.
* `the_same_reply_at_exit_zero_reaches_rung_five` — identical body, `exit 0`. Assert the message is
  exactly `"boom"` (credentials.rs:1568-1577). Add the empty-`error` variant
  (`{"ok":false,"error":""}`, exit 0) and assert the fallback
  `"Linux keyring recovery helper failed"`, which the `.filter(|m| !m.is_empty())` at :1573 produces.

#### 8c · MCP-269's two guards (`tests/mcp/credential_isolation.rs`)

```rust
/// (a) The SOURCE guard. `crates/cyrup-mcp` must never name `cyrup_config`'s credential store.
/// Precedent: `crates/cyrup-tui/src/tests/transcript_expand_wiring.rs:128-160`.
#[test]
fn no_cyrup_mcp_source_file_reaches_cyrup_configs_auth_store() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../cyrup-mcp/src");
    let mut offenders = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&src).unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    files.sort();
    for file in &files {
        for (n, line) in std::fs::read_to_string(file).unwrap().lines().enumerate() {
            let code = line.trim();
            if code.starts_with("//") || code.starts_with("///") || code.starts_with("//!") {
                continue; // the crate header DISCUSSES the decision (credentials.rs:14, :80-82)
            }
            if code.contains("cyrup_config::auth") || code.contains("AuthStore::") {
                offenders.push(format!("{}:{}: {code}", file.display(), n + 1));
            }
        }
    }
    assert!(offenders.is_empty(), "MCP credentials must not reach `auth.json`: {offenders:#?}");
}
```

Note the second predicate must not fire on `McpAuthStore::`, `AuthStoreError::` or
`AuthStorageOptions` — filter those first, or match on `cyrup_config::auth` alone plus a check that
`cyrup_config` is never imported with `auth` in the path.

```rust
/// (b) The BEHAVIOURAL guard. A full credential lifecycle leaves `<agent_dir>/auth.json`
/// non-existent and no token text anywhere on disk. `auth.json` is exactly what
/// `cyrup_config::env::Env::auth_path` constructs (crates/cyrup-config/src/env.rs:313-315).
#[test]
fn a_full_mcp_credential_lifecycle_never_creates_auth_json() { /* … */ }
```

Body: build the store with a `MemorySecretStore` backend rooted at `scratch.agent_dir()`, run
`save_auth_entry` → `update_credentials` → `update_state` → `remove_auth_entry` with a distinctive
token literal, then assert `agent_dir.join("auth.json")` does not exist and that a recursive walk of
`agent_dir` finds no file whose bytes contain that literal. Assert the same for the `cwd` half of
`McpDirs`.

### 9 · MCP-283's five cache cases (`crates/cyrup-mcp/src/credentials.rs`, beside :4136-4290)

All five use the existing `test_store` (:3640-3653) and `credentials` (:3656-3665) helpers.
`AuthEntry` has no `PartialEq` and a hand-written `Debug`, so compare with
`serde_json::to_value(&entry).unwrap()`.

* **(a)** `publication_normalizes_exactly_as_a_store_reload_does` — `backend.seed(&auth_entry_account("srv"),
  r#"{"serverUrl":"https://x.example/mcp","futureKey":9}"#)` (:1102, :763). Call
  `store.update_credentials("srv", credentials("t"), Some("https://x.example/mcp"))` — that goes
  through `mutate` → `save_auth_entry` → `publish_to_cache` (:2379-2392), which **re-parses the
  payload it just wrote**. Read once (cache hit), `store.reset_cache()`, read again (store reload),
  and assert the two `to_value`s are equal and that the stored payload no longer contains
  `futureKey`. *Upstream's framing does not port literally* — `AuthEntry` has no unknown-key channel
  on the write side — so the porting-correct assertion is hit-path ≡ miss-path for the same entry.
  Add `redirect_uris` (the one field that degrades silently, :3752-3765) to the entry so the
  comparison has something lossy to be right about.
* **(b)** `a_chunked_entry_is_reconstructed_once_then_served_from_the_cache` — build the
  `SimulatedFault::SizeLimited` store (:3836's fixture), save a 5000-char credential, `reset_cache()`,
  read once, record `backend.read_count()` (:1080), read twice more and assert the count is
  unchanged.
* **(c)** `the_cached_entry_is_isolated_from_the_caller_s_object` — the direction :4194 does not
  cover: keep the `&mut AuthEntry` handed to `save_auth_entry`, mutate a **nested** field
  (`client.redirect_uris`) after the call, then read and assert the served entry is unaffected.
* **(d)** `update_credentials_refreshes_the_published_value` — save, read (warm), then
  `update_credentials` with a different access token, record `read_count`, read, and assert the new
  token is served **with zero further backend reads**.
* **(e)** Fix **F4**: rename `remove_evicts_even_when_the_cache_is_disabled` to
  `remove_evicts_on_the_gate_on_path`, keeping its body; add
  `remove_and_invalidate_are_honoured_with_the_gate_off` built with
  `store_with_env(Arc::new(|k| (k == AUTH_CACHE_DISABLED_ENV[0]).then(|| "1".to_string())))`,
  asserting (i) `invalidate_cache` is harmless while disabled, and (ii) `remove_auth_entry`
  still clears the slot so the next read reaches the backend and returns `None`.

---

## Acceptance Criteria

**Harness A — `FakeEnv`**

- [ ] `crates/cyrup-mcp/src/proxy.rs`'s `FakeEnv` carries `call_fault`, `elicitation_action`,
      `guard_spill`, `complete_auth_status` and `complete_auth_inputs`, all `Default`-constructible,
      with a `CallFault` recipe enum that mints a `ProxyCallError` per call.
- [ ] `FakeEnv::{call_tool, read_resource}` return `Err` when a fault is scripted; every existing
      test in the module is unchanged and still passes.
- [ ] `FakeEnv::{handle_url_elicitation_required, complete_auth_from_input, guard_mcp_output}` honour
      their scripted values and default to today's behaviour when unset.

**MCP-165**

- [ ] A test asserts `auth_required` with `details.autoAuthAttempted` present, for both a carried
      `auth_message` and the `get_auth_required_message` fallback.
- [ ] A test table-drives all three `UrlElicitationAction`s, asserting the exact message and
      `details.action` for each.
- [ ] A test asserts `call_failed` for a plain error and `aborted` for an `McpError::Aborted`.
- [ ] A test asserts a spilled guard replaces `details.message` with
      `output truncated; see outputGuard.fullOutputPath` and that `details.outputGuard` is present.
- [ ] A test reaches `catch_arm` through the **resource** path (`resource_uri` set), covering the
      second call site.

**MCP-168**

- [ ] A test drives `McpTool::execute` with `redirectUrl`, `code` and `input` in turn and asserts all
      three reach `complete_auth_from_input` with their own value.
- [ ] The same test asserts precedence: all three keys at once records only `redirectUrl`'s value.
- [ ] A test asserts a non-`"authenticated"` status yields `details.error == "not_authenticated"`,
      echoes the status, renders the exact sentence, and leaves the connection open.

**MCP-151 / MCP-152 / MCP-192 — one literal each**

- [ ] `registration::proxy_tool_parameters` and `ProxyTool::parameters` both resolve to
      `proxy::mcp_tool_schema()`; no second schema literal exists in the crate.
- [ ] `PROXY_DESCRIPTION_HEAD` and `PROXY_DESCRIPTION_USAGE` exist once and are used by both
      `build_proxy_description` functions.
- [ ] `both_proxy_descriptions_…` asserts **whole-string** equality for the empty config and for a
      three-server config (one disabled, one with cached instructions, one with cached tools).
- [ ] `registration::PROXY_TOOL_PROMPT_GUIDELINE` is defined as `crate::proxy::MCP_TOOL_GUIDELINE`;
      the stale "exactly two occurrences" claim at registration.rs:156-161 is corrected.
- [ ] A test calls the real `cyrup_permission_system::sanitize::tools::sanitize_available_tools_section`
      and asserts the guideline bullet **survives** with `mcp` allowed and is **removed** with `mcp`
      denied, with an unrelated bullet surviving both. Asserted for the constant the registered
      `ProxyTool` carries.

**MCP-171**

- [ ] `config::locale_compare` builds at most one `feruca::Collator` per thread; no `Collator::new`
      remains on the per-comparison path.
- [ ] The implementation contains no `panic!`, `unwrap` or `expect`, and degrades to a fresh collator
      on TLS-unavailable / already-borrowed.
- [ ] The existing ordering assertions at config.rs:5098-5116 pass unchanged, plus one asserting a
      repeated comparison is stable.
- [ ] The "not on any hot path" paragraph at config.rs:4044-4046 names `rank_collate` and the three
      sort sites instead.

**MCP-278 / MCP-287 — `crates/cyrup-it/tests/mcp/keyring_recovery.rs`**

- [ ] The module is registered from `tests/mcp/main.rs`; **no new `[[test]]` target is added** — the
      `mcp` target at `crates/cyrup-it/Cargo.toml:198-204` already exists.
- [ ] A fixture `keyctl` enforces `$1 == "session"`, `$2 == "-"`, exits 64 otherwise, and does
      `shift 2; exec "$@"`.
- [ ] A revoked-keyring read is served through the recovery helper (marker file created, no exit-64
      error).
- [ ] A generic (non-revocation) store failure produces the store's own error and the marker file is
      never created.
- [ ] A 30-second helper yields exactly
      `Linux keyring recovery helper could not start: <keyctl path> timed out after 10000 ms`, and
      the call returns in under 20 s.
- [ ] `{"ok":false,"error":"boom"}` + `exit 1` yields
      `Linux keyring recovery helper failed with exit code 1`; the same reply at `exit 0` yields
      `boom`; an empty `error` at `exit 0` yields `Linux keyring recovery helper failed`.
- [ ] No test in the module calls `std::env::set_var` or `std::env::remove_var`; every switch is
      injected through `EnvFn`.

**MCP-269 — `crates/cyrup-it/tests/mcp/credential_isolation.rs`**

- [ ] A source guard asserts no non-comment line under `crates/cyrup-mcp/src` names
      `cyrup_config::auth` or `cyrup_config`'s `AuthStore`, without false-positiving on
      `McpAuthStore` / `AuthStoreError` / `AuthStorageOptions`.
- [ ] A behavioural guard runs save → update credentials → update state → remove and asserts
      `<agent_dir>/auth.json` never exists and no file under the scratch tree contains the token
      literal.
- [ ] `TODO(MCP-269)` at `crates/cyrup-mcp/src/credentials.rs:80-82` is replaced with a pointer to
      the two guards.

**MCP-283 — `crates/cyrup-mcp/src/credentials.rs`**

- [ ] Cache-hit and store-reload paths are asserted to produce an identical `serde_json::Value` for
      an entry seeded with an unknown key and a `redirect_uris` list, and the stored payload no
      longer carries the unknown key.
- [ ] A chunked entry is reconstructed once and then served with zero further backend reads.
- [ ] Clone isolation is asserted in the **write** direction at nested-field granularity.
- [ ] `update_credentials` refreshes the published value and the next read costs zero backend reads.
- [ ] `remove_evicts_even_when_the_cache_is_disabled` is renamed to match the fixture it builds, and
      a new test with `CYRUP_MCP_DISABLE_AUTH_CACHE=1` asserts `invalidate_cache` is harmless and
      `remove_auth_entry` still clears the slot.
- [ ] `TODO(MCP-278)` / `TODO(MCP-283)` / `TODO(MCP-287)` at
      `crates/cyrup-mcp/src/credentials.rs:70-79` are updated to reflect what now exists, including
      the correction that the `cyrup-it` target was already declared.
