//! Background ToDo tagging, independent of HTTP/Discord responses. Bounded concurrency and
//! a total timeout include credential loading, vocabulary reads, retries and LLM generation.
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use yuuka_todo::auto_tags::{parse_tags, prompt, TagJob};
use yuuka_todo::repo::TodoRepo;

use crate::{CronService, Schedule, ServiceContext};

/// User-owned auxiliary LLM (missing key, bad response or network error => None).
#[async_trait]
pub trait TagGenerator: Send + Sync {
    async fn generate(&self, user_id: &str, prompt: &str) -> Option<String>;
}

/// Durable queue consumer. Registered once by supervisor, even when legacy cron is disabled.
pub struct AutoTagService {
    pub generator: Arc<dyn TagGenerator>,
}

#[async_trait]
impl CronService for AutoTagService {
    fn name(&self) -> &'static str {
        "todo-auto-tags"
    }
    fn schedule(&self) -> Schedule {
        Schedule::FixedSecs(1)
    }
    async fn tick(&self, ctx: &ServiceContext) {
        let repo = TodoRepo::new(&ctx.db);
        let jobs = match repo.claim_tag_jobs(ctx.cross).await {
            Ok(jobs) => jobs,
            Err(error) => {
                tracing::warn!(%error, "ToDo自動タグ: キュー取得失敗");
                return;
            }
        };
        let mut tasks = tokio::task::JoinSet::new();
        for job in jobs {
            let db = ctx.db.clone();
            let generator = self.generator.clone();
            tasks.spawn(async move {
                let repo = TodoRepo::new(&db);
                process_job(&repo, generator.as_ref(), job, Duration::from_secs(200)).await;
            });
        }
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                tracing::warn!(%error, "ToDo自動タグ: worker失敗");
            }
        }
    }
}

async fn process_job(
    repo: &TodoRepo<'_>,
    generator: &dyn TagGenerator,
    job: TagJob,
    timeout: Duration,
) {
    let tags = tokio::time::timeout(timeout, generate(repo, generator, &job))
        .await
        .unwrap_or_default();
    if tags.is_empty() {
        tracing::debug!(
            todo_id = job.id,
            "ToDo自動タグ: スキップまたは生成失敗、タグを維持"
        );
    }
    if let Err(error) = repo.finish_tag_job(job, tags).await {
        tracing::warn!(%error, "ToDo自動タグ: 保存失敗");
    }
}

async fn generate(repo: &TodoRepo<'_>, generator: &dyn TagGenerator, job: &TagJob) -> Vec<String> {
    let vocabulary = match repo.open_tags(&job.scope).await {
        Ok(vocabulary) => vocabulary,
        Err(error) => {
            tracing::warn!(%error, "ToDo自動タグ: 語彙取得失敗");
            return Vec::new();
        }
    };
    generator
        .generate(job.scope.user_id().as_str(), &prompt(job, vocabulary))
        .await
        .map_or_else(Vec::new, |text| parse_tags(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ctx_null, seeded_db};
    use std::sync::Mutex;
    use yuuka_core::{BotId, UserId, UserScope};
    use yuuka_todo::dto::{NewTodo, TodoUpdate};

    struct Fake {
        response: Option<String>,
        calls: Mutex<Vec<(String, String)>>,
    }
    #[async_trait]
    impl TagGenerator for Fake {
        async fn generate(&self, user: &str, prompt: &str) -> Option<String> {
            self.calls
                .lock()
                .unwrap()
                .push((user.to_owned(), prompt.to_owned()));
            self.response.clone()
        }
    }
    async fn context() -> (ServiceContext, tempfile::TempDir) {
        let (db, dir) = seeded_db("");
        db.writer.execute(|c| {
            c.execute_batch("INSERT INTO users (discord_id, username, salt, sk_account_id) VALUES ('u','u','s','a'),('other','other','s','b');").map_err(yuuka_db::map_sqlite)?;
            Ok(())
        }).await.unwrap();
        (ctx_null(db), dir)
    }
    #[tokio::test]
    async fn worker_uses_scoped_vocabulary_and_preserves_tags_on_failure() {
        let (ctx, _dir) = context().await;
        let repo = TodoRepo::new(&ctx.db);
        let scope = UserScope::new(UserId::new("u"), BotId::new("bot"));
        let task = repo
            .add(
                &scope,
                NewTodo {
                    title: "mine".into(),
                    tags: vec!["明示".into()],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        for (user, bot, tag) in [
            ("other", "bot", "OTHER_USER_SECRET"),
            ("u", "other-bot", "OTHER_BOT_SECRET"),
        ] {
            repo.add(
                &UserScope::new(UserId::new(user), BotId::new(bot)),
                NewTodo {
                    title: "other".into(),
                    tags: vec![tag.into()],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        let fake = Arc::new(Fake {
            response: Some("[\"自動\"]".into()),
            calls: Mutex::new(Vec::new()),
        });
        let service = AutoTagService {
            generator: fake.clone(),
        };
        service.tick(&ctx).await;
        {
            let calls = fake.calls.lock().unwrap();
            let (_, prompt) = calls
                .iter()
                .find(|(user, prompt)| user == "u" && prompt.contains("mine"))
                .unwrap();
            assert!(prompt.contains("明示"));
            assert!(!prompt.contains("OTHER_USER_SECRET"));
            assert!(!prompt.contains("OTHER_BOT_SECRET"));
        }
        assert_eq!(
            repo.get(&scope, task.id).await.unwrap().unwrap().tags,
            vec!["明示", "自動"]
        );
        for response in [None, Some("invalid".into())] {
            repo.update(
                &scope,
                task.id,
                TodoUpdate {
                    id: task.id,
                    description: Some(format!("{response:?}")),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            AutoTagService {
                generator: Arc::new(Fake {
                    response,
                    calls: Mutex::new(Vec::new()),
                }),
            }
            .tick(&ctx)
            .await;
            assert_eq!(
                repo.get(&scope, task.id).await.unwrap().unwrap().tags,
                vec!["明示", "自動"]
            );
            assert!(repo.claim_tag_jobs(ctx.cross).await.unwrap().is_empty());
        }
    }
    struct Hanging;
    #[async_trait]
    impl TagGenerator for Hanging {
        async fn generate(&self, _user: &str, _prompt: &str) -> Option<String> {
            std::future::pending().await
        }
    }
    #[tokio::test]
    async fn timeout_finishes_job_without_changing_manual_tags() {
        let (ctx, _dir) = context().await;
        let repo = TodoRepo::new(&ctx.db);
        let scope = UserScope::new(UserId::new("u"), BotId::new("bot"));
        let task = repo
            .add(
                &scope,
                NewTodo {
                    title: "hanging".into(),
                    tags: vec!["keep".into()],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let job = repo.claim_tag_jobs(ctx.cross).await.unwrap().pop().unwrap();
        process_job(&repo, &Hanging, job, Duration::from_millis(1)).await;
        assert_eq!(
            repo.get(&scope, task.id).await.unwrap().unwrap().tags,
            vec!["keep"]
        );
        assert!(repo.claim_tag_jobs(ctx.cross).await.unwrap().is_empty());
    }
}
