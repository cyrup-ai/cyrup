//! The message `JSON.parse` throws for a document it rejects, as pi's runtime words it (CFG-088).
//!
//! pi reports an unparseable settings file as `Invalid settings file <path>: ${error.message}`
//! (`core/settings-diagnostics.ts:4-9` @v0.87.1), where `error` is whatever
//! `JSON.parse(stripBom(content))` threw (`settings-manager.ts:424`, caught at `:432-437`). pi runs
//! on Node `>=22.19.0` (`packages/coding-agent/package.json` `engines`), i.e. V8 12.4, so that
//! message is V8's: `Expected double-quoted property name in JSON at position 89 (line 5 column 1)`
//! for a trailing comma, where serde_json says `key must be a string at line 5 column 1`.
//!
//! [`json_parse_error_message`] reproduces V8's choice of message by walking the text the way
//! V8's `JsonParser` does (`src/json/json-parser.{h,cc}` at Node v22.22.2's `deps/v8`): the same
//! token classes, the same `Expect` sites with their explicit message templates
//! (`src/common/message-template.h`), the same position, line and column arithmetic
//! (`CalculateFileLocation`), and the same short-source / ellipsis context for an unexpected token
//! (`GetErrorMessageWithEllipses`, 10 characters either side). It only VALIDATES: serde_json still
//! does the parsing, and this is consulted only for the message once serde has rejected a
//! document.
//!
//! Positions are in UTF-16 code units, as V8's are, so a message about a document with non-ASCII
//! text before the error names the same position pi's does. Where V8's context would split a
//! surrogate pair, the lone half is rendered U+FFFD — what Node writes to a terminal for it.
//!
//! Verified beyond the committed corpus by a one-off differential run of 4 309 random mutations of
//! three settings-like documents against Node v22.22.2: every message matched.

/// V8's `JsonToken` (`json-parser.h:132-147`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tok {
    Number,
    String,
    LBrace,
    RBrace,
    LBrack,
    RBrack,
    True,
    False,
    Null,
    Whitespace,
    Colon,
    Comma,
    Illegal,
    Eos,
}

/// The explicit message templates V8's parser passes at its `Expect` / `ReportUnexpectedToken`
/// sites (`message-template.h`, the `JsonParse*` entries).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tpl {
    UnterminatedString,
    ExpectedPropNameOrRBrace,
    ExpectedCommaOrRBrack,
    ExpectedCommaOrRBrace,
    ExpectedDoubleQuotedPropertyName,
    ExponentPartMissingNumber,
    ExpectedColonAfterPropertyName,
    UnterminatedFractionalNumber,
    UnexpectedNonWhiteSpaceCharacter,
    BadEscapedCharacter,
    BadControlCharacter,
    BadUnicodeEscape,
    NoNumberAfterMinusSign,
}

impl Tpl {
    /// The template's text before ` at position % (line % column %)`.
    fn head(self) -> &'static str {
        match self {
            Tpl::UnterminatedString => "Unterminated string in JSON",
            Tpl::ExpectedPropNameOrRBrace => "Expected property name or '}' in JSON",
            Tpl::ExpectedCommaOrRBrack => "Expected ',' or ']' after array element in JSON",
            Tpl::ExpectedCommaOrRBrace => "Expected ',' or '}' after property value in JSON",
            Tpl::ExpectedDoubleQuotedPropertyName => "Expected double-quoted property name in JSON",
            Tpl::ExponentPartMissingNumber => "Exponent part is missing a number in JSON",
            Tpl::ExpectedColonAfterPropertyName => "Expected ':' after property name in JSON",
            Tpl::UnterminatedFractionalNumber => "Unterminated fractional number in JSON",
            Tpl::UnexpectedNonWhiteSpaceCharacter => {
                "Unexpected non-whitespace character after JSON"
            }
            Tpl::BadEscapedCharacter => "Bad escaped character in JSON",
            Tpl::BadControlCharacter => "Bad control character in string literal in JSON",
            Tpl::BadUnicodeEscape => "Bad Unicode escape in JSON",
            Tpl::NoNumberAfterMinusSign => "No number after minus sign in JSON",
        }
    }
}

/// `kMaxContextCharacters` / `kMinOriginalSourceLengthForContext` (`json-parser.h:333-335`).
const MAX_CONTEXT_CHARACTERS: usize = 10;
const MIN_ORIGINAL_SOURCE_LENGTH_FOR_CONTEXT: usize = MAX_CONTEXT_CHARACTERS * 2 + 1;

/// The message `JSON.parse(text)` throws, or `None` when V8 accepts `text`.
///
/// `text` is what pi hands `JSON.parse` — already BOM-stripped by the caller, as pi's
/// `stripBom(content)` does.
pub fn json_parse_error_message(text: &str) -> Option<String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut p = Parser {
        s: &units,
        cur: 0,
        next: Tok::Eos,
        message: None,
    };
    // `ParseJson` (`json-parser.cc:540-560`): one value, then nothing but whitespace.
    if p.parse_value().is_ok() && !p.check(Tok::Eos) {
        let next = p.next;
        let _ = p.report_token(next, Some(Tpl::UnexpectedNonWhiteSpaceCharacter));
    }
    p.message
}

/// A failed parse; the message is on [`Parser::message`].
struct Failed;

type Step = Result<(), Failed>;

fn is_digit(c: u16) -> bool {
    (u16::from(b'0')..=u16::from(b'9')).contains(&c)
}

/// `GetOneCharJsonToken` (`json-parser.cc:34-55`) over V8's Latin-1 table; anything wider is
/// `ILLEGAL` (`GetTokenForCharacter`, `:572-575`).
fn token_for(c: u16) -> Tok {
    let Ok(b) = u8::try_from(c) else {
        return Tok::Illegal;
    };
    match b {
        b'"' => Tok::String,
        b'0'..=b'9' | b'-' => Tok::Number,
        b'[' => Tok::LBrack,
        b'{' => Tok::LBrace,
        b']' => Tok::RBrack,
        b'}' => Tok::RBrace,
        b't' => Tok::True,
        b'f' => Tok::False,
        b'n' => Tok::Null,
        b' ' | b'\t' | b'\r' | b'\n' => Tok::Whitespace,
        b':' => Tok::Colon,
        b',' => Tok::Comma,
        _ => Tok::Illegal,
    }
}

/// `NumberPartField` of `GetJsonScanFlags` (`json-parser.cc:94-118`), Latin-1 only.
fn is_number_part(c: u16) -> bool {
    is_digit(c) || b".eE-+".iter().any(|b| c == u16::from(*b))
}

struct Parser<'a> {
    s: &'a [u16],
    /// V8's `cursor_`, as an index.
    cur: usize,
    /// V8's `next_`, set by [`Self::skip_whitespace`].
    next: Tok,
    message: Option<String>,
}

impl Parser<'_> {
    /// `CurrentCharacter`: `None` is `kEndOfString`.
    fn current(&self) -> Option<u16> {
        self.s.get(self.cur).copied()
    }

    fn current_is(&self, b: u8) -> bool {
        self.current() == Some(u16::from(b))
    }

    fn skip_whitespace(&mut self) {
        while self
            .current()
            .is_some_and(|c| token_for(c) == Tok::Whitespace)
        {
            self.cur += 1;
        }
        self.next = self.current().map_or(Tok::Eos, token_for);
    }

    fn check(&mut self, t: Tok) -> bool {
        self.skip_whitespace();
        if self.next != t {
            return false;
        }
        self.cur += 1;
        true
    }

    fn expect(&mut self, t: Tok, tpl: Tpl) -> Step {
        if self.next == t {
            self.cur += 1;
            Ok(())
        } else {
            let next = self.next;
            self.report_token(next, Some(tpl))
        }
    }

    fn expect_next(&mut self, t: Tok, tpl: Tpl) -> Step {
        self.skip_whitespace();
        self.expect(t, tpl)
    }

    /// `ReportUnexpectedCharacter` (`json-parser.cc:516-524`).
    fn report_char(&mut self, c: Option<u16>) -> Step {
        let tok = match c {
            None => Tok::Eos,
            Some(c) if c <= 0xFF => token_for(c),
            Some(_) => Tok::Illegal,
        };
        self.report_token(tok, None)
    }

    /// `ReportUnexpectedToken` (`json-parser.cc:472-514`) with `LookUpErrorMessageForJsonToken`
    /// (`:415-440`) for a token reported without an explicit template.
    fn report_token(&mut self, tok: Tok, tpl: Option<Tpl>) -> Step {
        let pos = self.cur;
        let (line, column) = self.file_location(pos);
        let located =
            |head: &str| format!("{head} at position {pos} (line {line} column {column})");
        let message = match tpl {
            Some(tpl) => located(tpl.head()),
            None => match tok {
                Tok::Eos => "Unexpected end of JSON input".to_string(),
                Tok::Number => located("Unexpected number in JSON"),
                Tok::String => located("Unexpected string in JSON"),
                _ if self.is_special_string() => {
                    format!("\"{}\" is not valid JSON", String::from_utf16_lossy(self.s))
                }
                _ => self.message_with_ellipses(pos),
            },
        };
        self.message = Some(message);
        Err(Failed)
    }

    /// `IsSpecialString` (`json-parser.cc:348-369`): the whole source is one of the four strings
    /// `JSON.parse` most often receives by mistake.
    fn is_special_string(&self) -> bool {
        ["[object Object]", "undefined", "Infinity", "NaN"]
            .iter()
            .any(|special| self.s.iter().copied().eq(special.encode_utf16()))
    }

    /// `GetErrorMessageWithEllipses` (`json-parser.cc:372-412`).
    fn message_with_ellipses(&self, pos: usize) -> String {
        let token = String::from_utf16_lossy(self.s.get(pos..=pos).unwrap_or_default());
        let len = self.s.len();
        if len < MIN_ORIGINAL_SOURCE_LENGTH_FOR_CONTEXT {
            let whole = String::from_utf16_lossy(self.s);
            return format!("Unexpected token '{token}', \"{whole}\" is not valid JSON");
        }
        let sub = |from: usize, to: usize| {
            String::from_utf16_lossy(self.s.get(from..to.min(len)).unwrap_or_default())
        };
        if pos < MAX_CONTEXT_CHARACTERS {
            let context = sub(0, pos + MAX_CONTEXT_CHARACTERS);
            format!("Unexpected token '{token}', \"{context}\"... is not valid JSON")
        } else if pos < len - MAX_CONTEXT_CHARACTERS {
            let context = sub(pos - MAX_CONTEXT_CHARACTERS, pos + MAX_CONTEXT_CHARACTERS);
            format!("Unexpected token '{token}', ...\"{context}\"... is not valid JSON")
        } else {
            let context = sub(pos - MAX_CONTEXT_CHARACTERS, len);
            format!("Unexpected token '{token}', ...\"{context}\" is not valid JSON")
        }
    }

    /// `CalculateFileLocation` (`json-parser.cc:442-469`): `\r`, `\n` and `\r\n` each end a line.
    fn file_location(&self, pos: usize) -> (usize, usize) {
        let (cr, lf) = (u16::from(b'\r'), u16::from(b'\n'));
        let mut line = 1;
        let mut last_line_break = 0;
        let mut i = 0;
        let at = |k: usize| self.s.get(k).copied();
        while i < pos {
            if at(i) == Some(cr) && i + 1 < pos && at(i + 1) == Some(lf) {
                i += 1;
            }
            if at(i) == Some(cr) || at(i) == Some(lf) {
                line += 1;
                last_line_break = i + 1;
            }
            i += 1;
        }
        (line, 1 + i - last_line_break)
    }

    /// `ParseJsonValue` (`json-parser.cc:1434-1762`), keeping only what decides an error: the
    /// continuation stack of open objects and arrays.
    fn parse_value(&mut self) -> Step {
        #[derive(Clone, Copy)]
        enum Cont {
            Object,
            Array,
        }
        let mut stack: Vec<Cont> = Vec::new();
        loop {
            // One value.
            loop {
                self.skip_whitespace();
                match self.next {
                    Tok::String => {
                        self.cur += 1;
                        self.scan_string()?;
                    }
                    Tok::Number => self.parse_number()?,
                    Tok::LBrace => {
                        self.cur += 1;
                        if !self.check(Tok::RBrace) {
                            stack.push(Cont::Object);
                            self.expect_next(Tok::String, Tpl::ExpectedPropNameOrRBrace)?;
                            self.scan_string()?;
                            self.expect_next(Tok::Colon, Tpl::ExpectedColonAfterPropertyName)?;
                            continue;
                        }
                    }
                    Tok::LBrack => {
                        self.cur += 1;
                        if !self.check(Tok::RBrack) {
                            stack.push(Cont::Array);
                            continue;
                        }
                    }
                    Tok::True => self.scan_literal(b"true")?,
                    Tok::False => self.scan_literal(b"false")?,
                    Tok::Null => self.scan_literal(b"null")?,
                    Tok::Colon
                    | Tok::Comma
                    | Tok::Illegal
                    | Tok::RBrace
                    | Tok::RBrack
                    | Tok::Eos
                    | Tok::Whitespace => {
                        let c = self.current();
                        return self.report_char(c);
                    }
                }
                break;
            }
            // Its continuations.
            loop {
                match stack.last() {
                    None => return Ok(()),
                    Some(Cont::Object) => {
                        if self.check(Tok::Comma) {
                            self.expect_next(Tok::String, Tpl::ExpectedDoubleQuotedPropertyName)?;
                            self.scan_string()?;
                            self.expect_next(Tok::Colon, Tpl::ExpectedColonAfterPropertyName)?;
                            break;
                        }
                        self.expect(Tok::RBrace, Tpl::ExpectedCommaOrRBrace)?;
                        stack.pop();
                    }
                    Some(Cont::Array) => {
                        if self.check(Tok::Comma) {
                            break;
                        }
                        self.expect(Tok::RBrack, Tpl::ExpectedCommaOrRBrack)?;
                        stack.pop();
                    }
                }
            }
        }
    }

    /// `ScanLiteral` (`json-parser.h:258-278`), `lit` without V8's trailing NUL.
    fn scan_literal(&mut self, lit: &[u8]) -> Step {
        let n = lit.len();
        let remaining = self.s.len() - self.cur;
        let rest = lit.get(1..).unwrap_or_default();
        if remaining >= n
            && self
                .s
                .get(self.cur + 1..self.cur + n)
                .is_some_and(|got| got.iter().copied().eq(rest.iter().map(|b| u16::from(*b))))
        {
            self.cur += n;
            return Ok(());
        }
        self.cur += 1;
        for &expected in lit.iter().skip(1).take((n - 1).min(remaining - 1)) {
            let Some(c) = self.current() else {
                break;
            };
            if c != u16::from(expected) {
                return self.report_char(Some(c));
            }
            self.cur += 1;
        }
        self.report_token(Tok::Eos, None)
    }

    /// `ScanJsonString` (`json-parser.cc:1998-2090`), entered just past the opening quote. Also
    /// serves property keys: `ScanJsonPropertyKey`'s array-index fast path (`:605-646`) rewinds to
    /// the key's start and falls back to this scan whenever the key is not a clean index, so it
    /// reports nothing of its own.
    fn scan_string(&mut self) -> Step {
        loop {
            while let Some(c) = self.current() {
                if c <= 0xFF && (c < 0x20 || c == u16::from(b'"') || c == u16::from(b'\\')) {
                    break;
                }
                self.cur += 1;
            }
            let Some(c) = self.current() else {
                return self.report_token(Tok::Illegal, Some(Tpl::UnterminatedString));
            };
            if c == u16::from(b'"') {
                self.cur += 1;
                return Ok(());
            }
            if c == u16::from(b'\\') {
                self.cur += 1;
                let escaped = match self.current() {
                    Some(e) if e <= 0xFF => e as u8,
                    other => return self.report_char(other),
                };
                match escaped {
                    b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                    b'u' => {
                        // `ScanUnicodeCharacter` (`:593-602`): four hex digits, each read with
                        // `NextCharacter`.
                        for _ in 0..4 {
                            self.cur += 1;
                            let hex = self
                                .current()
                                .and_then(|h| char::from_u32(u32::from(h)))
                                .is_some_and(|h| h.is_ascii_hexdigit());
                            if !hex {
                                return self
                                    .report_token(Tok::Illegal, Some(Tpl::BadUnicodeEscape));
                            }
                        }
                    }
                    _ => {
                        return self.report_token(Tok::Illegal, Some(Tpl::BadEscapedCharacter));
                    }
                }
                self.cur += 1;
                continue;
            }
            return self.report_token(Tok::Illegal, Some(Tpl::BadControlCharacter));
        }
    }

    /// `ParseJsonNumber` (`json-parser.cc:1771-1856`).
    fn parse_number(&mut self) -> Step {
        let mut negative = false;
        if self.current_is(b'-') {
            negative = true;
            self.cur += 1;
        }
        if self.current_is(b'0') {
            // "Prefix zero is only allowed if it's the only digit before a decimal point or
            // exponent."
            self.cur += 1;
            match self.current() {
                Some(c) if c <= 0xFF && is_number_part(c) => {
                    if is_digit(c) {
                        return self.report_token(Tok::Number, None);
                    }
                }
                _ if !negative => return Ok(()),
                _ => {}
            }
        } else {
            // The Smi fast path reads at most nine digits before deciding.
            let start = self.cur;
            let stop = (self.cur + 9).min(self.s.len());
            while self.cur < stop && self.current().is_some_and(is_digit) {
                self.cur += 1;
            }
            if self.cur == start {
                return self.report_token(Tok::Illegal, Some(Tpl::NoNumberAfterMinusSign));
            }
            match self.current() {
                Some(c) if c <= 0xFF && is_number_part(c) => {}
                _ => return Ok(()),
            }
            self.advance_to_non_decimal();
        }
        if self.current_is(b'.') {
            self.cur += 1;
            if !self.current().is_some_and(is_digit) {
                return self.report_token(Tok::Illegal, Some(Tpl::UnterminatedFractionalNumber));
            }
            self.advance_to_non_decimal();
        }
        // `AsciiAlphaToLower(CurrentCharacter()) == 'e'`.
        if self.current_is(b'e') || self.current_is(b'E') {
            self.cur += 1;
            if self.current_is(b'-') || self.current_is(b'+') {
                self.cur += 1;
            }
            if !self.current().is_some_and(is_digit) {
                return self.report_token(Tok::Illegal, Some(Tpl::ExponentPartMissingNumber));
            }
            self.advance_to_non_decimal();
        }
        Ok(())
    }

    fn advance_to_non_decimal(&mut self) {
        while self.current().is_some_and(is_digit) {
            self.cur += 1;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::json_parse_error_message;

    /// 60 documents with the message Node v22.22.2's `JSON.parse` threw for each (`null` where it
    /// accepted the document), captured with
    /// `for (const c of cases) { try { JSON.parse(c) } catch (e) { e.message } }`. The set covers
    /// every message template the parser can reach, CRLF line counting, the short-source and all
    /// three ellipsis contexts, the four special strings, non-ASCII text before the error and a
    /// leading BOM (which `JSON.parse` itself rejects — pi strips it first).
    const CORPUS: &str = include_str!("js_json_node22_corpus.json");

    #[derive(serde::Deserialize)]
    struct Case {
        text: String,
        message: Option<String>,
    }

    #[test]
    fn v8s_message_for_every_corpus_document() {
        let cases: Vec<Case> = serde_json::from_str(CORPUS).unwrap();
        assert_eq!(cases.len(), 60);
        for case in cases {
            assert_eq!(
                json_parse_error_message(&case.text),
                case.message,
                "JSON.parse({:?})",
                case.text
            );
        }
    }
}
