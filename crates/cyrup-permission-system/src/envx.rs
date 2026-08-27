//! The ONE process-environment accessor for this crate.
//!
//! Production (`#[cfg(not(test))]`) is a bare `std::env::var`. Under `cfg(test)` the read first
//! consults a THREAD-LOCAL overlay, so a test pins a variable for its own thread without touching
//! the process environment.
//!
//! Why not a mutex around `set_var`: in edition 2024 `std::env::set_var`/`remove_var` are `unsafe`
//! because glibc's `setenv`/`unsetenv` may realloc and free the `environ` array while another
//! thread is inside `getenv`. A lock held by the WRITER cannot make a non-participating READER
//! safe, and this crate had sixteen non-participating readers. The hazard is undefined behaviour,
//! not merely a stale value, so the mutation is removed rather than scheduled.

#[cfg(not(test))]
#[must_use]
pub(crate) fn var(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

#[cfg(test)]
thread_local! {
    /// Innermost-wins stack of `(key, value)` pins; `None` means "pinned to unset".
    static OVERLAY: std::cell::RefCell<Vec<(String, Option<String>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
#[must_use]
pub(crate) fn var(key: &str) -> Option<String> {
    let pinned = OVERLAY
        .with_borrow(|stack| stack.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.clone()));
    match pinned {
        Some(value) => value,
        None => std::env::var(key).ok(),
    }
}

/// Pin `key` to `value` (`None` = unset) for the CURRENT THREAD until the guard drops.
///
/// No process state is mutated, so parallel tests never observe each other and no lock is needed.
///
/// **Constraint:** the pin is thread-local, so it does NOT reach work moved to another thread.
/// Keep the pinned body synchronous, or drive it on a `new_current_thread` runtime (`#[tokio::test]`
/// defaults to one). Never pin across a multi-thread runtime.
#[cfg(test)]
pub(crate) fn pin(key: &str, value: Option<&str>) -> EnvPin {
    OVERLAY.with_borrow_mut(|stack| stack.push((key.to_string(), value.map(str::to_string))));
    EnvPin
}

/// The RAII guard [`pin`] returns: dropping it pops the pin off the current thread's overlay.
#[cfg(test)]
pub(crate) struct EnvPin;

#[cfg(test)]
impl Drop for EnvPin {
    fn drop(&mut self) {
        OVERLAY.with_borrow_mut(|stack| {
            stack.pop();
        });
    }
}
