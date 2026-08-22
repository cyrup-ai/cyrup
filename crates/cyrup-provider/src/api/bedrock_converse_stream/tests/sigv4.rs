//! SigV4.

use super::*;

/// RFC 4231 test case 2 — the standard HMAC-SHA256 vector.
#[test]
fn hmac_sha256_matches_rfc_4231() {
    assert_eq!(
        hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    // Case 3: a 20-byte key of 0xaa over 50 bytes of 0xdd.
    assert_eq!(
        hex(&hmac_sha256(&[0xaa; 20], &[0xdd; 50])),
        "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
    );
    // A key longer than the 64-byte block must be hashed first (case 4 of the same RFC uses a
    // 131-byte key).
    assert_eq!(
        hex(&hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}

/// AWS's published signing-key derivation example (Signature Version 4 documentation):
/// secret `wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY`, date `20150830`, region `us-east-1`,
/// service `iam`.
#[test]
fn sigv4_signing_key_derivation_matches_the_aws_example() {
    let k_date = hmac_sha256(
        b"AWS4wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        b"20150830",
    );
    let k_region = hmac_sha256(&k_date, b"us-east-1");
    let k_service = hmac_sha256(&k_region, b"iam");
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    assert_eq!(
        hex(&k_signing),
        "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
    );
}

#[test]
fn sigv4_timestamps_format_both_aws_date_forms() {
    // 2015-08-30T12:36:00Z.
    assert_eq!(
        sigv4_timestamps(1_440_938_160),
        ("20150830".to_string(), "20150830T123600Z".to_string())
    );
    assert_eq!(
        sigv4_timestamps(0),
        ("19700101".to_string(), "19700101T000000Z".to_string())
    );
}

#[test]
fn sigv4_signs_deterministically_and_covers_the_caller_headers() {
    let creds = AwsCredentials {
        access_key_id: "AKIDEXAMPLE".to_string(),
        secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
        session_token: Some("sess".to_string()),
    };
    let url = converse_stream_url(
        "https://bedrock-runtime.us-east-1.amazonaws.com",
        "arn:aws:bedrock:us-east-1:1:application-inference-profile/x",
    );
    // The ARN's `:` and `/` must be percent-encoded in the path.
    assert!(url.ends_with(
        "/model/arn%3Aaws%3Abedrock%3Aus-east-1%3A1%3Aapplication-inference-profile%2Fx/converse-stream"
    ));

    let sign = |extra: Option<(&str, &str)>| {
        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        if let Some((k, v)) = extra {
            headers.insert(k.to_string(), v.to_string());
        }
        sign_sigv4(&mut headers, &url, b"{\"a\":1}", &creds, "us-east-1", 1_440_938_160).unwrap();
        headers
    };

    let base = sign(None);
    assert_eq!(base, sign(None), "signing must be deterministic");
    assert_eq!(
        base.get("x-amz-date").map(String::as_str),
        Some("20150830T123600Z")
    );
    assert_eq!(base.get("x-amz-security-token").map(String::as_str), Some("sess"));
    let auth = base.get("authorization").expect("authorization");
    assert!(auth.starts_with(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/bedrock/aws4_request, SignedHeaders="
    ));
    assert!(auth.contains("content-type;host;x-amz-content-sha256;x-amz-date;x-amz-security-token"));

    // A caller header changes the signature — proving injected headers are covered by it, which
    // is the whole reason upstream registers its middleware at the `build` step.
    let with_extra = sign(Some(("x-allowed", "ok")));
    assert!(with_extra.get("authorization") != base.get("authorization"));
    assert!(
        with_extra
            .get("authorization")
            .expect("authorization")
            .contains("x-allowed")
    );
}

#[test]
fn missing_credentials_are_a_credential_error_not_an_unsigned_request() {
    let config = BedrockClientConfig {
        profile: Some("nope".to_string()),
        region: Some("us-east-1".to_string()),
        endpoint: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
        credentials: None,
        bearer_token: None,
    };
    let mut headers = BTreeMap::new();
    let err = authorize(&mut headers, &config, "https://x/y", b"{}").unwrap_err();
    assert!(err.contains("Could not load credentials"));
    assert!(err.contains("nope"));
    assert!(!headers.contains_key("authorization"));
}
