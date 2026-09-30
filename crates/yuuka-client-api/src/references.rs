//! チャット応答本文からのリファレンス推定（Node `messageReferences`・`clientRoutes.ts`）。
//!
//! キーワードの部分一致で `finance`/`calendar`/`todo`/`note` のいずれかを推定する（Node は正規表現
//! だが境界指定が無いため部分一致と等価・大小無視は小文字化で代替）。日本語キーワードは活用/送り
//! 仮名を問わない部分一致のため小文字化は不要。

use crate::dto::ChatReferenceView;

/// 本文からリファレンスを 1 件だけ推定する（Node と同じ優先順位: finance → calendar → todo →
/// note・最初に一致した種別のみ）。どれにも一致しなければ `None`。
pub fn infer_reference(content: &str) -> Option<ChatReferenceView> {
    let lower = content.to_lowercase();
    let has_any = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));

    if has_any(&[
        "expense", "income", "budget", "finance", "payment", "家計", "収支", "支出", "収入",
    ]) {
        return Some(ChatReferenceView {
            kind: "finance",
            title: "Finance",
            description: "Open the finance record",
            href: "/finance",
            meta: "Finance",
        });
    }
    if has_any(&["calendar", "schedule", "event", "予定", "カレンダー"]) {
        return Some(ChatReferenceView {
            kind: "calendar",
            title: "Calendar",
            description: "Open the scheduled event",
            href: "/calendar",
            meta: "Calendar",
        });
    }
    if has_any(&["task", "todo", "to-do", "タスク"]) {
        return Some(ChatReferenceView {
            kind: "todo",
            title: "Tasks",
            description: "Open the task list",
            href: "/todo",
            meta: "Tasks",
        });
    }
    if has_any(&["note", "memory", "ノート", "メモ"]) {
        return Some(ChatReferenceView {
            kind: "note",
            title: "Shared note",
            description: "Open shared memory",
            href: "/notes",
            meta: "Note",
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::infer_reference;

    #[test]
    fn detects_finance_keywords_case_insensitive() {
        let r = infer_reference("Let's talk about your Budget this month").unwrap();
        assert_eq!(r.kind, "finance");
    }

    #[test]
    fn detects_japanese_keywords() {
        assert_eq!(
            infer_reference("今日の予定を確認しました").unwrap().kind,
            "calendar"
        );
        assert_eq!(
            infer_reference("タスクを追加しました").unwrap().kind,
            "todo"
        );
        assert_eq!(
            infer_reference("メモに残しておきますね").unwrap().kind,
            "note"
        );
        assert_eq!(
            infer_reference("収支を記録しました").unwrap().kind,
            "finance"
        );
    }

    #[test]
    fn priority_is_finance_then_calendar_then_todo_then_note() {
        // finance と calendar 両方に一致する文でも finance が優先される（Node と同順）。
        let r = infer_reference("expense and schedule both mentioned").unwrap();
        assert_eq!(r.kind, "finance");
    }

    #[test]
    fn returns_none_when_no_keyword_matches() {
        assert!(infer_reference("こんにちは、元気ですか？").is_none());
    }
}
