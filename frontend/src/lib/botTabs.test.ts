// ─────────────────────────────────────────────────────────────────────────────
// botTabs の設定ハブ構成（hubGroups）の単体テスト。
// docs/design/settings-restructure.md の「見る人・Bot ごとの行数」と一致することを確かめる。
// ─────────────────────────────────────────────────────────────────────────────
import { describe, expect, it } from "vitest";
import { type HubContext, hubGroups, SETTINGS_CHILD_TABS } from "./botTabs";

const owned: HubContext = {
	preset: "secretary",
	defaultBot: false,
	systemDefaultBot: false,
	isAdmin: false,
};

function shape(ctx: HubContext) {
	return hubGroups(ctx).map((g) => ({
		category: g.category.id,
		tabs: g.items.map((i) => i.tab),
		unavailable: g.unavailable,
	}));
}

describe("hubGroups", () => {
	it("secretary bot shows 8 rows in 3 categories", () => {
		expect(shape(owned)).toEqual([
			{
				category: "bot",
				tabs: ["profile", "personas", "features", "access"],
				unavailable: false,
			},
			{
				category: "automation",
				tabs: ["playbooks", "delivery", "webhooks"],
				unavailable: false,
			},
			{ category: "discord", tabs: ["discord"], unavailable: false },
		]);
	});

	it("assistant bot keeps the automation category as a folded note", () => {
		const groups = shape({ ...owned, preset: "mcp_assistant" });
		expect(groups).toEqual([
			{
				category: "bot",
				tabs: ["profile", "personas", "features", "access"],
				unavailable: false,
			},
			{ category: "automation", tabs: [], unavailable: true },
			{ category: "discord", tabs: ["discord"], unavailable: false },
		]);
	});

	it("system default bot hides profile and discord for non-admins", () => {
		const ctx = { ...owned, defaultBot: true, systemDefaultBot: true };
		expect(shape(ctx)).toEqual([
			{
				category: "bot",
				tabs: ["personas", "features", "access"],
				unavailable: false,
			},
			{
				category: "automation",
				tabs: ["playbooks", "delivery", "webhooks"],
				unavailable: false,
			},
		]);
		// 管理者には Discord（トークン）が出る。
		expect(shape({ ...ctx, isAdmin: true }).at(-1)).toEqual({
			category: "discord",
			tabs: ["discord"],
			unavailable: false,
		});
	});

	it("per-user default bot hides only profile", () => {
		const tabs = hubGroups({ ...owned, defaultBot: true }).flatMap((g) =>
			g.items.map((i) => i.tab),
		);
		expect(tabs).not.toContain("profile");
		expect(tabs).toContain("discord");
	});

	it("every child tab belongs to the hub", () => {
		expect(SETTINGS_CHILD_TABS).toEqual([
			"profile",
			"personas",
			"features",
			"access",
			"playbooks",
			"delivery",
			"webhooks",
			"discord",
		]);
	});
});
