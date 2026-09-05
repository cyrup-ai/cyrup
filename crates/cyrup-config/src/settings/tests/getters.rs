use std::sync::Arc;

use cyrup_core::ModelThinkingLevel;

use crate::settings::*;
#[test]
fn migrations_applied_on_parse() {
    // settings-manager.ts:376-435
    // queueMode -> steeringMode
    let s = Settings::parse(r#"{ "queueMode": "all" }"#).unwrap();
    assert_eq!(s.get("steeringMode"), Some(&serde_json::json!("all")));
    assert!(s.get("queueMode").is_none());
    // websockets bool -> transport
    let s = Settings::parse(r#"{ "websockets": true }"#).unwrap();
    assert_eq!(s.get("transport"), Some(&serde_json::json!("websocket")));
    let s = Settings::parse(r#"{ "websockets": false }"#).unwrap();
    assert_eq!(s.get("transport"), Some(&serde_json::json!("sse")));
    // skills object -> array
    let s = Settings::parse(
        r#"{ "skills": { "enableSkillCommands": false, "customDirectories": ["/a", "/b"] } }"#,
    )
    .unwrap();
    assert_eq!(s.get("skills"), Some(&serde_json::json!(["/a", "/b"])));
    assert_eq!(
        s.get("enableSkillCommands"),
        Some(&serde_json::json!(false))
    );
    // retry.maxDelayMs -> retry.provider.maxRetryDelayMs
    let s = Settings::parse(r#"{ "retry": { "maxDelayMs": 5000 } }"#).unwrap();
    assert_eq!(
        s.get("retry").unwrap()["provider"]["maxRetryDelayMs"],
        serde_json::json!(5000)
    );
    assert!(s.get("retry").unwrap().get("maxDelayMs").is_none());
}

#[test]
fn typed_accessors_defaults() {
    let s = EffectiveSettings::from_settings(Settings::default());
    assert_eq!(s.steering_mode(), "one-at-a-time");
    assert_eq!(s.transport(), "auto");
    assert!(s.compaction_enabled());
    assert_eq!(s.compaction_reserve_tokens(), 16384);
    assert_eq!(s.retry_max_retries(), 3);
    assert_eq!(s.provider_max_retry_delay_ms(), 60000);
    assert_eq!(s.http_idle_timeout_ms().unwrap(), 300_000);
    assert!(s.show_images());
    assert_eq!(s.image_width_cells(), 60);
    assert_eq!(s.double_escape_action(), "tree");
    assert_eq!(s.tree_filter_mode(), "default");
    assert_eq!(s.autocomplete_max_visible(), 5);
    assert_eq!(s.code_block_indent(), "  ");
    // CFG-014 — `showCacheMissNotices` defaults to false (settings-manager.ts:96 @v0.83.0).
    assert!(!s.show_cache_miss_notices());
    // `outputPad` defaults to 1 (Pi `getOutputPad`: only an explicit 0 yields 0).
    assert_eq!(s.output_pad(), 1);
}

/// CFG-014 — the key round-trips through the merged view, so the TUI consumer (PROV-035's
/// wiring half) has a real value to read rather than a hardcoded `false`.
#[test]
fn show_cache_miss_notices_reads_the_settings_key() {
    let s = EffectiveSettings::from_settings(
        Settings::parse(r#"{"showCacheMissNotices": true}"#).expect("valid settings"),
    );
    assert!(s.show_cache_miss_notices());
}

/// CFG-030: a top level that is valid JSON but not an object is an ERROR, so the load-error
/// latch (`record_load_error` → `ensure_scope_writable`) engages and the next `/config` write is
/// REFUSED instead of rewriting the user's file from an empty document.
///
/// Red at HEAD: `Settings::parse` matched `Value::Object(..)` and returned `Ok(default)` for
/// everything else, so `[1,2,3]` parsed clean, produced no diagnostic, and was silently emptied
/// on the next write.
#[test]
fn a_non_object_top_level_settings_document_is_a_parse_error() {
    for text in ["[1,2,3]", "\"hello\"", "42", "null", "true"] {
        assert!(
            Settings::parse(text).is_err(),
            "non-object top level {text:?} must not parse as empty settings"
        );
    }
    // An object and an empty document are still fine.
    assert!(Settings::parse("{}").is_ok());
    assert!(Settings::parse("   ").is_ok());
}

/// CFG-040: `getMermaidRenderingMode` VALIDATES rather than parses —
/// `mode === "off" || mode === "final" ? mode : "streaming"` (settings-manager.ts:1251-1254
/// @v0.84.1) — so an unknown value and an absent key both yield `Streaming`.
///
/// Red at HEAD: `grep -rni mermaid crates/cyrup-config/src` returned ZERO; there was no getter.
#[test]
fn mermaid_rendering_mode_defaults_to_streaming_and_accepts_only_pis_three_values() {
    let g = |json: &str| {
        EffectiveSettings::from_settings(Settings::parse(json).unwrap()).mermaid_rendering_mode()
    };
    assert_eq!(
        g(r#"{"markdown":{"mermaid":"off"}}"#),
        MermaidRenderingMode::Off
    );
    assert_eq!(
        g(r#"{"markdown":{"mermaid":"final"}}"#),
        MermaidRenderingMode::Final
    );
    assert_eq!(
        g(r#"{"markdown":{"mermaid":"streaming"}}"#),
        MermaidRenderingMode::Streaming
    );
    assert_eq!(
        g(r#"{"markdown":{"mermaid":"nonsense"}}"#),
        MermaidRenderingMode::Streaming
    );
    assert_eq!(g("{}"), MermaidRenderingMode::Streaming);
    // A sibling markdown key is untouched by the getter.
    let s = Settings::parse(r#"{"markdown":{"codeBlockIndent":"\t","mermaid":"off"}}"#).unwrap();
    let eff = EffectiveSettings::from_settings(s);
    assert_eq!(eff.mermaid_rendering_mode(), MermaidRenderingMode::Off);
    assert_eq!(eff.code_block_indent(), "\t");
}

/// CFG-078 — the two v0.84.4 alt-screen keys, both of which DEGRADE rather than reject.
///
/// `getFullscreenExitOutput` is `this.settings.fullscreenExitOutput === "resume-hint" ?
/// "resume-hint" : "transcript"` (settings-manager.ts:1212-1214 @v0.84.4) and
/// `getFullscreenCopyOnSelect` is `this.settings.fullscreenCopyOnSelect ?? true` (`:1233-1235`).
/// The unknown-value legs are upstream's own case: `{"fullscreenExitOutput":"nothing"}` reads back
/// as `"transcript"` and the absent copy-on-select key as `true`
/// (`test/settings-manager.test.ts:471-476` @v0.84.4).
///
/// Red at HEAD before this change: `grep -rn 'fullscreenExitOutput\|fullscreenCopyOnSelect'
/// crates/cyrup-config/src` returned ZERO — neither getter existed, so the file did not compile.
#[test]
fn fullscreen_exit_output_and_copy_on_select_degrade_to_pis_defaults() {
    let eff = |json: &str| EffectiveSettings::from_settings(Settings::parse(json).unwrap());
    assert_eq!(
        eff(r#"{"fullscreenExitOutput":"resume-hint"}"#).fullscreen_exit_output(),
        FullscreenExitOutput::ResumeHint
    );
    assert_eq!(
        eff(r#"{"fullscreenExitOutput":"transcript"}"#).fullscreen_exit_output(),
        FullscreenExitOutput::Transcript
    );
    // Upstream's own "nothing" case, plus a wrong-typed value and the absent key.
    assert_eq!(
        eff(r#"{"fullscreenExitOutput":"nothing"}"#).fullscreen_exit_output(),
        FullscreenExitOutput::Transcript
    );
    assert_eq!(
        eff(r#"{"fullscreenExitOutput":true}"#).fullscreen_exit_output(),
        FullscreenExitOutput::Transcript
    );
    assert_eq!(
        eff("{}").fullscreen_exit_output(),
        FullscreenExitOutput::Transcript
    );
    assert_eq!(
        EffectiveSettings::from_settings(Settings::default()).fullscreen_exit_output(),
        FullscreenExitOutput::Transcript
    );

    // `?? true`: only an absent/null key defaults, an explicit `false` is honoured.
    assert!(!eff(r#"{"fullscreenCopyOnSelect":false}"#).fullscreen_copy_on_select());
    assert!(eff(r#"{"fullscreenCopyOnSelect":true}"#).fullscreen_copy_on_select());
    assert!(eff(r#"{"fullscreenCopyOnSelect":null}"#).fullscreen_copy_on_select());
    assert!(eff("{}").fullscreen_copy_on_select());
    // A non-boolean is not a boolean; pi would read `"false"` as truthy, and so does this.
    assert!(eff(r#"{"fullscreenCopyOnSelect":"false"}"#).fullscreen_copy_on_select());
}

/// CFG-078 — both setters write the GLOBAL scope with the spelling upstream stores
/// (settings-manager.ts:1216-1220 / `:1237-1241` @v0.84.4), so the keys round-trip between the two
/// implementations, and neither disturbs the other key or an unrelated one (R-07-004).
///
/// Red at HEAD before this change: `set_fullscreen_exit_output` /
/// `set_fullscreen_copy_on_select` did not exist.
#[tokio::test]
async fn fullscreen_exit_output_and_copy_on_select_round_trip_through_the_global_scope() {
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "theme": "dark" }"#);
    let mut mgr = SettingsManager::load(store.clone(), false);

    mgr.set_fullscreen_exit_output(FullscreenExitOutput::ResumeHint)
        .await
        .unwrap();
    mgr.set_fullscreen_copy_on_select(false).await.unwrap();

    let s = Settings::parse(&store.read(SettingsScope::Global).unwrap().unwrap()).unwrap();
    assert_eq!(
        s.get("fullscreenExitOutput"),
        Some(&serde_json::json!("resume-hint"))
    );
    assert_eq!(
        s.get("fullscreenCopyOnSelect"),
        Some(&serde_json::json!(false))
    );
    assert_eq!(s.get("theme"), Some(&serde_json::json!("dark")));

    assert_eq!(
        mgr.effective().fullscreen_exit_output(),
        FullscreenExitOutput::ResumeHint
    );
    assert!(!mgr.effective().fullscreen_copy_on_select());

    // And back, which is the leg that proves the reader is not latching.
    mgr.set_fullscreen_exit_output(FullscreenExitOutput::Transcript)
        .await
        .unwrap();
    mgr.set_fullscreen_copy_on_select(true).await.unwrap();
    assert_eq!(
        mgr.effective().fullscreen_exit_output(),
        FullscreenExitOutput::Transcript
    );
    assert!(mgr.effective().fullscreen_copy_on_select());
}

#[test]
fn output_pad_only_explicit_zero_disables() {
    // Pi `getOutputPad`: `outputPad === 0 ? 0 : 1` — only an explicit 0 turns padding off.
    let zero = EffectiveSettings::from_settings(Settings::parse(r#"{ "outputPad": 0 }"#).unwrap());
    assert_eq!(zero.output_pad(), 0);
    let one = EffectiveSettings::from_settings(Settings::parse(r#"{ "outputPad": 1 }"#).unwrap());
    assert_eq!(one.output_pad(), 1);
    // A stray/unexpected value (or unset) resolves to the default 1, not 0.
    let stray = EffectiveSettings::from_settings(Settings::parse(r#"{ "outputPad": 5 }"#).unwrap());
    assert_eq!(stray.output_pad(), 1);
}

#[test]
fn http_idle_timeout_invalid_errors() {
    let s = EffectiveSettings::from_settings(
        Settings::parse(r#"{ "httpIdleTimeoutMs": "garbage" }"#).unwrap(),
    );
    assert!(s.http_idle_timeout_ms().is_err());
    let s = EffectiveSettings::from_settings(
        Settings::parse(r#"{ "httpIdleTimeoutMs": "disabled" }"#).unwrap(),
    );
    assert_eq!(s.http_idle_timeout_ms().unwrap(), 0);
}

#[tokio::test]
async fn nested_set_preserves_siblings() {
    // R-07-004: setting terminal.showImages must not clobber terminal.imageWidthCells.
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(
        SettingsScope::Global,
        r#"{ "terminal": { "imageWidthCells": 40 } }"#,
    );
    let mut mgr = SettingsManager::load(store.clone(), false);
    mgr.set_show_images(false).await.unwrap();
    let raw = store.read(SettingsScope::Global).unwrap().unwrap();
    let s = Settings::parse(&raw).unwrap();
    assert_eq!(
        s.get("terminal").unwrap()["showImages"],
        serde_json::json!(false)
    );
    assert_eq!(
        s.get("terminal").unwrap()["imageWidthCells"],
        serde_json::json!(40)
    );
}

#[tokio::test]
async fn setters_clamp() {
    let store = Arc::new(InMemorySettingsStore::new());
    let mut mgr = SettingsManager::load(store.clone(), false);
    mgr.set_editor_padding_x(9.0).await.unwrap();
    assert_eq!(mgr.effective().editor_padding_x(), 3);
    mgr.set_autocomplete_max_visible(1.0).await.unwrap();
    assert_eq!(mgr.effective().autocomplete_max_visible(), 3);
    mgr.set_image_width_cells(0.0).await.unwrap();
    assert_eq!(mgr.effective().image_width_cells(), 1);
    assert!(mgr.set_http_idle_timeout_ms(-5.0).await.is_err());
}

#[test]
fn default_thinking_level_reads_correct_key() {
    // settings-manager.ts:84,735-737 — the key is `defaultThinkingLevel`.
    let s = EffectiveSettings::from_settings(
        Settings::parse(r#"{ "defaultThinkingLevel": "high" }"#).unwrap(),
    );
    assert_eq!(s.default_thinking_level(), Some(ModelThinkingLevel::High));
    // The legacy/wrong key must NOT be honoured.
    let s = EffectiveSettings::from_settings(
        Settings::parse(r#"{ "defaultModelThinkingLevel": "high" }"#).unwrap(),
    );
    assert_eq!(s.default_thinking_level(), None);
}

/// CFG-056 — RED before the fix, which returned `ModelThinkingLevel::default()` (= `Off`) for
/// an unset key. Pi's getter returns `undefined` (settings-manager.ts:740-742 @v0.83.0) and
/// every consumer falls back to `DEFAULT_THINKING_LEVEL` = `"medium"`
/// (`core/defaults.ts:3`), so the unset case must NOT be `Off` and must NOT be decided here.
#[test]
fn unset_default_thinking_level_is_none_and_falls_back_to_medium() {
    let s = EffectiveSettings::from_settings(Settings::parse("{}").unwrap());
    assert_eq!(s.default_thinking_level(), None);
    assert_eq!(
        s.default_thinking_level()
            .unwrap_or(crate::DEFAULT_THINKING_LEVEL),
        ModelThinkingLevel::Medium,
    );
    assert_ne!(crate::DEFAULT_THINKING_LEVEL, ModelThinkingLevel::default());
}

/// PROV-002 / pi `test/max-thinking.test.ts` ("is accepted by CLI and settings"): a settings
/// file declaring `"max"` must round-trip to the `Max` rung, not silently fall back to `off`.
#[test]
fn default_thinking_level_accepts_max() {
    let s = EffectiveSettings::from_settings(
        Settings::parse(r#"{ "defaultThinkingLevel": "max" }"#).unwrap(),
    );
    assert_eq!(s.default_thinking_level(), Some(ModelThinkingLevel::Max));
    // A genuinely unknown level still degrades to "unset" rather than erroring, and the
    // consumer's `DEFAULT_THINKING_LEVEL` fallback then applies (CFG-056).
    let s = EffectiveSettings::from_settings(
        Settings::parse(r#"{ "defaultThinkingLevel": "ultra" }"#).unwrap(),
    );
    assert_eq!(s.default_thinking_level(), None);
}

#[test]
fn theme_split_get_theme_vs_get_theme_setting() {
    // settings-manager.ts:718-727
    let s = EffectiveSettings::from_settings(Settings::parse(r#"{ "theme": "light" }"#).unwrap());
    assert_eq!(s.theme_setting(), Some("light".to_string()));
    assert_eq!(s.theme(), Some("light".to_string()));
    // namespaced (a/b) themes resolve to None in getTheme but are kept in getThemeSetting.
    let s =
        EffectiveSettings::from_settings(Settings::parse(r#"{ "theme": "pkg/dark" }"#).unwrap());
    assert_eq!(s.theme_setting(), Some("pkg/dark".to_string()));
    assert_eq!(s.theme(), None);
    // unset: both None (the "dark" default lives in the TUI layer).
    let s = EffectiveSettings::from_settings(Settings::default());
    assert_eq!(s.theme_setting(), None);
    assert_eq!(s.theme(), None);
}

#[test]
fn hardware_cursor_and_clear_on_shrink_env_fallback() {
    // settings-manager.ts:1077-1083,1165-1167 — setting wins, then env (== "1"), else false.
    let mut env = crate::env::EnvVars::default();
    let empty = EffectiveSettings::from_settings(Settings::default());
    assert!(!empty.show_hardware_cursor(&env));
    assert!(!empty.clear_on_shrink(&env));
    env.hardware_cursor = true;
    env.clear_on_shrink = true;
    assert!(empty.show_hardware_cursor(&env));
    assert!(empty.clear_on_shrink(&env));
    // explicit setting (even false) overrides the env fallback.
    let s = EffectiveSettings::from_settings(
        Settings::parse(
            r#"{ "showHardwareCursor": false, "terminal": { "clearOnShrink": false } }"#,
        )
        .unwrap(),
    );
    assert!(!s.show_hardware_cursor(&env));
    assert!(!s.clear_on_shrink(&env));
}

#[test]
fn thinking_budgets_warnings_and_combined_settings() {
    // settings-manager.ts:1043-1045 (thinkingBudgets), :1199-1201 (warnings),
    // :784-789 (branchSummary), :829-835 (providerRetry).
    let s = EffectiveSettings::from_settings(Settings::default());
    assert_eq!(s.thinking_budgets(), None);
    assert_eq!(s.warnings(), Warnings::default());
    assert_eq!(
        s.provider_retry_settings(),
        ProviderRetrySettings {
            timeout_ms: None,
            max_retries: None,
            max_retry_delay_ms: 60000
        }
    );
    assert_eq!(
        s.branch_summary_settings(),
        BranchSummarySettings {
            reserve_tokens: 16384,
            skip_prompt: false
        }
    );

    let s = EffectiveSettings::from_settings(
        Settings::parse(
            r#"{
                "thinkingBudgets": { "low": 100, "high": 9000 },
                "warnings": { "anthropicExtraUsage": false },
                "branchSummary": { "reserveTokens": 2048, "skipPrompt": true },
                "retry": { "provider": { "timeoutMs": 1234, "maxRetries": 7, "maxRetryDelayMs": 999 } }
            }"#,
        )
        .unwrap(),
    );
    assert_eq!(
        s.thinking_budgets(),
        Some(ThinkingBudgets {
            minimal: None,
            low: Some(100),
            medium: None,
            high: Some(9000)
        })
    );
    assert_eq!(
        s.warnings(),
        Warnings {
            anthropic_extra_usage: Some(false)
        }
    );
    assert_eq!(
        s.branch_summary_settings(),
        BranchSummarySettings {
            reserve_tokens: 2048,
            skip_prompt: true
        }
    );
    assert_eq!(
        s.provider_retry_settings(),
        ProviderRetrySettings {
            timeout_ms: Some(1234),
            max_retries: Some(7),
            max_retry_delay_ms: 999,
        }
    );
}

#[test]
fn thinking_budgets_and_warnings_parse_field_wise() {
    // Pi returns these objects raw/loosely-typed (settings-manager.ts:1043-1045, 1199-1201):
    // one malformed field does NOT discard the rest. The prior whole-object `from_value`
    // collapsed the ENTIRE object to None/Default on a single bad field. Assert the surviving
    // valid fields are preserved (Pi behaviour), not lost.
    let s = EffectiveSettings::from_settings(
        Settings::parse(
            r#"{
                "thinkingBudgets": { "minimal": 50, "low": "oops", "medium": 700 },
                "warnings": { "anthropicExtraUsage": "nope" }
            }"#,
        )
        .unwrap(),
    );
    // `low` is a string (malformed) → that field is None, but `minimal` and `medium` survive.
    // The whole-object parse would have yielded `None` for the entire budgets object.
    assert_eq!(
        s.thinking_budgets(),
        Some(ThinkingBudgets {
            minimal: Some(50),
            low: None,
            medium: Some(700),
            high: None
        })
    );
    // `anthropicExtraUsage` is a string (malformed) → that field falls back to None; the object
    // itself is still returned (present key) rather than collapsing.
    assert_eq!(
        s.warnings(),
        Warnings {
            anthropic_extra_usage: None
        }
    );

    // An empty `thinkingBudgets` object is present → `Some(default)`, distinct from unset/None.
    let s2 =
        EffectiveSettings::from_settings(Settings::parse(r#"{ "thinkingBudgets": {} }"#).unwrap());
    assert_eq!(s2.thinking_budgets(), Some(ThinkingBudgets::default()));
}

#[test]
fn enabled_models_distinguishes_unset_from_empty() {
    // Pi `getEnabledModels(): string[] | undefined` (settings-manager.ts:1133-1135): unset is
    // `undefined` (cycle ALL), an explicit `[]` is empty (cycle NONE). The prior
    // `unwrap_or_default` collapsed both to an empty Vec.
    let unset = EffectiveSettings::from_settings(Settings::default());
    assert_eq!(unset.enabled_models(), None);

    let empty =
        EffectiveSettings::from_settings(Settings::parse(r#"{ "enabledModels": [] }"#).unwrap());
    assert_eq!(empty.enabled_models(), Some(vec![]));

    let some = EffectiveSettings::from_settings(
        Settings::parse(r#"{ "enabledModels": ["anthropic/claude-opus-4-8"] }"#).unwrap(),
    );
    assert_eq!(
        some.enabled_models(),
        Some(vec!["anthropic/claude-opus-4-8".to_string()])
    );
}

#[test]
fn compaction_and_retry_combined_getters() {
    // settings-manager.ts:776-782 (getCompactionSettings), :808-814 (getRetrySettings).
    let s = EffectiveSettings::from_settings(Settings::default());
    assert_eq!(
        s.compaction_settings(),
        CompactionSettings {
            enabled: true,
            reserve_tokens: 16384,
            keep_recent_tokens: 20000
        }
    );
    assert_eq!(
        s.retry_settings(),
        RetrySettings {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 2000
        }
    );

    let s = EffectiveSettings::from_settings(
        Settings::parse(
            r#"{
                "compaction": { "enabled": false, "reserveTokens": 100, "keepRecentTokens": 200 },
                "retry": { "enabled": false, "maxRetries": 9, "baseDelayMs": 500 }
            }"#,
        )
        .unwrap(),
    );
    assert_eq!(
        s.compaction_settings(),
        CompactionSettings {
            enabled: false,
            reserve_tokens: 100,
            keep_recent_tokens: 200
        }
    );
    assert_eq!(
        s.retry_settings(),
        RetrySettings {
            enabled: false,
            max_retries: 9,
            base_delay_ms: 500
        }
    );
}

#[test]
fn typed_list_getters() {
    // settings-manager.ts:953-1031 — getPackages/getExtensionPaths/getSkillPaths/
    // getPromptTemplatePaths/getThemePaths, each with an empty-array default.
    let empty = EffectiveSettings::from_settings(Settings::default());
    assert!(empty.packages().is_empty());
    assert!(empty.extension_paths().is_empty());
    assert!(empty.skill_paths().is_empty());
    assert!(empty.prompt_template_paths().is_empty());
    assert!(empty.theme_paths().is_empty());

    let s = EffectiveSettings::from_settings(
        Settings::parse(
            r#"{
                "packages": ["pkg-a", { "source": "pkg-b", "extensions": ["x.ts"], "themes": ["t"] }],
                "extensions": ["/ext/a"],
                "skills": ["/skill/a", "/skill/b"],
                "prompts": ["/p"],
                "themes": ["/theme/a"]
            }"#,
        )
        .unwrap(),
    );
    assert_eq!(
        s.packages(),
        vec![
            PackageSource::Name("pkg-a".to_string()),
            PackageSource::Detailed {
                source: "pkg-b".to_string(),
                autoload: None,
                extensions: Some(vec!["x.ts".to_string()]),
                skills: None,
                prompts: None,
                themes: Some(vec!["t".to_string()]),
            },
        ]
    );
    assert_eq!(s.extension_paths(), vec!["/ext/a".to_string()]);
    assert_eq!(
        s.skill_paths(),
        vec!["/skill/a".to_string(), "/skill/b".to_string()]
    );
    assert_eq!(s.prompt_template_paths(), vec!["/p".to_string()]);
    assert_eq!(s.theme_paths(), vec!["/theme/a".to_string()]);
}

/// CFG-010 — `autoload` is a real key on the object form (Pi `PackageSource`,
/// settings-manager.ts:79). `PackageSource` is `#[serde(untagged)]` with no
/// `deny_unknown_fields`, so before it was modelled the key deserialized into `Detailed` and was
/// silently discarded — the user's opt-out simply evaporated between settings.json and
/// discovery.
#[test]
fn a_package_entry_carries_its_autoload_flag() {
    let s = EffectiveSettings::from_settings(
        Settings::parse(
            r#"{"packages": [
                 "plain",
                 { "source": "opted-out", "autoload": false },
                 { "source": "delta", "autoload": false, "skills": ["skills/a/**"] },
                 { "source": "explicit-on", "autoload": true }
               ]}"#,
        )
        .unwrap(),
    );
    let pkgs = s.packages();
    assert_eq!(
        pkgs.iter().map(PackageSource::autoload).collect::<Vec<_>>(),
        vec![None, Some(false), Some(false), Some(true)],
        "a bare string entry has no autoload; the object form round-trips the flag verbatim"
    );
    assert_eq!(
        pkgs.get(2).map(PackageSource::filters).and_then(|f| f.1),
        Some(["skills/a/**".to_string()].as_slice()),
        "the per-type patterns survive alongside it"
    );
    // Serializing back preserves the key (settings documents round-trip, R-07-004).
    let json = serde_json::to_string(&pkgs).unwrap();
    assert!(json.contains(r#""autoload":false"#), "{json}");
}
