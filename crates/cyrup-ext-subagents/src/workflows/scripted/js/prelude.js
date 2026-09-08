// The workflowScript prelude -- the KEPT half of pi's `WORKER_SOURCE` (`scripted-workflow.ts:35-996`)
// retargeted onto deno_core ops (SCOPE_3f 3.4).
//
// What is DELETED versus upstream, and why (SCOPE_3f 3.5): `promiseHooks`, the `Proxy` over `Promise`, the
// `nativePromiseTrackers`/`nativePromiseParents` WeakMap graph and the `Promise.prototype.then`
// patch -- 963 lines that existed only to infer "did the script await this launch?" from V8 promise
// topology. Here an op call IS the promise, and one `observe` advisory at the head of the chain
// tells the host what upstream had to reconstruct.
//
// What is KEPT is kept HERE, in JavaScript, and not moved into Rust: every rejection below fires
// synchronously, before a call crosses into the host, so the model gets a situated error at the
// right line. The message bytes are upstream's.

"use strict";

((globalThis) => {
  const core = Deno.core;
  const ops = core.ops;

  // --------------------------------------------------------------------------------------------
  // Observation (SCOPE_3f 3.5) -- one advisory at the head of the chain, keyed per launch.
  // --------------------------------------------------------------------------------------------

  let nextCallId = 0n;

  // A THENABLE facade: `await`, `Promise.all` and `.then` chains all assimilate it, and the first
  // consumption marks the underlying calls observed. Upstream marks observation when `then` is
  // first attached -- synchronously, before any result exists -- so this must too.
  function trackObservation(observations, promise) {
    let observed = false;
    const observe = () => {
      if (observed) return;
      observed = true;
      for (const o of observations) {
        try {
          ops.op_workflow_observe(o.operation, o.key ?? "", o.callId ?? 0n);
        } catch {
          // Observation is advisory: it must never be able to fail the script.
        }
      }
    };
    return {
      then(onFulfilled, onRejected) {
        observe();
        return promise.then(onFulfilled, onRejected);
      },
      catch(onRejected) {
        observe();
        return promise.catch(onRejected);
      },
      finally(onFinally) {
        observe();
        return promise.finally(onFinally);
      },
    };
  }

  // A launch delivers a tagged outcome so the rejection can carry upstream's `workflowErrorKind`
  // (`:903-906`), which a bare thrown string cannot.
  function unwrapDelivery(delivery) {
    if (delivery.status === "resolved") return delivery.result;
    const error = new Error(delivery.message);
    if (delivery.detached) error.workflowErrorKind = "detached-child";
    throw error;
  }

  // --------------------------------------------------------------------------------------------
  // Pure helpers kept from WORKER_SOURCE (`:78-92`, `:296-341`)
  // --------------------------------------------------------------------------------------------

  function stableRunJson(value) {
    if (Array.isArray(value)) return "[" + value.map(stableRunJson).join(",") + "]";
    if (value && typeof value === "object") {
      return "{" + Object.keys(value).sort().map((k) => JSON.stringify(k) + ":" + stableRunJson(value[k])).join(",") + "}";
    }
    return JSON.stringify(value) ?? "undefined";
  }

  function canonicalRunParams(params) {
    if (params.gate === undefined || params.acceptance !== false) return params;
    const { acceptance: _acceptance, ...withoutAcceptance } = params;
    return withoutAcceptance;
  }

  // The ONE guest copy of the key grammar (SCOPE_3 SCOPE_3f A.3 / SCOPE_3f 4.5). The host copy is
  // `WorkflowKey::parse`; this one lives in a `.js` asset so the Rust-literal gate stays clean.
  const runKeyPattern = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;

  function assertJsonValue(value, path = "emit", seen = new Set()) {
    if (value === null || typeof value === "string" || typeof value === "boolean") return;
    if (typeof value === "number") {
      if (!Number.isFinite(value)) throw new Error(path + " must contain only finite JSON numbers.");
      return;
    }
    if (typeof value !== "object") throw new Error(path + " must contain only JSON values.");
    if (seen.has(value)) throw new Error(path + " must not contain circular references.");
    seen.add(value);
    if (Array.isArray(value)) {
      for (let i = 0; i < value.length; i++) {
        if (!Object.prototype.hasOwnProperty.call(value, i)) throw new Error(path + " must not contain sparse arrays.");
        assertJsonValue(value[i], path + "[" + i + "]", seen);
      }
    } else {
      const proto = Object.getPrototypeOf(value);
      if (proto !== Object.prototype && proto !== null) throw new Error(path + " must contain only plain JSON objects.");
      if (Object.getOwnPropertySymbols(value).length > 0) throw new Error(path + " must not contain symbol keys.");
      for (const key of Object.keys(value)) assertJsonValue(value[key], path + "." + key, seen);
    }
    seen.delete(value);
  }

  function omitUndefinedWorkflowValues(value) {
    if (Array.isArray(value)) return value.map(omitUndefinedWorkflowValues);
    if (value && typeof value === "object") {
      const out = {};
      for (const [k, v] of Object.entries(value)) {
        if (v !== undefined) out[k] = omitUndefinedWorkflowValues(v);
      }
      return out;
    }
    return value;
  }

  // --------------------------------------------------------------------------------------------
  // runs.all's ordered-array guard (`:230-269` minus the promise tracking)
  // --------------------------------------------------------------------------------------------

  function runsAllKeyAccessError(prop) {
    return new Error(
      "Cannot read runs.all result property '" + prop + "'. runs.all resolves to an ordered array, not a key map. Use results[0], array destructuring, or results.map((result) => result.output), not results." + prop + ".",
    );
  }

  function isArrayIndexProperty(prop) {
    if (!/^(0|[1-9]\d*)$/.test(prop)) return false;
    const index = Number(prop);
    return Number.isSafeInteger(index) && index >= 0 && index < 4294967295;
  }

  const runsAllResultTargets = new WeakSet();

  // The model's instinct is a key map; upstream defends the array shape with a Proxy that throws a
  // re-prompt naming the fix. Kept verbatim in behaviour.
  function wrapRunsAllResults(results, keys) {
    const keySet = new Set(keys);
    const proxy = new Proxy(results, {
      get(target, prop, receiver) {
        if (typeof prop === "string" && keySet.has(prop) && !isArrayIndexProperty(prop)) {
          throw runsAllKeyAccessError(prop);
        }
        return Reflect.get(target, prop, receiver);
      },
    });
    runsAllResultTargets.add(results);
    return proxy;
  }

  function unwrapRunsAllResults(value) {
    return value;
  }

  // --------------------------------------------------------------------------------------------
  // Validation -- every message is a re-prompt and fires BEFORE the call crosses into the host
  // --------------------------------------------------------------------------------------------

  let runFingerprints = new Map();

  function validateLaneKey(key, owner) {
    if (typeof key !== "string" || !runKeyPattern.test(key)) {
      throw new Error(owner + " key must be 1-128 characters using letters, numbers, '.', '_' or '-', and start with a letter or number.");
    }
    return key;
  }

  function validateRunCall(key, params, owner, fingerprints) {
    // Upstream has TWO key validators with DIFFERENT wording, and the run path uses this one
    // (`scripted-workflow.ts:639`): `label + " has an invalid key."`. `validateKey` (`:348`), which
    // produces the "1-128 characters" text, is the lanes/state path. Do not unify them.
    if (typeof key !== "string" || !runKeyPattern.test(key)) {
      throw new Error(owner + " has an invalid key.");
    }
    if (!params || typeof params !== "object" || Array.isArray(params)) {
      throw new Error(owner + "('" + key + "', params) requires a params object.");
    }
    if (params.workflowScript !== undefined) {
      throw new Error("runs.run('" + key + "') cannot start a nested workflow script.");
    }
    if (params.action !== undefined) {
      throw new Error("runs.run('" + key + "') accepts execution params only; management action is not allowed.");
    }
    for (const field of ["tasks", "chain", "parallel", "concurrency", "chainDir"]) {
      if (params[field] !== undefined) {
        throw new Error("runs.run('" + key + "') accepts one child via { agent, task }; use runs.all(...) and JavaScript control flow for orchestration.");
      }
    }
    assertJsonValue(params, owner + " params");
    const fingerprint = stableRunJson(canonicalRunParams(params));
    const existing = fingerprints.get(key);
    if (existing !== undefined && existing !== fingerprint) {
      throw new Error("Duplicate workflow key '" + key + "' used with incompatible launch params.");
    }
    fingerprints.set(key, fingerprint);
    return fingerprint;
  }

  function validateStateKey(key) {
    if (typeof key !== "string" || !runKeyPattern.test(key)) {
      throw new Error("state key must be 1-128 characters using letters, numbers, '.', '_' or '-', and start with a letter or number.");
    }
    return key;
  }

  // --------------------------------------------------------------------------------------------
  // Transport -- an op call IS the operation (SCOPE_3f 3.2)
  // --------------------------------------------------------------------------------------------

  function launch(key, params, collectFailure, batch, generatedLaneKey) {
    const envelope = { key, params };
    if (collectFailure) envelope.collectFailure = true;
    if (batch) envelope.batch = batch;
    if (generatedLaneKey) envelope.generatedLaneKey = generatedLaneKey;
    return ops.op_runs_launch(envelope).then(unwrapDelivery);
  }

  function launchRunsAll(items, generatedLaneKeys) {
    if (!Array.isArray(items)) throw new Error("runs.all(items) requires an array.");
    const fingerprints = new Map(runFingerprints);
    const calls = [];
    for (let index = 0; index < items.length; index++) {
      if (!Object.prototype.hasOwnProperty.call(items, index)) throw new Error("runs.all items must not contain sparse entries.");
      const item = items[index];
      if (!item || typeof item !== "object" || Array.isArray(item)) throw new Error("runs.all item " + index + " must be an object.");
      const { key, ...params } = item;
      validateRunCall(key, params, "runs.all item " + index, fingerprints);
      calls.push({ key, params });
    }
    runFingerprints = fingerprints;
    const batch = { id: "batch-" + (++nextCallId), calls };
    const launched = calls.map(({ key, params }, index) =>
      ({ key, promise: launch(key, params, true, batch, generatedLaneKeys?.[index]) }));
    return { calls, launched };
  }

  function formatRef(result) {
    if (!result || typeof result !== "object") throw new Error("runs.ref(result) requires a run result object.");
    const parts = [];
    if (typeof result.key === "string" && result.key) parts.push(result.key);
    if (typeof result.runId === "string" && result.runId) parts.push("runId=" + result.runId);
    if (typeof result.outputReference === "string" && result.outputReference) parts.push("outputReference=" + result.outputReference);
    return parts.join(" ");
  }

  // Built per run from the capabilities the host granted, so a member the run may not use is
  // ABSENT rather than present-and-refused (SCOPE_3f 0.4): upstream's rule is that raw
  // workflowScript/workflowScriptPath "cannot use runs.host", and `undefined` is what the model
  // can actually discover.
  const runsSurface = {
    run(key, params) {
      validateRunCall(key, params, "runs.run", runFingerprints);
      return trackObservation(
        [{ operation: "run", key }],
        launch(key, params, false),
      );
    },
    all(items) {
      const { calls, launched } = launchRunsAll(items);
      return trackObservation(
        launched.map(({ key }) => ({ operation: "run", key })),
        Promise.all(launched.map(({ promise }) => promise))
          .then((results) => wrapRunsAllResults(results, calls.map(({ key }) => key))),
      );
    },
    lanes(laneSpecs) {
      return runLanes(laneSpecs);
    },
    host(key, params) {
      validateLaneKey(key, "runs.host");
      if (!params || typeof params !== "object" || Array.isArray(params)) {
        throw new Error("runs.host('" + key + "') params must be an object.");
      }
      assertJsonValue(params, "runs.host('" + key + "') params");
      const callId = ++nextCallId;
      return trackObservation(
        [{ operation: "host", key, callId }],
        ops.op_runs_host(callId, key, params),
      );
    },
    steer(key, message, options = {}) {
      if (typeof key !== "string" || !runKeyPattern.test(key)) throw new Error("runs.steer has an invalid key.");
      if (typeof message !== "string" || !message.trim()) throw new Error("runs.steer message must be a non-empty string.");
      if (!options || typeof options !== "object" || Array.isArray(options)) throw new Error("runs.steer options must be an object.");
      const allowed = new Set(["mode", "index", "ackTimeoutMs"]);
      for (const option of Object.keys(options)) {
        if (!allowed.has(option)) throw new Error("runs.steer options contain unsupported field '" + option + "'.");
      }
      if (options.mode !== undefined && !["steer", "follow_up", "auto"].includes(options.mode)) {
        throw new Error("runs.steer mode must be 'steer', 'follow_up', or 'auto'.");
      }
      if (options.index !== undefined && (!Number.isInteger(options.index) || options.index < 0 || options.index > 1000000)) {
        throw new Error("runs.steer index must be an integer between 0 and 1000000.");
      }
      if (options.ackTimeoutMs !== undefined && (!Number.isInteger(options.ackTimeoutMs) || options.ackTimeoutMs < 1)) {
        throw new Error("runs.steer ackTimeoutMs must be a positive integer.");
      }
      const callId = ++nextCallId;
      return trackObservation(
        [{ operation: "steer", key, callId }],
        ops.op_runs_steer(callId, key, message.trim(), options),
      );
    },
    status(keyOrRunId) {
      return ops.op_runs_status(typeof keyOrRunId === "string" ? keyOrRunId : "");
    },
    ref: formatRef,
    refs(results) {
      if (!Array.isArray(results)) throw new Error("runs.refs(results) requires an array.");
      return results.map(formatRef).join("\n");
    },
  };

  // --------------------------------------------------------------------------------------------
  // runs.lanes (`:342-561`) -- pure graph materialization, no host dependency
  // --------------------------------------------------------------------------------------------

  const MAX_LANES = 32;
  const MAX_LANE_STAGES = 16;

  function validateLaneSpecs(laneSpecs) {
    if (!Array.isArray(laneSpecs) || laneSpecs.length === 0) throw new Error("runs.lanes(lanes) requires a non-empty array.");
    if (laneSpecs.length > MAX_LANES) throw new Error("runs.lanes supports at most " + MAX_LANES + " lanes.");
    const laneKeys = new Set();
    const generatedKeys = new Set();
    const validationFingerprints = new Map(runFingerprints);
    const normalized = [];
    for (const lane of laneSpecs) {
      if (!lane || typeof lane !== "object" || Array.isArray(lane)) throw new Error("runs.lanes lane must be an object.");
      const laneKey = validateLaneKey(lane.key, "runs.lanes lane");
      if (laneKeys.has(laneKey)) throw new Error("runs.lanes lane key '" + laneKey + "' is duplicated.");
      laneKeys.add(laneKey);
      if (!Array.isArray(lane.stages) || lane.stages.length === 0) throw new Error("runs.lanes lane '" + laneKey + "' requires a non-empty stages array.");
      if (lane.stages.length > MAX_LANE_STAGES) throw new Error("runs.lanes lane '" + laneKey + "' supports at most " + MAX_LANE_STAGES + " stages.");
      const stages = [];
      for (let stageIndex = 0; stageIndex < lane.stages.length; stageIndex++) {
        const stage = lane.stages[stageIndex];
        if (!stage || typeof stage !== "object" || Array.isArray(stage)) throw new Error("runs.lanes stage must be an object.");
        const stageLabel = "runs.lanes lane '" + laneKey + "' stage " + stageIndex;
        const stageKey = validateLaneKey(stage.key, stageLabel);
        const generatedKey = laneKey + "." + stageKey;
        validateLaneKey(generatedKey, stageLabel + " generated");
        if (generatedKeys.has(generatedKey)) throw new Error("runs.lanes generated child key '" + generatedKey + "' is duplicated.");
        generatedKeys.add(generatedKey);
        const resume = stage.resume;
        if (resume !== undefined && resume !== "previous") {
          throw new Error(stageLabel + " resume must be 'previous'.");
        }
        if (stageIndex === 0 && resume === "previous") throw new Error(stageLabel + " cannot resume previous without a predecessor stage.");
        const { key: _k, resume: _r, ...params } = stage;
        const validationParams = resume === "previous" ? { ...params, resume: "retained-run-placeholder" } : params;
        validateRunCall(generatedKey, validationParams, stageLabel, validationFingerprints);
        stages.push({ key: stageKey, generatedKey, resume, params });
      }
      normalized.push({ key: laneKey, stages });
    }
    return normalized;
  }

  function laneStageIsBlocked(result) {
    const structured = result?.structuredOutput;
    return structured && typeof structured === "object" && !Array.isArray(structured) && structured.verdict === "blocked";
  }

  function laneStageRecord(key, result, forcedState) {
    const ok = result?.ok === true;
    const blocked = laneStageIsBlocked(result);
    const state = forcedState ?? (result?.stopped ? "stopped" : result?.detached ? "detached" : ok ? (blocked ? "blocked" : "completed") : "failed");
    const record = { key, state };
    if (typeof result?.runId === "string" && result.runId) record.runId = result.runId;
    if (state !== "completed") {
      record.error = state === "blocked" && blocked
        ? "Stage returned a blocked verdict."
        : (result?.error ?? result?.output ?? "Stage did not complete.");
    }
    return record;
  }

  function laneStageParams(stage, previous) {
    if (stage.resume !== "previous") return { ...stage.params };
    const runId = previous?.runId;
    if (typeof runId !== "string" || !runId.trim()) return undefined;
    return { ...stage.params, resume: runId };
  }

  function laneFailure(key, error) {
    const message = error instanceof Error ? error.message : String(error);
    return { key, ok: false, output: message, error: message };
  }

  function runLane(lane, firstResult, observe) {
    const records = [];
    const appendSkipped = (start) => {
      for (let i = start; i < lane.stages.length; i++) records.push({ key: lane.stages[i].key, state: "skipped" });
    };
    const finish = (state, failedStage) => ({ key: lane.key, state, ...(failedStage ? { failedStage } : {}), stages: records });
    const visit = (index, previous) => {
      if (index >= lane.stages.length) return Promise.resolve(finish("complete"));
      const stage = lane.stages[index];
      if (index === 0) {
        const record = laneStageRecord(stage.key, previous);
        records.push(record);
        if (record.state !== "completed") {
          appendSkipped(index + 1);
          return Promise.resolve(finish("blocked", stage.key));
        }
        return visit(index + 1, previous);
      }
      if (!previous || previous.ok !== true || laneStageIsBlocked(previous)) {
        records.push({ key: stage.key, state: "blocked", error: "Previous stage did not complete successfully." });
        appendSkipped(index + 1);
        return Promise.resolve(finish("blocked", stage.key));
      }
      const params = laneStageParams(stage, previous);
      if (!params) {
        records.push({ key: stage.key, state: "blocked", error: "Previous stage did not return a retained run id." });
        appendSkipped(index + 1);
        return Promise.resolve(finish("blocked", stage.key));
      }
      let launched;
      try {
        validateRunCall(stage.generatedKey, params, "runs.lanes stage", runFingerprints);
        // Later stages are observed as they are created -- the host keys observation per launch
        // key (SCOPE_3f 3.5), so a lane that advances after its aggregate was awaited still settles clean.
        observe([{ operation: "run", key: stage.generatedKey }]);
        launched = launch(stage.generatedKey, params, true, undefined, lane.key);
      } catch (error) {
        const failed = laneFailure(stage.generatedKey, error);
        records.push(laneStageRecord(stage.key, failed));
        appendSkipped(index + 1);
        return Promise.resolve(finish("blocked", stage.key));
      }
      return launched.then((result) => {
        const record = laneStageRecord(stage.key, result);
        records.push(record);
        if (record.state === "completed") return visit(index + 1, result);
        appendSkipped(index + 1);
        return finish("blocked", stage.key);
      }, (error) => {
        const failed = laneFailure(stage.generatedKey, error);
        records.push(laneStageRecord(stage.key, failed));
        appendSkipped(index + 1);
        return finish("blocked", stage.key);
      });
    };
    return visit(0, firstResult);
  }

  function workflowPlanStringMetadata(params) {
    const out = {};
    for (const field of ["phase", "label", "agent"]) {
      if (typeof params[field] === "string" && params[field].trim()) out[field] = params[field].trim();
    }
    return out;
  }

  function runLanes(laneSpecs) {
    const lanes = validateLaneSpecs(laneSpecs);
    try {
      ops.op_runs_lane_plan(JSON.stringify(lanes.map((lane) => ({
        key: lane.key,
        stages: lane.stages.map((stage) => ({
          key: stage.key,
          generatedKey: stage.generatedKey,
          ...workflowPlanStringMetadata(stage.params),
        })),
      }))));
    } catch {
      // The plan advisory is telemetry; it must never fail the run.
    }
    const firstItems = lanes.map((lane) => ({ key: lane.stages[0].generatedKey, ...lane.stages[0].params }));
    const firstBatch = launchRunsAll(firstItems, lanes.map((lane) => lane.key));
    const firstResults = firstBatch.launched.map(({ promise }) => promise);
    const laneObservations = firstBatch.launched.map(({ key }) => ({ operation: "run", key }));
    // Later-stage observations are reported AS THEY HAPPEN rather than pushed into a captured
    // array -- the one-shot-latch bug (SCOPE_3f 6 DL-9) that made every multi-stage lane fail settlement.
    const observe = (observations) => {
      for (const o of observations) {
        try {
          ops.op_workflow_observe(o.operation, o.key ?? "", 0n);
        } catch {
          // advisory
        }
      }
    };
    const laneAggregate = Promise.all(lanes.map((lane, index) => firstResults[index].then(
      (result) => runLane(lane, result, observe),
      (error) => runLane(lane, laneFailure(lane.stages[0].generatedKey, error), observe),
    )));
    return trackObservation(laneObservations, laneAggregate);
  }

  // --------------------------------------------------------------------------------------------
  // state / emit / console (`:770-776`)
  // --------------------------------------------------------------------------------------------

  const state = Object.freeze({
    get(key) {
      // An absent key is `undefined`, not `null` -- upstream's guest normalises the same way, and
      // scripts branch on `=== undefined`. The op returns Option<Value>, which crosses as null.
      return Promise.resolve()
        .then(() => ops.op_workflow_state_get(validateStateKey(key)))
        .then((raw) => (raw === null || raw === undefined ? undefined : raw));
    },
    set(key, value) {
      const validKey = validateStateKey(key);
      assertJsonValue(value, "state.set('" + validKey + "') value");
      return Promise.resolve().then(() => ops.op_workflow_state_set(validKey, value === undefined ? null : value));
    },
  });

  function inspectValue(value, depth = 4) {
    const seen = new Set();
    const walk = (entry, remaining) => {
      if (entry === null) return "null";
      if (entry === undefined) return "undefined";
      const kind = typeof entry;
      if (kind === "string") return entry;
      if (kind === "number" || kind === "boolean" || kind === "bigint") return String(entry);
      if (kind === "function") return "[Function: " + (entry.name || "anonymous") + "]";
      if (kind === "symbol") return entry.toString();
      if (entry instanceof Error) return entry.stack || entry.name + ": " + entry.message;
      if (seen.has(entry)) return "[Circular]";
      if (remaining < 0) return Array.isArray(entry) ? "[Array]" : "[Object]";
      seen.add(entry);
      try {
        if (Array.isArray(entry)) return "[ " + entry.map((i) => walk(i, remaining - 1)).join(", ") + " ]";
        return "{ " + Object.entries(entry).map(([k, v]) => k + ": " + walk(v, remaining - 1)).join(", ") + " }";
      } finally {
        seen.delete(entry);
      }
    };
    return walk(value, depth);
  }

  function consoleMethod(level) {
    return (...args) => {
      const text = args.map((a) => (typeof a === "string" ? a : inspectValue(a))).join(" ");
      ops.op_workflow_log(level, text);
    };
  }

  const capturedConsole = Object.freeze({
    log: consoleMethod("log"),
    info: consoleMethod("info"),
    warn: consoleMethod("warn"),
    error: consoleMethod("error"),
    debug: consoleMethod("log"),
    trace: consoleMethod("log"),
  });

  function emit(value) {
    const emittedValue = unwrapRunsAllResults(value);
    assertJsonValue(emittedValue);
    return ops.op_workflow_emit(emittedValue === undefined ? null : emittedValue);
  }

  // --------------------------------------------------------------------------------------------
  // The sandbox surface (SCOPE_3f 3.3) -- upstream `:916`
  // --------------------------------------------------------------------------------------------

  // The realm is assembled by the HOST, in one call, before any agent script exists -- so no script
  // can observe an intermediate state where a capability is still reachable. `install_sandbox` in
  // engine.rs is the only caller, and it deletes this hook immediately afterwards.
  globalThis.__cyrupWorkflowInstall = ({ stateEnabled, hostEnabled }) => {
    const surface = { ...runsSurface };
    if (!hostEnabled) delete surface.host;
    globalThis.runs = Object.freeze(surface);
    globalThis.emit = emit;
    globalThis.console = capturedConsole;
    if (stateEnabled) globalThis.state = state;
  };
  globalThis.__cyrupWorkflowOmitUndefined = omitUndefinedWorkflowValues;

  // The host awaits a TAGGED OUTCOME rather than a raw promise, which is upstream's worker-message
  // shape (`{type:"error", error, errorKind}`) and exists for one concrete reason: a rejection
  // carries `workflowErrorKind` as a property on the Error, and V8->Rust exception conversion does
  // NOT preserve arbitrary error properties. Returning the tag as data is the only way the
  // detached-child kind survives the boundary.
  globalThis.__cyrupWorkflowRun = (bodyPromise) =>
    bodyPromise.then(
      (value) => {
        const persisted = value === undefined ? null : omitUndefinedWorkflowValues(value);
        try {
          assertJsonValue(persisted, "return");
        } catch (error) {
          return { ok: false, message: String(error && error.message ? error.message : error), errorPhase: "return-serialization" };
        }
        return { ok: true, value: persisted };
      },
      (error) => ({
        ok: false,
        message: String(error && error.message ? error.message : error),
        ...(error && error.workflowErrorKind === "detached-child" ? { errorKind: "detached-child" } : {}),
      }),
    );
})(globalThis);
