//! TUI-076 — an extension command that collides with a builtin stays out of the `/` menu even
//! after the runner suffixes it.
//!
//! pi builds the menu's extension block as
//! `getRegisteredCommands().filter((cmd) => !builtinCommandNames.has(cmd.name)).map((cmd) => ({
//! name: cmd.invocationName, … }))` (`interactive-mode.ts:748-753` @v0.87.1): filter on the ORIGINAL
//! name, then map to the invocation name. Two extensions both registering `model` become
//! `model:1`/`model:2` (`runner.ts:757`), and pi drops BOTH. cyrup's catalog `name` is already the
//! invocation name, so filtering on it let both suffixed rows through as commands no dispatch arm
//! routes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::ExtensionId;
use cyrup_ext::{
    CommandDescriptor, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

/// Registers each of `commands` under its own extension id.
struct Registers {
    id: &'static str,
    commands: &'static [&'static str],
}

#[async_trait::async_trait]
impl NativeExtension for Registers {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(self.id)
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        for name in self.commands {
            api.register_command(
                *name,
                CommandDescriptor {
                    description: format!("{} from {}", name, self.id),
                    completions: Vec::new(),
                },
            );
        }
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// The in-process catalog the TUI reads, and the wire catalog RPC `get_commands` and a guest's
/// `get-commands` read.
async fn catalogs_with(
    extensions: Vec<Registers>,
) -> (TempDir, Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let tmp = TempDir::new().unwrap();
    let cwd: PathBuf = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    let mut builder = SessionBuilder::new(provider, cfg);
    for ext in extensions {
        let ext: Arc<dyn NativeExtension> = Arc::new(ext);
        builder = builder.with_native_extension(ext);
    }
    let session = builder.build().await.expect("build");
    let catalog = session.slash_command_catalog();
    let wire = session.slash_command_catalog_wire();
    (tmp, catalog, wire)
}

async fn catalog_with(extensions: Vec<Registers>) -> (TempDir, Vec<serde_json::Value>) {
    let (tmp, catalog, _wire) = catalogs_with(extensions).await;
    (tmp, catalog)
}

fn names(registry: &crate::CommandRegistry) -> Vec<String> {
    registry
        .commands()
        .iter()
        .map(|c| c.name.to_string())
        .collect()
}

/// The ledger's Verify, through the real producer and the real menu builder: two extensions both
/// register `model`, the runner suffixes them, and neither suffixed row reaches `commands()`. A
/// non-colliding command from the same extensions still does.
#[tokio::test]
async fn suffixed_builtin_collisions_are_absent_from_the_slash_menu() {
    let (_tmp, catalog) = catalog_with(vec![
        Registers {
            id: "first",
            commands: &["model", "deploy"],
        },
        Registers {
            id: "second",
            commands: &["model"],
        },
    ])
    .await;

    // The producer really did suffix them — otherwise this test proves nothing about the filter.
    let catalog_names: Vec<&str> = catalog
        .iter()
        .filter_map(|r| r.get("name").and_then(serde_json::Value::as_str))
        .collect();
    assert!(
        catalog_names.contains(&"model:1") && catalog_names.contains(&"model:2"),
        "fixture must produce suffixed invocation names: {catalog_names:?}"
    );

    let registry = crate::CommandRegistry::with_dynamic(
        crate::dynamic_commands_from_catalog_gated(&catalog, true),
    );
    let menu = names(&registry);
    assert!(
        !menu.iter().any(|n| n.starts_with("model:")),
        "a suffixed builtin collision must not be offered: {menu:?}"
    );
    assert_eq!(
        menu.iter().filter(|n| *n == "model").count(),
        1,
        "the builtin `/model` stays, once: {menu:?}"
    );
    assert!(
        menu.contains(&"deploy".to_string()),
        "a non-colliding extension command is unaffected: {menu:?}"
    );
}

/// The key is cyrup-original and only crosses when it carries information: a command whose
/// invocation name IS its registered name keeps pi's RPC row shape exactly.
#[tokio::test]
async fn registered_name_is_emitted_only_for_a_suffixed_command() {
    let (_tmp, catalog) = catalog_with(vec![
        Registers {
            id: "first",
            commands: &["model", "deploy"],
        },
        Registers {
            id: "second",
            commands: &["model"],
        },
    ])
    .await;
    let row = |name: &str| {
        catalog
            .iter()
            .find(|r| r.get("name").and_then(serde_json::Value::as_str) == Some(name))
            .unwrap_or_else(|| panic!("`{name}` missing: {catalog:?}"))
    };
    assert_eq!(row("model:1")["registeredName"], "model");
    assert_eq!(row("model:2")["registeredName"], "model");
    assert!(
        row("deploy").get("registeredName").is_none(),
        "an unsuffixed row omits the key: {}",
        row("deploy")
    );
}

/// The key never leaves the process: the wire catalog (RPC `get_commands`, the guest
/// `get-commands` import) is pi's `RpcSlashCommand` row shape exactly (`rpc-mode.ts:687`
/// @v0.87.1), and otherwise the same rows in the same order.
#[tokio::test]
async fn registered_name_is_stripped_from_the_wire_catalog() {
    let (_tmp, catalog, wire) = catalogs_with(vec![
        Registers {
            id: "first",
            commands: &["model", "deploy"],
        },
        Registers {
            id: "second",
            commands: &["model"],
        },
    ])
    .await;
    assert!(
        catalog.iter().any(|r| r.get("registeredName").is_some()),
        "fixture must produce a suffixed row: {catalog:?}"
    );
    assert!(
        wire.iter().all(|r| r.get("registeredName").is_none()),
        "the wire rows carry no cyrup-only key: {wire:?}"
    );
    let stripped: Vec<serde_json::Value> = catalog
        .into_iter()
        .map(|mut r| {
            if let Some(o) = r.as_object_mut() {
                o.remove("registeredName");
            }
            r
        })
        .collect();
    assert_eq!(wire, stripped, "only the one key differs");
}
