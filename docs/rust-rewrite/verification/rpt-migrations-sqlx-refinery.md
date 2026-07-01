I have more than enough understanding of the codebase's naive DROP-and-recreate pattern (lines 12, 880-894 show the SCHEMA_VERSION compare and the `DROP TABLE` legacy-rebuild path). I have all primary-source data verified. Let me compile the findings.

---

# Rust Database Migration Tooling Research (verified July 2026)

All version numbers and factual claims below were verified against fetched primary sources (crates.io API, docs.rs, official GitHub). URLs cited at the end. Confidence levels: 高 = corroborated by a primary source page I fetched; 中 = primary source but one page / slightly indirect; 低 = inferred.

## Context: what is being replaced

The current codebase (`/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/migrations.ts`) uses a string `SCHEMA_VERSION = "17"` (line 12) compared against a `system_settings.schema_version` row. When the legacy-v1 schema is detected (lines 880-894), it runs `PRAGMA foreign_keys = OFF` and `DROP TABLE IF EXISTS` across `LEGACY_TABLES` — i.e. **data loss on version mismatch**. Note: the newer per-migration functions (v3-v17) are already mostly idempotent additive/rebuild steps; the destructive path is specifically the legacy-v1 branch. The recommendation below replaces the whole `SCHEMA_VERSION`-compare mechanism with a real forward-only migration ledger.

---

## 1. sqlx migrate + sqlx-cli

**Versions (高 — crates.io API):**
- `sqlx` latest stable: **0.9.0**, released **2026-05-21**. Supports PostgreSQL, MySQL, SQLite.
- `sqlx-cli` latest stable: **0.9.0**, released **2026-05-21** (versioned in lockstep with sqlx). Prior line: 0.8.6 (2025-05-19).
- Actively maintained: yes (0.9.0 is ~6 weeks old as of this research; a 0.9.0-alpha.1 preceded it 2025-10-15).

**How SQLite migrations work (高 — sqlx-cli README + docs.rs):**
- Migrations live in a `migrations/` directory. `sqlx migrate add <name>` creates `migrations/<timestamp>-<name>.sql`. The timestamp gives a monotonic, sortable version.
- `sqlx migrate run` "compares the migration history of the running database against the `migrations/` folder and runs any scripts that are still pending."
- Applied migrations are tracked in a `_sqlx_migrations` table (default name; configurable via `sqlx.toml` or `dangerous_set_table_name()` — docs warn changing it in production risks re-running everything).

**Forward-only vs reversible (高 — README):**
- Default is forward-only: `sqlx migrate add <name>` → single `.sql` file.
- Reversible mode: `sqlx migrate add -r <name>` produces paired `…​.up.sql` / `…​.down.sql`. Once you start reversible, subsequent migrations follow that pattern. `sqlx migrate revert` undoes the latest applied migration.

**Checksum / tamper detection (高 — Migrator docs.rs):**
- `Migrator::run()` "executes pending migrations and validates previously applied ones" and is designed to "detect accidental changes in previously-applied migrations." Each migration's checksum is stored in `_sqlx_migrations`; if a file that was already applied is edited, `run()` errors out instead of silently proceeding. Already-applied (unchanged) migrations are skipped. There is a `sqlx.toml` option to normalize whitespace/line-endings in the hash for cross-platform stability.

**Embedding in the binary + run at startup (高 — docs.rs macro.migrate):**
- Confirmed. `sqlx::migrate!("db/migrations")` (or `sqlx::migrate!()` defaulting to `./migrations`) expands to a static `Migrator` embedding every migration file via `include_str!()` at compile time. Canonical runtime call:
  ```rust
  static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();
  MIGRATOR.run(&pool).await?;                 // or sqlx::migrate!().run(&pool).await?;
  ```
- Because file changes don't auto-trigger rebuilds, the docs recommend a `build.rs` with `println!("cargo:rerun-if-changed=migrations");` (or nightly `--cfg sqlx_macros_unstable`).

**SQLite-specific limitations (中 — sqlx issue #2085, #3527, general SQLite docs):**
- SQLite's `ALTER TABLE` is limited (ADD COLUMN, RENAME TABLE/COLUMN, DROP COLUMN only — no arbitrary type/constraint change). Complex changes require the create-new-table → copy → drop → rename dance (exactly the pattern the codebase already uses in `migrateToBotScopedData`).
- **Important gotcha:** sqlx wraps each migration in a transaction by default. `PRAGMA foreign_keys = OFF` cannot take effect inside a transaction, so the "rebuild table" pattern that needs FKs off is problematic. Issue #2085 (temporarily disabling FK enforcement in SQLite migrations) is **open** as of this research.
- Escape hatch: a `-- no-transaction` directive as the **first line** of a migration file runs it outside a transaction (added originally for Postgres `CREATE INDEX CONCURRENTLY`, but it applies generally). Caveats: for SQLite you'd then lose atomic rollback of that migration, and per issue #3527 the `no-transaction` directive is **not honored for down migrations**. So a safer SQLite idiom for table rebuilds is `PRAGMA foreign_keys=OFF` won't work inside the tx — instead use `PRAGMA legacy_alter_table` / `PRAGMA defer_foreign_keys=ON` (settable inside a tx) or a `-- no-transaction` migration.

---

## 2. refinery

**Versions (高 — crates.io API + GitHub releases):**
- Latest stable: **0.9.2**, released **2026-06-10** (the most recent of the three tools — ~3 weeks old). Preceding: 0.9.1 (2026-04-15), 0.9.0 (2025-10-10).
- Actively maintained in 2026: **yes.** 0.9.2 added rustls support for tokio-postgres and made TLS opt-in; 0.9.1 fixed nested-directory scan warnings. Steady cadence with dependency bumps.

**rusqlite / SQLite support (高 — README + release notes):**
- Yes. Supported drivers: `postgres`, `tokio-postgres`, `mysql`, `mysql_async`, **`rusqlite` (SQLite)**, and `tiberius` (SQL Server). Other drivers (e.g. sqlx) can be wired via a `Config` implementation.
- **0.9.2 supports rusqlite 0.39.x**; 0.9.1 supported 0.38.x. (So pin your `rusqlite` to a version refinery's release supports.)

**How it works (高 — docs.rs + README):**
- Migration files/modules named `[U|V]{version}__{name}.{sql|rs}`, e.g. `V1__initial_schema.sql`.
  - `V` = strictly **versioned** (contiguous — each new migration must have a strictly greater version; standard case).
  - `U` = **unversioned** (non-contiguous — more flexible ordering so multiple devs can merge migrations out of order). Note: refinery's `U` means *unversioned*, **not** "undo/down" — this differs from Flyway where `U` = undo. Verify per project. (中 — corroborated by refinery README wording, but the term collides with Flyway's convention.)
- `embed_migrations!("./migrations")` compiles the migrations into the binary and generates a `runner()` returning a `Runner`. Startup call:
  ```rust
  mod embedded { refinery::embed_migrations!("./migrations"); }
  let mut conn = rusqlite::Connection::open("app.db")?;
  embedded::migrations::runner().run(&mut conn)?;   // returns a Report or Error
  ```
- Applied migrations (version + checksum) are tracked in the **`refinery_schema_history`** table, used to detect divergent/missing/altered migrations.
- **Forward-only:** refinery follows Flyway's older philosophy — no built-in rollback. "To undo/rollback a migration, you have to generate a new one and write specifically what you want to undo."
- Optional `int8-versions` feature raises version numbers from `i32` to `i64`.

**refinery vs sqlx migrate (中 — synthesized from both primary sources):**

| | sqlx migrate | refinery |
|---|---|---|
| Version scheme | timestamp prefix `<ts>-name.sql` | integer `V{n}__name.sql` |
| History table | `_sqlx_migrations` | `refinery_schema_history` |
| Checksums / tamper detection | yes | yes |
| Reversible/down | yes (`-r`, `.down.sql`, `revert`) | no (forward-only by design) |
| Embed in binary | `sqlx::migrate!()` | `embed_migrations!()` |
| SQLite driver | sqlx's own async SQLite | `rusqlite` (sync) — also mysql/pg/mssql |
| Async | async-native | sync (works with sync rusqlite) |
| CLI | `sqlx-cli` (rich; also does compile-time query checking) | `refinery_cli` |
| Best fit | you're already on sqlx / want async + down-migrations | you're on **rusqlite** (sync) — natural fit |

**Also worth noting (中 — docs.rs) — `rusqlite_migration`** (latest **2.6.0**): a lighter alternative that stores version in SQLite's `user_version` PRAGMA (no history table, very fast), supports downward migrations, and `from-directory` loading. Good if you want minimal ceremony on pure rusqlite, but it has no per-migration checksum ledger (weaker tamper detection than refinery/sqlx).

---

## 3. Best-practice pattern to replace the naive SCHEMA_VERSION+DROP (2026 idiom)

**Core principle:** replace "one version string → DROP & recreate" with an **append-only ledger of immutable, forward-only migrations**. Each migration is a numbered/timestamped file that is applied exactly once, recorded with a checksum, and **never edited after being applied**. To change something, you add a *new* migration. No migration ever drops user data unless that is its explicit, intentional purpose.

**Canonical setup — refinery on rusqlite (recommended if the Rust rewrite uses sync rusqlite):**
1. Add `refinery = { version = "0.9.2", features = ["rusqlite"] }` and a matching `rusqlite`.
2. Create `migrations/` with `V{n}__{name}.sql` files.
3. At startup:
   ```rust
   mod embedded { refinery::embed_migrations!("./migrations"); }
   let mut conn = rusqlite::Connection::open(path)?;
   embedded::migrations::runner().run(&mut conn)?; // idempotent; applies only pending
   ```
4. refinery records each in `refinery_schema_history` with a checksum; re-running is a no-op; editing an applied file is detected and errors.

**Canonical setup — sqlx migrate (if the rewrite is async and uses sqlx's SQLite):**
1. `sqlx = { version = "0.9", features = ["sqlite", "runtime-tokio"] }`, `sqlx-cli` as a dev tool.
2. `sqlx migrate add <name>` → `migrations/<ts>-<name>.sql`.
3. `build.rs`: `println!("cargo:rerun-if-changed=migrations");`
4. Startup: `sqlx::migrate!().run(&pool).await?;`

**Baselining an existing populated database (critical — the codebase already has live data):**
The existing DB has real tables but no migration ledger. You must not let the tool think migration V1 is "pending" and try to `CREATE TABLE` over existing data. Two standard approaches:

- **refinery:** No first-class "baseline as applied" flag, so the idiom is to make **V1 idempotent** — write `V1__baseline.sql` as the current full schema using `CREATE TABLE IF NOT EXISTS …` (and `CREATE INDEX IF NOT EXISTS`). On an existing DB, V1 runs harmlessly (everything already exists) and gets recorded in `refinery_schema_history`; on a fresh DB it actually creates the schema. All *future* changes are plain `V2__…`, `V3__…` additive migrations. (中 — standard community idiom; refinery's own docs don't provide a dedicated baseline command.)

- **sqlx:** Same idempotent-baseline approach works; additionally `sqlx-cli`/`Migrator` exposes `skip(target)` / `migrate … --skip` semantics to **mark a migration as applied without executing its SQL** (`Migrator::skip` in docs.rs). So you can create `<ts>-baseline.sql` capturing today's schema and mark it applied on existing DBs, while it runs normally on fresh ones. (中 — `skip`/`run_to` methods confirmed on docs.rs Migrator page.)

**Migration path from the current TS system specifically:**
- Freeze the current schema (v17) as the **baseline migration #1** using `IF NOT EXISTS` DDL — this makes it safe on both existing and fresh DBs.
- Convert each existing idempotent step (v3-v17 in `migrations.ts`) into either (a) part of the baseline if it's already reflected in current table definitions, or (b) individual forward migrations if you want to preserve the historical sequence. Given they're already `IF NOT EXISTS`/`ADD COLUMN` guarded, folding them into a single baseline is simplest.
- **Drop the destructive legacy-v1 branch entirely** (lines 880-894) — that DROP path is the data-loss hazard. If any deployed DB is still on pre-v2 schema, handle it with a one-time explicit, reviewed migration, never an automatic version-mismatch DROP.
- Keep SQLite table rebuilds (the create-copy-drop-rename pattern) inside migrations, but be mindful of the transaction/FK gotcha from §1 (use `PRAGMA defer_foreign_keys=ON` inside the tx, or a `-- no-transaction` migration for sqlx).

---

## Recommendation summary

- If the Rust rewrite uses **sync `rusqlite`** (which the codebase comments suggest for the "Rust synapse engine" reading the same SQLite file): **refinery 0.9.2** is the natural fit — forward-only, checksummed, embeds in the binary, first-class rusqlite. Watch the rusqlite version pin (0.39.x for refinery 0.9.2).
- If the rewrite standardizes on **async sqlx**: **sqlx 0.9.0 migrate** gives you the same guarantees plus optional down-migrations and `skip`-based baselining, at the cost of using sqlx's async SQLite driver.
- Either way, the key change is conceptual: **immutable, append-only, checksum-verified forward migrations with an idempotent baseline; never DROP on version mismatch.**

---

## Sources fetched
- sqlx-cli versions/dates: https://crates.io/api/v1/crates/sqlx-cli
- sqlx versions/dates: https://crates.io/api/v1/crates/sqlx
- refinery versions/dates: https://crates.io/api/v1/crates/refinery
- sqlx::migrate! macro: https://docs.rs/sqlx/latest/sqlx/macro.migrate.html
- sqlx Migrator (checksums, run/skip/undo, _sqlx_migrations): https://docs.rs/sqlx/latest/sqlx/migrate/struct.Migrator.html
- sqlx-cli README (dirs, timestamps, -r reversible, revert): https://github.com/launchbadge/sqlx/blob/main/sqlx-cli/README.md
- refinery docs.rs (embed_migrations!, V{n}__, refinery_schema_history, drivers): https://docs.rs/refinery/latest/refinery/
- refinery GitHub README (naming, forward-only, driver list, example): https://github.com/rust-db/refinery
- refinery releases (0.9.0/0.9.1/0.9.2 dates, rusqlite 0.39.x): https://github.com/rust-db/refinery/releases
- sqlx SQLite FK/transaction limitation (open issue): https://github.com/launchbadge/sqlx/issues/2085
- sqlx `no-transaction` not applied to down migrations: https://github.com/launchbadge/sqlx/issues/3527
- rusqlite_migration (alternative, user_version, 2.6.0): https://docs.rs/rusqlite_migration/latest/rusqlite_migration/

**Confidence:** Versions/dates and core mechanics (embedding, history tables, checksums, forward-only vs reversible) are **高** — read directly off crates.io API and docs.rs/GitHub. The SQLite transaction/FK caveats and the baseline idioms are **中** — grounded in primary issue trackers and docs, but the "idempotent baseline" pattern is a community convention rather than a documented one-command feature. The refinery `U` = unversioned (not undo) distinction is **中** — verify against your team's expectations since it collides with Flyway's meaning.

One note: the environment's Gmail/Calendar/Drive MCP connectors require authorization (via claude.ai connector settings or an interactive `claude mcp`/`/mcp` session) and were unavailable here, but they weren't needed for this research.