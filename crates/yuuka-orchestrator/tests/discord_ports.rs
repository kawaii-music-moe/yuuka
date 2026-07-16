//! P1-3 Discord ポートの DB 実装の統合テスト（実 SQLite・ネットワーク不要）。
//!
//! [`DbBotDirectory`]（Bot メタ・アクセス判定・トークン復号）・[`DbMembership`]（申請/共有/ペルソナ）・
//! [`InMemoryRateLimiter`]（固定窓）を実マイグレーション済み DB へ seed して検証する。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use secrecy::{ExposeSecret, SecretString};
use yuuka_core::{BotId, GuildId, UserId};
use yuuka_crypto::SystemCrypto;
use yuuka_discord::{BotDirectory, MemberDecision, MembershipService, RateExceeded, RateLimiter};
use yuuka_orchestrator::{DbBotDirectory, DbMembership, InMemoryRateLimiter};
use yuuka_web::Db;

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn fresh_db() -> (Db, std::path::PathBuf) {
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("yuuka_ports_it_{}_{n}.sqlite", std::process::id()));
    {
        rusqlite::Connection::open(&path).expect("seed file");
    }
    let db = Db::open(&path).expect("open db");
    (db, path)
}

fn conn(path: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).expect("open")
}

fn seed_user(path: &std::path::Path, discord_id: &str, role: &str) {
    conn(path)
        .execute(
            "INSERT INTO users (discord_id, username, password_hash, salt, role) \
             VALUES (?1, ?1, 'x', '00', ?2)",
            rusqlite::params![discord_id, role],
        )
        .expect("seed user");
}

/// Bot を seed する（capabilities 既定は秘書込み・`guild_assistant=true` で secretary を外す）。
fn seed_bot(path: &std::path::Path, id: &str, owner: &str, name: &str, guild_assistant: bool) {
    let caps = if guild_assistant {
        r#"["persona","memory","mcp"]"#
    } else {
        r#"["persona","memory","mcp","secretary"]"#
    };
    conn(path)
        .execute(
            "INSERT INTO bots (id, user_id, name, capabilities) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, owner, name, caps],
        )
        .expect("seed bot");
}

#[tokio::test]
async fn directory_get_bot_maps_record_fields() {
    let (db, path) = fresh_db();
    seed_user(&path, "owner1", "user");
    seed_bot(&path, "botA", "owner1", "アシスタント", true);
    // 秘書 Bot（capabilities に secretary あり）。
    seed_bot(&path, "botS", "owner1", "秘書", false);
    let dir = DbBotDirectory::new(db, None);

    let a = dir.get_bot(&BotId::new("botA")).await.expect("botA");
    assert_eq!(a.name, "アシスタント");
    assert_eq!(a.owner_id, UserId::new("owner1"));
    assert!(a.is_guild_assistant, "secretary 無し → 汎用モード");
    assert!(!a.has_gemini_key, "キー未設定");

    let s = dir.get_bot(&BotId::new("botS")).await.expect("botS");
    assert!(!s.is_guild_assistant, "secretary 有り → 秘書");

    assert!(dir.get_bot(&BotId::new("missing")).await.is_none());
}

#[tokio::test]
async fn directory_list_bots_for_user_includes_owner_shared_and_default() {
    let (db, path) = fresh_db();
    seed_user(&path, "owner1", "user");
    seed_user(&path, "guest", "user");
    seed_bot(&path, "system_default", "owner1", "ユウカ", false);
    seed_bot(&path, "botOwned", "owner1", "自分の", false);
    seed_bot(&path, "botShared", "owner1", "共有元", false);
    // guest には botShared が active 共有されている。
    conn(&path)
        .execute(
            "INSERT INTO bot_shares (bot_id, owner_id, shared_user_id, status) \
             VALUES ('botShared', 'owner1', 'guest', 'active')",
            [],
        )
        .expect("share");
    let dir = DbBotDirectory::new(db, None);

    let ids = dir.list_bots_for_user(&UserId::new("guest")).await;
    // guest はオーナーではないが system_default + 共有 active を見られる。
    assert!(ids.contains(&BotId::new("system_default")));
    assert!(ids.contains(&BotId::new("botShared")));
    assert!(
        !ids.contains(&BotId::new("botOwned")),
        "非共有の他人 Bot は見えない"
    );
}

#[tokio::test]
async fn directory_membership_and_guild_role_checks() {
    let (db, path) = fresh_db();
    seed_user(&path, "owner1", "user");
    seed_bot(&path, "botA", "owner1", "A", true);
    conn(&path)
        .execute(
            "INSERT INTO bot_guilds (bot_id, guild_id) VALUES ('botA', 'g1')",
            [],
        )
        .unwrap();
    conn(&path)
        .execute(
            "INSERT INTO bot_members (bot_id, guild_id, user_id, added_by) \
             VALUES ('botA', 'g1', 'mem1', 'owner1')",
            [],
        )
        .unwrap();
    conn(&path)
        .execute(
            "INSERT INTO bot_roles (bot_id, guild_id, role_id, added_by) \
             VALUES ('botA', 'g1', 'role9', 'owner1')",
            [],
        )
        .unwrap();
    let dir = DbBotDirectory::new(db, None);
    let (bot, g1) = (BotId::new("botA"), GuildId::new("g1"));

    assert!(dir.is_guild_allowed(&bot, &g1).await);
    assert!(!dir.is_guild_allowed(&bot, &GuildId::new("g2")).await);
    assert!(dir.is_bot_member(&bot, &g1, &UserId::new("mem1")).await);
    assert!(!dir.is_bot_member(&bot, &g1, &UserId::new("stranger")).await);
    assert!(
        dir.is_any_role_allowed(&bot, &g1, &["roleX".into(), "role9".into()])
            .await
    );
    assert!(!dir.is_any_role_allowed(&bot, &g1, &["roleX".into()]).await);
    // 空ロールは false（Node パリティ）。
    assert!(!dir.is_any_role_allowed(&bot, &g1, &[]).await);
    // registered ユーザー判定。
    assert!(dir.is_registered_user(&UserId::new("owner1")).await);
    assert!(!dir.is_registered_user(&UserId::new("nobody")).await);
}

#[tokio::test]
async fn directory_decrypts_discord_token_with_crypto() {
    let (db, path) = fresh_db();
    seed_user(&path, "owner1", "user");
    seed_bot(&path, "botA", "owner1", "A", true);
    let crypto = SystemCrypto::new(SecretString::from("ports-test-secret".to_owned())).unwrap();
    let enc = crypto
        .encrypt_text("super-secret-bot-token")
        .expect("encrypt");
    conn(&path)
        .execute(
            "UPDATE bots SET discord_token_encrypted = ?1, discord_token_iv = ?2, \
             discord_token_tag = ?3 WHERE id = 'botA'",
            rusqlite::params![enc.encrypted, enc.iv, enc.auth_tag],
        )
        .unwrap();

    // crypto 有り → 復号できる。
    let dir = DbBotDirectory::new(db.clone(), Some(Arc::new(crypto)));
    let token = dir.decrypt_token(&BotId::new("botA")).await.expect("token");
    assert_eq!(token.expose_secret(), "super-secret-bot-token");

    // crypto 無し → None（起動対象 0 に縮退）。
    let dir_no_crypto = DbBotDirectory::new(db, None);
    assert!(dir_no_crypto
        .decrypt_token(&BotId::new("botA"))
        .await
        .is_none());
}

#[tokio::test]
async fn membership_submit_and_decide_adds_member() {
    let (db, path) = fresh_db();
    seed_user(&path, "owner1", "user");
    seed_bot(&path, "botA", "owner1", "A", true);
    let ms = DbMembership::new(db.clone());
    let dir = DbBotDirectory::new(db, None);
    let (bot, g1) = (BotId::new("botA"), GuildId::new("g1"));

    // owner 自身の申請は拒否。
    let owner_try = ms
        .submit_member_request(&bot, "g1", &UserId::new("owner1"), None, None)
        .await;
    assert!(!owner_try.ok);
    assert!(owner_try.message.contains("オーナー"));

    // 申請作成。
    let applied = ms
        .submit_member_request(&bot, "g1", &UserId::new("applicant"), None, None)
        .await;
    assert!(applied.ok, "{}", applied.message);

    // 二重申請は不可。
    let again = ms
        .submit_member_request(&bot, "g1", &UserId::new("applicant"), None, None)
        .await;
    assert!(!again.ok);

    // 承認 → メンバー追加（request id は 1）。
    let request_id: i64 = conn(&path)
        .query_row(
            "SELECT id FROM bot_member_requests WHERE user_id = 'applicant'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let decided = ms
        .decide_member_request(request_id, MemberDecision::Approved, &UserId::new("owner1"))
        .await;
    assert!(decided.ok, "{}", decided.message);
    assert_eq!(decided.status, Some(MemberDecision::Approved));
    assert!(
        dir.is_bot_member(&bot, &g1, &UserId::new("applicant"))
            .await,
        "承認でメンバー化"
    );

    // 非オーナー・非 admin は承認/却下不可（新規申請を作ってから試す）。
    ms.submit_member_request(&bot, "g1", &UserId::new("other"), None, None)
        .await;
    let other_req: i64 = conn(&path)
        .query_row(
            "SELECT id FROM bot_member_requests WHERE user_id = 'other'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let forbidden = ms
        .decide_member_request(
            other_req,
            MemberDecision::Approved,
            &UserId::new("stranger"),
        )
        .await;
    assert!(!forbidden.ok, "オーナー/Admin のみ");
}

#[tokio::test]
async fn membership_share_and_persona_import() {
    let (db, path) = fresh_db();
    seed_user(&path, "owner1", "user");
    seed_user(&path, "guest", "user");
    seed_bot(&path, "botA", "owner1", "A", false);
    // pending 共有 + 公開ペルソナ。
    conn(&path)
        .execute(
            "INSERT INTO bot_shares (bot_id, owner_id, shared_user_id, status) \
             VALUES ('botA', 'owner1', 'guest', 'pending')",
            [],
        )
        .unwrap();
    conn(&path)
        .execute(
            "INSERT INTO personas (owner_id, name, prompt, is_public) \
             VALUES ('owner1', '公開ペルソナ', 'プロンプト本文', 1)",
            [],
        )
        .unwrap();
    let ms = DbMembership::new(db);

    let share_id: i64 = conn(&path)
        .query_row(
            "SELECT id FROM bot_shares WHERE shared_user_id = 'guest'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let share = ms.get_share(share_id).await.expect("share");
    assert_eq!(share.status, "pending");
    assert_eq!(share.shared_user_id, UserId::new("guest"));

    // 承認 → active。
    ms.accept_share(&BotId::new("botA"), &UserId::new("guest"))
        .await;
    let status: String = conn(&path)
        .query_row(
            "SELECT status FROM bot_shares WHERE id = ?1",
            [share_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "active");

    // 公開ペルソナ取得 + インポート（独立コピー）。
    let persona_id: i64 = conn(&path)
        .query_row(
            "SELECT id FROM personas WHERE name = '公開ペルソナ'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(ms.get_public_persona(persona_id).await.is_some());
    assert!(ms.import_persona(&UserId::new("guest"), persona_id).await);
    let copies: i64 = conn(&path)
        .query_row(
            "SELECT COUNT(*) FROM personas WHERE owner_id = 'guest' AND name = '公開ペルソナ'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(copies, 1, "guest のコピーが 1 件作られる");
}

#[tokio::test]
async fn rate_limiter_denies_after_user_minute_limit() {
    let (db, path) = fresh_db();
    // ユーザー分間上限を 2 に絞る（日/ギルドは既定 100/1000 のまま）。
    conn(&path)
        .execute(
            "INSERT INTO system_settings (key, value) VALUES ('mcp_rate_user_per_minute', '2')",
            [],
        )
        .unwrap();
    let rl = InMemoryRateLimiter::new(db);
    let (bot, g1, user) = (BotId::new("botA"), GuildId::new("g1"), UserId::new("mem1"));

    assert!(rl.consume(&bot, &g1, &user).await.allowed, "1 回目 allow");
    assert!(rl.consume(&bot, &g1, &user).await.allowed, "2 回目 allow");
    let third = rl.consume(&bot, &g1, &user).await;
    assert!(!third.allowed, "3 回目は分間上限で deny");
    assert_eq!(third.exceeded, Some(RateExceeded::UserMinute));

    // 別ユーザーは独立カウント（影響を受けない）。
    assert!(rl.consume(&bot, &g1, &UserId::new("mem2")).await.allowed);
}
