//! The demo extension's commands for the CAPABILITY-scoped grants — `exec`, `http-client`,
//! `ext-fs` and `proc`. Each one reports the host's real effect when the grant is present and the
//! host's verbatim refusal when it is not, so the same command proves both directions of the seam.

use crate::{CommandDescriptor, ExecOptions, ExtensionApi, HttpRequest, ProcSpawnOptions};

pub(super) fn install(api: &mut ExtensionApi) {
    // A command exercising the capability-scoped exec grant (arch-08 exec; Pi `pi.exec` →
    // `execCommand`, exec.ts:34-46): run `echo hi` as a DIRECT argv (shell:false) and surface the
    // REAL captured stdout + `killed` flag. When the host has NOT granted exec (untrusted ⇒
    // `DenyServices`) the call errors and we notify the denial reason instead — proving the same
    // seam gates both ways.
    api.register_command(
        "execdemo",
        CommandDescriptor::new("Run `echo hi` via the exec capability and report stdout (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| match ctx.ctx().exec(
            "echo",
            &["hi"],
            &ExecOptions::default(),
        ) {
            Ok(r) => {
                ctx.ui().notify(&format!("exec stdout: {}", r.stdout.trim_end()));
                Ok(Some(format!("exec code {} killed {}", r.code, r.killed)))
            }
            Err(e) => {
                ctx.ui().notify(&format!("exec denied: {e}"));
                Ok(Some(format!("exec denied: {e}")))
            }
        },
    );

    // A command exercising the capability-scoped http-client grant (arch-08 §3.2 draft;
    // pi-mcp-adapter-port.md §3.2): GET `args` (the target URL) and surface the REAL captured status
    // + body. When the host has NOT granted http-client (untrusted ⇒ `DenyServices`) the call errors
    // and we notify the denial reason instead — the same seam gates both ways (mirrors `execdemo`).
    api.register_command(
        "httpdemo",
        CommandDescriptor::new(
            "GET a URL via the http-client capability and report status+body (demo).",
        ),
        |args: &str, ctx: &crate::CommandCtx| match ctx
            .ctx()
            .http_request(&HttpRequest::get(args.trim()))
        {
            Ok(r) => {
                let body = String::from_utf8_lossy(&r.body).into_owned();
                ctx.ui().notify(&format!("http status: {} body: {}", r.status, body));
                Ok(Some(format!("http status {} body {}", r.status, body)))
            }
            Err(e) => {
                ctx.ui().notify(&format!("http denied: {e}"));
                Ok(Some(format!("http denied: {e}")))
            }
        },
    );

    // Two commands exercising the capability-scoped `ext-fs` grant (EXT-054/EXT-055): `/fswrite
    // <name> <text>` and `/fsread <name>`, both addressing paths relative to the project root, both
    // reporting the host's verbatim refusal when the manifest's `capabilities.fs` does not cover the
    // path. They are the fs analog of `execdemo`/`httpdemo`: the same "granted ⇒ real effect,
    // ungranted ⇒ typed denial" seam, in the one capability that had NO guest-reachable surface at
    // all until the SDK gained `Ctx::read_file`/`Ctx::write_file`.
    api.register_command(
        "fswrite",
        CommandDescriptor::new("Write `<name> <text>` through the ext-fs capability (demo)."),
        |args: &str, ctx: &crate::CommandCtx| {
            let (name, text) = args.trim().split_once(' ').unwrap_or((args.trim(), ""));
            match ctx.ctx().write_file(name, text.as_bytes()) {
                Ok(()) => {
                    ctx.ui().notify(&format!("fs wrote: {name}"));
                    Ok(Some(format!("fs wrote {name}")))
                }
                Err(e) => {
                    ctx.ui().notify(&format!("fs write denied: {e}"));
                    Ok(Some(format!("fs write denied: {e}")))
                }
            }
        },
    );
    api.register_command(
        "fsread",
        CommandDescriptor::new("Read `<name>` through the ext-fs capability (demo)."),
        |args: &str, ctx: &crate::CommandCtx| match ctx.ctx().read_file(args.trim()) {
            Ok(bytes) => {
                let body = String::from_utf8_lossy(&bytes).into_owned();
                ctx.ui().notify(&format!("fs read: {body}"));
                Ok(Some(format!("fs read {body}")))
            }
            Err(e) => {
                ctx.ui().notify(&format!("fs read denied: {e}"));
                Ok(Some(format!("fs read denied: {e}")))
            }
        },
    );

    // A command exercising the streaming half of the http-client grant (`request-stream` /
    // `poll-stream-chunk`): open a stream to `args`, immediately surface the initiating response's
    // status+headers (closes L4 §2.3 — available BEFORE and INDEPENDENT of draining any chunk, off
    // the SAME round trip `request-stream` used to open the body, exactly what the real consumer this
    // backs — the MCP SDK's `StreamableHTTPClientTransport` — reads off its one `fetch()` response),
    // then poll every chunk to EOF and surface the real chunk count + concatenated body — proving the
    // host-owned stream registry (arch-08 §5.2's request/poll bridge) delivers real bytes across the
    // wasm boundary, in order.
    api.register_command(
        "httpstreamdemo",
        CommandDescriptor::new(
            "Stream a URL via the http-client capability and report status+headers+chunks+body (demo).",
        ),
        |args: &str, ctx: &crate::CommandCtx| {
            let opened = match ctx.ctx().http_request_stream(&HttpRequest::get(args.trim())) {
                Ok(o) => o,
                Err(e) => {
                    ctx.ui().notify(&format!("http stream denied: {e}"));
                    return Ok(Some(format!("http stream denied: {e}")));
                }
            };
            let content_type = opened
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            // Notified BEFORE any chunk is polled: proves status/headers are independent of the body.
            ctx.ui().notify(&format!(
                "http stream opened status: {} content-type: {content_type}",
                opened.status
            ));
            let handle = opened.handle;
            let mut body = Vec::new();
            let mut chunks = 0u32;
            loop {
                match ctx.ctx().http_poll_stream_chunk(handle) {
                    Ok(Some(chunk)) => {
                        chunks += 1;
                        body.extend_from_slice(&chunk);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        ctx.ui().notify(&format!("http stream poll error: {e}"));
                        break;
                    }
                }
            }
            ctx.ctx().http_close_stream(handle);
            let body = String::from_utf8_lossy(&body).into_owned();
            ctx.ui().notify(&format!("http stream chunks: {chunks} body: {body}"));
            Ok(Some(format!(
                "http stream status {} content-type {content_type} chunks {chunks} body {body}",
                opened.status
            )))
        },
    );

    // Commands exercising the capability-scoped `proc` grant (arch-08 §5.2 request/poll bridge;
    // pi-mcp-adapter-port.md §3.1): a long-lived, duplex-pipe child, distinct from the bounded
    // `execdemo` one-shot. Split into separate commands (rather than one big demo like
    // `execdemo`/`httpdemo`) so a HOST-side test can drive each step as its own top-level
    // `session.prompt(...)` round trip — proving stdin/stdout stay live across genuinely separate
    // calls, not just an internal loop within one guest invocation — and interleave real OS-level
    // process checks between `procspawn` and `prockill`.
    //
    // `procspawn` runs a marker-tagged shell read-echo loop (`sh -c 'while IFS= read -r line; do
    // printf "echo:%s\n" "$line"; done' <marker>`) — a genuine long-lived duplex process (not a
    // one-shot), with the marker as a trailing shell positional arg so a host-side `pgrep -f
    // <marker>` can find (and later confirm the disappearance of) the exact real OS process.
    api.register_command(
        "procspawn",
        CommandDescriptor::new(
            "Spawn a marker-tagged shell read-echo loop via the proc capability (demo). \
             args: <marker>",
        ),
        |args: &str, ctx: &crate::CommandCtx| {
            let marker = args.trim();
            match ctx.ctx().proc_spawn(
                "sh",
                &["-c", "while IFS= read -r line; do printf 'echo:%s\\n' \"$line\"; done", marker],
                &ProcSpawnOptions::default(),
            ) {
                Ok(handle) => {
                    ctx.ui().notify(&format!("proc spawned handle:{handle}"));
                    Ok(Some(format!("proc spawned handle:{handle}")))
                }
                Err(e) => {
                    ctx.ui().notify(&format!("proc denied: {e}"));
                    Ok(Some(format!("proc denied: {e}")))
                }
            }
        },
    );

    // `procspawnexit` (no args): spawns a child that exits ON ITS OWN shortly after starting
    // (`sh -c "sleep 0.1; exit 7"`) — no `kill` involved — so a host-side test can prove `poll-exit`
    // reports the REAL natural exit code, not just a `kill`-driven one.
    api.register_command(
        "procspawnexit",
        CommandDescriptor::new("Spawn a proc that exits on its own with code 7 (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            match ctx.ctx().proc_spawn(
                "sh",
                &["-c", "sleep 0.1; exit 7"],
                &ProcSpawnOptions::default(),
            ) {
                Ok(handle) => {
                    ctx.ui().notify(&format!("proc spawned handle:{handle}"));
                    Ok(Some(format!("proc spawned handle:{handle}")))
                }
                Err(e) => {
                    ctx.ui().notify(&format!("proc denied: {e}"));
                    Ok(Some(format!("proc denied: {e}")))
                }
            }
        },
    );

    // `procwrite <handle> <text>`: write `<text>\n` to the child's REAL stdin.
    api.register_command(
        "procwrite",
        CommandDescriptor::new("Write a line to a spawned proc's stdin (demo). args: <handle> <text>"),
        |args: &str, ctx: &crate::CommandCtx| {
            let mut it = args.trim().splitn(2, ' ');
            let handle: u32 = it.next().unwrap_or_default().parse().unwrap_or(0);
            let text = it.next().unwrap_or_default();
            let mut line = text.to_string();
            line.push('\n');
            match ctx.ctx().proc_write_stdin(handle, line.as_bytes()) {
                Ok(n) => {
                    ctx.ui().notify(&format!("proc wrote handle:{handle} bytes:{n}"));
                    Ok(Some(format!("proc wrote {n} bytes")))
                }
                Err(e) => {
                    ctx.ui().notify(&format!("proc write denied: {e}"));
                    Ok(Some(format!("proc write denied: {e}")))
                }
            }
        },
    );

    // `procreadpoll <handle> <needle>`: poll REAL stdout — across MULTIPLE `read-stdout` calls in a
    // bounded loop (empty = no data yet, never treated as EOF) — until the accumulated bytes
    // contain `<needle>`, proving the pipe is genuinely live, not a captured one-shot.
    api.register_command(
        "procreadpoll",
        CommandDescriptor::new(
            "Poll a spawned proc's stdout until a needle appears (demo). args: <handle> <needle>",
        ),
        |args: &str, ctx: &crate::CommandCtx| {
            let mut it = args.trim().splitn(2, ' ');
            let handle: u32 = it.next().unwrap_or_default().parse().unwrap_or(0);
            let needle = it.next().unwrap_or_default().as_bytes();
            let mut acc: Vec<u8> = Vec::new();
            let mut seen = false;
            for _ in 0..20_000u32 {
                match ctx.ctx().proc_read_stdout(handle, 4096) {
                    Ok(chunk) => acc.extend_from_slice(&chunk),
                    Err(e) => {
                        ctx.ui().notify(&format!("proc read denied: {e}"));
                        return Ok(Some(format!("proc read denied: {e}")));
                    }
                }
                if !needle.is_empty() && acc.windows(needle.len()).any(|w| w == needle) {
                    seen = true;
                    break;
                }
            }
            let acc_text = String::from_utf8_lossy(&acc).into_owned();
            ctx.ui().notify(&format!("proc read handle:{handle} seen:{seen} acc:{acc_text}"));
            Ok(Some(format!("proc read seen:{seen}")))
        },
    );

    // `procpollexit <handle>`: a single non-blocking `poll-exit` (none = still running).
    api.register_command(
        "procpollexit",
        CommandDescriptor::new("Poll a spawned proc's exit status once (demo). args: <handle>"),
        |args: &str, ctx: &crate::CommandCtx| {
            let handle: u32 = args.trim().parse().unwrap_or(0);
            let code = ctx.ctx().proc_poll_exit(handle);
            ctx.ui().notify(&format!("proc pollexit handle:{handle} code:{code:?}"));
            Ok(Some(format!("proc pollexit code:{code:?}")))
        },
    );

    // `prockill <handle>`: terminate the child (SIGTERM then SIGKILL after a grace period,
    // host-side policy) and report both the kill outcome and the exit status observed right after.
    api.register_command(
        "prockill",
        CommandDescriptor::new("Kill a spawned proc (demo). args: <handle>"),
        |args: &str, ctx: &crate::CommandCtx| {
            let handle: u32 = args.trim().parse().unwrap_or(0);
            let kill_result = ctx.ctx().proc_kill(handle);
            let code = ctx.ctx().proc_poll_exit(handle);
            ctx.ui().notify(&format!(
                "proc kill handle:{handle} ok:{} code:{code:?}",
                kill_result.is_ok()
            ));
            match kill_result {
                Ok(()) => Ok(Some(format!("proc killed code:{code:?}"))),
                Err(e) => Ok(Some(format!("proc kill denied: {e}"))),
            }
        },
    );
}
