//! yuuka-supervisor（bin `yuuka`）— 起動シーケンスと HTTP 配信。
//!
//! 起動: telemetry 初期化 → `config.yaml`+env 読込（**欠落型不一致は fail-fast**）→ 既存 SQLite
//! を開く → Redis セッション接続（不可でも縮退で継続）→ 実 [`CompositeAuth`] 構築 →
//! ルータ組立（API + SPA 静的配信 + 共通レイヤ）→ `axum::serve` を graceful shutdown 付きで駆動。
//!
//! 未配線（後続増分）: bot/gemini/services を含む JoinSet 全体監督・指数バックオフ再 spawn。
//! 現状は web サービス単体を常駐させる（Node の web と同居可能＝strangler 移行）。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use yuuka_auth::{CompositeAuth, SessionStore};
use yuuka_core::Config;
use yuuka_supervisor::build_app;
use yuuka_web::{AppState, Db, WebConfig};

/// 設定ファイルの既定パス（cwd 相対・Node と同じ `config.yaml`）。
const CONFIG_PATH: &str = "config.yaml";
/// ビルド済み SPA の配信元（vite `outDir` = `dist/public`）。
const DIST_DIR: &str = "dist/public";

#[tokio::main]
async fn main() -> ExitCode {
    init_telemetry();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // 起動失敗は fail-fast（非ゼロ終了）。設定/DB 不備で中途半端に動かさない（§5.6）。
            tracing::error!(error = %e, "起動に失敗しました");
            ExitCode::FAILURE
        }
    }
}

/// 起動シーケンス本体（失敗は文字列化して呼び出し元で fail-fast）。
async fn run() -> Result<(), String> {
    // 1) config.yaml + 環境変数（欠落は既定へ・壊れた YAML/不正値は致命）。
    let cfg = Config::load_and_validate(Path::new(CONFIG_PATH))
        .map_err(|e| format!("config load failed: {e}"))?;

    // 2) 既存 SQLite（Node 作成済み前提）を read pool + 単一 writer actor で開く。
    let db = Db::open(&cfg.db_path).map_err(|e| format!("open db {:?}: {e}", cfg.db_path))?;

    // 3) Redis セッション（到達不能でも起動継続＝Cookie のみ縮退）。
    let sessions = SessionStore::connect(&cfg.redis_url).await;

    // 4) 実 AuthBackend（Cookie=Redis / Bearer=SQLite）。
    let auth = Arc::new(CompositeAuth::new(db.clone(), sessions, cfg.session_ttl_days));
    let web_config = WebConfig::from_core(&cfg);
    let state = AppState::new(auth, web_config, db);

    // 5) ルータ（API + SPA。dist/public があれば静的配信を載せる）。
    let dist = PathBuf::from(DIST_DIR);
    let dist_ref = dist.is_dir().then_some(dist.as_path());
    if dist_ref.is_none() {
        tracing::warn!(dir = DIST_DIR, "SPA ディレクトリが無いため静的配信を無効化（API のみ）");
    }
    let app = build_app(state, dist_ref);

    // 6) bind + serve（graceful shutdown 付き）。
    let addr = SocketAddr::new(cfg.host, cfg.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("bind {addr}: {e}"))?;
    tracing::info!(%addr, "yuuka web serving");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| format!("serve: {e}"))?;
    tracing::info!("yuuka web stopped");
    Ok(())
}

/// telemetry（tracing）初期化。`RUST_LOG` で制御、既定は info。
fn init_telemetry() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    // 二重初期化（テスト等）でも panic させない。
    let _ = fmt().with_env_filter(filter).try_init();
}

/// Ctrl-C または SIGTERM を待つ（graceful shutdown のトリガ）。
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "SIGTERM ハンドラ登録に失敗（Ctrl-C のみ有効）");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutdown signal を受信。graceful shutdown を開始");
}
