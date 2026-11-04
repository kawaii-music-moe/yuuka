//! `/api/client/voices*` — PWA の音声チャットで使う声（Irodori-TTS の話者条件）の保存。
//!
//! 声は Bot ではなくユーザー本人のもの（どのエージェントと話しても同じ声で読める）なので、
//! `?botId=` は取らず [`AuthenticatedUser`] だけで所有者を決める（チャット添付と同じ）。全クエリを
//! `user_id` で絞り、他人の声は存在しないものとして 404 にする。
//!
//! PWA は声のサンプルの音声そのものを送らない。端末の GPU で話者エンコーダに通した結果（話者条件）
//! だけを受け取り、BLOB として保存する。一覧は軽い形（本体なし）で返し、本体は
//! `GET /api/client/voices/{id}/state` で生のバイト列として配る（PWA は端末にキャッシュする）。

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use rusqlite::{params, OptionalExtension, Row};
use yuuka_db::map_sqlite;
use yuuka_web::{AppState, AuthenticatedUser, Db};

use crate::dto::{VoiceCreateInput, VoicePatch, VoiceView};
use crate::error::ClientApiError;

/// 話者条件の 1 トークンの次元（Irodori-TTS v4.1 の speaker_dim）。
pub const SPEAKER_DIM: usize = 768;
/// 1 つの声のトークン数の上限（120 秒のサンプルで約 750）。
pub const MAX_TOKENS: usize = 1024;
/// 1 ユーザーが保存できる声の数。
pub const MAX_VOICES_PER_USER: i64 = 30;
pub const NAME_MAX_CHARS: usize = 40;
pub const CAPTION_MAX_CHARS: usize = 200;
const SPEAKER_MODEL_MAX_CHARS: usize = 64;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/client/voices", get(list).post(create))
        .route(
            "/api/client/voices/{id}",
            axum::routing::patch(update).delete(remove),
        )
        .route("/api/client/voices/{id}/state", get(state))
}

const COLUMNS: &str =
    "id, name, caption, source, seconds, speaker_model, tokens, speaker_mask, created_at, updated_at";

fn view(r: &Row<'_>) -> rusqlite::Result<VoiceView> {
    let mask: String = r.get(7)?;
    Ok(VoiceView {
        id: r.get::<_, i64>(0)?.to_string(),
        name: r.get(1)?,
        caption: r.get(2)?,
        source: r.get(3)?,
        seconds: r.get(4)?,
        speaker_model: r.get(5)?,
        tokens: r.get(6)?,
        // 書き込み時に検証済みの JSON。壊れていたら全トークン有効として扱う（話者条件は使える）。
        mask: serde_json::from_str(&mask).unwrap_or_default(),
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

fn parse_id(raw: &str) -> Result<i64, ClientApiError> {
    raw.parse::<i64>()
        .map_err(|_| ClientApiError::not_found("voice not found"))
}

fn trimmed(value: &str, max: usize, field: &str) -> Result<String, ClientApiError> {
    let value = value.trim();
    if value.chars().count() > max {
        return Err(ClientApiError::bad_request(format!(
            "{field} は {max} 文字以内にしてください"
        )));
    }
    Ok(value.to_owned())
}

/// 作成リクエストを検証し、保存する値（話者条件のバイト列を含む）にする。
fn validate(input: VoiceCreateInput) -> Result<NewVoice, ClientApiError> {
    let name = trimmed(&input.name, NAME_MAX_CHARS, "名前")?;
    if name.is_empty() {
        return Err(ClientApiError::bad_request("名前を入力してください"));
    }
    let caption = trimmed(&input.caption, CAPTION_MAX_CHARS, "話し方の指定")?;
    if !matches!(input.source.as_str(), "sample" | "design") {
        return Err(ClientApiError::bad_request("source が不正です"));
    }
    if !input.seconds.is_finite() || !(0.0..=600.0).contains(&input.seconds) {
        return Err(ClientApiError::bad_request("seconds が不正です"));
    }
    let speaker_model = input.speaker_model.trim().to_owned();
    if speaker_model.is_empty() || speaker_model.chars().count() > SPEAKER_MODEL_MAX_CHARS {
        return Err(ClientApiError::bad_request("speakerModel が不正です"));
    }
    let tokens = input.mask.len();
    if tokens == 0 || tokens > MAX_TOKENS {
        return Err(ClientApiError::bad_request("声のデータが長すぎるか空です"));
    }
    let state = base64::engine::general_purpose::STANDARD
        .decode(input.state_base64.as_bytes())
        .map_err(|_| ClientApiError::bad_request("stateBase64 が不正です"))?;
    if state.len() != tokens * SPEAKER_DIM * 4 {
        return Err(ClientApiError::bad_request(
            "声のデータの大きさが mask と合いません",
        ));
    }
    let mask = serde_json::to_string(&input.mask).map_err(ClientApiError::internal)?;
    Ok(NewVoice {
        name,
        caption,
        source: input.source,
        seconds: input.seconds,
        speaker_model,
        tokens: i64::try_from(tokens).map_err(ClientApiError::internal)?,
        state,
        mask,
    })
}

struct NewVoice {
    name: String,
    caption: String,
    source: String,
    seconds: f64,
    speaker_model: String,
    tokens: i64,
    state: Vec<u8>,
    mask: String,
}

enum Inserted {
    Voice(Box<VoiceView>),
    NoUser,
    TooMany,
}

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
) -> Result<Json<Vec<VoiceView>>, ClientApiError> {
    let uid = user.0.discord_id.clone();
    let voices = db
        .read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM client_voices WHERE user_id = ?1 ORDER BY id"
                ))
                .map_err(map_sqlite)?;
            let rows = stmt.query_map(params![uid], view).map_err(map_sqlite)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
        })
        .await?;
    Ok(Json(voices))
}

async fn create(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Json(input): Json<VoiceCreateInput>,
) -> Result<Response, ClientApiError> {
    let voice = validate(input)?;
    let uid = user.0.discord_id.clone();
    let inserted = db
        .writer
        .transaction(move |tx| {
            // `users` 行の無いユーザー（削除済みの残存セッション等）には作らない（外部キー違反を 500 にしない）。
            let exists = tx
                .query_row(
                    "SELECT 1 FROM users WHERE discord_id = ?1",
                    params![uid],
                    |_| Ok(()),
                )
                .optional()
                .map_err(map_sqlite)?
                .is_some();
            if !exists {
                return Ok(Inserted::NoUser);
            }
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM client_voices WHERE user_id = ?1",
                    params![uid],
                    |r| r.get(0),
                )
                .map_err(map_sqlite)?;
            if count >= MAX_VOICES_PER_USER {
                return Ok(Inserted::TooMany);
            }
            tx.execute(
                "INSERT INTO client_voices \
                   (user_id, name, caption, source, seconds, speaker_model, tokens, speaker_state, speaker_mask) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    uid,
                    voice.name,
                    voice.caption,
                    voice.source,
                    voice.seconds,
                    voice.speaker_model,
                    voice.tokens,
                    voice.state,
                    voice.mask
                ],
            )
            .map_err(map_sqlite)?;
            let id = tx.last_insert_rowid();
            tx.query_row(
                &format!("SELECT {COLUMNS} FROM client_voices WHERE id = ?1"),
                params![id],
                view,
            )
            .map(|v| Inserted::Voice(Box::new(v)))
            .map_err(map_sqlite)
        })
        .await?;
    match inserted {
        Inserted::Voice(v) => Ok((StatusCode::CREATED, Json(*v)).into_response()),
        Inserted::NoUser => Err(ClientApiError::not_found("user not found")),
        Inserted::TooMany => Err(ClientApiError::conflict(format!(
            "保存できる声は {MAX_VOICES_PER_USER} 個までです。使わない声を削除してください"
        ))),
    }
}

async fn update(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Path(id): Path<String>,
    Json(patch): Json<VoicePatch>,
) -> Result<Json<VoiceView>, ClientApiError> {
    let id = parse_id(&id)?;
    let name = patch
        .name
        .as_deref()
        .map(|n| trimmed(n, NAME_MAX_CHARS, "名前"))
        .transpose()?;
    if name.as_deref().is_some_and(str::is_empty) {
        return Err(ClientApiError::bad_request("名前を入力してください"));
    }
    let caption = patch
        .caption
        .as_deref()
        .map(|c| trimmed(c, CAPTION_MAX_CHARS, "話し方の指定"))
        .transpose()?;
    let uid = user.0.discord_id.clone();
    let updated = db
        .writer
        .transaction(move |tx| {
            let changed = tx
                .execute(
                    "UPDATE client_voices SET name = COALESCE(?3, name), caption = COALESCE(?4, caption), \
                       updated_at = datetime('now', 'localtime') \
                     WHERE id = ?1 AND user_id = ?2",
                    params![id, uid, name, caption],
                )
                .map_err(map_sqlite)?;
            if changed == 0 {
                return Ok(None);
            }
            tx.query_row(
                &format!("SELECT {COLUMNS} FROM client_voices WHERE id = ?1"),
                params![id],
                view,
            )
            .map(Some)
            .map_err(map_sqlite)
        })
        .await?;
    updated
        .map(Json)
        .ok_or_else(|| ClientApiError::not_found("voice not found"))
}

async fn remove(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Path(id): Path<String>,
) -> Result<StatusCode, ClientApiError> {
    let id = parse_id(&id)?;
    let uid = user.0.discord_id.clone();
    let deleted = db
        .writer
        .execute(move |conn| {
            conn.execute(
                "DELETE FROM client_voices WHERE id = ?1 AND user_id = ?2",
                params![id, uid],
            )
            .map_err(map_sqlite)
        })
        .await?;
    if deleted == 0 {
        return Err(ClientApiError::not_found("voice not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn state(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Path(id): Path<String>,
) -> Result<Response, ClientApiError> {
    let id = parse_id(&id)?;
    let uid = user.0.discord_id.clone();
    let bytes: Option<Vec<u8>> = db
        .read
        .read(move |conn| {
            conn.query_row(
                "SELECT speaker_state FROM client_voices WHERE id = ?1 AND user_id = ?2",
                params![id, uid],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await?;
    let bytes = bytes.ok_or_else(|| ClientApiError::not_found("voice not found"))?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        // 本人のデータなので共有キャッシュには載せない（PWA は端末の OPFS にキャッシュする）。
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(Body::from(bytes))
        .map_err(|e: axum::http::Error| ClientApiError::internal(e))
}
