//! Dropping an [`ExtensionHost`] frees every guest it loaded — the guest's store, its pooling
//! instance slot, and through the store the Wasmtime `Engine` itself.
//!
//! It did not. Two reference cycles ran through each guest's `GuestState` (owned by its store):
//!
//! * registry: `GuestState` -> the host's `ExtensionRegistry` -> the guest's materialized
//!   `WasmTool` -> `Arc<LiveExtension>` -> store -> `GuestState`, closed by any guest that
//!   registers a tool;
//! * dispatcher: `GuestState` -> `DispatcherProviderReduction` -> `Arc<Dispatcher>` -> the guest's
//!   `Arc<LiveExtension>` -> store -> `GuestState`, closed by EVERY loaded guest.
//!
//! Each leaked host kept its engine's pooling reservations and async-stack guard pages mapped,
//! about 2,000 mappings apiece, so the `cyrup-it` `ext` suite run in one process (`cargo test`, not
//! nextest's process-per-test) hit `vm.max_map_count` around its 33rd host and aborted with "failed
//! to set up alternative stack guard page" / "failed to protect stack guard page". A session that
//! rebuilds its host (session swap, `/reload`) leaked the same way.
//!
//! The assertion is on the instance itself (a `Weak<LiveExtension>` that must not upgrade once the
//! host is gone), not on `/proc/self/maps`: the mapping count is process-wide and other tests in
//! this binary build engines concurrently, while the `Weak` is exact and deterministic.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::{Arc, Weak};

use crate::host::LiveExtension;
use crate::tests::wat_guest::{Lowered, WatGuest};
use crate::{DenyServices, ExtMode, ExtensionHost, HostConfig};

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

const TOOL: &str = "held_tool";

/// The canonical-ABI `tool-descriptor` record (136 bytes): `name` at 0 and `parameters-json` at 24
/// point into the data segments below; every other member is empty / `none` / `false`.
fn record() -> String {
    let mut words = [0u32; 34];
    words[0] = 17200;
    words[1] = TOOL.len() as u32;
    words[6] = 17220;
    words[7] = 2;
    words
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .map(|b| format!("\\{b:02x}"))
        .collect()
}

/// A real component whose `init` registers one tool, so the host materializes a `WasmTool` bound
/// to the live instance (the registry cycle).
fn tool_guest() -> Vec<u8> {
    WatGuest {
        component: super::tool_exposure::guest_registration_tool_import(),
        lowered: vec![Lowered {
            core_name: "register_tool",
            component_func: "$register-tool",
            core_sig: "(param i32 i32)",
            needs_realloc: true,
        }],
        overrides: vec![(
            "init",
            "    (func (export \"init\") (result i32) \
             (call $register_tool (i32.const 17000) (i32.const 16512)) \
             i32.const 16)"
                .to_string(),
        )],
        data: vec![
            (17000, record()),
            (17200, TOOL.to_string()),
            (17220, "{}".to_string()),
        ],
    }
    .build()
}

/// Load `bytes` into a fresh host, drop the host, and hand back a `Weak` to the instance. Returns
/// whether the guest's tool was materialized, so a test can prove it exercised the cycle it names.
async fn load_then_drop_host(bytes: &[u8]) -> (Weak<LiveExtension>, bool) {
    let host = ExtensionHost::with_wasm(cfg()).expect("host with wasm runtime");
    let ext = host
        .load_wasm("guest".into(), bytes, Arc::new(DenyServices))
        .await
        .expect("guest loads");
    let materialized = host.registry().tool(TOOL).unwrap().is_some();
    let weak = Arc::downgrade(&ext);
    drop(ext);
    drop(host);
    (weak, materialized)
}

/// The dispatcher cycle: a guest that registers nothing is still freed with its host.
#[tokio::test]
async fn a_dropped_host_frees_a_guest_that_registered_nothing() {
    let (weak, materialized) = load_then_drop_host(&WatGuest::default().build()).await;
    assert!(!materialized, "this guest registers no tool");
    assert_eq!(
        weak.strong_count(),
        0,
        "the guest's LiveExtension (and the store and Engine it owns) outlived its ExtensionHost"
    );
}

/// The registry cycle: a guest whose tool the host materialized is freed with its host too.
#[tokio::test]
async fn a_dropped_host_frees_a_guest_whose_tool_it_materialized() {
    let (weak, materialized) = load_then_drop_host(&tool_guest()).await;
    assert!(
        materialized,
        "the guest's tool must be materialized for this test to mean anything"
    );
    assert_eq!(
        weak.strong_count(),
        0,
        "the guest's LiveExtension (and the store and Engine it owns) outlived its ExtensionHost"
    );
}

/// Building and dropping hosts in a loop does not accumulate instances: the shape that exhausted
/// `vm.max_map_count` in the one-process `ext` suite.
#[tokio::test]
async fn hosts_built_and_dropped_in_a_loop_leave_no_guest_alive() {
    let bytes = tool_guest();
    let mut survivors = 0usize;
    for _ in 0..8 {
        let (weak, _) = load_then_drop_host(&bytes).await;
        survivors += usize::from(weak.strong_count() > 0);
    }
    assert_eq!(
        survivors, 0,
        "{survivors} of 8 dropped hosts left their guest alive"
    );
}
