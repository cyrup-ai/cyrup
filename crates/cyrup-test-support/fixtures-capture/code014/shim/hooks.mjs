import { pathToFileURL } from "node:url";
import { resolve as r } from "node:path";
const shim = pathToFileURL(r(import.meta.dirname, "pi-ai.ts")).href;
export async function resolve(specifier, context, next) {
	if (specifier === "@earendil-works/pi-ai") return { url: shim, shortCircuit: true };
	return next(specifier, context);
}
