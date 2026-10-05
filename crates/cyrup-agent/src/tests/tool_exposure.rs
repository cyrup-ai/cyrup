//! CODE-005, agent-loop half — what a provider request declares versus what the loop will run.
//!
//! Every assertion here reads the schema the PROVIDER RECEIVES (`Context::tools` at the
//! `StreamFn` boundary), not an intermediate list: that is the confidentiality boundary the
//! exposure model exists to hold. Pi's rule (`core/agent-session.ts:1528-1572` @v1.0.1): the
//! active set is what is declared; `hidden` tools are dropped from it; a `prepare_loadout`
//! hook's `hiddenDeclarations` stay executable but leave the request.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::{Agent, StreamFn};
use cyrup_core::{
    CancelToken, Content, EventStream, LoadoutView, ModelRef, StopReason, Tool, ToolCallId,
    ToolError, ToolExposure, ToolLoadout, ToolLoadoutChanges, ToolResult, ToolUpdateSink,
};
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text, faux_tool_call};
use cyrup_provider::{Context, StreamEvent, StreamOptions, ToolDef};
use serde_json::{Value, json};

use super::support::model_ref;

type Hook = Arc<dyn Fn(&LoadoutView<'_>) -> Result<ToolLoadoutChanges, ToolError> + Send + Sync>;

/// A tool with a settable exposure, a ran-flag and an optional loadout hook.
struct Probe {
    name: String,
    description: String,
    params: Value,
    exposure: ToolExposure,
    ran: Arc<AtomicBool>,
    hook: Option<Hook>,
}

impl Probe {
    fn new(name: &str, exposure: ToolExposure) -> Self {
        Self {
            name: name.to_string(),
            description: format!("{name}: original"),
            params: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
            exposure,
            ran: Arc::new(AtomicBool::new(false)),
            hook: None,
        }
    }

    fn hiding(mut self, hidden: &[&str], descriptions: &[(&str, &str)]) -> Self {
        let hidden: Vec<String> = hidden.iter().map(|s| s.to_string()).collect();
        let descriptions: BTreeMap<String, String> = descriptions
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        self.hook = Some(Arc::new(move |_| {
            Ok(ToolLoadoutChanges {
                descriptions: descriptions.clone(),
                hidden_declarations: hidden.clone(),
            })
        }));
        self
    }

    fn ran(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.ran)
    }

    fn arc(self) -> Arc<dyn Tool> {
        Arc::new(self)
    }
}

#[async_trait::async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn exposure(&self) -> ToolExposure {
        self.exposure
    }
    fn prepare_loadout(&self, view: &LoadoutView<'_>) -> Result<ToolLoadoutChanges, ToolError> {
        match &self.hook {
            Some(f) => f(view),
            None => Ok(ToolLoadoutChanges::default()),
        }
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.ran.store(true, Ordering::SeqCst);
        Ok(ToolResult {
            content: vec![Content::text("ok")],
            ..Default::default()
        })
    }
}

/// Records the tool declarations of every request that reaches the provider.
/// The declarations of every request, in order.
type Seen = Arc<Mutex<Vec<Vec<ToolDef>>>>;

struct SchemaSpy {
    inner: Arc<dyn StreamFn>,
    seen: Seen,
}

impl StreamFn for SchemaSpy {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.seen.lock().unwrap().push(ctx.tools.clone());
        self.inner.stream(model, ctx, opts)
    }
}

fn spy(responses: Vec<cyrup_core::AssistantMessage>) -> (Arc<dyn StreamFn>, Seen) {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sf: Arc<dyn StreamFn> = Arc::new(SchemaSpy {
        inner: Arc::new(crate::ProviderStreamFn::new(faux)),
        seen: Arc::clone(&seen),
    });
    (sf, seen)
}

/// Run ONE turn with `loadout` and return the declarations the provider received.
async fn schema_for(loadout: ToolLoadout) -> Vec<ToolDef> {
    let (sf, seen) = spy(vec![faux_assistant_message(
        vec![faux_text("done")],
        StopReason::Stop,
    )]);
    let agent = Agent::builder(model_ref(), sf).loadout(loadout).build();
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;
    let reqs = seen.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1, "exactly one provider request: {reqs:?}");
    reqs.into_iter().next().unwrap()
}

fn declared_names(schema: &[ToolDef]) -> Vec<&str> {
    schema.iter().map(|d| d.name.as_str()).collect()
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn one_of_each() -> Vec<Arc<dyn Tool>> {
    vec![
        Probe::new("direct_t", ToolExposure::Direct).arc(),
        Probe::new("model_only_t", ToolExposure::ModelOnly).arc(),
        Probe::new("codemode_t", ToolExposure::Codemode).arc(),
        Probe::new("deferred_t", ToolExposure::Deferred).arc(),
        Probe::new("hidden_t", ToolExposure::Hidden).arc(),
    ]
}

/// CONTROL. A plain `direct` tool is in the schema with its own description and parameters. The
/// other tests in this file assert absence; this one proves the spy can see a tool at all.
#[tokio::test]
async fn a_direct_tool_reaches_the_schema_intact() {
    let registry = one_of_each();
    let schema = schema_for(ToolLoadout::resolve(&names(&["direct_t"]), &registry)).await;
    assert_eq!(declared_names(&schema), ["direct_t"]);
    assert_eq!(schema[0].description, "direct_t: original");
    assert_eq!(
        schema[0].parameters,
        json!({"type": "object", "properties": {"q": {"type": "string"}}})
    );
}

/// `model-only`: declared while active.
#[tokio::test]
async fn a_model_only_tool_reaches_the_schema_while_active() {
    let registry = one_of_each();
    let schema = schema_for(ToolLoadout::resolve(&names(&["model_only_t"]), &registry)).await;
    assert_eq!(declared_names(&schema), ["model_only_t"]);
}

/// `codemode`: not activated on registration, so a session whose active set is the registration
/// default never declares it. Naming it is an explicit activation, and pi then declares it like any
/// other tool (`_applyToolLoadout` drops only `hidden`).
#[tokio::test]
async fn a_codemode_tool_is_not_in_the_schema_unless_explicitly_activated() {
    let registry = one_of_each();
    let on_registration: Vec<String> = registry
        .iter()
        .filter(|t| t.exposure().activated_on_registration(t.default_active()))
        .map(|t| t.name().to_string())
        .collect();
    let schema = schema_for(ToolLoadout::resolve(&on_registration, &registry)).await;
    assert!(
        !declared_names(&schema).contains(&"codemode_t"),
        "codemode must not be declared by default: {:?}",
        declared_names(&schema)
    );
    let schema = schema_for(ToolLoadout::resolve(
        &names(&["direct_t", "codemode_t"]),
        &registry,
    ))
    .await;
    assert_eq!(declared_names(&schema), ["direct_t", "codemode_t"]);
}

/// `deferred`: same activation rule as `codemode`.
#[tokio::test]
async fn a_deferred_tool_is_not_in_the_schema_unless_explicitly_activated() {
    let registry = one_of_each();
    let on_registration: Vec<String> = registry
        .iter()
        .filter(|t| t.exposure().activated_on_registration(t.default_active()))
        .map(|t| t.name().to_string())
        .collect();
    let schema = schema_for(ToolLoadout::resolve(&on_registration, &registry)).await;
    assert!(!declared_names(&schema).contains(&"deferred_t"));
    let schema = schema_for(ToolLoadout::resolve(&names(&["deferred_t"]), &registry)).await;
    assert_eq!(declared_names(&schema), ["deferred_t"]);
}

/// `hidden`: unreachable. Naming it in the active set — the strongest request there is — still
/// leaves it out of the schema, and the loop will not run it either.
#[tokio::test]
async fn a_hidden_tool_never_reaches_the_schema_and_is_not_dispatched() {
    let hidden = Probe::new("hidden_t", ToolExposure::Hidden);
    let ran = hidden.ran();
    let registry: Vec<Arc<dyn Tool>> = vec![
        Probe::new("direct_t", ToolExposure::Direct).arc(),
        hidden.arc(),
    ];
    let loadout = ToolLoadout::resolve(&names(&["direct_t", "hidden_t"]), &registry);

    let (sf, seen) = spy(vec![
        faux_assistant_message(
            vec![faux_tool_call("hidden_t", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).loadout(loadout).build();
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    for (i, req) in seen.lock().unwrap().iter().enumerate() {
        assert_eq!(declared_names(req), ["direct_t"], "request {i}");
    }
    assert!(
        !ran.load(Ordering::SeqCst),
        "a hidden tool must not execute even when the model names it"
    );
}

/// `hiddenDeclarations`: the hook's tool stays active and CALLABLE by the loop, but its declaration
/// is absent from every request. This is the "not advertised, still dispatchable" case — pi's
/// `agent.state.tools` keeps the tool while `_hiddenDeclarations` projects its declaration out of
/// the request (`agent-session.ts:1721-1738` @v1.0.1).
#[tokio::test]
async fn a_hidden_declaration_is_left_out_of_the_schema_but_still_executes() {
    let bash = Probe::new("bash", ToolExposure::Direct);
    let bash_ran = bash.ran();
    let registry: Vec<Arc<dyn Tool>> = vec![
        Probe::new("codemode", ToolExposure::Direct)
            .hiding(&["bash"], &[])
            .arc(),
        bash.arc(),
        Probe::new("read", ToolExposure::Direct).arc(),
    ];
    let loadout = ToolLoadout::resolve(&names(&["codemode", "bash", "read"]), &registry);

    let (sf, seen) = spy(vec![
        faux_assistant_message(vec![faux_tool_call("bash", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).loadout(loadout).build();
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let reqs = seen.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    for (i, req) in reqs.iter().enumerate() {
        assert_eq!(declared_names(req), ["codemode", "read"], "request {i}");
    }
    assert!(
        bash_ran.load(Ordering::SeqCst),
        "a tool whose declaration is hidden is still executable by the loop"
    );
}

/// A `prepare_loadout` description reaches the provider; the schema (`parameters`) does not move.
#[tokio::test]
async fn a_loadout_description_reaches_the_schema() {
    let registry: Vec<Arc<dyn Tool>> = vec![
        Probe::new("codemode", ToolExposure::Direct)
            .hiding(&[], &[("read", "REWRITTEN by the loadout")])
            .arc(),
        Probe::new("read", ToolExposure::Direct).arc(),
    ];
    let schema = schema_for(ToolLoadout::resolve(
        &names(&["codemode", "read"]),
        &registry,
    ))
    .await;
    let read = schema.iter().find(|d| d.name == "read").unwrap();
    assert_eq!(read.description, "REWRITTEN by the loadout");
    assert_eq!(
        read.parameters,
        json!({"type": "object", "properties": {"q": {"type": "string"}}})
    );
}

/// The sticky-refresh seam carries the loadout, not a bare tool list: a `TurnUpdate` that pushes a
/// loadout with a hidden declaration applies it to the NEXT request and the declaration stays out.
#[tokio::test]
async fn a_pushed_loadout_keeps_its_hidden_declarations_on_the_next_request() {
    use crate::{HookError, Hooks, PostTurn, TurnUpdate};

    struct Push(Mutex<Option<ToolLoadout>>);
    #[async_trait::async_trait]
    impl Hooks for Push {
        async fn prepare_next_turn(
            &self,
            _ctx: PostTurn<'_>,
            _cancel: CancelToken,
        ) -> Result<Option<TurnUpdate>, HookError> {
            Ok(self.0.lock().unwrap().take().map(|l| TurnUpdate {
                tools: Some(l),
                ..TurnUpdate::default()
            }))
        }
    }

    let first = ToolLoadout::resolve(
        &names(&["direct_t"]),
        &[Probe::new("direct_t", ToolExposure::Direct).arc()],
    );
    let registry: Vec<Arc<dyn Tool>> = vec![
        Probe::new("codemode", ToolExposure::Direct)
            .hiding(&["bash"], &[])
            .arc(),
        Probe::new("bash", ToolExposure::Direct).arc(),
        Probe::new("direct_t", ToolExposure::Direct).arc(),
    ];
    let second = ToolLoadout::resolve(&names(&["codemode", "bash", "direct_t"]), &registry);

    let (sf, seen) = spy(vec![
        faux_assistant_message(
            vec![faux_tool_call("direct_t", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf)
        .loadout(first)
        .hooks(Arc::new(Push(Mutex::new(Some(second)))))
        .build();
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let reqs = seen.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    assert_eq!(declared_names(&reqs[0]), ["direct_t"]);
    assert_eq!(declared_names(&reqs[1]), ["codemode", "direct_t"]);
}
