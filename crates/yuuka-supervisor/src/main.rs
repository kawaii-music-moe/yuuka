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
use axum::Router;
use yuuka_auth::{AuthRuntime, CompositeAuth, SessionStore};
use yuuka_core::secrets::ExposeSecret;
use yuuka_core::{Config, DbError};
use yuuka_crypto::{rotate_secret_key, SystemCrypto, LEGACY_FALLBACK_SECRET};
use yuuka_discord::{DiscordManager, ManagerPorts, Prepared};
use yuuka_orchestrator::{ChatEngine, DbBotDirectory, DbMembership, InMemoryRateLimiter};
use yuuka_services::{MetricsRegistry, ServiceContext};
use yuuka_supervisor::{
    build_app, build_supervised_services, build_tool_registry, ws_routes, DiscordTenantService,
    MessengerRegistrationDm, ServiceError, ShutdownToken, SupervisedService, Supervisor,
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

    // 1.5) 保存時暗号シークレットの必須 + 強度チェック（Node `index.ts` §6.2・**N2**）。未設定/脆弱鍵での
    //      起動を拒否する。これが無いと Rust は暗号鍵ゼロでも起動を続け（setup/register だけ 500 に縮退）、
    //      at-rest 秘密（Gemini/Discord/OAuth トークン）に依存する機能が静かに壊れる Rust 固有の退行になる。
    require_encryption_secret(&cfg)?;

    // 2) 既存 SQLite（Node 作成済み前提）を read pool + 単一 writer actor で開く
    //    （writer 起動時に migrations が適用される）。
    let db = Db::open(&cfg.db_path).map_err(|e| format!("open db {:?}: {e}", cfg.db_path))?;

    // 2.5) 鍵ローテーション（P1-5・Node `rotateSecretKey` パリティ）。`YUUKA_ENCRYPTION_SECRET_NEW`
    //      が設定されていれば、サービス起動前に **writer actor 上で 1 回だけ**全暗号化列を
    //      旧鍵→新鍵で再暗号化する（R-2: 第二 writer 経路を作らない）。migrations 適用後・
    //      web/cron が書き込みを始める前に完了させる（Node の同期起動ローテーションと同じ位置）。
    rotate_secret_if_requested(&db, &cfg).await?;

    // 3) Redis セッション（到達不能でも起動継続＝Cookie のみ縮退）。発行（login/setup）と検証
    //    （CompositeAuth）で同一ストアを共有する（同じ clone を両者へ渡す）。
    let sessions = SessionStore::connect(&cfg.redis_url).await;

    // 4) 実 AuthBackend（Cookie=Redis / Bearer=SQLite）。
    let auth = Arc::new(CompositeAuth::new(
        db.clone(),
        sessions.clone(),
        cfg.session_ttl_days,
    ));
    let web_config = WebConfig::from_core(&cfg);
    let state = AppState::new(auth, web_config, db.clone());

    // 4.5) 認証発行ランタイム（P1-1）。セッション発行・Gemini キー暗号化・保留登録・DM ポート・
    //      レート制限を束ねる。暗号は `YUUKA_ENCRYPTION_SECRET` 未設定なら `None`（setup/verify のみ
    //      500 に縮退・login 等は動作）。DM は Discord live（P1-3）まで `NullRegistrationDm`（register は
    //      502）。招待コードは起動時に冪等シードする。
    let crypto = match SystemCrypto::from_config(&cfg) {
        Ok(c) => Some(Arc::new(c)),
        Err(e) => {
            tracing::warn!(error = %e, "SystemCrypto を構築できません（setup/register は 500 に縮退・login 等は動作）");
            None
        }
    };

    // 4.6) 会話エンジン（ChatEngine）— `/ws/chat`（デスクトップ）を駆動する（P1-2）。ツールレジストリ +
    //      暗号（Gemini キー復号）+ DB を保持。crypto 未設定でも構築でき、キー未設定ユーザーは ⚠️ 応答。
    let tool_registry =
        build_tool_registry(&db).map_err(|e| format!("build tool registry: {e}"))?;
    let chat_engine = Arc::new(ChatEngine::with_real_gemini(
        db.clone(),
        crypto.clone(),
        tool_registry,
    ));
    let chat_ws_routes = ws_routes(chat_engine.clone(), cfg.desktop_max_upload_mb);

    // 4.7) Discord マルチテナント（P1-3）。実ポート（BotDirectory/RateLimiter/MembershipService）＋
    //      会話エンジン（processor）を注入して DiscordManager を組み、`prepare` でトークン解決 +
    //      共有 Messenger を作る（ここでは twilight REST クライアント生成のみ・gateway 未接続）。
    //      Messenger は登録コード DM（下）・cron 通知（P1-4）の共通配信基盤として使う。gateway の
    //      起動は後述の YUUKA_RUST_DISCORD ゲートで制御する（二重 gateway ＝二重応答の回避）。
    let discord_manager = DiscordManager::new(ManagerPorts {
        directory: Arc::new(DbBotDirectory::new(db.clone(), crypto.clone())),
        rate_limiter: Arc::new(InMemoryRateLimiter::new(db.clone())),
        processor: chat_engine.clone(),
        membership: Arc::new(DbMembership::new(db.clone())),
    });
    let Prepared { runners, messenger } = discord_manager.prepare().await;

    match yuuka_auth::invite::seed_initial_codes(&db, &cfg.invite_codes).await {
        Ok(n) if n > 0 => tracing::info!(seeded = n, "招待コードをシードしました"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "招待コードのシードに失敗（起動は継続）"),
    }
    // 登録コード DM は Discord Messenger 経由（P1-1 の `NullRegistrationDm` を差し替え）。デフォルト Bot
    // が未起動（トークン未登録）なら送信は false を返し `/api/register` は 502 に縮退する（従来と同挙動）。
    let auth_runtime = Arc::new(AuthRuntime::new(
        sessions,
        cfg.session_ttl_days,
        crypto,
        Arc::new(MessengerRegistrationDm::new(messenger.clone())),
        cfg.admin_discord_ids.clone(),
    ));
    let auth_routes = yuuka_auth::routes(auth_runtime);

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
        auth_routes,
        ws_routes: chat_ws_routes,
        addr,
        dist_dir,
    });
    let mut supervisor = Supervisor::new().service(web);

    // 6.5) Discord ゲートウェイ（P1-3・strangler カットオーバーの env ゲート）。移行期は Node が gateway
    //      を所有し、同一トークンで Rust も接続すると MESSAGE_CREATE が二重処理される（＝二重応答）。
    //      Node bot を停止したら `YUUKA_RUST_DISCORD=1` で各テナント（Shard poll ループ）を監督下へ置く
    //      （panic 隔離 + 指数バックオフ・恒久クローズ=無効トークン等は再起動しない）。REST 送信（登録
    //      DM・通知）は gateway 非依存のため本ゲートに関わらず messenger 経由で機能する。
    if rust_discord_enabled() {
        if runners.is_empty() {
            tracing::warn!(
                "YUUKA_RUST_DISCORD 有効ですが起動対象 Bot がありません（トークン未登録 or 暗号鍵未設定）"
            );
        }
        for runner in runners {
            tracing::info!(bot_id = %runner.bot_id(), "Discord テナントを監督下に配置");
            supervisor = supervisor.service(Arc::new(DiscordTenantService::new(runner)));
        }
    } else {
        drop(runners);
        tracing::info!(
            "Rust Discord ゲートウェイは無効（既定・Node が gateway を所有）。有効化は YUUKA_RUST_DISCORD=1（Node bot 停止後）"
        );
    }

    // 7) cron 常駐サービス群（Phase 4）。**strangler カットオーバー用の env ゲート**で制御する:
    //    移行期は Node が cron を所有し Rust は read-only（二重 writer 回避が絶対条件・R-1）。
    //    Node cron を停止したら `YUUKA_RUST_CRON=1` で Rust cron を起動する（reminder は起動時
    //    即時実行で取りこぼしを復帰・§10）。通知先 Discord は未配線のため縮退（NullNotifier）で
    //    始まり、リマインド等は配信可能になるまで pending のまま保持される。
    //    P1-4: `impl yuuka_services::Notifier for DiscordMessenger`（notify_bridge）+ 上の Discord
    //    Messenger 構築（P1-3）が揃ったので、通知先を実 Discord Messenger に配線する。デフォルト Bot が
    //    未起動ならリマインド等は送信 false のまま保持され、Bot 起動後に配信可能になる。
    if rust_cron_enabled() {
        // マクロ定期実行（playbook）は会話エンジンを秘書ターンとして起動する。services→orchestrator の
        // 逆依存を避けるため、両者を知る supervisor 層でアダプタ経由に注入する（P2 縮退の解消）。
        let service_ctx = ServiceContext::new(
            db,
            messenger,
            Arc::new(MetricsRegistry::new()),
            Arc::new(PlaybookRunnerAdapter {
                engine: chat_engine.clone(),
            }),
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

/// [`yuuka_services::PlaybookRunner`] を [`ChatEngine`] へ橋渡しするアダプタ（マクロ定期実行）。
///
/// `services → orchestrator` の逆依存（循環）を避けるため、両者を知る supervisor 層でブリッジする
/// （notify_bridge と同思想の孤児回避）。playbook は本人の Gemini キー/データで秘書ターンとして実行し、
/// 進捗プレゼンスは cron では不要なため no-op の [`StatusSink`](yuuka_discord::StatusSink) を渡す。
struct PlaybookRunnerAdapter {
    engine: Arc<ChatEngine>,
}

#[async_trait]
impl yuuka_services::PlaybookRunner for PlaybookRunnerAdapter {
    async fn run_secretary(
        &self,
        bot_id: &yuuka_core::BotId,
        user_id: &yuuka_core::UserId,
        prompt: String,
    ) -> Result<String, String> {
        let msg = yuuka_discord::IncomingChat {
            text: prompt,
            ..Default::default()
        };
        let status: yuuka_discord::StatusSink = Arc::new(|_| {});
        match self.engine.secretary_turn(bot_id, user_id, msg, &status).await {
            Ok(reply) => Ok(reply.text),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// 保存時暗号シークレットの起動時チェック（Node `index.ts` §6.2 パリティ・**N2**）。
///
/// `YUUKA_ENCRYPTION_SECRET`（と鍵ローテ用 `_NEW`）が **どちらも未設定なら起動拒否**。設定済みでも
/// 32 文字未満なら起動拒否する（脆弱鍵は KDF 強度に関わらず総当たりの前提条件になる）。`_NEW` のみ
/// 設定はプレリリース版からのローテーション起動として許可する（[`rotate_secret_if_requested`]）。
///
/// # Errors
/// 両シークレット未設定、またはいずれかが `MIN_SECRET_LEN` 未満のとき `Err`（起動中断＝非ゼロ終了）。
fn require_encryption_secret(cfg: &Config) -> Result<(), String> {
    // Node は `String.length`（UTF-16 code unit）。base64 秘密は ASCII なので `chars().count()` と一致。
    check_secret_strength(
        cfg.encryption_secret
            .as_ref()
            .map(|s| s.expose_secret().chars().count()),
        cfg.encryption_secret_new
            .as_ref()
            .map(|s| s.expose_secret().chars().count()),
    )
}

/// [`require_encryption_secret`] の純粋な判定部（テスト可能）。引数は各シークレットの文字長（未設定は `None`）。
///
/// # Errors
/// 両方 `None`（未設定）、またはいずれかが `MIN_SECRET_LEN` 未満のとき `Err`。
fn check_secret_strength(secret_len: Option<usize>, new_len: Option<usize>) -> Result<(), String> {
    /// Node `MIN_SECRET_LEN`（`index.ts`）。
    const MIN_SECRET_LEN: usize = 32;

    if secret_len.is_none() && new_len.is_none() {
        return Err(
            "YUUKA_ENCRYPTION_SECRET が未設定です。十分に長いランダム文字列（例: openssl rand -base64 48）を \
             設定してください。プレリリース版からの移行は YUUKA_ENCRYPTION_SECRET_NEW に新鍵を設定して起動 \
             （鍵ローテーション）します。"
                .to_owned(),
        );
    }
    for (name, len) in [
        ("YUUKA_ENCRYPTION_SECRET", secret_len),
        ("YUUKA_ENCRYPTION_SECRET_NEW", new_len),
    ] {
        if let Some(n) = len {
            if n < MIN_SECRET_LEN {
                return Err(format!(
                    "{name} が短すぎます（{n} 文字）。推測困難な {MIN_SECRET_LEN} 文字以上のランダム値 \
                     （例: openssl rand -base64 48）を設定してください。"
                ));
            }
        }
    }
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

/// Rust Discord ゲートウェイ（各テナントの Shard poll ループ）を起動するか（strangler カットオーバーの
/// env ゲート）。`YUUKA_RUST_DISCORD` が `1`/`true`/`yes`（大小無視）のときのみ有効。無効時も REST 送信
/// （登録 DM・通知）は messenger 経由で機能する（gateway 二重接続＝二重応答のみを避ける）。
fn rust_discord_enabled() -> bool {
    std::env::var("YUUKA_RUST_DISCORD")
        .ok()
        .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

/// web サーバを [`SupervisedService`] 化する（絶対制約2: panic 隔離＋バックオフ再起動）。
///
/// `run` は毎回ルータを組み立て直して bind→serve する。bind/serve 失敗は
/// `ServiceError::Transient`（supervisor が再起動）、shutdown での正常停止は Ok(())（再起動しない）。
struct WebService {
    state: AppState,
    /// 認証発行ルータ（`AuthRuntime` を `Extension` で内包済み・再起動毎に clone して merge）。
    auth_routes: Router<AppState>,
    /// 会話 WS ルータ（`ChatEngine` を `Extension` で内包済み・`/ws/chat`）。
    ws_routes: Router<AppState>,
    addr: SocketAddr,
    dist_dir: Option<PathBuf>,
}

#[async_trait]
impl SupervisedService for WebService {
    fn name(&self) -> String {
        "web".to_owned()
    }

    async fn run(&self, mut shutdown: ShutdownToken) -> Result<(), ServiceError> {
        let app = build_app(
            self.state.clone(),
            self.auth_routes.clone(),
            self.ws_routes.clone(),
            self.dist_dir.as_deref(),
        );
        let listener = tokio::net::TcpListener::bind(self.addr)
            .await
            .map_err(|e| ServiceError::transient(format!("bind {}: {e}", self.addr)))?;
        tracing::info!(addr = %self.addr, "yuuka web serving");
        // ConnectInfo<SocketAddr> を有効化し、レート制限のクライアント IP 解決（Node getClientIp
        // 相当）が peer アドレスを参照できるようにする（信頼プロキシ配下では XFF を優先）。
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
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

#[cfg(test)]
mod tests {
    use super::check_secret_strength;

    #[test]
    fn both_absent_is_rejected() {
        // N2 の核心: 暗号鍵が一切無い起動は拒否する（Node index.ts の process.exit(1) 相当）。
        assert!(check_secret_strength(None, None).is_err());
    }

    #[test]
    fn weak_secret_is_rejected() {
        assert!(check_secret_strength(Some(31), None).is_err());
        assert!(check_secret_strength(None, Some(10)).is_err());
    }

    #[test]
    fn strong_secret_is_accepted() {
        assert!(check_secret_strength(Some(32), None).is_ok());
        assert!(check_secret_strength(Some(48), None).is_ok());
        // 鍵ローテーション（_NEW のみ・十分長）も許可。
        assert!(check_secret_strength(None, Some(48)).is_ok());
    }
}
