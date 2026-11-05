// `npm test`（tsx --test）で実行する。
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { IrodoriClient } from "./client";
import type { WorkerRequest, WorkerResponse } from "./protocol";

/** 送られた要求を記録し、応答を手で流せる偽の Worker。 */
function fakeWorker() {
	const sent: WorkerRequest[] = [];
	const worker = {
		onmessage: null as ((event: MessageEvent<WorkerResponse>) => void) | null,
		postMessage: (m: WorkerRequest) => sent.push(m),
	};
	const reply = (data: WorkerResponse) =>
		worker.onmessage?.({ data } as MessageEvent<WorkerResponse>);
	return { worker: worker as unknown as Worker, sent, reply };
}

describe("IrodoriClient", () => {
	it("answers every pending load (the page can be opened again while loading)", async () => {
		const { worker, sent, reply } = fakeWorker();
		const client = new IrodoriClient(worker);
		const progress: number[] = [];
		const first = client.load((loaded) => progress.push(loaded));
		const second = client.load(() => undefined);
		assert.deepEqual(
			sent.map((m) => m.type),
			["load", "load"],
		);
		reply({ type: "progress", loaded: 5, total: 10 });
		reply({ type: "ready", adapter: "intel" });
		reply({ type: "ready", adapter: "intel" });
		assert.deepEqual(await Promise.all([first, second]), ["intel", "intel"]);
		assert.deepEqual(progress, [5]);
		// 読み込み済みでも、次の load は Worker の ready で解決する。
		const third = client.load(() => undefined);
		reply({ type: "ready", adapter: "intel" });
		assert.equal(await third, "intel");
	});

	it("rejects every pending load on a load error", async () => {
		const { worker, reply } = fakeWorker();
		const client = new IrodoriClient(worker);
		const loads = [client.load(() => undefined), client.load(() => undefined)];
		reply({ type: "error", message: "GPU が足りません" });
		for (const load of loads) await assert.rejects(load, /GPU が足りません/);
	});

	it("returns probe results in request order", async () => {
		const { worker, reply } = fakeWorker();
		const client = new IrodoriClient(worker);
		const a = client.probe();
		const b = client.probe();
		reply({ type: "probe", supported: true, stored: false, totalBytes: 1 });
		reply({ type: "probe", supported: true, stored: true, totalBytes: 1 });
		assert.equal((await a).stored, false);
		assert.equal((await b).stored, true);
	});
});
