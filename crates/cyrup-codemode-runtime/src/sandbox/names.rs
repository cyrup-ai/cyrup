//! The names a sandbox accepts for its tools and globals (pi `runtime/host.ts:22-34` and
//! `:295-314` @v1.0.1). Pure: no engine, no state beyond what the caller passes in.

use std::collections::HashSet;

use cyrup_codemode::identifier::is_identifier;

use crate::types::CodemodeTool;

/// Top-level names the sandbox itself defines or the language reserves (`host.ts:24-34`).
const RESERVED_GLOBALS: [&str; 9] = [
    "tools",
    "ALL_TOOLS",
    "console",
    "text",
    "image",
    "exit",
    "globalThis",
    "store",
    "load",
];

/// Why a sandbox refused its configuration. The texts are upstream's `Error` messages.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SandboxConfigError {
    /// More than one dot, a part that is not an identifier, or a reserved first part.
    #[error("Invalid global name \"{name}\"")]
    InvalidGlobalName { name: String },
    #[error("Global \"{name}\" is already registered")]
    GlobalAlreadyRegistered { name: String },
    /// `a` and `a.b` are both globals: `a` would be a function and a namespace at once.
    #[error("Global \"{name}\" conflicts with the namespace \"{name}\"")]
    GlobalConflictsWithNamespace { name: String },
    #[error("Tool \"{name}\" is already registered")]
    ToolAlreadyRegistered { name: String },
}

/// Whether `name` may be a global: `identifier` or `namespace.identifier`, never one of the
/// reserved names as the first part.
fn is_valid_global_name(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    match parts.as_slice() {
        [single] => is_identifier(single) && !RESERVED_GLOBALS.contains(single),
        [namespace, member] => {
            is_identifier(namespace)
                && is_identifier(member)
                && !RESERVED_GLOBALS.contains(namespace)
        }
        _ => false,
    }
}

/// Checks the globals of a sandbox's options in upstream's order: per global its name, then
/// uniqueness; after all of them, that no plain global is also a namespace.
pub(super) fn validate_globals(globals: &[CodemodeTool]) -> Result<(), SandboxConfigError> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut namespaces: Vec<&str> = Vec::new();
    for global in globals {
        let name = global.declaration.name.as_str();
        if !is_valid_global_name(name) {
            return Err(SandboxConfigError::InvalidGlobalName {
                name: name.to_owned(),
            });
        }
        if seen.contains(name) {
            return Err(SandboxConfigError::GlobalAlreadyRegistered {
                name: name.to_owned(),
            });
        }
        if let Some((namespace, _)) = name.split_once('.')
            && !namespaces.contains(&namespace)
        {
            namespaces.push(namespace);
        }
        seen.insert(name);
    }
    for namespace in namespaces {
        if seen.contains(namespace) {
            return Err(SandboxConfigError::GlobalConflictsWithNamespace {
                name: namespace.to_owned(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use std::sync::Arc;

    use cyrup_codemode::types::ToolDeclaration;

    use super::*;

    fn global(name: &str) -> CodemodeTool {
        CodemodeTool {
            declaration: ToolDeclaration::new(name),
            execute: Arc::new(|_, _| Box::pin(async { Ok(None) })),
        }
    }

    #[test]
    fn accepts_plain_and_namespaced_names() {
        validate_globals(&[
            global("attach"),
            global("models.list"),
            global("models.first"),
        ])
        .unwrap();
    }

    #[test]
    fn rejects_malformed_and_reserved_names() {
        for name in [
            "a.b.c",
            "a.",
            ".a",
            "tools.x",
            "store.x",
            "a.not-valid",
            "not-valid",
            "tools",
            "console",
            "",
            "9a",
        ] {
            assert_eq!(
                validate_globals(&[global(name)]),
                Err(SandboxConfigError::InvalidGlobalName {
                    name: name.to_owned()
                }),
                "{name}"
            );
        }
    }

    #[test]
    fn rejects_duplicates_and_namespace_conflicts_in_either_order() {
        assert_eq!(
            validate_globals(&[global("a"), global("a")]),
            Err(SandboxConfigError::GlobalAlreadyRegistered { name: "a".into() })
        );
        for order in [["models", "models.list"], ["models.list", "models"]] {
            let globals: Vec<_> = order.iter().map(|name| global(name)).collect();
            assert_eq!(
                validate_globals(&globals),
                Err(SandboxConfigError::GlobalConflictsWithNamespace {
                    name: "models".into()
                })
            );
        }
    }

    #[test]
    fn error_texts_are_upstreams() {
        assert_eq!(
            SandboxConfigError::GlobalConflictsWithNamespace { name: "m".into() }.to_string(),
            "Global \"m\" conflicts with the namespace \"m\""
        );
        assert_eq!(
            SandboxConfigError::GlobalAlreadyRegistered { name: "m".into() }.to_string(),
            "Global \"m\" is already registered"
        );
        assert_eq!(
            SandboxConfigError::ToolAlreadyRegistered {
                name: "echo".into()
            }
            .to_string(),
            "Tool \"echo\" is already registered"
        );
    }
}
