//! The client `User-Agent` (1:1 port of pi `utils/pi-user-agent.ts` @v0.87.1).
//!
//! Pi `getPiUserAgent()` is `` `pi (${os.platform()} ${os.release()}; ${os.arch()})` `` and seven
//! adapters send it as the LOWEST-precedence header under the model and request overlays
//! (`openai-completions.ts:760`, `openai-responses.ts:248`, `azure-openai-responses.ts:259`,
//! `anthropic-messages.ts:295`, `google-generative-ai.ts:357`, `google-vertex.ts:398`,
//! `mistral-conversations.ts:338`; v0.84.3, #8305). Those seven send it rebranded as `cyrup (…)`
//! ([`default_user_agent`]); `openai-codex-responses` keeps pi's own product token because the
//! ChatGPT backend gates on that client identity (see its `headers.rs`).
//!
//! The platform and arch tokens are Node's (`linux`/`darwin`/`win32`, `x64`/`arm64`), not Rust's
//! `std::env::consts` spellings, so the string is byte-identical to pi's on the same host. Node's
//! `os.release()` is the kernel release from `uname(2)`: it is read from
//! `/proc/sys/kernel/osrelease` on Linux and from `uname -r` on other Unixes. Where neither is
//! available (Windows, or a sandbox that hides both) the release is omitted rather than invented,
//! giving `cyrup (<platform>; <arch>)`.

use crate::HeaderMap;
use std::sync::OnceLock;

/// `"{product} ({platform} {release}; {arch})"` — pi `getPiUserAgent` with `product` in place of
/// `pi`.
pub(crate) fn platform_user_agent(product: &str) -> String {
    match os_release() {
        Some(release) => format!("{product} ({} {release}; {})", node_platform(), node_arch()),
        None => format!("{product} ({}; {})", node_platform(), node_arch()),
    }
}

/// The default `User-Agent` of the seven pi-parity adapters: `cyrup (<platform> <release>; <arch>)`.
pub(crate) fn default_user_agent() -> String {
    platform_user_agent("cyrup")
}

/// Pi `{ "User-Agent": getPiUserAgent(), ...model.headers, ...optionsHeaders }`: the default sits
/// under every overlay. Called AFTER the overlays are merged, so it is inserted only when no
/// overlay named the header in any casing — a caller value overrides it and a caller `None`
/// suppresses it, exactly as a later spread key does upstream.
pub(crate) fn insert_default_user_agent(headers: &mut HeaderMap) {
    if !headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("user-agent"))
    {
        headers.insert("User-Agent".to_string(), Some(default_user_agent()));
    }
}

/// Node `os.platform()` for the running target.
fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        "solaris" | "illumos" => "sunos",
        other => other,
    }
}

/// Node `os.arch()` for the running target.
fn node_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "ia32",
        "aarch64" => "arm64",
        "powerpc" => "ppc",
        "powerpc64" => "ppc64",
        "loongarch64" => "loong64",
        other => other,
    }
}

/// Node `os.release()`, read once per process.
fn os_release() -> Option<&'static str> {
    static RELEASE: OnceLock<Option<String>> = OnceLock::new();
    RELEASE.get_or_init(read_os_release).as_deref()
}

fn read_os_release() -> Option<String> {
    let raw = if cfg!(any(target_os = "linux", target_os = "android")) {
        std::fs::read_to_string("/proc/sys/kernel/osrelease").ok()?
    } else if cfg!(unix) {
        let out = std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8(out.stdout).ok()?
    } else {
        return None;
    };
    let release = raw.trim();
    (!release.is_empty()).then(|| release.to_string())
}

/// Test driver shared by the seven adapters' PROV-095 tests. `build` is the adapter's own header
/// builder run with the given `opts.headers` overlay: with none, the request carries exactly the
/// default under `User-Agent`; with a caller `user-agent` (different casing on purpose), the
/// caller's value is the ONLY user-agent on the request.
#[cfg(test)]
pub(crate) fn assert_default_user_agent_under_overlays(
    build: impl Fn(Option<HeaderMap>) -> HeaderMap,
) {
    let user_agents = |h: &HeaderMap| -> Vec<Option<String>> {
        h.iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
            .map(|(_, v)| v.clone())
            .collect()
    };
    let plain = build(None);
    assert_eq!(
        plain.get("User-Agent"),
        Some(&Some(default_user_agent())),
        "{plain:?}"
    );
    assert_eq!(user_agents(&plain).len(), 1, "{plain:?}");

    let mut overlay = HeaderMap::new();
    overlay.insert("user-agent".to_string(), Some("caller/1.0".to_string()));
    let overridden = build(Some(overlay));
    assert_eq!(
        user_agents(&overridden),
        vec![Some("caller/1.0".to_string())],
        "{overridden:?}"
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn has_pis_shape_with_node_tokens() {
        let ua = platform_user_agent("pi");
        let inner = ua.strip_prefix("pi (").and_then(|s| s.strip_suffix(')'));
        let (left, arch) = inner.unwrap().split_once("; ").unwrap();
        assert_eq!(arch, node_arch());
        assert!(left.starts_with(node_platform()), "{ua}");
        #[cfg(target_os = "linux")]
        {
            let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap();
            assert_eq!(left, format!("linux {}", release.trim()));
            assert!(!ua.contains("x86_64") && !ua.contains("aarch64"), "{ua}");
        }
        assert!(default_user_agent().starts_with("cyrup ("));
    }

    #[test]
    fn an_overlay_in_any_casing_wins_and_none_suppresses() {
        let mut h = HeaderMap::new();
        insert_default_user_agent(&mut h);
        assert_eq!(h.get("User-Agent"), Some(&Some(default_user_agent())));

        let mut h = HeaderMap::new();
        h.insert("user-agent".to_string(), Some("mine/1".to_string()));
        insert_default_user_agent(&mut h);
        assert_eq!(h.len(), 1);
        assert_eq!(h.get("user-agent"), Some(&Some("mine/1".to_string())));

        let mut h = HeaderMap::new();
        h.insert("USER-AGENT".to_string(), None);
        insert_default_user_agent(&mut h);
        assert_eq!(h.len(), 1);
        assert_eq!(h.get("USER-AGENT"), Some(&None));
    }
}
