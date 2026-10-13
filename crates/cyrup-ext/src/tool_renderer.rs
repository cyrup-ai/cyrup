//! EXT-107 — tool-renderer RESOLVERS: how an extension draws calls to a tool it did not register.
//!
//! pi `pi.registerToolRenderer(resolver)` (`core/extensions/types.ts:1696` @f1b2e77f5, the
//! `ToolRendererResolver` type at `:664-667`, stored per extension by `loader.ts:367-371`) and
//! `ExtensionRunner.resolveToolRenderers(toolName, base)` (`runner.ts:790-795`): every extension's
//! resolvers run in EXTENSION LOAD ORDER, each receives `(toolName, next)`, and `next()` returns
//! what the remaining resolvers and then the registered tool (`base`) would use. Added by pi
//! `11449730c` so the bundled MCP extension can draw `mcp__<server>__<tool>` calls before (or
//! without) their server connecting: `next() ?? createMcpToolRenderers(...)`
//! (`extensions/mcp/index.ts:383-386` @f1b2e77f5).
//!
//! cyrup keeps ONE chain for both tiers ([`ToolRendererResolver`], held by
//! [`crate::ExtensionRegistry`] in registration order, which is load order):
//!
//! * a NATIVE extension may register a real callback ([`ToolRendererResolver::Callback`]) with
//!   upstream's `(toolName, next)` shape, or a declarative pattern;
//! * a WASM guest registers a declarative pattern ([`ToolRendererResolver::Pattern`]) through the
//!   `registration.register-tool-renderer` import. A closure cannot cross the component boundary
//!   (ADR-0002), so the two idioms a pi resolver is written in are named as a
//!   [`ToolRendererMode`]: [`ToolRendererMode::FillIn`] is `next() ?? (matches ? mine : undefined)`
//!   and [`ToolRendererMode::Claim`] is `matches ? mine : next()`.
//!
//! Both forms are evaluated by the SAME recursion ([`resolve_chain`]), so a pattern is not a
//! second semantics: it is exactly the resolver a pi author would have written by hand, placed in
//! the chain at its extension's load-order position.
//!
//! [CYRUP-DELTA] `next()` answers WHO would draw ([`ToolRenderers`] is an opaque "this extension"
//! / "the registered tool" handle), not callable renderer functions. pi's `next()` hands back the
//! `renderCall` / `renderResult` functions themselves, so a resolver could wrap them. In cyrup the
//! renderer of another extension (or a guest) is reached only through the host's render dispatch,
//! which a resolver running inside that dispatch cannot re-enter; choosing — the whole of what pi's
//! own consumers (`interactive-mode.ts:2228-2231`, the MCP extension) do with `next()` — is kept.
//! A second, smaller delta follows from the same split: a resolver that answers `None` without
//! consulting `next()` makes no EXTENSION draw the call, but cannot also suppress the host's own
//! built-in drawing of a built-in tool, which pi's `undefined` definition would.

use std::sync::Arc;

use cyrup_core::ExtensionId;

/// What the resolver chain chose to draw a tool call: pi's `ToolRenderers | undefined`, as a
/// handle. Obtained from `next()` (pass it on to defer) or from [`Self::mine`] (draw it yourself).
///
/// Opaque on purpose: a resolver can answer "mine" or "whatever comes after me", and nothing else —
/// it cannot name ANOTHER extension as the renderer, which is the same reach pi's resolver has
/// (it can only return its own functions or what `next()` gave it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRenderers(Pick);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Pick {
    /// The answering resolver's own extension; the chain rewrites it to [`Pick::Extension`] with
    /// that resolver's owner before anyone else sees it.
    Mine,
    /// An extension's resolver chose itself.
    Extension(ExtensionId),
    /// pi's `base()`: the registered tool's own renderer (an exact-name tool renderer, an SDK tool
    /// carrying its own `render_call`), or — for a tool that is known but has none — the host's
    /// built-in drawing.
    Base,
}

impl ToolRenderers {
    /// "I draw this call": the resolving extension's renderer is invoked with the TOOL NAME as its
    /// key (a native's `render_call_tree` / `render_call_under`, a guest's `render-call` export).
    pub fn mine() -> Self {
        Self(Pick::Mine)
    }

    /// The extension a resolver chose, or `None` when the choice is the registered tool itself.
    pub fn extension(&self) -> Option<&ExtensionId> {
        match &self.0 {
            Pick::Extension(id) => Some(id),
            Pick::Mine | Pick::Base => None,
        }
    }

    /// Whether this is pi's `base()` — the registered tool (or the host's built-in) draws.
    pub fn is_registered_tool(&self) -> bool {
        self.0 == Pick::Base
    }

    /// The `base()` answer, built only by the host.
    pub(crate) fn base() -> Self {
        Self(Pick::Base)
    }
}

/// A native resolver: pi's `ToolRendererResolver = (toolName, next) => ToolRenderers | undefined`.
pub type ToolRendererResolverFn =
    dyn Fn(&str, &mut dyn FnMut() -> Option<ToolRenderers>) -> Option<ToolRenderers> + Send + Sync;

/// How a declarative pattern sits in the chain — the two idioms a pi resolver is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolRendererMode {
    /// `next() ?? (matches ? mine : undefined)` — the MCP extension's form
    /// (`extensions/mcp/index.ts:383-386` @f1b2e77f5). Draws only when nothing after it would:
    /// a registered tool's own renderer, and a later extension's resolver, both win.
    FillIn,
    /// `matches ? mine : next()` — draws every matching call, ahead of the registered tool and of
    /// every resolver loaded after it.
    Claim,
}

/// A tool-name pattern: `*` matches any run of characters (including none); every other character
/// matches itself. `mcp__*` is a prefix, `*_ghost` a suffix, `mcp__*__*` pi's MCP family, and a
/// pattern without `*` one exact name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolNamePattern(String);

impl ToolNamePattern {
    /// A pattern, refused when empty (it could only match a tool with no name).
    pub fn new(pattern: impl Into<String>) -> Result<Self, String> {
        let pattern = pattern.into();
        if pattern.is_empty() {
            return Err("register-tool-renderer: the pattern is empty".to_string());
        }
        Ok(Self(pattern))
    }

    /// The pattern as declared.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `name` matches — the same matcher `--tools` / `--exclude-tools` patterns use
    /// ([`cyrup_core::tool_name_matches`], pi `createToolNameMatcher`), so one `*` rule holds
    /// across the host.
    pub fn matches(&self, name: &str) -> bool {
        cyrup_core::tool_name_matches(std::iter::once(&self.0), name)
    }
}

/// One entry of the chain.
#[derive(Clone)]
pub enum ToolRendererResolver {
    /// A declarative family — the guest tier's only form, and available to natives too.
    Pattern {
        pattern: ToolNamePattern,
        mode: ToolRendererMode,
    },
    /// A native callback with pi's `(toolName, next)` shape.
    Callback(Arc<ToolRendererResolverFn>),
}

impl std::fmt::Debug for ToolRendererResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pattern { pattern, mode } => f
                .debug_struct("Pattern")
                .field("pattern", pattern)
                .field("mode", mode)
                .finish(),
            Self::Callback(_) => f.write_str("Callback(..)"),
        }
    }
}

/// pi `resolveToolRenderers(toolName, base)` (`runner.ts:790-795` @f1b2e77f5):
/// `resolve(i) = i < resolvers.length ? resolvers[i](toolName, () => resolve(i + 1)) : base()`.
///
/// `resolvers` is in load order. A [`ToolRenderers::mine`] answer is rewritten to that resolver's
/// owner before it is returned to the resolver before it, so an earlier resolver deferring through
/// `next()` passes on WHICH extension chose.
///
/// A panicking native resolver is contained the way a panicking native renderer is: logged, and
/// treated as one that deferred (`next()`), so one broken extension cannot take the rest of the
/// chain — or the registered tool's own renderer — down with it.
pub(crate) fn resolve_chain(
    resolvers: &[(ExtensionId, ToolRendererResolver)],
    tool_name: &str,
    base: &mut dyn FnMut() -> Option<ToolRenderers>,
) -> Option<ToolRenderers> {
    let Some(((owner, resolver), later)) = resolvers.split_first() else {
        return base();
    };
    let mut next = || resolve_chain(later, tool_name, &mut *base);
    let picked = match resolver {
        ToolRendererResolver::Pattern {
            pattern,
            mode: ToolRendererMode::FillIn,
        } => next().or_else(|| pattern.matches(tool_name).then(ToolRenderers::mine)),
        ToolRendererResolver::Pattern {
            pattern,
            mode: ToolRendererMode::Claim,
        } => {
            if pattern.matches(tool_name) {
                Some(ToolRenderers::mine())
            } else {
                next()
            }
        }
        ToolRendererResolver::Callback(resolve) => {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                resolve(tool_name, &mut next)
            })) {
                Ok(picked) => picked,
                Err(_) => {
                    tracing::warn!(
                        extension = %owner, tool = %tool_name,
                        "tool-renderer resolver panicked (contained; treated as deferring to next())"
                    );
                    next()
                }
            }
        }
    };
    picked.map(|r| match r.0 {
        Pick::Mine => ToolRenderers(Pick::Extension(owner.clone())),
        other => ToolRenderers(other),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn id(s: &str) -> ExtensionId {
        ExtensionId::from(s)
    }

    fn pattern(p: &str, mode: ToolRendererMode) -> ToolRendererResolver {
        ToolRendererResolver::Pattern {
            pattern: ToolNamePattern::new(p).unwrap(),
            mode,
        }
    }

    #[test]
    fn glob_matches_prefix_suffix_infix_and_exact() {
        let p = |s: &str| ToolNamePattern::new(s).unwrap();
        assert!(p("mcp__*").matches("mcp__srv__tool"));
        assert!(!p("mcp__*").matches("xmcp__srv"));
        assert!(p("mcp__*__*").matches("mcp__srv__tool"));
        assert!(!p("mcp__*__*").matches("mcp__srv_tool"));
        assert!(p("*_ghost").matches("demo_ghost"));
        assert!(!p("*_ghost").matches("demo_ghosts"));
        assert!(p("a*b*c").matches("abc"));
        assert!(p("a*b*c").matches("axxbyyc"));
        assert!(!p("a*b*c").matches("acb"));
        assert!(p("exact").matches("exact"));
        assert!(!p("exact").matches("exactly"));
        assert!(p("*").matches(""));
        // The suffix must not reuse characters the prefix consumed.
        assert!(!p("ab*ba").matches("aba"));
        assert!(ToolNamePattern::new("").is_err());
    }

    /// pi's two-`next() ?? mine` composition: the FIRST resolver defers to the second, so the
    /// LATER-loaded fill-in draws when both match and nothing is registered.
    #[test]
    fn two_fill_ins_compose_as_pi_does() {
        let chain = vec![
            (id("a"), pattern("x_*", ToolRendererMode::FillIn)),
            (id("b"), pattern("x_*", ToolRendererMode::FillIn)),
        ];
        let got = resolve_chain(&chain, "x_1", &mut || None).unwrap();
        assert_eq!(got.extension(), Some(&id("b")));
        // ...and both defer to a registered tool.
        let got = resolve_chain(&chain, "x_1", &mut || Some(ToolRenderers::base())).unwrap();
        assert!(got.is_registered_tool());
    }

    /// A claim answers first in load order.
    #[test]
    fn claims_resolve_in_load_order() {
        let chain = vec![
            (id("a"), pattern("x_*", ToolRendererMode::Claim)),
            (id("b"), pattern("x_*", ToolRendererMode::Claim)),
        ];
        let got = resolve_chain(&chain, "x_1", &mut || Some(ToolRenderers::base())).unwrap();
        assert_eq!(got.extension(), Some(&id("a")));
        assert!(resolve_chain(&chain, "y", &mut || None).is_none());
    }

    /// A callback sees what `next()` would use and may pass it on; `mine` is rewritten to its owner.
    #[test]
    fn a_callback_defers_or_draws_with_pi_semantics() {
        let cb: Arc<ToolRendererResolverFn> = Arc::new(|name, next| {
            next().or_else(|| name.starts_with("mcp__").then(ToolRenderers::mine))
        });
        let chain = vec![
            (id("native"), ToolRendererResolver::Callback(cb)),
            (id("guest"), pattern("mcp__late*", ToolRendererMode::FillIn)),
        ];
        assert_eq!(
            resolve_chain(&chain, "mcp__s__t", &mut || None)
                .unwrap()
                .extension(),
            Some(&id("native"))
        );
        assert_eq!(
            resolve_chain(&chain, "mcp__late__t", &mut || None)
                .unwrap()
                .extension(),
            Some(&id("guest")),
            "the native's next() returned the later guest's choice and it passed it on"
        );
        assert!(resolve_chain(&chain, "bash", &mut || None).is_none());
    }

    /// A panicking callback is treated as deferring.
    #[test]
    fn a_panicking_callback_defers() {
        let cb: Arc<ToolRendererResolverFn> = Arc::new(|_, _| {
            #[allow(clippy::panic)]
            {
                panic!("resolver bug")
            }
        });
        let chain = vec![
            (id("bad"), ToolRendererResolver::Callback(cb)),
            (id("good"), pattern("*", ToolRendererMode::Claim)),
        ];
        assert_eq!(
            resolve_chain(&chain, "t", &mut || None)
                .unwrap()
                .extension(),
            Some(&id("good"))
        );
    }
}
