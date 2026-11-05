import { describe, expect, it } from "vitest";
import type { IntegratedGoogleAccountView } from "./api/types";
import {
	googleAssignOptions,
	googleAssignValue,
	googleGrantBody,
} from "./googleAssign";

const account = (
	id: number,
	email: string | null,
): IntegratedGoogleAccountView => ({
	id,
	email,
	is_primary: false,
	calendars: [],
});

describe("googleAssignOptions", () => {
	it("primary・連携なしの後にアカウントを並べ、メール未取得は番号で出す", () => {
		expect(
			googleAssignOptions([account(3, "a@example.com"), account(7, null)]),
		).toEqual([
			{ value: "primary", label: "（primaryを使用）" },
			{ value: "none", label: "連携なし" },
			{ value: "acct:3", label: "a@example.com" },
			{ value: "acct:7", label: "アカウント#7" },
		]);
	});
});

describe("googleAssignValue", () => {
	it("primary / none はそのまま、アカウント ID は acct: を付ける", () => {
		expect(googleAssignValue({ google_setting: "primary" })).toBe("primary");
		expect(googleAssignValue({ google_setting: "none" })).toBe("none");
		expect(googleAssignValue({ google_setting: 12 })).toBe("acct:12");
	});
});

describe("googleGrantBody", () => {
	it("選択肢の値を grantGoogle の body に戻す", () => {
		expect(googleGrantBody("bot_1", "primary")).toEqual({
			botId: "bot_1",
			mode: "primary",
		});
		expect(googleGrantBody("bot_1", "none")).toEqual({
			botId: "bot_1",
			mode: "none",
		});
		expect(googleGrantBody("bot_1", "acct:12")).toEqual({
			botId: "bot_1",
			mode: "account",
			accountId: 12,
		});
	});

	it("googleAssignValue と往復できる", () => {
		for (const gs of ["primary", "none", 5] as const) {
			const body = googleGrantBody(
				"b",
				googleAssignValue({ google_setting: gs }),
			);
			expect(body.mode === "account" ? body.accountId : body.mode).toBe(gs);
		}
	});
});
