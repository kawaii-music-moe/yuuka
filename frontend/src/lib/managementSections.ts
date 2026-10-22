// ─────────────────────────────────────────────────────────────────────────────
// 全体管理ページ（連携サービス / あなたの設定 / 管理者設定）のセクション定義。
//
// 各ページはハブ（/integrated 等）で下記セクションを縦並びリストで表示し、
// 行を選ぶと個別ページ（/integrated/<key> 等）へ遷移する。
// ManagementOverlayShell（ハブ描画・カテゴリ見出し・パンくず・適用範囲バッジ）と
// 各 Overlay（{#if section}）がこのキーを単一情報源として共有する。
// 構成は docs/design/settings-restructure.md。
// ─────────────────────────────────────────────────────────────────────────────
import type { SettingsScope } from "$lib/components/ui";

export interface ManagementSection {
	/** URL サブパス（/<base>/<key>） */
	key: string;
	label: string;
	/** ハブの行に出す一行説明 */
	desc: string;
	/** ハブ上の見出し。同じ見出しの行は続けて並べる（未指定は見出し無し）。 */
	category?: string;
	/** ページ全体の適用範囲。ページ内で範囲が混ざるときは未指定にして各カードに出す。 */
	scope?: SettingsScope;
}

export const INTEGRATED_SECTIONS: ManagementSection[] = [
	{
		key: "google",
		label: "Google アカウント",
		desc: "Googleアカウントの連携（複数可）・カレンダー設定・Botごとに使うアカウント",
		scope: "user",
	},
	{
		key: "mcp",
		label: "MCP サーバー",
		desc: "外部MCPサーバーの登録と、使えるBot",
		scope: "user",
	},
	{
		key: "credentials",
		label: "認証情報",
		desc: "パスワード等の登録（パスワードマネージャ）と、使えるBot",
		scope: "user",
	},
];

export const ACCOUNT_SECTIONS: ManagementSection[] = [
	{
		key: "profile",
		label: "表示名とテーマ",
		desc: "ご自身の表示名と、管理画面の外観テーマ",
		category: "プロフィール",
	},
	{
		key: "notifications",
		label: "時刻と通知",
		desc: "タイムゾーン・通知先・リマインドの既定・リッチ返信",
		category: "日々の動き",
		scope: "user",
	},
	{
		key: "ai",
		label: "AI モデル",
		desc: "秘書属性のBotが使う、あなた個人の API キーとモデル",
		category: "日々の動き",
		scope: "user",
	},
	{
		key: "data",
		label: "バックアップと移行",
		desc: "Google Drive への自動バックアップと、Bot移行パッケージ",
		category: "データ",
		scope: "user",
	},
	{
		key: "devices",
		label: "ログイン中の端末",
		desc: "デスクトップアプリのログインと、接続中の端末の失効",
		category: "セキュリティ",
		scope: "user",
	},
	{
		key: "delete",
		label: "アカウントの削除",
		desc: "アカウントと関連データの削除（取り消し不可）",
		category: "セキュリティ",
		scope: "user",
	},
];

export const ADMIN_SECTIONS: ManagementSection[] = [
	{
		key: "default-bot",
		label: "システムデフォルト Bot",
		desc: "デフォルトBotのDiscordトークン更新",
		category: "システム",
		scope: "system",
	},
	{
		key: "system",
		label: "全体設定",
		desc: "利用規約・プライバシーポリシーの URL",
		category: "システム",
		scope: "system",
	},
	{
		key: "bot-attributes",
		label: "Bot属性と利用上限",
		desc: "プリセットの表示名と汎用モードのレート制限既定値",
		category: "システム",
		scope: "system",
	},
	{
		key: "users",
		label: "ユーザー管理",
		desc: "登録ユーザーの一覧とロール管理",
		category: "ユーザーとBot",
		scope: "system",
	},
	{
		key: "moderation",
		label: "Bot モデレーション",
		desc: "全Botの管理・差し押さえ",
		category: "ユーザーとBot",
		scope: "system",
	},
	{
		key: "personas",
		label: "公開ペルソナ管理",
		desc: "公開ペルソナの非公開化・削除",
		category: "ユーザーとBot",
		scope: "system",
	},
	{
		key: "audit",
		label: "監査ログ",
		desc: "セキュリティ関連の操作履歴",
		category: "記録",
		scope: "system",
	},
];

/** ハブの見出しごとのまとまり（定義順。同じ見出しは最初に出た位置へまとめる）。 */
export interface ManagementGroup {
	category?: string;
	sections: ManagementSection[];
}

export function groupSections(
	sections: ManagementSection[],
): ManagementGroup[] {
	const groups: ManagementGroup[] = [];
	for (const section of sections) {
		let group = groups.find((g) => g.category === section.category);
		if (!group) {
			group = { category: section.category, sections: [] };
			groups.push(group);
		}
		group.sections.push(section);
	}
	return groups;
}

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
