//! `/ws/chat` WebSocket transport（Node `src/server/chatWebSocket.ts` パリティ）。
//!
//! クライアントは **Rust デスクトップ**（`clients/desktop/src/model.rs` の `ClientFrame`/`ServerFrame`
//! が唯一の契約）。認証は **Bearer デスクトップトークン**（[`AuthenticatedUser`] が Cookie→Bearer を
//! 解決）、`?botId=` は接続時に 1 Bot へ束縛し [`has_bot_access`] で検証する。フレームは全て JSON テキスト。
//!
//! **実装範囲（P1-2 増分）**: 接続時 `ready` → 受信 `msg` を [`ChatEngine::secretary_turn`] で処理し
//! `status`（thinking/writing）→ `done` を返す。`reset` はコンテキストクリア、`ping` は no-op。keepalive は
//! WS-native ping/pong（30 秒無応答でクローズ）。**縮退（後続）**: `interaction`（コンポーネント配信）・
//! `interim`/`push`（非同期 deferred）・`update` は未実装（オーケストレーションが同期のみのため）。

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use yuuka_discord::{BotStatus, FileAttachment, IncomingChat, InlineMedia, StatusSink, TurnReply};
use yuuka_orchestrator::ChatEngine;
use yuuka_types::SessionUser;
use yuuka_web::{has_bot_access, AppState, AuthenticatedUser, Db};

/// 1 メッセージ添付上限（MB）。Node `DESKTOP_MAX_UPLOAD_MB` 既定 20。
const MAX_UPLOAD_MB: u32 = 20;
/// keepalive ping 間隔（Node `PING_INTERVAL_MS = 30_000`）。
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// `/ws/chat` ルータ（`ChatEngine` を `Extension` で注入・`AppState` 上でマージ）。
pub fn ws_routes(engine: Arc<ChatEngine>) -> Router<AppState> {
    Router::new()
        .route("/ws/chat", get(ws_upgrade))
        .layer(Extension(engine))
}

#[derive(Debug, Deserialize)]
struct BotQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

/// WS upgrade ハンドラ。Bearer 認証（extractor）→ botId アクセス検証 → upgrade。
async fn ws_upgrade(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Extension(engine): Extension<Arc<ChatEngine>>,
    Query(q): Query<BotQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    // 未指定は既定 Bot（Node と同じ system_default 束縛）。
    let bot_id = q
        .bot_id
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "system_default".to_owned());
    match has_bot_access(&state.db, user.0.discord_id.as_str(), &bot_id).await {
        Ok(true) => {}
        Ok(false) => return (StatusCode::FORBIDDEN, "forbidden").into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response(),
    }
    ws.on_upgrade(move |socket| handle_socket(socket, user.0, bot_id, state.db.clone(), engine))
}

// ─── サーバ → クライアントのフレーム（clients/desktop/src/model.rs `ServerFrame` に一致） ───

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ServerFrame<'a> {
    Ready {
        user: &'a SessionUser,
        bot: BotInfo,
        bots: Vec<BotInfo>,
        #[serde(rename = "maxUploadMb")]
        max_upload_mb: u32,
    },
    Status {
        state: &'static str,
    },
    Done {
        #[serde(rename = "messageId")]
        message_id: String,
        text: String,
        embeds: Vec<serde_json::Value>,
        files: Vec<FilePayload>,
        components: Vec<serde_json::Value>,
        deferred: bool,
    },
    Error {
        code: &'static str,
        message: &'static str,
    },
}

#[derive(Serialize, Default)]
struct BotInfo {
    id: String,
    name: String,
    #[serde(rename = "discord_avatar_url", skip_serializing_if = "Option::is_none")]
    discord_avatar_url: Option<String>,
    primary: bool,
}

#[derive(Serialize)]
struct FilePayload {
    name: String,
    mime: String,
    data: String,
}

// ─── クライアント → サーバのフレーム（`ClientFrame`） ───

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ClientFrame {
    Msg {
        #[serde(default)]
        text: String,
        #[serde(default)]
        image: Option<WsAttachment>,
        #[serde(default)]
        audio: Option<WsAttachment>,
        #[serde(default, rename = "replyToId")]
        reply_to_id: Option<String>,
    },
    Reset,
    Ping,
    /// ボタン押下。コンポーネント配信は未実装のため現状は無視する（フィールドは受理して捨てる）。
    Interaction,
    /// 未知の `type` は無視する（Node の `default: return`）。
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct WsAttachment {
    mime: String,
    data: String,
}

/// 1 接続のライフサイクル。ready 配布 → フレーム処理 → keepalive。
async fn handle_socket(
    socket: WebSocket,
    user: SessionUser,
    bot_id: String,
    db: Db,
    engine: Arc<ChatEngine>,
) {
    let (mut sink, mut stream) = socket.split();
    // 送信は writer タスクへ集約（ステータスコールバックとメインループの並行送信を安全にする）。
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
    });

    // ready: 束縛 Bot・アクセス可能 Bot 一覧・添付上限を最初に配布する。
    let bots = list_bot_infos(&db, user.discord_id.as_str()).await;
    let bound = bots
        .iter()
        .find(|b| b.id == bot_id)
        .map_or_else(|| synthetic_bot(&bot_id), clone_bot);
    let ready = ServerFrame::Ready {
        user: &user,
        bot: bound,
        bots,
        max_upload_mb: MAX_UPLOAD_MB,
    };
    let _ = tx.send(json_msg(&ready));

    let mut ticker = tokio::time::interval(PING_INTERVAL);
    ticker.tick().await; // 直近の即時 tick を消費。
    let mut is_alive = true;

    loop {
        tokio::select! {
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(t))) => {
                        handle_text(t.as_str(), &user, &bot_id, &engine, &tx).await;
                    }
                    Some(Ok(Message::Pong(_))) => is_alive = true,
                    Some(Ok(Message::Ping(p))) => {
                        let _ = tx.send(Message::Pong(p));
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // Binary 等は無視（プロトコルは JSON テキスト）。
                    Some(Err(_)) => break,
                }
            }
            _ = ticker.tick() => {
                // 前サイクルで pong が来ていなければ死活断定でクローズ。
                if !is_alive { break; }
                is_alive = false;
                if tx.send(Message::Ping(Vec::new().into())).is_err() { break; }
            }
        }
    }

    drop(tx);
    let _ = writer.await;
}

/// テキストフレームを処理する（Node の受信スイッチ）。
async fn handle_text(
    text: &str,
    user: &SessionUser,
    bot_id: &str,
    engine: &Arc<ChatEngine>,
    tx: &mpsc::UnboundedSender<Message>,
) {
    // 上限超過（base64 込みのフレーム長で概算）。Node は raw バイト長で判定する。
    if text.len() > (MAX_UPLOAD_MB as usize) * 1024 * 1024 {
        let _ = tx.send(json_msg(&err_frame("too_large", "メッセージが大きすぎます。")));
        return;
    }
    let frame: ClientFrame = match serde_json::from_str(text) {
        Ok(f) => f,
        // JSON 不正 → internal（Node parity）。
        Err(_) => {
            let _ = tx.send(json_msg(&err_frame("internal", "不正なメッセージ形式です。")));
            return;
        }
    };

    match frame {
        ClientFrame::Msg {
            text,
            image,
            audio,
            reply_to_id,
        } => {
            process_msg(user, bot_id, engine, tx, text, image, audio, reply_to_id).await;
        }
        ClientFrame::Reset => {
            if let Err(e) = engine.reset_context(user.discord_id.as_str(), bot_id).await {
                tracing::warn!(error = %e, "ws reset のコンテキストクリアに失敗");
            }
        }
        // ping はアプリ層 no-op（keepalive は WS-native）。interaction は未実装（コンポーネント配信は後続）。
        ClientFrame::Ping | ClientFrame::Interaction | ClientFrame::Unknown => {}
    }
}

/// `msg` フレームを 1 ターン処理して `done` を返す。
#[allow(clippy::too_many_arguments)]
async fn process_msg(
    user: &SessionUser,
    bot_id: &str,
    engine: &Arc<ChatEngine>,
    tx: &mpsc::UnboundedSender<Message>,
    text: String,
    image: Option<WsAttachment>,
    audio: Option<WsAttachment>,
    reply_to_id: Option<String>,
) {
    // Gemini キー事前チェック（Node は processMessage 前に error/no_gemini_key を返す）。
    if !engine.user_has_gemini_key(user.discord_id.as_str()).await {
        let _ = tx.send(json_msg(&err_frame(
            "no_gemini_key",
            "Gemini APIキーが未設定です。管理画面から設定してください。",
        )));
        return;
    }

    let incoming = IncomingChat {
        text,
        image: image.map(to_media),
        audio: audio.map(to_media),
        reply_to_msg_id: reply_to_id,
        ..IncomingChat::default()
    };
    let status = status_sink(tx.clone());
    let bot = yuuka_core::BotId::new(bot_id.to_owned());
    let uid = yuuka_core::UserId::new(user.discord_id.clone());

    match engine.secretary_turn(&bot, &uid, incoming, &status).await {
        Ok(reply) => {
            let _ = tx.send(json_msg(&done_frame(reply)));
        }
        Err(e) => {
            tracing::warn!(error = %e, "ws turn 処理に失敗");
            let _ = tx.send(json_msg(&err_frame("internal", "処理中にエラーが発生しました。")));
        }
    }
}

/// ステータスコールバックを WS `status` フレームへ橋渡しする（`idle` は送らない）。
fn status_sink(tx: mpsc::UnboundedSender<Message>) -> StatusSink {
    Arc::new(move |s: BotStatus| {
        let state = match s {
            BotStatus::Thinking => "thinking",
            BotStatus::Writing => "writing",
            BotStatus::Idle => return,
        };
        let _ = tx.send(json_msg(&ServerFrame::Status { state }));
    })
}

/// [`TurnReply`] を `done` フレームへ写像する（files を base64 化・embeds/components は現状空）。
fn done_frame(reply: TurnReply) -> ServerFrame<'static> {
    let files = reply.files.into_iter().map(to_file_payload).collect();
    ServerFrame::Done {
        message_id: random_message_id(),
        text: reply.text,
        embeds: Vec::new(),
        files,
        components: Vec::new(),
        deferred: false,
    }
}

fn err_frame(code: &'static str, message: &'static str) -> ServerFrame<'static> {
    ServerFrame::Error { code, message }
}

fn to_media(a: WsAttachment) -> InlineMedia {
    InlineMedia {
        data_base64: a.data,
        mime_type: a.mime,
    }
}

fn to_file_payload(f: FileAttachment) -> FilePayload {
    let mime = if f.name.ends_with(".png") {
        "image/png"
    } else if f.name.ends_with(".jpg") || f.name.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "application/octet-stream"
    };
    FilePayload {
        name: f.name,
        mime: mime.to_owned(),
        data: base64::engine::general_purpose::STANDARD.encode(f.bytes),
    }
}

/// フレームを JSON テキストの WS メッセージへ（直列化失敗は空メッセージに倒す・実質発生しない）。
fn json_msg<T: Serialize>(frame: &T) -> Message {
    match serde_json::to_string(frame) {
        Ok(s) => Message::Text(s.into()),
        Err(_) => Message::Text(String::new().into()),
    }
}

/// `messageId`（16 バイト CSPRNG hex・Node `randomUUID` の代替）。
fn random_message_id() -> String {
    let mut b = [0u8; 16];
    if getrandom::getrandom(&mut b).is_err() {
        return "0".repeat(32);
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// アクセス可能 Bot 一覧を [`BotInfo`] で返す（Node `listBotsForUser().map(toBotInfo)`）。
async fn list_bot_infos(db: &Db, user_id: &str) -> Vec<BotInfo> {
    let user_id = user_id.to_owned();
    let rows = db
        .read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT b.id, b.name, b.discord_avatar_url FROM bots b \
                     LEFT JOIN bot_shares s ON s.bot_id = b.id AND s.shared_user_id = ?1 AND s.status = 'active' \
                     WHERE b.user_id = ?1 OR b.id = 'system_default' OR s.id IS NOT NULL \
                     ORDER BY b.created_at ASC",
                )
                .map_err(yuuka_db::map_sqlite)?;
            let out = stmt
                .query_map([user_id], |r| {
                    Ok(BotInfo {
                        id: r.get::<_, String>(0)?,
                        name: r.get::<_, String>(1)?,
                        discord_avatar_url: r.get::<_, Option<String>>(2)?,
                        primary: false, // 下で id 判定して設定。
                    })
                })
                .map_err(yuuka_db::map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(yuuka_db::map_sqlite)?;
            Ok(out)
        })
        .await
        .unwrap_or_default();
    rows.into_iter()
        .map(|mut b| {
            b.primary = b.id == "system_default";
            b
        })
        .collect()
}

/// 一覧に無い束縛 Bot の合成情報（Node の `?? { id, name: botId, ... }`）。
fn synthetic_bot(bot_id: &str) -> BotInfo {
    BotInfo {
        id: bot_id.to_owned(),
        name: bot_id.to_owned(),
        discord_avatar_url: None,
        primary: bot_id == "system_default",
    }
}

fn clone_bot(b: &BotInfo) -> BotInfo {
    BotInfo {
        id: b.id.clone(),
        name: b.name.clone(),
        discord_avatar_url: b.discord_avatar_url.clone(),
        primary: b.primary,
    }
}

#[cfg(test)]
mod tests {
    use super::{err_frame, BotInfo, ServerFrame};
    use yuuka_types::{Role, SessionUser};

    #[test]
    fn ready_frame_shape_matches_desktop_contract() {
        let user = SessionUser {
            discord_id: "u1".to_owned(),
            username: "yuu".to_owned(),
            role: Role::User,
        };
        let ready = ServerFrame::Ready {
            user: &user,
            bot: BotInfo {
                id: "system_default".to_owned(),
                name: "デフォルト".to_owned(),
                discord_avatar_url: None,
                primary: true,
            },
            bots: vec![],
            max_upload_mb: 20,
        };
        let v: serde_json::Value = serde_json::to_value(&ready).expect("ser");
        assert_eq!(v["type"], "ready");
        assert_eq!(v["user"]["discordId"], "u1");
        assert_eq!(v["bot"]["id"], "system_default");
        assert_eq!(v["bot"]["primary"], true);
        // discord_avatar_url は None なので出力されない。
        assert!(v["bot"].get("discord_avatar_url").is_none());
        assert_eq!(v["maxUploadMb"], 20);
    }

    #[test]
    fn status_and_error_and_done_shapes() {
        let s: serde_json::Value =
            serde_json::to_value(ServerFrame::Status { state: "thinking" }).unwrap();
        assert_eq!(s["type"], "status");
        assert_eq!(s["state"], "thinking");

        let e: serde_json::Value = serde_json::to_value(err_frame("no_gemini_key", "x")).unwrap();
        assert_eq!(e["type"], "error");
        assert_eq!(e["code"], "no_gemini_key");

        let d: serde_json::Value = serde_json::to_value(ServerFrame::Done {
            message_id: "m1".to_owned(),
            text: "はい。".to_owned(),
            embeds: vec![],
            files: vec![],
            components: vec![],
            deferred: false,
        })
        .unwrap();
        assert_eq!(d["type"], "done");
        assert_eq!(d["messageId"], "m1");
        assert_eq!(d["deferred"], false);
        assert!(d["files"].is_array());
    }

    #[test]
    fn client_frame_parses_msg_reset_ping_and_unknown() {
        use super::ClientFrame;
        assert!(matches!(
            serde_json::from_str::<ClientFrame>(r#"{"type":"msg","text":"hi"}"#).unwrap(),
            ClientFrame::Msg { .. }
        ));
        assert!(matches!(
            serde_json::from_str::<ClientFrame>(r#"{"type":"reset"}"#).unwrap(),
            ClientFrame::Reset
        ));
        assert!(matches!(
            serde_json::from_str::<ClientFrame>(r#"{"type":"ping"}"#).unwrap(),
            ClientFrame::Ping
        ));
        // 未知 type は Unknown（落ちない）。
        assert!(matches!(
            serde_json::from_str::<ClientFrame>(r#"{"type":"whatever"}"#).unwrap(),
            ClientFrame::Unknown
        ));
        // replyToId / 添付キーの検証。
        let f = serde_json::from_str::<ClientFrame>(
            r#"{"type":"msg","text":"x","image":{"mime":"image/png","data":"QUJD"},"replyToId":"9"}"#,
        )
        .unwrap();
        assert!(matches!(&f, ClientFrame::Msg { .. }));
        if let ClientFrame::Msg { image, reply_to_id, .. } = f {
            assert_eq!(image.unwrap().mime, "image/png");
            assert_eq!(reply_to_id.as_deref(), Some("9"));
        }
    }
}
