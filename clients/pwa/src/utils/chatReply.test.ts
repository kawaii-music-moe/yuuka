import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { ChatReplyTimeoutError } from "../api/chatPolling";
import { ApiError } from "../api/http";
import { describeChatError } from "./chatReply";

const timeout = "待ちきれませんでした";

describe("describeChatError", () => {
	it("shows the server message for synchronous rejections", () => {
		for (const status of [400, 409, 429])
			assert.equal(
				describeChatError(new ApiError("x", status, "処理中です"), { timeout }),
				"処理中です",
			);
	});

	it("does not show the server message for other statuses", () => {
		assert.equal(
			describeChatError(new ApiError("x", 500, "内部エラー"), { timeout }),
			"メッセージを送信できませんでした。",
		);
	});

	it("uses the page's message when the reply did not arrive in time", () => {
		assert.equal(
			describeChatError(new ChatReplyTimeoutError(240_000), { timeout }),
			timeout,
		);
	});

	it("asks to sign in again when the session expired", () => {
		assert.equal(
			describeChatError(new ApiError("x", 401), { timeout }),
			"ログインの有効期限が切れました。ログインし直してください。",
		);
	});

	it("shows the error message only when detail is requested", () => {
		const cause = new Error("マイクが見つかりません。");
		assert.equal(
			describeChatError(cause, { timeout }),
			"メッセージを送信できませんでした。",
		);
		assert.equal(
			describeChatError(cause, { timeout, detail: true }),
			"マイクが見つかりません。",
		);
		assert.equal(
			describeChatError("unknown", { timeout, detail: true }),
			"メッセージを送信できませんでした。",
		);
	});
});
