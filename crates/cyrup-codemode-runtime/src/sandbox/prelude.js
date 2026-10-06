(function (toolsJson, globalsJson, storeJson, memoryStride) {
	"use strict";
	// The only host entry points. They are captured here and `Deno` is deleted before the script
	// runs, so the script cannot reach them (see the end of this function).
	const hostOps = Deno.core.ops;
	const opCall = hostOps.op_codemode_call;
	const opOutput = hostOps.op_codemode_output;
	const opDoneOk = hostOps.op_codemode_done_ok;
	const opDoneErr = hostOps.op_codemode_done_err;
	const opMemoryExceeded = hostOps.op_codemode_memory_exceeded;

	function bridge(kind, a, b, c) {
		switch (kind) {
			case "call":
			case "global":
				opCall(a, kind, b, c === undefined ? "" : c, c !== undefined);
				break;
			case "output":
				opOutput(a, b, c === undefined ? "" : c);
				break;
			case "done":
				if (a) opDoneOk(b === undefined ? "" : b, b !== undefined, c);
				else opDoneErr(b);
				break;
		}
	}
	const stringify = JSON.stringify;
	const parse = JSON.parse;
	const promiseThen = Promise.prototype.then;
	const ErrorCtor = Error;
	const TypeErrorCtor = TypeError;
	const RangeErrorCtor = RangeError;
	const pending = new Map();
	let nextId = 1;
	let finished = false;
	// Thrown by exit() to unwind the script after it already reported success.
	const EXIT = Object.freeze({});

	function done(ok, payload, writes) {
		if (finished) return;
		finished = true;
		bridge("done", ok, payload, writes);
	}

	function serialize(value) {
		return value === undefined ? undefined : stringify(value);
	}

	// V8 stacks already start with "Name: message" followed by the frames. The text is rebuilt from
	// the error's current name and message and the frame lines alone, so it reads the same whether
	// or not the stack was captured before the error was renamed, and this prelude's frames are
	// dropped.
	function errorText(error) {
		const head = error.message ? error.name + ": " + error.message : String(error.name);
		const stack = typeof error.stack === "string" ? error.stack : "";
		const frames = stack
			.split("\n")
			.filter((line) => /^\s+at /.test(line) && !line.includes("codemode-prelude.js"));
		return [head, ...frames].join("\n");
	}

	function format(value) {
		if (typeof value === "string") return value;
		if (value instanceof ErrorCtor) return errorText(value);
		try {
			const json = stringify(value);
			return json === undefined ? String(value) : json;
		} catch {
			return String(value);
		}
	}

	// Throws nothing: a hostile thrown value (a revoked proxy, a throwing `stack` getter) must not
	// stop the script's failure from being reported.
	function describeError(error) {
		try {
			if (error instanceof ErrorCtor) {
				return stringify({ name: error.name, message: error.message, stack: errorText(error) });
			}
			return stringify({ message: format(error) });
		} catch {
			return stringify({ message: "The script threw a value that could not be described" });
		}
	}

	function caller(kind, name, spread) {
		return (...args) =>
			new Promise((resolve, reject) => {
				let json;
				try {
					json = serialize(spread ? args : args[0]);
				} catch (error) {
					reject(error);
					return;
				}
				const id = nextId++;
				pending.set(id, { resolve, reject });
				bridge(kind, id, name, json);
			});
	}

	const tools = Object.create(null);
	const allTools = [];
	for (const { name, jsName, description } of parse(toolsJson)) {
		const fn = caller("call", name);
		// The first tool wins when two names normalize to the same identifier.
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

	function checkKey(name, key) {
		if (typeof key !== "string") throw new TypeError(name + "() key must be a string");
	}

	function store(key, value) {
		checkKey("store", key);
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
		checkKey("load", key);
		const json = stored.get(key);
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
		if (finished) return;
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

	// Primitives become their string form, everything else JSON.
	function outputText(value) {
		if (value === undefined || value === null || typeof value !== "object" && typeof value !== "function") {
			return String(value);
		}
		const json = stringify(value);
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
		if (data.length % 4 !== 0 || !/^[A-Za-z0-9+/]+={0,2}$/.test(data)) {
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
	for (const level of ["log", "info", "warn", "error", "debug"]) {
		console[level] = (...args) => {
			output("text", args.map(format).join(" "));
		};
	}
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
		// The constructors, which every allocation by length, array, iterable or typed array goes
		// through, and the copies the language makes through a constructor looked up on the
		// instance (`slice`, `map`, `filter`), which the constructor property of each prototype
		// leads back to.
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
		// The copies the language makes without a constructor.
		const TypedArray = Object.getPrototypeOf(Int8Array.prototype);
		for (const name of ["toSorted", "toReversed", "with"]) {
			const native = TypedArray[name];
			if (typeof native === "function") {
				redefine(TypedArray, name, { [name](...args) { return allocated(reflectApply(native, this, args)); } }[name]);
			}
		}
		for (const name of ["transfer", "transferToFixedLength"]) {
			const native = ArrayBuffer.prototype[name];
			if (typeof native === "function") {
				redefine(ArrayBuffer.prototype, name, { [name](...args) { return allocated(reflectApply(native, this, args)); } }[name]);
			}
		}
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

	return {
		settle(id, ok, payload) {
			const entry = pending.get(id);
			if (!entry) return;
			pending.delete(id);
			if (!ok) {
				entry.reject(new ErrorCtor(payload));
				return;
			}
			let value;
			try {
				value = payload === undefined ? undefined : parse(payload);
			} catch (error) {
				entry.reject(error);
				return;
			}
			entry.resolve(value);
		},
		run(fn) {
			let promise;
			try {
				promise = fn(toolsProxy, console);
			} catch (error) {
				done(false, describeError(error));
				return;
			}
			promiseThen.call(
				promise,
				(value) => {
					let json;
					try {
						json = serialize(value);
					} catch (error) {
						done(false, describeError(error));
						return;
					}
					done(true, json, serializeWrites());
				},
				(error) => {
					done(false, describeError(error));
				},
			);
		},
		stalled() {
			if (finished || pending.size > 0) return false;
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