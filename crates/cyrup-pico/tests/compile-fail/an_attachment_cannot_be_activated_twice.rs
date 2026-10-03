//! `spec.md:1515-1519`: a document state *"must acquire all of its document baselines and commit
//! subscription in one Session-line operation **so it never exposes a mixture from one commit**"*.
//!
//! `Attachment::activate` takes `self`, so the baseline and the registration are handed over exactly
//! once and a second activation is `E0382`. Two live states over one registration would share one
//! pending buffer, so each would take frames the other never sees — which is the mixture the sentence
//! forbids, arrived at from inside one consumer instead of across two.

use cyrup_pico::Attachment;

struct Live;

fn canary(attachment: Attachment<Live>) {
    let _state = attachment.activate();
    let _twice = attachment.activate();
}

fn main() {}
