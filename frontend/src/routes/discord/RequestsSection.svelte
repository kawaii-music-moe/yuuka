<script lang="ts">
	// 利用申請（承認待ち）（旧 renderAssistantRequests + decideMemberRequest）。
	import { botAttributeApi } from "$lib/api/services";
	import { ApiError } from "$lib/api/client";
	import { pushToast } from "$lib/stores/toast";
	import { Button } from "$lib/components/ui";
	import type { MemberRequest, MemberRequestsResp } from "../config/configTypes";

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
			pushToast(err instanceof ApiError ? err.message : "通信エラーが発生しました。", "error");
		}
		onchanged();
	}
</script>

<details class="form-group collapsible-group">
	<summary>利用申請（承認待ち）</summary>
	<p class="description-text sub">
		ユーザーからの利用申請を承認/却下します。承認すると利用メンバーへ追加され、申請者へ通知されます。新規申請はデフォルトBotからDMでも届きます。
	</p>
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
						<Button variant="primary" small onclick={() => decide(r.id, "approved")}>承認</Button>
						<Button variant="secondary" small onclick={() => decide(r.id, "rejected")}>却下</Button>
					</div>
				</div>
			{/each}
		{/if}
	</div>
</details>

<style>
	.sub {
		margin: 4px 0 8px;
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
		gap: 8px;
	}
</style>
