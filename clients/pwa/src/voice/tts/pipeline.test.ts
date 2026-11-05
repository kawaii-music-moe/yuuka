// `npm test`（tsx --test）で実行する。
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { decodePlan, findFlatteningPoint, gaussianNoise } from "./pipeline";

describe("decodePlan", () => {
	it("covers every frame once, starting short and doubling", () => {
		for (const frames of [1, 13, 20, 25, 31, 137, 750]) {
			const plan = decodePlan(frames);
			assert.equal(plan[0][0], 0);
			assert.equal(plan.at(-1)?.[1], frames);
			for (let i = 1; i < plan.length; i++)
				assert.equal(plan[i][0], plan[i - 1][1], `frames=${frames}`);
		}
		assert.deepEqual(decodePlan(137), [
			[0, 20],
			[20, 60],
			[60, 137],
		]);
	});

	it("does not leave a tiny last chunk", () => {
		assert.deepEqual(decodePlan(25), [[0, 25]]);
	});
});

describe("findFlatteningPoint", () => {
	it("finds where the latent becomes flat and near zero", () => {
		const dim = 2;
		const frames = 60;
		const latent = new Float32Array(frames * dim);
		for (let i = 0; i < 30 * dim; i++) latent[i] = i % 2 ? 1 : -1;
		assert.equal(findFlatteningPoint(latent, frames, dim), 30);
	});

	it("returns the full length when it never flattens", () => {
		const latent = Float32Array.from({ length: 40 }, (_, i) =>
			i % 2 ? 1 : -1,
		);
		assert.equal(findFlatteningPoint(latent, 20, 2), 20);
	});
});

describe("gaussianNoise", () => {
	it("is deterministic per seed and roughly standard normal", () => {
		const a = gaussianNoise(20000, 7);
		assert.deepEqual(a.slice(0, 5), gaussianNoise(20000, 7).slice(0, 5));
		assert.notDeepEqual(a.slice(0, 5), gaussianNoise(20000, 8).slice(0, 5));
		const mean = a.reduce((s, v) => s + v, 0) / a.length;
		const variance = a.reduce((s, v) => s + (v - mean) ** 2, 0) / a.length;
		assert.ok(
			Math.abs(mean) < 0.05 && Math.abs(variance - 1) < 0.05,
			`mean=${mean} var=${variance}`,
		);
	});
});
