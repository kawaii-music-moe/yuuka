//! personal（連絡先）ルートハンドラ（`/api/contacts*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未アクセスは system_default にフォールバック）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! 参照スコープ = 連絡先のコア CRUD（list / save〔add|update〕 / delete）。誕生日リマインド
//! cron・部分一致検索・コンテキストノート・クリップボードは deferred（lib.rs 参照）。
//!
//! 方針（全ドメイン共通）: mutation の該当無は **404**（Node は delete で 200 を返すが、
//! Rust はより厳密に 404。golden test 段階で最終確定する）。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::Envelope;
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{ContactData, ContactDeletedData, ContactListData, NewContact};
use crate::repo::ContactRepo;

#[derive(Debug, Deserialize)]
struct BotQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdInput {
    id: i64,
}

/// personal ドメインのルータ（`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/contacts", get(list))
        .route("/api/contacts/save", post(save))
        .route("/api/contacts/delete", post(delete))
}

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Result<Json<Envelope<ContactListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let contacts = ContactRepo::new(&db).list(&scope).await?;
    Ok(Json(Envelope::ok(ContactListData { contacts })))
}

/// 連絡先の作成／更新（Node `contacts/save`）。`id` 有→更新、無→新規。
///
/// 両分岐とも作成／更新後の行を `{contact}` で返す（Node は更新時にメッセージのみだが、
/// CRUD スライスでは更新後の行を返す方を採る）。該当行なしの更新は 404。
async fn save(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson { bot_id, value: mut input }: ScopedJson<NewContact>,
) -> Result<Json<Envelope<ContactData>>, ApiError> {
    // 氏名は必須（空白のみは不可）。前後空白は Node 同様に除去する。
    input.name = input.name.trim().to_owned();
    if input.name.is_empty() {
        return Err(ApiError(WebError::Validation("name is required".to_owned())));
    }
    // 誕生日は 'YYYY-MM-DD' または '--MM-DD'。空文字は None に正規化。
    input.birthday = normalize_optional(input.birthday);
    if let Some(bday) = input.birthday.as_deref() {
        if !is_valid_birthday(bday) {
            return Err(ApiError(WebError::Validation(
                "birthday must be YYYY-MM-DD or --MM-DD".to_owned(),
            )));
        }
    }
    input.relationship = normalize_optional(input.relationship);
    input.contact_info = normalize_optional(input.contact_info);
    input.notes = normalize_optional(input.notes);

    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let repo = ContactRepo::new(&db);

    let contact = match input.id {
        Some(id) => match repo.update(&scope, id, input).await? {
            Some(contact) => contact,
            None => return Err(ApiError(WebError::NotFound)),
        },
        None => repo.add(&scope, input).await?,
    };
    Ok(Json(Envelope::ok(ContactData { contact })))
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson { bot_id, value: input }: ScopedJson<IdInput>,
) -> Result<Json<Envelope<ContactDeletedData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    if ContactRepo::new(&db).delete(&scope, input.id).await? {
        Ok(Json(Envelope::ok(ContactDeletedData {
            deleted_id: input.id,
        })))
    } else {
        Err(ApiError(WebError::NotFound))
    }
}

/// 空／空白のみの文字列を `None` に、それ以外は trim して `Some` にする（Node の
/// `typeof x === "string" && x.trim() ? x.trim() : null` 相当）。
fn normalize_optional(value: Option<String>) -> Option<String> {
    match value {
        Some(s) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_owned())
            }
        }
        None => None,
    }
}

/// `birthday` の形式検証（`'YYYY-MM-DD'` または `'--MM-DD'`）。Node `isValidBirthday` 相当。
///
/// パターン文字列を走査（`D`=ASCII数字, `-`=ハイフン）し、indexing を避けて判定する。
fn is_valid_birthday(birthday: &str) -> bool {
    matches_pattern(birthday, "DDDD-DD-DD") || matches_pattern(birthday, "--DD-DD")
}

/// `pattern`（`D`=ASCII数字・その他=リテラル）に完全一致するか。
fn matches_pattern(value: &str, pattern: &str) -> bool {
    if value.len() != pattern.len() {
        return false;
    }
    value.bytes().zip(pattern.bytes()).all(|(v, p)| match p {
        b'D' => v.is_ascii_digit(),
        other => v == other,
    })
}
