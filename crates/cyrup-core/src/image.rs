//! The cache-safe image resize profile, and its default.
//!
//! This is Pi's `ImageResizeOptions` (`packages/coding-agent/src/utils/image-resize-core.ts:4-9`
//! @v1.0.4) and `ModelImageResizeOptions` (`packages/ai/src/types.ts:1075-1081` @v1.0.4), which are
//! the SAME four keys on both sides of upstream's own split: the TYPE lives in `packages/ai` beside
//! the model catalog, the RESIZER that consumes it lives in `packages/coding-agent/src/utils`.
//!
//! It is homed in `cyrup-core` rather than in `cyrup-provider` for one structural reason: the
//! resizer lives in `cyrup-tools` (whose only cyrup dependency is `cyrup-core`), while the catalog
//! shape that declares the profile lives in `cyrup-provider`. `cyrup-core` is the only crate below
//! BOTH, so homing it here lets one type serve as the catalog field and the resizer parameter with
//! no new dependency edge and no second four-field struct to keep in sync. `cyrup-provider`
//! re-exports it at its original paths (`cyrup_provider::ModelImageResizeOptions`,
//! `cyrup_provider::DEFAULT_IMAGE_RESIZE`), so every catalog-side caller is unaffected.

/// A cache-safe resize profile applied before a new image enters conversation history (Pi
/// `ModelImageResizeOptions`, `packages/ai/src/types.ts:1075-1081` @v1.0.4).
///
/// Every key is optional and resolves INDEPENDENTLY against the shared default profile — upstream
/// spreads `{ ...DEFAULT_OPTIONS, ...options }` (`image-resize-core.ts:64` @v1.0.4), so a row that
/// names only `maxBytes` keeps the 2000px clamps. The defaults are [`DEFAULT_IMAGE_RESIZE`].
///
/// Keeping every field `Option` all the way down to the resizer is load-bearing, not stylistic:
/// upstream merges partial profiles THREE times over (the generator's
/// `{ ...DEFAULT_IMAGE_RESIZE, ...configured }`, the composer's `mergeInputLimits`, and the
/// resizer's `{ ...DEFAULT_OPTIONS, ...options }`), so collapsing to a struct of non-optional
/// fields would zero three keys for a profile that names one.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelImageResizeOptions {
    /// Longest permitted width in pixels. `u32` because that is what every image decoder in the
    /// tree measures in; upstream's schema bounds it only at `>= 1` (`model-config.ts:155-162`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u32>,
    /// Longest permitted height in pixels. Applied as a SECOND, sequential clamp after
    /// [`Self::max_width`] (`image-resize-core.ts:107-115`), so the two are not interchangeable
    /// with one `max_dim`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<u32>,
    /// Maximum BASE64-ENCODED payload size in bytes — the encoded form, not the raw bytes
    /// (upstream's own wording at `types.ts:1078`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// Leading JPEG quality of the re-encode ladder. Upstream builds
    /// `Array.from(new Set([opts.jpegQuality, 85, 70, 55, 40]))` (`image-resize-core.ts:121`), so
    /// this value leads the ladder rather than replacing it. Bounded `1..=100` by upstream's
    /// schema (`model-config.ts:160`), which `u8` covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jpeg_quality: Option<u8>,
}

/// The resize profile every image-capable model resolves to when its row declares none, and the
/// per-key fallback for a row that declares only some keys — Pi `DEFAULT_IMAGE_RESIZE`
/// (`packages/ai/scripts/generate-models.ts:424-430` @v1.0.4).
///
/// These four numbers are byte-for-byte the resizer's own `DEFAULT_OPTIONS`
/// (`packages/coding-agent/src/utils/image-resize-core.ts:24-29` @v1.0.4), which is why a stamped
/// row and an unstamped one resolve the SAME profile. `4_718_592` is `4.5 * 1024 * 1024`.
pub const DEFAULT_IMAGE_RESIZE: ModelImageResizeOptions = ModelImageResizeOptions {
    max_width: Some(2000),
    max_height: Some(2000),
    max_bytes: Some(4_718_592),
    jpeg_quality: Some(80),
};

impl ModelImageResizeOptions {
    /// Resolve this (possibly partial, possibly absent) profile against [`DEFAULT_IMAGE_RESIZE`]
    /// PER KEY — upstream's `{ ...DEFAULT_OPTIONS, ...options }` (`image-resize-core.ts:64`).
    ///
    /// A `None` profile resolves to the defaults in full, which is why an unstamped catalog row,
    /// a user `models.json` row that declares no `inputLimits`, and a CLI boundary with no model
    /// selected yet all resize identically.
    pub fn resolve(profile: Option<&Self>) -> ResolvedResize {
        // `expect`-free: the constant's four fields are `Some` by construction, but reading them
        // through `unwrap_or` keeps this total without an unreachable branch.
        ResolvedResize {
            max_width: profile
                .and_then(|p| p.max_width)
                .or(DEFAULT_IMAGE_RESIZE.max_width)
                .unwrap_or(2000)
                .max(1),
            max_height: profile
                .and_then(|p| p.max_height)
                .or(DEFAULT_IMAGE_RESIZE.max_height)
                .unwrap_or(2000)
                .max(1),
            max_bytes: profile
                .and_then(|p| p.max_bytes)
                .or(DEFAULT_IMAGE_RESIZE.max_bytes)
                .unwrap_or(4_718_592),
            jpeg_quality: profile
                .and_then(|p| p.jpeg_quality)
                .or(DEFAULT_IMAGE_RESIZE.jpeg_quality)
                .unwrap_or(80),
        }
    }
}

/// A fully-resolved resize profile: every key concrete, defaults already applied per key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedResize {
    pub max_width: u32,
    pub max_height: u32,
    pub max_bytes: u64,
    pub jpeg_quality: u8,
}

impl ResolvedResize {
    /// The JPEG quality ladder tried at each dimension step:
    /// `Array.from(new Set([opts.jpegQuality, 85, 70, 55, 40]))` (`image-resize-core.ts:121`).
    ///
    /// The profile's quality LEADS the ladder rather than replacing it, and is de-duplicated — so
    /// the default 80 yields `[80, 85, 70, 55, 40]` (the literal both of cyrup's previous resizers
    /// hardcoded) while a profile naming 40 yields `[40, 85, 70, 55]`, four steps, not five.
    pub fn quality_ladder(&self) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::with_capacity(5);
        for q in [self.jpeg_quality, 85, 70, 55, 40] {
            if !out.contains(&q) {
                out.push(q);
            }
        }
        out
    }
}
