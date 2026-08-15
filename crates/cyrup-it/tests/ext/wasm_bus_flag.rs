//! LIVE assembled-host reproduction + fix for the two dead-but-advertised gap-08 findings:
//!
//! (A) Cross-extension `EventBus` fan-out (§5.3). A real `.wasm` guest B `bus.subscribe`s a topic;
//!     a real `.wasm` guest A `bus.emit`s it. On the pre-fix mechanism the emit landed in a private
//!     per-guest log and reached NOTHING — this test asserts guest B does NOT receive after the emit
//!     alone (the dead-but-advertised state), then drives the fix (`ExtensionHost::deliver_bus_events`
//!     / `run_command`'s tail drain) and asserts B DOES receive.
//!
//! (B) `getFlag` CLI-override (§5.6). A real `.wasm` guest registers `--demo-flag` (default "off")
//!     and reads it via `getFlag`. Before applying the captured CLI override the guest reads the
//!     static default (the CLI value dropped one call short of `getFlag`); after
//!     `ExtensionHost::apply_extension_flag_values` the guest reads the CLI value.
//!
//! Both drive the REAL production entry points (`load_wasm` + `run_command` + the two new host
//! methods) against a live `wasm32-wasip2` COMPONENT — the assembled-product discipline the audit
//! demands, not a hand-built stub.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_core::CancelToken;
use cyrup_ext::{DenyServices, ExtMode, ExtensionFlagOverride, ExtensionHost, HostConfig};
use std::path::PathBuf;
use std::sync::Arc;

/// The `wasm32-wasip2` guest component. Built ONCE for the whole suite by
/// `crates/cyrup-it/build.rs` and handed over as `CYRUP_IT_COMPONENT`; this replaces the nested
/// `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` that used to live here.
/// `CYRUP_EXT_FIXTURE_COMPONENT` still overrides it — now read in one place instead of thirteen.
fn fixture_component() -> PathBuf {
    crate::support::bins::component()
}

fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: PathBuf::from(".") }
}

/// (A) The inter-extension event bus: emit from guest A reaches a subscribed handler in guest B.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_extension_bus_emit_reaches_a_subscribed_handler() {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component bytes");
    let host = ExtensionHost::with_wasm(cfg()).expect("host with wasm runtime");
    let cancel = CancelToken::new();

    // Two DISTINCT loaded extensions from the same component: A ("pub") and B ("sub"). Both declared
    // `bus.subscribe("demo:bus")` during their own `init` (the demo's `on_bus` handler).
    let ext_a =
        host.load_wasm("pub".into(), &bytes, Arc::new(DenyServices)).await.expect("load guest A");
    let ext_b =
        host.load_wasm("sub".into(), &bytes, Arc::new(DenyServices)).await.expect("load guest B");

    // Guest A emits `demo:bus` by running its `/buspub` command DIRECTLY on the A handle (bypassing
    // `run_command`'s tail drain) so we can observe the pre-delivery state — exactly the pre-fix
    // behavior where emit was the only thing that happened.
    let out = ext_a.execute_command("buspub", "hello", &cancel).await.expect("buspub ran");
    assert_eq!(out.as_deref(), Some("emitted demo:bus: hello"));

    // The emit genuinely fired (recorded in guest A's own per-guest log) ...
    assert!(
        ext_a.guest().bus_emits().iter().any(|(t, _)| t == "demo:bus"),
        "guest A actually emitted demo:bus"
    );
    // ... but WITHOUT the host fan-out step, guest B has received NOTHING — the dead-but-advertised
    // state: a published event reaches no subscriber (RED).
    assert!(
        !ext_b.guest().notifications().iter().any(|n| n.contains("bus recv")),
        "RED: guest B must NOT have received the bus event before delivery, got {:?}",
        ext_b.guest().notifications()
    );

    // Drive the fix: the host fans queued bus events out to every subscribed guest's `bus-deliver`.
    host.deliver_bus_events(&cancel).await;

    // GREEN: guest B's subscribed handler ran and surfaced the emitted payload.
    assert!(
        ext_b
            .guest()
            .notifications()
            .iter()
            .any(|n| n == "bus recv demo:bus: hello"),
        "GREEN: guest B received the cross-extension bus event, got {:?}",
        ext_b.guest().notifications()
    );
}

/// (A, production path) `run_command`'s tail drain delivers automatically — the assembled product
/// needs no manual `deliver_bus_events` call. `/buspub` routes to its owner, emits, and the drain
/// fans it out to every subscriber (both guests) before `run_command` returns.
///
/// INVOCATION NAME, not the registered name. Both loaded guests are the same component, so BOTH
/// register a command named `buspub`; pi's `resolveRegisteredCommands` therefore emits `buspub:1`
/// and `buspub:2` in load order and NOTHING named `buspub`, and `getCommand(name)` matches
/// `command.invocationName` alone —
/// `packages/coding-agent/src/core/extensions/runner.ts:596-628` and `:647-649` @v0.83.0, ported at
/// `cyrup-ext/src/registry.rs:644` / `facade.rs:1642`. This test predates that port (SEAM-048 /
/// EXT-017) and still asked for the bare `buspub`, which upstream resolves to `undefined`; because
/// the `it` suite is feature-gated off by default, nothing re-ran it after the port landed and it
/// has been red-in-waiting ever since. Asking for `buspub:1` is what a pi user's `/buspub:1` does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_command_auto_delivers_bus_events() {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component bytes");
    let host = ExtensionHost::with_wasm(cfg()).expect("host with wasm runtime");
    let cancel = CancelToken::new();

    let ext_a =
        host.load_wasm("pub".into(), &bytes, Arc::new(DenyServices)).await.expect("load guest A");
    let ext_b =
        host.load_wasm("sub".into(), &bytes, Arc::new(DenyServices)).await.expect("load guest B");

    // Pin the disambiguation itself before using it, so a regression that reinstates the old
    // last-registration-wins raw-name fallback fails HERE rather than silently making the drain
    // assertion below pass through the wrong door. Under a collision pi leaves no bare name.
    let bare = host.run_command("buspub", "ping", &cancel).await;
    assert!(
        matches!(&bare, Err(e) if e.to_string().contains("no such command: buspub")),
        "with two extensions registering `buspub`, pi resolves only `buspub:1`/`buspub:2` and \
         `getCommand(\"buspub\")` is undefined (runner.ts:596-628,647-649 @v0.83.0); got {bare:?}"
    );

    // Production slash-command path: no explicit drain call.
    let out = host.run_command("buspub:1", "ping", &cancel).await.expect("run_command buspub:1");
    assert_eq!(out.as_deref(), Some("emitted demo:bus: ping"));

    // BOTH guests (all subscribers, Pi delivers to every listener incl. the emitter) received it.
    for (label, ext) in [("A", &ext_a), ("B", &ext_b)] {
        assert!(
            ext.guest().notifications().iter().any(|n| n == "bus recv demo:bus: ping"),
            "guest {label} received the auto-delivered bus event, got {:?}",
            ext.guest().notifications()
        );
    }
}

/// (B) `getFlag` reads the CLI override applied via `apply_extension_flag_values`, following Pi's
/// `applyExtensionFlagValues` type rules; before the apply it reads the registered default.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_flag_reflects_the_applied_cli_override() {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component bytes");
    let host = ExtensionHost::with_wasm(cfg()).expect("host with wasm runtime");
    let cancel = CancelToken::new();

    host.load_wasm("flagger".into(), &bytes, Arc::new(DenyServices)).await.expect("load guest");

    // RED: no override applied — the guest's getFlag reads the registered default "off".
    let out = host.run_command("flagdemo", "", &cancel).await.expect("flagdemo ran");
    assert_eq!(
        out.as_deref(),
        Some("flag demo-flag = off"),
        "RED: getFlag reads the static default before any CLI override is applied"
    );

    // An UNREGISTERED flag is ignored (Pi records an "Unknown option" diagnostic + skips): no effect.
    host.apply_extension_flag_values(&[(
        "not-a-real-flag".into(),
        ExtensionFlagOverride::Str("x".into()),
    )])
    .expect("apply ignores an unregistered flag");
    let out = host.run_command("flagdemo", "", &cancel).await.expect("flagdemo ran");
    assert_eq!(out.as_deref(), Some("flag demo-flag = off"), "an unregistered flag has no effect");

    // A bare `--demo-flag` (no value) on a STRING-typed flag is skipped (Pi's "requires a value"):
    // the registered default still stands.
    host.apply_extension_flag_values(&[("demo-flag".into(), ExtensionFlagOverride::Bool(true))])
        .expect("apply a bare bool on a string flag");
    let out = host.run_command("flagdemo", "", &cancel).await.expect("flagdemo ran");
    assert_eq!(
        out.as_deref(),
        Some("flag demo-flag = off"),
        "a bare --demo-flag on a string flag requires a value (Pi) — default stands"
    );

    // GREEN: `--demo-flag=on` overrides the default; the guest's getFlag now reads "on".
    host.apply_extension_flag_values(&[("demo-flag".into(), ExtensionFlagOverride::Str("on".into()))])
        .expect("apply the string override");
    let out = host.run_command("flagdemo", "", &cancel).await.expect("flagdemo ran");
    assert_eq!(
        out.as_deref(),
        Some("flag demo-flag = on"),
        "GREEN: getFlag reads the CLI-supplied override value"
    );
}
