//! WebSocket クライアント（tokio-tungstenite）。
//!
//! 仕様: client_design.md §9 / backend_api.md §3。
//!
//! - `wss://{host}/ws/chat?botId={id}` に `Authorization: Bearer` 付きで接続。
//! - 受信タスク: JSON を [`ServerFrame`] にデコードし [`NetEvent::Frame`] で UI へ。
//! - 送信タスク: UI からの [`UiEvent::Send`] を WS テキストフレームへ。
//! - 指数バックオフ再接続（1s→2s→…→上限 30s）。再接続後 `ready` で復帰。
//! - keepalive: 一定間隔で WS Ping。
//! - **Bot 切替 = 再接続**（[`UiEvent::SwitchBot`] で現接続を閉じ新 botId で張り直す）。
//!
//! このモジュールは「タスク構造」を提供する。UI 消費側がスタブでも、
//! [`run_ws_loop`] は単体で回せる（mpsc を渡すだけ）。

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message;

use crate::config::ws_url;
use crate::model::ClientFrame;

use super::{ConnectionState, NetEvent, UiEvent};

/// keepalive Ping 間隔（backend_api.md §3.5: 例 30s）。
const PING_INTERVAL: Duration = Duration::from_secs(30);
/// 再接続バックオフの初期値・上限（client_design.md §9: 1s→…→30s）。
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// WS ループの終了理由（[`run_ws_loop`] の戻り値）。オーケストレータが分岐する。
pub enum WsExit {
    /// アプリ終了（`ui_rx` クローズ含む）。Net スレッドを止める。
    Shutdown,
    /// トークン失効 or 明示ログアウト。トークンを破棄して再ログインへ戻す。
    LoggedOut,
}

/// WS のメインループ。切断されたらバックオフして再接続し続ける。
///
/// チャネルは借用で受け取り（オーケストレータが所有・複数セッション間で再利用する）、
/// 終了理由を [`WsExit`] で返す:
/// - [`UiEvent::Shutdown`] / `ui_rx` クローズ → [`WsExit::Shutdown`]
/// - [`UiEvent::Logout`] / 401 / `error{unauthorized}` → [`WsExit::LoggedOut`]
/// - [`UiEvent::SwitchBot`] は内部で botId を差し替えて即再接続する。
pub async fn run_ws_loop(
    token: &str,
    mut bot_id: String,
    ui_rx: &mut mpsc::Receiver<UiEvent>,
    net_tx: &mpsc::Sender<NetEvent>,
    http: &reqwest::Client,
    fetched_avatars: &std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
) -> WsExit {
    let mut backoff = BACKOFF_INITIAL;
    // 切断中に積まれた送信を退避するキュー（NFR-6: 再接続後に再送）。
    let mut outbox: Vec<ClientFrame> = Vec::new();
    // 403 を通知済みの botId（同一 Bot への再試行ごとに ⚠ を出し続けない）。
    let mut forbidden_notified: Option<String> = None;
    // 直近で接続に成功した botId。403 で行き止まりにしないため、ここへ自動で戻す。
    let mut last_good_bot: Option<String> = None;

    loop {
        let _ = net_tx
            .send(NetEvent::Connection(ConnectionState::Connecting))
            .await;

        match connect(token, &bot_id).await {
            Ok(stream) => {
                backoff = BACKOFF_INITIAL; // 接続成功でバックオフをリセット。
                forbidden_notified = None; // 以後この Bot が 403 になれば再通知する。
                last_good_bot = Some(bot_id.clone()); // 403 時の自動復帰先として記憶。
                let _ = net_tx
                    .send(NetEvent::Connection(ConnectionState::Connected {
                        bot_id: bot_id.clone(),
                    }))
                    .await;

                // 退避していた送信を先頭に注入して、1 接続分のセッションを回す。
                match run_session(stream, ui_rx, net_tx, &mut outbox, http, fetched_avatars).await {
                    SessionOutcome::Shutdown => return WsExit::Shutdown,
                    SessionOutcome::LoggedOut => return WsExit::LoggedOut,
                    SessionOutcome::SwitchBot(new_id) => {
                        // Bot 切替: 即時に新 botId で張り直す（バックオフ無し）。
                        bot_id = new_id;
                        outbox.clear(); // 別 Bot の文脈なので退避は破棄。
                        continue;
                    }
                    SessionOutcome::Disconnected => {
                        // 通常切断 → バックオフへ。
                    }
                }
            }
            Err(e) => {
                // upgrade が 401 を返した = トークン失効。再ログインへ。
                if is_unauthorized(&e) {
                    return WsExit::LoggedOut;
                }
                // 403 = この Bot へのアクセス権が無い（共有取消・無効 ID 等）。
                if is_forbidden(&e) {
                    // NetError は UI に出ないため、エラーフレームを合成して履歴に可視化する
                    // （沈黙の無限リトライで「選んでも切り替わらない」ように見えるのを防ぐ）。
                    // 同一 Bot の再試行ごとに繰り返さないよう、通知は botId 単位で 1 度だけ。
                    if forbidden_notified.as_deref() != Some(bot_id.as_str()) {
                        forbidden_notified = Some(bot_id.clone());
                        let _ = net_tx
                            .send(NetEvent::Frame(crate::model::ServerFrame::Error {
                                code: crate::model::ErrorCode::Internal,
                                message: "この Bot に接続できません（アクセス権限がありません）。直前の Bot に戻ります。"
                                    .into(),
                            }))
                            .await;
                    }
                    // 直前に正常接続できていた別 Bot があれば、そこへ即座に戻す
                    // （切替先が 403 でもユーザーを行き止まりにせず、selector も追従する）。
                    if let Some(prev) = last_good_bot.clone() {
                        if prev != bot_id {
                            bot_id = prev;
                            outbox.clear();
                            backoff = BACKOFF_INITIAL;
                            continue;
                        }
                    }
                }
                let _ = net_tx
                    .send(NetEvent::NetError(format!("connect failed: {e}")))
                    .await;
            }
        }

        // 再接続待ち（指数バックオフ）。待機中に Shutdown / Logout / SwitchBot を受けたら反応する。
        let _ = net_tx
            .send(NetEvent::Connection(ConnectionState::Reconnecting {
                next_retry_secs: backoff.as_secs(),
            }))
            .await;

        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            msg = ui_rx.recv() => match msg {
                Some(UiEvent::Shutdown) | None => return WsExit::Shutdown,
                Some(UiEvent::Logout) => return WsExit::LoggedOut,
                Some(UiEvent::SwitchBot { bot_id: new_id }) => {
                    bot_id = new_id;
                    outbox.clear();
                    backoff = BACKOFF_INITIAL;
                    continue;
                }
                // 切断中の送信は退避キューへ（再接続後に再送）。
                Some(UiEvent::Send(frame)) => outbox.push(frame),
                // 接続済み相当の状態なので StartLogin は無視。
                Some(UiEvent::StartLogin { .. }) => {}
            }
        }

        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// upgrade レスポンスが 401 か（保存トークン失効の判定）。
fn is_unauthorized(e: &tokio_tungstenite::tungstenite::Error) -> bool {
    use tokio_tungstenite::tungstenite::http::StatusCode;
    use tokio_tungstenite::tungstenite::Error;
    matches!(e, Error::Http(resp) if resp.status() == StatusCode::UNAUTHORIZED)
}

/// upgrade レスポンスが 403 か（この Bot へのアクセス権が無い判定。server.ts upgrade）。
fn is_forbidden(e: &tokio_tungstenite::tungstenite::Error) -> bool {
    use tokio_tungstenite::tungstenite::http::StatusCode;
    use tokio_tungstenite::tungstenite::Error;
    matches!(e, Error::Http(resp) if resp.status() == StatusCode::FORBIDDEN)
}

/// 1 接続分のセッション内ループの結果。
enum SessionOutcome {
    /// アプリ終了。
    Shutdown,
    /// トークン失効 or 明示ログアウト（再ログインへ）。
    LoggedOut,
    /// Bot 切替要求（新 botId）。
    SwitchBot(String),
    /// 接続が切れた（再接続へ）。
    Disconnected,
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// `wss://.../ws/chat?botId={id}` へ Bearer 付きで接続する。
async fn connect(
    token: &str,
    bot_id: &str,
) -> Result<WsStream, tokio_tungstenite::tungstenite::Error> {
    let url = ws_url(bot_id);
    // ネイティブクライアントは upgrade リクエストに任意ヘッダを付けられる
    // （backend_api.md §3.1: Authorization: Bearer）。
    let mut request = url.into_client_request()?;
    request.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {token}")
            .parse()
            .expect("bearer header value is valid ascii"),
    );
    let (stream, _resp) = tokio_tungstenite::connect_async(request).await?;
    Ok(stream)
}

/// 接続確立後の送受信ループ。送信（UI/keepalive）と受信を `select!` で多重化する。
async fn run_session(
    stream: WsStream,
    ui_rx: &mut mpsc::Receiver<UiEvent>,
    net_tx: &mpsc::Sender<NetEvent>,
    outbox: &mut Vec<ClientFrame>,
    http: &reqwest::Client,
    fetched_avatars: &std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
) -> SessionOutcome {
    let (mut sink, mut source) = stream.split();
    let mut ping_timer = tokio::time::interval(PING_INTERVAL);
    ping_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // 再接続直後に退避分を送り直す（NFR-6）。退避キューを一旦取り出し、
    // 送れなかった分（とそれ以降）はキューへ戻して切断扱いにする。
    let pending: Vec<ClientFrame> = std::mem::take(outbox);
    let mut pending_iter = pending.into_iter();
    while let Some(frame) = pending_iter.next() {
        if send_frame(&mut sink, &frame, net_tx).await.is_err() {
            outbox.push(frame);
            outbox.extend(pending_iter); // 残りも順序を保って退避へ戻す。
            return SessionOutcome::Disconnected;
        }
    }

    loop {
        tokio::select! {
            // --- UI → WS（送信要求 / 切替 / ログアウト / 停止）---
            ui = ui_rx.recv() => match ui {
                None | Some(UiEvent::Shutdown) => {
                    let _ = sink.close().await;
                    return SessionOutcome::Shutdown;
                }
                Some(UiEvent::Logout) => {
                    let _ = sink.close().await;
                    return SessionOutcome::LoggedOut;
                }
                Some(UiEvent::SwitchBot { bot_id }) => {
                    let _ = sink.close().await;
                    return SessionOutcome::SwitchBot(bot_id);
                }
                Some(UiEvent::Send(frame)) => {
                    if send_frame(&mut sink, &frame, net_tx).await.is_err() {
                        // 送信失敗 → 退避して切断扱い。
                        outbox.push(frame);
                        return SessionOutcome::Disconnected;
                    }
                }
                // 接続済みなので StartLogin は無視。
                Some(UiEvent::StartLogin { .. }) => {}
            },

            // --- keepalive Ping ---
            _ = ping_timer.tick() => {
                if sink.send(Message::Ping(Vec::new())).await.is_err() {
                    return SessionOutcome::Disconnected;
                }
            }

            // --- WS → UI（受信）---
            incoming = source.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<crate::model::ServerFrame>(&text) {
                        Ok(frame) => {
                            // ready を受けたら束縛 Bot と一覧のアイコンを Net 側で先読みする
                            // （UI スレッドを塞がない・重複は fetched_avatars で抑止）。
                            if let crate::model::ServerFrame::Ready { bot, bots, .. } = &frame {
                                spawn_avatar_fetches(http, bot, bots, fetched_avatars, net_tx);
                            }
                            // トークン失効通知は UI へ転送しつつセッションを畳んで再ログインへ。
                            let unauthorized = matches!(
                                &frame,
                                crate::model::ServerFrame::Error {
                                    code: crate::model::ErrorCode::Unauthorized,
                                    ..
                                }
                            );
                            let _ = net_tx.send(NetEvent::Frame(frame)).await;
                            if unauthorized {
                                let _ = sink.close().await;
                                return SessionOutcome::LoggedOut;
                            }
                        }
                        Err(e) => {
                            let _ = net_tx
                                .send(NetEvent::NetError(format!("decode error: {e}")))
                                .await;
                        }
                    }
                }
                // サーバ Ping には tungstenite が自動で Pong を返す。Pong/Binary は無視。
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Binary(_))) | Some(Ok(Message::Frame(_))) => {}
                Some(Ok(Message::Close(_))) | None => return SessionOutcome::Disconnected,
                Some(Err(e)) => {
                    let _ = net_tx
                        .send(NetEvent::NetError(format!("ws error: {e}")))
                        .await;
                    return SessionOutcome::Disconnected;
                }
            },
        }
    }
}

/// `ready` の束縛 Bot + 一覧について、未取得のアイコンをバックグラウンドで取得する。
///
/// 各取得は独立タスクで走り、完了ぶんから [`NetEvent::Avatar`] で UI へ流す。`fetched`
/// には**取得に成功した** bot_id だけを記録して再接続/切替での重複取得を避ける。
/// 重要: 以前は取得前に楽観的に `fetched.insert` していたため、初回の一過性失敗
/// （ネットワーク瞬断・CDN 一時不調等）で「取得済み」と誤記録され、以後そのセッション中は
/// 二度と再取得されず**アイコンが永久に出ない**事故になっていた。記録は成功時のみに修正。
fn spawn_avatar_fetches(
    http: &reqwest::Client,
    bound: &crate::model::BotInfo,
    bots: &[crate::model::BotInfo],
    fetched: &std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    net_tx: &mpsc::Sender<NetEvent>,
) {
    let mut targets: Vec<(String, String)> = Vec::new();
    {
        // 成功済み集合を参照して未取得ぶんだけ選ぶ（ロックは選別中だけ・短時間保持）。
        let done = fetched.lock().unwrap_or_else(|e| e.into_inner());
        for b in std::iter::once(bound).chain(bots.iter()) {
            if done.contains(&b.id) {
                continue;
            }
            match b.discord_avatar_url.as_deref() {
                Some(url) if !url.is_empty() => targets.push((b.id.clone(), url.to_string())),
                _ => log::info!("bot {} has no avatar url; orb will show initial", b.id),
            }
        }
    }
    if !targets.is_empty() {
        log::info!("fetching {} bot avatar(s)", targets.len());
    }
    for (bot_id, url) in targets {
        let http = http.clone();
        let net_tx = net_tx.clone();
        let fetched = std::sync::Arc::clone(fetched);
        tokio::spawn(async move {
            match crate::net::rest::fetch_avatar(&http, &url).await {
                Ok(bytes) => {
                    log::info!("avatar fetched for {bot_id} ({} bytes)", bytes.len());
                    // 成功時にだけ記録（ロックは insert の間だけ・await をまたがない）。
                    fetched
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(bot_id.clone());
                    let _ = net_tx.send(NetEvent::Avatar { bot_id, bytes }).await;
                }
                Err(e) => log::warn!("avatar fetch failed for {bot_id} ({url}): {e}"),
            }
        });
    }
}

/// [`ClientFrame`] を JSON テキストフレームとして送る。
async fn send_frame<S>(
    sink: &mut S,
    frame: &ClientFrame,
    net_tx: &mpsc::Sender<NetEvent>,
) -> Result<(), ()>
where
    S: SinkExt<Message> + Unpin,
{
    let json = match serde_json::to_string(frame) {
        Ok(j) => j,
        Err(e) => {
            let _ = net_tx
                .send(NetEvent::NetError(format!("encode error: {e}")))
                .await;
            return Err(());
        }
    };
    sink.send(Message::Text(json)).await.map_err(|_| ())
}
