/// TUI-063 — `/share`'s viewer link, the only consumer of the `CYRUP_SHARE_VIEWER_URL` that
/// `cyrup --help` advertises at `crates/cyrup/src/cli.rs:1077`.
///
/// The env-var half lives in `tests/share_viewer_url.rs` (its own binary): `std::env::set_var` is
/// `unsafe` in edition 2024 and this crate is `#![forbid(unsafe_code)]`, the same split
/// `experimental_features_enabled_from` + `tests/experimental_marker.rs` already uses.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod share_viewer_url_tests {
    use crate::app::*;

    /// `gistUrl?.split("/").pop()` (`interactive-mode.ts:5599` @v0.83.0) over what `gh gist create`
    /// actually prints, plus the two shapes pi's `if (!gistId)` guard is testing for.
    #[test]
    fn the_gist_id_is_the_last_path_segment_of_gh_s_output() {
        assert_eq!(gist_id_from_url("https://gist.github.com/octocat/abc123def456"), "abc123def456");
        // JS `"abc".split("/")` is `["abc"]`, so `pop()` yields the whole string.
        assert_eq!(gist_id_from_url("abc123def456"), "abc123def456");
        // The two failures `if (!gistId)` catches: nothing on stdout, and a trailing separator.
        assert_eq!(gist_id_from_url(""), "");
        assert_eq!(gist_id_from_url("https://gist.github.com/octocat/"), "");
    }

    /// `${baseUrl}#${gistId}` with `baseUrl = process.env.PI_SHARE_VIEWER_URL || DEFAULT`
    /// (`config.ts:504-508` @v0.83.0). The default is pi's verbatim — see [`DEFAULT_SHARE_VIEWER_URL`].
    #[test]
    fn an_unset_or_empty_override_falls_back_to_pi_s_default_base() {
        assert_eq!(
            share_viewer_url_from(None, "abc123"),
            "https://pi.dev/session/#abc123",
            "`DEFAULT_SHARE_VIEWER_URL` is `https://pi.dev/session/` (`config.ts:502`)"
        );
        assert_eq!(
            share_viewer_url_from(Some(""), "abc123"),
            "https://pi.dev/session/#abc123",
            "JS `||` treats the empty string as unset — an exported-but-empty variable must not \
             produce a bare `#abc123`"
        );
    }

    /// The point of the item: a set variable REACHES the rendered link. Before this landed, `/share`
    /// printed the gist URL alone and the variable had no reader anywhere in `crates/`.
    #[test]
    fn a_set_override_becomes_the_base_of_the_share_url() {
        assert_eq!(
            share_viewer_url_from(Some("https://viewer.example/s/"), "abc123"),
            "https://viewer.example/s/#abc123"
        );
    }
}

