//! Yuuka Desktop — エントリポイント。
//!
//! 仕様: client_design.md §3 / §4 / main.rs の役割（eframe 起動・トレイ/ホットキー
//! 初期化・ランタイム起動・単一インスタンスロック）。
//!
//! 起動シーケンス:
//! 1. 起動引数（`--bot` / `--hidden`）と設定を読み、botId を決定する。
//! 2. **単一インスタンスロック**（botId キー）を取得。既起動なら終了。
//! 3. tokio ランタイムを**別スレッド**で起動し、Net タスク（WS/REST/Auth）を回す。
//! 4. トレイ + ホットキー初期化（ホットキーは**プライマリ Bot のみ登録** TODO）。
//! 5. eframe（透明・最前面・枠なし）を起動し UI スレッドで描画。

// Windows のリリースビルドではコンソールウィンドウを出さない（常駐 GUI）。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod attach;
mod audio;
mod config;
mod model;
mod net;
mod os;
mod ui;

use tokio::sync::mpsc;

use config::{CliArgs, Settings};
use net::{NetEvent, UiEvent};
use os::{InstanceLock, OsIntegration};

/// チャネルのバッファ長（UI↔Net）。送信要求はバースト程度なので控えめで十分。
const CHANNEL_CAP: usize = 64;

fn main() -> eframe::Result<()> {
    // ログ初期化（RUST_LOG で制御。既定 info）。
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // 1) 起動引数 + 設定 → botId 決定。
    let args = CliArgs::from_env();
    let settings = Settings::load();
    let bot_id = config::resolve_bot_id(&args, &settings);

    log::info!(
        "starting yuuka-desktop (api_base={}, bot_id={:?}, hidden={})",
        config::API_BASE,
        bot_id,
        args.hidden
    );

    // 2) 単一インスタンスロック（botId キー）。
    //    botId 未確定（初回起動）の場合は仮キー "pending" でロックし、
    //    ready 受領後の確定 botId での厳密な抑止は将来の最適化とする。
    let osint = os::platform();
    let lock_key = bot_id.clone().unwrap_or_else(|| "pending".to_string());
    let _instance_guard = match osint.acquire_single_instance(&lock_key) {
        InstanceLock::Acquired(guard) => guard, // 保持し続ける（drop でロック解放）。
        InstanceLock::AlreadyRunning => {
            // 同 botId が既に起動済み → 既存ウィンドウを前面化（TODO）し自プロセスは終了。
            log::info!("another instance for bot '{lock_key}' is already running; exiting");
            // TODO(Phase 5): 既存インスタンスのオーバーレイをフォーカスする IPC。
            return Ok(());
        }
    };

    // 3) UI ↔ Net チャネル。
    let (ui_tx, ui_rx) = mpsc::channel::<UiEvent>(CHANNEL_CAP);
    let (net_tx, net_rx) = mpsc::channel::<NetEvent>(CHANNEL_CAP);

    // 4) tokio ランタイムを別スレッドで起動し Net タスクを回す。
    //    起動引数で botId が確定している場合のみ即 WS 接続を試みる。
    //    （未確定/未ログインの場合はデバイスフロー後に接続する経路を Phase 2 後半で配線。）
    spawn_net_thread(bot_id.clone(), ui_rx, net_tx);

    // 5) トレイ + ホットキー（骨格）。
    //    NOTE: tray-icon / global-hotkey は eframe のイベントループと統合する必要があり、
    //    実配線は Phase 2 後半〜Phase 5。ここではプライマリ判定のフックだけ TODO で残す。
    //    TODO(hotkey): `ready.bot.primary == true` のインスタンスのみ
    //                  global-hotkey を登録する（client_design.md §4.3「プライマリが吸う」）。
    //    TODO(tray):   tray-icon メニュー「表示 / Bot 一覧 / 設定 / ログアウト / 終了」。

    // 6) eframe 起動。透明・最前面・枠なしのオーバーレイ。
    //
    // クリック透過（mouse_passthrough）は意図的に**使わない**。Windows では winit が
    // クリック透過を `WS_EX_LAYERED | WS_EX_TRANSPARENT` で実装するが、レイヤード
    // ウィンドウは OpenGL(glow) サーフェスの**ピクセル毎アルファを合成しない**ため、
    // 透過が効かず窓が不透明になる（→ Windows 11 が角を丸めるので「角丸の四角」に見える）。
    // さらに透過状態のままだとオーブ自体もクリックできない。glow 透過とレイヤード透過は
    // Windows では両立しないので、オーブ窓は「透明＋クリック可能」な通常窓として出す。
    // （オーブ窓は 76px 角と小さく、移動も可能なので背面が塞がれる影響は軽微。）
    let viewport = egui::ViewportBuilder::default()
        .with_transparent(true) // 透明背景（glow のアルファを DWM が合成）
        .with_always_on_top() // 最前面
        .with_decorations(false) // 枠なし
        .with_taskbar(false) // タスクバー非表示（常駐演出）
        // 起動直後はログインビュー（パネル大）。ready 後に Overlay へ縮小する（app.rs が制御）。
        .with_inner_size(app::PANEL_WINDOW_SIZE)
        .with_min_inner_size([app::ORB_WINDOW_SIZE, app::ORB_WINDOW_SIZE]);

    let native_options = eframe::NativeOptions {
        viewport,
        // 透明ウィンドウのため glow バックエンドでクリアカラーを尊重させる。
        ..Default::default()
    };

    eframe::run_native(
        "Yuuka Desktop",
        native_options,
        Box::new(move |cc| {
            // 日本語フォントを登録（未登録だと CJK が豆腐 □ になる）。
            app::install_fonts(&cc.egui_ctx);
            // 画像ローダを登録（オーブの Bot アイコン・受信画像の表示に必要）。
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(app::YuukaApp::new(settings, ui_tx, net_rx)))
        }),
    )
}

/// tokio ランタイムを専用スレッドで起動し、Net オーケストレータ（[`net::run`]）を回す。
///
/// トークン取得（keyring or デバイスフロー）→ Bot 解決 → WS セッション →
/// ログアウト/失効での再ログインまでを [`net::run`] が一括して扱う。
/// `bot_id` が `Some` なら起動直後にその Bot へ接続を試みる（トークンがある前提）。
fn spawn_net_thread(
    bot_id: Option<String>,
    ui_rx: mpsc::Receiver<UiEvent>,
    net_tx: mpsc::Sender<NetEvent>,
) {
    std::thread::Builder::new()
        .name("yuuka-net".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2) // 軽量重視（NFR-1）。
                .enable_all()
                .build()
                .expect("failed to build tokio runtime");

            rt.block_on(net::run(bot_id, ui_rx, net_tx));
        })
        .expect("failed to spawn net thread");
}
