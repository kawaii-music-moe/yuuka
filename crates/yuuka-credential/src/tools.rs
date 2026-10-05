//! credential ドメインの Native ツール（現行 `src/functions/credentialFunctions.ts` の移植・§6.4）。
//!
//! **参照実装**: `yuuka-todo::tools`（構造・命名・ok/fail JSON・bare ツール名・`UserScope`・
//! `DbError`→`ToolError::Execution` を踏襲）。core の凍結 [`Tool`] を実装し、`tools(db)` が
//! `Vec<Arc<dyn Tool>>` を返す（assembly 層が `NativeProvider` へ登録する）。
//!
//! **wire 契約の非対称に注意**: HTTP route の body は camelCase（`serviceName`）だが、**tool 引数は
//! snake_case**（`service_name`・Node の Gemini 宣言と一致）。ツール名は Node の system prompt が
//! 参照する **bare 名**（`listCredentialServices` / `deleteCredential`）を使う。
//!
//! **機密フェイルクローズ**: 一覧は暗号化列（`encrypted_password`/`iv`/`auth_tag`）を **決して
//! 露出しない**（repo が SELECT しない・§6.4）。パスワード値はツールの応答に一切含めない。
//!
//! 移植済み: listCredentialServices / deleteCredential / addCredential / updateCredential
//! （ユーザー鍵暗号化 = `SystemCrypto::encrypt_for_user`・salt は `users.salt`・crypto 注入）+
//! **browserFillCredential**（2026-07-15o＝Bot 許可ゲート〔`is_granted`〕→`get_full`+`decrypt_for_user`
//! で復号→共有 `BrowserManager::type_text` へ**直接入力**〔復号値は LLM 応答・ログに非露出・§6.3.2〕・
//! 失敗文言は秘匿値を除去〔`sanitize_error`〕）。許可フィルタ・grant 掃除も配線済み（2026-07-15a）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{DbError, Tool, ToolContext, ToolError, ToolName, ToolOutcome, UserScope};
use yuuka_web::Db;

use yuuka_browser::BrowserManager;
use yuuka_crypto::SystemCrypto;

use crate::access::CredentialAccessRepo;
use crate::repo::{check_field_lengths, CredentialRepo};

/// このドメインが公開する Native ツール一式を作る。
///
/// assembly 層（bot/WS）が `NativeProvider::register` で束ねる。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]（コンパイル時定数なので通常発生しない）。
/// `crypto` は保存時暗号化（ユーザー鍵）に使う。未設定（暗号鍵無し）なら add/update は 500 相当で
/// 縮退する（register/update は暗号化できないため）。
pub fn tools(
    db: Db,
    crypto: Option<Arc<SystemCrypto>>,
    browser: Option<Arc<BrowserManager>>,
) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![
        Arc::new(ListCredentialServicesTool {
            name: ToolName::checked("listCredentialServices".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(DeleteCredentialTool {
            name: ToolName::checked("deleteCredential".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(AddCredentialTool {
            name: ToolName::checked("addCredential".to_owned())?,
            db: db.clone(),
            crypto: crypto.clone(),
        }),
        Arc::new(UpdateCredentialTool {
            name: ToolName::checked("updateCredential".to_owned())?,
            db: db.clone(),
            crypto: crypto.clone(),
        }),
        Arc::new(BrowserFillCredentialTool {
            name: ToolName::checked("browserFillCredential".to_owned())?,
            db,
            crypto,
            browser,
        }),
    ])
}

/// ユーザー鍵でパスワードを暗号化する（crypto 未設定/ salt 無しは `Err(fail 文言)`）。
async fn encrypt_password(
    db: &Db,
    crypto: &Option<Arc<SystemCrypto>>,
    user_id: &str,
    password: &str,
) -> Result<yuuka_crypto::Encrypted, ToolOutcome> {
    let Some(crypto) = crypto else {
        return Err(fail_payload("暗号化が未設定のため保存できません。"));
    };
    let salt = CredentialRepo::new(db)
        .user_salt(user_id)
        .await
        .map_err(|e| {
            ToolOutcome::from_payload(json!({ "success": false, "message": e.to_string() }))
        })?;
    let Some(salt) = salt else {
        return Err(fail_payload("ユーザー情報が見つかりません。"));
    };
    crypto
        .encrypt_for_user(&salt, password)
        .map_err(|_| fail_payload("パスワードの暗号化に失敗しました。"))
}

// ─── 共通ヘルパ（todo/tools.rs と同一規約） ────────────────────────────────────

/// `{success:true, message, ...extra}`（Node `ok(msg, extra)` 相当）。
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

/// ctx からデータ分離スコープを組む（credential 表は user_id のみで束ねる・§12.2 契約5）。
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

/// `asOptionalPassword`（空文字のみ None・**前後空白は意味を持ち得るため trim しない**・Node
/// `credentialFunctions.asOptionalPassword` と一致）。パスワードを trim すると保存値が入力と食い違い、
/// 後で認証できなくなる（サイレントな資格情報破損）ため、パスワード列は決して trim しない。
fn arg_password(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// url 引数を「未指定 / 指定あり（空文字＝削除）」で解釈する（Node updateCredential の
/// `typeof args.url === "string" ? args.url : undefined` + secretService `trim() || null` パリティ）。
/// 返り: `None` = 未指定（現行維持）・`Some(None)` = 指定あり空＝削除・`Some(Some(u))` = 新 URL。
fn arg_url_update(args: &Value) -> Option<Option<String>> {
    match args.get("url") {
        Some(Value::String(s)) => {
            let t = s.trim();
            Some((!t.is_empty()).then(|| t.to_owned()))
        }
        _ => None,
    }
}

/// DbError をツール実行エラーへ（握り潰さず Gemini へ `{success:false}` として返る・§8.4）。
fn exec_err(e: DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

// ─── listCredentialServices ───────────────────────────────────────────────────

struct ListCredentialServicesTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ListCredentialServicesTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "パスワード保管庫に登録済みのサービス名・ユーザー名・URLの一覧を見る（パスワードは出ない）。\n\
                ・例:「どのサービスのアカウントを登録してる？」と聞かれた時。\n\
                ・ブラウザ自動ログイン（browserFillCredential）の前に、正しいサービス名を確かめたい時にも使う。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {}
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        // v5: 応対 Bot へ利用許可済み（bot_credential_access）の service だけ返す（HTTP GET・
        // browserFillCredential の isCredentialGrantedToBot ゲートと一致・未許可 service は露出しない）。
        let granted: std::collections::HashSet<String> = CredentialAccessRepo::new(&self.db)
            .list_credential_names_for_bot(ctx.bot_id.as_str(), ctx.user_id.as_str())
            .await
            .map_err(exec_err)?
            .into_iter()
            .collect();
        let services: Vec<_> = CredentialRepo::new(&self.db)
            .list(&scope_of(ctx))
            .await
            .map_err(exec_err)?
            .into_iter()
            .filter(|s| granted.contains(&s.service_name))
            .collect();

        if services.is_empty() {
            return Ok(ToolOutcome::from_payload(ok_payload(
                "このBotが利用を許可された認証情報はありません。addCredential で登録すると自動的にこのBotへ許可されます。既存の認証情報は統合管理ページ（Bot統合管理）でこのBotへ利用を許可してください。",
                json!({ "services": [] }),
            )));
        }

        let lines: Vec<String> = services
            .iter()
            .map(|s| {
                let url_part = match s.url.as_deref().filter(|u| !u.is_empty()) {
                    Some(u) => format!("、URL: {u}"),
                    None => String::new(),
                };
                format!(
                    "🔑 {}（ユーザー名: {}{}）",
                    s.service_name, s.username, url_part
                )
            })
            .collect();
        let message = format!(
            "登録済みサービス一覧 ({}件):\n{}\n※パスワードは表示できません。",
            services.len(),
            lines.join("\n"),
        );
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({ "services": services }),
        )))
    }
}

// ─── deleteCredential ─────────────────────────────────────────────────────────

struct DeleteCredentialTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for DeleteCredentialTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "登録済みのログイン情報を保管庫から完全に消す（取り消せない）。\n\
                ・ユーザーが「消して」とはっきり頼んだ時だけ呼ぶ。\n\
                ・呼ぶ前に、消すサービス名をユーザーに読み上げて確認をもらう。\n\
                ・確認なしに勝手に消さない。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "service_name": {
                        "type": "string",
                        "description": "消したいサービス名。listCredentialServices で確認できる。"
                    }
                },
                "required": ["service_name"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(service_name) = arg_str(&args, "service_name") else {
            return Ok(fail_payload("service_name を指定してください。"));
        };

        // repo.delete が内部で正規化（trim + 小文字化）して照合する（Node と一致）。
        let deleted = CredentialRepo::new(&self.db)
            .delete(&scope_of(ctx), &service_name)
            .await
            .map_err(exec_err)?;

        if deleted {
            // v5: 削除に伴い全 Bot の利用許可も掃除する（credentials への DB FK が無いため明示的に。
            // 残すと同名再登録時に意図せず許可が復活する・Node deleteCredential）。grant 側で正規化。
            CredentialAccessRepo::new(&self.db)
                .delete_all_grants(ctx.user_id.as_str(), &service_name)
                .await
                .map_err(exec_err)?;
            // 成功メッセージは正規化名（Node `serviceName.trim().toLowerCase()` と一致）。
            Ok(ToolOutcome::from_payload(ok_payload(
                format!(
                    "「{}」の認証情報を削除しました🗑️",
                    service_name.to_lowercase()
                ),
                json!({}),
            )))
        } else {
            // not-found は Node と同じく入力名（trim 済み・原ケース）で示す。
            Ok(fail_payload(format!(
                "サービス「{service_name}」の認証情報が見つかりません。listCredentialServices で登録済みサービス名を確認してください。"
            )))
        }
    }
}

// ─── addCredential ───────────────────────────────────────────────────────────

struct AddCredentialTool {
    name: ToolName,
    db: Db,
    crypto: Option<Arc<SystemCrypto>>,
}

#[async_trait]
impl Tool for AddCredentialTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "サービスのログイン情報（ユーザー名・パスワード）を暗号化して保存する。\
                パスワードはユーザー鍵で暗号化され、返信に値を含めてはいけない。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "service_name": { "type": "string", "description": "サービス名（例: GitHub）" },
                    "username": { "type": "string", "description": "ユーザー名／メールアドレス" },
                    "password": { "type": "string", "description": "パスワード" },
                    "url": { "type": "string", "description": "ログインURL（任意）" }
                },
                "required": ["service_name", "username", "password"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        // password は trim しない（前後空白も有効・Node `asOptionalPassword`）。service/username は trim。
        let (Some(service), Some(username), Some(password)) = (
            arg_str(&args, "service_name"),
            arg_str(&args, "username"),
            arg_password(&args, "password"),
        ) else {
            return Ok(fail_payload(
                "service_name・username・password は必須です。",
            ));
        };
        let url = arg_str(&args, "url");
        if let Some(msg) =
            check_field_lengths(&service, Some(&username), Some(&password), url.as_deref())
        {
            return Ok(fail_payload(msg));
        }
        let enc =
            match encrypt_password(&self.db, &self.crypto, ctx.user_id.as_str(), &password).await {
                Ok(e) => e,
                Err(outcome) => return Ok(outcome),
            };
        CredentialRepo::new(&self.db)
            .save(&scope_of(ctx), &service, username.clone(), url, enc)
            .await
            .map_err(exec_err)?;
        // v5: 会話から登録した認証情報は、いま応対している（秘書）Bot 自身へ即時に利用許可する（UX 維持・
        // Node addCredential）。他 Bot への共有は統合管理ページで行う。grant は正規化・冪等。
        CredentialAccessRepo::new(&self.db)
            .grant(ctx.bot_id.as_str(), ctx.user_id.as_str(), &service)
            .await
            .map_err(exec_err)?;
        Ok(ToolOutcome::from_payload(ok_payload(
            format!(
                "「{}」の認証情報を暗号化して登録しました🔐（ユーザー名: {}）。返信にパスワードの値を含めず、ユーザーにはチャット上のパスワード送信メッセージの削除を勧めてください。",
                service.to_lowercase(),
                username
            ),
            json!({}),
        )))
    }
}

// ─── updateCredential ────────────────────────────────────────────────────────

struct UpdateCredentialTool {
    name: ToolName,
    db: Db,
    crypto: Option<Arc<SystemCrypto>>,
}

#[async_trait]
impl Tool for UpdateCredentialTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "既存の認証情報のユーザー名／パスワード／URL を変更する。指定した項目だけ\
                更新する。返信にパスワードの値を含めてはいけない。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "service_name": { "type": "string", "description": "更新するサービス名" },
                    "username": { "type": "string", "description": "新しいユーザー名（任意）" },
                    "password": { "type": "string", "description": "新しいパスワード（任意）" },
                    "url": { "type": "string", "description": "新しいログインURL（任意）" }
                },
                "required": ["service_name"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(service) = arg_str(&args, "service_name") else {
            return Ok(fail_payload("service_name を指定してください。"));
        };
        let username = arg_str(&args, "username");
        // password は trim しない（前後空白も有効・Node `asOptionalPassword`）。
        let password = arg_password(&args, "password");
        // url は「指定あり空文字＝削除」を許容する（Node は url だけ別パーサ・下記 helper）。
        let url_arg = arg_url_update(&args);
        if username.is_none() && password.is_none() && url_arg.is_none() {
            return Ok(fail_payload(
                "更新する項目（username / password / url）を1つ以上指定してください。",
            ));
        }
        // 長さ検証は指定ありの新 URL（trim 済み）を見る（削除=空/未指定はスキップ・Node と同結果）。
        let url_for_check = url_arg.as_ref().and_then(|o| o.as_deref());
        if let Some(msg) = check_field_lengths(
            &service,
            username.as_deref(),
            password.as_deref(),
            url_for_check,
        ) {
            return Ok(fail_payload(msg));
        }
        let scope = scope_of(ctx);
        let repo = CredentialRepo::new(&self.db);
        let Some((cur_user, cur_url, cur_enc, cur_iv, cur_tag)) =
            repo.get_full(&scope, &service).await.map_err(exec_err)?
        else {
            return Ok(fail_payload(format!(
                "サービス「{}」の認証情報が見つかりません。listCredentialServices で登録済みサービス名を確認してください。",
                service.trim().to_lowercase()
            )));
        };
        // 部分マージ: 指定項目のみ差し替え。password 指定時のみ再暗号化。url は指定あり空文字で削除。
        let username_provided = username.is_some();
        let password_provided = password.is_some();
        let url_provided = url_arg.is_some();
        let new_user = username.unwrap_or(cur_user);
        let new_url = match url_arg {
            Some(inner) => inner, // 指定あり: 新 URL（Some）または削除（None）。
            None => cur_url,      // 未指定: 現行維持。
        };
        let enc = if let Some(pw) = &password {
            match encrypt_password(&self.db, &self.crypto, ctx.user_id.as_str(), pw).await {
                Ok(e) => e,
                Err(outcome) => return Ok(outcome),
            }
        } else {
            yuuka_crypto::Encrypted {
                encrypted: cur_enc,
                iv: cur_iv,
                auth_tag: cur_tag,
            }
        };
        repo.save(&scope, &service, new_user, new_url, enc)
            .await
            .map_err(exec_err)?;
        let mut changed = Vec::new();
        if username_provided {
            changed.push("ユーザー名");
        }
        if password_provided {
            changed.push("パスワード");
        }
        if url_provided {
            changed.push("URL");
        }
        Ok(ToolOutcome::from_payload(ok_payload(
            format!(
                "「{}」の{}を更新しました🔐。返信にパスワードの値を含めないでください。",
                service.trim().to_lowercase(),
                changed.join("・")
            ),
            json!({}),
        )))
    }
}

// ─── browserFillCredential ───────────────────────────────────────────────────

/// エラー文言から秘匿値を除去する（Node `sanitizeErrorMessage`）。
fn sanitize_error(message: &str, secrets: &[&str]) -> String {
    let mut out = message.to_owned();
    for secret in secrets {
        if !secret.is_empty() && out.contains(secret) {
            out = out.replace(secret, "***");
        }
    }
    out
}

/// 保管庫の資格情報を、開いている対話ブラウザの入力欄へ直接入力する（Node `browserFillCredential`）。
///
/// 復号値（ユーザー名・パスワード）は `BrowserManager` へ**直接**渡し、LLM 応答・ログには一切
/// 含めない（§6.3.2）。Bot 許可（`bot_credential_access`）ゲート必須。
struct BrowserFillCredentialTool {
    name: ToolName,
    db: Db,
    crypto: Option<Arc<SystemCrypto>>,
    browser: Option<Arc<BrowserManager>>,
}

#[async_trait]
impl Tool for BrowserFillCredentialTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "保管庫のログイン情報を、いま開いているブラウザのページの入力欄へ直接打ち込む。\n\
                ・ユーザー名とパスワードはブラウザにだけ渡り、あなた（LLM）には返らない。\n\
                ・ユーザーが「ログインして」などはっきり頼んだ時だけ使う。\n\
                ・自動ログインの手順: (1)browserInteractiveOpen でログインページを開く (2)browserInteractiveStatus で入力欄の数値IDを調べる (3)この関数で username_selector / password_selector に数値IDを渡して入力 (4)browserInteractiveClick でログインボタンを押す。\n\
                ・セレクタには browserInteractiveStatus に出る数値ID（data-yuuka-id）かCSSセレクタを使う。\n\
                ・ユーザー名だけ入れたい時は username_selector だけ指定する。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "service_name": { "type": "string", "description": "入力するログイン情報のサービス名。listCredentialServices で確認できる。" },
                    "username_selector": { "type": "string", "description": "ユーザー名の入力欄を指すセレクタ（browserInteractiveStatus で調べた数値ID か CSSセレクタ）。省略するとユーザー名は入れない。" },
                    "password_selector": { "type": "string", "description": "パスワードの入力欄を指すセレクタ（browserInteractiveStatus で調べた数値ID か CSSセレクタ）。省略するとパスワードは入れない。" }
                },
                "required": ["service_name"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(service_name) = arg_str(&args, "service_name") else {
            return Ok(fail_payload("service_name を指定してください。"));
        };
        let username_selector = arg_str(&args, "username_selector");
        let password_selector = arg_str(&args, "password_selector");
        if username_selector.is_none() && password_selector.is_none() {
            return Ok(fail_payload(
                "入力先フィールドが指定されていません。先に browserInteractiveStatus を呼び出してページ内のユーザー名・パスワード入力欄の数値ID（data-yuuka-id）を確認し、username_selector / password_selector に指定して再度呼び出してください。",
            ));
        }

        let clean = service_name.trim().to_lowercase();
        let uid = ctx.user_id.as_str();

        // Bot 許可ゲート（Node `isCredentialGrantedToBot`）。
        let granted = CredentialAccessRepo::new(&self.db)
            .is_granted(ctx.bot_id.as_str(), uid, &clean)
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        if !granted {
            return Ok(fail_payload(format!(
                "このBotは認証情報「{clean}」の利用を許可されていません。統合管理ページ（Bot統合管理）で当該Botへ利用を許可してください。"
            )));
        }

        // 復号（username は平文列・password のみ暗号化）。crypto/ salt 無しは縮退。
        let Some(crypto) = &self.crypto else {
            return Ok(fail_payload("暗号化が未設定のため復号できません。"));
        };
        let repo = CredentialRepo::new(&self.db);
        let Some((username, _url, enc, iv, tag)) = repo
            .get_full(&scope_of(ctx), &service_name)
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?
        else {
            return Ok(fail_payload(format!(
                "サービス「{service_name}」の認証情報が見つかりません。listCredentialServices で登録済みサービス名を確認してください。"
            )));
        };
        let salt = repo
            .user_salt(uid)
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        let Some(salt) = salt else {
            return Ok(fail_payload("ユーザー情報が見つかりません。"));
        };
        let Ok(password) = crypto.decrypt_for_user(&salt, &enc, &iv, &tag) else {
            return Ok(fail_payload("パスワードの復号に失敗しました。"));
        };

        let Some(browser) = &self.browser else {
            return Ok(fail_payload("ブラウザが利用できません。"));
        };

        // 復号値をブラウザへ直接入力（LLM 応答・ログに含めない・§6.3.2）。失敗文言は秘匿値を除去。
        let secrets = [password.as_str(), username.as_str()];
        let mut filled: Vec<&str> = Vec::new();
        if let Some(sel) = &username_selector {
            if let Err(e) = browser.type_text(uid, sel, &username).await {
                return Ok(fail_payload(fill_error(
                    &filled,
                    &sanitize_error(&e, &secrets),
                )));
            }
            filled.push("ユーザー名");
        }
        if let Some(sel) = &password_selector {
            if let Err(e) = browser.type_text(uid, sel, &password).await {
                return Ok(fail_payload(fill_error(
                    &filled,
                    &sanitize_error(&e, &secrets),
                )));
            }
            filled.push("パスワード");
        }

        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "message": format!(
                "「{clean}」の{}を入力しました。続けて browserInteractiveClick でログインボタンを押下してください。",
                filled.join("と")
            ),
        })))
    }
}

/// 入力失敗時の文言（Node の分岐: 一部入力済みなら「〜の入力後にエラー」）。
fn fill_error(filled: &[&str], detail: &str) -> String {
    let prefix = if filled.is_empty() {
        "入力に失敗しました: ".to_owned()
    } else {
        format!("{}の入力後にエラーが発生しました: ", filled.join("と"))
    };
    format!("{prefix}{detail} browserInteractiveStatus で入力欄の数値IDを確認し直してください。")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::{params, Connection};
    use yuuka_core::{BotId, UserId};

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// Node migrations.ts の credentials 表と一致（暗号化列を含む・ツールは SELECT しない）。
    const CREDENTIALS_DDL: &str = "CREATE TABLE credentials (
        user_id TEXT NOT NULL,
        service_name TEXT NOT NULL,
        url TEXT,
        username TEXT NOT NULL,
        encrypted_password TEXT NOT NULL,
        iv TEXT NOT NULL,
        auth_tag TEXT NOT NULL,
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        PRIMARY KEY (user_id, service_name)
    );";

    fn seed_db_at() -> (Db, std::path::PathBuf) {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_credential_tools_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(CREDENTIALS_DDL).unwrap();
        }
        (Db::open(&path).unwrap(), path)
    }

    fn insert_at(
        path: &std::path::Path,
        user_id: &str,
        service_name: &str,
        username: &str,
        url: &str,
    ) {
        let conn = Connection::open(path).unwrap();
        conn.execute(
            "INSERT INTO credentials \
               (user_id, service_name, url, username, encrypted_password, iv, auth_tag, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 'ENC', 'IV', 'TAG', datetime('now','localtime'))",
            params![user_id, service_name, url, username],
        )
        .unwrap();
    }

    /// 利用許可を直挿しする（FK を切って users/bots seed を省く・list フィルタ検証用）。
    fn grant_at(path: &std::path::Path, bot_id: &str, owner_id: &str, service_name: &str) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO bot_credential_access (bot_id, owner_id, service_name) \
             VALUES (?1, ?2, ?3)",
            params![bot_id, owner_id, service_name],
        )
        .unwrap();
    }

    fn ctx(user: &str) -> ToolContext {
        ToolContext::new(BotId::system_default(), UserId::new(user))
    }

    fn find<'a>(tools: &'a [Arc<dyn Tool>], name: &str) -> &'a Arc<dyn Tool> {
        tools
            .iter()
            .find(|t| t.declaration().name.as_str() == name)
            .unwrap()
    }

    #[tokio::test]
    async fn list_and_delete_roundtrip() {
        let (db, path) = seed_db_at();
        insert_at(&path, "userA", "github", "alice", "https://gh.test");
        insert_at(&path, "userA", "aws", "alice2", "");
        // 応対 Bot（ctx=system_default）へ両 service を許可（未許可は list に出ない）。
        grant_at(&path, "system_default", "userA", "github");
        grant_at(&path, "system_default", "userA", "aws");
        let tools = tools(db, None, None).unwrap();

        // 宣言名は bare（Node system prompt と一致）。
        let names: Vec<String> = tools
            .iter()
            .map(|t| t.declaration().name.to_string())
            .collect();
        assert!(names.contains(&"listCredentialServices".to_owned()));
        assert!(names.contains(&"deleteCredential".to_owned()));
        assert!(!names.iter().any(|n| n.contains(':')), "native は bare 名");

        // list: service_name 昇順で 2 件、暗号化列は payload に出ない。
        let list = find(&tools, "listCredentialServices");
        let out = list.call(&ctx("userA"), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        let arr = out.payload["services"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["service_name"], "aws");
        assert_eq!(arr[1]["service_name"], "github");
        assert_eq!(arr[1]["username"], "alice");
        // 機密フェイルクローズ: 暗号化列・user_id は絶対に露出しない。
        assert!(arr[0]["encrypted_password"].is_null());
        assert!(arr[0]["iv"].is_null());
        assert!(arr[0]["auth_tag"].is_null());
        assert!(arr[0]["user_id"].is_null());

        // delete: 入力は正規化（"  GitHub " → "github"）して照合、成功。
        let delete = find(&tools, "deleteCredential");
        let out = delete
            .call(&ctx("userA"), json!({"service_name": "  GitHub "}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);

        // list で 1 件（aws）に減る。
        let out = list.call(&ctx("userA"), json!({})).await.unwrap();
        assert_eq!(out.payload["services"].as_array().unwrap().len(), 1);

        // 二重削除は not-found（success:false）。
        let out = delete
            .call(&ctx("userA"), json!({"service_name": "github"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
    }

    #[tokio::test]
    async fn delete_validates_and_list_empty_message() {
        let (db, _path) = seed_db_at();
        let tools = tools(db, None, None).unwrap();

        // service_name 欠落 → fail。
        let delete = find(&tools, "deleteCredential");
        let out = delete.call(&ctx("u"), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // 空一覧でも success:true・services:[]（形状 parity）。
        let list = find(&tools, "listCredentialServices");
        let out = list.call(&ctx("u"), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["services"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn scope_isolation_across_users() {
        let (db, path) = seed_db_at();
        insert_at(&path, "userA", "github", "alice", "");
        insert_at(&path, "userB", "aws", "bob", "");
        // 各 owner の service を応対 Bot（system_default）へ許可。
        grant_at(&path, "system_default", "userA", "github");
        grant_at(&path, "system_default", "userB", "aws");
        let tools = tools(db, None, None).unwrap();
        let list = find(&tools, "listCredentialServices");

        // 別ユーザーには自分の分だけ。
        let out = list.call(&ctx("userB"), json!({})).await.unwrap();
        let arr = out.payload["services"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["service_name"], "aws");

        // 別ユーザーは他人の認証情報を削除できない（分離キーを型で強制）。
        let delete = find(&tools, "deleteCredential");
        let out = delete
            .call(&ctx("userB"), json!({"service_name": "github"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
        // userA の github は残る。
        let out = list.call(&ctx("userA"), json!({})).await.unwrap();
        assert_eq!(out.payload["services"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn add_update_credential_with_crypto() {
        let (db, path) = seed_db_at();
        // user_salt が引けるよう users 行に salt を seed。add の応対 Bot 付与（writer=FK 有効）が
        // 通るよう bots(system_default) も seed する（FK: bot_id→bots, owner_id→users）。
        let salt = yuuka_crypto::generate_user_salt().unwrap();
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                 VALUES ('userA', 'userA', 'x', ?1)",
                rusqlite::params![salt],
            )
            .unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO bots (id, user_id, name) VALUES ('system_default', 'userA', 'sd')",
                [],
            )
            .unwrap();
        }
        let crypto =
            Arc::new(SystemCrypto::new(secrecy::SecretString::from("master-secret")).unwrap());
        let tool_set = tools(db, Some(crypto), None).unwrap();
        let add = find(&tool_set, "addCredential");
        let list = find(&tool_set, "listCredentialServices");
        let update = find(&tool_set, "updateCredential");

        // 必須欠落 → fail。
        let out = add
            .call(
                &ctx("userA"),
                json!({"service_name": "GitHub", "username": "me"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
        // 登録成功（暗号化して保存）。
        let out = add
            .call(
                &ctx("userA"),
                json!({"service_name": "GitHub", "username": "me", "password": "pw", "url": "https://github.com"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        // list に 1 件（サービス名は正規化=小文字・パスワードは出ない）。
        let out = list.call(&ctx("userA"), json!({})).await.unwrap();
        let services = out.payload["services"].as_array().unwrap();
        assert_eq!(services.len(), 1);
        assert_eq!(services[0]["service_name"], "github");

        // 更新: 無指定 → fail、ユーザー名変更 → success、不在 → fail。
        assert_eq!(
            update
                .call(&ctx("userA"), json!({"service_name": "github"}))
                .await
                .unwrap()
                .payload["success"],
            false
        );
        let out = update
            .call(
                &ctx("userA"),
                json!({"service_name": "github", "username": "me2", "password": "pw2"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert!(out.payload["message"]
            .as_str()
            .unwrap()
            .contains("ユーザー名"));
        assert_eq!(
            update
                .call(
                    &ctx("userA"),
                    json!({"service_name": "gitlab", "username": "x"})
                )
                .await
                .unwrap()
                .payload["success"],
            false
        );

        // url="" は URL 削除（Node updateCredential パリティ・空文字＝削除の意図）。
        let out = update
            .call(&ctx("userA"), json!({"service_name": "github", "url": ""}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert!(out.payload["message"].as_str().unwrap().contains("URL"));
        let out = list.call(&ctx("userA"), json!({})).await.unwrap();
        let services = out.payload["services"].as_array().unwrap();
        assert_eq!(services[0]["service_name"], "github");
        assert!(services[0]["url"].is_null(), "url='' で URL が削除される");

        // 空白のみパスワードも受理される（trim しない・Node asOptionalPassword）。arg_str なら
        // 空扱いで弾かれていた退行を防ぐ。
        let out = add
            .call(
                &ctx("userA"),
                json!({"service_name": "spaces", "username": "u", "password": "   "}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);

        // crypto 未設定なら add は縮退（保存不可）。
        let (db2, _p2) = seed_db_at();
        let tools_no_crypto = tools(db2, None, None).unwrap();
        let add2 = find(&tools_no_crypto, "addCredential");
        let out = add2
            .call(
                &ctx("userA"),
                json!({"service_name": "X", "username": "u", "password": "p"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
    }

    #[test]
    fn sanitize_error_masks_secrets() {
        let msg = "要素 hunter2 の入力に失敗: alice";
        let out = sanitize_error(msg, &["hunter2", "alice"]);
        assert!(!out.contains("hunter2"));
        assert!(!out.contains("alice"));
        assert!(out.contains("***"));
    }

    #[test]
    fn fill_error_prefixes_by_progress() {
        assert!(fill_error(&[], "boom").starts_with("入力に失敗しました: "));
        assert!(fill_error(&["ユーザー名"], "boom")
            .starts_with("ユーザー名の入力後にエラーが発生しました: "));
    }

    #[tokio::test]
    async fn browser_fill_requires_a_selector() {
        let (db, _p) = seed_db_at();
        // browser 無し（None）でも、セレクタ検証は browser/DB より前なので到達しない。
        let tools = tools(db, None, None).unwrap();
        let fill = find(&tools, "browserFillCredential");
        // service_name 無し → fail。
        let out = fill.call(&ctx("userA"), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], false);
        // service_name あり・セレクタ無し → fail（入力先未指定の案内）。
        let out = fill
            .call(&ctx("userA"), json!({"service_name": "github"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
        assert!(out.payload["message"]
            .as_str()
            .unwrap()
            .contains("入力先フィールドが指定されていません"));
    }

    #[tokio::test]
    async fn browser_fill_blocks_ungranted_bot() {
        let (db, path) = seed_db_at();
        insert_at(&path, "userA", "github", "alice", "https://gh.test");
        // grant しない → 未許可 Bot は利用不可（decrypt/browser へ到達しない）。
        let tools = tools(db, None, None).unwrap();
        let fill = find(&tools, "browserFillCredential");
        let out = fill
            .call(
                &ctx("userA"),
                json!({"service_name": "github", "username_selector": "2"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
        assert!(out.payload["message"]
            .as_str()
            .unwrap()
            .contains("利用を許可されていません"));
    }
}
