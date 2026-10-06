//! SUBA-152 — the crate's port of pi `shared/disabled-features.ts` (@v0.75.0, annotated tag
//! `06f8452c` -> commit `ad56bf92`): the 15 feature groups an operator removes from the
//! parent-facing `subagent` tool through `config.disabledFeatures`, and the four behaviours that
//! turn that list into a validated config value, a resolved surface, a per-request refusal and an
//! operator-facing notice.
//!
//! Upstream's own framing (`disabled-features.ts:1-7`, verbatim): *"Opt-in feature groups an
//! operator can remove from the parent-facing `subagent` tool. Groups own parameters and actions
//! that enabled features do not need, so hiding them cannot remove a field that enabled behavior
//! still needs. `preflight` is the one shared parameter: it is script-only, so `workflow-scripts`
//! removes it too. Per-call options disable only the per-call override; configured defaults keep
//! applying."*
//!
//! # Why this lives at the crate root
//!
//! Upstream's `src/shared/*.ts` files map to this crate's top-level modules
//! ([`crate::formatters`] <- `shared/formatters.ts`, [`crate::paths`] <- `shared/utils.ts`,
//! [`crate::time`], [`crate::jsonl`]). `disabled-features.ts` is a `shared/` file with three
//! independent consumer trees upstream — the config validator (`extension/config.ts:185`), the
//! tool surface (`extension/index.ts:707`, `extension/tool-description.ts:61`,
//! `extension/schemas.ts:6`) and the executor (`runs/foreground/subagent-executor.ts:5286`,
//! `extension/rpc.ts:535`, `extension/fanout-child.ts:216`) — so it belongs beside them rather
//! than inside any one of them.
//!
//! # Two orderings in here are OBSERVABLE, not incidental
//!
//! **1. [`SUBAGENT_FEATURES`] is in upstream's declaration order.** `Object.keys(...)` follows it,
//! and [`validate_disabled_features`]'s unknown-entry refusal interpolates that join — so the
//! sentence an operator reads lists the 15 names in pi's own order.
//!
//! **2. [`DisabledSurfaceMap`] is insertion-ordered because
//! [`disabled_feature_use_error`] returns the FIRST disabled param it finds.** Upstream iterates a
//! JS `Map` in insertion order, which follows the `Set` iteration order of
//! [`DisabledFeatureSurface::features`], which is **the operator's own config array order**, with
//! the synthetic `schedules` appended last. A request carrying two disabled params therefore gets
//! a refusal naming whichever group the operator listed first. A `HashMap` would destroy that and
//! a `BTreeMap` would silently reorder it to alphabetical, so neither is usable here;
//! `the_first_disabled_param_follows_the_operators_config_order` pins the behaviour by disabling
//! the same two groups in both orders and asserting the two different sentences.
//!
//! # What this port can and cannot gate here
//!
//! Validation accepts **all 15 upstream names**, unconditionally and by design: an operator moving
//! a `config.json` from pi to cyrup must never be told a legitimate upstream feature name is
//! invalid. Gating then acts on whatever a request actually carries, so the table below is ported
//! VERBATIM rather than trimmed to cyrup's advertised schema.
//!
//! Three groups — `preflight`, `gates` and `extension-bindings` — name **no surface this port
//! advertises**: `extension/tool/schema.rs` has no `preflight`, `gate` or `extensionBindings`
//! property, and all three groups have an empty `actions` list upstream. Disabling one of them is
//! therefore a **no-op for a caller that follows cyrup's advertised schema**, which is the correct
//! outcome and NOT a bug to "fix" into a refusal: the name has to stay accepted (see above), and
//! inventing a cyrup `preflight`/`gate`/`extensionBindings` parameter so the gate had something to
//! bite would be a worse answer than a no-op. The same holds, per-param, for the two
//! `workflow-scripts` members cyrup does not advertise (`globalConcurrencyLimit`,
//! `maxSubagentSpawnsPerRun`) and for `preflight` again — each group still gates the members cyrup
//! does advertise.
//!
//! SUBA-151 CORRECTION — this paragraph previously also listed `missionStatus`, `missionId`,
//! `runMode`, `runStatus`, `summary`, `laneId`, `supersession`, `planId` and `args` as
//! unadvertised. **All nine ARE advertised** (`extension/tool/schema.rs`'s
//! `props.insert("missionStatus", …)` and its eight siblings, `args` among them), so those three
//! groups gate every member they name except `preflight`. The claim is no longer prose:
//! `exactly_five_group_params_have_no_advertised_surface_to_drop` intersects all 15 groups'
//! `params` with the advertised schema and pins BOTH halves, so a rename on either side fails
//! rather than silently un-gating a group. Every group's ACTIONS are fully
//! present here: all 59 verbs of `extension::tool::text::SUBAGENT_ACTIONS` cover every action the
//! 15 groups and the `schedules` surface name, which
//! `every_group_action_is_a_verb_this_port_dispatches` asserts.

use serde_json::{Map, Value};

/// One group's surface — pi's `{ actions, params }` record value (`disabled-features.ts:8-39`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubagentFeatureEntry {
    /// The group this entry describes.
    pub feature: SubagentFeature,
    /// The `action` values the group owns. Empty for a params-only group.
    pub actions: &'static [&'static str],
    /// The tool parameters the group owns. Never empty upstream — which is why
    /// [`disabled_feature_notice`] emits `options …` unconditionally and `actions …` only when
    /// non-empty, exactly as upstream's `parts` array is built (`:117-119`).
    pub params: &'static [&'static str],
}

/// pi `SubagentFeature = keyof typeof SUBAGENT_FEATURES` (`disabled-features.ts:41`) — the 15
/// group names `config.disabledFeatures` accepts, as a closed enum rather than a string.
///
/// A closed enum, not `Vec<String>`: the config field is typed on this, so a name that reached
/// [`crate::registration::SubagentExtensionConfig::disabled_features`] was already checked once
/// by serde and once by [`validate_disabled_features`], and [`SUBAGENT_FEATURES`] cannot gain a
/// group without the compiler demanding an arm for it in [`Self::as_str`] and
/// [`Self::surface`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SubagentFeature {
    /// `agent-management` — the create/update/delete/refine verbs and the `config` param.
    AgentManagement,
    /// `watchdog` — the four `watchdog.*` verbs.
    Watchdog,
    /// `panes` — the `inspector.*` and `project.*` pane verbs.
    Panes,
    /// `missions` — the seven `mission.*` verbs.
    Missions,
    /// `lane-management` — the lane/worktree convergence verbs.
    LaneManagement,
    /// `spawn-budget-grants` — `grant-spawn-budget`.
    SpawnBudgetGrants,
    /// `preflight` — the script-only `preflight` param. **Shares that param with
    /// [`Self::WorkflowScripts`]**; see [`resolve_disabled_feature_surface`].
    Preflight,
    /// `lane-metadata` — the `lane` param.
    LaneMetadata,
    /// `gates` — the `gate` param.
    Gates,
    /// `usage-budgets` — the `usageBudget` param.
    UsageBudgets,
    /// `tool-budgets` — the `toolBudget` param.
    ToolBudgets,
    /// `control-overrides` — the `control` param.
    ControlOverrides,
    /// `extension-bindings` — the `extensionBindings` param.
    ExtensionBindings,
    /// `external-machines` — the `machine` param.
    ExternalMachines,
    /// `workflow-scripts` — `validate`, the script params, and the `workflowScript` carrier.
    WorkflowScripts,
}

/// pi `SUBAGENT_FEATURES` (`disabled-features.ts:8-39`), ported VERBATIM: every group, every
/// action and every param name, **in upstream's declaration order**.
///
/// The order is load-bearing twice over (see this module's doc): `Object.keys(...)` feeds
/// [`validate_disabled_features`]'s unknown-entry sentence, and the table is the only place the
/// member lists exist, so [`SubagentFeature::surface`] cannot disagree with it.
pub const SUBAGENT_FEATURES: [SubagentFeatureEntry; 15] = [
    SubagentFeatureEntry {
        feature: SubagentFeature::AgentManagement,
        actions: &[
            "create",
            "update",
            "delete",
            "eject",
            "disable",
            "enable",
            "reset",
            "refine",
            "refine.show",
            "refine.rollback",
        ],
        params: &["config"],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::Watchdog,
        actions: &[
            "watchdog.status",
            "watchdog.check",
            "watchdog.configure",
            "watchdog.recommend-model",
        ],
        params: &["scope", "target", "thinking"],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::Panes,
        actions: &[
            "inspector.open",
            "inspector.command",
            "inspector.status",
            "inspector.close",
            "project.open",
            "project.status",
            "project.close",
        ],
        params: &["focus"],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::Missions,
        actions: &[
            "mission.create",
            "mission.list",
            "mission.show",
            "mission.update",
            "mission.resolve-decision",
            "mission.attach-run",
            "mission.close",
        ],
        params: &[
            "mission",
            "missionUpdate",
            "missionStatus",
            "missionScope",
            "missionId",
            "runMode",
            "runStatus",
            "summary",
        ],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::LaneManagement,
        actions: &[
            "lane.status",
            "lane.recordMerge",
            "lane.recordSupersession",
            "worktree.discard",
            "worktree.cleanup",
        ],
        params: &[
            "handoffPath",
            "laneId",
            "merge",
            "supersession",
            "repo",
            "planId",
        ],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::SpawnBudgetGrants,
        actions: &["grant-spawn-budget"],
        params: &["additional"],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::Preflight,
        actions: &[],
        params: &["preflight"],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::LaneMetadata,
        actions: &[],
        params: &["lane"],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::Gates,
        actions: &[],
        params: &["gate"],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::UsageBudgets,
        actions: &[],
        params: &["usageBudget"],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::ToolBudgets,
        actions: &[],
        params: &["toolBudget"],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::ControlOverrides,
        actions: &[],
        params: &["control"],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::ExtensionBindings,
        actions: &[],
        params: &["extensionBindings"],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::ExternalMachines,
        actions: &[],
        params: &["machine"],
    },
    SubagentFeatureEntry {
        feature: SubagentFeature::WorkflowScripts,
        actions: &["validate"],
        params: &[
            "workflow",
            "args",
            "preflight",
            "globalConcurrencyLimit",
            "maxSubagentSpawnsPerRun",
        ],
    },
];

/// pi's `SCHEDULE_SURFACE` (`disabled-features.ts:46-49`) — the synthetic sixteenth group's
/// actions and params.
///
/// NOT part of [`SUBAGENT_FEATURES`], deliberately: upstream's own comment at `:43` is
/// *"Schedules are turned off by `scheduledRuns.enabled: false`, not by `disabledFeatures`"*, and
/// [`validate_disabled_features`] refuses the literal `"schedules"` with a message pointing at
/// that key instead.
const SCHEDULE_SURFACE: SubagentFeatureEntry = SubagentFeatureEntry {
    // `feature` is unused for the schedule surface — [`SubagentSurfaceFeature::surface`] reads
    // only `actions`/`params` out of it, and the surface feature it belongs to is
    // [`SubagentSurfaceFeature::Schedules`], which has no [`SubagentFeature`]. The placeholder
    // keeps ONE entry type for both the real groups and this one, so
    // [`disabled_feature_notice`] needs no second shape.
    feature: SubagentFeature::WorkflowScripts,
    actions: &[
        "schedule.create",
        "schedule.list",
        "schedule.show",
        "schedule.history",
        "schedule.pause",
        "schedule.resume",
        "schedule.run",
        "schedule.run-due",
        "schedule.delete",
    ],
    params: &[
        "name",
        "at",
        "every",
        "sessionOnly",
        "quiet",
        "on",
        "timezone",
        "overlap",
        "catchUp",
    ],
};

impl SubagentFeature {
    /// The `config.disabledFeatures` wire name, or `None` for anything else — the crate's
    /// established `from_wire` shape (compare
    /// [`crate::extension::tool::lane_actions`]'s `LaneAction::from_wire`).
    ///
    /// The literal `"schedules"` is deliberately NOT accepted here: it is not a
    /// [`SubagentFeature`] upstream either, and [`validate_disabled_features`] gives it its own
    /// refusal naming `config.scheduledRuns.enabled`.
    #[must_use]
    pub fn from_wire(name: &str) -> Option<Self> {
        match name {
            "agent-management" => Some(Self::AgentManagement),
            "watchdog" => Some(Self::Watchdog),
            "panes" => Some(Self::Panes),
            "missions" => Some(Self::Missions),
            "lane-management" => Some(Self::LaneManagement),
            "spawn-budget-grants" => Some(Self::SpawnBudgetGrants),
            "preflight" => Some(Self::Preflight),
            "lane-metadata" => Some(Self::LaneMetadata),
            "gates" => Some(Self::Gates),
            "usage-budgets" => Some(Self::UsageBudgets),
            "tool-budgets" => Some(Self::ToolBudgets),
            "control-overrides" => Some(Self::ControlOverrides),
            "extension-bindings" => Some(Self::ExtensionBindings),
            "external-machines" => Some(Self::ExternalMachines),
            "workflow-scripts" => Some(Self::WorkflowScripts),
            _ => None,
        }
    }

    /// Round-trips [`Self::from_wire`]. Every refusal and notice that names a group interpolates
    /// this, so the sentence an operator reads carries the name they wrote in `config.json`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentManagement => "agent-management",
            Self::Watchdog => "watchdog",
            Self::Panes => "panes",
            Self::Missions => "missions",
            Self::LaneManagement => "lane-management",
            Self::SpawnBudgetGrants => "spawn-budget-grants",
            Self::Preflight => "preflight",
            Self::LaneMetadata => "lane-metadata",
            Self::Gates => "gates",
            Self::UsageBudgets => "usage-budgets",
            Self::ToolBudgets => "tool-budgets",
            Self::ControlOverrides => "control-overrides",
            Self::ExtensionBindings => "extension-bindings",
            Self::ExternalMachines => "external-machines",
            Self::WorkflowScripts => "workflow-scripts",
        }
    }

    /// This group's row of [`SUBAGENT_FEATURES`] — the table is the single source of the member
    /// lists, so a lookup can never disagree with what the const declares.
    #[must_use]
    pub fn surface(self) -> SubagentFeatureEntry {
        // A linear scan over 15 `Copy` entries, resolved once per session: the alternative (a
        // second `match` listing every group's members again) is exactly the drift this
        // indirection exists to prevent. The fallback is unreachable — every variant has a row,
        // which `every_feature_has_exactly_one_table_row` asserts — and is an EMPTY surface
        // rather than a panic because this crate denies `clippy::panic` outside tests. An empty
        // surface gates nothing, which is the fail-open direction; that is why the test asserts
        // the row exists rather than trusting this arm never runs.
        SUBAGENT_FEATURES
            .into_iter()
            .find(|entry| entry.feature == self)
            .unwrap_or(SubagentFeatureEntry {
                feature: self,
                actions: &[],
                params: &[],
            })
    }
}

/// `config.disabledFeatures` is deserialized through [`SubagentFeature::from_wire`] rather than
/// serde's own `rename`/`rename_all` derive, so the wire names have exactly ONE definition. A
/// `rename_all = "kebab-case"` derive would reproduce all 15 names today and silently stop
/// matching the day a group arrives upstream whose name is not plain kebab-case.
impl<'de> serde::Deserialize<'de> for SubagentFeature {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = <std::borrow::Cow<'de, str> as serde::Deserialize>::deserialize(deserializer)?;
        Self::from_wire(&name).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "config.disabledFeatures entry {} is not one of: {}",
                Value::String(name.to_string()),
                feature_name_list(),
            ))
        })
    }
}

impl serde::Serialize for SubagentFeature {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// pi `SubagentSurfaceFeature = SubagentFeature | "schedules"` (`disabled-features.ts:44`): a
/// [`SubagentFeature`] or the synthetic group `scheduledRuns.enabled: false` folds in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SubagentSurfaceFeature {
    /// One of the 15 groups `config.disabledFeatures` names.
    Feature(SubagentFeature),
    /// The synthetic `schedules` group — reachable ONLY from `scheduledRuns.enabled: false`,
    /// never from `config.disabledFeatures`, which refuses the literal name.
    Schedules,
}

impl SubagentSurfaceFeature {
    /// pi `featureSurface(...).disabledBy` (`disabled-features.ts:77-80`): **the setting that
    /// disabled this group**, as it appears inside every refusal and notice line.
    ///
    /// The two forms are not interchangeable — `schedules` names a different config key — and this
    /// is the only place either is built, so [`disabled_feature_use_error`] and
    /// [`disabled_feature_notice`] cannot word them differently.
    #[must_use]
    pub fn disabled_by(self) -> String {
        match self {
            Self::Schedules => "scheduledRuns.enabled=false".to_string(),
            Self::Feature(feature) => format!(r#"disabledFeatures "{}""#, feature.as_str()),
        }
    }

    /// pi `featureSurface(...)`'s `{ actions, params }` half (`:77-80`).
    #[must_use]
    pub fn surface(self) -> SubagentFeatureEntry {
        match self {
            Self::Schedules => SCHEDULE_SURFACE,
            Self::Feature(feature) => feature.surface(),
        }
    }
}

/// `Object.keys(SUBAGENT_FEATURES).join(", ")` (`disabled-features.ts:58`) — the 15 names in
/// upstream's declaration order, as [`validate_disabled_features`]'s unknown-entry sentence and
/// serde's own rejection both interpolate them.
fn feature_name_list() -> String {
    SUBAGENT_FEATURES
        .iter()
        .map(|entry| entry.feature.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// An **insertion-ordered** `name -> disabling feature` map: this port's stand-in for the
/// `ReadonlyMap<string, string>` pair of pi's `DisabledFeatureSurface`
/// (`disabled-features.ts:69-73`).
///
/// # Why not a `HashMap` or a `BTreeMap`
///
/// [`disabled_feature_use_error`] returns the FIRST disabled param it finds while iterating this
/// map, and upstream's iteration order is JS `Map` insertion order — i.e. the operator's own
/// `config.disabledFeatures` array order, with `schedules` last. A `HashMap` randomizes that and a
/// `BTreeMap` reorders it to alphabetical; either one changes WHICH refusal a caller sees for a
/// request carrying two disabled params, with nothing failing to say so. A `Vec` of pairs makes
/// the order part of the type, and these maps hold at most ~50 entries resolved once per session,
/// so the linear [`Self::get`] is cheaper than hashing.
///
/// [`Self::set`] reproduces JS `Map.set` exactly, including the part that matters for the shared
/// `preflight` param: re-setting an EXISTING key updates its value **in place** and does not move
/// it to the end.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisabledSurfaceMap {
    entries: Vec<(&'static str, SubagentSurfaceFeature)>,
}

impl DisabledSurfaceMap {
    /// JS `Map.prototype.set`: update the value of an existing key without moving it, else append.
    fn set(&mut self, name: &'static str, feature: SubagentSurfaceFeature) {
        match self.entries.iter_mut().find(|(key, _)| *key == name) {
            Some(slot) => slot.1 = feature,
            None => self.entries.push((name, feature)),
        }
    }

    /// JS `Map.prototype.get`: the feature that disabled `name`, or `None`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<SubagentSurfaceFeature> {
        self.entries
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, feature)| *feature)
    }

    /// The entries in insertion order — the order [`disabled_feature_use_error`] resolves ties in.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, SubagentSurfaceFeature)> + '_ {
        self.entries.iter().copied()
    }

    /// How many names this map disables.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is disabled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// pi `DisabledFeatureSurface` (`disabled-features.ts:69-73`): *"Disabled features, and each
/// disabled parameter and action mapped to the setting that disabled them."*
///
/// `features` stands in for pi's `ReadonlySet`, and is a `Vec` for the same reason the maps are
/// (see [`DisabledSurfaceMap`]): [`disabled_feature_notice`] renders one line per feature in
/// `Set` iteration order, which is the operator's config order with `schedules` appended last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisabledFeatureSurface {
    features: Vec<SubagentSurfaceFeature>,
    params: DisabledSurfaceMap,
    actions: DisabledSurfaceMap,
}

impl DisabledFeatureSurface {
    /// The disabled groups, in the operator's config order with `schedules` last.
    #[must_use]
    pub fn features(&self) -> &[SubagentSurfaceFeature] {
        &self.features
    }

    /// Each disabled parameter, mapped to the group that disabled it.
    #[must_use]
    pub fn params(&self) -> &DisabledSurfaceMap {
        &self.params
    }

    /// Each disabled action, mapped to the group that disabled it.
    #[must_use]
    pub fn actions(&self) -> &DisabledSurfaceMap {
        &self.actions
    }

    /// pi `surface.features.has(feature)` (`:109`).
    #[must_use]
    pub fn contains(&self, feature: SubagentSurfaceFeature) -> bool {
        self.features.contains(&feature)
    }

    /// True when the config disabled nothing — pi's `surface.features.size === 0` (`:116`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }
}

/// pi `validateDisabledFeatures` (`disabled-features.ts:51-66`), run on the RAW `config.json`
/// object for the same reason every other raw validator in
/// [`crate::registration::SubagentExtensionConfig::validate_raw_config`] is: serde alone reports a
/// bad value with its own generic text, where upstream names the key and the offending entry.
///
/// Four distinct refusals, each carrying upstream's exact sentence:
///
/// 1. not an array (`:54`);
/// 2. the literal `"schedules"` (`:57`) — which gets its OWN message pointing at
///    `config.scheduledRuns.enabled`, deliberately NOT the generic unknown-entry one, because
///    `schedules` IS a real surface and the operator is reaching for the wrong key rather than
///    inventing a name;
/// 3. an unknown entry (`:58-59`), whose message embeds the full comma-joined feature list and
///    the offending entry JSON-stringified;
/// 4. a duplicate entry (`:60`).
///
/// # The absent-versus-null distinction
///
/// Upstream's first line is `if (value === undefined) return;` — only an ABSENT key is accepted.
/// An explicit `null` is not `undefined`, so `Array.isArray(null)` is false and upstream throws
/// the not-an-array message. `raw.get("disabledFeatures")` draws the same line: `None` for absent,
/// `Some(Value::Null)` for a declared null.
///
/// # Errors
///
/// Upstream's own four sentences, verbatim.
pub fn validate_disabled_features(raw: &Value) -> Result<(), String> {
    let Some(value) = raw.get("disabledFeatures") else {
        return Ok(());
    };
    let Some(entries) = value.as_array() else {
        return Err("config.disabledFeatures must be an array of feature names".to_string());
    };
    let mut seen: Vec<&str> = Vec::new();
    for entry in entries {
        // Upstream tests the literal `"schedules"` BEFORE the type/membership test, so a config
        // that reaches for the wrong key is told which key to use rather than being handed the
        // 15-name list that deliberately omits `schedules`.
        if entry.as_str() == Some("schedules") {
            return Err(
                r#"config.disabledFeatures does not accept "schedules"; set config.scheduledRuns.enabled to false instead"#
                    .to_string(),
            );
        }
        let Some(name) = entry
            .as_str()
            .filter(|name| SubagentFeature::from_wire(name).is_some())
        else {
            // `JSON.stringify(entry)` (`:58`). `serde_json`'s compact form agrees with it for
            // every JSON value — including the quoting of a string entry, which is why a typo'd
            // name is reported as `"agentmanagement"` with the quotes. JS's one unrepresentable
            // case, `undefined`, cannot occur: it has no JSON spelling, so no `config.json` can
            // produce it.
            return Err(format!(
                "config.disabledFeatures entry {entry} is not one of: {}",
                feature_name_list()
            ));
        };
        if seen.contains(&name) {
            return Err(format!(
                r#"config.disabledFeatures lists "{name}" more than once"#
            ));
        }
        seen.push(name);
    }
    Ok(())
}

/// pi `resolveDisabledFeatureSurface` (`disabled-features.ts:82-95`): fold the operator's list and
/// the scheduled-runs switch into the `{features, params, actions}` triple everything else reads.
///
/// `scheduled_runs_enabled` must come from
/// [`crate::registration::SubagentExtensionConfig::scheduled_runs_enabled`], which owns the
/// tri-state polarity (absent and `true` both ENABLE; only the literal `false` disables). It is
/// taken as an already-resolved `bool` rather than re-derived here precisely so this function
/// cannot invert it — [`crate::registration::SubagentExtensionConfig::disabled_feature_surface`]
/// is its one production call site.
///
/// # The shared `preflight` param
///
/// `preflight` belongs to BOTH the `preflight` group and `workflow-scripts`. Upstream attributes
/// it to `workflow-scripts` whenever that feature is disabled, **whatever order the config lists
/// the features in** — its comment at `:88-89` says exactly that, and `:90`'s condition
/// (`feature === "workflow-scripts" || !params.has(param)`) is what encodes it: `workflow-scripts`
/// overwrites an existing attribution, every other group only fills a gap. A port that dropped
/// the first half of that disjunction would be wrong ONLY when both groups are disabled and
/// `preflight` is listed first, which is why
/// `workflow_scripts_owns_the_shared_preflight_param_in_either_config_order` tries both orders.
#[must_use]
pub fn resolve_disabled_feature_surface(
    disabled_features: &[SubagentFeature],
    scheduled_runs_enabled: bool,
) -> DisabledFeatureSurface {
    // `new Set<SubagentSurfaceFeature>(config.disabledFeatures)` (`:83`) — deduplicating while
    // keeping each name's FIRST position. `validate_disabled_features` already refuses a
    // duplicate, but this function is also reachable with an unvalidated list (pi's is too), and
    // a `Set` is what upstream builds.
    let mut features: Vec<SubagentSurfaceFeature> = Vec::new();
    for feature in disabled_features {
        let surface_feature = SubagentSurfaceFeature::Feature(*feature);
        if !features.contains(&surface_feature) {
            features.push(surface_feature);
        }
    }
    // `if (config.scheduledRuns?.enabled === false) features.add("schedules")` (`:84`) — ALWAYS
    // appended after the operator's own groups, and unreachable from `config.disabledFeatures`
    // (which refuses the name), so `schedules` is last in every surface that has it.
    if !scheduled_runs_enabled {
        features.push(SubagentSurfaceFeature::Schedules);
    }

    let mut params = DisabledSurfaceMap::default();
    let mut actions = DisabledSurfaceMap::default();
    for feature in &features {
        let entry = feature.surface();
        let owns_shared_params =
            *feature == SubagentSurfaceFeature::Feature(SubagentFeature::WorkflowScripts);
        for param in entry.params {
            // pi `:90` — see this function's doc on the shared `preflight` param.
            if owns_shared_params || params.get(param).is_none() {
                params.set(param, *feature);
            }
        }
        // pi `:91` sets every action unconditionally. No action is shared between two groups
        // today (`no_action_is_owned_by_two_groups` asserts it), so the difference from the param
        // loop above is latent — but it is upstream's, and `DisabledSurfaceMap::set`'s in-place
        // update means a future shared action would be attributed to the LAST group listed, as
        // upstream's would.
        for action in entry.actions {
            actions.set(action, *feature);
        }
    }
    DisabledFeatureSurface {
        features,
        params,
        actions,
    }
}

/// pi's default `label` for [`disabled_feature_use_error`] (`disabled-features.ts:98`) — the
/// parent-facing tool's own name. Upstream's other two call sites pass `"RPC spawn"`
/// (`extension/rpc.ts:535`) and `` `workflow child '<key>'` ``
/// (`runs/foreground/subagent-executor.ts:5294`).
pub const DEFAULT_USE_ERROR_LABEL: &str = "subagent";

/// pi `disabledFeatureUseError` (`disabled-features.ts:97-113`): *"Returns why a request uses a
/// disabled feature, or undefined when every requested field is enabled."*
///
/// Three checks, in upstream's order, returning the FIRST match:
///
/// 1. the request's `action`, trimmed, against `surface.actions` (`:101-103`) — the refusal names
///    the TRIMMED verb, so `" create "` is reported as `'create'`;
/// 2. each disabled param, in [`DisabledSurfaceMap`] order, for a key the request CARRIES
///    (`:104-106`);
/// 3. the `workflowScript` carrier (`:109-111`) — upstream's comment: *"workflowScript is the
///    internal carrier for slash, prompt-workflow, RPC, and scheduled scripts. Callers check the
///    original request, before the package lowers chain/tasks into its own script."*
///
/// Which refusal step 2 produces for a request carrying two disabled params depends on the
/// operator's config order; see [`DisabledSurfaceMap`].
///
/// # A declared `null` still uses the param
///
/// Upstream's test is `params[param] !== undefined`, and a JSON `null` is not `undefined` — so
/// `{"gate": null}` IS a use of `gate`. Testing key PRESENCE (`Map::contains_key`) rather than
/// `Some(v) if !v.is_null()` is what keeps that true, and it is the safe direction: a caller who
/// writes the key is reaching for the feature whatever they put in it.
#[must_use]
pub fn disabled_feature_use_error(
    request: &Map<String, Value>,
    surface: &DisabledFeatureSurface,
    label: &str,
) -> Option<String> {
    let action = request.get("action").and_then(Value::as_str).map(str::trim);
    if let Some(action) = action
        && let Some(feature) = surface.actions.get(action)
    {
        return Some(format!(
            "{label} action '{action}' is disabled by config {}.",
            feature.disabled_by()
        ));
    }
    for (param, feature) in surface.params.iter() {
        if request.contains_key(param) {
            return Some(format!(
                "{label} option '{param}' is disabled by config {}.",
                feature.disabled_by()
            ));
        }
    }
    if request.contains_key("workflowScript")
        && surface.contains(SubagentSurfaceFeature::Feature(
            SubagentFeature::WorkflowScripts,
        ))
    {
        return Some(format!(
            "{label} workflow scripts are disabled by config {}.",
            SubagentSurfaceFeature::Feature(SubagentFeature::WorkflowScripts).disabled_by()
        ));
    }
    None
}

/// pi `disabledFeatureNotice` (`disabled-features.ts:115-123`): *"Lists what config disabled, for
/// prepending to static reference docs that describe the full tool."*
///
/// One line per disabled group, in the operator's config order with `schedules` last.
///
/// # It reports each group's OWN surface, not the resolved attribution
///
/// Upstream re-reads `featureSurface(feature)` per line (`:118`) rather than filtering
/// `surface.params`, so a disabled `preflight` group still prints `options preflight` even when
/// `workflow-scripts` took the attribution in [`resolve_disabled_feature_surface`]. That is
/// correct for what this text is for — telling the operator what each group they named covers,
/// not which group a refusal would cite.
///
/// `options …` is emitted unconditionally and `actions …` only when the group has actions,
/// exactly as upstream builds its `parts` array (`:119`). Every group and the schedule surface
/// has at least one param, so the `options` half is never empty.
#[must_use]
pub fn disabled_feature_notice(surface: &DisabledFeatureSurface) -> Option<String> {
    if surface.is_empty() {
        return None;
    }
    let lines = surface
        .features
        .iter()
        .map(|feature| {
            let entry = feature.surface();
            let mut parts = vec![format!("options {}", entry.params.join(", "))];
            if !entry.actions.is_empty() {
                parts.push(format!("actions {}", entry.actions.join(", ")));
            }
            format!("- {}: {}", feature.disabled_by(), parts.join("; "))
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "Disabled by config in this session. The reference below still lists these, but calls that use them are rejected:\n{lines}"
    ))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use super::*;

    /// Every [`SubagentFeature`] variant, for the exhaustive sweeps below. A `match` in
    /// [`SubagentFeature::as_str`] keeps the enum honest; this list keeps the SWEEPS honest, and
    /// `the_feature_list_is_upstreams_fifteen_names_in_upstreams_order` ties it to the table.
    const ALL: [SubagentFeature; 15] = [
        SubagentFeature::AgentManagement,
        SubagentFeature::Watchdog,
        SubagentFeature::Panes,
        SubagentFeature::Missions,
        SubagentFeature::LaneManagement,
        SubagentFeature::SpawnBudgetGrants,
        SubagentFeature::Preflight,
        SubagentFeature::LaneMetadata,
        SubagentFeature::Gates,
        SubagentFeature::UsageBudgets,
        SubagentFeature::ToolBudgets,
        SubagentFeature::ControlOverrides,
        SubagentFeature::ExtensionBindings,
        SubagentFeature::ExternalMachines,
        SubagentFeature::WorkflowScripts,
    ];

    fn surface_of(names: &[&str], scheduled_runs_enabled: bool) -> DisabledFeatureSurface {
        let features: Vec<SubagentFeature> = names
            .iter()
            .map(|name| SubagentFeature::from_wire(name).expect("a real feature name"))
            .collect();
        resolve_disabled_feature_surface(&features, scheduled_runs_enabled)
    }

    fn request(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect()
    }

    // ---- the table ----

    /// The 15 group names, in upstream's declaration order (`disabled-features.ts:8-39`
    /// @v0.75.0). The ORDER is observable: it is what
    /// [`validate_disabled_features`]'s unknown-entry sentence joins.
    ///
    /// Mutation killed: reordering [`SUBAGENT_FEATURES`], renaming a group, or dropping one.
    #[test]
    fn the_feature_list_is_upstreams_fifteen_names_in_upstreams_order() {
        assert_eq!(
            SUBAGENT_FEATURES
                .iter()
                .map(|entry| entry.feature.as_str())
                .collect::<Vec<_>>(),
            [
                "agent-management",
                "watchdog",
                "panes",
                "missions",
                "lane-management",
                "spawn-budget-grants",
                "preflight",
                "lane-metadata",
                "gates",
                "usage-budgets",
                "tool-budgets",
                "control-overrides",
                "extension-bindings",
                "external-machines",
                "workflow-scripts",
            ],
            "pi SUBAGENT_FEATURES, in pi's own order"
        );
        assert_eq!(
            feature_name_list(),
            "agent-management, watchdog, panes, missions, lane-management, spawn-budget-grants, preflight, lane-metadata, gates, usage-budgets, tool-budgets, control-overrides, extension-bindings, external-machines, workflow-scripts",
            "Object.keys(SUBAGENT_FEATURES).join(\", \")"
        );
        assert_eq!(
            ALL.map(SubagentFeature::as_str).to_vec(),
            SUBAGENT_FEATURES
                .iter()
                .map(|entry| entry.feature.as_str())
                .collect::<Vec<_>>(),
            "the test sweep list must stay the table's list"
        );
    }

    /// Every group's `{actions, params}` pair, ported verbatim from
    /// `disabled-features.ts:9-38` @v0.75.0. Member ORDER matters for
    /// [`disabled_feature_notice`], which joins each list as written.
    ///
    /// Mutation killed: dropping, renaming or reordering any action or param in any group — the
    /// whole point of the row is that the table is not paraphrased.
    #[test]
    fn every_groups_members_are_upstreams_verbatim() {
        let expected: [(&str, &[&str], &[&str]); 15] = [
            (
                "agent-management",
                &[
                    "create",
                    "update",
                    "delete",
                    "eject",
                    "disable",
                    "enable",
                    "reset",
                    "refine",
                    "refine.show",
                    "refine.rollback",
                ],
                &["config"],
            ),
            (
                "watchdog",
                &[
                    "watchdog.status",
                    "watchdog.check",
                    "watchdog.configure",
                    "watchdog.recommend-model",
                ],
                &["scope", "target", "thinking"],
            ),
            (
                "panes",
                &[
                    "inspector.open",
                    "inspector.command",
                    "inspector.status",
                    "inspector.close",
                    "project.open",
                    "project.status",
                    "project.close",
                ],
                &["focus"],
            ),
            (
                "missions",
                &[
                    "mission.create",
                    "mission.list",
                    "mission.show",
                    "mission.update",
                    "mission.resolve-decision",
                    "mission.attach-run",
                    "mission.close",
                ],
                &[
                    "mission",
                    "missionUpdate",
                    "missionStatus",
                    "missionScope",
                    "missionId",
                    "runMode",
                    "runStatus",
                    "summary",
                ],
            ),
            (
                "lane-management",
                &[
                    "lane.status",
                    "lane.recordMerge",
                    "lane.recordSupersession",
                    "worktree.discard",
                    "worktree.cleanup",
                ],
                &[
                    "handoffPath",
                    "laneId",
                    "merge",
                    "supersession",
                    "repo",
                    "planId",
                ],
            ),
            (
                "spawn-budget-grants",
                &["grant-spawn-budget"],
                &["additional"],
            ),
            ("preflight", &[], &["preflight"]),
            ("lane-metadata", &[], &["lane"]),
            ("gates", &[], &["gate"]),
            ("usage-budgets", &[], &["usageBudget"]),
            ("tool-budgets", &[], &["toolBudget"]),
            ("control-overrides", &[], &["control"]),
            ("extension-bindings", &[], &["extensionBindings"]),
            ("external-machines", &[], &["machine"]),
            (
                "workflow-scripts",
                &["validate"],
                &[
                    "workflow",
                    "args",
                    "preflight",
                    "globalConcurrencyLimit",
                    "maxSubagentSpawnsPerRun",
                ],
            ),
        ];
        for (name, actions, params) in expected {
            let feature = SubagentFeature::from_wire(name).expect("a real feature name");
            let entry = feature.surface();
            assert_eq!(entry.actions, actions, "{name} actions");
            assert_eq!(entry.params, params, "{name} params");
        }

        // pi `SCHEDULE_SURFACE` (`disabled-features.ts:46-49`).
        let schedules = SubagentSurfaceFeature::Schedules.surface();
        assert_eq!(
            schedules.actions,
            [
                "schedule.create",
                "schedule.list",
                "schedule.show",
                "schedule.history",
                "schedule.pause",
                "schedule.resume",
                "schedule.run",
                "schedule.run-due",
                "schedule.delete",
            ]
        );
        assert_eq!(
            schedules.params,
            [
                "name",
                "at",
                "every",
                "sessionOnly",
                "quiet",
                "on",
                "timezone",
                "overlap",
                "catchUp",
            ]
        );
    }

    /// [`SubagentFeature::surface`]'s linear scan must find a row for every variant, and exactly
    /// one — its unreachable fallback returns an EMPTY surface, which would silently stop gating
    /// a whole group.
    ///
    /// Mutation killed: dropping a table row (its group then gates nothing) or duplicating one.
    #[test]
    fn every_feature_has_exactly_one_table_row() {
        for feature in ALL {
            let rows = SUBAGENT_FEATURES
                .iter()
                .filter(|entry| entry.feature == feature)
                .count();
            assert_eq!(rows, 1, "{} has {rows} table rows", feature.as_str());
            let entry = feature.surface();
            assert_eq!(entry.feature, feature);
            assert!(
                !entry.params.is_empty(),
                "{}: every upstream group owns at least one param, which is why \
                 disabled_feature_notice emits `options` unconditionally",
                feature.as_str()
            );
        }
        assert_eq!(SUBAGENT_FEATURES.len(), 15);
    }

    /// `preflight` is the ONE param two groups share — upstream says so at `:3-5` (*"`preflight`
    /// is the one shared parameter"*), and [`resolve_disabled_feature_surface`]'s attribution rule
    /// exists only for it. If a second shared param ever arrives upstream this fails, which is the
    /// signal to re-read that rule rather than assume it still covers one name.
    ///
    /// Mutation killed: adding a param to a second group without revisiting the attribution rule.
    #[test]
    fn preflight_is_the_only_param_two_groups_share() {
        let mut shared: Vec<&str> = Vec::new();
        for entry in &SUBAGENT_FEATURES {
            for param in entry.params {
                let owners = SUBAGENT_FEATURES
                    .iter()
                    .filter(|other| other.params.contains(param))
                    .count();
                if owners > 1 && !shared.contains(param) {
                    shared.push(param);
                }
            }
        }
        assert_eq!(shared, ["preflight"], "pi `disabled-features.ts:3-5`");
    }

    /// No action is owned by two groups, which is why
    /// [`resolve_disabled_feature_surface`]'s action loop can be unconditional as upstream's is.
    #[test]
    fn no_action_is_owned_by_two_groups() {
        let mut seen: Vec<&str> = Vec::new();
        for entry in SUBAGENT_FEATURES.iter().copied().chain([SCHEDULE_SURFACE]) {
            for action in entry.actions {
                assert!(!seen.contains(action), "{action} is owned twice");
                seen.push(action);
            }
        }
    }

    /// The judgement call this row records in code: every ACTION the 15 groups and the schedule
    /// surface name is a verb this port actually dispatches, so no group's action gate is a no-op.
    ///
    /// The PARAM side is deliberately not asserted this way — `preflight`, `gate` and
    /// `extensionBindings` (and eleven further params of `missions`, `lane-management` and
    /// `workflow-scripts`) are not advertised by this port, and the module doc records why
    /// accepting those group names anyway is correct rather than a bug.
    ///
    /// Mutation killed: renaming a group action to something this port does not dispatch.
    #[test]
    fn every_group_action_is_a_verb_this_port_dispatches() {
        let dispatched = crate::extension::subagent_actions();
        for entry in SUBAGENT_FEATURES.iter().copied().chain([SCHEDULE_SURFACE]) {
            for action in entry.actions {
                assert!(
                    dispatched.contains(action),
                    "{action} is gated but not dispatched by this port"
                );
            }
        }
    }

    /// [`SubagentFeature::from_wire`] round-trips [`SubagentFeature::as_str`], refuses the
    /// `schedules` pseudo-name (which has its own refusal), and is the one definition serde's
    /// `Deserialize` uses.
    ///
    /// Mutation killed: a `rename_all = "kebab-case"` derive in place of the manual impl would
    /// pass the round-trip but is the drift risk the impl's doc records; accepting `"schedules"`
    /// here would route it to the generic unknown-entry message.
    #[test]
    fn wire_names_round_trip_and_exclude_the_schedules_pseudo_name() {
        for feature in ALL {
            assert_eq!(SubagentFeature::from_wire(feature.as_str()), Some(feature));
            assert_eq!(
                serde_json::from_value::<SubagentFeature>(Value::String(
                    feature.as_str().to_string()
                ))
                .expect("a real name deserializes"),
                feature
            );
            assert_eq!(
                serde_json::to_value(feature).expect("serializes"),
                Value::String(feature.as_str().to_string())
            );
        }
        assert_eq!(SubagentFeature::from_wire("schedules"), None);
        assert_eq!(SubagentFeature::from_wire("AgentManagement"), None);
        assert_eq!(SubagentFeature::from_wire("agent_management"), None);
        assert_eq!(SubagentFeature::from_wire(""), None);
    }

    // ---- behaviour 1: validateDisabledFeatures ----

    /// pi `validateDisabledFeatures` (`disabled-features.ts:51-66`): FOUR distinct refusals, each
    /// with upstream's exact sentence, plus the absent/empty accepting cases.
    ///
    /// Mutation killed: collapsing the `"schedules"` arm into the generic unknown-entry message
    /// (the operator is then handed a 15-name list that deliberately omits `schedules` and is
    /// never told which key to use); dropping the duplicate check; accepting a non-array;
    /// accepting an explicit `null`.
    #[test]
    fn validate_disabled_features_refuses_upstreams_four_cases_by_sentence() {
        // Absent is the common case.
        assert_eq!(
            validate_disabled_features(&serde_json::json!({})),
            Ok(()),
            "pi `if (value === undefined) return;` (`:53`)"
        );
        assert_eq!(
            validate_disabled_features(&serde_json::json!({"disabledFeatures": []})),
            Ok(()),
            "an empty array disables nothing and is legal"
        );
        // Every real name, singly and all fifteen at once.
        for feature in ALL {
            assert_eq!(
                validate_disabled_features(
                    &serde_json::json!({"disabledFeatures": [feature.as_str()]})
                ),
                Ok(()),
                "{} must be accepted: an operator moving a config from pi must never be told a \
                 legitimate feature name is invalid",
                feature.as_str()
            );
        }
        assert_eq!(
            validate_disabled_features(&serde_json::json!({
                "disabledFeatures": ALL.map(SubagentFeature::as_str),
            })),
            Ok(())
        );

        // (1) not an array (`:54`) — including an explicit `null`, which is not `undefined`.
        for value in [
            serde_json::json!(null),
            serde_json::json!("agent-management"),
            serde_json::json!({"agent-management": true}),
            serde_json::json!(7),
            serde_json::json!(false),
        ] {
            assert_eq!(
                validate_disabled_features(&serde_json::json!({"disabledFeatures": value})),
                Err("config.disabledFeatures must be an array of feature names".to_string()),
                "{value} is not an array"
            );
        }

        // (2) the literal `"schedules"` (`:57`) — its OWN message, naming the key to use.
        assert_eq!(
            validate_disabled_features(&serde_json::json!({"disabledFeatures": ["schedules"]})),
            Err(
                r#"config.disabledFeatures does not accept "schedules"; set config.scheduledRuns.enabled to false instead"#
                    .to_string()
            )
        );
        // Checked per entry, and BEFORE the membership test, so position does not matter.
        assert_eq!(
            validate_disabled_features(
                &serde_json::json!({"disabledFeatures": ["watchdog", "schedules"]})
            ),
            Err(
                r#"config.disabledFeatures does not accept "schedules"; set config.scheduledRuns.enabled to false instead"#
                    .to_string()
            )
        );

        // (3) an unknown entry (`:58-59`) — the full comma-joined list, and the entry
        // JSON-stringified (so a string keeps its quotes).
        let list = "agent-management, watchdog, panes, missions, lane-management, spawn-budget-grants, preflight, lane-metadata, gates, usage-budgets, tool-budgets, control-overrides, extension-bindings, external-machines, workflow-scripts";
        assert_eq!(
            validate_disabled_features(&serde_json::json!({"disabledFeatures": ["watchdogs"]})),
            Err(format!(
                r#"config.disabledFeatures entry "watchdogs" is not one of: {list}"#
            ))
        );
        // `JSON.stringify` of a non-string entry.
        assert_eq!(
            validate_disabled_features(&serde_json::json!({"disabledFeatures": [7]})),
            Err(format!(
                "config.disabledFeatures entry 7 is not one of: {list}"
            ))
        );
        assert_eq!(
            validate_disabled_features(&serde_json::json!({"disabledFeatures": [null]})),
            Err(format!(
                "config.disabledFeatures entry null is not one of: {list}"
            ))
        );
        assert_eq!(
            validate_disabled_features(&serde_json::json!({"disabledFeatures": [{"a": 1}]})),
            Err(format!(
                r#"config.disabledFeatures entry {{"a":1}} is not one of: {list}"#
            ))
        );

        // (4) a duplicate (`:60`).
        assert_eq!(
            validate_disabled_features(
                &serde_json::json!({"disabledFeatures": ["watchdog", "panes", "watchdog"]})
            ),
            Err(r#"config.disabledFeatures lists "watchdog" more than once"#.to_string())
        );
        // The duplicate check runs AFTER membership, so an unknown name in between still reports
        // as unknown rather than as a duplicate.
        assert_eq!(
            validate_disabled_features(
                &serde_json::json!({"disabledFeatures": ["watchdog", "nope", "watchdog"]})
            ),
            Err(format!(
                r#"config.disabledFeatures entry "nope" is not one of: {list}"#
            ))
        );
    }

    // ---- behaviour 2: resolveDisabledFeatureSurface ----

    /// pi `resolveDisabledFeatureSurface` (`:82-95`): the `{features, params, actions}` triple,
    /// each entry carrying the setting that disabled it.
    ///
    /// Mutation killed: dropping the `disabledFeatures "<name>"` quoting; attributing an action to
    /// the wrong group; resolving a surface for a feature the config did not name.
    #[test]
    fn resolve_builds_the_three_maps_with_the_setting_that_disabled_each_entry() {
        let empty = resolve_disabled_feature_surface(&[], true);
        assert!(empty.is_empty(), "nothing disabled");
        assert!(empty.params().is_empty() && empty.actions().is_empty());
        assert_eq!(empty.features(), &[] as &[SubagentSurfaceFeature]);

        let surface = surface_of(&["watchdog", "gates"], true);
        assert_eq!(
            surface.features(),
            [
                SubagentSurfaceFeature::Feature(SubagentFeature::Watchdog),
                SubagentSurfaceFeature::Feature(SubagentFeature::Gates),
            ]
        );
        for action in [
            "watchdog.status",
            "watchdog.check",
            "watchdog.configure",
            "watchdog.recommend-model",
        ] {
            assert_eq!(
                surface.actions().get(action).map(|f| f.disabled_by()),
                Some(r#"disabledFeatures "watchdog""#.to_string()),
                "{action}"
            );
        }
        for param in ["scope", "target", "thinking"] {
            assert_eq!(
                surface.params().get(param).map(|f| f.disabled_by()),
                Some(r#"disabledFeatures "watchdog""#.to_string()),
                "{param}"
            );
        }
        assert_eq!(
            surface.params().get("gate").map(|f| f.disabled_by()),
            Some(r#"disabledFeatures "gates""#.to_string())
        );
        // `gates` has no actions, so nothing of it reaches the action map.
        assert_eq!(surface.actions().len(), 4);
        assert_eq!(surface.params().len(), 4);
        // An enabled group is absent from every map.
        assert_eq!(surface.params().get("config"), None);
        assert_eq!(surface.actions().get("create"), None);
        assert!(!surface.contains(SubagentSurfaceFeature::Schedules));

        // A duplicate in an UNVALIDATED list dedupes while keeping the first position, as
        // `new Set(...)` does — `validate_disabled_features` refuses it, but this function is
        // reachable without it.
        let deduped = surface_of(&["gates", "watchdog", "gates"], true);
        assert_eq!(
            deduped.features(),
            [
                SubagentSurfaceFeature::Feature(SubagentFeature::Gates),
                SubagentSurfaceFeature::Feature(SubagentFeature::Watchdog),
            ]
        );
    }

    /// pi `:84` — `scheduledRuns.enabled === false` ADDS the synthetic `schedules` group, with its
    /// own action/param surface and its own `disabledBy` string naming a DIFFERENT config key.
    /// It is always appended LAST, after whatever the operator listed.
    ///
    /// Mutation killed: inverting the polarity (every default config would then lose the nine
    /// `schedule.*` verbs); reusing the `disabledFeatures "schedules"` wording (which would point
    /// the operator at a key that refuses the name); inserting `schedules` before the operator's
    /// own groups.
    #[test]
    fn scheduled_runs_disabled_adds_the_synthetic_schedules_group_last() {
        // Enabled (the default, and `true`) adds nothing.
        let enabled = surface_of(&["gates"], true);
        assert!(!enabled.contains(SubagentSurfaceFeature::Schedules));
        assert_eq!(enabled.actions().get("schedule.create"), None);
        assert_eq!(enabled.params().get("catchUp"), None);

        let surface = surface_of(&["gates"], false);
        assert_eq!(
            surface.features(),
            [
                SubagentSurfaceFeature::Feature(SubagentFeature::Gates),
                SubagentSurfaceFeature::Schedules,
            ],
            "`schedules` is appended after the operator's own groups"
        );
        for action in [
            "schedule.create",
            "schedule.list",
            "schedule.show",
            "schedule.history",
            "schedule.pause",
            "schedule.resume",
            "schedule.run",
            "schedule.run-due",
            "schedule.delete",
        ] {
            assert_eq!(
                surface.actions().get(action).map(|f| f.disabled_by()),
                Some("scheduledRuns.enabled=false".to_string()),
                "{action}: a DIFFERENT config key from every real group's"
            );
        }
        for param in [
            "name",
            "at",
            "every",
            "sessionOnly",
            "quiet",
            "on",
            "timezone",
            "overlap",
            "catchUp",
        ] {
            assert_eq!(
                surface.params().get(param).map(|f| f.disabled_by()),
                Some("scheduledRuns.enabled=false".to_string()),
                "{param}"
            );
        }
        // Every REAL group's string is the other form.
        assert_eq!(
            SubagentSurfaceFeature::Feature(SubagentFeature::Gates).disabled_by(),
            r#"disabledFeatures "gates""#
        );
        // `scheduledRuns.enabled: false` with no `disabledFeatures` at all is just `schedules`.
        let only = resolve_disabled_feature_surface(&[], false);
        assert_eq!(only.features(), [SubagentSurfaceFeature::Schedules]);
    }

    // ---- fidelity trap 1: the shared `preflight` param ----

    /// pi `:88-90` — `preflight` belongs to BOTH the `preflight` group and `workflow-scripts`, and
    /// upstream attributes it to `workflow-scripts` whenever that feature is disabled, **whatever
    /// order the config lists the features in**. The `||` in
    /// `feature === "workflow-scripts" || !params.has(param)` is what does it.
    ///
    /// Both orders are tried because a port that dropped the first half of that disjunction is
    /// wrong ONLY in the `["preflight", "workflow-scripts"]` order — the case no casual test
    /// covers.
    ///
    /// Mutation killed: replacing the condition with a bare `!params.has(param)`. The
    /// `preflight`-first order then reports `disabledFeatures "preflight"`.
    #[test]
    fn workflow_scripts_owns_the_shared_preflight_param_in_either_config_order() {
        for order in [
            ["preflight", "workflow-scripts"],
            ["workflow-scripts", "preflight"],
        ] {
            let surface = surface_of(&order, true);
            assert_eq!(
                surface.params().get("preflight").map(|f| f.disabled_by()),
                Some(r#"disabledFeatures "workflow-scripts""#.to_string()),
                "config order {order:?}: pi attributes the shared param to workflow-scripts"
            );
            // And the refusal a caller reads says so.
            assert_eq!(
                disabled_feature_use_error(
                    &request(&[("preflight", serde_json::json!({"version": 1}))]),
                    &surface,
                    DEFAULT_USE_ERROR_LABEL,
                ),
                Some(
                    r#"subagent option 'preflight' is disabled by config disabledFeatures "workflow-scripts"."#
                        .to_string()
                ),
                "config order {order:?}"
            );
        }

        // With ONLY the `preflight` group disabled, it is of course its own.
        assert_eq!(
            surface_of(&["preflight"], true)
                .params()
                .get("preflight")
                .map(|f| f.disabled_by()),
            Some(r#"disabledFeatures "preflight""#.to_string())
        );
        // `workflow-scripts` alone, likewise.
        assert_eq!(
            surface_of(&["workflow-scripts"], true)
                .params()
                .get("preflight")
                .map(|f| f.disabled_by()),
            Some(r#"disabledFeatures "workflow-scripts""#.to_string())
        );

        // The in-place `set` keeps `preflight` at the position the FIRST group gave it, as JS
        // `Map.set` does — only its value changes. With `preflight` listed first it is therefore
        // still the first param in the map, which is what the order-sensitivity test below rests
        // on.
        let preflight_first = surface_of(&["preflight", "workflow-scripts"], true);
        assert_eq!(
            preflight_first.params().iter().next().map(|(name, _)| name),
            Some("preflight")
        );
        let scripts_first = surface_of(&["workflow-scripts", "preflight"], true);
        assert_eq!(
            scripts_first.params().iter().next().map(|(name, _)| name),
            Some("workflow")
        );
    }

    // ---- fidelity trap 2: map order is observable ----

    /// pi `:104-106` returns the FIRST disabled param it finds, iterating a JS `Map` in insertion
    /// order — which follows the `Set` order of `features`, i.e. **the operator's config array
    /// order**. So when ONE request carries two disabled params, WHICH refusal the caller reads
    /// depends on how the operator ordered `config.disabledFeatures`.
    ///
    /// Mutation killed: a `HashMap` (the sentence becomes nondeterministic) or a `BTreeMap` (both
    /// orders report `gate`, since `gate` sorts before `machine` — this test's two assertions then
    /// cannot both hold). Also killed: appending on re-set in [`DisabledSurfaceMap::set`].
    #[test]
    fn the_first_disabled_param_follows_the_operators_config_order() {
        let both = request(&[
            ("gate", serde_json::json!({})),
            ("machine", serde_json::json!("builder")),
        ]);

        let gates_first = surface_of(&["gates", "external-machines"], true);
        assert_eq!(
            disabled_feature_use_error(&both, &gates_first, DEFAULT_USE_ERROR_LABEL),
            Some(
                r#"subagent option 'gate' is disabled by config disabledFeatures "gates"."#
                    .to_string()
            ),
        );

        let machines_first = surface_of(&["external-machines", "gates"], true);
        assert_eq!(
            disabled_feature_use_error(&both, &machines_first, DEFAULT_USE_ERROR_LABEL),
            Some(
                r#"subagent option 'machine' is disabled by config disabledFeatures "external-machines"."#
                    .to_string()
            ),
            "`machine` sorts AFTER `gate`, so a BTreeMap cannot produce this"
        );

        // The maps themselves carry the order, not just the refusal.
        assert_eq!(
            gates_first
                .params()
                .iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            ["gate", "machine"]
        );
        assert_eq!(
            machines_first
                .params()
                .iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            ["machine", "gate"]
        );
    }

    // ---- behaviour 3: disabledFeatureUseError ----

    /// pi `disabledFeatureUseError` (`:97-113`): the `action` first, then each disabled param,
    /// then the `workflowScript` carrier — returning the FIRST match.
    ///
    /// Mutation killed: checking params before the action (a request with both then names the
    /// param); dropping the `.trim()`; reporting the untrimmed verb; treating a declared `null`
    /// as "not used"; firing on an enabled param.
    #[test]
    fn use_error_checks_the_action_then_each_param_then_the_carrier() {
        let surface = surface_of(&["agent-management", "gates"], true);

        // Nothing disabled is used.
        assert_eq!(
            disabled_feature_use_error(
                &request(&[("action", serde_json::json!("list"))]),
                &surface,
                DEFAULT_USE_ERROR_LABEL
            ),
            None
        );
        assert_eq!(
            disabled_feature_use_error(&request(&[]), &surface, DEFAULT_USE_ERROR_LABEL),
            None
        );
        // A surface that disables nothing never refuses anything.
        assert_eq!(
            disabled_feature_use_error(
                &request(&[
                    ("action", serde_json::json!("create")),
                    ("gate", serde_json::json!({}))
                ]),
                &resolve_disabled_feature_surface(&[], true),
                DEFAULT_USE_ERROR_LABEL
            ),
            None
        );

        // A disabled action.
        assert_eq!(
            disabled_feature_use_error(
                &request(&[("action", serde_json::json!("refine.rollback"))]),
                &surface,
                DEFAULT_USE_ERROR_LABEL
            ),
            Some(
                r#"subagent action 'refine.rollback' is disabled by config disabledFeatures "agent-management"."#
                    .to_string()
            )
        );
        // `.trim()` (`:100`), and the TRIMMED verb is what the sentence names.
        assert_eq!(
            disabled_feature_use_error(
                &request(&[("action", serde_json::json!("  create \t"))]),
                &surface,
                DEFAULT_USE_ERROR_LABEL
            ),
            Some(
                r#"subagent action 'create' is disabled by config disabledFeatures "agent-management"."#
                    .to_string()
            )
        );
        // A non-string `action` is `undefined` upstream, so no action check runs.
        assert_eq!(
            disabled_feature_use_error(
                &request(&[("action", serde_json::json!(7))]),
                &surface,
                DEFAULT_USE_ERROR_LABEL
            ),
            None
        );

        // The action is checked BEFORE the params: a request with both names the action.
        assert_eq!(
            disabled_feature_use_error(
                &request(&[
                    ("action", serde_json::json!("create")),
                    ("gate", serde_json::json!({})),
                ]),
                &surface,
                DEFAULT_USE_ERROR_LABEL
            ),
            Some(
                r#"subagent action 'create' is disabled by config disabledFeatures "agent-management"."#
                    .to_string()
            ),
            "pi returns the action refusal before looking at any param"
        );

        // A disabled param, including when its value is an explicit `null` — upstream's test is
        // `params[param] !== undefined`, and `null` is not `undefined`.
        for value in [
            serde_json::json!(null),
            serde_json::json!({}),
            serde_json::json!(false),
        ] {
            assert_eq!(
                disabled_feature_use_error(
                    &request(&[("gate", value.clone())]),
                    &surface,
                    DEFAULT_USE_ERROR_LABEL
                ),
                Some(
                    r#"subagent option 'gate' is disabled by config disabledFeatures "gates"."#
                        .to_string()
                ),
                "gate: {value}"
            );
        }

        // The label is the caller's — pi's other two call sites pass `"RPC spawn"`
        // (`extension/rpc.ts:535`) and `workflow child '<key>'`
        // (`runs/foreground/subagent-executor.ts:5294`).
        assert_eq!(
            disabled_feature_use_error(
                &request(&[("gate", serde_json::json!({}))]),
                &surface,
                "RPC spawn"
            ),
            Some(
                r#"RPC spawn option 'gate' is disabled by config disabledFeatures "gates"."#
                    .to_string()
            )
        );
        assert_eq!(DEFAULT_USE_ERROR_LABEL, "subagent");
    }

    /// pi `:108-111` — the `workflowScript` carrier is checked LAST, and ONLY when
    /// `workflow-scripts` is in the feature set. It is not a member of any group's `params` list,
    /// so nothing else would catch it.
    ///
    /// Mutation killed: dropping the `features.has("workflow-scripts")` guard (a config that
    /// disables only `gates` would then refuse every scripted run); checking the carrier before
    /// the params; adding `workflowScript` to the `workflow-scripts` params list, which would
    /// move the refusal into the param loop and change its sentence.
    #[test]
    fn the_workflow_script_carrier_is_refused_only_when_workflow_scripts_is_disabled() {
        let carrier = request(&[("workflowScript", serde_json::json!("return 1;"))]);

        // `workflow-scripts` disabled: its own sentence, naming neither an action nor an option.
        assert_eq!(
            disabled_feature_use_error(
                &carrier,
                &surface_of(&["workflow-scripts"], true),
                DEFAULT_USE_ERROR_LABEL
            ),
            Some(
                r#"subagent workflow scripts are disabled by config disabledFeatures "workflow-scripts"."#
                    .to_string()
            )
        );

        // Some OTHER group disabled, or nothing: the carrier passes.
        assert_eq!(
            disabled_feature_use_error(
                &carrier,
                &surface_of(&["gates", "panes"], false),
                DEFAULT_USE_ERROR_LABEL
            ),
            None,
            "only `workflow-scripts` gates the carrier"
        );
        assert_eq!(
            disabled_feature_use_error(
                &carrier,
                &resolve_disabled_feature_surface(&[], true),
                DEFAULT_USE_ERROR_LABEL
            ),
            None
        );

        // `workflowScript` is not in any group's param list, so the param loop cannot catch it.
        assert!(
            !SUBAGENT_FEATURES
                .iter()
                .any(|entry| entry.params.contains(&"workflowScript")),
            "the carrier is checked by its own arm, not through the param map"
        );

        // Checked AFTER the params: a request carrying both a disabled param and the carrier
        // names the param.
        assert_eq!(
            disabled_feature_use_error(
                &request(&[
                    ("workflowScript", serde_json::json!("return 1;")),
                    ("workflow", serde_json::json!("build")),
                ]),
                &surface_of(&["workflow-scripts"], true),
                DEFAULT_USE_ERROR_LABEL
            ),
            Some(
                r#"subagent option 'workflow' is disabled by config disabledFeatures "workflow-scripts"."#
                    .to_string()
            )
        );
        // And after the action.
        assert_eq!(
            disabled_feature_use_error(
                &request(&[
                    ("workflowScript", serde_json::json!("return 1;")),
                    ("action", serde_json::json!("validate")),
                ]),
                &surface_of(&["workflow-scripts"], true),
                DEFAULT_USE_ERROR_LABEL
            ),
            Some(
                r#"subagent action 'validate' is disabled by config disabledFeatures "workflow-scripts"."#
                    .to_string()
            )
        );
    }

    // ---- behaviour 4: disabledFeatureNotice ----

    /// pi `disabledFeatureNotice` (`:115-123`): the operator-facing summary prepended to the
    /// static reference docs, one line per disabled group in config order.
    ///
    /// Mutation killed: returning `Some` for an empty surface (the reference page would carry a
    /// bare header saying nothing was disabled); emitting `actions` for a params-only group;
    /// dropping the `options` half; reordering the lines; filtering through `surface.params`
    /// instead of each group's own surface.
    #[test]
    fn the_notice_lists_each_disabled_group_in_config_order() {
        assert_eq!(
            disabled_feature_notice(&resolve_disabled_feature_surface(&[], true)),
            None,
            "pi `if (surface.features.size === 0) return undefined` (`:116`)"
        );

        // A params-only group emits NO `actions` half; a group with actions emits both.
        assert_eq!(
            disabled_feature_notice(&surface_of(&["gates", "spawn-budget-grants"], true)),
            Some(
                concat!(
                    "Disabled by config in this session. The reference below still lists these, ",
                    "but calls that use them are rejected:\n",
                    "- disabledFeatures \"gates\": options gate\n",
                    "- disabledFeatures \"spawn-budget-grants\": options additional; actions grant-spawn-budget",
                )
                .to_string()
            )
        );

        // Reversing the config reverses the lines.
        assert_eq!(
            disabled_feature_notice(&surface_of(&["spawn-budget-grants", "gates"], true)),
            Some(
                concat!(
                    "Disabled by config in this session. The reference below still lists these, ",
                    "but calls that use them are rejected:\n",
                    "- disabledFeatures \"spawn-budget-grants\": options additional; actions grant-spawn-budget\n",
                    "- disabledFeatures \"gates\": options gate",
                )
                .to_string()
            )
        );

        // `schedules` gets its own line, last, naming its own key.
        let notice = disabled_feature_notice(&surface_of(&["gates"], false))
            .expect("two groups are disabled");
        let lines: Vec<&str> = notice.lines().collect();
        assert_eq!(lines.len(), 3, "a header and one line per group: {notice}");
        assert_eq!(lines[1], "- disabledFeatures \"gates\": options gate");
        assert_eq!(
            lines[2],
            concat!(
                "- scheduledRuns.enabled=false: options name, at, every, sessionOnly, quiet, on, ",
                "timezone, overlap, catchUp; actions schedule.create, schedule.list, ",
                "schedule.show, schedule.history, schedule.pause, schedule.resume, schedule.run, ",
                "schedule.run-due, schedule.delete",
            )
        );

        // It reports each group's OWN surface, not the resolved attribution: the `preflight`
        // group's line still says `options preflight` even though `workflow-scripts` took the
        // attribution in the param map.
        let shared = surface_of(&["preflight", "workflow-scripts"], true);
        assert_eq!(
            shared.params().get("preflight").map(|f| f.disabled_by()),
            Some(r#"disabledFeatures "workflow-scripts""#.to_string())
        );
        let notice = disabled_feature_notice(&shared).expect("two groups are disabled");
        assert!(
            notice.contains("- disabledFeatures \"preflight\": options preflight\n"),
            "pi re-reads `featureSurface(feature)` per line (`:118`): {notice}"
        );

        // Every group, so no group's line can be malformed unnoticed.
        let all = disabled_feature_notice(&resolve_disabled_feature_surface(&ALL, false))
            .expect("fifteen groups and schedules are disabled");
        assert_eq!(all.lines().count(), 17, "{all}");
        for feature in ALL {
            assert!(
                all.contains(&format!(
                    "- disabledFeatures \"{}\": options ",
                    feature.as_str()
                )),
                "{} has no line: {all}",
                feature.as_str()
            );
        }
    }
}
