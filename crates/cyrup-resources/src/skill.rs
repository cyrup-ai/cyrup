//! Skills — Agent Skills standard `SKILL.md` (arch-09 §3.3, R-09-001..006/026).
//!
//! A skill is a directory containing a `SKILL.md` with YAML front-matter (`name` + a
//! "use this skill when…" `description`) followed by the body. Only the front-matter is parsed
//! at discovery time; the body is read lazily (R-09-026).

use std::path::{Path, PathBuf};

use crate::discovery::Named;
use crate::error::ResourceError;
use crate::key::ResourceKey;
use crate::scope::{ResourceOrigin, ResourceScope};

/// Parsed YAML front-matter. Unknown keys are tolerated and round-trip via `extra`
/// (forward-compat, arch-00 serde policy).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillFrontMatter {
    /// Required (R-09-001).
    pub name: String,
    /// "use this skill when…".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Standard optional field. Accepts the Agent Skills standard kebab key `allowed-tools` as
    /// well as `allowedTools` (A-09-10, cross-harness fidelity).
    #[serde(default, alias = "allowed-tools", skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// Unmodelled keys round-trip unchanged.
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_yml::Value>,
}

/// A discovered skill. The body lives on disk and is read on demand (R-09-026).
#[derive(Clone, Debug)]
pub struct Skill {
    pub key: ResourceKey,
    pub front: SkillFrontMatter,
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
}

impl Skill {
    /// Metadata-only pointer; performs no IO (R-09-004).
    pub fn pointer(&self) -> SkillPointer {
        SkillPointer {
            name: self.front.name.clone(),
            description: self.front.description.clone(),
            path: self.skill_md.clone(),
        }
    }

    /// The explicit command form (R-09-005).
    pub fn command(&self) -> String {
        format!("/skill:{}", self.key)
    }

    /// Lazy body load (R-09-026): everything after the front-matter block.
    pub async fn read_body(&self) -> Result<String, ResourceError> {
        let raw = tokio::fs::read_to_string(&self.skill_md).await?;
        Ok(split_front_matter(&raw).map(|(_, body)| body.to_string()).unwrap_or(raw))
    }

    /// Parse a `SKILL.md` at `skill_md` into a [`Skill`] (front-matter only). Used by discovery
    /// and by direct `--skill` loads.
    pub fn load(
        skill_md: &Path,
        scope: ResourceScope,
        origin: ResourceOrigin,
    ) -> Result<Skill, ResourceError> {
        let raw = std::fs::read_to_string(skill_md)?;
        let (front_str, _body) = split_front_matter(&raw).ok_or_else(|| ResourceError::Skill {
            path: skill_md.to_path_buf(),
            reason: "missing YAML front-matter (expected leading `---` block)".to_string(),
        })?;
        let front: SkillFrontMatter =
            serde_yml::from_str(front_str).map_err(|e| ResourceError::FrontMatter {
                path: skill_md.to_path_buf(),
                reason: e.to_string(),
            })?;
        let key = ResourceKey::normalize(&front.name);
        if key.is_empty() {
            return Err(ResourceError::Skill {
                path: skill_md.to_path_buf(),
                reason: "front-matter `name` is empty".to_string(),
            });
        }
        let dir = skill_md.parent().map(Path::to_path_buf).unwrap_or_default();
        Ok(Skill { key, front, dir, skill_md: skill_md.to_path_buf(), scope, origin })
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

/// Split a `---\n…\n---` front-matter block from the body. Returns `(front_matter, body)`.
///
/// The opening fence must be the very first line. Returns `None` when there is no well-formed
/// front-matter block (the caller treats the whole file as body / an error).
pub(crate) fn split_front_matter(raw: &str) -> Option<(&str, &str)> {
    // Accept an optional UTF-8 BOM, then a leading `---` line.
    let s = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let after_open = s.strip_prefix("---\n").or_else(|| s.strip_prefix("---\r\n"))?;
    // Find the closing fence: a line that is exactly `---`.
    let mut idx = 0usize;
    for line in after_open.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" {
            let front = after_open.get(..idx)?;
            let body_start = idx.checked_add(line.len())?;
            let body = after_open.get(body_start..).unwrap_or("");
            return Some((front, body));
        }
        idx = idx.checked_add(line.len())?;
    }
    None
}
