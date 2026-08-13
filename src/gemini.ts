import {
	type Content,
	type Part,
	type FunctionCall,
} from "@google/generative-ai";
import type { EmbedBuilder } from "discord.js";
import fs from "node:fs";
import path from "node:path";
import {
	getFunctionModulesForCapabilities,
	getGuildAssistantFunctionModules,
} from "./functions/index.js";
import { buildFunctionRegistry } from "./functions/registry.js";
import { getMcpFunctionModuleForBot } from "./functions/mcpDynamic.js";
import {
	isCalendarEnabled,
	getCachedCalendars,
	getResolvedCalendarId,
} from "./services/googleCalendarService.js";
import {
	addMessageLog,
	getRecentContext,
	resolveReplyChain,
	addGuildMessageLog,
	getGuildContext,
	resolveGuildReplyChain,
	addBotDmMessageLog,
	getBotDmContext,
	GUILD_CONTEXT_LIMIT,
	type ContextEntry,
} from "./db/messageLogRepo.js";
import { getActivePersonaPrompt, getPersonaById } from "./db/personaRepo.js";
import { getContextNote } from "./db/contextNoteRepo.js";
import { getBotUserNote, getBotGuildNote } from "./db/botNoteRepo.js";
import { getBotById, type BotRecord } from "./db/botRepo.js";
import { resolveBotCapabilities } from "./services/botCapabilities.js";
import { recordFunctionCall } from "./services/actionRecorder.js";
import { getUserGoogleConfig, getUserRichReplyEnabled } from "./db/userRepo.js";
import { getUserGenAI, getBotGenAI } from "./services/llmClient.js";
import { config } from "./config.js";
import type { ToolContext, FunctionModule } from "./types/contracts.js";

// ─── 検索クロールスキル（docs/skills/search_skills.md のインライン読み込み） ──────────

let cachedSearchSkills: string | null = null;

/** 検索スキル仕様書を読み込む（存在しない場合は空文字） */
function loadSearchSkills(): string {
	if (cachedSearchSkills !== null) return cachedSearchSkills;
	const candidates = [
		path.resolve(process.cwd(), "docs/skills/search_skills.md"),
		path.resolve(process.cwd(), "docs/search_skills.md"), // 後方互換（旧配置）
	];
	for (const p of candidates) {
		try {
			if (fs.existsSync(p)) {
				cachedSearchSkills = fs.readFileSync(p, "utf-8");
				return cachedSearchSkills;
			}
		} catch {}
	}
	cachedSearchSkills = "";
	return cachedSearchSkills;
}

// ─── システムプロンプト構築 ──────────────────────────────────────────────────

/** デフォルトペルソナ（§4.1.1: 最低限の設定＝丁寧なアシスタント） */
const DEFAULT_PERSONA = `# あなたの役割
あなたは、タスク管理・スケジュール管理・家計管理・ブラウザ自動操作を支援する汎用AIアシスタントです。ユーザーの日常的な生産性向上と生活管理を、的確かつ効率的にサポートしてください。

# アシスタントプロファイル
- **スタイル:** 丁寧・論理的・実務的。過度なキャラクター演技はせず、フレンドリーかつプロフェッショナルに対応します。
- **応答方針:** ユーザーの意図を正確に把握し、必要な情報を整理して簡潔に伝えます。
- **優先事項:** 正確性・効率性・一貫性。`;

/**
 * ユーザー毎のシステムプロンプトを構築する（§3.1.2）
 * 構成順: ペルソナ → 機能・ルール → コンテキストノート（末尾 §3.7.3）
 */
async function buildSystemInstruction(
	userId: string,
	botId: string,
	richReplyEnabled: boolean,
	source: "discord" | "pwa",
): Promise<string> {
	const now = new Date();
	const year = now.getFullYear();
	const month = now.getMonth() + 1;
	const date = now.getDate();
	const hours = String(now.getHours()).padStart(2, "0");
	const minutes = String(now.getMinutes()).padStart(2, "0");
	const seconds = String(now.getSeconds()).padStart(2, "0");
	const dayOfWeek = ["日", "月", "火", "水", "木", "金", "土"][now.getDay()];
	const dateTimeStr = `${year}年${month}月${date}日 (${dayOfWeek}) ${hours}時${minutes}分${seconds}秒`;

	// ペルソナ（§4.1 / v8: (user_id, bot_id) 毎に独立。未設定時はデフォルト）
	let personaPrompt: string | null = null;
	try {
		personaPrompt = getActivePersonaPrompt(userId, botId);
	} catch (err) {
		console.error("ペルソナの取得に失敗しました:", err);
	}
	const personaSection = personaPrompt?.trim()
		? `# あなたの役割・キャラクター設定（ペルソナ）\n${personaPrompt.trim()}`
		: DEFAULT_PERSONA;

	// 連携中のGoogleカレンダー情報
	let calendarInfo = "";
	if (isCalendarEnabled(userId, botId)) {
		try {
			const calendars = await getCachedCalendars(userId, botId);
			const defaultCalendarId = getResolvedCalendarId(userId, botId);
			if (calendars.length > 0) {
				calendarInfo = `\n# 連携中のGoogleカレンダー一覧\n現在、予定を登録可能なカレンダーは以下の通りです。ユーザーからの予定追加指示の際、その内容や目的に最も適したカレンダーの「ID」を選択し、addSchedule関数の calendar_id 引数に指定して登録してください。\n`;
				for (const cal of calendars) {
					calendarInfo += `- カレンダー名: "${cal.summary}" (ID: "${cal.id}")\n`;
				}
				calendarInfo += `※もし内容や目的に合うカレンダーがない場合は、デフォルトのカレンダーIDである "${defaultCalendarId}" を使用してください。\n`;
			}
		} catch (err) {
			console.error(
				"システムプロンプト用のカレンダー一覧取得に失敗しました:",
				err,
			);
		}
	}

	// コンテキストノート（§3.7: システムプロンプトの末尾＝ペルソナの後に挿入）
	let contextNoteSection = "";
	try {
		const note = getContextNote(userId, botId);
		if (note && note.trim()) {
			contextNoteSection = `\n# コンテキストノート（ユーザーが「覚えておいてほしい」と登録した背景情報）\n以下はユーザー固有の考慮事項・背景知識です。会話・判断の際に常に考慮してください。\n${note.trim()}`;
		}
	} catch (err) {
		console.error("コンテキストノートの取得に失敗しました:", err);
	}

	// 記憶系ツールの使い分けルール（§3.6, §3.7, §3.10）
	const memoryRuleSection = `
# 情報保存ツールの使い分けルール（極めて重要）
あなたが情報を保存する際は、対象情報の性質に応じて以下を明確に使い分けてください。
1. **コンテキストノート（appendContextNote）**: ユーザーの長期的な属性・好み・習慣・背景知識（例:「乳製品アレルギー」「仕事はエンジニア」「締め切りは毎週金曜」）。既存ノートと重複・矛盾する情報を検出した場合は、ユーザーに確認した上で setContextNote で全体を整理して更新してください。
2. **クリップボード（addClipboardEntry）**: 「今日・今だけ」の揮発的な一時メモ（例:「今日の会議メモ」「あとで調べるURL」）。期限付き（デフォルト24時間）で自動削除されます。
3. **マクロ／Playbook（savePlaybook）**: Webログイン手順・データ取得手順など複数ステップの「操作・自動化の手順」。ユーザーが「今の操作を覚えておいて」と言った場合は getRecentActionHistory で直近の操作履歴を取得し、手順を要約してマクロ候補（呼び出し名・説明・手順）を提示し、承認を得てから savePlaybook で保存してください（§3.6）。
※静的な好み・事実を Playbook に保存してはいけません。操作手順をコンテキストノートに保存してもいけません。`;

	// ユーザー承認フロー（仕様で承認必須とされている操作）
	const confirmationRuleSection = `
# ユーザー承認フロー（必ず守ること）
以下の操作は、実行内容をユーザーに提示して明示的な承認を得てから確定してください。
- **マクロの実行**: findPlaybooks でマッチした手順は、実行内容を要約提示 → 承認後に runPlaybook で手順を取得し実行する。
- **タスク優先度の確定**: organizeTaskPriorities で取得・分析した提案はユーザーに提示のみ行い、承認後に applyTaskPriorities で確定する。
- **支払い予定の消込**: findSettlementCandidates で見つかった消込候補はペアを提示し、承認後に settlePlannedPayment を呼ぶ。
- **認証情報の登録・更新・削除**: 内容を復唱確認してから addCredential / updateCredential / deleteCredential を呼ぶ。
- **支払い予定の登録後**: 「ToDoとして追加する？」「リマインドを設定する？」を確認し、希望があれば linkPlannedPaymentTodo / linkPlannedPaymentReminder を呼ぶ。`;

	// リッチ返信ルール（§3.0）
	const richReplySection = richReplyEnabled
		? `
# リッチ返信の使い分け（§3.0）
返信の性質に応じてプレーンテキストとリッチ形式を使い分けてください。
- 単純な一問一答 → プレーンテキスト
- データの一覧・サマリ（タスク一覧、家計サマリ、連絡先詳細など） → showRichContent（Embed）
- 数値データの視覚化が有用な場合（カテゴリ別支出の内訳、月次推移、予算消化率、気温推移など） → sendChart（グラフ画像）
- エラー・警告の通知 → showRichContent（colorに error / warning を指定）
リッチ形式を使った場合も、本文テキストで要点を簡潔に添えてください。`
		: `
# リッチ返信は無効
ユーザー設定によりリッチ返信（Embed・グラフ）は無効です。showRichContent / sendChart を呼ばず、すべてプレーンテキストで返答してください。`;

	// 音声メモ（§3.14）
	const voiceRuleSection = `
# 音声メモの取り扱い（音声ファイルを受信した場合）
1. まず音声を正確に文字起こしし、結果をユーザーにプレビューとして提示する。
2. 内容に「〜しておいて」「〜を忘れないように」などのタスク依頼が含まれる場合は、ToDoへの変換を提案し、承認後に addTodo を呼ぶ。
3. ユーザーが希望すれば addClipboardEntry で文字起こし結果をクリップボードに保存する。`;

	// 検索スキル（docs/skills/search_skills.md をインライン化）
	const searchSkills = loadSearchSkills();
	const searchSkillsSection = searchSkills
		? `\n# 検索クロールスキル仕様書（インデックス）\n外部検索（searchWeb）や情報収集を行う際は、以下のスキル仕様書に合致するカテゴリがあれば、その推奨ドメイン・推奨クエリパターン・巡回（fetchDynamicPage）フローに従って調査を組み立ててください。\n\n${searchSkills}`
		: "";

	const parts = [
		source === "pwa"
			? `# Request channel\nThis request came from the personal PWA control panel, not Discord. Keep the PWA conversation context separate from Discord. Return standard Markdown suitable for the web client. When a task, calendar event, finance record, or shared note is relevant, state it clearly so the client can link to the corresponding screen.`
			: `# Request channel\nThis request came from Discord. Follow the Discord response conventions and do not assume a web UI is visible.`,
		personaSection,
		memoryRuleSection,
		confirmationRuleSection,
		richReplySection,
		voiceRuleSection,
		"",
		`# リアルタイム情報の正確性とファクトチェック（極めて重要）
- ユーザーから天気予報、電車の運行情報、ニュース、最新技術トレンド、または事実確認を求められた場合、不正確な推測や無根拠なデータを伝えてはいけません。
- 異なるソース同士で情報が食い違う場合は、数値の論理的整合性を確認し、必ず最も公式で最新のデータを優先してください。不確かな情報でユーザーの予定を狂わせないよう、徹底的に検証された正確な情報を伝えること。`,
		searchSkillsSection,
		"",
		`# あなたの機能（Discordアシスタントボットとしてのツール）
あなたはDiscord上の優秀なアシスタントボットとして以下の機能を持っています。ツールを適切に使い、論理的かつ効率的にユーザーをサポートしてください。

1. **タスク管理（ToDo）:** タスクの追加・一覧・完了・削除・タグ別表示・優先度整理。タグはバックグラウンドで自動付与されます。
2. **予定管理（スケジュール）:** 予定の登録・一覧・削除。Googleカレンダーと自動的に双方向同期されます。
3. **リマインド:** 時刻指定・繰り返し（cron式）のリマインドを設定できます。
4. **家計管理:** 収入・支出の記録、月間サマリー、カテゴリ別内訳、予算上限、支払い予定の登録と消込。
5. **メモ:** コンテキストノート（長期）、クリップボード（短期・TTL付き）、連絡先管理。
6. **会話ログ検索:** 過去の会話履歴をキーワード・期間で検索して要約できます（searchConversationLogs）。
7. **朝報・日報・週報:** 天気・ニュースの定期配信や日次・週次レポートの設定を変更できます（configureBriefing / configureReport）。
8. **インタラクティブブラウザ操作（ブラウザ自動化）:** ユーザーの代わりに特定のWebサイトを開き、入力、クリック、待機、ステータス確認などのインタラクティブ操作を行います。
   - **【最重要】一意の数値IDの最優先利用**: \`browserInteractiveStatus\` で取得できるマークダウン内の入力フィールドやボタンなどの要素には、\`[Input (text) ID: 2]\` や \`[Button ID: 3]\` のように **\`ID: 数値\`**（一意の数値ID）が一意に付与されています。
   - \`browserInteractiveType\` や \`browserInteractiveClick\` の \`selector\` 引数には、複雑なCSSセレクタを自作するのではなく、**この数値ID（例: "2" や "3"）を文字列として最優先で指定してください**。これが最も確実でエラーのない操作方法です。
   - 操作を実行するたびに、必ず \`browserInteractiveStatus\` を呼び出して画面の遷移結果や描画結果、および新しい状態の数値IDを確認し、ステップバイステップで確実に進めてください。
   - **自動ログイン:** 認証が必要なサイトでは (1) browserInteractiveOpen でログインページを開く → (2) browserInteractiveStatus で入力フィールドの数値IDを確認 → (3) browserFillCredential にサービス名とフィールドIDを渡して認証情報を直接入力（パスワードはあなたには渡されません） → (4) browserInteractiveClick でログインボタンを押す、の順で進めてください。パスワードの値をチャットに出力することは固く禁止されています。`,
		"",
		`# 重要なシステムルール
- 現在の日時: ${dateTimeStr}
- **【重要】時制の制御と基準日時**: 検索を行う際、および検索結果を分析・要約する際は、**必ず上記の「現在の日時」を絶対的な基準として使用してください**。検索結果（Webページやニュース記事等）に記載されている「今日」「昨日」「3日前」「今年」「昨年」「最新」などの表現や日付情報は、この現在の日時から正確に逆算し、時系列や時制（過去・現在・未来）を正確に認識した上で、正しい時制で回答してください。
- 「明日」「来週月曜」などの相対的な日時表現は、適切なISO 8601形式に変換してツールを呼び出してください。
- ユーザーが「n時間後に教えて」「n分後にリマインドして」のように簡易タイマーを求めた場合は addReminder を使用してください。カレンダーに登録すべき「予定」は addSchedule を使用してください（カレンダーを汚したくない単発タイマーに addSchedule を使う場合は local_only を true に設定）。
- カレンダーに登録されるような通常の予定を追加または削除した際は「Googleカレンダーにも同期（削除）しました」と自然に一言添えてください。local_only やリマインダーの場合はカレンダー同期の旨は言わないでください。
- 金額は日本円（整数）で扱ってください。
- 家計のカテゴリは「食費, 日用品, 交通費, 光熱費, 通信費, 医療費, 娯楽, 衣服, その他」です。
- レシート画像を受け取った場合、各商品を適切なカテゴリに分類し、'addExpense'関数（source: receipt_ocr）を使って記録してください。記録前に読み取り内容のプレビューを提示し、対応する支払い予定が存在しそうなら findSettlementCandidates で消込候補を確認してください（§3.4.2）。
- 機能に関係ない雑談にもペルソナ設定に沿って自然に応じてください。
- **エラー・失敗時の対応:** ブラウザ操作などのツール実行中にエラーが発生した場合、あるいはユーザーが求めた結果が最終的に得られなかった場合は、絶対に「処理が完了しました」のように正常終了したと誤解させる応答をしないでください。必ず「失敗しました」または「求めた結果が得られませんでした」と明記し、その具体的な理由やどの段階で失敗したかを論理的・客観的に伝えてください。
- **【最重要】未実行の完了報告の禁止:** 「登録しました」「追加しました」「削除しました」「設定しました」「リマインドしておきました」のような操作完了の報告は、このターンで実際に対応するツール（関数）を呼び出し、その実行結果を受け取った場合に限り行ってください。ツールを呼び出さずに、頭の中で実行したつもりになって完了を報告することは固く禁止します。操作を行うと述べる場合は、必ずその場で対応する関数を呼び出してください。呼び出していない操作について「やっておきました」「しておきますね」と述べてはいけません。
${calendarInfo}`,
		contextNoteSection,
	];

	return parts.filter((p) => p !== "").join("\n");
}

// ─── Gemini API 呼び出し（リトライ付き） ─────────────────────────────────────

export interface ChatMessage {
	text: string;
	/** Input channel controls context isolation and client-specific response policy. */
	source?: "discord" | "pwa";
	imageData?: {
		data: string; // base64
		mimeType: string;
	};
	audioData?: {
		data: string; // base64
		mimeType: string;
	};
	/** 受信したDiscordメッセージID（会話ログ永続化用） */
	discordMsgId?: string;
	/** 返信元DiscordメッセージID（返信チェーン解決用 §3.1.4） */
	replyToMsgId?: string;
}

/** レート制限エラーかどうか判定 */
function isRateLimitError(error: unknown): boolean {
	if (error && typeof error === "object" && "status" in error) {
		return (error as { status: number }).status === 429;
	}
	return false;
}

/** サーバー側の一時的なエラー(503など)かどうか判定 */
function isServerError(error: unknown): boolean {
	if (error && typeof error === "object" && "status" in error) {
		const status = (error as { status: number }).status;
		return status === 500 || status === 502 || status === 503 || status === 504;
	}
	return false;
}

function sleep(ms: number): Promise<void> {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

// ─── ログ用の引数サニタイズ（§6.3.2: 認証情報をログに残さない） ───────────────

/** 引数に秘匿値を含み得るFunction（引数ログ自体を抑止する） */
const SECRET_ARG_FUNCTIONS = new Set(["addCredential", "updateCredential"]);

/** 秘匿すべき引数キーのパターン */
const SECRET_KEY_PATTERN = /password|secret|token|api_?key|credential/i;

/**
 * コンソールログ用に Function Call 引数をサニタイズする。
 * 認証情報系Functionは引数全体を伏せ、その他も秘匿キー名の値をマスクする。
 */
function sanitizeArgsForLog(
	name: string,
	args: Record<string, unknown>,
): string {
	if (SECRET_ARG_FUNCTIONS.has(name)) {
		return `{"(引数は秘匿情報を含むため非表示)": "service=${String((args as { service_name?: unknown }).service_name ?? "?")}"}`;
	}
	const masked: Record<string, unknown> = {};
	for (const [key, value] of Object.entries(args)) {
		masked[key] = SECRET_KEY_PATTERN.test(key) ? "(秘匿)" : value;
	}
	return JSON.stringify(masked).slice(0, 500);
}

/** GenAI ハンドル（getUserGenAI / getBotGenAI の戻り値） */
type GenAiHandle = NonNullable<ReturnType<typeof getUserGenAI>>;

/**
 * リトライ付きでGemini APIを呼び出す。
 * 実行キーは呼び出し側が解決する（秘書: ユーザー個別キー §4.2 / 汎用モード: Bot専用キー）。
 */
async function generateWithRetry(
	ai: GenAiHandle,
	systemInstruction: string,
	declarations: import("@google/generative-ai").FunctionDeclaration[],
	contents: Content[],
	maxRetries: number = 3,
): Promise<import("@google/generative-ai").GenerateContentResult> {
	const model = ai.genAI.getGenerativeModel(
		{
			model: ai.model,
			systemInstruction,
			...(declarations.length > 0
				? { tools: [{ functionDeclarations: declarations }] }
				: {}),
		},
		// タイムアウト無しだと応答が返らない場合に await が永遠に解決せず、
		// 「入力中...」タイマーと共にリクエストが滞留し続ける
		{ timeout: 120_000 },
	);

	for (let attempt = 0; attempt <= maxRetries; attempt++) {
		try {
			return await model.generateContent({ contents });
		} catch (error) {
			if (
				(isRateLimitError(error) || isServerError(error)) &&
				attempt < maxRetries
			) {
				// RetryInfo からリトライ待機時間を取得、なければ指数バックオフ
				let waitMs = Math.min(1000 * Math.pow(2, attempt + 1), 60000);

				const errorDetails = (
					error as {
						errorDetails?: Array<{ "@type": string; retryDelay?: string }>;
					}
				).errorDetails;
				if (errorDetails) {
					const retryInfo = errorDetails.find(
						(d) => d["@type"] === "type.googleapis.com/google.rpc.RetryInfo",
					);
					if (retryInfo?.retryDelay) {
						const seconds = parseInt(retryInfo.retryDelay.replace("s", ""), 10);
						if (!isNaN(seconds)) {
							waitMs = (seconds + 1) * 1000;
						}
					}
				}

				const errorType = isRateLimitError(error)
					? "レート制限(枯渇)"
					: "サーバー高負荷";
				console.log(
					`⏳ ${errorType} (${attempt + 1}/${maxRetries})、${Math.ceil(waitMs / 1000)}秒後にリトライ...`,
				);
				await sleep(waitMs);
				continue;
			}
			throw error;
		}
	}
	throw new Error("リトライ上限に達しました");
}

// ─── Function Calling ループ（秘書・汎用モード共通） ─────────────────────────

interface LoopResult {
	text: string;
	browserToolCalled: boolean;
	browserToolFailed: boolean;
}

/**
 * 操作の「完了」を主張するテキストかどうかを判定する（完了ハルシネーション検知用）。
 * Gemini が functionCall を出さず自然文だけで「登録しました」等と返した場合、
 * 実際には何も実行されていないため、これを検知して1度だけ是正を促す。
 * 誤検知を抑えるため、Bot のツール操作に紐づく強い過去形・意思表明パターンに限定する。
 */
function claimsActionCompleted(text: string): boolean {
	if (!text) return false;
	// 「登録しました」「追加しておきました」「設定しておきますね」等
	const done =
		/(登録|追加|削除|設定|記録|保存|作成|更新|消込|予約|同期|変更|オン|オフ|有効化|無効化)(し(ました|ておきました|ておきます|ますね?))/;
	// 「やっておきました」「しておきますね」等の汎用的な実行表明
	const generic = /(やって|して)おき(ました|ます(ね)?)/;
	return done.test(text) || generic.test(text);
}

/** 未実行の完了報告を検知した際に注入する是正プロンプト */
const COMPLETION_CORRECTION_PROMPT =
	"【システム検証】あなたは今回のやり取りで一度もツール（関数）を呼び出していません。" +
	"そのため、上記で報告した操作は実際には一切実行されていません。" +
	"本当にその操作を行うのであれば、今すぐ対応する関数を呼び出してください。" +
	"操作する必要がない、あるいは実行できない場合は、完了したかのように装わず、その旨を正直に伝えてください。";

/**
 * Function Calling ループを実行し、最終テキスト応答を返す（最大10回）。
 * 秘書・汎用モードで共通。マクロ用の操作履歴記録（actionRecorder）は秘書のみ有効化する。
 */
async function runFunctionCallingLoop(
	ai: GenAiHandle,
	systemInstruction: string,
	registry: ReturnType<typeof buildFunctionRegistry>,
	contents: Content[],
	ctx: ToolContext,
	onStatusChange?: (status: "thinking" | "writing" | "idle") => void,
	options: { recordActions?: boolean } = {},
): Promise<LoopResult> {
	let browserToolCalled = false;
	let browserToolFailed = false;

	onStatusChange?.("thinking");
	let result = await generateWithRetry(
		ai,
		systemInstruction,
		registry.declarations,
		contents,
		3,
	);
	let response = result.response;

	let iterations = 0;
	const maxIterations = 10;
	let totalFunctionCalls = 0;
	let correctionAttempted = false;

	while (iterations < maxIterations) {
		const candidate = response.candidates?.[0];
		if (!candidate) break;

		const functionCalls = candidate.content.parts.filter(
			(p): p is Part & { functionCall: FunctionCall } => "functionCall" in p,
		);

		if (functionCalls.length === 0) {
			// 完了ハルシネーション検知: このターンで一度も関数を呼ばずに操作完了を主張している場合、
			// 実際には何も実行されていないため、1度だけ是正を促して再生成する（無限ループ防止のため1回限り）。
			if (totalFunctionCalls === 0 && !correctionAttempted) {
				let currentText = "";
				try {
					currentText = response.text();
				} catch {}
				if (claimsActionCompleted(currentText)) {
					console.log(
						"⚠️ 未実行の完了報告を検知。是正プロンプトを注入して再生成します。",
					);
					correctionAttempted = true;
					contents.push(candidate.content);
					contents.push({
						role: "user",
						parts: [{ text: COMPLETION_CORRECTION_PROMPT }],
					});
					onStatusChange?.("thinking");
					result = await generateWithRetry(
						ai,
						systemInstruction,
						registry.declarations,
						contents,
						3,
					);
					response = result.response;
					iterations++;
					continue;
				}
			}
			break;
		}

		totalFunctionCalls += functionCalls.length;

		// 各function callを実行
		const functionResponseParts: Part[] = [];

		for (const fc of functionCalls) {
			const { name, args } = fc.functionCall;
			console.log(
				`🔧 Function Call: ${name}`,
				sanitizeArgsForLog(name, (args ?? {}) as Record<string, unknown>),
			);

			const functionResult = await registry.dispatch(
				ctx,
				name,
				(args ?? {}) as Record<string, unknown>,
			);
			console.log(
				`📤 Function Result (Sent to Gemini): ${functionResult.substring(0, 500)}${functionResult.length > 500 ? "... (truncated in console log)" : ""}`,
			);

			// マクロ登録用に操作履歴を記録（§3.6 実行ベース登録。秘匿系は記録側で除外される）
			if (options.recordActions) {
				recordFunctionCall(
					ctx.userId,
					name,
					(args ?? {}) as Record<string, unknown>,
				).catch(() => {});
			}

			// ブラウザツールの実行と成否判定
			if (
				name.startsWith("browserInteractive") ||
				name === "browserFillCredential" ||
				["fetchDynamicPage", "takePageScreenshot", "searchWeb"].includes(name)
			) {
				browserToolCalled = true;
				try {
					const parsed = JSON.parse(functionResult);
					if (parsed && parsed.success === false) {
						browserToolFailed = true;
					}
				} catch {
					browserToolFailed = true;
				}
			}

			let parsedResult: object;
			try {
				parsedResult = JSON.parse(functionResult) as object;
			} catch {
				parsedResult = { result: functionResult };
			}

			functionResponseParts.push({
				functionResponse: {
					name,
					response: parsedResult,
				},
			});
		}

		// Function結果を含めて再度Geminiに送信
		contents.push(candidate.content);
		contents.push({ role: "user", parts: functionResponseParts });

		// 最後のテキスト生成の直前で書き込み中ステータスに変更
		onStatusChange?.("writing");

		result = await generateWithRetry(
			ai,
			systemInstruction,
			registry.declarations,
			contents,
			3,
		);
		response = result.response;
		iterations++;
	}

	if (iterations >= maxIterations) {
		browserToolFailed = true;
	}

	// ステータス表示の自然な演出のための遅延
	if (iterations === 0) {
		onStatusChange?.("writing");
		await sleep(1000);
	} else {
		await sleep(800);
	}

	// 最終テキスト応答を取得
	let text = "";
	try {
		text = response.text();
	} catch (e) {
		console.warn("response.text() retrieval failed:", e);
	}

	return { text, browserToolCalled, browserToolFailed };
}

// ─── メッセージ処理本体 ──────────────────────────────────────────────────────

export interface ProcessResult {
	text: string;
	embeds: EmbedBuilder[];
	files: { attachment: Buffer; name: string }[];
}

/**
 * メッセージを処理し、Function Callingループを含む完全な応答を返す（§3.1.2）
 *
 * @param botId 実行中のBotインスタンスID（通知クライアント解決用）
 * @param userId DiscordユーザーID（全データ分離の必須キー）
 */
export async function processMessage(
	botId: string,
	userId: string,
	message: ChatMessage,
	onStatusChange?: (status: "thinking" | "writing" | "idle") => void,
): Promise<ProcessResult> {
	const source = message.source ?? "discord";
	// リッチ返信のユーザー設定（§3.0.5）
	let richReplyEnabled = true;
	try {
		richReplyEnabled = getUserRichReplyEnabled(userId);
	} catch {}

	// Function Call 実行コンテキスト
	const ctx: ToolContext = {
		botId,
		userId,
		embeds: [],
		files: [],
		richReplyEnabled,
	};

	// 1. ユーザーのメッセージを永続ログ + コンテキストキャッシュへ保存（§7.1）
	const logText =
		message.text?.trim() ||
		(message.audioData
			? "[音声メッセージ]"
			: message.imageData
				? "[画像]"
				: "");
	if (logText) {
		await addMessageLog(
			userId,
			botId,
			"user",
			logText,
			message.discordMsgId,
			message.replyToMsgId,
			source,
		);
	}

	// 2. 会話コンテキストを取得（Redis直近15件 → SQLiteフォールバック §3.1.4）
	const history = await getRecentContext(userId, botId, 15, source);

	// 3. 返信チェーンの解決（§3.1.4: 15件キャッシュとは別枠でコンテキストの先頭に追加）
	const contents: Content[] = [];
	if (message.replyToMsgId) {
		try {
			const chain = resolveReplyChain(
				userId,
				message.replyToMsgId,
				config.replyChainMaxDepth,
			);
			if (chain.length > 0) {
				const chainText = chain
					.map(
						(m) => `${m.role === "user" ? "ユーザー" : "あなた"}: ${m.content}`,
					)
					.join("\n");
				contents.push({
					role: "user",
					parts: [
						{
							text: `[返信チェーン（このメッセージは以下のやり取りへの返信です。古い順）]\n${chainText}\n[返信チェーンここまで]`,
						},
					],
				});
				contents.push({
					role: "model",
					parts: [{ text: "（返信チェーンの文脈を把握しました）" }],
				});
			}
		} catch (err) {
			console.error("返信チェーンの解決に失敗しました:", err);
		}
	}

	// 4. 履歴をGeminiのContents形式へ変換（同ロール連続は結合して交互にする）
	for (const entry of history) {
		const role = entry.role === "assistant" ? "model" : "user";
		const last = contents[contents.length - 1];
		if (last && last.role === role) {
			const lastPart = last.parts[0];
			if ("text" in lastPart) {
				lastPart.text += "\n" + entry.content;
			}
		} else {
			contents.push({ role, parts: [{ text: entry.content }] });
		}
	}

	// 履歴が空の場合のフォールバック
	if (contents.length === 0) {
		contents.push({ role: "user", parts: [{ text: message.text || "" }] });
	}

	// 5. 最新メッセージに画像・音声がある場合、直近のユーザーコンテンツへパーツ追加
	const inlineParts: Part[] = [];
	if (message.imageData) {
		inlineParts.push({
			inlineData: {
				data: message.imageData.data,
				mimeType: message.imageData.mimeType,
			},
		});
	}
	if (message.audioData) {
		inlineParts.push({
			inlineData: {
				data: message.audioData.data,
				mimeType: message.audioData.mimeType,
			},
		});
	}
	if (inlineParts.length > 0) {
		const lastContent = contents[contents.length - 1];
		if (lastContent && lastContent.role === "user") {
			lastContent.parts.push(...inlineParts);
		} else {
			contents.push({ role: "user", parts: [{ text: "" }, ...inlineParts] });
		}
	}

	// 6. Function レジストリの構築（属性ゲート §4.2: 保持ケーパビリティのモジュールのみ + MCP動的 §4.4）
	//    秘書プリセット（既存Bot・system_default 含む）では従来のフルセットと完全に一致する。
	const caps = resolveBotCapabilities(botId);
	let mcpModule: FunctionModule = { declarations: [], handlers: {} };
	if (caps.has("mcp")) {
		try {
			// v5: 当該Botに利用許可(bot_mcp_access)されたサーバー＋システムレベルから取得する。
			// v7: 共有秘書(system_default)では発話者(userId)所有分のみへスコープする（クロステナント露出防止）。
			mcpModule = await getMcpFunctionModuleForBot(botId, userId);
		} catch (err) {
			console.error("MCP動的ツールの取得に失敗しました（スキップ）:", err);
		}
	}
	const registry = buildFunctionRegistry([
		...getFunctionModulesForCapabilities(caps),
		mcpModule,
	]);

	const systemInstruction = await buildSystemInstruction(
		userId,
		botId,
		richReplyEnabled,
		source,
	);

	try {
		const ai = getUserGenAI(userId);
		if (!ai) {
			throw new Error(
				"Gemini API Keyが設定されていません。管理画面からあなた専用のAPIキーを設定してください。",
			);
		}

		const { text, browserToolCalled, browserToolFailed } =
			await runFunctionCallingLoop(
				ai,
				systemInstruction,
				registry,
				contents,
				ctx,
				onStatusChange,
				{ recordActions: true },
			);

		// 最終テキスト応答を決定し、実際に返す文面を必ず会話ログへ保存する。
		// 重要: 空応答時のフォールバック定型文も保存すること。これを怠ると当該ターンに
		// assistant の記録が一切残らず、次ターンのコンテキストではユーザーの元の要求が
		// 「未応答」のまま見えてしまう。するとモデルがその要求を満たそうとして直前の
		// ブラウザ操作（ページ取得・Web検索）を勝手に再実行し、daemon が暖まった2回目は
		// 成功するため「前回のブラウザ操作の結果」を次の命令で返す回帰が起きる。
		const replyText =
			text && text.trim()
				? text
				: browserToolCalled || browserToolFailed
					? "ブラウザ操作に失敗しました。求めた結果が得られませんでした。"
					: "処理が完了しました。";
		await addMessageLog(userId, botId, "assistant", replyText, undefined, source);
		return { text: replyText, embeds: ctx.embeds, files: ctx.files };
	} catch (error) {
		if (error instanceof Error && error.message.includes("API Key")) {
			return { text: `⚠️ ${error.message}`, embeds: [], files: [] };
		}
		if (isRateLimitError(error)) {
			console.error("Gemini API レート制限:", error);
			return {
				text: "⚠️ 現在APIの利用制限（トークン枯渇など）に達しています。しばらく待ってからもう一度お試しください。",
				embeds: [],
				files: [],
			};
		}
		if (isServerError(error)) {
			console.error("Gemini API サーバーエラー:", error);
			return {
				text: "⚠️ AIサーバーが現在混み合っているか、一時的なエラーが発生しています（503等）。しばらく待ってからもう一度お試しください。",
				embeds: [],
				files: [],
			};
		}
		console.error("Gemini API エラー:", error);
		throw error;
	}
}

// ─── 汎用モード（MCPアシスタント）のメッセージ処理 ───────────────────────────
// bot_attributes_requirements.md §4.3: ギルド常駐の簡易Bot（MCP + ペルソナ + メモリのみ）。
// LLM呼び出しは Bot専用キー（getBotGenAI）で実行し、発話ユーザーの個人キーは使用しない。

/** ギルド会話の発話者情報（メンバー制 §4.3.3: Webアカウント未登録のDiscordユーザーも可） */
export interface GuildSpeaker {
	userId: string;
	displayName: string;
}

/** 履歴・返信チェーン・添付を Gemini Contents 形式へ組み立てる（汎用モード共通） */
function buildContentsFromHistory(
	history: ContextEntry[],
	message: ChatMessage,
	replyChainText?: string,
): Content[] {
	const contents: Content[] = [];

	if (replyChainText) {
		contents.push({
			role: "user",
			parts: [
				{
					text: `[返信チェーン（このメッセージは以下のやり取りへの返信です。古い順）]\n${replyChainText}\n[返信チェーンここまで]`,
				},
			],
		});
		contents.push({
			role: "model",
			parts: [{ text: "（返信チェーンの文脈を把握しました）" }],
		});
	}

	// 履歴をGeminiのContents形式へ変換（同ロール連続は結合して交互にする）
	for (const entry of history) {
		const role = entry.role === "assistant" ? "model" : "user";
		const last = contents[contents.length - 1];
		if (last && last.role === role) {
			const lastPart = last.parts[0];
			if ("text" in lastPart) {
				lastPart.text += "\n" + entry.content;
			}
		} else {
			contents.push({ role, parts: [{ text: entry.content }] });
		}
	}

	if (contents.length === 0) {
		contents.push({ role: "user", parts: [{ text: message.text || "" }] });
	}

	// 最新メッセージの画像・音声を直近のユーザーコンテンツへ添付
	const inlineParts: Part[] = [];
	if (message.imageData) {
		inlineParts.push({
			inlineData: {
				data: message.imageData.data,
				mimeType: message.imageData.mimeType,
			},
		});
	}
	if (message.audioData) {
		inlineParts.push({
			inlineData: {
				data: message.audioData.data,
				mimeType: message.audioData.mimeType,
			},
		});
	}
	if (inlineParts.length > 0) {
		const lastContent = contents[contents.length - 1];
		if (lastContent && lastContent.role === "user") {
			lastContent.parts.push(...inlineParts);
		} else {
			contents.push({ role: "user", parts: [{ text: "" }, ...inlineParts] });
		}
	}

	return contents;
}

/**
 * 汎用モードのシステムプロンプトを構築する。
 * 注入順は要件 §4.6.2 のとおり: ペルソナ → 共有ノート → 発話者の個人ノート。
 */
function buildGuildSystemInstruction(
	bot: BotRecord,
	scope: "guild" | "dm",
	guildId: string | null,
	speaker: GuildSpeaker,
): string {
	const now = new Date();
	const year = now.getFullYear();
	const month = now.getMonth() + 1;
	const date = now.getDate();
	const hours = String(now.getHours()).padStart(2, "0");
	const minutes = String(now.getMinutes()).padStart(2, "0");
	const seconds = String(now.getSeconds()).padStart(2, "0");
	const dayOfWeek = ["日", "月", "火", "水", "木", "金", "土"][now.getDay()];
	const dateTimeStr = `${year}年${month}月${date}日 (${dayOfWeek}) ${hours}時${minutes}分${seconds}秒`;

	// ペルソナ（要件 §4.4: Bot単位ペルソナ。未設定・削除済みは既存デフォルトへフォールバック）
	let personaSection = DEFAULT_PERSONA;
	if (bot.persona_id) {
		try {
			const persona = getPersonaById(bot.persona_id);
			if (persona?.prompt?.trim()) {
				personaSection = `# あなたの役割・キャラクター設定（ペルソナ）\n${persona.prompt.trim()}`;
			}
		} catch (err) {
			console.error("Bot単位ペルソナの取得に失敗しました:", err);
		}
	}

	const modeSection =
		scope === "guild"
			? `
# あなたの動作モード（サーバー常駐アシスタント）
あなたはDiscordサーバーに常駐し、登録された利用メンバーをサポートするアシスタントBotです。
- 会話のコンテキストはこのサーバーの利用メンバー全員で共有されています。過去の発言には「[名前]: 」の形式で発話者名が付いています。
- 返信は今あなたにメンションした発話者に向けて行ってください。
- 接続されたMCP拡張ツール・共有ノート・個人ノート・過去会話の検索を活用して、サーバー運用を支援してください。
- タスク管理・家計簿・ブラウザ操作などの秘書機能はこのBotにはありません。求められた場合は、その機能を持たないことを丁寧に伝えてください。

# 利用メンバー管理のルール
- メンバーから「@xx を追加して」と依頼されたら addBotMember で新しい利用メンバーを追加できます（メンバーなら誰でも依頼可能）。
- メンバーの削除は「本人による自己削除（私を外して）」と「Bot作成者」のみが行えます。他人の削除依頼には応じないでください。
- 「誰が使えるの？」には listBotMembers で答えてください。

# 記憶（ノート）の使い分けルール（重要）
- **個人ノート（appendMyNote）**: 発話者本人に関する長期的な情報。本人との会話でのみ参照されます。
- **共有ノート（appendGuildNote）**: サーバー全体で共有すべき知識（ルール・用語・運用手順）。メンバー全員との会話で参照されます。
- どちらに保存すべきか曖昧な場合は発話者に確認してください。`
			: `
# あなたの動作モード（owner との動作確認DM）
あなたはDiscordサーバー常駐型のアシスタントBotで、現在はBot作成者（owner）とのダイレクトメッセージで動作確認・管理用途の会話をしています。
- このDMの会話コンテキストはサーバーでの会話とは分離されています。
- 接続されたMCP拡張ツールと個人ノートを利用できます。サーバー（ギルド）スコープの機能（共有ノート・メンバー管理・会話検索）はDMでは利用できません。
- タスク管理・家計簿・ブラウザ操作などの秘書機能はこのBotにはありません。`;

	const richReplySection = `
# リッチ返信の使い分け
返信の性質に応じてプレーンテキストとリッチ形式を使い分けてください。
- 単純な一問一答 → プレーンテキスト
- データの一覧・サマリ・手順の整理 → showRichContent（Embed）
- エラー・警告の通知 → showRichContent（colorに error / warning を指定）
リッチ形式を使った場合も、本文テキストで要点を簡潔に添えてください。`;

	const systemRuleSection = `
# 重要なシステムルール
- 現在の日時: ${dateTimeStr}
- 「先週」「昨日」などの相対的な日時表現は、上記の現在日時を基準に正確に解釈してください。
- 確認必須（要承認）と明記された外部ツール（MCP拡張）は、実行内容（ツール名・引数）を発話者へ提示して承認を得てから呼び出してください。
- 不確かな情報を事実のように伝えないでください。ツール実行に失敗した場合や求められた結果が得られなかった場合は、正常終了したと誤解させる応答をせず、必ず失敗したことと理由を明記してください。
- **【最重要】未実行の完了報告の禁止:** 操作の完了報告（「登録しました」「追加しました」「設定しました」「やっておきました」等）は、このターンで実際に対応するツール（関数）を呼び出し、その実行結果を受け取った場合に限り行ってください。ツールを呼ばずに完了したかのように装うことは固く禁止します。操作を行うなら必ずその場で関数を呼び出してください。
- 機能に関係ない雑談にもペルソナ設定に沿って自然に応じてください。`;

	// 共有ノート（要件 §4.6.2: ギルド会話のみ。ペルソナの後に注入）
	let guildNoteSection = "";
	if (scope === "guild" && guildId) {
		try {
			const note = getBotGuildNote(bot.id, guildId);
			if (note.trim()) {
				guildNoteSection = `\n# 共有ノート（このサーバーの利用メンバー全員と共有している知識）\nサーバーのルール・用語・運用手順などの共有知識です。会話・判断の際に常に考慮してください。\n${note.trim()}`;
			}
		} catch (err) {
			console.error("共有ノートの取得に失敗しました:", err);
		}
	}

	// 発話者の個人ノート（要件 §4.6.2: 本人のプロンプトにのみ注入。他メンバーには注入しない）
	let personalNoteSection = "";
	try {
		const note = getBotUserNote(bot.id, speaker.userId);
		if (note.trim()) {
			personalNoteSection = `\n# 発話者の個人ノート（${speaker.displayName} さん専用の記憶）\n以下は現在の発話者本人に関する情報です。他のメンバーには開示しないでください。\n${note.trim()}`;
		}
	} catch (err) {
		console.error("個人ノートの取得に失敗しました:", err);
	}

	const speakerSection = `
# 現在の発話者
- 名前: ${speaker.displayName}
- メンション表記: <@${speaker.userId}>`;

	const parts = [
		personaSection,
		modeSection,
		richReplySection,
		systemRuleSection,
		guildNoteSection,
		personalNoteSection,
		speakerSection,
	];

	return parts.filter((p) => p !== "").join("\n");
}

/** 汎用モード共通のLLMエラー → ユーザー向けメッセージ変換 */
function guildErrorResult(error: unknown): ProcessResult {
	if (isRateLimitError(error)) {
		console.error("Gemini API レート制限 (Bot専用キー):", error);
		return {
			text: "⚠️ 現在APIの利用制限に達しています。しばらく待ってからもう一度お試しください。",
			embeds: [],
			files: [],
		};
	}
	if (isServerError(error)) {
		console.error("Gemini API サーバーエラー (Bot専用キー):", error);
		return {
			text: "⚠️ AIサーバーが現在混み合っているか、一時的なエラーが発生しています。しばらく待ってからもう一度お試しください。",
			embeds: [],
			files: [],
		};
	}
	console.error("Gemini API エラー (汎用モード):", error);
	return {
		text: "申し訳ございません、処理中にエラーが発生しました 😢\nしばらくしてからもう一度お試しください。",
		embeds: [],
		files: [],
	};
}

/**
 * 許可ギルド内の利用メンバーからのメッセージを処理する（汎用モード本体）。
 * 呼び出し前提（bot.ts が検証済み）: ギルド許可リスト・利用メンバー判定・レート制限・Bot専用キーの存在。
 */
export async function processGuildMessage(
	botId: string,
	guildId: string,
	speaker: GuildSpeaker,
	message: ChatMessage,
	onStatusChange?: (status: "thinking" | "writing" | "idle") => void,
): Promise<ProcessResult> {
	const bot = getBotById(botId);
	if (!bot) {
		return { text: "", embeds: [], files: [] };
	}

	const ctx: ToolContext = {
		botId,
		userId: speaker.userId,
		guildId,
		embeds: [],
		files: [],
		richReplyEnabled: true, // ギルド利用メンバーはユーザー設定を持たないため常に有効
	};

	// 1. 発話をギルドコンテキストへ記録（発話者を「[名前]: 」プレフィックスで区別 §4.6.1）
	const logText =
		message.text?.trim() ||
		(message.audioData
			? "[音声メッセージ]"
			: message.imageData
				? "[画像]"
				: "");
	if (logText) {
		await addGuildMessageLog(
			botId,
			guildId,
			speaker.userId,
			"user",
			`[${speaker.displayName}]: ${logText}`,
			message.discordMsgId,
			message.replyToMsgId,
		);
	}

	// 2. ギルドコンテキスト（直近30件）+ 返信チェーン（bot × guild スコープ）
	const history = await getGuildContext(botId, guildId, GUILD_CONTEXT_LIMIT);

	let replyChainText: string | undefined;
	if (message.replyToMsgId) {
		try {
			const chain = resolveGuildReplyChain(
				botId,
				guildId,
				message.replyToMsgId,
				config.replyChainMaxDepth,
			);
			if (chain.length > 0) {
				replyChainText = chain
					.map((m) => (m.role === "user" ? m.content : `あなた: ${m.content}`))
					.join("\n");
			}
		} catch (err) {
			console.error("ギルド返信チェーンの解決に失敗しました:", err);
		}
	}

	const contents = buildContentsFromHistory(history, message, replyChainText);

	// 3. Function レジストリ（汎用モード: core + memory の静的モジュール + Bot紐付けMCP §4.3.1）
	const caps = resolveBotCapabilities(botId);
	let mcpModule: FunctionModule = { declarations: [], handlers: {} };
	if (caps.has("mcp")) {
		try {
			// ギルド経路: 当該Botに利用許可されたMCPサーバーのみ（単一owner Botのため owner設定分をそのまま使う）。
			mcpModule = await getMcpFunctionModuleForBot(botId, speaker.userId);
		} catch (err) {
			console.error(
				"Bot紐付けMCP動的ツールの取得に失敗しました（スキップ）:",
				err,
			);
		}
	}
	const registry = buildFunctionRegistry([
		...getGuildAssistantFunctionModules("guild", caps),
		mcpModule,
	]);

	const systemInstruction = buildGuildSystemInstruction(
		bot,
		"guild",
		guildId,
		speaker,
	);

	try {
		// Bot専用キーで実行（要件 §4.3.3: 本人キーは使用しない。未設定はbot.tsが事前に弾く）
		const ai = getBotGenAI(botId);
		if (!ai) {
			console.warn(
				`[汎用モード] Bot ${botId} のGemini APIキーが未設定のため応答できません。`,
			);
			return { text: "", embeds: [], files: [] };
		}

		const { text } = await runFunctionCallingLoop(
			ai,
			systemInstruction,
			registry,
			contents,
			ctx,
			onStatusChange,
		);

		// 空応答時のフォールバック定型文も必ずログへ保存する（assistant の記録欠落により
		// ユーザー要求が次ターンで「未応答」扱いとなり、直前操作が再実行されるのを防ぐ）。
		const replyText = text && text.trim() ? text : "処理が完了しました。";
		await addGuildMessageLog(
			botId,
			guildId,
			speaker.userId,
			"assistant",
			replyText,
		);
		return { text: replyText, embeds: ctx.embeds, files: ctx.files };
	} catch (error) {
		return guildErrorResult(error);
	}
}

/**
 * owner との動作確認DM を処理する（要件 §4.3.2: DMは owner のみ・専用コンテキストで分離）。
 */
export async function processBotDmMessage(
	botId: string,
	owner: GuildSpeaker,
	message: ChatMessage,
	onStatusChange?: (status: "thinking" | "writing" | "idle") => void,
): Promise<ProcessResult> {
	const bot = getBotById(botId);
	if (!bot) {
		return { text: "", embeds: [], files: [] };
	}

	const ctx: ToolContext = {
		botId,
		userId: owner.userId,
		embeds: [],
		files: [],
		richReplyEnabled: true,
	};

	const logText =
		message.text?.trim() ||
		(message.audioData
			? "[音声メッセージ]"
			: message.imageData
				? "[画像]"
				: "");
	if (logText) {
		await addBotDmMessageLog(
			botId,
			owner.userId,
			"user",
			logText,
			message.discordMsgId,
			message.replyToMsgId,
		);
	}

	const history = await getBotDmContext(botId, owner.userId);

	let replyChainText: string | undefined;
	if (message.replyToMsgId) {
		try {
			const chain = resolveReplyChain(
				owner.userId,
				message.replyToMsgId,
				config.replyChainMaxDepth,
			);
			if (chain.length > 0) {
				replyChainText = chain
					.map(
						(m) => `${m.role === "user" ? "ユーザー" : "あなた"}: ${m.content}`,
					)
					.join("\n");
			}
		} catch (err) {
			console.error("owner DM 返信チェーンの解決に失敗しました:", err);
		}
	}

	const contents = buildContentsFromHistory(history, message, replyChainText);

	const caps = resolveBotCapabilities(botId);
	let mcpModule: FunctionModule = { declarations: [], handlers: {} };
	if (caps.has("mcp")) {
		try {
			// owner DM: 当該Botに利用許可されたMCPサーバーのみ（単一owner Botのため owner設定分をそのまま使う）。
			mcpModule = await getMcpFunctionModuleForBot(botId, owner.userId);
		} catch (err) {
			console.error(
				"Bot紐付けMCP動的ツールの取得に失敗しました（スキップ）:",
				err,
			);
		}
	}
	const registry = buildFunctionRegistry([
		...getGuildAssistantFunctionModules("dm", caps),
		mcpModule,
	]);

	const systemInstruction = buildGuildSystemInstruction(bot, "dm", null, owner);

	try {
		const ai = getBotGenAI(botId);
		if (!ai) {
			// owner への動作確認DMでは未設定理由を明示する（管理用途のため）
			return {
				text: "⚠️ このBotにはBot専用のGemini APIキーが設定されていません。管理画面の「Bot設定」→「汎用モード設定」から設定してください。",
				embeds: [],
				files: [],
			};
		}

		const { text } = await runFunctionCallingLoop(
			ai,
			systemInstruction,
			registry,
			contents,
			ctx,
			onStatusChange,
		);

		// 空応答時のフォールバック定型文も必ずログへ保存する（assistant の記録欠落により
		// ユーザー要求が次ターンで「未応答」扱いとなり、直前操作が再実行されるのを防ぐ）。
		const replyText = text && text.trim() ? text : "処理が完了しました。";
		await addBotDmMessageLog(botId, owner.userId, "assistant", replyText);
		return { text: replyText, embeds: ctx.embeds, files: ctx.files };
	} catch (error) {
		return guildErrorResult(error);
	}
}
