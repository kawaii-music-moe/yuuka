// ─────────────────────────────────────────────────────────────────────────────
// 全体管理ページ（統合管理 / アカウント管理 / 管理者設定）のセクション定義。
//
// 各ページはハブ（/integrated 等）で下記セクションを縦並びリストで表示し、
// 行を選ぶと個別ページ（/integrated/<key> 等）へ遷移する。
// ManagementOverlayShell（ハブ描画・パンくず）と各 Overlay（{#if section}）が
// このキーを単一情報源として共有する。
// ─────────────────────────────────────────────────────────────────────────────

export interface ManagementSection {
	/** URL サブパス（/<base>/<key>） */
	key: string;
	label: string;
	/** ハブの行に出す一行説明 */
	desc: string;
}

export const INTEGRATED_SECTIONS: ManagementSection[] = [
	{
		key: "health",
		label: "Bot ヘルス / 起動・停止",
		desc: "Botの稼働状況の確認と起動・停止",
	},
	{
		key: "credentials",
		label: "認証情報（パスワードマネージャ）",
		desc: "パスワード等の登録とBot別の利用許可",
	},
	{
		key: "mcp",
		label: "MCPサーバー",
		desc: "外部MCPサーバーの登録とBot別の利用許可",
	},
	{
		key: "google",
		label: "Googleアカウント連携",
		desc: "Googleアカウントの連携（複数可）とカレンダー設定",
	},
];

export const ACCOUNT_SECTIONS: ManagementSection[] = [
	{
		key: "gemini",
		label: "Gemini AI 個別設定",
		desc: "秘書モードで使うあなた個人のAPIキーとモデル",
	},
	{ key: "profile", label: "表示名（プロフィール）", desc: "ご自身の表示名" },
	{ key: "theme", label: "テーマ設定", desc: "管理画面の外観テーマ" },
	{
		key: "password",
		label: "パスワード変更",
		desc: "管理画面ログイン用パスワードの変更",
	},
	{
		key: "delete",
		label: "アカウントの削除",
		desc: "アカウントと関連データの削除（取り消し不可）",
	},
];

export const ADMIN_SECTIONS: ManagementSection[] = [
	{
		key: "default-bot",
		label: "システムデフォルト Bot",
		desc: "デフォルトBotのDiscordトークン更新",
	},
	{ key: "system", label: "システム全体設定", desc: "システム全体の共通設定" },
	{
		key: "bot-attributes",
		label: "Bot属性設定",
		desc: "プリセットの表示名と汎用モードのレート制限既定値",
	},
	{
		key: "users",
		label: "ユーザー管理",
		desc: "登録ユーザーの一覧とロール管理",
	},
	{
		key: "moderation",
		label: "Bot モデレーション",
		desc: "全Botの管理・差し押さえ",
	},
	{
		key: "personas",
		label: "ペルソナ マーケットプレイス管理",
		desc: "公開ペルソナの非公開化・削除",
	},
	{ key: "audit", label: "監査ログ", desc: "セキュリティ関連の操作履歴" },
	{
		key: "invites",
		label: "招待コード管理",
		desc: "新規ユーザー登録用の招待コードの発行・管理",
	},
];

/**
 * 現パスから選択中のセクションを返す（ハブ表示中・未知キーは null）。
 * route は cleanPath 済み（currentRoute ストア）を想定。
 */
export function sectionFromRoute(
	route: string,
	base: string,
	sections: ManagementSection[],
): ManagementSection | null {
	if (!route.startsWith(`${base}/`)) return null;
	const key = route.slice(base.length + 1);
	return sections.find((s) => s.key === key) ?? null;
}
