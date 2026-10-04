//! `fullscreenWheelScrollLines` (TUI-136 / CFG-100): the 1-100 clamp on read AND on write, and the
//! `"auto"` degrade, against `settings-manager.ts:1389-1400` @v1.0.0.

use std::sync::Arc;

use crate::settings::*;

fn eff(json: &str) -> EffectiveSettings {
    EffectiveSettings::from_settings(Settings::parse(json).unwrap())
}

fn lines(n: u8) -> WheelScrollLines {
    WheelScrollLines::Lines(WheelLineCount::from_number(f64::from(n)).unwrap())
}

/// `getFullscreenWheelScrollLines`: a finite number is floored and clamped into `1..=100`;
/// everything else — absent, `null`, a string (even `"5"`), a boolean — is `"auto"`. A hand-edited
/// `500` therefore reads back as `100` instead of being rejected.
///
/// Red before this change: `fullscreen_wheel_scroll_lines` and `WheelScrollLines` did not exist.
#[test]
fn clamps_on_read_and_degrades_to_auto() {
    let read = |json: &str| eff(json).fullscreen_wheel_scroll_lines();

    assert_eq!(read("{}"), WheelScrollLines::Auto, "default is `auto`");
    assert_eq!(
        read(r#"{"fullscreenWheelScrollLines":"auto"}"#),
        WheelScrollLines::Auto
    );
    assert_eq!(read(r#"{"fullscreenWheelScrollLines":3}"#), lines(3));
    assert_eq!(read(r#"{"fullscreenWheelScrollLines":100}"#), lines(100));
    // The CFG-100 obligation: a hand-edited 500 reads back as 100.
    assert_eq!(read(r#"{"fullscreenWheelScrollLines":500}"#), lines(100));
    assert_eq!(read(r#"{"fullscreenWheelScrollLines":0}"#), lines(1));
    assert_eq!(read(r#"{"fullscreenWheelScrollLines":-3}"#), lines(1));
    // `Math.floor`, then clamp.
    assert_eq!(read(r#"{"fullscreenWheelScrollLines":2.7}"#), lines(2));
    assert_eq!(read(r#"{"fullscreenWheelScrollLines":0.4}"#), lines(1));
    // `typeof lines === "number"` is false for every one of these.
    for not_a_number in [r#""5""#, "null", "true", "[3]"] {
        assert_eq!(
            read(&format!(
                r#"{{"fullscreenWheelScrollLines":{not_a_number}}}"#
            )),
            WheelScrollLines::Auto,
            "{not_a_number} reads as auto"
        );
    }
}

/// The write-side clamp is independent of the read-side one (`setFullscreenWheelScrollLines`):
/// `500` is STORED as `100`, not merely read back as it, and `auto` is stored as the string. The
/// row's cycle values (`settings-selector.ts:975`) go through the same constructors.
///
/// Red before this change: `set_fullscreen_wheel_scroll_lines` did not exist.
#[tokio::test]
async fn clamps_on_write_and_round_trips() {
    let store = Arc::new(InMemorySettingsStore::new());
    store.seed(SettingsScope::Global, r#"{ "theme": "dark" }"#);
    let mut mgr = SettingsManager::load(store.clone(), false);
    let stored = |store: &InMemorySettingsStore| {
        Settings::parse(&store.read(SettingsScope::Global).unwrap().unwrap())
            .unwrap()
            .get("fullscreenWheelScrollLines")
            .cloned()
    };

    mgr.set_fullscreen_wheel_scroll_lines(WheelScrollLines::from_number(500.0))
        .await
        .unwrap();
    assert_eq!(stored(&store), Some(serde_json::json!(100)));
    assert_eq!(
        mgr.effective().fullscreen_wheel_scroll_lines().to_string(),
        "100"
    );

    mgr.set_fullscreen_wheel_scroll_lines(WheelScrollLines::from_number(-7.0))
        .await
        .unwrap();
    assert_eq!(stored(&store), Some(serde_json::json!(1)));

    mgr.set_fullscreen_wheel_scroll_lines(WheelScrollLines::from_row_value("5"))
        .await
        .unwrap();
    assert_eq!(stored(&store), Some(serde_json::json!(5)), "a JSON number");

    mgr.set_fullscreen_wheel_scroll_lines(WheelScrollLines::from_row_value("auto"))
        .await
        .unwrap();
    assert_eq!(stored(&store), Some(serde_json::json!("auto")));
    assert_eq!(
        mgr.effective().fullscreen_wheel_scroll_lines(),
        WheelScrollLines::Auto
    );

    // An unrelated key survives every write (R-07-004).
    let s = Settings::parse(&store.read(SettingsScope::Global).unwrap().unwrap()).unwrap();
    assert_eq!(s.get("theme"), Some(&serde_json::json!("dark")));
}

/// The row's display and parse halves (`String(config.fullscreenWheelScrollLines)`, `newValue ===
/// "auto" ? "auto" : parseInt(newValue, 10)`): they are inverses over every value the row offers.
#[test]
fn row_value_text_round_trips() {
    for text in ["auto", "1", "2", "3", "5", "10", "100"] {
        assert_eq!(WheelScrollLines::from_row_value(text).to_string(), text);
    }
    assert_eq!(
        WheelScrollLines::from_row_value("garbage"),
        WheelScrollLines::Auto
    );
    assert_eq!(WheelScrollLines::from_row_value("250").to_string(), "100");
}
