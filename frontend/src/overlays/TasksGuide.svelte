<script lang="ts">
	// タスクの使い方ガイド（旧 index.html #task-guide-overlay）。公開ルート・API 不要。
	// 「閉じる」でタスクタブへ戻る（旧 btn-task-guide-back）。
	import { goto } from "$lib/router";
	import { Icon } from "$lib/components/ui";
</script>

<div class="overlay active">
	<div class="usage-card">
		<div class="login-header">
			<div class="usage-title-row">
				<div class="logo-icon-container">
					<Icon name="checklist" class="usage-title-icon" />
				</div>
				<h1 class="usage-title">Task Guide</h1>
			</div>
			<p class="usage-subtitle">タスク管理の使い方ガイド</p>
		</div>

		<div class="usage-content">
			<div class="usage-section">
				<h3>1. タスクの基本</h3>
				<p>
					「やること」を登録して、期限・優先度・進捗とともに管理できます。管理画面の「＋
					タスク追加」から、または Discord で
					「〇〇をタスクに追加して」と話しかけて登録できます。完了したら一覧のチェックボックス、または「〇〇終わった」で完了にできます。
				</p>
				<ul>
					<li>
						<strong>期限・開始日：</strong>
						期限（締め切り）と開始日を設定できます。両方を持つタスクはガントチャートにバーで表示されます。
					</li>
					<li>
						<strong>優先度：</strong>
						<Icon name="circle" size="0.95rem" fill class="prio-dot-high" />高
						/ <Icon name="circle" size="0.95rem" fill class="prio-dot-mid" />中
						/ <Icon name="circle" size="0.95rem" fill class="prio-dot-low" />低
						で重要度を表します。「タスクを整理して」と頼むと、AIが期限や内容から優先度を提案します（確定は承認後）。
					</li>
					<li>
						<strong>いつかやる：</strong>
						期限も開始日も決めていないタスクは「<Icon name="schedule" size={16} /> いつかやる」にまとまります。
					</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>2. サブタスクと進捗</h3>
				<ul>
					<li>
						<strong>サブタスク：</strong>
						大きなタスクを小さな手順に分解できます（1段まで）。親タスクの進捗は「完了したサブタスク数
						÷ 全体」で自動計算されます。
					</li>
					<li>
						<strong>進捗：</strong> サブタスクのないタスクは
						0〜100%
						で手動更新できます。「〇〇は半分終わった」のように伝えると進捗が記録され、メモとともに履歴に残ります。
					</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>3. タグでグループ分け</h3>
				<ul>
					<li>
						<strong>自動タグ付け：</strong>
						タスクを追加・更新すると、内容からAIが自動でタグ（グループ）を付けます。
					</li>
					<li>
						<strong>手動修正：</strong>
						「#3のタグを『買い物』に変えて」「『緊急』タグを足して」「『仮』タグを外して」のように、タグを後から直せます。
					</li>
					<li>
						<strong>グループ表示：</strong>
						「タスクをグループ別に見せて」と頼むと、タグごとにまとめて確認できます。タグの付いていないタスクは「未分類」にまとまります。
					</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>4. ルーチン（繰り返し）タスク</h3>
				<p>
					毎日・毎週・毎月など、定期的に繰り返すタスクを登録できます。「毎週月曜の朝に〇〇」のように伝えると、ルーチンとして登録されます。期日が来ると自動で次回ぶんへ更新されます。
				</p>
				<ul>
					<li>
						<strong>終わり方を決めて登録：</strong> 「年末まで毎週」のように<strong
							>終了日</strong
						>を、または「毎日5回だけ」のように<strong>回数</strong
						>を指定すると、その時点で自動的に繰り返しが止まります。
					</li>
					<li>
						<strong>あとから終了：</strong>
						「もう毎週の〇〇はやらなくていい」「#3のルーチンを止めて」と伝えると繰り返しを終了します。タスク自体は単発として残ります（完全に消すには削除してください）。
					</li>
					<li>
						<strong>リマインド：</strong>
						期限が近づくと自動でDM／チャンネルに通知されます。
					</li>
				</ul>
			</div>

			<div class="usage-section">
				<h3>5. 表示モード</h3>
				<ul>
					<li>
						<strong>一覧：</strong> 優先度・期限順に並びます。「全て / 未完了 /
						完了済み」で絞り込めます。
					</li>
					<li>
						<strong>ガント：</strong>
						開始日〜期限をバーで時系列表示します。日付のないタスクは「いつかやる」に並びます。
					</li>
				</ul>
			</div>
		</div>

		<div class="usage-footer">
			<button
				type="button"
				class="btn btn-secondary usage-back-btn"
				onclick={() => goto("/bot/tasks")}>閉じる</button
			>
		</div>
	</div>
</div>

<style>
	/* 優先度ドット（Icon の class 経由。Icon 内部要素のため :global が必要） */
	.usage-section :global(.prio-dot-high) {
		color: #e5484d;
	}
	.usage-section :global(.prio-dot-mid) {
		color: #f5a623;
	}
	.usage-section :global(.prio-dot-low) {
		color: #3b82f6;
	}
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
