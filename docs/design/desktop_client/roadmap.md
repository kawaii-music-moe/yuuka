# Yuuka Desktop — ロードマップ・タスク分解

対象: 実装計画。提案段階（未実装）。
前提: [backend_api.md](backend_api.md)・[client_design.md](client_design.md)。

---

## 1. 段階導入（フェーズ）

依存順に並べる。各フェーズは独立に検証可能。**Phase 0〜1 はバックエンド**（会話コアは無改修）、**Phase 2〜5 はクライアント**。

| Phase | 目的 | 主な成果物 | 完了条件 |
|---|---|---|---|
| **0. 認証基盤** | デバイスフロー + トークン | `desktop_tokens`(v13), `desktopAuthService`, `deviceAuthRoutes`, Bearer 解決の `routeRegistry` 拡張, `/device` 承認ページ, 端末管理 API/UI | ブラウザ承認で Bearer 発行 → `GET /api/bots` が Bearer で通る。Web から失効可 |
| **1. WS チャット（テキスト）** | 会話 API 本体 | `server.ts` upgrade 配線, `chatWebSocket`, `chatChannelService`, WS プロトコル(status/done/interim/push/error) | `wscat` 等で `msg`→`done` 往復。重い処理で `interim`→`push`。Discord と同一コンテキスト共有 |
| **2. クライアント骨格** | 常駐 + ログイン + テキスト会話 | egui アプリ, トレイ, ホットキー, デバイスフロー(`net/auth`), WS(`net/ws`), チャット UI + MD | exe 起動→ブラウザログイン→テキストで会話・返答ストリーム表示 |
| **3. 画像入力/リッチ表示** | 画像 in/out | 添付(貼付/選択/D&D), 受信画像/embed/グラフのインライン表示 | 画像を送って解析返答。グラフ PNG/Embed が表示される |
| **4. 音声入力** | 音声 in | `audio/record`（cpal→opus/ogg→base64） | 録音→送信→文字起こし＋応答。ToDo 化提案等が動く |
| **5. オーバーレイ仕上げ + 配布** | 製品化 | オーブ/クリック透過/最前面の作り込み, 自動起動, 設定, release 最小化, Web ダウンロード導線 | NFR（軽量/起動速度/省電力）達成。Web から DL して一連が動く |
| **将来** | 拡張 | トークン逐次表示, **TTS（音声出力）**, mac/Linux, 自動更新/署名, 大型添付の REST アップロード | — |

---

## 2. タスク分解（チェックリスト）

### Phase 0 — 認証基盤（バックエンド）— ✅ 実装済み（2026-06-23、未デプロイ）
- [x] `db/migrations.ts`: `migrateToDesktopTokens`（v12→**v13**、`desktop_tokens` 冪等追加）
- [x] `db/desktopTokenRepo.ts`: 発行/検証(sha256)/スライディング更新/失効/一覧
- [x] `services/desktopAuthService.ts`: デバイスコード発行（Redis 一時保存＋インメモリ fallback）・承認・トークン交換・`verifyToken→SessionUser`
- [x] `config.ts`: `DESKTOP_TOKEN_TTL_DAYS`(90)・`DESKTOP_MAX_UPLOAD_MB`(20)・`DESKTOP_DEVICE_CODE_TTL_SEC`(600) を `getSetting` で読込（既定値あり・必須env無し）
- [x] `server/routes/deviceAuthRoutes.ts`: `/api/auth/device/{code,approve,token}`
- [x] `server/routes/deviceMgmtRoutes.ts`: `/api/devices`・`/api/devices/revoke`
- [x] Bearer 解決を `dispatchRoute` へ注入（`httpHelpers.resolveRequestUser` = Cookie→Bearer）。ネイティブ Bearer は Origin 無しのため既存 CSRF 判定で自然に通る
- [x] `public/*`: `/device` 承認ページ + 「接続端末」管理 UI
- [x] パスワード変更時に当該ユーザーの desktop_tokens も失効（`revokeAllDesktopTokensForUser`。アカウント削除は FK CASCADE）
- [x] 監査ログ（`desktop.token_issue` / `desktop.token_revoke` / `desktop.device_approve`。トークン本体は記録しない）

### Phase 1 — WS チャット（バックエンド）— ✅ 実装済み（2026-06-23、未デプロイ）
- [x] WS ライブラリ採否の承認 → **`ws` 追加で確定**（2026-06-23。[backend_api.md §2.1](backend_api.md)）。`package.json` 追加＋architecture_v2 §0.6 依存リストへ追記済
- [x] `server.ts`: `httpServer.on("upgrade")` で `/ws/chat` を Bearer 認証＋`?botId=` 所有/共有検証して受理（否なら 401/403）
- [x] `server/chatWebSocket.ts`: **botId 束縛**の接続管理・keepalive(ping/pong 30s)・1接続1ターン直列・**`DESKTOP_MAX_UPLOAD_MB`(既定20) 上限適用**（`ready.maxUploadMb` で配布・超過は `too_large`）・バックプレッシャ（未処理ターン上限）
- [x] `services/chatChannelService.ts`: `ChatMessage` 組立 → `processMessage(束縛botId, …)` → 進捗/結果を WS フレーム化（`onStatusChange→status`, `asyncDelivery→interim/push`）
- [x] `serializeRich`（EmbedBuilder.toJSON + files base64）
- [x] `hasBotAccess(userId, botId)`（接続束縛 botId の検証）・`ready` で `bot`/`bots` 返却・Gemini キー未設定の `no_gemini_key`（`getUserGenAI` で事前判定）
- [x] レート制限（`consumeRateLimit` 流用）・エラーモデル
- [x] デプロイ: リバースプロキシの WS upgrade 透過設定（[deployment.md](../../guide/deployment.md) 追記）

> **注**: バックエンド Phase 0/1 は `feature/desktop-client` に実装済み（tsgo クリーン・SQL 検証済み）。**未デプロイ・実クライアント未接続**。`primary` フラグはプライマリ Bot の概念が未導入のため `id === "system_default"` を暫定採用。

### Phase 2 — クライアント骨格
- [ ] `clients/desktop/` Cargo プロジェクト・`profile.release` 最小化・URL 焼き込み
- [ ] eframe 起動 + トレイ(`tray-icon`) + ホットキー(`global-hotkey`)
- [ ] `os/` 抽象 + `os/windows.rs`（keyring/自動起動/オーバーレイ補完/**単一インスタンスロック(botId キー)**）
- [ ] **起動引数** `--bot <id>` / `--hidden` 解釈（`config.rs`）・botId 決定（引数→前回→主 Bot）
- [ ] `net/auth.rs`（デバイスフロー + keyring 保存）
- [ ] `net/ws.rs`（tokio-tungstenite, **`?botId=` 束縛**, 再接続/keepalive, **Bot 切替=再接続**, mpsc 連携）
- [ ] `ui/chat.rs`（履歴/入力/MD/ステータス/**Bot 切替セレクタ**）+ `ui/login.rs`
- [ ] `ui/overlay.rs`（**オーブ=Bot アイコン + 通知バッジ**、クリック透過、ドラッグ移動）
- [ ] `model.rs`（WS プロトコル serde、[backend_api.md §3](backend_api.md) と一致）

### Phase 3 — 画像
- [ ] 添付 UI（貼付 `arboard` / 選択 / D&D）+ サムネ + 取り消し + リサイズ
- [ ] 受信画像/embed/グラフのインライン表示（`egui_extras`）

### Phase 4 — 音声
- [ ] `audio/record.rs`（cpal 録音 → opus/ogg、WAV フォールバック）
- [ ] 録音 UI（インジケータ・上限・自動/確認送信設定）

### Phase 5 — オーバーレイ仕上げ + 配布
- [ ] オーブ/クリック透過/最前面/マルチモニタ/DPI の作り込み（実機）
- [ ] 設定画面 + 自動起動
- [ ] release 最小化計測（RAM/CPU/サイズ）
- [ ] Web ダウンロード導線 + CI（Windows release）

---

## 3. ファイル所有マップ追記案（architecture_v2 §10）

並行実装時の競合防止。新モジュール **desktopclient** を追加する想定（実装着手時に [architecture_v2.md §10](../../architecture/architecture_v2.md) へ反映）:

| モジュール | 所有ファイル |
|---|---|
| desktopclient | `server/routes/deviceAuthRoutes.ts`, `server/routes/deviceMgmtRoutes.ts`, `server/chatWebSocket.ts`, `services/desktopAuthService.ts`, `services/chatChannelService.ts`, `db/desktopTokenRepo.ts`, `clients/desktop/**` |
| 統合（既存・このために触る） | `server.ts`(upgrade), `server/routeRegistry.ts`(Bearer 解決), `db/migrations.ts`(v13), `public/*`(/device・端末管理・DL), `db/redis.ts`(device_auth 一時保存) |

> **重要**: `gemini.ts` / `turnPlanner.ts` / `llmClient.ts` / `functions/*` は**触らない**（会話コアは無改修で再利用）。トークン逐次表示を入れる将来フェーズでのみ `gemini.ts` に最終応答ストリーミングの小改修が入る。

---

## 4. 見積りの目安（粗）

実装規模の相対感（人日ではなく相対ポイント）:

| Phase | 規模感 | 主リスク |
|---|---|---|
| 0 認証 | 中 | デバイスフローのステート/失効・Bearer 解決の既存ルートへの差し込み |
| 1 WS | 中 | WS 依存採否・upgrade 配線・プロキシ設定・直列/バックプレッシャ |
| 2 骨格 | 中〜大 | egui/tokio スレッド連携・常駐/ホットキー・デバイスフロー UI |
| 3 画像 | 小 | 画像ローダ・クリップボード |
| 4 音声 | 中 | cpal デバイス差・Opus エンコードの実機検証 |
| 5 仕上げ | 中 | オーバーレイの OS 細部・配布/CI |

最小の縦切り（end-to-end が動く最小）は **Phase 0 → 1 → 2（テキストのみ）**。ここまでで「ログインして文字で会話」が成立する。画像/音声/仕上げはその上に積む。

---

## 5. オープンクエスチョン

### 決定済み（2026-06-23 オーナー合意）
- ✅ **WS 依存**: `ws` を追加で確定。
- ✅ **会話相手モデル**: **1 プロセス＝1 Bot＝1 WS（選択 Bot のみ）**。モーダル表示中に Bot 切替可（＝WS 再接続）。複数 Bot は**複数起動**で対応。デスクトップ専用の仮想 Bot は作らない（既存 Bot を使い Discord と記憶共有）。
- ✅ **オーバーレイ意匠**: オーブ＝接続中 Bot のアイコン。通知時は右上に数字バッジ。
- ✅ **デスクトップトークン**: TTL **既定 90 日**・アクセス毎スライディング延長。失効＝**端末単位 revoke（Web ダッシュボード）**＋パスワード変更時に当該ユーザー全失効。
- ✅ **大型添付の上限**: **サーバー管理者が設定**（`DESKTOP_MAX_UPLOAD_MB`、**既定 20MB**）。超過は `error{code:"too_large"}`。REST マルチパートアップロード移行は将来（同じ上限値を閾値に使う）。
- ✅ **コード配置**: **モノレポ** `clients/desktop/`。
- ✅ **ホットキー / 複数起動の衝突回避**: グローバルホットキーは**プライマリ Bot のインスタンスのみが登録（吸う）**。サブインスタンスは登録せず衝突回避。Bot 一覧/切替はプライマリのモーダルに集約し、ホットキー→プライマリのモーダル表示。サブはオーブのクリック/トレイで開閉。
- ✅ **TTS（音声出力）**: v1 非対応。別途予定の**音声クライアント開発**が落ち着き次第追従（バックエンド TTS 増設が前提）。

### 実機で検証しながら詰める
- オーバーレイのクリック透過/最前面が eframe 標準でどこまで足りるか（不足分のみ `windows-rs`。[client_design.md §4.2](client_design.md)）。Windows 10/11・マルチモニタ・DPI スケールで確認。
