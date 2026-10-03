//! `G-SCOPE-DETERMINES-LIFETIME`, the scope half / `spec.md:923-925`: *"scope directly determines
//! document ownership and lifetime."*
//!
//! `spec.md:1070-1071` makes a task document current-only, never copied by a conversation fork, and
//! retired when its task becomes terminal. A task document addressed in a conversation would get a
//! conversation's lifetime, which is the failure ADR-0030 §2.3 states as *"state private to a task
//! leaks across a session, or state a user expects to persist disappears."*
//!
//! Upstream keeps the six `tx.doc()` overloads apart by overload resolution. Here
//! `DocToken::in_conversation` is in an `impl` block bounded on `Place = ConversationScoped`, so a
//! task-scoped token has no such method: `E0599`.

use cyrup_pico::DocToken;
use cyrup_pico_store::ROOT_CONVERSATION_ID;

struct Pinned;

#[derive(serde::Serialize)]
struct Empty {}

impl cyrup_pico::DocDef for Pinned {
    type Value = Empty;
    type Place = cyrup_pico::TaskScoped;
    type Shape = cyrup_pico::Singleton;
    type Seed = ();
    const KIND: &'static str = "test.pinned";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

fn misplaced(token: &DocToken<Pinned>) {
    let _ = token.in_conversation(ROOT_CONVERSATION_ID);
}

fn main() {}
