//! The `codemode` tool on the wire: the real `cyrup` binary against an in-process fake OpenAI
//! chat-completions server (`support::fake_openai`), asserted at the REQUEST level (what a provider
//! receives) and at the RESULT level (the tool-result text the model gets back).
//!
//! WHY THIS FILE EXISTS. CODE-014 shipped an empty system prompt: the transcript replayed it, every
//! in-process test read it from the transcript, and no test looked at the bytes the provider
//! received. The in-process suites (`cyrup-session-svc`'s `codemode.rs`, the sandbox's own tests)
//! pin the parts; this pins the join, from argv and `settings.json` to the HTTP body and back. It
//! is the durable form of the live harness (`fake_openai.py` + `jsrun.py`) the codemode work was
//! verified with.
//!
//! RUNNING IT. Like every other seam test here it is gated behind the `it` feature and spawns the
//! binary `build.rs` resolves. Unix only: it signals and inspects real processes.
//!
//! ```text
//! # the supported route: a cleared environment, so the ambient-credential guards pass
//! cargo run -p xtask -- it --test bin -E 'test(codemode_wire)'
//! # the same, raw (a shell that exports provider credentials makes `support::env` red first)
//! cargo nextest run -p cyrup-it --features it --test bin -E 'test(codemode_wire)'
//! # reuse a binary you already built instead of linking a private copy; rebuild it first, a stale
//! # target/debug/cyrup tests the old code
//! cargo build -p cyrup
//! CYRUP_IT_BIN_DIR="$PWD/target/debug" cargo nextest run -p cyrup-it --features it --test bin -E 'test(codemode_wire)'
//! ```
//!
//! The scripts really run: V8 in the sandbox process, real built-in tools in a temp project, the
//! real permission and abort paths, a real stdio MCP server, a real subagent child. Nothing is
//! scripted but the model. Each test names the production behaviour it pins; the ones an earlier
//! fix produced fail again when that fix is reverted.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::support::fake_openai::{FakeOpenAi, Turn};

/// A scratch home for one or more launches: the agent directory and the project the binary runs
/// in. A second launch in the same home is a process restart over the same files.
struct Home {
    tmp: TempDir,
    agent_dir: PathBuf,
    project: PathBuf,
}

impl Home {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agent");
        let project = tmp.path().join("work");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        Self {
            tmp,
            agent_dir,
            project,
        }
    }

    fn write(root: &Path, files: &[(&str, &str)]) {
        for (path, body) in files {
            let path = root.join(path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, body).unwrap();
        }
    }
}

/// One launch of the real binary: what the model says, what `settings.json` holds, what argv adds,
/// and which files the agent directory and the project start with.
struct Wire<'a> {
    turns: Vec<Turn>,
    settings: Option<Value>,
    args: &'a [&'a str],
    files: &'a [(&'a str, &'a str)],
    agent_files: &'a [(&'a str, &'a str)],
    env: &'a [(&'a str, &'a str)],
    persist: bool,
    /// `--approve`, which every launch carries unless it asks to run in a project it has not trusted.
    approve: bool,
}

struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
    requests: Vec<Value>,
    elapsed: Duration,
}

impl<'a> Wire<'a> {
    /// The model calls `codemode` once with `code`, then answers `done`.
    fn script(code: &str) -> Self {
        Self::turns(vec![Turn::codemode(code), Turn::text("done")])
    }

    fn turns(turns: Vec<Turn>) -> Self {
        Self {
            turns,
            settings: None,
            args: &[],
            files: &[],
            agent_files: &[],
            env: &[],
            persist: false,
            approve: true,
        }
    }

    /// Launch without `--approve`: `-p` cannot ask whether to trust the project, so a project that
    /// has anything to gate (a permission policy of its own does) is not trusted.
    fn untrusted(mut self) -> Self {
        self.approve = false;
        self
    }

    fn settings(mut self, settings: Value) -> Self {
        self.settings = Some(settings);
        self
    }

    fn args(mut self, args: &'a [&'a str]) -> Self {
        self.args = args;
        self
    }

    /// Files created in the project before the launch.
    fn files(mut self, files: &'a [(&'a str, &'a str)]) -> Self {
        self.files = files;
        self
    }

    /// Files created in the agent directory before the launch (policy files, `mcp.json`).
    fn agent_files(mut self, files: &'a [(&'a str, &'a str)]) -> Self {
        self.agent_files = files;
        self
    }

    /// Environment variables added to the (otherwise cleared) child environment.
    fn env(mut self, env: &'a [(&'a str, &'a str)]) -> Self {
        self.env = env;
        self
    }

    /// Keep the session on disk, so a later launch in the same [`Home`] can continue it.
    fn persist(mut self) -> Self {
        self.persist = true;
        self
    }

    fn run(self) -> Outcome {
        self.run_in(&Home::new())
    }

    fn run_in(self, home: &Home) -> Outcome {
        let (mut cmd, server) = self.command(home, true);
        let started = Instant::now();
        let out = cmd.output().expect("spawn cyrup");
        let elapsed = started.elapsed();
        Outcome {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            requests: server.requests(),
            elapsed,
        }
    }

    /// The configured `Command` and the server it talks to. `print` adds `-p go`; without it the
    /// caller drives the process itself (`--mode rpc`).
    fn command(self, home: &Home, print: bool) -> (Command, FakeOpenAi) {
        let server = FakeOpenAi::start(self.turns);
        server.write_models_json(&home.agent_dir);
        if let Some(settings) = self.settings {
            std::fs::write(home.agent_dir.join("settings.json"), settings.to_string()).unwrap();
        }
        Home::write(&home.agent_dir, self.agent_files);
        Home::write(&home.project, self.files);

        // Hermetic by construction: `env_clear` + allowlist (see `support::env`).
        let mut cmd = crate::support::env::hermetic(crate::support::bins::cyrup(), home.tmp.path());
        cmd.current_dir(&home.project)
            .env("CYRUP_AGENT_DIR", &home.agent_dir)
            .args(["--offline", "--provider", "fake", "--model", "m1"])
            .args(if self.approve {
                &["--approve"][..]
            } else {
                &[][..]
            })
            .args(if self.persist {
                &[][..]
            } else {
                &["--no-session"][..]
            })
            .args(self.args)
            .envs(self.env.iter().copied());
        if print {
            cmd.args(["-p", "go"]);
        }
        // A closed stdin: `cyrup -p` waits for EOF on an open pipe.
        cmd.stdin(Stdio::null());
        (cmd, server)
    }
}

impl Outcome {
    /// Everything a failing assertion needs: the exit, both streams and a line per request with the
    /// tools it declared and the last message it carried.
    fn context(&self) -> String {
        let requests: Vec<String> = self
            .requests
            .iter()
            .enumerate()
            .map(|(n, request)| {
                let last = request["messages"]
                    .as_array()
                    .and_then(|messages| messages.last())
                    .map(|m| {
                        let text = content_text(&m["content"]);
                        format!(
                            "{}: {}",
                            m["role"],
                            text.chars().take(300).collect::<String>()
                        )
                    })
                    .unwrap_or_default();
                format!("  [{n}] tools {:?}; last {last}", self.tool_names(n))
            })
            .collect();
        format!(
            "exit {}\nstdout: {}\nstderr: {}\nrequests: {}\n{}",
            self.code,
            self.stdout,
            self.stderr,
            self.requests.len(),
            requests.join("\n")
        )
    }

    fn request(&self, n: usize) -> &Value {
        self.requests
            .get(n)
            .unwrap_or_else(|| panic!("no request {n}\n{}", self.context()))
    }

    /// The first message of request `n`, which must be the system prompt.
    fn system(&self, n: usize) -> String {
        let first = &self.request(n)["messages"][0];
        assert!(
            matches!(first["role"].as_str(), Some("system" | "developer")),
            "request {n} must open with the system prompt, got {first}\n{}",
            self.context()
        );
        content_text(&first["content"])
    }

    /// The names in request `n`'s `tools` array, in order.
    fn tool_names(&self, n: usize) -> Vec<String> {
        self.request(n)["tools"]
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|t| t["function"]["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn tool(&self, n: usize, name: &str) -> &Value {
        self.request(n)["tools"]
            .as_array()
            .and_then(|tools| tools.iter().find(|t| t["function"]["name"] == name))
            .map(|t| &t["function"])
            .unwrap_or_else(|| panic!("request {n} declares no tool {name}\n{}", self.context()))
    }

    /// The text of the LAST tool-result message in request `n`: the result of the call just made
    /// (a resumed session replays earlier ones before it).
    fn tool_result(&self, n: usize) -> String {
        self.request(n)["messages"]
            .as_array()
            .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "tool"))
            .map(|m| content_text(&m["content"]))
            .unwrap_or_default()
    }
}

impl Outcome {
    /// What the script produced: the tool result after its `Script completed|failed` header.
    fn script_output(&self) -> String {
        let result = self.tool_result(1);
        match result.split_once("Output:\n") {
            Some((_, body)) => body.trim().to_owned(),
            None => panic!("no script header in {result:?}\n{}", self.context()),
        }
    }

    /// The script's JSON return value.
    fn script_json(&self) -> Value {
        let body = self.script_output();
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("script output {body:?}: {e}"))
    }

    /// The run reached the model twice: the script call, then the answer after its result.
    fn assert_survived(&self) {
        assert_eq!(self.code, 0, "{}", self.context());
        assert_eq!(self.requests.len(), 2, "{}", self.context());
        assert!(self.stdout.contains("done"), "{}", self.context());
    }
}

/// The URLs of the image parts the messages of `request` carry (OpenAI `image_url` parts, which is
/// how a tool result's pictures reach the model). The script source in the assistant's tool call is
/// deliberately not searched: it holds the same picture, as text.
fn image_parts(request: &Value) -> Vec<String> {
    request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter(|part| part["type"] == "image_url")
        .filter_map(|part| part["image_url"]["url"].as_str().map(str::to_owned))
        .collect()
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

// ======================================================================================== request --

/// CODE-014 / PROMPT-001: the first message of the first request is the system prompt, with the
/// tools and the rules, and `codemode` is among them. It shipped EMPTY: the transcript replayed the
/// prompt, every in-process test read it from there, and the adapters (which read only
/// `Context::system_prompt`) sent `""`.
#[test]
fn the_first_request_opens_with_the_system_prompt_its_tools_and_its_rules() {
    let o = Wire::script("return 1")
        .args(&["--tools", "read,bash,codemode"])
        .run();
    o.assert_survived();
    let system = o.system(0);
    for part in [
        "You are a coding assistant operating inside cyrup",
        "<tools>",
        "- codemode: Run JavaScript that calls other tools",
        "<rules>",
        "- Use codemode to batch independent tool calls",
        "<cwd>",
    ] {
        assert!(system.contains(part), "{part:?} missing from:\n{system}");
    }
    assert_eq!(system.matches("<tools>").count(), 1, "{system}");
    assert_eq!(o.request(0)["messages"][1]["role"], "user");
    assert_eq!(o.tool_names(0), ["read", "bash", "codemode"]);
    // The next request carries the same prompt first, once, not rebuilt per turn.
    assert_eq!(o.system(1), system);
}

/// `--system-prompt` and `--append-system-prompt` reach the provider (they were dropped with the
/// rest of the prompt).
#[test]
fn system_prompt_flags_reach_the_provider() {
    let o = Wire::script("return 1")
        .args(&[
            "--tools",
            "read,codemode",
            "--system-prompt",
            "BASE-SENTINEL you are a test agent",
            "--append-system-prompt",
            "TAIL-SENTINEL always say done",
        ])
        .run();
    o.assert_survived();
    let system = o.system(0);
    let base = system.find("BASE-SENTINEL").expect("--system-prompt text");
    let tail = system
        .find("TAIL-SENTINEL")
        .expect("--append-system-prompt text");
    assert!(
        base < tail,
        "the appended text follows the base prompt:\n{system}"
    );
    assert!(
        !system.contains("You are a coding assistant operating inside cyrup"),
        "a custom prompt replaces the default preamble:\n{system}"
    );
}

/// `defaultTools` from `settings.json` alone (no flag) activates `codemode`; without it the tool is
/// registered but not offered.
#[test]
fn settings_default_tools_activate_codemode() {
    let off = Wire::script("return 1").run();
    assert!(
        !off.tool_names(0).contains(&"codemode".to_owned()),
        "codemode is inactive by default: {:?}",
        off.tool_names(0)
    );

    let on = Wire::script("return 1")
        .settings(json!({ "defaultTools": ["+codemode"] }))
        .run();
    on.assert_survived();
    let names = on.tool_names(0);
    for expected in ["read", "bash", "edit", "write", "codemode"] {
        assert!(
            names.contains(&expected.to_owned()),
            "{expected} in {names:?}"
        );
    }
    assert!(
        on.system(0).contains("- codemode: Run JavaScript"),
        "{}",
        on.system(0)
    );
}

/// `codemode.mode` from `settings.json`: `on` leaves the tools declared, `only` declares `codemode`
/// alone and lists the others in its description.
#[test]
fn settings_codemode_mode_decides_what_the_provider_is_offered() {
    let settings = |mode: &str| json!({ "defaultTools": ["read", "bash", "codemode"], "codemode": { "mode": mode } });
    let on = Wire::script("return 1").settings(settings("on")).run();
    // Extension tools (`mcp`, `ask_user_question`) are active next to the named ones.
    let on_names = on.tool_names(0);
    for expected in ["read", "bash", "codemode"] {
        assert!(on_names.contains(&expected.to_owned()), "{on_names:?}");
    }
    let on_description = on.tool(0, "codemode")["description"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        !on_description.contains("### `read`"),
        "a declared tool is not listed again:\n{on_description}"
    );

    let only = Wire::script("return 1").settings(settings("only")).run();
    only.assert_survived();
    assert_eq!(only.tool_names(0), ["codemode"]);
    let description = only.tool(0, "codemode")["description"]
        .as_str()
        .unwrap()
        .to_owned();
    for part in [
        "### `read`",
        "### `bash`",
        "declare const tools: { read(args",
        "declare const tools: { bash(args",
    ] {
        assert!(
            description.contains(part),
            "{part:?} missing from:\n{description}"
        );
    }
    // The hidden tools' prompt rules move to their sections: the system prompt keeps only codemode.
    let system = only.system(0);
    assert!(!system.contains("- read: Read file contents"), "{system}");
    assert!(system.contains("- codemode: Run JavaScript"), "{system}");
}

/// `--tools` is an allowlist and `-xt` a denylist, enforced on the registry: what the provider is
/// offered AND what a script can call.
#[test]
fn tools_and_exclude_tools_bound_the_declarations_and_the_scripts() {
    let probe = "return [Object.keys(tools).sort(), ALL_TOOLS.map(t => t.name).sort()]";

    let allow = Wire::script(probe)
        .args(&["--tools", "read,codemode"])
        .run();
    allow.assert_survived();
    assert_eq!(allow.tool_names(0), ["read", "codemode"]);
    assert_eq!(
        allow.script_json(),
        json!([["read"], ["read"]]),
        "{}",
        allow.context()
    );

    let deny = Wire::script(probe)
        .args(&["--tools", "read,bash,edit,codemode", "-xt", "bash,edit"])
        .run();
    deny.assert_survived();
    assert_eq!(deny.tool_names(0), ["read", "codemode"]);
    assert_eq!(
        deny.script_json(),
        json!([["read"], ["read"]]),
        "{}",
        deny.context()
    );

    // The permission system re-shapes the active tools every turn; a flag-excluded tool stays out
    // of its candidates (once it did not: the first request declared bash, edit, write ...).
    let policy = json!({ "defaultPolicy": {
        "tools": "allow", "bash": "allow", "mcp": "allow", "skills": "allow", "special": "allow",
    } })
    .to_string();

    // `-xt` alone, over the default set, with that policy installed: the excluded built-ins must
    // not come back through the registry (`--exclude-tools bash` was bypassed the same way).
    let excluded = Wire::script(probe)
        .settings(json!({ "defaultTools": ["read", "bash", "edit", "codemode"] }))
        .args(&["-xt", "bash,edit"])
        .agent_files(&[(POLICY_FILE, &policy)])
        .run();
    excluded.assert_survived();
    let declared = excluded.tool_names(0);
    for gone in ["bash", "edit"] {
        assert!(
            !declared.iter().any(|n| n == gone),
            "{gone} is excluded: {declared:?}\n{}",
            excluded.context()
        );
    }
    for kept in ["read", "codemode"] {
        assert!(declared.iter().any(|n| n == kept), "{kept}: {declared:?}");
    }
    let [callable, all]: [Vec<String>; 2] = serde_json::from_value(excluded.script_json()).unwrap();
    for list in [&callable, &all] {
        assert!(
            !list.iter().any(|n| n == "bash" || n == "edit") && list.iter().any(|n| n == "read"),
            "a script sees the excluded tools: {list:?}"
        );
    }

    let armed = Wire::script(probe)
        .args(&["--tools", "read,codemode"])
        .agent_files(&[(POLICY_FILE, &policy)])
        .run();
    armed.assert_survived();
    // The permission system re-derives the active set from the registry, so the ORDER is its own;
    // what the flag bounds is the set, and the order must not change between the turns of a run.
    let mut armed_names = armed.tool_names(0);
    armed_names.sort();
    assert_eq!(armed_names, ["codemode", "read"], "{}", armed.context());
    assert_eq!(
        armed.tool_names(1),
        armed.tool_names(0),
        "{}",
        armed.context()
    );
    assert_eq!(
        armed.script_json(),
        json!([["read"], ["read"]]),
        "{}",
        armed.context()
    );
}

/// The declaration the model sees: a function tool whose only argument is the script.
#[test]
fn the_codemode_tool_declares_its_schema_and_points_at_the_docs() {
    let o = Wire::script("return 1")
        .args(&["--tools", "read,codemode"])
        .run();
    o.assert_survived();
    let tool = o.tool(0, "codemode");
    assert_eq!(tool["parameters"]["type"], "object");
    assert_eq!(tool["parameters"]["properties"]["code"]["type"], "string");
    assert_eq!(tool["parameters"]["required"], json!(["code"]));
    let description = tool["description"].as_str().unwrap();
    for part in [
        "Run JavaScript that calls other tools",
        "@options",
        "timeout_ms",
        "searchTools",
        "describeTool",
    ] {
        assert!(
            description.contains(part),
            "{part:?} missing from:\n{description}"
        );
    }
}

// ========================================================================================= result --

/// Every built-in, called from a script through real V8, against a real project: the structured
/// results the declarations promise, and the effects on disk.
#[test]
fn every_builtin_tool_runs_from_a_script() {
    let home = Home::new();
    let o = Wire::script(
        "const w = await tools.write({ path: 'new.txt', content: 'one\\ntwo\\n' });\n\
         const r = await tools.read({ path: 'new.txt' });\n\
         const e = await tools.edit({ path: 'new.txt', edits: [{ oldText: 'two', newText: 'three' }] });\n\
         const g = await tools.grep({ pattern: 'three', path: '.' });\n\
         const f = await tools.find({ pattern: '*.txt' });\n\
         const l = await tools.ls({});\n\
         const b = await tools.bash({ command: 'cat new.txt; echo err >&2; exit 3' });\n\
         return { w, r, e, g, f, l, b };",
    )
    .args(&["--tools", "read,bash,edit,write,grep,find,ls,codemode"])
    .files(&[("a.txt", "hello world\n"), ("src/main.rs", "fn main() {}\n")])
    .run_in(&home);
    o.assert_survived();
    let result = o.script_json();
    assert_eq!(
        result["w"],
        "Successfully wrote to new.txt",
        "{}",
        o.context()
    );
    assert_eq!(result["r"], "one\ntwo\n");
    assert_eq!(result["e"], "Successfully replaced 1 block(s) in new.txt.");
    assert_eq!(result["g"], "new.txt:2: three");
    assert_eq!(result["f"], "a.txt\nnew.txt");
    assert_eq!(result["l"], "a.txt\nnew.txt\nsrc/");
    // A non-zero exit resolves to the structured result instead of rejecting the script.
    assert_eq!(result["b"]["exit_code"], 3);
    assert_eq!(result["b"]["output"], "one\nthree\nerr\n");
    assert_eq!(result["b"]["truncated"], false);
    assert!(result["b"]["wall_time_seconds"].is_number());
    assert_eq!(
        std::fs::read_to_string(home.project.join("new.txt")).unwrap(),
        "one\nthree\n",
        "the edit really happened on disk"
    );
}

/// A throwing script: the model gets a failed result with the partial output and the error's stack
/// (`codemode.js:LINE:COLUMN`), and the session carries on.
#[test]
fn a_script_error_reaches_the_model_with_its_stack_and_partial_output() {
    let o = Wire::script("text('before');\nthrow new Error('boom')")
        .args(&["--tools", "read,codemode"])
        .run();
    o.assert_survived();
    let result = o.tool_result(1);
    assert!(result.starts_with("Script failed\nWall time "), "{result}");
    assert!(result.contains("before"), "{result}");
    assert!(
        result.contains("Script error:\nError: boom\n    at codemode.js:2:"),
        "{result}"
    );
    assert!(result.contains("No tool calls were made."), "{result}");
}

/// `timeout_ms` is a hard deadline, tool calls included; the bash command it was waiting on is
/// killed with the script.
#[test]
fn the_timeout_option_stops_a_script_waiting_on_a_tool_and_kills_its_process() {
    let home = Home::new();
    let o = Wire::script(
        "// @options: {\"timeout_ms\": 500}\nawait tools.bash({ command: 'sleep 30 & echo $! > pid; wait' })",
    )
    .args(&["--tools", "bash,codemode"])
    .run_in(&home);
    o.assert_survived();
    let result = o.tool_result(1);
    assert!(result.starts_with("Script failed\nWall time "), "{result}");
    assert!(
        result.contains("Script timed out: Execution timed out after 500 ms"),
        "{result}"
    );
    assert!(result.contains("bash (cancelled)"), "{result}");
    assert!(
        o.elapsed < Duration::from_secs(20),
        "the 30 s sleep was not waited for: {:?}",
        o.elapsed
    );
    assert_process_gone(&home.project.join("pid"));
}

/// One allocation V8 cannot satisfy aborts the engine; it takes the script's sandbox process down,
/// not the session: the model gets a failed result and the run completes (it used to SIGTRAP the
/// whole `cyrup` after 7 s).
#[test]
fn a_huge_allocation_fails_the_script_and_not_the_session() {
    let o = Wire::script("text('started'); const a = new Array(2 ** 27).fill(0); return a.length")
        .args(&["--tools", "bash,codemode"])
        .run();
    o.assert_survived();
    let result = o.tool_result(1);
    assert!(result.starts_with("Script failed\nWall time "), "{result}");
    assert!(
        result.contains("started"),
        "output before the failure is kept: {result}"
    );
    assert!(
        result.contains("InternalError: out of memory")
            || result.contains("Script sandbox failed: The sandbox process crashed"),
        "{result}"
    );
}

/// `Promise.all` over real tools: every call answers, and no more than 16 of a script's nested calls
/// run at once (3000 parallel reads once held the process at 3.4 GB).
#[test]
fn parallel_nested_calls_all_answer_and_are_bounded() {
    let o = Wire::script(
        "const out = await Promise.all(Array.from({ length: 48 }, (_, i) => tools.bash({\n\
           command: `mkdir -p slots; mkdir slots/${i}; sleep 0.4; ls slots | wc -l; rmdir slots/${i}`,\n\
         })));\n\
         return out.map(r => Number(r.output.trim()));",
    )
    .args(&["--tools", "bash,codemode"])
    .run();
    o.assert_survived();
    let counts: Vec<u64> = serde_json::from_value(o.script_json()).unwrap();
    assert_eq!(counts.len(), 48, "{}", o.context());
    let peak = counts.iter().copied().max().unwrap();
    assert!(peak >= 2, "the calls really ran in parallel, peak {peak}");
    assert!(
        peak <= 16,
        "nested calls in flight are capped at 16, peak {peak}"
    );
}

/// A script's output past `max_output_tokens` keeps its start and end, and the full text goes to a
/// temp file the result names.
#[test]
fn large_output_is_truncated_for_the_model_and_spilled_to_a_file() {
    let o = Wire::script("text('x'.repeat(300000)); return 'tail-marker'")
        .args(&["--tools", "read,codemode"])
        .run();
    o.assert_survived();
    let result = o.tool_result(1);
    assert!(
        result.contains("Warning: truncated output"),
        "{result:.200}"
    );
    assert!(
        result.len() < 60_000,
        "the model got {} bytes",
        result.len()
    );
    let path = result
        .split("[Full output: ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .unwrap_or_else(|| panic!("no spill path in {:?}", &result[result.len() - 200..]));
    assert_private(path);
    let full = std::fs::read_to_string(path).unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        full.contains(&"x".repeat(300_000)) && full.contains("tail-marker"),
        "{} bytes",
        full.len()
    );
}

/// The arguments of a script's unsettled calls are bounded by what they weigh (CODE-042): a loop of
/// calls with 1 MiB arguments ends with a `RangeError` the script can catch after 31 of them, where
/// 5000 once held the host past a gigabyte (1055 MB peak, and the sandbox process dead after 82 s).
/// The same loop without a `catch` fails the script at once.
#[test]
fn a_loop_of_calls_with_large_arguments_ends_at_the_argument_limit() {
    let o = Wire::script(
        "const big = 'x'.repeat(1 << 20);\n\
         const calls = [];\n\
         try { for (let i = 0; i < 5000; i++) calls.push(tools.read({ path: 'a.txt', pad: big })); }\n\
         catch (e) { text(`${e.name} after ${calls.length}: ${e.message.slice(0, 80)}`); }\n\
         await Promise.all(calls);\n\
         for (;;) tools.read({ path: 'a.txt', pad: big });",
    )
    .args(&["--tools", "read,codemode"])
    .files(&[("a.txt", "hello\n")])
    .run();
    o.assert_survived();
    let result = o.tool_result(1);
    assert!(
        result.starts_with("Script failed\nWall time "),
        "{result:.600}"
    );
    assert!(
        result.contains("RangeError after 31: The calls in flight hold arguments that weigh "),
        "{result:.600}"
    );
    // The loop that does not catch ends the script with the same error.
    assert!(
        result
            .contains("Script error:\nRangeError: The calls in flight hold arguments that weigh "),
        "{result:.900}"
    );
    assert!(
        o.elapsed < Duration::from_secs(60),
        "the loop was not stopped by the limit: {:?}",
        o.elapsed
    );
}

/// A returned value of 16 million characters reaches the model as the text the script wrote
/// (CODE-043): the host keeps it as text and prints it, where it used to read it into a parsed value
/// that held 0.7 GB for an array of 1.25 million small objects. The model gets the start and end; the
/// whole text is in the spill file.
#[test]
fn a_returned_array_of_a_million_small_objects_is_shown_as_the_text_it_was() {
    let o = Wire::script("return Array.from({ length: 1250000 }, (_, i) => ({ a: i }));")
        .args(&["--tools", "read,codemode"])
        .run();
    o.assert_survived();
    let result = o.tool_result(1);
    assert!(
        result.starts_with("Script completed\nWall time "),
        "{result:.300}"
    );
    assert!(
        result.contains("Warning: truncated output"),
        "{result:.300}"
    );
    assert!(
        result.contains(r#"[{"a":0},{"a":1},{"a":2},"#),
        "{result:.300}"
    );
    let path = result
        .split("[Full output: ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .unwrap_or_else(|| panic!("no spill path in {:?}", &result[result.len() - 200..]));
    assert_private(path);
    let full = std::fs::read_to_string(path).unwrap();
    std::fs::remove_file(path).unwrap();
    // The spill file holds the output text and nothing else: 16388891 characters of JSON.
    assert_eq!(full.len(), 16_388_891);
    assert!(
        full.starts_with(r#"[{"a":0},"#) && full.ends_with(r#"{"a":1249998},{"a":1249999}]"#),
        "{:?} .. {:?}",
        &full[..40],
        &full[full.len() - 40..]
    );
}

/// `image()` from a real script: the model's next request carries the picture, after the label that
/// names the file it was saved to.
#[test]
fn an_image_shown_by_a_script_reaches_the_next_request() {
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";
    let o = Wire::script(&format!(
        "image('data:image/png;base64,{PNG}'); return 'shown'"
    ))
    .args(&["--tools", "read,codemode"])
    .run();
    o.assert_survived();
    let result = o.tool_result(1);
    let path = result
        .split("[Image saved to ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .unwrap_or_else(|| panic!("no saved-image label in {result:?}"));
    assert_private(path);
    std::fs::remove_file(path).unwrap();
    assert_eq!(
        image_parts(o.request(1)),
        [format!("data:image/png;base64,{PNG}")],
        "the picture travels to the model as an image part: {}",
        o.request(1)["messages"]
    );
}

/// Delete the pictures a result text says it saved (`[Image saved to <path> (...)]`): a script's
/// `image()` writes a temp file, and a test that does not look at it must still not leave it behind.
fn forget_saved_images(text: &str) {
    for part in text.split("[Image saved to ").skip(1) {
        if let Some(path) = part.split(' ').next() {
            drop(std::fs::remove_file(path));
        }
    }
}

/// The file exists and only its owner can read it: a script's output may hold anything it read.
fn assert_private(path: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("{path} does not exist: {e}"))
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "{path} is mode {:o}", mode & 0o777);
}

/// The process whose pid a script wrote into `pidfile` is not running. The pid is a background
/// child of the command's shell (`sleep N & echo $! > pid; wait`), so only killing the command's
/// whole process group removes it. A zombie awaiting its reaper counts as gone: a container's
/// init does not always collect orphans promptly, and a zombie runs nothing.
fn assert_process_gone(pidfile: &Path) {
    let pid = std::fs::read_to_string(pidfile)
        .unwrap_or_else(|e| panic!("the script never started its process: {e}"))
        .trim()
        .to_owned();
    let running = || {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            // `pid (comm) S ...`: the state follows the last `)` (comm may itself contain one).
            stat.rsplit_once(')')
                .and_then(|(_, rest)| rest.trim_start().chars().next())
                .is_some_and(|state| state != 'Z' && state != 'X')
        })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!running(), "process {pid} is still running");
}

// ================================================================================== permissions ==

const POLICY_FILE: &str = "cyrup-permissions.jsonc";

/// A tool the policy denies outright is not in `tools` at all; the other tools still are.
#[test]
fn a_tool_the_policy_denies_is_not_callable_from_a_script() {
    let policy = json!({
        "defaultPolicy": { "tools": "allow", "bash": "allow" },
        "tools": { "bash": "deny" },
    })
    .to_string();
    let o = Wire::script("return [Object.keys(tools).sort(), 'bash' in tools]")
        .args(&["--tools", "read,bash,codemode"])
        .agent_files(&[(POLICY_FILE, &policy)])
        .run();
    o.assert_survived();
    assert_eq!(o.script_json(), json!([["read"], false]), "{}", o.context());
}

/// A command rule that denies one command rejects that nested call with the policy's text, while
/// another command through the same tool runs.
#[test]
fn a_command_rule_rejects_one_nested_call_and_lets_another_run() {
    let policy = json!({
        "defaultPolicy": { "tools": "allow", "bash": "allow" },
        "bash": { "rm *": "deny" },
    })
    .to_string();
    let o = Wire::script(
        "const ok = await tools.bash({ command: 'echo fine' });\n\
         let denied;\n\
         try { await tools.bash({ command: 'rm -rf nothing' }) } catch (e) { denied = e.message }\n\
         return { ok: ok.output, denied };",
    )
    .args(&["--tools", "bash,codemode"])
    .agent_files(&[(POLICY_FILE, &policy)])
    .run();
    o.assert_survived();
    let result = o.script_json();
    assert_eq!(result["ok"], "fine\n", "{}", o.context());
    let denied = result["denied"]
        .as_str()
        .unwrap_or_else(|| panic!("the rm call was not refused: {result}\n{}", o.context()));
    assert!(
        denied.contains("is not permitted to run 'bash' command 'rm -rf nothing' (matched 'rm *')"),
        "{denied}"
    );
}

/// An `ask` rule with nobody to ask (`-p`) becomes a block, and the text says the call came from a
/// script and what to do about it.
#[test]
fn an_ask_rule_blocks_a_nested_call_when_there_is_no_ui_to_answer() {
    let policy = json!({
        "defaultPolicy": { "tools": "ask", "bash": "ask", "mcp": "ask", "skills": "ask", "special": "ask" },
        "tools": { "codemode": "allow" },
    })
    .to_string();
    let o = Wire::script(
        "try { await tools.bash({ command: 'ls' }); return 'ran' } catch (e) { return e.message }",
    )
    .args(&["--tools", "bash,codemode"])
    .agent_files(&[(POLICY_FILE, &policy)])
    .run();
    o.assert_survived();
    let message = o.script_output();
    assert!(
        message.contains("Running bash command 'ls' requires approval, but no interactive UI is available (from codemode script)."),
        "{message}"
    );
    assert!(
        message.contains("allow this call in the permission policy"),
        "{message}"
    );
}

/// The same `ask` rule with a person to answer (RPC `extension_ui_request`): the dialog names the
/// nested call and says it came from a script, "Allow Once" lets the call run and "Reject" refuses it.
#[test]
fn an_ask_rule_puts_the_nested_call_to_the_person_and_their_answer_decides_it() {
    let policy = json!({
        "defaultPolicy": { "tools": "ask", "bash": "ask", "mcp": "ask", "skills": "ask", "special": "ask" },
        "tools": { "codemode": "allow" },
    })
    .to_string();
    for (answer, runs) in [("Allow Once", true), ("Reject", false)] {
        let home = Home::new();
        let (cmd, _server) = Wire::script(
            "try { const r = await tools.bash({ command: 'echo asked > ran.txt' }); return 'ran:' + r.exit_code } catch (e) { return 'refused:' + e.message }",
        )
        .args(&["--mode", "rpc", "--tools", "bash,codemode"])
        .agent_files(&[(POLICY_FILE, &policy)])
        .command(&home, false);
        let mut rpc = Lines::spawn(cmd);
        rpc.send(&json!({ "type": "prompt", "message": "go", "id": "p1" }));
        let dialog = rpc.wait_for_frame("the permission dialog", Duration::from_secs(30), |f| {
            f["type"] == "extension_ui_request" && f["method"] == "select"
        });
        let title = dialog["title"].as_str().unwrap();
        assert!(
            title.contains("echo asked > ran.txt") && title.contains("(from codemode script)"),
            "the dialog names the call and where it came from: {title}"
        );
        assert_eq!(
            dialog["options"],
            json!([
                "Allow Once",
                "Allow Always",
                "Reject",
                "Reject with Reason",
                "Reject All From This Script"
            ]),
            "a call a script made also offers to refuse the rest of the script"
        );
        rpc.send(&json!({
            "type": "extension_ui_response", "id": dialog["id"], "value": answer,
        }));
        let settled = rpc.wait_for_frame("the codemode result", Duration::from_secs(30), |f| {
            f["type"] == "tool_execution_end" && f["toolName"] == "codemode"
        });
        let text = content_text(&settled["result"]["content"]);
        if runs {
            assert!(text.contains("ran:0"), "{answer}: {text}");
            assert_eq!(
                std::fs::read_to_string(home.project.join("ran.txt")).unwrap(),
                "asked\n"
            );
        } else {
            assert!(text.contains("refused:"), "{answer}: {text}");
            assert!(
                !home.project.join("ran.txt").exists(),
                "a rejected call must not run"
            );
        }
    }
}

/// A script that starts several calls together under `bash = ask` is one dialog per call, and the only
/// way out used to be answering each (measured in the TUI: Esc rejected one and the next appeared at
/// once, 25 times over). "Reject All From This Script" refuses the call on screen and every other call
/// of that script, the ones waiting behind the dialog included, with no further dialog.
#[test]
fn rejecting_the_script_in_the_dialog_refuses_the_rest_of_its_calls_without_more_dialogs() {
    let policy = json!({
        "defaultPolicy": { "tools": "ask", "bash": "ask", "mcp": "ask", "skills": "ask", "special": "ask" },
        "tools": { "codemode": "allow" },
    })
    .to_string();
    let home = Home::new();
    let (cmd, _server) = Wire::script(
        "const settled = await Promise.allSettled([1, 2, 3].map((n) => tools.bash({ command: 'echo ' + n + ' > ran' + n + '.txt' }))); return settled.map((r) => r.status + ':' + (r.reason ? r.reason.message : '')).join('|')",
    )
    .args(&["--mode", "rpc", "--tools", "bash,codemode"])
    .agent_files(&[(POLICY_FILE, &policy)])
    .command(&home, false);
    let mut rpc = Lines::spawn(cmd);
    rpc.send(&json!({ "type": "prompt", "message": "go", "id": "p1" }));
    let dialog = rpc.wait_for_frame("the permission dialog", Duration::from_secs(30), |f| {
        f["type"] == "extension_ui_request" && f["method"] == "select"
    });
    rpc.send(&json!({
        "type": "extension_ui_response", "id": dialog["id"], "value": "Reject All From This Script",
    }));
    let settled = rpc.wait_for_frame("the codemode result", Duration::from_secs(30), |f| {
        f["type"] == "tool_execution_end" && f["toolName"] == "codemode"
    });
    let text = content_text(&settled["result"]["content"]);
    assert_eq!(
        text.matches("rejected:").count(),
        3,
        "all three calls were refused: {text}"
    );
    assert!(
        text.contains("the user rejected this script's tool calls"),
        "{text}"
    );
    for n in 1..=3 {
        assert!(
            !home.project.join(format!("ran{n}.txt")).exists(),
            "a refused call must not run"
        );
    }
    let dialogs = rpc
        .seen
        .iter()
        .filter(|f| f["type"] == "extension_ui_request" && f["method"] == "select")
        .count();
    assert_eq!(
        dialogs,
        1,
        "one dialog for the whole script:\n{}",
        rpc.dump()
    );
}

/// The script reference the `codemode` description sends the model to lives under the agent
/// directory, outside the project. With an armed policy the external-directory guard asked about it
/// (a block under `-p`) or refused it outright (`Hard stop ... do not retry this path`), so the model
/// was told to read a file its own policy denied. A `read` of exactly that page skips the guard, in a
/// script and for the model alike, and the read rule still decides; any other path outside the
/// project is still guarded.
#[test]
fn the_codemode_docs_the_description_points_at_are_readable_under_an_armed_policy() {
    for external in ["ask", "deny"] {
        let policy = json!({
            "tools": { "codemode": "allow", "read": "allow" },
            "external_directory": external,
        })
        .to_string();
        let home = Home::new();
        let page = home.agent_dir.join("docs").join("codemode.md");
        let secret = home.agent_dir.join("auth.json");
        std::fs::write(&secret, "{}").unwrap();
        let o = Wire::script(&format!(
            "const page = await tools.read({{ path: {page:?} }});\n\
             let other;\n\
             try {{ await tools.read({{ path: {secret:?} }}); other = 'read' }} catch (e) {{ other = e.message }}\n\
             return {{ start: String(page).slice(0, 10), other }};",
            page = page.to_string_lossy(),
            secret = secret.to_string_lossy(),
        ))
        .args(&["--tools", "read,codemode"])
        .agent_files(&[(POLICY_FILE, &policy)])
        .run_in(&home);
        o.assert_survived();
        let result = o.script_json();
        assert_eq!(
            result["start"],
            "# Codemode",
            "external_directory {external}: {}",
            o.context()
        );
        let other = result["other"].as_str().unwrap();
        assert!(
            other.contains("outside the working directory") || other.contains("external directory"),
            "external_directory {external}: another file in the agent directory is still guarded: {other}"
        );
    }
}

/// A project the user has not trusted can only tighten the policy. A repository whose only `.cyrup`
/// content is a policy that `allow`s `bash` is a project with something to gate, so `-p` without
/// `--approve` does not trust it, and the user's own `ask` stands: the script's call is refused for
/// want of anybody to ask. The project's `deny` stands too. With `--approve` the same project's
/// `allow` applies.
#[test]
fn an_untrusted_project_cannot_allow_what_the_users_policy_asks_about() {
    let global = json!({
        "defaultPolicy": { "tools": "ask", "bash": "ask", "mcp": "ask", "skills": "ask", "special": "ask" },
        "tools": { "codemode": "allow" },
    })
    .to_string();
    let project = json!({ "bash": { "echo *": "allow", "rm *": "deny" } }).to_string();
    let script = "const out = {};\n\
         for (const command of ['echo hi', 'rm -rf nothing']) {\n\
           try { const r = await tools.bash({ command }); out[command] = 'ran:' + r.exit_code }\n\
           catch (e) { out[command] = e.message }\n\
         }\n\
         return out;";
    let files = [(".cyrup/agent/cyrup-permissions.jsonc", project.as_str())];

    let untrusted = Wire::script(script)
        .args(&["--tools", "bash,codemode"])
        .agent_files(&[(POLICY_FILE, &global)])
        .files(&files)
        .untrusted()
        .run();
    untrusted.assert_survived();
    let result = untrusted.script_json();
    let echo = result["echo hi"].as_str().unwrap();
    assert!(
        echo.contains("requires approval, but no interactive UI is available"),
        "an untrusted project's allow was honoured: {echo}\n{}",
        untrusted.context()
    );
    let rm = result["rm -rf nothing"].as_str().unwrap();
    assert!(
        rm.contains("is not permitted to run 'bash' command 'rm -rf nothing'"),
        "an untrusted project's deny still applies: {rm}"
    );

    let trusted = Wire::script(script)
        .args(&["--tools", "bash,codemode"])
        .agent_files(&[(POLICY_FILE, &global)])
        .files(&files)
        .run();
    trusted.assert_survived();
    assert_eq!(
        trusted.script_json()["echo hi"],
        "ran:0",
        "a trusted project's allow applies\n{}",
        trusted.context()
    );
}

/// A script whose first call waits on a dialog, and an RPC client that never answers it. Returns once
/// the dialog is open. The server and the home are returned so they outlive the test.
fn rpc_waiting_on_a_dialog(
    code: &str,
    tools: &str,
    policy: &Value,
    agent_files: &[(&str, &str)],
) -> (Home, Lines, FakeOpenAi, Value) {
    let policy = policy.to_string();
    let mut files = vec![(POLICY_FILE, policy.as_str())];
    files.extend_from_slice(agent_files);
    let home = Home::new();
    let (cmd, server) = Wire::script(code)
        .args(&["--mode", "rpc", "--tools", tools])
        .agent_files(&files)
        .command(&home, false);
    let mut rpc = Lines::spawn(cmd);
    rpc.send(&json!({ "type": "prompt", "message": "go", "id": "p1" }));
    let dialog = rpc.wait_for_frame("the dialog", Duration::from_secs(30), |f| {
        f["type"] == "extension_ui_request" && f["method"] == "select"
    });
    (home, rpc, server, dialog)
}

/// [`rpc_waiting_on_a_dialog`] for the permission dialog: every tool but `codemode` asks.
fn rpc_waiting_on_the_permission_dialog(code: &str) -> (Home, Lines, FakeOpenAi, Value) {
    let policy = json!({
        "defaultPolicy": { "tools": "ask", "bash": "ask", "mcp": "ask", "skills": "ask", "special": "ask" },
        "tools": { "codemode": "allow" },
    });
    rpc_waiting_on_a_dialog(code, "bash,codemode", &policy, &[])
}

/// `timeout_ms` is a deadline for the whole script, a call waiting for a person included. The
/// dialog stays open, unanswered, and the script still ends on time: the call is recorded as
/// cancelled and the run goes on to the model. A permission prompt that held its caller until it was
/// answered (the dialog is a blocking bridge, and the supervisor of a script polls a call's first
/// step itself) kept the script alive for as long as the person took.
#[test]
fn timeout_ms_ends_a_script_whose_call_waits_for_an_answer_to_the_permission_dialog() {
    let (_home, mut rpc, server, _dialog) = rpc_waiting_on_the_permission_dialog(
        "// @options: {\"timeout_ms\": 1500}\nawait tools.bash({ command: 'echo never > ran.txt' })",
    );
    let settled = rpc.wait_for_frame(
        "the codemode result, with the dialog still unanswered",
        Duration::from_secs(20),
        |f| f["type"] == "tool_execution_end" && f["toolName"] == "codemode",
    );
    let text = content_text(&settled["result"]["content"]);
    assert!(
        text.contains("Script timed out: Execution timed out after 1500 ms"),
        "{text}"
    );
    assert!(text.contains("bash (cancelled)"), "{text}");
    rpc.wait_for_frame("the run to end", Duration::from_secs(20), |f| {
        f["type"] == "agent_end"
    });
    assert_eq!(server.requests().len(), 2, "the model got the result");
}

/// The adapter's own approval dialog (`settings.approveTools`) is the other dialog a script's call can
/// wait on, and it holds its caller the same way: a script's MCP call that needs an answer must not
/// outlive `timeout_ms`. The permission policy allows the tool (a direct MCP tool is a `tools` entry),
/// so the only question put to the person is the adapter's; the options asserted below say which
/// dialog opened.
#[test]
fn timeout_ms_ends_a_script_whose_mcp_call_waits_for_an_answer_to_the_mcp_approval_dialog() {
    let mut mcp: Value = serde_json::from_str(&mcp_json(json!(true))).unwrap();
    mcp["settings"] = json!({ "approveTools": true });
    let policy = json!({
        "defaultPolicy": { "tools": "allow", "bash": "ask", "mcp": "allow", "skills": "ask", "special": "ask" },
        "tools": { "codemode": "allow" },
    });
    let (_home, mut rpc, server, dialog) = rpc_waiting_on_a_dialog(
        "// @options: {\"timeout_ms\": 1500}\nawait tools.fixture_echo({ text: 'never' })",
        "read,codemode,fixture_*",
        &policy,
        &[("mcp.json", &mcp.to_string())],
    );
    assert_eq!(
        dialog["options"],
        json!(["Allow once", "Allow for session", "Deny"]),
        "this is the adapter's dialog, not the permission gate's: {dialog}"
    );
    let settled = rpc.wait_for_frame(
        "the codemode result, with the dialog still unanswered",
        Duration::from_secs(20),
        |f| f["type"] == "tool_execution_end" && f["toolName"] == "codemode",
    );
    let text = content_text(&settled["result"]["content"]);
    assert!(
        text.contains("Script timed out: Execution timed out after 1500 ms"),
        "{text}"
    );
    assert!(text.contains("fixture_echo (cancelled)"), "{text}");
    rpc.wait_for_frame("the run to end", Duration::from_secs(20), |f| {
        f["type"] == "agent_end"
    });
    assert_eq!(server.requests().len(), 2, "the model got the result");
}

/// An RPC `abort` while a script's call waits for the dialog: the reply comes, the run ends, and
/// the call is recorded as cancelled. Without the fix the reply waited for a person to answer.
#[test]
fn an_rpc_abort_settles_while_a_nested_call_waits_for_an_answer_to_the_permission_dialog() {
    let (home, mut rpc, _server, _dialog) = rpc_waiting_on_the_permission_dialog(
        "await tools.bash({ command: 'echo never > ran.txt' })",
    );
    rpc.send(&json!({ "type": "abort", "id": "a1" }));
    let reply = rpc.wait_for_frame("the abort reply", Duration::from_secs(20), |f| {
        f["type"] == "response" && f["id"] == "a1"
    });
    assert_eq!(reply["success"], true, "{reply}");
    if !rpc.seen.iter().any(|f| f["type"] == "agent_end") {
        rpc.wait_for_frame("the run to end", Duration::from_secs(20), |f| {
            f["type"] == "agent_end"
        });
    }
    let settled = rpc
        .seen
        .iter()
        .find(|f| f["type"] == "tool_execution_end" && f["toolName"] == "codemode")
        .unwrap_or_else(|| panic!("the codemode call never settled:\n{}", rpc.dump()))
        .clone();
    assert_eq!(
        settled["result"]["details"]["calls"][0]["status"], "cancelled",
        "{settled}"
    );
    assert!(
        !home.project.join("ran.txt").exists(),
        "an unanswered dialog must not let the call run"
    );
}

/// RPC `new_session` while a script's call waits for the dialog: it replaces the session at once,
/// where it used to wait for a person to answer a question the old session no longer needed.
#[test]
fn a_new_session_does_not_wait_for_the_permission_dialog_of_a_nested_call() {
    let (_home, mut rpc, _server, _dialog) = rpc_waiting_on_the_permission_dialog(
        "await tools.bash({ command: 'echo never > ran.txt' })",
    );
    rpc.send(&json!({ "type": "new_session", "id": "n1" }));
    let reply = rpc.wait_for_frame("the new_session reply", Duration::from_secs(20), |f| {
        f["type"] == "response" && f["id"] == "n1"
    });
    assert_eq!(reply["success"], true, "{reply}");
    assert_eq!(reply["data"]["cancelled"], false, "{reply}");
}

// =========================================================================== abort and restart ==

/// SIGTERM in the middle of a script that waits on a real `bash`: the run ends without a further
/// model request and the command it was running is killed. The handler exits 143 after disposing
/// the runtime (pi `print-mode.ts:50-64`), but the abort also ends the in-flight turn, whose
/// `-p` flow returns exit 1 for an aborted last message (`print-mode.ts` `exitCode = 1`) — pi has
/// the same two paths racing, so either code is the contract; the assertions that matter are that
/// the process ends, no second request is made and the command is gone.
#[test]
fn sigterm_mid_script_kills_the_running_bash_and_ends_the_run() {
    let home = Home::new();
    let (mut cmd, server) =
        Wire::script("await tools.bash({ command: 'sleep 60 & echo $! > pid; wait' })")
            .args(&["--tools", "bash,codemode"])
            .command(&home, true);
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cyrup");
    let pidfile = home.project.join("pid");
    wait_for(
        || pidfile.exists(),
        Duration::from_secs(30),
        "the script's bash to start",
    );
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("run kill");
    assert!(status.success());
    let deadline = Instant::now() + Duration::from_secs(20);
    let code = loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => break status.code(),
            None if Instant::now() >= deadline => {
                drop(child.kill());
                panic!("cyrup still running 20 s after SIGTERM");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    assert!(
        matches!(code, Some(143 | 1)),
        "exit code after SIGTERM was {code:?}"
    );
    assert_process_gone(&pidfile);
    assert_eq!(
        server.requests().len(),
        1,
        "the run did not go on to a second request"
    );
}

/// A spawned `cyrup` that speaks JSON lines on stdio (`--mode rpc`, `--acp`): its stdout is read on
/// a thread, every frame read is kept in `seen`, and the process is killed when the value drops.
struct Lines {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Value>,
    seen: Vec<Value>,
}

impl Lines {
    fn spawn(mut cmd: Command) -> Self {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn cyrup");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, rx) = channel::<Value>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && tx.send(value).is_err()
                {
                    return;
                }
            }
        });
        Self {
            stdin: child.stdin.take(),
            child,
            rx,
            seen: Vec::new(),
        }
    }

    fn send(&mut self, frame: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{frame}").expect("write frame");
        stdin.flush().expect("flush frame");
    }

    /// Read frames until one satisfies `matches`; every frame read is kept in `seen`.
    fn wait_for_frame(
        &mut self,
        what: &str,
        limit: Duration,
        matches: impl Fn(&Value) -> bool,
    ) -> Value {
        let deadline = Instant::now() + limit;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left.max(Duration::from_millis(1))) {
                Ok(frame) => {
                    self.seen.push(frame.clone());
                    if matches(&frame) {
                        return frame;
                    }
                }
                Err(_) => panic!(
                    "timed out waiting for {what}; frames so far:\n{}",
                    self.dump()
                ),
            }
        }
    }

    fn dump(&self) -> String {
        self.seen
            .iter()
            .map(|f| {
                let s = f.to_string();
                s.chars().take(500).collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Drop for Lines {
    fn drop(&mut self) {
        drop(self.stdin.take());
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

/// RPC `abort` in the middle of a script that waits on a real `bash`: the `codemode` result settles
/// with the call recorded as cancelled, and the command is killed.
#[test]
fn an_rpc_abort_settles_the_running_nested_call_as_cancelled_and_kills_the_command() {
    let home = Home::new();
    let (cmd, _server) =
        Wire::script("await tools.bash({ command: 'sleep 60 & echo $! > pid; wait' })")
            .args(&["--mode", "rpc", "--tools", "bash,codemode"])
            .command(&home, false);
    let mut rpc = Lines::spawn(cmd);
    rpc.send(&json!({ "type": "prompt", "message": "go", "id": "p1" }));
    let pidfile = home.project.join("pid");
    wait_for(
        || pidfile.exists(),
        Duration::from_secs(30),
        "the script's bash to start",
    );
    rpc.send(&json!({ "type": "abort", "id": "a1" }));
    rpc.wait_for_frame("the run to end", Duration::from_secs(20), |f| {
        f["type"] == "agent_end"
    });
    let settled = rpc
        .seen
        .iter()
        .find(|f| f["type"] == "tool_execution_end" && f["toolName"] == "codemode")
        .unwrap_or_else(|| panic!("the codemode call never settled:\n{}", rpc.dump()))
        .clone();
    let calls = &settled["result"]["details"]["calls"];
    assert_eq!(calls[0]["name"], "bash", "{settled}");
    assert_eq!(calls[0]["status"], "cancelled", "{settled}");
    assert_process_gone(&pidfile);
}

fn wait_for(mut ready: impl FnMut() -> bool, limit: Duration, what: &str) {
    let deadline = Instant::now() + limit;
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A second process over the same session: `store()` values, the system prompt (once), the earlier
/// turns, the nested-call record and the `codemode` declaration all come back, and a script reads
/// what the first wrote.
#[test]
fn a_resumed_session_keeps_the_store_the_prompt_the_nested_calls_and_the_declaration() {
    let home = Home::new();
    // A `-p` run writes no session file unless asked: `-c` creates one when there is none.
    let first =
        Wire::script("await tools.read({ path: 'a.txt' }); store('k', { n: 7 }); return 'stored'")
            .args(&["--tools", "read,codemode", "-c"])
            .files(&[("a.txt", "hello\n")])
            .persist()
            .run_in(&home);
    first.assert_survived();

    let second = Wire::script("return load('k')")
        .args(&["--tools", "read,codemode", "-c"])
        .persist()
        .run_in(&home);
    second.assert_survived();
    assert_eq!(
        second.script_json(),
        json!({ "n": 7 }),
        "{}",
        second.context()
    );
    assert_eq!(second.tool_names(0), ["read", "codemode"]);
    // The declaration is rebuilt byte for byte (a provider's prompt cache keys on it).
    assert_eq!(
        second.tool(0, "codemode"),
        first.tool(0, "codemode"),
        "the codemode declaration changed across the restart"
    );
    let messages = second.request(0)["messages"].as_array().unwrap().clone();
    let roles: Vec<&str> = messages.iter().filter_map(|m| m["role"].as_str()).collect();
    assert_eq!(roles.first(), Some(&"system"), "{roles:?}");
    assert_eq!(
        roles.iter().filter(|r| **r == "system").count(),
        1,
        "the prompt is replayed once, not per resumed turn: {roles:?}"
    );
    assert_eq!(
        roles,
        ["system", "user", "assistant", "tool", "assistant", "user"],
        "the first process's turns, then this prompt"
    );
    assert!(second.system(0).contains("- codemode: Run JavaScript"));
    assert!(
        content_text(&messages[3]["content"]).contains("stored"),
        "the first run's script result is in the history"
    );

    // A third process, over RPC, reads the transcript back: the first script's nested `read` is
    // still recorded on its result (it does not travel on the provider wire, only in the session).
    let (cmd, _server) = Wire::turns(Vec::new())
        .args(&["--mode", "rpc", "--tools", "read,codemode", "-c"])
        .persist()
        .command(&home, false);
    let mut rpc = Lines::spawn(cmd);
    rpc.send(&json!({ "type": "get_messages", "id": "m1" }));
    let reply = rpc.wait_for_frame("get_messages", Duration::from_secs(30), |f| {
        f["type"] == "response" && f["id"] == "m1"
    });
    let result = reply["data"]["messages"]
        .as_array()
        .and_then(|all| {
            all.iter()
                .find(|m| m["role"] == "toolResult" && m["toolName"] == "codemode")
        })
        .unwrap_or_else(|| panic!("no codemode result in the resumed transcript: {reply}"));
    let nested = &result["nestedCalls"];
    assert_eq!(nested["calls"][0]["name"], "read", "{result}");
    assert_eq!(nested["calls"][0]["status"], "ok", "{result}");
    assert_eq!(nested["complete"], true, "{result}");
}

// ===================================================================================== json mode ==

/// `--mode json` from a real run: the settled `codemode` event carries the nested calls, the error
/// text and the image the script showed.
#[test]
fn json_mode_reports_the_nested_calls_the_error_and_the_image() {
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";
    let o = Wire::script(&format!(
        "await tools.read({{ path: 'a.txt' }});\nimage('data:image/png;base64,{PNG}');\nthrow new Error('late')"
    ))
    .args(&["--tools", "read,codemode", "--mode", "json"])
    .files(&[("a.txt", "hello\n")])
    .run();
    assert_eq!(o.code, 0, "{}", o.context());
    let events: Vec<Value> = o
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let settled = events
        .iter()
        .find(|e| e["type"] == "tool_execution_end" && e["toolName"] == "codemode")
        .unwrap_or_else(|| panic!("no settled codemode event in {}", o.stdout));
    assert_eq!(settled["isError"], true, "{settled}");
    let calls = &settled["result"]["details"]["calls"];
    assert_eq!(calls[0]["name"], "read", "{settled}");
    assert_eq!(calls[0]["status"], "ok", "{settled}");
    let content = settled["result"]["content"].as_array().unwrap();
    let text: String = content.iter().filter_map(|c| c["text"].as_str()).collect();
    forget_saved_images(&text);
    assert!(text.starts_with("Script failed"), "{text}");
    assert!(text.contains("Script error:\nError: late"), "{text}");
    let image = content
        .iter()
        .find(|c| c["type"] == "image")
        .unwrap_or_else(|| panic!("no image block in {content:?}"));
    assert_eq!(image["mimeType"], "image/png");
    assert_eq!(image["data"], PNG);
    // The same result is what the model was sent.
    assert!(
        o.tool_result(1).starts_with("Script failed"),
        "{}",
        o.context()
    );
}

/// `--acp` from a real run: the client is shown the one call the model made (`codemode`), failed,
/// with the script's error as its content; the nested `read` is not a tool call of its own, and the
/// record of it and the image travel in the call's raw output.
#[test]
fn acp_shows_the_codemode_call_and_its_failure_and_not_the_nested_calls() {
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";
    let home = Home::new();
    let (cmd, _server) = Wire::script(&format!(
        "await tools.read({{ path: 'a.txt' }});\nimage('data:image/png;base64,{PNG}');\nthrow new Error('late')"
    ))
    .args(&["--acp", "--tools", "read,codemode"])
    .files(&[("a.txt", "hello\n")])
    .command(&home, false);
    let mut acp = Lines::spawn(cmd);
    let limit = Duration::from_secs(45);
    acp.send(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": 1, "clientCapabilities": {} },
    }));
    acp.wait_for_frame("initialize", limit, |f| f["id"] == 1);
    acp.send(&json!({
        "jsonrpc": "2.0", "id": 2, "method": "session/new",
        "params": { "cwd": home.project, "mcpServers": [] },
    }));
    let created = acp.wait_for_frame("session/new", limit, |f| f["id"] == 2);
    let session = created["result"]["sessionId"].as_str().unwrap().to_owned();
    acp.send(&json!({
        "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
        "params": { "sessionId": session, "prompt": [{ "type": "text", "text": "go" }] },
    }));
    let done = acp.wait_for_frame("the prompt response", limit, |f| f["id"] == 3);
    assert!(done.get("error").is_none(), "{done}");

    let updates: Vec<&Value> = acp
        .seen
        .iter()
        .map(|f| &f["params"]["update"])
        .filter(|u| u.is_object())
        .collect();
    let announced: Vec<&str> = updates
        .iter()
        .filter(|u| u["sessionUpdate"] == "tool_call")
        .filter_map(|u| u["title"].as_str())
        .collect();
    assert_eq!(
        announced,
        ["codemode"],
        "only the model's call is shown:\n{}",
        acp.dump()
    );
    let last = updates
        .iter()
        .rev()
        .find(|u| u["sessionUpdate"] == "tool_call_update" && u["status"] == "failed")
        .unwrap_or_else(|| panic!("no failed tool_call_update:\n{}", acp.dump()));
    let shown = last["content"].to_string();
    forget_saved_images(&last["rawOutput"].to_string());
    assert!(shown.contains("Script failed"), "{shown}");
    assert!(shown.contains("Error: late"), "{shown}");
    let raw = &last["rawOutput"];
    assert_eq!(raw["details"]["calls"][0]["name"], "read", "{raw}");
    let image = raw["content"]
        .as_array()
        .and_then(|c| c.iter().find(|b| b["type"] == "image"))
        .unwrap_or_else(|| panic!("no image block in rawOutput: {raw}"));
    assert_eq!(image["data"], PNG);
}

// ========================================================================================== MCP ==

/// A real stdio MCP server as an `sh` script (the runtime `live_tool_call.rs` uses): `echo` answers
/// `echoed:<text>`, `pic` is listed so a catalogue has more than one tool.
const MCP_SERVER: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      pv=$(printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}\n' "$id" "$pv" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"echo back","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}},{"name":"pic","description":"a picture","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      text=$(printf '%s' "$line" | sed -n 's/.*"text":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echoed:%s"}],"isError":false}}\n' "$id" "$text" ;;
    *) if [ -n "$id" ]; then printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"; fi ;;
  esac
done
"#;

/// `mcp.json` naming [`MCP_SERVER`]; `direct_tools` is the adapter's `directTools` (`true` registers
/// the server's tools eagerly, `"search"` leaves them to `searchTools`).
fn mcp_json(direct_tools: Value) -> String {
    json!({ "mcpServers": { "fixture": {
        "command": "sh",
        "args": ["-c", MCP_SERVER],
        "directTools": direct_tools,
        "lifecycle": "keep-alive",
    } } })
    .to_string()
}

/// Eager MCP tools named by `--tools` are callable from a script, and a call resolves to the
/// server's own `CallToolResult`.
#[test]
fn eager_mcp_tools_are_callable_from_a_script() {
    let mcp = mcp_json(json!(true));
    let o = Wire::script(
        "return [ALL_TOOLS.map(t => t.name).filter(n => n.startsWith('fixture')).sort(),\n\
                 await tools.fixture_echo({ text: 'hi' })]",
    )
    .args(&["--tools", "read,codemode,fixture_*"])
    .agent_files(&[("mcp.json", &mcp)])
    .run();
    o.assert_survived();
    assert_eq!(
        o.script_json(),
        json!([
            ["fixture_echo", "fixture_pic"],
            { "content": [{ "type": "text", "text": "echoed:hi" }], "isError": false }
        ]),
        "{}",
        o.context()
    );
}

/// Search-mode MCP tools are not declared (in either mode); a script finds them with `searchTools`,
/// reads their declaration with `describeTool` and calls them.
#[test]
fn search_mode_mcp_tools_are_found_described_and_called_from_a_script() {
    let mcp = mcp_json(json!("search"));
    for mode in ["on", "only"] {
        let o = Wire::script(
            "const found = (await searchTools('echo')).map(t => t.name);\n\
             const declaration = await describeTool('fixture_echo');\n\
             const called = await tools.fixture_echo({ text: 'hi' });\n\
             return { found, declaration, called };",
        )
        .args(&["--tools", "read,codemode"])
        .settings(json!({ "codemode": { "mode": mode } }))
        .agent_files(&[("mcp.json", &mcp)])
        .run();
        o.assert_survived();
        let declared = o.tool_names(0);
        let description = o.tool(0, "codemode")["description"].as_str().unwrap();
        if mode == "only" {
            assert_eq!(declared, ["codemode"], "{}", o.context());
            assert!(description.contains("### `read`"), "{description}");
        } else {
            assert_eq!(declared, ["read", "codemode"], "{}", o.context());
        }
        assert!(
            !description.contains("fixture_echo"),
            "{mode}: a search-mode tool is not in the declaration:\n{description}"
        );
        let result = o.script_json();
        assert_eq!(
            result["found"],
            json!(["fixture_echo"]),
            "{mode}: {}",
            o.context()
        );
        let declaration = result["declaration"].as_str().unwrap();
        assert!(
            declaration.contains("fixture_echo(args: { text?: string; }): Promise<CallToolResult>"),
            "{mode}: {declaration}"
        );
        assert!(declaration.contains("Shared MCP Types"), "{declaration}");
        assert_eq!(
            result["called"]["content"][0]["text"], "echoed:hi",
            "{mode}"
        );
    }
}

/// `codemode.mode: only` with an MCP tool: once the server's catalogue is cached (a second launch),
/// the provider is offered `codemode` alone and its description lists the MCP tool; a script calls it.
#[test]
fn only_mode_lists_mcp_tools_in_the_codemode_description() {
    let mcp = mcp_json(json!(true));
    let home = Home::new();
    // Launch 1 connects the server, which writes its catalogue to the cache.
    let cold = Wire::script(
        "return ALL_TOOLS.map(t => t.name).filter(n => n.startsWith('fixture')).sort()",
    )
    .args(&["--tools", "read,codemode,fixture_*"])
    .agent_files(&[("mcp.json", &mcp)])
    .run_in(&home);
    cold.assert_survived();
    assert_eq!(
        cold.script_json(),
        json!(["fixture_echo", "fixture_pic"]),
        "{}",
        cold.context()
    );

    let warm = Wire::script("return await tools.fixture_echo({ text: 'again' })")
        .settings(json!({
            "defaultTools": ["read", "codemode", "fixture_echo"],
            "codemode": { "mode": "only" },
        }))
        .run_in(&home);
    warm.assert_survived();
    assert_eq!(warm.tool_names(0), ["codemode"], "{}", warm.context());
    let description = warm.tool(0, "codemode")["description"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(description.contains("### `fixture_echo`"), "{description}");
    assert!(
        description.contains("fixture_echo(args: { text?: string; }): Promise<CallToolResult>"),
        "{description}"
    );
    assert_eq!(
        warm.script_json()["content"][0]["text"],
        "echoed:again",
        "{}",
        warm.context()
    );
}

// ================================================================================== subagents ==

/// A subagent whose `tools:` names `codemode` runs a script of its own: the child process is offered
/// `read` and `codemode` only, its script reads a file of the shared project, and the parent gets
/// the child's final answer. The persona pins `extensions:` (empty), so the child is started with
/// `--no-extensions`, which used to drop `codemode` even though its tool list named it. The same fake
/// server plays both models, so request order is parent, child, child, parent.
#[test]
fn a_subagent_child_runs_a_codemode_script_from_its_own_tools_list() {
    let persona = "---\nname: scriptor\ndescription: runs scripts\ntools: read, codemode\nextensions:\n---\n\nYou run scripts.\n";
    let o = Wire::turns(vec![
        Turn::Tools(vec![(
            "subagent".to_owned(),
            json!({ "agent": "scriptor", "task": "read a.txt with a script", "async": false }),
        )]),
        Turn::codemode("return await tools.read({ path: 'a.txt' })"),
        Turn::text("CHILD-ANSWER the file says hello"),
        Turn::text("parent done"),
    ])
    .args(&["--tools", "read,subagent"])
    .files(&[
        ("a.txt", "hello from the project\n"),
        (".cyrup/agents/scriptor.md", persona),
    ])
    .env(&[("CYRUP_SUBAGENTS", "1")])
    .run();
    assert_eq!(o.code, 0, "{}", o.context());
    assert_eq!(
        o.requests.len(),
        4,
        "parent, child, child, parent\n{}",
        o.context()
    );
    // Request 1 is the child's: its own tool list, with codemode, not the parent's.
    assert_eq!(o.tool_names(1), ["read", "codemode"], "{}", o.context());
    assert!(
        o.system(1).contains("You run scripts."),
        "the persona's prompt: {}",
        o.system(1)
    );
    // Its script ran against the shared project, through the child's own V8 sandbox process.
    let script_result = o.tool_result(2);
    assert!(
        script_result.starts_with("Script completed"),
        "{script_result}"
    );
    assert!(
        script_result.contains("hello from the project"),
        "{script_result}"
    );
    // The parent was told what the child answered.
    assert!(
        o.tool_result(3).contains("CHILD-ANSWER"),
        "{}",
        o.tool_result(3)
    );
}

/// A child whose `ask` rule fires forwards the question to the session that started it. That session
/// here is `-p`: nobody will ever answer, and the child used to wait out the forwarding bound (10
/// minutes) and then report `User denied bash command ...` for a user who was never asked. It must
/// refuse at once, with the text a session without a UI gives itself, to the script that made the
/// call. The wait is shortened to 8 s so a regression ends the test with a failure instead of a hang.
#[test]
fn a_subagent_childs_ask_is_refused_at_once_when_the_session_that_started_it_has_no_ui() {
    let persona = "---\nname: scriptor\ndescription: runs scripts\ntools: bash, codemode\nextensions:\n---\n\nYou run scripts.\n";
    let policy = json!({
        "defaultPolicy": { "tools": "ask", "bash": "ask", "mcp": "ask", "skills": "ask", "special": "ask" },
        "tools": { "codemode": "allow", "subagent": "allow", "read": "allow" },
    })
    .to_string();
    let o = Wire::turns(vec![
        Turn::Tools(vec![(
            "subagent".to_owned(),
            json!({ "agent": "scriptor", "task": "run a command", "async": false }),
        )]),
        Turn::codemode(
            "try { const r = await tools.bash({ command: 'echo from-child' }); return 'ran:' + r.exit_code } catch (e) { return 'refused:' + e.message }",
        ),
        Turn::text("CHILD-ANSWER done"),
        Turn::text("parent done"),
    ])
    .args(&["--tools", "read,subagent"])
    .agent_files(&[(POLICY_FILE, &policy)])
    .files(&[(".cyrup/agents/scriptor.md", persona)])
    .env(&[
        ("CYRUP_SUBAGENTS", "1"),
        ("CYRUP_PERMISSION_FORWARDING_TIMEOUT_MS", "8000"),
    ])
    .run();
    assert_eq!(o.code, 0, "{}", o.context());
    assert_eq!(
        o.requests.len(),
        4,
        "parent, child, child, parent\n{}",
        o.context()
    );
    let script_result = o.tool_result(2);
    assert!(
        script_result.contains("refused:")
            && script_result.contains("requires approval, but no interactive UI is available")
            && script_result.contains("(from codemode script)"),
        "{script_result}"
    );
    assert!(
        !script_result.contains("User denied"),
        "nobody was asked: {script_result}"
    );
    assert!(
        o.elapsed < Duration::from_secs(7),
        "the child waited for an answer that could not come: {:?}\n{}",
        o.elapsed,
        o.context()
    );
}

/// The `no-ui` markers in a home's permission-forwarding spool (one per headless root session).
fn no_ui_markers(home: &Home) -> Vec<PathBuf> {
    let sessions = home
        .agent_dir
        .join("sessions")
        .join("permission-forwarding")
        .join("sessions");
    std::fs::read_dir(sessions)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path().join("no-ui"))
                .filter(|marker| marker.exists())
                .collect()
        })
        .unwrap_or_default()
}

/// A session id survives resume, so `cyrup -p ...` followed by `cyrup -c` with a UI is one session in
/// two processes. The headless run leaves the root's `no-ui` marker in the spool; the process with the
/// UI must not be bound by it. Its subagent child's `ask` has to reach the person as a dialog, and the
/// person's answer has to decide the call, exactly as in a session that was never headless.
///
/// `stamp_marker` runs between the two launches, on the marker the headless run left: a no-op for
/// the run as it happened (the writer has exited), or a rewrite that makes the writer a process that
/// is still running, so that only the UI's own withdrawal of the marker can lift it.
fn resumed_with_a_ui_after_a_headless_run(stamp_marker: impl FnOnce(&Path)) {
    let persona = "---\nname: scriptor\ndescription: runs commands\ntools: bash\nextensions:\n---\n\nYou run commands.\n";
    let policy = json!({
        "defaultPolicy": { "tools": "ask", "bash": "ask", "mcp": "ask", "skills": "ask", "special": "ask" },
        "tools": { "subagent": "allow", "read": "allow" },
    })
    .to_string();
    let home = Home::new();
    let env = [
        ("CYRUP_SUBAGENTS", "1"),
        ("CYRUP_PERMISSION_FORWARDING_TIMEOUT_MS", "8000"),
    ];

    let first = Wire::turns(vec![Turn::text("headless done")])
        .args(&["--tools", "read,subagent", "-c"])
        .agent_files(&[(POLICY_FILE, &policy)])
        .files(&[(".cyrup/agents/scriptor.md", persona)])
        .env(&env)
        .persist()
        .run_in(&home);
    assert_eq!(first.code, 0, "{}", first.context());
    let markers = no_ui_markers(&home);
    assert_eq!(
        markers.len(),
        1,
        "the headless run says it has no UI: {markers:?}"
    );
    stamp_marker(&markers[0]);

    let (cmd, server) = Wire::turns(vec![
        Turn::Tools(vec![(
            "subagent".to_owned(),
            json!({ "agent": "scriptor", "task": "run a command", "async": false }),
        )]),
        Turn::Tools(vec![(
            "bash".to_owned(),
            json!({ "command": "echo from-child" }),
        )]),
        Turn::text("CHILD-ANSWER done"),
        Turn::text("parent done"),
    ])
    .args(&["--mode", "rpc", "--tools", "read,subagent", "-c"])
    .env(&env)
    .persist()
    .command(&home, false);
    let mut rpc = Lines::spawn(cmd);
    rpc.send(&json!({ "type": "prompt", "message": "go", "id": "p1" }));
    let dialog = rpc.wait_for_frame(
        "the dialog for the child's question",
        Duration::from_secs(30),
        |f| f["type"] == "extension_ui_request" && f["method"] == "select",
    );
    let title = dialog["title"].as_str().unwrap();
    assert!(
        title.contains("Subagent 'scriptor' requested permission")
            && title.contains("echo from-child"),
        "{title}"
    );
    rpc.send(&json!({
        "type": "extension_ui_response", "id": dialog["id"], "value": "Allow Once",
    }));
    rpc.wait_for_frame("the run to end", Duration::from_secs(30), |f| {
        f["type"] == "agent_end"
    });

    let requests = server.requests();
    assert_eq!(
        requests.len(),
        4,
        "parent, child, child, parent\n{}",
        rpc.dump()
    );
    let child_result = requests[2]["messages"]
        .as_array()
        .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "tool"))
        .map(|m| content_text(&m["content"]))
        .unwrap_or_default();
    assert!(
        child_result.contains("from-child"),
        "the child's bash ran once the person allowed it: {child_result}"
    );
    assert!(
        !child_result.contains("no interactive UI is available"),
        "{child_result}"
    );
    assert!(
        no_ui_markers(&home).is_empty(),
        "the process with the UI withdrew the marker"
    );
}

/// The case as it happens: the `-p` run has exited, so its marker names a process that is gone.
#[test]
fn a_session_resumed_with_a_ui_after_a_headless_run_puts_its_subagent_childs_question_to_the_person()
 {
    resumed_with_a_ui_after_a_headless_run(|_| {});
}

/// The same, with the marker's writer a process that is still running (here this test), the case a
/// liveness check cannot help with: a UI that takes the session over withdraws the marker whoever
/// wrote it.
#[test]
fn a_ui_withdraws_a_no_ui_marker_whose_writer_is_still_running() {
    resumed_with_a_ui_after_a_headless_run(|marker| {
        let mut written: Value = serde_json::from_slice(&std::fs::read(marker).unwrap()).unwrap();
        written["pid"] = json!(std::process::id());
        written["processStartIdentity"] = Value::Null;
        std::fs::write(marker, written.to_string()).unwrap();
    });
}
