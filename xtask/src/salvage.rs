//! 旧環境のデータを、初期セットアップし直した新環境へ移すためのタスク。
//!
//! - `salvage-snapshot` — 稼働中の DB から一貫したスナップショットを取る（元の DB は読み取り専用で開く）。
//! - `salvage-import` — スナップショットを新環境の DB へ投入する。
//!
//! ```text
//! cargo run -p xtask -- salvage-snapshot --from data/yuuka.db --to data/salvage/<日時>/yuuka.db
//! cargo run -p xtask -- salvage-import --from <旧スナップショット.db> --into <新 DB> \
//!     --owner <管理者の Discord ID> [--secretary-name <名前>] [--apply]
//! ```
//!
//! 前提: 新環境で初期セットアップ（管理者登録 + システムデフォルト Bot のトークン登録）まで済ませ、
//! **サーバーを止めてから**実行する。`--apply` が無ければお試し実行（最後にロールバックし件数だけ出す）。
//!
//! 投入ルール:
//! - 暗号鍵は作り直す前提なので、暗号化された値（Gemini キー・Discord トークン・MCP 認証情報・
//!   Webhook シークレット・Google のリフレッシュトークン）は捨てて NULL にする。値そのものが暗号文の
//!   テーブル（認証情報・Google アカウント連携）と、使い捨てのペアリング（`voice_pairings`）は投入しない。
//! - システム Bot（`system_default`）は案内役で個人データを持たないため、`--owner` の
//!   `system_default` 宛てのデータは、このツールが作る新しい秘書 Bot へ付け替える。システム Bot の
//!   行そのものは投入しない（新環境のセットアップで作られたものを使う）。
//! - 旧環境で削除済みの Bot（`bots` に行が無い）に残っているデータは投入しない。
//! - 新環境に既にあるユーザー（セットアップした管理者）は、表示名・パスワード・権限・Gemini キーと
//!   モデルを新環境のままにし、通知・カレンダー・バックアップなどの設定だけ旧環境の値で上書きする。
//! - 監査ログは新環境の記録の後ろへ追記する（id は振り直す）。
//! - 投入中は外部キー検査を止め、最後に `PRAGMA foreign_key_check` で検査する。違反があれば全体を
//!   ロールバックする。

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection, OpenFlags, Transaction};

const SYSTEM_BOT: &str = "system_default";

/// 新秘書 Bot の能力（`yuuka-orchestrator` の `BotPresetId::Secretary` と同じ）。
const SECRETARY_CAPABILITIES: &str = r#"["persona","memory","mcp","secretary"]"#;

/// 投入しないテーブル（マイグレーション管理・FTS の索引・暗号文が本体・使い捨て）。
/// `message_logs_fts` はトリガーで `message_logs` から再構築される。
const SKIP_TABLES: &[&str] = &[
    "refinery_schema_history",
    "sqlite_sequence",
    "credentials",
    "user_google_accounts",
    "voice_pairings",
];

/// 個別に扱うテーブル（汎用コピーの対象外）。
const SPECIAL_TABLES: &[&str] = &["users", "bots", "audit_logs", "system_settings"];

/// 新環境のセットアップ・起動で行が入っていてよいテーブル。これ以外に行があれば投入しない。
const MAY_BE_POPULATED: &[&str] = &[
    "users",
    "bots",
    "audit_logs",
    "system_settings",
    "refinery_schema_history",
    "sqlite_sequence",
];

/// 捨てる暗号化列（テーブル → 列）。
const SECRET_COLUMNS: &[(&str, &[&str])] = &[
    (
        "users",
        &[
            "gemini_api_key_encrypted",
            "gemini_api_key_iv",
            "gemini_api_key_tag",
            "google_refresh_token_encrypted",
            "google_refresh_token_iv",
            "google_refresh_token_tag",
        ],
    ),
    (
        "bots",
        &[
            "discord_token_encrypted",
            "discord_token_iv",
            "discord_token_tag",
            "gemini_api_key_encrypted",
            "gemini_api_key_iv",
            "gemini_api_key_tag",
        ],
    ),
    (
        "mcp_servers",
        &[
            "auth_credential_encrypted",
            "auth_credential_iv",
            "auth_credential_tag",
        ],
    ),
    (
        "webhook_endpoints",
        &["secret_encrypted", "secret_iv", "secret_tag"],
    ),
];

/// 既存ユーザー（新環境で登録済み）に旧環境から引き継がない列。セットアップで入れ直した値
/// （表示名・認証・権限・モデル）と行の識別子。
const USER_KEEP_COLUMNS: &[&str] = &[
    "discord_id",
    "username",
    "password_hash",
    "salt",
    "role",
    "gemini_model",
    "created_at",
];

/// 新環境では引き継がない `system_settings` のキー（マイグレーション・バックフィルの実施印）。
const SKIP_SETTING_KEYS: &[&str] = &["schema_version", "v5_grants_backfilled"];

struct Options {
    from: PathBuf,
    into: PathBuf,
    owner: String,
    secretary_name: String,
    apply: bool,
}

/// テーブル単位の集計。
#[derive(Default)]
struct Tally {
    copied: usize,
    remapped: usize,
    orphaned: usize,
    secrets_cleared: usize,
}

#[derive(Default)]
struct Report {
    tables: BTreeMap<String, Tally>,
    skipped_tables: Vec<String>,
    warnings: Vec<String>,
}

/// `salvage-import` の入口（`args` はサブコマンド名を除いた残り）。
pub fn run(args: &[String]) -> Result<(), String> {
    let opts = parse_args(args)?;
    let source = Connection::open_with_flags(&opts.from, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("旧スナップショットを開けません: {e}"))?;
    let mut target = Connection::open_with_flags(&opts.into, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| format!("新 DB を開けません: {e}"))?;
    // 投入順を外部キーの依存順に並べずに済むよう、検査は最後にまとめて行う（トランザクション外で設定）。
    target
        .execute_batch("PRAGMA foreign_keys = OFF; PRAGMA busy_timeout = 5000;")
        .map_err(sql)?;

    let secretary_id = new_bot_id()?;
    let tx = target.transaction().map_err(sql)?;
    let report = import(&source, &tx, &opts, &secretary_id)?;

    let violations = foreign_key_violations(&tx)?;
    print_report(&report, &opts, &secretary_id);
    if !violations.is_empty() {
        for v in &violations {
            eprintln!("  外部キー違反: {v}");
        }
        return Err(format!(
            "外部キー違反が {} 件あるため、何も投入していません。",
            violations.len()
        ));
    }
    if opts.apply {
        tx.commit().map_err(sql)?;
        println!("\n投入を確定しました。");
    } else {
        tx.rollback().map_err(sql)?;
        println!(
            "\nお試し実行のため、何も書き込んでいません。確定するには --apply を付けてください。"
        );
    }
    Ok(())
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut from = None;
    let mut into = None;
    let mut owner = None;
    let mut secretary_name = "秘書".to_owned();
    let mut apply = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{arg} に値がありません"))
        };
        match arg.as_str() {
            "--from" => from = Some(PathBuf::from(value()?)),
            "--into" => into = Some(PathBuf::from(value()?)),
            "--owner" => owner = Some(value()?),
            "--secretary-name" => secretary_name = value()?,
            "--apply" => apply = true,
            other => return Err(format!("不明な引数: {other}\n{USAGE}")),
        }
    }
    Ok(Options {
        from: from.ok_or_else(|| format!("--from が必要です\n{USAGE}"))?,
        into: into.ok_or_else(|| format!("--into が必要です\n{USAGE}"))?,
        owner: owner.ok_or_else(|| format!("--owner が必要です\n{USAGE}"))?,
        secretary_name,
        apply,
    })
}

pub const SNAPSHOT_USAGE: &str =
    "usage: cargo run -p xtask -- salvage-snapshot --from <稼働中の DB> --to <保存先.db>";

/// `salvage-snapshot` の入口。`VACUUM INTO` で、書き込み中の DB からも一貫した 1 ファイルを作る
/// （WAL の内容も含む・読み取りトランザクション 1 本で読む）。保存先は既存ファイルを上書きしない。
pub fn snapshot(args: &[String]) -> Result<(), String> {
    let (mut from, mut to) = (None, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let value = it.next().cloned();
        match arg.as_str() {
            "--from" => from = value.map(PathBuf::from),
            "--to" => to = value.map(PathBuf::from),
            other => return Err(format!("不明な引数: {other}\n{SNAPSHOT_USAGE}")),
        }
    }
    let from = from.ok_or_else(|| format!("--from が必要です\n{SNAPSHOT_USAGE}"))?;
    let to = to.ok_or_else(|| format!("--to が必要です\n{SNAPSHOT_USAGE}"))?;
    if to.exists() {
        return Err(format!(
            "{} は既にあります。別の保存先を指定してください。",
            to.display()
        ));
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    }
    let source = Connection::open_with_flags(&from, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("{} を開けません: {e}", from.display()))?;
    let target = to
        .to_str()
        .ok_or_else(|| format!("保存先のパスが UTF-8 ではありません: {}", to.display()))?;
    source
        .execute("VACUUM INTO ?1", [target])
        .map_err(|e| format!("スナップショットに失敗: {e}"))?;
    let copy = Connection::open_with_flags(&to, OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(sql)?;
    // 1 ファイルで持ち運べるよう、WAL を使わない形にしておく。
    copy.execute_batch("PRAGMA journal_mode = DELETE;")
        .map_err(sql)?;
    let check: String = copy
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(sql)?;
    if check != "ok" {
        return Err(format!("スナップショットの整合性検査に失敗: {check}"));
    }
    drop(copy);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // パスワードハッシュや会話ログを含むため、持ち主だけが読めるようにする。
        std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("権限を設定できません: {e}"))?;
    }
    println!(
        "スナップショットを保存しました: {}（整合性検査 ok）",
        to.display()
    );
    Ok(())
}

pub const USAGE: &str =
    "usage: cargo run -p xtask -- salvage-import --from <旧スナップショット.db> \
--into <新 DB> --owner <管理者の Discord ID> [--secretary-name <名前>] [--apply]";

fn import(
    source: &Connection,
    tx: &Transaction<'_>,
    opts: &Options,
    secretary_id: &str,
) -> Result<Report, String> {
    preflight(source, tx, &opts.owner)?;
    let mut report = Report::default();
    let source_bots: HashSet<String> = query_strings(source, "SELECT id FROM bots")?
        .into_iter()
        .collect();
    let ctx = RowContext {
        owner: &opts.owner,
        secretary_id,
        source_bots: &source_bots,
    };

    import_users(source, tx, &mut report)?;
    import_bots(source, tx, &mut report)?;
    tx.execute(
        "INSERT INTO bots (id, user_id, name, capabilities) VALUES (?1, ?2, ?3, ?4)",
        [
            secretary_id,
            opts.owner.as_str(),
            opts.secretary_name.as_str(),
            SECRETARY_CAPABILITIES,
        ],
    )
    .map_err(sql)?;
    import_system_settings(source, tx, &ctx, &mut report)?;
    copy_table(source, tx, "audit_logs", &ctx, &mut report, &["id"])?;

    let target_tables: HashSet<String> = table_names(tx)?.into_iter().collect();
    for table in table_names(source)? {
        if SPECIAL_TABLES.contains(&table.as_str()) {
            continue;
        }
        if is_skipped(&table) {
            report.skipped_tables.push(table);
            continue;
        }
        if !target_tables.contains(&table) {
            report
                .warnings
                .push(format!("{table}: 新環境に無いテーブルのため投入しません"));
            continue;
        }
        copy_table(source, tx, &table, &ctx, &mut report, &[])?;
    }
    Ok(report)
}

fn is_skipped(table: &str) -> bool {
    SKIP_TABLES.contains(&table) || table.starts_with("message_logs_fts")
}

/// 新環境がセットアップ済みで、まだ何も投入されていないことを確かめる。
fn preflight(source: &Connection, tx: &Transaction<'_>, owner: &str) -> Result<(), String> {
    let exists = |conn: &Connection, sql_text: &str, key: &str| -> Result<bool, String> {
        conn.query_row(sql_text, [key], |_| Ok(()))
            .map(|()| true)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(false),
                e => Err(sql(e)),
            })
    };
    let user_sql = "SELECT 1 FROM users WHERE discord_id = ?1";
    if !exists(source, user_sql, owner)? {
        return Err(format!("旧スナップショットにユーザー {owner} がいません。"));
    }
    if !exists(tx, user_sql, owner)? {
        return Err(format!(
            "新 DB にユーザー {owner} がいません。初期セットアップで同じ Discord ID の管理者を登録してください。"
        ));
    }
    if !exists(tx, "SELECT 1 FROM bots WHERE id = ?1", SYSTEM_BOT)? {
        return Err(
            "新 DB にシステムデフォルト Bot がありません。初期セットアップの「デフォルトBotセットアップ」まで済ませてください。"
                .to_owned(),
        );
    }
    let other_bots: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM bots WHERE id != ?1",
            [SYSTEM_BOT],
            |r| r.get(0),
        )
        .map_err(sql)?;
    if other_bots > 0 {
        return Err(format!(
            "新 DB にシステムデフォルト以外の Bot が {other_bots} 件あります。投入は初期セットアップ直後の DB にだけ行えます。"
        ));
    }
    let mut populated = Vec::new();
    for table in table_names(tx)? {
        if MAY_BE_POPULATED.contains(&table.as_str()) || is_skipped(&table) {
            continue;
        }
        let n: i64 = tx
            .query_row(
                &format!("SELECT COUNT(*) FROM {}", quote(&table)),
                [],
                |r| r.get(0),
            )
            .map_err(sql)?;
        if n > 0 {
            populated.push(format!("{table}({n})"));
        }
    }
    if !populated.is_empty() {
        return Err(format!(
            "新 DB に既にデータがあります: {}。投入は初期セットアップ直後の DB にだけ行えます。",
            populated.join(", ")
        ));
    }
    Ok(())
}

/// 行の付け替えに使う文脈。
struct RowContext<'a> {
    owner: &'a str,
    secretary_id: &'a str,
    source_bots: &'a HashSet<String>,
}

/// 1 行の扱い。
enum RowAction {
    Copy,
    Remap,
    Orphan,
}

impl RowContext<'_> {
    /// `bot_id` 列の値と行の持ち主から、行の扱いを決める。
    fn classify(&self, bot: Option<&str>, user: Option<&str>) -> RowAction {
        match bot {
            Some(SYSTEM_BOT) if user == Some(self.owner) => RowAction::Remap,
            Some(SYSTEM_BOT) | None => RowAction::Copy,
            Some(b) if self.source_bots.contains(b) => RowAction::Copy,
            Some(_) => RowAction::Orphan,
        }
    }
}

/// ユーザー: 新環境に無ければ追加（暗号化列は NULL）、あれば設定列だけ上書きする。
fn import_users(
    source: &Connection,
    tx: &Transaction<'_>,
    report: &mut Report,
) -> Result<(), String> {
    let columns = common_columns(source, tx, "users")?;
    let secrets = secret_columns("users");
    let rows = read_rows(source, "users", &columns)?;
    let existing: HashSet<String> = query_strings(tx, "SELECT discord_id FROM users")?
        .into_iter()
        .collect();
    let tally = report.tables.entry("users".to_owned()).or_default();
    let id_index = index_of(&columns, "discord_id")?;
    for mut row in rows {
        let id = as_text(row.get(id_index)).unwrap_or_default();
        tally.secrets_cleared += clear_secrets(&columns, &mut row, secrets);
        if existing.contains(&id) {
            let updates: Vec<(&String, Value)> = columns
                .iter()
                .zip(row)
                .filter(|(c, _)| {
                    !USER_KEEP_COLUMNS.contains(&c.as_str()) && !secrets.contains(&c.as_str())
                })
                .collect();
            let set = updates
                .iter()
                .enumerate()
                .map(|(i, (c, _))| format!("{} = ?{}", quote(c), i + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let mut values: Vec<Value> = updates.into_iter().map(|(_, v)| v).collect();
            values.push(Value::Text(id));
            tx.execute(
                &format!(
                    "UPDATE users SET {set} WHERE discord_id = ?{}",
                    values.len()
                ),
                params_from_iter(values),
            )
            .map_err(|e| format!("users の更新に失敗: {e}"))?;
        } else {
            insert_row(tx, "users", &columns, row)?;
        }
        tally.copied += 1;
    }
    Ok(())
}

/// Bot: システム Bot 以外を追加する（暗号化列は NULL）。
fn import_bots(
    source: &Connection,
    tx: &Transaction<'_>,
    report: &mut Report,
) -> Result<(), String> {
    let columns = common_columns(source, tx, "bots")?;
    let secrets = secret_columns("bots");
    let id_index = index_of(&columns, "id")?;
    let tally = report.tables.entry("bots".to_owned()).or_default();
    for mut row in read_rows(source, "bots", &columns)? {
        if as_text(row.get(id_index)).as_deref() == Some(SYSTEM_BOT) {
            continue;
        }
        tally.secrets_cleared += clear_secrets(&columns, &mut row, secrets);
        insert_row(tx, "bots", &columns, row)?;
        tally.copied += 1;
    }
    Ok(())
}

/// システム設定: 実施印以外を、新環境に無いキーだけ追加する。会話の区切り
/// （`context_floor:<bot>:<user>`）は付け替え先の Bot に合わせてキーを書き換える。
fn import_system_settings(
    source: &Connection,
    tx: &Transaction<'_>,
    ctx: &RowContext<'_>,
    report: &mut Report,
) -> Result<(), String> {
    let columns = common_columns(source, tx, "system_settings")?;
    let key_index = index_of(&columns, "key")?;
    let tally = report
        .tables
        .entry("system_settings".to_owned())
        .or_default();
    for mut row in read_rows(source, "system_settings", &columns)? {
        let Some(key) = as_text(row.get(key_index)) else {
            continue;
        };
        if SKIP_SETTING_KEYS.contains(&key.as_str()) {
            continue;
        }
        let mut new_key = key.clone();
        if let Some(rest) = key.strip_prefix("context_floor:") {
            if let Some((bot, user)) = rest.split_once(':') {
                match ctx.classify(Some(bot), Some(user)) {
                    RowAction::Remap => {
                        new_key = format!("context_floor:{}:{user}", ctx.secretary_id);
                        tally.remapped += 1;
                    }
                    RowAction::Orphan => {
                        tally.orphaned += 1;
                        continue;
                    }
                    RowAction::Copy => {}
                }
            }
        }
        if let Some(slot) = row.get_mut(key_index) {
            *slot = Value::Text(new_key);
        }
        let n = insert_row_or_ignore(tx, "system_settings", &columns, row)?;
        tally.copied += n;
    }
    Ok(())
}

/// 汎用コピー。`omit` の列は投入しない（新環境で振り直す id など）。
fn copy_table(
    source: &Connection,
    tx: &Transaction<'_>,
    table: &str,
    ctx: &RowContext<'_>,
    report: &mut Report,
    omit: &[&str],
) -> Result<(), String> {
    let columns: Vec<String> = common_columns(source, tx, table)?
        .into_iter()
        .filter(|c| !omit.contains(&c.as_str()))
        .collect();
    if columns.is_empty() {
        return Ok(());
    }
    let secrets = secret_columns(table);
    let bot_index = columns.iter().position(|c| c == "bot_id");
    let user_index = columns
        .iter()
        .position(|c| c == "user_id")
        .or_else(|| columns.iter().position(|c| c == "owner_id"));
    let tally = report.tables.entry(table.to_owned()).or_default();
    let mut unowned_system_rows = 0usize;
    for mut row in read_rows(source, table, &columns)? {
        let bot = bot_index.and_then(|i| as_text(row.get(i)));
        let user = user_index.and_then(|i| as_text(row.get(i)));
        match ctx.classify(bot.as_deref(), user.as_deref()) {
            RowAction::Orphan => {
                tally.orphaned += 1;
                continue;
            }
            RowAction::Remap => {
                if let Some(slot) = bot_index.and_then(|i| row.get_mut(i)) {
                    *slot = Value::Text(ctx.secretary_id.to_owned());
                }
                tally.remapped += 1;
            }
            RowAction::Copy => {
                if bot.as_deref() == Some(SYSTEM_BOT) {
                    unowned_system_rows += 1;
                }
            }
        }
        tally.secrets_cleared += clear_secrets(&columns, &mut row, secrets);
        insert_row(tx, table, &columns, row)?;
        tally.copied += 1;
    }
    if unowned_system_rows > 0 {
        report.warnings.push(format!(
            "{table}: --owner 以外のユーザーのシステム Bot 宛ての行 {unowned_system_rows} 件は、システム Bot のまま投入しました"
        ));
    }
    Ok(())
}

fn secret_columns(table: &str) -> &'static [&'static str] {
    SECRET_COLUMNS
        .iter()
        .find(|(t, _)| *t == table)
        .map_or(&[], |(_, cols)| *cols)
}

/// 暗号化列を NULL にし、値があった列の数を返す。
fn clear_secrets(columns: &[String], row: &mut [Value], secrets: &[&str]) -> usize {
    let mut cleared = 0;
    for (column, value) in columns.iter().zip(row.iter_mut()) {
        if secrets.contains(&column.as_str()) && *value != Value::Null {
            *value = Value::Null;
            cleared += 1;
        }
    }
    usize::from(cleared > 0)
}

fn insert_row(
    tx: &Transaction<'_>,
    table: &str,
    columns: &[String],
    row: Vec<Value>,
) -> Result<(), String> {
    tx.execute(&insert_sql("INSERT", table, columns), params_from_iter(row))
        .map(|_| ())
        .map_err(|e| format!("{table} への投入に失敗: {e}"))
}

fn insert_row_or_ignore(
    tx: &Transaction<'_>,
    table: &str,
    columns: &[String],
    row: Vec<Value>,
) -> Result<usize, String> {
    tx.execute(
        &insert_sql("INSERT OR IGNORE", table, columns),
        params_from_iter(row),
    )
    .map_err(|e| format!("{table} への投入に失敗: {e}"))
}

fn insert_sql(verb: &str, table: &str, columns: &[String]) -> String {
    let names = columns
        .iter()
        .map(|c| quote(c))
        .collect::<Vec<_>>()
        .join(", ");
    let marks = (1..=columns.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{verb} INTO {} ({names}) VALUES ({marks})", quote(table))
}

fn read_rows(
    conn: &Connection,
    table: &str,
    columns: &[String],
) -> Result<Vec<Vec<Value>>, String> {
    let names = columns
        .iter()
        .map(|c| quote(c))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn
        .prepare(&format!("SELECT {names} FROM {}", quote(table)))
        .map_err(sql)?;
    let rows = stmt
        .query_map([], |r| {
            (0..columns.len())
                .map(|i| r.get::<_, Value>(i))
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(sql)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sql)
}

/// 両方の DB にある列（旧環境の列順）。スキーマが変わっても共通の列だけ移す。
fn common_columns(
    source: &Connection,
    target: &Connection,
    table: &str,
) -> Result<Vec<String>, String> {
    let target_columns: HashSet<String> = column_names(target, table)?.into_iter().collect();
    Ok(column_names(source, table)?
        .into_iter()
        .filter(|c| target_columns.contains(c))
        .collect())
}

fn column_names(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", quote(table)))
        .map_err(sql)?;
    let names = stmt.query_map([], |r| r.get::<_, String>(1)).map_err(sql)?;
    names.collect::<Result<Vec<_>, _>>().map_err(sql)
}

fn table_names(conn: &Connection) -> Result<Vec<String>, String> {
    query_strings(
        conn,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
}

fn query_strings(conn: &Connection, sql_text: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(sql_text).map_err(sql)?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(sql)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sql)
}

fn foreign_key_violations(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare("PRAGMA foreign_key_check").map_err(sql)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(format!(
                "{} rowid={:?} → {}",
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, String>(2)?
            ))
        })
        .map_err(sql)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sql)
}

fn index_of(columns: &[String], name: &str) -> Result<usize, String> {
    columns
        .iter()
        .position(|c| c == name)
        .ok_or_else(|| format!("列 {name} がありません"))
}

fn as_text(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::Text(s)) => Some(s.clone()),
        _ => None,
    }
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn sql(e: rusqlite::Error) -> String {
    e.to_string()
}

/// 管理画面の Bot 作成と同じ形式の id（`bot_<UUID v4>`）。
fn new_bot_id() -> Result<String, String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(|e| format!("乱数を取得できません: {e}"))?;
    if let Some(v) = b.get_mut(6) {
        *v = (*v & 0x0f) | 0x40;
    }
    if let Some(v) = b.get_mut(8) {
        *v = (*v & 0x3f) | 0x80;
    }
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "bot_{}-{}-{}-{}-{}",
        hex.get(0..8).unwrap_or_default(),
        hex.get(8..12).unwrap_or_default(),
        hex.get(12..16).unwrap_or_default(),
        hex.get(16..20).unwrap_or_default(),
        hex.get(20..32).unwrap_or_default()
    ))
}

fn print_report(report: &Report, opts: &Options, secretary_id: &str) {
    println!(
        "投入元: {}\n投入先: {}\n新しい秘書 Bot: {} ({secretary_id}) — 持ち主 {}",
        opts.from.display(),
        opts.into.display(),
        opts.secretary_name,
        opts.owner
    );
    println!(
        "\n{:<28} {:>7} {:>9} {:>9} {:>9}",
        "テーブル", "投入", "付け替え", "残骸除外", "暗号削除"
    );
    for (table, t) in &report.tables {
        if t.copied + t.orphaned == 0 {
            continue;
        }
        println!(
            "{table:<28} {:>7} {:>9} {:>9} {:>9}",
            t.copied, t.remapped, t.orphaned, t.secrets_cleared
        );
    }
    if !report.skipped_tables.is_empty() {
        println!("\n投入しないテーブル: {}", report.skipped_tables.join(", "));
    }
    for w in &report.warnings {
        println!("注意: {w}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_id_has_the_uuid_v4_shape() {
        let id = new_bot_id().unwrap_or_default();
        let uuid = id.strip_prefix("bot_").unwrap_or_default();
        assert_eq!(uuid.len(), 36, "{id}");
        assert_eq!(uuid.chars().nth(14), Some('4'), "{id}");
    }

    #[test]
    fn rows_are_classified_by_bot_and_owner() {
        let bots: HashSet<String> = ["b1".to_owned()].into_iter().collect();
        let ctx = RowContext {
            owner: "me",
            secretary_id: "new",
            source_bots: &bots,
        };
        assert!(matches!(
            ctx.classify(Some(SYSTEM_BOT), Some("me")),
            RowAction::Remap
        ));
        assert!(matches!(
            ctx.classify(Some(SYSTEM_BOT), Some("other")),
            RowAction::Copy
        ));
        assert!(matches!(
            ctx.classify(Some("b1"), Some("me")),
            RowAction::Copy
        ));
        assert!(matches!(
            ctx.classify(Some("deleted"), Some("me")),
            RowAction::Orphan
        ));
        assert!(matches!(ctx.classify(None, Some("me")), RowAction::Copy));
    }
}
