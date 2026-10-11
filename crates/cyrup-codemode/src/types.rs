//! The shapes the lane-A modules share (pi `packages/codemode/src/types.ts` @v1.0.1, CODE-004).
//!
//! Only what the engine-independent modules consume is here. The sandbox-facing half of
//! `types.ts` (`CodemodeToolContext`, `CodemodeSandboxOptions`, `CodemodeResult`, ...) belongs to
//! the crate that owns the engine.

use serde_json::Value;

use crate::identifier::{CodemodeIdentifier, to_codemode_identifier};

/// A tool or global as the declaration renderer sees it: pi `CodemodeTool` (`types.ts:14-41`)
/// without `execute`, which is the engine's business and never reaches a declaration.
///
/// Schemas are JSON Schema documents held as [`serde_json::Value`]; they are only rendered, never
/// validated against.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolDeclaration {
    /// Tools are called as `tools.<id>(args)` where `<id>` is
    /// [`to_codemode_identifier`](crate::identifier::to_codemode_identifier) of this name. Globals
    /// are called as `<name>(args)`, or `<namespace>.<member>` to group them into one object.
    pub name: String,
    /// [CYRUP-DELTA] The identifier scripts call a tool by when it is not
    /// [`to_codemode_identifier`] of [`Self::name`], because another tool has that identifier too
    /// (see [`IdentifierTable`](crate::identifier::IdentifierTable)). `None` for globals and for
    /// every tool whose identifier is the derived one.
    pub identifier: Option<CodemodeIdentifier>,
    /// Shown as a doc comment in [`render_declarations`](crate::declarations::render_declarations).
    pub description: Option<String>,
    /// Schema of the single argument. Rendered as the parameter type; `unknown` when omitted.
    pub input_schema: Option<Value>,
    /// Schema of the resolved value. Rendered as the promise type; `unknown` when omitted.
    pub output_schema: Option<Value>,
    /// Globals only: the script's call arguments reach the host as an array rather than the first
    /// one. The renderer does not look at it; it is carried for the globals lane.
    pub spread: bool,
    /// Globals only: a TypeScript parameter list and return type, such as
    /// `(type: string, id?: string): Promise<Model[]>`, replacing the rendering from the schemas.
    pub signature: Option<String>,
}

impl ToolDeclaration {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// The identifier scripts call this tool by: [`Self::identifier`], else the one derived from the
    /// name.
    #[must_use]
    pub fn script_identifier(&self) -> CodemodeIdentifier {
        self.identifier
            .clone()
            .unwrap_or_else(|| to_codemode_identifier(&self.name))
    }

    #[must_use]
    pub fn with_identifier(mut self, identifier: CodemodeIdentifier) -> Self {
        self.identifier = Some(identifier);
        self
    }

    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    #[must_use]
    pub fn with_input_schema(mut self, schema: Value) -> Self {
        self.input_schema = Some(schema);
        self
    }

    #[must_use]
    pub fn with_output_schema(mut self, schema: Value) -> Self {
        self.output_schema = Some(schema);
        self
    }

    #[must_use]
    pub fn with_signature(mut self, signature: impl Into<String>) -> Self {
        self.signature = Some(signature.into());
        self
    }
}

/// One item of a script's output, in the order the script produced it: `text()` produces text
/// items, `console.*` console items, `image()` image items (pi `CodemodeOutputItem`,
/// `types.ts:44-49` @v1.1.0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputItem {
    Text(String),
    /// One `console.*` call's line: pi's text item with `console: true` (`types.ts:48` @v1.1.0,
    /// `eb326d265`). [`format_output`](crate::output::format_output) moves these into one trailing
    /// `<console_output>` block.
    Console(String),
    /// `data` is base64.
    Image {
        data: String,
        mime_type: String,
    },
}
