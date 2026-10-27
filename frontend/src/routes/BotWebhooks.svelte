<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// Webhook タブ（旧 app.js fetchWebhooksList / fetchWebhookDeliveries +
//  index.html #tab-webhooks を移植）。webhookApi 使用（一覧・作成・受信履歴は選択中 Bot）。
// Playbook タブと同じく、次の 2 つをヘッダーのページ内タブで切り替える:
//   - 一覧: エンドポイント一覧（URL コピー・有効/無効トグル・削除）。追加で作成フォームに切り替わる
//   - 受信履歴: 直近50件
// エンドポイントは Bot ごとの設定のため、選択中 Bot が変わったら取り直す。
// ─────────────────────────────────────────────────────────────────────────

import { onDestroy } from "svelte";
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
	PageTabs,
} from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { headerExtra } from "$lib/stores/headerExtra";
import { pushToast } from "$lib/stores/toast";
import WebhookForm from "./webhooks/WebhookForm.svelte";
import {
	deliveryStatusLabel,
	deliveryStatusTone,
} from "./webhooks/webhookUtils";

const VIEWS = [
	{ id: "list", label: "一覧", icon: "list_alt" },
	{ id: "deliveries", label: "受信履歴", icon: "history" },
];
let view = $state("list");

let endpoints = $state<WebhookEndpointView[]>([]);
let deliveries = $state<WebhookDeliveryRecord[]>([]);
let adding = $state(false);

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

// 選択中 Bot のエンドポイント・受信履歴を取得（Bot 切り替えで取り直す）。
$effect(() => {
	void $activeBot?.id;
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
		adding = false;
		await loadEndpoints();
	} catch (e) {
		reportError(e);
	}
}

headerExtra.set(headerTabs);
onDestroy(() => headerExtra.set(null));
</script>

{#snippet headerTabs()}
	<PageTabs tabs={VIEWS} bind:active={view} label="Webhook の設定" flush />
{/snippet}

<section class="tab-view webhooks-tab">
	<!-- エンドポイント一覧 -->
	{#if view === "list"}
		{#if adding}
			<WebhookForm onsave={createEndpoint} oncancel={() => (adding = false)} />
		{:else}
			<div class="action-column card">
				<div class="column-header wh-header-row">
					<Button variant="primary" onclick={() => (adding = true)}
						><Icon name="add" />追加</Button
					>
				</div>
				<div class="wh-list">
					{#if endpoints.length > 0}
						{#each endpoints as ep (ep.id)}
							<div class="card-item glass wh-item">
								<div class="wh-item-body">
									<div class="wh-item-title">
										<span class="wh-title">{ep.name}</span>
										{#if ep.has_secret}
											<Badge tone="status-default">署名検証</Badge>
										{/if}
									</div>
									<div class="wh-url-row">
										<span class="wh-url">{ep.url}</span>
										<button
											type="button"
											class="wh-icon-plain"
											title="URLをコピー"
											aria-label="URLをコピー"
											onclick={() => copyUrl(ep.url)}
										>
											<Icon name="content_copy" size={18} />
										</button>
									</div>
									<div class="wh-meta">
										<span>通知先: {ep.notify_target_type === "channel" ? "チャンネル" : "DM"}</span>
										{#if ep.filter_keyword}
											<span>フィルタ: {ep.filter_keyword}</span>
										{/if}
										{#if ep.create_todo}<span>ToDo自動作成</span>{/if}
										{#if ep.create_reminder}<span>リマインダー自動作成</span>{/if}
									</div>
								</div>
								<div class="wh-item-actions">
									<button
										type="button"
										class="btn-mini"
										class:wh-on={ep.enabled}
										onclick={() => toggleEnabled(ep)}
									>
										{ep.enabled ? "有効" : "無効"}
									</button>
									<button
										type="button"
										class="wh-icon-plain wh-trash-plain"
										aria-label="削除"
										onclick={() => deleteEndpoint(ep)}
									>
										<Icon name="delete" />
									</button>
								</div>
							</div>
						{/each}
					{:else}
						<EmptyState
							icon="webhook"
							message="Webhookエンドポイントはありません。「追加」から発行できます。"
						/>
					{/if}
				</div>
			</div>
		{/if}
	{/if}

	<!-- 受信履歴 -->
	{#if view === "deliveries"}
		<div class="card">
			<div class="column-header wh-header-row">
				<Button variant="secondary" small onclick={loadDeliveries}>更新</Button>
			</div>
			<div class="wh-runs">
				{#if deliveries.length > 0}
					{#each deliveries as d (d.id)}
						<div
							class="wh-run"
							class:wh-run-failed={d.status === "failed"}
							class:wh-run-success={d.status === "notified"}
						>
							<div class="wh-run-head">
								<Badge tone={deliveryStatusTone(d.status)}>
									{deliveryStatusLabel(d.status)}
								</Badge>
								<span class="wh-run-meta">{d.created_at}</span>
							</div>
							{#if d.detail}
								<div class="wh-run-detail">{d.detail}</div>
							{/if}
						</div>
					{/each}
				{:else}
					<EmptyState icon="history" message="受信履歴はありません。" />
				{/if}
			</div>
		</div>
	{/if}
</section>

<style>
	@media (max-width: 768px) {
		/* 共通の .card-item は縦積みになるため、操作 UI を同じ行に戻す。 */
		.wh-item {
			padding-inline: 12px;
			flex-direction: row;
			align-items: flex-start;
		}
	}
	.wh-header-row {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 8px;
	}
	.wh-list {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.wh-item {
		display: flex;
		justify-content: space-between;
		gap: 10px;
		align-items: flex-start;
	}
	.wh-item-body {
		flex: 1;
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.wh-item-title {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.wh-title {
		font-weight: 600;
	}
	.wh-url-row {
		display: flex;
		align-items: center;
		gap: 4px;
		margin-top: 4px;
	}
	.wh-url {
		min-width: 0;
		font-family: var(--font-family-mono);
		font-size: 0.75rem;
		color: var(--color-primary);
		word-break: break-all;
	}
	.wh-meta {
		display: flex;
		flex-wrap: wrap;
		gap: 12px;
		margin-top: 4px;
		font-size: 0.75rem;
		color: var(--color-zinc-muted);
	}
	.wh-on {
		color: var(--color-primary);
		border-color: var(--color-primary);
	}
	.wh-icon-plain {
		display: inline-flex;
		align-items: center;
		flex-shrink: 0;
		padding: 4px;
		appearance: none;
		border: none;
		box-shadow: none;
		background: transparent;
		color: var(--text-secondary);
		cursor: pointer;
	}
	.wh-trash-plain {
		color: var(--color-error);
	}
	.wh-icon-plain:hover {
		opacity: 0.75;
	}
	.wh-item-actions {
		display: flex;
		gap: 6px;
		align-items: center;
		flex-shrink: 0;
	}
	.wh-runs {
		display: flex;
		flex-direction: column;
		gap: 8px;
		margin-top: 8px;
	}
	.wh-run {
		border-left: 3px solid var(--color-zinc-muted);
		padding: 8px 12px;
		border-radius: var(--radius);
		background: var(--surface-1dp);
	}
	.wh-run-success {
		border-left-color: var(--color-green, #22c55e);
	}
	.wh-run-failed {
		border-left-color: var(--color-red, #ef4444);
	}
	.wh-run-head {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.wh-run-meta {
		font-family: var(--font-family-mono);
		font-size: 0.72rem;
		color: var(--color-zinc-muted);
	}
	.wh-run-detail {
		margin-top: 4px;
		font-size: 0.8rem;
		overflow-wrap: anywhere;
	}
</style>
