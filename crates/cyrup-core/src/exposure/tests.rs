#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use serde_json::{Value, json};

type Hook = Arc<dyn Fn(&LoadoutView<'_>) -> Result<ToolLoadoutChanges, ToolError> + Send + Sync>;

/// A tool whose exposure, namespace, description and loadout hook are settable.
struct Fixture {
    name: String,
    description: String,
    params: Value,
    exposure: ToolExposure,
    default_active: bool,
    namespace: Option<ToolNamespace>,
    hook: Option<Hook>,
    guidelines: Vec<&'static str>,
}

impl Fixture {
    fn new(name: &str, exposure: ToolExposure) -> Self {
        Self {
            name: name.to_string(),
            description: format!("{name} original"),
            params: json!({"type": "object", "properties": {"arg": {"type": "string"}}}),
            exposure,
            default_active: true,
            namespace: None,
            hook: None,
            guidelines: Vec::new(),
        }
    }

    fn guided(mut self, guidelines: &[&'static str]) -> Self {
        self.guidelines = guidelines.to_vec();
        self
    }

    fn hook(
        mut self,
        f: impl Fn(&LoadoutView<'_>) -> Result<ToolLoadoutChanges, ToolError> + Send + Sync + 'static,
    ) -> Self {
        self.hook = Some(Arc::new(f));
        self
    }

    fn arc(self) -> Arc<dyn Tool> {
        Arc::new(self)
    }
}

#[async_trait::async_trait]
impl Tool for Fixture {
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
    fn namespace(&self) -> Option<&ToolNamespace> {
        self.namespace.as_ref()
    }
    fn default_active(&self) -> bool {
        self.default_active
    }
    fn prompt_guidelines(&self) -> Vec<&str> {
        self.guidelines.clone()
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
        _args: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::default())
    }
}

/// What a hook observed: declared, callable and registered names, one exposure, one namespace.
type Observed = (
    Vec<String>,
    Vec<String>,
    Vec<String>,
    ToolExposure,
    Option<ToolNamespace>,
);

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn five() -> Vec<Arc<dyn Tool>> {
    vec![
        Fixture::new("direct_t", ToolExposure::Direct).arc(),
        Fixture::new("model_only_t", ToolExposure::ModelOnly).arc(),
        Fixture::new("codemode_t", ToolExposure::Codemode).arc(),
        Fixture::new("deferred_t", ToolExposure::Deferred).arc(),
        Fixture::new("hidden_t", ToolExposure::Hidden).arc(),
    ]
}

fn exec_names(l: &ToolLoadout) -> Vec<&str> {
    l.executable().iter().map(|t| t.name()).collect()
}

#[test]
fn the_wire_spelling_is_pis_and_round_trips() {
    for (e, s) in [
        (ToolExposure::Direct, "direct"),
        (ToolExposure::ModelOnly, "model-only"),
        (ToolExposure::Codemode, "codemode"),
        (ToolExposure::Deferred, "deferred"),
        (ToolExposure::Hidden, "hidden"),
    ] {
        assert_eq!(e.as_str(), s);
        assert_eq!(s.parse::<ToolExposure>().unwrap(), e);
        assert_eq!(serde_json::to_value(e).unwrap(), json!(s));
        assert_eq!(serde_json::from_value::<ToolExposure>(json!(s)).unwrap(), e);
    }
    assert_eq!(
        "Direct".parse::<ToolExposure>().unwrap_err(),
        UnknownExposure("Direct".into())
    );
}

#[test]
fn registration_activation_is_declarable_and_default_active() {
    // Pi `_isActivatedOnRegistration`: direct and model-only, unless `defaultActive: false`.
    for (e, on, off) in [
        (ToolExposure::Direct, true, false),
        (ToolExposure::ModelOnly, true, false),
        (ToolExposure::Codemode, false, false),
        (ToolExposure::Deferred, false, false),
        (ToolExposure::Hidden, false, false),
    ] {
        assert_eq!(
            e.activated_on_registration(true),
            on,
            "{e:?} default active"
        );
        assert_eq!(
            e.activated_on_registration(false),
            off,
            "{e:?} defaultActive false"
        );
    }
}

#[test]
fn callability_is_pis_predicate() {
    // `codemode || deferred || (direct && active)`.
    for (e, active, inactive) in [
        (ToolExposure::Direct, true, false),
        (ToolExposure::ModelOnly, false, false),
        (ToolExposure::Codemode, true, true),
        (ToolExposure::Deferred, true, true),
        (ToolExposure::Hidden, false, false),
    ] {
        assert_eq!(e.callable(true), active, "{e:?} active");
        assert_eq!(e.callable(false), inactive, "{e:?} inactive");
    }
}

#[test]
fn every_exposure_but_hidden_may_be_activated() {
    assert!(ToolExposure::Direct.can_be_activated());
    assert!(ToolExposure::ModelOnly.can_be_activated());
    assert!(ToolExposure::Codemode.can_be_activated());
    assert!(ToolExposure::Deferred.can_be_activated());
    assert!(!ToolExposure::Hidden.can_be_activated());
}

#[test]
fn resolve_keeps_request_order_dedupes_and_ignores_unknown_names() {
    let registry = five();
    let l = ToolLoadout::resolve(
        &names(&["model_only_t", "nope", "direct_t", "model_only_t"]),
        &registry,
    );
    assert_eq!(exec_names(&l), ["model_only_t", "direct_t"]);
}

#[test]
fn a_hidden_tool_is_never_executable_or_advertised_even_when_named() {
    let registry = five();
    let l = ToolLoadout::resolve(&names(&["hidden_t", "direct_t"]), &registry);
    assert_eq!(exec_names(&l), ["direct_t"]);
    assert_eq!(l.advertised().names(), ["direct_t"]);
}

#[test]
fn codemode_and_deferred_are_advertised_only_when_explicitly_activated() {
    let registry = five();
    // Not named: not executable, not advertised.
    let l = ToolLoadout::resolve(&names(&["direct_t"]), &registry);
    assert!(!l.advertised().contains("codemode_t"));
    assert!(!l.advertised().contains("deferred_t"));
    // Named: declared exactly as a direct tool is (pi `_applyToolLoadout` drops only `hidden`).
    let l = ToolLoadout::resolve(&names(&["direct_t", "codemode_t", "deferred_t"]), &registry);
    assert_eq!(
        l.advertised().names(),
        ["direct_t", "codemode_t", "deferred_t"]
    );
}

#[test]
fn a_hidden_declaration_stays_executable_but_leaves_the_request() {
    let registry: Vec<Arc<dyn Tool>> = vec![
        Fixture::new("codemode", ToolExposure::Direct)
            .hook(|_| {
                Ok(ToolLoadoutChanges {
                    hidden_declarations: vec!["bash".to_string()],
                    ..Default::default()
                })
            })
            .arc(),
        Fixture::new("bash", ToolExposure::Direct).arc(),
        Fixture::new("read", ToolExposure::Direct).arc(),
    ];
    let l = ToolLoadout::resolve(&names(&["codemode", "bash", "read"]), &registry);
    assert_eq!(exec_names(&l), ["codemode", "bash", "read"]);
    assert_eq!(l.advertised().names(), ["codemode", "read"]);
    assert_eq!(l.hidden_declarations().iter().collect::<Vec<_>>(), ["bash"]);
    let decls = l.advertised().declarations();
    assert_eq!(
        decls.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
        ["codemode", "read"]
    );
}

#[test]
fn a_hook_description_replaces_the_description_everywhere_the_loop_looks() {
    let registry: Vec<Arc<dyn Tool>> = vec![
        Fixture::new("codemode", ToolExposure::Direct)
            .hook(|_| {
                Ok(ToolLoadoutChanges {
                    descriptions: BTreeMap::from([("read".to_string(), "rewritten".to_string())]),
                    ..Default::default()
                })
            })
            .arc(),
        Fixture::new("read", ToolExposure::Direct).arc(),
    ];
    let l = ToolLoadout::resolve(&names(&["codemode", "read"]), &registry);
    let decl = l
        .advertised()
        .declarations()
        .into_iter()
        .find(|d| d.name == "read")
        .unwrap();
    assert_eq!(decl.description, "rewritten");
    assert_eq!(
        decl.parameters,
        registry[1].parameters().clone(),
        "schema untouched"
    );
    let exec = l.executable().iter().find(|t| t.name() == "read").unwrap();
    assert_eq!(exec.description(), "rewritten");
    // The registry's own tool is not mutated.
    assert_eq!(registry[1].description(), "read original");
}

#[test]
fn a_hook_sees_the_three_views_and_both_accessors() {
    let seen: Arc<std::sync::Mutex<Option<Observed>>> = Arc::new(std::sync::Mutex::new(None));
    let sink = Arc::clone(&seen);
    let mut with_ns = Fixture::new("ns_t", ToolExposure::Deferred);
    with_ns.namespace = Some(ToolNamespace {
        name: "mcp__docs".into(),
        description: Some("docs".into()),
        instructions: Some("long".into()),
    });
    let mut registry = five();
    registry.push(with_ns.arc());
    registry.push(
        Fixture::new("probe", ToolExposure::Direct)
            .hook(move |v| {
                let list = |ts: &[Arc<dyn Tool>]| ts.iter().map(|t| t.name().to_string()).collect();
                *sink.lock().unwrap() = Some((
                    list(v.declared()),
                    list(v.callable()),
                    list(v.registered()),
                    v.exposure("hidden_t"),
                    v.namespace("ns_t"),
                ));
                Ok(ToolLoadoutChanges::default())
            })
            .arc(),
    );
    let _ = ToolLoadout::resolve(&names(&["direct_t", "model_only_t", "probe"]), &registry);
    let (declared, callable, registered, hidden_exposure, ns) =
        seen.lock().unwrap().take().unwrap();
    assert_eq!(declared, ["direct_t", "model_only_t", "probe"]);
    // active direct + every codemode/deferred; model-only and hidden never.
    assert_eq!(
        callable,
        ["direct_t", "codemode_t", "deferred_t", "ns_t", "probe"]
    );
    assert_eq!(registered.len(), 7);
    assert_eq!(hidden_exposure, ToolExposure::Hidden);
    assert_eq!(ns.unwrap().name, "mcp__docs");
}

#[test]
fn a_failing_hook_is_reported_and_changes_nothing() {
    let registry: Vec<Arc<dyn Tool>> = vec![
        Fixture::new("bad", ToolExposure::Direct)
            .hook(|_| Err(ToolError::new("boom")))
            .arc(),
        Fixture::new("read", ToolExposure::Direct).arc(),
    ];
    let l = ToolLoadout::resolve(&names(&["bad", "read"]), &registry);
    assert_eq!(l.advertised().names(), ["bad", "read"]);
    assert_eq!(
        l.hook_failures(),
        [LoadoutHookFailure {
            tool: "bad".into(),
            message: "boom".into()
        }]
    );
}

#[test]
fn an_inactive_tools_hook_is_not_consulted() {
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&called);
    let registry: Vec<Arc<dyn Tool>> = vec![
        Fixture::new("dormant", ToolExposure::Direct)
            .hook(move |_| {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(ToolLoadoutChanges::default())
            })
            .arc(),
        Fixture::new("read", ToolExposure::Direct).arc(),
    ];
    let _ = ToolLoadout::resolve(&names(&["read"]), &registry);
    assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn from_tools_is_resolve_over_exactly_those_tools() {
    let l = ToolLoadout::from_tools(five());
    assert_eq!(
        exec_names(&l),
        ["direct_t", "model_only_t", "codemode_t", "deferred_t"],
        "the hidden one is dropped even here"
    );
}

#[test]
fn a_later_registry_entry_replaces_an_earlier_one_of_the_same_name() {
    let mut first = Fixture::new("t", ToolExposure::Direct);
    first.description = "first".into();
    let mut second = Fixture::new("t", ToolExposure::Direct);
    second.description = "second".into();
    let registry = vec![first.arc(), second.arc()];
    let l = ToolLoadout::resolve(&names(&["t"]), &registry);
    assert_eq!(l.advertised().declarations()[0].description, "second");
}

#[test]
fn the_default_trait_surface_is_direct_active_and_ungrouped() {
    struct Bare(Value);
    #[async_trait::async_trait]
    impl Tool for Bare {
        fn name(&self) -> &str {
            "bare"
        }
        fn parameters(&self) -> &Value {
            &self.0
        }
        async fn execute(
            &self,
            _: ToolCallId,
            _: Value,
            _: CancelToken,
            _: ToolUpdateSink,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::default())
        }
    }
    let t = Bare(json!({}));
    assert_eq!(t.exposure(), ToolExposure::Direct);
    assert!(t.namespace().is_none());
    assert!(t.default_active());
    let reg: Vec<Arc<dyn Tool>> = vec![Arc::new(Bare(json!({})))];
    let view_names = names(&["bare"]);
    let l = ToolLoadout::resolve(&view_names, &reg);
    assert_eq!(l.advertised().names(), ["bare"]);
}

/// CODE-020 (pi `ToolLoadout.getPromptGuidelines`, `extensions/types.ts:549` @v1.0.4): a hook reads
/// a registered tool's guidelines, trimmed, without the empty ones and without a repeat; a name the
/// loadout does not know has none.
#[test]
fn a_loadout_hook_reads_the_prompt_guidelines_of_a_registered_tool() {
    let seen: Arc<std::sync::Mutex<Vec<(String, Vec<String>)>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let reader = Fixture::new("reader", ToolExposure::Direct)
        .hook(move |view| {
            let mut sink = sink.lock().unwrap();
            for name in ["guided", "bare", "unknown"] {
                sink.push((name.to_owned(), view.prompt_guidelines(name)));
            }
            Ok(ToolLoadoutChanges::default())
        })
        .arc();
    let guided = Fixture::new("guided", ToolExposure::Direct)
        .guided(&["  Use it.  ", "", "   ", "Use it.", "Then stop."])
        .arc();
    let bare = Fixture::new("bare", ToolExposure::Direct).arc();
    let registry = vec![reader, guided, bare];
    let _ = ToolLoadout::resolve(
        &["reader".to_owned(), "guided".to_owned(), "bare".to_owned()],
        &registry,
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            (
                "guided".to_owned(),
                vec!["Use it.".to_owned(), "Then stop.".to_owned()]
            ),
            ("bare".to_owned(), Vec::new()),
            ("unknown".to_owned(), Vec::new()),
        ]
    );
}
