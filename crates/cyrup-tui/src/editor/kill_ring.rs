use super::*;

impl InputEditor {
    /// Push killed text onto the ring, accumulating into the top entry when the previous action was
    /// also a kill (prepend for backward kills, append for forward kills; `kill-ring.ts`).
    pub(super) fn push_kill(&mut self, text: &str, append: bool) {
        if self.last_action == LastAction::Kill
            && let Some(top) = self.kill_ring.last_mut() {
                if append {
                    top.push_str(text);
                } else {
                    *top = format!("{text}{top}");
                }
                return;
            }
        self.kill_ring.push(text.to_string());
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
        if let Some(top) = self.kill_ring.pop() {
            self.kill_ring.insert(0, top);
        }
        if let Some(top) = self.kill_ring.last().cloned() {
            for c in top.chars() {
                self.insert_char(c);
            }
        }
    }
}
