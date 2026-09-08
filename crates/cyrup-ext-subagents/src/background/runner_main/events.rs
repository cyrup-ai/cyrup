//! Bounded `events.jsonl` journal for the detached runner: the size-cap env knobs and the
//! open/append primitives every other runner_main submodule logs through. Split out of
//! `background/runner_main.rs`; ports pi `runs/background/subagent-runner.ts`.

use crate::background::RunPaths;
use crate::jsonl::BoundedJsonlWriter;
use super::config::RunnerConfig;


/// pi `ASYNC_EVENTS_MAX_BYTES_ENV = "PI_SUBAGENT_ASYNC_EVENTS_MAX_BYTES"`
/// (`runs/background/subagent-runner.ts:307` @v0.64.0, added at v0.31.0) in this crate's `CYRUP_`
/// naming family: the operator's override for the `events.jsonl` byte cap, which otherwise is
/// [`crate::jsonl::DEFAULT_JSONL_CAP_BYTES`] — the same 50 MiB as upstream's
/// `DEFAULT_MAX_ASYNC_EVENTS_BYTES` (`:306`).
pub const ASYNC_EVENTS_MAX_BYTES_ENV: &str = "CYRUP_SUBAGENT_ASYNC_EVENTS_MAX_BYTES";

/// The upstream spelling of [`ASYNC_EVENTS_MAX_BYTES_ENV`], honoured as a read-side compatibility
/// alias (the convention `exec/spawn_budget.rs` and `exec/capability_ceiling.rs` document).
pub const ASYNC_EVENTS_MAX_BYTES_ENV_PI_ALIAS: &str = "PI_SUBAGENT_ASYNC_EVENTS_MAX_BYTES";

/// pi `maxAsyncEventsBytes()` (`subagent-runner.ts:318-324` @v0.64.0), over an injected lookup:
/// unset or empty → the default; `Number(raw)` not finite or negative → the default; otherwise
/// `Math.floor(parsed)`. So `"1e6"` and `"50.9"` are accepted (1 000 000 and 50), `"0"` caps the
/// log at zero bytes (every line dropped, as upstream), and anything unparsable falls back rather
/// than disabling the cap. The one JS coercion not reproduced: `Number("  ")` is `0` in JS, while
/// a whitespace-only value falls back to the default here — an artefact of `Number`, not a
/// documented contract, and noted so nobody reads the difference as a port error.
#[must_use]
pub fn resolve_async_events_cap_bytes(get: &dyn Fn(&str) -> Option<String>) -> u64 {
    let raw = get(ASYNC_EVENTS_MAX_BYTES_ENV).or_else(|| get(ASYNC_EVENTS_MAX_BYTES_ENV_PI_ALIAS));
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

/// R-SA-136/146: open the size-capped `events.jsonl` writer for this run, via the SAME shared
/// `BoundedJsonlWriter` primitive `spawn::SpawnedChild`'s per-attempt child-output tee uses
/// (`jsonl.rs`'s own module doc names this exact call site as one of its two intended writers).
/// A failure to open it (e.g. an unwritable run directory) degrades to `None` — `append_event`
/// then silently no-ops on every call — rather than failing this run over a best-effort
/// diagnostic log, mirroring every other non-`status.json`/`ResultFile` write in this function.
pub(super) async fn open_run_events(
    config: &RunnerConfig,
    run_paths: &RunPaths,
) -> Option<BoundedJsonlWriter> {
    let cap = resolve_async_events_cap_bytes(&|name| std::env::var(name).ok());
    let mut events = BoundedJsonlWriter::create_with_cap(&run_paths.events, cap)
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

/// R-SA-136/146: append one JSON-shaped line to this run's `events.jsonl` via the shared
/// [`BoundedJsonlWriter`] primitive, if a writer was successfully opened for this run (`events` is
/// `None` only when [`BoundedJsonlWriter::create`] itself failed at startup — see [`run`](super::run)'s own
/// construction site — in which case this is a silent no-op, matching this crate's established
/// "a `.jsonl` artifact's own failure never fails the run" convention, restated here at the
/// writer-availability level rather than only the per-line byte-cap level
/// [`BoundedJsonlWriter::write_line`] already enforces internally).
///
/// `kind` is a short, stable event-type tag (`"run.started"`, `"step.started"`, `"step.completed"`,
/// `"run.paused"`, `"run.completed"`) mirroring the shape [`crate::background::tracker`]'s own tailing-consumer
/// doc comment and test fixtures already assume for this file (one JSON object per line, a `kind`
/// field identifying the event). `detail` is folded into the same JSON object as additional fields
/// when present, so a consumer never has to parse a nested string-encoded sub-document.
pub(super) async fn append_event(
    events: &mut Option<BoundedJsonlWriter>,
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
    // A write failure here (genuine I/O error while still under the byte cap — the cap itself is
    // always a silent no-op, never an `Err`) is likewise never allowed to fail the run: this event
    // log is a best-effort diagnostic/tailing aid (R-SA-093), not part of R-SA-077's authoritative
    // status.json/ResultFile durability contract.
    let _ = writer.write_line(&line).await;
}

#[cfg(test)]
mod async_events_cap_tests {
    use super::{
        ASYNC_EVENTS_MAX_BYTES_ENV, ASYNC_EVENTS_MAX_BYTES_ENV_PI_ALIAS,
        resolve_async_events_cap_bytes,
    };
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
    /// zero-byte cap. Before this port `open_run_events` called `BoundedJsonlWriter::create`,
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

    /// The `PI_` spelling is a fallback, never an override.
    #[test]
    fn the_pi_alias_is_consulted_only_when_the_cyrup_spelling_is_unset() {
        assert_eq!(with(&[(ASYNC_EVENTS_MAX_BYTES_ENV_PI_ALIAS, "2048")]), 2048);
        assert_eq!(
            with(&[
                (ASYNC_EVENTS_MAX_BYTES_ENV, "4096"),
                (ASYNC_EVENTS_MAX_BYTES_ENV_PI_ALIAS, "2048"),
            ]),
            4096
        );
    }
}
