// ─────────────────────────────────────────────────────────────────────────────
// cron 式ビルダー（cronBuilder.ts）の単体テスト。
// 組み立て・分解の往復と、説明文がバックエンド（croner の 5 フィールド）の解釈と合うことを確かめる。
// ─────────────────────────────────────────────────────────────────────────────
import { describe, expect, it } from "vitest";
import {
	buildCron,
	type CronState,
	defaultState,
	describeCron,
	fieldExpr,
	hasDomAndDow,
	parseBounded,
	parseCron,
} from "./cronBuilder";

function parsed(expr: string): CronState {
	const s = parseCron(expr);
	if (!s) throw new Error(`parse failed: ${expr}`);
	return s;
}

describe("buildCron", () => {
	it("defaults to every hour at minute 0", () => {
		expect(buildCron(defaultState())).toBe("0 * * * *");
	});

	it("joins intervals, ranges and values in that order, sorted", () => {
		const s = defaultState();
		s[0] = {
			mode: "custom",
			values: [45, 5],
			intervals: [20],
			ranges: [{ start: 10, end: 30, step: 5 }],
		};
		expect(fieldExpr(s[0])).toBe("*/20,10-30/5,5,45");
	});

	it("falls back to * when a custom field has nothing added", () => {
		const s = defaultState();
		s[1] = { mode: "custom", values: [], intervals: [], ranges: [] };
		expect(buildCron(s)).toBe("0 * * * *");
	});
});

describe("parseCron", () => {
	it.each([
		"0 9 * * *",
		"*/30 * * * *",
		"0 9 * * 1",
		"0 9 1 * *",
		"30 8,18 * * 1-5",
		"0 0 1-15/2 */3 *",
		"* * * * *",
	])("round-trips %s", (expr) => {
		expect(buildCron(parsed(expr))).toBe(expr);
	});

	it("picks the input mode from what the field contains", () => {
		const s = parsed("0 */2 1-10 3 *");
		expect(s.map((f) => f.mode)).toEqual([
			"custom",
			"interval",
			"range",
			"custom",
			"every",
		]);
	});

	it("normalizes Sunday 7 to 0", () => {
		expect(buildCron(parsed("0 9 * * 7"))).toBe("0 9 * * 0");
	});

	it.each([
		"0 9 * *",
		"0 0 9 * * *",
		"0 9 * * MON",
		"0 9 L * *",
		"60 9 * * *",
		"0 24 * * *",
		"0 9 0 * *",
		"0 9 * 13 *",
		"0 9 * * 1#2",
		"0 9 10-5 * *",
		"",
	])("returns null for %j", (expr) => {
		expect(parseCron(expr)).toBeNull();
	});
});

describe("describeCron", () => {
	it.each([
		["0 * * * *", "毎時 0分 に実行"],
		["* * * * *", "毎分 に実行"],
		["*/30 * * * *", "30分ごと に実行"],
		["0 9 * * *", "毎日 9:00 に実行"],
		["0,30 9,18 * * *", "毎日 9:00・9:30・18:00・18:30 に実行"],
		["0 9 * * 1", "毎週 月曜日 9:00 に実行"],
		["0 9 1 * *", "毎月 1日 9:00 に実行"],
		["0 9 * 1 *", "1月の毎日 9:00 に実行"],
		["*/15 9-17 * * 1-5", "毎週 月曜〜金曜 9〜17時 15分ごと に実行"],
		["0 */2 * * *", "2時間ごと 0分 に実行"],
		["0 9 1 * 1", "毎月 1日 または 毎週 月曜日 9:00 に実行"],
	])("%s → %s", (expr, text) => {
		expect(describeCron(parsed(expr))).toBe(text);
	});
});

describe("parseBounded", () => {
	it.each([
		["0", 0],
		["59", 59],
		[" 7 ", 7],
		["０９", 9],
	])("reads %j", (raw, v) => {
		expect(parseBounded(raw, 0, 59)).toBe(v);
	});

	it.each(["", "60", "-1", "1.5", "abc", "100"])("rejects %j", (raw) => {
		expect(parseBounded(raw, 0, 59)).toBeNull();
	});

	it("applies the lower bound", () => {
		expect(parseBounded("1", 2, 23)).toBeNull();
	});
});

describe("hasDomAndDow", () => {
	it("is true only when both day of month and weekday are set", () => {
		expect(hasDomAndDow(parsed("0 9 1 * 1"))).toBe(true);
		expect(hasDomAndDow(parsed("0 9 1 * *"))).toBe(false);
		expect(hasDomAndDow(parsed("0 9 * * 1"))).toBe(false);
	});
});
