//! `elicitation-handler.ts` — `elicitation/create`, both legs.
//!
//! A server asks the human a typed, schema-validated question (**form** mode) or asks to open a URL
//! (**url** mode). [`McpClientHandler`](crate::runtime::McpClientHandler)'s `create_elicitation` —
//! its `rmcp` `ClientHandler` impl — is the entry point; [`crate::runtime::initialize_mcp`]'s step 6
//! installs the hook, gated on `settings.elicitation(has_ui)`.
//!
//! # Unknown property types drop the field, as upstream does
//!
//! [`rmcp::model::PrimitiveSchemaDefinition`] is `#[non_exhaustive]`, so a variant rmcp adds later
//! reaches this crate as an unmatched arm rather than a compile error. Every such arm here does what
//! upstream's "any other `type`" branch does (`elicitation-handler.ts:188`): **silently drop the
//! field** — no dialog, no value in the output object. If that field was `required`, the schema
//! assertion at the end of [`coerce_and_validate`] fails it, which is also upstream's outcome.
//!
//! # The one ordered read
//!
//! [`ordered_properties`] drives four user-visible orderings — the question sequence, the review
//! rows, the edit-picker labels, and the coercion order. Iterating
//! [`rmcp::model::ElicitationSchema::properties`] directly is LEXICOGRAPHIC, which is the silent bug
//! MCP-462 names.

use indexmap::IndexMap;
use rmcp::model::{
    ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationSchema, ErrorData,
    PrimitiveSchemaDefinition,
};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::sync::Arc;

use crate::owner::McpDialog;

/// `ElicitationValue` (`elicitation-handler.ts:15`) — `string | number | boolean | string[] |
/// undefined`. `None` is `undefined`, a distinct and meaningful state: "omitted", which the coercion
/// pass turns into either a skip or the missing-required throw.
pub type ElicitationValue = Option<FieldValue>;

/// The four inhabited spellings. A closed enum rather than [`serde_json::Value`] so the coercion
/// pass's `match` is exhaustive and [`js_bool`]/[`js_number`] cannot be handed an object.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Text(String),
    Number(f64),
    Bool(bool),
    List(Vec<String>),
}

/// `options.onUrlAccepted` — `(server, elicitation_id)`.
pub type UrlAcceptedHook = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// `FieldCollectionResult` (`elicitation-handler.ts:17`).
enum FieldOutcome {
    Cancelled,
    Collected(ElicitationValue),
}

/// `ElicitationHandlerOptions` (`elicitation-handler.ts:21-26`), minus `serverName`.
pub struct ElicitationOptions {
    /// `options.allowUrl` — `mode === "tui"`, NOT `hasUI`.
    pub allow_url: bool,
    /// The generation's fenced dialog source.
    pub session: Arc<crate::runtime::SessionSlot>,
    /// `import open from "open"` — `OpenerLauncher` in production, `NoopLauncher` headless.
    pub launcher: Arc<dyn crate::oauth::BrowserLauncher>,
    /// `options.onUrlAccepted` — the manager's `remember_url_elicitation`.
    ///
    /// Takes the server name as well as the id, because one options bag is shared by every server
    /// in the generation: upstream closes over `serverName` per `createClient`, and this is the
    /// same information arriving by argument instead of by capture.
    pub on_url_accepted: UrlAcceptedHook,
    /// The shared compile cache. The validator runs once per field AND once per review pass.
    pub validators: Arc<crate::schema::ValidatorCache>,
}

// The five fixed dialog labels. `HostServices::select` returns the chosen STRING, so every
// comparison in this module is against one of these (`elicitation-handler.ts:51`, `:68`, `:75`,
// `:331`).
pub const CONTINUE: &str = "Continue";
pub const DECLINE: &str = "Decline";
pub const SUBMIT: &str = "Submit";
pub const EDIT: &str = "Edit";
pub const OPEN: &str = "Open";
pub const CHOOSE_A_FIELD: &str = "Choose a field to edit";

/// `elicitation-handler.ts:309`.
pub const URL_ELICITATION_UNSUPPORTED: &str = "URL elicitation is not supported";
/// `:315`.
pub const URL_ELICITATION_INVALID_URL: &str = "URL elicitation supplied an invalid URL";
/// `:318`.
pub const URL_ELICITATION_SCHEME: &str = "URL elicitation only supports HTTP and HTTPS URLs";
/// `:342`.
pub const OPENED_BROWSER_NOTICE: &str = "Opened browser for MCP elicitation.";
/// The `spawn_blocking` join failure — no upstream counterpart, because JS has no join handle.
pub const URL_ELICITATION_OPEN_FAILED: &str = "Could not open the browser for MCP elicitation";
/// No upstream counterpart: upstream's wire union folds an unknown `mode` into `form`, so this is
/// reachable only if rmcp models a mode this port has not implemented.
pub const UNSUPPORTED_ELICITATION_MODE: &str = "Unsupported MCP elicitation mode";

fn internal_msg(message: &str) -> ErrorData {
    ErrorData::internal_error(message.to_string(), None)
}

/// `` `Could not open browser: ${message}` `` (`elicitation-handler.ts:338`).
#[must_use]
pub fn could_not_open_message(detail: &str) -> String {
    format!("Could not open browser: {detail}")
}

/// `` `Invalid elicitation response: ${errorMessage}` `` (`elicitation-handler.ts:263`).
///
/// The MESSAGE differs from ajv's — `jsonschema`'s renderer is its own — and that is accepted. The
/// PREFIX is load-bearing and must be byte-exact.
#[must_use]
pub fn invalid_elicitation_response(detail: &str) -> String {
    format!("Invalid elicitation response: {detail}")
}

/// `` `${label} is required` `` (`elicitation-handler.ts:201`).
#[must_use]
pub fn required_message(label: &str) -> String {
    format!("{label} is required")
}

/// `` `${label} must be a number` `` (`:211`).
#[must_use]
pub fn must_be_number_message(label: &str) -> String {
    format!("{label} must be a number")
}

/// `` `${label} must be an integer` `` (`:214`).
#[must_use]
pub fn must_be_integer_message(label: &str) -> String {
    format!("{label} must be an integer")
}

/// `` `${label} must be at least ${min}` `` (`:217`).
#[must_use]
pub fn minimum_message(label: &str, minimum: f64) -> String {
    format!("{label} must be at least {}", trim_number(minimum))
}

/// `` `${label} must be at most ${max}` `` (`:220`).
#[must_use]
pub fn maximum_message(label: &str, maximum: f64) -> String {
    format!("{label} must be at most {}", trim_number(maximum))
}

/// `` `${label} must be at least ${n} characters` `` (`:227`).
#[must_use]
pub fn min_length_message(label: &str, min: u32) -> String {
    format!("{label} must be at least {min} characters")
}

/// `` `${label} must be at most ${n} characters` `` (`:229`).
#[must_use]
pub fn max_length_message(label: &str, max: u32) -> String {
    format!("{label} must be at most {max} characters")
}

/// JS prints `5` for `5.0`; Rust's `Display` for `f64` prints `5`. Kept as one helper so every limit
/// message renders the same way.
fn trim_number(value: f64) -> String {
    if value.fract() == 0.0 && value.is_finite() {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

/// `Number(value)` — JavaScript's, not `str::parse::<f64>()`.
///
/// Node-verified divergences that matter here: `"0x1f"` → 31, `"1e3"` → 1000, `"Infinity"` → ∞,
/// `" 7 "` → 7 (surrounding whitespace trimmed), `""` → 0, `"7abc"` → NaN. `str::parse` rejects the
/// first two and the fourth, and errors rather than yielding 0 on the fifth — so a blank optional
/// numeric field would take a different branch.
#[must_use]
pub fn js_number(value: &str) -> f64 {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return u64::from_str_radix(rest, radix).map_or(f64::NAN, |parsed| parsed as f64);
        }
    }
    match trimmed {
        "Infinity" | "+Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        // Rust accepts "inf"/"NaN"; JS `Number()` does not. Reject them explicitly so the two agree.
        other
            if other.eq_ignore_ascii_case("inf")
                || other.eq_ignore_ascii_case("infinity")
                || other.eq_ignore_ascii_case("nan") =>
        {
            f64::NAN
        }
        other => other.parse::<f64>().unwrap_or(f64::NAN),
    }
}

/// `output[name] = typeof value === "boolean" ? value : value === "true"` (`:241`).
///
/// Every OTHER string is `false`, silently. Do not substitute `bool::from_str`, which errors.
#[must_use]
pub fn js_bool(value: &ElicitationValue) -> bool {
    match value {
        Some(FieldValue::Bool(flag)) => *flag,
        Some(FieldValue::Text(text)) => text == "true",
        _ => false,
    }
}

/// `formatChoice(value, title)` (`elicitation-handler.ts:268-270`) — a title equal to the value is
/// suppressed, not duplicated.
#[must_use]
pub fn format_choice(value: &str, title: Option<&str>) -> String {
    match title {
        Some(title) if title != value => format!("{title} ({value})"),
        _ => value.to_string(),
    }
}

/// `uniqueLabels` (`:272-280`) — append U+2026 until unique, against an ACCUMULATING set.
///
/// Necessary because `HostServices::select` returns the chosen STRING, not an index: two identical
/// labels would make the second unselectable. The edit picker deliberately does NOT use this.
#[must_use]
pub fn unique_labels(labels: &[String]) -> Vec<String> {
    let mut used: HashSet<String> = HashSet::new();
    labels
        .iter()
        .map(|label| {
            let mut unique = label.clone();
            while !used.insert(unique.clone()) {
                unique.push('…');
            }
            unique
        })
        .collect()
}

/// `uniqueAction(label, choices)` (`:282-286`) — the same trick for an action added BESIDE the
/// choices, tested against the list rather than a set.
#[must_use]
pub fn unique_action(label: &str, choices: &[String]) -> String {
    let mut unique = label.to_string();
    while choices.contains(&unique) {
        unique.push('…');
    }
    unique
}

/// `humanizeName(name)` (`:346-348`) — three replacements, in order: `[_-]+` → space,
/// lowerUpper → split, then upper-case the first character.
///
/// Written as two character passes rather than two `Regex`es: both patterns are trivial, and a
/// `Regex::new` on a literal is a fallible construction this crate would have to `expect` away
/// (`unwrap_used`/`expect_used` are denied). One `[_-]+` run collapses to a single space, which is
/// what the `+` in the pattern means.
#[must_use]
pub fn humanize_name(name: &str) -> String {
    let mut spaced = String::with_capacity(name.len());
    let mut in_separator = false;
    for ch in name.chars() {
        if ch == '_' || ch == '-' {
            if !in_separator {
                spaced.push(' ');
                in_separator = true;
            }
        } else {
            in_separator = false;
            spaced.push(ch);
        }
    }

    let mut split = String::with_capacity(spaced.len());
    let mut previous: Option<char> = None;
    for ch in spaced.chars() {
        if ch.is_uppercase() && previous.is_some_and(char::is_lowercase) {
            split.push(' ');
        }
        split.push(ch);
        previous = Some(ch);
    }

    let mut chars = split.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `Object.entries(params.requestedSchema.properties)` — the ONE ordered read
/// (`elicitation-handler.ts:48`, `:198`).
///
/// [`ElicitationSchema::properties`] is a `BTreeMap`, which is LEXICOGRAPHIC — iterating it directly
/// is the silent bug MCP-462 names. `property_order` is the wire order, filled from the `IndexMap`
/// the wire type deserialises through; it is `None` only for a schema this process constructed
/// itself, where the `BTreeMap` order is the only order there ever was.
#[must_use]
pub fn ordered_properties(schema: &ElicitationSchema) -> Vec<(&str, &PrimitiveSchemaDefinition)> {
    match schema.property_order.as_ref() {
        Some(order) => order
            .iter()
            .filter_map(|name| schema.properties.get_key_value(name.as_str()))
            .map(|(name, definition)| (name.as_str(), definition))
            .collect(),
        None => schema
            .properties
            .iter()
            .map(|(name, definition)| (name.as_str(), definition))
            .collect(),
    }
}

/// The `title` a property carries, across every arm of rmcp's closed enum.
#[must_use]
pub fn title_of(definition: &PrimitiveSchemaDefinition) -> Option<String> {
    use rmcp::model::{EnumSchema, MultiSelectEnumSchema, SingleSelectEnumSchema};
    let title = match definition {
        PrimitiveSchemaDefinition::String(schema) => schema.title.as_deref(),
        PrimitiveSchemaDefinition::Number(schema) => schema.title.as_deref(),
        PrimitiveSchemaDefinition::Integer(schema) => schema.title.as_deref(),
        PrimitiveSchemaDefinition::Boolean(schema) => schema.title.as_deref(),
        PrimitiveSchemaDefinition::Enum(EnumSchema::Legacy(schema)) => schema.title.as_deref(),
        PrimitiveSchemaDefinition::Enum(EnumSchema::Single(SingleSelectEnumSchema::Untitled(
            schema,
        ))) => schema.title.as_deref(),
        PrimitiveSchemaDefinition::Enum(EnumSchema::Single(SingleSelectEnumSchema::Titled(
            schema,
        ))) => schema.title.as_deref(),
        PrimitiveSchemaDefinition::Enum(EnumSchema::Multi(MultiSelectEnumSchema::Untitled(
            schema,
        ))) => schema.title.as_deref(),
        PrimitiveSchemaDefinition::Enum(EnumSchema::Multi(MultiSelectEnumSchema::Titled(
            schema,
        ))) => schema.title.as_deref(),
        // A variant rmcp added after this was written: no title we can read.
        _ => None,
    };
    title.map(str::to_string)
}

/// The label a field is shown under: its `title`, else its humanized name.
fn label_of(name: &str, definition: &PrimitiveSchemaDefinition) -> String {
    title_of(definition).unwrap_or_else(|| humanize_name(name))
}

/// `collectField` (`elicitation-handler.ts:114-190`) — one dialog per property shape.
///
/// rmcp's closed enum turns upstream's schema sniffing into arms. See the module doc for the one
/// delta: upstream's "any other type" arm cannot be reached here.
async fn collect_field(
    dialog: &McpDialog,
    name: &str,
    definition: &PrimitiveSchemaDefinition,
    current: ElicitationValue,
) -> FieldOutcome {
    use rmcp::model::{EnumSchema, MultiSelectEnumSchema, SingleSelectEnumSchema};

    let label = label_of(name, definition);
    let description = description_of(definition);
    let prompt = match description {
        Some(text) if !text.is_empty() => format!("{label}\n\n{text}"),
        _ => label.clone(),
    };

    match definition {
        // `type === "string" && "oneOf" in schema` — titled single select.
        PrimitiveSchemaDefinition::Enum(EnumSchema::Single(SingleSelectEnumSchema::Titled(
            schema,
        ))) => {
            let values: Vec<String> = schema
                .one_of
                .iter()
                .map(|item| item.const_.clone())
                .collect();
            let labels: Vec<String> = schema
                .one_of
                .iter()
                .map(|item| format_choice(&item.const_, Some(&item.title)))
                .collect();
            select_one(dialog, &prompt, &values, &labels).await
        }
        // `enumNames` — the legacy pairing.
        PrimitiveSchemaDefinition::Enum(EnumSchema::Legacy(schema)) => {
            let labels: Vec<String> = schema
                .enum_
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    format_choice(
                        value,
                        schema
                            .enum_names
                            .as_ref()
                            .and_then(|names| names.get(index))
                            .map(String::as_str),
                    )
                })
                .collect();
            select_one(dialog, &prompt, &schema.enum_, &labels).await
        }
        // `type === "string" && "enum" in schema` — untitled single select.
        PrimitiveSchemaDefinition::Enum(EnumSchema::Single(SingleSelectEnumSchema::Untitled(
            schema,
        ))) => {
            let labels = schema.enum_.clone();
            select_one(dialog, &prompt, &schema.enum_, &labels).await
        }
        // `type === "array"` — multi select, collected one pick at a time with a Done action.
        PrimitiveSchemaDefinition::Enum(EnumSchema::Multi(multi)) => {
            let (values, labels) = match multi {
                MultiSelectEnumSchema::Untitled(schema) => {
                    let values = schema.items.enum_.clone();
                    let labels = values.clone();
                    (values, labels)
                }
                MultiSelectEnumSchema::Titled(schema) => {
                    let values: Vec<String> = schema
                        .items
                        .any_of
                        .iter()
                        .map(|item| item.const_.clone())
                        .collect();
                    let labels: Vec<String> = schema
                        .items
                        .any_of
                        .iter()
                        .map(|item| format_choice(&item.const_, Some(&item.title)))
                        .collect();
                    (values, labels)
                }
                // Unknown multi-select shape: no choices to offer, so the field drops.
                _ => (Vec::new(), Vec::new()),
            };
            if values.is_empty() {
                return FieldOutcome::Collected(None);
            }
            select_many(dialog, &prompt, &values, &labels, current).await
        }
        // `type === "boolean"`.
        PrimitiveSchemaDefinition::Boolean(_) => {
            let values = vec!["true".to_string(), "false".to_string()];
            let labels = vec!["Yes".to_string(), "No".to_string()];
            match select_one(dialog, &prompt, &values, &labels).await {
                FieldOutcome::Collected(Some(FieldValue::Text(text))) => {
                    FieldOutcome::Collected(Some(FieldValue::Bool(text == "true")))
                }
                other => other,
            }
        }
        // Everything else is the free-text `input` arm.
        PrimitiveSchemaDefinition::String(_)
        | PrimitiveSchemaDefinition::Number(_)
        | PrimitiveSchemaDefinition::Integer(_) => {
            let placeholder = current.as_ref().map(render_value);
            match dialog.input(&prompt, placeholder.as_deref()).await {
                None => FieldOutcome::Cancelled,
                // `value === "" ? undefined : value` — a blank submission is `undefined`, which is
                // what makes an optional field skippable and a required one re-prompt.
                Some(text) if text.is_empty() => FieldOutcome::Collected(None),
                Some(text) => FieldOutcome::Collected(Some(FieldValue::Text(text))),
            }
        }
        // `default:` in upstream's switch — the field is dropped, not asked and not errored.
        _ => FieldOutcome::Collected(None),
    }
}

/// The description a property carries, across every arm.
fn description_of(definition: &PrimitiveSchemaDefinition) -> Option<String> {
    use rmcp::model::{EnumSchema, MultiSelectEnumSchema, SingleSelectEnumSchema};
    let description = match definition {
        PrimitiveSchemaDefinition::String(schema) => schema.description.as_deref(),
        PrimitiveSchemaDefinition::Number(schema) => schema.description.as_deref(),
        PrimitiveSchemaDefinition::Integer(schema) => schema.description.as_deref(),
        PrimitiveSchemaDefinition::Boolean(schema) => schema.description.as_deref(),
        PrimitiveSchemaDefinition::Enum(EnumSchema::Legacy(schema)) => {
            schema.description.as_deref()
        }
        PrimitiveSchemaDefinition::Enum(EnumSchema::Single(SingleSelectEnumSchema::Untitled(
            s,
        ))) => s.description.as_deref(),
        PrimitiveSchemaDefinition::Enum(EnumSchema::Single(SingleSelectEnumSchema::Titled(s))) => {
            s.description.as_deref()
        }
        PrimitiveSchemaDefinition::Enum(EnumSchema::Multi(MultiSelectEnumSchema::Untitled(s))) => {
            s.description.as_deref()
        }
        PrimitiveSchemaDefinition::Enum(EnumSchema::Multi(MultiSelectEnumSchema::Titled(s))) => {
            s.description.as_deref()
        }
        _ => None,
    };
    description.map(str::to_string)
}

/// How a collected value is shown back — in a placeholder or a review row.
fn render_value(value: &FieldValue) -> String {
    match value {
        FieldValue::Text(text) => text.clone(),
        FieldValue::Number(number) => trim_number(*number),
        FieldValue::Bool(flag) => flag.to_string(),
        FieldValue::List(items) => items.join(", "),
    }
}

/// One pick from a labelled list. Labels are uniquified because `select` returns the STRING.
async fn select_one(
    dialog: &McpDialog,
    prompt: &str,
    values: &[String],
    labels: &[String],
) -> FieldOutcome {
    let unique = unique_labels(labels);
    let refs: Vec<&str> = unique.iter().map(String::as_str).collect();
    let Some(chosen) = dialog.select(prompt, &refs).await else {
        return FieldOutcome::Cancelled;
    };
    match unique.iter().position(|label| *label == chosen) {
        Some(index) => FieldOutcome::Collected(values.get(index).cloned().map(FieldValue::Text)),
        // A selection that matches no label is a dismissal, not a silent empty pick.
        None => FieldOutcome::Cancelled,
    }
}

/// The array arm: pick repeatedly until the Done action, which is uniquified BESIDE the choices.
async fn select_many(
    dialog: &McpDialog,
    prompt: &str,
    values: &[String],
    labels: &[String],
    current: ElicitationValue,
) -> FieldOutcome {
    let mut picked: Vec<String> = match current {
        Some(FieldValue::List(items)) => items,
        _ => Vec::new(),
    };
    loop {
        let unique = unique_labels(labels);
        let done = unique_action("Done", &unique);
        let mut menu: Vec<String> = unique.clone();
        menu.push(done.clone());
        let refs: Vec<&str> = menu.iter().map(String::as_str).collect();
        let heading = if picked.is_empty() {
            prompt.to_string()
        } else {
            format!("{prompt}\n\nSelected: {}", picked.join(", "))
        };
        let Some(chosen) = dialog.select(&heading, &refs).await else {
            return FieldOutcome::Cancelled;
        };
        if chosen == done {
            return FieldOutcome::Collected(if picked.is_empty() {
                None
            } else {
                Some(FieldValue::List(picked))
            });
        }
        let Some(index) = unique.iter().position(|label| *label == chosen) else {
            return FieldOutcome::Cancelled;
        };
        let Some(value) = values.get(index) else {
            return FieldOutcome::Cancelled;
        };
        // Toggle: picking a selected value removes it, so the loop is usable as an editor.
        if let Some(existing) = picked.iter().position(|item| item == value) {
            picked.remove(existing);
        } else {
            picked.push(value.clone());
        }
    }
}

/// `coerceAndValidateFormValues` (`elicitation-handler.ts:196-265`).
///
/// Coercion order is [`ordered_properties`]'s, so the FIRST failing field in wire order is the one
/// reported — the same field the user was asked for first.
///
/// # Errors
///
/// The 13 templates above for a value that will not coerce, and
/// [`invalid_elicitation_response`] for one that coerces but fails the schema.
pub fn coerce_and_validate(
    options: &ElicitationOptions,
    schema: &ElicitationSchema,
    values: &IndexMap<String, ElicitationValue>,
) -> Result<Value, ErrorData> {
    let mut output = Map::new();
    for (name, definition) in ordered_properties(schema) {
        let label = label_of(name, definition);
        let required = schema
            .required
            .as_ref()
            .is_some_and(|names| names.iter().any(|entry| entry == name));
        let value = values.get(name).cloned().flatten();

        let Some(value) = value else {
            // `if (value === undefined) { if (required) throw …; continue; }`
            if required {
                return Err(internal_msg(&required_message(&label)));
            }
            continue;
        };

        match definition {
            PrimitiveSchemaDefinition::Number(number) => {
                let parsed = coerce_number(&value);
                if !parsed.is_finite() {
                    return Err(internal_msg(&must_be_number_message(&label)));
                }
                check_bounds(&label, parsed, number.minimum, number.maximum)?;
                output.insert(name.to_string(), json!(parsed));
            }
            PrimitiveSchemaDefinition::Integer(integer) => {
                let parsed = coerce_number(&value);
                if !parsed.is_finite() {
                    return Err(internal_msg(&must_be_number_message(&label)));
                }
                if parsed.fract() != 0.0 {
                    return Err(internal_msg(&must_be_integer_message(&label)));
                }
                check_bounds(
                    &label,
                    parsed,
                    integer.minimum.map(|min| min as f64),
                    integer.maximum.map(|max| max as f64),
                )?;
                #[allow(clippy::cast_possible_truncation)]
                output.insert(name.to_string(), json!(parsed as i64));
            }
            PrimitiveSchemaDefinition::Boolean(_) => {
                output.insert(name.to_string(), json!(js_bool(&Some(value))));
            }
            PrimitiveSchemaDefinition::Enum(rmcp::model::EnumSchema::Multi(_)) => {
                let items = match value {
                    FieldValue::List(items) => items,
                    other => vec![render_value(&other)],
                };
                output.insert(name.to_string(), json!(items));
            }
            PrimitiveSchemaDefinition::String(string) => {
                let text = render_value(&value);
                let length = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
                if let Some(min) = string.min_length
                    && length < min
                {
                    return Err(internal_msg(&min_length_message(&label, min)));
                }
                if let Some(max) = string.max_length
                    && length > max
                {
                    return Err(internal_msg(&max_length_message(&label, max)));
                }
                output.insert(name.to_string(), json!(text));
            }
            PrimitiveSchemaDefinition::Enum(_) => {
                output.insert(name.to_string(), json!(render_value(&value)));
            }
            // Dropped, exactly as `collect_field` dropped it. A `required` field left out here is
            // caught by the schema assertion below, which is upstream's outcome too.
            _ => {}
        }
    }

    let rendered = Value::Object(output);
    // `new AjvJsonSchemaValidator().getValidator(requestedSchema)(output)` (`:260-264`).
    let as_value = serde_json::to_value(schema)
        .map_err(|error| internal_msg(&invalid_elicitation_response(&error.to_string())))?;
    let validator = options
        .validators
        .get_or_compile(&as_value)
        .map_err(|error| internal_msg(&invalid_elicitation_response(&error.to_string())))?;
    if let Err(error) = validator.validate(&rendered) {
        return Err(internal_msg(&invalid_elicitation_response(
            &error.to_string(),
        )));
    }
    Ok(rendered)
}

/// `Number(value)` over an already-typed field.
fn coerce_number(value: &FieldValue) -> f64 {
    match value {
        FieldValue::Number(number) => *number,
        FieldValue::Text(text) => js_number(text),
        FieldValue::Bool(flag) => f64::from(u8::from(*flag)),
        FieldValue::List(_) => f64::NAN,
    }
}

fn check_bounds(
    label: &str,
    value: f64,
    minimum: Option<f64>,
    maximum: Option<f64>,
) -> Result<(), ErrorData> {
    if let Some(min) = minimum
        && value < min
    {
        return Err(internal_msg(&minimum_message(label, min)));
    }
    if let Some(max) = maximum
        && value > max
    {
        return Err(internal_msg(&maximum_message(label, max)));
    }
    Ok(())
}

/// `collectValidField` (`elicitation-handler.ts:86-112`) — the unbounded re-prompt loop.
///
/// Its synthetic schema **copies the whole schema and replaces only `properties`/`required`**, so a
/// sibling constraint that lives on the parent survives.
async fn collect_valid_field(
    options: &ElicitationOptions,
    dialog: &McpDialog,
    schema: &ElicitationSchema,
    name: &str,
    definition: &PrimitiveSchemaDefinition,
    mut current: ElicitationValue,
) -> Result<FieldOutcome, ErrorData> {
    let required = schema
        .required
        .as_ref()
        .is_some_and(|names| names.iter().any(|entry| entry == name));
    let mut single = schema.clone();
    single.properties = std::iter::once((name.to_string(), definition.clone())).collect();
    single.property_order = Some(vec![name.to_string()]);
    single.required = required.then(|| vec![name.to_string()]);

    loop {
        let FieldOutcome::Collected(value) =
            collect_field(dialog, name, definition, current.clone()).await
        else {
            return Ok(FieldOutcome::Cancelled);
        };
        let mut one = IndexMap::new();
        one.insert(name.to_string(), value.clone());
        match coerce_and_validate(options, &single, &one) {
            Ok(_) => return Ok(FieldOutcome::Collected(value)),
            Err(error) => {
                dialog.notify(&error.message, cyrup_ext::NotifyKind::Error);
                current = value;
            }
        }
    }
}

/// The review screen (`elicitation-handler.ts:56-66`).
fn format_review(
    server: &str,
    properties: &[(&str, &PrimitiveSchemaDefinition)],
    content: &Value,
) -> String {
    let mut lines = vec![format!("MCP Input Review\nServer: {server}"), String::new()];
    for (name, definition) in properties {
        let label = label_of(name, definition);
        let shown = content
            .get(*name)
            .map(render_json)
            .unwrap_or_else(|| "(omitted)".to_string());
        lines.push(format!("{label}: {shown}"));
    }
    lines.join("\n")
}

fn render_json(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(render_json).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// `handleFormElicitation` (`elicitation-handler.ts:44-84`).
///
/// **Two behaviours a Rust engineer will want to "fix" and must not:** the review loop's
/// [`coerce_and_validate`] call is *not* caught — a cross-field failure escapes as a JSON-RPC error,
/// not as an `ElicitResult`; and duplicate edit labels resolve first-wins via `position`, so two
/// identically-titled fields make the second unreachable from the picker. Both are upstream's.
///
/// # Errors
///
/// A cross-field coercion failure on the review pass, propagated as `-32603`.
pub async fn handle_form_elicitation(
    options: &ElicitationOptions,
    server: &str,
    message: &str,
    schema: &ElicitationSchema,
) -> Result<ElicitResult, ErrorData> {
    let Some(dialog) = options.session.dialog() else {
        // No UI to ask through. rmcp's own default for an unwired handler is Decline, and a client
        // that cannot ask must not accept on the user's behalf.
        return Ok(ElicitResult::new(ElicitationAction::Decline));
    };
    let properties = ordered_properties(schema);

    let gate = format!("MCP Input Request\nServer: {server}\n\n{message}");
    match dialog.select(&gate, &[CONTINUE, DECLINE]).await.as_deref() {
        None => return Ok(ElicitResult::new(ElicitationAction::Cancel)),
        Some(DECLINE) => return Ok(ElicitResult::new(ElicitationAction::Decline)),
        Some(_) => {}
    }
    // `if (properties.length === 0) return { action: "accept", content: {} }` — BEFORE any review
    // screen, and it is an empty object, not `None`.
    if properties.is_empty() {
        return Ok(ElicitResult::new(ElicitationAction::Accept).with_content(json!({})));
    }

    let mut values: IndexMap<String, ElicitationValue> = IndexMap::new();
    for (name, definition) in &properties {
        match collect_valid_field(options, &dialog, schema, name, definition, None).await? {
            FieldOutcome::Cancelled => return Ok(ElicitResult::new(ElicitationAction::Cancel)),
            FieldOutcome::Collected(value) => {
                values.insert((*name).to_string(), value);
            }
        }
    }

    loop {
        // NOT caught. A cross-field failure here is a JSON-RPC error, exactly as upstream's is.
        let content = coerce_and_validate(options, schema, &values)?;
        let review = format_review(server, &properties, &content);
        match dialog
            .select(&review, &[SUBMIT, EDIT, DECLINE])
            .await
            .as_deref()
        {
            None => return Ok(ElicitResult::new(ElicitationAction::Cancel)),
            Some(DECLINE) => return Ok(ElicitResult::new(ElicitationAction::Decline)),
            Some(SUBMIT) => {
                return Ok(ElicitResult::new(ElicitationAction::Accept).with_content(content));
            }
            Some(_) => {}
        }

        // The edit picker labels are deliberately NOT uniquified upstream (`:74`), and the lookup is
        // `indexOf`, i.e. first wins.
        let labels: Vec<String> = properties
            .iter()
            .map(|(name, definition)| format!("{} ({name})", label_of(name, definition)))
            .collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let Some(selected) = dialog.select(CHOOSE_A_FIELD, &refs).await else {
            return Ok(ElicitResult::new(ElicitationAction::Cancel));
        };
        // `if (!property) continue;` — a selection that matches no label re-runs the review loop.
        let Some(index) = labels.iter().position(|label| *label == selected) else {
            continue;
        };
        let Some((name, definition)) = properties.get(index) else {
            continue;
        };
        let current = values.get(*name).cloned().flatten();
        match collect_valid_field(options, &dialog, schema, name, definition, current).await? {
            FieldOutcome::Cancelled => return Ok(ElicitResult::new(ElicitationAction::Cancel)),
            FieldOutcome::Collected(value) => {
                values.insert((*name).to_string(), value);
            }
        }
    }
}

/// `handleUrlElicitation` (`elicitation-handler.ts:305-344`).
///
/// # Errors
///
/// Three `-32602`s — unsupported, unparseable, or non-HTTP(S) — and `-32603` if the blocking open
/// cannot be joined. Every other outcome is an `ElicitResult`.
pub async fn handle_url_elicitation(
    options: &ElicitationOptions,
    server: &str,
    message: &str,
    url: &str,
    elicitation_id: &str,
) -> Result<ElicitResult, ErrorData> {
    if !options.allow_url {
        return Err(ErrorData::invalid_params(URL_ELICITATION_UNSUPPORTED, None));
    }
    let Ok(parsed) = url::Url::parse(url) else {
        return Err(ErrorData::invalid_params(URL_ELICITATION_INVALID_URL, None));
    };
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(ErrorData::invalid_params(URL_ELICITATION_SCHEME, None));
    }
    let Some(dialog) = options.session.dialog() else {
        return Ok(ElicitResult::new(ElicitationAction::Decline));
    };

    // `Host:` is host+port — `Url::host_str` drops the port and `URL.host` in JS keeps it, so a
    // non-default port must be re-appended or the user is shown a different address than the one
    // that will open. `Full URL:` is the RAW input, never `parsed.as_str()`: `Url::parse`
    // normalises (trailing slash, percent-encoding, case), and the point of the line is to show
    // exactly what the server asked for.
    let host = match parsed.port() {
        Some(port) => format!("{}:{port}", parsed.host_str().unwrap_or_default()),
        None => parsed.host_str().unwrap_or_default().to_string(),
    };
    let prompt = [
        "MCP Browser Request",
        &format!("Server: {server}"),
        "",
        message,
        "",
        &format!("Host: {host}"),
        &format!("Full URL: {url}"),
        "",
        "Open this URL in your browser?",
    ]
    .join("\n");

    match dialog.select(&prompt, &[OPEN, DECLINE]).await.as_deref() {
        None => return Ok(ElicitResult::new(ElicitationAction::Cancel)),
        Some(DECLINE) => return Ok(ElicitResult::new(ElicitationAction::Decline)),
        Some(_) => {}
    }

    // `opener::open` is BLOCKING, and unlike the dialogs above it does no `block_in_place` of its
    // own, so it goes off the worker explicitly.
    let launcher = Arc::clone(&options.launcher);
    let target = url.to_string();
    let opened = tokio::task::spawn_blocking(move || launcher.open(&target))
        .await
        .map_err(|_| internal_msg(URL_ELICITATION_OPEN_FAILED))?;
    if let Err(error) = opened {
        dialog.notify(
            &could_not_open_message(&error.to_string()),
            cyrup_ext::NotifyKind::Error,
        );
        // CANCEL, not decline: the user said yes and the machine failed.
        return Ok(ElicitResult::new(ElicitationAction::Cancel));
    }

    // `options.onUrlAccepted?.(params.elicitationId)` — the registry write the completion notice's
    // dedupe reads.
    (options.on_url_accepted)(server, elicitation_id);
    dialog.notify(OPENED_BROWSER_NOTICE, cyrup_ext::NotifyKind::Info);
    Ok(ElicitResult::new(ElicitationAction::Accept))
}

/// `MCP-460`'s dispatch. rmcp's untagged wire enum already gives upstream's
/// *absent-or-unknown `mode` → form* for free, so this is one `match`.
///
/// # Errors
///
/// Whatever the chosen leg returns.
pub async fn handle_elicitation_request(
    options: &ElicitationOptions,
    server: &str,
    params: ElicitRequestParams,
) -> Result<ElicitResult, ErrorData> {
    match params {
        ElicitRequestParams::FormElicitationParams {
            message,
            requested_schema,
            ..
        } => handle_form_elicitation(options, server, &message, &requested_schema).await,
        ElicitRequestParams::UrlElicitationParams {
            message,
            url,
            elicitation_id,
            ..
        } => handle_url_elicitation(options, server, &message, &url, &elicitation_id).await,
        // A mode rmcp models and this port does not. Upstream's wire enum folds absent-or-unknown
        // into `form`, so reaching here means a genuinely new variant, not a malformed request.
        _ => Err(ErrorData::invalid_params(
            UNSUPPORTED_ELICITATION_MODE,
            None,
        )),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use rmcp::model::{ElicitationSchema, NumberSchema, StringSchema};

    fn options() -> ElicitationOptions {
        ElicitationOptions {
            allow_url: false,
            session: Arc::new(crate::runtime::SessionSlot::default()),
            launcher: Arc::new(crate::oauth::NoopLauncher),
            on_url_accepted: Arc::new(|_server, _id| {}),
            validators: Arc::new(crate::schema::ValidatorCache::default()),
        }
    }

    /// The bug MCP-462 names: `properties` is a `BTreeMap`, so iterating it is LEXICOGRAPHIC.
    /// `property_order` carries the wire order, and four user-visible orderings ride on it.
    #[test]
    fn ordered_properties_follows_the_wire_not_the_btreemap() {
        let mut schema = ElicitationSchema::new(std::collections::BTreeMap::new());
        for name in ["zebra", "apple", "middle"] {
            schema.properties.insert(
                name.to_string(),
                PrimitiveSchemaDefinition::String(StringSchema::new()),
            );
        }
        // Lexicographic would be apple, middle, zebra.
        schema.property_order = Some(vec![
            "zebra".to_string(),
            "apple".to_string(),
            "middle".to_string(),
        ]);
        let names: Vec<&str> = ordered_properties(&schema)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(names, vec!["zebra", "apple", "middle"]);

        // With no wire order the BTreeMap order is the only order there ever was.
        schema.property_order = None;
        let names: Vec<&str> = ordered_properties(&schema)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(names, vec!["apple", "middle", "zebra"]);
    }

    /// `Number(value)` — JavaScript's, not `str::parse::<f64>()`. Each of these differs.
    #[test]
    fn js_number_matches_javascript_not_rust_parse() {
        assert_eq!(js_number("0x1f"), 31.0, "hex — str::parse rejects this");
        assert_eq!(js_number("1e3"), 1000.0);
        assert_eq!(js_number(" 7 "), 7.0, "surrounding whitespace is trimmed");
        assert_eq!(js_number(""), 0.0, "empty is zero, not an error");
        assert!(js_number("7abc").is_nan());
        assert_eq!(js_number("Infinity"), f64::INFINITY);
        // Rust's parse accepts these spellings and JS's `Number()` does not.
        assert!(js_number("inf").is_nan(), "Rust accepts `inf`; JS does not");
        assert!(js_number("NaN").is_nan());
    }

    /// `typeof value === "boolean" ? value : value === "true"` — every other string is `false`,
    /// silently. `bool::from_str` would error on "yes" instead.
    #[test]
    fn js_bool_treats_every_other_string_as_false() {
        assert!(js_bool(&Some(FieldValue::Bool(true))));
        assert!(js_bool(&Some(FieldValue::Text("true".to_string()))));
        assert!(!js_bool(&Some(FieldValue::Text("yes".to_string()))));
        assert!(!js_bool(&Some(FieldValue::Text("TRUE".to_string()))));
        assert!(!js_bool(&None));
    }

    /// `formatChoice` — a title equal to its value is suppressed, not duplicated.
    #[test]
    fn a_title_equal_to_its_value_is_not_duplicated() {
        assert_eq!(format_choice("red", Some("Red")), "Red (red)");
        assert_eq!(format_choice("red", Some("red")), "red");
        assert_eq!(format_choice("red", None), "red");
    }

    /// `uniqueLabels` accumulates, so three identical labels get zero, one and two ellipses.
    /// Necessary because `select` returns the STRING: duplicates would be unselectable.
    #[test]
    fn duplicate_labels_are_disambiguated_by_accumulating_ellipses() {
        let labels = vec!["Same".to_string(), "Same".to_string(), "Same".to_string()];
        assert_eq!(unique_labels(&labels), vec!["Same", "Same…", "Same……"]);
    }

    /// `uniqueAction` tests against the choice LIST rather than a set.
    #[test]
    fn an_action_that_collides_with_a_choice_is_pushed_aside() {
        let choices = vec!["Done".to_string(), "Done…".to_string()];
        assert_eq!(unique_action("Done", &choices), "Done……");
        assert_eq!(unique_action("Other", &choices), "Other");
    }

    /// `humanizeName` — three replacements in order.
    #[test]
    fn humanize_name_applies_its_three_replacements_in_order() {
        assert_eq!(humanize_name("first_name"), "First name");
        assert_eq!(humanize_name("firstName"), "First Name");
        assert_eq!(humanize_name("api-key_id"), "Api key id");
        assert_eq!(humanize_name(""), "");
    }

    /// A missing REQUIRED field is the throw; a missing optional one is a skip, and the difference
    /// is visible in the output object.
    #[test]
    fn a_missing_required_field_throws_and_a_missing_optional_one_is_skipped() {
        let mut schema = ElicitationSchema::new(std::collections::BTreeMap::new());
        schema.properties.insert(
            "who".to_string(),
            PrimitiveSchemaDefinition::String(StringSchema::new()),
        );
        schema.property_order = Some(vec!["who".to_string()]);

        let mut values = IndexMap::new();
        values.insert("who".to_string(), None);

        // Optional: skipped, and the key is absent rather than null.
        let content = coerce_and_validate(&options(), &schema, &values).expect("optional skips");
        assert_eq!(content, json!({}));

        // Required: the throw, naming the humanized label.
        schema.required = Some(vec!["who".to_string()]);
        let error = coerce_and_validate(&options(), &schema, &values).expect_err("required throws");
        assert_eq!(error.message, required_message("Who"));
    }

    /// The number arm: coercion, then bounds, in that order.
    #[test]
    fn numbers_coerce_then_check_their_bounds() {
        let mut schema = ElicitationSchema::new(std::collections::BTreeMap::new());
        schema.properties.insert(
            "count".to_string(),
            PrimitiveSchemaDefinition::Number(NumberSchema::new().range(10.0, 20.0)),
        );
        schema.property_order = Some(vec!["count".to_string()]);

        let check = |text: &str| {
            let mut values = IndexMap::new();
            values.insert(
                "count".to_string(),
                Some(FieldValue::Text(text.to_string())),
            );
            coerce_and_validate(&options(), &schema, &values)
        };

        assert_eq!(check("15").expect("in range"), json!({"count": 15.0}));
        assert_eq!(
            check("abc").expect_err("not a number").message,
            must_be_number_message("Count")
        );
        assert_eq!(
            check("5").expect_err("below").message,
            minimum_message("Count", 10.0)
        );
        assert_eq!(
            check("25").expect_err("above").message,
            maximum_message("Count", 20.0)
        );
        // The limit is rendered as JS would print it — `10`, not `10.0`.
        assert!(minimum_message("Count", 10.0).ends_with("at least 10"));
    }

    /// The prefix is load-bearing even though the detail text is `jsonschema`'s, not ajv's.
    #[test]
    fn a_schema_failure_carries_the_byte_exact_prefix() {
        let mut schema = ElicitationSchema::new(std::collections::BTreeMap::new());
        schema.properties.insert(
            "email".to_string(),
            PrimitiveSchemaDefinition::String(StringSchema::email()),
        );
        schema.property_order = Some(vec!["email".to_string()]);
        schema.required = Some(vec!["email".to_string()]);

        let mut values = IndexMap::new();
        values.insert(
            "email".to_string(),
            Some(FieldValue::Text("nope".to_string())),
        );
        let error = coerce_and_validate(&options(), &schema, &values)
            .expect_err("format is an assertion, so this fails");
        assert!(
            error.message.starts_with("Invalid elicitation response: "),
            "got {}",
            error.message
        );
    }

    /// The three `-32602`s, and the fact that they precede any dialog.
    #[tokio::test]
    async fn the_url_leg_refuses_before_it_asks_anything() {
        let mut opts = options();
        // `allowUrl` false — refused outright.
        let error = handle_url_elicitation(&opts, "fixture", "m", "https://example.com", "id")
            .await
            .expect_err("unsupported");
        assert_eq!(error.message, URL_ELICITATION_UNSUPPORTED);

        opts.allow_url = true;
        let error = handle_url_elicitation(&opts, "fixture", "m", "not a url", "id")
            .await
            .expect_err("invalid");
        assert_eq!(error.message, URL_ELICITATION_INVALID_URL);

        let error = handle_url_elicitation(&opts, "fixture", "m", "file:///etc/passwd", "id")
            .await
            .expect_err("scheme");
        assert_eq!(error.message, URL_ELICITATION_SCHEME);

        // A well-formed URL with no dialog to ask through declines rather than accepting.
        let result = handle_url_elicitation(&opts, "fixture", "m", "https://example.com", "id")
            .await
            .expect("a headless generation answers rather than erroring");
        assert_eq!(result.action, ElicitationAction::Decline);
    }

    /// A headless generation must decline the form leg, never accept on the user's behalf.
    #[tokio::test]
    async fn a_headless_generation_declines_the_form_leg() {
        let schema = ElicitationSchema::new(std::collections::BTreeMap::new());
        let result = handle_form_elicitation(&options(), "fixture", "message", &schema)
            .await
            .expect("no dialog is an answer, not an error");
        assert_eq!(result.action, ElicitationAction::Decline);
    }
}
