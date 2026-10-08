// ─────────────────────────────────────────────────────────────────────────────
// activeBot ストアのシステム Bot 除外の単体テスト（#140）。
//
// 初期値は import 時に localStorage から読むため、各テストで window/localStorage を
// `vi.stubGlobal` で差し替え、`vi.resetModules()` 後に activeBot.ts を import し直す。
// ─────────────────────────────────────────────────────────────────────────────

import { get } from "svelte/store";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SYSTEM_BOT_ID, selectableBots } from "./activeBot";

function stubStorage(entries: Record<string, string>): Map<string, string> {
	const store = new Map(Object.entries(entries));
	vi.stubGlobal("window", {});
	vi.stubGlobal("localStorage", {
		getItem: (k: string) => store.get(k) ?? null,
		setItem: (k: string, v: string) => store.set(k, v),
		removeItem: (k: string) => store.delete(k),
	});
	return store;
}

async function loadActiveBot() {
	vi.resetModules();
	return (await import("./activeBot")).activeBot;
}

afterEach(() => {
	vi.unstubAllGlobals();
});

describe("selectableBots", () => {
	it("drops the system bot and keeps the rest in order", () => {
		const bots = [
			{ id: SYSTEM_BOT_ID, name: "システムデフォルト" },
			{ id: "bot_a", name: "A" },
			{ id: "bot_b", name: "B" },
		];
		expect(selectableBots(bots).map((b) => b.id)).toEqual(["bot_a", "bot_b"]);
	});
});

describe("activeBot initial value", () => {
	const mine = { id: "bot_a", name: "A", avatar: "", preset: "secretary" };

	it("restores a previously selected bot", async () => {
		stubStorage({ currentBot: JSON.stringify(mine) });
		expect(get(await loadActiveBot())).toEqual(mine);
	});

	it("discards a stored system bot selection", async () => {
		const store = stubStorage({
			currentBot: JSON.stringify({ ...mine, id: SYSTEM_BOT_ID }),
		});
		expect(get(await loadActiveBot())).toBeNull();
		expect(store.has("currentBot")).toBe(false);
	});

	it("discards a system bot left in the legacy keys", async () => {
		stubStorage({ currentBotId: SYSTEM_BOT_ID, currentBotName: "x" });
		expect(get(await loadActiveBot())).toBeNull();
	});
});
