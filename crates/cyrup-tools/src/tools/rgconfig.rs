//! `$RIPGREP_CONFIG_PATH` reader and flag layer for [`crate::tools::grep`].
//!
//! Pi's grep IS ripgrep: it spawns the binary (`grep.ts:226`) with no `env` key and no
//! `--no-config`, so the child inherits `process.env` verbatim and ripgrep reads the user's
//! config on every call. cyrup searches in-process, so the config has to be read here.
//!
//! # This is not a ripgrep CLI emulator
//!
//! ripgrep has 104 flags (`impl Flag for` in `flags/defs.rs` @14.1.0). Reproducing that table —
//! and ripgrep's `unrecognized flag` error with its similarity-suggestion block — would only make
//! cyrup *impersonate* a CLI it is not, and would pin cyrup to one ripgrep version: a user on a
//! newer rg with a newer flag would get a hard error instead of a search. So an unrecognised flag
//! is **ignored**, exactly as an unknown key in any config file is ignored, and the flags the user
//! *did* write still apply.
//!
//! # Parity is the floor, not the target
//!
//! Roughly half of ripgrep's flags cannot be observed through the `--json` pipeline pi consumes
//! (`--max-columns`, `-r/--replace`, the context flags, every colour/format flag, the mode flags):
//! they parse and change nothing. One flag is refused outright — see [`RgFlags::QUIET_IS_REFUSED`].

use ignore::overrides::{Override, OverrideBuilder};
use ignore::types::{Types, TypesBuilder};
use std::path::{Path, PathBuf};

/// Which of ripgrep's three mutually-exclusive case flags was written last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaseMode {
    /// `-s/--case-sensitive`.
    Sensitive,
    /// `-i/--ignore-case`.
    Insensitive,
    /// `-S/--smart-case` — case-insensitive only while the pattern is all lowercase.
    Smart,
}

/// Config settings that reach one of cyrup's three builders.
///
/// Every field maps onto something `grep.rs` already constructs. Fields are `Option` where
/// ripgrep distinguishes "unset" from "explicitly off", because the caller's own tool argument
/// must win over the config (ripgrep's own `config_args ++ CLI args` order, `flags/parse.rs`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RgFlags {
    // --- RegexMatcherBuilder ---
    /// `-i`, `-s` and `-S`, resolved to the LAST one written.
    ///
    /// One field rather than a `case_insensitive` flag beside a `case_smart` flag: ripgrep treats
    /// the three as a single mutually-exclusive group where the last occurrence wins, so as
    /// separate fields a config of `-S` then `-s` kept smart-case switched on even though the
    /// later `-s` should have turned it off. `None` means the config expressed no preference.
    pub case: Option<CaseMode>,
    /// `-w/--word-regexp`.
    pub word: bool,
    /// `-x/--line-regexp`.
    pub whole_line: bool,
    /// `--no-unicode`.
    pub no_unicode: bool,
    /// `--crlf`.
    pub crlf: bool,
    /// `-F/--fixed-strings`.
    pub fixed_strings: bool,

    // --- SearcherBuilder ---
    /// `-v/--invert-match`.
    pub invert_match: bool,
    /// `-m/--max-count` — per-file match cap.
    pub max_count: Option<u64>,
    /// `-E/--encoding`. Declaring an encoding the files do not use and then finding nothing is
    /// *correct* ripgrep behaviour, not a defect — it is honoured.
    pub encoding: Option<String>,
    /// `-a/--text` / `--binary` — search binary files rather than quitting at the first NUL.
    pub text: bool,

    // --- glob overrides ---
    /// `-g/--glob` and `--iglob`, in the order written, each paired with whether it is
    /// case-insensitive (`--iglob`) or not (`-g`).
    ///
    /// One ordered list rather than two, because `ignore`'s `Override` is last-match-wins: with
    /// the two kinds bucketed separately, `--iglob '*.RS'` followed by `-g '!vendor/**'` would
    /// evaluate in the wrong order and the negation would lose. `OverrideBuilder::case_insensitive`
    /// is a setter that applies to subsequent `add` calls, so preserving the order here is enough
    /// to preserve the semantics.
    pub globs: Vec<(String, bool)>,

    // --- ignore::types ---
    /// `--type-add` definitions, in order.
    pub type_adds: Vec<String>,
    /// `-t/--type` selections.
    pub types: Vec<String>,
    /// `-T/--type-not` exclusions.
    pub types_not: Vec<String>,

    /// `--ignore-file` values, in order — extra gitignore-format files to apply to the walk.
    ///
    /// Held as written, as strings. These are unresolved config text, not paths yet: they may be
    /// relative, and the directory they are relative TO is the tool's cwd, which this module does
    /// not know. `grep.rs` converts them once, where that cwd is in hand — so the `PathBuf` is
    /// built at the point it becomes meaningful rather than built here and taken apart again.
    ///
    /// NOT an override: an ignore file's patterns EXCLUDE, whereas a plain `Override` glob makes
    /// everything it does not match excluded instead. They are different matchers with opposite
    /// defaults, so this feeds `WalkBuilder::add_ignore` rather than the override.
    pub ignore_files: Vec<String>,

    // --- walk ---
    /// `--no-ignore` and the `-u` ladder: every ignore source off.
    pub no_ignore: bool,
    /// `--no-ignore-vcs` — gitignore family off, `.ignore` still honoured.
    pub no_ignore_vcs: bool,
    /// `--max-depth`/`--maxdepth`.
    pub max_depth: Option<usize>,
    /// `-L/--follow`.
    pub follow: bool,
    /// `--one-file-system`.
    pub one_file_system: bool,
    /// `--max-filesize`, in bytes, with ripgrep's `K`/`M`/`G` suffixes resolved.
    pub max_filesize: Option<u64>,
    /// `--sort=path` / `--sortr=path` — deterministic path order, and which DIRECTION.
    ///
    /// A direction rather than a bool: `--sortr=path` is ripgrep's REVERSE order, and collapsing
    /// it onto the same ascending sort silently returned the opposite result set. `grep` truncates
    /// at `limit`, so traversal order decides which matches the caller ever sees — reversing it
    /// changes the answer rather than merely the order of one.
    pub sort_path: Option<crate::ops::PathSort>,

    /// How many times `-u` was given, across the whole config — `-uu` and `-u` twice are the same
    /// thing to ripgrep, so this counts occurrences rather than per-line repeats.
    ///
    /// The ladder is cumulative, not three separate switches: `-u` is `--no-ignore`, `-uu` adds
    /// `--hidden`, `-uuu` adds `--binary`. [`RgFlags::parse`] resolves it into the plain fields
    /// once the whole file has been read, because the level is only known at the end.
    pub u_level: u32,
}

impl RgFlags {
    /// Why `-q/--quiet` is parsed and then deliberately dropped.
    ///
    /// `-q` makes ripgrep's `HiArgs::printer` return `Printer::Summary(SummaryKind::Quiet)`
    /// **before the JSON arm is reached** (`flags/hiargs.rs:565` @14.1.0). Under pi that means rg
    /// finds matches and exits 0 while emitting no `type:"match"` events, so pi's line handler sees
    /// none and answers `No matches found` with matches present — a silent wrong answer.
    ///
    /// cyrup gets this right today by ignoring the config wholesale, and must keep getting it right
    /// now that the config is read. Honouring `-q` would import pi's defect. Parity is the floor,
    /// not the target: where pi is wrong, do not port the defect.
    // The const is the citation's anchor: `quiet_in_config_is_ignored_and_matches_are_still_returned`
    // asserts on it, so the `hiargs.rs:565` reference cannot be dropped without a test failing.
    // Nothing in the production path consumes the string — `-q` is dropped silently, which is the
    // whole point — so outside `cfg(test)` it is deliberately inert.
    #[cfg_attr(not(test), allow(dead_code))]
    pub const QUIET_IS_REFUSED: &'static str =
        "-q/--quiet suppresses ripgrep's JSON match events (hiargs.rs:565); honouring it would \
         reproduce pi's silent 'No matches found' with matches present";

    /// Read and parse the config named by `path`.
    ///
    /// A missing, unreadable or non-UTF-8 file yields [`RgFlags::default()`] — ripgrep only warns
    /// on stderr in that case and searches anyway, and pi discards rg's stderr on a zero exit.
    pub fn read(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::default(),
        }
    }

    /// Parse config text: one argument per line, `#` comments stripped, each line trimmed.
    ///
    /// This is ripgrep's own format (`flags/config.rs` @14.1.0) — it is deliberately NOT a shell
    /// grammar, so there is no quoting, splitting or escaping to reproduce. A flag that takes a
    /// value accepts it either as `--flag=value` on one line or as the *next* line.
    pub fn parse(text: &str) -> Self {
        let args: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();

        let mut f = Self::default();
        let mut i = 0usize;
        while let Some(arg) = args.get(i) {
            i += 1;
            // `--flag` / `--flag=value`
            if let Some(long) = arg.strip_prefix("--") {
                let (name, inline) = match long.split_once('=') {
                    Some((n, v)) => (n, Some(v.to_string())),
                    None => (long, None),
                };
                let mut take = || -> Option<String> {
                    if let Some(v) = inline.clone() {
                        return Some(v);
                    }
                    let v = args.get(i).map(|s| (*s).to_string());
                    if v.is_some() {
                        i += 1;
                    }
                    v
                };
                f.apply_long(name, &mut take);
                continue;
            }
            // `-x`, `-xVALUE`, and clustered shorts like `-iF`
            if let Some(shorts) = arg.strip_prefix('-')
                && !shorts.is_empty()
            {
                let mut it = shorts.chars();
                while let Some(ch) = it.next() {
                    // Value-taking shorts consume the rest of the cluster, else the next line.
                    if matches!(ch, 'm' | 'E' | 'g' | 't' | 'T') {
                        let tail: String = it.by_ref().collect();
                        let v = if tail.is_empty() {
                            let v = args.get(i).map(|s| (*s).to_string());
                            if v.is_some() {
                                i += 1;
                            }
                            v
                        } else {
                            Some(tail)
                        };
                        f.apply_short_with_value(ch, v);
                    } else {
                        f.apply_short(ch);
                    }
                }
            }
            // A bare word is a PATH argument in ripgrep, not a flag. cyrup takes its search root
            // from the tool call, so a path in the config is ignored rather than allowed to
            // silently redirect the search.
        }
        f.resolve_u_ladder();
        f
    }

    /// Fold the `-u` count into the flags it stands for.
    ///
    /// `-u` = `--no-ignore`; `-uu` = that plus `--hidden`; `-uuu` = that plus `--binary`. Only the
    /// first and third have anything to do here: `grep` already walks with `include_hidden: true`
    /// unconditionally (pi passes `--hidden`, `grep.ts:215-219`), so the `-uu` rung is already
    /// satisfied before the config is read.
    fn resolve_u_ladder(&mut self) {
        if self.u_level >= 1 {
            self.no_ignore = true;
        }
        if self.u_level >= 3 {
            self.text = true;
        }
    }

    fn apply_long(&mut self, name: &str, take: &mut impl FnMut() -> Option<String>) {
        match name {
            "ignore-case" => self.case = Some(CaseMode::Insensitive),
            "case-sensitive" => self.case = Some(CaseMode::Sensitive),
            "smart-case" => self.case = Some(CaseMode::Smart),
            "word-regexp" => self.word = true,
            "line-regexp" => self.whole_line = true,
            "no-unicode" => self.no_unicode = true,
            "crlf" => self.crlf = true,
            "fixed-strings" => self.fixed_strings = true,
            "invert-match" => self.invert_match = true,
            "text" | "binary" => self.text = true,
            "max-count" => self.max_count = take().and_then(|v| v.parse().ok()),
            "encoding" => self.encoding = take(),
            "glob" => self.globs.extend(take().map(|g| (g, false))),
            "iglob" => self.globs.extend(take().map(|g| (g, true))),
            "type-add" => self.type_adds.extend(take()),
            "type" => self.types.extend(take()),
            "type-not" => self.types_not.extend(take()),
            "ignore-file" => self.ignore_files.extend(take()),
            "no-ignore" => self.no_ignore = true,
            "no-ignore-vcs" => self.no_ignore_vcs = true,
            "max-depth" | "maxdepth" => self.max_depth = take().and_then(|v| v.parse().ok()),
            "follow" => self.follow = true,
            "one-file-system" => self.one_file_system = true,
            "max-filesize" => self.max_filesize = take().as_deref().and_then(parse_size),
            // `--sort` ascends, `--sortr` descends. Any sort KEY other than `path` (ripgrep also
            // has `modified`, `accessed`, `created`) is one this walk cannot produce, so it clears
            // the setting instead of leaving an earlier one standing — last occurrence wins, which
            // is ripgrep's rule for the group.
            "sort" | "sortr" => {
                let dir = if name == "sortr" {
                    crate::ops::PathSort::Descending
                } else {
                    crate::ops::PathSort::Ascending
                };
                self.sort_path = (take().as_deref() == Some("path")).then_some(dir);
            }
            // `--quiet` is recognised so it does not fall through as unknown, and then dropped.
            // See `QUIET_IS_REFUSED`.
            "quiet" => {}
            // Everything else — inert through the JSON pipeline, or a flag from a ripgrep newer
            // than the one this was written against. Ignored, never fatal.
            _ => {}
        }
    }

    fn apply_short(&mut self, ch: char) {
        match ch {
            'i' => self.case = Some(CaseMode::Insensitive),
            's' => self.case = Some(CaseMode::Sensitive),
            'S' => self.case = Some(CaseMode::Smart),
            'w' => self.word = true,
            'x' => self.whole_line = true,
            'F' => self.fixed_strings = true,
            'v' => self.invert_match = true,
            'a' => self.text = true,
            'L' => self.follow = true,
            // Counted, not applied: the ladder is cumulative and its level is only known once
            // the whole config has been read. Resolved at the end of `parse`.
            'u' => self.u_level = self.u_level.saturating_add(1),
            'q' => {}
            _ => {}
        }
    }

    fn apply_short_with_value(&mut self, ch: char, v: Option<String>) {
        match ch {
            'm' => self.max_count = v.and_then(|v| v.parse().ok()),
            'E' => self.encoding = v,
            'g' => self.globs.extend(v.map(|g| (g, false))),
            't' => self.types.extend(v),
            'T' => self.types_not.extend(v),
            _ => {}
        }
    }
}

/// ripgrep's `--max-filesize` argument: a decimal with an optional `K`/`M`/`G` suffix.
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (digits, mult) = if let Some(d) = s.strip_suffix(['K', 'k']) {
        (d, 1024u64)
    } else if let Some(d) = s.strip_suffix(['M', 'm']) {
        (d, 1024 * 1024)
    } else if let Some(d) = s.strip_suffix(['G', 'g']) {
        (d, 1024 * 1024 * 1024)
    } else {
        (s, 1)
    };
    // `checked_mul`, not `*`: `--max-filesize=18446744073709551615K` overflows u64. Unchecked,
    // that PANICS under the workspace's dev profile (overflow-checks are on) and silently WRAPS in
    // release (`[profile.release]` leaves them off), yielding a tiny cap that quietly drops files
    // from the search with nothing in the output to say so. An overflowing value is unusable
    // either way, so it is dropped like any other unparseable one — the module's contract is that
    // a bad config never makes the tool fail.
    digits.trim().parse::<u64>().ok().and_then(|n| n.checked_mul(mult))
}

/// Resolve `$RIPGREP_CONFIG_PATH` the way ripgrep does.
///
/// Exactly one source — there is no `~/.ripgreprc` and no XDG path (`flags/config.rs:17`
/// @14.1.0). An empty value counts as unset.
pub(crate) fn config_path_from_env() -> Option<PathBuf> {
    let v = std::env::var_os("RIPGREP_CONFIG_PATH")?;
    if v.is_empty() {
        return None;
    }
    Some(PathBuf::from(v))
}

/// Compile the config's `-g/--glob`/`--iglob` list into an [`Override`].
///
/// `None` when the config named no globs, so the caller keeps its existing fast path untouched.
/// A glob that does not compile is DROPPED, not fatal — same rule as an unrecognised flag: one
/// bad line in a config must not turn every search into an error.
pub(crate) fn build_override(root: &Path, flags: &RgFlags) -> Option<Override> {
    if flags.globs.is_empty() {
        return None;
    }
    let mut b = OverrideBuilder::new(root);
    // Tracked so `case_insensitive` is only toggled when it actually changes: the setter is
    // fallible, and calling it per glob would discard globs behind a spurious error.
    let mut insensitive = false;
    for (pattern, want_insensitive) in &flags.globs {
        if *want_insensitive != insensitive {
            if b.case_insensitive(*want_insensitive).is_err() {
                continue;
            }
            insensitive = *want_insensitive;
        }
        let _ = b.add(pattern);
    }
    b.build().ok()
}

/// Compile the config's `--type-add`/`-t`/`-T` list into a [`Types`] matcher.
///
/// `add_defaults` first, because `-t rust` is meaningless without ripgrep's built-in definitions
/// and `--type-add` is documented as extending them rather than replacing them.
pub(crate) fn build_types(flags: &RgFlags) -> Option<Types> {
    if flags.type_adds.is_empty() && flags.types.is_empty() && flags.types_not.is_empty() {
        return None;
    }
    let mut b = TypesBuilder::new();
    b.add_defaults();
    for def in &flags.type_adds {
        let _ = b.add_def(def);
    }

    // Select and negate only names that actually resolve.
    //
    // `TypesBuilder::build` fails with `unrecognized file type: <name>` if ANY selection names a
    // type that does not exist, and that failure takes the whole matcher down — so a single
    // malformed `--type-add` (dropped just above) followed by a `-t` for the name it tried to
    // define also discarded a perfectly good `-t rust` written beside it. One bad line disabling
    // a feature the user configured correctly elsewhere is exactly what this module's contract
    // says must not happen: an unusable entry is dropped, and the flags that ARE usable still
    // apply.
    //
    // `all` is not in `definitions()` but is valid — `select`/`negate` expand it over every type
    // currently defined (ignore `types.rs:385-396`), so it is admitted explicitly.
    let known: std::collections::HashSet<String> =
        b.definitions().iter().map(|d| d.name().to_string()).collect();
    let resolves = |name: &String| name == "all" || known.contains(name);
    for t in flags.types.iter().filter(|t| resolves(t)) {
        b.select(t);
    }
    for t in flags.types_not.iter().filter(|t| resolves(t)) {
        b.negate(t);
    }
    b.build().ok()
}
