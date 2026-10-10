(function (toolsJson, globalsJson, storeJson, memoryStride) {
	"use strict";
	// The only host entry points. They are captured here and `Deno` is deleted before the script
	// runs, so the script cannot reach them (see the end of this function).
	const core = Deno.core;
	const hostOps = core.ops;
	const opCall = hostOps.op_codemode_call;
	const opOutput = hostOps.op_codemode_output;
	const opDoneOk = hostOps.op_codemode_done_ok;
	const opDoneErr = hostOps.op_codemode_done_err;
	const opHold = hostOps.op_codemode_hold;
	const opRelease = hostOps.op_codemode_release;
	const opPromiseHandled = hostOps.op_codemode_promise_handled;
	const opMemoryExceeded = hostOps.op_codemode_memory_exceeded;
	const opDrainRejections = hostOps.op_drain_pending_rejections;

	function bridge(kind, a, b, c, d) {
		switch (kind) {
			case "call":
			case "global":
				opCall(a, kind, b, c === undefined ? "" : c, c !== undefined);
				break;
			case "output":
				opOutput(a, b, c === undefined ? "" : c);
				break;
			case "done":
				if (a) opDoneOk(b === undefined ? "" : b, b !== undefined, c, d);
				else opDoneErr(b);
				break;
		}
	}
	const nativeStringify = JSON.stringify;
	const parse = JSON.parse;
	const promiseThen = Promise.prototype.then;
	const ErrorCtor = Error;
	const TypeErrorCtor = TypeError;
	const RangeErrorCtor = RangeError;
	const pending = new Map();
	// What the arguments of the calls in `pending` weigh together.
	let pendingWeight = 0;
	let nextId = 1;
	let finished = false;
	// [CYRUP-DELTA] Whether the script has run to its end and its settlement waits, in the host's hands,
	// for the engine to look at the rejections it left behind (`release`).
	let held = false;
	// Thrown by exit() to unwind the script after it already reported success.
	const EXIT = Object.freeze({});

	function done(ok, payload, writes) {
		if (finished) return;
		finished = true;
		bridge("done", ok, payload, writes, ok ? unobservedReport() : undefined);
	}

	// [CYRUP-DELTA] A string with a lone surrogate (a `slice` or `substring` that cut an emoji,
	// `split("").reverse()`) is written by JSON.stringify as an escape such as `\ud83d`. JSON allows
	// it and upstream's `JSON.parse` reads it back, but the host parses with serde_json, which
	// refuses an unpaired surrogate escape. The tool call, the return value, the store write or the
	// thrown error that carried one then failed (or ended the whole execution as "Sandbox bridge
	// broken", blaming built-ins the script never touched) after its tool calls had already run.
	// Every JSON text this prelude hands to the host is therefore written with U+FFFD in place of
	// each lone surrogate, which is what `text()` and `console.*` already print for one (the engine
	// converts the string for the host lossily). A well-formed JSON.stringify writes lowercase hex
	// and only escapes surrogates that have no partner, and a backslash that is itself escaped
	// (`\\ud83d`, the text of an escape) is skipped by the pair of backslashes.
	const LONE_SURROGATE_ESCAPE = /(?<!\\)((?:\\\\)*)\\ud[89a-f][0-9a-f]{2}/g;
	function wellFormedJson(json) {
		return json === undefined || json.indexOf("\\ud") === -1 ? json : json.replace(LONE_SURROGATE_ESCAPE, "$1\uFFFD");
	}

	// `JSON.stringify` for everything that crosses to the host or is shown as text.
	function stringify(value, replacer) {
		return wellFormedJson(nativeStringify(value, replacer));
	}

	// The same for a raw string, such as a store key, which is a JSON string on the way out.
	const LONE_SURROGATE = /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/g;
	function wellFormed(text) {
		return text.replace(LONE_SURROGATE, "\uFFFD");
	}

	function serialize(value) {
		return value === undefined ? undefined : stringify(value);
	}

	// [CYRUP-DELTA] The script's return value, rendered as above (a returned Error is its stack text).
	function serializeResult(value) {
		return value === undefined ? undefined : renderJson(value);
	}

	// [CYRUP-DELTA] The host reads the JSON of a store() value and of tool arguments with a parser
	// that stops at 128 levels (serde_json's recursion bound); V8's JSON.stringify has none. A value
	// nested deeper ended the whole execution as "Sandbox bridge broken", after the script's tool
	// calls had already run. So the depth is checked where the value is made, and a deeper one is a
	// RangeError the script can catch. The limit leaves room for the entries the session wraps a
	// store value in.
	const MAX_JSON_DEPTH = __MAX_JSON_DEPTH__;
	const MAX_PENDING_CALLS = __MAX_PENDING_CALLS__;
	const MAX_PENDING_ARGUMENT_WEIGHT = __MAX_PENDING_ARGUMENT_WEIGHT__;
	const COMMA_WEIGHT = __ARGUMENT_COMMA_WEIGHT__;
	const CONTAINER_WEIGHT = __ARGUMENT_CONTAINER_WEIGHT__;
	const WEIGHT_NOTE =
		"a character of JSON weighs 1, a comma __ARGUMENT_COMMA_WEIGHT__, and an array or object __ARGUMENT_CONTAINER_WEIGHT__, as data made of many small values costs the host many times its text";

	// One pass over the JSON text of a value: how deeply it nests, and how many commas and arrays
	// and objects it holds. Stops at the first level past the limit.
	function scanJson(json) {
		let depth = 0;
		let deepest = 0;
		let commas = 0;
		let containers = 0;
		let inString = false;
		for (let i = 0; i < json.length; i++) {
			const c = json.charCodeAt(i);
			if (inString) {
				if (c === 92) i++;
				else if (c === 34) inString = false;
			} else if (c === 34) {
				inString = true;
			} else if (c === 91 || c === 123) {
				containers++;
				if (++depth > deepest) deepest = depth;
				if (deepest > MAX_JSON_DEPTH) break;
			} else if (c === 93 || c === 125) {
				depth--;
			} else if (c === 44) {
				commas++;
			}
		}
		return { deepest, commas, containers };
	}

	// Returns what scanJson found, or nothing for no JSON.
	function checkDepth(what, json) {
		if (json === undefined) return undefined;
		const scan = scanJson(json);
		if (scan.deepest > MAX_JSON_DEPTH) {
			throw new RangeErrorCtor(
				what + " is nested more than __MAX_JSON_DEPTH__ levels deep, which is more than the host can read. Flatten it, or pass the deep part as a string with JSON.stringify.",
			);
		}
		return scan;
	}

	// V8 stacks already start with "Name: message" followed by the frames. The text is rebuilt from
	// the error's current name and message and the frame lines alone, so it reads the same whether
	// or not the stack was captured before the error was renamed, and this prelude's frames are
	// dropped.
	function scriptFrames(stack) {
		return (typeof stack === "string" ? stack : "")
			.split("\n")
			.filter((line) => /^\s+at /.test(line) && !line.includes("codemode-prelude.js"));
	}

	function errorText(error) {
		const head = error.message ? error.name + ": " + error.message : String(error.name);
		return [head, ...scriptFrames(error.stack)].join("\n");
	}

	// [CYRUP-DELTA] How values that JSON.stringify drops or refuses are shown wherever the host shows
	// text: the script's return value, text(), and console.*. Upstream renders an Error, a Map and a
	// Set as {} (the stack is only in console.log), and a BigInt makes the whole script fail with
	// "Do not know how to serialize a BigInt". A value JSON.stringify renders without help (every
	// JSON value) is not touched, so it is byte-identical to upstream.
	//  - an Error: its stack text, "Name: message" and the script frames;
	//  - a Map: its [key, value] entries; a Set: its values;
	//  - a BigInt: its decimal digits as a string.
	const arrayFrom = Array.from;
	const MapCtor = Map;
	const SetCtor = Set;
	function renderValue(key, value) {
		if (typeof value === "bigint") return String(value);
		if (typeof value === "object" && value !== null) {
			if (value instanceof ErrorCtor) return errorText(value);
			if (value instanceof MapCtor || value instanceof SetCtor) return arrayFrom(value);
		}
		return value;
	}

	// JSON text of a value for display; undefined where JSON.stringify has none (functions, symbols).
	function renderJson(value) {
		return stringify(value, renderValue);
	}

	function format(value) {
		if (typeof value === "string") return value;
		if (typeof value === "bigint") return String(value);
		if (value instanceof ErrorCtor) return errorText(value);
		try {
			const json = renderJson(value);
			return json === undefined ? String(value) : json;
		} catch {
			return String(value);
		}
	}

	// Throws nothing: a hostile thrown value (a revoked proxy, a throwing `stack` getter) must not
	// stop the script's failure from being reported.
	function describeError(error) {
		try {
			// The host requires string fields, and a script can set an error's `name` or `message` to
			// anything, so they are coerced (pi `describeError`, `prelude-source.ts` @v1.0.4).
			if (error instanceof ErrorCtor) {
				return stringify({ name: String(error.name), message: String(error.message), stack: errorText(error) });
			}
			return stringify({ message: String(format(error)) });
		} catch {
			return stringify({ message: "The script threw a value that could not be described" });
		}
	}

	function caller(kind, name, spread) {
		return (...args) => {
			// The script has returned: what a leftover continuation starts is not run, as when the host
			// stopped listening at the return.
			if (held) return new Promise(() => {});
			// [CYRUP-DELTA] Calls the script has started and not yet seen settle are held by the host
			// (their arguments, a task each) for as long as they run, so a loop that starts calls
			// without awaiting them grew the host's memory for the whole time the script was allowed to
			// run. Past the limit the call throws here, at the line that made it, which also ends such a
			// loop at once instead of at the deadline. It throws instead of rejecting because nothing
			// awaits the rejection in the loop this is for.
			if (pending.size >= MAX_PENDING_CALLS) {
				throw new RangeErrorCtor(
					"More than __MAX_PENDING_CALLS__ tool calls are in flight at once. Await some of them before starting more (for example in batches), and do not start calls in a loop without awaiting them.",
				);
			}
			// [CYRUP-DELTA] The rejection of a failed call is made later, from the host's reply, where no
			// frame of the script is on the stack, so it read "Error: msg" with nothing to say which call
			// failed (under Promise.all, none of them). The stack is taken here, where the script
			// calls, and its script frames become the rejection's.
			const site = new ErrorCtor();
			let json;
			let weight = 0;
			try {
				json = serialize(spread ? args : args[0]);
				const scan = checkDepth("the argument", json);
				if (scan !== undefined) weight = json.length + scan.commas * COMMA_WEIGHT + scan.containers * CONTAINER_WEIGHT;
			} catch (error) {
				return new Promise((resolve, reject) => {
					reject(error);
				});
			}
			// [CYRUP-DELTA] The count above says nothing of size: the host holds every unsettled call's
			// arguments (as text on the way, then as a parsed value, several times over for the sixteen
			// that run), and a loop of 5000 calls with 1 MiB arguments took the host past a gigabyte
			// and the sandbox process down before the script could be told. What the arguments weigh
			// counts against a limit too, and the call that would pass it throws here for the same reason
			// as above. A character weighs one, but data made of many small values costs the host many
			// times its text (two calls with 13 MB arrays of `{ a: n }` took it to 3.2 GB), so each comma
			// and each array or object weighs more.
			if (pendingWeight + weight > MAX_PENDING_ARGUMENT_WEIGHT) {
				throw new RangeErrorCtor(
					weight > MAX_PENDING_ARGUMENT_WEIGHT
						? "The arguments of this call weigh " +
								weight +
								", more than the limit of __MAX_PENDING_ARGUMENT_WEIGHT__ for the calls in flight together (" +
								WEIGHT_NOTE +
								"). Send less in one call: write the data to a file with a tool, or pass it in pieces."
						: "The calls in flight hold arguments that weigh " +
								pendingWeight +
								", and this call's " +
								weight +
								" more would pass the limit of __MAX_PENDING_ARGUMENT_WEIGHT__ (" +
								WEIGHT_NOTE +
								"). Await some of them before starting more (for example in batches), and do not start calls in a loop without awaiting them.",
				);
			}
			let entry;
			const promise = new Promise((resolve, reject) => {
				const id = nextId++;
				entry = { resolve, reject, site, weight, name, promise: undefined };
				pending.set(id, entry);
				pendingWeight += weight;
				bridge(kind, id, name, json);
			});
			// Kept so the end of the script can ask whether anything was waiting for the call
			// (`unobservedReport`).
			entry.promise = promise;
			return promise;
		};
	}

	// The error a call rejects with, carrying the frames of the script that made the call.
	function atCallSite(error, site) {
		try {
			const frames = scriptFrames(site.stack);
			const head = error.message ? error.name + ": " + error.message : String(error.name);
			error.stack = [head, ...frames].join("\n");
		} catch {
			// A stack that cannot be set stays as it is.
		}
		return error;
	}

	// [CYRUP-DELTA] Upstream ends a script whose tool call failed, was not awaited and has no handler as
	// a plain success: the error is nowhere, so a model that forgot an `await` on a `write` or an
	// `edit` is told its side effect happened. The prelude keeps the rejections nobody handled (the
	// engine reports each one as it finds it, and tells when a handler arrives late), and the script's
	// result carries them (`unobservedReport`, read by `execution::unobserved_errors`). Whether a
	// rejection is a tool call's is known from the error object, made here.
	const MAX_UNOBSERVED_SHOWN = __MAX_UNOBSERVED_SHOWN__;
	const MAX_UNOBSERVED_CHARS = __MAX_UNOBSERVED_CHARS__;
	const MAX_UNOBSERVED_TRACKED = __MAX_UNOBSERVED_TRACKED__;
	// The error a failed call rejected with -> the tool it called.
	const failedCalls = new WeakMap();
	// Rejected promises that had no handler when the engine looked, in the order it found them.
	const unhandled = new Map();
	let unhandledUntracked = 0;

	function rejectCall(entry, error) {
		atCallSite(error, entry.site);
		try {
			failedCalls.set(error, entry.name);
		} catch {
			// An error that cannot be tracked is still delivered.
		}
		entry.reject(error);
	}

	// One line of what the script would have seen: "Name: message (frame)". Throws nothing: a hostile
	// rejection (a revoked proxy, a throwing getter) is reported as such.
	function describeUnhandled(reason, failedCall) {
		let text;
		try {
			if (reason instanceof ErrorCtor) {
				// A failed call's error is a plain `Error` carrying the tool's text; its name adds nothing.
				const bare = failedCall && reason.name === "Error";
				const head = reason.message
					? bare
						? String(reason.message)
						: reason.name + ": " + reason.message
					: String(reason.name);
				const frames = scriptFrames(reason.stack);
				text = frames.length === 0 ? head : head + " (" + frames[0].trim().replace(/^at /, "") + ")";
			} else {
				text = String(format(reason));
			}
		} catch {
			text = "an error that could not be described";
		}
		text = wellFormed(text.replace(/\s+/g, " "));
		return text.length > MAX_UNOBSERVED_CHARS ? text.slice(0, MAX_UNOBSERVED_CHARS) + "\u2026" : text;
	}

	function noteUnhandled(promise, reason) {
		if (unhandled.size >= MAX_UNOBSERVED_TRACKED) {
			unhandledUntracked++;
			return;
		}
		let call;
		try {
			call = reason instanceof ErrorCtor ? failedCalls.get(reason) : undefined;
		} catch {
			call = undefined;
		}
		unhandled.set(promise, { call, message: describeUnhandled(reason, call !== undefined) });
	}

	// Returning true tells the engine the rejection is accounted for: it is held until the script ends
	// instead of failing the event loop, and dropped if a handler arrives meanwhile.
	core.setUnhandledPromiseRejectionHandler((promise, reason) => {
		noteUnhandled(promise, reason);
		return true;
	});
	core.setHandledPromiseRejectionHandler((promise) => {
		unhandled.delete(promise);
	});

	// What is left unhandled as the script ends, as JSON for the host. The engine reports a rejection
	// when the event loop next runs, which is after the script's last statement, so the ones it has
	// not reported yet are taken from it here.
	function unobservedReport() {
		try {
			const waiting = opDrainRejections();
			if (waiting !== undefined) {
				for (let i = 0; i + 1 < waiting.length; i += 3) noteUnhandled(waiting[i], waiting[i + 1]);
			}
			const shown = [];
			for (const record of unhandled.values()) {
				if (shown.length >= MAX_UNOBSERVED_SHOWN) break;
				shown.push(record);
			}
			// [CYRUP-DELTA] The calls the script started and never saw settle, in two lists. The host
			// knows which of them failed after the script had ended. `unsettled` holds those nothing
			// was waiting on: the script lost their errors. `waited` holds those something was waiting
			// on (an `await` in an `async` function nobody awaited, a `.then()`, a `.catch()`, a
			// `Promise.all` that had already rejected on a sibling): the engine cannot tell which of
			// these the script meant to be done with and which it needed, so the host names their
			// failures in words that claim neither. The engine says whether a reaction is attached;
			// the script cannot tell it from the outside, since `await` does not call `then`.
			const abandoned = [];
			const waited = [];
			for (const [id, entry] of pending) {
				(opPromiseHandled(entry.promise) ? waited : abandoned).push(id);
			}
			return stringify({
				unhandled: shown,
				total: unhandled.size + unhandledUntracked,
				unsettled: abandoned,
				waited,
			});
		} catch {
			return "{}";
		}
	}

	const tools = Object.create(null);
	const allTools = [];
	for (const { name, jsName, description } of parse(toolsJson)) {
		const fn = caller("call", name);
		// [CYRUP-DELTA] Upstream keeps the first tool when two names normalize to the same identifier
		// and drops the other. The host (IdentifierTable) gives each tool an identifier of its own, so
		// the check only guards a host that did not.
		if (!(jsName in tools)) {
			tools[jsName] = fn;
			allTools.push(Object.freeze({ name: jsName, description }));
		}
		if (!(name in tools)) tools[name] = fn;
	}
	Object.freeze(tools);
	Object.freeze(allTools);

	// Reading a member that does not exist throws an error that names the close matches, instead of
	// a later "not a function". `in` checks still work.
	const comparable = (name) => name.toLowerCase().replace(/[^a-z0-9]/g, "");
	function guard(target, label, names, hint) {
		return new Proxy(target, {
			get(object, property, receiver) {
				if (typeof property !== "string" || property in object || property in Object.prototype || property === "then" || property === "toJSON") {
					return Reflect.get(object, property, receiver);
				}
				const wanted = comparable(property);
				const exact = names.filter((name) => comparable(name) === wanted);
				const close = exact.length > 0 ? exact : names.filter((name) => wanted && (comparable(name).includes(wanted) || wanted.includes(comparable(name))));
				let message = label + "." + property + " does not exist.";
				if (close.length > 0) message += " Did you mean " + close.slice(0, 5).map((name) => label + "." + name).join(", ") + "?";
				else if (names.length <= 20) message += " Available: " + names.join(", ") + ".";
				if (hint) message += " " + hint;
				message += ' Check for a member with "' + property + '" in ' + label + ".";
				throw new TypeErrorCtor(message);
			},
		});
	}
	const toolsProxy = guard(
		tools,
		"tools",
		allTools.map((tool) => tool.name),
		"ALL_TOOLS lists every tool; searchTools(query) finds tools by topic.",
	);

	// A read-only, non-deletable property of the global object, whether or not the engine already
	// had one of that name (it has a `console`, which this prelude replaces).
	function define(name, value) {
		delete globalThis[name];
		Object.defineProperty(globalThis, name, { value, enumerable: true, writable: false, configurable: false });
	}

	const namespaces = new Map();
	for (const { name, spread } of parse(globalsJson)) {
		const fn = caller("global", name, spread);
		const dot = name.indexOf(".");
		if (dot === -1) {
			define(name, fn);
			continue;
		}
		const namespace = name.slice(0, dot);
		if (!namespaces.has(namespace)) namespaces.set(namespace, Object.create(null));
		namespaces.get(namespace)[name.slice(dot + 1)] = fn;
	}
	for (const [namespace, members] of namespaces) {
		Object.freeze(members);
		const value = guard(members, namespace, Object.keys(members));
		Object.defineProperty(globalThis, namespace, { value, enumerable: true });
	}

	// key -> JSON text. Sizes count key and JSON characters.
	const stored = new Map(Object.entries(parse(storeJson)));
	const writes = new Map();
	let storedChars = 0;
	for (const [key, json] of stored) storedChars += key.length + json.length;

	const STORE_HINT =
		"store() is for small state such as IDs or summaries. Show images with image(), keep large data in variables, or write it to a file with a tool.";

	// The key as the host will hold it: a lone surrogate in it is U+FFFD there (see wellFormedJson), so
	// a `load()` of the same key in a later script finds what `store()` wrote.
	function checkKey(name, key) {
		if (typeof key !== "string") throw new TypeError(name + "() key must be a string");
		return wellFormed(key);
	}

	function store(key, value) {
		key = checkKey("store", key);
		const previous = stored.has(key) ? key.length + stored.get(key).length : 0;
		if (value === undefined) {
			stored.delete(key);
			storedChars -= previous;
			writes.set(key, undefined);
			return;
		}
		let json;
		try {
			json = stringify(value);
		} catch (error) {
			throw new TypeError("store(" + stringify(key) + ") value is not JSON-serializable: " + format(error));
		}
		if (json === undefined) {
			throw new TypeError("store(" + stringify(key) + ") value is not JSON-serializable");
		}
		checkDepth("store(" + stringify(key) + ") value", json);
		if (json.length > __MAX_STORE_VALUE_CHARS__) {
			throw new RangeError(
				"store(" + stringify(key) + ") value has " + json.length + " characters of JSON, more than the limit of __MAX_STORE_VALUE_CHARS__. " +
					STORE_HINT,
			);
		}
		const next = storedChars - previous + key.length + json.length;
		if (next > __MAX_STORE_TOTAL_CHARS__) {
			throw new RangeError(
				"store is full: stored values would exceed __MAX_STORE_TOTAL_CHARS__ characters of JSON. Delete keys with store(key, undefined). " +
					STORE_HINT,
			);
		}
		stored.set(key, json);
		storedChars = next;
		writes.set(key, json);
	}

	function load(key) {
		const json = stored.get(checkKey("load", key));
		return json === undefined ? undefined : parse(json);
	}

	function serializeWrites() {
		const entries = [];
		for (const [key, json] of writes) entries.push(json === undefined ? [key] : [key, json]);
		return stringify(entries);
	}

	define("store", store);
	define("load", load);

	let outputChars = 0;
	let outputItems = 0;

	// Past the output limits the script fails: done() reports the error, so catching it does not
	// resume output, and the host ends the script.
	function output(kind, data, mimeType) {
		if (finished || held) return;
		outputChars += data.length;
		outputItems++;
		if (outputChars > __MAX_OUTPUT_CHARS__ || outputItems > __MAX_OUTPUT_ITEMS__) {
			const error = new RangeErrorCtor(
				"script output exceeded the limit of __MAX_OUTPUT_CHARS__ characters or __MAX_OUTPUT_ITEMS__ text(), image(), and console calls. " +
					"Print a summary instead, or write large data to a file with a tool.",
			);
			done(false, describeError(error));
			throw error;
		}
		bridge("output", kind, data, mimeType);
	}

	// [CYRUP-DELTA] The script's return value counts against the same character limit. The host reads
	// it as JSON, builds a value from it and renders that as text, which holds many times the JSON's
	// size for as long as it takes (a script that returned three million objects, 41 MB of JSON, took
	// the host 17 seconds and 1.3 GB), and upstream's limit never saw it because only text(),
	// image() and console.* went through output(). Returns the JSON, or throws the RangeError that
	// ends the script as a failure. It is thrown here, not through output(), because the script has
	// already finished and done() reports it.
	function checkResultSize(json) {
		if (json === undefined || outputChars + json.length <= __MAX_OUTPUT_CHARS__) return json;
		throw new RangeErrorCtor(
			"script output exceeded the limit of __MAX_OUTPUT_CHARS__ characters: the returned value is " +
				json.length +
				" characters of JSON" +
				(outputChars > 0 ? ", after " + outputChars + " characters of text(), image(), and console output" : "") +
				". Return a summary instead, or write large data to a file with a tool.",
		);
	}

	// Primitives become their string form, everything else JSON (an Error its stack text, see
	// renderValue).
	function outputText(value) {
		if (value === undefined || value === null || typeof value !== "object" && typeof value !== "function") {
			return String(value);
		}
		if (value instanceof ErrorCtor) return errorText(value);
		const json = renderJson(value);
		return json === undefined ? String(value) : json;
	}

	function text(value) {
		let rendered;
		try {
			rendered = outputText(value);
		} catch (error) {
			throw new TypeErrorCtor(error instanceof ErrorCtor ? error.message : String(error));
		}
		output("text", rendered);
	}

	function imageUrl(value) {
		if (typeof value === "string") return value;
		if (typeof value !== "object" || value === null || Array.isArray(value)) {
			throw new TypeErrorCtor(__IMAGE_HELPER_EXPECTS__);
		}
		if (value.image_url !== undefined) {
			if (typeof value.image_url !== "string") throw new TypeErrorCtor(__IMAGE_HELPER_EXPECTS__);
			return value.image_url;
		}
		if (typeof value.type !== "string") throw new TypeErrorCtor(__IMAGE_HELPER_EXPECTS__);
		if (value.type !== "image") {
			throw new TypeErrorCtor('image only accepts MCP image blocks, got "' + value.type + '"');
		}
		if (typeof value.data !== "string" || value.data === "") throw new TypeErrorCtor("image expected MCP image data");
		if (value.data.toLowerCase().startsWith("data:")) return value.data;
		return "data:;base64," + value.data;
	}

	// Base64 of the signatures of the formats providers accept inline (PNG, JPEG except
	// JPEG-LS, GIF, "RIFF....WEBP"). Signatures start at byte 0, so their encodings are prefixes.
	const IMAGE_SIGNATURES = [
		["image/png", /^iVBORw0KGg/],
		["image/jpeg", /^[/]9j[/](?!9)/],
		["image/gif", /^R0lGOD[dl]h/],
		["image/webp", /^UklG.{8}RUJQ/],
	];

	function image(value) {
		const url = imageUrl(value);
		if (url === "") throw new TypeErrorCtor(__IMAGE_HELPER_EXPECTS__);
		const colon = url.indexOf(":");
		const scheme = colon === -1 ? "" : url.slice(0, colon).toLowerCase();
		if (scheme === "http" || scheme === "https") {
			throw new TypeErrorCtor("remote image URLs are not supported in tool outputs. Pass a base64 data URI instead");
		}
		const comma = url.indexOf(",");
		const header = comma === -1 ? [] : url.slice(colon + 1, comma).split(";");
		if (scheme !== "data" || comma === -1 || header.slice(1).every((part) => part.toLowerCase() !== "base64")) {
			throw new TypeErrorCtor("invalid image output. Pass a base64 data URI instead");
		}
		// Providers reject the whole request on a bad image, and a persisted image block would be
		// resent on every later turn. Line breaks from wrapped base64 are dropped. The declared type
		// is ignored in favor of the detected one, as providers also reject mismatches.
		const data = url.slice(comma + 1).replace(/\s+/g, "");
		if (data.length % 4 !== 0) {
			throw new TypeErrorCtor("invalid image output. The image data is not valid base64 (truncated or corrupted?)");
		}
		// [CYRUP-DELTA] Providers reject an image over about 5 MB, and a rejected block that was saved
		// to the conversation is sent again with every later turn, so the turn after it fails too.
		// Checked before the pattern, which would walk megabytes first.
		const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
		const bytes = (data.length / 4) * 3 - padding;
		if (bytes > __MAX_IMAGE_BYTES__) {
			throw new RangeErrorCtor(
				"image is " + (bytes / 1048576).toFixed(1) + " MiB, more than the limit of " + (__MAX_IMAGE_BYTES__ / 1048576) +
					" MiB that providers accept. Shrink or re-encode it (a smaller size, or JPEG) with a tool before showing it, or save it to a file instead.",
			);
		}
		if (!/^[A-Za-z0-9+/]+={0,2}$/.test(data)) {
			throw new TypeErrorCtor("invalid image output. The image data is not valid base64 (truncated or corrupted?)");
		}
		const head = data.slice(0, 16);
		const signature = IMAGE_SIGNATURES.find(([, pattern]) => pattern.test(head));
		if (!signature) {
			throw new TypeErrorCtor("invalid image output. The image data is not a PNG, JPEG, GIF, or WebP image");
		}
		output("image", data, signature[0]);
	}

	function exit() {
		if (held) throw EXIT;
		let writesJson;
		try {
			writesJson = serializeWrites();
		} catch (error) {
			done(false, describeError(error));
			throw EXIT;
		}
		done(true, undefined, writesJson);
		throw EXIT;
	}

	const console = {};
	// console.group() indents what the console methods print until the matching groupEnd().
	let indent = "";
	function emit(line) {
		output("console", indent === "" ? line : line.split("\n").map((part) => indent + part).join("\n"));
	}
	const joined = (args) => args.map(format).join(" ");
	for (const level of ["log", "info", "warn", "error", "debug"]) {
		console[level] = (...args) => {
			emit(joined(args));
		};
	}

	// [CYRUP-DELTA] The rest of the console that scripts reach for. Upstream's QuickJS console has
	// log, info, warn, error and debug only, and a call to any other method failed the script with
	// "is not a function". There is no terminal here, so table() prints a Markdown table, and the
	// timers use Date.now().
	const counts = new MapCtor();
	const timers = new MapCtor();
	const now = Date.now;
	const objectKeys = Object.keys;
	const isArray = Array.isArray;
	const label = (value) => (value === undefined ? "default" : String(value));
	const cell = (value) =>
		(value === undefined ? "" : typeof value === "string" ? value : format(value)).replace(/\|/g, "\\|").replace(/\n/g, "\\n");

	console.dir = (value) => {
		emit(format(value));
	};
	console.group = (...args) => {
		if (args.length > 0) emit(joined(args));
		indent += "  ";
	};
	console.groupCollapsed = console.group;
	console.groupEnd = () => {
		indent = indent.slice(2);
	};
	console.assert = (condition, ...args) => {
		if (!condition) emit(args.length > 0 ? "Assertion failed: " + joined(args) : "Assertion failed");
	};
	console.count = (name) => {
		const key = label(name);
		const next = (counts.get(key) || 0) + 1;
		counts.set(key, next);
		emit(key + ": " + next);
	};
	console.countReset = (name) => {
		counts.delete(label(name));
	};
	console.time = (name) => {
		timers.set(label(name), now());
	};
	function elapsed(method, name, extra) {
		const key = label(name);
		if (!timers.has(key)) {
			emit("Warning: No such label '" + key + "' for console." + method + "()");
			return false;
		}
		emit(key + ": " + (now() - timers.get(key)) + "ms" + (extra.length > 0 ? " " + joined(extra) : ""));
		return true;
	}
	console.timeLog = (name, ...extra) => {
		elapsed("timeLog", name, extra);
	};
	console.timeEnd = (name) => {
		if (elapsed("timeEnd", name, [])) timers.delete(label(name));
	};
	console.table = (data, columns) => {
		if (typeof data !== "object" || data === null) {
			emit(format(data));
			return;
		}
		const rows = [];
		if (data instanceof MapCtor) {
			arrayFrom(data).forEach(([key, value]) => rows.push([cell(key), value]));
		} else if (data instanceof SetCtor) {
			arrayFrom(data).forEach((value, index) => rows.push([String(index), value]));
		} else {
			for (const key of objectKeys(data)) rows.push([key, data[key]]);
		}
		const names = [];
		let hasValues = false;
		for (const [, value] of rows) {
			if (typeof value === "object" && value !== null) {
				for (const key of objectKeys(value)) if (!names.includes(key)) names.push(key);
			} else {
				hasValues = true;
			}
		}
		const shown = isArray(columns) ? columns.map(String) : names;
		const header = ["(index)", ...shown, ...(hasValues ? ["Values"] : [])];
		const lines = ["| " + header.join(" | ") + " |", "| " + header.map(() => "---").join(" | ") + " |"];
		for (const [key, value] of rows) {
			const isObject = typeof value === "object" && value !== null;
			const cells = [key, ...shown.map((name) => (isObject && name in value ? cell(value[name]) : ""))];
			if (hasValues) cells.push(isObject ? "" : cell(value));
			lines.push("| " + cells.join(" | ") + " |");
		}
		emit(lines.join("\n"));
	};
	Object.freeze(console);

	define("tools", toolsProxy);
	define("ALL_TOOLS", allTools);
	define("console", console);
	define("text", text);
	define("image", image);
	define("exit", exit);

	// The engine's heap limit does not count the memory behind an ArrayBuffer or a typed array: it
	// is allocated outside the heap, so a script that keeps allocating them would grow the host
	// process without bound. Every way to create one reports to the host, which compares the
	// engine's own count of live backing stores with the memory limit, after a full collection so
	// that garbage is not counted. Past it the allocation throws what QuickJS throws for its
	// memory limit, `InternalError: out of memory`, and the script may catch it.
	//
	// A check is made once `memoryStride` bytes have been allocated since the last one, and at every
	// allocation while the limit is exceeded.
	{
		let sinceCheck = 0;
		let exceeded = false;
		function outOfMemory() {
			const error = new ErrorCtor("out of memory");
			Object.defineProperty(error, "name", { value: "InternalError", writable: true, configurable: true });
			return error;
		}
		function allocated(value) {
			sinceCheck += value.byteLength;
			if (!exceeded && sinceCheck < memoryStride) return value;
			sinceCheck = 0;
			exceeded = opMemoryExceeded();
			if (exceeded) throw outOfMemory();
			return value;
		}
		const reflectConstruct = Reflect.construct;
		const reflectApply = Reflect.apply;
		const redefine = (target, key, value) =>
			Object.defineProperty(target, key, { value, writable: true, configurable: true });
		// A method whose result is a new buffer is reported once it exists: the check cannot run
		// before the engine allocates, but it stops the loop that would keep allocating.
		const guardMethods = (target, names) => {
			for (const name of names) {
				const native = target[name];
				if (typeof native === "function") {
					redefine(target, name, { [name](...args) { return allocated(reflectApply(native, this, args)); } }[name]);
				}
			}
		};
		// The constructors, which every allocation by length, array, iterable or typed array goes
		// through, and the copies the language makes through a constructor looked up on the
		// instance, which the constructor property of each prototype leads back to.
		const constructors = [
			ArrayBuffer, Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array,
			Uint32Array, Float32Array, Float64Array, BigInt64Array, BigUint64Array,
		];
		if (typeof Float16Array === "function") constructors.push(Float16Array);
		for (const Native of constructors) {
			const guarded = new Proxy(Native, {
				construct: (target, args, newTarget) => allocated(reflectConstruct(target, args, newTarget)),
			});
			redefine(Native.prototype, "constructor", guarded);
			redefine(globalThis, Native.name, guarded);
		}
		// Those copies also resolve their constructor through `SpeciesConstructor`, which falls back
		// to the engine's own intrinsic when the instance's `constructor` is `undefined` (or its
		// species is), and the intrinsic is not the guarded constructor above: a script that
		// deleted or replaced `constructor` on one buffer copied it without limit. So the
		// methods report what they return, whichever constructor made it.
		const TypedArray = Object.getPrototypeOf(Int8Array.prototype);
		guardMethods(TypedArray, ["slice", "map", "filter", "toSorted", "toReversed", "with"]);
		guardMethods(ArrayBuffer.prototype, ["slice", "transfer", "transferToFixedLength"]);
		// Static factories that build the intrinsic directly, in engines that have them.
		guardMethods(Uint8Array, ["fromBase64", "fromHex"]);
		const nativeResize = ArrayBuffer.prototype.resize;
		if (typeof nativeResize === "function") {
			redefine(ArrayBuffer.prototype, "resize", {
				resize(length) {
					const before = this.byteLength;
					reflectApply(nativeResize, this, [length]);
					const grown = this.byteLength - before;
					if (grown > 0) allocated({ byteLength: grown });
				},
			}.resize);
		}
	}

	// What the engine adds to the global object beyond ECMAScript, and the ECMAScript members that
	// reach outside the script:
	//  - `Deno` and `__bootstrap` are the engine's own host handles (every op).
	//  - `WebAssembly` compiles and allocates machine code and linear memory outside the heap limit.
	//  - `SharedArrayBuffer` is shared memory with nothing to share with; `Atomics.wait` and
	//    `Atomics.waitAsync` are a blocking wait and a timer on it.
	//  - `Intl` builds ICU formatters in native memory the heap limit does not see, and QuickJS has
	//    none; `toLocaleString` and `localeCompare` keep working without it.
	//  - `queueMicrotask` is a web API, not ECMAScript: the script has promises.
	for (const name of ["Deno", "__bootstrap", "WebAssembly", "SharedArrayBuffer", "Intl", "queueMicrotask"]) {
		delete globalThis[name];
	}
	delete Atomics.wait;
	delete Atomics.waitAsync;

	// Freeze every object reachable from the built-in globals, plus the intrinsics that are only
	// reachable from instances (iterator and generator prototypes, %TypedArray%), and make the
	// built-in globals read-only (pi `lockdown()`, `prelude-source.ts` @v1.0.4, #10444). The script
	// shares them with this prelude, so a script that patched one (`Array.prototype.toJSON`,
	// `Promise.prototype.then`, `JSON`) could otherwise corrupt what the prelude reports to the host.
	// It runs here, after everything above that redefines or deletes a built-in, and before the
	// script does.
	//
	// Freezing alone breaks ordinary code through the "override mistake": a data property that is
	// read-only on a prototype cannot be assigned on an instance either, so
	// `this.name = "MyError"` in an Error subclass would throw. The commonly overridden properties
	// become accessors whose setter defines an own property on the instance instead.
	(function lockdown() {
		const OVERRIDABLE = new Set(["constructor", "name", "message", "toString", "toLocaleString", "valueOf", "toJSON"]);
		const seen = new Set([globalThis]);
		const queue = [];
		const add = (value) => {
			if ((typeof value === "object" && value !== null) || typeof value === "function") {
				if (!seen.has(value)) {
					seen.add(value);
					queue.push(value);
				}
			}
		};

		// Accessors from an object literal have no own prototype object, unlike function expressions,
		// whose prototype.constructor would be converted again without end.
		function allowOverride(object, key, value, enumerable) {
			const { get, set } = Object.getOwnPropertyDescriptor(
				{
					get accessor() {
						return value;
					},
					set accessor(next) {
						if (this === object) {
							throw new TypeError("Cannot assign to read only property '" + String(key) + "' of a built-in");
						}
						if ((typeof this !== "object" || this === null) && typeof this !== "function") return;
						Object.defineProperty(this, key, { value: next, writable: true, enumerable: true, configurable: true });
					},
				},
				"accessor",
			);
			Object.defineProperty(object, key, { get, set, enumerable, configurable: false });
			add(get);
			add(set);
		}

		for (const key of Reflect.ownKeys(globalThis)) {
			const descriptor = Object.getOwnPropertyDescriptor(globalThis, key);
			add(descriptor.value);
			add(descriptor.get);
			add(descriptor.set);
			if ("value" in descriptor && descriptor.configurable) {
				Object.defineProperty(globalThis, key, { writable: false, configurable: false });
			}
		}
		add(Object.getPrototypeOf(function* () {}));
		add(Object.getPrototypeOf(async function () {}));
		add(Object.getPrototypeOf(async function* () {}));
		add(Object.getPrototypeOf(Int8Array));
		add(Object.getPrototypeOf([][Symbol.iterator]()));
		add(Object.getPrototypeOf(new Map()[Symbol.iterator]()));
		add(Object.getPrototypeOf(new Set()[Symbol.iterator]()));
		add(Object.getPrototypeOf(""[Symbol.iterator]()));
		add(Object.getPrototypeOf(/a/[Symbol.matchAll]("")));
		if (typeof Iterator === "function") {
			if (typeof Iterator.prototype.map === "function") add(Object.getPrototypeOf([].values().map((x) => x)));
			if (typeof Iterator.from === "function") add(Object.getPrototypeOf(Iterator.from({ next() {} })));
		}

		while (queue.length > 0) {
			const object = queue.pop();
			add(Object.getPrototypeOf(object));
			const descriptors = Object.getOwnPropertyDescriptors(object);
			for (const key of Reflect.ownKeys(descriptors)) {
				const descriptor = descriptors[key];
				if ("value" in descriptor) {
					add(descriptor.value);
					if (
						descriptor.writable &&
						descriptor.configurable &&
						(object === Object.prototype || OVERRIDABLE.has(key))
					) {
						allowOverride(object, key, descriptor.value, descriptor.enumerable);
					}
				} else {
					add(descriptor.get);
					add(descriptor.set);
				}
			}
			Object.freeze(object);
		}
	})();

	return {
		settle(id, ok, payload) {
			const entry = pending.get(id);
			if (!entry) return;
			pending.delete(id);
			pendingWeight -= entry.weight;
			if (!ok) {
				rejectCall(entry, new ErrorCtor(payload));
				return;
			}
			let value;
			try {
				value = payload === undefined ? undefined : parse(payload);
			} catch (error) {
				rejectCall(entry, error);
				return;
			}
			entry.resolve(value);
		},
		run(fn) {
			let promise;
			try {
				promise = fn();
			} catch (error) {
				done(false, describeError(error));
				return;
			}
			promiseThen.call(
				promise,
				(value) => {
					let json;
					try {
						json = checkResultSize(serializeResult(value));
					} catch (error) {
						done(false, describeError(error));
						return;
					}
					// Reported once the engine has dispatched what this script left unhandled or handled late:
					// the rejection events are delivered when the event loop runs, not as they happen. The
					// host reports it as it stands if the event loop does not come back (a continuation that
					// never stops), so a script that returned is never held for what it left running.
					if (finished) return;
					held = true;
					opHold(json === undefined ? "" : json, json !== undefined, serializeWrites(), unobservedReport());
				},
				(error) => {
					done(false, describeError(error));
				},
			);
		},
		release() {
			if (!held) return;
			held = false;
			finished = true;
			opRelease(unobservedReport());
		},
		stalled() {
			if (finished || held || pending.size > 0) return false;
			done(
				false,
				stringify({
					name: "Error",
					message:
						"The script is waiting on a promise that can never settle: no tool call is pending, and timers do not exist here.",
				}),
			);
			return true;
		},
	};
})