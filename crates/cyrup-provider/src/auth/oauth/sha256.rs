//! SHA-256, as needed by the PKCE `S256` challenge.
//!
//! **Mechanism divergence.** pi calls `crypto.subtle.digest("SHA-256", data)`
//! (`ai/src/auth/oauth/pkce.ts:29`) — an ambient Web Crypto API that exists in Node and the
//! browser alike. Rust has no ambient crypto, and `cyrup-provider`'s manifest carries no hashing
//! dependency, so the digest is implemented here from FIPS 180-4. The output is byte-identical:
//! the tests pin the two FIPS 180-4 example vectors plus the RFC 7636 PKCE vector, so a
//! divergence from `crypto.subtle` cannot pass.

/// FIPS 180-4 §4.2.2 round constants.
#[rustfmt::skip]
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// FIPS 180-4 §5.3.3 initial hash value.
const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// `w[i]`, written without indexing so the workspace's `clippy::indexing_slicing` deny holds.
#[inline]
fn word(w: &[u32; 64], i: usize) -> u32 {
    w.get(i).copied().unwrap_or(0)
}

/// The SHA-256 digest of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state = H0;

    let mut blocks = data.chunks_exact(64);
    for block in blocks.by_ref() {
        compress(&mut state, block);
    }
    let rest = blocks.remainder();

    // FIPS 180-4 §5.1.1 padding: 0x80, zeroes, then the 64-bit big-endian bit length.
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut tail = [0u8; 128];
    for (dst, src) in tail.iter_mut().zip(rest.iter()) {
        *dst = *src;
    }
    if let Some(marker) = tail.get_mut(rest.len()) {
        *marker = 0x80;
    }
    // One block when the 0x80 marker and the 8-byte length still fit, i.e. `rest.len() <= 55`.
    let total = if rest.len() < 56 { 64 } else { 128 };
    for (dst, src) in tail
        .iter_mut()
        .skip(total - 8)
        .take(8)
        .zip(bit_len.to_be_bytes().iter())
    {
        *dst = *src;
    }
    for block in tail.get(..total).unwrap_or(&[]).chunks_exact(64) {
        compress(&mut state, block);
    }

    let mut out = [0u8; 32];
    for (chunk, word) in out.chunks_exact_mut(4).zip(state.iter()) {
        for (dst, src) in chunk.iter_mut().zip(word.to_be_bytes().iter()) {
            *dst = *src;
        }
    }
    out
}

/// FIPS 180-4 §6.2.2 — one 64-byte block. `block` shorter than 64 bytes is impossible here
/// (every caller feeds a `chunks_exact(64)` item); a short slice would simply zero-extend.
fn compress(state: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for (dst, src) in w.iter_mut().zip(block.chunks_exact(4)) {
        let bytes: [u8; 4] = src.try_into().unwrap_or([0; 4]);
        *dst = u32::from_be_bytes(bytes);
    }
    for i in 16..64 {
        let w15 = word(&w, i - 15);
        let w2 = word(&w, i - 2);
        let s0 = w15.rotate_right(7) ^ w15.rotate_right(18) ^ (w15 >> 3);
        let s1 = w2.rotate_right(17) ^ w2.rotate_right(19) ^ (w2 >> 10);
        let next = word(&w, i - 16)
            .wrapping_add(s0)
            .wrapping_add(word(&w, i - 7))
            .wrapping_add(s1);
        if let Some(slot) = w.get_mut(i) {
            *slot = next;
        }
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (wi, ki) in w.iter().zip(K.iter()) {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(*ki)
            .wrapping_add(*wi);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    for (slot, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h].iter()) {
        *slot = slot.wrapping_add(*add);
    }
}

/// Lowercase hex, for tests and for flows that need a hex digest.
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
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

    /// FIPS 180-4 §D.1.
    #[test]
    fn fips_vector_abc() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// The empty-message vector (NIST CAVP / RFC 6234).
    #[test]
    fn fips_vector_empty() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// FIPS 180-4 §D.2 — a 56-byte message, i.e. the two-block padding path.
    #[test]
    fn fips_vector_two_blocks() {
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    /// One million 'a's — FIPS 180-4 §D.3, exercising many blocks.
    #[test]
    fn fips_vector_million_a() {
        let data = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(&sha256(&data)),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// The padding boundaries: 55 bytes (last message that still fits one block), 56 (first that
    /// needs two), 63 and exactly 64. Digests cross-checked against coreutils `sha256sum`.
    #[test]
    fn padding_boundaries() {
        let cases: [(usize, &str); 4] = [
            (
                55,
                "d5e285683cd4efc02d021a5c62014694958901005d6f71e89e0989fac77e4072",
            ),
            (
                56,
                "04c26261370ee7541549d16dee320c723e3fd14671e66a099afe0a377c16888e",
            ),
            (
                63,
                "75220b47218278e656f2013bb8f0c455a25eaf01e86c64924e9d48d89776d6f2",
            ),
            (
                64,
                "7ce100971f64e7001e8fe5a51973ecdfe1ced42befe7ee8d5fd6219506b5393c",
            ),
        ];
        for (len, expected) in cases {
            assert_eq!(hex(&sha256(&vec![b'x'; len])), expected, "len {len}");
        }
    }
}
