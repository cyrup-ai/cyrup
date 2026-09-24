//! Bounded `events.jsonl` journal for the detached runner: the size-cap env knobs and the
//! open/append primitives every other runner_main submodule logs through. Split out of
//! `background/runner_main.rs`; ports pi `runs/background/subagent-runner.ts`.

use super::config::RunnerConfig;
use crate::background::RunPaths;
use crate::jsonl::RunEventLog;

/// pi `ASYNC_EVENTS_MAX_BYTES_ENV = "PI_SUBAGENT_ASYNC_EVENTS_MAX_BYTES"`
/// (`runs/background/subagent-runner.ts:307` @v0.64.0, added at v0.31.0) in this crate's `CYRUP_`
/// naming family: the operator's override for the `events.jsonl` byte cap, which otherwise is
/// [`crate::jsonl::DEFAULT_JSONL_CAP_BYTES`] — the same 50 MiB as upstream's
/// `DEFAULT_MAX_ASYNC_EVENTS_BYTES` (`:306`).
pub const ASYNC_EVENTS_MAX_BYTES_ENV: &str = "CYRUP_SUBAGENT_ASYNC_EVENTS_MAX_BYTES";

/// pi `maxAsyncEventsBytes()` (`subagent-runner.ts:318-324` @v0.64.0), over an injected lookup:
/// unset or empty → the default; `Number(raw)` not finite or negative → the default; otherwise
/// `Math.floor(parsed)`. So `"1e6"` and `"50.9"` are accepted (1 000 000 and 50), `"0"` gives the
/// child DIAGNOSTIC lines a zero budget (every one dropped behind a single truncation marker, as
/// upstream) while the lifecycle trail is still written in full, and anything unparsable falls
/// back rather than disabling the cap. The one JS coercion not reproduced: `Number("  ")` is `0` in JS, while
/// a whitespace-only value falls back to the default here — an artefact of `Number`, not a
/// documented contract, and noted so nobody reads the difference as a port error.
#[must_use]
pub fn resolve_async_events_cap_bytes(get: &dyn Fn(&str) -> Option<String>) -> u64 {
    let raw = get(ASYNC_EVENTS_MAX_BYTES_ENV);
    let Some(raw) = raw.filter(|value| !value.is_empty()) else {
        return crate::jsonl::DEFAULT_JSONL_CAP_BYTES;
    };
    match raw.trim().parse::<f64>() {
        Ok(parsed) if parsed.is_finite() && parsed >= 0.0 => {
            // `parsed` is finite and non-negative; `as` saturates at `u64::MAX` for larger values,
            // which is the only sensible reading of a cap that big.
            parsed.floor() as u64
        }
        _ => crate::jsonl::DEFAULT_JSONL_CAP_BYTES,
    }
}

/// R-SA-136/146: open this run's `events.jsonl` [`RunEventLog`] with the operator's diagnostic cap
/// `cap` ([`resolve_async_events_cap_bytes`]). The cap bounds only the child diagnostic lines the
/// telemetry pump journals ([`journal_child_line`]); every lifecycle line this handle writes is
/// uncapped, as upstream's `appendJsonl`. A failure to open it (e.g. an unwritable run directory) degrades to `None` — `append_event`
/// then silently no-ops on every call — rather than failing this run over a best-effort
/// diagnostic log, mirroring every other non-`status.json`/`ResultFile` write in this function.
pub(super) async fn open_run_events(
    config: &RunnerConfig,
    run_paths: &RunPaths,
    cap: u64,
) -> Option<RunEventLog> {
    let mut events = RunEventLog::create_with_cap(&run_paths.events, cap)
        .await
        .ok();
    append_event(
        &mut events,
        "subagent.run.started",
        Some(serde_json::json!({ "runId": config.run_id.as_str() })),
    )
    .await;
    events
}

/// R-SA-136/146: append one LIFECYCLE line (pi `appendJsonl`, never capped) to this run's
/// `events.jsonl`, if a writer was successfully opened for this run (`events` is `None` only when
/// [`RunEventLog::create`] itself failed at startup — see [`run`](super::run)'s own construction
/// site — in which case this is a silent no-op, matching this crate's established "a `.jsonl`
/// artifact's own failure never fails the run" convention).
///
/// `kind` is a short, stable event-type tag (`"run.started"`, `"step.started"`, `"step.completed"`,
/// `"run.paused"`, `"run.completed"`) mirroring the shape [`crate::background::tracker`]'s own tailing-consumer
/// doc comment and test fixtures already assume for this file (one JSON object per line, a `kind`
/// field identifying the event). `detail` is folded into the same JSON object as additional fields
/// when present, so a consumer never has to parse a nested string-encoded sub-document.
pub(super) async fn append_event(
    events: &mut Option<RunEventLog>,
    event_type: &str,
    detail: Option<serde_json::Value>,
) {
    let Some(writer) = events.as_mut() else {
        return;
    };
    let mut object = serde_json::Map::new();
    // Field name `type` (NOT `kind`) + `subagent.*` event-type strings, matching pi's
    // `events.jsonl` shape exactly (`subagent-runner.ts` `appendJsonl(eventsPath, { type: … })`).
    object.insert(
        "type".to_string(),
        serde_json::Value::String(event_type.to_string()),
    );
    object.insert(
        "ts".to_string(),
        serde_json::Value::from(crate::time::now_epoch_millis()),
    );
    if let Some(serde_json::Value::Object(fields)) = detail {
        for (key, value) in fields {
            object.insert(key, value);
        }
    }
    let line = serde_json::Value::Object(object).to_string();
    // A write failure here (a genuine I/O error — lifecycle lines are never capped) is never
    // allowed to fail the run: this event log is a best-effort diagnostic/tailing aid (R-SA-093), not part of R-SA-077's authoritative
    // status.json/ResultFile durability contract.
    let _ = writer.write_line(&line).await;
}

/// pi `shouldPersistChildEvent` (`subagent-runner.ts:334-336` @v0.43.0): every child event is
/// journaled except the streaming `message_update` deltas.
const UNPERSISTED_CHILD_EVENT_TYPE: &str = "message_update";

/// Where a journaled child line came from — pi `childEventContext` (`subagent-runner.ts:590-600`
/// @v0.43.0): the run, the FLAT step index, and the step's agent.
pub(crate) struct ChildEventContext<'a> {
    pub(crate) run_id: &'a str,
    pub(crate) step_index: usize,
    pub(crate) agent: &'a str,
}

/// pi `appendChildEvent` / `appendChildLine` (`subagent-runner.ts:590-622` @v0.43.0), the pure
/// half: the `events.jsonl` DIAGNOSTIC line one raw child stdout line becomes, and the event type
/// the truncation marker names if it is the one that overflows.
///
/// - A JSON object is journaled as itself plus `subagentSource: "child"`, `subagentRunId`,
///   `subagentStepIndex`, `subagentAgent` and `observedAt` — unless it is a `message_update`,
///   which is never journaled.
/// - Anything else that is not blank is journaled as `{type: "subagent.child.stdout", line}` with
///   the same five fields (pi `processStdoutLine`'s parse-failure arm, `:611-617`).
///
/// `None` for a blank line or a `message_update`.
pub(crate) fn child_event_journal_line(
    raw: &str,
    context: &ChildEventContext<'_>,
    observed_at: i64,
) -> Option<(String, Option<String>)> {
    if raw.trim().is_empty() {
        return None;
    }
    let mut object = match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::Object(object)) => object,
        _ => {
            let mut object = serde_json::Map::new();
            object.insert("type".into(), "subagent.child.stdout".into());
            object.insert("line".into(), raw.into());
            object
        }
    };
    let event_type = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    if event_type.as_deref() == Some(UNPERSISTED_CHILD_EVENT_TYPE) {
        return None;
    }
    object.insert("subagentSource".into(), "child".into());
    object.insert("subagentRunId".into(), context.run_id.into());
    object.insert("subagentStepIndex".into(), context.step_index.into());
    object.insert("subagentAgent".into(), context.agent.into());
    object.insert("observedAt".into(), observed_at.into());
    Some((serde_json::Value::Object(object).to_string(), event_type))
}

/// Journal one raw child stdout line as a capped DIAGNOSTIC line (pi `appendDiagnosticJsonl`).
/// Best-effort: an I/O failure never fails the run.
pub(super) async fn journal_child_line(
    events: &mut Option<RunEventLog>,
    raw: &str,
    context: &ChildEventContext<'_>,
) {
    let Some(writer) = events.as_mut() else {
        return;
    };
    let Some((line, event_type)) =
        child_event_journal_line(raw, context, crate::time::now_epoch_millis())
    else {
        return;
    };
    let _ = writer
        .write_diagnostic_line(&line, event_type.as_deref())
        .await;
}

#[cfg(test)]
mod child_event_journal_tests {
    #![allow(clippy::expect_used, clippy::indexing_slicing)]

    use super::{ChildEventContext, append_event, child_event_journal_line, journal_child_line};
    use crate::jsonl::{RunEventLog, TRUNCATED_EVENT_TYPE};

    /// The runner-level shape of the bug: with `CYRUP_SUBAGENT_ASYNC_EVENTS_MAX_BYTES=0` every
    /// lifecycle line the runner appends still lands, and the child journal writes ONE marker
    /// naming the event it dropped. Kills: `append_event` writing through the diagnostic path (the
    /// pre-fix behaviour — the lifecycle trail vanishes), and `journal_child_line` writing
    /// through the uncapped lifecycle path (the child event lands despite a zero budget).
    #[tokio::test]
    async fn a_zero_cap_keeps_the_lifecycle_trail_and_marks_the_dropped_child_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let mut lifecycle = RunEventLog::create_with_cap(&path, 0).await.ok();
        let mut journal = RunEventLog::create_with_cap(&path, 0).await.ok();
        append_event(&mut lifecycle, "subagent.run.started", None).await;
        journal_child_line(&mut journal, r#"{"type":"tool_execution_start"}"#, &CONTEXT).await;
        journal_child_line(&mut journal, r#"{"type":"message_end"}"#, &CONTEXT).await;
        append_event(&mut lifecycle, "subagent.run.completed", None).await;
        let text = std::fs::read_to_string(&path).expect("events.jsonl");
        let kinds: Vec<String> = text
            .lines()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).expect("json");
                v["type"].as_str().unwrap_or_default().to_string()
            })
            .collect();
        assert_eq!(
            kinds,
            ["subagent.run.started", "subagent.run.completed"],
            "a zero cap leaves no room even for the marker, and drops no lifecycle line: {text}"
        );
    }

    /// With a budget too small for the child event but big enough for the marker, the marker
    /// lands once, names the dropped event, and later child events are silent.
    #[tokio::test]
    async fn an_overflowing_child_event_is_replaced_by_one_marker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        // 600 - 512 = 88 bytes of diagnostics: less than one journaled child event.
        let mut journal = RunEventLog::create_with_cap(&path, 600).await.ok();
        journal_child_line(&mut journal, r#"{"type":"tool_execution_start"}"#, &CONTEXT).await;
        journal_child_line(&mut journal, r#"{"type":"message_end"}"#, &CONTEXT).await;
        let text = std::fs::read_to_string(&path).expect("events.jsonl");
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect();
        assert_eq!(lines.len(), 1, "{text}");
        assert_eq!(lines[0]["type"], TRUNCATED_EVENT_TYPE);
        assert_eq!(lines[0]["droppedEventType"], "tool_execution_start");
        assert_eq!(lines[0]["maxBytes"], 600);
    }

    const CONTEXT: ChildEventContext<'static> = ChildEventContext {
        run_id: "run-1",
        step_index: 2,
        agent: "scout",
    };

    /// A child event is journaled as itself plus pi's five provenance fields, and its own `type`
    /// is what the truncation marker would name. Kills: journaling the raw line without the
    /// provenance fields, or naming a fixed `droppedEventType`.
    #[test]
    fn a_child_event_carries_upstreams_provenance_fields() {
        let (line, kind) = child_event_journal_line(
            r#"{"type":"tool_execution_start","toolName":"read"}"#,
            &CONTEXT,
            7,
        )
        .expect("journaled");
        let v: serde_json::Value = serde_json::from_str(&line).expect("json");
        assert_eq!(kind.as_deref(), Some("tool_execution_start"));
        assert_eq!(v["toolName"], "read");
        assert_eq!(v["subagentSource"], "child");
        assert_eq!(v["subagentRunId"], "run-1");
        assert_eq!(v["subagentStepIndex"], 2);
        assert_eq!(v["subagentAgent"], "scout");
        assert_eq!(v["observedAt"], 7);
    }

    /// `message_update` deltas and blank lines are never journaled; a non-JSON line is journaled
    /// as `subagent.child.stdout`. Kills: dropping pi's `shouldPersistChildEvent` filter (the log
    /// fills with streaming deltas), and dropping non-JSON output instead of wrapping it.
    #[test]
    fn deltas_are_skipped_and_plain_output_is_wrapped() {
        assert!(child_event_journal_line(r#"{"type":"message_update"}"#, &CONTEXT, 1).is_none());
        assert!(child_event_journal_line("   ", &CONTEXT, 1).is_none());
        let (line, kind) = child_event_journal_line("plain text", &CONTEXT, 1).expect("journaled");
        let v: serde_json::Value = serde_json::from_str(&line).expect("json");
        assert_eq!(kind.as_deref(), Some("subagent.child.stdout"));
        assert_eq!(v["line"], "plain text");
        assert_eq!(v["subagentAgent"], "scout");
    }
}

#[cfg(test)]
mod async_events_cap_tests {
    use super::{ASYNC_EVENTS_MAX_BYTES_ENV, resolve_async_events_cap_bytes};
    use crate::jsonl::DEFAULT_JSONL_CAP_BYTES;

    fn with(pairs: &'static [(&'static str, &'static str)]) -> u64 {
        resolve_async_events_cap_bytes(&move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        })
    }

    /// CFG-067 / pi `maxAsyncEventsBytes()` (`subagent-runner.ts:318-324` @v0.64.0): unset,
    /// empty, unparsable and negative all fall back to the default; a finite non-negative number
    /// is floored, so JS `Number` forms like `1e6` and `50.9` are honoured and `0` is a real
    /// zero-byte cap. Before this port `open_run_events` called `RunEventLog::create`,
    /// which takes no cap at all, so no value of this variable could reach the writer.
    #[test]
    fn the_cap_override_follows_pis_number_coercion() {
        assert_eq!(with(&[]), DEFAULT_JSONL_CAP_BYTES);
        assert_eq!(
            with(&[(ASYNC_EVENTS_MAX_BYTES_ENV, "")]),
            DEFAULT_JSONL_CAP_BYTES
        );
        assert_eq!(
            with(&[(ASYNC_EVENTS_MAX_BYTES_ENV, "lots")]),
            DEFAULT_JSONL_CAP_BYTES
        );
        assert_eq!(
            with(&[(ASYNC_EVENTS_MAX_BYTES_ENV, "-1")]),
            DEFAULT_JSONL_CAP_BYTES
        );
        assert_eq!(with(&[(ASYNC_EVENTS_MAX_BYTES_ENV, "1024")]), 1024);
        assert_eq!(with(&[(ASYNC_EVENTS_MAX_BYTES_ENV, "1e6")]), 1_000_000);
        assert_eq!(with(&[(ASYNC_EVENTS_MAX_BYTES_ENV, "50.9")]), 50);
        assert_eq!(with(&[(ASYNC_EVENTS_MAX_BYTES_ENV, "0")]), 0);
    }

    /// The dropped upstream `PI_` spelling is inert (hard rename): alone it yields the default,
    /// and beside the real key it never wins.
    #[test]
    fn the_dropped_pi_spelling_is_ignored() {
        assert_eq!(
            with(&[("PI_SUBAGENT_ASYNC_EVENTS_MAX_BYTES", "2048")]),
            DEFAULT_JSONL_CAP_BYTES
        );
        assert_eq!(
            with(&[
                (ASYNC_EVENTS_MAX_BYTES_ENV, "4096"),
                ("PI_SUBAGENT_ASYNC_EVENTS_MAX_BYTES", "2048"),
            ]),
            4096
        );
    }
}
