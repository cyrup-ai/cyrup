use super::*;

/// `KillRing.push` (`pi/packages/tui/src/kill-ring.ts:19-28` @v0.83.0), as a free function over the
/// ring so the multi-line [`InputEditor`] and the single-line [`crate::text_input::Input`] share one
/// accumulate rule:
///
/// ```text
/// if (!text) return;
/// if (opts.accumulate && this.ring.length > 0) {
///     const last = this.ring.pop()!;
///     this.ring.push(opts.prepend ? text + last : last + text);
/// } else this.ring.push(text);
/// ```
///
/// `prepend` is set by backward kills (Ctrl+W, Ctrl+U), cleared by forward ones (Alt+D, Ctrl+K);
/// `accumulate` is the caller's `lastAction === "kill"`.
pub(crate) fn kill_ring_push(ring: &mut Vec<String>, text: &str, prepend: bool, accumulate: bool) {
    if text.is_empty() {
        return;
    }
    if accumulate && let Some(last) = ring.pop() {
        ring.push(if prepend {
            format!("{text}{last}")
        } else {
            format!("{last}{text}")
        });
        return;
    }
    ring.push(text.to_string());
}

/// `KillRing.rotate` (`kill-ring.ts:36-41`): move the last entry to the front, so a repeated
/// yank-pop cycles the ring. A no-op below two entries.
pub(crate) fn kill_ring_rotate(ring: &mut Vec<String>) {
    if ring.len() > 1
        && let Some(last) = ring.pop()
    {
        ring.insert(0, last);
    }
}

impl InputEditor {
    /// Push killed text onto the ring — [`kill_ring_push`], with cyrup's `append` being pi's
    /// `!prepend`. The `!text` early return is behaviour-preserving here: all four editor callers
    /// (`editor/edit.rs`) already return early on an empty range.
    pub(super) fn push_kill(&mut self, text: &str, append: bool) {
        kill_ring_push(
            &mut self.kill_ring,
            text,
            !append,
            self.last_action == LastAction::Kill,
        );
    }

    /// Yank the kill-ring top at the cursor (Ctrl+Y, `editor.ts:1852`).
    pub(super) fn yank(&mut self) {
        if let Some(top) = self.kill_ring.last().cloned() {
            for c in top.chars() {
                if c == '\n' {
                    self.insert_newline();
                } else {
                    self.insert_char(c);
                }
            }
        }
    }

    /// Yank-pop: only after a yank with ≥2 ring entries — delete the just-yanked text, rotate the
    /// ring, and insert the new top (Alt+Y, `editor.ts:1867`).
    pub(super) fn yank_pop(&mut self) {
        if self.last_action != LastAction::Yank || self.kill_ring.len() < 2 {
            return;
        }
        // Delete the previously-yanked text (the current ring top) backward from the cursor.
        if let Some(prev) = self.kill_ring.last().cloned() {
            let n = prev.chars().count();
            for _ in 0..n {
                self.backspace();
            }
        }
        // Rotate: move the top to the front (so a fresh top becomes current).
        kill_ring_rotate(&mut self.kill_ring);
        if let Some(top) = self.kill_ring.last().cloned() {
            for c in top.chars() {
                self.insert_char(c);
            }
        }
    }
}
