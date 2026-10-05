# Yuuka Desktop

Windows 常駐オーバーレイ型チャットクライアント（Rust / egui）。Discord を開かずとも
デスクトップから常時 Yuuka に話しかけられる軽量な常駐アプリ。

> 設計資料: [`docs/design/desktop_client/`](../../docs/design/desktop_client/)
> （`client_design.md` / `backend_api.md` / `architecture.md` / `requirements.md` / `roadmap.md`）。
>
> 現状は **Phase 2（クライアント骨格）** のスキャフォールド。プロトコル
> （`src/model.rs`）・設定（`src/config.rs`）・ネットワーク層（`src/net/`）は実装済み。
> GUI（`src/ui/`）と OS 統合（`src/os/`）は **コンパイルが通る正直なスタブ**で、
> 画像（Phase 3）・音声（Phase 4）・オーバーレイ仕上げ（Phase 5）で肉付けする。

## ビルド

公開 URL は**ビルド時に焼き込む**（`YUUKA_API_BASE`）。

```sh
# 開発（既定: ローカル backend http://127.0.0.1:7854）
cargo build

# 本番（URL 焼き込み・最小化リリースビルド）
YUUKA_API_BASE=https://yuuka.kawaii-music.moe cargo build --release
```

`YUUKA_API_BASE` 未設定時は `http://127.0.0.1:7854`（ローカル backend）が既定。
`.cargo/config.toml` の `[env]` で開発時の上書きも可能。

### Linux でのビルド注意（クロスプラットフォーム検証）

本アプリは **Windows ターゲットの GUI** だが、コードは全プラットフォームで
コンパイルできるよう書いてある。Linux で `cargo check`/`build` する場合、
eframe / tray-icon / global-hotkey が以下のシステムライブラリを要求する:

```sh
# Debian/Ubuntu 例
sudo apt-get install -y \
  libgtk-3-dev libxkbcommon-dev libxdo-dev libudev-dev \
  libglib2.0-dev pkg-config
```

これらが無い環境では GUI/OS 依存クレートのリンクに失敗する（コード自体の
エラーではない）。プロトコル/設定/ネットワーク層のロジックは
`cargo test --lib`（model/config のユニットテスト）で検証できる。

## フィーチャフラグ

| feature | 既定 | 内容 |
|---|---|---|
| `audio` | OFF | 音声入力（cpal/opus/ogg/hound）。Phase 4。OFF でもコンパイル可 |

```sh
# 音声入力を含めてビルド（Phase 4）
cargo build --features audio
```

## 起動引数

| 引数 | 意味 |
|---|---|
| `--bot <id>` | 起動時に束縛する botId（1 プロセス 1 Bot）。最優先 |
| `--hidden`   | トレイのみで起動（オーバーレイ非表示） |

botId 決定順: `--bot` → 保存された前回 Bot → 初回は `ready.bots` のプライマリ。
**異なる botId は同時起動可**。同一 botId の二重起動は単一インスタンスロックで抑止。

## モジュール構成

```
src/
├─ main.rs       eframe 起動・トレイ/ホットキー初期化・ランタイム起動・単一インスタンスロック
├─ app.rs        AppState + eframe::App 実装（ビューのルーティング・Net イベント消費）
├─ model.rs      WS プロトコル型（serde）。backend_api.md §3 と一致する「契約」
├─ config.rs     焼き込み URL・URL 導出・起動引数・非機微設定の読み書き
├─ ui/           egui ビュー（overlay / chat / login / settings）
├─ net/          ws（tokio-tungstenite）/ rest（reqwest）/ auth（デバイスフロー + keyring）
├─ os/           OS 依存の薄い抽象（trait + windows 実装 / 非 Windows は no-op）
└─ audio/        cpal 録音 → opus/ogg → base64（`audio` feature の裏）
```

## セキュリティ

- Bearer トークンは **OS 資格情報ストア（keyring）のみ**に保存。平文ファイル禁止（NFR-4）。
- 通信は TLS（`wss`/`https`、rustls で OS 非依存）。
- 会話履歴はローカルに保存しない（正はサーバの `message_logs`）。

## ライセンス / ブランディング

早瀬ユウカのオーブ意匠はファンメイド（ガイドライン準拠）。詳細は別途。
