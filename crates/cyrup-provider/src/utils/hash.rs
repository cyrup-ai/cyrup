//! Fast deterministic string hash (1:1 port of Pi `utils/hash.ts` `shortHash`).
//!
//! Used to shorten long ids into a compact base-36 token (e.g. the openai-responses foreign
//! function-call item id `fc_<shortHash>`). The algorithm mirrors Pi's exactly: two 32-bit lanes
//! mixed with `Math.imul` (32-bit wrapping multiply) over the string's UTF-16 code units, an
//! avalanche finalizer, then `(h2>>>0).toString(36) + (h1>>>0).toString(36)`.

/// Lowercase base-36 of a `u32` (matches JavaScript `Number.prototype.toString(36)`).
fn to_base36(mut n: u32) -> String {
    if n == 0 {
        return "0".to_string();
    }
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out: Vec<u8> = Vec::with_capacity(7); // u32 max in base36 is "1z141z3" (7 chars)
    while n > 0 {
        // `n % 36 < 36`, so `get` is always `Some`.
        out.push(DIGITS.get((n % 36) as usize).copied().unwrap_or(b'0'));
        n /= 36;
    }
    out.reverse();
    // `out` is valid ASCII produced from `DIGITS`.
    String::from_utf8_lossy(&out).into_owned()
}

/// Fast deterministic hash to shorten long strings (Pi `shortHash`, hash.ts:2-12). Iterates the
/// string's UTF-16 code units (matching JS `charCodeAt`) so the digest is byte-1:1 with Pi.
pub fn short_hash(s: &str) -> String {
    let mut h1: u32 = 0xdead_beef;
    let mut h2: u32 = 0x41c6_ce57;
    for ch in s.encode_utf16() {
        let ch = ch as u32;
        h1 = (h1 ^ ch).wrapping_mul(2_654_435_761);
        h2 = (h2 ^ ch).wrapping_mul(1_597_334_677);
    }
    h1 = (h1 ^ (h1 >> 16)).wrapping_mul(2_246_822_507)
        ^ (h2 ^ (h2 >> 13)).wrapping_mul(3_266_489_909);
    h2 = (h2 ^ (h2 >> 16)).wrapping_mul(2_246_822_507)
        ^ (h1 ^ (h1 >> 13)).wrapping_mul(3_266_489_909);
    format!("{}{}", to_base36(h2), to_base36(h1))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn base36_matches_js() {
        assert_eq!(to_base36(0), "0");
        assert_eq!(to_base36(35), "z");
        assert_eq!(to_base36(36), "10");
        assert_eq!(to_base36(u32::MAX), "1z141z3");
    }

    #[test]
    fn short_hash_is_deterministic_and_concatenated() {
        // Deterministic + stable across calls.
        let a = short_hash("hello world");
        let b = short_hash("hello world");
        assert_eq!(a, b);
        // Different inputs differ.
        assert_ne!(short_hash("a"), short_hash("b"));
        // Empty string hashes the seed lanes (non-empty base-36 token).
        assert!(!short_hash("").is_empty());
    }

    /// Cross-checked against the reference JS implementation for fixed inputs (byte-1:1 with Pi).
    #[test]
    fn short_hash_matches_reference_vectors() {
        // Computed with Pi's `shortHash` (hash.ts) for these exact inputs.
        assert_eq!(short_hash(""), "k4n83c7h0j2b");
        assert_eq!(short_hash("a"), "m8735310ae7sx");
        assert_eq!(short_hash("call_1234567890|item_abcdef"), "1l9m5cj1sk7o19");
        assert_eq!(short_hash("hello world"), "n7rb4n1m39uz8");
    }
}
