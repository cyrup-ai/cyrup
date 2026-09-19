//! PB-8 — the RPC parameter normalizers: pi `src/extension/rpc.ts:349-353` and `:385-559`
//! @v0.68.0.
//!
//! Every function here is a GATE, and every refusal sentence is upstream's, byte for byte. The
//! normalizers are deliberately narrow: an RPC caller is a sibling extension or a host, not the
//! model, so the surface it may drive is a strict subset of the `subagent` tool's own parameter
//! union — `spawn` cannot carry an `action`, `spawn` cannot run in the foreground, `manage`
//! accepts seven of cyrup's nine `schedule.*` verbs, and `resume` accepts one output mode.
//!
//! # `[CYRUP-DELTA, mechanism]` — `assertSubagentParams` is the tool's own parse
//!
//! Upstream validates the normalized object against the compiled `SubagentParams` TypeBox schema
//! before dispatching (`rpc.ts:355-361`, called at `:479`, `:510`). cyrup's equivalent check is
//! `serde_json::from_value::<SubagentToolParams>` inside
//! [`crate::extension::SubagentTool`]'s `Tool::execute`
//! (`extension/tool/mod.rs:210-211`), which answers with `invalid subagent tool call: …` — i.e.
//! the same rejection, produced by the same schema, one call later. Re-deriving it here would be a
//! second copy of the tool's parse that could drift from it, so the check is left where it already
//! is and its `Err` is mapped to `execution_failed` by the dispatcher.

use serde_json::{Map, Value};

use super::SUBAGENT_RPC_MANAGEMENT_ACTIONS;
use super::envelope::SubagentRpcError;

/// pi `assertRecordParams` (`rpc.ts:349-353`): an ABSENT `params` is `{}`; anything present that
/// is not a record is refused.
///
/// The `undefined`/`null` split is load-bearing and is not a JavaScript quirk to smooth over.
/// `parseRequest` carries `params` through whenever `raw.params !== undefined` (`rpc.ts:784`), so
/// an explicit JSON `null` arrives as a PRESENT value; `assertRecordParams` returns `{}` only for
/// `undefined` (`:350`) and `isRecord(null)` is false (`:351`), so `params: null` THROWS
/// `invalid_params`. cyrup preserves the distinction in the type — `envelope::parse_request` stores
/// `None` for absent and `Some(Value::Null)` for an explicit null — so `Some(Value::Null)` must
/// fall into the refusal arm with every other non-object. Folding it into `{}` would let
/// `{"method":"spawn","params":null}` reach [`crate::extension::SubagentTool`]'s `execute`, taking
/// a single-dispatch slot and a spawn-budget read, where upstream answers before the tool is ever
/// entered.
pub(crate) fn assert_record_params(
    params: Option<&Value>,
    method: &str,
) -> Result<Map<String, Value>, SubagentRpcError> {
    match params {
        None => Ok(Map::new()),
        Some(Value::Object(map)) => Ok(map.clone()),
        Some(_) => Err(SubagentRpcError::invalid_params(format!(
            "RPC {method} params must be an object."
        ))),
    }
}

/// pi `normalizeTargetParamsFromRecord` (`rpc.ts:385-392`) — `{id, runId, dir, index}` and nothing
/// else. The whitelist is the point: an RPC target names a run, it does not smuggle an `agent`, a
/// `task` or an `action` into a control verb.
fn normalize_target_params_from_record(input: &Map<String, Value>) -> Map<String, Value> {
    let mut output = Map::new();
    for key in ["id", "runId", "dir", "index"] {
        if let Some(value) = input.get(key) {
            output.insert(key.to_string(), value.clone());
        }
    }
    output
}

/// pi `normalizeTargetParams` (`rpc.ts:394-396`).
pub(crate) fn normalize_target_params(
    params: Option<&Value>,
    method: &str,
) -> Result<Map<String, Value>, SubagentRpcError> {
    Ok(normalize_target_params_from_record(&assert_record_params(
        params, method,
    )?))
}

/// pi `normalizeStatusParams` (`rpc.ts:398-404`) — the four target keys plus `view`/`lines`.
pub(crate) fn normalize_status_params(
    params: Option<&Value>,
) -> Result<Map<String, Value>, SubagentRpcError> {
    let input = assert_record_params(params, "status")?;
    let mut output = normalize_target_params_from_record(&input);
    for key in ["view", "lines"] {
        if let Some(value) = input.get(key) {
            output.insert(key.to_string(), value.clone());
        }
    }
    Ok(output)
}

/// pi `hasStatusTarget` (`rpc.ts:406-413`).
///
/// `view` and `lines` COUNT as targets, which is easy to miss and load-bearing:
/// `status {view:"fleet"}` must take the EXECUTOR tier (`rpc.ts:738-744`), because the in-memory
/// fast path has no rendering for a view selector at all.
pub(crate) fn has_status_target(params: &Map<String, Value>) -> bool {
    ["id", "runId", "dir", "index", "view", "lines"]
        .iter()
        .any(|key| params.contains_key(*key))
}

/// pi's `!target.id && !target.runId && !target.dir` (`rpc.ts:532`, `:548`).
///
/// A JS FALSINESS test, not a presence test: `{id: ""}` names no run, so it takes the same refusal
/// an absent `id` takes rather than being forwarded as a blank target. Only these three string keys
/// are tested this way — `hasStatusTarget` (`:406-413`) really is a `!== undefined` test, and is
/// spelled with `contains_key` for exactly that reason.
fn names_a_target(params: &Map<String, Value>) -> bool {
    ["id", "runId", "dir"].iter().any(|key| {
        params
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    })
}

/// pi `manageParams` (`rpc.ts:486-512`).
///
/// The action list is upstream's SEVEN (`rpc.ts:65-73`), not cyrup's nine
/// (`background/scheduled_runs/tool.rs:95-108` also knows `schedule.create` and
/// `schedule.run-due`). That narrowing is upstream's own choice — an RPC caller may inspect and
/// steer the schedule, and creates a schedule through `spawn`'s own surface — so the list stays at
/// seven rather than being widened to whatever this side happens to dispatch.
///
/// **Re-checked when the five lane/worktree verbs landed (LANES_2).** `rpc.ts:65-73` @v0.68.0 is
/// still the same seven, and `git show v0.68.0:src/extension/rpc.ts | grep -n
/// 'lane\|worktree\|handoff'` returns nothing — the whole RPC surface has no concept of either.
/// The symmetry argument for widening ("a host that fanned out over the bridge should converge
/// over it") rests on a FALSE premise, which is why it was checked: neither `rpc.ts`'s `spawn`
/// nor [`super::SUBAGENT_RPC_METHODS`] has a `worktree` parameter, so a bridge caller cannot
/// create a manifest here and has nothing to converge. Separately, `worktree.discard` is
/// confirm-gated by default and a bridge caller is not the session holding the UI, so widening
/// would either bypass that gate or route a destructive prompt to an arbitrary host. Unchanged,
/// and NOT a `[CYRUP-DELTA]` — matching upstream is the default.
pub(crate) fn manage_params(
    params: Option<&Value>,
) -> Result<Map<String, Value>, SubagentRpcError> {
    let input = assert_record_params(params, "manage")?;
    // `:488-493`.
    let Some(action) = input
        .get("action")
        .and_then(Value::as_str)
        .filter(|a| SUBAGENT_RPC_MANAGEMENT_ACTIONS.contains(a))
    else {
        return Err(SubagentRpcError::invalid_params(format!(
            "RPC manage action must be one of: {}.",
            SUBAGENT_RPC_MANAGEMENT_ACTIONS.join(", ")
        )));
    };
    // `:494-496` — a present `id` that is not a non-blank string is refused BEFORE the
    // requires-id rule below, so `id: 7` reports the type error rather than "requires id".
    let raw_id = input.get("id");
    if let Some(id) = raw_id
        && !id.as_str().is_some_and(|s| !s.trim().is_empty())
    {
        return Err(SubagentRpcError::invalid_params(
            "RPC manage id must be a non-empty string.",
        ));
    }
    // `:498-501` — six of the seven require one.
    let id = raw_id.and_then(Value::as_str);
    if action != "schedule.list" && id.is_none() {
        return Err(SubagentRpcError::invalid_params(format!(
            "RPC manage {action} requires id."
        )));
    }
    // `:502-504` — `quiet` is a boolean and is only meaningful for `schedule.run`.
    let quiet = input.get("quiet");
    if action == "schedule.run"
        && let Some(quiet) = quiet
        && !quiet.is_boolean()
    {
        return Err(SubagentRpcError::invalid_params(
            "RPC manage quiet must be a boolean.",
        ));
    }
    // `:505-509`.
    let mut output = Map::new();
    output.insert("action".to_string(), Value::from(action));
    if let Some(id) = id {
        output.insert("id".to_string(), Value::from(id.trim()));
    }
    if action == "schedule.run" && quiet == Some(&Value::Bool(true)) {
        output.insert("quiet".to_string(), Value::Bool(true));
    }
    Ok(output)
}

/// pi `spawnParams` (`rpc.ts:514-525`) — RPC spawn is DETACHED-ONLY and refuses an `action`.
///
/// Both guards are RPC-local rules, not the public-execution boundary: upstream calls
/// `normalizePublicSubagentExecution` first (`:516`) and only then adds these two
/// (`:518-523`). cyrup's port of that boundary is
/// [`crate::extension::tool::params::normalize_public_subagent_execution`], whose own doc records
/// which upstream clauses it carries; it is called here for the same reason upstream calls it —
/// so an RPC caller and the model hit the identical blank-action refusal — and the two RPC rules
/// are layered on top, exactly where upstream puts them.
pub(crate) fn spawn_params(params: Option<&Value>) -> Result<Map<String, Value>, SubagentRpcError> {
    let input = assert_record_params(params, "spawn")?;
    // `:516-517` — the public-execution boundary FIRST, so a blank `action` gets its own refusal.
    crate::extension::tool::params::normalize_public_subagent_execution(
        input.get("action").and_then(Value::as_str),
    )
    .map_err(|e| SubagentRpcError::invalid_params(e.message))?;
    // `:518-520` — upstream tests `normalized.params.action !== undefined`, so the key being
    // PRESENT is the refusal, whatever its type. A string-typed check would let `action: 7` past
    // and into the dispatched object, where the tool would answer with a serde rejection instead of
    // the RPC's own sentence.
    if input.contains_key("action") {
        return Err(SubagentRpcError::invalid_params(
            "RPC spawn does not accept management/control actions. Use status or interrupt RPC \
             methods instead.",
        ));
    }
    // `:521-523`. Note this reads the RAW input, not the normalized copy: upstream tests
    // `input.async === false`, so a missing `async` is accepted and forced to `true` below.
    if input.get("async") == Some(&Value::Bool(false)) {
        return Err(SubagentRpcError::invalid_params(
            "RPC spawn only supports detached async launches; omit async or set async: true.",
        ));
    }
    // `:524`.
    let mut output = input;
    output.insert("async".to_string(), Value::Bool(true));
    Ok(output)
}

/// pi `steerParams` (`rpc.ts:527-541`).
///
/// # `[CYRUP-DELTA, unrepresentable]` — `steeringRecovery: false` has no analogue
///
/// Upstream forces `steeringRecovery: false` (`:539`); its schema declares the flag as
/// *"forced false by extension RPC for exact ownership"* (`extension/schemas.ts:314` @v0.68.0).
/// cyrup's steer has no pause/revive-after-missed-ack mode to turn OFF —
/// `grep -rn 'steering_recovery' crates/` is empty — so there is no flag to force and none is
/// invented. The `mode` check above is kept even though
/// [`crate::background::control::SteerDeliveryMode`] validates it one level down, so an RPC caller
/// reads the RPC's own sentence rather than the tool's.
pub(crate) fn steer_params(params: Option<&Value>) -> Result<Map<String, Value>, SubagentRpcError> {
    let input = assert_record_params(params, "steer")?;
    // `:529-530`.
    let Some(message) = input
        .get("message")
        .and_then(Value::as_str)
        .filter(|m| !m.trim().is_empty())
    else {
        return Err(SubagentRpcError::invalid_params(
            "RPC steer requires a non-empty message.",
        ));
    };
    // `:531-532`.
    let mut output = normalize_target_params_from_record(&input);
    if !names_a_target(&output) {
        return Err(SubagentRpcError::invalid_params(
            "RPC steer requires id, runId, or dir.",
        ));
    }
    // `:533`.
    let mode = input.get("mode");
    if let Some(mode) = mode
        && !matches!(mode.as_str(), Some("steer" | "follow_up" | "auto"))
    {
        return Err(SubagentRpcError::invalid_params(
            "RPC steer mode must be steer, follow_up, or auto.",
        ));
    }
    // `:534-540`.
    output.insert("action".to_string(), Value::from("steer"));
    output.insert("message".to_string(), Value::from(message.trim()));
    if let Some(mode) = mode.and_then(Value::as_str) {
        output.insert("mode".to_string(), Value::from(mode));
    }
    Ok(output)
}

/// pi `resumeParams` (`rpc.ts:543-559`) — `outputMode` may ONLY be `"file-only"`.
pub(crate) fn resume_params(
    params: Option<&Value>,
) -> Result<Map<String, Value>, SubagentRpcError> {
    let input = assert_record_params(params, "resume")?;
    // `:545-546`.
    let Some(message) = input
        .get("message")
        .and_then(Value::as_str)
        .filter(|m| !m.trim().is_empty())
    else {
        return Err(SubagentRpcError::invalid_params(
            "RPC resume requires a non-empty message.",
        ));
    };
    // `:547-548`.
    let mut output = normalize_target_params_from_record(&input);
    if !names_a_target(&output) {
        return Err(SubagentRpcError::invalid_params(
            "RPC resume requires id, runId, or dir.",
        ));
    }
    // `:549-550`.
    let raw_output = input.get("output");
    if let Some(value) = raw_output
        && !value.as_str().is_some_and(|s| !s.trim().is_empty())
    {
        return Err(SubagentRpcError::invalid_params(
            "RPC resume output must be a non-empty path.",
        ));
    }
    // `:551-552`.
    if let Some(mode) = input.get("outputMode")
        && mode.as_str() != Some("file-only")
    {
        return Err(SubagentRpcError::invalid_params(
            "RPC resume supports only file-only output mode.",
        ));
    }
    // `:553-558`.
    output.insert("action".to_string(), Value::from("resume"));
    output.insert("message".to_string(), Value::from(message.trim()));
    if let Some(path) = raw_output.and_then(Value::as_str) {
        output.insert("output".to_string(), Value::from(path.trim()));
        output.insert("outputMode".to_string(), Value::from("file-only"));
    }
    Ok(output)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The refusal a gate produced, or a panic naming what it let through instead.
    fn refusal<T: std::fmt::Debug>(result: Result<T, SubagentRpcError>) -> String {
        match result {
            Err(error) => {
                assert_eq!(
                    error.code.as_str(),
                    "invalid_params",
                    "every normalizer refusal is invalid_params"
                );
                error.message
            }
            Ok(value) => panic!("expected a refusal, the gate let this through: {value:?}"),
        }
    }

    /// pi `assertRecordParams` (`rpc.ts:349-353`). The `undefined`/`null` split is the whole test:
    /// `parseRequest` (`:784`) carries an explicit `null` through as a PRESENT value, `isRecord`
    /// rejects it (`:351`), and only `undefined` becomes `{}` (`:350`).
    ///
    /// Accepting `null` as `{}` is not a harmless liberty — `{"method":"spawn","params":null}`
    /// would then be normalized to `{"async": true}` and DISPATCHED into
    /// [`crate::extension::SubagentTool`], taking a single-dispatch slot, where upstream answers
    /// before the tool is entered.
    #[test]
    fn an_explicit_null_params_is_refused_where_an_absent_one_is_an_empty_record() {
        assert_eq!(
            assert_record_params(None, "status").unwrap(),
            Map::new(),
            "an ABSENT params is the empty record"
        );
        assert_eq!(
            refusal(assert_record_params(Some(&Value::Null), "status")),
            "RPC status params must be an object.",
            "an explicit `params: null` is a present non-record and is refused"
        );
        for (value, method) in [
            (json!(7), "spawn"),
            (json!("x"), "steer"),
            (json!([]), "resume"),
            (json!(true), "manage"),
        ] {
            assert_eq!(
                refusal(assert_record_params(Some(&value), method)),
                format!("RPC {method} params must be an object.")
            );
        }
        assert_eq!(
            assert_record_params(Some(&json!({ "id": "r" })), "stop").unwrap()["id"],
            json!("r")
        );
    }

    /// pi `normalizeTargetParamsFromRecord` (`rpc.ts:385-392`) — FOUR keys and nothing else.
    ///
    /// The whitelist is the security property: without it an RPC caller could smuggle `agent`,
    /// `task` or `action` into a control verb and turn `interrupt` into a launch.
    #[test]
    fn a_target_normalizer_passes_four_keys_and_drops_everything_else() {
        let target = normalize_target_params(
            Some(&json!({
                "id": "run-1",
                "runId": "r1",
                "dir": "/tmp/x",
                "index": 2,
                "agent": "smuggled",
                "task": "smuggled",
                "action": "spawn",
                "async": true,
            })),
            "interrupt",
        )
        .unwrap();

        assert_eq!(
            target.keys().cloned().collect::<Vec<_>>(),
            vec![
                "id".to_string(),
                "runId".to_string(),
                "dir".to_string(),
                "index".to_string()
            ],
            "exactly the four target keys survive: {target:?}"
        );
        assert_eq!(
            refusal(normalize_target_params(Some(&json!(7)), "interrupt")),
            "RPC interrupt params must be an object."
        );
    }

    /// pi `normalizeStatusParams` (`:398-404`) and `hasStatusTarget` (`:406-413`).
    ///
    /// `view`/`lines` COUNT as targets, which is what routes `status {view:"fleet"}` to the
    /// EXECUTOR tier — the in-memory fast path can render no view selector at all.
    #[test]
    fn status_params_carry_view_and_lines_and_both_count_as_targets() {
        let params =
            normalize_status_params(Some(&json!({ "view": "fleet", "lines": 20, "agent": "x" })))
                .unwrap();
        assert_eq!(params["view"], json!("fleet"));
        assert_eq!(params["lines"], json!(20));
        assert!(!params.contains_key("agent"), "{params:?}");

        assert!(
            !has_status_target(&Map::new()),
            "an empty params names nothing"
        );
        for key in ["id", "runId", "dir", "index", "view", "lines"] {
            let mut one = Map::new();
            one.insert(key.to_string(), json!("v"));
            assert!(has_status_target(&one), "`{key}` must count as a target");
        }
    }

    /// pi `manageParams` (`rpc.ts:486-512`) — the SEVEN-action allowlist (`:65-73`).
    ///
    /// The narrowing is upstream's own and is load-bearing: `schedule.create` and
    /// `schedule.run-due` ARE dispatchable by this crate's schedule tool
    /// (`background/scheduled_runs/tool.rs`), so a widened list here would hand a bus client two
    /// verbs upstream deliberately withholds from the RPC surface.
    #[test]
    fn manage_accepts_exactly_seven_actions_and_names_them_in_its_refusal() {
        for action in SUBAGENT_RPC_MANAGEMENT_ACTIONS {
            let params =
                manage_params(Some(&json!({ "action": action, "id": "sched-1" }))).unwrap();
            assert_eq!(params["action"], json!(action));
        }
        let expected = format!(
            "RPC manage action must be one of: {}.",
            SUBAGENT_RPC_MANAGEMENT_ACTIONS.join(", ")
        );
        for rejected in ["schedule.create", "schedule.run-due", "status", "", "stop"] {
            assert_eq!(
                refusal(manage_params(Some(
                    &json!({ "action": rejected, "id": "x" })
                ))),
                expected,
                "`{rejected}` is not on the RPC management allowlist"
            );
        }
        assert_eq!(refusal(manage_params(None)), expected, "no action at all");
        assert_eq!(
            refusal(manage_params(Some(&json!({ "action": 7, "id": "x" })))),
            expected,
            "a non-string action is refused by the same gate"
        );
    }

    /// `:494-501` — six of the seven require an `id`, and the TYPE check runs first, so `id: 7`
    /// reports the type error rather than the (also true) "requires id".
    #[test]
    fn manage_requires_an_id_for_six_of_seven_and_checks_its_type_first() {
        assert!(
            !manage_params(Some(&json!({ "action": "schedule.list" })))
                .unwrap()
                .contains_key("id"),
            "schedule.list is the one action that needs no id"
        );
        for action in [
            "schedule.show",
            "schedule.history",
            "schedule.pause",
            "schedule.resume",
            "schedule.run",
            "schedule.delete",
        ] {
            assert_eq!(
                refusal(manage_params(Some(&json!({ "action": action })))),
                format!("RPC manage {action} requires id.")
            );
        }
        for bad in [json!(7), json!(""), json!("   "), json!(null), json!({})] {
            assert_eq!(
                refusal(manage_params(Some(
                    &json!({ "action": "schedule.show", "id": bad })
                ))),
                "RPC manage id must be a non-empty string.",
                "the type/blank check precedes the requires-id rule for {bad}"
            );
        }
        assert_eq!(
            manage_params(Some(&json!({ "action": "schedule.show", "id": "  s1  " }))).unwrap()["id"],
            json!("s1"),
            "`:507` trims"
        );
    }

    /// `:502-509` — `quiet` is a boolean, is only checked for `schedule.run`, and only `true`
    /// survives onto the dispatched object.
    #[test]
    fn manage_quiet_is_a_boolean_and_only_true_is_forwarded() {
        assert_eq!(
            refusal(manage_params(Some(
                &json!({ "action": "schedule.run", "id": "s", "quiet": "yes" })
            ))),
            "RPC manage quiet must be a boolean."
        );
        assert_eq!(
            manage_params(Some(
                &json!({ "action": "schedule.run", "id": "s", "quiet": true })
            ))
            .unwrap()["quiet"],
            json!(true)
        );
        assert!(
            !manage_params(Some(
                &json!({ "action": "schedule.run", "id": "s", "quiet": false })
            ))
            .unwrap()
            .contains_key("quiet"),
            "`quiet: false` is the default and is not forwarded"
        );
        assert!(
            manage_params(Some(
                &json!({ "action": "schedule.show", "id": "s", "quiet": "nonsense" })
            ))
            .is_ok(),
            "`quiet` is only meaningful — and only checked — for schedule.run"
        );
    }

    /// pi `steerParams` (`rpc.ts:527-541`): a non-empty message, a named target, and a mode from
    /// the three-value allowlist.
    ///
    /// The target test is pi's JS FALSINESS test (`:532`), not a presence test — `{id: ""}` names
    /// no run, so it must take the same refusal an absent `id` takes rather than being forwarded
    /// as a blank target the executor would then have to interpret.
    #[test]
    fn steer_requires_a_message_a_target_and_a_known_mode() {
        for bad in [json!(null), json!(""), json!("   "), json!(7)] {
            assert_eq!(
                refusal(steer_params(Some(&json!({ "id": "r", "message": bad })))),
                "RPC steer requires a non-empty message."
            );
        }
        assert_eq!(
            refusal(steer_params(Some(&json!({ "message": "go" })))),
            "RPC steer requires id, runId, or dir."
        );
        for blank in [json!(""), json!("  ")] {
            assert_eq!(
                refusal(steer_params(Some(&json!({ "message": "go", "id": blank })))),
                "RPC steer requires id, runId, or dir.",
                "a blank id names no run"
            );
        }
        assert_eq!(
            refusal(steer_params(Some(
                &json!({ "message": "go", "id": "r", "mode": "shout" })
            ))),
            "RPC steer mode must be steer, follow_up, or auto."
        );

        for mode in ["steer", "follow_up", "auto"] {
            let params = steer_params(Some(
                &json!({ "message": "  go  ", "id": "r", "mode": mode }),
            ))
            .unwrap();
            assert_eq!(params["action"], json!("steer"));
            assert_eq!(params["mode"], json!(mode));
            assert_eq!(params["message"], json!("go"), "`:537` trims");
            assert_eq!(params["id"], json!("r"));
        }
        // `runId` and `dir` each satisfy the target rule on their own.
        assert!(steer_params(Some(&json!({ "message": "go", "runId": "r1" }))).is_ok());
        assert!(steer_params(Some(&json!({ "message": "go", "dir": "/tmp/r" }))).is_ok());
        // …and the whitelist still applies: a smuggled `agent` does not survive.
        assert!(
            !steer_params(Some(&json!({ "message": "go", "id": "r", "agent": "x" })))
                .unwrap()
                .contains_key("agent")
        );
    }

    /// pi `resumeParams` (`rpc.ts:543-559`) — `outputMode` may ONLY be `"file-only"`.
    ///
    /// That restriction is why an RPC resume cannot be used to stream a run's output back through
    /// a channel that has no reader: the result goes to a FILE, and the caller is told where.
    #[test]
    fn resume_takes_a_message_a_target_and_only_file_only_output() {
        for bad in [json!(null), json!(""), json!("  "), json!(3)] {
            assert_eq!(
                refusal(resume_params(Some(&json!({ "id": "r", "message": bad })))),
                "RPC resume requires a non-empty message."
            );
        }
        assert_eq!(
            refusal(resume_params(Some(&json!({ "message": "again" })))),
            "RPC resume requires id, runId, or dir."
        );
        for bad in [json!(""), json!("  "), json!(7), json!(null)] {
            assert_eq!(
                refusal(resume_params(Some(
                    &json!({ "message": "again", "id": "r", "output": bad })
                ))),
                "RPC resume output must be a non-empty path."
            );
        }
        for bad in [json!("inline"), json!("both"), json!(true), json!(null)] {
            assert_eq!(
                refusal(resume_params(Some(
                    &json!({ "message": "again", "id": "r", "outputMode": bad })
                ))),
                "RPC resume supports only file-only output mode.",
                "only file-only is accepted, saw {bad}"
            );
        }

        let params = resume_params(Some(&json!({
            "message": "  again  ",
            "id": "r",
            "output": "  /tmp/out.md  ",
        })))
        .unwrap();
        assert_eq!(params["action"], json!("resume"));
        assert_eq!(params["message"], json!("again"));
        assert_eq!(params["output"], json!("/tmp/out.md"), "`:556` trims");
        assert_eq!(
            params["outputMode"],
            json!("file-only"),
            "`:556` forces the mode alongside the path"
        );

        // With no `output` at all, neither key is invented.
        let bare = resume_params(Some(&json!({ "message": "again", "id": "r" }))).unwrap();
        assert!(!bare.contains_key("output"));
        assert!(!bare.contains_key("outputMode"));
    }

    /// pi `spawnParams` (`rpc.ts:514-525`) — the two RPC-local rules, and the boundary call that
    /// precedes them (`:516`).
    ///
    /// `action` is refused on PRESENCE, whatever its type (`:518` tests
    /// `normalized.params.action !== undefined`): a string-typed check would let `action: 7` past
    /// and into the dispatched object, where the tool would answer with a serde rejection instead
    /// of the RPC's own sentence.
    #[test]
    fn spawn_is_detached_only_and_refuses_any_action_key() {
        for action in [json!("steer"), json!("status"), json!(7), json!(null)] {
            assert_eq!(
                refusal(spawn_params(Some(
                    &json!({ "agent": "x", "task": "t", "action": action })
                ))),
                "RPC spawn does not accept management/control actions. Use status or interrupt \
                 RPC methods instead.",
                "`action` is refused on presence, saw {action}"
            );
        }
        assert_eq!(
            refusal(spawn_params(Some(
                &json!({ "agent": "x", "task": "t", "async": false })
            ))),
            "RPC spawn only supports detached async launches; omit async or set async: true."
        );

        let omitted = spawn_params(Some(&json!({ "agent": "x", "task": "t" }))).unwrap();
        assert_eq!(
            omitted["async"],
            json!(true),
            "`:524` forces the detached launch when `async` was omitted"
        );
        assert_eq!(omitted["agent"], json!("x"), "the launch keys ride through");
        let explicit =
            spawn_params(Some(&json!({ "agent": "x", "task": "t", "async": true }))).unwrap();
        assert_eq!(explicit["async"], json!(true));
    }
}
