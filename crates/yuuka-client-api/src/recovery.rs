//! 起動時の回復スイープ（issue #77・PWA チャットの `202` 受理後の再起動）。
//!
//! `POST /api/client/chat/messages` は `202 Accepted` を返した後、ターンをプロセス内の `tokio::spawn` で
//! 実行する。その間にサーバーが再起動/異常終了すると、タスクも `InFlightTurns`（メモリ内）も消え、
//! 失敗/タイムアウト時に通知行を書く後始末も走らないため、履歴は「未回答のユーザー発言」で終わり、
//! クライアントは最大待機時間までポーリングを続けた末に待機エラーを表示するしかなかった。
//!
//! 方針は **fail-closed**（ターンは再開しない）。Gemini のツール呼び出しは副作用（ToDo/支出の追加等）を
//! 持ちうるため、途中まで進んだターンを再実行すると二重実行になりうる。代わりに、起動時にこのスイープを
//! 1 度実行して取り残された会話へ終端の通知行を書き、クライアントのポーリング（`sinceId` より後の
//! アシスタント行で終了）を有限時間で終わらせて、再送を促す文言を表示させる。受理したユーザー発言自体は
//! `202` の前に永続化済みなので（`chat_send`）失われない。

use yuuka_orchestrator::message_log;
use yuuka_web::Db;

/// 取り残された会話へ書く通知行の文言（他の失敗通知と同じく ⚠️ 始まり）。
pub const RESTART_NOTICE_TEXT: &str =
    "⚠️ サーバー再起動のため応答を生成できませんでした。もう一度送信してください。";

/// 起動時の回復スイープ: `source='pwa'` の最新行が未回答のユーザー発言のままの会話へ、終端の通知行
/// （[`RESTART_NOTICE_TEXT`]・`is_notice=1` のアシスタント行）を 1 件ずつ書く。書いた会話数を返す。
///
/// 詳細（クエリ・冪等性・コスト）は [`message_log::fail_orphaned_pwa_turns`] を参照。
///
/// **呼び出し条件**: プロセス起動時に 1 度だけ、HTTP を受け付ける**前**（ルータの serve 前）に呼ぶ。
/// 新しいプロセスは in-flight ターンを持たないため、該当会話は全て取り残しと断定できる。稼働中に
/// 呼んではならない（進行中のターンの会話まで失敗扱いにしてしまう）。
///
/// DB エラーは warn ログのみで握りつぶす（回復の失敗で起動を止めない。取り残された会話は従来どおり
/// クライアント側の最大待機時間で終端する）。
pub async fn recover_orphaned_chat_turns(db: &Db) -> usize {
    match message_log::fail_orphaned_pwa_turns(db, RESTART_NOTICE_TEXT).await {
        Ok(0) => 0,
        Ok(n) => {
            tracing::info!(
                conversations = n,
                "client-api: 再起動で取り残された PWA チャットの応答待ちに通知行を書きました"
            );
            n
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "client-api: PWA チャットの回復スイープに失敗（起動は継続）"
            );
            0
        }
    }
}
