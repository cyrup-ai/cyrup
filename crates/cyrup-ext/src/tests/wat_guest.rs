//! A real `cyrup:ext` COMPONENT, built from `startup_timings_guest.wat`, that can call host imports
//! and give chosen exports a body — so a test drives the production `LiveExtension::load` and event
//! dispatch without the `wasm32-wasip2` SDK toolchain.
//!
//! The base fixture defines its memory inside its one core module, which is instantiated LAST, so
//! nothing could lower a host import against it. [`WatGuest::build`] moves the memory (and a
//! realloc for lowered results) into a small module instantiated first, lowers each requested
//! import against it, and hands both to the fixture's module. Everything the fixture exports is
//! kept, so the component still satisfies the whole `extension` world.

#![allow(clippy::panic)]

use std::fmt::Write as _;

const BASE: &str = include_str!("startup_timings_guest.wat");

/// One host import, lowered for the core module as `(import "host" "<core_name>" …)`.
pub(super) struct Lowered {
    /// The name the core module imports it under; also its `$` identifier there.
    pub core_name: &'static str,
    /// The component-level function it lowers (an alias declared in [`WatGuest::component`]).
    pub component_func: &'static str,
    /// The lowered core signature, e.g. `(param i32 i32)`.
    pub core_sig: &'static str,
    /// Whether the result carries a string or list (the canonical ABI then needs `realloc`).
    pub needs_realloc: bool,
}

#[derive(Default)]
pub(super) struct WatGuest {
    /// Component-level `(import …)` + `(alias export …)` text.
    pub component: String,
    pub lowered: Vec<Lowered>,
    /// `(export name, full core func text)`: replaces the fixture's one-line body for that export.
    pub overrides: Vec<(&'static str, String)>,
    /// `(offset, WAT string literal body)` data segments in the shared memory.
    pub data: Vec<(u32, String)>,
}

/// Escape `s` for a WAT string literal.
pub(super) fn wat_str(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'"' | b'\\' => {
                let _ = write!(out, "\\{:02x}", b);
            }
            0x20..=0x7e => out.push(b as char),
            _ => {
                let _ = write!(out, "\\{:02x}", b);
            }
        }
    }
    out
}

impl WatGuest {
    pub(super) fn build(&self) -> Vec<u8> {
        let mut text = BASE.to_string();

        let mut core_imports = String::from("    (import \"env\" \"mem\" (memory $memory 1))\n");
        for l in &self.lowered {
            let _ = writeln!(
                core_imports,
                "    (import \"host\" \"{0}\" (func ${0} {1}))",
                l.core_name, l.core_sig
            );
        }
        core_imports.push_str("    (export \"mem\" (memory $memory))\n");
        for (offset, body) in &self.data {
            let _ = writeln!(core_imports, "    (data (i32.const {offset}) \"{body}\")");
        }
        text = text.replacen(
            "(core module $m\n    (memory (export \"mem\") 1)\n",
            &format!("(core module $m\n{core_imports}"),
            1,
        );

        for (export, body) in &self.overrides {
            let head = format!("    (func (export \"{export}\")");
            let start = text
                .find(&head)
                .unwrap_or_else(|| panic!("the fixture exports no core func {export:?}"));
            let end = start + text[start..].find('\n').unwrap_or(0);
            text.replace_range(start..end, body);
        }

        let mut prelude = String::from(
            "  (core module $memm\n    (memory (export \"mem\") 1)\n    \
             (global $bump (mut i32) (i32.const 32768))\n    \
             (func (export \"realloc\") (param i32 i32 i32 i32) (result i32) (local $p i32)\n      \
             global.get $bump local.get 2 i32.add i32.const 1 i32.sub\n      \
             i32.const 0 local.get 2 i32.sub i32.and local.tee $p\n      \
             local.get 3 i32.add global.set $bump local.get $p))\n  \
             (core instance $memi (instantiate $memm))\n  \
             (alias core export $memi \"mem\" (core memory $hmem))\n  \
             (alias core export $memi \"realloc\" (core func $hrealloc))\n",
        );
        prelude.push_str(&self.component);
        let mut host = String::from("  (core instance $host");
        for l in &self.lowered {
            let realloc = if l.needs_realloc {
                " (realloc $hrealloc)"
            } else {
                ""
            };
            let _ = writeln!(
                prelude,
                "  (core func $lowered-{0} (canon lower (func {1}) (memory $hmem){realloc}))",
                l.core_name, l.component_func
            );
            let _ = write!(host, " (export \"{0}\" (func $lowered-{0}))", l.core_name);
        }
        host.push_str(")\n");
        prelude.push_str(&host);
        let with_host = if self.lowered.is_empty() {
            ""
        } else {
            " (with \"host\" (instance $host))"
        };
        prelude.push_str(&format!(
            "  (core instance $i (instantiate $m (with \"env\" (instance $memi)){with_host}))\n"
        ));
        text = text.replacen("  (core instance $i (instantiate $m))\n", &prelude, 1);

        wat::parse_str(&text).unwrap_or_else(|e| panic!("generated component: {e}\n{text}"))
    }
}

/// `registration` import declaring the given functions.
pub(super) const REGISTRATION_FLAG_AND_UNSUBSCRIBE: &str = r#"  (import "cyrup:ext/registration@0.13.0" (instance $reg
    (export "register-flag" (func (param "name" string) (param "spec-json" string) (result (result (error string)))))
    (export "subscribe" (func (param "event-kinds" (list u8))))
    (export "unsubscribe" (func (param "event-kinds" (list u8))))))
  (alias export $reg "register-flag" (func $register-flag))
  (alias export $reg "subscribe" (func $subscribe))
  (alias export $reg "unsubscribe" (func $unsubscribe))
"#;

/// `ui` import declaring `set-status` and `select`.
pub(super) const UI_STATUS_AND_SELECT: &str = r#"  (import "cyrup:ext/ui@0.13.0" (instance $ui
    (export "set-status" (func (param "key" string) (param "text" (option string))))
    (export "select" (func (param "prompt" string) (param "options-json" string) (param "opts-json" string) (result (option string))))))
  (alias export $ui "set-status" (func $set-status))
  (alias export $ui "select" (func $select))
"#;
