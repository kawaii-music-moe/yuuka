//! credential ルートハンドラ（`/api/credentials*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未アクセスは system_default にフォールバック）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! **機密ドメイン**: 返す DTO は暗号化列・鍵材料を持たない（[`crate::dto`] のクリーンビュー）。
//!
//! 方針（M-12・全ドメイン共通）: delete の該当無は Node パリティで **200 `{success:false}`**
//! を返す（404 にしない・削除した service_name は返さない）。
//!
//! **deferred（コア CRUD 外・後回し）**: `POST /api/credentials/register`（secretService の
//! ユーザー鍵暗号化 Argon2id+AES-256-GCM が必要・本クレート外）、GET 一覧の
//! `bot_credential_access` 許可フィルタ、grant/revoke 連携。詳細は lib.rs docstring。

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Extension, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::{BotId, UserId, UserScope, WebError};
use yuuka_crypto::SystemCrypto;
use yuuka_types::{EmptyData, Envelope};
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::access::CredentialAccessRepo;
use crate::dto::{Credential, CredentialListData, DeleteCredential};
use crate::repo::{check_field_lengths, normalize_service_name, CredentialRepo};

#[derive(Debug, Deserialize)]
struct BotQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

/// `POST /api/credentials/register` の body（camelCase・全項目任意で受け、検証は Node と同順で行う）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterCredential {
    #[serde(default)]
    service_name: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

/// register の暗号化に使う crypto（省略可・Extension で運ぶ newtype・webhook ルータと同一方式）。
#[derive(Clone)]
struct CredentialCrypto(Option<Arc<SystemCrypto>>);

/// credential ドメインのルータ（crypto なし・テスト/後方互換用）。
pub fn routes() -> Router<AppState> {
    routes_with(None)
}

/// crypto を注入して credential ルータを組む（register の保存時暗号化に使う）。
pub fn routes_with(crypto: Option<Arc<SystemCrypto>>) -> Router<AppState> {
    Router::new()
        .route("/api/credentials", get(list))
        .route("/api/credentials/register", post(register))
        .route("/api/credentials/delete", post(delete))
        .layer(Extension(CredentialCrypto(crypto)))
}

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Result<Json<Envelope<CredentialListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    // v5: 認証情報は (owner=user) 所有のまま、応対 Bot へ利用許可済み（bot_credential_access）の
    // service だけ返す（ランタイムの isCredentialGrantedToBot ゲート・LLM listCredentialServices と一致）。
    // 未許可 service は表示しない（「一覧は全件見えるが利用時に弾かれる」認識の不整合を防ぐ）。
    let granted: HashSet<String> = CredentialAccessRepo::new(&db)
        .list_credential_names_for_bot(scope.bot_id().as_str(), scope.user_id().as_str())
        .await?
        .into_iter()
        .collect();
    let credentials: Vec<Credential> = CredentialRepo::new(&db)
        .list(&scope)
        .await?
        .into_iter()
        .filter(|c| granted.contains(&c.service_name))
        .collect();
    Ok(Json(Envelope::ok(CredentialListData { credentials })))
}

/// `POST /api/credentials/register` — 認証情報を暗号化保存し、owner 本人の全 Bot ＋ system_default へ
/// 利用許可する（Node `credentialRoutes` register + `secretService.registerCredential` パリティ）。
///
/// 検証失敗・暗号化不能はいずれも Node と同じ **400 `{success:false, message}`**（`WebError::Validation`）。
async fn register(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(CredentialCrypto(crypto)): Extension<CredentialCrypto>,
    ScopedJson {
        value: input,
        bot_id: _,
    }: ScopedJson<RegisterCredential>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    let service = input.service_name.unwrap_or_default();
    let username = input.username.unwrap_or_default();
    let password = input.password.unwrap_or_default();
    // 未入力（欠落・空文字）は Node のルート直下チェック（`!serviceName || ...`・trim 前）と一致。
    if service.is_empty() || username.is_empty() || password.is_empty() {
        return Err(ApiError(WebError::Validation(
            "サービス名、ユーザー名、およびパスワードは必須です。".to_owned(),
        )));
    }
    // 以降は secretService.registerCredential 相当（正規化 → 検証 → salt → 暗号化 → 保存）。
    let clean_service = normalize_service_name(&service);
    if clean_service.is_empty() {
        return Err(ApiError(WebError::Validation("サービス名が空です。".to_owned())));
    }
    let clean_username = username.trim().to_owned();
    if clean_username.is_empty() || password.is_empty() {
        return Err(ApiError(WebError::Validation(
            "ユーザー名とパスワードは必須です。".to_owned(),
        )));
    }
    let clean_url = input
        .url
        .map(|u| u.trim().to_owned())
        .filter(|s| !s.is_empty());
    if let Some(msg) = check_field_lengths(
        &clean_service,
        Some(&clean_username),
        Some(&password),
        clean_url.as_deref(),
    ) {
        return Err(ApiError(WebError::Validation(msg)));
    }

    let user_id = user.0.discord_id.clone();
    let repo = CredentialRepo::new(&db);
    // ユーザー鍵導出ソルト（未登録ユーザーはパスワードマネージャを利用できない・Node `requireUserSalt`）。
    let Some(salt) = repo.user_salt(&user_id).await? else {
        return Err(ApiError(WebError::Validation(
            "ユーザーが登録されていないため、パスワードマネージャを利用できません。先にユーザー登録を完了してください。".to_owned(),
        )));
    };
    // 暗号化（crypto 未設定＝鍵無しは Node の catch 相当で 400 汎用文言へ縮退。秘密値は文言に含めない）。
    let Some(crypto) = crypto else {
        return Err(ApiError(WebError::Validation(
            "資格情報の登録に失敗しました。".to_owned(),
        )));
    };
    let enc = crypto.encrypt_for_user(&salt, &password).map_err(|_| {
        ApiError(WebError::Validation(
            "資格情報の登録に失敗しました。".to_owned(),
        ))
    })?;
    // credentials は (user_id, service_name) 所有（bot_id 非依存）。scope の bot は使われない。
    let scope = UserScope::new(UserId::new(user_id.clone()), BotId::system_default());
    repo.save(&scope, &clean_service, clean_username, clean_url, enc)
        .await?;
    // 登録＝利用可能を保証（owner 本人の全 Bot + system_default へ冪等付与・Node grantCredentialToOwnerBots）。
    CredentialAccessRepo::new(&db)
        .grant_to_owner_bots(&user_id, &clean_service)
        .await?;
    Ok(Json(Envelope::ok_with_message(
        EmptyData {},
        "資格情報を正常に登録しました。",
    )))
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<DeleteCredential>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    // Node は `!serviceName`（trim せず空文字のみ）で 400。空白のみは通し、正規化後に不一致となり
    // 200 {success:false} の no-op になる（Node パリティ・メッセージも Node と一致）。
    if input.service_name.is_empty() {
        return Err(ApiError(WebError::Validation(
            "サービス名は必須です。".to_owned(),
        )));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let ok = CredentialRepo::new(&db)
        .delete(&scope, &input.service_name)
        .await?;
    // 削除成功時は全 Bot の利用許可も掃除する（credentials への DB FK が無いため明示的に。残すと
    // 同名再登録時に意図せず許可が復活する・Node `deleteAllGrantsForCredential`）。
    if ok {
        CredentialAccessRepo::new(&db)
            .delete_all_grants(scope.user_id().as_str(), &input.service_name)
            .await?;
    }
    Ok(Json(Envelope::bare(ok)))
}
