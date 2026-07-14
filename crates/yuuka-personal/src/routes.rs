//! personal ルートハンドラ（`/api/contacts*`・`/api/context-note`・`/api/clipboard*`・全て auth:user）。
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>` サブステートで取得、
//! スコープは **共通の [`resolve_scope`]**（`?botId=` を bot アクセス認可つきで解決・
//! 未アクセスは system_default にフォールバック）で束ねて repo に渡す。
//! 本ルータは supervisor 側で共通レイヤ（CSRF/body 上限）配下にマージされる。
//!
//! 参照スコープ = 連絡先のコア CRUD（list / save〔add|update〕 / delete）＋コンテキストノート
//! （GET 参照／POST 全体置換）＋クリップボード（GET 一覧／POST delete）。誕生日リマインド cron・
//! 連絡先の部分一致検索・クリップボード追加は deferred（lib.rs 参照）。
//!
//! 方針（M-12・全ドメイン共通）: delete の該当無は Node パリティで **200 `{success:false}`**
//! を返す（404 にしない・`deletedId` は返さない）。save の該当無更新は Node 同様 404。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yuuka_core::WebError;
use yuuka_types::{EmptyData, Envelope};
use yuuka_web::{resolve_scope, ApiError, AppState, AuthenticatedUser, Db, ScopedJson};

use crate::dto::{
    ClipboardListData, ContactData, ContactListData, ContextNoteData, NewContact, SetContextNote,
    CONTEXT_NOTE_MAX_LENGTH,
};
use crate::repo::{ClipboardRepo, ContactRepo, ContextNoteRepo};

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
        .route(
            "/api/context-note",
            get(context_note_get).post(context_note_set),
        )
        .route("/api/clipboard", get(clipboard_list))
        .route("/api/clipboard/delete", post(clipboard_delete))
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
    ScopedJson {
        bot_id,
        value: mut input,
    }: ScopedJson<NewContact>,
) -> Result<Json<Envelope<ContactData>>, ApiError> {
    // 氏名は必須（空白のみは不可）。前後空白は Node 同様に除去する。
    input.name = input.name.trim().to_owned();
    if input.name.is_empty() {
        return Err(ApiError(WebError::Validation(
            "name is required".to_owned(),
        )));
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
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<IdInput>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let ok = ContactRepo::new(&db).delete(&scope, input.id).await?;
    Ok(Json(Envelope::bare(ok)))
}

/// コンテキストノートの全文・更新時刻・上限を返す（Node `GET /api/context-note`）。
///
/// 未登録時は `{content:"", updated_at:null, max_length}` を返す（Node parity）。
async fn context_note_get(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Result<Json<Envelope<ContextNoteData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let (content, updated_at) = ContextNoteRepo::new(&db).get(&scope).await?;
    Ok(Json(Envelope::ok(ContextNoteData {
        content,
        updated_at,
        max_length: CONTEXT_NOTE_MAX_LENGTH,
    })))
}

/// コンテキストノートを全文置換する（Node `POST /api/context-note`）。
///
/// 上限超過（[`CONTEXT_NOTE_MAX_LENGTH`] 文字）は 400 で弾き、Node と同一の日本語メッセージを
/// 返す。文字数は Node の `content.length`（UTF-16 code unit）に対し Rust は `chars().count()`
/// で数える（BMP 内は一致・dto 参照）。保存後は Node 同様メッセージのみ返す。
async fn context_note_set(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<SetContextNote>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    let len = input.content.chars().count();
    if len > CONTEXT_NOTE_MAX_LENGTH {
        return Err(ApiError(WebError::Validation(format!(
            "コンテキストノートは{}文字以内です（現在: {}文字）",
            format_thousands(CONTEXT_NOTE_MAX_LENGTH),
            format_thousands(len),
        ))));
    }
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    ContextNoteRepo::new(&db).set(&scope, input.content).await?;
    Ok(Json(Envelope::ok_with_message(
        EmptyData {},
        "コンテキストノートを保存しました。",
    )))
}

/// 有効なクリップボードエントリ一覧を返す（Node `GET /api/clipboard`）。
async fn clipboard_list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Result<Json<Envelope<ClipboardListData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, q.bot_id.as_deref()).await?;
    let entries = ClipboardRepo::new(&db).list(&scope).await?;
    Ok(Json(Envelope::ok(ClipboardListData { entries })))
}

/// クリップボードエントリを削除する（Node `POST /api/clipboard/delete`）。
///
/// Node は削除可否で `{success, message}` を返す（該当無でも 200・メッセージ差替え）。
async fn clipboard_delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    ScopedJson {
        bot_id,
        value: input,
    }: ScopedJson<IdInput>,
) -> Result<Json<Envelope<EmptyData>>, ApiError> {
    let scope = resolve_scope(&user.0, &db, bot_id.as_deref()).await?;
    let ok = ClipboardRepo::new(&db).delete(&scope, input.id).await?;
    let message = if ok {
        "メモを削除しました。"
    } else {
        "メモが見つかりません。"
    };
    Ok(Json(if ok {
        Envelope::ok_with_message(EmptyData {}, message)
    } else {
        Envelope::err(EmptyData {}, message)
    }))
}

/// 整数を 3 桁区切りにする（Node `Number.prototype.toLocaleString()` の桁区切り相当）。
///
/// コンテキストノート上限メッセージの `10,000` 等を Node と一致させるため。末尾から 3 桁ごとに
/// カンマを差し込む（左端の余り桁は先頭グループとして残す・underflow を避ける実装）。
fn format_thousands(n: usize) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        // 残り桁数が 3 の倍数になる境界（先頭を除く）でカンマを置く。
        let remaining = len - i;
        if i != 0 && remaining.is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
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
