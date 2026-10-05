# Yuuka Desktop — WS プロトコル v2: Discord コンポーネント互換

対象: デスクトップ WS チャットに Discord の対話コンポーネント（ボタン/action row）と
インタラクション往復を追加する。スコープ = **汎用インフラ + 往復実証**
（既存システムDM[共有招待/メンバー申請/ペルソナ導入]のデスクトップ配信は次フェーズ）。

前提調査: コンポーネントは現状 `processMessage` ではなく `bot.ts` のシステムフローで
ハードコード生成され、`interaction.isButton()` のみ処理。種別は **ボタンのみ**
（セレクト/モーダル/コレクタ不使用）。`custom_id` は `action:id[:extra]`。

---

## 1. トランスポート表現（= Discord API JSON）

embed と同じ方針で **discord.js の `.toJSON()` が吐く Discord API JSON をそのまま運ぶ**。
これにより bot.ts 既存の `ActionRowBuilder` 資産をそのまま再利用でき、Rust 側も
Discord 標準形をパースするだけで将来のセレクト等にも拡張しやすい。

```
ActionRow = { type: 1, components: Component[] }
Button    = { type: 2, style: 1|2|3|4|5, label?: string,
              custom_id?: string, url?: string, emoji?: { name?: string }, disabled?: boolean }
  style: 1=Primary(青) 2=Secondary(灰) 3=Success(緑) 4=Danger(赤) 5=Link(url必須/custom_id無)
```

Rust は未知 `type`（セレクト等 type 3,5-8）を **serde で握りつぶしてスキップ**する
（`#[serde(other)]` 相当）。今は描画しないが受信で落ちないこと。

---

## 2. アウトバウンド（server → client）フレーム追加

`done` / `push` に任意 `components` を追加:

```jsonc
{ "type": "done", "messageId": "<uuid>", "text": "...", "embeds": [],
  "files": [], "components": [ActionRow, ...], "deferred": false }

{ "type": "push", "text": "...", "embeds": [], "files": [], "components": [ActionRow, ...] }
```

- `components` 省略可（後方互換: 旧クライアントは無視）。
- **`components` を含む場合 `messageId` は必須**（後述 interaction/update の突合キー）。
  `done` は既に `randomUUID()` 合成済み。`push` も components を載せる場合は messageId を付与する
  → `push` フレームに任意 `messageId` を追加。

新規アウトバウンド **`update`** フレーム（interaction の結果として元メッセージを書き換える。
Discord の `interaction.update()` 相当）:

```jsonc
{ "type": "update", "messageId": "<対象>", "text"?: "...",
  "embeds"?: [], "components"?: [ActionRow, ...] }
```

- クライアントは `messageId` で履歴中の該当メッセージを探し、存在するフィールドのみ差し替える。
- `components: []`（空配列）= **ボタンを除去**（押下後に無効化する典型動作）。
- ハンドラが「新規メッセージ追加」を選ぶ場合は従来どおり `push` を送る（update ではなく）。

---

## 3. インバウンド（client → server）フレーム追加

```jsonc
{ "type": "interaction", "messageId": "<元メッセージ>", "customId": "<button custom_id>" }
```

- `botId` は WS 接続で束縛済みのため不要。
- サーバはこれを **チャネル中立ディスパッチャ**へ渡す（次節）。

---

## 4. チャネル中立インタラクション・ディスパッチ（backend）

`bot.ts` の `handleInteraction`(376-532) の **アクション分岐ロジックを抽出**し、
新規 `src/services/componentInteractionService.ts` の中立関数へ移す。

```ts
export interface InteractionResponder {
  update(opts: { content?: string; components?: APIActionRowComponent[] }): Promise<void>;
  reply(opts: { content?: string; ephemeral?: boolean; components?: APIActionRowComponent[] }): Promise<void>;
  followUp(opts: { content?: string; components?: APIActionRowComponent[] }): Promise<void>;
}

export async function dispatchComponentInteraction(args: {
  userId: string;                 // Discord ID（WS は束縛トークン由来、Discord は interaction.user.id）
  customId: string;
  guildId?: string | null;        // DM/デスクトップでは null
  responder: InteractionResponder;
}): Promise<void>;
```

- 既存アクション（`share_accept/decline`・`memreq_approve/reject/apply`・`persona_import`）を
  この関数へ移植。`interaction.user.id` → `args.userId`、`interaction.guild?.id` → `args.guildId`、
  `interaction.update/reply/followUp` → `args.responder.*` に置換。
- **Discord 側**: `bot.ts` の `handleInteraction` は responder を discord.js Interaction から組み立てて
  `dispatchComponentInteraction` を呼ぶ薄いアダプタに縮小（挙動不変・回帰なきこと）。
- **WS 側**: `chatWebSocket.ts` の `interaction` フレーム受信で responder を組み立てる:
  - `update(opts)` → `{ type:"update", messageId, text:opts.content, components:opts.components }` を送信
  - `reply/followUp(opts)` → `{ type:"push", text:opts.content, components:opts.components, messageId:<new uuid> }`
  guildId は null（デスクトップは秘書/DM 相当）。

> 注: 共有/メンバー申請/ペルソナの各アクションはこのスコープではデスクトップから
> 発火しない（システムDM未配信のため）。だが分岐は中立化し、WS 経路は確実に
> ディスパッチャへ到達すること。回帰防止のため Discord 経路の挙動は完全維持。

---

## 5. 往復実証用スキャフォールド（フラグ既定OFF）

LLM もシステムフローも通常はチャット応答にボタンを出さないため、E2E 往復を実証する
最小の実コードを **config フラグ `DESKTOP_DEMO_COMPONENTS`（既定 false）** の裏に置く:

- ON 時、`chatChannelService` でユーザーメッセージ本文が `/__demo_buttons` と完全一致したら、
  `done` に action row 1 つ（Primary ボタン `label:"確認"` `custom_id:"demo_echo:<nonce>"`）を付けて返す。
- `dispatchComponentInteraction` に `demo_echo` アクションを追加:
  `responder.update({ content: "ボタンを受け取りました: <nonce>", components: [] })`。
- これで「done(components) → クリック → interaction フレーム → update(ボタン除去)」の
  完全往復を harness で検証できる。**本番挙動には一切影響しない**（フラグOFF）。

---

## 6. Rust クライアント

### model.rs
- `ServerFrame::Done` / `Push` に `components: Vec<ActionRow>`（`#[serde(default)]`）。
- `Push` に `message_id: Option<String>`（`#[serde(rename="messageId", default)]`）。
- 新 `ServerFrame::Update { message_id, text: Option<String>, components: Option<Vec<ActionRow>>, embeds: Option<...> }`。
- 新型 `ActionRow { components: Vec<Component> }`、`Component`(enum: Button{style,label,custom_id,url,disabled} / Unknown)。
  Discord JSON の `type` 数値で分岐（1=row, 2=button, それ以外=Unknown でスキップ）。
- 新 `ClientFrame::Interaction { message_id, custom_id }`（`tag="type"`, `rename_all="lowercase"` で `"interaction"`）。
- 往復 serde テストを追加（done with components / update / interaction 送信）。

### ui/mod.rs / ui/chat.rs / app.rs
- `ChatEntry` に `message_id: Option<String>` と `components: Vec<ActionRow>`。
- `ui/chat.rs`: メッセージ気泡の下に action row を描画。ボタンは Discord スタイルに対応した色
  （Primary=青/Secondary=灰/Success=緑/Danger=赤、Link はリンク表示）。`disabled` は不活性。
  クリックで `UiIntent::Interaction { message_id, custom_id }`。
- `app.rs`: `ServerFrame::Update` 受信で `history` から `message_id` 一致の `ChatEntry` を探し、
  `text`/`components` を差し替え（`components: []` で除去）。`UiIntent::Interaction` を
  `UiEvent::Send(ClientFrame::Interaction{..})` へ変換。
- Link ボタン（style 5）はクリックで既定ブラウザを開く（`open` クレート）。

---

## 7. 影響ファイル（所有マップ）

| トラック | ファイル |
|---|---|
| backend | `src/services/componentInteractionService.ts`(新), `src/services/chatChannelService.ts`, `src/server/chatWebSocket.ts`, `src/bot.ts`(handleInteraction縮小), `src/gemini.ts`(ProcessResult.components?), `src/types/contracts.ts`(deliverFinal payload), `src/config.ts`(DESKTOP_DEMO_COMPONENTS) |
| frontend | `clients/desktop/src/model.rs`, `clients/desktop/src/ui/mod.rs`, `clients/desktop/src/ui/chat.rs`, `clients/desktop/src/app.rs`, `clients/desktop/src/net/mod.rs`(UiIntent::Interaction 経路) |
| 共有契約 | 本書（§1-3 が両トラックの単一の真実） |

2 トラックはファイル集合が交わらない（`src/**` ⟂ `clients/desktop/**`）ため並列実装可能。

---

## 8. 検証

1. backend: `pnpm typecheck`（tsgo）緑。Discord 経路の handleInteraction 回帰なし（手動レビュー）。
2. frontend: `cargo build` / `cargo test` 緑（model 往復テスト含む）。
3. 往復 E2E: 隔離 backend を `DESKTOP_DEMO_COMPONENTS=true` で起動 → harness が
   `/__demo_buttons` 送信 → `done`(components) 受信 → `interaction` 送信 →
   `update`(components:[]) 受信、を検証（node harness を拡張）。
