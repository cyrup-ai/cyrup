//! The identifier a script uses for a tool (pi `packages/codemode/src/identifier.ts:5`
//! `toCodemodeIdentifier` @v1.0.1, CODE-004).

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

#[cfg(test)]
mod tests;
