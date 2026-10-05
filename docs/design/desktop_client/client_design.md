# Yuuka Desktop — クライアント設計（Rust / egui）

対象: デスクトップアプリ実装。提案段階（未実装）。
前提: [architecture.md](architecture.md)・[backend_api.md](backend_api.md)。
方針: **とにかく軽量**（単一 exe・低 RAM・イベント駆動描画）。

---

## 1. プロジェクト配置・命名

- リポジトリ内に **`clients/desktop/`**（新トップレベル）として Cargo プロジェクトを置く（プロトコル資料と近接・モノレポで版管理を一元化）。別リポジトリ化も可だが v1 はモノレポ内を推奨。
- crate 名 `yuuka-desktop` / 製品名「Yuuka Desktop」。
- 既存 Rust crate（`src/rust_crawler`・`src/rust_synapse`）は **Node が起動する常駐デーモン**であり性質が異なる。本アプリは**単体配布される GUI アプリ**なので別ディレクトリが妥当。
- edition は既存に合わせ `2021`（または 2024 が安定なら 2024）。`profile.release` で `opt-level="z"`, `lto=true`, `codegen-units=1`, `strip=true`, `panic="abort"` を設定しバイナリを最小化（NFR-1）。

---

## 2. 依存クレート（最小構成）

| 目的 | クレート | 備考 |
|---|---|---|
| GUI | `eframe` / `egui` | 描画は `wgpu`（既定）。軽量重視なら `glow`(OpenGL) feature も検討 |
| Markdown 表示 | `egui_commonmark` | 返答の MD レンダリング |
| 画像表示/ローダ | `egui_extras`（image loader）+ `image` | 受信画像・グラフ PNG・添付サムネ |
| 非同期/WS/HTTP | `tokio` + `tokio-tungstenite` + `reqwest`(rustls) | UI と別スレッド。rustls で OS 非依存 TLS |
| シリアライズ | `serde` / `serde_json` | WS/REST プロトコル |
| 音声録音 | `cpal` | マイク入力（PCM）。UI/フレームワーク非依存 |
| 音声エンコード | `opus` + `ogg`（または `audiopus`） | OGG/Opus で小サイズ（`audio/ogg`）。フォールバックで WAV（`hound`） |
| トレイ | `tray-icon` | 常駐メニュー |
| グローバルホットキー | `global-hotkey` | 呼び出し |
| トークン保存 | `keyring` | Windows Credential Manager |
| クリップボード | `arboard` | 画像貼り付け（`Ctrl+V`） |
| ブラウザ起動 | `open` | OAuth デバイスフローの verification_uri を開く |
| OS 補完（必要時のみ） | `windows`(windows-rs) | クリック透過/最前面の細部（§4） |
| 設定保存 | `directories` + `serde`（TOML/JSON） | 非機微設定のみ（トークンは keyring） |

> 公開 URL は**ビルド時に焼き込む**: `const API_BASE: &str = env!("YUUKA_API_BASE");`（本番ビルドで `YUUKA_API_BASE=https://yuuka.kawaii-music.moe`）。開発は `.cargo/config.toml` か環境変数で `http://127.0.0.1:7854`。

---

## 3. アプリ構造

```
clients/desktop/src/
├─ main.rs            … eframe 起動・トレイ/ホットキー初期化・ランタイム起動
├─ app.rs             … App 状態（egui::App 実装）。UI 状態 + チャネル端点
├─ ui/
│  ├─ overlay.rs      … オーブ表示（透明・最前面・クリック透過）
│  ├─ chat.rs         … チャットモーダル（履歴・入力・MD・画像・録音ボタン）
│  ├─ login.rs        … デバイスフロー進行 UI（「ブラウザでログイン」）
│  └─ settings.rs     … 最小設定
├─ net/
│  ├─ ws.rs           … WS クライアント（tokio-tungstenite）。送受信を mpsc で UI へ
│  ├─ rest.rs         … reqwest（/api/bots, /api/devices …）
│  └─ auth.rs         … OAuth デバイスフロー（code→open→poll→keyring 保存）
├─ audio/
│  └─ record.rs       … cpal 録音 → opus/ogg エンコード → base64
├─ os/                … OS 依存の薄い抽象（trait + windows 実装）
│  ├─ mod.rs          … trait: tray, hotkey, secret_store, overlay_window, single_instance
│  └─ windows.rs      … Windows 実装（単一インスタンスロック含む。将来 mac.rs/linux.rs 追加）
├─ model.rs           … プロトコル型（serde）。backend_api.md §3 と一致
└─ config.rs          … 焼き込み URL・設定の読み書き・起動引数(--bot)
```

- **UI スレッド（egui）** と **ネットワークスレッド（tokio runtime）** を分離。`tokio::sync::mpsc` で `UiEvent`（送信要求/録音完了）↔ `NetEvent`（status/done/push/error）を双方向に流す。
- 受信イベントで `ctx.request_repaint()` を呼び、**それ以外は再描画しない**（イベント駆動で省電力 NFR-3）。
- **1 プロセス＝1 Bot**: 起動時に対象 botId を決定（`--bot <id>` → 前回 Bot → 主 Bot）し、その Bot に束縛して WS 接続。同一 botId の二重起動は単一インスタンスロックで抑止（§4.4）。

---

## 4. オーバーレイ / 常駐の実現

### 4.1 ウィンドウ構成

- eframe `ViewportBuilder`:
  - `.with_transparent(true)`（透明背景）
  - `.with_always_on_top()`（最前面）
  - `.with_decorations(false)`（枠なし）
  - `.with_taskbar(false)`（タスクバー非表示・常駐演出）
  - `.with_mouse_passthrough(true)`（オーブ以外クリック透過）。**モーダル展開時は false に切替**して操作を受ける。
- **2 状態 UI**:
  - **collapsed（オーブ）**: **接続中 Bot のアイコン**（`discord_avatar_url`）を円形に描画。`mouse_passthrough(true)` で周囲は透過、オーブ部のみヒット。**通知件数を右上に数字バッジ**（未読返答・完了プッシュ。モーダル展開でクリア）。ドラッグで移動・位置記憶。
  - **expanded（モーダル）**: パネルサイズへリサイズ、`mouse_passthrough(false)`。フォーカス喪失/`Esc` で collapsed へ。
- 実装簡素化のため **単一ビューポートをリサイズ**して 2 状態を表現（マルチビューポートは将来の最適化）。
- Bot アイコンは `rest.rs` で取得しメモリ/ディスクキャッシュ（`egui_extras` 画像ローダ）。取得前は名前頭文字のプレースホルダ円。

### 4.2 OS 補完（必要時のみ `windows-rs`）

eframe の `mouse_passthrough` で大半が足りるが、不足時のみ:
- `WS_EX_LAYERED | WS_EX_TRANSPARENT`（クリック透過の細粒度制御）
- `SetWindowPos(HWND_TOPMOST)`（堅牢な最前面）
- マルチモニタ位置の保存/復元

→ これらは `os/windows.rs` に隔離。`os/mod.rs` の trait 越しにのみ UI から触る（NFR-7 移植性）。

### 4.3 常駐・呼び出し

- `tray-icon`: メニュー「表示 / **Bot 一覧（この Bot で開く / 切替）** / 設定 / ログアウト / 終了」。アイコン（=接続中 Bot）で未読/オフライン状態も表現。
- `global-hotkey`: 既定 `Alt+Y`（設定可）でモーダルをトグル。表示まで < 100ms（NFR-2）。**登録するのはプライマリ Bot のインスタンスのみ**（`ready.bot.primary` で判定）。サブインスタンスは登録せず衝突を回避（§4.4）。
- 起動時自動実行（設定）: Windows スタートアップ登録（レジストリ `Run` キー or スタートアップフォルダ。`os/windows.rs`）。常駐させたい Bot ごとに `--bot <id>` 付きで登録。

### 4.4 複数起動（1 プロセス 1 Bot）

- **単一インスタンスロック（botId キー）**: 起動時に `Local\yuuka-desktop-{botId}` 等の名前付き Mutex を取得（`os/windows.rs`）。既に存在＝同 Bot が起動済みなら、既存ウィンドウを前面化して自プロセスは終了。
- **異なる botId は同時起動可**: 各プロセスが独立の WS・オーバーレイ・通知バッジを持つ。
- **ホットキーはプライマリのみ**: グローバルホットキーを登録するのは**プライマリ Bot のインスタンスだけ**（`ready.bot.primary` で判定。「プライマリが吸う」）。サブインスタンスは登録せず衝突を回避。プライマリのモーダルに **Bot 一覧/切替**を集約するため、ホットキー → プライマリのモーダル表示 → そこから対象 Bot を切替/別オーバーレイ起動できる。**サブはオーブのクリック/トレイで開閉**。
- **Bot 切替の 2 形態**:
  - ① **同プロセスで切替**（プライマリのモーダル内セレクタ）: 現 WS を閉じ、新 botId で再接続。オーブのアイコンも切替先 Bot に更新。
  - ② **別オーバーレイで開く**（トレイ/モーダルの導線）: `--bot <新id>` で新プロセス（サブ）を起動（既起動ならフォーカス）。
- 起動引数 `--bot <id>`・`--hidden`（トレイのみで起動）等を `config.rs` で解釈。

---

## 5. チャット UI（`ui/chat.rs`）

- **履歴ビュー**: 上から会話を積む `ScrollArea`。各メッセージは role（user/assistant）で左右/色分け。assistant 本文は `egui_commonmark` で MD レンダリング。
- **リッチ表示**: `embeds` をカード（タイトル/本文/色帯/フィールド）に。`files`/embed image はインライン画像（`egui_extras` 画像ローダ、base64 を一時バイトとして読み込み）。
- **ステータス行**: `status` 受信で「考え中…/入力中…」のスピナー。`interim` は通常の assistant 気泡として差し込み、`push`（最終）で続報を追記。
- **入力欄**: 複数行 `TextEdit`。`Enter` 送信 / `Shift+Enter` 改行。送信中は無効化。
- **添付**: 画像のサムネ列（×で取り消し）。貼り付け（`arboard`）/ ファイル選択 / D&D（`egui` の `dropped_files`）。
- **録音ボタン**: 押下で録音開始（§6）、停止で送信（or 確認）。録音中インジケータ。
- **相手 Bot 選択**: `ready` の `bots[]` を `ComboBox` に。既定は primary。**選択＝WS 再接続**（`?botId=` を変えて張り直し、オーブのアイコンも更新）。各項目に「別オーバーレイで開く」（新プロセス起動）も用意。選択は記憶。
- **リセット**: 「会話をクリア」ボタン → WS `{type:"reset"}`。
- **接続状態**: オフライン/再接続中をヘッダに小さく表示。

---

## 6. 音声入力（`audio/record.rs`）

```
[マイクボタン押下]
  cpal で既定入力デバイスを開く（サンプルレート/チャネルを取得）
  → ストリームコールバックで PCM(f32/i16) をリングバッファへ
[停止]
  → PCM を Opus エンコード（フレーム 20ms）→ OGG コンテナへ多重化
  → base64 → WS {type:"msg", audio:{mime:"audio/ogg", data}}
```

- バックエンドは Gemini ネイティブの音声マルチモーダルで**文字起こし＋応答**（外部 STT 不要）。対応 MIME は `audio/ogg`（推奨, Opus）・`audio/wav`・`audio/flac` 等（[`src/bot.ts`](../../../src/bot.ts) の `SUPPORTED_AUDIO_TYPES` と一致させる）。
- 軽量重視で **OGG/Opus**（小サイズ）を既定。エンコードが重い/不安定な環境向けに **WAV(PCM16)** フォールバック（`hound`）。
- 録音は別スレッド。UI はインジケータのみ。長尺は上限（例 60s / 20MB）で打ち切り。
- v1 は**音声入力のみ**。TTS（読み上げ）は将来（[roadmap.md](roadmap.md)）。

---

## 7. 画像入力

- 取得経路: ① ファイル選択（`rfd` 採用 or OS ダイアログ）② クリップボード（`arboard::Clipboard::get_image`）③ D&D（`egui` `dropped_files`）。
- 送信前にサムネ表示・取り消し。必要なら長辺リサイズ（`image`）で送信サイズ削減（軽量・上限対策）。
- MIME はファイル/クリップボードから判定（既定 `image/png`）。WS `{type:"msg", image:{mime,data}}`。
- 受信画像（embed image・グラフ PNG）は `egui_extras` でインライン表示。

---

## 8. 認証フロー（`net/auth.rs`）

```
起動 → keyring からトークン取得
  ├─ 有る  → WS 接続（Bearer）。401 なら破棄して下へ
  └─ 無い/無効 →
       POST /api/auth/device/code  → user_code 等
       open(verification_uri_complete)            // 既定ブラウザでログイン&承認
       loop: POST /api/auth/device/token (interval) 
             authorization_pending → 継続 / approved → access_token
       keyring へ保存 → WS 接続
```

- ログイン中 UI（`ui/login.rs`）は user_code とブラウザ誘導を表示。
- ログアウト（トレイ）: keyring 破棄 + `POST /api/devices/revoke`（自端末）+ WS 切断。

---

## 9. ネットワーク層（`net/ws.rs`）

- `tokio-tungstenite` で `wss://{API_BASE host}/ws/chat?botId={接続中Bot}`、`Authorization: Bearer` ヘッダ付き upgrade（ネイティブはヘッダ設定可）。**接続は 1 Bot に束縛**。
- 受信タスク: JSON を `NetEvent` に変換し UI へ（`ready/status/interim/token/done/push/error`）。`ready` の `bot`/`bots` でオーブ画像と切替セレクタを更新。
- 送信タスク: UI からの `UiEvent`（msg/reset/ping）を WS へ（botId は接続束縛なのでメッセージには含めない）。
- **Bot 切替**: 現接続を `close` し、新 botId で張り直す（同じ再接続パスを再利用）。
- 再接続: 切断で指数バックオフ（例 1s→2s→…→30s）。再接続後 `ready` 受領で復帰。切断中の送信はキュー退避し再送（NFR-6）。
- keepalive: WS ping/pong（30s）。

---

## 10. 設定・データ保存

| 種別 | 保存先 |
|---|---|
| Bearer トークン（機微） | **keyring（Windows Credential Manager）** のみ |
| 非機微設定（ホットキー・オーバーレイ位置/不透明度・相手 Bot・自動起動・録音自動送信） | `directories` の config dir に TOML/JSON |
| 会話履歴 | **保存しない**（正はサーバ `message_logs`）。再接続時に必要なら REST で直近取得（任意） |

---

## 11. ビルド・配布

- **本番ビルド**: `YUUKA_API_BASE=https://yuuka.kawaii-music.moe cargo build --release`（URL 焼き込み）。`profile.release` 最小化設定（§1）。成果物は**単一 exe**（≤20MB 目標, NFR-1）。
- **配布**: Web ダッシュボードに「デスクトップ版をダウンロード」導線を追加（`src/public/*`）。直リンクで exe 配布（v1）。将来 `.msi`（`cargo-wix`）/署名/自動更新（`self_update`）。
- **CI**: GitHub Actions の Windows ランナーで release ビルド → アーティファクト。署名は将来。
- **アイコン/ブランディング**: 早瀬ユウカのオーブ意匠（ファンメイド・ガイドライン準拠。README のライセンス節参照）。

---

## 12. テスト・検証観点

- 単体: プロトコル `model.rs` の serde 往復、音声エンコード（OGG/Opus 生成物が Gemini で文字起こし可能か実機確認）。
- 結合: ローカル backend（`http://127.0.0.1:7854`）に対しデバイスフロー→WS→`processMessage` 往復。
- オーバーレイ: クリック透過/最前面/マルチモニタ/DPI スケールの実機確認（Windows 10/11）。
- 軽量性: アイドル RAM/CPU 計測（NFR-1/3）。リーク確認（長時間常駐）。
- 障害: backend 停止・WS 切断・トークン失効時のデグレードと再ログイン。

---

## 13. 既知の検討事項（実装前に詰める）

- `egui` のクリック透過は eframe 既定でどこまで行けるか（不足分の `windows-rs` 範囲を実機で確定）。
- トークン逐次表示を後で入れる場合の `gemini.ts` 最終応答ストリーミング化（会話コア小改修。[architecture.md §6](architecture.md)）。
- 大型添付（高解像度画像/長尺音声）の上限と、超過時の REST アップロード→参照 ID 方式への移行（[backend_api.md §3.5](backend_api.md)）。
- `wgpu` と `glow` のどちらが軽量・安定か（ターゲット GPU 次第。要ベンチ）。
