//! PKCE (RFC 7636) verifier/challenge generation.
//!
//! 1:1 port of pi v0.83.0 `packages/ai/src/auth/oauth/pkce.ts`.
//!
//! Divergences, both language-forced, neither observable in the produced values:
//!
//! * `generatePKCE` is `async` upstream (`pkce.ts:21`) only because `crypto.subtle.digest`
//!   returns a promise. [`generate_pkce`] is synchronous.
//! * `base64urlEncode` (`pkce.ts:9-15`) hand-rolls `btoa` + three `replace`s; here the `base64`
//!   crate's `URL_SAFE_NO_PAD` engine produces the identical alphabet-and-padding result.

use super::{OAuthError, random, sha256::sha256};
use base64::Engine as _;

/// The verifier/challenge pair returned by [`generate_pkce`]
/// (`{ verifier: string; challenge: string }`, `pkce.ts:21`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

/// Base64url, unpadded — `base64urlEncode` (`pkce.ts:9-15`: `btoa` then `+`→`-`, `/`→`_`, drop
/// `=`).
pub fn base64url_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The `S256` challenge for an existing verifier: `base64url(sha256(ascii(verifier)))`
/// (`pkce.ts:27-30`). Split out of [`generate_pkce`] so flows can recompute a challenge for a
/// verifier they carried across a process boundary, and so the RFC 7636 Appendix B vector is
/// directly assertable.
pub fn pkce_challenge(verifier: &str) -> String {
    base64url_encode(&sha256(verifier.as_bytes()))
}

/// Generate a PKCE code verifier and its `S256` challenge (`generatePKCE`, `pkce.ts:21-33`):
/// 32 random bytes, base64url-encoded, then SHA-256'd and base64url-encoded again.
pub fn generate_pkce() -> Result<Pkce, OAuthError> {
    let verifier_bytes = random::random_bytes(32)?;
    let verifier = base64url_encode(&verifier_bytes);
    let challenge = pkce_challenge(&verifier);
    Ok(Pkce {
        verifier,
        challenge,
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    /// RFC 7636 Appendix B — the canonical PKCE vector, and the one `crypto.subtle`-backed
    /// `generatePKCE` also satisfies. This is an upstream-independent fixture: if the digest or
    /// the base64url alphabet drifted, this fails.
    #[test]
    fn rfc7636_appendix_b_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    /// `base64urlEncode` replaces `+`→`-` and `/`→`_` and strips `=` (`pkce.ts:14`). These bytes
    /// are chosen to produce all three characters under standard base64 (`+/=`).
    #[test]
    fn base64url_alphabet_matches_the_btoa_replacements() {
        // 0xfb 0xff 0xfe encodes to "+//+" in standard base64 with no padding...
        assert_eq!(base64url_encode(&[0xfb, 0xff, 0xbe]), "-_--");
        // ...and a 1-byte input, which standard base64 would pad with "==".
        assert_eq!(base64url_encode(&[0xff]), "_w");
        assert_eq!(base64url_encode(&[]), "");
    }

    #[test]
    fn generated_verifier_is_43_chars_and_self_consistent() {
        let a = generate_pkce().unwrap();
        // 32 bytes -> 43 unpadded base64url chars, inside RFC 7636's 43..128 legal range.
        assert_eq!(a.verifier.len(), 43);
        assert!(
            a.verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "verifier must be RFC 7636 unreserved: {}",
            a.verifier
        );
        assert_eq!(a.challenge, pkce_challenge(&a.verifier));
        assert_eq!(a.challenge.len(), 43);

        let b = generate_pkce().unwrap();
        assert_ne!(a.verifier, b.verifier, "verifiers must not repeat");
        assert_ne!(a.challenge, b.challenge);
    }
}
