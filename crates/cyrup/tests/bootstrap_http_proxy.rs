//! PROV-047 — the BOOTSTRAP `httpProxy` install, proven against the real shipped binary.
//!
//! pi applies the setting twice. The second call is the one cyrup already had
//! (`main.ts:801` → `cyrup-session-svc/src/builder.rs`'s `apply_http_proxy_settings`, reached only
//! when a session is BUILT). The first is `main.ts:536-538` @v0.83.0:
//!
//! ```ts
//! const bootstrapSettingsManager = SettingsManager.create(cwd, agentDir, { projectTrusted: false });
//! applyHttpProxySettings(bootstrapSettingsManager.getGlobalSettings().httpProxy);
//! configureHttpDispatcher();
//! ```
//!
//! and it sits ABOVE `handlePackageCommand` (`:541`), `runCredentialPrintCommand` (`:557`) and
//! `parseArgs` (`:562`) — i.e. above every command that egresses before a session exists. cyrup had
//! only the second, so `cyrup update --models` (`subcommands.rs:533` →
//! `provider::refresh_model_catalogs`) and the `cyrup auth check` / `print-bearer-token` OAuth
//! refresh both ran with the proxy setting never installed.
//!
//! **Measured before the fix**, with `{"httpProxy":"socks5://127.0.0.1:1080"}` in the GLOBAL
//! `settings.json` and no ambient proxy variables: `cyrup update --models` printed
//! `Model catalogs refreshed` and exited 0 — it had reached `https://pi.dev` DIRECTLY. Not a
//! failure the operator could attribute: unannounced egress on a machine whose operator had
//! configured a proxy precisely so that would not happen.
//!
//! Both tests are hermetic. The first never opens a socket at all: cyrup's ported resolver
//! (`node_http_proxy.rs`, pi `node-http-proxy.ts:89`) rejects a SOCKS proxy URL before any connect,
//! so the SOCKS spelling is the assertion device — it makes "the resolver saw the setting" a string
//! in stderr rather than a packet. The second only ever dials `127.0.0.1`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::{Command, Output};

/// pi `UNSUPPORTED_PROXY_PROTOCOL_MESSAGE` (`packages/ai/src/utils/node-http-proxy.ts:89`
/// @v0.83.0), ported verbatim as `cyrup_provider::UNSUPPORTED_PROXY_PROTOCOL_MESSAGE`. Spelled out
/// here rather than imported so the test pins the user-visible STRING, not the constant.
const UNSUPPORTED: &str = "Unsupported proxy protocol. SOCKS and PAC proxy URLs are not supported";

/// A global `settings.json` + an `auth.json` with one configured provider, so
/// `refresh_model_catalogs`'s credential-gated provider set is non-empty and a request is actually
/// attempted (`provider.rs:233-241`).
fn agent_dir(root: &Path, settings: &str) -> std::path::PathBuf {
    let agent = root.join("agent");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::write(agent.join("settings.json"), settings).unwrap();
    std::fs::write(
        agent.join("auth.json"),
        r#"{"anthropic":{"type":"api_key","key":"sk-test-not-a-real-key"}}"#,
    )
    .unwrap();
    agent
}

/// `cyrup update --models` with every ambient proxy variable cleared, so the GLOBAL `httpProxy`
/// setting is the only possible source of a proxy. `extra_env` adds the ambient variables a
/// specific test wants to be the exception.
fn run_update_models(agent: &Path, cwd: &Path, extra_env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cyrup"));
    cmd.args(["update", "--models"])
        .current_dir(cwd)
        .env("CYRUP_AGENT_DIR", agent);
    // pi's `??=` gives an ambient variable precedence over the setting, and `getProxyEnv` reads
    // both cases of all four names — so all eight must go, or the ambient environment of whoever
    // runs the suite silently decides the outcome.
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ] {
        cmd.env_remove(name);
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("spawning the cyrup binary")
}

/// A TCP port with nothing listening on it: bind ephemeral, read the port, drop the listener.
fn closed_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// The bootstrap install itself: a GLOBAL `httpProxy` reaches an egress path that runs entirely
/// BEFORE any session is built.
#[test]
fn prov047_the_global_http_proxy_reaches_a_pre_session_subcommand() {
    let root = tempfile::tempdir().unwrap();
    let agent = agent_dir(root.path(), r#"{"httpProxy":"socks5://127.0.0.1:1080"}"#);

    let out = run_update_models(&agent, root.path(), &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // Presence first: the refresh ran, consulted the resolver, and the resolver had the setting.
    assert!(
        stderr.contains("Could not refresh model catalogs"),
        "expected pi's `Error: Could not refresh model catalogs: …` (subcommands.rs:533-536); got\n\
         stdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&out.stdout),
    );
    assert!(
        stderr.contains(UNSUPPORTED),
        "the GLOBAL `httpProxy` never reached the catalog refresh — before the PROV-047 bootstrap \
         install this printed `Model catalogs refreshed` and exited 0, having gone DIRECT to \
         pi.dev; got\nstdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&out.stdout),
    );
    assert_eq!(out.status.code(), Some(1), "pi exits 1 on this branch");
}

/// …and pi's `??=` precedence survives at the bootstrap: an ambient `HTTPS_PROXY` WINS over the
/// setting, exactly as `process.env.HTTPS_PROXY ??= proxy` leaves an already-set variable alone
/// (`http-dispatcher.ts:47` @v0.83.0). Reversing this would let a stale `settings.json` override
/// the environment the operator exported for this run.
#[test]
fn prov047_an_ambient_https_proxy_still_wins_over_the_setting() {
    let root = tempfile::tempdir().unwrap();
    // The SETTING is the SOCKS one this time, so if it were (wrongly) preferred the resolver's
    // unsupported-protocol message would appear.
    let agent = agent_dir(root.path(), r#"{"httpProxy":"socks5://127.0.0.1:1080"}"#);
    let ambient = format!("http://127.0.0.1:{}", closed_loopback_port());

    let out = run_update_models(&agent, root.path(), &[("HTTPS_PROXY", ambient.as_str())]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stderr.contains("Could not refresh model catalogs"),
        "expected the refresh to fail against the dead ambient proxy {ambient}; got\n\
         stdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&out.stdout),
    );
    assert!(
        !stderr.contains(UNSUPPORTED),
        "the SETTING overrode an ambient HTTPS_PROXY — pi's `??=` gives the ambient value \
         precedence (http-dispatcher.ts:47); got\nstderr: {stderr}",
    );
    assert_eq!(out.status.code(), Some(1));
}
