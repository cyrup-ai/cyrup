//! The identifier a script uses for a tool (pi `packages/codemode/src/identifier.ts:5`
//! `toCodemodeIdentifier` @v1.0.1, CODE-004).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// A string known to be a valid ASCII JavaScript identifier (`[A-Za-z_$][A-Za-z0-9_$]*`).
///
/// Only [`to_codemode_identifier`] constructs one, so a value of this type is the proof that the
/// name was normalised: a script can write `tools.<id>` for it, and no two call sites can disagree
/// about what "the identifier for a tool" is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct CodemodeIdentifier(String);

impl CodemodeIdentifier {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for CodemodeIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for CodemodeIdentifier {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

fn is_identifier_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '$'
}

fn is_identifier_part(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// Whether `name` matches `/^[A-Za-z_$][A-Za-z0-9_$]*$/`, upstream's `IDENTIFIER`
/// (`declarations.ts:7`). Used to decide whether a schema property needs quoting.
#[must_use]
pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(is_identifier_start) && chars.all(is_identifier_part)
}

/// Characters that are not valid in a JavaScript identifier become `_`; the empty name becomes
/// `_`. `mcp__docs__search` stays as is, `my-tool` becomes `my_tool`, `9lives` becomes `_lives`.
///
/// Iterates Unicode scalar values, as `for (const char of name)` does: one astral character is one
/// `_`, not two.
#[must_use]
pub fn to_codemode_identifier(name: &str) -> CodemodeIdentifier {
    let mut identifier = String::with_capacity(name.len());
    for c in name.chars() {
        let valid = if identifier.is_empty() {
            is_identifier_start(c)
        } else {
            is_identifier_part(c)
        };
        identifier.push(if valid { c } else { '_' });
    }
    if identifier.is_empty() {
        identifier.push('_');
    }
    CodemodeIdentifier(identifier)
}

/// The identifier of each tool a script can call, unique across them.
///
/// [`to_codemode_identifier`] is not injective: `gh-search` and `gh_search` are both
/// `gh_search`, and a script could reach only one of them (upstream's prelude keeps the first and
/// silently drops the other, `prelude-source.ts:213-219` @v1.0.4).
///
/// [CYRUP-DELTA] The table gives every tool an identifier of its own. A name that already is an
/// identifier keeps it, then each normalised name takes its identifier if that is still free (in
/// order of the raw name, so the result does not depend on the order tools were registered in), and
/// the names left over take `<identifier>_2`, `<identifier>_3`, ... Names that do not collide get
/// exactly [`to_codemode_identifier`]'s result, which is all upstream has.
///
/// Everything a script or the model reads an identifier from, such as `tools.<id>`, the declarations,
/// `ALL_TOOLS` and `searchTools()`, must come from one table over the same set of tools.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdentifierTable {
    identifiers: BTreeMap<String, CodemodeIdentifier>,
}

impl IdentifierTable {
    /// The table over `names`, the raw names of all tools a script can call.
    #[must_use]
    pub fn assign<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut names: Vec<&str> = names.into_iter().collect();
        names.sort_unstable();
        names.dedup();
        let mut identifiers: BTreeMap<String, CodemodeIdentifier> = BTreeMap::new();
        let mut taken: BTreeSet<String> = BTreeSet::new();
        for &name in &names {
            if to_codemode_identifier(name).as_str() == name {
                taken.insert(name.to_owned());
                identifiers.insert(name.to_owned(), CodemodeIdentifier(name.to_owned()));
            }
        }
        let mut colliding: Vec<(&str, CodemodeIdentifier)> = Vec::new();
        for &name in &names {
            if identifiers.contains_key(name) {
                continue;
            }
            let identifier = to_codemode_identifier(name);
            if taken.insert(identifier.as_str().to_owned()) {
                identifiers.insert(name.to_owned(), identifier);
            } else {
                colliding.push((name, identifier));
            }
        }
        for (name, identifier) in colliding {
            let unique = (2_usize..)
                .map(|n| format!("{identifier}_{n}"))
                .find(|candidate| taken.insert(candidate.clone()));
            if let Some(unique) = unique {
                identifiers.insert(name.to_owned(), CodemodeIdentifier(unique));
            }
        }
        Self { identifiers }
    }

    /// The identifier scripts use for the tool `name`; [`to_codemode_identifier`] of it for a name
    /// the table was not made over.
    #[must_use]
    pub fn get(&self, name: &str) -> CodemodeIdentifier {
        self.identifiers
            .get(name)
            .cloned()
            .unwrap_or_else(|| to_codemode_identifier(name))
    }

    /// The identifier the table assigned to `name`, if it was made over it.
    #[must_use]
    pub fn assigned(&self, name: &str) -> Option<&CodemodeIdentifier> {
        self.identifiers.get(name)
    }

    /// The names whose identifier is not [`to_codemode_identifier`]'s, that is those that lost a
    /// collision, with the identifier they got.
    pub fn renamed(&self) -> impl Iterator<Item = (&str, &CodemodeIdentifier)> {
        self.identifiers
            .iter()
            .filter(|(name, identifier)| to_codemode_identifier(name) != **identifier)
            .map(|(name, identifier)| (name.as_str(), identifier))
    }
}

#[cfg(test)]
mod tests;
