//! EXT-093 — a NATIVE extension reads the value of a flag it declared.
//!
//! pi's `pi.getFlag(name)` is on the API every inline factory receives
//! (`packages/coding-agent/src/core/extensions/loader.ts:366-370` @HEAD v1.0.0-25, identical at
//! tag v0.99.2): `undefined` unless this extension registered the flag, else the CLI override
//! (`runtime.flagValues`) or the registered default. cyrup's native `InitApi::register_flag` could
//! declare a flag but nothing could READ it: `GuestState::get_flag` is reachable only from a WASM
//! guest, and the CLI overrides are applied (`apply_extension_flag_values`) only AFTER every native
//! `init` has run, so the only way for a native to learn its own flag was to re-parse argv.
//!
//! `HostServices::flag_value` is the missing read. These tests drive the whole production chain:
//! a native registers a flag in `init`, the CLI override is captured in `SessionConfig`, the builder
//! applies it, and the native's `execute_command` — reached through the real `prompt("/readflag …")`
//! entry point — reads the answer through the `Arc<dyn HostServices>` the builder bound to it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use crate::{ExtensionFlagValue, NotifyKind, SessionBuilder, SessionConfig, UiEffect};
use cyrup_core::ExtensionId;
use cyrup_ext::{
    CommandDescriptor, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi,
    NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedReceiver;

/// Declares one string flag with a default, one boolean flag with none, and `/readflag <name>`,
/// which answers with what `HostServices::flag_value` returns for `<name>`.
struct FlagReader {
    services: OnceLock<Arc<dyn HostServices>>,
}

#[async_trait::async_trait]
impl NativeExtension for FlagReader {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("flag-reader")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        let _ = self.services.set(services);
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_flag(
            "probe-path",
            json!({"type": "string", "default": "from-default", "description": "a path"}),
        );
        api.register_flag(
            "probe-switch",
            json!({"type": "boolean", "description": "a switch with no default"}),
        );
        api.register_command(
            "readflag",
            CommandDescriptor {
                description: "report a flag's value".into(),
                completions: Vec::new(),
            },
        );
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    async fn execute_command(
        &self,
        _name: &str,
        args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        let services = self
            .services
            .get()
            .ok_or_else(|| ExtError::Component("no host services were bound".into()))?;
        Ok(Some(format!("{:?}", services.flag_value(args.trim()))))
    }
}

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

/// Build a session whose CLI captured `flags`, and run `/readflag <name>` for each of `names`,
/// returning the handler's answers in order.
async fn read_flags(flags: &[(&str, ExtensionFlagValue)], names: &[&str]) -> Vec<String> {
    let fx = fixture();
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg.extension_flag_values = flags
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .with_native_extension(Arc::new(FlagReader {
            services: OnceLock::new(),
        }))
        .build()
        .await
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    session.services().host_services.set_ui_effect_sink(tx);

    let mut answers = Vec::new();
    for name in names {
        let _ = session.prompt(format!("/readflag {name}")).await.unwrap();
        session.wait_for_idle().await;
        answers.push(drain_one(&mut rx));
    }
    answers
}

fn drain_one(rx: &mut UnboundedReceiver<UiEffect>) -> String {
    let mut notes = Vec::new();
    while let Ok(effect) = rx.try_recv() {
        if let UiEffect::Notify { message, kind } = effect {
            assert_eq!(kind, NotifyKind::Info, "{message}");
            notes.push(message);
        }
    }
    assert_eq!(notes.len(), 1, "one command, one answer: {notes:?}");
    notes.remove(0)
}

/// THE ROW'S VERIFY: a native registers a flag, the CLI sets it, and `execute_command` reads the
/// override rather than the default (pi `runtime.flagValues.has(name) ? … : pendingFlagValues`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_native_reads_the_cli_override_of_its_flag() {
    let answers = read_flags(
        &[("probe-path", ExtensionFlagValue::Str("/from/cli".into()))],
        &["probe-path"],
    )
    .await;

    assert_eq!(answers, vec![r#"Some(String("/from/cli"))"#]);
}

/// With no CLI override the registered default answers (pi seeds `flagValues` from
/// `options.default`), a flag with no default and no override is `None` (pi `undefined`), and a
/// boolean CLI flag reads `true`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_an_override_the_default_answers_and_a_defaultless_flag_is_none() {
    let answers = read_flags(&[], &["probe-path", "probe-switch"]).await;
    assert_eq!(answers, vec![r#"Some(String("from-default"))"#, "None"]);

    let answers = read_flags(
        &[("probe-switch", ExtensionFlagValue::Bool(true))],
        &["probe-switch"],
    )
    .await;
    assert_eq!(answers, vec!["Some(Bool(true))"]);
}

/// A flag nobody registered reads `None` — pi's `if (!extension.flags.has(name)) return undefined`
/// (`loader.ts:368`) — even when the CLI captured a value under that name (the builder reports the
/// unknown option; the value never becomes readable).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unregistered_flag_reads_none() {
    let answers = read_flags(
        &[("nobody-owns-this", ExtensionFlagValue::Str("x".into()))],
        &["nobody-owns-this"],
    )
    .await;

    assert_eq!(answers, vec!["None"]);
}
