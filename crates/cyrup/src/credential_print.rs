//! `cyrup auth print-api-key` / `cyrup auth print-bearer-token` — the credential-print surface
//! external clients script against (arch-11 §3.7).
//!
//! A 1:1 port of Pi `coding-agent/src/cli/credential-print.ts` (v0.83.0, 152 lines) plus its
//! driver `runCredentialPrintCommand` (main.ts:130-167), dispatched — like Pi's — BEFORE ordinary
//! argument parsing (main.ts:557-559, right after the package/config subcommand block). Before this
//! module existed, `auth` was not in [`crate::subcommands::SUBCOMMANDS`], so
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

/// Which credential the command prints (Pi `CredentialPrintKind`, credential-print.ts:6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialPrintKind {
    ApiKey,
    BearerToken,
}

impl CredentialPrintKind {
    /// The noun upstream uses in `No usable <…> is configured` (credential-print.ts:145-147).
    fn noun(self) -> &'static str {
        match self {
            CredentialPrintKind::ApiKey => "API key",
            CredentialPrintKind::BearerToken => "OAuth bearer token",
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

/// The `auth` usage block (Pi `printCredentialPrintHelp`, credential-print.ts:24-30), rebranded.
pub fn credential_print_help() -> String {
    "Usage:\n  cyrup auth print-api-key --model <model> [--provider <provider>]\n  cyrup auth \
     print-bearer-token --model <model> [--provider <provider>] [--min-expiry <duration>]\n\nPrints \
     the configured credential alone on stdout. Provider inference uses configured credentials; \
     specify --provider to select explicitly. Bearer tokens have a 30-minute minimum expiry by \
     default. --min-expiry accepts ms, s, m, or h (for example, 30m).\n"
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
        Some("print-api-key") => CredentialPrintKind::ApiKey,
        Some("print-bearer-token") => CredentialPrintKind::BearerToken,
        other => {
            return Err(format!(
                "Unknown auth command \"{}\". Use \"cyrup auth print-api-key\" or \"cyrup auth \
                 print-bearer-token\".",
                other.unwrap_or("")
            ));
        }
    };

    let mut args: Vec<String> = Vec::new();
    let mut min_expiry_ms: Option<i64> = None;
    let mut index = 2usize;
    while let Some(arg) = argv.get(index) {
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
    }))
}

/// Reject anything outside `--provider` / `--model` (Pi `validateCredentialPrintArgs`,
/// credential-print.ts:66-76). `positionals` is cyrup's combined carrier for Pi's `messages` +
/// `fileArgs`; `extension_flags` is Pi's `unknownFlags`.
pub fn validate_credential_print_args(cli: &Cli) -> Result<(), CredentialPrintError> {
    if !cli.model.as_deref().is_some_and(|m| !m.trim().is_empty()) {
        return Err(CredentialPrintError::msg(
            "Credential printing requires --model <model>",
        ));
    }
    if cli.api_key.is_some() {
        return Err(CredentialPrintError::msg(
            "Credential printing reads configured credentials; --api-key is not supported",
        ));
    }
    if !cli.positionals.is_empty() || !cli.extension_flags.is_empty() {
        return Err(CredentialPrintError::msg(
            "Credential printing only accepts --provider and --model",
        ));
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
    value.get(..6).filter(|p| p.eq_ignore_ascii_case("Bearer"))?;
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
    validate_credential_print_args(cli)?;
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
    let has_configured_auth = |m: &Model| {
        cyrup_config::provider_is_configured(&auth, &models_json, &m.provider, None)
    };

    // Pi's two branches: an explicit `--provider` resolves exactly one model (an error is fatal);
    // otherwise every provider that HAS a stored credential is tried and ambiguity is reported.
    let mut models: Vec<Model> = Vec::new();
    if let Some(provider) = cli.provider.as_deref() {
        let resolved = cyrup_config::resolve_cli_model(
            Some(provider),
            cli.model.as_deref(),
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
                CredentialPrintKind::ApiKey => None,
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
            CredentialPrintKind::ApiKey => api_key,
        };
        if let Some(value) = value {
            credentials.push((model.provider.as_str().to_string(), value));
        }
    }

    if let [(_, value)] = credentials.as_slice() {
        return Ok(value.clone());
    }
    if credentials.is_empty() {
        let provider_id = models.first().map(|m| m.provider.as_str().to_string());
        let stored = provider_id
            .as_deref()
            .and_then(|p| credential_types.get(p).copied());
        if let Some(provider_id) = provider_id.filter(|_| cli.provider.is_some()) {
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
            "Unknown auth command \"login\". Use \"cyrup auth print-api-key\" or \"cyrup auth \
             print-bearer-token\"."
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

        let err = parse_credential_print_command(&v(&[
            "auth",
            "print-api-key",
            "--min-expiry",
            "45m",
        ]))
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

    #[test]
    fn validation_mirrors_upstream() {
        let base = |args: &[&str]| {
            let mut argv = vec!["cyrup".to_string()];
            argv.extend(v(args));
            Cli::try_parse_from(&argv).unwrap()
        };
        assert_eq!(
            validate_credential_print_args(&base(&[]))
                .unwrap_err()
                .message(),
            "Credential printing requires --model <model>"
        );
        assert_eq!(
            validate_credential_print_args(&base(&["--model", "   "]))
                .unwrap_err()
                .message(),
            "Credential printing requires --model <model>"
        );
        assert_eq!(
            validate_credential_print_args(&base(&["--model", "m", "--api-key", "k"]))
                .unwrap_err()
                .message(),
            "Credential printing reads configured credentials; --api-key is not supported"
        );
        assert_eq!(
            validate_credential_print_args(&base(&["--model", "m", "hello"]))
                .unwrap_err()
                .message(),
            "Credential printing only accepts --provider and --model"
        );
        assert!(
            validate_credential_print_args(&base(&["--model", "m", "--provider", "openai"]))
                .is_ok()
        );
    }
}
