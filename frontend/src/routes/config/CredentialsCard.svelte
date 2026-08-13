<script lang="ts">
	// 利用可能なAI認証情報カード（読み取り専用。旧 fetchCredentialsSettings）。
	// 登録・管理は「Bot統合管理」ページ（他グループ）へ誘導するのみ。
	import { credentialApi } from "$lib/api/services";
	import type { CredentialRow, CredentialsResp } from "./configTypes";

	interface Props {
		/** botId 変更で付与状況を再取得（/api/credentials は bot-scoped） */
		botId: string;
	}
	let { botId }: Props = $props();

	let rows = $state<CredentialRow[]>([]);

	$effect(() => {
		void botId;
		void load();
	});

	async function load() {
		try {
			const res = (await credentialApi.list()) as CredentialsResp;
			rows = res.success ? (res.credentials ?? []) : [];
		} catch {
			rows = [];
		}
	}

	function serviceOf(c: CredentialRow): string {
		return c.service_name ?? c.serviceName ?? "";
	}
	function updatedOf(c: CredentialRow): string {
		return c.updated_at ?? c.updatedAt ?? "";
	}
</script>

<details class="config-card card">
	<summary class="column-header badge-right">
		<h3><span class="material-symbols-outlined header-icon-symbol">lock</span>利用可能なAI認証情報</h3>
		<span class="badge badge-accent">読み取り専用</span>
	</summary>
	<p class="description-text">
		このBotがAIエージェントの自動ブラウジング等で利用できる認証情報の一覧です。パスワードはAES-256-GCMで暗号化保存され、ここには表示されません。認証情報の登録・管理は「Bot統合管理」で行います。
	</p>

	<div class="table-responsive">
		<table class="expense-table">
			<thead>
				<tr>
					<th>対象サービス名</th>
					<th>ユーザー名 (ID)</th>
					<th>パスワード</th>
					<th>URL</th>
					<th>最終更新</th>
				</tr>
			</thead>
			<tbody>
				{#if rows.length > 0}
					{#each rows as c, i (serviceOf(c) + i)}
						<tr>
							<td class="td-service">{serviceOf(c)}</td>
							<td class="td-mono">{c.username ?? ""}</td>
							<td class="td-mono td-muted">••••••••••••</td>
							<td class="td-url" title={c.url ?? ""}>{c.url || "—"}</td>
							<td class="td-date">{updatedOf(c)}</td>
						</tr>
					{/each}
				{:else}
					<tr>
						<td colspan="5" class="td-empty">利用可能なAI認証情報はありません。</td>
					</tr>
				{/if}
			</tbody>
		</table>
	</div>

	<div class="cred-footer">
		<span class="field-sub">認証情報の登録・管理は「Bot統合管理」で行います。</span>
		<a href="/admin/integrated" class="btn btn-secondary btn-sm">Bot統合管理へ</a>
	</div>
</details>

<style>
	.table-responsive {
		margin-top: 16px;
	}
	.expense-table {
		width: 100%;
		border-collapse: collapse;
	}
	.expense-table th {
		text-align: left;
		padding: 10px;
		font-weight: 500;
		font-size: 0.85rem;
		color: var(--color-zinc-muted);
		border-bottom: 1px solid var(--border-matte);
	}
	.expense-table td {
		padding: 12px 10px;
		font-size: 0.85rem;
		border-bottom: 1px solid var(--border-matte);
	}
	.td-service {
		font-weight: 700;
	}
	.td-mono {
		font-family: var(--font-family-mono);
	}
	.td-muted {
		color: var(--color-zinc-muted);
	}
	.td-url {
		font-size: 0.78rem;
		font-family: var(--font-family-mono);
		max-width: 200px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.td-date {
		font-size: 0.75rem;
		color: var(--color-zinc-muted);
	}
	.td-empty {
		text-align: center;
		padding: 20px;
		color: var(--color-zinc-muted);
		font-size: 0.8rem;
	}
	.cred-footer {
		margin-top: 18px;
		display: flex;
		justify-content: flex-end;
		align-items: center;
		gap: 12px;
	}
</style>
