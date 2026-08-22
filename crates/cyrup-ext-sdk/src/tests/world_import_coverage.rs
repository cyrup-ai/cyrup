//! EXT-M04 / EXT-M05 — every IMPORT the `cyrup:ext` world declares has a caller in this SDK.
//!
//! The world is a two-sided contract, and the two sides fail differently. The HOST side cannot
//! silently skip an import: `wasmtime::component::bindgen!` generates a trait and a missing member
//! is an `E0046`/`E0407` — which is precisely how the mid-flight `interface ui` change announced
//! itself. The GUEST side has no such forcing function: `wit-bindgen` emits a free function per
//! import, and a function nobody calls is not a warning, not an error, not anything. So an import
//! can be designed, declared in BOTH `world.wit` copies, implemented host-side, wired through the
//! facade, covered by host tests, and documented in the world's own comments as the thing that
//! makes some pi API expressible — and still be unreachable by any extension, because the SDK
//! never gave an author a way to call it.
//!
//! Two imports were in exactly that state when this test was written, both landed by passes that
//! closed their host halves and recorded the item as done:
//!
//! * `ui.unsubscribe-terminal-input` (EXT-M04) — the declared counterpart of pi's
//!   `onTerminalInput(handler): () => void` RETURN value. The world comment for the pair says in as
//!   many words "The returned unsubscribe function is `unsubscribe-terminal-input`". `guest::init`
//!   called `subscribe_terminal_input()` and nothing, anywhere, called its opposite.
//! * `provider-stream.on-payload` / `on-response` (EXT-M05) — the `streamSimple` MUST-INVOKE
//!   contract quoted verbatim in the world. `ProviderStream::emit` was the type's only method, so a
//!   guest provider could not have honoured the contract if its author had tried.
//!
//! This is the guest-side twin of the EXT-023 class (a descriptor field the host lifts and never
//! reads), and it hides the same way. So the check is structural and covers the whole world at
//! once: the next import added to any interface cannot land half-wired.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

const WORLD: &str = include_str!("../../wit/world.wit");

/// Every `.rs` file in this crate that may hold a binding call, concatenated.
const SDK_SOURCES: &str = concat!(
    include_str!("../ctx/base.rs"),
    include_str!("../ctx/command.rs"),
    include_str!("../ctx/exec.rs"),
    include_str!("../ctx/fs.rs"),
    include_str!("../ctx/http.rs"),
    include_str!("../ctx/mod.rs"),
    include_str!("../ctx/models.rs"),
    include_str!("../ctx/proc.rs"),
    include_str!("../ctx/session.rs"),
    include_str!("../ctx/tool_call.rs"),
    include_str!("../ctx/tools.rs"),
    include_str!("../ctx/ui.rs"),
    include_str!("../ctx/with_session.rs"),
    include_str!("../guest.rs"),
    include_str!("../provider.rs"),
    include_str!("../api.rs"),
    include_str!("../widget.rs"),
);

/// `SDK_SOURCES` is a hand-maintained `include_str!` list, and `src/ctx/` is a DIRECTORY of one
/// submodule per WIT import interface. A new submodule nobody adds to the list puts its binding
/// calls outside the coverage check below — and `exec`, `ext-fs`, `http-client` and `proc` are
/// called from `ctx` and nowhere else, so the whole of four interfaces rides on that list being
/// complete. Containment is checked on CONTENT, not on the file name, because `include_str!`
/// inlines the content and only the content proves the file is actually in there.
///
/// This is the same shape as [`every_world_interface_is_classified`] one level down: a guard whose
/// job is to notice the thing nobody is looking at.
#[test]
fn every_ctx_submodule_is_in_sdk_sources() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ctx"));
    let mut missing: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    for entry in std::fs::read_dir(dir).expect("src/ctx is a directory") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        scanned += 1;
        let body = std::fs::read_to_string(&path).expect("readable source file");
        let probe: String = body.chars().take(200).collect();
        if !probe.is_empty() && !SDK_SOURCES.contains(probe.as_str()) {
            missing.push(path.display().to_string());
        }
    }
    // Non-vacuity: a scan that finds nothing satisfies the containment loop trivially, so the COUNT
    // is the only thing that proves this guard did any work. The literal is one per submodule plus
    // `mod.rs` rather than `> 0` so that REMOVING a submodule is deliberate too: dropping the file
    // and its `include_str!` line together leaves the containment loop passing over a smaller tree,
    // and only the count notices. (The opposite slip — dropping the file but keeping the
    // `include_str!` — needs no guard here; it does not compile.)
    assert!(
        scanned >= 13,
        "the `src/ctx/` scan found only {scanned} `.rs` file(s) — this guard would be vacuous"
    );
    assert!(
        missing.is_empty(),
        "these `src/ctx/` submodules are not in SDK_SOURCES, so their binding calls are invisible \
         to `every_declared_world_import_has_a_caller_in_the_sdk`: {missing:?}"
    );
}

/// The `func` names declared inside `interface <name> { … }`.
fn declared_funcs(interface: &str) -> Vec<String> {
    let open = format!("\ninterface {interface} {{");
    let body = WORLD
        .split_once(open.as_str())
        .map(|(_, rest)| rest.split("\n}\n").next().unwrap_or(rest))
        .unwrap_or_else(|| panic!("`interface {interface} {{` block present in wit/world.wit"));
    // Non-vacuity: a column-0 `interface` inside the slice means the `\n}\n` split over-ran the
    // closing brace and swallowed the rest of the world. (`interface` in a COMMENT is fine — the
    // prose mentions several — hence anchoring on the newline.)
    assert!(
        !body.contains("\ninterface "),
        "the extracted `interface {interface}` body over-ran its closing brace, so this test would be vacuous"
    );

    body.lines()
        .filter_map(|line| {
            // A declaration is `  <kebab-name>: func(…)` at exactly one indent level; comments
            // start with `//` after trimming, and enum/record members carry no `func`.
            let trimmed = line.trim_start();
            if !line.starts_with("  ") || trimmed.starts_with("//") {
                return None;
            }
            let (name, rest) = trimmed.split_once(':')?;
            rest.trim_start().starts_with("func").then(|| name.trim().to_string())
        })
        .collect()
}

/// Whether the SDK calls `interface::func`.
///
/// Calls are matched on the FULLY QUALIFIED path (`ui::notify(`) or, when the file introduced a
/// module alias for that interface (`use …::ext::control as c;`), on the aliased path (`c::switch(`).
/// Matching a bare `func(` would be far too loose to mean anything — an unrelated `emit(` would
/// satisfy `provider-stream.emit-event` and the test would pass for the wrong reason.
fn sdk_calls(interface: &str, func: &str) -> bool {
    let module = interface.replace('-', "_");
    let name = func.replace('-', "_");
    if SDK_SOURCES.contains(&format!("{module}::{name}(")) {
        return true;
    }
    // `use crate::guest::bindings::cyrup::ext::<module> as <alias>;`
    let alias_marker = format!("::ext::{module} as ");
    SDK_SOURCES.split(alias_marker.as_str()).skip(1).any(|rest| {
        let alias = rest.split(';').next().unwrap_or("").trim();
        !alias.is_empty() && SDK_SOURCES.contains(&format!("{alias}::{name}("))
    })
}

/// The world's IMPORT interfaces — everything except `events`, which is the guest's EXPORT surface
/// (its functions are implemented by the `export_extension!` macro, not called).
/// Kept in sync with the world by [`every_world_interface_is_classified`] below, so adding an
/// interface cannot quietly exclude it from the coverage check.
const IMPORT_INTERFACES: &[&str] = &[
    "ui",
    "bus",
    "provider-stream",
    "host-tool",
    "oauth",
    "control",
    "session",
    "models",
    "registration",
    "ext-tools",
    "ctx-state",
    "exec",
    "proc",
    "http-client",
    "ext-fs",
];

/// `types` declares no functions; `events` is the guest's EXPORT surface, whose members are
/// IMPLEMENTED by `export_extension!` rather than called.
const NON_IMPORT_INTERFACES: &[&str] = &["types", "events"];

/// Every `interface` in the world is either checked for callers or explicitly exempted. Without
/// this, a new interface would be silently outside the coverage test — the same "nobody is looking"
/// shape the test exists to catch, one level up.
#[test]
fn every_world_interface_is_classified() {
    for line in WORLD.lines() {
        let Some(rest) = line.strip_prefix("interface ") else { continue };
        let name = rest.split_whitespace().next().unwrap_or("");
        assert!(
            IMPORT_INTERFACES.contains(&name) || NON_IMPORT_INTERFACES.contains(&name),
            "`interface {name}` is in world.wit but neither checked for SDK callers nor exempted — \
             add it to IMPORT_INTERFACES (or to NON_IMPORT_INTERFACES with the reason)"
        );
    }
}

#[test]
fn every_declared_world_import_has_a_caller_in_the_sdk() {
    // Assert PRESENCE before absence: prove the parse actually found the two imports this test was
    // written for, so a parse that silently yields nothing cannot pass.
    assert!(
        declared_funcs("ui").iter().any(|n| n == "unsubscribe-terminal-input"),
        "the world parse lost `ui.unsubscribe-terminal-input` (EXT-M04)"
    );
    assert!(
        declared_funcs("provider-stream").iter().any(|n| n == "on-payload"),
        "the world parse lost `provider-stream.on-payload` (EXT-M05)"
    );

    let mut unwired: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for interface in IMPORT_INTERFACES {
        for func in declared_funcs(interface) {
            checked += 1;
            if !sdk_calls(interface, &func) {
                unwired.push(format!("{interface}.{func}"));
            }
        }
    }
    assert!(checked >= 60, "only {checked} imports parsed across {IMPORT_INTERFACES:?} — parse broke");

    assert!(
        unwired.is_empty(),
        "these world imports are declared and implemented by the host, but NO code in \
         cyrup-ext-sdk calls them, so no extension can reach them: {unwired:?}. wit-bindgen emits \
         an uncalled import without a warning — unlike the host side, where bindgen's generated \
         trait makes a missing member a compile error (EXT-M04 / EXT-M05)."
    );
}
