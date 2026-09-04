//! Account id.

use super::*;

#[test]
fn account_id_comes_from_the_namespaced_claim() {
    let token = fake_jwt(&json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": "acct_abc123" },
        "sub": "user_1",
    }));
    assert_eq!(extract_account_id(&token).unwrap(), "acct_abc123");
}

#[test]
fn every_account_id_failure_collapses_to_one_message() {
    const FAILED: &str = "Failed to extract accountId from token";
    // Wrong segment count (:1567).
    assert_eq!(extract_account_id("a.b").unwrap_err(), FAILED);
    assert_eq!(extract_account_id("a.b.c.d").unwrap_err(), FAILED);
    // Undecodable payload.
    assert_eq!(extract_account_id("a.!!!!.c").unwrap_err(), FAILED);
    // Claim absent (:1569-1570).
    assert_eq!(
        extract_account_id(&fake_jwt(&json!({ "sub": "user_1" }))).unwrap_err(),
        FAILED
    );
    // Claim present but empty — falsy in pi's `if (!accountId)`.
    let empty = fake_jwt(&json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": "" },
    }));
    assert_eq!(extract_account_id(&empty).unwrap_err(), FAILED);
    // MIRROR: the same shape with a non-empty id still succeeds, so the assertions above are
    // testing the claim rules and not a permanently-broken decoder.
    let ok = fake_jwt(&json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": "acct_ok" },
    }));
    assert_eq!(extract_account_id(&ok).unwrap(), "acct_ok");
}
