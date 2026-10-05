//! ネットワーク層: WS クライアント・REST・OAuth デバイスフロー。
//!
//! UI スレッド（egui）と Net スレッド（tokio runtime）は分離され、
//! [`UiEvent`] / [`NetEvent`] を `tokio::sync::mpsc` で双方向に流す
//! （client_design.md §3 / architecture.md §8）。
//!
//! - `UiEvent`: UI → Net（送信要求・Bot 切替・リセット・録音完了 …）
//! - `NetEvent`: Net → UI（`ready`/`status`/`done`/`push`/`error` や接続状態変化）

pub mod auth;
pub mod rest;
pub mod ws;

use tokio::sync::mpsc;

use crate::model::{ClientFrame, ServerFrame};

use auth::LoginProgress;

/// UI → Net の要求イベント。
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// WS でフレームを送る（`msg`/`reset`/`ping`/`interaction`）。
    /// ボタン押下（`UiIntent::Interaction`）も `ClientFrame::Interaction` として
    /// この経路を通る（ws_components.md §6）。
    Send(ClientFrame),
    /// 別 Bot へ切替（現 WS を閉じ `?botId=` を変えて張り直す。client_design.md §9）。
    SwitchBot { bot_id: String },
    /// OAuth デバイスフロー開始（未ログイン時。`device_name` は端末一覧の表示名）。
    StartLogin { device_name: Option<String> },
    /// ログアウト（保存トークンを破棄し、未ログイン状態へ戻す）。
    Logout,
    /// アプリ終了に伴う Net タスク停止要求。
    Shutdown,
}

/// Net → UI の通知イベント。
#[derive(Debug, Clone)]
pub enum NetEvent {
    /// 接続状態の変化（ヘッダのオンライン/再接続中表示に使う）。
    Connection(ConnectionState),
    /// サーバから届いたプロトコルフレーム（そのまま UI へ）。
    Frame(ServerFrame),
    /// 致命的でないネットワークエラーの通知（再接続は内部で継続）。
    NetError(String),
    /// ログイン（デバイスフロー）の進捗（ログインビューの状態遷移に使う）。
    Login(LoginEvent),
    /// Bot アイコン画像の取得完了（オーブ/セレクタ描画用。`bytes` は生の PNG/WebP/JPEG）。
    Avatar { bot_id: String, bytes: Vec<u8> },
}

/// デバイスフローの進捗通知（`ui/login.rs` の [`crate::app::LoginUiState`] を駆動）。
#[derive(Debug, Clone)]
pub enum LoginEvent {
    /// `device/code` 取得直後。`user_code` を表示し承認 URL を案内する。
    Code {
        user_code: String,
        verification_uri: String,
    },
    /// 承認待ちポーリング中（スピナー継続）。
    Pending,
    /// 承認完了。以後 WS 接続が始まり `ready` でオーバーレイへ遷移する。
    Succeeded,
    /// ログイン失敗/失効（人間向けメッセージ）。
    Failed(String),
}

/// WS 接続状態（client_design.md §5「接続状態」表示）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// 未接続（起動直後やログイン前）。
    Disconnected,
    /// 接続試行中。
    Connecting,
    /// 接続済み（`ready` 受領後）。`bot_id` は束縛中の Bot。
    Connected { bot_id: String },
    /// 切断され再接続待ち（指数バックオフ中）。`next_retry_secs` は次回待機秒。
    Reconnecting { next_retry_secs: u64 },
}

// ===========================================================================
// Net オーケストレータ（トークン取得 → Bot 解決 → WS セッション → 再ログイン）
// ===========================================================================

/// Net スレッドのトップレベルループ。
///
/// 1. keyring のトークンを読む。無ければ [`UiEvent::StartLogin`] を待ってデバイス
///    フローを回し、access_token を得る（client_design.md §8）。
/// 2. `bot_id` 未確定なら `GET /api/bots` でプライマリ Bot を解決する
///    （architecture.md §8.1）。
/// 3. WS セッションを回す（[`ws::run_ws_loop`]）。ログアウト/トークン失効で
///    セッションが終わったらトークンを破棄して 1. へ戻る。
/// 4. [`UiEvent::Shutdown`]（または `ui_rx` クローズ）で終了する。
pub async fn run(
    startup_bot_id: Option<String>,
    mut ui_rx: mpsc::Receiver<UiEvent>,
    net_tx: mpsc::Sender<NetEvent>,
) {
    // デバイスフロー/REST 用の HTTP クライアント（rustls・OS 非依存）。
    let http = reqwest::Client::builder().build().unwrap_or_default();
    let mut bot_id = startup_bot_id;
    // 取得「成功」済み Bot アイコンの bot_id 集合（再接続/切替で重複取得しないため全体で保持）。
    // 取得は別タスクで走り、成功時にだけここへ記録する。`Arc<Mutex>` なのは、その記録を
    // バックグラウンドの取得タスクから行うため（失敗は記録せず＝次の再接続で再試行できる）。
    let fetched_avatars: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

    loop {
        // 1) トークン取得（keyring → 無ければデバイスフロー）。
        let token = match auth::load_token() {
            Ok(Some(t)) => t,
            Ok(None) => {
                let _ = net_tx
                    .send(NetEvent::Connection(ConnectionState::Disconnected))
                    .await;
                match await_login(&http, &mut ui_rx, &net_tx).await {
                    Some(t) => t,
                    None => return, // shutdown
                }
            }
            Err(e) => {
                log::warn!("keyring load failed: {e}");
                match await_login(&http, &mut ui_rx, &net_tx).await {
                    Some(t) => t,
                    None => return,
                }
            }
        };

        // 2) bot_id 未確定なら REST でプライマリ Bot を解決する。
        if bot_id.is_none() {
            match rest::get_bots(&http, &token).await {
                Ok(bots) => {
                    bot_id = bots
                        .iter()
                        .find(|b| b.primary)
                        .or_else(|| bots.first())
                        .map(|b| b.id.clone());
                }
                Err(rest::RestError::Unauthorized) => {
                    // 保存トークンが失効していた → 破棄して再ログインへ。
                    let _ = auth::delete_token();
                    let _ = net_tx
                        .send(NetEvent::Login(LoginEvent::Failed(
                            "認証の有効期限が切れています。再度ログインしてください。".into(),
                        )))
                        .await;
                    continue;
                }
                Err(e) => {
                    // 一過性のネットワークエラー。少し待って再試行する
                    // （トークンは有効なので await_login には戻さない）。
                    let _ = net_tx
                        .send(NetEvent::NetError(format!(
                            "Bot 一覧の取得に失敗（再試行します）: {e}"
                        )))
                        .await;
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            }
        }

        let Some(active_bot) = bot_id.clone() else {
            // Bot を解決できない（共有 Bot ゼロ等）。ユーザーへ通知し、
            // 次の StartLogin か Shutdown まで待機する（ビジーループ回避）。
            let _ = net_tx
                .send(NetEvent::NetError(
                    "利用可能な Bot がありません。Web 設定をご確認ください。".into(),
                ))
                .await;
            loop {
                match ui_rx.recv().await {
                    None | Some(UiEvent::Shutdown) => return,
                    Some(UiEvent::Logout) => {
                        let _ = auth::delete_token();
                        break;
                    }
                    _ => {}
                }
            }
            bot_id = None;
            continue;
        };

        // 3) WS セッション（ログアウト/失効/終了で復帰）。
        match ws::run_ws_loop(
            &token,
            active_bot,
            &mut ui_rx,
            &net_tx,
            &http,
            &fetched_avatars,
        )
        .await
        {
            ws::WsExit::Shutdown => return,
            ws::WsExit::LoggedOut => {
                // トークン失効 or 明示ログアウト → 破棄して再ログインへ。
                let _ = auth::delete_token();
                bot_id = None; // 別ユーザーになりうるので Bot も再解決する。
                let _ = net_tx
                    .send(NetEvent::Connection(ConnectionState::Disconnected))
                    .await;
            }
        }
    }
}

/// 未ログイン状態で [`UiEvent::StartLogin`] を待ち、デバイスフローを実行する。
///
/// 成功で access_token を返す。`ui_rx` クローズ/[`UiEvent::Shutdown`] で `None`。
/// 失敗時は [`LoginEvent::Failed`] を通知し、次の `StartLogin` を待ち続ける。
async fn await_login(
    http: &reqwest::Client,
    ui_rx: &mut mpsc::Receiver<UiEvent>,
    net_tx: &mpsc::Sender<NetEvent>,
) -> Option<String> {
    loop {
        match ui_rx.recv().await {
            None | Some(UiEvent::Shutdown) => return None,
            Some(UiEvent::StartLogin { device_name }) => {
                let mut progress = ChannelProgress {
                    net_tx: net_tx.clone(),
                };
                match auth::run_device_flow(http, device_name, &mut progress).await {
                    Ok(token) => {
                        let _ = net_tx.send(NetEvent::Login(LoginEvent::Succeeded)).await;
                        return Some(token);
                    }
                    Err(e) => {
                        let _ = net_tx
                            .send(NetEvent::Login(LoginEvent::Failed(login_error_message(&e))))
                            .await;
                        // 継続して次の StartLogin を待つ。
                    }
                }
            }
            // 未ログイン中の送信/切替/ログアウトは無視（まだ接続が無い）。
            Some(_) => {}
        }
    }
}

/// [`auth::AuthError`] を人間向けの短いメッセージへ変換する。
fn login_error_message(e: &auth::AuthError) -> String {
    use auth::AuthError;
    match e {
        AuthError::Expired => "コードの有効期限が切れました。もう一度お試しください。".into(),
        AuthError::Http(_) => "サーバに接続できませんでした。ネットワークをご確認ください。".into(),
        AuthError::Keyring(_) => "トークンの保存に失敗しました（OS 資格情報ストア）。".into(),
        AuthError::Server(msg) => format!("ログインに失敗しました: {msg}"),
    }
}

/// デバイスフロー進捗を [`NetEvent::Login`] として UI へ転送する [`LoginProgress`]。
///
/// コールバックは同期だが `try_send`（ノンブロッキング）なので tokio タスクから
/// 安全に叩ける。満杯時の取りこぼしは進捗表示なので許容する。
struct ChannelProgress {
    net_tx: mpsc::Sender<NetEvent>,
}

impl LoginProgress for ChannelProgress {
    fn on_code(&mut self, user_code: &str, verification_uri_complete: &str) {
        let _ = self.net_tx.try_send(NetEvent::Login(LoginEvent::Code {
            user_code: user_code.to_string(),
            verification_uri: verification_uri_complete.to_string(),
        }));
    }

    fn on_pending(&mut self) {
        let _ = self.net_tx.try_send(NetEvent::Login(LoginEvent::Pending));
    }
}
