//! 選択可能な機能モジュールのカタログ（Node `functions/moduleCatalog.ts` の UI/API 用メタ情報）。
//!
//! Web `/api/bots/modules` が「当該 Bot の capability 配下の selectable モジュール一覧 + ユーザー視点の
//! 有効/無効」を返すために使う静的定義。ランタイムの `FunctionModule` 参照は含めない（メタのみ）。
//! `id` は永続化キー（`bots.enabled_modules` へ保存・リネーム禁止）。

/// カタログ 1 エントリ（`listSelectableModules` の要素・全て `selectable=true`）。
#[derive(Debug, Clone, Copy)]
pub struct ModuleMeta {
    pub id: &'static str,
    /// `"core" | "persona" | "memory" | "mcp" | "secretary"`。
    pub cap: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// 管理 UI サイドバーの data-tab 値（無い場合は `None`）。
    pub settings_key: Option<&'static str>,
}

/// selectable なモジュール（Node `MODULE_CATALOG` の `selectable=true` エントリ・同順）。
/// `richContent`（core・selectable=false）は含めない。
pub const SELECTABLE_MODULES: &[ModuleMeta] = &[
    ModuleMeta {
        id: "todo",
        cap: "secretary",
        label: "ToDo・タスク管理",
        description: "タスクの登録・タグ・優先度・ルーチン管理",
        settings_key: Some("tasks"),
    },
    ModuleMeta {
        id: "schedule",
        cap: "secretary",
        label: "スケジュール",
        description: "予定の管理（Googleカレンダー連携）",
        settings_key: Some("schedules"),
    },
    ModuleMeta {
        id: "timeline",
        cap: "secretary",
        label: "デイリータイムライン",
        description: "1日の行動計画・移動・記録（写真・支出・タスク完了）",
        settings_key: Some("timeline"),
    },
    ModuleMeta {
        id: "reminder",
        cap: "secretary",
        label: "リマインダー",
        description: "通知のスケジュール・お知らせ",
        settings_key: Some("reminders"),
    },
    ModuleMeta {
        id: "finance",
        cap: "secretary",
        label: "家計・支出管理",
        description: "支出の記録・集計・予算管理",
        settings_key: Some("expenses"),
    },
    ModuleMeta {
        id: "browser",
        cap: "secretary",
        label: "ブラウザ操作・Web検索",
        description: "Web検索・ページ取得・ブラウザ自動操作",
        settings_key: None,
    },
    ModuleMeta {
        id: "credential",
        cap: "secretary",
        label: "認証情報の保管",
        description: "ログイン情報の暗号化保存・管理",
        settings_key: None,
    },
    ModuleMeta {
        id: "playbook",
        cap: "secretary",
        label: "プレイブック・自動化",
        description: "定型ワークフロー・自動化スクリプト",
        settings_key: Some("playbooks"),
    },
    ModuleMeta {
        id: "note",
        cap: "memory",
        label: "個人メモ",
        description: "メモの記録・参照",
        settings_key: Some("personal"),
    },
    ModuleMeta {
        id: "clipboard",
        cap: "secretary",
        label: "クリップボード共有",
        description: "テキストの一時共有",
        settings_key: None,
    },
    ModuleMeta {
        id: "contact",
        cap: "secretary",
        label: "連絡先",
        description: "連絡先の管理",
        settings_key: None,
    },
    ModuleMeta {
        id: "conversation",
        cap: "memory",
        label: "会話履歴・記憶検索",
        description: "過去の会話の検索・記憶",
        settings_key: None,
    },
    ModuleMeta {
        id: "briefing",
        cap: "secretary",
        label: "朝刊・ニュース",
        description: "ニュース・RSS取得・朝刊",
        settings_key: None,
    },
    ModuleMeta {
        id: "chart",
        cap: "secretary",
        label: "グラフ生成",
        description: "数値データのグラフ可視化",
        settings_key: None,
    },
];

/// 指定 ID が selectable なモジュールか（Node `isKnownSelectableModule`）。
#[must_use]
pub fn is_known_selectable(id: &str) -> bool {
    SELECTABLE_MODULES.iter().any(|m| m.id == id)
}

/// ID からエントリを引く。
#[must_use]
pub fn find(id: &str) -> Option<&'static ModuleMeta> {
    SELECTABLE_MODULES.iter().find(|m| m.id == id)
}
