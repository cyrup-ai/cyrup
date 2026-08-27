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

/// The crate front page and the event-kind table it describes. Kept OUT of `SDK_SOURCES`: that
/// constant is grepped for `module::name(` call paths, and folding prose into it would let a doc
/// sentence satisfy an import-coverage check.
const LIB_RS: &str = include_str!("../lib.rs");
const API_RS: &str = include_str!("../api.rs");

/// The `export_extension!` body, read as TEXT because that is the only way to see it from a host
/// test: the macro's guest arm is `#[cfg(target_arch = "wasm32")]` (`src/macros.rs:34`), so on the
/// target the default suite runs on, its event-kind literals are never parsed at all. Kept out of
/// `SDK_SOURCES` for the same reason `LIB_RS` is — the macro is the world's EXPORT surface, and
/// folding it in would let an export body satisfy an IMPORT-coverage check.
const MACROS_RS: &str = include_str!("../macros.rs");

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

/// The crate-root doc opens by telling an author how many lifecycle events they may subscribe to,
/// and `api::kind` is where that number actually comes from. Nothing connected the two: EXT-072
/// raised the count to 33 in `api.rs`, `guest.rs` and `macros.rs` and left `lib.rs` reading 30 —
/// next to a SEPARATE, correct "30 typed event payloads" that made the stale one read as
/// deliberate. A wrong digit in prose is not a rustdoc warning and not a compile error, so the
/// count is pinned to its source of truth here.
#[test]
fn crate_root_doc_states_the_real_event_count() {
    let kind_mod = API_RS
        .split_once("\nmod kind {\n")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map(|(body, _)| body)
        .expect("`mod kind {` block present in src/api.rs");
    // Non-vacuity: prove the slice really spans the table — first discriminant to last — rather
    // than an empty or truncated match that would make the count below meaningless.
    assert!(
        kind_mod.contains("TOOL_CALL: u8 = 0;") && kind_mod.contains("SESSION_INFO_CHANGED: u8 = 32;"),
        "the `mod kind` slice lost its first or last discriminant, so this guard would be vacuous"
    );

    let kinds = kind_mod
        .lines()
        .filter(|line| line.starts_with("    pub const ") && line.contains(": u8 = "))
        .count();

    let (before, _) = LIB_RS
        .split_once(" lifecycle events")
        .expect("the crate-root doc states a `<n> lifecycle events` count");
    let stated: usize = before
        .rsplit(' ')
        .next()
        .and_then(|word| word.parse().ok())
        .expect("the word before `lifecycle events` in the crate-root doc is that count");

    assert_eq!(
        stated, kinds,
        "src/lib.rs advertises {stated} lifecycle events but `api::kind` declares {kinds} \
         discriminants — update the crate-root doc (and check `api.rs`, `guest.rs`, `macros.rs`, \
         which state the same number)."
    );
}

// ---------------------------------------------------------------------------
// The event-kind discriminant, leg 2 of 3.
//
// The numbering exists in three hand-maintained copies: the host's `EventKind`
// (`cyrup-ext/src/event.rs`), `mod kind` in this crate's `src/api.rs`, and the bare literals
// `export_extension!` passes to `guest::hook`/`guest::notify`. This file checks the second pair;
// `cyrup-ext/src/tests/event_kind_lockstep.rs` checks the first, in the crate that can see both.
//
// The macro literal is the leg with no compiler behind it. Its arm is `#[cfg(target_arch =
// "wasm32")]`, so a wrong number is not even PARSED on the host target the default suite runs on;
// and on wasm32 it still type-checks, because every discriminant is a `u8`. The guest then reports
// one event's number for another event's export, and neither side can notice: the host's dispatch
// (`cyrup-ext/src/host/live.rs`) maps the number through `EventKind::from_u8` and finds a perfectly
// real kind, so the handler runs against another event's argument strings.
// ---------------------------------------------------------------------------

/// `on_*` exports of `export_extension!` that carry NO event kind, and why. `on_terminal_input` is
/// the guest half of pi's `onTerminalInput` handler — it is routed by `guest::on_terminal_input`
/// and returns a `TerminalInputResult`, not by the kind-numbered `hook`/`notify` pair.
const NON_KIND_EXPORTS: &[&str] = &["on_terminal_input"];

/// The `fn on_<name>` -> `api::kind::<CONST>` pairs that are NOT the SCREAMING_SNAKE of `<name>`.
/// The exports are named after the events (`on_tool_execution_start`) and three of the consts are
/// abbreviated (`TOOL_EXEC_START`), so the pairing cannot be a case conversion — a test that
/// assumed one would report three false failures and tempt the next reader to rename the consts.
const KIND_NAME_OVERRIDES: &[(&str, &str)] = &[
    ("on_tool_execution_start", "TOOL_EXEC_START"),
    ("on_tool_execution_update", "TOOL_EXEC_UPDATE"),
    ("on_tool_execution_end", "TOOL_EXEC_END"),
];

/// The `pub const <NAME>: u8 = <N>;` table of `mod kind` in `src/api.rs`.
fn kind_consts() -> Vec<(String, u32)> {
    let body = API_RS
        .split_once("\nmod kind {\n")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map(|(body, _)| body)
        .expect("`mod kind {` block present in src/api.rs");
    body.lines()
        .filter_map(|line| {
            let (name, value) = line.trim().strip_prefix("pub const ")?.split_once(": u8 = ")?;
            let value: u32 = value.trim().trim_end_matches(';').trim().parse().ok()?;
            Some((name.trim().to_string(), value))
        })
        .collect()
}

/// Every `fn on_<name>` in `src/macros.rs`, paired with the first discriminant its body passes to
/// `guest::hook` / `guest::notify` — `None` when the body delegates somewhere else.
fn macro_event_exports() -> Vec<(String, Option<u32>)> {
    let lines: Vec<&str> = MACROS_RS.lines().collect();
    let mut out: Vec<(String, Option<u32>)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix("fn on_") else { continue };
        let Some((suffix, _)) = rest.split_once('(') else { continue };
        let mut discriminant = None;
        // Stop at the next `fn `: a body that dispatches no kind must come back `None` rather than
        // silently adopt the following export's literal and pass for the wrong reason.
        for (j, body_line) in lines.iter().enumerate().skip(i + 1) {
            let trimmed = body_line.trim_start();
            if trimmed.starts_with("fn ") {
                break;
            }
            let Some((_, after)) = trimmed
                .split_once("guest::hook(")
                .or_else(|| trimmed.split_once("guest::notify("))
            else {
                continue;
            };
            // Calls with many arguments are wrapped by rustfmt, which puts the discriminant alone
            // on the next line.
            let digits = if after.trim().is_empty() { lines.get(j + 1).copied().unwrap_or_default() } else { after };
            discriminant =
                digits.trim_start().chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok();
            break;
        }
        out.push((format!("on_{suffix}"), discriminant));
    }
    out
}

#[test]
fn every_numbered_macro_export_matches_its_mod_kind_discriminant() {
    let kinds = kind_consts();
    assert!(
        kinds.len() >= 33,
        "`mod kind` in src/api.rs parsed as only {} const(s) — this guard would be vacuous",
        kinds.len()
    );

    let mut mismatched: Vec<String> = Vec::new();
    let mut unaccounted: Vec<String> = Vec::new();
    let mut allowlisted: Vec<String> = Vec::new();
    let mut paired = 0usize;

    for (export, literal) in macro_event_exports() {
        let Some(literal) = literal else {
            if NON_KIND_EXPORTS.contains(&export.as_str()) {
                allowlisted.push(export);
            } else {
                unaccounted.push(export);
            }
            continue;
        };
        let const_name = KIND_NAME_OVERRIDES
            .iter()
            .find(|(export_name, _)| *export_name == export)
            .map(|(_, const_name)| (*const_name).to_string())
            .unwrap_or_else(|| export.trim_start_matches("on_").to_uppercase());
        match kinds.iter().find(|(name, _)| *name == const_name) {
            Some((_, declared)) if *declared == literal => paired += 1,
            Some((_, declared)) => mismatched.push(format!(
                "`{export}` reports kind {literal} to the host, but `api::kind::{const_name}` is {declared}"
            )),
            None => mismatched.push(format!(
                "`{export}` maps to `api::kind::{const_name}`, which `mod kind` does not declare — \
                 add it, or add the export to KIND_NAME_OVERRIDES / NON_KIND_EXPORTS"
            )),
        }
    }

    assert!(
        mismatched.is_empty(),
        "{} `export_extension!` export(s) disagree with `api::kind`. The macro literal is what the \
         guest actually sends, and `ExtensionApi` subscribes and dispatches by the const, so a \
         disagreement routes an event to the wrong handler (or to none) with no diagnostic \
         anywhere:\n  {}",
        mismatched.len(),
        mismatched.join("\n  "),
    );
    assert!(
        unaccounted.is_empty(),
        "these `export_extension!` exports pass no discriminant to `guest::hook`/`guest::notify`: \
         {unaccounted:?}. If that is deliberate, add each to NON_KIND_EXPORTS with the reason; \
         otherwise the export is dead — the host will emit the event and no handler will run."
    );
    for entry in NON_KIND_EXPORTS {
        assert!(
            allowlisted.iter().any(|export| export == entry),
            "NON_KIND_EXPORTS lists `{entry}`, but src/macros.rs has no such kindless `on_*` export \
             any more — drop the stale entry so the allowlist keeps meaning something"
        );
    }
    // Non-vacuity, the shape `every_ctx_submodule_is_in_sdk_sources` uses: the loops above are all
    // satisfied trivially by a parse that finds nothing, so the COUNT is the only evidence this
    // guard read the macro at all. 33 is the full event catalog (`EventKind::COUNT`), so the floor
    // also catches an export that is deleted rather than renumbered.
    assert!(
        paired >= 33,
        "only {paired} numbered `on_*` export(s) were paired with an `api::kind` const — \
         src/macros.rs declares one per event kind, so this parse lost some and the guard is \
         checking almost nothing"
    );
}
