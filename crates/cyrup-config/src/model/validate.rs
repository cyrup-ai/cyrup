//! Field-by-field `models.json` schema validation over a parsed `serde_json::Value`, and the
//! per-field report rendered from its findings.

/// One `models.json` schema violation, in the shape Pi renders it at model-config.ts:274-277
/// @v0.83.0 — `  - ${formatValidationPath(error)}: ${error.message}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelsSchemaError {
    /// The dotted instance path (`formatValidationPath`, model-config.ts:217-228): the JSON pointer
    /// with its leading `/` stripped and the rest of the `/`s turned into `.`; `root` when empty,
    /// and `<basePath>.<missingProperty>` for a `required` failure.
    pub path: String,
    /// The validator's message, e.g. `Expected number`.
    pub message: String,
}

/// Typebox message strings. These are the messages the LIBRARY produces (`typebox/error`), not
/// literals present in `model-config.ts`; the pi code opened at v0.83.0 only interpolates
/// `error.message` (`:276`). Recorded here so the rendered report is one place, not eight.
mod schema_msg {
    pub const REQUIRED: &str = "Expected required property";
    pub const OBJECT: &str = "Expected object";
    pub const ARRAY: &str = "Expected array";
    pub const STRING: &str = "Expected string";
    pub const NUMBER: &str = "Expected number";
    pub const BOOLEAN: &str = "Expected boolean";
    pub const UNION: &str = "Expected union value";
    /// `Type.String({ minLength: 1 })` — the check CFG-046 exists for.
    pub const MIN_LENGTH_1: &str = "Expected string length greater or equal to 1";
}

/// Render a JSON-pointer-ish path segment list the way `formatValidationPath` does
/// (model-config.ts:217-228 @v0.83.0): dotted, `root` when empty.
fn schema_path(segments: &[String]) -> String {
    if segments.is_empty() {
        "root".to_string()
    } else {
        segments.join(".")
    }
}

fn push_err(errs: &mut Vec<ModelsSchemaError>, segments: &[String], message: &str) {
    errs.push(ModelsSchemaError {
        path: schema_path(segments),
        message: message.to_string(),
    });
}

fn child(segments: &[String], key: &str) -> Vec<String> {
    let mut out = segments.to_vec();
    out.push(key.to_string());
    out
}

/// `Type.Optional(Type.String({ minLength: 1 }))` — the shape carried by `name`, `baseUrl`,
/// `apiKey` and `api` on `ProviderConfigSchema` (model-config.ts:188-198 @v0.83.0) and by `name` /
/// `api` / `baseUrl` on `ModelDefinitionSchema` (`:155-158`).
fn check_opt_string_min1(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    let Some(v) = obj.get(key) else { return };
    let here = child(at, key);
    match v.as_str() {
        None => push_err(errs, &here, schema_msg::STRING),
        Some("") => push_err(errs, &here, schema_msg::MIN_LENGTH_1),
        Some(_) => {}
    }
}

fn check_opt_number(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    if let Some(v) = obj.get(key)
        && !v.is_number()
    {
        push_err(errs, &child(at, key), schema_msg::NUMBER);
    }
}

fn check_opt_bool(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    if let Some(v) = obj.get(key)
        && !v.is_boolean()
    {
        push_err(errs, &child(at, key), schema_msg::BOOLEAN);
    }
}

/// `Type.Optional(Type.Record(Type.String(), Type.String()))` — `headers`.
fn check_opt_string_record(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    let Some(v) = obj.get(key) else { return };
    let here = child(at, key);
    let Some(map) = v.as_object() else {
        push_err(errs, &here, schema_msg::OBJECT);
        return;
    };
    for (k, hv) in map {
        if !hv.is_string() {
            push_err(errs, &child(&here, k), schema_msg::STRING);
        }
    }
}

/// `Type.Optional(Type.Array(Type.Union([Type.Literal("text"), Type.Literal("image")])))` —
/// `input` (model-config.ts:161 / :172 @v0.83.0).
fn check_opt_modalities(
    obj: &serde_json::Map<String, serde_json::Value>,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    let Some(v) = obj.get("input") else { return };
    let here = child(at, "input");
    let Some(arr) = v.as_array() else {
        push_err(errs, &here, schema_msg::ARRAY);
        return;
    };
    for (i, item) in arr.iter().enumerate() {
        if !matches!(item.as_str(), Some("text" | "image")) {
            push_err(errs, &child(&here, &i.to_string()), schema_msg::UNION);
        }
    }
}

/// `ModelCostSchema` (model-config.ts:149-152 @v0.83.0): the four rates are REQUIRED, `tiers` is an
/// optional array of `ModelCostTierSchema` (`:145-148`, whose `inputTokensAbove` plus the same four
/// rates are all required).
fn check_cost(
    obj: &serde_json::Map<String, serde_json::Value>,
    at: &[String],
    required_rates: bool,
    errs: &mut Vec<ModelsSchemaError>,
) {
    let Some(v) = obj.get("cost") else { return };
    let here = child(at, "cost");
    let Some(map) = v.as_object() else {
        push_err(errs, &here, schema_msg::OBJECT);
        return;
    };
    for rate in ["input", "output", "cacheRead", "cacheWrite"] {
        match map.get(rate) {
            None if required_rates => push_err(errs, &child(&here, rate), schema_msg::REQUIRED),
            None => {}
            Some(rv) if !rv.is_number() => push_err(errs, &child(&here, rate), schema_msg::NUMBER),
            Some(_) => {}
        }
    }
    let Some(tiers) = map.get("tiers") else {
        return;
    };
    let tiers_at = child(&here, "tiers");
    let Some(arr) = tiers.as_array() else {
        push_err(errs, &tiers_at, schema_msg::ARRAY);
        return;
    };
    for (i, tier) in arr.iter().enumerate() {
        let tier_at = child(&tiers_at, &i.to_string());
        let Some(tm) = tier.as_object() else {
            push_err(errs, &tier_at, schema_msg::OBJECT);
            continue;
        };
        for field in [
            "inputTokensAbove",
            "input",
            "output",
            "cacheRead",
            "cacheWrite",
        ] {
            match tm.get(field) {
                None => push_err(errs, &child(&tier_at, field), schema_msg::REQUIRED),
                Some(fv) if !fv.is_number() => {
                    push_err(errs, &child(&tier_at, field), schema_msg::NUMBER);
                }
                Some(_) => {}
            }
        }
    }
}

/// `ModelDefinitionSchema` (model-config.ts:154-166 @v0.83.0).
fn check_model_definition(
    value: &serde_json::Value,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    let Some(obj) = value.as_object() else {
        push_err(errs, at, schema_msg::OBJECT);
        return;
    };
    match obj.get("id") {
        None => push_err(errs, &child(at, "id"), schema_msg::REQUIRED),
        Some(v) => match v.as_str() {
            None => push_err(errs, &child(at, "id"), schema_msg::STRING),
            Some("") => push_err(errs, &child(at, "id"), schema_msg::MIN_LENGTH_1),
            Some(_) => {}
        },
    }
    for key in ["name", "api", "baseUrl"] {
        check_opt_string_min1(obj, key, at, errs);
    }
    check_opt_bool(obj, "reasoning", at, errs);
    check_opt_modalities(obj, at, errs);
    check_cost(obj, at, true, errs);
    check_opt_number(obj, "contextWindow", at, errs);
    check_opt_number(obj, "maxTokens", at, errs);
    check_opt_string_record(obj, "headers", at, errs);
}

/// `ModelOverrideSchema` (model-config.ts:168-186 @v0.83.0). Its `cost` block differs from a model
/// definition's: every rate is individually optional (`:174-182`).
fn check_model_override(
    value: &serde_json::Value,
    at: &[String],
    errs: &mut Vec<ModelsSchemaError>,
) {
    let Some(obj) = value.as_object() else {
        push_err(errs, at, schema_msg::OBJECT);
        return;
    };
    check_opt_string_min1(obj, "name", at, errs);
    check_opt_bool(obj, "reasoning", at, errs);
    check_opt_modalities(obj, at, errs);
    check_cost(obj, at, false, errs);
    check_opt_number(obj, "contextWindow", at, errs);
    check_opt_number(obj, "maxTokens", at, errs);
    check_opt_string_record(obj, "headers", at, errs);
}

/// Validate a parsed `models.json` against Pi's `ModelsConfigSchema`
/// (`validateModelsConfig.Check(parsed)`, model-config.ts:265 @v0.83.0) and return every failure,
/// which is what Pi renders — `.Errors(parsed).map(...)` at `:272-277`, not just the first.
///
/// **[CYRUP-DELTA]** `compat` is left to serde. Upstream types it as a three-way union of ~40
/// optional keys (`ProviderCompatSchema`, model-config.ts:133-137); reproducing that union's
/// per-arm error text here would duplicate `cyrup_provider::api::compat`'s own definition. A
/// malformed `compat` therefore surfaces through the serde pass below, still under the
/// `Invalid models.json schema:` heading and still naming the offending key.
pub fn validate_models_config(value: &serde_json::Value) -> Vec<ModelsSchemaError> {
    let mut errs: Vec<ModelsSchemaError> = Vec::new();
    let root: Vec<String> = Vec::new();
    let Some(obj) = value.as_object() else {
        push_err(&mut errs, &root, schema_msg::OBJECT);
        return errs;
    };
    // `providers: Type.Record(...)` is NOT optional (model-config.ts:201-203).
    let Some(providers) = obj.get("providers") else {
        push_err(&mut errs, &["providers".to_string()], schema_msg::REQUIRED);
        return errs;
    };
    let providers_at = vec!["providers".to_string()];
    let Some(providers) = providers.as_object() else {
        push_err(&mut errs, &providers_at, schema_msg::OBJECT);
        return errs;
    };
    for (provider_id, provider) in providers {
        let at = child(&providers_at, provider_id);
        let Some(pobj) = provider.as_object() else {
            push_err(&mut errs, &at, schema_msg::OBJECT);
            continue;
        };
        for key in ["name", "baseUrl", "apiKey", "api"] {
            check_opt_string_min1(pobj, key, &at, &mut errs);
        }
        // `oauth: Type.Optional(Type.Literal("radius"))` (model-config.ts:194).
        if let Some(oauth) = pobj.get("oauth")
            && oauth.as_str() != Some("radius")
        {
            push_err(&mut errs, &child(&at, "oauth"), "Expected \"radius\"");
        }
        check_opt_string_record(pobj, "headers", &at, &mut errs);
        check_opt_bool(pobj, "authHeader", &at, &mut errs);
        if let Some(models) = pobj.get("models") {
            let models_at = child(&at, "models");
            match models.as_array() {
                None => push_err(&mut errs, &models_at, schema_msg::ARRAY),
                Some(arr) => {
                    for (i, m) in arr.iter().enumerate() {
                        check_model_definition(m, &child(&models_at, &i.to_string()), &mut errs);
                    }
                }
            }
        }
        if let Some(overrides) = pobj.get("modelOverrides") {
            let ov_at = child(&at, "modelOverrides");
            match overrides.as_object() {
                None => push_err(&mut errs, &ov_at, schema_msg::OBJECT),
                Some(map) => {
                    for (id, ov) in map {
                        check_model_override(ov, &child(&ov_at, id), &mut errs);
                    }
                }
            }
        }
    }
    errs
}

/// Render a schema-failure list as Pi's report body (model-config.ts:272-278 @v0.83.0), including
/// its `|| "Unknown schema error"` fallback for an empty list.
pub(super) fn render_schema_errors(errs: &[ModelsSchemaError]) -> String {
    if errs.is_empty() {
        return "Unknown schema error".to_string();
    }
    errs.iter()
        .map(|e| format!("  - {}: {}", e.path, e.message))
        .collect::<Vec<_>>()
        .join("\n")
}
