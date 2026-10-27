//! settings ルートハンドラ（全て `AuthenticatedUser` extractor で auth:"user" を型強制）。
//!
//! レスポンス本文は Node `settingsRoutes.ts` の `sendJson` とバイト単位一致（フラット
//! `{success, message}`・日本語 verbatim）。想定内の業務エラー（400/401/404/409）は明示的な JSON
//! レスポンス、想定外の [`DbError`](yuuka_core::DbError) のみ `?` で [`ApiError`] に写像（500）。
//!
//! プロフィール変更は Node と同じく**セッションを再発行**して継続ログインさせる
//! （現在のセッションを失効→新規発行）。

use std::sync::Arc;

use axum::extract::State;
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{json, Value};
use yuuka_auth::{build_session_cookie, SessionCookieToken};
use yuuka_core::WebError;
use yuuka_types::SessionUser;
use yuuka_web::{ApiError, AppState, AuthenticatedUser};

use crate::dto::{BackupInput, GeminiInput, ProfileInput};
use crate::repo::{self, UserSettingsPatch};
use crate::SettingsRuntime;

// ─── プロフィール更新 ─────────────────────────────────────────────────────────

pub(crate) async fn profile(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<SettingsRuntime>>,
    State(state): State<AppState>,
    SessionCookieToken(current): SessionCookieToken,
    Json(body): Json<ProfileInput>,
) -> Result<Response, ApiError> {
    let username = body.username.as_deref().map(str::trim).unwrap_or_default();
    if username.is_empty() {
        return Ok(bad_request("有効なユーザーネームを指定してください。"));
    }
    if !repo::update_username(&state.db, &user.0.discord_id, username).await? {
        return Ok(bad_request(
            "プロファイルの更新に失敗しました。同じ名前が既に使われている可能性があります。",
        ));
    }
    // セッション内のユーザー名を更新するため、現在のセッションを失効させて再発行する。
    if let Some(token) = current {
        rt.sessions.destroy(&token).await;
    }
    let su = SessionUser {
        discord_id: user.0.discord_id.clone(),
        username: username.to_owned(),
        role: user.0.role,
    };
    let cookie = reissue_cookie(&rt, &state, &su).await?;
    Ok(json_with_cookie(
        json!({ "success": true, "message": "プロファイルを更新しました。" }),
        cookie,
    ))
}

// ─── アカウント削除（本人による退会・最後の管理者は不可） ──────────────────────

pub(crate) async fn delete_account(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<SettingsRuntime>>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let Some(u) = yuuka_auth::users::get_user_by_discord_id(&state.db, &user.0.discord_id).await?
    else {
        return Ok(not_found("アカウントが見つかりません。"));
    };
    // 唯一の管理者ガード（管理者 0 人化を防ぐ・409）。
    if u.role.eq_ignore_ascii_case("admin") && repo::count_admins(&state.db).await? <= 1 {
        return Ok(conflict(
            "あなたは唯一の管理者のため、アカウントを削除できません。先に他のユーザーへ管理者権限を付与してください。",
        ));
    }
    // 所有する起動中の独自 Bot を停止（system_default は対象外・runtime シーム）。
    for bot_id in repo::bot_ids_owned_by(&state.db, &user.0.discord_id).await? {
        rt.bots.stop(&bot_id).await;
    }
    if !repo::delete_user(&state.db, &user.0.discord_id).await? {
        return Ok(not_found("アカウントが見つかりません。"));
    }
    rt.sessions.destroy_all_for_user(&user.0.discord_id).await;
    yuuka_auth::audit::add_audit_log(
        &state.db,
        &user.0.discord_id,
        "auth.account_delete",
        None,
        None,
    )
    .await;
    Ok(ok_message(
        "アカウントを削除しました。関連データもすべて削除されました。",
    ))
}

// ─── ユーザー設定の取得・更新（リッチ返信・リマインド既定・通知先・タイムゾーン） ─

/// 現在値の取得（Bot スコープの `/api/status` を通さずに読む・「あなたの設定」用）。
pub(crate) async fn user_settings_get(
    user: AuthenticatedUser,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let p = repo::user_preferences(&state.db, &user.0.discord_id).await?;
    Ok(json_ok(json!({
        "success": true,
        "richReplyEnabled": p.rich_reply_enabled,
        "remindDefaultMinutes": p.remind_default_minutes,
        "notifyTarget": { "type": p.notify_target_type, "id": p.notify_target_id },
        "timezone": p.timezone,
    })))
}

pub(crate) async fn user_settings(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let mut patch = UserSettingsPatch::default();
    if let Some(obj) = body.as_object() {
        // present 判定は Node の `!== undefined`（キーが存在すれば・null も present）に合わせる。
        if let Some(v) = obj.get("richReplyEnabled") {
            patch.rich_reply = Some(matches!(v, Value::Bool(true)));
        }
        if let Some(v) = obj.get("remindDefaultMinutes") {
            // Node: Math.max(0, Number(x) || 0) → repo で floor + max(0)。
            let coerced = js_number_or(Some(v), 0.0);
            patch.remind_minutes = Some(coerced.max(0.0).floor().max(0.0) as i64);
        }
        if let Some(v) = obj.get("notifyTargetType") {
            patch.notify_type = if v.as_str() == Some("channel") {
                Some("channel")
            } else {
                Some("dm")
            };
        }
        if let Some(v) = obj.get("notifyTargetId") {
            patch.notify_id = Some(match v {
                Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
                _ => None,
            });
        }
        if let Some(Value::String(s)) = obj.get("timezone") {
            let t = s.trim();
            if !t.is_empty() {
                patch.timezone = Some(t.to_owned());
            }
        }
    }
    repo::update_user_settings(&state.db, &user.0.discord_id, patch).await?;
    Ok(ok_message("ユーザー設定を更新しました。"))
}

// ─── Gemini 設定更新（ユーザー単位） ──────────────────────────────────────────

pub(crate) async fn gemini_get(
    user: AuthenticatedUser,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    use rusqlite::OptionalExtension;
    let uid = user.0.discord_id;
    // API キーは本体を返さず、登録済みか（hasApiKey）だけを返す。
    let (model, has_api_key) = state
        .db
        .read
        .read(move |conn| {
            conn.query_row(
                "SELECT gemini_model, \
                 COALESCE(gemini_api_key_encrypted, '') != '' \
                 FROM users WHERE discord_id = ?1",
                rusqlite::params![uid],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, bool>(1)?)),
            )
            .optional()
            .map_err(yuuka_db::map_sqlite)
        })
        .await?
        .unwrap_or((None, false));
    let model = model
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| yuuka_gemini::LlmSettings::default().model);
    Ok(json_ok(
        json!({ "success": true, "provider": "gemini", "model": model, "hasApiKey": has_api_key,
        "models": yuuka_gemini::ALLOWED_MODELS, "defaults": yuuka_gemini::LlmSettings::default() }),
    ))
}

pub(crate) async fn gemini(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<SettingsRuntime>>,
    State(state): State<AppState>,
    Json(body): Json<GeminiInput>,
) -> Result<Response, ApiError> {
    // Node: `if (!model)` は生値（trim 前）の空判定。保存は trim 後。
    let model_raw = body.model.unwrap_or_default();
    if model_raw.is_empty() {
        return Ok(bad_request("モデル名は必須項目です。"));
    }
    let model = model_raw.trim();
    if let Err(e) = yuuka_gemini::LlmSettings::validate(
        body.provider.as_deref().unwrap_or("gemini").trim(),
        model,
    ) {
        return Ok(bad_request(&e.to_string()));
    }

    let api_key = body.api_key.unwrap_or_default();
    let (encrypted, iv, tag): (Option<String>, Option<String>, Option<String>);
    if !api_key.is_empty() && !api_key.starts_with("****") {
        let key = api_key.trim();
        if !is_likely_gemini_key(key) {
            return Ok(bad_request(
                "Gemini APIキーの形式が正しくありません。Google AI Studio で取得したキー（「AIzaSy」または「AQ.」で始まるもの）をそのまま入力してください。",
            ));
        }
        let Some(crypto) = rt.crypto.as_ref() else {
            return Err(ApiError(WebError::Internal));
        };
        let enc = crypto
            .encrypt_text(key)
            .map_err(|_| ApiError(WebError::Internal))?;
        (encrypted, iv, tag) = (Some(enc.encrypted), Some(enc.iv), Some(enc.auth_tag));
    } else {
        // マスク or 空 → 既存のキーを維持する（3 列をそのまま書き戻す）。
        (encrypted, iv, tag) = repo::get_gemini_enc(&state.db, &user.0.discord_id)
            .await?
            .unwrap_or((None, None, None));
    }
    repo::set_gemini(&state.db, &user.0.discord_id, encrypted, iv, tag, model).await?;
    Ok(ok_message("Gemini 設定を更新しました。"))
}

// ─── バックアップ設定の取得・更新 ─────────────────────────────────────────────

/// 現在値の取得。フォルダ ID は本人の Drive フォルダの識別子で秘密情報ではないため、編集できるよう
/// マスクせずに返す（`/api/status` はマスク済みの値を返す）。
pub(crate) async fn backup_get(
    user: AuthenticatedUser,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let p = repo::user_preferences(&state.db, &user.0.discord_id).await?;
    Ok(json_ok(json!({
        "success": true,
        "enabled": p.backup_enabled,
        "folderId": p.backup_folder_id,
        "intervalHours": p.backup_interval_hours,
        "generations": p.backup_generations,
        "lastRunAt": p.backup_last_run_at,
    })))
}

pub(crate) async fn backup(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Json(body): Json<BackupInput>,
) -> Result<Response, ApiError> {
    // フォルダ指定は「フォルダ ID 単体」でも「Google Drive の URL」でも受け付ける。
    let folder = match body.folder_id.as_deref().map(str::trim) {
        Some(f) if !f.is_empty() => match extract_drive_folder_id(f) {
            Some(id) => Some(id),
            None => {
                return Ok(bad_request(
                    "バックアップ先フォルダの指定が不正です。フォルダIDまたはGoogle DriveのフォルダURLを入力してください。",
                ));
            }
        },
        // 未指定 or 空 → backup_folder_id 列は触らない（Node normalizedFolderId undefined）。
        _ => None,
    };
    let enabled = body.enabled == Some(true);
    let interval = js_number_or(body.interval_hours.as_ref(), 24.0);
    let generations = js_number_or(body.generations.as_ref(), 7.0);
    repo::update_backup(
        &state.db,
        &user.0.discord_id,
        enabled,
        interval,
        generations,
        folder,
    )
    .await?;
    Ok(ok_message("バックアップ設定を保存しました。"))
}

// ─── レスポンス・ユーティリティ ───────────────────────────────────────────────

/// 旧セッション失効後に新しいセッションを発行し `Set-Cookie` 値を作る。
async fn reissue_cookie(
    rt: &SettingsRuntime,
    state: &AppState,
    su: &SessionUser,
) -> Result<HeaderValue, ApiError> {
    let token = rt
        .sessions
        .create(su, rt.session_ttl_secs)
        .await
        .map_err(|_| ApiError(WebError::Internal))?;
    let max_age = i64::try_from(rt.session_ttl_secs).unwrap_or(i64::MAX);
    Ok(build_session_cookie(state.config.https, &token, max_age))
}

fn json_ok(body: Value) -> Response {
    (StatusCode::OK, Json(body)).into_response()
}

fn json_with_cookie(body: Value, cookie: HeaderValue) -> Response {
    let mut resp = json_ok(body);
    resp.headers_mut().insert(SET_COOKIE, cookie);
    resp
}

fn ok_message(message: &str) -> Response {
    json_ok(json!({ "success": true, "message": message }))
}

fn status_json(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

fn bad_request(message: &str) -> Response {
    status_json(StatusCode::BAD_REQUEST, message)
}

fn conflict(message: &str) -> Response {
    status_json(StatusCode::CONFLICT, message)
}

fn not_found(message: &str) -> Response {
    status_json(StatusCode::NOT_FOUND, message)
}

/// JS `Number(x) || fallback` 相当（0/NaN/非数は fallback）。`intervalHours`/`generations` 用。
fn js_number_or(value: Option<&Value>, fallback: f64) -> f64 {
    let n = value.map_or(f64::NAN, js_number);
    if n != 0.0 && n.is_finite() {
        n
    } else {
        fallback
    }
}

/// JS `Number(x)` 相当（number そのまま・numeric string は parse・空文字/null は 0・その他 NaN）。
fn js_number(value: &Value) -> f64 {
    match value {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else {
                t.parse::<f64>().unwrap_or(f64::NAN)
            }
        }
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Null => 0.0,
        _ => f64::NAN,
    }
}

/// Gemini API キーの形式検証（`^(AIzaSy[A-Za-z0-9_-]{33}|AQ\.[A-Za-z0-9_-]{35,})$`）。
/// 旧形式（`AIzaSy` + 33 文字）と新形式（`AQ.` + 35 文字以上）の両方を受け付ける。
fn is_likely_gemini_key(value: &str) -> bool {
    let is_key_char = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-');
    if let Some(rest) = value.strip_prefix("AIzaSy") {
        rest.len() == 33 && rest.bytes().all(is_key_char)
    } else if let Some(rest) = value.strip_prefix("AQ.") {
        rest.len() >= 35 && rest.bytes().all(is_key_char)
    } else {
        false
    }
}

/// Google Drive のフォルダ ID を抽出する（Node `extractDriveFolderId`）。フォルダ ID 単体、
/// `/folders/<ID>` を含む URL、`?id=<ID>` の URL を受け付ける。不正は `None`。
fn extract_drive_folder_id(input: &str) -> Option<String> {
    let value = input.trim();
    if value.is_empty() {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    let is_url = lower.starts_with("http://") || lower.starts_with("https://");
    if !is_url {
        // URL でなければフォルダ ID 本体とみなす（英数・`_`・`-` のみ）。
        return valid_folder_id(value).then(|| value.to_owned());
    }
    // 形式1: /folders/<ID>。
    if let Some((_, rest)) = value.split_once("/folders/") {
        let id: String = rest.chars().take_while(is_folder_char).collect();
        if !id.is_empty() {
            return Some(id);
        }
    }
    // 形式2: ?id=<ID>（クエリの id パラメータ）。
    if let Some((_, query)) = value.split_once('?') {
        for pair in query.split('&') {
            if let Some(raw) = pair.strip_prefix("id=") {
                let val = raw.split('#').next().unwrap_or(raw);
                if valid_folder_id(val) {
                    return Some(val.to_owned());
                }
            }
        }
    }
    None
}

fn valid_folder_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn is_folder_char(c: &char) -> bool {
    c.is_ascii_alphanumeric() || *c == '_' || *c == '-'
}

#[cfg(test)]
mod tests {
    use super::{extract_drive_folder_id, is_likely_gemini_key, js_number_or};
    use serde_json::json;

    #[test]
    fn gemini_key_shape() {
        let old = format!("AIzaSy{}", "A0b_-".repeat(6) + "c1d");
        let new = format!("AQ.{}", "Ab8_-".repeat(7));
        // 旧形式（AIzaSy + 33 文字ちょうど）。
        assert!(is_likely_gemini_key(&old));
        assert!(!is_likely_gemini_key(&format!("{old}x")));
        assert!(!is_likely_gemini_key(&old[..old.len() - 1]));
        // 新形式（AQ. + 35 文字以上）。
        assert!(is_likely_gemini_key(&new));
        assert!(is_likely_gemini_key(&format!("{new}{}", "z".repeat(100))));
        assert!(!is_likely_gemini_key(&new[..new.len() - 1]));
        // 接頭辞違い・本体に `.` や空白・非 ASCII を含むものは不可（コピペ事故の検出）。
        assert!(!is_likely_gemini_key(&format!("AIzaSz{}", &old[6..])));
        assert!(!is_likely_gemini_key(&format!("BQ.{}", &new[3..])));
        assert!(!is_likely_gemini_key(&format!("AQ.{}.", &new[3..])));
        assert!(!is_likely_gemini_key(&format!("AQ.{} x", &new[3..])));
        assert!(!is_likely_gemini_key(&format!("AQ.{}あ", &new[3..])));
        assert!(!is_likely_gemini_key(""));
    }

    #[test]
    fn drive_folder_id_forms() {
        // ID 単体。
        assert_eq!(
            extract_drive_folder_id("abc_123-XYZ").as_deref(),
            Some("abc_123-XYZ")
        );
        // /folders/<ID> を含む URL。
        assert_eq!(
            extract_drive_folder_id("https://drive.google.com/drive/folders/FOLDER_1").as_deref(),
            Some("FOLDER_1")
        );
        assert_eq!(
            extract_drive_folder_id("https://drive.google.com/drive/u/0/folders/ABC-9?usp=sharing")
                .as_deref(),
            Some("ABC-9")
        );
        // ?id=<ID>。
        assert_eq!(
            extract_drive_folder_id("https://drive.google.com/open?id=ID12345").as_deref(),
            Some("ID12345")
        );
        // 不正（空・記号入り ID・URL だが folder/id 無し）。
        assert_eq!(extract_drive_folder_id("  "), None);
        assert_eq!(extract_drive_folder_id("has space"), None);
        assert_eq!(extract_drive_folder_id("https://example.com/x"), None);
    }

    #[test]
    fn number_or_fallback() {
        // number そのまま。
        assert_eq!(js_number_or(Some(&json!(48)), 24.0), 48.0);
        // numeric string。
        assert_eq!(js_number_or(Some(&json!("12")), 24.0), 12.0);
        // 0 は falsy → fallback（Node `Number(x) || fallback`）。
        assert_eq!(js_number_or(Some(&json!(0)), 24.0), 24.0);
        // NaN（非数文字列）→ fallback。
        assert_eq!(js_number_or(Some(&json!("abc")), 24.0), 24.0);
        // 未指定 → fallback。
        assert_eq!(js_number_or(None, 7.0), 7.0);
    }
}
