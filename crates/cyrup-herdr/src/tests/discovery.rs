//! The gate: `HerdrPane::discover` is `None` outside a herdr pane, and `None` means *nothing runs*.

use std::collections::HashMap;

use crate::env::{
    HERDR_ENV, HERDR_PANE_ID, HERDR_SOCKET_PATH, HERDR_TAB_ID, HERDR_WORKSPACE_ID, HerdrPane,
};
use crate::error::Unavailable;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn a_herdr_pane_exposes_every_injected_identifier() {
    let pane = HerdrPane::discover(&env(&[
        (HERDR_ENV, "1"),
        (HERDR_SOCKET_PATH, "/run/herdr/herdr.sock"),
        (HERDR_PANE_ID, "w1:p1"),
        (HERDR_TAB_ID, "w1:t1"),
        (HERDR_WORKSPACE_ID, "w1"),
    ]))
    .expect("HERDR_ENV=1 with a pane id is a herdr pane");

    assert_eq!(pane.pane_id(), "w1:p1");
    assert_eq!(pane.tab_id(), Some("w1:t1"));
    assert_eq!(pane.workspace_id(), Some("w1"));
    assert_eq!(
        pane.socket_path(),
        std::path::Path::new("/run/herdr/herdr.sock")
    );
}

/// herdr removes `HERDR_PANE_ID` for a `PaneLaunchIdentity::OmitPane` launch
/// (`tmp/herdr/src/pane.rs:170-172`) while leaving `HERDR_ENV=1` and `HERDR_SOCKET_PATH` in place.
/// Such a process is inside herdr and can see a live socket, but owns no pane — reporting agent
/// state against a pane id it does not have is the fabricated success this crate refuses.
#[test]
fn herdr_env_without_a_pane_id_is_not_a_pane() {
    assert!(
        HerdrPane::discover(&env(&[
            (HERDR_ENV, "1"),
            (HERDR_SOCKET_PATH, "/run/herdr/herdr.sock"),
        ]))
        .is_none(),
        "HERDR_ENV=1 alone is not a pane: herdr strips HERDR_PANE_ID on an OmitPane launch"
    );
    assert!(
        HerdrPane::discover(&env(&[
            (HERDR_ENV, "1"),
            (HERDR_PANE_ID, ""),
            (HERDR_SOCKET_PATH, "/run/herdr/herdr.sock"),
        ]))
        .is_none(),
        "an empty HERDR_PANE_ID is no pane id"
    );
}

#[test]
fn a_pane_id_without_herdr_env_is_not_a_pane() {
    assert!(
        HerdrPane::discover(&env(&[(HERDR_PANE_ID, "w1:p1")])).is_none(),
        "a stray HERDR_PANE_ID in the environment is not a herdr pane"
    );
    assert!(
        HerdrPane::discover(&env(&[(HERDR_ENV, "0"), (HERDR_PANE_ID, "w1:p1")])).is_none(),
        "HERDR_ENV must be exactly \"1\" (tmp/herdr/src/main.rs:4)"
    );
}

/// `HerdrPane::require` is the asked-for path — a user typed an inspector verb — and it names the
/// reason instead of returning `None`.
#[test]
fn require_names_the_reason_outside_a_pane() {
    let error = HerdrPane::require(&env(&[])).unwrap_err();
    assert!(matches!(
        error.unavailable(),
        Some(Unavailable::NotInHerdrPane)
    ));
    assert_eq!(
        error.to_string(),
        "not running inside a herdr pane (HERDR_ENV/HERDR_PANE_ID are not both set)"
    );
}
