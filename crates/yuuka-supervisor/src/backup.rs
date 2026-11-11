//! `yuuka backup` — サーバー全体の DB の定期バックアップ（issue #241）。
//!
//! 稼働中の DB から [`yuuka_db::snapshot`]（`VACUUM INTO`）で一貫した 1 ファイルを
//! `<保存先>/yuuka-YYYYMMDD-HHMMSS.db` に書き、古い世代を `--keep` 件だけ残して消す。サービスを
//! 止める必要は無い。systemd timer から回す（`deploy/systemd/yuuka-backup.timer.example`）。
//!
//! 暗号化された値（Gemini キー・Discord トークン・Google のリフレッシュトークン等）は、DB と一緒に
//! `YUUKA_ENCRYPTION_SECRET`（`deploy/prod/secret.key`）を保管しないと戻しても読めない。この
//! コマンドは DB だけを扱う（鍵は別の場所に控える前提・`docs/guide/deployment.md`）。

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};

/// `yuuka backup` の使い方。
pub const USAGE: &str =
    "usage: yuuka backup [--to <保存先ディレクトリ>] [--keep <残す世代数>] [--db <DB のパス>]\n\
  --to    既定は DB と同じ場所の backups/（例: data/backups）\n\
  --keep  新しい順に残す数（既定 14・1 以上）。今回の分を含む\n\
  --db    既定は config.yaml / 環境変数の DB_PATH";

/// 既定で残す世代数。毎日 1 回なら 2 週間分。
pub const DEFAULT_KEEP: usize = 14;

/// バックアップのファイル名の接頭辞と拡張子。掃除の対象はこの形のファイルだけ。
const FILE_PREFIX: &str = "yuuka-";
const FILE_SUFFIX: &str = ".db";

/// 解決済みの実行条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// 元の DB。
    pub db: PathBuf,
    /// 保存先ディレクトリ。
    pub dir: PathBuf,
    /// 残す世代数（今回の分を含む）。
    pub keep: usize,
}

/// 実行結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// 書いたファイル。
    pub written: PathBuf,
    /// 消した古い世代（新しい順ではなく、消した順）。
    pub removed: Vec<PathBuf>,
}

/// 引数を解釈する。`--db` が無ければ `default_db` を使う。
///
/// # Errors
/// 不明な引数・値の欠落・`--keep` が数でないか 0 のとき。
pub fn parse_args(args: &[String], default_db: PathBuf) -> Result<Options, String> {
    let (mut db, mut dir, mut keep) = (None, None, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--db" => db = Some(PathBuf::from(next_value(&mut it, arg)?)),
            "--to" => dir = Some(PathBuf::from(next_value(&mut it, arg)?)),
            "--keep" => {
                let raw = next_value(&mut it, arg)?;
                let n: usize = raw
                    .trim()
                    .parse()
                    .map_err(|_| format!("--keep は 1 以上の整数で指定してください: {raw}"))?;
                if n == 0 {
                    return Err("--keep は 1 以上で指定してください。".to_owned());
                }
                keep = Some(n);
            }
            other => return Err(format!("不明な引数: {other}\n{USAGE}")),
        }
    }
    let db = db.unwrap_or(default_db);
    let dir = dir.unwrap_or_else(|| default_dir(&db));
    Ok(Options {
        db,
        dir,
        keep: keep.unwrap_or(DEFAULT_KEEP),
    })
}

/// 既定の保存先（DB と同じディレクトリの `backups/`）。
fn default_dir(db: &Path) -> PathBuf {
    db.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("backups"), |p| p.join("backups"))
}

fn next_value<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<String, String> {
    it.next()
        .cloned()
        .ok_or_else(|| format!("{flag} に値がありません\n{USAGE}"))
}

/// 今回のファイル名（`yuuka-YYYYMMDD-HHMMSS.db`）。名前の順が時刻の順になる。
#[must_use]
pub fn file_name(now: DateTime<Local>) -> String {
    format!("{FILE_PREFIX}{}{FILE_SUFFIX}", now.format("%Y%m%d-%H%M%S"))
}

/// スナップショットを 1 つ書き、古い世代を消す。
///
/// # Errors
/// スナップショットに失敗したとき（古い世代は消さない）。掃除の失敗は警告にとどめ、エラーにしない。
pub fn run(opts: &Options, now: DateTime<Local>) -> Result<Report, String> {
    let written = opts.dir.join(file_name(now));
    yuuka_db::snapshot(&opts.db, &written).map_err(|e| e.to_string())?;
    let removed = prune(&opts.dir, opts.keep);
    Ok(Report { written, removed })
}

/// `dir` のバックアップを新しい順に `keep` 件残し、それより古いものを消す。消せたものを返す。
fn prune(dir: &Path, keep: usize) -> Vec<PathBuf> {
    let mut names: Vec<String> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.starts_with(FILE_PREFIX) && n.ends_with(FILE_SUFFIX))
            .collect(),
        Err(_) => return Vec::new(),
    };
    // 名前＝時刻なので、辞書順の降順が新しい順。
    names.sort_unstable_by(|a, b| b.cmp(a));
    names
        .into_iter()
        .skip(keep)
        .map(|n| dir.join(n))
        .filter(|path| match std::fs::remove_file(path) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("古いバックアップを消せません: {}: {e}", path.display());
                false
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn defaults_follow_the_db_location() {
        let opts = parse_args(&[], PathBuf::from("data/yuuka.db")).unwrap();
        assert_eq!(opts.db, PathBuf::from("data/yuuka.db"));
        assert_eq!(opts.dir, PathBuf::from("data/backups"));
        assert_eq!(opts.keep, DEFAULT_KEEP);
    }

    #[test]
    fn flags_override_defaults() {
        let opts = parse_args(
            &args(&["--to", "/tmp/b", "--keep", "3", "--db", "/srv/y.db"]),
            PathBuf::from("data/yuuka.db"),
        )
        .unwrap();
        assert_eq!(opts.db, PathBuf::from("/srv/y.db"));
        assert_eq!(opts.dir, PathBuf::from("/tmp/b"));
        assert_eq!(opts.keep, 3);
    }

    #[test]
    fn rejects_zero_keep_unknown_flags_and_missing_values() {
        let db = PathBuf::from("data/yuuka.db");
        assert!(parse_args(&args(&["--keep", "0"]), db.clone()).is_err());
        assert!(parse_args(&args(&["--keep", "x"]), db.clone()).is_err());
        assert!(parse_args(&args(&["--bogus"]), db.clone()).is_err());
        assert!(parse_args(&args(&["--to"]), db).is_err());
    }

    #[test]
    fn file_name_sorts_by_time() {
        let early = Local.with_ymd_and_hms(2026, 11, 11, 4, 0, 0).unwrap();
        let late = Local.with_ymd_and_hms(2026, 11, 12, 4, 0, 0).unwrap();
        assert_eq!(file_name(early), "yuuka-20261111-040000.db");
        assert!(file_name(early) < file_name(late));
    }

    #[test]
    fn run_writes_a_copy_and_keeps_only_the_newest_generations() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("yuuka.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE t (v TEXT); INSERT INTO t VALUES ('x');")
            .unwrap();
        drop(conn);

        let dir = tmp.path().join("backups");
        std::fs::create_dir_all(&dir).unwrap();
        // 既存の古い世代（掃除の対象）と、形の違うファイル（対象外）。
        for name in ["yuuka-20260101-000000.db", "yuuka-20260102-000000.db"] {
            std::fs::write(dir.join(name), b"old").unwrap();
        }
        std::fs::write(dir.join("notes.txt"), b"keep").unwrap();

        let opts = Options {
            db: db.clone(),
            dir: dir.clone(),
            keep: 2,
        };
        let now = Local.with_ymd_and_hms(2026, 11, 11, 4, 0, 0).unwrap();
        let report = run(&opts, now).unwrap();

        assert_eq!(report.written, dir.join("yuuka-20261111-040000.db"));
        assert_eq!(report.removed, vec![dir.join("yuuka-20260101-000000.db")]);
        assert!(dir.join("yuuka-20260102-000000.db").exists());
        assert!(dir.join("notes.txt").exists());

        let copy = rusqlite::Connection::open_with_flags(
            &report.written,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let v: String = copy.query_row("SELECT v FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "x");
    }

    #[test]
    fn run_fails_when_the_source_is_missing_and_prunes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("backups");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("yuuka-20260101-000000.db"), b"old").unwrap();
        let opts = Options {
            db: tmp.path().join("missing.db"),
            dir: dir.clone(),
            keep: 1,
        };
        assert!(run(&opts, Local::now()).is_err());
        assert!(dir.join("yuuka-20260101-000000.db").exists());
    }
}
