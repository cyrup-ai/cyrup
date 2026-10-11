//! ICU4C's word-break answers for Chinese/Japanese text, rebuilt over ICU4X's data.
//!
//! [CYRUP-DELTA] relative to ICU4X (upward: cyrup matches pi, whose `Intl.Segmenter` is the
//! ICU4C inside node — 77.1 in node 22.22.0). ICU4X 2.3.0's `WordSegmenter::new_auto` is not the
//! same algorithm as ICU4C for unspaced CJ text, in two ways this module undoes (EXT-113):
//!
//! 1. **Boundaries.** ICU4X splits a CJ run with a greedy longest match over the `cjdict` trie
//!    (`icu_segmenter-2.3.0/src/complex/dictionary.rs`, which ignores the trie's values) and
//!    merges consecutive unknown characters. ICU4C's `CjkBreakEngine::divideUpDictionaryRange`
//!    (`icu4c/source/common/dictbe.cpp` @release-77-1) runs a dynamic programme over the SAME
//!    trie's values — the summed cost ("snlp") of a segmentation is minimised, an unknown
//!    character is a one-character word of cost 255, and a Katakana run is offered as one word at
//!    `getKatakanaCost(length)`. So `います` is `い`/`ます` in node and `いま`/`す` under ICU4X, and
//!    `々々` is two words in node and one under ICU4X. [`cj_breaks`] is that programme, over
//!    ICU4X's own `cjdict` payload (the same `UCharsTrie` ICU4C reads, values included).
//!
//! 2. **`isWordLike`.** ICU4X tags every segment its dictionary pass produces `WordType::Letter`
//!    (`ComplexWord::BREAK_STATUS`, `icu_segmenter-2.3.0/src/word.rs`). ICU4C instead gives every
//!    dictionary-subdivided segment the status of the RULE-pass segment it came from
//!    (`DictionaryCache::populateDictionary` / `following` in `rbbi_cache.cpp`: every boundary
//!    after the first gets `fOtherRuleStatusIndex`, the rule status at the end of the range).
//!    `word.txt` @release-77-1 decides that status: `$KanaKanji $KanaKanji {400}` chains a run of
//!    `[$Han $Hiragana $Katakana]`, so a run of two or more is word-like; a run of one is word-like
//!    only through `$Katakana $ExFm* {400}`, `$Hiragana $ExFm* {400}` or
//!    `$Ideographic $ExFm* {400}`. `々` and `〻` are `Script=Han` but `Ideographic=No`, so alone they
//!    match no status rule (0, not word-like), while `〆` (`Script=Common`, `Ideographic=Yes`) is
//!    word-like alone — ICU4X has all three backwards.
//!
//! Plus one non-CJ status fix of the same kind: a lone `ExtendNumLet` (`_`, `＿`, `‿`) matches only
//! `.;` in `word.txt` (the `{200}` rules 13a/13b need a neighbour), so it is not word-like; ICU4X
//! says it is.
//!
//! The rule-pass model is deliberately narrow — exactly the `word.txt` rules that touch
//! `$KanaKanji`: rule 4 (`$ExFm*` attaches to the preceding character), rule 13
//! (`$Katakana $ExFm* $Katakana`) and the CJK chain. A run that touches an `ExtendNumLet` (rules
//! 13a/13b fold Katakana into a letter word) is left to ICU4X.

use std::sync::OnceLock;

use icu_collections::char16trie::{Char16Trie, TrieResult};
use icu_normalizer::{ComposingNormalizerBorrowed, DecomposingNormalizerBorrowed};
use icu_properties::props::{CanonicalCombiningClass, Ideographic, Script, WordBreak};
use icu_properties::{CodePointMapData, CodePointSetData};
use icu_provider::prelude::{
    DataIdentifierBorrowed, DataMarkerAttributes, DataProvider, DataRequest, DataRequestMetadata,
};
use icu_segmenter::provider::{Baked, SegmenterDictionaryAutoV1};

/// `maxWordSize` (`dictbe.cpp`, `divideUpDictionaryRange`): a dictionary match is at most this
/// many UTF-16 code units long.
const MAX_WORD_UNITS: usize = 20;
/// `maxSnlp`: the cost of a character the dictionary has no one-character word for.
const MAX_SNLP: u32 = 255;
/// `kMaxKatakanaGroupLength`: a Katakana run this long or longer is not offered as one word.
const MAX_KATAKANA_GROUP: usize = 20;

/// `getKatakanaCost(wordLength)` (`dictbe.cpp`).
fn katakana_cost(len: usize) -> u32 {
    const COST: [u32; 9] = [8192, 984, 408, 240, 204, 252, 300, 372, 480];
    COST.get(len).copied().unwrap_or(8192)
}

/// `isKatakana(value)` (`dictbe.cpp`) — over the NFKC-normalised text the programme reads.
fn is_katakana_cost_char(c: char) -> bool {
    let v = u32::from(c);
    ((0x30A1..=0x30FE).contains(&v) && v != 0x30FB) || (0xFF66..=0xFF9F).contains(&v)
}

/// The `cjdict` trie, from ICU4X's baked data, loaded once.
fn cj_trie() -> Option<&'static Char16Trie<'static>> {
    static TRIE: OnceLock<Option<Char16Trie<'static>>> = OnceLock::new();
    TRIE.get_or_init(|| {
        let mut metadata = DataRequestMetadata::default();
        metadata.silent = true;
        metadata.attributes_prefix_match = true;
        let response = DataProvider::<SegmenterDictionaryAutoV1>::load(
            &Baked,
            DataRequest {
                id: DataIdentifierBorrowed::for_marker_attributes(
                    DataMarkerAttributes::from_str_or_panic("cjdict"),
                ),
                metadata,
            },
        )
        .ok()?;
        let data = response.payload.get_static()?;
        Some(Char16Trie::new(data.trie_data.clone()))
    })
    .as_ref()
}

/// The character classes of `word.txt` this module needs.
#[derive(Clone, Copy)]
struct Class {
    /// `$KanaKanji = [$Han $Hiragana $Katakana]` (`$Katakana` is `Word_Break=Katakana`).
    kana_kanji: bool,
    /// `$Katakana`.
    katakana: bool,
    /// `$ExFm = [$Extend $Format $ZWJ]`, with `$Extend = [\p{Word_Break = Extend}-$Han]`.
    ex_fm: bool,
    /// `$ExtendNumLet`.
    extend_num_let: bool,
    /// The single-character `{400}` rules: `$Katakana`, `$Hiragana` or `$Ideographic`.
    status_400_alone: bool,
    /// `Ideographic=Yes`.
    ideographic: bool,
    /// The `Script` value (what `UnhandledEngine::handleCharacter` groups by).
    script: Script,
    /// The CJ break engine's set, `[[:Han:][:Hiragana:][:Katakana:]ーｰﾞﾟ]`
    /// (`CjkBreakEngine::CjkBreakEngine`).
    cj_engine: bool,
}

fn classify(c: char) -> Class {
    let script = CodePointMapData::<Script>::new().get(c);
    let wb = CodePointMapData::<WordBreak>::new().get(c);
    let ideographic = CodePointSetData::new::<Ideographic>().contains(c);
    let han = script == Script::Han;
    let hiragana = script == Script::Hiragana;
    let katakana = wb == WordBreak::Katakana;
    Class {
        kana_kanji: han || hiragana || katakana,
        katakana,
        ex_fm: (wb == WordBreak::Extend && !han) || wb == WordBreak::Format || wb == WordBreak::ZWJ,
        extend_num_let: wb == WordBreak::ExtendNumLet,
        status_400_alone: katakana || hiragana || ideographic,
        ideographic,
        script,
        cj_engine: han
            || hiragana
            || script == Script::Katakana
            || matches!(u32::from(c), 0x30FC | 0xFF70 | 0xFF9E | 0xFF9F),
    }
}

/// One rule-pass `$KanaKanji` segment: chars `[start, end)`, and its status.
struct Run {
    start: usize,
    end: usize,
    word_like: bool,
}

/// The `$KanaKanji` rule segments of `chars`, as ICU4C's rule pass would end them.
fn kana_kanji_runs(classes: &[Class]) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut i = 0usize;
    while let Some(first) = classes.get(i) {
        if !first.kana_kanji {
            i += 1;
            continue;
        }
        let start = i;
        let mut last_kk = i;
        let mut kk_count = 1usize;
        let mut j = i + 1;
        loop {
            // Rule 4: `$ExFm*` after the last character.
            let mut k = j;
            while classes.get(k).is_some_and(|c| c.ex_fm) {
                k += 1;
            }
            let Some(next) = classes.get(k) else {
                j = k;
                break;
            };
            let last_is_katakana = classes.get(last_kk).is_some_and(|c| c.katakana);
            // `$KanaKanji $KanaKanji` (adjacent) or `$Katakana $ExFm* $Katakana`.
            let chains = next.kana_kanji && (k == j || (last_is_katakana && next.katakana));
            if chains {
                last_kk = k;
                kk_count += 1;
                j = k + 1;
            } else {
                j = k;
                break;
            }
        }
        // Rules 13a/13b would fold this run into an `ExtendNumLet` word; leave it to ICU4X.
        let touches_extend_num_let = start
            .checked_sub(1)
            .and_then(|p| {
                let mut p = p;
                while classes.get(p).is_some_and(|c| c.ex_fm) {
                    p = p.checked_sub(1)?;
                }
                classes.get(p)
            })
            .is_some_and(|c| c.extend_num_let)
            || classes.get(j).is_some_and(|c| c.extend_num_let);
        if !touches_extend_num_let {
            let trailing_ex_fm = j > last_kk + 1;
            let word_like = if trailing_ex_fm || kk_count == 1 {
                classes.get(last_kk).is_some_and(|c| c.status_400_alone)
            } else {
                true
            };
            runs.push(Run {
                start,
                end: j,
                word_like,
            });
        }
        i = j.max(i + 1);
    }
    runs
}

/// `nfkcNorm2->hasBoundaryBefore(c)`, as far as this needs it: `c`'s NFKD decomposition starts
/// with a starter (canonical combining class 0). The caller also requires that normalising the
/// chunk with `c` equals normalising the two apart, which rules out a starter that composes
/// backwards.
fn has_boundary_before(c: char) -> bool {
    let nfkd = DecomposingNormalizerBorrowed::new_nfkd();
    let mut buffer = [0; 4];
    nfkd.normalize(c.encode_utf8(&mut buffer))
        .chars()
        .next()
        .is_none_or(|first| {
            CodePointMapData::<CanonicalCombiningClass>::new().get(first)
                == CanonicalCombiningClass::NotReordered
        })
}

/// The interior breaks `divideUpDictionaryRange` finds in one CJ-engine range, as char indices
/// into `chars` (never `0`, never `chars.len()`: the range start is not pushed and a break at the
/// range end is popped).
fn cj_breaks(trie: &Char16Trie<'_>, chars: &[char]) -> Vec<usize> {
    let n = chars.len();
    // NFKC, when the range is not already normalised: chunks split where the next character
    // starts a fresh normalisation (`hasBoundaryBefore`), and every normalised character maps
    // back to the start of its chunk.
    let nfkc = ComposingNormalizerBorrowed::new_nfkc();
    let original: String = chars.iter().collect();
    let (norm, map): (Vec<char>, Vec<usize>) = if nfkc.is_normalized(&original) {
        (chars.to_vec(), (0..=n).collect())
    } else {
        let mut norm = Vec::new();
        let mut map = Vec::new();
        let mut chunk = String::new();
        let mut chunk_start = 0usize;
        let flush = |chunk: &str, start: usize, norm: &mut Vec<char>, map: &mut Vec<usize>| {
            for c in nfkc.normalize(chunk).chars() {
                norm.push(c);
                map.push(start);
            }
        };
        for (idx, &c) in chars.iter().enumerate() {
            if !chunk.is_empty() {
                let mut joined = chunk.clone();
                joined.push(c);
                let apart = format!(
                    "{}{}",
                    nfkc.normalize(&chunk),
                    nfkc.normalize(c.encode_utf8(&mut [0; 4]))
                );
                if has_boundary_before(c) && nfkc.normalize(&joined) == apart {
                    flush(&chunk, chunk_start, &mut norm, &mut map);
                    chunk.clear();
                    chunk_start = idx;
                }
            }
            chunk.push(c);
        }
        flush(&chunk, chunk_start, &mut norm, &mut map);
        map.push(n);
        (norm, map)
    };

    let m = norm.len();
    let mut best: Vec<Option<u32>> = vec![None; m + 1];
    let mut prev: Vec<Option<usize>> = vec![None; m + 1];
    if let Some(first) = best.first_mut() {
        *first = Some(0);
    }
    let relax = |best: &mut Vec<Option<u32>>,
                 prev: &mut Vec<Option<usize>>,
                 from: usize,
                 to: usize,
                 cost: u32| {
        if let (Some(slot), Some(back)) = (best.get_mut(to), prev.get_mut(to))
            && slot.is_none_or(|b| cost < b)
        {
            *slot = Some(cost);
            *back = Some(from);
        }
    };
    let mut is_prev_katakana = false;
    for i in 0..m {
        let Some(base) = best.get(i).copied().flatten() else {
            continue;
        };
        // `fDictionary->matches(...)`: every prefix the trie has a value for, in code points.
        let mut words: Vec<(usize, u32)> = Vec::new();
        let mut iter = trie.iter();
        let mut units = 0usize;
        for (k, &c) in norm.iter().skip(i).enumerate() {
            let result = iter.next(c);
            units += c.len_utf16();
            match result {
                TrieResult::Intermediate(v) => {
                    words.push((k + 1, u32::try_from(v).unwrap_or(u32::MAX)));
                }
                TrieResult::FinalValue(v) => {
                    words.push((k + 1, u32::try_from(v).unwrap_or(u32::MAX)));
                    break;
                }
                TrieResult::NoMatch => break,
                TrieResult::NoValue => {}
            }
            if units >= MAX_WORD_UNITS {
                break;
            }
        }
        if words.first().is_none_or(|&(len, _)| len != 1) {
            words.push((1, MAX_SNLP));
        }
        for (len, value) in words {
            relax(&mut best, &mut prev, i, i + len, base.saturating_add(value));
        }
        let is_katakana = norm.get(i).is_some_and(|&c| is_katakana_cost_char(c));
        if !is_prev_katakana && is_katakana {
            let mut run = 1usize;
            while run < MAX_KATAKANA_GROUP
                && norm.get(i + run).is_some_and(|&c| is_katakana_cost_char(c))
            {
                run += 1;
            }
            if run < MAX_KATAKANA_GROUP {
                relax(
                    &mut best,
                    &mut prev,
                    i,
                    i + run,
                    base.saturating_add(katakana_cost(run)),
                );
            }
        }
        is_prev_katakana = is_katakana;
    }

    // Walk the best segmentation back from the end, then map to original indices ascending.
    let mut cuts = vec![m];
    let mut at = m;
    while at > 0 {
        let Some(p) = prev.get(at).copied().flatten() else {
            break;
        };
        cuts.push(p);
        at = p;
    }
    if cuts.last() != Some(&0) {
        cuts.push(0);
    }
    let mut out: Vec<usize> = Vec::new();
    let mut last_pos: Option<usize> = None;
    for &cut in cuts.iter().rev() {
        let pos = map.get(cut).copied().unwrap_or(n);
        if last_pos.is_some_and(|l| pos <= l) {
            continue;
        }
        last_pos = Some(pos);
        if pos != 0 {
            out.push(pos);
        }
    }
    if out.last() == Some(&n) {
        out.pop();
    }
    out
}

/// ICU4X's `(byte_start, byte_len, word_like)` segments of `text`, corrected to ICU4C's.
pub(super) fn reconcile(text: &str, icu4x: Vec<(usize, usize, bool)>) -> Vec<(usize, usize, bool)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let classes: Vec<Class> = chars.iter().map(|&(_, c)| classify(c)).collect();
    let byte_at = |i: usize| chars.get(i).map_or(text.len(), |&(b, _)| b);

    // (byte_start, byte_end, word_like) for every segment inside a `$KanaKanji` run.
    let mut run_segments: Vec<(usize, usize, bool)> = Vec::new();
    if let Some(trie) = cj_trie() {
        for run in kana_kanji_runs(&classes) {
            let run_chars: Vec<char> = chars
                .get(run.start..run.end)
                .unwrap_or_default()
                .iter()
                .map(|&(_, c)| c)
                .collect();
            let mut breaks: Vec<usize> = Vec::new();
            // `populateDictionary` returns at once for a range of one UTF-16 unit.
            if run_chars.iter().map(|c| c.len_utf16()).sum::<usize>() > 1 {
                let mut k = 0usize;
                while let Some(class) = classes.get(run.start + k).filter(|_| k < run_chars.len()) {
                    if class.ex_fm {
                        // Not a dictionary character: `populateDictionary` steps over it.
                        k += 1;
                    } else if class.cj_engine {
                        // The CJ engine takes the run of its own set (`findBreaks`).
                        let mut e = k;
                        while e < run_chars.len()
                            && classes.get(run.start + e).is_some_and(|c| c.cj_engine)
                        {
                            e += 1;
                        }
                        let sub = run_chars.get(k..e).unwrap_or_default();
                        breaks.extend(cj_breaks(trie, sub).into_iter().map(|b| run.start + k + b));
                        k = e;
                    } else {
                        // A dictionary character no engine handles (`〱`, `゛`, `゠`): the
                        // `UnhandledEngine` swallows the run of that character's Script
                        // (`handleCharacter` applies `UCHAR_SCRIPT`), finding no breaks — so a
                        // following `ー` (Script=Common) goes with it, not to the CJ engine.
                        let script = class.script;
                        k += 1;
                        while k < run_chars.len()
                            && classes
                                .get(run.start + k)
                                .is_some_and(|c| c.script == script)
                        {
                            k += 1;
                        }
                    }
                }
            }
            let mut from = run.start;
            for cut in breaks.into_iter().chain(std::iter::once(run.end)) {
                if cut > from {
                    run_segments.push((byte_at(from), byte_at(cut), run.word_like));
                    from = cut;
                }
            }
        }
    }

    let in_run = |b: usize| run_segments.iter().any(|&(s, e, _)| s < b && b < e);
    let mut boundaries: Vec<usize> = icu4x
        .iter()
        .flat_map(|&(s, l, _)| [s, s.saturating_add(l)])
        .filter(|&b| !in_run(b))
        .chain(run_segments.iter().flat_map(|&(s, e, _)| [s, e]))
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut out = Vec::new();
    for pair in boundaries.windows(2) {
        let (Some(&a), Some(&b)) = (pair.first(), pair.get(1)) else {
            continue;
        };
        let word_like =
            if let Some(&(_, _, w)) = run_segments.iter().find(|&&(s, e, _)| s <= a && b <= e) {
                w
            } else {
                let icu4x_word_like = icu4x
                    .iter()
                    .find(|&&(s, l, _)| s <= a && a < s.saturating_add(l))
                    .is_some_and(|&(_, _, w)| w);
                lone_character_status(text.get(a..b).unwrap_or_default()).unwrap_or(icu4x_word_like)
            };
        out.push((a, b.saturating_sub(a), word_like));
    }
    out
}

/// `word.txt`'s status for a segment that is ONE character plus `$ExFm*`, where it differs from
/// ICU4X's: a lone `$ExtendNumLet` matches only `.;` (not word-like), and a lone non-`$KanaKanji`
/// `$Ideographic` (`〆`) matches `$Ideographic $ExFm* {400}` (word-like). `None` otherwise.
fn lone_character_status(segment: &str) -> Option<bool> {
    let mut it = segment.chars();
    let base = classify(it.next()?);
    if !it.all(|c| classify(c).ex_fm) {
        return None;
    }
    if base.extend_num_let {
        Some(false)
    } else if base.ideographic && !base.kana_kanji {
        Some(true)
    } else {
        None
    }
}
