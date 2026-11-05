import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
	describeCurrentVoice,
	describeEngine,
	type EngineStatus,
	loadingPercent,
	usesIrodori,
} from "./engineStatus";

const GiB = 2 ** 30;

describe("usesIrodori", () => {
	it("is true only when the voice is made on this device or the server", () => {
		const statuses: [EngineStatus, boolean][] = [
			[{ kind: "checking" }, false],
			[{ kind: "needs-download", totalBytes: GiB }, false],
			[{ kind: "loading", loaded: 0, total: GiB }, false],
			[{ kind: "ready", adapter: "webgpu" }, true],
			[{ kind: "server", reason: "x" }, true],
			[{ kind: "browser", reason: "x" }, false],
			[{ kind: "none", reason: "x" }, false],
		];
		for (const [status, expected] of statuses)
			assert.equal(usesIrodori(status), expected, status.kind);
	});
});

describe("loadingPercent", () => {
	it("is the progress while loading, capped at 100", () => {
		assert.equal(
			loadingPercent({ kind: "loading", loaded: GiB / 4, total: GiB }),
			25,
		);
		assert.equal(loadingPercent({ kind: "loading", loaded: 5, total: 4 }), 100);
		assert.equal(loadingPercent({ kind: "loading", loaded: 0, total: 0 }), 0);
	});

	it("is 0 when not loading", () => {
		assert.equal(loadingPercent({ kind: "ready", adapter: "webgpu" }), 0);
	});
});

describe("describeEngine", () => {
	it("shows the download size and the progress", () => {
		assert.match(
			describeEngine({ kind: "needs-download", totalBytes: 1.5 * GiB }),
			/約 1\.5 GB/,
		);
		assert.match(
			describeEngine({ kind: "loading", loaded: GiB / 3, total: GiB }),
			/（33%）/,
		);
	});

	it("is empty once the model is ready", () => {
		assert.equal(describeEngine({ kind: "ready", adapter: "webgpu" }), "");
	});

	it("includes the reason for falling back", () => {
		assert.match(
			describeEngine({ kind: "browser", reason: "GPU がありません" }),
			/ブラウザ標準の読み上げを使います（GPU がありません）/,
		);
	});
});

describe("describeCurrentVoice", () => {
	it("names the voice and where it is made", () => {
		assert.equal(
			describeCurrentVoice(
				{ kind: "ready", adapter: "webgpu" },
				"ゆうか",
				false,
			),
			"声: ゆうか",
		);
		assert.equal(
			describeCurrentVoice({ kind: "server", reason: "x" }, "ゆうか", true),
			"声: ゆうか（サーバーで生成）",
		);
	});

	it("tells what reads aloud until the model is ready", () => {
		const loading: EngineStatus = { kind: "loading", loaded: 0, total: GiB };
		assert.equal(
			describeCurrentVoice(loading, "ゆうか", true),
			"声: ゆうか（準備ができるまではサーバーで生成）",
		);
		assert.equal(
			describeCurrentVoice(loading, "ゆうか", false),
			"声: ゆうか（準備ができるまでは標準の読み上げ）",
		);
	});

	it("does not name a voice when Irodori-TTS is not used", () => {
		assert.equal(
			describeCurrentVoice({ kind: "browser", reason: "x" }, "ゆうか", false),
			"ブラウザ標準の読み上げ",
		);
		assert.equal(
			describeCurrentVoice({ kind: "none", reason: "x" }, "ゆうか", false),
			"読み上げなし（返答は画面に表示します）",
		);
	});
});
