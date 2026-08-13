# Agent Desk PWA

Discord エージェント基盤向けの Web コントロールパネルです。Vue 3 + TypeScript + Vite を使用し、PWA としてインストールできます。

## 起動

```bash
npm install
npm run mock  # 別のターミナルで起動: http://localhost:8787
npm run dev   # http://localhost:5173
```

開発サーバーは `/api` をモックサーバーへプロキシします。実サーバーを検証する場合は `VITE_API_PROXY_TARGET=https://your-agent.example` を設定してください。実サーバー側では、クライアントのオリジンを CORS で許可します。

## API 境界

画面は `src/api/gateway.ts` の `AgentGateway` だけに依存します。HTTP の URL、リクエスト、レスポンスの詳細は `src/api/httpAdapter.ts` に閉じ込めています。実 API が確定したらこのアダプタのみを変更してください。

`mock/server.ts` は同じ Gateway 契約を検証するためのインメモリ API です。状態はプロセス停止時に破棄されます。

## チャットと Markdown

チャットは `ChatMessage` と `ChatReference` を契約に持ちます。`references` を返すことで、メッセージから TODO、カレンダー、家計、共有ノートを参照カードとして開けます。実エージェントは関連レコードを生成・検索した後、対応する `href` を返してください。

表示は CommonMark / GFM の見出し、表、タスクリスト、リンク、打消し、脚注、コードブロックとシンタックスハイライトに対応しています。HTML は無効化し、レンダリング結果もサニタイズしています。

## Git 運用

日常の変更は `dev` ブランチにコミットします。`master` はリリース時にのみ `dev` からマージします。機能単位で `dev` からブランチを切り、1 機能 1 コミットを目安にします。
