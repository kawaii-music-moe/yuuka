//! 外部 Webhook 受信リポジトリ（Node `webhookRepo`・`webhook_endpoints`/`webhook_deliveries`）。
//!
//! シークレットは**暗号化済み 3 列**（`secret_encrypted`/`iv`/`tag`）で保存する（暗号化はルート層で
//! [`yuuka_crypto::SystemCrypto`] により行い、本リポジトリは平文シークレットに触れない）。安全ビューは
//! `has_secret` フラグのみを露出し、暗号文は決して JSON へ出さない（§3.13.4）。

use base64::Engine as _;
use rusqlite::{params, OptionalExtension, Row};
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// 暗号化済みシークレットの 3 列（hex）。
#[derive(Debug, Clone)]
pub struct EncryptedSecret {
    pub encrypted: String,
    pub iv: String,
    pub tag: String,
}

/// `webhook_endpoints` 1 行（`secret_*` は保持するが view には出さない）。
#[derive(Debug, Clone)]
pub struct WebhookEndpoint {
    pub id: i64,
    pub user_id: String,
    pub name: String,
    pub token: String,
    pub secret_encrypted: Option<String>,
    pub secret_iv: Option<String>,
    pub secret_tag: Option<String>,
    pub notify_target_type: String,
    pub notify_target_id: Option<String>,
    pub template: Option<String>,
    pub filter_keyword: Option<String>,
    pub create_todo: bool,
    pub create_reminder: bool,
    pub enabled: bool,
    pub created_at: String,
}

impl WebhookEndpoint {
    /// シークレットが設定されているか（view の `has_secret`）。
    #[must_use]
    pub fn has_secret(&self) -> bool {
        self.secret_encrypted
            .as_deref()
            .is_some_and(|s| !s.is_empty())
    }
}

/// `webhook_deliveries` 1 行（受信監査）。
#[derive(Debug, Clone)]
pub struct WebhookDelivery {
    pub id: i64,
    pub endpoint_id: i64,
    pub user_id: String,
    pub payload: String,
    pub status: String,
    pub detail: Option<String>,
    pub created_at: String,
}

/// エンドポイント作成入力（シークレットは暗号化済み・ルート層で暗号化して渡す）。
#[derive(Debug, Default)]
pub struct EndpointCreate {
    pub name: String,
    pub secret: Option<EncryptedSecret>,
    pub notify_target_type: String,
    pub notify_target_id: Option<String>,
    pub template: Option<String>,
    pub filter_keyword: Option<String>,
    pub create_todo: bool,
    pub create_reminder: bool,
}

/// エンドポイント部分更新（present な列だけ上書き・Node `updateEndpoint` の `key in obj` 意味論）。
#[derive(Debug, Default)]
pub struct EndpointPatch {
    pub name: Option<String>,
    /// `None`=変更なし・`Some(None)`=シークレット解除・`Some(Some)`=新シークレット（暗号化済み）。
    pub secret: Option<Option<EncryptedSecret>>,
    pub notify_target_type: Option<String>,
    pub notify_target_id: Option<Option<String>>,
    pub template: Option<Option<String>>,
    pub filter_keyword: Option<Option<String>>,
    pub create_todo: Option<bool>,
    pub create_reminder: Option<bool>,
    pub enabled: Option<bool>,
}

/// CSPRNG の URL トークン（Node `generateToken(24)`＝24 バイトの base64url・無パディング）。
#[must_use]
pub fn generate_token() -> String {
    let mut buf = [0u8; 24];
    let _ = getrandom::getrandom(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

fn row_to_endpoint(row: &Row) -> rusqlite::Result<WebhookEndpoint> {
    Ok(WebhookEndpoint {
        id: row.get("id")?,
        user_id: row.get("user_id")?,
        name: row.get("name")?,
        token: row.get("token")?,
        secret_encrypted: row.get("secret_encrypted")?,
        secret_iv: row.get("secret_iv")?,
        secret_tag: row.get("secret_tag")?,
        notify_target_type: row.get("notify_target_type")?,
        notify_target_id: row.get("notify_target_id")?,
        template: row.get("template")?,
        filter_keyword: row.get("filter_keyword")?,
        create_todo: row.get::<_, i64>("create_todo")? != 0,
        create_reminder: row.get::<_, i64>("create_reminder")? != 0,
        enabled: row.get::<_, i64>("enabled")? != 0,
        created_at: row.get("created_at")?,
    })
}

/// エンドポイントを作成し、作成後の行を返す（Node `createEndpoint`）。
///
/// # Errors
/// 書き込み・取得失敗時 [`DbError`]。
pub async fn create_endpoint(
    db: &Db,
    user_id: &str,
    input: EndpointCreate,
) -> Result<WebhookEndpoint, DbError> {
    let uid = user_id.to_owned();
    let token = generate_token();
    db.writer
        .transaction(move |tx| {
            let (enc, iv, tag) = match input.secret {
                Some(s) => (Some(s.encrypted), Some(s.iv), Some(s.tag)),
                None => (None, None, None),
            };
            tx.execute(
                "INSERT INTO webhook_endpoints \
                   (user_id, name, token, secret_encrypted, secret_iv, secret_tag, \
                    notify_target_type, notify_target_id, template, filter_keyword, \
                    create_todo, create_reminder) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    uid,
                    input.name,
                    token,
                    enc,
                    iv,
                    tag,
                    input.notify_target_type,
                    input.notify_target_id,
                    input.template,
                    input.filter_keyword,
                    i64::from(input.create_todo),
                    i64::from(input.create_reminder),
                ],
            )
            .map_err(map_sqlite)?;
            let id = tx.last_insert_rowid();
            tx.query_row(
                "SELECT * FROM webhook_endpoints WHERE id = ?1",
                params![id],
                row_to_endpoint,
            )
            .map_err(map_sqlite)
        })
        .await
}

/// 本人スコープで 1 件取得（Node `getEndpointById`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_endpoint(
    db: &Db,
    user_id: &str,
    id: i64,
) -> Result<Option<WebhookEndpoint>, DbError> {
    let uid = user_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT * FROM webhook_endpoints WHERE user_id = ?1 AND id = ?2",
                params![uid, id],
                row_to_endpoint,
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// 受信ルート用: トークンで解決（公開ルートのため user_id 条件なし・Node `getEndpointByToken`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_endpoint_by_token(
    db: &Db,
    token: &str,
) -> Result<Option<WebhookEndpoint>, DbError> {
    let token = token.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT * FROM webhook_endpoints WHERE token = ?1",
                params![token],
                row_to_endpoint,
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// 本人のエンドポイント一覧（created_at DESC・Node `listEndpoints`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_endpoints(db: &Db, user_id: &str) -> Result<Vec<WebhookEndpoint>, DbError> {
    let uid = user_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM webhook_endpoints WHERE user_id = ?1 ORDER BY created_at DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![uid], row_to_endpoint)
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// エンドポイントを部分更新（Node `updateEndpoint`＝現在値へ present 列を重ねる）。行が動けば `true`。
///
/// # Errors
/// 読み取り・書き込み失敗時 [`DbError`]。
pub async fn update_endpoint(
    db: &Db,
    user_id: &str,
    id: i64,
    patch: EndpointPatch,
) -> Result<bool, DbError> {
    let Some(current) = get_endpoint(db, user_id, id).await? else {
        return Ok(false);
    };
    // present 列を現在値へ重ねる。
    let name = patch.name.unwrap_or(current.name);
    let (enc, iv, tag) = match patch.secret {
        None => (
            current.secret_encrypted,
            current.secret_iv,
            current.secret_tag,
        ),
        Some(None) => (None, None, None),
        Some(Some(s)) => (Some(s.encrypted), Some(s.iv), Some(s.tag)),
    };
    let notify_type = patch
        .notify_target_type
        .unwrap_or(current.notify_target_type);
    let notify_id = match patch.notify_target_id {
        Some(v) => v,
        None => current.notify_target_id,
    };
    let template = match patch.template {
        Some(v) => v,
        None => current.template,
    };
    let filter = match patch.filter_keyword {
        Some(v) => v,
        None => current.filter_keyword,
    };
    let create_todo = patch.create_todo.unwrap_or(current.create_todo);
    let create_reminder = patch.create_reminder.unwrap_or(current.create_reminder);
    let enabled = patch.enabled.unwrap_or(current.enabled);

    let uid = user_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "UPDATE webhook_endpoints SET \
                       name = ?1, secret_encrypted = ?2, secret_iv = ?3, secret_tag = ?4, \
                       notify_target_type = ?5, notify_target_id = ?6, template = ?7, \
                       filter_keyword = ?8, create_todo = ?9, create_reminder = ?10, enabled = ?11 \
                     WHERE user_id = ?12 AND id = ?13",
                    params![
                        name,
                        enc,
                        iv,
                        tag,
                        notify_type,
                        notify_id,
                        template,
                        filter,
                        i64::from(create_todo),
                        i64::from(create_reminder),
                        i64::from(enabled),
                        uid,
                        id,
                    ],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// エンドポイントを削除（Node `deleteEndpoint`）。行が消えたら `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn delete_endpoint(db: &Db, user_id: &str, id: i64) -> Result<bool, DbError> {
    let uid = user_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "DELETE FROM webhook_endpoints WHERE user_id = ?1 AND id = ?2",
                    params![uid, id],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// 受信ペイロードの監査記録（payload は 8KB へ truncate・Node `addDelivery`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn add_delivery(
    db: &Db,
    endpoint_id: i64,
    user_id: &str,
    payload: &str,
    status: &str,
    detail: Option<&str>,
) -> Result<(), DbError> {
    let uid = user_id.to_owned();
    // 8192 バイト境界を UTF-8 文字境界へ丸めて truncate。
    let payload = {
        let mut end = payload.len().min(8192);
        while end > 0 && !payload.is_char_boundary(end) {
            end -= 1;
        }
        payload[..end].to_owned()
    };
    let (status, detail) = (status.to_owned(), detail.map(str::to_owned));
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO webhook_deliveries (endpoint_id, user_id, payload, status, detail) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![endpoint_id, uid, payload, status, detail],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 受信履歴（id DESC・`endpoint_id` 絞り込み可・Node `listDeliveries`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_deliveries(
    db: &Db,
    user_id: &str,
    endpoint_id: Option<i64>,
    limit: i64,
) -> Result<Vec<WebhookDelivery>, DbError> {
    let uid = user_id.to_owned();
    db.read
        .read(move |conn| {
            let mut out = Vec::new();
            let map = |row: &Row| {
                Ok(WebhookDelivery {
                    id: row.get("id")?,
                    endpoint_id: row.get("endpoint_id")?,
                    user_id: row.get("user_id")?,
                    payload: row.get("payload")?,
                    status: row.get("status")?,
                    detail: row.get("detail")?,
                    created_at: row.get("created_at")?,
                })
            };
            if let Some(eid) = endpoint_id {
                let mut stmt = conn
                    .prepare(
                        "SELECT * FROM webhook_deliveries WHERE user_id = ?1 AND endpoint_id = ?2 \
                         ORDER BY id DESC LIMIT ?3",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, eid, limit], map)
                    .map_err(map_sqlite)?;
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
            } else {
                let mut stmt = conn
                    .prepare(
                        "SELECT * FROM webhook_deliveries WHERE user_id = ?1 \
                         ORDER BY id DESC LIMIT ?2",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, limit], map)
                    .map_err(map_sqlite)?;
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
            }
            Ok(out)
        })
        .await
}
