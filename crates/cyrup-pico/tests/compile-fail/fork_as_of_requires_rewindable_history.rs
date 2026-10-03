//! `G-SCOPE-DETERMINES-LIFETIME`, the fork half / `spec.md:1068-1069`: *"`fork: "asOf"` requires
//! `history: "rewindable"`."*
//!
//! Upstream lists this among §3.1's validation rules. ADR-0030 §2.3 classifies it
//! **unrepresentable in the token**, and the mechanism is that the two histories carry *different
//! fork types*: `ConversationSemantics::Latest` takes a `LatestFork`, which has no `AsOf` variant to
//! name at all.
//!
//! The failure it prevents is not cosmetic. `fork: "asOf"` on a current-only document asks a fork to
//! read the parent's value *at an entry*, and `spec.md:1413-1416` permits physical reclamation of a
//! current-only document's older records after a committed base — so the child's value would depend
//! on which records happened to survive.
//!
//! The error to expect is `E0599`/`E0433`: `LatestFork` has no `AsOf`.

use cyrup_pico_store::{ConversationSemantics, LatestFork};

struct Chatter;

#[derive(serde::Serialize)]
struct Empty {}

impl cyrup_pico::DocDef for Chatter {
    type Value = Empty;
    type Place = cyrup_pico::ConversationScoped;
    type Shape = cyrup_pico::Singleton;
    type Seed = ();
    const KIND: &'static str = "test.chatter";
    const VERSION: u32 = 1;
    const POLICY: ConversationSemantics = ConversationSemantics::Latest {
        fork: LatestFork::AsOf,
    };
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

fn main() {}
