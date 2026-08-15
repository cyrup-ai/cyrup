//! `cyrup auth print-api-key` / `print-bearer-token` — the credential-print surface external
//! clients script against, proved at the BINARY seam (the only place the stdout contract, the
//! stderr text and the exit code are observable together).
//!
//! Upstream: pi v0.83.0 dispatches `if (await runCredentialPrintCommand(args)) return;` BEFORE
//! `parseArgs` (`packages/coding-agent/src/main.ts`), over
//! `packages/coding-agent/src/cli/credential-print.ts` — `parseCredentialPrintCommand`,
//! `validateCredentialPrintArgs`, `resolveCredentialForPrint`. It writes the bare credential on
//! stdout and reports every failure as `Error: <message>` with exit code 1, and it advertises the
//! command in `--help` (`cli/args.ts`).
//!
//! cyrup had no `auth` verb at all: `SUBCOMMANDS` (subcommands.rs) did not list it, so
//! `cyrup auth print-api-key --provider … --model …` survived arg leniency as bare positionals,
//! became a chat PROMPT and started an agent session — no credential, no error. Any script
//! following the upstream contract (`KEY=$(cyrup auth print-api-key …)`) captured assistant prose.
//!
//! Fully offline and hermetic. The fixture declares a CUSTOM provider in `models.json` whose base
//! URL is `http://127.0.0.1:1/v1`, so even the pre-fix fallthrough this test guards against cannot
//! reach a real provider; `--offline` is set, every ambient provider key and proxy is scrubbed from
//! the child env, and HOME / the agent dir are tempdirs. No network, no credentials, no paid tokens.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::process::{Command, Stdio};

use tempfile::TempDir;

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

/// A tempdir agent home carrying one stored API-key credential for the custom `acme` provider.
///
/// `acme` is declared in `models.json` rather than reusing a built-in on purpose: a built-in would
/// make the pre-fix fallthrough (which becomes a real agent session on a real provider) capable of
/// egress, and this test must be incapable of touching a network by construction.
fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(tmp.path().join("work")).unwrap();
    std::fs::write(
        agent_dir.join("auth.json"),
        r#"{ "acme": { "type": "api_key", "key": "sk-acme-stored" } }"#,
    )
    .unwrap();
    std::fs::write(
        agent_dir.join("models.json"),
        r#"{ "providers": { "acme": {
              "baseUrl": "http://127.0.0.1:1/v1",
              "api": "openai-completions",
              "models": [ { "id": "acme-1", "name": "Acme One" } ]
        } } }"#,
    )
    .unwrap();
    tmp
}

/// Run the real `cyrup` binary against `tmp`, appending `args` verbatim. Stdin is `/dev/null`: a
/// non-TTY at EOF, so a run that (wrongly) fell through to the ordinary CLI would select the
/// one-shot path rather than blocking on a TUI.
///
/// The child env is **cleared**, not selectively scrubbed. A denylist of the obvious
/// `*_API_KEY` names is not good enough here: the failure this test guards against is the binary
/// falling through to a real agent session, and `findInitialModel` will happily launch on ANY
/// provider whose env key happens to be exported on the developer's machine — which would spend
/// real tokens on a run that is supposed to be inert. Only `HOME`, the agent dir and `PATH` are
/// reinstated, mirroring the `env -i` + allowlist shape of pi's own `test.sh`.
fn run(tmp: &TempDir, args: &[&str]) -> Run {
    let out = Command::new(crate::support::bins::cyrup())
        .current_dir(tmp.path().join("work"))
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", tmp.path())
        .env("CYRUP_AGENT_DIR", tmp.path().join("agent"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn cyrup");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// THE headline: the credential is written ALONE on stdout with a trailing newline, nothing else,
/// exit 0 — the shape `KEY=$(cyrup auth print-api-key …)` depends on.
#[test]
fn print_api_key_writes_the_credential_alone_on_stdout() {
    let tmp = fixture();
    let r = run(
        &tmp,
        &["auth", "print-api-key", "--provider", "acme", "--model", "acme-1"],
    );
    assert_eq!(
        r.stdout, "sk-acme-stored\n",
        "stdout must be the bare credential; stderr was: {}",
        r.stderr
    );
    assert_eq!(r.code, 0, "stderr was: {}", r.stderr);
    assert_eq!(r.stderr, "", "the success path is silent on stderr");
}

/// With no `--provider`, upstream infers it from the providers that HAVE a stored credential
/// (credential-print.ts `for (const provider of modelRuntime.getProviders())`).
#[test]
fn print_api_key_infers_the_provider_from_stored_credentials() {
    let tmp = fixture();
    let r = run(&tmp, &["auth", "print-api-key", "--model", "acme-1"]);
    assert_eq!(r.stdout, "sk-acme-stored\n", "stderr was: {}", r.stderr);
    assert_eq!(r.code, 0, "stderr was: {}", r.stderr);
}

/// An unknown `auth` verb is upstream's error, not a prompt: exit 1, the message on stderr, and
/// stdout left completely empty for the caller's `$( … )` capture.
///
/// THE THIRD VERB. This asserted the v0.83.0 two-verb sentence
/// (`credential-print.ts:39`); `auth check` arrived at pi v0.84.1 and cyrup ported it (SEAM-050),
/// so the live sentence is `auth-command.ts:60-62` @v0.84.1's three-verb form. The test was never
/// re-run after that port — `cyrup-it` is `required-features = ["it"]` and the merge gate does not
/// arm it — so it sat red-in-waiting.
#[test]
fn an_unknown_auth_command_errors_instead_of_prompting() {
    let tmp = fixture();
    let r = run(&tmp, &["auth", "login"]);
    assert_eq!(
        r.stderr.trim_end(),
        "Error: Unknown auth command \"login\". Use \"cyrup auth print-api-key\", \"cyrup auth \
         print-bearer-token\", or \"cyrup auth check\"."
    );
    assert_eq!(r.code, 1);
    assert_eq!(r.stdout, "", "stdout must stay clean on failure");
}

/// A bare `cyrup auth` prints the usage block and exits 0 (Pi `isAuthCommandHelp` +
/// `printAuthCommandHelp`, `cli/auth-command.ts:31-45` @v0.84.1).
///
/// Same staleness as the test above: the old assertion demanded `--model <model>` as a REQUIRED
/// argument in the two print lines, which is the v0.83.0 `printCredentialPrintHelp` text
/// (`credential-print.ts:24-30`). v0.84.1 made both `[--provider <provider>] [--model <model>]`
/// optional-bracketed and added the `check` line, because auth commands now require provider OR
/// model rather than model alone.
#[test]
fn bare_auth_prints_the_usage_block() {
    let tmp = fixture();
    let r = run(&tmp, &["auth"]);
    assert_eq!(r.code, 0, "stderr was: {}", r.stderr);
    assert!(r.stdout.starts_with("Usage:"), "stdout was: {}", r.stdout);
    for line in [
        "cyrup auth print-api-key [--provider <provider>] [--model <model>]",
        "cyrup auth print-bearer-token [--provider <provider>] [--model <model>] [--min-expiry <duration>]",
        "cyrup auth check [--provider <provider>] [--model <model>] [--json] [--credentials] [--no-refresh]",
    ] {
        assert!(r.stdout.contains(line), "usage block is missing `{line}`; stdout was: {}", r.stdout);
    }
    // pi `:44` — the sentence that states the provider-OR-model rule the v0.84.1 surface turns on.
    assert!(
        r.stdout.contains("Auth commands require at least one of --provider or --model."),
        "stdout was: {}",
        r.stdout
    );
}

/// `print-bearer-token` against a provider configured with an API key is upstream's typed error
/// (credential-print.ts `Provider "…" is not configured with an OAuth bearer token`).
#[test]
fn print_bearer_token_rejects_an_api_key_provider() {
    let tmp = fixture();
    let r = run(
        &tmp,
        &["auth", "print-bearer-token", "--provider", "acme", "--model", "acme-1"],
    );
    assert_eq!(
        r.stderr.trim_end(),
        "Error: Provider \"acme\" is not configured with an OAuth bearer token"
    );
    assert_eq!(r.code, 1);
    assert_eq!(r.stdout, "");
}

/// The `validateAuthCommandArgs` surface (`cli/auth-command.ts:96-116` @v0.84.1), each reached
/// through the real binary.
///
/// The v0.83.0 `validateCredentialPrintArgs` had three separate messages and required `--model`;
/// v0.84.1 folded `--api-key`/positionals into ONE sentence (`:103-105`) and made the requirement
/// provider-OR-model (`:113-115`). Production followed (SEAM-050); this test did not.
#[test]
fn credential_printing_validates_its_argument_surface() {
    let tmp = fixture();

    // pi `:103-105` — `apiKey !== undefined || messages.length > 0 || fileArgs.length > 0` is a
    // SINGLE message, not one per cause.
    let r = run(
        &tmp,
        &["auth", "print-api-key", "--model", "acme-1", "--api-key", "sk-injected"],
    );
    assert_eq!(
        r.stderr.trim_end(),
        "Error: Auth commands only accept --provider and --model"
    );
    assert_eq!(r.code, 1);
    assert_eq!(r.stdout, "", "an injected key must never be echoed back");

    let r = run(&tmp, &["auth", "print-api-key", "--model", "acme-1", "extra"]);
    assert_eq!(
        r.stderr.trim_end(),
        "Error: Auth commands only accept --provider and --model"
    );
    assert_eq!(r.code, 1);

    // pi `:113-115` — neither provider nor model.
    let r = run(&tmp, &["auth", "print-api-key"]);
    assert_eq!(
        r.stderr.trim_end(),
        "Error: Credential printing requires --provider <provider> or --model <model>"
    );
    assert_eq!(r.code, 1);

    // pi `:99-102` — an unmatched flag names the command it was given to.
    let r = run(&tmp, &["auth", "print-api-key", "--model", "acme-1", "--nope"]);
    assert_eq!(
        r.stderr.trim_end(),
        "Error: Unknown option --nope for \"auth print-api-key\"."
    );
    assert_eq!(r.code, 1);

    let r = run(
        &tmp,
        &["auth", "print-bearer-token", "--model", "acme-1", "--min-expiry", "30d"],
    );
    assert_eq!(
        r.stderr.trim_end(),
        "Error: --min-expiry must use a duration such as 30m or 1h"
    );
    assert_eq!(r.code, 1);
}

/// `--provider` ALONE prints the credential — no `--model` required.
///
/// pi `resolveCredentialForPrint` (`cli/credential-print.ts:29-40` @v0.84.1) branches on `cliModel`:
/// with a model it resolves one; WITHOUT one it pushes `{ id: provider.id }` and fetches the
/// credential with `getAuth(provider.id, …)` (`:66`). SEAM-050 relaxed the VALIDATOR to pi's
/// provider-OR-model rule but never gave the RESOLVER that second branch, so cyrup passed the
/// absent model into `resolve_cli_model` and turned its empty result into
/// `Unable to resolve the requested provider/model` — the rejection moved one layer down and the
/// port read as finished. This is the assertion that distinguishes the two.
#[test]
fn print_api_key_accepts_a_provider_with_no_model() {
    let tmp = fixture();
    let r = run(&tmp, &["auth", "print-api-key", "--provider", "acme"]);
    assert_eq!(
        r.stdout, "sk-acme-stored\n",
        "a bare --provider must print the credential; stderr was: {}",
        r.stderr
    );
    assert_eq!(r.code, 0, "stderr was: {}", r.stderr);
    assert_eq!(r.stderr, "", "the success path is silent on stderr");
}

/// The companion bound: an UNKNOWN `--provider` is pi's own named error, not the generic resolver
/// message.
///
/// pi `credential-print.ts:30-32` @v0.84.1 checks `modelRuntime.getProvider(cliProvider)` FIRST and
/// throws `Unknown provider "…". Use --list-models to see available providers.`. cyrup had no such
/// check at all — an unknown provider fell through `resolve_cli_model` and surfaced as
/// `Unable to resolve the requested provider/model`, which never mentions `--list-models`. Without
/// this test the fix above could have been "accept every provider", which is not the port.
#[test]
fn an_unknown_provider_is_named_and_points_at_list_models() {
    let tmp = fixture();
    let r = run(&tmp, &["auth", "print-api-key", "--provider", "nosuchprovider"]);
    assert_eq!(
        r.stderr.trim_end(),
        "Error: Unknown provider \"nosuchprovider\". Use --list-models to see available providers."
    );
    assert_eq!(r.code, 1);
    assert_eq!(r.stdout, "", "stdout must stay clean on failure");
}

/// `--help` advertises the command exactly as upstream's `cli/args.ts` does.
#[test]
fn help_advertises_the_auth_command() {
    let tmp = fixture();
    let r = run(&tmp, &["--help"]);
    assert_eq!(r.code, 0, "stderr was: {}", r.stderr);
    assert!(
        r.stdout.contains("cyrup auth <command>            Print credentials for external clients"),
        "stdout was: {}",
        r.stdout
    );
    assert!(
        r.stdout
            .contains("cyrup auth print-api-key --provider openai --model gpt-5.5"),
        "stdout was: {}",
        r.stdout
    );
}

/// MIRROR (green before AND after the fix): the pre-dispatch is narrow. A first positional that
/// merely LOOKS auth-ish is still an ordinary prompt, so the new `auth` arm cannot swallow user
/// input. This is what shows the assertions above are not passing because the binary refuses
/// everything.
#[test]
fn mirror_an_auth_shaped_prompt_is_still_a_prompt() {
    let tmp = fixture();
    let r = run(
        &tmp,
        &[
            "--offline",
            "--no-session",
            "--no-extensions",
            "--model",
            "faux/faux-1",
            "--mode",
            "json",
            "authenticate the user",
        ],
    );
    assert_eq!(r.code, 0, "stderr was: {}", r.stderr);
    let user_text = r
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| {
            value.get("type").and_then(|t| t.as_str()) == Some("message_start")
                && value.pointer("/message/role").and_then(|r| r.as_str()) == Some("user")
        })
        .and_then(|value| {
            value
                .pointer("/message/content/0/text")
                .and_then(|t| t.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| panic!("no user message in:\n{}", r.stdout));
    assert_eq!(user_text, "authenticate the user");
}

/// MIRROR (green before AND after the fix): the sibling package/config pre-dispatch still runs.
/// The `auth` arm is inserted directly after it, so this guards the insertion point ordering.
#[test]
fn mirror_the_package_subcommand_pre_dispatch_still_runs() {
    let tmp = fixture();
    let r = run(&tmp, &["list"]);
    assert_eq!(r.code, 0, "stderr was: {}", r.stderr);
    assert!(
        r.stdout.contains("No packages installed."),
        "stdout was: {}",
        r.stdout
    );
}
