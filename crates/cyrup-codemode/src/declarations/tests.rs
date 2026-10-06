#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::{Value, json};

use super::*;

fn mcp_result_schema(structured_content: Option<Value>) -> Value {
    let mut properties = json!({
        "content": { "type": "array", "items": { "type": "object" } },
        "isError": { "type": "boolean" },
        "_meta": { "type": "object" },
    });
    if let Some(structured) = structured_content {
        properties["structuredContent"] = structured;
    }
    json!({ "type": "object", "properties": properties, "required": ["content"] })
}

fn ty(schema: &Value) -> String {
    schema_to_type(schema, None)
}

// ---- declarations.test.ts :: schemaToType ----

#[test]
fn renders_primitives_literals_and_unions() {
    assert_eq!(ty(&json!({"type": "string"})), "string");
    assert_eq!(ty(&json!({"type": "integer"})), "number");
    assert_eq!(ty(&json!({"type": ["string", "null"]})), "string | null");
    assert_eq!(ty(&json!({"const": "a"})), "\"a\"");
    assert_eq!(ty(&json!({"enum": ["a", 1, null]})), "\"a\" | 1 | null");
    assert_eq!(
        ty(&json!({"anyOf": [{"type": "string"}, {"type": "number"}]})),
        "string | number"
    );
    assert_eq!(ty(&json!({"anyOf": [{"type": "string"}, {}]})), "unknown");
    assert_eq!(
        ty(&json!({"allOf": [{"anyOf": [{"type": "string"}, {"type": "number"}]}, {"const": 1}]})),
        "(string | number) & 1"
    );
    assert_eq!(ty(&json!({"$ref": "#/defs/x"})), "unknown");
    assert_eq!(ty(&json!(true)), "unknown");
    assert_eq!(ty(&json!(false)), "never");
}

#[test]
fn renders_objects_on_one_line_with_sorted_properties() {
    assert_eq!(
        ty(&json!({
            "type": "object",
            "properties": { "city": {"type": "string"}, "max-lines": {"type": "number"} },
            "required": ["city"],
            "additionalProperties": false,
        })),
        "{ city: string; \"max-lines\"?: number; }"
    );
    assert_eq!(
        ty(&json!({"type": "object", "additionalProperties": {"type": "number"}})),
        "{ [key: string]: number; }"
    );
    assert_eq!(
        ty(&json!({"type": "object"})),
        "{ [key: string]: unknown; }"
    );
    assert_eq!(
        ty(&json!({"type": "object", "properties": {}, "additionalProperties": false})),
        "{}"
    );
}

#[test]
fn puts_property_descriptions_on_comment_lines() {
    assert_eq!(
        ty(&json!({
            "type": "object",
            "properties": {
                "weather": {
                    "type": "array",
                    "description": "look up weather for a given list of locations",
                    "items": { "type": "object", "properties": { "location": {"type": "string"} }, "required": ["location"] },
                },
            },
            "required": ["weather"],
        })),
        "{\n  // look up weather for a given list of locations\n  weather: Array<{ location: string; }>;\n}"
    );
    assert_eq!(
        ty(&json!({
            "type": "object",
            "properties": {
                "outer": {
                    "type": "object",
                    "description": "Outer",
                    "properties": { "inner": {"type": "string", "description": "Inner"} },
                },
            },
        })),
        "{\n  // Outer\n  outer?: {\n    // Inner\n    inner?: string;\n  };\n}"
    );
}

#[test]
fn resolves_local_references_and_stops_at_recursive_ones() {
    let schema = json!({
        "type": "object",
        "properties": {
            "item": { "$ref": "#/$defs/Item" },
            "legacy": { "$ref": "#/definitions/Legacy" },
            "remote": { "$ref": "https://example.com/schema.json" },
        },
        "required": ["item"],
        "$defs": {
            "Item": {
                "type": "object",
                "properties": { "id": {"type": "string"}, "parent": {"$ref": "#/$defs/Item"} },
                "required": ["id"],
            },
        },
        "definitions": { "Legacy": {"enum": ["a", "b"]} },
    });
    assert_eq!(
        ty(&schema),
        "{ item: { id: string; parent?: unknown; }; legacy?: \"a\" | \"b\"; remote?: unknown; }"
    );
}

#[test]
fn renders_arrays_and_tuples() {
    assert_eq!(
        ty(&json!({"type": "array", "items": {"type": "string"}})),
        "Array<string>"
    );
    assert_eq!(
        ty(&json!({"type": "array", "prefixItems": [{"type": "string"}, {"type": "number"}]})),
        "[string, number]"
    );
    assert_eq!(ty(&json!({"type": "array"})), "unknown[]");
}

#[test]
fn renders_types_over_the_budget_as_unknown() {
    let properties: serde_json::Map<String, Value> = (0..50)
        .map(|i| (format!("field{i}"), json!({"type": "string"})))
        .collect();
    let schema = json!({"type": "object", "properties": properties});
    assert_eq!(schema_to_type(&schema, Some(100)), "unknown");
    assert!(ty(&schema).contains("field49?: string;"));
}

// ---- declarations.test.ts :: tool declarations ----

#[test]
fn renders_signatures_with_normalized_identifiers() {
    let tool = ToolDeclaration::new("hidden-dynamic-tool")
        .with_input_schema(json!({
            "type": "object",
            "properties": { "city": {"type": "string"} },
            "required": ["city"],
            "additionalProperties": false,
        }))
        .with_output_schema(json!({
            "type": "object", "properties": { "ok": {"type": "boolean"} }, "required": ["ok"]
        }));
    assert_eq!(
        render_tool_signature(&tool, None),
        "hidden_dynamic_tool(args: { city: string; }): Promise<{ ok: boolean; }>;"
    );
    assert_eq!(
        render_tool_signature(&ToolDeclaration::new("free"), None),
        "free(args: unknown): Promise<unknown>;"
    );
}

#[test]
fn renders_mcp_call_tool_result_output_schemas_as_call_tool_result() {
    let input_schema = json!({"type": "object", "properties": {}, "additionalProperties": false});
    let structured = json!({
        "type": "object",
        "properties": { "results": { "type": "array", "items": {"$ref": "#/definitions/Result~1item~0v1"} } },
        "required": ["results"],
        "additionalProperties": false,
        "definitions": {
            "Result/item~v1": {
                "type": "object",
                "properties": { "id": {"type": "string"}, "score": {"type": "number"} },
                "required": ["id", "score"],
                "additionalProperties": false,
            },
        },
    });
    let sample = ToolDeclaration::new("mcp__sample__search")
        .with_input_schema(input_schema.clone())
        .with_output_schema(mcp_result_schema(Some(structured)));
    assert_eq!(
        render_tool_signature(&sample, None),
        "mcp__sample__search(args: {}): Promise<CallToolResult<{ results: Array<{ id: string; score: number; }>; }>>;"
    );
    let plain = ToolDeclaration::new("plain")
        .with_input_schema(input_schema)
        .with_output_schema(mcp_result_schema(None));
    assert_eq!(
        render_tool_signature(&plain, None),
        "plain(args: {}): Promise<CallToolResult>;"
    );
    let not_mcp = json!({"type": "object", "properties": {"content": {"type": "array"}}});
    assert_eq!(mcp_structured_content_schema(Some(&not_mcp)), None);
}

#[test]
fn renders_the_per_tool_sample() {
    let tool = ToolDeclaration::new("foo")
        .with_description("bar")
        .with_input_schema(json!({"type": "string"}));
    assert_eq!(
        render_tool_sample(&tool, None),
        "bar\n\ncodemode tool declaration:\n```ts\ndeclare const tools: { foo(args: string): Promise<unknown>; };\n```"
    );
    // No description: the sample starts with the blank line.
    assert_eq!(
        render_tool_sample(&ToolDeclaration::new("x").with_description("  \n "), None),
        "\n\ncodemode tool declaration:\n```ts\ndeclare const tools: { x(args: unknown): Promise<unknown>; };\n```"
    );
}

// ---- declarations.test.ts :: renderDeclarations ----

#[test]
fn renders_tools_and_globals() {
    let tools = [
        ToolDeclaration::new("read")
            .with_description("Read a file.\nSecond line.")
            .with_input_schema(json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}))
            .with_output_schema(json!({"type": "string"})),
        ToolDeclaration::new("remote-api"),
    ];
    let globals = [ToolDeclaration::new("attach")
        .with_description("Attach it.")
        .with_input_schema(json!({"type": "string"}))];
    assert_eq!(
        render_declarations(&tools, &globals),
        [
            "declare const tools: {",
            "  /**",
            "   * Read a file.",
            "   * Second line.",
            "   */",
            "  read(args: { path: string; }): Promise<string>;",
            "  remote_api(args: unknown): Promise<unknown>;",
            "};",
            "",
            "/** Attach it. */",
            "declare function attach(args: string): Promise<unknown>;",
        ]
        .join("\n")
    );
}

#[test]
fn renders_namespaced_globals_and_explicit_signatures() {
    let globals = [
        ToolDeclaration::new("models.list")
            .with_description("List models.")
            .with_signature("(type: string): Promise<string[]>"),
        ToolDeclaration::new("models.get").with_input_schema(json!({"type": "string"})),
        ToolDeclaration::new("plain").with_signature("(): void"),
    ];
    assert_eq!(
        render_declarations(&[], &globals),
        [
            "declare function plain(): void;",
            "",
            "declare const models: {",
            "  /** List models. */",
            "  list(type: string): Promise<string[]>;",
            "  get(args: string): Promise<unknown>;",
            "};",
        ]
        .join("\n")
    );
}

#[test]
fn escapes_comment_terminators_in_descriptions() {
    let text = render_declarations(&[ToolDeclaration::new("x").with_description("a */ b")], &[]);
    assert!(text.contains("/** a *\\/ b */"), "{text}");
}

// ---- behaviours the upstream tests do not name ----

#[test]
fn nothing_renders_to_the_empty_string() {
    assert_eq!(render_declarations(&[], &[]), "");
}

#[test]
fn doc_comments_keep_blank_lines_and_split_on_crlf() {
    let text = render_declarations(
        &[ToolDeclaration::new("x").with_description("  first\r\n\r\nthird  ")],
        &[],
    );
    assert_eq!(
        text,
        "declare const tools: {\n  /**\n   * first\n   *\n   * third\n   */\n  x(args: unknown): Promise<unknown>;\n};"
    );
}

#[test]
fn namespaces_appear_in_order_of_first_member_after_plain_functions() {
    let globals = [
        ToolDeclaration::new("b.one").with_signature("(): void"),
        ToolDeclaration::new("plain").with_signature("(): void"),
        ToolDeclaration::new("a.one").with_signature("(): void"),
        ToolDeclaration::new("b.two").with_signature("(): void"),
    ];
    let text = render_declarations(&[], &globals);
    assert_eq!(
        text,
        "declare function plain(): void;\n\ndeclare const b: {\n  one(): void;\n  two(): void;\n};\n\ndeclare const a: {\n  one(): void;\n};"
    );
}

#[test]
fn input_types_over_the_default_budget_degrade_to_unknown_but_outputs_do_not() {
    let properties: serde_json::Map<String, Value> = (0..900)
        .map(|i| {
            (
                format!("a_long_property_name_{i}"),
                json!({"type": "string"}),
            )
        })
        .collect();
    let big = json!({"type": "object", "properties": properties});
    assert!(utf16_len_of(&ty(&big)) > DEFAULT_INPUT_SCHEMA_MAX_CHARS);
    let tool = ToolDeclaration::new("t")
        .with_input_schema(big.clone())
        .with_output_schema(big);
    let signature = render_tool_signature(&tool, None);
    assert!(signature.starts_with("t(args: unknown): Promise<{ a_long_property_name_0?: string;"));
    // An explicit budget replaces the default.
    let relaxed = render_tool_signature(&tool, Some(usize::MAX));
    assert!(relaxed.starts_with("t(args: { a_long_property_name_0?: string;"));
}

fn utf16_len_of(text: &str) -> usize {
    text.encode_utf16().count()
}

#[test]
fn the_character_budget_counts_utf16_units_not_bytes() {
    // `"` + 10 x `é` + `"` is 12 UTF-16 units and 22 bytes.
    let schema = json!({"const": "éééééééééé"});
    assert_eq!(schema_to_type(&schema, Some(12)), "\"éééééééééé\"");
    assert_eq!(schema_to_type(&schema, Some(11)), "unknown");
    // An astral character is 2 units: `"` + 6 x U+1F600 + `"` is 14 units, 4 scalars fewer than bytes.
    let astral = json!({"const": "\u{1f600}\u{1f600}\u{1f600}\u{1f600}\u{1f600}\u{1f600}"});
    assert_eq!(schema_to_type(&astral, Some(14)).chars().count(), 8);
    assert_eq!(schema_to_type(&astral, Some(13)), "unknown");
}

#[test]
fn a_schema_expands_at_most_thirty_two_references() {
    let properties: serde_json::Map<String, Value> = (0..40)
        .map(|i| (format!("p{i:02}"), json!({"$ref": "#/$defs/S"})))
        .collect();
    let schema =
        json!({"type": "object", "properties": properties, "$defs": {"S": {"type": "string"}}});
    let rendered = ty(&schema);
    assert_eq!(rendered.matches("?: string;").count(), MAX_REF_EXPANSIONS);
    assert_eq!(
        rendered.matches("?: unknown;").count(),
        40 - MAX_REF_EXPANSIONS
    );
    assert!(rendered.contains("p31?: string;") && rendered.contains("p32?: unknown;"));
}

#[test]
fn the_root_reference_is_recursive_after_one_expansion() {
    let schema =
        json!({"type": "object", "properties": {"child": {"$ref": "#"}}, "required": ["child"]});
    assert_eq!(ty(&schema), "{ child: { child: unknown; }; }");
}

#[test]
fn reference_pointers_decode_and_skip_empty_segments() {
    let schema = json!({
        "type": "object",
        "properties": {
            "a": {"$ref": "#//$defs//A"},
            "b": {"$ref": "#/$defs/%41"},
            "c": {"$ref": "#/$defs/R~1S~0T"},
            "d": {"$ref": "#foo"},
            "e": {"$ref": "#/$defs/A/items"},
            "f": {"$ref": "#/$defs/Number"},
            // Only `#` and `#/...` are pointers; `#x$defs/A` would resolve if the `#` and one more
            // character were merely skipped.
            "g": {"$ref": "#x$defs/A"},
        },
        "$defs": {"A": {"type": "string"}, "R/S~T": {"type": "boolean"}, "Number": 5},
    });
    assert_eq!(
        ty(&schema),
        "{ a?: string; b?: string; c?: boolean; d?: unknown; e?: unknown; f?: unknown; g?: unknown; }"
    );
}

#[test]
fn a_reference_with_malformed_percent_escapes_renders_unknown() {
    let schema = json!({
        "type": "object",
        "properties": {"a": {"$ref": "#/$defs/%E0%A4%A"}, "b": {"$ref": "#/$defs/%"}},
        "$defs": {"A": {"type": "string"}},
    });
    assert_eq!(ty(&schema), "{ a?: unknown; b?: unknown; }");
}

#[test]
fn properties_sort_by_utf16_code_units() {
    let schema = json!({
        "type": "object",
        "properties": {
            "\u{ff5e}": {"type": "string"},
            "\u{1f600}": {"type": "string"},
            "\u{e000}": {"type": "string"},
            "Z": {"type": "string"},
            "a": {"type": "string"},
        },
    });
    // U+1F600 is the surrogate pair D83D DE00, below U+E000 and U+FF5E.
    assert_eq!(
        ty(&schema),
        "{ Z?: string; a?: string; \"\u{1f600}\"?: string; \"\u{e000}\"?: string; \"\u{ff5e}\"?: string; }"
    );
}

#[test]
fn const_and_enum_values_print_as_json_stringify_does() {
    let parse = |text: &str| serde_json::from_str::<Value>(text).unwrap();
    assert_eq!(ty(&parse(r#"{"const": 1.0}"#)), "1");
    assert_eq!(ty(&parse(r#"{"const": 1E21}"#)), "1e+21");
    assert_eq!(ty(&parse(r#"{"const": -0}"#)), "0");
    assert_eq!(ty(&parse(r#"{"const": 1e-7}"#)), "1e-7");
    assert_eq!(
        ty(&parse(r#"{"const": {"b": 1, "2": 2, "a": 3, "1": 4}}"#)),
        "{\"1\":4,\"2\":2,\"b\":1,\"a\":3}"
    );
    assert_eq!(ty(&parse(r#"{"enum": ["a", "a", 1, 1.0]}"#)), "\"a\" | 1");
    assert_eq!(ty(&parse(r#"{"enum": []}"#)), "never");
    assert_eq!(
        ty(&parse(r#"{"enum": ["a", {"anyOf": 1}], "type": "string"}"#)),
        "\"a\" | {\"anyOf\":1}"
    );
}

#[test]
fn type_arrays_and_unrecognised_types() {
    assert_eq!(ty(&json!({"type": ["string", "string"]})), "string");
    assert_eq!(ty(&json!({"type": ["string", {}]})), "unknown");
    assert_eq!(ty(&json!({"type": []})), "never");
    assert_eq!(
        ty(&json!({"type": [["string"], "number"]})),
        "string | number"
    );
    assert_eq!(ty(&json!({"type": "weird"})), "unknown");
    assert_eq!(ty(&json!({"type": null})), "unknown");
    assert_eq!(
        ty(&json!({"type": ["array", "null"], "items": {"type": "string"}})),
        "Array<string> | null"
    );
    // `type` absent: properties/required/additionalProperties mean object, items/prefixItems array.
    assert_eq!(
        ty(&json!({"required": ["a"]})),
        "{ [key: string]: unknown; }"
    );
    assert_eq!(ty(&json!({"items": {"type": "number"}})), "Array<number>");
    assert_eq!(ty(&json!({"description": "only"})), "unknown");
}

#[test]
fn composition_precedence_and_filtering() {
    // anyOf before oneOf before allOf; a non-array anyOf is skipped.
    assert_eq!(
        ty(&json!({"anyOf": [{"type": "string"}], "oneOf": [{"type": "number"}]})),
        "string"
    );
    assert_eq!(
        ty(&json!({"anyOf": "x", "oneOf": [{"type": "number"}]})),
        "number"
    );
    assert_eq!(ty(&json!({"oneOf": []})), "never");
    assert_eq!(ty(&json!({"allOf": []})), "unknown");
    assert_eq!(ty(&json!({"allOf": [{}, true]})), "unknown");
    assert_eq!(
        ty(&json!({"allOf": [{"type": "string"}, {"type": "number"}, false]})),
        "string & number & never"
    );
    // `$ref` wins over everything else on the same schema.
    assert_eq!(ty(&json!({"$ref": "#/nope", "type": "string"})), "unknown");
}

#[test]
fn items_in_every_shape() {
    assert_eq!(
        ty(&json!({"type": "array", "items": null})),
        "Array<unknown>"
    );
    assert_eq!(
        ty(&json!({"type": "array", "items": [{"type": "string"}, {"type": "number"}]})),
        "[string, number]"
    );
    assert_eq!(
        ty(
            &json!({"type": "array", "prefixItems": [{"type": "string"}], "items": [{"type": "number"}]})
        ),
        "[string]"
    );
    assert_eq!(ty(&json!({"type": "array", "items": []})), "unknown[]");
    assert_eq!(
        ty(&json!({"type": "array", "items": true})),
        "Array<unknown>"
    );
}

#[test]
fn objects_with_every_additional_properties_shape() {
    let props = json!({"a": {"type": "string"}});
    let with = |additional: Option<Value>| {
        let mut schema = json!({"type": "object", "properties": props});
        if let Some(additional) = additional {
            schema["additionalProperties"] = additional;
        }
        ty(&schema)
    };
    assert_eq!(with(None), "{ a?: string; }");
    assert_eq!(with(Some(json!(false))), "{ a?: string; }");
    assert_eq!(
        with(Some(json!(true))),
        "{ a?: string; [key: string]: unknown; }"
    );
    assert_eq!(
        with(Some(Value::Null)),
        "{ a?: string; [key: string]: unknown; }"
    );
    assert_eq!(
        with(Some(json!({"type": "number"}))),
        "{ a?: string; [key: string]: number; }"
    );
    // `required` entries that are not strings name nothing.
    assert_eq!(
        ty(&json!({"type": "object", "properties": props, "required": [1, null, "a"]})),
        "{ a: string; }"
    );
}

#[test]
fn description_lines_are_trimmed_and_blank_ones_dropped() {
    let schema = json!({
        "type": "object",
        "properties": {
            "a": {"type": "string", "description": "  one \n\n  two\r\n\u{a0}"},
            "b": {"type": "string", "description": "   "},
            "c": {"type": "string"},
        },
    });
    assert_eq!(
        ty(&schema),
        "{\n  // one\n  // two\n  a?: string;\n  b?: string;\n  c?: string;\n}"
    );
    // Only U+0085 around the text: not whitespace to JavaScript, so it is a description.
    let nel =
        json!({"type": "object", "properties": {"a": {"type": "string", "description": "\u{85}"}}});
    assert_eq!(ty(&nel), "{\n  // \u{85}\n  a?: string;\n}");
}

#[test]
fn the_additional_properties_member_is_not_reindented_as_upstream() {
    let schema = json!({
        "type": "object",
        "properties": {"a": {"type": "string", "description": "A"}},
        "additionalProperties": {"type": "object", "properties": {"x": {"description": "X", "type": "string"}}},
    });
    assert_eq!(
        ty(&schema),
        "{\n  // A\n  a?: string;\n  [key: string]: {\n  // X\n  x?: string;\n};\n}"
    );
}

#[test]
fn only_mcp_results_with_all_four_markers_are_call_tool_results() {
    let base = mcp_result_schema(None);
    assert_eq!(
        mcp_structured_content_schema(Some(&base)),
        Some(&Value::Bool(true))
    );
    assert_eq!(render_tool_output_type(Some(&base)), "CallToolResult");

    let boolean_structured = mcp_result_schema(Some(json!(false)));
    assert_eq!(
        mcp_structured_content_schema(Some(&boolean_structured)),
        Some(&Value::Bool(false))
    );
    assert_eq!(
        render_tool_output_type(Some(&boolean_structured)),
        "CallToolResult<never>"
    );

    for broken in [
        json!({"content": {"type": "string", "items": {"type": "object"}}, "isError": {"type": "boolean"}, "_meta": {"type": "object"}}),
        json!({"content": {"type": "string"}, "isError": {"type": "boolean"}, "_meta": {"type": "object"}}),
        json!({"content": {"type": "array", "items": {"type": "string"}}, "isError": {"type": "boolean"}, "_meta": {"type": "object"}}),
        json!({"content": {"type": "array", "items": {"type": "object"}}, "isError": {"type": "number"}, "_meta": {"type": "object"}}),
        json!({"content": {"type": "array", "items": {"type": "object"}}, "isError": {"type": "boolean"}, "_meta": {"type": "array"}}),
        json!({"content": {"type": "array", "items": {"type": "object"}}, "isError": {"type": "boolean"}}),
    ] {
        let schema = json!({"type": "object", "properties": broken});
        assert_eq!(
            mcp_structured_content_schema(Some(&schema)),
            None,
            "{schema}"
        );
    }
    assert_eq!(mcp_structured_content_schema(Some(&json!(true))), None);
    assert_eq!(mcp_structured_content_schema(None), None);
    assert_eq!(render_tool_output_type(None), "unknown");
}

#[test]
fn the_preamble_is_the_mcp_type_family() {
    assert!(MCP_TYPESCRIPT_PREAMBLE.starts_with("type Role = \"user\" | \"assistant\";\n"));
    assert!(
        MCP_TYPESCRIPT_PREAMBLE
            .contains("type CallToolResult<TStructured = { [key: string]: unknown }> = {")
    );
    assert!(MCP_TYPESCRIPT_PREAMBLE.ends_with("  [key: string]: unknown;\n};"));
    assert_eq!(MCP_TYPESCRIPT_PREAMBLE.lines().count(), 76);
    assert_eq!(MCP_TYPESCRIPT_PREAMBLE.len(), 1543);
}

// ---- byte parity with upstream's declarations.ts @v1.0.1, over generated corpora ----
//
// The expected strings in `testdata/` are what upstream's own functions returned for the same JSON
// texts (parsed with `JSON.parse`).

fn corpus(text: &str) -> Vec<Value> {
    match serde_json::from_str::<Value>(text).unwrap() {
        Value::Array(items) => items,
        other => panic!("not an array: {other}"),
    }
}

fn expected(case: &Value, key: &str) -> Option<String> {
    case[key]
        .get("ok")
        .map(|ok| ok.as_str().unwrap().to_owned())
}

#[test]
fn schema_to_type_agrees_with_upstream_on_the_corpus() {
    let cases = corpus(include_str!("../../testdata/schema_to_type.json"));
    assert!(cases.len() > 400);
    let mut compared = 0;
    let mut upstream_threw = 0;
    for case in &cases {
        let text = case["schema"].as_str().unwrap();
        let schema: Value = serde_json::from_str(text).unwrap();
        let Some(want_type) = expected(case, "type") else {
            // Upstream's `decodeURIComponent` threw `URIError`; only a malformed escape can do that.
            assert!(text.contains('%'), "{text}");
            upstream_threw += 1;
            continue;
        };
        assert_eq!(schema_to_type(&schema, None), want_type, "{text}");
        assert_eq!(
            schema_to_type(&schema, Some(40)),
            expected(case, "budget").unwrap(),
            "budget: {text}"
        );
        assert_eq!(
            render_tool_output_type(Some(&schema)),
            expected(case, "out").unwrap(),
            "output: {text}"
        );
        let tool = ToolDeclaration::new("my-tool")
            .with_input_schema(schema.clone())
            .with_output_schema(schema);
        assert_eq!(
            render_tool_signature(&tool, Some(120)),
            expected(case, "sig").unwrap(),
            "signature: {text}"
        );
        compared += 1;
    }
    assert!(compared > 400, "{compared}");
    assert!(upstream_threw > 0);
}

#[test]
fn mcp_results_agree_with_upstream_on_the_corpus() {
    let cases = corpus(include_str!("../../testdata/mcp.json"));
    assert!(cases.len() >= 80);
    for case in &cases {
        let text = case["schema"].as_str().unwrap();
        let schema: Value = serde_json::from_str(text).unwrap();
        let structured = mcp_structured_content_schema(Some(&schema));
        assert_eq!(
            structured.is_none(),
            case["structuredIsUndefined"].as_bool().unwrap(),
            "{text}"
        );
        assert_eq!(
            serde_json::to_string(&structured).unwrap(),
            serde_json::to_string(
                &serde_json::from_str::<Value>(case["structured"].as_str().unwrap()).unwrap()
            )
            .unwrap(),
            "{text}"
        );
        assert_eq!(
            render_tool_output_type(Some(&schema)),
            case["out"].as_str().unwrap(),
            "{text}"
        );
        let tool = ToolDeclaration::new("mcp__s__t").with_output_schema(schema);
        assert_eq!(
            render_tool_signature(&tool, None),
            case["sig"].as_str().unwrap(),
            "{text}"
        );
    }
}

fn declaration_from(value: &Value) -> ToolDeclaration {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    ToolDeclaration {
        name: text("name").unwrap(),
        description: text("description"),
        input_schema: value.get("inputSchema").cloned(),
        output_schema: value.get("outputSchema").cloned(),
        spread: false,
        signature: text("signature"),
    }
}

#[test]
fn render_declarations_and_samples_agree_with_upstream_on_the_corpus() {
    let cases = corpus(include_str!("../../testdata/render_declarations.json"));
    assert!(cases.len() >= 100);
    let (mut rendered, mut samples) = (0, 0);
    for case in &cases {
        let tools: Vec<ToolDeclaration> = case["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(declaration_from)
            .collect();
        let globals: Vec<ToolDeclaration> = case["globals"]
            .as_array()
            .unwrap()
            .iter()
            .map(declaration_from)
            .collect();
        match expected(case, "render") {
            Some(want) => {
                assert_eq!(render_declarations(&tools, &globals), want, "{case}");
                rendered += 1;
            }
            None => assert!(case.to_string().contains('%'), "{case}"),
        }
        for (tool, want) in tools.iter().zip(case["samples"].as_array().unwrap()) {
            if let Some(want) = want.get("ok") {
                assert_eq!(
                    render_tool_sample(tool, Some(200)),
                    want.as_str().unwrap(),
                    "{tool:?}"
                );
                samples += 1;
            }
        }
    }
    assert!(rendered > 80 && samples > 50, "{rendered} {samples}");
}
