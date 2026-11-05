import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it } from "node:test";
import {
	DEFAULT_CAPTION,
	DESIGNED_VOICE,
	loadVoicePreferences,
	saveVoicePreference,
} from "./preferences";

/** Node には localStorage が無いので、テストの間だけ Map で代わりを置く。 */
function installStorage(): Map<string, string> {
	const store = new Map<string, string>();
	const localStorage = {
		getItem: (key: string) => store.get(key) ?? null,
		setItem: (key: string, value: string) => store.set(key, value),
		removeItem: (key: string) => store.delete(key),
	};
	Object.defineProperty(globalThis, "window", {
		value: { localStorage },
		configurable: true,
	});
	return store;
}

describe("voice preferences", () => {
	let store: Map<string, string>;
	beforeEach(() => {
		store = installStorage();
	});
	afterEach(() => {
		Reflect.deleteProperty(globalThis, "window");
	});

	it("uses the defaults when nothing is stored", () => {
		assert.deepEqual(loadVoicePreferences(), {
			caption: DEFAULT_CAPTION,
			voiceSeed: 0,
			handsFree: false,
			selectedVoice: DESIGNED_VOICE,
			speed: 1,
			useServer: false,
		});
	});

	it("reads back what was saved", () => {
		saveVoicePreference("caption", "明るい声");
		saveVoicePreference("voiceSeed", 3);
		saveVoicePreference("handsFree", true);
		saveVoicePreference("selectedVoice", "42");
		saveVoicePreference("speed", 1.1);
		saveVoicePreference("useServer", true);
		assert.deepEqual(loadVoicePreferences(), {
			caption: "明るい声",
			voiceSeed: 3,
			handsFree: true,
			selectedVoice: "42",
			speed: 1.1,
			useServer: true,
		});
	});

	it("keeps the stored format of the earlier version", () => {
		saveVoicePreference("handsFree", false);
		saveVoicePreference("speed", 0.85);
		assert.equal(store.get("agent-desk:voice-hands-free"), "0");
		assert.equal(store.get("agent-desk:voice-speed"), "0.85");
	});

	it("falls back to the defaults for broken numbers", () => {
		store.set("agent-desk:voice-seed", "abc");
		store.set("agent-desk:voice-speed", "0");
		const preferences = loadVoicePreferences();
		assert.equal(preferences.voiceSeed, 0);
		assert.equal(preferences.speed, 1);
	});

	it("works without localStorage", () => {
		Reflect.deleteProperty(globalThis, "window");
		saveVoicePreference("handsFree", true);
		assert.equal(loadVoicePreferences().handsFree, false);
	});
});
