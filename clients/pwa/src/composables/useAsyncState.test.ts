import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { useAsyncState } from "./useAsyncState";

/** 外から解決できる Promise。 */
function deferred<T>() {
	let resolve!: (value: T) => void;
	let reject!: (reason: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

describe("useAsyncState", () => {
	it("starts loading and returns the result", async () => {
		const { loading, error, run } = useAsyncState();
		assert.equal(loading.value, true);
		assert.deepEqual(await run(async () => [1, 2], "失敗"), [1, 2]);
		assert.equal(loading.value, false);
		assert.equal(error.value, "");
	});

	it("sets the message when the task fails", async () => {
		const { loading, error, run } = useAsyncState(false);
		assert.equal(loading.value, false);
		const result = await run(async () => {
			throw new Error("boom");
		}, "取得できません。");
		assert.equal(result, undefined);
		assert.equal(loading.value, false);
		assert.equal(error.value, "取得できません。");
	});

	it("clears the previous error when it runs again", async () => {
		const { error, run } = useAsyncState();
		await run(() => Promise.reject(new Error("boom")), "取得できません。");
		await run(async () => "ok", "取得できません。");
		assert.equal(error.value, "");
	});

	it("ignores a run that was overtaken by a later one", async () => {
		const { loading, error, run } = useAsyncState();
		const first = deferred<string>();
		const second = deferred<string>();
		const firstRun = run(() => first.promise, "失敗");
		const secondRun = run(() => second.promise, "失敗");

		second.resolve("new");
		assert.equal(await secondRun, "new");
		assert.equal(loading.value, false);

		// 古い読み込みは、後から成功しても失敗しても表示を変えない。
		first.reject(new Error("late"));
		assert.equal(await firstRun, undefined);
		assert.equal(error.value, "");
		assert.equal(loading.value, false);
	});

	it("keeps loading while the latest run is still pending", async () => {
		const { loading, run } = useAsyncState();
		const first = deferred<string>();
		const second = deferred<string>();
		const firstRun = run(() => first.promise, "失敗");
		void run(() => second.promise, "失敗");

		first.resolve("old");
		assert.equal(await firstRun, undefined);
		assert.equal(loading.value, true);
		second.resolve("new");
	});
});
