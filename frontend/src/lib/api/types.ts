import type { BudgetLimit as WireBudgetLimit } from "./generated/BudgetLimit";
import type { BudgetLimitListData } from "./generated/BudgetLimitListData";
import type { CategoryTotal as WireCategoryTotal } from "./generated/CategoryTotal";
import type { ClipboardEntry as WireClipboardEntry } from "./generated/ClipboardEntry";
import type { ClipboardListData } from "./generated/ClipboardListData";
import type { Contact as WireContactView } from "./generated/Contact";
import type { ContactListData } from "./generated/ContactListData";
import type { ContextNoteData } from "./generated/ContextNoteData";
import type { Credential as WireCredentialView } from "./generated/Credential";
import type { CredentialListData } from "./generated/CredentialListData";
import type { DayPlanBlock as WireDayPlanBlock } from "./generated/DayPlanBlock";
import type { Envelope } from "./generated/Envelope";
import type { Expense as WireExpenseRecord } from "./generated/Expense";
import type { ExpenseListData } from "./generated/ExpenseListData";
import type { MeData } from "./generated/MeData";
import type { MonthlyTrendPoint as WireMonthlyTrendPoint } from "./generated/MonthlyTrendPoint";
import type { Persona as WirePersonaRecord } from "./generated/Persona";
import type { PersonaListData } from "./generated/PersonaListData";
import type { PlannedPayment as WirePlannedPayment } from "./generated/PlannedPayment";
import type { PlannedPaymentListData } from "./generated/PlannedPaymentListData";
import type { Playbook as WirePlaybookRecord } from "./generated/Playbook";
import type { PlaybookListData } from "./generated/PlaybookListData";
import type { PlaybookRun as WirePlaybookRunRecord } from "./generated/PlaybookRun";
import type { PlaybookSchedule as WirePlaybookScheduleRecord } from "./generated/PlaybookSchedule";
import type { Reminder as WireReminderRecord } from "./generated/Reminder";
import type { ReminderListData } from "./generated/ReminderListData";
import type { RunListData } from "./generated/RunListData";
import type { Schedule as WireScheduleRecord } from "./generated/Schedule";
import type { ScheduleListData } from "./generated/ScheduleListData";
import type { SessionUser as WireSessionUser } from "./generated/SessionUser";
import type { TaskData } from "./generated/TaskData";
import type { TaskDetailData } from "./generated/TaskDetailData";
import type { TaskListData } from "./generated/TaskListData";
import type { TaskProgressLog as WireTaskProgressLogRecord } from "./generated/TaskProgressLog";
import type { TimelineDayData } from "./generated/TimelineDayData";
import type { TimelineNeighborsData } from "./generated/TimelineNeighborsData";
import type { TimelineRecord as WireTimelineRecord } from "./generated/TimelineRecord";
import type { Todo as WireTodoRecord } from "./generated/Todo";
import type { TodoWithSubtasks as WireTodoWithSubtasks } from "./generated/TodoWithSubtasks";
import type { JsonWire } from "./wire";

// ─────────────────────────────────────────────────────────────────────────────
// 共通 API 型定義（型付き API クライアント層）
//
// - サーバの大半のレスポンスは { success: boolean; message?: string } & payload
//   の「トップレベル直置き」形状（`data` ラッパは無い）。→ ApiResponse<T> で表現。
// - Rust wire DTO（ts-rs 生成型・generated/）がある同一契約は生成型を使う。i64 の
//   bigint → number の変換だけを JsonWire<T>（wire.ts）で行う。Rust DTO が未生成の
//   管理用契約は手書きで維持する。
// - PWA の camelCase / string ID 契約は別 wire 契約。client/pwa/src/api/contracts.ts
//   で独立管理し、管理画面の生成型を PWA から直接 import しない（wire.ts 参照）。
// ─────────────────────────────────────────────────────────────────────────────

/** 共通エンベロープ（Rust `Envelope<T>` の生成型）。ペイロードはトップレベル直置き。 */
export type ApiResponse<T = unknown> = Envelope<T>;
export type SessionUser = WireSessionUser;

/** GET /api/me。法的 URL は未設定でも空文字で常に返る。 */
export type MeResponse = ApiResponse<MeData>;

// ─── bots ────────────────────────────────────────────────────────────────────

/** botViewSchema（src/types/apiViews.ts）。機密トークン/APIキー暗号文は含まれない。 */
export interface BotView {
	id: string;
	user_id: string;
	name: string;
	recommended_persona_id: number | null;
	persona_id: number | null;
	capabilities: string;
	discord_username: string | null;
	discord_avatar_url: string | null;
	discord_application_id: string | null;
	suspended: number;
	created_at: string;
	updated_at: string;
	preset: string;
	preset_display_name: string;
	has_gemini_key: boolean;
	has_token: boolean;
	running: boolean;
	connected: boolean;
	shared: boolean;
}

export type BotListResponse = ApiResponse<{ bots: BotView[] }>;
export type BotResponse = ApiResponse<{ bot: BotView }>;

export interface PresetOption {
	preset: string;
	display_name: string;
	description?: string;
}
export type PresetsResponse = ApiResponse<{ presets: PresetOption[] }>;

export interface BotShareView {
	bot_id: string;
	shared_user_id: string;
	shared_username?: string | null;
	status: string;
	created_at?: string;
}
export type BotSharesResponse = ApiResponse<{ shares: BotShareView[] }>;

/** GET /api/bots/usage（呼び出し側が手動で botId を query 付与する: scope:'user'） */
export type BotUsageResponse = ApiResponse<{
	usage?: Record<string, unknown>;
}>;

// ─── tasks（todoRepo） ────────────────────────────────────────────────────────

export type TodoPriority = "high" | "medium" | "low";

export type TodoRecord = JsonWire<WireTodoRecord>;

export type TodoWithSubtasks = JsonWire<WireTodoWithSubtasks>;

export type TaskProgressLogRecord = JsonWire<WireTaskProgressLogRecord>;

/** GET /api/tasks・/api/tasks/someday（親タスクのみ・subtasks ネスト付き）。 */
export type TasksResponse = ApiResponse<JsonWire<TaskListData>>;
export type TaskDetailResponse = ApiResponse<JsonWire<TaskDetailData>>;
export type TaskResponse = ApiResponse<JsonWire<TaskData>>;
export type TaskGanttResponse = ApiResponse<JsonWire<TaskListData>>;

// ─── expenses（expenseRepo / plannedPaymentRepo） ─────────────────────────────

export type ExpenseType = "income" | "expense";

export type ExpenseRecord = JsonWire<WireExpenseRecord>;

export type CategoryTotal = JsonWire<WireCategoryTotal>;

export type MonthlyTrendPoint = JsonWire<WireMonthlyTrendPoint>;

export type BudgetLimit = JsonWire<WireBudgetLimit>;

export type PlannedPaymentStatus = "pending" | "settled" | "cancelled";

export type PlannedPayment = JsonWire<WirePlannedPayment>;

/** GET /api/expenses（一覧＋指定月の支出合計 total・収入合計 incomeTotal・内訳・6 ヶ月推移）。 */
export type ExpensesResponse = ApiResponse<JsonWire<ExpenseListData>>;
export type BudgetLimitsResponse = ApiResponse<JsonWire<BudgetLimitListData>>;
export type PlannedPaymentsResponse = ApiResponse<
	JsonWire<PlannedPaymentListData>
>;

// ─── schedules（scheduleRepo） ───────────────────────────────────────────────

export type ScheduleRecord = JsonWire<WireScheduleRecord>;
export type SchedulesResponse = ApiResponse<JsonWire<ScheduleListData>>;

// ─── reminders（reminderRepo） ───────────────────────────────────────────────

export type ReminderTargetType = "dm" | "channel";
export type ReminderStatus = "pending" | "sent" | "cancelled";

export type ReminderRecord = JsonWire<WireReminderRecord>;
export type RemindersResponse = ApiResponse<JsonWire<ReminderListData>>;

// ─── timeline（timelineRepo） ────────────────────────────────────────────────

export type PlanBlockType = "task" | "transit" | "event" | "free";
export type RecordType =
	| "memo"
	| "expense"
	| "task_done"
	| "media"
	| "location";

export type DayPlanBlock = JsonWire<WireDayPlanBlock>;

export type TimelineRecord = JsonWire<WireTimelineRecord>;

// 注: サーバ（GET /api/timeline/day）は `blocks` キーで返す（`plan` ではない）。
export type TimelineDayResponse = ApiResponse<JsonWire<TimelineDayData>>;

/** GET /api/timeline/neighbors（前後で計画か記録のある最寄りの日付。無い方向は null） */
export type TimelineNeighborsResponse = ApiResponse<
	JsonWire<TimelineNeighborsData>
>;

// ─── personal（contextNote / clipboard / contacts） ──────────────────────────

export type ContactView = JsonWire<WireContactView>;
export type ContactsResponse = ApiResponse<JsonWire<ContactListData>>;

export type ClipboardEntry = JsonWire<WireClipboardEntry>;
export type ClipboardResponse = ApiResponse<JsonWire<ClipboardListData>>;

/** GET /api/context-note（content + max_length を返す。未登録時 updated_at は null） */
export type ContextNoteResponse = ApiResponse<JsonWire<ContextNoteData>>;

// ─── personas（personaRepo） ─────────────────────────────────────────────────

export type PersonaRecord = JsonWire<WirePersonaRecord>;

export interface PublicPersonaView {
	id: number;
	name: string;
	/** 全文（一覧側で文字数と冒頭を出す）。 */
	prompt: string;
	owner_username: string;
	updated_at: string;
}

/** GET /api/personas — personas + 適用中ID + 最大文字数。 */
export type PersonasResponse = ApiResponse<JsonWire<PersonaListData>>;
export type PersonaMarketplaceResponse = ApiResponse<{
	personas: PublicPersonaView[];
}>;
/** GET /api/personas/marketplace/:id — 公開ペルソナの全文（インポート判断用）。 */
export type PersonaMarketplaceDetailResponse = ApiResponse<{
	persona: { id: number; name: string; prompt: string };
}>;

// ─── playbooks（playbookRepo） ───────────────────────────────────────────────

/** playbookService.Playbook（name が識別子。id 列は持たない）。 */
export type PlaybookRecord = JsonWire<WirePlaybookRecord>;
/** playbookScheduleService.PlaybookSchedule。 */
export type PlaybookScheduleRecord = JsonWire<WirePlaybookScheduleRecord>;
/** playbookScheduleService.PlaybookRun。 */
export type PlaybookRunRecord = JsonWire<WirePlaybookRunRecord>;
export type PlaybooksResponse = ApiResponse<JsonWire<PlaybookListData>>;
/**
 * GET /api/playbooks/schedules。Rust 側の `ScheduleListData` は予定ドメインと同名で、
 * 生成時に予定側が先に書き出されるため playbook 版は生成されない。手書きで維持する。
 */
export type PlaybookSchedulesResponse = ApiResponse<{
	schedules: PlaybookScheduleRecord[];
}>;
export type PlaybookRunsResponse = ApiResponse<JsonWire<RunListData>>;

// ─── webhooks（webhookRepo） ─────────────────────────────────────────────────

export interface WebhookEndpointView {
	id: number;
	/** 帰属 Bot（受信で作る通知・ToDo・リマインドの Bot）。帰属先が無い旧エンドポイントは null。 */
	bot_id: string | null;
	name: string;
	token: string;
	has_secret: boolean;
	notify_target_type: "dm" | "channel";
	notify_target_id: string | null;
	template: string | null;
	filter_keyword: string | null;
	create_todo: boolean;
	create_reminder: boolean;
	enabled: boolean;
	created_at: string;
	/** buildHookUrl で付与される完全な受信URL（一覧・作成・更新応答に同梱）。 */
	url: string;
}
export interface WebhookDeliveryRecord {
	id: number;
	endpoint_id: number;
	payload: string;
	status: "received" | "notified" | "filtered" | "failed";
	detail: string | null;
	created_at: string;
}
/** GET /api/webhooks — サーバは `endpoints` キーで返す。 */
export type WebhooksResponse = ApiResponse<{
	endpoints: WebhookEndpointView[];
}>;
export type WebhookCreateResponse = ApiResponse<{
	endpoint: WebhookEndpointView;
}>;
export type WebhookDeliveriesResponse = ApiResponse<{
	deliveries: WebhookDeliveryRecord[];
}>;

// ─── mcp（mcpRepo） ──────────────────────────────────────────────────────────

export interface McpToolDef {
	name: string;
	description?: string;
	inputSchema?: unknown;
}
/** GET /api/mcp-servers の toSafeView 出力（mcpRoutes.ts）。 */
export interface McpServerView {
	id: number;
	scope: "system" | "user";
	name: string;
	endpoint_url: string;
	has_auth: boolean;
	tools: { name: string; description: string }[];
	tools_cache_updated: string | null;
	requires_confirmation: boolean;
	enabled: boolean;
	created_at: string;
	/** 許可済み Bot（一覧ハンドラで付与） */
	granted_bot_ids?: string[];
}
export type McpServersResponse = ApiResponse<{ servers: McpServerView[] }>;

// ─── credentials（credentialRepo） ───────────────────────────────────────────

export type CredentialView = JsonWire<WireCredentialView>;
export type CredentialsResponse = ApiResponse<JsonWire<CredentialListData>>;

// ─── 定期報告（/api/periodic-reports） ───────────────────────────────────────

/** 定期報告に載せる内容の区分（periodic_reports.sections の要素） */
export type ReportSection =
	| "weather"
	| "news"
	| "tasks"
	| "schedules"
	| "payments"
	| "finance"
	| "topics";

/** 登録済みの定期報告 */
export interface PeriodicReport {
	id: number;
	name: string;
	enabled: boolean;
	schedule_cron: string;
	target_type: "dm" | "channel";
	target_id: string | null;
	/** 載せる内容（表示順） */
	sections: ReportSection[];
	location_name: string | null;
	weather_lat: number | null;
	weather_lng: number | null;
	news_feeds: string[];
	news_keywords: string[];
	/** 非公開の内容（タスク・予定・支払い予定・家計）を含む＝DM にだけ送る */
	private: boolean;
}

/** 登録・更新で送る内容（更新は id と変えるキーだけでよい） */
export type PeriodicReportInput = Partial<
	Omit<PeriodicReport, "id" | "private">
>;

export type PeriodicReportsResponse = ApiResponse<{
	reports: PeriodicReport[];
}>;
export type PeriodicReportResponse = ApiResponse<{ report: PeriodicReport }>;

// ─── settings（userRepo / status / discord / google） ────────────────────────

export type StatusResponse = ApiResponse<{ [k: string]: unknown }>;
export type DiscordSettingsResponse = ApiResponse<{ [k: string]: unknown }>;
export type GoogleOAuthUrlResponse = ApiResponse<{ url: string }>;

// ─── integrated（integratedRoutes: /api/integrated/overview） ─────────────────

/** 統合オーバービューの Bot 行（overview handler の bots[] を写す・所有 Bot のみ。システム Bot は含まない） */
export interface IntegratedBotView {
	id: string;
	name: string;
	preset: string;
	suspended: boolean;
	stopped: boolean;
	has_token: boolean;
	running: boolean;
	connected: boolean;
	discord_username: string | null;
	discord_avatar_url: string | null;
	/** owner 本人が許可した MCP サーバ ID */
	granted_mcp_ids: number[];
	/** owner 本人が許可した認証情報サービス名 */
	granted_credentials: string[];
	/** "primary" | "none" | account id (number) */
	google_setting: string | number;
}
/** 統合オーバービューの MCP サーバ行 */
export interface IntegratedMcpServerView {
	id: number;
	name: string;
	endpoint_url: string;
	enabled: boolean;
	has_auth: boolean;
	tools: number;
}
/** 統合オーバービューの認証情報行 */
export interface IntegratedCredentialView {
	service_name: string;
	username: string;
	url: string | null;
	updated_at: string | null;
}
/** 統合オーバービューの Google アカウント行 */
export interface IntegratedGoogleAccountView {
	id: number;
	email: string | null;
	is_primary: boolean;
	calendars: string[];
	[k: string]: unknown;
}
export type IntegratedOverviewResponse = ApiResponse<{
	bots: IntegratedBotView[];
	mcpServers: IntegratedMcpServerView[];
	credentials: IntegratedCredentialView[];
	googleAccounts: IntegratedGoogleAccountView[];
}>;
/** カレンダー一覧（GET /api/integrated/google/accounts/:id/calendars） */
export interface GoogleCalendarView {
	id: string;
	summary?: string;
	[k: string]: unknown;
}
export type GoogleCalendarsResponse = ApiResponse<{
	calendars: GoogleCalendarView[];
}>;

// ─── admin（adminRoutes） ────────────────────────────────────────────────────

export interface AdminUserView {
	discord_id: string;
	username: string;
	role: string;
	created_at: string;
	updated_at: string;
}
/** GET /api/admin/stats の stats フィールド */
export interface AdminStats {
	totalUsers: number;
	totalBots: number;
	suspendedBots: number;
}
/** GET /api/admin/bots が返す Bot 行（BotView とは別形状） */
export interface AdminBotView {
	id: string;
	name: string;
	user_id: string;
	owner_username: string;
	discord_username: string | null;
	discord_avatar_url: string | null;
	suspended: number;
	hasCustomToken: boolean;
	isRunning: boolean;
	created_at: string;
	updated_at: string;
}
export interface AuditLogEntry {
	id: number;
	user_id?: string;
	action: string;
	target?: string | null;
	detail?: string | null;
	created_at: string;
	[k: string]: unknown;
}
/** GET /api/admin/bot-attribute-settings */
export interface AdminPresetSetting {
	id: string;
	displayName: string;
	[k: string]: unknown;
}
export interface AdminRateLimits {
	userPerMinute?: number;
	userPerDay?: number;
	guildPerDay?: number;
	[k: string]: unknown;
}
export type AdminBotAttributeSettingsResponse = ApiResponse<{
	presets: AdminPresetSetting[];
	rate_limits: AdminRateLimits;
}>;
// Admin のペルソナ管理一覧は GET /api/personas/marketplace（personaApi.marketplace）を
// 流用し、非公開化・削除のみ adminApi を叩く。専用の一覧型は設けない（PublicPersonaView 再利用）。
export type AdminStatsResponse = ApiResponse<{ stats: AdminStats }>;
export type AdminUsersResponse = ApiResponse<{ users: AdminUserView[] }>;
export type AdminBotsResponse = ApiResponse<{ bots: AdminBotView[] }>;
export type AdminAuditLogsResponse = ApiResponse<{
	logs: AuditLogEntry[];
	total: number;
}>;
export type AdminSystemSettingsResponse = ApiResponse<{
	privacyPolicyUrl: string;
	termsUrl: string;
}>;

// ─── devices（deviceMgmtRoutes） ─────────────────────────────────────────────

export interface DeviceRecord {
	id: string;
	name: string;
	created_at: string;
	last_used_at?: string | null;
}
export type DevicesResponse = ApiResponse<{ devices: DeviceRecord[] }>;

// ─── device auth (OAuth device flow) ─────────────────────────────────────────

/** POST /api/auth/device/code のレスポンス（OAuth device authorization 形状） */
export interface DeviceCodeResponse {
	device_code: string;
	user_code: string;
	verification_uri: string;
	verification_uri_complete?: string;
	expires_in: number;
	interval: number;
}

/** deviceApi.pollToken の戻り（§10.4。汎用 request を通さない専用形状） */
export type PollResult =
	| { status: "authorized"; token: string }
	| { status: "pending"; slowDown: boolean }
	| { status: "error"; code: string };
