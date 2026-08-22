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
