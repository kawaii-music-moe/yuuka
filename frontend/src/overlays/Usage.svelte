<script lang="ts">
// 利用ガイド（旧 index.html #usage-overlay）。公開ルート・API 不要。
// App.svelte が /usage で本コンポーネントを描画する（.active は常時付与）。

import { Icon } from "$lib/components/ui";
import { goto } from "$lib/router";
</script>

<div class="overlay active">
	<div class="usage-card">
		<div class="login-header">
			<div class="usage-title-row">
				<div class="logo-icon-container">
					<Icon name="help" class="usage-title-icon" />
				</div>
				<h1 class="usage-title">Usage Guide</h1>
			</div>
			<p class="usage-subtitle">Yuuka (Discord 秘書 Bot 兼 管理画面) のご利用方法</p>
		</div>

		<div class="usage-content">
			<div class="usage-section">
				<h3>1. システム概要</h3>
				<p>
					『Yuuka』は、Gemini API を活用した Discord 秘書 Bot と Web
					管理画面のハイブリッドシステムです。タスク・予定（Googleカレンダー同期）・家計簿などをスマートに管理できます。
				</p>
			</div>

			<div class="usage-section">
				<h3>2. アカウント登録と初期設定</h3>
				<ul>
					<li>
						<strong>管理者登録（初期セットアップ）：</strong>
						データベースにユーザーがいない状態で起動すると、初期セットアップ画面が表示されます。Discord
						ユーザーID、パスワード、および Gemini API キーを入力して管理者を登録します。
					</li>
					<li>
						<strong>デフォルトBotの設定：</strong> 管理者登録後、Discord Developer Portal
						で作成した Discord Bot トークンを登録します。
					</li>
					<li>
						<strong>一般ユーザー登録：</strong>
						管理者が生成した「招待コード」を使用し、新規アカウントを作成します。
					</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>3. Discord での操作方法</h3>
				<p>Bot にメンション、またはDMで話しかけて操作します。</p>
				<ul>
					<li>
						<strong>タスク管理：</strong>
						「〇〇をタスクに追加して」「タスク一覧を見せて」「〇〇のタスクを完了して」のように話しかけます。
					</li>
					<li>
						<strong>予定管理：</strong>
						「明日14時からミーティング」「来週の予定を教えて」でスケジュールが追加され、Google
						カレンダーに同期されます。
					</li>
					<li>
						<strong>家計簿・支出：</strong>
						「昼食代800円」「コンビニで日用品を1200円買った」と入力すると、自動で金額やカテゴリを判定して家計簿に記録します。
					</li>
					<li>
						<strong>レシート解析：</strong> レシート画像を Discord に貼り付けると、Gemini
						が画像を解析して家計簿へ登録します。
					</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>4. Web 管理画面の活用</h3>
				<ul>
					<li>
						<strong>ダッシュボード：</strong>
						支出の推移グラフやカテゴリ別比率、未完了タスクが一目でわかります。
					</li>
					<li>
						<strong>Playbook 管理：</strong>
						AIエージェントに実行させたいブラウザ操作等の自動化手順（Playbook）を記述し、Cron式で定期実行できます。
					</li>
					<li>
						<strong>AIレシートスキャナー：</strong>
						管理画面からもレシート画像をドラッグ＆ドロップして自動家計簿登録が可能です。
					</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>5. 新機能の紹介</h3>
				<ul>
					<li>
						<strong>ペルソナ：</strong>
						AIのキャラクター設定を複数作成して切り替えられます。気に入ったペルソナはマーケットプレイスに公開でき、他のユーザーが公開したペルソナをインポートして使うこともできます。
					</li>
					<li>
						<strong>リマインダー：</strong>
						「明日9時に教えて」のような単発リマインドに加え、cron式による繰り返しリマインドをDMまたはチャンネルへ配信できます。
					</li>
					<li>
						<strong>朝報・日報・週報：</strong>
						天気・予定・ニュースをまとめた朝報や、活動サマリーの日報・週報を指定スケジュールで自動配信できます。
					</li>
					<li>
						<strong>Webhook 連携：</strong>
						専用URLを発行して外部サービス（GitHub等）からの通知をDiscordへ転送。キーワードフィルタやToDo/リマインダーの自動作成にも対応します。
					</li>
					<li>
						<strong>MCPサーバー：</strong> 外部のMCP (Model Context Protocol)
						サーバーを登録すると、提供されるToolをAIが会話の中で利用できます。
					</li>
					<li>
						<strong>パスワードマネージャ＆ブラウザ自動ログイン：</strong>
						暗号化保存した資格情報を使って、AIエージェントがPlaybook実行時に対象サイトへ自動ログインできます。
					</li>
					<li>
						<strong>マクロ：</strong>
						よく使う一連の指示をマクロとして登録し、ひとことで呼び出せます。
					</li>
				</ul>
			</div>
		</div>

		<div class="usage-footer">
			<button
				type="button"
				class="btn btn-secondary usage-back-btn"
				onclick={() => goto("/login")}>ログイン画面に戻る</button
			>
		</div>
	</div>
</div>

<style>
	.usage-title-row {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 12px;
		margin-bottom: 12px;
	}
	.logo-icon-container {
		width: 48px;
		height: 48px;
		display: flex;
		align-items: center;
		justify-content: center;
		background-color: var(--surface-1dp);
		border: 1px solid var(--border-matte);
		border-radius: 50%;
	}
	.usage-title-row :global(.usage-title-icon) {
		font-size: 24px;
		color: var(--color-primary);
	}
	.usage-title {
		font-size: 1.5rem;
		margin: 0;
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	.usage-subtitle {
		margin-bottom: 16px;
	}
	.usage-footer {
		margin-top: 24px;
		text-align: center;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 12px;
	}
	.usage-back-btn {
		min-width: 150px;
	}
</style>
