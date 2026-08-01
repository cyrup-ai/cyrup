//! Skills — Agent Skills standard `SKILL.md` (arch-09 §3.3, R-09-001..006/026).
//!
//! A skill is a directory containing a `SKILL.md` with YAML front-matter (`name` + a
//! "use this skill when…" `description`) followed by the body. Only the front-matter is parsed
//! at discovery time; the body is read lazily (R-09-026).

use std::path::{Path, PathBuf};

use crate::discovery::Named;
use crate::error::{ResourceDiagnostic, ResourceError, ResourceKind};
use crate::key::ResourceKey;
use crate::scope::{ResourceOrigin, ResourceScope};

/// Max skill name length per the Agent Skills spec (skills.ts:11).
pub const MAX_NAME_LENGTH: usize = 64;
/// Max skill description length per the Agent Skills spec (skills.ts:14).
pub const MAX_DESCRIPTION_LENGTH: usize = 1024;

/// Parsed YAML front-matter. Unknown keys are tolerated and round-trip via `extra`
/// (forward-compat, arch-00 serde policy).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillFrontMatter {
    /// Optional in the file — falls back to the parent directory name (skills.ts:287,296).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// "use this skill when…". Required: a skill with no description is dropped (skills.ts:305-307).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `disable-model-invocation: true` excludes the skill from the prompt; it can then only be
    /// invoked explicitly via `/skill:name` (skills.ts:70,316,335-336).
    #[serde(
        default,
        rename = "disable-model-invocation",
        alias = "disableModelInvocation",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub disable_model_invocation: bool,
    /// Standard optional field. Accepts the Agent Skills standard kebab key `allowed-tools` as
    /// well as `allowedTools` (A-09-10, cross-harness fidelity).
    #[serde(
        default,
        alias = "allowed-tools",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub allowed_tools: Vec<String>,
    /// Unmodelled keys round-trip unchanged.
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_yml::Value>,
}

/// Validate a skill name per the Agent Skills spec (skills.ts:92-112). Returns human-readable error
/// messages (empty when valid). Pi keeps the skill even with name warnings.
pub fn validate_name(name: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let len = name.chars().count();
    if len > MAX_NAME_LENGTH {
        errors.push(format!("name exceeds {MAX_NAME_LENGTH} characters ({len})"));
    }
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        errors.push(
            "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)"
                .to_string(),
        );
    }
    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".to_string());
    }
    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".to_string());
    }
    errors
}

/// Validate a skill description per the Agent Skills spec (skills.ts:117-127).
pub fn validate_description(description: Option<&str>) -> Vec<String> {
    let mut errors = Vec::new();
    match description {
        None => errors.push("description is required".to_string()),
        Some(d) if d.trim().is_empty() => errors.push("description is required".to_string()),
        Some(d) => {
            let len = d.chars().count();
            if len > MAX_DESCRIPTION_LENGTH {
                errors.push(format!(
                    "description exceeds {MAX_DESCRIPTION_LENGTH} characters ({len})"
                ));
            }
        }
    }
    errors
}

/// A discovered skill. The body lives on disk and is read on demand (R-09-026).
#[derive(Clone, Debug)]
pub struct Skill {
    pub key: ResourceKey,
    /// Resolved name: frontmatter `name`, or the parent directory basename when absent
    /// (skills.ts:287,296). Case preserved (validation may still flag it).
    pub name: String,
    pub front: SkillFrontMatter,
    /// `disable-model-invocation` (skills.ts:316,335-336) — excluded from the prompt when true.
    pub disable_model_invocation: bool,
    /// Skill directory; supporting files resolve relative to here.
    pub dir: PathBuf,
    /// Path to `SKILL.md` — body read on demand.
    pub skill_md: PathBuf,
    pub scope: ResourceScope,
    pub origin: ResourceOrigin,
}

/// Short pointer injected into the system prompt by arch-06 (R-09-004). Body excluded.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillPointer {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The agent opens this with the `read` tool on demand (DI-4).
    pub path: PathBuf,
    /// Mirror of [`Skill::disable_model_invocation`] (`disable-model-invocation`, skills.ts:316).
    /// When true the skill is EXCLUDED from the `<available_skills>` prompt section
    /// (`formatSkillsForPrompt`, skills.ts:335-336) but stays in the pointer set so its explicit
    /// `/skill:name` command is still registered.
    #[serde(default)]
    pub disable_model_invocation: bool,
}

impl Skill {
    /// Metadata-only pointer; performs no IO (R-09-004).
    pub fn pointer(&self) -> SkillPointer {
        SkillPointer {
            name: self.name.clone(),
            description: self.front.description.clone(),
            path: self.skill_md.clone(),
            disable_model_invocation: self.disable_model_invocation,
        }
    }

    /// The explicit command form (R-09-005).
    pub fn command(&self) -> String {
        format!("/skill:{}", self.key)
    }

    /// Lazy body load (R-09-026): everything after the front-matter block.
    pub async fn read_body(&self) -> Result<String, ResourceError> {
        let raw = tokio::fs::read_to_string(&self.skill_md).await?;
        // Pi `stripFrontmatter` = `parseFrontmatter(content).body` (frontmatter.ts:39): the
        // normalized, fence-stripped, trimmed body.
        Ok(split_front_matter(&raw).1)
    }

    /// Parse a `SKILL.md` at `skill_md` into a [`Skill`] (front-matter only). Used by discovery
    /// and by direct `--skill` loads. Errors when the description is missing (Pi drops such skills,
    /// skills.ts:305-307). Name falls back to the parent directory basename (skills.ts:287,296).
    pub fn load(
        skill_md: &Path,
        scope: ResourceScope,
        origin: ResourceOrigin,
    ) -> Result<Skill, ResourceError> {
        let (skill, _diags) = Self::load_with_diagnostics(skill_md, scope, origin)?;
        skill.ok_or_else(|| ResourceError::Skill {
            path: skill_md.to_path_buf(),
            reason: "description is required".to_string(),
        })
    }

    /// Parse a `SKILL.md` collecting non-fatal validation diagnostics (skills.ts:loadSkillFromFile,
    /// lines 274-336). Returns `Ok((None, diags))` when the skill is dropped (missing description)
    /// but the file was otherwise readable; `Err` only on IO/parse faults.
    pub fn load_with_diagnostics(
        skill_md: &Path,
        scope: ResourceScope,
        origin: ResourceOrigin,
    ) -> Result<(Option<Skill>, Vec<ResourceDiagnostic>), ResourceError> {
        let raw = std::fs::read_to_string(skill_md)?;
        let front: SkillFrontMatter = match split_front_matter(&raw).0 {
            Some(front_str) => {
                serde_yml::from_str(&front_str).map_err(|e| ResourceError::FrontMatter {
                    path: skill_md.to_path_buf(),
                    reason: e.to_string(),
                })?
            }
            // No frontmatter block → empty frontmatter (utils/frontmatter.ts returns `{}`).
            None => SkillFrontMatter {
                name: None,
                description: None,
                disable_model_invocation: false,
                allowed_tools: Vec::new(),
                extra: std::collections::BTreeMap::new(),
            },
        };

        let dir = skill_md.parent().map(Path::to_path_buf).unwrap_or_default();
        let parent_dir_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        // name = frontmatter name, else parent directory name (skills.ts:296).
        let name = match front.name.as_deref() {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => parent_dir_name,
        };

        let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();
        for msg in validate_description(front.description.as_deref()) {
            diagnostics.push(ResourceDiagnostic::warning(
                ResourceKind::Skill,
                skill_md,
                msg,
            ));
        }
        for msg in validate_name(&name) {
            diagnostics.push(ResourceDiagnostic::warning(
                ResourceKind::Skill,
                skill_md,
                msg,
            ));
        }

        // Drop the skill entirely when the description is missing/blank (skills.ts:305-307).
        let has_description = front
            .description
            .as_deref()
            .is_some_and(|d| !d.trim().is_empty());
        if !has_description {
            return Ok((None, diagnostics));
        }

        let key = ResourceKey::normalize(&name);
        if key.is_empty() {
            return Ok((None, diagnostics));
        }
        let disable_model_invocation = front.disable_model_invocation;
        let skill = Skill {
            key,
            name,
            front,
            disable_model_invocation,
            dir,
            skill_md: skill_md.to_path_buf(),
            scope,
            origin,
        };
        Ok((Some(skill), diagnostics))
    }
}

impl Named for Skill {
    fn key(&self) -> &ResourceKey {
        &self.key
    }
    fn scope(&self) -> ResourceScope {
        self.scope
    }
}

/// Normalize newlines exactly as Pi's `normalizeNewlines` (utils/frontmatter.ts:8): `\r\n` → `\n`,
/// then any remaining bare `\r` → `\n`.
fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

/// Split the leading `---` front-matter block from the body. Returns `(yaml_string, body)`, a 1:1
/// port of Pi's `extractFrontmatter` (utils/frontmatter.ts:10-26).
///
/// The whole input is first newline-normalized (`\r\n`/`\r` → `\n`, frontmatter.ts:8,11). When a
/// well-formed block is present the returned `body` is `trim()`ed (frontmatter.ts:24); otherwise
/// `yaml_string` is `None` and `body` is the normalized whole content (Pi yields `{}` + that body,
/// frontmatter.ts:14,19,33).
///
/// Fence detection is intentionally loose to match Pi byte-for-byte: the content must merely
/// *start with* `---` (not `---\n`), and the block ends at the first `\n---` substring at or after
/// byte offset 3 (frontmatter.ts:13,17). `yaml_string = slice(4, endIndex)` and the body begins
/// after the closing `\n---` (frontmatter.ts:23-24) — the close need not be its own line.
pub(crate) fn split_front_matter(raw: &str) -> (Option<String>, String) {
    let normalized = normalize_newlines(raw);
    // Pi: `if (!normalized.startsWith("---")) return { yamlString: null, body: normalized }`.
    if !normalized.starts_with("---") {
        return (None, normalized);
    }
    // Pi: `endIndex = normalized.indexOf("\n---", 3)`; -1 → no front-matter.
    let Some(rel) = normalized.get(3..).and_then(|s| s.find("\n---")) else {
        return (None, normalized);
    };
    let end_index = 3 + rel;
    // Pi: `yamlString = normalized.slice(4, endIndex)` (skips the opening `---` + one char).
    let yaml = normalized.get(4..end_index).unwrap_or("").to_string();
    // Pi: `body = normalized.slice(endIndex + 4).trim()` (skips the closing `\n---`).
    let body = normalized
        .get(end_index.saturating_add(4)..)
        .unwrap_or("")
        .trim()
        .to_string();
    (Some(yaml), body)
}
