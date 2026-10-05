#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::{Value, json};

use super::*;

fn tool_schema(properties: Value) -> Value {
    json!({ "type": "object", "properties": properties })
}

fn document(name: &str, description: &str, properties: Value) -> ToolSearchDocument {
    create_tool_search_document(name, description, &tool_schema(properties), None)
}

fn names(matches: &[ToolSearchMatch]) -> Vec<&str> {
    matches.iter().map(|m| m.name.as_str()).collect()
}

// ---- tool-search.test.ts :: tokenize ----

#[test]
fn tokenize_splits_camel_case_and_snake_case_drops_stop_words_and_folds_plurals() {
    assert_eq!(
        tokenize("listIssues for the GitHub_repo"),
        ["list", "issue", "git", "hub", "repo"]
    );
    assert_eq!(
        tokenize("searches queries HTTPServer"),
        ["search", "query", "http", "server"]
    );
}

// ---- tool-search.test.ts :: Bm25Ranker ----

fn fixture() -> Vec<ToolSearchDocument> {
    vec![
        document(
            "mcp__github__list_issues",
            "List issues in a repository.",
            json!({"state": {"type": "string", "description": "open or closed"}}),
        ),
        document(
            "mcp__github__create_pull_request",
            "Open a pull request.",
            json!({}),
        ),
        document(
            "mcp__linear__search_issues",
            "Search Linear issues by text.",
            json!({}),
        ),
        document("mcp__docs__search", "Search the documentation.", json!({})),
    ]
}

#[test]
fn ranks_by_term_relevance_and_respects_the_limit() {
    let documents = fixture();
    let ranker = Bm25Ranker::default();
    assert_eq!(
        names(&ranker.rank("issue", &documents, 8)),
        ["mcp__linear__search_issues", "mcp__github__list_issues"]
    );
    assert_eq!(
        ranker.rank("pull requests", &documents, 8)[0].name,
        "mcp__github__create_pull_request"
    );
    assert_eq!(ranker.rank("search", &documents, 1).len(), 1);
    // Property names and descriptions are searchable.
    assert_eq!(
        names(&ranker.rank("closed", &documents, 8)),
        ["mcp__github__list_issues"]
    );
}

#[test]
fn returns_nothing_for_unknown_or_empty_queries() {
    let documents = fixture();
    let ranker = Bm25Ranker::default();
    assert!(ranker.rank("kubernetes", &documents, 8).is_empty());
    assert!(ranker.rank("the", &documents, 8).is_empty());
    // Known v1 limit: no synonyms, so "tickets" does not find "issues".
    assert!(ranker.rank("tickets", &documents, 8).is_empty());
    assert!(ranker.rank("", &documents, 8).is_empty());
    assert!(ranker.rank("issue", &[], 8).is_empty());
    assert!(ranker.rank("issue", &documents, 0).is_empty());
}

#[test]
fn includes_the_namespace_in_the_search_text() {
    let namespace = ToolNamespace {
        name: "mcp__x".into(),
        description: Some("Kubernetes cluster tools".into()),
        instructions: None,
    };
    let document = create_tool_search_document(
        "mcp__x__run",
        "Run it.",
        &tool_schema(json!({})),
        Some(&namespace),
    );
    let matches = Bm25Ranker::default().rank("kubernetes", &[document], 8);
    assert_eq!(names(&matches), ["mcp__x__run"]);
    assert!(matches[0].score > 0.0);
}

// ---- the arithmetic and the tie-break ----

#[test]
fn scores_are_the_okapi_bm25_of_the_fixture() {
    // One document, one query term: idf = ln(1 + (1 - 1 + 0.5) / (1 + 0.5)); the document is the
    // average length, so norm = k1 and the score is idf * (tf * (k1 + 1)) / (tf + k1).
    let documents = [ToolSearchDocument {
        name: "only".into(),
        text: "alpha beta beta".into(),
    }];
    let matches = Bm25Ranker::default().rank("beta", &documents, 8);
    let idf = (1.0_f64 + 0.5 / 1.5).ln();
    let expected = idf * ((2.0 * 2.2) / (2.0 + 1.2));
    assert!(
        (matches[0].score - expected).abs() < 1e-12,
        "{} vs {expected}",
        matches[0].score
    );
}

#[test]
fn ties_keep_document_order() {
    let documents: Vec<ToolSearchDocument> = ["zeta", "alpha", "mid"]
        .into_iter()
        .map(|name| ToolSearchDocument {
            name: name.into(),
            text: "shared term".into(),
        })
        .collect();
    let matches = Bm25Ranker::default().rank("term", &documents, 8);
    assert_eq!(names(&matches), ["zeta", "alpha", "mid"]);
    assert!(
        matches
            .windows(2)
            .all(|pair| pair[0].score == pair[1].score)
    );
}

#[test]
fn a_higher_score_beats_document_order() {
    let documents = [
        ToolSearchDocument {
            name: "weak".into(),
            text: "term padding padding padding padding".into(),
        },
        ToolSearchDocument {
            name: "strong".into(),
            text: "term term".into(),
        },
    ];
    assert_eq!(
        names(&Bm25Ranker::default().rank("term", &documents, 8)),
        ["strong", "weak"]
    );
}

#[test]
fn repeated_query_terms_count_once() {
    let documents = fixture();
    let ranker = Bm25Ranker::default();
    assert_eq!(
        ranker.rank("issue issues Issue", &documents, 8),
        ranker.rank("issue", &documents, 8)
    );
}

#[test]
fn k1_and_b_are_configurable() {
    let documents = [
        ToolSearchDocument {
            name: "short".into(),
            text: "term".into(),
        },
        ToolSearchDocument {
            name: "long".into(),
            text: "term a1 a2 a3 a4 a5 a6 a7".into(),
        },
    ];
    let with_length_norm = Bm25Ranker::new(1.2, 0.75).rank("term", &documents, 8);
    let without = Bm25Ranker::new(1.2, 0.0).rank("term", &documents, 8);
    assert!(with_length_norm[0].score > with_length_norm[1].score);
    assert_eq!(without[0].score, without[1].score);
}

// ---- tokenizer details ----

#[test]
fn tokenize_stems_like_upstream() {
    assert_eq!(
        tokenize(
            "ties dies bus gas glass classes boxes buses matches wishes waxes fizzes series uses"
        ),
        [
            "tie", "die", "bus", "gas", "glass", "class", "box", "buse", "match", "wish", "wax",
            "fizz", "sery", "use"
        ]
    );
}

#[test]
fn tokenize_case_splitting() {
    assert_eq!(tokenize("XMLHttpRequest"), ["xml", "http", "request"]);
    assert_eq!(tokenize("a1B2"), ["a1", "b2"]);
    assert_eq!(tokenize("ABCD"), ["abcd"]);
    assert_eq!(tokenize("Ab AAb"), ["ab", "ab"]);
    assert_eq!(tokenize("AAbC"), ["ab", "c"]);
    assert_eq!(tokenize("HTML5Parser"), ["html5", "parser"]);
    assert_eq!(tokenize("naïve 😀emoji"), ["na", "ve", "emoji"]);
    assert_eq!(tokenize("İstanbul"), ["i", "stanbul"]);
}

// ---- documents ----

#[test]
fn a_document_holds_name_spaced_name_description_schema_text_and_namespace() {
    let schema = json!({
        "type": "object",
        "description": "Top.",
        "properties": {
            "b": {"type": "string", "description": "Bee."},
            "2": {"type": "array", "items": {"description": "Item."}},
            "a": {"anyOf": [{"description": "Left."}, {"properties": {"deep": {"description": "Deep."}}}]},
            "o": {"oneOf": [{"description": "Either."}]},
            "l": {"allOf": [{"description": "Both."}]},
            "t": {"items": [{"description": "Ignored tuple item."}]},
        },
    });
    let namespace = ToolNamespace {
        name: "mcp__ns".into(),
        description: Some("   ".into()),
        instructions: Some("How.".into()),
    };
    let document = create_tool_search_document("a_b", "Does it.", &schema, Some(&namespace));
    assert_eq!(document.name, "a_b");
    // `Object.keys` order: the integer-like `2` first. Blank namespace description is dropped.
    assert_eq!(
        document.text,
        "a_b a b Does it. Top. 2 Item. b Bee. a Left. deep Deep. o Either. l Both. t mcp__ns How."
    );
}

#[test]
fn blank_parts_are_dropped_with_the_javascript_trim() {
    let document = create_tool_search_document("n", "\u{feff}\u{a0}", &Value::Null, None);
    assert_eq!(document.text, "n n");
    let nel = create_tool_search_document("n", "\u{85}", &Value::Null, None);
    assert_eq!(nel.text, "n n \u{85}");
}

// ---- byte parity with upstream's tool-search/tool.ts @v1.0.1 ----
//
// The expected values in `testdata/` are what upstream's `tokenize`, `createToolSearchDocument` and
// `Bm25Ranker` returned for the same inputs, scores as the doubles V8 computed.

#[test]
fn tokenize_agrees_with_upstream_on_the_corpus() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../testdata/tokenize.json")).unwrap();
    assert!(cases.len() >= 200);
    for case in &cases {
        let text = case["text"].as_str().unwrap();
        let want: Vec<&str> = case["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert_eq!(tokenize(text), want, "{text:?}");
    }
}

#[test]
fn ranking_agrees_with_upstream_on_the_corpus_including_scores() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("../../testdata/rank.json")).unwrap();
    let mut ranked = 0;
    for case in &cases {
        let mut documents = Vec::new();
        for (tool, want) in case["tools"]
            .as_array()
            .unwrap()
            .iter()
            .zip(case["docs"].as_array().unwrap())
        {
            let namespace = tool
                .get("namespace")
                .filter(|n| !n.is_null())
                .map(|n| ToolNamespace {
                    name: n["name"].as_str().unwrap().to_owned(),
                    description: n
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    instructions: n
                        .get("instructions")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            let document = create_tool_search_document(
                tool["name"].as_str().unwrap(),
                tool["description"].as_str().unwrap(),
                &tool["parameters"],
                namespace.as_ref(),
            );
            assert_eq!(document.text, want["text"].as_str().unwrap(), "{tool}");
            assert_eq!(document.name, want["name"].as_str().unwrap());
            documents.push(document);
        }
        for result in case["results"].as_array().unwrap() {
            let query = result["query"].as_str().unwrap();
            let limit = usize::try_from(result["limit"].as_u64().unwrap()).unwrap();
            let actual = Bm25Ranker::default().rank(query, &documents, limit);
            let want = result["matches"].as_array().unwrap();
            assert_eq!(actual.len(), want.len(), "{query:?}");
            for (actual, want) in actual.iter().zip(want) {
                assert_eq!(actual.name, want["name"].as_str().unwrap(), "{query:?}");
                // Exact: the same double, not within a tolerance.
                assert_eq!(
                    actual.score,
                    want["score"].as_f64().unwrap(),
                    "{query:?} {}",
                    actual.name
                );
            }
            ranked += want.len();
        }
    }
    assert!(ranked > 100, "{ranked}");
}
