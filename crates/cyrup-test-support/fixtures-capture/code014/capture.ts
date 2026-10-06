import { runAgentLoop } from "./packages/agent/src/agent-loop.ts";
import { EventStream, getCurrentSystemMessage } from "@earendil-works/pi-ai";
import {
	buildSystemPromptSections,
	diffSystemPromptSections,
} from "./packages/coding-agent/src/core/system-prompt.ts";

// Frozen so the capture is deterministic: the loop stamps a row it creates itself with `Date.now()`.
Date.now = () => 1_754_611_203_000;

const out: Record<string, unknown> = {};

// ---- 1. the sections pi builds for one fixed set of options -------------------------------------
const options = {
	selectedTools: ["read", "bash"],
	toolSnippets: { read: "Read file contents", bash: "Execute bash commands" },
	toolGuidelines: { read: ["Use read to examine files instead of cat or sed."] },
	promptGuidelines: ["Keep answers short"],
	appendSystemPrompt: "APPENDED TEXT",
	cwd: "/work/proj",
	contextFiles: [
		{ path: "/work/proj/AGENTS.md", content: "house rules" },
		{ path: "/work/AGENTS.md", content: "more rules" },
	],
	skills: [],
};
out.sections = buildSystemPromptSections(options as never);
out.sectionsCustom = buildSystemPromptSections({ ...options, customPrompt: "MY OWN PROMPT" } as never);
out.sectionsNone = buildSystemPromptSections({ selectedTools: [], cwd: "C:\\win\\dir" } as never);

// ---- 2. diffSystemPromptSections over the pairs the port is tested on -----------------------------
const prev = { preamble: "p", tools: "t1", gone: "g", cwd: "c" };
const cur = { preamble: "p", tools: "t2", cwd: "c", fresh: "f" };
out.diffs = {
	changedNewRemoved: diffSystemPromptSections(prev, cur) ?? null,
	identical: diffSystemPromptSections(cur, cur) ?? null,
	bothEmpty: diffSystemPromptSections({}, {}) ?? null,
	fromEmpty: diffSystemPromptSections({}, cur) ?? null,
	unknownFromPi: diffSystemPromptSections(
		{ preamble: "pi", experimental_pi_only: "<x>\nkept by pi\n</x>", cwd: "<cwd>\n/w\n</cwd>" },
		{ preamble: "cyrup", cwd: "<cwd>\n/w\n</cwd>" },
	) ?? null,
};

// ---- 3. the system rows pi's LOOP writes -----------------------------------------------------------
const tool = (name: string, description: string) => ({
	name,
	description,
	parameters: { type: "object", properties: {} },
	label: name,
	execute: async () => ({ content: [], details: {} }),
});
const fakeStream = () => {
	const stream = new EventStream<any, any>(
		(e) => e.type === "done" || e.type === "error",
		(e) => e.message,
	);
	const message = {
		role: "assistant",
		content: [{ type: "text", text: "ok" }],
		api: "faux",
		provider: "faux",
		model: "faux-1",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
		stopReason: "stop",
		timestamp: 0,
	};
	queueMicrotask(() => stream.push({ type: "done", reason: "stop", message }));
	return stream;
};
const config = {
	model: { id: "faux-1", provider: "faux", api: "faux" },
	convertToLlm: (messages: any[]) => messages,
} as any;
const loopRows = async (label: string, prompts: any[], context: any) => {
	const rows: string[] = [];
	await runAgentLoop(
		prompts,
		context,
		config,
		async (event: any) => {
			if (event.type === "message_end" && event.message.role === "system") {
				rows.push(JSON.stringify(event.message));
			}
		},
		undefined,
		fakeStream as never,
	);
	out[label] = rows;
};
const userPrompt = { role: "user", content: [{ type: "text", text: "hi" }], timestamp: 1 };
const promptUpdate = {
	role: "system",
	content: "",
	sections: { preamble: "You are pi.", cwd: "<cwd>\n/pi/work\n</cwd>" },
	timestamp: 1700000000000,
};
// (a) a pending prompt update meets the loop's tool declaration: the tool keys are appended AFTER `timestamp`
await loopRows("loopPromptAndTools", [promptUpdate, userPrompt], {
	messages: [],
	tools: [tool("read", "Read a file"), tool("bash", "Run a command")],
});
// (b) no pending system message: the loop writes its own, and it carries `timestamp` before `toolsAdded`
await loopRows("loopToolsOnly", [userPrompt], { messages: [], tools: [tool("read", "Read a file")] });
// (c) a later turn: a tool is swapped, so the row has `toolsAdded` AND `toolsRemoved`
await loopRows(
	"loopSwap",
	[userPrompt],
	{
		messages: [
			{ role: "system", content: "", timestamp: 5, toolsAdded: [{ name: "old", description: "d", parameters: { type: "object", properties: {} } }] },
			userPrompt,
		],
		tools: [tool("new", "n")],
	},
);

// ---- 4. the replayed snapshot a compaction entry carries ---------------------------------------------
const replayed = getCurrentSystemMessage([
	{ role: "system", content: "", sections: { preamble: "You are pi.", cwd: "<cwd>\n/pi/work\n</cwd>" }, timestamp: 10, toolsAdded: [{ name: "read", description: "Read a file", parameters: { type: "object", properties: {} } }] },
	{ role: "system", content: "", sections: { cwd: null }, timestamp: 20 },
] as never);
out.compactionSnapshot = JSON.stringify({ ...replayed, timestamp: 1754611204000 });

console.log(JSON.stringify(out, null, 1));
