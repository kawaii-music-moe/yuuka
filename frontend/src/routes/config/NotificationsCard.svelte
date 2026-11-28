<script lang="ts">
// Web の受信箱（通知先を「Web」にしている人へのリマインド・定期通知など）。
// docs/design/optional_discord.md §6.1。
import { onMount } from "svelte";
import { settingsApi } from "$lib/api/services";
import type { WebNotificationView } from "$lib/api/types";
import { Button } from "$lib/components/ui";
import { toastError } from "$lib/stores/toast";

let items = $state<WebNotificationView[]>([]);
let unread = $state(0);

async function load(): Promise<void> {
	try {
		const res = await settingsApi.notifications();
		items = res.notifications ?? [];
		unread = res.unread ?? 0;
	} catch (err) {
		toastError(err, "通知を読み込めませんでした。");
	}
}

onMount(load);

async function markAllRead(): Promise<void> {
	try {
		await settingsApi.markNotificationsRead();
		await load();
	} catch (err) {
		toastError(err, "既読にできませんでした。");
	}
}
</script>

<section class="config-card card">
	<div class="column-header">
		<h3>
			<span class="material-symbols-outlined header-icon-symbol">notifications</span>通知一覧
			{#if unread > 0}<span class="unread-badge">{unread}</span>{/if}
		</h3>
	</div>
	<p class="description-text">
		通知先を「Web」にしているときは、リマインドなどがここに届きます。
	</p>
	{#if items.length === 0}
		<p class="empty">通知はありません。</p>
	{:else}
		<ul class="notification-list">
			{#each items as item (item.id)}
				<li class:unread={!item.readAt}>
					<time>{item.createdAt}</time>
					<p>{item.content}</p>
				</li>
			{/each}
		</ul>
		<Button type="button" variant="secondary" disabled={unread === 0} onclick={markAllRead}
			>すべて既読にする</Button
		>
	{/if}
</section>

<style>
	.notification-list {
		list-style: none;
		margin: 16px 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.notification-list li {
		padding: 8px 12px;
		border-radius: 8px;
		border: 1px solid var(--border-color, rgba(127, 127, 127, 0.3));
	}
	.notification-list li.unread {
		border-left-width: 4px;
	}
	.notification-list time {
		font-size: 0.8em;
		opacity: 0.7;
	}
	.notification-list p {
		margin: 4px 0 0;
		white-space: pre-wrap;
	}
	.unread-badge {
		margin-left: 8px;
		font-size: 0.8em;
		padding: 0 8px;
		border-radius: 999px;
		background: var(--accent-color, #c0392b);
		color: #fff;
	}
	.empty {
		margin-top: 12px;
	}
</style>
