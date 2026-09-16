//! The workflow resource registry and resolver — ports pi `workflows/workflow-resources.ts`
//! (225 LOC @ `df26ebc8`, including commit `1deda864`'s discriminated-union `normalizeArgs`).
//!
//! Resolve only extension-owned resources so policy can distinguish them from raw scripts;
//! caller-provided script text is never consulted (pi `:178`'s doc, the reason
//! [`ResolvedWorkflowResource`] carries a [`WorkflowResourcePermit`]).
//!
//! Upstream hangs its registry off a `Symbol.for` global (`:45-56`); here it is a value the
//! executor owns (`extension/executor/mod.rs`, beside `completion_bus`, for that field's stated
//! reason: a `static` cannot be reset between sessions). The two "malformed registry" guards
//! (`:55`, `:73`) are structurally absent — the registry is a typed field, not a global an
//! arbitrary writer could clobber.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use serde_json::{Map, Value};

use crate::identity::SessionId;

use super::key::WorkflowKey;
use super::permit::{
    WorkflowResourceAuthority, WorkflowResourceHostAuthority, WorkflowResourceId,
    WorkflowResourcePermit, WorkflowResourcePermitInput, WorkflowResourceProvenance,
};
use super::stable_json::stable_json_digest;

/// pi `MAX_ARGS_BYTES` (`workflow-resources.ts:11`).
const MAX_ARGS_BYTES: usize = 16 * 1024;
/// pi `MAX_STRING_BYTES` (`workflow-resources.ts:12`).
const MAX_STRING_BYTES: usize = 16 * 1024;

/// What a resource's `resolve` returns on success — the expansion half of pi's
/// `WorkflowResourceDefinition["resolve"]` return union (`workflow-resources.ts:27`).
///
/// The union itself is the `Result` in [`WorkflowResourceResolve`]: a resolver returning neither
/// an expansion nor an error, or both, is unrepresentable — which is exactly what upstream's
/// runtime shape checks at `:200`/`:206` exist to reject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowResourceExpansion {
    /// The workflow script text this resource expands to.
    pub script: String,
    /// Host-command grants the script is entitled to (`runs.host` authority).
    pub host_commands: Option<Vec<WorkflowResourceHostAuthority>>,
}

/// A resource's trusted synchronous validation/expansion function — pi
/// `WorkflowResourceDefinition["resolve"]` (`workflow-resources.ts:24-29`; "The extension owns
/// semantic command binding").
///
/// Two upstream guards are structurally absent here: "resolve must be synchronous" (`:196-199` —
/// a plain `Fn` returns no future) and "must be a function" (`:66` — the field's type).
pub type WorkflowResourceResolve =
    Arc<dyn Fn(&Map<String, Value>) -> Result<WorkflowResourceExpansion, String> + Send + Sync>;

/// A registered workflow resource — pi `WorkflowResourceDefinition`
/// (`workflow-resources.ts:24-29`).
///
/// `name` is a [`WorkflowKey`] by construction (pi's resource-name pattern IS the key grammar,
/// SCOPE_3d §0.14), which deletes upstream's two `.test()` calls (`:65`, `:191`) and makes the
/// protected-builtin comparison a typed equality. The raw-input boundary that maps a grammar
/// failure to `"Workflow definition requires a safe resource name."` is whichever surface accepts
/// an untyped definition (the workflow runtime's registration API, SCOPE_3f) — the parser owns
/// the grammar, that call site owns the wording.
#[derive(Clone)]
pub struct WorkflowResourceDefinition {
    /// The resource's name — the lookup key.
    pub name: WorkflowKey,
    /// The resource's version (positive integer; `u32`, comfortably inside upstream's
    /// safe-integer bound).
    pub version: u32,
    /// The trusted synchronous resolver.
    pub resolve: WorkflowResourceResolve,
}

impl std::fmt::Debug for WorkflowResourceDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowResourceDefinition")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("resolve", &"<fn>")
            .finish()
    }
}

/// A fully resolved workflow resource — pi `ResolvedWorkflowResource`
/// (`workflow-resources.ts:14-18`).
#[derive(Debug)]
pub struct ResolvedWorkflowResource {
    /// The expanded workflow script text.
    pub script: String,
    /// The single-consumption permit binding the script to its resolution.
    pub permit: WorkflowResourcePermit,
    /// The resolution's audit provenance (read back off the permit — one constructor).
    pub provenance: WorkflowResourceProvenance,
}

/// The resolution outcome — pi `WorkflowResourceResolution` (`workflow-resources.ts:20-22`), a
/// union ported as an enum, never a struct with an `ok` flag.
#[derive(Debug)]
pub enum WorkflowResourceResolution {
    /// The resource resolved; the script is extension-owned.
    Ok(ResolvedWorkflowResource),
    /// The resolution was refused; the message is bounded to 4096 bytes.
    Err(String),
}

type SessionBuckets = HashMap<SessionId, HashMap<WorkflowKey, Arc<WorkflowResourceDefinition>>>;

/// The session-scoped workflow resource registry — pi's `Symbol.for`-global
/// `WorkflowResourceRegistry` (`workflow-resources.ts:41-56`), as an executor-owned value.
///
/// Session ID scopes lookup, not authentication. Dispose on `session_shutdown`
/// ([`WorkflowResourceRegistry::dispose_session`]); issued permits remain valid (pi `:58`).
#[derive(Clone, Debug, Default)]
pub struct WorkflowResourceRegistry {
    inner: Arc<Mutex<SessionBuckets>>,
}

/// The disposal handle a registration returns — pi `WorkflowResourceRegistration`
/// (`workflow-resources.ts:31-33`, closure at `:76-84`): `dispose` is **idempotent** and
/// **identity-checked** (it removes the entry only while the stored snapshot is still *its*
/// snapshot, so a re-registration after a dispose is never deleted by the stale handle), and
/// `Drop` disposes too, so a dropped handle cannot leak its registration.
#[derive(Debug)]
pub struct WorkflowResourceRegistration {
    registry: Weak<Mutex<SessionBuckets>>,
    session_id: SessionId,
    name: WorkflowKey,
    snapshot: Weak<WorkflowResourceDefinition>,
    disposed: bool,
}

impl WorkflowResourceRegistration {
    /// Idempotent, identity-checked disposal (see the type doc). Upstream additionally checks the
    /// session's bucket is "still the same bucket" before dropping an emptied session entry
    /// (`:83`) — structurally covered here: this always operates on the LIVE map, so the bucket
    /// it sees is by definition the current one.
    pub fn dispose(&mut self) {
        if self.disposed {
            return;
        }
        self.disposed = true;
        let Some(inner) = self.registry.upgrade() else {
            return;
        };
        let mut sessions = inner.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(bucket) = sessions.get_mut(&self.session_id) else {
            return;
        };
        let still_ours = bucket.get(&self.name).is_some_and(|stored| {
            self.snapshot
                .upgrade()
                .is_some_and(|snapshot| Arc::ptr_eq(stored, &snapshot))
        });
        if still_ours {
            bucket.remove(&self.name);
        }
        if bucket.is_empty() {
            sessions.remove(&self.session_id);
        }
    }
}

impl Drop for WorkflowResourceRegistration {
    fn drop(&mut self) {
        self.dispose();
    }
}

impl WorkflowResourceRegistry {
    /// A fresh, empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// pi `registerWorkflowResource` (`workflow-resources.ts:58-85`). Session ID scopes lookup,
    /// not authentication.
    ///
    /// Upstream's shape guards ("only sessionId and definition", "only name, version and
    /// resolve", "resolve must be a function", the two malformed-registry throws) are structurally
    /// absent; the name grammar is [`WorkflowKey`]'s by construction. What remains, in upstream's
    /// order, is below.
    ///
    /// # Errors
    ///
    /// Upstream's messages verbatim: the sessionId rule (`:61` — stricter than
    /// [`SessionId::parse`]'s non-empty rule, kept HERE as a registration-site check because
    /// tightening the shared type would break the path-shaped ids it documents), the version rule
    /// (`:65`), the protected-builtin collision (`:67`) and the same-session duplicate (`:70`).
    pub fn register(
        &self,
        session_id: &SessionId,
        definition: WorkflowResourceDefinition,
    ) -> Result<WorkflowResourceRegistration, String> {
        let raw_session = session_id.as_str();
        // pi compares `.length` — UTF-16 code units — so the bound is counted the same way.
        if raw_session.trim() != raw_session
            || raw_session.encode_utf16().count() > 256
            || raw_session.contains('\0')
        {
            return Err(
                "Workflow registration requires a non-empty trimmed sessionId of at most 256 characters without NUL."
                    .to_string(),
            );
        }
        if definition.version < 1 {
            return Err("Workflow definition version must be a positive safe integer.".to_string());
        }
        if find_builtin(&definition.name).is_some() {
            return Err(format!(
                "Workflow resource '{}' is a protected builtin.",
                definition.name.as_str()
            ));
        }
        let mut sessions = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let bucket = sessions.entry(session_id.clone()).or_default();
        if bucket.contains_key(&definition.name) {
            return Err(format!(
                "Workflow resource '{}' is already registered in this session; dispose it first.",
                definition.name.as_str()
            ));
        }
        let name = definition.name.clone();
        let snapshot = Arc::new(definition);
        bucket.insert(name.clone(), Arc::clone(&snapshot));
        Ok(WorkflowResourceRegistration {
            registry: Arc::downgrade(&self.inner),
            session_id: session_id.clone(),
            name,
            snapshot: Arc::downgrade(&snapshot),
            disposed: false,
        })
    }

    /// Drop every registration a session holds — the `session_shutdown` hook pi's `:58` doc
    /// prescribes. Issued permits remain valid.
    pub fn dispose_session(&self, session_id: &SessionId) {
        let mut sessions = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        sessions.remove(session_id);
    }

    /// pi `resolveWorkflowResource` (`workflow-resources.ts:178-186`) — the public wrapper: it
    /// catches everything the inner resolution throws and truncates every error (business
    /// refusals included) to 4096 bytes on a char boundary, because a resource's `resolve` is
    /// third-party code and its error text is untrusted.
    #[must_use]
    pub fn resolve(
        &self,
        name_value: &Value,
        args_value: Option<&Value>,
        session_id: Option<&SessionId>,
    ) -> WorkflowResourceResolution {
        match self.resolve_resource(name_value, args_value, session_id) {
            Ok(WorkflowResourceResolution::Ok(resource)) => {
                WorkflowResourceResolution::Ok(resource)
            }
            Ok(WorkflowResourceResolution::Err(error)) | Err(error) => {
                WorkflowResourceResolution::Err(truncate_error(&error))
            }
        }
    }

    /// pi `resolveResource` (`workflow-resources.ts:188-225`) — the two-layer split is upstream's
    /// and is kept: `Err` here is the THROW channel (invalid resolver behaviour), while
    /// `Ok(WorkflowResourceResolution::Err(..))` is an expected refusal.
    fn resolve_resource(
        &self,
        name_value: &Value,
        args_value: Option<&Value>,
        session_id: Option<&SessionId>,
    ) -> Result<WorkflowResourceResolution, String> {
        let Some(raw_name) = name_value
            .as_str()
            .map(str::trim)
            .filter(|trimmed| !trimmed.is_empty())
        else {
            return Ok(WorkflowResourceResolution::Err(
                "workflow must be a non-empty resource name.".to_string(),
            ));
        };
        let Ok(name) = WorkflowKey::parse(raw_name) else {
            return Ok(WorkflowResourceResolution::Err(
                "workflow must use a safe resource name.".to_string(),
            ));
        };
        // Builtins first, then the caller session's registrations (`:191`-adjacent lookup order).
        let resource: Option<Arc<WorkflowResourceDefinition>> =
            find_builtin(&name).map(Arc::new).or_else(|| {
                session_id.and_then(|session_id| {
                    self.inner
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get(session_id)
                        .and_then(|bucket| bucket.get(&name).cloned())
                })
            });
        let Some(resource) = resource else {
            return Ok(WorkflowResourceResolution::Err(format!(
                "Unknown workflow resource '{}'. Available resources: {}.",
                name.as_str(),
                BUILTIN_NAMES.join(", ")
            )));
        };
        let args = match normalize_args(args_value) {
            Ok(args) => args,
            Err(error) => return Ok(WorkflowResourceResolution::Err(error)),
        };
        let expansion = match (resource.resolve)(&args) {
            Ok(expansion) => expansion,
            Err(error) => {
                // pi `:201-204`: an error object with extra keys is structurally absent
                // (`Err(String)`); the non-blank check is the half that remains.
                if error.trim().is_empty() {
                    return Err("Workflow resource returned an invalid error.".to_string());
                }
                return Ok(WorkflowResourceResolution::Err(error));
            }
        };
        // pi `:205-207`: extra expansion keys are structurally absent; a blank script is not.
        if expansion.script.trim().is_empty() {
            return Err("Workflow resource returned an invalid expansion.".to_string());
        }
        // `randomUUID()` — the HYPHENATED 36-char form (SCOPE_3d §0.22), which lands on the wire
        // as `WorkflowResourceProvenance.id`; deliberately not `RunId::new`'s `as_simple` idiom.
        // A hyphenated UUIDv4 always matches `WorkflowResourceId`'s grammar (WORKFLOW_3 §0.10), so
        // this `ok_or_else` names an internal-error diagnostic that never actually fires rather
        // than `.expect`ing it away (this crate denies `clippy::expect_used` outside tests).
        let resource_id =
            WorkflowResourceId::parse(&uuid::Uuid::new_v4().to_string()).ok_or_else(|| {
                "internal error: minted resource id failed its own grammar.".to_string()
            })?;
        let permit = WorkflowResourcePermit::issue(WorkflowResourcePermitInput {
            resource_name: resource.name.clone(),
            resource_version: resource.version,
            resource_id,
            script_digest: stable_json_digest(&Value::String(expansion.script.clone())),
            authority: WorkflowResourceAuthority {
                host: expansion.host_commands.clone(),
            },
        })?;
        let provenance = permit.provenance().clone();
        Ok(WorkflowResourceResolution::Ok(ResolvedWorkflowResource {
            script: expansion.script,
            permit,
            provenance,
        }))
    }
}

/// Truncate an untrusted error to 4096 bytes on a char boundary — pi's `.slice(0, 4096)` at
/// `workflow-resources.ts:182`/`:184`, applied byte-wise here without ever splitting a scalar.
fn truncate_error(error: &str) -> String {
    if error.len() <= 4096 {
        return error.to_string();
    }
    let mut end = 4096;
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    error.get(..end).unwrap_or_default().to_string()
}

/// pi `isPlainRecord` for a [`Value`]: only a JSON object counts (class instances and exotic
/// prototypes are structurally absent).
fn is_plain_record(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

/// pi `jsonByteLength` (`workflow-resources.ts:93-101`): the UTF-8 byte length of the compact
/// JSON encoding. The `undefined`/unencodable throw paths are structurally absent for a [`Value`];
/// an encoder failure degrades to upstream's prefixed message.
fn json_byte_length(value: &Value) -> Result<usize, String> {
    serde_json::to_string(value)
        .map(|encoded| encoded.len())
        .map_err(|error| format!("must contain plain JSON data: {error}"))
}

/// pi `validatePlainJson` (`workflow-resources.ts:103-126`) — the bounded-JSON validator, limits
/// verbatim: depth > 8, blank or > 16 KiB strings, array length > 64, object fields > 16, blank
/// field names; errors are path-prefixed (`workflow args.foo[2].bar is too deeply nested.`).
///
/// pi's non-finite-number rejection (`:115`) is structurally absent — a JSON [`Value`] cannot
/// hold one — and its non-plain-object rejection (`:120`) likewise.
fn validate_plain_json(value: &Value, path: &str, depth: u32) -> Result<(), String> {
    if depth > 8 {
        return Err(format!("{path} is too deeply nested."));
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
        Value::String(text) => {
            if text.trim().is_empty() {
                return Err(format!("{path} must not be empty."));
            }
            if text.len() > MAX_STRING_BYTES {
                return Err(format!("{path} exceeds {MAX_STRING_BYTES} bytes."));
            }
            Ok(())
        }
        Value::Array(items) => {
            if items.len() > 64 {
                return Err(format!("{path} contains too many items."));
            }
            for (index, entry) in items.iter().enumerate() {
                validate_plain_json(entry, &format!("{path}[{index}]"), depth + 1)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            if map.len() > 16 {
                return Err(format!("{path} contains too many fields."));
            }
            for (key, entry) in map {
                if key.trim().is_empty() {
                    return Err(format!("{path} contains an empty field name."));
                }
                validate_plain_json(entry, &format!("{path}.{key}"), depth + 1)?;
            }
            Ok(())
        }
    }
}

/// pi `normalizeArgs` (`workflow-resources.ts:128-138`) — the discriminated-union form upstream
/// commit `1deda864` landed: `Result<Map, String>`, never an optional-field bag.
///
/// `pub(crate)` (re-exported as [`crate::workflows::normalize_workflow_args`]) because upstream
/// has a SECOND caller outside this file: `parseScheduleTarget`
/// (`runs/background/scheduled-runs.ts:293`) runs a persisted schedule's `target.args` through
/// exactly this normalisation on every read. Two implementations of "what a workflow's args may
/// be" would let a schedule persist args the workflow runtime then refuses.
pub(crate) fn normalize_args(value: Option<&Value>) -> Result<Map<String, Value>, String> {
    let Some(value) = value else {
        return Ok(Map::new());
    };
    let Some(map) = is_plain_record(value) else {
        return Err("workflow args must be a plain JSON object.".to_string());
    };
    validate_plain_json(value, "workflow args", 0)?;
    if json_byte_length(value)? > MAX_ARGS_BYTES {
        return Err(format!("workflow args exceed {MAX_ARGS_BYTES} bytes."));
    }
    // pi deep-clones via `JSON.parse(JSON.stringify(value))`; a `Value` clone is that, minus the
    // re-encode.
    Ok(map.clone())
}

/// pi `resolveRunCi` (`workflow-resources.ts:140-153`).
fn resolve_run_ci(args: &Map<String, Value>) -> Result<WorkflowResourceExpansion, String> {
    let unsupported: Vec<&str> = args
        .keys()
        .map(String::as_str)
        .filter(|key| *key != "command" && *key != "timeoutMs")
        .collect();
    if !unsupported.is_empty() {
        return Err(format!(
            "workflow 'run-ci' args contain unsupported fields: {}.",
            unsupported.join(", ")
        ));
    }
    let command = match args.get("command") {
        None => "npm test",
        Some(Value::String(command)) if command == "npm test" || command == "npm run typecheck" => {
            command
        }
        Some(_) => {
            return Err(
                "workflow 'run-ci' args.command must be 'npm test' or 'npm run typecheck'."
                    .to_string(),
            );
        }
    };
    let timeout_ms: i64 = match args.get("timeoutMs") {
        None => 120_000,
        Some(value) => {
            // JS `Number.isInteger` accepts `120000.0`; so does this (a finite float with no
            // fractional part), while anything else — including a non-number — falls to the
            // refusal.
            let integer = value
                .as_f64()
                .filter(|float| float.fract() == 0.0 && (1.0..=86_400_000.0).contains(float));
            match integer {
                #[allow(clippy::cast_possible_truncation)]
                Some(float) => float as i64,
                None => {
                    return Err(
                        "workflow 'run-ci' args.timeoutMs must be an integer from 1 to 86400000."
                            .to_string(),
                    );
                }
            }
        }
    };
    // Rendered by hand so the script text matches upstream's
    // `JSON.stringify({ kind: "command", command, timeoutMs, role: "ci" })` byte-for-byte —
    // `serde_json`'s default map would re-sort the keys.
    let command_json = Value::String(command.to_string()).to_string();
    let script = format!(
        "return await runs.host(\"ci\", {{\"kind\":\"command\",\"command\":{command_json},\"timeoutMs\":{timeout_ms},\"role\":\"ci\"}});"
    );
    let key = WorkflowKey::parse("ci")
        .map_err(|_| "workflow 'run-ci' host grant key is invalid.".to_string())?;
    Ok(WorkflowResourceExpansion {
        script,
        host_commands: Some(vec![WorkflowResourceHostAuthority {
            key,
            command: command.to_string(),
        }]),
    })
}

/// pi `resolveReview` (`workflow-resources.ts:155-163`).
fn resolve_review(args: &Map<String, Value>) -> Result<WorkflowResourceExpansion, String> {
    let unsupported: Vec<&str> = args
        .keys()
        .map(String::as_str)
        .filter(|key| *key != "task")
        .collect();
    if !unsupported.is_empty() {
        return Err(format!(
            "workflow 'review' args contain unsupported fields: {}.",
            unsupported.join(", ")
        ));
    }
    let task = args
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|trimmed| !trimmed.is_empty());
    let Some(task) = task else {
        return Err("workflow 'review' requires a non-empty string args.task.".to_string());
    };
    let task_json = Value::String(task.to_string()).to_string();
    Ok(WorkflowResourceExpansion {
        script: format!(
            "return (await runs.run(\"review\", {{ agent: \"reviewer\", task: {task_json} }})).output;"
        ),
        host_commands: None,
    })
}

/// The builtin names, in pi `WORKFLOW_RESOURCES` order (`workflow-resources.ts:165-168`) — the
/// list `listWorkflowResourceNames` renders into the unknown-resource message (builtins only,
/// never session registrations, exactly as upstream).
const BUILTIN_NAMES: [&str; 2] = ["review", "run-ci"];

/// pi `findWorkflowResource` (`workflow-resources.ts:170-172`), over the two builtins.
fn find_builtin(name: &WorkflowKey) -> Option<WorkflowResourceDefinition> {
    match name.as_str() {
        "review" => Some(WorkflowResourceDefinition {
            name: name.clone(),
            version: 1,
            resolve: Arc::new(resolve_review),
        }),
        "run-ci" => Some(WorkflowResourceDefinition {
            name: name.clone(),
            version: 1,
            resolve: Arc::new(resolve_run_ci),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde_json::json;

    use super::*;

    fn session(raw: &str) -> SessionId {
        SessionId::parse(raw).expect("non-empty")
    }

    fn expect_err(resolution: WorkflowResourceResolution) -> String {
        match resolution {
            WorkflowResourceResolution::Err(error) => error,
            WorkflowResourceResolution::Ok(resource) => {
                panic!("expected a refusal, resolved {:?}", resource.provenance)
            }
        }
    }

    fn expect_ok(resolution: WorkflowResourceResolution) -> ResolvedWorkflowResource {
        match resolution {
            WorkflowResourceResolution::Ok(resource) => resource,
            WorkflowResourceResolution::Err(error) => panic!("expected a resolution: {error}"),
        }
    }

    /// The `review` builtin end-to-end: exact script text (trimmed task, JSON-escaped), a permit
    /// that consumes against that script, and a three-literal provenance with a hyphenated UUID.
    #[test]
    fn review_builtin_resolves_with_permit_and_provenance() {
        let registry = WorkflowResourceRegistry::new();
        let mut resolved = expect_ok(registry.resolve(
            &json!("review"),
            Some(&json!({ "task": "  check the diff \"now\"  " })),
            None,
        ));
        assert_eq!(
            resolved.script,
            "return (await runs.run(\"review\", { agent: \"reviewer\", task: \"check the diff \\\"now\\\"\" })).output;"
        );
        assert_eq!(resolved.provenance.version.value(), 1);
        assert_eq!(resolved.provenance.name.as_str(), "review");
        assert_eq!(
            resolved.provenance.id.as_str().len(),
            36,
            "hyphenated UUID (§0.22)"
        );
        assert_eq!(resolved.provenance.id.as_str().matches('-').count(), 4);
        assert_eq!(resolved.provenance, *resolved.permit.provenance());
        let consumption = resolved
            .permit
            .consume(&resolved.script)
            .expect("digest matches");
        assert_eq!(
            consumption.authority.host, None,
            "review grants no host commands"
        );
    }

    /// The `run-ci` builtin: defaults, the exact upstream script rendering (key order and all),
    /// and a consumable host grant for the chosen command.
    #[test]
    fn run_ci_builtin_defaults_and_grants_host() {
        let registry = WorkflowResourceRegistry::new();
        let mut resolved = expect_ok(registry.resolve(&json!("run-ci"), None, None));
        assert_eq!(
            resolved.script,
            "return await runs.host(\"ci\", {\"kind\":\"command\",\"command\":\"npm test\",\"timeoutMs\":120000,\"role\":\"ci\"});"
        );
        resolved.permit.consume(&resolved.script).expect("consumes");
        assert_eq!(resolved.permit.authorize_host("ci", "npm test"), Ok(()));
        assert!(
            resolved
                .permit
                .authorize_host("ci", "npm run typecheck")
                .is_err()
        );

        // Explicit args, including a JS-integer-shaped float timeout.
        let explicit = expect_ok(registry.resolve(
            &json!("run-ci"),
            Some(&json!({ "command": "npm run typecheck", "timeoutMs": 5_000.0 })),
            None,
        ));
        assert!(
            explicit
                .script
                .contains("\"command\":\"npm run typecheck\"")
        );
        assert!(explicit.script.contains("\"timeoutMs\":5000,"));
    }

    /// The refusal ladder, upstream wording: bad name value, unsafe name, unknown resource
    /// (listing builtins only), builtin arg validation.
    #[test]
    fn resolution_refusals_carry_upstream_messages() {
        let registry = WorkflowResourceRegistry::new();
        assert_eq!(
            expect_err(registry.resolve(&json!(7), None, None)),
            "workflow must be a non-empty resource name."
        );
        assert_eq!(
            expect_err(registry.resolve(&json!("   "), None, None)),
            "workflow must be a non-empty resource name."
        );
        assert_eq!(
            expect_err(registry.resolve(&json!("bad/name"), None, None)),
            "workflow must use a safe resource name."
        );
        assert_eq!(
            expect_err(registry.resolve(&json!("nope"), None, None)),
            "Unknown workflow resource 'nope'. Available resources: review, run-ci."
        );
        assert_eq!(
            expect_err(registry.resolve(&json!("review"), Some(&json!({ "task": 3 })), None)),
            "workflow 'review' requires a non-empty string args.task."
        );
        assert_eq!(
            expect_err(registry.resolve(&json!("review"), Some(&json!({ "extra": "x" })), None)),
            "workflow 'review' args contain unsupported fields: extra."
        );
        assert_eq!(
            expect_err(registry.resolve(
                &json!("run-ci"),
                Some(&json!({ "command": "rm -rf /" })),
                None
            )),
            "workflow 'run-ci' args.command must be 'npm test' or 'npm run typecheck'."
        );
        assert_eq!(
            expect_err(registry.resolve(
                &json!("run-ci"),
                Some(&json!({ "timeoutMs": 1.5 })),
                None
            )),
            "workflow 'run-ci' args.timeoutMs must be an integer from 1 to 86400000."
        );
    }

    /// `normalizeArgs`/`validatePlainJson`: the path-prefixed messages, the byte budgets, and the
    /// plain-object requirement.
    #[test]
    fn args_validation_is_path_prefixed_and_bounded() {
        let registry = WorkflowResourceRegistry::new();
        assert_eq!(
            expect_err(registry.resolve(&json!("review"), Some(&json!([1])), None)),
            "workflow args must be a plain JSON object."
        );
        assert_eq!(
            expect_err(registry.resolve(
                &json!("review"),
                Some(&json!({ "task": ["a", ""] })),
                None
            )),
            "workflow args.task[1] must not be empty."
        );
        let deep = json!({ "a": { "b": { "c": { "d": { "e": { "f": { "g": { "h": { "i": 1 } } } } } } } } });
        assert_eq!(
            expect_err(registry.resolve(&json!("review"), Some(&deep), None)),
            "workflow args.a.b.c.d.e.f.g.h.i is too deeply nested."
        );
        let big = json!({ "task": "x".repeat(17 * 1024) });
        assert_eq!(
            expect_err(registry.resolve(&json!("review"), Some(&big), None)),
            "workflow args.task exceeds 16384 bytes."
        );
        let many: Map<String, Value> = (0..17)
            .map(|index| (format!("k{index}"), json!(1)))
            .collect();
        assert_eq!(
            expect_err(registry.resolve(&json!("review"), Some(&Value::Object(many)), None)),
            "workflow args contains too many fields."
        );
    }

    /// Registration: session scoping, the protected-builtin refusal, the same-session duplicate,
    /// identity-checked idempotent disposal, and third-party error handling (blank error →
    /// upstream's invalid-error message; long error → 4096-byte char-boundary truncation).
    #[test]
    fn registry_scopes_disposes_and_bounds_third_party_errors() {
        let registry = WorkflowResourceRegistry::new();
        let session_a = session("session-a");
        let session_b = session("session-b");
        let definition =
            |version: u32, resolve: WorkflowResourceResolve| WorkflowResourceDefinition {
                name: WorkflowKey::parse("custom").expect("valid"),
                version,
                resolve,
            };
        let ok_resolve: WorkflowResourceResolve = Arc::new(|_| {
            Ok(WorkflowResourceExpansion {
                script: "return 1;".to_string(),
                host_commands: None,
            })
        });
        assert_eq!(
            registry
                .register(
                    &session_a,
                    WorkflowResourceDefinition {
                        name: WorkflowKey::parse("review").expect("valid"),
                        version: 1,
                        resolve: Arc::clone(&ok_resolve),
                    }
                )
                .err(),
            Some("Workflow resource 'review' is a protected builtin.".to_string())
        );
        assert_eq!(
            registry
                .register(&session_a, definition(0, Arc::clone(&ok_resolve)))
                .err(),
            Some("Workflow definition version must be a positive safe integer.".to_string())
        );
        assert_eq!(
            registry
                .register(&session(" padded "), definition(1, Arc::clone(&ok_resolve)))
                .err(),
            Some("Workflow registration requires a non-empty trimmed sessionId of at most 256 characters without NUL.".to_string())
        );

        let mut first = registry
            .register(&session_a, definition(1, Arc::clone(&ok_resolve)))
            .expect("registers");
        assert_eq!(
            registry.register(&session_a, definition(2, Arc::clone(&ok_resolve))).err(),
            Some("Workflow resource 'custom' is already registered in this session; dispose it first.".to_string())
        );
        // Session scoping: session B cannot see session A's resource.
        assert_eq!(
            expect_err(registry.resolve(&json!("custom"), None, Some(&session_b))),
            "Unknown workflow resource 'custom'. Available resources: review, run-ci."
        );
        let resolved = expect_ok(registry.resolve(&json!("custom"), None, Some(&session_a)));
        assert_eq!(resolved.script, "return 1;");

        // Identity-checked disposal: dispose, re-register, dispose the STALE handle again — the
        // new registration must survive.
        first.dispose();
        let _second = registry
            .register(&session_a, definition(2, Arc::clone(&ok_resolve)))
            .expect("re-registers after dispose");
        first.dispose();
        expect_ok(registry.resolve(&json!("custom"), None, Some(&session_a)));

        // Third-party resolver errors: blank is the resolver's OWN bug (upstream's throw), long
        // ones truncate on a char boundary.
        let blank: WorkflowResourceResolve = Arc::new(|_| Err("   ".to_string()));
        let long: WorkflowResourceResolve = Arc::new(|_| Err(format!("{}é", "x".repeat(4095))));
        drop(
            registry
                .register(
                    &session_b,
                    WorkflowResourceDefinition {
                        name: WorkflowKey::parse("blank").expect("valid"),
                        version: 1,
                        resolve: blank,
                    },
                )
                .expect("registers"),
        );
        // The handle above was dropped, disposing it — register again under a kept handle.
        let _blank_reg = registry
            .register(
                &session_b,
                WorkflowResourceDefinition {
                    name: WorkflowKey::parse("blank").expect("valid"),
                    version: 1,
                    resolve: Arc::new(|_| Err("   ".to_string())),
                },
            )
            .expect("registers");
        let _long_reg = registry
            .register(
                &session_b,
                WorkflowResourceDefinition {
                    name: WorkflowKey::parse("long").expect("valid"),
                    version: 1,
                    resolve: long,
                },
            )
            .expect("registers");
        assert_eq!(
            expect_err(registry.resolve(&json!("blank"), None, Some(&session_b))),
            "Workflow resource returned an invalid error."
        );
        let truncated = expect_err(registry.resolve(&json!("long"), None, Some(&session_b)));
        assert_eq!(
            truncated.len(),
            4095,
            "the 2-byte 'é' straddling 4096 is dropped whole"
        );
        assert!(truncated.chars().all(|c| c == 'x'));

        // A blank script is the resolver's own bug too.
        let _blank_script = registry
            .register(
                &session_b,
                WorkflowResourceDefinition {
                    name: WorkflowKey::parse("empty-script").expect("valid"),
                    version: 1,
                    resolve: Arc::new(|_| {
                        Ok(WorkflowResourceExpansion {
                            script: "  ".to_string(),
                            host_commands: None,
                        })
                    }),
                },
            )
            .expect("registers");
        assert_eq!(
            expect_err(registry.resolve(&json!("empty-script"), None, Some(&session_b))),
            "Workflow resource returned an invalid expansion."
        );

        // dispose_session drops everything the session held.
        registry.dispose_session(&session_b);
        assert_eq!(
            expect_err(registry.resolve(&json!("blank"), None, Some(&session_b))),
            "Unknown workflow resource 'blank'. Available resources: review, run-ci."
        );
    }
}
