//! `spec.md:3797`: `start()` *"installs the **sole** serialized asynchronous listener"*.
//!
//! Upstream enforces that with a runtime guard. `DocWatch::start` takes `self`, so a second
//! installation is `E0382` — use of a moved value — and the *"sole"* in the specification becomes the
//! signature. The reason it matters: two listeners on one watch would each see a subset of the frames
//! with no rule saying which, so a consumer could converge to a value no commit ever produced.

use cyrup_pico::{DocWatch, Frame, ListenerFailed};

struct Live;

async fn nothing(_frame: Frame<Live>) -> Result<(), ListenerFailed> {
    Ok(())
}

fn canary(watch: DocWatch<Live>) {
    let (_first, _delivery) = watch.start(nothing);
    let (_second, _again) = watch.start(nothing);
}

fn main() {}
