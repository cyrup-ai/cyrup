# 13f · Credential storage, keychain and consent

> Part of **[13 — cyrup-mcp](13-cyrup-mcp.md)**, which holds the thesis, the seam map, the
> architecture and the one canonical table of every port unit. Method and phasing are in
> **[MCP-PORT-METHODOLOGY.md](MCP-PORT-METHODOLOGY.md)**.

Upstream is `pi-mcp-adapter` v2.25.0. cyrup is branch `david/cyrup`. rmcp is the checkout at
`/Users/davidmaple/cyrup.ai/rmcp` (`rmcp-v3.1.2-7-gf713ebd`).

This subsystem is the adapter's credential vault, and after the dependency decision it is **the only
part of the OAuth story that is still hand-written**. `rmcp::transport::auth` owns the protocol —
protected-resource metadata, authorization-server metadata, dynamic client registration, PKCE S256,
code exchange, refresh, scope upgrade — and it reaches persistence through exactly two object-safe
traits, `CredentialStore` (`load`/`save`/`clear`, no key) and `StateStore`
(`save`/`load`/`delete`, keyed by CSRF token). Section 06 is the implementation of those two traits
over the OS keychain, plus everything upstream wrapped around the same keychain that rmcp has no
opinion about: account naming, the chunking manifest, the process-lifetime read cache, the legacy
plaintext import, the URL binding, and the Linux revoked-keyring recovery hop.

It lands as **extension-owned code in a native built-in crate**. `crates/cyrup-mcp` links `keyring`,
`sha2`, `serde_json` and `std::process` directly, the same way `cyrup-ext-subagents` links
`tokio::process`, `nix` and `jsonschema` directly. Nothing in this section needs a new host surface:
`HostServices` is the capability wall for a *WASM guest*, and a native crate on the far side of it
opens its own keychain handles. The only host verbs this section touches at all belong to other
sections' dialogs (`HostServices::{confirm, input, oauth_prompt, notify}`), and it needs none of
them itself.

The single most important behavioural fact to carry across: **the OS keychain is the only store, and
there is no plaintext fallback.** Upstream throws `OAuth secure credential storage is unavailable.
Configure the OS credential store and retry authentication.` rather than degrading, and the
`mcp-oauth/sha256-<hash>/tokens.json` path that still appears throughout `mcp-auth.ts` is
**import-only** — read once, written into the keychain, then the file *and its directory* are
removed. Any port that reintroduces a plaintext write is a security regression against upstream, and
it collides with cyrup's current posture, where every provider credential is a plaintext-JSON
`auth.json` at mode 0600 (`cyrup_config::auth::AuthStore`, written under
`cyrup_config::lock::FileLock`). cyrup has **no OS keychain code today** — a `keyring | keyctl |
secret-service | keyutils | security-framework` grep over `crates/**/*.rs` and every `Cargo.toml`
returns nothing. `keyring` is a new dependency and this section is where it enters the workspace.

The second fact is layering. Three correctness surfaces stack here and collapsing them ships a bug.
(1) A **payload layer** that chunks one JSON record across N keychain accounts because Windows
Credential Manager caps a value at `CRED_MAX_CREDENTIAL_BLOB_SIZE` = 2560 bytes stored as UTF-16,
i.e. 1280 characters. (2) A **backend layer** — the real store plus four fault-injection stores
upstream hand-rolled, which in Rust collapse onto `keyring_core::mock::Store` + `set_error`. (3) A
**process-lifetime cache layer** keyed by server name alone, whose invalidation points live in
`server-manager.ts` and `session-recovery.ts`, i.e. in another section. Get the layering wrong and a
keychain write silently succeeds while reads keep serving a stale token — the bug class that makes a
401 loop unkillable.

The third: `mcp-keyring-helper.cjs` is a **separate process**, and the reason survives the port to
Rust unchanged. On Linux with the `linux-keyutils` backend, credentials live in the **kernel session
keyring**. When a session keyring is revoked — a routine outcome of `su`, a detached tmux/systemd
session, or an SSH session whose keyring was reaped — *every* keyring syscall from that process fails
with `KeyRevoked` and **no in-process API can recover**: the process is permanently attached to a
dead keyring. The only fix is to perform the call from a process that joined a *fresh* session
keyring, which is exactly what `keyctl session -` does — it creates an anonymous session keyring and
`exec`s its remaining argv inside it. That `keyring` links in-process in Rust changes nothing about
this: an in-process library is precisely what cannot recover. So the mechanism (`keyctl session -
<program>`) is preserved literally, with `<program>` becoming `std::env::current_exe()` under a
hidden subcommand — the pattern `crates/cyrup/src/intercom_broker_cmd.rs` and
`crates/cyrup/src/subagent_runner_cmd.rs` already establish, both pre-dispatched from `main()` before
any clap parsing.

Finally, **consent is not in this port.** `ConsentManager` exists solely to gate an MCP-UI iframe
re-entering the host at `POST /proxy/tools/call`; its only consumers are `ui-server.ts` and
`ui-session.ts`, both removed by Cut 2. It goes with them. The model-facing approval gate that shares
its name in the brief — `tool-approval.ts`'s `approveTools` matching and the session
`approvedToolCalls` cache — is a different mechanism on a different path, it survives, and it belongs
to the approval section. The seam is stated below.

---

### How it lands

| adapter capability | upstream mechanism | cyrup mechanism | verdict |
|---|---|---|---|
| OAuth protocol: PRM/AS discovery, DCR, PKCE S256, exchange, refresh, scope upgrade | `mcp-oauth-provider.ts` + `@modelcontextprotocol/client` | `rmcp::transport::auth::{AuthorizationManager, OAuthState, AuthorizationSession}` | **rmcp** |
| Where credentials persist | `@napi-rs/keyring` `Entry(service, account)` | `keyring` 4.1.6 → `keyring_core::Entry::{new, get_password, set_password, delete_credential}` | **extension-owned** |
| How rmcp reaches that store | n/a — the TS SDK provider owns storage | `impl rmcp::transport::auth::CredentialStore` + `impl StateStore`, one instance per server, handed to `AuthorizationManager::{set_credential_store, set_state_store}` | **hand-written** |
| Stored record shape | `AuthEntry { tokens, clientInfo, codeVerifier, oauthState, serverUrl }` | `AuthEntry { credentials: StoredCredentials, client: StoredClientInfo, state: StoredAuthorizationState, serverUrl }` | **hand-written** |
| Account naming | `sha256-${sha256_hex(utf8(serverName))}` | same, via `sha2::Sha256` | **hand-written** |
| Chunking past the Windows value cap | manifest at the base account + `.chunk.<digest>.<i>` accounts | same, chunked on char boundaries | **hand-written** |
| Windows 2560-byte ceiling | `sizeLimitedAuthSecretStore` string-matched by tests | `keyring_core::Error::TooLong(name, limit)` — a **typed** variant, same `Display` text | **extension-owned** |
| "no such credential" | `Entry.getPassword() → null` | `Err(keyring_core::Error::NoEntry)` mapped to `Ok(None)` | **extension-owned** |
| "store unreachable" | `getKeyringEntry` throw around dynamic `.node` load | `keyring_core::Error::NoDefaultStore` / `keyring::Entry::store_status()` | **extension-owned** |
| Dynamic native-binding fallback across 12 triples | `loadKeyringEntryClass` + fallback table | none — backends are statically linked | **extension-owned** (dissolved) |
| Linux revoked-keyring recovery | `spawnSync("keyctl","session","-","node",helper)` | `keyctl session - <current_exe()> __mcp-keyring-helper`, same stdin/stdout JSON, same 10 s cap | **hand-written** |
| The helper process | `mcp-keyring-helper.cjs` | hidden `__mcp-keyring-helper` subcommand of the `cyrup` binary, beside `intercom_broker_cmd`/`subagent_runner_cmd` | **hand-written** |
| Legacy plaintext import + delete | `tokens.json` read → keychain write → `rmSync` file then dir | `std::fs` | **hand-written** |
| Legacy base dir resolution | `MCP_OAUTH_DIR` → `settings.oauthDir` → `getAgentPath('mcp-oauth')` | same order over `cyrup_config::env`'s `ConfigDirs.agent_dir` (`CYRUP_AGENT_DIR` → `PI_CODING_AGENT_DIR`) | **extension-owned** |
| Process-lifetime read cache + kill switch | `authEntryCache` Map + `PI_MCP_ADAPTER_DISABLE_AUTH_CACHE` | `HashMap<String, Option<AuthEntry>>` under an `RwLock`, dual-read env | **hand-written** |
| Token expiry check and refresh | `isTokenExpired` + `getValidToken` | `AuthorizationManager::get_access_token` (30 s `REFRESH_BUFFER_SECS`) → `try_refresh_or_reauth` → `refresh_token` | **rmcp** |
| SDK token-shape conversion | `oauth-handler.ts` `getStoredTokens` → `OAuthTokens` | none needed — `StoredCredentials.token_response` **is** `oauth2::StandardTokenResponse` | **rmcp** (dissolved) |
| Confidential-client secret across restarts | `StoredClientInfo.clientSecret` | rmcp's `StoredCredentials` carries only `client_id`; re-apply via `AuthorizationManager::configure_client(OAuthClientConfig::new(..).with_client_secret(..))` after `initialize_from_store()` | **hand-written** |
| PKCE verifier + CSRF state persistence | `AuthEntry.codeVerifier` / `.oauthState` | `StoredAuthorizationState { pkce_verifier, csrf_token, expected_issuer, require_issuer, created_at, requested_scopes }` through `StateStore` | **rmcp** shape, **hand-written** store |
| Serialized read-modify-write | none — JS single-threadedness supplies it | per-server-name `tokio::sync::Mutex` inside the store | **hand-written** |
| Fault-injection test backends | five hand-rolled stores + one env selector | `keyring_core::mock::Store` + `Store::set_error(Error)` | **extension-owned** |
| Embedder façade (`getMcpOAuthTokensForUrl`, …) | `oauth.ts` | a `pub` module on `cyrup-mcp` | **extension-owned** |
| Browser-iframe tool-call consent | `consent-manager.ts`, `ConsentError` | — | **cut** (Cut 2) |
| Model-facing tool approval | `tool-approval.ts` + `pi.events` broker | `ExtHooks::before_tool_call` + `cyrup_permission_system`'s `create_mcp_permission_targets` | **host-verb** — another section |

---

### Behavioural specification

#### 6.1 Constants — reproduce every one of these literally

| constant | value | where | why that value |
|---|---|---|---|
| keychain **service** | `pi-mcp-adapter.oauth` upstream; **`cyrup.mcp.oauth`** in the port | `mcp-auth.ts` `AUTH_SECRET_SERVICE` | the durable identity of every credential, and user-visible in Keychain Access / seahorse / `keyctl show`. The rename is forced — see MCP-280. |
| `AUTH_SECRET_CHUNK_SIZE` | `1000` | `mcp-auth.ts` | Both the chunk width and the chunking **threshold**. A threshold *above* the value limit is a pinned regression. |
| `AUTH_SECRET_VALUE_LIMIT` | `1280` | `mcp-auth.ts` | `CRED_MAX_CREDENTIAL_BLOB_SIZE` (2560 bytes) ÷ 2, because Windows stores the blob as UTF-16. |
| `AUTH_CHUNK_MANIFEST_KEY` | `__piMcpAdapterOAuthChunked` | `mcp-auth.ts` | The discriminator that makes a payload a manifest rather than an entry. Keep the literal — it is a stored-format token, not branding. |
| `KEYRING_RECOVERY_TIMEOUT_MS` | `10_000` | `mcp-auth.ts` | Wall-clock cap on the `keyctl` subprocess. |
| helper stdin cap | `1024 * 1024` (1 MiB) | `mcp-keyring-helper.cjs` `readStdin`; parent `maxBuffer` in `runLinuxKeyringRecoveryOperation` | Both sides cap independently. |
| `TEST_AUTH_STORE_ENV` | `PI_MCP_ADAPTER_TEST_AUTH_STORE` | `mcp-auth.ts` | Backend override; `memory` \| `sizelimited` \| `unavailable` \| `keyrevoked`, matched by **exact** string equality. |
| `AUTH_CACHE_DISABLED_ENV` | `PI_MCP_ADAPTER_DISABLE_AUTH_CACHE` | `mcp-auth.ts` `isAuthEntryCacheEnabled` | `=== '1'` disables. Any other value — `"true"`, `"0"`, empty — leaves it **enabled**. |
| `KEYRING_RECOVERY_DISABLED_ENV` | `PI_MCP_ADAPTER_DISABLE_KEYRING_RECOVERY` | `mcp-auth.ts` | `=== '1'` disables recovery entirely. |
| `KEYRING_RECOVERY_KEYCTL_ENV` | `PI_MCP_ADAPTER_KEYRING_RECOVERY_KEYCTL` | `mcp-auth.ts` | Overrides the `keyctl` program path (trimmed; blank ⇒ `"keyctl"`). |
| `KEYRING_RECOVERY_HELPER_ENV` | `PI_MCP_ADAPTER_KEYRING_RECOVERY_HELPER` | `mcp-auth.ts` | Overrides the helper; upstream's default resolves `./mcp-keyring-helper.cjs` against `import.meta.url`, the port's default is `current_exe()`. |
| `TEST_LINUX_KEYRING_RECOVERY_ENV` | `PI_MCP_ADAPTER_TEST_LINUX_KEYRING_RECOVERY` | `mcp-auth.ts` | `=== '1'` forces the recovery path on non-Linux, for tests. |
| `MCP_OAUTH_DIR` | (env, unprefixed) | `mcp-auth.ts` `getAuthBaseDir` | Highest-precedence legacy-import base dir. Stays **unprefixed and unchanged** — a user may point it at a real pi install. |
| `PI_MCP_ADAPTER_KEYRING_RECOVERY_NODE` | (env) | `mcp-auth.ts` | **Does not port** — it names a JavaScript interpreter and there is none. Recorded in Coverage. |
| `PI_MCP_ADAPTER_FAKE_KEYRING_STORE` | (env) | `__tests__/mcp-auth-storage.test.ts` fixtures only | Read by the *fake* helper, never by `mcp-auth.ts`. The Rust fixture needs an equivalent. |

Every surviving `PI_MCP_ADAPTER_*` switch is read dual-name, `CYRUP_MCP_<SUFFIX>` first then
`PI_MCP_ADAPTER_<SUFFIX>` — the convention `cyrup_config::env` already uses
(`["CYRUP_AGENT_DIR", "PI_CODING_AGENT_DIR"]`) and `cyrup_provider::auth::oauth::callback` already
uses (`["CYRUP_OAUTH_CALLBACK_HOST", "PI_OAUTH_CALLBACK_HOST"]`).

#### 6.2 The stored record

Upstream's on-keychain payload is `JSON.stringify(AuthEntry)`:

```
AuthEntry { tokens?, clientInfo?, codeVerifier?, oauthState?, serverUrl? }
StoredTokens     { accessToken (req), refreshToken?, expiresAt? /*fractional Unix seconds*/, scope?, issuer? }
StoredClientInfo { clientId (req), clientSecret?, clientIdIssuedAt?, clientSecretExpiresAt?,
                   redirectUris?, issuer?, configPreRegistered? }
```

Three of those five slots are now typed by rmcp, so the port's record is:

```rust
struct AuthEntry {
    credentials: Option<rmcp::transport::auth::StoredCredentials>,        // replaces `tokens`
    client:      Option<StoredClientInfo>,                               // the DCR fields rmcp drops
    state:       Option<rmcp::transport::auth::StoredAuthorizationState>,// replaces codeVerifier + oauthState
    server_url:  Option<String>,
}
```

* `StoredCredentials { client_id, token_response: Option<OAuthTokenResponse>, granted_scopes,
  token_received_at: Option<u64>, issuer }` — `#[non_exhaustive]`, `Serialize + Deserialize`,
  constructed with `StoredCredentials::new(..).with_issuer(..)`, fields public thereafter. Its
  `Debug` already redacts `token_response` as `[REDACTED]`.
* `StoredAuthorizationState { pkce_verifier, csrf_token, expected_issuer, require_issuer, created_at,
  requested_scopes }` — `#[non_exhaustive]`, `Serialize + Deserialize`, constructed with
  `StoredAuthorizationState::{new, new_with_expected_issuer}` + `with_requested_scopes`. Its `Debug`
  redacts both secrets. Keeping **one** state slot per server, and having `StateStore::load(csrf)`
  return it only when `state.csrf_token == csrf`, reproduces upstream's single `oauthState` slot
  exactly while satisfying rmcp's keyed trait.
* `StoredClientInfo` stays hand-written because rmcp's `StoredCredentials` persists **only**
  `client_id` — see MCP-290.

`configPreRegistered: true` marks an entry written by the config-`clientId` path of
`saveClientInformation` (`mcp-oauth-provider.ts`). `clientInformation()` refuses to return such a
stub — detected either by the explicit marker or by the legacy shape `{clientId, issuer}` with no
`clientSecret`/`clientIdIssuedAt`/`clientSecretExpiresAt`/`redirectUris`. Serving it would send a
refresh with a `client_id` and no secret, drawing `invalid_client` and wiping credentials. The
enforcement lives in section 07; storage must carry the flag through verbatim or the enforcement has
nothing to read.

**Normalization on read is strict and field-typed** (`toAuthEntry` / `toStoredTokens` /
`toStoredClientInfo` / `toRecord` / `optionalString` / `optionalNumber` / `optionalBoolean` /
`stringArray` in `mcp-auth.ts`). Four rules:

1. **Unknown keys are dropped, not rejected.** serde's default (ignore-unknown) matches; do **not**
   add `deny_unknown_fields`. Pinned end to end by `__tests__/mcp-auth-cache.test.ts`, which saves
   `{tokens:{accessToken:"a",unexpected:"discard"},unexpected:true}` and asserts both the cached and
   the re-read value are exactly `{tokens:{accessToken:"a"},serverUrl}`.
2. **A wrong-typed *optional* field poisons the whole entry.** The `optional*` helpers return
   `undefined` for absent and `null` for wrong-typed; any `null` makes the containing converter
   return `undefined`, which makes `parseAuthEntryPayload` throw ``Failed to parse OAuth credentials
   for ${serverName} from ${source}: invalid credential shape``. serde's typed deserialization gives
   this for free.
3. **A missing *required* field does the same**: `tokens` without a string `accessToken`;
   `clientInfo` without a string `clientId`. Note the asymmetry: `tokens` present-but-invalid is
   fatal, `tokens` **absent** is fine.
4. `redirectUris` is accepted only if it is an array of strings; a mixed array yields "omit the
   field", *not* "reject the entry", because `stringArray` never returns `null`. This one field
   degrades silently. Reproduce with `#[serde(default, deserialize_with = …)]` mapping a
   non-`Vec<String>` to `None` rather than erroring.

`toRecord` also rejects a JSON **array** payload the same way it rejects a scalar.

`serverUrl` is set by `saveAuthEntry` as a **side effect on the caller's object** when a `serverUrl`
argument is supplied. Port as `&mut AuthEntry`.

#### 6.3 Key naming in the OS keychain

```
service       = "cyrup.mcp.oauth"                                     // upstream: "pi-mcp-adapter.oauth"
account       = "sha256-" + hex(sha256(utf8(serverName)))             // getAuthEntryAccount
chunk account = format!("{account}.chunk.{chunk_digest}.{index}")     // getAuthEntryChunkAccount
```

The same `sha256-<64 hex>` token is also the **legacy directory name** (`getServerDir`), which is
what makes an arbitrary configured server name path-safe. `__tests__/mcp-auth-storage.test.ts` pins
this against hostile names — `"Cloudflare Workers"`, `"сервер"`, `"../escape"`, `"@scope/name"`,
`""` — asserting the relative legacy path always matches `/^sha256-[a-f0-9]{64}\/tokens\.json$/` and
that `<authDir>/../escape/tokens.json` never exists.

Two consequences a porter must not miss:

* **The account is derived from the server name only.** Not from the URL, not from `oauthDir`, not
  from the config file the server came from. Two projects configuring a server named `github` share
  one keychain entry. The test *does not use configured oauthDir values as secure-store namespaces*
  pins this deliberately: writes through two different options objects land on the same account and
  both reads return the *last* write.
* **The empty string is a valid server name** with a valid account (`sha256` of zero bytes). Do not
  special-case it.

`AuthStorageOptions.baseDir` affects **only** the legacy import path. `getAuthBaseDir` precedence:
`MCP_OAUTH_DIR` (trimmed, non-empty) → `options.baseDir` → `getAgentPath('mcp-oauth')`.
`options.baseDir` comes from `getAuthStorageOptions(settings.oauthDir, cwd)` →
`resolveConfiguredOAuthDir` (`config.ts`), which throws `"settings.oauthDir must be a string"` on a
non-string, returns `undefined` for `undefined`/`null`/blank, and otherwise
`path.resolve(cwd, trimmed)`. In the port, `getAgentPath('mcp-oauth')` becomes
`<ConfigDirs.agent_dir>/mcp-oauth`, resolved by `cyrup_config::env` through
`CYRUP_AGENT_DIR` → `PI_CODING_AGENT_DIR` → `<home>/.cyrup/agent`, with `~` expansion — so a user who
still has `PI_CODING_AGENT_DIR` set finds their pi install's legacy files, which is the only reason
the default path has any value at all.

`getAuthBaseDir` leaks out of this module into `mcp-auth-flow.ts` as part of two **in-flight dedup
keys** — ``${serverName}|${getAuthBaseDir(options)}`` and
``${serverName}|${serverUrl}|${getAuthBaseDir(authStorageOptions)}``. The base dir is *not* dead once
import is done: it still partitions concurrent auth attempts. Keep the accessor public.

#### 6.4 Chunking — the write algorithm, in order

`writeSecureAuthEntryToStore(store, serverName, entry)`:

1. `account = getAuthEntryAccount(serverName)`.
2. `payload = JSON.stringify(entry)` — **compact**, no whitespace. The comment states why:
   *"Compact: multiline secrets corrupt gnome-keyring plaintext (GKeyFile) collections."* Use
   `serde_json::to_string`, never `to_string_pretty`.
3. `previousManifest = readExistingChunkManifest(store, serverName, account)` — a *swallow-all*
   read: any error, and any payload that is not a manifest, yields `undefined`.
4. `manifest = payload.length > 1000 ? createChunkManifest(payload) : undefined`, where
   `chunkCount = ceil(payload.length / 1000)` and `chunkDigest = hex(sha256(utf8(payload)))[0..16]`.
5. If chunking: write chunks `0..chunkCount` **first**, each
   `payload.slice(i*1000, (i+1)*1000)` to `{account}.chunk.{digest}.{i}`; **then** write
   `JSON.stringify(manifest)` to the base `account`. *Order is load-bearing:* a crash between them
   leaves orphan chunks while the base account still holds the previous good value, so reads stay
   consistent.
6. Else write `payload` to the base `account`.
7. If `previousManifest?.chunkDigest !== manifest?.chunkDigest`,
   `tryRemoveChunkPayloads(store, account, previousManifest)` — best-effort, errors swallowed
   (*"Stale chunk cleanup must not hide a successful credential write."*). When the digest is
   unchanged the old chunk accounts **are** the new ones, so skipping cleanup is correct, not lazy.
   Both-`None` is a no-op.
8. On any throw in 5–7: `tryRemoveChunkPayloads(store, account, manifest)` — remove the **new**
   chunks — then throw `OAuthCredentialStoreError('write')` with message ``Failed to write OAuth
   credentials for ${serverName} to the OS secure credential store``.
9. On success: `publishAuthEntryToCache(serverName, payload)`. This sits *outside* the `try`, so a
   failed write never publishes.

Manifest JSON, key order as emitted (JS insertion order; in Rust, struct declaration order):

```json
{"__piMcpAdapterOAuthChunked":1,"chunkCount":7,"chunkDigest":"a1b2c3d4e5f60718"}
```

`isAuthEntryChunkManifest` validates all four properties: the key `=== 1` (strict, not truthy),
`chunkCount` a number that `Number.isInteger` and `> 0`, `chunkDigest` matching `/^[a-f0-9]{16}$/`.
A payload failing any check is treated as an ordinary entry. Upstream imposes **no upper bound** on
`chunkCount` — see MCP-286.

**Read** (`readChunkedAuthEntry`): read each chunk account in index order; a chunk that is
`undefined` throws ``Missing OAuth credential chunk ${chunkAccount} for ${serverName}`` wrapped in
`OAuthCredentialStoreError('read')`; `chunks.join('')`; then `parseAuthEntryPayload(...,
'OS secure credential store chunks')`. The test pins that a deleted chunk surfaces as
`inspectAuthForUrl(...).status === "unavailable"`, **not** `"absent"` — a partially-lost credential
must never look like "no credential".

**Remove** (`removeAuthEntryFromStore`): read the base account; if it parses as a manifest, remove
all chunk accounts via `removeChunkPayloads` — the **non**-best-effort variant — then remove the base
account. Any throw → `OAuthCredentialStoreError('remove')`. That asymmetry against the write path is
deliberate; see MCP-285.

> **The one genuine Rust hazard.** `payload.length` and `payload.slice()` are **UTF-16 code units**
> while `chunkDigest` hashes the **UTF-8** bytes of the same payload. Rust's `String::len()` is UTF-8
> bytes and `&s[a..b]` panics off a char boundary. `JSON.stringify` does not escape non-ASCII BMP or
> astral characters, so a `scope` string, an `issuer` host or a token with any non-ASCII character
> reaches the chunker verbatim, and upstream's `slice` can even split a surrogate pair into a lone
> surrogate that Security.framework and gnome-keyring (both UTF-8) cannot faithfully store. Because
> the port's payload is **never** read by a JS writer (the record shape changed and the service name
> changed — MCP-280), byte-boundary chunking is free: chunk over `char_indices()`, cutting at the
> largest char boundary ≤ `i*1000` bytes, so a chunk is ≤1000 bytes (well under the 1280 ceiling) and
> no code point is split. Self-consistency is the whole contract; cross-implementation
> byte-compatibility is not one.

#### 6.5 Backend selection and the real `keyring` API

`getAuthSecretStore()` dispatches on `PI_MCP_ADAPTER_TEST_AUTH_STORE` with **exact** string equality:

| value | store | behaviour | bumps read counter? |
|---|---|---|---|
| `"memory"` | `memoryAuthSecretStore` | `Map<account,payload>` | yes |
| `"sizelimited"` | `sizeLimitedAuthSecretStore` | same map, but `write` throws ``Value of 'password encoded as UTF-16' is longer than the platform limit of 2560 chars`` when `payload.length > 1280`. Mimics Windows. | yes |
| `"unavailable"` | `unavailableAuthSecretStore` | every op throws `'simulated secure credential store unavailable'` | **yes** |
| `"keyrevoked"` | `keyRevokedAuthSecretStore` | every op throws `Error("Couldn't access platform storage: KeyRevoked", { cause: Error("KeyRevoked") })` — i.e. the recovery predicate matches | **yes** |
| anything else / unset | `keyringAuthSecretStore` | `new Entry(service, account)` → `getPassword()` / `setPassword()` / `deleteCredential()` | no |
| *not selectable* | `linuxKeyringRecoveryAuthSecretStore` | each op is one `keyctl session -` subprocess round trip; entered **only** as a retry (§6.7) | no |

All four **read**-path counter bumps are pinned by `__tests__/mcp-auth-cache.test.ts` (the throwing
pair under `PI_MCP_ADAPTER_DISABLE_KEYRING_RECOVERY=1` so the throw is observable). `memory` and
`sizelimited` share the same backing map and the same counter.
`resetTestAuthSecretStore()` clears map + cache + counter together; `resetAuthEntryCache()` clears
only the cache and **leaves the counter alone**. The three inspection exports —
`getTestAuthSecretStoreReadCount`, `getTestAuthSecretStoreEntries`, `removeTestAuthSecretStoreEntry`
— are how both test files reach inside the store; they are contract, not incidental.

**What the `keyring` 4.1.6 crate actually gives**, read from the published source, not assumed:

* The `v1` feature re-exports `keyring::Entry` wrapping `keyring_core::Entry`, with
  `Entry::new(service, username) -> Result<Self>` — **`new` now returns `Result`**, unlike the 3.x
  API and unlike `@napi-rs/keyring`. `set_password`, `get_password`, `set_secret`, `get_secret`,
  `delete_credential` are all `Result`-returning.
* `keyring::Entry::store_status() -> &'static Result<()>` reports the one-time credential-store
  initialisation. That is the exact analogue of upstream's `getKeyringEntry` try/catch and is where
  the ``OAuth secure credential storage is unavailable. Configure the OS credential store and retry
  authentication.`` message attaches.
* `keyring_core::Error` is the whole taxonomy, and three variants map directly onto behaviour
  upstream had to string-match:
  * `NoEntry` — `Display` `"No matching credential found"`. **Absence is an `Err`, not an
    `Ok(None)`.** The store adapter must map it to `Ok(None)` or every fresh server looks like a
    store failure.
  * `TooLong(name, limit)` — `Display` ``Value of '{name}' is longer than the platform limit of
    {limit} chars``, which is **byte-identical** to the string upstream's `sizeLimitedAuthSecretStore`
    fabricates. This confirms `@napi-rs/keyring` binds this crate family, and it means the Windows
    ceiling is a *typed* condition in Rust rather than a message match.
  * `NoStorageAccess(err)` — `Display` ``Couldn't access platform storage: {err}``, again identical
    to upstream's simulated `KeyRevoked` message, and it is the variant whose `source()` carries the
    platform error the revoked-keyring predicate walks.
  * `NoDefaultStore`, `PlatformFailure(err)`, `BadEncoding`, `Invalid`, `Ambiguous`,
    `NotSupportedByStore` complete the set.
* **The four fault-injection backends collapse into one upstream-provided type.**
  `keyring_core::mock::Store` is ungated (no feature) and exposes `set_error(Error)`, so `memory` is
  the bare mock, `sizelimited` is the mock with `TooLong("password encoded as UTF-16", 2560)`,
  `unavailable` is the mock with `PlatformFailure`, and `keyrevoked` is the mock with
  `NoStorageAccess(<KeyRevoked>)`. The read counter and the entry-inspection hooks are the only parts
  still hand-written.
* **Backend availability is a feature decision with a behavioural consequence.** `keyring`'s
  `default = ["v1"]`, and `v1 = ["apple-native-keyring-store/keychain", "windows-native-keyring-store",
  "zbus-secret-service-keyring-store"]` — i.e. on Linux, `v1` selects **Secret Service over zbus, not
  the kernel keyutils store**. The `KeyRevoked` condition the whole of §6.7 exists to handle is
  specific to `linux-keyutils-keyring-store`, which `v1` does not enable. Reaching it means either
  the `cli` feature (`keyring::use_native_store(false)` → keyutils on Linux) or linking
  `keyring-core` plus the store crates directly. This is the section's one genuine open decision —
  see *What does not fit cleanly*.

Upstream's `loadKeyringEntryClass` and its absolute-path native-binding fallback table across
`darwin-{arm64,x64}`, `win32-{arm64,x64,ia32}-msvc`, `linux-{arm64-gnu,arm64-musl,arm-gnueabihf,
riscv64-gnu,x64-gnu,x64-musl}` and `freebsd-x64` **vanish entirely** — the Rust backends are linked
at compile time, so there is no module load to fail and no path to fall back to. The *error* the
table guards against still exists (locked keychain, no D-Bus session, no default store) and must
still produce the same sentence.

#### 6.6 The process-lifetime entry cache

`authEntryCache: Map<serverName, AuthEntry | undefined>`. Note the value type: **absence is cached**
as an explicit `undefined`, distinguished from "not cached" by `Map.has()`. In Rust:
`HashMap<String, Option<AuthEntry>>` + `contains_key`.

Every rule below is pinned by `__tests__/mcp-auth-cache.test.ts`:

1. **Enabled unless** `PI_MCP_ADAPTER_DISABLE_AUTH_CACHE === '1'`. The adapter's own suite runs with
   it set to `"1"` and opts in per test.
2. **Keyed by server name only** — not by `AuthStorageOptions`, matching the account derivation.
3. **Read is cacheable only when `behavior.migrateLegacy !== false` AND the cache is enabled.** The
   comment states the defence: *"Status-only reads deliberately bypass the cache because they do not
   migrate legacy entries"* — a status read must never seed the cache with a value an ordinary read
   would have migrated and normalized differently. Pinned twice: two `inspectAuthForUrl` calls cost
   two backend reads, and an inspect after a warm ordinary read still costs one.
4. **Both get and set deep-clone** (`cloneAuthEntry` via `structuredClone`). Pinned at
   **nested-field** granularity, in *both* directions: mutating `entry.tokens.issuer` and pushing
   onto `entry.clientInfo.redirectUris` on a returned entry must not affect the next read, whether
   that entry came from a miss or a hit. Every mutator does `getAuthEntry(...) ?? {}` then
   `delete entry.tokens`; without the clone those deletes would mutate the cache in place. In Rust an
   owned `AuthEntry` returned **by value** gives this for free — the hazard reappears the moment the
   API hands out `&AuthEntry`, `Arc<AuthEntry>` or a `Cow`.
5. **Writes publish** (`publishAuthEntryToCache`): the payload just written is re-parsed and
   re-normalized through `toAuthEntry`, so the cache holds *the shape a fresh store read would
   return*, not the caller's object. If normalization fails the entry is **deleted**, not set.
   Publication is gated on the enable flag, which is what makes the suite's `writeBehindTheCache`
   helper work — so the flag must be read **per call**, not captured once.
6. **Store failures are never cached**: two consecutive throwing reads both throw and leave no entry,
   so a later working read returns the true value rather than a poisoned one.
7. **Invalidation points, all outside this file.** `removeAuthEntry` deletes the key
   **unconditionally**, i.e. *not* behind the enable flag — a removal performed while the cache is
   off still evicts. `invalidateAuthEntryCache(serverName)` is called from `server-manager.ts` twice
   (connection setup got a 401 on an OAuth-capable server; the connect loop got a 401) and from
   `session-recovery.ts` once (an in-flight call got `UnauthorizedError`/HTTP 401). All three gate on
   `supportsOAuth(definition)`, and the two `server-manager` sites additionally guard with an
   `invalidated` boolean so a single connect attempt invalidates at most once.
   `invalidateAuthEntryCache` evicts only its target and is harmless while disabled.
   `resetAuthEntryCache()` has **no non-test caller** at v2.25.0.
8. There is **no TTL and no size bound.** The cache lives for the process.

#### 6.7 Linux keyring recovery — the separate process

Trigger predicate, both halves required (`shouldAttemptLinuxKeyringRecovery`):

* `isLinuxKeyringRecoveryEnabled()`: `PI_MCP_ADAPTER_DISABLE_KEYRING_RECOVERY !== '1'` **and**
  (`process.platform === 'linux'` **or** `PI_MCP_ADAPTER_TEST_LINUX_KEYRING_RECOVERY === '1'`).
* `causeChainContains(error, /key\s*(?:has been\s*)?revoked|keyrevoked/i)`: walks `error.cause`
  transitively with a `Set` cycle guard, testing `name`, `message` and `code` of each link,
  case-insensitively. The loop condition also admits `function` links.

The regex matches, in practice: `KeyRevoked` (the `linux-keyutils` error name), `Key has been
revoked` (the strerror text for `EKEYREVOKED`), and `key revoked`. In Rust the chain is
`std::error::Error::source()`, and `keyring_core::Error::{PlatformFailure, NoStorageAccess,
BadDataFormat}` are the only variants that return a `source`, so the walk terminates naturally.

Recovery is attempted **once**, as a straight retry against
`linuxKeyringRecoveryAuthSecretStore`, at exactly three sites: `writeSecureAuthEntry`,
`readAuthEntry`, `removeAuthEntry`. A second failure propagates.

The subprocess call (`runLinuxKeyringRecoveryOperation`):

```
argv     : <keyctl>  "session"  "-"  <program> [args…]
stdin    : JSON.stringify({ operation, service, account, payload }) + "\n"
encoding : utf8      maxBuffer: 1 MiB      timeout: 10_000 ms      windowsHide: true
```

`payload` is `undefined` for `read`/`remove`, and `JSON.stringify` **omits** undefined values — so
those requests carry **no `payload` key at all**, not `"payload": null`. In Rust:
`#[serde(skip_serializing_if = "Option::is_none")]`.

Response validation, in this order, each with its own message:

1. `result.error` present → ``Linux keyring recovery helper could not start: ${result.error.message}``.
   **This is also the timeout path** — `spawnSync`'s `timeout` populates `result.error`, not
   `result.status`.
2. `result.status !== 0` → ``Linux keyring recovery helper failed with exit code ${result.status ?? 'unknown'}``
3. `JSON.parse(result.stdout.trim())` throws → `'Linux keyring recovery helper returned invalid JSON'`
4. not an object, or `typeof ok !== 'boolean'` → `'Linux keyring recovery helper returned an invalid response'`
5. `ok === false` → `typedResponse.error || 'Linux keyring recovery helper failed'` — **unreachable
   against the real helper**, which sets `process.exitCode = 1` alongside every `ok:false` and
   therefore trips rung 2 first.
6. `operation === 'read' && found === true && typeof value !== 'string'` →
   `'Linux keyring recovery helper returned an invalid read response'`

Store adapter semantics: `read` returns `response.value` only when `ok && found === true`, else
`undefined`; `write`/`remove` discard the response.

The helper itself (`mcp-keyring-helper.cjs`): reads all of stdin as UTF-8, aborting with
`'request too large'` past 1 MiB; `JSON.parse`; validates the request is an object, `operation ∈
{read,write,remove}`, `service` a non-empty string, `account` a non-empty string, and for `write` a
string `payload`; constructs one `Entry(service, account)`; performs the op; writes exactly one line
of JSON to stdout. Read replies `{ok:true,found:false}` or `{ok:true,found:true,value}`;
write/remove reply `{ok:true}`. Any throw replies `{ok:false,error:<message>}` and sets
`process.exitCode = 1`. It carries its own copy of the Linux-only native-binding fallback table —
again, gone in Rust.

The test harness proves the mechanism end to end with a fake `keyctl` that asserts
`$1 == "session" && $2 == "-"` (exiting **64** otherwise), `shift 2`s, and `exec "$@"` — so the port
must pass exactly `session`, `-`, then the program and its args, in that order, with no extra flags.
Its negative twin sets the store to `unavailable` and a `keyctl` that exits 99, and asserts the error
propagates with `/OS secure credential store/` and that the fake store file was **never created** —
recovery must not fire for a generic failure.

#### 6.8 Read / write / remove — full call sequences

`readAuthEntryFromStore(store, serverName, options, behavior)`:

1. `payload = store.read(account)`; a throw becomes `OAuthCredentialStoreError('read', …)` with
   message ``Failed to read OAuth credentials for ${serverName} from the OS secure credential store``.
2. If `payload !== undefined`:
   a. parse as manifest → chunked read, else direct parse. **Neither of these parse throws is
      wrapped** in `OAuthCredentialStoreError` — see MCP-284.
   b. **`removeLegacyAuthEntry(serverName, options)`** — yes, even on a pure read, and yes, even
      under `migrateLegacy: false`. A plaintext file must not survive once the keychain holds the
      record.
   c. return the entry.
3. Else `legacyEntry = readLegacyAuthEntry(...)` (`existsSync` → `readFileSync` → strict parse with
   the file path as the `source` label). If none, return `undefined`.
4. If `behavior.migrateLegacy === false`, **return the legacy entry without writing or deleting** —
   this is what makes status inspection non-destructive *for a server that has no keychain entry
   yet*. It is **not** non-destructive when a keychain entry exists, because of step 2b.
5. Otherwise `writeSecureAuthEntryToStore(...)` then `removeLegacyAuthEntry(...)`, then return.

`removeLegacyAuthEntry`: if the file is absent, no-op. `rmSync(file, {force:true})`; a failure throws
``Failed to remove legacy plaintext OAuth credentials for ${serverName} at ${filePath}``. Then
`rmSync(dir, {recursive:true})` with the error **swallowed** (*"Directory may contain future
non-secret metadata; the plaintext file was already removed."*). Asymmetric on purpose: failing to
delete the secret is fatal, failing to delete the directory is not.

`saveAuthEntry(serverName, entry, serverUrl?, options?)`: set `entry.serverUrl` if `serverUrl` is
truthy; `writeSecureAuthEntry`; `removeLegacyAuthEntry`.

`removeAuthEntry`: `removeAuthEntryFromStore` with the one recovery retry, then
`authEntryCache.delete(serverName)`, then `removeLegacyAuthEntry`. Note the ordering — the cache is
purged **after** the store op, so a throwing remove leaves the cache intact and the next read still
serves the old value. That is upstream behaviour; do not "fix" it silently.

#### 6.9 URL binding, expiry and the mutators

`getAuthForUrl(serverName, serverUrl, options)` is the **fail-closed** accessor: `undefined` when
there is no entry, when `entry.serverUrl` is absent (*"If no serverUrl is stored, this is from an old
version - consider it invalid"*), or when `entry.serverUrl !== serverUrl`. String equality, **no URL
normalization** — a trailing-slash change invalidates the credential.

`inspectAuthForUrl` is the **status** accessor and the only three-state one:
`{status:'present', entry}` | `{status:'absent'}` | `{status:'unavailable', message}`. It reads with
`migrateLegacy: false` (so: no cache, no migration-write), maps "no entry / no `serverUrl` / URL
mismatch" to `absent`, and converts an `OAuthCredentialStoreError` — and *only* that class; anything
else rethrows — into `unavailable` carrying `formatOAuthCredentialStoreUnavailable(error)`, which
returns verbatim:

* `process.platform === 'linux'` **and** the cause chain matches the revoked-key regex →
  `'OAuth credential store unavailable: the Linux session keyring may be revoked. Start Pi from a
  fresh login/keyring session and retry.'`
* otherwise →
  `'OAuth credential store unavailable. Configure or unlock the OS credential store and retry.'`

The doc comment states the invariant the two accessors jointly maintain: *"Authentication operations
continue to use `getAuthForUrl()` directly and therefore remain fail-closed"* — a broken keychain
must degrade the status UI, never grant access. Consumers are `commands.ts`'s `/mcp` status panel and
`oauth.ts`.

**Expiry, and where it now lives.** Upstream stores `expiresAt` as fractional Unix **seconds**,
written as `Date.now() / 1000 + tokens.expires_in` (`mcp-oauth-provider.ts`, whose comment notes
expiry is preserved even when `expires_in === 0` so an already-expired token stays expired), and
reads it through three predicates with three different zero-semantics:

| site | test | `expiresAt = 0` behaves as |
|---|---|---|
| `mcp-auth.ts` `isTokenExpired` | `!expiresAt` | **no expiry** ⇒ `false` |
| `oauth-handler.ts` `getStoredTokens` | `expiresAt !== undefined && expiresAt < now` | **expired** ⇒ returns `undefined` |
| `mcp-oauth-provider.ts` `tokens()` | `expiresAt ? … : undefined` | **no expiry** ⇒ omits `expires_in` |

`isTokenExpired` is itself tri-state: `null` when there is no entry or no `entry.tokens`; `false`
when `!entry.tokens.expiresAt` (JS falsy — `0`, `-0` or `NaN` as well as absent); otherwise
`entry.tokens.expiresAt < Date.now() / 1000`. `hasStoredTokens` is `!!entry?.tokens`, with no expiry
consideration.

**In the port, rmcp owns the live predicate and all three JS variants collapse.**
`AuthorizationManager::get_access_token` reads `token_response.expires_in()` against
`StoredCredentials.token_received_at` (integral epoch seconds), refreshes when the remaining lifetime
is under `REFRESH_BUFFER_SECS = 30`, and returns `AuthError::AuthorizationRequired` when refresh is
impossible. Absolute fractional `expiresAt` survives in exactly **one** place: the legacy-import
converter, which must turn `{accessToken, refreshToken, expiresAt, scope, issuer}` into
`StoredCredentials { client_id, token_response, granted_scopes, token_received_at, issuer }`. There,
`token_received_at = now` and `expires_in = max(0, floor(expiresAt - now))`, which is precisely
`oauth-handler.ts`'s conversion arithmetic, and the `expiresAt = 0` case must resolve to
`expires_in = 0` (already-expired), i.e. the `getStoredTokens` semantic and **not** the
`isTokenExpired` one.

**The four mutators share one algorithm** — read, conditionally purge siblings, set one field, save:

| mutator | sets | deletes when `serverUrl && entry.serverUrl !== serverUrl` |
|---|---|---|
| `updateTokens` | `tokens` | `clientInfo`, `codeVerifier`, `oauthState` |
| `updateClientInfo` | `clientInfo` | `tokens`, `codeVerifier`, `oauthState` |
| `updateCodeVerifier` | `codeVerifier` | `tokens`, `clientInfo`, `oauthState` |
| `updateOAuthState` | `oauthState` | `tokens`, `clientInfo`, `codeVerifier` |

In every case the deleted set is "all four fields except the one being written". The defence: a
server whose URL changed is a different authorization context, so no artifact from the old one may
survive alongside a new one. The base is `getAuthEntry(...) ?? {}` — the *unvalidated-for-URL* read,
so the comparison is against whatever URL is stored. The clearers (`clearCodeVerifier`,
`clearOAuthState`, `clearClientInfo`, `clearTokens`) are trivial read-delete-save pairs, each no-ops
when there is no entry, and each passes `serverUrl: undefined` so a clear never rewrites the stored
URL. `clearAllCredentials` is a bare alias for `removeAuthEntry`; `getOAuthState` is
`entry?.oauthState`.

Note how this rule interacts with rmcp: `AuthorizationManager::initialize_from_store` performs its
*own* version of the same defence on the **issuer** axis — when `stored.issuer` differs from the
current metadata issuer it either clears the store outright or, for a CIMD (`https://…`) client id,
keeps the portable client id and discards the tokens. Both defences are needed and neither subsumes
the other: rmcp fences on the authorization server's identity, the adapter fences on the MCP server's
URL, and only the adapter knows the latter.

**Token refresh does not live in this section.** `getValidToken` (`mcp-auth-flow.ts`) is the refresh
driver and belongs to section 07; what binds it here is (a) it reads via `getAuthForUrl` then
`isTokenExpired`, (b) it refreshes only when expired *and* a refresh token exists, (c) on success it
*re-reads* rather than trusting a return value, and (d) it rethrows `OAuthCredentialStoreError` while
swallowing every other refresh error into `null`. That last one is the contract this section owes it:
the store's error type must stay distinguishable from an ordinary auth failure, or a broken keychain
becomes an infinite silent re-auth loop. In the port the equivalent distinction is
`AuthStoreError::Unavailable` versus rmcp's `AuthError::{AuthorizationRequired, TokenRefreshFailed,
TokenRefreshRejected}`.

#### 6.10 Read-modify-write has no lock upstream, and Rust must replace what JS supplied

Upstream's mutators are read → mutate → write with **no** in-process mutex and **no** cross-process
lock. Two `pi` processes refreshing the same server concurrently can lose one token; one process with
two concurrent flows is protected only by JS's single-threaded execution between `await` points — and
every function in `mcp-auth.ts` is **synchronous**, so within one process the sequence is in fact
atomic.

That last clause does not survive the port. `keyring` calls are blocking syscalls issued from a
multi-threaded tokio runtime, and rmcp's `CredentialStore` is `async_trait` with `&self`, so
`AuthorizationManager` will call `save` from whatever task refreshed. The JS-atomicity guarantee is
gone and must be replaced explicitly, or a concurrent refresh loses a rotated refresh token and locks
the user out until they re-authenticate. cyrup has the right shape one layer down:
`cyrup_provider::auth::store::CredentialStore::modify` is documented as *"THE ONLY write path.
Serialized read-modify-write per provider id"*, implemented in `cyrup_config::auth::AuthStore` with a
per-provider `tokio::sync::Mutex` plus a cross-process `cyrup_config::lock::FileLock` around an
atomic 0600 write. Copy the *shape*, not the type — see MCP-268.

#### 6.11 Consent, and where the seam falls

`ConsentManager` (`consent-manager.ts`) holds two `Set<string>` of server names and a
`ToolConsentMode ∈ {"never","once-per-server","always"}`, defaulting to `"once-per-server"`. It is
constructed in exactly one production place, hard-coded `new ConsentManager("once-per-server")` in
`init.ts`, stored on `McpExtensionState.consentManager` (`state.ts`). Its **only** consumers are
`ui-server.ts` — `requiresPrompt`/`shouldCacheConsent` baked into the host HTML as
`requireToolConsent`/`cacheToolConsent`, `ensureApproved` gating `POST /proxy/tools/call`,
`registerDecision` recording the browser's answer at `POST /proxy/ui/consent` — and `ui-session.ts`,
which threads the state through. Both are Cut 2, so `ConsentManager` and its `ConsentError` pair are
cut with them.

**The seam, precisely.** Two different "may this proceed" records share the word *approval* in this
adapter and only one of them is a browser thing:

* **Cut half — browser consent.** `ConsentManager` + `ConsentError`
  (`CONSENT_DENIED` / `CONSENT_REQUIRED`, under the `McpUiError` umbrella). Actor: an MCP-App iframe
  re-entering the local host server. With no host server and no iframe, it has zero callers.
* **Surviving half — model-facing approval.** `tool-approval.ts`'s `isToolCallApprovalRequired`
  (global + per-server glob matching with legacy-name compatibility), `ensureToolCallApproved`'s
  three-way `Allow once / Allow for session / Deny` select, the session `approvedToolCalls` cache,
  and the headless refusal `{ok:false, reason:"approval_required_headless"}`. Actor: the model
  calling `mcp({tool,…})` or a registered direct tool. That half survives whole, reaches the human
  through `HostServices::select` under `HostServices::human_interaction_lock`, and sits behind
  `ExtHooks::before_tool_call` + `cyrup_permission_system::manager`'s existing
  `create_mcp_permission_targets`. It belongs to the approval section, not this one.

The one thing this section must *not* do is treat the cut half's absence as licence to persist the
surviving half. Upstream persists neither. `cyrup_permission_system::stores::SessionApprovalStore` is
the in-tree precedent for the surviving half's lifecycle — in-memory, session-scoped, cleared at both
session start and session shutdown.

---

### Port units

Verdicts: **`rmcp`** · **`extension-owned`** (the native crate does it with its own dependencies) ·
**`hand-written`** (new code in `cyrup-mcp`) · **`host-verb`** · **`host-addition`** ·
**`open-decision`** · **`cut`**.

Severity is the house scale — `critical` = data loss, silent wrong output, a permission bypass, or a
crash on a normal path. Blocking-ness is in the body, never in the rating.

**MCP-250 — The `AuthEntry` record and its strict normalization** · high · M · **hand-written**
**upstream** — `mcp-auth.ts`: `StoredTokens` / `StoredClientInfo` / `AuthEntry`, and the parse family
`parseJsonPayload`, `parseAuthEntryPayload`, `toAuthEntry`, `toStoredTokens`, `toStoredClientInfo`,
`toRecord`, `optionalString/Number/Boolean`, `stringArray`.
**behavior** — A corrupted or half-written credential surfaces as an error naming the server and the
source (`Failed to parse OAuth credentials for <name> from <source>: invalid credential shape`),
never as "no credentials" — the latter silently restarts an OAuth flow the user already completed.
Unknown keys written by a newer version round-trip harmlessly (dropped, not fatal). A JSON array
payload is rejected the same as a scalar. `redirectUris` is the one field that degrades silently.
`configPreRegistered` must survive storage verbatim; section 07's stub check reads it.
**cyrup** — `serde` structs with `Option<T>` fields, `#[serde(rename_all = "camelCase")]`, serde's
default ignore-unknown (do **not** add `deny_unknown_fields`). Three of the five slots are rmcp
types: `credentials: Option<StoredCredentials>`, `state: Option<StoredAuthorizationState>` — both
`#[non_exhaustive]`, both `Serialize + Deserialize`, both constructed through their `new`
constructors. Only `StoredClientInfo` and `server_url` stay hand-written.
`redirectUris` needs a custom `deserialize_with` mapping a non-`Vec<String>` to `None` instead of
erroring. Serialization is `serde_json::to_string`, compact, per MCP-275.
**verify** — a table of malformed payloads (`{"tokens":{}}`, `{"tokens":{"accessToken":1}}`,
`{"codeVerifier":5}`, `{"clientInfo":{"clientSecret":true}}`, `[1,2]`) each producing the exact error
string; `{"credentials":{…},"futureKey":9}` round-tripping with the unknown key dropped;
`{"clientInfo":{"clientId":"c","redirectUris":["a",2]}}` yielding `redirect_uris: None` and a valid
entry.

**MCP-251 — Derive the keychain account and legacy directory from `sha256-<hex>` of the server name** · high · S · **hand-written**
**upstream** — `mcp-auth.ts` `getAuthEntryAccount`, `getServerDir`, `getAuthEntryFilePath`.
**behavior** — A configured server may be named anything the user typed: `../escape`,
`@scope/name`, non-ASCII, or the empty string. The derived legacy path must never leave the base dir
and the derived account must be stable across processes and platforms. Two different `oauthDir`s must
**not** produce different accounts.
**cyrup** — `sha2::Sha256` over `server_name.as_bytes()`, lowercase hex, prefixed `sha256-`. `sha2`
is not in the workspace dependency table; `cyrup-config` and `cyrup-ext-subagents` each declare it
per-crate, and `cyrup-mcp` does the same. `cyrup_ext_subagents::exec::mcp_direct_tools`'s
`compute_mcp_server_hash` is the in-tree formatting precedent (`format!("{byte:02x}")`).
**verify** — port the hostile-name test: for each of `["Cloudflare Workers", "сервер", "../escape",
"@scope/name", ""]`, assert the legacy path relative to the base dir matches
`^sha256-[a-f0-9]{64}/tokens\.json$`, is not absolute, and does not start with `..`.

**MCP-252 — Add the OS keyring backend and map its error taxonomy** · high · M · **extension-owned**
**upstream** — `mcp-auth.ts`: the `KeyringEntry` interface (`getPassword` / `setPassword` /
`deleteCredential`), `keyringAuthSecretStore`, `getKeyringEntry`.
**behavior** — Persistent MCP OAuth credentials live in the OS credential store. There is **no**
plaintext fallback: when the store cannot be reached, the operation fails with `OAuth secure
credential storage is unavailable. Configure the OS credential store and retry authentication.` and
authentication does not proceed.
**cyrup** — absent workspace-wide; `keyring` 4.1.6 is a new dependency and this is where it enters.
`keyring::Entry::new(service, account) -> Result<Entry>` (note: fallible, unlike 3.x and unlike
`@napi-rs/keyring`); `get_password` / `set_password` / `delete_credential`;
`keyring::Entry::store_status()` reports the one-time store-initialisation result and is where the
message above attaches. The internal seam is a small trait mirroring upstream's `AuthSecretStore`:
`fn read(&self, account:&str) -> Result<Option<String>, AuthStoreError>`, `fn write(…)`,
`fn remove(…)`. Three `keyring_core::Error` mappings are load-bearing: `NoEntry` → `Ok(None)`
(absence is an `Err` in Rust and mapping it wrong makes every fresh server look like a store
failure); `TooLong(name, limit)` → the Windows ceiling, a *typed* condition whose `Display` is
byte-identical to the string upstream's fault-injection store fabricates; `NoStorageAccess(err)` →
the variant whose `source()` chain the revoked-keyring predicate walks. Upstream's dynamic
native-binding fallback across 12 triples has no counterpart — backends are statically linked. Which
Linux backend gets linked is the open decision below, and it decides whether MCP-260…262/287 are live
code.
**verify** — unit against an injected in-memory store; a live round trip on macOS and Linux for real
persistence (and a second run of the same binary to prove it survives the process); a case asserting
the exact unavailability sentence when store construction fails.

**MCP-253 — The chunking manifest write path** · high · M · **hand-written**
**upstream** — `mcp-auth.ts`: the limits, `AuthEntryChunkManifest`, `isAuthEntryChunkManifest`,
`getAuthEntryChunkAccount`/`…Accounts`, `createChunkManifest`, `writeSecureAuthEntryToStore`.
**behavior** — A credential larger than 1000 units persists intact on every platform, including
Windows Credential Manager's 1280-character per-value ceiling. A crash mid-write never leaves the
base account pointing at chunks that do not exist. A shrink from chunked to single leaves exactly one
entry behind.
**cyrup** — `chunk_count = ceil(len/1000)`; `chunk_digest = hex(sha256(payload.as_bytes()))[..16]`;
chunk account `format!("{account}.chunk.{digest}.{i}")`; **write chunks first, manifest last**; on
error remove the *new* chunks and return `AuthStoreError::Write`; on digest change remove the
*previous* chunks best-effort. Manifest field order is `__piMcpAdapterOAuthChunked`, `chunkCount`,
`chunkDigest` (serde emits declaration order). **Forced mechanism divergence, with the cost now
zero:** upstream chunks on UTF-16 code units while hashing UTF-8 bytes; the port chunks on char
boundaries ≤ `i*1000` bytes. Because MCP-280 changes both the service name and the record shape, no
JS writer ever reads these accounts, so self-consistency is the entire contract.
**verify** — a 5000-character token round-trips; every stored value ≤1280; exactly one non-`.chunk.`
entry at the base account; port the four upstream chunking cases and the threshold regression
(threshold must be ≤ the value limit); add a non-ASCII payload case with no upstream twin and assert
self-consistency.

**MCP-254 — The chunked read path and the `AuthStoreError` taxonomy** · high · S · **hand-written**
**upstream** — `mcp-auth.ts`: `OAuthCredentialStoreError` (code `OAUTH_CREDENTIAL_STORE_UNAVAILABLE`,
`operation: 'read'|'write'|'remove'`), `readChunkedAuthEntry`, `readAuthEntryFromStore`,
`removeAuthEntryFromStore`.
**behavior** — A missing chunk is an *error*, never "no credentials": the upstream test pins that a
deleted chunk yields `inspectAuthForUrl(...).status === "unavailable"`. The error class must be
distinguishable by callers, because `mcp-auth-flow.ts`'s refresh driver rethrows it and swallows all
other refresh errors into `null`.
**cyrup** — `#[derive(thiserror::Error)] enum AuthStoreError { Unavailable { operation: StoreOp,
source }, Parse { server, source_label }, … }` carrying the operation discriminant and a `source`
chain — the chain is load-bearing for MCP-262's predicate. Messages byte-exact: `Failed to
{read,write,remove} OAuth credentials for {server} {from,to} the OS secure credential store`, and
`Missing OAuth credential chunk {chunkAccount} for {server}`. Do **not** conflate this with cyrup's
two existing `AuthError` enums (`cyrup_provider::error::AuthError`, which
`cyrup_provider::auth::store` uses, and `cyrup_config::error::AuthError`) or with rmcp's
`rmcp::transport::auth::AuthError`; `CredentialStore::load` must convert into rmcp's at the trait
boundary, and `AuthError::InternalError(String)` is the variant that carries a store failure through
without being mistaken for `AuthorizationRequired`.
**verify** — delete one chunk account and assert `inspect` → `unavailable`; assert the variant
survives a `?` chain up to the section-07 refresh driver; assert a store failure reaching rmcp
arrives as `InternalError`, not `AuthorizationRequired`.

**MCP-255 — Stale-chunk cleanup ordering and its error-swallowing** · medium · S · **hand-written**
**upstream** — `mcp-auth.ts` `readExistingChunkManifest` (swallows everything),
`tryRemoveChunkPayloads` (swallows everything), and the two cleanup call sites inside
`writeSecureAuthEntryToStore`.
**behavior** — Rewriting a large credential must not leave the previous generation's chunks in the
keychain — they contain a real token. A failure to clean up must not fail, or hide, a write that
succeeded. A rewrite with identical content must **not** delete its own chunks.
**cyrup** — compare `previous_manifest.map(|m| m.chunk_digest)` against `manifest.map(…)`, including
the both-`None` case (a no-op), and only then remove. All cleanup paths use `let _ = …`. The
failure-path cleanup targets the **new** manifest; the success-path cleanup targets the **previous**
one.
**verify** — port the two upstream stale-chunk cases; add a store whose `remove` always fails and
assert the write still returns `Ok`.

**MCP-256 — The legacy plaintext import-and-delete path** · high · M · **hand-written**
**upstream** — `mcp-auth.ts` module doc, `getAuthStorageOptions`, `getAuthBaseDir`,
`getAuthEntryFilePath`, `readLegacyAuthEntry`, `removeLegacyAuthEntry` and their call sites in
`readAuthEntryFromStore` / `saveAuthEntry` / `removeAuthEntry`; `config.ts`
`resolveConfiguredOAuthDir`; `agent-dir.ts` `getAgentPath`.
**behavior** — A `tokens.json` written by an older adapter version is read once, written into the
keychain, and then **deleted along with its directory**. A plaintext credential must not survive a
single successful keychain read. Deletion asymmetry is mandatory: file-removal failure is fatal
(`Failed to remove legacy plaintext OAuth credentials for {server} at {path}`), directory-removal
failure is swallowed. Base-dir precedence is `MCP_OAUTH_DIR` (trimmed, non-empty) →
`settings.oauthDir` resolved against `cwd` → `<agentDir>/mcp-oauth`.
**cyrup** — `std::fs` + `std::path`; base dir from `cyrup_config::env`'s `ConfigDirs.agent_dir`
(`CYRUP_AGENT_DIR` → `PI_CODING_AGENT_DIR` → `<home>/.cyrup/agent`, with `~` expansion), keeping
`MCP_OAUTH_DIR` as the explicit unprefixed escape hatch so a user can point at a real pi install.
**The importer is also the record translator**, and this is the one place absolute fractional
`expiresAt` survives: `{accessToken, refreshToken, expiresAt, scope, issuer}` +
`clientInfo.clientId` → `StoredCredentials { client_id, token_response, granted_scopes:
scope.split(' '), token_received_at: now, issuer }` with `expires_in = max(0, floor(expiresAt -
now))`. **Named behaviour delta:** `StoredCredentials.client_id` is required and non-`Option`, so a
legacy entry carrying `tokens` but **no** `clientInfo.clientId` cannot be translated. Import its
`clientInfo`/`serverUrl` if present, drop the tokens, delete the file, and let the next call
re-authenticate — never fabricate a client id.
**verify** — port the upstream legacy cases: configured dir is the import source, the file is gone
after import, a second read still works from the keychain; env beats settings; inspection does
**not** migrate. Port the agent-dir path cases (default `<agentDir>/mcp-oauth`, tilde expansion,
`MCP_OAUTH_DIR` override) against cyrup's dir chain. Add the no-`clientId` case and assert the
tokens are dropped rather than a synthetic client id being written.

**MCP-257 — The process-lifetime auth-entry cache and its three external invalidation points** · high · M · **hand-written**
**upstream** — `mcp-auth.ts` `authEntryCache`, `cloneAuthEntry`, `isAuthEntryCacheEnabled`,
`publishAuthEntryToCache`, `readAuthEntry`, `invalidateAuthEntryCache`, and the eviction inside
`removeAuthEntry`; callers in `server-manager.ts` (two sites) and `session-recovery.ts` (one).
**behavior** — Repeated credential reads within a process hit the keychain once — a real cost on
macOS, where each access can prompt. Absence is cached as well as presence. Store *failures* are
never cached. A 401 from an OAuth-capable server invalidates the cached entry so the next read sees
whatever the auth flow wrote. Status-only reads never populate or consume the cache.
`removeAuthEntry` evicts **even when the cache is disabled**. A returned entry is fully isolated
from the cached one at nested-field granularity, in both directions.
**cyrup** — `HashMap<String, Option<AuthEntry>>` behind an `RwLock`, keyed by server name only;
`contains_key` distinguishes "cached absent" from "not cached"; values are returned **by value**
(`.clone()`) on both get and set — never `&AuthEntry`, `Arc<AuthEntry>` or a `Cow`, or the isolation
invariant breaks. The enable flag is read **per call** (upstream reads `process.env` each time, and
the suite's write-behind-the-cache helper depends on exactly that). Publish-on-write re-serializes
and re-parses so the cached shape equals a fresh read's; if that parse fails, **remove** the key.
`cyrup_config::auth::AuthStore`'s `cached: RwLock<…>` snapshot, refreshed after each `modify`, is the
in-tree shape precedent — the *"preserve the last valid snapshot"* discipline is the same idea.
**verify** — MCP-283 is the acceptance gate. Minimum: two reads ⇒ one backend read; `inspect` ⇒ cache
untouched; `invalidate` ⇒ next read hits the backend; a write ⇒ next read serves the written value
with zero backend reads. The invalidation *policy* (at most once per connect attempt, never on the
initial implicit challenge, never for non-OAuth servers, single-flight under concurrency) belongs to
the server-manager section; this item owns only the eviction primitive it drives.

**MCP-258 — Fault-injection backends behind an explicit selector** · medium · S · **extension-owned**
**upstream** — `mcp-auth.ts`: the five stores, `getAuthSecretStore`, `resetTestAuthSecretStore`,
`resetAuthEntryCache`, `getTestAuthSecretStoreReadCount`, `getTestAuthSecretStoreEntries`,
`removeTestAuthSecretStoreEntry`.
**behavior** — The entire storage layer is testable without an OS keychain, including the Windows
size ceiling, a totally unavailable store, and a revoked Linux keyring. **All four** test stores
increment the read counter on `read`, including the two that throw.
`resetTestAuthSecretStore` clears map + cache + counter; `resetAuthEntryCache` clears the cache
**only** and leaves the counter.
**cyrup** — this is the item the real crate shrinks most. `keyring_core::mock::Store` is ungated and
exposes `set_error(Error)`, so the four hand-rolled backends become one mock plus an injected error:
`memory` = bare mock; `sizelimited` = `TooLong("password encoded as UTF-16", 2560)`; `unavailable` =
`PlatformFailure`; `keyrevoked` = `NoStorageAccess(<KeyRevoked>)`. Only the read counter and the
entry-inspection hooks stay hand-written. Constructor injection
(`McpAuthStore::with_backend(Arc<dyn AuthSecretStore>)`) is the primary seam; the env selector exists
for the one end-to-end test that crosses a process boundary (MCP-260's recovery test genuinely needs
it). `cyrup_ext::caps::proc::ProcCaps`'s `with_kill_grace`/`with_write_stdin_timeout` are the in-tree
test-override precedent.
**verify** — each backend selectable and observable; the counter increments for all four injected
backends and not for the real keyring backend; `reset_test_store` vs `reset_cache` differ exactly as
upstream pins.

**MCP-259 — Honour the auth-cache disable switch** · low · S · **hand-written**
**upstream** — `mcp-auth.ts` `isAuthEntryCacheEnabled` and its two consumers.
**behavior** — An operator debugging a credential problem can force every read to hit the OS store.
With the gate off, two reads cost two backend reads, writes do not publish, and
`invalidateAuthEntryCache` is a harmless no-op — but `removeAuthEntry` still evicts.
**cyrup** — read the env per call and compare to the literal `"1"`; any other value leaves the cache
on. Dual-read `CYRUP_MCP_DISABLE_AUTH_CACHE` then `PI_MCP_ADAPTER_DISABLE_AUTH_CACHE`. The eviction
in `remove_auth_entry` is **not** behind the flag.
**verify** — with `"1"`, two reads ⇒ two backend reads; with `"true"` or `"0"`, one; a remove
performed while disabled still evicts a previously-cached entry.

**MCP-260 — Re-exec under `keyctl session -` via a hidden `__mcp-keyring-helper` subcommand** · high · M · **hand-written**
**upstream** — `mcp-auth.ts` `runLinuxKeyringRecoveryOperation`,
`linuxKeyringRecoveryAuthSecretStore` and its three retry sites; `mcp-keyring-helper.cjs`.
**behavior** — On Linux, when the process's kernel session keyring has been revoked, credential
read/write/remove still work — once, per operation — by performing the keyring call inside a freshly
created session keyring. Without it a user in a `su` / detached-tmux / systemd session cannot
authenticate to any MCP server and gets a message they cannot act on. That an in-process library
cannot recover is not a JavaScript artefact; it is the reason the hop exists, and it applies to
`keyring` identically.
**cyrup** — keep `keyctl session -` **literally**; replace `node <helper.cjs>` with
`<std::env::current_exe()> __mcp-keyring-helper`. argv is exactly `["session", "-", <program>,
<args…>]` — the upstream fake `keyctl` asserts `$1 == "session" && $2 == "-"`, exits 64 otherwise,
then `shift 2; exec "$@"`, so any extra flag breaks it. Use `std::process::Command` (blocking, the
`spawnSync` analogue) with piped stdin/stdout, a 10 s wall-clock kill and a 1 MiB output cap. The
hidden-subcommand mechanism already exists and is idiomatic: `crates/cyrup/src/intercom_broker_cmd.rs`
and `crates/cyrup/src/subagent_runner_cmd.rs` each expose a `SUBCOMMAND` const, an `is_selected(argv)`
matching `argv[1]`, and a `dispatch`, all pre-dispatched from `crates/cyrup/src/main.rs` before any
clap parsing; `cyrup_intercom::transport::spawn` is the `current_exe()` precedent. Add
`crates/cyrup/src/mcp_keyring_helper_cmd.rs` and its `pub mod` in `crates/cyrup/src/lib.rs`.
`PI_MCP_ADAPTER_KEYRING_RECOVERY_NODE` is dropped — there is no interpreter; `…_KEYCTL` and
`…_HELPER` survive, the latter now naming a program rather than a script.
**verify** — an integration fixture `keyctl` shell script asserting the exact argv and `exec`ing the
rest, plus a fake store; a `KeyRevoked` backend write-then-read-then-clear round-tripping through the
subprocess. Port the upstream recovery test including its `PI_MCP_ADAPTER_FAKE_KEYRING_STORE` fixture
variable. `crates/cyrup-it` sets `autotests = false` and gates every `[[test]]` behind
`required-features = ["it"]`, so a new MCP target must be declared by hand.

**MCP-261 — The helper's one-shot JSON stdio protocol** · medium · S · **hand-written**
**upstream** — `mcp-keyring-helper.cjs` (stdin read with 1 MiB cap, validation, single op, single
response line, `process.exitCode = 1` on error); request construction and the six-rung response
ladder in `mcp-auth.ts` `runLinuxKeyringRecoveryOperation`.
**behavior** — The helper performs exactly one keyring operation and exits. It never reads config,
never touches the network, never logs a secret, and its stdout is exactly one line of JSON.
**cyrup** — request
`{"operation":"read"|"write"|"remove","service":<string>,"account":<string>[,"payload":<string>]}`
followed by `\n`. **`payload` is omitted entirely for `read`/`remove`** — `JSON.stringify` drops
`undefined` — so use `#[serde(skip_serializing_if = "Option::is_none")]`, not a serialized `null`.
Validation order and messages: `request too large` (>1 MiB), `invalid request`, `invalid operation`,
`invalid service`, `invalid account`, `invalid payload`. Responses: read ⇒ `{"ok":true,"found":false}`
or `{"ok":true,"found":true,"value":"…"}`; write/remove ⇒ `{"ok":true}`; error ⇒
`{"ok":false,"error":"<message>"}` **and exit code 1**. Parent-side validation is the six-rung ladder
in §6.7 with its six exact messages. `service` travels the wire even though it is a constant — keep
it, and keep the helper validating it, so the helper stays a general one-shot keyring tool rather
than a hard-coded one. The helper's own body is a single `keyring::Entry` call, so it is the one code
path in the crate that must not initialise the cache, the config, or tracing.
**verify** — each malformed request ⇒ its exact message and exit 1; each well-formed request ⇒ its
exact response shape; a `read`/`remove` request serializes with **no** `payload` key; the parent-side
ladder one case per message (see MCP-287 for which rungs are reachable).

**MCP-262 — The revoked-keyring cause-chain predicate** · medium · S · **hand-written**
**upstream** — `mcp-auth.ts` `causeChainContains` (cycle-guarded, tests `name`/`message`/`code`),
`isLinuxKeyringRecoveryEnabled`, `shouldAttemptLinuxKeyringRecovery`.
**behavior** — Recovery fires **only** for a revoked keyring and **never** for a generic store
failure. The negative test sets a `keyctl` that exits 99 and asserts the fake store file is never
created — a wrong predicate spawns a subprocess on every keychain hiccup.
**cyrup** — walk `std::error::Error::source()` transitively with a depth or pointer guard, matching
`Display` against `regex::Regex::new(r"(?i)key\s*(?:has been\s*)?revoked|keyrevoked")`. `regex` is
already reachable in the workspace (`cyrup-permission-system` declares it). Enablement: disabled by
`…_DISABLE_KEYRING_RECOVERY == "1"`; enabled when `cfg!(target_os = "linux")` **or** the test-forcing
variable is `"1"`. The `Display`-matching risk the previous edition flagged is now **partly
resolved**: `keyring_core::Error::NoStorageAccess(err)` renders as `Couldn't access platform storage:
{err}`, byte-identical to the string upstream fabricates, and it is one of only three variants that
return a `source()`, so the walk is short and well-defined. What remains unvalidated is the exact
`Display` of `linux-keyutils-keyring-store`'s inner platform error on a genuinely revoked box.
**verify** — a 3-deep source chain whose innermost `Display` is `KeyRevoked` ⇒ true; the same chain
with `permission denied` ⇒ false; a self-referential chain terminates; the negative fixture (generic
failure ⇒ no subprocess) ported verbatim.

**MCP-263 — Emit the two credential-store-unavailable messages verbatim** · low · S · **hand-written**
**upstream** — `mcp-auth.ts` `formatOAuthCredentialStoreUnavailable`.
**behavior** — When the store is unreachable the user gets an actionable sentence, and on Linux
specifically the *revoked-keyring* sentence, which names the only fix.
**cyrup** — two literals. Linux + predicate match ⇒ `OAuth credential store unavailable: the Linux
session keyring may be revoked. Start Pi from a fresh login/keyring session and retry.` Otherwise ⇒
`OAuth credential store unavailable. Configure or unlock the OS credential store and retry.` The word
**"Pi" is host branding** and becomes the cyrup app name; that is the one intentional edit and it
should be the only one. The third string, `OAuth secure credential storage is unavailable. Configure
the OS credential store and retry authentication.`, belongs to MCP-252.
**verify** — port the upstream diagnostics test: a `KeyRevoked` cause chain yields a message
containing `session keyring may be revoked` and `fresh login/keyring session` on Linux, and the
generic sentence elsewhere.

**MCP-264 — URL binding and the mutators' sibling-purge rule** · critical · M · **hand-written**
**upstream** — `mcp-auth.ts` `getAuthForUrl`, `saveAuthEntry`, the four mutators (`updateTokens`,
`updateClientInfo`, `updateCodeVerifier`, `updateOAuthState`) and the four clearers.
**behavior** — Credentials are bound to the exact `serverUrl` string they were issued for. An entry
with no stored URL is invalid (it predates the binding). When a mutator is given a URL differing from
the stored one, **every other artifact in the entry is deleted** — a stale `clientInfo` or PKCE
verifier from a previous authorization server must never be paired with a new one. Each clearer
passes `serverUrl: undefined`, so a clear never rewrites the stored URL.
**Severity rationale, on the four clauses:** getting this wrong presents a credential minted for one
authorization server to a different one, and re-uses a PKCE verifier across authorization contexts —
a permission bypass and a credential disclosure, silently.
**cyrup** — plain struct manipulation. `save_auth_entry` takes `&mut AuthEntry` to reproduce the
caller-visible mutation. Comparison is exact string equality — **no** URL normalization, no
trailing-slash tolerance; adding any would silently widen credential reuse. Each mutator reads via
the *unvalidated* `get_auth_entry` (not `get_auth_for_url`), purges the three siblings when a URL is
supplied and differs, sets its one field, saves. Note this is orthogonal to rmcp's own defence:
`AuthorizationManager::initialize_from_store` fences on the **issuer** changing (clearing the store,
or keeping a portable CIMD client id and discarding tokens); the adapter fences on the **MCP server
URL** changing, which rmcp cannot see. Both are required.
**verify** — for each of the four mutators, a URL change wipes exactly the other three slots;
`get_auth_for_url` returns `None` for an entry without `server_url` and for a trailing-slash
mismatch; each clearer leaves `server_url` intact; a URL change followed by a fresh authorization
leaves no residue of the previous PKCE state.

**MCP-265 — `inspectAuthForUrl`'s three-state status and its fail-open/fail-closed split** · high · S · **hand-written**
**upstream** — `mcp-auth.ts` `OAuthCredentialStatus` and `inspectAuthForUrl`; consumers `commands.ts`
(`/mcp` status) and `oauth.ts`.
**behavior** — Status UI distinguishes "no credentials" from "the store is broken"; authentication
paths do not — they use `getAuthForUrl` and stay fail-closed. A broken keychain degrades `/mcp`
status output; it never silently grants access and never silently restarts auth. Inspection is
non-destructive **only** when no keychain entry exists — when one does, §6.8 step 2b still deletes
the legacy file.
**cyrup** — `enum OAuthCredentialStatus { Present(AuthEntry), Absent, Unavailable { message } }`;
read with `migrate_legacy = false` (no cache read, no cache write, no migration write). `Absent`
covers no-entry, no-`server_url` and URL-mismatch. Only `AuthStoreError::Unavailable` maps to
`Unavailable`; every other error propagates — including a parse failure on the stored payload, which
is deliberately *not* wrapped (MCP-284).
**verify** — port the two upstream cases (inspection does not migrate; a deleted chunk ⇒
`unavailable`, not `absent`); add a case where a bare parse error on the *store* payload propagates
instead of becoming `Unavailable`; add a case pinning that an inspect on a server that *does* have a
keychain entry still deletes a stray legacy file.

**MCP-266 — The accessor surface section 07 consumes** · medium · S · **hand-written**
**upstream** — `mcp-auth.ts` exports 34 symbols: 6 types/classes, 1 formatter
(`formatOAuthCredentialStoreUnavailable`), 5 test hooks, 1 dead loader hook
(`loadTestKeyringEntryClass`, excluded), and 21 production accessors/mutators (`getAuthStorageOptions`,
`getAuthBaseDir`, `getAuthEntryFilePath`, `getAuthEntry`, `getAuthForUrl`, `inspectAuthForUrl`,
`saveAuthEntry`, `removeAuthEntry`, `invalidateAuthEntryCache`, the four `update*`, the four
`clear*`, `getOAuthState`, `isTokenExpired`, `hasStoredTokens`, `clearAllCredentials`).
**behavior** — Section 07's OAuth provider and auth-flow driver depend on that production set plus
the store error type and the formatter.
**cyrup** — one `pub struct McpAuthStore` owning the backend, cache and options, with inherent
methods — **not** free functions over process-global state, which is what upstream's module-level
`authEntryCache`/`memoryAuthEntries` amount to. **The surface shrinks substantially**: `isTokenExpired`
and the SDK token conversion dissolve into rmcp (MCP-267, MCP-271), and `updateCodeVerifier` /
`clearCodeVerifier` / `updateOAuthState` / `getOAuthState` / `clearOAuthState` are subsumed by
`StateStore::{save, load, delete}` (MCP-291). What must stay reachable regardless:
`get_auth_base_dir`, because `mcp-auth-flow.ts` folds it into two in-flight dedup keys; and
`get_auth_entry_file_path`, because two upstream test suites assert on it.
**verify** — one test per surviving production accessor. Note the mutator and clearer edge cases have
**no** dedicated upstream test and must be written fresh from the source semantics in §6.9.

**MCP-267 — Expiry arithmetic** · medium · S · **rmcp**
**upstream** — `mcp-auth.ts` `isTokenExpired`; write site `mcp-oauth-provider.ts`
(`expiresAt: Date.now() / 1000 + tokens.expires_in`, preserving expiry even when `expires_in === 0`);
read sites in `oauth-handler.ts` and `mcp-oauth-provider.ts`.
**behavior** — Upstream's `isTokenExpired` is tri-state: `null` (no tokens), `false` (tokens with no
usable expiry — including `expiresAt === 0`, which means "no expiry", not "expired at the epoch"),
`true` (expired).
**cyrup** — **dissolved into rmcp for the live path.** `AuthorizationManager::get_access_token`
computes `remaining = expires_in - (now - token_received_at)` over `StoredCredentials`'s integral
`token_received_at: Option<u64>` and refreshes when `remaining < REFRESH_BUFFER_SECS = 30`, falling
back to returning the token unchanged when either half of the expiry information is absent — which is
`isTokenExpired`'s `false` branch, reached by a different route. Fractional absolute seconds survive
**only** at the legacy-import boundary (MCP-256), where the correct semantic is
`oauth-handler.ts`'s (`expiresAt = 0` ⇒ already expired), not `isTokenExpired`'s.
**cyrup unit trap** — `cyrup_provider::auth::types::Credential::Oauth.expires` is `i64`
**milliseconds**. MCP's `expiresAt` is fractional **seconds** and rmcp's `token_received_at` is
integral seconds. Never assign between them; nothing in a unit test that constructs both sides
consistently would catch it. Assert against a fixture.
**verify** — the import converter against a table `expiresAt ∈ {absent, 0.0, now-1.0, now+60.0}`,
asserting `expires_in ∈ {absent, 0, 0, 60}`; and rmcp's own buffer behaviour exercised through
`get_access_token` with a `token_received_at` inside and outside the 30 s window.

**MCP-268 — Serialize read-modify-write per server** · high · M · **hand-written**
**upstream** — `mcp-auth.ts`'s mutators are synchronous read-mutate-write with no lock of any kind;
within one process the JS event loop makes the sequence atomic, and across processes upstream accepts
last-writer-wins.
**behavior** — Two concurrent refreshes of the same server must not lose a rotated refresh token.
Upstream gets the single-process half for free and does not attempt the cross-process half.
**cyrup** — a per-server-name `tokio::sync::Mutex` inside `McpAuthStore`, held across the whole
read-mutate-write, restoring exactly the guarantee the language previously supplied and claiming
nothing upstream did not have. Deliberately **not** a cross-process lock: the store is a keychain,
there is no file to lock, and inventing a lock file would add an on-disk artifact upstream has no
counterpart for. `cyrup_provider::auth::store::CredentialStore::modify` — *"THE ONLY write path.
Serialized read-modify-write per provider id"* — is the shape to copy, and `cyrup_config::auth`'s
per-provider `tokio::sync::Mutex` is the implementation to copy; do **not** copy its `FileLock` half.
This matters more in Rust than upstream because rmcp calls `CredentialStore::save` from whichever
task performed the refresh.
**verify** — N concurrent `update_tokens` on one server name; the final entry is one of the writes
intact, never a merge, and no chunk set is orphaned.

**MCP-269 — MCP credentials never reach `auth.json`** · medium · S · **hand-written**
**upstream** — `mcp-auth.ts` module doc: persistent entries live in the OS credential store; the
plaintext file is deleted after import.
**behavior** — MCP OAuth tokens are never written in plaintext.
**cyrup** — decided: `cyrup-mcp` keeps its own keychain-backed store and does not touch
`cyrup_config::auth::AuthStore`. The alternatives fail on inspection rather than on preference:
folding MCP credentials into `auth.json` under synthetic provider ids is a plaintext downgrade *and*
a schema mismatch (`Credential` is `ProviderId`-keyed with no `serverUrl`, no PKCE slot, no
`configPreRegistered`, and a millisecond `expires`); converting the whole workspace to a keychain is
a `cyrup-config`-wide change that would touch every provider flow and is not this port's business.
The consequence someone must own is that cyrup then has two credential stores with different
postures, and the user-facing answer to "where are my secrets" differs by subsystem.
**verify** — a repo-level test asserting no MCP credential material ever reaches
`cyrup_config::env`'s auth path, and no `Serialize` derive that could route an `AuthEntry` there.

**MCP-270 — The embedder façade** · low · S · **extension-owned**
**upstream** — `oauth.ts`: `getMcpOAuthTokensForUrl` (delegates to `getValidToken`, `?? undefined`),
`inspectMcpOAuthTokensForUrl`, `updateMcpOAuthTokensForUrl`, and the four type aliases.
**behavior** — A caller outside the adapter can read, inspect and write MCP OAuth tokens without
importing the storage internals. The one piece of real logic: a `present` status whose entry has no
`tokens` collapses to `absent`, so an entry holding only a `clientInfo` or a PKCE verifier is not
reported as authenticated. `McpOAuthTokenStatus` is a **different** three-state enum from
`OAuthCredentialStatus` — its `present` variant carries tokens, not the whole entry.
**cyrup** — a `pub` module on `cyrup-mcp`, thin re-exports plus that one mapping. This is seam-map
row OA-13 and it needs no host involvement. `McpOAuthTokenOptions`'s `signal` becomes a
`tokio_util::sync::CancellationToken`; `skipIssuerMetadataValidation` maps onto
`AuthorizationManager::set_allow_missing_issuer`.
**verify** — an entry with `client` but no `credentials` inspects as `absent`; an `unavailable` status
passes through unchanged.

**MCP-271 — The MCP-SDK `OAuthTokens` conversion** · n/a · S · **rmcp**
**upstream** — `oauth-handler.ts` `getStoredTokens`: reads `getAuthEntry(serverName)?.tokens`, drops
tokens whose `expiresAt` is defined and past, then builds `{access_token, token_type: "Bearer",
refresh_token, expires_in: max(0, floor(expiresAt - now)), scope, …(issuer !== undefined ? {issuer} :
{})}`. It takes **no** storage options, so it always resolves the default legacy base dir.
**behavior** — the adapter hands the TS SDK its own token shape.
**cyrup** — **dissolved.** rmcp's `StoredCredentials.token_response` is
`oauth2::StandardTokenResponse<VendorExtraTokenFields, BasicTokenType>` — the SDK shape *is* the
stored shape, so there is no conversion, no separate expiry predicate at this boundary, and no
`Bearer` literal to write. What is left of this file's behaviour lives in two other items: the expiry
semantics in MCP-267 and the legacy-file translation in MCP-256, whose arithmetic is exactly
`getStoredTokens`'s.
**verify** — n/a; MCP-256's converter table covers the residue.

**MCP-272 — `ConsentManager`** · n/a · S · **cut**
**upstream** — `consent-manager.ts`; constructed once in production as
`new ConsentManager("once-per-server")` in `init.ts`; state slot in `state.ts`; consumed only by
`ui-server.ts` and `ui-session.ts`.
**cyrup** — cut with Cut 2. Recorded in *Out of scope* with the reason and with the seam against the
surviving model-facing approval gate, so a later pass does not re-file it. Behaviour preserved here
for the record, because it is not written down anywhere else: `requiresPrompt` returns `false` in
`never`, **`true` for a denied server** (re-prompt), `true` in `always`, else `!approved.has(name)`;
`shouldCacheConsent` is `mode !== "always"`, hence `true` in `never` too; `registerDecision` removes
from **both** sets before inserting into one; `ensureApproved` returns immediately in `never`, throws
`ConsentError{denied}` for a denied server, throws `ConsentError{requiresApproval}` when not
approved, and in `always` mode **consumes** the approval (one-shot); `clear(name?)` drops one or all.
The deliberate asymmetry — a denied server is re-askable by `requiresPrompt` but not callable by
`ensureApproved` — is the subtlety that would have been lost.
**verify** — unit: `cyrup-mcp` exposes no consent-mode surface — no `never`/`always`/`once-per-server`
selector and no `requires_prompt`/`ensure_approved`/`register_decision` trio — so the cut cannot be
half-reintroduced beside the surviving model-facing approval gate, which is section 05's unit and
carries its own tests.

**MCP-273 — `ConsentError`** · n/a · S · **cut**
**upstream** — `errors.ts`, extending `McpUiError`. Messages byte-exact: denied ⇒ ``Tool calls for
"${server}" were denied for this session``, code `CONSENT_DENIED`, hint `"The user denied tool
access. Start a new session to try again."`; required ⇒ ``Tool call approval required for
"${server}"``, code `CONSENT_REQUIRED`, hint `"Prompt the user for consent before calling tools."`;
both carry `context: { server }` and set `.denied = options.denied ?? false`.
**cyrup** — cut with MCP-272. The `McpUiError` umbrella it lives under is itself Cut 2.
**verify** — unit: a table test pins both triples — message, `CONSENT_DENIED` / `CONSENT_REQUIRED`,
and hint text — wherever the surviving taxonomy lands (section 02's error-taxonomy unit owns that
decision), so a later pass that revives a consent error cannot silently drift the codes or the
strings; and no cut UI path in `cyrup-mcp` constructs one.

**MCP-274 — Consent state is process-scoped and must not be persisted** · n/a · S · **cut**
**upstream** — two in-memory `Set`s with no I/O anywhere in the file, constructed per
`initializeMcp`.
**cyrup** — cut with MCP-272. The negative requirement it encoded — "do not make consent sticky" —
transfers to the *surviving* approval half in the approval section, where
`cyrup_permission_system::stores::SessionApprovalStore` is the lifecycle precedent: in-memory,
session-scoped, cleared at both session start and session shutdown.
**verify** — cyrup-it: approve an MCP tool call, then drive `session_shutdown` and a fresh
`session_start`, and assert the approval is gone at **both** edges (not just the second); plus a
filesystem assertion that the agent dir gained no file naming the server and that `auth.json` is
byte-unchanged, so approval state cannot leak into the one file that does persist.

**MCP-275 — Compact JSON serialization** · medium · S · **hand-written**
**upstream** — `mcp-auth.ts` `writeSecureAuthEntryToStore`, with the comment *"Compact: multiline
secrets corrupt gnome-keyring plaintext (GKeyFile) collections."*
**behavior** — A stored secret contains no newline. On gnome-keyring's plaintext (GKeyFile)
collection backend a multi-line value corrupts the collection file, losing every credential in it —
not just this one.
**cyrup** — `serde_json::to_string`. Add a debug assertion that the payload contains no `\n` before
any `write`. Note the contrasting in-tree convention: `cyrup_config::auth::AuthStore` deliberately
uses `to_string_pretty` for `auth.json`. Copying that habit here is the failure mode.
**verify** — every value handed to the backend satisfies `!payload.contains('\n')`, including each
chunk and the manifest.

**MCP-276 — The non-string server-name guards do not port** · n/a · S · **extension-owned**
**upstream** — `mcp-auth.ts` `getServerDir` and `getAuthEntryAccount` both open with
``if (typeof serverName !== 'string') throw new Error(`Invalid MCP server name:
${JSON.stringify(serverName)}`)``, pinned by a test that calls `getAuthEntryFilePath(undefined as
unknown as string)`.
**behavior** — in TypeScript the guard defends the storage boundary against an untyped caller,
because a non-string reaching `join()` would produce a path outside the base dir.
**cyrup** — the parameter is `&str`; the value's only origin is a `serde_json` object **key** from
`mcp.json`, which is always a string; there is no `unknown`-typed caller to defend against. The
*hazard* the guard exists for — an arbitrary name escaping the base dir — is fully discharged by
MCP-251's SHA-256 derivation, which the same test's other half pins. Filed rather than dropped so a
later reader does not read the missing guard as an oversight.
**verify** — n/a; MCP-251's path-safety test covers the residual risk.

**MCP-277 — Prove the absence of secret leakage through `Debug`, logs and errors** · critical · S · **hand-written**
**upstream** — no error message in `mcp-auth.ts` interpolates a token, a payload or a chunk body;
only server names, account names and file paths appear. `consent-manager.ts` logs only the server
name.
**behavior** — A credential never reaches a log, a panic message, a `Debug` render or a UI string.
**Severity rationale, on the four clauses:** a credential printed into a transcript, a log file or a
crash report grants whoever reads it the access the credential encodes — a permission bypass with a
blast radius bounded only by log retention.
**cyrup** — hand-written `Debug` on `AuthEntry` and `StoredClientInfo` that redacts every
secret-bearing field; no `#[derive(Debug)]`. Error variants carry `server`, `account` and
`source_label` only. **rmcp already does the right thing for its half**: `StoredCredentials`'s
`Debug` renders `token_response` as `[REDACTED]`, and `StoredAuthorizationState`'s renders both
`pkce_verifier` and `csrf_token` as `[REDACTED]` — so the composed `AuthEntry` inherits redaction for
three of its four slots and only `StoredClientInfo.clientSecret` needs new work. The pattern **not**
to copy is in tree: `cyrup_provider::auth::types::Credential` derives `Debug` over its `refresh` and
`access` fields, so a `{:?}` prints tokens. That is a pre-existing hazard in another crate, out of
this section's scope, and exactly the shape to avoid. Deliberately **not** proposing `zeroize`:
upstream does not zeroize, the OS store owns the durable copy, and adding it would be an unrequested
mechanism change — flag it as possible hardening, not as a port unit.
**verify** — `format!("{:?}", entry)` on a fully-populated entry contains none of the secret
substrings; a grep-based test asserting no payload/token interpolation in the crate's error strings.

**MCP-278 — The storage acceptance suite** · medium · M · **hand-written**
**upstream** — `__tests__/mcp-auth-storage.test.ts`: **17** tests — 1 diagnostics plus 16 storage —
covering path safety for hostile names, the non-string guard, legacy import, `oauthDir` vs
`MCP_OAUTH_DIR` precedence, four chunking cases, three stale-chunk-cleanup cases, the
recovery-helper spawn, and the *negative* case (a generic failure must not invoke recovery).
**behavior** — this file is half the contract (MCP-283 is the other half). Every behaviour in
§6.2–§6.8 that is not obvious from the implementation is pinned in one of the two, including three
regressions the comments name explicitly.
**cyrup** — Rust `#[test]` for the in-process cases; `crates/cyrup-it` for the two subprocess cases,
which need a fixture `keyctl` script and a real spawn. Keep the `keyctl` argv assertion
(`$1 == "session"`, `$2 == "-"`, exit 64 otherwise, `shift 2`, `exec "$@"`) — it is the only thing
that pins MCP-260's argv shape. `cyrup-it` sets `autotests = false`, so declare the `[[test]]` target
explicitly with `required-features = ["it"]`.
**verify** — the suite itself; a missing port of any of the 17 is an incomplete item.

**MCP-283 — The cache acceptance suite** · medium · M · **hand-written**
**upstream** — `__tests__/mcp-auth-cache.test.ts`: 13 tests across three describes (`foundation`,
`coherence`, `invalidation`).
**behavior** — nine cache invariants that exist nowhere else in the source and would otherwise be
re-derived by guesswork: (1) the suite runs cache-**disabled** by default and opts in per test, with
the disable state restored between tests; (2) read-counter semantics for `memory`/`sizelimited`, and
`resetAuthEntryCache` **not** resetting the counter while `resetTestAuthSecretStore` does; (3)
`unavailable` and `keyrevoked` also bump the counter on a *throwing* read, observed with recovery
disabled; (4) present **and** absent reads are cached — two reads ⇒ one backend read for both; (5) a
write publishes, `updateTokens` refreshes the published value, `removeAuthEntry` evicts; (6) clone
isolation at **nested-field** granularity in both directions, plus two `inspectAuthForUrl` calls
costing two backend reads; (7) inspection stays uncached even after an ordinary read warmed the
cache; (8) store failures are not cached, and a chunked entry is reconstructed once then served with
zero further backend reads; (9) publication normalizes exactly as a store reload does, dropping
unknown keys identically on the hit and miss paths; plus invalidation reloading external rotations,
absent→present transitions and chunked entries, evicting only its target, being harmless while
disabled, and `removeAuthEntry` evicting **even with the gate off**.
**cyrup** — Rust `#[test]` mirroring each case, with an env-guard helper equivalent to the suite's
`useAuthCacheHarness` and a write-behind-the-cache helper that relies on the enable flag being read
per call.
**verify** — the suite itself; a missing port of any of the 13 is an incomplete item.

**MCP-284 — The parse-error wrapping asymmetry between read and remove** · medium · S · **hand-written**
**upstream** — `mcp-auth.ts`: on the **read** path only `store.read()` is inside the `try` that
produces `OAuthCredentialStoreError('read')`; the subsequent manifest parse and
`parseAuthEntryPayload` run *outside* it, so a corrupt base payload throws a **bare** `Error` which
`inspectAuthForUrl` **rethrows** rather than degrading to `unavailable`. On the **remove** path the
same parse *is* inside the try, so identical corruption becomes
`OAuthCredentialStoreError('remove')`.
**behavior** — wrapping uniformly changes `/mcp` status output for a corrupt entry from
"propagate" to "unavailable", and changes the section-07 refresh driver (which rethrows only the
store error class and swallows everything else) from "propagate" to "silent `null`".
**cyrup** — two distinct error variants and two distinct `?`-scopes. `read_auth_entry_from_store`
wraps **only** the backend `read`; the parse returns `AuthStoreError::Parse` un-promoted.
`remove_auth_entry_from_store` wraps its whole body, promoting `Parse` into
`Unavailable { operation: Remove }`.
**verify** — seed the base account with `"{not json"` and assert `inspect` **propagates**, not
`Unavailable`; seed the same and call `remove`, asserting `Unavailable { operation: Remove }`.

**MCP-285 — Remove-path chunk cleanup is fatal, not best-effort** · medium · S · **hand-written**
**upstream** — `mcp-auth.ts` `removeChunkPayloads` (non-swallowing, used by
`removeAuthEntryFromStore`) versus `tryRemoveChunkPayloads` (swallowing, used by the write path).
**behavior** — a single failing chunk delete during `removeAuthEntry` aborts before the base account
is removed, so the base keeps its manifest while some chunks are gone, leaving a credential that
reads as `unavailable` forever and cannot be cleared by retrying if the same chunk keeps failing.
That is upstream's behaviour and the port reproduces it rather than "improving" it into best-effort,
because the alternative — deleting the base first — would orphan chunks holding a live token.
**cyrup** — use the non-swallowing removal in the remove path and the swallowing one in the write
path; do not share a helper between them.
**verify** — a backend whose `remove` fails for chunk index 1 only: `remove_auth_entry` returns
`Err(Unavailable { Remove })`, the base account still holds the manifest, and chunk 0 is gone.

**MCP-286 — Bound `chunkCount` on read** · low · S · **hand-written**
**upstream** — `isAuthEntryChunkManifest` requires `chunkCount` to be an integer `> 0` and imposes
**no upper bound**; `getAuthEntryChunkAccounts` then materialises exactly that many account strings.
**behavior** — a corrupt or hostile base payload claiming `chunkCount: 1e9` drives an unbounded loop
of keyring reads. In JS that is a slow hang; in Rust, allocation on an attacker-influenced count is a
sharper failure.
**cyrup** — decided: cap at 64, which covers a 64 KB credential with headroom, and treat anything
above it as "not a manifest" — the same degradation upstream's own validator already applies to every
other malformed field, so the *shape* of the divergence is idiomatic. Document the cap as a cyrup
addition. The threat model is "an attacker who can already write your keychain", so this is
belt-and-braces, not a fix.
**verify** — a manifest with `chunkCount: 100000` is treated as an ordinary (unparseable) entry
rather than looping.

**MCP-287 — The subprocess timeout path and the unreachable ladder rung** · medium · S · **hand-written**
**upstream** — `mcp-auth.ts` `runLinuxKeyringRecoveryOperation` and `mcp-keyring-helper.cjs`'s catch
block.
**behavior** — two facts a Rust implementer will otherwise get wrong. (1) `spawnSync`'s
`timeout: 10_000` surfaces through `result.error`, **not** `result.status`, so a hung `keyctl`
produces the rung-1 message `Linux keyring recovery helper could not start: <ETIMEDOUT message>` —
not a timeout-specific string and not an exit-code message. (2) Rung 5 (`ok === false` ⇒ the helper's
own error text) is **unreachable against the real helper**, because the helper sets
`process.exitCode = 1` alongside every `{ok:false}` reply and the parent checks `status !== 0` first;
the user therefore sees `Linux keyring recovery helper failed with exit code 1`. A Rust helper that
exits 0 on error would silently *change* the message the user sees.
**cyrup** — implement the kill-after-10 s as a wait-with-timeout that, on expiry, kills the child and
returns the **rung-1** variant, not a new one. Make the Rust helper exit **1** on every error reply,
matching the `.cjs`, so rung 2 keeps winning. Keep rung 5 implemented anyway — it is the documented
contract for any third-party helper substituted through `…_KEYRING_RECOVERY_HELPER`.
**verify** — a fixture helper that sleeps 30 s ⇒ the rung-1 message within ~10 s and no zombie; a
fixture helper printing `{"ok":false,"error":"boom"}` and exiting 1 ⇒ the rung-2 message; the same
helper exiting 0 ⇒ the rung-5 message `boom`.

**MCP-288 — The three `expiresAt` predicates** · low · S · **rmcp**
**upstream** — three call sites read the same field with three different zero-semantics:
`mcp-auth.ts` `isTokenExpired` (`!expiresAt` ⇒ "no expiry"), `oauth-handler.ts` `getStoredTokens`
(`expiresAt !== undefined && expiresAt < now` ⇒ `0` is expired), `mcp-oauth-provider.ts` `tokens()`
(`expiresAt ? … : undefined` ⇒ `0` omits `expires_in`).
**behavior** — collapsing them into one helper is the natural refactor and it silently changes
behaviour at whichever site loses.
**cyrup** — **largely dissolved.** rmcp owns the live predicate (MCP-267) and the SDK-shape
conversion no longer exists (MCP-271), so two of the three sites disappear. Exactly one predicate
remains hand-written, in the legacy-import converter, and it takes `getStoredTokens`'s semantic
(`expiresAt = 0` ⇒ already expired). Record the divergence in a comment at that one site so a later
reader does not "restore" `isTokenExpired`'s falsy rule there.
**verify** — the import-converter table in MCP-267, with the `expiresAt = 0.0` row asserted as
`expires_in = 0`, not "no expiry".

**MCP-289 — Create the `cyrup-mcp` crate** · n/a · S · **extension-owned**
**upstream** — n/a.
**cyrup** — a new workspace member `crates/cyrup-mcp`, added to `members` and `default-members`, the
same shape as `crates/cyrup-ext-subagents`: a **native built-in crate compiled into the binary**,
attached in `crates/cyrup/src/main.rs` through `SessionFactory::with_native_extension` and loaded by
the session builder via `ExtensionHost::load_native_with_services`. Not a host addition, not an open
decision, and not a prerequisite in the sense the previous edition used — creating a workspace member
is ordinary work with an existing template. `NativeExtension::is_ambient` is `true` (upstream ships
as an installed package, so `--no-extensions` must switch it off);
`NativeExtension::decides_project_trust` stays `false` (its default — opting in runs `init` twice on
the same object in the pre-trust bootstrap pass, and `init` is not idempotent). This section's home
inside it is `src/auth/{mod,entry,account,chunk,backend,cache,recovery,legacy,rmcp_store}.rs`. The one
part that lands **outside** it is `crates/cyrup/src/mcp_keyring_helper_cmd.rs`, alongside
`intercom_broker_cmd.rs` and `subagent_runner_cmd.rs`.
**verify** — `cargo tree -p cyrup-mcp` succeeds and `cargo test --workspace` still builds.

**MCP-280 — The keychain service name, and what happens to a co-installed `pi-mcp-adapter`** · high · S · **hand-written**
**upstream** — `mcp-auth.ts` `AUTH_SECRET_SERVICE = 'pi-mcp-adapter.oauth'`.
**behavior** — the service name is the durable identity of every stored credential and is
user-visible in Keychain Access, seahorse and `keyctl show`.
**cyrup** — **decided, and the decision is close to forced.** The port's payload is not
wire-compatible with upstream's: `StoredTokens{accessToken,…}` versus
`StoredCredentials{client_id, token_response{access_token,…}}`. Writing the new shape under
`pi-mcp-adapter.oauth` would put an unreadable payload on the exact account a live `pi-mcp-adapter`
install reads, and `parseAuthEntryPayload` would reject it — destroying that install's credentials.
So: service becomes **`cyrup.mcp.oauth`**, and on a cold read the store performs a **one-time,
read-only import** from the same account under `pi-mcp-adapter.oauth` using MCP-256's translator,
writes the result under the new service, and — unlike the legacy *file* case — **does not delete the
source**. A keychain entry is not a plaintext leak, and deleting it would break the co-installed
install for no security benefit. That asymmetry between the file importer (delete, mandatory) and the
keychain importer (leave, mandatory) is the thing to get right.
**verify** — seed the legacy service with an upstream-shaped `AuthEntry` and assert: the first read
produces a translated record under the new service, the legacy entry is still present afterwards, the
second read does not touch the legacy service, and a legacy entry that fails translation leaves both
services untouched rather than half-written.

**MCP-281 — Adopt the keychain-mandatory posture** · medium · M · **hand-written**
**upstream** — `mcp-auth.ts` module doc and `getKeyringEntry`: keychain or nothing; the plaintext file
is import-only and is deleted.
**behavior** — on a machine with no usable credential store — a headless Linux box with no D-Bus and
a revoked keyring, a hardened CI container — MCP OAuth becomes unusable. That is upstream's
deliberate choice, the failure is loud and it names the fix.
**cyrup** — adopt it verbatim. cyrup's opposite posture (`cyrup_config::auth::AuthStore`'s plaintext
0600 `auth.json`, with headless/CI as a supported path) is not a counter-argument here because no
cyrup MCP credential exists today, so nothing regresses. A keychain-first-with-file-fallback variant
would need an explicit product decision, must never be the default, and would have to be loud; a
file-only variant is a plaintext downgrade against upstream and must not be chosen for convenience.
The one thing that materially affects (a)'s viability on Linux is which backend gets linked — the
open decision below.
**verify** — with no default store available, every store operation returns the MCP-252
unavailability sentence and no file is written anywhere.

**MCP-282 — Env-var namespace for the surviving switches** · low · S · **hand-written**
**upstream** — the six `PI_MCP_ADAPTER_*` storage switches plus the unprefixed `MCP_OAUTH_DIR`.
**behavior** — operators and the test suite drive storage behaviour through these names; a silent
rename breaks existing scripts.
**cyrup** — apply the established in-tree dual-read convention: `CYRUP_MCP_<SUFFIX>` first, then
`PI_MCP_ADAPTER_<SUFFIX>`. `cyrup_config::env` reads `["CYRUP_AGENT_DIR", "PI_CODING_AGENT_DIR"]`,
`cyrup_provider::auth::oauth::callback` reads
`["CYRUP_OAUTH_CALLBACK_HOST", "PI_OAUTH_CALLBACK_HOST"]`, and
`cyrup_ext_subagents::exec::mcp_direct_tools` does the same by hand — this is convention, not a new
decision. `MCP_OAUTH_DIR` stays **unprefixed and unchanged**, because it names a directory a user may
deliberately share with a real pi install. `…_KEYRING_RECOVERY_NODE` is dropped outright (MCP-260).
**verify** — each switch honoured under both names, with `CYRUP_*` winning.

**MCP-290 — Persist the DCR client record rmcp's `StoredCredentials` drops** · medium · S · **hand-written**
**upstream** — `mcp-oauth-provider.ts`'s `saveClientInformation` / `clientInformation` over
`StoredClientInfo { clientId, clientSecret, clientIdIssuedAt, clientSecretExpiresAt, redirectUris,
issuer, configPreRegistered }`.
**behavior** — a **confidential** dynamically-registered client must survive a restart with its
secret, or the first refresh after a restart sends a `client_id` with no secret, draws
`invalid_client`, and wipes the credentials.
**cyrup** — rmcp's `StoredCredentials` carries `client_id` only, and
`AuthorizationManager::initialize_from_store` calls `configure_client_id(&stored.client_id)`. So
`cyrup-mcp` persists the DCR response fields itself in the same `AuthEntry` (the `client` slot) and,
**after** `initialize_from_store()` returns, calls
`AuthorizationManager::configure_client(OAuthClientConfig::new(client_id, redirect_uri)
.with_client_secret(secret))` to re-apply them. rmcp's public API supports this; it is roughly
twenty lines and not a blocker. `configPreRegistered` rides in the same record for section 07's stub
check.
**verify** — register a confidential client, drop and rebuild the `AuthorizationManager` from the
store, and assert the next refresh carries the secret; assert a `configPreRegistered` stub is stored
and round-trips but is never handed back as a usable client.

**MCP-291 — Implement `rmcp::transport::auth::{CredentialStore, StateStore}` over the keychain** · high · M · **hand-written**
**upstream** — no direct analogue: upstream's `McpOAuthProvider` implements the TS SDK's
`OAuthClientProvider` and owns storage itself.
**behavior** — rmcp's `AuthorizationManager` performs every persistence operation through these two
traits, so this adapter is the single point where the whole OAuth flow meets the keychain. Get it
wrong and either credentials never persist or the manager sees a store failure as "no credentials"
and restarts an authorization the user already completed.
**cyrup** — `CredentialStore` is `#[async_trait]` with `load()/save()/clear()` and **takes no key**,
so `cyrup-mcp` instantiates **one store per server**, bound to that server's account. That is the
natural shape, not a workaround, and it is what makes `McpAuthStore`'s server-name keying line up
with rmcp's keyless trait. `StateStore` is keyed by CSRF token
(`save(csrf, state)` / `load(csrf)` / `delete(csrf)`); keeping one `state` slot in the `AuthEntry`
and returning it only when `state.csrf_token == csrf` reproduces upstream's single-`oauthState`
semantics exactly. Both traits are `async` while `keyring` is blocking, so every call goes through
`tokio::task::spawn_blocking`, and both must map `AuthStoreError::Unavailable` to
`rmcp::transport::auth::AuthError::InternalError` — never to `AuthorizationRequired`, or a broken
keychain becomes an infinite re-auth loop, which is exactly the failure mode §6.9's error-class
contract exists to prevent. Handing the stores over is
`AuthorizationManager::{set_credential_store, set_state_store}`; rmcp's `InMemoryCredentialStore` and
`InMemoryStateStore` remain the defaults for tests.
**verify** — a full `OAuthState` round trip against a mock authorization server with the keychain
store attached: authorize, callback, exchange, restart the manager, `initialize_from_store()` returns
`true`, refresh succeeds. Assert a store failure during `load` surfaces as `InternalError` and does
**not** trigger re-authorization. Assert a `StateStore::load` with a non-matching CSRF token returns
`None` rather than the stored state.

---

### Out of scope

Each of these is a **decision by the project owner**, recorded with its reason so a later pass does
not re-file it as a gap.

* **MCP Apps / the UI extension, entirely (Cut 2)** — and with it, from this section:
  `consent-manager.ts` (`ConsentManager`), `errors.ts`'s `ConsentError` and the `McpUiError` umbrella
  it extends, and the per-session `McpExtensionState.consentManager` slot. **Reason:** `ConsentManager`
  exists to gate an MCP-App iframe re-entering the local host server at `POST /proxy/tools/call`; a
  `git grep` at v2.25.0 finds its only consumers are `ui-server.ts` (three call sites) and
  `ui-session.ts` (one). With no host server and no iframe, it has zero callers. **Where the seam
  falls:** the model-facing approval gate — `tool-approval.ts`'s `isToolCallApprovalRequired`, the
  `Allow once / Allow for session / Deny` select, the session `approvedToolCalls` cache, and the
  headless `approval_required_headless` refusal — is a *different mechanism on a different path* and
  it survives whole, behind `ExtHooks::before_tool_call` and `cyrup_permission_system`'s
  `create_mcp_permission_targets`. It belongs to the approval section, not this one. This refines the
  seam map's §6 row A-5 and §9 file table, which group `consent-manager.ts` with `tool-approval.ts`
  under "port": the *approval* half of that pair ports, the *consent* half does not, and the grouping
  is exactly the entanglement Cut 2 asks to be named.
* **`McpToolApprovalOrigin`'s `"iframe"` variant (Cut 2)** — no app-initiated tool calls exist, so no
  approval can originate from one.
* **`McpToolApprovalOrigin`'s `"script"` variant and `McpSettings.scriptMode` (Cut 4)** — the
  `mcpScript` worker is cut; `executeCall`'s `origin?: "proxy" | "script"` parameter keeps its
  `"proxy"` default and loses its other call site.
* **`PI_MCP_ADAPTER_KEYRING_RECOVERY_NODE`** — it names a JavaScript interpreter. After MCP-260 the
  re-execed program is the `cyrup` binary itself and there is nothing to override. The
  `…_KEYRING_RECOVERY_KEYCTL` and `…_KEYRING_RECOVERY_HELPER` overrides survive; the latter now names
  a program rather than a script. **Recorded rather than silently dropped**, because a reader who
  finds five of six switches ported would otherwise read the sixth as an oversight.
* **`loadKeyringEntryClass`, `loadKeyringNativeBindingFallback`, `getKeyringNativeBindingTargets`,
  `getKeyringNativeBindingSuffixes`, `loadTestKeyringEntryClass`, `formatErrorMessage`, and the
  identical block inside `mcp-keyring-helper.cjs`** — a `.node` dynamic-load fallback across twelve
  platform triples. **Reason:** the Rust backends are linked at compile time; there is no module load
  to fail and no path to fall back to. The *error condition* they guard is preserved (MCP-252 keeps
  the unavailability sentence).
* **`recheck` and any JavaScript engine** — not applicable to this section, and named here only so
  the question does not resurface: with Cut 4 there is no JS in the port at all, and `node` is not a
  production dependency of `cyrup-mcp` — including in the keyring-recovery hop, which re-execs
  `current_exe()`.
* **`axum` and any local HTTP server** — Cut 2 removed the only reason to want one. The OAuth
  loopback callback listener is a separate thing, it stays, and it is
  `cyrup_provider::auth::oauth::callback`'s, not a new one.

---

### What does not fit cleanly

**One genuine open decision, and it is a dependency-configuration decision, not a host addition.**
No host-additions arise from this section: everything here is a native crate opening its own keychain
handles, spawning its own subprocess and writing its own files. `HostServices` is not consulted once.

**OPEN-1 — which Linux credential store `keyring` links, and therefore whether MCP-260/261/262/287
are live code.**

The revoked-session-keyring failure mode is specific to the kernel keyutils backend. Read from the
published `keyring` 4.1.6 manifest and `src/v1.rs`:

* `default = ["v1"]`, and
  `v1 = ["apple-native-keyring-store/keychain", "windows-native-keyring-store",
  "zbus-secret-service-keyring-store"]`. On Linux, `v1` selects **Secret Service over zbus**.
  `linux-keyutils-keyring-store` is an optional dependency that `v1` does **not** enable.
* `cli` enables every store crate, including `linux-keyutils-keyring-store`, and exposes
  `keyring::use_native_store(prefer_secret_service: bool)`, whose Linux arm picks `keyutils` when the
  flag is `false`. `cli` does **not** export an `Entry` type — with `cli` alone you use
  `keyring_core::Entry` directly.
* The crate's own docs say applications that want to control which stores they use "should not be
  linking to this library at all" and should link `keyring-core` plus the specific store crates.

Three options:

* **(a) `keyring = { version = "4.1.6", default-features = false, features = ["v1"] }`.** Smallest
  dependency surface, `keyring::Entry` unchanged from upstream's shape. Linux uses Secret Service,
  so a revoked kernel session keyring cannot occur and **MCP-260, MCP-261, MCP-262 and MCP-287 become
  dead code and should be cut**. Cost: a headless Linux box with no D-Bus session has no credential
  store at all, where keyutils would have worked — which is precisely the environment upstream's
  recovery path exists to serve, and it makes MCP-281's keychain-mandatory posture harsher on exactly
  the machines most likely to hit it.
* **(b) `keyring = { version = "4.1.6", default-features = false, features = ["cli"] }`** plus an
  explicit `keyring-core` dependency, calling `keyring::use_native_store(false)` once at store
  construction and using `keyring_core::Entry`. Reproduces `@napi-rs/keyring`'s Linux backend, keeps
  the `keyctl` mechanism load-bearing, and keeps `Entry::{new, get_password, set_password,
  delete_credential}` identical. Cost: `cli` also pulls `dbus-secret-service-keyring-store`,
  `zbus-secret-service-keyring-store`, `db-keystore` (sqlite) and `keyring-core/sample` — real weight,
  and `db-keystore` compiles on every non-mobile target.
* **(c) Link `keyring-core = "1"` plus `apple-native-keyring-store`, `windows-native-keyring-store`
  and `linux-keyutils-keyring-store` directly**, calling `keyring_core::set_default_store` per
  platform. Exactly what keyring 4.x's docs prescribe for an application, exactly the backends
  upstream has, and no sqlite or duplicate secret-service crate. Cost: four dependencies instead of
  one, and the `keyring = "4.1.6"` line in the settled dependency set becomes `keyring-core` plus
  stores — the same code, a different façade.

**Recommendation: (c)**, falling back to (b) if the settled `keyring` line must be preserved
verbatim. (c) gives the upstream backend set with the smallest tree, keeps the whole of §6.7 real,
and matches the workspace's habit of not pulling optional store backends it never uses. (a) is
defensible only as an explicit product decision that headless Linux without D-Bus is unsupported —
and if it is taken, four port units must be cut in the same breath rather than left as dead code.
Whichever is chosen, `keyring_core::Error`'s variants and `Display` strings are identical across all
three, so MCP-252, MCP-258 and MCP-262 are unaffected in shape.

**Named residual risk, not a decision.** MCP-262's predicate matches `Display` text. The outer layer
is now confirmed — `keyring_core::Error::NoStorageAccess` renders as `Couldn't access platform
storage: {err}`, byte-identical to the string upstream fabricates — but the inner platform error's
rendering under `linux-keyutils-keyring-store` on a genuinely revoked session keyring has not been
observed. macOS never revokes a keyring, so MCP-260/261/262/287 are exercised on a dev machine only
through the forced-test variable and a fake `keyctl`. A real revoked-session Linux box is the only
thing that closes it, and under option (a) the question does not arise at all.

---

### Coverage

**Read** —
*Upstream, in full at v2.25.0*: `mcp-auth.ts`, `mcp-keyring-helper.cjs`, `consent-manager.ts`,
`oauth.ts`, `oauth-handler.ts`, `__tests__/mcp-auth-storage.test.ts` (17 tests),
`__tests__/mcp-auth-cache.test.ts` (13 tests), `__tests__/oauth-handler.test.ts`, `agent-dir.ts`.
*Upstream, targeted regions*: `config.ts` `resolveConfiguredOAuthDir`; `errors.ts` `ConsentError` /
`McpUiError`; `init.ts` `initializeMcp`'s storage-options and consent-construction sites; `state.ts`
`McpExtensionState`; `ui-server.ts`'s three consent call sites and `ui-session.ts`'s state hand-off;
`mcp-auth-flow.ts`'s two dedup-key sites and `getValidToken`; `mcp-oauth-provider.ts`
`saveClientInformation` / `clientInformation` / `tokens` and the `expiresAt` write site;
`server-manager.ts`'s two `invalidateAuthEntryCache` sites; `session-recovery.ts`'s one;
`commands.ts`'s `/mcp` status consumer; `__tests__/consent-manager.test.ts`;
`__tests__/agent-dir-paths.test.ts`; describe/it index of
`__tests__/server-manager-auth-cache-recovery{,-integration}.test.ts`. Repo-wide `git grep` at the
tag for `ConsentManager`, `resetAuthEntryCache`, `inspectAuthForUrl`, `getAuthEntryFilePath`.
*rmcp, from the checkout*: `crates/rmcp/src/transport/auth.rs` — `StoredCredentials`,
`CredentialStore`, `InMemoryCredentialStore`, `StoredAuthorizationState`, `StateStore`,
`InMemoryStateStore`, `AuthError`, `AuthorizationManager::{initialize_from_store, get_access_token,
try_refresh_or_reauth, refresh_token, set_credential_store, set_state_store, configure_client,
configure_client_id, set_allow_missing_issuer}`, `OAuthClientConfig`, `OAuthTokenResponse`,
`REFRESH_BUFFER_SECS`; `crates/rmcp/src/transport.rs` exports.
*`keyring` 4.1.6 and `keyring-core` 1.0.0, from the published crates*: the crates.io index manifest
(features, optional store dependencies, target gating), `keyring/src/v1.rs` (`Entry::{new,
store_status, get_password, set_password, delete_credential}`, `set_credential_store`),
`keyring/src/cli.rs` (`NAMED_STORES`, `use_named_store`, `use_native_store`,
`use_linux_keyutils_store`), `keyring-core/src/error.rs` (the `Error` enum, its `Display`, its
`source`), `keyring-core/src/lib.rs` (`Entry::new`, `set_default_store`), `keyring-core/src/mock.rs`
(`Store::new`, `set_error`).
*cyrup, by symbol on branch `david/cyrup`*: `cyrup_config::auth::AuthStore` (open/reload/modify/
delete, the `to_string_pretty` write, the 0600 assertion), `cyrup_config::lock::FileLock`,
`cyrup_config::env` (`ConfigDirs`, the `CYRUP_AGENT_DIR`/`PI_CODING_AGENT_DIR` pair),
`cyrup_provider::auth::store::CredentialStore` (and its `modify` contract),
`cyrup_provider::auth::types::Credential`, `cyrup_provider::auth::oauth::callback`
(`CallbackServerConfig`, `CallbackHandler`, `CallbackOutcome`, and its env-var dual-read),
`crates/cyrup/src/{intercom_broker_cmd,subagent_runner_cmd,main,lib}.rs`,
`cyrup_intercom::transport::spawn`'s `current_exe()` use,
`cyrup_ext_subagents::exec::mcp_direct_tools` (`compute_mcp_server_hash`, the agent-dir chain),
`cyrup_permission_system::{jsonc, stores::SessionApprovalStore}`,
`cyrup_ext::host::services::HostServices`, `cyrup_ext::caps::proc::ProcCaps`, `crates/cyrup-it`'s
`autotests`/`required-features` layout.

**Excluded** —
* `mcp-auth-flow.ts`, `mcp-oauth-provider.ts`, `mcp-callback-server.ts` — section 07's files, read
  only far enough to pin the cross-section contracts (the store error class must stay
  distinguishable; `getAuthBaseDir` must stay public; the `expiresAt` write site).
* `ui-server.ts` beyond its three consent call sites, and `host-html-template.ts`'s
  `requireToolConsent`/`cacheToolConsent` rendering — Cut 2.
* `tool-approval.ts` and the model-facing approval gate — a surviving mechanism that belongs to the
  approval section; only the seam against consent is stated here.
* `__tests__/ui-integration.test.ts`, `ui-viewer-none.test.ts`, `commands-auth.test.ts`,
  `commands-panel-auth-storage.test.ts`, `mcp-panel-auth.test.ts`, `mcp-oauth-provider.test.ts`,
  `mcp-auth-flow-client-credentials.test.ts`, `direct-tools-auto-auth.test.ts`,
  `proxy-modes-{auto,manual}-auth.test.ts`, `server-manager-http-auth.test.ts` — they exercise other
  sections *through* this section's API rather than pinning this section's behaviour.
  `server-manager-auth-cache-recovery{,-integration}.test.ts` were indexed, not read line by line:
  they pin the *invalidation policy* that drives MCP-257's primitive, and that policy belongs to the
  server-manager section.
* `structuredClone` — `Clone` on an owned record with no cycles is the exact equivalent; the
  *property* it provides is specified in MCP-257 and pinned by MCP-283.
* `zeroize` / `secrecy` adoption — upstream does not zeroize and the OS store owns the durable copy;
  named in MCP-277 as possible hardening, not filed as a port unit.
* `oauth2 = "5.0.0"`, already in `cyrup-config` — this section does not touch it directly. It is,
  however, the crate rmcp's `auth` feature pulls, so it is not dead: `AuthorizationManager` and
  `StoredCredentials.token_response` are `oauth2` types.

**Corrections to the first pass** —
* `keyring = "4.1.6"` **exists and is not yanked**, and its feature names are
  `apple-native-keyring-store/keychain`, `windows-native-keyring-store`,
  `linux-keyutils-keyring-store`, `zbus-secret-service-keyring-store`,
  `dbus-secret-service-keyring-store`, `db-keystore`, composed by `v1` and `cli`. The first pass
  "corrected" these to 3.6.3's names (`apple-native`, `linux-native-sync-persistent`, …) from an
  eleven-month-stale index snapshot; those are the **wrong** names for 4.x. Read from the live index
  manifest and the published crate source.
* `keyring` 4.x is a *registry* architecture over `keyring-core`, not the 3.x monolith:
  `Entry::new` returns `Result`, absence is `Err(Error::NoEntry)` rather than a null, and a default
  store must be set before any entry exists. All three change the store adapter.
* The claim that `@napi-rs/keyring` binds this crate family, flagged unverifiable by the first pass,
  is **confirmed by string identity**: `keyring_core::Error::TooLong`'s `Display` is
  ``Value of '{name}' is longer than the platform limit of {limit} chars`` and `NoStorageAccess`'s is
  ``Couldn't access platform storage: {err}`` — byte-identical to the two messages upstream's
  fault-injection stores fabricate.
* `keyring_core::mock::Store` with `set_error(Error)` is ungated and replaces four of upstream's five
  hand-rolled test backends. MCP-258 shrinks accordingly.
* The Windows 2560-byte ceiling is a **typed** variant (`Error::TooLong(name, limit)`), not a message
  match. The first pass specified a string-matching test store.
* **`keyring`'s `v1` feature selects Secret Service on Linux, not keyutils.** The first pass assumed
  the keyutils backend was a given and never questioned whether the recovery path was reachable.
  It is now OPEN-1, and under one option four port units are cut.
* **`consent-manager.ts` is cut, not ported.** Its only consumers are `ui-server.ts` and
  `ui-session.ts`, both Cut 2. MCP-272/273/274 are re-verdicted `cut`, and the seam against the
  surviving model-facing approval gate is stated in *Out of scope*.
* **`crates/cyrup-mcp` is not a "critical open-question prerequisite".** It is a new native built-in
  crate, the same shape as `crates/cyrup-ext-subagents`, attached through
  `SessionFactory::with_native_extension`. MCP-289 is re-verdicted `extension-owned`, severity `n/a`.
* **`getStoredTokens`'s SDK conversion dissolves into rmcp** — `StoredCredentials.token_response`
  *is* `oauth2::StandardTokenResponse`. MCP-271 is re-verdicted `rmcp` and MCP-288 shrinks from three
  predicates to one.
* **`isTokenExpired` dissolves into rmcp** — `AuthorizationManager::get_access_token` owns the live
  expiry check with a 30 s refresh buffer. MCP-267 is re-verdicted `rmcp`; fractional absolute
  seconds survive only in the legacy-import converter.
* **`codeVerifier` and `oauthState` dissolve into rmcp's `StoredAuthorizationState`** and are served
  through `StateStore`. Five of upstream's 21 production accessors go with them, and MCP-266 shrinks.
* **The keychain service name is a near-forced decision, not an open one.** The port's payload is not
  wire-compatible with upstream's, so writing under `pi-mcp-adapter.oauth` would destroy a
  co-installed `pi-mcp-adapter`'s credentials. MCP-280 is re-verdicted `hand-written` with the
  decision and a one-time read-only import that deliberately does **not** delete the source.
* **The UTF-16 chunking hazard's blocking branch is gone.** Because the service name and the record
  shape both change, no JS writer ever reads these accounts, so byte-boundary chunking has no cost.
* **MCP-268 and MCP-281 are decided, not open.** A per-server `tokio::sync::Mutex` restores exactly
  the atomicity JS supplied and invents nothing; keychain-mandatory is upstream's posture and nothing
  regresses because no cyrup MCP credential exists yet.
* **MCP-282 is convention, not a decision.** `CYRUP_*` then `PI_*` dual-read is already the in-tree
  pattern in `cyrup_config::env` and `cyrup_provider::auth::oauth::callback`.
* **MCP-286 is decided** — cap `chunk_count` at 64 and treat anything larger as "not a manifest",
  which is the degradation upstream's own validator already applies to every other malformed field.
* Two new units were added for work the first pass had no place for, both now visible because rmcp
  owns the protocol: **MCP-290** (persist the DCR client record `StoredCredentials` drops) and
  **MCP-291** (implement rmcp's two store traits over the keychain — the central adapter of this
  section).
* Every cyrup line-number and commit citation from the first pass is removed. Nothing in this section
  is anchored to a line, a sha, or a working-tree state.
