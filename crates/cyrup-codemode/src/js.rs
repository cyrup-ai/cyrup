//! The JavaScript string and number semantics the ported modules depend on, reproduced once.
//!
//! Each function names the ECMAScript operation it stands for. None of this is an approximation:
//! where Rust's own behaviour differs from JavaScript's (whitespace set, string ordering, number
//! formatting, key order), the difference is exactly what these functions remove.

use std::cmp::Ordering;

use serde_json::{Map, Value};

/// Whether `c` is in `String.prototype.trim`'s set: `WhiteSpace` plus `LineTerminator`
/// (ECMA-262 §12.2, §12.3). That is Unicode `White_Space` without U+0085 (NEL), plus U+FEFF.
#[must_use]
pub fn is_js_whitespace(c: char) -> bool {
    c == '\u{feff}' || (c.is_whitespace() && c != '\u{85}')
}

/// `String.prototype.trim`.
#[must_use]
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// `String.prototype.trimStart`.
#[must_use]
pub fn js_trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_whitespace)
}

/// `String.prototype.length`: UTF-16 code units, not bytes and not scalar values.
#[must_use]
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The default `Array.prototype.sort` comparison for strings: UTF-16 code-unit order. It differs
/// from Rust's byte order for a scalar above U+FFFF against one in U+E000..=U+FFFF.
#[must_use]
pub fn js_string_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `s.split(/\r?\n/)`: split at `\n`, taking a `\r` directly before it along. A `\r` that is not
/// followed by `\n` stays in the text.
#[must_use]
pub fn split_lines(s: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = s.split('\n').collect();
    let last = parts.len().saturating_sub(1);
    for (index, part) in parts.iter_mut().enumerate() {
        if index < last {
            *part = part.strip_suffix('\r').unwrap_or(part);
        }
    }
    parts
}

/// The numeric value of `key` if it is an array index in the sense of ECMA-262 §10.4.2: the
/// canonical decimal form of an integer in `0..2^32 - 1`. Such keys come first in `Object.keys`.
fn array_index(key: &str) -> Option<u64> {
    let bytes = key.as_bytes();
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if bytes.len() > 1 && bytes.first() == Some(&b'0') {
        return None;
    }
    key.parse::<u64>().ok().filter(|n| *n < u64::from(u32::MAX))
}

/// `Object.keys(object)`: integer-like keys in ascending numeric order, then the rest in insertion
/// order. This is what makes iteration order differ from insertion order in JavaScript, and the
/// workspace's `serde_json` `preserve_order` supplies the insertion order.
#[must_use]
pub fn own_keys(map: &Map<String, Value>) -> Vec<&str> {
    let mut indices: Vec<(u64, &str)> = Vec::new();
    let mut named: Vec<&str> = Vec::new();
    for key in map.keys() {
        match array_index(key) {
            Some(n) => indices.push((n, key)),
            None => named.push(key),
        }
    }
    indices.sort_by_key(|&(n, _)| n);
    indices
        .into_iter()
        .map(|(_, key)| key)
        .chain(named)
        .collect()
}

/// `Number.prototype.toString()` for a double (ECMA-262 `Number::toString`).
#[must_use]
pub fn number_to_string(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    // `{:e}` prints the shortest digit string that round-trips, as `d.ddde<exp>`; the spec's `s`
    // (digits) and `n` (decimal point position) come straight from it.
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let k = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    let n = exponent + 1;
    let body = if k <= n && n <= 21 {
        format!(
            "{digits}{}",
            "0".repeat(usize::try_from(n - k).unwrap_or(0))
        )
    } else if 0 < n && n <= 21 {
        let split = usize::try_from(n).unwrap_or(0);
        let (whole, fraction) = digits.split_at(split);
        format!("{whole}.{fraction}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat(usize::try_from(-n).unwrap_or(0)))
    } else {
        let exp = n - 1;
        let exp_sign = if exp < 0 { '-' } else { '+' };
        let mut chars = digits.chars();
        let first = chars.next().unwrap_or('0');
        let rest: String = chars.collect();
        if rest.is_empty() {
            format!("{first}e{exp_sign}{}", exp.abs())
        } else {
            format!("{first}.{rest}e{exp_sign}{}", exp.abs())
        }
    };
    format!("{sign}{body}")
}

/// `JSON.stringify(value)` with no indentation, for a value that came from `JSON.parse`: numbers are
/// doubles formatted by [`number_to_string`], strings are escaped as JSON does (which `serde_json`
/// already matches, including leaving U+2028/U+2029 alone), and object members come out in
/// [`own_keys`] order.
#[must_use]
pub fn json_stringify(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number
            .as_f64()
            .map_or_else(|| "null".to_owned(), number_to_string),
        Value::String(text) => serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned()),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(json_stringify).collect();
            format!("[{}]", items.join(","))
        }
        Value::Object(map) => {
            let members: Vec<String> = own_keys(map)
                .into_iter()
                .filter_map(|key| {
                    let member = map.get(key)?;
                    let key = serde_json::to_string(key).ok()?;
                    Some(format!("{key}:{}", json_stringify(member)))
                })
                .collect();
            format!("{{{}}}", members.join(","))
        }
    }
}

/// `Math.log` as V8 computes it: fdlibm's `__ieee754_log` (`src/base/ieee754.cc` `log`), ported
/// operation for operation. A platform `log` (glibc, or the `libm` crate's current routine) is
/// allowed to differ in the last bit, and BM25 scores built on it then differ from pi's.
// The constants are fdlibm's published decimal expansions, kept as printed so each can be checked
// against `e_log.c`; they round to the same doubles as the shortest forms clippy suggests.
#[allow(clippy::excessive_precision)]
#[must_use]
pub fn math_log(x: f64) -> f64 {
    const LN2_HI: f64 = 6.931_471_803_691_238_164_90e-01;
    const LN2_LO: f64 = 1.908_214_929_270_587_700_02e-10;
    const TWO54: f64 = 1.801_439_850_948_198_400_00e16;
    const LG1: f64 = 6.666_666_666_666_735_130e-01;
    const LG2: f64 = 3.999_999_999_940_941_908e-01;
    const LG3: f64 = 2.857_142_874_366_239_149e-01;
    const LG4: f64 = 2.222_219_843_214_978_396e-01;
    const LG5: f64 = 1.818_357_216_161_805_012e-01;
    const LG6: f64 = 1.531_383_769_920_937_332e-01;
    const LG7: f64 = 1.479_819_860_511_658_591e-01;

    let high_word = |value: f64| (value.to_bits() >> 32) as i32;
    let mut x = x;
    let mut hx = high_word(x);
    let lx = x.to_bits() as u32;
    let mut k: i32 = 0;
    if hx < 0x0010_0000 {
        if (hx & 0x7fff_ffff) as u32 | lx == 0 {
            return f64::NEG_INFINITY;
        }
        if hx < 0 {
            return f64::NAN;
        }
        k -= 54;
        x *= TWO54;
        hx = high_word(x);
    }
    if hx >= 0x7ff0_0000 {
        return x + x;
    }
    k += (hx >> 20) - 1023;
    hx &= 0x000f_ffff;
    let i = (hx + 0x95f64) & 0x0010_0000;
    let normalized_high = (hx | (i ^ 0x3ff0_0000)) as u32;
    x = f64::from_bits((u64::from(normalized_high) << 32) | (x.to_bits() & 0xffff_ffff));
    k += i >> 20;
    let f = x - 1.0;
    if (0x000f_ffff & (2 + hx)) < 3 {
        // |f| < 2**-20
        if f == 0.0 {
            if k == 0 {
                return 0.0;
            }
            let dk = f64::from(k);
            return dk * LN2_HI + dk * LN2_LO;
        }
        let r = f * f * (0.5 - 0.333_333_333_333_333_33 * f);
        if k == 0 {
            return f - r;
        }
        let dk = f64::from(k);
        return dk * LN2_HI - ((r - dk * LN2_LO) - f);
    }
    let s = f / (2.0 + f);
    let dk = f64::from(k);
    let z = s * s;
    let mut i = hx - 0x6147a;
    let w = z * z;
    let j = 0x6b851 - hx;
    let t1 = w * (LG2 + w * (LG4 + w * LG6));
    let t2 = z * (LG1 + w * (LG3 + w * (LG5 + w * LG7)));
    i |= j;
    let r = t2 + t1;
    if i > 0 {
        let hfsq = 0.5 * f * f;
        if k == 0 {
            f - (hfsq - s * (hfsq + r))
        } else {
            dk * LN2_HI - ((hfsq - s * (hfsq + r)) - (dk * LN2_LO + f))
        }
    } else if k == 0 {
        f - s * (f - r)
    } else {
        dk * LN2_HI - ((s * (f - r) - dk * LN2_LO) - f)
    }
}

/// `decodeURIComponent`. `None` where JavaScript throws `URIError`: a `%` not followed by two hex
/// digits, or percent-escapes that do not decode to well-formed UTF-8.
#[must_use]
pub fn decode_uri_component(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let hi = bytes.get(index + 1).and_then(|b| hex_value(*b))?;
            let lo = bytes.get(index + 2).and_then(|b| hex_value(*b))?;
            out.push(hi * 16 + lo);
            index += 3;
        } else {
            out.push(byte);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
