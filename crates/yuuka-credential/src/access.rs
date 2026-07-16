//! `CredentialAccessRepo` — Bot への認証情報利用許可（`bot_credential_access`）。
//!
//! Node `db/credentialAccessRepo.ts` パリティ。`credentials` は `(owner_id, service_name)` 所有の
//! まま、使わせる Bot を許可リストで選ぶ（v5・§6）。credentials への DB FK は無いため、削除時の
//! 掃除は [`CredentialAccessRepo::delete_all_grants`] を明示的に呼ぶ。`service_name` は credentials と
//! 同じ正規化前提（本 repo でも trim + 小文字化して照合・保存する）。**`owner_id` を全クエリの必須
//! キー**とし、他ユーザーの許可を跨がない（データ分離・§12.2 契約5）。
//!
//! 消費側（register ルートの owner-Bot 一括付与 / GET 一覧の許可フィルタ / delete の掃除 /
//! addCredential ツールの応対 Bot 付与 / ランタイムの `is_granted` ゲート）は後続増分で配線する。

use rusqlite::params;
use yuuka_core::DbError;
use yuuka_db::{map_sqlite, ReadPool, WriterHandle};
use yuuka_web::Db;

use crate::repo::normalize_service_name;

/// `bot_credential_access` へのデータアクセス（DB ハンドルを借用する軽量ラッパ）。
pub struct CredentialAccessRepo<'a> {
    read: &'a ReadPool,
    writer: &'a WriterHandle,
}

impl<'a> CredentialAccessRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// Bot に当該認証情報の利用を許可する（冪等・Node `grantCredentialToBot`）。
    ///
    /// # Errors
    /// 書き込み失敗時 [`DbError`]。
    pub async fn grant(
        &self,
        bot_id: &str,
        owner_id: &str,
        service_name: &str,
    ) -> Result<(), DbError> {
        let (bid, oid, svc) = (
            bot_id.to_owned(),
            owner_id.to_owned(),
            normalize_service_name(service_name),
        );
        self.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT OR IGNORE INTO bot_credential_access (bot_id, owner_id, service_name) \
                     VALUES (?1, ?2, ?3)",
                    params![bid, oid, svc],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
    }

    /// Bot から利用許可を取り消す（Node `revokeCredentialFromBot`）。
    ///
    /// # Errors
    /// 書き込み失敗時 [`DbError`]。
    pub async fn revoke(
        &self,
        bot_id: &str,
        owner_id: &str,
        service_name: &str,
    ) -> Result<(), DbError> {
        let (bid, oid, svc) = (
            bot_id.to_owned(),
            owner_id.to_owned(),
            normalize_service_name(service_name),
        );
        self.writer
            .transaction(move |tx| {
                tx.execute(
                    "DELETE FROM bot_credential_access \
                     WHERE bot_id = ?1 AND owner_id = ?2 AND service_name = ?3",
                    params![bid, oid, svc],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
    }

    /// 当該認証情報の利用を許可されている Bot ID 一覧（Node `listBotIdsForCredential`）。
    ///
    /// # Errors
    /// 読み取り失敗時 [`DbError`]。
    pub async fn list_bot_ids_for_credential(
        &self,
        owner_id: &str,
        service_name: &str,
    ) -> Result<Vec<String>, DbError> {
        let (oid, svc) = (owner_id.to_owned(), normalize_service_name(service_name));
        self.read
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT bot_id FROM bot_credential_access \
                         WHERE owner_id = ?1 AND service_name = ?2",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![oid, svc], |r| r.get::<_, String>(0))
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// Bot が利用を許可されている認証情報名一覧（owner 所有分・Node `listCredentialNamesForBot`）。
    ///
    /// # Errors
    /// 読み取り失敗時 [`DbError`]。
    pub async fn list_credential_names_for_bot(
        &self,
        bot_id: &str,
        owner_id: &str,
    ) -> Result<Vec<String>, DbError> {
        let (bid, oid) = (bot_id.to_owned(), owner_id.to_owned());
        self.read
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT service_name FROM bot_credential_access \
                         WHERE bot_id = ?1 AND owner_id = ?2",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![bid, oid], |r| r.get::<_, String>(0))
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// Bot が当該認証情報の利用を許可されているか（ランタイムゲート・Node `isCredentialGrantedToBot`）。
    ///
    /// # Errors
    /// 読み取り失敗時 [`DbError`]。
    pub async fn is_granted(
        &self,
        bot_id: &str,
        owner_id: &str,
        service_name: &str,
    ) -> Result<bool, DbError> {
        let (bid, oid, svc) = (
            bot_id.to_owned(),
            owner_id.to_owned(),
            normalize_service_name(service_name),
        );
        self.read
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT 1 FROM bot_credential_access \
                         WHERE bot_id = ?1 AND owner_id = ?2 AND service_name = ?3 LIMIT 1",
                    )
                    .map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![bid, oid, svc], |_| Ok::<(), rusqlite::Error>(()))
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => {
                        row.map_err(map_sqlite)?;
                        Ok(true)
                    }
                    None => Ok(false),
                }
            })
            .await
    }

    /// 登録した認証情報を「owner 本人の全 Bot ＋ 共有秘書（system_default）」へ利用許可する。
    ///
    /// Node `credentialRoutes.grantCredentialToOwnerBots`（register ルート）パリティ:
    /// `listBotsOwnedBy(owner)`（system_default 以外の所有 Bot）∪ `{system_default}` の各 Bot へ
    /// 冪等付与する。付与は全て owner 本人のスコープなのでクロステナント露出は起きない
    /// （system_default は「発話者 = owner の会話」でのみ当該許可が効く）。所有 Bot 取得と付与を
    /// 単一書き込みトランザクションで原子的に行う（部分適用を避ける）。`service_name` は正規化する。
    ///
    /// # Errors
    /// 書き込み失敗時 [`DbError`]。
    pub async fn grant_to_owner_bots(
        &self,
        owner_id: &str,
        service_name: &str,
    ) -> Result<(), DbError> {
        let (oid, svc) = (owner_id.to_owned(), normalize_service_name(service_name));
        self.writer
            .transaction(move |tx| {
                // owner 本人が所有する Bot（system_default 以外）を集める。
                let mut bot_ids: Vec<String> = {
                    let mut stmt = tx
                        .prepare(
                            "SELECT id FROM bots WHERE user_id = ?1 AND id != 'system_default'",
                        )
                        .map_err(map_sqlite)?;
                    let rows = stmt
                        .query_map(params![oid], |r| r.get::<_, String>(0))
                        .map_err(map_sqlite)?;
                    let mut out = Vec::new();
                    for row in rows {
                        out.push(row.map_err(map_sqlite)?);
                    }
                    out
                };
                // 共有秘書は常に付与対象（Node `.add("system_default")`）。
                bot_ids.push("system_default".to_owned());
                for bot_id in &bot_ids {
                    tx.execute(
                        "INSERT OR IGNORE INTO bot_credential_access (bot_id, owner_id, service_name) \
                         VALUES (?1, ?2, ?3)",
                        params![bot_id, oid, svc],
                    )
                    .map_err(map_sqlite)?;
                }
                Ok(())
            })
            .await
    }

    /// 認証情報削除時に、その許可を全て掃除する（credentials への DB FK が無いため明示的に呼ぶ・
    /// Node `deleteAllGrantsForCredential`）。
    ///
    /// # Errors
    /// 書き込み失敗時 [`DbError`]。
    pub async fn delete_all_grants(
        &self,
        owner_id: &str,
        service_name: &str,
    ) -> Result<(), DbError> {
        let (oid, svc) = (owner_id.to_owned(), normalize_service_name(service_name));
        self.writer
            .transaction(move |tx| {
                tx.execute(
                    "DELETE FROM bot_credential_access WHERE owner_id = ?1 AND service_name = ?2",
                    params![oid, svc],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// FK（bot_id→bots・owner_id→users）を満たすよう users + bots を seed した DB を返す。
    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_credaccess_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).expect("create empty");
            drop(conn);
        }
        let db = Db::open(&path).expect("open");
        {
            let conn = rusqlite::Connection::open(&path).expect("seed conn");
            for uid in ["owner", "stranger"] {
                conn.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                     VALUES (?1, ?1, 'x', 'x')",
                    rusqlite::params![uid],
                )
                .expect("seed user");
            }
            for bot in ["system_default", "b1"] {
                conn.execute(
                    "INSERT OR IGNORE INTO bots (id, user_id, name) VALUES (?1, 'owner', 'n')",
                    rusqlite::params![bot],
                )
                .expect("seed bot");
            }
            // 他ユーザー所有 Bot（grant_to_owner_bots が跨がないことの検証用）。
            conn.execute(
                "INSERT OR IGNORE INTO bots (id, user_id, name) VALUES ('foreign', 'stranger', 'n')",
                [],
            )
            .expect("seed foreign bot");
        }
        db
    }

    #[tokio::test]
    async fn grant_is_idempotent_and_normalizes_service_name() {
        let db = seed_db();
        let repo = CredentialAccessRepo::new(&db);
        // 大文字/前後空白は正規化されて保存される。
        repo.grant("system_default", "owner", "  GitHub  ")
            .await
            .unwrap();
        // 冪等: 正規化後同名を再付与してもエラーにならず 1 行のまま。
        repo.grant("system_default", "owner", "github")
            .await
            .unwrap();
        repo.grant("b1", "owner", "GITHUB").await.unwrap();

        let mut bots = repo
            .list_bot_ids_for_credential("owner", "github")
            .await
            .unwrap();
        bots.sort();
        assert_eq!(bots, vec!["b1".to_owned(), "system_default".to_owned()]);

        // 正規化により大文字問い合わせでもヒットする。
        assert!(repo
            .is_granted("system_default", "owner", "GitHub")
            .await
            .unwrap());
        assert!(!repo.is_granted("b1", "owner", "gitlab").await.unwrap());
    }

    #[tokio::test]
    async fn list_names_scopes_to_bot_and_owner() {
        let db = seed_db();
        let repo = CredentialAccessRepo::new(&db);
        repo.grant("system_default", "owner", "github")
            .await
            .unwrap();
        repo.grant("system_default", "owner", "gitlab")
            .await
            .unwrap();
        repo.grant("b1", "owner", "aws").await.unwrap();

        let mut names = repo
            .list_credential_names_for_bot("system_default", "owner")
            .await
            .unwrap();
        names.sort();
        assert_eq!(names, vec!["github".to_owned(), "gitlab".to_owned()]);
    }

    #[tokio::test]
    async fn grant_to_owner_bots_covers_owned_and_system_default_only() {
        let db = seed_db();
        let repo = CredentialAccessRepo::new(&db);
        // owner の全 Bot（b1）＋ system_default へ冪等付与。正規化される。
        repo.grant_to_owner_bots("owner", "  GitHub ")
            .await
            .unwrap();
        let mut bots = repo
            .list_bot_ids_for_credential("owner", "github")
            .await
            .unwrap();
        bots.sort();
        assert_eq!(bots, vec!["b1".to_owned(), "system_default".to_owned()]);
        // 他ユーザー所有 Bot（foreign）は付与されない（クロステナント露出しない）。
        assert!(!bots.contains(&"foreign".to_owned()));
        assert!(!repo.is_granted("foreign", "owner", "github").await.unwrap());
        // 冪等: 再実行しても重複しない。
        repo.grant_to_owner_bots("owner", "github").await.unwrap();
        let bots2 = repo
            .list_bot_ids_for_credential("owner", "github")
            .await
            .unwrap();
        assert_eq!(bots2.len(), 2);
    }

    #[tokio::test]
    async fn revoke_and_delete_all_clear_grants() {
        let db = seed_db();
        let repo = CredentialAccessRepo::new(&db);
        repo.grant("system_default", "owner", "github")
            .await
            .unwrap();
        repo.grant("b1", "owner", "github").await.unwrap();

        // revoke は 1 Bot 分のみ消す。
        repo.revoke("system_default", "owner", "github")
            .await
            .unwrap();
        assert!(!repo
            .is_granted("system_default", "owner", "github")
            .await
            .unwrap());
        assert!(repo.is_granted("b1", "owner", "github").await.unwrap());

        // delete_all_grants は owner×service の全 Bot 分を掃除する。
        repo.grant("system_default", "owner", "github")
            .await
            .unwrap();
        repo.delete_all_grants("owner", "github").await.unwrap();
        assert!(repo
            .list_bot_ids_for_credential("owner", "github")
            .await
            .unwrap()
            .is_empty());
    }
}
