//! ログイン（OAuth デバイスフロー）進行 UI。
//!
//! 仕様: client_design.md §8 / requirements.md FR-13。
//! 未ログイン時に「ブラウザでログイン」を提示し、`device/code` 取得後は
//! user_code と verification_uri を表示してポーリング中スピナーを出す。
//!
//! 本フェーズは骨格。実際のデバイスフロー実行は `net::auth::run_device_flow`
//! を Net スレッドで回し、進捗を [`crate::app::LoginUiState`] に反映する。

use crate::app::{AppState, LoginUiState};

/// ログインビューを描画する。
pub fn view(state: &mut AppState, ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.heading("Yuuka Desktop");
        ui.add_space(8.0);

        match &state.login {
            LoginUiState::LoggedOut => {
                ui.label("ログインが必要です。");
                if ui.button("ブラウザでログイン").clicked() {
                    // 実配線（Phase 2 後半）: Net スレッドへデバイスフロー開始を要求。
                    // ここでは状態だけ Starting に進めておく。
                    state.login = LoginUiState::Starting;
                    state.request_login_start = true;
                }
            }
            LoginUiState::Starting => {
                ui.spinner();
                ui.label("デバイスコードを取得しています…");
            }
            LoginUiState::AwaitingApproval {
                user_code,
                verification_uri,
            } => {
                ui.label("ブラウザで以下のコードを承認してください:");
                ui.add_space(4.0);
                ui.monospace(egui::RichText::new(user_code).size(24.0).strong());
                ui.add_space(8.0);
                if ui.link(verification_uri.clone()).clicked() {
                    // ブラウザ起動は net::auth 側でも行うが、再オープン導線も置く。
                    let _ = open::that_detached(verification_uri);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("承認待ち…");
                });
            }
            LoginUiState::Connecting => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("接続中…");
                });
            }
            LoginUiState::Error(msg) => {
                ui.colored_label(egui::Color32::LIGHT_RED, msg);
                if ui.button("再試行").clicked() {
                    state.login = LoginUiState::LoggedOut;
                }
            }
        }
    });
}
