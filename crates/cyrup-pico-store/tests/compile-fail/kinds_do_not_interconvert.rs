//! ADR-0030 §1.5 and F6 §A: cross-kind confusion is `unrepresentable` in-process. Upstream applies
//! its brands with two bare casts (`ids.ts:4-6, 9-11`), so id 41 can be written as two record
//! types and a fork can rewind to another conversation's entry.

use cyrup_pico_store::{ConversationId, EntryId, TaskId};

fn needs_an_entry(_id: EntryId) {}

fn main() {
    let task: TaskId = serde_json::from_str("41").unwrap();
    let entry: EntryId = serde_json::from_str("41").unwrap();
    let conversation: ConversationId = serde_json::from_str("41").unwrap();

    needs_an_entry(task);

    // No conversion in any direction, and no comparison either.
    let _: EntryId = TaskId::from(entry);
    let _ = entry == conversation;
}
