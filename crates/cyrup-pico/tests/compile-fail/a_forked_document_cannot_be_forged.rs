//! `G-FORK-POLICY-PERSISTED`'s *"never from the token"* half, and PICO5-PLAN S9's canary for it.
//!
//! `spec.md:1478-1479`: *"each conversation document follows the history/fork policy **persisted in its
//! `DocumentRecord`**"*, and `spec.md:1117-1120` says why the record and not the caller is the
//! authority: *"the record preserves scope and conversation history/fork semantics so unavailable
//! extension code does not make existing data disappear."*
//!
//! A [`cyrup_pico::ForkedDocument`] reports what the fork read off that record. If it could be
//! struct-literalled, a caller could assert a policy the record does not carry and hand it to code
//! that then behaves as though the fork had honoured it — the token-reinterprets-the-record hazard,
//! one layer out from where S5 closed it. Every field is private and there is no public constructor,
//! so the only `ForkedDocument` that exists is one a fork made.
//!
//! `E0451` (a private field in a struct literal) plus `E0063` (the rest are missing).

use cyrup_pico::ForkedDocument;
use cyrup_pico_store::RewindableFork;

fn forge() -> ForkedDocument {
    ForkedDocument {
        policy: RewindableFork::AsOf,
    }
}

fn main() {}
