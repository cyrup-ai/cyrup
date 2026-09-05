//! SUBA-074 stage 2 — the external child's environment: `externalEnvironment`
//! (`pi-subagents/src/runs/shared/external-cli-runner.ts:88-101` @v0.64.0).
//!
//! **Why this is a type and not a `HashMap`.** This crate's only other spawn type,
//! [`crate::spawn::ChildSpawnSpec`], documents the opposite rule — the native subagent child "MUST
//! inherit the parent process's full environment", and `env_clear()` is a grep-verifiable
//! never-called invariant there. That is correct for a cyrup child re-exec'ing the cyrup binary and
//! catastrophic for a FOREIGN CLI: the orchestrator's environment carries the subagent permission
//! policy, the tool-budget and capability-ceiling encodings, the structured-output capture paths,
//! `CYRUP_SUBAGENT_BINARY`, and every credential this process holds for other providers. Upstream's
//! claude-code allowlist is 32 named keys precisely so the foreign CLI sees its own credentials and
//! nothing else (`claude-code-adapter.ts:10-42`).
//!
//! So the environment is a two-variant enum built at the boundary, and [`ExternalEnv::Allowlisted`]
//! has exactly one constructor, which rejects an unlisted injected value and a malformed key. There
//! is no way to hand the runner a bag of strings and hope.

use std::collections::{BTreeMap, BTreeSet};

/// A validated environment variable name: non-empty, and free of `=` and NUL
/// (`external-cli-runner.ts:93`).
///
/// The inner field is private, so the only way to obtain one is [`Self::parse`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EnvKey(String);

impl EnvKey {
    /// Upstream's `if (!key || key.includes("=") || key.includes("\0")) throw` (`:93`).
    ///
    /// # Errors
    ///
    /// Upstream's message, with the key JSON-quoted exactly as `JSON.stringify(key)` renders it.
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty() || raw.contains('=') || raw.contains('\0') {
            return Err(format!(
                "Invalid external CLI environment key: {}.",
                serde_json::Value::String(raw.to_string())
            ));
        }
        Ok(Self(raw.to_string()))
    }

    /// The validated name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How an external child's environment is built.
///
/// No `Default`, no `From<HashMap<_, _>>`: the choice between inheriting the orchestrator's
/// environment and projecting it through an allowlist is a decision someone has to write down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalEnv {
    /// The generic `external-cli` path with no adapter — upstream inherits `process.env` whole
    /// (`:89`, minus its extension-bindings key). This is upstream's own choice, not an inference:
    /// an author who names a bare `command:` gets the environment they would have got from a shell.
    ///
    /// [CYRUP-DELTA] upstream strips `PI_SUBAGENT_EXTENSION_BINDINGS`
    /// (`extension-bindings.ts:74-77`); cyrup has no extension-bindings env family, so there is
    /// nothing to strip and the arm is a plain inherit.
    Inherited,
    /// The adapter path: an allowlist projection of the parent environment, plus adapter-injected
    /// `values` whose keys MUST all be in the allowlist.
    Allowlisted {
        /// Keys copied from the parent environment when present.
        allow: BTreeSet<EnvKey>,
        /// Adapter-injected values, overriding the projection.
        values: BTreeMap<EnvKey, String>,
    },
}

impl ExternalEnv {
    /// The only constructor for [`Self::Allowlisted`] (`:90-99`).
    ///
    /// # Errors
    ///
    /// A malformed key (from [`EnvKey::parse`]), or an injected value whose key is not in the
    /// allowlist — upstream's `External CLI environment value '<key>' is not in the adapter
    /// allowlist.`
    pub fn allowlisted(allow: &[&str], values: &[(&str, &str)]) -> Result<Self, String> {
        let allow: BTreeSet<EnvKey> = allow
            .iter()
            .map(|key| EnvKey::parse(key))
            .collect::<Result<_, _>>()?;
        let mut injected = BTreeMap::new();
        for (key, value) in values {
            let key = EnvKey::parse(key)?;
            if !allow.contains(&key) {
                return Err(format!(
                    "External CLI environment value '{}' is not in the adapter allowlist.",
                    key.as_str()
                ));
            }
            injected.insert(key, (*value).to_string());
        }
        Ok(Self::Allowlisted {
            allow,
            values: injected,
        })
    }

    /// Materialise the environment against a lookup — `std::env::var` in production, a fixed map in
    /// tests. The lookup is injected rather than read directly so this stays a pure function of its
    /// inputs (and because this crate forbids `unsafe`, so a test cannot mutate the real
    /// environment).
    ///
    /// [`Self::Inherited`] returns `None`, which the runner reads as "do not touch the child's
    /// environment"; [`Self::Allowlisted`] returns the COMPLETE environment the child gets, which
    /// the runner installs after `env_clear()`.
    #[must_use]
    pub fn materialise(
        &self,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<BTreeMap<String, String>> {
        match self {
            Self::Inherited => None,
            Self::Allowlisted { allow, values } => {
                let mut env = BTreeMap::new();
                for key in allow {
                    // `if (process.env[key] !== undefined) env[key] = process.env[key];` — an unset
                    // allowlisted key is simply absent, never an empty string.
                    if let Some(value) = get(key.as_str()) {
                        env.insert(key.as_str().to_string(), value);
                    }
                }
                for (key, value) in values {
                    env.insert(key.as_str().to_string(), value.clone());
                }
                Some(env)
            }
        }
    }
}

/// [`ExternalEnv::materialise`]'s production lookup.
#[must_use]
pub fn process_env_lookup(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// The key validator is upstream's, including the JSON-quoted key in the message (`:93`).
    #[test]
    fn a_key_with_an_equals_a_nul_or_no_bytes_at_all_cannot_be_constructed() {
        assert_eq!(EnvKey::parse("PATH").unwrap().as_str(), "PATH");
        assert_eq!(
            EnvKey::parse("A=B").unwrap_err(),
            "Invalid external CLI environment key: \"A=B\"."
        );
        assert!(EnvKey::parse("A\0B").is_err());
        assert_eq!(
            EnvKey::parse("").unwrap_err(),
            "Invalid external CLI environment key: \"\"."
        );
    }

    /// An injected value may only name an ALLOWLISTED key (`:96-98`) — the guard that stops an
    /// adapter smuggling a variable past its own declared surface.
    #[test]
    fn an_injected_value_outside_the_allowlist_is_refused() {
        assert_eq!(
            ExternalEnv::allowlisted(&["PATH"], &[("ANTHROPIC_API_KEY", "sk-x")]).unwrap_err(),
            "External CLI environment value 'ANTHROPIC_API_KEY' is not in the adapter allowlist."
        );
        assert!(ExternalEnv::allowlisted(&["PATH"], &[("PATH", "/bin")]).is_ok());
        assert!(ExternalEnv::allowlisted(&["BAD=KEY"], &[]).is_err());
    }

    /// The projection copies only listed keys that are actually SET, and an injected value
    /// overrides the projected one.
    #[test]
    fn the_projection_carries_only_listed_and_present_keys() {
        let env = ExternalEnv::allowlisted(&["PATH", "HOME", "UNSET_KEY"], &[("HOME", "/adapter")])
            .unwrap();
        let parent = |key: &str| match key {
            "PATH" => Some("/usr/bin".to_string()),
            "HOME" => Some("/parent".to_string()),
            "CYRUP_SUBAGENT_PERMISSION_POLICY" => Some("{}".to_string()),
            "ANTHROPIC_API_KEY" => Some("sk-secret".to_string()),
            _ => None,
        };
        let materialised = env.materialise(&parent).unwrap();
        assert_eq!(
            materialised.get("PATH").map(String::as_str),
            Some("/usr/bin")
        );
        assert_eq!(
            materialised.get("HOME").map(String::as_str),
            Some("/adapter"),
            "an injected value overrides the projected one"
        );
        assert!(!materialised.contains_key("UNSET_KEY"));
        assert!(
            !materialised.contains_key("CYRUP_SUBAGENT_PERMISSION_POLICY"),
            "the orchestrator's own subagent configuration must never reach a foreign CLI"
        );
        assert!(
            !materialised.contains_key("ANTHROPIC_API_KEY"),
            "an unlisted credential must not leak even when the parent holds it"
        );
    }

    /// The generic arm is an explicit decision, and it is distinguishable at the type level from
    /// "an allowlist I forgot to fill in".
    #[test]
    fn the_inherited_arm_materialises_to_no_override_at_all() {
        assert_eq!(ExternalEnv::Inherited.materialise(&|_| None), None);
        assert_eq!(
            ExternalEnv::allowlisted(&[], &[])
                .unwrap()
                .materialise(&|_| Some("x".to_string())),
            Some(BTreeMap::new()),
            "an EMPTY allowlist seals the environment completely; it is not an inherit"
        );
    }
}
