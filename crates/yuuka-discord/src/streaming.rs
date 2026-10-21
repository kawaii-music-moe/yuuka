//! 生成中の本文を Discord へ逐次表示する（返信を 1 通送り、間隔を空けて編集し、最終結果で確定する）。
//!
//! 本文は [`StreamingReply::push`] で同期的に受け取り（FC ループのコールバックから呼ばれる）、送信と編集は
//! 裏のタスクが行う。編集は最新の本文だけを [`EDIT_INTERVAL`] ごとに反映し、途中の版は飛ばす。最終結果は
//! [`StreamingReply::finish`] が、裏のタスクの送信・編集が終わるのを待ってから確定する（遅れて届いた編集で
//! 最終結果を上書きしない）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, Mutex};
use twilight_http::Client;
use twilight_model::id::marker::{ChannelMarker, MessageMarker};
use twilight_model::id::Id;

use crate::ports::TurnReply;
use crate::reply;

/// 最初の本文が届いてから返信を送るまでの待ち。その間に終わる短い返信は逐次表示せず、従来どおり 1 通で送る
/// （送ってすぐ編集しても速くならず、「編集済み」が付くだけなので）。
const FIRST_SEND_DELAY: Duration = Duration::from_millis(800);
/// 編集の最小間隔（Discord のメッセージ編集のレート制限に収める）。
const EDIT_INTERVAL: Duration = Duration::from_millis(1200);

/// 1 ターン分の逐次表示。返信先（チャンネル・入力メッセージ）はメッセージ受信時に確定する。
pub(crate) struct StreamingReply {
    http: Arc<Client>,
    channel_id: Id<ChannelMarker>,
    reply_to: Id<MessageMarker>,
    /// 「入力中…」を止める（逐次表示の返信を送ったら、入力中の表示は要らない）。
    typing_abort: tokio::task::AbortHandle,
    latest: watch::Sender<String>,
    spawned: AtomicBool,
    state: Arc<Mutex<StreamState>>,
}

#[derive(Default)]
struct StreamState {
    /// 逐次表示の返信（まだ送っていなければ `None`）。
    message_id: Option<Id<MessageMarker>>,
    /// いま表示している本文（整形後）。
    shown: String,
    /// 確定した（または送信に失敗して逐次表示をやめた）。以後は送信も編集もしない。
    closed: bool,
}

impl StreamingReply {
    pub(crate) fn new(
        http: Arc<Client>,
        channel_id: Id<ChannelMarker>,
        reply_to: Id<MessageMarker>,
        typing_abort: tokio::task::AbortHandle,
    ) -> Arc<Self> {
        Arc::new(Self {
            http,
            channel_id,
            reply_to,
            typing_abort,
            latest: watch::Sender::new(String::new()),
            spawned: AtomicBool::new(false),
            state: Arc::default(),
        })
    }

    /// 生成中の本文（ここまでに届いた全体）を受け取る。最初の呼び出しで送信・編集のタスクを起こす。
    pub(crate) fn push(&self, text: &str) {
        self.latest.send_replace(text.to_owned());
        if !self.spawned.swap(true, Ordering::AcqRel) {
            tokio::spawn(run(
                Arc::clone(&self.http),
                self.channel_id,
                self.reply_to,
                self.typing_abort.clone(),
                self.latest.subscribe(),
                Arc::clone(&self.state),
            ));
        }
    }

    /// 最終結果で逐次表示を確定する。逐次表示の返信へ最終結果を届けたら `true`。
    ///
    /// まだ返信を送っていなければ何もせず `false`（呼び出し側が従来どおり送る）。最終結果が黙殺（空）なら
    /// 逐次表示の返信を消して `false`（黙殺するかは呼び出し側の方針に従う）。
    pub(crate) async fn finish(&self, reply: &TurnReply) -> bool {
        let mut state = self.state.lock().await;
        state.closed = true;
        let Some(message_id) = state.message_id.take() else {
            return false;
        };
        if reply.is_silent() {
            reply::delete_stream(&self.http, self.channel_id, message_id).await;
            return false;
        }
        reply::complete_stream(
            &self.http,
            self.channel_id,
            self.reply_to,
            message_id,
            &state.shown,
            reply,
        )
        .await;
        true
    }
}

/// 送信・編集のタスク。最初の送信まで [`FIRST_SEND_DELAY`] 待ち、以後は最新の本文を [`EDIT_INTERVAL`]
/// ごとに反映する。確定（`closed`）したら、または本文の送り手がいなくなったら終わる。
async fn run(
    http: Arc<Client>,
    channel_id: Id<ChannelMarker>,
    reply_to: Id<MessageMarker>,
    typing_abort: tokio::task::AbortHandle,
    mut latest: watch::Receiver<String>,
    state: Arc<Mutex<StreamState>>,
) {
    tokio::time::sleep(FIRST_SEND_DELAY).await;
    loop {
        let preview = reply::stream_preview(&latest.borrow_and_update());
        {
            // 送信・編集の間はロックを持ち続け、確定（finish）がその完了を待つようにする。
            let mut state = state.lock().await;
            if state.closed {
                return;
            }
            if !preview.is_empty() && preview != state.shown {
                match state.message_id {
                    None => {
                        let Some(id) =
                            reply::send_stream_start(&http, channel_id, reply_to, &preview).await
                        else {
                            // 送れなければ逐次表示をやめる（最終結果は呼び出し側が従来どおり送る）。
                            state.closed = true;
                            return;
                        };
                        typing_abort.abort();
                        state.message_id = Some(id);
                    }
                    Some(id) => reply::edit_stream_text(&http, channel_id, id, &preview).await,
                }
                state.shown = preview;
            }
        }
        tokio::time::sleep(EDIT_INTERVAL).await;
        if latest.changed().await.is_err() {
            return;
        }
    }
}
