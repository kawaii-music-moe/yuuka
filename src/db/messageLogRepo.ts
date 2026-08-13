import { config } from "../config.js";
import { getDb } from "./database.js";
import { getRedisClient } from "./redis.js";
import { getSystemSetting, setSystemSetting } from "./systemSettingsRepo.js";

// ─── 会話履歴の永続化（§7）+ コンテキストキャッシュ（§3.1.4）+ 全文検索（§3.12） ─
//
// 正の履歴は SQLite (message_logs) が保持し、Redis (context:{userId}) は
// LLM へ受け渡す直近15件の高速キャッシュとして二重書き込みする。
// Redis キャッシュ消失時は SQLite から直近15件を再構築する。

// ─── 型定義 ──────────────────────────────────────────────────────────────────

export interface MessageLogRecord {
	id: number;
	user_id: string;
	bot_id: string;
	discord_msg_id: string | null;
	role: "user" | "assistant";
	content: string;
	reply_to_msg_id: string | null;
	/** 発話ギルドID（NULL = DM・秘書利用。bot_attributes_requirements.md §4.6.1） */
	guild_id: string | null;
	created_at: string;
	source: "discord" | "pwa";
}

/** LLMへ受け渡す会話コンテキストの1エントリ */
export interface ContextEntry {
	role: "user" | "assistant";
	content: string;
}

/** PWA history is stored separately from Discord history. */
export function listPwaMessages(userId: string, botId: string, limit: number = 100): MessageLogRecord[] {
	const db = getDb();
	return db.prepare(`SELECT * FROM message_logs WHERE user_id = ? AND bot_id = ? AND source = 'pwa' ORDER BY id DESC LIMIT ?`)
		.all(userId, botId, Math.min(Math.max(limit, 1), 200)).reverse() as MessageLogRecord[];
}

/** searchMessages の検索条件 */
export interface MessageSearchOptions {
	/** 全文検索キーワード（省略時は期間のみで検索） */
	keyword?: string;
	/** 検索開始日時（ISO日付 'YYYY-MM-DD' または 'YYYY-MM-DD HH:MM:SS'） */
	from?: string;
	/** 検索終了日時（日付のみ指定時はその日の終わりまで含む） */
	to?: string;
	/** 最大取得件数（デフォルト10件） */
	limit?: number;
}

// ─── 定数 ────────────────────────────────────────────────────────────────────

/** Redisコンテキストキャッシュの保持件数（§3.1.4: 直近15件） */
const CONTEXT_LIMIT = 15;

/**
 * 汎用モードのギルドコンテキスト保持件数
 * （bot_attributes_requirements.md §4.6.1: 複数人の発話が混ざるため秘書の15件から30件へ拡張）
 */
export const GUILD_CONTEXT_LIMIT = 30;

/** Redisコンテキストキャッシュの TTL（§3.1.4: セッション有効期限7日に連動） */
const CONTEXT_TTL_SECONDS = config.sessionTtlDays * 24 * 60 * 60;

/**
 * Redisコンテキストキャッシュのキー。
 * 秘書業務データのBot別分離に伴い、Bot単位で会話コンテキストを分離する
 * （context:{botId}:secretary:{discord_user_id}）。system_default も明示的に含める。
 */
function contextKey(userId: string, botId: string): string {
	return `context:${botId}:secretary:${userId}`;
}

/** 汎用モードのギルドコンテキストキー（要件 §4.6.1: context:{botId}:{guildId}） */
function guildContextKey(botId: string, guildId: string): string {
	return `context:${botId}:${guildId}`;
}

/** 汎用モードの owner DM コンテキストキー（要件 §4.6.1: context:{botId}:dm:{ownerId}） */
function botDmContextKey(botId: string, ownerId: string): string {
	return `context:${botId}:dm:${ownerId}`;
}

/**
 * コンテキストリセット境界の system_settings キー。
 * clearContext 実行時点の最大 message_logs.id を記録し、
 * Redisキャッシュ再構築時にそれ以前のメッセージを復元しないようにする
 * （SQLiteの永続ログ自体は §7.1 に従い削除しない）。
 */
function contextFloorKey(userId: string, botId: string): string {
	return `context_floor:${botId}:${userId}`;
}

/**
 * 汎用モード owner DM のリセット境界キー。
 * 秘書コンテキスト（context_floor:{botId}:{userId}）とは別軸で管理し、
 * getBotDmContext の SQLite 再構築でも過去メッセージを復元しないようにする。
 */
function botDmContextFloorKey(botId: string, ownerId: string): string {
	return `context_floor:${botId}:dm:${ownerId}`;
}

/** コンテキストリセット境界（これより小さい id はコンテキスト再構築に使わない） */
function getContextFloor(userId: string, botId: string): number {
	const value = getSystemSetting(contextFloorKey(userId, botId), "0");
	const parsed = parseInt(value, 10);
	return Number.isFinite(parsed) ? parsed : 0;
}

/** 汎用モード owner DM のリセット境界（これより小さい id は再構築に使わない） */
function getBotDmContextFloor(ownerId: string, botId: string): number {
	const value = getSystemSetting(botDmContextFloorKey(botId, ownerId), "0");
	const parsed = parseInt(value, 10);
	return Number.isFinite(parsed) ? parsed : 0;
}

// ─── 書き込み ────────────────────────────────────────────────────────────────

/** Redisのリスト型コンテキストキャッシュへ1件追記する（失敗は警告のみ） */
async function pushContextEntry(
	key: string,
	entry: ContextEntry,
	limit: number,
): Promise<void> {
	const redis = getRedisClient();
	if (!redis) return;
	try {
		await redis.rPush(key, JSON.stringify(entry));
		await redis.lTrim(key, -limit, -1);
		await redis.expire(key, CONTEXT_TTL_SECONDS);
	} catch (err) {
		console.error(
			"⚠️ Redis コンテキストキャッシュへの書き込みに失敗しました (SQLiteには保存済み):",
			err,
		);
	}
}

/**
 * 送受信メッセージを記録する（§7.1: 送信と同時にSQLiteへ永続保存）。
 * SQLite への永続化と Redis コンテキストキャッシュへの二重書き込みを行う。
 * Redis 書き込みの失敗は警告に留め、SQLite 保存が成功していればエラーにしない。
 */
export async function addMessageLog(
	userId: string,
	botId: string,
	role: "user" | "assistant",
	content: string,
	discordMsgId?: string,
	replyToMsgId?: string,
	source: "discord" | "pwa" = "discord",
): Promise<void> {
	// 1. SQLite へ永続化（正の履歴。guild_id = NULL は DM・秘書利用）
	const db = getDb();
	db.prepare(
		`INSERT INTO message_logs (user_id, bot_id, discord_msg_id, role, content, reply_to_msg_id, source)
     VALUES (?, ?, ?, ?, ?, ?, ?)`,
	).run(
		userId,
		botId,
		discordMsgId ?? null,
		role,
		content,
		replyToMsgId ?? null,
		source,
	);

	// 2. Redis コンテキストキャッシュへ追記（末尾が最新、直近15件のみ保持、TTLリセット）
	await pushContextEntry(
		contextKey(userId, `${botId}:${source}`),
		{ role, content },
		CONTEXT_LIMIT,
	);
}

// ─── コンテキスト取得・再構築（§3.1.4） ─────────────────────────────────────

/**
 * LLMへ渡す直近の会話コンテキストを取得する（古い順）。
 * Redis キャッシュを優先し、キャッシュが無ければ SQLite から直近 limit 件で再構築する。
 */
export async function getRecentContext(
	userId: string,
	botId: string,
	limit: number = CONTEXT_LIMIT,
	source: "discord" | "pwa" = "discord",
): Promise<ContextEntry[]> {
	const key = contextKey(userId, `${botId}:${source}`);
	const redis = getRedisClient();

	// 1. Redis キャッシュからの読み出しを試みる
	if (redis) {
		try {
			const cached = await redis.lRange(key, 0, -1);
			if (cached && cached.length > 0) {
				// アクセスに合わせて TTL をリセット（セッション有効期限に連動）
				await redis.expire(key, CONTEXT_TTL_SECONDS);
				const parsed = cached.map((item) => JSON.parse(item) as ContextEntry);
				return parsed.slice(-limit);
			}
		} catch (err) {
			console.error(
				"⚠️ Redis コンテキストの読み込みに失敗しました。SQLite から再構築します。:",
				err,
			);
		}
	}

	// 2. キャッシュミス時: SQLite から直近 limit 件で再構築（リセット境界より後のみ）。
	//    ギルド会話（guild_id 付き）および汎用モードBot（secretary なし）の owner DM は
	//    秘書コンテキストへ復元しない（要件 §4.6.1: 秘書とギルドBotのコンテキスト完全分離）
	const db = getDb();
	// Keep reset boundaries separate as well as the Redis keys and SQL rows.
	const floor = getContextFloor(userId, `${botId}:${source}`);
	const rows = db
		.prepare(
			`SELECT role, content FROM (
         SELECT id, role, content FROM message_logs
         WHERE user_id = ? AND bot_id = ? AND id > ? AND guild_id IS NULL AND source = ?
         ORDER BY id DESC
         LIMIT ?
       ) ORDER BY id ASC`,
		)
		.all(userId, botId, floor, source, limit) as { role: string; content: string }[];

	const history: ContextEntry[] = rows.map((row) => ({
		role: row.role as "user" | "assistant",
		content: row.content,
	}));

	// 3. 取得した履歴で Redis キャッシュを再構築
	if (redis && history.length > 0) {
		try {
			await redis.del(key);
			await redis.rPush(
				key,
				history.map((item) => JSON.stringify(item)),
			);
			await redis.lTrim(key, -CONTEXT_LIMIT, -1);
			await redis.expire(key, CONTEXT_TTL_SECONDS);
		} catch (err) {
			console.error(
				"⚠️ Redis コンテキストキャッシュの再構築に失敗しました:",
				err,
			);
		}
	}

	return history;
}

/**
 * ユーザーの会話コンテキストをリセットする。
 * Redis キャッシュを削除し、リセット境界（現在の最大id）を記録することで
 * 以降の SQLite 再構築でも過去メッセージを復元しないようにする。
 * 永続ログ (message_logs) 自体は削除しない（§7.1: 件数・期間の制限なし。検索 §3.12 でも利用）。
 */
export async function clearContext(
	userId: string,
	botId: string,
): Promise<void> {
	// 1. リセット境界を記録（SQLite からの再構築を防ぐ）
	const db = getDb();
	const row = db
		.prepare(
			"SELECT MAX(id) AS max_id FROM message_logs WHERE user_id = ? AND bot_id = ?",
		)
		.get(userId, botId) as { max_id: number | null };
	if (row.max_id !== null) {
		setSystemSetting(contextFloorKey(userId, botId), String(row.max_id));
	}

	// 2. Redis キャッシュを削除
	const redis = getRedisClient();
	if (redis) {
		try {
			await redis.del(contextKey(userId, botId));
		} catch (err) {
			console.error("⚠️ Redis コンテキストキャッシュの削除に失敗しました:", err);
		}
	}
}

// ─── 汎用モード: ギルド単位コンテキスト（bot_attributes_requirements.md §4.6.1） ─
//
// 会話は bot_id × guild_id でスコープし、ギルド内の利用メンバー間で共有される1つの
// 流れとして扱う（チャンネル分離はPhase 2）。秘書の context:{userId} とは完全に分離する。
// 発話者の区別は記録時に content へ「[表示名]: 」プレフィックスを付けて表現する（呼び出し側）。

/**
 * ギルド会話を記録する（SQLite 永続化 + Redis ギルドコンテキストへ二重書き込み）。
 * user_id にはWebアカウント未登録のDiscordユーザーIDも入る（メンバー制 §4.3.3）。
 */
export async function addGuildMessageLog(
	botId: string,
	guildId: string,
	userId: string,
	role: "user" | "assistant",
	content: string,
	discordMsgId?: string,
	replyToMsgId?: string,
): Promise<void> {
	const db = getDb();
	db.prepare(
		`INSERT INTO message_logs (user_id, bot_id, discord_msg_id, role, content, reply_to_msg_id, guild_id)
     VALUES (?, ?, ?, ?, ?, ?, ?)`,
	).run(
		userId,
		botId,
		discordMsgId ?? null,
		role,
		content,
		replyToMsgId ?? null,
		guildId,
	);

	await pushContextEntry(
		guildContextKey(botId, guildId),
		{ role, content },
		GUILD_CONTEXT_LIMIT,
	);
}

/**
 * ギルドコンテキスト（直近30件・古い順）を取得する。
 * Redis キャッシュ優先、ミス時は SQLite（bot_id × guild_id）から再構築する。
 */
export async function getGuildContext(
	botId: string,
	guildId: string,
	limit: number = GUILD_CONTEXT_LIMIT,
): Promise<ContextEntry[]> {
	const key = guildContextKey(botId, guildId);
	const redis = getRedisClient();

	if (redis) {
		try {
			const cached = await redis.lRange(key, 0, -1);
			if (cached && cached.length > 0) {
				await redis.expire(key, CONTEXT_TTL_SECONDS);
				return cached
					.map((item) => JSON.parse(item) as ContextEntry)
					.slice(-limit);
			}
		} catch (err) {
			console.error(
				"⚠️ Redis ギルドコンテキストの読み込みに失敗しました。SQLite から再構築します。:",
				err,
			);
		}
	}

	const db = getDb();
	const rows = db
		.prepare(
			`SELECT role, content FROM (
         SELECT id, role, content FROM message_logs
         WHERE bot_id = ? AND guild_id = ?
         ORDER BY id DESC
         LIMIT ?
       ) ORDER BY id ASC`,
		)
		.all(botId, guildId, limit) as { role: string; content: string }[];

	const history: ContextEntry[] = rows.map((row) => ({
		role: row.role as "user" | "assistant",
		content: row.content,
	}));

	if (redis && history.length > 0) {
		try {
			await redis.del(key);
			await redis.rPush(
				key,
				history.map((item) => JSON.stringify(item)),
			);
			await redis.lTrim(key, -GUILD_CONTEXT_LIMIT, -1);
			await redis.expire(key, CONTEXT_TTL_SECONDS);
		} catch (err) {
			console.error(
				"⚠️ Redis ギルドコンテキストキャッシュの再構築に失敗しました:",
				err,
			);
		}
	}

	return history;
}

// ─── 汎用モード: owner DM コンテキスト（要件 §4.3.2 / §4.6.1: 直近15件で分離） ─

/** owner との DM 会話を記録する（ギルドコンテキストとは分離した専用キャッシュ） */
export async function addBotDmMessageLog(
	botId: string,
	ownerId: string,
	role: "user" | "assistant",
	content: string,
	discordMsgId?: string,
	replyToMsgId?: string,
): Promise<void> {
	const db = getDb();
	db.prepare(
		`INSERT INTO message_logs (user_id, bot_id, discord_msg_id, role, content, reply_to_msg_id)
     VALUES (?, ?, ?, ?, ?, ?)`,
	).run(
		ownerId,
		botId,
		discordMsgId ?? null,
		role,
		content,
		replyToMsgId ?? null,
	);

	await pushContextEntry(
		botDmContextKey(botId, ownerId),
		{ role, content },
		CONTEXT_LIMIT,
	);
}

/** owner DM コンテキスト（直近15件・古い順）を取得する */
export async function getBotDmContext(
	botId: string,
	ownerId: string,
	limit: number = CONTEXT_LIMIT,
): Promise<ContextEntry[]> {
	const key = botDmContextKey(botId, ownerId);
	const redis = getRedisClient();

	if (redis) {
		try {
			const cached = await redis.lRange(key, 0, -1);
			if (cached && cached.length > 0) {
				await redis.expire(key, CONTEXT_TTL_SECONDS);
				return cached
					.map((item) => JSON.parse(item) as ContextEntry)
					.slice(-limit);
			}
		} catch (err) {
			console.error(
				"⚠️ Redis owner DMコンテキストの読み込みに失敗しました。SQLite から再構築します。:",
				err,
			);
		}
	}

	const db = getDb();
	const floor = getBotDmContextFloor(ownerId, botId);
	const rows = db
		.prepare(
			`SELECT role, content FROM (
         SELECT id, role, content FROM message_logs
         WHERE bot_id = ? AND user_id = ? AND id > ? AND guild_id IS NULL
         ORDER BY id DESC
         LIMIT ?
       ) ORDER BY id ASC`,
		)
		.all(botId, ownerId, floor, limit) as { role: string; content: string }[];

	const history: ContextEntry[] = rows.map((row) => ({
		role: row.role as "user" | "assistant",
		content: row.content,
	}));

	if (redis && history.length > 0) {
		try {
			await redis.del(key);
			await redis.rPush(
				key,
				history.map((item) => JSON.stringify(item)),
			);
			await redis.lTrim(key, -CONTEXT_LIMIT, -1);
			await redis.expire(key, CONTEXT_TTL_SECONDS);
		} catch (err) {
			console.error(
				"⚠️ Redis owner DMコンテキストキャッシュの再構築に失敗しました:",
				err,
			);
		}
	}

	return history;
}

/**
 * 汎用モード Bot の owner DM 会話コンテキストをリセットする。
 * 秘書の clearContext と同様に、Redis キャッシュを削除しリセット境界（現在の最大id）を
 * 記録することで、以降の SQLite 再構築でも過去メッセージを復元しないようにする。
 * 永続ログ (message_logs) 自体は削除しない（§7.1）。
 * 汎用モードの DM は owner 限定のため、対象は (ownerId, botId) の DM 会話（guild_id IS NULL）のみ。
 */
export async function clearBotDmContext(
	ownerId: string,
	botId: string,
): Promise<void> {
	// 1. リセット境界を記録（SQLite からの再構築を防ぐ）
	const db = getDb();
	const row = db
		.prepare(
			"SELECT MAX(id) AS max_id FROM message_logs WHERE user_id = ? AND bot_id = ? AND guild_id IS NULL",
		)
		.get(ownerId, botId) as { max_id: number | null };
	if (row.max_id !== null) {
		setSystemSetting(botDmContextFloorKey(botId, ownerId), String(row.max_id));
	}

	// 2. Redis キャッシュを削除
	const redis = getRedisClient();
	if (redis) {
		try {
			await redis.del(botDmContextKey(botId, ownerId));
		} catch (err) {
			console.error(
				"⚠️ Redis owner DMコンテキストキャッシュの削除に失敗しました:",
				err,
			);
		}
	}
}

// ─── 返信チェーン解決（§7.3） ────────────────────────────────────────────────

/**
 * DiscordメッセージIDから保存済みメッセージを1件取得する。
 * 注意: DiscordメッセージIDはグローバル一意（Snowflake）だが、
 * 呼び出し側は取得結果の user_id が要求ユーザーと一致するか必ず確認すること（データ分離）。
 */
export function getMessageByDiscordId(
	discordMsgId: string,
): MessageLogRecord | undefined {
	const db = getDb();
	return db
		.prepare(
			"SELECT * FROM message_logs WHERE discord_msg_id = ? ORDER BY id DESC LIMIT 1",
		)
		.get(discordMsgId) as MessageLogRecord | undefined;
}

/**
 * 返信チェーンを再帰的に遡って解決する（§7.3）。
 * reply_to_msg_id を辿り、ルートメッセージ（reply_to_msg_id = NULL）または
 * 上限深度 maxDepth（デフォルト: config.replyChainMaxDepth）に達した時点で停止する。
 * 戻り値は古い順（ルート→直近の返信元）。本人の user_id に一致するメッセージのみ辿る。
 */
export function resolveReplyChain(
	userId: string,
	replyToMsgId: string,
	maxDepth: number = config.replyChainMaxDepth,
): MessageLogRecord[] {
	const db = getDb();
	const stmt = db.prepare(
		"SELECT * FROM message_logs WHERE discord_msg_id = ? AND user_id = ? ORDER BY id DESC LIMIT 1",
	);

	const chain: MessageLogRecord[] = [];
	const visited = new Set<string>(); // 循環参照による無限ループ防止
	let currentMsgId: string | null = replyToMsgId;

	while (currentMsgId && chain.length < maxDepth) {
		if (visited.has(currentMsgId)) break;
		visited.add(currentMsgId);

		const record = stmt.get(currentMsgId, userId) as
			| MessageLogRecord
			| undefined;
		if (!record) break; // ログに無いメッセージ（Bot導入前・他ユーザー等）に達したら停止

		chain.unshift(record); // 古い順に並べる
		currentMsgId = record.reply_to_msg_id;
	}

	return chain;
}

/**
 * ギルド会話の返信チェーンを再帰的に解決する（汎用モード用）。
 * bot_id × guild_id スコープで辿るため、メンバー全員の発話がチェーン対象になる。
 */
export function resolveGuildReplyChain(
	botId: string,
	guildId: string,
	replyToMsgId: string,
	maxDepth: number = config.replyChainMaxDepth,
): MessageLogRecord[] {
	const db = getDb();
	const stmt = db.prepare(
		"SELECT * FROM message_logs WHERE discord_msg_id = ? AND bot_id = ? AND guild_id = ? ORDER BY id DESC LIMIT 1",
	);

	const chain: MessageLogRecord[] = [];
	const visited = new Set<string>();
	let currentMsgId: string | null = replyToMsgId;

	while (currentMsgId && chain.length < maxDepth) {
		if (visited.has(currentMsgId)) break;
		visited.add(currentMsgId);

		const record = stmt.get(currentMsgId, botId, guildId) as
			| MessageLogRecord
			| undefined;
		if (!record) break;

		chain.unshift(record);
		currentMsgId = record.reply_to_msg_id;
	}

	return chain;
}

// ─── 全文検索（§3.12: FTS5 trigram） ─────────────────────────────────────────

/**
 * 期間指定を created_at の保存形式 'YYYY-MM-DD HH:MM:SS' に正規化する。
 * 日付のみ指定の場合、終了側はその日の終わり（23:59:59）まで含める。
 */
function normalizePeriod(value: string, isEnd: boolean): string {
	const v = value.trim().replace("T", " ");
	if (/^\d{4}-\d{2}-\d{2}$/.test(v)) {
		return isEnd ? `${v} 23:59:59` : `${v} 00:00:00`;
	}
	return v;
}

/**
 * 過去の会話履歴を全文検索する（§3.12）。
 * プライバシー配慮（§3.12.3）: 本人の user_id を必須条件とし、他ユーザーの会話は対象外。
 * - keyword あり（3文字以上）: FTS5 (trigramトークナイザ) の MATCH で高速検索
 * - keyword あり（3文字未満）: trigram は3文字未満を索引できないため LIKE にフォールバック
 * - keyword なし: 期間のみで検索
 * 戻り値は新しい順。
 */
export function searchMessages(
	userId: string,
	botId: string,
	options: MessageSearchOptions,
): MessageLogRecord[] {
	const db = getDb();
	const { keyword, from, to } = options;
	const limit = Math.min(Math.max(Math.floor(options.limit ?? 10), 1), 100);

	// 期間条件（created_at 範囲）を組み立てる
	const periodConds: string[] = [];
	const periodParams: string[] = [];
	if (from) {
		periodConds.push("m.created_at >= ?");
		periodParams.push(normalizePeriod(from, false));
	}
	if (to) {
		periodConds.push("m.created_at <= ?");
		periodParams.push(normalizePeriod(to, true));
	}
	const periodSql =
		periodConds.length > 0 ? ` AND ${periodConds.join(" AND ")}` : "";

	const trimmedKeyword = keyword?.trim();

	if (trimmedKeyword && Array.from(trimmedKeyword).length >= 3) {
		// FTS5 MATCH 構文のエスケープ: ダブルクォートで囲み、内部の " は "" に二重化
		// （ユーザー入力を演算子として解釈させない）
		const matchExpr = `"${trimmedKeyword.replace(/"/g, '""')}"`;
		return db
			.prepare(
				`SELECT m.* FROM message_logs_fts
         JOIN message_logs m ON m.id = message_logs_fts.rowid
         WHERE message_logs_fts MATCH ? AND m.user_id = ? AND m.bot_id = ? AND m.guild_id IS NULL${periodSql}
         ORDER BY m.id DESC LIMIT ?`,
			)
			.all(
				matchExpr,
				userId,
				botId,
				...periodParams,
				limit,
			) as MessageLogRecord[];
	}

	if (trimmedKeyword) {
		// trigram トークナイザは3文字未満の部分一致を検索できないため LIKE で代替
		const likePattern = `%${trimmedKeyword.replace(/[\\%_]/g, (ch) => `\\${ch}`)}%`;
		return db
			.prepare(
				`SELECT m.* FROM message_logs m
         WHERE m.user_id = ? AND m.bot_id = ? AND m.guild_id IS NULL AND m.content LIKE ? ESCAPE '\\'${periodSql}
         ORDER BY m.id DESC LIMIT ?`,
			)
			.all(
				userId,
				botId,
				likePattern,
				...periodParams,
				limit,
			) as MessageLogRecord[];
	}

	// keyword 省略時: 期間のみで検索
	return db
		.prepare(
			`SELECT m.* FROM message_logs m
       WHERE m.user_id = ? AND m.bot_id = ? AND m.guild_id IS NULL${periodSql}
       ORDER BY m.id DESC LIMIT ?`,
		)
		.all(userId, botId, ...periodParams, limit) as MessageLogRecord[];
}

/**
 * ギルド会話を全文検索する（汎用モード。bot_attributes_requirements.md §4.6.1）。
 * 検索対象は bot_id × guild_id のそのギルドでの会話のみ（他ギルド・DM・秘書の会話は対象外）。
 */
export function searchGuildMessages(
	botId: string,
	guildId: string,
	options: MessageSearchOptions,
): MessageLogRecord[] {
	const db = getDb();
	const { keyword, from, to } = options;
	const limit = Math.min(Math.max(Math.floor(options.limit ?? 10), 1), 100);

	const periodConds: string[] = [];
	const periodParams: string[] = [];
	if (from) {
		periodConds.push("m.created_at >= ?");
		periodParams.push(normalizePeriod(from, false));
	}
	if (to) {
		periodConds.push("m.created_at <= ?");
		periodParams.push(normalizePeriod(to, true));
	}
	const periodSql =
		periodConds.length > 0 ? ` AND ${periodConds.join(" AND ")}` : "";

	const trimmedKeyword = keyword?.trim();

	if (trimmedKeyword && Array.from(trimmedKeyword).length >= 3) {
		const matchExpr = `"${trimmedKeyword.replace(/"/g, '""')}"`;
		return db
			.prepare(
				`SELECT m.* FROM message_logs_fts
         JOIN message_logs m ON m.id = message_logs_fts.rowid
         WHERE message_logs_fts MATCH ? AND m.bot_id = ? AND m.guild_id = ?${periodSql}
         ORDER BY m.id DESC LIMIT ?`,
			)
			.all(
				matchExpr,
				botId,
				guildId,
				...periodParams,
				limit,
			) as MessageLogRecord[];
	}

	if (trimmedKeyword) {
		const likePattern = `%${trimmedKeyword.replace(/[\\%_]/g, (ch) => `\\${ch}`)}%`;
		return db
			.prepare(
				`SELECT m.* FROM message_logs m
         WHERE m.bot_id = ? AND m.guild_id = ? AND m.content LIKE ? ESCAPE '\\'${periodSql}
         ORDER BY m.id DESC LIMIT ?`,
			)
			.all(
				botId,
				guildId,
				likePattern,
				...periodParams,
				limit,
			) as MessageLogRecord[];
	}

	return db
		.prepare(
			`SELECT m.* FROM message_logs m
       WHERE m.bot_id = ? AND m.guild_id = ?${periodSql}
       ORDER BY m.id DESC LIMIT ?`,
		)
		.all(botId, guildId, ...periodParams, limit) as MessageLogRecord[];
}

/** Bot別の日次利用回数（直近 days 日。コスト可視化 要件 §6） */
export function countBotDailyUsage(
	botId: string,
	days: number = 14,
): Array<{ date: string; count: number }> {
	const db = getDb();
	const clamped = Math.min(Math.max(Math.floor(days), 1), 90);
	// bot_id 単位の集計（コスト可視化のための読み取り専用クエリ。user_id 全件走査の明示的例外）
	return db
		.prepare(
			`SELECT date(created_at) AS date, COUNT(*) AS count
       FROM message_logs
       WHERE bot_id = ? AND role = 'user' AND created_at >= date('now', 'localtime', ?)
       GROUP BY date(created_at)
       ORDER BY date DESC`,
		)
		.all(botId, `-${clamped} days`) as Array<{ date: string; count: number }>;
}

/**
 * YYYY-MM-DD 形式のローカル日付文字列（offsetDays 日前）。
 * message_logs.created_at は列DEFAULT datetime('now','localtime') によりローカル時刻で保存されるため、
 * 集計側は bare date(created_at)（'localtime' 修飾子なし）で一致する。ここに 'localtime' を足すと
 * 二重変換になり日付がUTCオフセット分ずれるため付けないこと。
 */
function localDateString(offsetDays: number): string {
	const d = new Date();
	d.setDate(d.getDate() - offsetDays);
	const y = d.getFullYear();
	const m = String(d.getMonth() + 1).padStart(2, "0");
	const day = String(d.getDate()).padStart(2, "0");
	return `${y}-${m}-${day}`;
}

/**
 * Bot別の日次メッセージ統計（直近 days 日、今日を含み欠損日は0補完）。
 * 汎用モードのAPI使用量チャート用（コスト可視化 要件 §6）。
 * requests = 受信（role='user'）、responses = 応答（role='assistant'）。
 */
export function getBotUsageSeries(
	botId: string,
	days: number = 14,
): {
	series: Array<{ date: string; requests: number; responses: number }>;
	totals: { requests: number; responses: number };
} {
	const db = getDb();
	const clamped = Math.min(Math.max(Math.floor(days), 1), 90);
	// bot_id 単位の集計（コスト可視化のための読み取り専用クエリ。user_id 全件走査の明示的例外）
	const rows = db
		.prepare(
			`SELECT date(created_at) AS date, role, COUNT(*) AS count
       FROM message_logs
       WHERE bot_id = ? AND created_at >= date('now', 'localtime', ?)
       GROUP BY date(created_at), role`,
		)
		.all(botId, `-${clamped - 1} days`) as Array<{
		date: string;
		role: string;
		count: number;
	}>;

	const byDate = new Map<string, { requests: number; responses: number }>();
	for (const r of rows) {
		const entry = byDate.get(r.date) ?? { requests: 0, responses: 0 };
		if (r.role === "user") entry.requests += r.count;
		else if (r.role === "assistant") entry.responses += r.count;
		byDate.set(r.date, entry);
	}

	// 直近 clamped 日（今日を含む）の連続日付列を生成し、欠損日を0補完する（チャートのX軸を連続させる）
	const series: Array<{ date: string; requests: number; responses: number }> =
		[];
	let totalReq = 0;
	let totalRes = 0;
	for (let i = clamped - 1; i >= 0; i--) {
		const date = localDateString(i);
		const e = byDate.get(date) ?? { requests: 0, responses: 0 };
		totalReq += e.requests;
		totalRes += e.responses;
		series.push({ date, requests: e.requests, responses: e.responses });
	}
	return { series, totals: { requests: totalReq, responses: totalRes } };
}

/** ユーザーの保存済みメッセージ総数を返す（統計・レポート用） */
export function countMessages(userId: string): number {
	const db = getDb();
	const row = db
		.prepare("SELECT COUNT(*) AS cnt FROM message_logs WHERE user_id = ?")
		.get(userId) as { cnt: number };
	return row.cnt;
}
