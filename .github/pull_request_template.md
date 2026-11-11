<!-- タイトルは Conventional Commits 風に: feat: / fix: / docs: / refactor: / chore: など -->

## 変更概要
<!-- 何を・なぜ変更したか。関連 issue があればリンク（例: Closes #123） -->



## 変更種別
- [ ] バグ修正 (fix)
- [ ] 新機能 (feat)
- [ ] リファクタ（挙動不変）(refactor)
- [ ] ドキュメント (docs)
- [ ] その他 (chore / ci など)

## テスト
<!-- 実施した確認にチェックし、該当しない項目は消す。
     ローカルのコンパイルは develop 限定。作業ブランチでは制限を守り、
     GitHub-hosted CI で仮マージコミットを検証する（AGENTS.md）。
     スキップは検証成功ではない。未実施理由と CI の結果を記録する。 -->
- [ ] 最新の変更に対する `pr-gate` が成功した（CI 実行URL: ___）
- [ ] `pnpm check`（svelte-check + biome）が exit 0
- [ ] フロント: `pnpm test:front` が通る
- [ ] dev スクリプト: `pnpm test:dev` が通る
- [ ] PWA: `clients/pwa` で `npm run check` / `npm test` が通る
- [ ] Rust: `cargo fmt --check` / `cargo clippy` / `cargo test` が通る
- [ ] デスクトップ: Linux で全 feature の clippy / test / build が通る（Windows 固有部分は別途確認）
- [ ] ローカルでの確認をスキップした（理由: ___）→ CI の結果で確認
- [ ] 実 Discord / ブラウザでの動作確認を実施

## デプロイ影響
<!-- 挙動変更・破壊的変更・マイグレーション・環境変数・再ビルド要否など -->
- [ ] 挙動変更なし
- [ ] DB マイグレーションあり（schema バージョン: ___）
- [ ] 環境変数 / 設定の追加・変更あり
- [ ] その他のデプロイ手順あり（本文に記載）

## チェックリスト
- [ ] ベースブランチは `develop`（リリース時のみ `develop → main`）
- [ ] 最新のベースブランチを取り込み、追加コミット後の再検証・再レビューを完了した
- [ ] 秘密情報（トークン・鍵・個人情報）を含めていない
- [ ] 関連ドキュメント（`docs/`）を更新した（必要な場合）
