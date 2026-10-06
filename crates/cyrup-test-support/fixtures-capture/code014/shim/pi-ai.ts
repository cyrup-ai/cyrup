// A shim for `@earendil-works/pi-ai`: the three real modules the loop and the prompt builder use,
// re-exported from their v1.0.0 sources, and one stub for the validator (never reached: no tool calls).
export * from "../packages/ai/src/utils/transcript.ts";
export * from "../packages/ai/src/utils/text.ts";
export { EventStream } from "../packages/ai/src/utils/event-stream.ts";
export function validateToolArguments(): never {
	throw new Error("not reached");
}
