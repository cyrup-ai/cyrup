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
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    /// Byte-exact pin for the two skippable token buckets. `cacheWrite1h` is the non-obvious
    /// `rename_all = "camelCase"` case — the `1h` segment starts with a digit, so serde emits
    /// `cacheWrite1h` (NOT `cacheWrite1H`), which is what Pi writes. `reasoning` is single-segment
    /// and unrenamed. Both sit between `cacheWrite` and `totalTokens` in Pi's field order.
    #[test]
    fn usage_optional_buckets_emit_pi_camelcase_bytes() {
        let u = Usage {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: Some(400_000),
            reasoning: Some(12),
            total_tokens: 0,
            cost: Cost::default(),
        };
        assert_eq!(
            serde_json::to_string(&u).expect("serialize"),
            concat!(
                r#"{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"#,
                r#""cacheWrite1h":400000,"reasoning":12,"totalTokens":0,"#,
                r#""cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}"#,
            )
        );
        let back: Usage = serde_json::from_str(&serde_json::to_string(&u).expect("serialize"))
            .expect("deserialize");
        assert_eq!(back, u);
    }

    /// `skip_serializing_if = "Option::is_none"`: neither key may appear when unset — Pi omits
    /// them rather than writing `null`, and a spurious `cacheWrite1h: null` would be a wire delta.
    #[test]
    fn usage_optional_buckets_absent_when_none() {
        let u = Usage { cache_write_1h: None, reasoning: None, ..Usage::default() };
        let s = serde_json::to_string(&u).expect("serialize");
        assert!(!s.contains("cacheWrite1h"), "cacheWrite1h omitted when None: {s}");
        assert!(!s.contains("reasoning"), "reasoning omitted when None: {s}");
        let v = serde_json::to_value(&u).expect("serialize");
        assert!(v.get("cacheWrite1h").is_none());
        assert!(v.get("reasoning").is_none());
        // Absent on the wire round-trips back to `None` (both fields carry `default`).
        assert_eq!(serde_json::from_str::<Usage>(&s).expect("deserialize"), u);
    }

    /// The zero value — what every synthesized/errored assistant turn carries — is exactly Pi's
    /// six always-present keys and nothing else.
    #[test]
    fn usage_default_emits_no_optional_keys() {
        let s = serde_json::to_string(&Usage::default()).expect("serialize");
        assert_eq!(
            s,
            concat!(
                r#"{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"#,
                r#""cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}"#,
            )
        );
        let v = serde_json::to_value(Usage::default()).expect("serialize");
        assert_eq!(v.as_object().expect("object").len(), 6, "no optional keys: {v}");
    }
}
