//! `cyrup auth print-api-key` / `cyrup auth print-bearer-token` — the credential-print surface
//! external clients script against (arch-11 §3.7).
//!
//! A 1:1 port of Pi `coding-agent/src/cli/credential-print.ts` (v0.83.0, 152 lines) plus its
//! driver `runCredentialPrintCommand` (main.ts:130-167), dispatched — like Pi's — BEFORE ordinary
//! argument parsing (main.ts:557-559, right after the package/config subcommand block). Before this
//! module existed, `auth` was not in `crate::subcommands::SUBCOMMANDS`, so
//! `cyrup auth print-api-key --provider openai --model gpt-5.5` fell through arg leniency as two
//! bare positionals, became a chat PROMPT, and started an agent session: no credential on stdout,
//! no error, exit 0, tokens burned and a session file written. Any script following the upstream
//! contract (`KEY=$(cyrup auth print-api-key …)`) captured assistant prose.
//!
//! The contract, verbatim from upstream: the credential **alone** on stdout with a trailing
//! newline; every failure is `Error: <message>` on stderr with exit code 1.
//!
//! **[CYRUP-DELTA] — what `print-bearer-token` can actually return.** The command is ported in
//! full and reaches the same resolution path, but cyrup registers no OAuth provider strategy
//! ([`cyrup_provider::ProviderAuth::oauth`] is `None` for every built-in — cyrup implements no OAuth
//! *login* flows by design, see CLAUDE.md "Known divergences → Auth"). So a stored `oauth`
//! credential has no strategy to mint a request token from and resolution yields nothing, which
//! surfaces as upstream's own `No usable OAuth bearer token is configured`. The parse, validation,
//! `--min-expiry` handling, provider/model resolution and error taxonomy are all live; only the
//! final token materialisation waits on the OAuth provider registry. `print-api-key` is fully
//! functional.

use std::collections::BTreeMap;
use std::sync::Arc;

use clap::Parser;
use cyrup_config::{AuthStore, ConfigDirs};
use cyrup_provider::{
    AuthOverrides, CredentialStore, CredentialType, InMemoryCredentialStore, Model,
};

use crate::cli::{Cli, partition_extension_flags};
use crate::diagnostics::apply_arg_leniency;

/// Pi `DEFAULT_BEARER_TOKEN_MIN_EXPIRY_MS = 30 * 60_000` (credential-print.ts:8).
const DEFAULT_BEARER_TOKEN_MIN_EXPIRY_MS: i64 = 30 * 60_000;

/// Which auth command was invoked — Pi `AuthCommandKind = "check" | "api_key" | "bearer_token"`
/// (`cli/auth-command.ts:4` @v0.84.1; the v0.83.0 file was `CredentialPrintKind` with the two print
/// kinds only, `credential-print.ts:6`). SEAM-050.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialPrintKind {
    /// `auth check` — new at v0.84.1. Reports `ready` / `not_ready` / `invalid` and exits 0/1/2.
    Check,
    ApiKey,
    BearerToken,
}

impl CredentialPrintKind {
    /// The noun upstream uses in `No usable <…> is configured` (credential-print.ts:145-147).
    fn noun(self) -> &'static str {
        match self {
            CredentialPrintKind::ApiKey => "API key",
            CredentialPrintKind::BearerToken => "OAuth bearer token",
            CredentialPrintKind::Check => "credential",
        }
    }

    /// Pi `getAuthCommandName` (`auth-command.ts:23-25`) — the name quoted in
    /// `Unknown option --X for "<name>".`
    pub const fn command_name(self) -> &'static str {
        match self {
            CredentialPrintKind::Check => "auth check",
            CredentialPrintKind::ApiKey => "auth print-api-key",
            CredentialPrintKind::BearerToken => "auth print-bearer-token",
        }
    }

    /// Pi `AUTH_COMMAND_USAGE` (`auth-command.ts:17-21`), rebranded.
    pub const fn usage(self) -> &'static str {
        match self {
            CredentialPrintKind::Check => {
                "cyrup auth check --provider <provider> [--json] [--credentials] [--no-refresh]"
            }
            CredentialPrintKind::ApiKey => {
                "cyrup auth print-api-key --provider <provider> [--model <model>]"
            }
            CredentialPrintKind::BearerToken => {
                "cyrup auth print-bearer-token --provider <provider> [--model <model>] \
                 [--min-expiry <duration>]"
            }
        }
    }
}

/// The `auth check` verdict — Pi `AuthCheckResult` (`cli/auth-check.ts:15-20` @v0.84.1). Field names
/// are pi's, because `--json` serializes this object verbatim (`main.ts:207`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AuthCheckResult {
    /// `"ready" | "not_ready" | "invalid"` (`auth-check.ts:8`) — drives the 0/1/2 exit code.
    pub status: &'static str,
    pub provider: String,
    /// `"provider_not_found" | "credentials_not_configured" | "credential_not_available" |
    /// "invalid_state"` (`auth-check.ts:9-13`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    /// `"api_key" | "oauth"` (`auth-check.ts:19`), present only on `ready`.
    #[serde(rename = "authType", skip_serializing_if = "Option::is_none")]
    pub auth_type: Option<&'static str>,
    /// Pi splices this in beside the result rather than declaring it on the interface:
    /// `JSON.stringify({ ...result, ...(credential ? { credentials: credential } : {}) })`
    /// (`main.ts:207`), so it is elided when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credentials: Option<String>,
}

impl AuthCheckResult {
    /// Pi `process.exitCode = result.status === "ready" ? 0 : result.status === "not_ready" ? 1 : 2`
    /// (`main.ts:208`).
    pub fn exit_code(&self) -> i32 {
        match self.status {
            "ready" => 0,
            "not_ready" => 1,
            _ => 2,
        }
    }
}

/// A parsed `auth <command>` invocation (Pi `CredentialPrintCommand`, credential-print.ts:10-14).
/// `args` is the residual argv the ordinary CLI parser then sees, with `--min-expiry <value>`
/// already consumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialPrintCommand {
    pub kind: CredentialPrintKind,
    pub args: Vec<String>,
    pub min_expiry_ms: Option<i64>,
    /// `--json`, `--credentials`, `--no-refresh` — accepted **only** by `check`
    /// (`auth-command.ts:82-88` @v0.84.1). SEAM-050.
    pub json: bool,
    pub credentials: bool,
    pub no_refresh: bool,
}

/// A credential-print failure (Pi `CredentialPrintError` vs. any other thrown error,
/// credential-print.ts:16 + main.ts:163-165). [`Self::Opaque`] is upstream's catch-all: a non
/// `CredentialPrintError` throw is reported as `Failed to resolve credential`, never as the
/// underlying error, so a provider/storage fault cannot leak into a scripted stdout contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialPrintError {
    Message(String),
    Opaque,
}

impl CredentialPrintError {
    fn msg(text: impl Into<String>) -> Self {
        CredentialPrintError::Message(text.into())
    }

    /// The text that follows `Error: ` on stderr.
    pub fn message(&self) -> String {
        match self {
            CredentialPrintError::Message(m) => m.clone(),
            CredentialPrintError::Opaque => "Failed to resolve credential".to_string(),
        }
    }
}

/// Whether `argv` is a bare `auth` / `auth help` / `auth --help` / `auth -h` (Pi
/// `isCredentialPrintHelp`, credential-print.ts:18-22).
pub fn is_credential_print_help(argv: &[String]) -> bool {
    if argv.first().map(String::as_str) != Some("auth") {
        return false;
    }
    matches!(
        argv.get(1).map(String::as_str),
        None | Some("help") | Some("--help") | Some("-h")
    )
}

/// The `auth` usage block (Pi `printAuthCommandHelp`, `cli/auth-command.ts:38-45` @v0.84.1 — the
/// v0.83.0 `printCredentialPrintHelp`, `credential-print.ts:24-30`, listed only the two print
/// verbs), rebranded. SEAM-050 added the `check` line and pi's provider-OR-model sentence.
pub fn credential_print_help() -> String {
    "Usage:\n  cyrup auth print-api-key [--provider <provider>] [--model <model>]\n  cyrup auth \
     print-bearer-token [--provider <provider>] [--model <model>] [--min-expiry <duration>]\n  \
     cyrup auth check [--provider <provider>] [--model <model>] [--json] [--credentials] \
     [--no-refresh]\n\nAuth commands require at least one of --provider or --model. Checks refresh \
     expired OAuth credentials by default; --no-refresh prevents this. --credentials emits the \
     credential, or includes it in JSON output.\n"
        .to_string()
}

/// Parse `duration` against Pi's `/^(\d+)(ms|s|m|h)$/iu` (credential-print.ts:53), returning
/// milliseconds. `None` when the value does not match.
fn parse_min_expiry(value: &str) -> Option<i64> {
    let split = value
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(i, _)| i)?;
    let (digits, unit) = value.split_at(split);
    if digits.is_empty() {
        return None;
    }
    let amount: i64 = digits.parse().ok()?;
    let unit = unit.to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => return None,
    };
    amount.checked_mul(multiplier)
}

/// Parse the `cyrup auth` command surface before normal startup (Pi
/// `parseCredentialPrintCommand`, credential-print.ts:33-64).
///
/// `Ok(None)` means "not an auth command — fall through to the ordinary CLI". `Err` carries the
/// message upstream throws as a `CredentialPrintError`.
pub fn parse_credential_print_command(
    argv: &[String],
) -> Result<Option<CredentialPrintCommand>, String> {
    if argv.first().map(String::as_str) != Some("auth") {
        return Ok(None);
    }
    let kind = match argv.get(1).map(String::as_str) {
        Some("check") => CredentialPrintKind::Check,
        Some("print-api-key") => CredentialPrintKind::ApiKey,
        Some("print-bearer-token") => CredentialPrintKind::BearerToken,
        other => {
            // Pi `auth-command.ts:60-62` @v0.84.1 — the sentence gained the third verb.
            return Err(format!(
                "Unknown auth command \"{}\". Use \"cyrup auth print-api-key\", \"cyrup auth \
                 print-bearer-token\", or \"cyrup auth check\".",
                other.unwrap_or("")
            ));
        }
    };

    let mut args: Vec<String> = Vec::new();
    let mut min_expiry_ms: Option<i64> = None;
    let mut json = false;
    let mut credentials = false;
    let mut no_refresh = false;
    let mut index = 2usize;
    while let Some(arg) = argv.get(index) {
        // Pi `auth-command.ts:82-88` @v0.84.1: the three check-only flags are consumed here and
        // rejected outright for the print kinds, with the flag itself in the message.
        if matches!(arg.as_str(), "--json" | "--credentials" | "--no-refresh") {
            if kind != CredentialPrintKind::Check {
                return Err(format!("{arg} is only supported by auth check"));
            }
            match arg.as_str() {
                "--json" => json = true,
                "--credentials" => credentials = true,
                _ => no_refresh = true,
            }
            index += 1;
            continue;
        }
        if arg != "--min-expiry" {
            args.push(arg.clone());
            index += 1;
            continue;
        }
        if kind != CredentialPrintKind::BearerToken {
            return Err("--min-expiry is only supported by print-bearer-token".to_string());
        }
        // Pi consumes `args[++index]` unconditionally, so a trailing `--min-expiry` sees `undefined`.
        index += 1;
        let value = argv.get(index).map(String::as_str).unwrap_or("");
        let Some(ms) = parse_min_expiry(value) else {
            return Err("--min-expiry must use a duration such as 30m or 1h".to_string());
        };
        min_expiry_ms = Some(ms);
        index += 1;
    }

    Ok(Some(CredentialPrintCommand {
        kind,
        args,
        min_expiry_ms,
        json,
        credentials,
        no_refresh,
    }))
}

/// Reject anything outside `--provider` / `--model` — Pi `validateAuthCommandArgs`
/// (`cli/auth-command.ts:96-116` @v0.84.1; the v0.83.0 `validateCredentialPrintArgs`,
/// `credential-print.ts:66-76`). `positionals` is cyrup's combined carrier for Pi's `messages` +
/// `fileArgs`; `extension_flags` is Pi's `unknownFlags`.
///
/// **[CYRUP-DELTA] (SEAM-108) — this whole validation surface is `@v0.84.1`'s, not the ported tag's,
/// and that is deliberate.** cyrup ports pi at **v0.83.0**; `SEAM-050` closed by landing v0.84.1's
/// `auth` command tree wholesale, so THREE things here disagree with `credential-print.ts` at the
/// baseline a later fidelity pass would read: the required-argument rule, the verb COUNT (three —
/// `print-api-key`, `print-bearer-token`, `check` (`:42-47` above) — where v0.83.0's
/// `printCredentialPrintHelp` (`credential-print.ts:24-30`) shows two), and the two error sentences
/// below. Under v0.83.0, `cyrup auth print-api-key --provider openai` would be REJECTED with
/// `Credential printing requires --model <model>` (`credential-print.ts:67-68`); it succeeds here.
/// This is a forward-port, not a defect — **do not "restore" v0.83.0's shape** without an owner
/// decision that also reverts `SEAM-050`.
///
/// SEAM-050 changed three things to match v0.84.1, which routes BOTH print verbs through this same
/// function (`credential-print.ts:24` calls `validateAuthCommandArgs(args, kind)`):
/// * an unmatched flag is now `Unknown option --X for "auth print-api-key".` (`:99-102`) rather than
///   folded into the generic "only accepts" message, and it is checked FIRST, as pi checks it;
/// * the requirement is `--provider` **or** `--model` (`:113-115`), not `--model` alone — v0.83.0's
///   `credential-print.ts:67-68` required `--model`, so `cyrup auth print-api-key --provider openai`
///   was rejected where pi v0.84.1 accepts it;
/// * `check` gets its own sentence, `Auth checks require …` (`:108-110`).
pub fn validate_credential_print_args(
    cli: &Cli,
    kind: CredentialPrintKind,
) -> Result<(), CredentialPrintError> {
    // Pi `:97-102` — the unknown-flag rejection comes first and names the command.
    if let Some(flag) = cli.extension_flags.first() {
        return Err(CredentialPrintError::msg(format!(
            "Unknown option --{} for \"{}\".",
            flag.name,
            kind.command_name()
        )));
    }
    // Pi `:103-105` — `apiKey !== undefined || messages.length > 0 || fileArgs.length > 0`.
    if cli.api_key.is_some() || !cli.positionals.is_empty() {
        return Err(CredentialPrintError::msg(
            "Auth commands only accept --provider and --model",
        ));
    }
    let provider = cli
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let model = cli
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    if provider.is_none() && model.is_none() {
        return Err(CredentialPrintError::msg(match kind {
            // Pi `:108-110`.
            CredentialPrintKind::Check => {
                "Auth checks require --provider <provider> or --model <model>"
            }
            // Pi `:113-115`.
            _ => "Credential printing requires --provider <provider> or --model <model>",
        }));
    }
    Ok(())
}

/// Lift an `auth.json` credential into the provider-side shape the request path resolves against.
/// The two types are the same on-disk contract declared in two crates (`cyrup-config` owns the
/// file, `cyrup-provider` owns the resolution).
fn to_provider_credential(cred: cyrup_config::Credential) -> cyrup_provider::Credential {
    match cred {
        cyrup_config::Credential::ApiKey { key, env } => {
            cyrup_provider::Credential::ApiKey { key, env }
        }
        cyrup_config::Credential::Oauth {
            refresh,
            access,
            expires,
            ext,
        } => cyrup_provider::Credential::Oauth {
            refresh,
            access,
            expires,
            ext,
        },
    }
}

/// Extract the token from an `Authorization: Bearer <token>` header value (Pi
/// `/^Bearer\s+(.+)$/iu`, credential-print.ts:127).
fn strip_bearer(value: &str) -> Option<String> {
    value
        .get(..6)
        .filter(|p| p.eq_ignore_ascii_case("Bearer"))?;
    let tail = value.get(6..)?;
    if !tail.starts_with(char::is_whitespace) {
        return None;
    }
    let token = tail.trim_start();
    (!token.is_empty()).then(|| token.to_string())
}

/// Resolve one request credential for a specific provider/model pair (Pi
/// `resolveCredentialForPrint`, credential-print.ts:84-152).
///
/// Runs through the ordinary request-auth path ([`cyrup_provider::Models::get_auth_with`]) rather
/// than reading `auth.json` directly, exactly as upstream deliberately routes through
/// `ModelRuntime.getAuth()`: the value printed is the value a request would actually send, env
/// fallbacks and `models.json`-configured keys included.
pub async fn resolve_credential_for_print(
    cli: &Cli,
    dirs: &ConfigDirs,
    kind: CredentialPrintKind,
    min_expiry_ms: Option<i64>,
) -> Result<String, CredentialPrintError> {
    validate_credential_print_args(cli, kind)?;
    let requested_model = cli.model.as_deref().unwrap_or_default();

    // Seed the provider-side store from `auth.json` so `get_auth_with` resolves exactly what a
    // session would. `list_credentials()` is Pi's `listCredentials()` (model-runtime.ts:424 →
    // runtime-credentials.ts:29): the composed `{ providerId, type }` view, secrets untouched.
    let auth = AuthStore::at(dirs.agent_dir.join("auth.json"));
    let seed = InMemoryCredentialStore::new();
    for info in auth.list_credentials().unwrap_or_default() {
        if let Ok(Some(cred)) = auth.read(&info.provider).await {
            seed.insert(info.provider.clone(), to_provider_credential(cred));
        }
    }
    let store = Arc::new(seed) as Arc<dyn CredentialStore>;
    // `const credentialTypes = new Map((await modelRuntime.listCredentials()).map(...))`
    // (credential-print.ts:92-94). Read back off the SAME store the request path resolves against,
    // through `CredentialStore::list()` (ai/src/auth/types.ts:71) — so the kind that decides which
    // providers this export skips can never disagree with the credential that will be resolved.
    let credential_types: BTreeMap<String, CredentialType> = store
        .list()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|info| (info.provider.as_str().to_string(), info.credential_type))
        .collect();

    let (models_json, _load_errors) =
        cyrup_config::load_models_file_reporting(&dirs.agent_dir.join("models.json"));
    let registry = crate::provider::registry_with_credentials(&models_json, store);
    let all = registry.get_models(None);
    let has_configured_auth =
        |m: &Model| cyrup_config::provider_is_configured(&auth, &models_json, &m.provider, None);

    // Pi's `--provider` value is the TRIMMED one `validateAuthCommandArgs` returns
    // (`auth-command.ts:97-98` @v0.84.1: `args.provider?.trim() || undefined`), and
    // `resolveCredentialForPrint` destructures exactly that (`credential-print.ts:24`) — so a
    // `--provider " acme "` must resolve, not miss.
    let explicit_provider = cli
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());

    // Pi's two branches: an explicit `--provider` names ONE provider; otherwise every provider that
    // HAS a stored credential is tried and ambiguity is reported.
    let mut models: Vec<Model> = Vec::new();
    if let Some(provider) = explicit_provider {
        // Pi `credential-print.ts:29-32` @v0.84.1 — the provider must EXIST first, and its absence
        // is its own message. cyrup had no such check: an unknown `--provider` fell into
        // `resolve_cli_model` and came back as the generic "Unable to resolve the requested
        // provider/model", so `--list-models` was never suggested.
        if !registry
            .get_providers()
            .iter()
            .any(|p| p.id().as_str() == provider)
        {
            return Err(CredentialPrintError::msg(format!(
                "Unknown provider \"{provider}\". Use --list-models to see available providers."
            )));
        }
        match cli
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
        {
            // Pi `:33-38` — a `--model` alongside `--provider` must resolve, and a failure is fatal.
            Some(model_pattern) => {
                let resolved = cyrup_config::resolve_cli_model(
                    Some(provider),
                    Some(model_pattern),
                    None,
                    &all,
                    &has_configured_auth,
                );
                match (resolved.error, resolved.model) {
                    (Some(error), _) => return Err(CredentialPrintError::msg(error)),
                    (None, None) => {
                        return Err(CredentialPrintError::msg(
                            "Unable to resolve the requested provider/model",
                        ));
                    }
                    (None, Some(model)) => models.push(model),
                }
            }
            // Pi `:39-40` — `providers.push({ id: provider.id })`: the provider is exported with NO
            // model, and the credential is fetched with `getAuth(provider.id, …)` (`:66`).
            //
            // THIS BRANCH DID NOT EXIST. cyrup passed `cli.model` (here `None`) straight into
            // `resolve_cli_model` and turned its empty result into a hard "Unable to resolve the
            // requested provider/model" — so `cyrup auth print-api-key --provider acme` errored
            // where pi prints the credential. SEAM-050 had already relaxed
            // `validate_credential_print_args` to pi's provider-OR-model rule (see its doc comment,
            // which names this exact invocation), but the RESOLVER was never given the matching
            // branch, so the rejection simply moved from the validator to here and the fix read as
            // complete. Found by running `cyrup-it`'s `bin` target, which is `required-features =
            // ["it"]` and therefore never runs in the merge gate.
            //
            // cyrup resolves auth through a `&Model`, so a provider with no model needs a
            // representative one — the same stand-in `run_auth_check` already uses for pi's
            // `getAuth(provider)` (see its tiers 6-7). A provider with an empty catalog leaves
            // `models` empty and falls through to the no-usable-credential branch below, which
            // still names the provider via `explicit_provider`.
            None => {
                if let Some(model) = all.iter().find(|m| m.provider.as_str() == provider) {
                    models.push(model.clone());
                }
            }
        }
    } else {
        for provider in registry.get_providers() {
            let id = provider.id().as_str().to_string();
            if !credential_types.contains_key(&id) {
                continue;
            }
            let resolved = cyrup_config::resolve_cli_model(
                Some(&id),
                cli.model.as_deref(),
                None,
                &all,
                &has_configured_auth,
            );
            // A "Using custom model id" warning means the pattern did not really match this
            // provider's catalog — upstream excludes those from the inference set (:104).
            let custom_id = resolved
                .warning
                .as_deref()
                .is_some_and(|w| w.contains("Using custom model id"));
            if let Some(model) = resolved.model
                && resolved.error.is_none()
                && !custom_id
            {
                models.push(model);
            }
        }
        if models.is_empty() {
            return Err(CredentialPrintError::msg(format!(
                "Model \"{requested_model}\" not found. Use --list-models to see available models."
            )));
        }
    }

    let mut credentials: Vec<(String, String)> = Vec::new();
    for model in &models {
        let stored = credential_types.get(model.provider.as_str()).copied();
        // An OAuth-only provider has no API key to print, and an API-key provider has no bearer
        // token — each kind skips the other's providers (:113-114).
        if kind == CredentialPrintKind::ApiKey && stored == Some(CredentialType::Oauth) {
            continue;
        }
        if kind == CredentialPrintKind::BearerToken && stored != Some(CredentialType::Oauth) {
            continue;
        }
        let overrides = AuthOverrides {
            api_key: None,
            env: None,
            // Only the bearer-token export widens the OAuth refresh window and takes on the
            // post-refresh contract (:118-120); the API-key branch passes no overrides at all.
            min_oauth_validity_ms: match kind {
                CredentialPrintKind::BearerToken => {
                    Some(min_expiry_ms.unwrap_or(DEFAULT_BEARER_TOKEN_MIN_EXPIRY_MS))
                }
                // `Check` never reaches this function — `dispatch` routes it to `run_auth_check`
                // before the print path, exactly as pi's `if (command.kind !== "check")` guard does
                // (`main.ts:171`).
                CredentialPrintKind::ApiKey | CredentialPrintKind::Check => None,
            },
        };
        // Pi lets a non-`CredentialPrintError` throw here reach the outer catch, which reports the
        // generic message — a storage/OAuth fault must not leak into the stdout contract.
        let resolved = registry
            .get_auth_with(model, overrides)
            .await
            .map_err(|_| CredentialPrintError::Opaque)?;
        let api_key = resolved.as_ref().and_then(|r| r.auth.api_key.clone());
        let bearer = resolved
            .as_ref()
            .and_then(|r| r.auth.headers.as_ref())
            .and_then(|headers| {
                headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                    .and_then(|(_, value)| value.as_deref())
            })
            .and_then(strip_bearer);
        let value = match kind {
            CredentialPrintKind::BearerToken => api_key.or(bearer),
            CredentialPrintKind::ApiKey | CredentialPrintKind::Check => api_key,
        };
        if let Some(value) = value {
            credentials.push((model.provider.as_str().to_string(), value));
        }
    }

    if let [(_, value)] = credentials.as_slice() {
        return Ok(value.clone());
    }
    if credentials.is_empty() {
        // Pi `providers[0]?.id` (`credential-print.ts:73`). In the model-less `--provider` branch
        // pi's `providers[0]` is `{ id: provider.id }`, so the id is present even with no model —
        // `explicit_provider` is what keeps the two typed messages below reachable when the named
        // provider has an empty catalog and `models` is therefore empty.
        let provider_id = models
            .first()
            .map(|m| m.provider.as_str().to_string())
            .or_else(|| explicit_provider.map(str::to_string));
        let stored = provider_id
            .as_deref()
            .and_then(|p| credential_types.get(p).copied());
        if let Some(provider_id) = provider_id.filter(|_| explicit_provider.is_some()) {
            if kind == CredentialPrintKind::ApiKey && stored == Some(CredentialType::Oauth) {
                return Err(CredentialPrintError::msg(format!(
                    "Provider \"{provider_id}\" is configured with OAuth, not an API key"
                )));
            }
            if kind == CredentialPrintKind::BearerToken && stored != Some(CredentialType::Oauth) {
                return Err(CredentialPrintError::msg(format!(
                    "Provider \"{provider_id}\" is not configured with an OAuth bearer token"
                )));
            }
        }
        return Err(CredentialPrintError::msg(format!(
            "No usable {} is configured",
            kind.noun()
        )));
    }
    let ids: Vec<&str> = credentials.iter().map(|(id, _)| id.as_str()).collect();
    Err(CredentialPrintError::msg(format!(
        "Model \"{requested_model}\" has multiple configured providers ({}). Specify --provider.",
        ids.join(", ")
    )))
}

/// `cyrup auth check` — Pi `checkProviderAuth` (`cli/auth-check.ts:22-52` @v0.84.1) plus the
/// `--credentials` splice its driver applies (`main.ts:190-199`). SEAM-050.
///
/// pi's tiers, in pi's order:
/// 1. `validateAuthCommandArgs(args, "check")` (`:26`) — done by the caller;
/// 2. an explicit `--model` resolves the provider through `resolveCliModel`, and a resolution
///    failure is a THROWN `AuthCommandError`, not a `not_ready` (`:28-34`);
/// 3. `modelRuntime.getError()` ⇒ `invalid` / `invalid_state` (`:37-39`) — cyrup's analog is a
///    `models.json` composition error;
/// 4. `!modelRuntime.getProvider(provider)` ⇒ `not_ready` / `provider_not_found` (`:40-42`);
/// 5. `!(await modelRuntime.checkAuth(provider))` ⇒ `not_ready` / `credentials_not_configured`
///    (`:44-45`);
/// 6. with refresh on, `!(await modelRuntime.getAuth(provider))` ⇒ the same (`:46-48`);
/// 7. otherwise `ready`, carrying `authType` (`:49`).
///
/// CYRUP-DELTA — the `--no-refresh` credential source. pi swaps the whole store for a
/// `ReadOnlyAuthStorage` (`main.ts:186`) so an expired OAuth credential cannot be silently
/// refreshed on disk; cyrup's equivalent is that `--no-refresh` skips tier 6 and reads the STORED
/// `access` token directly (pi's `getProviderCredential` early-return, `auth-check.ts:60`), because
/// cyrup's credential-print path already seeds an in-memory store from `auth.json` and never writes
/// back — the read-only property pi's wrapper exists to guarantee holds by construction here.
pub async fn run_auth_check(
    cli: &Cli,
    dirs: &ConfigDirs,
    want_credentials: bool,
    refresh: bool,
) -> Result<AuthCheckResult, CredentialPrintError> {
    validate_credential_print_args(cli, CredentialPrintKind::Check)?;

    let auth = AuthStore::at(dirs.agent_dir.join("auth.json"));
    let seed = InMemoryCredentialStore::new();
    for info in auth.list_credentials().unwrap_or_default() {
        if let Ok(Some(cred)) = auth.read(&info.provider).await {
            seed.insert(info.provider.clone(), to_provider_credential(cred));
        }
    }
    let store = Arc::new(seed) as Arc<dyn CredentialStore>;
    let credential_types: BTreeMap<String, CredentialType> = store
        .list()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|info| (info.provider.as_str().to_string(), info.credential_type))
        .collect();

    let (models_json, load_errors) =
        cyrup_config::load_models_file_reporting(&dirs.agent_dir.join("models.json"));
    let registry = crate::provider::registry_with_credentials(&models_json, store);
    let all = registry.get_models(None);
    let has_configured_auth =
        |m: &Model| cyrup_config::provider_is_configured(&auth, &models_json, &m.provider, None);

    // Tier 2 — an explicit `--model` names the provider (`auth-check.ts:28-34`).
    let mut provider = cli
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    if let Some(model_pattern) = cli
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        let resolved = cyrup_config::resolve_cli_model(
            cli.provider.as_deref(),
            Some(model_pattern),
            None,
            &all,
            &has_configured_auth,
        );
        match (resolved.error, resolved.model) {
            (Some(error), _) => return Err(CredentialPrintError::msg(error)),
            (None, None) => {
                return Err(CredentialPrintError::msg(format!(
                    "Unable to resolve model \"{model_pattern}\""
                )));
            }
            (None, Some(model)) => provider = Some(model.provider.as_str().to_string()),
        }
    }
    // Pi `:35` — `if (!provider) throw new AuthCommandError("Unable to resolve an auth provider");`
    let Some(provider) = provider else {
        return Err(CredentialPrintError::msg(
            "Unable to resolve an auth provider",
        ));
    };

    // Tier 3 — `modelRuntime.getError()` (`:37-39`).
    if load_errors.is_some() {
        return Ok(AuthCheckResult {
            status: "invalid",
            provider,
            reason: Some("invalid_state"),
            auth_type: None,
            credentials: None,
        });
    }
    // Tier 4 — `modelRuntime.getProvider(provider)` (`:40-42`).
    if !registry
        .get_providers()
        .iter()
        .any(|p| p.id().as_str() == provider)
    {
        return Ok(AuthCheckResult {
            status: "not_ready",
            provider,
            reason: Some("provider_not_found"),
            auth_type: None,
            credentials: None,
        });
    }
    // Tier 5 — `modelRuntime.checkAuth(provider)` (`:44-45`). cyrup's predicate is the same one the
    // launch path and `--list-models` use: a stored credential, a known provider env var, or a
    // `models.json` block carrying its own `apiKey`.
    let configured = cyrup_config::provider_is_configured(
        &auth,
        &models_json,
        &cyrup_sdk::core::ProviderId::from(provider.as_str()),
        None,
    );
    if !configured {
        return Ok(AuthCheckResult {
            status: "not_ready",
            provider,
            reason: Some("credentials_not_configured"),
            auth_type: None,
            credentials: None,
        });
    }
    let stored = credential_types.get(&provider).copied();
    let auth_type = match stored {
        Some(CredentialType::Oauth) => "oauth",
        _ => "api_key",
    };

    // Tiers 6-7, plus the driver's `--credentials` splice. Both need the RESOLVED request auth, so
    // they share one call: pi runs `getAuth(provider)` for the refresh probe (`:46-48`) and again
    // inside `getProviderCredential` (`auth-check.ts:62`).
    let model = all.iter().find(|m| m.provider.as_str() == provider);
    let resolved = match model {
        Some(model) => registry
            .get_auth_with(
                model,
                AuthOverrides {
                    api_key: None,
                    env: None,
                    min_oauth_validity_ms: None,
                },
            )
            .await
            .map_err(|_| CredentialPrintError::Opaque)?,
        None => None,
    };
    if refresh && resolved.is_none() {
        return Ok(AuthCheckResult {
            status: "not_ready",
            provider,
            reason: Some("credentials_not_configured"),
            auth_type: None,
            credentials: None,
        });
    }

    let mut result = AuthCheckResult {
        status: "ready",
        provider,
        reason: None,
        auth_type: Some(auth_type),
        credentials: None,
    };
    if want_credentials {
        // Pi `getProviderCredential` (`auth-check.ts:54-63`): without refresh an OAuth credential
        // answers with its STORED `access` token; otherwise the resolved request auth's api key,
        // else the `Bearer` half of its `Authorization` header (`getAuthCredential`, `:118-125`).
        let credential = if !refresh && stored == Some(CredentialType::Oauth) {
            match auth
                .read(&cyrup_sdk::core::ProviderId::from(result.provider.as_str()))
                .await
            {
                Ok(Some(cyrup_config::Credential::Oauth { access, .. })) => Some(access),
                _ => None,
            }
        } else {
            let api_key = resolved.as_ref().and_then(|r| r.auth.api_key.clone());
            api_key.or_else(|| {
                resolved
                    .as_ref()
                    .and_then(|r| r.auth.headers.as_ref())
                    .and_then(|headers| {
                        headers
                            .iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                            .and_then(|(_, value)| value.as_deref())
                    })
                    .and_then(strip_bearer)
            })
        };
        match credential {
            // Pi `main.ts:196-198` — a `ready` check that cannot produce the credential the caller
            // asked for degrades to `not_ready` / `credential_not_available`.
            None => {
                return Ok(AuthCheckResult {
                    status: "not_ready",
                    provider: result.provider,
                    reason: Some("credential_not_available"),
                    auth_type: None,
                    credentials: None,
                });
            }
            Some(c) => result.credentials = Some(c),
        }
    }
    Ok(result)
}

/// The pre-parse dispatcher (Pi `runCredentialPrintCommand`, main.ts:130-167).
///
/// `None` means `argv` is not an auth command and the ordinary CLI must run; `Some(code)` is the
/// process exit code. `argv` has the program name stripped and short aliases normalized, matching
/// the sibling `install`/`config` pre-dispatch in `main.rs`.
pub async fn dispatch(argv: &[String]) -> Option<i32> {
    if is_credential_print_help(argv) {
        print!("{}", credential_print_help());
        return Some(0);
    }
    let command = match parse_credential_print_command(argv) {
        Ok(None) => return None,
        Ok(Some(command)) => command,
        Err(message) => {
            eprintln!("Error: {message}");
            return Some(1);
        }
    };

    // Pi `parseArgs(command.args)` + `if (parsed.diagnostics.length > 0)`: EVERY diagnostic —
    // warnings included — is printed as `Error:` here and exits 1 (main.ts:147-153). That is
    // stricter than the ordinary startup path on purpose: the stdout contract is a bare credential.
    let (lenient, diagnostics) = apply_arg_leniency(&command.args);
    if !diagnostics.is_empty() {
        for diagnostic in &diagnostics {
            eprintln!("Error: {}", diagnostic.message);
        }
        return Some(1);
    }
    let (clean, extension_flags) = partition_extension_flags(&lenient);
    let mut clap_argv = vec!["cyrup".to_string()];
    clap_argv.extend(clean);
    let mut cli = match Cli::try_parse_from(&clap_argv) {
        Ok(cli) => cli,
        Err(error) => {
            let message = error.to_string();
            let first = message.lines().next().unwrap_or("Invalid arguments");
            eprintln!("Error: {first}");
            return Some(1);
        }
    };
    cli.extension_flags = extension_flags;
    cli.normalize_list_flags();

    // No CLI config overrides: Pi's `ModelRuntime.create` reads the standard agent dir, and
    // `validateCredentialPrintArgs` has already rejected everything but `--provider`/`--model`.
    let env = cyrup_config::EnvVars::from_process();
    let Ok(dirs) = ConfigDirs::resolve(&cyrup_config::CliConfigOverrides::default(), &env) else {
        eprintln!("Error: {}", CredentialPrintError::Opaque.message());
        return Some(1);
    };

    if command.kind == CredentialPrintKind::Check {
        // Pi `main.ts:184-208` @v0.84.1. The output is the credential when `--credentials` produced
        // one, else the bare status word; `--json` serializes the whole result object instead
        // (`:206-207`). The exit code is the status, 0/1/2 (`:208`).
        return Some(
            match run_auth_check(&cli, &dirs, command.credentials, !command.no_refresh).await {
                Ok(result) => {
                    let output = if command.json {
                        serde_json::to_string(&result).unwrap_or_else(|_| result.status.to_string())
                    } else {
                        result
                            .credentials
                            .clone()
                            .unwrap_or_else(|| result.status.to_string())
                    };
                    println!("{output}");
                    result.exit_code()
                }
                Err(error) => {
                    eprintln!("Error: {}", error.message());
                    // Pi `process.exitCode = command.kind === "check" ? 2 : 1` (main.ts:210-211).
                    2
                }
            },
        );
    }

    match resolve_credential_for_print(&cli, &dirs, command.kind, command.min_expiry_ms).await {
        Ok(credential) => {
            println!("{credential}");
            Some(0)
        }
        Err(error) => {
            eprintln!("Error: {}", error.message());
            Some(1)
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn non_auth_argv_falls_through() {
        assert_eq!(parse_credential_print_command(&v(&["-p", "hi"])), Ok(None));
        assert_eq!(parse_credential_print_command(&v(&[])), Ok(None));
        assert!(!is_credential_print_help(&v(&["install", "x"])));
    }

    #[test]
    fn help_forms_match_upstream() {
        assert!(is_credential_print_help(&v(&["auth"])));
        assert!(is_credential_print_help(&v(&["auth", "help"])));
        assert!(is_credential_print_help(&v(&["auth", "--help"])));
        assert!(is_credential_print_help(&v(&["auth", "-h"])));
        assert!(!is_credential_print_help(&v(&["auth", "print-api-key"])));
    }

    #[test]
    fn unknown_auth_command_is_an_error() {
        let err = parse_credential_print_command(&v(&["auth", "login"])).unwrap_err();
        assert_eq!(
            err,
            "Unknown auth command \"login\". Use \"cyrup auth print-api-key\", \"cyrup auth \
             print-bearer-token\", or \"cyrup auth check\"."
        );
    }

    /// SEAM-050 — `auth check` is a real verb at v0.84.1 (`cli/auth-command.ts:4`, `:50-58`), and
    /// its three flags are accepted by IT ALONE (`:82-88`). Before this, `cyrup auth check` hit the
    /// unknown-command arm and exited 1, which an external tool branching on pi's 0/1/2 contract
    /// could not distinguish from `not_ready`.
    #[test]
    fn auth_check_parses_with_its_three_flags() {
        let cmd = parse_credential_print_command(&v(&[
            "auth",
            "check",
            "--provider",
            "openai",
            "--json",
            "--credentials",
            "--no-refresh",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(cmd.kind, CredentialPrintKind::Check);
        assert!(cmd.json && cmd.credentials && cmd.no_refresh);
        // The check-only flags are CONSUMED, so the residual argv the ordinary parser sees is just
        // the provider pair (pi pushes only non-flag args into `commandArgs`, `:89`).
        assert_eq!(cmd.args, v(&["--provider", "openai"]));

        // …and are rejected for the print verbs, with pi's message (`:83`).
        for flag in ["--json", "--credentials", "--no-refresh"] {
            let err =
                parse_credential_print_command(&v(&["auth", "print-api-key", flag])).unwrap_err();
            assert_eq!(err, format!("{flag} is only supported by auth check"));
        }
    }

    /// SEAM-050 — the 0/1/2 mapping is pi's `main.ts:208`, and it is the whole point of the verb:
    /// an external tool branches on it.
    #[test]
    fn auth_check_exit_codes_match_pi() {
        let mk = |status| AuthCheckResult {
            status,
            provider: "openai".to_string(),
            reason: None,
            auth_type: None,
            credentials: None,
        };
        assert_eq!(mk("ready").exit_code(), 0);
        assert_eq!(mk("not_ready").exit_code(), 1);
        assert_eq!(mk("invalid").exit_code(), 2);
    }

    /// SEAM-108 — the whole `auth` surface is v0.84.1-shaped against a v0.83.0 port, and that has to
    /// be SAID in-source, not merely be true. The three behaviours below are all deliberate
    /// forward-ports (they came in with SEAM-050); without a delta naming the tag, the next fidelity
    /// pass reading `credential-print.ts` @v0.83.0 finds three "defects" and reverts them.
    ///
    /// RED before this pass: no `[CYRUP-DELTA]` mentioning SEAM-108 existed anywhere in this file.
    /// Reads the module's own source at compile time, which is the only way to assert on a doc block.
    #[test]
    fn the_v0_84_1_forward_port_is_declared_as_a_cyrup_delta() {
        let src = include_str!("credential_print.rs");
        let delta = src
            .lines()
            .find(|l| l.contains("[CYRUP-DELTA]") && l.contains("SEAM-108"))
            .expect("the argument-validation site must carry a [CYRUP-DELTA] naming SEAM-108");
        assert!(
            delta.contains("v0.84.1"),
            "the delta must name the tag it forward-ported FROM: {delta}"
        );
        // …and it must say which tag is the ported baseline, or it does not tell an auditor which
        // side of the comparison to distrust.
        let block: String = src
            .lines()
            .skip_while(|l| !(l.contains("[CYRUP-DELTA]") && l.contains("SEAM-108")))
            .take(12)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            block.contains("v0.83.0"),
            "the delta must name the ported baseline too: {block}"
        );
        for owed in ["verb", "requires --model", "print-api-key"] {
            assert!(
                block.contains(owed),
                "the delta must name what diverges (`{owed}` missing): {block}"
            );
        }
    }

    /// SEAM-050 — v0.84.1 routes the PRINT verbs through `validateAuthCommandArgs` too
    /// (`credential-print.ts:24`), so `--provider` alone is enough; v0.83.0's `--model`-required
    /// check (`credential-print.ts:67-68`) rejected `cyrup auth print-api-key --provider openai`,
    /// which pi accepts. Also pins pi's per-command unknown-option message (`auth-command.ts:99-102`).
    #[test]
    fn auth_arg_validation_matches_v0_84_1() {
        let cli = Cli {
            provider: Some("openai".to_string()),
            ..Cli::default()
        };
        assert!(validate_credential_print_args(&cli, CredentialPrintKind::ApiKey).is_ok());
        assert!(validate_credential_print_args(&cli, CredentialPrintKind::Check).is_ok());

        let bare = Cli::default();
        assert_eq!(
            validate_credential_print_args(&bare, CredentialPrintKind::Check)
                .unwrap_err()
                .message(),
            "Auth checks require --provider <provider> or --model <model>"
        );
        assert_eq!(
            validate_credential_print_args(&bare, CredentialPrintKind::BearerToken)
                .unwrap_err()
                .message(),
            "Credential printing requires --provider <provider> or --model <model>"
        );

        let mut unknown = Cli {
            provider: Some("openai".to_string()),
            ..Cli::default()
        };
        unknown.extension_flags = vec![crate::cli::ExtensionFlag {
            name: "bogus".to_string(),
            value: crate::cli::ExtFlagValue::Bool(true),
        }];
        assert_eq!(
            validate_credential_print_args(&unknown, CredentialPrintKind::Check)
                .unwrap_err()
                .message(),
            "Unknown option --bogus for \"auth check\"."
        );
    }

    #[test]
    fn min_expiry_units() {
        assert_eq!(parse_min_expiry("500ms"), Some(500));
        assert_eq!(parse_min_expiry("30S"), Some(30_000));
        assert_eq!(parse_min_expiry("30m"), Some(1_800_000));
        assert_eq!(parse_min_expiry("1h"), Some(3_600_000));
        assert_eq!(parse_min_expiry("30"), None);
        assert_eq!(parse_min_expiry("m"), None);
        assert_eq!(parse_min_expiry("30d"), None);
        assert_eq!(parse_min_expiry("-1m"), None);
    }

    #[test]
    fn min_expiry_is_consumed_and_kind_gated() {
        let cmd = parse_credential_print_command(&v(&[
            "auth",
            "print-bearer-token",
            "--model",
            "gpt-5.5",
            "--min-expiry",
            "45m",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(cmd.kind, CredentialPrintKind::BearerToken);
        assert_eq!(cmd.args, v(&["--model", "gpt-5.5"]));
        assert_eq!(cmd.min_expiry_ms, Some(2_700_000));

        let err =
            parse_credential_print_command(&v(&["auth", "print-api-key", "--min-expiry", "45m"]))
                .unwrap_err();
        assert_eq!(err, "--min-expiry is only supported by print-bearer-token");

        let err =
            parse_credential_print_command(&v(&["auth", "print-bearer-token", "--min-expiry"]))
                .unwrap_err();
        assert_eq!(err, "--min-expiry must use a duration such as 30m or 1h");
    }

    #[test]
    fn bearer_header_extraction() {
        assert_eq!(strip_bearer("Bearer tok-1"), Some("tok-1".to_string()));
        assert_eq!(strip_bearer("bearer   tok-2"), Some("tok-2".to_string()));
        assert_eq!(strip_bearer("Bearer"), None);
        assert_eq!(strip_bearer("Bearer "), None);
        assert_eq!(strip_bearer("Basic tok"), None);
    }

    /// v0.84.1's `validateAuthCommandArgs` (`cli/auth-command.ts:96-116`), which BOTH the print
    /// verbs and `check` now route through (`credential-print.ts:24`). SEAM-050 rewrote three of
    /// these expectations against that function; the v0.83.0 `--model`-required shape they used to
    /// pin is gone upstream.
    #[test]
    fn validation_mirrors_upstream() {
        let base = |args: &[&str]| {
            let mut argv = vec!["cyrup".to_string()];
            argv.extend(v(args));
            Cli::try_parse_from(&argv).unwrap()
        };
        // pi `:113-115` — provider OR model, and the sentence names neither as required alone.
        assert_eq!(
            validate_credential_print_args(&base(&[]), CredentialPrintKind::ApiKey)
                .unwrap_err()
                .message(),
            "Credential printing requires --provider <provider> or --model <model>"
        );
        // A whitespace-only value is `args.model?.trim() || undefined` (`:98`) — i.e. absent.
        assert_eq!(
            validate_credential_print_args(&base(&["--model", "   "]), CredentialPrintKind::ApiKey)
                .unwrap_err()
                .message(),
            "Credential printing requires --provider <provider> or --model <model>"
        );
        // pi `:103-105` — `apiKey !== undefined || messages.length > 0 || fileArgs.length > 0`, one
        // message for all three.
        assert_eq!(
            validate_credential_print_args(
                &base(&["--model", "m", "--api-key", "k"]),
                CredentialPrintKind::ApiKey
            )
            .unwrap_err()
            .message(),
            "Auth commands only accept --provider and --model"
        );
        assert_eq!(
            validate_credential_print_args(
                &base(&["--model", "m", "hello"]),
                CredentialPrintKind::BearerToken
            )
            .unwrap_err()
            .message(),
            "Auth commands only accept --provider and --model"
        );
        assert!(
            validate_credential_print_args(
                &base(&["--model", "m", "--provider", "openai"]),
                CredentialPrintKind::ApiKey
            )
            .is_ok()
        );
        // SEAM-050: `--provider` ALONE is now enough for a print verb — v0.83.0's
        // `credential-print.ts:67-68` rejected this and v0.84.1 accepts it.
        assert!(
            validate_credential_print_args(
                &base(&["--provider", "openai"]),
                CredentialPrintKind::ApiKey
            )
            .is_ok()
        );
    }
}
