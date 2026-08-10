//! LaTeX math → terminal Unicode, a port of `pi/packages/tui/src/latex.ts` (v0.84.1, 1373 lines).
//!
//! M12. `markdown.ts:123-144` registers `LATEX_MARKDOWN_EXTENSIONS` — a block tokenizer for
//! `$$…$$` / `\[…\]` and an inline one for `$…$` / `\(…\)` / `$$…$$` / `\[…\]` — and the two
//! renderer arms at `markdown.ts:505-512` (`latexBlock`, `{ display: true }`) and `:645-652`
//! (`latex`, inline) call [`render_latex`] on the token's `text`. Both arms fall back to the token's
//! **raw source** when the renderer declines: `renderLatex(...) ?? latexToken.raw` — an expression
//! this typesetter cannot handle is printed verbatim, never dropped and never half-rendered.
//!
//! [`render_latex`] answers `None` for exactly the inputs upstream's `renderLatex` answers
//! `undefined` for: `LatexParser.render` returns `undefined` when `supported` was cleared or when
//! the walk stopped short of the end of the source (`latex.ts:816-822`).
//!
//! The tokenizer lives in [`crate::markdown`] because pulldown-cmark has no equivalent of marked's
//! `TokenizerExtension` hook; see `markdown::latex_prepass`.
#![allow(clippy::too_many_lines)]

use ratatui::text::Span;

/// `NEGATIVE_SPACE` (`latex.ts:511`) — the sentinel `\!` and friends return so `parseSequence` can
/// eat the preceding space (`:842-848`).
const NEGATIVE_SPACE: &str = "\u{0}";
/// `NAMED_OPERATOR_START` / `NAMED_OPERATOR_END` (`latex.ts:627-628`) — sentinels that survive until
/// [`normalize_output`] turns each into a space or nothing depending on its neighbour.
const NAMED_OPERATOR_START: char = '\u{f0004}';
const NAMED_OPERATOR_END: char = '\u{f0005}';
/// `LAYOUT_MARKER_START` / `LAYOUT_MARKER_END` (`latex.ts:672-673`) — `\u{f0000}<index>\u{f0001}`
/// stands in for a stacked fraction / limit operator / matrix until [`render_layout`] draws it.
const LAYOUT_MARKER_START: char = '\u{f0000}';
const LAYOUT_MARKER_END: char = '\u{f0001}';
/// `PROTECTED_SPACE` (`latex.ts:676`) — matrix cell padding that must survive `normalizeOutput`'s
/// whitespace collapse; rewritten to a real space at the very end of [`render_latex`].
const PROTECTED_SPACE: char = '\u{f0002}';

/// `SYMBOLS` (`latex.ts`), sorted for binary search.
static SYMBOLS: &[(&str, &str)] = &[
    ("Delta", "Δ"),
    ("Gamma", "Γ"),
    ("Im", "ℑ"),
    ("Lambda", "Λ"),
    ("Leftarrow", "⇐"),
    ("Leftrightarrow", "⇔"),
    ("Longleftarrow", "⇐"),
    ("Longleftrightarrow", "⇔"),
    ("Longrightarrow", "⇒"),
    ("Omega", "Ω"),
    ("Phi", "Φ"),
    ("Pi", "Π"),
    ("Psi", "Ψ"),
    ("Re", "ℜ"),
    ("Rightarrow", "⇒"),
    ("Sigma", "Σ"),
    ("Theta", "Θ"),
    ("Upsilon", "Υ"),
    ("Vdash", "⊩"),
    ("Vert", "‖"),
    ("Vvdash", "⊪"),
    ("Xi", "Ξ"),
    ("aleph", "ℵ"),
    ("alpha", "α"),
    ("amalg", "⨿"),
    ("angle", "∠"),
    ("approx", "≈"),
    ("ast", "∗"),
    ("asymp", "≍"),
    ("backslash", "\\"),
    ("because", "∵"),
    ("beta", "β"),
    ("beth", "ℶ"),
    ("bigcap", "⋂"),
    ("bigcirc", "○"),
    ("bigcup", "⋃"),
    ("bigodot", "⨀"),
    ("bigoplus", "⨁"),
    ("bigotimes", "⨂"),
    ("bigsqcup", "⨆"),
    ("biguplus", "⨄"),
    ("bigvee", "⋁"),
    ("bigwedge", "⋀"),
    ("bot", "⊥"),
    ("bullet", "•"),
    ("cap", "∩"),
    ("cdot", "·"),
    ("cdots", "⋯"),
    ("checkmark", "✓"),
    ("chi", "χ"),
    ("circ", "∘"),
    ("colon", ":"),
    ("complement", "∁"),
    ("cong", "≅"),
    ("coprod", "∐"),
    ("cup", "∪"),
    ("dagger", "†"),
    ("daleth", "ℸ"),
    ("dashv", "⊣"),
    ("ddagger", "‡"),
    ("ddots", "⋱"),
    ("delta", "δ"),
    ("div", "÷"),
    ("doteq", "≐"),
    ("dots", "…"),
    ("downarrow", "↓"),
    ("ell", "ℓ"),
    ("emptyset", "∅"),
    ("epsilon", "ϵ"),
    ("equiv", "≡"),
    ("eta", "η"),
    ("exists", "∃"),
    ("forall", "∀"),
    ("gamma", "γ"),
    ("ge", "≥"),
    ("geq", "≥"),
    ("geqslant", "≥"),
    ("gets", "←"),
    ("gg", "≫"),
    ("gimel", "ℷ"),
    ("hbar", "ℏ"),
    ("hookleftarrow", "↩"),
    ("hookrightarrow", "↪"),
    ("iff", "⇔"),
    ("iiint", "∭"),
    ("iint", "∬"),
    ("implies", "⇒"),
    ("in", "∈"),
    ("infty", "∞"),
    ("int", "∫"),
    ("iota", "ι"),
    ("kappa", "κ"),
    ("lVert", "‖"),
    ("lambda", "λ"),
    ("land", "∧"),
    ("langle", "⟨"),
    ("lbrace", "{"),
    ("lceil", "⌈"),
    ("ldots", "…"),
    ("le", "≤"),
    ("leadsto", "⇝"),
    ("leftarrow", "←"),
    ("leftharpoondown", "↽"),
    ("leftharpoonup", "↼"),
    ("leftrightarrow", "↔"),
    ("leftrightharpoons", "⇋"),
    ("leq", "≤"),
    ("leqslant", "≤"),
    ("lfloor", "⌊"),
    ("ll", "≪"),
    ("longleftarrow", "←"),
    ("longleftrightarrow", "↔"),
    ("longmapsto", "↦"),
    ("longrightarrow", "→"),
    ("lor", "∨"),
    ("lozenge", "◊"),
    ("lvert", "|"),
    ("mapsto", "↦"),
    ("mid", "∣"),
    ("models", "⊨"),
    ("mp", "∓"),
    ("mu", "μ"),
    ("nabla", "∇"),
    ("ne", "≠"),
    ("nearrow", "↗"),
    ("neg", "¬"),
    ("neq", "≠"),
    ("nexists", "∄"),
    ("ni", "∋"),
    ("notin", "∉"),
    ("nu", "ν"),
    ("nvDash", "⊭"),
    ("nvdash", "⊬"),
    ("nwarrow", "↖"),
    ("odot", "⊙"),
    ("oint", "∮"),
    ("omega", "ω"),
    ("ominus", "⊖"),
    ("oplus", "⊕"),
    ("oslash", "⊘"),
    ("otimes", "⊗"),
    ("parallel", "∥"),
    ("partial", "∂"),
    ("perp", "⊥"),
    ("phi", "ϕ"),
    ("pi", "π"),
    ("pm", "±"),
    ("prec", "≺"),
    ("preceq", "≼"),
    ("prime", "′"),
    ("prod", "∏"),
    ("propto", "∝"),
    ("psi", "ψ"),
    ("rVert", "‖"),
    ("rangle", "⟩"),
    ("rbrace", "}"),
    ("rceil", "⌉"),
    ("rfloor", "⌋"),
    ("rho", "ρ"),
    ("rightarrow", "→"),
    ("rightharpoondown", "⇁"),
    ("rightharpoonup", "⇀"),
    ("rightleftharpoons", "⇌"),
    ("rightsquigarrow", "⇝"),
    ("rvert", "|"),
    ("searrow", "↘"),
    ("setminus", "∖"),
    ("sigma", "σ"),
    ("sim", "∼"),
    ("simeq", "≃"),
    ("sqcap", "⊓"),
    ("sqcup", "⊔"),
    ("sqsubset", "⊏"),
    ("sqsubseteq", "⊑"),
    ("sqsupset", "⊐"),
    ("sqsupseteq", "⊒"),
    ("square", "□"),
    ("star", "⋆"),
    ("subset", "⊂"),
    ("subseteq", "⊆"),
    ("succ", "≻"),
    ("succeq", "≽"),
    ("sum", "∑"),
    ("supset", "⊃"),
    ("supseteq", "⊇"),
    ("swarrow", "↙"),
    ("tau", "τ"),
    ("therefore", "∴"),
    ("theta", "θ"),
    ("times", "×"),
    ("to", "→"),
    ("top", "⊤"),
    ("triangle", "△"),
    ("triangleleft", "◁"),
    ("triangleright", "▷"),
    ("twoheadleftarrow", "↞"),
    ("twoheadrightarrow", "↠"),
    ("uparrow", "↑"),
    ("uplus", "⊎"),
    ("upsilon", "υ"),
    ("varepsilon", "ε"),
    ("varkappa", "ϰ"),
    ("varnothing", "∅"),
    ("varphi", "φ"),
    ("varpi", "ϖ"),
    ("varrho", "ϱ"),
    ("varsigma", "ς"),
    ("vartheta", "ϑ"),
    ("vdash", "⊢"),
    ("vdots", "⋮"),
    ("vee", "∨"),
    ("vert", "|"),
    ("wedge", "∧"),
    ("wp", "℘"),
    ("wr", "≀"),
    ("xi", "ξ"),
    ("zeta", "ζ"),
];
/// `NEGATED_SYMBOLS` (`latex.ts`), sorted for binary search.
static NEGATED_SYMBOLS: &[(&str, &str)] = &[
    ("<", "≮"),
    ("=", "≠"),
    (">", "≯"),
    ("←", "↚"),
    ("→", "↛"),
    ("↔", "↮"),
    ("⇐", "⇍"),
    ("⇒", "⇏"),
    ("⇔", "⇎"),
    ("∈", "∉"),
    ("∋", "∌"),
    ("∣", "∤"),
    ("∥", "∦"),
    ("∼", "≁"),
    ("≃", "≄"),
    ("≅", "≇"),
    ("≈", "≉"),
    ("≡", "≢"),
    ("≤", "≰"),
    ("≥", "≱"),
    ("≺", "⊀"),
    ("≻", "⊁"),
    ("≼", "⋠"),
    ("≽", "⋡"),
    ("⊂", "⊄"),
    ("⊃", "⊅"),
    ("⊆", "⊈"),
    ("⊇", "⊉"),
    ("⊢", "⊬"),
    ("⊨", "⊭"),
];
/// `BLACKBOARD` (`latex.ts`), sorted for binary search.
static BLACKBOARD: &[(&str, &str)] = &[
    ("C", "ℂ"),
    ("H", "ℍ"),
    ("N", "ℕ"),
    ("P", "ℙ"),
    ("Q", "ℚ"),
    ("R", "ℝ"),
    ("Z", "ℤ"),
];
/// `SUPERSCRIPTS` (`latex.ts`), sorted for binary search.
static SUPERSCRIPTS: &[(&str, &str)] = &[
    ("(", "⁽"),
    (")", "⁾"),
    ("+", "⁺"),
    ("-", "⁻"),
    ("0", "⁰"),
    ("1", "¹"),
    ("2", "²"),
    ("3", "³"),
    ("4", "⁴"),
    ("5", "⁵"),
    ("6", "⁶"),
    ("7", "⁷"),
    ("8", "⁸"),
    ("9", "⁹"),
    ("=", "⁼"),
    ("a", "ᵃ"),
    ("b", "ᵇ"),
    ("c", "ᶜ"),
    ("d", "ᵈ"),
    ("e", "ᵉ"),
    ("f", "ᶠ"),
    ("g", "ᵍ"),
    ("h", "ʰ"),
    ("i", "ⁱ"),
    ("j", "ʲ"),
    ("k", "ᵏ"),
    ("l", "ˡ"),
    ("m", "ᵐ"),
    ("n", "ⁿ"),
    ("o", "ᵒ"),
    ("p", "ᵖ"),
    ("r", "ʳ"),
    ("s", "ˢ"),
    ("t", "ᵗ"),
    ("u", "ᵘ"),
    ("v", "ᵛ"),
    ("w", "ʷ"),
    ("x", "ˣ"),
    ("y", "ʸ"),
    ("z", "ᶻ"),
];
/// `SUBSCRIPTS` (`latex.ts`), sorted for binary search.
static SUBSCRIPTS: &[(&str, &str)] = &[
    ("(", "₍"),
    (")", "₎"),
    ("+", "₊"),
    ("-", "₋"),
    ("0", "₀"),
    ("1", "₁"),
    ("2", "₂"),
    ("3", "₃"),
    ("4", "₄"),
    ("5", "₅"),
    ("6", "₆"),
    ("7", "₇"),
    ("8", "₈"),
    ("9", "₉"),
    ("=", "₌"),
    ("a", "ₐ"),
    ("e", "ₑ"),
    ("h", "ₕ"),
    ("i", "ᵢ"),
    ("j", "ⱼ"),
    ("k", "ₖ"),
    ("l", "ₗ"),
    ("m", "ₘ"),
    ("n", "ₙ"),
    ("o", "ₒ"),
    ("p", "ₚ"),
    ("r", "ᵣ"),
    ("s", "ₛ"),
    ("t", "ₜ"),
    ("u", "ᵤ"),
    ("v", "ᵥ"),
    ("x", "ₓ"),
];
/// `ACCENTS` (`latex.ts`), sorted for binary search.
static ACCENTS: &[(&str, &str)] = &[
    ("acute", "\u{301}"),
    ("bar", "\u{305}"),
    ("breve", "\u{306}"),
    ("check", "\u{30c}"),
    ("ddot", "\u{308}"),
    ("dot", "\u{307}"),
    ("grave", "\u{300}"),
    ("hat", "\u{302}"),
    ("mathring", "\u{30a}"),
    ("overleftarrow", "\u{20d6}"),
    ("overleftrightarrow", "\u{20e1}"),
    ("overline", "\u{305}"),
    ("overrightarrow", "\u{20d7}"),
    ("tilde", "\u{303}"),
    ("underline", "\u{332}"),
    ("vec", "\u{20d7}"),
    ("widehat", "\u{302}"),
    ("widetilde", "\u{303}"),
];
/// `NAMED_OPERATORS` (`latex.ts`), sorted for binary search.
static NAMED_OPERATORS: &[&str] = &[
    "Pr",
    "arccos",
    "arcsin",
    "arctan",
    "arg",
    "cos",
    "cosh",
    "cot",
    "coth",
    "csc",
    "deg",
    "det",
    "dim",
    "exp",
    "gcd",
    "hom",
    "inf",
    "ker",
    "lg",
    "lim",
    "liminf",
    "limsup",
    "ln",
    "log",
    "max",
    "min",
    "sec",
    "sin",
    "sinh",
    "sup",
    "tan",
    "tanh",
];
/// `LIMIT_OPERATORS` (`latex.ts`), sorted for binary search.
static LIMIT_OPERATORS: &[&str] = &[
    "argmax",
    "argmin",
    "inf",
    "injlim",
    "lim",
    "liminf",
    "limsup",
    "max",
    "min",
    "projlim",
    "sup",
];
/// `DISPLAY_LIMIT_SYMBOLS` (`latex.ts`), sorted for binary search.
static DISPLAY_LIMIT_SYMBOLS: &[&str] = &[
    "bigcap",
    "bigcup",
    "bigodot",
    "bigoplus",
    "bigotimes",
    "bigsqcup",
    "biguplus",
    "bigvee",
    "bigwedge",
    "coprod",
    "iiint",
    "iint",
    "int",
    "oint",
    "prod",
    "sum",
];
/// `RELATION_COMMANDS` (`latex.ts`), sorted for binary search.
static RELATION_COMMANDS: &[&str] = &[
    "Leftarrow",
    "Leftrightarrow",
    "Longleftarrow",
    "Longleftrightarrow",
    "Longrightarrow",
    "Rightarrow",
    "Vdash",
    "Vvdash",
    "approx",
    "asymp",
    "cong",
    "dashv",
    "doteq",
    "downarrow",
    "equiv",
    "ge",
    "geq",
    "geqslant",
    "gets",
    "gg",
    "hookleftarrow",
    "hookrightarrow",
    "iff",
    "implies",
    "in",
    "le",
    "leadsto",
    "leftarrow",
    "leftharpoondown",
    "leftharpoonup",
    "leftrightarrow",
    "leftrightharpoons",
    "leq",
    "leqslant",
    "ll",
    "longleftarrow",
    "longleftrightarrow",
    "longmapsto",
    "longrightarrow",
    "mapsto",
    "mid",
    "models",
    "ne",
    "nearrow",
    "neq",
    "ni",
    "notin",
    "nvDash",
    "nvdash",
    "nwarrow",
    "parallel",
    "perp",
    "prec",
    "preceq",
    "propto",
    "rightarrow",
    "rightharpoondown",
    "rightharpoonup",
    "rightleftharpoons",
    "rightsquigarrow",
    "searrow",
    "sim",
    "simeq",
    "sqsubset",
    "sqsubseteq",
    "sqsupset",
    "sqsupseteq",
    "subset",
    "subseteq",
    "succ",
    "succeq",
    "supset",
    "supseteq",
    "swarrow",
    "to",
    "triangleleft",
    "triangleright",
    "twoheadleftarrow",
    "twoheadrightarrow",
    "uparrow",
    "vdash",
];
/// `SPACING_COMMANDS` (`latex.ts`), sorted for binary search.
static SPACING_COMMANDS: &[&str] = &[
    " ",
    ",",
    ":",
    ";",
    ">",
    "enskip",
    "enspace",
    "medspace",
    "qquad",
    "quad",
    "thickspace",
    "thinspace",
];
/// `NEGATIVE_SPACING_COMMANDS` (`latex.ts`), sorted for binary search.
static NEGATIVE_SPACING_COMMANDS: &[&str] = &[
    "!",
    "negmedspace",
    "negthickspace",
    "negthinspace",
];
/// `IGNORED_COMMANDS` (`latex.ts`), sorted for binary search.
static IGNORED_COMMANDS: &[&str] = &[
    "displaystyle",
    "limits",
    "nolimits",
    "scriptscriptstyle",
    "scriptstyle",
    "textstyle",
];
/// `SIZE_COMMANDS` (`latex.ts`), sorted for binary search.
static SIZE_COMMANDS: &[&str] = &[
    "Big",
    "Bigg",
    "Biggl",
    "Biggr",
    "Bigl",
    "Bigr",
    "big",
    "bigg",
    "biggl",
    "biggr",
    "bigl",
    "bigr",
];
/// `PLAIN_WRAPPERS` (`latex.ts`), sorted for binary search.
static PLAIN_WRAPPERS: &[&str] = &[
    "bm",
    "boldsymbol",
    "emph",
    "mathbf",
    "mathcal",
    "mathfrak",
    "mathit",
    "mathnormal",
    "mathrm",
    "mathscr",
    "mathsf",
    "mathtt",
    "mathup",
    "mbox",
    "overbrace",
    "pmb",
    "smash",
    "substack",
    "text",
    "textbf",
    "textit",
    "textmd",
    "textnormal",
    "textrm",
    "textsc",
    "textsf",
    "textsl",
    "texttt",
    "textup",
    "underbrace",
];

// ── lookups ──────────────────────────────────────────────────────────────────────────────────────

fn lookup(table: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    let i = table.binary_search_by(|(k, _)| (*k).cmp(key)).ok()?;
    table.get(i).map(|(_, v)| *v)
}

fn has(set: &'static [&'static str], key: &str) -> bool {
    set.binary_search(&key).is_ok()
}

/// Visible width in terminal columns — Pi's `visibleWidth` (`latex.ts:1`, imported from `utils.ts`).
/// `Span::width` is unicode-width-backed; never `chars().count()`.
fn visible_width(s: &str) -> usize {
    Span::raw(s).width()
}

/// `chars[from..to]` without a slice index — the no-panic policy denies `indexing_slicing`.
fn slice(chars: &[char], from: usize, to: usize) -> String {
    chars.iter().skip(from).take(to.saturating_sub(from)).collect()
}

fn slice_from(chars: &[char], from: usize) -> String {
    chars.iter().skip(from).collect()
}

/// First index >= `from` at which `needle` occurs in `hay` (`String.prototype.indexOf`).
fn find_sub(hay: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(hay.len()));
    }
    let last = hay.len().checked_sub(needle.len())?;
    (from..=last).find(|&i| needle.iter().enumerate().all(|(k, c)| hay.get(i + k) == Some(c)))
}

// ── scalar formatters (`latex.ts:587-625`) ───────────────────────────────────────────────────────

/// `replaceCharacters` (`latex.ts:587-598`) — map EVERY code point through `replacements`, or fail.
fn replace_characters(
    value: &str,
    table: &'static [(&'static str, &'static str)],
) -> Option<String> {
    let mut out = String::new();
    for ch in value.chars() {
        let mut buf = [0u8; 4];
        out.push_str(lookup(table, ch.encode_utf8(&mut buf))?);
    }
    Some(out)
}

/// `value.replace(/\s*([=+-])\s*/g, "$1")` (`latex.ts:602`).
fn collapse_around_ops(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if matches!(c, '=' | '+' | '-') {
            while out.ends_with(char::is_whitespace) {
                out.pop();
            }
            out.push(c);
            while it.peek().is_some_and(|n| n.is_whitespace()) {
                it.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `/^[\p{L}\p{N}.]+$/u` (`latex.ts:617`, `:624`).
fn is_letters_digits_dot(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphabetic() || c.is_numeric() || c == '.')
}

/// `/^[\p{N}.]+$/u` (`latex.ts:618`).
fn is_digits_dot(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_numeric() || c == '.')
}

/// `formatScript` (`latex.ts:599-612`): Unicode super/subscripts when EVERY character maps, else the
/// `^x` / `_x` / `^(xy)` textual fallback. The fallback tests the pre-collapse `value`, not the
/// collapsed one — `:606-610` reads `value`, and `:602` only fed the lookup.
fn format_script(value: &str, sub: bool) -> String {
    let value = value.trim();
    let table = if sub { SUBSCRIPTS } else { SUPERSCRIPTS };
    if let Some(u) = replace_characters(&collapse_around_ops(value), table) {
        return u;
    }
    let prefix = if sub { '_' } else { '^' };
    let single = value.chars().count() == 1;
    let alpha_sub = sub && !value.is_empty() && value.chars().all(|c| c.is_ascii_alphabetic());
    if single || alpha_sub { format!("{prefix}{value}") } else { format!("{prefix}({value})") }
}

/// `formatFraction` (`latex.ts:614-620`) — the INLINE fraction `a/b`, parenthesised where ambiguous.
fn format_fraction(numerator: &str, denominator: &str) -> String {
    let numerator = numerator.trim();
    let denominator = denominator.trim();
    let simple_num = is_letters_digits_dot(numerator);
    let simple_den = is_digits_dot(denominator) || denominator.chars().count() == 1;
    let n = if simple_num { numerator.to_string() } else { format!("({numerator})") };
    let d = if simple_den { denominator.to_string() } else { format!("({denominator})") };
    format!("{n}/{d}")
}

/// `formatRoot` (`latex.ts:622-625`).
fn format_root(value: &str, symbol: &str) -> String {
    let value = value.trim();
    if is_letters_digits_dot(value) {
        format!("{symbol}{value}")
    } else {
        format!("{symbol}({value})")
    }
}

/// `normalizeOutput` (`latex.ts:632-643`).
///
/// Four rewrites in order, and the ORDER is load-bearing: the named-operator END lookahead at
/// `:635` runs against a string from which the START sentinels have already been removed by `:634`.
fn normalize_output(value: &str) -> String {
    // `/(?<=[\p{L}\p{N})\]}\u{f0001}])\u{f0004}/gu` → " ", then `replaceAll(START, "")`.
    let mut a = String::new();
    let mut prev: Option<char> = None;
    for ch in value.chars() {
        if ch == NAMED_OPERATOR_START {
            let spaced = prev.is_some_and(|p| {
                p.is_alphabetic()
                    || p.is_numeric()
                    || matches!(p, ')' | ']' | '}')
                    || p == LAYOUT_MARKER_END
            });
            if spaced {
                a.push(' ');
            }
        } else {
            a.push(ch);
        }
        prev = Some(ch);
    }
    // `/\u{f0005}(?=[\p{L}\p{N}√\u{f0000}])/gu` → " ", then `replaceAll(END, "")`.
    let ach: Vec<char> = a.chars().collect();
    let mut b = String::new();
    for (i, ch) in ach.iter().enumerate() {
        if *ch == NAMED_OPERATOR_END {
            let spaced = ach.get(i + 1).is_some_and(|n| {
                n.is_alphabetic() || n.is_numeric() || *n == '√' || *n == LAYOUT_MARKER_START
            });
            if spaced {
                b.push(' ');
            }
        } else {
            b.push(*ch);
        }
    }
    // Per line: `/[ \t]+/g` → " ", then trim; then drop blank lines that are neither interior
    // (`:640`).
    let lines: Vec<String> = b.split('\n').map(|l| collapse_blanks(l).trim().to_string()).collect();
    let n = lines.len();
    let kept: Vec<String> = lines
        .into_iter()
        .enumerate()
        .filter(|(i, l)| !l.is_empty() || (*i > 0 && *i + 1 < n))
        .map(|(_, l)| l)
        .collect();
    kept.join("\n").trim().to_string()
}

/// `/[ \t]+/g` → `" "`.
fn collapse_blanks(s: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for c in s.chars() {
        if c == ' ' || c == '\t' {
            if !in_run {
                out.push(' ');
                in_run = true;
            }
        } else {
            in_run = false;
            out.push(c);
        }
    }
    out
}

// ── two-dimensional layout (`latex.ts:645-795`) ──────────────────────────────────────────────────

enum LayoutNode {
    Fraction { numerator: String, denominator: String },
    Operator { operator: String, lower: Option<String>, upper: Option<String> },
    Matrix { lines: Vec<String>, baseline: usize },
}

struct Layout {
    lines: Vec<String>,
    width: usize,
    baseline: usize,
}

/// `padLayoutLine` (`latex.ts:678-682`).
fn pad_layout_line(line: &str, width: usize, centered: bool) -> String {
    let padding = width.saturating_sub(visible_width(line));
    let left = if centered { padding / 2 } else { 0 };
    format!("{}{line}{}", " ".repeat(left), " ".repeat(padding - left))
}

/// `joinLayouts` (`latex.ts:684-707`) — sit boxes side by side on a shared baseline.
fn join_layouts(layouts: &[Layout]) -> Layout {
    if layouts.is_empty() {
        return Layout { lines: vec![String::new()], width: 0, baseline: 0 };
    }
    let baseline = layouts.iter().map(|l| l.baseline).max().unwrap_or(0);
    let below = layouts
        .iter()
        .map(|l| l.lines.len().saturating_sub(l.baseline).saturating_sub(1))
        .max()
        .unwrap_or(0);
    let mut lines: Vec<String> = Vec::new();
    for row in 0..=baseline + below {
        let mut line = String::new();
        for layout in layouts {
            // `const sourceRow = row - baseline + layout.baseline;` — may go negative upstream, so
            // the comparison is done in signed space here.
            let source_row = (row + layout.baseline).checked_sub(baseline);
            match source_row.and_then(|r| layout.lines.get(r)) {
                Some(src) => line.push_str(&pad_layout_line(src, layout.width, false)),
                None => line.push_str(&" ".repeat(layout.width)),
            }
        }
        lines.push(line.trim_end().to_string());
    }
    let width = layouts.iter().map(|l| l.width).sum();
    Layout { lines, width, baseline }
}

/// Scan one source line for `\u{f0000}(\d+)\u{f0001}` (`LAYOUT_MARKER_PATTERN`, `latex.ts:674`).
/// Returns `(char index, char length, node index)` per match, in order.
fn layout_markers(line: &[char]) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < line.len() {
        if line.get(i) == Some(&LAYOUT_MARKER_START) {
            let mut j = i + 1;
            let mut digits = String::new();
            while line.get(j).is_some_and(char::is_ascii_digit) {
                if let Some(d) = line.get(j) {
                    digits.push(*d);
                }
                j += 1;
            }
            if !digits.is_empty() && line.get(j) == Some(&LAYOUT_MARKER_END) {
                if let Ok(index) = digits.parse::<usize>() {
                    out.push((i, j + 1 - i, index));
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// `renderLayout` (`latex.ts:709-795`).
fn render_layout(source: &str, nodes: &[LayoutNode]) -> Layout {
    let mut rendered_lines: Vec<String> = Vec::new();
    let mut first_baseline = 0usize;
    for source_line in source.split('\n') {
        let ch: Vec<char> = source_line.chars().collect();
        let mut layouts: Vec<Layout> = Vec::new();
        let mut position = 0usize;
        let mut previous: Option<&LayoutNode> = None;
        for (index, len, node_index) in layout_markers(&ch) {
            let Some(node) = nodes.get(node_index) else { continue };
            if index > position {
                let sliced = slice(&ch, position, index);
                let trimmed = if previous.is_some() {
                    sliced.trim_start().trim_end().to_string()
                } else {
                    sliced.trim_end().to_string()
                };
                let keep_leading = matches!(previous, Some(LayoutNode::Matrix { .. }))
                    && sliced.starts_with(char::is_whitespace);
                let keep_trailing = matches!(node, LayoutNode::Matrix { .. })
                    && sliced.ends_with(char::is_whitespace);
                let text = if trimmed.is_empty() {
                    if keep_leading || keep_trailing { " ".to_string() } else { String::new() }
                } else {
                    format!(
                        "{}{trimmed}{}",
                        if keep_leading { " " } else { "" },
                        if keep_trailing { " " } else { "" }
                    )
                };
                let width = visible_width(&text);
                layouts.push(Layout { lines: vec![text], width, baseline: 0 });
            }
            match node {
                LayoutNode::Fraction { numerator, denominator } => {
                    let num = render_layout(numerator, nodes);
                    let den = render_layout(denominator, nodes);
                    let content_width = num.width.max(den.width).max(1);
                    let width = content_width + 2;
                    let mut lines: Vec<String> =
                        num.lines.iter().map(|l| pad_layout_line(l, width, true)).collect();
                    lines.push(format!(" {} ", "─".repeat(content_width)));
                    lines.extend(den.lines.iter().map(|l| pad_layout_line(l, width, true)));
                    layouts.push(Layout { lines, width, baseline: num.lines.len() });
                }
                LayoutNode::Operator { operator, lower, upper } => {
                    let content_width = visible_width(operator)
                        .max(lower.as_deref().map(visible_width).unwrap_or(0))
                        .max(upper.as_deref().map(visible_width).unwrap_or(0));
                    let mut lines: Vec<String> = Vec::new();
                    if let Some(u) = upper {
                        lines.push(format!("{} ", pad_layout_line(u, content_width, true)));
                    }
                    lines.push(format!("{} ", pad_layout_line(operator, content_width, true)));
                    if let Some(l) = lower {
                        lines.push(format!("{} ", pad_layout_line(l, content_width, true)));
                    }
                    let baseline = usize::from(upper.is_some());
                    layouts.push(Layout { lines, width: content_width + 1, baseline });
                }
                LayoutNode::Matrix { lines: mlines, baseline } => {
                    let width = mlines.iter().map(|l| visible_width(l)).max().unwrap_or(0);
                    layouts.push(Layout {
                        lines: mlines.iter().map(|l| pad_layout_line(l, width, false)).collect(),
                        width,
                        baseline: *baseline,
                    });
                }
            }
            position = index + len;
            previous = Some(node);
        }
        if position < ch.len() {
            let sliced = slice_from(&ch, position);
            let trimmed =
                if previous.is_some() { sliced.trim_start().to_string() } else { sliced.clone() };
            let text = if matches!(previous, Some(LayoutNode::Matrix { .. }))
                && sliced.starts_with(char::is_whitespace)
            {
                format!(" {trimmed}")
            } else {
                trimmed
            };
            let width = visible_width(&text);
            layouts.push(Layout { lines: vec![text], width, baseline: 0 });
        }
        let line_layout = join_layouts(&layouts);
        if rendered_lines.is_empty() {
            first_baseline = line_layout.baseline;
        }
        rendered_lines.extend(line_layout.lines);
    }
    let width = rendered_lines.iter().map(|l| visible_width(l)).max().unwrap_or(0);
    Layout { lines: rendered_lines, width, baseline: first_baseline }
}

// ── the parser (`latex.ts:797-1344`) ─────────────────────────────────────────────────────────────

/// How a limit-style operator renders its subscript when limits are NOT stacked (`latex.ts:1082`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum InlineLower {
    /// `\lim_{x\to 0}` → `lim[x → 0]`.
    Bracket,
    /// `\sum_{i}` → `∑ᵢ`.
    Script,
}

/// `class LatexParser` (`latex.ts:797`).
struct Parser<'a> {
    src: Vec<char>,
    nodes: &'a mut Vec<LayoutNode>,
    display: bool,
    position: usize,
    supported: bool,
    stack_fractions: bool,
}

impl<'a> Parser<'a> {
    fn new(source: &str, nodes: &'a mut Vec<LayoutNode>, display: bool) -> Self {
        Parser {
            src: source.chars().collect(),
            nodes,
            display,
            position: 0,
            supported: true,
            stack_fractions: true,
        }
    }

    fn at(&self, i: usize) -> Option<char> {
        self.src.get(i).copied()
    }

    fn cur(&self) -> Option<char> {
        self.at(self.position)
    }

    /// `render()` (`latex.ts:813-820`) — `undefined` unless the walk consumed the WHOLE source with
    /// nothing unsupported on the way.
    fn render(&mut self) -> Option<String> {
        let rendered = self.parse_sequence(None);
        if !self.supported || self.position != self.src.len() {
            return None;
        }
        Some(normalize_output(&rendered))
    }

    /// `parseSequence` (`latex.ts:822-903`).
    fn parse_sequence(&mut self, end_character: Option<char>) -> String {
        let mut result = String::new();
        while self.position < self.src.len() {
            let Some(character) = self.cur() else { break };
            if end_character == Some(character) {
                self.position += 1;
                return result;
            }
            if character == '}' {
                self.supported = false;
                return result;
            }
            if character == '{' {
                self.position += 1;
                let inner = self.parse_sequence(Some('}'));
                result.push_str(&inner);
                continue;
            }
            if character == '\\' {
                let command = self.parse_command();
                if command == NEGATIVE_SPACE {
                    // `result = result.trimEnd(); if (result.endsWith(END)) result = …slice(0, -1)`
                    // (`:843-848`).
                    let trimmed = result.trim_end().to_string();
                    result = trimmed;
                    if result.ends_with(NAMED_OPERATOR_END) {
                        result.pop();
                    }
                } else {
                    result.push_str(&command);
                }
                continue;
            }
            if character == '^' || character == '_' {
                self.position += 1;
                result = result.trim_end().to_string();
                let arg = self.parse_required_argument(false);
                let script = format_script(&arg, character == '_');
                if result.ends_with(NAMED_OPERATOR_END) {
                    // The script goes INSIDE the operator sentinel so the spacing pass still sees
                    // the operator's boundary (`:857-861`).
                    result.pop();
                    result.push_str(&script);
                    result.push(NAMED_OPERATOR_END);
                } else {
                    result.push_str(&script);
                }
                continue;
            }
            if character.is_whitespace() {
                result.push_str(&self.parse_whitespace());
                continue;
            }
            if character == '=' || character == '<' || character == '>' {
                result = format!("{} {character} ", result.trim_end());
                self.position += 1;
                continue;
            }
            if character == '&' {
                self.position += 1;
                continue;
            }
            if character == '~' {
                self.position += 1;
                result.push(' ');
                continue;
            }
            if character == '.' {
                // A `.` immediately after a matrix marker belongs to the matrix's LAST row
                // (`:884-895`), e.g. the period ending a displayed equation.
                if let Some(index) = trailing_layout_marker(&result)
                    && let Some(LayoutNode::Matrix { lines, .. }) = self.nodes.get_mut(index)
                {
                    if let Some(last) = lines.last_mut() {
                        last.push('.');
                    } else {
                        lines.push(".".to_string());
                    }
                    self.position += 1;
                    continue;
                }
            }
            result.push(character);
            self.position += 1;
        }
        if end_character.is_some() {
            self.supported = false;
        }
        result
    }

    /// `parseWhitespace` (`latex.ts:905-910`) — a run of whitespace collapses to one space.
    fn parse_whitespace(&mut self) -> String {
        while self.cur().is_some_and(char::is_whitespace) {
            self.position += 1;
        }
        " ".to_string()
    }

    /// `parseCommand` (`latex.ts:912-1076`). The order of the arms is upstream's, verbatim.
    fn parse_command(&mut self) -> String {
        self.position += 1;
        if self.position >= self.src.len() {
            self.supported = false;
            return String::new();
        }
        let first = self.cur().unwrap_or('\0');
        let command: String = if first.is_ascii_alphabetic() {
            let start = self.position;
            while self.cur().is_some_and(|c| c.is_ascii_alphabetic()) {
                self.position += 1;
            }
            slice(&self.src, start, self.position)
        } else {
            self.position += 1;
            first.to_string()
        };

        if command == "\\" {
            return "\n".to_string();
        }
        if has(SPACING_COMMANDS, &command) {
            return " ".to_string();
        }
        if has(NEGATIVE_SPACING_COMMANDS, &command) {
            return NEGATIVE_SPACE.to_string();
        }
        if has(IGNORED_COMMANDS, &command) {
            return String::new();
        }
        if matches!(command.as_str(), "{" | "}" | "$" | "%" | "#" | "_" | "&") {
            return command;
        }
        if command == "|" {
            return "‖".to_string();
        }
        if command == "not" {
            let value = self.parse_required_argument(false).trim().to_string();
            if let Some(negated) = lookup(NEGATED_SYMBOLS, &value) {
                return format!(" {negated} ");
            }
            let mut chars = value.chars();
            let Some(head) = chars.next() else {
                self.supported = false;
                return String::new();
            };
            let rest: String = chars.collect();
            return format!(" {head}\u{338}{rest} ");
        }
        if has(LIMIT_OPERATORS, &command) {
            return self.parse_operator(&command, InlineLower::Bracket, true, true);
        }
        if let Some(symbol) = lookup(SYMBOLS, &command) {
            if has(DISPLAY_LIMIT_SYMBOLS, &command) {
                return self.parse_operator(symbol, InlineLower::Script, true, false);
            }
            return if command == "cdot" || command == "times" || has(RELATION_COMMANDS, &command) {
                format!(" {symbol} ")
            } else {
                symbol.to_string()
            };
        }
        if has(NAMED_OPERATORS, &command) {
            return format!("{NAMED_OPERATOR_START}{command}{NAMED_OPERATOR_END}");
        }
        if has(SIZE_COMMANDS, &command) {
            return String::new();
        }
        if command == "left" || command == "middle" || command == "right" {
            if self.cur() == Some('.') {
                self.position += 1;
            }
            return String::new();
        }
        if command == "frac" || command == "dfrac" || command == "tfrac" {
            let should_stack = self.display && self.stack_fractions && command != "tfrac";
            let numerator = self.parse_required_argument(!should_stack);
            let denominator = self.parse_required_argument(!should_stack);
            if should_stack {
                self.nodes.push(LayoutNode::Fraction {
                    numerator: normalize_output(&numerator),
                    denominator: normalize_output(&denominator),
                });
                let index = self.nodes.len() - 1;
                return format!("{LAYOUT_MARKER_START}{index}{LAYOUT_MARKER_END}");
            }
            return format_fraction(&numerator, &denominator);
        }
        if command == "sqrt" {
            let degree = self.parse_optional_argument().map(|d| d.trim().to_string());
            let value = self.parse_required_argument(true);
            return match degree.as_deref() {
                None | Some("2") => format_root(&value, "√"),
                Some("3") => format_root(&value, "∛"),
                Some("4") => format_root(&value, "∜"),
                Some(d) => format!("{}{}", format_script(d, false), format_root(&value, "√")),
            };
        }
        if command == "boxed" || command == "fbox" {
            return format!("[{}]", self.parse_required_argument(true).trim());
        }
        if command == "binom" || command == "dbinom" || command == "tbinom" {
            let a = self.parse_required_argument(true);
            let b = self.parse_required_argument(true);
            return format!("({a} choose {b})");
        }
        if let Some(accent) = lookup(ACCENTS, &command) {
            let value = self.parse_required_argument(true);
            return if value.chars().count() == 1 {
                format!("{value}{accent}")
            } else {
                format!("{command}({value})")
            };
        }
        if command == "mathbb" {
            let value = self.parse_required_argument(true);
            return value
                .chars()
                .map(|c| {
                    let mut buf = [0u8; 4];
                    lookup(BLACKBOARD, c.encode_utf8(&mut buf))
                        .map_or_else(|| c.to_string(), str::to_string)
                })
                .collect();
        }
        if command == "operatorname" {
            let starred = self.cur() == Some('*');
            if starred {
                self.position += 1;
            }
            let arg = self.parse_required_argument(true);
            let operator = normalize_output(&arg).trim().to_string();
            return self.parse_operator(&operator, InlineLower::Bracket, starred, true);
        }
        if command == "mod" || command == "bmod" {
            return " mod ".to_string();
        }
        if command == "pmod" || command == "pod" {
            let value = self.parse_required_argument(true).trim().to_string();
            return if command == "pmod" {
                format!(" (mod {value})")
            } else {
                format!(" ({value})")
            };
        }
        if command == "overset" || command == "stackrel" {
            let upper = self.parse_required_argument(true);
            let value = self.parse_required_argument(true).trim().to_string();
            return format!("{value}{}", format_script(&upper, false));
        }
        if command == "underset" {
            let lower = self.parse_required_argument(true);
            let value = self.parse_required_argument(true).trim().to_string();
            return format!("{value}{}", format_script(&lower, true));
        }
        if has(PLAIN_WRAPPERS, &command) {
            let value = self.parse_required_argument(true);
            return if command.starts_with("text") || command == "mbox" {
                value
            } else {
                value.trim().to_string()
            };
        }
        if command == "begin" {
            return self.parse_environment();
        }
        if command == "end" {
            self.supported = false;
            return String::new();
        }

        self.supported = false;
        format!("\\{command}")
    }

    /// `parseOperator` (`latex.ts:1078-1133`).
    fn parse_operator(
        &mut self,
        operator: &str,
        inline_lower_style: InlineLower,
        display_limits: bool,
        spaced: bool,
    ) -> String {
        let mut use_display_limits = display_limits;
        let mut modifier_position = self.position;
        while self.at(modifier_position).is_some_and(|c| c == ' ' || c == '\t') {
            modifier_position += 1;
        }
        // `/^\\(limits|nolimits)(?![A-Za-z])/`
        if self.at(modifier_position) == Some('\\') {
            for (name, limits) in [("limits", true), ("nolimits", false)] {
                let needle: Vec<char> = name.chars().collect();
                let after = modifier_position + 1 + needle.len();
                let matches = needle
                    .iter()
                    .enumerate()
                    .all(|(k, c)| self.at(modifier_position + 1 + k) == Some(*c))
                    && !self.at(after).is_some_and(|c| c.is_ascii_alphabetic());
                if matches {
                    use_display_limits = limits;
                    self.position = after;
                    break;
                }
            }
        }

        let mut lower: Option<String> = None;
        let mut upper: Option<String> = None;
        loop {
            let mut script_position = self.position;
            while self.at(script_position).is_some_and(|c| c == ' ' || c == '\t') {
                script_position += 1;
            }
            let kind = self.at(script_position);
            if kind != Some('_') && kind != Some('^') {
                break;
            }
            self.position = script_position + 1;
            let arg = self.parse_required_argument(false);
            let value = normalize_output(&arg).replace(' ', "");
            if kind == Some('_') {
                if lower.is_some() {
                    self.supported = false;
                }
                lower = Some(value);
            } else {
                if upper.is_some() {
                    self.supported = false;
                }
                upper = Some(value);
            }
        }

        if self.display && use_display_limits && (lower.is_some() || upper.is_some()) {
            self.nodes.push(LayoutNode::Operator {
                operator: operator.to_string(),
                lower,
                upper,
            });
            let index = self.nodes.len() - 1;
            return format!("{LAYOUT_MARKER_START}{index}{LAYOUT_MARKER_END}");
        }

        let mut rendered = operator.to_string();
        if let Some(l) = &lower {
            match inline_lower_style {
                InlineLower::Bracket => rendered.push_str(&format!("[{l}]")),
                InlineLower::Script => rendered.push_str(&format_script(l, true)),
            }
        }
        if let Some(u) = &upper {
            rendered.push_str(&format_script(u, false));
        }
        if spaced { format!(" {rendered} ") } else { rendered }
    }

    /// `parseRequiredArgument` (`latex.ts:1135-1141`) — `stackFractions` is AND-ed, never restored
    /// upward.
    fn parse_required_argument(&mut self, stack_fractions: bool) -> String {
        let previous = self.stack_fractions;
        self.stack_fractions = previous && stack_fractions;
        let value = self.parse_required_argument_value();
        self.stack_fractions = previous;
        value
    }

    /// `parseRequiredArgumentValue` (`latex.ts:1143-1160`).
    fn parse_required_argument_value(&mut self) -> String {
        while self.cur().is_some_and(|c| c == ' ' || c == '\t') {
            self.position += 1;
        }
        if self.position >= self.src.len() {
            self.supported = false;
            return String::new();
        }
        if self.cur() == Some('{') {
            self.position += 1;
            return self.parse_sequence(Some('}'));
        }
        if self.cur() == Some('\\') {
            return self.parse_command();
        }
        let value = self.cur().map(|c| c.to_string()).unwrap_or_default();
        self.position += 1;
        value
    }

    /// `parseOptionalArgument` (`latex.ts:1162-1177`).
    fn parse_optional_argument(&mut self) -> Option<String> {
        while self.cur().is_some_and(|c| c == ' ' || c == '\t') {
            self.position += 1;
        }
        if self.cur() != Some('[') {
            return None;
        }
        let Some(end) = find_sub(&self.src, &[']'], self.position + 1) else {
            self.supported = false;
            return None;
        };
        let value = slice(&self.src, self.position + 1, end);
        self.position = end + 1;
        Some(self.render_nested(&value, true))
    }

    /// `readRawGroup` (`latex.ts:1179-1207`).
    fn read_raw_group(&mut self) -> Option<String> {
        while self.cur().is_some_and(|c| c == ' ' || c == '\t') {
            self.position += 1;
        }
        if self.cur() != Some('{') {
            self.supported = false;
            return None;
        }
        self.position += 1;
        let start = self.position;
        let mut depth = 1usize;
        while self.position < self.src.len() {
            let character = self.cur();
            if character == Some('\\') {
                self.position += 2;
                continue;
            }
            if character == Some('{') {
                depth += 1;
            }
            if character == Some('}') {
                depth -= 1;
            }
            if depth == 0 {
                let value = slice(&self.src, start, self.position);
                self.position += 1;
                return Some(value);
            }
            self.position += 1;
        }
        self.supported = false;
        None
    }

    /// `parseEnvironment` (`latex.ts:1213-1284`).
    fn parse_environment(&mut self) -> String {
        let Some(environment) = self.read_raw_group().filter(|e| !e.is_empty()) else {
            return String::new();
        };
        let end_marker: Vec<char> = format!("\\end{{{environment}}}").chars().collect();
        let Some(end) = find_sub(&self.src, &end_marker, self.position) else {
            self.supported = false;
            return String::new();
        };
        let body = slice(&self.src, self.position, end);
        self.position = end + end_marker.len();

        if matches!(environment.as_str(), "equation" | "equation*" | "displaymath") {
            return self.render_nested(&body, true).trim().to_string();
        }

        if matches!(
            environment.as_str(),
            "aligned"
                | "align"
                | "align*"
                | "alignedat"
                | "alignat"
                | "alignat*"
                | "gather"
                | "gathered"
                | "multline"
                | "multline*"
                | "split"
        ) {
            let aligned_at = matches!(environment.as_str(), "alignedat" | "alignat" | "alignat*");
            let aligned_body =
                if aligned_at { strip_leading_brace_group(&body) } else { body.clone() };
            let rows: Vec<String> = split_environment_rows(&aligned_body)
                .into_iter()
                .map(|row| {
                    let cells: Vec<&str> = row.split('&').collect();
                    let source = if aligned_at {
                        // `Array.from({length: ceil(cells.length / 2)}, (_, i) =>
                        // cells.slice(i*2, i*2+2).join("")).join(" ")` (`:1257-1261`).
                        cells
                            .chunks(2)
                            .map(|pair| pair.concat())
                            .collect::<Vec<String>>()
                            .join(" ")
                    } else {
                        cells.concat()
                    };
                    self.render_nested(&source, true).trim().to_string()
                })
                .filter(|r| !r.is_empty())
                .collect();
            return rows.join("\n");
        }

        if environment == "cases" || environment == "cases*" {
            let rows: Vec<Vec<String>> = split_environment_rows(&body)
                .into_iter()
                .map(|row| {
                    row.split('&')
                        .map(|cell| self.render_nested(cell, false).trim().to_string())
                        .collect::<Vec<String>>()
                })
                .filter(|row: &Vec<String>| row.iter().any(|c| !c.is_empty()))
                .collect();
            let count = rows.len();
            return rows
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    let value = trim_trailing_comma(row.first().map_or("", String::as_str));
                    let condition = row.get(1).map_or("", String::as_str);
                    let delimiter = if index == 0 {
                        '⎧'
                    } else if index + 1 == count {
                        '⎩'
                    } else {
                        '⎨'
                    };
                    let tail = if condition.is_empty() {
                        String::new()
                    } else {
                        let prefix = if starts_with_condition_word(condition) { " " } else { " if " };
                        format!("{prefix}{condition}")
                    };
                    format!("{delimiter} {value}{tail}")
                })
                .collect::<Vec<String>>()
                .join("\n");
        }

        if matches!(
            environment.as_str(),
            "array"
                | "matrix"
                | "smallmatrix"
                | "pmatrix"
                | "bmatrix"
                | "Bmatrix"
                | "vmatrix"
                | "Vmatrix"
        ) {
            let matrix_body =
                if environment == "array" { strip_leading_brace_group(&body) } else { body };
            return self.render_matrix(&environment, &matrix_body);
        }

        self.supported = false;
        body
    }

    /// `renderMatrix` (`latex.ts:1286-1336`).
    fn render_matrix(&mut self, environment: &str, body: &str) -> String {
        let matrix: Vec<Vec<String>> = split_environment_rows(body)
            .into_iter()
            .map(|row| {
                row.split('&')
                    .map(|cell| self.render_nested(cell, false).trim().to_string())
                    .collect::<Vec<String>>()
            })
            .filter(|row: &Vec<String>| row.iter().any(|c| !c.is_empty()))
            .collect();
        let column_count = matrix.iter().map(Vec::len).max().unwrap_or(0);
        let column_widths: Vec<usize> = (0..column_count)
            .map(|column| {
                matrix
                    .iter()
                    .map(|row| row.get(column).map_or(0, |c| visible_width(c)))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let rows: Vec<String> = matrix
            .iter()
            .map(|row| {
                (0..column_count)
                    .map(|column| {
                        let cell = row.get(column).map_or("", String::as_str);
                        let pad = column_widths
                            .get(column)
                            .copied()
                            .unwrap_or(0)
                            .saturating_sub(visible_width(cell));
                        format!("{cell}{}", PROTECTED_SPACE.to_string().repeat(pad))
                    })
                    .collect::<Vec<String>>()
                    .join(" │ ")
            })
            .collect();

        let lines: Vec<String> = if matches!(environment, "array" | "matrix" | "smallmatrix") {
            rows
        } else {
            let delimiter = match environment {
                "pmatrix" => ['⎛', '⎞', '⎜', '⎟', '⎝', '⎠'],
                "bmatrix" => ['⎡', '⎤', '⎢', '⎥', '⎣', '⎦'],
                "Bmatrix" => ['⎧', '⎫', '⎨', '⎬', '⎩', '⎭'],
                "vmatrix" => ['│', '│', '│', '│', '│', '│'],
                "Vmatrix" => ['║', '║', '║', '║', '║', '║'],
                _ => {
                    self.supported = false;
                    return rows.join("\n");
                }
            };
            let count = rows.len();
            rows.iter()
                .enumerate()
                .map(|(index, row)| {
                    let pick = |first: usize, last: usize, mid: usize| {
                        let i = if index == 0 {
                            first
                        } else if index + 1 == count {
                            last
                        } else {
                            mid
                        };
                        delimiter.get(i).copied().unwrap_or(' ')
                    };
                    format!("{} {row} {}", pick(0, 4, 2), pick(1, 5, 3))
                })
                .collect()
        };

        if lines.len() <= 1 {
            return lines.first().cloned().unwrap_or_default();
        }
        self.nodes.push(LayoutNode::Matrix { lines, baseline: 0 });
        let index = self.nodes.len() - 1;
        format!("{LAYOUT_MARKER_START}{index}{LAYOUT_MARKER_END}")
    }

    /// `renderNested` (`latex.ts:1338-1343`) — a failed nested parse poisons the OUTER parse and
    /// yields the nested source verbatim.
    fn render_nested(&mut self, source: &str, stack_fractions: bool) -> String {
        let display = self.display && stack_fractions;
        let rendered = {
            let mut parser = Parser::new(source, self.nodes, display);
            parser.render()
        };
        match rendered {
            Some(r) => r,
            None => {
                self.supported = false;
                source.to_string()
            }
        }
    }
}

/// `TRAILING_LAYOUT_MARKER_PATTERN` (`latex.ts:675`) — the node index of a marker at the very END.
fn trailing_layout_marker(result: &str) -> Option<usize> {
    let ch: Vec<char> = result.chars().collect();
    if ch.last() != Some(&LAYOUT_MARKER_END) {
        return None;
    }
    let mut i = ch.len().checked_sub(1)?;
    let mut digits = String::new();
    while i > 0 {
        i -= 1;
        match ch.get(i) {
            Some(c) if c.is_ascii_digit() => digits.insert(0, *c),
            Some(c) if *c == LAYOUT_MARKER_START => {
                return if digits.is_empty() { None } else { digits.parse().ok() };
            }
            _ => return None,
        }
    }
    None
}

/// `body.split(/\\\\(?:\[[^\]\n]*\])?/)` (`latex.ts:1209-1211`).
fn split_environment_rows(body: &str) -> Vec<String> {
    let ch: Vec<char> = body.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 0usize;
    while i < ch.len() {
        if ch.get(i) == Some(&'\\') && ch.get(i + 1) == Some(&'\\') {
            let mut next = i + 2;
            if ch.get(next) == Some(&'[') {
                let mut k = next + 1;
                let mut closed = false;
                while let Some(c) = ch.get(k) {
                    if *c == ']' {
                        closed = true;
                        break;
                    }
                    if *c == '\n' {
                        break;
                    }
                    k += 1;
                }
                if closed {
                    next = k + 1;
                }
            }
            out.push(std::mem::take(&mut cur));
            i = next;
            continue;
        }
        if let Some(c) = ch.get(i) {
            cur.push(*c);
        }
        i += 1;
    }
    out.push(cur);
    out
}

/// `body.replace(/^\s*\{[^}]*\}/, "")` (`latex.ts:1256`, `:1279`).
fn strip_leading_brace_group(body: &str) -> String {
    let ch: Vec<char> = body.chars().collect();
    let mut i = 0usize;
    while ch.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    if ch.get(i) != Some(&'{') {
        return body.to_string();
    }
    let mut j = i + 1;
    while let Some(c) = ch.get(j) {
        if *c == '}' {
            return slice_from(&ch, j + 1);
        }
        j += 1;
    }
    body.to_string()
}

/// `value.replace(/,\s*$/, "")` (`latex.ts:1271`).
fn trim_trailing_comma(value: &str) -> String {
    let trimmed = value.trim_end();
    trimmed.strip_suffix(',').unwrap_or(trimmed).to_string()
}

/// `/^(?:if|when|for|otherwise)\b/i` (`latex.ts:1274`).
fn starts_with_condition_word(condition: &str) -> bool {
    let lower = condition.to_ascii_lowercase();
    ["if", "when", "for", "otherwise"].iter().any(|w| {
        lower.strip_prefix(w).is_some_and(|rest| {
            rest.chars().next().is_none_or(|c| !c.is_alphanumeric() && c != '_')
        })
    })
}

/// Render a LaTeX math expression as terminal-friendly Unicode — `renderLatex`
/// (`latex.ts:1355-1372`). `None` when the expression is unsupported or malformed, which is the
/// caller's cue to print the raw source instead.
pub(crate) fn render_latex(source: &str, display: bool) -> Option<String> {
    let mut nodes: Vec<LayoutNode> = Vec::new();
    let rendered = {
        let mut parser = Parser::new(source, &mut nodes, display);
        parser.render()
    }?;
    if nodes.is_empty() {
        return Some(rendered.replace(PROTECTED_SPACE, " "));
    }
    let lines = render_layout(&rendered, &nodes).lines;
    // `Math.min(...lines.filter(l => l.trim()).map(l => l.length - l.trimStart().length))`
    // (`:1365-1367`) — `Math.min()` of nothing is `Infinity`, and `slice(Infinity)` empties every
    // line, which `None` reproduces here.
    let indentation = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().count() - l.trim_start().chars().count())
        .min();
    let out: Vec<String> = lines
        .iter()
        .map(|l| match indentation {
            Some(n) => l.chars().skip(n).collect::<String>().trim_end().to_string(),
            None => String::new(),
        })
        .collect();
    Some(out.join("\n").trim_end().replace(PROTECTED_SPACE, " "))
}

// ── the marked tokenizer extensions (`markdown.ts:50-144`) ───────────────────────────────────────

/// One recognised math span.
pub(crate) struct LatexToken {
    /// Char length of `raw` — how much of the source the token consumed.
    pub(crate) raw_len: usize,
    /// The source the token consumed, verbatim. Both renderer arms fall back to it.
    pub(crate) raw: String,
    /// The expression between the delimiters.
    pub(crate) text: String,
    /// A delimiter that has not closed yet — mid-stream. Upstream never typesets a pending token:
    /// `!latexToken.pending && …` gates BOTH arms (`markdown.ts:507`, `:648`).
    pub(crate) pending: bool,
}

/// `isEscaped` (`markdown.ts:71-78`) — an odd run of preceding backslashes escapes the delimiter.
fn is_escaped(source: &[char], index: usize) -> bool {
    let mut backslashes = 0usize;
    let mut position = index;
    while position > 0 && source.get(position - 1) == Some(&'\\') {
        backslashes += 1;
        position -= 1;
    }
    backslashes % 2 == 1
}

/// `findClosingDelimiter` (`markdown.ts:80-86`).
fn find_closing_delimiter(source: &[char], closing: &[char], start: usize) -> Option<usize> {
    let mut index = find_sub(source, closing, start);
    while let Some(i) = index {
        if !is_escaped(source, i) {
            return Some(i);
        }
        index = find_sub(source, closing, i + closing.len());
    }
    None
}

/// `looksLikePendingDollarMath` (`markdown.ts:88-90`):
/// `/\\[A-Za-z]+|[_^=+*\/<>()[\]|±≤≥≠≈∈→⇒∞∫∑√-]/`.
fn looks_like_pending_dollar_math(source: &[char]) -> bool {
    for (i, c) in source.iter().enumerate() {
        if *c == '\\' && source.get(i + 1).is_some_and(|n| n.is_ascii_alphabetic()) {
            return true;
        }
        if "_^=+*/<>()[]|±≤≥≠≈∈→⇒∞∫∑√-".contains(*c) {
            return true;
        }
    }
    false
}

/// `tokenizeInlineLatex` (`markdown.ts:92-99` and the guards at `:110-125` of the upstream file —
/// see the quoted conditions below).
pub(crate) fn tokenize_inline(source: &[char]) -> Option<LatexToken> {
    let starts = |p: &str| -> bool {
        p.chars().enumerate().all(|(i, c)| source.get(i) == Some(&c))
    };
    let (opening, closing): (&str, &str) = if starts("$$") {
        ("$$", "$$")
    } else if starts("\\(") {
        ("\\(", "\\)")
    } else if starts("\\[") {
        ("\\[", "\\]")
    } else if starts("$") && !source.get(1).is_some_and(|c| c.is_whitespace()) {
        // `source.startsWith("$") && !/^\$\s/.test(source)` — `$` at end of input has no next char,
        // and `/^\$\s/` cannot match then either.
        ("$", "$")
    } else {
        return None;
    };
    let open_len = opening.chars().count();
    let close: Vec<char> = closing.chars().collect();
    let closing_index = find_closing_delimiter(source, &close, open_len);

    if let Some(ci) = closing_index
        && opening == "$"
    {
        let inner: Vec<char> = source.iter().copied().skip(open_len).take(ci - open_len).collect();
        let after: &[char] = source.get(ci + close.len()..).unwrap_or(&[]);
        // `/\s$/.test(inner) || /^\d/.test(after) ||
        //  (/^[A-Z_][A-Z0-9_]*(?:[^A-Za-z0-9_\s])?$/.test(inner) &&
        //   /^[A-Za-z_][A-Za-z0-9_]*/.test(after)) || inner.includes("`")`
        // — the currency / SHOUTY_IDENT heuristics that stop `$5` and `$FOO_BAR` reading as math.
        let inner_ends_ws = inner.last().is_some_and(|c| c.is_whitespace());
        let after_digit = after.first().is_some_and(char::is_ascii_digit);
        let shouty = is_shouty_ident(&inner)
            && after.first().is_some_and(|c| c.is_ascii_alphabetic() || *c == '_');
        if inner_ends_ws || after_digit || shouty || inner.contains(&'`') {
            return None;
        }
    }

    let Some(closing_index) = closing_index else {
        let pending: &[char] = source.get(open_len..).unwrap_or(&[]);
        if opening.starts_with('\\') || looks_like_pending_dollar_math(pending) {
            return Some(LatexToken {
                raw_len: source.len(),
                raw: source.iter().collect(),
                text: pending.iter().collect(),
                pending: true,
            });
        }
        return None;
    };

    let text: String =
        source.iter().copied().skip(open_len).take(closing_index - open_len).collect();
    if text.is_empty() || text.contains('\n') {
        return None;
    }
    let raw_len = closing_index + close.len();
    Some(LatexToken {
        raw_len,
        raw: source.iter().copied().take(raw_len).collect(),
        text,
        pending: false,
    })
}

/// `/^[A-Z_][A-Z0-9_]*(?:[^A-Za-z0-9_\s])?$/`.
fn is_shouty_ident(inner: &[char]) -> bool {
    let mut it = inner.iter().copied();
    let Some(first) = it.next() else { return false };
    if !(first.is_ascii_uppercase() || first == '_') {
        return false;
    }
    let rest: Vec<char> = it.collect();
    let core_len = rest
        .iter()
        .position(|c| !(c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_'))
        .unwrap_or(rest.len());
    match rest.len() - core_len {
        0 => true,
        1 => rest
            .get(core_len)
            .is_some_and(|c| !c.is_ascii_alphanumeric() && *c != '_' && !c.is_whitespace()),
        _ => false,
    }
}

/// `tokenizeBlockLatex` (`markdown.ts:101-121`).
pub(crate) fn tokenize_block(source: &[char]) -> Option<LatexToken> {
    for (open, close) in [("$$", "$$"), ("\\[", "\\]")] {
        if let Some(t) = block_delimited(source, open, close) {
            return Some(t);
        }
    }
    // `pendingBracket` (`:113-116`) has NO `looksLikePendingDollarMath` guard and no non-empty
    // requirement; `pendingDollar` (`:117-120`) has both.
    if let Some((body_start, _)) = block_open(source, "\\[") {
        let text: String = source.iter().copied().skip(body_start).collect();
        return Some(LatexToken {
            raw_len: source.len(),
            raw: source.iter().collect(),
            text,
            pending: true,
        });
    }
    if let Some((body_start, _)) = block_open(source, "$$") {
        let body: &[char] = source.get(body_start..).unwrap_or(&[]);
        if !body.is_empty() && looks_like_pending_dollar_math(body) {
            return Some(LatexToken {
                raw_len: source.len(),
                raw: source.iter().collect(),
                text: body.iter().collect(),
                pending: true,
            });
        }
    }
    None
}

/// `^ {0,3}<open>[ \t]*(?:\n)?` — returns `(body start, open end)` in char indices.
fn block_open(source: &[char], open: &str) -> Option<(usize, usize)> {
    let mut i = 0usize;
    while i < 3 && source.get(i) == Some(&' ') {
        i += 1;
    }
    if !open.chars().enumerate().all(|(k, c)| source.get(i + k) == Some(&c)) {
        return None;
    }
    let open_end = i + open.chars().count();
    let mut j = open_end;
    while source.get(j).is_some_and(|c| *c == ' ' || *c == '\t') {
        j += 1;
    }
    if source.get(j) == Some(&'\n') {
        j += 1;
    }
    Some((j, open_end))
}

/// `^ {0,3}<open>[ \t]*(?:\n)?([\s\S]*?)<close>[ \t]*(?:\n|$)` — the closed block form.
fn block_delimited(source: &[char], open: &str, close: &str) -> Option<LatexToken> {
    let (body_start, _) = block_open(source, open)?;
    let close_chars: Vec<char> = close.chars().collect();
    let close_at = find_sub(source, &close_chars, body_start)?;
    let body: String = source.iter().copied().skip(body_start).take(close_at - body_start).collect();
    // `dollarMatch?.[1]` / `bracketMatch?.[1]` are truthiness-tested (`:104`, `:108`), so an EMPTY
    // body falls through to the pending arms.
    if body.is_empty() {
        return None;
    }
    let mut end = close_at + close_chars.len();
    while source.get(end).is_some_and(|c| *c == ' ' || *c == '\t') {
        end += 1;
    }
    // `(?:\n|$)` — a trailing newline is part of the raw; anything else and the regex fails.
    if source.get(end) == Some(&'\n') {
        end += 1;
    } else if end != source.len() {
        return None;
    }
    Some(LatexToken {
        raw_len: end,
        raw: source.iter().copied().take(end).collect(),
        text: body.trim().to_string(),
        pending: false,
    })
}

/// Render one token the way its markdown arm does.
///
/// Block: `!pending ? (renderLatex(text, { display: true }) ?? raw.trim()) : raw.trim()`
/// (`markdown.ts:505-512`). Inline: the same without `display` and without the trim
/// (`markdown.ts:645-652`).
pub(crate) fn render_token(token: &LatexToken, display: bool) -> String {
    let fallback = if display { token.raw.trim().to_string() } else { token.raw.clone() };
    if token.pending {
        return fallback;
    }
    render_latex(&token.text, display).unwrap_or(fallback)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::render_latex;

    /// Every `defineCases` pair from `pi/packages/tui/test/latex.test.ts` at v0.84.1, extracted
    /// mechanically. These call `renderLatex(source)` with no options, i.e. inline mode.
    const INLINE_CASES: &[(&str, &str)] = &[
        ("\\mathbb{C}^3 \\to \\mathbb{C}^3", "ℂ³ → ℂ³"),
        ("\\{3x+2y,\\; 27x^2-4z-1,\\; x(x-1)(x+1)\\} \\quad\\Rightarrow\\quad x \\in \\{0, \\pm 1\\},", "{3x+2y, 27x²-4z-1, x(x-1)(x+1)} ⇒ x ∈ {0, ± 1},"),
        ("F_1 = -\\frac{1}{4x^2}.", "F₁ = -1/(4x²)."),
        ("-2", "-2"),
        ("(0,0,-1/4)", "(0,0,-1/4)"),
        ("(1,-3/2,13/2)", "(1,-3/2,13/2)"),
        ("(1,1,1)", "(1,1,1)"),
        ("(2,1,0)", "(2,1,0)"),
        ("(-1/4, 0, 0)", "(-1/4, 0, 0)"),
        ("\\{(0,0,-1/4), (1,-3/2,13/2), (-1,3/2,13/2)\\}", "{(0,0,-1/4), (1,-3/2,13/2), (-1,3/2,13/2)}"),
        ("(2,1,1)", "(2,1,1)"),
        ("(7/3,-2/5,11/7)", "(7/3,-2/5,11/7)"),
        ("\\{y - p(x),\\; q(x)\\}", "{y - p(x), q(x)}"),
        ("\\deg q = 3", "deg q = 3"),
        ("[\\mathbb{C}(x,y,z):\\mathbb{C}(F_1,F_2,F_3)] = 3", "[ℂ(x,y,z):ℂ(F₁,F₂,F₃)] = 3"),
        ("u = 1+xy", "u = 1+xy"),
        ("G = u^2 z + y^2(4+3xy)", "G = u² z + y²(4+3xy)"),
        ("F_1 = uG", "F₁ = uG"),
        ("F_2 = y + 3xG", "F₂ = y + 3xG"),
        ("x=0", "x = 0"),
        ("F_2 = F_3 = 0", "F₂ = F₃ = 0"),
        ("xy = -3/2", "xy = -3/2"),
        ("x^2 z = 13/2", "x² z = 13/2"),
        ("\\mathbb{C}^*", "ℂ^*"),
        ("s \\mapsto (s,\\, -\\tfrac{3}{2s},\\, \\tfrac{13}{2s^2})", "s ↦ (s, -3/(2s), 13/(2s²))"),
        ("X", "X"),
        ("p_\\pm", "p_±"),
        ("F(-x,-y,z) = (F_1, -F_2, -F_3)", "F(-x,-y,z) = (F₁, -F₂, -F₃)"),
        ("p_0", "p₀"),
        ("s \\to \\infty", "s → ∞"),
        ("(0,0,0)", "(0,0,0)"),
        ("\\Rightarrow", "⇒"),
        ("\\ge 2", "≥ 2"),
        ("\\ge 3", "≥ 3"),
        ("1", "1"),
        ("\\mathrm{diag}(-1/2,1,1)", "diag(-1/2,1,1)"),
        ("4+3xy", "4+3xy"),
        ("E \\approx \\frac{0.1\\ \\text{lux}}{100\\ \\text{lm/W}} = 0.001\\ \\text{W/m}^2", "E ≈ (0.1 lux)/(100 lm/W) = 0.001 W/m²"),
        ("\\boxed{1\\ \\text{milliwatt per square metre}}", "[1 milliwatt per square metre]"),
        ("5\\ \\text{km}^2 = 5{,}000{,}000\\ \\text{m}^2", "5 km² = 5,000,000 m²"),
        ("P_{\\text{light}} = 0.001 \\times 5{,}000{,}000\n= \\boxed{5{,}000\\ \\text{W}}", "P_light = 0.001 × 5,000,000 = [5,000 W]"),
        ("P_{\\text{electric}} = 5\\ \\text{kW} \\times 0.2\n= \\boxed{1\\ \\text{kW}}", "P_electric = 5 kW × 0.2 = [1 kW]"),
        ("\\pi(2.5\\ \\text{km})^2 = 19.6\\ \\text{km}^2", "π(2.5 km)² = 19.6 km²"),
        ("0.001\\ \\text{W/m}^2 \\times 19.6 \\times 10^6\\ \\text{m}^2\n\\approx \\boxed{20\\ \\text{kW optical}}", "0.001 W/m² × 19.6 × 10⁶ m² ≈ [20 kW optical]"),
        ("1\\ \\text{kW} \\times \\frac{1}{3600}\\ \\text{hour}\n= \\boxed{0.28\\ \\text{Wh}}", "1 kW × 1/3600 hour = [0.28 Wh]"),
        ("\\det\\!\\left(\\frac{\\partial(F_1,F_2,F_3)}{\\partial(x,y,z)}\\right)=-2.", "det((∂(F₁,F₂,F₃))/(∂(x,y,z))) = -2."),
        ("\\begin{aligned}\nF(0,0,-\\tfrac14)&=(-\\tfrac14,0,0),\\\\\nF(1,-\\tfrac32,\\tfrac{13}2)&=(-\\tfrac14,0,0),\\\\\nF(-1,\\tfrac32,\\tfrac{13}2)&=(-\\tfrac14,0,0).\n\\end{aligned}", "F(0,0,-1/4) = (-1/4,0,0),\nF(1,-3/2,13/2) = (-1/4,0,0),\nF(-1,3/2,13/2) = (-1/4,0,0)."),
        ("F=(F_1,F_2,F_3)", "F = (F₁,F₂,F₃)"),
        ("F", "F"),
        ("3", "3"),
        ("J = \\begin{pmatrix}\n\\frac{\\partial f_1}{\\partial x} & \\frac{\\partial f_1}{\\partial y} & \\frac{\\partial f_1}{\\partial z} \\\\\n\\frac{\\partial f_2}{\\partial x} & \\frac{\\partial f_2}{\\partial y} & \\frac{\\partial f_2}{\\partial z} \\\\\n\\frac{\\partial f_3}{\\partial x} & \\frac{\\partial f_3}{\\partial y} & \\frac{\\partial f_3}{\\partial z}\n\\end{pmatrix}", "J = ⎛ (∂ f₁)/(∂ x) │ (∂ f₁)/(∂ y) │ (∂ f₁)/(∂ z) ⎞\n    ⎜ (∂ f₂)/(∂ x) │ (∂ f₂)/(∂ y) │ (∂ f₂)/(∂ z) ⎟\n    ⎝ (∂ f₃)/(∂ x) │ (∂ f₃)/(∂ y) │ (∂ f₃)/(∂ z) ⎠"),
        ("\\begin{aligned}\nf_1 &= (1+xy)^3 z + y^2(1+xy)(4+3xy) \\\\\nf_2 &= y + 3x(1+xy)^2 z + 3xy^2(4+3xy) \\\\\nf_3 &= 2x - 3x^2y - x^3z\n\\end{aligned}", "f₁ = (1+xy)³ z + y²(1+xy)(4+3xy)\nf₂ = y + 3x(1+xy)² z + 3xy²(4+3xy)\nf₃ = 2x - 3x²y - x³z"),
        ("x, y, z", "x, y, z"),
        ("(x, y, z)", "(x, y, z)"),
        ("(0,\\; 0,\\; -\\tfrac14)", "(0, 0, -1/4)"),
        ("(-\\tfrac14,\\; 0,\\; 0)", "(-1/4, 0, 0)"),
        ("(1,\\; -\\tfrac32,\\; \\tfrac{13}{2})", "(1, -3/2, 13/2)"),
        ("(-1,\\; \\tfrac32,\\; \\tfrac{13}{2})", "(-1, 3/2, 13/2)"),
        ("(-\\frac14, 0, 0)", "(-1/4, 0, 0)"),
        ("F: \\mathbb{C}^3 \\to \\mathbb{C}^3", "F: ℂ³ → ℂ³"),
        ("F(0,0,-\\tfrac14) = F(1,-\\tfrac32,\\tfrac{13}{2}) = F(-1,\\tfrac32,\\tfrac{13}{2}) = (-\\tfrac14, 0, 0)", "F(0,0,-1/4) = F(1,-3/2,13/2) = F(-1,3/2,13/2) = (-1/4, 0, 0)"),
        ("\\mathbb{C}^3", "ℂ³"),
        ("\\begin{aligned}\nf_1 &= \\frac{f_1^{\\text{ut}}(u,t)}{x^2}, \\quad\nf_2 = \\frac{f_2^{\\text{ut}}(u,t)}{x}, \\quad\nf_3 = x\\,(2 - 3u - t)\n\\end{aligned}", "f₁ = (f₁ᵘᵗ(u,t))/(x²), f₂ = (f₂ᵘᵗ(u,t))/x, f₃ = x (2 - 3u - t)"),
        ("\\det J_F", "det J_F"),
        ("(-\\tfrac14, 0, 0)", "(-1/4, 0, 0)"),
        ("u = xy", "u = xy"),
        ("t = x^2z", "t = x²z"),
        ("x \\neq 0", "x ≠ 0"),
        ("f_1^{\\text{ut}}, f_2^{\\text{ut}}", "f₁ᵘᵗ, f₂ᵘᵗ"),
        ("u,t", "u,t"),
        ("x", "x"),
        ("x, x^2", "x, x²"),
        ("\\mathbb{C}^n \\to \\mathbb{C}^n", "ℂⁿ → ℂⁿ"),
        ("n \\geq 2", "n ≥ 2"),
        ("\\mathbb{P}^3", "ℙ³"),
        ("e^{i\\pi}+1=0", "e^(iπ)+1 = 0"),
        ("\\boxed{\n\\mathcal{Z}(\\beta)\n=\n\\int_{\\mathcal M}\n\\exp\\!\\left(\n-\\beta\\left[\n\\frac12 g^{ij}(x)\\,\\partial_i\\phi\\,\\partial_j\\phi\n+V(\\phi)\n\\right]\\right)\n\\mathcal D\\phi\n}", "[Z(β) = ∫_M exp( -β[ 1/2 gⁱʲ(x) ∂ᵢϕ ∂ⱼϕ +V(ϕ) ]) Dϕ]"),
        ("\\begin{aligned}\n\\nabla_\\mu T^{\\mu\\nu}\n&=\n\\frac{1}{\\sqrt{-g}}\n\\partial_\\mu\\!\\left(\\sqrt{-g}\\,T^{\\mu\\nu}\\right)\n+\\Gamma^\\nu_{\\mu\\lambda}T^{\\mu\\lambda}\n=0, \\\\[4pt]\nR_{\\mu\\nu}-\\frac12 Rg_{\\mu\\nu}+\\Lambda g_{\\mu\\nu}\n&=\n\\frac{8\\pi G}{c^4}T_{\\mu\\nu}.\n\\end{aligned}", "∇_μ T^(μν) = 1/(√(-g)) ∂_μ(√(-g) T^(μν)) +Γ^ν_(μλ)T^(μλ) = 0,\nR_(μν)-1/2 Rg_(μν)+Λ g_(μν) = (8π G)/(c⁴)T_(μν)."),
        ("f(z)\n=\n\\frac{1}{2\\pi i}\n\\oint_{\\gamma}\n\\frac{f(\\zeta)}{\\zeta-z}\\,d\\zeta,\n\\qquad\n\\det\\!\\begin{pmatrix}\n\\lambda-a & -b & 0\\\\\n-c & \\lambda-d & -e\\\\\n0 & -f & \\lambda-g\n\\end{pmatrix}\n=0.", "f(z) = 1/(2π i) ∮_γ (f(ζ))/(ζ-z) dζ, det⎛ λ-a │ -b  │ 0   ⎞ = 0.\n                                        ⎜ -c  │ λ-d │ -e  ⎟\n                                        ⎝ 0   │ -f  │ λ-g ⎠"),
        ("\\Psi(x,t)=\n\\sum_{n=1}^{\\infty}\n\\underbrace{\nc_n\n\\sqrt{\\frac{2}{L}}\n\\sin\\!\\left(\\frac{n\\pi x}{L}\\right)\n}_{\\text{spatial eigenmode}}\n\\exp\\!\\left(-\\frac{i\\hbar n^2\\pi^2}{2mL^2}t\\right),\n\\qquad\n|\\Psi(x,t)|^2\n=\n\\begin{cases}\n\\Psi^\\ast\\Psi, & 0<x<L,\\\\\n0, & \\text{otherwise}.\n\\end{cases}", "Ψ(x,t) = ∑ₙ₌₁^∞ cₙ √(2/L) sin((nπ x)/L)_(spatial eigenmode) exp(-(iℏ n²π²)/(2mL²)t), |Ψ(x,t)|² = ⎧ Ψ^∗Ψ if 0 < x < L,\n⎩ 0 otherwise."),
        ("x=\\frac{-b\\pm\\sqrt{b^2-4ac}}{2a}", "x = (-b±√(b²-4ac))/(2a)"),
        ("\\int_0^\\infty e^{-x^2}\\,dx=\\frac{\\sqrt{\\pi}}{2}", "∫₀^∞ e^(-x²) dx = (√π)/2"),
        ("e^{i\\theta}=\\cos\\theta+i\\sin\\theta", "e^(iθ) = cos θ+i sin θ"),
        ("\\sum_{n=1}^{\\infty}\\frac{1}{n^2}=\\frac{\\pi^2}{6}", "∑ₙ₌₁^∞1/(n²) = π²/6"),
        ("\\lim_{x\\to 0}\\frac{\\sin x}{x}=1", "lim[x→0] (sin x)/x = 1"),
        ("\\lim_{n\\to\\infty}\n\\left(1+\\frac{1}{n}\\right)^n=e", "lim[n→∞] (1+1/n)ⁿ = e"),
        ("\\int_0^1 \\frac{x^2}{1+x^3}\\,dx\n=\\frac{1}{3}\\ln 2", "∫₀¹ x²/(1+x³) dx = 1/3 ln 2"),
        ("\\sum_{k=1}^{n}\\frac{k}{k+1}\n=n+1-H_{n+1}", "∑ₖ₌₁ⁿk/(k+1) = n+1-Hₙ₊₁"),
        ("\\frac{\n  \\displaystyle \\frac{x^2+1}{x-1}\n  -\n  \\displaystyle \\frac{2x}{x+1}\n}{\n  \\displaystyle \\frac{x}{x^2-1}\n}", "((x²+1)/(x-1) - 2x/(x+1))/(x/(x²-1))"),
        ("\\lim_{x\\to 0}\n\\frac{\n  \\displaystyle \\frac{\\sin x}{x}-1\n}{\n  \\displaystyle \\frac{e^x-1}{x}-1\n}\n=0", "lim[x→0] ((sin x)/x-1)/((eˣ-1)/x-1) = 0"),
        ("\\frac{\n  1+\\displaystyle\\frac{1}{1+\\frac{1}{x}}\n}{\n  1-\\displaystyle\\frac{1}{1-\\frac{1}{x}}\n}", "(1+1/(1+1/x))/(1-1/(1-1/x))"),
        ("\\sum_{n=1}^{\\infty}\n\\frac{\n  \\displaystyle \\frac{1}{n}-\\frac{1}{n+1}\n}{\n  \\displaystyle 1+\\frac{1}{n^2}\n}", "∑ₙ₌₁^∞ (1/n-1/(n+1))/(1+1/(n²))"),
    ];

    #[test]
    fn matches_pi_inline_expectations() {
        let mut failures: Vec<String> = Vec::new();
        for (source, expected) in INLINE_CASES {
            match render_latex(source, false) {
                Some(actual) if actual == *expected => {}
                other => failures.push(format!("{source:?}\n  want {expected:?}\n  got  {other:?}")),
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} pi latex cases diverge:\n{}",
            failures.len(),
            INLINE_CASES.len(),
            failures.join("\n")
        );
    }

    /// The `it(...)` assertions of `latex.test.ts` that `defineCases` does not cover — the
    /// display-mode stacking cases and the two `undefined` groups.
    #[test]
    fn matches_pi_display_and_rejection_expectations() {
        let display: &[(&str, &str)] = &[
            // "stacks operator limits in display mode"
            (r"\sum_{i=0}^n x_i", " n\n ∑  xᵢ\ni=0"),
            (r"\min_{x\in X} f(x)", "min f(x)\nx∈X"),
            (r"\operatorname*{arg\,max}_{x\in X} f(x)", "arg max f(x)\n  x∈X"),
            (r"\int\nolimits_0^1 f(x)\,dx", "∫₀¹ f(x) dx"),
            (r"\int\limits_0^1 f(x)\,dx", "1\n∫ f(x) dx\n0"),
            // "stacks fractions in display mode"
            (r"x=\frac{-b\pm\sqrt{b^2-4ac}}{2a}", "    -b±√(b²-4ac)\nx = ────────────\n         2a"),
            (r"\frac{x^2+1}{x-1}", "x²+1\n────\nx-1"),
            // "keeps nested display fractions linear"
            (
                r"\frac{\frac{x^2+1}{x-1}-\frac{2x}{x+1}}{\frac{x}{x^2-1}}",
                "(x²+1)/(x-1)-2x/(x+1)\n─────────────────────\n      x/(x²-1)",
            ),
            (
                r"\lim_{x\to 0}\frac{\frac{\sin x}{x}-1}{\frac{e^x-1}{x}-1}=0",
                "     (sin x)/x-1\nlim  ─────────── = 0\nx→0  (eˣ-1)/x-1",
            ),
            (
                r"\frac{1+\frac{1}{1+\frac{1}{x}}}{1-\frac{1}{1-\frac{1}{x}}}",
                "1+1/(1+1/x)\n───────────\n1-1/(1-1/x)",
            ),
            // "keeps fractions linear in scripts and text-style fractions"
            (r"e^{\frac{1}{2}}", "e^(1/2)"),
            (r"\tfrac{1}{2}", "1/2"),
            // "renders matrices with display delimiters"
            (
                "A\\mathbf e_1=\\begin{pmatrix}\\pi\\\\0\\end{pmatrix},\\qquad A\\mathbf e_2=\\begin{pmatrix}0\\\\\\frac{1}{\\pi}\\end{pmatrix}.",
                "Ae₁ = ⎛ π ⎞, Ae₂ = ⎛ 0   ⎞\n      ⎝ 0 ⎠        ⎝ 1/π ⎠.",
            ),
            (
                r"\sum_{i=0}^n x_i=\begin{pmatrix}a&b\\c&d\end{pmatrix}.",
                " n\n ∑  xᵢ = ⎛ a │ b ⎞\ni=0      ⎝ c │ d ⎠.",
            ),
        ];
        let mut failures: Vec<String> = Vec::new();
        for (source, expected) in display {
            match render_latex(source, true) {
                Some(actual) if actual == *expected => {}
                other => failures.push(format!("{source:?}\n  want {expected:?}\n  got  {other:?}")),
            }
        }
        // "normalizes relation, multiplication, and named-operator spacing"
        for source in ["x=y", "x =y", "x=\ny", "x\n=\ny"] {
            let got = render_latex(source, false);
            if got.as_deref() != Some("x = y") {
                failures.push(format!("{source:?} → {got:?}, want \"x = y\""));
            }
        }
        // "uses the middle brace for intermediate case rows"
        let cases = r"\begin{cases}a & x<0 \\ b & x=0 \\ c & x>0\end{cases}";
        let got = render_latex(cases, false);
        if got.as_deref() != Some("⎧ a if x < 0\n⎨ b if x = 0\n⎩ c if x > 0") {
            failures.push(format!("cases → {got:?}"));
        }
        // "returns undefined for unsupported commands" / "for malformed groups and environments"
        for source in [r"x + \unknown{y}", r"\frac{1}{x", "x}", r"\begin{matrix}1 & 2", "x\\"] {
            let got = render_latex(source, false);
            if got.is_some() {
                failures.push(format!("{source:?} must be None, got {got:?}"));
            }
        }
        assert!(failures.is_empty(), "{} divergences:\n{}", failures.len(), failures.join("\n"));
    }
}
