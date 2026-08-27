//! The event-kind discriminant, leg 1 of 3 — the host's [`EventKind`] against the guest SDK's
//! `mod kind`.
//!
//! The numbering is hand-maintained in three copies: [`EventKind`] here (`src/event.rs`),
//! `mod kind` in `crates/cyrup-ext-sdk/src/api.rs`, and the bare literals `export_extension!`
//! passes to `guest::hook`/`guest::notify` (`crates/cyrup-ext-sdk/src/macros.rs`). This test checks
//! the first pair; `cyrup-ext-sdk/src/tests/world_import_coverage.rs` checks the second, in the
//! crate that can see the macro text.
//!
//! Nothing else connects them. The two crates share no type across this seam — the discriminant
//! crosses the WIT boundary as a plain `u8` — so adding an event mid-enum, or reordering two,
//! compiles clean on every target and passes the whole suite. Both couplings then break silently:
//! a guest's `subscribe` list is filtered through `EventKind::from_u8` in `src/host/live.rs`, whose
//! `if let Some(kind)` has no `else`, so a number the host does not know is DROPPED without a log;
//! and a number the host does know but that means a different event routes the payload to the
//! wrong handler. The symptom is "my extension's hook never fires", with no diagnostic anywhere
//! near the numbering.
//!
//! Read from disk rather than `include_str!` so the failure is a readable assertion naming the SDK
//! path, not a compile error in a sibling crate's source.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::EventKind;
use std::path::PathBuf;

/// `mod kind` consts whose lowercase spelling is NOT the [`EventKind::name`] of the event they
/// stand for: the SDK abbreviates `tool_execution_*` to `TOOL_EXEC_*`. Every other const is the
/// SCREAMING_SNAKE of its event name, checked by the lowercase fallback below.
const ABBREVIATED_CONSTS: &[(&str, &str)] = &[
    ("TOOL_EXEC_START", "tool_execution_start"),
    ("TOOL_EXEC_UPDATE", "tool_execution_update"),
    ("TOOL_EXEC_END", "tool_execution_end"),
];

fn sdk_api_rs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../cyrup-ext-sdk/src/api.rs")
}

#[test]
fn every_sdk_kind_const_names_the_host_event_kind_with_that_discriminant() {
    let path = sdk_api_rs();
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let body = src
        .split_once("\nmod kind {\n")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map(|(body, _)| body)
        .unwrap_or_else(|| panic!("`mod kind {{` block present in {}", path.display()));

    let mut wrong: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for line in body.lines() {
        let Some((name, value)) =
            line.trim().strip_prefix("pub const ").and_then(|rest| rest.split_once(": u8 = "))
        else {
            continue;
        };
        let Ok(discriminant) = value.trim().trim_end_matches(';').trim().parse::<u8>() else {
            continue;
        };
        checked += 1;
        let expected = ABBREVIATED_CONSTS
            .iter()
            .find(|(const_name, _)| *const_name == name)
            .map(|(_, event)| (*event).to_string())
            .unwrap_or_else(|| name.to_lowercase());
        match EventKind::from_u8(discriminant) {
            Some(kind) if kind.name() == expected => {}
            Some(kind) => wrong.push(format!(
                "the SDK's `kind::{name}` is {discriminant}, but the host's {discriminant} is \
                 `{}` ({kind:?}) — a guest subscribing to `{name}` would be handed that event's \
                 payload instead",
                kind.name()
            )),
            None => wrong.push(format!(
                "the SDK's `kind::{name}` is {discriminant}, which `EventKind::from_u8` rejects — \
                 `src/host/live.rs` drops such a subscription silently, so the guest's handler \
                 would simply never fire"
            )),
        }
    }

    // Non-vacuity: every loop above is satisfied by a parse that yields nothing, so the COUNT is
    // the only evidence the slice really spanned the table. `EventKind::COUNT` is the host's own
    // declaration of how many kinds exist, and the SDK declares one const per kind, so anything
    // below it means the parse lost entries (or the SDK dropped some).
    assert!(
        checked >= usize::from(EventKind::COUNT),
        "only {checked} `pub const … : u8 = …` line(s) parsed out of `mod kind` in {}, against \
         `EventKind::COUNT` = {} — the parse lost part of the table and this guard would be \
         checking almost nothing",
        path.display(),
        EventKind::COUNT,
    );
    assert!(
        wrong.is_empty(),
        "{} SDK event-kind const(s) disagree with the host `EventKind`. The discriminant crosses \
         the WIT boundary as a bare `u8`, so nothing but this test compares them:\n  {}",
        wrong.len(),
        wrong.join("\n  "),
    );
}
