# Yuuka Desktop — アーキテクチャ

対象読者: 実装エンジニア / AI 実装エージェント。提案段階（未実装）。
前提: [index.md](index.md)・[requirements.md](requirements.md)。

---

## 1. 設計原則

1. **デスクトップは薄いフロントエンド**。思考・記憶・ツール実行・暗号・DB は一切持たず、すべてバックエンドへ委譲する。
2. **クライアント非依存の汎用チャット API を新設**する。Discord 専用でも Windows 専用でもない「チャンネル中立」な入口を作り、将来の他クライアント（mac/Linux/モバイル/Web チャット）も同じ API を使う。
3. **会話コアは無改修で再利用**。`processMessage()` は既に Discord 非依存なので、新チャンネルは「`ChatMessage` を作って `processMessage` を呼び、`ProcessResult` と進捗コールバックを WS に流す」薄いアダプタとして実装する。
4. **既存の不変条件を跨プロセスで維持**（[architecture_v2 §0](../../architecture/architecture_v2.md)）。全 API はトークン由来 `user_id` でスコープ。認証情報は返さない。
5. **障害分離＝デグレード**。WS 切断・バックエンド不通でアプリは落ちず、再接続と再送で回復する。

---

## 2. システムトポロジ

```
                              【デスクトップ端末 (Windows)】
┌──────────────────────────────────────────────────────────────────────┐
│ Yuuka Desktop (Rust / 単一 exe)                                         │
│                                                                        │
│  ┌────────────┐  ┌─────────────┐  ┌──────────────┐  ┌──────────────┐   │
│  │ Tray/Hotkey│  │ Overlay(egui)│  │ Chat Modal    │  │ Audio (cpal) │   │
│  │ 常駐・呼出  │  │ 透明・最前面  │  │ 入力/表示/MD  │  │ 録音→ogg/opus │   │
│  └────────────┘  └─────────────┘  └──────┬───────┘  └──────┬───────┘   │
│         │                                 │                 │           │
│         └───────────────┬─────────────────┴─────────────────┘           │
│                         ▼                                                │
│            ┌──────────────────────────────┐   keyring → Windows         │
│            │ Net Layer                     │   Credential Manager        │
│            │ ・WS client (tungstenite)     │   （Bearer トークン保存）     │
│            │ ・REST client (reqwest)       │                             │
│            │ ・OAuth device flow + 'open'  │                             │
│            └──────────────┬───────────────┘                             │
└───────────────────────────┼────────────────────────────────────────────┘
                            │ wss:// (チャット) / https:// (REST・認証)
                            │ Authorization: Bearer <desktop token>
                            ▼  ── リバースプロキシ (TLS 終端 / WS upgrade 透過) ──
┌──────────────────────────────────────────────────────────────────────┐
│ Yuuka Backend (Node, 既存モノリス / コンテナ PORT 7854)                  │
│                                                                        │
│  【新設（薄いアダプタ層）】                                              │
│  ┌────────────────────┐  ┌──────────────────────┐  ┌────────────────┐ │
│  │ OAuth デバイスフロー  │  │ /ws/chat  WS ハンドラ  │  │ desktop_tokens │ │
│  │ (device/code/token) │  │ ・Bearer 認証          │  │ 表 + Repo      │ │
│  │ + Web 承認ページ      │  │ ・ChatMessage 組立     │  │ (v13 migrate)  │ │
│  │ /device              │  │ ・進捗→WSフレーム化     │  │ 端末管理 API    │ │
│  └────────────────────┘  └───────────┬──────────┘  └────────────────┘ │
│                                       │ 呼ぶだけ（無改修）               │
│  【再利用（変更なし）】                ▼                                  │
│  processMessage(botId,userId,ChatMessage,onStatusChange,asyncDelivery) │
│   → gemini.ts / turnPlanner.ts / llmClient.ts / Function 群 / MCP      │
│   → message_logs(SQLite) / Redis ctx / synapse / Gemini API           │
└──────────────────────────────────────────────────────────────────────┘
```

---

> **1 プロセス = 1 Bot = 1 WS 接続。** 各インスタンスは起動時に 1 つの Bot に束縛され、その Bot のみに WS 接続する（オーブはその Bot のアイコンを表示）。**複数 Bot を同時に使う場合は複数インスタンスを起動**（Bot ごとに別プロセス・別オーバーレイ）。同一 Bot の二重起動は抑止する（§8.1）。

## 3. 既存資産の再利用マップ

| 区分 | 対象 | 扱い |
|---|---|---|
| **無改修で再利用** | [`processMessage()`](../../../src/gemini.ts) / `gemini.ts` 全体 / `turnPlanner.ts` / `llmClient.ts` / `functions/*` / `mcpClient` / `message_logs` / synapse / Redis ctx | そのまま呼ぶ。チャンネルに依存しない |
| **既存型を再利用** | `ChatMessage`・`ProcessResult`・`TurnAsyncDelivery`（[`src/types/contracts.ts`](../../../src/types/contracts.ts)） | WS ハンドラが組み立て/受け取る |
| **新設（薄いアダプタ）** | `/ws/chat` ハンドラ / OAuth デバイスフロー / `desktop_tokens` 表・Repo / 端末管理ルート | [backend_api.md](backend_api.md) |
| **新設（サーバ基盤）** | `node:http` の `upgrade` 受け口（WebSocket） | [backend_api.md §2](backend_api.md) |
| **新設（Web）** | `/device` 承認ページ / 端末管理 UI / ダウンロード導線（`src/public/*`） | 統合フェーズ |
| **使わない（バイパス）** | `notifier.ts`（Discord 送信）/ `bot.ts` の `setupMessageListener` | デスクトップは WS が出入口。`notifier` は通さない |

> **要点**: Discord 密結合は「入口（`bot.ts`）」と「出口（`notifier.ts`）」の 2 点のみ。新チャンネルはこの 2 点を WS で置き換えるだけで、思考エンジンには触れない。

---

## 4. 認証フロー（OAuth デバイスフロー / RFC 8628 型）

アプリにパスワードを入力させない。ログインはブラウザ（Web 側、既存のセッション認証）で行う。

```
[アプリ起動・未ログイン]
  1. アプリ → POST /api/auth/device/code            （Bearer 不要）
       ← { device_code, user_code, verification_uri_complete, interval, expires_in }
  2. アプリが 'open' クレートで verification_uri_complete を既定ブラウザで開く
       例: https://yuuka.kawaii-music.moe/device?code=WDJB-MJHT
  3. [ブラウザ] ユーザーが既存のログイン（/api/login のセッション Cookie）で認証済みなら
       /device ページに user_code が表示され「この端末を許可」ボタン → POST /api/auth/device/approve
       （未ログインならまずログイン画面へ誘導）
  4. アプリは interval 秒ごとに POST /api/auth/device/token { device_code } をポーリング
       ← 承認前: { error:"authorization_pending" } / 承認後: { access_token, token_type:"Bearer", user:{discordId,username} }
  5. アプリは access_token を Windows 資格情報マネージャ（keyring）へ保存
  6. 以後、WS/REST で Authorization: Bearer <access_token>
```

- **発行されるトークン**は長命（既定 TTL 例 90 日・スライディング）。`desktop_tokens` 表に `sha256` で保存し、端末名とともに管理。Web ダッシュボードから**端末単位で失効**できる。
- 失効/期限切れ時、WS/REST が 401 を返す → アプリは保存トークンを破棄し再度デバイスフローへ。
- ネイティブクライアントは CORS の対象外（ブラウザではない）。トークンはアンビエント資格情報（Cookie）ではない **Bearer** なので **CSRF 非該当**。WS upgrade で Origin チェックは課さず、Bearer のみで認証する。

詳細なエンドポイント仕様・ステートマシン・セキュリティは [backend_api.md §1](backend_api.md)。

---

## 5. 会話のデータフロー（1 ターン）

```
[デスクトップ]                         [/ws/chat ハンドラ]                 [会話コア（無改修）]
 入力(text/image/audio) ──msg──►  Bearer 検証 → userId 解決
                                   botId = 接続時に固定（?botId=）
                                   所有/共有 Bot か検証
                                   ChatMessage 組立                 processMessage(
                                     {text, imageData?,             botId, userId, msg,
                                      audioData?}                    onStatusChange,──┐ "thinking"/"writing"
                                   )                                 asyncDelivery)   │
        ◄──status──────────────  onStatusChange を                                  │
        ◄──status──────────────  {type:"status"} へ ◄───────────────────────────────┘
                                   ……（Function Call ループ）……
        ◄──done(reply, rich)───  ProcessResult を                  return {text, embeds, files}
                                   {type:"done"} へ（embeds/files
                                   は §7 で desktop 用に直列化）
   ── 重い処理（deferred）の場合 ──
        ◄──interim(一時応答)────  asyncDelivery.onInterim →
                                   {type:"interim"}
        ◄──push(最終結果)───────  asyncDelivery.deliverFinal →
                                   {type:"push"}（モーダル閉でもトレイ通知）
```

- `botId`: デスクトップは秘書モードで会話する。**botId は WS 接続時に固定**（`/ws/chat?botId=...`）し、その接続では当該 Bot のみと会話する（クライアント申告の botId はサーバが「そのユーザーが所有/共有する Bot か」を検証）。コンテキストキャッシュキー `context:{botId}:secretary:{userId}` を Discord DM と共有 → **Discord と会話が連続**する。
- **Bot 切替**: モーダル表示中に別 Bot を選ぶと、現 WS を閉じ新 botId で再接続する（オーブのアイコンも切替先 Bot に変わる）。1 プロセスで同時に複数 Bot へは繋がない。
- **複数 Bot 同時利用**: 複数インスタンスを起動（1 プロセス 1 Bot）。各プロセスが独立した WS・オーバーレイ・通知バッジを持つ。
- `userId`: Bearer トークンから解決した Discord ユーザー ID。全データ分離キー。
- 進捗 `onStatusChange("thinking"|"writing"|"idle")` を WS `status` フレームへ。重い処理は `TurnAsyncDelivery`（`onInterim`/`deliverFinal`）を WS `interim`/`push` フレームへ。`notifier.ts`（Discord）は**通さない**。

WS フレームの完全な型定義は [backend_api.md §3](backend_api.md)。

---

## 6. ストリーミング方針（段階的）

`processMessage` は内部で Function Calling ループを回すため、**真のトークン逐次ストリーミング**（`generateContentStream`）は初期段階では導入しない。WebSocket を選んだ価値は以下で十分に発揮される:

| フェーズ | 体験 | 実装 |
|---|---|---|
| **v1（必須）** | 「考え中…/入力中…」のステータス逐次 + 最終メッセージ一括 + 重い処理の非同期完了プッシュ | 既存 `onStatusChange` / `asyncDelivery` を WS フレーム化するだけ。**`gemini.ts` 無改修** |
| **将来** | 最終応答のトークン逐次表示（タイプライタ風） | `gemini.ts` の最終応答生成を `generateContentStream` 化し、`onToken(delta)` コールバックを追加（会話コアに小改修。統合フェーズ） |

→ v1 は WS の双方向性を「ステータス + 非同期プッシュ」で活かし、トークン逐次は後付けで段階導入する。WebSocket を選んでおけば後付けがフレーム追加だけで済む。

---

## 7. リッチコンテンツの直列化（Discord → デスクトップ）

`ProcessResult.embeds` は discord.js の `EmbedBuilder`、`files` は PNG `Buffer`（グラフ等）。デスクトップは discord.js を持たないため、WS ハンドラで**デスクトップ中立な JSON へ直列化**する。

```ts
// WS ハンドラ内のアダプタ（擬似）
function serializeRich(r: ProcessResult) {
  return {
    text: r.text,
    embeds: r.embeds.map(e => e.toJSON()),         // {title, description, color, fields, image:{url}, ...}
    files: r.files.map(f => ({                      // チャート PNG 等
      name: f.name,
      mime: "image/png",
      data: f.attachment.toString("base64"),
    })),
    deferred: r.deferred,
  };
}
```

- Embed の `image: attachment://chart.png` 参照は、`files` の同名添付（base64）へ解決してインライン表示する（egui の `egui_extras` 画像ローダ）。
- `richReplyEnabled=false` のユーザーは `embeds/files` が空のため、デスクトップはテキストのみ描画（既存ゲートを踏襲）。

---

## 8. クライアント内部アーキテクチャ（egui）

詳細は [client_design.md](client_design.md)。要点のみ:

```
main (eframe) ── App 状態 (Arc<Mutex<…>> / channel)
  ├─ UI スレッド（egui 再描画。イベント駆動 = request_repaint_after / 待機時は描画停止）
  │    ├─ Overlay ビュー（オーブ / 透明・最前面・クリック透過）
  │    └─ Chat モーダルビュー（入力・履歴・MD・画像・録音ボタン）
  ├─ Net タスク（tokio runtime 別スレッド）
  │    ├─ WS クライアント（tokio-tungstenite）: 送受信を mpsc で UI と接続
  │    ├─ REST（reqwest）: /api/bots など
  │    └─ OAuth デバイスフロー
  ├─ Audio タスク（cpal）: 録音 → ogg/opus エンコード → base64
  └─ OS 統合（薄い抽象）
       ├─ tray-icon（常駐メニュー）
       ├─ global-hotkey（呼び出し）
       ├─ keyring（トークン保存 = Windows Credential Manager）
       └─ overlay 制御（透明/最前面/クリック透過 = eframe ViewportBuilder + 必要時 windows-rs）
```

- UI（egui）とネットワーク（tokio）は**別スレッド**。`mpsc`/`watch` チャネルで疎結合。UI はイベント受信時のみ再描画（NFR-3 省電力）。
- OS 依存は薄い抽象（trait）に隔離し、将来 mac/Linux に差し替え可能（NFR-7）。

### 8.1 複数起動モデル（1 プロセス 1 Bot）

- 各プロセスは**起動時に 1 つの botId に束縛**され、その Bot の WS・オーバーレイ・通知バッジを独立に持つ。
- **同一 Bot の二重起動を抑止**: botId をキーにした単一インスタンスロック（名前付き Mutex 等、`os/windows.rs`）を取得。既に動いていればそのオーバーレイをフォーカスして終了。**異なる botId なら同時起動可**。
- **起動時の botId 決定**: 起動引数 `--bot <id>`（トレイ/Web の「この Bot で開く」導線から渡す）→ 無ければ前回の Bot → 初回は `GET /api/bots` の主 Bot。
- **Bot 切替の 2 形態**: ① 同プロセスで切替＝WS 再接続（オーブ更新）。② 別 Bot を「新しいオーバーレイで開く」＝新プロセス起動。トレイメニューに Bot 一覧を出し両操作を提供。
- **ホットキーはプライマリのみ**: グローバルホットキーを登録するのは**プライマリ Bot のインスタンスだけ**（`ready.bot.primary` で判定。「プライマリが吸う」）。サブは登録せず衝突を回避し、オーブのクリック/トレイで開閉する。Bot 一覧/切替はプライマリのモーダルに集約。
- **オーブ意匠**: オーブ＝接続中 Bot の `discord_avatar_url`（REST で取得・キャッシュ）。**通知（未読返答・完了プッシュ）件数を右上に数字バッジ**。モーダル展開でクリア。

---

## 9. デプロイ/運用への影響

- **リバースプロキシ**: `/ws/chat` の WebSocket upgrade を透過する設定が必要（nginx なら `Upgrade`/`Connection` ヘッダ転送、長めの `proxy_read_timeout`）。[deployment.md](../../guide/deployment.md) に追記予定。
- **バックエンド**: WS 受け口の追加でプロセス常駐の WS 接続を保持する。接続数・アイドルタイムアウト・ping/pong（keepalive）を設計（[backend_api.md §3.4](backend_api.md)）。
- **スキーマ**: `desktop_tokens` を **v13** で冪等追加（既存不変、後方互換）。
- **設定**: 公開 URL は**クライアント側にビルド時焼き込み**。サーバ側は新規の必須環境変数なし（全て既定値で動作）。**サーバー管理者が任意設定**: `DESKTOP_TOKEN_TTL_DAYS`（既定 90）・`DESKTOP_MAX_UPLOAD_MB`（既定 20）・`DESKTOP_DEVICE_CODE_TTL_SEC`（既定 600）。詳細 [backend_api.md §8](backend_api.md)。

---

## 10. リスクと留意点

| リスク | 対応 |
|---|---|
| 「新規依存禁止」方針と WS ライブラリ追加の衝突 | 意図的な例外として承認を取る。代替はミニマル WS 手実装（[backend_api.md §2](backend_api.md)） |
| 真のトークンストリーミング未対応で体感が一括表示 | v1 はステータス逐次で体感を担保。トークン逐次は段階導入（§6） |
| オーバーレイのクリック透過 / 最前面は OS 依存が強い | eframe の機能で大半カバー。不足分のみ `windows-rs`（WS_EX_LAYERED/TRANSPARENT）で補完（[client_design.md](client_design.md)） |
| Gemini キー未設定ユーザー | 秘書モードはユーザーキー必須。未設定なら WS 接続時に明示エラーを返し Web 設定へ誘導 |
| 長命トークンの漏洩 | OS 資格情報ストア保存 + 端末単位失効 + TLS。平文ファイル保存禁止（NFR-4） |
| WS を介した未認証アクセス | upgrade 時に Bearer 必須・無効なら 401 で upgrade 拒否（[backend_api.md §1.5](backend_api.md)） |
