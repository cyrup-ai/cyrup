//! The reasoning-effort ladder: the per-request [`ThinkingLevel`] and the model-level
//! [`ModelThinkingLevel`] selection that can also be `off` (func-01 §12).

/// Reasoning effort *level* (Pi `ThinkingLevel`, types.ts:74) — the "on" levels only, with NO
/// `off`. This is the per-request reasoning intensity (Pi `SimpleStreamOptions.reasoning?:
/// ThinkingLevel`); the *absence* of a level (or [`ModelThinkingLevel::Off`]) means reasoning is
/// disabled. Kept distinct from [`ModelThinkingLevel`] so an `off`-bearing selection cannot be
/// confused with an on-level (func-01 §12).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    /// The top rung (Pi added `"max"` in fbdd4638). Declared LAST so the declaration order stays
    /// the ascending ladder `EXTENDED_THINKING_LEVELS` walks when clamping upward.
    Max,
}

/// A model's selectable reasoning level (Pi `ModelThinkingLevel = "off" | ThinkingLevel`,
/// types.ts:75) — the [`ThinkingLevel`] set PLUS `off`. This is the user-facing / session-local
/// selection and the key space of `ThinkingLevelMap`. `Off` is the default (reasoning disabled).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelThinkingLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    /// See [`ThinkingLevel::Max`]. Last, for the same ladder-ordering reason.
    Max,
}

impl ModelThinkingLevel {
    /// The on-level [`ThinkingLevel`], or `None` when `Off` (Pi: `reasoning` is `undefined` for off).
    pub fn level(self) -> Option<ThinkingLevel> {
        match self {
            ModelThinkingLevel::Off => None,
            ModelThinkingLevel::Minimal => Some(ThinkingLevel::Minimal),
            ModelThinkingLevel::Low => Some(ThinkingLevel::Low),
            ModelThinkingLevel::Medium => Some(ThinkingLevel::Medium),
            ModelThinkingLevel::High => Some(ThinkingLevel::High),
            ModelThinkingLevel::Xhigh => Some(ThinkingLevel::Xhigh),
            ModelThinkingLevel::Max => Some(ThinkingLevel::Max),
        }
    }

    /// `true` for any on-level (reasoning enabled).
    pub fn is_on(self) -> bool {
        !matches!(self, ModelThinkingLevel::Off)
    }
}

impl From<ThinkingLevel> for ModelThinkingLevel {
    fn from(l: ThinkingLevel) -> Self {
        match l {
            ThinkingLevel::Minimal => ModelThinkingLevel::Minimal,
            ThinkingLevel::Low => ModelThinkingLevel::Low,
            ThinkingLevel::Medium => ModelThinkingLevel::Medium,
            ThinkingLevel::High => ModelThinkingLevel::High,
            ThinkingLevel::Xhigh => ModelThinkingLevel::Xhigh,
            ThinkingLevel::Max => ModelThinkingLevel::Max,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn model_thinking_level_splits_off_from_levels() {
        assert_eq!(ModelThinkingLevel::default(), ModelThinkingLevel::Off);
        assert_eq!(ModelThinkingLevel::Off.level(), None);
        assert_eq!(ModelThinkingLevel::High.level(), Some(ThinkingLevel::High));
        assert!(ModelThinkingLevel::Minimal.is_on());
        assert_eq!(
            ModelThinkingLevel::from(ThinkingLevel::Low),
            ModelThinkingLevel::Low
        );
    }

    /// PROV-002: the `max` rung Pi added in fbdd4638 (`types.ts:79`). It must be an ON level and
    /// must serialize to the bare `"max"` key the `thinkingLevelMap`, settings and session
    /// persistence all use.
    #[test]
    fn max_is_a_first_class_on_level() {
        assert_eq!(ModelThinkingLevel::Max.level(), Some(ThinkingLevel::Max));
        assert!(ModelThinkingLevel::Max.is_on());
        assert_eq!(
            ModelThinkingLevel::from(ThinkingLevel::Max),
            ModelThinkingLevel::Max
        );
        assert_eq!(
            serde_json::to_value(ModelThinkingLevel::Max).expect("ser"),
            serde_json::json!("max")
        );
        assert_eq!(
            serde_json::to_value(ThinkingLevel::Max).expect("ser"),
            serde_json::json!("max")
        );
        assert_eq!(
            serde_json::from_value::<ModelThinkingLevel>(serde_json::json!("max")).expect("de"),
            ModelThinkingLevel::Max
        );
    }
}
