# Yuuka Desktop — デスクトップオーバーレイクライアント 設計資料

『Yuuka』の対話機能（画像 / 音声 / テキスト入力 → 返答）を、Discord 以外の経路から使えるようにする**新クライアント開発**の第一弾。手始めに **Windows デスクトップ**向けの、常駐オーバーレイ型チャットクライアントを **Rust（最軽量志向）**で実装する。

> ⚠️ **本資料は `docs/design/` 配下の設計方針書である**（[../../index.md](../../index.md) の参照優先順位を参照）。現行コードへの拘束力は持たない。実装済みの範囲（バックエンドの Phase 0 認証・Phase 1 WS チャット）と食い違う場合は**現行コードが正**。
>
> ⚠️ **旧 Node 実装時点の記述を含みます:** 本資料（と `architecture.md` / `backend_api.md` / `client_design.md`）が参照する `src/gemini.ts`・`src/server.ts`・`src/bot.ts` 等は Node 実装（[#68](https://github.com/kawaii-music-moe/yuuka/pull/68) で撤去済み）のファイルで、現行リポジトリには存在しません（リンク切れを含む）。撤去前のソースは `git show 390df39^:<path>` で参照できます。現行の実装は、バックエンド = `crates/yuuka-supervisor/src/ws.rs`（`/ws/chat`）・`crates/yuuka-auth`（デバイス認証）・`crates/yuuka-orchestrator`（会話エンジン）、クライアント = `clients/desktop/`（Rust / egui）。`architecture_v2.md` への言及は履歴資料（旧 Node 実装の規範）を指す。

---

## 📂 資料構成

| ファイル | 内容 |
|---|---|
| [index.md](index.md) | このファイル（目次・前提・確定事項） |
| [requirements.md](requirements.md) | 要件定義（スコープ・非ゴール・機能/非機能要件・UX） |
| [architecture.md](architecture.md) | 全体アーキテクチャ（トポロジ・データフロー・既存資産の再利用方針） |
| [backend_api.md](backend_api.md) | バックエンド API 増設仕様（OAuth デバイスフロー・WebSocket チャットプロトコル・`processMessage` 統合・端末管理） |
| [client_design.md](client_design.md) | egui クライアント設計（crate 構成・オーバーレイ/トレイ/ホットキー・音声録音・画像・配布） |
| [roadmap.md](roadmap.md) | 段階導入ロードマップ・タスク分解・ファイル所有マップ追記案 |

---

## 🎯 このクライアントは何か（1 段落）

タスクトレイに常駐し、画面上の小さなオーバーレイ（オーブ）をクリック（またはグローバルホットキー）すると小さなチャットモーダルが開く。ユーザーは**テキスト・画像・音声**で話しかけ、Yuuka（早瀬ユウカ）が返答する。会話内容は Discord で同じ Bot に DM したときと**同一の会話コンテキスト・記憶・ツール群**を共有する。**管理機能（タスク/家計/設定/Bot 管理 等）はデスクトップに載せず、すべて Web ダッシュボードに集約**する。デスクトップは「会話するだけの軽量フロントエンド」に徹する。

---

## ✅ 確定済みの設計判断（2026-06-23 ユーザー合意）

| 論点 | 決定 | 理由 |
|---|---|---|
| 言語 / プラットフォーム | **Rust / Windows**（まず手始め。将来 mac/Linux・他クライアントへ展開） | 軽量・単一バイナリ |
| UI 描画基盤 | **egui (eframe)** | 純 Rust・最軽量（単一 exe、RAM 最小）。オーバーレイ（透明・最前面・クリック透過）が容易 |
| 会話の応答方式 | **WebSocket（双方向）** | トークン/ステータスの逐次配信＋重い処理の非同期完了プッシュを 1 本で扱える。既存の `onStatusChange` / `asyncDelivery` 設計と相性◎ |
| WS 実装の依存 | **`ws` を追加（承認済み）** | Node 定番・transitive 依存ゼロ。新規 npm 依存禁止方針への意図的な例外として承認 |
| 認証方式 | **ブラウザ OAuth デバイスフロー**（RFC 8628 型） | アプリにパスワードを入力させない。本番は公開 URL を焼き込み、ブラウザでログイン→アプリへトークン受領 |
| 会話相手モデル | **1 プロセス = 1 Bot = 1 WS 接続**。**複数起動を許可**（Bot ごとに別プロセス） | WS は選択中の Bot のみに接続。ポップアップ表示中に Bot 切替可（切替＝WS 再接続）。複数 Bot を同時に使うなら複数インスタンス起動 |
| オーバーレイ意匠 | **オーブ＝Bot アイコン**（`discord_avatar_url`）。**通知時は右上に数字バッジ** | 1 プロセス 1 Bot なのでオーブがどの Bot かを表す。未読/完了プッシュ件数をバッジ表示 |
| ホットキー | **プライマリ Bot のインスタンスのみがグローバルホットキーを登録（吸う）。サブは登録しない** | 複数起動時の衝突回避。Bot 一覧/切替はプライマリのモーダルに集約。サブはオーブクリック/トレイで開閉 |
| デスクトップトークン | TTL **既定 90 日**・スライディング延長。**端末単位失効**＋パスワード変更で全失効 | [backend_api.md §1.4](backend_api.md) |
| 大型添付の上限 | **サーバー管理者が設定**（`DESKTOP_MAX_UPLOAD_MB`、**既定 20MB**） | 超過は `too_large`。REST アップロード移行は将来（同上限を閾値に） |
| TTS（音声出力） | **v1 非対応**。別途予定の**音声クライアント**が落ち着き次第追従 | バックエンド TTS 増設が前提 |
| コード配置 | **モノレポ** `clients/desktop/` | プロトコル資料と近接・版管理一元化 |
| 配布 | 本番は**公開 URL を焼き込み、Web ダッシュボードからダウンロード** | 管理を Web に集約する方針と一貫 |
| 管理機能 | **デスクトップには載せない**（Web のみ） | 同上 |

---

## 🔑 最重要の前提（調査で判明した事実）

1. **会話用の HTTP API は現状存在しない。** 会話はすべて Discord ゲートウェイ経由（`src/bot.ts` → `src/gemini.ts`）。→ **デスクトップ用に新しいチャット API を増設するのが本プロジェクト最大のバックエンド作業。**
2. **会話エンジンのコアは既に Discord 非依存。** [`src/gemini.ts:926`](../../../src/gemini.ts) の `processMessage(botId, userId, message: ChatMessage, onStatusChange?, asyncDelivery?)` は `ChatMessage`（`text` + `imageData{base64,mime}` + `audioData{base64,mime}`）を受け取り、`ProcessResult` を返す純粋な関数。**画像・音声は Gemini ネイティブのマルチモーダルで処理**（外部 STT 不要）。
3. **障壁は「入口」と「出口」だけ。** Discord 密結合は ① メッセージ受信（`bot.ts` の `setupMessageListener`）と ② 返答送信（`notifier.ts` の `sendToUser`）に集中している。コアの会話ロジック（`gemini.ts` / `turnPlanner.ts` / `llmClient.ts` / Function 群）は**無改修で再利用できる**。
4. **TTS（音声出力）は現状バックエンドに無い。** v1 では「音声入力 → テキスト返答」。音声で読み上げる返答は将来フェーズ（[roadmap.md](roadmap.md) 参照）。
5. **WebSocket はサーバに未実装。** [`src/server.ts`](../../../src/server.ts) は素の `node:http` で `upgrade` を処理していない。WS 受け口の新設が必要。

---

## 🗺️ 全体像（30 秒）

```
┌────────────────────────┐         wss://yuuka.kawaii-music.moe/ws/chat
│ Yuuka Desktop (Rust)   │  ──────────────────────────────────────────►  ┌──────────────────────┐
│ ・トレイ常駐 + オーバーレイ │   {type:'msg', text, image?, audio?}            │ Yuuka Backend (Node) │
│ ・egui チャットモーダル    │  ◄──────────────────────────────────────────  │                      │
│ ・cpal 音声録音           │   {status}/{token}/{done}/{push}               │  新設:               │
│ ・OAuth デバイスフロー     │                                                │  ・/ws/chat (WS)      │
└────────────────────────┘   初回: ブラウザで OAuth → Bearer トークン       │  ・OAuth デバイスフロー │
            │                                                               │  ・desktop_tokens 表  │
            │  GET /api/bots 等（REST, Bearer）                              │                      │
            └──────────────────────────────────────────────────────────►  │  無改修で再利用:       │
                                                                            │  ・processMessage()   │
                                                                            │  ・gemini/turnPlanner │
                                                                            │  ・Function/MCP 群    │
                                                                            └──────────────────────┘
```

詳細は [architecture.md](architecture.md) を参照。
