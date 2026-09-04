//! Environment interpolation and the `!` / `!!` command-secret grammar — `utils.ts`, plus the two
//! connect-time sites in `server-manager.ts` that execute it (MCP-082/MCP-083, MCP-342/MCP-349).
//!
//! # Why this is its own module
//!
//! Upstream these five functions live in `utils.ts` and are imported by `mcp-oauth-provider.ts`
//! (the `clientSecret` leg), by `server-manager.ts` (`resolveEnv` and `connectHttpClient`) and by
//! `metadata-cache.ts` (the *non-executing* hash leg). The port originally landed the engine inside
//! [`crate::oauth`] because the OAuth `clientSecret` was its first caller, and `runtime.rs` deferred
//! the other two to "a `secrets` module" that did not exist. Hoisting it here makes the layering
//! match the call graph — [`crate::credentials`] owns the interpolation engine, this module owns the
//! secret grammar and the subprocess, and `oauth` / `runtime` are its callers — instead of making
//! the stdio transport depend on the OAuth flow for a shell spawn. [`crate::oauth`] re-exports every
//! name that moved, so `crate::oauth::resolve_command_secret` and friends still resolve.
//!
//! # The security property is a *timing* property (MCP-083, conformance C6)
//!
//! [`resolve_command_secret`] is the only function here that spawns anything, and it is reachable
//! from exactly three places: [`resolve_env`] (stdio connect), [`resolve_http_secrets`] (HTTP
//! connect) and `oauth::resolve_client_secret` (token leg). Discovery, config merge, `/mcp`
//! previews, `computeServerHash` and every renderer use [`interpolate_env_record`] /
//! [`interpolate_secret_expression`] instead, which unescape `!!x` to the literal `!x` and leave a
//! bare `!x` **verbatim, unexecuted**. Getting that split wrong — resolving `!` at merge or hash
//! time — means merely listing config in a repo that carries a hostile `.mcp.json` runs arbitrary
//! shell the user never approved. Every function below states which side of the line it is on.
//!
//! # The four `context` strings
//!
//! `context` is what the user reads when their secret command fails, so all four of upstream's
//! strings are reproduced at the site that actually resolves the secret (13b §9):
//!
//! | context | site |
//! |---|---|
//! | `MCP server "{name}" stdio env "{key}"` | [`resolve_env`] |
//! | `MCP server "{name}" HTTP header "{key}"` | [`resolve_http_secrets`] |
//! | `MCP server "{name}" HTTP bearer token` | [`resolve_http_secrets`] |
//! | `MCP server "{name}" OAuth clientSecret` | `oauth::resolve_client_secret` |

use std::collections::{BTreeMap, HashMap};
use std::io::Read as _;
use std::sync::LazyLock;
use std::time::Duration;

use crate::config::{AuthKind, AuthMode, ServerEntry};
use crate::credentials::EnvFn;
use crate::errors::{McpError, McpResult};

// ===================================================================================================
// 1 · Interpolation — the non-executing half (MCP-082, MCP-342)
// ===================================================================================================

/// The process environment, resolved once. [`crate::credentials::process_env`] allocates an `Arc`
/// per call; the interpolators run per config field, so the handle is hoisted.
pub(crate) static PROCESS_ENV: LazyLock<EnvFn> = LazyLock::new(crate::credentials::process_env);

/// `interpolateEnvVars(value)` (`utils.ts:74`) — expand the **three** placeholder forms, in order,
/// each falling back to the empty string on a missing variable:
///
/// ```text
/// 1.  ${VAR}       /\$\{(\w+)\}/g
/// 2.  $env:VAR     /\$env:(\w+)/g
/// 3.  {env:VAR}    /\{env:(\w+)\}/g      <- the form both existing cyrup copies are missing
/// ```
///
/// **MCP-342.** `cyrup_ext::caps::proc::interpolate_env_vars` (currently `pub(crate)`) and the
/// private copy in `cyrup_ext_subagents::exec::mcp_direct_tools` each implement only the first two
/// forms, so a `{env:VAR}`-bearing `clientId` would reach the authorization server literally. That
/// is a parity defect in both, not a visibility problem; this is the third implementation and the
/// only complete one. The shared-implementation consolidation is filed in the report.
///
/// One deliberate character-class divergence: JavaScript's `\w` is ASCII-only while Rust's `regex`
/// crate makes `\w` Unicode-aware, so the patterns spell the class out as `[A-Za-z0-9_]` to keep
/// the two engines matching the same names.
#[must_use]
pub fn interpolate_env_vars(value: &str) -> String {
    crate::credentials::interpolate_env_vars(value, &PROCESS_ENV)
}

/// **De-duplicated at integration (MCP-082, MCP-342).** The engine is
/// [`crate::credentials::interpolate_env_vars_with`] — one implementation for the whole crate, so
/// an `oauth` block and a `bearerToken` cannot disagree about what `${VAR}` means. The three-pass
/// chaining moved with it.
pub use crate::credentials::interpolate_env_vars_with;

/// `interpolateSecretExpression(value)` (`utils.ts:102`) — `!!X` becomes `X` interpolated (one `!`
/// removed), a single leading `!` is preserved verbatim for the command resolver, and everything
/// else is interpolated.
///
/// **Spawns nothing.** This is the form every non-connect path uses.
#[must_use]
pub fn interpolate_secret_expression(value: &str) -> String {
    // One implementation, in `credentials.rs` (MCP-084); this is its process-env arity, which is
    // upstream's own one-argument signature.
    crate::credentials::interpolate_secret_expression(value, &PROCESS_ENV)
}

/// `interpolateEnvRecord(values)` (`utils.ts:107`) — [`interpolate_secret_expression`] applied per
/// value, `undefined` in / `undefined` out.
///
/// **This is the record form that must NOT spawn**, and it is the one every non-connect caller
/// wants. Upstream's callers are `metadata-cache.ts:90/93/98` (the `computeServerHash` pre-image for
/// `env`, `headers` and `requestHeadersCommand.env`) and `mcp-auth-flow.ts:254` (the OAuth
/// discovery headers). Its in-tree consumer-in-waiting is
/// [`crate::dirs::ResolvedIdentity`]'s `env` / `headers`, whose `verbatim` constructor still copies
/// them unresolved under a `TODO(MCP-082, MCP-084)`.
///
/// The `env` seam is explicit rather than reading `std::env` so a hash test can pin a variable
/// without `std::env::set_var`, which edition 2024 makes `unsafe` (MCP-082's `cyrup` note).
#[must_use]
pub fn interpolate_env_record(
    values: Option<&BTreeMap<String, String>>,
    env: &EnvFn,
) -> Option<BTreeMap<String, String>> {
    values.map(|values| {
        values
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    crate::credentials::interpolate_secret_expression(value, env),
                )
            })
            .collect()
    })
}

// ===================================================================================================
// 2 · `resolveCommandSecret` — the executing half (MCP-083, MCP-349)
// ===================================================================================================

/// `COMMAND_SECRET_TIMEOUT_MS` (`utils.ts:116`).
pub const COMMAND_SECRET_TIMEOUT: Duration = Duration::from_secs(10);
/// `COMMAND_SECRET_MAX_OUTPUT_BYTES` (`utils.ts:117`) — 1 MiB.
pub const COMMAND_SECRET_MAX_OUTPUT_BYTES: usize = 1024 * 1024;
/// How often the wait loop polls the child while the wall clock runs.
const COMMAND_SECRET_POLL: Duration = Duration::from_millis(10);

/// `CREATE_NO_WINDOW` (`winbase.h`) — see [`resolve_command_secret`]'s `windowsHide` note.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `resolveCommandSecret(value, context)` (`utils.ts:123`) — MCP-349.
///
/// * `!!X` ⇒ interpolate `X` with one `!` stripped, **no subprocess**;
/// * a value not starting with `!` ⇒ plain interpolation;
/// * otherwise run `value[1..]` **through a shell** and take its trimmed stdout.
///
/// `shell: true` upstream means the string goes to `/bin/sh -c` (or `cmd.exe /C`); a port that
/// spawned the argv directly would change which configs work, so the shell is reproduced. **stderr
/// is discarded**, exactly as upstream's `stdio: ["ignore","pipe","ignore"]` does — a failing
/// command's diagnostics never reach the user, only its exit code does.
///
/// The five failure strings are the contract and all carry the caller's `context` verbatim; the
/// four `context` strings are the table in this module's header. **A failure is always an `Err`** —
/// there is no arm anywhere in this module that degrades an unresolvable secret to `""`, because
/// upstream `throw`s at every one of them and a silently-empty credential is an authentication
/// failure the user cannot diagnose.
///
/// Synchronous, as upstream is. **Named delta (MCP-349):** upstream calls `clientInformation()` —
/// and therefore this resolver — up to three times per token leg, so one token request can spawn
/// the user's secret command three times. Under `rmcp` the secret is applied once at
/// `configure_client` time and reused, so the port resolves it **once per configuration**.
///
/// # `windowsHide`, and what is and is not covered
///
/// Upstream passes `windowsHide: true`, which libuv turns into `UV_PROCESS_WINDOWS_HIDE` — two
/// distinct suppressions. The console half is the process-creation flag `CREATE_NO_WINDOW`, and
/// `std::os::windows::process::CommandExt::creation_flags` (stable since 1.16, and safe — it takes
/// a `u32`, so `#![forbid(unsafe_code)]` is untouched) sets exactly that; the Windows arm below
/// does. That is the half that governs this call site: `cmd /C <helper>` is a console-subsystem
/// child, and without the flag every `!op read …` in a user's config flashes a console window.
///
/// The GUI half — `STARTUPINFO.wShowWindow = SW_HIDE` with `STARTF_USESHOWWINDOW`, which suppresses
/// the first window of a *GUI*-subsystem child that honours `SW_SHOWDEFAULT` — is **not** applied.
/// `wShowWindow` is not reachable through `creation_flags`; the only API that reaches it is
/// `std::os::windows::process::CommandExt::show_window`, and this arm deliberately does not call
/// it. Two reasons, in order: it cannot be compiled or run from this tree (no
/// `x86_64-pc-windows-msvc` std is installed, so a Windows-only call is unverifiable at the moment
/// it is written); and hiding a credential helper's *GUI* is the behaviour a user is least likely
/// to want silently, since an interactive unlock prompt that never appears is indistinguishable
/// from a hang. **Recorded delta**: a `!`-secret helper that opens a GUI window shows it here where
/// upstream hides it. Closing it is a one-line change on a host that can build the target.
///
/// # Errors
///
/// [`McpError::Other`] carrying one of upstream's five `Failed to resolve {context}: …` sentences.
pub fn resolve_command_secret(value: &str, context: &str) -> McpResult<String> {
    if let Some(rest) = value.strip_prefix("!!") {
        return Ok(interpolate_env_vars(&format!("!{rest}")));
    }
    let Some(command) = value.strip_prefix('!') else {
        return Ok(interpolate_env_vars(value));
    };

    let failure = |reason: &str| McpError::other(format!("Failed to resolve {context}: {reason}"));

    #[cfg(windows)]
    let (shell, flag) = ("cmd", "/C");
    #[cfg(not(windows))]
    let (shell, flag) = ("/bin/sh", "-c");

    let mut spawner = std::process::Command::new(shell);
    spawner
        .arg(flag)
        .arg(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    // `windowsHide: true`'s console half. See the doc comment above for the GUI half's delta.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        let _ = spawner.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = spawner
        .spawn()
        .map_err(|_| failure("command failed to start"))?;

    // The child's stdout must be drained on another thread: a command that fills the pipe buffer
    // would otherwise block forever while this thread polls `try_wait`, and upstream's `maxBuffer`
    // has no such deadlock because libuv reads continuously.
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || -> (Vec<u8>, bool) {
        let Some(mut stdout) = stdout else {
            return (Vec::new(), false);
        };
        let mut buffer = Vec::new();
        // One byte past the cap is enough to *detect* the overflow, which is all upstream's
        // `ENOBUFS` does.
        let mut limited = (&mut stdout).take((COMMAND_SECRET_MAX_OUTPUT_BYTES + 1) as u64);
        let _ = limited.read_to_end(&mut buffer);
        let overflowed = buffer.len() > COMMAND_SECRET_MAX_OUTPUT_BYTES;
        // Drain the rest so the child is never wedged on a full pipe while we wait for it.
        let mut sink = Vec::new();
        let _ = stdout.read_to_end(&mut sink);
        (buffer, overflowed)
    });

    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if started.elapsed() >= COMMAND_SECRET_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(COMMAND_SECRET_POLL);
    };

    let (buffer, overflowed) = reader.join().unwrap_or((Vec::new(), false));

    let Some(status) = status else {
        return Err(failure("command timed out after 10 seconds"));
    };
    if overflowed {
        return Err(failure("command output exceeded 1 MiB"));
    }
    if !status.success() {
        let code = status
            .code()
            .map_or_else(|| "unknown".to_string(), |code| code.to_string());
        return Err(failure(&format!("command exited with code {code}")));
    }

    let resolved = String::from_utf8_lossy(&buffer).trim().to_string();
    if resolved.is_empty() {
        return Err(failure("command returned empty output"));
    }
    Ok(resolved)
}

/// `resolveCommandSecretsRecord(values, context)` (`utils.ts:155`) — [`resolve_command_secret`] per
/// value, with the caller building the `context` string from the **key**.
///
/// `None` in / `None` out, which is what makes upstream's two call sites read
/// `resolveCommandSecretsRecord(...) ?? {}` and `overrides ? {...} : resolved`.
///
/// **Fails closed, and on the first failing key.** Upstream's `Object.entries(...).map(...)` is
/// eager and left-to-right, and a `throw` from inside `map` abandons the whole record — so a record
/// whose second value fails never resolves its third, and no partially-resolved record is ever
/// handed to a transport. `?` inside the loop reproduces both halves exactly.
///
/// **Named delta.** Upstream iterates `Object.entries`, i.e. **insertion order**;
/// [`ServerEntry::env`] and [`ServerEntry::headers`] are `BTreeMap`s, so the iteration is
/// alphabetical. The only observable consequence is *which* of two independently-failing keys is
/// reported first, and how many commands ran before the first failure. Every individual value
/// resolves identically, and the returned record is a map either way.
///
/// # Errors
///
/// The first value whose command fails, carrying `context(key)` — see [`resolve_command_secret`].
pub fn resolve_command_secrets_record<F>(
    values: Option<&BTreeMap<String, String>>,
    context: F,
) -> McpResult<Option<BTreeMap<String, String>>>
where
    F: Fn(&str) -> String,
{
    let Some(values) = values else {
        return Ok(None);
    };
    let mut resolved = BTreeMap::new();
    for (key, value) in values {
        let _ = resolved.insert(key.clone(), resolve_command_secret(value, &context(key))?);
    }
    Ok(Some(resolved))
}

// ===================================================================================================
// 3 · The stdio connect site — `resolveEnv` (`server-manager.ts:1230`, MCP-083 + MCP-101)
// ===================================================================================================

/// `process.env` as an owned map — the base [`resolve_env`] layers over.
///
/// `std::env::vars()` **panics** on a variable whose name or value is not valid UTF-8, which the
/// crate's no-panic policy (arch-00 §8) forbids on any normal path; `vars_os` plus a UTF-8 filter
/// silently drops those instead. That filter is also the closest analogue of upstream's
/// `if (value !== undefined)` guard: a variable the child cannot be handed as a string is simply not
/// in the record. A non-UTF-8 variable is unrepresentable in `StdioTransportSpec::env` regardless,
/// since `TokioChildProcess` is handed `HashMap<String, String>`.
#[must_use]
pub fn process_env_snapshot() -> HashMap<String, String> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

/// `resolveEnv(env, serverName, literalEnv)` (`server-manager.ts:1230`) — the child's **whole**
/// environment, not a set of overrides.
///
/// The full `process.env` is copied first and the entry's `env` is layered *over* it, which is why
/// [`crate::runtime::StdioTransportSpec::env`] is documented as replace-not-merge: rmcp is handed
/// this map after an `env_clear()`, so a variable a caller deliberately dropped from `base` stays
/// dropped.
///
/// `literal_env` — set only by [`crate::agent_plugin`] — takes the entry's values **verbatim**: no
/// interpolation and, decisively, **no subprocess**. That is half of what stops a third-party plugin
/// manifest from reading the user's environment or running a shell command; the other half is that
/// the loader never copies a `command`/`args` it did not build itself.
///
/// The `context` string is `` MCP server "{server}" stdio env "{key}" `` — one of the four in
/// this module's header, and the one that had no call site before this unit landed.
///
/// # Errors
///
/// The first `env` value whose `!command` fails, with that key named.
pub fn resolve_env(
    overrides: Option<&BTreeMap<String, String>>,
    server_name: &str,
    literal_env: bool,
    base: &HashMap<String, String>,
) -> McpResult<HashMap<String, String>> {
    let mut resolved = base.clone();
    if literal_env {
        for (key, value) in overrides.into_iter().flatten() {
            let _ = resolved.insert(key.clone(), value.clone());
        }
        return Ok(resolved);
    }
    let layered = resolve_command_secrets_record(overrides, |key| {
        format!("MCP server \"{server_name}\" stdio env \"{key}\"")
    })?;
    for (key, value) in layered.into_iter().flatten() {
        let _ = resolved.insert(key, value);
    }
    Ok(resolved)
}

/// [`resolve_env`] applied to a whole [`ServerEntry`] — the shape
/// [`crate::runtime::StdioTransportSpec`] consumes.
///
/// # Errors
///
/// See [`resolve_env`].
pub fn resolve_stdio_env(
    entry: &ServerEntry,
    server_name: &str,
    base: &HashMap<String, String>,
) -> McpResult<HashMap<String, String>> {
    resolve_env(
        entry.env.as_deref(),
        server_name,
        entry.literal_env == Some(true),
        base,
    )
}

// ===================================================================================================
// 4 · The HTTP connect site — `connectHttpClient`'s secret pre-flight (§3.4 steps 2–6)
// ===================================================================================================

/// What [`resolve_http_secrets`] hands [`crate::runtime::HttpTransportSpec`]: the two fields of it
/// that carry secrets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedHttpSecrets {
    /// `definition.headers` with every `!command` executed, in the order the map iterates.
    /// `Authorization` is **not** in here — see [`Self::bearer_token`].
    pub headers: Vec<(String, String)>,
    /// The bearer token **without** the `Bearer ` prefix, or `None` when `auth` is not `"bearer"`
    /// or no token resolved truthy.
    pub bearer_token: Option<String>,
}

/// `connectHttpClient`'s secret pre-flight (`server-manager.ts:838-864`), §3.4 steps 2–6 — MCP-083's
/// half of MCP-114.
///
/// Step 1 (`resolveServerUrl`) and step 7 (`requestInit`) are **not** here: the URL resolver is
/// MCP-084's and the transport assembly is [`crate::runtime::build_http_transport_config`]'s. What
/// this function owns is every step that can execute a command, in upstream's exact order:
///
/// 2. `hasCommandHeader` — computed from the **raw** header values, before any resolution.
/// 3. the header record through [`resolve_command_secrets_record`], context
///    `` MCP server "{server}" HTTP header "{key}" ``.
/// 4. `commandBearer` — `definition.bearerToken` starting `!` but **not** `!!`, again raw.
/// 5. only when `auth === "bearer"`: the command bearer through [`resolve_command_secret`] with
///    context `` MCP server "{server}" HTTP bearer token ``, else
///    [`crate::credentials::resolve_bearer_token`]'s static ladder. Upstream's `if (token)` is a
///    **truthiness** test, so a command that could not run is an error (never `""`) and a
///    `bearerTokenEnv` pointing at an empty variable sets no header at all.
/// 6. the injection guard, run only when a command actually sourced one of these values.
///
/// # Why step 4 reads the raw `bearerToken` and step 5 can still call the static ladder
///
/// The two are not alternatives applied to the same string. `commandBearer` is upstream's test on
/// the **configured** value; the static ladder additionally consults `bearerTokenEnv` and unescapes
/// `!!x` through `interpolateSecretExpression`. A port that ran `is_command_secret` on the ladder's
/// *output* would execute a `!`-prefixed value that arrived from `bearerTokenEnv` — a shell command
/// smuggled in through an environment variable, which upstream never executes.
///
/// # The injection guard (step 6)
///
/// Upstream validates by constructing `new Headers(headers)` and converts the throw into
/// `` Failed to resolve MCP server "{server}" HTTP command secret: command returned an invalid
/// header value ``. Here that is `http::HeaderName`/`HeaderValue`'s `TryFrom`, which rejects the
/// same CR/LF and control bytes. Two things about it are load-bearing:
///
/// * It runs **only** when `hasCommandHeader || commandBearer`, exactly as upstream gates it. A
///   statically-configured bad header is *not* rejected here — it falls through to
///   [`crate::runtime::build_http_transport_config`], whose message does not falsely blame a
///   command.
/// * The bearer token is validated as the `Bearer {token}` **value** even though the port routes it
///   through rmcp's `auth_header` rather than the custom-header map. Upstream writes it into
///   `headers` before the guard, so a newline-bearing token fails pre-flight there; without this
///   arm it would instead fail inside `reqwest` at first request, long after the connect the user
///   was watching.
///
/// # Errors
///
/// [`McpError::Other`] with one of [`resolve_command_secret`]'s five sentences, or with the
/// invalid-header-value sentence above.
pub fn resolve_http_secrets(
    entry: &ServerEntry,
    server_name: &str,
    env: &EnvFn,
) -> McpResult<ResolvedHttpSecrets> {
    // Step 2 — the raw values, before resolution, so `!!x` (an escaped literal) never counts.
    let has_command_header = entry
        .headers
        .as_deref()
        .into_iter()
        .flatten()
        .any(|(_, value)| crate::credentials::is_command_secret(value));

    // Step 3.
    let mut headers: Vec<(String, String)> =
        resolve_command_secrets_record(entry.headers.as_deref(), |key| {
            format!("MCP server \"{server_name}\" HTTP header \"{key}\"")
        })?
        .unwrap_or_default()
        .into_iter()
        .collect::<Vec<(String, String)>>();

    // Step 4.
    let command_bearer = entry
        .bearer_token
        .as_deref()
        .filter(|value| crate::credentials::is_command_secret(value));

    // Step 5.
    let mut bearer_token = None;
    if entry.auth == Some(AuthMode::Named(AuthKind::Bearer)) {
        let token = match command_bearer {
            Some(marker) => Some(resolve_command_secret(
                marker,
                &format!("MCP server \"{server_name}\" HTTP bearer token"),
            )?),
            None => crate::credentials::resolve_bearer_token(
                entry.bearer_token.as_deref(),
                entry.bearer_token_env.as_deref(),
                env,
            ),
        };
        // `if (token) headers["Authorization"] = ...` — JavaScript truthiness, so `""` sets nothing.
        bearer_token = token.filter(|token| !token.is_empty());
    }

    // Upstream's line is an ASSIGNMENT into the already-resolved record — `headers["Authorization"]
    // = ...` (`server-manager.ts:855`) — so a literal `Authorization` the user configured is
    // OVERWRITTEN, not joined. cyrup carries the token out separately (rmcp applies it via
    // `bearer_auth`), so reproducing the overwrite means dropping any `Authorization` the header map
    // still holds. Without this the two go on the wire together — `bearer_auth` first, then
    // reqwest's `header()` APPENDS rather than replaces — and step 6 below would validate a header
    // set upstream had already discarded. Case-insensitive because HTTP field names are.
    if bearer_token.is_some() {
        headers.retain(|(name, _)| !name.eq_ignore_ascii_case("authorization"));
    }

    // Step 6.
    if has_command_header || command_bearer.is_some() {
        validate_command_sourced_headers(server_name, &headers, bearer_token.as_deref())?;
    }

    Ok(ResolvedHttpSecrets {
        headers,
        bearer_token,
    })
}

/// `new Headers(headers)` as an injection guard — see [`resolve_http_secrets`] step 6 for why it is
/// gated and why the bearer value is included.
fn validate_command_sourced_headers(
    server_name: &str,
    headers: &[(String, String)],
    bearer_token: Option<&str>,
) -> McpResult<()> {
    let invalid = || {
        McpError::other(format!(
            "Failed to resolve MCP server \"{server_name}\" HTTP command secret: command returned an invalid header value"
        ))
    };
    for (name, value) in headers {
        if http::HeaderName::try_from(name.as_str()).is_err()
            || http::HeaderValue::try_from(value.as_str()).is_err()
        {
            return Err(invalid());
        }
    }
    if let Some(token) = bearer_token
        && http::HeaderValue::try_from(format!("Bearer {token}")).is_err()
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn env_of(pairs: &[(&str, &str)]) -> EnvFn {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        Arc::new(move |name: &str| map.get(name).cloned())
    }

    fn record(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    fn http_entry(headers: &[(&str, &str)]) -> ServerEntry {
        ServerEntry {
            url: Some("https://example.test/mcp".to_string()),
            headers: Some(record(headers).into()),
            ..ServerEntry::default()
        }
    }

    // --- the record form (MCP-083 obligation 1) -------------------------------------------------

    #[test]
    fn the_record_form_resolves_every_value_and_names_the_failing_key() {
        let resolved = resolve_command_secrets_record(
            Some(&record(&[
                ("A", "!printf one"),
                ("B", "!!literal"),
                ("C", "plain"),
            ])),
            |key| format!("ctx \"{key}\""),
        )
        .unwrap()
        .unwrap();
        assert_eq!(resolved.get("A").map(String::as_str), Some("one"));
        assert_eq!(
            resolved.get("B").map(String::as_str),
            Some("!literal"),
            "`!!` consumes exactly one `!` and never spawns"
        );
        assert_eq!(resolved.get("C").map(String::as_str), Some("plain"));

        // `undefined` in, `undefined` out — the arm both upstream call sites `??`/`?:` against.
        assert!(
            resolve_command_secrets_record(None, |key| key.to_string())
                .unwrap()
                .is_none()
        );

        // The context string is built from the KEY, and the whole record fails closed.
        let err = resolve_command_secrets_record(Some(&record(&[("K", "!exit 3")])), |key| {
            format!("ctx \"{key}\"")
        })
        .unwrap_err()
        .to_string();
        assert_eq!(
            err,
            "Failed to resolve ctx \"K\": command exited with code 3"
        );
    }

    #[test]
    fn a_failing_key_abandons_the_whole_record_rather_than_emptying_one_value() {
        // `B` fails; nothing partially-resolved may escape, and `C` must not be handed an empty
        // string. Upstream's `map` throws out of the whole `Object.fromEntries`.
        let outcome = resolve_command_secrets_record(
            Some(&record(&[
                ("A", "!printf ok"),
                ("B", "!exit 1"),
                ("C", "!printf ok"),
            ])),
            |key| format!("ctx \"{key}\""),
        );
        assert!(outcome.is_err(), "an unresolvable secret is never a value");
    }

    // --- the `stdio env` context string (MCP-083 obligation 2) ----------------------------------

    #[test]
    fn resolve_env_layers_over_the_base_and_uses_the_stdio_env_context() {
        let base: HashMap<String, String> = [("HOST_ONLY".to_string(), "kept".to_string())]
            .into_iter()
            .collect();

        let resolved = resolve_env(
            Some(&record(&[("TOKEN", "!printf hunter2"), ("PLAIN", "x")])),
            "srv",
            false,
            &base,
        )
        .unwrap();
        assert_eq!(resolved.get("TOKEN").map(String::as_str), Some("hunter2"));
        assert_eq!(resolved.get("PLAIN").map(String::as_str), Some("x"));
        assert_eq!(
            resolved.get("HOST_ONLY").map(String::as_str),
            Some("kept"),
            "the full base environment is copied first"
        );

        let err = resolve_env(Some(&record(&[("TOKEN", "!exit 7")])), "srv", false, &base)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "Failed to resolve MCP server \"srv\" stdio env \"TOKEN\": command exited with code 7",
            "the third of the four context strings, at the site that resolves it"
        );
    }

    #[test]
    fn literal_env_takes_values_verbatim_and_spawns_nothing() {
        let base = HashMap::new();
        // `!exit 1` would fail loudly if it were executed, and `${HOME}` would be expanded.
        let resolved = resolve_env(
            Some(&record(&[("A", "!exit 1"), ("B", "${HOME}")])),
            "plugin",
            true,
            &base,
        )
        .unwrap();
        assert_eq!(resolved.get("A").map(String::as_str), Some("!exit 1"));
        assert_eq!(resolved.get("B").map(String::as_str), Some("${HOME}"));
    }

    // --- the two HTTP context strings (MCP-083 obligation 2) ------------------------------------

    #[test]
    fn a_failing_header_command_names_the_header_key() {
        let entry = http_entry(&[("X-Token", "!exit 4")]);
        let err = resolve_http_secrets(&entry, "srv", &env_of(&[]))
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "Failed to resolve MCP server \"srv\" HTTP header \"X-Token\": command exited with code 4"
        );
    }

    #[test]
    fn a_failing_bearer_command_uses_the_bearer_context_not_the_header_one() {
        let entry = ServerEntry {
            auth: Some(AuthMode::Named(AuthKind::Bearer)),
            bearer_token: Some("!exit 5".to_string()),
            ..http_entry(&[])
        };
        let err = resolve_http_secrets(&entry, "srv", &env_of(&[]))
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "Failed to resolve MCP server \"srv\" HTTP bearer token: command exited with code 5"
        );
    }

    #[test]
    fn the_bearer_ladder_is_only_consulted_when_auth_is_bearer() {
        let env = env_of(&[("TOK", "from-env")]);

        let mut entry = http_entry(&[]);
        entry.bearer_token_env = Some("TOK".to_string());
        assert_eq!(
            resolve_http_secrets(&entry, "srv", &env)
                .unwrap()
                .bearer_token,
            None,
            "`auth` absent means no Authorization header at all"
        );

        entry.auth = Some(AuthMode::Named(AuthKind::Bearer));
        assert_eq!(
            resolve_http_secrets(&entry, "srv", &env)
                .unwrap()
                .bearer_token
                .as_deref(),
            Some("from-env")
        );

        // `!!x` is an escaped literal on the static ladder: unescaped, never executed.
        entry.bearer_token = Some("!!literal".to_string());
        assert_eq!(
            resolve_http_secrets(&entry, "srv", &env)
                .unwrap()
                .bearer_token
                .as_deref(),
            Some("!literal")
        );

        // A `!`-prefixed value arriving through `bearerTokenEnv` is NOT a command — upstream tests
        // `definition.bearerToken`, not the ladder's output.
        entry.bearer_token = None;
        let hostile = env_of(&[("TOK", "!touch /nonexistent/cyrup-mcp-should-never-run")]);
        assert_eq!(
            resolve_http_secrets(&entry, "srv", &hostile)
                .unwrap()
                .bearer_token
                .as_deref(),
            Some("!touch /nonexistent/cyrup-mcp-should-never-run")
        );
    }

    #[test]
    fn a_command_sourced_header_carrying_a_newline_cannot_inject_a_second_header() {
        let entry = http_entry(&[("X-Token", "!printf 'a\\nX-Evil: 1'")]);
        let err = resolve_http_secrets(&entry, "srv", &env_of(&[]))
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "Failed to resolve MCP server \"srv\" HTTP command secret: command returned an invalid header value"
        );
    }

    #[test]
    fn a_command_sourced_bearer_carrying_a_newline_fails_the_same_guard() {
        let entry = ServerEntry {
            auth: Some(AuthMode::Named(AuthKind::Bearer)),
            bearer_token: Some("!printf 'a\\nX-Evil: 1'".to_string()),
            ..http_entry(&[])
        };
        let err = resolve_http_secrets(&entry, "srv", &env_of(&[]))
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "Failed to resolve MCP server \"srv\" HTTP command secret: command returned an invalid header value"
        );
    }

    #[test]
    fn a_statically_configured_bad_header_is_not_blamed_on_a_command() {
        // No `!` anywhere, so the guard does not run at all and the value survives this stage —
        // `build_http_transport_config` is what rejects it, with a message that names the header.
        let entry = http_entry(&[("X-A", "a\r\nX-Evil: 1")]);
        let resolved = resolve_http_secrets(&entry, "srv", &env_of(&[])).unwrap();
        assert_eq!(resolved.headers.len(), 1);
    }

    // --- the non-executing record form (conformance C6) -----------------------------------------

    #[test]
    fn interpolate_env_record_unescapes_but_never_executes() {
        let env = env_of(&[("HOME", "/home/u")]);
        let resolved = interpolate_env_record(
            Some(&record(&[
                ("CMD", "!touch /nonexistent/cyrup-mcp-should-never-run"),
                ("ESCAPED", "!!${HOME}"),
                ("PLAIN", "${HOME}/x"),
            ])),
            &env,
        )
        .unwrap();
        assert_eq!(
            resolved.get("CMD").map(String::as_str),
            Some("!touch /nonexistent/cyrup-mcp-should-never-run"),
            "a command marker survives hashing VERBATIM and unexecuted"
        );
        assert_eq!(
            resolved.get("ESCAPED").map(String::as_str),
            Some("!/home/u")
        );
        assert_eq!(resolved.get("PLAIN").map(String::as_str), Some("/home/u/x"));
        assert!(interpolate_env_record(None, &env).is_none());
    }

    /// Upstream ASSIGNS `headers["Authorization"]` (`server-manager.ts:855`), so a literal
    /// `Authorization` the user configured is replaced by the resolved bearer, never joined to it.
    ///
    /// cyrup carries the token out on [`ResolvedHttpSecrets::bearer_token`] for rmcp's `bearer_auth`
    /// to apply, so the equivalent of upstream's overwrite is REMOVING the configured header. Before
    /// that landed both went on the wire — `bearer_auth` first, then reqwest's `header()` appends —
    /// which is two `Authorization` headers on a request carrying credentials.
    #[test]
    fn a_resolved_bearer_evicts_a_user_configured_authorization_header() {
        let entry: ServerEntry = serde_json::from_str(
            r#"{
                "url": "https://api.example.com/mcp",
                "auth": "bearer",
                "bearerToken": "tok",
                "headers": { "Authorization": "Bearer stale", "X-Keep": "yes" }
            }"#,
        )
        .expect("entry");
        let env: EnvFn = std::sync::Arc::new(|_: &str| None);
        let resolved = resolve_http_secrets(&entry, "s", &env).expect("resolves");

        assert_eq!(resolved.bearer_token.as_deref(), Some("tok"));
        assert!(
            !resolved
                .headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("authorization")),
            "the configured Authorization must be evicted, got {:?}",
            resolved.headers
        );
        // Eviction is surgical: every other configured header survives untouched.
        assert_eq!(
            resolved.headers,
            vec![("X-Keep".to_string(), "yes".to_string())]
        );
    }

    /// The eviction is conditional on a token actually resolving. `auth: "bearer"` with an empty
    /// token is JavaScript-falsy upstream, so the assignment never runs and the user's own
    /// `Authorization` stands — dropping it here would silently unauthenticate the request.
    #[test]
    fn no_resolved_bearer_leaves_a_configured_authorization_alone() {
        let entry: ServerEntry = serde_json::from_str(
            r#"{
                "url": "https://api.example.com/mcp",
                "auth": "bearer",
                "bearerToken": "",
                "headers": { "Authorization": "Basic keepme" }
            }"#,
        )
        .expect("entry");
        let env: EnvFn = std::sync::Arc::new(|_: &str| None);
        let resolved = resolve_http_secrets(&entry, "s", &env).expect("resolves");

        assert_eq!(resolved.bearer_token, None, "empty token is falsy upstream");
        assert_eq!(
            resolved.headers,
            vec![("Authorization".to_string(), "Basic keepme".to_string())]
        );
    }
}
