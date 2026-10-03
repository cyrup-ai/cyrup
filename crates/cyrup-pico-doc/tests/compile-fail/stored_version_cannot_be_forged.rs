//! `G-VERSION-PER-RECORD` — ADR-0030 §2.3 (`spec.md:1119-1120, 4366-4367`): *"a definition version
//! belongs to each stored record; a delta can never cross a version boundary."*
//!
//! The mechanism is the witness: *"a `StoredVersion` witness means a delta cannot claim a version it
//! did not read"* (F6 §C), and ADR-0030 §10's serde table withholds **both** directions — *"an
//! in-process witness. A `Deserialize` would let a recovered record mint a version claim."* So the
//! three ways to forge one are all compile errors: a struct literal (the `_seal: ()` field is
//! private), a decode, and an encode-then-decode round trip.

use cyrup_pico_doc::{DefVersion, StoredVersion};

fn main() {
    // No struct literal.
    let literal = StoredVersion {
        version: DefVersion::FIRST,
        _seal: (),
    };

    // No constructor.
    let minted = StoredVersion::mint(DefVersion::FIRST);

    // No `Deserialize`: a recovered record cannot mint a version claim.
    let decoded: StoredVersion = serde_json::from_str("1").expect("no Deserialize impl exists");

    // No `Serialize` either, so there is nothing to round-trip back through.
    let encoded = serde_json::to_string(&StoredVersion::mint(DefVersion::FIRST));

    let _ = (literal, minted, decoded, encoded);
}
