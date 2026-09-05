/// TUI-063 — `/share`'s viewer link, the only consumer of the `CYRUP_SHARE_VIEWER_URL` that
/// `cyrup --help` advertises at `crates/cyrup/src/cli.rs:1077`.
///
/// The env-var half lives in `tests/share_viewer_url.rs` (its own binary): `std::env::set_var` is
/// `unsafe` in edition 2024 and this crate is `#![forbid(unsafe_code)]`, the same split
/// `experimental_features_enabled_from` + `tests/experimental_marker.rs` already uses.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod share_viewer_url_tests {
    use crate::app::*;

    /// `gistUrl?.split("/").pop()` (`interactive-mode.ts:5599` @v0.83.0) over what `gh gist create`
    /// actually prints, plus the two shapes pi's `if (!gistId)` guard is testing for.
    #[test]
    fn the_gist_id_is_the_last_path_segment_of_gh_s_output() {
        assert_eq!(
            gist_id_from_url("https://gist.github.com/octocat/abc123def456"),
            "abc123def456"
        );
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

/// DRIFT-053 — the Radius-first `/share`: the `pi.share` export entry and the three outcomes the
/// upload reports.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod radius_share_tests {
    use crate::app::*;
    use crate::transcript::Entry;
    use crate::{App, UiTheme};
    use cyrup_provider::providers::radius_share::RadiusShareOutcome;
    use ratatui::backend::TestBackend;

    fn new_app() -> App<TestBackend> {
        App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
    }

    fn statuses(app: &App<TestBackend>) -> Vec<String> {
        app.state()
            .transcript
            .pending()
            .iter()
            .filter_map(|e| match e {
                Entry::Status(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn errors(app: &App<TestBackend>) -> Vec<String> {
        app.state()
            .transcript
            .pending()
            .iter()
            .filter_map(|e| match e {
                Entry::Error(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    const JSONL: &str = concat!(
        r#"{"type":"session","version":3,"id":"s1","timestamp":"2026-09-05T00:00:00.000Z","cwd":"/w"}"#,
        "\n",
        r#"{"type":"user","id":"e1","timestamp":"2026-09-05T00:00:01.000Z","content":"hi"}"#,
        "\n",
        r#"{"type":"assistant","id":"e2","timestamp":"2026-09-05T00:00:02.000Z","content":"yo"}"#,
        "\n",
    );

    /// `exportSessionForShare` (`session-share.ts:25-43` @v0.84.4) over `exportSessionToJsonl`'s
    /// two callback arguments (`core/session-export.ts:21`, `:31-36`): the trailing entry's
    /// `parentId` is the LAST exported entry's id and its `timestamp` is the SESSION HEADER's, not
    /// a fresh clock read.
    #[test]
    fn the_share_entry_carries_pis_type_parent_and_timestamp() {
        let params = serde_json::json!({"type": "object"});
        let tools = vec![ShareTool {
            name: "bash",
            description: "run a command",
            parameters: &params,
        }];
        let out = append_share_metadata(JSONL, "abcd1234", "You are cyrup.", &tools);
        assert!(
            out.starts_with(JSONL),
            "the exported document must be preserved byte for byte and only appended to"
        );
        let last: serde_json::Value =
            serde_json::from_str(out.lines().last().unwrap()).expect("trailing entry is JSON");
        assert_eq!(last["type"], "custom");
        assert_eq!(
            last["customType"], "pi.share",
            "`customType` is a wire tag the Radius viewer matches on and must not be rebranded"
        );
        assert_eq!(last["id"], "abcd1234");
        assert_eq!(last["parentId"], "e2", "the last exported entry's id");
        assert_eq!(
            last["timestamp"], "2026-09-05T00:00:00.000Z",
            "`exportSessionToJsonl` hands the callback the HEADER's timestamp"
        );
        assert_eq!(last["data"]["systemPrompt"], "You are cyrup.");
        assert_eq!(last["data"]["tools"][0]["name"], "bash");
        assert_eq!(last["data"]["tools"][0]["description"], "run a command");
        assert_eq!(last["data"]["tools"][0]["parameters"]["type"], "object");
        assert!(out.ends_with('\n'), "`lines.join()` + a trailing newline");
    }

    /// `let parentId: string | null = null;` before the loop (`session-export.ts:31`) — an export
    /// with a header and no entries leaves it `null`, not the header's id.
    #[test]
    fn an_entryless_export_gets_a_null_parent() {
        let header = "{\"type\":\"session\",\"version\":3,\"id\":\"s1\",\"timestamp\":\"T\",\"cwd\":\"/w\"}\n";
        let out = append_share_metadata(header, "id0", "prompt", &[]);
        let last: serde_json::Value = serde_json::from_str(out.lines().last().unwrap()).unwrap();
        assert!(last["parentId"].is_null());
        assert_eq!(last["data"]["tools"].as_array().unwrap().len(), 0);
    }

    /// `context.showStatus(\`Share URL: ${hyperlink(shareUrl, shareUrl)}\`)` (`:139`), and NOT the
    /// gist path's two-line `Share URL: …\nGist: …`.
    #[test]
    fn a_successful_radius_upload_reports_the_canonical_url() {
        let mut app = new_app();
        app.apply_share_outcome(ShareMsg::radius(Ok(RadiusShareOutcome::Shared {
            url: "https://radius.pi.dev/a/xyz".to_string(),
        })));
        assert_eq!(
            statuses(&app),
            vec!["Share URL: https://radius.pi.dev/a/xyz".to_string()]
        );
        assert!(errors(&app).is_empty());
    }

    /// `context.showError(\`Failed to upload Radius artifact: ${…}\`)` (`:133-136`, `:144-146`) —
    /// and the item's own Verify line: a failed Radius upload must NOT fall through to `gh`, which
    /// would publish the gist the Radius configuration exists to avoid.
    #[test]
    fn a_failed_radius_upload_reports_and_does_not_fall_back() {
        let mut app = new_app();
        app.apply_share_outcome(ShareMsg::radius(Ok(RadiusShareOutcome::Failed {
            detail: "not in an organization".to_string(),
        })));
        assert_eq!(
            errors(&app),
            vec!["Error: Failed to upload Radius artifact: not in an organization".to_string()]
        );
        assert!(
            statuses(&app).is_empty(),
            "no `Share URL:` line, and nothing that looks like a gist fallback"
        );
    }

    /// `if (loader.signal.aborted) return true` (`:125`, `:130`) and the `catch`'s
    /// `if (!loader.signal.aborted)` guard (`:142`): a cancelled upload prints NOTHING over the
    /// `Share cancelled` the cancel path already wrote.
    #[test]
    fn a_cancelled_radius_upload_prints_nothing() {
        let mut app = new_app();
        app.apply_share_outcome(ShareMsg::radius(Err("aborted".to_string())));
        assert!(statuses(&app).is_empty());
        assert!(errors(&app).is_empty());
    }
}
