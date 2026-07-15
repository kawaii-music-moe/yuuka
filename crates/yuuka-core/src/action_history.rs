//! 操作履歴レコーダー（Node `src/services/actionRecorder.ts` パリティ・§3.6 実行ベースマクロ登録）。
//!
//! gemini FC ループが dispatch 毎に [`ActionRecorder::record`] し、`getRecentActionHistory` ツールが
//! [`ActionRecorder::recent`] で直近列を読む。Node は Redis `fc_history:{userId}`（直近 30 件・TTL 2h）
//! ＋ in-memory フォールバック。**本実装は in-memory フォールバック相当**（Redis interop は経路 B・
//! 別途）。プロセス内で `Arc<ActionRecorder>` を 1 つ共有し、FC ループとツールへ注入する。
//!
//! 秘匿・除外は Node と同一契約: 認証情報系ツール／記録系自身／マクロ管理系は記録しない
//! （[`is_excluded`]）。引数のうち秘匿キー（password/secret/token/api key/credential_value）は
//! 値を `(秘匿)` にマスクし、他は 150 文字で truncate する（[`summarize_args`]）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

/// 記録 1 件（Node `RecordedAction`）。`argsSummary` は wire 名（camelCase）を維持する。
#[derive(Debug, Clone, Serialize)]
pub struct RecordedAction {
    /// ツール名（bare 名）。
    pub name: String,
    /// 引数要約（秘匿マスク済み・各値 150 文字 truncate）。
    #[serde(rename = "argsSummary")]
    pub args_summary: String,
    /// 記録時刻（ISO 8601・UTC・ミリ秒）。
    pub at: String,
}

/// 保持する最大件数（Node `MAX_ACTIONS`）。
const MAX_ACTIONS: usize = 30;
/// 揮発 TTL（Node `TTL_SECONDS` = 2 時間）。
const TTL: Duration = Duration::from_secs(2 * 60 * 60);
/// 1 引数値の最大文字数（Node の `slice(0, 150)`）。
const MAX_ARG_CHARS: usize = 150;

/// 記録から除外する Function 名（Node `EXCLUDED_FUNCTIONS`）。
///
/// 認証情報系（値が引数に載り得る）・記録系自身・マクロ管理系（自己言及ループ防止）。
/// `browserFillCredential` は手順上重要で引数にセレクタしか載らないため**記録する**（Node と同じ）。
#[must_use]
pub fn is_excluded(name: &str) -> bool {
    matches!(
        name,
        "listCredentialServices"
            | "addCredential"
            | "updateCredential"
            | "deleteCredential"
            | "savePlaybook"
            | "findPlaybooks"
            | "deletePlaybook"
            | "runPlaybook"
            | "getRecentActionHistory"
    )
}

/// 秘匿すべき引数キーか（Node `SECRET_ARG_PATTERN` = `/password|secret|token|api_?key|credential_value/i`）。
///
/// JS `.test()` は部分一致なので Rust も `contains`。大文字小文字は無視する。`api_?key` は
/// `apikey`／`api_key` の双方に一致する。
fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("password")
        || k.contains("secret")
        || k.contains("token")
        || k.contains("apikey")
        || k.contains("api_key")
        || k.contains("credential_value")
}

/// 1 値を 150 文字で truncate する（超過時は `…` を付す・Node `slice(0,150) + "…"`）。
fn truncate_value(s: &str) -> String {
    if s.chars().count() > MAX_ARG_CHARS {
        let head: String = s.chars().take(MAX_ARG_CHARS).collect();
        format!("{head}…")
    } else {
        s.to_owned()
    }
}

/// 引数を `key=value, …` へ要約する（Node `summarizeArgs`）。null/未指定はスキップ、秘匿キーは
/// `(秘匿)`、文字列はそのまま・非文字列は JSON 文字列化してから truncate する。
#[must_use]
pub fn summarize_args(args: &Value) -> String {
    let Some(obj) = args.as_object() else {
        return String::new();
    };
    let mut parts = Vec::new();
    for (key, value) in obj {
        if value.is_null() {
            continue;
        }
        let rendered = if is_secret_key(key) {
            "(秘匿)".to_owned()
        } else if let Some(s) = value.as_str() {
            truncate_value(s)
        } else {
            truncate_value(&value.to_string())
        };
        parts.push(format!("{key}={rendered}"));
    }
    parts.join(", ")
}

/// ISO 8601（UTC・ミリ秒・`Z`）の現在時刻（Node `new Date().toISOString()`）。
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// ユーザ 1 人ぶんの記録（TTL 付き）。
struct Entry {
    actions: Vec<RecordedAction>,
    expires_at: Instant,
}

/// プロセス内共有の操作履歴レコーダー（Node in-memory フォールバック相当）。
///
/// `Arc<ActionRecorder>` として FC ループ（書き）とツール（読み）へ注入する。
#[derive(Default)]
pub struct ActionRecorder {
    inner: Mutex<HashMap<String, Entry>>,
}

impl ActionRecorder {
    /// 空のレコーダーを作る。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// ポイズン時もガードを回収する（記録欠落 < パニック伝播）。
    fn guard(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// FC dispatch を 1 件記録する（除外関数はスキップ）。TTL 切れの既存記録は捨てて作り直す。
    pub fn record(&self, user_id: &str, name: &str, args: &Value) {
        if is_excluded(name) {
            return;
        }
        let action = RecordedAction {
            name: name.to_owned(),
            args_summary: summarize_args(args),
            at: now_iso(),
        };
        let now = Instant::now();
        let expires_at = now + TTL;
        let mut map = self.guard();
        match map.get_mut(user_id) {
            Some(entry) if entry.expires_at >= now => {
                entry.actions.push(action);
                // 直近 MAX_ACTIONS 件だけ残す（先頭を落とす）。
                let len = entry.actions.len();
                if len > MAX_ACTIONS {
                    entry.actions.drain(0..len - MAX_ACTIONS);
                }
                entry.expires_at = expires_at;
            }
            _ => {
                map.insert(
                    user_id.to_owned(),
                    Entry {
                        actions: vec![action],
                        expires_at,
                    },
                );
            }
        }
    }

    /// 直近の操作履歴を古い順で返す（TTL 切れ・未記録は空）。
    #[must_use]
    pub fn recent(&self, user_id: &str) -> Vec<RecordedAction> {
        let now = Instant::now();
        let map = self.guard();
        match map.get(user_id) {
            Some(entry) if entry.expires_at >= now => entry.actions.clone(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn excluded_functions_not_recorded() {
        let rec = ActionRecorder::new();
        rec.record("u", "addCredential", &json!({"service": "x"}));
        rec.record("u", "getRecentActionHistory", &json!({}));
        assert!(rec.recent("u").is_empty());
    }

    #[test]
    fn records_in_order_and_masks_secrets() {
        let rec = ActionRecorder::new();
        rec.record("u", "addTodo", &json!({"title": "牛乳", "password": "hunter2"}));
        rec.record("u", "addExpense", &json!({"amount": 500}));
        let actions = rec.recent("u");
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].name, "addTodo");
        assert_eq!(actions[1].name, "addExpense");
        // password はマスク、title はそのまま。
        assert!(actions[0].args_summary.contains("password=(秘匿)"));
        assert!(actions[0].args_summary.contains("title=牛乳"));
        // 非文字列は JSON 文字列化。
        assert!(actions[1].args_summary.contains("amount=500"));
    }

    #[test]
    fn scope_is_per_user() {
        let rec = ActionRecorder::new();
        rec.record("a", "addTodo", &json!({"title": "A"}));
        assert_eq!(rec.recent("a").len(), 1);
        assert!(rec.recent("b").is_empty());
    }

    #[test]
    fn caps_at_max_actions_keeping_latest() {
        let rec = ActionRecorder::new();
        for i in 0..(MAX_ACTIONS + 5) {
            rec.record("u", "addTodo", &json!({ "n": i }));
        }
        let actions = rec.recent("u");
        assert_eq!(actions.len(), MAX_ACTIONS);
        // 先頭が落ちて最新が残る（最後は n=34）。
        assert!(actions[0].args_summary.contains("n=5"));
        assert!(actions[MAX_ACTIONS - 1].args_summary.contains(&format!("n={}", MAX_ACTIONS + 4)));
    }

    #[test]
    fn secret_key_variants_detected() {
        assert!(is_secret_key("password"));
        assert!(is_secret_key("apiKey"));
        assert!(is_secret_key("api_key"));
        assert!(is_secret_key("access_token"));
        assert!(is_secret_key("credential_value"));
        assert!(!is_secret_key("title"));
    }
}
