#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::Value;

use super::*;

fn parse(input: &str) -> ParsedCodemodeSource {
    parse_codemode_source(input).unwrap()
}

fn options(max_output_tokens: Option<u64>, timeout_ms: Option<u64>) -> CodemodeSourceOptions {
    CodemodeSourceOptions {
        max_output_tokens,
        timeout_ms,
    }
}

#[test]
fn returns_plain_code_unchanged() {
    assert_eq!(
        parse("text('hi')"),
        ParsedCodemodeSource {
            code: "text('hi')".into(),
            options: options(None, None)
        }
    );
    assert_eq!(
        parse("// just a comment\nreturn 1"),
        ParsedCodemodeSource {
            code: "// just a comment\nreturn 1".into(),
            options: options(None, None)
        }
    );
}

#[test]
fn parses_the_options_line_and_keeps_line_numbers() {
    assert_eq!(
        parse("// @options: {\"timeout_ms\": 10}\nconst a = 1;\ntext(a)"),
        ParsedCodemodeSource {
            code: "\nconst a = 1;\ntext(a)".into(),
            options: options(None, Some(10))
        }
    );
    assert_eq!(
        parse("  // @options:{\"max_output_tokens\":0,\"timeout_ms\":1500}\r\ntext(1)").options,
        options(Some(0), Some(1500))
    );
    assert_eq!(
        parse("// @options: {}\ntext(1)"),
        ParsedCodemodeSource {
            code: "\ntext(1)".into(),
            options: options(None, None)
        }
    );
}

#[test]
fn only_treats_the_first_line_as_an_options_line() {
    let input = "text(1)\n// @options: {\"timeout_ms\": 1}";
    assert_eq!(
        parse(input),
        ParsedCodemodeSource {
            code: input.into(),
            options: options(None, None)
        }
    );
    assert_eq!(
        parse("// @optionsx {}\ntext(1)").options,
        options(None, None)
    );
}

#[test]
fn rejects_empty_input_and_invalid_options() {
    // Upstream's `rejects empty input and invalid options` table, with the variant each message
    // belongs to. `Err(message)` compares the whole text; a `prefix` the start of it.
    enum Expect {
        Whole(&'static str),
        Prefix(&'static str),
    }
    let cases: Vec<(&str, Expect)> = vec![
        (
            "",
            Expect::Prefix("Expected JavaScript source text (non-empty)"),
        ),
        (
            "  \n",
            Expect::Prefix("Expected JavaScript source text (non-empty)"),
        ),
        (
            "// @options:\ntext(1)",
            Expect::Prefix("@options must be a JSON object with supported fields"),
        ),
        (
            "// @options: {timeout_ms: 1}\ntext(1)",
            Expect::Prefix("@options must be valid JSON with supported fields"),
        ),
        (
            "// @options: [1]\ntext(1)",
            Expect::Prefix("@options must be a JSON object with supported fields"),
        ),
        (
            "// @options: {\"yield\": 1}\ntext(1)",
            Expect::Whole(
                "@options only supports `max_output_tokens` and `timeout_ms`; got `yield`",
            ),
        ),
        (
            "// @options: {\"max_output_tokens\": 1.5}\ntext(1)",
            Expect::Whole("@options field `max_output_tokens` must be a non-negative safe integer"),
        ),
        (
            "// @options: {\"timeout_ms\": 0}\ntext(1)",
            Expect::Prefix("@options field `timeout_ms` must be a positive integer"),
        ),
        (
            "// @options: {\"timeout_ms\": 1}",
            Expect::Whole(
                "The @options line must be followed by JavaScript source on subsequent lines",
            ),
        ),
        (
            "// @options: {\"timeout_ms\": 1}\n  \n",
            Expect::Whole(
                "The @options line must be followed by JavaScript source on subsequent lines",
            ),
        ),
    ];
    for (input, expect) in cases {
        let message = parse_codemode_source(input).unwrap_err().to_string();
        match expect {
            Expect::Whole(whole) => assert_eq!(message, whole, "{input:?}"),
            Expect::Prefix(prefix) => assert!(message.starts_with(prefix), "{input:?}: {message}"),
        }
    }
}

#[test]
fn each_refusal_is_its_own_variant() {
    let err = |input: &str| parse_codemode_source(input).unwrap_err();
    assert_eq!(err(""), CodemodeSourceError::EmptySource);
    assert_eq!(err("\u{feff}"), CodemodeSourceError::EmptySource);
    assert_eq!(
        err("// @options: {\"timeout_ms\": 1}"),
        CodemodeSourceError::OptionsWithoutCode
    );
    assert_eq!(
        err("// @options:\nx"),
        CodemodeSourceError::OptionsNotAnObject
    );
    assert_eq!(
        err("// @options: [1]\nx"),
        CodemodeSourceError::OptionsNotAnObject
    );
    assert_eq!(
        err("// @options: null\nx"),
        CodemodeSourceError::OptionsNotAnObject
    );
    assert!(matches!(
        err("// @options: {a: 1}\nx"),
        CodemodeSourceError::OptionsInvalidJson { .. }
    ));
    assert_eq!(
        err("// @options: {\"b\": 1}\nx"),
        CodemodeSourceError::UnsupportedOption { field: "b".into() }
    );
    assert_eq!(
        err("// @options: {\"max_output_tokens\": -1}\nx"),
        CodemodeSourceError::InvalidMaxOutputTokens
    );
    assert_eq!(
        err("// @options: {\"timeout_ms\": 2147483648}\nx"),
        CodemodeSourceError::InvalidTimeoutMs
    );
}

#[test]
fn integers_are_judged_as_javascript_numbers() {
    let ok = |input: &str| parse(input).options;
    // `1e3` and `1.0` are integers in JavaScript, `-0` is `>= 0`.
    assert_eq!(
        ok("// @options: {\"timeout_ms\": 1e3}\nx"),
        options(None, Some(1000))
    );
    assert_eq!(
        ok("// @options: {\"timeout_ms\": 1.0}\nx"),
        options(None, Some(1))
    );
    assert_eq!(
        ok("// @options: {\"max_output_tokens\": -0}\nx"),
        options(Some(0), None)
    );
    assert_eq!(
        ok("// @options: {\"timeout_ms\": 2147483647}\nx"),
        options(None, Some(2_147_483_647))
    );
    assert_eq!(
        ok("// @options: {\"max_output_tokens\": 9007199254740991}\nx"),
        options(Some(9_007_199_254_740_991), None)
    );
    for refused in [
        "{\"max_output_tokens\": 9007199254740992}",
        "{\"max_output_tokens\": null}",
        "{\"max_output_tokens\": \"5\"}",
        "{\"max_output_tokens\": 1e300}",
    ] {
        assert_eq!(
            parse_codemode_source(&format!("// @options: {refused}\nx")).unwrap_err(),
            CodemodeSourceError::InvalidMaxOutputTokens,
            "{refused}"
        );
    }
    assert_eq!(
        parse_codemode_source("// @options: {\"timeout_ms\": -0}\nx").unwrap_err(),
        CodemodeSourceError::InvalidTimeoutMs
    );
}

#[test]
fn unsupported_field_is_the_first_in_object_keys_order() {
    // `Object.keys` lists integer-like keys first, so `"1"` is reported although `b` came first.
    assert_eq!(
        parse_codemode_source("// @options: {\"b\": 1, \"1\": 2}\nx").unwrap_err(),
        CodemodeSourceError::UnsupportedOption { field: "1".into() }
    );
    // An unsupported field is reported before an invalid supported one.
    assert_eq!(
        parse_codemode_source("// @options: {\"max_output_tokens\": -1, \"zzz\": 2}\nx")
            .unwrap_err(),
        CodemodeSourceError::UnsupportedOption {
            field: "zzz".into()
        }
    );
}

#[test]
fn whitespace_is_trimmed_as_javascript_does() {
    // U+FEFF is whitespace to `trim()`; U+0085 is not.
    assert_eq!(
        parse_codemode_source("\u{feff}\u{a0}\n").unwrap_err(),
        CodemodeSourceError::EmptySource
    );
    assert_eq!(parse("\u{85}").code, "\u{85}");
    assert_eq!(parse("\u{feff}// @options: {}\nx").code, "\nx");
    assert_eq!(parse("\u{a0}// @options: {}\nx").code, "\nx");
}

#[test]
fn the_grammar_is_upstreams_byte_for_byte() {
    assert_eq!(
        CODEMODE_SOURCE_GRAMMAR,
        concat!(
            "\n",
            "start: options_source | plain_source\n",
            "options_source: OPTIONS_LINE NEWLINE SOURCE\n",
            "plain_source: SOURCE\n",
            "\n",
            "OPTIONS_LINE: /[ \\t]*\\/\\/ @options:[^\\r\\n]*/\n",
            "NEWLINE: /\\r?\\n/\n",
            "SOURCE: /[\\s\\S]+/\n",
        )
    );
}

#[test]
fn agrees_with_upstream_on_the_corpus() {
    let corpus: Value = serde_json::from_str(include_str!("../../testdata/source.json")).unwrap();
    let cases = corpus.as_array().unwrap();
    assert!(cases.len() > 40);
    for case in cases {
        let input = case["input"].as_str().unwrap();
        let result = &case["result"];
        let actual = parse_codemode_source(input);
        if let Some(ok) = result.get("ok") {
            let parsed = actual.unwrap_or_else(|error| panic!("{input:?}: {error}"));
            assert_eq!(parsed.code, ok["code"].as_str().unwrap(), "{input:?}");
            let upstream = &ok["options"];
            assert_eq!(
                parsed.options.timeout_ms,
                upstream
                    .get("timeoutMs")
                    .map(|v| v.as_f64().unwrap() as u64),
                "{input:?}"
            );
            assert_eq!(
                parsed.options.max_output_tokens,
                upstream
                    .get("maxOutputTokens")
                    .map(|v| v.as_f64().unwrap() as u64),
                "{input:?}"
            );
        } else {
            let expected = result["throws"].as_str().unwrap();
            let message = actual.expect_err(input).to_string();
            // The tail after an invalid-JSON message is V8's own `SyntaxError` text.
            let json_prefix = "@options must be valid JSON with supported fields `max_output_tokens` and `timeout_ms`: ";
            if expected.starts_with(json_prefix) {
                assert!(message.starts_with(json_prefix), "{input:?}: {message}");
            } else {
                assert_eq!(message, expected, "{input:?}");
            }
        }
    }
}
