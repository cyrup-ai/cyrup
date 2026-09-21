//! The `agent.view.*` filter/sort DSL, asserted against **herdr's own published example**.
//!
//! The types in [`crate::schema::agents`] are a tree of `#[serde(tag = "op")]`,
//! `#[serde(untagged)]` and `rename_all = "snake_case"` enums. Every one of those attributes is
//! invisible at the type level and load-bearing on the wire: drop `untagged` from
//! `AgentViewField` and the field serialises as `{"Builtin":"workspace_id"}` instead of
//! `"workspace_id"`; drop `rename_all` and `state_change_seq` becomes `"StateChangeSeq"`. herdr
//! answers either with `invalid_agent_view` and installs nothing
//! (`tmp/herdr/src/app/api/agent_view.rs:13`), which a caller that only logs the failure — as the
//! status bridge deliberately does — would never surface.
//!
//! So the whole DSL is round-tripped against the request `socket-api.mdx:429-456` prints, byte for
//! byte. **That is also what justifies the filter half existing at all**: cyrup's own projection
//! sends no filter (`cyrup_ext_subagents::herdr::view` says why — one view server-wide, and a
//! filter would hide the user's other agents), so without this the filter tree would be a shape
//! nothing had ever serialised, waiting to be wrong for its first caller.

use crate::schema::agents::{
    AgentViewBuiltinField, AgentViewBuiltinSortField, AgentViewClearParams, AgentViewContext,
    AgentViewField, AgentViewFilter, AgentViewSetParams, AgentViewSort, AgentViewSortField,
    AgentViewSortOrder, AgentViewValue,
};
use crate::schema::{Method, Request};

/// `socket-api.mdx:429-456`, reconstructed from the typed tree and compared as JSON.
#[test]
fn the_documented_agent_view_set_request_round_trips_byte_for_byte() {
    let params = AgentViewSetParams {
        source: "plugin:example.agent-views".to_owned(),
        label: Some("focus".to_owned()),
        filter: Some(AgentViewFilter::Any {
            filters: vec![
                AgentViewFilter::Eq {
                    field: AgentViewField::Builtin(AgentViewBuiltinField::WorkspaceId),
                    value: AgentViewValue::Context {
                        context: AgentViewContext::CurrentWorkspaceId,
                    },
                },
                AgentViewFilter::In {
                    field: AgentViewField::Builtin(AgentViewBuiltinField::Status),
                    values: vec![
                        AgentViewValue::String("blocked".to_owned()),
                        AgentViewValue::String("done".to_owned()),
                    ],
                },
            ],
        }),
        sort: vec![
            AgentViewSort::desc(AgentViewBuiltinSortField::Attention),
            AgentViewSort::desc(AgentViewBuiltinSortField::StateChangeSeq),
        ],
    };

    let request = Request {
        id: "view_set".to_owned(),
        method: Method::AgentViewSet(params),
    };
    let sent: serde_json::Value = serde_json::to_value(&request).expect("the request serialises");

    assert_eq!(
        sent,
        serde_json::json!({
            "id": "view_set",
            "method": "agent.view.set",
            "params": {
                "source": "plugin:example.agent-views",
                "label": "focus",
                "filter": {
                    "op": "any",
                    "filters": [
                        {
                            "op": "eq",
                            "field": "workspace_id",
                            "value": {"context": "current_workspace_id"}
                        },
                        {
                            "op": "in",
                            "field": "status",
                            "values": ["blocked", "done"]
                        }
                    ]
                },
                "sort": [
                    {"field": "attention", "order": "desc"},
                    {"field": "state_change_seq", "order": "desc"}
                ]
            }
        }),
        "the DSL must serialise as `socket-api.mdx:429-456` prints it"
    );
}

/// The remaining operators, fields and value shapes — every variant this crate declares reaches
/// the wire at least once, so none of them can carry a wrong attribute unnoticed.
#[test]
fn every_filter_operator_field_and_value_shape_has_herdrs_spelling() {
    let filter = AgentViewFilter::All {
        filters: vec![
            AgentViewFilter::Not {
                filter: Box::new(AgentViewFilter::Exists {
                    field: AgentViewField::Token {
                        token: "summary".to_owned(),
                    },
                }),
            },
            AgentViewFilter::Eq {
                field: AgentViewField::Builtin(AgentViewBuiltinField::Seen),
                value: AgentViewValue::Bool(true),
            },
            AgentViewFilter::Eq {
                field: AgentViewField::Builtin(AgentViewBuiltinField::StateChangeSeq),
                value: AgentViewValue::Number(7),
            },
            AgentViewFilter::Eq {
                field: AgentViewField::Builtin(AgentViewBuiltinField::TabId),
                value: AgentViewValue::Context {
                    context: AgentViewContext::CurrentTabId,
                },
            },
            AgentViewFilter::Eq {
                field: AgentViewField::Builtin(AgentViewBuiltinField::PaneId),
                value: AgentViewValue::String("w1:p1".to_owned()),
            },
            AgentViewFilter::Eq {
                field: AgentViewField::Builtin(AgentViewBuiltinField::Agent),
                value: AgentViewValue::String("cyrup".to_owned()),
            },
        ],
    };

    assert_eq!(
        serde_json::to_value(&filter).expect("serialises"),
        serde_json::json!({
            "op": "all",
            "filters": [
                {"op": "not", "filter": {"op": "exists", "field": {"token": "summary"}}},
                {"op": "eq", "field": "seen", "value": true},
                {"op": "eq", "field": "state_change_seq", "value": 7},
                {"op": "eq", "field": "tab_id", "value": {"context": "current_tab_id"}},
                {"op": "eq", "field": "pane_id", "value": "w1:p1"},
                {"op": "eq", "field": "agent", "value": "cyrup"},
            ]
        })
    );

    // The eight sort fields and both orders, plus the token form.
    let sorts = vec![
        AgentViewSort {
            field: AgentViewSortField::Builtin(AgentViewBuiltinSortField::WorkspaceOrder),
            order: AgentViewSortOrder::Asc,
        },
        AgentViewSort {
            field: AgentViewSortField::Builtin(AgentViewBuiltinSortField::TabOrder),
            order: AgentViewSortOrder::Asc,
        },
        AgentViewSort {
            field: AgentViewSortField::Builtin(AgentViewBuiltinSortField::PaneOrder),
            order: AgentViewSortOrder::Asc,
        },
        AgentViewSort::desc(AgentViewBuiltinSortField::Status),
        AgentViewSort::desc(AgentViewBuiltinSortField::Agent),
        AgentViewSort::desc(AgentViewBuiltinSortField::Seen),
        AgentViewSort {
            field: AgentViewSortField::Token {
                token: "summary".to_owned(),
            },
            order: AgentViewSortOrder::Asc,
        },
    ];
    assert_eq!(
        serde_json::to_value(&sorts).expect("serialises"),
        serde_json::json!([
            {"field": "workspace_order", "order": "asc"},
            {"field": "tab_order", "order": "asc"},
            {"field": "pane_order", "order": "asc"},
            {"field": "status", "order": "desc"},
            {"field": "agent", "order": "desc"},
            {"field": "seen", "order": "desc"},
            {"field": {"token": "summary"}, "order": "asc"},
        ])
    );
}

/// Both `agent.view.clear` shapes of `socket-api.mdx:490-491`: unconditional, and source-scoped.
///
/// `params` must be present on BOTH — herdr's `Method` is adjacently tagged and refuses a line
/// without it (`tmp/herdr/src/api/schema.rs:41`), which is why the unconditional form is `{}` and
/// not an omitted key.
#[test]
fn both_documented_clear_shapes_carry_params() {
    let bare = Request {
        id: "view_clear".to_owned(),
        method: Method::AgentViewClear(AgentViewClearParams::default()),
    };
    assert_eq!(
        serde_json::to_value(&bare).expect("serialises"),
        serde_json::json!({"id": "view_clear", "method": "agent.view.clear", "params": {}})
    );

    let owned = Request {
        id: "view_clear_owned".to_owned(),
        method: Method::AgentViewClear(AgentViewClearParams::owned_by(
            "plugin:example.agent-views",
        )),
    };
    assert_eq!(
        serde_json::to_value(&owned).expect("serialises"),
        serde_json::json!({
            "id": "view_clear_owned",
            "method": "agent.view.clear",
            "params": {"source": "plugin:example.agent-views"}
        })
    );
}

/// The answer BOTH verbs give — `type: "agent_view"` with `active`, `source` and optional `label`
/// (`socket-api.mdx:494-495`, `tmp/herdr/src/api/schema/response.rs:110-116`).
///
/// A clear whose `source` did not own the view comes back `active: true` naming the owner that
/// kept it (`tmp/herdr/src/app/api/agent_view.rs:63-71`), which is why the accessor hands back
/// the whole record rather than `()`.
#[test]
fn the_agent_view_answer_decodes_both_outcomes() {
    let installed: crate::schema::ResponseResult = serde_json::from_value(serde_json::json!({
        "type": "agent_view",
        "active": true,
        "source": "cyrup:subagents",
        "label": "cyrup fleet",
    }))
    .expect("decodes");
    let view = installed.agent_view("agent.view.set").expect("agent_view");
    assert!(view.active);
    assert_eq!(view.source.as_deref(), Some("cyrup:subagents"));
    assert_eq!(view.label.as_deref(), Some("cyrup fleet"));

    let cleared: crate::schema::ResponseResult =
        serde_json::from_value(serde_json::json!({"type": "agent_view", "active": false}))
            .expect("decodes with the optionals absent");
    let view = cleared.agent_view("agent.view.clear").expect("agent_view");
    assert!(!view.active);
    assert_eq!(view.source, None);

    // A mismatched clear: still `active`, and NOT this source.
    let kept: crate::schema::ResponseResult = serde_json::from_value(serde_json::json!({
        "type": "agent_view",
        "active": true,
        "source": "someone.else",
    }))
    .expect("decodes");
    let view = kept.agent_view("agent.view.clear").expect("agent_view");
    assert!(view.active);
    assert_eq!(view.source.as_deref(), Some("someone.else"));

    // Any other result type is a failure, never a default.
    let wrong: crate::schema::ResponseResult =
        serde_json::from_value(serde_json::json!({"type": "ok"})).expect("decodes");
    assert!(wrong.agent_view("agent.view.set").is_err());
}
