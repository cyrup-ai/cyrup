//! Replaceable built-in extensions: pi's `{ name, factory, replaceable: true, builtin: true }`
//! entries of `builtInExtensions` (`packages/coding-agent/src/extensions/index.ts:9-14` @v1.0.1),
//! consumed by `omitReplacedExtensions` (`core/resource-loader.ts:116-153`).
//!
//! # The rule
//!
//! Upstream collects, for every loaded extension, the names it registered while its factory ran:
//! `tool:<name>`, `command:<name>` and `flag:<name>` (`resource-loader.ts:124-128`). The names of the
//! extensions that are NOT replaceable form the taken set (`:129-133`), and a replaceable extension
//! is left out of the loaded set when ANY of its own names is in it (`:135-138`). Two replaceable
//! extensions never leave each other out; they meet `detectExtensionConflicts` like any other pair.
//! The factory of a left-out extension has run, which is why `InlineExtension.replaceable` asks it to
//! register only tools, commands, flags and event handlers (`core/extensions/types.ts:2018-2024`).
//!
//! # How the registry applies it
//!
//! Upstream decides once, after every extension has loaded. This registry is written to as
//! extensions load, so the same decision is taken at the registration that completes the overlap,
//! whichever extension registers second: [`judge`] is that decision, a pure function of who
//! already holds a name. The outcome is order independent, as upstream's is:
//!
//! * a replaceable claimant meets a non-replaceable holder: the claimant is left out;
//! * a non-replaceable claimant meets replaceable holders: the holders are left out;
//! * any other pair is the ordinary first-wins rule, with its conflict record.
//!
//! Leaving an extension out drops every registration it made (`purge_owner`) and later ones are
//! ignored; the host then drops what is not in the registry (event handlers, bus subscriptions, the
//! id reservation). A record of each is kept for the startup diagnostics, as upstream pushes a
//! warning for each left-out built-in (`:139-150`).

use std::collections::HashSet;

use cyrup_core::ExtensionId;

/// Which of the three namespaces a name was registered in (`tool:`, `command:`, `flag:`,
/// `resource-loader.ts:124-128`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimKind {
    Tool,
    Command,
    Flag,
}

impl ClaimKind {
    /// The word upstream's warning uses (`kind` of `replacement.name.split(":", 2)`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Command => "command",
            Self::Flag => "flag",
        }
    }

    /// The name as its owner wrote it: `/name` for a command, `--name` for a flag, the bare name
    /// for a tool (`registeredName`, `resource-loader.ts:143`).
    pub fn registered_name(self, name: &str) -> String {
        match self {
            Self::Tool => name.to_owned(),
            Self::Command => format!("/{name}"),
            Self::Flag => format!("--{name}"),
        }
    }
}

/// A replaceable extension that was left out because another extension registered a name it
/// registered too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OmittedExtension {
    /// The extension that was left out.
    pub extension: ExtensionId,
    pub kind: ClaimKind,
    /// The name both registered.
    pub name: String,
    /// The non-replaceable extension that holds it.
    pub by: ExtensionId,
}

impl OmittedExtension {
    /// The warning upstream attaches to the load result (`resource-loader.ts:143-146`).
    ///
    /// Upstream's "To use ..." sentence first tells the user to run `pi config` and make sure the
    /// built-in is enabled under "Built-in extensions"; `cyrup config` has no such listing, so the
    /// sentence names the one step that applies.
    pub fn warning(&self) -> String {
        format!(
            "Extension {} registers {} `{}`, so built-in extension `{}` was not loaded. To use `{}`, disable or remove the existing extension. We recommend only having one or the other loaded at a time.",
            self.by,
            self.kind.as_str(),
            self.kind.registered_name(&self.name),
            self.extension,
            self.extension,
        )
    }
}

/// What a registration that meets other extensions' holdings of the same name does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Judgement {
    /// Nothing is replaceable here: the ordinary first-wins rule applies.
    Ordinary,
    /// The claimant is replaceable and a non-replaceable extension holds the name: it is left out.
    ClaimantLeftOut { by: ExtensionId },
    /// The claimant is not replaceable: these replaceable holders are left out.
    HoldersLeftOut(Vec<ExtensionId>),
}

/// `resource-loader.ts:129-138` for one name: `holders` are the OTHER extensions that registered
/// it.
pub(crate) fn judge(
    replaceable: &HashSet<ExtensionId>,
    claimant: &ExtensionId,
    holders: &[ExtensionId],
) -> Judgement {
    if replaceable.contains(claimant) {
        return match holders.iter().find(|holder| !replaceable.contains(*holder)) {
            Some(by) => Judgement::ClaimantLeftOut { by: by.clone() },
            None => Judgement::Ordinary,
        };
    }
    let left_out: Vec<ExtensionId> = holders
        .iter()
        .filter(|holder| replaceable.contains(*holder))
        .cloned()
        .collect();
    if left_out.is_empty() {
        Judgement::Ordinary
    } else {
        Judgement::HoldersLeftOut(left_out)
    }
}

/// The registry's record of replaceable extensions and of the ones left out.
#[derive(Default)]
pub(crate) struct Replaceables {
    replaceable: HashSet<ExtensionId>,
    omitted: Vec<OmittedExtension>,
    /// Left out, but the host has not yet dropped what the registry does not hold.
    pending_unload: Vec<ExtensionId>,
}

impl Replaceables {
    pub(crate) fn mark(&mut self, extension: ExtensionId) {
        self.replaceable.insert(extension);
    }

    pub(crate) fn replaceable(&self) -> &HashSet<ExtensionId> {
        &self.replaceable
    }

    pub(crate) fn is_omitted(&self, extension: &ExtensionId) -> bool {
        self.omitted.iter().any(|o| &o.extension == extension)
    }

    /// Record `extension` as left out. Once, however many of its names overlap.
    pub(crate) fn omit(
        &mut self,
        extension: ExtensionId,
        kind: ClaimKind,
        name: &str,
        by: ExtensionId,
    ) {
        if self.is_omitted(&extension) {
            return;
        }
        self.pending_unload.push(extension.clone());
        self.omitted.push(OmittedExtension {
            extension,
            kind,
            name: name.to_owned(),
            by,
        });
    }

    pub(crate) fn omitted(&self) -> &[OmittedExtension] {
        &self.omitted
    }

    pub(crate) fn take_pending_unload(&mut self) -> Vec<ExtensionId> {
        std::mem::take(&mut self.pending_unload)
    }

    /// Forget a replaceable that failed to load: it is neither replaceable nor left out any more.
    pub(crate) fn forget(&mut self, extension: &ExtensionId) {
        self.replaceable.remove(extension);
        self.omitted.retain(|o| &o.extension != extension);
        self.pending_unload.retain(|e| e != extension);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> ExtensionId {
        ExtensionId::from(name)
    }

    fn replaceable(names: &[&str]) -> HashSet<ExtensionId> {
        names.iter().map(|n| id(n)).collect()
    }

    #[test]
    fn a_replaceable_claimant_yields_to_a_non_replaceable_holder() {
        let set = replaceable(&["builtin"]);
        assert_eq!(
            judge(&set, &id("builtin"), &[id("third-party")]),
            Judgement::ClaimantLeftOut {
                by: id("third-party")
            }
        );
    }

    #[test]
    fn a_non_replaceable_claimant_displaces_replaceable_holders_only() {
        let set = replaceable(&["builtin"]);
        assert_eq!(
            judge(&set, &id("third-party"), &[id("builtin"), id("other")]),
            Judgement::HoldersLeftOut(vec![id("builtin")])
        );
    }

    /// `taken` is built from the non-replaceable extensions alone (`resource-loader.ts:129-133`).
    #[test]
    fn two_replaceable_extensions_do_not_leave_each_other_out() {
        let set = replaceable(&["a", "b"]);
        assert_eq!(judge(&set, &id("a"), &[id("b")]), Judgement::Ordinary);
        assert_eq!(judge(&set, &id("b"), &[id("a")]), Judgement::Ordinary);
    }

    #[test]
    fn two_ordinary_extensions_keep_the_first_wins_rule() {
        assert_eq!(
            judge(&HashSet::new(), &id("a"), &[id("b")]),
            Judgement::Ordinary
        );
        assert_eq!(judge(&HashSet::new(), &id("a"), &[]), Judgement::Ordinary);
    }

    #[test]
    fn the_warning_names_the_name_as_its_owner_wrote_it() {
        let omitted = |kind, name: &str| OmittedExtension {
            extension: id("codemode"),
            kind,
            name: name.to_owned(),
            by: id("mine"),
        };
        assert_eq!(
            omitted(ClaimKind::Tool, "codemode").warning(),
            "Extension mine registers tool `codemode`, so built-in extension `codemode` was not loaded. To use `codemode`, disable or remove the existing extension. We recommend only having one or the other loaded at a time."
        );
        assert!(
            omitted(ClaimKind::Command, "mcp")
                .warning()
                .contains("registers command `/mcp`")
        );
        assert!(
            omitted(ClaimKind::Flag, "x")
                .warning()
                .contains("registers flag `--x`")
        );
    }
}
