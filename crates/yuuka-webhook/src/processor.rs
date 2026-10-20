//! Webhook 受信の実処理（Node `webhookProcessor.processIncomingWebhook` パリティ・§3.13）。
//!
//! 公開ルート `POST /hook/{token}` が即時 200 を返したあと、[`RealWebhookProcessor`] が非同期に
//! HMAC 署名検証 → リプレイ検出 → 監査ログ → キーワードフィルタ → 補助 LLM による解釈 → Discord 通知 →
//! ToDo / リマインド化 → 受信履歴の記録、の順に処理する。Discord 配信と補助 LLM は本クレートの外にあるため
//! [`WebhookNotifier`] / [`AuxTextGenerator`] ポートで受け取り、supervisor が実装を注入する。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde::de::{Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;
use yuuka_core::{BotId, UserId, UserScope};
use yuuka_crypto::SystemCrypto;
use yuuka_todo::dto::NewTodo;
use yuuka_todo::repo::TodoRepo;
use yuuka_web::{has_bot_access, Db};

use crate::repo::{self, WebhookEndpoint};
use crate::routes::WebhookProcessor;

/// 同一署名の再送を拒否する期間（Node `REPLAY_WINDOW_MS`）。
const REPLAY_WINDOW: Duration = Duration::from_secs(5 * 60);
/// 通知 Embed の色（Node: 🪝 + ブルー §3.0.2）。
const NOTICE_COLOR: u32 = 0x5865f2;
/// 通知 Embed のフッタ。
const NOTICE_FOOTER: &str = "外部Webhook通知";

/// Webhook 通知の送信先（エンドポイント設定の `notify_target_type`/`notify_target_id`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookTarget {
    /// 本人の DM。
    Dm,
    /// チャンネル（見つからない・本人が閲覧できない場合は配信側で DM へフォールバック）。
    Channel(String),
}

/// Webhook 通知 1 通（Node の Embed: タイトル `🪝 {name}`・本文・色・フッタ）。
#[derive(Debug, Clone)]
pub struct WebhookNotice {
    pub title: String,
    pub description: String,
    /// 0xRRGGBB。
    pub color: u32,
    pub footer: String,
    pub target: WebhookTarget,
}

/// Webhook 通知の配信ポート（Node `sendToUser`）。送信成功なら `true`。
#[async_trait]
pub trait WebhookNotifier: Send + Sync {
    async fn notify(&self, user_id: &str, bot_id: &str, notice: WebhookNotice) -> bool;
}

pub use yuuka_core::aux_text::AuxTextGenerator;

/// 受信履歴（`webhook_deliveries.status`）に残す処理結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryStatus {
    Notified,
    Filtered,
    Failed,
}

impl DeliveryStatus {
    /// DB に保存する値。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Notified => "notified",
            Self::Filtered => "filtered",
            Self::Failed => "failed",
        }
    }
}

/// 1 回の受信処理の結果（Node `WebhookProcessResult`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookOutcome {
    pub status: DeliveryStatus,
    pub detail: String,
}

/// 受信の実処理（[`WebhookProcessor`] 実装）。
pub struct RealWebhookProcessor {
    db: Db,
    /// シークレットの復号に使う（未設定なら署名を検証できないため全件拒否）。
    crypto: Option<Arc<SystemCrypto>>,
    notifier: Arc<dyn WebhookNotifier>,
    aux: Arc<dyn AuxTextGenerator>,
    /// 直近に受理した署名（`(endpoint_id, 署名 hex)` → 期限）。プロセス内のみで保持する（Node と同じ）。
    seen_signatures: Mutex<HashMap<(i64, String), Instant>>,
}

impl RealWebhookProcessor {
    /// 依存を注入して構築する。
    #[must_use]
    pub fn new(
        db: Db,
        crypto: Option<Arc<SystemCrypto>>,
        notifier: Arc<dyn WebhookNotifier>,
        aux: Arc<dyn AuxTextGenerator>,
    ) -> Self {
        Self {
            db,
            crypto,
            notifier,
            aux,
            seen_signatures: Mutex::new(HashMap::new()),
        }
    }

    /// 受信 1 件を処理し、受信履歴へ記録した結果を返す（[`WebhookProcessor::process`] の本体）。
    pub async fn handle(
        &self,
        endpoint: &WebhookEndpoint,
        raw_body: &[u8],
        signature: Option<&str>,
    ) -> WebhookOutcome {
        let payload_text = String::from_utf8_lossy(raw_body).into_owned();
        let user_id = endpoint.user_id.as_str();

        // 1. HMAC 署名検証（§3.13.4）。失敗は監査ログを膨らませないよう受信履歴にだけ残す。
        if !self.verify_signature(endpoint, raw_body, signature) {
            let detail = "HMAC署名の検証に失敗しました";
            tracing::warn!(endpoint_id = endpoint.id, "Webhook: {detail}");
            return self
                .finish(endpoint, &payload_text, DeliveryStatus::Failed, detail)
                .await;
        }

        // 1b. リプレイ検出（同一署名の再送を一定時間拒否する §3.13.4）。
        let sig_hex = signature_hex(signature.unwrap_or_default()).to_ascii_lowercase();
        if !sig_hex.is_empty() && self.is_replayed(endpoint.id, &sig_hex) {
            let detail = "リプレイ（同一署名の再送）を検出したため拒否しました";
            tracing::warn!(endpoint_id = endpoint.id, "Webhook: {detail}");
            return self
                .finish(endpoint, &payload_text, DeliveryStatus::Failed, detail)
                .await;
        }

        // 検証を通過した受信のみ監査ログに記録する。
        yuuka_auth::audit::add_audit_log(
            &self.db,
            user_id,
            "webhook.received",
            Some(&endpoint.name),
            None,
        )
        .await;

        // 2. キーワードフィルタ（合致しないペイロードは通知しない §3.13.3）。
        if let Some(keyword) = endpoint.filter_keyword.as_deref().filter(|k| !k.is_empty()) {
            if !payload_text.contains(keyword) {
                let detail = format!("フィルタ「{keyword}」に合致しないため通知をスキップしました");
                return self
                    .finish(endpoint, &payload_text, DeliveryStatus::Filtered, &detail)
                    .await;
            }
        }

        // 3. 補助 LLM によるペイロード解釈（§3.13.2。キー未設定・失敗時はフォールバック整形）。
        let summary = self
            .aux
            .generate_aux_text(user_id, &interpret_prompt(endpoint, &payload_text))
            .await
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        let interpreted = summary.is_some();
        let notify_text = summary.unwrap_or_else(|| fallback_summary(&payload_text));

        // 通知・ToDo・リマインドはエンドポイントの帰属 Bot（作成時に選択中だった Bot）に紐付ける。
        // system_default は個人データを持たない案内役（#138）なので帰属先にしない。共有が外れたなど
        // 本人がもう使えない Bot なら、見えないデータを作らず失敗として記録する。
        let bot_id = match self.usable_bot(endpoint).await {
            Ok(bot_id) => bot_id,
            Err(detail) => {
                tracing::warn!(endpoint_id = endpoint.id, "Webhook: {detail}");
                return self
                    .finish(endpoint, &payload_text, DeliveryStatus::Failed, detail)
                    .await;
            }
        };

        // 4. Discord へ通知する。
        let notice = WebhookNotice {
            title: format!("🪝 {}", endpoint.name),
            description: truncate_chars(&notify_text, 4000).to_owned(),
            color: NOTICE_COLOR,
            footer: NOTICE_FOOTER.to_owned(),
            target: notify_target(endpoint),
        };
        if !self.notifier.notify(user_id, &bot_id, notice).await {
            let detail = "Discord通知の送信に失敗しました";
            tracing::warn!(endpoint_id = endpoint.id, "Webhook: {detail}");
            return self
                .finish(endpoint, &payload_text, DeliveryStatus::Failed, detail)
                .await;
        }

        let first_line = notify_text.split('\n').next().unwrap_or_default();
        let mut conversions: Vec<&str> = Vec::new();

        // 5. ToDo への変換（§3.13.3）。
        if endpoint.create_todo {
            let scope = UserScope::new(UserId::new(user_id), BotId::new(bot_id.clone()));
            let input = NewTodo {
                title: format!("[Webhook] {}", truncate_chars(first_line, 80)),
                description: Some(format!(
                    "Webhook「{}」からの自動登録\n\n{}",
                    endpoint.name,
                    truncate_chars(&notify_text, 500)
                )),
                tags: vec!["webhook".to_owned(), endpoint.name.clone()],
                ..NewTodo::default()
            };
            match TodoRepo::new(&self.db).add(&scope, input).await {
                Ok(_) => conversions.push("ToDo登録"),
                Err(e) => {
                    tracing::error!(endpoint_id = endpoint.id, error = %e, "Webhook: ToDo変換に失敗しました");
                }
            }
        }

        // 6. リマインドへの変換（1 時間後の単発リマインド §3.13.3）。
        if endpoint.create_reminder {
            let message = format!(
                "[Webhook: {}] {}",
                endpoint.name,
                truncate_chars(first_line, 100)
            );
            match repo::add_webhook_reminder(&self.db, endpoint, &bot_id, &message).await {
                Ok(_) => conversions.push("リマインド設定(1時間後)"),
                Err(e) => {
                    tracing::error!(endpoint_id = endpoint.id, error = %e, "Webhook: リマインド変換に失敗しました");
                }
            }
        }

        let mut detail = "通知済み".to_owned();
        if !conversions.is_empty() {
            detail.push_str(&format!("（{}）", conversions.join("・")));
        }
        if !interpreted {
            detail.push_str("（LLM解釈なし・フォールバック整形）");
        }
        tracing::info!(endpoint_id = endpoint.id, "Webhook: {detail}");
        self.finish(endpoint, &payload_text, DeliveryStatus::Notified, &detail)
            .await
    }

    /// エンドポイントの帰属 Bot が本人にまだ使えるか（所有・共有）を確かめて返す。
    async fn usable_bot(&self, endpoint: &WebhookEndpoint) -> Result<String, &'static str> {
        let bot_id = endpoint
            .bot_id
            .as_deref()
            .filter(|b| !b.is_empty() && *b != BotId::SYSTEM_DEFAULT)
            .ok_or("通知に使う Bot が設定されていません（Webhook を作り直してください）")?;
        match has_bot_access(&self.db, &endpoint.user_id, bot_id).await {
            Ok(true) => Ok(bot_id.to_owned()),
            Ok(false) => Err("通知に使う Bot が見つからないか、利用できなくなりました"),
            Err(e) => {
                tracing::error!(endpoint_id = endpoint.id, error = %e, "Webhook: Bot の確認に失敗");
                Err("通知に使う Bot を確認できませんでした")
            }
        }
    }

    /// 受信履歴へ記録して結果を返す（記録の失敗は処理結果を変えない）。
    async fn finish(
        &self,
        endpoint: &WebhookEndpoint,
        payload_text: &str,
        status: DeliveryStatus,
        detail: &str,
    ) -> WebhookOutcome {
        if let Err(e) = repo::add_delivery(
            &self.db,
            endpoint.id,
            &endpoint.user_id,
            payload_text,
            status.as_str(),
            Some(detail),
        )
        .await
        {
            tracing::error!(endpoint_id = endpoint.id, error = %e, "Webhook: 受信履歴の記録に失敗しました");
        }
        WebhookOutcome {
            status,
            detail: detail.to_owned(),
        }
    }

    /// 署名を検証する。シークレット未設定のエンドポイントは認証できないため受理しない（既定拒否）。
    fn verify_signature(
        &self,
        endpoint: &WebhookEndpoint,
        raw_body: &[u8],
        signature: Option<&str>,
    ) -> bool {
        let non_empty = |v: &Option<String>| v.clone().filter(|s| !s.is_empty());
        let (Some(enc), Some(iv), Some(tag)) = (
            non_empty(&endpoint.secret_encrypted),
            non_empty(&endpoint.secret_iv),
            non_empty(&endpoint.secret_tag),
        ) else {
            return false;
        };
        let Some(signature) = signature.filter(|s| !s.is_empty()) else {
            return false;
        };
        let Some(crypto) = self.crypto.as_deref() else {
            tracing::error!(
                endpoint_id = endpoint.id,
                "Webhook: 暗号鍵が未設定のためシークレットを復号できません"
            );
            return false;
        };
        let secret = match crypto.decrypt_text(&enc, &iv, &tag) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(endpoint_id = endpoint.id, error = %e, "Webhook: シークレットの復号に失敗しました");
                return false;
            }
        };
        verify_hmac_sha256_hex(&secret, raw_body, signature)
    }

    /// 同一署名が直近に受理済みなら `true`（リプレイ）。未受理なら記録して `false`。
    fn is_replayed(&self, endpoint_id: i64, sig_hex: &str) -> bool {
        let now = Instant::now();
        let mut seen = self
            .seen_signatures
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // 肥大防止に期限切れを掃除（残るのは期限内のものだけ）。
        seen.retain(|_, expires| *expires > now);
        let key = (endpoint_id, sig_hex.to_owned());
        if seen.contains_key(&key) {
            return true;
        }
        seen.insert(key, now + REPLAY_WINDOW);
        false
    }
}

#[async_trait]
impl WebhookProcessor for RealWebhookProcessor {
    async fn process(
        &self,
        endpoint: WebhookEndpoint,
        raw_body: Vec<u8>,
        signature: Option<String>,
    ) {
        self.handle(&endpoint, &raw_body, signature.as_deref())
            .await;
    }
}

/// HMAC-SHA256 署名を検証する（Node `verifyHmacSha256Hex`）。
///
/// `"sha256=<hex>"`（GitHub 互換）と素の hex の両方を受け付ける。hex として読めない・長さが違う場合は
/// `false`。比較は [`Mac::verify_slice`] の定数時間比較。
#[must_use]
pub fn verify_hmac_sha256_hex(secret: &str, raw_body: &[u8], signature_header: &str) -> bool {
    let Ok(provided) = hex::decode(signature_hex(signature_header)) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(raw_body);
    mac.verify_slice(&provided).is_ok()
}

/// 署名ヘッダから hex 部分を取り出す（先頭の `sha256=` は大小無視で除き、前後の空白を落とす）。
fn signature_hex(header: &str) -> &str {
    let rest = match (header.get(..7), header.get(7..)) {
        (Some(prefix), Some(rest)) if prefix.eq_ignore_ascii_case("sha256=") => rest,
        _ => header,
    };
    rest.trim()
}

/// エンドポイント設定から通知先を決める（`channel` かつ id ありのときだけチャンネル宛）。
fn notify_target(endpoint: &WebhookEndpoint) -> WebhookTarget {
    match endpoint.notify_target_id.as_deref() {
        Some(id) if endpoint.notify_target_type == "channel" && !id.is_empty() => {
            WebhookTarget::Channel(id.to_owned())
        }
        _ => WebhookTarget::Dm,
    }
}

/// 補助 LLM へ渡す解釈指示（Node と同文）。
fn interpret_prompt(endpoint: &WebhookEndpoint, payload_text: &str) -> String {
    let template_instruction = endpoint
        .template
        .as_deref()
        .filter(|t| !t.is_empty())
        .map_or_else(String::new, |t| {
            format!("\n\n通知文は次のテンプレート・指示に従って整形してください:\n{t}")
        });
    format!(
        "外部サービス「{name}」から以下のWebhookペイロードを受信しました。\
         内容を解釈し、Discord通知用の簡潔で分かりやすい日本語メッセージ（500文字以内、重要な情報を優先）を生成してください。\
         通知文のみを出力し、前置きや説明は不要です。{template_instruction}\n\n\
         ペイロード:\n{payload}",
        name = endpoint.name,
        payload = truncate_chars(payload_text, 8000),
    )
}

/// LLM を使わない通知文（Node `buildFallbackSummary`）。JSON のオブジェクト/配列なら先頭 12 項目を
/// `- **key**: value` で列挙し、それ以外は本文の先頭 800 文字。
fn fallback_summary(payload_text: &str) -> String {
    let entries: Option<Vec<(String, Value)>> =
        if let Ok(OrderedObject(entries)) = serde_json::from_str(payload_text) {
            Some(entries)
        } else if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(payload_text) {
            Some(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| (i.to_string(), v))
                    .collect(),
            )
        } else {
            None
        };
    let Some(entries) = entries else {
        return truncate_chars(payload_text, 800).to_owned();
    };
    let lines: Vec<String> = entries
        .iter()
        .take(12)
        .map(|(key, value)| {
            let text = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            format!("- **{key}**: {}", truncate_chars(&text, 120))
        })
        .collect();
    if lines.is_empty() {
        "（空のペイロード）".to_owned()
    } else {
        lines.join("\n")
    }
}

/// 先頭 `max` 文字（Unicode スカラー値単位）までを返す。
fn truncate_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => s.get(..i).unwrap_or(s),
        None => s,
    }
}

/// トップレベルの JSON オブジェクトをキーの出現順のまま読む（`serde_json::Map` はキー順に並べ替えるため、
/// Node `Object.entries` と同じ順で列挙するのに使う）。オブジェクト以外はエラー。
struct OrderedObject(Vec<(String, Value)>);

impl<'de> Deserialize<'de> for OrderedObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntriesVisitor;

        impl<'de> Visitor<'de> for EntriesVisitor {
            type Value = OrderedObject;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry::<String, Value>()? {
                    entries.push(entry);
                }
                Ok(OrderedObject(entries))
            }
        }

        deserializer.deserialize_map(EntriesVisitor)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use hmac::{Hmac, Mac};
    use secrecy::SecretString;
    use sha2::Sha256;
    use yuuka_crypto::SystemCrypto;
    use yuuka_web::Db;

    use super::*;
    use crate::repo::{EncryptedSecret, EndpointCreate};

    const SECRET: &str = "abcdefghij0123456789";

    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    #[test]
    fn hmac_accepts_prefixed_and_bare_hex() {
        let body = br#"{"event":"push"}"#;
        let sig = sign(SECRET, body);
        assert!(verify_hmac_sha256_hex(
            SECRET,
            body,
            &format!("sha256={sig}")
        ));
        assert!(verify_hmac_sha256_hex(
            SECRET,
            body,
            &format!("SHA256={sig}")
        ));
        assert!(verify_hmac_sha256_hex(SECRET, body, &sig));
        assert!(verify_hmac_sha256_hex(
            SECRET,
            body,
            &sig.to_ascii_uppercase()
        ));
    }

    #[test]
    fn hmac_rejects_wrong_or_malformed_signatures() {
        let body = br#"{"event":"push"}"#;
        let sig = sign(SECRET, body);
        // シークレット違い・本文改ざん。
        assert!(!verify_hmac_sha256_hex("other-secret-0123456", body, &sig));
        assert!(!verify_hmac_sha256_hex(SECRET, b"{}", &sig));
        // 空・hex でない・長さ違い（切り詰め）・sha1 形式。
        assert!(!verify_hmac_sha256_hex(SECRET, body, ""));
        assert!(!verify_hmac_sha256_hex(SECRET, body, "sha256=zz"));
        assert!(!verify_hmac_sha256_hex(SECRET, body, &sig[..32]));
        assert!(!verify_hmac_sha256_hex(
            SECRET,
            body,
            &format!("sha1={sig}")
        ));
    }

    #[test]
    fn fallback_lists_top_level_keys_in_order() {
        let text = fallback_summary(
            r#"{"zeta":"z","alpha":1,"nested":{"k":[1,2]},"none":null,"flag":true}"#,
        );
        assert_eq!(
            text,
            "- **zeta**: z\n- **alpha**: 1\n- **nested**: {\"k\":[1,2]}\n- **none**: null\n- **flag**: true"
        );
    }

    #[test]
    fn fallback_limits_entries_and_value_length() {
        let fields: Vec<String> = (0..15).map(|i| format!("\"k{i}\":{i}")).collect();
        let text = fallback_summary(&format!("{{{}}}", fields.join(",")));
        assert_eq!(text.lines().count(), 12);
        assert!(text.ends_with("- **k11**: 11"));

        let long = "あ".repeat(200);
        let text = fallback_summary(&format!(r#"{{"msg":"{long}"}}"#));
        assert_eq!(text, format!("- **msg**: {}", "あ".repeat(120)));
    }

    #[test]
    fn fallback_handles_arrays_empty_and_non_json() {
        assert_eq!(fallback_summary(r#"["a","b"]"#), "- **0**: a\n- **1**: b");
        assert_eq!(fallback_summary("{}"), "（空のペイロード）");
        let plain = "x".repeat(900);
        assert_eq!(fallback_summary(&plain), "x".repeat(800));
        assert_eq!(fallback_summary("42"), "42");
    }

    // ─── 受信処理（DB + 偽ポート） ────────────────────────────────────────────

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_webhook_processor_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        drop(rusqlite::Connection::open(&path).expect("create empty"));
        let db = Db::open(&path).expect("open");
        let conn = rusqlite::Connection::open(&path).expect("seed conn");
        conn.execute(
            "INSERT OR IGNORE INTO users (discord_id, username, salt) \
             VALUES ('u', 'u', '')",
            [],
        )
        .expect("seed user");
        conn.execute(
            "INSERT OR IGNORE INTO bots (id, user_id, name) VALUES ('test_bot', 'u', 'Test Bot')",
            [],
        )
        .expect("seed bot");
        db
    }

    #[derive(Default)]
    struct FakeNotifier {
        fail: bool,
        sent: Mutex<Vec<(String, String, WebhookNotice)>>,
    }

    #[async_trait]
    impl WebhookNotifier for FakeNotifier {
        async fn notify(&self, user_id: &str, bot_id: &str, notice: WebhookNotice) -> bool {
            self.sent
                .lock()
                .unwrap()
                .push((user_id.to_owned(), bot_id.to_owned(), notice));
            !self.fail
        }
    }

    #[derive(Default)]
    struct FakeAux {
        reply: Option<String>,
        prompts: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AuxTextGenerator for FakeAux {
        async fn generate_aux_text(&self, _user_id: &str, prompt: &str) -> Option<String> {
            self.prompts.lock().unwrap().push(prompt.to_owned());
            self.reply.clone()
        }
    }

    struct Fixture {
        db: Db,
        crypto: Arc<SystemCrypto>,
        notifier: Arc<FakeNotifier>,
        aux: Arc<FakeAux>,
        processor: RealWebhookProcessor,
    }

    fn fixture(notifier: FakeNotifier, aux: FakeAux) -> Fixture {
        let db = seed_db();
        let crypto = Arc::new(SystemCrypto::new(SecretString::from("test-secret-xyz")).unwrap());
        let notifier = Arc::new(notifier);
        let aux = Arc::new(aux);
        let processor = RealWebhookProcessor::new(
            db.clone(),
            Some(crypto.clone()),
            notifier.clone(),
            aux.clone(),
        );
        Fixture {
            db,
            crypto,
            notifier,
            aux,
            processor,
        }
    }

    async fn endpoint(fx: &Fixture, input: EndpointCreate) -> WebhookEndpoint {
        let enc = fx.crypto.encrypt_text(SECRET).unwrap();
        let input = EndpointCreate {
            secret: Some(EncryptedSecret {
                encrypted: enc.encrypted,
                iv: enc.iv,
                tag: enc.auth_tag,
            }),
            ..input
        };
        repo::create_endpoint(&fx.db, "u", "test_bot", input)
            .await
            .unwrap()
    }

    fn named(name: &str) -> EndpointCreate {
        EndpointCreate {
            name: name.to_owned(),
            notify_target_type: "dm".to_owned(),
            ..EndpointCreate::default()
        }
    }

    async fn count(db: &Db, sql: &'static str) -> i64 {
        db.read
            .read(move |conn| {
                conn.query_row(sql, [], |r| r.get(0))
                    .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn rejects_bad_signature_and_records_failure() {
        let fx = fixture(FakeNotifier::default(), FakeAux::default());
        let ep = endpoint(&fx, named("CI")).await;
        let body = br#"{"event":"push"}"#;

        let out = fx.processor.handle(&ep, body, Some("sha256=00")).await;
        assert_eq!(out.status, DeliveryStatus::Failed);
        assert_eq!(out.detail, "HMAC署名の検証に失敗しました");
        let out = fx.processor.handle(&ep, body, None).await;
        assert_eq!(out.status, DeliveryStatus::Failed);

        assert!(fx.notifier.sent.lock().unwrap().is_empty());
        let deliveries = repo::list_deliveries(&fx.db, "u", "test_bot", Some(ep.id), 50)
            .await
            .unwrap();
        assert_eq!(deliveries.len(), 2);
        assert_eq!(deliveries[0].status, "failed");
        assert_eq!(
            count(
                &fx.db,
                "SELECT COUNT(*) FROM audit_logs WHERE action = 'webhook.received'"
            )
            .await,
            0
        );
    }

    /// 帰属 Bot は作成時の選択中 Bot。id 順で先に来る別の所有 Bot があっても、そちらへは入れない。
    #[tokio::test]
    async fn uses_endpoint_bot_even_if_user_owns_other_bots() {
        let fx = fixture(FakeNotifier::default(), FakeAux::default());
        fx.db
            .writer
            .execute(|conn| {
                conn.execute(
                    "INSERT INTO bots (id, user_id, name) VALUES ('a_first', 'u', 'First Bot')",
                    [],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        let ep = endpoint(
            &fx,
            EndpointCreate {
                create_todo: true,
                ..named("CI")
            },
        )
        .await;
        assert_eq!(ep.bot_id.as_deref(), Some("test_bot"));
        let body = br#"{"status":"success"}"#;

        let out = fx
            .processor
            .handle(&ep, body, Some(&sign(SECRET, body)))
            .await;
        assert_eq!(out.status, DeliveryStatus::Notified);
        assert_eq!(fx.notifier.sent.lock().unwrap()[0].1, "test_bot");
        assert_eq!(
            count(
                &fx.db,
                "SELECT COUNT(*) FROM todos WHERE user_id = 'u' AND bot_id = 'test_bot'"
            )
            .await,
            1
        );
    }

    /// 帰属 Bot が無い旧行・使えなくなった Bot では、見えないデータを作らず失敗として記録する。
    #[tokio::test]
    async fn fails_without_usable_bot() {
        let fx = fixture(FakeNotifier::default(), FakeAux::default());
        let mut ep = endpoint(
            &fx,
            EndpointCreate {
                create_todo: true,
                ..named("CI")
            },
        )
        .await;
        let body = br#"{"n":1}"#;

        ep.bot_id = None;
        let out = fx
            .processor
            .handle(&ep, body, Some(&sign(SECRET, body)))
            .await;
        assert_eq!(out.status, DeliveryStatus::Failed);
        assert_eq!(
            out.detail,
            "通知に使う Bot が設定されていません（Webhook を作り直してください）"
        );

        ep.bot_id = Some("someone_elses_bot".to_owned());
        let body = br#"{"n":2}"#;
        let out = fx
            .processor
            .handle(&ep, body, Some(&sign(SECRET, body)))
            .await;
        assert_eq!(out.status, DeliveryStatus::Failed);
        assert_eq!(
            out.detail,
            "通知に使う Bot が見つからないか、利用できなくなりました"
        );

        assert!(fx.notifier.sent.lock().unwrap().is_empty());
        assert_eq!(count(&fx.db, "SELECT COUNT(*) FROM todos").await, 0);
        let deliveries = repo::list_deliveries(&fx.db, "u", "test_bot", Some(ep.id), 50)
            .await
            .unwrap();
        assert_eq!(deliveries.len(), 2);
    }

    #[tokio::test]
    async fn rejects_endpoint_without_secret() {
        let fx = fixture(FakeNotifier::default(), FakeAux::default());
        let ep = repo::create_endpoint(&fx.db, "u", "test_bot", named("legacy"))
            .await
            .unwrap();
        let body = b"{}";
        let out = fx
            .processor
            .handle(&ep, body, Some(&sign(SECRET, body)))
            .await;
        assert_eq!(out.status, DeliveryStatus::Failed);
    }

    #[tokio::test]
    async fn notifies_and_converts_to_todo_and_reminder() {
        let fx = fixture(
            FakeNotifier::default(),
            FakeAux {
                reply: Some("  ビルドが成功しました\n詳細: main  ".to_owned()),
                ..FakeAux::default()
            },
        );
        let ep = endpoint(
            &fx,
            EndpointCreate {
                notify_target_type: "channel".to_owned(),
                notify_target_id: Some("123".to_owned()),
                template: Some("絵文字を付ける".to_owned()),
                create_todo: true,
                create_reminder: true,
                ..named("CI")
            },
        )
        .await;
        let body = br#"{"status":"success"}"#;
        let sig = sign(SECRET, body);

        let out = fx
            .processor
            .handle(&ep, body, Some(&format!("sha256={sig}")))
            .await;
        assert_eq!(out.status, DeliveryStatus::Notified);
        assert_eq!(out.detail, "通知済み（ToDo登録・リマインド設定(1時間後)）");

        // 解釈指示にエンドポイント名・テンプレート・ペイロードが入る。
        let prompts = fx.aux.prompts.lock().unwrap().clone();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("外部サービス「CI」"));
        assert!(prompts[0].contains("従って整形してください:\n絵文字を付ける"));
        assert!(prompts[0].ends_with("ペイロード:\n{\"status\":\"success\"}"));

        let sent = fx.notifier.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        let (user, bot_id, notice) = &sent[0];
        assert_eq!(user, "u");
        assert_eq!(bot_id, "test_bot");
        assert_eq!(notice.title, "🪝 CI");
        assert_eq!(notice.description, "ビルドが成功しました\n詳細: main");
        assert_eq!(notice.footer, "外部Webhook通知");
        assert_eq!(notice.target, WebhookTarget::Channel("123".to_owned()));

        let (title, tags, bot): (String, String, String) = fx
            .db
            .read
            .read(|conn| {
                conn.query_row(
                    "SELECT title, tags, bot_id FROM todos WHERE user_id = 'u'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(title, "[Webhook] ビルドが成功しました");
        assert_eq!(tags, r#"["webhook","CI"]"#);
        let (explicit, pending): (String, i64) = fx.db.read.read(|c| {
            c.query_row("SELECT s.explicit_tags, s.pending FROM todo_tagging s JOIN todos t ON t.id=s.todo_id WHERE t.user_id='u' AND t.bot_id='test_bot'", [], |r| Ok((r.get(0)?, r.get(1)?))).map_err(yuuka_db::map_sqlite)
        }).await.unwrap();
        assert_eq!(explicit, tags);
        assert_eq!(pending, 1);

        assert_eq!(bot, "test_bot");

        let (message, target_type, target_id, source, source_id, in_future): (
            String,
            String,
            Option<String>,
            String,
            String,
            bool,
        ) = fx
            .db
            .read
            .read(|conn| {
                conn.query_row(
                    "SELECT message, target_type, target_id, source, source_id, \
                     trigger_at > datetime('now', 'localtime', '+59 minutes') \
                     FROM reminders WHERE user_id = 'u' AND status = 'pending'",
                    [],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                        ))
                    },
                )
                .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(message, "[Webhook: CI] ビルドが成功しました");
        assert_eq!(target_type, "channel");
        assert_eq!(target_id.as_deref(), Some("123"));
        assert_eq!(source, "webhook");
        assert_eq!(source_id, ep.id.to_string());
        assert!(in_future);

        assert_eq!(
            count(
                &fx.db,
                "SELECT COUNT(*) FROM audit_logs WHERE action = 'webhook.received'"
            )
            .await,
            1
        );
        let deliveries = repo::list_deliveries(&fx.db, "u", "test_bot", Some(ep.id), 50)
            .await
            .unwrap();
        assert_eq!(deliveries[0].status, "notified");
        assert_eq!(deliveries[0].payload, r#"{"status":"success"}"#);

        // 同じ署名の再送はリプレイとして拒否する（通知は増えない）。
        let out = fx.processor.handle(&ep, body, Some(&sig)).await;
        assert_eq!(out.status, DeliveryStatus::Failed);
        assert_eq!(
            out.detail,
            "リプレイ（同一署名の再送）を検出したため拒否しました"
        );
        assert_eq!(fx.notifier.sent.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn filters_by_keyword() {
        let fx = fixture(FakeNotifier::default(), FakeAux::default());
        let ep = endpoint(
            &fx,
            EndpointCreate {
                filter_keyword: Some("failure".to_owned()),
                ..named("CI")
            },
        )
        .await;
        let body = br#"{"status":"success"}"#;
        let out = fx
            .processor
            .handle(&ep, body, Some(&sign(SECRET, body)))
            .await;
        assert_eq!(out.status, DeliveryStatus::Filtered);
        assert_eq!(
            out.detail,
            "フィルタ「failure」に合致しないため通知をスキップしました"
        );
        assert!(fx.notifier.sent.lock().unwrap().is_empty());
        assert!(fx.aux.prompts.lock().unwrap().is_empty());

        let body = br#"{"status":"failure"}"#;
        let out = fx
            .processor
            .handle(&ep, body, Some(&sign(SECRET, body)))
            .await;
        assert_eq!(out.status, DeliveryStatus::Notified);
    }

    #[tokio::test]
    async fn falls_back_without_llm_and_targets_dm() {
        let fx = fixture(FakeNotifier::default(), FakeAux::default());
        let ep = endpoint(&fx, named("Shop")).await;
        let body = br#"{"order":"A-1","total":1200}"#;
        let out = fx
            .processor
            .handle(&ep, body, Some(&sign(SECRET, body)))
            .await;
        assert_eq!(out.status, DeliveryStatus::Notified);
        assert_eq!(out.detail, "通知済み（LLM解釈なし・フォールバック整形）");
        let sent = fx.notifier.sent.lock().unwrap().clone();
        assert_eq!(sent[0].2.description, "- **order**: A-1\n- **total**: 1200");
        assert_eq!(sent[0].2.target, WebhookTarget::Dm);
        assert_eq!(count(&fx.db, "SELECT COUNT(*) FROM todos").await, 0);
        assert_eq!(count(&fx.db, "SELECT COUNT(*) FROM reminders").await, 0);
    }

    #[tokio::test]
    async fn notify_failure_skips_conversions() {
        let fx = fixture(
            FakeNotifier {
                fail: true,
                ..FakeNotifier::default()
            },
            FakeAux::default(),
        );
        let ep = endpoint(
            &fx,
            EndpointCreate {
                create_todo: true,
                create_reminder: true,
                ..named("CI")
            },
        )
        .await;
        let body = b"{}";
        let out = fx
            .processor
            .handle(&ep, body, Some(&sign(SECRET, body)))
            .await;
        assert_eq!(out.status, DeliveryStatus::Failed);
        assert_eq!(out.detail, "Discord通知の送信に失敗しました");
        assert_eq!(count(&fx.db, "SELECT COUNT(*) FROM todos").await, 0);
        assert_eq!(count(&fx.db, "SELECT COUNT(*) FROM reminders").await, 0);
    }
}
