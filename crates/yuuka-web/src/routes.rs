//! ルートハンドラ（Phase 1 増分1: `/api/me` のみ。以降の増分で拡張）。

use axum::extract::State;
use axum::Json;
use yuuka_types::{Envelope, MeData};

use crate::auth::AuthenticatedUser;
use crate::state::AppState;

/// `GET /api/me`（auth: user）。認証済みユーザーと規約 URL を返す。
///
/// 既存 Node の `{ success:true, user, privacyPolicyUrl, termsUrl }` に一致
/// （`Envelope<MeData>` の flatten）。未認証は extractor が 401 を返す。
pub async fn me(user: AuthenticatedUser, State(state): State<AppState>) -> Json<Envelope<MeData>> {
    Json(Envelope::ok(MeData {
        user: user.0,
        privacy_policy_url: state.config.privacy_policy_url.clone(),
        terms_url: state.config.terms_url.clone(),
    }))
}
