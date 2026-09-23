<script lang="ts">
// カレンダー同期設定モーダル（旧 intEditCalendars。prompt → 共通 Modal + チェックボックス一覧）。
// 開くたびに対象アカウントの利用可能カレンダーを取得し、現在の同期対象を初期選択にする。
// 保存は onsave コールバックで親に委譲（親が API を叩き再取得する）。

import { ApiError } from "$lib/api/client";
import { integratedApi } from "$lib/api/services";
import type {
	GoogleCalendarView,
	IntegratedGoogleAccountView,
} from "$lib/api/types";
import { Button, Checkbox, Modal } from "$lib/components/ui";

interface Props {
	open?: boolean;
	account: IntegratedGoogleAccountView | null;
	onsave: (payload: { accountId: number; calendars: string[] }) => void;
}

let { open = $bindable(false), account, onsave }: Props = $props();

let loading = $state(false);
let errorMsg = $state("");
let available = $state<GoogleCalendarView[]>([]);
let selected = $state<Set<string>>(new Set());

const label = $derived(
	account ? `アカウント: ${account.email || "#" + account.id}` : "",
);

// 開くたびに最新アカウントで一覧を取得し初期選択を反映（旧 openModal 直後の fetch）。
$effect(() => {
	if (!open || !account) return;
	void loadCalendars(account);
});

async function loadCalendars(acct: IntegratedGoogleAccountView) {
	loading = true;
	errorMsg = "";
	available = [];
	selected = new Set(acct.calendars ?? []);
	try {
		const res = await integratedApi.googleAccountCalendars(acct.id);
		available = res.calendars ?? [];
	} catch {
		available = [];
	} finally {
		loading = false;
	}
}

function toggle(id: string, checked: boolean) {
	const next = new Set(selected);
	if (checked) next.add(id);
	else next.delete(id);
	selected = next;
}

function save() {
	if (!account) return;
	errorMsg = "";
	try {
		onsave({ accountId: account.id, calendars: [...selected] });
	} catch (e) {
		errorMsg = e instanceof ApiError ? e.message : "保存に失敗しました。";
	}
}
</script>

<Modal bind:open title="カレンダー同期設定">
	{#if label}
		<p class="description-text int-cal-label">{label}</p>
	{/if}

	{#if loading}
		<p class="description-text">読み込み中…</p>
	{:else if available.length === 0}
		<p class="description-text">
			カレンダーを取得できませんでした。アカウントの連携状態を確認してください。
		</p>
	{:else}
		<div class="int-cal-list">
			{#each available as c (c.id)}
				<label class="glass int-cal-item">
					<Checkbox
						checked={selected.has(c.id)}
						aria-label={c.summary || c.id}
						onchange={(checked) => toggle(c.id, checked)}
					/>
					<div class="int-cal-text">
						<div class="int-cal-summary">{c.summary || c.id}</div>
						<div class="int-cal-id">{c.id}</div>
					</div>
				</label>
			{/each}
		</div>
	{/if}

	{#if errorMsg}
		<p class="error-msg int-cal-error">{errorMsg}</p>
	{/if}

	{#snippet footer()}
		<Button variant="primary" disabled={loading || available.length === 0} onclick={save}
			>保存</Button
		>
	{/snippet}
</Modal>

<style>
	.int-cal-label {
		margin-bottom: 12px;
	}
	.int-cal-list {
		display: flex;
		flex-direction: column;
		gap: 8px;
		max-height: 50vh;
		overflow-y: auto;
	}
	.int-cal-item {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px 12px;
		border-radius: 8px;
		cursor: pointer;
	}
	.int-cal-text {
		min-width: 0;
	}
	.int-cal-summary {
		font-size: 0.9rem;
	}
	.int-cal-id {
		font-size: 0.72rem;
		color: var(--color-zinc-muted, #a1a1aa);
		word-break: break-all;
	}
	.int-cal-error {
		margin-top: 8px;
	}
</style>
