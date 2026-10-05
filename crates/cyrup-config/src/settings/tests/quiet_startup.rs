//! `quietStartup` is a tri-state — `true | false | "header"` (`settings-manager.ts:112`, `:150`
//! @v1.0.0) — not a boolean: the read rule (`:1089-1092`), the write (`:1094-1098`), the two
//! decisions it feeds (`interactive-mode.ts:1409-1417`) and the `/settings` row's value text.

use std::sync::Arc;

use crate::settings::*;

fn eff(json: &str) -> EffectiveSettings {
    EffectiveSettings::from_settings(Settings::parse(json).unwrap())
}

/// `getQuietStartup`: `value === true || value === "header" ? value : false`. Only the boolean
/// `true` and the exact string `"header"` are recognised; every other value, of any type, and an
/// absent key read as `false`.
#[test]
fn read_rule_recognises_only_true_and_the_exact_string_header() {
    let read = |json: &str| eff(json).quiet_startup();

    assert_eq!(
        read("{}"),
        QuietStartup::Off,
        "absent key defaults to false"
    );
    assert_eq!(read(r#"{"quietStartup":true}"#), QuietStartup::On);
    assert_eq!(read(r#"{"quietStartup":false}"#), QuietStartup::Off);
    assert_eq!(read(r#"{"quietStartup":"header"}"#), QuietStartup::Header);
    for unrecognised in [
        r#""HEADER""#,
        r#""Header""#,
        r#"" header""#,
        r#""true""#,
        r#""false""#,
        r#""""#,
        r#""full""#,
        "null",
        "1",
        "0",
        "[]",
        r#"["header"]"#,
        "{}",
    ] {
        assert_eq!(
            read(&format!(r#"{{"quietStartup":{unrecognised}}}"#)),
            QuietStartup::Off,
            "{unrecognised} reads as false"
        );
    }
}

/// `this.settings` is the merged view, so a project value replaces the global one — including an
/// unrecognised project value, which reads as `false` and so UN-silences a global `true`.
#[test]
fn project_layer_overrides_global_before_the_read_rule_runs() {
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "quietStartup": true }"#);
    store.seed(SettingsScope::Project, r#"{ "quietStartup": "header" }"#);
    let mgr = SettingsManager::load(store.clone(), true);
    assert_eq!(mgr.effective().quiet_startup(), QuietStartup::Header);

    store.seed(SettingsScope::Project, r#"{ "quietStartup": "bogus" }"#);
    let mgr = SettingsManager::load(store, true);
    assert_eq!(mgr.effective().quiet_startup(), QuietStartup::Off);
}

/// `shouldShowStartupHeader` (`verbose || quietStartup !== true`) and `shouldShowStartupDetails`
/// (`verbose || quietStartup === false`), over every value and both `--verbose` states.
#[test]
fn header_and_details_decisions_match_pi_for_every_value() {
    use QuietStartup::{Header, Off, On};
    // (value, verbose, header, details)
    let table = [
        (Off, false, true, true),
        (On, false, false, false),
        (Header, false, true, false),
        (Off, true, true, true),
        (On, true, true, true),
        (Header, true, true, true),
    ];
    for (value, verbose, header, details) in table {
        assert_eq!(
            value.shows_header(verbose),
            header,
            "{value} verbose={verbose}: header"
        );
        assert_eq!(
            value.shows_details(verbose),
            details,
            "{value} verbose={verbose}: details"
        );
    }
}

/// The `/settings` row: `String(config.quietStartup)`, values `["true", "header", "false"]`, and
/// `newValue === "header" ? "header" : newValue === "true"` on the way back.
#[test]
fn row_value_text_round_trips_over_the_rows_own_values() {
    assert_eq!(QuietStartup::ROW_VALUES, ["true", "header", "false"]);
    for text in QuietStartup::ROW_VALUES {
        assert_eq!(QuietStartup::from_row_value(text).to_string(), text);
    }
    assert_eq!(QuietStartup::from_row_value("garbage"), QuietStartup::Off);
    assert_eq!(QuietStartup::default().to_string(), "false");
}

/// `setQuietStartup` stores the boolean for `On`/`Off` and the STRING for `Header`, GLOBAL scope,
/// and an unrelated key survives every write.
#[tokio::test]
async fn set_writes_a_bool_or_the_header_string() {
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "theme": "dark" }"#);
    let mut mgr = SettingsManager::load(store.clone(), false);
    let stored = |store: &InMemorySettingsStore| {
        Settings::parse(&store.read(SettingsScope::Global).unwrap().unwrap())
            .unwrap()
            .get("quietStartup")
            .cloned()
    };

    mgr.set_quiet_startup(QuietStartup::Header).await.unwrap();
    assert_eq!(stored(&store), Some(serde_json::json!("header")));
    assert_eq!(mgr.effective().quiet_startup(), QuietStartup::Header);

    mgr.set_quiet_startup(QuietStartup::On).await.unwrap();
    assert_eq!(stored(&store), Some(serde_json::json!(true)));
    assert_eq!(mgr.effective().quiet_startup(), QuietStartup::On);

    mgr.set_quiet_startup(QuietStartup::Off).await.unwrap();
    assert_eq!(stored(&store), Some(serde_json::json!(false)));
    assert_eq!(mgr.effective().quiet_startup(), QuietStartup::Off);

    let s = Settings::parse(&store.read(SettingsScope::Global).unwrap().unwrap()).unwrap();
    assert_eq!(s.get("theme"), Some(&serde_json::json!("dark")));
}

/// A `settings.json` pi wrote with `"quietStartup": "header"` — or with a value this build does not
/// recognise — survives a cyrup read-modify-write of ANOTHER key byte for byte: the only change is
/// the one key that was set.
#[tokio::test]
async fn pi_written_header_survives_an_unrelated_read_modify_write_byte_for_byte() {
    for quiet in [r#""header""#, r#""bogus""#, "true", "null"] {
        let before = format!("{{\n  \"quietStartup\": {quiet},\n  \"theme\": \"dark\"\n}}\n");
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, &before);
        let mut mgr = SettingsManager::load(store.clone(), false);

        mgr.set(SettingsScope::Global, "defaultThinkingLevel", "low")
            .await
            .unwrap();

        let after = store.read(SettingsScope::Global).unwrap().unwrap();
        let expected = format!(
            "{{\n  \"quietStartup\": {quiet},\n  \"theme\": \"dark\",\n  \"defaultThinkingLevel\": \"low\"\n}}\n"
        );
        assert_eq!(
            after, expected,
            "quietStartup {quiet} must be written back untouched"
        );
    }
}
