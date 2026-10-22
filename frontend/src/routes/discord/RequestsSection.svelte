<script lang="ts">
// 利用申請（承認待ち）（旧 renderAssistantRequests + decideMemberRequest）。

import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import { pushToast } from "$lib/stores/toast";
import type { MemberRequest, MemberRequestsResp } from "../config/configTypes";
import AddButton from "./AddButton.svelte";
import DeleteButton from "./DeleteButton.svelte";

interface Props {
	botId: string;
	/** リロード同期用（親が assistant-config を引き直すたび ++ される nonce） */
	reloadKey: number;
	/** 承認/却下後に親の設定全体を再取得させる */
	onchanged: () => void;
}
let { botId, reloadKey, onchanged }: Props = $props();

let requests = $state<MemberRequest[]>([]);
let loading = $state(true);

$effect(() => {
	void botId;
	void reloadKey;
	void load();
});

async function load() {
	loading = true;
	try {
		const res = (await botAttributeApi.memberRequests(
			botId,
			"pending",
		)) as MemberRequestsResp;
		requests = res.requests ?? [];
	} catch {
		requests = [];
	} finally {
		loading = false;
	}
}

async function decide(id: number, decision: "approved" | "rejected") {
	try {
		const res = await botAttributeApi.decideMemberRequest(id, decision);
		if (!res.success) pushToast(res.message ?? "操作に失敗しました。", "error");
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "通信エラーが発生しました。",
			"error",
		);
	}
	onchanged();
}
</script>

<section class="config-card card">
	<div class="column-header">
		<h3><span class="material-symbols-outlined header-icon-symbol">how_to_reg</span>利用申請（承認待ち）</h3>
	</div>
	<div class="section-body">
		<div class="row-list">
			{#if loading}
				<span class="field-sub">読み込み中…</span>
			{:else if requests.length === 0}
				<span class="field-sub">承認待ちの利用申請はありません。</span>
			{:else}
				{#each requests as r (r.id)}
					<div class="request-row">
						<span class="row-label">
							<span class="mono">{r.user_id}</span> @ ギルド {r.guild_id}
							{#if r.note}<br /><span class="note">📝 {r.note}</span>{/if}
						</span>
						<div class="actions">
							<AddButton onclick={() => decide(r.id, "approved")} icon="check" title="承認" />
							<DeleteButton
								onclick={() => decide(r.id, "rejected")}
								icon="close"
								title="却下"
								label="却下"
							/>
						</div>
					</div>
				{/each}
			{/if}
		</div>
	</div>
</section>

<style>
	.section-body {
		display: flex;
		flex-direction: column;
		gap: 12px;
		margin-top: 12px;
	}
	.row-list {
		display: flex;
		flex-direction: column;
		gap: 8px;
		margin-top: 8px;
	}
	.request-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
		flex-wrap: wrap;
	}
	.mono {
		font-family: var(--font-family-mono);
	}
	/* 申請行は表示名が無くIDが主役のため、.discord-tab .row-label .mono の縮小・減色を打ち消す */
	.row-label .mono {
		font-size: inherit;
		color: inherit;
	}
	.note {
		opacity: 0.8;
	}
	.actions {
		display: flex;
		gap: 4px;
	}
</style>
