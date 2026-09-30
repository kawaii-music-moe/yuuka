<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// Webhook タブ（旧 app.js fetchWebhooksList / fetchWebhookDeliveries +
//  index.html #tab-webhooks を移植）。webhookApi 使用（scope:'user'）。
//   - エンドポイント一覧（URL コピー・有効/無効トグル・削除）
//   - 受信履歴（直近50件）
//   - 作成モーダル
// user-scoped のため activeBot に依存しない（$effect で一度だけ取得）。
// ─────────────────────────────────────────────────────────────────────────

import { ApiError } from "$lib/api/client";
import { webhookApi } from "$lib/api/services";
import type {
	WebhookDeliveryRecord,
	WebhookEndpointView,
} from "$lib/api/types";
import {
	Badge,
	Button,
	confirmDialog,
	EmptyState,
	Icon,
} from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import WebhookCreateModal from "./webhooks/WebhookCreateModal.svelte";
import {
	deliveryStatusLabel,
	deliveryStatusTone,
} from "./webhooks/webhookUtils";

let endpoints = $state<WebhookEndpointView[]>([]);
let deliveries = $state<WebhookDeliveryRecord[]>([]);
let createOpen = $state(false);

function reportError(e: unknown) {
	pushToast(
		e instanceof ApiError ? e.message : "エラーが発生しました",
		"error",
	);
}

async function loadEndpoints() {
	try {
		const res = await webhookApi.list();
		endpoints = res.endpoints ?? [];
	} catch (e) {
		reportError(e);
		endpoints = [];
	}
}

async function loadDeliveries() {
	try {
		const res = await webhookApi.deliveries();
		deliveries = res.deliveries ?? [];
	} catch (e) {
		reportError(e);
		deliveries = [];
	}
}

// user-scoped。マウント時に一度取得（activeBot 非依存）。
$effect(() => {
	void loadEndpoints();
	void loadDeliveries();
});

async function copyUrl(url: string) {
	try {
		await navigator.clipboard.writeText(url);
		pushToast("コピーしました", "success");
	} catch {
		pushToast(
			"コピーに失敗しました。手動で選択してコピーしてください。",
			"error",
		);
	}
}

async function toggleEnabled(ep: WebhookEndpointView) {
	try {
		await webhookApi.update({ id: ep.id, enabled: !ep.enabled });
		await loadEndpoints();
	} catch (e) {
		reportError(e);
	}
}

async function deleteEndpoint(ep: WebhookEndpointView) {
	const ok = await confirmDialog({
		message: `Webhook「${ep.name}」を削除しますか？\n発行済みURLは無効になります。`,
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		await webhookApi.delete(ep.id);
		await loadEndpoints();
	} catch (e) {
		reportError(e);
	}
}

async function createEndpoint(payload: {
	name: string;
	secret: string;
	notifyTargetType: "dm" | "channel";
	notifyTargetId: string;
	template: string;
	filterKeyword: string;
	createTodo: boolean;
	createReminder: boolean;
}) {
	try {
		const res = await webhookApi.create(payload);
		pushToast(res.message ?? "Webhookを作成しました。", "success");
		createOpen = false;
		await loadEndpoints();
	} catch (e) {
		reportError(e);
	}
}
</script>

<section class="tab-view">
	<div class="view-actions-card card">
		<div class="filters-group">
			<span class="label-text">
				外部サービスからの通知を受け取るWebhookエンドポイントを管理します。
			</span>
		</div>
		<Button variant="primary" onclick={() => (createOpen = true)}>＋ Webhook作成</Button>
	</div>

	<!-- エンドポイント一覧 -->
	<div class="card webhook-card-spacer">
		<div class="column-header">
			<h3><Icon name="webhook" class="header-icon-symbol" />エンドポイント一覧</h3>
		</div>
		<div class="webhook-list">
			{#if endpoints.length > 0}
				{#each endpoints as ep (ep.id)}
					<div class="card-item glass webhook-item">
						<div class="webhook-item-head">
							<div class="webhook-item-title">
								<span class="webhook-name">{ep.name}</span>
								{#if ep.enabled}
									<Badge tone="status-active">有効</Badge>
								{:else}
									<Badge tone="status-suspended">無効</Badge>
								{/if}
								{#if ep.has_secret}
									<Badge tone="status-default">署名検証</Badge>
								{/if}
							</div>
							<div class="webhook-item-actions">
								<button
									type="button"
									class="btn-mini"
									onclick={() => toggleEnabled(ep)}
								>
									{ep.enabled ? "無効化" : "有効化"}
								</button>
								<button
									type="button"
									class="btn-trash"
									aria-label="削除"
									onclick={() => deleteEndpoint(ep)}
								>
									<Icon name="delete" />
								</button>
							</div>
						</div>
						<div class="webhook-url-row">
							<input
								type="text"
								class="webhook-url-input"
								readonly
								value={ep.url}
							/>
							<Button variant="secondary" small onclick={() => copyUrl(ep.url)}
								>URLコピー</Button
							>
						</div>
						<div class="webhook-meta">
							<span>通知先: {ep.notify_target_type === "channel" ? "チャンネル" : "DM"}</span>
							{#if ep.filter_keyword}
								<span>フィルタ: {ep.filter_keyword}</span>
							{/if}
							{#if ep.create_todo}<span>ToDo自動作成</span>{/if}
							{#if ep.create_reminder}<span>リマインダー自動作成</span>{/if}
						</div>
					</div>
				{/each}
			{:else}
				<EmptyState
					icon="webhook"
					message="Webhookエンドポイントはありません。「＋ Webhook作成」から発行できます。"
				/>
			{/if}
		</div>
	</div>

	<!-- 受信履歴 -->
	<div class="card webhook-card-spacer">
		<div class="column-header action-right">
			<h3><Icon name="history" class="header-icon-symbol" />受信履歴 (直近50件)</h3>
			<Button variant="secondary" small onclick={loadDeliveries}>更新</Button>
		</div>
		<div class="table-responsive webhook-table-wrap">
			<table class="expense-table">
				<thead>
					<tr>
						<th>受信日時</th>
						<th>ステータス</th>
						<th>詳細</th>
					</tr>
				</thead>
				<tbody>
					{#if deliveries.length > 0}
						{#each deliveries as d (d.id)}
							<tr>
								<td class="webhook-mono">{d.created_at}</td>
								<td>
									<Badge tone={deliveryStatusTone(d.status)}>
										{deliveryStatusLabel(d.status)}
									</Badge>
								</td>
								<td>{d.detail || "—"}</td>
							</tr>
						{/each}
					{:else}
						<tr>
							<td colspan="3" class="webhook-empty">受信履歴はありません。</td>
						</tr>
					{/if}
				</tbody>
			</table>
		</div>
	</div>
</section>

<WebhookCreateModal bind:open={createOpen} onsave={createEndpoint} />

<style>
	.webhook-card-spacer {
		margin-top: 20px;
	}
	.webhook-list {
		display: flex;
		flex-direction: column;
		gap: 12px;
		margin-top: 12px;
	}
	.webhook-item {
		flex-direction: column;
		align-items: stretch;
		gap: 10px;
	}
	.webhook-item-head {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.webhook-item-title {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.webhook-name {
		font-weight: 600;
	}
	.webhook-item-actions {
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.webhook-url-row {
		display: flex;
		gap: 8px;
		align-items: center;
	}
	.webhook-url-input {
		flex: 1;
		font-family: var(--font-family-mono);
		font-size: 0.8rem;
	}
	.webhook-meta {
		display: flex;
		flex-wrap: wrap;
		gap: 12px;
		font-size: 0.75rem;
		color: var(--color-zinc-muted);
	}
	.webhook-table-wrap {
		margin-top: 12px;
	}
	.webhook-mono {
		font-family: var(--font-family-mono);
		font-size: 0.8rem;
	}
	.webhook-empty {
		text-align: center;
		font-size: 0.8rem;
		color: var(--color-zinc-muted);
	}
</style>
