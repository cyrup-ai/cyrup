//! Model resolution: pattern matching (`provider/id`, bare id, partial/alias), the `:level`
//! thinking shorthand, per-provider defaults, scoping + cycling, and custom `models.json`
//! (arch-07 §3.6/§6.4, R-07-019…R-07-023).

use std::path::Path;

use cyrup_core::{ModelThinkingLevel, ProviderId};
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
        let exact: Vec<&Model> = self
            .available
            .iter()
            .filter(|m| m.id.as_str().to_ascii_lowercase() == lower)
            .collect();
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
                let providers: Vec<String> = v
                    .iter()
                    .map(|m| format!("{}/{}", m.provider, m.id))
                    .collect();
                Err(format!(
                    "ambiguous model id '{reference}': matches {}",
                    providers.join(", ")
                ))
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
                let providers: Vec<String> = v
                    .iter()
                    .map(|m| format!("{}/{}", m.provider, m.id))
                    .collect();
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
            return ParsedModel {
                model: None,
                thinking_level: None,
                warning: None,
                ambiguous: false,
            };
        };
        let (prefix, rest) = pattern.split_at(idx);
        let suffix = rest.get(1..).unwrap_or("");

        if let Some(level) = parse_thinking_level(suffix) {
            let inner = self.parse_pattern(prefix, strict);
            let thinking = if inner.warning.is_some() || inner.ambiguous {
                None
            } else {
                Some(level)
            };
            ParsedModel {
                model: inner.model,
                thinking_level: thinking,
                warning: inner.warning,
                ambiguous: inner.ambiguous,
            }
        } else if strict {
            ParsedModel {
                model: None,
                thinking_level: None,
                warning: None,
                ambiguous: false,
            }
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
        let push = |model: Model,
                    level: Option<ModelThinkingLevel>,
                    seen: &mut Vec<(String, String)>,
                    out: &mut Vec<ScopedModel>| {
            let key = (
                model.provider.as_str().to_string(),
                model.id.as_str().to_string(),
            );
            if !seen.contains(&key) {
                seen.push(key);
                out.push(ScopedModel {
                    model,
                    thinking_level: level,
                });
            }
        };

        for pattern in patterns {
            // Pi treats `*`, `?`, or `[` as glob characters (model-resolver.ts:264).
            if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
                // Strip an optional `:level` thinking suffix (e.g. `provider/*:high`; :266-276).
                let mut glob_pattern = pattern.as_str();
                let mut level: Option<ModelThinkingLevel> = None;
                if let Some(idx) = pattern.rfind(':') {
                    let suffix = pattern.get(idx + 1..).unwrap_or("");
                    if let Some(lvl) = parse_thinking_level(suffix) {
                        level = Some(lvl);
                        glob_pattern = pattern.get(..idx).unwrap_or(pattern);
                    }
                }
                for m in self.available.iter().filter(|m| {
                    glob_match(glob_pattern, &format!("{}/{}", m.provider, m.id))
                        || glob_match(glob_pattern, m.id.as_str())
                }) {
                    push(m.clone(), level, &mut seen, &mut out);
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

/// `minimatch`-style glob matcher (Pi uses `minimatch(.., { nocase: true })`,
/// model-resolver.ts:282). Supports `*` (any run), `?` (one char), and `[...]` character classes
/// (with a leading `!`/`^` negation), case-insensitively. No external dep.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let t: Vec<char> = text.to_ascii_lowercase().chars().collect();
    glob_match_chars(&p, &t)
}

fn glob_match_chars(p: &[char], t: &[char]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    // Backtracking position for the most recent `*` and the text index it started consuming at.
    let (mut star_pi, mut star_ti): (Option<usize>, usize) = (None, 0);
    while ti < t.len() {
        let pc = p.get(pi).copied();
        if pc == Some('*') {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let (Some(tc), true) = (t.get(ti).copied(), pc.is_some())
            && {
                let (m, next) = match_unit(p, pi, tc);
                if m {
                    pi = next;
                }
                m
            }
        {
            ti += 1;
        } else if let Some(sp) = star_pi {
            // Backtrack: let the `*` consume one more text char.
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while p.get(pi).copied() == Some('*') {
        pi += 1;
    }
    pi == p.len()
}

/// Match the pattern unit at `p[pi]` (a `?`, a `[...]` class, or a literal char) against `c`.
/// Returns `(matched, next_pi)` where `next_pi` is the pattern index just past this unit.
fn match_unit(p: &[char], pi: usize, c: char) -> (bool, usize) {
    match p.get(pi).copied() {
        Some('?') => (true, pi + 1),
        Some('[') => {
            let mut j = pi + 1;
            let negate = matches!(p.get(j).copied(), Some('!') | Some('^'));
            if negate {
                j += 1;
            }
            let class_start = j;
            while let Some(cur) = p.get(j).copied() {
                if cur == ']' && j != class_start {
                    break;
                }
                j += 1;
            }
            // `j` now points at the closing `]` (or past the end if unterminated).
            if p.get(j).copied() == Some(']') {
                let matched = class_matches(p, class_start, j, c);
                (matched != negate, j + 1)
            } else {
                // Unterminated class → treat `[` as a literal.
                (c == '[', pi + 1)
            }
        }
        Some(lit) => (lit == c, pi + 1),
        None => (false, pi),
    }
}

/// Whether char `c` is in the class body `p[start..end)` (ranges like `a-z` and bare chars).
fn class_matches(p: &[char], start: usize, end: usize, c: char) -> bool {
    let mut j = start;
    while j < end {
        let cur = match p.get(j).copied() {
            Some(ch) => ch,
            None => break,
        };
        let dash = p.get(j + 1).copied();
        let hi = p.get(j + 2).copied();
        if j + 2 < end && dash == Some('-') && hi.is_some_and(|h| h != ']') {
            if let Some(h) = hi
                && c >= cur
                && c <= h
            {
                return true;
            }
            j += 3;
        } else {
            if cur == c {
                return true;
            }
            j += 1;
        }
    }
    false
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

/// `true` if two models refer to the same provider+id (Pi `modelsAreEqual`, models.ts:435).
fn models_are_equal(a: &Model, b: &Model) -> bool {
    a.id == b.id && a.provider == b.provider
}

/// Curated default model id per known provider (Pi `defaultModelPerProvider`,
/// model-resolver.ts:14-50). Returns `None` for an unknown provider.
pub fn default_model_per_provider(provider: &str) -> Option<&'static str> {
    let id = match provider {
        "amazon-bedrock" => "us.anthropic.claude-opus-4-6-v1",
        "ant-ling" => "Ring-2.6-1T",
        "anthropic" => "claude-opus-4-8",
        "openai" => "gpt-5.5",
        "azure-openai-responses" => "gpt-5.4",
        "openai-codex" => "gpt-5.5",
        "nvidia" => "nvidia/nemotron-3-super-120b-a12b",
        "deepseek" => "deepseek-v4-pro",
        "google" => "gemini-3.1-pro-preview",
        "google-vertex" => "gemini-3.1-pro-preview",
        "github-copilot" => "gpt-5.4",
        "openrouter" => "moonshotai/kimi-k2.6",
        "vercel-ai-gateway" => "zai/glm-5.1",
        "xai" => "grok-4.20-0309-reasoning",
        "groq" => "openai/gpt-oss-120b",
        "cerebras" => "zai-glm-4.7",
        "zai" => "glm-5.1",
        "zai-coding-cn" => "glm-5.1",
        "mistral" => "devstral-medium-latest",
        "minimax" => "MiniMax-M2.7",
        "minimax-cn" => "MiniMax-M2.7",
        "moonshotai" => "kimi-k2.6",
        "moonshotai-cn" => "kimi-k2.6",
        "huggingface" => "moonshotai/Kimi-K2.6",
        "fireworks" => "accounts/fireworks/models/kimi-k2p6",
        "together" => "moonshotai/Kimi-K2.6",
        "opencode" => "kimi-k2.6",
        "opencode-go" => "kimi-k2.6",
        "kimi-coding" => "kimi-for-coding",
        "cloudflare-workers-ai" => "@cf/moonshotai/kimi-k2.6",
        "cloudflare-ai-gateway" => "workers-ai/@cf/moonshotai/kimi-k2.6",
        "xiaomi" => "mimo-v2.5-pro",
        "xiaomi-token-plan-cn" => "mimo-v2.5-pro",
        "xiaomi-token-plan-ams" => "mimo-v2.5-pro",
        "xiaomi-token-plan-sgp" => "mimo-v2.5-pro",
        _ => return None,
    };
    Some(id)
}

/// The ordered list of known providers, used to scan for a curated default (Pi iterates
/// `Object.keys(defaultModelPerProvider)`, model-resolver.ts:593/655).
const KNOWN_PROVIDERS: &[&str] = &[
    "amazon-bedrock",
    "ant-ling",
    "anthropic",
    "openai",
    "azure-openai-responses",
    "openai-codex",
    "nvidia",
    "deepseek",
    "google",
    "google-vertex",
    "github-copilot",
    "openrouter",
    "vercel-ai-gateway",
    "xai",
    "groq",
    "cerebras",
    "zai",
    "zai-coding-cn",
    "mistral",
    "minimax",
    "minimax-cn",
    "moonshotai",
    "moonshotai-cn",
    "huggingface",
    "fireworks",
    "together",
    "opencode",
    "opencode-go",
    "kimi-coding",
    "cloudflare-workers-ai",
    "cloudflare-ai-gateway",
    "xiaomi",
    "xiaomi-token-plan-cn",
    "xiaomi-token-plan-ams",
    "xiaomi-token-plan-sgp",
];

/// Find the first available model whose (provider, id) matches a curated default, else the first
/// available model (Pi's loop at model-resolver.ts:593-602 / 655-667).
fn first_default_or_first(available: &[Model]) -> Option<Model> {
    for provider in KNOWN_PROVIDERS {
        if let Some(default_id) = default_model_per_provider(provider)
            && let Some(m) = available
                .iter()
                .find(|m| m.provider.as_str() == *provider && m.id.as_str() == default_id)
        {
            return Some(m.clone());
        }
    }
    available.first().cloned()
}

/// Synthesize a custom model for `(provider, model_id)` by cloning the provider's curated-default
/// (or first) model and overriding id/name (Pi `buildFallbackModel`, model-resolver.ts:163-177).
pub fn build_fallback_model(provider: &str, model_id: &str, available: &[Model]) -> Option<Model> {
    let provider_models: Vec<&Model> = available
        .iter()
        .filter(|m| m.provider.as_str() == provider)
        .collect();
    let base = provider_models.first().copied()?;
    let default_id = default_model_per_provider(provider);
    let base = match default_id {
        Some(did) => provider_models
            .iter()
            .find(|m| m.id.as_str() == did)
            .copied()
            .unwrap_or(base),
        None => base,
    };
    let mut model = base.clone();
    model.id = model_id.into();
    model.name = model_id.to_string();
    Some(model)
}

/// Result of [`resolve_cli_model`] (Pi `ResolveCliModelResult`, model-resolver.ts:318-327).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CliModelResult {
    pub model: Option<Model>,
    pub thinking_level: Option<ModelThinkingLevel>,
    pub warning: Option<String>,
    /// CLI-display error; when set, `model` is `None`.
    pub error: Option<String>,
}

/// Resolve a single model from CLI flags (Pi `resolveCliModel`, model-resolver.ts:340-511).
///
/// `all` is the full model set (NOT just authed models, so `--api-key` first-time setup works).
/// `has_configured_auth` reports whether a model has usable auth (Pi `modelRegistry.hasConfiguredAuth`).
pub fn resolve_cli_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    cli_thinking: Option<ModelThinkingLevel>,
    all: &[Model],
    has_configured_auth: &dyn Fn(&Model) -> bool,
) -> CliModelResult {
    let Some(cli_model) = cli_model else {
        return CliModelResult::default();
    };

    if all.is_empty() {
        return CliModelResult {
            error: Some(
                "No models available. Check your installation or add models to models.json."
                    .to_string(),
            ),
            ..Default::default()
        };
    }

    // Canonical (case-insensitive) provider lookup.
    let canonical_provider = |name: &str| -> Option<String> {
        all.iter()
            .find(|m| m.provider.as_str().eq_ignore_ascii_case(name))
            .map(|m| m.provider.as_str().to_string())
    };

    let mut provider = cli_provider.and_then(canonical_provider);
    if cli_provider.is_some() && provider.is_none() {
        return CliModelResult {
            error: Some(format!(
                "Unknown provider \"{}\". Use --list-models to see available providers/models.",
                cli_provider.unwrap_or("")
            )),
            ..Default::default()
        };
    }

    let mut pattern = cli_model.to_string();
    let mut inferred_provider = false;

    // Infer `provider/model` when the prefix matches a known provider.
    if provider.is_none()
        && let Some(slash) = cli_model.find('/')
    {
        let maybe = &cli_model[..slash];
        if let Some(canonical) = canonical_provider(maybe) {
            provider = Some(canonical);
            pattern = cli_model[slash + 1..].to_string();
            inferred_provider = true;
        }
    }

    // No provider inferred: try exact id / provider/id match across all models.
    if provider.is_none() {
        let lower = cli_model.to_ascii_lowercase();
        if let Some(exact) = all.iter().find(|m| {
            m.id.as_str().to_ascii_lowercase() == lower
                || format!("{}/{}", m.provider, m.id).to_ascii_lowercase() == lower
        }) {
            return CliModelResult {
                model: Some(exact.clone()),
                ..Default::default()
            };
        }
    }

    // Both --provider and --model <provider>/<pattern>: strip the redundant prefix.
    if let (Some(cp), Some(p)) = (cli_provider, provider.as_deref()) {
        let _ = cp;
        let prefix = format!("{p}/");
        if cli_model
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
        {
            pattern = cli_model[prefix.len()..].to_string();
        }
    }

    let candidates: Vec<Model> = match provider.as_deref() {
        Some(p) => all
            .iter()
            .filter(|m| m.provider.as_str() == p)
            .cloned()
            .collect(),
        None => all.to_vec(),
    };
    let resolver = ModelResolver::new(&candidates);
    let parsed = resolver.parse_pattern(&pattern, true);

    if let Some(model) = parsed.model.clone() {
        // Provider inference matched an unauthenticated pair: prefer an authed raw id match.
        if inferred_provider {
            let raw_exact: Vec<&Model> = all
                .iter()
                .filter(|m| {
                    m.id.as_str().eq_ignore_ascii_case(cli_model) && !models_are_equal(m, &model)
                })
                .collect();
            if !raw_exact.is_empty() && !has_configured_auth(&model) {
                let authed: Vec<&Model> = raw_exact
                    .into_iter()
                    .filter(|m| has_configured_auth(m))
                    .collect();
                if authed.len() == 1
                    && let Some(m) = authed.first()
                {
                    return CliModelResult {
                        model: Some((*m).clone()),
                        ..Default::default()
                    };
                }
            }
        }
        return CliModelResult {
            model: Some(model),
            thinking_level: parsed.thinking_level,
            warning: parsed.warning,
            error: None,
        };
    }

    // Inferred a provider but no match within it: fall back to a raw id match across all models.
    if inferred_provider {
        let lower = cli_model.to_ascii_lowercase();
        if let Some(exact) = all.iter().find(|m| {
            m.id.as_str().to_ascii_lowercase() == lower
                || format!("{}/{}", m.provider, m.id).to_ascii_lowercase() == lower
        }) {
            return CliModelResult {
                model: Some(exact.clone()),
                ..Default::default()
            };
        }
        let fallback = ModelResolver::new(all).parse_pattern(cli_model, true);
        if let Some(m) = fallback.model {
            return CliModelResult {
                model: Some(m),
                thinking_level: fallback.thinking_level,
                warning: fallback.warning,
                error: None,
            };
        }
    }

    if let Some(p) = provider.as_deref() {
        // Parse a `:level` suffix from the pattern before building the fallback model.
        let mut fallback_pattern = pattern.clone();
        let mut fallback_thinking: Option<ModelThinkingLevel> = None;
        if cli_thinking.is_none()
            && let Some(idx) = pattern.rfind(':')
        {
            let suffix = pattern.get(idx + 1..).unwrap_or("");
            if let Some(lvl) = parse_thinking_level(suffix) {
                fallback_pattern = pattern.get(..idx).unwrap_or(&pattern).to_string();
                fallback_thinking = Some(lvl);
            }
        }
        if let Some(mut fallback_model) = build_fallback_model(p, &fallback_pattern, all) {
            let requested = cli_thinking.or(fallback_thinking);
            if matches!(requested, Some(l) if l.is_on()) {
                fallback_model.reasoning = true;
            }
            let base_warn = format!(
                "Model \"{fallback_pattern}\" not found for provider \"{p}\". Using custom model id."
            );
            let warning = match parsed.warning {
                Some(w) => format!("{w} {base_warn}"),
                None => base_warn,
            };
            return CliModelResult {
                model: Some(fallback_model),
                thinking_level: fallback_thinking,
                warning: Some(warning),
                error: None,
            };
        }
    }

    let display = match provider.as_deref() {
        Some(p) => format!("{p}/{pattern}"),
        None => cli_model.to_string(),
    };
    CliModelResult {
        model: None,
        thinking_level: None,
        warning: parsed.warning,
        error: Some(format!(
            "Model \"{display}\" not found. Use --list-models to see available models."
        )),
    }
}

/// Result of [`find_initial_model`] (Pi `InitialModelResult`, model-resolver.ts:513-517).
#[derive(Clone, Debug, PartialEq)]
pub struct InitialModelResult {
    pub model: Option<Model>,
    pub thinking_level: ModelThinkingLevel,
    pub fallback_message: Option<String>,
    /// A CLI error surfaced from step 1 (`resolve_cli_model`). Pi calls `process.exit(1)`; the bin
    /// owns process exit, so we propagate the message instead (additive, non-panicking).
    pub error: Option<String>,
}

/// Find the initial model by priority (Pi `findInitialModel`, model-resolver.ts:527-607):
/// 1) CLI provider+model, 2) first scoped (unless continuing), 3) saved settings default,
/// 4) first available model matching a curated default (else first available), 5) none.
#[allow(clippy::too_many_arguments)]
pub fn find_initial_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    scoped_models: &[ScopedModel],
    is_continuing: bool,
    default_provider: Option<&str>,
    default_model_id: Option<&str>,
    default_thinking_level: Option<ModelThinkingLevel>,
    all: &[Model],
    available: &[Model],
    has_configured_auth: &dyn Fn(&Model) -> bool,
) -> InitialModelResult {
    let default_level = ModelThinkingLevel::default();

    // 1. CLI args take priority.
    if let (Some(_), Some(_)) = (cli_provider, cli_model) {
        let resolved = resolve_cli_model(cli_provider, cli_model, None, all, has_configured_auth);
        if let Some(err) = resolved.error {
            return InitialModelResult {
                model: None,
                thinking_level: default_level,
                fallback_message: None,
                error: Some(err),
            };
        }
        if let Some(model) = resolved.model {
            return InitialModelResult {
                model: Some(model),
                thinking_level: default_level,
                fallback_message: None,
                error: None,
            };
        }
    }

    // 2. First scoped model (unless continuing/resuming a session).
    if let Some(first) = scoped_models.first()
        && !is_continuing
    {
        return InitialModelResult {
            model: Some(first.model.clone()),
            thinking_level: first
                .thinking_level
                .or(default_thinking_level)
                .unwrap_or(default_level),
            fallback_message: None,
            error: None,
        };
    }

    // 3. Saved default from settings.
    if let (Some(dp), Some(dm)) = (default_provider, default_model_id)
        && let Some(found) = all
            .iter()
            .find(|m| m.provider.as_str() == dp && m.id.as_str() == dm)
    {
        return InitialModelResult {
            model: Some(found.clone()),
            thinking_level: default_thinking_level.unwrap_or(default_level),
            fallback_message: None,
            error: None,
        };
    }

    // 4. First available model with valid auth (curated-default first).
    if let Some(model) = first_default_or_first(available) {
        return InitialModelResult {
            model: Some(model),
            thinking_level: default_level,
            fallback_message: None,
            error: None,
        };
    }

    // 5. No model.
    InitialModelResult {
        model: None,
        thinking_level: default_level,
        fallback_message: None,
        error: None,
    }
}

/// Result of [`restore_model_from_session`] (Pi `restoreModelFromSession` return,
/// model-resolver.ts:612-681).
#[derive(Clone, Debug, PartialEq)]
pub struct RestoredModelResult {
    pub model: Option<Model>,
    pub fallback_message: Option<String>,
}

/// Restore a model saved in a session, re-checking auth and falling back (Pi
/// `restoreModelFromSession`, model-resolver.ts:612-681). The console messaging is a front-end
/// concern; only the model + fallback message are returned.
pub fn restore_model_from_session(
    saved_provider: &str,
    saved_model_id: &str,
    current_model: Option<&Model>,
    all: &[Model],
    available: &[Model],
    has_configured_auth: &dyn Fn(&Model) -> bool,
) -> RestoredModelResult {
    let restored = all
        .iter()
        .find(|m| m.provider.as_str() == saved_provider && m.id.as_str() == saved_model_id);
    let restored_has_auth = restored.is_some_and(has_configured_auth);

    if let Some(model) = restored
        && restored_has_auth
    {
        return RestoredModelResult {
            model: Some(model.clone()),
            fallback_message: None,
        };
    }

    let reason = if restored.is_none() {
        "model no longer exists"
    } else {
        "no auth configured"
    };

    if let Some(current) = current_model {
        return RestoredModelResult {
            model: Some(current.clone()),
            fallback_message: Some(format!(
                "Could not restore model {saved_provider}/{saved_model_id} ({reason}). Using {}/{}.",
                current.provider, current.id
            )),
        };
    }

    if let Some(fallback) = first_default_or_first(available) {
        let msg = format!(
            "Could not restore model {saved_provider}/{saved_model_id} ({reason}). Using {}/{}.",
            fallback.provider, fallback.id
        );
        return RestoredModelResult {
            model: Some(fallback),
            fallback_message: Some(msg),
        };
    }

    RestoredModelResult {
        model: None,
        fallback_message: None,
    }
}

/// A `models.json` provider request config (Pi `ProviderConfigSchema`, model-registry.ts:204-214):
/// the request-auth-relevant fields. `apiKey`/`headers` carry unresolved config-value templates;
/// resolve them with [`ProviderConfig::resolve_request_auth`].
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub auth_header: Option<bool>,
    /// Inline model definitions (preserved verbatim; full parsing lives in the model registry).
    #[serde(default)]
    pub models: Vec<serde_json::Value>,
}

/// Resolved request auth for a provider (Pi `ResolvedRequestAuth` ok-branch,
/// model-registry.ts:249-259).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedRequestAuth {
    pub api_key: Option<String>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub auth_header: Option<bool>,
}

impl ProviderConfig {
    /// Resolve `apiKey` + `headers` through the config-value language (Pi
    /// `getApiKeyAndHeaders`/`resolveHeadersOrThrow`, model-registry.ts:659-736). `env` is an
    /// optional provider-scoped override map. Returns an error string on an unresolvable template.
    pub fn resolve_request_auth(
        &self,
        env: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<ResolvedRequestAuth, String> {
        let api_key = match &self.api_key {
            Some(raw) => Some(crate::config_value::resolve_config_value_or_throw(
                raw, "API key", env,
            )?),
            None => None,
        };
        let headers = match &self.headers {
            Some(map) => {
                let owned: std::collections::HashMap<String, String> =
                    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                crate::config_value::resolve_headers_or_throw(Some(&owned), "provider", env)?
            }
            None => None,
        };
        Ok(ResolvedRequestAuth {
            api_key,
            headers,
            auth_header: self.auth_header,
        })
    }
}

/// A parsed `models.json` in Pi's `{ providers: { <name>: ProviderConfig } }` shape
/// (model-registry.ts:216-218).
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct ModelFile {
    #[serde(default)]
    pub providers: std::collections::BTreeMap<String, ProviderConfig>,
}

/// Load a `models.json` provider-config file (Pi's `{ providers: {...} }` shape). A missing or
/// empty file yields an empty [`ModelFile`]. This is additive alongside [`load_custom_models`]
/// (which reads the legacy flat `Vec<Model>` shape) so existing consumers are unaffected.
pub fn load_models_file(path: &Path) -> Result<ModelFile, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ModelFile::default()),
        Err(e) => return Err(ConfigError::Io(e)),
    };
    if text.trim().is_empty() {
        return Ok(ModelFile::default());
    }
    let file: ModelFile = serde_json::from_str(&text)?;
    Ok(file)
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
            base_url: String::new(),
            reasoning: true,
            input: vec![Modality::Text],
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
        assert_eq!(
            parsed.model.as_ref().unwrap().id.as_str(),
            "claude-opus-4-latest"
        );
        assert_eq!(parsed.thinking_level, Some(ModelThinkingLevel::High));
    }

    #[test]
    fn ambiguous_bare_id_errors() {
        // A-07-6
        let models = vec![
            model("a", "shared", "A Shared"),
            model("b", "shared", "B Shared"),
        ];
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
        assert_eq!(
            parsed.model.as_ref().unwrap().id.as_str(),
            "claude-3-5-sonnet-latest"
        );
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
        assert_eq!(
            parsed.model.as_ref().unwrap().id.as_str(),
            "gpt-4o:extended"
        );
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
    fn default_model_table_matches_pi() {
        // model-resolver.ts:14-50
        assert_eq!(
            default_model_per_provider("anthropic"),
            Some("claude-opus-4-8")
        );
        assert_eq!(default_model_per_provider("openai"), Some("gpt-5.5"));
        assert_eq!(
            default_model_per_provider("amazon-bedrock"),
            Some("us.anthropic.claude-opus-4-6-v1")
        );
        assert_eq!(default_model_per_provider("totally-unknown"), None);
    }

    #[test]
    fn resolve_cli_model_provider_and_pattern() {
        let models = vec![
            model("anthropic", "claude-opus-4-8", "Opus"),
            model("openai", "gpt-5.5", "GPT"),
        ];
        let auth = |_: &Model| true;
        // --provider anthropic --model opus → fuzzy match.
        let r = resolve_cli_model(Some("anthropic"), Some("opus"), None, &models, &auth);
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "claude-opus-4-8");
        assert!(r.error.is_none());
        // provider/model inference.
        let r = resolve_cli_model(None, Some("openai/gpt-5.5"), None, &models, &auth);
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "gpt-5.5");
        // unknown provider → error.
        let r = resolve_cli_model(Some("nope"), Some("x"), None, &models, &auth);
        assert!(r.error.as_ref().unwrap().contains("Unknown provider"));
    }

    #[test]
    fn resolve_cli_model_builds_fallback_custom_id() {
        // A custom model id under a known provider builds a fallback from the provider default.
        let models = vec![model("anthropic", "claude-opus-4-8", "Opus")];
        let auth = |_: &Model| true;
        let r = resolve_cli_model(
            Some("anthropic"),
            Some("my-custom-id"),
            None,
            &models,
            &auth,
        );
        let m = r.model.as_ref().unwrap();
        assert_eq!(m.id.as_str(), "my-custom-id");
        assert_eq!(m.provider.as_str(), "anthropic");
        assert!(
            r.warning
                .as_ref()
                .unwrap()
                .contains("Using custom model id")
        );
    }

    #[test]
    fn find_initial_model_priority() {
        let all = vec![
            model("anthropic", "claude-opus-4-8", "Opus"),
            model("openai", "gpt-5.5", "GPT"),
        ];
        let available = all.clone();
        let auth = |_: &Model| true;
        // CLI args win.
        let r = find_initial_model(
            Some("openai"),
            Some("gpt-5.5"),
            &[],
            false,
            None,
            None,
            None,
            &all,
            &available,
            &auth,
        );
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "gpt-5.5");
        // No CLI, no scoped, no saved → curated default (anthropic first in table → opus).
        let r = find_initial_model(
            None,
            None,
            &[],
            false,
            None,
            None,
            None,
            &all,
            &available,
            &auth,
        );
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "claude-opus-4-8");
        // Saved settings default beats curated default.
        let r = find_initial_model(
            None,
            None,
            &[],
            false,
            Some("openai"),
            Some("gpt-5.5"),
            None,
            &all,
            &available,
            &auth,
        );
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "gpt-5.5");
    }

    #[test]
    fn restore_model_falls_back_when_no_auth() {
        let all = vec![model("anthropic", "claude-opus-4-8", "Opus")];
        let available = all.clone();
        // saved model has no auth → fall back to curated default with a message.
        let no_auth = |_: &Model| false;
        let r = restore_model_from_session(
            "anthropic",
            "claude-opus-4-8",
            None,
            &all,
            &available,
            &no_auth,
        );
        assert!(
            r.fallback_message
                .as_ref()
                .unwrap()
                .contains("no auth configured")
        );
        assert_eq!(r.model.as_ref().unwrap().id.as_str(), "claude-opus-4-8");
        // saved model with auth → restored, no message.
        let yes_auth = |_: &Model| true;
        let r = restore_model_from_session(
            "anthropic",
            "claude-opus-4-8",
            None,
            &all,
            &available,
            &yes_auth,
        );
        assert!(r.fallback_message.is_none());
    }

    #[test]
    fn glob_question_and_class_and_scope_level() {
        let models = vec![
            model("anthropic", "claude-opus-4-8", "Opus"),
            model("anthropic", "claude-haiku-4", "Haiku"),
            model("openai", "gpt-5.5", "GPT"),
        ];
        let r = ModelResolver::new(&models);
        // `?` matches one char; `[...]` class.
        let scoped = r.resolve_scope(&["anthropic/claude-opus-4-?".to_string()]);
        assert_eq!(scoped.len(), 1);
        let scoped = r.resolve_scope(&["anthropic/claude-[ho]*".to_string()]);
        assert_eq!(scoped.len(), 2);
        // `:level` suffix on a glob applies to every match.
        let scoped = r.resolve_scope(&["anthropic/*:high".to_string()]);
        assert!(
            scoped
                .iter()
                .all(|s| s.thinking_level == Some(ModelThinkingLevel::High))
        );
    }

    #[test]
    fn models_file_provider_config_resolves_auth() {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("models.json");
        std::fs::write(
            &path,
            r#"{ "providers": { "acme": { "baseUrl": "https://api.acme.test", "apiKey": "literal-key", "authHeader": true } } }"#,
        )
        .unwrap();
        let file = load_models_file(&path).unwrap();
        let cfg = file.providers.get("acme").unwrap();
        assert_eq!(cfg.base_url.as_deref(), Some("https://api.acme.test"));
        let resolved = cfg.resolve_request_auth(None).unwrap();
        assert_eq!(resolved.api_key.as_deref(), Some("literal-key"));
        assert_eq!(resolved.auth_header, Some(true));
        // missing file → empty
        assert!(
            load_models_file(&dir.join("nope.json"))
                .unwrap()
                .providers
                .is_empty()
        );
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
        assert!(
            load_custom_models(&dir.join("nope.json"))
                .unwrap()
                .is_empty()
        );
    }
}
