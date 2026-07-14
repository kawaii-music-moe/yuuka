//! yuuka-briefing — 朝ブリーフィング/日報/週報の配信設定ツール（秘書経路）。Node
//! `briefingFunctions` の DB 設定系を移植する。
//!
//! 移植済み: configureReport（日報/週報の配信設定を部分更新）・getBriefingConfig（現在設定の読取）。
//! **残（後続）**: configureBriefing（SSRF ガード + ニュースフィード配列の部分更新）・runBriefingNow
//! （briefing サービス本体＝天気/RSS の HTTP 取得に依存）。

mod repo;
mod tools;

pub use tools::tools;
