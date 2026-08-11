//! The two HTML pages a login flow serves back to the browser at the end of the redirect.
//!
//! 1:1 port of pi v0.83.0 `packages/ai/src/auth/oauth/oauth-page.ts`. The markup, the CSS, the
//! escaping rules and both page titles are byte-for-byte upstream's — this page is what the user
//! actually sees, so any drift is user-visible.

/// `LOGO_SVG` (`oauth-page.ts:1`), verbatim.
const LOGO_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 800 800" aria-hidden="true"><path fill="#fff" fill-rule="evenodd" d="M165.29 165.29 H517.36 V400 H400 V517.36 H282.65 V634.72 H165.29 Z M282.65 282.65 V400 H400 V282.65 Z"/><path fill="#fff" d="M517.36 400 H634.72 V634.72 H517.36 Z"/></svg>"##;

/// `escapeHtml` (`oauth-page.ts:3-10`). Order matters: `&` is replaced first so the entities the
/// later replacements introduce are not double-escaped.
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// `renderPage` (`oauth-page.ts:12-92`).
fn render_page(title: &str, heading: &str, message: &str, details: Option<&str>) -> String {
    let title = escape_html(title);
    let heading = escape_html(heading);
    let message = escape_html(message);
    let details = details.map(escape_html);
    let details_block = match details {
        Some(d) => format!("<div class=\"details\">{d}</div>"),
        None => String::new(),
    };

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{title}</title>
  <style>
    :root {{
      --text: #fafafa;
      --text-dim: #a1a1aa;
      --page-bg: #09090b;
      --font-sans: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, "Noto Sans", sans-serif, "Apple Color Emoji", "Segoe UI Emoji", "Segoe UI Symbol", "Noto Color Emoji";
      --font-mono: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace;
    }}
    * {{ box-sizing: border-box; }}
    html {{ color-scheme: dark; }}
    body {{
      margin: 0;
      min-height: 100vh;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 24px;
      background: var(--page-bg);
      color: var(--text);
      font-family: var(--font-sans);
      text-align: center;
    }}
    main {{
      width: 100%;
      max-width: 560px;
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
    }}
    .logo {{
      width: 72px;
      height: 72px;
      display: block;
      margin-bottom: 24px;
    }}
    h1 {{
      margin: 0 0 10px;
      font-size: 28px;
      line-height: 1.15;
      font-weight: 650;
      color: var(--text);
    }}
    p {{
      margin: 0;
      line-height: 1.7;
      color: var(--text-dim);
      font-size: 15px;
    }}
    .details {{
      margin-top: 16px;
      font-family: var(--font-mono);
      font-size: 13px;
      color: var(--text-dim);
      white-space: pre-wrap;
      word-break: break-word;
    }}
  </style>
</head>
<body>
  <main>
    <div class="logo">{LOGO_SVG}</div>
    <h1>{heading}</h1>
    <p>{message}</p>
    {details_block}
  </main>
</body>
</html>"#
    )
}

/// `oauthSuccessHtml` (`oauth-page.ts:94-100`).
pub fn oauth_success_html(message: &str) -> String {
    render_page(
        "Authentication successful",
        "Authentication successful",
        message,
        None,
    )
}

/// `oauthErrorHtml` (`oauth-page.ts:102-109`).
pub fn oauth_error_html(message: &str, details: Option<&str>) -> String {
    render_page(
        "Authentication failed",
        "Authentication failed",
        message,
        details,
    )
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
    fn success_page_uses_the_upstream_title_and_heading() {
        let html = oauth_success_html("Signed in to OpenRouter. You may now close this page.");
        assert!(html.starts_with("<!doctype html>\n<html lang=\"en\">"));
        assert!(html.contains("<title>Authentication successful</title>"));
        assert!(html.contains("<h1>Authentication successful</h1>"));
        assert!(
            html.contains("<p>Signed in to OpenRouter. You may now close this page.</p>"),
            "{html}"
        );
        // renderPage only emits `.details` when `options.details` is set (oauth-page.ts:90).
        assert!(!html.contains("class=\"details\""));
        assert!(html.contains(LOGO_SVG));
        assert!(html.ends_with("</html>"));
    }

    #[test]
    fn error_page_renders_details_when_given() {
        let plain = oauth_error_html("Callback route not found.", None);
        assert!(plain.contains("<title>Authentication failed</title>"));
        assert!(plain.contains("<h1>Authentication failed</h1>"));
        assert!(!plain.contains("class=\"details\""));

        let detailed = oauth_error_html(
            "OpenRouter authorization was denied.",
            Some("access_denied"),
        );
        assert!(
            detailed.contains("<div class=\"details\">access_denied</div>"),
            "{detailed}"
        );
    }

    /// `escapeHtml` (`oauth-page.ts:3-10`) — all five entities, and `&` first so `&lt;` does not
    /// become `&amp;lt;`.
    #[test]
    fn escapes_all_five_entities_without_double_escaping() {
        let html = oauth_error_html("a&b <script>alert(\"x\")</script> 'q'", Some("<&>\"'"));
        assert!(
            html.contains(
                "<p>a&amp;b &lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &#39;q&#39;</p>"
            ),
            "{html}"
        );
        assert!(
            html.contains("<div class=\"details\">&lt;&amp;&gt;&quot;&#39;</div>"),
            "{html}"
        );
        assert!(!html.contains("&amp;lt;"), "double-escaped");
        assert!(
            !html.contains("<script>"),
            "raw script tag survived escaping"
        );
    }

    /// The page carries no external references — pi's page is fully self-contained (inline CSS,
    /// inline SVG) because the browser hitting it may have no network path to anything else.
    #[test]
    fn page_is_self_contained() {
        let html = oauth_success_html("ok");
        // The only absolute URL upstream emits is the SVG namespace.
        assert_eq!(html.matches("http://").count(), 1, "{html}");
        assert!(html.contains("xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains(" src="), "{html}");
        assert!(!html.contains("<link"), "{html}");
        assert!(html.contains("<style>"));
    }
}
