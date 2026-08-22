//! Pattern matching and `--models` scope expansion: the `:level` thinking shorthand, exact /
//! partial reference matching with the UCA tie-break, per-provider defaults, and the scope
//! diagnostics `resolve_scope_reporting` emits (R-07-019…R-07-022).

use cyrup_core::{ModelThinkingLevel, ProviderId};
use cyrup_provider::Model;

use super::glob::glob_match;

/// Parse a thinking-level token (`off|minimal|low|medium|high|xhigh|max` — Pi
/// `VALID_THINKING_LEVELS`, args.ts:59).
pub fn parse_thinking_level(s: &str) -> Option<ModelThinkingLevel> {
    match s.trim().to_ascii_lowercase().as_str() {
        "off" => Some(ModelThinkingLevel::Off),
        "minimal" => Some(ModelThinkingLevel::Minimal),
        "low" => Some(ModelThinkingLevel::Low),
        "medium" => Some(ModelThinkingLevel::Medium),
        "high" => Some(ModelThinkingLevel::High),
        "xhigh" => Some(ModelThinkingLevel::Xhigh),
        "max" => Some(ModelThinkingLevel::Max),
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
/// Mirrors Pi's `ParsedModelResult` (`{ model, thinkingLevel, warning }`, model-resolver.ts:156-161):
/// Pi has no "ambiguous" concept — an ambiguous bare id resolves via partial matching, never errors.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedModel {
    pub model: Option<Model>,
    pub thinking_level: Option<ModelThinkingLevel>,
    pub warning: Option<String>,
}

/// Result of resolving a reference (Pi `tryMatchModel` return: `Model | undefined`).
/// Pi never errors on ambiguity: `findExactModelReferenceMatch` returns `undefined` for a bare id
/// present on >1 provider, so resolution falls through to partial matching
/// (model-resolver.ts:90-118, 124-154).
enum Match<'a> {
    None,
    One(&'a Model),
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

    /// Exact model-reference match, with NO partial fallback (Pi
    /// `findExactModelReferenceMatch`, model-resolver.ts:79-120 @v0.83.0). Accepts either a
    /// canonical `provider/modelId` reference or a bare model id; a bare id carried by more than
    /// one provider is ambiguous and yields `None` (`:118`).
    ///
    /// Split out of [`ModelResolver::match_reference`] because Pi calls it in TWO places the
    /// partial matcher must not run: `tryMatchModel`'s first step (`:128`) and — the case CFG-018
    /// records — INSIDE the glob branch of `resolveModelScope` (`:297`), before the minimatch
    /// filter.
    fn exact_reference_match(&self, reference: &str) -> Option<&'a Model> {
        let reference = reference.trim();
        if reference.is_empty() {
            return None;
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
                return Some(m);
            }
        }

        // 2. bare exact id. Pi's `findExactModelReferenceMatch` returns the model ONLY when exactly
        // one id matches; a bare id present on >1 provider returns `undefined` (it does NOT error),
        // so it falls through to partial matching in `tryMatchModel` (model-resolver.ts:116-118).
        // Likewise a zero-hit exact match falls through.
        let exact: Vec<&Model> = self
            .available
            .iter()
            .filter(|m| m.id.as_str().to_ascii_lowercase() == lower)
            .collect();
        if exact.len() == 1 {
            return exact.first().copied();
        }
        None
    }

    fn match_reference(&self, reference: &str) -> Match<'a> {
        let reference = reference.trim();
        if reference.is_empty() {
            return Match::None;
        }
        let lower = reference.to_ascii_lowercase();

        // 1-2. exact reference (canonical or bare id) — Pi `tryMatchModel`'s first step, :128.
        if let Some(m) = self.exact_reference_match(reference) {
            return Match::One(m);
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
        // alias first, then highest-sorting id (descending). Pi tie-breaks with
        // `b.id.localeCompare(a.id)` (model-resolver.ts:147,151) — locale-aware UCA collation, NOT a
        // Unicode-scalar `String::cmp`. The two diverge when matched alias ids differ only by case or
        // by `-`/`_`/`.` (e.g. byte-order puts `B` < `a` and `-`(0x2d) < `.`(0x2e) < `_`(0x5f), while
        // localeCompare/UCA orders case as a tertiary weight and weights punctuation differently).
        // Reuse the same `feruca` (pure-Rust UCA) collator config proven to match Node's default
        // `localeCompare` for `cyrup-tools` `ls` (ls.rs:85-87): CLDR-root tailoring, non-ignorable
        // ("not shifted") variable handling, byte-value final tiebreak. `collate(b, a)` reproduces
        // the descending `b.localeCompare(a)`.
        let mut collator = feruca::Collator::new(feruca::Tailoring::default(), false, true);
        partial.sort_by(|a, b| {
            let aa = is_alias(a.id.as_str());
            let ba = is_alias(b.id.as_str());
            ba.cmp(&aa)
                .then_with(|| collator.collate(b.id.as_str(), a.id.as_str()))
        });
        match partial.first() {
            Some(m) => Match::One(m),
            None => Match::None,
        }
    }

    /// Exact-then-partial lookup (Pi `tryMatchModel`, model-resolver.ts:124-154). `None` = no match.
    /// Pi never errors on an ambiguous bare id; it falls through to partial matching, which always
    /// resolves to a single (alias-preferred, highest-sorting) model.
    pub fn find_exact(&self, reference: &str) -> Option<&'a Model> {
        match self.match_reference(reference) {
            Match::One(m) => Some(m),
            Match::None => None,
        }
    }

    /// Parse a `pattern[:level]` (R-07-020). `strict` (CLI `--model`) refuses to guess on an
    /// invalid trailing token; non-strict (scope mode) warns and recurses on the prefix.
    pub fn parse_pattern(&self, pattern: &str, strict: bool) -> ParsedModel {
        // Try a full exact/partial match first (Pi `tryMatchModel`, model-resolver.ts:198-201).
        if let Match::One(m) = self.match_reference(pattern) {
            return ParsedModel {
                model: Some(m.clone()),
                thinking_level: None,
                warning: None,
            };
        }

        // No match — split on the LAST colon if present (Pi model-resolver.ts:203-211).
        let Some(idx) = pattern.rfind(':') else {
            return ParsedModel {
                model: None,
                thinking_level: None,
                warning: None,
            };
        };
        let (prefix, rest) = pattern.split_at(idx);
        let suffix = rest.get(1..).unwrap_or("");

        if let Some(level) = parse_thinking_level(suffix) {
            // Valid level — recurse on the prefix. Keep the level only when the inner parse is
            // clean (Pi `thinkingLevel: result.warning ? undefined : suffix`; :213-224). When the
            // prefix itself does not resolve, return the inner result verbatim (:224).
            let inner = self.parse_pattern(prefix, strict);
            if inner.model.is_some() {
                let thinking = if inner.warning.is_some() {
                    None
                } else {
                    Some(level)
                };
                ParsedModel {
                    model: inner.model,
                    thinking_level: thinking,
                    warning: inner.warning,
                }
            } else {
                inner
            }
        } else if strict {
            // Strict (CLI `--model`): don't guess — treat the suffix as part of the id and fail
            // (Pi :228-232).
            ParsedModel {
                model: None,
                thinking_level: None,
                warning: None,
            }
        } else {
            // Scope mode: recurse on the prefix and warn (Pi :234-244).
            let inner = self.parse_pattern(prefix, strict);
            if inner.model.is_some() {
                ParsedModel {
                    model: inner.model,
                    thinking_level: None,
                    // Pi's exact sentence (model-resolver.ts:243 @v0.83.0):
                    // `Invalid thinking level "X" in pattern "Y". Using default instead.` — `Y` is
                    // the pattern at THIS recursion level, which is what upstream interpolates.
                    warning: Some(format!(
                        "Invalid thinking level \"{suffix}\" in pattern \"{pattern}\". Using default instead."
                    )),
                }
            } else {
                inner
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
    ///
    /// Diagnostic-free convenience wrapper over [`ModelResolver::resolve_scope_reporting`]; Pi's
    /// `resolveModelScope` always returns `{ scopedModels, diagnostics }` (model-resolver.ts:270
    /// @v0.83.0), so a caller that wants the warnings must use the reporting form.
    pub fn resolve_scope(&self, patterns: &[String]) -> Vec<ScopedModel> {
        self.resolve_scope_reporting(patterns).models
    }

    /// Expand scope patterns AND report Pi's `ModelScopeDiagnostic`s (`model-resolver.ts:261-270`
    /// @v0.83.0): `no-match` for a pattern that resolves to nothing (pushed at `:316` on the glob
    /// path and `:340` on the reference path) and `invalid-thinking-level` for a bad `:level`
    /// suffix (minted at `:243`, pushed at `:334`).
    pub fn resolve_scope_reporting(&self, patterns: &[String]) -> ModelScopeResult {
        let mut diagnostics: Vec<ModelScopeDiagnostic> = Vec::new();
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
                // Pi tries an EXACT reference match before the minimatch filter (`:297-303`), so a
                // pattern that happens to carry a glob metacharacter (`[`, `?`) but names a real
                // model resolves directly. CFG-018.
                if let Some(exact) = self.exact_reference_match(glob_pattern) {
                    push(exact.clone(), level, &mut seen, &mut out);
                    continue;
                }
                let matching: Vec<&Model> = self
                    .available
                    .iter()
                    .filter(|m| {
                        glob_match(glob_pattern, &format!("{}/{}", m.provider, m.id))
                            || glob_match(glob_pattern, m.id.as_str())
                    })
                    .collect();
                if matching.is_empty() {
                    diagnostics.push(ModelScopeDiagnostic {
                        level: ModelScopeDiagnosticLevel::Warning,
                        code: ModelScopeDiagnosticCode::NoMatch,
                        message: format!("No models match pattern \"{pattern}\""),
                        pattern: pattern.clone(),
                    });
                    continue;
                }
                for m in matching {
                    push(m.clone(), level, &mut seen, &mut out);
                }
            } else {
                let parsed = self.parse_pattern(pattern, false);
                if let Some(warning) = parsed.warning {
                    diagnostics.push(ModelScopeDiagnostic {
                        level: ModelScopeDiagnosticLevel::Warning,
                        code: ModelScopeDiagnosticCode::InvalidThinkingLevel,
                        message: warning,
                        pattern: pattern.clone(),
                    });
                }
                match parsed.model {
                    Some(m) => push(m, parsed.thinking_level, &mut seen, &mut out),
                    None => diagnostics.push(ModelScopeDiagnostic {
                        level: ModelScopeDiagnosticLevel::Warning,
                        code: ModelScopeDiagnosticCode::NoMatch,
                        message: format!("No models match pattern \"{pattern}\""),
                        pattern: pattern.clone(),
                    }),
                }
            }
        }
        ModelScopeResult {
            models: out,
            diagnostics,
        }
    }
}

/// Severity of a [`ModelScopeDiagnostic`] (Pi's `type: "warning"`, model-resolver.ts:262 @v0.83.0 —
/// upstream mints only warnings today, so the enum has one arm).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelScopeDiagnosticLevel {
    Warning,
}

/// Machine-readable diagnostic code (Pi `code: "no-match" | "invalid-thinking-level"`,
/// model-resolver.ts:263 @v0.83.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelScopeDiagnosticCode {
    NoMatch,
    InvalidThinkingLevel,
}

/// One warning emitted while expanding `--models` scope patterns (Pi `ModelScopeDiagnostic`,
/// model-resolver.ts:261-268 @v0.83.0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelScopeDiagnostic {
    pub level: ModelScopeDiagnosticLevel,
    pub code: ModelScopeDiagnosticCode,
    pub message: String,
    /// The originating pattern, verbatim (Pi carries `pattern` on every diagnostic, `:267`).
    pub pattern: String,
}

/// Result of [`ModelResolver::resolve_scope_reporting`] (Pi's `{ scopedModels, diagnostics }`,
/// model-resolver.ts:270 @v0.83.0).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelScopeResult {
    pub models: Vec<ScopedModel>,
    pub diagnostics: Vec<ModelScopeDiagnostic>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::model::fixtures::model;

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

    /// PROV-002: `max` is a valid level token (Pi `VALID_THINKING_LEVELS`, args.ts:59), so a
    /// `model:max` shorthand must SPLIT. Before the fix `parse_thinking_level("max")` returned
    /// `None`, so `:max` was swallowed into the model id and the pattern failed to resolve.
    #[test]
    fn max_thinking_shorthand_parses() {
        assert_eq!(parse_thinking_level("max"), Some(ModelThinkingLevel::Max));
        assert_eq!(parse_thinking_level("MAX"), Some(ModelThinkingLevel::Max));
        assert_eq!(parse_thinking_level("bogus"), None);

        let models = vec![model("anthropic", "claude-opus-4-6", "Claude Opus 4.6")];
        let r = ModelResolver::new(&models);
        let parsed = r.parse_pattern("claude-opus-4-6:max", true);
        assert_eq!(
            parsed.model.as_ref().expect("model resolves").id.as_str(),
            "claude-opus-4-6"
        );
        assert_eq!(parsed.thinking_level, Some(ModelThinkingLevel::Max));

        // …and on the glob/scope path too.
        let scoped = r.resolve_scope(&["anthropic/*:max".to_string()]);
        assert!(!scoped.is_empty());
        assert!(
            scoped
                .iter()
                .all(|s| s.thinking_level == Some(ModelThinkingLevel::Max))
        );
    }

    #[test]
    fn ambiguous_bare_id_resolves_via_partial_like_pi() {
        // Pi never errors on an ambiguous bare id: `findExactModelReferenceMatch` returns
        // `undefined` for an id on >1 provider (model-resolver.ts:116-118), so `tryMatchModel`
        // falls through to partial matching, which always yields a single model
        // (alias-preferred, then `b.id.localeCompare(a.id)` descending → first in original order
        // on ties). Ground truth derived from Pi: both "shared" ids are aliases and equal, so the
        // first-listed (provider "a") wins.
        let models = vec![
            model("a", "shared", "A Shared"),
            model("b", "shared", "B Shared"),
        ];
        let r = ModelResolver::new(&models);
        // find_exact (Pi `tryMatchModel`) resolves to provider a, never erroring.
        let found = r
            .find_exact("shared")
            .expect("ambiguous bare id resolves, never errors");
        assert_eq!(found.provider.as_str(), "a");
        // parse_pattern likewise resolves (no warning, a concrete model).
        let parsed = r.parse_pattern("shared", true);
        assert_eq!(parsed.model.as_ref().unwrap().provider.as_str(), "a");
        assert!(parsed.warning.is_none());
        // A realistic Pi case: `kimi-k2.6` is shared by moonshotai/moonshotai-cn/opencode/
        // opencode-go. Pi yields exactly 1 cycle entry (moonshotai, first-listed); the old crate
        // yielded 0 by erroring. Assert resolve_scope now returns 1.
        let shared_kimi = vec![
            model("moonshotai", "kimi-k2.6", "Kimi"),
            model("moonshotai-cn", "kimi-k2.6", "Kimi CN"),
            model("opencode", "kimi-k2.6", "Kimi OC"),
            model("opencode-go", "kimi-k2.6", "Kimi OCG"),
        ];
        let r = ModelResolver::new(&shared_kimi);
        let scoped = r.resolve_scope(&["kimi-k2.6".to_string()]);
        assert_eq!(
            scoped.len(),
            1,
            "Pi resolves an ambiguous bare id to 1 model"
        );
        assert_eq!(
            scoped.first().unwrap().model.provider.as_str(),
            "moonshotai"
        );
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
        let m = r.find_exact("openai/gpt-4o").unwrap();
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
    fn partial_tiebreak_collator_matches_pi_localecompare() {
        // Pi tie-breaks ambiguous partial matches with `b.id.localeCompare(a.id)`
        // (model-resolver.ts:147,151). Every (a, b, sign) triple is the EXACT sign Node returned for
        // `b.localeCompare(a)`, captured to `src/testdata/locale_compare.json`. Assert the `feruca`
        // collator we tie-break with (CLDR-root, non-ignorable, byte tiebreak — the same config the
        // sort uses) reproduces that sign for every pair, so the tie-break is byte-1:1 with Pi rather
        // than the old Unicode-scalar `String::cmp` (which diverges on case + `-`/`_`/`.`).
        let fixture = include_str!("../testdata/locale_compare.json");
        let cases: Vec<(String, String, i32)> =
            serde_json::from_str(fixture).expect("valid locale_compare fixture");
        assert!(cases.len() >= 800, "fixture should be comprehensive");
        let mut collator = feruca::Collator::new(feruca::Tailoring::default(), false, true);
        let mut mismatches = Vec::new();
        for (a, b, sign) in &cases {
            let got = match collator.collate(b.as_str(), a.as_str()) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
            // Also prove the OLD scalar cmp would have diverged on the divergent pairs (informational
            // — the assertion is only on the collator matching Pi).
            if got != *sign {
                let scalar = match b.as_str().cmp(a.as_str()) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                };
                mismatches.push(format!(
                    "b={b:?} a={a:?}: Pi localeCompare={sign}, feruca={got} (scalar cmp={scalar})"
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "tie-break collator diverges from Pi localeCompare:\n{}",
            mismatches.join("\n")
        );
    }

    #[test]
    fn scope_glob_is_path_segment_aware_like_pi() {
        // Assembled behaviour: the proven miss is that `anthropic*` matched ALL anthropic models in
        // the old flat matcher but matches NONE in Pi (the `*` cannot cross `/` into the id, and no
        // bare id starts with "anthropic"). Conversely `anthropic/*` matches the two anthropic
        // models. Models with multi-segment ids (cloudflare) require `**` to traverse.
        let models = vec![
            model("anthropic", "claude-opus-4-8", "Opus"),
            model("anthropic", "claude-haiku-4", "Haiku"),
            model("openai", "gpt-5.5", "GPT"),
            model("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6", "Kimi"),
        ];
        let r = ModelResolver::new(&models);
        assert_eq!(
            r.resolve_scope(&["anthropic*".to_string()]).len(),
            0,
            "Pi: `anthropic*` crosses no `/`, matches 0"
        );
        assert_eq!(r.resolve_scope(&["anthropic/*".to_string()]).len(), 2);
        // `{anthropic,openai}/*` brace-expands to two segment patterns → 3 models.
        assert_eq!(
            r.resolve_scope(&["{anthropic,openai}/*".to_string()]).len(),
            3
        );
        // A single `*` segment does NOT match the multi-segment cloudflare id; `**` does.
        assert_eq!(
            r.resolve_scope(&["cloudflare-workers-ai/*".to_string()])
                .len(),
            0
        );
        assert_eq!(
            r.resolve_scope(&["cloudflare-workers-ai/**".to_string()])
                .len(),
            1
        );
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

    /// CFG-018: the glob branch tries `findExactModelReferenceMatch` BEFORE minimatch
    /// (`model-resolver.ts:297-303` @v0.83.0), so an id carrying a glob metacharacter resolves to
    /// itself. CFG-008: the same call now reports pi's `no-match` / `invalid-thinking-level`
    /// diagnostics (`:316`, `:334`, `:243`).
    ///
    /// Red at HEAD: `resolve_scope` went straight to the filter (so `qwen[chat]` matched nothing)
    /// and returned a bare `Vec`, dropping every diagnostic.
    #[test]
    fn glob_scope_short_circuits_on_an_exact_reference_and_reports_diagnostics() {
        let models = vec![
            model("qwen", "qwen[chat]", "Qwen Chat"),
            model("anthropic", "claude-opus-4-8", "Claude Opus"),
        ];
        let r = ModelResolver::new(&models);

        let out = r.resolve_scope_reporting(&["qwen/qwen[chat]".to_string()]);
        assert_eq!(out.models.len(), 1);
        assert_eq!(out.models[0].model.id.as_str(), "qwen[chat]");
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);

        let out = r.resolve_scope_reporting(&["anthorpic/*".to_string()]);
        assert!(out.models.is_empty());
        assert_eq!(out.diagnostics.len(), 1);
        assert_eq!(out.diagnostics[0].code, ModelScopeDiagnosticCode::NoMatch);
        assert_eq!(
            out.diagnostics[0].message,
            "No models match pattern \"anthorpic/*\""
        );

        let out = r.resolve_scope_reporting(&["claude-opus-4-8:bogus".to_string()]);
        assert_eq!(out.models.len(), 1);
        assert_eq!(
            out.diagnostics[0].code,
            ModelScopeDiagnosticCode::InvalidThinkingLevel
        );
        assert_eq!(
            out.diagnostics[0].message,
            "Invalid thinking level \"bogus\" in pattern \"claude-opus-4-8:bogus\". Using default instead."
        );
    }
}
