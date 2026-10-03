//! A delivered frame's context carries the commit's **values** and cannot inherit its cancellation.
//!
//! `spec.md:3889`: *"The selected commit Context's values are preserved without inheriting the
//! producer's cancellation."* The hazard is concrete: a watch callback that inherited the producing
//! commit's token would be cancelled when the *committing* caller gave up, so a consumer's convergence
//! would depend on whether some unrelated task was still interested — and the commit is already
//! durable, so there is nothing left to cancel.
//!
//! `FrameContext` has no cancellation field, so it has no `token()` and no `is_cancelled()`. `E0599`
//! twice. This is ADR-0030 open question 6 held open safely: the type's *values* half may change when
//! cyrup grows a value-carrying context, and this half may not.

use cyrup_pico::FrameContext;

fn canary(context: &FrameContext) {
    let _token = context.token();
    let _gave_up = context.is_cancelled();
}

fn main() {}
