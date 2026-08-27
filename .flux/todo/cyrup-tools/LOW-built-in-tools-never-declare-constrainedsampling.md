---
title: Built-in tools never declare constrainedSampling
priority: LOW
tool: read/bash/edit/write
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: done
updated: 2026-08-27
---

# Built-in tools never declare `constrainedSampling`

> **Merged finding.** Five separate findings (bash, edit, read, write, and one cross-cutting)
> describe the *same* missing capability. They are one task: the opt-in is declared per tool,
> so the fix is a single mechanism applied at four call sites.

---

## 1. Core objective

When `CYRUP_EXPERIMENTAL=1` (or `PI_EXPERIMENTAL=1`), the four coding built-ins — `read`, `bash`,
`edit`, `write` — must declare `{"type":"json_schema","strict":"prefer"}` on their tool definition,
and the request cyrup sends to a strict-capable model must carry `strict: true` **together with the
strict-converted schema**, exactly as pi does.

The second half of that sentence is the part the original finding missed, and it is not optional —
see §3.

---

## 2. Verified state of the world

### 2.1 The adversary was right: the pipeline already exists

Everything below is live in Rust today. **Do not rebuild any of it.**

| Layer | Location |
| --- | --- |
| Wire types (`ConstrainedSampling` / `ConstrainedSamplingConfig::{JsonSchema,Grammar}` / `StrictSampling`) | [cyrup-core/src/constrained_sampling.rs](../../../crates/cyrup-core/src/constrained_sampling.rs) |
| Trait seam `Tool::constrained_sampling()` (default `None`) | [cyrup-core/src/tool.rs:156](../../../crates/cyrup-core/src/tool.rs) |
| Agent-loop forward onto `ToolDef` | [cyrup-agent/src/agent/run/stream.rs:94](../../../crates/cyrup-agent/src/agent/run/stream.rs) |
| Provider-facing `ToolDef.constrained_sampling` | [cyrup-provider/src/context.rs:31](../../../crates/cyrup-provider/src/context.rs) |
| Resolvers | [cyrup-provider/src/utils/constrained_sampling.rs:209](../../../crates/cyrup-provider/src/utils/constrained_sampling.rs) |
| Adapter emission of `strict: true` | [anthropic_messages.rs:1252](../../../crates/cyrup-provider/src/api/anthropic_messages.rs), [openai_completions.rs:845](../../../crates/cyrup-provider/src/api/openai_completions.rs), [openai_responses.rs:970](../../../crates/cyrup-provider/src/api/openai_responses.rs), [mistral_conversations.rs:471](../../../crates/cyrup-provider/src/api/mistral_conversations.rs), [bedrock_converse_stream/convert.rs:284](../../../crates/cyrup-provider/src/api/bedrock_converse_stream/convert.rs), [google_generative_ai.rs:903](../../../crates/cyrup-provider/src/api/google_generative_ai.rs) |
| Extension / WASM / SDK opt-in | [cyrup-ext-sdk/src/descriptor.rs:163](../../../crates/cyrup-ext-sdk/src/descriptor.rs), [cyrup-ext/src/host/live.rs:1960](../../../crates/cyrup-ext/src/host/live.rs), [cyrup-ext/src/wrapper.rs:123](../../../crates/cyrup-ext/src/wrapper.rs), [cyrup-ext/src/registry.rs:54](../../../crates/cyrup-ext/src/registry.rs) |
| Experimental-flag reads (two existing copies) | [cyrup/src/startup.rs:77](../../../crates/cyrup/src/startup.rs), [cyrup-tui/src/status.rs:474](../../../crates/cyrup-tui/src/status.rs) |

`rg -c constrained_sampling crates/cyrup-tools/src` returns **zero hits**. The four `impl Tool for`
blocks — [read.rs:58](../../../crates/cyrup-tools/src/tools/read.rs),
[bash.rs:84](../../../crates/cyrup-tools/src/tools/bash.rs),
[edit.rs:128](../../../crates/cyrup-tools/src/tools/edit.rs),
[write.rs:55](../../../crates/cyrup-tools/src/tools/write.rs) — override
`name`/`label`/`parameters`/`description`/`prompt_snippet`/`prompt_guidelines` (plus `render_kind`
and `prepare_arguments` on `edit`) and never `constrained_sampling`.

### 2.2 Citation corrections against the real source

The vendored pi checkout at `tmp/pi` is **v0.84.2-136-ge8682309** (`package.json` version `0.84.3`),
not v0.83.0. Corrections to the claims in the original finding:

* `constrainedSampling` on `ToolDefinition` is [extensions/types.ts:465](../../../tmp/pi/packages/coding-agent/src/core/extensions/types.ts), **not** `:463`.
* `Tool.constrainedSampling` in the ai package is [ai/src/types.ts:518](../../../tmp/pi/packages/ai/src/types.ts), **not** `:484`.
* The four built-in declaration lines are correct as recorded and were re-verified verbatim:
  [read.ts:222](../../../tmp/pi/packages/coding-agent/src/core/tools/read.ts),
  [bash.ts:354](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts),
  [edit.ts:329](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts),
  [write.ts:200](../../../tmp/pi/packages/coding-agent/src/core/tools/write.ts),
  plus [server/create-harness.ts:34](../../../tmp/pi/packages/coding-agent/src/server/create-harness.ts).
* [core/experimental.ts](../../../tmp/pi/packages/coding-agent/src/core/experimental.ts) is nine
  lines: `PREFER_STRICT_TOOL_SAMPLING` at `:1`, `areExperimentalFeaturesEnabled` at `:3-5`,
  `getExperimentalToolSampling` at `:7-9`. Correct as recorded.
* **The stale doc-comment is stale for a specific reason.** The claim at
  [cyrup-core/src/tool.rs:151-155](../../../crates/cyrup-core/src/tool.rs) and in the module header
  of [cyrup-core/src/constrained_sampling.rs:22-27](../../../crates/cyrup-core/src/constrained_sampling.rs)
  ("No built-in tool declares it — upstream or here") was **true at v0.83.0**. `git log -S` in the
  vendored checkout pins the change to pi commit `7915cdac` — *"feat(ai): add strict tool schema
  conversion"*, 2026-08-11 — first tagged in **v0.84.2**. `git show v0.84.1:…/read.ts | grep -c
  constrainedSampling` → `0`; at `v0.84.2` → `1`. So this is version-bump drift, not an incorrect
  original observation.

---

## 3. What the finding got wrong: this is not four one-liners

Pi commit `7915cdac` added the four declarations **and** the machinery that makes them safe, in one
commit:

```
packages/ai/src/api/constrained-sampling.ts                    | 137 ++++++++++++-
packages/ai/src/utils/validation.ts                            |  33 +++++
packages/ai/src/api/anthropic-messages.ts                      |   7 +-
packages/ai/src/api/bedrock-converse-stream.ts                 |   7 +-
packages/ai/src/api/google-generative-ai.ts                    |   8 +-
packages/ai/src/api/google-shared.ts                           |  21 ++--
packages/ai/src/api/mistral-conversations.ts                   |   4 +-
packages/ai/src/api/openai-completions.ts                      |   3 +-
packages/ai/src/api/openai-responses-shared.ts                 |   6 +-
packages/coding-agent/src/core/experimental.ts                 |   6 +
packages/coding-agent/src/core/tools/{read,bash,edit,write}.ts |   2 each
```

cyrup's port of `constrained-sampling.ts` is explicitly documented as
`@v0.83.0 … byte-identical at v0.84.1`
([utils/constrained_sampling.rs:1-3](../../../crates/cyrup-provider/src/utils/constrained_sampling.rs)) —
i.e. **pre-`7915cdac`**. `rg 'make_strict_json_schema|get_json_schema_tool_parameters|normalize_optional_nulls' crates/`
returns nothing.

Declaring the opt-in without porting that machinery is **actively harmful**, in two concrete ways:

**(a) Strict-capable routes would be sent an invalid schema.**
[compat.rs:660](../../../crates/cyrup-provider/src/api/compat.rs) sets
`supports_strict_mode: !is_moonshot && !is_together && !is_cloudflare_ai_gateway && !is_nvidia` —
i.e. **true by default** for OpenAI-completions routes; Mistral passes `true` unconditionally
([mistral_conversations.rs:471](../../../crates/cyrup-provider/src/api/mistral_conversations.rs));
Codex defaults `supports_strict_mode` to `true`
([openai_codex_responses.rs:733-739](../../../crates/cyrup-provider/src/api/openai_codex_responses.rs)).
Every adapter today emits `"parameters": t.parameters.clone()` — the raw schema. `read`'s schema has
`required: ["path"]` with optional `offset`/`limit` and no `additionalProperties`
([read.rs:38-46](../../../crates/cyrup-tools/src/tools/read.rs)). Strict function calling requires
every key of `properties` to appear in `required` and `additionalProperties: false`. The request is
rejected before the model ever runs.

**(b) Even if accepted, optional arguments would be silently corrupted.**
A strict-converted schema forces optional properties to be present-and-nullable, so the model emits
`{"command":"ls","timeout":null}` / `{"path":"x","offset":null,"limit":null}`. cyrup's coercion pass
maps `null` → `0` for numbers
([validate.rs:321](../../../crates/cyrup-provider/src/validate.rs) — pi's own falsy table). Result:
`read` runs with `limit = 0` (reads nothing) and `bash` runs with `timeout = 0`. Pi fixed exactly
this by adding `normalizeOptionalNulls`, which **deletes** the null before validation
([validation.ts:240-269, called at :319](../../../tmp/pi/packages/ai/src/utils/validation.ts)).

**Therefore the required path is: land §4 (the machinery) and §5 (the declarations) together.**

---

## 4. Required change — the strict-schema machinery

### 4.1 `crates/cyrup-provider/src/utils/constrained_sampling.rs` — the conversion

Port [constrained-sampling.ts:10-142](../../../tmp/pi/packages/ai/src/api/constrained-sampling.ts).
Add above the existing `GrammarConstrainedSampling` block (`json!`, `Value` and `Map` are already
imported at the top of the file; nothing else is needed):

```rust
/// Pi `UNSUPPORTED_STRICT_SCHEMA_KEYS` (`constrained-sampling.ts:12-29`). A schema carrying any of
/// these cannot be expressed in the strict subset the providers constrain against.
const UNSUPPORTED_STRICT_SCHEMA_KEYS: [&str; 16] = [
    "$ref", "$defs", "definitions", "allOf", "oneOf", "patternProperties", "dependentSchemas",
    "dependencies", "unevaluatedProperties", "propertyNames", "contains", "prefixItems", "not",
    "if", "then", "else",
];

/// Pi `isStructuredSchema` (`constrained-sampling.ts:35-44`).
fn is_structured_schema(schema: &Value) -> bool {
    let Some(o) = schema.as_object() else { return false };
    let types: Vec<&str> = match o.get("type") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    types.contains(&"object")
        || types.contains(&"array")
        || o.contains_key("properties")
        || o.contains_key("items")
}

/// Pi `schemaAllowsNull` (`constrained-sampling.ts:46-51`).
fn schema_allows_null(schema: &Value) -> bool {
    let Some(o) = schema.as_object() else { return false };
    let ty_is_null = match o.get("type") {
        Some(Value::String(s)) => s == "null",
        Some(Value::Array(a)) => a.iter().any(|v| v.as_str() == Some("null")),
        _ => false,
    };
    if ty_is_null {
        return true;
    }
    if o.get("const") == Some(&Value::Null) {
        return true;
    }
    if o.get("enum").and_then(Value::as_array).is_some_and(|a| a.contains(&Value::Null)) {
        return true;
    }
    o.get("anyOf")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(schema_allows_null))
}

/// Pi `makeJsonSchemaNodeStrict` (`constrained-sampling.ts:53-115`) — mutates `schema` in place.
/// The error strings are pi's `UnsupportedStrictJsonSchemaError` messages verbatim; they reach the
/// model through [`ConstrainedSamplingError`] exactly as pi's thrown text does.
fn make_json_schema_node_strict(schema: &mut Value) -> Result<()> {
    let Some(o) = schema.as_object_mut() else {
        return err("boolean schemas are unsupported");
    };
    for key in UNSUPPORTED_STRICT_SCHEMA_KEYS {
        if o.contains_key(key) {
            return err(format!("{key} schemas are unsupported"));
        }
    }

    if let Some(any_of) = o.get_mut("anyOf") {
        let Some(variants) = any_of.as_array_mut().filter(|a| !a.is_empty()) else {
            return err("anyOf must contain at least one schema");
        };
        for variant in variants.iter_mut() {
            if is_structured_schema(variant) {
                return err("object and array unions are unsupported");
            }
            make_json_schema_node_strict(variant)?;
        }
    }

    if let Some(items) = o.get_mut("items") {
        if items.is_array() {
            return err("tuple schemas are unsupported");
        }
        make_json_schema_node_strict(items)?;
    }

    let is_object_schema = o.get("type") == Some(&Value::String("object".into()));
    if o.contains_key("properties") && !is_object_schema {
        return err("properties require type object");
    }
    if !is_object_schema {
        return Ok(());
    }
    match o.get("additionalProperties") {
        None | Some(Value::Bool(false)) => {}
        Some(_) => return err("schema-valued or true additionalProperties is unsupported"),
    }
    if o.get("properties").is_some_and(|p| !p.is_object()) {
        return err("object properties must be a schema map");
    }
    if o.get("required")
        .is_some_and(|r| r.as_array().is_none_or(|a| a.iter().any(|k| !k.is_string())))
    {
        return err("object required must be a string array");
    }

    let required: Vec<String> = o
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let property_names: Vec<String> = o
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    if required.iter().any(|k| !property_names.contains(k)) {
        return err("required contains an unknown property");
    }

    if let Some(properties) = o.get_mut("properties").and_then(Value::as_object_mut) {
        for (key, property) in properties.iter_mut() {
            make_json_schema_node_strict(property)?;
            // Pi wraps every non-required property in `anyOf: [property, {type:"null"}]`
            // (`constrained-sampling.ts:110-112`) so the constrainer can require EVERY key while
            // still letting the model decline one by emitting `null`.
            if !required.contains(key) && !schema_allows_null(property) {
                let inner = property.take();
                *property = json!({ "anyOf": [inner, { "type": "null" }] });
            }
        }
    }
    o.insert("required".to_string(), json!(property_names));
    o.insert("additionalProperties".to_string(), Value::Bool(false));
    Ok(())
}

/// Pi `makeStrictJsonSchema` (`constrained-sampling.ts:117-127`). Clones first — the caller's schema
/// is never mutated (upstream `structuredClone`).
pub fn make_strict_json_schema(schema: &Value) -> Result<Value> {
    let mut cloned = schema.clone();
    if !cloned.is_object() {
        return err("root schema must have type object");
    }
    make_json_schema_node_strict(&mut cloned)?;
    if cloned.get("type") != Some(&Value::String("object".into())) {
        return err("root schema must have type object");
    }
    Ok(cloned)
}

/// Pi `getJsonSchemaToolParameters` (`constrained-sampling.ts:129-131`) — the schema an adapter must
/// serialize. Upstream does NOT catch the throw here: a `strict === true` that came from a caller
/// DEFAULT rather than from [`resolve_json_schema_strict_sampling`] surfaces the raw message, so
/// this returns `Err` carrying that same bare text.
pub fn json_schema_tool_parameters(tool: &ToolDef, strict: bool) -> Result<Value> {
    if strict {
        make_strict_json_schema(&tool.parameters)
    } else {
        Ok(tool.parameters.clone())
    }
}
```

### 4.2 `resolve_json_schema_strict_sampling` must validate before saying yes

Current — [utils/constrained_sampling.rs:205-228](../../../crates/cyrup-provider/src/utils/constrained_sampling.rs):

```rust
/// Pi `resolveJsonSchemaStrictSampling` (`constrained-sampling.ts:83-97`).
pub fn resolve_json_schema_strict_sampling(
    tool: &ToolDef,
    supports_strict_mode: bool,
) -> Result<Option<bool>> {
    let Some(ConstrainedSamplingConfig::JsonSchema { strict }) =
        tool.constrained_sampling.as_ref().and_then(|c| c.config())
    else {
        return Ok(None);
    };

    if supports_strict_mode {
        return Ok(Some(true));
    }
    if *strict == StrictSampling::Require {
        return err(format!(
            "Tool \"{}\" requires JSON-schema constrained sampling, but strict tools are unsupported.",
            tool.name
        ));
    }
    Ok(None)
}
```

Replacement — pi `7915cdac`, [constrained-sampling.ts:208-227](../../../tmp/pi/packages/ai/src/api/constrained-sampling.ts):

```rust
/// Pi `resolveJsonSchemaStrictSampling` (`constrained-sampling.ts:208-227` @v0.84.2).
///
/// A route that CAN do strict mode still only gets `true` when the tool's schema actually converts
/// to the strict subset; a schema that does not convert degrades to `None` under `prefer` and fails
/// the request under `require`, carrying the conversion's own reason.
pub fn resolve_json_schema_strict_sampling(
    tool: &ToolDef,
    supports_strict_mode: bool,
) -> Result<Option<bool>> {
    let Some(ConstrainedSamplingConfig::JsonSchema { strict }) =
        tool.constrained_sampling.as_ref().and_then(|c| c.config())
    else {
        return Ok(None);
    };

    if supports_strict_mode {
        return match make_strict_json_schema(&tool.parameters) {
            Ok(_) => Ok(Some(true)),
            Err(e) if *strict == StrictSampling::Require => err(format!(
                "Tool \"{}\" requires JSON-schema constrained sampling, but {}.",
                tool.name, e.0
            )),
            Err(_) => Ok(None),
        };
    }
    if *strict == StrictSampling::Require {
        return err(format!(
            "Tool \"{}\" requires JSON-schema constrained sampling, but strict tools are unsupported.",
            tool.name
        ));
    }
    Ok(None)
}
```

Also bump the module header's `@v0.83.0 … byte-identical at v0.84.1` provenance line at
[utils/constrained_sampling.rs:1-3](../../../crates/cyrup-provider/src/utils/constrained_sampling.rs)
to `@v0.84.2`, naming `7915cdac` as the source of the strict conversion.

### 4.3 Six adapter call sites serialize the converted schema

Each site already computes `strict` and then writes `t.parameters` / `tool.parameters` raw. Each must
write `json_schema_tool_parameters(...)` instead, and add `json_schema_tool_parameters` to the
existing `use crate::utils::constrained_sampling::{…}` import in its file.

**1. [anthropic_messages.rs:1252-1292](../../../crates/cyrup-provider/src/api/anthropic_messages.rs)** —
pi [anthropic-messages.ts:1297-1311](../../../tmp/pi/packages/ai/src/api/anthropic-messages.ts).
Current, reading `tool.parameters` three times:

```rust
let strict = resolve_json_schema_strict_sampling(tool, supports_strict_tools)?;
let name = if is_oauth { to_claude_code_name(&tool.name) } else { tool.name.clone() };
let properties = tool.parameters.get("properties").cloned().unwrap_or_else(|| json!({}));
let required = tool.parameters.get("required").cloned().unwrap_or_else(|| json!([]));
```

Replacement — bind the converted schema once, read all three from it:

```rust
let strict = resolve_json_schema_strict_sampling(tool, supports_strict_tools)?;
// PROV-011 @v0.84.2 (`anthropic-messages.ts:1299`): BOTH the legacy three-key subset and the
// strict spread are taken from the CONVERTED schema, not from the tool's raw parameters.
let parameters = json_schema_tool_parameters(tool, strict == Some(true))?;
let name = if is_oauth { to_claude_code_name(&tool.name) } else { tool.name.clone() };
let properties = parameters.get("properties").cloned().unwrap_or_else(|| json!({}));
let required = parameters.get("required").cloned().unwrap_or_else(|| json!([]));
```

and in the strict arm below, `tool.parameters.as_object()` becomes `parameters.as_object()`:

```rust
let input_schema = if strict == Some(true) {
    let mut merged = parameters.as_object().cloned().unwrap_or_else(Map::new);
    for (k, v) in &legacy {
        merged.insert(k.clone(), v.clone());
    }
    Value::Object(merged)
} else {
    Value::Object(legacy)
};
```

**2. [openai_completions.rs:845-849](../../../crates/cyrup-provider/src/api/openai_completions.rs)** —
pi [openai-completions.ts:1367](../../../tmp/pi/packages/ai/src/api/openai-completions.ts). Current:

```rust
let strict = resolve_json_schema_strict_sampling(t, compat.supports_strict_mode)?;
let mut function = Map::new();
function.insert("name".to_string(), json!(t.name));
function.insert("description".to_string(), json!(t.description));
function.insert("parameters".to_string(), t.parameters.clone());
```

Replacement:

```rust
let strict = resolve_json_schema_strict_sampling(t, compat.supports_strict_mode)?;
let mut function = Map::new();
function.insert("name".to_string(), json!(t.name));
function.insert("description".to_string(), json!(t.description));
function.insert(
    "parameters".to_string(),
    json_schema_tool_parameters(t, strict == Some(true))?,
);
```

**3. [openai_responses.rs:968-990](../../../crates/cyrup-provider/src/api/openai_responses.rs)** —
pi [openai-responses-shared.ts:380-393](../../../tmp/pi/packages/ai/src/api/openai-responses-shared.ts).
Ordering matters: upstream resolves `strict = constrainedStrict ?? defaultStrict` **first** and
converts on that, so a caller-supplied `default_strict = Some(true)` also converts. This site covers
Azure ([azure_openai_responses.rs](../../../crates/cyrup-provider/src/api/azure_openai_responses.rs))
and Codex ([openai_codex_responses.rs:733-739](../../../crates/cyrup-provider/src/api/openai_codex_responses.rs)),
which both delegate here. Current:

```rust
let constrained_strict =
    resolve_json_schema_strict_sampling(t, options.supports_strict_mode)?;
let mut o = Map::new();
o.insert("type".to_string(), json!("function"));
o.insert("name".to_string(), json!(t.name));
o.insert("description".to_string(), json!(t.description));
o.insert("parameters".to_string(), t.parameters.clone());
```

Replacement:

```rust
let constrained_strict =
    resolve_json_schema_strict_sampling(t, options.supports_strict_mode)?;
// `const strict = constrainedStrict ?? defaultStrict` (`:381`) — resolved BEFORE the schema is
// converted, and reused for the `strict` key below.
let strict = constrained_strict.or(options.default_strict);
let mut o = Map::new();
o.insert("type".to_string(), json!("function"));
o.insert("name".to_string(), json!(t.name));
o.insert("description".to_string(), json!(t.description));
o.insert(
    "parameters".to_string(),
    json_schema_tool_parameters(t, strict == Some(true))?,
);
```

and the `strict` key insertion further down loses its recomputation:

```rust
if options.supports_strict_mode {
    o.insert(
        "strict".to_string(),
        match strict {
            Some(b) => json!(b),
            None => Value::Null,
        },
    );
}
```

**4. [mistral_conversations.rs:471-479](../../../crates/cyrup-provider/src/api/mistral_conversations.rs)** —
pi [mistral-conversations.ts:756](../../../tmp/pi/packages/ai/src/api/mistral-conversations.ts):

```rust
let strict = resolve_json_schema_strict_sampling(t, true)?;
Ok(json!({
    "type": "function",
    "function": {
        "name": t.name,
        "description": t.description,
        "parameters": json_schema_tool_parameters(t, strict == Some(true))?,
        "strict": strict.unwrap_or(false),
    },
}))
```

**5. [bedrock_converse_stream/convert.rs:284-292](../../../crates/cyrup-provider/src/api/bedrock_converse_stream/convert.rs)** —
pi [bedrock-converse-stream.ts:1011](../../../tmp/pi/packages/ai/src/api/bedrock-converse-stream.ts):

```rust
let strict = resolve_json_schema_strict_sampling(tool, supports_strict_mode)?;
let mut spec = Map::new();
spec.insert("name".to_string(), json!(tool.name));
spec.insert("description".to_string(), json!(tool.description));
spec.insert(
    "inputSchema".to_string(),
    json!({ "json": json_schema_tool_parameters(tool, strict == Some(true))? }),
);
```

**6. [google_generative_ai.rs:851-866](../../../crates/cyrup-provider/src/api/google_generative_ai.rs)** —
pi [google-shared.ts:286-306](../../../tmp/pi/packages/ai/src/api/google-shared.ts). This one needs a
signature change: `convert_tools` currently takes no strict information and is infallible. Current:

```rust
pub(crate) fn convert_tools(tools: &[ToolDef]) -> Option<Value> {
    if tools.is_empty() {
        return None;
    }
    let decls: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "parametersJsonSchema": t.parameters,
            })
        })
        .collect();
    Some(json!([{ "functionDeclarations": decls }]))
}
```

Replacement:

```rust
/// Convert tools to Gemini `functionDeclarations` (Pi `convertTools`, `google-shared.ts:286-306`
/// @v0.84.2). Uses `parametersJsonSchema` (full JSON Schema). `None` when there are no tools.
///
/// PROV-011: `supports_strict_mode` is `supportsGoogleStrictToolSampling(model.id)`, threaded in
/// from the caller so a tool that opted in is serialized with the STRICT-converted schema Gemini's
/// VALIDATED mode constrains against.
pub(crate) fn convert_tools(
    tools: &[ToolDef],
    supports_strict_mode: bool,
) -> Result<Option<Value>, ConstrainedSamplingError> {
    if tools.is_empty() {
        return Ok(None);
    }
    let decls: Vec<Value> = tools
        .iter()
        .map(|t| {
            let strict = resolve_json_schema_strict_sampling(t, supports_strict_mode)?;
            Ok(json!({
                "name": t.name,
                "description": t.description,
                "parametersJsonSchema": json_schema_tool_parameters(t, strict == Some(true))?,
            }))
        })
        .collect::<Result<Vec<Value>, ConstrainedSamplingError>>()?;
    Ok(Some(json!([{ "functionDeclarations": decls }])))
}
```

and the call site at
[google_generative_ai.rs:343-352](../../../crates/cyrup-provider/src/api/google_generative_ai.rs)
hoists the capability read so both uses share it (pi `google-generative-ai.ts:370`):

```rust
if !ctx.tools.is_empty() {
    let supports_strict_mode = supports_google_strict_tool_sampling(model.id.as_str());
    if let Some(tools) = convert_tools(&ctx.tools, supports_strict_mode)? {
        obj.insert("tools".to_string(), tools);
    }
    let mode = resolve_google_function_calling_mode(
        &ctx.tools,
        opts.tool_choice.as_ref(),
        supports_strict_mode,
    )?;
```

### 4.4 `crates/cyrup-provider/src/validate.rs` — strip the strict-mode nulls

Port pi's `normalizeOptionalNulls`
([validation.ts:240-269](../../../tmp/pi/packages/ai/src/utils/validation.ts)), called at
[validation.ts:319](../../../tmp/pi/packages/ai/src/utils/validation.ts) as the first statement of
`validateToolArguments`. cyrup's `validate_tool_call` is that function's counterpart and is the sole
gate the agent preflight runs
([preflight.rs:42](../../../crates/cyrup-agent/src/agent/run/tools/preflight.rs)), so placing the
call there covers every tool on every route.

Current — [validate.rs:56-58](../../../crates/cyrup-provider/src/validate.rs):

```rust
pub fn validate_tool_call(schema: &Value, arguments: Value) -> Result<Value, ToolValidationError> {
    coerce(schema, arguments, "$", false)
}
```

Replacement:

```rust
pub fn validate_tool_call(schema: &Value, arguments: Value) -> Result<Value, ToolValidationError> {
    let mut arguments = arguments;
    normalize_optional_nulls(&mut arguments, schema);
    coerce(schema, arguments, "$", false)
}

/// Pi `normalizeOptionalNulls` (`validation.ts:240-269` @v0.84.2), run BEFORE coercion.
///
/// Strict constrained sampling forces every property into `required` and wraps each optional one in
/// `anyOf: [T, {type:"null"}]` (see `make_json_schema_node_strict`), so the model legitimately emits
/// `"limit": null` for an argument it is declining. Validation still runs against the tool's OWN
/// schema, where `limit` is `{"type":"number"}` — and the falsy coercion table turns that `null`
/// into `0` (`coerce_number` below, pi `validation.ts:60-73`), which would run `read` with a
/// zero-line limit and `bash` with a zero-second timeout. Deleting the key restores "absent".
fn normalize_optional_nulls(value: &mut Value, schema: &Value) {
    let Some(schema) = schema.as_object() else { return };

    if let Some(items) = value.as_array_mut() {
        match schema.get("items") {
            Some(Value::Array(tuple)) => {
                for (index, item) in items.iter_mut().enumerate() {
                    if let Some(item_schema) = tuple.get(index) {
                        normalize_optional_nulls(item, item_schema);
                    }
                }
            }
            Some(item_schema) => {
                for item in items.iter_mut() {
                    normalize_optional_nulls(item, item_schema);
                }
            }
            None => {}
        }
        return;
    }

    let Some(properties) = schema.get("properties").and_then(Value::as_object) else { return };
    let Some(object) = value.as_object_mut() else { return };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    for (key, property_schema) in properties {
        let Some(current) = object.get_mut(key) else { continue };
        // Upstream skips `$ref` properties because it cannot compile a sub-validator for them;
        // cyrup's coercer has no `$ref` support either, so the same key is skipped.
        let is_ref = property_schema.get("$ref").is_some_and(Value::is_string);
        // Upstream's `getSubSchemaValidator(propertySchema)?.Check(null) === false`. The STRICT pass
        // of `coerce` accepts only a value that ALREADY has the exact JSON type the schema wants, so
        // it is exactly that predicate: `{"type":"number"}` rejects null, while
        // `{"type":["number","null"]}` and an `anyOf` containing `{"type":"null"}` accept it.
        let rejects_null = coerce(property_schema, Value::Null, "$", true).is_err();
        if current.is_null() && !required.contains(&key.as_str()) && !is_ref && rejects_null {
            object.remove(key);
        } else {
            normalize_optional_nulls(current, property_schema);
        }
    }
}
```

Update the module header at [validate.rs:1-28](../../../crates/cyrup-provider/src/validate.rs) to
record the new first stage and its `@v0.84.2` provenance.

---

## 5. Required change — the four declarations

### 5.1 `crates/cyrup-core/src/constrained_sampling.rs` — the helper

`Tool::constrained_sampling()` returns `Option<&ConstrainedSampling>` — a **reference** — so the
value cannot be constructed per call. It must be a `'static`. Append to the module (pi
[core/experimental.ts](../../../tmp/pi/packages/coding-agent/src/core/experimental.ts)):

```rust
/// Pi `PREFER_STRICT_TOOL_SAMPLING` (`core/experimental.ts:1`) — the single value every built-in
/// declares. A `static` because [`crate::Tool::constrained_sampling`] hands out a reference.
static PREFER_STRICT_TOOL_SAMPLING: ConstrainedSampling =
    ConstrainedSampling::Config(ConstrainedSamplingConfig::JsonSchema {
        strict: StrictSampling::Prefer,
    });

/// [`experimental_tool_sampling`] against an injected environment, so the `||` precedence is
/// exercisable without touching process state. Same shape as
/// `cyrup_tui::status::experimental_features_enabled_from`.
pub fn experimental_tool_sampling_from(
    get: impl Fn(&str) -> Option<String>,
) -> Option<&'static ConstrainedSampling> {
    let enabled = get("CYRUP_EXPERIMENTAL").as_deref() == Some("1")
        || get("PI_EXPERIMENTAL").as_deref() == Some("1");
    enabled.then_some(&PREFER_STRICT_TOOL_SAMPLING)
}

/// Pi `getExperimentalToolSampling` (`core/experimental.ts:7-9`): the strict-`prefer` JSON-schema
/// declaration when the experimental flag is on, and nothing otherwise.
///
/// `CYRUP_EXPERIMENTAL` is the renamed primary and `PI_EXPERIMENTAL` survives as the
/// lower-precedence fallback — the same pair, in the same order, as
/// `cyrup::startup::are_experimental_features_enabled` (`startup.rs:77-84`) and
/// `cyrup_tui::status::experimental_features_enabled` (`status.rs:474-483`). Upstream re-reads
/// `process.env` on every call but only ever calls it while BUILDING a tool definition; the env is
/// read once here and latched, because cyrup likewise builds its tool set once
/// (`ToolRegistry::with_builtins`).
pub fn experimental_tool_sampling() -> Option<&'static ConstrainedSampling> {
    static RESOLVED: std::sync::OnceLock<Option<&'static ConstrainedSampling>> =
        std::sync::OnceLock::new();
    *RESOLVED.get_or_init(|| experimental_tool_sampling_from(|k| std::env::var(k).ok()))
}
```

Re-export from [cyrup-core/src/lib.rs:21-23](../../../crates/cyrup-core/src/lib.rs):

```rust
pub use constrained_sampling::{
    experimental_tool_sampling, experimental_tool_sampling_from, ConstrainedSampling,
    ConstrainedSamplingConfig, GrammarVariants, StrictSampling,
};
```

Delete the now-false `# No built-in tool declares it — upstream or here` section from the module
header at [constrained_sampling.rs:22-27](../../../crates/cyrup-core/src/constrained_sampling.rs) and
replace it with the v0.84.2 fact: the four coding built-ins declare it via
`getExperimentalToolSampling()`, added in pi `7915cdac`, and `experimental_tool_sampling` above is
that function's Rust counterpart.

### 5.2 `crates/cyrup-core/src/tool.rs` — correct the trait doc

At [tool.rs:150-155](../../../crates/cyrup-core/src/tool.rs) the paragraph currently reads:

```rust
    /// Default `None` = the field is absent, which upstream is indistinguishable from `false`
    /// (`ConstrainedSampling::Disabled`). No pi built-in tool declares it — the three hits of
    /// `git grep constrainedSampling v0.83.0 -- packages/coding-agent/src packages/agent/src` are
    /// the field declaration and the two wrapper copies — so every built-in correctly keeps the
    /// default.
```

Replace with:

```rust
    /// Default `None` = the field is absent, which upstream is indistinguishable from `false`
    /// (`ConstrainedSampling::Disabled`), and is what a tool with no opinion keeps.
    ///
    /// The four coding built-ins DO declare it as of pi `7915cdac` ("feat(ai): add strict tool
    /// schema conversion", first tagged v0.84.2): `constrainedSampling:
    /// getExperimentalToolSampling()` at `core/tools/read.ts:222`, `bash.ts:354` (the shared shell
    /// definition, so upstream `powershell` inherits it from the same line), `edit.ts:329` and
    /// `write.ts:200`, plus `server/create-harness.ts:34` for harness tools. `cyrup-tools` mirrors
    /// that by returning [`crate::constrained_sampling::experimental_tool_sampling`]. An earlier
    /// revision of this doc asserted the opposite; it was true at v0.83.0 and went stale at
    /// v0.84.2.
```

### 5.3 The four overrides

One method per tool, placed immediately after `prompt_guidelines` and before `execute` in each
`impl Tool` block. `grep`, `find` and `ls` are deliberately **not** touched — upstream declares it on
exactly the four coding tools.

**[crates/cyrup-tools/src/tools/read.rs](../../../crates/cyrup-tools/src/tools/read.rs)** — after
line 92:

```rust
    fn prompt_guidelines(&self) -> Vec<&str> {
        vec!["Use read to examine files instead of cat or sed."]
    }

    /// Pi `constrainedSampling: getExperimentalToolSampling()` (`core/tools/read.ts:222`
    /// @v0.84.2). With `CYRUP_EXPERIMENTAL=1`/`PI_EXPERIMENTAL=1` this asks a strict-capable route
    /// to constrain generation to the declared schema; `prefer` degrades silently elsewhere.
    fn constrained_sampling(&self) -> Option<&cyrup_core::ConstrainedSampling> {
        cyrup_core::experimental_tool_sampling()
    }
```

**[crates/cyrup-tools/src/tools/bash.rs](../../../crates/cyrup-tools/src/tools/bash.rs)** — after the
`prompt_guidelines` block that ends at line 167:

```rust
    /// Pi `constrainedSampling: getExperimentalToolSampling()` (`core/tools/bash.ts:354`
    /// @v0.84.2). It sits on `createShellToolDefinition`, so upstream `powershell` carries the same
    /// declaration from the same line.
    fn constrained_sampling(&self) -> Option<&cyrup_core::ConstrainedSampling> {
        cyrup_core::experimental_tool_sampling()
    }
```

**[crates/cyrup-tools/src/tools/edit.rs](../../../crates/cyrup-tools/src/tools/edit.rs)** — after the
`prompt_guidelines` block that ends at line 193:

```rust
    /// Pi `constrainedSampling: getExperimentalToolSampling()` (`core/tools/edit.ts:329`
    /// @v0.84.2), declared on the same object literal as `renderShell: "self"` and
    /// `prepareArguments`.
    fn constrained_sampling(&self) -> Option<&cyrup_core::ConstrainedSampling> {
        cyrup_core::experimental_tool_sampling()
    }
```

**[crates/cyrup-tools/src/tools/write.rs](../../../crates/cyrup-tools/src/tools/write.rs)** — after
line 89:

```rust
    /// Pi `constrainedSampling: getExperimentalToolSampling()` (`core/tools/write.ts:200`
    /// @v0.84.2).
    fn constrained_sampling(&self) -> Option<&cyrup_core::ConstrainedSampling> {
        cyrup_core::experimental_tool_sampling()
    }
```

All four files already `use cyrup_core::{…}`; the fully-qualified
`cyrup_core::experimental_tool_sampling()` needs no import change and keeps the call site
self-documenting.

---

## 6. Files that change

| File | Change |
| --- | --- |
| [cyrup-core/src/constrained_sampling.rs](../../../crates/cyrup-core/src/constrained_sampling.rs) | Add `PREFER_STRICT_TOOL_SAMPLING`, `experimental_tool_sampling`, `experimental_tool_sampling_from`; rewrite the stale "No built-in tool declares it" module section |
| [cyrup-core/src/lib.rs](../../../crates/cyrup-core/src/lib.rs) | Re-export the two new functions |
| [cyrup-core/src/tool.rs](../../../crates/cyrup-core/src/tool.rs) | Correct the `constrained_sampling` doc-comment (§5.2) |
| [cyrup-tools/src/tools/read.rs](../../../crates/cyrup-tools/src/tools/read.rs) | Override `constrained_sampling` |
| [cyrup-tools/src/tools/bash.rs](../../../crates/cyrup-tools/src/tools/bash.rs) | Override `constrained_sampling` |
| [cyrup-tools/src/tools/edit.rs](../../../crates/cyrup-tools/src/tools/edit.rs) | Override `constrained_sampling` |
| [cyrup-tools/src/tools/write.rs](../../../crates/cyrup-tools/src/tools/write.rs) | Override `constrained_sampling` |
| [cyrup-provider/src/utils/constrained_sampling.rs](../../../crates/cyrup-provider/src/utils/constrained_sampling.rs) | Add the strict-conversion functions + `json_schema_tool_parameters`; rewrite `resolve_json_schema_strict_sampling`; bump provenance to `@v0.84.2` |
| [cyrup-provider/src/api/anthropic_messages.rs](../../../crates/cyrup-provider/src/api/anthropic_messages.rs) | `convert_tools` serializes the converted schema |
| [cyrup-provider/src/api/openai_completions.rs](../../../crates/cyrup-provider/src/api/openai_completions.rs) | `convert_tools` serializes the converted schema |
| [cyrup-provider/src/api/openai_responses.rs](../../../crates/cyrup-provider/src/api/openai_responses.rs) | `convert_responses_tools` resolves `strict` first, then converts (covers Azure + Codex) |
| [cyrup-provider/src/api/mistral_conversations.rs](../../../crates/cyrup-provider/src/api/mistral_conversations.rs) | `to_function_tools` serializes the converted schema |
| [cyrup-provider/src/api/bedrock_converse_stream/convert.rs](../../../crates/cyrup-provider/src/api/bedrock_converse_stream/convert.rs) | `convert_tool_config` serializes the converted schema |
| [cyrup-provider/src/api/google_generative_ai.rs](../../../crates/cyrup-provider/src/api/google_generative_ai.rs) | `convert_tools` gains `supports_strict_mode` + `Result`; call site hoists the capability read |
| [cyrup-provider/src/validate.rs](../../../crates/cyrup-provider/src/validate.rs) | Add `normalize_optional_nulls`; call it first in `validate_tool_call` |

Nothing in [cyrup-ext/src/wrapper.rs](../../../crates/cyrup-ext/src/wrapper.rs),
[cyrup-ext-sdk/src/descriptor.rs](../../../crates/cyrup-ext-sdk/src/descriptor.rs),
[cyrup-agent/src/agent/run/stream.rs](../../../crates/cyrup-agent/src/agent/run/stream.rs) or
[cyrup-provider/src/context.rs](../../../crates/cyrup-provider/src/context.rs) changes — that plumbing
is already correct and already forwards the declaration.

---

## 7. Genuinely uncertain

* **Latched vs. re-read flag.** Upstream re-reads `process.env` per `getExperimentalToolSampling()`
  call, but only ever calls it while building a tool definition. §5.1 latches with a `OnceLock`,
  matching the precedent recorded at
  [cyrup-tui/src/app/state.rs:281-285](../../../crates/cyrup-tui/src/app/state.rs). If a future
  caller rebuilds the tool registry after mutating the process env, the latch will be observed as
  stale; `experimental_tool_sampling_from` is the escape hatch.
* **`supports_strict_mode` breadth.** cyrup enables strict mode by default on OpenAI-completions
  routes ([compat.rs:660](../../../crates/cyrup-provider/src/api/compat.rs)) and unconditionally on
  Mistral. Whether every provider behind those routes accepts pi's strict subset for `edit`'s nested
  array-of-objects schema is not verifiable from source alone. `strict: "prefer"` means a provider
  that rejects it degrades rather than fails, but that degradation is provider-side, not cyrup-side.
* **The `$ref` skip in `normalize_optional_nulls`.** Upstream skips `$ref` properties because it
  cannot compile a standalone validator for them. cyrup's coercer has no `$ref` support at all, so
  the skip is preserved for behavioural parity, but no schema in the workspace exercises it.

---

## Definition of done

Observable behaviour that must hold once the change lands:

1. With the experimental flag unset, `ReadTool`, `BashTool`, `EditTool` and `WriteTool` each report
   `constrained_sampling() == None`, and every provider request is byte-identical to today's.
2. With `CYRUP_EXPERIMENTAL=1` — and, independently, with `PI_EXPERIMENTAL=1` — each of those four
   tools reports
   `Some(ConstrainedSampling::Config(ConstrainedSamplingConfig::JsonSchema { strict: StrictSampling::Prefer }))`;
   `grep`, `find` and `ls` still report `None`.
3. That declaration survives the agent loop: the `ToolDef` entries for `read`, `bash`, `edit` and
   `write` in the `Context` handed to the provider carry it.
4. On a route with strict mode available, the serialized tool for each of the four carries
   `strict: true` **and** a schema in which every key of `properties` appears in `required`,
   `additionalProperties` is `false` at every object level, and each property that was optional in
   the tool's own schema is `{"anyOf":[<original>,{"type":"null"}]}` — for `read` that is
   `offset` and `limit`, for `bash` `timeout`; `edit` and `write` change only by gaining
   `additionalProperties: false` (at both levels for `edit`), because all of their properties are
   already required.
5. On a route without strict mode, no `strict: true` is emitted for these tools, the raw schema is
   sent unchanged, and the request is not failed — `prefer` degrades silently.
6. A tool call arriving with an optional argument set to `null` — `{"command":"ls","timeout":null}`,
   `{"path":"x","offset":null,"limit":null}` — executes with that argument **absent**: `bash` with no
   timeout, `read` with its default offset and limit. It is never coerced to `0`.
7. A tool declaring `strict: "require"` whose schema cannot be converted fails the request with pi's
   message shape `Tool "<name>" requires JSON-schema constrained sampling, but <reason>.`; the same
   tool declaring `prefer` is sent unconstrained instead.
8. No doc-comment anywhere in the workspace still asserts that no pi built-in declares
   `constrainedSampling`.
