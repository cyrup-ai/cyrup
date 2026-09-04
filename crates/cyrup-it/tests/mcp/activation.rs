//! **MCP-001's verify paragraph, end to end** (`docs/gap-analysis/13a-mcp-activation.md:1200-1201`):
//!
//! > a `cyrup-it` test asserting the `mcp` tool appears in `all_tool_names()` for a session built
//! > with a fixture `mcp.json`, and does **not** appear under `--no-extensions`.
//!
//! Both halves are one claim about one mechanism, and it is worth naming because it is the only
//! part of the adapter's activation that no unit test can reach. `cyrup_mcp::McpExtension` declares
//! [`NativeExtension::is_ambient`]` -> true` (`crates/cyrup-mcp/src/extension.rs`), which is the
//! port of `pi-mcp-adapter` being an *installed npm package* rather than an inline factory —
//! `resource-loader.ts:451-452` @v0.83.0 collapses exactly that tier to the explicit `-e` paths
//! under `noExtensions`. cyrup's analogue is
//! `cyrup_session_svc::builder::native_survives_no_extensions`, which consults `is_ambient()` and
//! nothing else. A crate-local test can assert the *method* returns `true`
//! (`extension.rs::the_adapter_is_ambient_and_does_not_decide_project_trust` does), but only an
//! assembled session runs the gate, so only an assembled session can prove the declaration is
//! wired to an effect. Get `is_ambient` wrong and every unit test in `cyrup-mcp` still passes
//! while `cyrup --no-extensions` silently keeps the MCP surface.
//!
//! **What `all_tool_names()` means here.** MCP-001 names the `HostServices` seam
//! (`cyrup_ext::HostServices::all_tool_names`) — pi's `getAllTools`, the full enable-able registry
//! rather than the exposed subset. Its implementation is
//! `crates/cyrup-session-svc/src/host_services.rs:1770`, one line:
//! `Some(Self::lock(&dt).all().into_iter().map(|t| t.name).collect())`. `AgentSession::all_tools()`
//! (`session.rs:5629`) reads that same `DynamicToolState::all()`. Asserting through the session
//! accessor is therefore the same registry by construction, and it is the one an integration test
//! can hold without reaching into the live host's private services.
//!
//! **Hermeticity.** The adapter's config ladder has three home-anchored rungs
//! (`~/.config/mcp/mcp.json`, `~/.agents/mcp.json`, `~/.agents/mcp/mcp.json`) and seven import
//! families. `CYRUP_HOME` would move them, but `std::env::set_var` is `unsafe` in edition 2024 and
//! unsound in a consolidated test binary — docs/TEST-ARCHITECTURE.md §4 R2, restated on
//! `tests/support/env.rs`. So the home is pinned by value through `McpExtension::with_home`, the
//! same field-not-call shape `cyrup_mcp::config::ConfigContext::with_home` already carries and for
//! the same stated reason. Everything else about the extension is production: `with_config(dirs,
//! None)` is verbatim what `mcp_extension_for_env` constructs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_mcp::{EXTENSION_ID, McpExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

/// The gateway tool `installMcpAdapter` registers (`cyrup_mcp::registration::PROXY_TOOL_NAME`).
const PROXY_TOOL: &str = "mcp";

/// A server the fixture marks `"disabled": true`. Its NAME is the evidence: `buildProxyDescription`
/// step 4 emits a `Disabled servers (enable with /mcp enable <server> and /reload): …` line from
/// the CONFIG alone — no metadata cache required — so finding this string inside the registered
/// tool's description proves the fixture file was parsed, merged, and carried all the way into the
/// live session's tool registry. Without it the test would pass just as well against an adapter
/// that never opened `mcp.json`, because the proxy tool registers unconditionally by default.
const DISABLED_SERVER: &str = "mcp-001-disabled-fixture";

/// A `lifecycle: "lazy"` stdio server — the shape MCP-003 calls the cold-start case. `lazy` is
/// load-bearing for the assertion that nothing spawns: `needs_load_time_initialization` pre-warms
/// only `eager` / `keep-alive`, so the `command` below is never executed and does not need to exist.
const LAZY_SERVER: &str = "mcp-001-lazy-fixture";

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

/// A temp `<agent_dir>` carrying the fixture `mcp.json` at the USER rung
/// (`McpDirs::user_config` = `<agent_dir>/mcp.json`), plus an empty project cwd.
fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("mcp.json"),
        format!(
            r#"{{
  "mcpServers": {{
    "{LAZY_SERVER}": {{
      "command": "/nonexistent/mcp-001-never-spawned",
      "args": ["--stdio"],
      "lifecycle": "lazy"
    }},
    "{DISABLED_SERVER}": {{
      "command": "/nonexistent/mcp-001-also-never-spawned",
      "disabled": true
    }}
  }}
}}
"#
        ),
    )
    .unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

/// `SessionConfig::new` already sets `home = agent_dir`, so the session and the adapter agree on
/// one temp home with no env mutation anywhere.
fn config(fx: &Fixture, no_extensions: bool) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = no_extensions;
    cfg
}

/// The adapter exactly as `crates/cyrup/src/main.rs` attaches it — `McpExtension::with_config(dirs,
/// None)` through `into_arc`, which is `mcp_extension_for_env`'s entire body — with only the home
/// pinned.
///
/// `into_arc` rather than a bare `Arc::new`: it is what binds the extension's self-handle, and
/// without it this fixture would differ from production in precisely the field that decides
/// whether the `onToolMetadataUpdated` listener can install — so a test of the late path would
/// take the unbound branch and pass for the wrong reason.
fn adapter(fx: &Fixture) -> Arc<dyn cyrup_ext::NativeExtension> {
    let dirs = cyrup_mcp::dirs::McpDirs::new(fx.agent_dir.clone(), fx.cwd.clone());
    McpExtension::with_config(dirs, None)
        .with_home(fx.agent_dir.clone())
        .into_arc() as Arc<dyn cyrup_ext::NativeExtension>
}

/// Build a real session with the adapter attached and return its FULL registered tool set
/// (`getAllTools`) plus the `mcp` tool's description, if it registered one.
async fn tool_registry(fx: &Fixture, no_extensions: bool) -> (Vec<String>, Option<String>) {
    let faux = Arc::new(FauxProvider::new()) as Arc<dyn Provider>;
    let session = SessionBuilder::new(faux, config(fx, no_extensions))
        .with_native_extension(adapter(fx))
        .build()
        .await
        .unwrap();
    let all = session.all_tools();
    let names = all.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
    let proxy_description = all
        .iter()
        .find(|t| t.name == PROXY_TOOL)
        .map(|t| t.description.clone());
    (names, proxy_description)
}

/// Nothing under `dir` may be an executable the adapter spawned. The fixture's two `command`s point
/// at `/nonexistent/…` precisely so a spawn would surface as a build error rather than as a stray
/// child, but the stronger statement MCP-003 wants is that the surface came off DISK — which the
/// description assertion below makes directly.
fn assert_no_socket_or_pidfile(dir: &Path) {
    let stray: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "sock" || x == "pid"))
        .collect();
    assert!(
        stray.is_empty(),
        "the adapter connected nothing at load: {stray:?}"
    );
}

// ================================================================================================
// MCP-001 — the two halves.
// ================================================================================================

/// **Half one.** A session built with a fixture `mcp.json` offers the `mcp` gateway tool, and the
/// description it offers was built from that file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mcp_tool_is_registered_for_a_session_built_with_a_fixture_config() {
    let fx = fixture();
    let (names, description) = tool_registry(&fx, false).await;

    assert!(
        names.iter().any(|n| n == PROXY_TOOL),
        "the `mcp` gateway tool must be in the session's full registry; got: {names:?}"
    );

    // The registry is a SUPERSET, not a replacement: attaching the adapter must not displace the
    // built-ins. A green `mcp` over an otherwise-empty registry would mean the session failed to
    // assemble, not that the adapter worked.
    for builtin in ["read", "bash"] {
        assert!(
            names.iter().any(|n| n == builtin),
            "the built-in `{builtin}` survives the attach; got: {names:?}"
        );
    }

    // THE FIXTURE-WAS-READ PROOF. `buildProxyDescription` step 4 lists disabled servers straight
    // from the merged config, so this string can only be here if `<agent_dir>/mcp.json` was found,
    // parsed, merged and handed to `register_surface` inside `init`.
    let description = description.expect("the `mcp` tool carries a description");
    assert!(
        description.contains(DISABLED_SERVER),
        "the registered description must name the fixture's disabled server — otherwise the tool \
         registered without ever reading `mcp.json`; got: {description}"
    );
    assert!(
        description.contains("Disabled servers (enable with /mcp enable <server> and /reload)"),
        "…through `buildProxyDescription`'s own step-4 sentence; got: {description}"
    );
    // The enabled `lazy` server has no cache entry, so it contributes 0 proxy-reachable tools and
    // is deliberately NOT listed (step 3 `continue`s on `total_items == 0`). Asserting its absence
    // pins that arm: a port that listed every configured server would change the system prompt
    // between a cold and a warm start, which is the exact regression `freezeDirectTools` exists for.
    assert!(
        !description.contains(LAZY_SERVER),
        "an uncached server contributes no `Servers:` entry; got: {description}"
    );

    assert_no_socket_or_pidfile(&fx.agent_dir);
}

/// **Half two.** The same fixture, the same attach, `--no-extensions` — and the `mcp` tool is gone.
///
/// This is `native_survives_no_extensions`'s `if !ext.is_ambient() { return true }` running for
/// real. The built-ins are asserted present in the same breath so that "absent" cannot be read as
/// "the session failed to build".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mcp_tool_is_absent_under_no_extensions() {
    let fx = fixture();
    let (names, description) = tool_registry(&fx, true).await;

    assert!(
        !names.iter().any(|n| n == PROXY_TOOL),
        "`--no-extensions` must switch the ambient MCP adapter off; got: {names:?}"
    );
    assert!(
        description.is_none(),
        "no `mcp` tool means no `mcp` description"
    );
    for builtin in ["read", "bash"] {
        assert!(
            names.iter().any(|n| n == builtin),
            "`--no-extensions` gates EXTENSIONS, not the built-in tools; got: {names:?}"
        );
    }
}

/// The construction gate itself (`mcp_extension_for_env`), asserted where the two session tests
/// cannot reach it: it returns `Some` unconditionally — the adapter is an installed package
/// upstream, present in every session of every mode — and the object it returns is the one whose
/// ambience the gate above consults.
#[test]
fn the_construction_gate_attaches_unconditionally_and_is_ambient() {
    let fx = fixture();
    let ext = cyrup_mcp::mcp_extension_for_env(&fx.agent_dir, None, fx.cwd.clone())
        .expect("`mcp_extension_for_env` never gates — `--no-extensions` is the off switch");
    assert_eq!(ext.id().as_str(), EXTENSION_ID);
    assert!(
        ext.is_ambient(),
        "the adapter must declare pi's INSTALLED-package tier, or `--no-extensions` means two \
         different things in the two products"
    );
    assert!(
        !ext.decides_project_trust(),
        "opting into the pre-trust bootstrap pass would run this non-idempotent `init` twice on \
         the same object (MCP-001)"
    );
}
