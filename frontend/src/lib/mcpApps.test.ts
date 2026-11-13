import { describe, expect, it } from "vitest";
import {
	clampFrameHeight,
	errorToolResult,
	isOpenableLink,
	needsArguments,
	parseArguments,
	sandboxAttribute,
} from "./mcpApps";

describe("mcpApps", () => {
	it("allow-same-origin は別オリジンのときだけ付ける", () => {
		expect(sandboxAttribute(false)).not.toContain("allow-same-origin");
		expect(sandboxAttribute(true)).toContain("allow-same-origin");
		expect(sandboxAttribute(false)).toContain("allow-scripts");
	});

	it("必須の引数があるか", () => {
		expect(needsArguments(null)).toBe(false);
		expect(needsArguments({ type: "object" })).toBe(false);
		expect(needsArguments({ type: "object", required: [] })).toBe(false);
		expect(needsArguments({ type: "object", required: ["q"] })).toBe(true);
	});

	it("引数は JSON オブジェクトだけ受け付ける", () => {
		expect(parseArguments("")).toEqual({});
		expect(parseArguments('{"q":"x"}')).toEqual({ q: "x" });
		expect(() => parseArguments("[1]")).toThrow();
		expect(() => parseArguments("not json")).toThrow();
	});

	it("高さを表示範囲に収める", () => {
		expect(clampFrameHeight(50, 1000)).toBe(120);
		expect(clampFrameHeight(300.2, 1000)).toBe(301);
		expect(clampFrameHeight(5000, 1000)).toBe(800);
		expect(clampFrameHeight(Number.NaN, 1000)).toBe(480);
	});

	it("http(s) のリンクだけ開く", () => {
		expect(isOpenableLink("https://example.com/a")).toBe(true);
		expect(isOpenableLink("javascript:alert(1)")).toBe(false);
		expect(isOpenableLink("not a url")).toBe(false);
	});

	it("エラーを CallToolResult の形にする", () => {
		expect(errorToolResult("x")).toEqual({
			content: [{ type: "text", text: "x" }],
			isError: true,
		});
	});
});
