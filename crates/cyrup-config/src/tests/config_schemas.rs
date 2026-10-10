//! CFG-106 — the published `schemas/*.schema.json` documents: committed bytes match the generator,
//! and each schema accepts and rejects what cyrup's runtime does.
//!
//! Port of pi's `packages/coding-agent/test/config-schemas.test.ts` @f1b2e77f5, validated with the
//! `jsonschema` crate in place of TypeBox's `Compile`. Two pi cases are not ported here: the theme
//! cases (the theme schema is split out, see `config_schemas`'s module doc) and the runtime
//! `$schema` cases, which `model/load.rs` (`cfg105_dollar_schema_must_be_a_string`, CFG-105) and
//! the settings round-trip already cover.
//!
//! Regenerate the committed files with
//! `CYRUP_REGENERATE_SCHEMAS=1 cargo test -p cyrup-config config_schemas`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;

use serde_json::{Value, json};

use crate::config_schemas::{
    CONFIG_SCHEMA_PATHS, KEYBINDING_SCHEMA_DESCRIPTIONS, config_schema_url, render_config_schemas,
};
use crate::model::validate_models_config;
use crate::{KEYBINDING_IDS, ModelFile};

const MODELS: &str = "schemas/models.schema.json";
const SETTINGS: &str = "schemas/settings.schema.json";
const KEYBINDINGS: &str = "schemas/keybindings.schema.json";

fn rendered(path: &str) -> Value {
    let (_, text) = render_config_schemas()
        .unwrap()
        .into_iter()
        .find(|(p, _)| *p == path)
        .unwrap_or_else(|| panic!("{path} is not rendered"));
    serde_json::from_str(&text).unwrap()
}

fn validator(path: &str) -> jsonschema::Validator {
    jsonschema::validator_for(&rendered(path))
        .unwrap_or_else(|e| panic!("{path} is not a valid JSON Schema: {e}"))
}

/// Every validation error, as `<instance path>: <message>`, for assertion messages.
fn errors(v: &jsonschema::Validator, doc: &Value) -> Vec<String> {
    v.iter_errors(doc)
        .map(|e| format!("{}: {e}", e.instance_path()))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The committed artifacts (pi `matches the four committed artifacts`, :33-47)
// ---------------------------------------------------------------------------------------------

/// The Verify line's third clause: "the regenerate-and-diff test fails when a settings key is
/// added without regenerating". A MISSING committed file fails too — this is a staleness gate,
/// never a writer, unless `CYRUP_REGENERATE_SCHEMAS=1` asks for one.
#[test]
fn committed_schemas_match_the_generator() {
    let regenerate = std::env::var("CYRUP_REGENERATE_SCHEMAS").as_deref() == Ok("1");
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let rendered = render_config_schemas().unwrap();
    assert_eq!(
        rendered.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
        CONFIG_SCHEMA_PATHS,
        "pi's artifact order (generate-schemas.ts:32-59), theme excluded"
    );
    let mut stale = Vec::new();
    for (path, expected) in rendered {
        let file = crate_dir.join(path);
        if regenerate {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, &expected).unwrap();
            continue;
        }
        match std::fs::read_to_string(&file) {
            Ok(actual) if actual == expected => {}
            Ok(_) => stale.push(format!("{path} is stale")),
            Err(e) => stale.push(format!("{path} is missing ({e})")),
        }
    }
    assert!(
        stale.is_empty(),
        "{}; run CYRUP_REGENERATE_SCHEMAS=1 cargo test -p cyrup-config config_schemas",
        stale.join("; ")
    );
}

#[test]
fn every_document_carries_pi_s_header_and_a_resolvable_id() {
    for path in CONFIG_SCHEMA_PATHS {
        let schema = rendered(path);
        let keys: Vec<&String> = schema.as_object().unwrap().keys().take(3).collect();
        assert_eq!(keys, ["$schema", "$id", "$comment"], "{path}");
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(schema["$id"], config_schema_url(path));
        assert!(
            schema["$id"]
                .as_str()
                .unwrap()
                .ends_with(&format!("crates/cyrup-config/{path}")),
            "the $id must name the committed file: {}",
            schema["$id"]
        );
        // Each document accepts its own `$schema` key, so a file can name it.
        let mut own = json!({ "$schema": config_schema_url(path) });
        if path == MODELS {
            own["providers"] = json!({});
        }
        assert!(validator(path).is_valid(&own), "{path}");
    }
}

// ---------------------------------------------------------------------------------------------
// Representative documents (pi `validates representative $name documents`, :90-140)
// ---------------------------------------------------------------------------------------------

#[test]
fn models_schema_validates_representative_documents() {
    let v = validator(MODELS);
    let valid = json!({
        "providers": {
            "local": {
                "compat": {
                    "zaiToolStream": true,
                    "thinkingTokenBudgetField": "thinking_budget",
                    "chatTemplateKwargs": { "budget": { "$var": "thinking.budget" } },
                    "supportsMidConvoSystemMessages": true,
                },
                "models": [{
                    "id": "model",
                    "inputLimits": { "maxRequestBytes": 1024, "images": { "maxPerRequest": 2 } },
                    "promptCache": { "short": 300 },
                }],
            },
        },
    });
    assert!(v.is_valid(&valid), "{:?}", errors(&v, &valid));
    assert!(!v.is_valid(&json!({ "providers": { "local": { "models": [{ "id": "" }] } } })));
}

#[test]
fn settings_schema_validates_representative_documents() {
    let v = validator(SETTINGS);
    let valid = json!({
        "theme": "dark",
        "cacheWarming": "idle",
        "extensionSetting": { "enabled": true },
        "queueMode": "all",
        "websockets": true,
        "skills": { "enableSkillCommands": true, "customDirectories": ["./skills"] },
        "retry": { "maxDelayMs": 1000 },
    });
    assert!(v.is_valid(&valid), "{:?}", errors(&v, &valid));
    assert!(!v.is_valid(&json!({ "cacheWarming": "always" })));
}

#[test]
fn keybindings_schema_validates_representative_documents() {
    let v = validator(KEYBINDINGS);
    assert!(v.is_valid(&json!({ "app.session.new": "ctrl+n", "extension.action": ["alt+x"] })));
    assert!(!v.is_valid(&json!({ "app.session.new": 42 })));
}

// ---------------------------------------------------------------------------------------------
// models.json (CFG-104 / CFG-105 encoded in the published schema)
// ---------------------------------------------------------------------------------------------

/// pi `rejects non-positive model token limits` (:142-167) — the published half of CFG-105.
#[test]
fn models_schema_rejects_non_positive_token_limits_in_models_and_overrides() {
    let v = validator(MODELS);
    for field in ["contextWindow", "maxTokens"] {
        for value in [json!(0), json!(-1), json!(-0.5)] {
            let model = json!({ "providers": { "local": { "models": [{ "id": "model", field: value }] } } });
            assert!(!v.is_valid(&model), "models[].{field}={value}");
            let over = json!({ "providers": { "local": { "modelOverrides": { "model": { field: value } } } } });
            assert!(!v.is_valid(&over), "modelOverrides.{field}={value}");
        }
    }
    let ok = json!({
        "providers": { "local": {
            "models": [{ "id": "model", "contextWindow": 1, "maxTokens": 1 }],
            "modelOverrides": { "model": { "contextWindow": 1, "maxTokens": 0.5 } },
        } },
    });
    assert!(v.is_valid(&ok), "{:?}", errors(&v, &ok));
}

/// The published half of CFG-104: `samplingParamsByThinkingLevel` is a declared property with one
/// free-form object per thinking level, on a definition and on an override.
#[test]
fn models_schema_declares_sampling_params_by_thinking_level() {
    let v = validator(MODELS);
    let by_level = json!({ "off": { "temperature": 0.2 }, "high": { "top_p": 0.9, "top_k": 20 } });
    let ok = json!({ "providers": { "local": {
        "models": [{ "id": "m", "samplingParams": { "temperature": 0.7 }, "samplingParamsByThinkingLevel": by_level }],
        "modelOverrides": { "m": { "samplingParamsByThinkingLevel": { "max": { "min_p": 0.1 } } } },
    } } });
    assert!(v.is_valid(&ok), "{:?}", errors(&v, &ok));
    for bad in [json!("hot"), json!({ "high": 0.9 }), json!({ "low": [1] })] {
        let doc = json!({ "providers": { "local": { "models": [{ "id": "m", "samplingParamsByThinkingLevel": bad }] } } });
        assert!(!v.is_valid(&doc), "{bad}");
        let doc = json!({ "providers": { "local": { "modelOverrides": { "m": { "samplingParamsByThinkingLevel": bad } } } } });
        assert!(!v.is_valid(&doc), "override {bad}");
    }
    // Editors complete the seven level keys.
    let schema = rendered(MODELS);
    let levels = &schema["properties"]["providers"]["patternProperties"]["^.*$"]["properties"]["models"]
        ["items"]["properties"]["samplingParamsByThinkingLevel"]["properties"];
    assert_eq!(
        levels.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
    );
}

#[test]
fn models_schema_rejects_a_non_string_schema_key_and_a_missing_providers() {
    let v = validator(MODELS);
    assert!(!v.is_valid(&json!({ "$schema": 42, "providers": {} })));
    assert!(!v.is_valid(&json!({})));
    assert!(v.is_valid(&json!({ "$schema": config_schema_url(MODELS), "providers": {} })));
}

/// cyrup's routing types are `deny_unknown_fields` (PROV-066), so the schema closes them too: a
/// misspelled OpenRouter key is flagged in the editor, as it is at load.
#[test]
fn models_schema_closes_the_routing_objects_cyrup_closes() {
    let v = validator(MODELS);
    let doc = |routing: Value| json!({ "providers": { "openrouter": { "compat": { "openRouterRouting": routing } } } });
    assert!(v.is_valid(&doc(json!({ "allow_fallbacks": false, "order": ["a"] }))));
    let typo = doc(json!({ "allow_fallback": false }));
    assert!(!v.is_valid(&typo));
    assert!(
        serde_json::from_value::<ModelFile>(typo).is_err(),
        "the runtime refuses it too"
    );
    // ...while `compat` itself stays open, as pi's is.
    assert!(
        v.is_valid(&json!({ "providers": { "x": { "compat": { "customOption": "value" } } } }))
    );
    assert!(!v.is_valid(
        &json!({ "providers": { "x": { "compat": { "supportsLongCacheRetention": "yes" } } } })
    ));
}

/// Pull every `r#"..."#` raw string out of a Rust source file.
fn raw_strings(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find("r#\"") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("\"#") else { break };
        out.push(after[..end].to_string());
        rest = &after[end + 2..];
    }
    out
}

/// Every source file under `src/` that writes a `models.json` fixture.
const MODELS_FIXTURE_SOURCES: [(&str, &str); 5] = [
    ("model/compose.rs", include_str!("../model/compose.rs")),
    ("model/load.rs", include_str!("../model/load.rs")),
    ("model/schema.rs", include_str!("../model/schema.rs")),
    ("model/validate.rs", include_str!("../model/validate.rs")),
    (
        "tests/models_json_provider.rs",
        include_str!("models_json_provider.rs"),
    ),
];

/// The runtime verdict on a `models.json` document's VALUES: `load_models_file_reporting`'s schema
/// tier (`validate_models_config`), its serde tier, then the per-value checks `compose` makes
/// (`invalid contextWindow`, `invalid inputLimits.*` — pi enforces those in the schema, CFG-085).
/// `compose`'s structural errors (a custom model with no `baseUrl`, an empty block) are pi's
/// composer errors, not schema errors, so they do not count. The base gives every provider a
/// built-in (so a custom model inherits a `baseUrl` and reaches its value checks) and every
/// `modelOverrides` target a model to land on, so an override is actually validated.
fn runtime_accepts(doc: &Value) -> bool {
    if !validate_models_config(doc).is_empty() {
        return false;
    }
    let Ok(file) = serde_json::from_value::<ModelFile>(doc.clone()) else {
        return false;
    };
    let mut base = Vec::new();
    for (provider, block) in &file.providers {
        let ids = std::iter::once("schema-test-base")
            .chain(block.model_overrides.keys().map(String::as_str));
        for id in ids {
            base.push(
                serde_json::from_value::<cyrup_provider::Model>(json!({
                    "id": id, "name": id, "api": "openai-completions", "provider": provider,
                    "baseUrl": "https://base.test/v1", "reasoning": true, "input": ["text"],
                    "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
                    "contextWindow": 1000, "maxTokens": 100,
                }))
                .unwrap(),
            );
        }
    }
    let (_, compose_errors) = file.compose(&base);
    !compose_errors.iter().any(|e| e.contains(": invalid "))
}

/// Verify: "the models schema accepts every `models.json` fixture in `cyrup-config`". Every
/// literal fixture is collected from the sources, so a new one is covered without being listed.
/// The other direction holds too: a fixture the runtime REJECTS (a CFG-105 `maxTokens: -1`, a
/// CFG-046 empty `baseUrl`, a typed-out `oauth`) is rejected by the schema as well.
#[test]
fn models_schema_agrees_with_the_runtime_on_every_models_json_fixture() {
    let v = validator(MODELS);
    let mut accepted = 0;
    let mut rejected = 0;
    let mut disagreements = Vec::new();
    for (file, source) in MODELS_FIXTURE_SOURCES {
        for text in raw_strings(source) {
            let Ok(doc) = serde_json::from_str::<Value>(&text) else {
                continue; // a `format!` template or a non-JSON string
            };
            if doc.get("providers").is_none() {
                continue;
            }
            let runtime = runtime_accepts(&doc);
            let schema = v.is_valid(&doc);
            if runtime {
                accepted += 1;
            } else {
                rejected += 1;
            }
            if runtime != schema {
                disagreements.push(format!(
                    "{file}: runtime {runtime}, schema {schema}: {text}\n  {:?}",
                    errors(&v, &doc)
                ));
            }
        }
    }
    assert!(
        accepted >= 35,
        "fixture scan found only {accepted} accepted documents"
    );
    assert!(
        rejected >= 3,
        "fixture scan found only {rejected} rejected documents"
    );
    assert!(disagreements.is_empty(), "{}", disagreements.join("\n"));
}

// ---------------------------------------------------------------------------------------------
// settings.json
// ---------------------------------------------------------------------------------------------

/// pi `rejects settings values that runtime accessors reject` (:187-217). The first and fifth are
/// the two documents the Verify line names.
#[test]
fn settings_schema_rejects_values_runtime_accessors_reject() {
    let v = validator(SETTINGS);
    for invalid in [
        json!({ "compaction": { "reserveTokens": -1 } }),
        json!({ "compaction": { "keepRecentTokens": 1.5 } }),
        json!({ "compaction": { "modelOverrides": { "provider/model": { "reserveTokens": 9_007_199_254_740_992_u64 } } } }),
        json!({ "httpIdleTimeoutMs": -1 }),
        json!({ "httpIdleTimeoutMs": "bogus" }),
        json!({ "websocketConnectTimeoutMs": -1 }),
        json!({ "websocketConnectTimeoutMs": "30000" }),
        json!({ "codemode": { "inlineBudget": -1 } }),
    ] {
        assert!(!v.is_valid(&invalid), "accepted {invalid}");
    }
    let valid = json!({
        "compaction": {
            "reserveTokens": 0,
            "keepRecentTokens": 9_007_199_254_740_991_u64,
            "modelOverrides": { "provider/model": { "reserveTokens": 0 } },
        },
        "httpIdleTimeoutMs": 0,
        "websocketConnectTimeoutMs": "disabled",
        "codemode": { "inlineBudget": 0 },
    });
    assert!(v.is_valid(&valid), "{:?}", errors(&v, &valid));
}

/// The two Verify documents are rejected by cyrup's getters too — the schema states the runtime
/// rule rather than inventing one.
#[test]
fn the_verify_documents_are_runtime_errors_as_well() {
    use crate::{EffectiveSettings, Settings};
    let eff = |text: &str| EffectiveSettings::from_settings(Settings::parse(text).unwrap());
    assert!(
        eff(r#"{"compaction":{"reserveTokens":-1}}"#)
            .compaction_reserve_tokens(None)
            .is_err()
    );
    assert!(
        eff(r#"{"httpIdleTimeoutMs":"bogus"}"#)
            .http_idle_timeout_ms()
            .is_err()
    );
}

/// pi `preserves moved model and settings guidance` (:66-88) and `describes legacy settings
/// migrations accurately` (:219-230).
#[test]
fn schemas_keep_pi_s_editor_guidance() {
    let models = rendered(MODELS);
    let cost = &models["$defs"]["ModelCost"]["properties"];
    assert!(
        cost["input"]["description"]
            .as_str()
            .unwrap()
            .contains("USD per million tokens")
    );
    assert!(
        cost["tiers"]["description"]
            .as_str()
            .unwrap()
            .contains("highest matching input threshold")
    );
    assert!(
        models["$defs"]["ModelInputLimits"]["properties"]["maxRequestBytes"]["description"]
            .as_str()
            .unwrap()
            .contains("serialized provider request size")
    );

    let settings = rendered(SETTINGS);
    let props = &settings["properties"];
    for (key, needle) in [
        ("quietStartup", "hide all startup output"),
        ("defaultTools", "+name and -name"),
        ("fullscreenWheelScrollLines", "1 to 100"),
    ] {
        assert!(
            props[key]["description"].as_str().unwrap().contains(needle),
            "{key}"
        );
    }
    assert!(
        props["retry"]["properties"]["maxDelayMs"]["description"]
            .as_str()
            .unwrap()
            .contains("provider.maxRetryDelayMs")
    );
    assert_eq!(
        props["queueMode"]["description"],
        "Legacy setting migrated to steeringMode."
    );
    assert_eq!(props["queueMode"]["deprecated"], true);
    assert_eq!(props["websockets"]["deprecated"], true);
    assert_eq!(settings["additionalProperties"], true);
}

/// The defaults the schema advertises are the ones cyrup's getters apply — spot-checked against
/// literal values so a getter change shows up here as well as in the committed file.
#[test]
fn settings_schema_defaults_are_the_getter_defaults() {
    let settings = rendered(SETTINGS);
    let p = &settings["properties"];
    assert_eq!(
        p["compaction"]["properties"]["reserveTokens"]["default"],
        16384
    );
    assert_eq!(
        p["compaction"]["properties"]["keepRecentTokens"]["default"],
        20000
    );
    assert_eq!(p["transport"]["default"], "auto");
    assert_eq!(p["tuiMode"]["default"], "fullscreen");
    assert_eq!(p["cacheWarming"]["default"], "streaming");
    assert_eq!(p["quietStartup"]["default"], false);
    assert_eq!(p["defaultProjectTrust"]["default"], "ask");
    assert_eq!(p["retry"]["properties"]["maxRetries"]["default"], 3);
    assert_eq!(
        p["retry"]["properties"]["provider"]["properties"]["maxRetryDelayMs"]["default"],
        60000
    );
    assert_eq!(p["codemode"]["properties"]["inlineBudget"]["default"], 3000);
    assert_eq!(p["fullscreenWheelScrollLines"]["default"], "auto");
    assert_eq!(
        p["markdown"]["properties"]["mermaid"]["default"],
        "streaming"
    );
    assert_eq!(p["outputPad"]["default"], 1);
    assert!(
        p["httpIdleTimeoutMs"].get("default").is_none(),
        "pi states no default here"
    );
}

/// Every `settings/tests` source, scanned for fixtures.
const SETTINGS_FIXTURE_SOURCES: [(&str, &str); 5] = [
    ("getters.rs", include_str!("../settings/tests/getters.rs")),
    (
        "merge_and_scope.rs",
        include_str!("../settings/tests/merge_and_scope.rs"),
    ),
    (
        "quiet_startup.rs",
        include_str!("../settings/tests/quiet_startup.rs"),
    ),
    (
        "wheel_lines.rs",
        include_str!("../settings/tests/wheel_lines.rs"),
    ),
    (
        "write_refusal.rs",
        include_str!("../settings/tests/write_refusal.rs"),
    ),
];

/// The fixtures under `settings/tests` that exist to feed a getter a value pi's schema forbids —
/// each pins how the runtime DEGRADES on a bad value, so it is not a valid `settings.json`, and the
/// schema must reject it. Matched by the fixture's compact JSON form.
const DELIBERATELY_INVALID_SETTINGS_FIXTURES: &[(&str, &str)] = &[
    // Getters that THROW on the value (pi's accessors throw; cyrup returns `Err`).
    (
        r#"{"httpIdleTimeoutMs":"garbage"}"#,
        "http_idle_timeout_ms rejects it",
    ),
    (
        r#"{"compaction":{"reserveTokens":-1}}"#,
        "compaction getter rejects it",
    ),
    (
        r#"{"compaction":{"keepRecentTokens":1.5}}"#,
        "compaction getter rejects it",
    ),
    (
        r#"{"compaction":{"reserveTokens":"4096"}}"#,
        "compaction getter rejects it",
    ),
    (
        r#"{"compaction":{"reserveTokens":9007199254740992}}"#,
        "compaction getter rejects it",
    ),
    (
        r#"{"compaction":{"reserveTokens":null,"modelOverrides":{"a/b":{"reserveTokens":1}}}}"#,
        "compaction getter rejects the null ordinary value",
    ),
    (
        r#"{"compaction":{"modelOverrides":{"a/b":[1,2]}}}"#,
        "compaction getter rejects it",
    ),
    (
        r#"{"compaction":{"modelOverrides":{"a/b":{"keepRecentTokens":{"n":1}}}}}"#,
        "compaction getter rejects it",
    ),
    (
        r#"{"compaction":{"modelOverrides":{"x/y":"bad"}}}"#,
        "compaction getter rejects it",
    ),
    // Getters that DEGRADE an out-of-domain value to the default (pi's `=== "x" ? x : default`).
    (
        r#"{"markdown":{"mermaid":"nonsense"}}"#,
        "reads back as streaming",
    ),
    (
        r#"{"fullscreenExitOutput":"nothing"}"#,
        "reads back as transcript",
    ),
    (
        r#"{"fullscreenExitOutput":true}"#,
        "reads back as transcript",
    ),
    (r#"{"fullscreenCopyOnSelect":null}"#, "reads back as true"),
    (
        r#"{"fullscreenCopyOnSelect":"false"}"#,
        "a string is not a boolean",
    ),
    (r#"{"outputPad":5}"#, "reads back as 1"),
    (r#"{"defaultThinkingLevel":"ultra"}"#, "reads back as unset"),
    (
        r#"{"thinkingBudgets":{"minimal":50,"low":"oops","medium":700},"warnings":{"anthropicExtraUsage":"nope"}}"#,
        "the bad members read back as unset",
    ),
    (
        r#"{"terminal":{"images":true}}"#,
        "only false is a boolean images value",
    ),
    (r#"{"tuiMode":"other"}"#, "reads back as fullscreen"),
    (
        r#"{"tuiMode":"Regular"}"#,
        "case-sensitive; reads back as fullscreen",
    ),
    (r#"{"tuiMode":true}"#, "reads back as fullscreen"),
    (r#"{"tuiMode":null}"#, "reads back as fullscreen"),
    (
        r#"{"defaultTools":["+grep",7,null]}"#,
        "non-string entries are dropped",
    ),
    (r#"{"defaultTools":[7]}"#, "non-string entries are dropped"),
    (
        r#"{"defaultTools":"read"}"#,
        "a non-array reads back as unset",
    ),
    (r#"{"defaultTools":{}}"#, "a non-array reads back as unset"),
    (
        r#"{"defaultTools":"grep"}"#,
        "a non-array reads back as unset",
    ),
    (
        r#"{"codemode":{"mode":"only","inlineBudget":"x"}}"#,
        "budget reads back as 3000",
    ),
    (r#"{"npmCommand":null}"#, "reads back as unset"),
    (r#"{"cacheWarming":"sometimes"}"#, "reads back as streaming"),
    (r#"{"quietStartup":"bogus"}"#, "reads back as false"),
    // `packages` entries the loader reports and skips.
    (
        r#"{"packages":["a","b","c",42,"e","f","g","h","i","j"]}"#,
        "42 is reported and skipped",
    ),
    (
        r#"{"defaultModel":"anthropic/x","packages":[17,"good-pkg",{"source":"filtered","skills":["a"]}]}"#,
        "17 is reported and skipped",
    ),
    (r#"{"packages":"oops"}"#, "a non-array is reported"),
];

/// Verify: "`schemas/settings.schema.json` validates every `settings.json` fixture under
/// `crates/cyrup-config/src/settings/tests`". Every raw-string JSON object in those files is
/// collected. The ones that exist to exercise a getter's handling of an out-of-domain value are
/// listed in [`DELIBERATELY_INVALID_SETTINGS_FIXTURES`]; that list must be EXACTLY the set the
/// schema rejects, so neither a newly rejected valid fixture nor a stale exemption gets through.
#[test]
fn settings_schema_validates_every_settings_fixture() {
    let v = validator(SETTINGS);
    let mut checked = 0;
    let mut rejected = Vec::new();
    for (file, source) in SETTINGS_FIXTURE_SOURCES {
        for text in raw_strings(source) {
            let Ok(doc @ Value::Object(_)) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            checked += 1;
            if !v.is_valid(&doc) {
                rejected.push((file, doc.to_string(), errors(&v, &doc)));
            }
        }
    }
    assert!(
        checked >= 100,
        "fixture scan found only {checked} documents"
    );
    let unexpected: Vec<String> = rejected
        .iter()
        .filter(|(_, text, _)| {
            !DELIBERATELY_INVALID_SETTINGS_FIXTURES
                .iter()
                .any(|(fixture, _)| fixture == text)
        })
        .map(|(file, text, errs)| format!("{file}: {text}\n  {errs:?}"))
        .collect();
    assert!(
        unexpected.is_empty(),
        "schema rejects settings fixtures:\n{}",
        unexpected.join("\n")
    );
    let stale: Vec<&str> = DELIBERATELY_INVALID_SETTINGS_FIXTURES
        .iter()
        .map(|(fixture, _)| *fixture)
        .filter(|f| !rejected.iter().any(|(_, text, _)| text == f))
        .collect();
    assert!(
        stale.is_empty(),
        "exempted fixtures the schema accepts (or that no longer exist): {stale:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// keybindings.json
// ---------------------------------------------------------------------------------------------

/// pi `validates keybinding syntax` (:169-185).
#[test]
fn keybindings_schema_validates_key_syntax() {
    let v = validator(KEYBINDINGS);
    for binding in [
        "a",
        "9",
        "pageUp",
        "+",
        "ctrl+shift+x",
        "alt+ctrl+?",
        "ctrl+shift+alt+super+f12",
    ] {
        assert!(
            v.is_valid(&json!({ "extension.action": binding })),
            "{binding}"
        );
    }
    for binding in [
        "",
        "Ctrl+x",
        "control+x",
        "ctrl+not-a-key",
        "ctrl+ctrl+x",
        "ctrl+shift+alt+super+ctrl+x",
    ] {
        assert!(
            !v.is_valid(&json!({ "extension.action": binding })),
            "{binding}"
        );
    }
    assert!(v.is_valid(&json!({ "app.session.new": ["ctrl+n", "alt+n"] })));
    assert!(!v.is_valid(&json!({ "app.session.new": ["ctrl+n", "Alt+N"] })));
}

/// Every id cyrup's keybinding table declares is a documented property of the schema, in the same
/// relative order.
#[test]
fn keybindings_schema_documents_every_declared_id() {
    let ids: Vec<&str> = KEYBINDING_SCHEMA_DESCRIPTIONS
        .iter()
        .map(|(id, _)| *id)
        .collect();
    let missing: Vec<&&str> = KEYBINDING_IDS
        .iter()
        .filter(|id| !ids.contains(id))
        .collect();
    assert!(
        missing.is_empty(),
        "undocumented keybinding ids: {missing:?}"
    );
    let in_schema_order: Vec<&str> = ids
        .iter()
        .copied()
        .filter(|id| KEYBINDING_IDS.contains(id))
        .collect();
    assert_eq!(in_schema_order, KEYBINDING_IDS, "declaration order");

    let schema = rendered(KEYBINDINGS);
    let props = schema["properties"].as_object().unwrap();
    for (id, description) in KEYBINDING_SCHEMA_DESCRIPTIONS {
        assert_eq!(props[id]["description"], description, "{id}");
        assert_eq!(props[id]["$ref"], "#/$defs/KeybindingValue", "{id}");
    }
    assert_eq!(props.len(), KEYBINDING_SCHEMA_DESCRIPTIONS.len() + 1);
}
