//! EXT-078 — `Dispatcher::dispatch_boundary`, pi `emitBoundary` (`core/extensions/runner.ts:1029-1080`
//! @v1.1.0): `entries`/`continue` replace and chain across extensions, the preview is rebuilt after
//! EVERY extension (a faulting one included), and only the LAST preview decides whether the drafts
//! stand.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::{
    BoundaryState, EventKind, EventPatch, ExtError, ExtMode, ExtensionHost, HookOutcome,
    HostConfig, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_core::{CancelToken, ExtensionId};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<(Value, bool, Value)>>>;
type Answer = fn() -> Result<Option<EventPatch>, ()>;

struct Step {
    id: &'static str,
    seen: Seen,
    answer: Answer,
}

#[async_trait::async_trait]
impl NativeExtension for Step {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(self.id)
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::AgentBeforeSettle]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        let HostEvent::AgentBeforeSettle { boundary } = ev else {
            return HookOutcome::Noop;
        };
        self.seen.lock().unwrap().push((
            boundary.entries.clone(),
            boundary.continue_,
            boundary.context.clone(),
        ));
        match (self.answer)() {
            Ok(Some(patch)) => HookOutcome::Mutate(patch),
            Ok(None) => HookOutcome::Noop,
            Err(()) => panic!("a faulting handler"),
        }
    }
}

fn host() -> ExtensionHost {
    ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    })
}

/// The preview: the number of drafts, refusing any draft named "bad".
fn preview(entries: &Value) -> Result<Value, String> {
    let list = entries.as_array().cloned().unwrap_or_default();
    if list.iter().any(|e| e == "bad") {
        return Err("a bad draft".into());
    }
    Ok(json!({"drafts": list.len()}))
}

async fn run(steps: Vec<(&'static str, Answer)>) -> (crate::BoundaryDispatch, Seen, Vec<String>) {
    let host = host();
    let seen: Seen = Arc::default();
    let errors: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = Arc::clone(&errors);
    host.add_error_listener(Arc::new(move |e| {
        sink.lock()
            .unwrap()
            .push(format!("{}: {}", e.extension, e.error));
    }));
    for (id, answer) in steps {
        host.load_native(Arc::new(Step {
            id,
            seen: Arc::clone(&seen),
            answer,
        }))
        .await
        .unwrap();
    }
    let result = host
        .dispatcher()
        .dispatch_boundary(
            HostEvent::AgentBeforeSettle {
                boundary: BoundaryState::default(),
            },
            &preview,
            &CancelToken::new(),
        )
        .await;
    let errors = errors.lock().unwrap().clone();
    (result, seen, errors)
}

#[tokio::test]
async fn entries_and_continue_chain_and_the_preview_follows_every_extension() {
    let (result, seen, errors) = run(vec![
        ("one", || {
            Ok(Some(EventPatch::Boundary {
                entries: Some(json!(["a"])),
                continue_: Some(true),
            }))
        }),
        ("faults", || Err(())),
        ("two", || {
            Ok(Some(EventPatch::Boundary {
                entries: Some(json!(["a", "b"])),
                continue_: None,
            }))
        }),
    ])
    .await;
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0], (json!([]), false, json!({"drafts": 0})));
    assert_eq!(seen[1], (json!(["a"]), true, json!({"drafts": 1})));
    assert_eq!(
        seen[2],
        (json!(["a"]), true, json!({"drafts": 1})),
        "the fault changed nothing"
    );
    assert_eq!(
        result,
        crate::BoundaryDispatch {
            entries: json!(["a", "b"]),
            continue_: true,
            context: json!({"drafts": 2}),
            valid: true,
        }
    );
    assert_eq!(errors.len(), 1, "the fault is reported: {errors:?}");
}

#[tokio::test]
async fn only_the_last_preview_decides_whether_the_drafts_stand() {
    // A bad list a later extension replaces stands corrected…
    let (result, _, errors) = run(vec![
        ("breaks", || {
            Ok(Some(EventPatch::Boundary {
                entries: Some(json!(["bad"])),
                continue_: Some(true),
            }))
        }),
        ("fixes", || {
            Ok(Some(EventPatch::Boundary {
                entries: Some(json!(["good"])),
                continue_: None,
            }))
        }),
    ])
    .await;
    assert!(result.valid);
    assert_eq!(result.entries, json!(["good"]));
    assert!(result.continue_);
    assert_eq!(errors, ["breaks: Invalid boundary entries: a bad draft"]);

    // …and one left in place discards every draft and the continuation.
    let (result, seen, errors) = run(vec![
        ("breaks", || {
            Ok(Some(EventPatch::Boundary {
                entries: Some(json!(["ok", "bad"])),
                continue_: Some(true),
            }))
        }),
        ("reads", || Ok(None)),
    ])
    .await;
    assert_eq!(
        result,
        crate::BoundaryDispatch {
            entries: json!([]),
            continue_: false,
            context: json!({"drafts": 0}),
            valid: false,
        }
    );
    assert_eq!(
        seen.lock().unwrap()[1].2,
        json!({"drafts": 0}),
        "the last good preview"
    );
    assert_eq!(errors.len(), 2, "{errors:?}");
}
