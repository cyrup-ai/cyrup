//! SEAM-084 — the `sourceInfo` an extension-registered slash command carries over `get_commands`.
//!
//! Upstream derives the whole object ONCE per extension, in `createExtension`
//! (`pi/packages/coding-agent/src/core/extensions/loader.ts:433-444` @v0.83.0):
//!
//! ```ts
//! const source =
//!     extensionPath.startsWith("<") && extensionPath.endsWith(">")
//!         ? extensionPath.slice(1, -1).split(":")[0] || "temporary"
//!         : "local";
//! const baseDir = extensionPath.startsWith("<") ? undefined : path.dirname(resolvedPath);
//! …
//! sourceInfo: createSyntheticSourceInfo(extensionPath, { source, baseDir }),
//! ```
//!
//! `registerCommand` copies it onto every `RegisteredCommand`, and `rpc-mode.ts:681-686` passes
//! `sourceInfo: command.sourceInfo` straight through. `SourceInfo` is
//! `{path, source, scope, origin, baseDir?}` (`core/source-info.ts:6-12`), with `scope`/`origin`
//! defaulting to `"temporary"`/`"top-level"` (`:36-37`) — which `createExtension` never overrides.
//!
//! What was RED before this pass, at `session.rs`'s extension branch:
//!
//! * `sourceInfo.source` was the literal `"extension"`, a value that exists nowhere upstream;
//! * `sourceInfo.baseDir` was absent even for a filesystem-loaded extension;
//! * `description` was a non-optional `String` and always serialized, emitting `""` where pi's
//!   `description?: string` (`core/extensions/types.ts:1163-1168`) omits the key.
//!
//! `SEAM-055` (closed) fixed only `sourceInfo.path`; these are the three divergences that survived it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use crate::{SessionBuilder, SessionConfig};
use cyrup_core::ExtensionId;
use cyrup_ext::{
    CommandDescriptor, ExtError, ExtensionProvenance, HookOutcome, HostCtx, HostEvent, InitApi,
    NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use serde_json::Value;
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

const OWNER: &str = "provenance-probe";

/// Registers one DESCRIBED and one UNDESCRIBED command. The undescribed one is the whole point of
/// the `description` half: pi's `RegisteredCommand.description` is optional, so its entry has no
/// `description` key at all.
struct TwoCommands;

#[async_trait::async_trait]
impl NativeExtension for TwoCommands {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(OWNER)
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_command(
            "described",
            CommandDescriptor {
                description: "has a description".into(),
                completions: Vec::new(),
            },
        );
        api.register_command(
            "undescribed",
            // The empty string is this port's representation of pi's absent `description?`, which
            // is why the emitter must omit rather than send `""`.
            CommandDescriptor {
                description: String::new(),
                completions: Vec::new(),
            },
        );
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

fn entry<'a>(catalog: &'a [Value], name: &str) -> &'a Value {
    catalog
        .iter()
        .find(|c| c.get("name").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("`{name}` must be in the catalog: {catalog:?}"))
}

/// A native built-in is upstream's inline-factory tier: `loadExtensionFromFactory`'s default
/// `extensionPath` is the literal `"<inline>"` (`loader.ts:490` @v0.83.0), so the `<…>` split gives
/// `source: "inline"` and `baseDir: undefined`.
///
/// RED before this pass on the first and third assertions: `source` was `"extension"`, and the
/// undescribed command carried `"description": ""`.
#[tokio::test]
async fn extension_commands_carry_real_provenance_and_omit_an_absent_description() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    let ext: Arc<dyn NativeExtension> = Arc::new(TwoCommands);
    let session = SessionBuilder::new(faux, cfg)
        .with_native_extension(ext)
        .build()
        .await
        .expect("build");

    let catalog = session.slash_command_catalog();
    let described = entry(&catalog, "described");
    let info = &described["sourceInfo"];

    assert_ne!(
        info["source"], "extension",
        "`\"extension\"` is not a value pi's SourceInfo.source ever takes: {described}"
    );
    assert_eq!(
        info["source"], "inline",
        "a host-loaded native is upstream's `<inline>` tier (loader.ts:490 @v0.83.0): {described}"
    );
    assert!(
        info.get("baseDir").is_none(),
        "a SYNTHETIC extension has no baseDir (`loader.ts:438`): {described}"
    );

    // Presence before absence: SEAM-055's `path` and the two `createSyntheticSourceInfo` defaults
    // must survive this change, and the TOP-LEVEL `source` is a different field (pi's
    // `SlashCommandSource`, `core/slash-commands.ts:4`) that really is `"extension"`.
    assert_eq!(
        info["path"], OWNER,
        "SEAM-055's owner id must not regress: {described}"
    );
    assert_eq!(info["scope"], "temporary");
    assert_eq!(info["origin"], "top-level");
    assert_eq!(described["source"], "extension");
    assert_eq!(described["description"], "has a description");

    let undescribed = entry(&catalog, "undescribed");
    assert!(
        undescribed.get("description").is_none(),
        "pi's `description?: string` omits the key for an undescribed command, never `\"\"`: \
         {undescribed}"
    );
}

/// The FILESYSTEM arm of the same derivation: `source: "local"` and `baseDir` = the extension's own
/// directory (`loader.ts:437-438` @v0.83.0). The provenance is recorded here rather than by loading a
/// real component so the assertion is about the EMITTER, not about the WASM tier — the discovery path
/// records exactly this value at `cyrup-ext/src/facade.rs`'s `load_discovered`.
///
/// RED before this pass on both assertions: `source` was the hard-coded `"extension"` and no
/// `baseDir` key was ever emitted, for any extension.
#[tokio::test]
async fn a_filesystem_extension_reports_local_and_its_base_dir() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    let ext: Arc<dyn NativeExtension> = Arc::new(TwoCommands);
    let session = SessionBuilder::new(faux, cfg)
        .with_native_extension(ext)
        .build()
        .await
        .expect("build");

    let dir = fx.cwd.join(".cyrup").join("extensions").join("probe");
    session
        .ext_host()
        .registry()
        .record_extension_provenance(
            ExtensionId::from(OWNER),
            ExtensionProvenance::local(dir.to_string_lossy().into_owned()),
        )
        .expect("recording provenance must not poison the registry");

    let catalog = session.slash_command_catalog();
    let info = &entry(&catalog, "described")["sourceInfo"];
    assert_eq!(
        info["source"], "local",
        "a filesystem extension is pi's `else \"local\"` branch"
    );
    assert_eq!(
        info["baseDir"].as_str(),
        Some(dir.to_string_lossy().as_ref()),
        "baseDir is the extension's directory — what a client resolves its assets against"
    );
}
