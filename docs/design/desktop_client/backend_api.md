# Yuuka Desktop — バックエンド API 増設仕様

対象: バックエンド（Node モノリス）への増設。提案段階（未実装）。
前提: [architecture.md](architecture.md)。（旧 Node 実装時点の規範 [architecture_v2.md](../../architecture/architecture_v2.md) は履歴資料で、現行の規範ではない。本書が想定する Node モノリスは撤去済みで、現行のバックエンドは Rust（`crates/yuuka-supervisor/src/ws.rs`・`crates/yuuka-auth`）。詳細は [index.md](index.md) の注記を参照。本文中の `src/*.ts` へのリンクは切れている。）

この章は **クライアント非依存の汎用チャット API** を定義する。Discord にも Windows にも縛られない（将来の他クライアントも同じ API を使う）。

---

## 0. 増設サマリ（差分一覧）

| 種別 | 追加物 | 認可 |
|---|---|---|
| ルート | `POST /api/auth/device/code` | none |
| ルート | `POST /api/auth/device/token` | none（device_code で認可） |
| ルート | `POST /api/auth/device/approve` | user（ブラウザのセッション） |
| Web | `GET /device`（承認ページ・SPA 内） | user |
| ルート | `GET /api/devices` / `POST /api/devices/revoke` | user（端末管理） |
| WS | `GET /ws/chat`（HTTP upgrade） | Bearer（desktop token） |
| サーバ基盤 | `node:http` の `upgrade` イベント処理 | — |
| サービス | `services/desktopAuthService.ts`（デバイスフロー状態 + トークン発行/検証/失効） | — |
| サービス | `services/chatChannelService.ts`（WS ↔ `processMessage` アダプタ） | — |
| DB | `desktop_tokens` 表（v13 冪等追加） | — |
| Repo | `db/desktopTokenRepo.ts` | — |

> **ファイル所有マップ追記案**（[architecture_v2 §10](../../architecture/architecture_v2.md)）: 新モジュール **desktopclient** =
> `server/routes/deviceAuthRoutes.ts`, `server/routes/deviceMgmtRoutes.ts`, `server/chatWebSocket.ts`, `services/desktopAuthService.ts`, `services/chatChannelService.ts`, `db/desktopTokenRepo.ts`。
> 統合フェーズで触れるのは `server.ts`（upgrade 配線）, `db/migrations.ts`（v13）, `public/*`（/device・端末管理 UI）, `db/redis.ts`（デバイスコード一時保存）。**`gemini.ts` は触らない**（[roadmap.md](roadmap.md)）。

---

## 1. 認証: OAuth デバイスフロー（RFC 8628 型）

### 1.1 `POST /api/auth/device/code`（auth: none）

アプリ起動時、未ログインで呼ぶ。デバイスコードとユーザーコードを発行。

```
リクエスト: { client: "desktop", device_name?: string }   // device_name 例 "DESKTOP-ABC (Windows)"
レスポンス 200:
{
  "device_code": "<不透明・推測不能・サーバ保持>",
  "user_code": "WDJB-MJHT",                 // 人間可読・短い（ブラウザ確認用）
  "verification_uri": "https://yuuka.kawaii-music.moe/device",
  "verification_uri_complete": "https://yuuka.kawaii-music.moe/device?code=WDJB-MJHT",
  "interval": 5,                            // ポーリング最小間隔(秒)
  "expires_in": 600                         // 10分で失効
}
```

- 保存先: Redis `device_auth:{sha256(device_code)}` に `{ user_code, status:"pending", device_name, created_at }`、TTL=`expires_in`。Redis 不通時は in-memory フォールバック（セッションと同様）。
- `user_code` はレート制限つきで衝突回避・総当り耐性のため十分な空間（例 8 文字 Crockford Base32）。

### 1.2 `POST /api/auth/device/approve`（auth: user）

ブラウザ（既存セッション Cookie で認証済み）から、`/device` ページ経由で呼ぶ。

```
リクエスト: { user_code: "WDJB-MJHT" }
処理: user_code から device_auth レコードを引き、status を "approved" にし approved_user = ctx.user.discordId を記録
レスポンス 200: { success: true, device_name }   // 画面に「(端末名) を許可しました」
失効/不一致: { success:false, message } （404/410）
```

- **必ず `auth:"user"`**。承認はログイン済みユーザー本人の操作。CSRF は既存の Origin チェック（[routeRegistry](../../../src/server/routeRegistry.ts)）が効く。
- 任意強化: 承認前に「この端末を許可しますか？ 端末名: …」の確認 UI を `/device` に出す（フィッシング耐性）。

### 1.3 `POST /api/auth/device/token`（auth: none, device_code で認可）

アプリが `interval` 秒ごとにポーリング。

```
リクエスト: { device_code }
レスポンス:
  承認待ち   202/200: { error: "authorization_pending" }
  早すぎる   200:     { error: "slow_down" }            // interval を増やす
  期限切れ   410:     { error: "expired_token" }
  承認済み   200:     { access_token, token_type:"Bearer", expires_in, user:{discordId, username, role} }
```

- 承認済みを 1 回だけトークン化（device_code は使い切り、Redis から削除）。
- 発行トークンは §1.4 の `desktop_tokens` に登録。

### 1.4 デスクトップトークン（長命 Bearer）

- 生成: CSPRNG 生トークン。クライアントへは生で返し、サーバは **`sha256(token)` のみ保存**（セッションと同方針）。
- 保存表 `desktop_tokens`（§5）。検証は `desktopAuthService.verifyToken(token) → SessionUser | null`。
- TTL: 既定 90 日・アクセス毎スライディング延長（`last_used_at` 更新）。設定 `DESKTOP_TOKEN_TTL_DAYS`（任意）。
- 失効: ユーザーが Web から端末単位で revoke（§4）。パスワード変更時は当該ユーザーの全 desktop_tokens も失効（`destroyAllForUser` 相当を拡張）。
- クライアント保存: Windows 資格情報マネージャ（keyring）。平文ファイル禁止（NFR-4）。

### 1.5 Bearer 認証の組み込み

- 既存の `RouteAuth = "none"|"user"|"admin"` の `resolveUser()`（[routeRegistry](../../../src/server/routeRegistry.ts)）を拡張し、**`Authorization: Bearer <token>` があれば `desktopAuthService.verifyToken` で SessionUser を解決**する（Cookie が無くても通る）。これにより REST（`GET /api/bots` 等）も Bearer で利用可能。
- 優先順位: Cookie セッション → Bearer の順で解決（どちらかが成功すれば認証成立）。
- WS upgrade（§3）も同じ `verifyToken` を使う。
- ネイティブ Bearer は CSRF 非該当のため、Bearer 認証済みリクエストは Origin チェックを免除してよい（Cookie 認証時のみ Origin チェック）。

---

## 2. サーバ基盤: WebSocket 受け口の新設

現状 [`src/server.ts`](../../../src/server.ts) は素の `node:http` で `upgrade` を処理していない。WS を同一ポートで受けるため `server.on("upgrade", ...)` を追加する。

### 2.1 WebSocket ライブラリの方針（要承認の例外）

[architecture_v2 §0.6](../../architecture/architecture_v2.md) は「新規 npm 依存は原則禁止」。WebSocket 採用に伴い **`ws` の追加を承認済み**（2026-06-23 オーナー合意）。Node 定番・transitive 依存ゼロ・枯れており、`crawler`/`synapse` の Rust 同様「機能の核に必要な最小依存は許容」の線。実装着手時に [architecture_v2 §0.6](../../architecture/architecture_v2.md) の導入済み依存リストへ `ws` を追記すること。

> 参考（不採用）: 依存ゼロを死守するならミニマル手実装（RFC 6455 ハンドシェイク `Sec-WebSocket-Accept`=base64(sha1(key+GUID)) + フレーム parse/encode を `node:crypto` で自作）も可能だが、フラグメント/マスク/制御フレーム/バックプレッシャ（特にバイナリ＝画像/音声）の保守コストが大きいため `ws` を採る。

### 2.2 ルーティング

```ts
// server.ts（統合フェーズ）
httpServer.on("upgrade", (req, socket, head) => {
  const url = new URL(req.url ?? "", "http://x");
  if (url.pathname !== "/ws/chat") { socket.destroy(); return; }
  const user = resolveBearerUser(req);            // Authorization: Bearer
  if (!user) { socket.write("HTTP/1.1 401 Unauthorized\r\n\r\n"); socket.destroy(); return; }
  const botId = url.searchParams.get("botId");    // 接続時に 1 Bot へ束縛（1 プロセス 1 Bot）
  if (!botId || !userOwnsOrShares(user.discordId, botId)) {
    socket.write("HTTP/1.1 403 Forbidden\r\n\r\n"); socket.destroy(); return;
  }
  wss.handleUpgrade(req, socket, head, (ws) => chatWebSocket.onConnection(ws, user, botId));
});
```

- **`botId` は接続時に固定**（`/ws/chat?botId=...`）。クライアントは 1 接続＝1 Bot。サーバは「そのユーザーが所有/共有する Bot か」を検証し、否なら 403 で upgrade 拒否。
- 複数 Bot 同時利用は**クライアントの複数起動**（別プロセス＝別 WS）で実現する。サーバはユーザー×Bot ごとに独立した接続を保持できる。

---

## 3. WebSocket チャットプロトコル（`/ws/chat`）

改行や JSON テキストフレームを基本とし、メッセージは JSON。画像/音声は base64 を JSON に内包（実装簡素・Discord 経路と同じ `ChatMessage` 形）。**大きすぎる場合の上限と分割は §3.5**。

### 3.1 接続確立

- upgrade 時に Bearer 認証（§1.5）＋ `?botId=` の所有/共有検証（§2.2）。**接続は 1 Bot に束縛**される。
- 成功で `{type:"ready", user, bot:{...}, bots:[…], maxUploadMb}` を最初に送る。
  - `bot`: この接続が束縛された Bot（`id, name, discord_avatar_url, primary`）。クライアントはオーブのアイコンに使う。`primary` はプライマリ Bot 判定（ホットキー登録の可否に使う）。
  - `bots`: `GET /api/bots` 相当の一覧。**Bot 切替 UI（モーダル内）と「別オーバーレイで開く」導線**に使う。切替＝この WS を閉じ新 botId で再接続。
  - `maxUploadMb`: 添付上限（`DESKTOP_MAX_UPLOAD_MB`、既定 20）。クライアントは送信前にこの値で弾く。
- 認証失敗で 401、botId 不正で 403、いずれも upgrade 拒否。

### 3.2 クライアント → サーバ

`botId` は接続時に固定済みのため、各メッセージには含めない（サーバは接続束縛の botId を使う）。

```jsonc
// 発話
{
  "type": "msg",
  "text": "明日10時に歯医者リマインド",
  "image": { "mime": "image/png",  "data": "<base64>" },   // 任意
  "audio": { "mime": "audio/ogg",  "data": "<base64>" },   // 任意
  "replyToId": "<任意: 返信チェーン>"
}

// 会話リセット（コンテキストクリア。接続束縛の Bot に対して）
{ "type": "reset" }

// keepalive（任意。WS ping/pong を使うなら不要）
{ "type": "ping" }
```

> Bot を切り替えたい場合はメッセージで切り替えるのではなく、**WS を再接続**（`?botId=` を変えて張り直す）。これにより「1 接続 1 Bot」と「接続束縛 botId の検証」が単純に保たれる。

### 3.3 サーバ → クライアント

```jsonc
{ "type": "ready",
  "user": {...},
  "bot":  { "id":"...", "name":"...", "discord_avatar_url":"https://...", "primary":true },
  "bots": [{ "id":"...", "name":"...", "discord_avatar_url":"...", "primary":true }],
  "maxUploadMb": 20 }

{ "type": "status",  "state": "thinking" }      // onStatusChange("thinking")
{ "type": "status",  "state": "writing"  }      // onStatusChange("writing")

{ "type": "interim", "text": "やっておきます。" }  // asyncDelivery.onInterim（重い処理の一時応答）

{ "type": "token",   "delta": "…" }             // 将来: トークン逐次（v1 では送らない。architecture.md §6）

{ "type": "done",
  "messageId": "<server msg id>",
  "text": "リマインドを登録しました。",
  "embeds": [ { "title":"…", "description":"…", "color":3447003, "fields":[…], "image":{"name":"chart.png"} } ],
  "files":  [ { "name":"chart.png", "mime":"image/png", "data":"<base64>" } ],
  "deferred": false
}

// 重い処理の最終結果（done で deferred:true を返した後、完了時に届く）
{ "type": "push",
  "text": "ブラウザ操作が完了しました。", "embeds": [...], "files": [...] }

{ "type": "error", "code": "no_gemini_key" | "rate_limited" | "internal" | "unauthorized",
  "message": "Gemini APIキーが未設定です。Web で設定してください。" }
```

### 3.4 サーバ側ハンドラ（`chatChannelService` の中核・擬似コード）

```ts
// botId は接続確立時に束縛済み（onConnection(ws, user, botId)）。メッセージごとには受け取らない。
async function handleMsg(ws, user, botId, m) {
  const chat: ChatMessage = {
    text: m.text ?? "",
    imageData: m.image ? { data: m.image.data, mimeType: m.image.mime } : undefined,
    audioData: m.audio ? { data: m.audio.data, mimeType: m.audio.mime } : undefined,
    replyToMsgId: m.replyToId,
  };
  const asyncDelivery: TurnAsyncDelivery = {
    onInterim: (t) => wsSend(ws, { type: "interim", text: t }),
    deliverFinal: async (p) => wsSend(ws, { type: "push", ...serializeRich(p) }),
  };
  try {
    const r = await processMessage(
      botId, user.discordId, chat,
      (s) => s !== "idle" && wsSend(ws, { type: "status", state: s }),   // onStatusChange → status
      asyncDelivery,
    );
    wsSend(ws, { type: "done", ...serializeRich(r) });
  } catch (e) {
    wsSend(ws, { type: "error", code: classify(e), message: userMessage(e) });
  }
}
```

- **`processMessage` は無改修**。Discord 経路（`bot.ts`）の `asyncDelivery` 組み立てと完全に同型（[`src/bot.ts:594`](../../../src/bot.ts)）で、送り先だけ Discord → WS に替える。
- `notifier.ts` は通さない（重い処理の最終結果も WS `push` で返す）。
- `serializeRich` は [architecture.md §7](architecture.md)。

### 3.5 制限・keepalive・バックプレッシャ

| 項目 | 方針 |
|---|---|
| 1 メッセージ上限 | **サーバー管理者が設定**（`DESKTOP_MAX_UPLOAD_MB`、**既定 20MB**。画像/音声 base64 込）。超過は `error{code:"too_large"}`。`ready` フレームで上限値をクライアントへ通知し、送信前にローカルでも弾く。大型は将来 REST マルチパートアップロード→参照 ID 方式へ（同上限を閾値に） |
| 同時処理 | 1 接続あたり 1 ターン直列（処理中の新規 `msg` は待機 or 拒否） |
| keepalive | WS ping/pong（例 30s）。無応答でクローズ。クライアントは指数バックオフ再接続 |
| アイドルタイムアウト | 接続は維持（プッシュ受信のため）。長時間無通信でも ping で保持 |
| レート制限 | ユーザー単位（`botRateLimit` の方針を流用）。`error{code:"rate_limited"}` |
| バックプレッシャ | base64 大型送信時の `ws.bufferedAmount` 監視。閾値超で送出抑制 |

---

## 4. 端末管理 API（Web ダッシュボードに集約）

管理は Web に集約する方針に従い、端末（desktop トークン）の閲覧/失効は Web で行う。

```
GET  /api/devices            (auth: user) → [{ id, device_name, created_at, last_used_at, current? }]
POST /api/devices/revoke     (auth: user) { id }  → { success }   // sha256 行を無効化
```

- Web ダッシュボードに「接続端末」セクションを追加（`src/public/*`、統合フェーズ）。失効後、当該端末の WS/REST は次回 401。
- 監査: 端末発行/失効を `auditRepo.addAuditLog`（トークン本体は記録しない）。

---

## 5. DB スキーマ追加（v13）

[`src/db/migrations.ts`](../../../src/db/migrations.ts) に `migrateToDesktopTokens` を追加（現 `SCHEMA_VERSION="12"` → **"13"**）。既存テーブル不変・冪等追加（後方互換）。

```sql
CREATE TABLE IF NOT EXISTS desktop_tokens (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id      TEXT NOT NULL,                 -- Discord ユーザーID（データ分離キー）
  token_hash   TEXT NOT NULL UNIQUE,          -- sha256(生トークン)
  device_name  TEXT,
  created_at   TEXT NOT NULL DEFAULT (datetime('now','localtime')),
  last_used_at TEXT,
  revoked      INTEGER NOT NULL DEFAULT 0,
  FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_desktop_tokens_user ON desktop_tokens(user_id);
```

- `desktop_tokens` の `user_id` は **Web 登録済みユーザー**のみ（デスクトップ利用＝ログイン必須なので users への FK は妥当。汎用モードの未登録 ID とは無関係）。
- デバイスコードの一時状態は Redis（揮発で十分）。永続化するのは発行済みトークンのみ。

---

## 6. セキュリティ要件（不変条件の継承）

| # | 要件 |
|---|---|
| 1 | **データ分離**: WS/REST の全処理はトークン由来 `user_id` でスコープ。`processMessage(botId, userId, …)` の `userId` は必ずトークンから（クライアント申告を信用しない）。 |
| 2 | **認証情報非露出**: 返答（`done`/`push`）に秘匿値を入れない（既存 `secretGuard`/Function 戻り値規約を継承。会話コアが既に担保）。 |
| 3 | **トークン保護**: サーバは `sha256` のみ保存。クライアントは OS 資格情報ストア保存・TLS 必須。端末単位失効。 |
| 4 | **認可境界**: `device/approve` は必ずログイン済みユーザー本人（`auth:"user"`）。`device/code`・`device/token` は device_code 自体が capability。 |
| 5 | **Gemini キー**: 秘書モードはユーザー自身のキー。未設定は `error{code:"no_gemini_key"}` で Web 設定へ誘導（鍵をアプリで扱わない）。 |
| 6 | **総当り耐性**: `user_code` 承認・`device/token` ポーリングにレート制限。`user_code` 空間は十分大きく短命（10 分）。 |
| 7 | **CSRF**: Bearer はアンビエント資格情報でないため非該当。Cookie 認証経路（`/device` 承認）は既存 Origin チェックを維持。 |

---

## 7. エラーモデル（クライアント挙動）

| code | 意味 | クライアント挙動 |
|---|---|---|
| `unauthorized` | トークン無効/失効 | 保存トークン破棄 → デバイスフロー再実行 |
| `no_gemini_key` | ユーザーの Gemini キー未設定 | 「Web で Gemini キーを設定」導線を表示 |
| `rate_limited` | レート超過 | バックオフして再送 |
| `too_large` | ペイロード超過 | 画像縮小/音声短縮を促す |
| `internal` | サーバ内部エラー | 再試行を促す。詳細はログのみ |

WS 切断時はオフライン表示 + 指数バックオフ再接続。切断中のユーザー入力はローカル退避し再接続後に再送（NFR-6）。

---

## 8. サーバ設定（環境変数 / config.yaml）

**サーバー管理者が設定**する（クライアントには焼き込まない）。いずれも任意・既定値あり。

| 設定 | 既定 | 役割 |
|---|---|---|
| `DESKTOP_TOKEN_TTL_DAYS` | `90` | デスクトップトークンの TTL（日）。アクセス毎にスライディング延長 |
| `DESKTOP_MAX_UPLOAD_MB` | `20` | 1 メッセージの添付上限（画像/音声 base64 込, MB）。`ready` で配布し超過は `too_large` |
| `DESKTOP_DEVICE_CODE_TTL_SEC` | `600` | デバイスコードの有効期限（秒, §1.1 の `expires_in`） |

- 既存 `config.ts` の `getSetting()` 流儀で読み込む（環境変数 or `config.yaml`）。新規の必須環境変数は無し（全て既定値で動作）。
- 公開 URL（`verification_uri` 等）は既存 `baseUrl`（`BASE_URL`）を流用。
