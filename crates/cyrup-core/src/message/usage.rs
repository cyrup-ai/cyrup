//! Token + cost accounting — [`Usage`] and [`Cost`] (func-01 §4.5).

/// Token + cost accounting (func-01 §4.5).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_write_1h: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning: Option<u64>,
    pub total_tokens: u64,
    pub cost: Cost,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    #[serde(serialize_with = "js_number")]
    pub input: f64,
    #[serde(serialize_with = "js_number")]
    pub output: f64,
    #[serde(serialize_with = "js_number")]
    pub cache_read: f64,
    #[serde(serialize_with = "js_number")]
    pub cache_write: f64,
    #[serde(serialize_with = "js_number")]
    pub total: f64,
}

/// Writes a cost the way `JSON.stringify` writes a number: `0` for zero, `0.000012` for twelve
/// millionths (serde writes `0.0` and `1.2e-5`). pi session files and cyrup session files are the
/// same file format, so a row pi wrote must re-save byte-for-byte. Deserialisation accepts every
/// spelling. A non-finite value has no JSON spelling; JavaScript writes `null`, so does this.
fn js_number<S: serde::Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    use serde::Serialize as _;
    if !value.is_finite() {
        return serializer.serialize_none();
    }
    match serde_json::value::RawValue::from_string(crate::json::js_number_string(*value)) {
        Ok(raw) => raw.serialize(serializer),
        Err(_) => serializer.serialize_f64(*value),
    }
}

/// Field-wise sum of two [`Usage`]s — pi `combineUsage` (`coding-agent/src/core/usage-totals.ts`
/// @v1.0.1, previously `compaction.ts:884-909`). `cacheWrite1h` / `reasoning` stay `None` unless at
/// least one side reports them: pi spreads the key in conditionally, so an absent value must not
/// materialize as a `0`.
pub fn combine_usage(first: &Usage, second: &Usage) -> Usage {
    Usage {
        input: first.input.saturating_add(second.input),
        output: first.output.saturating_add(second.output),
        cache_read: first.cache_read.saturating_add(second.cache_read),
        cache_write: first.cache_write.saturating_add(second.cache_write),
        cache_write_1h: match (first.cache_write_1h, second.cache_write_1h) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
        },
        reasoning: match (first.reasoning, second.reasoning) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
        },
        total_tokens: first.total_tokens.saturating_add(second.total_tokens),
        cost: Cost {
            input: first.cost.input + second.cost.input,
            output: first.cost.output + second.cost.output,
            cache_read: first.cost.cache_read + second.cost.cache_read,
            cache_write: first.cost.cache_write + second.cost.cache_write,
            total: first.cost.total + second.cost.total,
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    /// pi writes the number `0` for a zero cost component (`JSON.stringify(0)`), not `0.0`.
    #[test]
    fn a_zero_cost_serialises_as_pi_writes_it() {
        let json = serde_json::to_string(&Cost::default()).unwrap();
        assert_eq!(
            json,
            r#"{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}"#
        );
    }

    #[test]
    fn a_fractional_cost_keeps_its_fraction_and_both_spellings_load() {
        let cost = Cost {
            input: 0.5,
            output: 3.0,
            cache_read: 0.000_001,
            cache_write: 0.0,
            total: 3.500_001,
        };
        let json = serde_json::to_string(&cost).unwrap();
        assert_eq!(
            json,
            r#"{"input":0.5,"output":3,"cacheRead":0.000001,"cacheWrite":0,"total":3.500001}"#
        );
        assert_eq!(serde_json::from_str::<Cost>(&json).unwrap(), cost);
        let float_spelling =
            r#"{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}"#;
        assert_eq!(
            serde_json::from_str::<Cost>(float_spelling).unwrap(),
            Cost::default()
        );
    }
}
