import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../client", () => ({
	api: { get: vi.fn(), post: vi.fn(), del: vi.fn() },
}));

import { api } from "../client";
import { botApi } from "./botApi";
import { botAttributeApi } from "./botAttributeApi";

describe("bot endpoint contracts", () => {
	beforeEach(() => vi.clearAllMocks());

	it.each([
		"approved",
		"rejected",
	] as const)("sends the server decision contract for %s through both facades", (decision) => {
		botApi.decideMemberRequest(17, decision);
		botAttributeApi.decideMemberRequest(17, decision);
		expect(api.post).toHaveBeenCalledTimes(2);
		for (const call of vi.mocked(api.post).mock.calls) {
			expect(call).toEqual([
				"/api/bots/member-requests/17/decide",
				{ decision },
				{ scope: "user" },
			]);
		}
	});

	it("keeps the explicit target user and bot scope when inviting or revoking", () => {
		const body = { botId: "shared-bot", targetUserId: "target-user" };
		botAttributeApi.inviteShare(body);
		botApi.revokeShare(body);
		expect(api.post).toHaveBeenNthCalledWith(
			1,
			"/api/bots/shares/invite",
			body,
			{ scope: "user" },
		);
		expect(api.post).toHaveBeenNthCalledWith(
			2,
			"/api/bots/shares/revoke",
			body,
			{ scope: "user" },
		);
	});

	it("defines each shared /api/bots endpoint only in botApi", () => {
		expect(botAttributeApi.presets).toBe(botApi.presets);
		expect(botAttributeApi.usage).toBe(botApi.usage);
		expect(botAttributeApi.shares).toBe(botApi.shares);
		expect(botAttributeApi.inviteShare).toBe(botApi.inviteShare);
		expect(botAttributeApi.revokeShare).toBe(botApi.revokeShare);
		expect(botAttributeApi.memberRequests).toBe(botApi.memberRequests);
		expect(botAttributeApi.decideMemberRequest).toBe(
			botApi.decideMemberRequest,
		);
		expect(botAttributeApi.syncDiscord).toBe(botApi.syncDiscord);
		expect(botAttributeApi.updateBotProfile).toBe(botApi.updateProfile);
		expect(botAttributeApi.botList).toBe(botApi.list);
	});

	it("retains the pending filter and explicit bot ID", () => {
		botAttributeApi.memberRequests("shared-bot");
		expect(api.get).toHaveBeenCalledWith("/api/bots/member-requests", {
			scope: "user",
			query: { botId: "shared-bot", status: "pending" },
		});
	});
});
