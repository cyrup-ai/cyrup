//! How the codemode tool presents tools that are both declared and callable from scripts (pi
//! `prepareCodemodeLoadout`, `extensions/codemode/tool.ts:324-359` @v1.0.1, CODE-007):
//!
//! * `on`: their descriptions say how scripts call them, and the codemode description lists only
//!   the callable tools without `direct` exposure.
//! * `only`: the codemode description lists every callable tool, and requests leave out the
//!   declarations of active `direct` tools.
//!
//! Listing by exposure, not by the active set, keeps the codemode description unchanged when
//! `tool_search` loads a tool, so loads do not redeclare codemode.
//!
//! # Production call path
//!
//! [`cyrup_core::ToolLoadout::resolve`] calls [`cyrup_core::Tool::prepare_loadout`] for every
//! active tool; [`super::CodemodeTool`]'s implementation is [`prepare_codemode_loadout`]. The
//! session runs `resolve` at build time and on every active-set change.

use std::collections::{BTreeMap, BTreeSet};

use cyrup_codemode::identifier::IdentifierTable;
use cyrup_config::CodemodeMode;
use cyrup_core::{LoadoutView, ToolExposure, ToolLoadoutChanges};

use super::description::{
    DescriptionOptions, callable_tools, create_codemode_description, describe_script_call,
};
use super::{CODEMODE_TOOL_NAME, CodemodeToolOptions};

/// The description updates and hidden declarations for `view` under the configured mode.
#[must_use]
pub fn prepare_codemode_loadout(
    view: &LoadoutView<'_>,
    options: &CodemodeToolOptions,
) -> ToolLoadoutChanges {
    let mode = options.effective_mode();
    let is_direct = |name: &str| view.exposure(name) == ToolExposure::Direct;
    let callable = callable_tools(view.callable());
    let callable_names: BTreeSet<&str> = callable.iter().map(|tool| tool.name()).collect();
    // The identifiers over every callable tool, as the sandbox assigns them for a script
    // (`execute_codemode`), so the text names the identifier a script has to use.
    let identifiers = IdentifierTable::assign(callable_names.iter().copied());

    let mut descriptions: BTreeMap<String, String> = BTreeMap::new();
    if mode == CodemodeMode::On {
        for tool in view.declared() {
            if callable_names.contains(tool.name()) {
                descriptions.insert(
                    tool.name().to_owned(),
                    describe_script_call(tool.as_ref(), &identifiers),
                );
            }
        }
    }

    let listed: Vec<_> = if mode == CodemodeMode::Only {
        callable.clone()
    } else {
        callable
            .iter()
            .filter(|tool| !is_direct(tool.name()))
            .cloned()
            .collect()
    };
    let namespaces: BTreeMap<String, _> = listed
        .iter()
        .filter_map(|tool| {
            view.namespace(tool.name())
                .map(|namespace| (tool.name().to_owned(), namespace))
        })
        .collect();
    // A listed tool carries its prompt guidelines with its declaration: the system prompt only has
    // them for declared tools (CODE-020).
    let guidelines: BTreeMap<String, Vec<String>> = listed
        .iter()
        .map(|tool| (tool.name().to_owned(), view.prompt_guidelines(tool.name())))
        .collect();
    let deferred: BTreeSet<String> = listed
        .iter()
        .filter(|tool| view.exposure(tool.name()) == ToolExposure::Deferred)
        .map(|tool| tool.name().to_owned())
        .collect();
    descriptions.insert(
        CODEMODE_TOOL_NAME.to_owned(),
        create_codemode_description(
            &listed,
            &DescriptionOptions {
                models: options.declares_models(),
                namespaces: &namespaces,
                deferred: &deferred,
                guidelines: &guidelines,
                inline_budget: Some(options.effective_inline_budget()),
                docs_path: &options.docs_path,
                identifiers: &identifiers,
            },
        ),
    );

    let declared_names: BTreeSet<&str> = view.declared().iter().map(|tool| tool.name()).collect();
    ToolLoadoutChanges {
        descriptions,
        hidden_declarations: if mode == CodemodeMode::Only {
            callable
                .iter()
                .filter(|tool| is_direct(tool.name()) && declared_names.contains(tool.name()))
                .map(|tool| tool.name().to_owned())
                .collect()
        } else {
            Vec::new()
        },
    }
}
