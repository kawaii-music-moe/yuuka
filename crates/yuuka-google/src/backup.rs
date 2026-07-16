//! ユーザー単位バックアップの live 実装（`BackupPort` の実 Drive アップロード・Node `backupService.ts`）。
//!
//! Node は当該ユーザーの行だけを抽出した一時 SQLite を `archiver` で ZIP 化し、ユーザー個人の Google Drive
//! へタイムスタンプ付き新規ファイルとしてアップロードし、`backup_generations` を超える古い世代を削除する。
//! 本移植は同じ手順を [`rusqlite`]（一時 DB エクスポート）+ [`zip`]（deflate）+ [`reqwest`]（Drive
//! multipart アップロード）で行う。リフレッシュトークンは owner の primary アカウント（`user_google_accounts`）
//! から取り、**システム鍵**（[`SystemCrypto::decrypt_text`]）で復号する（[`crate::http::GoogleHttpClient`]
//! と同一のトークン経路）。
//!
//! ## 実 HTTP は本番環境検証待ち（live-deferred）
//!
//! Drive の `files.create`（multipart アップロード）/ `files.list` / `files.delete` の**実 Google 通信**は
//! 本番の OAuth 設定が無いと走らせられないため、単体テストでは検証していない（[`crate::http`] の
//! `GoogleHttpClient` と同方針）。単体テストは**純ロジックのみ**——ユーザースコープ行/スキーマのコピー
//! （in-memory/temp SQLite）、バックアップファイル名の書式、世代プルーニングの選択、multipart ボディの
//! 組み立て形状——を検証する。実通信は実環境検証で担保する。
//!
//! [`SystemCrypto::decrypt_text`]: yuuka_crypto::SystemCrypto::decrypt_text

use std::io::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::Connection;
use yuuka_core::DbError;
use yuuka_crypto::SystemCrypto;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

use crate::ports::{BackupPort, GoogleError};
use crate::repo;

/// バックアップファイル名のプレフィックス（Node `BACKUP_PREFIX`）。
const BACKUP_PREFIX: &str = "yuuka_backup_";
/// バックアップ ZIP の MIME（Node `BACKUP_MIME`）。
const BACKUP_MIME: &str = "application/zip";
/// HTTP タイムアウト（`GoogleHttpClient` と揃える）。
const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// Drive multipart アップロードエンドポイント（`drive.files.create`・`uploadType=multipart`）。
const UPLOAD_ENDPOINT: &str =
    "https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart&fields=id%2CwebViewLink";
/// Drive ファイル一覧エンドポイント（`drive.files.list`）。
const FILES_ENDPOINT: &str = "https://www.googleapis.com/drive/v3/files";
/// トークンリフレッシュエンドポイント（`GoogleHttpClient` と同一）。
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// `user_id` 列でユーザーデータが分離されているテーブル群（Node `USER_SCOPED_TABLES`・backupService.ts:29-49）。
///
/// **Node からそのまま写した 18 テーブル**（順序も一致）。列は `SELECT *` で保持順にコピーする。
const USER_SCOPED_TABLES: &[&str] = &[
    "personas",
    "bot_active_personas",
    "message_logs",
    "todos",
    "schedules",
    "reminders",
    "expenses",
    "budget_limits",
    "planned_payments",
    "playbooks",
    "playbook_schedules",
    "playbook_runs",
    "clipboard_entries",
    "contacts",
    "credentials",
    "webhook_endpoints",
    "webhook_deliveries",
    "report_configs",
    "mcp_servers",
];

/// `user_id` が主キーの単一行テーブル（Node `USER_KEYED_TABLES`・backupService.ts:52）。
const USER_KEYED_TABLES: &[&str] = &["context_notes", "briefing_configs"];

/// Drive ファイルの最小ビュー（Node `listBackupFiles` の返り値要素）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct DriveFile {
    id: String,
    name: String,
}

/// ユーザーのバックアップ設定（Node `getUserBackupConfig`・`users` 行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupConfig {
    pub enabled: bool,
    pub interval_hours: i64,
    pub generations: i64,
    pub folder_id: Option<String>,
    pub last_run_at: Option<String>,
}

/// Google Drive バックアップの live 実装（`BackupPort`）。
///
/// [`crate::http::GoogleHttpClient`] とトークン経路（primary アカウントの復号リフレッシュトークン →
/// アクセストークン）を共有する。共有 `reqwest::Client` / `Arc<SystemCrypto>` / `Db` を注入する。
pub struct GoogleBackupClient {
    client_id: String,
    client_secret: String,
    crypto: Arc<SystemCrypto>,
    db: Db,
    http: reqwest::Client,
}

impl std::fmt::Debug for GoogleBackupClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleBackupClient")
            .field("configured", &!self.client_id.is_empty())
            .finish_non_exhaustive()
    }
}

impl GoogleBackupClient {
    /// live バックアップクライアントを組み立てる（`main` から設定と共有ハンドルを注入する）。
    #[must_use]
    pub fn new(
        client_id: String,
        client_secret: String,
        crypto: Arc<SystemCrypto>,
        db: Db,
        http: reqwest::Client,
    ) -> Self {
        Self {
            client_id,
            client_secret,
            crypto,
            db,
            http,
        }
    }

    /// primary アカウントの復号済みリフレッシュトークンからアクセストークンを得る（`GoogleHttpClient`
    /// と同一のトークン経路）。
    ///
    /// # Errors
    /// アカウント不在 / 復号失敗 / 上流失敗時 [`GoogleError`]。
    async fn access_token_for(&self, user_id: &str) -> Result<String, GoogleError> {
        let tokens = repo::get_primary_account_tokens(&self.db, user_id)
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?
            .ok_or(GoogleError::NotConfigured)?;
        let (_id, enc, iv, tag) = tokens;
        let refresh = self
            .crypto
            .decrypt_text(&enc, &iv, &tag)
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        self.refresh_access_token(&refresh).await
    }

    /// リフレッシュトークンをアクセストークンへ交換する（`GoogleHttpClient::refresh_access_token` と同一）。
    ///
    /// # Errors
    /// 上流 HTTP 失敗 / `access_token` 不在時 [`GoogleError::Upstream`]。
    async fn refresh_access_token(&self, refresh_token: &str) -> Result<String, GoogleError> {
        let form = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ];
        let resp = self
            .http
            .post(TOKEN_ENDPOINT)
            .form(&form)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(GoogleError::Upstream(format!(
                "token refresh status {}",
                resp.status()
            )));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        parse_access_token(&body)
            .ok_or_else(|| GoogleError::Upstream("no access_token in refresh response".to_owned()))
    }

    /// ZIP バイト列を Drive へ multipart アップロードする（Node `uploadToGoogleDrive`＝`drive.files.create`）。
    /// 返り値は `webViewLink`（無ければ空文字・Node と同じ）。
    ///
    /// # Errors
    /// 上流 HTTP 失敗 / 非 2xx 時 [`GoogleError::Upstream`]。
    async fn upload_zip(
        &self,
        access_token: &str,
        file_name: &str,
        folder_id: Option<&str>,
        zip_bytes: Vec<u8>,
    ) -> Result<String, GoogleError> {
        let (content_type, body) = build_multipart_body(file_name, folder_id, &zip_bytes);
        let resp = self
            .http
            .post(UPLOAD_ENDPOINT)
            .bearer_auth(access_token)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(GoogleError::Upstream(format!(
                "drive upload status {}",
                resp.status()
            )));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        Ok(parse_web_view_link(&text))
    }

    /// プレフィックスでバックアップファイル一覧を取得する（Node `listBackupFiles`・createdTime desc）。
    /// 失敗は空一覧へ縮退する（Node は catch → `[]`）。
    async fn list_backup_files(
        &self,
        access_token: &str,
        folder_id: Option<&str>,
    ) -> Vec<DriveFile> {
        let mut query = format!("name contains '{BACKUP_PREFIX}' and trashed=false");
        if let Some(fid) = folder_id.filter(|f| !f.is_empty()) {
            query.push_str(&format!(" and '{fid}' in parents"));
        }
        let params = [
            ("q", query.as_str()),
            ("fields", "files(id,name,createdTime)"),
            ("orderBy", "createdTime desc"),
            ("pageSize", "100"),
            ("spaces", "drive"),
        ];
        let resp = self
            .http
            .get(FILES_ENDPOINT)
            .query(&params)
            .bearer_auth(access_token)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await;
        let Ok(resp) = resp else {
            return Vec::new();
        };
        if !resp.status().is_success() {
            return Vec::new();
        }
        let Ok(text) = resp.text().await else {
            return Vec::new();
        };
        parse_backup_files(&text)
    }

    /// Drive ファイルを削除する（Node `deleteDriveFile`）。成功は `true`（失敗は false へ縮退）。
    async fn delete_file(&self, access_token: &str, file_id: &str) -> bool {
        let url = format!("{FILES_ENDPOINT}/{file_id}");
        let resp = self
            .http
            .delete(&url)
            .bearer_auth(access_token)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await;
        matches!(resp, Ok(r) if r.status().is_success())
    }

    /// 世代管理: 保持世代数を超える古いバックアップを削除する（Node `runBackup` の §8.2）。
    /// 個々の失敗は握り潰す（Node の per-file try 相当・全体は非致命）。
    async fn prune_generations(
        &self,
        access_token: &str,
        folder_id: Option<&str>,
        generations: i64,
    ) {
        let gens = generations.max(1);
        let files = self.list_backup_files(access_token, folder_id).await;
        for file in stale_files(&files, gens) {
            if self.delete_file(access_token, &file.id).await {
                tracing::info!(name = %file.name, "🗑️ [Backup] 古い世代を削除しました");
            }
        }
    }
}

#[async_trait]
impl BackupPort for GoogleBackupClient {
    async fn run_backup(&self, user_id: &str) -> Result<String, GoogleError> {
        // 1) バックアップ設定を検証（Node `runBackup`: ユーザー不在/無効は失敗）。
        let config = read_backup_config(&self.db, user_id)
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?
            .ok_or(GoogleError::NotConfigured)?;
        if !config.enabled {
            return Err(GoogleError::NotConfigured);
        }

        // 2) 当該ユーザーの行だけを抽出した一時 SQLite をエクスポートし、ZIP 化する。
        //    読み取りソース接続の中で一時 DB を開いてコピー・close する（Node `getDb()` + `new Database()`）。
        let zip_bytes = self.export_and_zip(user_id).await?;

        // 3) アクセストークンを解決し、Drive へ multipart アップロードする（新規ファイル・世代管理のため上書きしない）。
        let access = self.access_token_for(user_id).await?;
        let file_name = backup_file_name(chrono::Local::now());
        let url = self
            .upload_zip(&access, &file_name, config.folder_id.as_deref(), zip_bytes)
            .await?;

        // 4) 世代管理（古い世代削除）。失敗しても本体成功は返す（Node は prune を try で握り潰す）。
        self.prune_generations(&access, config.folder_id.as_deref(), config.generations)
            .await;

        // 5) 最終実行時刻を更新（Node `touchBackupLastRun`）。失敗は握り潰す（本体成功を優先）。
        if let Err(e) = touch_backup_last_run(&self.db, user_id).await {
            tracing::warn!(user = %user_id, error = %e, "⚠️ [Backup] last_run 更新に失敗");
        }
        Ok(url)
    }
}

impl GoogleBackupClient {
    /// 当該ユーザーの行だけを抽出した一時 SQLite を作り、`data/yuuka.db` として ZIP 化してバイト列を返す。
    ///
    /// 読み取りソース接続（read pool）のクロージャ内で一時 DB を開いてコピーし close する（Node は
    /// `getDb()`＝ソースと `new Database(tempDbPath)`＝送り先を同期で扱う）。一時ファイルは
    /// [`tempfile`] で作り、関数を抜けると自動削除される（Node の finally unlink 相当）。
    ///
    /// # Errors
    /// 一時 DB 作成 / コピー / ZIP 化失敗時 [`GoogleError::Upstream`]。
    async fn export_and_zip(&self, user_id: &str) -> Result<Vec<u8>, GoogleError> {
        let uid = user_id.to_owned();
        let db_bytes = self
            .db
            .read
            .read(move |conn| {
                // 一時 SQLite ファイルへ本人分をエクスポート（Node `exportUserData`）。
                let tmp = tempfile::Builder::new()
                    .prefix("yuuka_backup_")
                    .suffix(".db")
                    .tempfile()
                    .map_err(|e| DbError::Operation(format!("temp db: {e}")))?;
                let tmp_path = tmp.path().to_path_buf();
                {
                    let dest = Connection::open(&tmp_path).map_err(map_sqlite)?;
                    export_user_data(conn, &dest, &uid).map_err(map_sqlite)?;
                    // dest は close（drop）してからファイルを読む。
                }
                std::fs::read(&tmp_path)
                    .map_err(|e| DbError::Operation(format!("read temp db: {e}")))
            })
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;

        zip_single_file("data/yuuka.db", &db_bytes)
            .map_err(|e| GoogleError::Upstream(format!("zip: {e}")))
    }
}

// ─── 純ロジック（テスト対象） ──────────────────────────────────────────────────

/// データベーススキーマ + 当該ユーザーのデータのみを送り先 SQLite へエクスポートする（Node `exportUserData`）。
///
/// スキーマ: `sqlite_master` の table/index 定義（FTS5 仮想テーブル・内部シャドウ・`sqlite_%` は除外）。
/// データ: `users WHERE discord_id`、[`USER_SCOPED_TABLES`] + [`USER_KEYED_TABLES`] を `WHERE user_id`、
/// `bots WHERE user_id`。テーブル不在は skip（Node try/catch）。`SELECT *` の列順を保って INSERT する。
///
/// # Errors
/// スキーマ/データコピーの SQL 失敗時 [`rusqlite::Error`]（テーブル不在の skip は含まない）。
fn export_user_data(src: &Connection, dest: &Connection, user_id: &str) -> rusqlite::Result<()> {
    // 1) スキーマ（table/index 定義）をコピー。個々の失敗は skip（Node の per-statement try/catch）。
    let schema: Vec<String> = {
        let mut stmt = src.prepare(
            "SELECT sql FROM sqlite_master \
             WHERE type IN ('table', 'index') AND sql IS NOT NULL \
               AND name NOT LIKE 'sqlite_%' \
               AND name NOT LIKE 'message_logs_fts%'",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for sql in &schema {
        // 壊れた/重複定義は skip（Node `console.warn` して継続）。
        let _ = dest.execute_batch(sql);
    }

    // 2) 本人分の行を INSERT（`SELECT *` の列順を保つ）。
    copy_rows(src, dest, "users", "discord_id", user_id)?;
    for table in USER_SCOPED_TABLES.iter().chain(USER_KEYED_TABLES) {
        copy_rows(src, dest, table, "user_id", user_id)?;
    }
    copy_rows(src, dest, "bots", "user_id", user_id)?;
    Ok(())
}

/// `SELECT * FROM {table} WHERE {where_col} = ?` の全行を送り先へコピーする（Node `copyRows`）。
/// テーブル不在（prepare 失敗）は skip（Node の try/catch return）。列名は `SELECT *` の順を保つ。
///
/// # Errors
/// INSERT の SQL 失敗時 [`rusqlite::Error`]（prepare 失敗＝テーブル不在は `Ok(())` へ握る）。
fn copy_rows(
    src: &Connection,
    dest: &Connection,
    table: &str,
    where_col: &str,
    user_id: &str,
) -> rusqlite::Result<()> {
    let select = format!("SELECT * FROM {table} WHERE {where_col} = ?1");
    // テーブル不在は prepare で失敗 → skip（Node の catch return）。
    let Ok(mut stmt) = src.prepare(&select) else {
        return Ok(());
    };
    let columns: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|c| (*c).to_owned())
        .collect();
    if columns.is_empty() {
        return Ok(());
    }
    let col_list = columns.join(", ");
    let placeholders = (1..=columns.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let insert = format!("INSERT INTO {table} ({col_list}) VALUES ({placeholders})");

    let col_count = columns.len();
    let mut rows = stmt.query([user_id])?;
    while let Some(row) = rows.next()? {
        // 各セルを rusqlite の動的値として読み、そのまま INSERT する（型を保持）。
        let values: Vec<rusqlite::types::Value> = (0..col_count)
            .map(|i| row.get::<_, rusqlite::types::Value>(i))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let params = rusqlite::params_from_iter(values.iter());
        dest.execute(&insert, params)?;
    }
    Ok(())
}

/// タイムスタンプ付きバックアップファイル名（Node `backupFileName`＝`yuuka_backup_YYYYMMDD_HHMMSS.zip`）。
#[must_use]
fn backup_file_name(now: chrono::DateTime<chrono::Local>) -> String {
    use chrono::{Datelike, Timelike};
    format!(
        "{BACKUP_PREFIX}{:04}{:02}{:02}_{:02}{:02}{:02}.zip",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
    )
}

/// 保持世代数を超える古いバックアップ（createdTime desc 前提の `generations` 件目以降）を返す
/// （Node `files.slice(generations)`）。`generations` は 1 以上にクランプ済み前提。
fn stale_files(files: &[DriveFile], generations: i64) -> Vec<DriveFile> {
    let keep = usize::try_from(generations.max(1)).unwrap_or(usize::MAX);
    files.iter().skip(keep).cloned().collect()
}

/// 単一ファイルを deflate ZIP にして返す（Node `archiver` の `archive.file(..., { name })` 相当）。
///
/// # Errors
/// ZIP 書き込み失敗時 [`zip::result::ZipError`]。
fn zip_single_file(name: &str, bytes: &[u8]) -> Result<Vec<u8>, zip::result::ZipError> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buf);
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file(name, options)?;
        writer.write_all(bytes)?;
        writer.finish()?;
    }
    Ok(buf.into_inner())
}

/// multipart/related ボディを組み立てる（Node `drive.files.create` の `uploadType=multipart`）。
/// 返り値は `(Content-Type ヘッダ値, ボディバイト列)`。metadata パートは JSON、media パートは ZIP。
fn build_multipart_body(
    file_name: &str,
    folder_id: Option<&str>,
    zip_bytes: &[u8],
) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "yuuka_backup_boundary_v1";
    let metadata = drive_metadata_json(file_name, folder_id);

    let mut body = Vec::new();
    // metadata パート（application/json）。
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
    body.extend_from_slice(metadata.as_bytes());
    body.extend_from_slice(b"\r\n");
    // media パート（application/zip）。
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(format!("Content-Type: {BACKUP_MIME}\r\n\r\n").as_bytes());
    body.extend_from_slice(zip_bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

    let content_type = format!("multipart/related; boundary={BOUNDARY}");
    (content_type, body)
}

/// Drive ファイルメタデータ JSON（`{name, mimeType, parents?}`・Node `fileMetadata`）。
fn drive_metadata_json(file_name: &str, folder_id: Option<&str>) -> String {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "name".to_owned(),
        serde_json::Value::String(file_name.to_owned()),
    );
    meta.insert(
        "mimeType".to_owned(),
        serde_json::Value::String(BACKUP_MIME.to_owned()),
    );
    if let Some(fid) = folder_id.filter(|f| !f.is_empty()) {
        // folderId 有り（空文字は除外）のときのみ parents を付ける（Node `folderId || undefined`）。
        meta.insert(
            "parents".to_owned(),
            serde_json::Value::Array(vec![serde_json::Value::String(fid.to_owned())]),
        );
    }
    serde_json::Value::Object(meta).to_string()
}

/// トークン応答 JSON から `access_token` を抜く（`GoogleHttpClient::parse_access_token` と同一規則）。
fn parse_access_token(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// `files.create` 応答 JSON から `webViewLink` を抜く（不在は空文字・Node `data.webViewLink || ""`）。
fn parse_web_view_link(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("webViewLink")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// `files.list` 応答 JSON を [`DriveFile`] へ写像する（`id` と `name` の両方があり、かつ名前が
/// プレフィックスで始まるものだけ・Node の `filter`）。順序は応答（createdTime desc）を保つ。
fn parse_backup_files(body: &str) -> Vec<DriveFile> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let Some(files) = value.get("files").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    files
        .iter()
        .filter_map(|f| {
            let id = f.get("id").and_then(serde_json::Value::as_str)?;
            let name = f.get("name").and_then(serde_json::Value::as_str)?;
            if !name.starts_with(BACKUP_PREFIX) {
                return None;
            }
            Some(DriveFile {
                id: id.to_owned(),
                name: name.to_owned(),
            })
        })
        .collect()
}

// ─── バックアップ設定の read/write（Node `getUserBackupConfig` / `touchBackupLastRun`） ──────────

/// ユーザーのバックアップ設定を読む（Node `getUserBackupConfig`・`users` 行・不在は `None`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn read_backup_config(db: &Db, user_id: &str) -> Result<Option<BackupConfig>, DbError> {
    let user_id = user_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT backup_enabled, backup_interval_hours, backup_generations, \
                        backup_folder_id, backup_last_run_at \
                 FROM users WHERE discord_id = ?1",
                [user_id],
                |r| {
                    Ok(BackupConfig {
                        enabled: r.get::<_, i64>(0)? != 0,
                        interval_hours: r.get(1)?,
                        generations: r.get(2)?,
                        folder_id: r.get(3)?,
                        last_run_at: r.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// バックアップ最終実行時刻を現在ローカル時刻で更新する（Node `touchBackupLastRun`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn touch_backup_last_run(db: &Db, user_id: &str) -> Result<(), DbError> {
    let user_id = user_id.to_owned();
    db.writer
        .execute(move |conn| {
            conn.execute(
                "UPDATE users SET backup_last_run_at = datetime('now','localtime'), \
                        updated_at = datetime('now','localtime') WHERE discord_id = ?1",
                [user_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

use rusqlite::OptionalExtension as _;

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use rusqlite::Connection;

    use super::{
        backup_file_name, build_multipart_body, copy_rows, drive_metadata_json, export_user_data,
        parse_backup_files, parse_web_view_link, stale_files, zip_single_file, DriveFile,
        BACKUP_PREFIX,
    };

    /// 最小スキーマ（users / todos / message_logs / message_logs_fts / bots / personas）を持つソース DB を作る。
    fn source_db() -> Connection {
        let conn = Connection::open_in_memory().expect("mem db");
        conn.execute_batch(
            "CREATE TABLE users (discord_id TEXT PRIMARY KEY, username TEXT);\
             CREATE TABLE todos (id INTEGER PRIMARY KEY, user_id TEXT, title TEXT);\
             CREATE TABLE message_logs (id INTEGER PRIMARY KEY, user_id TEXT, content TEXT);\
             CREATE TABLE personas (id INTEGER PRIMARY KEY, user_id TEXT, name TEXT);\
             CREATE TABLE bots (id TEXT PRIMARY KEY, user_id TEXT, name TEXT);\
             CREATE INDEX idx_todos_user ON todos(user_id);\
             CREATE VIRTUAL TABLE message_logs_fts USING fts5(content);",
        )
        .expect("ddl");
        // 2 ユーザー分を入れる。
        conn.execute_batch(
            "INSERT INTO users (discord_id, username) VALUES ('alice','A'),('bob','B');\
             INSERT INTO todos (user_id, title) VALUES ('alice','買い物'),('alice','掃除'),('bob','他人');\
             INSERT INTO message_logs (user_id, content) VALUES ('alice','hi'),('bob','bye');\
             INSERT INTO personas (user_id, name) VALUES ('alice','秘書');\
             INSERT INTO bots (id, user_id, name) VALUES ('b1','alice','MyBot'),('b2','bob','Other');\
             INSERT INTO message_logs_fts (content) VALUES ('hi');",
        )
        .expect("seed");
        conn
    }

    #[test]
    fn export_copies_only_target_user_rows() {
        let src = source_db();
        let dest = Connection::open_in_memory().expect("dest");
        export_user_data(&src, &dest, "alice").expect("export");

        // users: 本人のみ。
        let users: i64 = dest
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .unwrap();
        assert_eq!(users, 1);
        let name: String = dest
            .query_row("SELECT username FROM users", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "A");

        // todos: alice の 2 件のみ（bob は除外）。
        let todos: i64 = dest
            .query_row("SELECT COUNT(*) FROM todos", [], |r| r.get(0))
            .unwrap();
        assert_eq!(todos, 2);

        // message_logs: alice の 1 件のみ。
        let logs: i64 = dest
            .query_row("SELECT COUNT(*) FROM message_logs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(logs, 1);

        // bots: alice がオーナーの 1 件のみ。
        let bots: i64 = dest
            .query_row("SELECT COUNT(*) FROM bots", [], |r| r.get(0))
            .unwrap();
        assert_eq!(bots, 1);
    }

    #[test]
    fn export_excludes_fts_shadow_tables() {
        let src = source_db();
        let dest = Connection::open_in_memory().expect("dest");
        export_user_data(&src, &dest, "alice").expect("export");
        // message_logs_fts のスキーマはコピーされない（NOT LIKE 'message_logs_fts%'）。
        let has_fts: i64 = dest
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'message_logs_fts%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_fts, 0);
    }

    #[test]
    fn copy_rows_skips_missing_table() {
        let src = source_db();
        let dest = Connection::open_in_memory().expect("dest");
        // 送り先に存在しないテーブル名でも prepare 失敗 → skip（Node try/catch）でエラーにしない。
        copy_rows(&src, &dest, "nonexistent_table", "user_id", "alice").expect("skip missing");
    }

    #[test]
    fn export_preserves_column_order() {
        // dest スキーマは src と同一定義でコピーされるため列順が保たれ、値が正しい列へ入る。
        let src = source_db();
        let dest = Connection::open_in_memory().expect("dest");
        export_user_data(&src, &dest, "alice").expect("export");
        let titles: Vec<String> = {
            let mut stmt = dest.prepare("SELECT title FROM todos ORDER BY id").unwrap();
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            rows
        };
        assert_eq!(titles, vec!["買い物".to_owned(), "掃除".to_owned()]);
    }

    #[test]
    fn backup_file_name_format() {
        let dt = chrono::Local
            .with_ymd_and_hms(2026, 6, 12, 4, 30, 5)
            .single()
            .expect("valid");
        assert_eq!(backup_file_name(dt), "yuuka_backup_20260612_043005.zip");
    }

    #[test]
    fn backup_file_name_always_has_prefix_and_ext() {
        let now = chrono::Local::now();
        let name = backup_file_name(now);
        assert!(name.starts_with(BACKUP_PREFIX));
        assert!(name.ends_with(".zip"));
        // yuuka_backup_ (13) + 8 + _ (1) + 6 + .zip (4) = 32。
        assert_eq!(name.len(), 32);
    }

    fn files(names: &[&str]) -> Vec<DriveFile> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| DriveFile {
                id: format!("id{i}"),
                name: (*n).to_owned(),
            })
            .collect()
    }

    #[test]
    fn stale_files_selects_beyond_generations() {
        // createdTime desc 前提。5 世代あり generations=3 → 4,5 件目（index 3,4）が古い＝削除対象。
        let list = files(&["f5", "f4", "f3", "f2", "f1"]);
        let stale = stale_files(&list, 3);
        assert_eq!(stale.len(), 2);
        assert_eq!(stale[0].name, "f2");
        assert_eq!(stale[1].name, "f1");
    }

    #[test]
    fn stale_files_keeps_all_when_within_generations() {
        let list = files(&["f3", "f2", "f1"]);
        assert!(stale_files(&list, 3).is_empty());
        assert!(stale_files(&list, 7).is_empty());
    }

    #[test]
    fn stale_files_clamps_generations_to_min_one() {
        // generations=0 は 1 にクランプ → 先頭 1 件を残し残りを削除。
        let list = files(&["f3", "f2", "f1"]);
        let stale = stale_files(&list, 0);
        assert_eq!(stale.len(), 2);
        assert_eq!(stale[0].name, "f2");
    }

    #[test]
    fn zip_single_file_roundtrips() {
        let payload = b"hello sqlite bytes";
        let zipped = zip_single_file("data/yuuka.db", payload).expect("zip");
        // ZIP マジック（PK\x03\x04）。
        assert_eq!(&zipped[0..2], b"PK");
        // 読み戻して内容とエントリ名を検証。
        let reader = std::io::Cursor::new(zipped);
        let mut archive = zip::ZipArchive::new(reader).expect("open zip");
        assert_eq!(archive.len(), 1);
        let mut file = archive.by_index(0).expect("entry");
        assert_eq!(file.name(), "data/yuuka.db");
        use std::io::Read as _;
        let mut out = Vec::new();
        file.read_to_end(&mut out).expect("read");
        assert_eq!(out, payload);
    }

    #[test]
    fn drive_metadata_includes_parents_only_when_folder_present() {
        let with = drive_metadata_json("yuuka_backup_x.zip", Some("FOLDER1"));
        let v: serde_json::Value = serde_json::from_str(&with).unwrap();
        assert_eq!(v["name"], "yuuka_backup_x.zip");
        assert_eq!(v["mimeType"], "application/zip");
        assert_eq!(v["parents"], serde_json::json!(["FOLDER1"]));

        let without = drive_metadata_json("yuuka_backup_x.zip", None);
        let v2: serde_json::Value = serde_json::from_str(&without).unwrap();
        assert!(v2.get("parents").is_none());
        // 空文字フォルダも parents 無し（Node の `folderId || undefined`）。
        let empty = drive_metadata_json("yuuka_backup_x.zip", Some(""));
        let v3: serde_json::Value = serde_json::from_str(&empty).unwrap();
        assert!(v3.get("parents").is_none());
    }

    #[test]
    fn multipart_body_has_two_parts_and_boundary() {
        let (content_type, body) =
            build_multipart_body("yuuka_backup_x.zip", Some("F1"), b"ZIPDATA");
        assert!(content_type.starts_with("multipart/related; boundary="));
        let text = String::from_utf8_lossy(&body);
        // metadata パート（JSON）と media パート（zip）の 2 パート + 終端。
        assert!(text.contains("Content-Type: application/json"));
        assert!(text.contains("Content-Type: application/zip"));
        assert!(text.contains("\"name\":\"yuuka_backup_x.zip\""));
        assert!(text.contains("\"parents\":[\"F1\"]"));
        assert!(text.contains("ZIPDATA"));
        // 終端境界。
        let boundary = content_type.trim_start_matches("multipart/related; boundary=");
        assert!(text.contains(&format!("--{boundary}--")));
    }

    #[test]
    fn parse_web_view_link_extracts_or_empty() {
        assert_eq!(
            parse_web_view_link(r#"{"id":"abc","webViewLink":"https://drive/x"}"#),
            "https://drive/x"
        );
        // webViewLink 不在 → 空文字（Node `|| ""`）。
        assert_eq!(parse_web_view_link(r#"{"id":"abc"}"#), "");
        assert_eq!(parse_web_view_link("garbage"), "");
    }

    #[test]
    fn parse_backup_files_filters_prefix_and_maps() {
        let body = r#"{
            "files":[
                {"id":"1","name":"yuuka_backup_20260612_010000.zip","createdTime":"2026-06-12T01:00:00Z"},
                {"id":"2","name":"other_file.zip","createdTime":"2026-06-11T01:00:00Z"},
                {"id":"3","name":"yuuka_backup_20260611_010000.zip"},
                {"name":"yuuka_backup_no_id.zip"},
                {"id":"5"}
            ]
        }"#;
        let out = parse_backup_files(body);
        // プレフィックス一致 + id/name 両方あるものだけ（順序保持）。
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "1");
        assert_eq!(out[1].id, "3");
    }

    #[test]
    fn parse_backup_files_empty_on_missing_or_garbage() {
        assert!(parse_backup_files(r#"{"kind":"x"}"#).is_empty());
        assert!(parse_backup_files("nope").is_empty());
        assert!(parse_backup_files(r#"{"files":[]}"#).is_empty());
    }
}
