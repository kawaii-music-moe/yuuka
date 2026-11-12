//! 会話記憶の保守 cron（docs/design/memory_retention.md）と、その評価に使う Gemini の呼び出し口。
//!
//! - [`SynapseMaintenanceService`]: 毎時、候補が溜まったスコープ（10 件以上か最古が 12 時間超）を
//!   評価器（[`yuuka_orchestrator::synapse_maintain::evaluate_scope`]）にかけ、日付が変わっていれば
//!   減衰と忘却（`decay_and_prune`）を 1 回行う。
//! - [`GeminiMemoryLlm`]: 秘書スコープ（guild なし）は利用者本人の Gemini キー、ギルドスコープは
//!   Bot 専用キーで評価する（[`crate::GeminiAuxText`] と同じ要領。キーが無ければ `None`＝縮退）。
//!
//! `yuuka-services` は orchestrator に依存しないため、両者を知る supervisor でサービスを組む
//! （playbook / backup のアダプタと同じ規律）。

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Local, NaiveDate};
use tokio::sync::Mutex;
use yuuka_core::secrets::wrap_secret;
use yuuka_core::MetricsSink;
use yuuka_crypto::SystemCrypto;
use yuuka_gemini::{Content, GeminiClient, DEFAULT_MODEL};
use yuuka_orchestrator::synapse_maintain::{decay_and_prune, evaluate_scope};
use yuuka_orchestrator::{bot_repo, user, MemoryLlm, MemoryScope};
use yuuka_services::{CronService, Schedule, ServiceContext};
use yuuka_synapse::SynapseEngine;
use yuuka_web::Db;

/// このだけ候補が溜まったら、待たずに評価する。
pub const EVAL_WHEN_COUNT: i64 = 10;
/// 最古の候補がこの秒数を過ぎたら、件数が少なくても評価する。
pub const EVAL_WHEN_OLDEST_SECS: i64 = 12 * 3600;

/// 記憶の評価・減衰・忘却を回す cron サービス。
pub struct SynapseMaintenanceService {
    engine: Arc<Mutex<SynapseEngine>>,
    llm: Arc<dyn MemoryLlm>,
    /// 最後に減衰と忘却を行ったローカル日付（1 日 1 回にする）。
    last_prune: Mutex<Option<NaiveDate>>,
}

impl SynapseMaintenanceService {
    /// エンジン（RAM 索引）と評価用 LLM を注入して作る。
    #[must_use]
    pub fn new(engine: Arc<Mutex<SynapseEngine>>, llm: Arc<dyn MemoryLlm>) -> Self {
        Self {
            engine,
            llm,
            last_prune: Mutex::new(None),
        }
    }

    /// 候補を評価するスコープか（件数か待ち時間のどちらかが閾値を超えた）。
    #[must_use]
    pub fn is_due(count: i64, oldest_age_secs: i64) -> bool {
        count >= EVAL_WHEN_COUNT || oldest_age_secs >= EVAL_WHEN_OLDEST_SECS
    }
}

#[async_trait]
impl CronService for SynapseMaintenanceService {
    fn name(&self) -> &'static str {
        "synapse_maintenance"
    }

    fn schedule(&self) -> Schedule {
        Schedule::Hourly
    }

    /// 起動直後は走らせない（再起動のたびに LLM を呼ばない。次の正時に回る）。
    fn run_on_start(&self) -> bool {
        false
    }

    async fn tick(&self, ctx: &ServiceContext) {
        let metrics: &dyn MetricsSink = ctx.metrics.as_ref();
        match yuuka_orchestrator::synapse_repo::list_candidate_scopes(&ctx.db).await {
            Ok(scopes) => {
                for s in scopes {
                    if !Self::is_due(s.count, s.oldest_age_secs) {
                        continue;
                    }
                    if let Err(e) =
                        evaluate_scope(&ctx.db, &self.engine, &*self.llm, &s.scope, Some(metrics))
                            .await
                    {
                        tracing::warn!(error = %e, user = %s.scope.user_id, bot = %s.scope.bot_id, "[Synapse] 候補の評価に失敗");
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "[Synapse] 候補のあるスコープを読めません"),
        }

        let today = Local::now().date_naive();
        let mut last = self.last_prune.lock().await;
        if *last == Some(today) {
            return;
        }
        match decay_and_prune(&ctx.db, &self.engine, Some(metrics)).await {
            Ok(_) => *last = Some(today),
            Err(e) => {
                tracing::warn!(error = %e, "[Synapse] 減衰と忘却に失敗（次の時刻にやり直します）")
            }
        }
    }
}

/// 評価に使う Gemini の呼び出し口。
pub struct GeminiMemoryLlm {
    db: Db,
    crypto: Option<Arc<SystemCrypto>>,
}

impl GeminiMemoryLlm {
    /// DB（キーの読み出し）と crypto（復号）を注入して構築する。
    #[must_use]
    pub fn new(db: Db, crypto: Option<Arc<SystemCrypto>>) -> Self {
        Self { db, crypto }
    }

    /// スコープに応じたキーとモデルを取り出す（秘書＝本人キー・ギルド＝Bot 専用キー）。
    async fn key_and_model(&self, scope: &MemoryScope) -> Option<(String, String, String, String)> {
        if scope.guild_id.is_some() {
            let t = bot_repo::bot_gemini(&self.db, &scope.bot_id).await.ok()??;
            Some((t.encrypted, t.iv, t.tag, DEFAULT_MODEL.to_owned()))
        } else {
            let cfg = user::user_gemini(&self.db, &scope.user_id).await.ok()??;
            Some((cfg.encrypted, cfg.iv, cfg.tag, cfg.model))
        }
    }
}

#[async_trait]
impl MemoryLlm for GeminiMemoryLlm {
    async fn generate(&self, scope: &MemoryScope, prompt: &str) -> Option<String> {
        let (encrypted, iv, tag, model) = self.key_and_model(scope).await?;
        let crypto = self.crypto.as_ref()?;
        let key = match crypto.decrypt_text(&encrypted, &iv, &tag) {
            Ok(key) => key,
            Err(e) => {
                tracing::warn!(error = %e, "[Synapse] 評価用の Gemini キーを復号できません");
                return None;
            }
        };
        let client = match GeminiClient::new(model, wrap_secret(key)) {
            Ok(client) => client,
            Err(e) => {
                tracing::warn!(error = %e, "[Synapse] Gemini クライアントを構築できません");
                return None;
            }
        };
        let resp = client
            .generate_aux(None, vec![Content::user_text(prompt)], None)
            .await?;
        Some(resp.text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_when_enough_candidates_or_old_enough() {
        assert!(SynapseMaintenanceService::is_due(10, 0));
        assert!(SynapseMaintenanceService::is_due(1, 12 * 3600));
        assert!(!SynapseMaintenanceService::is_due(9, 12 * 3600 - 1));
    }
}
