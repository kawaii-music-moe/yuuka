//! 最小設定ビュー。
//!
//! 仕様: client_design.md §10 / requirements.md FR-14。
//! ホットキー・オーバーレイ不透明度・前回 Bot・録音自動送信を編集する。
//! 機微（トークン）は扱わない。保存は `Settings::save`（app.rs が呼ぶ）。

use crate::app::AppState;

/// 設定ビューを描画する。変更があれば `true`（呼び出し側が保存する合図）。
pub fn view(state: &mut AppState, ui: &mut egui::Ui) -> bool {
    let mut changed = false;

    // 管理画面・PWA の設定と混同しないよう、端末ローカルの設定であることを見出しで示す
    // （docs/design/settings-restructure.md「PWA・デスクトップ」）。
    ui.heading("この端末の設定");
    ui.label("この PC だけに保存されます。アカウントや Bot の設定は管理画面で変更します。");
    ui.separator();

    egui::Grid::new("settings-grid")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("ホットキー");
            // 実際のキーキャプチャ UI は Phase 5。ここでは文字列編集。
            if ui
                .text_edit_singleline(&mut state.settings.hotkey)
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label("オーバーレイ不透明度");
            if ui
                .add(egui::Slider::new(
                    &mut state.settings.overlay_opacity,
                    0.3..=1.0,
                ))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label("録音停止で自動送信");
            if ui
                .checkbox(&mut state.settings.auto_send_recording, "")
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label("自動起動 (Windows)");
            // 自動起動トグルは Phase 5 で os::set_autostart に配線する（現状は無効表示）。
            let mut autostart_placeholder = false;
            ui.add_enabled(
                false,
                egui::Checkbox::new(&mut autostart_placeholder, "(Phase 5)"),
            );
            ui.end_row();
        });

    ui.separator();
    if ui.button("ログアウト").clicked() {
        state.request_logout = true;
    }

    changed
}
