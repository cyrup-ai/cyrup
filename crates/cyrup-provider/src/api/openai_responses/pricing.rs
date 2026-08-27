//! Stream decoding (Pi processResponsesStream, openai-responses-shared.ts:295-531):
//! service-tier pricing (Pi `getServiceTierCostMultiplier` / `applyServiceTierPricing`).

use crate::model::Model;
use cyrup_core::Usage;

/// Pi `getServiceTierCostMultiplier` (openai-responses.ts:281-293).
fn service_tier_multiplier(model_id: &str, service_tier: Option<&str>) -> f64 {
    match service_tier {
        Some("flex") => 0.5,
        Some("priority") => {
            if model_id == "gpt-5.5" {
                2.5
            } else {
                2.0
            }
        }
        _ => 1.0,
    }
}

/// Pi `applyServiceTierPricing` (openai-responses.ts:295-308).
pub(super) fn apply_service_tier_pricing(
    usage: &mut Usage,
    service_tier: Option<&str>,
    model: &Model,
) {
    let multiplier = service_tier_multiplier(model.id.as_str(), service_tier);
    if multiplier == 1.0 {
        return;
    }
    usage.cost.input *= multiplier;
    usage.cost.output *= multiplier;
    usage.cost.cache_read *= multiplier;
    usage.cost.cache_write *= multiplier;
    usage.cost.total =
        usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
}
