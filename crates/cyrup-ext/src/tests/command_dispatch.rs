//! SEAM-048 — the `name:N` disambiguation tier, driven END TO END through the dispatch facade.
//!
//! `aggregation.rs::command_invocation_names_are_disambiguated_in_load_order` and
//! `payload_and_seam_parity.rs::a_second_extension_registering_deploy_is_reachable_as_deploy_2`
//! both stop at the REGISTRY: they assert `resolved_command_owner("deploy:2")` names the second
//! extension and go no further. That is only half of pi's mechanism and it hid the other half.
//!
//! Upstream resolves the invocation name ONCE — `getCommand(name)` matches
//! `command.invocationName` (`core/extensions/runner.ts:647-649` @v0.83.0) — and then calls the
//! BOUND closure it found: `await command.handler(args, ctx)` (`core/agent-session.ts:1283`), and
//! `getArgumentCompletions: cmd.getArgumentCompletions` for the completion half
//! (`modes/interactive/interactive-mode.ts:607`). The registered `name` is never used for a second
//! lookup, so the `:N` suffix never leaves the resolver.
//!
//! cyrup cannot bind a closure across the WIT boundary — the handler is reached by NAME a second
//! time, inside the extension (`SdkApi::execute_command` matches `n == name`,
//! `cyrup-ext-sdk/src/api.rs:1033`; `NativeExtension::execute_command`'s default arm errors on an
//! unknown name, `native.rs:545`). The facade used to forward the INVOCATION name into that second
//! lookup, so `deploy:2` routed to the right owner and then failed inside it with
//! `no such command: deploy:2`: the tier resolved, advertised, and still could not execute.
//!
//! These tests therefore assert on what the HANDLER received and returned, not on what the registry
//! resolved.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::{
    CommandDescriptor, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent, InitApi,
    NativeExtension,
};
use cyrup_core::{CancelToken, ExtensionId};
use std::sync::{Arc, Mutex};

fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: std::path::PathBuf::from(".") }
}

/// A native built-in shaped like the real ones: it registers ONE command under a fixed name and its
/// `execute_command` dispatches by matching that same registered name — which is what
/// `cyrup-intercom` (`src/extension.rs:516`) and every SDK guest
/// (`cyrup-ext-sdk/src/api.rs:1033`) do. An extension has no way to know it collided, so it can
/// only ever answer to the name it registered.
struct DeployExt {
    id: ExtensionId,
    /// Distinguishes the two registrants in the returned payload.
    tag: &'static str,
    /// Every `name` this extension's handler was actually invoked with.
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl NativeExtension for DeployExt {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.register_command(
            "deploy",
            CommandDescriptor { description: "ship it".into(), completions: vec![] },
        );
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    async fn execute_command(
        &self,
        name: &str,
        args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, crate::ExtError> {
        self.seen.lock().unwrap().push(name.to_string());
        if name != "deploy" {
            return Err(crate::ExtError::Component(format!("no such command: {name}")));
        }
        Ok(Some(format!("{}:{args}", self.tag)))
    }
}

async fn two_colliding_deploys() -> (ExtensionHost, Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<String>>>)
{
    let host = ExtensionHost::new(cfg());
    let first = Arc::new(Mutex::new(Vec::new()));
    let second = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(DeployExt { id: "first".into(), tag: "A", seen: first.clone() }))
        .await
        .unwrap();
    host.load_native(Arc::new(DeployExt { id: "second".into(), tag: "B", seen: second.clone() }))
        .await
        .unwrap();
    (host, first, second)
}

/// The end-to-end claim: BOTH registrants of `deploy` are executable, each at its own suffix, and
/// each handler is handed the name it REGISTERED rather than the name the user typed.
#[tokio::test]
async fn each_colliding_deploy_executes_at_its_own_suffix_and_sees_its_registered_name() {
    let (host, first_seen, second_seen) = two_colliding_deploys().await;
    let cancel = CancelToken::new();

    // Guard against a vacuous pass: the suffixes must exist at all before asserting what they do.
    let resolved = host.registry().resolved_commands().unwrap();
    let invocations: Vec<&str> = resolved.iter().map(|r| r.invocation_name.as_str()).collect();
    assert_eq!(invocations, vec!["deploy:1", "deploy:2"], "load-order suffixing, pi's rule");

    let one = host
        .execute_native_command("deploy:1", "prod", &cancel)
        .await
        .expect("routing succeeds")
        .expect("a native extension owns deploy:1");
    assert_eq!(
        one.expect("the handler must not error — it was handed a name it registered"),
        Some("A:prod".to_string()),
        "deploy:1 is the FIRST registrant in load order"
    );

    let two = host
        .execute_native_command("deploy:2", "stage", &cancel)
        .await
        .expect("routing succeeds")
        .expect("a native extension owns deploy:2");
    assert_eq!(
        two.expect("the handler must not error"),
        Some("B:stage".to_string()),
        "deploy:2 is the SECOND registrant — the one last-wins used to make unreachable"
    );

    // The load-bearing half: pi's suffix is a ROUTING key and never reaches the handler. Before the
    // fix these vectors held `["deploy:1"]` / `["deploy:2"]` and both calls came back `Err`.
    assert_eq!(first_seen.lock().unwrap().as_slice(), ["deploy".to_string()]);
    assert_eq!(second_seen.lock().unwrap().as_slice(), ["deploy".to_string()]);
}

/// The BARE name of a collided command is not a command at all upstream: `resolveRegisteredCommands`
/// emits `deploy:1`/`deploy:2` and nothing named `deploy`, so `getCommand("deploy")` returns
/// `undefined` and `_tryExecuteExtensionCommand` returns `false` — the text falls through to a
/// normal prompt (`core/agent-session.ts:1276-1277` @v0.83.0).
///
/// cyrup answered it with the LAST registrant, via a `command_owner` fallback inside
/// `resolved_command_owner`. That fallback was the last-registration-wins defect surviving in the
/// one place the disambiguation tier could not cover, so `/deploy` silently meant "extension B".
#[tokio::test]
async fn the_bare_name_of_a_collided_command_falls_through_instead_of_picking_the_last_registrant() {
    let (host, first_seen, second_seen) = two_colliding_deploys().await;
    let cancel = CancelToken::new();

    // Presence first: `deploy` IS in the raw last-wins map, so `None` below is the resolver
    // declining it rather than the command never having been registered.
    assert!(host.registry().has_command("deploy").unwrap());
    assert_eq!(
        host.registry().command_owner("deploy").unwrap(),
        Some(ExtensionId::from("second")),
        "the raw map still answers `deploy` with the last registrant — that is what must NOT be routed"
    );

    assert_eq!(
        host.registry().resolved_command_owner("deploy").unwrap(),
        None,
        "pi's getCommand matches invocationName only, and no ResolvedCommand is named `deploy`"
    );
    assert!(
        host.execute_native_command("deploy", "prod", &cancel).await.expect("routing succeeds").is_none(),
        "Ok(None) is pi's `false`: the caller falls through to a normal prompt"
    );
    // And no handler ran.
    assert!(first_seen.lock().unwrap().is_empty());
    assert!(second_seen.lock().unwrap().is_empty());
}

/// An UNCOLLIDED command keeps its bare name — `counts.get(name) > 1` is the only thing that
/// triggers a suffix upstream (`runner.ts:606`) — so removing the raw-name fallback must not have
/// cost the ordinary single-registrant case its route.
#[tokio::test]
async fn an_uncollided_command_still_dispatches_under_its_bare_name() {
    let host = ExtensionHost::new(cfg());
    let seen = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(DeployExt { id: "solo".into(), tag: "S", seen: seen.clone() }))
        .await
        .unwrap();

    assert_eq!(
        host.registry()
            .resolved_commands()
            .unwrap()
            .iter()
            .map(|r| r.invocation_name.clone())
            .collect::<Vec<_>>(),
        vec!["deploy".to_string()],
        "one registrant, no suffix"
    );
    let out = host
        .execute_native_command("deploy", "prod", &CancelToken::new())
        .await
        .expect("routing succeeds")
        .expect("the solo extension owns it");
    assert_eq!(out.unwrap(), Some("S:prod".to_string()));
    assert_eq!(seen.lock().unwrap().as_slice(), ["deploy".to_string()]);
}

/// A name nothing registered is `Ok(None)` — pi's `getCommand` -> `undefined` -> `false`.
#[tokio::test]
async fn an_unregistered_command_name_routes_nowhere() {
    let (host, _a, _b) = two_colliding_deploys().await;
    assert!(host
        .execute_native_command("nope", "", &CancelToken::new())
        .await
        .expect("routing succeeds")
        .is_none());
    // A suffix past the end is equally unrouted — `takenInvocationNames` only ever assigns as many
    // as there are registrations.
    assert!(host
        .execute_native_command("deploy:3", "", &CancelToken::new())
        .await
        .expect("routing succeeds")
        .is_none());
}
