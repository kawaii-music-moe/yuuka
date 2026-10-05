//! チャットモーダルビュー（履歴・入力・ステータス・Bot 切替）。
//!
//! 仕様: client_design.md §5 / requirements.md FR-4 / FR-7 / FR-8 / FR-11 / FR-12。
//!
//! 本フェーズの骨格に含むもの:
//! - 履歴ビュー（role で色分け、assistant は MD レンダリング）
//! - ステータス行（thinking/writing のスピナー）
//! - 入力欄（Enter 送信 / Shift+Enter 改行）
//! - Bot 切替 ComboBox（選択 = WS 再接続）
//! - 会話クリア（reset）/ 接続状態表示
//!
//! 画像添付・録音ボタンは Phase 3/4 で追加するため、ここではプレースホルダのみ。

use egui_commonmark::CommonMarkViewer;

use crate::app::{AppState, UiIntent};
use crate::model::StatusState;
use crate::net::ConnectionState;

use super::Role;

/// チャットモーダルを描画し、ユーザー操作を [`UiIntent`] として返す。
///
/// 返した intent は `app.rs` が Net スレッドへ橋渡しする（UI とネットの疎結合）。
pub fn view(state: &mut AppState, ui: &mut egui::Ui) -> Option<UiIntent> {
    let mut intent: Option<UiIntent> = None;

    // 履歴中の画像添付（受信ファイル＋送信ローカルエコー）を一度だけ egui へ登録する。
    register_image_files(state, ui.ctx());

    // --- 入力エリアを下部パネルへ固定する ---
    // 履歴 ScrollArea は残り高さを埋める（auto_shrink false）ため、素朴に縦へ並べると
    // 入力欄が画面外へ押し出されて「文字入力 UI が見つからない」状態になる。入力を
    // 下部パネルに置くことで、履歴がどれだけ伸びても入力欄は常に画面内に出る。
    egui::TopBottomPanel::bottom("chat-input")
        .resizable(false)
        .show_inside(ui, |ui| {
            ui.add_space(4.0);
            if state.recording.is_some() {
                // 録音中は入力欄を隠し、録音インジケータ（停止/取消）だけを出す。
                recording_bar(state, ui, &mut intent);
            } else {
                ingest_dropped_and_pasted(state, ui);
                attachment_bar(state, ui);
                input_row(state, ui, &mut intent);
            }
            ui.add_space(2.0);
        });

    // --- ヘッダ: Bot 切替セレクタ + 接続状態 + 会話クリア ---
    ui.horizontal(|ui| {
        bot_selector(state, ui, &mut intent);
        ui.separator();
        connection_label(&state.connection, ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("会話をクリア").clicked() {
                intent = Some(UiIntent::Reset);
                clear_history(state, ui.ctx());
            }
        });
    });
    ui.separator();

    // --- 履歴ビュー（中央の残り高さを埋める）---
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for (idx, entry) in state.history.iter().enumerate() {
                if let Some(i) = message_bubble(idx, entry, ui, &mut state.commonmark_cache) {
                    intent = Some(i);
                }
            }

            // ステータス行（考え中…/入力中…）。
            if let Some(status) = &state.status {
                ui.horizontal(|ui| {
                    ui.spinner();
                    let label = match status {
                        StatusState::Thinking => "考え中…",
                        StatusState::Writing => "入力中…",
                        StatusState::Other => "処理中…",
                    };
                    ui.label(label);
                });
            }
        });

    intent
}

/// 入力欄（複数行 TextEdit + 送信）。Enter 送信 / Shift+Enter 改行。
/// 本文か画像のいずれかがあれば送信可能（画像のみの発話も許容。§3.2）。
fn input_row(state: &mut AppState, ui: &mut egui::Ui, intent: &mut Option<UiIntent>) {
    ui.horizontal(|ui| {
        let connected = matches!(state.connection, ConnectionState::Connected { .. });
        let over_limit = state
            .pending_attachment
            .as_ref()
            .is_some_and(|a| a.exceeds_limit(state.max_upload_mb));
        let send_enabled = state.status.is_none() && connected && !over_limit;

        // 通常 Enter（Shift なし）は「送信」専用にし、複数行 TextEdit に**改行を
        // 入れさせない**。そのため、TextEdit が処理する前に Enter を消費しておく
        // （消費しないと、IME 変換確定の Enter で改行が入ってしまう）。Shift+Enter は
        // 消費しないので従来どおり改行になる。フォーカス判定は前フレームの記憶を見る。
        let field_id = ui.id().with("chat-input");
        let has_focus = ui.memory(|m| m.has_focus(field_id));
        let plain_enter = ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        // IME 変換確定の Enter は同じフレームに IME イベント（Commit/Preedit）を伴う。
        // そのフレームは「確定だけ」＝改行も送信もしない。確定後、IME を伴わない次の
        // Enter で初めて送信する（日本語入力の確定→送信の 2 段階）。
        let ime_active = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Ime(_))));
        if has_focus && plain_enter {
            ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
            });
        }

        let text_edit = egui::TextEdit::multiline(&mut state.input)
            .id(field_id)
            .hint_text("メッセージを入力（Enter 送信 / Shift+Enter 改行）")
            .desired_rows(2)
            .desired_width(f32::INFINITY);
        ui.add_enabled(send_enabled, text_edit);

        let enter_pressed = has_focus && plain_enter && !ime_active;
        let send_clicked = ui
            .add_enabled(send_enabled, egui::Button::new("送信"))
            .clicked();

        let has_payload = !state.input.trim().is_empty() || state.pending_attachment.is_some();
        if send_enabled && has_payload && (enter_pressed || send_clicked) {
            let text = state.input.trim().to_string();
            let staged = state.pending_attachment.take();
            let attachment = staged.as_ref().map(|s| s.to_attachment());

            // ローカルエコー（送った画像はユーザー気泡にサムネ表示する）。
            let mut entry = super::ChatEntry::user(text.clone());
            if let (Some(s), Some(att)) = (&staged, &attachment) {
                entry.files.push(crate::model::FilePayload {
                    name: s.name.clone(),
                    mime: att.mime.clone(),
                    data: att.data.clone(),
                });
            }
            state.history.push(entry);
            state.input.clear();
            *intent = Some(UiIntent::SendMessage {
                text,
                image: attachment,
                audio: None,
            });
        }
    });
}

/// D&D されたファイル / クリップボード貼り付け（Ctrl/Cmd+V）から画像をステージする。
fn ingest_dropped_and_pasted(state: &mut AppState, ui: &egui::Ui) {
    // ドロップされたファイル（最初の画像を採用）。raw.dropped_files は毎フレーム空に戻る。
    let dropped = ui.input(|i| i.raw.dropped_files.clone());
    for f in &dropped {
        let staged = if let Some(path) = &f.path {
            crate::attach::from_path(path)
        } else if let Some(bytes) = &f.bytes {
            crate::attach::from_named_bytes(&f.name, bytes)
        } else {
            None
        };
        if let Some(img) = staged {
            state.pending_attachment = Some(img);
            break;
        }
    }
    // クリップボード貼り付け（画像があれば添付へ。テキストは TextEdit が別途処理）。
    let paste = ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::V));
    if paste {
        if let Some(img) = crate::attach::from_clipboard() {
            state.pending_attachment = Some(img);
        }
    }
}

/// 「📎 画像」ファイル選択ボタンと、ステージ済み添付のサムネ＋削除＋サイズ警告。
fn attachment_bar(state: &mut AppState, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let pick = ui
            .button("📎 画像")
            .on_hover_text("画像を添付（ドラッグ＆ドロップ / 貼り付けも可）")
            .clicked();
        if pick {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("画像", crate::attach::IMAGE_EXTS)
                .pick_file()
            {
                if let Some(img) = crate::attach::from_path(&path) {
                    state.pending_attachment = Some(img);
                }
            }
        }
        // 音声入力（録音開始）。録音中は recording_bar 側へ切り替わる。
        if ui
            .button("🎤 録音")
            .on_hover_text("音声で入力（クリックで録音開始）")
            .clicked()
        {
            start_recording(state, ui);
        }
    });

    let mut clear = false;
    if let Some(att) = &state.pending_attachment {
        let over = att.exceeds_limit(state.max_upload_mb);
        ui.horizontal(|ui| {
            // uri は内容ハッシュで変える（同名別画像＝連続貼り付けでキャッシュが古くならないよう）。
            let uri = format!("bytes://staged-{:016x}", att.tag);
            ui.add(
                egui::Image::new(egui::ImageSource::Bytes {
                    uri: uri.into(),
                    bytes: egui::load::Bytes::from(att.bytes.clone()),
                })
                .max_height(48.0)
                .maintain_aspect_ratio(true)
                .rounding(egui::Rounding::same(4.0)),
            );
            ui.vertical(|ui| {
                ui.label(&att.name);
                if over {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 80, 80),
                        format!("サイズ超過（上限 {}MB）。送信できません。", state.max_upload_mb),
                    );
                }
            });
            if ui.button("✕").on_hover_text("添付を取り消す").clicked() {
                clear = true;
            }
        });
    }
    if clear {
        state.pending_attachment = None;
    }
}

/// 録音を開始する。失敗（マイク無し / 音声 feature 無効ビルド等）はメッセージで知らせる。
fn start_recording(state: &mut AppState, ui: &egui::Ui) {
    match crate::audio::record::Recorder::start() {
        Ok(recorder) => {
            let started_at = ui.input(|i| i.time);
            state.recording = Some(crate::app::Recording {
                recorder,
                started_at,
            });
        }
        Err(e) => {
            state
                .history
                .push(super::ChatEntry::assistant(format!(
                    "⚠ 録音を開始できません: {e}"
                )));
        }
    }
}

/// 録音中インジケータ（点滅する赤丸＋経過秒）と、停止して送信 / 取消ボタン。
/// 「録音中」が一目で分かる UI。60 秒で自動停止して送信する。
fn recording_bar(state: &mut AppState, ui: &mut egui::Ui, intent: &mut Option<UiIntent>) {
    let now = ui.input(|i| i.time);
    let elapsed = state
        .recording
        .as_ref()
        .map(|r| (now - r.started_at).max(0.0))
        .unwrap_or(0.0);
    // 経過秒・点滅を更新するため録音中は継続再描画（~20fps）。
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(50));

    let mut stop = elapsed >= 60.0; // 上限 60s で自動停止＆送信。
    let mut cancel = false;

    egui::Frame::none()
        .fill(egui::Color32::from_rgb(60, 30, 30))
        .inner_margin(egui::Margin::symmetric(8.0, 6.0))
        .rounding(egui::Rounding::same(6.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // 点滅する赤丸（録音中の視覚指標）。
                let pulse = 0.5_f32 + 0.5 * ((now * 4.0).sin() as f32); // 0..1
                let red = (170.0_f32 + 70.0 * pulse) as u8;
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(rect.center(), 6.0, egui::Color32::from_rgb(red, 40, 40));
                ui.colored_label(
                    egui::Color32::from_rgb(235, 120, 120),
                    format!("● 録音中  {elapsed:.1} 秒"),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("■ 停止して送信").clicked() {
                        stop = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
        });

    if cancel {
        // 録音を破棄（送信しない）。Recorder を drop すれば cpal ストリームも止まる
        // （WAV エンコードを無駄に走らせない）。
        state.recording = None;
    } else if stop {
        if let Some(rec) = state.recording.take() {
            match rec.recorder.stop() {
                Ok(audio) => {
                    state
                        .history
                        .push(super::ChatEntry::user("🎤 音声メッセージ".to_string()));
                    *intent = Some(UiIntent::SendMessage {
                        text: String::new(),
                        image: None,
                        audio: Some(crate::model::Attachment {
                            mime: audio.mime,
                            data: audio.data_base64,
                        }),
                    });
                }
                Err(e) => {
                    state
                        .history
                        .push(super::ChatEntry::assistant(format!(
                            "⚠ 録音の処理に失敗: {e}"
                        )));
                }
            }
        }
    }
}

/// Bot 切替セレクタ（選択で WS 再接続 = [`UiIntent::SwitchBot`]）。
///
/// 所有/共有/個人用すべての Bot（`ready.bots`）を一覧する。小さなウィンドウでも全件へ
/// 届くよう、ポップアップに高さ上限（＝スクロール）を設ける。各行に Bot アイコンを添える。
fn bot_selector(state: &mut AppState, ui: &mut egui::Ui, intent: &mut Option<UiIntent>) {
    let current_name = state
        .bot
        .as_ref()
        .map(|b| b.name.clone())
        .unwrap_or_else(|| "(未接続)".to_string());
    // クローンしておきセレクタ内での state 借用衝突を避ける。
    let current_id = state.bot.as_ref().map(|b| b.id.clone());

    egui::ComboBox::from_id_salt("bot-selector")
        .selected_text(current_name)
        .width(160.0)
        .height(260.0) // 小窓でも全 Bot へスクロールで到達できるよう上限を設ける。
        .show_ui(ui, |ui| {
            let bots = state.bots.clone();
            if bots.is_empty() {
                ui.weak("(利用可能な Bot がありません)");
                return;
            }
            for bot in &bots {
                let selected = current_id.as_deref() == Some(bot.id.as_str());
                let resp = ui
                    .horizontal(|ui| {
                        bot_row_icon(state, &bot.id, ui);
                        // 名前空の Bot でもクリックできるよう識別子をフォールバック表示。
                        let label = if bot.name.is_empty() {
                            bot.id.as_str()
                        } else {
                            bot.name.as_str()
                        };
                        ui.selectable_label(selected, label)
                    })
                    .inner;
                if resp.clicked() && !selected {
                    *intent = Some(UiIntent::SwitchBot {
                        bot_id: bot.id.clone(),
                    });
                }
            }
        });
}

/// セレクタ 1 行の先頭に Bot アイコン（取得済みは小さな円画像、未取得は色丸）を描く。
fn bot_row_icon(state: &AppState, bot_id: &str, ui: &mut egui::Ui) {
    const SZ: f32 = 18.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(SZ, SZ), egui::Sense::hover());
    if let Some(bytes) = state.avatars.get(bot_id) {
        egui::Image::new(egui::ImageSource::Bytes {
            uri: crate::app::avatar_uri(bot_id).into(),
            bytes: bytes.clone(),
        })
        .rounding(egui::Rounding::same(SZ / 2.0))
        .paint_at(ui, rect);
    } else {
        ui.painter()
            .circle_filled(rect.center(), SZ / 2.0, egui::Color32::from_rgb(70, 110, 180));
    }
}

/// 接続状態は色付きの丸だけで示す（ヘッダを簡潔に。client_design.md §5）。
/// 詳細（再接続の残り秒数など）はホバーのツールチップに載せる。
fn connection_label(conn: &ConnectionState, ui: &mut egui::Ui) {
    let (color, tip) = match conn {
        ConnectionState::Connected { .. } => {
            (egui::Color32::from_rgb(80, 200, 120), "オンライン".to_string())
        }
        ConnectionState::Connecting => (egui::Color32::GRAY, "接続中…".to_string()),
        ConnectionState::Reconnecting { next_retry_secs } => (
            egui::Color32::from_rgb(230, 180, 80),
            format!("再接続中… ({next_retry_secs}s)"),
        ),
        ConnectionState::Disconnected => {
            (egui::Color32::from_rgb(200, 80, 80), "オフライン".to_string())
        }
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, color);
    resp.on_hover_text(tip);
}

/// 1 メッセージの気泡描画（role で色分け、assistant は MD）。
///
/// ボタン押下があれば [`UiIntent::Interaction`] を返す（ws_components.md §6）。
fn message_bubble(
    entry_idx: usize,
    entry: &super::ChatEntry,
    ui: &mut egui::Ui,
    cache: &mut egui_commonmark::CommonMarkCache,
) -> Option<UiIntent> {
    let mut intent: Option<UiIntent> = None;
    let (align, frame_fill) = match entry.role {
        Role::User => (egui::Align::Max, egui::Color32::from_rgb(60, 90, 140)),
        Role::Assistant => (egui::Align::Min, egui::Color32::from_rgb(45, 45, 55)),
    };

    ui.with_layout(egui::Layout::top_down(align), |ui| {
        egui::Frame::none()
            .fill(frame_fill)
            .inner_margin(egui::Margin::same(8.0))
            .rounding(egui::Rounding::same(8.0))
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() * 0.85);
                match entry.role {
                    Role::User => {
                        ui.label(&entry.text);
                    }
                    Role::Assistant => {
                        // Markdown レンダリング（コードブロック/リスト/リンク）。
                        CommonMarkViewer::new().show(ui, cache, &entry.text);
                    }
                }

                // Embed カード（embed image は files の同名添付をインライン表示）。
                for embed in &entry.embeds {
                    embed_card(entry_idx, embed, &entry.files, ui);
                }
                // files: 画像はインライン表示、それ以外は弱ラベル。embed が参照済みの
                // 添付は二重表示しない。
                let referenced: std::collections::HashSet<&str> = entry
                    .embeds
                    .iter()
                    .filter_map(|e| e.image.as_ref().map(|im| im.name.as_str()))
                    .collect();
                for (file_idx, file) in entry.files.iter().enumerate() {
                    if referenced.contains(file.name.as_str()) {
                        continue;
                    }
                    file_attachment(entry_idx, file_idx, file, ui);
                }

                // 対話コンポーネント（action row）= 横並びのボタン列。
                for row in &entry.components {
                    if let Some(i) = action_row(row, entry.message_id.as_deref(), ui) {
                        intent = Some(i);
                    }
                }
            });
    });
    ui.add_space(4.0);
    intent
}

/// 1 つの action row を横並びのボタン列として描画する（ws_components.md §6）。
fn action_row(
    row: &crate::model::ActionRow,
    message_id: Option<&str>,
    ui: &mut egui::Ui,
) -> Option<UiIntent> {
    use crate::model::Component;
    let mut intent: Option<UiIntent> = None;
    ui.horizontal_wrapped(|ui| {
        for comp in &row.components {
            // 行内に現れるのはボタンのみ想定。Row/Unknown は描画しない。
            let Component::Button {
                style,
                label,
                custom_id,
                url,
                disabled,
            } = comp
            else {
                continue;
            };

            let text = label.clone().unwrap_or_default();
            let button = egui::Button::new(text).fill(button_color(*style));
            let resp = ui.add_enabled(!*disabled, button);

            if resp.clicked() {
                if *style == 5 {
                    // Link ボタン: 既定ブラウザで URL を開く（送信しない）。
                    if let Some(url) = url {
                        let _ = open::that_detached(url);
                    }
                } else if let (Some(message_id), Some(custom_id)) = (message_id, custom_id) {
                    // 非 Link ボタン: interaction を発火（message_id と custom_id 必須）。
                    intent = Some(UiIntent::Interaction {
                        message_id: message_id.to_string(),
                        custom_id: custom_id.clone(),
                    });
                }
            }
        }
    });
    intent
}

/// Discord スタイル番号を気泡ボタンの塗り色へ対応づける（ws_components.md §1）。
/// 1=Primary(青) 2=Secondary(灰) 3=Success(緑) 4=Danger(赤) 5=Link(リンク表示)。
fn button_color(style: u8) -> egui::Color32 {
    match style {
        1 => egui::Color32::from_rgb(60, 110, 200),  // Primary 青
        3 => egui::Color32::from_rgb(60, 160, 90),   // Success 緑
        4 => egui::Color32::from_rgb(200, 70, 70),   // Danger 赤
        5 => egui::Color32::from_rgb(70, 90, 130),   // Link リンク調
        _ => egui::Color32::from_rgb(90, 90, 100),   // Secondary 灰（既定）
    }
}

/// Embed カードの描画（タイトル/本文/色帯/フィールド/インライン画像）。
///
/// `embed.image`（`attachment://name`）は `files` の同名添付へ解決して画像表示する
/// （architecture.md §7）。画像バイトは `register_image_files` で事前登録済み。
fn embed_card(
    entry_idx: usize,
    embed: &crate::model::Embed,
    files: &[crate::model::FilePayload],
    ui: &mut egui::Ui,
) {
    let stripe = embed
        .color
        .map(|c| {
            egui::Color32::from_rgb(
                ((c >> 16) & 0xFF) as u8,
                ((c >> 8) & 0xFF) as u8,
                (c & 0xFF) as u8,
            )
        })
        .unwrap_or(egui::Color32::GRAY);

    egui::Frame::none()
        .fill(egui::Color32::from_rgb(35, 35, 42))
        .stroke(egui::Stroke::new(3.0, stripe))
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            if let Some(title) = &embed.title {
                ui.strong(title);
            }
            if let Some(desc) = &embed.description {
                ui.label(desc);
            }
            for f in &embed.fields {
                ui.horizontal(|ui| {
                    ui.strong(format!("{}:", f.name));
                    ui.label(&f.value);
                });
            }
            if let Some(img) = &embed.image {
                if let Some((file_idx, file)) =
                    files.iter().enumerate().find(|(_, f)| f.name == img.name)
                {
                    file_attachment(entry_idx, file_idx, file, ui);
                }
            }
        });
}

/// チャット画像添付の安定 URI。履歴は append-only（並べ替え無し）なので、出現位置
/// （履歴インデックス＋添付インデックス）で一意・衝突無し。会話クリア時は
/// [`clear_history`] が `forget_all_images` でテクスチャを破棄し、インデックス再利用
/// による古画像表示を防ぐ。
fn file_uri(entry_idx: usize, file_idx: usize) -> String {
    format!("bytes://chatfile-{entry_idx}-{file_idx}")
}

/// 履歴中の未登録な画像添付を base64 デコードして egui へ登録する（初回のみ）。
/// 以降の描画は `Image::new(Uri)` で再デコード無しにテクスチャを参照できる。
fn register_image_files(state: &mut AppState, ctx: &egui::Context) {
    use base64::Engine;
    // history 借用を先に解放するため、未登録ぶんを (uri, base64) で集めてから処理する。
    let mut pending: Vec<(String, String)> = Vec::new();
    for (ei, entry) in state.history.iter().enumerate() {
        for (fi, file) in entry.files.iter().enumerate() {
            if file.mime.starts_with("image/") {
                let uri = file_uri(ei, fi);
                if !state.loaded_files.contains(&uri) {
                    pending.push((uri, file.data.clone()));
                }
            }
        }
    }
    for (uri, b64) in pending {
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) {
            ctx.include_bytes(uri.clone(), egui::load::Bytes::from(bytes));
        }
        // デコード失敗もマークして毎フレームの再試行を避ける（画像は出ないが落ちない）。
        state.loaded_files.insert(uri);
    }
}

/// 会話履歴をクリアし、登録済み画像テクスチャ／バイトも破棄する。
///
/// これをしないと (a) クリア後にインデックスを再利用した新画像が旧テクスチャを表示し、
/// (b) 画像キャッシュがセッション中に無制限に増える。アバター等 `ImageSource::Bytes`
/// 由来は毎フレーム再登録されるため `forget_all_images` でも消えたままにはならない。
fn clear_history(state: &mut AppState, ctx: &egui::Context) {
    state.history.clear();
    state.loaded_files.clear();
    ctx.forget_all_images();
}

/// 1 添付の描画。画像はインライン表示（事前登録済みバイトを位置ベース URI で参照）、
/// それ以外は弱ラベル（ダウンロード経路は未提供）。
fn file_attachment(
    entry_idx: usize,
    file_idx: usize,
    file: &crate::model::FilePayload,
    ui: &mut egui::Ui,
) {
    if file.mime.starts_with("image/") {
        ui.add(
            egui::Image::new(egui::ImageSource::Uri(file_uri(entry_idx, file_idx).into()))
                .max_height(220.0)
                .maintain_aspect_ratio(true)
                .rounding(egui::Rounding::same(6.0)),
        );
    } else {
        ui.weak(format!("📎 {} ({})", file.name, file.mime));
    }
}
