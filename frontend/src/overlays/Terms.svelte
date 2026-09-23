<script lang="ts">
// 利用規約（旧 index.html #terms-overlay）。公開ルート・API 不要。

import { Icon } from "$lib/components/ui";
import { goto } from "$lib/router";
</script>

<div class="overlay active">
	<div class="usage-card">
		<div class="login-header">
			<div class="usage-title-row">
				<div class="logo-icon-container">
					<Icon name="gavel" class="usage-title-icon" />
				</div>
				<h1 class="usage-title">Terms of Use</h1>
			</div>
			<p class="usage-subtitle">Yuuka (Discord 秘書 Bot 兼 管理画面) 利用規約</p>
		</div>

		<div class="usage-content">
			<div class="usage-section">
				<h3>1. はじめに</h3>
				<p>
					この利用規約（以下「本規約」）は、本システム『Yuuka』（以下「本サービス」）の利用条件を定めるものです。ユーザーは本サービスを利用することにより、本規約のすべての内容に同意したものとみなされます。
				</p>
			</div>

			<div class="usage-section">
				<h3>2. 利用資格とアカウント管理</h3>
				<ul>
					<li>
						本サービスは、管理者が発行した「招待コード」を所持しているユーザー、または初期管理者として登録されたユーザーのみが利用できます。
					</li>
					<li>
						ユーザーは、自己の責任においてDiscordユーザーIDおよびログインパスワード、招待コードを厳重に管理するものとします。
					</li>
					<li>アカウントの第三者への譲渡、貸与、共有は一切禁止します。</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>3. Gemini APIキーおよび各種認証情報</h3>
				<ul>
					<li>
						ユーザーは、本サービスを利用するために自身が取得した Google Gemini API
						キーを登録する必要があります。
					</li>
					<li>
						登録されたAPIキーおよび連携したGoogleアカウント（OAuth）の認証情報は、本サービス内部で暗号化して保管され、AIアシスタント機能の実行およびGoogle
						カレンダー同期以外の目的には使用されません。
					</li>
					<li>
						ユーザーは、自身が登録するAPIキーの利用枠および発生する利用料金等について、自己の責任で管理するものとします。
					</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>4. 禁止事項</h3>
				<p>ユーザーは、本サービスの利用にあたり、以下の行為を行ってはなりません。</p>
				<ul>
					<li>他者なりすましによる本サービスの不正利用行為</li>
					<li>
						本サービスのバグや脆弱性を意図的に悪用し、システムに不当な負荷をかける行為
					</li>
					<li>
						AIモデル（Gemini
						API）に対する有害なプロンプトの入力、その他公序良俗に反するコンテンツの生成を目的とした利用
					</li>
					<li>
						管理者の事前の許可なく本サービスを商用目的で第三者に提供・再販売する行為
					</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>5. 免責事項</h3>
				<ul>
					<li>
						本サービスで提供されるAIによるテキスト生成、レシート画像の解析結果等の正確性、完全性、最新性について、本システムおよびその開発者は一切の保証を行いません。
					</li>
					<li>
						本サービスの利用、または利用できなかったことによりユーザーに生じた損害（Gemini
						APIの利用料金、Google
						カレンダー同期に伴う予定の消失、データの損壊等を含む）について、一切の責任を負いません。
					</li>
					<li>
						本サービスは事前の予告なく、機能の変更、追加、一時停止、または提供終了を行うことがあります。
					</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>6. 規約の変更</h3>
				<p>
					管理者は、必要と判断した場合にはいつでも本規約を変更することができるものとします。変更後の利用規約は本画面に表示された時点から効力を生じるものとし、ユーザーが変更後に本サービスを利用した場合は、変更後の規約に同意したものとみなされます。
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
