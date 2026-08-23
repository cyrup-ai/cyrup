//! SigV4 (the signing the SDK performs for upstream).

use super::config::AwsCredentials;
use super::url::{uri_encode, url_authority, url_path};
use std::collections::BTreeMap;

/// The SigV4 service name for the Bedrock runtime endpoint.
const SIGV4_SERVICE: &str = "bedrock";

/// HMAC-SHA256 (RFC 2104) over cyrup's dependency-free SHA-256
/// ([`crate::auth::oauth::sha256`], itself written because the crate carries no hashing dependency).
pub(super) fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    use crate::auth::oauth::sha256::sha256;
    const BLOCK: usize = 64;
    let mut padded = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = sha256(key);
        for (dst, src) in padded.iter_mut().zip(digest.iter()) {
            *dst = *src;
        }
    } else {
        for (dst, src) in padded.iter_mut().zip(key.iter()) {
            *dst = *src;
        }
    }
    let mut inner = Vec::with_capacity(BLOCK + message.len());
    let mut outer = Vec::with_capacity(BLOCK + 32);
    for b in padded.iter() {
        inner.push(b ^ 0x36);
        outer.push(b ^ 0x5c);
    }
    inner.extend_from_slice(message);
    let inner_digest = sha256(&inner);
    outer.extend_from_slice(&inner_digest);
    sha256(&outer)
}

/// Lower-case hex.
pub(super) fn hex(bytes: &[u8]) -> String {
    crate::auth::oauth::sha256::hex(bytes)
}

/// Seconds since the Unix epoch (0 on a clock error — never panics).
pub(super) fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `(YYYYMMDD, YYYYMMDDTHHMMSSZ)` for a Unix timestamp — the two SigV4 date forms.
pub(super) fn sigv4_timestamps(epoch_seconds: u64) -> (String, String) {
    let days = (epoch_seconds / 86_400) as i64;
    let secs_of_day = epoch_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    (
        format!("{year:04}{month:02}{day:02}"),
        format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z"),
    )
}

/// Howard Hinnant's `civil_from_days`, the inverse of the `days_from_civil` cyrup already uses in
/// [`crate::utils::http_date`].
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        y + i64::from(m <= 2),
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

/// Sign the request with AWS Signature Version 4 and insert the resulting headers.
///
/// This is what the SDK's signing middleware does for upstream. `x-amz-date`, `host`,
/// `x-amz-content-sha256` and (with temporary credentials) `x-amz-security-token` are added to the
/// map before the canonical request is built, so they are covered by the signature — the same
/// invariant upstream relies on when it forbids callers from overwriting `x-amz-*` / `host` /
/// `authorization` (`is_reserved_header`).
pub(super) fn sign_sigv4(
    headers: &mut BTreeMap<String, String>,
    url: &str,
    body: &[u8],
    creds: &AwsCredentials,
    region: &str,
    epoch_seconds: u64,
) -> Result<(), String> {
    let authority = url_authority(url).ok_or_else(|| format!("invalid Bedrock endpoint: {url}"))?;
    let (date, amz_date) = sigv4_timestamps(epoch_seconds);
    let payload_hash = hex(&crate::auth::oauth::sha256::sha256(body));

    headers.insert("host".to_string(), authority.to_string());
    headers.insert("x-amz-date".to_string(), amz_date.clone());
    headers.insert("x-amz-content-sha256".to_string(), payload_hash.clone());
    if let Some(token) = &creds.session_token {
        headers.insert("x-amz-security-token".to_string(), token.clone());
    }

    // `headers` is a BTreeMap, so iteration is already the lower-cased ascending order SigV4 wants.
    let mut canonical_headers = String::new();
    let mut signed_headers = String::new();
    for (name, value) in headers.iter() {
        canonical_headers.push_str(name);
        canonical_headers.push(':');
        canonical_headers.push_str(value.trim());
        canonical_headers.push('\n');
        if !signed_headers.is_empty() {
            signed_headers.push(';');
        }
        signed_headers.push_str(name);
    }

    // The canonical URI is the request path URI-encoded a SECOND time (every service but S3).
    let canonical_uri = uri_encode(url_path(url), true);
    let canonical_request = format!(
        "POST\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );

    let scope = format!("{date}/{region}/{SIGV4_SERVICE}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&crate::auth::oauth::sha256::sha256(canonical_request.as_bytes()))
    );

    let k_date = hmac_sha256(
        format!("AWS4{}", creds.secret_access_key).as_bytes(),
        date.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, SIGV4_SERVICE.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    headers.insert(
        "authorization".to_string(),
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            creds.access_key_id
        ),
    );
    Ok(())
}
