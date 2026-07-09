//! yuuka-supervisor（bin `yuuka`）— 起動シーケンスと監督。
//!
//! 起動: telemetry 初期化 → `config.yaml`+env 読込（**欠落型不一致は fail-fast**）→ 既存 SQLite
//! を開く → Redis セッション接続（不可でも縮退で継続）→ 実 [`CompositeAuth`] 構築 →
//! web サービスを [`Supervisor`] 配下へ登録 → JoinSet 監督ループを graceful shutdown 付きで駆動。
//!
//! web は **supervised task**（panic 隔離＋指数バックオフ再起動・絶対制約2）として動く。
//! bot/gemini/services は Phase 3/4 で `Supervisor::service` に追加していく（同一監督下）。
//! Node の web と同居可能＝strangler 移行。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use async_trait::async_trait;
use yuuka_auth::{CompositeAuth, SessionStore};
use yuuka_core::secrets::ExposeSecret;
use yuuka_core::{Config, DbError};
use yuuka_crypto::{rotate_secret_key, LEGACY_FALLBACK_SECRET};
use yuuka_services::{MetricsRegistry, NullNotifier, ServiceContext};
use yuuka_supervisor::{
    build_app, build_supervised_services, ServiceError, ShutdownToken, SupervisedService, Supervisor,
};
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

/// 起動シーケンス本体（起動時 config/DB 不備のみ fail-fast・§5.6/§5.7）。
async fn run() -> Result<(), String> {
    // 1) config.yaml + 環境変数（欠落は既定へ・壊れた YAML/不正値は致命）。
    let cfg = Config::load_and_validate(Path::new(CONFIG_PATH))
        .map_err(|e| format!("config load failed: {e}"))?;

    // 2) 既存 SQLite（Node 作成済み前提）を read pool + 単一 writer actor で開く
    //    （writer 起動時に migrations が適用される）。
    let db = Db::open(&cfg.db_path).map_err(|e| format!("open db {:?}: {e}", cfg.db_path))?;

    // 2.5) 鍵ローテーション（P1-5・Node `rotateSecretKey` パリティ）。`YUUKA_ENCRYPTION_SECRET_NEW`
    //      が設定されていれば、サービス起動前に **writer actor 上で 1 回だけ**全暗号化列を
    //      旧鍵→新鍵で再暗号化する（R-2: 第二 writer 経路を作らない）。migrations 適用後・
    //      web/cron が書き込みを始める前に完了させる（Node の同期起動ローテーションと同じ位置）。
    rotate_secret_if_requested(&db, &cfg).await?;

    // 3) Redis セッション（到達不能でも起動継続＝Cookie のみ縮退）。
    let sessions = SessionStore::connect(&cfg.redis_url).await;

    // 4) 実 AuthBackend（Cookie=Redis / Bearer=SQLite）。
    let auth = Arc::new(CompositeAuth::new(db.clone(), sessions, cfg.session_ttl_days));
    let web_config = WebConfig::from_core(&cfg);
    let state = AppState::new(auth, web_config, db.clone());

    // 5) 静的配信元（dist/public があれば SPA を載せる）。
    let dist = PathBuf::from(DIST_DIR);
    let dist_dir = dist.is_dir().then_some(dist);
    if dist_dir.is_none() {
        tracing::warn!(dir = DIST_DIR, "SPA ディレクトリが無いため静的配信を無効化（API のみ）");
    }

    // 6) web を supervised task として登録し、JoinSet 監督ループを駆動する。
    //    ここから先は「落ちない」— web の panic/一過性障害は隔離＋指数バックオフ再起動される。
    let addr = SocketAddr::new(cfg.host, cfg.port);
    let web = Arc::new(WebService {
        state,
        addr,
        dist_dir,
    });
    let mut supervisor = Supervisor::new().service(web);

    // 7) cron 常駐サービス群（Phase 4）。**strangler カットオーバー用の env ゲート**で制御する:
    //    移行期は Node が cron を所有し Rust は read-only（二重 writer 回避が絶対条件・R-1）。
    //    Node cron を停止したら `YUUKA_RUST_CRON=1` で Rust cron を起動する（reminder は起動時
    //    即時実行で取りこぼしを復帰・§10）。通知先 Discord は未配線のため縮退（NullNotifier）で
    //    始まり、リマインド等は配信可能になるまで pending のまま保持される。
    //    P1-4: `impl yuuka_services::Notifier for DiscordMessenger`（notify_bridge）は実装済み。
    //    Discord live 化（P1-3）で `Arc<DiscordMessenger>` を構築できたら、ここの `NullNotifier` を
    //    それへ差し替えるだけで実 Discord 配信へ切り替わる（アダプタは配線待ち）。
    if rust_cron_enabled() {
        let service_ctx = ServiceContext::new(
            db,
            Arc::new(NullNotifier),
            Arc::new(MetricsRegistry::new()),
        );
        let cron = build_supervised_services(&service_ctx);
        tracing::warn!(
            count = cron.len(),
            "YUUKA_RUST_CRON 有効: Rust cron 常駐サービスを監督下に配置（Node cron が停止済みであること）"
        );
        for svc in cron {
            supervisor = supervisor.service(svc);
        }
    } else {
        tracing::info!(
            "Rust cron は無効（既定・Node が cron を所有）。有効化は YUUKA_RUST_CRON=1（Node cron 停止後）"
        );
    }

    tracing::info!(%addr, "yuuka supervisor 起動（web を監督下に配置）");
    supervisor.run(shutdown_signal()).await;
    tracing::info!("yuuka supervisor stopped");
    Ok(())
}

/// `YUUKA_ENCRYPTION_SECRET_NEW` が設定されていれば鍵ローテーションを実行する（Node パリティ）。
///
/// 旧鍵は現行 `YUUKA_ENCRYPTION_SECRET`、未設定ならプレリリース版フォールバック鍵
/// （Node `rotateSecretKey` と同一のレスキュー動作）。writer actor 上で単一 Tx を回し、
/// 1 件でも復号失敗すれば全ロールバックして起動を fail-fast させる（部分適用を残さない）。
///
/// # Errors
/// ローテーション（DB/復号/鍵導出）失敗で `Err`（起動中断）。
async fn rotate_secret_if_requested(db: &Db, cfg: &Config) -> Result<(), String> {
    let Some(new_secret) = cfg.encryption_secret_new.as_ref() else {
        return Ok(()); // _NEW 未設定＝通常起動（ローテーションしない）。
    };
    let new = new_secret.expose_secret().to_owned();

    // 旧鍵: 現行 secret。未設定なら既知フォールバック鍵からの移行（漏えい済みとみなす）。
    let old = match cfg.encryption_secret.as_ref() {
        Some(cur) => cur.expose_secret().to_owned(),
        None => {
            tracing::warn!(
                "YUUKA_ENCRYPTION_SECRET 未設定のためプレリリース版フォールバック鍵で復号して再暗号化します。\
                 移行後は保存済みトークン/APIキー等を各プロバイダ側で必ずローテーションしてください"
            );
            LEGACY_FALLBACK_SECRET.to_owned()
        }
    };

    let rotated = db
        .writer
        .execute(move |conn| {
            rotate_secret_key(conn, &old, &new).map_err(|e| DbError::Operation(e.to_string()))
        })
        .await
        .map_err(|e| format!("secret rotation failed: {e}"))?;

    tracing::warn!(
        rotated,
        "YUUKA_ENCRYPTION_SECRET ローテーション完了。次手順: _NEW の値を _SECRET に昇格し _NEW を削除して再起動"
    );
    Ok(())
}

/// Rust cron 常駐サービスを起動するか（strangler カットオーバーの env ゲート）。
/// `YUUKA_RUST_CRON` が `1`/`true`/`yes`（大小無視）のときのみ有効。
fn rust_cron_enabled() -> bool {
    std::env::var("YUUKA_RUST_CRON")
        .ok()
        .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

/// web サーバを [`SupervisedService`] 化する（絶対制約2: panic 隔離＋バックオフ再起動）。
///
/// `run` は毎回ルータを組み立て直して bind→serve する。bind/serve 失敗は
/// `ServiceError::Transient`（supervisor が再起動）、shutdown での正常停止は Ok(())（再起動しない）。
struct WebService {
    state: AppState,
    addr: SocketAddr,
    dist_dir: Option<PathBuf>,
}

#[async_trait]
impl SupervisedService for WebService {
    fn name(&self) -> String {
        "web".to_owned()
    }

    async fn run(&self, mut shutdown: ShutdownToken) -> Result<(), ServiceError> {
        let app = build_app(self.state.clone(), self.dist_dir.as_deref());
        let listener = tokio::net::TcpListener::bind(self.addr)
            .await
            .map_err(|e| ServiceError::transient(format!("bind {}: {e}", self.addr)))?;
        tracing::info!(addr = %self.addr, "yuuka web serving");
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { shutdown.cancelled().await })
            .await
            .map_err(|e| ServiceError::transient(format!("serve: {e}")))?;
        tracing::info!("yuuka web stopped");
        Ok(())
    }
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
