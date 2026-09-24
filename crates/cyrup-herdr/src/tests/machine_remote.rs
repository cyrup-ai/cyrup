//! The saved-machine surface: the `herdr machine list --json` reader, the remote endpoint parse,
//! the remote command text, and the three placement verbs' wire shapes.
//!
//! The catalog reader is driven through a real process — a shell script in a tempdir that prints
//! herdr's own `MachineListRow` bytes (`tmp/herdr/src/cli/machine.rs:20-28`) — and every error
//! sentence is pi-subagents' (`src/runs/shared/herdr-machine.ts:98-133` @v0.68.0), byte for byte.

use std::path::{Path, PathBuf};

use crate::machine::{CatalogError, MachineProfile, parse_machine_catalog, read_machine_catalog};
use crate::schema::{
    AgentPromptParams, AgentPromptWaitOptions, AgentStartParams, AgentStatus, Method, Request,
    ResponseResult, TabCreateParams, WorkspaceCreateParams,
};

fn profile(id: &str, label: Option<&str>, target: &str, enabled: bool) -> MachineProfile {
    MachineProfile {
        id: id.to_owned(),
        label: label.map(str::to_owned),
        target: target.to_owned(),
        session: None,
        enabled,
    }
}

/// herdr prints `MachineListRow` (`id, label, target, session, enabled, selected`) as a pretty
/// array; pi also accepts `{machines: […]}`, skips rows without a usable `id`/`target`, trims
/// labels and sessions, and reads `enabled` as "anything but `false`".
#[test]
fn the_catalog_parses_herdrs_rows_with_pis_leniency() {
    let herdr = r#"[
  { "id": "m1", "label": " gpu-box ", "target": "me@gpu", "session": "default", "enabled": true, "selected": false },
  { "id": "m2", "label": "", "target": "me@cpu", "session": " work ", "enabled": false, "selected": true },
  { "id": "  ", "label": "blank-id", "target": "x" },
  { "id": "m3", "label": "no-target" },
  { "id": "m4", "target": "me@edge" },
  "not an object"
]"#;
    let parsed = parse_machine_catalog(herdr).unwrap();
    assert_eq!(
        parsed,
        vec![
            MachineProfile {
                session: Some("default".to_owned()),
                ..profile("m1", Some("gpu-box"), "me@gpu", true)
            },
            MachineProfile {
                session: Some("work".to_owned()),
                ..profile("m2", None, "me@cpu", false)
            },
            profile("m4", None, "me@edge", true),
        ]
    );
    let wrapped = parse_machine_catalog(r#"{"machines":[{"id":"a","target":"t"}]}"#).unwrap();
    assert_eq!(wrapped, vec![profile("a", None, "t", true)]);
}

#[test]
fn the_catalog_refusals_are_pis_sentences() {
    let malformed = parse_machine_catalog("nope").unwrap_err();
    assert!(
        malformed
            .to_string()
            .starts_with("Failed to parse herdr machine list --json: "),
        "{malformed}"
    );
    for no_list in [r#"{"machines":{}}"#, "42", r#"{"other":[]}"#] {
        assert_eq!(
            parse_machine_catalog(no_list).unwrap_err().to_string(),
            "herdr machine list --json returned no machine list."
        );
    }
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[tokio::test]
async fn the_catalog_is_read_from_the_binary_and_its_failures_are_named() {
    let dir = tempfile::tempdir().unwrap();
    let env = std::collections::HashMap::<String, String>::new();

    let ok = script(
        dir.path(),
        "herdr-ok",
        r#"[ "$1 $2 $3" = "machine list --json" ] || exit 9
echo '[{"id":"m1","label":"gpu-box","target":"me@gpu","session":"default","enabled":true,"selected":false}]'"#,
    );
    let rows = read_machine_catalog(Some(ok.to_str().unwrap()), &env)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows.first().and_then(|row| row.label.as_deref()),
        Some("gpu-box")
    );

    let failing = script(
        dir.path(),
        "herdr-fail",
        "echo 'machine catalog locked' >&2; exit 3",
    );
    assert_eq!(
        read_machine_catalog(Some(failing.to_str().unwrap()), &env)
            .await
            .unwrap_err()
            .to_string(),
        "herdr machine list --json exited with code 3: machine catalog locked"
    );

    let stdout_only = script(dir.path(), "herdr-stdout", "echo 'only stdout'; exit 4");
    assert_eq!(
        read_machine_catalog(Some(stdout_only.to_str().unwrap()), &env)
            .await
            .unwrap_err(),
        CatalogError::Exit {
            status: "4".to_owned(),
            output: "only stdout".to_owned()
        }
    );

    let missing = dir.path().join("no-such-herdr");
    assert_eq!(
        read_machine_catalog(Some(missing.to_str().unwrap()), &env)
            .await
            .unwrap_err()
            .to_string(),
        format!(
            "Herdr CLI '{}' was not found on PATH. Saved-machine placement needs Herdr installed locally.",
            missing.display()
        )
    );

    // `HERDR_BIN` is the ladder when no binary is named.
    let mut via_env = std::collections::HashMap::new();
    via_env.insert("HERDR_BIN".to_owned(), ok.display().to_string());
    assert_eq!(read_machine_catalog(None, &via_env).await.unwrap().len(), 1);
}

#[cfg(unix)]
mod remote {
    use crate::remote::{
        HERDR_REMOTE_PATH, HERDR_SSH_BASE, SshTransport, parse_endpoint, remote_shell_command,
        shell_quote_remote, ssh_env_command,
    };

    /// `herdr status server --json` (`tmp/herdr/src/cli/status.rs:262-275`).
    fn status(session: Option<&str>, running: bool, compatible: bool) -> String {
        serde_json::json!({
            "status": if running { "running" } else { "not_running" },
            "running": running,
            "version": "0.9.1",
            "protocol": 7,
            "capabilities": null,
            "compatible": compatible,
            "endpoint_compatible": true,
            "socket": "/home/me/.config/herdr/herdr.sock",
            "session": session,
            "restart_needed": false,
            "server_binary_stale": false
        })
        .to_string()
    }

    #[test]
    fn the_endpoint_parse_holds_pis_identity_rules() {
        let endpoint = parse_endpoint(&status(Some("default"), true, true), None).unwrap();
        assert_eq!(endpoint.socket, "/home/me/.config/herdr/herdr.sock");
        assert_eq!(endpoint.session, None, "`default` is the default session");
        assert_eq!(endpoint.protocol, 7);

        assert_eq!(
            parse_endpoint(&status(Some("work"), true, true), None)
                .unwrap_err()
                .to_string(),
            "Remote Herdr session identity mismatch: expected default, received work."
        );
        assert!(parse_endpoint(&status(Some("work"), true, true), Some("work")).is_ok());
        for (running, compatible) in [(false, true), (true, false)] {
            assert_eq!(
                parse_endpoint(&status(None, running, compatible), None)
                    .unwrap_err()
                    .to_string(),
                "The selected remote Herdr session is stopped or incompatible."
            );
        }
        assert_eq!(
            parse_endpoint("{", None).unwrap_err().to_string(),
            "Remote Herdr endpoint discovery returned malformed JSON."
        );
        assert_eq!(
            parse_endpoint("[]", None).unwrap_err().to_string(),
            "Remote Herdr endpoint discovery returned no endpoint."
        );
        assert_eq!(
            parse_endpoint(
                r#"{"socket":"relative.sock","version":"1","protocol":1,"running":true,"compatible":true}"#,
                None
            )
            .unwrap_err()
            .to_string(),
            "Remote Herdr endpoint discovery returned incomplete identity."
        );
    }

    /// `remoteShellCommand`, `sshEnvCommand` and `herdrSshArgs` (`herdr-connection.ts:15-28`):
    /// arguments never enter the script text, every quote survives, and the agent socket rides as
    /// `IdentityAgent=`.
    #[test]
    fn remote_commands_quote_their_arguments_out_of_the_script() {
        assert_eq!(shell_quote_remote("it's"), r"'it'\''s'");
        let command = remote_shell_command("cat \"$1\"", &["/tmp/a b", "x'y"]);
        assert_eq!(
            command,
            format!(
                "sh -c 'PATH=\"{HERDR_REMOTE_PATH}\"; export PATH; cat \"$1\"' sh '/tmp/a b' 'x'\\''y'"
            )
        );
        assert!(ssh_env_command(Some("work"), "true").starts_with(
            "/usr/bin/env -u HERDR_SOCKET_PATH -u HERDR_SESSION HERDR_SESSION='work' sh -c "
        ));
        assert!(
            ssh_env_command(Some("default"), "true")
                .starts_with("/usr/bin/env -u HERDR_SOCKET_PATH -u HERDR_SESSION sh -c ")
        );
        let transport = SshTransport::new("ssh", Some("/tmp/agent.sock".to_owned()));
        let args = transport.args();
        let (base, extra) = args.split_at(HERDR_SSH_BASE.len());
        assert_eq!(base, HERDR_SSH_BASE.as_slice());
        assert_eq!(extra, ["-o", "IdentityAgent=/tmp/agent.sock"]);
        assert!(args.iter().any(|arg| arg == "BatchMode=yes"));
        assert!(args.iter().any(|arg| arg == "SendEnv=-*"));
    }
}

/// The placement verbs' params land on the wire in herdr's spelling, and their answers decode
/// into the typed accessors (`tmp/herdr/src/api/schema/response.rs:57-106`).
#[test]
fn the_placement_verbs_round_trip() {
    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "CYRUP_SUBAGENTS_HERDR_RUN_ID".to_owned(),
        "run_1".to_owned(),
    );
    let line = serde_json::to_value(Request {
        id: "r".to_owned(),
        method: Method::TabCreate(TabCreateParams {
            workspace_id: Some("w1".to_owned()),
            cwd: Some("/srv/repo".to_owned()),
            focus: false,
            label: Some("subagent-12345678".to_owned()),
            env,
        }),
    })
    .unwrap();
    assert_eq!(
        line.get("params").cloned().unwrap(),
        serde_json::json!({"workspace_id":"w1","cwd":"/srv/repo","focus":false,"label":"subagent-12345678","env":{"CYRUP_SUBAGENTS_HERDR_RUN_ID":"run_1"}})
    );
    let workspace = serde_json::to_value(Method::WorkspaceCreate(WorkspaceCreateParams {
        cwd: Some("/srv".to_owned()),
        ..WorkspaceCreateParams::default()
    }))
    .unwrap();
    assert_eq!(
        workspace.get("params").cloned().unwrap(),
        serde_json::json!({"cwd":"/srv","focus":false})
    );
    let start = serde_json::to_value(Method::AgentStart(AgentStartParams {
        name: "codex-x".to_owned(),
        kind: "codex".to_owned(),
        pane_id: "w1:p2".to_owned(),
        args: vec!["--sandbox".to_owned(), "read-only".to_owned()],
        timeout_ms: Some(45_000),
    }))
    .unwrap();
    assert_eq!(
        start.get("params").cloned().unwrap(),
        serde_json::json!({"name":"codex-x","kind":"codex","pane_id":"w1:p2","args":["--sandbox","read-only"],"timeout_ms":45000})
    );
    let prompt = serde_json::to_value(Method::AgentPrompt(AgentPromptParams {
        target: "codex-x".to_owned(),
        text: "do it".to_owned(),
        wait: Some(AgentPromptWaitOptions {
            until: vec![AgentStatus::Idle, AgentStatus::Done, AgentStatus::Blocked],
            timeout_ms: Some(90_000),
        }),
    }))
    .unwrap();
    assert_eq!(
        prompt.pointer("/params/wait").cloned().unwrap(),
        serde_json::json!({"until":["idle","done","blocked"],"timeout_ms":90000})
    );

    let agent = serde_json::json!({"terminal_id":"t1","agent_status":"idle","workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2","focused":false,"revision":1});
    let pane = serde_json::json!({"pane_id":"w1:p2","terminal_id":"t1","workspace_id":"w1","tab_id":"w1:t2","focused":false,"agent_status":"idle","revision":1});
    let tab = serde_json::json!({"tab_id":"w1:t2","workspace_id":"w1","number":2,"label":"x","focused":false,"pane_count":1,"agent_status":"idle"});
    let ws = serde_json::json!({"workspace_id":"w1","number":1,"label":"cyrup-subagents-abc","focused":false,"pane_count":1,"tab_count":1,"active_tab_id":"w1:t2","agent_status":"idle"});

    let created: ResponseResult = serde_json::from_value(
        serde_json::json!({"type":"workspace_created","workspace":ws,"tab":tab,"root_pane":pane}),
    )
    .unwrap();
    let (workspace, _, root) = created.workspace_created("workspace.create").unwrap();
    assert_eq!(
        (workspace.workspace_id.as_str(), root.pane_id.as_str()),
        ("w1", "w1:p2")
    );
    let tab_created: ResponseResult = serde_json::from_value(
        serde_json::json!({"type":"tab_created","tab":tab,"root_pane":pane}),
    )
    .unwrap();
    assert_eq!(
        tab_created.tab_created("tab.create").unwrap().1.pane_id,
        "w1:p2"
    );
    let started: ResponseResult = serde_json::from_value(
        serde_json::json!({"type":"agent_started","agent":agent,"argv":["codex","--no-alt-screen"]}),
    )
    .unwrap();
    assert_eq!(
        started.agent_started("agent.start").unwrap().1,
        ["codex", "--no-alt-screen"]
    );
    let prompted: ResponseResult =
        serde_json::from_value(serde_json::json!({"type":"agent_prompted","agent":agent})).unwrap();
    assert_eq!(
        prompted.agent_prompted("agent.prompt").unwrap().terminal_id,
        "t1"
    );
    // A wrong answer is never taken for a right one.
    let wrong: ResponseResult = serde_json::from_value(serde_json::json!({"type":"ok"})).unwrap();
    assert!(wrong.tab_created("tab.create").is_err());
}
