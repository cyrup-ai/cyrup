//! `prompts.ts` — the prompt-command grammar and its result renderer.
//!
//! Pure functions over `&[CachedPromptArgument]` and [`rmcp::model::GetPromptResult`] — no
//! [`crate::state::McpState`], no host — so MCP-398's handler is a *caller* of these and they stand
//! alone.

use indexmap::IndexMap;
use rmcp::model::{ContentBlock, GetPromptResult, ResourceContents, Role};

use crate::registration::CachedPromptArgument;

/// `prompts.ts:65-103` `tokenizeArgs` (13h §5.3).
///
/// The quote characters STAY in the token; [`strip_quotes`] removes them later. That is the whole
/// reason `strip_quotes` exists — `find_unquoted_equals` needs to see them.
fn tokenize_args(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        // Backslash is LITERAL inside single quotes.
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            current.push(ch);
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            current.push(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    // `escaped` is consumed across iterations and never flushed, so a TRAILING LONE BACKSLASH is
    // dropped. Upstream behaviour (`prompts.ts:101`); do not "fix" it.
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// `findUnquotedEquals` (`prompts.ts:105-117`) — the BYTE index of the first `=` outside quotes.
///
/// Byte, not char: the caller slices with it, and every quote/`=` this scans is ASCII, so the two
/// agree wherever it returns `Some`.
fn find_unquoted_equals(token: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (index, ch) in token.char_indices() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if ch == '=' {
            return Some(index);
        }
    }
    None
}

/// `stripQuotes` (`prompts.ts:119-124`) — strip exactly one layer when the value is at least two
/// characters and its first and last CHARACTERS are the same quote.
fn strip_quotes(value: &str) -> &str {
    let mut chars = value.chars();
    let (Some(first), Some(last)) = (chars.next(), chars.next_back()) else {
        return value; // fewer than two characters
    };
    if (first == '"' || first == '\'') && first == last {
        return chars.as_str();
    }
    value
}

/// `parsePromptArgs`'s return (`prompts.ts:44`).
///
/// `IndexMap` because loop 2 of [`resolve_prompt_args`] iterates it and JS object key order is
/// insertion order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedPromptArgs {
    pub named: IndexMap<String, String>,
    pub positional: Vec<String>,
}

/// `parsePromptArgs` (`prompts.ts:126-138`) — MCP-396.
#[must_use]
pub fn parse_prompt_args(input: &str) -> ParsedPromptArgs {
    let mut out = ParsedPromptArgs::default();
    for token in tokenize_args(input) {
        // `eq > 0` is STRICT: a token starting with `=` is POSITIONAL, not a named arg with an
        // empty key. And a whitespace-only key falls THROUGH to positional — upstream's `if (key)`
        // guard sits inside the `eq > 0` branch and does not `continue` when it fails.
        let named = find_unquoted_equals(&token)
            .filter(|eq| *eq > 0)
            .and_then(|eq| {
                let key = token.get(..eq)?.trim();
                if key.is_empty() {
                    return None;
                }
                let value = strip_quotes(token.get(eq + 1..)?.trim());
                Some((key.to_string(), value.to_string()))
            });
        match named {
            Some((key, value)) => {
                out.named.insert(key, value);
            }
            None => out.positional.push(strip_quotes(&token).to_string()),
        }
    }
    out
}

/// `resolvePromptArgs` (`prompts.ts:140-168`) — MCP-397.
///
/// `Err` is [`build_usage_message`]'s text, which the caller notifies at `Error`.
///
/// # Errors
///
/// The usage message when a declared **required** argument resolves empty or missing.
pub fn resolve_prompt_args(
    declared: &[CachedPromptArgument],
    command_name: &str,
    parsed: &ParsedPromptArgs,
) -> Result<IndexMap<String, String>, String> {
    let mut args: IndexMap<String, String> = IndexMap::new();
    let mut positional_index = 0usize;

    // LOOP 1 — declaration order. The positional cursor advances ONLY on a named MISS: upstream's
    // `??` short-circuits before evaluating `positional[positionalIndex++]`. Written as
    // `.or_else(|| positional.get(i))` with an unconditional bump, every later positional shifts by
    // one — silent wrong output, not an error.
    for argument in declared {
        let value = match parsed.named.get(&argument.name) {
            Some(value) => Some(value.clone()),
            None => {
                let value = parsed.positional.get(positional_index).cloned();
                positional_index += 1;
                value
            }
        };
        if let Some(value) = value
            && !value.is_empty()
        {
            args.insert(argument.name.clone(), value);
        }
    }

    // LOOP 2 — undeclared named args are forwarded UNFILTERED: the MCP spec allows arbitrary string
    // key/values in `prompts/get` params.
    //
    // MCP-397a: NO `is_empty()` guard here. `topic=` is rejected by loop 1 and is therefore not in
    // `args`, so it lands as `args["topic"] = ""` for a declared OPTIONAL argument, while a declared
    // REQUIRED one still fails the `missing` filter below.
    for (key, value) in &parsed.named {
        if !args.contains_key(key) {
            args.insert(key.clone(), value.clone());
        }
    }

    let missing: Vec<&str> = declared
        .iter()
        .filter(|argument| argument.required.unwrap_or(false))
        .filter(|argument| args.get(&argument.name).is_none_or(String::is_empty))
        .map(|argument| argument.name.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(build_usage_message(declared, command_name, &missing));
    }
    Ok(args)
}

/// `buildUsageMessage` (`prompts.ts:170-176`).
///
/// The trailing `.trim()` is what removes the dangling space when the prompt declares no arguments
/// at all.
fn build_usage_message(
    declared: &[CachedPromptArgument],
    command_name: &str,
    missing: &[&str],
) -> String {
    let usage = declared
        .iter()
        .map(|argument| {
            if argument.required.unwrap_or(false) {
                format!("<{}>", argument.name)
            } else {
                format!("[{}]", argument.name)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let plural = if missing.len() > 1 { "s" } else { "" };
    format!(
        "Missing required argument{plural}: {}.\nUsage: /{command_name} {usage}",
        missing.join(", ")
    )
    .trim()
    .to_string()
}

/// `prompts.ts:185-197` `formatPromptResult` (13h §5.6) — MCP-399.
///
/// **Not** [`crate::renderers::transform_mcp_content`]: that is tool-result shaping over
/// `serde_json::Value`, with different casing, different bracket text, and an unknown arm that
/// re-serializes the JSON instead of contributing nothing. Two functions, deliberately.
#[must_use]
pub fn format_prompt_result(result: &GetPromptResult) -> String {
    let single = result.messages.len() == 1;
    let mut lines: Vec<String> = Vec::new();
    for message in &result.messages {
        let text = extract_message_text(&message.content);
        if text.is_empty() {
            continue;
        }
        // A lone USER message is emitted bare; everything else — including a lone ASSISTANT
        // message — keeps its `[role] ` prefix.
        if single && message.role == Role::User {
            lines.push(text);
        } else {
            lines.push(format!("[{}] {text}", role_str(&message.role)));
        }
    }
    lines.join("\n\n").trim().to_string()
}

/// `Role` is exhaustive in rmcp, so no wildcard — and the two spellings are its
/// `serde(rename_all = "camelCase")` wire values, which is what upstream interpolates.
const fn role_str(role: &Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// `extractMessageText` (`prompts.ts:199-222`).
fn extract_message_text(content: &ContentBlock) -> String {
    match content {
        ContentBlock::Text(text) => text.text.clone(),
        ContentBlock::Resource(embedded) => match &embedded.resource {
            ResourceContents::TextResourceContents { uri, text, .. } => {
                format!("[resource {uri}]\n{text}")
            }
            ResourceContents::BlobResourceContents { uri, .. } => format!("[resource {uri}]"),
            // `ResourceContents` is `#[non_exhaustive]` and has no `uri()` accessor, so this arm is
            // REQUIRED by the compiler even though 3.1.4 has exactly the two above. Upstream answers
            // `""` for a resource shape it cannot read (`if (!resource) return ""`), so this is the
            // faithful value, not a placeholder.
            _ => String::new(),
        },
        // Em dash, not a hyphen. rmcp's `Resource::uri`/`::name` are non-optional `String`, so
        // upstream's `uri ?? ""` cannot fire and `name ?` is an emptiness test.
        ContentBlock::ResourceLink(resource) => {
            if resource.name.is_empty() {
                format!("[resource_link {}]", resource.uri)
            } else {
                format!(
                    "[resource_link {} \u{2014} {}]",
                    resource.uri, resource.name
                )
            }
        }
        ContentBlock::Image(image) => {
            let mime = if image.mime_type.is_empty() {
                "unknown"
            } else {
                image.mime_type.as_str()
            };
            let embedded = if image.data.is_empty() {
                ""
            } else {
                " (embedded)"
            };
            format!("[image {mime}{embedded}]")
        }
        ContentBlock::Audio(audio) => {
            let mime = if audio.mime_type.is_empty() {
                "unknown"
            } else {
                audio.mime_type.as_str()
            };
            format!("[audio {mime}]")
        }
        // `ContentBlock` is `#[non_exhaustive]`, so this arm is required by the compiler AND is
        // upstream's `default: return ""`. Do not turn it into a stringify — that is
        // `transform_mcp_content`'s rule, not this one's.
        _ => String::new(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn arg(name: &str, required: bool) -> CachedPromptArgument {
        CachedPromptArgument {
            name: name.to_string(),
            description: None,
            required: Some(required),
        }
    }

    // ---- resolve_prompt_args: the load-bearing one ----

    /// **The assertion this module exists for.** Loop 1's positional cursor advances ONLY in the
    /// `None` branch, because upstream's `??` short-circuits before evaluating
    /// `positional[positionalIndex++]`.
    ///
    /// Written as `.or_else(|| positional.get(i))` with an unconditional bump, `a`'s named hit would
    /// still consume index 0 and `b` would bind to nothing — every later positional shifted by one.
    /// That is silent wrong output, not an error, which is why it is pinned here rather than left to
    /// the type system.
    #[test]
    fn a_named_hit_does_not_consume_a_positional() {
        let declared = [arg("a", true), arg("b", false)];
        let parsed = parse_prompt_args("a=x y");
        let bound = resolve_prompt_args(&declared, "cmd", &parsed).expect("both bind");
        assert_eq!(bound.get("a").map(String::as_str), Some("x"));
        assert_eq!(
            bound.get("b").map(String::as_str),
            Some("y"),
            "`b` must receive the FIRST positional; an unconditional bump leaves it unbound"
        );
    }

    /// MCP-397a — an explicit empty named value survives for a declared OPTIONAL argument. Loop 1
    /// rejects it as empty, loop 2 forwards it unguarded, so the server sees `""` rather than
    /// nothing. Adding an `is_empty()` guard to loop 2 breaks exactly this.
    #[test]
    fn an_explicit_empty_value_survives_for_an_optional_argument() {
        let declared = [arg("topic", false)];
        let parsed = parse_prompt_args("topic=");
        let bound = resolve_prompt_args(&declared, "cmd", &parsed).expect("optional accepts empty");
        assert_eq!(bound.get("topic").map(String::as_str), Some(""));
    }

    /// The same input against a REQUIRED argument still fails the `missing` filter.
    #[test]
    fn an_explicit_empty_value_still_fails_a_required_argument() {
        let declared = [arg("topic", true)];
        let parsed = parse_prompt_args("topic=");
        let usage =
            resolve_prompt_args(&declared, "cmd", &parsed).expect_err("required rejects it");
        assert_eq!(
            usage,
            "Missing required argument: topic.\nUsage: /cmd <topic>"
        );
    }

    /// Undeclared named arguments are forwarded UNFILTERED — the MCP spec allows arbitrary string
    /// key/values in `prompts/get` params.
    #[test]
    fn undeclared_named_arguments_are_forwarded() {
        let declared = [arg("a", false)];
        let parsed = parse_prompt_args("a=1 extra=2");
        let bound = resolve_prompt_args(&declared, "cmd", &parsed).expect("no required args");
        assert_eq!(bound.get("extra").map(String::as_str), Some("2"));
    }

    // ---- build_usage_message ----

    /// Singular at one, plural above it, and the `.trim()` that removes the space which would
    /// otherwise trail `/cmd` when the prompt declares no arguments at all.
    #[test]
    fn the_usage_message_singularises_and_trims() {
        let none: [CachedPromptArgument; 0] = [];
        assert_eq!(
            build_usage_message(&none, "cmd", &["x"]),
            "Missing required argument: x.\nUsage: /cmd",
            "no trailing space after the command name"
        );
        let declared = [arg("a", true), arg("b", true)];
        assert_eq!(
            build_usage_message(&declared, "cmd", &["a", "b"]),
            "Missing required arguments: a, b.\nUsage: /cmd <a> <b>"
        );
        // Optional arguments render in brackets, required in angles.
        let mixed = [arg("a", true), arg("b", false)];
        assert!(build_usage_message(&mixed, "cmd", &["a"]).ends_with("/cmd <a> [b]"));
    }

    // ---- tokenize_args ----

    /// Quotes STAY in the token — `find_unquoted_equals` needs to see them — and one layer is
    /// removed later by `strip_quotes`.
    #[test]
    fn quotes_stay_in_the_token_and_are_stripped_once() {
        assert_eq!(tokenize_args(r#""a b""#), vec![r#""a b""#.to_string()]);
        let parsed = parse_prompt_args(r#"k="a b""#);
        assert_eq!(parsed.named.get("k").map(String::as_str), Some("a b"));
    }

    /// A backslash is LITERAL inside single quotes (`char === "\\" && quote !== "'"`) and escapes
    /// outside them.
    #[test]
    fn a_backslash_is_literal_inside_single_quotes() {
        assert_eq!(tokenize_args(r"'a\b'"), vec![r"'a\b'".to_string()]);
        // Outside quotes it escapes, so the space joins rather than splits.
        assert_eq!(tokenize_args(r"a\ b"), vec!["a b".to_string()]);
    }

    /// **A trailing lone backslash is DROPPED.** `escaped` is set on the last character and never
    /// flushed after the loop, so the backslash vanishes. Upstream behaviour (`prompts.ts:101`) —
    /// do not "fix" it.
    #[test]
    fn a_trailing_lone_backslash_is_dropped() {
        assert_eq!(tokenize_args(r"a\"), vec!["a".to_string()]);
    }

    /// An unterminated quote runs to the end of the input rather than erroring.
    #[test]
    fn an_unterminated_quote_runs_to_end_of_input() {
        assert_eq!(
            tokenize_args("\"unterminated"),
            vec!["\"unterminated".to_string()]
        );
    }

    // ---- parse_prompt_args ----

    /// `eq > 0` is STRICT: a token starting with `=` is POSITIONAL, not a named argument with an
    /// empty key.
    #[test]
    fn a_leading_equals_is_positional_not_an_empty_key() {
        let parsed = parse_prompt_args("=v");
        assert!(parsed.named.is_empty());
        assert_eq!(parsed.positional, vec!["=v".to_string()]);
    }

    /// A key that trims to empty falls THROUGH to positional — upstream's `if (key)` guard sits
    /// inside the `eq > 0` branch and does not `continue` when it fails.
    #[test]
    fn a_whitespace_only_key_falls_through_to_positional() {
        // `\ =v` — an escaped space, then `=v`, so the token is ` =v` with `eq` at index 1.
        let parsed = parse_prompt_args(r"\ =v");
        assert!(parsed.named.is_empty(), "got {:?}", parsed.named);
        assert_eq!(parsed.positional, vec![" =v".to_string()]);
    }

    // ---- format_prompt_result ----

    fn text_message(role: Role, text: &str) -> rmcp::model::PromptMessage {
        rmcp::model::PromptMessage::new(
            role,
            ContentBlock::Text(rmcp::model::TextContent::new(text.to_string())),
        )
    }

    fn result(messages: Vec<rmcp::model::PromptMessage>) -> GetPromptResult {
        // `GetPromptResult` is `#[non_exhaustive]`; `new` is rmcp's own constructor.
        GetPromptResult::new(messages)
    }

    /// A LONE user message is emitted bare; everything else — including a lone ASSISTANT message —
    /// keeps its `[role] ` prefix.
    #[test]
    fn a_lone_user_message_is_bare_and_a_lone_assistant_message_is_not() {
        assert_eq!(
            format_prompt_result(&result(vec![text_message(Role::User, "hello")])),
            "hello"
        );
        assert_eq!(
            format_prompt_result(&result(vec![text_message(Role::Assistant, "hi")])),
            "[assistant] hi"
        );
        assert_eq!(
            format_prompt_result(&result(vec![
                text_message(Role::User, "a"),
                text_message(Role::Assistant, "b"),
            ])),
            "[user] a\n\n[assistant] b"
        );
    }

    /// An empty block contributes nothing and is skipped rather than leaving a blank paragraph.
    #[test]
    fn empty_blocks_are_skipped() {
        assert_eq!(
            format_prompt_result(&result(vec![
                text_message(Role::User, ""),
                text_message(Role::Assistant, "only"),
            ])),
            "[assistant] only"
        );
    }

    // ---- extract_message_text ----

    /// The separator is a U+2014 EM DASH, and a nameless link omits it entirely. rmcp's
    /// `Resource::uri`/`::name` are non-optional `String`, so these are emptiness tests rather than
    /// `Option` fallbacks.
    #[test]
    fn a_resource_link_uses_an_em_dash_only_when_it_has_a_name() {
        let mut resource = rmcp::model::Resource::new("file:///x", "");
        assert_eq!(
            extract_message_text(&ContentBlock::ResourceLink(resource.clone())),
            "[resource_link file:///x]"
        );
        resource.name = "readme".to_string();
        assert_eq!(
            extract_message_text(&ContentBlock::ResourceLink(resource)),
            "[resource_link file:///x \u{2014} readme]"
        );
    }

    /// An empty `mime_type` reads `unknown`, and the `(embedded)` suffix is an emptiness test on
    /// `data`.
    #[test]
    fn image_blocks_report_unknown_mime_and_flag_embedded_data() {
        let bare = rmcp::model::ImageContent::new(String::new(), String::new());
        assert_eq!(
            extract_message_text(&ContentBlock::Image(bare)),
            "[image unknown]"
        );
        let embedded = rmcp::model::ImageContent::new("AAA".to_string(), "image/png".to_string());
        assert_eq!(
            extract_message_text(&ContentBlock::Image(embedded)),
            "[image image/png (embedded)]"
        );
    }
}
