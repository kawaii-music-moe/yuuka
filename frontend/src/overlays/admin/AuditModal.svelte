<script lang="ts">
// 監査ログ全件モーダル（旧 modal-audit + fetchAdminAuditLogsModal + prev/next/filter）。
// action フィルタ + offset ページングを自前 state で持ち、開くたび / ページ・フィルタ変更で再取得。

import { ApiError } from "$lib/api/client";
import { adminApi } from "$lib/api/services";
import type { AuditLogEntry } from "$lib/api/types";
import { Button, Modal } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import { maskDiscordId } from "./adminUtils";

interface Props {
	open?: boolean;
}
let { open = $bindable(false) }: Props = $props();

const PAGE_SIZE = 20;
let page = $state(0);
let action = $state("");
let filterInput = $state("");
let logs = $state<AuditLogEntry[]>([]);
let total = $state(0);

const totalPages = $derived(Math.max(1, Math.ceil(total / PAGE_SIZE)));
const from = $derived(page * PAGE_SIZE + 1);
const to = $derived(page * PAGE_SIZE + logs.length);
const pageInfo = $derived(
	logs.length === 0
		? "0 件"
		: `${from}–${to} 件 / 全 ${total} 件（${page + 1} / ${totalPages} ページ）`,
);

// 開くたびに（現在のフィルタ入力を確定して）1ページ目から取得。
$effect(() => {
	if (!open) return;
	action = filterInput.trim();
	page = 0;
	void load();
});

async function load() {
	try {
		const res = await adminApi.auditLogs({
			limit: PAGE_SIZE,
			offset: page * PAGE_SIZE,
			action: action || undefined,
		});
		logs = res.logs ?? [];
		total = typeof res.total === "number" ? res.total : 0;
	} catch (e) {
		pushToast(
			e instanceof ApiError ? e.message : "監査ログの取得に失敗しました",
			"error",
		);
		logs = [];
		total = 0;
	}
}

function applyFilter(e: SubmitEvent) {
	e.preventDefault();
	action = filterInput.trim();
	page = 0;
	void load();
}
function prev() {
	if (page > 0) {
		page -= 1;
		void load();
	}
}
function next() {
	if (page + 1 < totalPages) {
		page += 1;
		void load();
	}
}
</script>

<Modal bind:open title="監査ログ（全件）" wide>
	<form class="audit-filter" onsubmit={applyFilter}>
		<input
			type="text"
			class="audit-filter-input"
			bind:value={filterInput}
			placeholder="action でフィルタ (空欄で全件)"
		/>
		<Button variant="secondary" type="submit">絞り込み</Button>
	</form>
	<div class="table-responsive">
		<table class="expense-table audit-table">
			<thead>
				<tr>
					<th class="admin-table-th">日時</th>
					<th class="admin-table-th">ユーザー</th>
					<th class="admin-table-th">Action</th>
					<th class="admin-table-th">対象</th>
					<th class="admin-table-th">詳細</th>
				</tr>
			</thead>
			<tbody>
				{#if logs.length === 0}
					<tr><td colspan="5" class="audit-empty">監査ログはありません。</td></tr>
				{:else}
					{#each logs as log (log.id)}
						<tr>
							<td class="admin-table-td audit-date">{log.created_at}</td>
							<td class="admin-table-td admin-discord-id">{maskDiscordId(log.user_id)}</td>
							<td class="admin-table-td audit-action">{log.action}</td>
							<td class="admin-table-td audit-muted">{log.target || "—"}</td>
							<td class="admin-table-td audit-muted">{log.detail || "—"}</td>
						</tr>
					{/each}
				{/if}
			</tbody>
		</table>
	</div>
	<div class="audit-pager">
		<span class="description-text audit-page-info">{pageInfo}</span>
		<div class="audit-pager-btns">
			<Button variant="secondary" disabled={page === 0} onclick={prev}>前へ</Button>
			<Button variant="secondary" disabled={page + 1 >= totalPages || logs.length === 0} onclick={next}
				>次へ</Button
			>
		</div>
	</div>
</Modal>

<style>
	.audit-filter {
		display: flex;
		gap: 12px;
		margin-top: 8px;
		margin-bottom: 16px;
	}
	.audit-filter-input {
		flex-grow: 1;
		font-family: var(--font-family-mono);
	}
	.audit-table {
		width: 100%;
	}
	.audit-empty {
		text-align: center;
		padding: 20px;
		color: var(--color-zinc-muted);
		font-size: 0.8rem;
	}
	.audit-date {
		font-size: 0.78rem;
		white-space: nowrap;
		color: var(--color-zinc-muted);
	}
	.audit-action {
		font-family: var(--font-family-mono);
		font-size: 0.78rem;
	}
	.audit-muted {
		font-size: 0.78rem;
		color: var(--color-zinc-muted);
	}
	.audit-pager {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		margin-top: 16px;
	}
	.audit-page-info {
		margin: 0;
	}
	.audit-pager-btns {
		display: flex;
		gap: 8px;
	}
</style>
