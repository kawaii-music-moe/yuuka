//! オーバーレイ（オーブ）ビュー。
//!
//! 仕様: client_design.md §4.1 / requirements.md FR-2 / FR-2b。
//!
//! - collapsed（オーブ）: **接続中 Bot のアイコン**を円形に描画。右上に通知バッジ
//!   （未読件数）。ドラッグで移動・位置記憶。クリックでモーダルを開く。
//! - オーブ窓は「透明＋クリック可能」な通常窓（クリック透過は使わない）。Windows では
//!   クリック透過＝`WS_EX_LAYERED` で glow のアルファ合成が壊れ、透過が効かず窓が
//!   不透明になる（オーブも押せない）ため。詳細は main.rs / app.rs のコメント参照。
//!
//! アイコンは Net 側が取得した画像バイト（`AppState::avatars`）を egui の画像ローダ越しに
//! 円形描画する。未取得/デコード中は背景円＋Bot 名頭文字のプレースホルダを見せる。

use crate::app::AppState;

/// オーブ 1 個分の直径（論理ピクセル）。
const ORB_DIAMETER: f32 = 56.0;

/// オーブビューを描画する。クリックされたら `true`（モーダルを開く合図）。
pub fn view(state: &mut AppState, ui: &mut egui::Ui) -> bool {
    let mut clicked = false;

    // 小さな collapsed 窓の中央にオーブを置く（窓＝オーブ大なので中央寄せで安定）。
    let full = ui.available_rect_before_wrap();
    let rect = egui::Rect::from_center_size(full.center(), egui::vec2(ORB_DIAMETER, ORB_DIAMETER));
    let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());

    let center = rect.center();
    let radius = ORB_DIAMETER / 2.0;

    // 背景プレースホルダ円（アイコン未取得/デコード中もこれが見える＝オーブが消えない）。
    let fill = if response.hovered() {
        egui::Color32::from_rgb(90, 130, 200)
    } else {
        egui::Color32::from_rgb(70, 110, 180)
    };
    ui.painter().circle_filled(center, radius, fill);

    // 頭文字プレースホルダを常に下地として描く。アイコン未取得・取得失敗・デコード失敗の
    // いずれでもオーブが「ただの空の円」に見えないようにするため（取得できれば下の画像が覆う）。
    let initial = state
        .bot
        .as_ref()
        .and_then(|b| b.name.chars().next())
        .unwrap_or('Y')
        .to_string();
    ui.painter().text(
        center,
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(24.0),
        egui::Color32::WHITE,
    );

    // 接続中 Bot のアイコンが取得済みなら、頭文字の上へ円形にクリップして重ねる。
    let bound_id = state.bot.as_ref().map(|b| b.id.clone());
    let avatar = bound_id
        .as_ref()
        .and_then(|id| state.avatars.get(id).cloned());
    if let (Some(id), Some(bytes)) = (bound_id.as_ref(), avatar) {
        egui::Image::new(egui::ImageSource::Bytes {
            // uri は bot_id 毎に安定 → egui がデコード済みテクスチャをキャッシュする。
            uri: crate::app::avatar_uri(id).into(),
            bytes,
        })
        .rounding(egui::Rounding::same(radius))
        .paint_at(ui, rect);
    }

    // 通知バッジ（右上に未読件数）。モーダルを開くとクリアされる（app.rs 側）。
    if state.unread > 0 {
        let badge_center = egui::pos2(rect.right() - 8.0, rect.top() + 8.0);
        let painter = ui.painter();
        painter.circle_filled(badge_center, 9.0, egui::Color32::from_rgb(220, 60, 60));
        painter.text(
            badge_center,
            egui::Align2::CENTER_CENTER,
            state.unread.to_string(),
            egui::FontId::proportional(11.0),
            egui::Color32::WHITE,
        );
    }

    // --- タップ（押して開く）とドラッグ（掴んで動かす）を自前で切り分ける ---
    //
    // 以前は egui の `click_and_drag` に委ね、`clicked()` でモーダルを開き
    // `drag_started()` で OS ネイティブの `StartDrag` 移動ループへ入れていた。しかし
    //   1. egui は押下が 6px 動く／0.8 秒を超えた時点で「ドラッグ確定」にし、その押下では
    //      二度と `clicked()` を返さない（少しの手ブレ・わずかな長押しでもクリックが消える）。
    //   2. `StartDrag` の OS 移動ループはボタンの release（WM_LBUTTONUP）を食べてしまう。
    // ため「クリックしてもモーダルが出ない」事故になっていた。そこで押下中の生の移動量を
    // 自前で積算し、しきい値未満のまま離されたらタップ＝モーダルを開き、しきい値を超えた
    // ときだけネイティブ移動を開始する（egui の click/drag 判定の 6px・0.8s ゲートに依存しない）。
    const ORB_DRAG_THRESHOLD: f32 = 8.0; // 論理 px。これ未満の移動はタップ扱い。

    // is_pointer_button_down_on＝押下起点がオーブの間 true。primary_down と AND して、
    // OS 移動ループ後に万一フラグが残っても確実に release 側（タップ判定）へ倒す。
    let pressing = response.is_pointer_button_down_on()
        && ui.ctx().input(|i| i.pointer.primary_down());
    if pressing {
        let moved = ui.ctx().input(|i| i.pointer.delta()).length();
        let travel = state.orb_press_travel.get_or_insert(0.0);
        let before = *travel;
        *travel += moved;
        // しきい値を初めて超えた瞬間にだけネイティブ窓移動を開始する（以後は OS の移動
        // ループが引き継ぐので二重送出しない）。移動後の左上座標は app.rs が `outer_rect`
        // から読み取り `overlay_pos` へ記憶する。
        if before < ORB_DRAG_THRESHOLD && *travel >= ORB_DRAG_THRESHOLD {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    } else if let Some(travel) = state.orb_press_travel.take() {
        // 離した: 移動が小さければタップ＝モーダルを開く。
        if travel < ORB_DRAG_THRESHOLD {
            clicked = true;
        }
    }
    // 1 フレーム内で完結した素早いクリックの取りこぼし防止（egui が click 判定したら開く）。
    if response.clicked() {
        clicked = true;
    }

    // 移動可能であることを示すためカーソルを grab 表示にする。
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }

    clicked
}
