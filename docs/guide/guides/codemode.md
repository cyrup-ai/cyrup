# Codemode

The `codemode` tool lets the model write a JavaScript script that calls cyrup's other tools and runs non-LLM models, such as classifiers and image models. Only the script's output reaches the model, so a script can run calls in parallel and filter large results before the model sees them. To turn it on, see [Enable codemode](#enable-codemode).

## Enable codemode

`codemode` is a built-in tool that is registered but not active by default, unless a permission policy turns it on (see the list below). Activate it like any other tool: add it to `defaultTools` in [settings](../reference/settings.md#tools-and-codemode), or name it with `--tools`, which is an allowlist (see [Tools](../reference/cli.md#tools)).

```json
{ "defaultTools": ["+codemode"] }
```

That line in `~/.cyrup/agent/settings.json` keeps the default `read`, `bash`, `edit`, and `write` and adds `codemode`. For one run, list every tool, since `--tools` replaces the selection:

```sh
cyrup --tools read,bash,edit,write,codemode
```

What turns it on without those, and what switches it off or keeps it off:

- **An armed permission system turns it on.** Once a permission policy exists (a `cyrup-permissions.jsonc` is enough, the project's `.cyrup/agent/cyrup-permissions.jsonc` too, whether or not the project is trusted; see [The permission system](../guides/tools-and-permissions.md#the-permission-system)), the permission system sets the active tools at the start of each prompt to every registered tool its policy does not `deny`. `codemode`, `tool_search`, `powershell`, `grep`, `find` and `ls` are then active with no `defaultTools` line, and neither `defaultTools` nor `--no-builtin-tools` narrows that set. `--tools`, `--no-tools` and `--exclude-tools` still bound it. To keep `codemode` off under a policy, deny it (`"tools": { "codemode": "deny" }`) or pass `--exclude-tools codemode`.
- **Project settings need a trusted project.** The same `defaultTools` line in a project's `.cyrup/settings.json` counts only when the project is trusted. An untrusted project's settings are not read at all, so `codemode` is simply missing; the terminal interface prints an untrusted-project warning, and a `-p` run says nothing. `-p`, `--mode json` and `--mode rpc` cannot ask, so they treat a project with no saved decision as untrusted: pass `--approve` or put the line in the global file. See [Project trust](../guides/tools-and-permissions.md#project-trust).
- **`--no-extensions` removes it.** A built-in extension provides the tool, and `--no-extensions` skips the built-in extensions, so the tool does not exist in that session. `defaultTools` then warns (`defaultTools: no activatable tool is registered as "codemode"`); a `--tools` list that names it adds an `[Extension issues]` entry to the terminal interface's startup panel, and a `-p` run prints nothing.
- **`--no-tools` and `--no-builtin-tools` do not activate it** by themselves, and `--exclude-tools codemode` removes it.
- **A subagent child** gets it only when its `tools:` list names it; see [Subagents](#subagents).

`codemode.mode` and `codemode.inlineBudget` in [settings](../reference/settings.md#tools-and-codemode) change how it presents tools; see [Call tools](#call-tools). Both are read when a session is built, so a change applies after `/reload` or a restart.

Codemode is useful without MCP: a script can run several tool calls in parallel, filter large output before it reaches the model, classify JSON with a classifier model, and generate images (see [Models](#models)).

## Scripts

The script is plain JavaScript source, not a markdown code fence (one fence around the whole script is tolerated and stripped; a fence that never closes is an error). On a model that supports OpenAI grammar tools (`compat.supportsOpenAIGrammarTools`, which the catalog sets for capable OpenAI Responses-family models and which a custom model entry can set for a Responses or Chat Completions route) the tool is sent as a grammar-constrained custom tool and the model writes the script as raw text; everywhere else it is a normal function tool and the script is the string value of its `code` argument. Either way the call is stored in the session as `{"code": "<script>"}`. It runs as the body of an async function in a V8 sandbox, so top-level `await` and `return` work. Declare variables freely, with two exceptions to keep in mind: `tools` and `console` may be redeclared (`const tools = await searchTools("read")` is fine), but `text`, `image`, `store`, `load`, `exit`, and the other globals below are plain variables of the sandbox, so `const text = await tools.read(...)` makes the next `text(...)` call your string. The sandbox has no Node APIs, file system, network, or timers; scripts reach the outside world only through tools and `models`. The global object holds ECMAScript's own built-ins and the globals below, nothing else: `WebAssembly`, `SharedArrayBuffer`, `Atomics.wait`, `Intl`, and `queueMicrotask` are removed, and dynamic `import()` rejects.

A script may start with an options line. It must be the first non-blank line of the script (blank lines and a surrounding code fence before it are fine); an `// @options:` line anywhere later fails the call instead of being ignored as a comment.

```js
// @options: {"max_output_tokens": 2000, "timeout_ms": 60000}
```

- `max_output_tokens` (default 10000) limits the output. Longer output keeps its start and end, and the full text is written to a temp file whose path is included in the result. A script fails when its output passes 16777216 characters of text and base64 image data or 100000 `text()`, `image()`, and `console` calls; write large data to a file with a tool instead. The JSON text of the value the script returns counts toward the same 16777216 characters.
- `timeout_ms` is a hard deadline for the whole script, tool calls included. Without it a script stops after 120 seconds of its own running time or 30 minutes in all, whichever comes first. The running time is the time the script spends executing JavaScript; time spent waiting for a tool call does not count, so `await tools.bash(...)` or an image generation that takes minutes does not use it up, while `while (true) {}` does. Setting `timeout_ms` replaces both limits with that one deadline, so use it to allow a script longer than 30 minutes (up to 2147483647 ms) and keep it generous for scripts that generate images. A script that reaches a default limit fails with `Script timed out` and a line saying how to raise it.

The result starts with `Script completed` or `Script failed`, the wall time, and the output (`(no output)` when the script printed and returned nothing). Text and image items appear in order, each on its own line. When the output has more than one text item (from `text()` or `return`), each starts with a `==> text N/M <==` line. `console` calls follow in one `<console_output>` block with one line per call. A failed script keeps its partial output, followed by `Script error:` and the error. The error is the script's stack: its first line is `Name: message`, and the frames read `at codemode.js:LINE:COLUMN`, where `LINE` and `COLUMN` count the script's own lines and characters, so `codemode.js:1:7` is the seventh character of the first line you wrote. A tool call that rejects carries the frame of the call that made it, so under `Promise.all()` the stack says which call failed. Tool calls are real: calls made before a failure are not undone. Calls still running when the script ends are cancelled, and unawaited promises are discarded. A script that otherwise succeeded gets a last line saying so, such as `Note: 2 tool calls were still running when the script ended and were cancelled: write x2. Await every call before the script ends (await Promise.all([...]), await main()).`, because the commonest cause is a forgotten `await` (`main();` without it, `forEach(async ...)`); upstream marks them cancelled in the call list only. A call that was not awaited and has already failed, an `async` function that threw and was not awaited, and a `Promise.reject(...)` that nothing handled do not fail the script either (nothing was waiting for them), and are named the same way: `Note: 1 error was never handled and did not fail the script: write: ENOENT: no such file (codemode.js:1:7). A tool call or async function that is not awaited loses its error. ...`, the first five of them, each cut to a line, with the number of the rest. An error that reaches the script and is handled, however late (a `try`/`catch`, `.catch()`, `Promise.allSettled()`, a promise kept and awaited after the call failed), is not reported. A call that nothing waits on is reported as above whenever it fails before the script's result is made. A call that fails after the script has ended, when something was waiting on it, is named in a note of its own that gives no advice, because the script may have dealt with it or may have lost it: `Note: 2 calls failed after the script ended, so the script did not see the errors: read: ENOENT: no such file; read: ENOENT: no such file.` That covers the other calls of a `Promise.all` that had already rejected, a call with a `.catch()`, and the forgotten `await` whose call has a handler: an `async` function nobody awaited (`main();`), a `forEach(async ...)` callback, a `.then(f)` with no `.catch()`. The first five are listed, with the number of the rest. A call still running when the script ends gets the cancelled note above instead. Upstream ends such a script as a plain success.

## Globals

| Global | Purpose |
|---|---|
| `tools.<name>(args)` | Call a tool. See [Call tools](#call-tools). |
| `text(value)` | Add a text item to the output. Strings are added as is, other values as JSON. An `Error` is added as its stack text, a `Map` as its `[key, value]` entries, a `Set` as its values, and a `BigInt` as a decimal string (also inside objects and arrays). |
| `image(value)` | Add an image to the output: a base64 `data:` URL, an `{ image_url }` object, or an image block `{ type: "image", data, mimeType }` such as those returned by MCP tools and `models.generateImages()`. Remote URLs are not supported. PNG, JPEG, GIF, and WebP are accepted, up to 5 MiB decoded each; a larger image throws a `RangeError` (providers reject such images, and the saved block would be sent again with every later turn), so shrink or re-encode it with a tool first. Each image is also saved to a temp file, and the result names the path before the image. |
| `console.log(...)` | Add a line to the `<console_output>` block after the other output, with the arguments joined by a space; `info`, `warn`, `error`, and `debug` do the same. `console.dir`, `group`/`groupCollapsed`/`groupEnd` (indent what follows), `assert`, `count`/`countReset`, `time`/`timeLog`/`timeEnd`, and `table` (a Markdown table) also exist and print to the same block. |
| `return value` | A top-level `return` adds the value like `text()`. |
| `exit()` | End the script successfully. |
| `store(key, value)` / `load(key)` | Keep small JSON values across `codemode` calls. See [Store values](#store-values). |
| `ALL_TOOLS` | Every callable tool as `{ name, description }`, including tools the description does not list. |
| `searchTools(query, { limit?, namespace? })` | Rank callable tools by relevance (BM25, default limit 8). Resolves to `{ name, description }[]`. |
| `describeTool(name)` | Resolves to a tool's description and TypeScript declaration, or `undefined`. |
| `describeNamespace(name)` | Resolves to `{ name, description?, instructions?, tools }` for a namespace, the group an extension registers related tools under, or `undefined`. |
| `models` | List and run non-LLM models. See [Models](#models). |

## Call tools

Every tool the script can call is a method of `tools`, named by its identifier: characters that are not valid in a JavaScript identifier become `_`, so the MCP tool `dev-radius_search` is `tools.dev_radius_search`, and with `toolPrefix` `mcp` (see [MCP tools](#mcp-tools)) `mcp__dev-radius_search` is `tools.mcp__dev_radius_search`. Each method takes one object with the tool's arguments. Reading a member that does not exist throws an error that lists the tools that do exist and names the close matches (`tools.Bash` suggests `tools.bash`); to probe for a tool, use `"name" in tools`.

**Which tools a script can call.** How a tool is exposed decides it:

- A `direct` tool, which is every built-in and most extension tools, is callable only while it is active. In a default session only `read`, `bash`, `edit`, and `write` are active, so `tools.grep`, `tools.find`, and `tools.ls` do not exist until you activate them (`--tools`, `defaultTools`). A session started with `--tools read,codemode` has no `tools.bash` either. The permission system, when it is armed, re-derives the active set from its policy at the start of each prompt, so there every registered tool the policy does not deny is active, `grep`, `find`, `ls`, `powershell` and `codemode` itself included (see [Enable codemode](#enable-codemode)).
- A `codemode` or `deferred` tool, such as the tools of an MCP server in search mode, is callable whenever it is registered, active or not.
- A `model-only` tool such as `tool_search` and a `hidden` tool are never callable from a script, and `codemode` itself is not: a script cannot start a script.
- A tool that `--tools`, `--exclude-tools`, or a permission `deny` rule leaves out does not exist for scripts either.

`ALL_TOOLS`, `searchTools()`, and the `tools` object hold exactly the callable tools.

Two tools can have the same identifier, for example `gh-search` and `gh_search`. A name that already is an identifier keeps it, and the other gets a numeric suffix: `tools.gh_search` is `gh_search` and `tools.gh_search_2` is `gh-search`. Both are listed under those identifiers in the description, `ALL_TOOLS`, and `searchTools()`, and `describeTool()` finds either by identifier or by its own name; `tools["gh-search"]` works too. The suffix depends only on the set of names, not on the order the tools were registered in.

What a call resolves to depends on the tool:

- Tools that declare an output schema resolve to their structured value, also when the result is an error that carries one. `bash` resolves to `{ output, truncated, full_output_path?, exit_code, wall_time_seconds }`, also for non-zero exit codes. Its `output` is not limited to the 2000 lines or 50KB the model sees: it holds up to 1 MiB, and longer output keeps its first and last 512 KiB around a `[... N bytes omitted ...]` marker, with `truncated` set and the full output in `full_output_path`.
- MCP tools resolve to the server's `CallToolResult` without `_meta`: its `content` blocks as the server sent them (image blocks included, which `image()` shows), `structuredContent` and `isError`. A result with `isError` resolves like any other, so check it; a call that never got an answer from the server rejects. The result is complete: the shortening that the model's own view of an MCP result gets does not apply to what a script reads. `describeTool()` includes the shared types that declaration uses. This holds for every tool a server registers, in eager and in search mode alike; what differs is whether the tool is callable at all (see [MCP tools](#mcp-tools)).
- The `mcp` gateway tool (active by default) resolves to the text the model would read, not to a `CallToolResult`: `await tools.mcp({ tool: "docs_search", args: { query: "retry" } })` is the string the server answered with.
- `read` resolves to the file's text, or for an image to an image block `{ type: "image", data, mimeType, note }` that `image()` shows. `data` is the base64 image the model would see and `note` the text that goes with it, such as resize hints.
- Other tools, such as `edit`, `write`, `grep`, `find`, and `ls`, resolve to their text output.

A script that names an MCP server's tools, or calls `searchTools()`, `describeTool()`, `describeNamespace()` or reads `ALL_TOOLS`, waits before it starts for the MCP servers that are still connecting, so a server that has not finished starting, such as a first `npx` download, has registered its tools when the script looks. `tool_search` waits the same way. The wait is bounded: two minutes, or the script's own `timeout_ms` when that is shorter, because it comes before the script starts and a script that asked for three seconds should not wait two minutes for a server. When a wait runs out, the session says `MCP servers are still connecting; their tools become available once connected.` (where there is a UI to say it in), and the script runs against the servers that have registered. A wait that ran the whole two minutes is not repeated: later scripts and searches in that session do not wait for those servers again. A wait cut short by a script's `timeout_ms` is not that, and a later script that allows more waits again. How long a server may take to answer a request, `initialize` included, is its `requestTimeoutMs` in `mcp.json`, or the global `settings.requestTimeoutMs`; with none set (or a value of `0` or less) it is 60 seconds, the MCP SDK's default. A server that does not answer `initialize` or the tool lists in that time fails to connect with `Request timed out`, and a tool call that is not answered in that time rejects with `MCP request timed out after 60000 ms`; a call's 60 seconds start again whenever the tool reports progress. A reply the client cannot parse is dropped, as the other MCP SDKs do, so it ends the request the same way. cyrup's JSON reader also refuses a message nested more than 128 levels deep, which a JavaScript client would read, so a tool list or result nested that deep times out too.

A call made while the servers are still connecting, whether to the `mcp` gateway (`await tools.mcp({ search: "docs" })`) or to a server's own tool, waits for them to finish instead of answering `MCP not initialized`. That wait is bounded too: after 30 seconds the result says `MCP initialization is still in progress. Try again shortly.`; the servers keep connecting, and a retry finds them.

### MCP tools

A server's `directTools` setting in `mcp.json` decides how its tools are registered, and so whether a script reaches them. Tools are named by the `toolPrefix` setting: `<server>_<tool>` by default, `mcp__<server>_<tool>` with `"mcp"`, the bare tool name with `"none"`. The user guide has no MCP chapter yet; this table is the part that matters to scripts.

| `directTools` | The server's tools are | Declared to the model | A script can call them |
|---|---|---|---|
| `true`, or a list of tool names | `direct` tools, one each | While active | Only while active. `--tools read,codemode` leaves them inactive, so name them (`--tools 'read,codemode,docs_*'`) or keep the default selection |
| `"search"` | `deferred` tools | Not until `tool_search` loads a match | Whenever registered, found with `searchTools()`, described with `describeTool()` |
| unset or `false` | reached through the `mcp` gateway | Only the gateway | `tools.mcp({ tool, args })`, while the gateway is active |

**When the eager tools are declared.** When some server declares tools to the model (`directTools: true` or a list of tool names), the first prompt of a session waits for servers that are still connecting, for at most 10 seconds (pi's built-in MCP extension waits as long), so that their `direct` tools are declared in its first request. With `directTools` unset or `"search"` nothing is declared at the first prompt and it is not held. Only the first prompt waits, and when the wait runs out the session says `MCP servers are still connecting; their tools become available once connected.` pi waits for the servers that declare tools; here the wait is for the whole connect pass (one future for every server), so a slow server that declares none holds the first prompt as long as another that does. It does so with or without a metadata cache (`mcp-cache.json` in the agent directory, written the first time a server connects). Measured with a stdio server and `directTools: true`, `keep-alive`, no cache: a server that took 0, 2 or 7 seconds to answer `initialize` had its tools in the first request, which was sent after 0.4, 2.4 and 7.4 seconds; one that took 13 seconds had not, its first request was sent after 10.6 seconds without the tools, with no error, and the tools were declared from a later request on. A script that names the server waits for it (see above), so this affects what the model is offered, not what a script can call.

Neither `codemode` nor `tool_search` is turned on when a server connects, whatever its mode: name them in `defaultTools` or `--tools` (an armed permission system activates them too, see [Enable codemode](#enable-codemode)). With `directTools: "search"` and neither of them active, the model sees the `mcp` gateway and none of the server's tools. `codemode.mode: "only"` hides declared tools from the model but does not change which tools a script can call.

A call that fails, is blocked, or gets invalid arguments rejects with an `Error` that carries the tool's error text. A `bash` command that exits with a non-zero code is not such a failure: it resolves to the structured result, so check `exit_code`. Use `Promise.allSettled()` to keep the results of the calls that succeed.

The `codemode` description lists tools with their TypeScript declarations, grouped by namespace. Tools with `deferred` exposure are not listed, so the description stays the same while extensions register and change such tools. Listed declarations share a budget of 3000 estimated tokens (`codemode.inlineBudget` in [settings](../reference/settings.md#tools-and-codemode)). The built-in tools (`read`, `bash`, `edit`, `write`, `powershell`, `grep`, `find`, `ls`) are placed first, so a tool-heavy session does not push them out; whatever the budget leaves out is counted (`N tools not listed; use ALL_TOOLS / searchTools`, or `(some tools not listed)` after a namespace heading). Scripts find the other tools with `searchTools()`, `describeTool()`, `describeNamespace()`, or by filtering `ALL_TOOLS`.

While `codemode` is active, `codemode.mode` in [settings](../reference/settings.md#tools-and-codemode) decides how the other tools are presented. With `on` (default) declared tools stay declared, and their descriptions say how to call them from scripts. With `only` they are hidden from the model and listed in the `codemode` description instead, so the model calls them through scripts. Tool declarations in the `codemode` description, `describeTool()`, and `ALL_TOOLS` carry the tools' prompt guidelines, since the system prompt rules only cover declared tools.

## Permissions

A script's calls are real tool calls and go through the same permission gate as any call the model makes, one gate per call: the permission policy, then any extension that handles `tool_call`. Extensions see a nested call as they see any other, with the id `<codemode call id>/<n>` and the `codemode` call's id as `parentToolCallId`. A tool the policy denies outright (`"tools": { "bash": "deny" }`) is not in `tools` at all, so `tools.bash` throws "does not exist"; a command rule that denies one command (`"bash": { "rm *": "deny" }`) rejects that call with the policy's text.

The `codemode` call is a tool call too, so its own rule applies first. With `ask` on `codemode` you are asked once to run the script, and then once for each nested call whose own rule is `ask`; allowing one does not allow the other. The prompt of a nested call ends with `(from codemode script)` so you can tell it from a call the model made directly. Its dialog has a fifth option after the usual four, `Reject All From This Script`: it refuses the call on screen and every other call that script makes, the ones already waiting behind the dialog included, at once and without another dialog; each of them fails with `the user rejected this script's tool calls` for the script to catch or let end it. The script is not aborted, and what it did before is not undone. Another script's calls, and the model's own, are asked as before. A script that starts its calls together (`Promise.all` over a list) otherwise puts one dialog up for each of them, and `Esc` and `Ctrl+C` answer only the one on screen. The audit log entry of such a decision carries the `parentToolCallId` of the `codemode` call.

A run without a UI (`-p`, `--mode json`) has nobody to ask, so an `ask` becomes a block. In a script that rejects the call with `Running bash command 'ls' requires approval, but no interactive UI is available (from codemode script). A script cannot answer an approval prompt without a UI: allow this call in the permission policy, or run interactively to be asked.` Catch it like any other failed call, or give the tools a script needs an `allow` rule. A subagent child's `ask` is put to the session that started it, and when that session has no UI either (`-p`, `--mode json`) the child refuses at once with the same text instead of waiting for an answer that cannot come (see [the dialog](../extensions/permissions.md#the-dialog)).

`--mode rpc` does have somebody to ask, the client. The dialog is an `extension_ui_request` with `method: "select"`, a title that ends `(from codemode script)` and the options `Allow Once`, `Allow Always`, `Reject`, `Reject with Reason` and `Reject All From This Script` (the first four only, for a call the model made directly). The `extension_ui_response` the client sends decides the call: `Allow Once` lets it run, `Reject` makes the script's call fail. A call that is waiting for the answer is still bound by the script's limits: when `timeout_ms` runs out, the run is aborted or the session is replaced, the call is cancelled and shows as `(cancelled)`, the script ends as usual, and an `Allow Once` that arrives afterwards runs nothing. The dialog stays on the client until someone answers it. An ACP editor is asked the same way, with a `session/request_permission`.

## Subagents

A subagent child has a tool list of its own, and `codemode` is not among a child's default tools. A child with a `tools:` list can run scripts only when the list names `codemode` (`tools: read, grep, codemode`), and `excludeTools` can still remove it. This holds for an agent that pins `extensions:` too, although such a child starts with `--no-extensions`. A capability ceiling that denies extensions does not: the child then fails at start with a missing-tool message naming `codemode`. A child without a `tools:` list is started with no `--tools` flag, so an armed permission system activates `codemode` in it as in any session (see [Enable codemode](#enable-codemode)); a `tools:` list pins the set. Details are in [Subagents](../extensions/subagents.md#tools-a-subagent-gets).

Inside a child, a script's calls go through the child's permission gate. They are not counted as tool calls of the child in its progress, tool count, current tool, or recent tools (the `codemode` call is), but they do count for the check that the child changed files.

## Store values

`store(key, value)` keeps a JSON value under a string key for later `codemode` calls; storing `undefined` deletes the key. `load(key)` returns the value, or `undefined`. Writes are kept only when the script succeeds: each successful script that stores values appends a `codemode-store` custom entry to the session, so resumed sessions keep the values and each branch sees only the values written on its path.

The store is for small state such as IDs, cursors, or summaries. One value may have at most 262144 characters of JSON and all keys and values together at most 1048576. Do not store image data; show images with `image()`, which also saves them to a temp file.

## Models

`models` reaches the model catalog and runs non-LLM models with the session's credentials: classifiers, which answer typed questions about JSON state, and image models, which generate images. Chat models are listed but cannot be run from scripts. Classifier models come from [llama.cpp](../llama-cpp.md#classifier-models), where every model the router lists is also a classifier model, and from hosted services: `openai` lists `gpt-6-luna` (OpenAI's Decisions API) for an `OPENAI_API_KEY` credential but not for Sign in with ChatGPT, whose tokens that API rejects, and `typesafe` lists TypeSafe's `jev-latest` for a `TYPESAFE_API_KEY`; image models come from the `openrouter` provider and need OpenRouter credentials. Catalog entries carry no `headers`.

The `codemode` description points the model at this page. A cyrup binary carries a copy of it and writes that copy to `docs/codemode.md` under the agent directory when a session starts (replacing a copy an older build left there), so the path the model reads exists wherever cyrup is installed. That copy is outside the project, so under a permission policy a `read` of exactly that file skips the `external_directory` rule (the `read` rule still applies); every other path outside the project is gated as usual. The same goes for the spill files below: a `read` of exactly the `pi-codemode-*` file that a result names skips `external_directory`, in a script and for the model alike.

```ts
type ModelType = "chat" | "image" | "classifier";

/** A catalog entry. `provider` and `id` identify it; other fields depend on the type. */
interface ModelInfo {
  type?: ModelType;
  provider: string;
  id: string;
  name: string;
  api: string;
  input: ("text" | "image")[];
  contextWindow?: number;
  [key: string]: unknown;
}

declare const models: {
  /** Every known model of a type, optionally for one provider. */
  getModelsOfType(type: ModelType, provider?: string): Promise<ModelInfo[]>;
  /** Models of a type whose provider has working credentials. */
  getAvailableOfType(type: ModelType, provider?: string): Promise<ModelInfo[]>;
  /** One catalog entry, or undefined. */
  getModelOfType(type: ModelType, provider: string, id: string): Promise<ModelInfo | undefined>;
  /** Answer `context.questions` about `context.state`; answers are in `result.answers` by question ID. */
  classify(model: ModelInfo, context: ClassifierContext): Promise<ClassifierResult>;
  /** Generate images from `context.input` text and image blocks; show `result.output` blocks with image(). Can take minutes. */
  generateImages(model: ModelInfo, context: ImagesContext): Promise<ImagesResult>;
};
```

`classify()` and `generateImages()` use only the `provider` and `id` of `model`, so `{ provider, id }` works as well. They do not throw on provider errors: check `stopReason` and `errorMessage`. At most four such calls run at once per script; more calls wait for a free slot, so `Promise.all()` over many items is fine. Their usage is added to the `codemode` tool result and counts toward the session cost.

Model IDs differ between providers, and one model can be listed by several. Use `models.getAvailableOfType(type)` to find the IDs that work with the current credentials. Passing a model that does not exist, or one of the wrong type, rejects with an error that says so and points to `models.getAvailableOfType()`.

### Classify

```ts
interface ClassifierContext {
  /** The data to classify. */
  state: Record<string, unknown>;
  /** Questions by ID. One call answers all of them. */
  questions: Record<string, ClassifierQuestion>;
}

type ClassifierQuestion =
  /** Pick one label. `criteria` maps each label to what it means. */
  | { type: "choice"; instructions: string; criteria: Record<string, string> }
  /** Score on an ordered scale. `criteria` describes each level, lowest first. */
  | { type: "score"; instructions: string; criteria: string[] }
  /** Yes or no. */
  | { type: "bool"; instructions: string; criteria: { true: string; false: string } };

interface ClassifierResult {
  provider: string;
  model: string;
  /** Answers by question ID. */
  answers: Record<string, ClassifierAnswer>;
  usage?: ModelUsage;
  stopReason: "stop" | "error" | "aborted";
  errorMessage?: string;
}

type ClassifierAnswer =
  | { type: "choice"; choice: string; probabilities: Record<string, number>; confidence: number }
  /** `score` is the expected level index, from 0 to `criteria.length - 1`. */
  | { type: "score"; score: number; confidence: number }
  /** Probability of `true`. */
  | { type: "bool"; probability: number };

/** Token counts and cost in USD, when the service reports them. */
type ModelUsage = { input: number; output: number; totalTokens: number; cost: { total: number } };
```

Classify several items by calling `classify()` once per item. This script sorts feedback messages, for example ones a tool returned earlier in the script:

```js
const [classifier] = await models.getAvailableOfType("classifier", "llama.cpp");
const results = await Promise.all(
  messages.map((message) =>
    models.classify(classifier, {
      state: { message },
      questions: {
        sentiment: {
          type: "choice",
          instructions: "How does the user feel about the product?",
          criteria: { positive: "Satisfied or happy", negative: "Unhappy or frustrated", neutral: "Neither" },
        },
        urgency: {
          type: "score",
          instructions: "How urgently does this need a reply?",
          criteria: ["no reply needed", "reply this week", "reply today"],
        },
      },
    }),
  ),
);
return results.map((result, i) =>
  result.stopReason === "stop"
    ? { message: messages[i], sentiment: result.answers.sentiment.choice, urgency: result.answers.urgency.score }
    : { message: messages[i], error: result.errorMessage },
);
```

### Generate images

```ts
interface ImagesContext {
  /** The prompt as text blocks, plus image blocks to edit or use as references. */
  input: (TextBlock | ImageBlock)[];
}

interface ImagesResult {
  provider: string;
  model: string;
  /** Generated images, and text blocks for models that also return text. */
  output: (TextBlock | ImageBlock)[];
  usage?: ModelUsage;
  stopReason: "stop" | "error" | "aborted";
  errorMessage?: string;
}

type TextBlock = { type: "text"; text: string };
/** `data` is base64. */
type ImageBlock = { type: "image"; data: string; mimeType: string };
```

Show generated images with `image(block)`. Do not print `data` with `text()`, `console`, or `return`: it is large and the model cannot read it as text. `image()` also saves each image to a temp file and puts its path in the result, so a later turn can copy or move the file.

```js
// @options: {"timeout_ms": 300000}
const painter = await models.getModelOfType("image", "openrouter", "google/gemini-2.5-flash-image");
const result = await models.generateImages(painter, {
  input: [{ type: "text", text: "A red fox in the snow, watercolor" }],
});
if (result.stopReason !== "stop") return result.errorMessage;
for (const block of result.output) {
  if (block.type === "image") image(block);
  else text(block.text);
}
```

## Limits

- A script's V8 isolate has a heap limit of 256 MB. V8 cannot recover from running out of heap, so this cannot be caught with `try`/`catch`: the script ends at once and fails with `InternalError: out of memory`, keeping the output printed before. The error has no script frames in its stack. Memory behind `ArrayBuffer`s and typed arrays is not part of the V8 heap; it counts against the same 256 MB, and allocating past it throws the same `InternalError`, which a script can catch. Filter or aggregate large data instead of accumulating it.
- Each script runs in a process of its own (the `cyrup` binary started with the hidden `__codemode-sandbox` argument), with a cleared environment (only `LD_LIBRARY_PATH`, `DYLD_LIBRARY_PATH` and `SYSTEMROOT` are passed on), no core dumps and a cap on writable memory of about 1.3 GB (`RLIMIT_DATA`, which macOS does not enforce and Windows does not have: there the process is still disposable, but only the heap limit and the `ArrayBuffer` budget bound its memory). Whatever a script does to its process (one allocation V8 cannot satisfy, such as `new Array(2 ** 27).fill(0)`, which aborts the engine, or a built-in that never yields), your session sees a failed script: `InternalError: out of memory` when the engine reports it, or `Script sandbox failed: The sandbox process crashed ...` naming the signal when it dies without a word. A timeout, an abort or the end of the tool call kills the process. Starting it costs about 35 ms.
- A script that waits on a promise that can never settle (no tool call pending) fails immediately, since there are no timers. For the same reason there is no `sleep`; the only thing a script can wait on is a tool call.
- Everything that crosses to the host is JSON: tool arguments and results, `store()` values, and the return value. A `Date` arrives as its ISO string, `undefined` members of an object are dropped, and functions and symbols have no JSON form. A string with a lone surrogate (what `slice()` leaves when it cuts an emoji in two) arrives with U+FFFD in its place, as it prints with `text()`; upstream passes the lone surrogate through, and here the host could not read it.
- The value a script returns is output too. Its JSON text is added to the characters that `text()`, `image()` and `console` calls already produced, and past 16777216 the script fails with `RangeError: script output exceeded the limit of 16777216 characters: the returned value is N characters of JSON ...`. The error cannot be caught, because the script has already ended when the value is read. Upstream does not count it. The host keeps a returned value as the JSON text the script wrote and prints that, without reading it into a parsed value, so a returned value costs it a few copies of its text whatever its shape (a returned array of 1.25 million `{a: n}` objects, 16 MB of JSON, took the host about 80 MB; reading it into a parsed value took about 0.7 GB). It is still shown to the model, so return a summary, or write large data to a file with a tool.
- Built-ins are frozen, so a polyfill such as `Array.prototype.chunk = ...` is silently ignored and `Array.prototype.chunk` stays undefined; write a helper function instead.
- The engine is V8, so syntax errors and engine error messages read as V8's, and newer language features exist (`Temporal`, the `Iterator` helpers).
- A `store()` value or the arguments of a tool call nested deeper than 64 levels throw a `RangeError` at the call, which the script can catch; flatten the value or pass the deep part as a JSON string. A returned value has no depth limit beyond what `JSON.stringify` writes.
- At most 16 tool calls of one script run at the same time; the rest of a `Promise.all` over thousands of items wait for a free place and start in order, so 3000 parallel `read`s finish in seconds instead of holding gigabytes. A call that waits has no row in the call list yet. Calls that depend on each other to finish (one holding for another) can therefore deadlock beyond 16 of them; await them in batches instead.
- A script may have at most 10000 tool calls started and not yet settled. The call past that throws a `RangeError` where it is made, which also ends a loop that starts calls without awaiting them (`for (;;) tools.read(...)`) at once instead of at the timeout. Await calls in batches.
- The arguments of the calls started and not yet settled may weigh 33554432 together (32 Mi), and the call that would pass that throws a `RangeError` where it is made, naming the weight, so a loop of 5000 `tools.write` calls with 1 MiB of content each stops after about 31 instead of holding gigabytes in the session. A character of JSON weighs 1, but data made of many small values costs the host many times its text, so each comma weighs 48 and each array and object 256: 32 Mi is about 30 million characters of strings, 600 thousand array elements, or 100 thousand small objects (`{ a: 1 }`) in all the calls in flight. A single call whose arguments weigh more than that can never be made: write the data to a file with a tool, or pass it in pieces. Calls that settled free their weight, so awaiting calls in batches keeps a script under it.
- The call list of a running script is shown as it changes, at most ten times a second, and always with its latest state. It keeps the latest 500 calls; earlier ones are folded into a first row, `... N earlier calls`, in the live list and in the result's `details`. The list of calls a failed script made, which the model reads, is the same list. The `nestedCalls` that the session stores keep their own, smaller caps.
- Scripts cannot start other `codemode` scripts.

## Security model

A script runs in a sandbox that is created for one `codemode` call and discarded at its end. What that does and does not protect:

- **A fresh isolate, in a process of its own, for every call.** Nothing a script defines survives into the next call. What does is what it hands to `store()`, a JSON value the session records. The process is the `cyrup` binary started with the hidden `__codemode-sandbox` argument, with a cleared environment: only `LD_LIBRARY_PATH`, `DYLD_LIBRARY_PATH` and `SYSTEMROOT` are passed on, so your other environment variables, API keys included, are not in it.
- **No ambient authority inside it.** There is no file system, network, timers, Node API, `WebAssembly`, `SharedArrayBuffer`, `Atomics.wait`, `Intl` or dynamic `import()`, and the built-ins are frozen. The only ways out are `tools.*` and `models.*`.
- **The tools are the exposure.** A tool call from a script is a real call with your privileges: `tools.bash` runs a real shell command and `tools.write` writes a real file, and the sandbox does not make either safer. What stands in front of them is the permission gate, which treats every nested call as a call of its own (see [Permissions](#permissions)), and `--tools` / `--exclude-tools`, which decide what exists in `tools` at all. Allowing `codemode` allows no other tool.
- **The operating system does not confine the sandbox process further.** cyrup gives it a cleared environment, no core file and a memory ceiling, and runs it as you. It adds no seccomp filter, namespace, chroot or change of user. The boundary is the engine and the empty global object, so a flaw in V8 would run with your privileges. Read the sandbox as protection against a script's mistakes and against what a script can reach through the language, and the permission policy as the protection against what tools can do.

What happens when a script reaches a limit (the numbers are under [Limits](#limits)):

| Limit | What happens |
|---|---|
| V8 heap, 256 MB | The script ends at once with `InternalError: out of memory` and keeps its earlier output. It cannot be caught. |
| `ArrayBuffer` and typed-array memory, counted against the same 256 MB | The allocation throws the same `InternalError`, which a script can catch. |
| Process memory ceiling, about 1.3 GB (Unix without macOS) | An allocation fails inside the sandbox process: `InternalError: out of memory`, or `Script sandbox failed: The sandbox process crashed ...` naming the signal. Your session is not affected. |
| Running time, 120 s of the script's own JavaScript or 30 min in all, or `timeout_ms` when set | The process is killed and the script fails with `Script timed out`. Calls in flight are cancelled, and a `bash` command one of them was running is stopped: the failed result lists it as `bash (cancelled)`. |
| 16 calls at once, 10000 not yet settled, arguments of the unsettled ones weighing 32 Mi | Calls past 16 wait for a free place. The call past 10000, or the one that would take the arguments in flight past 32 Mi, throws a `RangeError`. |
| Output past 16777216 characters (the returned value's JSON text included) or 100000 `text()`, `image()` and `console` calls, an `image()` past 5 MiB | The script fails; the image throws a `RangeError`. |

An abort (a cancelled turn, a closed session, an RPC abort) kills the process the same way and cancels the calls in flight.

## Files a script leaves behind

Two things a script produces are written to the system temp directory (`$TMPDIR`, or `/tmp`, on Unix) and named in the result, and a third is left there without being named:

- **Full output.** Output longer than `max_output_tokens` is shortened, and the whole text is written to `pi-codemode-<16 hex digits>.txt`. The result ends with `[Full output: <path> (read or tools.read with offset/limit)]`, or with `[Could not save the full output: <error>]` when the write failed.
- **Images.** Each `image()` is saved as `pi-codemode-<16 hex digits>` with the extension of its type (`.png`, `.jpg`, `.gif` or `.webp`), and a line `[Image saved to <path> (<mime type>, <size>)]` names it before the image. An image shown twice is saved once. A failed write becomes the label `[Image (<mime type>, <size>) could not be saved: <error>]` and does not end the script.

- **Output of the script's own `tools.bash` and `tools.powershell` calls.** The tool writes a command's whole output to `cyrup-bash-<id>.log` (`cyrup-powershell-<id>.log`) once it passes the 2000 lines or 50KB that the model sees, which is a lower limit than the 1 MiB a script receives (see [Call tools](#call-tools)). A script that filters a large command's output therefore leaves one such file per call, with all of that output in it, although the result it received says `truncated: false` and carries no `full_output_path`. Only a result that is itself truncated names its file.

The files are created exclusively (an existing path is an error, never overwritten) and are readable only by you (mode `0600` on Unix). **cyrup never removes them.** They stay until you delete them or the system clears its temp directory, and they hold whatever the script printed or showed, including data it read from files and the output of the commands it ran. Clear `pi-codemode-*` and `cyrup-bash-*` (`cyrup-powershell-*`) out of the temp directory now and then, or point `TMPDIR` at a directory you clean.

## What you see

**Terminal interface.** A `codemode` call is one tool row; the calls the script makes are drawn inside it, never as rows of their own.

- The call shows the script, highlighted, cut to its first 10 lines (wrapped lines count) with `... (N more lines, Ctrl+O to expand)`.
- While the script runs, the row lists its calls: `…` running, `✓` ok, `✗` failed, `⊘` cancelled, then the tool, its arguments (cut to 80 characters while collapsed), how long it took, and its cost for a model call. A `bash` command that exits non-zero shows `✗`, although the script received its result. Collapsed, only the latest 8 calls are listed after `... (N earlier calls, Ctrl+O to expand)`; expanded lists them all, with the error text of each failed one. When more than one call has a cost, a `Model calls: $0.0042` line gives the total.
- When the script ends, its output replaces the `Script completed` header: up to 5 lines collapsed, then `... (N more lines, Ctrl+O to expand)`, and a `Full output: <path>` line when the output went to a file. A failed script shows its output in the error colour, ending with `Script error:` and the stack, whose frames are `codemode.js:LINE:COLUMN`.
- Images a script shows with `image()` are image blocks of the result, and the tool row draws them like the images of any other tool: as cells of the picture when the terminal has an image protocol and `terminal.showImages` is on (see [settings](../reference/settings.md)), otherwise as a one-line `[Image: [image/png] 1x1]` placeholder. The line `[Image saved to <path> ...]` names the file either way.

`Ctrl+O` expands or collapses every tool row at once. See [The terminal interface](../guides/tui.md#inspecting-tool-output).

**`--mode json` and `--mode rpc`.** The calls a script makes are `tool_execution_start`, `tool_execution_update` and `tool_execution_end` events of their own, with the id `<codemode call id>/<n>` and the `codemode` call's id in `parentToolCallId`. A consumer that counts tool calls should skip events that carry one.

**Zed and other ACP editors.** The editor shows one tool call, `codemode`, and the nested calls are not sent as tool calls of their own. While the script runs, the call's updates carry the partial result as raw JSON, `{"content": [], "details": {"calls": [...]}}` with one entry per nested call and its status, which an editor shows as plain text; when the script ends the content is the script's output, or `Script failed` and the error. The output keeps the lines it had for the model: each `text()` or `console.*` call is a line of its own, and an `image()` is shown as an image beside the line that says where it was saved. The image data also stays in the call's `rawOutput`, as it does in pi-acp, so an editor is sent it twice. (pi-acp joins the blocks of a result with nothing, which printed `first linesecond line`; cyrup joins every tool's text blocks with a newline, except a terminal's output, which stays one stream.) A nested call that needs approval does reach the editor, as a permission request whose text ends `(from codemode script)`.

**The session file.** The `codemode` result message carries `details.calls` (what the terminal interface draws) and `nestedCalls` (id, name, status, arguments and duration of each nested call, with a flag saying whether the list is complete). Each script that stores values appends a `codemode-store` entry.

## Troubleshooting

**`codemode` is not offered to the model.** It is registered but off by default, unless a permission policy is armed, which turns it on. Check, in order: `"defaultTools": ["+codemode"]` is in the global `settings.json`, or in a project's `.cyrup/settings.json` of a trusted project (a `-p` run needs `--approve`); `--tools` names it when you pass `--tools`; `--no-extensions` is not set; `--exclude-tools` does not match it; the policy does not `deny` it. Startup says `defaultTools: no activatable tool is registered as "codemode"` when nothing registered it. The opposite surprise, `codemode` offered although you did not turn it on, is a permission policy (a `cyrup-permissions.jsonc` in the agent directory or in `.cyrup/agent/` of the repository is enough): deny it or pass `--exclude-tools codemode`. See [Enable codemode](#enable-codemode).

**`tools.grep does not exist` (or `find`, `ls`, `bash`, an MCP tool).** The error lists what does exist. A `direct` tool must be active to be callable: `grep`, `find` and `ls` are off in a default session (a permission policy turns them on), and eager MCP tools are off under `--tools read,codemode`. A permission `deny`, `--tools` and `--exclude-tools` remove a tool altogether. See [Call tools](#call-tools).

**`... requires approval, but no interactive UI is available (from codemode script)`.** The policy says `ask` for that call, and `-p` and `--mode json` have nobody to ask. Give the call an `allow` rule, or run interactively. (`--mode rpc` does not end here: it asks the client, which answers the `extension_ui_request`.) See [Permissions](#permissions).

**`Script timed out`.** The script ran past 120 seconds of its own JavaScript or 30 minutes in all. Waiting on a tool does not count towards the 120 seconds. Raise it with `timeout_ms` in the options line, or batch the work into several calls.

**`InternalError: out of memory`.** The script used more than 256 MB. Filter or aggregate large data instead of accumulating it; read a file's part with `offset` and `limit`.

**`Script sandbox failed: The sandbox process crashed ...`.** The sandbox process died without reporting, and the message names the signal. Your session is fine. Rerun with less data; a small script that crashes the sandbox every time is a bug worth reporting, with the signal the message names. If cyrup was upgraded or reinstalled while the session was open, the message instead ends `restart cyrup` (`Failed to start the sandbox process: No such file or directory ...`, or `it exited with status N` before the script printed anything): the sandbox process is started from the cyrup file, and on non-Linux systems that is no longer the one this session runs. Restart cyrup. On Linux the process is started from the image the session is running, so an upgrade does not affect it.

**A script hangs with many calls in flight.** At most 16 of a script's calls run at the same time. Calls that wait for each other, such as a call that holds a lock another call needs, can fill all 16 places and never finish; await them in batches.

**A settings change has no effect.** `codemode.mode`, `codemode.inlineBudget` and `defaultTools` are read when a session is built. Run `/reload` or restart. `/reload` activates names added to `defaultTools` but does not deactivate names you removed.

**An MCP server's tools are missing from `ALL_TOOLS`.** A script that names the server or calls `searchTools()`, `describeTool()`, `describeNamespace()` or reads `ALL_TOOLS` waits for servers that are still connecting (see [Call tools](#call-tools), at most two minutes and not again once it has run out); a server that failed to connect has no tools to list (`/mcp` shows its state). An eager server's tools are also missing when they are not active; see [MCP tools](#mcp-tools).

**Temp files pile up.** A script leaves `pi-codemode-*` files, and a `cyrup-bash-*.log` for each `tools.bash` call whose output passed the model's limit, even when that result was not truncated. See [Files a script leaves behind](#files-a-script-leaves-behind).
