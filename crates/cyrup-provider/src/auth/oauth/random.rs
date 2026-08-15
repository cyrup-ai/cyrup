//! Cryptographic randomness for login flows: PKCE verifiers, `state` nonces and the per-login
//! callback path.
//!
//! `[CYRUP-DELTA]` **Mechanism, not behaviour.** pi calls the ambient `crypto.getRandomValues`
//! (`ai/src/auth/oauth/pkce.ts:24`) and `crypto.randomUUID()` (`openrouter.ts:246`). Rust has no
//! ambient CSPRNG, so the draw goes through [`ring::rand::SystemRandom`] — already a direct
//! dependency of this crate (see the `ring` entry in `Cargo.toml`, and its second use at
//! `auth/google_adc.rs:383`), so this costs nothing in the dependency graph. The generated
//! *values* have the same shape and the same entropy: 32 random bytes for a verifier, a v4 UUID
//! for the callback path.
//!
//! **PROV-RANDOM.** This module used to carry a `cfg(unix)` / `cfg(not(unix))` attribute pair: the
//! unix arm read `/dev/urandom`, and the other arm ignored its buffer and returned
//! `Entropy("no OS random source is reachable on this platform")` for every input. That arm is
//! genuinely compiled and shipped on Windows, so `/login` could not complete for ANY provider
//! there — every flow dies at [`super::pkce::generate_pkce`]. pi has no platform arm at all
//! (`pkce.ts:19`: "Uses Web Crypto API for cross-platform compatibility"), and neither does this
//! module any more. Do not reintroduce a fallback arm: `SystemRandom` reaches the OS CSPRNG on
//! every target this workspace builds for, so there is nothing to fall back *from*, and a login
//! that silently draws weaker entropy is worse than one that refuses.

use super::OAuthError;
use ring::rand::SecureRandom as _;

/// Fill `buf` with cryptographically secure random bytes — `crypto.getRandomValues(buf)`
/// (`pkce.ts:24`).
///
/// One body, no platform branch. [`ring::rand::SystemRandom`] resolves to the OS CSPRNG on every
/// supported target (`getrandom(2)`/`/dev/urandom` on unix, `BCryptGenRandom`/`RtlGenRandom` on
/// Windows) — the same sources Node's `crypto.getRandomValues` ultimately draws from.
pub fn fill_random(buf: &mut [u8]) -> Result<(), OAuthError> {
    ring::rand::SystemRandom::new()
        .fill(buf)
        .map_err(|_| OAuthError::Entropy("OS random source unavailable".to_string()))
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

    /// PROV-RANDOM regression, and the reason it needs a *source* assertion.
    ///
    /// `fill_random` previously had two bodies: `#[cfg(unix)]` read `/dev/urandom` and returned
    /// `Ok(())`, while `#[cfg(not(unix))]` ignored its buffer and returned
    /// `Entropy("no OS random source is reachable on this platform")` for every input. That arm
    /// is genuinely compiled on Windows, so no `/login` flow for ANY provider could produce a
    /// PKCE verifier there. A behavioural test cannot reach it from a unix host — the failing arm
    /// is not compiled here, so `random_bytes(32)` passes either way — which is exactly how the
    /// defect survived. Asserting on the source covers **both** arms from **either** platform.
    ///
    /// If this fails you have reintroduced a platform branch. Don't: see the module note.
    #[test]
    fn fill_random_has_exactly_one_body_and_no_platform_arm() {
        let src = include_str!("random.rs");
        // Everything above the test module, with COMMENT lines stripped. The strip is
        // load-bearing, not tidiness: the module header above describes the arm this replaced and
        // quotes its refusal string verbatim, so scanning the raw text would match the very
        // literal the last assertion forbids and fail against a correct file.
        let code: String = src
            .split("#[cfg(test)]")
            .next()
            .unwrap_or(src)
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("#[cfg("),
            "no cfg attribute may gate this module's code — a platform arm here means some \
             supported target ships an entropy source that differs from every other target"
        );
        assert_eq!(
            code.matches("pub fn fill_random").count(),
            1,
            "fill_random must have exactly one definition"
        );
        // The literal the dead arm returned. Its absence is what makes login work on Windows.
        assert!(
            !code.contains("no OS random source is reachable on this platform"),
            "the unconditional-refusal arm is back"
        );
    }

    /// `fill_random` must actually fill — the refused arm returned without touching its buffer,
    /// so a caller that ignored the `Result` would have base64url'd 32 zero bytes into a PKCE
    /// verifier. Over 8192 draws every byte value appears with overwhelming probability
    /// (a specific value missing has probability `(255/256)^8192` ≈ 1e-14).
    #[test]
    fn fill_random_writes_the_whole_buffer_with_spread_values() {
        let mut buf = [0u8; 8192];
        fill_random(&mut buf).expect("OS CSPRNG must be reachable on every supported platform");
        let mut seen = [false; 256];
        for byte in buf {
            seen[byte as usize] = true;
        }
        assert!(
            seen.iter().all(|s| *s),
            "every byte value must appear across 8192 random bytes"
        );
    }

    /// A zero-length draw is well-defined and succeeds, matching `getRandomValues(new
    /// Uint8Array(0))`.
    #[test]
    fn fill_random_accepts_an_empty_buffer() {
        assert!(fill_random(&mut []).is_ok());
    }

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
