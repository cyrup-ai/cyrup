//! Core state + config types (port of pi `types.ts:1-43`). All source-certain; host-independent.

use std::collections::BTreeMap;

/// pi `types.ts:1` — the tri-state a permission resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionState {
    Allow,
    Deny,
    Ask,
}

impl PermissionState {
    /// Parse one of the three literal states, else `None` (pi `common.ts:23-25`
    /// `isPermissionState`). Anything not exactly `allow`/`deny`/`ask` is not a state.
    #[must_use]
    pub fn parse(value: &str) -> Option<PermissionState> {
        match value {
            "allow" => Some(PermissionState::Allow),
            "deny" => Some(PermissionState::Deny),
            "ask" => Some(PermissionState::Ask),
            _ => None,
        }
    }
}

/// The origin of a resolved [`PermissionCheckResult`] (pi `types.ts:42`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckSource {
    Tool,
    Bash,
    Mcp,
    Skill,
    Special,
    Default,
}

/// pi `types.ts:15-21` — the per-category default policy. ALL default to `Ask`
/// (`permission-manager.ts:44-50`, `DEFAULT_POLICY`).
#[derive(Debug, Clone, Copy)]
pub struct DefaultPolicy {
    pub tools: PermissionState,
    pub bash: PermissionState,
    pub mcp: PermissionState,
    pub skills: PermissionState,
    pub special: PermissionState,
}

impl Default for DefaultPolicy {
    fn default() -> Self {
        // pi `permission-manager.ts:44-50` — every category defaults to `ask`.
        DefaultPolicy {
            tools: PermissionState::Ask,
            bash: PermissionState::Ask,
            mcp: PermissionState::Ask,
            skills: PermissionState::Ask,
            special: PermissionState::Ask,
        }
    }
}

/// The five permission-record categories (pi `PermissionRecordCategory`,
/// `permission-manager.ts:321`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Tools,
    Bash,
    Mcp,
    Skills,
    Special,
}

/// pi `types.ts:23-30` — one config layer's rules (each map is `pattern -> state`) plus its optional
/// partial default-policy override. Insertion order within a category is preserved (pi iterates
/// `Object.entries`, which is insertion order for string keys) via [`OrderedRules`].
#[derive(Debug, Clone, Default)]
pub struct AgentPermissions {
    /// A partial default-policy override (`defaultPolicy` frontmatter/JSONC key). Only the categories
    /// the layer actually set are `Some`.
    pub default_policy: PartialDefaultPolicy,
    pub tools: OrderedRules,
    pub bash: OrderedRules,
    pub mcp: OrderedRules,
    pub skills: OrderedRules,
    pub special: OrderedRules,
}

impl AgentPermissions {
    /// The rules for one category (pi `layer.permissions[category]`).
    #[must_use]
    pub fn category(&self, category: Category) -> &OrderedRules {
        match category {
            Category::Tools => &self.tools,
            Category::Bash => &self.bash,
            Category::Mcp => &self.mcp,
            Category::Skills => &self.skills,
            Category::Special => &self.special,
        }
    }
}

/// A partial per-category default policy — only the categories a layer explicitly set (pi
/// `normalizePartialPolicy`, `permission-manager.ts:72-97`).
#[derive(Debug, Clone, Copy, Default)]
pub struct PartialDefaultPolicy {
    pub tools: Option<PermissionState>,
    pub bash: Option<PermissionState>,
    pub mcp: Option<PermissionState>,
    pub skills: Option<PermissionState>,
    pub special: Option<PermissionState>,
}

impl PartialDefaultPolicy {
    /// The set value for one default category, if present.
    #[must_use]
    pub fn get(&self, category: DefaultCategory) -> Option<PermissionState> {
        match category {
            DefaultCategory::Tools => self.tools,
            DefaultCategory::Bash => self.bash,
            DefaultCategory::Mcp => self.mcp,
            DefaultCategory::Skills => self.skills,
            DefaultCategory::Special => self.special,
        }
    }
}

/// The default-policy categories (pi `PermissionDefaultCategory`, `permission-manager.ts:322`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultCategory {
    Tools,
    Bash,
    Mcp,
    Skills,
    Special,
}

/// Insertion-ordered `pattern -> state` entries. pi relies on `Object.entries` insertion order for
/// last-match-wins (`compilePermissionPatternsFromLayers`, `permission-manager.ts:363`); a
/// `BTreeMap` would silently re-sort and change which pattern wins, so order is preserved here as a
/// `Vec` with a `BTreeMap`-less dedup-last-write to mirror JS object-key semantics (a repeated key
/// overwrites in place, keeping its original position — pi `{...a, key}`).
#[derive(Debug, Clone, Default)]
pub struct OrderedRules {
    entries: Vec<(String, PermissionState)>,
}

impl OrderedRules {
    #[must_use]
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Insert or overwrite `pattern` with `state`, preserving first-seen position (JS object-key
    /// assignment semantics: re-assigning an existing key keeps its position, updates its value).
    pub fn insert(&mut self, pattern: String, state: PermissionState) {
        if let Some(slot) = self.entries.iter_mut().find(|(p, _)| *p == pattern) {
            slot.1 = state;
        } else {
            self.entries.push((pattern, state));
        }
    }

    /// Iterate entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, PermissionState)> {
        self.entries.iter().map(|(p, s)| (p.as_str(), *s))
    }

    /// Look up a single key's state (pi `record[key]`).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<PermissionState> {
        self.entries.iter().find(|(p, _)| p == key).map(|(_, s)| *s)
    }

    /// True when any entry's state is `allow` (pi `Object.values(...).some(state => state === "allow")`).
    #[must_use]
    pub fn any_allow(&self) -> bool {
        self.entries.iter().any(|(_, s)| *s == PermissionState::Allow)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A flat pattern rule (pi `evaluate-permission.ts:4-8`) — the shape both approval stores hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternRule {
    pub tool: String,
    pub pattern: String,
    pub action: PermissionState,
}

/// The engine's output (pi `types.ts:36-43`).
#[derive(Debug, Clone)]
pub struct PermissionCheckResult {
    pub tool_name: String,
    pub state: PermissionState,
    pub matched_pattern: Option<String>,
    pub command: Option<String>,
    pub target: Option<String>,
    pub source: CheckSource,
}

/// The fully-resolved global config (pi `GlobalPermissionConfig`, `types.ts:32-34`): an
/// [`AgentPermissions`] plus a *complete* default policy (every category resolved). The derived
/// `Default` is pi's `EMPTY_GLOBAL_CONFIG` (`permission-manager.ts:52-59`): an all-`ask`
/// [`DefaultPolicy`] + empty records.
#[derive(Debug, Clone, Default)]
pub struct GlobalPermissionConfig {
    pub default_policy: DefaultPolicy,
    pub permissions: AgentPermissions,
}

/// A shallow merged view (pi `mergePermissions`, `permission-manager.ts:761-788`): scalar
/// `defaultPolicy` per-category `{...global, ...agent}` and per-category record `{...global,
/// ...agent}`. Used only for the `merged.bash`/`merged.mcp`/`hasAllowedSkills` scalar reads; the
/// per-pattern matchers use the layer array, not this merge.
pub type MergedRecords = BTreeMap<String, PermissionState>;
