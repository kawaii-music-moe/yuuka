import assert from "node:assert/strict";
import { afterEach, it, mock } from "node:test";
import { selectAgent } from "./agents";
import type { ReceiptDraft, Todo } from "./contracts";
import { httpAgentGateway } from "./httpAdapter";

const originalFetch = globalThis.fetch;
afterEach(() => {
	globalThis.fetch = originalFetch;
	selectAgent("");
});

it("keeps the PWA string IDs, bare view response and completion patch contract", async () => {
	selectAgent("shared-bot");
	const todo: Todo = {
		id: "41",
		title: "task",
		completed: true,
		list: "work",
		dueDate: "2026-10-20",
	};
	const fetch = mock.fn(
		async (_input: string | URL | Request, _init?: RequestInit) =>
			Response.json(todo),
	);
	globalThis.fetch = fetch;
	assert.deepEqual(
		await httpAgentGateway.updateTodo("41", { completed: true }),
		todo,
	);
	const [path, init] = fetch.mock.calls[0].arguments;
	assert.equal(path, "/api/client/todos/41?botId=shared-bot");
	assert.equal(init?.method, "PATCH");
	assert.deepEqual(JSON.parse(String(init?.body)), { completed: true });
});

it("returns a receipt draft without registering a transaction", async () => {
	selectAgent("shared-bot");
	const draft: ReceiptDraft = {
		date: null,
		kind: "expense",
		category: "food",
		description: "",
		amount: null,
	};
	const fetch = mock.fn(
		async (_input: string | URL | Request, _init?: RequestInit) =>
			Response.json(draft),
	);
	globalThis.fetch = fetch;
	const image = { imageBase64: "AAAA", mimeType: "image/png" as const };
	assert.deepEqual(await httpAgentGateway.scanReceipt(image), draft);
	assert.equal(fetch.mock.calls.length, 1);
	assert.equal(
		fetch.mock.calls[0].arguments[0],
		"/api/client/finance/receipt?botId=shared-bot",
	);
});

it("keeps voices per user (no botId), returns raw state bytes and accepts 204 on delete", async () => {
	selectAgent("shared-bot");
	const fetch = mock.fn(
		async (input: string | URL | Request, init?: RequestInit) => {
			if (String(input).endsWith("/state"))
				return new Response(new Float32Array([1, 2]).buffer);
			if (init?.method === "DELETE") return new Response(null, { status: 204 });
			return Response.json([]);
		},
	);
	globalThis.fetch = fetch;
	assert.deepEqual(await httpAgentGateway.listVoices(), []);
	assert.deepEqual(
		[...new Float32Array(await httpAgentGateway.getVoiceState("7"))],
		[1, 2],
	);
	await httpAgentGateway.deleteVoice("7");
	assert.deepEqual(
		fetch.mock.calls.map((call) => String(call.arguments[0])),
		[
			"/api/client/voices",
			"/api/client/voices/7/state",
			"/api/client/voices/7",
		],
	);
});
