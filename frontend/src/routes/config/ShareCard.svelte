<script lang="ts">
// Bot共有管理カード（旧 fetchBotShares / renderBotShares / bot-share-invite-form）。
// owner のみ表示（403 = 非オーナーなら親が描画しない or 自己判定で非表示）。

import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import { Button, confirmDialog } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import type { BotShare, BotSharesResp } from "./configTypes";

interface Props {
	botId: string;
}
let { botId }: Props = $props();

let shares = $state<BotShare[]>([]);
let visible = $state(false);
let inviteId = $state("");

$effect(() => {
	void botId;
	void load();
});

async function load() {
	try {
		const res = (await botAttributeApi.shares(botId)) as BotSharesResp;
		if (!res.success) {
			visible = false;
			return;
		}
		shares = (res.shares ?? []).filter((s) => s.status !== "revoked");
		visible = true;
	} catch {
		// 403（非オーナー）→ 非表示
		visible = false;
	}
}

async function invite(e: SubmitEvent) {
	e.preventDefault();
	const targetUserId = inviteId.trim();
	if (!targetUserId) return;
	try {
		const res = await botAttributeApi.inviteShare({ botId, targetUserId });
		pushToast(
			res.message ?? "招待を送信しました。",
			res.success ? "success" : "error",
		);
		if (res.success) {
			inviteId = "";
			await load();
		}
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "通信エラーが発生しました。",
			"error",
		);
	}
}

async function revoke(share: BotShare) {
	const name = share.shared_username || share.shared_user_id;
	const ok = await confirmDialog({
		message: `${name} さんへの共有を取り消しますか？`,
		danger: true,
		confirmLabel: "取り消し",
	});
	if (!ok) return;
	try {
		const res = await botAttributeApi.revokeShare({
			botId,
			targetUserId: share.shared_user_id,
		});
		if (res.success) await load();
		else pushToast(res.message ?? "取り消しに失敗しました。", "error");
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "通信エラーが発生しました。",
			"error",
		);
	}
}
</script>

{#if visible}
	<details class="config-card card">
		<summary class="column-header badge-right">
			<h3><span class="material-symbols-outlined header-icon-symbol">group_add</span>Bot 共有管理</h3>
			<span class="badge badge-accent">オーナー専用</span>
		</summary>
		<p class="description-text">
			このBotを他のユーザーと共有できます。招待DMが送信され、相手が承認するとアクセスが有効になります。
		</p>
		<div class="shares-list">
			{#if shares.length === 0}
				<p class="empty">共有中のユーザーはいません。</p>
			{:else}
				{#each shares as share (share.shared_user_id)}
					<div class="share-row">
						<div class="share-info">
							<span class="share-name">
								{share.shared_username || share.shared_user_id}
							</span>
							<span
								class="status-badge {share.status === 'active'
									? 'status-sent'
									: 'status-pending'}"
							>
								{share.status === "active" ? "承認済み" : "招待中"}
							</span>
						</div>
						<Button variant="secondary" small onclick={() => revoke(share)}>
							取り消し
						</Button>
					</div>
				{/each}
			{/if}
		</div>
		<form onsubmit={invite} class="invite-form">
			<input
				type="text"
				required
				placeholder="招待するユーザーの Discord ID"
				bind:value={inviteId}
			/>
			<Button type="submit" variant="primary">招待を送信</Button>
		</form>
	</details>
{/if}

<style>
	.shares-list {
		display: flex;
		flex-direction: column;
		gap: 8px;
		margin-top: 16px;
	}
	.empty {
		font-size: 0.8rem;
		color: var(--text-secondary);
		margin: 0;
	}
	.share-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
		padding: 8px 12px;
		border: 1px solid var(--border-matte);
		border-radius: var(--radius);
	}
	.share-info {
		display: flex;
		align-items: center;
		gap: 10px;
		min-width: 0;
	}
	.share-name {
		font-size: 0.88rem;
		font-weight: 600;
		color: var(--text-primary);
	}
	.invite-form {
		display: flex;
		gap: 12px;
		margin-top: 16px;
	}
	.invite-form input {
		flex-grow: 1;
	}
</style>
