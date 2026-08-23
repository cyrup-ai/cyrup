//! G82: choosing which source a child's acceptance report is read from — its answer text or a
//! child-authored output file.

use std::path::Path;

use crate::exec::output::looks_like_acceptance_report;

use super::AcceptanceStatus;

// ============================================================================================
// G82: the child-authored output file as an acceptance-report source
// (pi `parseAcceptanceReportSources`, `acceptance.ts:753-771`)
// ============================================================================================

/// G82 — pi's `fileOutput` input to `evaluateAcceptance` (`acceptance.ts:1214-1220`). Upstream's
/// own doc on the field:
///
/// > Content the child sent to its configured output file (from its own write tool calls, not from
/// > disk, so a concurrent writer to the same path cannot be misattributed). Searched for the
/// > acceptance report; searched before the assistant output when `authoritative` (outputMode
/// > "file-only").
///
/// Built by [`crate::exec::output::extract_child_written_output`], never by reading the path.
#[derive(Debug, Clone, Copy)]
pub struct AcceptanceFileOutput<'a> {
    /// The exact bytes the child's own successful `write` call sent to `path`.
    pub content: &'a str,
    /// The configured output path, quoted verbatim in the "(in configured output …)" parse-error
    /// suffix upstream produces (`acceptance.ts:763`).
    pub path: &'a Path,
    /// `outputMode === "file-only"` — the file becomes the PRIMARY report source, searched before
    /// the assistant output.
    pub authoritative: bool,
}

/// G82 — source: `parseAcceptanceReportSources(output, fileOutput)` (`acceptance.ts:753-771`).
///
/// ```text
/// const [primary, secondary] = fileOutput?.authoritative ? [fromFile, fromText] : [fromText, fromFile];
/// const first = primary();
/// // A malformed report in the primary source is a defect to surface, not a
/// // miss to paper over with the secondary source; only a genuinely absent
/// // report falls through.
/// if (first.report || first.error !== ACCEPTANCE_REPORT_NOT_FOUND) return first;
/// return secondary();
/// ```
///
/// Ported as a choice of SOURCE TEXT rather than of parse result, because every rung of
/// [`crate::exec::acceptance::model::evaluate::evaluate_acceptance`] re-reads the report from text ([`self_report_floor`]'s companion-key
/// probe and `declared_structural_failures`'s full parse want the same source). The selection
/// rule is identical: the primary source wins whenever it yields a report OR any parse error other
/// than "not found"; only a genuinely absent report falls through to the secondary.
pub(crate) fn select_acceptance_report_source<'a>(
    output: Option<&'a str>,
    file_output: Option<&AcceptanceFileOutput<'a>>,
) -> Option<&'a str> {
    /// `ACCEPTANCE_REPORT_NOT_FOUND` (`acceptance.ts:699`) — the one error that is a MISS rather
    /// than a defect. Reuses the model port's own constant so the two selectors can never drift.
    use crate::exec::acceptance::model::ACCEPTANCE_REPORT_NOT_FOUND;

    let from_text = output;
    let from_file = file_output.map(|f| f.content);
    let (primary, secondary) = if file_output.is_some_and(|f| f.authoritative) {
        (from_file, from_text)
    } else {
        (from_text, from_file)
    };
    let primary_is_decisive = primary.is_some_and(|text| {
        let parsed = crate::exec::acceptance::model::parse_acceptance_report(text);
        parsed.report.is_some() || parsed.error.as_deref() != Some(ACCEPTANCE_REPORT_NOT_FOUND)
    });
    if primary_is_decisive {
        primary
    } else {
        // `return secondary()` — and when there is no secondary at all, its result is the same
        // "not found" the primary already produced, so the primary text is kept for the (identical)
        // downstream outcome rather than discarding the run's own output.
        secondary.or(primary)
    }
}

/// Step 2's self-report floor: `NotRequired` if no `final_output` or no acceptance-report-shaped
/// block is present at all; `Claimed` if a block is present but carries no recognizable companion
/// evidence field; `Attested` if it carries at least one of
/// [`crate::exec::output::ACCEPTANCE_REPORT_COMPANION_KEYS`] alongside `criteriaSatisfied`.
pub(crate) fn self_report_floor(final_output: Option<&str>) -> AcceptanceStatus {
    let Some(text) = final_output else {
        return AcceptanceStatus::NotRequired;
    };
    if !looks_like_acceptance_report(text) {
        return AcceptanceStatus::NotRequired;
    }
    if extract_acceptance_report(text).is_some_and(|report| report.has_companion_evidence) {
        AcceptanceStatus::Attested
    } else {
        AcceptanceStatus::Claimed
    }
}

/// A minimally-parsed view of one child's self-reported `acceptance-report` block, used only to
/// decide the `Claimed` vs. `Attested` self-report floor (step 2 above) — this is NOT a full
/// schema validator (that is R-SA-030's `exec/output.rs`/structured-output concern for a
/// DIFFERENT, schema-declared structured output, not this prose-embedded report block).
#[derive(Debug, Clone)]
struct ParsedAcceptanceReport {
    has_companion_evidence: bool,
}

/// Extract and minimally interpret the acceptance-report-shaped block from `text` (the same shape
/// [`crate::exec::output::looks_like_acceptance_report`] detects), if any. Reuses that function's
/// own detection logic rather than re-implementing fenced-block scanning a second time — this
/// function's only additional job is deciding whether the block, once located, carries a
/// companion-evidence key.
fn extract_acceptance_report(text: &str) -> Option<ParsedAcceptanceReport> {
    if !looks_like_acceptance_report(text) {
        return None;
    }
    // G79 gave every acceptance-report field a snake_case alias (`acceptance.ts:486-508`), so the
    // companion-evidence probe has to accept both spellings or a child whose report the PARSER
    // accepts in full is still scored as a bare `Claimed` here.
    let has_companion_evidence = crate::exec::output::ACCEPTANCE_REPORT_COMPANION_KEYS
        .iter()
        .any(|key| crate::exec::output::text_mentions_report_key(text, key));
    Some(ParsedAcceptanceReport {
        has_companion_evidence,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::lattice::contract::AcceptanceContract;


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn self_report_floor_distinguishes_claimed_from_attested() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);
        // NotRequired contract is a no-op regardless, so force a Checked requirement that a
        // guard-pass alone satisfies, and inspect the self-report floor via a HIGHER achieved
        // level than what Checked alone would produce is not observable here (Checked already
        // dominates Claimed) — this test instead exercises the private classification path via
        // the achieved level when required_level is NotRequired-but-not-quite: use is_no_op
        // false by attaching required_level Checked with the guard failing so ONLY the self-
        // report floor and the checked evidence combine into `achieved`, and assert the ledger's
        // achieved status directly reflects the self-report floor when it's the higher of the two.
        let _ = contract; // documented above; real assertions happen in the two tests below.

        let bare_claim = self_report_floor(Some(
            "Done.\n```acceptance-report\n{\"criteriaSatisfied\": true}\n```",
        ));
        assert_eq!(bare_claim, AcceptanceStatus::Claimed);

        let attested = self_report_floor(Some(
            "Done.\n```acceptance-report\n{\"criteriaSatisfied\": true, \"changedFiles\": [\"a.rs\"]}\n```",
        ));
        assert_eq!(attested, AcceptanceStatus::Attested);

        let nothing = self_report_floor(Some("Just a plain answer, no report block."));
        assert_eq!(nothing, AcceptanceStatus::NotRequired);

        let none_at_all = self_report_floor(None);
        assert_eq!(none_at_all, AcceptanceStatus::NotRequired);

        let _ = dir; // no subprocess needed in this particular test; tempdir kept for symmetry.
    }


    // --------------------------------------------------------------------------------------
    // G82: `parseAcceptanceReportSources` (`acceptance.ts:753-772`), enum-lattice side
    // --------------------------------------------------------------------------------------

    /// The lattice-side [`select_acceptance_report_source`] has its own copy of upstream's
    /// primary/secondary rule and — unlike `crate::exec::acceptance::model::parse_acceptance_report_sources` — had no unit
    /// test of it. The live `file-only` integration test cannot supply one: in that mode the run's
    /// `final_output` is absent by the time the gate runs, so the secondary source is `None` and
    /// BOTH the `authoritative` ordering and the primary-is-decisive rule collapse to the same
    /// answer no matter which way they are written.
    #[test]
    fn the_authoritative_file_is_searched_first_and_a_primary_defect_is_never_papered_over() {
        let file_report =
            "artifact\n```acceptance-report\n{\"criteriaSatisfied\": [], \"diffSummary\": \"from-file\"}\n```";
        let text_report =
            "receipt\n```acceptance-report\n{\"criteriaSatisfied\": [], \"diffSummary\": \"from-text\"}\n```";
        let path = Path::new("out.md");

        // Not authoritative: the assistant output is primary. It has no report, so the file — the
        // secondary — supplies it.
        assert_eq!(
            select_acceptance_report_source(
                Some("no report here"),
                Some(&AcceptanceFileOutput {
                    content: file_report,
                    path,
                    authoritative: false,
                }),
            ),
            Some(file_report)
        );

        // Not authoritative, and BOTH carry a report: the assistant output wins.
        assert_eq!(
            select_acceptance_report_source(
                Some(text_report),
                Some(&AcceptanceFileOutput {
                    content: file_report,
                    path,
                    authoritative: false,
                }),
            ),
            Some(text_report)
        );

        // `outputMode: "file-only"` makes the file authoritative: it is searched FIRST and wins
        // even though the assistant output also carries a report.
        assert_eq!(
            select_acceptance_report_source(
                Some(text_report),
                Some(&AcceptanceFileOutput {
                    content: file_report,
                    path,
                    authoritative: true,
                }),
            ),
            Some(file_report)
        );

        // A MALFORMED report in the primary source is a defect to surface, not a miss to paper
        // over with the secondary — only a genuinely absent report falls through.
        let malformed = "receipt\n```acceptance-report\n{\"criteriaSatisfied\": [{\"id\": \"c1\"}]}\n```";
        assert_eq!(
            select_acceptance_report_source(
                Some(malformed),
                Some(&AcceptanceFileOutput {
                    content: file_report,
                    path,
                    authoritative: false,
                }),
            ),
            Some(malformed),
            "a defective primary must not be replaced by the secondary"
        );
    }


    /// The full truth table for [`select_acceptance_report_source`], because the case above leaves
    /// three of its inputs unexercised and each hides an independent rule:
    ///
    /// * the defect rule is only ever seen with the swap OFF, so `authoritative` + a DEFECTIVE
    ///   file reads the same as `authoritative` + a valid one;
    /// * the fall-through is only ever seen with the swap OFF, so `authoritative` + a file with no
    ///   report at all never demonstrates that the assistant output is still consulted;
    /// * `file_output: None` is never passed, so the `secondary.or(primary)` tail — the branch that
    ///   keeps the run's own output rather than returning nothing — is never reached.
    ///
    /// Each source string's parse classification is asserted FIRST, so a change to the report
    /// grammar can never silently turn a "defective" fixture into an "absent" one and quietly
    /// reinterpret every row below it.
    #[test]
    fn the_report_source_truth_table_is_exhaustive_over_authoritative_and_parse_outcome() {
        const VALID_TEXT: &str =
            "receipt\n```acceptance-report\n{\"criteriaSatisfied\": [], \"diffSummary\": \"from-text\"}\n```";
        const VALID_FILE: &str =
            "artifact\n```acceptance-report\n{\"criteriaSatisfied\": [], \"diffSummary\": \"from-file\"}\n```";
        const DEFECTIVE: &str =
            "artifact\n```acceptance-report\n{\"criteriaSatisfied\": [{\"id\": \"c1\"}]}\n```";
        const ABSENT_TEXT: &str = "no report in the assistant output";
        const ABSENT_FILE: &str = "no report in the configured output file";

        // --- the three parse classifications the selection rule discriminates on ---
        for valid in [VALID_TEXT, VALID_FILE] {
            assert!(
                crate::exec::acceptance::model::parse_acceptance_report(valid).report.is_some(),
                "fixture must parse to a report: {valid:?}"
            );
        }
        let defective = crate::exec::acceptance::model::parse_acceptance_report(DEFECTIVE);
        assert!(defective.report.is_none(), "fixture must not parse");
        assert_ne!(
            defective.error.as_deref(),
            Some(crate::exec::acceptance::model::ACCEPTANCE_REPORT_NOT_FOUND),
            "a DEFECTIVE fixture must report a defect, never `not found`"
        );
        for absent in [ABSENT_TEXT, ABSENT_FILE] {
            assert_eq!(
                crate::exec::acceptance::model::parse_acceptance_report(absent).error.as_deref(),
                Some(crate::exec::acceptance::model::ACCEPTANCE_REPORT_NOT_FOUND),
                "fixture must be a genuine MISS: {absent:?}"
            );
        }

        let path = Path::new("out.md");
        let file = |content: &'static str, authoritative: bool| AcceptanceFileOutput {
            content,
            path,
            authoritative,
        };

        // --- `authoritative` OFF: the assistant output is primary ---
        // decisive primary wins over a decisive secondary...
        assert_eq!(
            select_acceptance_report_source(Some(VALID_TEXT), Some(&file(VALID_FILE, false))),
            Some(VALID_TEXT)
        );
        // ...including a DEFECTIVE primary, which is a defect to surface, not a miss to paper over.
        assert_eq!(
            select_acceptance_report_source(Some(DEFECTIVE), Some(&file(VALID_FILE, false))),
            Some(DEFECTIVE)
        );
        // only a genuine MISS falls through.
        assert_eq!(
            select_acceptance_report_source(Some(ABSENT_TEXT), Some(&file(VALID_FILE, false))),
            Some(VALID_FILE)
        );

        // --- `authoritative` ON (`outputMode: "file-only"`): the file is primary ---
        assert_eq!(
            select_acceptance_report_source(Some(VALID_TEXT), Some(&file(VALID_FILE, true))),
            Some(VALID_FILE)
        );
        // The defect rule, with the swap ON: a DEFECTIVE authoritative file is still decisive, so
        // the assistant output must NOT rescue it.
        assert_eq!(
            select_acceptance_report_source(Some(VALID_TEXT), Some(&file(DEFECTIVE, true))),
            Some(DEFECTIVE),
            "a defective authoritative file must not be replaced by the assistant output"
        );
        // The fall-through, with the swap ON: an authoritative file with NO report at all is a
        // miss, so the assistant output is consulted after it.
        assert_eq!(
            select_acceptance_report_source(Some(VALID_TEXT), Some(&file(ABSENT_FILE, true))),
            Some(VALID_TEXT),
            "an authoritative file that carries no report must fall through to the output"
        );

        // --- no secondary at all: `secondary.or(primary)` keeps the run's own output ---
        assert_eq!(
            select_acceptance_report_source(Some(ABSENT_TEXT), None),
            Some(ABSENT_TEXT),
            "with no file source, a missing report must not discard the run's own output"
        );
        assert_eq!(
            select_acceptance_report_source(Some(VALID_TEXT), None),
            Some(VALID_TEXT)
        );
        assert_eq!(select_acceptance_report_source(None, None), None);
        // A missing assistant output still lets a non-authoritative file supply the report.
        assert_eq!(
            select_acceptance_report_source(None, Some(&file(VALID_FILE, false))),
            Some(VALID_FILE)
        );
        // Neither source carries a report: the (identically "not found") secondary is returned.
        assert_eq!(
            select_acceptance_report_source(Some(ABSENT_TEXT), Some(&file(ABSENT_FILE, false))),
            Some(ABSENT_FILE)
        );
        assert_eq!(
            select_acceptance_report_source(None, Some(&file(ABSENT_FILE, true))),
            Some(ABSENT_FILE)
        );
    }

}
