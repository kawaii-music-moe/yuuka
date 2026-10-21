<script lang="ts">
// 選択中 Bot の稼働状態と起動・停止・再起動（旧 統合管理 › Bot ヘルスの 1 行分）。
// 操作はオーナー本人の Bot だけ（$lib/botRuntime）。それ以外は状態だけを出す。
import { ApiError } from "$lib/api/client";
import { botApi } from "$lib/api/services";
import type { BotView } from "$lib/api/types";
import {
	canControlRuntime,
	controlRuntime,
	type RuntimeAction,
	runtimeChip,
} from "$lib/botRuntime";
import { Button, StatusChip } from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { currentUser } from "$lib/stores/session";
import { pushToast } from "$lib/stores/toast";

let bot = $state<BotView | null>(null);
let busy = $state(false);

const chip = $derived(bot ? runtimeChip(bot) : null);
const canControl = $derived(
	!!bot && canControlRuntime(bot, $currentUser?.discordId),
);

async function load(id: string) {
	try {
		const res = await botApi.list();
		if (id !== ($activeBot?.id ?? "")) return;
		bot = (res.bots ?? []).find((b) => b.id === id) ?? null;
	} catch {
		bot = null;
	}
}

$effect(() => {
	const id = $activeBot?.id ?? "";
	bot = null;
	if (id) void load(id);
});

async function run(action: RuntimeAction) {
	if (!bot) return;
	const id = bot.id;
	busy = true;
	try {
		const wait = await controlRuntime(action, id);
		setTimeout(() => {
			busy = false;
			void load(id);
		}, wait);
	} catch (e) {
		busy = false;
		pushToast(
			e instanceof ApiError ? e.message : "エラーが発生しました",
			"error",
		);
	}
}
</script>

{#if bot && chip}
	<div class="card runtime-card">
		<div class="runtime-state">
			<span class="runtime-label">稼働状態</span>
			<StatusChip status={chip.status} label={chip.label} />
		</div>
		{#if canControl}
			<div class="runtime-actions">
				{#if bot.running}
					<Button variant="secondary" small disabled={busy} onclick={() => run("restart")}
						>再起動</Button
					>
					<Button variant="secondary" small disabled={busy} onclick={() => run("stop")}
						>停止</Button
					>
				{:else}
					<Button variant="primary" small disabled={busy} onclick={() => run("start")}
						>起動</Button
					>
				{/if}
			</div>
		{/if}
	</div>
{/if}

<style>
	.runtime-card {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		flex-wrap: wrap;
		padding: 12px 16px;
	}
	.runtime-state {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.runtime-label {
		font-weight: 600;
	}
	.runtime-actions {
		display: flex;
		gap: 8px;
	}
</style>
