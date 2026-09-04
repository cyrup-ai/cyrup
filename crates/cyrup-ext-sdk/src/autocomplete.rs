//! Global autocomplete provider stacking (Pi `addAutocompleteProvider(factory)`, types.ts:225;
//! `AutocompleteProviderFactory`, types.ts:124). Each factory WRAPS the current provider, forming a
//! chain the host folds over its own built-in suggestions. A guest models one stacked provider as an
//! [`AutocompleteProvider`]: given the editor [`AutocompleteQuery`] and the suggestions the wrapped
//! ("current") provider produced, it returns the suggestions IT contributes (`None` = defer to the
//! wrapped provider). The host calls `autocomplete-suggest` to drive the fold (sdk gap #2).

use serde::{Deserialize, Serialize};

/// A single completion item (Pi `AutocompleteItem`, tui/autocomplete.ts:219).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AutocompleteItem {
    /// The completion text itself. [`Self::new`] reuses it as the [`Self::label`];
    /// [`Self::labelled`] sets the two apart.
    pub value: String,
    /// The text shown for the item, which [`Self::new`] leaves equal to [`Self::value`].
    pub label: String,
    /// Optional extra detail alongside the label; neither constructor sets one, and `None` is
    /// omitted from the serialized item rather than sent as `null` (`skip_serializing_if`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl AutocompleteItem {
    /// An item whose label IS its value — the common case, where the completion text is what the
    /// user should see.
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            label: value.clone(),
            value,
            description: None,
        }
    }
    /// An item displayed as `label` but completing to `value`.
    pub fn labelled(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: None,
        }
    }
}

/// The suggestions a provider returns (Pi `AutocompleteSuggestions`, tui/autocomplete.ts:236).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AutocompleteSuggestions {
    /// The matched items (WIT `types.autocomplete-suggestions.items`, `wit/world.wit:175`).
    pub items: Vec<AutocompleteItem>,
    /// The prefix those items match against — the WIT record documents the pair as "the matched
    /// items + the prefix they match against" (`wit/world.wit:172-177`).
    pub prefix: String,
}

/// The editor cursor query a provider answers (Pi `getSuggestions(lines, cursorLine, cursorCol)`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutocompleteQuery {
    /// The editor's lines (Pi `getSuggestions(lines, …)`); `lines` on the wire.
    #[serde(default)]
    pub lines: Vec<String>,
    /// The index into [`Self::lines`] the cursor sits on — what [`Self::current_line`] looks up;
    /// `cursorLine` on the wire.
    #[serde(default)]
    pub cursor_line: u32,
    /// The cursor's column within [`Self::current_line`]; `cursorCol` on the wire.
    #[serde(default)]
    pub cursor_col: u32,
    /// Whether the user forced completion (explicit Tab) vs. an incremental trigger.
    #[serde(default)]
    pub force: bool,
}

impl AutocompleteQuery {
    /// The text of the line the cursor is on (empty if out of range — never panics).
    pub fn current_line(&self) -> &str {
        self.lines
            .get(self.cursor_line as usize)
            .map(String::as_str)
            .unwrap_or("")
    }
}

/// One stacked autocomplete provider (the result of a Pi `AutocompleteProviderFactory`). Receives the
/// query and the suggestions the wrapped ("current") provider produced.
pub trait AutocompleteProvider: 'static {
    /// Return the suggestions this provider contributes, or `None` to defer to `current`.
    fn suggest(
        &self,
        query: &AutocompleteQuery,
        current: Option<&AutocompleteSuggestions>,
    ) -> Option<AutocompleteSuggestions>;
}

impl<F> AutocompleteProvider for F
where
    F: Fn(&AutocompleteQuery, Option<&AutocompleteSuggestions>) -> Option<AutocompleteSuggestions>
        + 'static,
{
    fn suggest(
        &self,
        query: &AutocompleteQuery,
        current: Option<&AutocompleteSuggestions>,
    ) -> Option<AutocompleteSuggestions> {
        (self)(query, current)
    }
}
