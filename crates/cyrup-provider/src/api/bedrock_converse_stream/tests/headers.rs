//! Custom-header injection (pi `bedrock-custom-headers.test.ts` VC1/VC2/VC3).

use super::*;

#[test]
fn caller_headers_are_injected_but_reserved_ones_are_skipped_case_insensitively() {
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    headers.insert("authorization".to_string(), "real-auth".to_string());
    headers.insert("x-amz-date".to_string(), "real-date".to_string());
    headers.insert("host".to_string(), "real-host".to_string());

    let caller: HeaderMap = [
        ("authorization", Some("evil")),
        ("x-amz-date", Some("evil")),
        ("x-allowed", Some("ok")),
        ("Authorization", Some("evil2")),
        ("X-Amz-Date", Some("evil2")),
        ("HOST", Some("evil3")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
    .collect();

    apply_custom_headers(&mut headers, Some(&caller), None);

    assert_eq!(
        headers.get("authorization").map(String::as_str),
        Some("real-auth")
    );
    assert_eq!(
        headers.get("x-amz-date").map(String::as_str),
        Some("real-date")
    );
    assert_eq!(headers.get("host").map(String::as_str), Some("real-host"));
    assert_eq!(headers.get("x-allowed").map(String::as_str), Some("ok"));
    // No mixed-case leak (pi's VC2 key-set assertion).
    let keys: Vec<&str> = headers.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec!["authorization", "host", "x-allowed", "x-amz-date"]
    );
}

#[test]
fn no_caller_headers_changes_nothing() {
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    let before = headers.clone();
    apply_custom_headers(&mut headers, None, None);
    apply_custom_headers(&mut headers, Some(&HeaderMap::new()), None);
    assert_eq!(headers, before);
}
