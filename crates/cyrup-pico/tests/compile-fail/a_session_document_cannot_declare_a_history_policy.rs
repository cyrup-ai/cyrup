//! `G-SCOPE-DETERMINES-LIFETIME`, the policy half / `spec.md:923-925`: *"only conversation documents
//! declare history and fork behavior."*
//!
//! Upstream states it as a sentence and encodes it as a TypeScript discriminated union whose
//! `{ scope: "session" }` arm simply has no `history` member — enforced in the token and
//! **re-read from storage where the union is not enforced** (ADR-0030 §2.3).
//!
//! Here `DocDef::POLICY` has type `<Self::Place as Placement>::Policy`, which is `()` for
//! `SessionScoped`. *"A session document declared rewindable"* is therefore a type error, `E0326`,
//! and not a validation rule anyone has to remember to run.

use cyrup_pico_store::{ConversationSemantics, RewindableFork};

struct Rewindable;

#[derive(serde::Serialize)]
struct Empty {}

impl cyrup_pico::DocDef for Rewindable {
    type Value = Empty;
    type Place = cyrup_pico::SessionScoped;
    type Shape = cyrup_pico::Singleton;
    type Seed = ();
    const KIND: &'static str = "test.rewindable";
    const VERSION: u32 = 1;
    // A session document has no history to declare: the type of this constant is `()`.
    const POLICY: ConversationSemantics = ConversationSemantics::Rewindable {
        fork: RewindableFork::AsOf,
    };
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

fn main() {}
