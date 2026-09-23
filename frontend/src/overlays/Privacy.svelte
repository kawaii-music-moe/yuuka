<script lang="ts">
// プライバシーポリシー（旧 index.html #privacy-overlay）。公開ルート・API 不要。

import { Icon } from "$lib/components/ui";
import { goto } from "$lib/router";
</script>

<div class="overlay active">
	<div class="usage-card">
		<div class="login-header">
			<div class="usage-title-row">
				<div class="logo-icon-container">
					<Icon name="security" class="usage-title-icon" />
				</div>
				<h1 class="usage-title">Privacy Policy</h1>
			</div>
			<p class="usage-subtitle">
				Yuuka (Discord 秘書 Bot 兼 管理画面) プライバシーポリシー
			</p>
		</div>

		<div class="usage-content">
			<div class="usage-section">
				<h3>1. 収集する情報</h3>
				<p>本システムは、機能提供のために以下の情報を収集・利用します。</p>
				<ul>
					<li>
						<strong>アカウント情報:</strong> Discord
						ユーザーID、表示名、ログイン用パスワード（ハッシュ化して保存）。
					</li>
					<li>
						<strong>AI設定情報:</strong> ユーザーが個別に登録した Google Gemini API キー。
					</li>
					<li>
						<strong>外部サービス連携情報:</strong> Google Calendar API 連携のための OAuth
						認証情報。
					</li>
					<li>
						<strong>システム利用データ:</strong>
						タスク、スケジュール、家計簿データ（レシート解析画像を含む）、および自動実行用プレイブック。
					</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>2. 情報の利用目的</h3>
				<p>収集した情報は、以下の目的でのみ使用します。</p>
				<ul>
					<li>本サービスにおけるログイン認証およびユーザー識別のため。</li>
					<li>
						Gemini API を利用した Discord 秘書 Bot
						による応答およびレシート画像の解析処理のため。
					</li>
					<li>
						Google カレンダーとのスケジュール同期、および Google Drive
						へのバックアップ実行のため。
					</li>
					<li>登録されたプレイブックに基づいた自動ブラウジング等の実行のため。</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>3. データの安全管理</h3>
				<ul>
					<li>
						Gemini API
						キー、Discord Token、およびブラウザ自動実行用の外部サービスパスワードなどの機密情報は、暗号化（AES-256-GCM）を施した上でデータベースに保存されます。
					</li>
					<li>
						データの漏洩、紛失、改ざんを防止するため、適切なセキュリティ対策を講じています。
					</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>4. 第三者への開示・提供</h3>
				<p>
					本システムは、ユーザーの同意がある場合、または法令に基づく場合を除き、収集した個人情報を第三者に提供・開示することはありません。
				</p>
			</div>

			<div class="usage-section">
				<h3>5. 外部APIの利用</h3>
				<p>
					AI機能の処理にあたり、ユーザーが登録したAPIキーを使用して Google Gemini API
					にリクエストを送信します。送信されたデータの取り扱いは、Googleのプライバシーポリシーに従います。
				</p>
			</div>

			<div class="usage-section">
				<h3>6. ポリシーの変更</h3>
				<p>
					管理者は、本ポリシーを必要に応じて変更することができます。変更後のポリシーは本画面に表示された時点から適用されます。
				</p>
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
	}
	.usage-back-btn {
		min-width: 150px;
	}
</style>
