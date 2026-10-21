//! Engine quota admission, independent of Gemini credentials and HTTP adapters.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
use std::sync::Arc;
use yuuka_core::{BotId, GuildId, UserId};
use yuuka_discord::RateExceeded;
use yuuka_orchestrator::{Channel, ChatEngine, RealGeminiFactory};

fn engine(minute: u32, day: u32, guild: u32) -> ChatEngine {
    let path = std::env::temp_dir().join(format!(
        "yuuka_permits_{}_{}_{}_{}.sqlite",
        std::process::id(),
        minute,
        day,
        guild
    ));
    let _ = std::fs::remove_file(&path);
    let db = yuuka_web::Db::open(&path).expect("db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    for (key, value) in [
        ("mcp_rate_user_per_minute", minute),
        ("mcp_rate_user_per_day", day),
        ("mcp_rate_guild_per_day", guild),
    ] {
        conn.execute(
            "INSERT INTO system_settings (key,value) VALUES (?1,?2)",
            rusqlite::params![key, value.to_string()],
        )
        .unwrap();
    }
    ChatEngine::new(
        db,
        None,
        yuuka_tools::ToolRegistry::new(),
        Arc::new(RealGeminiFactory),
        None,
        Arc::new(yuuka_mcp::NullMcpClient),
        None,
    )
}

#[tokio::test]
async fn minute_window_is_shared_across_discord_pwa_web_and_engine_clones() {
    let e = engine(2, 100, 100);
    let (b, u) = (BotId::new("b"), UserId::new("u"));
    e.acquire_turn(&b, &u, Channel::Discord(GuildId::new("guild")))
        .await
        .unwrap();
    e.clone().acquire_turn(&b, &u, Channel::Pwa).await.unwrap();
    assert!(matches!(
        e.acquire_turn(&b, &u, Channel::Web).await,
        Err(RateExceeded::UserMinute)
    ));
    e.acquire_turn(&b, &UserId::new("other"), Channel::Web)
        .await
        .unwrap();
    e.acquire_turn(&BotId::new("other"), &u, Channel::Web)
        .await
        .unwrap();
}

#[tokio::test]
async fn day_window_is_shared_across_http_channels() {
    let e = engine(100, 1, 100);
    let (b, u) = (BotId::new("b"), UserId::new("u"));
    e.acquire_turn(&b, &u, Channel::Pwa).await.unwrap();
    assert!(matches!(
        e.acquire_turn(&b, &u, Channel::Web).await,
        Err(RateExceeded::UserDay)
    ));
}

#[tokio::test]
async fn guild_day_windows_keep_discord_pwa_and_web_separate() {
    let e = engine(100, 100, 1);
    let (b, u) = (BotId::new("b"), UserId::new("u"));
    for channel in [
        Channel::Discord(GuildId::new("guild")),
        Channel::Pwa,
        Channel::Web,
    ] {
        e.acquire_turn(&b, &u, channel).await.unwrap();
    }
    for channel in [
        Channel::Discord(GuildId::new("guild")),
        Channel::Pwa,
        Channel::Web,
    ] {
        assert!(matches!(
            e.acquire_turn(&b, &UserId::new("other"), channel).await,
            Err(RateExceeded::GuildDay)
        ));
    }
    e.acquire_turn(&b, &u, Channel::Discord(GuildId::new("other-guild")))
        .await
        .unwrap();
}

struct NoDelivery;
#[async_trait::async_trait]
impl yuuka_discord::TurnDelivery for NoDelivery {
    async fn on_interim(&self, _: String) {
        unreachable!("quota rejection must precede interim delivery");
    }
    async fn deliver_final(&self, _: yuuka_discord::TurnReply) {
        unreachable!("quota rejection must precede deferred delivery");
    }
}

#[tokio::test]
async fn discord_rejection_precedes_llm_user_log_and_deferred_worker() {
    use yuuka_discord::{IncomingChat, Speaker, TurnError, TurnProcessor};
    let e = engine(1, 100, 100);
    let (b, u, g) = (BotId::new("b"), UserId::new("u"), GuildId::new("g"));
    e.acquire_turn(&b, &u, Channel::Pwa).await.unwrap();
    let result = e
        .process_guild(
            &b,
            &g,
            Speaker {
                user_id: u,
                display_name: "User".into(),
            },
            IncomingChat::default(),
            Arc::new(|_| {}),
            Arc::new(NoDelivery),
        )
        .await;
    assert!(matches!(
        result,
        Err(TurnError::RateLimited(RateExceeded::UserMinute))
    ));
}
