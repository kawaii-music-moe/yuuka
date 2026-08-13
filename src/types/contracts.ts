import type { FunctionDeclaration } from "@google/generative-ai";
import type { EmbedBuilder } from "discord.js";
import type { IncomingMessage, ServerResponse } from "node:http";

// ─── Function Call 実行コンテキスト ──────────────────────────────────────────

/**
 * Function Call ハンドラに渡される実行コンテキスト。
 * userId（DiscordユーザーID）が全データ分離の必須キーである。
 */
export interface ToolContext {
	/** 実行中のBotインスタンスID（通知クライアントの解決等に使用） */
	botId: string;
	/** DiscordユーザーID（データ分離キー。全リポジトリ呼び出しに必須） */
	userId: string;
	/**
	 * 発話ギルドID（MCPアシスタント等のギルド常駐Botでのみ設定。DM・秘書利用では undefined）。
	 * bot_id × guild_id スコープのFunction（共有ノート・メンバー管理等）が参照する。
	 */
	guildId?: string;
	/** リッチ返信キュー（push すると返信メッセージに添付される） */
	embeds: EmbedBuilder[];
	/** ファイル添付キュー（グラフPNG等） */
	files: { attachment: Buffer; name: string }[];
	/** ユーザー設定: リッチ返信の有効/無効（falseの場合 embeds/files への push 禁止） */
	richReplyEnabled: boolean;
}

/** 各機能モジュールが export する Function Call の束 */
export interface FunctionModule {
	declarations: FunctionDeclaration[];
	handlers: Record<
		string,
		(
			ctx: ToolContext,
			args: Record<string, unknown>,
		) => Promise<string> | string
	>;
}

// ─── HTTPルートモジュール ────────────────────────────────────────────────────

export interface SessionUser {
	discordId: string;
	username: string;
	role: "user" | "admin";
}

export type RouteAuth = "none" | "user" | "admin";

export interface RouteRequestCtx {
	req: IncomingMessage;
	res: ServerResponse;
	url: URL;
	/** auth: "none" の場合は null の可能性あり */
	user: SessionUser | null;
	/** JSONボディ（パース済。無ければ空オブジェクト） */
	body: Record<string, unknown>;
	/** HMAC署名検証等に使う生ボディ */
	rawBody: Buffer;
	/** パスパターン :name の解決値 */
	params: Record<string, string>;
}

export interface RouteDef {
	method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS";
	/** 例: "/api/contacts", "/hook/:token" */
	path: string;
	auth: RouteAuth;
	handler: (ctx: RouteRequestCtx) => Promise<void>;
}

/** JSONレスポンス送信ヘルパー（基本的なセキュリティヘッダー付き） */
export function sendJson(
	res: ServerResponse,
	statusCode: number,
	obj: unknown,
): void {
	const body = JSON.stringify(obj);
	res.writeHead(statusCode, {
		"Content-Type": "application/json; charset=utf-8",
		"Content-Length": Buffer.byteLength(body),
		"X-Content-Type-Options": "nosniff",
		"X-Frame-Options": "SAMEORIGIN",
	});
	res.end(body);
}
