import { describe, expect, it, vi } from "vitest";
import { asyncState } from "./asyncState";

function deferred<T>() {
	let resolve!: (value: T) => void;
	let reject!: (error: unknown) => void;
	const promise = new Promise<T>((yes, no) => {
		resolve = yes;
		reject = no;
	});
	return { promise, resolve, reject };
}

describe("asyncState", () => {
	it("only applies the latest success even if an older request finishes last", async () => {
		const request = asyncState();
		const older = deferred<string>();
		const newer = deferred<string>();
		const apply = vi.fn();
		const error = vi.fn();
		const first = request.run(() => older.promise, apply, error);
		const second = request.run(() => newer.promise, apply, error);
		newer.resolve("new bot");
		expect(await second).toBe(true);
		older.resolve("old bot");
		expect(await first).toBe(false);
		expect(apply.mock.calls).toEqual([["new bot"]]);
		expect(error).not.toHaveBeenCalled();
	});

	it("stale failures cannot clear current data, toast, or stop loading", async () => {
		const loading = vi.fn();
		const request = asyncState(loading);
		const older = deferred<string>();
		const newer = deferred<string>();
		const apply = vi.fn();
		const error = vi.fn();
		const first = request.run(() => older.promise, apply, error);
		const second = request.run(() => newer.promise, apply, error);
		older.reject(new Error("old request failed"));
		expect(await first).toBe(false);
		expect(error).not.toHaveBeenCalled();
		expect(loading.mock.calls).toEqual([[true], [true]]);
		newer.reject(new Error("current request failed"));
		expect(await second).toBe(true);
		expect(error).toHaveBeenCalledTimes(1);
		expect(loading.mock.calls).toEqual([[true], [true], [false]]);
	});

	it("cancel discards pending results and permits a new request", async () => {
		const request = asyncState();
		const older = deferred<string>();
		const apply = vi.fn();
		const error = vi.fn();
		const first = request.run(() => older.promise, apply, error);
		request.cancel();
		older.resolve("unmounted component");
		expect(await first).toBe(false);
		expect(apply).not.toHaveBeenCalled();
		expect(await request.run(async () => "new request", apply, error)).toBe(
			true,
		);
		expect(apply.mock.calls).toEqual([["new request"]]);
	});
});
