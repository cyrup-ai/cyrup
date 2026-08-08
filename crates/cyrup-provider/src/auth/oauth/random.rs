//! Cryptographic randomness for login flows: PKCE verifiers, `state` nonces and the per-login
//! callback path.
//!
//! **Mechanism divergence.** pi calls the ambient `crypto.getRandomValues`
//! (`ai/src/auth/oauth/pkce.ts:24`) and `crypto.randomUUID()` (`openrouter.ts:246`). Rust has no
//! ambient CSPRNG and `cyrup-provider`'s manifest carries no RNG dependency, so the OS source is
//! read directly. The generated *values* have the same shape and the same entropy: 32 random
//! bytes for a verifier, a v4 UUID for the callback path.

use super::OAuthError;

/// Fill `buf` with cryptographically secure random bytes.
///
/// On unix this reads `/dev/urandom` — the same source `getrandom(2)` serves, and the source
/// Node's `crypto.getRandomValues` ultimately draws from.
#[cfg(unix)]
pub fn fill_random(buf: &mut [u8]) -> Result<(), OAuthError> {
    use std::io::Read;

    let mut file = std::fs::File::open("/dev/urandom")
        .map_err(|e| OAuthError::Entropy(format!("/dev/urandom: {e}")))?;
    file.read_exact(buf)
        .map_err(|e| OAuthError::Entropy(format!("/dev/urandom: {e}")))
}

/// Non-unix builds have no dependency-free CSPRNG reachable from this crate, so login is
/// refused rather than served weak entropy. See this module's `not_done` note: lifting this
/// needs an RNG entry in `cyrup-provider`'s manifest.
#[cfg(not(unix))]
pub fn fill_random(_buf: &mut [u8]) -> Result<(), OAuthError> {
    Err(OAuthError::Entropy(
        "no OS random source is reachable on this platform".to_string(),
    ))
}

/// `n` cryptographically secure random bytes.
pub fn random_bytes(n: usize) -> Result<Vec<u8>, OAuthError> {
    let mut buf = vec![0u8; n];
    fill_random(&mut buf)?;
    Ok(buf)
}

/// A base64url (unpadded) token over `n` random bytes — the shape pi's `state` nonces take.
pub fn random_token(n: usize) -> Result<String, OAuthError> {
    Ok(super::pkce::base64url_encode(&random_bytes(n)?))
}

/// A random v4 UUID, matching `crypto.randomUUID()` (used for the per-login callback path,
/// `openrouter.ts:246`): 8-4-4-4-12 lowercase hex, version nibble `4`, variant bits `10`.
pub fn random_uuid_v4() -> Result<String, OAuthError> {
    let mut bytes = [0u8; 16];
    fill_random(&mut bytes)?;
    if let Some(b) = bytes.get_mut(6) {
        *b = (*b & 0x0f) | 0x40;
    }
    if let Some(b) = bytes.get_mut(8) {
        *b = (*b & 0x3f) | 0x80;
    }

    let mut out = String::with_capacity(36);
    for (i, byte) in bytes.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    Ok(out)
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

    #[test]
    fn random_bytes_are_the_requested_length_and_not_constant() {
        let a = random_bytes(32).unwrap();
        let b = random_bytes(32).unwrap();
        assert_eq!(a.len(), 32);
        assert_eq!(b.len(), 32);
        assert_ne!(a, b, "two draws must not collide");
        assert!(a.iter().any(|byte| *byte != 0), "all-zero draw");
    }

    /// `crypto.randomUUID()` shape: 36 chars, dashes at 8/13/18/23, version 4, RFC 4122 variant.
    #[test]
    fn uuid_v4_matches_crypto_random_uuid_shape() {
        let id = random_uuid_v4().unwrap();
        assert_eq!(id.len(), 36);
        let chars: Vec<char> = id.chars().collect();
        for pos in [8usize, 13, 18, 23] {
            assert_eq!(chars[pos], '-', "dash at {pos} in {id}");
        }
        assert_eq!(chars[14], '4', "version nibble in {id}");
        assert!(
            matches!(chars[19], '8' | '9' | 'a' | 'b'),
            "variant nibble in {id}"
        );
        assert!(
            id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
            "hex only in {id}"
        );
        assert!(id.chars().all(|c| !c.is_ascii_uppercase()), "lowercase");
        assert_ne!(id, random_uuid_v4().unwrap());
    }

    #[test]
    fn random_token_is_base64url_without_padding() {
        let token = random_token(32).unwrap();
        assert!(!token.contains('='), "unpadded: {token}");
        assert!(!token.contains('+') && !token.contains('/'), "url-safe");
        assert_eq!(token.len(), 43, "32 bytes -> 43 base64url chars");
    }
}
