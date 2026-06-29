//! Model resolution: pattern matching (`provider/id`, bare id, partial/alias), the `:level`
//! thinking shorthand, per-provider defaults, scoping + cycling, and custom `models.json`
//! (arch-07 §3.6/§6.4, R-07-019…R-07-023).

use std::path::Path;

use cyrup_core::{ProviderId, ModelThinkingLevel};
use cyrup_provider::Model;

use crate::error::ConfigError;

/// Parse a thinking-level token (`off|minimal|low|medium|high|xhigh`).
pub fn parse_thinking_level(s: &str) -> Option<ModelThinkingLevel> {
    match s.trim().to_ascii_lowercase().as_str() {
        "off" => Some(ModelThinkingLevel::Off),
        "minimal" => Some(ModelThinkingLevel::Minimal),
        "low" => Some(ModelThinkingLevel::Low),
        "medium" => Some(ModelThinkingLevel::Medium),
        "high" => Some(ModelThinkingLevel::High),
        "xhigh" => Some(ModelThinkingLevel::Xhigh),
        _ => None,
    }
}

/// A model + an optional scoped thinking level.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopedModel {
    pub model: Model,
    pub thinking_level: Option<ModelThinkingLevel>,
}

/// Outcome of parsing a model pattern (arch-07 §3.6). `warning` is surfaced, never panics.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedModel {
    pub model: Option<Model>,
    pub thinking_level: Option<ModelThinkingLevel>,
    pub warning: Option<String>,
    pub ambiguous: bool,
}

enum Match<'a> {
    None,
    One(&'a Model),
    Ambiguous(Vec<&'a Model>),
}

/// `true` if a model id looks like a dated version (`…-YYYYMMDD`), i.e. NOT an alias.
fn is_dated(id: &str) -> bool {
    match id.rsplit('-').next() {
        Some(tail) => tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

fn is_alias(id: &str) -> bool {
    id.ends_with("-latest") || !is_dated(id)
}

/// Resolves model patterns against an available model set (R-07-019).
pub struct ModelResolver<'a> {
    available: &'a [Model],
}

impl<'a> ModelResolver<'a> {
    pub fn new(available: &'a [Model]) -> Self {
        Self { available }
    }

    fn match_reference(&self, reference: &str) -> Match<'a> {
        let reference = reference.trim();
        if reference.is_empty() {
            return Match::None;
        }
        let lower = reference.to_ascii_lowercase();

        // 1. exact provider/id (case-insensitive) — unambiguous.
        if let Some((prov, id)) = reference.split_once('/') {
            let prov = prov.to_ascii_lowercase();
            let id = id.to_ascii_lowercase();
            if let Some(m) = self.available.iter().find(|m| {
                m.provider.as_str().to_ascii_lowercase() == prov
                    && m.id.as_str().to_ascii_lowercase() == id
            }) {
                return Match::One(m);
            }
        }

        // 2. bare exact id (ambiguous across providers ⇒ error).
        let exact: Vec<&Model> =
            self.available.iter().filter(|m| m.id.as_str().to_ascii_lowercase() == lower).collect();
        match exact.len() {
            1 => {
                if let Some(m) = exact.first() {
                    return Match::One(m);
                }
            }
            n if n > 1 => return Match::Ambiguous(exact),
            _ => {}
        }

        // 3. partial match against id or name; alias-preferred, highest-sorting on ties.
        let mut partial: Vec<&Model> = self
            .available
            .iter()
            .filter(|m| {
                m.id.as_str().to_ascii_lowercase().contains(&lower)
                    || m.name.to_ascii_lowercase().contains(&lower)
            })
            .collect();
        if partial.is_empty() {
            return Match::None;
        }
        partial.sort_by(|a, b| {
            // alias first, then highest-sorting id (descending).
            let aa = is_alias(a.id.as_str());
            let ba = is_alias(b.id.as_str());
            ba.cmp(&aa).then_with(|| b.id.as_str().cmp(a.id.as_str()))
        });
        match partial.first() {
            Some(m) => Match::One(m),
            None => Match::None,
        }
    }

    /// Exact/partial lookup; `Ok(None)` = no match, `Err` = ambiguous bare id (R-07-019).
    pub fn find_exact(&self, reference: &str) -> Result<Option<&'a Model>, String> {
        match self.match_reference(reference) {
            Match::One(m) => Ok(Some(m)),
            Match::None => Ok(None),
            Match::Ambiguous(v) => {
                let providers: Vec<String> =
                    v.iter().map(|m| format!("{}/{}", m.provider, m.id)).collect();
                Err(format!("ambiguous model id '{reference}': matches {}", providers.join(", ")))
            }
        }
    }

    /// Parse a `pattern[:level]` (R-07-020). `strict` (CLI `--model`) refuses to guess on an
    /// invalid trailing token; non-strict (scope mode) warns and recurses on the prefix.
    pub fn parse_pattern(&self, pattern: &str, strict: bool) -> ParsedModel {
        match self.match_reference(pattern) {
            Match::One(m) => {
                return ParsedModel {
                    model: Some(m.clone()),
                    thinking_level: None,
                    warning: None,
                    ambiguous: false,
                };
            }
            Match::Ambiguous(v) => {
                let providers: Vec<String> =
                    v.iter().map(|m| format!("{}/{}", m.provider, m.id)).collect();
                return ParsedModel {
                    model: None,
                    thinking_level: None,
                    warning: Some(format!(
                        "ambiguous model id '{pattern}': matches {}",
                        providers.join(", ")
                    )),
                    ambiguous: true,
                };
            }
            Match::None => {}
        }

        // Strip a trailing `:<token>` (colon-safe: split at the LAST colon, recurse on prefix).
        let Some(idx) = pattern.rfind(':') else {
            return ParsedModel { model: None, thinking_level: None, warning: None, ambiguous: false };
        };
        let (prefix, rest) = pattern.split_at(idx);
        let suffix = rest.get(1..).unwrap_or("");

        if let Some(level) = parse_thinking_level(suffix) {
            let inner = self.parse_pattern(prefix, strict);
            let thinking = if inner.warning.is_some() || inner.ambiguous { None } else { Some(level) };
            ParsedModel {
                model: inner.model,
                thinking_level: thinking,
                warning: inner.warning,
                ambiguous: inner.ambiguous,
            }
        } else if strict {
            ParsedModel { model: None, thinking_level: None, warning: None, ambiguous: false }
        } else {
            let inner = self.parse_pattern(prefix, strict);
            ParsedModel {
                model: inner.model,
                thinking_level: None,
                warning: Some(format!("invalid thinking level '{suffix}'")),
                ambiguous: inner.ambiguous,
            }
        }
    }

    /// Per-provider default model (R-07-021): an alias-preferred model for the provider.
    pub fn provider_default(&self, provider: &ProviderId) -> Option<&'a Model> {
        let lower = provider.as_str().to_ascii_lowercase();
        let mut candidates: Vec<&Model> = self
            .available
            .iter()
            .filter(|m| m.provider.as_str().to_ascii_lowercase() == lower)
            .collect();
        candidates.sort_by(|a, b| {
            let aa = is_alias(a.id.as_str());
            let ba = is_alias(b.id.as_str());
            ba.cmp(&aa).then_with(|| b.id.as_str().cmp(a.id.as_str()))
        });
        candidates.into_iter().next()
    }

    /// Expand scope patterns (incl. simple `*` globs) into an ordered, de-duplicated candidate set
    /// (R-07-022).
    pub fn resolve_scope(&self, patterns: &[String]) -> Vec<ScopedModel> {
        let mut out: Vec<ScopedModel> = Vec::new();
        let mut seen: Vec<(String, String)> = Vec::new();
        let push = |model: Model, level: Option<ModelThinkingLevel>, seen: &mut Vec<(String, String)>, out: &mut Vec<ScopedModel>| {
            let key = (model.provider.as_str().to_string(), model.id.as_str().to_string());
            if !seen.contains(&key) {
                seen.push(key);
                out.push(ScopedModel { model, thinking_level: level });
            }
        };

        for pattern in patterns {
            if pattern.contains('*') {
                for m in self.available.iter().filter(|m| {
                    glob_match(pattern, &format!("{}/{}", m.provider, m.id))
                        || glob_match(pattern, m.id.as_str())
                }) {
                    push(m.clone(), None, &mut seen, &mut out);
                }
            } else {
                let parsed = self.parse_pattern(pattern, false);
                if let Some(m) = parsed.model {
                    push(m, parsed.thinking_level, &mut seen, &mut out);
                }
            }
        }
        out
    }
}

/// Minimal `*`-glob matcher (no external regex/glob dep). `*` matches any run of characters.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p = pattern.to_ascii_lowercase();
    let t = text.to_ascii_lowercase();
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == t;
    }
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !t.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == parts.len() - 1 {
            return t.get(pos..).is_some_and(|rest| rest.ends_with(part));
        } else {
            match t.get(pos..).and_then(|rest| rest.find(part)) {
                Some(found) => pos += found + part.len(),
                None => return false,
            }
        }
    }
    true
}

/// Cursor over candidate models for Ctrl+P / Ctrl+N cycling (R-07-022).
pub struct ModelCycler {
    candidates: Vec<ScopedModel>,
    idx: usize,
}

impl ModelCycler {
    pub fn new(candidates: Vec<ScopedModel>) -> Self {
        Self { candidates, idx: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    /// Advance to the next candidate, reporting (model, current thinking level).
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<(&Model, ModelThinkingLevel)> {
        if self.candidates.is_empty() {
            return None;
        }
        self.idx = (self.idx + 1) % self.candidates.len();
        self.current()
    }

    pub fn prev(&mut self) -> Option<(&Model, ModelThinkingLevel)> {
        if self.candidates.is_empty() {
            return None;
        }
        self.idx = (self.idx + self.candidates.len() - 1) % self.candidates.len();
        self.current()
    }

    pub fn current(&self) -> Option<(&Model, ModelThinkingLevel)> {
        self.candidates
            .get(self.idx)
            .map(|sm| (&sm.model, sm.thinking_level.unwrap_or_default()))
    }
}

/// Load custom OpenAI/Anthropic/Google-compatible model defs from a `models.json` (R-07-023).
pub fn load_custom_models(path: &Path) -> Result<Vec<Model>, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ConfigError::Io(e)),
    };
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let models: Vec<Model> = serde_json::from_str(&text)?;
    Ok(models)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use cyrup_provider::{ApiId, Modality, ModelCost};

    fn model(provider: &str, id: &str, name: &str) -> Model {
        Model {
            id: id.into(),
            name: name.to_string(),
            api: ApiId::from("anthropic-messages"),
            provider: provider.into(),
            base_url: None,
            reasoning: true,
            input: vec![Modality::Text],
            output: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 200_000,
            max_tokens: 8192,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    #[test]
    fn thinking_shorthand_resolves_model_and_level() {
        // A-07-6: claude-opus:high
        let models = vec![model("anthropic", "claude-opus-4-latest", "Claude Opus 4")];
        let r = ModelResolver::new(&models);
        let parsed = r.parse_pattern("claude-opus:high", true);
        assert_eq!(parsed.model.as_ref().unwrap().id.as_str(), "claude-opus-4-latest");
        assert_eq!(parsed.thinking_level, Some(ModelThinkingLevel::High));
    }

    #[test]
    fn ambiguous_bare_id_errors() {
        // A-07-6
        let models = vec![model("a", "shared", "A Shared"), model("b", "shared", "B Shared")];
        let r = ModelResolver::new(&models);
        let err = r.find_exact("shared");
        assert!(err.is_err());
        let parsed = r.parse_pattern("shared", true);
        assert!(parsed.ambiguous);
        assert!(parsed.model.is_none());
    }

    #[test]
    fn latest_preferred_over_dated() {
        // A-07-6: -latest preferred over a dated alias.
        let models = vec![
            model("anthropic", "claude-3-5-sonnet-20241022", "Sonnet dated"),
            model("anthropic", "claude-3-5-sonnet-latest", "Sonnet latest"),
        ];
        let r = ModelResolver::new(&models);
        let parsed = r.parse_pattern("claude-3-5-sonnet", false);
        assert_eq!(parsed.model.as_ref().unwrap().id.as_str(), "claude-3-5-sonnet-latest");
    }

    #[test]
    fn exact_provider_id_case_insensitive() {
        let models = vec![model("OpenAI", "GPT-4o", "GPT-4o")];
        let r = ModelResolver::new(&models);
        let m = r.find_exact("openai/gpt-4o").unwrap().unwrap();
        assert_eq!(m.id.as_str(), "GPT-4o");
    }

    #[test]
    fn colon_in_id_is_handled() {
        let models = vec![model("openai", "gpt-4o:extended", "GPT extended")];
        let r = ModelResolver::new(&models);
        // exact match on an id that contains a colon
        let parsed = r.parse_pattern("openai/gpt-4o:extended", true);
        assert_eq!(parsed.model.as_ref().unwrap().id.as_str(), "gpt-4o:extended");
        assert_eq!(parsed.thinking_level, None);
    }

    #[test]
    fn invalid_thinking_level_strict_vs_scope() {
        let models = vec![model("anthropic", "claude-opus-latest", "Opus")];
        let r = ModelResolver::new(&models);
        // strict: don't guess
        let strict = r.parse_pattern("claude-opus:bogus", true);
        assert!(strict.model.is_none());
        // scope: warn + recurse on prefix
        let scope = r.parse_pattern("claude-opus:bogus", false);
        assert!(scope.model.is_some());
        assert!(scope.warning.is_some());
        assert_eq!(scope.thinking_level, None);
    }

    #[test]
    fn provider_default_picks_alias() {
        let models = vec![
            model("anthropic", "claude-3-5-sonnet-20241022", "dated"),
            model("anthropic", "claude-3-5-sonnet-latest", "latest"),
        ];
        let r = ModelResolver::new(&models);
        let d = r.provider_default(&ProviderId::from("anthropic")).unwrap();
        assert_eq!(d.id.as_str(), "claude-3-5-sonnet-latest");
    }

    #[test]
    fn scope_and_cycle() {
        // R-07-022
        let models = vec![
            model("anthropic", "claude-opus-latest", "Opus"),
            model("anthropic", "claude-haiku-latest", "Haiku"),
            model("openai", "gpt-4o", "GPT-4o"),
        ];
        let r = ModelResolver::new(&models);
        let scoped = r.resolve_scope(&["anthropic/*".to_string()]);
        assert_eq!(scoped.len(), 2);
        let mut cycler = ModelCycler::new(scoped);
        let (m1, _) = cycler.current().unwrap();
        let id1 = m1.id.as_str().to_string();
        let (m2, lvl) = cycler.next().unwrap();
        assert_ne!(m2.id.as_str(), id1);
        assert_eq!(lvl, ModelThinkingLevel::Off);
        // wraps around
        cycler.next();
        let (m_wrap, _) = cycler.current().unwrap();
        assert_eq!(m_wrap.id.as_str(), id1);
    }

    #[test]
    fn load_custom_models_roundtrip() {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("models.json");
        let models = vec![model("custom", "my-model", "My Model")];
        std::fs::write(&path, serde_json::to_string(&models).unwrap()).unwrap();
        let loaded = load_custom_models(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.first().unwrap().id.as_str(), "my-model");
        // missing file → empty
        assert!(load_custom_models(&dir.join("nope.json")).unwrap().is_empty());
    }
}
