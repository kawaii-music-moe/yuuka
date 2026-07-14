//! personal（連絡先）ドメインの Native ツール（現行 `src/functions/contactFunctions.ts` の移植）。
//!
//! 雛形は yuuka-todo `tools.rs`（構造・ok/fail JSON・bare ツール名・`UserScope`・
//! `DbError → ToolError::Execution`・`tools(db) -> Vec<Arc<dyn Tool>>` を踏襲）。
//!
//! **wire 契約の非対称に注意**: HTTP route の body は camelCase（`contactInfo`）だが、**tool 引数は
//! snake_case**（`contact_info`・Node の Gemini 宣言と一致）。ツール名は Node の system prompt が
//! 参照する **bare 名**（`addContact` 等・namespace 無し）を使う。
//!
//! 移植済み: addContact / listContacts / updateContact / deleteContact（既存 repo
//! add/list/get+update/delete に素直に対応）＋ クリップボード（一時メモ）の
//! addClipboardEntry / listClipboardEntries / deleteClipboardEntry（Node `clipboardFunctions.ts`）。
//! 未移植（deferred・repo 拡張要）: searchContacts（`ContactRepo` に部分一致検索メソッドが無い）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{DbError, Tool, ToolContext, ToolError, ToolName, ToolOutcome, UserScope};
use yuuka_web::Db;

use crate::dto::{NewContact, CONTEXT_NOTE_MAX_LENGTH};
use crate::repo::{ClipboardRepo, ContactRepo, ContextNoteRepo};

/// このドメインが公開する Native ツール一式を作る。
///
/// assembly 層（bot/WS）が `NativeProvider::register` で束ねる。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]（コンパイル時定数なので通常発生しない）。
pub fn tools(db: Db) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![
        Arc::new(AddContactTool {
            name: ToolName::checked("addContact".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(ListContactsTool {
            name: ToolName::checked("listContacts".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(UpdateContactTool {
            name: ToolName::checked("updateContact".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(DeleteContactTool {
            name: ToolName::checked("deleteContact".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(AddClipboardEntryTool {
            name: ToolName::checked("addClipboardEntry".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(ListClipboardEntriesTool {
            name: ToolName::checked("listClipboardEntries".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(DeleteClipboardEntryTool {
            name: ToolName::checked("deleteClipboardEntry".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(GetContextNoteTool {
            name: ToolName::checked("getContextNote".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(SetContextNoteTool {
            name: ToolName::checked("setContextNote".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(AppendContextNoteTool {
            name: ToolName::checked("appendContextNote".to_owned())?,
            db,
        }),
    ])
}

// ─── 共通ヘルパ ───────────────────────────────────────────────────────────────

/// `{success:true, message, ...extra}`（Node `ok(msg, extra)`）。
fn ok_payload(message: impl Into<String>, extra: Value) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("success".to_owned(), Value::Bool(true));
    obj.insert("message".to_owned(), Value::String(message.into()));
    if let Value::Object(map) = extra {
        for (k, v) in map {
            obj.insert(k, v);
        }
    }
    Value::Object(obj)
}

/// `{success:false, message}`（Node `fail(msg)`）。実行エラーではなく「妥当だが失敗」な結果。
fn fail_payload(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::from_payload(json!({ "success": false, "message": message.into() }))
}

/// ctx からデータ分離スコープを組む。
fn scope_of(ctx: &ToolContext) -> UserScope {
    UserScope::new(ctx.user_id.clone(), ctx.bot_id.clone())
}

/// `asOptionalString`（trim 後空なら None）。
fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// 数値、または数値文字列を i64 へ（Node `Number.isInteger` 相当・小数は弾く）。
fn arg_i64(args: &Value, key: &str) -> Option<i64> {
    let v = args.get(key)?;
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

/// 数値、または数値文字列を f64 へ（Node `Number(...)` 相当・TTL 用）。
fn arg_f64(args: &Value, key: &str) -> Option<f64> {
    let v = args.get(key)?;
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

/// JSON 配列引数を `Vec<String>`（文字列要素のみ採用）へ。
fn arg_str_vec(args: &Value, key: &str) -> Option<Vec<String>> {
    args.get(key).and_then(Value::as_array).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    })
}

/// DbError をツール実行エラーへ（握り潰さず Gemini へ `{success:false}` として返る・§8.4）。
fn exec_err(e: DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

/// `birthday` の形式検証（`'YYYY-MM-DD'` または `'--MM-DD'`）。Node `isValidBirthday` 相当。
fn is_valid_birthday(birthday: &str) -> bool {
    matches_pattern(birthday, "DDDD-DD-DD") || matches_pattern(birthday, "--DD-DD")
}

/// `pattern`（`D`=ASCII数字・その他=リテラル）に完全一致するか（indexing を避けて判定）。
fn matches_pattern(value: &str, pattern: &str) -> bool {
    if value.len() != pattern.len() {
        return false;
    }
    value.bytes().zip(pattern.bytes()).all(|(v, p)| match p {
        b'D' => v.is_ascii_digit(),
        other => v == other,
    })
}

const BAD_BIRTHDAY_MSG: &str =
    "誕生日は YYYY-MM-DD 形式（年不明なら --MM-DD 形式）で指定してください。";

// ─── addContact ──────────────────────────────────────────────────────────────

struct AddContactTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for AddContactTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "知り合いの連絡先やメモ（名前・誕生日・関係・連絡先・覚え書き）を新しく登録する。\
                「〜さんの誕生日は…」「同僚の…さんは…好き」のように人の情報を記録したい時に使う。\
                誕生日を入れておくと前日に自動でお知らせが届く。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "名前や呼び方（例: '田中太郎', '佐藤さん'）" },
                    "birthday": { "type": "string", "description": "誕生日 YYYY-MM-DD。年不明は --MM-DD（例 '--05-03'）（任意）" },
                    "relationship": { "type": "string", "description": "どんな関係か（例: '同僚', '家族', '友人'）（任意）" },
                    "contact_info": { "type": "string", "description": "電話番号やメールなどの連絡先（任意）" },
                    "notes": { "type": "string", "description": "自由なメモ（好みや覚えておきたいこと）（任意）" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "分類用のタグ（例 ['仕事']）（任意）" }
                },
                "required": ["name"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(name) = arg_str(&args, "name") else {
            return Ok(fail_payload("氏名は必須です。"));
        };
        let birthday = arg_str(&args, "birthday");
        if let Some(b) = &birthday {
            if !is_valid_birthday(b) {
                return Ok(fail_payload(BAD_BIRTHDAY_MSG));
            }
        }
        let new = NewContact {
            id: None,
            name,
            birthday,
            relationship: arg_str(&args, "relationship"),
            contact_info: arg_str(&args, "contact_info"),
            notes: arg_str(&args, "notes"),
            tags: arg_str_vec(&args, "tags").unwrap_or_default(),
        };
        let contact = ContactRepo::new(&self.db)
            .add(&scope_of(ctx), new)
            .await
            .map_err(exec_err)?;
        let message = format!("連絡先「{}」を登録しました👤", contact.name);
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({ "contact": contact }),
        )))
    }
}

// ─── listContacts ────────────────────────────────────────────────────────────

struct ListContactsTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ListContactsTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "登録ずみの連絡先をすべて一覧で取り出す。\
                「連絡先を見せて」「登録した人を全部出して」と言われた時に使う。"
                .to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        let contacts = ContactRepo::new(&self.db)
            .list(&scope_of(ctx))
            .await
            .map_err(exec_err)?;
        // Node listContacts は `{success, count, contacts}`（message は付けない）。
        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "count": contacts.len(),
            "contacts": contacts,
        })))
    }
}

// ─── updateContact ───────────────────────────────────────────────────────────

struct UpdateContactTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for UpdateContactTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "すでにある連絡先の情報を書き換える。変えたい項目だけを渡す\
                （触らない項目は指定しない）。メモは渡した全文で丸ごと置き換わる。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "contact_id": { "type": "number", "description": "書き換えたい連絡先の ID" },
                    "name": { "type": "string", "description": "名前。変える時だけ指定（任意）" },
                    "birthday": { "type": "string", "description": "誕生日 YYYY-MM-DD または --MM-DD。変える時だけ（任意）" },
                    "relationship": { "type": "string", "description": "どんな関係か。変える時だけ（任意）" },
                    "contact_info": { "type": "string", "description": "電話番号やメールなど。変える時だけ（任意）" },
                    "notes": { "type": "string", "description": "メモ。変える時だけ（全文で置換）（任意）" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "分類用のタグ。変える時だけ（任意）" }
                },
                "required": ["contact_id"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(id) = arg_i64(&args, "contact_id") else {
            return Ok(fail_payload("contact_id が不正です。"));
        };
        let repo = ContactRepo::new(&self.db);
        let scope = scope_of(ctx);

        // 現在値を取得（無ければ not-found）。以降は「指定された項目だけ上書き」する部分更新。
        let Some(current) = repo.get(&scope, id).await.map_err(exec_err)? else {
            return Ok(fail_payload("指定された連絡先が見つかりません。"));
        };

        // birthday: 指定があれば検証して上書き、無ければ現在値を保持。
        let birthday = match arg_str(&args, "birthday") {
            Some(b) => {
                if !is_valid_birthday(&b) {
                    return Ok(fail_payload(BAD_BIRTHDAY_MSG));
                }
                Some(b)
            }
            None => current.birthday.clone(),
        };

        let merged = NewContact {
            id: Some(id),
            name: arg_str(&args, "name").unwrap_or_else(|| current.name.clone()),
            birthday,
            relationship: arg_str(&args, "relationship").or_else(|| current.relationship.clone()),
            contact_info: arg_str(&args, "contact_info").or_else(|| current.contact_info.clone()),
            notes: arg_str(&args, "notes").or_else(|| current.notes.clone()),
            tags: arg_str_vec(&args, "tags").unwrap_or_else(|| current.tags.clone()),
        };

        match repo.update(&scope, id, merged).await.map_err(exec_err)? {
            Some(contact) => {
                let message = format!("連絡先「{}」を更新しました👤", contact.name);
                Ok(ToolOutcome::from_payload(ok_payload(
                    message,
                    json!({ "contact": contact }),
                )))
            }
            None => Ok(fail_payload("更新に失敗しました。")),
        }
    }
}

// ─── deleteContact ───────────────────────────────────────────────────────────

struct DeleteContactTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for DeleteContactTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "連絡先を削除する（元に戻せない）。消す前にどの人を消すかユーザーに確認してから呼ぶ。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": { "contact_id": { "type": "number", "description": "削除したい連絡先の ID" } },
                "required": ["contact_id"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(id) = arg_i64(&args, "contact_id") else {
            return Ok(fail_payload("contact_id が不正です。"));
        };
        let ok = ContactRepo::new(&self.db)
            .delete(&scope_of(ctx), id)
            .await
            .map_err(exec_err)?;
        if ok {
            Ok(ToolOutcome::from_payload(ok_payload(
                "連絡先を削除しました🗑️",
                json!({}),
            )))
        } else {
            Ok(fail_payload("指定された連絡先が見つかりませんでした。"))
        }
    }
}

// ─── addClipboardEntry ─────────────────────────────────────────────────────────

struct AddClipboardEntryTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for AddClipboardEntryTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "「今日だけ」の一時メモをクリップボードに保存する。期限が来ると自動で消える（既定24時間）。\
                ずっと使う情報はこれではなく appendContextNote を使う。「今週中だけ」→ ttl_hours:168、\
                「1時間後に消して」→ ttl_hours:1 のように変換する。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "メモする内容" },
                    "ttl_hours": { "type": "number", "description": "何時間で自動削除するか。省略=24時間。0=ずっと消えない" }
                },
                "required": ["content"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        // Node: `String(args.content ?? "").trim()` が空なら fail。
        let Some(content) = arg_str(&args, "content") else {
            return Ok(fail_payload("メモの内容が空です。"));
        };
        // Node: `args.ttl_hours === undefined ? 24 : Number(args.ttl_hours)`。非有限/負は fail。
        let ttl = if args.get("ttl_hours").is_none() {
            24.0
        } else {
            match arg_f64(&args, "ttl_hours") {
                Some(v) if v.is_finite() && v >= 0.0 => v,
                _ => {
                    return Ok(fail_payload(
                        "ttl_hours は0以上の数値で指定してください。",
                    ))
                }
            }
        };
        // 0 = 無期限（expires_at NULL）。それ以外は now + ttl 時間。
        let ttl_hours = if ttl == 0.0 { None } else { Some(ttl) };
        let entry = ClipboardRepo::new(&self.db)
            .add(&scope_of(ctx), content, ttl_hours)
            .await
            .map_err(exec_err)?;
        let message = if ttl == 0.0 {
            "クリップボードに保存しました（無期限）📎".to_owned()
        } else {
            format!("クリップボードに保存しました（{ttl}時間後に自動削除）📎")
        };
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({ "entry": {
                "id": entry.id,
                "content": entry.content,
                "expires_at": entry.expires_at,
            } }),
        )))
    }
}

// ─── listClipboardEntries ──────────────────────────────────────────────────────

struct ListClipboardEntriesTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ListClipboardEntriesTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "クリップボード（一時メモ）の今も有効なメモを一覧で取り出す。\
                「メモを見せて」「さっきクリップした内容は？」と言われた時に呼ぶ。"
                .to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        let entries = ClipboardRepo::new(&self.db)
            .list(&scope_of(ctx))
            .await
            .map_err(exec_err)?;
        // Node: `expires_at ?? "無期限"`（無期限は文字列で返す）・created_at も同梱。
        let mapped: Vec<Value> = entries
            .iter()
            .map(|e| {
                json!({
                    "id": e.id,
                    "content": e.content,
                    "expires_at": e.expires_at.clone().unwrap_or_else(|| "無期限".to_owned()),
                    "created_at": e.created_at,
                })
            })
            .collect();
        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "count": entries.len(),
            "entries": mapped,
        })))
    }
}

// ─── deleteClipboardEntry ──────────────────────────────────────────────────────

struct DeleteClipboardEntryTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for DeleteClipboardEntryTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "クリップボードの指定したメモを1件削除する。消し間違えないよう、\
                先に listClipboardEntries で正しいIDを確かめてから呼ぶ。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "entry_id": { "type": "number", "description": "削除するメモのID（listClipboardEntries で確認した番号）" }
                },
                "required": ["entry_id"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        // Node: `Number.isInteger(Number(entry_id))` でなければ fail。
        let Some(id) = arg_i64(&args, "entry_id") else {
            return Ok(fail_payload("entry_id が不正です。"));
        };
        let ok = ClipboardRepo::new(&self.db)
            .delete(&scope_of(ctx), id)
            .await
            .map_err(exec_err)?;
        // Node: `{success: ok, message}`（見つからない場合も success:false + 定型文）。
        let message = if ok {
            "メモを削除しました🗑️"
        } else {
            "指定されたメモが見つかりませんでした。"
        };
        Ok(ToolOutcome::from_payload(
            json!({ "success": ok, "message": message }),
        ))
    }
}

/// 整数を 3 桁区切りにする（Node `Number.toLocaleString()` の桁区切り・上限メッセージ用）。
fn format_thousands(n: usize) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i != 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

// ─── getContextNote ──────────────────────────────────────────────────────────

struct GetContextNoteTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for GetContextNoteTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "自由記述のメモ帳（コンテキストノート）の現在の全文を読む。\
                「メモ帳見せて」等や、setContextNote で書き換える前に現在の中身を確認する時に呼ぶ。"
                .to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        let (content, _updated) = ContextNoteRepo::new(&self.db)
            .get(&scope_of(ctx))
            .await
            .map_err(exec_err)?;
        let length = content.chars().count();
        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "content": content,
            "length": length,
            "max_length": CONTEXT_NOTE_MAX_LENGTH,
        })))
    }
}

// ─── setContextNote ──────────────────────────────────────────────────────────

struct SetContextNoteTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for SetContextNoteTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: format!(
                "メモ帳（コンテキストノート）の全文を書き換える（全置換）。{}文字まで。\
                 誤って消さないため、先に getContextNote で今の中身を読み、書き換え後の全文をユーザーに見せて承認を得てから呼ぶ。",
                format_thousands(CONTEXT_NOTE_MAX_LENGTH)
            ),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": format!("書き換え後のメモ帳の全文（{}文字まで・改行区切りの箇条書き推奨）", format_thousands(CONTEXT_NOTE_MAX_LENGTH)) }
                },
                "required": ["content"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        // Node: `String(args.content ?? "")`（trim せず・空許容）。
        let content = args.get("content").and_then(Value::as_str).unwrap_or("");
        let length = content.chars().count();
        if length > CONTEXT_NOTE_MAX_LENGTH {
            return Ok(fail_payload(format!(
                "コンテキストノートは{}文字以内です（現在: {}文字）",
                format_thousands(CONTEXT_NOTE_MAX_LENGTH),
                format_thousands(length),
            )));
        }
        ContextNoteRepo::new(&self.db)
            .set(&scope_of(ctx), content.to_owned())
            .await
            .map_err(exec_err)?;
        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "message": "コンテキストノートを更新しました📝",
            "total_length": length,
        })))
    }
}

// ─── appendContextNote ───────────────────────────────────────────────────────

struct AppendContextNoteTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for AppendContextNoteTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "メモ帳（コンテキストノート）に1行追記する。ユーザーについて覚えておくべき\
                事実（好み・約束・背景など）を残す時に呼ぶ。全体を整理し直す時は setContextNote を使う。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "追記する内容（1行・要点を簡潔に）" }
                },
                "required": ["content"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let trimmed = args
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        if trimmed.is_empty() {
            return Ok(fail_payload("記憶する内容が空です。"));
        }
        let scope = scope_of(ctx);
        let repo = ContextNoteRepo::new(&self.db);
        let (current, _updated) = repo.get(&scope).await.map_err(exec_err)?;
        // Node parity: 現在が空でなければ改行で連結。
        let next = if current.is_empty() {
            trimmed
        } else {
            format!("{current}\n{trimmed}")
        };
        let next_len = next.chars().count();
        if next_len > CONTEXT_NOTE_MAX_LENGTH {
            return Ok(fail_payload(format!(
                "コンテキストノートの上限（{}文字）を超えるため追記できません。不要な項目を整理してから追記してください（現在: {}文字）",
                format_thousands(CONTEXT_NOTE_MAX_LENGTH),
                format_thousands(current.chars().count()),
            )));
        }
        repo.set(&scope, next).await.map_err(exec_err)?;
        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "message": "コンテキストノートに追記しました📝",
            "total_length": next_len,
            "max_length": CONTEXT_NOTE_MAX_LENGTH,
        })))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;
    use yuuka_core::{BotId, UserId};

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // 実効スキーマは `Db::open`（= run_migrations）が V17__baseline.sql を丸ごと適用して作る
    // （contacts + clipboard_entries を含む）。部分 DDL は後続 INDEX と衝突しうるため空ファイルから
    // マイグレーションに一任する（他ドメインのテストと同方針）。
    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_personal_tools_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            drop(conn);
        }
        let db = Db::open(&path).unwrap();
        // contacts/clipboard_entries は users への FK を張るため、対象ユーザーを先に作る。
        {
            let conn = Connection::open(&path).unwrap();
            for uid in ["userA", "userB"] {
                conn.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                     VALUES (?1, ?1, 'x', 'x')",
                    rusqlite::params![uid],
                )
                .unwrap();
            }
        }
        db
    }

    fn ctx() -> ToolContext {
        ToolContext::new(BotId::system_default(), UserId::new("userA"))
    }

    fn find<'a>(tools: &'a [Arc<dyn Tool>], name: &str) -> &'a Arc<dyn Tool> {
        tools
            .iter()
            .find(|t| t.declaration().name.as_str() == name)
            .unwrap()
    }

    #[tokio::test]
    async fn add_list_update_delete_roundtrip() {
        let db = seed_db();
        let tools = tools(db).unwrap();

        // 宣言名は bare（Node system prompt と一致）。
        let names: Vec<String> = tools
            .iter()
            .map(|t| t.declaration().name.to_string())
            .collect();
        assert!(names.contains(&"addContact".to_owned()));
        assert!(!names.iter().any(|n| n.contains(':')), "native は bare 名");

        // add（snake_case 引数）。
        let add = find(&tools, "addContact");
        let out = add
            .call(
                &ctx(),
                json!({
                    "name": "田中太郎",
                    "birthday": "--05-03",
                    "contact_info": "tanaka@example.com",
                    "tags": ["同僚"]
                }),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["contact"]["name"], "田中太郎");
        assert_eq!(out.payload["contact"]["birthday"], "--05-03");
        assert_eq!(out.payload["contact"]["contact_info"], "tanaka@example.com");
        assert_eq!(out.payload["contact"]["tags"][0], "同僚");
        let id = out.payload["contact"]["id"].as_i64().unwrap();

        // list（count + contacts、message なし）。
        let list = find(&tools, "listContacts");
        let out = list.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["count"], 1);
        assert_eq!(out.payload["contacts"].as_array().unwrap().len(), 1);

        // update（relationship だけ渡す → 他項目は保持・H-1 相当の消去防止）。
        let update = find(&tools, "updateContact");
        let out = update
            .call(&ctx(), json!({"contact_id": id, "relationship": "同僚"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["contact"]["relationship"], "同僚");
        // 未指定の contact_info / birthday / name は保持される。
        assert_eq!(out.payload["contact"]["contact_info"], "tanaka@example.com");
        assert_eq!(out.payload["contact"]["birthday"], "--05-03");
        assert_eq!(out.payload["contact"]["name"], "田中太郎");

        // update 不正 birthday → fail。
        let out = update
            .call(&ctx(), json!({"contact_id": id, "birthday": "not-a-date"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // 存在しない id の update → not-found fail。
        let out = update
            .call(&ctx(), json!({"contact_id": 999999, "name": "x"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // delete。
        let delete = find(&tools, "deleteContact");
        let out = delete.call(&ctx(), json!({"contact_id": id})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        // 二重削除は not-found（success:false）。
        let out = delete.call(&ctx(), json!({"contact_id": id})).await.unwrap();
        assert_eq!(out.payload["success"], false);
    }

    #[tokio::test]
    async fn add_validates_name_and_birthday() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addContact");

        // name 欠落 → fail。
        let out = add.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // 空白のみの name → fail。
        let out = add.call(&ctx(), json!({"name": "   "})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // 不正 birthday → fail。
        let out = add
            .call(&ctx(), json!({"name": "x", "birthday": "bad"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // 正しい birthday（YYYY-MM-DD）→ 成功。
        let out = add
            .call(&ctx(), json!({"name": "y", "birthday": "1990-01-02"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["contact"]["birthday"], "1990-01-02");
    }

    #[tokio::test]
    async fn scope_isolation_across_users() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addContact");
        let list = find(&tools, "listContacts");

        add.call(&ctx(), json!({"name": "A の知人"})).await.unwrap();

        // 別ユーザーには見えない。
        let ctx_b = ToolContext::new(BotId::system_default(), UserId::new("userB"));
        let out = list.call(&ctx_b, json!({})).await.unwrap();
        assert_eq!(out.payload["count"], 0);
        assert_eq!(out.payload["contacts"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn clipboard_add_list_delete_roundtrip() {
        let db = seed_db();
        let tools = tools(db).unwrap();

        // 3 ツールが bare 名で登録されている。
        let names: Vec<String> = tools
            .iter()
            .map(|t| t.declaration().name.to_string())
            .collect();
        for n in ["addClipboardEntry", "listClipboardEntries", "deleteClipboardEntry"] {
            assert!(names.contains(&n.to_owned()), "missing tool: {n}");
        }

        let add = find(&tools, "addClipboardEntry");
        // 既定 24 時間（expires_at は非 null）。
        let out = add
            .call(&ctx(), json!({"content": "会議メモ"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["message"], "クリップボードに保存しました（24時間後に自動削除）📎");
        assert_eq!(out.payload["entry"]["content"], "会議メモ");
        assert!(out.payload["entry"]["expires_at"].is_string());
        let id = out.payload["entry"]["id"].as_i64().unwrap();

        // ttl_hours:0 → 無期限（expires_at null・メッセージも無期限）。
        let out = add
            .call(&ctx(), json!({"content": "ずっと", "ttl_hours": 0}))
            .await
            .unwrap();
        assert_eq!(out.payload["message"], "クリップボードに保存しました（無期限）📎");
        assert!(out.payload["entry"]["expires_at"].is_null());

        // 空 content → fail。
        let out = add.call(&ctx(), json!({"content": "  "})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // 負の ttl → fail。
        let out = add
            .call(&ctx(), json!({"content": "x", "ttl_hours": -1}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // list（有効エントリ 2 件・無期限は "無期限" 文字列）。
        let list = find(&tools, "listClipboardEntries");
        let out = list.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["count"], 2);
        let entries = out.payload["entries"].as_array().unwrap();
        assert!(entries.iter().any(|e| e["expires_at"] == "無期限"));

        // delete（成功 → 見つからない）。
        let delete = find(&tools, "deleteClipboardEntry");
        let out = delete.call(&ctx(), json!({"entry_id": id})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["message"], "メモを削除しました🗑️");
        let out = delete.call(&ctx(), json!({"entry_id": id})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // entry_id 非整数 → fail。
        let out = delete.call(&ctx(), json!({"entry_id": 1.5})).await.unwrap();
        assert_eq!(out.payload["success"], false);
        assert_eq!(out.payload["message"], "entry_id が不正です。");
    }

    #[tokio::test]
    async fn clipboard_scope_isolation() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let add = find(&tools, "addClipboardEntry");
        let list = find(&tools, "listClipboardEntries");

        add.call(&ctx(), json!({"content": "A のメモ"})).await.unwrap();

        let ctx_b = ToolContext::new(BotId::system_default(), UserId::new("userB"));
        let out = list.call(&ctx_b, json!({})).await.unwrap();
        assert_eq!(out.payload["count"], 0);
    }

    #[tokio::test]
    async fn context_note_get_set_append() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let get = find(&tools, "getContextNote");
        let set = find(&tools, "setContextNote");
        let append = find(&tools, "appendContextNote");

        // 初期は空。
        let out = get.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["content"], "");
        assert_eq!(out.payload["length"], 0);

        // set: 全置換。
        let out = set.call(&ctx(), json!({"content": "コーヒーはブラック"})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        let out = get.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["content"], "コーヒーはブラック");

        // append: 改行で連結。
        let out = append.call(&ctx(), json!({"content": "犬を飼っている"})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        let out = get.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["content"], "コーヒーはブラック\n犬を飼っている");

        // append 空 → fail。
        let out = append.call(&ctx(), json!({"content": "   "})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // set 上限超過 → fail（scope は保持）。
        let big = "あ".repeat(super::CONTEXT_NOTE_MAX_LENGTH + 1);
        let out = set.call(&ctx(), json!({"content": big})).await.unwrap();
        assert_eq!(out.payload["success"], false);
        // 別ユーザーには漏れない。
        let ctx_b = ToolContext::new(BotId::system_default(), UserId::new("userB"));
        let out = get.call(&ctx_b, json!({})).await.unwrap();
        assert_eq!(out.payload["content"], "");
    }
}
