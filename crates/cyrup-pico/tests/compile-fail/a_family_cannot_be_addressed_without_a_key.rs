//! `G-SCOPE-DETERMINES-LIFETIME`, the shape half / `spec.md:4341-4342`: *"a missing key means the
//! singleton, not every family member."*
//!
//! A keyless address for a **family** is not a wildcard and not a mistake to catch at runtime — it is
//! a different address, the singleton's, at the same kind and scope. Upstream relies on
//! `tx.doc()`'s six overloads and `family: true` to keep the two apart; here a family definition's
//! only address constructors take a `FamilyKey`, so `DocToken::at` does not exist for it.
//!
//! The error to expect is `E0599` naming the trait bound that was not satisfied: `at` is in an
//! `impl` block bounded on `Shape = Singleton`.

use cyrup_pico::DocToken;

struct Members;

#[derive(serde::Serialize)]
struct Empty {}

impl cyrup_pico::DocDef for Members {
    type Value = Empty;
    type Place = cyrup_pico::SessionScoped;
    type Shape = cyrup_pico::Family;
    type Seed = ();
    const KIND: &'static str = "test.members";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

fn keyless(token: &DocToken<Members>) {
    // If this compiled, a family would silently share the singleton's address.
    let _ = token.at();
}

fn main() {}
