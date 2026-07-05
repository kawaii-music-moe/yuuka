//! yuuka-credential — credential ドメイン（T1 fan-out）。**参照実装は yuuka-todo**（repo/dto/routes の縦スライス）。
//!
//! Phase 1 スケルトン: `routes()`/`export_bindings()` は空。fan-out で自クレート内に
//! repo.rs（`ScopedRepo`・全メソッド `&UserScope`・共通 `yuuka_web::resolve_scope` 使用）、
//! dto.rs（ts-rs・内部列を持たせない構造的フェイルクローズ）、routes.rs を埋める。
//! 共有集約点（root Cargo.toml members/deps・supervisor build_app・xtask export）は配線済み。

use std::path::Path;

use axum::Router;
use yuuka_web::AppState;

/// credential ドメインのルータ（supervisor が共通レイヤ配下にマージ）。fan-out で route を追加。
pub fn routes() -> Router<AppState> {
    Router::new()
}

/// 本ドメインの wire DTO を生成する（xtask gen-types が呼ぶ）。fan-out で DTO export を追加。
///
/// # Errors
/// ts-rs のシリアライズ／書き込み失敗時 [`ts_rs::ExportError`]。
pub fn export_bindings(_base_dir: &Path) -> Result<(), ts_rs::ExportError> {
    Ok(())
}
