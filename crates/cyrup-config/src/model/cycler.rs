//! The Ctrl+P / Ctrl+N cursor over a resolved scope set (R-07-022).

use cyrup_core::ModelThinkingLevel;
use cyrup_provider::Model;

use super::resolver::ScopedModel;

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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::model::ModelResolver;
    use crate::model::fixtures::model;

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
}
