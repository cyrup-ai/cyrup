//! The crate root's flat re-export block and [`crate::prelude`] must name the SAME set of symbols.
//!
//! `lib.rs` carries two hand-written export lists — the flat `pub use api::{…}` / `pub use
//! descriptor::{…}` block at the root, and the `pub use crate::api::{…}` / … block inside `pub mod
//! prelude`. Nothing forced them to agree, and they drifted: ten names sat in the flat block and
//! not the prelude, among them the parameter and return types of `Ctx::exec`
//! (`src/ctx/exec.rs`: `pub fn exec(&self, cmd: &str, args: &[&str], opts: &ExecOptions) ->
//! Result<ExecResult, String>`) and the return type of `define_tool` (`src/tool_factory.rs`:
//! `-> RegisteredTool`). Since `src/macros.rs` documents `use cyrup_ext_sdk::prelude::*;` as THE
//! author entry point, that drift meant the documented import could not express the crate's own
//! reference extension — which is exactly what `src/example/commands_capability.rs` shows, importing `ExecOptions`
//! from `crate::{…}` instead.
//!
//! Neither half of the drift is a compile error, a warning, or a rustdoc complaint: a missing
//! re-export is simply a name a downstream author cannot write, and it surfaces in THEIR build.
//! So the equality is checked structurally here, over the text of `lib.rs`, rather than by a
//! third hand-maintained list of names — a hand-written list would itself go stale in the same
//! silence, and would not notice a name added to the flat block alone.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

const LIB_RS: &str = include_str!("../lib.rs");

/// The line that opens the prelude module, and so separates the two lists.
const PRELUDE_OPEN: &str = "pub mod prelude {";

/// `widget` is re-exported by the prelude as a MODULE (`pub use crate::widget;`) so that
/// `widget::text(..)` reads at the call site. The crate root needs no `pub use` twin for it —
/// `pub mod widget;` in `lib.rs` already makes `cyrup_ext_sdk::widget` a nameable path — so it is
/// the one name legitimately present in the prelude set and absent from the root set.
/// [`prelude_and_root_reexport_the_same_names`] re-checks that `pub mod widget;` is still there,
/// so this exemption cannot outlive its justification.
const PRELUDE_ONLY: &[&str] = &["widget"];

/// `region` with every comment line dropped, so `pub use` written inside prose cannot be counted
/// as a re-export (the prelude's own doc comments discuss re-exports at length).
fn code_only(region: &str) -> String {
    region
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The leaf names a region's `pub use` statements re-export.
///
/// A brace group contributes each name inside it; `path::Name;` contributes `Name`; a glob
/// contributes its WHOLE path (`events::*`) rather than a bare `*`, so two regions globbing
/// different modules cannot be mistaken for agreement. A leading `crate::` is stripped, because
/// that is the only spelling difference the two lists are allowed to have.
fn reexports(region: &str) -> BTreeSet<String> {
    let code = code_only(region);
    let mut names = BTreeSet::new();
    for chunk in code.split("pub use ").skip(1) {
        let Some(statement) = chunk.split(';').next() else {
            continue;
        };
        let path = statement.trim();
        let path = path.strip_prefix("crate::").unwrap_or(path);
        if let Some((_, after_brace)) = path.split_once('{') {
            let Some((inner, _)) = after_brace.split_once('}') else {
                continue;
            };
            names.extend(
                inner
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string),
            );
        } else if path.ends_with("::*") {
            names.insert(path.to_string());
        } else if let Some(leaf) = path.rsplit("::").next() {
            names.insert(leaf.to_string());
        }
    }
    names
}

/// `lib.rs` split into the text BEFORE `pub mod prelude {` (the crate-root flat block) and the
/// body of that module.
fn export_regions() -> (String, String) {
    let (root, rest) = LIB_RS
        .split_once(PRELUDE_OPEN)
        .expect("`pub mod prelude {` present in src/lib.rs");
    // The prelude's own `pub use …{ … };` groups close with an INDENTED `};`, so the first
    // column-0 `}` line is the module's closing brace.
    let prelude = rest
        .split_once("\n}\n")
        .map(|(body, _)| body)
        .expect("the `prelude` module has a column-0 closing brace");
    (root.to_string(), prelude.to_string())
}

#[test]
fn prelude_and_root_reexport_the_same_names() {
    let (root_src, prelude_src) = export_regions();
    let root = reexports(&root_src);
    let prelude = reexports(&prelude_src);

    // Non-vacuity: prove the split landed where it should and both parses found real lists before
    // any emptiness is allowed to satisfy the set comparisons below.
    assert!(
        !prelude_src.contains("\npub mod ") && !prelude_src.contains("\npub(crate) fn "),
        "the `pub mod prelude` body over-ran its closing brace and swallowed crate-root items, \
         so this test would compare a region against itself"
    );
    assert!(
        root.contains("ExtensionApi") && root.contains("events::*") && root.contains("define_tool"),
        "the crate-root parse lost known re-exports; it found {root:?}"
    );
    assert!(
        prelude.contains("ExtensionApi") && prelude.contains("events::*"),
        "the prelude parse lost known re-exports; it found {prelude:?}"
    );
    assert!(
        root.len() >= 60,
        "only {} root re-exports parsed — the crate root re-exports well over 60 names, so the \
         parse is broken and this guard would be near-vacuous",
        root.len()
    );

    let missing_from_prelude: Vec<&str> = root.difference(&prelude).map(String::as_str).collect();
    assert!(
        missing_from_prelude.is_empty(),
        "these names are re-exported at the crate root but NOT by `prelude`, so an author \
         following the documented `use cyrup_ext_sdk::prelude::*;` cannot name them: \
         {missing_from_prelude:?}. Add them to the matching `pub use crate::…` group inside \
         `pub mod prelude` in src/lib.rs."
    );

    let missing_from_root: Vec<&str> = prelude
        .difference(&root)
        .map(String::as_str)
        .filter(|name| !PRELUDE_ONLY.contains(name))
        .collect();
    assert!(
        missing_from_root.is_empty(),
        "these names are in `prelude` but NOT in the crate-root flat block, so they cannot be \
         written as `cyrup_ext_sdk::<Name>`: {missing_from_root:?}. Add them to the matching \
         `pub use …` group at the top of src/lib.rs (or, if the omission is deliberate, to \
         PRELUDE_ONLY with the reason)."
    );

    // The `widget` exemption is only sound while `pub mod widget;` keeps the module nameable at
    // the root, and only earns its place while the prelude actually re-exports it.
    for name in PRELUDE_ONLY {
        assert!(
            prelude.contains(*name),
            "`{name}` is exempted as prelude-only but the prelude no longer re-exports it — drop \
             the stale PRELUDE_ONLY entry"
        );
        assert!(
            !root.contains(*name),
            "`{name}` is now in the crate-root flat block too, so the PRELUDE_ONLY exemption is \
             obsolete — remove it and let the set comparison cover the name"
        );
    }
    assert!(
        LIB_RS.contains("\npub mod widget;\n"),
        "the `widget` PRELUDE_ONLY exemption rests on `pub mod widget;` making \
         `cyrup_ext_sdk::widget` nameable at the crate root, and that declaration is gone"
    );
}

/// Compile-time half of the same property, for the names the drift actually cost an author.
///
/// The text comparison above proves the two LISTS agree; this proves the resulting import surface
/// really resolves. Naming a type through the glob is the whole test — if a name leaves the
/// prelude, this module stops compiling.
mod prelude_glob_names_the_signature_types {
    #![allow(dead_code)]

    use crate::prelude::*;

    // `Ctx::exec` (src/ctx/exec.rs) takes `&ExecOptions` and returns `Result<ExecResult, String>`.
    type Exec0 = ExecOptions;
    type Exec1 = ExecResult;
    // The registration handles the `ExtensionApi` builders hand back.
    type Reg0 = RawOutcome;
    type Reg1 = RegisteredCommand;
    type Reg2 = RegisteredRenderer;
    type Reg3 = RegisteredShortcut;
    type Reg4 = RegisteredTool;
    // Descriptor fields: `pub render_shell: RenderShell` and `pub cost: ModelCost`.
    type Desc0 = RenderShell;
    type Desc1 = ModelCost;
    type Desc2 = ModelCostTier;
}

/// The seven signature types that were reachable only through `cyrup_ext_sdk::api::` /
/// `cyrup_ext_sdk::descriptor::` module paths, pinned to the crate root.
mod root_names_the_signature_types {
    #![allow(dead_code)]

    // Reachable from `ExtensionApi::on_terminal_input` / `handle_terminal_input` (src/api.rs) and
    // from `pub completions: Option<ArgCompleter>`.
    type Arg = crate::ArgCompleter;
    type TermIn = crate::TerminalInputResult;
    fn takes_a_terminal_input_handler<H: crate::TerminalInputHandler>(_handler: H) {}

    // Reachable from the `constrained_sampling` descriptor field and its builder
    // (src/descriptor.rs).
    type Cs0 = crate::ConstrainedSampling;
    type Cs1 = crate::ConstrainedSamplingConfig;
    type Cs2 = crate::GrammarVariants;
    type Cs3 = crate::StrictSampling;
}
