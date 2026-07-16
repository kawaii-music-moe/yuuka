//! yuuka-google — Google 連携の共有層（複数アカウント repo + OAuth/Calendar/Drive シーム）。
//!
//! `yuuka-settings`（`/api/status`・`/api/settings/google/*`・`/api/settings/calendars`・
//! `/api/settings/backup/trigger`）と `yuuka-integrated`（`/api/integrated/google/*`・`grants/google`）
//! が共有する。DB 効果は常に完全に働き、外部 HTTP はポート越しに [`NullGoogleOAuth`]/[`NullCalendar`]/
//! [`NullBackup`] へ縮退する。DAG: `google → web, db, core`。

pub mod http;
pub mod ports;
pub mod repo;

pub use http::GoogleHttpClient;
pub use ports::{
    BackupPort, CalendarPort, CalendarSummary, GoogleError, GoogleOAuthPort, GoogleTokens,
    NullBackup, NullCalendar, NullGoogleOAuth, OAuthStateStore,
};
pub use repo::{AccountOwner, BotGoogleMode, GoogleAccountSafe, PrimaryAccount};
